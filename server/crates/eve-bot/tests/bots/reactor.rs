//! **MN-G, the reactor career**, over a socket: a reactor bot buys a formula
//! from an NPC's ask, buys the inputs from a player's asks, installs a
//! reaction at its station, collects the output and lists it.
//!
//! **Synthetic furniture only** (the no-client-data rule): the formula, its
//! inputs, its product, their prices, times and quantities are invented. An
//! invented NPC corporation owns the home station and lists the formula, so
//! the exchange sells it there (O5-a, MN-F: a formula is a category-9 type
//! like any blueprint). Alice, a human stand-in, posts the input asks. The
//! reactor learns Reactions by a GM line in setup, as a fixture; the bot's own
//! path is [`eve_bot::acquire`]'s. The skills are invented rows under the
//! server's own skill ids.
//!
//! **No `[A]` value is an expected value**: the runs are what Alice's asks
//! cover, the ask is whatever the pricing rule says
//! ([`builder::ask_price`] is not used: the cost floor is the rule's own, so
//! the test asks only that the product is listed at a price inside the band).

use crate::common::{config, gm, login, serve, until};
use eve_bot::Conn;
use eve_bot::builder::{self, BROKER_FEE, SellLine};
use eve_bot::reactor;
use eve_server::content::Content;
use eve_server::econ_exchange as exchange;
use eve_server::game_clock::GameClock;
use eve_server::industry;
use eve_server::model::Character;
use eve_server::reactions::{SKILL_MASS_REACTIONS, SKILL_REACTIONS};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids and numbers only.
const REACTOR: i64 = 90_000_801;
const POOR: i64 = 90_000_802;
const ALICE: i64 = 90_000_803;
const CORP: i64 = 1_999_801;
const FORMULA: i64 = 990_800_001;
const FUEL: i64 = 990_800_101;
const GAS: i64 = 990_800_102;
const PRODUCT: i64 = 990_800_201;
const FORMULA_GROUP: i64 = 1888;
const GOODS_GROUP: i64 = 99_801;
const PRODUCT_GROUP: i64 = 428;
const SKILL_GROUP: i64 = 990_020_100;
/// One run: 5 fuel and 100 gas make 25 product, in a minute.
const FUEL_PER_RUN: i64 = 5;
const GAS_PER_RUN: i64 = 100;
const PER_RUN: i64 = 25;
const SECONDS_PER_RUN: i64 = 60;
const FUEL_REF: f64 = 20.0;
const GAS_REF: f64 = 3.0;
const PRODUCT_REF: f64 = 45.0;
const FORMULA_REF: f64 = 100_000.0;
const WALLET: f64 = 5_000_000.0;
/// Less than the formula costs.
const POOR_WALLET: f64 = 50_000.0;
/// Runs Alice stocks the inputs for.
const RUNS: i64 = 2;
/// Game seconds per wall second: two runs end in about two seconds.
const SCALE: u32 = 60;
const WITHIN: Duration = Duration::from_secs(60);

