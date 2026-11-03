//! Damage that survives docking, and the station repair shop that mends it
//! (GAP-REPAIR; spec `handback/evejs-gap-svcD/report.md` §3.1).
//!
//! Before this a dock repaired everything for free (an `[A]` that was meant to
//! be falsified, `combat.rs`'s old note): the ship's health record was
//! forgotten when it left space, and the next undock began with a full ship.
//! Now a dock **stashes** the ship's armour and structure damage, and its
//! fitted modules' damage, as `hulldmg` delta rows (`item -> armor_damage
//! structure_damage`, kept only while non-zero); an undock seeds them back
//! into the health record and the modules' slot. Docking still refills the
//! shield and the capacitor (`[P]`, the retail rule); only the repair shop
//! mends armour, structure and modules, for ISK.
//!
//! **The repair shop's wire** is [`crate::service::repair`]; the quote maths
//! and the repair itself are here, so a bot or a test can call them directly.
//!
//! **Cost**: `unit = STATION_REPAIR_COST_MULTIPLIER (0.1, [V], client
//! constant) × P_ref(type) / maxHealth` per point of damage. `[A]` that a full
//! repair from zero health costs 10% of the item's reference price;
//! falsified by a retail-like ship whose quote the owner reads an order of
//! magnitude off.
//!
//! **What is repaired** (`[V]`, the header names only structure and armour
//! amounts): armour and structure, never shield; a module's damage (attribute
//! 3 against `hp`). **Order** `[A]`: armour first, then structure, on a
//! partial payment; falsified by a retail partial repair that mends structure
//! first.

use crate::service::Registry;

/// The delta kind this module owns: `item_id -> armor_damage
/// structure_damage`.
pub const KIND: &str = "hulldmg";

/// `repair.STATION_REPAIR_COST_MULTIPLIER` `[V]`.
pub const COST_MULTIPLIER: f64 = 0.1;

/// `appConst.refRepairBill` `[V]`.
pub const REF_REPAIR_BILL: i64 = 15;

/// `categoryModule`, `categoryDrone` (`inventorycommon.const`) `[V]`.
const CATEGORY_MODULE: i64 = 7;
const CATEGORY_DRONE: i64 = 18;

/// One item's stashed damage, in hit points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hurt {
    pub armor: f64,
    pub structure: f64,
}

impl Hurt {
    pub fn total(&self) -> f64 {
        self.armor + self.structure
    }

    fn is_zero(&self) -> bool {
        self.armor <= 0.0 && self.structure <= 0.0
    }
}

/// An item's stashed damage, if it has any and the item still exists.
pub fn hurt(reg: &Registry, item_id: i64) -> Option<Hurt> {
    reg.item(item_id)?;
    let v = reg.deltas().of(KIND).get(&vec![item_id.to_string()])?;
    let [a, s] = v.as_slice() else { return None };
    let (armor, structure) = (a.parse::<f64>().ok()?, s.parse::<f64>().ok()?);
    (armor.is_finite() && structure.is_finite()).then_some(())?;
    let h = Hurt {
        armor: armor.max(0.0),
        structure: structure.max(0.0),
    };
    (!h.is_zero()).then_some(h)
}

/// Store an item's damage; a whole item loses its row.
pub fn put(reg: &mut Registry, item_id: i64, h: Hurt) {
    let key = vec![item_id.to_string()];
    if h.is_zero() {
        reg.deltas_mut().del(KIND, &key);
    } else {
        reg.deltas_mut().put(
            KIND,
            key,
            vec![h.armor.to_string(), h.structure.to_string()],
        );
    }
}

/// Whether an item carries damage a repackage must refuse
/// (`repackaging.CheckCanRepackage` -> `CantRepackageDamagedItem`, `[V]`).
pub fn is_damaged(reg: &Registry, item_id: i64) -> bool {
    hurt(reg, item_id).is_some() || crate::overheat::module_damage(reg, item_id) > 0.0
}

