//! P4.1 over a real socket: a hostile NPC acts on a bot that does nothing.
//!
//! `docs/lanes/npc.md` §8.2. The same limitation as every test in this
//! crate: both ends marshal with `eve-marshal`, so this proves the server's
//! *behaviour* -- an NPC sees, locks and shoots with no payload from anybody
//! -- and not its bytes, which are the client suite's business.

use super::common::{SYSTEM, config, pilot, serve};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const ALICE_SHIP: i64 = 100_000_001;
/// The no-bundle rat, invented (`npc::FIXTURE_RAT`, CP61), and its owner.
const HIJACKER: i64 = eve_server::npc::FIXTURE_RAT.type_id;
const ARCHANGELS: i64 = eve_server::npc::FIXTURE_RAT.owner_corp_id;

/// Alice in a Rifter -- an NPC ignores a capsule, and the sandbox's default
/// ship is one -- and Bob in his capsule, at the one station.
fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        eve_server::model::Character {
            ship_type_id: 587,
            ..pilot(ALICE, "Alice", ALICE_SHIP)
        },
        pilot(BOB, "Bob", 100_000_002),
    ];
    serve(config)
}

fn in_space(addr: SocketAddr, who: i64) -> eve_bot::Conn {
    let mut c = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("connects");
    c.handshake().expect("handshake");
    c.select_character(who).expect("a character");
    c.undock().expect("undocks");
    c
}

/// `/entity deploy 1 <the fixture rat>`, and every push that came with it or right
/// after it: the `AddBalls2` is queued on the turn and may trail the answer.
fn deploy(c: &mut eve_bot::Conn) -> Vec<PyValue> {
    let mut frames = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(
                format!("/entity deploy 1 {HIJACKER}").into_bytes(),
            )],
        )
        .expect("the slash is answered")
        .notifications;
    frames.extend(
        c.collect_pushes(Duration::from_millis(500))
            .unwrap_or_default(),
    );
    frames
}

/// A dict's value by `str` key; `PyValue::Dict` is `(value, key)`.
fn key<'a>(dict: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let dict = match dict {
        PyValue::Instance { state, .. } => state.as_ref(),
        other => other,
    };
    let PyValue::Dict(pairs) = dict else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| *k == PyValue::Bytes(name.as_bytes().to_vec()))
        .map(|(v, _)| v)
}

/// The NPC ball an `AddBalls2` in this frame adds, if it adds one: its slim
/// must carry the Hijacker's type and the Archangels as owner.
fn npc_added(frame: &PyValue) -> Option<i64> {
    let (event, args) = eve_bot::notification(frame)?;
    if event != "DoDestinyUpdate" {
        return None;
    }
    eve_bot::read_action_list(&args)
        .into_iter()
        .filter(|(_, name, _)| name == "AddBalls2")
        .find_map(|(_, _, operands)| {
            let PyValue::Tuple(state) = operands.first()? else {
                return None;
            };
            let PyValue::List(extra) = state.get(1)? else {
                return None;
            };
            extra.iter().find_map(|e| {
                let slim = match e {
                    PyValue::Tuple(pair) => pair.first()?,
                    other => other,
                };
                (key(slim, "typeID") == Some(&PyValue::Int(HIJACKER))
                    && key(slim, "ownerID") == Some(&PyValue::Int(ARCHANGELS)))
                .then(|| match key(slim, "itemID") {
                    Some(PyValue::Int(id)) => Some(*id),
                    _ => None,
                })
                .flatten()
            })
        })
}

