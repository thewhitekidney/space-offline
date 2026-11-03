//! Codec for CCP's marshalled-Python wire format ("PyRep"), as spoken by the
//! EVE client over machoNet.
//!
//! This crate is the byte layer only: it turns a stream of marshalled bytes
//! into a [`PyValue`] tree and back. It has no dependencies, does no IO, and
//! knows nothing about packets, sessions or compression — those sit above it.
//!
//! Layering, bottom to top:
//!   - [`Reader`] / [`Writer`]: raw little-endian primitives and the marshal
//!     length convention. Below the opcode layer; format-stable.
//!   - `PyValue` + load/save: the opcode-tagged object graph. (Being filled in
//!     against the exact tag values this client build uses.)
//!   - [`dbrow`]: the one type code with a layout of its own, `blue.DBRow`'s
//!     packed-byte form. Sits beside the opcode layer rather than above it,
//!     because `0x2A` is dispatched from the same table.
//!
//! Every parser is written to treat its input as hostile: lengths are checked
//! against the remaining buffer before allocating, and a truncated or
//! malformed stream yields a [`WireError`], never a panic.

#![forbid(unsafe_code)]

use std::fmt;

pub mod build;
mod constants;
pub mod dbrow;
mod string_table;
pub mod tag;
pub mod value;
pub use constants::{AddressType, DbType, MachoNetMsg, call, compression, handshake};
pub use dbrow::{Column, DbRow, DbRowDescriptor, DbRowError, DbValue, SizeClass};
pub use string_table::STRING_TABLE;
pub use value::{
    Decoder, Header, ObjectForm, PyValue, Stream, save, take_header, take_stream,
    write_empty_header,
};

/// Anything that can go wrong decoding a marshal stream. Encoding is
/// infallible (it only ever appends to a `Vec`), so this is a read-side type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Wanted `need` more bytes at `at`, but the buffer ended.
    UnexpectedEof { at: usize, need: usize },
    /// An opcode byte that the codec does not recognise.
    UnknownTag { at: usize, tag: u8 },
    /// A structurally valid but nonsensical value (e.g. a negative length).
    Malformed { at: usize, why: &'static str },
    /// A type code this build accepts but whose payload layout has not been
    /// established from the client. Carries the type code and its offset so a
    /// capture can be used to close the gap. Never decoded on a guess.
    UnsupportedTag { at: usize, tag: u8 },
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::UnexpectedEof { at, need } => {
                write!(
                    f,
                    "unexpected end of stream at {at}: need {need} more byte(s)"
                )
            }
            WireError::UnknownTag { at, tag } => {
                write!(f, "unknown marshal tag {tag:#04x} at {at}")
            }
            WireError::Malformed { at, why } => write!(f, "malformed stream at {at}: {why}"),
            WireError::UnsupportedTag { at, tag } => write!(
                f,
                "type code {tag:#04x} at {at} is valid but its layout is not yet established"
            ),
        }
    }
}

impl std::error::Error for WireError {}

