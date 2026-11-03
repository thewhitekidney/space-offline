//! **E9: the builder's industry act**, over a socket (R-ECON §3 row E9).
//!
//! **Synthetic furniture only** (the no-client-data rule): an invented
//! product, two invented minerals and an invented recipe of one second a
//! run, on the first rung of the builder's ladder
//! ([`builder::BLUEPRINT_LADDER`], named, never its value). An invented NPC
//! corporation owns the home station and lists that blueprint, so under
//! O5-a the exchange sells it there (E5). Alice, a human stand-in, posts the
//! mineral asks. The builder learns Industry by a GM line in setup, as a
//! fixture; the bot's own path is `acquire`'s.

use crate::common::{config, gm, login, serve, until};
use eve_bot::Conn;
use eve_bot::builder::{self, BLUEPRINT_LADDER, BROKER_FEE, SellLine};
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::industry;
use eve_server::model::Character;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids and numbers only.
const BUILDER: i64 = 90_000_201;
const ALICE: i64 = 90_000_202;
const CORP: i64 = 1_999_601;
const PRODUCT: i64 = 990_300_001;
const MINERAL_A: i64 = 990_300_101;
const MINERAL_B: i64 = 990_300_102;
const PRODUCT_GROUP: i64 = 99_083;
const BLUEPRINT_GROUP: i64 = 99_105;
const MINERAL_GROUP: i64 = 18;
const PRODUCT_REF: f64 = 100.0;
const A_REF: f64 = 4.0;
const B_REF: f64 = 10.0;
const BLUEPRINT_REF: f64 = 100_000.0;
/// One run: 50 A and 5 B make 10 of the product, in a second.
const A_PER_RUN: i64 = 50;
const B_PER_RUN: i64 = 5;
const PER_RUN: i64 = 10;
const SECONDS_PER_RUN: i64 = 1;
const WALLET: f64 = 1_000_000.0;
const WITHIN: Duration = Duration::from_secs(30);

fn blueprint() -> i64 {
    BLUEPRINT_LADDER[0]
}

/// `Content::fixture()` plus the types, their prices, the recipe and the
/// corporation that sells the blueprint at the home station.
fn content() -> Content {
    let bp = blueprint();
    let types = format!(
        "{}\
         type\t{PRODUCT}\t{PRODUCT_GROUP}\t8\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Product\n\
         type\t{MINERAL_A}\t{MINERAL_GROUP}\t4\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Mineral A\n\
         type\t{MINERAL_B}\t{MINERAL_GROUP}\t4\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Mineral B\n\
         type\t{bp}\t{BLUEPRINT_GROUP}\t9\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Blueprint\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{PRODUCT}\t{PRODUCT_REF}\trecipe\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t1\t1"),
        format!("{MINERAL_A}\t{A_REF}\tbase\t{A_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{MINERAL_B}\t{B_REF}\tbase\t{B_REF}\t4\t{MINERAL_GROUP}\t\t1"),
        format!("{bp}\t{BLUEPRINT_REF}\tbase\t{BLUEPRINT_REF}\t9\t{BLUEPRINT_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let t = SECONDS_PER_RUN;
    let recipes = industry::Table::parse(
        &[
            format!("{bp}\t1\tactivity\t0\t0\t\t{t}\t10"),
            format!("{bp}\t1\tmaterial\t{MINERAL_A}\t{A_PER_RUN}\t\t{t}\t10"),
            format!("{bp}\t1\tmaterial\t{MINERAL_B}\t{B_PER_RUN}\t\t{t}\t10"),
            format!("{bp}\t1\tproduct\t{PRODUCT}\t{PER_RUN}\t\t{t}\t10"),
            format!("{bp}\t1\tskill\t{}\t1\t\t{t}\t10", industry::SKILL_INDUSTRY),
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
    let pilot = |id, name: &str, ship| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance: WALLET,
        ..Character::default()
    };
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(BUILDER, "Builder1", 100_000_201),
        pilot(ALICE, "Alice", 100_000_202),
    ];
    c.content = Some(Arc::new(content()));
    serve(c)
}

/// Alice lists `quantity` of `type_id` at the home station at `price`.
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

/// The builder, docked, knowing Industry (a fixture line, module header).
fn builder_conn(addr: SocketAddr) -> Conn {
    let mut b = login(addr, "Builder1");
    gm(
        &mut b,
        &format!("/giveskill {} 1", industry::SKILL_INDUSTRY),
    );
    b
}

fn cycle(b: &mut Conn, content: &Content) -> builder::Cycle {
    builder::run(b, &[], Some(content)).expect("a cycle")
}

fn prices() -> BTreeMap<i64, f64> {
    [
        (PRODUCT, PRODUCT_REF),
        (MINERAL_A, A_REF),
        (MINERAL_B, B_REF),
    ]
    .into_iter()
    .collect()
}

/// **E9, the row's test**: the builder buys the fixture blueprint from the
/// NPC's ask, buys its materials from Alice's asks, installs a job, delivers
/// it once its end time has passed, and lists the product -- one ask per
/// (type, station), at its markup.
#[test]
fn the_builder_buys_a_blueprint_and_materials_installs_completes_and_lists() {
    let addr = start();
    let content = content();
    alice_asks(addr, MINERAL_A, A_PER_RUN, A_REF);
    alice_asks(addr, MINERAL_B, B_PER_RUN, B_REF);
    let mut b = builder_conn(addr);

    let first = cycle(&mut b, &content);
    assert_eq!(first.blueprint, Some(blueprint()), "{first:?}");
    for m in [MINERAL_A, MINERAL_B] {
        assert!(
            first.bought.iter().any(|&(t, n, _)| t == m && n > 0),
            "{m} bought from Alice: {first:?}"
        );
    }
    let [(job_id, bp_type, runs)] = first.installed[..] else {
        panic!("one job installed: {first:?}");
    };
    assert_eq!(
        (bp_type, runs),
        (blueprint(), 1),
        "the run Alice's asks cover"
    );

    // Its end time.
    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "the job never reached its end time"
    );
    let second = cycle(&mut b, &content);
    assert_eq!(second.delivered, vec![(job_id, PRODUCT, 1)], "{second:?}");
    let ask = builder::ask_price(PRODUCT_REF, None).expect("an ask price");
    assert!(
        second
            .posted
            .iter()
            .any(|l| l.type_id == PRODUCT && l.quantity == PER_RUN && l.price == ask),
        "the product is listed: {second:?}"
    );
    let station = b.session().station_id().expect("docked");
    let asks: Vec<_> = b
        .my_open_orders()
        .expect("my orders")
        .into_iter()
        .filter(|o| !o.order.bid && o.order.type_id == PRODUCT && o.order.station_id == station)
        .collect();
    assert_eq!(asks.len(), 1, "one ask per (type, station): {asks:?}");
}

