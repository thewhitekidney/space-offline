//! **Drone skills, hull bonuses, bay limits and damage application**
//! (K-DRONE). The numbers a launched drone flies with, and the roll its
//! volleys make. `drones.rs` owns the drone; this owns what the pilot and
//! the hull do to it.
//!
//! # Where a drone's numbers come from [V-data]
//!
//! A drone is a **category-18 type**, and every bonus a pilot gets for it
//! is an ordinary dogma modifier aimed at "everything the character owns
//! that requires skill S" (`OwnerRequiredSkillModifier`, `crate::dogma`
//! rule 2):
//!
//! - **Drone Interfacing** (3442), effect 6663: `damageMultiplier` (64) of
//!   every drone that requires Drones (3436), post-percent by the skill's
//!   `damageMultiplierBonus` (292) -- 10 per level, because the skill's own
//!   effect 146 multiplies 292 by its level.
//! - **Drone Sharpshooting** (23606), effect 6664: `maxRange` (54) by 294.
//! - **Drone Navigation** (12305), effect 6667: `maxVelocity` (37) by 2603.
//! - **Drone Durability** (23618), effect 6665: shield, armour and hull.
//! - **A hull's drone bonus** (Vexor's `shipBonusDroneDamageMultiplierGC2`
//!   2188, `shipBonusDroneHitpointsGC2` 2179): the same owner modifier,
//!   sourced from the **ship**, whose bonus attribute the hull-skill's
//!   `...SkillLevelPreMulShipBonus...` effect already multiplied by level.
//! - **Drone Damage Amplifier** modules (effect 6556), the same shape from
//!   a fitted module.
//!
//! [`crate::dogma`] already evaluates all of that for a ship's own modules
//! and charges, and a **charge** is the one item it lets the owner's
//! modifiers reach (`IsOwnerModifiable`, `chargeDogmaItem:21-36`). So
//! [`stats`] asks it for the drone as a charge in a **phantom module** of
//! the pilot's real ship: the real skills, the real hull, the real fitted
//! modules, stacking penalties and all. One thing the engine does not
//! know is **effect 1730 `droneDmgBonus`** -- Light, Medium, Heavy and
//! the racial specialisation skills: a **Python effect** whose
//! `Start` registers `AddOwnerRequiredSkillModifier(operation =
//! dgmAssPostPercent, charID, affectedSkillID = env.itemTypeID, modified =
//! damageMultiplier (64), modifying = damageMultiplierBonus (292))`
//! (`pythonEffects/migratedeffects.pyc`, class `DroneDmgBonus`) [V]: a
//! skill's bonus reaches the drones that **require that skill**. [`stats`]
//! applies it after the engine, which is exact: it is a post-percent from a
//! skill, which is never stacking-penalised, so the order does not matter.
//!
//! # Hitting [V-data] / [A]
//!
//! A combat drone carries the turret's three application attributes:
//! `trackingSpeed` 160 (1.815 rad/s on a Hobgoblin I), `falloff` 158 and
//! `optimalSigRadius` 620 (25 m). [`chance_to_hit`] is the turret formula
//! on them. `[A]` that CCP's server uses it for drones with the drone's own
//! motion in the angular velocity (the client has no say: the combat log
//! only shows the verdict); falsified by a retail log in which drones
//! orbiting a frigate never miss. A drone with **no** tracking attribute
//! (`trackingSpeed` 0) always hits for the full amount, as every shot of
//! this server's turrets does.

use crate::damage::Damage;
use crate::dogma::{self, Inputs, ModuleInput, State};
use crate::drones::{DroneType, SKILL_DRONES};
use crate::service::Registry;
use crate::typedata;

/// `falloff` (158) [V-data].
pub const ATTR_FALLOFF: i64 = 158;
/// `trackingSpeed` (160) [V-data].
pub const ATTR_TRACKING: i64 = 160;
/// `optimalSigRadius` (620) [V-data]: the signature a gun is built to hit.
pub const ATTR_SIG_RESOLUTION: i64 = 620;
/// `droneControlDistance` (458) bonus per level / per module (459).
pub const ATTR_CONTROL_BONUS: i64 = 459;
/// `damageMultiplierBonus` (292), the bonus effect 1730 reads.
pub const ATTR_DAMAGE_BONUS: i64 = 292;
/// `droneIsAgressive` (1275, CCP's spelling): the aggressive/passive
/// setting; default 1 [V-data].
pub const ATTR_AGGRESSIVE: i64 = 1275;
/// `droneFocusFire` (1297): default 0 [V-data].
pub const ATTR_FOCUS_FIRE: i64 = 1297;
/// `droneDmgBonus` (1730), a Python effect [V].
pub const EFFECT_DRONE_DMG_BONUS: i64 = 1730;
/// `droneControlDistance` module effect (Drone Link Augmentor) [V-data].
pub const EFFECT_LINK_AUGMENTOR: i64 = 2000;
/// The phantom module's item id: no real item has it.
const PHANTOM: i64 = i64::MAX;

