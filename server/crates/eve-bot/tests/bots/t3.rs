//! **T3-H over a socket**: a builder invents a hull from a relic toward its
//! doctrine kit, and a pilot assembles a strategic cruiser from four
//! subsystems it owns, boards it and undocks.
//!
//! **Synthetic furniture only** (the no-client-data rule): every type,
//! recipe, time and chance is invented. The only ids named are the client's
//! attribute and effect ids the server reads, and the server's own group
//! constants. Pilots get their goods by GM line in setup, as fixtures.

use crate::common::{config, gm, login, serve, until};
use eve_bot::builder::{self, INVENTION_MIN_WALLET};
use eve_bot::{Conn, t3};
use eve_server::content::Content;
use eve_server::model::Character;
use eve_server::subsystems::{CATEGORY_SUBSYSTEM, FIRST_FLAG, GROUP_STRATEGIC_CRUISER};
use eve_server::{industry, typedata};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids. The builder's character id is even, and `id / 2` odd, so
// its doctrine is the second strategic cruiser (HULL_B).
const BUILDER: i64 = 90_000_802;
const PILOT: i64 = 90_000_803;
const HULL_A: i64 = 990_980_001;
const HULL_B: i64 = 990_980_002;
const SUB_A: [i64; 4] = [990_980_011, 990_980_012, 990_980_013, 990_980_014];
const SUB_B: [i64; 4] = [990_980_021, 990_980_022, 990_980_023, 990_980_024];
const RELIC: i64 = 990_983_001;
const CORE_X: i64 = 990_983_011;
const CORE_Y: i64 = 990_983_012;
const BLUEPRINT_GROUP: i64 = 99_801;
const RELIC_GROUP: i64 = 99_802;
const CORE_GROUP: i64 = 99_803;
const SUB_GROUP: i64 = 990_958;
const RELIC_CATEGORY: i64 = 34;
/// Runs each invented copy carries (invented).
const COPY_RUNS: i64 = 4;
const PER_RUN: i64 = 1;
const WITHIN: Duration = Duration::from_secs(60);

fn bp(item: i64) -> i64 {
    item + 1000
}

/// Both hulls and every subsystem: the one invented relic invents them all
/// (the real families are one per slot; the doctrine logic is the same).
fn items() -> Vec<i64> {
    let mut v = vec![HULL_A, HULL_B];
    v.extend(SUB_A);
    v.extend(SUB_B);
    v
}

fn content() -> Content {
    let mut text = String::from(typedata::FIXTURE);
    text += "effect\t3772\t0\t0\t0\t0\t0\t0\t0\tsubSystem\t\n";
    text += "effect\t3773\t0\t0\t0\t0\t0\t0\t0\thardPointModifierEffect\t\n";
    text += "effect\t3774\t0\t0\t0\t0\t0\t0\t0\tslotModifier\t\n";
    for h in [HULL_A, HULL_B] {
        text += &format!(
            "type\t{h}\t{GROUP_STRATEGIC_CRUISER}\t6\t1\t1\t100000.0\t0.0\t12000000.0\t200.0\t0\t\t\
             9=2000.0;11=1000.0;12=0.0;13=0.0;14=0.0;48=500.0;70=100.0;101=0.0;102=0.0;1137=3.0\t\
             Fixture Modular Hull {h}\n"
        );
    }
    for (subs, hull) in [(SUB_A, HULL_A), (SUB_B, HULL_B)] {
        for (n, id) in subs.iter().enumerate() {
            text += &format!(
                "type\t{id}\t{SUB_GROUP}\t{CATEGORY_SUBSYSTEM}\t1\t1\t40.0\t0.0\t1200000.0\t1.0\t0\t3772,3774,3773\t\
                 1366={}.0;1380={hull}.0;1374=1.0\tFixture Subsystem {id}\n",
                FIRST_FLAG + n as i64
            );
        }
    }
    let row = |t: i64, g: i64, c: i64, name: &str| {
        format!("type\t{t}\t{g}\t{c}\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\t{name}\n")
    };
    for item in items() {
        text += &row(bp(item), BLUEPRINT_GROUP, 9, "Fixture Blueprint");
    }
    text += &row(RELIC, RELIC_GROUP, RELIC_CATEGORY, "Fixture Relic");
    text += &row(CORE_X, CORE_GROUP, 17, "Fixture Core X");
    text += &row(CORE_Y, CORE_GROUP, 17, "Fixture Core Y");
    let td = typedata::Table::parse(&text).expect("the test rows parse");
    // The relic's invention (time 1 s, chance 1) offers both hulls; both
    // blueprints build their hull.
    let mut lines = vec![
        format!("{RELIC}\t8\tactivity\t0\t0\t\t1\t300"),
        format!("{RELIC}\t8\tmaterial\t{CORE_X}\t{PER_RUN}\t\t1\t300"),
        format!("{RELIC}\t8\tmaterial\t{CORE_Y}\t{PER_RUN}\t\t1\t300"),
        format!(
            "{RELIC}\t8\tskill\t{}\t1\t\t1\t300",
            industry::SKILL_SCIENCE
        ),
    ];
    for hull in items() {
        lines.push(format!(
            "{RELIC}\t8\tproduct\t{}\t{COPY_RUNS}\t1.0\t1\t300",
            bp(hull)
        ));
        lines.push(format!("{}\t1\tactivity\t0\t0\t\t900\t1", bp(hull)));
        lines.push(format!("{}\t1\tmaterial\t34\t100\t\t900\t1", bp(hull)));
        lines.push(format!("{}\t1\tproduct\t{hull}\t1\t\t900\t1", bp(hull)));
        lines.push(format!(
            "{}\t1\tskill\t{}\t1\t\t900\t1",
            bp(hull),
            industry::SKILL_INDUSTRY
        ));
    }
    let recipes = industry::Table::parse(&lines, &eve_server::econ_prices::Table::default())
        .expect("the recipes parse");
    let mut c = Content::fixture();
    c.insert("typedata", typedata::Installed::build(td));
    c.insert("industry", recipes);
    c
}

