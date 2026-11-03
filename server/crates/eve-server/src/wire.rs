//! One GPS payload: a complete marshal stream carrying exactly one object.
//!
//! The handshake is Python, not native — `carbon/common/script/net/GPS.pyc`.
//! Every line of it is one `macho.Dumps` / `macho.Loads` round trip over one
//! `sendpacket`, and `macho.Dumps` is `blue.marshal.Save`: a stream header
//! followed by one marshalled object. So "a payload" always means
//! header-plus-one-object, never a bare object, and that convention lives
//! here rather than in `eve-marshal` because it belongs to the transport.
//!
//! See `docs/reference/login-path.md` §4 for the payload-by-payload table.

use eve_marshal::value::Budget;
use eve_marshal::{
    Decoder, PyValue, Reader, WireError, Writer, save, take_stream, write_empty_header,
};

/// The most a client's compressed payload may inflate to on the server.
///
/// The client itself drops any marshal blob past
/// `machoNet.dropPacketThreshold` [V] (`eve_proto::MAX_PAYLOAD`), so nothing
/// legitimate inflates past it. The bots and `eve-probe` keep
/// `eve_inflate::MAX_OUTPUT`, because what they inflate is the server's own
/// responses (`docs/review/V2-wire.md` F2).
pub const MAX_INBOUND_INFLATED: usize = eve_proto::MAX_PAYLOAD;

/// Marshal one object into a complete payload: `macho.Dumps(v)`.
///
/// We always emit the `0x7E` header with `mapcount = 0`. Nothing we send has
/// shared objects, and the meaning of the `0x7D` form's `extra` byte is still
/// undetermined (`docs/archive/marshal-tags.md`), so we never emit it.
pub fn dumps(v: &PyValue) -> Vec<u8> {
    let mut w = Writer::new();
    write_empty_header(&mut w);
    save(v, &mut w);
    w.into_bytes()
}

/// Parse a complete payload: `macho.Loads(bytes)`.
///
/// Trailing bytes after the object are an error. A GPS payload is exactly one
/// object, and silently ignoring a tail would hide a framing bug — which is
/// the class of bug this server is most likely to have.
///
/// A payload may arrive **zlib-compressed**: the client's native marshal layer
/// compresses the whole stream — header and all — once it grows past a few
/// hundred bytes, and the first thing the first real client sent after the
/// handshake was a `78 01` frame (its `machoNet.GetServiceInfo` call). So the
/// two forms are told apart here, by the zlib header's own validity rules
/// rather than by a guess: a marshal stream starts `0x7E` or `0x7D`, neither of
/// which can be a zlib CMF byte, so the test is exact in both directions.
///
/// **This is the server's inbound path**, so it is bounded twice over:
/// inflation stops at [`MAX_INBOUND_INFLATED`], and the decode at
/// [`Budget::INBOUND`] (`docs/review/V2-wire.md` F2). A payload past either is
/// `WireError::Malformed`. Decoding the server's own output (a capture's
/// responses, a cached result) wants [`loads_unbudgeted`] instead.
pub fn loads(bytes: &[u8]) -> Result<PyValue, WireError> {
    loads_within(bytes, MAX_INBOUND_INFLATED, Some(Budget::INBOUND))
}

/// [`loads`] without the inbound limits: the 64 MiB inflate cap the bots
/// use, and no decode budget. For bytes the server wrote, never for a
/// client's.
pub fn loads_unbudgeted(bytes: &[u8]) -> Result<PyValue, WireError> {
    loads_within(bytes, eve_inflate::MAX_OUTPUT, None)
}

fn loads_within(
    bytes: &[u8],
    inflate_cap: usize,
    budget: Option<Budget>,
) -> Result<PyValue, WireError> {
    if eve_inflate::looks_like_zlib(bytes) {
        let plain = eve_inflate::inflate_zlib_capped(bytes, inflate_cap).map_err(|_| {
            WireError::Malformed {
                at: 0,
                why: "payload claims to be zlib but does not inflate",
            }
        })?;
        return loads_plain(&plain, budget);
    }
    loads_plain(bytes, budget)
}

