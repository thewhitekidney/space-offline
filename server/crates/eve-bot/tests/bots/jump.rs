//! P3.3 over a real socket: a bot jumps through a stargate into a second
//! system, and the bots on either side see what they should.
//!
//! `docs/lanes/stargates.md` §4.3 tests 15 and 16. The same limitation as
//! every test in this crate: both ends marshal with `eve-marshal`, so this
//! proves the server's *behaviour* -- the order of the three things a jump
//! sends, on the world clock, and the arrival -- and not its bytes, which
//! are the client's to judge. Whether the real client takes the "no ego"
//! branch on the session change is the human check, not this.
//!
//! **Synthetic furniture**: two systems with made-up gate ids and
//! coordinates, the no-client-data rule. The system ids are the sandbox's
//! home (`Character::default()`'s) and one other.

use super::common::{config, pilot, serve};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const ALICE_SHIP: i64 = 100_000_001;
const BOB_SHIP: i64 = 100_000_002;
const A: i64 = 30_000_001;
const B: i64 = 30_000_003;
const GATE_A: i64 = 50_000_001;
const GATE_B: i64 = 50_000_002;

/// Synthetic, in the ten-column format `tools/celestials.py` writes.
fn catalogue() -> eve_server::celestial::Catalogue {
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{A}\t20000001\t10000001\n\
         #system\t{B}\t20000001\t10000001\n\
         planet\t40000002\t11\t500.0\t100000000.0\t0.0\t0.0\t{A}\t0\t0\n\
         gate\t{GATE_A}\t29624\t2473.0\t3000000000.0\t0.0\t0.0\t{A}\t{GATE_B}\t{B}\n\
         planet\t40000102\t11\t500.0\t0.0\t100000000.0\t0.0\t{B}\t0\t0\n\
         gate\t{GATE_B}\t29624\t2473.0\t0.0\t0.0\t-3000000000.0\t{B}\t{GATE_A}\t{A}\n"
    ))
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
    ];
    config.sandbox.celestials = catalogue();
    serve(config)
}

/// Undocked and bound in the home system, with its state read.
fn in_space(addr: SocketAddr, who: i64) -> (eve_bot::Conn, eve_bot::WorldState) {
    // Five seconds: long enough for any reply on loopback, short enough
    // that [`watch`]'s last read ends soon after the jump's last frame.
    let mut c = eve_bot::Conn::connect(addr, Duration::from_secs(5)).expect("connects");
    c.handshake().expect("handshake");
    c.select_character(who).expect("a character");
    c.undock().expect("undocks");
    let world = c.enter_space(A).expect("a state in the home system");
    (c, world)
}

/// What one frame says about a jump, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    /// `OnSpecialFX(ship, ..., 'effects.JumpOut', ...)`.
    Fx(i64),
    /// `RemoveBalls([ship])`.
    Removed(i64),
    /// A session change `solarsystemid: (old, new)`.
    Change(i64, i64),
    /// An `AddBalls2` naming this ship.
    Added(i64),
}

fn classify(frame: &PyValue) -> Vec<Seen> {
    if let PyValue::Instance { class, .. } = frame
        && class.ends_with(b"SessionChangeNotification")
    {
        return pair_under(frame, "solarsystemid")
            .map(|(a, b)| Seen::Change(a, b))
            .into_iter()
            .collect();
    }
    let Some((event, args)) = eve_bot::notification(frame) else {
        return Vec::new();
    };
    if event != "DoDestinyUpdate" {
        return Vec::new();
    }
    eve_bot::read_action_list(&args)
        .into_iter()
        .filter_map(|(_, name, ops)| match name.as_str() {
            "OnSpecialFX" if ops.get(5) == Some(&PyValue::Bytes(b"effects.JumpOut".to_vec())) => {
                match ops.first() {
                    Some(PyValue::Int(ship)) => Some(Seen::Fx(*ship)),
                    _ => None,
                }
            }
            "RemoveBalls" => match ops.first() {
                // A jumper's own batch clears its park, with the ego last.
                Some(PyValue::List(ids)) => match ids.last() {
                    Some(PyValue::Int(ship)) => Some(Seen::Removed(*ship)),
                    _ => None,
                },
                _ => None,
            },
            "AddBalls2" => item_ids(&ops).into_iter().next().map(Seen::Added),
            _ => None,
        })
        .collect()
}

