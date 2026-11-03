//! X-LPSTORE over a real socket: a mission runner is paid LP by its agent,
//! and at the desk turns it into ISK. It buys the input an offer takes on
//! the market, redeems the offer at the agent corporation's store, and
//! lists the product as an ask.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): one distribution agent
//! and courier mission whose `fixed_lp` pays the LP, one invented store
//! corporation (the agent's) with one offer, one invented product (a
//! module, so the escort's sale treats it as loot) and one invented input
//! (a commodity), both with reference prices. Every expected number is
//! computed from the offer and the named constants.

use super::common::{SYSTEM, config, gm, login, pilot, serve};
use eve_bot::builder::SellLine;
use eve_bot::lp_spend;
use eve_bot::missioner::{self, Errand, Step};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::mission_pool::{self as pool, Agent, Mission, Table};
use std::net::SocketAddr;
use std::sync::Arc;

const RUNNER: i64 = 90_000_811;
const SELLER: i64 = 90_000_812;
const GIVER: i64 = 3_998_811;
const ADDRESSEE: i64 = 3_998_812;
const HAUL: i64 = 998_811;
const DISTRIBUTION: i64 = 22;
/// The agent's corporation, whose store the LP buys from.
const CORP: i64 = 1_998_811;
const TABLE: i64 = 9_981;
const OFFER: i64 = 99_810_000;
const PRODUCT: i64 = 990_811;
const INPUT: i64 = 990_812;
const PRODUCT_GROUP: i64 = 99_811;
const INPUT_GROUP: i64 = 99_812;
const PRODUCT_BASE: f64 = 200_000.0;
const INPUT_BASE: f64 = 1_000.0;
/// What one redeem costs and takes.
const LP_COST: i64 = 100;
const ISK_COST: i64 = 1_000;
const INPUT_EACH: i64 = 5;
/// What the mission pays: three redeems' worth of LP.
const REDEEMS: i64 = 3;
const FIXED_LP: i64 = LP_COST * REDEEMS;
const WALLET: f64 = 5_000_000.0;
const STOCK: i64 = 100;

fn home() -> i64 {
    eve_server::model::Character::default()
        .station_id
        .expect("the default character is docked")
}

fn pool() -> Table {
    let fixture = Table::fixture();
    let mut agents: Vec<Agent> = fixture.agents().cloned().collect();
    // The giver is in space; the addressee sits in the home station, so the
    // haul's drop-off is the one station this server docks a ship in.
    let agent = |id, station| Agent {
        id,
        agent_type: pool::TYPE_BASIC,
        division: DISTRIBUTION,
        level: 1,
        corp: CORP,
        faction: 598_001,
        station,
        system: Some(SYSTEM),
        ..Agent::default()
    };
    agents.push(agent(GIVER, None));
    agents.push(agent(ADDRESSEE, Some(home())));
    let mut missions: Vec<Mission> = fixture.missions().cloned().collect();
    missions.push(Mission {
        id: HAUL,
        name_id: 998_911,
        template: "BasicCourier".into(),
        family: "courier".into(),
        level: Some(1),
        obj_type: Some(eve_server::mission::CARGO_TYPE_ID),
        obj_qty: Some(1),
        fixed_lp: FIXED_LP,
        ..Mission::default()
    });
    Table::new(agents, missions, pool::Policy::TRANQUILITY)
}

fn content() -> Content {
    let types = format!(
        "{}\
         type\t{PRODUCT}\t{PRODUCT_GROUP}\t7\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture LP Product\n\
         type\t{INPUT}\t{INPUT_GROUP}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture LP Input\n",
        eve_server::typedata::FIXTURE,
        cat = exchange::CATEGORY_COMMODITY,
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PRODUCT}\t{PRODUCT_BASE}\tbase\t{PRODUCT_BASE}\t7\t{PRODUCT_GROUP}\t\t1"),
        format!(
            "{INPUT}\t{INPUT_BASE}\tbase\t{INPUT_BASE}\t{}\t{INPUT_GROUP}\t\t1",
            exchange::CATEGORY_COMMODITY
        ),
    ])
    .expect("the prices parse");
    let offer =
        format!("{TABLE}\t{OFFER}\t{PRODUCT}\t1\t{LP_COST}\t{ISK_COST}\t{INPUT}={INPUT_EACH}");
    let corp = format!("{CORP}\t\t\t\t\t\t0\t0\t0\t\t\t{TABLE}\tLPS\tStore");
    let store = eve_server::loyalty::Table::parse(&[offer], &[corp]).expect("the store parses");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("loyalty", store);
    c.insert("mission_pool", pool());
    c
}

