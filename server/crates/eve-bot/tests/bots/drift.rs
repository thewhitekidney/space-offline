//! FX6 over a real socket: the park's ego after an orbit fight.
//!
//! An escort-like runner orbits its rat while the rat orbits it (a mutual
//! tether); the bot's park and the server advance that pair at different
//! cadences, so by the kill the bot's ego was 150-400 km from the server's
//! (`park.rs`'s header, `docs/lanes/fx6.md`). The cure is one
//! `UpdateStateRequest` when the ego's tether target leaves the park; this
//! checks the driver sends it, in order: fire, the rat gone and the state
//! asked for, then the loot.
//!
//! **Invented rows only**: the same fixture as `site_runner.rs` (copied:
//! its helpers are that file's own) -- an invented high-sec home whose one
//! combat anomaly holds one no-bundle rat. **No `[A]` value is an expected
//! value here**: kinds and order only.

use super::common::{SYSTEM, config, dump, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Pace};
use eve_bot::site_runner;
use eve_server::celestial::Universe;
use eve_server::dungeon_data::{self, Dungeon, Object, Room, Spawn};
use eve_server::npc_data::{self, Bounty, Needs, NpcTypeRow, RegionPirate, Tier};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

const RUNNER: i64 = 90_000_001;
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

/// **The row's test**: after the rat the runner orbited dies, the driver
/// asks for a fresh state (`resync`, ok) before it loots the wreck.
#[test]
fn after_an_orbit_fight_the_park_asks_for_the_truth_before_looting() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(RUNNER, "Runner1", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", eve_server::population::Table::fixture());
    config.content = Some(Arc::new(content));
    let addr = serve(config);

    let config = BotConfig::new("Runner1", addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&site_runner::ROLE);
    let bot = driver::spawn(config).0;
    let order = || {
        let log = bot.log();
        let orbit = log.iter().position(|e| ok(e, ActKind::Orbit))?;
        let fire = log.iter().position(|e| ok(e, ActKind::Fire))?;
        let resync = fire
            + log[fire..]
                .iter()
                .position(|e| ok(e, ActKind::Resync) && e.what.contains("tether"))?;
        let loot = resync + log[resync..].iter().position(|e| ok(e, ActKind::Loot))?;
        Some((orbit, fire, resync, loot))
    };
    assert!(
        until(Duration::from_secs(180), || order().is_some()),
        "orbit, fire, a resync once the rat is gone, then loot:\n{}",
        dump(&bot)
    );
    let (orbit, fire, resync, loot) = order().expect("checked");
    assert!(orbit < resync && fire < resync && resync < loot);
    bot.stop();
}
