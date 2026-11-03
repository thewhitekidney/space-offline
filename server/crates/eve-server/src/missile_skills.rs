//! **Missile skills, implants, rigs and modules** (K-MSKILL): what a fitted
//! launcher's missile is once the pilot's dogma has been applied to it.
//!
//! Missiles fly and hit through [`crate::missile`]; this module is the seam
//! between that flight and the dogma engine ([`crate::dogma`]). It holds **no
//! bonus of its own**. Every modifier that changes a missile is a
//! `modifierInfo` element of an effect in the client's data, and the engine
//! already applies those by the client's own rules (`crate::dogma`'s header);
//! what was missing was *reading* the answers:
//!
//! | what the pilot trains / fits | the modifier (client data **[V]**) | the answer read here |
//! |---|---|---|
//! | missile damage skills, specialisations, Warhead Upgrades, hull bonuses | `OwnerRequiredSkillModifier` on the charge's `emDamage` 114 / `explosiveDamage` 116 / `kineticDamage` 117 / `thermalDamage` 118 | [`Sheet::charge`] |
//! | Missile Bombardment, hull and rig flight bonuses, Missile Guidance Enhancer / Computer | on the charge's `explosionDelay` 281 | [`Sheet::charge`] |
//! | missile velocity bonuses | on the charge's `maxVelocity` 37 | [`Sheet::charge`] |
//! | Guided Missile Precision, Warhead Rigor Catalyst, guidance modules | on the charge's `aoeCloudSize` 654 (explosion radius) | [`Sheet::charge`] |
//! | Target Navigation Prediction, guidance modules | on the charge's `aoeVelocity` 653 (explosion velocity) | [`Sheet::charge`] |
//! | Ballistic Control Systems, Warhead Calefaction Catalyst rigs, implants | `ItemModifier` on the **pilot's** `missileDamageMultiplier` 212 | [`Sheet::character`] |
//! | Missile Launcher Operation, Rapid Launch, hull rate-of-fire bonuses | on the launcher's `speed` 51 (its cycle) | [`Sheet::module`] |
//!
//! # The client's own arithmetic **[V]**
//!
//! `fittingDogmaLocationUtil._GetLauncherDamagePerShot:254-260` is
//! `GetDamageFromItem(charge) * GetMissleDamageMultiplier(...)`, the second
//! factor being the **character's** attribute `missileDamageMultiplier` (212)
//! (`:263-269`), or `1` for a bomb launcher (group 862, `groupMissileLauncherBomb`).
//! `GetLauncherDps:246-251` divides a shot by the launcher's `attributeRateOfFire`
//! (51). So the fitting window's per-missile figure is exactly
//! [`Stats::volley`], and its rate of fire [`Stats::cycle_ms`]. A missile
//! flies at the charge's modified 37 for the charge's modified 281; its
//! explosion is the charge's modified 654 / 653 / 1353
//! ([`crate::missile::damage_factor`]).
//!
//! # What the engine does with each shape **[V]** (`crate::dogma`)
//!
//! - A skill's `skillLevel` scales its bonus attribute (the skill's own
//!   passive effect pre-multiplies it by 280), and a skill modifier is never
//!   stacking-penalised.
//! - A loaded charge is owner-modifiable: the pilot's `charID`-domain
//!   `OwnerRequiredSkillModifier`s (skills, hull bonuses, implants, rigs) reach
//!   every charge requiring the named skill. A module's `shipID`
//!   `LocationRequiredSkillModifier` reaches the charges loaded in the fit
//!   (Guidance Enhancers; Computers while active).
//! - A script loaded in a Guidance Computer is a charge whose `otherID`
//!   modifier scales the computer's own bonus attributes.
//! - `missileDamageMultiplier` is not `stackable`: Ballistic Control Systems
//!   and rigs (modules) are penalised, skills and implants are not.
//!
//! # `[A]`s
//!
//! - **Implants.** The pilot's implants are the items at `flagImplant` (89)
//!   inside the character ([`crate::dogma::FLAG_IMPLANT`]) **[I]**; the client
//!   keeps them in `charMgr`'s implant dict, which the server does not serve
//!   yet (`GetImplants` is empty), so the *fitting window* of an implanted
//!   pilot will not show them even though the server applies them. Falsifier:
//!   an implant in the character's inventory changing no number here.
//! - **Overload** (category 5 effects, `overloadSelfMissileGuidanceBonus5`) is
//!   not applied; `crate::dogma` does not model overload.
//! - The last round of a stack fires with the base numbers of its type when
//!   it is not the type now loaded (the engine's sheet holds the loaded one).

use crate::damage::{Damage, attr as dmg};
use crate::dogma::Sheet;
use crate::missile::{Aoe, Charge, attr as flight};
use crate::service::Registry;

/// The dogma ids this module reads **[V]**.
pub mod attr {
    /// The pilot's `missileDamageMultiplier`.
    pub const MISSILE_DAMAGE_MULTIPLIER: i64 = 212;
    /// The launcher's cycle, `attributeRateOfFire`.
    pub const RATE_OF_FIRE: i64 = 51;
}

/// `groupMissileLauncherBomb` **[V]**: a bomb's damage ignores 212
/// (`GetMissleDamageMultiplier:265-266`).
pub const BOMB_LAUNCHER_GROUP: i64 = 862;

/// Where a migrated effect's modifier reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// `AddOwnerRequiredSkillModifier` on `charID`: the pilot's charges
    /// (and drones) that require the skill.
    Pilot,
    /// `AddLocationRequiredSkillModifier` on `shipID`: the modules fitted
    /// to the ship that require the skill.
    Ship,
}

/// One of the client's **Python effects** (`migratedeffects.pyc`) that has
/// no `modifierInfo` in the data, as `crate::dogma` applies it **[V]**: every
/// one is `Start: env.dogmaLM.Add{Owner,Location}RequiredSkillModifier(
/// operation, id, affectedSkillID = env.itemTypeID, modifiedAttributeID,
/// affectingModuleID = env.itemID, modifyingAttributeID)` -- the skill's own
/// type is the required skill, the skill item is the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migrated {
    pub reach: Reach,
    /// `dgmAssPostPercent`.
    pub operation: i64,
    pub modified: i64,
    pub modifying: i64,
}

/// `attributeDamageMultiplierBonus` [V]: the skill's per-level damage bonus.
const DAMAGE_MULTIPLIER_BONUS: i64 = 292;
/// `attributeRofBonus` [V].
const ROF_BONUS: i64 = 293;