/// The required-skill level attributes, in the order of
/// [`dogma::attr::REQUIRED_SKILLS`] [V-data].
const REQUIRED_LEVELS: [i64; 6] = [277, 278, 279, 1286, 1287, 1288];

/// Every `(skill, level)` a type requires.
pub fn required_skills(table: &typedata::Table, type_id: i64) -> Vec<(i64, i64)> {
    let Some(row) = table.get(type_id) else {
        return Vec::new();
    };
    dogma::attr::REQUIRED_SKILLS
        .iter()
        .zip(REQUIRED_LEVELS)
        .filter_map(|(skill, level)| {
            let s = row.attribute(*skill).filter(|s| *s > 0.0)?;
            Some((
                s as i64,
                row.attribute(level).unwrap_or(1.0).max(1.0) as i64,
            ))
        })
        .collect()
}

/// A pilot's trained level of `skill`; 0 when not injected.
pub fn level_of(w: &Registry, owner: i64, skill: i64) -> i64 {
    w.skills_for(Some(owner))
        .and_then(|s| s.get(skill))
        .map_or(0, |e| e.level)
}

/// The first skill a pilot lacks to fly `type_id`: `(skill, level needed)`.
/// `[A]` the launch is refused for it; the client's own check lives in the
/// fitting window, not in `LaunchFromShip`.
pub fn missing_skill(w: &Registry, owner: i64, type_id: i64) -> Option<(i64, i64)> {
    let table = typedata::installed_in(w.content()).table();
    required_skills(table, type_id)
        .into_iter()
        .find(|(skill, level)| level_of(w, owner, *skill) < *level)
}

/// The drones this pilot may have out at once: the Drones skill's level
/// (`maxActiveDroneBonus` 1 per level, effects 315 and 316 [V-data]).
pub fn max_active(w: &Registry, owner: i64) -> i64 {
    level_of(w, owner, SKILL_DRONES)
}

/// The hull's drone bay and bandwidth **as fitted**: a rig or a module
/// that changes attributes 283 or 1271 shows (the sheet's), else the
/// hull's own.
pub fn bay_of(w: &Registry, ship_id: i64) -> Option<crate::drones::Bay> {
    let ship = w.store().get(ship_id)?;
    let base = crate::drones::bay_in(w.content(), ship.type_id)?;
    Some(match dogma::sheet(w, ship_id) {
        Some(s) => crate::drones::Bay {
            ship_type_id: base.ship_type_id,
            capacity: s.ship(typedata::attr::DRONE_CAPACITY),
            bandwidth: s.ship(typedata::attr::DRONE_BANDWIDTH),
        },
        None => base,
    })
}

/// How far from its ship a pilot may command a drone, metres: the
/// character's 20 km, plus **Drone Avionics** and **Advanced Drone
/// Avionics** per level (attribute 459 of the skill, effects 448 and 504)
/// and each fitted, online **Drone Link Augmentor** (459 of the module,
/// effect 2000), which add without a stacking penalty [V-data].
pub fn control_range(w: &Registry, owner: i64, ship_id: i64) -> f64 {
    let table = typedata::installed_in(w.content()).table();
    let mut range = crate::drones::CONTROL_RANGE;
    if let Some(skills) = w.skills_for(Some(owner)) {
        for e in skills.entries() {
            if let Some(row) = table.get(e.type_id)
                && row.has_effect(504)
            {
                range += row.attribute(ATTR_CONTROL_BONUS).unwrap_or(0.0) * e.level as f64;
            }
        }
    }
    if let Some(inputs) = dogma::inputs(w, ship_id)
        && let Some(sheet) = dogma::sheet(w, ship_id)
    {
        // The effect is an addition (`ItemModifier`, operation 2), which
        // the engine never stacking-penalises.
        range += inputs
            .modules
            .iter()
            .filter(|m| m.state >= State::Online)
            .filter(|m| {
                table
                    .get(m.type_id)
                    .is_some_and(|r| r.has_effect(EFFECT_LINK_AUGMENTOR))
            })
            .filter_map(|m| sheet.module(m.item_id, ATTR_CONTROL_BONUS))
            .sum::<f64>();
    }
    range
}

