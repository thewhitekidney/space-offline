//! SHA-1, because `machobase.PasswordHash` is 1001 of them and the server is
//! std-only.
//!
//! This is here for one reason: the handshake's `user_password_hash` field.
//! Nothing else in the probe hashes anything (the Placebo crypto pack's
//! `CryptoHash` is a CRC — see [`crate::crypto`]), and SHA-1 is used here as a
//! *protocol constant*, not as a security primitive.
//!
//! FIPS 180-1 appendix test vectors pin it below.

/// SHA-1 of `data`, as 20 bytes.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    // Message schedule over the padded message, a block at a time, without
    // materialising the padded copy: the tail is at most two blocks.
    let bitlen = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(128);
    let whole = data.len() / 64 * 64;
    tail.extend_from_slice(&data[whole..]);
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&bitlen.to_be_bytes());

    for block in data[..whole].chunks_exact(64).chain(tail.chunks_exact(64)) {
        compress(&mut h, block);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn compress(h: &mut [u32; 5], block: &[u8]) {
    debug_assert_eq!(block.len(), 64);
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }

    let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

/// Lowercase hex, for transcripts.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('?'));
        s.push(char::from_digit(u32::from(b & 0xF), 16).unwrap_or('?'));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-1 appendix A/B/C, plus the empty string. These are the
    /// published vectors, so they test SHA-1 rather than this implementation
    /// of it.
    #[test]
    fn fips_vectors() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            hex(&sha1(&b"a".repeat(1_000_000))),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    /// The block boundary is where a hand-rolled padding goes wrong, so walk
    /// across it: 55, 56, 57 bytes straddle the "length does not fit this
    /// block" case.
    #[test]
    fn padding_across_the_block_boundary() {
        // Independent values: each is the SHA-1 of n 'a's, from the same
        // published algorithm, computed by a reference implementation.
        assert_eq!(
            hex(&sha1(&b"a".repeat(55))),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            hex(&sha1(&b"a".repeat(56))),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
        assert_eq!(
            hex(&sha1(&b"a".repeat(57))),
            "f08f24908d682555111be7ff6f004e78283d989a"
        );
        assert_eq!(
            hex(&sha1(&b"a".repeat(64))),
            "0098ba824b5c16427bd7a1122a5a442a25ec644d"
        );
    }
}
