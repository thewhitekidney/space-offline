//! PA-10 (`--coalesce-destiny`): the classifier, the outbox tail merge, the
//! encode, and the order and history oracles. Invented ids and rows only; no
//! `[A]` value is asserted (the caps are used by name).

use super::tests::{noise, pair, read_frames, registered};
use super::*;
use std::io::Read;

const NODE: i64 = 9;

/// `(stamp, (name, (n,)))`, the shape every Destiny action takes.
fn entry(stamp: i64, name: &str, n: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(stamp),
        PyValue::Tuple(vec![
            PyValue::Bytes(name.as_bytes().to_vec()),
            PyValue::Tuple(vec![PyValue::Int(n)]),
        ]),
    ])
}

fn args_of(entries: Vec<PyValue>) -> Vec<PyValue> {
    vec![PyValue::List(entries), PyValue::Int(0)]
}

fn src_of(entries: Vec<PyValue>) -> PartSrc {
    PartSrc::of("DoDestinyUpdate", &args_of(entries)).expect("a part")
}

fn part(client: i64, stamp: i64, ns: &[i64]) -> Part {
    src_of(ns.iter().map(|n| entry(stamp, "Orbit", *n)).collect()).for_client(NODE, client)
}

/// A push of fixed entries to fixed clients.
#[derive(Debug)]
struct Entries {
    to: Vec<i64>,
    args: Vec<PyValue>,
}

impl crate::notify::Push for Entries {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("DoDestinyUpdate", self.args.clone())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// What a payload carries, for the oracles: a raw marker frame, or the
/// destiny entries `(stamp, name, n)` in order, with the batch they came in.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Raw(u32),
    Batch(Vec<(i64, String, i64)>),
}

fn raw_payload(n: u32) -> Vec<u8> {
    let mut v = b"RAW!".to_vec();
    v.extend_from_slice(&n.to_be_bytes());
    v
}

/// The client's side of one frame, with a test-local decoder.
fn decode(payload: &[u8]) -> Item {
    if let Some(rest) = payload.strip_prefix(b"RAW!") {
        return Item::Raw(u32::from_be_bytes(rest[..4].try_into().expect("4 bytes")));
    }
    let value = crate::loads(payload).expect("a marshal stream");
    let packet = eve_proto::MachoPacket::from_value(&value).expect("a packet");
    let PyValue::Tuple(t) = &packet.body[0] else {
        panic!("body")
    };
    let PyValue::Bytes(pickle) = &t[1] else {
        panic!("pickle")
    };
    let inner = crate::loads(pickle).expect("the pickle");
    let PyValue::Tuple(i) = inner else {
        panic!("0,x")
    };
    let PyValue::Tuple(i) = &i[1] else {
        panic!("1,args")
    };
    let PyValue::Tuple(a) = &i[1] else {
        panic!("args")
    };
    assert_eq!(a[1], PyValue::Int(0), "waitForBubble stays 0");
    assert_eq!(a.len(), 2, "no dogmaMessages");
    let PyValue::List(entries) = &a[0] else {
        panic!("list")
    };
    Item::Batch(
        entries
            .iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else { panic!("e") };
                let (PyValue::Int(s), PyValue::Tuple(nm)) = (&e[0], &e[1]) else {
                    panic!("(stamp, (name, args))")
                };
                let PyValue::Bytes(name) = &nm[0] else {
                    panic!("name")
                };
                let PyValue::Tuple(ar) = &nm[1] else {
                    panic!("args")
                };
                let n = match ar.first() {
                    Some(PyValue::Int(n)) => *n,
                    _ => 0,
                };
                (*s, String::from_utf8_lossy(name).into_owned(), n)
            })
            .collect(),
    )
}

