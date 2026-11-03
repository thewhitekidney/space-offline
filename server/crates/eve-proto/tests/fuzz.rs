//! Q-FUZZ: seeded fuzzing of the framing layer and the packet/call parsers.
//!
//! Std only (SplitMix64). Targets:
//!
//! - **framing** (`try_split_frame`, `write_frame`): arbitrary bytes, every
//!   flag combination, declared lengths up to 2^28, OOB lengths up to 2^32,
//!   and the streaming property: a buffer fed in arbitrary chunks yields the
//!   same frames as the whole buffer fed at once, whatever the chunking;
//! - **packet parsers** (`MachoPacket::from_value`/`from_state`,
//!   `MachoAddress::from_value`/`from_state`, `CallPayload::from_payload`) on
//!   decoded random bytes and on random value trees of the right and wrong
//!   shapes: never a panic, and `from(to(x)) == x` where an encoder exists.
//!
//! Properties: never panics; terminates; the bytes a frame claims are bounded
//! by the buffer actually held (a declared length never allocates or reads
//! past it); round-trips.
//!
//! Long mode: `EVE_FUZZ_SEED`, `EVE_FUZZ_CASES`, `--ignored`.

use std::panic::{self, AssertUnwindSafe};

use eve_marshal::value::{Budget, DECODE_STACK};
use eve_marshal::{Decoder, MachoNetMsg, PyValue, Reader, take_stream};
use eve_proto::{
    CallPayload, FrameError, MAX_FRAME, MachoAddress, MachoPacket, try_split_frame, write_frame,
};

const CASES: u64 = 20_000;

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
        .unwrap_or(1_000_000);
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

// --- framing ---------------------------------------------------------------

/// Split `buf` exactly as the connection loop does: take frames off the front
/// until more bytes are needed. Returns the frames (flags, oob, payload) and
/// either how many bytes were used or the error that stopped it.
type Taken = (
    Vec<(u32, Option<Vec<u8>>, Vec<u8>)>,
    usize,
    Option<FrameError>,
);

fn drain(buf: &[u8]) -> Taken {
    let mut frames = Vec::new();
    let mut used = 0;
    loop {
        match try_split_frame(&buf[used..]) {
            Ok(Some((f, n))) => {
                assert!(
                    n >= 4 && used + n <= buf.len(),
                    "consumed {n} of {}",
                    buf.len() - used
                );
                // The slices lie inside the frame that was claimed.
                assert!(f.payload.len() + f.oob.map_or(0, |o| o.len()) <= n - 4);
                assert!(f.header.frame_len() == n);
                assert!(f.header.frame_len() <= MAX_FRAME);
                frames.push((f.header.plen, f.oob.map(|o| o.to_vec()), f.payload.to_vec()));
                used += n;
            }
            Ok(None) => return (frames, used, None),
            Err(e) => return (frames, used, Some(e)),
        }
    }
}

fn check_framing(buf: &[u8], rng_seed: u64) {
    let whole = drain(buf);
    // Chunked arrival: append random-sized pieces, drain after each, keep the
    // unconsumed tail, exactly as `conn.rs` buffers. Same frames, same error.
    let mut rng = Rng(rng_seed);
    let mut held: Vec<u8> = Vec::new();
    let mut frames = Vec::new();
    let mut err = None;
    let mut fed = 0;
    while fed < buf.len() && err.is_none() {
        let n = rng.range(1, 9).min(buf.len() - fed);
        held.extend_from_slice(&buf[fed..fed + n]);
        fed += n;
        let (mut got, used, e) = drain(&held);
        frames.append(&mut got);
        held.drain(..used);
        err = e;
        // A pending partial frame is never larger than the cap.
        assert!(held.len() <= MAX_FRAME + 4);
    }
    assert_eq!(frames, whole.0, "chunking changed the frames");
    assert_eq!(err, whole.2, "chunking changed the error");
}

