//! **Skills and hull bonuses on squadrons** (FT-H, `docs/lanes/ft-gh.md`;
//! spec R-DEEP §2.3, §2.9).
//!
//! # Where a squadron's numbers come from [V-data] / [V]
//!
//! Every bonus a pilot or a hull gives a squadron is an ordinary dogma
//! modifier, the shape drones have (`crate::drone_skills`): "everything the
//! character owns that requires skill S" (`OwnerRequiredSkillModifier`,
//! `crate::dogma` rule 2). In the client's data: `skillBonusFighters`
//! (damage of every type that requires Fighters), the light, support and
//! heavy class skills, the racial specialisations, the fighter-generic role
//! bonuses, and the carrier and supercarrier hull bonuses; none of the
//! thirteen *ability* effects has a modifier, so the bonus lands on the
//! **type's own attributes** (damage multiplier, speed, shield capacity,
//! ability durations) and the abilities read those. The client's own dogma
//! does the same for a fighter in space: `FighterDogmaItem` re-applies the
//! owner's required-skill modifiers to it
//! (`AddFighterControllerOwnerModifiers`, `_IterOwnerReqSkillModifiers`),
//! and its `IsOwnerModifiable` wants an owner and a pilot ship [V
//! `dogma/items/fighterDogmaItem.py`].
//!
//! [`numbers`] asks [`crate::dogma`] for the squadron's type as a **charge
//! in a phantom module** of the pilot's real ship, exactly as
//! `drone_skills::stats` does for a drone: the real skills, the real hull,
//! the real fitted modules and implants, stacking penalties and all. The
//! answer is **fixed at launch** (as a drone's is) and kept in the
//! squadron's [`super::ability::Runtime`]; [`attr_of`] reads it, falling
//! back to the type. Nothing about the **size** of a squadron is a dogma
//! modifier: damage is per fighter times the current size ([`super::ability`]).
//!
//! **Fighter Hangar Management** (`skillBonusFighterHangarManagement`, a
//! `shipID` item modifier on `fighterCapacity` 2055 from the skill's 2340
//! [V-data]) and any other modifier on the hull's tubes and class slots reach
//! the carrier's **sheet**, and [`ship_slots`] reads those: the bay's
//! capacity, the tube count and the class slots as fitted, not as the type
//! states.
//!
//! # The gate
//!
//! A fighter type names its skills in `requiredSkill1..6` and their levels
//! (attributes 182-184, 1285-1287 and 277-279, 1286-1288; the class skill
//! (Light, Support or Heavy Fighters) and Fighters are among them [V-data]).
//! The client has the message and no raise site: `FighterHasSkillPrerequisites`
//! with `{skillCount, item, requiredSkills}` [V text: "You do not have the
//! required skills to command this {item.name}. You require the following
//! skills: {requiredSkills}"]. **[A]** it is raised at **launch**: a pilot
//! can hold a fighter in a bay or a tube without the skills, and cannot put
//! it in space. Falsifier: the stock server refuses at the load, or lets an
//! unskilled pilot launch.

use super::*;
use crate::dogma::{self, Inputs, ModuleInput};
use crate::fighters_data::HullSlots;

/// A squadron's modified type attributes, **only those that differ** from the
/// type's own.
pub type Numbers = BTreeMap<i64, f64>;

/// The phantom module's item id: no real item has it (`drone_skills`).
const PHANTOM: i64 = i64::MAX;

/// A squadron's attribute: the pilot's and the hull's modified value for a
/// squadron that has them, the type's otherwise.
pub fn attr_of(w: &Registry, id: i64, type_id: i64, attribute: i64) -> f64 {
    state(w)
        .rt
        .get(&id)
        .and_then(|r| r.numbers.get(&attribute))
        .copied()
        .unwrap_or_else(|| {
            typedata::installed_in(w.content())
                .table()
                .attribute(type_id, attribute)
        })
}

