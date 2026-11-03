//! P8a over a real socket: the player verbs, each against an in-process
//! server, each asserting the evidence the real client waits for.
//!
//! `docs/lanes/bots.md` §7 (T3, plus one call-and-evidence test per verb).
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **No `[A]` value is an expected value here.** Ranges, cycle times and
//! yields are the server's; these tests assert kinds, ids and flags.
//!
//! **Synthetic furniture**: one rock with a made-up id and a fixture radius
//! law (the no-client-data rule, as in `tests/jump.rs`). GM slash commands
//! appear only as setup, before the verb under test, and are called by name
//! at the call site so they are visible (`docs/lanes/bots.md` §2.1).

use super::common::{ROCK, SYSTEM, config, gm, login, pilot, rock, serve};
use eve_bot::{Conn, Error, LockState};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const MINER1: i64 = 90_000_001;
const ALICE: i64 = 90_000_002;
/// A Merlin and a Miner I, type ids from the build's static data [V-data].
const MERLIN: i64 = 603;
const MINER_I: i64 = 483;
/// Mining, the skill a Miner I needs [V-data].
const MINING: i64 = 3386;

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(MINER1, "Miner1", 100_000_001),
        pilot(ALICE, "Alice", 100_000_002),
    ];
    config.sandbox.rocks =
        eve_server::asteroid::Field::from_rocks([rock(0, 100_000, [0.0, 0.0, 120_000.0])]);
    serve(config)
}

/// Setup: a Merlin and a Miner I in the hangar, the skill to use it, then
/// board and fit with the verbs. Returns `(merlin, fitted miner)`.
fn boarded_and_fitted(c: &mut Conn) -> (i64, i64) {
    gm(c, &format!("/giveskill {MINING} 1"));
    gm(c, "/createitem merlin");
    gm(c, &format!("/createitem {MINER_I}"));
    let hangar = c.hangar_items().expect("the hangar lists");
    let merlin = hangar
        .iter()
        .find(|r| r.type_id == MERLIN)
        .expect("a Merlin in the hangar")
        .item_id;
    let miner = hangar
        .iter()
        .find(|r| r.type_id == MINER_I)
        .expect("a Miner I in the hangar")
        .item_id;
    c.board(merlin).expect("boards");
    let fitted = c.fit_auto(&[miner]).expect("fits");
    (merlin, fitted[0].item_id)
}

#[test]
fn a_bot_picks_itself_off_the_select_screen_by_name() {
    let addr = start();
    let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    c.handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    let names: Vec<String> = c
        .characters()
        .expect("GetCharacterSelectionData")
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["Miner1".to_owned(), "Alice".to_owned()]);

    let outcome = c.select_character_named("Alice").expect("selects");
    assert!(outcome.session_changed);
    assert_eq!(c.session().char_id(), Some(ALICE));
    assert_eq!(c.session().ship_id(), Some(100_000_002));
    assert!(c.session().station_id().is_some(), "docked at login");

    let mut nobody = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    nobody
        .handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    match nobody.select_character_named("Nobody") {
        Err(Error::Protocol(why)) => assert!(why.contains("Nobody"), "{why}"),
        other => panic!("expected a protocol error, got {other:?}"),
    }
}

/// **T3.** Board, fit, online -- and the session, the listings and the dogma
/// row each say so.
#[test]
fn a_bot_boards_fits_and_onlines_the_player_way() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    let (merlin, miner) = boarded_and_fitted(&mut c);

    assert_eq!(c.session().ship_id(), Some(merlin), "the shipid change");
    let fitted = c.fitted().expect("the ship lists");
    let row = fitted
        .iter()
        .find(|r| r.item_id == miner)
        .expect("the miner is in the ship");
    assert_eq!(row.type_id, MINER_I);
    assert!(
        (27..=34).contains(&row.flag_id),
        "a high slot: {}",
        row.flag_id
    );
    assert!(row.singleton, "fitted modules are assembled");

    // Online either way: by the server on fitting, or by this call.
    c.set_online(miner).expect("online");
    assert!(c.is_online(miner).expect("ItemGetInfo"));
}

