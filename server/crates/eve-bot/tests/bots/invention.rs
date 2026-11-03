//! **SOC-B7: the builder's laboratory**, over a socket: an original is
//! copied, the copy invented from with the hangar's datacores, and the
//! Tech II copy built once its component is in the hangar.
//!
//! **Synthetic furniture only** (the no-client-data rule): every type,
//! recipe, time and chance is invented. The only ids named are the
//! server's skill constants and the builder's own consts. The builder is
//! given its blueprint, datacores, component and skills by GM lines in
//! setup, as fixtures; the bot's own sources (the ladder, the market) are
//! `builder.rs`'s.

use crate::common::{config, gm, login, serve, until};
use eve_bot::Conn;
use eve_bot::builder::{self, INVENTION_MIN_WALLET};
use eve_server::content::Content;
use eve_server::industry;
use eve_server::model::Character;
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

// Invented ids and numbers only.
const BUILDER: i64 = 90_000_701;
const T1_BP: i64 = 990_710_001;
const T1_PRODUCT: i64 = 990_710_002;
const T2_BP: i64 = 990_710_003;
const T2_PRODUCT: i64 = 990_710_004;
const CORE_A: i64 = 990_710_011;
const CORE_B: i64 = 990_710_012;
const COMPONENT: i64 = 990_710_021;
const MINERAL: i64 = 990_710_031;
const BLUEPRINT_GROUP: i64 = 99_710;
const GOODS_GROUP: i64 = 99_711;
const CORE_GROUP: i64 = 99_712;
/// The copy licence a T1 copy carries at most, so a copy job is short.
const LICENCE: i64 = 2;
/// Runs each invented copy carries.
const T2_RUNS: i64 = 2;
/// Datacores per attempt.
const PER_ATTEMPT: i64 = 1;
/// Components per T2 run, at ME 0.
const COMPONENTS_PER_RUN: i64 = 4;
const WITHIN: Duration = Duration::from_secs(60);
/// The racial invention's encryption skill: the skill fixture's first
/// invented skill (`eve_server::skill::FIXTURE`, rank 1, no prerequisite),
/// so the server can inject and train it.
const ENCRYPTION: i64 = 990_020_001;
const ENCRYPTION_GROUP: i64 = 990_020_100;
/// The school's corporation, which lists nothing, and the station's owner.
const SCHOOL_CORP: i64 = 1_999_711;
const STATION_OWNER: i64 = 1_999_712;

fn content() -> Content {
    content_with(&[])
}

