//! The Placebo crypto pack, from the *client's* side.
//!
//! `evecrypto/placebo.py` is four functions, and a probe needs three of them:
//!
//! ```text
//! crypto_hash(*args) = str(binascii.crc_hqx(blue.marshal.Save(args), 0))
//! get_random_bytes(n) = '\x00' * n
//! create_context().Initialize() = {}
//! ```
//!
//! Two of those are traps.
//!
//! **The hash is a CRC over bytes, so the encoder has to be byte-exact.** It
//! is not a hash of the value; it is a hash of *one particular marshalling*
//! of the value. `docs/reference/login-path.md` §4 records the cost of learning this
//! from the other side: our server CRC'd a `0x10`-tagged string where the
//! client had written `0x13`, the two decode identically, and the client
//! refused the handshake with "the server's crypto hash wasn't correct".
//! [`blue_save`] is the client's encoder, not the general one.
//!
//! **`get_random_bytes` returns zeros**, so `clientChallenge` is 64 NUL bytes
//! and every Placebo handshake hashes the same values. That is why EvEJS can
//! hardcode `challenge_responsehash: "55087"` and get away with it
//! (`server/src/network/tcp/handshake.js`) — and why the vectors in this
//! module's tests are the real values off a real wire.

use eve_marshal::{PyValue, Writer, save, tag::code as c};

use crate::sha1::sha1;

