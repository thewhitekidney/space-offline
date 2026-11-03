//! Q-FUZZ: seeded fuzzing of the server's inbound parse path.
//!
//! Std only (SplitMix64). Two targets here, the ones that need the server
//! crate; the marshal decoder, the inflater, the framing and the packet
//! parsers have their own `tests/fuzz.rs` in `eve-marshal`, `eve-inflate`
//! and `eve-proto`.
//!
//! - **`wire::loads`**, the one function every client payload goes through:
//!   inflate (capped), header, id table, budgeted decode, trailing-byte check.
//!   Random bytes, valid payloads mutated, zlib-wrapped payloads (the stored
//!   -block form, so the wrapper is valid and the body is what varies).
//! - **the login handshake** (`Handshake::on_payload`): sequences of payloads
//!   in every phase, built from the correct payload for that phase with
//!   random fields replaced, truncated or retyped, plus fully random shapes.
//!   The phase must only move forward, `Closed` is terminal, and nothing
//!   panics.
//! - **`pyutil`**, the shared argument readers (`dict_get`, `pairs_get`,
//!   `as_bytes`, `opt_id`) on random values. The per-service `int_arg` /
//!   `text_arg` helpers are private to their files and are reached by
//!   `tests/arm_fuzz.rs`, which calls every dispatch arm with wrong-typed
//!   arguments; its `KNOWN_PANICS` is empty.
//!
//! Long mode: `EVE_FUZZ_SEED`, `EVE_FUZZ_CASES`, `--ignored`.

use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use eve_marshal::PyValue;
use eve_marshal::value::DECODE_STACK;
use eve_server::handshake::CHALLENGE_LEN;
use eve_server::pyutil;
use eve_server::{Handshake, Phase, crypto_hash, dumps, loads};

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
    for x in b.iter().take(768) {
        s.push_str(&format!("{x:02x}"));
    }
    if b.len() > 768 {
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
        .unwrap_or(500_000);
    eprintln!("q-fuzz: seed={seed} cases={cases}");
    (seed, cases)
}

fn big_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(2 * DECODE_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|e| panic::resume_unwind(e));
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

fn gen_scalar(rng: &mut Rng) -> PyValue {
    match rng.below(9) {
        0 => PyValue::None,
        1 => PyValue::Int(
            *[0i64, 1, -1, 24, 3619879, i64::MAX, i64::MIN]
                .get(rng.below(7) as usize)
                .unwrap(),
        ),
        2 => PyValue::Int(rng.next_u64() as i64),
        3 => {
            let n = rng.range(0, 40);
            PyValue::Bytes(rng.bytes(n))
        }
        4 => PyValue::Unicode("placebo".into()),
        5 => PyValue::Bool(rng.chance(50)),
        6 => PyValue::Real(
            *[0.0, 24.01, f64::NAN, f64::INFINITY]
                .get(rng.below(4) as usize)
                .unwrap(),
        ),
        7 => PyValue::Bytes(b"VK".to_vec()),
        _ => PyValue::Bytes(b"QC".to_vec()),
    }
}

fn gen_shape(rng: &mut Rng, depth: u32) -> PyValue {
    if depth == 0 || rng.chance(40) {
        return gen_scalar(rng);
    }
    match rng.below(4) {
        0 => PyValue::Tuple(
            (0..rng.range(0, 8))
                .map(|_| gen_shape(rng, depth - 1))
                .collect(),
        ),
        1 => PyValue::List(
            (0..rng.range(0, 4))
                .map(|_| gen_shape(rng, depth - 1))
                .collect(),
        ),
        2 => PyValue::Dict(
            (0..rng.range(0, 4))
                .map(|_| (gen_shape(rng, depth - 1), gen_scalar(rng)))
                .collect(),
        ),
        _ => PyValue::Instance {
            class: b"utillib.KeyVal".to_vec(),
            state: Box::new(gen_shape(rng, depth - 1)),
        },
    }
}

