//! **FE-VALUE over a socket** (R-FLUIDECON §3.1-3.4): a bot reads the live
//! book (`GetOrders`) and `averagePrice` (`GetAverageMarketPrices`), forms
//! its perceived value `V`, and quotes inside the new caps. Two humans
//! rest orders; the reader's `V` and ask follow the book on its next cycle.
//!
//! **Synthetic furniture only**: one invented trade good, three invented
//! pilots, the sandbox's home station.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::pricing::{self, Prices, Quote};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::model::Character;
use std::sync::Arc;

const SELLER: i64 = 90_000_901;
const BUYER: i64 = 90_000_902;
const READER: i64 = 90_000_903;
const GOOD: i64 = 990_900_102;
const GOOD_GROUP: i64 = 99_290;
const GOOD_BASE: f64 = 10_000.0;
/// The seller's ask and the first bid (invented), then the deep bid.
const ASK: f64 = 12_000.0;
const BID: f64 = 11_000.0;
const DEEP_BID: f64 = 11_900.0;
const WALLET: f64 = 5.0e8;

fn home() -> Character {
    Character::default()
}

fn content() -> Content {
    let types = format!(
        "{}type\t{GOOD}\t{GOOD_GROUP}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Trade Good\n",
        eve_server::typedata::FIXTURE,
        cat = exchange::CATEGORY_COMMODITY,
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[format!(
        "{GOOD}\t{GOOD_BASE}\tbase\t{GOOD_BASE}\t{}\t{GOOD_GROUP}\t\t1",
        exchange::CATEGORY_COMMODITY
    )])
    .expect("the prices parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c
}

#[test]
fn a_bot_reads_the_live_book_and_the_average_and_quotes_inside_the_caps() {
    let mut cfg = config();
    let pilot = |id: i64, name: &str, ship: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        ..home()
    };
    cfg.sandbox.characters = vec![
        pilot(SELLER, "Seller", 100_000_901),
        pilot(BUYER, "Buyer", 100_000_902),
        pilot(READER, "Reader", 100_000_903),
    ];
    cfg.content = Some(Arc::new(content()));
    let addr = serve(cfg);
    let station = home().station_id.expect("docked");

    // Nothing rests yet: V is the reference price (cold start).
    let mut reader = login(addr, "Reader");
    let mut cold = Prices::read(&mut reader).expect("the index reads");
    let p_ref = cold.p_ref(GOOD).expect("a reference price");
    assert_eq!(cold.value(&mut reader, GOOD).expect("reads"), Some(p_ref));
    assert!(cold.average(GOOD).is_some(), "averagePrice is read too");

    // A human asks, another bids.
    let mut s = login(addr, "Seller");
    gm(&mut s, &format!("/createitem {GOOD} 10"));
    let stack = s
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == GOOD)
        .expect("goods");
    s.sell(
        &[SellLine {
            station_id: station,
            type_id: GOOD,
            item_id: stack.item_id,
            price: ASK,
            quantity: 10,
        }],
        7,
        pricing::BROKER_FEE,
    )
    .expect("lists");
    let mut b = login(addr, "Buyer");
    b.buy(station, GOOD, BID, 10, 7, pricing::BROKER_FEE)
        .expect("bids");

    let mut first = Prices::read(&mut reader).expect("reads");
    let v1 = first.value(&mut reader, GOOD).expect("reads").expect("V");
    let mid = (ASK + BID) / 2.0;
    assert!(
        v1 > p_ref && v1 <= mid,
        "V {v1} follows the book mid {mid} from {p_ref}"
    );

    // The deep bid: the book's mid and pressure rise, so V and the ask do.
    b.buy(station, GOOD, DEEP_BID, 1_000, 7, pricing::BROKER_FEE)
        .expect("bids deep");
    let mut second = Prices::read(&mut reader).expect("reads");
    let v2 = second.value(&mut reader, GOOD).expect("reads").expect("V");
    assert!(
        v2 > v1,
        "a deeper bid lifts V within one cycle: {v1} -> {v2}"
    );
    let pressure = second.pressure(&mut reader, GOOD).expect("reads");
    assert!(pressure > 0.5, "bids outweigh asks: {pressure}");
    let quote = Quote {
        p_mkt: v2,
        pressure,
        ..Quote::at_ref(p_ref)
    };
    let ask = pricing::ask(&quote).expect("an ask");
    let (lo, hi) = pricing::band(p_ref).expect("band");
    assert!(ask >= lo && ask <= hi, "inside the band: {ask}");
    assert!(
        ask <= pricing::up_to_cent(v2 * (1.0 + pricing::FE_SCARCITY_K) * (1.0 - pricing::UNDERCUT)),
        "inside the scarcity cap: {ask}"
    );
    // V never strays from the book by more than the day cap from the EWMA, and
    // with no history it is only a blend of the book and the index.
    let flat = pricing::ask(&Quote {
        pressure: 0.0,
        ..quote
    })
    .expect("an ask");
    assert!(ask > flat, "bid pressure marks the ask up: {ask} vs {flat}");
}
