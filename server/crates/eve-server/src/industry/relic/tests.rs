//! Relics and decryptors (lane `t3-a`). **Invented ids and rows throughout**:
//! no client value is in this file. The numbers in the rows below (chances,
//! runs, decryptor attributes) are made up for the test, so a test reads them
//! from the row it built and no `[A]` constant is asserted.

use super::*;
use crate::industry::{
    BlueprintItem, FLAG_HANGAR, Facility, InstallRequest, Jobs, Material, Pilot, Refusal, Stack,
    invention_probability, invention_probability_with,
};
use crate::item::{self, Edit, Slot};
use crate::skill::Skills;

const ME: i64 = 90_000_001;
const STATION: i64 = 60_000_001;
const STATION_OWNER: i64 = 1_000_001;
const STATION_TYPE: i64 = 1_531;
const SYSTEM: i64 = 30_000_001;

// Relics: one family of two tiers, same datacores and skills.
const RELIC_INTACT: i64 = 990_801;
const RELIC_WRECKED: i64 = 990_802;
// Datacores, skills.
const CORE_A: i64 = 990_811;
const CORE_B: i64 = 990_812;
const SKILL_X: i64 = 990_821;
const SKILL_Y: i64 = 990_822;
const SKILL_Z: i64 = 990_823;
// Product blueprints (manufacturing rows exist for both) and their products.
const HULL_BP_A: i64 = 990_831;
const HULL_BP_B: i64 = 990_832;
const HULL_A: i64 = 990_841;
const HULL_B: i64 = 990_842;
// A reaction-like row and an inert one: neither is a relic.
const FORMULA: i64 = 990_861;
const INERT: i64 = 990_862;
// Decryptors, and a type that is not one.
const DECRYPTOR_SLOW: i64 = 990_851;
const DECRYPTOR_LUCKY: i64 = 990_852;
const NOT_A_DECRYPTOR: i64 = 990_853;

/// The invented numbers of the rows below.
const INTACT_RUNS: i64 = 20;
const INTACT_CHANCE: f64 = 0.30;
const WRECKED_RUNS: i64 = 3;
const WRECKED_CHANCE: f64 = 0.10;
const CORES_PER_RUN: i64 = 3;
/// The invented decryptor attributes (see [`content`]).
const SLOW_MULTIPLIER: f64 = 0.5;
const SLOW_RUNS: i64 = 7;

fn rows() -> Vec<String> {
    let mut r: Vec<String> = Vec::new();
    let mut relic = |bp: i64, runs: i64, chance: f64| {
        r.push(format!("{bp}\t8\tactivity\t0\t0\t\t3600\t300"));
        for core in [CORE_A, CORE_B] {
            r.push(format!(
                "{bp}\t8\tmaterial\t{core}\t{CORES_PER_RUN}\t\t3600\t300"
            ));
        }
        for hull in [HULL_BP_A, HULL_BP_B] {
            r.push(format!(
                "{bp}\t8\tproduct\t{hull}\t{runs}\t{chance}\t3600\t300"
            ));
        }
        for skill in [SKILL_X, SKILL_Y, SKILL_Z] {
            r.push(format!("{bp}\t8\tskill\t{skill}\t1\t\t3600\t300"));
        }
    };
    relic(RELIC_INTACT, INTACT_RUNS, INTACT_CHANCE);
    relic(RELIC_WRECKED, WRECKED_RUNS, WRECKED_CHANCE);
    for (bp, product) in [(HULL_BP_A, HULL_A), (HULL_BP_B, HULL_B)] {
        r.push(format!("{bp}\t1\tactivity\t0\t0\t\t900\t1"));
        r.push(format!("{bp}\t1\tmaterial\t34\t100\t\t900\t1"));
        r.push(format!("{bp}\t1\tproduct\t{product}\t1\t\t900\t1"));
        r.push(format!("{bp}\t1\tskill\t3380\t1\t\t900\t1"));
    }
    // A reaction formula: activity 9 only.
    r.push(format!("{FORMULA}\t9\tactivity\t0\t0\t\t600\t1"));
    r.push(format!("{FORMULA}\t9\tmaterial\t34\t10\t\t600\t1"));
    r.push(format!("{FORMULA}\t9\tproduct\t35\t10\t\t600\t1"));
    // Copying only: no manufacturing, no invention.
    r.push(format!("{INERT}\t5\tactivity\t0\t0\t\t600\t1"));
    r
}

