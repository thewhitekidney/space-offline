//! Q-FUZZ: seeded, deterministic fuzzing of `eve-inflate`.
//!
//! Std only: SplitMix64 plus a small DEFLATE *encoder* written for the test
//! (stored, fixed-Huffman and dynamic-Huffman blocks with random LZ77
//! matches and random but complete code shapes), so the decoder is fuzzed
//! both ways: valid streams must round-trip, and mutated or random ones must
//! fail cleanly.
//!
//! Properties, for any input:
//! - never panics, and terminates (a wall-clock bound per case);
//! - never produces more than the caller's cap (zip bombs stop at the cap);
//! - `inflate(encode(x)) == x` for generated `x`, on every block type;
//! - every strict prefix of a valid zlib stream is an error.
//!
//! Normal run: [`CASES`] per target. Overnight:
//!
//! ```text
//! EVE_FUZZ_SEED=7 EVE_FUZZ_CASES=5000000 \
//!   tools/dev.sh cargo test --locked -p eve-inflate --test fuzz --release -- --ignored --nocapture
//! ```

use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use eve_inflate::{
    InflateError, adler32, inflate_raw, inflate_zlib, inflate_zlib_capped, looks_like_zlib,
};

const CASES: u64 = 20_000;
const CASE_TIME: Duration = Duration::from_secs(5);

pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below((hi - lo + 1) as u64) as usize
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

fn hex(b: &[u8]) -> String {
    let mut s = String::new();
    for x in b.iter().take(4096) {
        s.push_str(&format!("{x:02x}"));
    }
    if b.len() > 4096 {
        s.push_str(&format!("...({} bytes)", b.len()));
    }
    s
}

fn env_run() -> (u64, u64) {
    let seed = std::env::var("EVE_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let cases = std::env::var("EVE_FUZZ_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);
    eprintln!("q-fuzz: seed={seed} cases={cases}");
    (seed, cases)
}

fn guarded(seed: u64, case: u64, what: &str, bytes: &[u8], f: impl FnOnce(&[u8])) {
    let r = panic::catch_unwind(AssertUnwindSafe(|| f(bytes)));
    if let Err(e) = r {
        let msg = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        panic!(
            "q-fuzz FAILURE ({what}) seed={seed} case={case}: {msg}\n  input ({} bytes): {}",
            bytes.len(),
            hex(bytes)
        );
    }
}

// --- a DEFLATE encoder, for the round trip ----------------------------------

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }
    /// LSB first, as DEFLATE packs everything but Huffman codes.
    fn bits(&mut self, v: u32, n: u32) {
        self.acc |= u64::from(v) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// A Huffman code goes in MSB first.
    fn code(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.bits((code >> i) & 1, 1);
        }
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

#[derive(Clone, Copy)]
enum Tok {
    Lit(u8),
    Match { len: usize, dist: usize },
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn len_sym(len: usize) -> (usize, u32, u32) {
    let i = (0..29)
        .rev()
        .find(|&i| LENGTH_BASE[i] as usize <= len)
        .unwrap();
    (
        257 + i,
        (len - LENGTH_BASE[i] as usize) as u32,
        LENGTH_EXTRA[i] as u32,
    )
}

fn dist_sym(dist: usize) -> (usize, u32, u32) {
    let i = (0..30)
        .rev()
        .find(|&i| DIST_BASE[i] as usize <= dist)
        .unwrap();
    (
        i,
        (dist - DIST_BASE[i] as usize) as u32,
        DIST_EXTRA[i] as u32,
    )
}

/// Canonical codes for a list of lengths (RFC 1951 §3.2.2).
fn canonical(lengths: &[u8]) -> Vec<(u32, u32)> {
    let mut count = [0u32; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                (0, 0)
            } else {
                let c = next[l as usize];
                next[l as usize] += 1;
                (c, u32::from(l))
            }
        })
        .collect()
}

