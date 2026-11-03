//! Lane `t3-h`: the pure rules. **Invented ids and numbers throughout**; the
//! only real ids are the attributes the code under test reads (`subSystemSlot`
//! 1366, `fitsToShipType` 1380, `requiredSkill1` 182 and its level 277) and the
//! group ids the server itself names (`subsystems`, `stance`).

use super::*;
use crate::builder::INVENTION_BATCH;
use eve_server::subsystems::{CATEGORY_SUBSYSTEM, FIRST_FLAG, GROUP_STRATEGIC_CRUISER};

const HULL_A: i64 = 990_970_001;
const HULL_B: i64 = 990_970_002;
const DESTROYER: i64 = 990_970_003;
/// A modular hull with only three slots' subsystems.
const HULL_THIN: i64 = 990_970_004;
const SKILL: i64 = 990_970_901;
const SKILL_LEVEL: i64 = 3;
const CORE_X: i64 = 990_970_801;
const CORE_Y: i64 = 990_970_802;
const PER_RUN: i64 = 3;
const STATION: i64 = 60_000_777;
const SUB_GROUP: i64 = 990_958;
const HULL_RUNS: i64 = 20;

/// Subsystems of [`HULL_A`], one per flag, then a second one for flag 0.
const SUB_A: [i64; 4] = [990_970_011, 990_970_012, 990_970_013, 990_970_014];
const SUB_A_ALT: i64 = 990_970_015;
const SUB_B: [i64; 4] = [990_970_021, 990_970_022, 990_970_023, 990_970_024];
const SUB_THIN: [i64; 3] = [990_970_031, 990_970_032, 990_970_033];

/// The blueprint of an item: invented as the item id + 1000.
fn bp(item: i64) -> i64 {
    item + 1000
}

/// Relic families: hull relics (two tiers), one per subsystem flag, small.
const RELIC_HULL: i64 = 990_972_001;
const RELIC_HULL_WRECKED: i64 = 990_972_002;
const RELIC_FLAG: [i64; 4] = [990_972_011, 990_972_012, 990_972_013, 990_972_014];
const RELIC_SMALL: i64 = 990_972_021;
const WRECKED_RUNS: i64 = 3;

fn hull_row(id: i64, group: i64, name: &str, skill: bool) -> String {
    let attrs = if skill {
        format!("182={SKILL}.0;277={SKILL_LEVEL}.0")
    } else {
        String::new()
    };
    format!(
        "type\t{id}\t{group}\t6\t1\t1\t100000.0\t0.0\t12000000.0\t200.0\t0\t\t{attrs}\t{name}\n"
    )
}

fn sub_row(id: i64, flag: i64, fits: i64) -> String {
    format!(
        "type\t{id}\t{SUB_GROUP}\t{CATEGORY_SUBSYSTEM}\t1\t1\t40.0\t0.0\t1200000.0\t1.0\t0\t\t\
         1366={flag}.0;1380={fits}.0;182={SKILL}.0;277=1.0\tFixture Subsystem {id}\n"
    )
}

fn content() -> Content {
    let mut text = String::from(typedata::FIXTURE);
    text += &hull_row(HULL_A, GROUP_STRATEGIC_CRUISER, "Fixture Hull A", true);
    text += &hull_row(HULL_B, GROUP_STRATEGIC_CRUISER, "Fixture Hull B", false);
    text += &hull_row(HULL_THIN, GROUP_STRATEGIC_CRUISER, "Fixture Thin", false);
    text += &hull_row(
        DESTROYER,
        stance::GROUP_TACTICAL_DESTROYER,
        "Fixture Destroyer",
        false,
    );
    for (n, id) in SUB_A.iter().enumerate() {
        text += &sub_row(*id, FIRST_FLAG + n as i64, HULL_A);
    }
    text += &sub_row(SUB_A_ALT, FIRST_FLAG, HULL_A);
    for (n, id) in SUB_B.iter().enumerate() {
        text += &sub_row(*id, FIRST_FLAG + n as i64, HULL_B);
    }
    for (n, id) in SUB_THIN.iter().enumerate() {
        text += &sub_row(*id, FIRST_FLAG + n as i64, HULL_THIN);
    }
    // The destroyer's three modes, named after it (the server's rule).
    for (n, suffix) in ["Defense Mode", "Propulsion Mode", "Sharpshooter Mode"]
        .iter()
        .enumerate()
    {
        text += &format!(
            "type\t{}\t{}\t6\t0\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t\tFixture Destroyer {suffix}\n",
            990_970_041 + n as i64,
            stance::GROUP_STANCE_MODE
        );
    }
    let table = typedata::Table::parse(&text).expect("the invented rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", typedata::Installed::build(table));
    c
}

