//! P8b over a real socket: whole bots -- brain, driver, verbs -- playing the
//! mining loop against an in-process server.
//!
//! `docs/lanes/bots.md` §7, T4-T10 (T1 is `brain.rs`'s tables, T2 is
//! `park.rs`'s, T3 is `tests/verbs.rs`). The same limitation as every test in
//! this crate: both ends marshal with `eve-marshal`, so this proves
//! behaviour, not bytes.
//!
//! **No `[A]` value is an expected value here.** Every bot runs at
//! [`Pace::Fast`]; the assertions are about kinds, ids, types and order,
//! never a delay, a range, a yield or a count the policy chose. Deadlines are
//! generous ceilings ("within a minute"), not durations.
//!
//! **Synthetic furniture**: rocks with made-up ids at made-up places and a
//! fixture radius law, ore type 1230 (Veldspar [V-data]) -- the no-client-
//! data rule, as in `tests/jump.rs`. GM slash commands appear in these tests
//! only as a non-bot's setup, named at the call site; a bot's own GM is the
//! driver's and is in its act log (T9).

use super::common::{
    ROCK, SYSTEM, VELDSPAR, config, dump, gm, gm_only_in_setup, login, ok, pilot, rock, serve,
    until,
};
use eve_bot::Event;
use eve_bot::brain::{ActKind, Place};
use eve_bot::driver::{self, BotConfig, Entry, Handle, Pace};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const MINER1: i64 = 90_000_001;
const ALICE: i64 = 90_000_004;
/// A Merlin, a Miner I and the Mining skill, from the build's static data
/// [V-data] -- for a non-bot's setup only.
const MERLIN: i64 = 603;
const MINER_I: i64 = 483;
const MINING: i64 = 3386;
/// Small Shield Extender I [V-data]: cargo that is not ore, 5 m3 a unit,
/// so thirty fill a Merlin's 150 m3 hold exactly.
const JUNK: i64 = 377;

/// One rock: `(quantity, position)`. Ids are `ROCK + i`.
type RockSpec = (i64, [f64; 3]);

/// Rocks a little more than 100 km from the station (at the origin), well
/// inside one warp: so a bot approaches, it does not warp.
fn belt(n: usize, quantity: i64) -> Vec<RockSpec> {
    (0..n)
        .map(|i| (quantity, [(i as f64 - 1.0) * 20_000.0, 0.0, 120_000.0]))
        .collect()
}

