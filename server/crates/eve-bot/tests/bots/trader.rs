//! **The trader** (SOC-B8) over a socket: a good that a player sells
//! cheap at the home station and an NPC bids for one jump away is bought,
//! hauled through the gate, sold into the bid, and the trader flies home.
//!
//! **Synthetic furniture only** (the no-client-data rule): two systems
//! with invented gates and an invented second station (the travel test's
//! layout), one invented trade good, and an invented NPC corporation that
//! owns the far station and bids for the good (E5b: a negative
//! `corporationTrades` entry in a bid category). A third character,
//! seeded at the far station, makes that station one this world knows.

use crate::common::{config, gm, login, serve};
use eve_bot::builder::SellLine;
use eve_bot::trader::{self, Costs, Setup, SkillUse};
use eve_bot::travel::{Map, SECURITY_FLOOR, Trips};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;

// Invented ids and numbers only.
const TRADER: i64 = 90_000_401;
const SELLER: i64 = 90_000_402;
const CAROL: i64 = 90_000_403;
const CORP: i64 = 1_999_801;
const B: i64 = 30_000_004;
const STATION_B: i64 = 60_900_101;
const GATE_A: i64 = 50_900_101;
const GATE_B: i64 = 50_900_102;
const GATE_TYPE: i64 = 990_400_101;
const GOOD: i64 = 990_400_102;
const GOOD_GROUP: i64 = 99_280;
const GOOD_BASE: f64 = 10_000.0;
/// The seller's price: a tenth of what the NPC pays.
const CHEAP: f64 = 1_000.0;
const UNITS: i64 = 100;
/// The fixture frigate of `typedata::FIXTURE`: a hull with a hold.
const FRIGATE: i64 = 990_000_001;
const WALLET: f64 = 1_000_000.0;
const SECURE: f64 = 1.0;

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

