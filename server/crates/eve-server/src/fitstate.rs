//! The ship's fitting state as the **client's godma reads it**: CPU and
//! powergrid output and load, calibration, and the free hardpoints and rig
//! slots (RIG-CAL).
//!
//! ## Why this exists [V]
//!
//! The fitting window's gauges (`fittingCenter.UpdateCalibrationGauge`, the
//! CPU and powergrid ones, the turret and launcher counters) read the active
//! ship through `fittingUtil.GetShipAttributeWithDogmaLocation`, which for
//! `session.shipid` is `godma.GetItem(shipID)` and then `getattr(ship, name)`.
//! godma answers from the attribute dict `dogmaIM.ItemGetInfo` /
//! `GetAllInfo` gave and from `OnModuleAttributeChange` pushes, and an
//! attribute in neither is the **definition's default, 0**
//! (`godma.StateManager.GetAttribute`). The ship's dict (`service.rs`
//! `ship_attributes_of`) carried the HUD's twenty and none of these, so the
//! server enforced calibration (`rigs.rs`) while the gauge read 0 / 0 for
//! ever. The client's own dogma (`UpgradeFittedEffect` adding `upgradeCost`
//! to `upgradeLoad`) is a second, separate state that these gauges do not
//! read when the ship is the active one.
//!
//! ## What is served
//!
//! `cpuOutput` 48, `powerOutput` 11, `cpuLoad` 49, `powerLoad` 15,
//! `upgradeCapacity` 1132, `upgradeLoad` 1152, `upgradeSlotsLeft` 1154,
//! `turretSlotsLeft` 102, `launcherSlotsLeft` 101: in the ship's row, and as
//! `OnModuleAttributeChange` (ownerID = the pilot, itemKey = the ship, the
//! shape the capacitor uses) whenever a call that can move one finishes
//! ([`around`]). godma's `ApplyAttributeChange` scatters `OnAttributes`,
//! which is what makes the open fitting window redraw
//! (`fittingController.OnAttributes`).
//!
//! The **twelve resonances** (shield 271-274, armor 267-270, structure
//! 113/111/109/110) are served the same way ([`defense`]): the defense panel
//! (`fittingPanels/defensePanel.py`) reads them through the same godma path,
//! and an attribute godma does not hold is the definition's default 1.0,
//! which the panel shows as 0%. They are in the prime, in the fit diffs, and
//! `modules.rs` pushes them when a hardener starts or stops.
//!
//! The ship's **sheet attributes** ([`sheet_attributes`]: capacity and the
//! special holds, drone bay and bandwidth, signature, sensors, targeting,
//! mass, agility, warp speed, the HP maxima) ride the same prime and diffs
//! (SHIP-SHEET): `invCache.GetCapacity` reads the active ship's holds from
//! godma, and an attribute it lacks is 0.0 m3.
//!
//! **[A]** that the gauges want the modified outputs (CPU Management and the
//! like) and the loads as `crate::dogma` sums them; falsified by a gauge that
//! disagrees with the refusal text of `NotEnoughCpu`.

use crate::modules::AttributeChange;
use crate::service::Registry;
use std::collections::BTreeMap;

/// `upgradeCapacity`: calibration.
pub const ATTR_UPGRADE_CAPACITY: i64 = crate::rigs::ATTR_CALIBRATION;
/// `upgradeLoad`: calibration in use.
pub const ATTR_UPGRADE_LOAD: i64 = 1152;
/// `upgradeSlotsLeft`: free rig slots.
pub const ATTR_UPGRADE_SLOTS_LEFT: i64 = 1154;

