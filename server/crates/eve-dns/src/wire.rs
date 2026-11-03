//! The DNS message codec: enough of the wire format to read a query and write
//! an answer, and deliberately nothing else.
//!
//! Source of truth is RFC 1035 — §4.1.1 for the header and the RCODE values,
//! §4.1.2 for the question, §4.1.3 for a resource record, §4.1.4 for name
//! compression, §2.3.4 for the size limits, §5.1 for the presentation-format
//! escapes — plus RFC 3596 §2.1 for the `AAAA` type code and RFC 2181 §8 for
//! the usable TTL range. Every constant below names the section it came from;
//! none of them were written from recall.
//!
//! This parses untrusted input. The jailed client is the only thing that can
//! reach the socket, but the entire premise of the jail (`docs/guides/jail.md`) is
//! that we do not trust what runs inside it. So every length is checked
//! against the remaining buffer, the parser never indexes a slice directly,
//! and a hostile or malformed packet produces an error rather than a panic or
//! an unbounded loop.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

/// RFC 1035 §4.1.1: the header is six 16-bit fields.
pub const HEADER_LEN: usize = 12;

/// RFC 1035 §2.3.4: "labels          63 octets or less".
pub const MAX_LABEL_LEN: usize = 63;
/// RFC 1035 §2.3.4: "names           255 octets or less". This counts the
/// length octet of every label plus the terminating root label, i.e. the
/// encoded form, not the dotted presentation form.
pub const MAX_NAME_LEN: usize = 255;
/// The terminating zero-length root label, which every encoded name carries.
const ROOT_LABEL_LEN: usize = 1;

/// RFC 1035 §2.3.4: "UDP messages    512 octets or less". We never advertise
/// EDNS0, so this is the ceiling a response must respect on UDP.
pub const MAX_UDP_PAYLOAD: usize = 512;

/// RFC 1035 §4.2.2: a message sent over TCP is prefixed by its length in two
/// octets, which is also why a TCP message cannot exceed `u16::MAX`.
pub const TCP_LENGTH_PREFIX_LEN: usize = 2;

// RFC 1035 §4.1.1, flag word bit assignments. The RFC numbers bits
// most-significant-first within the 16-bit field, so QR is the top bit:
//
//   bit 15      QR       query (0) / response (1)
//   bits 14..11 OPCODE
//   bit 10      AA       authoritative answer
//   bit 9       TC       truncated
//   bit 8       RD       recursion desired
//   bit 7       RA       recursion available
//   bits 6..4   Z        reserved, must be zero
//   bits 3..0   RCODE
/// Bit 15: set on a response.
pub const FLAG_QR: u16 = 0x8000;
/// Bits 14..11: the opcode field.
pub const OPCODE_MASK: u16 = 0x7800;
/// Shift to move the opcode field down to bit 0.
pub const OPCODE_SHIFT: u32 = 11;
/// Bit 10: authoritative answer.
pub const FLAG_AA: u16 = 0x0400;
/// Bit 9: this message was truncated.
pub const FLAG_TC: u16 = 0x0200;
/// Bit 8: recursion desired; echoed back from the query.
pub const FLAG_RD: u16 = 0x0100;
/// Bit 7: recursion available. We never set it — we are a stub with no
/// upstream, so claiming recursion would be a lie.
pub const FLAG_RA: u16 = 0x0080;
/// Bits 6..4: reserved, "Must be zero in all queries and responses".
pub const Z_MASK: u16 = 0x0070;
/// Bits 3..0: the response code.
pub const RCODE_MASK: u16 = 0x000F;

/// RFC 1035 §4.1.1 OPCODE 0: a standard query.
pub const OPCODE_QUERY: u16 = 0;

/// RFC 1035 §4.1.1 RCODE 0: no error.
pub const RCODE_NO_ERROR: u16 = 0;
/// RCODE 1: the server could not interpret the query.
pub const RCODE_FORMAT_ERROR: u16 = 1;
/// RCODE 2: server failure.
pub const RCODE_SERVER_FAILURE: u16 = 2;
/// RCODE 3: name error — authoritatively, the name does not exist. This is
/// what the RFC calls "Name Error" and everyone else calls NXDOMAIN.
pub const RCODE_NAME_ERROR: u16 = 3;
/// RCODE 4: the server does not implement the requested kind of query.
pub const RCODE_NOT_IMPLEMENTED: u16 = 4;
/// RCODE 5: the server refuses to perform the operation.
pub const RCODE_REFUSED: u16 = 5;

/// RFC 1035 §3.2.2 TYPE 1: a host address.
pub const TYPE_A: u16 = 1;
/// RFC 1035 §3.3.13 TYPE 6: start of a zone of authority.
pub const TYPE_SOA: u16 = 6;
/// RFC 3596 §2.1 TYPE 28: an IPv6 host address.
pub const TYPE_AAAA: u16 = 28;
/// RFC 1035 §3.2.3 QTYPE 255: a request for all records.
pub const QTYPE_ANY: u16 = 255;

/// RFC 1035 §3.2.4 CLASS 1: the Internet.
pub const CLASS_IN: u16 = 1;