// --- wire::loads -------------------------------------------------------------

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A valid zlib wrapper made of stored blocks.
fn zlib_stored(plain: &[u8]) -> Vec<u8> {
    let mut z = vec![0x78, 0x01];
    let mut chunks = plain.chunks(65535).peekable();
    if plain.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        z.push(u8::from(chunks.peek().is_none()));
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&adler32(plain).to_be_bytes());
    z
}

fn check_loads(bytes: &[u8]) {
    let started = Instant::now();
    if let Ok(v) = loads(bytes) {
        // anything it accepts it can write and read back the same
        let again = loads(&dumps(&v)).expect("re-encode of an accepted payload");
        assert_eq!(format!("{v:?}"), format!("{again:?}"));
    }
    assert!(
        started.elapsed() < CASE_TIME,
        "took {:?}",
        started.elapsed()
    );
}

fn mutate(rng: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..rng.range(1, 4) {
        if b.is_empty() {
            break;
        }
        let i = rng.below(b.len() as u64) as usize;
        match rng.below(6) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => b[i] = rng.byte(),
            2 => b.insert(i, rng.byte()),
            3 => {
                b.remove(i);
            }
            4 => b.truncate(i),
            _ => {
                let end = (i + 5).min(b.len());
                b.splice(i..end, [0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
            }
        }
    }
    b
}

fn run_loads(seed: u64, cases: u64) {
    let mut rng = Rng(seed);
    for case in 0..cases {
        let v = gen_shape(&mut rng, 4);
        let plain = dumps(&v);
        let bytes = match rng.below(5) {
            0 => {
                let n = rng.range(0, 80);
                rng.bytes(n)
            }
            1 => mutate(&mut rng, &plain),
            2 => zlib_stored(&plain),
            3 => mutate(&mut rng, &zlib_stored(&plain)),
            _ => plain,
        };
        guarded(seed, case, "wire::loads", &bytes, check_loads);
    }
}

#[test]
fn fuzz_wire_loads() {
    big_stack(|| run_loads(1, CASES));
}

// --- the handshake -----------------------------------------------------------

fn phase_rank(p: Phase) -> u8 {
    match p {
        Phase::Greeting => 0,
        Phase::AwaitVersion => 1,
        Phase::AwaitUserKey => 2,
        Phase::AwaitCryptoPack => 3,
        Phase::AwaitAuthRequest => 4,
        Phase::AwaitAuthResponse => 5,
        Phase::Authenticated => 6,
        Phase::Closed => 7,
    }
}

/// Replace one field of a payload with something else, or cut it short.
fn corrupt(rng: &mut Rng, v: PyValue) -> PyValue {
    match v {
        PyValue::Tuple(mut t) if !t.is_empty() => {
            let i = rng.below(t.len() as u64) as usize;
            match rng.below(4) {
                0 => t[i] = gen_shape(rng, 2),
                1 => t.truncate(i),
                2 => t.push(gen_scalar(rng)),
                _ => t[i] = corrupt(rng, t[i].clone()),
            }
            PyValue::Tuple(t)
        }
        PyValue::Dict(mut d) if !d.is_empty() => {
            let i = rng.below(d.len() as u64) as usize;
            if rng.chance(50) {
                d[i].0 = gen_shape(rng, 2);
            } else {
                d[i].1 = gen_shape(rng, 1);
            }
            PyValue::Dict(d)
        }
        _ => gen_shape(rng, 2),
    }
}

fn run_handshake(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0x4841);
    for case in 0..cases {
        let server_challenge = [rng.byte(); CHALLENGE_LEN];
        let mut hs = Handshake::new(Default::default(), server_challenge);
        let greeting = hs.greeting();
        let PyValue::Tuple(g) = &greeting else {
            panic!("greeting is not a tuple");
        };
        let client_challenge = rng.bytes(CHALLENGE_LEN);
        // the correct payload for each phase, in order
        let good: Vec<PyValue> = vec![
            PyValue::Tuple(g[..6].to_vec()),
            PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Bytes(b"VK".to_vec()),
                PyValue::Bytes(vec![1; 16]),
            ]),
            PyValue::Tuple(vec![PyValue::Bytes(b"placebo".to_vec())]),
            PyValue::Tuple(vec![
                PyValue::Bytes(client_challenge),
                PyValue::Dict(vec![
                    (
                        PyValue::Bytes(b"someone".to_vec()),
                        PyValue::Bytes(b"user_name".to_vec()),
                    ),
                    (
                        PyValue::Bytes(b"tok".to_vec()),
                        PyValue::Bytes(b"user_sso_token".to_vec()),
                    ),
                    (PyValue::Int(1), PyValue::Bytes(b"macho_version".to_vec())),
                ]),
            ]),
            PyValue::Tuple(vec![PyValue::Bytes(
                crypto_hash(&[PyValue::Bytes(server_challenge.to_vec())]).into_bytes(),
            )]),
        ];
        let mut trace = format!("{greeting:?}");
        let mut rank = phase_rank(hs.phase());
        for step in 0..8 {
            let phase_idx = step.min(good.len() - 1);
            let payload = match rng.below(10) {
                0..=3 => good[phase_idx].clone(),
                4..=7 => corrupt(&mut rng, good[phase_idx].clone()),
                _ => gen_shape(&mut rng, 3),
            };
            trace.push_str(&format!("\n{payload:?}"));
            let before = hs.phase();
            let r = panic::catch_unwind(AssertUnwindSafe(|| hs.on_payload(&payload)));
            let out = match r {
                Ok(o) => o,
                Err(_) => panic!(
                    "q-fuzz FAILURE (handshake) seed={seed} case={case}: panic in {before:?}\n{trace}"
                ),
            };
            let now = phase_rank(hs.phase());
            assert!(
                now >= rank,
                "seed={seed} case={case}: phase went back from {before:?} to {:?}\n{trace}",
                hs.phase()
            );
            if before == Phase::Closed {
                assert!(out.is_err(), "Closed accepted a payload\n{trace}");
                assert_eq!(hs.phase(), Phase::Closed);
            }
            rank = now;
            if let Ok(o) = &out {
                assert!(o.replies.len() <= 2);
            }
        }
    }
}