fn recipes() -> industry::Table {
    let mut r: Vec<String> = Vec::new();
    let mut relic = |id: i64, runs: i64, products: &[i64]| {
        r.push(format!("{id}\t8\tactivity\t0\t0\t\t3600\t300"));
        for core in [CORE_X, CORE_Y] {
            r.push(format!("{id}\t8\tmaterial\t{core}\t{PER_RUN}\t\t3600\t300"));
        }
        for p in products {
            r.push(format!(
                "{id}\t8\tproduct\t{}\t{runs}\t0.25\t3600\t300",
                bp(*p)
            ));
        }
        r.push(format!("{id}\t8\tskill\t{SKILL}\t1\t\t3600\t300"));
    };
    relic(RELIC_HULL, HULL_RUNS, &[HULL_A, HULL_B]);
    relic(RELIC_HULL_WRECKED, WRECKED_RUNS, &[HULL_A, HULL_B]);
    relic(RELIC_SMALL, HULL_RUNS, &[DESTROYER]);
    let flag_products: [Vec<i64>; 4] = std::array::from_fn(|i| vec![SUB_A[i], SUB_B[i]]);
    for (i, id) in RELIC_FLAG.iter().enumerate() {
        relic(*id, HULL_RUNS, &flag_products[i]);
    }
    let mut every: Vec<i64> = vec![HULL_A, HULL_B, DESTROYER];
    every.extend(SUB_A);
    every.extend(SUB_B);
    for item in every {
        r.push(format!("{}\t1\tactivity\t0\t0\t\t900\t1", bp(item)));
        r.push(format!("{}\t1\tmaterial\t34\t100\t\t900\t1", bp(item)));
        r.push(format!("{}\t1\tproduct\t{item}\t1\t\t900\t1", bp(item)));
        r.push(format!("{}\t1\tskill\t3380\t1\t\t900\t1", bp(item)));
    }
    industry::Table::parse(&r, &eve_server::econ_prices::Table::default()).expect("recipes parse")
}

fn row(item_id: i64, type_id: i64, category_id: i64, group_id: i64) -> ItemRow {
    ItemRow {
        item_id,
        type_id,
        owner_id: 90_000_001,
        location_id: STATION,
        flag_id: crate::FLAG_HANGAR,
        quantity: 1,
        group_id,
        category_id,
        singleton: false,
    }
}

fn copy_of(item_id: i64, item: i64, runs: i64) -> BlueprintRow {
    BlueprintRow {
        item_id,
        type_id: bp(item),
        location_id: STATION,
        flag_id: crate::FLAG_HANGAR,
        runs,
        material_efficiency: 2,
        job_id: None,
    }
}

fn relic_stack(item_id: i64, type_id: i64, units: i64) -> BlueprintRow {
    BlueprintRow {
        item_id,
        type_id,
        location_id: STATION,
        flag_id: crate::FLAG_HANGAR,
        runs: units,
        material_efficiency: 0,
        job_id: None,
    }
}

#[test]
fn the_catalog_is_found_by_group_and_leaves_out_a_hull_missing_a_slot() {
    let kits = catalog(&content());
    let hulls: Vec<i64> = kits.iter().map(|k| k.hull).collect();
    assert_eq!(hulls, [HULL_A, HULL_B, DESTROYER], "{kits:?}");
    let a = &kits[0];
    assert!(a.modular() && a.subs.len() == 4);
    assert_eq!(a.subs[0], [SUB_A[0], SUB_A_ALT], "two for flag one");
    assert_eq!(a.pieces().len(), 5);
    let d = &kits[2];
    assert!(!d.modular() && d.pieces() == [vec![DESTROYER]]);
}