/// Drop every row whose item no longer exists (destroyed, trashed or
/// reprocessed): no item, no row.
pub fn prune(reg: &mut Registry) {
    let dead: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| {
            k.first()
                .and_then(|id| id.parse::<i64>().ok())
                .is_none_or(|id| reg.item(id).is_none())
        })
        .cloned()
        .collect();
    for k in dead {
        reg.deltas_mut().del(KIND, &k);
    }
}

/// **A dock**: stash the ship's armour and structure damage and its modules'
/// damage, and forget the in-memory record. Called by `Registry::dock_change`
/// before the roster forgets the pilot.
pub fn on_dock(reg: &mut Registry, actor: i64) {
    let Some(ship_id) = reg.character(actor).map(|c| c.ship_id) else {
        return;
    };
    let now = crate::pyutil::filetime_now();
    if let Some(rec) = reg.combat_mut().take_ship(ship_id) {
        let r = rec.rings(now);
        put(
            reg,
            ship_id,
            Hurt {
                armor: r.armor_damage,
                structure: r.damage,
            },
        );
    }
    let inside: Vec<i64> = reg
        .store()
        .contents_of(ship_id)
        .map(|i| i.item_id)
        .collect();
    for id in inside {
        let hp = reg.ext_mut::<crate::overheat::Heat>().take_damage(id);
        if hp > 0.0 {
            put(
                reg,
                id,
                Hurt {
                    armor: 0.0,
                    structure: hp,
                },
            );
        }
    }
}

/// **An undock**: a damaged ship gets a health record with a full shield and
/// the stashed armour and structure; its modules' damage goes back to the
/// heat slot. The rows go: the record is the truth while in space.
pub fn on_undock(reg: &mut Registry, actor: i64) {
    let Some((ship_id, type_id)) = reg.character(actor).map(|c| (c.ship_id, c.ship_type_id)) else {
        return;
    };
    let now = crate::pyutil::filetime_now();
    if let Some(h) = hurt(reg, ship_id) {
        let profile = crate::dogma::profile(reg, ship_id).unwrap_or_else(|| {
            crate::damage::Profile::from_hull_in(
                reg.content(),
                crate::destiny::hull::of_in(reg.content(), type_id),
            )
        });
        reg.combat_mut().seed_ship(
            ship_id,
            crate::damage::ShipDamage::with_damage(profile, now, h.armor, h.structure),
        );
    }
    reg.deltas_mut().del(KIND, &[ship_id.to_string()]);
    let inside: Vec<i64> = reg
        .store()
        .contents_of(ship_id)
        .map(|i| i.item_id)
        .collect();
    for id in inside {
        if let Some(h) = hurt(reg, id) {
            reg.ext_mut::<crate::overheat::Heat>()
                .put_damage(id, h.structure);
            reg.deltas_mut().del(KIND, &[id.to_string()]);
        }
    }
}

/// One line of a quote: a ship's hull, a fitted module or a drone.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub item_id: i64,
    pub type_id: i64,
    pub group_id: i64,
    pub hurt: Hurt,
    pub max_health: f64,
    pub unit_cost: f64,
    pub is_module: bool,
    pub is_ship: bool,
}

impl Line {
    /// What the client charges for the line: `ceil(damage) × unit`.
    pub fn cost(&self) -> f64 {
        self.hurt.total().ceil() * self.unit_cost
    }
}

fn line(reg: &Registry, item_id: i64, h: Hurt) -> Option<Line> {
    let item = reg.item(item_id)?;
    let is_ship = item.category_id == crate::model::CATEGORY_SHIP;
    let max_health = if is_ship {
        let profile = crate::dogma::profile(reg, item_id).unwrap_or_else(|| {
            crate::damage::Profile::from_hull_in(
                reg.content(),
                crate::destiny::hull::of_in(reg.content(), item.type_id),
            )
        });
        profile.armor_hp + profile.hp
    } else {
        crate::overheat::module_hp(reg, item.type_id)
    };
    let price = crate::econ_prices::p_ref(reg, item.type_id).unwrap_or(0.0);
    let unit_cost = if max_health > 0.0 {
        COST_MULTIPLIER * price / max_health
    } else {
        0.0
    };
    Some(Line {
        item_id,
        type_id: item.type_id,
        group_id: item.group_id,
        hurt: h,
        max_health,
        unit_cost,
        is_module: item.category_id == CATEGORY_MODULE || item.category_id == CATEGORY_DRONE,
        is_ship,
    })
}

