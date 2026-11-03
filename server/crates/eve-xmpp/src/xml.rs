//! The XML subset an XMPP stream needs, and nothing more.
//!
//! **Accepted:** start, end and empty tags; attributes in `'` or `"`; text;
//! the five predefined entities and `&#N;` / `&#xN;`; one `<?xml ...?>`
//! declaration before the stream header.
//!
//! **Rejected, not skipped:** comments, CDATA, DTDs and any other processing
//! instruction. RFC 6120 §11.1 forbids all of them on a stream, and a parser
//! that skipped them would be a parser with a mode nobody tests.
//!
//! Names are **qualified names, literally** (`stream:stream`, `xml:lang`).
//! There is no namespace processing, because the client's own parser runs in
//! non-namespace mode (`docs/lanes/chat.md` §3 rule 2) and the server must
//! match tag names the way the client writes and reads them. `xmlns` is an
//! ordinary attribute here.

use std::fmt;

/// One element: its name, attributes in document order, element children,
/// and all of its character data concatenated.
///
/// Mixed content is flattened the way the client's own `XmppElement` does it
/// (`characters()` appends to one `text`): nothing in XMPP interleaves text
/// and child elements meaningfully.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Element {
            name: name.to_owned(),
            ..Element::default()
        }
    }

    /// Builder: append an attribute. Order is kept on output.
    pub fn with_attr(mut self, key: &str, value: impl Into<String>) -> Self {
        self.attrs.push((key.to_owned(), value.into()));
        self
    }

    /// Builder: append a child element.
    pub fn with_child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    /// Builder: set the character data.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// The first attribute with this name.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The first **direct** child with this name -- one level only, like the
    /// client's `find_child_with_tag`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Serialise with **no whitespace anywhere** that is not in the data.
    ///
    /// Not cosmetic: a single byte of character data between two top-level
    /// stanzas raises `AttributeError` inside the client's SAX handler and
    /// kills its parser for the rest of the stream (`docs/lanes/chat.md` §3
    /// rule 1). Attributes are single-quoted, which is what the client sends.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("='");
            out.push_str(&escape(v));
            out.push('\'');
        }
        if self.children.is_empty() && self.text.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        out.push_str(&escape(&self.text));
        for c in &self.children {
            c.write_into(out);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push('>');
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_xml())
    }
}

/// Escape `& < > " '` for text or an attribute value.
///
/// All five, everywhere: the client builds some of its own stanzas with
/// `str.format` into single-quoted attributes, so we cannot assume which
/// quote a value will end up inside if it is ever echoed.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Why a stream was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    /// A construct this subset refuses on purpose: comment, CDATA, DTD, PI.
    Rejected(&'static str),
    /// Not well-formed XML.
    Malformed(String),
    /// More buffered bytes than [`Framer::LIMIT`] without a complete stanza.
    TooLarge,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            XmlError::Rejected(what) => write!(f, "rejected: {what}"),
            XmlError::Malformed(why) => write!(f, "malformed: {why}"),
            XmlError::TooLarge => f.write_str("stanza too large"),
        }
    }
}

impl std::error::Error for XmlError {}

fn malformed(why: impl Into<String>) -> XmlError {
    XmlError::Malformed(why.into())
}

/// What a stream yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The `<stream:stream ...>` open tag. Children are always empty.
    Open(Element),
    /// One complete depth-1 element.
    Stanza(Element),
    /// `</stream:stream>`.
    Close,
}

/// `Ok(None)` is "need more bytes"; `Ok(Some((value, next_position)))`.
type Step<T> = Result<Option<(T, usize)>, XmlError>;

/// An incremental stream framer: bytes in, [`Event`]s out.
///
/// Feed it whatever `read` returned, then call [`Framer::next_event`] until
/// it says `None`. Splitting the input at any byte gives the same events;
/// the tokenizer re-reads an incomplete item from its start next time rather
/// than keeping a half-parsed state, which at stanza sizes costs nothing and
/// leaves no state machine to get wrong.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
    open: bool,
}

/// Elements nested deeper than this are refused. XMPP stanzas use four.
const MAX_DEPTH: usize = 32;