fn table() -> Table {
    let prices = crate::econ_prices::Table::parse(&["34\t4.0\tbase\t1.0\t4\t18\t\t1"])
        .expect("invented prices parse");
    Table::parse(&rows(), &prices).expect("invented rows parse")
}

/// `typedata::FIXTURE` plus the invented types: relics (category 34, stack),
/// datacores, blueprints (category 9), two decryptors of group 1304 and one
/// type of another group carrying the same attributes.
fn content() -> crate::content::Content {
    let ty = |id: i64, group: i64, cat: i64, attrs: &str, name: &str| {
        format!("type\t{id}\t{group}\t{cat}\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t{attrs}\t{name}\n")
    };
    let slow = format!("1112={SLOW_MULTIPLIER};1113=2.0;1114=-2.0;1124={SLOW_RUNS}.0");
    let lucky = "1112=1.9;1113=-1.5;1114=6.0;1124=0.0";
    let mut rows = String::new();
    for (id, name) in [(RELIC_INTACT, "Intact"), (RELIC_WRECKED, "Wrecked")] {
        rows += &ty(id, 990_971, 34, "", &format!("Fixture {name} Relic"));
    }
    for id in [CORE_A, CORE_B] {
        rows += &ty(id, 990_333, 17, "", &format!("Fixture Core {id}"));
    }
    for id in [HULL_BP_A, HULL_BP_B] {
        rows += &ty(id, 990_105, 9, "", &format!("Fixture Blueprint {id}"));
    }
    rows += &ty(DECRYPTOR_SLOW, GROUP_DECRYPTORS, 35, &slow, "Fixture Slow");
    rows += &ty(
        DECRYPTOR_LUCKY,
        GROUP_DECRYPTORS,
        35,
        lucky,
        "Fixture Lucky",
    );
    rows += &ty(NOT_A_DECRYPTOR, 990_004, 17, &slow, "Fixture Imposter");
    let text = format!("{}{rows}", crate::typedata::FIXTURE);
    let parsed = crate::typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(parsed));
    c.insert("industry", table());
    c
}

thread_local! {
    static CONTENT: &'static crate::content::Content = Box::leak(Box::new(content()));
}

fn fx() -> &'static crate::content::Content {
    CONTENT.with(|c| *c)
}

fn t() -> &'static Table {
    fx().get::<Table>()
}

fn levels(pairs: &[(i64, i64)]) -> Skills {
    Skills::restore(
        pairs
            .iter()
            .map(|&(type_id, level)| crate::skill::SkillEntry {
                type_id,
                level,
                points: 0,
                rank: 1,
            })
            .collect(),
    )
}

/// All three relic skills at level 1.
fn trained() -> Skills {
    levels(&[(SKILL_X, 1), (SKILL_Y, 1), (SKILL_Z, 1)])
}

fn station() -> Facility {
    Facility::npc_station(STATION, STATION_OWNER, STATION_TYPE, SYSTEM)
}

fn pilot(skills: &Skills) -> Pilot<'_> {
    Pilot {
        character_id: ME,
        corporation_id: None,
        skills: Some(skills),
        implants: &[],
        balance: 1e12,
    }
}

/// The relic stack as an item in the station hangar.
fn relic_item(item_id: i64, type_id: i64, quantity: i64) -> item::Item {
    item::Item {
        item_id,
        type_id,
        owner_id: ME,
        location_id: STATION,
        flag_id: FLAG_HANGAR,
        quantity,
        group_id: 990_971,
        category_id: 34,
        singleton: false,
    }
}

fn request(blueprint_id: i64, runs: i64, product: Option<i64>) -> InstallRequest {
    InstallRequest {
        activity_id: INVENTION,
        blueprint_id,
        runs,
        input_location_id: STATION,
        input_flag_id: FLAG_HANGAR,
        output_location_id: STATION,
        output_flag_id: FLAG_HANGAR,
        licensed_runs: 0,
        product_type_id: product,
        decryptor: None,
    }
}

fn stack(item_id: i64, type_id: i64, quantity: i64) -> Stack {
    Stack {
        item_id,
        type_id,
        quantity,
        slot: Slot::new(ME, STATION, FLAG_HANGAR),
    }
}

