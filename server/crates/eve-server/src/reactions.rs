//! Reactions at NPC stations (R-MOON packet MN-A): the numbers a reaction job
//! adds to the shared industry engine, and the invented rows its tests run on.
//!
//! # What a reaction is, in this client [V]
//!
//! `industry.REACTION` is activity 9, one of the six activities
//! (`industry/const.pyc`). A reaction job is validated by the same
//! `Job._get_errors` as a manufacturing job, and `Reaction(Activity)` differs
//! from `Manufacturing` in four ways only:
//!
//! - **the blueprint is a formula**, a category-9 item in the groups of
//!   [`REACTION_GROUPS`], and it has the one activity 9 and no manufacturing
//!   activity (`REQUIRES_ORIGINAL`: a formula is an original, never a copy);
//! - **there is no ME and no TE**: materials are `base x runs` and the only
//!   time terms are the character's;
//! - the character's time term is `reactionTimeMultiplier` (attribute 2662),
//!   a PostPercent of skill 45746 Reactions' `reactionTimeBonus` (2660, `-4`
//!   per level), and the slots are the character type's `reactionSlotLimit`
//!   (2664, `1`) plus `reactionSlotBonus` (2661, `+1`) on skills 45748 Mass
//!   Reactions and 45749 Advanced Mass Reactions
//!   (`industryCommon.CHARACTER_MODIFIERS`) **[V, V-data]**;
//! - **the output is an item stack** (`runs x product.quantity`), not a
//!   blueprint, and the price is the Estimated Item Value of the *inputs*
//!   (`industryCommon.GetBlueprintPrice(bp, 9)` sums the activity-9
//!   materials), times runs, through the usual `cost_lines` with the
//!   `refReaction` (135) journal type, which `industry::ACCOUNT_ENTRY_TYPES`
//!   already carries.
//!
//! **No structure is needed**: the facility test is "online, tax is a number,
//! activity in `facility.activities`, and the blueprint in the activity's
//! `blueprints` set only when that set is non-empty". Our NPC-station facility
//! row (`service/industry.rs facility_row`) lists all six activities with
//! empty filters, which the client reads as "any blueprint", so the client has
//! always allowed a reaction at a station and what refused it was this server.
//! The whole reaction tree (all the formulas of the bundle, their products and
//! materials: simple, complex, hybrid polymer, biochemical and
//! molecular-forged) is in the item store; the hybrid polymers (fullerite gas,
//! minerals and fuel blocks, no moon material) are what T3 needs, and give
//! gas mining its first manufacturing consumer.
//!
//! The logic itself is in `crate::industry` (`Table::reactions`,
//! `Jobs::plan_install`'s REACTION arms, `Jobs::plan_delivery`), because it is
//! the same plan functions as every other activity; this module holds the
//! constants, the two small character formulas and the test furniture.
//!
//! # Marks
//!
//! - **[A] 1.** NPC stations host reactions at tax `0` and the normal SCC
//!   fee. Falsified by an owner decision that reactions are structure-only,
//!   or a retail log of a station refusing activity 9.
//! - **[A] 2.** Slots are `1 + L(45748) + L(45749)`, capped at
//!   [`MAX_REACTION_SLOTS`] like manufacturing's. Falsified by the Industry
//!   window's job-count tooltip (`industryTooltips` shows the slot sources).
//! - **[A] 3.** No ME on reactions at a station. Falsified by a facility
//!   `materialModifier` appearing in a retail capture.
//! - **[A] 4.** No implant folds into the reaction time multiplier. The
//!   Zainou/Inherent reaction implants are not items here; falsified by one
//!   appearing in the item store.

/// `REACTION_GROUPS`: Composite 1888, Polymer 1889, Biochemical 1890 and
/// Molecular-Forged 4097 Reaction Formulas **[V]**
/// (`industry/const.pyc`, `IsReactionBlueprint`).
pub const REACTION_GROUPS: [i64; 4] = [1888, 1889, 1890, 4097];

/// Whether this item group is one of the formula groups.
pub fn is_formula_group(group_id: i64) -> bool {
    REACTION_GROUPS.contains(&group_id)
}

/// Skill 45746 Reactions: required (at some level) by every formula, and the
/// one whose `reactionTimeBonus` shortens the job **[V-data]**.
pub const SKILL_REACTIONS: i64 = 45746;
/// Skill 45748 Mass Reactions: `reactionSlotBonus` +1 per level **[V-data]**.
pub const SKILL_MASS_REACTIONS: i64 = 45748;
/// Skill 45749 Advanced Mass Reactions: `reactionSlotBonus` +1 per level
/// **[V-data]**.
pub const SKILL_ADVANCED_MASS_REACTIONS: i64 = 45749;

/// `reactionTimeBonus` (2660) of [`SKILL_REACTIONS`], percent per level
/// **[V-data]**. A PostPercent on `reactionTimeMultiplier`, so the term is
/// `1 + bonus * level / 100`.
pub const REACTION_TIME_BONUS_PER_LEVEL: f64 = -4.0;

