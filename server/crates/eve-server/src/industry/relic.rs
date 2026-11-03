//! Ancient relics as blueprints (reverse engineering, T3-B) and the generic
//! decryptors (T3-C). Lane `t3-a`; spec `R-T3.md` §0, §1.2, §1.3, §3, §6.2.
//!
//! # There is no separate reverse-engineering activity
//!
//! **[V]** `industry/const.pyc` has six activities (1, 3, 4, 5, 8, 9) and
//! `industry_activities.static` the same six. Reverse engineering is
//! **invention (activity 8)** whose "blueprint" is an *ancient relic*, an
//! item of category 34:
//!
//! - `industryCommon.IsBlueprintCategory` counts category 34 as a blueprint
//!   category, and `BlueprintInstance` derives
//!   `original = data.quantity != -2 and category != 34`: a relic is never an
//!   original, so `Invention.REQUIRES_COPY` is satisfied by it;
//! - `blueprints.static` has a row for each relic type with **only** an
//!   `invention` activity: two datacores (3 each, per run), three skills at
//!   level 1, N products (blueprint types) whose `probability` is the base
//!   chance and whose `quantity` is the **runs of the copy made**.
//!
//! So the activity is already inside [`super::Jobs::plan_install`]'s
//! invention path. What was missing, and is here, is that [`super::Table`]
//! dropped every blueprint without a manufacturing activity, so a relic was
//! no blueprint and `InstallJob` answered `ItemNotBlueprint`.
//!
//! # What a relic job spends **[A]**
//!
//! A relic is a *stack* (`quantity` units under one item id); a job of `r`
//! runs consumes `r` units of the stack, `r x 3` of each datacore, and the
//! decryptor if one is chosen. The stack's row on the wire reads
//! `runs = quantity = units` ([`super::BlueprintItem::wire_quantity_in`]).
//! Evidence for: `Invention.job_max_runs` is
//! `min(blueprint.runsRemaining, ...)`, `INVALID_RUNS` needs
//! `runsRemaining > 0`, and `job_output_extras` hands the input back with
//! `runsRemaining - runs`: for a relic that *is* the rest of the stack.
//! Falsifier: the Runs slider for a stack of 5 intact relics in the client
//! does not stop at 5 (`docs/guides/human-checks.md`, H-T3-1).
//!
//! Because the units are taken **at install** (not held by the job), a relic
//! stack is never "installed": a second job may use what is left of it while
//! the first runs. `BlueprintInstalled` is not checked for a relic and its
//! row's `jobID` is `None`.
//!
//! # Decryptors
//!
//! **[V]** `industryCommon.GetDecryptors` offers one optional material per
//! type of group 1304 (the eight *generic* decryptors; the racial and hybrid
//! decryptors of groups 728..731 and 979 are not offered by this client).
//! The chosen decryptor is a consumed material of quantity 1 carrying four
//! modifiers read from its own attributes:
//!
//! | attribute | modifier | effect |
//! |---|---|---|
//! | 1112 `inventionPropabilityMultiplier` | `ProbabilityModifier(attr)` | multiplies the chance, the product is clamped to 0..=1 |
//! | 1113 `inventionMEModifier` | `MaterialModifier(attr / 100, output)` | `+int(sum(amount * 100))` to the copy's ME 2 |
//! | 1114 `inventionPEModifier` | `TimeModifier(attr / 100, output)` | the same for TE 4 |
//! | 1124 `inventionMaxRunModifier` | `MaxRunsModifier(attr, output)` | `+attr` to the copy's runs, `int(round(quantity + maxRuns))` |
//!
//! The numbers come from the decryptor type's own attributes in the content
//! bundle (`types/typedata.tsv`), never from a table compiled in here.
//! It applies to T2 invention as well; nothing in the client limits it to
//! relics.

use std::collections::BTreeMap;

use super::{
    Blueprint, Gathered, INVENTION, INVENTION_MATERIAL_EFFICIENCY, INVENTION_TIME_EFFICIENCY,
    Invention, MANUFACTURING, Science, Table,
};

/// `groupDecryptors` **[V]** `inventorycommon/const.pyc`: the only group
/// `industryCommon.GetDecryptors` offers.
pub const GROUP_DECRYPTORS: i64 = 1304;
/// `attributeInventionPropabilityMultiplier` **[V]** (the spelling is CCP's).
pub const ATTR_PROBABILITY_MULTIPLIER: i64 = 1112;
/// `attributeInventionMEModifier` **[V]**.
pub const ATTR_MATERIAL_EFFICIENCY: i64 = 1113;
/// `attributeInventionPEModifier` **[V]**.
pub const ATTR_TIME_EFFICIENCY: i64 = 1114;
/// `attributeInventionMaxRunModifier` **[V]**.
pub const ATTR_MAX_RUNS: i64 = 1124;

/// One ancient relic type, as the invention-only blueprint it is.
///
/// `blueprint` is a **synthetic** [`Blueprint`]: no product, no materials, no
/// manufacturing skills; `time` is the invention time and
/// `max_production_limit` the row's `max_runs`. It exists so the shared
/// checks in `plan_install` (`blueprint.blueprint(table)`) have a row to read.
#[derive(Debug, Clone, PartialEq)]
pub struct Relic {
    pub blueprint: Blueprint,
    /// Only `invention` is `Some`.
    pub science: Science,
}

