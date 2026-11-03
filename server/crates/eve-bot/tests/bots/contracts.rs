//! X-CONTRACT over a real socket: a player puts goods up as an item exchange
//! contract, a bot's trader finds it in the Search tab's call and accepts it,
//! the goods and the ISK change hands once, and both journals show it. A
//! builder bot's own offer is the other half: a blueprint copy it has no use
//! for goes up at the asking share, public, and a trader leaves it alone.
//!
//! Invented rows only: two invented types and invented reference prices.

use super::common::{config, gm, login, pilot, serve};
use eve_bot::Conn;
use eve_bot::contracts::{self, Career, Offer};
use eve_marshal::{DbValue, PyValue};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

const ALICE: i64 = 90_000_901;
const BOB: i64 = 90_000_902;
const GOOD: i64 = 990_901_001;
const BP: i64 = 990_901_002;
/// An invented reference price per unit.
const UNIT: f64 = 1_000.0;
const BP_REF: f64 = 80_000.0;
const WALLET: f64 = 5_000_000.0;
const UNITS: i64 = 100;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{GOOD}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Contract Good\n\
         type\t{BP}\t99991\t9\t1\t100\t0.0\t0.0\t0.01\t1.0\t0\t\t\tFixture Contract Copy\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let mut prices = eve_server::econ_prices::Table::fixture();
    prices.prices.extend(
        eve_server::econ_prices::Table::parse(&[
            format!("{GOOD}\t{UNIT}\tbase\t{UNIT}\t6\t99010\t\t1"),
            format!("{BP}\t{BP_REF}\tbase\t{BP_REF}\t6\t99010\t\t1"),
        ])
        .expect("parses")
        .prices,
    );
    content.insert("econ_prices", prices);
    Arc::new(content)
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_901),
        eve_server::model::Character {
            balance: WALLET,
            ..pilot(BOB, "Bob", 100_000_902)
        },
    ];
    config.content = Some(content());
    serve(config)
}