fn start(rocks: &[RockSpec]) -> SocketAddr {
    let mut config = config();
    // Marked bots: a bot learns that its warp landed from the arrival batch
    // (the server sends a human pilot only the massive demotion).
    let bots = |mut c: eve_server::model::Character| {
        c.is_bot = true;
        c
    };
    config.sandbox.characters = vec![
        bots(pilot(MINER1, "Miner1", 100_000_001)),
        bots(pilot(MINER1 + 1, "Miner2", 100_000_002)),
        bots(pilot(MINER1 + 2, "Miner3", 100_000_003)),
        pilot(ALICE, "Alice", 100_000_004),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks(
        rocks
            .iter()
            .enumerate()
            .map(|(i, &(quantity, position))| rock(i, quantity, position)),
    );
    serve(config)
}

fn bot(addr: SocketAddr, name: &str) -> Handle {
    driver::spawn(BotConfig::new(name, addr, Pace::Fast)).0
}

/// The rock of every successful `Mine`, in order.
fn mined(log: &[Entry]) -> Vec<i64> {
    log.iter()
        .filter(|e| ok(e, ActKind::Mine))
        .filter_map(|e| e.ids.get(1).copied())
        .collect()
}

/// **T4.** One bot, a fresh character in a capsule: it sets itself up,
/// undocks, binds, locks the rock and starts the laser -- the
/// `OnGodmaShipEffect(67, start=1)` is the `Mine` act's own evidence -- and
/// within a cycle and ten seconds its hold holds ore **of the rock's type**.
#[test]
fn one_bot_mines_a_rock_into_its_hold() {
    let addr = start(&belt(1, 100_000));
    let miner = bot(addr, "Miner1");
    assert!(
        until(Duration::from_secs(90), || mined(&miner.log())
            .contains(&ROCK)),
        "the laser starts on the rock:\n{}",
        dump(&miner)
    );
    let cycle = Duration::from_millis(
        eve_bot::modules::find(None, MINER_I)
            .expect("Miner I")
            .0
            .duration_ms() as u64,
    );
    let has_ore = || {
        miner.observation().is_some_and(|o| {
            o.cargo
                .iter()
                .any(|r| r.type_id == VELDSPAR && r.quantity > 0)
        })
    };
    assert!(
        until(cycle + Duration::from_secs(10), has_ore),
        "ore in the hold after a cycle: {:?}\n{}",
        miner.observation().map(|o| o.cargo),
        dump(&miner)
    );
    let log = miner.log();
    let kinds: Vec<ActKind> = log
        .iter()
        .filter(|e| e.verdict.is_ok())
        .map(|e| e.kind)
        .collect();
    let first = |k| kinds.iter().position(|&x| x == k);
    assert!(
        first(ActKind::Board) < first(ActKind::Undock)
            && first(ActKind::Undock) < first(ActKind::Bind)
            && first(ActKind::Bind) < first(ActKind::Lock)
            && first(ActKind::Lock) < first(ActKind::Mine),
        "board, undock, bind, lock, mine, in that order:\n{}",
        dump(&miner)
    );
    gm_only_in_setup(&miner);
    miner.stop();
}

/// **T5.** Two tiny rocks: the bot mines one out, takes the other, and with
/// the belt empty goes home, docks, unloads and undocks again. An observer
/// in space sees both rocks removed. **No `rebind_ballpark` anywhere** --
/// the driver has no call to it -- so every bind after the first is the
/// park release on docking at work (§4.1.3).
#[test]
fn a_bot_mines_a_belt_out_then_docks_unloads_and_goes_again() {
    // Less than one cycle's yield each, so each goes in one cycle.
    let addr = start(&belt(2, 20));
    let mut watcher = login(addr, "Alice");
    watcher.undock().expect("the watcher undocks");
    watcher.enter_space(SYSTEM).expect("the watcher's state");
    let miner = bot(addr, "Miner1");

    let mut removed: Vec<i64> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(150);
    let done = |log: &[Entry]| {
        let rocks = mined(log);
        let unloaded = log.iter().position(|e| ok(e, ActKind::Unload));
        rocks.contains(&ROCK)
            && rocks.contains(&(ROCK + 1))
            && unloaded.is_some_and(|at| log[at..].iter().any(|e| ok(e, ActKind::Undock)))
    };
    while Instant::now() < deadline && !(done(&miner.log()) && removed.len() == 2) {
        for e in watcher.pump(Duration::from_millis(250)).expect("reads") {
            if let Event::Push(n, a) = e
                && n == "DoDestinyUpdate"
            {
                for (_, f, ops) in eve_bot::read_action_list(&a) {
                    if f == "RemoveBalls"
                        && let Some(PyValue::List(ids)) = ops.first()
                    {
                        for id in ids {
                            if let PyValue::Int(id) = id
                                && eve_bot::park::is_rock(*id)
                                && !removed.contains(id)
                            {
                                removed.push(*id);
                            }
                        }
                    }
                }
            }
        }
    }
    let log = miner.log();
    assert!(
        done(&log),
        "both rocks mined, then unload and undock:\n{}",
        dump(&miner)
    );
    removed.sort_unstable();
    assert_eq!(
        removed,
        vec![ROCK, ROCK + 1],
        "both rocks left everyone's park"
    );

    // The order: the second rock only after the first was mined out, and
    // the dock only after both.
    let rocks = mined(&log);
    assert_ne!(rocks[0], rocks[rocks.len() - 1], "a different rock next");
    let last_mine = log
        .iter()
        .rposition(|e| ok(e, ActKind::Mine))
        .expect("mined");
    let dock = log
        .iter()
        .position(|e| ok(e, ActKind::Dock))
        .expect("docked");
    assert!(last_mine < dock, "docked after the belt was empty");

    // Unloaded: the hangar has the ore, the hold has none.
    let unloaded = || {
        miner.observation().is_some_and(|o| {
            o.hangar.iter().any(|r| r.type_id == VELDSPAR)
                && !o.cargo.iter().any(|r| r.type_id == VELDSPAR)
        }) || {
            // Already undocked again: the hangar is not listed in space,
            // so read the hold only.
            miner
                .observation()
                .is_some_and(|o| !matches!(o.place, Place::Docked { .. }) && o.cargo.is_empty())
        }
    };
    assert!(unloaded(), "{:?}", miner.observation());
    gm_only_in_setup(&miner);
    miner.stop();
}

/// **T6.** Three bots, three rocks: each one's laser runs, on **three
/// distinct rocks**. Distinctness is our policy (`brain::pick_rock` and the
/// yield rule), stated as a requirement, not as a claim about EVE.
#[test]
fn three_bots_mine_three_different_rocks() {
    let addr = start(&belt(3, 100_000));
    let bots: Vec<Handle> = ["Miner1", "Miner2", "Miner3"]
        .into_iter()
        .map(|n| bot(addr, n))
        .collect();
    // The rock each bot's laser is on now, from its own observation.
    let on = |h: &Handle| {
        h.observation()
            .and_then(|o| o.miner)
            .filter(|l| l.active)
            .and_then(|l| l.target)
    };
    let apart = || {
        let mut rocks: Vec<i64> = bots.iter().filter_map(on).collect();
        rocks.sort_unstable();
        rocks.dedup();
        rocks.len() == 3
    };
    assert!(
        until(Duration::from_secs(150), apart),
        "three lasers on three rocks: {:?}\n{}",
        bots.iter().map(on).collect::<Vec<_>>(),
        bots.iter().map(dump).collect::<Vec<_>>().join("\n--\n")
    );
    for h in &bots {
        assert!(!mined(&h.log()).is_empty(), "{} sent a Mine", h.name);
        gm_only_in_setup(h);
        h.stop();
    }
}

/// **T7.** The connection drops mid-mining. The supervisor reconnects,
/// selects **by name** again, rebuilds its observation from the server
/// before it acts, and mines again.
///
/// **A characterisation test** for `docs/lanes/bots.md` §4.3's
/// `[unknown]` -- what the server makes of a character whose socket
/// dropped in space. Whatever place the server returns is printed, not
/// asserted: the requirement is only that the bot copes with it.
#[test]
fn a_dropped_bot_reconnects_rebuilds_and_mines_again() {
    let addr = start(&belt(1, 100_000));
    let miner = bot(addr, "Miner1");
    assert!(
        until(Duration::from_secs(90), || !mined(&miner.log()).is_empty()),
        "mining before the drop:\n{}",
        dump(&miner)
    );
    let before = miner.log().len();
    miner.kick();

    let resumed = || {
        let log = miner.log();
        log[before.min(log.len())..]
            .iter()
            .any(|e| ok(e, ActKind::Mine))
    };
    assert!(
        until(Duration::from_secs(120), resumed),
        "mining again after the drop:\n{}",
        dump(&miner)
    );
    let log = miner.log();
    let after = &log[before..];
    let lost = after
        .iter()
        .position(|e| e.kind == ActKind::Lost)
        .expect("the drop is logged");
    let next: Vec<ActKind> = after[lost + 1..].iter().take(2).map(|e| e.kind).collect();
    assert_eq!(
        next,
        vec![ActKind::Connect, ActKind::Observe],
        "a fresh login, then a fresh observation, before any act:\n{}",
        dump(&miner)
    );
    let observed = &after[lost + 2];
    assert_eq!(
        after[lost + 1].ids,
        vec![MINER1],
        "selected itself by name again"
    );
    println!(
        "T7: after a drop in space the server returned: {}",
        observed.what
    );
    gm_only_in_setup(&miner);
    miner.stop();
}

/// **T8.** A friend -- a plain connection, not a bot -- creates a fleet and
/// invites Miner1; the bot joins (Alice is told, `OnFleetJoin`). Alice then
/// goes more than a warp away (her own `/tp`), and the bot warps to her **by
/// character id**, which Alice's park sees as a `WarpTo` of the bot's ship.
#[test]
fn a_friend_invites_a_bot_and_it_follows_her() {
    let addr = start(&belt(1, 100_000));
    let miner = bot(addr, "Miner1");
    assert!(
        until(Duration::from_secs(30), || miner
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Observe)),
        "the bot is on line:\n{}",
        dump(&miner)
    );

    let mut alice = login(addr, "Alice");
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
    let mut joined = false;
    while !joined && Instant::now() < deadline {
        for e in alice.pump(Duration::from_millis(250)).expect("reads") {
            joined |= matches!(e, Event::Push(n, _) if n == "OnFleetJoin");
        }
    }
    assert!(joined, "Alice is told the bot joined:\n{}", dump(&miner));

    // Alice's own setup: undock, and go far.
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    gm(&mut alice, "/tp 1000");

    let bot_ship = || miner.observation().and_then(|o| o.ship).map(|s| s.ship_id);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = false;
    while !seen && Instant::now() < deadline {
        for e in alice.pump(Duration::from_millis(250)).expect("reads") {
            if let Event::Push(n, a) = e
                && n == "DoDestinyUpdate"
                && let Some(ship) = bot_ship()
            {
                seen |= eve_bot::read_action_list(&a)
                    .iter()
                    .any(|(_, f, ops)| f == "WarpTo" && ops.first() == Some(&PyValue::Int(ship)));
            }
        }
    }
    assert!(seen, "Alice sees the bot warp:\n{}", dump(&miner));
    assert!(
        until(Duration::from_secs(5), || miner.log().iter().any(|e| ok(
            e,
            ActKind::WarpToMember
        ) && e.ids
            == vec![ALICE])),
        "the bot warped to Alice by character id:\n{}",
        dump(&miner)
    );
    gm_only_in_setup(&miner);
    miner.stop();
}