/// Random *complete* prefix-code lengths for `n` leaves, none deeper than 15,
/// by splitting random leaves of a binary tree.
fn random_complete_lengths(rng: &mut Rng, n: usize) -> Vec<u8> {
    if n == 1 {
        return vec![1];
    }
    let mut leaves = vec![0u8]; // depths
    while leaves.len() < n {
        let i = rng.below(leaves.len() as u64) as usize;
        if leaves[i] >= 15 {
            // find any splittable leaf
            if let Some(j) = leaves.iter().position(|&d| d < 15) {
                let d = leaves[j];
                leaves[j] = d + 1;
                leaves.push(d + 1);
            } else {
                break;
            }
        } else {
            let d = leaves[i];
            leaves[i] = d + 1;
            leaves.push(d + 1);
        }
    }
    leaves
}

fn gen_tokens(rng: &mut Rng, target: usize) -> (Vec<Tok>, Vec<u8>) {
    let mut toks = Vec::new();
    let mut out: Vec<u8> = Vec::new();
    let alphabet = *[2u64, 4, 16, 256].get(rng.below(4) as usize).unwrap();
    while out.len() < target {
        if out.is_empty() || rng.chance(55) {
            let b = rng.below(alphabet) as u8;
            toks.push(Tok::Lit(b));
            out.push(b);
        } else {
            let maxd = out.len().min(32768);
            let dist = if rng.chance(30) {
                1
            } else {
                rng.range(1, maxd)
            };
            let len = if rng.chance(20) {
                258
            } else {
                rng.range(3, 258)
            };
            toks.push(Tok::Match { len, dist });
            for _ in 0..len {
                let b = out[out.len() - dist];
                out.push(b);
            }
        }
    }
    (toks, out)
}

fn write_tokens(w: &mut BitWriter, toks: &[Tok], lit: &[(u32, u32)], dist: &[(u32, u32)]) {
    for t in toks {
        match *t {
            Tok::Lit(b) => {
                let (c, l) = lit[b as usize];
                w.code(c, l);
            }
            Tok::Match { len, dist: d } => {
                let (s, extra, nb) = len_sym(len);
                let (c, l) = lit[s];
                w.code(c, l);
                w.bits(extra, nb);
                let (ds, dextra, dnb) = dist_sym(d);
                let (c, l) = dist[ds];
                w.code(c, l);
                w.bits(dextra, dnb);
            }
        }
    }
    let (c, l) = lit[256];
    w.code(c, l);
}

