//! Q-FUZZ regressions for `eve-inflate`.

use std::time::{Duration, Instant};

use eve_inflate::{InflateError, adler32, inflate_zlib_capped};

/// Millions of empty blocks: ten bits each (BFINAL 0, BTYPE 01, end-of-block
/// 0000000) for a fixed block. Each used to rebuild both Huffman trees.
#[test]
fn a_frame_of_empty_fixed_blocks_is_linear_and_quick() {
    let blocks = 2_000_000usize; // 2.5 MB of input: half the 5 MB frame cap
    let mut bits: Vec<u8> = Vec::with_capacity(blocks * 10 / 8 + 8);
    let (mut acc, mut n) = (0u64, 0u32);
    let mut push = |v: u32, k: u32, bits: &mut Vec<u8>| {
        acc |= u64::from(v) << n;
        n += k;
        while n >= 8 {
            bits.push(acc as u8);
            acc >>= 8;
            n -= 8;
        }
    };
    for _ in 0..blocks {
        push(0, 1, &mut bits); // not final
        push(1, 2, &mut bits); // fixed
        push(0, 7, &mut bits); // end of block (symbol 256 is 0000000)
    }
    push(1, 1, &mut bits);
    push(1, 2, &mut bits);
    push(0, 7, &mut bits);
    push(0, 8, &mut bits); // flush
    let mut z = vec![0x78, 0x01];
    z.extend_from_slice(&bits);
    z.extend_from_slice(&adler32(&[]).to_be_bytes());
    let started = Instant::now();
    let out = inflate_zlib_capped(&z, 1 << 20).expect("empty blocks inflate to nothing");
    assert!(out.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// The zip bomb the cap exists for: one literal and a million
/// length-258/distance-1 matches. Refused at the cap, not after the copy.
#[test]
fn a_run_length_bomb_stops_at_the_cap() {
    // fixed block: 'a' = 0x91 (8 bits 10010001), match 258 = symbol 285
    // (8 bits 11000101), distance code 0 (5 bits 00000); written MSB first
    let mut bits: Vec<u8> = Vec::new();
    let (mut acc, mut n) = (0u64, 0u32);
    let mut put = |v: u32, k: u32, msb_first: bool, bits: &mut Vec<u8>| {
        for i in 0..k {
            let b = if msb_first {
                (v >> (k - 1 - i)) & 1
            } else {
                (v >> i) & 1
            };
            acc |= u64::from(b) << n;
            n += 1;
            if n == 8 {
                bits.push(acc as u8);
                acc = 0;
                n = 0;
            }
        }
    };
    put(1, 1, false, &mut bits);
    put(1, 2, false, &mut bits);
    put(0x30 + u32::from(b'a'), 8, true, &mut bits);
    for _ in 0..1_000_000 {
        put(0xC5, 8, true, &mut bits);
        put(0, 5, true, &mut bits);
    }
    put(0, 7, true, &mut bits);
    put(0, 8, false, &mut bits);
    let mut z = vec![0x78, 0x01];
    z.extend_from_slice(&bits);
    z.extend_from_slice(&[0, 0, 0, 0]);
    for cap in [0usize, 5, 257, 259, 1 << 20] {
        assert_eq!(
            inflate_zlib_capped(&z, cap),
            Err(InflateError::OutputTooLarge),
            "cap {cap}"
        );
    }
}
