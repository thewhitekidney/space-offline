//! **FE-DEMAND over a socket** (R-FLUIDECON §2, §7): a doctrine's bids appear
//! on the market with size, a builder reads them back as a demand map, and a
//! gatherer's target shifts toward what the bids pull.
//!
//! Two doctrines on invented rows (`typedata::FIXTURE`'s frigate, autocannon
//! and round; two invented minerals). Alice and Bob are members; Dave bids on
//! a mineral the way a builder's margin bids do; Reader only looks.

use crate::common::{config, login, pilot, serve};
use eve_bot::acquire::Order;
use eve_bot::demand::{self, DemandMap};
use eve_bot::doctrine::{Profile, RoleFit};
use eve_bot::hull_choice;
use eve_bot::pricing::{self, Prices};
use eve_server::damage::{Damage, Profile as Tank};
use eve_server::econ_exchange as exchange;
use std::collections::BTreeMap;
use std::sync::Arc;

const ALICE: i64 = 90_000_911;
const BOB: i64 = 90_000_912;
const DAVE: i64 = 90_000_913;
const READER: i64 = 90_000_914;
/// `typedata::FIXTURE`'s frigate, autocannon and ammo.
const FRIGATE: i64 = 990_000_001;
const GUN: i64 = 990_000_002;
const ROUND: i64 = 990_000_005;
/// The second doctrine's hull: an invented commodity standing in for a hull.
const HULL_B: i64 = 990_910_001;
const MIN_A: i64 = 990_910_101;
const MIN_B: i64 = 990_910_102;
const GROUP: i64 = 99_291;
const BASE: f64 = 10_000.0;
const WALLET: f64 = 5.0e9;
const ROUNDS: u32 = 60;

fn content() -> eve_server::content::Content {
    let mut types = eve_server::typedata::FIXTURE.to_owned();
    for t in [HULL_B, MIN_A, MIN_B] {
        types.push_str(&format!(
            "type\t{t}\t{GROUP}\t{}\t1\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t\tFixture Good {t}\n",
            exchange::CATEGORY_COMMODITY
        ));
    }
    let td = eve_server::typedata::Table::parse(&types).expect("the rows parse");
    let row = |t: i64, cat: i64| format!("{t}\t{BASE}\tbase\t{BASE}\t{cat}\t{GROUP}\t\t1");
    let prices = eve_server::econ_prices::Table::parse(&[
        row(FRIGATE, 6),
        row(GUN, 7),
        row(ROUND, 8),
        row(HULL_B, exchange::CATEGORY_COMMODITY),
        row(MIN_A, exchange::CATEGORY_COMMODITY),
        row(MIN_B, exchange::CATEGORY_COMMODITY),
    ])
    .expect("the prices parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c
}

fn fit(hull: i64, guns: u8) -> RoleFit {
    let l = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };
    RoleFit {
        hull,
        modules: vec![(GUN, guns)],
        charges: vec![(ROUND, ROUNDS)],
        skills: vec![],
        profile: Profile::blank(Tank {
            shield_capacity: 1.0,
            shield_recharge_rate: 0.0,
            armor_hp: 1.0,
            hp: 1.0,
            shield_resonance: l,
            armor_resonance: l,
            hull_resonance: l,
        }),
        cost: 1.0,
    }
}