/// The lines a request for `item_id` gets: the item itself when it is the
/// actor's, loose in `station`'s hangar and damaged, and for a ship each
/// damaged module or drone inside it. Undamaged items give no line (the
/// client skips `ceil(damage) == 0` anyway).
pub fn lines_for(reg: &Registry, actor: i64, station: i64, item_id: i64) -> Vec<Line> {
    let Some(item) = reg.item(item_id) else {
        return Vec::new();
    };
    if item.owner_id != actor {
        return Vec::new();
    }
    let in_hangar = |i: &crate::item::Item| {
        i.owner_id == actor && i.location_id == station && i.flag_id == crate::model::FLAG_HANGAR
    };
    let own = || {
        hurt(reg, item_id)
            .and_then(|h| line(reg, item_id, h))
            .into_iter()
            .collect::<Vec<Line>>()
    };
    if in_hangar(item) {
        let mut out = own();
        if item.category_id == crate::model::CATEGORY_SHIP {
            for inner in reg.store().contents_of(item_id) {
                if let Some(l) = hurt(reg, inner.item_id).and_then(|h| line(reg, inner.item_id, h))
                {
                    out.push(l);
                }
            }
        }
        return out;
    }
    // A module or drone named by its own id, as the window's rows do: it
    // counts when its ship sits in the hangar.
    if reg
        .item(item.location_id)
        .is_some_and(|ship| ship.category_id == crate::model::CATEGORY_SHIP && in_hangar(ship))
    {
        return own();
    }
    Vec::new()
}

/// Why a repair was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    NotDocked,
    /// A structure that does not offer the repair service to this pilot.
    NoService(&'static str),
    /// A module is in the set and the payment is under the full price.
    PartialWithModule,
    Funds {
        required: f64,
        balance: f64,
    },
}

/// What a repair did.
#[derive(Debug, Clone, PartialEq)]
pub struct Repaired {
    pub spent: f64,
    pub total: f64,
}

/// **`RepairItemsInStation`**: recompute the quote for `ids`, and mend what
/// `payment` pays for. A module in the set needs the full price; otherwise
/// `min(payment, total)` is spent on the ships in id order, armour first.
pub fn repair_items(
    reg: &mut Registry,
    actor: i64,
    ids: &[i64],
    payment: f64,
) -> Result<Repaired, Refusal> {
    repair_core(reg, actor, ids, Some(payment))
}

/// **`RepairItemsInStructure(itemIDs)`**: a structure's window sends no
/// payment (`base_repairshop.RepairItems`), so the whole quote is paid
/// **[A]** (a structure's own fee on top is not modelled; falsifier: a
/// retail quote that differs between a station and a structure).
pub fn repair_all(reg: &mut Registry, actor: i64, ids: &[i64]) -> Result<Repaired, Refusal> {
    repair_core(reg, actor, ids, None)
}