#[test]
fn fuzz_handshake_sequences() {
    big_stack(|| run_handshake(2, CASES));
}

// --- pyutil ------------------------------------------------------------------

fn run_pyutil(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0x5959);
    for case in 0..cases {
        let v = gen_shape(&mut rng, 4);
        let klen = rng.range(0, 3);
        let key: Vec<u8> = rng.bytes(klen);
        let dbg = format!("{v:?}");
        guarded(seed, case, "pyutil", dbg.as_bytes(), |_| {
            let _ = pyutil::dict_get(&v, &key);
            let _ = pyutil::dict_get(&v, b"user_name");
            let _ = pyutil::as_bytes(&v);
            let _ = pyutil::opt_id(Some(&v));
            let _ = pyutil::opt_id(None);
            if let PyValue::Dict(e) = &v {
                let _ = pyutil::pairs_get(e, &key);
            }
            if let PyValue::Tuple(t) | PyValue::List(t) = &v {
                for x in t {
                    let _ = pyutil::as_bytes(x);
                    let _ = pyutil::opt_id(Some(x));
                }
            }
        });
    }
}

#[test]
fn fuzz_pyutil_readers() {
    big_stack(|| run_pyutil(3, CASES));
}

#[test]
#[ignore = "long run: EVE_FUZZ_SEED, EVE_FUZZ_CASES"]
fn long_server_input() {
    let (seed, cases) = env_run();
    big_stack(move || {
        run_loads(seed, cases);
        run_handshake(seed, cases);
        run_pyutil(seed, cases);
    });
}
