//! SOC-B3 over a real socket: a pilot fits a probe launcher, launches
//! probes, scans a cosmic signature down to a perfect result, warps to it
//! and saves its location -- `eve_bot::probe`'s API, as another career
//! would call it.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): an invented high-sec
//! home system in an invented region with one invented pirate; one invented
//! data-site dungeon; a population row that puts one data signature there;
//! an invented launcher and an invented probe in the type data, whose
//! numbers are not the client's. The data archetype and its scan
//! attribute are the server's named constants. **No `[A]` value is an
//! expected value here**: the assertions are about kinds, ids and order.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::probe::{self, Kind};
use eve_server::celestial::{Band, Universe};
use eve_server::dungeon_data::{self, Dungeon, Entry, Object, Room};
use eve_server::npc_data::{self, RegionPirate};
use eve_server::population::{self, Diffs, Row};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const PROBER: i64 = 90_000_001;
const REGION: i64 = 19_053_001;
const CONSTELLATION: i64 = 29_053_001;
const PIRATE: i64 = 599_053;
const DUNGEON: i64 = 999_531;
const CAN: i64 = 990_531;
/// Invented type rows: a launcher (group 481, a high slot, `chargeGroup1`
/// the probes' group) and a probe (group 479) whose step 1 is half an AU.
const LAUNCHER: i64 = 990_532_001;
const PROBE: i64 = 990_532_010;
/// `typedata::FIXTURE`'s frigate: three high slots.
const FRIGATE: i64 = 990_000_001;
/// The data-site archetype, as the server names it (its scan attribute
/// is radar, `population::SCAN_ATTRIBUTES`).
const DATA: i64 = dungeon_data::FACTION_ARCHETYPES[2];

fn types() -> String {
    let launcher_group = probe::GROUP_LAUNCHER;
    let probe_group = probe::GROUP_PROBE;
    let factor = probe::RANGE_FACTOR;
    format!(
        "{}\
type\t{LAUNCHER}\t{launcher_group}\t7\t1\t1\t5.0\t0.8\t0.0\t1.0\t0\t12,16\t30=1.0;50=10.0;604={probe_group}.0\tFixture Probe Launcher\n\
type\t{PROBE}\t{probe_group}\t8\t1\t1\t0.1\t0.0\t1.0\t1.0\t0\t\t281=3600000.0;1370=0.5;1371=50.0;1373={factor}\tFixture Site Probe\n",
        eve_server::typedata::FIXTURE
    )
}

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

/// One data site: a room with one container.
fn dungeons() -> dungeon_data::Table {
    let d = Dungeon {
        id: DUNGEON,
        archetype: DATA,
        faction: Some(PIRATE),
        difficulty: Some(1),
        entry_type: Some(dungeon_data::ENTRY_SIGNATURE),
        name_id: Some(DUNGEON + 1),
        entry_object: Some(1),
        rooms: vec![Room {
            id: 1,
            ..Room::default()
        }],
        objects: vec![Object {
            room: 1,
            id: 1,
            type_id: CAN,
            pos: [0.0; 3],
            radius: Some(500.0),
            ..Object::default()
        }],
        ..Dungeon::default()
    };
    dungeon_data::Table {
        dungeons: BTreeMap::from([(DUNGEON, d)]),
        ..dungeon_data::Table::default()
    }
}