/// [`repair_items`] with `payment` `None` meaning "all of it".
fn repair_core(
    reg: &mut Registry,
    actor: i64,
    ids: &[i64],
    payment: Option<f64>,
) -> Result<Repaired, Refusal> {
    let station = reg.docked_station(actor).ok_or(Refusal::NotDocked)?;
    if let Some(why) =
        crate::structure_dock::service_refusal(reg, actor, crate::structure_profile::SERVICE_REPAIR)
    {
        return Err(Refusal::NoService(why));
    }
    prune(reg);
    let mut ids: Vec<i64> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut lines: Vec<Line> = Vec::new();
    for id in ids {
        for l in lines_for(reg, actor, station, id) {
            if !lines.iter().any(|x| x.item_id == l.item_id) {
                lines.push(l);
            }
        }
    }
    let total: f64 = lines.iter().map(Line::cost).sum();
    let payment = payment.unwrap_or(total);
    let payment = if payment.is_finite() {
        payment.max(0.0)
    } else {
        0.0
    };
    let full = payment >= total * (1.0 - 1e-9) - 1e-9;
    if lines.iter().any(|l| l.is_module) && !full {
        return Err(Refusal::PartialWithModule);
    }
    let spent = payment.min(total);
    let (balance, station_owner) = reg
        .character(actor)
        .map(|c| (c.balance, c.station_owner_id))
        .ok_or(Refusal::NotDocked)?;
    if balance < spent {
        return Err(Refusal::Funds {
            required: spent,
            balance,
        });
    }
    let now = crate::pyutil::filetime_now();
    let active = reg.character(actor).map_or(0, |c| c.ship_id);
    let mut left = spent;
    for l in &lines {
        let cost = l.cost();
        let before = l.hurt;
        let after = if full || left >= cost - 1e-9 {
            left = (left - cost).max(0.0);
            Hurt::default()
        } else {
            let points = if l.unit_cost > 0.0 {
                left / l.unit_cost
            } else {
                0.0
            };
            left = 0.0;
            let armor = (before.armor - points).max(0.0);
            let rest = (points - before.armor).max(0.0);
            Hurt {
                armor,
                structure: (before.structure - rest).max(0.0),
            }
        };
        put(reg, l.item_id, after);
        if l.is_ship && l.item_id == active {
            for (attr, new, old) in [
                (crate::damage::attr::ARMOR_DAMAGE, after.armor, before.armor),
                (
                    crate::damage::attr::DAMAGE,
                    after.structure,
                    before.structure,
                ),
            ] {
                tell(reg, actor, l.item_id, attr, new, old, now);
            }
        } else if l.is_module {
            tell(
                reg,
                actor,
                l.item_id,
                crate::damage::attr::DAMAGE,
                0.0,
                before.structure,
                now,
            );
        }
    }
    if spent > 0.0 {
        let item = lines.first().map_or(0, |l| l.item_id);
        reg.charge(actor, spent, REF_REPAIR_BILL, station_owner, item);
    }
    Ok(Repaired { spent, total })
}