impl Framer {
    /// The most a peer may buffer without completing a stanza. A 4096-byte
    /// chat body escaped five-for-one still fits, with room for its envelope.
    pub const LIMIT: usize = 64 * 1024;

    pub fn new() -> Self {
        Framer::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Forget the stream header, keeping any unread bytes.
    ///
    /// After SASL success both sides start a new stream (RFC 6120 §6.4.6),
    /// and the next thing on the wire is a fresh `<?xml?>` and header.
    pub fn reset(&mut self) {
        self.open = false;
    }

    /// Whether a stream header has been read and not yet closed.
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn next_event(&mut self) -> Result<Option<Event>, XmlError> {
        let step = if self.open {
            self.stanza()
        } else {
            self.header()
        };
        match step? {
            Some((event, used)) => {
                self.buf.drain(..used);
                match event {
                    Event::Open(_) => self.open = true,
                    Event::Close => self.open = false,
                    Event::Stanza(_) => {}
                }
                Ok(Some(event))
            }
            None if self.buf.len() > Self::LIMIT => Err(XmlError::TooLarge),
            None => Ok(None),
        }
    }

    fn header(&self) -> Step<Event> {
        let b = &self.buf;
        let mut pos = skip_ws(b, 0);
        if pos >= b.len() {
            return Ok(None);
        }
        if b[pos] != b'<' {
            return Err(malformed("text before the stream header"));
        }
        if pos + 1 >= b.len() {
            return Ok(None);
        }
        if b[pos + 1] == b'?' {
            let Some(end) = find(b, pos, b"?>") else {
                return Ok(None);
            };
            let pi = &b[pos..end];
            let is_decl = pi.len() > 5
                && &pi[..5] == b"<?xml"
                && matches!(pi[5], b' ' | b'\t' | b'\r' | b'\n');
            if !is_decl {
                return Err(XmlError::Rejected("processing instruction"));
            }
            pos = skip_ws(b, end + 2);
            if pos >= b.len() {
                return Ok(None);
            }
        }
        reject_markup_decl(b, pos)?;
        let Some(((name, attrs, empty), next)) = start_tag(b, pos)? else {
            return Ok(None);
        };
        if name != "stream:stream" {
            return Err(malformed(format!("expected stream:stream, got {name}")));
        }
        if empty {
            return Err(malformed("an empty stream:stream"));
        }
        Ok(Some((
            Event::Open(Element {
                name,
                attrs,
                ..Element::default()
            }),
            next,
        )))
    }

    fn stanza(&self) -> Step<Event> {
        let b = &self.buf;
        // Whitespace between a peer's stanzas is legal XMPP and we tolerate
        // it inbound. It is only *our* output that must never carry any.
        let pos = skip_ws(b, 0);
        if pos >= b.len() {
            return Ok(None);
        }
        if b[pos] != b'<' {
            return Err(malformed("character data at stream level"));
        }
        if pos + 1 >= b.len() {
            return Ok(None);
        }
        match b[pos + 1] {
            b'/' => {
                let Some((name, next)) = end_tag(b, pos)? else {
                    return Ok(None);
                };
                if name == "stream:stream" {
                    Ok(Some((Event::Close, next)))
                } else {
                    Err(malformed(format!("stray end tag {name}")))
                }
            }
            _ => Ok(element(b, pos, 1)?.map(|(e, n)| (Event::Stanza(e), n))),
        }
    }
}

/// Parse one complete element starting at the `<` at `pos`.
fn element(b: &[u8], pos: usize, depth: usize) -> Step<Element> {
    if depth > MAX_DEPTH {
        return Err(malformed("nested too deeply"));
    }
    reject_markup_decl(b, pos)?;
    let Some(((name, attrs, empty), mut p)) = start_tag(b, pos)? else {
        return Ok(None);
    };
    let mut el = Element {
        name,
        attrs,
        ..Element::default()
    };
    if empty {
        return Ok(Some((el, p)));
    }
    loop {
        // Character data runs to the next `<`. If there is none yet, the
        // element is not complete either.
        let Some(lt) = b[p..].iter().position(|&c| c == b'<').map(|i| p + i) else {
            return Ok(None);
        };
        if lt > p {
            el.text.push_str(&decode(&b[p..lt])?);
        }
        if lt + 1 >= b.len() {
            return Ok(None);
        }
        if b[lt + 1] == b'/' {
            let Some((name, next)) = end_tag(b, lt)? else {
                return Ok(None);
            };
            if name != el.name {
                return Err(malformed(format!("</{name}> closes <{}>", el.name)));
            }
            return Ok(Some((el, next)));
        }
        let Some((child, next)) = element(b, lt, depth + 1)? else {
            return Ok(None);
        };
        el.children.push(child);
        p = next;
    }
}

/// `<!` (comment, CDATA, DTD) and `<?` are refused wherever a tag may start.
fn reject_markup_decl(b: &[u8], pos: usize) -> Result<(), XmlError> {
    match b.get(pos + 1) {
        Some(b'!') => {
            let rest = &b[pos..];
            let what = if rest.starts_with(b"<!--") {
                "comment"
            } else if rest.starts_with(b"<![") {
                "CDATA"
            } else {
                "DTD"
            };
            Err(XmlError::Rejected(what))
        }
        Some(b'?') => Err(XmlError::Rejected("processing instruction")),
        _ => Ok(()),
    }
}

type StartTag = (String, Vec<(String, String)>, bool);

/// `<name attr='v' ...>` or `.../>`, from the `<` at `pos`.
fn start_tag(b: &[u8], pos: usize) -> Step<StartTag> {
    let Some((name, mut p)) = name(b, pos + 1)? else {
        return Ok(None);
    };
    let mut attrs = Vec::new();
    loop {
        p = skip_ws(b, p);
        let Some(&c) = b.get(p) else {
            return Ok(None);
        };
        match c {
            b'>' => return Ok(Some(((name, attrs, false), p + 1))),
            b'/' => {
                return match b.get(p + 1) {
                    None => Ok(None),
                    Some(b'>') => Ok(Some(((name, attrs, true), p + 2))),
                    Some(_) => Err(malformed("'/' not followed by '>'")),
                };
            }
            _ => {
                let Some((key, q)) = self::name(b, p)? else {
                    return Ok(None);
                };
                let q = skip_ws(b, q);
                match b.get(q) {
                    None => return Ok(None),
                    Some(b'=') => {}
                    Some(_) => return Err(malformed(format!("attribute {key} has no '='"))),
                }
                let q = skip_ws(b, q + 1);
                let Some(&quote) = b.get(q) else {
                    return Ok(None);
                };
                if quote != b'\'' && quote != b'"' {
                    return Err(malformed(format!("attribute {key} is not quoted")));
                }
                let Some(close) = b[q + 1..].iter().position(|&c| c == quote) else {
                    return Ok(None);
                };
                let raw = &b[q + 1..q + 1 + close];
                if raw.contains(&b'<') {
                    return Err(malformed(format!("'<' in attribute {key}")));
                }
                if attrs.iter().any(|(k, _)| *k == key) {
                    return Err(malformed(format!("attribute {key} given twice")));
                }
                attrs.push((key, decode(raw)?));
                p = q + 2 + close;
            }
        }
    }
}

/// `</name>`, from the `<` at `pos`.
fn end_tag(b: &[u8], pos: usize) -> Step<String> {
    let Some((name, p)) = name(b, pos + 2)? else {
        return Ok(None);
    };
    let p = skip_ws(b, p);
    match b.get(p) {
        None => Ok(None),
        Some(b'>') => Ok(Some((name, p + 1))),
        Some(_) => Err(malformed(format!("junk in </{name}>"))),
    }
}

/// A tag or attribute name. Needs a terminator to be sure it is complete.
fn name(b: &[u8], pos: usize) -> Step<String> {
    let mut p = pos;
    while let Some(&c) = b.get(p) {
        if matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>' | b'=') {
            break;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, b':' | b'_' | b'-' | b'.')) {
            return Err(malformed(format!("byte 0x{c:02x} in a name")));
        }
        p += 1;
    }
    if p >= b.len() {
        return Ok(None);
    }
    if p == pos {
        return Err(malformed("empty name"));
    }
    // ASCII by the check above.
    Ok(Some((String::from_utf8_lossy(&b[pos..p]).into_owned(), p)))
}

