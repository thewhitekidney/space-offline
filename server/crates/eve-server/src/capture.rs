//! An append-only log of every frame a connection carried.
//!
//! The project's one irreplaceable asset is a recording of what the real
//! client actually sends. Disassembly got us the wire format; a capture is what
//! *validates* it, and the first client connection is the only cheap chance to
//! take one. So this server logs unconditionally, before it tries to interpret
//! anything, and it logs the bytes rather than its interpretation of them — an
//! interpretation we may well have wrong is exactly the thing a capture is
//! supposed to catch.
//!
//! ## File format
//!
//! ```text
//! magic    14 bytes  "EVEOFFLINECAP\n"
//! version   1 byte   currently 1
//! then, repeated:
//!   dir     1 byte   0 = client->server, 1 = server->client
//!   micros  8 bytes  u64 LE, since this capture was opened
//!   len     4 bytes  u32 LE
//!   bytes   len
//! ```
//!
//! Deliberately not a pcap. A pcap would record TCP segments, and the thing we
//! want to reason about is *frames* — the boundaries the framing layer decided
//! on. Recording our own notion of a frame means a disagreement between the
//! capture and the client's behaviour localises the bug to the splitter.
//!
//! ## The one frame that is not verbatim
//!
//! **No capture ever holds a login token** (`docs/lanes/account-spec.md`
//! §4.3). The client's H6 request carries `user_sso_token`, and once
//! accounts land that token is a password. [`redact`] rewrites that one
//! frame: decoded, the token replaced by [`REDACTED`], marshalled again, and
//! stored **uncompressed** even if the client sent it zlib-compressed.
//! Every other frame is written byte for byte.

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

pub const MAGIC: &[u8; 14] = b"EVEOFFLINECAP\n";
pub const VERSION: u8 = 1;

/// The handshake key whose value never reaches a capture.
pub const SSO_KEY: &[u8] = b"user_sso_token";
/// What a capture holds in its place.
pub const REDACTED: &[u8] = b"<redacted>";

/// `payload` as it may be written to a capture: unchanged, unless it is a
/// client frame that names [`SSO_KEY`].
///
/// Such a frame is decoded, every value under that key (at any depth of
/// tuples, lists and dicts) becomes [`REDACTED`], and the value is
/// marshalled again. If the frame names the key but cannot be decoded, or
/// the key sits somewhere the walk does not reach, the whole frame is
/// withheld and a short string stands in for it: a capture missing a frame
/// is a lesser loss than a capture holding a token.
///
/// [A] A zlib frame that does not inflate is written as is; the server
/// refuses it, so no token in it was ever read. Falsified by a capture
/// whose undecodable frame inflates elsewhere to a token.
pub fn redact(direction: Direction, payload: &[u8]) -> Cow<'_, [u8]> {
    if direction != Direction::ClientToServer {
        return Cow::Borrowed(payload);
    }
    let plain: Cow<'_, [u8]> = if eve_inflate::looks_like_zlib(payload) {
        match eve_inflate::inflate_zlib_capped(payload, crate::wire::MAX_INBOUND_INFLATED) {
            Ok(p) => Cow::Owned(p),
            Err(_) => return Cow::Borrowed(payload),
        }
    } else {
        Cow::Borrowed(payload)
    };
    if !plain.windows(SSO_KEY.len()).any(|w| w == SSO_KEY) {
        return Cow::Borrowed(payload);
    }
    let withheld = || {
        Cow::Owned(crate::wire::dumps(&eve_marshal::PyValue::Bytes(
            b"capture: frame withheld, it named user_sso_token and did not redact".to_vec(),
        )))
    };
    let Ok(mut value) = crate::wire::loads(&plain) else {
        return withheld();
    };
    if !redact_value(&mut value) {
        return withheld();
    }
    Cow::Owned(crate::wire::dumps(&value))
}

/// Replace every [`SSO_KEY`] value under `v`; true if any was found.
fn redact_value(v: &mut eve_marshal::PyValue) -> bool {
    use eve_marshal::PyValue;
    match v {
        PyValue::Tuple(items) | PyValue::List(items) => {
            let mut hit = false;
            for item in items {
                hit |= redact_value(item);
            }
            hit
        }
        // `(value, key)`: the wire order (`pyutil::pairs_get`).
        PyValue::Dict(pairs) => {
            let mut hit = false;
            for (value, key) in pairs {
                if crate::pyutil::as_bytes(key) == Some(SSO_KEY) {
                    *value = PyValue::Bytes(REDACTED.to_vec());
                    hit = true;
                } else {
                    hit |= redact_value(value);
                }
            }
            hit
        }
        _ => false,
    }
}