/// A drone's numbers for **this pilot in this ship**: [`DroneType`] with
/// the skills, the hull's drone bonuses and the fitted amplifiers applied
/// (module header). `base` is the type's own row ([`crate::drones::find_in`]).
/// Unchanged when the ship has no sheet.
pub fn stats(w: &Registry, ship_id: i64, base: &DroneType) -> DroneType {
    let table = typedata::installed_in(w.content()).table();
    let Some(mut inputs) = dogma::inputs(w, ship_id) else {
        return *base;
    };
    // Only ships have sheets; a hull the table lacks has none.
    if table
        .get(inputs.ship_type)
        .is_none_or(|r| r.category_id != typedata::CATEGORY_SHIP)
    {
        return *base;
    }
    phantom(&mut inputs, base.type_id);
    let Some(sheet) = dogma::evaluate(table, &inputs) else {
        return *base;
    };
    // The sheet answers 0 for an attribute nothing names; the type's own
    // value stands in for one the type does not carry.
    let own = table.get(base.type_id);
    let c = |attr: i64| {
        let v = sheet.charge(PHANTOM, attr)?;
        (v != 0.0 || own.is_some_and(|r| r.attribute(attr).is_some())).then_some(v)
    };
    let mut out = *base;
    let mining = crate::drones::is_miner(w.content(), base.type_id);

    // Effect 1730 (module header): the skill's own bonus, for the drones
    // that require that skill.
    let required = required_skills(table, base.type_id);
    let mut multiplier = c(typedata::attr::DAMAGE_MULTIPLIER).unwrap_or(base.damage_multiplier);
    for &(skill, level) in &inputs.skills {
        let Some(row) = table.get(skill) else {
            continue;
        };
        if row.has_effect(EFFECT_DRONE_DMG_BONUS) && required.iter().any(|(s, _)| *s == skill) {
            multiplier *=
                1.0 + row.attribute(ATTR_DAMAGE_BONUS).unwrap_or(0.0) * level as f64 / 100.0;
        }
    }
    out.damage_multiplier = multiplier;
    if let Some(v) = c(crate::drones::attr::MAX_RANGE) {
        out.optimal = v;
    }
    if let Some(v) = c(ATTR_FALLOFF) {
        out.falloff = v;
    }
    if let Some(v) = c(ATTR_TRACKING) {
        out.tracking = v;
    }
    if let Some(v) = c(ATTR_SIG_RESOLUTION) {
        out.sig_resolution = v;
    }
    if let Some(v) = c(crate::drones::attr::ENTITY_FLY_RANGE) {
        out.orbit_range = v;
    }
    if !mining && let Some(v) = c(crate::drones::attr::SPEED).filter(|v| *v > 0.0) {
        out.weapon_period_ms = v as i64;
    }
    if let Some(v) = c(typedata::attr::SHIELD_CAPACITY) {
        out.hull.shield_capacity = v;
    }
    if let Some(v) = c(typedata::attr::ARMOR_HP) {
        out.hull.armor_hp = v;
    }
    if let Some(v) = c(typedata::attr::HP) {
        out.hull.hp = v;
    }
    if let Some(v) = c(typedata::attr::SIGNATURE_RADIUS) {
        out.hull.signature_radius = v;
    }
    if let Some(v) = c(typedata::attr::MAX_VELOCITY).filter(|v| *v > 0.0) {
        // The cruise speed stays the same *speed* the type states, so the
        // fraction follows the new top speed (a faster drone orbits at the
        // same pace, not faster).
        if base.hull.max_velocity > 0.0 && base.cruise_fraction > 0.0 {
            out.cruise_fraction =
                (base.cruise_fraction * f64::from(base.hull.max_velocity) / v).min(1.0);
        }
        out.hull.max_velocity = v as f32;
    }
    out
}

/// Add the drone as a charge in a phantom, offline, type-less module. The
/// module has no row, so none of its effects start; the charge is the
/// drone's own type, so its passive effects do.
fn phantom(inputs: &mut Inputs, drone_type: i64) {
    inputs.modules.push(ModuleInput {
        item_id: PHANTOM,
        type_id: 0,
        state: State::Offline,
        charge_type: Some(drone_type),
    });
}