fn skip_ws(b: &[u8], mut p: usize) -> usize {
    while matches!(b.get(p), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        p += 1;
    }
    p
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|i| from + i)
}

/// Resolve entity and character references, then require UTF-8.
pub fn decode(raw: &[u8]) -> Result<String, XmlError> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'&' {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        let Some(semi) = raw[i..].iter().position(|&c| c == b';') else {
            return Err(malformed("'&' without ';'"));
        };
        let entity = &raw[i + 1..i + semi];
        let ch: char = match entity {
            b"lt" => '<',
            b"gt" => '>',
            b"amp" => '&',
            b"quot" => '"',
            b"apos" => '\'',
            _ if entity.first() == Some(&b'#') => {
                let digits = String::from_utf8_lossy(&entity[1..]).into_owned();
                let n = match digits.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16),
                    None => digits.parse::<u32>(),
                }
                .map_err(|_| malformed(format!("bad character reference &{digits};")))?;
                char::from_u32(n).ok_or_else(|| malformed(format!("&#{n}; is not a character")))?
            }
            _ => {
                return Err(malformed(format!(
                    "unknown entity &{};",
                    String::from_utf8_lossy(entity)
                )));
            }
        };
        let mut tmp = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
        i += semi + 1;
    }
    String::from_utf8(out).map_err(|_| malformed("not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact login transcript the client sends (`docs/lanes/chat.md` §2,
    /// read from `xmpphandler.py` [V]), steps 1 to 6, one string.
    const CLIENT_TRANSCRIPT: &str = "<?xml version='1.0'?><stream:stream to='eveoffline' version='1.0' xmlns='jabber:client' xmlns:stream='http://etherx.jabber.org/streams'>\
<auth xmlns='urn:ietf:params:xml:ns:xmpp-sasl' mechanism='PLAIN'>ADkwMDAwMDAxAHRva2Vu</auth>\
<iq id='id1' type='set' from='90000001@eveoffline' ><bind xmlns='urn:ietf:params:xml:ns:xmpp-bind'><resource>eveclient</resource></bind></iq>\
<iq id='id2' type='set' from='90000001@eveoffline/eveclient' ><session xmlns='urn:ietf:params:xml:ns:xmpp-session'/></iq>\
<presence><show/></presence>\
<message from='90000001@eveoffline' to='local_30000001@conference.eveoffline' type='groupchat' xml:lang='en' id='id3'><body>a &lt;b&gt; &amp; &#233;&#xe9;</body></message>";

    fn all(f: &mut Framer) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(e) = f.next_event().expect("well-formed") {
            out.push(e);
        }
        out
    }

    #[test]
    fn the_client_transcript_parses_into_a_header_and_five_stanzas() {
        let mut f = Framer::new();
        f.feed(CLIENT_TRANSCRIPT.as_bytes());
        let ev = all(&mut f);
        assert_eq!(ev.len(), 6);
        let Event::Open(h) = &ev[0] else {
            panic!("first event is the header: {:?}", ev[0])
        };
        assert_eq!(h.attr("to"), Some("eveoffline"));
        assert_eq!(
            h.attr("xmlns:stream"),
            Some("http://etherx.jabber.org/streams")
        );
        let Event::Stanza(iq) = &ev[2] else { panic!() };
        assert_eq!(iq.name, "iq");
        assert_eq!(iq.children[0].name, "bind");
        assert_eq!(iq.children[0].child("resource").unwrap().text, "eveclient");
        let Event::Stanza(msg) = &ev[5] else { panic!() };
        assert_eq!(msg.attr("xml:lang"), Some("en"));
        assert_eq!(msg.child("body").unwrap().text, "a <b> & \u{e9}\u{e9}");
    }

    /// Every split point of the transcript gives the same events as one feed.
    #[test]
    fn feeding_split_at_every_byte_gives_the_same_events() {
        let whole = {
            let mut f = Framer::new();
            f.feed(CLIENT_TRANSCRIPT.as_bytes());
            all(&mut f)
        };
        let bytes = CLIENT_TRANSCRIPT.as_bytes();
        for cut in 0..=bytes.len() {
            let mut f = Framer::new();
            f.feed(&bytes[..cut]);
            let mut got = all(&mut f);
            f.feed(&bytes[cut..]);
            got.extend(all(&mut f));
            assert_eq!(got, whole, "split at {cut}");
        }
        // And one byte at a time.
        let mut f = Framer::new();
        let mut got = Vec::new();
        for b in bytes {
            f.feed(std::slice::from_ref(b));
            got.extend(all(&mut f));
        }
        assert_eq!(got, whole);
    }

    #[test]
    fn entities_round_trip_through_escape_and_decode() {
        let s = "<&>\"' plain \u{2603}";
        assert_eq!(decode(escape(s).as_bytes()).unwrap(), s);
        let e = Element::new("body").with_attr("a", s).with_text(s);
        let mut f = Framer::new();
        f.feed(b"<stream:stream>");
        f.feed(e.to_xml().as_bytes());
        assert!(matches!(f.next_event(), Ok(Some(Event::Open(_)))));
        assert_eq!(f.next_event(), Ok(Some(Event::Stanza(e))));
    }

    #[test]
    fn comments_cdata_dtd_and_pis_are_rejected() {
        for bad in [
            "<stream:stream><!-- hi --><a/>",
            "<stream:stream><a><![CDATA[x]]></a>",
            "<!DOCTYPE x><stream:stream>",
            "<stream:stream><a><?pi x?></a>",
            "<?php x?><stream:stream>",
        ] {
            let mut f = Framer::new();
            f.feed(bad.as_bytes());
            let mut res = f.next_event();
            if let Ok(Some(Event::Open(_))) = res {
                res = f.next_event();
            }
            assert!(
                matches!(res, Err(XmlError::Rejected(_))),
                "{bad} gave {res:?}"
            );
        }
    }

    #[test]
    fn serialisation_has_no_whitespace_between_tags() {
        let e = Element::new("presence")
            .with_attr("from", "r@c/1")
            .with_child(
                Element::new("x")
                    .with_attr("xmlns", "ns")
                    .with_child(Element::new("item")),
            )
            .with_child(Element::new("status"));
        assert_eq!(
            e.to_xml(),
            "<presence from='r@c/1'><x xmlns='ns'><item/></x><status/></presence>"
        );
    }

    #[test]
    fn the_stream_closes_and_reset_accepts_a_second_header() {
        let mut f = Framer::new();
        f.feed(b"<?xml version='1.0'?><stream:stream a='1'><x/>");
        assert!(matches!(f.next_event(), Ok(Some(Event::Open(_)))));
        assert!(matches!(f.next_event(), Ok(Some(Event::Stanza(_)))));
        f.reset();
        f.feed(b"<?xml version='1.0'?><stream:stream a='2'></stream:stream>");
        let Ok(Some(Event::Open(h))) = f.next_event() else {
            panic!()
        };
        assert_eq!(h.attr("a"), Some("2"));
        assert_eq!(f.next_event(), Ok(Some(Event::Close)));
        assert!(!f.is_open());
    }

    #[test]
    fn malformed_input_is_an_error_not_a_hang() {
        for bad in [
            "<stream:stream><a></b>",
            "<stream:stream>text",
            "<stream:stream><a b=c/>",
            "<stream:stream><a>&bogus;</a>",
            "<other>",
        ] {
            let mut f = Framer::new();
            f.feed(bad.as_bytes());
            let mut res = f.next_event();
            if let Ok(Some(Event::Open(_))) = res {
                res = f.next_event();
            }
            assert!(
                matches!(res, Err(XmlError::Malformed(_))),
                "{bad} gave {res:?}"
            );
        }
    }

    #[test]
    fn an_endless_stanza_is_refused_at_the_limit() {
        let mut f = Framer::new();
        f.feed(b"<stream:stream><message><body>");
        assert!(matches!(f.next_event(), Ok(Some(Event::Open(_)))));
        f.feed(&vec![b'a'; Framer::LIMIT + 1]);
        assert_eq!(f.next_event(), Err(XmlError::TooLarge));
    }
}
