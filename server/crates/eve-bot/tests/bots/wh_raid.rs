//! T3-SALVAGE over a real socket: raiders -- brain, driver, verbs -- dive
//! through a hole into a J-space system, clear its Sleeper anomaly, loot and
//! salvage the Sleeper's wreck for Ancient Salvage, and come out again.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): an invented null-sec
//! home system with one static (an invented type, an invented archetype-38
//! dungeon for it and for the K162), an invented J-space system of class
//! [`J_CLASS`], one invented Sleeper anomaly there with one spawn of the
//! server's hand ship (`npc::FIXTURE_RAT`) as the Sleeper, an invented
//! salvager and two invented Ancient Salvage types, a launcher and probe as
//! in `wh_dive.rs` (the launcher a mid-slot module here: the harness's Rifter
//! has three high slots, and the guns and the salvager take them). The hole is pinned to exist
//! (`Sites::with_hole_chance`). **No `[A]` value is an expected value
//! here**: the assertions are about kinds, systems and order; the policy's
//! cap floor ([`eve_bot::brain::Policy::raid_cap`]) stands in for a
//! cruiser, since the harness's hulls are not the combat ladder's.

use super::common::{SYSTEM, config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Handle, Pace};
use eve_bot::{probe, wh_raid};
use eve_server::celestial::Universe;
use eve_server::dungeon_data::{
    self, Dungeon, ENTRY_ANOMALY, ENTRY_SIGNATURE, Object, Room, Spawn,
};
use eve_server::npc_data::{self, Needs, Role};
use eve_server::population;
use eve_server::wormhole::{self, ARCHETYPE, K162};
use eve_server::wormhole_sites::{self, Family, Hull, Rank, SiteRow, Sites, SleeperRow};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const RAIDER1: i64 = 90_000_001;
const RAIDER2: i64 = 90_000_002;
const REGION: i64 = 19_055_001;
const CONSTELLATION: i64 = 29_055_001;
/// An invented J-space system (the `31xxxxxx` range is the rule's) and its
/// class.
const J_SYSTEM: i64 = 31_955_001;
const J_CLASS: i64 = 2;
/// The invented static, its dungeon ids, the anomaly and its structure.
const STATIC: i64 = 990_552;
const COMBAT_D: i64 = 999_560;
const STRUCTURE: i64 = 990_551;
const SALVAGER: i64 = 990_561;
/// Two invented Ancient Salvage types (group 966, the rule's).
const ANCIENT: [i64; 2] = [990_571, 990_572];
const LAUNCHER: i64 = 990_553_001;
const PROBE: i64 = 990_553_010;

fn types() -> String {
    let launcher_group = probe::GROUP_LAUNCHER;
    let probe_group = probe::GROUP_PROBE;
    let factor = probe::RANGE_FACTOR;
    let salvaging = eve_server::salvage::EFFECT_SALVAGING;
    let group = wh_raid::GROUP_ANCIENT_SALVAGE;
    format!(
        "{}\
type\t{LAUNCHER}\t{launcher_group}\t7\t1\t1\t5.0\t0.8\t0.0\t1.0\t0\t13,16\t30=1.0;50=10.0;604={probe_group}.0\tFixture Probe Launcher\n\
type\t{PROBE}\t{probe_group}\t8\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t281=3600000.0;1370=0.5;1371=50.0;1373={factor}\tFixture Site Probe\n\
type\t{SALVAGER}\t1122\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{salvaging}\t12,16,{salvaging}\t6=0.0;54=5000.0;73=1000.0;902=100.0\tFixture Salvager\n\
type\t{}\t{group}\t17\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Ancient Salvage A\n\
type\t{}\t{group}\t17\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Ancient Salvage B\n\
effect\t{salvaging}\t2\t0\t73\t6\t54\t0\t0\tsalvaging\teffects.Salvaging\n",
        eve_server::typedata::FIXTURE,
        ANCIENT[0],
        ANCIENT[1],
    )
}