/// [`loads`] for a payload already known to be uncompressed.
fn loads_plain(bytes: &[u8], budget: Option<Budget>) -> Result<PyValue, WireError> {
    // The shared-object id table lives at the **end** of the stream, and it is
    // needed rather than merely skipped: a `0x1B` is an index into it. See
    // `take_stream`, which is where that layout is written down.
    let stream = take_stream(bytes)?;
    let mut r = Reader::new(stream.body);
    let mut d = Decoder::with_shared(stream.shared_ids);
    if let Some(b) = budget {
        d = d.with_budget(b);
    }
    let v = d.load(&mut r)?;
    if r.remaining() != 0 {
        return Err(WireError::Malformed {
            at: r.pos(),
            why: "trailing bytes after the payload object",
        });
    }
    Ok(v)
}

/// CRC-16/XMODEM, which is Python's `binascii.crc_hqx`.
///
/// Reference vectors taken from `binascii.crc_hqx` itself (CPython 3.11, same
/// function): `b"" -> 0`, `b"a" -> 31879`, `b"123456789" -> 12739`, and
/// `bytes(range(32)) -> 54015`. All four are pinned in the tests below.
pub fn crc_hqx(data: &[u8], seed: u16) -> u16 {
    let mut crc = seed;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// `Crypto.CryptoHash(*args)` under the Placebo crypto pack.
///
/// `evecrypto/placebo.pyc` reduces `CryptoHash` to
/// `str(crc_hqx(marshal.Save(args), 0))`. Two things about that are easy to
/// get wrong, so they are stated here:
///
/// 1. `CryptoHash` is **varargs**, so it hashes the *tuple* of its arguments.
///    Hashing one 64-byte challenge means hashing the 1-tuple containing it,
///    not the challenge itself.
/// 2. The result is a decimal **string**, not an integer — `str()` of a
///    16-bit number, so between one and five ASCII digits.
///
/// This is not a security property. Under Placebo it is a 16-bit checksum that
/// both sides compute the same way, and that is all the client asks of it.
pub fn crypto_hash(args: &[PyValue]) -> String {
    crc_hqx(&blue_marshal(&PyValue::Tuple(args.to_vec())), 0).to_string()
}

/// Marshal a value **exactly the way the client's `blue.marshal.Save` does**,
/// byte for byte.
///
/// This is deliberately separate from [`crate::wire::dumps`] and
/// `eve_marshal::save`. Those produce a stream the client *decodes* correctly,
/// but they are free to pick whichever valid tag is convenient — and they pick
/// `0x10` (STRING_SHORT) for short strings. The client's own writer always uses
/// `0x13` for strings, and emits the special tuple tags for the 0-, 1- and
/// 2-element cases. For `CryptoHash` that difference is fatal: the hash is a
/// CRC over the raw marshalled bytes, so `str(challenge)` under tag `0x10`
/// CRCs differently from the same string under `0x13`, and the client rejects
/// the handshake with *"Server response signature incorrect"* — which is
/// exactly what the first real client did (it marshals every string as `0x13`,
/// confirmed from its own H2/H4/H6 frames).
///
/// Only the shapes `CryptoHash` actually passes are handled — tuples, byte
/// strings, ints, None — because that is all a challenge hash ever contains.
/// Anything else is a bug in the caller, not a marshal case to support.
fn blue_marshal(v: &PyValue) -> Vec<u8> {
    let mut out = vec![0x7E, 0, 0, 0, 0]; // empty stream header, mapcount 0
    blue_save(v, &mut out);
    out
}

fn blue_save(v: &PyValue, out: &mut Vec<u8>) {
    match v {
        PyValue::None => out.push(0x01),
        PyValue::Int(n) => blue_save_int(*n, out),
        PyValue::Bytes(b) => blue_save_str(b, out),
        PyValue::Tuple(items) => blue_save_tuple(items, out),
        other => panic!("blue_marshal: unsupported value in a crypto hash: {other:?}"),
    }
}

/// `0x13` + escaped length + bytes — the client's string encoding.
fn blue_save_str(b: &[u8], out: &mut Vec<u8>) {
    out.push(0x13);
    blue_size(b.len(), out);
    out.extend_from_slice(b);
}

/// The client's tuple encoding: special tags for 0/1/2 elements, the general
/// `0x14` + count form otherwise.
fn blue_save_tuple(items: &[PyValue], out: &mut Vec<u8>) {
    match items.len() {
        0 => out.push(0x24),
        1 => {
            out.push(0x25);
            blue_save(&items[0], out);
        }
        2 => {
            out.push(0x2C);
            blue_save(&items[0], out);
            blue_save(&items[1], out);
        }
        n => {
            out.push(0x14);
            blue_size(n, out);
            for item in items {
                blue_save(item, out);
            }
        }
    }
}

fn blue_save_int(n: i64, out: &mut Vec<u8>) {
    // Narrowest width, matching the reader's fixed-width int tags. Challenges
    // never carry ints, but a responseDict hash might, so handle them.
    if let Ok(v) = i8::try_from(n) {
        out.push(0x06);
        out.push(v as u8);
    } else if let Ok(v) = i16::try_from(n) {
        out.push(0x05);
        out.extend_from_slice(&v.to_le_bytes());
    } else if let Ok(v) = i32::try_from(n) {
        out.push(0x04);
        out.extend_from_slice(&v.to_le_bytes());
    } else {
        out.push(0x03);
        out.extend_from_slice(&n.to_le_bytes());
    }
}

/// The marshal size convention: one byte, or `0xFF` then a `u32`.
fn blue_size(n: usize, out: &mut Vec<u8>) {
    if n < 0xFF {
        out.push(n as u8);
    } else {
        out.push(0xFF);
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_hqx_matches_python_binascii() {
        // Captured from binascii.crc_hqx in the analysis container.
        assert_eq!(crc_hqx(b"", 0), 0);
        assert_eq!(crc_hqx(b"a", 0), 31879);
        assert_eq!(crc_hqx(b"123456789", 0), 12739);
        let all: Vec<u8> = (0u8..32).collect();
        assert_eq!(crc_hqx(&all, 0), 54015);
    }

    #[test]
    fn payload_round_trips() {
        let v = PyValue::Tuple(vec![
            PyValue::Int(170_472),
            PyValue::Real(24.01),
            PyValue::Bytes(b"V24.01@ccp".to_vec()),
            PyValue::None,
        ]);
        assert_eq!(loads(&dumps(&v)).unwrap(), v);
    }

    #[test]
    fn payload_starts_with_the_empty_header() {
        let bytes = dumps(&PyValue::None);
        assert_eq!(&bytes[..5], &[0x7E, 0, 0, 0, 0]);
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = dumps(&PyValue::Int(1));
        bytes.push(0x00);
        assert!(loads(&bytes).is_err());
    }

    #[test]
    fn truncated_payload_is_refused() {
        let bytes = dumps(&PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)]));
        for cut in 1..bytes.len() {
            assert!(
                loads(&bytes[..cut]).is_err(),
                "a payload cut to {cut} bytes was accepted"
            );
        }
    }

    #[test]
    fn crypto_hash_is_a_decimal_string_of_the_argument_tuple() {
        let h = crypto_hash(&[PyValue::Bytes(vec![b'A'; 64])]);
        assert!(h.chars().all(|c| c.is_ascii_digit()), "not decimal: {h}");
        // The tuple, not the bare element: hashing the element must differ.
        let bare = crc_hqx(&blue_marshal(&PyValue::Bytes(vec![b'A'; 64])), 0).to_string();
        assert_ne!(h, bare);
    }

    #[test]
    fn blue_marshal_encodes_a_string_the_way_the_client_does() {
        // A 64-byte string: tag 0x13, one length byte 0x40, then the bytes.
        // The real client marshals every string this way (confirmed from its
        // H2/H4/H6 frames); STRING_SHORT (0x10), which our general encoder
        // uses, would produce a different CRC and fail the handshake.
        let mut got = Vec::new();
        blue_save_str(&[0u8; 64], &mut got);
        assert_eq!(got[0], 0x13);
        assert_eq!(got[1], 0x40);
        assert_eq!(got.len(), 2 + 64);
    }

    #[test]
    fn crypto_hash_matches_the_real_client_for_the_placebo_challenge() {
        // Under Placebo the client challenge is 64 zero bytes, so the hash the
        // client expects is deterministic. The first real client rejected our
        // value 654 (produced by the old 0x10 encoding); the client marshals
        // (challenge,) as 7E 00 00 00 00  25  13 40  <64 zeros> and CRCs that.
        let expected = {
            let mut v = vec![0x7E, 0, 0, 0, 0, 0x25, 0x13, 0x40];
            v.extend_from_slice(&[0u8; 64]);
            crc_hqx(&v, 0).to_string()
        };
        let got = crypto_hash(&[PyValue::Bytes(vec![0u8; 64])]);
        assert_eq!(got, expected);
        assert_ne!(got, "654", "the old STRING_SHORT encoding is back");
    }

    #[test]
    fn blue_marshal_tuple_tags_match_the_special_forms() {
        let mut t0 = Vec::new();
        blue_save_tuple(&[], &mut t0);
        assert_eq!(t0, vec![0x24]);
        let mut t1 = Vec::new();
        blue_save_tuple(&[PyValue::None], &mut t1);
        assert_eq!(t1, vec![0x25, 0x01]);
        let mut t2 = Vec::new();
        blue_save_tuple(&[PyValue::None, PyValue::None], &mut t2);
        assert_eq!(t2, vec![0x2C, 0x01, 0x01]);
        let mut t3 = Vec::new();
        blue_save_tuple(&[PyValue::None, PyValue::None, PyValue::None], &mut t3);
        assert_eq!(t3, vec![0x14, 0x03, 0x01, 0x01, 0x01]);
    }

    // --- the inbound limits (docs/review/V2-wire.md F2) -------------------

    /// zlib around `data` in stored (uncompressed) blocks.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut chunks = data.chunks(0xFFFF).peekable();
        while let Some(chunk) = chunks.next() {
            out.push(u8::from(chunks.peek().is_none()));
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        out.extend_from_slice(&eve_inflate::adler32(data).to_be_bytes());
        out
    }

    /// A marshal stream of exactly `len` bytes: one `str` of filler.
    fn plain_stream_of(len: usize) -> Vec<u8> {
        let mut w = Writer::new();
        write_empty_header(&mut w);
        let head = w.into_bytes().len() + 1 + 5; // header, tag, escaped size
        let s = PyValue::Bytes(vec![b'x'; len - head]);
        let bytes = dumps(&s);
        assert_eq!(bytes.len(), len);
        bytes
    }

    #[test]
    fn an_inbound_zlib_payload_past_the_inbound_cap_is_refused() {
        let at_cap = zlib_stored(&plain_stream_of(MAX_INBOUND_INFLATED));
        assert!(loads(&at_cap).is_ok(), "exactly the cap is accepted");

        let past = zlib_stored(&plain_stream_of(MAX_INBOUND_INFLATED + 1));
        assert_eq!(
            loads(&past),
            Err(WireError::Malformed {
                at: 0,
                why: "payload claims to be zlib but does not inflate",
            })
        );
        assert!(
            loads_unbudgeted(&past).is_ok(),
            "the server's own output keeps the larger cap"
        );
    }

    #[test]
    fn an_inbound_payload_past_the_decode_budget_is_refused() {
        // `[shared list of k Nones, then refs back-references to it]`, in a
        // stream whose id table holds the one id.
        let k = 1000;
        let refs = Budget::INBOUND.nodes / k + 1;
        let mut w = Writer::new();
        w.u8(0x7E);
        w.i32(1);
        w.u8(eve_marshal::tag::code::LIST);
        w.size(refs + 1);
        w.u8(eve_marshal::tag::code::LIST | eve_marshal::tag::SHARED_FLAG);
        w.size(k);
        for _ in 0..k {
            w.u8(eve_marshal::tag::code::NONE);
        }
        for _ in 0..refs {
            w.u8(eve_marshal::tag::code::SHARED_REF);
            w.size(1);
        }
        w.i32(1);
        let bytes = w.into_bytes();

        assert!(matches!(
            loads(&bytes),
            Err(WireError::Malformed {
                why: "decode budget",
                ..
            })
        ));
        assert!(matches!(
            loads_unbudgeted(&bytes),
            Ok(PyValue::List(items)) if items.len() == refs + 1
        ));
    }
}