/// `Content::fixture()` plus the good (1 m3 a unit), its reference price,
/// and the corporation that bids for it at the far station only.
fn content() -> Content {
    let types = format!(
        "{}\
         type\t{GOOD}\t{GOOD_GROUP}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Trade Good\n",
        eve_server::typedata::FIXTURE,
        cat = exchange::CATEGORY_COMMODITY,
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[format!(
        "{GOOD}\t{GOOD_BASE}\tbase\t{GOOD_BASE}\t{}\t{GOOD_GROUP}\t\t1",
        exchange::CATEGORY_COMMODITY
    )])
    .expect("the prices parse");
    let table = exchange::Table::new(
        vec![exchange::Station {
            station_id: STATION_B,
            system_id: B,
            region_id: home().region_id,
            owner_id: CORP,
        }],
        &[(CORP, GOOD, -1.0)],
        [(
            GOOD,
            exchange::TypeRow {
                group_id: GOOD_GROUP,
                category_id: exchange::CATEGORY_COMMODITY,
                base_price: GOOD_BASE,
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
    c.insert("econ_exchange", table);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: TRADER,
            name: "Trader1".to_owned(),
            ship_id: 100_000_401,
            balance: WALLET,
            ..home()
        },
        Character {
            character_id: SELLER,
            name: "Seller".to_owned(),
            ship_id: 100_000_402,
            balance: WALLET,
            ..home()
        },
        Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_403,
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn map() -> Map {
    Map::from_catalogue(&catalogue(), None)
        .with_security(system_a(), SECURE)
        .with_security(B, SECURE)
}

/// **The lane's harness test**: the seller lists the good cheap at home;
/// the NPC bids for it one jump away. The trader's cycle picks that deal,
/// buys every unit at home, flies through the gate, sells into the bid,
/// and flies home: two jumps, the hold empty, the wallet up.
#[test]
fn a_trader_buys_at_a_and_sells_into_a_bid_one_jump_away() {
    let addr = start();

    let mut s = login(addr, "Seller");
    gm(&mut s, &format!("/createitem {GOOD} {UNITS}"));
    let stack = s
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == GOOD)
        .expect("the seller's goods");
    s.sell(
        &[SellLine {
            station_id: station_a(),
            type_id: GOOD,
            item_id: stack.item_id,
            price: CHEAP,
            quantity: stack.quantity,
        }],
        trader::LIST_DAYS,
        trader::BROKER_FEE,
    )
    .expect("the seller lists");

    let mut t = login(addr, "Trader1");
    gm(&mut t, &format!("/createitem {FRIGATE}"));
    let hull = t
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    t.board(hull.item_id).expect("boards the frigate");

    let book = t.market_orders(GOOD).expect("the book reads");
    assert!(
        book.asks.iter().any(|a| a.station_id == station_a()),
        "{book:?}"
    );
    assert!(
        book.bids.iter().any(|b| b.station_id == STATION_B),
        "the NPC bids at B: {book:?}"
    );

    let map = map();
    let content = content();
    let setup = Setup {
        trips: Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        },
        content: Some(&content),
        types: &[GOOD],
        offset: 0,
        couriers: false,
        costs: Costs::default(),
        skills: SkillUse::Given,
    };
    let got = trader::run(&mut t, &setup).expect("a cycle");

    let deal = got.deal.expect("a deal");
    assert_eq!(deal.type_id, GOOD, "{got:?}");
    assert_eq!(
        (deal.buy_station, deal.sell_station),
        (station_a(), STATION_B),
        "{got:?}"
    );
    assert_eq!(deal.jumps, 2, "{got:?}");
    assert_eq!(got.bought, UNITS, "{}", got.summary());
    assert_eq!(got.sold, UNITS, "{}", got.summary());
    assert_eq!(got.jumps, 2, "one jump out, one back: {}", got.summary());
    assert!(got.after > got.before, "{}", got.summary());
    assert_eq!(
        t.session().station_id(),
        Some(station_a()),
        "docked at home"
    );
    assert!(
        t.cargo().expect("the hold lists").is_empty(),
        "the hold is empty"
    );
}

// ---- X-TRADE: the skills over a socket --------------------------------------------
//
// The NPC trade-goods loop at the home and far stations (an NPC corporation
// asks the good at home at `TRADE_GOOD_ASK` x base and another bids
// `TIER_A_BID` x base one jump away), and the trade skills as rows of the
// skill table so `/giveskill` knows them. Invented ids; the skills' type ids
// are the client's (`market_fees::TYPE_*`).

use eve_server::econ_exchange::{TIER_A_BID, TRADE_GOOD_ASK};
use eve_server::market_fees::{
    self as fees, TYPE_ACCOUNTING, TYPE_BROKER_RELATIONS, TYPE_TRADE, sales_tax_rate,
};

const SKILLED: i64 = 90_000_411;
const PLAIN: i64 = 90_000_412;
const HUMAN: i64 = 90_000_413;
const ASKER: i64 = 1_999_811;

/// The skill table: the trade skills (invented rank and attributes), the
/// prerequisites as the client's table has them.
fn skills() -> eve_server::skill::Table {
    let rows = format!(
        "{}skill\t{TYPE_ACCOUNTING}\t990400200\t3\t164\t166\t0.01\t{TYPE_TRADE}=4\tFixture Accounting\n\
         skill\t{TYPE_BROKER_RELATIONS}\t990400200\t2\t164\t166\t0.01\t{TYPE_TRADE}=2\tFixture Broker Relations\n",
        eve_server::skill::FIXTURE,
    );
    eve_server::skill::Table::parse(&rows).expect("the skill rows parse")
}

/// The loop: NPC asks at home, NPC bids at B. The price row's refine floor
/// is 0 (a trade good is not refined), or the server's `ASK_REFINE_GUARD`
/// would lift the ask over the floor.
fn loop_content() -> Content {
    let types = format!(
        "{}\
         type\t{GOOD}\t{GOOD_GROUP}\t{cat}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Trade Good\n",
        eve_server::typedata::FIXTURE,
        cat = exchange::CATEGORY_COMMODITY,
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[format!(
        "{GOOD}\t{GOOD_BASE}\tbase\t0.0\t{}\t{GOOD_GROUP}\t\t1",
        exchange::CATEGORY_COMMODITY
    )])
    .expect("the prices parse");
    let table = exchange::Table::new(
        vec![
            exchange::Station {
                station_id: station_a(),
                system_id: system_a(),
                region_id: home().region_id,
                owner_id: ASKER,
            },
            exchange::Station {
                station_id: STATION_B,
                system_id: B,
                region_id: home().region_id,
                owner_id: CORP,
            },
        ],
        &[(ASKER, GOOD, 1.0), (CORP, GOOD, -1.0)],
        [(
            GOOD,
            exchange::TypeRow {
                group_id: GOOD_GROUP,
                category_id: exchange::CATEGORY_COMMODITY,
                base_price: GOOD_BASE,
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
    c.insert("econ_exchange", table);
    c.insert(eve_server::skill::TABLE.name, skills());
    c
}

fn start_loop() -> SocketAddr {
    let mut c = config();
    let pilot = |id: i64, name: &str, ship: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        ..home()
    };
    c.sandbox.characters = vec![
        pilot(SKILLED, "Skilled", 100_000_411),
        pilot(PLAIN, "Plain", 100_000_412),
        pilot(HUMAN, "Human", 100_000_413),
        Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_403,
            station_id: Some(STATION_B),
            solar_system_id: Some(B),
            ..home()
        },
    ];
    c.sandbox.celestials = catalogue();
    c.content = Some(Arc::new(loop_content()));
    serve(c)
}

fn board_frigate(t: &mut eve_bot::Conn) {
    gm(t, &format!("/createitem {FRIGATE}"));
    let hull = t
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate");
    t.board(hull.item_id).expect("boards the frigate");
}

/// **The loop pays only with the skills, over a socket**: the same cycle,
/// the same book, two traders. The one with Accounting IV reads its skills,
/// prices the 8 % tax down to 4.48 %, flies the NPC trade-goods loop and
/// ends richer by exactly the units times the net; the one with no skills
/// finds the loop net-negative and takes nothing (its wallet is unchanged).
#[test]
fn a_trader_with_accounting_flies_the_npc_loop_and_one_without_stays_home() {
    let addr = start_loop();
    let (map, content) = (map(), loop_content());
    let setup = |t: &'static [i64]| Setup {
        trips: Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        },
        content: Some(&content),
        types: t,
        offset: 0,
        couriers: false,
        // Trips cost a little ISK so the margin is the skills' work alone.
        costs: Costs {
            isk_per_jump: 100.0,
            min_profit: 1.0,
            ..Costs::default()
        },
        skills: SkillUse::Own,
    };

    // The control: no skills. The loop loses, so there is no deal.
    let mut p = login(addr, "Plain");
    board_frigate(&mut p);
    let got = trader::run(&mut p, &setup(&[GOOD])).expect("a cycle");
    assert_eq!(got.deal, None, "{got:?}");
    assert_eq!(got.after, got.before, "nothing bought, nothing lost");
    let book = p.market_orders(GOOD).expect("the book reads");
    assert!(
        book.asks.iter().any(|a| a.station_id == station_a())
            && book.bids.iter().any(|b| b.station_id == STATION_B),
        "the loop exists, so the refusal is the tax's: {book:?}"
    );

    // The skilled trader: Accounting IV (GM-given: the harness, not the bot).
    let mut t = login(addr, "Skilled");
    gm(&mut t, &format!("/giveskill {TYPE_TRADE} 4"));
    gm(&mut t, &format!("/giveskill {TYPE_ACCOUNTING} 4"));
    board_frigate(&mut t);
    let got = trader::run(&mut t, &setup(&[GOOD])).expect("a cycle");
    let deal = got
        .deal
        .unwrap_or_else(|| panic!("Accounting IV makes the loop pay: {got:?}"));
    assert_eq!(deal.type_id, GOOD, "{}", got.summary());
    assert_eq!(
        (deal.buy_station, deal.sell_station),
        (station_a(), STATION_B)
    );
    assert!(
        got.bought > 0 && got.sold == got.bought,
        "{}",
        got.summary()
    );
    let (ask, bid) = (GOOD_BASE * TRADE_GOOD_ASK, GOOD_BASE * TIER_A_BID);
    let net = got.sold as f64 * (bid * (1.0 - sales_tax_rate(4)) - ask);
    assert!(
        (got.after - got.before - net).abs() < 1.0,
        "wallet {} -> {}, expected {net:+.2}",
        got.before,
        got.after
    );
    assert!(net > 0.0, "the loop made money");
}

/// **The fee the skills lower, over a socket**: two pilots list the same
/// stack at the same price; the one with Broker Relations V pays the
/// server's lower commission, the other the base 3 % -- the numbers the
/// trader's price rule uses (`market_fees`).
#[test]
fn a_pilot_with_broker_relations_pays_the_lower_listing_fee() {
    let addr = start_loop();
    let list = |name: &str, br: i64| -> f64 {
        let mut c = login(addr, name);
        if br > 0 {
            gm(&mut c, &format!("/giveskill {TYPE_BROKER_RELATIONS} {br}"));
        }
        gm(&mut c, &format!("/createitem {GOOD} {UNITS}"));
        let stack = c
            .hangar_items()
            .expect("the hangar lists")
            .into_iter()
            .find(|r| r.type_id == GOOD)
            .expect("the goods");
        let before = c.cash_balance().expect("a balance");
        c.sell(
            &[SellLine {
                station_id: station_a(),
                type_id: GOOD,
                item_id: stack.item_id,
                price: CHEAP,
                quantity: stack.quantity,
            }],
            trader::LIST_DAYS,
            trader::BROKER_FEE,
        )
        .expect("lists");
        before - c.cash_balance().expect("a balance")
    };
    let value = UNITS as f64 * CHEAP;
    let plain = list("Plain", 0);
    let skilled = list("Skilled", 5);
    let rate = |br| fees::broker_rate(br, fees::Standings::default());
    assert!((plain - value * rate(0)).abs() < 0.01, "{plain}");
    assert!((skilled - value * rate(5)).abs() < 0.01, "{skilled}");
    assert!(skilled < plain);
}

/// **The trader learns, over a socket**: a fresh trader with a million ISK
/// finds Trade IV is the plan's first step, buys the book from a human's
/// ask at home, injects it and queues it -- the same `learn_skills` path
/// the builder uses -- and prices that cycle at the rates it has.
#[test]
fn a_fresh_trader_buys_and_queues_the_first_plan_step() {
    let addr = start_loop();
    let mut h = login(addr, "Human");
    gm(&mut h, &format!("/createitem {TYPE_TRADE} 1"));
    let book = h
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == TYPE_TRADE)
        .expect("the book");
    h.sell(
        &[SellLine {
            station_id: station_a(),
            type_id: TYPE_TRADE,
            item_id: book.item_id,
            price: 5_000.0,
            quantity: 1,
        }],
        trader::LIST_DAYS,
        trader::BROKER_FEE,
    )
    .expect("the human lists the book");

    let (map, content) = (map(), loop_content());
    let mut t = login(addr, "Plain");
    board_frigate(&mut t);
    let setup = Setup {
        trips: Trips {
            map: &map,
            floor: SECURITY_FLOOR,
        },
        content: Some(&content),
        types: &[GOOD],
        offset: 0,
        couriers: false,
        costs: Costs::default(),
        skills: SkillUse::Own,
    };
    let before = eve_bot::acquire::skill_levels(&mut t).expect("skills read");
    assert!(!before.contains_key(&TYPE_TRADE));
    let got = trader::run(&mut t, &setup).expect("a cycle");
    let after = eve_bot::acquire::skill_levels(&mut t).expect("skills read");
    assert!(
        after.contains_key(&TYPE_TRADE),
        "the Trade book is injected: {}",
        got.summary()
    );
    assert!(
        got.notes.iter().any(|n| n.contains("queued skills")),
        "{:?}",
        got.notes
    );
}