#[test]
fn invention_products_map_to_the_items_they_make() {
    let can = inventable(&recipes());
    assert_eq!(can.get(&HULL_A), Some(&bp(HULL_A)));
    assert_eq!(can.get(&SUB_B[3]), Some(&bp(SUB_B[3])));
    assert!(!can.contains_key(&SUB_A_ALT), "no relic invents it");
    assert!(!can.contains_key(&SUB_THIN[0]));
}

#[test]
fn relics_come_best_tier_first() {
    let r = recipes();
    assert_eq!(
        relics_for(&r, bp(HULL_A)),
        [RELIC_HULL, RELIC_HULL_WRECKED],
        "the intact one (20 runs) before the wrecked (3)"
    );
    assert_eq!(relics_for(&r, bp(SUB_A[2])), [RELIC_FLAG[2]]);
    assert!(relics_for(&r, bp(SUB_A_ALT)).is_empty());
}

#[test]
fn a_builder_finishes_the_kit_it_has_begun() {
    let kits = catalog(&content());
    let r = recipes();
    let held = BTreeMap::new();
    // Nothing yet: seed 0 prefers a strategic cruiser, the first by hull id
    // (seed / 2 = 0); seed 2 picks the second of the doctrine.
    let a = doctrine(&kits, &r, &[], &held, 0).expect("a kit");
    assert_eq!(a.hull, HULL_A);
    let b = doctrine(&kits, &r, &[], &held, 2).expect("a kit");
    assert_eq!(b.hull, HULL_B);
    // An odd seed prefers a destroyer.
    let d = doctrine(&kits, &r, &[], &held, 1).expect("a kit");
    assert_eq!(d.hull, DESTROYER);
    // It holds one of B's subsystem copies: B wins for every seed that
    // prefers a cruiser.
    let owned = [copy_of(1, SUB_B[1], HULL_RUNS)];
    for seed in [0, 2, 4] {
        assert_eq!(
            doctrine(&kits, &r, &owned, &held, seed).map(|k| k.hull),
            Some(HULL_B),
            "seed {seed}"
        );
    }
}

#[test]
fn wants_lists_each_missing_piece_once_and_a_held_item_covers_one() {
    let kits = catalog(&content());
    let r = recipes();
    let a = &kits[0];
    let none = wants(a, &r, &[], &BTreeMap::new());
    assert_eq!(
        none,
        [
            bp(HULL_A),
            bp(SUB_A[0]),
            bp(SUB_A[1]),
            bp(SUB_A[2]),
            bp(SUB_A[3])
        ]
    );
    let owned = [copy_of(1, HULL_A, 1)];
    let held = BTreeMap::from([(SUB_A[2], 1)]);
    assert_eq!(
        wants(a, &r, &owned, &held),
        [bp(SUB_A[0]), bp(SUB_A[1]), bp(SUB_A[3])],
        "a copy covers the hull, a built subsystem its slot"
    );
    // A copy with no runs left covers nothing.
    let spent = [copy_of(1, HULL_A, 0)];
    assert_eq!(wants(a, &r, &spent, &BTreeMap::new()).len(), 5);
}

#[test]
fn an_invention_beats_buying_cores_which_beats_buying_a_relic() {
    let kits = catalog(&content());
    let r = recipes();
    let seed = 0; // HULL_A
    // Nothing held: the first want's relic family is bought.
    assert_eq!(
        plan_step(&kits, &r, STATION, &[], &BTreeMap::new(), seed),
        Some(Step::Relic(vec![RELIC_HULL, RELIC_HULL_WRECKED]))
    );
    // A hull relic stack, no cores: buy cores for T3_BATCH attempts.
    let stack = relic_stack(7, RELIC_HULL, 5);
    let short = plan_step(&kits, &r, STATION, &[stack], &BTreeMap::new(), seed);
    let per = PER_RUN * T3_BATCH;
    assert_eq!(
        short,
        Some(Step::Datacores(vec![(CORE_X, per), (CORE_Y, per)]))
    );
    // Cores for two attempts: invent with two runs, aimed at the hull.
    let held = BTreeMap::from([(CORE_X, 2 * PER_RUN), (CORE_Y, 2 * PER_RUN)]);
    let step = plan_step(&kits, &r, STATION, &[stack], &held, seed);
    assert_eq!(
        step,
        Some(Step::Invent {
            relic: stack,
            product: bp(HULL_A),
            runs: 2
        })
    );
    // A relic for the *second* want with cores beats a first want still
    // needing its relic.
    let flag = relic_stack(8, RELIC_FLAG[1], 1);
    let step = plan_step(&kits, &r, STATION, &[flag], &held, seed);
    assert_eq!(
        step,
        Some(Step::Invent {
            relic: flag,
            product: bp(SUB_A[1]),
            runs: 1
        })
    );
    // The runs never pass the stack, nor the batch.
    let big = BTreeMap::from([(CORE_X, 99 * PER_RUN), (CORE_Y, 99 * PER_RUN)]);
    let step = plan_step(
        &kits,
        &r,
        STATION,
        &[relic_stack(9, RELIC_HULL, 99)],
        &big,
        seed,
    );
    assert!(matches!(step, Some(Step::Invent { runs, .. }) if runs == T3_BATCH));
    const { assert!(T3_BATCH <= INVENTION_BATCH) };
}