/// Undock, bind, approach the rock, lock it, start the laser: the
/// `OnGodmaShipEffect` with `start=1` is the evidence (`mining.md` §1.1).
#[test]
fn a_bot_locks_a_rock_and_starts_its_laser() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    let (_, miner) = boarded_and_fitted(&mut c);
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state");
    assert!(
        c.world().rocks().any(|r| r.ball_id == ROCK),
        "the rock is in the bot's park"
    );

    c.follow(ROCK, 1_000.0).expect("follows");
    c.request_state().expect("a fresh state");
    let ship = c.session().ship_id().expect("a ship");
    let gap = c
        .world_now()
        .surface_distance(ship, ROCK)
        .expect("both balls");
    assert!(
        gap < 15_000.0,
        "within any laser's reach after following: {gap}"
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut state = c.lock_target(ROCK).expect("AddTarget is answered");
    while state == LockState::Pending && Instant::now() < deadline {
        let pushes = c.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, eve_bot::Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(ROCK)))
        }) {
            state = LockState::Locked;
        }
    }
    assert_eq!(state, LockState::Locked);

    let effect = c.mine(miner, ROCK).expect("the laser starts");
    assert!(effect.start);
    assert_eq!(effect.item_id, miner);
    assert_eq!(effect.target, Some(ROCK));
    c.deactivate(miner, eve_bot::MINING_LASER.0)
        .expect("Deactivate is answered");
}

/// Dock, the client's way: refused locally from the undock point (outside
/// 2,500 m), sent once the bot has closed the distance, and the session
/// change releases the park -- so the next undock binds with
/// `bind_ballpark`, never `rebind_ballpark`.
#[test]
fn a_bot_docks_only_in_range_and_binds_again_after() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    let station = c.session().station_id().expect("docked");
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state");
    // Setup: far from the station, so the client's own gate is shut.
    gm(&mut c, "/tp 1000");
    c.request_state().expect("a fresh state");

    match c.dock(station) {
        Err(Error::NotSent(why)) => assert!(why.contains("2500"), "{why}"),
        other => panic!("1000 km out is outside the client's dock gate: {other:?}"),
    }

    c.follow(station, 0.0).expect("approaches");
    c.request_state().expect("a fresh state");
    c.dock(station).expect("docks");
    assert_eq!(c.session().station_id(), Some(station));
    assert!(c.ballpark().is_none(), "the park went with the dock");
    assert!(!c.world().has_state());

    c.undock().expect("undocks again");
    c.bind_ballpark(SYSTEM)
        .expect("a plain bind, because the dock released the park");
    assert!(c.world().has_state());
}

/// Hangar to hold and back (P5.12), both the client's drags: `load_cargo`
/// is the ship inventory's `Add(itemID, stationID, qty, flag=5)` and
/// `unload` the hangar's `Add(itemID, shipID, qty, flag=4)`
/// (`invControllers.py:267`, `docs/lanes/inventory.md` P5.12) **[V]**. Each
/// listing says so: the hold gains the stack and the hangar loses it, then
/// the other way round.
#[test]
fn a_bot_moves_cargo_between_hangar_and_hold() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    boarded_and_fitted(&mut c);
    gm(&mut c, &format!("/createitem {MINER_I}"));
    let spare = c
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == MINER_I)
        .expect("a spare Miner I")
        .item_id;

    c.load_cargo(spare, Some(1)).expect("loads");
    assert!(
        !c.hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.item_id == spare),
        "the hangar lost it"
    );
    let cargo = c.cargo().expect("the hold lists");
    let row = cargo
        .iter()
        .find(|r| r.type_id == MINER_I)
        .expect("in the hold");
    match c.load_cargo(spare, None) {
        Err(Error::NotSent(why)) => assert!(why.contains("not in the hangar"), "{why}"),
        other => panic!("nothing left to load: {other:?}"),
    }

    c.unload(row.item_id, None).expect("unloads");
    assert!(
        !c.cargo()
            .expect("lists")
            .iter()
            .any(|r| r.type_id == MINER_I),
        "the hold is empty of it"
    );
    assert!(
        c.hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.item_id == row.item_id && r.flag_id == eve_bot::FLAG_HANGAR),
        "the hangar has it"
    );
}

