//! Y-ESCAL over a real socket.
//!
//! 1. **The player's side.** A pilot is told by `OnEscalatingPathMessage`
//!    (the push the journal opens the Expeditions transmission on), lists
//!    the escalation as `journal.GetEscalations` does
//!    (`dungeonExplorationMgr.GetMyEscalatingPathDetails`), and warps to it
//!    by `CmdWarpToStuff('epinstance', instanceID)` as the transmission's
//!    Warp To does; the site is then in the world, in the named system.
//! 2. **The bots' side.** A site runner clears a combat site whose roll the
//!    test has made certain (the world's seed is chosen, not the chance's
//!    value), the escalation appears for it in the named system, and the
//!    runner follows it from its own Expeditions list and clears that too:
//!    the row goes when the site is cleared.
//!
//! **Invented rows only** (the no-client-data rule). **No `[A]` value is an
//! expected value**: the chance, the pairing and the limit are
//! `combat_site`'s and `escalation`'s own functions and constants.

use super::common::{SYSTEM, config, dump, gm, login, pilot, until};
use eve_bot::Event;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Handle, Pace};
use eve_bot::site_runner::{self, BOARD};
use eve_marshal::PyValue;
use eve_server::celestial::Universe;
use eve_server::combat_site;
use eve_server::dungeon_data::{self, Dungeon, Object, Room, Spawn};
use eve_server::escalation;
use eve_server::npc_data::{self, Bounty, Needs, NpcTypeRow, RegionPirate, Tier};
use eve_server::server::Server;
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_201;
const RUNNER: i64 = 90_000_202;
const REGION: i64 = 19_057_001;
const CONSTELLATION: i64 = 29_057_001;
const PIRATE: i64 = 599_057;
const COMBAT_SITE: i64 = 999_571;
const ESCALATION: i64 = 999_572;
/// An invented archetype-31 dungeon below the real floor, so a frigate
/// runner's hull fits it (`site_runner::difficulty_cap`): the real rungs
/// start at `combat_site::ESCALATION_MIN_DIFFICULTY`, which a frigate skips.
const EASY: i64 = 999_573;
const STRUCTURE: i64 = 990_571;
/// The escalation's journal text (a message id of the invented dungeon).
const TEXT: i64 = 999_579;
const ROOM: i64 = 30;
/// The combat site is the escalation floor's neighbour below, so the pairing
/// is `combat_site`'s own rule at the floor.
const COMBAT_DIFFICULTY: i64 = 2;

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

/// A one-room dungeon: a beacon-like structure and one wave of rats.
fn dungeon(id: i64, archetype: i64, difficulty: i64) -> Dungeon {
    let object = |id, x| Object {
        room: ROOM,
        id,
        type_id: STRUCTURE,
        pos: [x, 0.0, 0.0],
        radius: Some(1_000.0),
        ..Object::default()
    };
    Dungeon {
        id,
        archetype,
        faction: Some(PIRATE),
        difficulty: Some(difficulty),
        entry_type: Some(dungeon_data::ENTRY_ANOMALY),
        name_id: Some(id + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: ROOM,
            message_text_id: (archetype == combat_site::ESCALATION_ARCHETYPE).then_some(TEXT),
            ..Room::default()
        }],
        objects: vec![object(1, 0.0), object(3, 3_000.0)],
        spawns: vec![Spawn {
            room: ROOM,
            object: 3,
            entity: 71,
            spawn_once: true,
            ..Spawn::default()
        }],
        ..Dungeon::default()
    }
}

fn dungeons() -> dungeon_data::Table {
    let list = [
        dungeon(
            COMBAT_SITE,
            combat_site::COMBAT_ARCHETYPE,
            COMBAT_DIFFICULTY,
        ),
        dungeon(
            ESCALATION,
            combat_site::ESCALATION_ARCHETYPE,
            combat_site::ESCALATION_MIN_DIFFICULTY,
        ),
        dungeon(EASY, combat_site::ESCALATION_ARCHETYPE, 1),
    ];
    dungeon_data::Table {
        dungeons: list.into_iter().map(|d| (d.id, d)).collect(),
        ..dungeon_data::Table::default()
    }
}

/// The region's one pirate, whose one ship is the no-bundle rat, and a
/// weapon pool of invented pieces.
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

fn start(characters: Vec<eve_server::model::Character>) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let mut config = config();
    config.sandbox.characters = characters;
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", eve_server::population::Table::fixture());
    config.content = Some(Arc::new(content));
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn rows_of(c: &mut eve_bot::Conn) -> Vec<site_runner::EscalationRow> {
    let out = c
        .call(
            site_runner::ESCALATION_SERVICE,
            site_runner::ESCALATION_METHOD,
            vec![],
        )
        .expect("answered");
    site_runner::read_escalations(&out.value().expect("a rowset"))
}