/// [`content`], the invention also requiring `extra` skills.
fn content_with(extra: &[i64]) -> Content {
    let row = |t: i64, g: i64, c: i64, name: &str| {
        format!("type\t{t}\t{g}\t{c}\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\t{name}\n")
    };
    let types = [
        eve_server::typedata::FIXTURE.to_owned(),
        row(T1_BP, BLUEPRINT_GROUP, 9, "Fixture T1 Blueprint"),
        row(T2_BP, BLUEPRINT_GROUP, 9, "Fixture T2 Blueprint"),
        row(T1_PRODUCT, GOODS_GROUP, 8, "Fixture T1 Product"),
        row(T2_PRODUCT, GOODS_GROUP, 8, "Fixture T2 Product"),
        row(CORE_A, CORE_GROUP, 17, "Fixture Datacore A"),
        row(CORE_B, CORE_GROUP, 17, "Fixture Datacore B"),
        row(COMPONENT, GOODS_GROUP, 17, "Fixture Component"),
        row(MINERAL, GOODS_GROUP, 4, "Fixture Mineral"),
        row(ENCRYPTION, ENCRYPTION_GROUP, 16, "Fixture Skill One"),
    ]
    .concat();
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let industry_skill = industry::SKILL_INDUSTRY;
    let science = industry::SKILL_SCIENCE;
    let mut lines = vec![
        format!("{T1_BP}\t1\tactivity\t0\t0\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t1\tmaterial\t{MINERAL}\t10\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t1\tproduct\t{T1_PRODUCT}\t1\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t1\tskill\t{industry_skill}\t1\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t5\tactivity\t0\t0\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t8\tactivity\t0\t0\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t8\tmaterial\t{CORE_A}\t{PER_ATTEMPT}\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t8\tmaterial\t{CORE_B}\t{PER_ATTEMPT}\t\t1\t{LICENCE}"),
        format!("{T1_BP}\t8\tproduct\t{T2_BP}\t{T2_RUNS}\t1.0\t1\t{LICENCE}"),
        format!("{T1_BP}\t8\tskill\t{science}\t1\t\t1\t{LICENCE}"),
        format!("{T2_BP}\t1\tactivity\t0\t0\t\t1\t10"),
        format!("{T2_BP}\t1\tmaterial\t{COMPONENT}\t{COMPONENTS_PER_RUN}\t\t1\t10"),
        format!("{T2_BP}\t1\tproduct\t{T2_PRODUCT}\t1\t\t1\t10"),
        format!("{T2_BP}\t1\tskill\t{industry_skill}\t1\t\t1\t10"),
    ];
    for skill in extra {
        lines.push(format!("{T1_BP}\t8\tskill\t{skill}\t1\t\t1\t{LICENCE}"));
    }
    let recipes = industry::Table::parse(&lines, &eve_server::econ_prices::Table::default())
        .expect("the recipes parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("industry", recipes);
    c
}

fn start() -> SocketAddr {
    start_with(content())
}

fn start_with(content: Content) -> SocketAddr {
    let mut c = config();
    c.sandbox.characters = vec![Character {
        character_id: BUILDER,
        name: "Builder1".to_owned(),
        ship_id: 100_000_701,
        balance: INVENTION_MIN_WALLET * 2.0,
        ..Character::default()
    }];
    c.content = Some(Arc::new(content));
    serve(c)
}

fn cycle(b: &mut Conn, content: &Content) -> builder::Cycle {
    builder::run(b, &[], Some(content)).expect("a cycle")
}

/// Wait for job `job_id` to reach its end time.
fn ready(b: &mut Conn, job_id: i64) {
    assert!(
        until(WITHIN, || b.industry_jobs().expect("jobs").iter().any(
            |j| j.job_id == job_id && j.ready_at(eve_server::pyutil::filetime_now())
        )),
        "job {job_id} never reached its end time"
    );
}

/// **SOC-B7, the row's test**: copy, invent, then build Tech II -- each
/// job the builder's own, from its cycle.
#[test]
fn the_builder_copies_invents_and_builds_tech_two() {
    let addr = start();
    let content = content();
    let mut b = login(addr, "Builder1");
    for skill in [industry::SKILL_INDUSTRY, industry::SKILL_SCIENCE] {
        gm(&mut b, &format!("/giveskill {skill} 1"));
    }
    gm(&mut b, &format!("/createitem {T1_BP} 1"));
    // Two attempts' datacores.
    gm(&mut b, &format!("/createitem {CORE_A} {}", 2 * PER_ATTEMPT));
    gm(&mut b, &format!("/createitem {CORE_B} {}", 2 * PER_ATTEMPT));

    // 1. A copy of the original, to invent from.
    let first = cycle(&mut b, &content);
    let [(copy_job, bp, activity, runs)] = first.lab[..] else {
        panic!("one lab job: {first:?}");
    };
    assert_eq!(
        (bp, activity, runs),
        (T1_BP, industry::COPYING, 1),
        "{first:?}"
    );
    assert!(
        first.installed.is_empty(),
        "no T1 build without a margin: {first:?}"
    );
    ready(&mut b, copy_job);

    // 2. Delivered; the invention, as many attempts as the cores cover.
    let second = cycle(&mut b, &content);
    assert!(
        second.delivered.iter().any(|d| d.0 == copy_job),
        "{second:?}"
    );
    let [(invent_job, bp, activity, runs)] = second.lab[..] else {
        panic!("one lab job: {second:?}");
    };
    assert_eq!(
        (bp, activity, runs),
        (T1_BP, industry::INVENTION, 2),
        "{second:?}"
    );
    ready(&mut b, invent_job);

    // 3. Delivered: a T2 copy, which waits for its component (stage D).
    let third = cycle(&mut b, &content);
    assert!(
        third.delivered.iter().any(|d| d.0 == invent_job),
        "{third:?}"
    );
    assert!(third.installed.is_empty(), "{third:?}");
    assert!(
        third.notes.iter().any(|n| n.contains("T2 copy")),
        "the missing component is a note: {third:?}"
    );
    let t2: Vec<_> = b
        .blueprints()
        .expect("blueprints")
        .into_iter()
        .filter(|r| r.type_id == T2_BP)
        .collect();
    assert!(
        !t2.is_empty() && t2.iter().all(|r| r.runs == T2_RUNS),
        "{t2:?}"
    );

    // 4. With its components, the T2 copy is built.
    gm(
        &mut b,
        &format!("/createitem {COMPONENT} {}", COMPONENTS_PER_RUN * T2_RUNS),
    );
    let fourth = cycle(&mut b, &content);
    let [(_, bp, runs)] = fourth.installed[..] else {
        panic!("one T2 job: {fourth:?}");
    };
    assert_eq!((bp, runs), (T2_BP, T2_RUNS), "{fourth:?}");
}

/// [`content_with`] the encryption skill, and an NPC exchange whose one
/// station, the builder's home, is a school's: its corporation lists no
/// book, and the encryption book is an invention skill nobody lists, so
/// the school asks it (`econ_exchange::SCHOOL_INVENTION_BOOKS`).
fn racial_content() -> Content {
    use eve_server::econ_exchange as exchange;
    let home = Character::default();
    let station = home.station_id.expect("the default pilot is docked");
    let table = exchange::Table::build(
        vec![exchange::Station {
            station_id: station,
            system_id: home.solar_system_id.expect("in a system"),
            region_id: home.region_id,
            owner_id: STATION_OWNER,
        }],
        &[],
        BTreeMap::from([(
            ENCRYPTION,
            exchange::TypeRow {
                group_id: ENCRYPTION_GROUP,
                category_id: exchange::CATEGORY_SKILL,
                base_price: 0.0,
                meta_group_id: None,
                tech_level: None,
            },
        )]),
        &[],
        &[exchange::School {
            corp_id: SCHOOL_CORP,
            stations: vec![station],
        }],
        &BTreeSet::new(),
        &BTreeSet::from([industry::SKILL_SCIENCE, ENCRYPTION]),
    )
    .expect("the exchange builds");
    let mut c = content_with(&[ENCRYPTION]);
    c.insert("econ_exchange", table);
    c
}

/// **GAPS-B, racial invention**: a builder whose invention needs an
/// encryption skill it lacks buys the book from its school station's NPC
/// ask, injects it and queues it; once trained, it invents.
#[test]
fn a_builder_buys_its_encryption_book_at_the_school_and_then_invents() {
    const { assert!(eve_server::econ_exchange::SCHOOL_INVENTION_BOOKS) };
    let content = racial_content();
    let addr = start_with(racial_content());
    let mut b = login(addr, "Builder1");
    for skill in [industry::SKILL_INDUSTRY, industry::SKILL_SCIENCE] {
        gm(&mut b, &format!("/giveskill {skill} 1"));
    }
    gm(&mut b, &format!("/createitem {T1_BP} 1"));
    gm(&mut b, &format!("/createitem {CORE_A} {PER_ATTEMPT}"));
    gm(&mut b, &format!("/createitem {CORE_B} {PER_ATTEMPT}"));

    // 1. The copy to invent from.
    let first = cycle(&mut b, &content);
    let [(copy_job, _, activity, _)] = first.lab[..] else {
        panic!("one lab job: {first:?}");
    };
    assert_eq!(activity, industry::COPYING, "{first:?}");
    ready(&mut b, copy_job);

    // 2. The invention is refused for the encryption skill: its book is
    //    bought from the school's ask, injected and queued.
    let before = b.cash_balance().expect("wallet");
    let second = cycle(&mut b, &content);
    assert!(second.lab.is_empty(), "{second:?}");
    assert!(
        second
            .notes
            .iter()
            .any(|n| n.contains(&format!("queued [{ENCRYPTION}]"))),
        "{second:?}"
    );
    let spent = before - b.cash_balance().expect("wallet");
    assert!(
        (spent - eve_server::econ_exchange::INVENTION_BOOK_ASK).abs() < 1.0,
        "the school's ask: spent {spent}"
    );
    assert!(
        !b.hangar_items()
            .expect("hangar")
            .iter()
            .any(|r| r.type_id == ENCRYPTION),
        "the book was injected"
    );

    // 3. Trained (a GM line stands in for the training time): it invents.
    gm(&mut b, &format!("/giveskill {ENCRYPTION} 1"));
    let third = cycle(&mut b, &content);
    let [(_, bp, activity, runs)] = third.lab[..] else {
        panic!("one lab job: {third:?}");
    };
    assert_eq!(
        (bp, activity, runs),
        (T1_BP, industry::INVENTION, 1),
        "{third:?}"
    );
}