/// What `take_due` gave, as payloads in order.
fn payloads(queue: Vec<Frame>) -> Vec<Vec<u8>> {
    queue
        .into_iter()
        .map(|f| match f {
            Frame::Part(p) => encode_part(&p).expect("encodes"),
            Frame::Raw(p) => p,
            Frame::Ready { .. } => panic!("no ready frames here"),
        })
        .collect()
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

/// T1: the classifier.
#[test]
fn only_a_plain_single_stamp_batch_is_a_part() {
    let ok = args_of(vec![entry(5, "Orbit", 1), entry(5, "Stop", 2)]);
    let (stamp, n, entries) = destiny_part("DoDestinyUpdate", &ok).expect("a part");
    assert_eq!((stamp, n, entries.len()), (5, 2, 2));
    let not = |what: &str, event: &str, args: Vec<PyValue>| {
        assert!(destiny_part(event, &args).is_none(), "{what}");
    };
    not(
        "SetState first",
        "DoDestinyUpdate",
        args_of(vec![entry(5, "SetState", 1), entry(5, "Stop", 2)]),
    );
    not(
        "SetState in the middle",
        "DoDestinyUpdate",
        args_of(vec![
            entry(5, "Stop", 1),
            entry(5, "SetState", 2),
            entry(5, "Stop", 3),
        ]),
    );
    not(
        "PackagedAction",
        "DoDestinyUpdate",
        args_of(vec![entry(5, "PackagedAction", 1)]),
    );
    not(
        "mixed stamps",
        "DoDestinyUpdate",
        args_of(vec![entry(5, "Stop", 1), entry(6, "Stop", 2)]),
    );
    not(
        "waitForBubble 1",
        "DoDestinyUpdate",
        vec![PyValue::List(vec![entry(5, "Stop", 1)]), PyValue::Int(1)],
    );
    not(
        "waitForBubble false",
        "DoDestinyUpdate",
        vec![
            PyValue::List(vec![entry(5, "Stop", 1)]),
            PyValue::Bool(false),
        ],
    );
    not(
        "dogmaMessages",
        "DoDestinyUpdate",
        vec![
            PyValue::List(vec![entry(5, "Stop", 1)]),
            PyValue::Int(0),
            PyValue::List(vec![]),
        ],
    );
    not("empty list", "DoDestinyUpdate", args_of(vec![]));
    not("not destiny", "OnSpecialFX", ok.clone());
    not("an On* push", "OnDamageMessage", ok.clone());
    not(
        "not a list",
        "DoDestinyUpdate",
        vec![PyValue::Tuple(vec![entry(5, "Stop", 1)]), PyValue::Int(0)],
    );
    not(
        "a bare entry",
        "DoDestinyUpdate",
        vec![PyValue::List(vec![PyValue::Int(5)]), PyValue::Int(0)],
    );
}

/// T2: the tail merge and each thing that stops one.
#[test]
fn a_part_merges_into_an_adjacent_same_stamp_tail_and_nothing_else() {
    let now = Instant::now();
    let mut o = Outbox::default();
    assert!(!o.post_part(now, part(1, 5, &[1, 2])).expect("posted"));
    assert!(o.post_part(now, part(1, 5, &[3])).expect("posted"));
    assert!(o.post_part(now, part(1, 5, &[4, 5])).expect("posted"));
    assert_eq!(o.len(), 1, "one entry");
    let got = payloads(o.take_due(now));
    assert_eq!(
        decode(&got[0]),
        Item::Batch((1..=5).map(|n| (5, "Orbit".to_owned(), n)).collect()),
        "the concatenation, in post order"
    );

    // A different stamp.
    let mut o = Outbox::default();
    o.post_part(now, part(1, 5, &[1])).expect("posted");
    assert!(!o.post_part(now, part(1, 6, &[2])).expect("posted"));
    assert_eq!(o.len(), 2);

    // A frame between two parts.
    let mut o = Outbox::default();
    o.post_part(now, part(1, 5, &[1])).expect("posted");
    o.post(now, Frame::Raw(raw_payload(0))).expect("posted");
    assert!(!o.post_part(now, part(1, 5, &[2])).expect("posted"));
    assert_eq!(o.len(), 3);

    // A back entry due later than this part: it merges, since the FIFO
    // would hold it behind that entry anyway.
    let mut o = Outbox::default();
    o.post_part(now + Duration::from_secs(2), part(1, 5, &[1]))
        .expect("posted");
    assert!(o.post_part(now, part(1, 5, &[2])).expect("posted"));
    assert_eq!(o.len(), 1);
    // ...one due within the slack after it merges...
    let mut o = Outbox::default();
    o.post_part(now, part(1, 5, &[1])).expect("posted");
    assert!(
        o.post_part(now + MERGE_DUE_SLACK, part(1, 5, &[2]))
            .expect("posted")
    );
    assert_eq!(o.len(), 1);
    // ...and one due later than that keeps its own entry, so it never goes
    // out before its `after`.
    let mut o = Outbox::default();
    o.post_part(now, part(1, 5, &[1])).expect("posted");
    assert!(
        !o.post_part(now + Duration::from_secs(2), part(1, 5, &[2]))
            .expect("posted")
    );
    assert_eq!(o.len(), 2);

    // The entry cap, by name.
    let mut o = Outbox::default();
    for n in 0..MAX_MERGED_ENTRIES as i64 {
        o.post_part(now, part(1, 5, &[n])).expect("posted");
    }
    assert_eq!(o.len(), 1, "up to the cap");
    assert!(!o.post_part(now, part(1, 5, &[-1])).expect("posted"));
    assert_eq!(o.len(), 2, "one more opens an entry");
}

fn xorshift(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

/// A random run of parts and frames (stamps from two values): the input
/// order of its items, and the frames to post.
fn random_run(seed: u64, len: usize) -> (Vec<Item>, Vec<Frame>) {
    let mut x = seed | 1;
    let mut want = Vec::new();
    let mut frames = Vec::new();
    let mut n = 0i64;
    for i in 0..len as u32 {
        if xorshift(&mut x).is_multiple_of(4) {
            want.push(Item::Raw(i));
            frames.push(Frame::Raw(raw_payload(i)));
        } else {
            let stamp = 5 + (xorshift(&mut x) % 2) as i64;
            let k = 1 + xorshift(&mut x) % 3;
            let ns: Vec<i64> = (0..k as i64).map(|j| n + j).collect();
            n += k as i64;
            want.push(Item::Batch(
                ns.iter().map(|n| (stamp, "Orbit".to_owned(), *n)).collect(),
            ));
            frames.push(Frame::Part(part(1, stamp, &ns)));
        }
    }
    (want, frames)
}

/// The entries of every batch in order, with the raw frames where they were.
fn flat(items: &[Item]) -> Vec<Item> {
    let mut out = Vec::new();
    for it in items {
        match it {
            Item::Raw(n) => out.push(Item::Raw(*n)),
            Item::Batch(es) => {
                for e in es {
                    out.push(Item::Batch(vec![e.clone()]));
                }
            }
        }
    }
    out
}

/// T3: the per-connection order oracle.
#[test]
fn merging_never_reorders_a_connections_stream() {
    for seed in 1..40u64 {
        let (want, frames) = random_run(seed.wrapping_mul(0x9E37_79B9), 120);
        let now = Instant::now();
        let mut o = Outbox::default();
        for f in frames {
            match f {
                Frame::Part(p) => {
                    o.post_part(now, p).expect("posted");
                }
                f => o.post(now, f).expect("posted"),
            }
        }
        let got: Vec<Item> = payloads(o.take_due(now))
            .iter()
            .map(|p| decode(p))
            .collect();
        assert!(got.len() <= want.len());
        assert_eq!(flat(&got), flat(&want), "seed {seed}: the entry stream");
        // Every merged batch holds one stamp.
        for it in &got {
            if let Item::Batch(es) = it {
                assert!(es.iter().all(|e| e.0 == es[0].0), "seed {seed}");
            }
        }
    }
}

/// The reference the clock encoded before PA-10, per recipient.
fn reference(node_id: i64, push: &dyn crate::notify::Push, client: i64) -> Vec<u8> {
    let (event, args) = push.to_push();
    eve_proto::MachoPacket::notification(
        eve_proto::MachoAddress::node(node_id),
        event,
        "*clientID",
        vec![PyValue::Int(client)],
        args,
    )
    .to_payload()
    .expect("encodes")
}

/// T4: a one-part entry is byte for byte today's frame, and `stage_frames`
/// keeps the drain's order, with frames where the classifier says frame.
#[test]
fn a_one_part_entry_is_byte_identical_to_the_frame_it_replaces() {
    let pushes: Vec<Entries> = vec![
        Entries {
            to: vec![11, 12],
            args: args_of(vec![entry(5, "Orbit", 1), entry(5, "Stop", 2)]),
        },
        Entries {
            to: vec![12],
            args: args_of(vec![entry(5, "SetState", 3)]),
        },
        Entries {
            to: vec![11, 12, 13],
            args: args_of(vec![entry(6, "WarpTo", 4)]),
        },
    ];
    let frames: Vec<Outgoing> = pushes
        .iter()
        .map(|p| Outgoing {
            push: Box::new(Entries {
                to: p.to.clone(),
                args: p.args.clone(),
            }),
            clients: p.to.clone(),
        })
        .collect();
    for threads in [1, 3, 8] {
        let staged = stage_frames_with(NODE, &frames, threads);
        let mut i = 0;
        for (push, o) in pushes.iter().zip(&frames) {
            for &c in &o.clients {
                let (client, frame) = &staged[i];
                i += 1;
                assert_eq!(*client, c);
                let want = reference(NODE, push, c);
                let is_set_state = push.args == pushes[1].args;
                match frame {
                    Frame::Part(p) => {
                        assert!(!is_set_state, "SetState is a barrier");
                        assert_eq!(encode_part(p).expect("encodes"), want, "{threads} threads");
                    }
                    Frame::Raw(bytes) => {
                        assert!(is_set_state, "only the barrier stays a frame");
                        assert_eq!(*bytes, want);
                    }
                    Frame::Ready { .. } => panic!("not deflated here"),
                }
            }
        }
        assert_eq!(i, staged.len());
    }
}

/// The client's history merge (PA-10 report 1.2, `Park.FlushState`), written
/// from the client's code: a state whose first entry is a `SetState` sets the
/// floor and purges entries strictly older; any other state older than the
/// floor is dropped; same-stamp entries are extended, never doubled.
#[derive(Default, Debug, PartialEq)]
struct History {
    floor: i64,
    entries: Vec<(i64, Vec<(String, i64)>)>,
}

impl History {
    fn flush(&mut self, state: &[(i64, String, i64)]) {
        if state[0].1 == "SetState" {
            self.floor = state[0].0;
            self.entries.retain(|h| h.0 >= self.floor);
        } else if state[0].0 < self.floor {
            return;
        }
        for (stamp, name, n) in state {
            let item = (name.clone(), *n);
            match self.entries.iter().position(|h| h.0 >= *stamp) {
                Some(i) if self.entries[i].0 == *stamp => self.entries[i].1.push(item),
                Some(i) => self.entries.insert(i, (*stamp, vec![item])),
                None => self.entries.push((*stamp, vec![item])),
            }
        }
    }
}

/// T5: with no pre-tick between them, the merged frames leave the client's
/// history exactly where the separate frames do, and no merged batch has a
/// `SetState` anywhere.
#[test]
fn the_clients_history_is_the_same_for_merged_and_separate_frames() {
    for seed in 1..40u64 {
        let mut x = seed.wrapping_mul(0x2545_F491) | 1;
        let now = Instant::now();
        let mut o = Outbox::default();
        let mut separate = History::default();
        let mut n = 0i64;
        for i in 0..80u32 {
            let stamp = 5 + (xorshift(&mut x) % 3) as i64;
            if xorshift(&mut x).is_multiple_of(6) {
                // A SetState batch: a barrier frame, first in its batch.
                n += 1;
                let state = vec![(stamp, "SetState".to_owned(), n)];
                separate.flush(&state);
                let a = args_of(vec![entry(stamp, "SetState", n)]);
                assert!(destiny_part("DoDestinyUpdate", &a).is_none());
                let payload = eve_proto::MachoPacket::notification(
                    eve_proto::MachoAddress::node(NODE),
                    "DoDestinyUpdate",
                    "*clientID",
                    vec![PyValue::Int(1)],
                    a,
                )
                .to_payload()
                .expect("encodes");
                o.post(now, Frame::Raw(payload)).expect("posted");
                let _ = i;
            } else {
                let k = 1 + xorshift(&mut x) % 3;
                let ns: Vec<i64> = (0..k as i64).map(|j| n + 1 + j).collect();
                n += k as i64;
                separate.flush(
                    &ns.iter()
                        .map(|n| (stamp, "Orbit".to_owned(), *n))
                        .collect::<Vec<_>>(),
                );
                o.post_part(now, part(1, stamp, &ns)).expect("posted");
            }
        }
        let mut merged = History::default();
        for p in payloads(o.take_due(now)) {
            // A frame is either a part or a SetState batch.
            let Item::Batch(state) = decode(&p) else {
                panic!("no raw frames in this run")
            };
            let at = state.iter().position(|e| e.1 == "SetState");
            assert!(at.is_none() || at == Some(0), "SetState only at index 0");
            if at.is_none() {
                assert!(state.iter().all(|e| e.0 == state[0].0), "one stamp");
            }
            merged.flush(&state);
        }
        assert_eq!(merged, separate, "seed {seed}");
    }
}

/// T6: the capture records the frame that went on the wire, once per frame.
#[test]
fn the_capture_holds_the_merged_frame_the_socket_got() {
    let dir = std::env::temp_dir().join(format!("pa10-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("1.evecap");
    let (ours, peer) = pair();
    let cap = Capture::create(&path).expect("capture");
    let wire = Arc::new(Mutex::new(Wire::new(ours, Some(cap))));
    let writers = Writers::default();
    writers.register(1, &wire);
    let now = Instant::now();
    assert!(writers.post_part(1, now, part(1, 5, &[1, 2])));
    assert!(writers.post_part(1, now, part(1, 5, &[3])));
    assert!(writers.post(1, now, raw_payload(7)));
    sweep_shard(&writers, 0, 1, Instant::now());
    let got = read_frames(peer, 2);
    assert_eq!(got.len(), 2, "two frames left, from three posts");
    assert_eq!(
        decode(&got[0]),
        Item::Batch((1..=3).map(|n| (5, "Orbit".to_owned(), n)).collect())
    );
    assert_eq!(wire.lock().expect("wire").capture_count(), 2);
    let records: Vec<Vec<u8>> = crate::capture::read_all(&path)
        .expect("capture reads")
        .into_iter()
        .map(|r| r.bytes)
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0], got[0],
        "the record is the payload on the socket"
    );
    assert_eq!(records[1], raw_payload(7));
    let _ = std::fs::remove_dir_all(&dir);
}

/// T7: a merge takes no frame slot, the byte estimate still counts, and an
/// overflow closes the outbox and shuts the socket.
#[test]
fn merged_parts_take_one_slot_and_the_caps_still_close_the_wire() {
    let now = Instant::now();
    // 4096 parts, one stamp: 4096 / MAX entries, nowhere near the frame cap.
    let mut o = Outbox::default();
    for n in 0..OUTBOX_FRAMES as i64 {
        o.post_part(now, part(1, 5, &[n])).expect("posted");
    }
    assert_eq!(o.len(), OUTBOX_FRAMES.div_ceil(MAX_MERGED_ENTRIES));

    // The frame cap by distinct stamps, through a registered wire.
    let far = far();
    let (ours, mut peer) = pair();
    let mut reader = ours.try_clone().expect("clone");
    let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
    let writers = Writers::default();
    writers.register(7, &wire);
    for s in 0..OUTBOX_FRAMES as i64 {
        assert!(writers.post_part(7, far, part(7, s, &[s])), "below the cap");
    }
    assert!(!wire.lock().expect("wire").is_closed());
    assert!(
        !writers.post_part(7, far, part(7, 0, &[1])),
        "a new entry past the frame cap is refused"
    );
    assert!(wire.lock().expect("wire").is_closed());
    assert_eq!(wire.lock().expect("wire").queued(), 0);
    reader
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut b = [0u8; 1];
    assert_eq!(reader.read(&mut b).expect("woken, not timed out"), 0);
    peer.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    assert_eq!(peer.read(&mut b).expect("the peer sees the end"), 0);

    // The byte cap: two halves fit, merged into one slot; a third crosses it.
    let (ours, _peer) = pair();
    let wire = Arc::new(Mutex::new(Wire::new(ours, None)));
    let writers = Writers::default();
    writers.register(8, &wire);
    let half = |n: i64| {
        let entries = vec![entry(5, "Orbit", n)];
        let src = PartSrc::new(
            destiny_part("DoDestinyUpdate", &args_of(entries)).expect("a part"),
            OUTBOX_BYTES / 2 - PART_ENVELOPE,
        );
        src.for_client(NODE, 8)
    };
    assert!(writers.post_part(8, far, half(1)));
    assert!(
        writers.post_part(8, far, half(2)),
        "exactly the cap is taken"
    );
    assert_eq!(wire.lock().expect("wire").queued(), 1, "in one slot");
    assert!(!wire.lock().expect("wire").is_closed());
    assert!(!writers.post_part(8, far, half(3)), "past the byte cap");
    assert!(wire.lock().expect("wire").is_closed());
}

/// `want` items in, in this order, through the writers.
fn flatten_stream(frames: &[Vec<u8>]) -> Vec<Item> {
    let items: Vec<Item> = frames
        .iter()
        .map(|f| {
            let f = if eve_inflate::looks_like_zlib(f) {
                eve_inflate::inflate_zlib(f).expect("a whole zlib stream")
            } else {
                f.clone()
            };
            decode(&f)
        })
        .collect();
    flat(&items)
}

/// T8a: PA-4's shard order test with parts mixed in. Every client's flattened
/// stream equals what was posted to it, while three shard threads sweep.
#[test]
fn shards_keep_each_clients_order_with_parts_in_the_queue() {
    let writers = Arc::new(Writers::default());
    let shards = 3;
    let ids = [10_i64, 11, 12, 13, 14];
    let per = 150u32;
    let mut keep = Vec::new();
    let mut readers = Vec::new();
    for id in ids {
        let (wire, peer) = registered(&writers, id);
        keep.push(wire);
        readers.push(std::thread::spawn(move || read_frames(peer, usize::MAX)));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut pool = Vec::new();
    for s in 0..shards {
        let (w, stop) = (Arc::clone(&writers), Arc::clone(&stop));
        pool.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                sweep_shard(&w, s, shards, Instant::now());
                std::thread::sleep(Duration::from_millis(1));
            }
        }));
    }
    let now = Instant::now();
    let mut want: Vec<Item> = Vec::new();
    for n in 0..per {
        for id in ids {
            if n % 5 == 0 {
                assert!(writers.post(id, now, raw_payload(n)));
            } else {
                assert!(writers.post_part(
                    id,
                    now,
                    part(id, 5 + i64::from(n / 40), &[i64::from(n)])
                ));
            }
        }
        want.push(if n % 5 == 0 {
            Item::Raw(n)
        } else {
            Item::Batch(vec![(
                5 + i64::from(n / 40),
                "Orbit".to_owned(),
                i64::from(n),
            )])
        });
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while keep
        .iter()
        .any(|w| w.lock().map_or(true, |w| w.queued() > 0 || w.unsent() > 0))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    for w in &keep {
        w.lock().expect("wire").close();
    }
    for (r, id) in readers.into_iter().zip(ids) {
        let frames = r.join().expect("reader");
        assert_eq!(flatten_stream(&frames), flat(&want), "client {id}");
    }
    stop.store(true, Ordering::Relaxed);
    for t in pool {
        t.join().expect("shard");
    }
}