/// `OnModuleAttributeChange` to the pilot, so a docked fitting window updates.
fn tell(reg: &mut Registry, actor: i64, item: i64, attr: i64, new: f64, old: f64, now: i64) {
    reg.push(crate::modules::AttributeChange {
        to: vec![actor],
        owner_id: actor,
        item_id: item,
        attribute_id: attr,
        time: now,
        new_value: new,
        old_value: old,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::{Profile, ShipDamage};
    use eve_marshal::PyValue;

    const ALICE: i64 = 90_000_101;
    const ALICE_SHIP: i64 = 100_000_101;
    const RIFTER: i64 = 587;
    /// An invented hull price; a test asserts code against itself, never it.
    const PRICE: f64 = 100_000.0;
    /// Invented module item ids, put in the ship by the test.
    const MOD_A: i64 = 100_000_901;
    const SHIELD_EXT: i64 = 3841;

    fn world() -> Registry {
        let pilot = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ship_type_id: RIFTER,
            balance: 10.0 * PRICE,
            ..crate::model::Character::default()
        };
        let mut content = crate::content::Content::fixture();
        let prices = crate::econ_prices::Table::parse(&[
            format!("{RIFTER}\t{PRICE}\trecipe\t{}\t6\t25\t1\t1", PRICE / 2.0),
            format!("{SHIELD_EXT}\t2000\trecipe\t1000\t7\t38\t1\t1"),
        ])
        .expect("parses");
        content.insert("econ_prices", prices);
        let mut reg = crate::testkit::registry_with(|c| c.characters = vec![pilot])
            .with_content(std::sync::Arc::new(content));
        let rifter = *crate::item::find_in(reg.content(), RIFTER).expect("Rifter is spawnable");
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = rifter.type_id;
            i.group_id = rifter.group_id;
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: ALICE_SHIP + 1,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        reg
    }

    /// A module item inside the ship, as a fitted one.
    fn fit_module(reg: &mut Registry) {
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: MOD_A,
            type_id: SHIELD_EXT,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: 27,
            quantity: 1,
            group_id: 38,
            category_id: 7,
            singleton: true,
        });
        let _ = station;
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: MOD_A + 1,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
    }

    fn profile(reg: &Registry) -> Profile {
        crate::damage::Profile::from_hull_in(
            reg.content(),
            crate::destiny::hull::of_in(reg.content(), RIFTER),
        )
    }

    /// Alice flies, is hurt for `armor` and `hull`, and docks.
    fn hurt_and_docked(reg: &mut Registry, armor: f64, hull: f64) {
        crate::destiny::on_ballpark_bind(reg, Some(ALICE));
        let _ = reg.take_pending();
        let now = crate::pyutil::filetime_now();
        let p = profile(reg);
        reg.combat_mut()
            .set_ship(ALICE_SHIP, ShipDamage::with_damage(p, now, armor, hull));
        reg.dock_change(Some(ALICE)).expect("a dock change");
        let _ = reg.take_pending();
    }

    fn quote(reg: &mut Registry, ids: &[i64]) -> PyValue {
        let args = [PyValue::List(
            ids.iter().map(|i| PyValue::Int(*i)).collect(),
        )];
        crate::service::repair::dispatch(
            reg,
            &crate::service::Request {
                service: "repairSvc",
                method: "GetRepairQuotes",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("a quote")
    }

    fn rows_for(v: &PyValue, id: i64) -> usize {
        let PyValue::Dict(d) = v else {
            panic!("a dict")
        };
        d.iter()
            .find(|(_, k)| *k == PyValue::Int(id))
            .map(|(rows, _)| match rows {
                PyValue::List(l) => l.len(),
                _ => panic!("rows are a list"),
            })
            .expect("the id is answered")
    }

    fn pay(reg: &mut Registry, ids: &[i64], payment: f64) -> Result<Repaired, Refusal> {
        repair_items(reg, ALICE, ids, payment)
    }

    #[test]
    fn a_dock_keeps_the_damage_and_an_undock_seeds_it_back() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 300.0, 100.0);
        assert_eq!(
            hurt(&reg, ALICE_SHIP),
            Some(Hurt {
                armor: 300.0,
                structure: 100.0
            })
        );
        assert!(reg.combat().ship(ALICE_SHIP).is_none(), "no record docked");
        reg.undock_change(Some(ALICE)).expect("an undock");
        let rec = reg.combat().ship(ALICE_SHIP).expect("seeded");
        let now = crate::pyutil::filetime_now();
        let r = rec.rings(now);
        assert_eq!((r.armor_damage, r.damage), (300.0, 100.0));
        assert_eq!(r.shield_charge, r.shield_capacity, "a full shield");
        assert_eq!(hurt(&reg, ALICE_SHIP), None, "the record is the truth now");
    }

    #[test]
    fn an_undamaged_ship_stashes_nothing() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 0.0, 0.0);
        assert!(reg.deltas().of(KIND).is_empty());
    }

    #[test]
    fn a_quote_has_a_row_for_the_hull_and_one_for_a_damaged_module() {
        let mut reg = world();
        fit_module(&mut reg);
        reg.ext_mut::<crate::overheat::Heat>()
            .set_damage(MOD_A, 20.0);
        hurt_and_docked(&mut reg, 300.0, 100.0);
        assert_eq!(hurt(&reg, MOD_A).map(|h| h.structure), Some(20.0));
        let v = quote(&mut reg, &[ALICE_SHIP]);
        assert_eq!(rows_for(&v, ALICE_SHIP), 2);
        let station = reg.docked_station(ALICE).expect("docked");
        let lines = lines_for(&reg, ALICE, station, ALICE_SHIP);
        let p = profile(&reg);
        let hull = &lines[0];
        assert_eq!(hull.max_health, p.armor_hp + p.hp);
        assert_eq!(hull.hurt.total(), 400.0);
        assert!((hull.unit_cost * hull.max_health - COST_MULTIPLIER * PRICE).abs() < 1e-6);
    }

    #[test]
    fn a_full_payment_clears_the_rows_and_books_ref_15() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 300.0, 100.0);
        let station = reg.docked_station(ALICE).expect("docked");
        let total: f64 = lines_for(&reg, ALICE, station, ALICE_SHIP)
            .iter()
            .map(Line::cost)
            .sum();
        assert!(total > 0.0);
        let before = reg.character(ALICE).expect("alice").balance;
        let done = pay(&mut reg, &[ALICE_SHIP], total).expect("repaired");
        assert_eq!(done.spent, total);
        assert_eq!(reg.character(ALICE).expect("alice").balance, before - total);
        assert_eq!(hurt(&reg, ALICE_SHIP), None);
        let row = reg
            .journal()
            .entries()
            .iter()
            .find(|r| r.entry_type_id == REF_REPAIR_BILL)
            .expect("a ref 15 row");
        assert_eq!(row.amount, -total);
    }

    #[test]
    fn a_partial_payment_on_a_hull_repairs_part_armour_first() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 300.0, 100.0);
        let station = reg.docked_station(ALICE).expect("docked");
        let unit = lines_for(&reg, ALICE, station, ALICE_SHIP)[0].unit_cost;
        let before = reg.character(ALICE).expect("alice").balance;
        let payment = 100.0 * unit;
        pay(&mut reg, &[ALICE_SHIP], payment).expect("repaired");
        let h = hurt(&reg, ALICE_SHIP).expect("still hurt");
        assert!((h.armor - 200.0).abs() < 1e-6, "{h:?}");
        assert_eq!(h.structure, 100.0);
        let after = reg.character(ALICE).expect("alice").balance;
        assert!((before - after - payment).abs() < 1e-9);
    }

    #[test]
    fn a_partial_payment_with_a_module_is_refused_and_costs_nothing() {
        let mut reg = world();
        fit_module(&mut reg);
        reg.ext_mut::<crate::overheat::Heat>()
            .set_damage(MOD_A, 20.0);
        hurt_and_docked(&mut reg, 300.0, 100.0);
        let before = reg.character(ALICE).expect("alice").balance;
        let r = pay(&mut reg, &[ALICE_SHIP], 1.0);
        assert_eq!(r, Err(Refusal::PartialWithModule));
        assert_eq!(reg.character(ALICE).expect("alice").balance, before);
        assert!(hurt(&reg, ALICE_SHIP).is_some() && hurt(&reg, MOD_A).is_some());
    }

    #[test]
    fn short_funds_raise_and_change_nothing() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 300.0, 100.0);
        reg.character_mut(ALICE).expect("alice").balance = 0.5;
        let station = reg.docked_station(ALICE).expect("docked");
        let total: f64 = lines_for(&reg, ALICE, station, ALICE_SHIP)
            .iter()
            .map(Line::cost)
            .sum();
        let r = pay(&mut reg, &[ALICE_SHIP], total);
        assert!(matches!(r, Err(Refusal::Funds { .. })), "{r:?}");
        assert_eq!(reg.character(ALICE).expect("alice").balance, 0.5);
        assert_eq!(hurt(&reg, ALICE_SHIP).map(|h| h.total()), Some(400.0));
    }

    #[test]
    fn a_damaged_ship_cannot_be_repackaged_and_a_destroyed_one_has_no_row() {
        let mut reg = world();
        hurt_and_docked(&mut reg, 300.0, 100.0);
        assert!(is_damaged(&reg, ALICE_SHIP));
        reg.apply_items(&[crate::item::Edit::Destroy {
            item_id: ALICE_SHIP,
        }])
        .expect("destroyed");
        assert_eq!(hurt(&reg, ALICE_SHIP), None);
        prune(&mut reg);
        assert!(reg.deltas().of(KIND).is_empty());
    }
}