fn start() -> SocketAddr {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        eve_server::model::Character {
            balance: WALLET,
            ..pilot(RUNNER, "Runner", 100_000_811)
        },
        eve_server::model::Character {
            balance: WALLET,
            ..pilot(SELLER, "Seller", 100_000_812)
        },
    ];
    cfg.content = Some(Arc::new(content()));
    serve(cfg)
}

fn lp_with(c: &mut eve_bot::Conn, corp: i64) -> i64 {
    let v = c
        .call("LPSvc", "GetAllMyCharacterWalletLPBalances", vec![])
        .expect("answered")
        .value()
        .unwrap_or(eve_marshal::PyValue::None);
    eve_bot::plexer::read_lp(&v, corp)
}

/// **The packet's test.** A seller stocks the input as an ask. The runner
/// turns in a mission that pays [`FIXED_LP`] and, in the same desk visit,
/// buys the input, redeems the offer [`REDEEMS`] times at the agent's
/// store, and lists the product.
#[test]
fn a_mission_runner_spends_its_lp_and_lists_the_product() {
    let addr = start();

    let mut s = login(addr, "Seller");
    gm(&mut s, &format!("/createitem {INPUT} {STOCK}"));
    let stack = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == INPUT)
        .expect("the seller's stock");
    s.sell(
        &[SellLine {
            station_id: home(),
            type_id: INPUT,
            item_id: stack.item_id,
            price: INPUT_BASE,
            quantity: stack.quantity,
        }],
        eve_bot::escort::LIST_DAYS,
        eve_bot::escort::LIST_BROKER_FEE,
    )
    .expect("the seller lists");

    let mut c = login(addr, "Runner");
    gm(&mut c, "/createitem merlin");
    assert_eq!(lp_with(&mut c, CORP), 0, "no LP before the mission");
    let errand = Errand {
        agent: Some(GIVER),
        level: 0,
        trips: None,
    };
    let report = missioner::run(&mut c, &errand).expect("the desk visit");
    let note = report
        .steps
        .iter()
        .find_map(|s| match s {
            Step::LpSpent { note } => Some(note.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("an LP spend step: {}", report.summary()));
    assert!(
        note.contains(&format!("offer {OFFER} x{REDEEMS}")),
        "{note}"
    );

    assert_eq!(lp_with(&mut c, CORP), 0, "all the LP was spent: {note}");
    let held = |c: &mut eve_bot::Conn, t: i64| -> i64 {
        c.hangar_items()
            .expect("the hangar lists")
            .iter()
            .filter(|r| r.type_id == t)
            .map(|r| r.quantity)
            .sum()
    };
    assert_eq!(held(&mut c, INPUT), 0, "the input went into the offers");
    assert_eq!(held(&mut c, PRODUCT), 0, "the product went to market");
    let asks: i64 = c
        .my_orders()
        .expect("the orders read")
        .iter()
        .filter(|o| !o.bid && o.type_id == PRODUCT && o.station_id == home())
        .map(|o| o.vol_remaining)
        .sum();
    assert_eq!(asks, REDEEMS, "the product is listed: {note}");
    let book = s.market_orders(PRODUCT).expect("the book reads");
    assert!(
        book.asks.iter().any(|a| a.vol_remaining == REDEEMS),
        "another pilot sees the ask: {book:?}"
    );
    // The input came off the seller's ask, unit for unit.
    let left: i64 = s
        .market_orders(INPUT)
        .expect("the book reads")
        .asks
        .iter()
        .filter(|a| a.station_id == home())
        .map(|a| a.vol_remaining)
        .sum();
    assert_eq!(left, STOCK - REDEEMS * INPUT_EACH, "bought what it took");
}

/// With no LP the spend is a note and nothing is bought.
#[test]
fn with_no_lp_a_bot_buys_nothing() {
    let addr = start();
    let mut c = login(addr, "Runner");
    let spend = lp_spend::spend(&mut c, None).expect("a visit");
    assert!(spend.redeemed.is_empty(), "{}", spend.summary());
    assert_eq!(spend.lp, 0);
}