/// **E9**: with no mineral asks, the builder rests one standing bid per
/// (mineral, station), each at or under the margin rule; a second cycle
/// stacks none and sends no modify inside the modification delay, which
/// the server would refuse.
#[test]
fn the_standing_bids_keep_the_margin_rule_one_per_type_and_station() {
    let addr = start();
    let content = content();
    let mut b = builder_conn(addr);
    let station = b.session().station_id().expect("docked");

    let first = cycle(&mut b, &content);
    assert_eq!(first.blueprint, Some(blueprint()), "{first:?}");
    assert!(
        first.installed.is_empty(),
        "nothing to build with: {first:?}"
    );
    let recipes: &industry::Table = content.get();
    let bp = recipes.find(blueprint()).expect("the recipe");
    let rule = builder::margin_bids(bp, &prices()).expect("a margin at P_ref");
    let bids = |b: &mut Conn| -> Vec<builder::OwnOrder> {
        b.my_open_orders()
            .expect("my orders")
            .into_iter()
            .filter(|o| o.order.bid && o.order.station_id == station)
            .collect()
    };
    let open = bids(&mut b);
    let mut types: Vec<i64> = open.iter().map(|o| o.order.type_id).collect();
    types.sort_unstable();
    assert_eq!(
        types,
        vec![MINERAL_A, MINERAL_B],
        "one bid per mineral: {open:?}"
    );
    for o in &open {
        assert!(
            o.order.price <= rule[&o.order.type_id],
            "{o:?} over the margin rule {rule:?}"
        );
    }
    let per_run: f64 = bp
        .materials
        .iter()
        .map(|&(t, q)| {
            let bid = open.iter().find(|o| o.order.type_id == t).expect("bid");
            q as f64 * bid.order.price * (1.0 + BROKER_FEE)
        })
        .sum();
    assert!(per_run <= builder::material_budget(bp, PRODUCT_REF) + 1e-6);

    let second = cycle(&mut b, &content);
    assert!(second.bids.is_empty(), "no second bid: {second:?}");
    assert!(
        second.repriced.is_empty() && !second.notes.iter().any(|n| n.contains("reprice")),
        "no modify inside the delay: {second:?}"
    );
    assert_eq!(bids(&mut b).len(), 2, "still one per (type, station)");

    // The server's side of the delay the builder respects: a modify now is
    // refused.
    let own = bids(&mut b)[0];
    let early = b.modify_order(&own, own.order.price - 0.01);
    assert!(
        matches!(&early, Err(eve_bot::Error::Refused(why)) if why.contains("MktOrderDelay")),
        "{early:?}"
    );
}

/// **E9 through the act**: a `--loop builder` bot with the recipes in its
/// content installs a job from its own [`eve_bot::brain::Act::Market`].
#[test]
fn a_builder_bot_installs_a_job_through_its_market_act() {
    use eve_bot::brain::{ActKind, Policy};
    use eve_bot::driver::{BotConfig, Pace};
    let addr = start();
    alice_asks(addr, MINERAL_A, A_PER_RUN, A_REF);
    alice_asks(addr, MINERAL_B, B_PER_RUN, B_REF);
    drop(builder_conn(addr));

    let role = &builder::ROLE;
    let mut config = BotConfig::new("Builder1", addr, Pace::Fast).with_role(role);
    config.chat = false;
    config.content = Some(Arc::new(content()));
    config.policy = Policy {
        role,
        gm_setup: false,
        closed: true,
        ..Policy::fast()
    };
    let (handle, thread) = eve_bot::driver::spawn(config);
    let installed = until(WITHIN, || {
        handle
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Market && e.what.contains("installed 1"))
    });
    handle.stop();
    let _ = thread.join();
    assert!(installed, "no install:\n{}", crate::common::dump(&handle));
    assert!(
        !handle
            .log()
            .iter()
            .any(|e| e.kind == ActKind::Slash || e.kind == ActKind::GmSetup),
        "a GM slash from a closed-economy bot:\n{}",
        crate::common::dump(&handle)
    );
}