/// **What `type_id` is for the pilot of `ship_id`**: every attribute of the
/// type the engine moves. Empty for a ship the table does not hold or a
/// pilot with nothing that applies.
pub fn numbers(w: &Registry, ship_id: i64, type_id: i64) -> Numbers {
    let table = typedata::installed_in(w.content()).table();
    let (Some(mut inputs), Some(row)) = (dogma::inputs(w, ship_id), table.get(type_id)) else {
        return Numbers::new();
    };
    if table
        .get(inputs.ship_type)
        .is_none_or(|r| r.category_id != typedata::CATEGORY_SHIP)
    {
        return Numbers::new();
    }
    phantom(&mut inputs, type_id);
    let Some(sheet) = dogma::evaluate(table, &inputs) else {
        return Numbers::new();
    };
    row.attributes
        .iter()
        .filter_map(|&(attribute, own)| {
            let v = sheet.charge(PHANTOM, attribute)?;
            ((v - own).abs() > 1e-9).then_some((attribute, v))
        })
        .collect()
}

/// The fighter as a charge in a phantom, offline, type-less module: the
/// module has no row so none of its effects start; the charge is the
/// fighter's own type, so its passive effects do (`drone_skills::phantom`).
fn phantom(inputs: &mut Inputs, type_id: i64) {
    inputs.modules.push(ModuleInput {
        item_id: PHANTOM,
        type_id: 0,
        state: dogma::State::Offline,
        charge_type: Some(type_id),
    });
}

/// A hull's tubes, class slots and bay **as fitted**: the carrier's sheet
/// (Fighter Hangar Management on 2055, and anything else that modifies the
/// hull's fighter attributes), the type's own numbers for a hull with no
/// sheet.
pub fn ship_slots(w: &Registry, ship_id: i64) -> HullSlots {
    let Some(ship) = w.item(ship_id) else {
        return HullSlots::default();
    };
    match dogma::sheet(w, ship_id) {
        Some(sheet) => HullSlots::from_attrs(|a| Some(sheet.ship(a))),
        None => hull_slots(w.content(), ship.type_id),
    }
}

/// Every `(skill, level)` a pilot lacks to command `type_id`, in the order
/// the type names them. Empty for a type that requires nothing.
pub fn missing(w: &Registry, owner: i64, type_id: i64) -> Vec<(i64, i64)> {
    let table = typedata::installed_in(w.content()).table();
    crate::drone_skills::required_skills(table, type_id)
        .into_iter()
        .filter(|(skill, level)| crate::drone_skills::level_of(w, owner, *skill) < *level)
        .collect()
}

