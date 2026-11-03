//! Q-FUZZ: seeded, deterministic fuzzing of the marshal decoder.
//!
//! Std only: a SplitMix64 PRNG, no crates, no cargo-fuzz. Every client byte is
//! hostile input, so the decoder has to hold five properties for any bytes:
//!
//! 1. **never panics**;
//! 2. **terminates**, in time proportional to the input (a wall-clock bound
//!    per case catches a quadratic or runaway loop);
//! 3. **allocates in proportion to the input**, never to a declared length:
//!    a stream with no shared objects builds at most one node per input byte,
//!    and owns at most as many heap bytes as it was given;
//! 4. **never recurses past the client's own depth limit** (1000): the
//!    height of every value it returns is bounded, shared back-references
//!    included;
//! 5. **round-trips**: `decode(encode(x)) == x` for generated values, and a
//!    decoded value re-encodes to a stream that decodes to the same value.
//!
//! Two modes. The plain `#[test]`s run a fixed budget ([`CASES`] per target)
//! in the normal suite. The `#[ignore]`d `long_*` tests are for overnight
//! runs and read their seed and case count from the environment:
//!
//! ```text
//! EVE_FUZZ_SEED=7 EVE_FUZZ_CASES=5000000 \
//!   tools/dev.sh cargo test --locked -p eve-marshal --test fuzz --release -- --ignored --nocapture
//! ```
//!
//! A failure prints the seed, the case number and the input as hex. Paste the
//! hex into a regression test in `tests/regress.rs`.

use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use eve_marshal::tag::code as c;
use eve_marshal::value::{Budget, DECODE_STACK};
use eve_marshal::{
    DbRow, DbRowDescriptor, DbType, DbValue, Decoder, ObjectForm, PyValue, Reader, Writer, save,
    take_stream,
};

/// Cases per target in the normal test run.
const CASES: u64 = 20_000;
/// The client's recursion ceiling, which the decoder mirrors.
const MAX_DEPTH: usize = 1000;
/// No single case may take longer than this. Generous: the slowest honest
/// case is milliseconds, in a debug build.
const CASE_TIME: Duration = Duration::from_secs(5);

// --- the PRNG --------------------------------------------------------------

/// SplitMix64.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (`n > 0`).
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
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b.iter().take(768) {
        s.push_str(&format!("{x:02x}"));
    }
    if b.len() > 768 {
        s.push_str(&format!("...({} bytes)", b.len()));
    }
    s
}

/// Seed and case count for a long run.
fn env_run() -> (u64, u64) {
    let seed = std::env::var("EVE_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let cases = std::env::var("EVE_FUZZ_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    eprintln!("q-fuzz: seed={seed} cases={cases}");
    (seed, cases)
}

/// Run on a thread with the stack the server gives a connection thread, so a
/// deep-but-legal value does not overflow the test harness's smaller one.
fn big_stack<F: FnOnce() + Send + 'static>(f: F) {
    std::thread::Builder::new()
        .stack_size(2 * DECODE_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|e| panic::resume_unwind(e));
}

// --- value measurements ----------------------------------------------------

/// Height of the tree, iteratively (the thing under test must not be what
/// measures itself with unbounded recursion).
fn stats(v: &PyValue) -> (usize, usize, usize) {
    // (height, nodes, heap bytes owned, DbRow blob bytes folded in)
    let mut height = 0;
    let mut nodes = 0;
    let mut heap = 0;
    let mut stack: Vec<(&PyValue, usize)> = vec![(v, 1)];
    while let Some((v, d)) = stack.pop() {
        height = height.max(d);
        nodes += 1;
        match v {
            PyValue::Bytes(b) | PyValue::Global(b) | PyValue::SubStream(b) => heap += b.len(),
            PyValue::Unicode(s) => heap += s.len(),
            PyValue::Tuple(xs) | PyValue::List(xs) => {
                stack.extend(xs.iter().map(|x| (x, d + 1)));
            }
            PyValue::Dict(es) => {
                for (a, b) in es {
                    stack.push((a, d + 1));
                    stack.push((b, d + 1));
                }
            }
            PyValue::Callback(x) => stack.push((x, d + 1)),
            PyValue::Instance { class, state } => {
                heap += class.len();
                stack.push((state, d + 1));
            }
            PyValue::ObjectEx {
                header, list, dict, ..
            } => {
                stack.push((header, d + 1));
                stack.extend(list.iter().map(|x| (x, d + 1)));
                for (a, b) in dict {
                    stack.push((a, d + 1));
                    stack.push((b, d + 1));
                }
            }
            PyValue::DbRow(row) => {
                heap += row.descriptor().blob_size();
                for val in row.values() {
                    if let DbValue::Object(o) = val {
                        stack.push((o, d + 1));
                    }
                }
            }
            _ => {}
        }
    }
    (height, nodes, heap)
}