#[test]
fn a_relic_in_another_station_or_in_a_job_is_not_used() {
    let kits = catalog(&content());
    let r = recipes();
    let mut away = relic_stack(7, RELIC_HULL, 5);
    away.location_id = STATION + 1;
    let mut busy = relic_stack(8, RELIC_HULL, 5);
    busy.job_id = Some(1);
    for b in [away, busy] {
        assert!(matches!(
            plan_step(&kits, &r, STATION, &[b], &BTreeMap::new(), 0),
            Some(Step::Relic(_))
        ));
    }
}

#[test]
fn a_whole_kit_is_assembled_from_distinct_items() {
    let kits = catalog(&content());
    let hull_a = row(1, HULL_A, acquire::CATEGORY_SHIP, GROUP_STRATEGIC_CRUISER);
    let subs: Vec<ItemRow> = SUB_A
        .iter()
        .enumerate()
        .map(|(n, t)| row(10 + n as i64, *t, CATEGORY_SUBSYSTEM, SUB_GROUP))
        .collect();
    let mut hangar = vec![hull_a];
    hangar.extend(subs.iter().take(3).copied());
    assert!(
        ready(&kits, &hangar).is_empty(),
        "three of four is not a kit"
    );
    hangar.push(subs[3]);
    let got = ready(&kits, &hangar);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].hull_item, 1);
    assert_eq!(
        got[0].subs,
        SUB_A
            .iter()
            .zip(10..)
            .map(|(t, i)| (*t, i))
            .collect::<Vec<_>>(),
        "in flag order"
    );
    // The alternate subsystem for flag one also does; other hulls' do not.
    let mut other = hangar.clone();
    other.retain(|r| r.type_id != SUB_A[0]);
    other.push(row(20, SUB_A_ALT, CATEGORY_SUBSYSTEM, SUB_GROUP));
    assert_eq!(ready(&kits, &other).len(), 1);
    let mut wrong = hangar;
    wrong.retain(|r| r.type_id != SUB_A[0]);
    wrong.push(row(21, SUB_B[0], CATEGORY_SUBSYSTEM, SUB_GROUP));
    assert!(ready(&kits, &wrong).is_empty());
}

#[test]
fn a_destroyer_needs_only_its_hull_and_a_fitted_item_is_not_free() {
    let kits = catalog(&content());
    let d = row(
        5,
        DESTROYER,
        acquire::CATEGORY_SHIP,
        stance::GROUP_TACTICAL_DESTROYER,
    );
    assert_eq!(ready(&kits, &[d]).len(), 1);
    let mut fitted = d;
    fitted.flag_id = 27;
    assert!(ready(&kits, &[fitted]).is_empty());
}

#[test]
fn a_cruiser_kit_comes_before_a_destroyer_and_items_are_used_once() {
    let kits = catalog(&content());
    let mut hangar = vec![
        row(1, HULL_A, acquire::CATEGORY_SHIP, GROUP_STRATEGIC_CRUISER),
        row(
            2,
            DESTROYER,
            acquire::CATEGORY_SHIP,
            stance::GROUP_TACTICAL_DESTROYER,
        ),
    ];
    for (n, t) in SUB_A.iter().enumerate() {
        hangar.push(row(10 + n as i64, *t, CATEGORY_SUBSYSTEM, SUB_GROUP));
    }
    let got = ready(&kits, &hangar);
    assert_eq!(
        got.iter().map(|a| a.hull_type).collect::<Vec<_>>(),
        [HULL_A, DESTROYER]
    );
}