/// The null-sec home (class 9 by its band) and one J-space system.
fn universe() -> Universe {
    let k = format!(
        "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    let j = format!(
        "{J_SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t{J_CLASS}\t0\t\t\tFixture J"
    );
    Universe::parse(
        &[k, j],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

/// The two hole dungeons (as `wh_dive.rs`) and the J-space anomaly: a
/// structure and one spawn at it.
fn dungeons(difficulty: i64) -> dungeon_data::Table {
    let hole = |i: usize, t: i64| {
        let object = |id, type_id| Object {
            room: 1,
            id,
            type_id,
            ..Object::default()
        };
        Dungeon {
            id: 999_550 + i as i64,
            archetype: ARCHETYPE,
            entry_type: Some(ENTRY_SIGNATURE),
            entry_object: Some(1),
            rooms: vec![Room {
                id: 1,
                ..Room::default()
            }],
            objects: vec![object(1, t), object(2, ENTRY_SIGNATURE)],
            ..Dungeon::default()
        }
    };
    let anomaly = Dungeon {
        id: COMBAT_D,
        archetype: eve_server::signal_tracker::ARCHETYPE_COMBAT,
        faction: Some(wormhole_sites::FACTION),
        // Two, not a Sleeper site's real three: the Agency's pick is by the
        // Rifter's own cap, which the policy floor does not move.
        difficulty: Some(difficulty),
        entry_type: Some(ENTRY_ANOMALY),
        name_id: Some(COMBAT_D + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![Object {
            room: 1,
            id: 1,
            type_id: STRUCTURE,
            pos: [0.0; 3],
            radius: Some(1_000.0),
            ..Object::default()
        }],
        spawns: vec![Spawn {
            room: 1,
            object: 1,
            entity: 71,
            ..Spawn::default()
        }],
        ..Dungeon::default()
    };
    let list = [hole(0, STATIC), hole(1, K162), anomaly];
    dungeon_data::Table {
        dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
        ..dungeon_data::Table::default()
    }
}

fn statics() -> wormhole::Table {
    wormhole::Table::new(vec![wormhole::Row {
        class: wormhole::CLASS_NULL,
        type_id: STATIC,
        dest_class: J_CLASS,
    }])
    .expect("one row")
}

/// The Sleeper is the server's hand ship; its wreck is the hand ship's own
/// (`npc::HAND_SMALL_WRECK`), which the salvage overlay gives the two
/// Ancient Salvage types.
fn sleeper_row() -> SleeperRow {
    SleeperRow {
        type_id: eve_server::npc::FIXTURE_RAT.type_id,
        group_id: eve_server::npc::FIXTURE_RAT.group_id,
        hull: Hull::Frigate,
        rank: Rank::Emergent,
        needs: Needs::TURRET,
        role: Role::Dps,
        wreck_type_id: Some(eve_server::npc::HAND_SMALL_WRECK),
    }
}

fn population_table() -> population::Table {
    population::Table::new(vec![population::Row {
        key: "wh_combat".to_owned(),
        band: eve_server::celestial::Band::Wormhole,
        archetypes: vec![eve_server::signal_tracker::ARCHETYPE_COMBAT],
        entry: dungeon_data::Entry::Anomaly,
        count: 1.0,
        difficulties: population::Diffs::Star,
        respawn_s: 600,
        ttl_s: 172_800,
    }])
    .expect("one row")
}

/// The anomaly's difficulty in the first test: two, which the Rifter's own
/// cap runs (a Sleeper site's real floor is three).
const RUNNABLE: i64 = 2;

fn start(raiders: &[(i64, &str)]) -> SocketAddr {
    start_with(raiders, RUNNABLE)
}

fn start_with(raiders: &[(i64, &str)], difficulty: i64) -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = raiders
        .iter()
        .enumerate()
        .map(|(i, (id, name))| pilot(*id, name, 100_000_001 + i as i64))
        .collect();
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&types()).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons(difficulty));
    content.insert("wormhole", statics());
    content.insert(
        "wormhole_sites",
        Sites::new(vec![SiteRow {
            dungeon: COMBAT_D,
            class_lo: 1,
            class_hi: 6,
            family: Family::Combat,
        }])
        .expect("one site")
        .with_hole_chance(1.0),
    );
    content.insert("population", population_table());
    let npcs = wormhole_sites::with_sleepers(
        npc_data::Table::default(),
        &[sleeper_row()],
        BTreeSet::from([COMBAT_D]),
    );
    content.insert("npc_data", npcs);
    content.insert(
        "salvage",
        wormhole_sites::overlay_of(&[sleeper_row()], &ANCIENT),
    );
    config.content = Some(Arc::new(content));
    serve(config)
}