/// The migrated effect `effect_id` (K-MSKILL), if it is one that matters to
/// missiles: `missileEMDmgBonus` 660, `missileExplosiveDmgBonus` 661,
/// `missileThermalDmgBonus` 662 and `missileKineticDmgBonus2` 668 (the
/// per-family damage skills: Rockets, Light/Heavy/Auto-Targeting Missiles,
/// Torpedoes, Cruise Missiles, Heavy Assault Missiles) and `selfRof` 1851
/// (the specialisations' rate of fire). `droneDmgBonus` 1730 is the same
/// shape for drones, left to the drone lane.
///
/// **The client's data lists these effects without a modifier**; an engine
/// that read `modifierInfo` alone would give a Heavy Missiles V pilot no
/// damage at all. Falsifier: the fitting window's per-missile figure for a
/// Heavy Missiles V pilot disagreeing with `Stats::volley`.
pub fn migrated(effect_id: i64) -> Option<Migrated> {
    let pilot = |modified| Migrated {
        reach: Reach::Pilot,
        operation: crate::dogma::op::POST_PERCENT,
        modified,
        modifying: DAMAGE_MULTIPLIER_BONUS,
    };
    match effect_id {
        660 => Some(pilot(dmg::EM_DAMAGE)),
        661 => Some(pilot(dmg::EXPLOSIVE_DAMAGE)),
        662 => Some(pilot(dmg::THERMAL_DAMAGE)),
        668 => Some(pilot(dmg::KINETIC_DAMAGE)),
        1851 => Some(Migrated {
            reach: Reach::Ship,
            operation: crate::dogma::op::POST_PERCENT,
            modified: attr::RATE_OF_FIRE,
            modifying: ROF_BONUS,
        }),
        _ => None,
    }
}

/// The missile damage factor of a launcher in group `launcher_group` on a
/// sheet: the pilot's modified 212, or `1` for a bomb launcher.
pub fn damage_multiplier(sheet: &Sheet, launcher_group: i64) -> f64 {
    if launcher_group == BOMB_LAUNCHER_GROUP {
        1.0
    } else {
        sheet.character(attr::MISSILE_DAMAGE_MULTIPLIER)
    }
}

/// [`damage_multiplier`] for the launcher item `module_id` (its own group).
/// `None` when the module is not an item.
pub fn launcher_multiplier(w: &Registry, sheet: &Sheet, module_id: i64) -> Option<f64> {
    w.item(module_id)
        .map(|m| damage_multiplier(sheet, m.group_id))
}

/// `base` with the numbers the sheet gives the charge loaded in `module_id`:
/// its modified speed (37), fuse (281) and explosion (654, 653, 1353). The
/// type's own radius, mass and agility stay. `base` unchanged when the sheet
/// holds no charge there, or when a modifier would leave it unable to fly.
pub fn fitted_charge(base: Charge, sheet: &Sheet, module_id: i64) -> Charge {
    let (Some(speed), Some(delay)) = (
        sheet.charge(module_id, flight::MAX_VELOCITY),
        sheet.charge(module_id, flight::EXPLOSION_DELAY),
    ) else {
        return base;
    };
    let fuse_s = delay / 1000.0;
    if speed <= 0.0 || fuse_s <= 0.0 {
        return base;
    }
    let a = |id| sheet.charge(module_id, id).unwrap_or(0.0);
    Charge {
        speed,
        fuse_s,
        aoe: Aoe {
            cloud: a(flight::AOE_CLOUD_SIZE),
            velocity: a(flight::AOE_VELOCITY),
            drf: a(flight::AOE_DAMAGE_REDUCTION_FACTOR),
        },
        ..base
    }
}

/// **The missile a launcher fires**, with the pilot's dogma on it: what
/// `crate::combat::on_cycles` hands to [`crate::missile::launch`]. The raw
/// charge ([`Charge::of`]) when the ship has no sheet (no typedata, an NPC),
/// or when `charge_type` is not the type loaded now (the last round of a
/// stack). `None` when the charge cannot fly at all.
pub fn charge_for(w: &Registry, ship_id: i64, module_id: i64, charge_type: i64) -> Option<Charge> {
    let charge = match stats(w, ship_id, module_id) {
        Some(s) if s.charge.type_id == charge_type => Some(s.charge),
        _ => Charge::of(
            crate::typedata::installed_in(w.content()).table(),
            charge_type,
            1.0,
            1.0,
        ),
    };
    // A guidance disruptor on the launching ship (EWAR-2B). `stats` is the
    // fitting window and stays clean.
    charge.map(|c| disrupted(c, crate::ewar::missile_factors(w, ship_id)))
}

/// `charge` with a guidance disruptor's factors on its speed, flight time and
/// explosion numbers.
pub fn disrupted(
    charge: Charge,
    (speed, fuse, aoe_velocity, aoe_cloud): (f64, f64, f64, f64),
) -> Charge {
    Charge {
        speed: charge.speed * speed,
        fuse_s: charge.fuse_s * fuse,
        aoe: Aoe {
            velocity: charge.aoe.velocity * aoe_velocity,
            cloud: charge.aoe.cloud * aoe_cloud,
            ..charge.aoe
        },
        ..charge
    }
}

/// What the fitting window and Show Info say about one launcher with its
/// charge, in the server's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// The missile as it flies: [`fitted_charge`].
    pub charge: Charge,
    /// One missile's damage, before the explosion factor: the charge's four
    /// modified damages times [`Stats::multiplier`].
    pub volley: Damage,
    /// The pilot's `missileDamageMultiplier` (1 for a bomb launcher).
    pub multiplier: f64,
    /// The launcher's cycle, milliseconds (attribute 51).
    pub cycle_ms: f64,
    /// The missile's `maxVelocity`, m/s.
    pub speed: f64,
    /// The missile's flight time, seconds.
    pub flight_s: f64,
    /// `speed * flight_s`, metres: how far it can fly.
    pub range: f64,
    pub aoe: Aoe,
}

impl Stats {
    /// One missile's damage over the cycle: the fitting window's missile DPS
    /// (`GetLauncherDps:249`, without the reload), per second.
    #[cfg(test)]
    pub fn dps(&self) -> f64 {
        if self.cycle_ms <= 0.0 {
            return 0.0;
        }
        let d = self.volley;
        (d.em + d.explosive + d.kinetic + d.thermal) / (self.cycle_ms / 1000.0)
    }
}