fn fixed_lengths() -> (Vec<u8>, Vec<u8>) {
    let mut l = vec![0u8; 288];
    for (i, x) in l.iter_mut().enumerate() {
        *x = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    (l, vec![5; 30])
}

/// A dynamic block: random complete literal/length and distance codes over
/// just the symbols the tokens use (plus EOB), the lengths run-length coded
/// with a random complete code-length code.
fn dynamic_block(w: &mut BitWriter, rng: &mut Rng, toks: &[Tok], last: bool) {
    let mut lit_used = vec![false; 286];
    let mut dist_used = vec![false; 30];
    lit_used[256] = true;
    for t in toks {
        match *t {
            Tok::Lit(b) => lit_used[b as usize] = true,
            Tok::Match { len, dist } => {
                lit_used[len_sym(len).0] = true;
                dist_used[dist_sym(dist).0] = true;
            }
        }
    }
    // Optionally enable extra unused symbols so the shapes vary.
    for u in lit_used.iter_mut() {
        if rng.chance(3) {
            *u = true;
        }
    }
    let assign = |rng: &mut Rng, used: &[bool]| -> Vec<u8> {
        let ids: Vec<usize> = (0..used.len()).filter(|&i| used[i]).collect();
        let mut lens = vec![0u8; used.len()];
        if ids.is_empty() {
            return lens;
        }
        let mut shape = random_complete_lengths(rng, ids.len());
        // shuffle which symbol gets which depth
        for i in (1..shape.len()).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            shape.swap(i, j);
        }
        for (k, &id) in ids.iter().enumerate() {
            lens[id] = shape[k];
        }
        lens
    };
    let lit_lens = assign(rng, &lit_used);
    let mut dist_lens = assign(rng, &dist_used);
    if dist_lens.iter().all(|&l| l == 0) {
        // no matches: one distance code of length 1 is the RFC's allowance
        dist_lens[0] = 1;
    }
    let hlit = (257..=286)
        .rev()
        .find(|&n| lit_lens[n - 1] != 0)
        .unwrap()
        .max(257);
    let hdist = (1..=30).rev().find(|&n| dist_lens[n - 1] != 0).unwrap_or(1);
    let mut all: Vec<u8> = lit_lens[..hlit].to_vec();
    all.extend_from_slice(&dist_lens[..hdist]);

    // Run-length code the lengths with 16/17/18 sometimes.
    let mut syms: Vec<(u8, u32, u32)> = Vec::new(); // (symbol, extra, extra bits)
    let mut i = 0;
    while i < all.len() {
        let v = all[i];
        let mut run = 1;
        while i + run < all.len() && all[i + run] == v {
            run += 1;
        }
        if v == 0 && run >= 11 && rng.chance(80) {
            let n = run.min(138);
            syms.push((18, (n - 11) as u32, 7));
            i += n;
        } else if v == 0 && run >= 3 && rng.chance(80) {
            let n = run.min(10);
            syms.push((17, (n - 3) as u32, 3));
            i += n;
        } else if i > 0 && all[i - 1] == v && run >= 3 && rng.chance(80) {
            let n = run.min(6);
            syms.push((16, (n - 3) as u32, 2));
            i += n;
        } else {
            syms.push((v, 0, 0));
            i += 1;
        }
    }
    let mut cl_used = [false; 19];
    for s in &syms {
        cl_used[s.0 as usize] = true;
    }
    let cl_lens = {
        let ids: Vec<usize> = (0..19).filter(|&i| cl_used[i]).collect();
        let mut shape = random_complete_lengths(rng, ids.len());
        // the code-length code's lengths are 3 bits, so at most 7
        loop {
            if shape.iter().all(|&d| d <= 7) {
                break;
            }
            shape = random_complete_lengths(rng, ids.len());
        }
        for i in (1..shape.len()).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            shape.swap(i, j);
        }
        let mut lens = [0u8; 19];
        for (k, &id) in ids.iter().enumerate() {
            lens[id] = shape[k];
        }
        lens
    };
    let hclen = (4..=19)
        .rev()
        .find(|&n| cl_lens[CLEN_ORDER[n - 1]] != 0)
        .unwrap_or(4)
        .max(4);

    w.bits(u32::from(last), 1);
    w.bits(2, 2);
    w.bits((hlit - 257) as u32, 5);
    w.bits((hdist - 1) as u32, 5);
    w.bits((hclen - 4) as u32, 4);
    for &slot in CLEN_ORDER.iter().take(hclen) {
        w.bits(u32::from(cl_lens[slot]), 3);
    }
    let cl_codes = canonical(&cl_lens);
    for (s, extra, nb) in syms {
        let (c, l) = cl_codes[s as usize];
        w.code(c, l);
        w.bits(extra, nb);
    }
    let lit_codes = canonical(&lit_lens);
    let dist_codes = canonical(&dist_lens);
    write_tokens(w, toks, &lit_codes, &dist_codes);
}

/// Encode `data` as raw DEFLATE using a random mix of block types. Returns
/// the stream. `toks` per block are generated from the data by the caller.
fn encode_blocks(rng: &mut Rng, blocks: &[(Vec<Tok>, Vec<u8>)]) -> Vec<u8> {
    let mut w = BitWriter::new();
    let (fl, fd) = fixed_lengths();
    let (flc, fdc) = (canonical(&fl), canonical(&fd));
    let mut written: Vec<u8> = Vec::new();
    for (i, (toks, _)) in blocks.iter().enumerate() {
        let last = i + 1 == blocks.len();
        let kind = rng.below(3);
        // a block's matches refer back into earlier blocks, so the token
        // streams were generated against the cumulative output.
        let _ = &mut written;
        match kind {
            0 if toks.iter().all(|t| matches!(t, Tok::Lit(_))) => {
                let data: Vec<u8> = toks
                    .iter()
                    .map(|t| match t {
                        Tok::Lit(b) => *b,
                        _ => unreachable!(),
                    })
                    .collect();
                let mut chunks = data.chunks(65535).peekable();
                if data.is_empty() {
                    w.bits(u32::from(last), 1);
                    w.bits(0, 2);
                    w.align();
                    w.bits(0, 16);
                    w.bits(0xFFFF, 16);
                }
                while let Some(ch) = chunks.next() {
                    let fin = last && chunks.peek().is_none();
                    w.bits(u32::from(fin), 1);
                    w.bits(0, 2);
                    w.align();
                    w.bits(ch.len() as u32, 16);
                    w.bits(!(ch.len() as u32) & 0xFFFF, 16);
                    for &b in ch {
                        w.bits(u32::from(b), 8);
                    }
                }
            }
            1 => {
                w.bits(u32::from(last), 1);
                w.bits(1, 2);
                write_tokens(&mut w, toks, &flc, &fdc);
            }
            _ => dynamic_block(&mut w, rng, toks, last),
        }
    }
    w.finish()
}