fn units(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

fn stack(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .iter()
        .find(|r| r.type_id == type_id)
        .expect("the stack is there")
        .item_id
}

/// Every `(entryTypeID, referenceID, amount)` in the personal journal.
fn journal(c: &mut Conn) -> Vec<(i64, i64, f64)> {
    fn walk(v: &PyValue, out: &mut Vec<(i64, i64, f64)>) {
        match v {
            PyValue::DbRow(row) => {
                let get = |name: &str| {
                    row.descriptor()
                        .columns()
                        .iter()
                        .position(|c| c.name == name.as_bytes())
                        .and_then(|i| match row.values().get(i) {
                            Some(DbValue::Int(n)) => Some(*n),
                            _ => None,
                        })
                };
                if let (Some(t), Some(r), Some(a)) =
                    (get("entryTypeID"), get("referenceID"), get("amount"))
                {
                    out.push((t, r, a as f64 / 10_000.0));
                }
            }
            PyValue::List(xs) | PyValue::Tuple(xs) => xs.iter().for_each(|x| walk(x, out)),
            PyValue::Dict(kv) => kv.iter().for_each(|(a, b)| {
                walk(a, out);
                walk(b, out)
            }),
            PyValue::Instance { state, .. } => walk(state, out),
            PyValue::ObjectEx {
                header, list, dict, ..
            } => {
                walk(header, out);
                list.iter().for_each(|x| walk(x, out));
                dict.iter().for_each(|(a, b)| {
                    walk(a, out);
                    walk(b, out)
                });
            }
            _ => {}
        }
    }
    let out = c
        .call(
            "account",
            "GetTransactions",
            vec![
                PyValue::Int(1000),
                PyValue::None,
                PyValue::None,
                PyValue::Bool(false),
            ],
        )
        .expect("GetTransactions");
    let mut rows = Vec::new();
    if let Some(v) = out.value() {
        walk(&v, &mut rows);
    }
    rows
}

fn prices() -> BTreeMap<i64, f64> {
    BTreeMap::from([(GOOD, UNIT), (BP, BP_REF)])
}

#[test]
fn a_bot_finds_a_players_bargain_and_both_wallets_and_hangars_move() {
    let addr = start();
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {GOOD} {UNITS}"));
    let station = alice.session().station_id().expect("docked");
    let item = stack(&mut alice, GOOD);
    assert_eq!(units(&mut alice, GOOD), UNITS);

    // A bargain: well under the bot's UNDERPRICED_BELOW of the reference.
    let price = (UNIT * UNITS as f64 * contracts::UNDERPRICED_BELOW * 0.5).round();
    let id = contracts::create(
        &mut alice,
        station,
        &Offer {
            item_id: item,
            type_id: GOOD,
            quantity: UNITS,
            price,
        },
    )
    .expect("the call is answered")
    .expect("the server creates it");
    assert_eq!(units(&mut alice, GOOD), 0, "the goods are in escrow");
    let alice_before = alice.cash_balance().expect("a wallet");

    let mut bob = login(addr, "Bob");
    let listings = contracts::search_here(&mut bob, station).expect("searches");
    let found = listings
        .iter()
        .find(|l| l.contract_id == id)
        .expect("the search finds the contract");
    assert_eq!(found.price, price);
    assert_eq!(found.gives, vec![(GOOD, UNITS)]);

    let bob_before = bob.cash_balance().expect("a wallet");
    let taken = contracts::take_underpriced_with(&mut bob, &prices()).expect("accepts");
    assert_eq!(taken.contract_id, Some(id), "{}", taken.note);
    assert_eq!(taken.paid, price);

    // Items: Bob got them, or listed every one for sale (the trader's
    // resale), and in either case Alice's escrow is empty.
    assert_eq!(units(&mut alice, GOOD), 0);
    assert!(
        taken.listed > 0 || units(&mut bob, GOOD) == UNITS,
        "{}",
        taken.note
    );
    // ISK: once each way.
    // The price, and the market's broker fee on the resale listing.
    let fee = if taken.listed > 0 {
        UNIT * contracts::ASK_SHARE * UNITS as f64 * eve_bot::builder::BROKER_FEE
    } else {
        0.0
    };
    let bob_after = bob.cash_balance().expect("a wallet");
    assert!(
        (bob_after - (bob_before - price - fee)).abs() < 1.0,
        "Bob {bob_before} -> {bob_after}, price {price}, fee {fee}"
    );
    assert_eq!(
        alice.cash_balance().expect("a wallet"),
        alice_before + price
    );
    // Journals: a row naming the contract, for the price (the entry type is
    // the server's [A]; the contract id as the reference is not).
    let (a, b) = (journal(&mut alice), journal(&mut bob));
    assert!(
        a.iter().any(|&(_, r, amt)| r == id && amt == price),
        "Alice's journal: {a:?}"
    );
    assert!(
        b.iter().any(|&(_, r, amt)| r == id && amt == -price),
        "Bob's journal: {b:?}"
    );
    // Taken: it is no longer in the Search tab, and a second pass finds nothing.
    assert!(
        contracts::search_here(&mut bob, station)
            .expect("searches")
            .iter()
            .all(|l| l.contract_id != id)
    );
    let again = contracts::take_underpriced_with(&mut bob, &prices()).expect("again");
    assert_eq!(again.contract_id, None);
}

#[test]
fn a_builders_unwanted_copy_goes_up_public_at_the_asking_share_and_is_not_a_bargain() {
    let addr = start();
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {BP}"));
    assert_eq!(units(&mut alice, BP), 1);
    let note = contracts::offer_surplus(&mut alice, Career::Builder).expect("offers");
    assert!(note.contains("1 offered"), "{note}");
    assert_eq!(units(&mut alice, BP), 0, "the copy is in escrow");

    let station = alice.session().station_id().expect("docked");
    let mut bob = login(addr, "Bob");
    let listings = contracts::search_here(&mut bob, station).expect("searches");
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].gives, vec![(BP, 1)]);
    assert_eq!(listings[0].price, (BP_REF * contracts::ASK_SHARE).round());
    assert_eq!(listings[0].issuer_id, ALICE);
    // The asking share is above the trader's bargain line: nothing taken.
    let taken = contracts::take_underpriced_with(&mut bob, &prices()).expect("looks");
    assert_eq!(taken.contract_id, None, "{}", taken.note);

    // A second pass offers nothing more and keeps the one contract open.
    let again = contracts::offer_surplus(&mut alice, Career::Builder).expect("offers");
    assert!(again.contains("nothing to offer"), "{again}");
    assert_eq!(contracts::open_count(&mut alice, ALICE).expect("counts"), 1);
}

#[test]
fn a_bot_cannot_take_its_own_contract_or_one_it_cannot_afford() {
    let addr = start();
    let mut alice = login(addr, "Alice");
    gm(&mut alice, &format!("/createitem {GOOD} {UNITS}"));
    let station = alice.session().station_id().expect("docked");
    let item = stack(&mut alice, GOOD);
    // Priced at a tenth of the reference: a bargain to anyone else.
    let price = UNIT * UNITS as f64 * 0.1;
    contracts::create(
        &mut alice,
        station,
        &Offer {
            item_id: item,
            type_id: GOOD,
            quantity: UNITS,
            price,
        },
    )
    .expect("answered")
    .expect("created");
    // The issuer's own bargain is not taken (`pick_underpriced`'s own rule).
    let own = contracts::take_underpriced_with(&mut alice, &prices()).expect("looks");
    assert_eq!(own.contract_id, None, "{}", own.note);
    // A pilot whose wallet is under the price is not offered it.
    let poor = prices();
    let l = contracts::listings_of(&PyValue::None);
    assert!(l.is_empty());
    assert_eq!(
        contracts::pick_underpriced(
            &contracts::search_here(&mut login(addr, "Bob"), station).expect("searches"),
            &poor,
            price,
            BOB
        ),
        None,
        "the price is over the spend share of that wallet"
    );
}