#[test]
fn a_doctrine_switchs_bids_appear_on_the_market_and_a_gatherer_shifts() {
    let mut cfg = config();
    let rich = |id: i64, name: &str, ship: i64| {
        let mut p = pilot(id, name, ship);
        p.balance = WALLET;
        p
    };
    cfg.sandbox.characters = vec![
        rich(ALICE, "Alice", 100_000_911),
        rich(BOB, "Bob", 100_000_912),
        rich(DAVE, "Dave", 100_000_913),
        rich(READER, "Reader", 100_000_914),
    ];
    cfg.content = Some(Arc::new(content()));
    let addr = serve(cfg);

    let mut reader = login(addr, "Reader");
    let watch = [FRIGATE, GUN, ROUND, HULL_B, MIN_A, MIN_B];
    let mut prices = Prices::read(&mut reader).expect("the index reads");
    let empty = demand::map(&mut reader, &mut prices, &watch).expect("reads");
    assert!(watch.iter().all(|&t| empty.unmet(t) == 0), "a quiet market");

    // Doctrine A: both members have lost their hulls; each replaces by bid.
    let order_a = Order::from_fit(&fit(FRIGATE, 2));
    let mut placed_by_member = Vec::new();
    for name in ["Alice", "Bob"] {
        let mut c = login(addr, name);
        let mut p = Prices::read(&mut c).expect("reads");
        let placed = hull_choice::replenish(&mut c, &mut p, &order_a, WALLET).expect("bids");
        placed_by_member.push(placed);
    }
    for placed in &placed_by_member {
        let get = |t: i64| placed.iter().find(|p| p.0 == t).map(|p| p.1);
        assert_eq!(get(FRIGATE), Some(1), "one hull each");
        assert_eq!(get(GUN), Some(2), "the fit's two guns");
        assert!(get(ROUND).is_some_and(|n| n >= i64::from(ROUNDS)));
    }
    let mut prices = Prices::read(&mut reader).expect("reads");
    let a = demand::map(&mut reader, &mut prices, &watch).expect("reads");
    assert_eq!(
        a.unmet(FRIGATE),
        2,
        "two members, two hulls: bid_units == 2"
    );
    assert_eq!(a.unmet(GUN), 4);
    assert!(a.pull(FRIGATE) > 0.5, "bids with no asks pull hard");
    assert_eq!(a.unmet(HULL_B), 0);

    // A second replenish by Alice does not double her bids (topped up only).
    let mut c = login(addr, "Alice");
    let mut p = Prices::read(&mut c).expect("reads");
    let again = hull_choice::replenish(&mut c, &mut p, &order_a, WALLET).expect("bids");
    assert!(again.is_empty(), "already resting: {again:?}");

    // The switch: doctrine B wants another hull (same guns, fewer).
    let order_b = Order::from_fit(&fit(HULL_B, 1));
    for name in ["Alice", "Bob"] {
        let mut c = login(addr, name);
        let mut p = Prices::read(&mut c).expect("reads");
        hull_choice::replenish(&mut c, &mut p, &order_b, WALLET).expect("bids");
    }
    let mut prices = Prices::read(&mut reader).expect("reads");
    let b = demand::map(&mut reader, &mut prices, &watch).expect("reads");
    assert_eq!(b.unmet(HULL_B), 2, "the new doctrine's hull is bid for");
    assert_eq!(b.unmet(GUN), 4, "the guns already rest: no double bid");
    let ranked = b.ranked();
    assert!(ranked.iter().any(|&(t, _)| t == HULL_B));

    // The bid sits at or under the reservation cap (never above it).
    let cap = prices
        .reservation(&mut reader, HULL_B)
        .expect("reads")
        .expect("cap");
    let best = b.get(HULL_B).and_then(|d| d.best_bid).expect("a best bid");
    assert!(best <= cap + 0.01, "{best} within {cap}");

    // A gatherer: two ores of one volume, one yielding MIN_A, one MIN_B. At
    // equal values they pay alike; Dave bids deep for MIN_B and the pull
    // makes MIN_B's ore the target, MIN_A's ore falling under the edge.
    let value: BTreeMap<i64, f64> = BTreeMap::from([(MIN_A, BASE), (MIN_B, BASE)]);
    let isk = |d: &DemandMap, m: i64| {
        demand::ore_isk_per_m3(&[(m, 100)], 100, 1.0, &|x| {
            value.get(&x).map(|&v| d.weight(x, v))
        })
        .expect("priced")
    };
    let before = demand::map(&mut reader, &mut prices, &[MIN_A, MIN_B]).expect("reads");
    assert!((isk(&before, MIN_A) - isk(&before, MIN_B)).abs() < 1e-9);
    assert!(
        demand::below_the_edge(&[
            (1, Some(isk(&before, MIN_A))),
            (2, Some(isk(&before, MIN_B)))
        ])
        .is_empty()
    );
    let mut dave = login(addr, "Dave");
    let station = dave.session().station_id().expect("docked");
    dave.buy(station, MIN_B, BASE, 5_000, 7, pricing::BROKER_FEE)
        .expect("Dave bids deep");
    let mut fresh = Prices::read(&mut reader).expect("reads");
    let after = demand::map(&mut reader, &mut fresh, &[MIN_A, MIN_B]).expect("reads");
    assert_eq!(after.unmet(MIN_B), 5_000);
    assert!(isk(&after, MIN_B) > isk(&after, MIN_A));
    assert_eq!(
        demand::below_the_edge(&[(1, Some(isk(&after, MIN_A))), (2, Some(isk(&after, MIN_B)))]),
        vec![1],
        "the target shifted to the pulled mineral's ore"
    );
}
