//! P8c over a real socket: an escort bot -- brain, driver, verbs -- fighting
//! for a friend against an in-process server.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **No `[A]` value is an expected value here.** The bot runs at
//! [`Pace::Fast`]; the assertions are about kinds, ids and order -- the NPC
//! is gone, the bot's guns were on it, the bot went back to its boss, a
//! line reached the fleet room -- never a range, a radius, a delay, a damage
//! number or a line's words. Deadlines are generous ceilings.
//!
//! **The friend is a plain connection** ("Alice"): her GM commands --
//! `/tp`, `/entity deploy` -- are her setup, named at the call site. The
//! bot's own GM is its driver's and is in its act log, before its first
//! undock (T9's rule, checked here too).

use super::common::{
    SYSTEM, config, dump, gm, gm_only_in_setup, login, ok, pilot, serve_with_chat, until,
};
use eve_bot::brain::{ActKind, Role};
use eve_bot::driver::{self, BotConfig, Handle, Pace, Verdict};
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const ESCORT: i64 = 90_000_001;
const ALICE: i64 = 90_000_002;
const ALICE_SHIP: i64 = 100_000_002;
/// The no-bundle rat `/entity deploy` knows, invented (`npc::FIXTURE_RAT`,
/// CP61).
const HIJACKER: i64 = eve_server::npc::FIXTURE_RAT.type_id;
/// A Rifter **[V-data]**: Alice's hull, so the NPC (which ignores capsules)
/// has her to shoot -- as in `tests/npc.rs`.
const RIFTER: i64 = 587;

/// The escort (it sets itself up in a Rifter, so two Rifter balls fly); Alice in a Rifter. `chat` turns the server's
/// chat on, on a port the kernel picks (port 0); the second half is that
/// port, read back from the bound listener.
fn start(chat: bool) -> (SocketAddr, Option<u16>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ESCORT, "Escort1", 100_000_001),
        eve_server::model::Character {
            ship_type_id: RIFTER,
            ..pilot(ALICE, "Alice", ALICE_SHIP)
        },
    ];
    config.chat = chat.then_some(eve_server::chat::ChatConfig {
        port: 0,
        advertise: None,
    });
    serve_with_chat(config)
}

fn escort(addr: SocketAddr, chat_port: u16) -> Handle {
    let mut config = BotConfig::new("Escort1", addr, Pace::Fast).with_role(Role::Escort);
    config.chat_port = chat_port;
    driver::spawn(config).0
}

/// Alice makes a fleet: the fleet object she invites through.
fn create_fleet(alice: &mut Conn) -> eve_bot::ObjectRef {
    let created = alice
        .call("fleetObjectHandler", "CreateFleet", vec![])
        .expect("CreateFleet");
    let fleet = eve_bot::object_refs(&created.value().expect("a reference"))
        .into_iter()
        .next()
        .expect("an object");
    alice
        .call_on(
            &fleet,
            "Init",
            vec![PyValue::Int(super::common::FLEET_SHIP)],
            vec![],
        )
        .expect("Init");
    fleet
}

/// Alice invites the bot; `true` once she hears it join.
fn invite(alice: &mut Conn, fleet: &eve_bot::ObjectRef, who: i64) -> bool {
    alice
        .call_on(
            fleet,
            "Invite",
            vec![
                PyValue::Int(who),
                PyValue::None,
                PyValue::None,
                PyValue::None,
            ],
            vec![],
        )
        .expect("Invite");
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        for e in alice.pump(Duration::from_millis(250)).expect("reads") {
            if matches!(e, Event::Push(n, _) if n == "OnFleetJoin") {
                return true;
            }
        }
    }
    false
}

/// Read Alice's pushes for `within`, or until `done` says so.
fn watch(alice: &mut Conn, within: Duration, mut done: impl FnMut(&Event, &Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        for e in alice.pump(Duration::from_millis(250)).expect("reads") {
            if done(&e, alice) {
                return true;
            }
        }
    }
    false
}