/// A random multi-block stream and the bytes it must inflate to.
fn gen_stream(rng: &mut Rng) -> (Vec<u8>, Vec<u8>) {
    let nblocks = rng.range(1, 4);
    let mut blocks = Vec::new();
    let mut expect: Vec<u8> = Vec::new();
    for _ in 0..nblocks {
        let target = match rng.below(6) {
            0 => 0,
            1 => rng.range(1, 8),
            2 => rng.range(1000, 20_000),
            _ => rng.range(1, 600),
        };
        // Generate against what is already there so matches may reach back.
        let mut toks = Vec::new();
        let mut produced = 0usize;
        let start = expect.len();
        let alphabet = *[2u64, 4, 16, 256].get(rng.below(4) as usize).unwrap();
        while produced < target {
            let have = expect.len();
            if have == 0 || rng.chance(55) {
                let b = rng.below(alphabet) as u8;
                toks.push(Tok::Lit(b));
                expect.push(b);
                produced += 1;
            } else {
                let maxd = have.min(32768);
                let dist = if rng.chance(30) {
                    1
                } else {
                    rng.range(1, maxd)
                };
                let len = if rng.chance(20) {
                    258
                } else {
                    rng.range(3, 258)
                };
                toks.push(Tok::Match { len, dist });
                for _ in 0..len {
                    let b = expect[expect.len() - dist];
                    expect.push(b);
                }
                produced += len;
            }
        }
        let _ = start;
        blocks.push((toks, Vec::new()));
    }
    let _ = gen_tokens; // kept for ad hoc experiments
    (encode_blocks(rng, &blocks), expect)
}

fn zlib_wrap(raw: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut v = vec![0x78, 0x9C];
    v.extend_from_slice(raw);
    v.extend_from_slice(&adler32(plain).to_be_bytes());
    v
}

/// A valid zlib header for the given CMF/FLG high bits, FCHECK fixed up.
fn zlib_header(cmf: u8, flg_hi: u8) -> [u8; 2] {
    let base = (u16::from(cmf) << 8) | u16::from(flg_hi & 0xE0);
    let fcheck = (31 - base % 31) % 31;
    [cmf, (flg_hi & 0xE0) | fcheck as u8]
}

// --- properties --------------------------------------------------------------

/// Whatever the bytes, inflate under `cap` must not panic, must stay in time,
/// and must never exceed the cap.
fn check_any(bytes: &[u8]) {
    let started = Instant::now();
    for cap in [0usize, 1, 100, 1 << 16] {
        if let Ok(out) = inflate_zlib_capped(bytes, cap) {
            assert!(out.len() <= cap, "output {} past cap {cap}", out.len());
        }
    }
    if let Ok(out) = inflate_raw(bytes) {
        assert!(out.len() <= eve_inflate::MAX_OUTPUT);
    }
    let _ = looks_like_zlib(bytes);
    assert!(
        started.elapsed() < CASE_TIME,
        "took {:?}",
        started.elapsed()
    );
}

// --- targets -----------------------------------------------------------------