/// The character type's own `reactionSlotLimit` (attribute 2664) **[V-data]**.
/// As for manufacturing, the attribute's `defaultValue` is 0 and the
/// character type carries it explicitly at 1.
pub const CHARACTER_BASE_REACTION_SLOTS: i64 = 1;

/// The most reaction jobs one character may run **[A 2]**: the same ceiling
/// as manufacturing's, which `1 + 5 + 5` reaches.
pub const MAX_REACTION_SLOTS: i64 = 11;

/// The character's `reactionTimeMultiplier`: `1.0` times one PostPercent
/// term per level of [`SKILL_REACTIONS`] **[V-data]**, folded here the way
/// `industry::time_multiplier` folds manufacturing's (we have no dogma
/// engine). No implant **[A 4]**.
pub fn time_multiplier<F>(level: F) -> f64
where
    F: Fn(i64) -> i64,
{
    (1.0 + REACTION_TIME_BONUS_PER_LEVEL * level(SKILL_REACTIONS).clamp(0, 5) as f64 / 100.0)
        .max(0.0)
}

/// How many reaction jobs one character may run: the character's
/// `reactionSlotLimit` plus one `SlotModifier` per level of each of the two
/// slot skills **[V-data, A 2]**.
pub fn max_slots<F>(level: F) -> i64
where
    F: Fn(i64) -> i64,
{
    (CHARACTER_BASE_REACTION_SLOTS
        + level(SKILL_MASS_REACTIONS).clamp(0, 5)
        + level(SKILL_ADVANCED_MASS_REACTIONS).clamp(0, 5))
    .min(MAX_REACTION_SLOTS)
}

// ---------------------------------------------------------------------------
// Invented furniture. Every id, quantity, time and price below is made up;
// nothing is a client value (the no-client-data rule).
// ---------------------------------------------------------------------------

/// A simple formula: one fuel block, Tritanium and Pyerite make a goo.
pub const FIXTURE_FORMULA: i64 = 9_906_001;
/// A second-stage formula that eats the first one's product.
pub const FIXTURE_FORMULA_TWO: i64 = 9_906_002;
/// A formula whose second material no item table knows.
pub const FIXTURE_FORMULA_BROKEN: i64 = 9_906_003;
/// What [`FIXTURE_FORMULA`] makes.
pub const FIXTURE_GOO: i64 = 9_906_101;
/// What [`FIXTURE_FORMULA_TWO`] makes.
pub const FIXTURE_ALLOY: i64 = 9_906_102;
/// The fuel block both formulas burn.
pub const FIXTURE_FUEL: i64 = 9_906_201;
/// The material no item table knows.
pub const FIXTURE_UNKNOWN: i64 = 9_906_999;
/// The Tritanium and Pyerite of the item store's hand rows.
pub const FIXTURE_MINERAL_A: i64 = 34;
pub const FIXTURE_MINERAL_B: i64 = 35;

/// The invented `industry/blueprints.tsv` rows: three formulas.
///
/// `FIXTURE_FORMULA`: 3600 s a run, 5 fuel, 100 A and 40 B make 20 goo.
/// `FIXTURE_FORMULA_TWO`: 7200 s a run, 10 goo, 5 fuel make 5 alloy.
pub fn fixture_rows() -> Vec<String> {
    let skill = SKILL_REACTIONS;
    let (f1, f2, f3) = (FIXTURE_FORMULA, FIXTURE_FORMULA_TWO, FIXTURE_FORMULA_BROKEN);
    let (a, b) = (FIXTURE_MINERAL_A, FIXTURE_MINERAL_B);
    vec![
        format!("{f1}\t9\tactivity\t0\t0\t\t3600\t1000"),
        format!("{f1}\t9\tmaterial\t{FIXTURE_FUEL}\t5\t\t3600\t1000"),
        format!("{f1}\t9\tmaterial\t{a}\t100\t\t3600\t1000"),
        format!("{f1}\t9\tmaterial\t{b}\t40\t\t3600\t1000"),
        format!("{f1}\t9\tproduct\t{FIXTURE_GOO}\t20\t\t3600\t1000"),
        format!("{f1}\t9\tskill\t{skill}\t3\t\t3600\t1000"),
        format!("{f2}\t9\tactivity\t0\t0\t\t7200\t1000"),
        format!("{f2}\t9\tmaterial\t{FIXTURE_FUEL}\t5\t\t7200\t1000"),
        format!("{f2}\t9\tmaterial\t{FIXTURE_GOO}\t10\t\t7200\t1000"),
        format!("{f2}\t9\tproduct\t{FIXTURE_ALLOY}\t5\t\t7200\t1000"),
        format!("{f2}\t9\tskill\t{skill}\t3\t\t7200\t1000"),
        format!("{f3}\t9\tactivity\t0\t0\t\t600\t1000"),
        format!("{f3}\t9\tmaterial\t{a}\t10\t\t600\t1000"),
        format!("{f3}\t9\tmaterial\t{FIXTURE_UNKNOWN}\t5\t\t600\t1000"),
        format!("{f3}\t9\tproduct\t{FIXTURE_GOO}\t1\t\t600\t1000"),
        format!("{f3}\t9\tskill\t{skill}\t3\t\t600\t1000"),
    ]
}