impl Relic {
    fn from_gathered(blueprint_type_id: i64, g: &Gathered) -> Relic {
        let sorted = |mut v: Vec<(i64, i64)>| {
            v.sort_unstable();
            v
        };
        Relic {
            blueprint: Blueprint {
                blueprint_type_id,
                product_type_id: 0,
                product_quantity: 0,
                time: g.time,
                max_production_limit: g.max_runs,
                materials: Vec::new(),
                skills: Vec::new(),
            },
            science: Science {
                blueprint_type_id,
                research_material_time: None,
                research_time_time: None,
                copying_time: None,
                invention: Some(Invention {
                    time: g.time,
                    materials: sorted(g.materials.clone()),
                    products: g.products.clone(),
                    skills: sorted(g.skills.clone()),
                }),
            },
        }
    }

    /// The relic's invention activity.
    pub fn invention(&self) -> &Invention {
        self.science
            .invention
            .as_ref()
            .unwrap_or_else(|| unreachable!("a relic is built with its invention"))
    }
}

/// Every blueprint type that has an `invention` activity with a product and
/// **no manufacturing activity at all**, ascending by type id (the map's
/// order). Called by [`Table::parse`] before it consumes the map.
pub(super) fn collect(by_bp: &BTreeMap<i64, BTreeMap<i64, Gathered>>) -> Vec<Relic> {
    by_bp
        .iter()
        .filter(|(_, acts)| !acts.contains_key(&MANUFACTURING))
        .filter_map(|(&type_id, acts)| {
            let g = acts.get(&INVENTION).filter(|g| !g.products.is_empty())?;
            Some(Relic::from_gathered(type_id, g))
        })
        .collect()
}

impl Table {
    /// The relic with this **type** id.
    pub fn relic(&self, type_id: i64) -> Option<&Relic> {
        self.relics
            .binary_search_by_key(&type_id, |r| r.blueprint.blueprint_type_id)
            .ok()
            .map(|i| &self.relics[i])
    }

    /// Whether this type id is an ancient relic we run invention on.
    pub fn is_relic(&self, type_id: i64) -> bool {
        self.relic(type_id).is_some()
    }
}

/// One generic decryptor, read from its type's attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decryptor {
    pub type_id: i64,
    /// Attribute 1112, the factor in the chance. `1.0` when absent.
    pub probability_multiplier: f64,
    /// Attribute 1113, in percent (the client divides it by 100).
    pub material_efficiency: f64,
    /// Attribute 1114, in percent.
    pub time_efficiency: f64,
    /// Attribute 1124, runs added to the copy.
    pub max_runs: f64,
}

impl Decryptor {
    /// The decryptor this type is, or `None` when the content has no such
    /// type or it is not in [`GROUP_DECRYPTORS`].
    pub fn of(content: &crate::content::Content, type_id: i64) -> Option<Decryptor> {
        let row = crate::typedata::installed_in(content)
            .table()
            .get(type_id)?;
        if row.group_id != GROUP_DECRYPTORS {
            return None;
        }
        let at = |attr: i64, absent: f64| row.attribute(attr).unwrap_or(absent);
        Some(Decryptor {
            type_id,
            probability_multiplier: at(ATTR_PROBABILITY_MULTIPLIER, 1.0),
            material_efficiency: at(ATTR_MATERIAL_EFFICIENCY, 0.0),
            time_efficiency: at(ATTR_TIME_EFFICIENCY, 0.0),
            max_runs: at(ATTR_MAX_RUNS, 0.0),
        })
    }
}

/// The decryptor a request's `materials` keys select.
///
/// `GetDecryptors`: `for typeID in job.request['materials']: if typeID in
/// allTypes: decryptor.select(typeID)` -- the last key that is a decryptor
/// wins, the others are the activity's own materials.
pub fn decryptor_among(content: &crate::content::Content, keys: &[i64]) -> Option<i64> {
    keys.iter()
        .copied()
        .rfind(|&t| Decryptor::of(content, t).is_some())
}

/// What an invented copy starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyStart {
    pub material_efficiency: i64,
    pub time_efficiency: i64,
    pub runs: i64,
}

/// `Invention.job_output_products`' arithmetic, for a product whose
/// `quantity` is `per_copy` runs:
///
/// ```text
///     materialEfficiency = int(sum([m.amount * 100.0 for m in MaterialModifiers]))
///     timeEfficiency     = int(sum([m.amount * 100.0 for m in TimeModifiers]))
///     maxRuns            = sum([m.amount for m in MaxRunsModifiers])
///     runsRemaining      = int(round(product.quantity + maxRuns))
///     ME = INVENTION_MATERIAL_EFFICIENCY + materialEfficiency
/// ```
///
/// where a decryptor's `MaterialModifier.amount` is `attr / 100.0`. The
/// division and multiplication are kept so a value like 0.07 truncates the
/// way Python's `int()` does (`as i64` truncates toward zero as well);
/// `round` is half away from zero in Python 2 and in `f64::round`.
pub fn copy_start(per_copy: i64, decryptor: Option<&Decryptor>) -> CopyStart {
    let (me, te, runs) = match decryptor {
        None => (0.0, 0.0, 0.0),
        Some(d) => (
            d.material_efficiency / 100.0 * 100.0,
            d.time_efficiency / 100.0 * 100.0,
            d.max_runs,
        ),
    };
    CopyStart {
        material_efficiency: INVENTION_MATERIAL_EFFICIENCY + me as i64,
        time_efficiency: INVENTION_TIME_EFFICIENCY + te as i64,
        runs: (per_copy as f64 + runs).round() as i64,
    }
}

#[cfg(test)]
mod tests;