/// Datacores for `runs` runs, and one of each decryptor kind.
fn hangar(runs: i64) -> Vec<Stack> {
    vec![
        stack(7_001, CORE_A, CORES_PER_RUN * runs),
        stack(7_002, CORE_B, CORES_PER_RUN * runs),
        stack(7_003, DECRYPTOR_SLOW, 1),
        stack(7_004, DECRYPTOR_LUCKY, 2),
        stack(7_005, NOT_A_DECRYPTOR, 1),
    ]
}

fn consumed(edits: &[Edit]) -> Vec<(i64, i64)> {
    edits
        .iter()
        .filter_map(|e| match e {
            Edit::Consume { item_id, quantity } => Some((*item_id, *quantity)),
            _ => None,
        })
        .collect()
}

fn install(
    jobs: &Jobs,
    skills: &Skills,
    blueprint: &BlueprintItem,
    request: &InstallRequest,
    on_hand: &[Stack],
) -> Result<crate::industry::Install, Refusal> {
    jobs.plan_install(
        t(),
        fx(),
        1_000,
        &pilot(skills),
        &station(),
        blueprint,
        request,
        on_hand,
    )
}

fn relic(jobs: &Jobs, item_id: i64, type_id: i64, quantity: i64) -> BlueprintItem {
    jobs.blueprint_item(t(), &relic_item(item_id, type_id, quantity))
        .expect("a relic is a blueprint")
}

/// A manufacturing original in the hangar, for the cases that are not relics.
fn plain_blueprint(item_id: i64) -> BlueprintItem {
    BlueprintItem::original_of(
        t(),
        &item::Item {
            item_id,
            type_id: HULL_BP_A,
            group_id: 990_105,
            category_id: 9,
            singleton: true,
            quantity: 1,
            ..relic_item(item_id, HULL_BP_A, 1)
        },
    )
    .expect("a blueprint")
}

/// Install, let the clock run out, deliver: the delivery and the finished job.
fn run_to_delivery(
    jobs: &mut Jobs,
    plan: &crate::industry::Install,
) -> (crate::industry::Delivery, crate::industry::Job) {
    jobs.commit_install(plan);
    let mut job = *jobs.get(plan.job.job_id).expect("committed");
    job.end_date = 1;
    jobs.commit(job);
    jobs.plan_delivery(t(), fx(), job.job_id, 10_i64.pow(18))
        .expect("delivers")
}

// ---------------------------------------------------------------------------
// T3-B: the table
// ---------------------------------------------------------------------------

#[test]
fn a_relic_row_is_kept_and_a_plain_blueprint_is_not_changed() {
    let table = t();
    assert_eq!(
        table
            .relics
            .iter()
            .map(|r| r.blueprint.blueprint_type_id)
            .collect::<Vec<_>>(),
        vec![RELIC_INTACT, RELIC_WRECKED],
        "both relics, ascending"
    );
    assert!(table.is_relic(RELIC_INTACT) && !table.is_relic(HULL_BP_A));
    assert!(
        !table.is_relic(FORMULA) && !table.is_relic(INERT),
        "a reaction formula and an inert row are not relics"
    );
    // Relics are not manufacturing blueprints: `find`, `science` and
    // `blueprints` hold the two hull blueprints exactly as before.
    assert!(table.find(RELIC_INTACT).is_none());
    assert!(!table.is_blueprint_type(RELIC_INTACT));
    assert_eq!(
        table
            .blueprints
            .iter()
            .map(|b| b.blueprint_type_id)
            .collect::<Vec<_>>(),
        vec![HULL_BP_A, HULL_BP_B]
    );
    assert_eq!(table.science.len(), table.blueprints.len());
    // `science` still answers for a relic, through the fallback.
    let inv = table
        .science(RELIC_INTACT)
        .and_then(|s| s.invention.as_ref())
        .expect("the relic's invention");
    assert_eq!(inv.time, 3600);
    assert_eq!(
        inv.materials,
        vec![(CORE_A, CORES_PER_RUN), (CORE_B, CORES_PER_RUN)]
    );
    assert_eq!(inv.skills, vec![(SKILL_X, 1), (SKILL_Y, 1), (SKILL_Z, 1)]);
    assert_eq!(
        inv.products.iter().map(|p| p.type_id).collect::<Vec<_>>(),
        vec![HULL_BP_A, HULL_BP_B]
    );
    assert_eq!(inv.products[0].quantity, INTACT_RUNS);
}