/// The invented `econ/prices.tsv` rows for the three new types: the fuel block
/// at 20, the goo at 30, the alloy at 90. (The two minerals are priced by the
/// industry fixture already.)
pub fn fixture_prices() -> Vec<String> {
    vec![
        format!("{FIXTURE_FUEL}\t20.0\tbase\t20.0\t4\t99061\t\t1"),
        format!("{FIXTURE_GOO}\t30.0\tbase\t30.0\t4\t99062\t\t1"),
        format!("{FIXTURE_ALLOY}\t90.0\tbase\t90.0\t4\t99062\t\t1"),
    ]
}

/// `Content::fixture()` with the item types the reaction fixture needs: the
/// formulas as category-9 originals (group 1888), the fuel block, the goo and
/// the alloy. For the tests that install and deliver; the unknown material is
/// left out on purpose.
#[cfg(test)]
pub(crate) fn test_content() -> crate::content::Content {
    let row = |t: i64, g: i64, c: i64, name: &str| {
        format!("type\t{t}\t{g}\t{c}\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\t{name}\n")
    };
    let types = [
        crate::typedata::FIXTURE.to_owned(),
        row(FIXTURE_FORMULA, 1888, 9, "Fixture Formula One"),
        row(FIXTURE_FORMULA_TWO, 1888, 9, "Fixture Formula Two"),
        row(FIXTURE_FORMULA_BROKEN, 1888, 9, "Fixture Formula Broken"),
        row(FIXTURE_FUEL, 1136, 4, "Fixture Fuel Block"),
        row(FIXTURE_GOO, 427, 4, "Fixture Goo"),
        row(FIXTURE_ALLOY, 428, 4, "Fixture Alloy"),
    ]
    .concat();
    let td = crate::typedata::Table::parse(&types).expect("the reaction test rows parse");
    let mut c = crate::content::Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(td));
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::industry::{
        self, Blueprint, BlueprintItem, FILETIME_SECOND, FLAG_HANGAR, Facility, InstallRequest,
        Jobs, MAX_RUN_LENGTH, Pilot, REACTION, Refusal, Stack, Status, Table,
    };
    use crate::item::{self, Slot};
    use crate::skill::Skills;

    const STATION: i64 = 60_012_526;
    const STATION_OWNER: i64 = 1_000_126;
    const SYSTEM: i64 = 30_000_001;
    const ME: i64 = 90_000_001;

    thread_local! {
        // Leaked once per test thread, like `industry`'s own tests: the rows
        // are invented and a lookup hands out `&'static` borrows.
        static ROWS: &'static Table = Box::leak(Box::new(Table::fixture()));
        static CONTENT: &'static crate::content::Content =
            Box::leak(Box::new(test_content()));
    }

    fn table() -> &'static Table {
        ROWS.with(|r| *r)
    }

    fn content() -> &'static crate::content::Content {
        CONTENT.with(|c| *c)
    }

    fn station() -> Facility {
        Facility::npc_station(STATION, STATION_OWNER, 2502, SYSTEM)
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

    fn pilot<'a>(skills: &'a Skills, balance: f64) -> Pilot<'a> {
        Pilot {
            character_id: ME,
            corporation_id: None,
            skills: Some(skills),
            implants: &[],
            balance,
        }
    }

    fn formula(item_id: i64, type_id: i64) -> BlueprintItem {
        BlueprintItem {
            item_id,
            type_id,
            owner_id: ME,
            location_id: STATION,
            location_type_id: 2502,
            flag_id: FLAG_HANGAR,
            runs_remaining: -1,
            material_efficiency: 0,
            time_efficiency: 0,
        }
    }

    fn request(blueprint_id: i64, runs: i64) -> InstallRequest {
        InstallRequest {
            activity_id: REACTION,
            blueprint_id,
            runs,
            input_location_id: STATION,
            input_flag_id: FLAG_HANGAR,
            output_location_id: STATION,
            output_flag_id: FLAG_HANGAR,
            licensed_runs: 0,
            product_type_id: None,
            decryptor: None,
        }
    }

    fn stacks(bp: &Blueprint, runs: i64) -> Vec<Stack> {
        bp.materials
            .iter()
            .enumerate()
            .map(|(i, &(type_id, base))| Stack {
                item_id: 100_000 + i as i64,
                type_id,
                quantity: base * runs,
                slot: Slot::new(ME, STATION, FLAG_HANGAR),
            })
            .collect()
    }

    fn one() -> &'static Blueprint {
        table()
            .find_reaction(FIXTURE_FORMULA)
            .expect("the fixture formula")
    }

    fn trained() -> Skills {
        levels(&[(SKILL_REACTIONS, 3)])
    }

    // ---- the static layer --------------------------------------------

    #[test]
    fn a_formula_row_is_parsed_with_activity_9_only() {
        let t = table();
        let bp = one();
        assert_eq!(bp.product_type_id, FIXTURE_GOO);
        assert_eq!(bp.product_quantity, 20);
        assert_eq!(bp.time, 3600);
        assert_eq!(bp.skills, vec![(SKILL_REACTIONS, 3)]);
        let mut want = vec![
            (FIXTURE_MINERAL_A, 100),
            (FIXTURE_MINERAL_B, 40),
            (FIXTURE_FUEL, 5),
        ];
        want.sort_unstable();
        assert_eq!(bp.materials, want);
        // A formula is not a manufacturing blueprint: it is not in the
        // manufacturing list and has no science row.
        assert!(t.find(FIXTURE_FORMULA).is_none());
        assert!(t.science(FIXTURE_FORMULA).is_none());
        assert!(t.is_blueprint_type(FIXTURE_FORMULA));
        assert!(t.is_formula(FIXTURE_FORMULA));
        assert!(!t.is_formula(691));
    }

    #[test]
    fn the_reaction_list_is_sorted_so_the_lookup_is_valid() {
        assert!(
            table()
                .reactions
                .windows(2)
                .all(|w| w[0].blueprint_type_id < w[1].blueprint_type_id)
        );
        // Parsed out of order, the same.
        let mut rows = fixture_rows();
        rows.reverse();
        let mut prices = fixture_prices();
        prices.push("34\t4.0\tbase\t1.0\t4\t18\t\t1".to_owned());
        let p = crate::econ_prices::Table::parse(&prices).expect("prices");
        let t = Table::parse(&rows, &p).expect("parses");
        assert_eq!(t.reactions.len(), 3);
        assert!(t.find_reaction(FIXTURE_FORMULA_TWO).is_some());
    }

    #[test]
    fn a_formula_with_two_products_or_none_is_left_out() {
        let none = "9906500\t9\tactivity\t0\t0\t\t60\t10".to_owned();
        let two = [
            "9906501\t9\tproduct\t9906101\t1\t\t60\t10",
            "9906501\t9\tproduct\t9906102\t1\t\t60\t10",
        ];
        let mut rows = vec![none];
        rows.extend(two.iter().map(|s| (*s).to_owned()));
        let t = Table::parse(&rows, &crate::econ_prices::Table::default()).expect("parses");
        assert!(t.reactions.is_empty());
    }

    #[test]
    fn a_manufacturing_blueprint_is_not_a_formula_and_refuses_reactions() {
        let t = table();
        let skills = levels(&[(industry::SKILL_INDUSTRY, 1), (SKILL_REACTIONS, 3)]);
        let jobs = Jobs::new();
        let bp = t.find(691).expect("fixture hull");
        let mut req = request(500, 1);
        req.activity_id = REACTION;
        let r = jobs.plan_install(
            t,
            content(),
            1,
            &pilot(&skills, 1e9),
            &station(),
            &formula(500, 691),
            &req,
            &stacks(bp, 1),
        );
        assert_eq!(r.unwrap_err(), Refusal::IncompatibleActivity(REACTION));
    }

    #[test]
    fn a_formula_refuses_manufacturing() {
        let skills = trained();
        let jobs = Jobs::new();
        let mut req = request(500, 1);
        req.activity_id = industry::MANUFACTURING;
        let r = jobs.plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 1e9),
            &station(),
            &formula(500, FIXTURE_FORMULA),
            &req,
            &stacks(one(), 1),
        );
        assert_eq!(
            r.unwrap_err(),
            Refusal::IncompatibleActivity(industry::MANUFACTURING)
        );
    }

    #[test]
    fn reaction_needs_an_original() {
        let skills = trained();
        let jobs = Jobs::new();
        let mut copy = formula(500, FIXTURE_FORMULA);
        copy.runs_remaining = 10;
        let r = jobs.plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 1e9),
            &station(),
            &copy,
            &request(500, 1),
            &stacks(one(), 1),
        );
        assert_eq!(r.unwrap_err(), Refusal::IncompatibleActivity(REACTION));
    }

    // ---- the maths ---------------------------------------------------

    fn install(
        runs: i64,
        skills: &Skills,
        on_hand: &[Stack],
    ) -> Result<industry::Install, Refusal> {
        Jobs::new().plan_install(
            table(),
            content(),
            7_000 * FILETIME_SECOND,
            &pilot(skills, 1e9),
            &station(),
            &formula(500, FIXTURE_FORMULA),
            &request(500, runs),
            on_hand,
        )
    }

    #[test]
    fn reaction_materials_are_base_times_runs_without_me() {
        let skills = trained();
        // ME on the item is ignored: reactions have none.
        let mut with_me = formula(500, FIXTURE_FORMULA);
        with_me.material_efficiency = 10;
        for runs in [1, 7] {
            let ok = Jobs::new()
                .plan_install(
                    table(),
                    content(),
                    1,
                    &pilot(&skills, 1e9),
                    &station(),
                    &with_me,
                    &request(500, runs),
                    &stacks(one(), runs),
                )
                .expect("installs");
            for m in &ok.materials {
                let base = one()
                    .materials
                    .iter()
                    .find(|(t, _)| *t == m.type_id)
                    .map(|&(_, q)| q)
                    .expect("a formula material");
                assert_eq!(m.quantity, base * runs, "type {}", m.type_id);
            }
            assert_eq!(ok.materials.len(), one().materials.len());
        }
    }

    #[test]
    fn fuel_blocks_are_consumed() {
        let skills = trained();
        let held = stacks(one(), 3);
        let ok = install(3, &skills, &held).expect("installs");
        let fuel_stack = held
            .iter()
            .find(|s| s.type_id == FIXTURE_FUEL)
            .expect("fuel is on hand");
        let want = 5 * 3;
        assert!(ok.edits.iter().any(|e| matches!(
            e,
            item::Edit::Consume { item_id, quantity }
                if *item_id == fuel_stack.item_id && *quantity == want
        )));
        // The formula itself is not consumed: it stays where it is.
        assert!(
            !ok.edits
                .iter()
                .any(|e| matches!(e, item::Edit::Consume { item_id, .. } if *item_id == 500))
        );
        assert_eq!(ok.blueprint_after, None);
    }

    #[test]
    fn a_missing_fuel_block_is_a_missing_material() {
        let skills = trained();
        let mut held = stacks(one(), 2);
        held.retain(|s| s.type_id != FIXTURE_FUEL);
        let r = install(2, &skills, &held);
        assert_eq!(
            r.unwrap_err(),
            Refusal::MissingMaterial(FIXTURE_FUEL, 10, 0)
        );
    }

    #[test]
    fn reaction_cost_is_eiv_times_runs() {
        let t = table();
        let skills = trained();
        // EIV: 5 fuel at 20, 100 A at 4, 40 B at 10.
        let eiv = 5.0 * 20.0 + 100.0 * 4.0 + 40.0 * 10.0;
        assert_eq!(industry::estimated_item_value(t, one()), eiv);
        for runs in [1, 4] {
            let ok = install(runs, &skills, &stacks(one(), runs)).expect("installs");
            let raw = eiv * runs as f64;
            let fac = station();
            let want = industry::cost_lines(raw, REACTION, &fac, fac.cost_multiplier);
            assert_eq!(ok.cost, want);
            assert_eq!(ok.job.cost, want.total());
            assert_eq!(want.base, raw.round() as i64);
            assert!(want.scc_surcharge > 0, "the SCC surcharge applies");
        }
        // The journal type is the reaction one.
        assert_eq!(industry::Cost::ref_type(REACTION), Some(135));
    }

    #[test]
    fn a_short_wallet_refuses_with_account_funds() {
        let skills = trained();
        let held = stacks(one(), 1);
        let r = Jobs::new().plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 10.0),
            &station(),
            &formula(500, FIXTURE_FORMULA),
            &request(500, 1),
            &held,
        );
        assert!(matches!(r.unwrap_err(), Refusal::AccountFunds(..)));
    }

    #[test]
    fn reaction_time_uses_the_reaction_multiplier_and_the_run_cap() {
        let bp = one();
        // Reactions 3 is the least that installs: -12 %.
        let l3 = trained();
        let m3 = time_multiplier(|s| if s == SKILL_REACTIONS { 3 } else { 0 });
        assert_eq!(m3, 1.0 + REACTION_TIME_BONUS_PER_LEVEL * 3.0 / 100.0);
        let ok = install(2, &l3, &stacks(bp, 2)).expect("installs");
        assert_eq!(
            ok.job.time_in_seconds,
            (bp.time as f64 * 2.0 * m3).round() as i64
        );
        assert_eq!(
            ok.job.end_date - ok.job.start_date,
            ok.job.time_in_seconds * FILETIME_SECOND
        );
        // Reactions 5 is faster.
        let l5 = levels(&[(SKILL_REACTIONS, 5)]);
        let m5 = time_multiplier(|s| if s == SKILL_REACTIONS { 5 } else { 0 });
        assert!(m5 < m3);
        let fast = install(2, &l5, &stacks(bp, 2)).expect("installs");
        assert_eq!(
            fast.job.time_in_seconds,
            (bp.time as f64 * 2.0 * m5).round() as i64
        );
        // The cap is `ceil(MAX_RUN_LENGTH / time / mods)` and one more is refused.
        let cap = (MAX_RUN_LENGTH as f64 / bp.time as f64 / m5).ceil() as i64;
        let at = install(cap, &l5, &stacks(bp, cap)).expect("the cap installs");
        assert_eq!(at.job.runs, cap);
        assert_eq!(
            install(cap + 1, &l5, &stacks(bp, cap + 1)).unwrap_err(),
            Refusal::InvalidRuns(cap)
        );
        // A manufacturing multiplier does not leak in.
        let industry_five = levels(&[(SKILL_REACTIONS, 3), (industry::SKILL_INDUSTRY, 5)]);
        let same = install(2, &industry_five, &stacks(bp, 2)).expect("installs");
        assert_eq!(same.job.time_in_seconds, ok.job.time_in_seconds);
    }

    #[test]
    fn reaction_time_is_paced_by_the_game_clock() {
        let skills = trained();
        let real = install(2, &skills, &stacks(one(), 2))
            .expect("installs")
            .paced(crate::game_clock::GameClock::REAL);
        let n: u32 = 6;
        let fast = install(2, &skills, &stacks(one(), 2))
            .expect("installs")
            .paced(crate::game_clock::GameClock::new(n).expect("a scale"));
        let len = |j: &industry::Job| j.end_date - j.start_date;
        assert_eq!(len(&fast.job) * i64::from(n), len(&real.job));
        assert_eq!(fast.job.time_in_seconds, real.job.time_in_seconds);
    }

    #[test]
    fn reaction_slots_follow_mass_reactions() {
        let lvl = |m: i64, a: i64| {
            move |s: i64| match s {
                SKILL_MASS_REACTIONS => m,
                SKILL_ADVANCED_MASS_REACTIONS => a,
                _ => 0,
            }
        };
        assert_eq!(max_slots(lvl(0, 0)), CHARACTER_BASE_REACTION_SLOTS);
        assert_eq!(max_slots(lvl(2, 0)), CHARACTER_BASE_REACTION_SLOTS + 2);
        assert_eq!(max_slots(lvl(2, 3)), CHARACTER_BASE_REACTION_SLOTS + 5);
        assert_eq!(max_slots(lvl(5, 5)), MAX_REACTION_SLOTS);
        assert_eq!(max_slots(lvl(9, 9)), MAX_REACTION_SLOTS, "levels clamp");

        // Through the plan: one slot, a second job is refused; with Mass
        // Reactions 1 the second installs.
        let mut jobs = Jobs::new();
        let one_slot = trained();
        let held = stacks(one(), 2);
        let first = jobs
            .plan_install(
                table(),
                content(),
                1,
                &pilot(&one_slot, 1e9),
                &station(),
                &formula(500, FIXTURE_FORMULA),
                &request(500, 1),
                &held,
            )
            .expect("the first installs");
        jobs.commit_install(&first);
        let second_try = |skills: &Skills, jobs: &Jobs| {
            jobs.plan_install(
                table(),
                content(),
                1,
                &pilot(skills, 1e9),
                &station(),
                &formula(501, FIXTURE_FORMULA),
                &request(501, 1),
                &held,
            )
        };
        assert_eq!(
            second_try(&one_slot, &jobs).unwrap_err(),
            Refusal::SlotsFull(1, 1)
        );
        let two = levels(&[(SKILL_REACTIONS, 3), (SKILL_MASS_REACTIONS, 1)]);
        assert!(second_try(&two, &jobs).is_ok());
        // Manufacturing's slot skill does not count for reactions, and a
        // running reaction does not take a manufacturing slot.
        let mfg = levels(&[(SKILL_REACTIONS, 3), (industry::SKILL_MASS_PRODUCTION, 4)]);
        assert_eq!(
            second_try(&mfg, &jobs).unwrap_err(),
            Refusal::SlotsFull(1, 1)
        );
        assert_eq!(jobs.used_slots(ME, industry::MANUFACTURING), 0);
        assert_eq!(jobs.used_slots(ME, REACTION), 1);
    }

    #[test]
    fn reaction_missing_skill_is_refused() {
        for have in [0, 2] {
            let skills = levels(&[(SKILL_REACTIONS, have)]);
            let r = install(1, &skills, &stacks(one(), 1));
            assert_eq!(
                r.unwrap_err(),
                Refusal::MissingSkill(SKILL_REACTIONS, 3, have)
            );
        }
        // Not trained at all.
        let nothing = levels(&[]);
        assert_eq!(
            install(1, &nothing, &stacks(one(), 1)).unwrap_err(),
            Refusal::MissingSkill(SKILL_REACTIONS, 3, 0)
        );
    }

    #[test]
    fn reaction_in_another_station_is_refused() {
        let skills = trained();
        let jobs = Jobs::new();
        let mut elsewhere = formula(500, FIXTURE_FORMULA);
        elsewhere.location_id = STATION + 1;
        let r = jobs.plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 1e9),
            &station(),
            &elsewhere,
            &request(500, 1),
            &stacks(one(), 1),
        );
        assert_eq!(r.unwrap_err(), Refusal::BlueprintAccess);
        // A remote facility needs Remote Reactions, not Supply Chain Management.
        let mut far = station();
        far.distance = 1;
        let no_remote = Jobs::new().plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 1e9),
            &far,
            &formula(500, FIXTURE_FORMULA),
            &request(500, 1),
            &stacks(one(), 1),
        );
        assert_eq!(no_remote.unwrap_err(), Refusal::FacilityDistance(1, 0));
        let remote = levels(&[
            (SKILL_REACTIONS, 3),
            (industry::SKILL_REMOTE_REACTIONS, 1),
            (industry::SKILL_SUPPLY_CHAIN_MANAGEMENT, 0),
        ]);
        let ok = Jobs::new().plan_install(
            table(),
            content(),
            1,
            &pilot(&remote, 1e9),
            &far,
            &formula(500, FIXTURE_FORMULA),
            &request(500, 1),
            &stacks(one(), 1),
        );
        assert!(ok.is_ok(), "{ok:?}");
        let scm = levels(&[
            (SKILL_REACTIONS, 3),
            (industry::SKILL_SUPPLY_CHAIN_MANAGEMENT, 5),
        ]);
        let wrong_skill = Jobs::new().plan_install(
            table(),
            content(),
            1,
            &pilot(&scm, 1e9),
            &far,
            &formula(500, FIXTURE_FORMULA),
            &request(500, 1),
            &stacks(one(), 1),
        );
        assert_eq!(wrong_skill.unwrap_err(), Refusal::FacilityDistance(1, 0));
    }

    #[test]
    fn a_formula_with_an_unknown_material_is_refused_by_name() {
        let skills = trained();
        let bp = table().find_reaction(FIXTURE_FORMULA_BROKEN).expect("row");
        let r = Jobs::new().plan_install(
            table(),
            content(),
            1,
            &pilot(&skills, 1e9),
            &station(),
            &formula(500, FIXTURE_FORMULA_BROKEN),
            &request(500, 1),
            &stacks(bp, 1),
        );
        assert_eq!(r.unwrap_err(), Refusal::UnknownType(FIXTURE_UNKNOWN));
    }

    #[test]
    fn a_running_job_blocks_a_second_install_from_the_same_formula() {
        let skills = levels(&[(SKILL_REACTIONS, 3), (SKILL_MASS_REACTIONS, 2)]);
        let mut jobs = Jobs::new();
        let held = stacks(one(), 2);
        let plan = |jobs: &Jobs| {
            jobs.plan_install(
                table(),
                content(),
                1,
                &pilot(&skills, 1e9),
                &station(),
                &formula(500, FIXTURE_FORMULA),
                &request(500, 1),
                &held,
            )
        };
        let first = plan(&jobs).expect("installs");
        let id = jobs.commit_install(&first);
        assert_eq!(plan(&jobs).unwrap_err(), Refusal::BlueprintInstalled(id));
    }

    // ---- delivery ----------------------------------------------------

    #[test]
    fn reaction_delivery_creates_runs_times_quantity() {
        let skills = trained();
        for runs in [1, 5] {
            let mut jobs = Jobs::new();
            let install = install(runs, &skills, &stacks(one(), runs)).expect("installs");
            let end = install.job.end_date;
            let id = jobs.commit_install(&install);
            // Not before the clock runs out.
            assert!(jobs.plan_delivery(table(), content(), id, end - 1).is_err());
            let (delivery, done) = jobs
                .plan_delivery(table(), content(), id, end + 1)
                .expect("delivers");
            assert_eq!(
                delivery.product,
                industry::Material {
                    type_id: FIXTURE_GOO,
                    quantity: 20 * runs
                }
            );
            assert_eq!(done.status, Status::Delivered);
            let [
                item::Edit::Create {
                    type_id,
                    quantity,
                    to,
                },
            ] = &delivery.edits[..]
            else {
                panic!("one create: {:?}", delivery.edits);
            };
            assert_eq!((*type_id, *quantity), (FIXTURE_GOO, 20 * runs));
            assert_eq!(*to, Slot::new(ME, STATION, FLAG_HANGAR));
            // No blueprint changes hands.
            assert_eq!(delivery.blueprint_after, None);
            assert_eq!(delivery.creates, None);
        }
    }

    #[test]
    fn a_delivered_reaction_feeds_the_next_formula() {
        // The chain: goo from the first formula
        // is the second formula's input.
        let skills = trained();
        let first = install(2, &skills, &stacks(one(), 2)).expect("installs");
        let mut jobs = Jobs::new();
        let id = jobs.commit_install(&first);
        let (delivery, _) = jobs
            .plan_delivery(table(), content(), id, first.job.end_date + 1)
            .expect("delivers");
        let goo = delivery.product.quantity;
        assert_eq!(goo, 40);
        let two = table().find_reaction(FIXTURE_FORMULA_TWO).expect("row");
        let runs = goo / 10;
        let mut held = stacks(two, runs);
        // Replace the goo stack with the delivered amount.
        held.iter_mut()
            .filter(|s| s.type_id == FIXTURE_GOO)
            .for_each(|s| s.quantity = goo);
        let second = Jobs::new()
            .plan_install(
                table(),
                content(),
                1,
                &pilot(&skills, 1e9),
                &station(),
                &formula(600, FIXTURE_FORMULA_TWO),
                &request(600, runs),
                &held,
            )
            .expect("the second stage installs");
        assert_eq!(second.job.product_type_id, FIXTURE_ALLOY);
        assert_eq!(second.job.time_in_seconds, {
            let m = time_multiplier(|s| if s == SKILL_REACTIONS { 3 } else { 0 });
            (7200.0 * runs as f64 * m).round() as i64
        });
    }

    #[test]
    fn the_formula_groups_are_the_four_the_client_names() {
        assert!(is_formula_group(1888) && is_formula_group(1889));
        assert!(is_formula_group(1890) && is_formula_group(4097));
        assert!(!is_formula_group(1136));
    }

    #[test]
    fn the_fixture_formulas_are_items_and_a_hull_blueprint_row_is_untouched() {
        let c = content();
        for id in [FIXTURE_FORMULA, FIXTURE_FORMULA_TWO] {
            let row = item::find_in(c, id).expect("the formula is an item");
            assert_eq!(row.category_id, 9);
            assert!(is_formula_group(row.group_id));
        }
        assert!(industry::missing_type(c, one()).is_none());
        let t = table();
        let rifter = t.find(691).expect("fixture hull");
        assert_eq!(rifter.product_type_id, 587);
    }

    /// The one test that reads a real bundle: set `EVE_TEST_BUNDLE` to an
    /// extracted `out/content` and every formula in it is checked as a whole,
    /// with **no id named here** -- what is asserted is the shape (a formula
    /// has the one activity, all its types are items, it is priced, the four
    /// formula groups are all present, and it installs and delivers with the
    /// inputs on hand). Skipped, passing, without the variable, like the
    /// other bundle-driven checks.
    #[test]
    fn every_bundle_formula_installs_and_delivers() {
        let Some(dir) = std::env::var_os("EVE_TEST_BUNDLE") else {
            return;
        };
        let dir = std::path::Path::new(&dir);
        let loaded = crate::content::load(Some(dir), &crate::authored::Dir::compiled_only())
            .expect("the bundle loads");
        let content = &loaded.content;
        let table: &Table = content.get();
        assert!(!table.reactions.is_empty(), "the bundle has formulas");
        let mut groups = std::collections::BTreeSet::new();
        let skills = levels(&[
            (SKILL_REACTIONS, 5),
            (SKILL_MASS_REACTIONS, 5),
            (crate::industry::SKILL_REMOTE_REACTIONS, 5),
        ]);
        for bp in &table.reactions {
            let id = bp.blueprint_type_id;
            let item =
                item::find_in(content, id).unwrap_or_else(|| panic!("formula {id} is not an item"));
            assert_eq!(item.category_id, 9, "formula {id}");
            assert!(is_formula_group(item.group_id), "formula {id}");
            groups.insert(item.group_id);
            assert!(!bp.materials.is_empty(), "formula {id} has inputs");
            assert!(bp.product_quantity > 0 && bp.time > 0, "formula {id}");
            assert_eq!(
                industry::missing_type(content, bp),
                None,
                "formula {id}: a type the item store lacks"
            );
            assert!(
                bp.skills.iter().any(|&(s, _)| s == SKILL_REACTIONS),
                "formula {id} needs Reactions"
            );
            assert!(
                industry::estimated_item_value(table, bp) > 0.0,
                "formula {id} is unpriced, so its fee would be zero"
            );
            // Install one run with the inputs on hand and deliver it.
            let on_hand: Vec<Stack> = bp
                .materials
                .iter()
                .enumerate()
                .map(|(i, &(type_id, q))| Stack {
                    item_id: 200_000 + i as i64,
                    type_id,
                    quantity: q,
                    slot: Slot::new(ME, STATION, FLAG_HANGAR),
                })
                .collect();
            let mut jobs = Jobs::new();
            let ok = jobs
                .plan_install(
                    table,
                    content,
                    1,
                    &pilot(&skills, 1e15),
                    &station(),
                    &formula(500, id),
                    &request(500, 1),
                    &on_hand,
                )
                .unwrap_or_else(|e| panic!("formula {id} does not install: {e:?}"));
            let end = ok.job.end_date;
            let job_id = jobs.commit_install(&ok);
            let (d, _) = jobs
                .plan_delivery(table, content, job_id, end + 1)
                .unwrap_or_else(|e| panic!("formula {id} does not deliver: {e:?}"));
            assert_eq!(d.product.quantity, bp.product_quantity, "formula {id}");
        }
        for g in REACTION_GROUPS {
            assert!(groups.contains(&g), "no formula of group {g} in the bundle");
        }
        // Every skill the formulas need can be trained.
        for bp in &table.reactions {
            for &(skill, _) in &bp.skills {
                assert!(
                    crate::skill::find_in(content, skill).is_some(),
                    "formula {} needs skill {skill}, which nobody can train",
                    bp.blueprint_type_id
                );
            }
        }
        for s in [
            SKILL_REACTIONS,
            SKILL_MASS_REACTIONS,
            SKILL_ADVANCED_MASS_REACTIONS,
            crate::industry::SKILL_REMOTE_REACTIONS,
        ] {
            assert!(
                crate::skill::find_in(content, s).is_some(),
                "reaction skill {s} is not in the skill table"
            );
        }
    }
}