// ---------------------------------------------------------------------------
// Hitting
// ---------------------------------------------------------------------------

/// What a volley needs to know about the target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aim {
    /// Centre to centre, metres.
    pub distance: f64,
    /// The target's speed across the drone's line of sight, relative to the
    /// drone, metres per second.
    pub transversal: f64,
    /// The target's signature radius, metres.
    pub signature: f64,
}

/// The target's transversal speed relative to the shooter: the part of
/// `target velocity - shooter velocity` perpendicular to the line between
/// them. Zero when they are on top of each other.
pub fn transversal(from: [f64; 3], from_v: [f64; 3], to: [f64; 3], to_v: [f64; 3]) -> f64 {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-9 {
        return 0.0;
    }
    let u = [d[0] / len, d[1] / len, d[2] / len];
    let v = [
        to_v[0] - from_v[0],
        to_v[1] - from_v[1],
        to_v[2] - from_v[2],
    ];
    let along = v[0] * u[0] + v[1] * u[1] + v[2] * u[2];
    let p = [
        v[0] - along * u[0],
        v[1] - along * u[1],
        v[2] - along * u[2],
    ];
    (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt()
}

/// **The turret formula** on a drone's attributes:
/// `0.5 ^ ((angular * sigResolution / (tracking * signature))^2 +
/// (max(0, distance - optimal) / falloff)^2)`, `angular` in rad/s. 1.0 for a
/// drone with no `trackingSpeed` (module header); 0.0 past the optimal for
/// one with tracking but no falloff.
pub fn chance_to_hit(k: &DroneType, aim: Aim) -> f64 {
    hit_chance(k.optimal, k.falloff, k.tracking, k.sig_resolution, aim)
}

/// [`chance_to_hit`] on the four numbers alone, so a turret (DOC-0,
/// `combat::Application`) calls the very same function.
pub fn hit_chance(optimal: f64, falloff: f64, tracking: f64, sig_resolution: f64, aim: Aim) -> f64 {
    if tracking <= 0.0 {
        return 1.0;
    }
    let angular = aim.transversal / aim.distance.max(1.0);
    let sig = aim.signature.max(1.0);
    let track = angular * sig_resolution / (tracking * sig);
    let over = (aim.distance - optimal).max(0.0);
    let range = if over <= 0.0 {
        0.0
    } else if falloff <= 0.0 {
        return 0.0;
    } else {
        over / falloff
    };
    0.5f64.powf(track * track + range * range)
}

/// `[A]` the share of hits that are **wrecking**: three times the damage.
/// The turret rule is "a roll under 1%" [I]; falsified by a retail log in
/// which drones never wreck.
pub const WRECK_ROLL: f64 = 0.01;
/// `[A]` a wrecking hit's multiplier.
pub const WRECK_MULTIPLIER: f64 = 3.0;
/// `[A]` an ordinary hit's multiplier runs from this...
pub const HIT_FLOOR: f64 = 0.5;
/// ...for a roll of 0 to this plus one for a roll just under 1: a mean of
/// 1.0, so damage averages what the fitting window says times the chance.
pub const HIT_SPREAD: f64 = 1.0;

/// `hitQuality` labels: 0 miss; 1 barely scratches, 2 glances off, 3 hits,
/// 4 penetrates, 5 smashes, 6 wrecks (`UI/Inflight/HitQuality*`,
/// `crate::damage::Shot`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub quality: u8,
    /// Times the volley's damage: 0 on a miss.
    pub multiplier: f64,
}

/// One volley's verdict from its two rolls (each in `[0, 1)`): the first
/// against the chance to hit, the second for the quality.
pub fn verdict(chance: f64, hit_roll: f64, quality_roll: f64) -> Verdict {
    if hit_roll >= chance {
        return Verdict {
            quality: 0,
            multiplier: 0.0,
        };
    }
    if quality_roll < WRECK_ROLL {
        return Verdict {
            quality: 6,
            multiplier: WRECK_MULTIPLIER,
        };
    }
    let m = HIT_FLOOR + HIT_SPREAD * quality_roll;
    let quality = match m {
        m if m < 0.6 => 1,
        m if m < 0.8 => 2,
        m if m < 1.2 => 3,
        m if m < 1.4 => 4,
        _ => 5,
    };
    Verdict {
        quality,
        multiplier: m,
    }
}