#[test]
fn skills_come_from_the_types_and_the_highest_level_counts() {
    let c = content();
    let types = [HULL_A, SUB_A[0], SUB_A[1]];
    assert_eq!(
        lacking_skills(&c, &types, &BTreeMap::new()),
        [(SKILL, SKILL_LEVEL)],
        "the hull asks more than the subsystems"
    );
    let trained = BTreeMap::from([(SKILL, SKILL_LEVEL)]);
    assert!(lacking_skills(&c, &types, &trained).is_empty());
    let low = BTreeMap::from([(SKILL, SKILL_LEVEL - 1)]);
    assert_eq!(lacking_skills(&c, &[HULL_A], &low).len(), 1);
    assert!(lacking_skills(&c, &[HULL_B], &BTreeMap::new()).is_empty());
}

#[test]
fn the_escort_keeps_a_t3_hull() {
    let hangar = [
        row(1, HULL_A, acquire::CATEGORY_SHIP, GROUP_STRATEGIC_CRUISER),
        row(2, 990_970_099, acquire::CATEGORY_SHIP, 25),
    ];
    assert!(is_flying(&hangar, Some(1)));
    assert!(!is_flying(&hangar, Some(2)));
    assert!(!is_flying(&hangar, None));
}

#[test]
fn a_destroyer_stance_is_picked_by_the_servers_rule_when_the_hull_offers_it() {
    let c = content();
    assert_eq!(
        stance_wanted(&c, DESTROYER, 0.9, Some(1.0), 10.0),
        Some(stance::STANCE_DEFENCE)
    );
    assert_eq!(
        stance_wanted(&c, DESTROYER, 0.0, Some(5.0), 10.0),
        Some(stance::STANCE_SNIPER)
    );
    assert_eq!(
        stance_wanted(&c, DESTROYER, 0.0, None, 10.0),
        Some(stance::STANCE_SPEED)
    );
    assert_eq!(stance_wanted(&c, HULL_A, 0.9, None, 10.0), None);
}

#[test]
fn the_tech_two_laboratory_leaves_relics_to_this_module() {
    let r = recipes();
    let held = BTreeMap::from([(CORE_X, 99), (CORE_Y, 99)]);
    let stack = relic_stack(7, RELIC_HULL, 5);
    assert_eq!(builder::plan_lab(&[stack], STATION, &r, &held), None);
}

// ---- the kit's bill and the builder's own reaction (T3-POLYMER-PRICE) ------------

const COMPONENT: i64 = 990_975_001;
const POLYMER: i64 = 990_975_002;
const GAS: i64 = 990_975_003;
const FUEL: i64 = 990_975_004;
const FORMULA: i64 = 990_975_101;
const COMPONENT_BP: i64 = 990_975_102;
const HULL_COMPONENTS: i64 = 20;
const POLYMER_PER_COMPONENT: i64 = 4;
const MINERAL_PER_HULL: i64 = 100;
const POLYMER_PER_RUN: i64 = 10;
const GAS_PER_RUN: i64 = 100;
const FUEL_PER_RUN: i64 = 5;