// --- the decode property ---------------------------------------------------

/// Decode `bytes` as a whole stream the way the server does (header, id table,
/// budget) and check every property that holds for arbitrary input.
fn check_stream(bytes: &[u8]) {
    let started = Instant::now();
    let Ok(stream) = take_stream(bytes) else {
        return;
    };
    let mapcount = stream.header.mapcount;
    let mut r = Reader::new(stream.body);
    let mut d = Decoder::with_shared(stream.shared_ids.clone()).with_budget(Budget::INBOUND);
    let res = d.load(&mut r);
    assert!(r.pos() <= stream.body.len(), "reader ran past the body");
    assert!(
        started.elapsed() < CASE_TIME,
        "decode took {:?}",
        started.elapsed()
    );
    let Ok(v) = res else { return };

    let spent = d.spent();
    assert!(
        spent.nodes <= Budget::INBOUND.nodes && spent.bytes <= Budget::INBOUND.bytes,
        "decode overspent its budget: {spent:?}"
    );
    let (height, nodes, heap) = stats(&v);
    assert!(
        height <= MAX_DEPTH,
        "decoded a value {height} deep, past the client's {MAX_DEPTH}"
    );
    assert!(nodes <= Budget::INBOUND.nodes);
    if mapcount == 0 {
        // No shared objects: no clones, so every node cost its own tag byte.
        assert!(
            nodes <= stream.body.len(),
            "{nodes} nodes from {} bytes",
            stream.body.len()
        );
        // A DbRow's blob is RLE: one byte can stand for 16 zero bytes.
        assert!(
            heap <= 16 * stream.body.len() + 64,
            "{heap} heap bytes from {} bytes",
            stream.body.len()
        );
    }

    // A decoded value re-encodes to a stream that decodes to the same value.
    let mut w = Writer::new();
    save(&v, &mut w);
    let enc = w.into_bytes();
    let again = Decoder::new()
        .load(&mut Reader::new(&enc))
        .unwrap_or_else(|e| panic!("re-encode of a decoded value failed: {e}"));
    assert_eq!(
        format!("{v:?}"),
        format!("{again:?}"),
        "re-encoding changed the value"
    );
}

/// Run `f` on `bytes`; on a panic, name the seed, the case and the input.
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

// --- generators ------------------------------------------------------------

/// Every valid tag byte, shared flag clear.
const VALID_TAGS: [u8; 36] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x10, 0x11,
    0x13, 0x14, 0x15, 0x16, 0x17, 0x19, 0x1A, 0x1B, 0x1C, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x26,
    0x27, 0x28, 0x29, 0x2A,
];

/// A byte that is usually a tag, sometimes a length escape, sometimes noise.
fn tagish(rng: &mut Rng) -> u8 {
    match rng.below(10) {
        0..=5 => {
            let t = *rng.pick(&VALID_TAGS);
            if rng.chance(25) { t | 0x40 } else { t }
        }
        6 => 0xFF,
        7 => *rng.pick(&[0x00, 0x01, 0x02, 0x7F, 0x80, 0xFE]),
        8 => 0x2B,
        _ => rng.byte(),
    }
}

