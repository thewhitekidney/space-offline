//! **Ship and cargo scanners** (EWAR-2D, `docs/lanes/ewar-2d.md`).
//!
//! A ship scanner (effect 46) and a cargo scanner (effect 47) are target
//! effects, category 2, **not offensive** [V `dogmaeffects`]: no aggression,
//! no crimewatch. One cycle, then the answer goes to the scanning pilot
//! alone, as the client's two handlers want it [V `shipUI.py:986`, `:996`]:
//!
//! - `OnShipScanCompleted(shipID, capacitorCharge, capacitorCapacity,
//!   hardwareList)`, `hardwareList` a list of `(typeID, quantity)`;
//! - `OnCargoScanComplete(shipID, cargoList)`, `cargoList` a list of
//!   `(typeID, quantity)`, quantity `-2` marking a blueprint copy.
//!
//! The client indexes `bp.slimItems[shipID]`, which raises if the ball is
//! gone, so nothing is sent unless the target is still in the pilot's
//! system and in the roster.
//!
//! ## What is ours [A]
//!
//! - **Ship scan**: every item at a fitting flag (modules and loaded
//!   charges) is kept with the chance [`crate::ewar::falloff`] gives at the
//!   gap (the attribute text says "accuracy has fallen by half" at range
//!   plus falloff [V]). A per-item roll is an EvEJS hint; falsified by a
//!   capture of a partial list with another pattern.
//! - **Cargo scan**: the target's attribute 188 ("chance of being able to
//!   resist" [V]) at or above the roll resists, and a resisted scan answers
//!   an **empty** list. Falsified by a live refusal or message instead.
//!   Holds are the cargo and every other hold but the drone bay; a
//!   container is listed with what is in it, one level down.
//! - **Not a ship in the roster** (a can, an NPC): an empty list.
//! - **Reach** is range plus [`crate::ewar::FALLOFF_REACH`] falloffs (EvEJS
//!   says one). Falsified by a live refusal between the two.

use crate::item::Item;
use crate::modules::Cycle;
use crate::notify::Push;
use crate::service::Registry;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

pub const SHIP_SCAN: i64 = 46;
pub const CARGO_SCAN: i64 = 47;

/// `maxRange` of a ship scanner, and its falloff [V `dogmaeffects`].
const SHIP_RANGE: i64 = 125;
const SHIP_FALLOFF: i64 = 510;
/// `maxRange` of a cargo scanner; it has no falloff attribute [V].
const CARGO_RANGE: i64 = 126;
/// `attributeCargoScanResist` [V].
const CARGO_RESIST: i64 = 188;
/// The roll's salt, so a scan and an ECM on one module do not share rolls.
const SALT: u64 = 0x5CA7_0046;
/// `-singletonBlueprintCopy`, the client's marker for a blueprint copy [V].
const BLUEPRINT_COPY: i64 = -2;

pub fn is_scanner(effect_id: i64) -> bool {
    matches!(effect_id, SHIP_SCAN | CARGO_SCAN)
}

/// `(range, falloff)` attribute ids of a scanner, `None` for any other effect.
pub fn reach_attrs(effect_id: i64) -> Option<(i64, Option<i64>)> {
    match effect_id {
        SHIP_SCAN => Some((SHIP_RANGE, Some(SHIP_FALLOFF))),
        CARGO_SCAN => Some((CARGO_RANGE, None)),
        _ => None,
    }
}

/// `OnShipScanCompleted`, to the scanning pilot alone.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipScanned {
    pub to: i64,
    pub ship_id: i64,
    pub charge: f64,
    pub capacity: f64,
    pub hardware: Vec<(i64, i64)>,
}

/// `OnCargoScanComplete`, to the scanning pilot alone.
#[derive(Debug, Clone, PartialEq)]
pub struct CargoScanned {
    pub to: i64,
    pub ship_id: i64,
    pub cargo: Vec<(i64, i64)>,
}

fn rows(list: &[(i64, i64)]) -> PyValue {
    PyValue::List(
        list.iter()
            .map(|(t, q)| PyValue::Tuple(vec![PyValue::Int(*t), PyValue::Int(*q)]))
            .collect(),
    )
}

