//! **GB-FIX over a socket**: a society-mode builder in a closed economy
//! whose first ladder blueprint needs a mineral no miner at home brings.
//!
//! The stage-B gate soak's seven builders each bought the EMP S and Miner I
//! blueprints (4 and 2 jumps out), rested bids for a Nocxium and a Mexallon
//! nobody could mine in a Veldspar-and-Scordite school system, and never
//! installed a job in 24 game hours. This is that world in miniature:
//!
//! - home A, whose rocks refine into mineral X only; far system B, one
//!   gate away, whose rocks refine into mineral Y;
//! - the ladder's first blueprint (`BLUEPRINT_LADDER[0]`, named, never its
//!   value) takes X and Y; the starter rung's first
//!   (`STARTER_AMMO[0]`) takes X only;
//! - an invented NPC corporation sells **both** blueprints at the far
//!   station, and nowhere else (so the builder must make a trip);
//! - the content carries [`Supply`] as `eve-bots` registers it.
//!
//! **Synthetic furniture only** (the no-client-data rule): every id,
//! number and name below is invented, as in `travel.rs` and `industry.rs`
//! (the harnesses this follows).

use crate::common::{config, gm, login, serve, until};
use eve_bot::Conn;
use eve_bot::builder::{self, BLUEPRINT_LADDER, BROKER_FEE, STARTER_AMMO, SellLine};
use eve_bot::supply::Supply;
use eve_bot::travel::{Map, SECURITY_FLOOR, Trips};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::industry;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids and numbers only.
const BUILDER: i64 = 90_000_401;
const ALICE: i64 = 90_000_402;
const CAROL: i64 = 90_000_403;
const CORP: i64 = 1_999_801;
const B: i64 = 30_000_004;
const STATION_B: i64 = 60_900_011;
const GATE_A: i64 = 50_900_011;
const GATE_B: i64 = 50_900_012;
const GATE_TYPE: i64 = 990_500_001;
const PRODUCT_E: i64 = 990_500_002;
const PRODUCT_S: i64 = 990_500_003;
const MINERAL_X: i64 = 990_500_101;
const MINERAL_Y: i64 = 990_500_102;
const ORE_HOME: i64 = 990_500_201;
const ORE_FAR: i64 = 990_500_202;
const PRODUCT_GROUP: i64 = 99_083;
const BLUEPRINT_GROUP: i64 = 99_105;
const MINERAL_GROUP: i64 = 18;
const PRODUCT_REF: f64 = 100.0;
const X_REF: f64 = 4.0;
const Y_REF: f64 = 10.0;
const BLUEPRINT_REF: f64 = 20_000.0;
/// One run: 50 X (and, for the ladder's own blueprint, 5 Y) make 10.
const X_PER_RUN: i64 = 50;
const Y_PER_RUN: i64 = 5;
const PER_RUN: i64 = 10;
const SECONDS_PER_RUN: i64 = 1;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 1_000_000.0;
const SECURE: f64 = 1.0;
const WITHIN: Duration = Duration::from_secs(30);

fn ladder_first() -> i64 {
    BLUEPRINT_LADDER[0]
}

fn starter() -> i64 {
    STARTER_AMMO[0]
}

fn home() -> Character {
    Character::default()
}

fn system_a() -> i64 {
    home().solar_system_id.expect("in a system")
}

fn station_a() -> i64 {
    home().station_id.expect("docked")
}

/// The two systems, a gate pair a thousand kilometres out, a station each.
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

fn supply() -> Supply {
    Supply::from_rocks([(system_a(), ORE_HOME), (B, ORE_FAR)], |ore| match ore {
        ORE_HOME => vec![MINERAL_X],
        _ => vec![MINERAL_Y],
    })
}