/// **The launch gate**: `FighterHasSkillPrerequisites` for a pilot who lacks
/// a skill the fighter's type requires.
pub fn gate(w: &Registry, owner: i64, type_id: i64) -> Result<(), Refusal> {
    let lacking = missing(w, owner, type_id);
    if lacking.is_empty() {
        return Ok(());
    }
    // `[A]` the `requiredSkills` text is "Name level N, ...": the client only
    // substitutes it into the sentence (`implants.rs` does the same).
    let text: Vec<String> = lacking
        .iter()
        .map(|(skill, level)| {
            let name = crate::skill::find_in(w.content(), *skill)
                .map(|t| t.name.to_owned())
                .or_else(|| {
                    typedata::installed_in(w.content())
                        .table()
                        .get(*skill)
                        .map(|r| r.name.clone())
                })
                .unwrap_or_else(|| skill.to_string());
            format!("{name} level {level}")
        })
        .collect();
    Err(Refusal::with(
        "FighterHasSkillPrerequisites",
        vec![
            ("skillCount", PyValue::Int(lacking.len() as i64)),
            ("item", PyValue::Int(type_id)),
            ("requiredSkills", PyValue::Unicode(text.join(", "))),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fighters::ability::tests as at;
    use crate::fighters::fixture::*;
    use crate::fighters::tests as ft;
    use crate::fighters::tests::{ALICE, ALICE_SHIP, BAY, CARRIER, SUPPORT};

    /// Alice in `hull`, trained as given, undocked with `bay` loaded and
    /// launched: the squadrons' ids.
    fn flying(
        trained: &[(i64, i64)],
        hull: i64,
        bay: &[(i64, i64)],
    ) -> (Registry, Vec<i64>, BlueTime) {
        let mut reg = ft::world(hull, bay);
        train(&mut reg, ALICE, trained);
        at::launch_all(reg, bay)
    }

    fn own(reg: &Registry, type_id: i64, attribute: i64) -> f64 {
        typedata::installed_in(reg.content())
            .table()
            .attribute(type_id, attribute)
    }

    const SKILLED_OK: [(i64, i64); 2] = [(SK_LIGHT, 1), (SK_FIGHTERS, 2)];

    #[test]
    fn fighters_skill_raises_squadron_damage_per_level() {
        let mut last = 0.0;
        for level in 2..=5 {
            let (reg, ids, _) = flying(
                &[(SK_LIGHT, 1), (SK_FIGHTERS, level)],
                CARRIER,
                &[(SKILLED, SQUADRON)],
            );
            let got = attr_of(&reg, ids[0], SKILLED, 2226);
            let want = own(&reg, SKILLED, 2226) * (1.0 + FIGHTERS_PCT * level as f64 / 100.0);
            assert!((got - want).abs() < 1e-9, "level {level}: {got} vs {want}");
            assert!(got > last, "more per level");
            last = got;
        }
    }

    /// **The skilled pilot's squadron hits harder**, through the real volley:
    /// the same squadron of the same type against the same rat, two levels
    /// of the skill apart.
    #[test]
    fn a_skilled_squadron_deals_more_damage_than_an_unskilled_ones() {
        let lost = |level: i64| {
            let (mut reg, ids, _) = flying(
                &[(SK_LIGHT, 1), (SK_FIGHTERS, level)],
                CARRIER,
                &[(SKILLED, SQUADRON)],
            );
            let rat = at::rat_beyond(&mut reg, ids[0], 4_000.0);
            let _ = ft::pending(&mut reg);
            at::activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
            let now = crate::pyutil::filetime_now() + ms(100);
            tick(&mut reg, now);
            at::shield_lost(&reg, rat, now)
        };
        let (low, high) = (lost(2), lost(5));
        let ratio = (1.0 + FIGHTERS_PCT * 5.0 / 100.0) / (1.0 + FIGHTERS_PCT * 2.0 / 100.0);
        assert!(high > low, "{high} vs {low}");
        assert!(
            (high / low - ratio).abs() < 0.05,
            "the damage ratio {} against the bonus ratio {ratio}",
            high / low
        );
    }

    #[test]
    fn the_carriers_hull_bonus_stacks_with_the_skills() {
        let level = 3;
        let (reg, ids, _) = flying(
            &[(SK_LIGHT, 1), (SK_FIGHTERS, level)],
            SK_CARRIER,
            &[(SKILLED, SQUADRON)],
        );
        let want = own(&reg, SKILLED, 2226)
            * (1.0 + FIGHTERS_PCT * level as f64 / 100.0)
            * (1.0 + HULL_DAMAGE_PCT / 100.0);
        let got = attr_of(&reg, ids[0], SKILLED, 2226);
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }

    #[test]
    fn a_pilot_without_a_bonus_skill_gets_the_other_factors_only() {
        // Fighters II is the launch minimum; the hull bonus is there.
        let (reg, ids, _) = flying(&SKILLED_OK, SK_CARRIER, &[(SKILLED, SQUADRON)]);
        let want = own(&reg, SKILLED, 2226)
            * (1.0 + FIGHTERS_PCT * 2.0 / 100.0)
            * (1.0 + HULL_DAMAGE_PCT / 100.0);
        assert!((attr_of(&reg, ids[0], SKILLED, 2226) - want).abs() < 1e-9);
        // A hull without the bonus: the skill alone.
        let (reg, ids, _) = flying(&SKILLED_OK, CARRIER, &[(SKILLED, SQUADRON)]);
        let want = own(&reg, SKILLED, 2226) * (1.0 + FIGHTERS_PCT * 2.0 / 100.0);
        assert!((attr_of(&reg, ids[0], SKILLED, 2226) - want).abs() < 1e-9);
    }

    #[test]
    fn speed_and_hit_points_reach_the_ball_and_the_engine() {
        let level = 4;
        let (reg, ids, _) = flying(
            &[(SK_LIGHT, level), (SK_FIGHTERS, 2)],
            SK_CARRIER,
            &[(SKILLED, SQUADRON)],
        );
        let sq = ids[0];
        let speed = own(&reg, SKILLED, 37) * (1.0 + LIGHT_PCT * level as f64 / 100.0);
        let h = base_hull(&reg, sq).expect("flying");
        assert!(
            (f64::from(h.max_velocity) - speed).abs() < 1.0,
            "{}",
            h.max_velocity
        );
        // What the engine integrates and the clients are told of.
        let engine = crate::dogma::hull(&reg, sq).expect("a squadron");
        assert_eq!(engine.max_velocity, h.max_velocity);
        // The hull's shield bonus, in the health a hit is taken from.
        let shield = own(&reg, SKILLED, 263) * (1.0 + HULL_SHIELD_PCT / 100.0);
        let p = crate::dogma::profile(&reg, sq).expect("a squadron's health");
        assert!(
            (p.shield_capacity - shield).abs() < 1e-9,
            "{}",
            p.shield_capacity
        );
        // The unskilled hull's squadron keeps the type's own.
        let (reg, ids, _) = flying(&SKILLED_OK, CARRIER, &[(SKILLED, SQUADRON)]);
        let p = crate::dogma::profile(&reg, ids[0]).expect("health");
        assert_eq!(p.shield_capacity, own(&reg, SKILLED, 263));
    }

    #[test]
    fn hangar_management_raises_capacity() {
        let capacity = |level: i64| {
            let mut reg = ft::world(SK_CARRIER, &[]);
            train(&mut reg, ALICE, &[(SK_HANGAR, level)]);
            bay_capacity(&reg, ALICE_SHIP)
        };
        let base = own(&ft::world(SK_CARRIER, &[]), SK_CARRIER, 2055);
        for level in 1..=5 {
            let want = base * (1.0 + HANGAR_PCT * level as f64 / 100.0);
            assert!((capacity(level) - want).abs() < 1e-9, "level {level}");
        }
        // The raised room takes a stack the type's own would refuse.
        let mut reg = ft::world(SK_CARRIER, &[]);
        let fits = (capacity(5) / ft::LIGHT_VOLUME) as i64;
        assert!(plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, ft::HANGAR, Some(fits)).is_err());
        train(&mut reg, ALICE, &[(SK_HANGAR, 5)]);
        assert!(plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, ft::HANGAR, Some(fits)).is_ok());
        // And the client's own question answers it.
        assert_eq!(
            bay_capacity_value(&reg, ALICE_SHIP),
            crate::pyutil::keyval(vec![
                ("capacity", PyValue::Real(capacity(5))),
                ("used", PyValue::Real(0.0)),
            ])
        );
    }

    #[test]
    fn a_hull_modifier_adds_a_tube() {
        let reg = ft::world(SK_CARRIER, &[(SUPPORT, 2)]);
        let slots = ship_slots(&reg, ALICE_SHIP);
        assert_eq!(slots.tubes, own(&reg, SK_CARRIER, 2216) as i64 + 1);
        assert_eq!(
            slots.support,
            own(&reg, SK_CARRIER, 2218) as i64,
            "the class slots are as stated"
        );
        // The fourth tube (flag 162) takes a squadron on the bonus hull...
        let mut reg = ft::world(SK_CARRIER, &[(SUPPORT, 2)]);
        ft::load_to(&mut reg, BAY, FIRST_TUBE + 3).expect("a fourth tube");
        // ... and not on the plain one.
        let mut plain = ft::world(CARRIER, &[(SUPPORT, 2)]);
        let e = ft::load_to(&mut plain, BAY, FIRST_TUBE + 3).expect_err("three tubes");
        assert_eq!(
            ft::key_of(&e),
            "CannotLoadFighterTubeNoTubesAvailableForClass"
        );
    }

    #[test]
    fn a_missing_skill_refuses_the_launch() {
        let mut reg = ft::world(CARRIER, &[(SKILLED, SQUADRON)]);
        // Light Fighters trained, Fighters not (the type wants it at II).
        train(&mut reg, ALICE, &[(SK_LIGHT, 1), (SK_FIGHTERS, 1)]);
        ft::undock(&mut reg);
        ft::load_to(&mut reg, BAY, FIRST_TUBE).expect("a pilot may carry what he cannot fly");
        ft::settle(&mut reg);
        let reply = ft::call(
            &mut reg,
            "LaunchFightersFromTubes",
            &[ft::ints(&[FIRST_TUBE])],
        )
        .expect("a dict");
        let PyValue::Dict(d) = reply else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1);
        assert_eq!(ft::key_of_value(&d[0].0), "FighterHasSkillPrerequisites");
        assert_eq!(state(&reg).squadrons().count(), 0, "nothing in space");
        assert!(ft::tube_of(&reg, FIRST_TUBE).is_some(), "still in its tube");
        // The error carries the client's three arguments.
        let r = gate(&reg, ALICE, SKILLED).expect_err("refused");
        assert_eq!(r.key, "FighterHasSkillPrerequisites");
        let get = |name: &str| {
            r.args
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("skillCount"), Some(PyValue::Int(1)));
        assert_eq!(get("item"), Some(PyValue::Int(SKILLED)));
        assert_eq!(
            get("requiredSkills"),
            Some(PyValue::Unicode("Fixture Fighters level 2".to_owned()))
        );
        // Train it and the same tube launches.
        train(&mut reg, ALICE, &[(SK_FIGHTERS, 2)]);
        let reply = ft::call(
            &mut reg,
            "LaunchFightersFromTubes",
            &[ft::ints(&[FIRST_TUBE])],
        )
        .expect("a dict");
        let PyValue::Dict(d) = reply else {
            panic!("a dict")
        };
        assert_eq!(d[0].0, PyValue::None, "launched");
        assert_eq!(state(&reg).squadrons().count(), 1);
    }

    #[test]
    fn the_gate_names_every_missing_skill_and_only_those() {
        let mut reg = ft::world(CARRIER, &[]);
        assert_eq!(
            missing(&reg, ALICE, SKILLED),
            vec![(SK_LIGHT, 1), (SK_FIGHTERS, 2)]
        );
        let r = gate(&reg, ALICE, SKILLED).expect_err("an untrained pilot");
        let count = r
            .args
            .iter()
            .find(|(k, _)| *k == "skillCount")
            .map(|(_, v)| v.clone());
        assert_eq!(count, Some(PyValue::Int(2)));
        train(&mut reg, ALICE, &[(SK_LIGHT, 1)]);
        assert_eq!(missing(&reg, ALICE, SKILLED), vec![(SK_FIGHTERS, 2)]);
        train(&mut reg, ALICE, &[(SK_FIGHTERS, 5)]);
        assert!(gate(&reg, ALICE, SKILLED).is_ok());
        // A type that names no skill is open to anyone.
        assert!(gate(&reg, ALICE, ft::LIGHT_A).is_ok());
    }
}
