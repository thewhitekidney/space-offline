//! CP55 over a real socket: site runners -- brain, driver, verbs -- read
//! the Agency, warp to a combat anomaly, clear it and loot.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): the home system gets an
//! invented high-sec row so the server's population fixture
//! (`population::Table::fixture`, its high-sec combat row) draws the one
//! invented combat dungeon here, whose single room holds one structure and
//! one ungated, never-respawning spawn. Its faction is an invented pirate,
//! the region's only one, whose only ship is the server's no-bundle rat
//! (`npc::FIXTURE_RAT`), so a kill leaves a wreck with loot in it.
//!
//! **No `[A]` value is an expected value here.** The runners play at
//! [`Pace::Fast`]; the assertions are about kinds, ids and order. Deadlines
//! are generous ceilings.

use super::common::{SYSTEM, config, dump, gm_only_in_setup, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Handle, Pace};
use eve_bot::site_runner::{self, BOARD};
use eve_marshal::PyValue;
use eve_server::celestial::Universe;
use eve_server::dungeon_data::{self, Dungeon, Object, Room, Spawn};
use eve_server::npc_data::{self, Bounty, Needs, NpcTypeRow, RegionPirate, Tier};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const RUNNER1: i64 = 90_000_001;
const RUNNER2: i64 = 90_000_002;
const HUMAN: i64 = 90_000_003;
/// Invented: the home system's region and constellation, its pirate, the
/// dungeon and its structure's type.
const REGION: i64 = 19_055_001;
const CONSTELLATION: i64 = 29_055_001;
const PIRATE: i64 = 599_055;
const DUNGEON: i64 = 999_551;
const STRUCTURE: i64 = 990_551;
/// `dunArchetypeCombatSites` [V], as the server names it.
const COMBAT: i64 = eve_server::signal_tracker::ARCHETYPE_COMBAT;