/// A hull made of components made of a polymer that only a reaction makes,
/// and a relic that invents the hull.
fn chain() -> industry::Table {
    let prices = eve_server::econ_prices::Table::parse(&[
        format!("{FUEL}\t20.0\tbase\t20.0\t4\t99\t\t1"),
        format!("{GAS}\t3.0\tbase\t3.0\t4\t99\t\t1"),
        format!("{FORMULA}\t5000.0\tbase\t0.0\t9\t1888\t\t1"),
    ])
    .expect("prices");
    let skill = eve_server::reactions::SKILL_REACTIONS;
    let r = vec![
        format!("{RELIC_HULL}\t8\tactivity\t0\t0\t\t3600\t300"),
        format!("{RELIC_HULL}\t8\tmaterial\t{CORE_X}\t{PER_RUN}\t\t3600\t300"),
        format!(
            "{RELIC_HULL}\t8\tproduct\t{}\t{HULL_RUNS}\t0.25\t3600\t300",
            bp(HULL_A)
        ),
        format!("{RELIC_HULL}\t8\tskill\t{SKILL}\t1\t\t3600\t300"),
        format!("{}\t1\tactivity\t0\t0\t\t900\t1", bp(HULL_A)),
        format!(
            "{}\t1\tmaterial\t{COMPONENT}\t{HULL_COMPONENTS}\t\t900\t1",
            bp(HULL_A)
        ),
        format!(
            "{}\t1\tmaterial\t34\t{MINERAL_PER_HULL}\t\t900\t1",
            bp(HULL_A)
        ),
        format!("{}\t1\tproduct\t{HULL_A}\t1\t\t900\t1", bp(HULL_A)),
        format!("{}\t1\tskill\t3380\t1\t\t900\t1", bp(HULL_A)),
        format!("{COMPONENT_BP}\t1\tactivity\t0\t0\t\t900\t1"),
        format!("{COMPONENT_BP}\t1\tmaterial\t{POLYMER}\t{POLYMER_PER_COMPONENT}\t\t900\t1"),
        format!("{COMPONENT_BP}\t1\tproduct\t{COMPONENT}\t1\t\t900\t1"),
        format!("{COMPONENT_BP}\t1\tskill\t3380\t1\t\t900\t1"),
        format!("{FORMULA}\t9\tactivity\t0\t0\t\t3600\t1000"),
        format!("{FORMULA}\t9\tmaterial\t{FUEL}\t{FUEL_PER_RUN}\t\t3600\t1000"),
        format!("{FORMULA}\t9\tmaterial\t{GAS}\t{GAS_PER_RUN}\t\t3600\t1000"),
        format!("{FORMULA}\t9\tproduct\t{POLYMER}\t{POLYMER_PER_RUN}\t\t3600\t1000"),
        format!("{FORMULA}\t9\tskill\t{skill}\t3\t\t3600\t1000"),
    ];
    industry::Table::parse(&r, &prices).expect("chain parses")
}

fn lone_hull() -> Kit {
    Kit {
        hull: HULL_A,
        subs: Vec::new(),
    }
}

#[test]
fn the_bill_goes_down_to_the_polymer_and_nets_what_is_held() {
    let r = chain();
    let kit = lone_hull();
    let bill_all = bill(&kit, &r, &BTreeMap::new());
    assert_eq!(
        bill_all,
        BTreeMap::from([
            (34, MINERAL_PER_HULL),
            (POLYMER, HULL_COMPONENTS * POLYMER_PER_COMPONENT)
        ])
    );
    // Five components in the hangar are five fewer to build; polymer held
    // counts only for what is still to be built.
    let held = BTreeMap::from([(COMPONENT, 5), (POLYMER, 10)]);
    let b = bill(&kit, &r, &held);
    assert_eq!(
        b[&POLYMER],
        (HULL_COMPONENTS - 5) * POLYMER_PER_COMPONENT - 10
    );
}

#[test]
fn the_kit_note_lists_the_missing_goods_most_first() {
    let kits = vec![lone_hull()];
    let r = chain();
    let note = kit_note(&kits, &r, &[], &BTreeMap::new(), 0).expect("a kit");
    let polymer = HULL_COMPONENTS * POLYMER_PER_COMPONENT;
    // The mineral is 100 and the polymer 80: most first.
    assert_eq!(
        note,
        format!("[t3] kit {HULL_A} missing 2 types: 34={MINERAL_PER_HULL} {POLYMER}={polymer}")
    );
    assert_eq!(kit_note(&[], &r, &[], &BTreeMap::new(), 0), None);
}

fn need_polymer() -> BTreeMap<i64, i64> {
    BTreeMap::from([(POLYMER, 30)])
}

fn formula_row() -> BlueprintRow {
    relic_stack(77, FORMULA, -1)
}