/// **The player's side**: told, listed, warped to, placed.
#[test]
fn a_pilot_is_told_lists_and_warps_to_an_escalation() {
    let (addr, world) = start(vec![pilot(ALICE, "Alice", 100_000_201)]);
    let mut c = login(addr, "Alice");
    assert!(rows_of(&mut c).is_empty(), "nothing before");
    gm(&mut c, "/escalate");

    // The push the journal opens the Expeditions transmission on.
    let mut told = None;
    assert!(
        until(Duration::from_secs(20), || {
            for e in c.pump(Duration::from_millis(200)).expect("reads") {
                if let Event::Push(name, args) = e
                    && name == escalation::EVENT_MESSAGE
                {
                    told = Some(args);
                }
            }
            told.is_some()
        }),
        "OnEscalatingPathMessage arrives"
    );
    let Some(PyValue::Int(id)) = told.and_then(|a| a.into_iter().next()) else {
        panic!("the push carries an instance id")
    };

    // The list: one row, in the named system, of the escalation dungeon.
    let rows = rows_of(&mut c);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].instance_id, id);
    assert_eq!(rows[0].system, SYSTEM);
    assert_eq!(
        rows[0].difficulty,
        Some(combat_site::ESCALATION_MIN_DIFFICULTY)
    );
    assert!(rows[0].expiry > eve_server::pyutil::filetime_now());

    // Warp To, as the transmission sends it.
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("in space");
    let out = c
        .bound_call_kw(
            "CmdWarpToStuff",
            vec![
                PyValue::Bytes(site_runner::ESCALATION_SUBJECT.to_vec()),
                PyValue::Int(id),
            ],
            vec![(
                PyValue::Real(site_runner::WARP_RANGE),
                PyValue::Bytes(b"minRange".to_vec()),
            )],
        )
        .expect("answered");
    assert_eq!(out.refusal(), None, "the warp is accepted");
    assert!(
        until(Duration::from_secs(120), || {
            let _ = c.pump(Duration::from_millis(200));
            escalation::placed(&world.read().expect("world"), id).is_some()
        }),
        "the escalation's site exists once the pilot is there"
    );
    let w = world.read().expect("world");
    assert_eq!(escalation::placed(&w, id), Some((SYSTEM, ESCALATION)));
}

fn runner(addr: SocketAddr) -> Handle {
    let config = BotConfig::new("Runner1", addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&site_runner::ROLE);
    driver::spawn(config).0
}

/// A world seed under which every combat site of the home system that the
/// population draws rolls an escalation as the first instance (`seq` 0): the
/// chance is the world's, the test only picks the world.
fn certain_seed(world: &Arc<RwLock<Registry>>) -> u64 {
    let table = dungeons();
    let from = table.dungeons[&COMBAT_SITE].clone();
    let mut w = world.write().expect("world");
    for seed in 1..200_000u64 {
        escalation::set_seed_for_test(&mut w, seed);
        let now = w.world().birth;
        let sites: Vec<_> = eve_server::population::sites(&w, SYSTEM, now)
            .sites
            .into_iter()
            .filter(|s| s.dungeon_id == COMBAT_SITE)
            .collect();
        if !sites.is_empty()
            && sites
                .iter()
                .all(|s| combat_site::escalation_for(&table, &from, seed, s.site_id, 0).is_some())
        {
            return seed;
        }
    }
    panic!("no seed makes every combat site escalate")
}

/// **The bots' side**: complete, escalate, follow, complete again.
#[test]
fn a_cleared_site_opens_an_escalation_for_the_runner_who_cleared_it() {
    let (addr, world) = start(vec![pilot(RUNNER, "Runner1", 100_000_202)]);
    let seed = certain_seed(&world);
    let bot = runner(addr);
    let pitch = (addr, SYSTEM);
    assert!(
        until(Duration::from_secs(60), || !BOARD.picks(pitch).is_empty()),
        "the runner picks a site:\n{}",
        dump(&bot)
    );

    // The escalation the clear opened: in the named system, the faction's
    // dungeon, shown to the runner who finished the site.
    let mut seen = None;
    assert!(
        until(Duration::from_secs(240), || {
            let w = world.read().expect("world");
            seen = escalation::all(&w).into_iter().next();
            seen.is_some()
        }),
        "the cleared site escalated (seed {seed}):\n{}",
        dump(&bot)
    );
    let e = seen.expect("seen");
    assert_eq!(e.system, SYSTEM);
    assert_eq!(e.dungeon, ESCALATION);
    assert_eq!(e.from_dungeon, COMBAT_SITE);
    assert_eq!(e.chars, vec![RUNNER]);
    assert!(escalation::is_escalation_site(e.site_id));

    bot.stop();
}

/// **The bots' side, following**: a runner with an escalation on its list,
/// in its own system and of a difficulty its hull fits, takes it before the
/// Agency's anomalies, clears it, and the row leaves the journal.
#[test]
fn a_runner_follows_its_own_escalation_and_clears_it() {
    let (addr, world) = start(vec![pilot(RUNNER, "Runner1", 100_000_202)]);
    let id = {
        let mut w = world.write().expect("world");
        let e = combat_site::Escalation {
            from_site: 0,
            from_dungeon: COMBAT_SITE,
            system: SYSTEM,
            dungeon_id: EASY,
            when: eve_server::pyutil::filetime_now(),
        };
        escalation::open(&mut w, &e, vec![RUNNER], e.when).expect("opened")
    };
    let bot = runner(addr);
    assert!(
        until(Duration::from_secs(240), || {
            escalation::placed(&world.read().expect("world"), id).is_some()
        }),
        "the runner went to its escalation:\n{}",
        dump(&bot)
    );
    assert_eq!(
        escalation::placed(&world.read().expect("world"), id),
        Some((SYSTEM, EASY))
    );
    assert!(
        until(Duration::from_secs(240), || {
            escalation::get(&world.read().expect("world"), id).is_none()
        }),
        "the runner cleared its escalation:\n{}",
        dump(&bot)
    );
    bot.stop();
}