/// The kit a raider's hangar holds when it starts, given the way a player
/// would have bought it: a salvager, a launcher and probes. (The hull, guns
/// and rounds are the open economy's GM setup, as every fighter's.)
fn stock(addr: SocketAddr, name: &str) {
    let mut c = login(addr, name);
    gm(&mut c, &format!("/createitem {SALVAGER}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
}

fn raider(addr: SocketAddr, name: &str) -> Handle {
    let mut config = BotConfig::new(name, addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&wh_raid::ROLE);
    config.policy.raid_cap = Some(4);
    driver::spawn(config).0
}

/// **The row's test**: a raider arms, undocks, scans the static, dives into
/// the class the cap allows, reads the Agency in J-space, clears the
/// Sleeper anomaly, loots and salvages the wreck, and comes out again.
#[test]
fn a_raider_dives_clears_a_sleeper_anomaly_and_salvages_ancient_salvage() {
    let addr = start(&[(RAIDER1, "Raider1")]);
    stock(addr, "Raider1");
    let bot = raider(addr, "Raider1");
    let dived =
        |e: &eve_bot::driver::Entry| ok(e, ActKind::Raid) && e.what == wh_raid::Op::In.describe();
    // 180 s, not 60: under a loaded gate (several lanes building) the dive has
    // taken longer than a minute to start; alone it starts in seconds.
    assert!(
        until(Duration::from_secs(180), || bot.log().iter().any(dived)),
        "the dive goes in:\n{}",
        dump(&bot)
    );
    assert!(
        until(Duration::from_secs(80), || {
            let log = bot.log();
            let dive = log.iter().position(dived);
            let fired = log.iter().position(|e| ok(e, ActKind::Fire));
            let looted = log.iter().rposition(|e| ok(e, ActKind::Loot));
            let salvaged = log.iter().position(|e| ok(e, ActKind::Salvage));
            matches!((dive, fired, looted, salvaged), (Some(d), Some(f), Some(l), Some(s)) if d < f && f < l && l < s)
        }),
        "in J-space: fire on the Sleeper, loot its wreck, then salvage it:\n{}",
        dump(&bot)
    );
    // The cycle ends (the bot goes on to the next site), and the salvage is
    // in the hold: read it as a player would, from the ship the bot left in
    // J-space.
    assert!(
        until(Duration::from_secs(180), || {
            let log = bot.log();
            let salvaged = log.iter().position(|e| ok(e, ActKind::Salvage));
            let after = log.iter().rposition(|e| e.kind == ActKind::Sites);
            matches!((salvaged, after), (Some(s), Some(a)) if s < a)
        }),
        "the bot goes on once the cycle has ended:\n{}",
        dump(&bot)
    );
    bot.stop();
    std::thread::sleep(Duration::from_secs(2));
    let mut c = login(addr, "Raider1");
    let hold = c.cargo().expect("the hold lists");
    let ancient: i64 = hold
        .iter()
        .filter(|r| r.group_id == wh_raid::GROUP_ANCIENT_SALVAGE)
        .map(|r| r.quantity)
        .sum();
    assert!(
        ancient > 0,
        "Ancient Salvage in the hold after the Sleeper's wreck was salvaged"
    );
}

/// **Nothing to fight, nothing to sweep: out.** An anomaly above the hull's
/// cap is not run, the system holds no relic or data signature, so two
/// raiders say so, scan down the K162 and come out in k-space.
#[test]
fn two_raiders_with_nothing_to_run_come_out_by_the_k162() {
    let addr = start_with(&[(RAIDER1, "Raider1"), (RAIDER2, "Raider2")], 5);
    stock(addr, "Raider1");
    stock(addr, "Raider2");
    let bots = [raider(addr, "Raider1"), raider(addr, "Raider2")];
    let out = |e: &eve_bot::driver::Entry| {
        ok(e, ActKind::Raid) && e.what.starts_with(wh_raid::Op::Out.describe())
    };
    assert!(
        until(Duration::from_secs(240), || bots
            .iter()
            .all(|b| b.log().iter().any(out))),
        "both raiders come out:\n{}\n--\n{}",
        dump(&bots[0]),
        dump(&bots[1])
    );
    for bot in &bots {
        let log = bot.log();
        let at = |f: &dyn Fn(&eve_bot::driver::Entry) -> bool| log.iter().position(f);
        let dive = at(&|e| ok(e, ActKind::Raid) && e.what == wh_raid::Op::In.describe());
        let sweep = at(&|e| e.kind == ActKind::Raid && e.what == wh_raid::Op::Sweep.describe());
        let out_at = at(&out);
        assert!(
            matches!((dive, sweep, out_at), (Some(d), Some(s), Some(o)) if d < s && s < o),
            "dive, a look for relic and data sites, the way out:\n{}",
            dump(bot)
        );
    }
    for bot in &bots {
        bot.stop();
    }
}