/// A forward-only cursor over a marshal byte stream.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Byte offset of the cursor, for error reporting and tests.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.remaining() < n {
            return Err(WireError::UnexpectedEof {
                at: self.pos,
                need: n,
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    /// Look at the next byte without consuming it (for opcode dispatch).
    pub fn peek_u8(&self) -> Result<u8, WireError> {
        self.buf
            .get(self.pos)
            .copied()
            .ok_or(WireError::UnexpectedEof {
                at: self.pos,
                need: 1,
            })
    }

    pub fn i16(&mut self) -> Result<i16, WireError> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i32(&mut self) -> Result<i32, WireError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i64(&mut self) -> Result<i64, WireError> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn f64(&mut self) -> Result<f64, WireError> {
        let b = self.take(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        self.take(n)
    }

    /// The marshal length convention, used for strings and collection counts:
    /// a single length byte, except that a byte of `0xFF` escapes to a full
    /// little-endian `u32` that follows. Keeps small lengths in one byte while
    /// still allowing large ones.
    pub fn size(&mut self) -> Result<usize, WireError> {
        let first = self.u8()?;
        if first == 0xFF {
            let n = self.i32()?;
            if n < 0 {
                return Err(WireError::Malformed {
                    at: self.pos - 4,
                    why: "negative length",
                });
            }
            Ok(n as usize)
        } else {
            Ok(first as usize)
        }
    }
}

/// An append-only marshal byte builder.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    /// Mirror of [`Reader::size`].
    pub fn size(&mut self, n: usize) {
        if n < 0xFF {
            self.buf.push(n as u8);
        } else {
            self.buf.push(0xFF);
            // Counts above i32::MAX cannot occur on this wire (1 MiB-ish
            // packets); clamp defensively rather than silently truncate.
            let n = i32::try_from(n).unwrap_or(i32::MAX);
            self.i32(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut w = Writer::new();
        w.u8(0x2a);
        w.i16(-1234);
        w.i32(-123456);
        w.i64(-1234567890123);
        w.f64(core::f64::consts::PI);
        w.bytes(b"macho");
        let bytes = w.into_bytes();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 0x2a);
        assert_eq!(r.i16().unwrap(), -1234);
        assert_eq!(r.i32().unwrap(), -123456);
        assert_eq!(r.i64().unwrap(), -1234567890123);
        assert_eq!(r.f64().unwrap(), core::f64::consts::PI);
        assert_eq!(r.bytes(5).unwrap(), b"macho");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn size_uses_one_byte_below_0xff_and_escapes_above() {
        // Boundary cases of the length convention, in both directions.
        for &n in &[0usize, 1, 0xFE, 0xFF, 0x100, 70_000, 1 << 20] {
            let mut w = Writer::new();
            w.size(n);
            let encoded = w.into_bytes();
            // 0..=0xFE fit in one byte; 0xFF and up take the 1 + 4 escape.
            let expected_len = if n < 0xFF { 1 } else { 5 };
            assert_eq!(encoded.len(), expected_len, "n={n}");
            let mut r = Reader::new(&encoded);
            assert_eq!(r.size().unwrap(), n, "n={n}");
            assert_eq!(r.remaining(), 0, "n={n}");
        }
    }

    #[test]
    fn short_read_reports_eof_without_panicking() {
        let mut r = Reader::new(&[0x01, 0x02]);
        assert_eq!(r.i16().unwrap(), 0x0201);
        assert_eq!(r.i32(), Err(WireError::UnexpectedEof { at: 2, need: 4 }));
    }

    /// Tag `0x2F`, the arbitrary-precision int, against the real client.
    ///
    /// Its `PingReq` carries hop timings as Windows FILETIMEs under this tag.
    /// These two are the exact bytes from capture `1792405830-0002.evecap`
    /// frame 13; they decode to unix 1792405830, which is that capture's own
    /// timestamp — so the values are self-checking, not just plausible.
    #[test]
    fn the_long_tag_decodes_the_filetimes_a_real_client_sent() {
        fn decode(body: &[u8]) -> PyValue {
            let mut bytes = vec![tag::code::LONG, body.len() as u8];
            bytes.extend_from_slice(body);
            let mut r = Reader::new(&bytes);
            Decoder::new().load(&mut r).expect("0x2F must decode")
        }
        const FILETIME_TO_UNIX: i64 = 11_644_473_600;
        for body in [
            [0xc4, 0xeb, 0x50, 0xdd, 0xb4, 0x5f, 0xdd, 0x01],
            [0x53, 0xd9, 0x60, 0xdd, 0xb4, 0x5f, 0xdd, 0x01],
        ] {
            let PyValue::Int(ticks) = decode(&body) else {
                panic!("0x2F must decode to an integer")
            };
            assert_eq!(ticks / 10_000_000 - FILETIME_TO_UNIX, 1_792_405_830);
        }
    }

    #[test]
    fn the_long_tag_handles_the_edges() {
        fn decode(body: &[u8]) -> Result<PyValue, WireError> {
            let mut bytes = vec![tag::code::LONG, body.len() as u8];
            bytes.extend_from_slice(body);
            Decoder::new().load(&mut Reader::new(&bytes))
        }
        // len == 0 is the integer zero.
        assert_eq!(decode(&[]).unwrap(), PyValue::Int(0));
        // Signed, little-endian: 0xFF alone is -1, not 255.
        assert_eq!(decode(&[0xFF]).unwrap(), PyValue::Int(-1));
        assert_eq!(decode(&[0x01, 0x00]).unwrap(), PyValue::Int(1));
        assert_eq!(decode(&[0x00, 0x80]).unwrap(), PyValue::Int(-32768));
        // i64 bounds exactly.
        assert_eq!(
            decode(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]).unwrap(),
            PyValue::Int(i64::MAX)
        );
        assert_eq!(
            decode(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80]).unwrap(),
            PyValue::Int(i64::MIN)
        );
        // Wider than 8 bytes is fine when the extra bytes are sign extension.
        assert_eq!(
            decode(&[0x01, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap(),
            PyValue::Int(1)
        );
        assert_eq!(
            decode(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).unwrap(),
            PyValue::Int(-1)
        );
        // A genuine bignum is refused rather than truncated to something wrong.
        assert!(decode(&[0, 0, 0, 0, 0, 0, 0, 0, 0x01]).is_err());
        assert!(decode(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]).is_err());
    }

    #[test]
    fn string_table_has_expected_shape() {
        // 195 entries, from blue.dll's own initialisation loop
        // (`cmp edi, 0xc3; jl`) -- not from marshalstrings.py, whose table is
        // built and then immediately swapped back out. See string_table.rs.
        assert_eq!(STRING_TABLE.len(), 195);
        assert_eq!(STRING_TABLE[0], "*corpid"); // wire index 1
        assert_eq!(STRING_TABLE[194], "agent.StorylineMissionDetails"); // 195
    }

    /// The one entry whose index we have seen a real client put on the wire.
    ///
    /// Its first service call carries `{machoVersion: 1}`, with the key as
    /// tag `0x11` byte `0x87` = 135. That single pairing is what proves both
    /// the table and the 1-based convention, so it is pinned on its own: if
    /// a regenerated table ever moves it, every string-table decode is wrong.
    #[test]
    fn the_index_a_real_client_sent_resolves_to_what_it_meant() {
        assert_eq!(STRING_TABLE[135 - 1], "machoVersion");
    }

    #[test]
    fn size_rejects_negative_escaped_length() {
        // 0xFF escape followed by -1 as i32 must be refused, not wrapped huge.
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut r = Reader::new(&bytes);
        assert_eq!(
            r.size(),
            Err(WireError::Malformed {
                at: 1,
                why: "negative length"
            })
        );
    }
}