fn start() -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![
        Character {
            character_id: BUILDER,
            name: "T3Builder".to_owned(),
            ship_id: 100_000_802,
            balance: INVENTION_MIN_WALLET * 2.0,
            ..Character::default()
        },
        Character {
            character_id: PILOT,
            name: "T3Pilot".to_owned(),
            ship_id: 100_000_803,
            ..Character::default()
        },
    ];
    c.content = Some(Arc::new(content()));
    serve(c)
}

fn ready(b: &mut Conn, job_id: i64) {
    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "job {job_id} never reached its end time"
    );
}

/// A builder with a relic and the cores invents **the doctrine kit's hull**,
/// not the first product on the relic's list, and gets a copy that builds it.
#[test]
fn a_builder_aims_its_relic_at_the_doctrine_hull() {
    let addr = start();
    let content = content();
    let mut b = login(addr, "T3Builder");
    gm(&mut b, &format!("/giveskill {} 1", industry::SKILL_SCIENCE));
    gm(
        &mut b,
        &format!("/giveskill {} 1", industry::SKILL_INDUSTRY),
    );
    gm(&mut b, &format!("/createitem {RELIC} 2"));
    gm(&mut b, &format!("/createitem {CORE_X} 6"));
    gm(&mut b, &format!("/createitem {CORE_Y} 6"));

    let first = builder::run(&mut b, &[], Some(&content)).expect("a cycle");
    let [(job, relic, activity, runs)] = first.lab[..] else {
        panic!("one lab job: {first:?}");
    };
    assert_eq!(
        (relic, activity, runs),
        (RELIC, industry::INVENTION, 2),
        "{first:?}"
    );
    ready(&mut b, job);

    let second = builder::run(&mut b, &[], Some(&content)).expect("a cycle");
    assert!(second.delivered.iter().any(|d| d.0 == job), "{second:?}");
    let copies: Vec<_> = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .filter(|r| r.type_id == bp(HULL_B) || r.type_id == bp(HULL_A))
        .collect();
    assert!(
        !copies.is_empty() && copies.iter().all(|r| r.type_id == bp(HULL_B)),
        "the doctrine hull only: {copies:?}"
    );
    assert!(copies.iter().all(|r| r.runs == COPY_RUNS), "{copies:?}");
}

/// A pilot holding a strategic cruiser and its four subsystems assembles
/// them (`AssembleShip` with `subSystems`), boards, and undocks.
#[test]
fn a_pilot_assembles_a_strategic_cruiser_from_four_subsystems_and_undocks() {
    let addr = start();
    let content = content();
    let mut p = login(addr, "T3Pilot");
    gm(&mut p, &format!("/createitem {HULL_B}"));
    for sub in SUB_B {
        gm(&mut p, &format!("/createitem {sub}"));
    }
    let hangar = p.hangar_items().expect("hangar");
    let kits = t3::catalog(&content);
    assert_eq!(
        kits.iter().map(|k| k.hull).collect::<Vec<_>>(),
        [HULL_A, HULL_B]
    );
    assert_eq!(t3::ready(&kits, &hangar).len(), 1, "only B's kit is whole");

    let done = t3::pilot_step(&mut p, &content, 7).expect("a step");
    assert!(done.flying, "{done:?}");
    // The kit has no launcher hardpoints, so the weapon the step reports is a
    // turret one, whatever the seed drew.
    assert!(
        done.weapon
            .is_some_and(|w| w != eve_bot::hull_choice::Weapon::Missile),
        "{done:?}"
    );
    let ship = p.session().ship_id().expect("a ship");
    let rows = p.hangar_items().expect("hangar");
    let hull = rows.iter().find(|r| r.item_id == ship);
    assert!(
        hull.is_some_and(|r| r.type_id == HULL_B && r.singleton),
        "boarded the assembled hull: {rows:?}"
    );
    // The four subsystems went into the hull's flags; none is left lying.
    assert!(
        !rows.iter().any(|r| SUB_B.contains(&r.type_id)),
        "the subsystems are fitted: {rows:?}"
    );
    p.undock().expect("a full strategic cruiser undocks");
}

/// With three of four subsystems nothing is assembled and the pilot is not
/// flying a T3 hull.
#[test]
fn three_subsystems_are_not_a_kit() {
    let addr = start();
    let content = content();
    let mut p = login(addr, "T3Pilot");
    gm(&mut p, &format!("/createitem {HULL_B}"));
    for sub in &SUB_B[..3] {
        gm(&mut p, &format!("/createitem {sub}"));
    }
    let done = t3::pilot_step(&mut p, &content, 7).expect("a step");
    assert!(!done.flying, "{done:?}");
    assert!(
        p.hangar_items()
            .expect("hangar")
            .iter()
            .filter(|r| SUB_B.contains(&r.type_id))
            .count()
            == 3
    );
}