/// `(old, new)` under a dict key anywhere in a value. `PyValue::Dict` is
/// `(value, key)`.
fn pair_under(v: &PyValue, name: &str) -> Option<(i64, i64)> {
    match v {
        PyValue::Dict(kv) => kv.iter().find_map(|(value, key)| {
            if *key == PyValue::Bytes(name.as_bytes().to_vec())
                && let PyValue::Tuple(p) = value
                && let [PyValue::Int(a), PyValue::Int(b)] = p.as_slice()
            {
                return Some((*a, *b));
            }
            pair_under(value, name)
        }),
        PyValue::Instance { state, .. } => pair_under(state, name),
        PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().find_map(|x| pair_under(x, name)),
        _ => None,
    }
}

/// Every `itemID` in every slim dict below this value.
fn item_ids(v: &[PyValue]) -> Vec<i64> {
    fn walk(v: &PyValue, out: &mut Vec<i64>) {
        match v {
            PyValue::Dict(kv) => {
                for (value, key) in kv {
                    if *key == PyValue::Bytes(b"itemID".to_vec())
                        && let PyValue::Int(id) = value
                    {
                        out.push(*id);
                    }
                    walk(value, out);
                }
            }
            PyValue::Instance { state, .. } => walk(state, out),
            PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    v.iter().for_each(|x| walk(x, &mut out));
    out
}

/// The `jumps` row of the slim whose `itemID` is `item`, as its two values.
fn jumps_of(v: &PyValue, item: i64) -> Option<Vec<eve_marshal::DbValue>> {
    match v {
        PyValue::Dict(kv) => {
            let is_it = kv.iter().any(|(value, key)| {
                *key == PyValue::Bytes(b"itemID".to_vec()) && *value == PyValue::Int(item)
            });
            if is_it {
                return kv.iter().find_map(|(value, key)| {
                    if *key != PyValue::Bytes(b"jumps".to_vec()) {
                        return None;
                    }
                    let PyValue::List(rows) = value else {
                        return None;
                    };
                    match rows.first()? {
                        PyValue::DbRow(row) => Some(row.values().to_vec()),
                        _ => None,
                    }
                });
            }
            kv.iter().find_map(|(value, _)| jumps_of(value, item))
        }
        PyValue::Instance { state, .. } => jumps_of(state, item),
        PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().find_map(|x| jumps_of(x, item)),
        _ => None,
    }
}

/// Read every frame for `within`, or until `stop` has been seen, keeping
/// what a jump would show. A read timeout ends the watch: silence is an
/// answer here, and the caller asserts on what was seen.
fn watch(c: &mut eve_bot::Conn, within: Duration, stop: Option<Seen>) -> Vec<Seen> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match c.recv() {
            Ok(frame) => seen.extend(classify(&frame)),
            Err(eve_bot::Error::Io(_)) => break,
            Err(e) => panic!("the connection failed while watching: {e}"),
        }
        if stop.is_some_and(|s| seen.contains(&s)) {
            break;
        }
    }
    seen
}

fn jump(c: &mut eve_bot::Conn, from: i64, to: i64, ship: i64) -> Vec<Seen> {
    let outcome = c
        .bound_call(
            "CmdStargateJump",
            vec![PyValue::Int(from), PyValue::Int(to), PyValue::Int(ship)],
        )
        .expect("the jump is answered");
    assert_eq!(
        eve_bot::reply_value(&outcome.result),
        Some(PyValue::None),
        "CmdStargateJump answers None; PerformSessionChange discards it"
    );
    outcome.notifications.iter().flat_map(classify).collect()
}

/// The order a jump's frames first appear in.
fn order(seen: &[Seen]) -> Vec<Seen> {
    let mut out = Vec::new();
    for s in seen {
        if !out.contains(s) {
            out.push(*s);
        }
    }
    out
}