/// The home system as an invented high-sec row, in an invented region.
fn universe() -> Universe {
    let system = format!(
        "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t0.9\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    Universe::parse(
        &[system],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

/// One combat anomaly: a structure and one spawn at it, no respawn window,
/// so killing the squad clears the site.
fn dungeons() -> dungeon_data::Table {
    let d = Dungeon {
        id: DUNGEON,
        archetype: COMBAT,
        faction: Some(PIRATE),
        difficulty: Some(1),
        entry_type: Some(dungeon_data::ENTRY_ANOMALY),
        name_id: Some(DUNGEON + 1),
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
    dungeon_data::Table {
        dungeons: BTreeMap::from([(DUNGEON, d)]),
        ..dungeon_data::Table::default()
    }
}

/// The region's one pirate, whose one ship is the no-bundle rat.
fn npcs() -> npc_data::Table {
    let rat = eve_server::npc::FIXTURE_RAT.type_id;
    let mut t = npc_data::Table::default();
    t.types.insert(
        rat,
        NpcTypeRow {
            type_id: rat,
            group_id: eve_server::npc::FIXTURE_RAT.group_id,
            faction_id: PIRATE,
            context: eve_server::npc_gen::ASTEROID.to_owned(),
            class: "Frigate".to_owned(),
            tier: Tier::Base,
            alias_of: None,
            needs: Needs::TURRET,
            role: npc_data::Role::Dps,
        },
    );
    t.bounty.insert(
        rat,
        Bounty {
            bounty: 1.0,
            ..Bounty::default()
        },
    );
    t.regions.insert(
        REGION,
        vec![RegionPirate {
            faction_id: PIRATE,
            weight: 1.0,
            rule: npc_data::Rule::Override,
        }],
    );
    t
}

/// A server whose home system has combat anomalies; `runners` in their
/// capsules at the home station, and a human stand-in.
fn start(runners: &[(i64, &str)]) -> SocketAddr {
    let mut config = config();
    let mut chars: Vec<_> = runners
        .iter()
        .enumerate()
        .map(|(i, (id, name))| pilot(*id, name, 100_000_001 + i as i64))
        .collect();
    chars.push(pilot(HUMAN, "Watcher", 100_000_009));
    config.sandbox.characters = chars;
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", eve_server::population::Table::fixture());
    config.content = Some(Arc::new(content));
    serve(config)
}

/// A runner, sending its token -- never its name (CA19).
fn runner(addr: SocketAddr, name: &str) -> Handle {
    let config = BotConfig::new(name, addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&site_runner::ROLE);
    assert_ne!(config.token, config.name, "the name is never the token");
    driver::spawn(config).0
}

/// What a plain connection in the system reads from the Agency, once the
/// world's population stage has handed the system's sites to the tracker
/// (its own rows appear then).
fn agency(addr: SocketAddr) -> (BTreeMap<i64, i64>, Vec<i64>) {
    let mut c = login(addr, "Watcher");
    let call = |c: &mut eve_bot::Conn, m: &str| {
        c.call(site_runner::AGENCY, m, vec![])
            .expect("the Agency answers")
            .value()
            .unwrap_or(PyValue::None)
    };
    let mut rows = Vec::new();
    until(Duration::from_secs(30), || {
        rows = site_runner::read_rows(&call(&mut c, site_runner::ROWS_METHOD), SYSTEM);
        !rows.is_empty()
    });
    let counts = site_runner::read_counts(&call(&mut c, site_runner::COUNT_METHOD));
    (counts, rows.iter().map(|r| r.site_id).collect())
}

/// **The row's test**: the runner arms itself, undocks, reads the Agency
/// and warps to an anomaly it lists; on the site's grid it fires on the
/// site's rat, loots its wreck, and -- the grid quiet -- runs again for the
/// next site.
#[test]
fn a_runner_reads_the_agency_warps_to_an_anomaly_clears_it_and_loots() {
    let addr = start(&[(RUNNER1, "Runner1")]);
    let (counts, sites) = agency(addr);
    assert!(
        counts.get(&SYSTEM).is_some_and(|n| *n > 0) && !sites.is_empty(),
        "the fixture has combat anomalies at home: {counts:?} {sites:?}"
    );

    let bot = runner(addr, "Runner1");
    let pitch = (addr, SYSTEM);
    assert!(
        until(Duration::from_secs(120), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Sites))),
        "the runner reads the Agency and warps:\n{}",
        dump(&bot)
    );
    let picks = BOARD.picks(pitch);
    let first = picks.first().map(|p| p.1).expect("a pick on the board");
    assert_eq!(picks[0].0, RUNNER1);
    assert!(
        sites.contains(&first),
        "{first} is an Agency row: {sites:?}"
    );

    let at = |kind| {
        let log = bot.log();
        log.iter().position(|e| ok(e, kind))
    };
    assert!(
        until(Duration::from_secs(150), || {
            let log = bot.log();
            let fired = log.iter().position(|e| ok(e, ActKind::Fire));
            let looted = log.iter().rposition(|e| ok(e, ActKind::Loot));
            let again = log.iter().rposition(|e| e.kind == ActKind::Sites);
            matches!((fired, looted, again), (Some(f), Some(l), Some(s)) if f < l && l < s)
        }),
        "fire on the site's rat, loot its wreck, then the next site:\n{}",
        dump(&bot)
    );
    let fire = at(ActKind::Fire).expect("checked");
    let sites_at = at(ActKind::Sites).expect("checked");
    assert!(
        sites_at < fire,
        "the warp to the site came first:\n{}",
        dump(&bot)
    );
    gm_only_in_setup(&bot);
    bot.stop();
}

/// **Two runners in one system never pick the same site twice in a row**:
/// on the shared board, their first picks differ, and no runner's pick
/// repeats its own last.
#[test]
fn two_runners_in_one_system_never_pick_the_same_site_twice_in_a_row() {
    let addr = start(&[(RUNNER1, "Runner1"), (RUNNER2, "Runner2")]);
    let (_, sites) = agency(addr);
    assert!(sites.len() >= 2, "two anomalies for two runners: {sites:?}");
    let one = runner(addr, "Runner1");
    let two = runner(addr, "Runner2");
    let pitch = (addr, SYSTEM);
    assert!(
        until(Duration::from_secs(120), || {
            let picks = BOARD.picks(pitch);
            [RUNNER1, RUNNER2]
                .iter()
                .all(|r| picks.iter().any(|p| p.0 == *r))
        }),
        "both runners pick:\n{}\n--\n{}",
        dump(&one),
        dump(&two)
    );
    let picks = BOARD.picks(pitch);
    let first = |r: i64| picks.iter().find(|p| p.0 == r).map(|p| p.1);
    assert_ne!(first(RUNNER1), first(RUNNER2), "{picks:?}");
    for r in [RUNNER1, RUNNER2] {
        let own: Vec<i64> = picks.iter().filter(|p| p.0 == r).map(|p| p.1).collect();
        assert!(own.windows(2).all(|w| w[0] != w[1]), "{r}: {own:?}");
    }
    one.stop();
    two.stop();
}