/// [`Stats`] from a sheet: `base` is the charge's own numbers (the fallback
/// for anything the sheet lacks), `launcher_group` the launcher's group.
pub fn stats_from(sheet: &Sheet, module_id: i64, base: Charge, launcher_group: i64) -> Stats {
    let charge = fitted_charge(base, sheet, module_id);
    let multiplier = damage_multiplier(sheet, launcher_group);
    let d = |id, unmodified| sheet.charge(module_id, id).unwrap_or(unmodified);
    let volley = Damage {
        em: d(dmg::EM_DAMAGE, base.damage.em) * multiplier,
        explosive: d(dmg::EXPLOSIVE_DAMAGE, base.damage.explosive) * multiplier,
        kinetic: d(dmg::KINETIC_DAMAGE, base.damage.kinetic) * multiplier,
        thermal: d(dmg::THERMAL_DAMAGE, base.damage.thermal) * multiplier,
    };
    Stats {
        charge,
        volley,
        multiplier,
        cycle_ms: sheet.module(module_id, attr::RATE_OF_FIRE).unwrap_or(0.0),
        speed: charge.speed,
        flight_s: charge.fuse_s,
        range: charge.speed * charge.fuse_s,
        aoe: charge.aoe,
    }
}

/// [`Stats`] of the launcher `module_id` on `ship_id` as the world has it.
/// `None` for a ship with no sheet, a module that is not a fitted launcher,
/// or one with nothing flyable loaded.
pub fn stats(w: &Registry, ship_id: i64, module_id: i64) -> Option<Stats> {
    let sheet = crate::dogma::sheet(w, ship_id)?;
    let module = w.item(module_id)?;
    let charge = crate::modules::loaded(w.store(), ship_id, module.flag_id)?;
    let base = Charge::of(
        crate::typedata::installed_in(w.content()).table(),
        charge.type_id,
        1.0,
        1.0,
    )?;
    Some(stats_from(&sheet, module_id, base, module.group_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dogma::{Inputs, ModuleInput, State, evaluate};
    use crate::service::Registry;
    use crate::typedata::Table;

    // Invented ids: **not the client's types or effects**. The attribute ids
    // are protocol (`crate::missile::attr`), the shapes of the modifiers are
    // the client's (`docs/lanes/k-mskill.md` lists the effects they model).
    const HULL: i64 = 990_470_001;
    /// The hull with the missile bonuses.
    const BONUS_HULL: i64 = 990_470_002;
    const HULL_SKILL: i64 = 990_470_010;
    const LAUNCHER_OP: i64 = 990_470_011;
    const HEAVY: i64 = 990_470_012;
    const SPECIAL: i64 = 990_470_013;
    const BOMBARDMENT: i64 = 990_470_014;
    const PROJECTION: i64 = 990_470_015;
    const PRECISION: i64 = 990_470_016;
    const NAVIGATION: i64 = 990_470_017;
    const WARHEAD: i64 = 990_470_018;
    const RAPID: i64 = 990_470_019;
    const LAUNCHER: i64 = 990_470_020;
    const MISSILE: i64 = 990_470_021;
    const OTHER_MISSILE: i64 = 990_470_022;
    const BCS: i64 = 990_470_023;
    const CALEFACTION_RIG: i64 = 990_470_024;
    const RIGOR_RIG: i64 = 990_470_025;
    const ENHANCER: i64 = 990_470_026;
    const COMPUTER: i64 = 990_470_027;
    const SCRIPT: i64 = 990_470_028;
    const IMPLANT: i64 = 990_470_029;
    const SNAPSHOT_IMPLANT: i64 = 990_470_033;
    const BOMB_LAUNCHER: i64 = 990_470_030;
    const SPECIAL_LAUNCHER: i64 = 990_470_032;

    const FIXTURE: &str = "\
attribute\t37\t0.0\tmaxVelocity
attribute\t51\t0.0\tspeed
attribute\t114\t0.0\temDamage
attribute\t116\t0.0\texplosiveDamage
attribute\t117\t0.0\tkineticDamage
attribute\t118\t0.0\tthermalDamage
attribute\t182\t0.0\trequiredSkill1
attribute\t183\t0.0\trequiredSkill2
attribute\t184\t0.0\trequiredSkill3
attribute\t212\t1.0\tmissileDamageMultiplier
attribute\t213\t0.0\tmissileDamageMultiplierBonus
attribute\t280\t0.0\tskillLevel
attribute\t281\t0.0\texplosionDelay
attribute\t292\t0.0\tdamageMultiplierBonus
attribute\t293\t0.0\tspeedBonus
attribute\t460\t0.0\tshipBonusMF
attribute\t461\t0.0\tshipBonusMF2
attribute\t547\t0.0\tmissileVelocityBonus
attribute\t557\t0.0\tmaxFlightTimeBonus
attribute\t596\t0.0\texplosionDelayBonus
attribute\t653\t0.0\taoeVelocity
attribute\t654\t0.0\taoeCloudSize
attribute\t847\t0.0\taoeVelocityBonus
attribute\t848\t0.0\taoeCloudSizeBonus
attribute\t2026\t0.0\tscriptFlightTimeBonusBonus
attribute\t1353\t1.0\taoeDamageReductionFactor
attrmeta\t37\t0\t1\t0\t0\t4
attrmeta\t51\t0\t0\t0\t0\t4
attrmeta\t212\t0\t1\t0\t0\t5
attrmeta\t213\t1\t1\t0\t0\t5
attrmeta\t281\t0\t1\t0\t0\t4
attrmeta\t653\t0\t1\t0\t0\t4
attrmeta\t654\t0\t0\t0\t0\t4
type\t990470001\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t\t9=300.0\tTest Cruiser
type\t990470002\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t9001,9002\t9=300.0;461=-5.0;460=5.0\tTest Missile Cruiser
type\t990470010\t257\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9110\t\tTest Hull Skill
type\t990470011\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9120\t293=-5.0\tTest Launcher Operation
type\t990470012\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t152,660,661,662,668\t292=5.0\tTest Heavy Missiles
type\t990470013\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t163,1851\t293=-2.0\tTest Heavy Specialization
type\t990470014\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9123\t557=10.0\tTest Bombardment
type\t990470015\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9124\t547=10.0\tTest Projection
type\t990470016\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9125\t848=-5.0\tTest Precision
type\t990470017\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9126\t847=5.0\tTest Navigation
type\t990470018\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9127\t292=2.0\tTest Warhead Upgrades
type\t990470019\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9128\t293=-3.0\tTest Rapid Launch
type\t990470020\t510\t7\t1\t1\t10.0\t0.9\t0.0\t1.0\t101\t12,16,40,101\t51=10000.0;56=1.0;182=990470011.0;183=990470012.0;604=385.0\tTest Launcher
type\t990470030\t862\t7\t1\t1\t10.0\t0.9\t0.0\t1.0\t101\t12,16,40,101\t51=10000.0;56=1.0;182=990470011.0;604=385.0\tTest Bomb Launcher
type\t990470021\t385\t8\t1\t100\t0.03\t0.0\t1000.0\t300.0\t9\t9\t37=4000.0;70=0.0001;117=100.0;114=20.0;281=6000.0;653=100.0;654=200.0;1353=0.5;182=990470012.0;183=990470011.0\tTest Missile
type\t990470022\t385\t8\t1\t100\t0.03\t0.0\t1000.0\t300.0\t9\t9\t37=4000.0;117=100.0;281=6000.0;653=100.0;654=200.0;1353=0.5;182=990470018.0\tTest Other Missile
type\t990470032\t510\t7\t1\t1\t10.0\t0.9\t0.0\t1.0\t101\t12,16,40,101\t51=10000.0;56=1.0;182=990470011.0;183=990470013.0;604=385.0\tTest Special Launcher
type\t990470023\t339\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t11,16,9200\t213=1.1\tTest Ballistic Control
type\t990470024\t773\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t9210\t213=1.1\tTest Calefaction Rig
type\t990470025\t773\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t9211\t848=-15.0\tTest Rigor Rig
type\t990470026\t1\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t11,16,9220\t547=10.0;596=10.0;847=10.0;848=-10.0\tTest Enhancer
type\t990470027\t2\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t9230\t13,16,9230\t547=10.0;596=10.0;847=10.0;848=-10.0\tTest Computer
type\t990470028\t3\t8\t1\t100\t0.03\t0.0\t0.0\t1.0\t0\t9231\t2026=100.0\tTest Script
type\t990470033\t300\t20\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t9241\t292=3.0\tTest Snapshot Implant
type\t990470029\t300\t20\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t9240\t213=1.05\tTest Implant
effect\t11\t0\t0\t0\t0\t0\t0\t0\tloPower\t
effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t
effect\t13\t0\t0\t0\t0\t0\t0\t0\tmedPower\t
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t
effect\t152\t0\t0\t0\t0\t0\t0\t0\tskillBoostDamageMultiplierBonus\t
effect\t163\t0\t0\t0\t0\t0\t0\t0\trapidFiringSkillBoostRofBonus\t
effect\t660\t0\t0\t0\t0\t0\t0\t0\tmissileEMDmgBonus\t
effect\t661\t0\t0\t0\t0\t0\t0\t0\tmissileExplosiveDmgBonus\t
effect\t662\t0\t0\t0\t0\t0\t0\t0\tmissileThermalDmgBonus\t
effect\t668\t0\t0\t0\t0\t0\t0\t0\tmissileKineticDmgBonus2\t
effect\t1851\t0\t0\t0\t0\t0\t0\t0\tselfRof\t
effect\t9\t2\t1\t0\t0\t0\t0\t0\tmissileLaunching\teffects.FixtureLaunch
effect\t9001\t0\t0\t0\t0\t0\t0\t0\thullMissileDamage\t
effect\t9002\t0\t0\t0\t0\t0\t0\t0\thullMissileRof\t
effect\t9110\t0\t0\t0\t0\t0\t0\t0\thullSkill\t
effect\t9120\t0\t0\t0\t0\t0\t0\t0\tlauncherOperationRof\t
effect\t9123\t0\t0\t0\t0\t0\t0\t0\tbombardment\t
effect\t9124\t0\t0\t0\t0\t0\t0\t0\tprojection\t
effect\t9125\t0\t0\t0\t0\t0\t0\t0\tprecision\t
effect\t9126\t0\t0\t0\t0\t0\t0\t0\tnavigation\t
effect\t9127\t0\t0\t0\t0\t0\t0\t0\twarheadUpgrades\t
effect\t9128\t0\t0\t0\t0\t0\t0\t0\trapidLaunch\t
effect\t9200\t4\t0\t0\t0\t0\t0\t0\tballisticControl\t
effect\t9210\t0\t0\t0\t0\t0\t0\t0\tcalefactionRig\t
effect\t9211\t0\t0\t0\t0\t0\t0\t0\trigorRig\t
effect\t9220\t4\t0\t0\t0\t0\t0\t0\tguidanceEnhancer\t
effect\t9230\t1\t0\t0\t0\t0\t0\t0\tguidanceComputer\teffects.FixtureComputer
effect\t9231\t0\t0\t0\t0\t0\t0\t0\tscript\t
effect\t9241\t0\t0\t0\t0\t0\t0\t0\timplantFamilyDamage\t
effect\t9240\t0\t0\t0\t0\t0\t0\t0\timplantDamage\t
modifier\t152\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t163\titemID\tItemModifier\t293\t280\t0\t\t
modifier\t9001\tcharID\tOwnerRequiredSkillModifier\t117\t460\t6\t990470012\t
modifier\t9002\tshipID\tLocationRequiredSkillModifier\t51\t461\t6\t990470011\t
modifier\t9110\tshipID\tItemModifier\t460\t280\t0\t\t
modifier\t9110\tshipID\tItemModifier\t461\t280\t0\t\t
modifier\t9120\titemID\tItemModifier\t293\t280\t0\t\t
modifier\t9120\tshipID\tLocationRequiredSkillModifier\t51\t293\t6\t990470011\t
modifier\t9123\titemID\tItemModifier\t557\t280\t0\t\t
modifier\t9123\tcharID\tOwnerRequiredSkillModifier\t281\t557\t6\t990470011\t
modifier\t9124\titemID\tItemModifier\t547\t280\t0\t\t
modifier\t9124\tcharID\tOwnerRequiredSkillModifier\t37\t547\t6\t990470011\t
modifier\t9125\titemID\tItemModifier\t848\t280\t0\t\t
modifier\t9125\tcharID\tOwnerRequiredSkillModifier\t654\t848\t6\t990470011\t
modifier\t9126\titemID\tItemModifier\t847\t280\t0\t\t
modifier\t9126\tcharID\tOwnerRequiredSkillModifier\t653\t847\t6\t990470011\t
modifier\t9127\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t9127\tcharID\tOwnerRequiredSkillModifier\t117\t292\t6\t990470011\t
modifier\t9128\titemID\tItemModifier\t293\t280\t0\t\t
modifier\t9128\tshipID\tLocationRequiredSkillModifier\t51\t293\t6\t990470011\t
modifier\t9200\tcharID\tItemModifier\t212\t213\t0\t\t
modifier\t9210\tcharID\tItemModifier\t212\t213\t0\t\t
modifier\t9211\tcharID\tOwnerRequiredSkillModifier\t654\t848\t6\t990470011\t
modifier\t9220\tshipID\tLocationRequiredSkillModifier\t37\t547\t6\t990470011\t
modifier\t9220\tshipID\tLocationRequiredSkillModifier\t281\t596\t6\t990470011\t
modifier\t9220\tshipID\tLocationRequiredSkillModifier\t653\t847\t6\t990470011\t
modifier\t9220\tshipID\tLocationRequiredSkillModifier\t654\t848\t6\t990470011\t
modifier\t9230\tshipID\tLocationRequiredSkillModifier\t37\t547\t6\t990470011\t
modifier\t9230\tshipID\tLocationRequiredSkillModifier\t281\t596\t6\t990470011\t
modifier\t9230\tshipID\tLocationRequiredSkillModifier\t653\t847\t6\t990470011\t
modifier\t9230\tshipID\tLocationRequiredSkillModifier\t654\t848\t6\t990470011\t
modifier\t9231\totherID\tItemModifier\t596\t2026\t6\t\t
modifier\t9241\tcharID\tOwnerRequiredSkillModifier\t117\t292\t6\t990470012\t
modifier\t9240\tcharID\tItemModifier\t212\t213\t0\t\t
";

    fn table() -> Table {
        Table::parse(FIXTURE).expect("the fixture parses")
    }

    fn launcher(item_id: i64, charge: Option<i64>) -> ModuleInput {
        ModuleInput {
            item_id,
            type_id: LAUNCHER,
            state: State::Active,
            charge_type: charge,
        }
    }

    fn module(item_id: i64, type_id: i64, state: State) -> ModuleInput {
        ModuleInput {
            item_id,
            type_id,
            state,
            charge_type: None,
        }
    }

    /// The launcher (item 1) loaded with `MISSILE`, plus `extra` modules,
    /// the given skills and implants.
    fn fit(skills: &[(i64, i64)], extra: Vec<ModuleInput>, implants: &[i64]) -> Sheet {
        fit_on(HULL, skills, extra, implants)
    }

    fn fit_on(
        hull: i64,
        skills: &[(i64, i64)],
        extra: Vec<ModuleInput>,
        implants: &[i64],
    ) -> Sheet {
        let mut modules = vec![launcher(1, Some(MISSILE))];
        modules.extend(extra);
        modules.sort_by_key(|m| m.item_id);
        evaluate(
            &table(),
            &Inputs {
                ship_type: hull,
                skills: skills.to_vec(),
                modules,
                implants: implants.to_vec(),
                ..Inputs::default()
            },
        )
        .expect("the hull is in the fixture")
    }

    fn base_charge() -> Charge {
        Charge::of(&table(), MISSILE, 1.0, 1.0).expect("flies")
    }

    fn stats_of(sheet: &Sheet) -> Stats {
        stats_from(sheet, 1, base_charge(), 510)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1.0)
    }

    /// Nothing trained, nothing fitted: the charge's own numbers, a 1.0
    /// multiplier and the launcher's own cycle -- the sheet and the raw
    /// table agree, so the seam changes nothing for an unskilled pilot.
    #[test]
    fn an_unskilled_pilot_gets_the_charges_own_numbers() {
        let s = stats_of(&fit(&[], vec![], &[]));
        let raw = base_charge();
        assert_eq!(s.multiplier, 1.0);
        assert_eq!(s.volley, raw.damage);
        assert_eq!(s.speed, 4000.0);
        assert_eq!(s.flight_s, 6.0);
        assert_eq!(s.range, 24_000.0);
        assert_eq!(s.cycle_ms, 10_000.0);
        assert_eq!(s.aoe, raw.aoe);
        assert_eq!(
            s.aoe,
            Aoe {
                cloud: 200.0,
                velocity: 100.0,
                drf: 0.5
            }
        );
    }

    /// **Missile damage skills** (the family skill, a specialisation,
    /// Warhead Upgrades): each is a per-level post-percent on the damage of
    /// the charges that require it, and they add (one operator, no penalty
    /// for a skill). The charge that does not require the skill gets none.
    #[test]
    fn damage_skills_scale_by_level_and_only_charges_requiring_them() {
        // 5 %/level x 4 on every damage the charge has; 2 %/level x 5
        // Warhead Upgrades on the kinetic only (as the fixture has it).
        let s = fit(&[(HEAVY, 4), (WARHEAD, 5), (LAUNCHER_OP, 0)], vec![], &[]);
        let v = stats_of(&s).volley;
        assert!(close(v.em, 20.0 * 1.20), "{v:?}");
        assert!(close(v.kinetic, 100.0 * 1.20 * 1.10), "{v:?}");
        assert_eq!(v.explosive, 0.0);
        // A charge that requires neither skill: untouched.
        let mut modules = vec![launcher(1, Some(OTHER_MISSILE))];
        modules.sort_by_key(|m| m.item_id);
        let other = evaluate(
            &table(),
            &Inputs {
                ship_type: HULL,
                skills: vec![(HEAVY, 5), (LAUNCHER_OP, 5)],
                modules,
                implants: vec![],
                ..Inputs::default()
            },
        )
        .expect("evaluates");
        assert_eq!(other.charge(1, 117), Some(100.0), "needs the other skill");
    }

    /// **A specialisation** (the client's `selfRof` Python effect, no
    /// `modifierInfo`) shortens the cycle of the launchers that require it
    /// and no other.
    #[test]
    fn a_specialisation_speeds_only_the_launchers_requiring_it() {
        let cycle = |launcher_type: i64, special: i64| {
            let sheet = evaluate(
                &table(),
                &Inputs {
                    ship_type: HULL,
                    skills: vec![(SPECIAL, special)],
                    modules: vec![ModuleInput {
                        item_id: 1,
                        type_id: launcher_type,
                        state: State::Active,
                        charge_type: Some(MISSILE),
                    }],
                    implants: vec![],
                    ..Inputs::default()
                },
            )
            .expect("evaluates");
            sheet.module(1, attr::RATE_OF_FIRE).expect("on the sheet")
        };
        // -2 %/level, five levels.
        assert!(close(cycle(SPECIAL_LAUNCHER, 5), 10_000.0 * 0.90));
        assert_eq!(
            cycle(LAUNCHER, 5),
            10_000.0,
            "it does not require the skill"
        );
        assert_eq!(
            cycle(SPECIAL_LAUNCHER, 0),
            10_000.0,
            "level zero is no bonus"
        );
    }

    /// **The client's family damage skills have no `modifierInfo`**
    /// (`missileKineticDmgBonus2` and its three siblings are Python
    /// effects): the engine must still give a trained pilot the damage, and
    /// each of the four damage types is its own effect.
    #[test]
    fn the_family_skills_boost_each_damage_type_through_the_python_effects() {
        assert!(table().modifiers(668).is_empty(), "no modifier in the data");
        let s = stats_of(&fit(&[(HEAVY, 2)], vec![], &[]));
        assert!(close(s.volley.em, 20.0 * 1.10));
        assert!(close(s.volley.kinetic, 100.0 * 1.10));
        for (effect, attribute) in [(660, 114), (661, 116), (662, 118), (668, 117)] {
            let m = migrated(effect).expect("a migrated effect");
            assert_eq!(
                (m.reach, m.modified, m.modifying),
                (Reach::Pilot, attribute, 292)
            );
            assert_eq!(m.operation, crate::dogma::op::POST_PERCENT);
        }
        let rof = migrated(1851).expect("selfRof");
        assert_eq!(
            (rof.reach, rof.modified, rof.modifying),
            (Reach::Ship, 51, 293)
        );
        assert_eq!(migrated(9), None);
    }

    /// **Rate of fire**: Missile Launcher Operation and Rapid Launch cut the
    /// launcher's cycle per level (post-percent on `speed`).
    #[test]
    fn rate_of_fire_skills_shorten_the_cycle() {
        let s = stats_of(&fit(&[(LAUNCHER_OP, 4), (RAPID, 5)], vec![], &[]));
        assert!(close(s.cycle_ms, 10_000.0 * (1.0 - 0.20) * (1.0 - 0.15)));
        // DPS is damage over the cycle.
        let flat = stats_of(&fit(&[], vec![], &[]));
        assert!(s.dps() > flat.dps());
        assert!(close(flat.dps(), (20.0 + 100.0) / 10.0));
    }

    /// **Flight time** (Missile Bombardment) and **velocity** (Missile
    /// Projection) move the range: the missile flies `37 x 281`.
    #[test]
    fn flight_time_and_velocity_skills_lengthen_the_range() {
        // Bombardment 10 %/level x 5 = +50 % flight.
        let s = stats_of(&fit(&[(BOMBARDMENT, 5), (LAUNCHER_OP, 0)], vec![], &[]));
        assert!(close(s.flight_s, 6.0 * 1.5));
        assert_eq!(s.speed, 4000.0);
        let s = stats_of(&fit(&[(PROJECTION, 3), (LAUNCHER_OP, 0)], vec![], &[]));
        assert!(close(s.speed, 4000.0 * 1.3));
        assert!(close(s.range, 4000.0 * 1.3 * 6.0));
        let both = stats_of(&fit(
            &[(BOMBARDMENT, 5), (PROJECTION, 3), (LAUNCHER_OP, 0)],
            vec![],
            &[],
        ));
        assert!(close(both.range, 4000.0 * 1.3 * 6.0 * 1.5));
    }

    /// **Explosion radius** (Guided Missile Precision) and **explosion
    /// velocity** (Target Navigation Prediction).
    #[test]
    fn precision_and_navigation_shape_the_explosion() {
        let s = stats_of(&fit(
            &[(PRECISION, 4), (NAVIGATION, 5), (LAUNCHER_OP, 0)],
            vec![],
            &[],
        ));
        assert!(close(s.aoe.cloud, 200.0 * (1.0 - 0.20)));
        assert!(close(s.aoe.velocity, 100.0 * 1.25));
        assert_eq!(
            s.aoe.drf, 0.5,
            "the damage reduction factor is the charge's"
        );
    }

    /// **Ballistic Control Systems** (online, module) multiply the pilot's
    /// missile damage; three are stacking-penalised, as the client's
    /// non-`stackable` 212 does it.
    #[test]
    fn ballistic_control_systems_multiply_damage_with_a_penalty() {
        let one = stats_of(&fit(&[], vec![module(2, BCS, State::Online)], &[]));
        assert!(close(one.multiplier, 1.1));
        assert!(close(one.volley.kinetic, 110.0));
        // An offline one does nothing.
        let off = stats_of(&fit(&[], vec![module(2, BCS, State::Offline)], &[]));
        assert_eq!(off.multiplier, 1.0);
        let three = stats_of(&fit(
            &[],
            (2..=4).map(|i| module(i, BCS, State::Online)).collect(),
            &[],
        ));
        let want: f64 = (0..3)
            .map(|i| 1.0 + 0.1 / crate::dogma::nurf_denominator(i))
            .product::<f64>();
        // 1.1 x 3 -> the 2nd is scaled by 1/exp((1/2.67)^2), the 3rd by
        // 1/exp((2/2.67)^2): strictly less than three full bonuses.
        assert!(close(three.multiplier, want));
        assert!(three.multiplier < 1.1f64.powi(3));
    }

    /// **Rigs**: a Warhead Calefaction rig is a passive multiplier on the
    /// pilot's damage (no online needed); a Warhead Rigor rig shrinks the
    /// explosion radius.
    #[test]
    fn rigs_change_damage_and_explosion_radius_while_merely_fitted() {
        let s = stats_of(&fit(
            &[(LAUNCHER_OP, 0)],
            vec![
                module(2, CALEFACTION_RIG, State::Offline),
                module(3, RIGOR_RIG, State::Offline),
            ],
            &[],
        ));
        assert!(close(s.multiplier, 1.1));
        assert!(close(s.aoe.cloud, 200.0 * 0.85));
    }

    /// **Implants** (hardwirings): a passive multiplier on the pilot's
    /// damage, never stacking-penalised, and it multiplies with a BCS.
    #[test]
    fn an_implant_multiplies_the_pilots_missile_damage_without_a_penalty() {
        let s = stats_of(&fit(&[], vec![], &[IMPLANT]));
        assert!(close(s.multiplier, 1.05));
        let both = stats_of(&fit(&[], vec![module(2, BCS, State::Online)], &[IMPLANT]));
        assert!(close(both.multiplier, 1.05 * 1.1));
        let two = stats_of(&fit(&[], vec![], &[IMPLANT, IMPLANT]));
        assert!(
            close(two.multiplier, 1.05 * 1.05),
            "an implant is not penalised"
        );
    }

    /// A **skill-family implant** (a Snapshot hardwiring) is a flat bonus to
    /// the damage of the charges requiring its skill, beside the skill's
    /// own, and no other missile feels it.
    #[test]
    fn a_family_implant_adds_to_the_skill_for_its_own_missiles() {
        let s = stats_of(&fit(&[(HEAVY, 2)], vec![], &[SNAPSHOT_IMPLANT]));
        assert!(close(s.volley.kinetic, 100.0 * 1.10 * 1.03));
        // This one boosts kinetic only, and works with no skill trained.
        assert!(close(s.volley.em, 20.0 * 1.10));
        let bare = stats_of(&fit(&[], vec![], &[SNAPSHOT_IMPLANT]));
        assert!(close(bare.volley.kinetic, 103.0));
    }

    /// **Guidance enhancers** (online, lows) and **computers** (active,
    /// mids) change flight and explosion together; a computer that is not
    /// running does nothing, and a **script** in it scales its flight bonus.
    #[test]
    fn guidance_modules_and_their_scripts() {
        let skills = [(LAUNCHER_OP, 0)];
        let enhancer = stats_of(&fit(&skills, vec![module(2, ENHANCER, State::Online)], &[]));
        assert!(close(enhancer.speed, 4000.0 * 1.1));
        assert!(close(enhancer.flight_s, 6.0 * 1.1));
        assert!(close(enhancer.aoe.velocity, 100.0 * 1.1));
        assert!(close(enhancer.aoe.cloud, 200.0 * 0.9));

        let idle = stats_of(&fit(&skills, vec![module(3, COMPUTER, State::Online)], &[]));
        assert_eq!(idle.speed, 4000.0, "a computer is an active module");
        let run = stats_of(&fit(&skills, vec![module(3, COMPUTER, State::Active)], &[]));
        assert!(close(run.speed, 4000.0 * 1.1));

        // The script doubles the computer's flight-time bonus (10 % -> 20 %)
        // and leaves its other bonuses.
        let mut scripted = module(3, COMPUTER, State::Active);
        scripted.charge_type = Some(SCRIPT);
        let s = stats_of(&fit(&skills, vec![scripted], &[]));
        assert!(close(s.flight_s, 6.0 * 1.2));
        assert!(close(s.speed, 4000.0 * 1.1));
    }

    /// **Hull bonuses**: a bonus on the hull, scaled by the hull skill's
    /// level, for damage and for rate of fire.
    #[test]
    fn hull_bonuses_are_scaled_by_the_hull_skill() {
        // +5 % kinetic damage and -5 % cycle per level, three levels.
        let s = stats_of(&fit_on(BONUS_HULL, &[(HULL_SKILL, 3)], vec![], &[]));
        assert!(close(s.volley.kinetic, 100.0 * 1.15));
        assert!(close(s.cycle_ms, 10_000.0 * 0.85));
        // The hull skill untrained: the hull's own unscaled 5 %.
        let s = stats_of(&fit_on(BONUS_HULL, &[], vec![], &[]));
        assert!(close(s.volley.kinetic, 105.0));
        assert!(close(s.cycle_ms, 10_000.0 * 0.95));
    }

    /// **A bomb launcher ignores `missileDamageMultiplier`**
    /// (`GetMissleDamageMultiplier:265-266` **[V]**).
    #[test]
    fn a_bomb_launcher_ignores_the_pilots_multiplier() {
        let s = fit(&[], vec![module(2, BCS, State::Online)], &[IMPLANT]);
        assert!(damage_multiplier(&s, 510) > 1.0);
        assert_eq!(damage_multiplier(&s, BOMB_LAUNCHER_GROUP), 1.0);
        assert_eq!(BOMB_LAUNCHER, 990_470_030, "the fixture row exists");
    }

    // -----------------------------------------------------------------
    // The world: a launcher fitted to a ship, fired through `on_cycles`
    // -----------------------------------------------------------------

    const ALICE: i64 = 90_047_001;
    const BOB: i64 = 90_047_002;
    const ALICE_SHIP: i64 = 100_047_001;
    const BOB_SHIP: i64 = 100_047_002;
    const LAUNCHER_ITEM: i64 = 100_047_101;
    const BCS_ITEM: i64 = 100_047_102;
    const IMPLANT_ITEM: i64 = 100_047_103;
    /// `flagHiSlot0`, `flagLoSlot0` [V].
    const HI_SLOT: i64 = 27;
    const LO_SLOT: i64 = 11;
    /// The fixture's cruiser stands in for the fixture hull both pilots fly.
    const FLOWN_HULL: i64 = 587;

    /// Alice, in a ship the table holds, with a launcher online in a high
    /// slot and `rounds` missiles in it; Bob next to her in the same hull.
    fn world() -> Registry {
        let text = FIXTURE.replace("type\t990470001\t25\t6", "type\t587\t25\t6");
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(Table::parse(&text).expect("parses")),
        );
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: FLOWN_HULL,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        let mut items = reg.store().items().to_vec();
        for i in items
            .iter_mut()
            .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
        {
            i.type_id = FLOWN_HULL;
            i.group_id = 25;
            i.category_id = crate::typedata::CATEGORY_SHIP;
        }
        items.push(crate::item::Item {
            item_id: LAUNCHER_ITEM,
            type_id: LAUNCHER,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT,
            quantity: 1,
            group_id: 510,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_048_000,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(LAUNCHER_ITEM, true);
        reg.apply_items(&[crate::item::Edit::Create {
            type_id: MISSILE,
            quantity: 5,
            to: crate::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT),
        }])
        .expect("loaded");
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        reg
    }

    fn train(reg: &mut Registry, skills: &[(i64, i64)]) {
        let entries = skills
            .iter()
            .map(|&(type_id, level)| crate::skill::SkillEntry {
                type_id,
                level,
                points: 0,
                rank: 1,
            })
            .collect();
        *reg.skills_mut(ALICE).expect("Alice has skills") = crate::skill::Skills::restore(entries);
    }

    fn add_item(reg: &mut Registry, item: crate::item::Item) {
        let mut items = reg.store().items().to_vec();
        items.push(item);
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_049_000,
            ..crate::store::Snapshot::default()
        });
    }

    fn fire(reg: &mut Registry) {
        let now = crate::pyutil::filetime_now();
        crate::combat::on_cycles(
            reg,
            vec![crate::modules::Cycle {
                character_id: ALICE,
                ship_id: ALICE_SHIP,
                module_id: LAUNCHER_ITEM,
                type_id: LAUNCHER,
                effect_id: crate::modules::EFFECT_USE_MISSILES,
                target_id: Some(BOB_SHIP),
                at: now,
                index: 0,
                charge_type_id: Some(MISSILE),
            }],
            now,
        );
    }

    /// **The whole path**: a trained pilot with a ballistic control system
    /// and an implant fires through `combat::on_cycles`; the missile in the
    /// air carries the sheet's speed, fuse, explosion and volley, and each
    /// change to the pilot (a skill level, an implant, a module going
    /// online) is a different number on the next launch with nothing
    /// invalidated by hand.
    #[test]
    fn a_launch_carries_the_pilots_dogma_and_follows_changes() {
        let mut reg = world();
        // Untrained, unfitted: the type's own numbers.
        fire(&mut reg);
        let (speed, volley, aoe, fuse) =
            reg.ext::<crate::missile::Flights>().first().expect("flies");
        assert_eq!((speed, fuse), (4000.0, 6.0));
        assert_eq!(
            volley,
            Damage {
                em: 20.0,
                explosive: 0.0,
                kinetic: 100.0,
                thermal: 0.0
            }
        );
        assert_eq!(
            aoe,
            Aoe {
                cloud: 200.0,
                velocity: 100.0,
                drf: 0.5
            }
        );

        // Trained: damage, flight time, velocity and explosion all move.
        train(
            &mut reg,
            &[
                (HEAVY, 5),
                (BOMBARDMENT, 4),
                (PROJECTION, 2),
                (PRECISION, 5),
                (NAVIGATION, 5),
                (LAUNCHER_OP, 0),
            ],
        );
        let s = stats(&reg, ALICE_SHIP, LAUNCHER_ITEM).expect("a launcher with a charge");
        assert!(close(s.speed, 4000.0 * 1.2));
        assert!(close(s.flight_s, 6.0 * 1.4));
        assert!(close(s.aoe.cloud, 200.0 * 0.75));
        assert!(close(s.aoe.velocity, 100.0 * 1.25));
        assert!(close(s.volley.kinetic, 100.0 * 1.25));
        // The number the combat code multiplies is the same one.
        let volley =
            crate::combat::volley_fitted(&reg, ALICE_SHIP, LAUNCHER_ITEM, LAUNCHER, Some(MISSILE))
                .expect("a volley");
        assert_eq!(volley, s.volley);
        assert_eq!(
            charge_for(&reg, ALICE_SHIP, LAUNCHER_ITEM, MISSILE),
            Some(s.charge)
        );

        // EWAR-2B: a guidance disruptor on the launching ship moves the
        // missile that flies, and not the fitting window.
        reg.ext_mut::<crate::ewar::Ewars>().factors_for_test(
            ALICE_SHIP,
            crate::ewar::Factors {
                missile_speed: 0.5,
                missile_fuse: 0.75,
                aoe_velocity: 0.9,
                aoe_cloud: 0.8,
                ..crate::ewar::Factors::default()
            },
        );
        let flown = charge_for(&reg, ALICE_SHIP, LAUNCHER_ITEM, MISSILE).expect("a charge");
        assert!(close(flown.speed, s.charge.speed * 0.5));
        assert!(close(flown.fuse_s, s.charge.fuse_s * 0.75));
        assert!(close(flown.aoe.velocity, s.charge.aoe.velocity * 0.9));
        assert!(close(flown.aoe.cloud, s.charge.aoe.cloud * 0.8));
        assert_eq!(flown.aoe.drf, s.charge.aoe.drf);
        assert_eq!(
            stats(&reg, ALICE_SHIP, LAUNCHER_ITEM),
            Some(s),
            "the window stays clean"
        );
        reg.ext_mut::<crate::ewar::Ewars>()
            .factors_for_test(ALICE_SHIP, crate::ewar::Factors::default());

        // A ballistic control system, online in a low slot, and an implant
        // in the clone: 1.1 x 1.05 on the pilot's multiplier.
        add_item(
            &mut reg,
            crate::item::Item {
                item_id: BCS_ITEM,
                type_id: BCS,
                owner_id: ALICE,
                location_id: ALICE_SHIP,
                flag_id: LO_SLOT,
                quantity: 1,
                group_id: 339,
                category_id: 7,
                singleton: true,
            },
        );
        reg.modules_mut().set_online(BCS_ITEM, false);
        let offline = stats(&reg, ALICE_SHIP, LAUNCHER_ITEM)
            .expect("stats")
            .multiplier;
        assert_eq!(offline, 1.0, "fitted but offline");
        reg.modules_mut().set_online(BCS_ITEM, true);
        add_item(
            &mut reg,
            crate::item::Item {
                item_id: IMPLANT_ITEM,
                type_id: IMPLANT,
                owner_id: ALICE,
                location_id: ALICE,
                flag_id: crate::dogma::FLAG_IMPLANT,
                quantity: 1,
                group_id: 300,
                category_id: 20,
                singleton: false,
            },
        );
        let s = stats(&reg, ALICE_SHIP, LAUNCHER_ITEM).expect("stats");
        assert!(close(s.multiplier, 1.1 * 1.05));
        assert!(close(s.volley.kinetic, 100.0 * 1.25 * 1.1 * 1.05));

        // And the missile that leaves now carries all of it.
        let _ = reg.take_pending();
        fire(&mut reg);
        let flights = reg.ext::<crate::missile::Flights>();
        assert_eq!(flights.len(), 2);
        let (speed, volley, aoe, fuse) = flights.last_probe().expect("two in the air");
        assert!(close(speed, s.speed) && close(fuse, s.flight_s));
        assert_eq!(volley, s.volley);
        assert_eq!(aoe, s.aoe);
    }

    /// A sheet with no charge in that module leaves the raw charge alone,
    /// and a modifier that would stop the missile flying is ignored.
    #[test]
    fn the_raw_charge_stands_when_the_sheet_cannot_improve_it() {
        let empty = evaluate(
            &table(),
            &Inputs {
                ship_type: HULL,
                modules: vec![launcher(1, None)],
                ..Inputs::default()
            },
        )
        .expect("evaluates");
        assert_eq!(fitted_charge(base_charge(), &empty, 1), base_charge());
        // Another module id: nothing there either.
        let loaded = fit(&[], vec![], &[]);
        assert_eq!(fitted_charge(base_charge(), &loaded, 77), base_charge());
        assert_eq!(fitted_charge(base_charge(), &loaded, 1), base_charge());
        // A modifier that would leave the missile unable to fly (a speed of
        // zero on the sheet) is ignored.
        let broken = FIXTURE.replace("37=4000.0;70", "37=0.0;70");
        assert_ne!(broken, FIXTURE);
        let sheet = evaluate(
            &Table::parse(&broken).expect("parses"),
            &Inputs {
                ship_type: HULL,
                modules: vec![launcher(1, Some(MISSILE))],
                ..Inputs::default()
            },
        )
        .expect("evaluates");
        assert_eq!(fitted_charge(base_charge(), &sheet, 1), base_charge());
    }
}