/// `Content::fixture()` plus the types, prices and recipes, and the
/// corporation that sells both blueprints at the far station only; with
/// `with_supply`, the mineral supply `eve-bots` registers.
fn content(with_supply: bool) -> Content {
    let (bp_e, bp_s) = (ladder_first(), starter());
    let mut types = eve_server::typedata::FIXTURE.to_owned();
    for (t, group, category, name) in [
        (PRODUCT_E, PRODUCT_GROUP, 8, "Fixture Product E"),
        (PRODUCT_S, PRODUCT_GROUP, 8, "Fixture Product S"),
        (MINERAL_X, MINERAL_GROUP, 4, "Fixture Mineral X"),
        (MINERAL_Y, MINERAL_GROUP, 4, "Fixture Mineral Y"),
        (bp_e, BLUEPRINT_GROUP, 9, "Fixture Blueprint E"),
        (bp_s, BLUEPRINT_GROUP, 9, "Fixture Blueprint S"),
    ] {
        types.push_str(&format!(
            "type\t{t}\t{group}\t{category}\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\t{name}\n"
        ));
    }
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PRODUCT_E}\t{PRODUCT_REF}\trecipe\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t1\t1"),
        format!("{PRODUCT_S}\t{PRODUCT_REF}\trecipe\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t1\t1"),
        format!("{MINERAL_X}\t{X_REF}\tbase\t{X_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{MINERAL_Y}\t{Y_REF}\tbase\t{Y_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{bp_e}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
        format!("{bp_s}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let t = SECONDS_PER_RUN;
    let skill = industry::SKILL_INDUSTRY;
    let recipes = industry::Table::parse(
        &[
            format!("{bp_e}\t1\tactivity\t0\t0\t\t{t}\t10"),
            format!("{bp_e}\t1\tmaterial\t{MINERAL_X}\t{X_PER_RUN}\t\t{t}\t10"),
            format!("{bp_e}\t1\tmaterial\t{MINERAL_Y}\t{Y_PER_RUN}\t\t{t}\t10"),
            format!("{bp_e}\t1\tproduct\t{PRODUCT_E}\t{PER_RUN}\t\t{t}\t10"),
            format!("{bp_e}\t1\tskill\t{skill}\t1\t\t{t}\t10"),
            format!("{bp_s}\t1\tactivity\t0\t0\t\t{t}\t10"),
            format!("{bp_s}\t1\tmaterial\t{MINERAL_X}\t{X_PER_RUN}\t\t{t}\t10"),
            format!("{bp_s}\t1\tproduct\t{PRODUCT_S}\t{PER_RUN}\t\t{t}\t10"),
            format!("{bp_s}\t1\tskill\t{skill}\t1\t\t{t}\t10"),
        ],
        &prices,
    )
    .expect("the recipes parse");
    let row = |bp| {
        (
            bp,
            exchange::TypeRow {
                group_id: BLUEPRINT_GROUP,
                category_id: exchange::CATEGORY_BLUEPRINT,
                base_price: BLUEPRINT_REF,
                meta_group_id: None,
                tech_level: None,
            },
        )
    };
    let exchange = exchange::Table::new(
        vec![exchange::Station {
            station_id: STATION_B,
            system_id: B,
            region_id: home().region_id,
            owner_id: CORP,
        }],
        &[(CORP, bp_e, 1.0), (CORP, bp_s, 1.0)],
        [row(bp_e), row(bp_s)].into_iter().collect(),
        &[],
    )
    .expect("the exchange builds");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("industry", recipes);
    c.insert("econ_exchange", exchange);
    if with_supply {
        c.insert("mineral_supply", supply());
    }
    c
}

fn start(with_supply: bool) -> SocketAddr {
    let pilot = |id, name: &str, ship| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        ..home()
    };
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(BUILDER, "Builder1", 100_000_401),
        pilot(ALICE, "Alice", 100_000_402),
        // Seeded at the far station, so this world knows it.
        Character {
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            ..pilot(CAROL, "Carol", 100_000_403)
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content(with_supply)));
    serve(c)
}

fn map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(system_a(), SECURE)
        .with_security(B, SECURE)
}