/// Which way a frame was travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

impl Direction {
    fn code(self) -> u8 {
        match self {
            Direction::ClientToServer => 0,
            Direction::ServerToClient => 1,
        }
    }

    fn from_code(c: u8) -> Option<Self> {
        match c {
            0 => Some(Direction::ClientToServer),
            1 => Some(Direction::ServerToClient),
            _ => None,
        }
    }
}

/// One logged frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub direction: Direction,
    pub micros: u64,
    pub bytes: Vec<u8>,
}

/// A capture being written.
pub struct Capture {
    out: BufWriter<File>,
    started: Instant,
    count: u64,
}

impl Capture {
    /// Create a capture file. Refuses to overwrite: a capture is evidence, and
    /// silently replacing one would be the single most annoying way to lose a
    /// session we cannot easily retake.
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = File::create_new(path)?;
        file.write_all(MAGIC)?;
        file.write_all(&[VERSION])?;
        Ok(Capture {
            out: BufWriter::new(file),
            started: Instant::now(),
            count: 0,
        })
    }

    /// Append a frame and flush. A frame holding a login token is
    /// [`redact`]ed first.
    ///
    /// Flushing every record costs a syscall per frame and is worth it: if the
    /// client crashes us, or the jail kills the process, the frames up to that
    /// point are the most interesting ones we will have.
    pub fn write(&mut self, direction: Direction, bytes: &[u8]) -> io::Result<()> {
        let bytes = &*redact(direction, bytes);
        let micros = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.out.write_all(&[direction.code()])?;
        self.out.write_all(&micros.to_le_bytes())?;
        self.out
            .write_all(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes())?;
        self.out.write_all(bytes)?;
        self.out.flush()?;
        self.count += 1;
        Ok(())
    }

    /// Frames written so far.
    pub fn count(&self) -> u64 {
        self.count
    }
}

/// Read a whole capture back.
///
/// The reader is strict about truncation rather than returning what it managed
/// to parse: a short final record means the writer died mid-frame, and knowing
/// that is more useful than quietly dropping it.
pub fn read_all(path: &Path) -> io::Result<Vec<Record>> {
    let mut buf = Vec::new();
    File::open(path)?.read_to_end(&mut buf)?;
    parse(&buf)
}

