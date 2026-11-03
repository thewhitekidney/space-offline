//! **Where boosters come from** (issue #80, `handback/r-boosters/report.md`
//! §6, owner decision D1): no TQ source for the booster blueprints is in the
//! client data, and under "nothing from nowhere" a booster with no source is
//! no booster. So, by owner decision:
//!
//! - **the reaction formulas** of the booster chain are sold by NPCs
//!   (`econ_exchange`, the orphan formulas), and
//! - **the booster blueprints** are blueprint copies that NPCs drop
//!   (`eve-mod-econ`'s `boosters`).
//!
//! This file is the rule that says which types those are. **No type id is
//! written here**: everything is selected from the bundle, so a rebuilt
//! bundle changes the set.
//!
//! - A **drug booster** is a published type of group 303 with a duration
//!   (attribute 330) and at least one side effect (an effect with a
//!   `fittingUsageChanceAttributeID`). The cerebral accelerators have an age
//!   limit instead of a duration, and the odd storyline boosters have no side
//!   effect, so neither is selected. A table written before the usage-chance
//!   column selects nothing (fail closed, as the use of a booster does).
//! - A **booster blueprint** is a manufacturing blueprint whose product is a
//!   drug booster.
//! - A **chain formula** is a reaction formula that makes a drug booster or
//!   anything reached upward from a booster blueprint's materials through
//!   other formulas' inputs (the pure intermediates, then what they react
//!   from), to a fixpoint. Inputs no formula makes (gas, Spirits, Garbage,
//!   fuel blocks) end the walk, so unrelated reactions stay unsold.
//!
//! **[A]** that "drug booster" is the set the owner means by Standard,
//! Improved, Strong and Synth. Falsifier: the selected set on the pinned
//! bundle is not the 32 drug boosters (8 drugs by 4 tiers) the report counted;
//! `tools/analysis.sh` over `out/content` shows it.

use crate::industry;
use crate::typedata;
use std::collections::BTreeSet;

/// `groupBooster` (303) **[V]**.
pub const GROUP_BOOSTER: i64 = 303;
/// `boosterDuration` (330) **[V]**, ms.
pub const ATTR_DURATION: i64 = 330;

/// The drug boosters of `t`, by type id.
pub fn drug_boosters(t: &typedata::Table) -> BTreeSet<i64> {
    t.types()
        .filter(|r| r.group_id == GROUP_BOOSTER && r.published)
        .filter(|r| r.attribute(ATTR_DURATION).is_some_and(|d| d > 0.0))
        .filter(|r| {
            r.effects.iter().any(|e| {
                t.effect(*e)
                    .and_then(|e| e.usage_chance_attribute)
                    .is_some_and(|a| a > 0)
            })
        })
        .map(|r| r.type_id)
        .collect()
}