fn run_roundtrip(seed: u64, cases: u64) {
    let mut rng = Rng(seed);
    for case in 0..cases {
        let (raw, expect) = gen_stream(&mut rng);
        let z = zlib_wrap(&raw, &expect);
        guarded(seed, case, "round trip", &z, |z| {
            let got = inflate_zlib(z).unwrap_or_else(|e| panic!("own stream failed: {e}"));
            assert_eq!(got, expect, "inflate(encode(x)) != x");
            assert_eq!(inflate_raw(&raw).as_deref(), Ok(&expect[..]));
            // the cap is exact: len passes, len-1 does not
            assert_eq!(
                inflate_zlib_capped(z, expect.len()).as_deref(),
                Ok(&expect[..])
            );
            if !expect.is_empty() {
                assert_eq!(
                    inflate_zlib_capped(z, expect.len() - 1),
                    Err(InflateError::OutputTooLarge)
                );
            }
            // every strict prefix is an error
            let step = (z.len() / 30).max(1);
            for cut in (0..z.len()).step_by(step) {
                assert!(inflate_zlib(&z[..cut]).is_err(), "prefix {cut} inflated");
            }
        });
    }
}

fn run_mutations(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0xFACE);
    for case in 0..cases {
        let (raw, expect) = gen_stream(&mut rng);
        let mut z = zlib_wrap(&raw, &expect);
        for _ in 0..rng.range(1, 4) {
            if z.is_empty() {
                break;
            }
            let i = rng.below(z.len() as u64) as usize;
            match rng.below(5) {
                0 => z[i] ^= 1 << rng.below(8),
                1 => z[i] = rng.byte(),
                2 => z.insert(i, rng.byte()),
                3 => {
                    z.remove(i);
                }
                _ => z.truncate(i),
            }
        }
        guarded(seed, case, "mutated stream", &z, check_any);
    }
}

fn run_random(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0xD00D);
    for case in 0..cases {
        let n = rng.range(0, 200);
        let body = rng.bytes(n);
        // Half with a valid zlib header so the deflate body is what is tested.
        let bytes = if rng.chance(70) {
            let mut v = zlib_header(0x78, 0x00).to_vec();
            v.extend_from_slice(&body);
            v
        } else {
            body
        };
        guarded(seed, case, "random", &bytes, check_any);
        // Raw-deflate reading of the same bytes, beyond the zlib gate.
        guarded(seed, case, "random raw", &bytes, |b| {
            let _ = inflate_raw(b);
        });
    }
}

#[test]
fn fuzz_roundtrip_every_block_type() {
    run_roundtrip(1, CASES / 4);
}

#[test]
fn fuzz_mutated_valid_streams() {
    run_mutations(2, CASES / 2);
}

#[test]
fn fuzz_random_deflate_bodies() {
    run_random(3, CASES);
}