fn run_framing(seed: u64, cases: u64) {
    let mut rng = Rng(seed);
    for case in 0..cases {
        let mut buf = Vec::new();
        for _ in 0..rng.range(0, 4) {
            let payload_len = rng.range(0, 40);
            let payload = rng.bytes(payload_len);
            match rng.below(6) {
                0 => {
                    // a clean frame
                    write_frame(&payload, &mut buf).unwrap();
                }
                1 => {
                    // an OOB frame: flag, oobLen, oob, payload
                    let oob_len = rng.range(0, 10);
                    let plen = 4 + oob_len + payload.len();
                    let word0 = plen as u32 | 0x1000_0000;
                    buf.extend_from_slice(&word0.to_le_bytes());
                    buf.extend_from_slice(&(oob_len as u32).to_le_bytes());
                    buf.extend(rng.bytes(oob_len));
                    buf.extend_from_slice(&payload);
                }
                2 => {
                    // a hostile length word: any flags, any length
                    let w = rng.next_u64() as u32;
                    buf.extend_from_slice(&w.to_le_bytes());
                    buf.extend_from_slice(&payload);
                }
                3 => {
                    // OOB with a hostile oobLen
                    let plen = rng.range(4, 30) as u32;
                    buf.extend_from_slice(&(plen | 0x1000_0000).to_le_bytes());
                    let oob = *[u32::MAX, u32::MAX - 3, 1 << 31, plen, plen - 4, plen - 3, 0]
                        .get(rng.below(7) as usize)
                        .unwrap();
                    buf.extend_from_slice(&oob.to_le_bytes());
                    buf.extend(rng.bytes(plen as usize - 4));
                }
                4 => {
                    // declared length at and past the cap
                    let plen = *[
                        0x0FFF_FFFFu32,
                        0x0FFF_FFFE,
                        (MAX_FRAME - 4) as u32,
                        (MAX_FRAME - 3) as u32,
                        (MAX_FRAME - 5) as u32,
                    ]
                    .get(rng.below(5) as usize)
                    .unwrap();
                    buf.extend_from_slice(&plen.to_le_bytes());
                    buf.extend(rng.bytes(payload_len));
                }
                _ => buf.extend_from_slice(&payload),
            }
        }
        let chunk_seed = rng.next_u64();
        guarded(seed, case, "framing", &buf, |b| {
            check_framing(b, chunk_seed)
        });
    }
}

/// The writer refuses what the reader would, and `write` then `split` is the
/// identity for every length up to a few hundred and the edges of the cap.
#[test]
fn fuzz_framing_roundtrip_and_limits() {
    let mut rng = Rng(11);
    for len in (0..600usize).chain([65_535, 65_536, 1 << 20]) {
        let p = rng.bytes(len);
        let mut out = Vec::new();
        write_frame(&p, &mut out).unwrap();
        let (f, used) = try_split_frame(&out).unwrap().unwrap();
        assert_eq!((f.payload, used), (&p[..], len + 4));
        assert_eq!(f.oob, None);
        // one byte short: not yet a frame
        assert_eq!(try_split_frame(&out[..out.len() - 1]), Ok(None));
    }
    // the cap: a payload whose frame is MAX_FRAME is accepted by neither side
    // past it (no allocation of the big buffer is needed to see the refusal)
    let mut out = Vec::new();
    let big = vec![0u8; MAX_FRAME - 3];
    assert!(matches!(
        write_frame(&big, &mut out),
        Err(FrameError::TooLong { .. })
    ));
    assert!(out.is_empty());
    // every bit pattern of the unused / zlib / oob flags with a short length
    for flags in 0..16u32 {
        let word0 = (flags << 28) | 8;
        let mut b = word0.to_le_bytes().to_vec();
        b.extend_from_slice(&[0u8; 8]);
        let _ = try_split_frame(&b);
    }
}

#[test]
fn fuzz_framing_streams() {
    run_framing(1, CASES);
}

// --- packet and call parsers -------------------------------------------------