/// `RemoveTarget` on the `dogmaIM` moniker: the lock goes, and
/// `OnTarget('lost', rock)` is the evidence the client closes its box on.
#[test]
fn a_bot_unlocks_a_rock() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    boarded_and_fitted(&mut c);
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state");
    c.follow(ROCK, 1_000.0).expect("follows");
    c.request_state().expect("a fresh state");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut state = c.lock_target(ROCK).expect("AddTarget is answered");
    while state == LockState::Pending && Instant::now() < deadline {
        let pushes = c.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, eve_bot::Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(ROCK)))
        }) {
            state = LockState::Locked;
        }
    }
    assert_eq!(state, LockState::Locked);
    c.unlock_target(ROCK).expect("the lock goes");
    let targets = c.targets().expect("GetTargets");
    assert!(
        !format!("{:?}", targets.value()).contains(&ROCK.to_string()),
        "GetTargets no longer lists it: {:?}",
        targets.value()
    );
}

/// A friend invites the bot: the invite arrives as a push, the bot accepts
/// on the fleet's own moniker, and the `fleetid` session change is the
/// evidence. Then, both in space, the bot warps to its friend by
/// **character** id and the friend's park is told.
#[test]
fn a_bot_accepts_a_fleet_invite_and_warps_to_its_friend() {
    let addr = start();
    let mut alice = login(addr, "Alice");
    let mut bot = login(addr, "Miner1");

    // Alice is a plain connection here, not a bot: `fleetSvc.CreateFleet`'s
    // three calls (`fleetSvc.py:395-399`), then the menu's invite.
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
    let fleet_id = alice
        .session()
        .fleet_id()
        .expect("Alice's session has the fleet");
    alice
        .call_on(
            &fleet,
            "Invite",
            vec![
                PyValue::Int(MINER1),
                PyValue::None,
                PyValue::None,
                PyValue::None,
            ],
            vec![],
        )
        .expect("Invite");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut invite = None;
    while invite.is_none() && Instant::now() < deadline {
        for e in bot.pump(Duration::from_millis(500)).expect("reads") {
            if let eve_bot::Event::Push(n, a) = e {
                invite = invite.or(eve_bot::FleetInvite::from_push(&n, &a));
            }
        }
    }
    let invite = invite.expect("OnFleetInvite reaches the bot");
    assert_eq!(invite.fleet_id, fleet_id);
    assert_eq!(invite.inviter_id, ALICE);
    bot.accept_fleet_invite(fleet_id, super::common::FLEET_SHIP)
        .expect("joins");
    assert_eq!(bot.session().fleet_id(), Some(fleet_id));

    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    // Setup for Alice, who is not a bot: far enough to be a warp away.
    gm(&mut alice, "/tp 1000");
    bot.undock().expect("the bot undocks");
    bot.enter_space(SYSTEM).expect("the bot's state");
    let _ = alice.collect_pushes(Duration::from_millis(500));

    bot.warp_to_member(ALICE, 0.0)
        .expect("CmdWarpToStuff('char', ...)");
    let bot_ship = bot.session().ship_id().expect("a ship");
    assert!(
        bot.world_now()
            .get(bot_ship)
            .is_some_and(eve_bot::Tracked::warp_commanded),
        "the bot's own park has it warping"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while !seen && Instant::now() < deadline {
        for e in alice.pump(Duration::from_millis(500)).expect("reads") {
            if let eve_bot::Event::Push(n, a) = e
                && n == "DoDestinyUpdate"
            {
                seen |= eve_bot::read_action_list(&a).iter().any(|(_, f, ops)| {
                    f == "WarpTo" && ops.first() == Some(&PyValue::Int(bot_ship))
                });
            }
        }
    }
    assert!(seen, "Alice's park is told the bot is warping");
}