/// A QTYPE's name, for logging. Unknown types print their number, because a
/// query for something we did not anticipate is exactly the thing worth
/// seeing in a log rather than hiding behind "other".
pub fn type_name(qtype: u16) -> String {
    match qtype {
        TYPE_A => "A".to_owned(),
        TYPE_SOA => "SOA".to_owned(),
        TYPE_AAAA => "AAAA".to_owned(),
        QTYPE_ANY => "ANY".to_owned(),
        // The handful a stock resolver also emits, named so the log reads.
        2 => "NS".to_owned(),
        5 => "CNAME".to_owned(),
        12 => "PTR".to_owned(),
        15 => "MX".to_owned(),
        16 => "TXT".to_owned(),
        33 => "SRV".to_owned(),
        41 => "OPT".to_owned(),
        65 => "HTTPS".to_owned(),
        other => format!("TYPE{other}"),
    }
}
/// RFC 1035 §3.2.5 QCLASS 255: any class.
pub const QCLASS_ANY: u16 = 255;

/// RFC 1035 §3.4.1: the RDATA of an `A` record is a single 32-bit address.
pub const A_RDLENGTH: u16 = 4;

/// RFC 1035 §4.1.4: a label length octet with its two high bits set is a
/// pointer, not a length.
const POINTER_TAG: u16 = 0xC000;
/// The low 14 bits of a pointer word are the offset.
const POINTER_OFFSET_MASK: u16 = 0x3FFF;
/// Top two bits of a length octet select label kind: 00 length, 11 pointer;
/// "The 10 and 01 combinations are reserved for future use" (RFC 1035 §4.1.4).
const LABEL_KIND_MASK: u8 = 0xC0;
const LABEL_KIND_PLAIN: u8 = 0x00;
const LABEL_KIND_POINTER: u8 = 0xC0;

/// Hard ceiling on compression-pointer jumps while parsing one name.
///
/// We already require every pointer to point strictly backwards, which makes a
/// cycle impossible on its own. This bound covers the remaining case: a legal
/// but absurd descending chain of pointers that carry no labels at all, which
/// the 255-octet name limit does not constrain because it adds no octets. A
/// real query uses zero jumps; one is all a compressed name needs.
const MAX_POINTER_JUMPS: usize = 64;

/// RFC 1035 §2.3.4 gives the TTL's range as "positive values of a signed 32
/// bit number", and RFC 2181 §8 settles the disagreement in RFC 1035 itself by
/// requiring the field be read as unsigned with the top bit zero. Either way
/// the largest value every resolver agrees on is 2^31 - 1, so we clamp rather
/// than emit something a peer may read as negative.
pub const MAX_TTL: u32 = 0x7FFF_FFFF;

/// The offset at which a response we build places the question's QNAME: the
/// question section follows the fixed-size header immediately. Answer records
/// point here instead of repeating the name (RFC 1035 §4.1.4).
const QUESTION_NAME_OFFSET: u16 = HEADER_LEN as u16;
const _: () = assert!(QUESTION_NAME_OFFSET & !POINTER_OFFSET_MASK == 0);

// A response to the largest question we will accept still fits in a UDP
// message: header + the longest legal name + QTYPE/QCLASS + one answer
// (2-byte name pointer, TYPE, CLASS, TTL, RDLENGTH, 4-byte address).
const _: () = assert!(HEADER_LEN + MAX_NAME_LEN + 4 + 2 + 2 + 2 + 4 + 2 + 4 <= MAX_UDP_PAYLOAD);

/// Everything that can go wrong reading or building a message.
///
/// Dedicated to this layer: it knows about octets and labels and nothing about
/// the matching policy above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsError {
    /// A field ran off the end of the buffer.
    Truncated { need: usize, have: usize },
    /// A label length octet used one of the two reserved encodings (top bits
    /// 01 or 10), which nothing we speak uses.
    ReservedLabel { byte: u8 },
    /// A compression pointer outside the message, or one that does not point
    /// strictly backwards as RFC 1035 §4.1.4 requires.
    BadPointer { target: usize, from: usize },
    /// A pointer chain longer than [`MAX_POINTER_JUMPS`].
    PointerLoop,
    /// A label longer than [`MAX_LABEL_LEN`].
    LabelTooLong { len: usize },
    /// An encoded name longer than [`MAX_NAME_LEN`].
    NameTooLong { len: usize },
    /// A zero-length label somewhere other than the end of the name.
    EmptyLabel,
    /// A configured name with a byte outside ASCII. An internationalised name
    /// must be given in its A-label (punycode) form.
    NotAscii,
    /// The message has its QR bit set: it is a response, not a query.
    NotAQuery,
    /// We serve exactly one question per message, as every stub resolver sends.
    UnexpectedQuestionCount { qdcount: u16 },
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsError::Truncated { need, have } => {
                write!(f, "message truncated: need {need} bytes, have {have}")
            }
            DnsError::ReservedLabel { byte } => {
                write!(f, "reserved label kind in length octet {byte:#04x}")
            }
            DnsError::BadPointer { target, from } => {
                write!(
                    f,
                    "compression pointer at {from} to invalid offset {target}"
                )
            }
            DnsError::PointerLoop => write!(f, "compression pointer chain too long"),
            DnsError::LabelTooLong { len } => write!(f, "label of {len} bytes exceeds 63"),
            DnsError::NameTooLong { len } => write!(f, "name of {len} bytes exceeds 255"),
            DnsError::EmptyLabel => write!(f, "empty label inside a name"),
            DnsError::NotAscii => write!(f, "name contains a non-ASCII byte"),
            DnsError::NotAQuery => write!(f, "message is a response, not a query"),
            DnsError::UnexpectedQuestionCount { qdcount } => {
                write!(f, "qdcount is {qdcount}, expected exactly 1")
            }
        }
    }
}