/// **15.** One bot jumps A -> B. Within the jump delay and some slack it
/// sees, in this order, its own departure FX, its own ship removed from its
/// own park, and the session change into B. Its next bind is B's park, with
/// the ship by gate B and gate B's slim pointing back at gate A.
#[test]
fn a_bot_jumps_and_arrives_by_the_far_gate() {
    let addr = start();
    let (mut alice, home) = in_space(addr, ALICE);
    assert!(
        home.park.balls.iter().any(|b| b.ball_id == GATE_A),
        "the home gate is a ball, so it can be clicked"
    );
    assert!(!home.park.balls.iter().any(|b| b.ball_id == GATE_B));

    let mut seen = jump(&mut alice, GATE_A, GATE_B, ALICE_SHIP);
    seen.extend(watch(
        &mut alice,
        eve_server::jump::JUMP_DELAY + Duration::from_secs(2),
        Some(Seen::Change(A, B)),
    ));
    assert_eq!(
        order(&seen),
        vec![
            Seen::Fx(ALICE_SHIP),
            Seen::Removed(ALICE_SHIP),
            Seen::Change(A, B)
        ],
        "FX, then the ship's removal, then the session change"
    );

    // `bind_ballpark`, not `rebind_ballpark`: the jump's session change
    // released the park, as the client's `Park.Release` does (P8a).
    let outcome = alice.bind_ballpark(B).expect("binds B's park");
    let (state, slims) = outcome
        .notifications
        .iter()
        .find_map(|f| {
            let (event, args) = eve_bot::notification(f)?;
            (event == "DoDestinyUpdate")
                .then(|| eve_bot::read_set_state(&args).map(|s| (s, args.clone())))
                .flatten()
        })
        .expect("a SetState for B");
    let ego = state.ego_ball().expect("the ship is in the park").position;
    let gate = state
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == GATE_B)
        .expect("gate B is a ball");
    let surface = distance(ego, gate.position) - f64::from(gate.radius);
    assert!(
        surface > 0.0 && surface < eve_server::jump::MAX_JUMP_DISTANCE,
        "the arrival is a place to jump back from: {surface} m"
    );
    assert!(!state.park.balls.iter().any(|b| b.ball_id == GATE_A));
    let row = slims.iter().find_map(|a| jumps_of(a, GATE_B));
    assert_eq!(
        row,
        Some(vec![
            eve_marshal::DbValue::Int(GATE_A),
            eve_marshal::DbValue::Int(A)
        ]),
        "gate B leads back to gate A in A"
    );
}

/// **16.** Two bots. Bob stays in A while Alice jumps: he sees her FX and
/// her ship removed, and no session change of his own. Then Bob follows her:
/// Alice, already in B, is told about his ship when he arrives.
#[test]
fn the_pilots_on_both_sides_see_the_jump() {
    let addr = start();
    let (mut bob, _) = in_space(addr, BOB);
    let (mut alice, _) = in_space(addr, ALICE);
    // Bob's AddBalls2 for Alice's undock.
    let _ = bob.collect_pushes(Duration::from_millis(500));

    let _ = jump(&mut alice, GATE_A, GATE_B, ALICE_SHIP);
    let bob_saw = watch(
        &mut bob,
        eve_server::jump::JUMP_DELAY + Duration::from_secs(1),
        None,
    );
    assert!(bob_saw.contains(&Seen::Fx(ALICE_SHIP)), "{bob_saw:?}");
    assert!(bob_saw.contains(&Seen::Removed(ALICE_SHIP)), "{bob_saw:?}");
    assert!(
        !bob_saw.iter().any(|s| matches!(s, Seen::Change(..))),
        "the observer's session does not move: {bob_saw:?}"
    );
    let alice_saw = watch(
        &mut alice,
        eve_server::jump::JUMP_DELAY,
        Some(Seen::Change(A, B)),
    );
    assert!(alice_saw.contains(&Seen::Change(A, B)), "{alice_saw:?}");
    alice.bind_ballpark(B).expect("Alice binds B");

    let _ = jump(&mut bob, GATE_A, GATE_B, BOB_SHIP);
    let bob_saw = watch(
        &mut bob,
        eve_server::jump::JUMP_DELAY + Duration::from_secs(2),
        Some(Seen::Change(A, B)),
    );
    assert!(bob_saw.contains(&Seen::Change(A, B)), "{bob_saw:?}");
    bob.bind_ballpark(B).expect("Bob binds B");
    let alice_saw: Vec<Seen> = alice
        .collect_pushes(Duration::from_millis(1500))
        .expect("pushes")
        .iter()
        .flat_map(classify)
        .collect();
    assert!(
        alice_saw.contains(&Seen::Added(BOB_SHIP)),
        "the pilot already in B sees the arrival: {alice_saw:?}"
    );
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