/// The twelve resonances of `ship_id`, ascending by id: the sheet's
/// (modified: fitted and running modules, skills, buffs), else the hull
/// type's own, else 1.0 (the attribute definition's default, so nothing to
/// say). **[A]** godma wants the modified values, as the HUD gets the fitted
/// maxima; falsified by a fitting window that disagrees with "Simulate
/// fitting" on the same fit.
pub fn defense(w: &Registry, ship_id: i64) -> Vec<(i64, f64)> {
    use crate::damage::{Damage, attr as d};
    let Some(ship) = w.item(ship_id) else {
        return Vec::new();
    };
    let layers: [Damage; 3] = match crate::dogma::profile(w, ship_id) {
        Some(p) => [p.shield_resonance, p.armor_resonance, p.hull_resonance],
        None => crate::typedata::resonances_in(w.content(), ship.type_id).unwrap_or(
            [Damage {
                em: 1.0,
                explosive: 1.0,
                kinetic: 1.0,
                thermal: 1.0,
            }; 3],
        ),
    };
    let ids = [d::SHIELD_RESONANCE, d::ARMOR_RESONANCE, d::HULL_RESONANCE];
    let mut out = Vec::with_capacity(12);
    for (layer, ids) in layers.iter().zip(ids) {
        out.push((ids[0], layer.em));
        out.push((ids[1], layer.explosive));
        out.push((ids[2], layer.kinetic));
        out.push((ids[3], layer.thermal));
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// The ship attributes a running module can move besides the resonances:
/// lock range 76, scan resolution 564, signature radius 552 and the four
/// sensor strengths 208-211. A Sensor Booster's own effect (2670,
/// `sensorBoosterActivePercentage`) is `modifierInfo` on the ship with no
/// charge in it [V-data: `out/content/types/typedata.tsv` modifier rows], so
/// it moves 76 and 564 bare; unpushed, godma kept the hull's lock range
/// (fci_script_empty, regress-w23a).
pub const SENSOR_ATTRIBUTES: &[i64] = &[76, 208, 209, 210, 211, 552, 564];

/// [`defense`] plus [`SENSOR_ATTRIBUTES`] of `ship_id`, ascending by id: the
/// numbers a module starting or ending can move, as the pilot's godma should
/// hold them. A sensor attribute is the sheet's modified value times the
/// projected factor `crate::ewar` last told the pilot, so this push and a
/// remote dampener's agree; with no sheet it is left out.
pub fn live(w: &Registry, ship_id: i64) -> Vec<(i64, f64)> {
    let mut out: BTreeMap<i64, f64> = defense(w, ship_id).into_iter().collect();
    if let Some(sheet) = crate::dogma::sheet(w, ship_id) {
        for &id in SENSOR_ATTRIBUTES {
            if let Some(v) = sheet.ship_opt(id) {
                out.insert(id, v * crate::ewar::told_factor_on(w, ship_id, id));
            }
        }
    }
    out.into_iter().collect()
}

/// The ship attributes godma and the fitting window read that the base row
/// (`service::ship_attributes_of`) leaves out, or that a fit can move, served
/// from the sheet's **modified** values (SHIP-SHEET). Never a charged or
/// current-state attribute (shield charge 264, capacitor charge 18, damage
/// 3 and 266): those have their own owners. `maxVelocity` 37 is not here
/// either: `modules::speed_changed` pushes it.
///
/// * 38 `capacity` and every special-hold capacity the client's
///   `inventoryFlagData` names (`invCache.GetCapacity` for the active ship
///   reads `godma.GetItem(ship).GetCapacity(flag)`, so a missing one is a
///   hold of 0.0 m3);
/// * 283 `droneCapacity`, 1271 `droneBandwidth`;
/// * 552 `signatureRadius`, 208-211 the four sensor strengths, 76, 564, 192;
/// * 4 `mass`, 70 `agility`, 600 and 1281 (warp speed);
/// * 9, 263, 265 the HP maxima, so a docked fit change reaches the HUD.
pub const SHEET_ATTRIBUTES: &[i64] = &[
    4, 9, 38, 70, 76, 192, 208, 209, 210, 211, 263, 265, 283, 552, 564, 600, 908, 912, 1271, 1281,
    1549, 1556, 1557, 1558, 1559, 1560, 1561, 1562, 1563, 1564, 1573, 1646, 1653, 1770, 1804, 2055,
    2467, 2657, 2675, 3020, 3136, 3227, 5325, 5646, 5693, 5944,
];

/// [`SHEET_ATTRIBUTES`] of `ship_id`: the sheet's modified value, else the
/// hull type's own row; an attribute neither names is left out (godma's
/// default is 0 for it too). **[A]** godma wants the modified values, as the
/// fitting window's Simulate fitting shows them; falsified by the two
/// disagreeing on the same fit.
pub fn sheet_attributes(w: &Registry, ship_id: i64) -> Vec<(i64, f64)> {
    let Some(ship) = w.item(ship_id) else {
        return Vec::new();
    };
    let sheet = crate::dogma::sheet(w, ship_id);
    SHEET_ATTRIBUTES
        .iter()
        .filter_map(|&id| {
            let v = match &sheet {
                Some(s) => s.ship_opt(id),
                None => crate::typedata::attribute_in(w.content(), ship.type_id, id),
            };
            v.map(|v| (id, v))
        })
        .collect()
}

/// Every fitting-state attribute of `ship_id` and its resonances, ascending
/// by id. Only the resonances when the server has no fitting numbers for the
/// hull; empty when there is no such item.
pub fn attributes(w: &Registry, ship_id: i64) -> Vec<(i64, f64)> {
    use crate::dogma::attr;
    let Some(ship) = w.item(ship_id) else {
        return Vec::new();
    };
    let mut out: BTreeMap<i64, f64> = defense(w, ship_id).into_iter().collect();
    out.extend(sheet_attributes(w, ship_id));
    let Some(hull) = crate::fitting::hull_of_ship(w.content(), w.store(), ship_id) else {
        return out.into_iter().collect();
    };
    let sheet = crate::dogma::sheet(w, ship_id);
    let inputs = crate::dogma::inputs(w, ship_id);
    let (cpu_out, power_out) = match &sheet {
        Some(s) => (s.ship(attr::CPU_OUTPUT), s.ship(attr::POWER_OUTPUT)),
        None => (hull.cpu_output, hull.power_output),
    };
    let (cpu_load, power_load) = match (&sheet, &inputs) {
        (Some(s), Some(i)) => s.load(i, None),
        _ => unmodified_load(w, ship_id),
    };
    let fitted: Vec<&crate::item::Item> = w
        .store()
        .contents_of(ship_id)
        .filter(|i| {
            crate::item::is_ship_fitting_flag(i.flag_id) && !crate::item::is_loaded_charge(i)
        })
        .collect();
    let used = |hp: crate::fitting::Hardpoint| {
        fitted
            .iter()
            .filter(|i| {
                !crate::rigs::is_rig_flag(i.flag_id)
                    && crate::modules::find_in(w.content(), i.type_id).and_then(|t| t.hardpoint)
                        == Some(hp)
            })
            .count() as i64
    };
    let rigs = fitted
        .iter()
        .filter(|i| crate::rigs::is_rig_flag(i.flag_id))
        .count() as i64;
    let rig_hull = crate::rigs::hull_rigs_in(w.content(), ship.type_id);
    out.insert(attr::POWER_OUTPUT, power_out);
    out.insert(attr::POWER_LOAD, power_load);
    out.insert(attr::CPU_OUTPUT, cpu_out);
    out.insert(attr::CPU_LOAD, cpu_load);
    out.insert(
        attr::LAUNCHER_SLOTS,
        (hull.launchers - used(crate::fitting::Hardpoint::Launcher)).max(0) as f64,
    );
    out.insert(
        attr::TURRET_SLOTS,
        (hull.turrets - used(crate::fitting::Hardpoint::Turret)).max(0) as f64,
    );
    out.insert(
        ATTR_UPGRADE_CAPACITY,
        rig_hull.map_or(0.0, |h| h.calibration),
    );
    out.insert(
        ATTR_UPGRADE_LOAD,
        crate::rigs::calibration_used(w.content(), w.store(), ship_id),
    );
    out.insert(ATTR_UPGRADE_SLOTS_LEFT, (hull.rig - rigs).max(0) as f64);
    out.into_iter().collect()
}

/// The loads from the modules' own `cpu` / `power`, for a hull `crate::dogma`
/// has no sheet for: the sum over the online fitted modules.
fn unmodified_load(w: &Registry, ship_id: i64) -> (f64, f64) {
    let (mut cpu, mut power) = (0.0, 0.0);
    for i in w.store().contents_of(ship_id).filter(|i| {
        i.singleton
            && crate::item::is_ship_fitting_flag(i.flag_id)
            && w.modules().is_online(i.item_id)
    }) {
        if let Some(t) = crate::modules::find_in(w.content(), i.type_id) {
            cpu += t.cpu();
            power += t.power();
        }
    }
    (cpu, power)
}

/// Run `call`, then tell the pilot's godma every fitting-state attribute of
/// their active ship that the call moved. For the calls that fit, unfit,
/// destroy or (off)line a module.
pub fn around<T>(w: &mut Registry, actor: Option<i64>, call: impl FnOnce(&mut Registry) -> T) -> T {
    let before = actor.map(|a| snapshot(w, a));
    let out = call(w);
    if let (Some(a), Some(before)) = (actor, before) {
        sync(w, a, &before);
    }
    out
}

/// The active ship of `character_id` and its [`attributes`].
pub fn snapshot(w: &Registry, character_id: i64) -> (i64, Vec<(i64, f64)>) {
    match w.character(character_id) {
        Some(c) => (c.ship_id, attributes(w, c.ship_id)),
        None => (0, Vec::new()),
    }
}

/// Push each attribute that differs from `before` (an attribute `before` did
/// not hold counts as 0, godma's default). A different ship than before is
/// not a change of this ship: the boarding sends the new ship whole.
pub fn sync(w: &mut Registry, character_id: i64, before: &(i64, Vec<(i64, f64)>)) {
    let Some(c) = w.character(character_id) else {
        return;
    };
    let ship_id = c.ship_id;
    if ship_id != before.0 {
        return;
    }
    let old: BTreeMap<i64, f64> = before.1.iter().copied().collect();
    let now = crate::pyutil::filetime_now();
    let mut resisted = false;
    for (id, new) in attributes(w, ship_id) {
        let was = old.get(&id).copied().unwrap_or(0.0);
        if (new - was).abs() < 1e-9 {
            continue;
        }
        resisted |= crate::damage::attr::is_resonance(id);
        w.push(AttributeChange {
            to: vec![character_id],
            owner_id: character_id,
            item_id: ship_id,
            attribute_id: id,
            time: now,
            new_value: new,
            old_value: was,
        });
    }
    // A moved resist moves the combat record too (it was read at the first
    // hit and would keep the old one); a no-op for a ship nothing has hit.
    if resisted {
        crate::dbuff::refresh_record(w, ship_id, character_id, now);
    }
}