impl std::error::Error for DnsError {}

/// Read a big-endian `u16` at `pos`, bounds-checked.
fn read_u16(msg: &[u8], pos: usize) -> Result<u16, DnsError> {
    let bytes = msg.get(pos..pos + 2).ok_or(DnsError::Truncated {
        need: pos + 2,
        have: msg.len(),
    })?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// The 12-byte message header (RFC 1035 §4.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub id: u16,
    /// The raw flag word. Use the accessors rather than masking at call sites.
    pub flags: u16,
    pub qdcount: u16,
    pub ancount: u16,
    pub nscount: u16,
    pub arcount: u16,
}

impl Header {
    /// Parse the header from the front of `msg`.
    pub fn parse(msg: &[u8]) -> Result<Self, DnsError> {
        if msg.len() < HEADER_LEN {
            return Err(DnsError::Truncated {
                need: HEADER_LEN,
                have: msg.len(),
            });
        }
        Ok(Header {
            id: read_u16(msg, 0)?,
            flags: read_u16(msg, 2)?,
            qdcount: read_u16(msg, 4)?,
            ancount: read_u16(msg, 6)?,
            nscount: read_u16(msg, 8)?,
            arcount: read_u16(msg, 10)?,
        })
    }

    /// The header as it goes on the wire.
    pub fn to_bytes(self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..2].copy_from_slice(&self.id.to_be_bytes());
        out[2..4].copy_from_slice(&self.flags.to_be_bytes());
        out[4..6].copy_from_slice(&self.qdcount.to_be_bytes());
        out[6..8].copy_from_slice(&self.ancount.to_be_bytes());
        out[8..10].copy_from_slice(&self.nscount.to_be_bytes());
        out[10..12].copy_from_slice(&self.arcount.to_be_bytes());
        out
    }

    /// True if this is a response (QR set).
    pub fn qr(self) -> bool {
        self.flags & FLAG_QR != 0
    }

    /// The opcode, shifted down to bit 0.
    pub fn opcode(self) -> u16 {
        (self.flags & OPCODE_MASK) >> OPCODE_SHIFT
    }

    /// True if the querier asked for recursion.
    pub fn rd(self) -> bool {
        self.flags & FLAG_RD != 0
    }

    /// True if the message was marked truncated.
    pub fn tc(self) -> bool {
        self.flags & FLAG_TC != 0
    }

    /// The response code.
    pub fn rcode(self) -> u16 {
        self.flags & RCODE_MASK
    }
}

/// A domain name as a sequence of labels, with no trailing root label stored.
///
/// Labels keep the bytes they arrived as. DNS comparison is case-insensitive
/// (RFC 1035 §2.3.3) but a response must echo the question exactly as asked,
/// so we preserve case on the wire and fold it only when comparing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Name {
    labels: Vec<Vec<u8>>,
}

impl Name {
    /// The root name, `.`.
    pub fn root() -> Self {
        Name { labels: Vec::new() }
    }

    /// Build a name from its presentation form, e.g. `servers.eveonline.com`.
    ///
    /// A trailing dot is accepted and ignored — every name here is absolute.
    /// ASCII only, so that case folding is well defined.
    pub fn from_ascii(s: &str) -> Result<Self, DnsError> {
        if !s.is_ascii() {
            return Err(DnsError::NotAscii);
        }
        let trimmed = s.strip_suffix('.').unwrap_or(s);
        if trimmed.is_empty() {
            return Ok(Name::root());
        }
        let mut labels = Vec::new();
        let mut encoded = ROOT_LABEL_LEN;
        for part in trimmed.split('.') {
            if part.is_empty() {
                return Err(DnsError::EmptyLabel);
            }
            if part.len() > MAX_LABEL_LEN {
                return Err(DnsError::LabelTooLong { len: part.len() });
            }
            encoded += 1 + part.len();
            if encoded > MAX_NAME_LEN {
                return Err(DnsError::NameTooLong { len: encoded });
            }
            labels.push(part.as_bytes().to_vec());
        }
        Ok(Name { labels })
    }

