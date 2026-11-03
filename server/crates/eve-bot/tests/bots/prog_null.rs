//! **Supply runs** (PROG-NULL) over a socket: a builder whose only seller
//! of its next rung's original is in a null-sec system, behind a gate the
//! high-sec floor forbids, weighs the danger, flies there at the safest
//! floor that reaches it (here: anything), buys the original and brings it
//! home -- where the plain trip of `travel.rs` finds no seller in reach.
//!
//! Synthetic furniture only, as `travel.rs`: two invented systems, an
//! invented blueprint on the first rung of the ladder
//! ([`builder::BLUEPRINT_LADDER`], named), an invented NPC corporation that
//! lists it at the far station only.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::{self, BLUEPRINT_LADDER};
use eve_bot::travel::{self, Map, SECURITY_FLOOR, Trips};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::industry;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

// Invented ids and numbers only.
const BUILDER: i64 = 90_000_301;
const CAROL: i64 = 90_000_302;
const CORP: i64 = 1_999_701;
const B: i64 = 30_000_003;
const STATION_B: i64 = 60_900_001;
const GATE_A: i64 = 50_900_001;
const GATE_B: i64 = 50_900_002;
const GATE_TYPE: i64 = 990_400_001;
const PRODUCT: i64 = 990_400_002;
const BLUEPRINT_GROUP: i64 = 99_105;
const PRODUCT_GROUP: i64 = 99_083;
const BLUEPRINT_REF: f64 = 100_000.0;
const PRODUCT_REF: f64 = 100.0;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 1_000_000.0;
/// Both systems: a security every floor passes.
const SECURE: f64 = 1.0;

fn blueprint() -> i64 {
    BLUEPRINT_LADDER[0]
}

fn home() -> Character {
    Character::default()
}

fn system_a() -> i64 {
    home()
        .solar_system_id
        .expect("the default pilot is in a system")
}

fn station_a() -> i64 {
    home().station_id.expect("the default pilot is docked")
}

/// Ten columns, as `tools/celestials.py` writes them: each system's
/// station at its origin, and one gate pair a thousand kilometres out,
/// beyond the client's warp floor.
fn catalogue() -> eve_server::celestial::Catalogue {
    let (a, home_station) = (system_a(), station_a());
    let (constellation, region) = (home().constellation_id, home().region_id);
    let station_type = home().station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         #system\t{B}\t{constellation}\t{region}\n\
         station\t{home_station}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_A}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_B}\t{B}\n\
         station\t{STATION_B}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{B}\t0\t0\n\
         gate\t{GATE_B}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{B}\t{GATE_A}\t{a}\n"
    ))
}

/// `Content::fixture()` plus the blueprint, its product and recipe, and
/// the corporation that sells it at the far station only.
fn content() -> Content {
    let bp = blueprint();
    let types = format!(
        "{}\
         type\t{PRODUCT}\t{PRODUCT_GROUP}\t8\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Product\n\
         type\t{bp}\t{BLUEPRINT_GROUP}\t9\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Blueprint\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PRODUCT}\t{PRODUCT_REF}\tbase\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t\t1"),
        format!("{bp}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let recipes = industry::Table::parse(
        &[
            format!("{bp}\t1\tactivity\t0\t0\t\t1\t10"),
            format!("{bp}\t1\tproduct\t{PRODUCT}\t1\t\t1\t10"),
        ],
        &prices,
    )
    .expect("the recipe parses");
    let exchange = exchange::Table::new(
        vec![exchange::Station {
            station_id: STATION_B,
            system_id: B,
            region_id: home().region_id,
            owner_id: CORP,
        }],
        &[(CORP, bp, 1.0)],
        [(
            bp,
            exchange::TypeRow {
                group_id: BLUEPRINT_GROUP,
                category_id: exchange::CATEGORY_BLUEPRINT,
                base_price: BLUEPRINT_REF,
                meta_group_id: None,
                tech_level: None,
            },
        )]
        .into_iter()
        .collect(),
        &[],
    )
    .expect("the exchange builds");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("industry", recipes);
    c.insert("econ_exchange", exchange);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: BUILDER,
            name: "Builder1".to_owned(),
            ship_id: 100_000_301,
            balance: WALLET,
            ..home()
        },
        // Seeded at the far station, so this world knows it.
        Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_302,
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

/// The bot's map: the far system is null-sec.
fn map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(system_a(), SECURE)
        .with_security(B, DEEP)
}

/// Null-sec: under every floor but `ANY_SECURITY`.
const DEEP: f64 = -0.5;

/// The plain trip finds no seller (the floor forbids B); the supply run
/// weighs the null-sec trip against the original's worth, goes, buys, and
/// comes home: docked at home, the blueprint in the home hangar.
#[test]
fn a_builder_runs_to_a_null_station_for_an_original_and_comes_home() {
    let addr = start();
    let mut b = login(addr, "Builder1");
    gm(&mut b, &format!("/createitem {FRIGATE}"));
    let hull = b
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    b.board(hull.item_id).expect("boards the frigate");

    let map = map();
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };
    assert_eq!(
        travel::find(&mut b, trips, blueprint(), 1, None).expect("the book reads"),
        None,
        "the plain errand finds nothing at the high-sec floor"
    );
    let content = content();
    let cycle = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");

    assert_eq!(cycle.blueprint, Some(blueprint()), "{cycle:?}");
    assert!(
        cycle.notes.iter().any(|n| n.starts_with(&format!(
            "supply run: rung 0 from station {STATION_B} (1 jumps"
        ))),
        "the run is announced with its risk: {cycle:?}"
    );
    assert!(
        cycle
            .notes
            .iter()
            .any(|n| n.starts_with(&format!("trip to station {STATION_B} (2 jumps)"))),
        "one jump out, one back: {cycle:?}"
    );
    assert_eq!(
        b.session().station_id(),
        Some(station_a()),
        "docked at home"
    );
    let held = b.blueprints().expect("the blueprints list");
    assert!(
        held.iter()
            .any(|r| r.type_id == blueprint() && r.location_id == station_a()),
        "the original is in the home hangar: {held:?}"
    );
    assert!(b.cargo().expect("the hold lists").is_empty());
}