/// Hand-built hostile streams: zip bombs and bad Huffman tables.
#[test]
fn fuzz_hostile_streams() {
    // A zip bomb: one fixed-Huffman block, a literal then match(258, dist 1)
    // repeated. 4 MiB of input per ~1 GiB of output would pass any cap check
    // that ran after the copy.
    let (fl, fd) = fixed_lengths();
    let (flc, fdc) = (canonical(&fl), canonical(&fd));
    let mut toks = vec![Tok::Lit(b'a')];
    for _ in 0..200_000 {
        toks.push(Tok::Match { len: 258, dist: 1 });
    }
    let mut w = BitWriter::new();
    w.bits(1, 1);
    w.bits(1, 2);
    write_tokens(&mut w, &toks, &flc, &fdc);
    let raw = w.finish();
    let mut z = zlib_header(0x78, 0).to_vec();
    z.extend_from_slice(&raw);
    z.extend_from_slice(&[0, 0, 0, 0]);
    let started = Instant::now();
    for cap in [0usize, 1, 257, 258, 1 << 20, 4 << 20] {
        match inflate_zlib_capped(&z, cap) {
            Err(InflateError::OutputTooLarge) => {}
            other => panic!("cap {cap}: {other:?}"),
        }
    }
    assert!(started.elapsed() < CASE_TIME);
    // The unbounded-direction cap still holds.
    assert!(
        inflate_raw(&raw).is_err() || inflate_raw(&raw).unwrap().len() <= eve_inflate::MAX_OUTPUT
    );

    // Stored blocks: 65535 per 5 bytes of header; many of them.
    let mut w = BitWriter::new();
    for i in 0..64 {
        w.bits(u32::from(i == 63), 1);
        w.bits(0, 2);
        w.align();
        w.bits(0xFFFF, 16);
        w.bits(0, 16);
        for _ in 0..0xFFFF {
            w.bits(0x55, 8);
        }
    }
    let raw = w.finish();
    let mut z = zlib_header(0x78, 0).to_vec();
    z.extend_from_slice(&raw);
    z.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(
        inflate_zlib_capped(&z, 1 << 20),
        Err(InflateError::OutputTooLarge)
    );

    // Bad Huffman tables, each as the first block after a zlib header.
    let mut tables: Vec<Vec<u8>> = Vec::new();
    // Dynamic header with every (hlit, hdist, hclen) extreme and random lengths.
    let mut rng = Rng(9);
    for hlit in [0u32, 1, 28, 29, 30, 31] {
        for hdist in [0u32, 1, 29, 30, 31] {
            for hclen in [0u32, 5, 15] {
                for _ in 0..40 {
                    let mut w = BitWriter::new();
                    w.bits(1, 1);
                    w.bits(2, 2);
                    w.bits(hlit, 5);
                    w.bits(hdist, 5);
                    w.bits(hclen, 4);
                    for _ in 0..(hclen + 4) {
                        w.bits(rng.below(8) as u32, 3);
                    }
                    for _ in 0..rng.range(0, 60) {
                        w.bits(rng.next_u64() as u32, 16);
                    }
                    tables.push(w.finish());
                }
            }
        }
    }
    // Over-subscribed code-length code: five codes of length 1.
    {
        let mut w = BitWriter::new();
        w.bits(1, 1);
        w.bits(2, 2);
        w.bits(0, 5);
        w.bits(0, 5);
        w.bits(15, 4);
        for _ in 0..19 {
            w.bits(1, 3);
        }
        tables.push(w.finish());
    }
    // Repeat-previous (16) as the very first length.
    {
        let mut w = BitWriter::new();
        w.bits(1, 1);
        w.bits(2, 2);
        w.bits(0, 5);
        w.bits(0, 5);
        w.bits(0, 4);
        // cl lengths in order 16,17,18,0: give 16 and 0 length 1
        w.bits(1, 3);
        w.bits(0, 3);
        w.bits(0, 3);
        w.bits(1, 3);
        // symbol order canonical: 0 -> '0', 16 -> '1'
        w.code(1, 1);
        w.bits(0, 2);
        tables.push(w.finish());
    }
    // Reserved block type, stored LEN/NLEN mismatch, empty input.
    tables.push(vec![0b111]);
    tables.push(vec![0b001, 5, 0, 0, 0]);
    tables.push(vec![]);
    // Fixed block using the unassigned literal codes 286/287 and distances 30/31.
    for sym in [286usize, 287] {
        let mut w = BitWriter::new();
        w.bits(1, 1);
        w.bits(1, 2);
        let (c, l) = flc[sym];
        w.code(c, l);
        w.bits(0, 16);
        tables.push(w.finish());
    }
    for dsym in [30u32, 31] {
        let mut w = BitWriter::new();
        w.bits(1, 1);
        w.bits(1, 2);
        let (c, l) = flc[b'a' as usize];
        w.code(c, l);
        let (c, l) = flc[257];
        w.code(c, l);
        w.code(dsym, 5);
        w.bits(0, 16);
        tables.push(w.finish());
    }
    for (i, body) in tables.iter().enumerate() {
        let mut z = zlib_header(0x78, 0).to_vec();
        z.extend_from_slice(body);
        z.extend_from_slice(&[0, 0, 0, 1]);
        guarded(9, i as u64, "bad table", &z, check_any);
        guarded(9, i as u64, "bad table raw", body, |b| {
            let _ = inflate_raw(b);
        });
    }
    // Every header the zlib gate looks at.
    for cmf in 0..=255u8 {
        for flg in 0..=255u8 {
            let b = [cmf, flg, 3, 0, 0, 0, 0, 1];
            guarded(9, cmf as u64 * 256 + flg as u64, "header", &b, check_any);
        }
    }
}

#[test]
#[ignore = "long run: EVE_FUZZ_SEED, EVE_FUZZ_CASES"]
fn long_inflate() {
    let (seed, cases) = env_run();
    run_roundtrip(seed, cases / 4);
    run_mutations(seed, cases);
    run_random(seed, cases);
}