/// The region's pirate, which a faction archetype's slot asks for.
fn npcs() -> npc_data::Table {
    let mut t = npc_data::Table::default();
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

/// One data signature in every high-sec system.
fn sites() -> population::Table {
    population::Table::new(vec![Row {
        key: "fx_data_high".to_owned(),
        band: Band::High,
        archetypes: vec![DATA],
        entry: Entry::Signature,
        count: 1.0,
        difficulties: Diffs::In(vec![1]),
        respawn_s: 600,
        ttl_s: 172_800,
    }])
    .expect("one row")
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(PROBER, "Prober", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&types()).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("npc_data", npcs());
    content.insert("population", sites());
    config.content = Some(Arc::new(content));
    serve(config)
}

/// **The row's test**: outfitted docked, in space the pilot's probes skip
/// the data signature when asked for a relic site, scan it down when asked
/// for a data site, and the pilot warps onto it and saves the location.
#[test]
fn a_pilot_scans_a_data_signature_down_warps_to_it_and_saves_it() {
    let addr = start();
    let mut c = login(addr, "Prober");
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate is in the hangar");
    c.board(hull.item_id).expect("boards");

    let kit = probe::outfit(&mut c).expect("fits, onlines and loads");
    assert!(kit.loaded > 0, "{kit:?}");
    let again = probe::outfit(&mut c).expect("a second outfit is a no-op");
    assert_eq!(again.launcher, kit.launcher);

    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state in the home system");

    let manager = c.system_scan_manager().expect("the scan manager");
    let mut overlay = Vec::new();
    assert!(
        until(Duration::from_secs(30), || {
            overlay = c.sensor_overlay(&manager).expect("GetFullState");
            !overlay.is_empty()
        }),
        "the population puts a signature on the overlay"
    );
    let target = overlay[0].target_id.clone();

    assert_eq!(
        probe::find(&mut c, Kind::Relic).expect("scans"),
        None,
        "a data site is not a relic site"
    );
    let sig = probe::find(&mut c, Kind::Data)
        .expect("scans")
        .expect("the data site is scanned down");
    assert_eq!(sig.kind, Kind::Data);
    assert_eq!(sig.target_id, target);
    assert_eq!(sig.solar_system_id, SYSTEM);

    probe::warp_to(&mut c, &sig).expect("warps onto the signature");
    let mark = probe::bookmark(&mut c, &sig).expect("saves the location");
    assert!(mark > 0);

    let mut scanner = probe::Scanner::open(&mut c).expect("the probes are still out");
    let out = scanner.probes.len();
    assert!(out > 0);
    assert_eq!(scanner.recover(&mut c).expect("recovers"), out);
}

/// **The explorer career**: a bot playing [`probe::ROLE`] boards the hull
/// in its hangar, outfits on the way out, and in space scans the data
/// signature down, warps to it and books it on the ledger, saved.
#[test]
fn an_explorer_bot_outfits_scans_a_signature_down_and_books_it() {
    use super::common::{dump, gm_only_in_setup, ok};
    use eve_bot::brain::ActKind;
    use eve_bot::driver::{self, BotConfig, DEV_TOKEN, Pace};

    let addr = start();
    let mut c = login(addr, "Prober");
    gm(&mut c, &format!("/createitem {FRIGATE}"));
    gm(&mut c, &format!("/createitem {LAUNCHER}"));
    gm(
        &mut c,
        &format!("/createitem {PROBE} {}", probe::SETUP_PROBES),
    );
    drop(c);

    let config = BotConfig::new("Prober", addr, Pace::Fast)
        .with_token(DEV_TOKEN)
        .with_role(&probe::ROLE);
    let bot = driver::spawn(config).0;
    let found = |bot: &driver::Handle| {
        bot.log()
            .iter()
            .any(|e| ok(e, ActKind::Explore) && e.what.contains("[explore] data signature"))
    };
    assert!(
        until(Duration::from_secs(150), || found(&bot)),
        "outfit, undock, scan down, warp, save:\n{}",
        dump(&bot)
    );
    let log = bot.log();
    let board = log.iter().position(|e| ok(e, ActKind::Board));
    let undock = log.iter().position(|e| ok(e, ActKind::Undock));
    let outfit = log.iter().position(|e| ok(e, ActKind::Explore));
    assert!(
        matches!((board, outfit, undock), (Some(b), Some(o), Some(u)) if b < o && o < u),
        "board, outfit docked, then undock:\n{}",
        dump(&bot)
    );
    let hit = log
        .iter()
        .find(|e| ok(e, ActKind::Explore) && e.what.contains("[explore]"))
        .expect("checked");
    assert!(hit.what.contains("bookmark "), "{}", hit.what);
    assert!(
        probe::LEDGER
            .of_kind(addr, Kind::Data)
            .iter()
            .any(|(system, _)| *system == SYSTEM),
        "the ledger books the data site"
    );
    gm_only_in_setup(&bot);
    bot.stop();
}
