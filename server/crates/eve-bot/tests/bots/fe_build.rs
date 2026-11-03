//! **FE-BUILD / DOC-7, over a socket**: a doctrine wants a plate, and the
//! builder -- which has no plate on its ladder -- buys the plate's blueprint
//! from the NPC that sells it, buys the plate's minerals, and installs the
//! job. The leaders' side is the [`PartsBoard`] in the content tables; a
//! wanted part with no source is logged once and gets no rung.
//!
//! Invented ids only (the no-client-data rule); the plate's blueprint is
//! none the ladder names.

use crate::common::{config, gm, login, serve};
use eve_bot::Conn;
use eve_bot::builder;
use eve_bot::doctrine_parts::PartsBoard;
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::industry;
use eve_server::model::Character;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

const BUILDER: i64 = 90_000_301;
const ALICE: i64 = 90_000_302;
const CORP: i64 = 1_999_611;
const PLATE: i64 = 990_400_001;
const GHOST: i64 = 990_400_002;
const MINERAL_A: i64 = 990_400_101;
const MINERAL_B: i64 = 990_400_102;
const PLATE_BP: i64 = 990_400_201;
const PLATE_GROUP: i64 = 99_084;
const BLUEPRINT_GROUP: i64 = 99_106;
const MINERAL_GROUP: i64 = 18;
const PLATE_REF: f64 = 100.0;
const A_REF: f64 = 4.0;
const B_REF: f64 = 10.0;
const BLUEPRINT_REF: f64 = 100_000.0;
const A_PER_RUN: i64 = 50;
const B_PER_RUN: i64 = 5;
const WALLET: f64 = 1_000_000.0;

fn content() -> Content {
    let types = format!(
        "{}\
         type\t{PLATE}\t{PLATE_GROUP}\t7\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Plate\n\
         type\t{GHOST}\t{PLATE_GROUP}\t7\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Ghost Part\n\
         type\t{MINERAL_A}\t{MINERAL_GROUP}\t4\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Mineral A\n\
         type\t{MINERAL_B}\t{MINERAL_GROUP}\t4\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Mineral B\n\
         type\t{PLATE_BP}\t{BLUEPRINT_GROUP}\t9\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Plate Blueprint\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PLATE}\t{PLATE_REF}\trecipe\t{PLATE_REF}\t7\t{PLATE_GROUP}\t1\t1"),
        format!("{GHOST}\t{PLATE_REF}\tbase\t{PLATE_REF}\t7\t{PLATE_GROUP}\t\t1"),
        format!("{MINERAL_A}\t{A_REF}\tbase\t{A_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{MINERAL_B}\t{B_REF}\tbase\t{B_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{PLATE_BP}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let t = 1;
    let recipes = industry::Table::parse(
        &[
            format!("{PLATE_BP}\t1\tactivity\t0\t0\t\t{t}\t10"),
            format!("{PLATE_BP}\t1\tmaterial\t{MINERAL_A}\t{A_PER_RUN}\t\t{t}\t10"),
            format!("{PLATE_BP}\t1\tmaterial\t{MINERAL_B}\t{B_PER_RUN}\t\t{t}\t10"),
            format!("{PLATE_BP}\t1\tproduct\t{PLATE}\t10\t\t{t}\t10"),
            format!(
                "{PLATE_BP}\t1\tskill\t{}\t1\t\t{t}\t10",
                industry::SKILL_INDUSTRY
            ),
        ],
        &prices,
    )
    .expect("the recipe parses");
    let home = Character::default();
    let station = home.station_id.expect("the default pilot is docked");
    let exchange = exchange::Table::new(
        vec![exchange::Station {
            station_id: station,
            system_id: home.solar_system_id.expect("in a system"),
            region_id: home.region_id,
            owner_id: CORP,
        }],
        &[(CORP, PLATE_BP, 1.0)],
        [(
            PLATE_BP,
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
    c.insert("parts_board", PartsBoard::new());
    c
}

fn start() -> SocketAddr {
    let pilot = |id, name: &str, ship| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        ..Character::default()
    };
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(BUILDER, "Builder1", 100_000_301),
        pilot(ALICE, "Alice", 100_000_302),
    ];
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn alice_asks(addr: SocketAddr, type_id: i64, quantity: i64, price: f64) {
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {type_id} {quantity}"));
    let station = alice.session().station_id().expect("docked");
    let stack = alice
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id && r.location_id == station)
        .expect("the stack");
    alice
        .sell(
            &[builder::SellLine {
                station_id: station,
                type_id,
                item_id: stack.item_id,
                price,
                quantity,
            }],
            14,
            builder::BROKER_FEE,
        )
        .expect("Alice's ask");
}

fn builder_conn(addr: SocketAddr) -> Conn {
    let mut b = login(addr, "Builder1");
    gm(
        &mut b,
        &format!("/giveskill {} 1", industry::SKILL_INDUSTRY),
    );
    b
}

/// **DOC-7's socket test**: with no doctrine wanting a plate the builder
/// buys nothing (the plate is on no rung); once a leader's doctrine wants
/// plates the builder buys the plate's blueprint from the NPC, the minerals
/// from Alice, and installs the job. A wanted part nothing can supply is
/// logged once and gets no rung.
#[test]
fn builder_makes_a_plate_socket() {
    let addr = start();
    let content = content();
    alice_asks(addr, MINERAL_A, A_PER_RUN, A_REF);
    alice_asks(addr, MINERAL_B, B_PER_RUN, B_REF);
    let mut b = builder_conn(addr);
    let board: &PartsBoard = content.get();

    let quiet = builder::run(&mut b, &[], Some(&content)).expect("a cycle");
    assert_eq!(
        quiet.blueprint, None,
        "no doctrine, no plate rung: {quiet:?}"
    );
    assert!(quiet.installed.is_empty(), "{quiet:?}");

    board.publish(1, BTreeMap::from([(PLATE, 40), (GHOST, 7)]));
    let first = builder::run(&mut b, &[], Some(&content)).expect("a cycle");
    assert_eq!(first.blueprint, Some(PLATE_BP), "{first:?}");
    assert!(
        first
            .notes
            .iter()
            .any(|n| n.contains("no source") && n.contains(&GHOST.to_string())),
        "the ghost part is logged: {first:?}"
    );
    assert!(
        first
            .bought
            .iter()
            .any(|&(t, n, _)| t == MINERAL_A && n > 0),
        "{first:?}"
    );
    let [(_, bp, runs)] = first.installed[..] else {
        panic!("one job installed: {first:?}");
    };
    assert_eq!((bp, runs), (PLATE_BP, 1), "{first:?}");
    assert_eq!(first.picks.len(), 1, "{first:?}");

    let second = builder::run(&mut b, &[], Some(&content)).expect("a cycle");
    assert!(
        !second.notes.iter().any(|n| n.contains("no source")),
        "logged once only: {second:?}"
    );
}