/// A deterministic roll in `[0, 1)` from three numbers (splitmix64): the
/// same drone's same volley always rolls the same, so a test can replay it.
pub fn roll(drone_id: i64, at: i64, salt: u64) -> f64 {
    let mut z = (drone_id as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(at as u64)
        .wrapping_add(salt.wrapping_mul(0xD1B5_4A32_D192_ED03));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// `damage` scaled by `k`.
pub fn scaled(damage: Damage, k: f64) -> Damage {
    Damage {
        em: damage.em * k,
        explosive: damage.explosive * k,
        kinetic: damage.kinetic * k,
        thermal: damage.thermal * k,
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// A pilot's drone settings, sent by `dogmaIM.ChangeDroneSettings` before
/// every launch (`godma:2704-2708`) and when changed in the window (`:2722`)
/// [V]: `{attributeDroneIsAggressive: bool, attributeDroneFocusFire: bool}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// **Aggressive** (the default [V-data]): idle drones fight whatever is
    /// attacking their ship or them. **Passive**: they only do what they
    /// are told.
    pub aggressive: bool,
    /// **Focus fire**: idle drones agree on one target (the one nearest the
    /// ship) instead of each taking the nearest to itself.
    pub focus_fire: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            aggressive: true,
            focus_fire: false,
        }
    }
}

impl Settings {
    /// `self` with the entries of a `ChangeDroneSettings` dict applied:
    /// keys are the attribute ids [`ATTR_AGGRESSIVE`] and
    /// [`ATTR_FOCUS_FIRE`]; a value is a bool or an int.
    pub fn changed_by(self, arg: &eve_marshal::PyValue) -> Settings {
        use eve_marshal::PyValue;
        let mut out = self;
        let eve_marshal::PyValue::Dict(entries) = arg else {
            return out;
        };
        for (value, key) in entries {
            let on = match value {
                PyValue::Bool(b) => *b,
                PyValue::Int(n) => *n != 0,
                _ => continue,
            };
            match key {
                PyValue::Int(k) if *k == ATTR_AGGRESSIVE => out.aggressive = on,
                PyValue::Int(k) if *k == ATTR_FOCUS_FIRE => out.focus_fire = on,
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eve_marshal::PyValue;

    /// A drone with round invented numbers: tracking 2, optimal 1,000,
    /// falloff 500, resolution 100.
    fn k() -> DroneType {
        DroneType {
            tracking: 2.0,
            optimal: 1_000.0,
            falloff: 500.0,
            sig_resolution: 100.0,
            ..crate::drones::HOBGOBLIN_I
        }
    }

    fn aim(distance: f64, transversal: f64, signature: f64) -> Aim {
        Aim {
            distance,
            transversal,
            signature,
        }
    }

    #[test]
    fn a_drone_with_no_tracking_always_hits() {
        let none = DroneType {
            tracking: 0.0,
            ..k()
        };
        assert_eq!(chance_to_hit(&none, aim(1e6, 1e6, 1.0)), 1.0);
    }

    #[test]
    fn a_still_target_inside_optimal_is_hit_for_sure() {
        assert_eq!(chance_to_hit(&k(), aim(900.0, 0.0, 50.0)), 1.0);
    }

    /// One falloff past optimal halves the chance; the tracking term
    /// halves it at "angular * resolution = tracking * signature".
    #[test]
    fn falloff_and_tracking_each_halve_at_their_unit() {
        let c = chance_to_hit(&k(), aim(1_500.0, 0.0, 50.0));
        assert!((c - 0.5).abs() < 1e-12, "{c}");
        // distance 1,000: angular = transversal / 1000. Want
        // angular * 100 / (2 * 50) = 1, so angular = 1, transversal 1000.
        let c = chance_to_hit(&k(), aim(1_000.0, 1_000.0, 50.0));
        assert!((c - 0.5).abs() < 1e-12, "{c}");
        // Both: 0.5 ^ (1 + 1).
        let c = chance_to_hit(&k(), aim(1_500.0, 1_500.0, 50.0));
        assert!((c - 0.25).abs() < 1e-12, "{c}");
    }

    /// A bigger target is easier to track, a faster one harder.
    #[test]
    fn signature_and_speed_move_the_chance_the_right_way() {
        let small = chance_to_hit(&k(), aim(800.0, 400.0, 30.0));
        let big = chance_to_hit(&k(), aim(800.0, 400.0, 300.0));
        let fast = chance_to_hit(&k(), aim(800.0, 800.0, 30.0));
        assert!(big > small && small > fast);
    }

    #[test]
    fn past_optimal_with_no_falloff_is_a_miss() {
        let sharp = DroneType {
            falloff: 0.0,
            ..k()
        };
        assert_eq!(chance_to_hit(&sharp, aim(1_001.0, 0.0, 50.0)), 0.0);
    }

    #[test]
    fn transversal_is_the_perpendicular_part_of_the_relative_velocity() {
        // Target 1 km along x, moving 30 along x (radial) and 40 along y.
        let t = transversal([0.0; 3], [0.0; 3], [1_000.0, 0.0, 0.0], [30.0, 40.0, 0.0]);
        assert!((t - 40.0).abs() < 1e-9);
        // The shooter's own sideways speed counts against it.
        let t = transversal(
            [0.0; 3],
            [0.0, 100.0, 0.0],
            [1_000.0, 0.0, 0.0],
            [30.0, 40.0, 0.0],
        );
        assert!((t - 60.0).abs() < 1e-9);
        assert_eq!(transversal([1.0; 3], [5.0; 3], [1.0; 3], [9.0; 3]), 0.0);
    }

    #[test]
    fn a_roll_over_the_chance_is_a_miss_and_under_it_a_hit() {
        assert_eq!(verdict(0.6, 0.6, 0.5).quality, 0);
        assert_eq!(verdict(0.6, 0.6, 0.5).multiplier, 0.0);
        assert_ne!(verdict(0.6, 0.59, 0.5).quality, 0);
    }

    #[test]
    fn a_low_quality_roll_wrecks_for_triple() {
        let v = verdict(1.0, 0.0, WRECK_ROLL / 2.0);
        assert_eq!((v.quality, v.multiplier), (6, WRECK_MULTIPLIER));
    }

    /// Qualities rise with the roll, every hit lands in 1..=5, and over
    /// the whole range the ordinary multiplier averages its midpoint.
    #[test]
    fn ordinary_hits_rise_in_quality_with_the_roll() {
        let mut last = 0;
        let mut sum = 0.0;
        let n = 10_000;
        for i in 0..n {
            let r = WRECK_ROLL + (1.0 - WRECK_ROLL) * f64::from(i) / f64::from(n);
            let v = verdict(1.0, 0.0, r);
            assert!((1..=5).contains(&v.quality));
            assert!(v.quality >= last);
            last = v.quality;
            sum += v.multiplier;
        }
        let mean = sum / f64::from(n);
        assert!((mean - (HIT_FLOOR + HIT_SPREAD * (1.0 + WRECK_ROLL) / 2.0)).abs() < 1e-3);
    }

    #[test]
    fn rolls_are_deterministic_unit_interval_and_vary() {
        assert_eq!(roll(7, 1_000, 1), roll(7, 1_000, 1));
        assert_ne!(roll(7, 1_000, 1), roll(7, 1_000, 2));
        assert_ne!(roll(7, 1_000, 1), roll(8, 1_000, 1));
        let mut lo = 1.0f64;
        let mut hi = 0.0f64;
        for t in 0..2_000 {
            let r = roll(42, t * 40_000_000, 1);
            assert!((0.0..1.0).contains(&r));
            lo = lo.min(r);
            hi = hi.max(r);
        }
        assert!(lo < 0.05 && hi > 0.95, "{lo} {hi}");
    }

    #[test]
    fn the_settings_default_to_aggressive_without_focus() {
        let s = Settings::default();
        assert!(s.aggressive && !s.focus_fire);
        let d = PyValue::Dict(vec![
            (PyValue::Bool(false), PyValue::Int(ATTR_AGGRESSIVE)),
            (PyValue::Int(1), PyValue::Int(ATTR_FOCUS_FIRE)),
            (PyValue::Bool(false), PyValue::Int(31_337)),
        ]);
        let s = s.changed_by(&d);
        assert!(!s.aggressive && s.focus_fire);
        assert_eq!(s.changed_by(&PyValue::None), s);
    }

    #[test]
    fn scaling_damage_scales_every_type() {
        let d = scaled(
            Damage {
                em: 1.0,
                explosive: 2.0,
                kinetic: 3.0,
                thermal: 4.0,
            },
            2.0,
        );
        assert_eq!(
            (d.em, d.explosive, d.kinetic, d.thermal),
            (2.0, 4.0, 6.0, 8.0)
        );
    }
}