/// **The escort kills an NPC that is shooting its boss, and returns to
/// her.** Alice -- a plain connection in a Rifter -- undocks, goes more
/// than a warp from the station (her `/tp`), deploys a Hijacker beside
/// herself, and waits until its gun is on her. Only then does she invite
/// the bot, which by then has set itself up and undocked. Within the
/// deadline: the bot warps to her by character id, its guns are on the NPC,
/// the NPC's ball leaves Alice's park (nobody else is shooting it), and
/// after that the bot orders an approach to **her** ship. The server's chat
/// is off, and the bot says so once and fights anyway.
#[test]
fn an_escort_kills_the_npc_shooting_its_boss_and_returns_to_her() {
    // Chat is off, so the bot never dials: its default port is never used.
    let (addr, _) = start(false);
    let bot = escort(addr, eve_bot::chat::PORT);
    assert!(
        until(Duration::from_secs(60), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Undock))),
        "the bot sets itself up and undocks:\n{}",
        dump(&bot)
    );

    // Alice's own setup: undock, go far, deploy a rat beside herself.
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    gm(&mut alice, "/tp 1000");
    let _ = alice.collect_pushes(Duration::from_millis(500));
    gm(&mut alice, &format!("/entity deploy 1 {HIJACKER}"));
    let mut npc = None;
    let shooting_her = watch(&mut alice, Duration::from_secs(30), |e, c| {
        if npc.is_none() {
            npc = c
                .world()
                .balls()
                .find(|b| b.type_id() == Some(HIJACKER))
                .map(|b| b.ball_id);
        }
        matches!(e, Event::Push(n, a)
            if n == "OnSpecialFX"
                && npc.is_some()
                && a.first() == npc.map(PyValue::Int).as_ref()
                && a.get(3) == Some(&PyValue::Int(ALICE_SHIP))
                && a.get(7) == Some(&PyValue::Int(1)))
    });
    let npc = npc.expect("the deploy put a Hijacker in Alice's park");
    assert!(shooting_her, "the NPC's gun is on Alice");

    let fleet = create_fleet(&mut alice);
    assert!(
        invite(&mut alice, &fleet, ESCORT),
        "Alice is told the bot joined:\n{}",
        dump(&bot)
    );

    let dead = watch(&mut alice, Duration::from_secs(150), |_, c| {
        c.world().get(npc).is_none()
    });
    assert!(dead, "the NPC dies:\n{}", dump(&bot));

    let log = bot.log();
    let warped = log
        .iter()
        .position(|e| ok(e, ActKind::WarpToMember) && e.ids == vec![ALICE]);
    assert!(warped.is_some(), "the bot warped to Alice:\n{}", dump(&bot));
    let fired = log
        .iter()
        .rposition(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&npc));
    assert!(
        fired.is_some(),
        "the bot's guns were on the NPC:\n{}",
        dump(&bot)
    );
    assert!(
        warped < fired,
        "warped to Alice, then fired:\n{}",
        dump(&bot)
    );

    // Afterwards: back to her ship. The bot orders an approach -- or, when
    // the wreck was already within `ESCORT_RANGE` of her (where the NPC
    // died is a matter of timing: it orbits her), it has nothing to
    // approach and waits there, which is also back with her. The test
    // accepts either and reads the distance from Alice's own world.
    let fired = fired.expect("checked");
    let mut gap = None;
    let mut returned = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !returned && Instant::now() < deadline {
        let _ = alice.pump(Duration::from_millis(250)).expect("reads");
        returned = {
            if bot.log()[fired..]
                .iter()
                .any(|e| ok(e, ActKind::Follow) && e.ids == vec![ALICE_SHIP])
            {
                true
            } else {
                let w = alice.world_now();
                let (Some(a), Some(b)) = (
                    w.get(ALICE_SHIP),
                    w.balls()
                        .find(|b| b.type_id() == Some(RIFTER) && b.ball_id != ALICE_SHIP),
                ) else {
                    continue;
                };
                let d = (0..3)
                    .map(|i| (a.motion.position[i] - b.motion.position[i]).powi(2))
                    .sum::<f64>()
                    .sqrt()
                    - a.radius
                    - b.radius;
                gap = Some(d);
                d <= eve_bot::escort::ESCORT_RANGE
            }
        };
    }
    assert!(
        returned,
        "the bot returns to Alice after the kill (gap {gap:?}):\n{}",
        dump(&bot)
    );

    // Chat off: one `chat` entry saying so, and no line said.
    let log = bot.log();
    assert!(
        log.iter()
            .any(|e| e.kind == ActKind::Chat && matches!(e.verdict, Verdict::NotSent(_))),
        "the bot notes the chat is off:\n{}",
        dump(&bot)
    );
    assert!(!log.iter().any(|e| ok(e, ActKind::Say)));
    gm_only_in_setup(&bot);
    bot.stop();
}

