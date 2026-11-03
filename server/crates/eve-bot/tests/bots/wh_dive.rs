//! Y-WH over a real socket: a pilot in null-sec scans a wormhole signature
//! down, warps to it, reads where the hole leads from its slim, jumps into
//! J-space (`wormholeMgr.WormholeJump`, the session change), and then finds
//! the K162 by where it landed, warps to it and jumps out again --
//! `eve_bot::wh_dive`'s `dive_in` and `come_out`, as the gas dive calls them.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): an invented null-sec
//! home system and an invented J-space system (a `31xxxxxx` id, class 3,
//! the rule's constant); the wormhole tables' own invented statics rows
//! (`wormhole::Row`, one type per direction) with an invented archetype-38
//! dungeon for the static type and one for the K162; an invented launcher
//! and probe as in `probe.rs`. The hole is pinned to exist
//! (`Sites::with_hole_chance`), since the world's seed is minted at first
//! run. **No `[A]` value is an expected value here**: the assertions are
//! about systems, classes and order.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::probe;
use eve_bot::wh_dive::{self, Leads};
use eve_server::celestial::Universe;
use eve_server::dungeon_data::{self, Dungeon, ENTRY_SIGNATURE, Object, Room};
use eve_server::wormhole::{self, ARCHETYPE, K162};
use eve_server::wormhole_sites::Sites;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const DIVER: i64 = 90_000_001;
const REGION: i64 = 19_055_001;
const CONSTELLATION: i64 = 29_055_001;
/// An invented J-space system (the `31xxxxxx` range is the rule's) and the
/// class it is, C3 (a [V] constant).
const J_SYSTEM: i64 = 31_955_001;
const J_CLASS: i64 = 3;
/// The invented static: a type no client has, and its dungeon.
const STATIC: i64 = 990_552;
const LAUNCHER: i64 = 990_553_001;
const PROBE: i64 = 990_553_010;
/// `typedata::FIXTURE`'s frigate: three high slots.
const FRIGATE: i64 = 990_000_001;

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

/// Null-sec home (class 9 by its band) and one J-space system of class 3.
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

/// An archetype-38 dungeon per hole type: a room with the hole and a
/// Deadspace Signature.
fn dungeons() -> dungeon_data::Table {
    let list = [STATIC, K162].iter().enumerate().map(|(i, t)| {
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
            objects: vec![object(1, *t), object(2, ENTRY_SIGNATURE)],
            ..Dungeon::default()
        }
    });
    dungeon_data::Table {
        dungeons: list.map(|d| (d.id, d)).collect(),
        ..dungeon_data::Table::default()
    }
}

/// One static row from null-sec (class 9, a [V] constant) to class 3.
fn statics() -> wormhole::Table {
    wormhole::Table::new(vec![wormhole::Row {
        class: wormhole::CLASS_NULL,
        type_id: STATIC,
        dest_class: J_CLASS,
    }])
    .expect("one row")
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(DIVER, "Diver", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&types()).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    content.insert("universe", universe());
    content.insert("dungeon_data", dungeons());
    content.insert("wormhole", statics());
    content.insert(
        "wormhole_sites",
        Sites::new(Vec::new())
            .expect("an empty table")
            .with_hole_chance(1.0),
    );
    config.content = Some(Arc::new(content));
    serve(config)
}

/// **The row's test**: in, and out by the K162.
#[test]
fn a_pilot_dives_in_through_a_hole_and_comes_out_by_its_k162() {
    let addr = start();
    let mut c = login(addr, "Diver");
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
    probe::outfit(&mut c).expect("fits, onlines and loads");
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state in the home system");

    // The static is on the home overlay once the system is live.
    let manager = c.system_scan_manager().expect("the scan manager");
    assert!(
        until(Duration::from_secs(40), || {
            !c.sensor_overlay(&manager).expect("GetFullState").is_empty()
        }),
        "the static is a signature in the home system"
    );

    // In.
    let entered = wh_dive::dive_in(&mut c)
        .expect("the dive goes")
        .expect("a hole into J-space is found");
    assert_eq!(entered.from, SYSTEM);
    assert_eq!(
        entered.class, J_CLASS,
        "the slim names the class it leads to"
    );
    assert_eq!(c.session().solar_system_id(), Some(J_SYSTEM));
    assert!(wh_dive::in_j_space(J_SYSTEM));

    // The K162 is on the arrival grid, and leads home.
    assert!(
        until(Duration::from_secs(20), || {
            wh_dive::pick_hole(&wh_dive::holes_in(&mut c), Leads::KSpace).is_some()
        }),
        "the K162 stands where the bot landed: {:?}",
        wh_dive::holes_in(&mut c)
    );

    // Out, by where the bot arrived.
    let manager = c
        .system_scan_manager()
        .expect("the scan manager in J-space");
    assert!(
        until(Duration::from_secs(60), || {
            // The K162's signature reaches the tracker on the system's next sync.
            !c.sensor_overlay(&manager).expect("GetFullState").is_empty()
        }),
        "the K162 is a signature in J-space"
    );
    let out = wh_dive::come_out(&mut c, Some(entered.arrival));
    assert!(out.expect("the way out runs"), "the way out is found");
    assert_eq!(c.session().solar_system_id(), Some(SYSTEM), "home again");
    assert!(!wh_dive::in_j_space(SYSTEM));
}
