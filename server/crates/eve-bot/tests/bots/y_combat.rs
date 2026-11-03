//! Y-COMBAT over a real socket: a site runner clears a combat anomaly of
//! three rooms -- the first wave, the gated wave it triggers, a gate, a
//! second room, a gate, the last room -- and the site completes with its
//! cache in the last room.
//!
//! The runner's whole path is the player's: it warps to the anomaly through
//! the Agency, fights the rats the grid reconcile brings, loots, and with a
//! quiet grid and an acceleration gate on it (the slim says
//! `gateActivationRange`) calls `keeper.ActivateAccelerationGate` and rides
//! the warp to the next room. The server side is read through the world
//! handle, as `bot_corp.rs` does.
//!
//! **Invented rows only** (the no-client-data rule). The dungeon's
//! difficulty is the one the server's population fixture draws for the home
//! system's high-sec combat row. **No `[A]` value is an expected value**:
//! the cache's size is `combat_site`'s own function of the difficulty.

use super::common::{SYSTEM, config, dump, gm_only_in_setup, ok, pilot, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Handle, Pace};
use eve_bot::site_runner::{self, BOARD};
use eve_server::celestial::Universe;
use eve_server::combat_site;
use eve_server::dungeon_data::{self, Dungeon, Gate, Object, Room, Spawn};
use eve_server::npc_data::{self, Bounty, Needs, NpcTypeRow, RegionPirate, Tier};
use eve_server::server::Server;
use eve_server::service::Registry;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const RUNNER: i64 = 90_000_001;
const REGION: i64 = 19_056_001;
const CONSTELLATION: i64 = 29_056_001;
const PIRATE: i64 = 599_056;
const DUNGEON: i64 = 999_561;
const STRUCTURE: i64 = 990_561;
/// The cache's invented pieces, and the group of their types.
const PIECES: [i64; 3] = [990_571, 990_572, 990_573];
const COMBAT: i64 = eve_server::signal_tracker::ARCHETYPE_COMBAT;
/// The entry room has the highest id, as in the bundle.
const ENTRY: i64 = 30;
const MIDDLE: i64 = 10;
const LAST: i64 = 11;
const GATE_ENTRY: i64 = 2;
const GATE_MIDDLE: i64 = 4;
/// The home system's high-sec combat row draws difficulties 1 and 2 (the
/// server's population fixture); a dungeon of the larger one leaves a cache.
const DIFFICULTY: i64 = 2;

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

/// Three rooms, two gates: the entry room (a beacon and a gate) has no
/// squad; the middle room has a wave and a gated wave; the last one a wave.
fn dungeons() -> dungeon_data::Table {
    let object = |room, id, x| Object {
        room,
        id,
        type_id: STRUCTURE,
        pos: [x, 0.0, 0.0],
        radius: Some(1_000.0),
        ..Object::default()
    };
    let spawn = |room, object, entity, gated| Spawn {
        room,
        object,
        entity,
        trigger_spawn: gated,
        spawn_once: true,
        ..Spawn::default()
    };
    let d = Dungeon {
        id: DUNGEON,
        archetype: COMBAT,
        faction: Some(PIRATE),
        difficulty: Some(DIFFICULTY),
        entry_type: Some(dungeon_data::ENTRY_ANOMALY),
        name_id: Some(DUNGEON + 1),
        entry_object: Some(1),
        rooms: [ENTRY, MIDDLE, LAST]
            .into_iter()
            .map(|id| Room {
                id,
                ..Room::default()
            })
            .collect(),
        objects: vec![
            object(ENTRY, 1, 0.0),
            object(ENTRY, GATE_ENTRY, 5_000.0),
            object(MIDDLE, 3, 0.0),
            object(MIDDLE, GATE_MIDDLE, 8_000.0),
            object(LAST, 5, 0.0),
        ],
        gates: [GATE_ENTRY, GATE_MIDDLE]
            .into_iter()
            .map(|from_object| Gate {
                from_object,
                ..Gate::default()
            })
            .collect(),
        spawns: vec![
            spawn(MIDDLE, 3, 71, false),
            spawn(MIDDLE, 3, 72, true),
            spawn(LAST, 5, 73, false),
        ],
        ..Dungeon::default()
    };
    dungeon_data::Table {
        dungeons: BTreeMap::from([(DUNGEON, d)]),
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
    for (k, piece) in PIECES.iter().enumerate() {
        t.loot.push(npc_data::Loot {
            faction_id: PIRATE,
            size: combat_site::cache_size(DIFFICULTY),
            kind: "weapon".to_owned(),
            tier: Tier::Base,
            type_id: *piece,
            meta_level: Some(k as i64),
        });
    }
    t
}

/// The fixture's typedata and the pieces' types.
fn typedata() -> eve_server::typedata::Installed {
    let rows: String = PIECES
        .iter()
        .map(|p| {
            format!("type\t{p}\t990333\t7\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Piece {p}\n")
        })
        .collect();
    let table =
        eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
            .expect("the rows parse");
    eve_server::typedata::Installed::build(table)
}

fn start() -> (SocketAddr, Arc<RwLock<Registry>>) {
    let mut config = config();
    config.sandbox.characters = vec![pilot(RUNNER, "Runner1", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("typedata", typedata());
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

fn runner(addr: SocketAddr) -> Handle {
    let config = BotConfig::new("Runner1", addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&site_runner::ROLE);
    driver::spawn(config).0
}

/// **The row's test**: the runner takes the site through its gates, room by
/// room, and the site's last room leaves its cache.
#[test]
fn a_runner_takes_a_combat_site_through_its_gates_and_the_site_leaves_its_cache() {
    let (addr, world) = start();
    let bot = runner(addr);
    let pitch = (addr, SYSTEM);
    assert!(
        until(Duration::from_secs(60), || !BOARD.picks(pitch).is_empty()),
        "the runner picks a site:\n{}",
        dump(&bot)
    );
    let site = BOARD.picks(pitch)[0].1;

    // All three rooms placed: the gates were activated by the runner.
    let state = |world: &Arc<RwLock<Registry>>| {
        let mut w = world.write().expect("world");
        let rooms = combat_site::open_rooms(&mut w, site).len();
        let me = w
            .roster()
            .get(RUNNER)
            .map(|p| (p.motion.position, format!("{:?}", p.motion.mode)));
        format!("rooms {rooms}, runner {me:?}")
    };
    assert!(
        until(Duration::from_secs(240), || {
            let mut w = world.write().expect("world");
            combat_site::open_rooms(&mut w, site).len() == 3
                && combat_site::rooms_unopened(&mut w, site) == 0
        }),
        "the runner went through both gates: {}\n{}",
        state(&world),
        dump(&bot)
    );
    // The cache: one of the pool's pieces, wherever the runner has put it.
    assert!(
        until(Duration::from_secs(240), || {
            let w = world.read().expect("world");
            w.store()
                .items()
                .iter()
                .any(|i| PIECES.contains(&i.type_id))
        }),
        "the last room's cache held a piece of the faction's pool:\n{}",
        dump(&bot)
    );
    // It fought: only this runner could have used the gates, and each room
    // had rats to kill before the site could complete.
    assert!(
        bot.log().iter().any(|e| ok(e, ActKind::Fire)),
        "it fired:\n{}",
        dump(&bot)
    );
    gm_only_in_setup(&bot);
    bot.stop();
}