    /// Parse the name starting at `pos` in `msg`, following compression
    /// pointers.
    ///
    /// Returns the name and the offset just past the name *as encoded at
    /// `pos`* — that is, past the terminating root label or past the first
    /// pointer, which is where the next field of the record begins.
    pub fn parse(msg: &[u8], pos: usize) -> Result<(Self, usize), DnsError> {
        let mut labels: Vec<Vec<u8>> = Vec::new();
        let mut at = pos;
        let mut end: Option<usize> = None;
        let mut jumps = 0usize;
        let mut encoded = ROOT_LABEL_LEN;

        loop {
            let octet = *msg.get(at).ok_or(DnsError::Truncated {
                need: at + 1,
                have: msg.len(),
            })?;
            match octet & LABEL_KIND_MASK {
                LABEL_KIND_PLAIN => {
                    // Top bits are 00, so the length cannot exceed 63 and
                    // MAX_LABEL_LEN is structurally guaranteed here.
                    let len = usize::from(octet);
                    at += 1;
                    if len == 0 {
                        return Ok((Name { labels }, end.unwrap_or(at)));
                    }
                    encoded += 1 + len;
                    if encoded > MAX_NAME_LEN {
                        return Err(DnsError::NameTooLong { len: encoded });
                    }
                    let label = msg.get(at..at + len).ok_or(DnsError::Truncated {
                        need: at + len,
                        have: msg.len(),
                    })?;
                    labels.push(label.to_vec());
                    at += len;
                }
                LABEL_KIND_POINTER => {
                    let word = read_u16(msg, at)?;
                    let target = usize::from(word & POINTER_OFFSET_MASK);
                    // The name as encoded here stops after the pointer word.
                    if end.is_none() {
                        end = Some(at + 2);
                    }
                    jumps += 1;
                    if jumps > MAX_POINTER_JUMPS {
                        return Err(DnsError::PointerLoop);
                    }
                    // RFC 1035 §4.1.4: a pointer names "a prior occurrence of
                    // the same name". Requiring strictly backwards movement is
                    // what makes a cycle impossible, so it is enforced, not
                    // merely preferred.
                    if target >= at || target >= msg.len() {
                        return Err(DnsError::BadPointer { target, from: at });
                    }
                    at = target;
                }
                reserved => return Err(DnsError::ReservedLabel { byte: reserved }),
            }
        }
    }

    /// The labels, outermost first, without the root label.
    pub fn labels(&self) -> &[Vec<u8>] {
        &self.labels
    }

    /// True for the root name.
    pub fn is_root(&self) -> bool {
        self.labels.is_empty()
    }

    /// Bytes this name occupies when written uncompressed.
    pub fn encoded_len(&self) -> usize {
        self.labels.iter().map(|l| 1 + l.len()).sum::<usize>() + ROOT_LABEL_LEN
    }

    /// Append the uncompressed encoding: each label length-prefixed, then the
    /// root label.
    pub fn write(&self, out: &mut Vec<u8>) {
        for label in &self.labels {
            // Lengths are capped at construction and at parse, so this cast
            // cannot lose information.
            out.push(label.len() as u8);
            out.extend_from_slice(label);
        }
        out.push(0);
    }

    /// Case-insensitive equality, per RFC 1035 §2.3.3.
    pub fn eq_ignore_case(&self, other: &Name) -> bool {
        self.labels.len() == other.labels.len()
            && self
                .labels
                .iter()
                .zip(&other.labels)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }

    /// True if `self` is `suffix` or a name beneath it, compared label by
    /// label and case-insensitively.
    ///
    /// Label-wise is the point: a textual suffix test would make
    /// `noteveonline.com` a match for `eveonline.com`. The apex itself counts
    /// as a match, so configuring `servers.eveonline.com` answers both
    /// `tranquility.servers.eveonline.com` and the bare apex.
    pub fn is_subdomain_of(&self, suffix: &Name) -> bool {
        if suffix.labels.len() > self.labels.len() {
            return false;
        }
        let skip = self.labels.len() - suffix.labels.len();
        self.labels[skip..]
            .iter()
            .zip(&suffix.labels)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }
}

impl FromStr for Name {
    type Err = DnsError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Name::from_ascii(s)
    }
}

impl fmt::Display for Name {
    /// Presentation form, always absolute (trailing dot), with RFC 1035 §5.1
    /// escapes for bytes that are not printable ASCII.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.labels.is_empty() {
            return f.write_str(".");
        }
        for label in &self.labels {
            for &b in label {
                match b {
                    b'.' | b'\\' => {
                        let c = b as char;
                        write!(f, "\\{c}")?;
                    }
                    0x21..=0x7E => {
                        let c = b as char;
                        write!(f, "{c}")?;
                    }
                    _ => write!(f, "\\{b:03}")?,
                }
            }
            f.write_str(".")?;
        }
        Ok(())
    }
}

/// One entry of the question section (RFC 1035 §4.1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub name: Name,
    pub qtype: u16,
    pub qclass: u16,
}

impl Question {
    /// Parse a question starting at `pos`, returning it and the offset just
    /// past it.
    pub fn parse(msg: &[u8], pos: usize) -> Result<(Self, usize), DnsError> {
        let (name, after) = Name::parse(msg, pos)?;
        let qtype = read_u16(msg, after)?;
        let qclass = read_u16(msg, after + 2)?;
        Ok((
            Question {
                name,
                qtype,
                qclass,
            },
            after + 4,
        ))
    }

    /// Append the question, with the name uncompressed.
    pub fn write(&self, out: &mut Vec<u8>) {
        self.name.write(out);
        out.extend_from_slice(&self.qtype.to_be_bytes());
        out.extend_from_slice(&self.qclass.to_be_bytes());
    }

    /// Bytes this question occupies when written.
    pub fn encoded_len(&self) -> usize {
        self.name.encoded_len() + 4
    }
}

/// A parsed query: the header plus its single question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub header: Header,
    pub question: Question,
}

/// Parse a standard query carrying exactly one question.
///
/// Records after the question — an EDNS0 `OPT` in the additional section, for
/// instance — are not parsed. We do not implement EDNS0, and a resolver that
/// gets a reply with no `OPT` simply falls back to plain DNS, which is the
/// documented behaviour in RFC 6891 §6.1.1 and is fine here because every
/// answer we produce is far below 512 bytes.
pub fn parse_query(msg: &[u8]) -> Result<Query, DnsError> {
    let header = Header::parse(msg)?;
    if header.qr() {
        return Err(DnsError::NotAQuery);
    }
    if header.qdcount != 1 {
        return Err(DnsError::UnexpectedQuestionCount {
            qdcount: header.qdcount,
        });
    }
    let (question, _) = Question::parse(msg, HEADER_LEN)?;
    Ok(Query { header, question })
}