#[test]
fn a_builder_buys_the_formula_then_the_inputs_then_reacts() {
    let r = chain();
    let none = BTreeSet::new();
    // No formula: buy it.
    assert_eq!(
        plan_reaction(&need_polymer(), &r, STATION, &[], &BTreeMap::new(), &none),
        Some(Make::Formula(FORMULA))
    );
    // The formula and no inputs: buy three runs' worth (30 units, 10 a run).
    let owned = [formula_row()];
    let Some(Make::Inputs(short)) = plan_reaction(
        &need_polymer(),
        &r,
        STATION,
        &owned,
        &BTreeMap::new(),
        &none,
    ) else {
        panic!("expected inputs");
    };
    assert_eq!(
        short.iter().copied().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(FUEL, 3 * FUEL_PER_RUN), (GAS, 3 * GAS_PER_RUN)])
    );
    // Inputs for two runs: install two.
    let held = BTreeMap::from([(FUEL, 2 * FUEL_PER_RUN), (GAS, 2 * GAS_PER_RUN)]);
    assert_eq!(
        plan_reaction(&need_polymer(), &r, STATION, &owned, &held, &none),
        Some(Make::Install {
            formula: formula_row(),
            runs: 2
        })
    );
}

#[test]
fn a_polymer_someone_sells_is_bought_not_made() {
    let r = chain();
    let sold = BTreeSet::from([POLYMER]);
    assert_eq!(
        plan_reaction(&need_polymer(), &r, STATION, &[], &BTreeMap::new(), &sold),
        None
    );
}

#[test]
fn a_formula_in_a_job_or_another_station_is_not_used_and_a_batch_is_capped() {
    let r = chain();
    let none = BTreeSet::new();
    let mut busy = formula_row();
    busy.job_id = Some(9);
    assert_eq!(
        plan_reaction(
            &need_polymer(),
            &r,
            STATION,
            &[busy],
            &BTreeMap::new(),
            &none
        ),
        Some(Make::Formula(FORMULA))
    );
    let mut far = formula_row();
    far.location_id = STATION + 1;
    assert_eq!(
        plan_reaction(
            &need_polymer(),
            &r,
            STATION,
            &[far],
            &BTreeMap::new(),
            &none
        ),
        Some(Make::Formula(FORMULA))
    );
    let big = BTreeMap::from([(POLYMER, 10_000)]);
    let held = BTreeMap::from([(FUEL, 100 * FUEL_PER_RUN), (GAS, 100 * GAS_PER_RUN)]);
    let Some(Make::Install { runs, .. }) =
        plan_reaction(&big, &r, STATION, &[formula_row()], &held, &none)
    else {
        panic!("expected an install");
    };
    assert_eq!(runs, REACTION_BATCH);
}

// ---- PROG-PVE2: the fight steers the stance -------------------------------------

mod steer {
    use super::*;
    use crate::brain::{Ball, Health, Hostile, Memory, Observation, ParkView, Place, Policy, Role};
    use std::collections::BTreeMap as Map;
    use std::time::{Duration, Instant};