/// **T9, the other half.** With GM setup off and nothing in the hangar, the
/// bot says `MISSING` and sends **no** slash at all.
#[test]
fn without_gm_a_bot_with_no_ship_says_missing_and_never_slashes() {
    let addr = start(&belt(1, 100_000));
    let mut config = BotConfig::new("Miner1", addr, Pace::Fast);
    config.policy.gm_setup = false;
    let (miner, _) = driver::spawn(config);
    assert!(
        until(Duration::from_secs(30), || miner
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Missing)),
        "MISSING is logged:\n{}",
        dump(&miner)
    );
    let log = miner.log();
    assert!(
        !log.iter()
            .any(|e| matches!(e.kind, ActKind::Slash | ActKind::GmSetup)),
        "no GM at all:\n{}",
        dump(&miner)
    );
    assert!(
        !log.iter().any(|e| e.kind == ActKind::Undock),
        "it stays in"
    );
    miner.stop();
}

/// **T10.** No hot loop on a refusal the brain cannot foresee. The bot's
/// hold is full of shield extenders -- cargo, not ore, so the brain neither
/// unloads it nor knows the hold is full -- and every `Activate` is refused
/// (`NotEnoughCargoSpace`, `mining::check_activation`). Until the bot settles
/// (it says `MISSING` and idles) or a minute passes, never more than three
/// `Mine` refusals in a row before a recovery act (going home, docking,
/// `MISSING`, reconnecting).
///
/// `docs/lanes/bots.md` §7 asked for a rock outside the laser's range; the
/// bot dead-reckons with the server's own law and range table, so it cannot
/// be made to misjudge range without lying to it. A full hold is the same
/// property -- a persistent refusal -- reached honestly.
#[test]
fn a_refused_laser_does_not_hammer_the_server() {
    let addr = start(&belt(1, 100_000));
    {
        // The pilot's own setup, as a plain connection before the bot runs.
        let mut c = login(addr, "Miner1");
        gm(&mut c, &format!("/giveskill {MINING} 1"));
        gm(&mut c, "/createitem merlin");
        gm(&mut c, &format!("/createitem {MINER_I}"));
        gm(&mut c, &format!("/createitem {JUNK} 30"));
        let hangar = c.hangar_items().expect("lists");
        let find = |t: i64| {
            hangar
                .iter()
                .find(|r| r.type_id == t)
                .unwrap_or_else(|| panic!("{t} in the hangar: {hangar:?}"))
                .item_id
        };
        c.board(find(MERLIN)).expect("boards");
        c.fit_auto(&[find(MINER_I)]).expect("fits");
        c.load_cargo(find(JUNK), None).expect("the hold takes it");
        assert!(
            c.cargo().expect("lists").iter().any(|r| r.type_id == JUNK),
            "the hold is full of shield extenders"
        );
    }
    let miner = bot(addr, "Miner1");
    let refused = || {
        miner
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Mine && !e.verdict.is_ok())
    };
    assert!(
        until(Duration::from_secs(60), refused),
        "the laser is refused:\n{}",
        dump(&miner)
    );
    // Wait for what the checks below need, not for a fixed time (V4 S2).
    // After the refusal the bot either settles -- it says what it is
    // missing and idles -- or it hammers, and four refused `Mine`s is
    // enough for "a fourth refusal in a row" to show. Either ends the wait;
    // a bot that does neither runs out the ceiling, and the assertions
    // below judge the log as it is.
    let settled = || {
        let log = miner.log();
        let refused = |e: &Entry| e.kind == ActKind::Mine && !e.verdict.is_ok();
        let Some(first) = log.iter().position(refused) else {
            return false;
        };
        let after = &log[first..];
        after.iter().any(|e| e.kind == ActKind::Missing)
            || after.iter().filter(|e| refused(e)).count() > 3
    };
    let _ = until(Duration::from_secs(60), settled);
    let log = miner.log();
    let mut run = 0;
    let mut recoveries = 0;
    for e in &log {
        match e.kind {
            ActKind::Mine if !e.verdict.is_ok() => {
                run += 1;
                assert!(run <= 3, "a fourth refusal in a row:\n{}", dump(&miner));
            }
            ActKind::Mine => run = 0,
            ActKind::Dock
            | ActKind::Reconnect
            | ActKind::WarpToItem
            | ActKind::Follow
            | ActKind::Missing
                if run > 0 =>
            {
                run = 0;
                recoveries += 1;
            }
            _ => {}
        }
    }
    assert!(recoveries > 0, "a recovery act came:\n{}", dump(&miner));
    miner.stop();
}