/// The manufacturing blueprints that make one of `boosters`, ascending.
pub fn booster_blueprints(table: &industry::Table, boosters: &BTreeSet<i64>) -> Vec<i64> {
    let mut v: Vec<i64> = table
        .blueprints
        .iter()
        .filter(|b| boosters.contains(&b.product_type_id))
        .map(|b| b.blueprint_type_id)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The reaction formulas of the booster chain (see the module doc), by
/// blueprint type id.
pub fn chain_formulas(table: &industry::Table, boosters: &BTreeSet<i64>) -> BTreeSet<i64> {
    let mut wanted: BTreeSet<i64> = boosters.clone();
    for b in &table.blueprints {
        if boosters.contains(&b.product_type_id) {
            wanted.extend(b.materials.iter().map(|(m, _)| *m));
        }
    }
    let mut found = BTreeSet::new();
    loop {
        let before = (wanted.len(), found.len());
        for r in &table.reactions {
            if wanted.contains(&r.product_type_id) && found.insert(r.blueprint_type_id) {
                wanted.extend(r.materials.iter().map(|(m, _)| *m));
            }
        }
        if (wanted.len(), found.len()) == before {
            return found;
        }
    }
}

/// The booster blueprints of a loaded world, for a drop to pick from.
pub fn blueprints_in(content: &crate::content::Content) -> Vec<i64> {
    let boosters = drug_boosters(typedata::installed_in(content).table());
    booster_blueprints(content.get::<industry::Table>(), &boosters)
}

/// The chain formulas of a bundle: what `econ_exchange` sells when no
/// corporation does. Empty when the bundle has no type data or no formulas.
pub fn formulas_in_bundle(b: &crate::content::Bundle) -> Result<BTreeSet<i64>, String> {
    let Some(path) = b.path(crate::content::TYPEDATA) else {
        return Ok(BTreeSet::new());
    };
    let types = typedata::Table::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let boosters = drug_boosters(&types);
    if boosters.is_empty() {
        return Ok(BTreeSet::new());
    }
    Ok(chain_formulas(&industry::Table::from_bundle(b)?, &boosters))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boosters::fixture;

    fn types() -> typedata::Table {
        let text = format!(
            "{}{}{}",
            typedata::FIXTURE,
            crate::implants::fixture::rows(),
            fixture::type_rows()
        );
        typedata::Table::parse(&text).expect("the fixture parses")
    }

    // Invented ids: a booster 7001, its pure intermediates 7101 and 7102, a
    // gas 7201, and a formula for each of the first two plus an unrelated one.
    fn recipes(booster: i64) -> industry::Table {
        let rows = [
            // The blueprint 7301 makes the booster from the pure 7101.
            "7301\t1\tactivity\t0\t0\t\t60\t10".to_owned(),
            "7301\t1\tmaterial\t7101\t3\t\t60\t10".to_owned(),
            format!("7301\t1\tproduct\t{booster}\t1\t\t60\t10"),
            // Formula 7401: 7101 from 7102. Formula 7402: 7102 from gas 7201.
            "7401\t9\tactivity\t0\t0\t\t60\t10".to_owned(),
            "7401\t9\tmaterial\t7102\t2\t\t60\t10".to_owned(),
            "7401\t9\tproduct\t7101\t1\t\t60\t10".to_owned(),
            "7402\t9\tactivity\t0\t0\t\t60\t10".to_owned(),
            "7402\t9\tmaterial\t7201\t2\t\t60\t10".to_owned(),
            "7402\t9\tproduct\t7102\t1\t\t60\t10".to_owned(),
            // Formula 7403 makes something no booster needs.
            "7403\t9\tactivity\t0\t0\t\t60\t10".to_owned(),
            "7403\t9\tmaterial\t7201\t2\t\t60\t10".to_owned(),
            "7403\t9\tproduct\t7999\t1\t\t60\t10".to_owned(),
        ];
        industry::Table::parse(&rows, &crate::econ_prices::Table::default()).expect("parses")
    }

    #[test]
    fn a_drug_booster_has_a_duration_and_a_side_effect() {
        let set = drug_boosters(&types());
        assert!(set.contains(&fixture::ALWAYS));
        assert!(set.contains(&fixture::NEVER));
        // No side effect, or no duration (an age limit instead).
        assert!(!set.contains(&fixture::PLAIN));
        assert!(!set.contains(&fixture::OTHER_SLOT));
        assert!(!set.contains(&fixture::AGE_LIMITED));
    }

    #[test]
    fn a_table_without_the_usage_chance_column_selects_nothing() {
        let text =
            typedata::FIXTURE.to_owned() + &fixture::type_rows().replace("\t1\t1089\n", "\n");
        let old = typedata::Table::parse(&text).expect("parses");
        assert!(drug_boosters(&old).is_empty());
    }

    #[test]
    fn the_chain_is_walked_up_to_the_gas_and_no_further() {
        let t = recipes(fixture::ALWAYS);
        let boosters = BTreeSet::from([fixture::ALWAYS]);
        assert_eq!(booster_blueprints(&t, &boosters), vec![7301]);
        assert_eq!(chain_formulas(&t, &boosters), BTreeSet::from([7401, 7402]));
    }

    #[test]
    fn nothing_is_selected_for_a_world_without_boosters() {
        let t = recipes(fixture::ALWAYS);
        let none = BTreeSet::new();
        assert!(booster_blueprints(&t, &none).is_empty());
        assert!(chain_formulas(&t, &none).is_empty());
    }
}