/// Parse capture bytes. Separate from [`read_all`] so it can be tested without
/// touching a filesystem.
pub fn parse(buf: &[u8]) -> io::Result<Vec<Record>> {
    let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_owned());
    if buf.len() < MAGIC.len() + 1 {
        return Err(bad("capture is shorter than its header"));
    }
    if &buf[..MAGIC.len()] != MAGIC {
        return Err(bad("not a capture file"));
    }
    let version = buf[MAGIC.len()];
    if version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("capture version {version}, expected {VERSION}"),
        ));
    }

    let mut at = MAGIC.len() + 1;
    let mut out = Vec::new();
    while at < buf.len() {
        // 1 direction + 8 micros + 4 length
        if buf.len() - at < 13 {
            return Err(bad("truncated record header"));
        }
        let direction =
            Direction::from_code(buf[at]).ok_or_else(|| bad("unknown direction byte"))?;
        let micros = u64::from_le_bytes(buf[at + 1..at + 9].try_into().expect("8 bytes"));
        let len = u32::from_le_bytes(buf[at + 9..at + 13].try_into().expect("4 bytes")) as usize;
        at += 13;
        if buf.len() - at < len {
            return Err(bad("truncated record body"));
        }
        out.push(Record {
            direction,
            micros,
            bytes: buf[at..at + len].to_vec(),
        });
        at += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.push(VERSION);
        v
    }

    fn record(dir: u8, micros: u64, body: &[u8]) -> Vec<u8> {
        let mut v = vec![dir];
        v.extend_from_slice(&micros.to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn empty_capture_parses_to_nothing() {
        assert_eq!(parse(&header()).unwrap(), vec![]);
    }

    #[test]
    fn records_round_trip() {
        let mut buf = header();
        buf.extend(record(0, 10, b"hello"));
        buf.extend(record(1, 20, b""));
        buf.extend(record(0, 30, &[0xFF; 300]));
        let got = parse(&buf).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].direction, Direction::ClientToServer);
        assert_eq!(got[0].bytes, b"hello");
        assert_eq!(got[1].direction, Direction::ServerToClient);
        assert!(got[1].bytes.is_empty());
        assert_eq!(got[2].bytes.len(), 300);
        assert_eq!(got[2].micros, 30);
    }

    #[test]
    fn wrong_magic_is_refused() {
        let mut buf = b"NOTACAPTURE!!\n".to_vec();
        buf.push(VERSION);
        assert!(parse(&buf).is_err());
    }

    #[test]
    fn wrong_version_is_refused() {
        let mut buf = MAGIC.to_vec();
        buf.push(VERSION + 1);
        assert!(parse(&buf).is_err());
    }

    #[test]
    fn truncation_is_an_error_not_a_short_read() {
        let mut full = header();
        full.extend(record(0, 1, b"abcdefgh"));
        for cut in header().len() + 1..full.len() {
            assert!(
                parse(&full[..cut]).is_err(),
                "a capture cut to {cut} bytes parsed as if complete"
            );
        }
        assert!(parse(&full).is_ok());
    }

    /// An H6-shaped frame: `(challenge, request dict)`, with an invented
    /// token.
    fn h6_frame(token: &str) -> Vec<u8> {
        use crate::pyutil::{bstr, pydict};
        crate::wire::dumps(&eve_marshal::PyValue::Tuple(vec![
            eve_marshal::PyValue::Bytes(vec![0x11; 8]),
            pydict(vec![
                ("user_name", bstr("someone")),
                ("user_sso_token", bstr(token)),
            ]),
        ]))
    }

    /// `plain` as a zlib stream of one stored block, as the client's
    /// marshal layer may send it.
    fn zlib_stored(plain: &[u8]) -> Vec<u8> {
        let mut v = vec![0x78, 0x01, 0x01];
        let n = plain.len() as u16;
        v.extend_from_slice(&n.to_le_bytes());
        v.extend_from_slice(&(!n).to_le_bytes());
        v.extend_from_slice(plain);
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in plain {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        v.extend_from_slice(&((b << 16) | a).to_be_bytes());
        v
    }

    #[test]
    fn the_sso_token_is_redacted() {
        use crate::pyutil::{as_bytes, dict_get};
        let token = "invented-secret-token";
        let has = |hay: &[u8]| hay.windows(token.len()).any(|w| w == token.as_bytes());
        let plain = h6_frame(token);
        let packed = zlib_stored(&plain);
        assert!(eve_inflate::looks_like_zlib(&packed));
        assert_eq!(
            crate::wire::loads(&packed).unwrap(),
            crate::wire::loads(&plain).unwrap()
        );

        let dir = std::env::temp_dir().join(format!("evecap-sso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.evecap");
        let _ = std::fs::remove_file(&path);
        let mut cap = Capture::create(&path).unwrap();
        cap.write(Direction::ClientToServer, &plain).unwrap();
        cap.write(Direction::ClientToServer, &packed).unwrap();
        cap.write(Direction::ClientToServer, b"\x7e\0\0\0\0\x01")
            .unwrap();
        drop(cap);
        let file = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!has(&file), "the token reached the capture file");

        let got = parse(&file).unwrap();
        for r in &got[..2] {
            let v = crate::wire::loads(&r.bytes).expect("a redacted frame still decodes");
            let eve_marshal::PyValue::Tuple(t) = &v else {
                panic!("{v:?}")
            };
            assert_eq!(dict_get(&t[1], SSO_KEY).and_then(as_bytes), Some(REDACTED));
            assert_eq!(
                dict_get(&t[1], b"user_name").and_then(as_bytes),
                Some(&b"someone"[..])
            );
        }
        // A frame without the key is kept byte for byte, and so is anything
        // the server sent.
        assert_eq!(got[2].bytes, b"\x7e\0\0\0\0\x01");
        assert_eq!(redact(Direction::ServerToClient, &plain), &plain[..]);
        // A frame that names the key but does not decode is withheld.
        let mut broken = plain.clone();
        broken.push(0);
        assert!(!has(&redact(Direction::ClientToServer, &broken)));
    }

    #[test]
    fn unknown_direction_is_refused() {
        let mut buf = header();
        buf.extend(record(7, 1, b"x"));
        assert!(parse(&buf).is_err());
    }

    #[test]
    fn writing_then_reading_agrees() {
        let dir = std::env::temp_dir().join(format!("evecap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.evecap");
        let _ = std::fs::remove_file(&path);
        let mut cap = Capture::create(&path).unwrap();
        cap.write(Direction::ClientToServer, b"\x01\x02").unwrap();
        cap.write(Direction::ServerToClient, b"\x03").unwrap();
        assert_eq!(cap.count(), 2);
        drop(cap);
        let got = read_all(&path).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].bytes, b"\x01\x02");
        assert_eq!(got[1].direction, Direction::ServerToClient);
        // Creating over an existing capture must fail rather than clobber it.
        assert!(Capture::create(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