/// A header plus a body of tag-biased bytes, with a plausible id table.
fn random_stream(rng: &mut Rng) -> Vec<u8> {
    let max = if rng.chance(10) { 600 } else { 48 };
    let body_len = rng.range(0, max);
    let mapcount = if rng.chance(40) { rng.range(0, 6) } else { 0 };
    let mut out = vec![0x7E];
    out.extend_from_slice(&(mapcount as i32).to_le_bytes());
    for _ in 0..body_len {
        out.push(tagish(rng));
    }
    for _ in 0..mapcount {
        let id = if rng.chance(85) {
            rng.range(1, mapcount.max(1)) as u32
        } else {
            rng.next_u64() as u32
        };
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

fn gen_string(rng: &mut Rng) -> Vec<u8> {
    let n = match rng.below(20) {
        0 => 0,
        1 => 1,
        2 => rng.range(250, 260),
        3 => rng.range(65_530, 65_540),
        _ => rng.range(0, 24),
    };
    rng.bytes(n)
}

fn gen_text(rng: &mut Rng) -> String {
    const CH: [char; 9] = [
        'a',
        'Z',
        '0',
        ' ',
        '\u{e9}',
        '\u{20ac}',
        '\u{3042}',
        '\u{1F680}',
        '\0',
    ];
    let n = rng.range(0, 12);
    (0..n).map(|_| *rng.pick(&CH)).collect()
}

fn gen_int(rng: &mut Rng) -> i64 {
    match rng.below(8) {
        0 => *rng.pick(&[
            -1,
            0,
            1,
            i8::MIN as i64,
            i8::MAX as i64,
            127,
            128,
            -128,
            -129,
        ]),
        1 => *rng.pick(&[i16::MIN as i64, i16::MAX as i64, 32768, -32769]),
        2 => *rng.pick(&[i32::MIN as i64, i32::MAX as i64, 1 << 31, -(1 << 31) - 1]),
        3 => *rng.pick(&[i64::MIN, i64::MAX, i64::MIN + 1]),
        4 => rng.next_u64() as i8 as i64,
        5 => rng.next_u64() as i16 as i64,
        6 => rng.next_u64() as i32 as i64,
        _ => rng.next_u64() as i64,
    }
}

fn gen_real(rng: &mut Rng) -> f64 {
    match rng.below(6) {
        0 => 0.0,
        1 => -0.0,
        2 => f64::INFINITY,
        3 => f64::NEG_INFINITY,
        4 => f64::MIN_POSITIVE,
        _ => loop {
            let f = f64::from_bits(rng.next_u64());
            if !f.is_nan() {
                return f;
            }
        },
    }
}

fn gen_dbrow(rng: &mut Rng) -> Option<DbRow> {
    const TYPES: [DbType; 12] = [
        DbType::Bool,
        DbType::I1,
        DbType::Ui1,
        DbType::I2,
        DbType::Ui2,
        DbType::I4,
        DbType::Ui4,
        DbType::I8,
        DbType::R4,
        DbType::R5,
        DbType::Bytes,
        DbType::WStr,
    ];
    let n = rng.range(1, 8);
    let cols: Vec<(String, DbType)> = (0..n)
        .map(|i| (format!("c{i}"), *rng.pick(&TYPES)))
        .collect();
    let pairs: Vec<(&str, DbType)> = cols.iter().map(|(s, t)| (s.as_str(), *t)).collect();
    let d = DbRowDescriptor::from_pairs(&pairs).ok()?;
    let values: Vec<DbValue> = cols
        .iter()
        .map(|(_, t)| {
            if rng.chance(15) {
                return DbValue::Null;
            }
            match t {
                DbType::Bool => DbValue::Bool(rng.chance(50)),
                DbType::I1 => DbValue::Int(rng.next_u64() as i8 as i64),
                DbType::Ui1 => DbValue::Int(rng.next_u64() as u8 as i64),
                DbType::I2 => DbValue::Int(rng.next_u64() as i16 as i64),
                DbType::Ui2 => DbValue::Int(rng.next_u64() as u16 as i64),
                DbType::I4 | DbType::Ui4 => DbValue::Int(rng.next_u64() as i32 as i64),
                DbType::I8 => DbValue::Int(rng.next_u64() as i64),
                // f32-exact so the R4 round trip is exact.
                DbType::R4 => DbValue::Real(f64::from(rng.range(0, 1 << 20) as f32)),
                DbType::R5 => DbValue::Real(gen_real(rng)),
                _ => DbValue::Object(if rng.chance(50) {
                    PyValue::Bytes(gen_string(rng))
                } else {
                    PyValue::Unicode(gen_text(rng))
                }),
            }
        })
        .collect();
    DbRow::new(d, values).ok()
}

/// A random value the encoder can write and the decoder must read back equal.
fn gen_value(rng: &mut Rng, depth: u32) -> PyValue {
    let leaf = depth == 0 || rng.chance(35);
    if leaf {
        return match rng.below(9) {
            0 => PyValue::None,
            1 => PyValue::Bool(rng.chance(50)),
            2 | 3 => PyValue::Int(gen_int(rng)),
            4 => PyValue::Real(gen_real(rng)),
            5 => PyValue::Bytes(gen_string(rng)),
            6 => PyValue::Unicode(gen_text(rng)),
            7 => PyValue::Global(gen_string(rng)),
            _ => PyValue::SubStream(gen_string(rng)),
        };
    }
    let kids = |rng: &mut Rng, n: usize| -> Vec<PyValue> {
        (0..n).map(|_| gen_value(rng, depth - 1)).collect()
    };
    match rng.below(9) {
        0 => {
            // A wide container (past the one-byte length) only over leaves.
            let n = *rng.pick(&[0, 1, 2, 3, 5, 300]);
            let n = if n == 300 && depth > 1 {
                5
            } else if n == 300 {
                260
            } else {
                n
            };
            PyValue::Tuple(kids(rng, n))
        }
        1 => {
            let n = *rng.pick(&[0, 1, 2, 4, 300]);
            let n = if n == 300 && depth > 1 {
                4
            } else if n == 300 {
                260
            } else {
                n
            };
            PyValue::List(kids(rng, n))
        }
        2 => {
            let n = rng.range(0, 4);
            PyValue::Dict(
                (0..n)
                    .map(|_| (gen_value(rng, depth - 1), gen_value(rng, depth - 1)))
                    .collect(),
            )
        }
        3 => PyValue::Callback(Box::new(gen_value(rng, depth - 1))),
        4 => PyValue::Instance {
            class: {
                let mut s = gen_string(rng);
                s.extend_from_slice(b"x");
                s
            },
            state: Box::new(gen_value(rng, depth - 1)),
        },
        5 => PyValue::ObjectEx {
            form: if rng.chance(50) {
                ObjectForm::Call
            } else {
                ObjectForm::New
            },
            header: Box::new(gen_value(rng, depth - 1)),
            list: {
                let n = rng.range(0, 3);
                kids(rng, n)
            },
            dict: (0..rng.range(0, 3))
                .map(|_| (gen_value(rng, depth - 1), gen_value(rng, depth - 1)))
                .collect(),
        },
        6 => gen_dbrow(rng).map_or(PyValue::None, |r| PyValue::DbRow(Box::new(r))),
        _ => PyValue::Tuple(kids(rng, 2)),
    }
}

fn encode(v: &PyValue) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(0x7E);
    w.i32(0);
    save(v, &mut w);
    w.into_bytes()
}

fn mutate(rng: &mut Rng, src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..rng.range(1, 4) {
        if b.is_empty() {
            b.push(rng.byte());
            continue;
        }
        let i = rng.below(b.len() as u64) as usize;
        match rng.below(8) {
            0 => b[i] ^= 1 << rng.below(8),
            1 => b[i] = rng.byte(),
            2 => b[i] = tagish(rng),
            3 => b.insert(i, tagish(rng)),
            4 => {
                b.remove(i);
            }
            5 => b.truncate(i),
            6 => {
                // Plant a huge declared length where a length may be.
                let huge: [u8; 5] = *rng.pick(&[
                    [0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
                    [0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
                    [0xFF, 0x00, 0x00, 0x00, 0x80],
                    [0xFF, 0x00, 0x00, 0x10, 0x00],
                    [0xFF, 0xFF, 0xFF, 0xFF, 0x00],
                ]);
                let end = (i + huge.len()).min(b.len());
                b.splice(i..end, huge.iter().copied());
            }
            _ => {
                // Duplicate a slice somewhere else.
                let j = (i + rng.range(1, 16)).min(b.len());
                let piece = b[i..j].to_vec();
                let at = rng.below(b.len() as u64) as usize;
                b.splice(at..at, piece);
            }
        }
    }
    b
}

// --- targets ---------------------------------------------------------------

fn run_random_streams(seed: u64, cases: u64) {
    let mut rng = Rng(seed);
    for case in 0..cases {
        let bytes = random_stream(&mut rng);
        guarded(seed, case, "random stream", &bytes, check_stream);
    }
}

fn run_raw_bytes(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0x5151);
    for case in 0..cases {
        let n = rng.range(0, 64);
        let mut bytes = rng.bytes(n);
        // Half the time make the header valid so the body is reached.
        if rng.chance(50) {
            let mut h = vec![0x7E, 0, 0, 0, 0];
            h.append(&mut bytes);
            bytes = h;
        }
        guarded(seed, case, "raw bytes", &bytes, check_stream);
    }
}

fn run_roundtrip(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0xA0A0);
    for case in 0..cases {
        let depth = rng.range(0, 5) as u32;
        let v = gen_value(&mut rng, depth);
        let bytes = encode(&v);
        guarded(seed, case, "round trip", &bytes, |b| {
            let s = take_stream(b).expect("own header");
            let mut r = Reader::new(s.body);
            let got = Decoder::new()
                .with_budget(Budget::INBOUND)
                .load(&mut r)
                .unwrap_or_else(|e| panic!("own encoding did not decode: {e}"));
            assert_eq!(r.remaining(), 0, "own encoding left bytes");
            assert_eq!(got, v, "decode(encode(x)) != x");
            // Every strict prefix is an incomplete stream: Err, never a value.
            let step = (s.body.len() / 40).max(1);
            for cut in (0..s.body.len()).step_by(step) {
                let r = Decoder::new().load(&mut Reader::new(&s.body[..cut]));
                assert!(
                    r.is_err(),
                    "a {cut}-byte prefix of a {}-byte value decoded",
                    s.body.len()
                );
            }
        });
    }
}

fn run_mutations(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0xBEEF);
    for case in 0..cases {
        let depth = rng.range(1, 4) as u32;
        let v = gen_value(&mut rng, depth);
        let bytes = mutate(&mut rng, &encode(&v));
        guarded(seed, case, "mutated encoding", &bytes, check_stream);
    }
}

