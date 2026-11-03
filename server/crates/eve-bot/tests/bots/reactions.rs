//! **MN-A, reactions at a station**, over a socket: a pilot holding a formula
//! installs a reaction at the home station through the same `industryManager`
//! calls the client's Industry window makes, waits out the game clock and
//! collects the output.
//!
//! **Synthetic furniture only** (the no-client-data rule): the formula, its
//! inputs, its product, their prices, times and quantities are invented. The
//! only real ids named are the server's skill constants
//! (`eve_server::reactions`), which are the client's own and already in the
//! code. The skills are added to the fixture skill table as invented rows
//! under those ids, because the sandbox has no bundle.

use crate::common::{config, gm, login, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::game_clock::GameClock;
use eve_server::industry::{self, Facility};
use eve_server::model::Character;
use eve_server::reactions::{SKILL_MASS_REACTIONS, SKILL_REACTIONS};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids and numbers only.
const PILOT: i64 = 90_000_901;
const FORMULA: i64 = 990_900_001;
const FUEL: i64 = 990_900_101;
const GAS: i64 = 990_900_102;
const MINERAL: i64 = 990_900_103;
const PRODUCT: i64 = 990_900_201;
const FORMULA_GROUP: i64 = 1888;
const GOODS_GROUP: i64 = 99_901;
const SKILL_GROUP: i64 = 990_020_100;
/// One run: 5 fuel, 100 gas and 40 mineral make 25 product.
const FUEL_PER_RUN: i64 = 5;
const GAS_PER_RUN: i64 = 100;
const MINERAL_PER_RUN: i64 = 40;
const PER_RUN: i64 = 25;
/// A run's nominal length, game seconds.
const SECONDS_PER_RUN: i64 = 60;
const FUEL_REF: f64 = 20.0;
const GAS_REF: f64 = 3.0;
const MINERAL_REF: f64 = 5.0;
const WALLET: f64 = 10_000_000.0;
/// Game seconds per wall second, so two runs end in about two seconds.
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
        row(MINERAL, GOODS_GROUP, 4, "Fixture Mineral"),
        row(PRODUCT, GOODS_GROUP, 8, "Fixture Polymer"),
    ]
    .concat();
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{FUEL}\t{FUEL_REF}\tbase\t{FUEL_REF}\t4\t{GOODS_GROUP}\t\t1"),
        format!("{GAS}\t{GAS_REF}\tbase\t{GAS_REF}\t4\t{GOODS_GROUP}\t\t1"),
        format!("{MINERAL}\t{MINERAL_REF}\tbase\t{MINERAL_REF}\t4\t{GOODS_GROUP}\t\t1"),
    ])
    .expect("the prices parse");
    let t = SECONDS_PER_RUN;
    let recipes = industry::Table::parse(
        &[
            format!("{FORMULA}\t9\tactivity\t0\t0\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tmaterial\t{FUEL}\t{FUEL_PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tmaterial\t{GAS}\t{GAS_PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tmaterial\t{MINERAL}\t{MINERAL_PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tproduct\t{PRODUCT}\t{PER_RUN}\t\t{t}\t1000"),
            format!("{FORMULA}\t9\tskill\t{SKILL_REACTIONS}\t1\t\t{t}\t1000"),
        ],
        &prices,
    )
    .expect("the formula parses");
    // The reaction skills as invented rows under their ids.
    let skills = eve_server::skill::Table::parse(&format!(
        "{}skill\t{SKILL_REACTIONS}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tFixture Reactions\n\
         skill\t{SKILL_MASS_REACTIONS}\t{SKILL_GROUP}\t1\t165\t166\t0.01\t\tFixture Mass Reactions\n",
        eve_server::skill::FIXTURE
    ))
    .expect("the skills parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c.insert("industry", recipes);
    c.insert(eve_server::skill::TABLE.name, skills);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![Character {
        character_id: PILOT,
        name: "Reactor1".to_owned(),
        ship_id: 100_000_901,
        balance: WALLET,
        ..Character::default()
    }];
    c.time_scale = GameClock::new(SCALE).expect("a scale");
    c.content = Some(Arc::new(content()));
    serve(c)
}

/// `industry.job.Location` as the plain dict the server also reads.
fn here(station: i64) -> PyValue {
    eve_server::pyutil::pydict(vec![
        ("itemID", PyValue::Int(station)),
        ("flagID", PyValue::Int(industry::FLAG_HANGAR)),
    ])
}

/// The window's `InstallJob(job.dump())` for activity 9.
fn install(b: &mut Conn, formula_item: i64, station: i64, runs: i64) -> Result<i64, String> {
    let dump = eve_server::pyutil::pydict(vec![
        ("activityID", PyValue::Int(industry::REACTION)),
        ("blueprintID", PyValue::Int(formula_item)),
        ("runs", PyValue::Int(runs)),
        ("licensedRuns", PyValue::Int(0)),
        ("productTypeID", PyValue::None),
        ("inputLocation", here(station)),
        ("outputLocation", here(station)),
    ]);
    let outcome = b
        .call("industryManager", "InstallJob", vec![dump])
        .map_err(|e| format!("{e:?}"))?;
    match (outcome.refusal(), outcome.value()) {
        (None, Some(PyValue::Int(job_id))) => Ok(job_id),
        (Some(key), _) => Err(key),
        (None, other) => Err(format!("answered {other:?}")),
    }
}