fn gen_scalar(rng: &mut Rng) -> PyValue {
    match rng.below(8) {
        0 => PyValue::None,
        1 => PyValue::Int(
            *[
                0i64,
                1,
                2,
                4,
                6,
                7,
                8,
                12,
                15,
                16,
                18,
                20,
                21,
                -1,
                i64::MAX,
                i64::MIN,
            ]
            .get(rng.below(16) as usize)
            .unwrap(),
        ),
        2 => PyValue::Int(rng.next_u64() as i64),
        3 => {
            let n = rng.range(0, 8);
            PyValue::Bytes(rng.bytes(n))
        }
        4 => PyValue::Unicode("svc".into()),
        5 => PyValue::Bool(rng.chance(50)),
        6 => PyValue::Real(1.5),
        _ => PyValue::Bytes(b"ClientHasReleasedTheseObjects".to_vec()),
    }
}

/// A value shaped like (or nearly like) an address, packet state, or call.
fn gen_shape(rng: &mut Rng, depth: u32) -> PyValue {
    if depth == 0 || rng.chance(30) {
        return gen_scalar(rng);
    }
    match rng.below(6) {
        0 => {
            // address-like: (type, ...)
            let ty = *[1i64, 2, 4, 8, 3, 0, 16]
                .get(rng.below(7) as usize)
                .unwrap();
            let n = rng.range(0, 5);
            let mut t = vec![PyValue::Int(ty)];
            t.extend((0..n).map(|_| gen_shape(rng, depth - 1)));
            PyValue::Tuple(t)
        }
        1 => {
            // packet-state-like: 14 fields, command first
            let mut t: Vec<PyValue> = (0..rng.range(12, 15))
                .map(|_| gen_shape(rng, depth - 1))
                .collect();
            t[0] = PyValue::Int(
                *[6i64, 7, 12, 15, 16, 18, 20, 21, 2, 3, 8]
                    .get(rng.below(11) as usize)
                    .unwrap(),
            );
            PyValue::Tuple(t)
        }
        2 => PyValue::Tuple(
            (0..rng.range(0, 6))
                .map(|_| gen_shape(rng, depth - 1))
                .collect(),
        ),
        3 => PyValue::List(
            (0..rng.range(0, 4))
                .map(|_| gen_shape(rng, depth - 1))
                .collect(),
        ),
        4 => PyValue::Dict(
            (0..rng.range(0, 3))
                .map(|_| (gen_shape(rng, depth - 1), gen_shape(rng, depth - 1)))
                .collect(),
        ),
        _ => PyValue::Instance {
            class: b"carbon.common.script.net.machoNetPacket.CallReq".to_vec(),
            state: Box::new(gen_shape(rng, depth - 1)),
        },
    }
}

fn parse_everything(v: &PyValue) {
    let _ = MachoAddress::from_value(v);
    let _ = MachoAddress::from_state(v);
    let _ = CallPayload::from_payload(v);
    if let Ok(p) = MachoPacket::from_value(v) {
        // a parsed packet re-encodes (or says why not) and the encoding parses
        // back to the same packet
        if let Ok(value) = p.to_value() {
            let again = MachoPacket::from_value(&value).expect("own encoding parses");
            // Debug text, not ==: a mutated float may be NaN.
            assert_eq!(
                format!("{again:?}"),
                format!("{p:?}"),
                "from_value(to_value(p)) != p"
            );
        }
        if let Ok(bytes) = p.to_payload() {
            let st = take_stream(&bytes).expect("own header");
            let v = Decoder::with_shared(st.shared_ids)
                .with_budget(Budget::INBOUND)
                .load(&mut Reader::new(st.body))
                .expect("own payload decodes");
            assert_eq!(
                format!(
                    "{:?}",
                    MachoPacket::from_value(&v).expect("own payload parses")
                ),
                format!("{p:?}")
            );
        }
    }
    let _ = MachoPacket::from_state(v);
    // any parsed address round-trips
    if let Ok(a) = MachoAddress::from_value(v) {
        assert_eq!(
            format!("{:?}", MachoAddress::from_value(&a.to_value())),
            format!("{:?}", Ok::<_, ()>(&a))
        );
        let _ = a.address_type();
        let _ = (a.service(), a.call_id());
    }
    if let Ok(a) = MachoAddress::from_state(v) {
        assert_eq!(
            format!("{:?}", MachoAddress::from_state(&a.to_state())),
            format!("{:?}", Ok::<_, ()>(&a))
        );
    }
    if let Ok(c) = CallPayload::from_payload(v) {
        let again = CallPayload::from_payload(&c.to_payload()).expect("own call parses");
        assert_eq!(format!("{again:?}"), format!("{c:?}"));
    }
}