/// One `A` record to emit in the answer section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub ttl: u32,
    pub addr: Ipv4Addr,
}

/// Append a compression pointer to `offset` (RFC 1035 §4.1.4).
fn write_pointer(offset: u16, out: &mut Vec<u8>) {
    debug_assert!(offset & !POINTER_OFFSET_MASK == 0);
    out.extend_from_slice(&(POINTER_TAG | (offset & POINTER_OFFSET_MASK)).to_be_bytes());
}

/// Response flags derived from a query's: QR and AA set, the opcode and RD
/// echoed, RA clear because we cannot recurse, Z clear because the RFC
/// requires it, and `rcode` in the low nibble.
fn response_flags(query: &Header, rcode: u16) -> u16 {
    let echoed = query.flags & (OPCODE_MASK | FLAG_RD);
    FLAG_QR | FLAG_AA | echoed | (rcode & RCODE_MASK)
}

/// Build a response echoing `question` and carrying `answers`.
///
/// `answers` are emitted as `A` records in class IN whose owner name is a
/// compression pointer back to the question. At most `u16::MAX` are written,
/// since ANCOUNT cannot express more; the policy layer never passes more than
/// one. TTLs are clamped to [`MAX_TTL`].
///
/// If the result would exceed [`MAX_UDP_PAYLOAD`] the answer section is
/// dropped and TC is set, which is the RFC 1035 §4.2.1 behaviour that makes a
/// resolver retry over TCP. The const assertion above shows this cannot
/// trigger for a single answer to a legal question, which is all the policy
/// layer ever asks for; it exists so that a future caller with a longer answer
/// section cannot produce an oversized datagram. The limit is applied even
/// when the message is bound for TCP, where it does not apply — a deliberate
/// simplification, since one message shape is easier to reason about than two
/// and the bound is unreachable either way.
pub fn build_response(
    query: &Header,
    question: &Question,
    rcode: u16,
    answers: &[Answer],
) -> Vec<u8> {
    let ancount = u16::try_from(answers.len()).unwrap_or(u16::MAX);

    let mut body = Vec::with_capacity(question.encoded_len() + answers.len() * 16);
    question.write(&mut body);
    let question_end = body.len();
    for answer in answers.iter().take(usize::from(ancount)) {
        write_pointer(QUESTION_NAME_OFFSET, &mut body);
        body.extend_from_slice(&TYPE_A.to_be_bytes());
        body.extend_from_slice(&CLASS_IN.to_be_bytes());
        body.extend_from_slice(&answer.ttl.min(MAX_TTL).to_be_bytes());
        body.extend_from_slice(&A_RDLENGTH.to_be_bytes());
        body.extend_from_slice(&answer.addr.octets());
    }

    let mut flags = response_flags(query, rcode);
    let mut ancount = ancount;
    if HEADER_LEN + body.len() > MAX_UDP_PAYLOAD {
        body.truncate(question_end);
        flags |= FLAG_TC;
        ancount = 0;
    }

    let header = Header {
        id: query.id,
        flags,
        qdcount: 1,
        ancount,
        nscount: 0,
        arcount: 0,
    };
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&body);
    out
}