/// CRC-16/XMODEM, which is what `binascii.crc_hqx` computes: polynomial
/// `0x1021`, no input or output reflection, no final XOR, and the initial
/// value supplied by the caller (the Placebo pack passes `0`).
pub fn crc_hqx(data: &[u8], init: u16) -> u16 {
    let mut crc = init;
    for &byte in data {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// `blue.marshal.Save`, byte-for-byte as *this client build writes it*.
///
/// Only the tags the client is observed to emit, because the point of this
/// function is to agree with the client's bytes and nothing else:
///
/// | value | tag |
/// |---|---|
/// | `str` | `0x0E` empty, else `0x13` + escaped length + bytes |
/// | `unicode` | `0x28` empty, else `0x2E` + escaped length + UTF-8 |
/// | tuple | `0x24` / `0x25` / `0x2C` / `0x14` + count |
/// | list | `0x26` / `0x27` / `0x15` + count |
/// | dict | `0x16` + count, entries as (value, key) |
/// | int | `0x07` / `0x08` / `0x09` for -1/0/1, else narrowest of `0x06`/`0x05`/`0x04`/`0x03` |
///
/// Anything else falls through to [`eve_marshal::save`], which is right for
/// the general codec and may well be wrong for a CRC. `CryptoHash` is only
/// ever called on a string in this protocol (`CaseFold(user_name)`,
/// `clientChallenge`, `serverChallenge`), so the fallback is unreachable on
/// the login path; it exists so this function is not a partial one.
///
/// The string case is pinned by two values recovered from a capture of the
/// real client against EvEJS — see this module's tests.
pub fn blue_save(v: &PyValue) -> Vec<u8> {
    let mut w = Writer::new();
    // The stream header the client writes: `0x7E` plus a zero mapcount. The
    // CRC covers it, so it is part of the hash input.
    w.u8(0x7E);
    w.i32(0);
    blue_write(v, &mut w);
    w.into_bytes()
}

fn blue_write(v: &PyValue, w: &mut Writer) {
    match v {
        PyValue::None => w.u8(c::NONE),
        PyValue::Bool(true) => w.u8(c::TRUE),
        PyValue::Bool(false) => w.u8(c::FALSE),
        PyValue::Int(i) => match *i {
            -1 => w.u8(c::INT_MINUS_ONE),
            0 => w.u8(c::INT_ZERO),
            1 => w.u8(c::INT_ONE),
            n if i8::try_from(n).is_ok() => {
                w.u8(c::INT8);
                w.u8(n as u8);
            }
            n if i16::try_from(n).is_ok() => {
                w.u8(c::INT16);
                w.i16(n as i16);
            }
            n if i32::try_from(n).is_ok() => {
                w.u8(c::INT32);
                w.i32(n as i32);
            }
            n => {
                w.u8(c::INT64);
                w.i64(n);
            }
        },
        PyValue::Bytes(b) => {
            if b.is_empty() {
                w.u8(c::STRING_EMPTY);
            } else {
                // 0x13, not 0x10. This single byte is the whole lesson of
                // this module.
                w.u8(c::STRING_ALT);
                w.size(b.len());
                w.bytes(b);
            }
        }
        PyValue::Unicode(s) => {
            if s.is_empty() {
                w.u8(c::UNICODE_EMPTY);
            } else {
                w.u8(c::UNICODE_UTF8);
                w.size(s.len());
                w.bytes(s.as_bytes());
            }
        }
        PyValue::Tuple(items) => match items.len() {
            0 => w.u8(c::TUPLE_EMPTY),
            1 => {
                w.u8(c::TUPLE1);
                blue_write(&items[0], w);
            }
            2 => {
                w.u8(c::TUPLE2);
                blue_write(&items[0], w);
                blue_write(&items[1], w);
            }
            n => {
                w.u8(c::TUPLE);
                w.size(n);
                for i in items {
                    blue_write(i, w);
                }
            }
        },
        PyValue::List(items) => match items.len() {
            0 => w.u8(c::LIST_EMPTY),
            1 => {
                w.u8(c::LIST1);
                blue_write(&items[0], w);
            }
            n => {
                w.u8(c::LIST);
                w.size(n);
                for i in items {
                    blue_write(i, w);
                }
            }
        },
        PyValue::Dict(entries) => {
            w.u8(c::DICT);
            w.size(entries.len());
            for (value, key) in entries {
                blue_write(value, w);
                blue_write(key, w);
            }
        }
        other => save(other, w),
    }
}

/// `evecrypto.crypto.CryptoHash(*args)` under the Placebo pack.
///
/// It is **varargs**, so one argument is hashed as a 1-tuple. Getting that
/// wrong is the second-easiest way to fail the handshake and gives no clue
/// which way it was wrong.
pub fn crypto_hash(args: &[PyValue]) -> String {
    let tuple = PyValue::Tuple(args.to_vec());
    crc_hqx(&blue_save(&tuple), 0).to_string()
}

/// `machobase.CaseFold`, as the login path uses it on the user name.
///
/// ASCII lowercase. **[V]** against a capture: the real client sent
/// `CryptoHash(CaseFold('EVE-SSO-CONNECTION')) == "2470"`, which is the CRC
/// of the lowercased form and of no other case folding of it (see tests).
pub fn case_fold(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// `evecrypto.placebo.get_random_bytes(n)`: zeros.
///
/// Not a stand-in. The function really does return `'\x00' * n`, so a probe
/// that generated real randomness here would be the one speaking a different
/// protocol.
pub fn placebo_random(n: usize) -> Vec<u8> {
    vec![0u8; n]
}

/// `machobase.PasswordHash(userName, password)`.
///
/// utf-16-le of the stripped, lowercased user name as salt;
/// `sha1(utf16le(password) + salt)`, then 1000 further
/// `sha1(digest + salt)`. The 20 raw bytes.
///
/// **[I]** on the exact iteration count and ordering: it is transcribed from
/// `docs/reference/login-path.md` §4 and has never been checked against a server that
/// verifies it. EvEJS does not — `config/server.json` sets
/// `devSkipPasswordValidation: true`, and its handler only re-hexes the
/// field — so a probe run cannot falsify this. What would: our own server
/// computing the same hash for the same credentials as a real client's
/// capture of them.
pub fn password_hash(user_name: &str, password: &str) -> [u8; 20] {
    let salt = utf16le(&user_name.trim().to_lowercase());
    let mut buf = utf16le(password);
    buf.extend_from_slice(&salt);
    let mut digest = sha1(&buf);
    for _ in 0..1000 {
        let mut next = Vec::with_capacity(20 + salt.len());
        next.extend_from_slice(&digest);
        next.extend_from_slice(&salt);
        digest = sha1(&next);
    }
    digest
}

fn utf16le(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC-16/XMODEM's published check value: `"123456789"` -> `0x31C3`.
    /// Tests the algorithm, not our use of it.
    #[test]
    fn crc_hqx_matches_the_published_check_value() {
        assert_eq!(crc_hqx(b"123456789", 0), 0x31C3);
        assert_eq!(crc_hqx(b"", 0), 0);
    }

    /// **Two values read off a real wire.**
    ///
    /// `/tmp/evejs-session.pcap`, the 2026-10-20 session of the real V24.01
    /// client against EvEJS, carries the client's own H3 and H9:
    ///
    /// ```text
    /// H3  14 03 01 13 02 "VK" 13 04 "2470"
    /// H9  14 03 13 05 "44596" ...
    /// ```
    ///
    /// So the client computed `CryptoHash(CaseFold('EVE-SSO-CONNECTION'))` as
    /// `"2470"` and `CryptoHash('')` — EvEJS sends an empty serverChallenge —
    /// as `"44596"`. These are the client's arithmetic, not ours, which makes
    /// them the only test here that could tell us our encoder is wrong.
    #[test]
    fn crypto_hash_agrees_with_the_real_client() {
        let folded = case_fold("EVE-SSO-CONNECTION");
        assert_eq!(folded, "eve-sso-connection");
        assert_eq!(
            crypto_hash(&[PyValue::Bytes(folded.into_bytes())]),
            "2470",
            "H3 vipKey from the capture"
        );
        assert_eq!(
            crypto_hash(&[PyValue::Bytes(Vec::new())]),
            "44596",
            "H9 response to EvEJS's empty serverChallenge"
        );
    }

    /// The unfolded and title-cased forms give different values, so the
    /// previous test really does pin the folding and not just the encoder.
    #[test]
    fn case_folding_changes_the_hash() {
        assert_ne!(
            crypto_hash(&[PyValue::Bytes(b"EVE-SSO-CONNECTION".to_vec())]),
            "2470"
        );
        assert_ne!(
            crypto_hash(&[PyValue::Bytes(b"Eve-Sso-Connection".to_vec())]),
            "2470"
        );
    }

    /// The value a server must send back for a Placebo client challenge, and
    /// the one EvEJS hardcodes. If this number ever moves, either the
    /// challenge stopped being 64 zeros or the encoder stopped matching.
    #[test]
    fn the_zero_challenge_hashes_to_what_evejs_hardcodes() {
        let challenge = placebo_random(64);
        assert_eq!(challenge, vec![0u8; 64]);
        assert_eq!(crypto_hash(&[PyValue::Bytes(challenge)]), "55087");
    }

    /// The 1-tuple wrapping is load-bearing: hashing the bare string gives a
    /// different answer, and that is a failure with no diagnostic attached.
    #[test]
    fn varargs_wrapping_is_not_optional() {
        let bare = crc_hqx(&blue_save(&PyValue::Bytes(b"".to_vec())), 0).to_string();
        assert_ne!(bare, "44596");
    }

    /// `0x13` versus `0x10`: the two encode the same string and CRC
    /// differently. This is the defect `docs/reference/login-path.md` §4 records, kept
    /// as a test so the encoder cannot quietly drift back to the general one.
    #[test]
    fn the_general_encoder_would_give_a_different_crc() {
        let v = PyValue::Bytes(b"eve-sso-connection".to_vec());
        let mut w = Writer::new();
        eve_marshal::write_empty_header(&mut w);
        save(&PyValue::Tuple(vec![v.clone()]), &mut w);
        let general = crc_hqx(&w.into_bytes(), 0).to_string();
        assert_ne!(general, "2470", "the general encoder must not match");
        assert_eq!(crypto_hash(&[v]), "2470");
    }

    /// A string of 0xFF bytes or more takes the escaped-length path. No
    /// capture exercises it (every `CryptoHash` argument on the login path is
    /// at most 64 bytes), so this only pins that we use the same length
    /// convention as the rest of the codec rather than inventing one.
    #[test]
    fn long_strings_use_the_escaped_length() {
        let bytes = blue_save(&PyValue::Bytes(vec![b'x'; 300]));
        assert_eq!(&bytes[5..6], &[c::STRING_ALT]);
        assert_eq!(&bytes[6..11], &[0xFF, 0x2C, 0x01, 0x00, 0x00]);
    }

    /// The password hash is 20 bytes and depends on both inputs. It cannot be
    /// checked against a real value from here (see [`password_hash`]), so
    /// this asserts only the shape and that neither argument is ignored —
    /// which is the failure a one-line transcription error produces.
    #[test]
    fn password_hash_shape_and_sensitivity() {
        let a = password_hash("probe", "secret");
        let b = password_hash("probe", "secret2");
        let c = password_hash("probe2", "secret");
        assert_eq!(a.len(), 20);
        assert_ne!(a, b, "password must affect the digest");
        assert_ne!(a, c, "user name is the salt, so it must affect the digest");
        // The salt is case-folded and stripped, so these are the same account.
        assert_eq!(a, password_hash("  PROBE  ", "secret"));
    }
}