/// **The hold-full path end to end**: mine until the hold is full, then
/// warp home, dock and unload. Not run by default: a Merlin's hold is many
/// cycles, minutes of wall clock (`docs/lanes/bots.md` §7, "not tested
/// here"); its decision half is T1's `a_full_hold_goes_home_warp_far_dock_near`.
/// `tools/dev.sh cargo test -p eve-bot -- --ignored`.
#[test]
#[ignore = "a soak: a Merlin's hold takes minutes of 15 s cycles to fill"]
fn soak_a_bot_fills_its_hold_and_unloads_it() {
    let addr = start(&belt(1, 10_000_000));
    let miner = bot(addr, "Miner1");
    let full_then_unloaded = || {
        let log = miner.log();
        let dock = log.iter().position(|e| ok(e, ActKind::Dock));
        dock.is_some_and(|at| log[at..].iter().any(|e| ok(e, ActKind::Unload)))
    };
    assert!(
        until(Duration::from_secs(900), full_then_unloaded),
        "a full hold goes home and unloads:\n{}",
        dump(&miner)
    );
    assert!(
        miner
            .observation()
            .is_some_and(|o| o.hangar.iter().any(|r| r.type_id == VELDSPAR)),
        "the ore is in the hangar"
    );
    miner.stop();
}