/// T8b: a partial write of a merged frame resumes byte-identical: the
/// peer's bytes equal one blocking write of the frames the outbox produced.
#[test]
fn a_partial_write_of_a_merged_frame_resumes_byte_identical() {
    let writers = Writers::default();
    let (wire, mut peer) = registered(&writers, 5);
    let now = Instant::now();
    // Entries carry noise so the merged frames are big and do not deflate
    // away; two stamps and a raw frame between give three frames.
    let big = |stamp: i64, n: i64| {
        PyValue::Tuple(vec![
            PyValue::Int(stamp),
            PyValue::Tuple(vec![
                PyValue::Bytes(b"Orbit".to_vec()),
                PyValue::Tuple(vec![
                    PyValue::Int(n),
                    PyValue::Bytes(noise(160 * 1024, n as u64)),
                ]),
            ]),
        ])
    };
    let mut probe = Outbox::default();
    for n in 0..40i64 {
        let stamp = if n < 25 { 5 } else { 6 };
        let p = src_of(vec![big(stamp, n)]).for_client(NODE, 5);
        probe.post_part(now, p.clone()).expect("posted");
        assert!(writers.post_part(5, now, p));
        if n == 24 {
            probe.post(now, Frame::Raw(raw_payload(1))).expect("posted");
            assert!(writers.post(5, now, raw_payload(1)));
        }
    }
    let mut expect = Vec::new();
    for p in payloads(probe.take_due(now)) {
        write_frame(&compress_payload(&p), &mut expect).expect("frame");
    }
    let want = expect.len();
    let reader = std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut chunk = vec![0u8; 32768];
        peer.set_read_timeout(Some(Duration::from_secs(10))).ok();
        while got.len() < want {
            std::thread::sleep(Duration::from_millis(2));
            match peer.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
            }
        }
        got
    });
    let mut resumed = false;
    for _ in 0..3000 {
        sweep_shard(&writers, 0, 1, Instant::now());
        let w = wire.lock().expect("wire");
        resumed |= w.unsent() > 0;
        if w.unsent() == 0 && w.queued() == 0 {
            break;
        }
        drop(w);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(reader.join().expect("reader"), expect);
    assert!(resumed, "the test never met a partial write");
}

/// T11 (part): a merge is one queue slot however many parts it holds.
#[test]
fn posting_parts_through_the_registry_merges_in_the_outbox() {
    let writers = Writers::default();
    let (w, _peer) = registered(&writers, 1);
    let now = far();
    for n in 0..3 {
        assert!(writers.post_part(1, now, part(1, 5, &[n])));
    }
    assert_eq!(w.lock().expect("wire").queued(), 1);
}