    const SHIP: i64 = 100_000_001;
    const TARGET: i64 = 7_000_000_001;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 0.0,
        }
    }

    fn fighting() -> (Memory, Observation) {
        let mut m = Memory::new(
            "Invented Pilot",
            Policy {
                role: Role::Escort,
                ..Policy::fast()
            },
        );
        m.combat.t3_hull = true;
        m.target = Some((TARGET, "Invented Rat".to_owned()));
        let mut o = Observation::empty(Instant::now());
        o.place = Place::InSpace {
            system: 30_000_001,
            bound: true,
        };
        o.health = Some(Health {
            shield: 0.2,
            armour: 0.4,
            hull: 1.0,
        });
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: Map::new(),
            hostiles: vec![Hostile {
                ball: ball(TARGET, 8_000.0),
                type_id: 1,
                name: "Invented Rat".to_owned(),
                shooting: None,
            }],
            wrecks: Vec::new(),
            ego_tether: None,
        });
        (m, o)
    }

    #[test]
    fn a_tech_three_hull_in_a_fight_asks_for_a_stance_by_its_health_and_the_range() {
        let (mut m, o) = fighting();
        let acts = stance_steer(&mut m, &o, vec![Act::Wait(Duration::from_secs(1))]);
        match &acts[..] {
            [Act::Stance { hurt, range, reach }, Act::Wait(_)] => {
                // Shield 20 %, armour 40 %: 70 % lost.
                assert!((hurt - 0.7).abs() < 1e-9, "{hurt}");
                assert_eq!(*range, Some(8_000.0));
                assert!(*reach > 0.0);
            }
            other => panic!("expected a stance then the old acts, got {other:?}"),
        }
        // The pick is the server's rule: hurt means defence.
        let want = stance_wanted(&content(), DESTROYER, 0.7, Some(8_000.0), 10_000.0);
        assert_eq!(want, Some(stance::STANCE_DEFENCE));
    }

    #[test]
    fn it_asks_once_per_interval_and_not_when_it_is_no_tech_three_hull_or_not_fighting() {
        let (mut m, o) = fighting();
        let first = stance_steer(&mut m, &o, vec![Act::Resync]);
        assert!(matches!(first[0], Act::Stance { .. }));
        // Asked again at once: the acts as they were.
        assert_eq!(
            stance_steer(&mut m, &o, vec![Act::Resync]),
            vec![Act::Resync]
        );
        // After the interval it asks again.
        let mut later = o.clone();
        later.now = o.now + STANCE_EVERY;
        assert!(matches!(
            stance_steer(&mut m, &later, vec![Act::Resync])[0],
            Act::Stance { .. }
        ));
        // A T1 hull never does.
        let (mut t1, o) = fighting();
        t1.combat.t3_hull = false;
        assert_eq!(
            stance_steer(&mut t1, &o, vec![Act::Resync]),
            vec![Act::Resync]
        );
        // Nothing engaged: nothing to steer.
        let (mut idle, o) = fighting();
        idle.target = None;
        assert_eq!(
            stance_steer(&mut idle, &o, vec![Act::Resync]),
            vec![Act::Resync]
        );
        // On the way home: leave it.
        let (mut home, o) = fighting();
        home.homing = Some(crate::brain::Homing::Repair);
        assert_eq!(
            stance_steer(&mut home, &o, vec![Act::Resync]),
            vec![Act::Resync]
        );
    }
}

/// A subsystem row that also carries `launcherHardPointModifier` 1369.
fn launcher_sub_row(id: i64, launchers: i64) -> String {
    format!(
        "type\t{id}\t{SUB_GROUP}\t{CATEGORY_SUBSYSTEM}\t1\t1\t40.0\t0.0\t1200000.0\t1.0\t0\t\t\
         1366=125.0;1380={HULL_A}.0;1369={launchers}.0\tFixture Offensive {id}\n"
    )
}

fn weapon_content() -> Content {
    let mut text = String::from(typedata::FIXTURE);
    text += &launcher_sub_row(990_970_051, 5);
    text += &launcher_sub_row(990_970_052, 0);
    let table = typedata::Table::parse(&text).expect("the invented rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", typedata::Installed::build(table));
    c
}

#[test]
fn a_launcher_only_kit_fires_missiles_whatever_the_seed() {
    use crate::hull_choice::Weapon;
    let c = weapon_content();
    let tengu = HULL_RACES[0].0;
    for seed in [1, 2, 3, 4, 7, 8] {
        assert_eq!(
            kit_weapon(&c, tengu, &[990_970_051], seed),
            Weapon::Missile,
            "seed {seed}"
        );
        // The same hull with a launcher-less offensive subsystem is a
        // turret one: never missiles, for any seed.
        assert_eq!(
            kit_weapon(&c, tengu, &[990_970_052], seed),
            Weapon::Hybrid,
            "seed {seed}"
        );
    }
}

#[test]
fn a_turret_kit_follows_its_hulls_race_and_an_unknown_hull_never_gets_missiles() {
    use crate::hull_choice::Weapon;
    let c = weapon_content();
    let by_race = |race| HULL_RACES.iter().find(|h| h.1 == race).expect("a hull").0;
    let w = |race, seed| kit_weapon(&c, by_race(race), &[990_970_052], seed);
    assert_eq!(w(crate::progression::Race::Amarr, 2), Weapon::Laser);
    assert_eq!(w(crate::progression::Race::Gallente, 2), Weapon::Hybrid);
    assert_eq!(w(crate::progression::Race::Minmatar, 3), Weapon::Projectile);
    for seed in 0..40 {
        assert_ne!(
            kit_weapon(&c, 990_970_999, &[990_970_052], seed),
            Weapon::Missile,
            "seed {seed}"
        );
    }
}