fn run_shapes(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0x77);
    for case in 0..cases {
        let v = gen_shape(&mut rng, 4);
        let dbg = format!("{v:?}");
        guarded(seed, case, "shape", dbg.as_bytes(), |_| {
            parse_everything(&v)
        });
    }
}

/// Real packets: generated, encoded, mutated, then decoded the way the server
/// does and parsed.
fn run_packets(seed: u64, cases: u64) {
    let mut rng = Rng(seed ^ 0x99);
    for case in 0..cases {
        let cmd = *[
            MachoNetMsg::CallReq,
            MachoNetMsg::CallRsp,
            MachoNetMsg::Notification,
            MachoNetMsg::PingReq,
            MachoNetMsg::ErrorResponse,
            MachoNetMsg::SessionChangeNotification,
        ]
        .get(rng.below(6) as usize)
        .unwrap();
        let addr = |rng: &mut Rng| match rng.below(4) {
            0 => MachoAddress::node(rng.next_u64() as i64),
            1 => MachoAddress::client(rng.next_u64() as i64),
            2 => MachoAddress::Any {
                service: Some("svc".into()),
                call_id: Some(rng.below(100) as i64),
            },
            _ => MachoAddress::Broadcast {
                broadcast_id: "b".into(),
                narrowcast: vec![PyValue::Int(1)],
                id_type: "*clientID".into(),
            },
        };
        let (s, d) = (addr(&mut rng), addr(&mut rng));
        let body = (0..rng.range(0, 3)).map(|_| gen_scalar(&mut rng)).collect();
        let mut p = MachoPacket::new(cmd, s, d, body);
        if rng.chance(30) {
            p.user_id = Some(rng.next_u64() as i64);
        }
        let Ok(mut bytes) = p.to_payload() else {
            continue;
        };
        if rng.chance(70) {
            for _ in 0..rng.range(1, 3) {
                let i = rng.below(bytes.len() as u64) as usize;
                match rng.below(4) {
                    0 => bytes[i] ^= 1 << rng.below(8),
                    1 => bytes[i] = rng.byte(),
                    2 => bytes.truncate(i),
                    _ => bytes.insert(i, rng.byte()),
                }
                if bytes.is_empty() {
                    break;
                }
            }
        }
        guarded(seed, case, "packet bytes", &bytes, |b| {
            let Ok(st) = take_stream(b) else { return };
            let Ok(v) = Decoder::with_shared(st.shared_ids)
                .with_budget(Budget::INBOUND)
                .load(&mut Reader::new(st.body))
            else {
                return;
            };
            parse_everything(&v);
        });
    }
}

#[test]
fn fuzz_packet_and_call_shapes() {
    big_stack(|| run_shapes(2, CASES));
}

#[test]
fn fuzz_mutated_packet_payloads() {
    big_stack(|| run_packets(3, CASES / 2));
}

/// Every address-type code the enum knows, and a few it does not, as the
/// first element of a short tuple.
#[test]
fn fuzz_address_types_exhaustive() {
    for ty in 0..=255i64 {
        for n in 0..6usize {
            let mut t = vec![PyValue::Int(ty)];
            t.extend((0..n).map(|i| {
                if i % 2 == 0 {
                    PyValue::None
                } else {
                    PyValue::Int(i as i64)
                }
            }));
            let v = PyValue::Tuple(t);
            parse_everything(&v);
        }
    }
}

#[test]
#[ignore = "long run: EVE_FUZZ_SEED, EVE_FUZZ_CASES"]
fn long_proto() {
    let (seed, cases) = env_run();
    big_stack(move || {
        run_framing(seed, cases);
        run_shapes(seed, cases);
        run_packets(seed, cases / 2);
    });
}
