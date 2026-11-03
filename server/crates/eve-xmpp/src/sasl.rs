//! SASL PLAIN (RFC 4616) and the base64 it rides in (RFC 4648 §4).
//!
//! The client sends `base64("\0" + charid + "\0" + password)` with an empty
//! authorization id (`xmpphandler.authenticate`, `docs/lanes/chat.md` §2 [V]).

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with `=` padding.
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Strict standard base64: whitespace is not skipped, padding is required.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let t = text.as_bytes();
    if !t.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32);
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (i, quad) in t.chunks(4).enumerate() {
        let last = i == t.len() / 4 - 1;
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// The body of a PLAIN `<auth>`: `\0authcid\0password`, base64.
pub fn plain_encode(authcid: &str, password: &str) -> String {
    base64_encode(format!("\0{authcid}\0{password}").as_bytes())
}

/// `(authzid, authcid, password)` out of a PLAIN `<auth>` body.
pub fn plain_decode(text: &str) -> Option<(String, String, String)> {
    let raw = base64_decode(text.trim())?;
    let mut parts = raw.split(|&b| b == 0);
    let authzid = parts.next()?;
    let authcid = parts.next()?;
    let password = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let s = |b: &[u8]| String::from_utf8(b.to_vec()).ok();
    Some((s(authzid)?, s(authcid)?, s(password)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 §10 test vectors.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, enc) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), plain.as_bytes());
        }
        assert_eq!(base64_decode("Zg="), None);
        assert_eq!(base64_decode("Zg==Zg=="), None);
        assert_eq!(base64_decode("Z!=="), None);
    }

    #[test]
    fn plain_round_trips_with_an_empty_authzid() {
        let enc = plain_encode("90000001", "token");
        assert_eq!(enc, "ADkwMDAwMDAxAHRva2Vu");
        assert_eq!(
            plain_decode(&enc),
            Some((String::new(), "90000001".to_owned(), "token".to_owned()))
        );
        assert_eq!(plain_decode(&base64_encode(b"no-nuls")), None);
    }
}