fn held(b: &mut Conn, type_id: i64) -> i64 {
    b.hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

/// A pilot stocked for `runs` runs, the formula in the hangar.
fn stocked(addr: SocketAddr, runs: i64, skill: bool) -> Conn {
    let mut b = login(addr, "Reactor1");
    if skill {
        gm(&mut b, &format!("/giveskill {SKILL_REACTIONS} 1"));
    }
    gm(&mut b, &format!("/createitem {FORMULA} 1"));
    gm(
        &mut b,
        &format!("/createitem {FUEL} {}", FUEL_PER_RUN * runs),
    );
    gm(&mut b, &format!("/createitem {GAS} {}", GAS_PER_RUN * runs));
    gm(
        &mut b,
        &format!("/createitem {MINERAL} {}", MINERAL_PER_RUN * runs),
    );
    b
}

/// **The packet's test**: install at the station, the inputs and the fee
/// leave, the formula stays, the job ends on the game clock, delivery puts the
/// product in the hangar.
#[test]
fn a_pilot_installs_a_reaction_at_a_station_and_collects_the_output() {
    let runs = 2;
    let addr = start();
    let mut b = stocked(addr, runs, true);
    let station = b.session().station_id().expect("docked");

    // The formula is listed as an unresearched original.
    let formula = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .find(|r| r.type_id == FORMULA)
        .expect("the formula is listed");
    assert!(formula.is_original(), "{formula:?}");
    assert_eq!(formula.job_id, None);

    let before = b.cash_balance().expect("wallet");
    let job_id = install(&mut b, formula.item_id, station, runs).expect("the reaction installs");

    // The inputs are burned, the formula stays.
    for t in [FUEL, GAS, MINERAL] {
        assert_eq!(held(&mut b, t), 0, "type {t} was consumed");
    }
    assert_eq!(held(&mut b, FORMULA), 1, "the formula is kept");
    // The fee: EIV of the inputs times runs, plus the SCC surcharge, from the
    // wallet.
    let eiv = (FUEL_PER_RUN as f64 * FUEL_REF
        + GAS_PER_RUN as f64 * GAS_REF
        + MINERAL_PER_RUN as f64 * MINERAL_REF)
        * runs as f64;
    let fee = industry::cost_lines(
        eiv,
        industry::REACTION,
        &Facility::npc_station(station, 0, 2502, 0),
        1.0,
    )
    .total();
    let spent = before - b.cash_balance().expect("wallet");
    assert!(
        (spent - fee as f64).abs() < 1.0,
        "spent {spent}, the fee is {fee}"
    );
    // The window's list says what the job is.
    let job = b
        .industry_jobs()
        .expect("jobs")
        .into_iter()
        .find(|j| j.job_id == job_id)
        .expect("the job is listed");
    assert_eq!(job.activity_id, industry::REACTION);
    assert_eq!(job.product_type_id, PRODUCT);
    assert_eq!(job.runs, runs);
    assert!(!job.completed());
    // And the formula is busy until it is delivered.
    let busy = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .find(|r| r.type_id == FORMULA)
        .expect("listed");
    assert_eq!(busy.job_id, Some(job_id));

    // Not before the game clock runs out...
    let early = b.complete_job(&job);
    assert!(early.is_err(), "delivered early: {early:?}");
    assert_eq!(held(&mut b, PRODUCT), 0);

    // ...and then it delivers: runs x the formula's per-run quantity.
    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "the reaction never reached its end time"
    );
    b.complete_job(&job).expect("delivers");
    assert_eq!(held(&mut b, PRODUCT), PER_RUN * runs);
    assert!(
        b.industry_jobs()
            .expect("jobs")
            .iter()
            .all(|j| j.job_id != job_id || j.completed()),
        "the job is finished"
    );
}

/// The game clock paces it: two nominal minutes end in about two seconds, not
/// two minutes. (Reactions 1 shortens the nominal time by a factor, which the
/// bound allows for.)
#[test]
fn the_reaction_ends_on_the_game_clock_not_the_wall_clock() {
    let addr = start();
    let mut b = stocked(addr, 2, true);
    let station = b.session().station_id().expect("docked");
    let formula = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .find(|r| r.type_id == FORMULA)
        .expect("the formula");
    let job_id = install(&mut b, formula.item_id, station, 2).expect("installs");
    let job = b
        .industry_jobs()
        .expect("jobs")
        .into_iter()
        .find(|j| j.job_id == job_id)
        .expect("listed");
    let wall_left = (job.end_date - eve_server::pyutil::filetime_now()) / industry::FILETIME_SECOND;
    let nominal = 2 * SECONDS_PER_RUN;
    assert!(
        wall_left <= nominal / i64::from(SCALE) + 1,
        "{wall_left} s of wall time left for {nominal} nominal seconds at scale {SCALE}"
    );
}

/// Refusals reach the pilot as refusals and cost nothing: no skill, then no
/// inputs, then a manufacturing request on a formula.
#[test]
fn a_reaction_without_the_skill_or_the_inputs_is_refused_and_changes_nothing() {
    let addr = start();
    // No Reactions skill.
    let mut b = stocked(addr, 1, false);
    let station = b.session().station_id().expect("docked");
    let formula = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .find(|r| r.type_id == FORMULA)
        .expect("the formula");
    let before = b.cash_balance().expect("wallet");
    let refused = install(&mut b, formula.item_id, station, 1);
    assert!(refused.is_err(), "no skill: {refused:?}");
    assert_eq!(held(&mut b, FUEL), FUEL_PER_RUN, "nothing moved");
    assert_eq!(b.cash_balance().expect("wallet"), before, "and no fee");
    assert!(b.industry_jobs().expect("jobs").is_empty());

    // The skill, but one run's inputs for two runs.
    gm(&mut b, &format!("/giveskill {SKILL_REACTIONS} 1"));
    let short = install(&mut b, formula.item_id, station, 2);
    assert!(short.is_err(), "short of inputs: {short:?}");
    assert_eq!(held(&mut b, GAS), GAS_PER_RUN, "nothing moved");
    // One run is fine.
    install(&mut b, formula.item_id, station, 1).expect("one run installs");
}