/// The builder, docked at home in a frigate with a hold (a player's kit)
/// and knowing Industry (a fixture line, as `industry.rs` does).
fn builder_conn(addr: SocketAddr) -> Conn {
    let mut b = login(addr, "Builder1");
    gm(
        &mut b,
        &format!("/giveskill {} 1", industry::SKILL_INDUSTRY),
    );
    gm(&mut b, &format!("/createitem {FRIGATE}"));
    let hull = b
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    b.board(hull.item_id).expect("boards the frigate");
    b
}

/// Alice, the miner's stand-in, lists `quantity` of `type_id` at home.
fn alice_asks(addr: SocketAddr, type_id: i64, quantity: i64, price: f64) {
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {type_id} {quantity}"));
    let station = alice.session().station_id().expect("docked");
    let stack = alice
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .expect("the stack");
    let line = SellLine {
        station_id: station,
        type_id,
        item_id: stack.item_id,
        price,
        quantity: stack.quantity,
    };
    alice
        .sell(&[line], builder::ORDER_DAYS, BROKER_FEE)
        .expect("Alice lists");
}

/// **The gate soak's builder, fixed.** With the supply table the builder
/// never takes the ladder's first blueprint (it needs Y, which no rock at
/// home refines into): it fetches the starter blueprint from the far
/// station, rests bids only for X, and once a miner's X is on the book,
/// installs its first job.
#[test]
fn a_builder_whose_miners_bring_only_x_fetches_the_starter_blueprint_and_installs() {
    let addr = start(true);
    let content = content(true);
    let mut b = builder_conn(addr);
    let map = map();
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };

    let first = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");
    assert_eq!(first.blueprint, Some(starter()), "{first:?}");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.starts_with(&format!("trip to station {STATION_B} (2 jumps)"))),
        "one jump out, one back: {first:?}"
    );
    assert_eq!(b.session().station_id(), Some(station_a()), "docked home");
    let held = b.blueprints().expect("the blueprints list");
    assert!(
        held.iter()
            .any(|r| r.type_id == starter() && r.location_id == station_a()),
        "the starter blueprint is in the home hangar: {held:?}"
    );
    assert!(
        held.iter().all(|r| r.type_id != ladder_first()),
        "the blueprint Y would block was never bought: {held:?}"
    );
    assert!(
        !first.bids.is_empty() && first.bids.iter().all(|&(t, _, _)| t == MINERAL_X),
        "bids only for what the miners bring: {first:?}"
    );
    let resting = b.my_open_orders().expect("my orders");
    assert!(
        resting.iter().all(|o| o.order.type_id != MINERAL_Y),
        "no standing bid for a mineral nobody here mines: {resting:?}"
    );
    assert!(first.installed.is_empty(), "no X to build from yet");

    // A miner's X reaches the book; the next cycle installs.
    alice_asks(addr, MINERAL_X, X_PER_RUN, X_REF);
    let second = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");
    let [(job_id, bp_type, runs)] = second.installed[..] else {
        panic!("the first job installed: {second:?}");
    };
    assert_eq!((bp_type, runs), (starter(), 1), "{second:?}");
    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "the job never reached its end time"
    );
    let third = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");
    assert_eq!(third.delivered, vec![(job_id, PRODUCT_S, 1)], "{third:?}");
}

/// **What the soak did**, kept as the contrast: with no supply table (no
/// bundle, so nothing is ruled out) the builder takes the ladder's first
/// blueprint and rests a bid for Y that nobody at home can fill. The
/// supply table is what changes this, and it is off only without a bundle.
#[test]
fn without_a_supply_table_the_builder_climbs_the_ladder_as_before() {
    let addr = start(false);
    let content = content(false);
    let mut b = builder_conn(addr);
    let map = map();
    let trips = Trips {
        map: &map,
        floor: SECURITY_FLOOR,
    };
    let first = builder::run_with(&mut b, &[], Some(&content), Some(trips)).expect("a cycle");
    assert_eq!(first.blueprint, Some(ladder_first()), "{first:?}");
    assert!(
        first.bids.iter().any(|&(t, _, _)| t == MINERAL_Y),
        "a bid for the mineral nobody mines: {first:?}"
    );
}