#[test]
fn fuzz_random_streams() {
    big_stack(|| run_random_streams(1, CASES));
}

#[test]
fn fuzz_raw_bytes() {
    big_stack(|| run_raw_bytes(2, CASES));
}

#[test]
fn fuzz_roundtrip_and_truncation() {
    big_stack(|| run_roundtrip(3, CASES / 2));
}

#[test]
fn fuzz_mutated_valid_encodings() {
    big_stack(|| run_mutations(4, CASES));
}

/// Deterministic hostile shapes that random bytes rarely reach.
#[test]
fn fuzz_hostile_shapes() {
    big_stack(hostile_shapes);
}

fn stream(body: &[u8]) -> Vec<u8> {
    let mut v = vec![0x7E, 0, 0, 0, 0];
    v.extend_from_slice(body);
    v
}

fn hostile_shapes() {
    let mut rng = Rng(5);
    // Deep nesting of every container tag, either side of the depth limit.
    let nests: [(&str, &[u8]); 6] = [
        ("tuple1", &[c::TUPLE1]),
        ("list1", &[c::LIST1]),
        ("callback", &[c::CALLBACK]),
        ("tuple2", &[c::TUPLE2]),
        ("tuple n=1", &[c::TUPLE, 1]),
        ("dict n=1", &[c::DICT, 1]),
    ];
    for (name, unit) in nests {
        for depth in [10usize, 998, 999, 1000, 1001, 1002, 5000, 200_000] {
            let mut body = Vec::new();
            for _ in 0..depth {
                body.extend_from_slice(unit);
                if unit[0] == c::TUPLE2 {
                    // the first child is a leaf; the second nests
                    body.push(c::NONE);
                }
                if unit[0] == c::DICT {
                    body.push(c::NONE);
                }
            }
            body.push(c::NONE);
            if unit[0] == c::DICT {
                // value-then-key: the NONE after each tag is the value
            }
            body.extend(std::iter::repeat_n(c::NONE, depth * 2));
            guarded(5, depth as u64, name, &stream(&body), check_stream);
        }
    }
    // ObjectEx / Instance nesting (no count byte, terminator based).
    for tag in [c::OBJECT_22, c::OBJECT_23, c::INSTANCE] {
        for depth in [999usize, 1001, 100_000] {
            let mut body = Vec::new();
            for _ in 0..depth {
                if tag == c::INSTANCE {
                    body.extend_from_slice(&[tag, c::STRING_CHAR, b'x']);
                } else {
                    body.push(tag);
                }
            }
            body.push(c::NONE);
            guarded(
                5,
                depth as u64,
                "wrapper nest",
                &stream(&body),
                check_stream,
            );
        }
    }
    // Huge declared sizes with no bytes behind them.
    let huge = [0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
    let neg = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
    for tag in [
        c::TUPLE,
        c::LIST,
        c::DICT,
        c::STRING,
        c::STRING_ALT,
        c::UNICODE_UTF8,
        c::UNICODE_UTF16,
        c::LONG,
        c::GLOBAL,
        c::SUBSTREAM,
        c::SHARED_REF,
        c::DBROW,
    ] {
        for size in [
            &huge[..],
            &neg[..],
            &[0xFF, 0, 0, 0, 0x40][..],
            &[0xFE][..],
            &[0xFF][..],
        ] {
            let mut body = vec![tag];
            body.extend_from_slice(size);
            guarded(5, tag as u64, "huge size", &stream(&body), check_stream);
        }
    }
    // A header that claims a huge shared table, and one that overlaps itself.
    for mc in [i32::MAX, i32::MIN, -1, 0x4000_0000, 1 << 24, 1 << 20] {
        let mut b = vec![0x7E];
        b.extend_from_slice(&mc.to_le_bytes());
        b.extend_from_slice(&[c::NONE; 16]);
        guarded(5, mc as u64, "mapcount", &b, check_stream);
    }
    // Truncated headers.
    for n in 0..8 {
        let b = vec![0x7D, 0, 1, 0, 0, 0, 0, 0][..n].to_vec();
        guarded(5, n as u64, "short header", &b, check_stream);
    }
    // Reference fan-out: one shared list, then a list of N references to it,
    // nested k deep. The budget has to stop it long before memory does.
    for (refs, depth) in [
        (1000usize, 1usize),
        (50_000, 1),
        (200, 3),
        (20, 12),
        (2, 30),
    ] {
        let mut body = Vec::new();
        // shared list (flag 0x40) of two Nones, registers id 1
        body.extend_from_slice(&[c::LIST | 0x40, 2, c::NONE, c::NONE]);
        for _ in 0..depth {
            // a list of `refs` references to the previous level; each level
            // registers too, so the next can reference it
            body.push(c::LIST | 0x40);
            body.push(0xFF);
            body.extend_from_slice(&(refs as i32).to_le_bytes());
            for _ in 0..refs {
                body.extend_from_slice(&[c::SHARED_REF, 1]);
            }
        }
        // the whole thing as a tuple holding the chain is not needed: the
        // decoder reads one root, the first list; the rest is trailing.
        let mut tup = vec![c::TUPLE, 0xFF];
        tup.extend_from_slice(&1i32.to_le_bytes());
        let mut s = vec![0x7E];
        s.extend_from_slice(&((depth + 1) as i32).to_le_bytes());
        s.extend_from_slice(&tup);
        s.extend_from_slice(&body[..]);
        for id in 1..=(depth + 1) as u32 {
            s.extend_from_slice(&id.to_le_bytes());
        }
        guarded(5, refs as u64, "ref fan-out", &s, check_stream);
    }
    // A reference chain: S1 = [], S2 = [ref S1], S3 = [ref S2], ... Sk. Each
    // clone is one level taller, so the height of the result grows with k
    // while the stream stays small: the depth limit has to hold across clones.
    for k in [10usize, 400, 900, 1100, 1300, 1500, 3000] {
        // The root tuple holds all k levels, so each is decoded (and so
        // registered) in order.
        let mut body = vec![c::TUPLE, 0xFF];
        body.extend_from_slice(&(k as i32).to_le_bytes());
        body.extend_from_slice(&[c::LIST | 0x40, 0]); // level 1: empty shared list, id 1
        // Level 2 is a LIST1 (one child); LIST1 does not register, so use LIST n=1.
        for i in 2..=k {
            body.extend_from_slice(&[c::LIST | 0x40, 1, c::SHARED_REF]);
            if i - 1 < 0xFF {
                body.push((i - 1) as u8);
            } else {
                body.push(0xFF);
                body.extend_from_slice(&((i - 1) as i32).to_le_bytes());
            }
        }
        let mut s = vec![0x7E];
        s.extend_from_slice(&(k as i32).to_le_bytes());
        s.extend_from_slice(&body);
        for id in 1..=k as u32 {
            s.extend_from_slice(&id.to_le_bytes());
        }
        guarded(5, k as u64, "ref chain", &s, check_stream);
        let st = take_stream(&s).unwrap();
        let got = Decoder::with_shared(st.shared_ids.clone())
            .with_budget(Budget::INBOUND)
            .load(&mut Reader::new(st.body));
        eprintln!(
            "ref chain k={k}: {}",
            match &got {
                Ok(v) => format!("ok, height {}", stats(v).0),
                Err(e) => format!("{e}"),
            }
        );
        if k == 400 {
            assert!(got.is_ok(), "a legal 400-deep chain was refused");
        }
    }
    // Height through sharing: S1 is a shared list nested 900 deep; S2 nests
    // 900 more LIST1 levels around a reference to S1, and so on. Each stage
    // costs ~1,800 nodes but the value gets 900 taller, so under the 2 M-node
    // budget the height can reach hundreds of thousands, far past the
    // client's 1000, and the recursive clone/drop/save of it is a stack
    // overflow.
    for stages in [1usize, 2, 3, 10, 200] {
        let per = 900usize;
        let mut body = vec![c::TUPLE, 0xFF];
        body.extend_from_slice(&(stages as i32).to_le_bytes());
        for st in 0..stages {
            // the outermost level of each stage registers, so a reference to
            // it clones the whole stage
            body.extend_from_slice(&[c::LIST | 0x40, 1]);
            body.extend(std::iter::repeat_n(c::LIST1, per));
            if st == 0 {
                body.push(c::NONE);
            } else {
                body.push(c::SHARED_REF);
                if st < 0xFF {
                    body.push(st as u8);
                } else {
                    body.push(0xFF);
                    body.extend_from_slice(&(st as i32).to_le_bytes());
                }
            }
        }
        let mut s = vec![0x7E];
        s.extend_from_slice(&(stages as i32).to_le_bytes());
        s.extend_from_slice(&body);
        for id in 1..=stages as u32 {
            s.extend_from_slice(&id.to_le_bytes());
        }
        guarded(5, stages as u64, "height through sharing", &s, check_stream);
    }
    // Many small descriptors: a DBRow whose descriptor has N columns.
    for cols in [100usize, 5_000, 40_000] {
        let names: Vec<(String, DbType)> =
            (0..cols).map(|i| (format!("n{i}"), DbType::I4)).collect();
        let pairs: Vec<(&str, DbType)> = names.iter().map(|(s, t)| (s.as_str(), *t)).collect();
        if let Ok(d) = DbRowDescriptor::from_pairs(&pairs) {
            let row = DbRow::new(d, vec![DbValue::Int(7); cols]).unwrap();
            let b = encode(&PyValue::DbRow(Box::new(row)));
            guarded(5, cols as u64, "wide dbrow", &b, check_stream);
        }
    }
    // Plain noise at length, for memory behaviour.
    for _ in 0..50 {
        let n = rng.range(1000, 20_000);
        let mut b = vec![0x7E, 0, 0, 0, 0];
        b.extend((0..n).map(|_| tagish(&mut rng)));
        guarded(5, n as u64, "noise", &b, check_stream);
    }
}

// --- the long mode ---------------------------------------------------------

/// Overnight run: all random targets with a seed and count from the
/// environment. `cargo test -p eve-marshal --test fuzz --release -- --ignored`.
#[test]
#[ignore = "long run: EVE_FUZZ_SEED, EVE_FUZZ_CASES"]
fn long_marshal() {
    let (seed, cases) = env_run();
    big_stack(move || {
        run_random_streams(seed, cases);
        run_raw_bytes(seed, cases);
        run_roundtrip(seed, cases / 2);
        run_mutations(seed, cases);
    });
}