/// **B1.** Alice deploys one NPC and then makes no call at all. Within 30 s,
/// in this order of first appearance: the NPC's ball, its lock on her, its
/// gun on her, and her shield bar below full -- all of it from the clock.
#[test]
fn an_npc_locks_and_shoots_a_bot_that_does_nothing() {
    let addr = start();
    let mut alice = in_space(addr, ALICE);
    alice.enter_space(SYSTEM).expect("alice reads the world");

    let mut frames = deploy(&mut alice);
    let npc = frames
        .iter()
        .find_map(npc_added)
        .expect("the deploy's AddBalls2 names a Hijacker owned by the Archangels");

    // From here Alice does nothing.
    let mut order: Vec<&'static str> = vec!["added"];
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) && order.len() < 4 {
        frames.extend(
            alice
                .collect_pushes(Duration::from_millis(500))
                .unwrap_or_default(),
        );
        for frame in std::mem::take(&mut frames) {
            let Some((event, args)) = eve_bot::notification(&frame) else {
                continue;
            };
            match event.as_str() {
                "OnTarget"
                    if args.first() == Some(&PyValue::Bytes(b"otheradd".to_vec()))
                        && args.get(1) == Some(&PyValue::Int(npc))
                        && !order.contains(&"locked") =>
                {
                    order.push("locked");
                }
                "OnSpecialFX"
                    if args.first() == Some(&PyValue::Int(npc))
                        && args.get(1) == Some(&PyValue::Int(npc))
                        && args.get(3) == Some(&PyValue::Int(ALICE_SHIP))
                        && args.get(7) == Some(&PyValue::Int(1))
                        && !order.contains(&"firing") =>
                {
                    order.push("firing");
                }
                "DoDestinyUpdate" if !order.contains(&"hit") => {
                    let hit = eve_bot::read_action_list(&args).iter().any(|(_, f, a)| {
                        f == "OnDamageStateChange"
                            && a.first() == Some(&PyValue::Int(ALICE_SHIP))
                            && match a.get(1) {
                                Some(PyValue::Tuple(bars)) => matches!(
                                    bars.first(),
                                    Some(PyValue::Tuple(shield))
                                        if eve_bot::real_at(shield, 0).is_some_and(|s| s < 1.0)
                                ),
                                _ => false,
                            }
                    });
                    if hit {
                        order.push("hit");
                    }
                }
                _ => {}
            }
        }
    }
    eprintln!(
        "npc: {order:?} after {:.1} s",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(order, vec!["added", "locked", "firing", "hit"]);
}

/// **B2.** Bob binds after the spawn: his `SetState` holds the NPC's ball,
/// in the NPC id range, with the Archangels as its slim's owner.
#[test]
fn a_bot_that_binds_after_the_spawn_sees_the_npc() {
    let addr = start();
    let mut alice = in_space(addr, ALICE);
    alice.enter_space(SYSTEM).expect("alice reads the world");
    let npc = deploy(&mut alice)
        .iter()
        .find_map(npc_added)
        .expect("a Hijacker");
    assert!(
        (eve_server::npc::MIN_NPC_ID..eve_server::npc::END_NPC_ID).contains(&npc),
        "{npc}"
    );

    let mut bob = in_space(addr, BOB);
    let outcome = bob.bind_ballpark(SYSTEM).expect("bob binds");
    let (park, slims) = outcome
        .notifications
        .iter()
        .filter_map(eve_bot::notification)
        .filter(|(event, _)| event == "DoDestinyUpdate")
        .find_map(|(_, args)| {
            let world = eve_bot::read_set_state(&args)?;
            let (_, _, operands) = eve_bot::read_action_list(&args).into_iter().next()?;
            let Some(PyValue::List(slims)) = key(operands.first()?, "slims").cloned() else {
                return None;
            };
            Some((world.park, slims))
        })
        .expect("bob's SetState");
    assert!(
        park.balls.iter().any(|b| b.ball_id == npc),
        "the NPC's ball is in bob's park"
    );
    let slim = slims
        .iter()
        .find(|s| key(s, "itemID") == Some(&PyValue::Int(npc)))
        .expect("and its slim");
    assert_eq!(key(slim, "ownerID"), Some(&PyValue::Int(ARCHANGELS)));
    assert_eq!(key(slim, "charID"), Some(&PyValue::None));
}
