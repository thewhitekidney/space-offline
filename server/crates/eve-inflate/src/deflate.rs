//! DEFLATE (RFC 1951) compression, from scratch, for large server replies.
//!
//! The real server compresses marshal streams past a size threshold and the
//! client inflates any stream whose first byte is not `0x7D`/`0x7E`
//! (`eve_marshal::constants::compression`, **[V]**). This is the matching
//! writer: an LZ77 matcher (hash chains, 32 KiB window) feeding **one block
//! of the fixed Huffman code** (RFC 1951 §3.2.6). Fixed codes compress a
//! little worse than dynamic ones, but marshal output is overwhelmingly
//! repeated field names and similar rows, which the matcher catches, and the
//! fixed code needs no tree building to get wrong. No `unsafe`.
//!
//! Output is a complete zlib stream ([`deflate_zlib`]); [`crate::inflate_zlib`]
//! reads it back and the tests do exactly that.

use crate::adler32;

const WINDOW: usize = 32 * 1024;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Hash-chain candidates tried per position. A bound on the work, not on
/// correctness: any match found is valid.
const MAX_CHAIN: usize = 24;
const HASH_BITS: u32 = 15;
const NIL: u32 = u32::MAX;

/// (base length, extra bits) for length codes 257..=285.
const LEN_TABLE: [(u16, u8); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// (base distance, extra bits) for distance codes 0..=29.
const DIST_TABLE: [(u16, u8); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

/// LSB-first bit sink.
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn bits(&mut self, value: u32, count: u32) {
        self.acc |= u64::from(value) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code goes out most-significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let mut rev = 0u32;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.bits(rev, len);
    }

    /// A literal/length symbol under the fixed code.
    fn symbol(&mut self, s: u32) {
        match s {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + (s - 144), 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xC0 + (s - 280), 8),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn hash3(d: &[u8]) -> usize {
    let v = u32::from(d[0]) | u32::from(d[1]) << 8 | u32::from(d[2]) << 16;
    (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

fn put_match(w: &mut BitWriter, len: usize, dist: usize) {
    let li = LEN_TABLE
        .iter()
        .rposition(|&(base, _)| usize::from(base) <= len)
        .unwrap_or(0);
    let (lbase, lextra) = LEN_TABLE[li];
    w.symbol(257 + li as u32);
    w.bits((len - usize::from(lbase)) as u32, u32::from(lextra));
    let di = DIST_TABLE
        .iter()
        .rposition(|&(base, _)| usize::from(base) <= dist)
        .unwrap_or(0);
    let (dbase, dextra) = DIST_TABLE[di];
    w.code(di as u32, 5);
    w.bits((dist - usize::from(dbase)) as u32, u32::from(dextra));
}

/// Compress `data` as a zlib stream (`78 01` header, one fixed-Huffman block,
/// Adler-32 trailer).
pub fn deflate_zlib(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter {
        out: vec![0x78, 0x01],
        acc: 0,
        n: 0,
    };
    w.bits(1, 1); // BFINAL
    w.bits(1, 2); // BTYPE = 01, fixed Huffman

    let mut head = vec![NIL; 1 << HASH_BITS];
    let mut prev = vec![NIL; WINDOW];
    let insert = |head: &mut [u32], prev: &mut [u32], data: &[u8], i: usize| {
        if i + MIN_MATCH <= data.len() {
            let h = hash3(&data[i..]);
            prev[i % WINDOW] = head[h];
            head[h] = i as u32;
        }
    };

    let mut i = 0;
    while i < data.len() {
        let mut best_len = 0;
        let mut best_dist = 0;
        if i + MIN_MATCH <= data.len() {
            let max = MAX_MATCH.min(data.len() - i);
            let mut cand = head[hash3(&data[i..])];
            let mut tries = 0;
            while cand != NIL && tries < MAX_CHAIN {
                let c = cand as usize;
                if c >= i || i - c > WINDOW {
                    break;
                }
                let mut l = 0;
                while l < max && data[c + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - c;
                    if l == max {
                        break;
                    }
                }
                cand = prev[c % WINDOW];
                tries += 1;
            }
        }
        if best_len >= MIN_MATCH {
            put_match(&mut w, best_len, best_dist);
            for k in 0..best_len {
                insert(&mut head, &mut prev, data, i + k);
            }
            i += best_len;
        } else {
            w.symbol(u32::from(data[i]));
            insert(&mut head, &mut prev, data, i);
            i += 1;
        }
    }
    w.symbol(256);
    let mut out = w.finish();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflate_zlib;

    fn round(data: &[u8]) -> usize {
        let z = deflate_zlib(data);
        assert!(crate::looks_like_zlib(&z));
        assert_eq!(inflate_zlib(&z).expect("inflates"), data);
        z.len()
    }

    #[test]
    fn round_trips_edge_cases() {
        round(b"");
        round(b"a");
        round(b"ab");
        round(b"abcabcabcabcabc");
        round(&[0u8; 100_000]);
        let counting: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        round(&counting);
    }

    #[test]
    fn round_trips_noise_and_repetition_over_the_window() {
        // A deterministic xorshift stream: incompressible, then the same
        // block again 40 KiB later (past the 32 KiB window, so unmatched).
        let mut x = 0x1234_5678u32;
        let mut block = Vec::new();
        for _ in 0..40_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            block.push(x as u8);
        }
        let mut data = block.clone();
        data.extend_from_slice(&block);
        round(&data);
    }

    #[test]
    fn repetitive_marshal_like_rows_shrink_a_lot() {
        let mut data = Vec::new();
        for i in 0..5_000u32 {
            data.extend_from_slice(b"\x1b\x00\x00\x00price\x05volRemaining\x03typeID");
            data.extend_from_slice(&i.to_le_bytes());
        }
        let n = round(&data);
        assert!(n * 5 < data.len(), "{n} of {}", data.len());
    }
}