/// Build a response with no question section, for a message we could not
/// parse far enough to echo one.
///
/// Omitting the question is what RFC 1035 §4.1.2 leaves us: QDCOUNT is the
/// number of entries actually present, and we have none we can trust. A
/// resolver matches the reply by ID and, finding a non-zero RCODE, gives up on
/// this server for that query — which is the outcome we want for a malformed
/// packet.
pub fn build_bare_response(query: &Header, rcode: u16) -> Vec<u8> {
    let header = Header {
        id: query.id,
        flags: response_flags(query, rcode),
        qdcount: 0,
        ancount: 0,
        nscount: 0,
        arcount: 0,
    };
    header.to_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a query the way a stub resolver does, so tests exercise real
    /// input rather than our own encoder's idea of it.
    fn query_bytes(id: u16, name: &str, qtype: u16, qclass: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(
            &Header {
                id,
                flags: FLAG_RD,
                qdcount: 1,
                ancount: 0,
                nscount: 0,
                arcount: 0,
            }
            .to_bytes(),
        );
        Question {
            name: Name::from_ascii(name).unwrap(),
            qtype,
            qclass,
        }
        .write(&mut out);
        out
    }

    #[test]
    fn flag_bits_are_where_rfc1035_4_1_1_puts_them() {
        // Bit numbering in the RFC is most-significant-first, so QR is 0x8000
        // and RCODE is the low nibble. This test exists because getting this
        // backwards produces a server that looks almost right.
        assert_eq!(FLAG_QR, 1 << 15);
        assert_eq!(OPCODE_MASK, 0b0111_1000 << 8);
        assert_eq!(FLAG_AA, 1 << 10);
        assert_eq!(FLAG_TC, 1 << 9);
        assert_eq!(FLAG_RD, 1 << 8);
        assert_eq!(FLAG_RA, 1 << 7);
        assert_eq!(Z_MASK, 0b0111 << 4);
        assert_eq!(RCODE_MASK, 0b1111);
        // The eight flag-word fields must tile the word exactly once.
        assert_eq!(
            FLAG_QR | OPCODE_MASK | FLAG_AA | FLAG_TC | FLAG_RD | FLAG_RA | Z_MASK | RCODE_MASK,
            u16::MAX
        );
        // And the RCODE values are the RFC's, in order.
        assert_eq!(
            [
                RCODE_NO_ERROR,
                RCODE_FORMAT_ERROR,
                RCODE_SERVER_FAILURE,
                RCODE_NAME_ERROR,
                RCODE_NOT_IMPLEMENTED,
                RCODE_REFUSED
            ],
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn a_header_round_trips() {
        let h = Header {
            id: 0xBEEF,
            flags: FLAG_QR | FLAG_AA | FLAG_RD | RCODE_NAME_ERROR,
            qdcount: 1,
            ancount: 2,
            nscount: 3,
            arcount: 4,
        };
        let bytes = h.to_bytes();
        assert_eq!(
            bytes[0..2],
            [0xBE, 0xEF],
            "the ID is big-endian on the wire"
        );
        assert_eq!(Header::parse(&bytes), Ok(h));
        assert!(h.qr());
        assert_eq!(h.opcode(), OPCODE_QUERY);
        assert!(h.rd());
        assert_eq!(h.rcode(), RCODE_NAME_ERROR);
    }

    #[test]
    fn a_name_round_trips_through_the_wire_form() {
        let name = Name::from_ascii("tranquility.servers.eveonline.com").unwrap();
        let mut buf = Vec::new();
        name.write(&mut buf);
        assert_eq!(buf[0], 11, "first label is length-prefixed");
        assert_eq!(*buf.last().unwrap(), 0, "and the name ends at the root");
        assert_eq!(buf.len(), name.encoded_len());
        let (parsed, end) = Name::parse(&buf, 0).unwrap();
        assert_eq!(parsed, name);
        assert_eq!(end, buf.len());
        assert_eq!(parsed.to_string(), "tranquility.servers.eveonline.com.");
    }

    #[test]
    fn a_query_round_trips() {
        let raw = query_bytes(
            0x1234,
            "tranquility.servers.eveonline.com",
            TYPE_A,
            CLASS_IN,
        );
        let q = parse_query(&raw).unwrap();
        assert_eq!(q.header.id, 0x1234);
        assert!(!q.header.qr());
        assert_eq!(q.question.qtype, TYPE_A);
        assert_eq!(q.question.qclass, CLASS_IN);
        let shouted = Name::from_ascii("TRANQUILITY.Servers.EveOnline.COM").unwrap();
        assert_ne!(
            q.question.name, shouted,
            "derived equality is byte-wise: the query's case is preserved verbatim \
             so that the response can echo it exactly"
        );
        assert!(
            q.question.name.eq_ignore_case(&shouted),
            "RFC 1035 2.3.3: comparison itself is case-insensitive"
        );
    }

    #[test]
    fn the_root_name_is_a_single_zero_octet() {
        let mut buf = Vec::new();
        Name::root().write(&mut buf);
        assert_eq!(buf, vec![0]);
        assert_eq!(Name::parse(&buf, 0).unwrap().0, Name::root());
        assert_eq!(Name::root().to_string(), ".");
        assert!(Name::from_ascii(".").unwrap().is_root());
        assert!(Name::from_ascii("").unwrap().is_root());
    }

    #[test]
    fn a_truncated_message_is_an_error_not_a_panic() {
        let raw = query_bytes(1, "tranquility.servers.eveonline.com", TYPE_A, CLASS_IN);
        for n in 0..raw.len() {
            let err = parse_query(&raw[..n]).unwrap_err();
            assert!(
                matches!(err, DnsError::Truncated { .. }),
                "{n} bytes gave {err:?}"
            );
        }
        assert!(parse_query(&raw).is_ok());
    }

    #[test]
    fn a_label_running_off_the_end_is_an_error() {
        // Length octet claims 60 bytes with 3 available.
        let mut raw = Header {
            id: 1,
            flags: 0,
            qdcount: 1,
            ancount: 0,
            nscount: 0,
            arcount: 0,
        }
        .to_bytes()
        .to_vec();
        raw.extend_from_slice(&[60, b'a', b'b', b'c']);
        assert!(matches!(parse_query(&raw), Err(DnsError::Truncated { .. })));
    }

    #[test]
    fn a_reserved_label_kind_is_rejected() {
        // Top bits 01 and 10 are neither a length nor a pointer. A length
        // octet can therefore never encode more than 63, which is why a
        // parsed label cannot be over-long -- the over-long case is only
        // reachable through from_ascii, tested separately.
        for bogus in [0x40u8, 0x80, 0xBF] {
            let mut msg = vec![0u8; HEADER_LEN];
            msg[5] = 1; // qdcount = 1
            msg.extend_from_slice(&[bogus, 0, 0, 0, 0]);
            assert_eq!(
                parse_query(&msg),
                Err(DnsError::ReservedLabel {
                    byte: bogus & LABEL_KIND_MASK
                }),
                "length octet {bogus:#04x}"
            );
        }
    }

    #[test]
    fn a_compression_pointer_is_followed() {
        // "b.example" at offset 0, then "a" + a pointer to offset 2
        // ("example") at offset 10.
        let mut msg = Vec::new();
        Name::from_ascii("b.example").unwrap().write(&mut msg);
        assert_eq!(msg.len(), 11);
        let at = msg.len();
        msg.push(1);
        msg.push(b'a');
        write_pointer(2, &mut msg);
        let (name, end) = Name::parse(&msg, at).unwrap();
        assert_eq!(name, Name::from_ascii("a.example").unwrap());
        assert_eq!(end, msg.len(), "the name ends after the pointer word");
    }

    #[test]
    fn a_pointer_loop_cannot_hang_the_parser() {
        // The classic hostile packet: a pointer to itself.
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1;
        write_pointer(HEADER_LEN as u16, &mut msg);
        msg.extend_from_slice(&[0, 0, 0, 0]);
        assert!(
            matches!(parse_query(&msg), Err(DnsError::BadPointer { .. })),
            "a self-pointer is not strictly backwards"
        );

        // A two-pointer cycle, forwards then backwards.
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1;
        write_pointer(HEADER_LEN as u16 + 2, &mut msg);
        write_pointer(HEADER_LEN as u16, &mut msg);
        assert!(matches!(
            parse_query(&msg),
            Err(DnsError::BadPointer { .. })
        ));
    }

    #[test]
    fn a_long_backwards_pointer_chain_is_bounded() {
        // Every pointer here is strictly backwards, so there is no cycle, and
        // it carries no labels, so the 255-octet name limit never trips. Only
        // MAX_POINTER_JUMPS stops it.
        let chain = MAX_POINTER_JUMPS + 10;
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1;
        // msg[HEADER_LEN..] is a descending chain: slot i points at slot i-1.
        msg.extend_from_slice(&[0, 0]); // slot 0: the root label, padded
        for i in 1..=chain {
            write_pointer((HEADER_LEN + (i - 1) * 2) as u16, &mut msg);
        }
        let start = HEADER_LEN + chain * 2;
        assert_eq!(Name::parse(&msg, start), Err(DnsError::PointerLoop));
    }

    #[test]
    fn a_pointer_past_the_end_of_the_message_is_rejected() {
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1;
        msg.extend_from_slice(&[0xC0, 0xFF]); // offset 255, past the end
        assert!(matches!(
            parse_query(&msg),
            Err(DnsError::BadPointer { .. })
        ));
    }

    #[test]
    fn a_label_over_63_bytes_is_rejected() {
        let long = "a".repeat(MAX_LABEL_LEN + 1);
        assert_eq!(
            Name::from_ascii(&format!("{long}.example.com")),
            Err(DnsError::LabelTooLong {
                len: MAX_LABEL_LEN + 1
            })
        );
        assert!(Name::from_ascii(&format!("{}.example.com", "a".repeat(MAX_LABEL_LEN))).is_ok());
    }

    #[test]
    fn a_name_over_255_bytes_is_rejected_on_both_paths() {
        // 4 labels of 63 bytes encode to 4*64 + 1 = 257 octets.
        let label = "a".repeat(MAX_LABEL_LEN);
        let text = [label.as_str(); 4].join(".");
        assert!(matches!(
            Name::from_ascii(&text),
            Err(DnsError::NameTooLong { .. })
        ));

        // And the same name hand-built on the wire, which from_ascii never
        // produced, must also be refused rather than parsed.
        let mut msg = vec![0u8; HEADER_LEN];
        msg[5] = 1;
        for _ in 0..4 {
            msg.push(MAX_LABEL_LEN as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.extend_from_slice(&[0, 0, TYPE_A as u8, 0, CLASS_IN as u8]);
        assert!(matches!(
            parse_query(&msg),
            Err(DnsError::NameTooLong { .. })
        ));
    }

    #[test]
    fn an_empty_interior_label_and_non_ascii_are_rejected() {
        assert_eq!(Name::from_ascii("a..b"), Err(DnsError::EmptyLabel));
        assert_eq!(Name::from_ascii(".leading"), Err(DnsError::EmptyLabel));
        assert_eq!(Name::from_ascii("evé.com"), Err(DnsError::NotAscii));
    }

    #[test]
    fn a_response_is_not_treated_as_a_query() {
        let mut raw = query_bytes(7, "a.example", TYPE_A, CLASS_IN);
        let flags = u16::from_be_bytes([raw[2], raw[3]]) | FLAG_QR;
        raw[2..4].copy_from_slice(&flags.to_be_bytes());
        assert_eq!(parse_query(&raw), Err(DnsError::NotAQuery));
    }

    #[test]
    fn a_question_count_other_than_one_is_rejected() {
        for qdcount in [0u16, 2, u16::MAX] {
            let mut raw = query_bytes(7, "a.example", TYPE_A, CLASS_IN);
            raw[4..6].copy_from_slice(&qdcount.to_be_bytes());
            assert_eq!(
                parse_query(&raw),
                Err(DnsError::UnexpectedQuestionCount { qdcount })
            );
        }
    }

    #[test]
    fn subdomain_matching_is_label_wise_not_textual() {
        let suffix = Name::from_ascii("servers.eveonline.com").unwrap();
        for yes in [
            "servers.eveonline.com",
            "SERVERS.EVEONLINE.COM",
            "tranquility.servers.eveonline.com",
            "a.b.servers.eveonline.com",
        ] {
            assert!(
                Name::from_ascii(yes).unwrap().is_subdomain_of(&suffix),
                "{yes} should match"
            );
        }
        for no in [
            "eveonline.com",
            "notservers.eveonline.com",
            "servers.eveonline.com.evil.test",
            "servers.noteveonline.com",
            "",
        ] {
            assert!(
                !Name::from_ascii(no).unwrap().is_subdomain_of(&suffix),
                "{no} should not match"
            );
        }
    }

    #[test]
    fn an_answer_names_the_question_by_pointer_and_round_trips() {
        let q = parse_query(&query_bytes(
            0x4321,
            "tranquility.servers.eveonline.com",
            TYPE_A,
            CLASS_IN,
        ))
        .unwrap();
        let out = build_response(
            &q.header,
            &q.question,
            RCODE_NO_ERROR,
            &[Answer {
                ttl: 60,
                addr: Ipv4Addr::new(10, 137, 0, 1),
            }],
        );

        let h = Header::parse(&out).unwrap();
        assert_eq!(h.id, 0x4321);
        assert!(h.qr() && !h.tc());
        assert_eq!(h.flags & FLAG_AA, FLAG_AA, "we answer authoritatively");
        assert_eq!(h.flags & FLAG_RA, 0, "we never claim recursion");
        assert_eq!(h.flags & Z_MASK, 0, "RFC 1035 4.1.1: Z must be zero");
        assert_eq!(h.flags & FLAG_RD, FLAG_RD, "RD is echoed");
        assert_eq!(h.rcode(), RCODE_NO_ERROR);
        assert_eq!((h.qdcount, h.ancount, h.nscount, h.arcount), (1, 1, 0, 0));

        // The question is echoed verbatim, then one A record.
        let (echoed, after) = Question::parse(&out, HEADER_LEN).unwrap();
        assert_eq!(echoed, q.question);
        assert_eq!(out[after], 0xC0, "the answer's owner name is a pointer");
        assert_eq!(out[after + 1], HEADER_LEN as u8);
        // Parsing the pointer must yield the question's name back.
        let (owner, next) = Name::parse(&out, after).unwrap();
        assert_eq!(owner, q.question.name);
        assert_eq!(next, after + 2);
        assert_eq!(read_u16(&out, next).unwrap(), TYPE_A);
        assert_eq!(read_u16(&out, next + 2).unwrap(), CLASS_IN);
        assert_eq!(
            u32::from_be_bytes(out[next + 4..next + 8].try_into().unwrap()),
            60
        );
        assert_eq!(read_u16(&out, next + 8).unwrap(), A_RDLENGTH);
        assert_eq!(&out[next + 10..], &[10, 137, 0, 1]);
        assert_eq!(out.len(), next + 14);
    }

    #[test]
    fn a_ttl_is_clamped_to_the_rfc2181_maximum() {
        let q = parse_query(&query_bytes(1, "a.example", TYPE_A, CLASS_IN)).unwrap();
        let out = build_response(
            &q.header,
            &q.question,
            RCODE_NO_ERROR,
            &[Answer {
                ttl: u32::MAX,
                addr: Ipv4Addr::LOCALHOST,
            }],
        );
        let ttl_at = out.len() - 4 - 2 - 4;
        assert_eq!(
            u32::from_be_bytes(out[ttl_at..ttl_at + 4].try_into().unwrap()),
            MAX_TTL,
            "a TTL with the top bit set reads as negative to some resolvers"
        );
    }

    #[test]
    fn an_oversized_answer_section_sets_tc_instead_of_overflowing_udp() {
        let q = parse_query(&query_bytes(1, "a.example", TYPE_A, CLASS_IN)).unwrap();
        let many = vec![
            Answer {
                ttl: 1,
                addr: Ipv4Addr::LOCALHOST,
            };
            64
        ];
        let out = build_response(&q.header, &q.question, RCODE_NO_ERROR, &many);
        let h = Header::parse(&out).unwrap();
        assert!(h.tc());
        assert_eq!(h.ancount, 0);
        assert!(out.len() <= MAX_UDP_PAYLOAD);
    }

    #[test]
    fn a_bare_response_carries_the_id_and_rcode_only() {
        let h = Header {
            id: 0x0F0F,
            flags: FLAG_RD,
            qdcount: 9,
            ancount: 0,
            nscount: 0,
            arcount: 0,
        };
        let out = build_bare_response(&h, RCODE_FORMAT_ERROR);
        assert_eq!(out.len(), HEADER_LEN);
        let r = Header::parse(&out).unwrap();
        assert_eq!(r.id, 0x0F0F);
        assert!(r.qr());
        assert_eq!(r.rcode(), RCODE_FORMAT_ERROR);
        assert_eq!(r.qdcount, 0, "we echo no question we could not parse");
    }

    #[test]
    fn errors_display_without_panicking() {
        for e in [
            DnsError::Truncated { need: 12, have: 3 },
            DnsError::ReservedLabel { byte: 0x40 },
            DnsError::BadPointer {
                target: 99,
                from: 12,
            },
            DnsError::PointerLoop,
            DnsError::LabelTooLong { len: 64 },
            DnsError::NameTooLong { len: 300 },
            DnsError::EmptyLabel,
            DnsError::NotAscii,
            DnsError::NotAQuery,
            DnsError::UnexpectedQuestionCount { qdcount: 0 },
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