impl Push for ShipScanned {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnShipScanCompleted",
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Real(self.charge),
                PyValue::Real(self.capacity),
                rows(&self.hardware),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

impl Push for CargoScanned {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCargoScanComplete",
            vec![PyValue::Int(self.ship_id), rows(&self.cargo)],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// The fitted items and loaded charges that the scan kept, one row per type.
/// `keep` is the per-item roll.
pub fn hardware_rows(items: &[Item], keep: impl Fn(&Item) -> bool) -> Vec<(i64, i64)> {
    let mut by_type: BTreeMap<i64, i64> = BTreeMap::new();
    for i in items {
        if crate::item::is_ship_fitting_flag(i.flag_id) && keep(i) {
            *by_type.entry(i.type_id).or_insert(0) += i.quantity.max(1);
        }
    }
    by_type.into_iter().collect()
}

/// Cargo rows from `(item, is a blueprint copy)`: stacks of one type are
/// summed, and each copy is its own row with quantity `-2`.
pub fn cargo_rows(items: &[(Item, bool)]) -> Vec<(i64, i64)> {
    let mut by_type: BTreeMap<i64, i64> = BTreeMap::new();
    let mut copies = Vec::new();
    for (i, copy) in items {
        if *copy {
            copies.push((i.type_id, BLUEPRINT_COPY));
        } else {
            *by_type.entry(i.type_id).or_insert(0) += i.quantity.max(1);
        }
    }
    let mut out: Vec<(i64, i64)> = by_type.into_iter().collect();
    out.extend(copies);
    out
}

/// Whether a flag is a hold a cargo scan reads: the cargo and every other
/// hold, the drone bay excepted [A].
fn scanned_hold(flag_id: i64) -> bool {
    flag_id == crate::fitting::FLAG_CARGO
        || (crate::item::hold_of(flag_id).is_some() && flag_id != crate::drones::FLAG_DRONE_BAY)
}

/// The pure part of a ship scan: what the roll keeps at `gap`.
fn kept(gap: f64, range: f64, fall: f64, c: &Cycle, item: &Item) -> bool {
    let chance = crate::ewar::falloff(gap, range, fall);
    crate::drone_skills::roll(c.module_id, c.index, SALT ^ item.item_id as u64) < chance
}

/// Every ended scanner cycle answers; any other cycle is not this file's.
pub fn on_completed(w: &mut Registry, done: &[Cycle]) {
    for c in done {
        if is_scanner(c.effect_id) {
            scan(w, c);
        }
    }
}

fn read(w: &Registry, c: &Cycle, attr: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, c.ship_id, c.module_id, attr)
        .or_else(|| crate::modules::find_in(w.content(), c.type_id)?.attribute(attr))
}

fn scan(w: &mut Registry, c: &Cycle) {
    let Some(target) = c.target_id else { return };
    let Some(here) = w.roster().get(c.character_id).cloned() else {
        return;
    };
    if here.ship_id != c.ship_id {
        return;
    }
    // A target that left the roster, or the system, sends nothing: the
    // client would raise on a ball it no longer has.
    let Some(there) = w.roster().by_ship(target).cloned() else {
        return;
    };
    if there.solar_system_id != here.solar_system_id {
        return;
    }
    let Some((range_attr, fall_attr)) = reach_attrs(c.effect_id) else {
        return;
    };
    let range = read(w, c, range_attr).unwrap_or(0.0);
    let fall = fall_attr.and_then(|a| read(w, c, a)).unwrap_or(0.0);
    let Some(gap) = crate::module_range::gap_between(w, c.ship_id, target) else {
        return;
    };
    let to = c.character_id;
    if c.effect_id == SHIP_SCAN {
        let items: Vec<Item> = w.store().contents_of(target).cloned().collect();
        let hardware = hardware_rows(&items, |i| kept(gap, range, fall, c, i));
        let ship_type = w.pilot(there.character_id).map_or(0, |p| p.ship_type_id);
        let (charge, capacity) =
            crate::modules::capacitor_at(w, target, ship_type, crate::pyutil::filetime_now());
        w.schedule(
            Duration::ZERO,
            ShipScanned {
                to,
                ship_id: target,
                charge,
                capacity,
                hardware,
            },
        );
    } else {
        let resist = crate::dogma::sheet(w, target)
            .and_then(|s| s.ship_opt(CARGO_RESIST))
            .unwrap_or(0.0);
        let resisted = resist > 0.0
            && crate::drone_skills::roll(c.module_id, c.index, SALT.wrapping_add(1)) < resist;
        let cargo = if resisted {
            Vec::new()
        } else {
            let mut flat: Vec<Item> = Vec::new();
            for i in w
                .store()
                .contents_of(target)
                .filter(|i| scanned_hold(i.flag_id))
            {
                flat.push(i.clone());
                flat.extend(w.store().contents_of(i.item_id).cloned());
            }
            let table = w.content().get();
            let tagged: Vec<(Item, bool)> = flat
                .into_iter()
                .map(|i| {
                    let copy = w
                        .jobs()
                        .blueprint_item(table, &i)
                        .is_some_and(|b| !b.original());
                    (i, copy)
                })
                .collect();
            cargo_rows(&tagged)
        };
        w.schedule(
            Duration::ZERO,
            CargoScanned {
                to,
                ship_id: target,
                cargo,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, type_id: i64, flag: i64, quantity: i64) -> Item {
        Item {
            item_id: id,
            type_id,
            owner_id: 1,
            location_id: 7,
            flag_id: flag,
            quantity,
            group_id: 0,
            category_id: 0,
            singleton: false,
        }
    }

    const FITTED: i64 = 19;
    const CARGO: i64 = 5;
    const RANGE: f64 = 10_000.0;
    const FALL: f64 = 5_000.0;

    fn cycle() -> Cycle {
        Cycle {
            character_id: 1,
            ship_id: 2,
            module_id: 3,
            type_id: 990_840_001,
            effect_id: SHIP_SCAN,
            target_id: Some(7),
            at: 0,
            index: 0,
            charge_type_id: None,
        }
    }

    #[test]
    fn scanner_reach_names_the_clients_attributes() {
        assert_eq!(reach_attrs(SHIP_SCAN), Some((125, Some(510))));
        assert_eq!(reach_attrs(CARGO_SCAN), Some((126, None)));
        assert_eq!(reach_attrs(6426), None);
    }

    #[test]
    fn inside_range_every_fitted_item_is_listed() {
        let items = vec![
            item(10, 990_840_010, FITTED, 1),
            item(11, 990_840_011, FITTED, 1),
            item(12, 990_840_012, CARGO, 50),
        ];
        let c = cycle();
        let rows = hardware_rows(&items, |i| kept(RANGE - 1.0, RANGE, FALL, &c, i));
        assert_eq!(rows, vec![(990_840_010, 1), (990_840_011, 1)]);
    }

    #[test]
    fn past_range_only_the_rolled_items_are_listed_and_past_reach_none() {
        let items: Vec<Item> = (0..200)
            .map(|n| item(100 + n, 990_840_100 + n, FITTED, 1))
            .collect();
        let c = cycle();
        let some = hardware_rows(&items, |i| kept(RANGE + FALL, RANGE, FALL, &c, i));
        assert!(
            !some.is_empty() && some.len() < items.len(),
            "{}",
            some.len()
        );
        let none = hardware_rows(&items, |i| kept(RANGE + 4.0 * FALL, RANGE, FALL, &c, i));
        assert!(none.is_empty());
    }

    #[test]
    fn cargo_sums_stacks_and_marks_each_copy_negative() {
        let rows = cargo_rows(&[
            (item(1, 990_840_020, CARGO, 40), false),
            (item(2, 990_840_020, CARGO, 2), false),
            (item(3, 990_840_021, CARGO, 1), true),
        ]);
        assert_eq!(rows, vec![(990_840_020, 42), (990_840_021, -2)]);
    }

    #[test]
    fn the_cargo_flag_and_holds_are_read_but_not_the_drone_bay() {
        assert!(scanned_hold(CARGO));
        assert!(!scanned_hold(crate::drones::FLAG_DRONE_BAY));
        assert!(!scanned_hold(FITTED));
    }

    /// A target that is not in the roster sends nothing: the client raises
    /// on a ball it does not have.
    #[test]
    fn a_target_that_is_gone_sends_nothing() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let mut c = cycle();
        c.target_id = Some(990_840_777);
        on_completed(&mut reg, &[c]);
        c.effect_id = CARGO_SCAN;
        on_completed(&mut reg, &[c]);
        let far = std::time::Instant::now() + Duration::from_secs(3_600);
        assert!(reg.take_due(far).is_empty());
    }

    #[test]
    fn the_pushes_go_to_the_scanner_alone_with_the_clients_shapes() {
        let s = ShipScanned {
            to: 9,
            ship_id: 8,
            charge: 1.0,
            capacity: 2.0,
            hardware: vec![(5, 1)],
        };
        let (name, args) = s.to_push();
        assert_eq!(name, "OnShipScanCompleted");
        assert_eq!(args.len(), 4);
        assert_eq!(s.audience(), Some(vec![9]));
        let k = CargoScanned {
            to: 9,
            ship_id: 8,
            cargo: vec![(5, -2)],
        };
        let (name, args) = k.to_push();
        assert_eq!(name, "OnCargoScanComplete");
        assert_eq!(
            args[1],
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(5),
                PyValue::Int(-2)
            ])])
        );
    }
}