fn content() -> Content {
    let row = |t: i64, g: i64, c: i64, name: &str| {
        format!("type\t{t}\t{g}\t{c}\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\t{name}\n")
    };
    let types = [
        eve_server::typedata::FIXTURE.to_owned(),
        row(FORMULA, FORMULA_GROUP, 9, "Fixture Formula"),
        row(FUEL, GOODS_GROUP, 4, "Fixture Fuel"),
        row(GAS, GOODS_GROUP, 25, "Fixture Gas"),
        row(PRODUCT, PRODUCT_GROUP, 4, "Fixture Composite"),
    ]
    .concat();
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{FUEL}\t{FUEL_REF}\tbase\t{FUEL_REF}\t4\t{GOODS_GROUP}\t\t1"),
        format!("{GAS}\t{GAS_REF}\tbase\t{GAS_REF}\t4\t{GOODS_GROUP}\t\t1"),
        format!("{PRODUCT}\t{PRODUCT_REF}\treaction\t0.0\t4\t{PRODUCT_GROUP}\t\t1"),
        format!("{FORMULA}\t{FORMULA_REF}\tbase\t0.0\t9\t{FORMULA_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let t = SECONDS_PER_RUN;
    let recipes = industry::Table::parse(
        &[
            format!("{FORMULA}\t9\tactivity\t0\t0\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tmaterial\t{FUEL}\t{FUEL_PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tmaterial\t{GAS}\t{GAS_PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tproduct\t{PRODUCT}\t{PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tskill\t{SKILL_REACTIONS}\t1\t\t{t}\t1000"),
        ],
        &prices,
    )
    .expect("the formula parses");
    let skills = eve_server::skill::Table::parse(&format!(
        "{}skill\t{SKILL_REACTIONS}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tFixture Reactions\n\
         skill\t{SKILL_MASS_REACTIONS}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tFixture Mass Reactions\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skills parse");
    let home = Character::default();
    let station = home.station_id.expect("the default pilot is docked");
    let exchange = exchange::Table::new(
        vec![exchange::Station {
            station_id: station,
            system_id: home.solar_system_id.expect("in a system"),
            region_id: home.region_id,
            owner_id: CORP,
        }],
        &[(CORP, FORMULA, 1.0)],
        [(
            FORMULA,
            exchange::TypeRow {
                group_id: FORMULA_GROUP,
                category_id: exchange::CATEGORY_BLUEPRINT,
                base_price: FORMULA_REF,
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
    c.insert(eve_server::skill::TABLE.name, skills);
    c
}

fn start() -> SocketAddr {
    let pilot = |id, name: &str, ship, balance| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        balance,
        ..Character::default()
    };
    let mut c = config();
    c.sandbox.characters = vec![
        pilot(REACTOR, "Reactor1", 100_000_801, WALLET),
        pilot(POOR, "Reactor2", 100_000_802, POOR_WALLET),
        pilot(ALICE, "Alice", 100_000_803, WALLET),
    ];
    c.time_scale = GameClock::new(SCALE).expect("a scale");
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

/// A reactor, docked, knowing Reactions (a fixture line, module header).
fn reactor_conn(addr: SocketAddr, name: &str) -> Conn {
    let mut b = login(addr, name);
    gm(&mut b, &format!("/giveskill {SKILL_REACTIONS} 1"));
    b
}

fn cycle(b: &mut Conn, content: &Content) -> builder::Cycle {
    reactor::run(b, Some(content), None).expect("a cycle")
}

fn stock_alice(addr: SocketAddr) {
    alice_asks(addr, FUEL, FUEL_PER_RUN * RUNS, FUEL_REF);
    alice_asks(addr, GAS, GAS_PER_RUN * RUNS, GAS_REF);
}

/// **MN-G, the row's test**: the reactor buys the formula from the NPC's ask
/// (nothing is given), buys the inputs from Alice's asks, installs a
/// reaction (activity 9), collects it once its end time has passed on the
/// game clock, and lists the product -- one ask per (type, station).
#[test]
fn a_reactor_buys_a_formula_and_inputs_installs_collects_and_lists() {
    let addr = start();
    let content = content();
    stock_alice(addr);
    let mut b = reactor_conn(addr, "Reactor1");
    let wallet = b.cash_balance().expect("wallet");

    let first = cycle(&mut b, &content);
    assert_eq!(first.blueprint, Some(FORMULA), "{first:?}");
    assert!(
        first.bought.iter().any(|&(t, n, _)| t == FORMULA && n == 1),
        "the formula came from the NPC's ask: {first:?}"
    );
    for t in [FUEL, GAS] {
        assert!(
            first.bought.iter().any(|&(b, n, _)| b == t && n > 0),
            "{t} bought from Alice: {first:?}"
        );
    }
    let [(job_id, formula, runs)] = first.installed[..] else {
        panic!("one reaction installed: {first:?}");
    };
    assert_eq!(
        (formula, runs),
        (FORMULA, RUNS),
        "the runs Alice's asks cover"
    );
    assert!(
        b.cash_balance().expect("wallet") < wallet,
        "the formula, the inputs and the fee left the wallet"
    );
    let job = b
        .industry_jobs()
        .expect("jobs")
        .into_iter()
        .find(|j| j.job_id == job_id)
        .expect("the job is listed");
    assert_eq!(job.activity_id, industry::REACTION);
    assert_eq!(job.product_type_id, PRODUCT);

    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "the reaction never reached its end time"
    );
    let second = cycle(&mut b, &content);
    assert_eq!(
        second.delivered,
        vec![(job_id, PRODUCT, RUNS)],
        "{second:?}"
    );
    assert!(
        second
            .posted
            .iter()
            .any(|l| l.type_id == PRODUCT && l.quantity == PER_RUN * RUNS),
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
    let own = asks[0].order;
    let (lo, hi) = eve_bot::pricing::band(PRODUCT_REF).expect("a band");
    assert!(own.price >= lo && own.price <= hi, "{own:?} in the clamp");
    assert_eq!(own.vol_remaining, PER_RUN * RUNS);
    // The formula is kept, for the next job.
    assert!(
        b.blueprints()
            .expect("blueprints")
            .iter()
            .any(|r| r.type_id == FORMULA && r.is_original()),
        "the formula stays"
    );
}

/// With no input asks the reactor rests one standing bid per (input, station)
/// at or under the margin rule, and a second cycle stacks none.
#[test]
fn the_standing_bids_keep_the_margin_rule_one_per_input_and_station() {
    let addr = start();
    let content = content();
    let mut b = reactor_conn(addr, "Reactor1");
    let station = b.session().station_id().expect("docked");
    let first = cycle(&mut b, &content);
    assert_eq!(first.blueprint, Some(FORMULA), "{first:?}");
    assert!(
        first.installed.is_empty(),
        "nothing to react with: {first:?}"
    );
    let recipes: &industry::Table = content.get();
    let bp = recipes.find_reaction(FORMULA).expect("the formula");
    let refs = recipes.prices.clone();
    let fee = reactor::run_fee(recipes, bp, 1);
    let rule = reactor::reaction_bids(bp, &refs, &refs, fee).expect("a margin at P_ref");
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
    assert_eq!(types, vec![FUEL, GAS], "one bid per input: {open:?}");
    for o in &open {
        assert!(
            o.order.price <= rule[&o.order.type_id],
            "{o:?} over the rule {rule:?}"
        );
    }
    let second = cycle(&mut b, &content);
    assert!(second.bids.is_empty(), "no second bid: {second:?}");
    assert_eq!(bids(&mut b).len(), 2, "still one per (type, station)");
}

/// A reactor that cannot pay for the formula buys nothing and installs
/// nothing: no formula is minted for it.
#[test]
fn a_reactor_without_the_isk_buys_no_formula_and_makes_nothing() {
    let addr = start();
    let content = content();
    stock_alice(addr);
    let mut b = reactor_conn(addr, "Reactor2");
    let first = cycle(&mut b, &content);
    assert_eq!(first.blueprint, None, "{first:?}");
    assert!(
        first.installed.is_empty() && first.bought.is_empty(),
        "{first:?}"
    );
    assert!(
        b.blueprints()
            .expect("blueprints")
            .iter()
            .all(|r| r.type_id != FORMULA),
        "no formula appeared"
    );
}

/// **MN-G through the act**: a `--loop reactor` bot with the formulas in its
/// content installs a reaction from its own [`eve_bot::brain::Act::Market`].
#[test]
fn a_reactor_bot_installs_a_reaction_through_its_market_act() {
    use eve_bot::brain::{ActKind, Policy};
    use eve_bot::driver::{BotConfig, Pace};
    let addr = start();
    stock_alice(addr);
    drop(reactor_conn(addr, "Reactor1"));

    let role = &reactor::ROLE;
    let mut config = BotConfig::new("Reactor1", addr, Pace::Fast).with_role(role);
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