/// **WARP-MIN: an escort whose boss is a few km away approaches her; it
/// never warps to her.** Both undock and sit at the station; Alice invites the
/// bot. Within the deadline the bot orders a follow to her ship, and its log
/// holds no successful warp to her (the server would refuse one: the landing
/// point is inside the minimum warp distance).
#[test]
fn an_escort_beside_its_boss_approaches_instead_of_warping() {
    let (addr, _) = start(false);
    let bot = escort(addr, eve_bot::chat::PORT);
    assert!(
        until(Duration::from_secs(60), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Undock))),
        "the bot undocks:\n{}",
        dump(&bot)
    );
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    let fleet = create_fleet(&mut alice);
    assert!(
        invite(&mut alice, &fleet, ESCORT),
        "joined:\n{}",
        dump(&bot)
    );
    // Alice flies off a few km (full throttle, a fixed heading): the bot is
    // then a few km from its boss, inside the minimum warp distance.
    alice.goto_direction([1.0, 0.0, 0.0]).expect("heading");
    alice.set_speed_fraction(1.0).expect("throttle");
    let followed = until(Duration::from_secs(90), || {
        bot.log()
            .iter()
            .any(|e| ok(e, ActKind::Follow) && e.ids == vec![ALICE_SHIP])
    });
    let log = bot.log();
    assert!(
        !log.iter().any(|e| ok(e, ActKind::WarpToMember)
            || (ok(e, ActKind::WarpToItem) && e.ids == vec![ALICE_SHIP])),
        "the bot warped to a boss beside it:\n{}",
        dump(&bot)
    );
    assert!(followed, "the bot approaches Alice:\n{}", dump(&bot));
    bot.stop();
}

/// **With the server's chat on, the bot speaks in the fleet room.** Alice
/// logs in to chat the client's way (`chatAuthenticationService` token,
/// `ResyncSystemChannelAccess`), invites the bot, joins the fleet's room,
/// and hears a groupchat line **from the bot's character** in it. The words
/// are [A] and not asserted.
#[test]
fn with_chat_on_an_escort_says_a_line_in_fleet_chat() {
    let (addr, port) = start(true);
    let port = port.expect("chat is bound");
    let bot = escort(addr, port);
    assert!(
        until(Duration::from_secs(60), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Chat))),
        "the bot's chat is up:\n{}",
        dump(&bot)
    );

    let mut alice = login(addr, "Alice");
    let host = alice
        .call("XmppChatMgr", "Hostname", vec![])
        .expect("Hostname")
        .value()
        .and_then(|v| match v {
            PyValue::Bytes(b) => Some(String::from_utf8_lossy(&b).into_owned()),
            PyValue::Unicode(s) => Some(s),
            _ => None,
        })
        .expect("a chat host");
    let token = alice
        .call(
            "chatAuthenticationService",
            "GetAuthenticationToken",
            vec![],
        )
        .expect("a token")
        .value()
        .and_then(|v| match v {
            PyValue::Bytes(b) => Some(String::from_utf8_lossy(&b).into_owned()),
            PyValue::Unicode(s) => Some(s),
            _ => None,
        })
        .expect("a token string");
    let mut chat = eve_xmpp::Client::login(SocketAddr::new(addr.ip(), port), &host, ALICE, &token)
        .expect("Alice logs in to chat");

    // Alice is in her fleet's room before the bot is in her fleet: its
    // hello comes once, on joining.
    let fleet = create_fleet(&mut alice);
    let rooms = alice
        .call("XmppChatMgr", "ResyncSystemChannelAccess", vec![])
        .expect("rooms")
        .value();
    let Some(PyValue::List(rooms)) = rooms else {
        panic!("a list of rooms")
    };
    let fleet_room = rooms
        .iter()
        .filter_map(|r| match r {
            PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            PyValue::Unicode(s) => Some(s.clone()),
            _ => None,
        })
        .find(|r| r.starts_with("fleet_"))
        .expect("Alice is in a fleet room");
    chat.join(&fleet_room).expect("joins");
    assert!(
        invite(&mut alice, &fleet, ESCORT),
        "the bot joins:\n{}",
        dump(&bot)
    );

    let from_bot = format!("{}/{ESCORT}", chat.room_jid(&fleet_room));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut heard = false;
    while !heard && Instant::now() < deadline {
        let _ = alice.pump(Duration::from_millis(50));
        if let Some(stanza) = chat
            .recv_timeout(Duration::from_millis(250))
            .expect("chat reads")
        {
            heard = stanza.name == "message"
                && stanza.attr("type") == Some("groupchat")
                && stanza.attr("from") == Some(from_bot.as_str());
        }
    }
    assert!(
        heard,
        "a line from the bot in {fleet_room}:\n{}",
        dump(&bot)
    );
    assert!(bot.log().iter().any(|e| ok(e, ActKind::Say)));
    bot.stop();
}