#[test]
fn relic_stack_is_listed_with_runs_equal_to_quantity() {
    let jobs = Jobs::new();
    let five = relic(&jobs, 500, RELIC_INTACT, 5);
    assert_eq!(five.runs_remaining, 5, "[A] one run per unit");
    assert!(!five.original(), "category 34 is never an original");
    assert_eq!(five.wire_quantity_in(t()), 5, "a positive stack size");
    assert_eq!((five.material_efficiency, five.time_efficiency), (0, 0));
    assert_eq!(five.blueprint(t()).map(|b| b.product_type_id), Some(0));
    // A plain blueprint keeps its own wire quantity.
    assert_eq!(plain_blueprint(501).wire_quantity_in(t()), -1);
    // A type that is neither is not a blueprint item at all.
    assert!(
        jobs.blueprint_item(t(), &relic_item(502, CORE_A, 4))
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// T3-B: the job
// ---------------------------------------------------------------------------

#[test]
fn relic_job_consumes_one_unit_and_two_datacores_per_run() {
    let jobs = Jobs::new();
    let skills = trained();
    let stack_of_five = relic(&jobs, 500, RELIC_INTACT, 5);
    let runs = 3;
    let plan = install(
        &jobs,
        &skills,
        &stack_of_five,
        &request(500, runs, Some(HULL_BP_A)),
        &hangar(runs),
    )
    .expect("a relic job");
    let took = consumed(&plan.edits);
    assert!(
        took.contains(&(500, runs)),
        "{runs} units of the stack: {took:?}"
    );
    assert!(took.contains(&(7_001, CORES_PER_RUN * runs)));
    assert!(took.contains(&(7_002, CORES_PER_RUN * runs)));
    assert_eq!(took.len(), 3, "nothing else: {took:?}");
    assert_eq!(plan.blueprint_after, None, "a relic has no runs row");
    assert_eq!(plan.job.activity_id, INVENTION);
    assert!(plan.job.blueprint_copy);
    assert_eq!(plan.job.product_type_id, HULL_BP_A);
    assert_eq!(plan.job.runs, runs);
    // More runs than units is INVALID_RUNS with the stack as the ceiling.
    assert_eq!(
        install(
            &jobs,
            &skills,
            &stack_of_five,
            &request(500, 6, Some(HULL_BP_A)),
            &hangar(6)
        ),
        Err(Refusal::InvalidRuns(5))
    );
    // Short of a datacore: MISSING_MATERIAL (16).
    let mut short = hangar(runs);
    short[0].quantity -= 1;
    assert_eq!(
        install(
            &jobs,
            &skills,
            &stack_of_five,
            &request(500, runs, Some(HULL_BP_A)),
            &short
        )
        .map_err(|e| e.code()),
        Err(16)
    );
}

#[test]
fn relic_job_needs_the_three_skills() {
    let jobs = Jobs::new();
    let one = relic(&jobs, 500, RELIC_INTACT, 1);
    for missing in [SKILL_X, SKILL_Y, SKILL_Z] {
        let pairs: Vec<(i64, i64)> = [SKILL_X, SKILL_Y, SKILL_Z]
            .into_iter()
            .filter(|&s| s != missing)
            .map(|s| (s, 1))
            .collect();
        assert_eq!(
            install(
                &jobs,
                &levels(&pairs),
                &one,
                &request(500, 1, Some(HULL_BP_A)),
                &hangar(1)
            ),
            Err(Refusal::MissingSkill(missing, 1, 0)),
            "without {missing}"
        );
    }
}

#[test]
fn intact_malfunctioning_wrecked_have_their_own_runs_and_chance() {
    let skills = trained();
    let skill_factor = 1.0
        + [SKILL_X, SKILL_Y, SKILL_Z]
            .into_iter()
            .map(crate::industry::skill_probability)
            .sum::<f64>();
    for (type_id, chance, runs) in [
        (RELIC_INTACT, INTACT_CHANCE, INTACT_RUNS),
        (RELIC_WRECKED, WRECKED_CHANCE, WRECKED_RUNS),
    ] {
        let mut jobs = Jobs::new();
        let item = relic(&jobs, 500, type_id, 100);
        // A hundred runs so that some succeed at any of the chances.
        let plan = install(
            &jobs,
            &skills,
            &item,
            &request(500, 100, Some(HULL_BP_A)),
            &hangar(100),
        )
        .expect("installs");
        assert_eq!(plan.job.probability, chance * skill_factor, "{type_id}");
        let (delivery, done) = run_to_delivery(&mut jobs, &plan);
        assert!(done.successful_runs > 0, "{type_id}");
        let state = delivery.creates.expect("a copy template");
        assert_eq!(state.runs_remaining, runs, "{type_id}");
    }
}

#[test]
fn the_chosen_product_is_required_when_a_relic_has_many() {
    let jobs = Jobs::new();
    let skills = trained();
    let one = relic(&jobs, 500, RELIC_INTACT, 1);
    assert_eq!(
        install(&jobs, &skills, &one, &request(500, 1, None), &hangar(1)),
        Err(Refusal::InvalidProduct)
    );
    // A type the relic does not make is refused too.
    assert_eq!(
        install(
            &jobs,
            &skills,
            &one,
            &request(500, 1, Some(HULL_A)),
            &hangar(1)
        ),
        Err(Refusal::InvalidProduct)
    );
    let plan = install(
        &jobs,
        &skills,
        &one,
        &request(500, 1, Some(HULL_BP_B)),
        &hangar(1),
    )
    .expect("the second product");
    assert_eq!(plan.job.product_type_id, HULL_BP_B);
}

#[test]
fn delivery_makes_copies_with_me_2_te_4() {
    assert_eq!(
        copy_start(INTACT_RUNS, None),
        CopyStart {
            material_efficiency: INVENTION_MATERIAL_EFFICIENCY,
            time_efficiency: INVENTION_TIME_EFFICIENCY,
            runs: INTACT_RUNS,
        }
    );
    let mut jobs = Jobs::new();
    let skills = levels(&[(SKILL_X, 5), (SKILL_Y, 5), (SKILL_Z, 5)]);
    let stack_of_many = relic(&jobs, 500, RELIC_INTACT, 300);
    let runs = 200;
    let plan = install(
        &jobs,
        &skills,
        &stack_of_many,
        &request(500, runs, Some(HULL_BP_A)),
        &hangar(runs),
    )
    .expect("installs");
    let (delivery, done) = run_to_delivery(&mut jobs, &plan);
    assert!(done.successful_runs > 0 && done.successful_runs <= runs);
    assert_eq!(
        delivery.edits,
        vec![Edit::Create {
            type_id: HULL_BP_A,
            quantity: done.successful_runs,
            to: Slot::new(ME, STATION, FLAG_HANGAR),
        }]
    );
    let state = delivery.creates.expect("a copy template");
    assert_eq!(
        (
            state.material_efficiency,
            state.time_efficiency,
            state.runs_remaining
        ),
        (
            INVENTION_MATERIAL_EFFICIENCY,
            INVENTION_TIME_EFFICIENCY,
            INTACT_RUNS
        )
    );
}

#[test]
fn relic_in_another_station_is_refused() {
    let jobs = Jobs::new();
    let skills = trained();
    let mut elsewhere = relic(&jobs, 500, RELIC_INTACT, 1);
    elsewhere.location_id = STATION + 1;
    assert_eq!(
        install(
            &jobs,
            &skills,
            &elsewhere,
            &request(500, 1, Some(HULL_BP_A)),
            &hangar(1)
        ),
        Err(Refusal::BlueprintAccess)
    );
    // And somebody else's stack is not yours.
    let mut theirs = relic(&jobs, 500, RELIC_INTACT, 1);
    theirs.owner_id = ME + 1;
    assert_eq!(
        install(
            &jobs,
            &skills,
            &theirs,
            &request(500, 1, Some(HULL_BP_A)),
            &hangar(1)
        ),
        Err(Refusal::InvalidOwner)
    );
}

#[test]
fn what_is_left_of_a_stack_is_free_for_a_second_job() {
    let mut jobs = Jobs::new();
    // A second laboratory slot, so that the slot is not what refuses.
    let skills = levels(&[
        (SKILL_X, 1),
        (SKILL_Y, 1),
        (SKILL_Z, 1),
        (crate::industry::SKILL_LABORATORY_OPERATION, 1),
    ]);
    let stack_of_four = relic(&jobs, 500, RELIC_INTACT, 4);
    let first = install(
        &jobs,
        &skills,
        &stack_of_four,
        &request(500, 2, Some(HULL_BP_A)),
        &hangar(2),
    )
    .expect("first");
    jobs.commit_install(&first);
    // The stack as the store now has it: two units left, a job on its id.
    let rest = relic(&jobs, 500, RELIC_INTACT, 2);
    let second = install(
        &jobs,
        &skills,
        &rest,
        &request(500, 2, Some(HULL_BP_B)),
        &hangar(2),
    );
    assert!(second.is_ok(), "{second:?}");
}

// ---------------------------------------------------------------------------
// T3-C: decryptors
// ---------------------------------------------------------------------------

#[test]
fn decryptor_probability_multiplies_in_and_clamps_to_one() {
    let inv = t()
        .science(RELIC_INTACT)
        .and_then(|s| s.invention.as_ref())
        .expect("a relic");
    // No decryptor is the plain chance, and the old entry point agrees.
    assert_eq!(invention_probability_with(inv, |_| 0, 1.0), INTACT_CHANCE);
    assert_eq!(invention_probability(inv, |_| 0), INTACT_CHANCE);
    // A multiplier multiplies.
    assert_eq!(
        invention_probability_with(inv, |_| 0, SLOW_MULTIPLIER),
        INTACT_CHANCE * SLOW_MULTIPLIER
    );
    // The clamp is over the whole product: skills past certainty, brought
    // back under it by a multiplier below one, are not clamped first.
    let skills = 1.0 + 3.0 * 5.0 * crate::industry::skill_probability(SKILL_X);
    let at_five = |_: i64| 5;
    assert_eq!(
        invention_probability_with(inv, at_five, 2.0),
        (INTACT_CHANCE * skills * 2.0).clamp(0.0, 1.0)
    );
    assert!(INTACT_CHANCE * skills > 0.3);
    // A big multiplier cannot pass certainty, nor a negative go below zero.
    assert_eq!(invention_probability_with(inv, at_five, 1_000.0), 1.0);
    assert_eq!(invention_probability_with(inv, at_five, -1.0), 0.0);
}

#[test]
fn decryptor_me_te_are_percent_added_to_2_and_4() {
    let slow = Decryptor::of(fx(), DECRYPTOR_SLOW).expect("a decryptor");
    assert_eq!(slow.probability_multiplier, SLOW_MULTIPLIER);
    let s = copy_start(10, Some(&slow));
    assert_eq!(s.material_efficiency, INVENTION_MATERIAL_EFFICIENCY + 2);
    assert_eq!(
        s.time_efficiency,
        INVENTION_TIME_EFFICIENCY - 2,
        "a negative modifier lowers it"
    );
    // The client's `int()` truncates toward zero: -1.5 is -1, 6.0 is 6.
    let lucky = Decryptor::of(fx(), DECRYPTOR_LUCKY).expect("a decryptor");
    let s = copy_start(10, Some(&lucky));
    assert_eq!(s.material_efficiency, INVENTION_MATERIAL_EFFICIENCY - 1);
    assert_eq!(s.time_efficiency, INVENTION_TIME_EFFICIENCY + 6);
}

#[test]
fn decryptor_runs_added_to_product_quantity() {
    let slow = Decryptor::of(fx(), DECRYPTOR_SLOW).expect("a decryptor");
    assert_eq!(
        copy_start(WRECKED_RUNS, Some(&slow)).runs,
        WRECKED_RUNS + SLOW_RUNS
    );
    assert_eq!(copy_start(WRECKED_RUNS, None).runs, WRECKED_RUNS);
    // `int(round(quantity + maxRuns))`: half away from zero.
    let half = Decryptor {
        max_runs: 0.5,
        ..slow
    };
    assert_eq!(copy_start(3, Some(&half)).runs, 4);
}

#[test]
fn a_decryptor_outside_group_1304_is_refused() {
    let jobs = Jobs::new();
    let skills = trained();
    let one = relic(&jobs, 500, RELIC_INTACT, 1);
    let mut asked = request(500, 1, Some(HULL_BP_A));
    asked.decryptor = Some(NOT_A_DECRYPTOR);
    assert_eq!(
        install(&jobs, &skills, &one, &asked, &hangar(1)),
        Err(Refusal::MismatchMaterial(NOT_A_DECRYPTOR))
    );
    assert_eq!(Refusal::MismatchMaterial(1).code(), 28);
    // A type the content does not have at all.
    asked.decryptor = Some(1);
    assert_eq!(
        install(&jobs, &skills, &one, &asked, &hangar(1)),
        Err(Refusal::MismatchMaterial(1))
    );
    // `decryptor_among` only ever names a real one; the last key wins.
    assert_eq!(
        decryptor_among(fx(), &[CORE_A, NOT_A_DECRYPTOR, DECRYPTOR_SLOW, CORE_B]),
        Some(DECRYPTOR_SLOW)
    );
    assert_eq!(
        decryptor_among(fx(), &[DECRYPTOR_SLOW, DECRYPTOR_LUCKY]),
        Some(DECRYPTOR_LUCKY)
    );
    assert_eq!(decryptor_among(fx(), &[CORE_A, NOT_A_DECRYPTOR]), None);
    // A decryptor on a manufacturing job is not offered either.
    let plain = InstallRequest {
        activity_id: crate::industry::MANUFACTURING,
        decryptor: Some(DECRYPTOR_SLOW),
        ..request(600, 1, None)
    };
    let skilled = levels(&[(crate::industry::SKILL_INDUSTRY, 1)]);
    assert_eq!(
        install(&jobs, &skilled, &plain_blueprint(600), &plain, &[]),
        Err(Refusal::MismatchMaterial(DECRYPTOR_SLOW))
    );
}

#[test]
fn a_chosen_decryptor_is_consumed_scales_the_chance_and_reaches_the_copy() {
    let mut jobs = Jobs::new();
    let skills = trained();
    let stack_of_many = relic(&jobs, 500, RELIC_INTACT, 300);
    let plain = install(
        &jobs,
        &skills,
        &stack_of_many,
        &request(500, 1, Some(HULL_BP_A)),
        &hangar(1),
    )
    .expect("plain");
    let mut asked = request(500, 1, Some(HULL_BP_A));
    asked.decryptor = Some(DECRYPTOR_SLOW);
    let with =
        install(&jobs, &skills, &stack_of_many, &asked, &hangar(1)).expect("with a decryptor");
    assert_eq!(
        with.job.probability,
        plain.job.probability * SLOW_MULTIPLIER
    );
    assert_eq!(with.job.optional_type_id, Some(DECRYPTOR_SLOW));
    assert!(
        with.materials.contains(&Material {
            type_id: DECRYPTOR_SLOW,
            quantity: 1
        }),
        "one unit: {:?}",
        with.materials
    );
    assert!(consumed(&with.edits).contains(&(7_003, 1)));
    // Without one in the hangar: MISSING_MATERIAL.
    let none: Vec<Stack> = hangar(1)
        .into_iter()
        .filter(|s| s.type_id != DECRYPTOR_SLOW)
        .collect();
    assert_eq!(
        install(&jobs, &skills, &stack_of_many, &asked, &none).map_err(|e| e.code()),
        Err(16)
    );
    // Two hundred runs with it: the delivered copy carries its ME, TE, runs.
    let mut asked = request(500, 200, Some(HULL_BP_A));
    asked.decryptor = Some(DECRYPTOR_SLOW);
    let big = install(
        &jobs,
        &levels(&[(SKILL_X, 5), (SKILL_Y, 5), (SKILL_Z, 5)]),
        &stack_of_many,
        &asked,
        &hangar(200),
    )
    .expect("200 runs");
    let (delivery, done) = run_to_delivery(&mut jobs, &big);
    assert!(done.successful_runs > 0);
    let state = delivery.creates.expect("copies");
    assert_eq!(state.runs_remaining, INTACT_RUNS + SLOW_RUNS);
    assert_eq!(state.material_efficiency, INVENTION_MATERIAL_EFFICIENCY + 2);
    assert_eq!(state.time_efficiency, INVENTION_TIME_EFFICIENCY - 2);
}
