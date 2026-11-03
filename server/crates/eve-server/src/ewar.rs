//! **Player electronic warfare: webs, ECM, sensor dampeners and target
//! painters** (K-EWAR). Scramblers and disruptors are `crate::tackle`'s
//! (W16 CP57) and are not repeated here; NPC webs are `crate::npc_ewar`'s.
//!
//! Marks as `docs/content/README.md` §2. The lane note is
//! `docs/lanes/k-ewar.md`.
//!
//! # Energy warfare (NEUT-NOS)
//!
//! `energyNeutralizerFalloff` (6187) and `energyNosferatuFalloff` (6197) are
//! the same family, with the client's buff slots `ewEnergyNeut` and
//! `ewEnergyVampire` [V, `BUFF_SLOTS_BY_EFFECT_ID`]. They act **once per
//! source cycle**, on its first reconcile, at the weight that cycle holds:
//! falloff, times the target's resistance (the module's 2138 names it; the
//! player modules carry no 2138, so the target's `energyWarfareResistance`
//! 2045, default 1, stands in [V typedata]), times `min(1, targetSig /
//! 2451)` when the module has `energyNeutralizerSignatureResolution` above
//! zero (only entity and structure neuts do).
//!
//! - **Neut**: attribute 97 times the weight comes out of the target's
//!   capacitor (`modules::drain_capacitor_by`; an NPC target has a
//!   capacitor of attributes 482 and 55).
//! - **Nos**: `min(attribute 90 x weight, target's capacitor, room in the
//!   source's)` moves from the target to the source, and **nothing moves**
//!   while the source holds at least the target's capacitor, unless
//!   `nosOverride` (1945) is above zero [V the module text, loc 91798: "will
//!   not drain your target's capacitor below your own capacitor level";
//!   **[A]** that the comparison is in GJ, not in percent. Falsified by a
//!   nosferatu that drains a fuller ship in the live client, or one that
//!   refuses a ship that is fuller by percent only].
//!
//! # The four modules [V]
//!
//! All four are target effects (`effectCategory` 2, offensive) whose duration
//! is attribute 73, range `maxRange` (54) and falloff **`falloffEffectiveness`
//! (2044)** (`dogmaeffects[6426, 6470, 6422, 6425]`); the client's guid and
//! buff-bar slot come from `dogma/effects/__init__.py` (`BUFF_SLOTS_BY_EFFECT_ID`,
//! lines 206-211):
//!
//! | effect | module | what it does to the target | `jammingType` |
//! |---|---|---|---|
//! | 6426 `remoteWebifierFalloff` | Stasis Webifier | `maxVelocity` (37) `postPercent` the module's `speedFactor` (20) | `webify` |
//! | 6470 `remoteECMFalloff` | ECM | breaks every lock for the cycle, by chance | `electronic` |
//! | 6422 `remoteSensorDampFalloff` | Remote Sensor Dampener | `maxTargetRange` (76) `postPercent` 309, `scanResolution` (564) `postPercent` 566 | `ewRemoteSensorDamp` |
//! | 6425 `remoteTargetPaintFalloff` | Target Painter | `signatureRadius` (552) `postPercent` 554 | `ewTargetPaint` |
//!
//! **The target attributes are not stackable** (`dogmaattributes[20, 37, 76,
//! 564, 552]`, `stackable = False`), so two webs, two dampeners or two
//! painters on one ship are stacking-penalised by `crate::dogma::apply`, as
//! the client's `nurfed` does [V]. A web and a painter never meet: they act on
//! different attributes. **The modifier rows of these four effects are empty
//! in the client's table (they are Python effects)**, so the arithmetic below
//! is the standard one [I]; its falsifiers are in the lane note.
//!
//! **Falloff** [I]: a source acts at full strength out to its `maxRange`, then
//! at `0.5^(((distance - range) / falloff)^2)` (the client's own table for
//! `falloffEffectiveness`), and not at all past [`FALLOFF_REACH`] falloffs
//! **[A]**. A module with no falloff stops at its range, as a scrambler does.
//! **Resistance** [V]: the module's attribute 2138 (`remoteResistanceID`)
//! names an attribute of the target (2115 for webs, 2253 for ECM, 2112 for
//! dampeners, 2114 for painters, default 1) that multiplies the strength.
//!
//! **ECM** [I]: each cycle of each jammer is one roll, once, on the cycle's
//! start: it jams with probability `jammer strength / sensor strength`, the
//! jammer's strength of the target's own sensor type (the target's strongest
//! of 208-211 against the jammer's 241, 239, 240, 238 by name), times the
//! falloff and resistance weight. A jam drops **every** lock the target holds
//! and refuses it new ones until the end of the cycle ([`lock_limit`]); the
//! target's client is told with `OnJamStart(source, module, target,
//! 'electronic', startTime, duration)` and `OnJamEnd` (`godma.py:250`,
//! `:262`; the HUD timer, `shipUI.py:1213`) [V shape, I server-sent: no local
//! emitter]. The roll is `drone_skills::roll(module, cycle, SALT)`, so a
//! replay of the same fight rolls the same.
//!
//! # Disruptors, boosters and computers (EWAR-2B, EWAR-2C)
//!
//! Four more kinds, one `Kind` each, on the same footing as the four above:
//! a tracking disruptor (6424) and a remote tracking computer (6428) on the
//! target's turrets (optimal, falloff and tracking speed, read by
//! `combat::turret_numbers` through [`gun_factors`]); a guidance disruptor
//! (6423) on its missiles (speed, flight, explosion velocity and radius, read
//! at launch through [`missile_factors`]); a remote sensor booster (6427) on
//! the dampener's lock range and scan resolution, with the opposite sign, and
//! on the sensor strengths the ECM roll is made against. The two disruptors
//! name no resistance in 2138; the effect row's own stands in ([`Kind::resistance`]).
//! The booster and the computer are help: they go through
//! `crimewatch::assistance`, not `aggression` ([`Kind::is_assist`]).
//! [`check_activation`] refuses a target by its 872 and 854 flags. Lane note:
//! `docs/lanes/ewar-2bc.md`.
//!
//! # ECM burst (EWAR-2E)
//!
//! A burst jammer (6714) has no target: the first moment of each cycle every
//! ship in its radius rolls like an ECM cycle, and one that loses has its
//! locks broken with no jam timer. See [`bursts`]. Lane note:
//! `docs/lanes/ewar-2e.md`.
//!
//! # What this module holds
//!
//! Like `tackle`, nothing is stored that `Modules::running_against` does not
//! already say: the [`SLOT`] holds only what each target's client was last
//! *told* (the factors, the icons and the jam), which is also what the seams
//! read ([`speed_factor`], [`signature_factor`], [`range_factor`],
//! [`scan_factor`], [`lock_limit`]). It is never persisted: a restart empties
//! every park and every running effect.
//!
//! The target may be a player, a bot or an NPC: all of them are in the roster
//! under their own id. A speed change on an NPC reaches its motion through
//! `Registry::mover`, on a pilot through `dogma::hull`.
//!
//! **Every act is aggression** (CW-1): each new cycle of a source, hit or
//! miss, goes through `crimewatch::aggression`, so a web on a capsuleer is a
//! weapons timer (and a crime where shooting would be), a web on an NPC an
//! NPC timer, and a web on CONCORD's ship in high sec a crime
//! (`concord::attack_on_law`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use eve_marshal::PyValue;

use crate::damage::BlueTime;
use crate::dogma::{Incoming, apply, op};
use crate::modules::{MSEC, Running};
use crate::notify::Push;
use crate::service::Registry;
use crate::tackle::{Data, Live};
use crate::world_ext::{Boxed, SlotSpec};

/// `remoteWebifierFalloff` [V].
pub const WEB: i64 = 6426;
/// `remoteECMFalloff` [V].
pub const ECM: i64 = 6470;
/// `remoteSensorDampFalloff` [V].
pub const DAMP: i64 = 6422;
/// `remoteTargetPaintFalloff` [V].
pub const PAINT: i64 = 6425;
/// `energyNeutralizerFalloff` [V].
pub const NEUT: i64 = 6187;
/// `energyNosferatuFalloff` [V].
pub const NOS: i64 = 6197;
/// `shipModuleTrackingDisruptor` [V].
pub const TRACKING: i64 = 6424;
/// `shipModuleGuidanceDisruptor` [V].
pub const GUIDANCE: i64 = 6423;
/// `remoteSensorBoostFalloff` [V].
pub const SENSOR_BOOST: i64 = 6427;
/// `shipModuleRemoteTrackingComputer` [V].
pub const REMOTE_TRACKING: i64 = 6428;

/// Dogma attributes [V] (`dogmaattributes`).
pub mod attr {
    pub const SPEED_FACTOR: i64 = 20;
    pub const MAX_VELOCITY: i64 = 37;
    pub const MAX_RANGE: i64 = 54;
    pub const MAX_TARGET_RANGE: i64 = 76;
    pub const SCAN_RESOLUTION: i64 = 564;
    pub const SIGNATURE_RADIUS: i64 = 552;
    pub const SIGNATURE_BONUS: i64 = 554;
    pub const RANGE_BONUS: i64 = 309;
    pub const RESOLUTION_BONUS: i64 = 566;
    /// `energyNeutralizerAmount`.
    pub const NEUT_AMOUNT: i64 = 97;
    /// `powerTransferAmount`: a nosferatu's.
    pub const NOS_AMOUNT: i64 = 90;
    /// `nosOverride`: above zero, a nosferatu drains a fuller ship too.
    pub const NOS_OVERRIDE: i64 = 1945;
    /// `energyWarfareResistance`: on the target, default 1.
    pub const ENERGY_RESISTANCE: i64 = 2045;
    /// `energyNeutralizerSignatureResolution`: on the module.
    pub const NEUT_SIGNATURE: i64 = 2451;
    /// `falloffEffectiveness`: the falloff of all four effects.
    pub const FALLOFF: i64 = 2044;
    /// `remoteResistanceID`: on the module, the target attribute that
    /// resists it.
    pub const RESISTANCE_ID: i64 = 2138;
    /// A tracking disruptor's or computer's bonus to a turret's `maxRange`
    /// (54), `falloff` (158) and `trackingSpeed` (160) [V
    /// `weaponDisruptionUtil`]; the bonus attributes are the module's own.
    pub const RANGE_DISRUPT: i64 = 351;
    pub const FALLOFF_DISRUPT: i64 = 349;
    pub const TRACKING_DISRUPT: i64 = 767;
    /// A guidance disruptor's bonus to a missile's `maxVelocity` (37),
    /// `explosionDelay` (281), `aoeVelocity` (653) and `aoeCloudSize` (654).
    pub const MISSILE_VELOCITY_BONUS: i64 = 547;
    pub const EXPLOSION_DELAY_BONUS: i64 = 596;
    pub const AOE_VELOCITY_BONUS: i64 = 847;
    pub const AOE_CLOUD_BONUS: i64 = 848;
    /// The turret attributes a tracking disruptor changes.
    pub const TURRET_FALLOFF: i64 = 158;
    pub const TURRET_TRACKING: i64 = 160;
    /// `remoteResistanceID` targets [V `dogmaeffects`]: a disruptor's, and a
    /// remote tracking computer's.
    pub const DISRUPT_RESISTANCE: i64 = 2113;
    pub const TRACKING_ASSIST_RESISTANCE: i64 = 2135;
    /// `disallowOffensiveModifiers`: on the target, refuses a hostile module.
    pub const DISALLOW_OFFENSIVE: i64 = 872;
    /// `disallowAssistance`: on the target, refuses a helpful one.
    pub const DISALLOW_ASSISTANCE: i64 = 854;
    /// `disallowAgainstEwImmuneTarget`: on an assistance module, it refuses
    /// a target with 872 [V description].
    pub const ASSIST_NEEDS_OPEN: i64 = 1798;
    pub const SCAN_RADAR: i64 = 208;
    pub const SCAN_LADAR: i64 = 209;
    pub const SCAN_MAGNETOMETRIC: i64 = 210;
    pub const SCAN_GRAVIMETRIC: i64 = 211;
    pub const JAM_GRAVIMETRIC: i64 = 238;
    pub const JAM_LADAR: i64 = 239;
    pub const JAM_MAGNETOMETRIC: i64 = 240;
    pub const JAM_RADAR: i64 = 241;
}

/// The target's sensor strength attribute and the jammer's strength
/// attribute for the same sensor type [V] (names in `dogmaattributes`).
const SENSORS: [(i64, i64); 4] = [
    (attr::SCAN_RADAR, attr::JAM_RADAR),
    (attr::SCAN_LADAR, attr::JAM_LADAR),
    (attr::SCAN_MAGNETOMETRIC, attr::JAM_MAGNETOMETRIC),
    (attr::SCAN_GRAVIMETRIC, attr::JAM_GRAVIMETRIC),
];

/// The percent attributes of a remote sensor booster on the module, one per
/// sensor type, in [`SENSORS`] order: radar, ladar, magnetometric,
/// gravimetric [V `sensorFalloff`].
const SENSOR_PERCENT: [i64; 4] = [1030, 1028, 1029, 1027];

/// **[A]** How many falloffs past its range a source still counts: at
/// three the factor is `0.5^9`, under 0.2%. Falsified by a module the live
/// client shows holding a lock effect past that.
pub const FALLOFF_REACH: f64 = 3.0;

/// The roll's salt, so the ECM stream differs from the NPC brain's.
pub const SALT: u64 = 0xEC31;

/// The ten kinds of effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Web,
    Ecm,
    Damp,
    Paint,
    /// An energy neutralizer (NEUT-NOS).
    Neut,
    /// An energy nosferatu (NEUT-NOS).
    Nos,
    /// A tracking disruptor: the target's turrets (EWAR-2B).
    Tracking,
    /// A guidance disruptor: the target's missiles (EWAR-2B).
    Guidance,
    /// A remote sensor booster: help, the opposite of [`Kind::Damp`]
    /// (EWAR-2C).
    SensorBoost,
    /// A remote tracking computer: help, the opposite of [`Kind::Tracking`]
    /// (EWAR-2C).
    RemoteTracking,
}

impl Kind {
    /// The kind of an effect id, `None` for any other.
    pub fn of(effect_id: i64) -> Option<Kind> {
        match effect_id {
            WEB => Some(Kind::Web),
            ECM => Some(Kind::Ecm),
            DAMP => Some(Kind::Damp),
            PAINT => Some(Kind::Paint),
            NEUT => Some(Kind::Neut),
            NOS => Some(Kind::Nos),
            TRACKING => Some(Kind::Tracking),
            GUIDANCE => Some(Kind::Guidance),
            SENSOR_BOOST => Some(Kind::SensorBoost),
            REMOTE_TRACKING => Some(Kind::RemoteTracking),
            _ => None,
        }
    }

    /// `jammingType` [V] (`buffBarConst.Slot_*`).
    pub fn jamming(self) -> &'static str {
        match self {
            Kind::Web => "webify",
            Kind::Ecm => "electronic",
            Kind::Damp => "ewRemoteSensorDamp",
            Kind::Paint => "ewTargetPaint",
            Kind::Neut => "ewEnergyNeut",
            Kind::Nos => "ewEnergyVampire",
            Kind::Tracking => "ewTrackingDisrupt",
            Kind::Guidance => "ewGuidanceDisrupt",
            Kind::SensorBoost => "sensorBooster",
            Kind::RemoteTracking => "remoteTracking",
        }
    }

    /// Help, not harm: no aggression on the target, and `crimewatch`'s
    /// assistance rules apply instead.
    pub fn is_assist(self) -> bool {
        matches!(self, Kind::SensorBoost | Kind::RemoteTracking)
    }

    /// The target attribute that resists the effect when the module names
    /// none (no 2138): a disruptor's and a tracking computer's are on the
    /// effect row in the client, which `typedata.tsv` drops [V
    /// `dogmaeffects`].
    pub fn resistance(self) -> Option<i64> {
        match self {
            Kind::Tracking | Kind::Guidance => Some(attr::DISRUPT_RESISTANCE),
            Kind::RemoteTracking => Some(attr::TRACKING_ASSIST_RESISTANCE),
            _ => None,
        }
    }

    /// An energy-warfare kind: it acts on the capacitor once per cycle.
    pub fn is_energy(self) -> bool {
        matches!(self, Kind::Neut | Kind::Nos)
    }
}

/// Whether `effect_id` is one of the ten.
pub fn is_ewar(effect_id: i64) -> bool {
    Kind::of(effect_id).is_some()
}

// ---------------------------------------------------------------------------
// The reads
// ---------------------------------------------------------------------------

/// A target's attribute: its own sheet's (fitting, skills, implants), else
/// its type's. A pilot's ship has a sheet; an NPC has its type only.
fn target_attr(w: &Registry, d: &dyn Data, ship: i64, attribute: i64) -> Option<f64> {
    d.ship_attr(w, ship, attribute).or_else(|| {
        let p = w.roster().by_ship(ship)?;
        let ty = w.pilot(p.character_id)?.ship_type_id;
        d.type_attr(w, ty, attribute)
    })
}

/// One effect that counts against a ship now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub running: Running,
    pub kind: Kind,
    /// Falloff times resistance, in `[0, 1]` (and above 1 for a negative
    /// resistance, which nothing here has).
    pub weight: f64,
}

/// `0.5^(((d - range) / falloff)^2)` [I], 1 inside the range, 0 past
/// [`FALLOFF_REACH`] falloffs; a source with no falloff stops at its range.
pub fn falloff(distance: f64, range: f64, falloff: f64) -> f64 {
    if distance <= range {
        return 1.0;
    }
    if falloff <= 0.0 || distance > range + FALLOFF_REACH * falloff {
        return 0.0;
    }
    0.5_f64.powf(((distance - range) / falloff).powi(2))
}

/// Whether `r` counts against `target` at `now`, and with what weight.
///
/// As `tackle::holds`: no deactivation has taken hold, the source's pilot is
/// still in that ship, in the target's system, the lock is in the graph;
/// then the falloff and the target's resistance.
fn weight_of(
    w: &Registry,
    r: &Running,
    target: &crate::space_state::Presence,
    now: BlueTime,
    d: &dyn Data,
) -> Option<f64> {
    if r.stop_at.is_some_and(|s| s <= now) {
        return None;
    }
    let source = w.roster().get(r.character_id)?;
    if source.ship_id != r.ship_id || source.solar_system_id != target.solar_system_id {
        return None;
    }
    if !w.locks().targets_of(r.ship_id).contains(&target.ship_id) {
        return None;
    }
    let range = d.module_attr(w, r, attr::MAX_RANGE).unwrap_or(0.0);
    let fall = d.module_attr(w, r, attr::FALLOFF).unwrap_or(0.0);
    let gap = crate::space_state::distance(source.motion.position, target.motion.position);
    let reach = if range > 0.0 {
        falloff(gap, range, fall)
    } else {
        1.0
    };
    if reach <= 0.0 {
        return None;
    }
    let named = d
        .module_attr(w, r, attr::RESISTANCE_ID)
        .filter(|id| *id > 0.0)
        .map(|id| id as i64);
    // A disruptor or a tracking computer names none either: the effect row's
    // own resistance stands in [V `dogmaeffects`].
    let named = named.or(Kind::of(r.effect.effect_id).and_then(Kind::resistance));
    let energy = Kind::of(r.effect.effect_id).is_some_and(Kind::is_energy);
    // A player neutralizer or nosferatu names no resistance (no 2138): the
    // target's 2045 stands in [V typedata].
    let named = named.or(energy.then_some(attr::ENERGY_RESISTANCE));
    let resist = named
        .and_then(|id| target_attr(w, d, target.ship_id, id))
        .unwrap_or(1.0);
    let sig = match d.module_attr(w, r, attr::NEUT_SIGNATURE) {
        Some(res) if energy && res > 0.0 => signature_fit(signature_of(w, d, target.ship_id), res),
        _ => 1.0,
    };
    Some(reach * resist.max(0.0) * sig)
}

/// `min(1, targetSig / resolution)`: a neutralizer built for big targets
/// bites a small one less [I, EvEJS; V that 2451 exists].
pub fn signature_fit(target_signature: f64, resolution: f64) -> f64 {
    if resolution <= 0.0 {
        return 1.0;
    }
    (target_signature / resolution).clamp(0.0, 1.0)
}

/// The ship's signature radius now: the sheet's or the hull's, painted.
pub fn signature_of(w: &Registry, d: &dyn Data, ship: i64) -> f64 {
    base_of(w, d, ship, attr::SIGNATURE_RADIUS, |w| {
        hull_of(w, ship).signature_radius
    }) * signature_factor(w, ship)
}

/// The ship's `energyWarfareResistance` (2045): 1 when it has none.
pub fn energy_resistance(w: &Registry, d: &dyn Data, ship: i64) -> f64 {
    target_attr(w, d, ship, attr::ENERGY_RESISTANCE).unwrap_or(1.0)
}

/// Every effect of the ten kinds holding against `ship` at `now`.
pub fn held(w: &Registry, ship: i64, now: BlueTime, d: &dyn Data) -> Vec<Held> {
    let Some(target) = w.roster().by_ship(ship).copied() else {
        return Vec::new();
    };
    // UNDOCK-INVULN: nothing holds a protected ship.
    if crate::invuln::ship_protected(w, ship, now) {
        return Vec::new();
    }
    w.modules()
        .running_against(ship)
        .into_iter()
        .filter_map(|r| {
            let kind = Kind::of(r.effect.effect_id)?;
            let weight = weight_of(w, &r, &target, now, d)?;
            Some(Held {
                running: r,
                kind,
                weight,
            })
        })
        .collect()
}

/// The ship's multipliers, 1.0 where nothing acts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Factors {
    /// On `maxVelocity`.
    pub speed: f64,
    /// On `signatureRadius`.
    pub signature: f64,
    /// On `maxTargetRange`.
    pub range: f64,
    /// On `scanResolution`.
    pub scan: f64,
    /// On its turrets' `maxRange` (54), `falloff` (158) and `trackingSpeed`
    /// (160): tracking disruptors against remote tracking computers.
    pub optimal: f64,
    pub gun_falloff: f64,
    pub tracking: f64,
    /// On its missiles' speed (37), flight time (281), explosion velocity
    /// (653) and explosion radius (654): guidance disruptors.
    pub missile_speed: f64,
    pub missile_fuse: f64,
    pub aoe_velocity: f64,
    pub aoe_cloud: f64,
    /// On its sensor strengths, in [`SENSORS`] order: remote sensor boosters.
    pub sensor: [f64; 4],
}

impl Default for Factors {
    fn default() -> Self {
        Factors {
            speed: 1.0,
            signature: 1.0,
            range: 1.0,
            scan: 1.0,
            optimal: 1.0,
            gun_falloff: 1.0,
            tracking: 1.0,
            missile_speed: 1.0,
            missile_fuse: 1.0,
            aoe_velocity: 1.0,
            aoe_cloud: 1.0,
            sensor: [1.0; 4],
        }
    }
}

/// **The stacking rule**: each group's `postPercent` modifiers, scaled by
/// their weights, as one penalised set per target attribute. A harmful and a
/// helpful module on one attribute share the set, and `dogma::apply` splits
/// it above and below 1.0 as the client's `nurfed` does.
pub fn factors(w: &Registry, found: &[Held], d: &dyn Data) -> Factors {
    let group = |kinds: &[Kind], attribute: i64| -> f64 {
        let mods: Vec<Incoming> = found
            .iter()
            .filter(|h| kinds.contains(&h.kind))
            .map(|h| Incoming {
                op: op::POST_PERCENT,
                value: d.module_attr(w, &h.running, attribute).unwrap_or(0.0) * h.weight,
                penalised: true,
            })
            .collect();
        apply(1.0, &mods)
    };
    let guns = [Kind::Tracking, Kind::RemoteTracking];
    let sensors = [Kind::SensorBoost];
    Factors {
        speed: group(&[Kind::Web], attr::SPEED_FACTOR),
        signature: group(&[Kind::Paint], attr::SIGNATURE_BONUS),
        range: group(&[Kind::Damp, Kind::SensorBoost], attr::RANGE_BONUS),
        scan: group(&[Kind::Damp, Kind::SensorBoost], attr::RESOLUTION_BONUS),
        optimal: group(&guns, attr::RANGE_DISRUPT),
        gun_falloff: group(&guns, attr::FALLOFF_DISRUPT),
        tracking: group(&guns, attr::TRACKING_DISRUPT),
        missile_speed: group(&[Kind::Guidance], attr::MISSILE_VELOCITY_BONUS),
        missile_fuse: group(&[Kind::Guidance], attr::EXPLOSION_DELAY_BONUS),
        aoe_velocity: group(&[Kind::Guidance], attr::AOE_VELOCITY_BONUS),
        aoe_cloud: group(&[Kind::Guidance], attr::AOE_CLOUD_BONUS),
        sensor: SENSOR_PERCENT.map(|a| group(&sensors, a)),
    }
}

/// **What a target's flags say to an activation** (EWAR-2C) [V
/// `dogma/effects/util.py`]: a ship with `disallowOffensiveModifiers` (872)
/// takes no hostile module, one with `disallowAssistance` (854) no helpful
/// one, and a helpful module with `disallowAgainstEwImmuneTarget` (1798)
/// refuses a target with 872. A target whose resistance to the effect is
/// under 1% is refused the same way **[A]** (`PreCheckResist` takes its key
/// from the caller; falsified by a capture with another key). Anything that
/// is not one of the ten kinds passes.
pub fn check_activation(
    w: &Registry,
    module: &crate::modules::ModuleType,
    effect_id: i64,
    target: Option<i64>,
) -> Result<(), crate::service::ServiceError> {
    check_activation_in(w, module, effect_id, target, &Live)
}

fn check_activation_in(
    w: &Registry,
    module: &crate::modules::ModuleType,
    effect_id: i64,
    target: Option<i64>,
    d: &dyn Data,
) -> Result<(), crate::service::ServiceError> {
    let (Some(kind), Some(ship)) = (Kind::of(effect_id), target) else {
        return Ok(());
    };
    // Only a ship in the roster has flags to read.
    if w.roster().by_ship(ship).is_none() {
        return Ok(());
    }
    let above = |attribute: i64| target_attr(w, d, ship, attribute).is_some_and(|v| v > 0.0);
    let refuse = |key: &str| Err(crate::modules::user_error(key, vec![]));
    if kind.is_assist() {
        if above(attr::DISALLOW_ASSISTANCE) {
            return refuse("DeniedActivateTargetAssistDisallowed");
        }
        if module
            .attribute(attr::ASSIST_NEEDS_OPEN)
            .is_some_and(|v| v > 0.0)
            && above(attr::DISALLOW_OFFENSIVE)
        {
            return refuse("DeniedActivateTargetModuleDisallowed");
        }
    } else if above(attr::DISALLOW_OFFENSIVE) {
        return refuse("DeniedActivateTargetModuleDisallowed");
    }
    let named = module
        .attribute(attr::RESISTANCE_ID)
        .filter(|id| *id > 0.0)
        .map(|id| id as i64)
        .or_else(|| kind.resistance());
    if let Some(resist) = named.and_then(|id| target_attr(w, d, ship, id))
        && resist < IMMUNE
    {
        return refuse("DeniedActivateTargetModuleDisallowed");
    }
    Ok(())
}

/// **[A]** Under this resistance a target is immune: the client's own
/// "1e-05 means immune" reads.
const IMMUNE: f64 = 0.01;

/// **The gun factors on `ship`'s turrets** as its client was last told:
/// optimal range, falloff and tracking speed, 1.0 untouched. The hit roll
/// reads them (`combat::turret_numbers`).
pub fn gun_factors(w: &Registry, ship: i64) -> (f64, f64, f64) {
    let f = told_factors(w, ship);
    (f.optimal, f.gun_falloff, f.tracking)
}

/// **The missile factors on `ship`'s launchers**: speed, flight time,
/// explosion velocity and explosion radius, 1.0 untouched. Sampled when a
/// missile launches **[A]**.
pub fn missile_factors(w: &Registry, ship: i64) -> (f64, f64, f64, f64) {
    let f = told_factors(w, ship);
    (f.missile_speed, f.missile_fuse, f.aoe_velocity, f.aoe_cloud)
}

/// The chance one cycle of the ECM `h` jams `ship` [I]: the jammer's
/// strength against the target's strongest sensor, times the weight; 0 for a
/// target with no sensor strength to read.
pub fn jam_chance(w: &Registry, h: &Held, ship: i64, d: &dyn Data) -> f64 {
    jam_chance_of(w, &h.running, h.weight, ship, d)
}

/// [`jam_chance`] for a module `r` and a `weight` made elsewhere: the ECM
/// burst has no lock and no falloff, only the target's resistance.
fn jam_chance_of(w: &Registry, r: &Running, weight: f64, ship: i64, d: &dyn Data) -> f64 {
    // A remote sensor booster raises the strength it is jammed against
    // (EWAR-2C).
    let boost = told_factors(w, ship).sensor;
    let strongest = SENSORS
        .iter()
        .zip(boost)
        .map(|((sensor, jam), f)| (target_attr(w, d, ship, *sensor).unwrap_or(0.0) * f, *jam))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let Some((sensor, jam)) = strongest.filter(|(s, _)| *s > 0.0) else {
        return 0.0;
    };
    let strength = d.module_attr(w, r, jam).unwrap_or(0.0);
    ((strength / sensor).clamp(0.0, 1.0) * weight).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// The state: what each target was told
// ---------------------------------------------------------------------------

/// `effects.ECMBurst` (`ECMBurstJammer`) [V `dogmaeffects`]. Not a [`Kind`]:
/// it has no target.
pub const BURST: i64 = 6714;
/// `ecmBurstRange`: the radius, on the module [V]. Surface to surface [A].
const BURST_RADIUS: i64 = 142;
/// The burst's resistance, on the target [V `dogmaeffects`].
const BURST_RESISTANCE: i64 = 2253;

/// **ECM burst** (EWAR-2E): the first moment of each cycle, each ship within
/// the radius rolls like an ECM cycle (strength against its strongest sensor,
/// times its resistance), and one that loses breaks every lock it holds. No
/// jam timer, no refused relock: the client's only words for it are "target
/// locks broken" (`OnECMBurst*`) [V]. A ship whose locks broke is told with
/// `OnCombatMessage`, and is an act of aggression against, per ship **[A]**
/// (owner O2 default; falsified by an owner choice of a safety refusal). A
/// roll that lost says and does nothing **[A]**; one that won against a ship
/// with no lock to break is aggression with no combat line **[A]** (#138).
fn bursts(w: &mut Registry, now: BlueTime, d: &dyn Data) {
    let running: Vec<Running> = w
        .modules()
        .running_all()
        .into_iter()
        .filter(|r| r.effect.effect_id == BURST)
        .collect();
    let live: BTreeSet<i64> = running.iter().map(|r| r.module_id).collect();
    w.ext_mut::<Ewars>().bursts.retain(|m, _| live.contains(m));
    for r in running {
        if r.stop_at.is_some_and(|s| s <= now) {
            continue;
        }
        let seen = w.ext::<Ewars>().bursts.get(&r.module_id).copied();
        if seen.is_some_and(|n| n >= r.cycles) {
            continue;
        }
        w.ext_mut::<Ewars>().bursts.insert(r.module_id, r.cycles);
        burst_cycle(w, &r, now, d);
    }
}

fn burst_cycle(w: &mut Registry, r: &Running, now: BlueTime, d: &dyn Data) {
    let Some(source) = w.roster().get(r.character_id).copied() else {
        return;
    };
    if source.ship_id != r.ship_id {
        return;
    }
    let radius = d.module_attr(w, r, BURST_RADIUS).unwrap_or(0.0);
    if radius <= 0.0 {
        return;
    }
    for id in w
        .roster()
        .others_in_system(source.solar_system_id, r.character_id)
    {
        let Some(target) = w.roster().get(id).copied() else {
            continue;
        };
        let ship = target.ship_id;
        // A cloaked ship is not seen, and an abandoned hull has no one to aim
        // at: neither takes a burst, so neither can start a crime.
        if ship == r.ship_id
            || crate::invuln::ship_protected(w, ship, now)
            || crate::cloak::is_cloaked(w, ship)
            || crate::pilotless::pilot_of(w, id).is_some()
        {
            continue;
        }
        match crate::module_range::gap_between(w, r.ship_id, ship) {
            Some(gap) if gap <= radius => {}
            _ => continue,
        }
        // A ship that refuses hostile modules (872) refuses the burst too, as
        // `check_activation` refuses a targeted one.
        if target_attr(w, d, ship, attr::DISALLOW_OFFENSIVE).is_some_and(|v| v > 0.0) {
            continue;
        }
        let resist = target_attr(w, d, ship, BURST_RESISTANCE).unwrap_or(1.0);
        if resist < IMMUNE {
            continue;
        }
        let chance = jam_chance_of(w, r, resist, ship, d);
        let salt = SALT.wrapping_add(ship as u64);
        if crate::drone_skills::roll(r.module_id, r.cycles, salt) >= chance {
            continue;
        }
        // The roll is the hit (#138): a ship it wins against is aggressed
        // whether or not it held a lock, as a targeted module's cycle is,
        // through the same crimewatch path (a capsuleer: weapons timer and,
        // in high sec, suspect or CONCORD by that path's own verdict; an
        // NPC: the NPC timer and its threat ledger; one's own fleet: that
        // path's own rule).
        let broke = !w.locks().targets_of(ship).is_empty();
        if broke {
            break_locks(w, ship, id);
        }
        crate::crimewatch::aggression(w, r.character_id, id, now, now);
        crate::npc_threat::on_act(w, r.character_id, id, 0.0, now);
        if !broke {
            continue;
        }
        if w.character(r.character_id).is_some() {
            schedule(
                w,
                CombatMessage {
                    to: r.character_id,
                    name: "OnECMBurstAttacker",
                    args: vec![
                        ("specialObject", PyValue::Int(ship)),
                        ("type", PyValue::Int(r.type_id)),
                    ],
                },
            );
        }
        if w.character(id).is_some() {
            schedule(
                w,
                CombatMessage {
                    to: id,
                    name: "OnECMBurstDefenderFromCharacter",
                    args: vec![
                        ("owner", PyValue::Int(r.character_id)),
                        ("type", PyValue::Int(r.type_id)),
                    ],
                },
            );
        }
    }
}

/// `OnCombatMessage(messageName, messageArgs)` [V `tactical.py:154`]: a line
/// of the combat log, to one pilot. `messageArgs` is a dict.
#[derive(Debug, Clone, PartialEq)]
pub struct CombatMessage {
    pub to: i64,
    pub name: &'static str,
    pub args: Vec<(&'static str, PyValue)>,
}

impl Push for CombatMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCombatMessage",
            vec![
                crate::pyutil::bstr(self.name),
                crate::pyutil::pydict(self.args.clone()),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// One icon on the target's buff bar: `(sourceBall, moduleID, jammingType)`.
type Icon = (i64, i64, &'static str);

/// What one target's client was last told.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    /// The target's roster key: its character, or an NPC's own id.
    key: i64,
    icons: BTreeSet<Icon>,
    /// Source module -> the cycles already counted (aggression, the roll).
    cycles: BTreeMap<i64, i64>,
    factors: Factors,
    /// The ECM cycles that jam it, each with the end of its cycle; the HUD
    /// timer of each is up until then.
    jams: BTreeMap<Icon, BlueTime>,
}

impl Told {
    fn new(key: i64) -> Told {
        Told {
            key,
            icons: BTreeSet::new(),
            cycles: BTreeMap::new(),
            factors: Factors::default(),
            jams: BTreeMap::new(),
        }
    }

    /// Nothing is acting on the ship any more.
    fn is_idle(&self) -> bool {
        self.icons.is_empty() && self.jams.is_empty() && self.factors == Factors::default()
    }
}

/// **The slot**: per target ship, what its client was last told.
#[derive(Debug, Default)]
pub struct Ewars {
    ships: BTreeMap<i64, Told>,
    /// ECM burst module -> the cycles already handled (EWAR-2E).
    bursts: BTreeMap<i64, i64>,
}

#[cfg(test)]
impl Ewars {
    /// A jam on `ship` until `until`, as a rolled ECM cycle leaves it.
    pub(crate) fn jam_for_test(&mut self, ship: i64, until: BlueTime) {
        let mut t = Told::new(0);
        t.jams.insert((0, 0, "electronic"), until);
        self.ships.insert(ship, t);
    }

    /// `ship` as told these `factors`, as a disruptor's cycle leaves it.
    pub(crate) fn factors_for_test(&mut self, ship: i64, factors: Factors) {
        let mut t = Told::new(0);
        t.factors = factors;
        self.ships.insert(ship, t);
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "ewar",
    make: |_| Box::new(Ewars::default()) as Boxed,
};

fn told_factors(w: &Registry, ship: i64) -> Factors {
    w.ext::<Ewars>()
        .ships
        .get(&ship)
        .map_or_else(Factors::default, |t| t.factors)
}

/// **The projected factor on one ship attribute** as `ship`'s client was
/// last told it: lock range, scan resolution, signature radius or one of
/// the [`SENSORS`]; 1.0 for anything else or an untouched ship.
pub fn told_factor_on(w: &Registry, ship: i64, attribute: i64) -> f64 {
    let f = told_factors(w, ship);
    match attribute {
        attr::MAX_TARGET_RANGE => f.range,
        attr::SCAN_RESOLUTION => f.scan,
        attr::SIGNATURE_RADIUS => f.signature,
        a => SENSORS
            .iter()
            .position(|(s, _)| *s == a)
            .map_or(1.0, |i| f.sensor[i]),
    }
}

/// **The web factor on `ship`** as its client was last told: 1.0 unwebbed.
/// `dogma::hull` and `Registry::mover` multiply `maxVelocity` by it.
pub fn speed_factor(w: &Registry, ship: i64) -> f64 {
    // A squadron's web or tackle holds too (`crate::fighters`, FT-F).
    told_factors(w, ship).speed * crate::fighters::web_factor(w, ship)
}

/// **The painter factor on `ship`**: 1.0 unpainted. `dogma::hull`, the
/// missile and the lock time read it.
pub fn signature_factor(w: &Registry, ship: i64) -> f64 {
    told_factors(w, ship).signature
}

/// **The dampener factor on `ship`'s lock range** (`maxTargetRange`): 1.0
/// undamped. The range sweep reads it.
pub fn range_factor(w: &Registry, ship: i64) -> f64 {
    told_factors(w, ship).range
}

/// **The dampener factor on `ship`'s `scanResolution`**: 1.0 undamped. The
/// lock time reads it.
pub fn scan_factor(w: &Registry, ship: i64) -> f64 {
    told_factors(w, ship).scan
}

/// Whether `ship` is jammed at `now`: an ECM cycle that took hold has not
/// ended.
pub fn jammed_at(w: &Registry, ship: i64, now: BlueTime) -> bool {
    w.ext::<Ewars>()
        .ships
        .get(&ship)
        .is_some_and(|t| t.jams.values().any(|until| *until > now))
        // A squadron's ECM too (`crate::fighters`, FT-F).
        || crate::fighters::jammed_at(w, ship, now)
}

/// [`jammed_at`] at the present (the wall clock, as `concord::jam_limit`).
pub fn jammed(w: &Registry, ship: i64) -> bool {
    jammed_at(w, ship, crate::pyutil::filetime_now())
}

/// **`targeting::lock`'s limit under ECM**: 0 while `ship` is jammed, else
/// `limit` itself.
pub fn lock_limit(w: &Registry, ship: i64, limit: f64) -> f64 {
    if jammed(w, ship) { 0.0 } else { limit }
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnJamStart(sourceBallID, moduleID, targetBallID, jammingType, startTime,
/// duration)` / `OnJamEnd(...four)` [V shape] (`godma.py:250`, `:262`): the
/// ECM timer on the jammed pilot's HUD. Server-sent [I]; to the target alone.
#[derive(Debug, Clone, PartialEq)]
pub struct JamPush {
    pub to: i64,
    pub source_ball: i64,
    pub module_id: i64,
    pub target_ball: i64,
    pub jamming: &'static str,
    /// `(startTime, duration in ms)` for a start, `None` for an end.
    pub start: Option<(BlueTime, i64)>,
}

impl Push for JamPush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let mut args = vec![
            PyValue::Int(self.source_ball),
            PyValue::Int(self.module_id),
            PyValue::Int(self.target_ball),
            crate::pyutil::bstr(self.jamming),
        ];
        match self.start {
            Some((at, ms)) => {
                args.push(PyValue::Int(at));
                args.push(PyValue::Int(ms));
                ("OnJamStart", args)
            }
            None => ("OnJamEnd", args),
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

fn schedule<P: Push + 'static>(w: &mut Registry, p: P) {
    w.schedule(Duration::ZERO, p);
}

fn ewar_push(w: &mut Registry, to: i64, icon: Icon, ship: i64, start: bool) {
    schedule(
        w,
        crate::tackle::Ewar {
            to,
            source_ball: icon.0,
            module_id: icon.1,
            target_ball: ship,
            jamming: icon.2,
            start,
        },
    );
}

fn attribute_push(w: &mut Registry, to: i64, ship: i64, id: i64, new: f64, old: f64, now: i64) {
    schedule(
        w,
        crate::modules::ModulePush::Attribute(crate::modules::AttributeChange {
            to: vec![to],
            owner_id: to,
            item_id: ship,
            attribute_id: id,
            time: now,
            new_value: new,
            old_value: old,
        }),
    );
}

/// `SetMaxSpeed(ship, max)` to everyone in the ship's system (the update's
/// grid role keeps it to the parks that hold the ball).
fn speed_push(w: &mut Registry, key: i64, ship: i64, max: f64) {
    let Some(here) = w.roster().get(key).copied() else {
        return;
    };
    let mut to = vec![key];
    to.extend(w.roster().others_in_system(here.solar_system_id, key));
    to.retain(|id| w.character(*id).is_some());
    let stamp = w.next_destiny_stamp();
    let action = crate::destiny::Action::SetMaxSpeed { ball: ship, max };
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, vec![action]) {
        w.schedule(Duration::ZERO, update);
    }
}

/// The ship's unmodified number: its sheet's, else the hull table's `base`.
fn base_of(
    w: &Registry,
    d: &dyn Data,
    ship: i64,
    attribute: i64,
    hull: impl Fn(&Registry) -> f64,
) -> f64 {
    d.ship_attr(w, ship, attribute)
        .filter(|v| *v > 0.0)
        .unwrap_or_else(|| hull(w))
}

/// The type of the ship `ship` (0 when nobody flies it).
fn type_of(w: &Registry, ship: i64) -> i64 {
    w.roster()
        .by_ship(ship)
        .and_then(|p| w.pilot(p.character_id))
        .map_or(0, |p| p.ship_type_id)
}

/// The hull table's row of the ship `ship`.
fn hull_of(w: &Registry, ship: i64) -> crate::destiny::hull::Hull {
    *crate::destiny::hull::of_in(w.content(), type_of(w, ship))
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's stage**, after `tackle`: [`reconcile`] on the
/// server's own data.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    reconcile(w, now, &Live);
}

/// Every ship an effect of the ten kinds is aimed at.
fn targets(w: &Registry) -> BTreeSet<i64> {
    w.modules()
        .running_all()
        .into_iter()
        .filter(|r| is_ewar(r.effect.effect_id))
        .filter_map(|r| r.target_id)
        .collect()
}

/// Bring every affected ship up to `now`, over `d`: the roll of each new
/// ECM cycle, the aggression of each new cycle, the icons, the factors and
/// what the target's client is told about each.
pub fn reconcile(w: &mut Registry, now: BlueTime, d: &dyn Data) {
    bursts(w, now, d);
    let mut ships = targets(w);
    ships.extend(w.ext::<Ewars>().ships.keys().copied());
    for ship in ships {
        reconcile_ship(w, ship, now, d);
    }
}

fn reconcile_ship(w: &mut Registry, ship: i64, now: BlueTime, d: &dyn Data) {
    let before = w.ext::<Ewars>().ships.get(&ship).cloned();
    let Some(key) = w.roster().by_ship(ship).map(|p| p.character_id) else {
        // Gone (docked, jumped, died, logged off): the session change
        // re-primes the ship's attributes. Nobody is left to tell.
        w.ext_mut::<Ewars>().ships.remove(&ship);
        return;
    };
    let pilot = w.character(key).is_some().then_some(key);
    let found = held(w, ship, now, d);
    if found.is_empty() && before.is_none() {
        return;
    }
    let mut told = before.clone().unwrap_or_else(|| Told::new(key));

    // A jam whose cycle ended: the HUD timer goes.
    let ended: Vec<Icon> = told
        .jams
        .iter()
        .filter(|(_, until)| **until <= now)
        .map(|(i, _)| *i)
        .collect();
    for icon in ended {
        told.jams.remove(&icon);
        if let Some(to) = pilot {
            schedule(
                w,
                JamPush {
                    to,
                    source_ball: icon.0,
                    module_id: icon.1,
                    target_ball: ship,
                    jamming: icon.2,
                    start: None,
                },
            );
        }
    }
    // Aggression, and the ECM roll, once per new cycle of each source.
    for h in &found {
        let r = h.running;
        if told
            .cycles
            .get(&r.module_id)
            .is_some_and(|n| *n >= r.cycles)
        {
            continue;
        }
        if h.kind.is_assist() {
            // Help is not an attack: the helper inherits the target's flags
            // (EWAR-2C).
            crate::crimewatch::assistance(w, r.character_id, ship, now, now);
            crate::npc_threat::assisted(w, r.ship_id, ship, 0.0, now);
            continue;
        }
        crate::crimewatch::aggression(w, r.character_id, key, now, now);
        crate::npc_threat::on_act(w, r.character_id, key, 0.0, now);
        if h.kind.is_energy() {
            energy_cycle(w, h, ship, now, d);
            continue;
        }
        if h.kind != Kind::Ecm {
            continue;
        }
        let chance = jam_chance(w, h, ship, d);
        if crate::drone_skills::roll(r.module_id, r.cycles, SALT) >= chance {
            continue;
        }
        let until = r.next_boundary();
        let icon: Icon = (r.ship_id, r.module_id, h.kind.jamming());
        told.jams.insert(icon, until);
        break_locks(w, ship, key);
        if let Some(to) = pilot {
            schedule(
                w,
                JamPush {
                    to,
                    source_ball: icon.0,
                    module_id: icon.1,
                    target_ball: ship,
                    jamming: icon.2,
                    start: Some((now, ((until - now).max(0)) / MSEC)),
                },
            );
        }
    }
    told.cycles = found
        .iter()
        .map(|h| (h.running.module_id, h.running.cycles))
        .collect();

    // The icons: web, dampener, painter and ECM (which also has its jam timer).
    // An ECM's icon is its *jam*, not its running module: `OnEwarEnd` is the
    // only thing that clears the client's `jammersByJammingType` set, and a
    // module that keeps cycling after its jam ended must not keep the icon
    // (the `fci_ewar` "the jam ends on B's buff bar" red).
    let icons: BTreeSet<Icon> = found
        .iter()
        .map(|h| {
            (
                h,
                (h.running.ship_id, h.running.module_id, h.kind.jamming()),
            )
        })
        .filter(|(h, icon)| h.kind != Kind::Ecm || told.jams.contains_key(icon))
        .map(|(_, icon)| icon)
        .collect();
    if let Some(to) = pilot {
        for icon in icons.difference(&told.icons) {
            ewar_push(w, to, *icon, ship, true);
        }
        for icon in told.icons.difference(&icons) {
            ewar_push(w, to, *icon, ship, false);
        }
    }
    told.icons = icons;

    // The factors.
    let now_factors = factors(w, &found, d);
    let old = told.factors;
    if now_factors.speed != old.speed {
        // Integrated to now at the old speed, before the slot changes what
        // `dogma::hull` and `mover` answer.
        w.advance_character(key);
    }
    told.factors = now_factors;
    let idle = told.is_idle();
    {
        let slot = &mut w.ext_mut::<Ewars>().ships;
        if idle {
            slot.remove(&ship);
        } else {
            slot.insert(ship, told);
        }
    }
    if now_factors.speed != old.speed {
        let base = base_of(w, d, ship, attr::MAX_VELOCITY, |w| {
            f64::from(hull_of(w, ship).max_velocity)
        });
        // `npc_ewar`'s own web multiplies in too.
        let other = crate::npc_ewar::speed_factor(w, ship);
        let (new_max, old_max) = (base * now_factors.speed * other, base * old.speed * other);
        speed_push(w, key, ship, new_max);
        if let Some(to) = pilot {
            attribute_push(w, to, ship, attr::MAX_VELOCITY, new_max, old_max, now);
        }
    }
    if let Some(to) = pilot {
        turret_pushes(w, to, ship, (&old, &now_factors), now, d);
        sensor_pushes(w, to, ship, (&old, &now_factors), now, d);
        if now_factors.range != old.range {
            let base = base_of(w, d, ship, attr::MAX_TARGET_RANGE, |w| {
                w.max_target_range_of(type_of(w, ship))
            });
            attribute_push(
                w,
                to,
                ship,
                attr::MAX_TARGET_RANGE,
                base * now_factors.range,
                base * old.range,
                now,
            );
        }
        if now_factors.scan != old.scan {
            let base = base_of(w, d, ship, attr::SCAN_RESOLUTION, |w| {
                w.scan_resolution_of(type_of(w, ship))
            });
            attribute_push(
                w,
                to,
                ship,
                attr::SCAN_RESOLUTION,
                base * now_factors.scan,
                base * old.scan,
                now,
            );
        }
        if now_factors.signature != old.signature {
            let base = base_of(w, d, ship, attr::SIGNATURE_RADIUS, |w| {
                hull_of(w, ship).signature_radius
            });
            attribute_push(
                w,
                to,
                ship,
                attr::SIGNATURE_RADIUS,
                base * now_factors.signature,
                base * old.signature,
                now,
            );
        }
    }
}

/// A fitted turret's number as its own ship's sheet has it, else its type's.
fn turret_base(w: &Registry, ship: i64, module: i64, type_id: i64, attribute: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship, module, attribute)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, attribute))
        .or_else(|| {
            crate::modules::find_in(w.content(), type_id).and_then(|t| t.attribute(attribute))
        })
}

/// The target's turrets' numbers, when a disruptor or computer moved them:
/// 54, 158 and 160 on each fitted turret, from the number the sheet gives
/// (which carries no projected effect) times the old and the new factor.
fn turret_pushes(
    w: &mut Registry,
    to: i64,
    ship: i64,
    (old, new): (&Factors, &Factors),
    now: BlueTime,
    _: &dyn Data,
) {
    let moved = [
        (attr::MAX_RANGE, old.optimal, new.optimal),
        (attr::TURRET_FALLOFF, old.gun_falloff, new.gun_falloff),
        (attr::TURRET_TRACKING, old.tracking, new.tracking),
    ];
    if moved.iter().all(|(_, o, n)| o == n) {
        return;
    }
    let turrets: Vec<(i64, i64)> = w
        .store()
        .contents_of(ship)
        .filter(|i| {
            crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
                && crate::modules::find_in(w.content(), i.type_id).and_then(|t| t.hardpoint)
                    == Some(crate::fitting::Hardpoint::Turret)
        })
        .map(|i| (i.item_id, i.type_id))
        .collect();
    for (module, type_id) in turrets {
        for (id, o, n) in moved {
            if o == n {
                continue;
            }
            let Some(base) = turret_base(w, ship, module, type_id, id).filter(|b| *b > 0.0) else {
                continue;
            };
            attribute_push(w, to, module, id, base * n, base * o, now);
        }
    }
}

/// The target's sensor strengths (208-211), when a booster moved them.
fn sensor_pushes(
    w: &mut Registry,
    to: i64,
    ship: i64,
    (old, new): (&Factors, &Factors),
    now: BlueTime,
    d: &dyn Data,
) {
    for (i, (sensor, _)) in SENSORS.iter().enumerate() {
        if old.sensor[i] == new.sensor[i] {
            continue;
        }
        let Some(base) = target_attr(w, d, ship, *sensor).filter(|b| *b > 0.0) else {
            continue;
        };
        attribute_push(
            w,
            to,
            ship,
            *sensor,
            base * new.sensor[i],
            base * old.sensor[i],
            now,
        );
    }
}

/// **One cycle of a neutralizer or nosferatu** on `ship` (NEUT-NOS), at the
/// weight the cycle holds. The module sits in `h`.
fn energy_cycle(w: &mut Registry, h: &Held, ship: i64, now: BlueTime, d: &dyn Data) {
    use crate::modules::{capacitor_at, drain_capacitor_by, give_capacitor_by};
    let r = h.running;
    match h.kind {
        Kind::Neut => {
            let amount = d.module_attr(w, &r, attr::NEUT_AMOUNT).unwrap_or(0.0) * h.weight;
            if let Some(p) = drain_capacitor_by(w, ship, amount, now).and_then(|m| m.push) {
                schedule(w, p);
            }
        }
        Kind::Nos => {
            let amount = d.module_attr(w, &r, attr::NOS_AMOUNT).unwrap_or(0.0) * h.weight;
            let (source_type, target_type) = (type_of(w, r.ship_id), type_of(w, ship));
            let (mine, room) = capacitor_at(w, r.ship_id, source_type, now);
            let (theirs, _) = capacitor_at(w, ship, target_type, now);
            let over = d.module_attr(w, &r, attr::NOS_OVERRIDE).unwrap_or(0.0) > 0.0;
            // **[A]** GJ, not percent (module doc).
            if !over && mine >= theirs {
                return;
            }
            let want = amount.min(theirs).min(room - mine);
            let Some(took) = drain_capacitor_by(w, ship, want, now) else {
                return;
            };
            if let Some(p) = took.push {
                schedule(w, p);
            }
            if let Some(p) = give_capacitor_by(w, r.ship_id, took.amount, now).and_then(|m| m.push)
            {
                schedule(w, p);
            }
        }
        _ => {}
    }
}

/// **The jam**: every lock `ship` holds is dropped now, on the clock, to
/// the locker (one `lost` each) and to each locked pilot (`otherlost`).
fn break_locks(w: &mut Registry, ship: i64, key: i64) {
    use crate::targeting::{Reason, TargetChange, What};
    for t in w.locks().targets_of(ship) {
        if !w.locks_mut().remove(ship, t) {
            continue;
        }
        if w.character(key).is_some() {
            schedule(
                w,
                TargetChange {
                    to: vec![key],
                    what: What::Lost,
                    ship: Some(t),
                    reason: Some(Reason::Unlocked),
                },
            );
        }
        if let Some(other) = w
            .roster()
            .by_ship(t)
            .map(|x| x.character_id)
            .filter(|c| w.character(*c).is_some())
        {
            schedule(
                w,
                TargetChange {
                    to: vec![other],
                    what: What::OtherLost,
                    ship: Some(ship),
                    reason: Some(Reason::Unlocked),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::modules::Effect;

    // Invented ids and round fixture values (lane rule): the attribute ids
    // are the client's own [V]; every number below is made up.
    const ALICE: i64 = 1_580_001;
    const BOB: i64 = 1_580_002;
    const CAROL: i64 = 1_580_003;
    const ALICE_SHIP: i64 = 158_000_001;
    const BOB_SHIP: i64 = 158_000_002;
    const CAROL_SHIP: i64 = 158_000_003;
    const BOB_MOD: i64 = 158_100_001;
    const CAROL_MOD: i64 = 158_100_002;

    /// Fixture module types.
    const WEB_T: i64 = 990_580_001;
    const ECM_T: i64 = 990_580_002;
    const DAMP_T: i64 = 990_580_003;
    const PAINT_T: i64 = 990_580_004;
    /// EWAR-2B/2C: a tracking disruptor, a guidance disruptor, a remote
    /// sensor booster and a remote tracking computer.
    const TD_T: i64 = 990_580_020;
    const GD_T: i64 = 990_580_021;
    const RSB_T: i64 = 990_580_022;
    const RTC_T: i64 = 990_580_023;
    /// EWAR-2E: an ECM burst, and its radius.
    const BURST_T: i64 = 990_580_030;
    const BURST_REACH: f64 = 1_500.0;
    /// Their bonuses, percent.
    const TD_RANGE_PCT: f64 = -20.0;
    const TD_FALLOFF_PCT: f64 = -30.0;
    const TD_TRACKING_PCT: f64 = -40.0;
    const RTC_RANGE_PCT: f64 = 30.0;
    const GD_SPEED_PCT: f64 = -50.0;
    const GD_FUSE_PCT: f64 = -25.0;
    const GD_AOE_VELOCITY_PCT: f64 = -10.0;
    const GD_AOE_CLOUD_PCT: f64 = -20.0;
    const RSB_LADAR_PCT: f64 = 100.0;
    const RSB_RANGE_PCT: f64 = 40.0;
    const RSB_SCAN_PCT: f64 = 20.0;
    /// NEUT-NOS: a neutralizer, a nosferatu, and a neutralizer with a
    /// signature resolution.
    const NEUT_T: i64 = 990_580_010;
    const NOS_T: i64 = 990_580_011;
    const BIG_NEUT_T: i64 = 990_580_012;
    /// Their amounts, GJ, and the target's signature.
    const NEUT_GJ: f64 = 30.0;
    const NOS_GJ: f64 = 50.0;
    const ALICE_SIG: f64 = 100.0;

    const RANGE: f64 = 10_000.0;
    const FALL: f64 = 5_000.0;
    /// A web's `speedFactor`, a painter's bonus, a dampener's two bonuses.
    const WEB_PCT: f64 = -50.0;
    const PAINT_PCT: f64 = 25.0;
    const DAMP_RANGE_PCT: f64 = -30.0;
    const DAMP_SCAN_PCT: f64 = -20.0;
    /// Alice's ladar strength, and the jammer's, per type.
    const SENSOR: f64 = 6.0;
    const JAM_LADAR: f64 = 3.0;
    const JAM_RADAR: f64 = 99.0;
    const ALICE_TOP_SPEED: f64 = 400.0;
    /// The resistance attribute the fixture modules name.
    const RESIST_ATTR: i64 = 2115;

    struct Fixture {
        /// Alice's value of [`RESIST_ATTR`].
        resist: f64,
        /// The jammer's ladar strength.
        jam: f64,
        /// Alice's 872 and 854.
        no_offense: f64,
        no_help: f64,
    }

    const PLAIN: Fixture = Fixture {
        resist: 1.0,
        jam: JAM_LADAR,
        no_offense: 0.0,
        no_help: 0.0,
    };

    impl Data for Fixture {
        fn ship_attr(&self, _: &Registry, ship_id: i64, attribute_id: i64) -> Option<f64> {
            if ship_id != ALICE_SHIP {
                return None;
            }
            match attribute_id {
                attr::SCAN_LADAR => Some(SENSOR),
                attr::SCAN_RADAR => Some(1.0),
                attr::DISALLOW_OFFENSIVE => Some(self.no_offense),
                attr::DISALLOW_ASSISTANCE => Some(self.no_help),
                attr::MAX_VELOCITY => Some(ALICE_TOP_SPEED),
                attr::SIGNATURE_RADIUS => Some(ALICE_SIG),
                RESIST_ATTR
                | attr::ENERGY_RESISTANCE
                | attr::DISRUPT_RESISTANCE
                | BURST_RESISTANCE
                | attr::TRACKING_ASSIST_RESISTANCE => Some(self.resist),
                _ => None,
            }
        }
        fn module_attr(&self, w: &Registry, r: &Running, attribute_id: i64) -> Option<f64> {
            if attribute_id == attr::JAM_LADAR && (r.type_id == ECM_T || r.type_id == BURST_T) {
                return Some(self.jam);
            }
            self.type_attr(w, r.type_id, attribute_id)
        }
        fn type_attr(&self, _: &Registry, type_id: i64, attribute_id: i64) -> Option<f64> {
            match (type_id, attribute_id) {
                (
                    WEB_T | ECM_T | DAMP_T | PAINT_T | NEUT_T | NOS_T | BIG_NEUT_T | TD_T | GD_T
                    | RSB_T | RTC_T,
                    attr::MAX_RANGE,
                ) => Some(RANGE),
                (
                    WEB_T | ECM_T | DAMP_T | PAINT_T | NEUT_T | NOS_T | BIG_NEUT_T | TD_T | GD_T
                    | RSB_T | RTC_T,
                    attr::FALLOFF,
                ) => Some(FALL),
                (BURST_T, BURST_RADIUS) => Some(BURST_REACH),
                (TD_T, attr::RANGE_DISRUPT) => Some(TD_RANGE_PCT),
                (TD_T, attr::FALLOFF_DISRUPT) => Some(TD_FALLOFF_PCT),
                (TD_T, attr::TRACKING_DISRUPT) => Some(TD_TRACKING_PCT),
                (RTC_T, attr::RANGE_DISRUPT) => Some(RTC_RANGE_PCT),
                (GD_T, attr::MISSILE_VELOCITY_BONUS) => Some(GD_SPEED_PCT),
                (GD_T, attr::EXPLOSION_DELAY_BONUS) => Some(GD_FUSE_PCT),
                (GD_T, attr::AOE_VELOCITY_BONUS) => Some(GD_AOE_VELOCITY_PCT),
                (GD_T, attr::AOE_CLOUD_BONUS) => Some(GD_AOE_CLOUD_PCT),
                (RSB_T, 1028) => Some(RSB_LADAR_PCT),
                (RSB_T, attr::RANGE_BONUS) => Some(RSB_RANGE_PCT),
                (RSB_T, attr::RESOLUTION_BONUS) => Some(RSB_SCAN_PCT),
                (NEUT_T | BIG_NEUT_T, attr::NEUT_AMOUNT) => Some(NEUT_GJ),
                (NOS_T, attr::NOS_AMOUNT) => Some(NOS_GJ),
                (BIG_NEUT_T, attr::NEUT_SIGNATURE) => Some(2.0 * ALICE_SIG),
                (WEB_T | ECM_T | DAMP_T | PAINT_T, attr::RESISTANCE_ID) => Some(RESIST_ATTR as f64),
                (RSB_T, attr::RESISTANCE_ID) => Some(attr::TRACKING_ASSIST_RESISTANCE as f64),
                (WEB_T, attr::SPEED_FACTOR) => Some(WEB_PCT),
                (PAINT_T, attr::SIGNATURE_BONUS) => Some(PAINT_PCT),
                (DAMP_T, attr::RANGE_BONUS) => Some(DAMP_RANGE_PCT),
                (DAMP_T, attr::RESOLUTION_BONUS) => Some(DAMP_SCAN_PCT),
                (ECM_T, attr::JAM_RADAR) => Some(JAM_RADAR),
                _ => None,
            }
        }
    }

    fn effect(effect_id: i64) -> Effect {
        Effect {
            effect_id,
            name: "fixture",
            category: crate::modules::CATEGORY_TARGET,
            guid: "",
            is_offensive: true,
            duration_attribute: 73,
            warp_safe: None,
        }
    }

    fn now() -> i64 {
        crate::pyutil::filetime_now()
    }

    fn far() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(3_600)
    }

    /// Alice, Bob and Carol in one system, Bob and Carol 1 km from Alice,
    /// each locking Alice.
    fn world() -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
            ],
            ..SandboxConfig::default()
        });
        for c in [ALICE, BOB, CAROL] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [(ALICE, 0.0), (BOB, 1_000.0), (CAROL, -1_000.0)] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
            p.since = crate::space_state::now_seconds();
        }
        reg.locks_mut().add(BOB_SHIP, ALICE_SHIP);
        reg.locks_mut().add(CAROL_SHIP, ALICE_SHIP);
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    /// `who` runs a module of `type_id` with `effect_id` against `target`,
    /// started a moment before `now`.
    fn run(reg: &mut Registry, who: (i64, i64, i64), type_id: i64, effect_id: i64, target: i64) {
        let (character_id, ship_id, module_id) = who;
        reg.modules_mut().insert_running(Running {
            character_id,
            ship_id,
            module_id,
            type_id,
            effect: effect(effect_id),
            target_id: Some(target),
            started: now() - MSEC,
            period: 5_000 * MSEC,
            repeat: 1000,
            cycles: 1,
            stop_at: None,
        });
    }

    fn bob(reg: &mut Registry, type_id: i64, effect_id: i64, target: i64) {
        run(reg, (BOB, BOB_SHIP, BOB_MOD), type_id, effect_id, target);
    }

    fn carol(reg: &mut Registry, type_id: i64, effect_id: i64, target: i64) {
        run(
            reg,
            (CAROL, CAROL_SHIP, CAROL_MOD),
            type_id,
            effect_id,
            target,
        );
    }

    type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

    fn pushes(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(far())
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (name, args, p.audience())
            })
            .collect()
    }

    fn attribute_told(sent: &[Sent], id: i64) -> Option<(f64, f64)> {
        sent.iter()
            .find(|(n, a, _)| {
                *n == "OnModuleAttributeChange" && a.get(2) == Some(&PyValue::Int(id))
            })
            .map(|(_, a, _)| match (&a[4], &a[5]) {
                (PyValue::Real(n), PyValue::Real(o)) => (*n, *o),
                other => panic!("not reals: {other:?}"),
            })
    }

    /// The falloff curve: full inside the range, half at one falloff past
    /// it, nothing past [`FALLOFF_REACH`] of them, and a module with no
    /// falloff stops at its range.
    #[test]
    fn falloff_is_full_inside_half_at_one_falloff_and_cut_beyond_reach() {
        assert_eq!(falloff(RANGE - 1.0, RANGE, FALL), 1.0);
        assert_eq!(falloff(RANGE, RANGE, FALL), 1.0);
        assert!((falloff(RANGE + FALL, RANGE, FALL) - 0.5).abs() < 1e-12);
        assert!(falloff(RANGE + 2.0 * FALL, RANGE, FALL) < 0.07);
        assert_eq!(
            falloff(RANGE + FALLOFF_REACH * FALL + 1.0, RANGE, FALL),
            0.0
        );
        assert_eq!(falloff(RANGE + 1.0, RANGE, 0.0), 0.0, "no falloff");
        assert_eq!(Kind::of(WEB), Some(Kind::Web));
        assert_eq!(Kind::of(39), None, "a disruptor is tackle's");
    }

    /// A web: `postPercent` of the module's `speedFactor` on top speed; a
    /// second web is stacking-penalised, so it slows less than the first.
    #[test]
    fn a_web_cuts_speed_and_a_second_is_penalised() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, WEB_T, WEB, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!((f.speed - 0.5).abs() < 1e-12, "{}", f.speed);
        assert_eq!((f.signature, f.range, f.scan), (1.0, 1.0, 1.0));
        carol(&mut reg, WEB_T, WEB, ALICE_SHIP);
        let two = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN).speed;
        assert!(two < 0.5, "a second web still slows: {two}");
        assert!(two > 0.25, "but is penalised, not multiplied: {two}");
        let want = apply(
            1.0,
            &[WEB_PCT, WEB_PCT].map(|value| Incoming {
                op: op::POST_PERCENT,
                value,
                penalised: true,
            }),
        );
        assert!((two - want).abs() < 1e-12);
    }

    /// A painter raises the signature radius, a dampener cuts the lock range
    /// and the scan resolution; neither touches speed.
    #[test]
    fn a_painter_raises_signature_and_a_dampener_cuts_range_and_resolution() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, PAINT_T, PAINT, ALICE_SHIP);
        carol(&mut reg, DAMP_T, DAMP, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!((f.signature - 1.25).abs() < 1e-12);
        assert!((f.range - 0.7).abs() < 1e-12);
        assert!((f.scan - 0.8).abs() < 1e-12);
        assert_eq!(f.speed, 1.0);
    }

    /// Falloff scales the percentage, and the target's resistance
    /// (attribute 2138 names it) scales it again.
    #[test]
    fn falloff_and_resistance_scale_the_effect() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, WEB_T, WEB, ALICE_SHIP);
        // Resisted by half: -25%.
        let half = Fixture {
            resist: 0.5,
            jam: JAM_LADAR,
            ..PLAIN
        };
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &half), &half);
        assert!((f.speed - 0.75).abs() < 1e-12);
        // One falloff past the range: half the strength, -25%.
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += RANGE + FALL - 1_000.0;
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!((f.speed - 0.75).abs() < 1e-9, "{}", f.speed);
        // Past the reach: nothing holds.
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += 10.0 * FALL;
        assert!(held(&reg, ALICE_SHIP, t, &PLAIN).is_empty());
    }

    /// A source counts only while it holds: lock lost, not aimed there.
    #[test]
    fn a_source_counts_only_while_it_holds() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, WEB_T, WEB, ALICE_SHIP);
        assert_eq!(held(&reg, CAROL_SHIP, t, &PLAIN).len(), 0, "not Carol");
        reg.locks_mut().remove(BOB_SHIP, ALICE_SHIP);
        assert!(held(&reg, ALICE_SHIP, t, &PLAIN).is_empty(), "lock lost");
    }

    /// The target's client is told: the buff-bar icon, the new top speed
    /// (attribute 37 and `SetMaxSpeed`); once the web ends, all of it back.
    /// A tick with nothing changed says nothing.
    #[test]
    fn the_target_is_told_when_a_web_starts_and_ends() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, WEB_T, WEB, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        let icon = sent
            .iter()
            .find(|(n, ..)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(
            icon.1,
            vec![
                PyValue::Int(BOB_SHIP),
                PyValue::Int(BOB_MOD),
                PyValue::Int(ALICE_SHIP),
                crate::pyutil::bstr("webify"),
            ]
        );
        assert_eq!(icon.2, Some(vec![ALICE]));
        assert_eq!(
            attribute_told(&sent, attr::MAX_VELOCITY),
            Some((ALICE_TOP_SPEED * 0.5, ALICE_TOP_SPEED))
        );
        assert!(
            sent.iter().any(|(n, ..)| *n == "DoDestinyUpdate"),
            "SetMaxSpeed to the system"
        );
        assert_eq!(speed_factor(&reg, ALICE_SHIP), 0.5);

        reconcile(&mut reg, t, &PLAIN);
        assert!(pushes(&mut reg).is_empty(), "nothing changed, nothing said");

        crate::modules::stop(&mut reg, BOB_MOD, t, None);
        let _ = pushes(&mut reg);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        assert!(
            sent.iter()
                .any(|(n, a, _)| *n == "OnEwarEnd" && a[3] == crate::pyutil::bstr("webify"))
        );
        assert_eq!(
            attribute_told(&sent, attr::MAX_VELOCITY),
            Some((ALICE_TOP_SPEED, ALICE_TOP_SPEED * 0.5))
        );
        assert_eq!(speed_factor(&reg, ALICE_SHIP), 1.0);
    }

    /// Painter and dampener tell the client their attributes and icons.
    #[test]
    fn painter_and_dampener_are_told_to_the_client() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, PAINT_T, PAINT, ALICE_SHIP);
        carol(&mut reg, DAMP_T, DAMP, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        let types: Vec<PyValue> = sent
            .iter()
            .filter(|(n, ..)| *n == "OnEwarStart")
            .map(|(_, a, _)| a[3].clone())
            .collect();
        assert!(types.contains(&crate::pyutil::bstr("ewTargetPaint")));
        assert!(types.contains(&crate::pyutil::bstr("ewRemoteSensorDamp")));
        assert_eq!(
            attribute_told(&sent, attr::MAX_TARGET_RANGE).map(|(n, o)| n / o),
            Some(0.7)
        );
        assert!(attribute_told(&sent, attr::SCAN_RESOLUTION).is_some());
        assert!(attribute_told(&sent, attr::SIGNATURE_RADIUS).is_some());
        assert!((signature_factor(&reg, ALICE_SHIP) - 1.25).abs() < 1e-12);
        assert!((range_factor(&reg, ALICE_SHIP) - 0.7).abs() < 1e-12);
        assert!((scan_factor(&reg, ALICE_SHIP) - 0.8).abs() < 1e-12);
    }

    /// The jam chance is the jammer's strength of the **target's sensor
    /// type** over the sensor's strength, times the weight.
    #[test]
    fn the_jam_chance_reads_the_targets_sensor_type() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, ECM_T, ECM, ALICE_SHIP);
        let found = held(&reg, ALICE_SHIP, t, &PLAIN);
        // Alice is a ladar ship (6); the jammer's radar strength (99) is
        // beside the point; its ladar strength is 3.
        assert_eq!(jam_chance(&reg, &found[0], ALICE_SHIP, &PLAIN), 0.5);
        let resisted = Fixture {
            resist: 0.5,
            jam: JAM_LADAR,
            ..PLAIN
        };
        let found = held(&reg, ALICE_SHIP, t, &resisted);
        assert_eq!(jam_chance(&reg, &found[0], ALICE_SHIP, &resisted), 0.25);
        let strong = Fixture {
            resist: 1.0,
            jam: 60.0,
            ..PLAIN
        };
        let found = held(&reg, ALICE_SHIP, t, &strong);
        assert_eq!(jam_chance(&reg, &found[0], ALICE_SHIP, &strong), 1.0);
    }

    /// A jam that takes hold drops every lock the target holds, refuses it
    /// new ones until the cycle ends, and tells it (`OnJamStart`, then
    /// `OnJamEnd`). A jammer too weak to ever win does nothing.
    #[test]
    fn a_jam_breaks_every_lock_and_refuses_new_ones_for_the_cycle() {
        let t = now();
        let sure = Fixture {
            resist: 1.0,
            jam: 1_000.0,
            ..PLAIN
        };
        let mut reg = world();
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        reg.locks_mut().add(ALICE_SHIP, CAROL_SHIP);
        bob(&mut reg, ECM_T, ECM, ALICE_SHIP);
        assert!(!jammed_at(&reg, ALICE_SHIP, t));
        reconcile(&mut reg, t, &sure);
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty(), "locks gone");
        assert!(jammed_at(&reg, ALICE_SHIP, t));
        assert_eq!(lock_limit(&reg, ALICE_SHIP, 5.0), 0.0);
        assert_eq!(lock_limit(&reg, BOB_SHIP, 5.0), 5.0, "only the target");
        let sent = pushes(&mut reg);
        let start = sent
            .iter()
            .find(|(n, ..)| *n == "OnJamStart")
            .expect("OnJamStart");
        assert_eq!(start.2, Some(vec![ALICE]));
        assert_eq!(start.1[3], crate::pyutil::bstr("electronic"));
        // EWAR-ICONS (a): the buff-bar icon beside the timer, to the target.
        let icon = sent
            .iter()
            .find(|(n, ..)| *n == "OnEwarStart")
            .expect("OnEwarStart electronic");
        assert_eq!(icon.1[3], crate::pyutil::bstr("electronic"));
        assert_eq!(icon.2, Some(vec![ALICE]));
        assert!(
            matches!(start.1[5], PyValue::Int(ms) if (4_900..=5_100).contains(&ms)),
            "ms left in the cycle: {:?}",
            start.1[5]
        );
        assert_eq!(
            sent.iter().filter(|(n, ..)| *n == "OnTarget").count(),
            4,
            "a lost to Alice and an otherlost to the pilot, for each of two locks"
        );
        // The next tick says nothing: the cycle was rolled once.
        reconcile(&mut reg, t + MSEC, &sure);
        assert!(pushes(&mut reg).is_empty());
        // The cycle's end.
        reconcile(&mut reg, t + 6_000 * MSEC, &sure);
        let sent = pushes(&mut reg);
        assert!(
            sent.iter()
                .any(|(n, a, _)| *n == "OnJamEnd" && a[3] == crate::pyutil::bstr("electronic"))
        );
        // The buff-bar icon goes with the jam, though the module still cycles.
        assert!(
            sent.iter()
                .any(|(n, a, _)| *n == "OnEwarEnd" && a[3] == crate::pyutil::bstr("electronic")),
            "OnEwarEnd electronic when the jam ends: {sent:?}"
        );
        assert!(!jammed_at(&reg, ALICE_SHIP, t + 6_000 * MSEC));

        let weak = Fixture {
            resist: 1.0,
            jam: 0.0,
            ..PLAIN
        };
        let mut reg = world();
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        bob(&mut reg, ECM_T, ECM, ALICE_SHIP);
        reconcile(&mut reg, t, &weak);
        assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);
        assert!(!jammed_at(&reg, ALICE_SHIP, t));
    }

    /// Every act is aggression: a web on a capsuleer is the weapons timer
    /// that keeps the webber on the gate.
    #[test]
    fn a_web_is_aggression() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, WEB_T, WEB, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!(
            crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err(),
            "the webber has a weapons timer"
        );
        assert!(crate::crimewatch::refuse_jump(&reg, Some(CAROL), t).is_ok());
    }

    /// An NPC is a target too: webbed it slows (the factor `Registry::mover`
    /// reads), jammed it loses its lock and may not lock again.
    #[test]
    fn an_npc_can_be_webbed_and_jammed() {
        let t = now();
        let sure = Fixture {
            resist: 1.0,
            jam: 1_000.0,
            ..PLAIN
        };
        let mut reg = world();
        let p = reg.roster().get(ALICE).expect("in space");
        let (system, at) = (p.solar_system_id, p.motion.position);
        let id = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, at).expect("an id");
        assert!(reg.locks_mut().add(id, ALICE_SHIP));
        assert!(reg.locks_mut().add(BOB_SHIP, id));
        bob(&mut reg, WEB_T, WEB, id);
        carol(&mut reg, ECM_T, ECM, id);
        assert!(reg.locks_mut().add(CAROL_SHIP, id));
        // The rat is a ladar ship in the fixture, as Alice is.
        struct Rat<'a>(&'a Fixture, i64);
        impl Data for Rat<'_> {
            fn ship_attr(&self, w: &Registry, ship_id: i64, a: i64) -> Option<f64> {
                if ship_id == self.1 && a == attr::SCAN_LADAR {
                    return Some(SENSOR);
                }
                self.0.ship_attr(w, ship_id, a)
            }
            fn module_attr(&self, w: &Registry, r: &Running, a: i64) -> Option<f64> {
                self.0.module_attr(w, r, a)
            }
            fn type_attr(&self, w: &Registry, ty: i64, a: i64) -> Option<f64> {
                self.0.type_attr(w, ty, a)
            }
        }
        let d = Rat(&sure, id);
        reconcile(&mut reg, t, &d);
        assert_eq!(speed_factor(&reg, id), 0.5);
        assert!(jammed_at(&reg, id, t));
        assert!(
            reg.locks().targets_of(id).is_empty(),
            "the rat's lock on Alice broke"
        );
        assert!(
            crate::crimewatch::logoff_hold(&reg, BOB, t).is_some(),
            "webbing an NPC is aggression too (the NPC timer)"
        );
    }

    /// #137: a tracking disruptor aimed at a rat reaches the factors its gun
    /// reads (the reach the brain uses), and ends with the module.
    #[test]
    fn a_tracking_disruptor_on_a_rat_reaches_its_gun_and_ends() {
        let t = now();
        let mut reg = world();
        let p = reg.roster().get(ALICE).expect("in space");
        let (system, at) = (p.solar_system_id, p.motion.position);
        let id = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, at).expect("an id");
        assert!(reg.locks_mut().add(BOB_SHIP, id));
        assert_eq!(crate::combat::entity_gun_reach(&reg, id), None);
        bob(&mut reg, TD_T, TRACKING, id);
        reconcile(&mut reg, t, &PLAIN);
        let reach = crate::combat::entity_gun_reach(&reg, id).expect("cut");
        assert!(close(reach, 0.8), "{reach}");
        crate::modules::stop(&mut reg, BOB_MOD, t, None);
        reconcile(&mut reg, t, &PLAIN);
        assert_eq!(gun_factors(&reg, id), (1.0, 1.0, 1.0));
        assert_eq!(crate::combat::entity_gun_reach(&reg, id), None);
    }

    // ---- NEUT-NOS ----------------------------------------------------------

    /// Alice's and Bob's capacitor now, GJ.
    fn cap(reg: &Registry, ship: i64, t: i64) -> f64 {
        crate::modules::capacitor_at(reg, ship, 0, t).0
    }

    fn fill(reg: &mut Registry, ship: i64, value: f64, t: i64) {
        reg.modules_mut().set_capacitor(ship, value, t);
    }

    fn drains(sent: &[Sent], ship_ball: i64) -> usize {
        sent.iter()
            .filter(|(n, a, _)| {
                *n == "OnModuleAttributeChange"
                    && a.get(2) == Some(&PyValue::Int(18))
                    && a.get(1) == Some(&PyValue::Int(ship_ball))
            })
            .count()
    }

    #[test]
    fn the_energy_kinds_and_their_slots() {
        assert_eq!(Kind::of(NEUT), Some(Kind::Neut));
        assert_eq!(Kind::of(NOS), Some(Kind::Nos));
        assert_eq!(Kind::Neut.jamming(), "ewEnergyNeut");
        assert_eq!(Kind::Nos.jamming(), "ewEnergyVampire");
        assert!(is_ewar(NEUT) && is_ewar(NOS));
    }

    /// One cycle in range takes the module's amount, tells the pilot once
    /// (attribute 18) and puts the icon up once; the same cycle drains once.
    #[test]
    fn a_neutralizer_drains_the_capacitor_once_per_cycle() {
        let t = now();
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        bob(&mut reg, NEUT_T, NEUT, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        assert!((cap(&reg, ALICE_SHIP, t) - (100.0 - NEUT_GJ)).abs() < 1e-9);
        assert_eq!(attribute_charge(&sent), Some((70.0, 100.0)));
        assert_eq!(drains(&sent, ALICE_SHIP), 1);
        let icon = sent
            .iter()
            .find(|(n, ..)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(icon.1[3], crate::pyutil::bstr("ewEnergyNeut"));
        assert_eq!(icon.2, Some(vec![ALICE]));
        // The same cycle does not drain twice.
        reconcile(&mut reg, t, &PLAIN);
        assert!(pushes(&mut reg).is_empty());
        assert!((cap(&reg, ALICE_SHIP, t) - 70.0).abs() < 1e-9);
        // The next cycle does.
        let mut r = *reg.modules().running_all().first().expect("running");
        r.cycles = 2;
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, t, &PLAIN);
        assert!((cap(&reg, ALICE_SHIP, t) - 40.0).abs() < 1e-9);
        // It clips at empty.
        r.cycles = 3;
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, t, &PLAIN);
        r.cycles = 4;
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, t, &PLAIN);
        assert_eq!(cap(&reg, ALICE_SHIP, t), 0.0);
    }

    fn attribute_charge(sent: &[Sent]) -> Option<(f64, f64)> {
        attribute_told(sent, 18)
    }

    #[test]
    fn a_neutralizer_weakens_in_falloff_and_by_resistance() {
        let t = now();
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        bob(&mut reg, NEUT_T, NEUT, ALICE_SHIP);
        // One falloff past the range: half.
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += RANGE + FALL - 1_000.0;
        reconcile(&mut reg, t, &PLAIN);
        assert!((cap(&reg, ALICE_SHIP, t) - (100.0 - NEUT_GJ / 2.0)).abs() < 1e-9);
        // Resisted by half (2045 = 0.5; the module names no 2138).
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        bob(&mut reg, NEUT_T, NEUT, ALICE_SHIP);
        let half = Fixture {
            resist: 0.5,
            jam: JAM_LADAR,
            ..PLAIN
        };
        reconcile(&mut reg, t, &half);
        assert!((cap(&reg, ALICE_SHIP, t) - (100.0 - NEUT_GJ / 2.0)).abs() < 1e-9);
    }

    /// 2451 on the module: a target under that signature is hit
    /// proportionally less.
    #[test]
    fn a_signature_resolution_scales_the_drain() {
        let t = now();
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        bob(&mut reg, BIG_NEUT_T, NEUT, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!((cap(&reg, ALICE_SHIP, t) - (100.0 - NEUT_GJ / 2.0)).abs() < 1e-9);
        assert_eq!(signature_fit(300.0, 200.0), 1.0);
        assert_eq!(signature_fit(50.0, 200.0), 0.25);
    }

    #[test]
    fn a_nosferatu_moves_capacitor_only_to_the_emptier_source() {
        let t = now();
        // Bob 40, Alice 100: Alice loses 50, Bob gains it.
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        fill(&mut reg, BOB_SHIP, 40.0, t);
        bob(&mut reg, NOS_T, NOS, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        assert!((cap(&reg, ALICE_SHIP, t) - 50.0).abs() < 1e-9);
        assert!((cap(&reg, BOB_SHIP, t) - 90.0).abs() < 1e-9);
        assert_eq!((drains(&sent, ALICE_SHIP), drains(&sent, BOB_SHIP)), (1, 1));
        let icon = sent
            .iter()
            .find(|(n, ..)| *n == "OnEwarStart")
            .expect("OnEwarStart");
        assert_eq!(icon.1[3], crate::pyutil::bstr("ewEnergyVampire"));

        // Bob already holds more than Alice: nothing moves.
        let mut reg = world();
        fill(&mut reg, ALICE_SHIP, 100.0, t);
        fill(&mut reg, BOB_SHIP, 110.0, t);
        bob(&mut reg, NOS_T, NOS, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!((cap(&reg, ALICE_SHIP, t) - 100.0).abs() < 1e-9);
        assert!((cap(&reg, BOB_SHIP, t) - 110.0).abs() < 1e-9);

        // With 1945 > 0 it moves anyway, as far as Bob's room allows.
        struct Over;
        impl Data for Over {
            fn ship_attr(&self, w: &Registry, s: i64, a: i64) -> Option<f64> {
                PLAIN.ship_attr(w, s, a)
            }
            fn module_attr(&self, w: &Registry, r: &Running, a: i64) -> Option<f64> {
                (a == attr::NOS_OVERRIDE)
                    .then_some(1.0)
                    .or_else(|| PLAIN.module_attr(w, r, a))
            }
            fn type_attr(&self, w: &Registry, ty: i64, a: i64) -> Option<f64> {
                PLAIN.type_attr(w, ty, a)
            }
        }
        reconcile(&mut reg, t, &Over);
        let mut r = *reg.modules().running_all().first().expect("running");
        r.cycles = 2;
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, t, &Over);
        let room = cap(&reg, BOB_SHIP, t);
        assert!((room - 120.0).abs() < 1e-9, "full, not over: {room}");
        assert!(
            (cap(&reg, ALICE_SHIP, t) - 90.0).abs() < 1e-9,
            "only the room"
        );
    }

    /// A rat has a capacitor of its own, which a player neutralizer
    /// drains; and the neut is aggression the rats register.
    #[test]
    fn a_neutralizer_drains_an_npc_and_makes_its_pilot_an_aggressor() {
        let t = now();
        let mut reg = world();
        let p = reg.roster().get(ALICE).expect("in space");
        let (system, at) = (p.solar_system_id, p.motion.position);
        let id = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, at).expect("an id");
        assert!(reg.locks_mut().add(BOB_SHIP, id));
        let full = crate::npc::FIXTURE_RAT.hull.capacitor_capacity;
        assert_eq!(cap_of(&reg, id, t), full);
        bob(&mut reg, NEUT_T, NEUT, id);
        reconcile(&mut reg, t, &PLAIN);
        assert!((cap_of(&reg, id, t) - (full - NEUT_GJ)).abs() < 1e-9);
        assert!(
            reg.ext::<crate::npc_threat::Threat>()
                .standing(id, BOB_SHIP, t)
                .is_some(),
            "a neut is an act the rat holds against Bob"
        );
        assert!(crate::crimewatch::logoff_hold(&reg, BOB, t).is_some());
    }

    fn cap_of(reg: &Registry, npc: i64, t: i64) -> f64 {
        let ty = reg.npcs().get(npc).expect("an npc").kind.type_id;
        crate::modules::capacitor_at(reg, npc, ty, t).0
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn penalised(values: &[f64]) -> f64 {
        let mods: Vec<Incoming> = values
            .iter()
            .map(|value| Incoming {
                op: op::POST_PERCENT,
                value: *value,
                penalised: true,
            })
            .collect();
        apply(1.0, &mods)
    }

    /// EWAR-2B: a tracking disruptor scales the three gun numbers by its
    /// bonuses and nothing else; two are penalised.
    #[test]
    fn a_tracking_disruptor_cuts_the_guns_and_two_are_penalised() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(f.optimal, 0.8) && close(f.gun_falloff, 0.7) && close(f.tracking, 0.6));
        assert_eq!(
            (f.speed, f.signature, f.range, f.scan, f.missile_speed),
            (1.0, 1.0, 1.0, 1.0, 1.0)
        );
        carol(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        let two = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(two.optimal, penalised(&[TD_RANGE_PCT, TD_RANGE_PCT])));
        assert!(two.optimal < 0.8 && two.optimal > 0.64, "{}", two.optimal);
    }

    /// Past its range the weight follows the falloff, and the target's
    /// impedance (2113) scales the percent again.
    #[test]
    fn a_tracking_disruptor_weakens_in_falloff_and_by_impedance() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        let half = Fixture {
            resist: 0.5,
            ..PLAIN
        };
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &half), &half);
        assert!(close(f.optimal, 0.9) && close(f.tracking, 0.8), "{f:?}");
        let p = reg.roster_mut().get_mut(BOB).expect("in space");
        p.motion.position[0] += RANGE + FALL - 1_000.0;
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!((f.optimal - 0.9).abs() < 1e-9, "one falloff past: half");
    }

    /// A remote tracking computer helps: it shares the disruptor's set,
    /// where the stacking rule splits harm and help.
    #[test]
    fn a_tracking_computer_and_a_disruptor_are_two_penalised_sides() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        carol(&mut reg, RTC_T, REMOTE_TRACKING, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(f.optimal, penalised(&[TD_RANGE_PCT, RTC_RANGE_PCT])));
        assert!(
            close(f.optimal, 0.8 * 1.3),
            "one of each side is no penalty"
        );
        assert!(
            close(f.gun_falloff, 0.7),
            "the computer has no falloff bonus"
        );
    }

    /// A guidance disruptor moves the four missile numbers and no gun one.
    #[test]
    fn a_guidance_disruptor_changes_only_the_missiles() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, GD_T, GUIDANCE, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(f.missile_speed, 0.5) && close(f.missile_fuse, 0.75));
        assert!(close(f.aoe_velocity, 0.9) && close(f.aoe_cloud, 0.8));
        assert_eq!((f.optimal, f.gun_falloff, f.tracking), (1.0, 1.0, 1.0));
        reconcile(&mut reg, t, &PLAIN);
        assert_eq!(gun_factors(&reg, ALICE_SHIP), (1.0, 1.0, 1.0));
        let (s, fuse, v, c) = missile_factors(&reg, ALICE_SHIP);
        assert!(close(s, 0.5) && close(fuse, 0.75) && close(v, 0.9) && close(c, 0.8));
    }

    /// A tracking disruptor reaches the gun factors the hit roll reads and
    /// leaves the missile ones alone.
    #[test]
    fn a_tracking_disruptor_reaches_the_gun_seam_only() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let (o, f, tr) = gun_factors(&reg, ALICE_SHIP);
        assert!(close(o, 0.8) && close(f, 0.7) && close(tr, 0.6));
        assert_eq!(missile_factors(&reg, ALICE_SHIP), (1.0, 1.0, 1.0, 1.0));
        assert_eq!(gun_factors(&reg, BOB_SHIP), (1.0, 1.0, 1.0), "not Bob");
    }

    /// The four new kinds draw their buff-bar slots on start and end.
    #[test]
    fn the_new_kinds_have_their_buff_bar_slots() {
        for (kind, slot) in [
            (Kind::Tracking, "ewTrackingDisrupt"),
            (Kind::Guidance, "ewGuidanceDisrupt"),
            (Kind::SensorBoost, "sensorBooster"),
            (Kind::RemoteTracking, "remoteTracking"),
        ] {
            assert_eq!(kind.jamming(), slot);
        }
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        carol(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        let starts: Vec<PyValue> = sent
            .iter()
            .filter(|(n, ..)| *n == "OnEwarStart")
            .map(|(_, a, _)| a[3].clone())
            .collect();
        assert!(starts.contains(&crate::pyutil::bstr("ewTrackingDisrupt")));
        assert!(starts.contains(&crate::pyutil::bstr("sensorBooster")));
        crate::modules::stop(&mut reg, BOB_MOD, t, None);
        let _ = pushes(&mut reg);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        let ends: Vec<PyValue> = sent
            .iter()
            .filter(|(n, ..)| *n == "OnEwarEnd")
            .map(|(_, a, _)| a[3].clone())
            .collect();
        assert_eq!(ends, vec![crate::pyutil::bstr("ewTrackingDisrupt")]);
        assert_eq!(gun_factors(&reg, ALICE_SHIP), (1.0, 1.0, 1.0));
    }

    /// A cycle of a tracking disruptor is aggression: a weapons timer.
    #[test]
    fn a_tracking_disruptor_cycle_is_aggression() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, TD_T, TRACKING, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err());
    }

    /// EWAR-2C: a sensor booster raises range, resolution and the sensor it
    /// names; with a dampener on the same ship both sides are penalised.
    #[test]
    fn a_sensor_booster_raises_what_a_dampener_cuts() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(f.range, 1.4) && close(f.scan, 1.2), "{f:?}");
        assert!(close(f.sensor[1], 2.0), "ladar is the second sensor");
        assert_eq!((f.sensor[0], f.sensor[2], f.sensor[3]), (1.0, 1.0, 1.0));
        carol(&mut reg, DAMP_T, DAMP, ALICE_SHIP);
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &PLAIN), &PLAIN);
        assert!(close(f.range, penalised(&[RSB_RANGE_PCT, DAMP_RANGE_PCT])));
        assert!(close(f.scan, penalised(&[RSB_SCAN_PCT, DAMP_SCAN_PCT])));
    }

    /// A boosted sensor makes the ECM less likely to land; the impedance
    /// (2135) halves the boost.
    #[test]
    fn a_sensor_booster_lowers_the_jam_chance() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, ECM_T, ECM, ALICE_SHIP);
        let before = jam_chance(
            &reg,
            &held(&reg, ALICE_SHIP, t, &PLAIN)[0],
            ALICE_SHIP,
            &PLAIN,
        );
        assert!(close(before, JAM_LADAR / SENSOR));
        carol(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let ecm = held(&reg, ALICE_SHIP, t, &PLAIN)
            .into_iter()
            .find(|h| h.kind == Kind::Ecm)
            .expect("the jammer");
        let after = jam_chance(&reg, &ecm, ALICE_SHIP, &PLAIN);
        assert!(close(after, JAM_LADAR / (SENSOR * 2.0)), "{after}");

        let mut reg = world();
        carol(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        let half = Fixture {
            resist: 0.5,
            ..PLAIN
        };
        let f = factors(&reg, &held(&reg, ALICE_SHIP, t, &half), &half);
        assert!(close(f.sensor[1], 1.5) && close(f.range, 1.2), "{f:?}");
    }

    /// The boosted pilot is told the new sensor strength (209, ladar).
    #[test]
    fn the_boosted_pilot_is_told_the_sensor_strength() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        assert_eq!(
            attribute_told(&sent, attr::SCAN_LADAR),
            Some((SENSOR * 2.0, SENSOR))
        );
        assert_eq!(attribute_told(&sent, attr::SCAN_RADAR), None);
        assert!(attribute_told(&sent, attr::MAX_TARGET_RANGE).is_some());
    }

    /// Help is not an attack: no weapons timer for the helper.
    #[test]
    fn helping_is_not_aggression() {
        let t = now();
        let mut reg = world();
        bob(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        carol(&mut reg, RTC_T, REMOTE_TRACKING, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_ok());
        assert!(crate::crimewatch::refuse_jump(&reg, Some(CAROL), t).is_ok());
    }

    /// Helping a flagged pilot passes his timer on (`crimewatch::assistance`).
    #[test]
    fn helping_a_flagged_pilot_inherits_the_flag() {
        let t = now();
        let mut reg = world();
        crate::crimewatch::aggression(&mut reg, ALICE, CAROL, t, t);
        bob(&mut reg, RSB_T, SENSOR_BOOST, ALICE_SHIP);
        reconcile(&mut reg, t, &PLAIN);
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err());
    }

    fn module_type(attributes: &'static [(i64, f64)]) -> crate::modules::ModuleType {
        crate::modules::ModuleType {
            type_id: 990_580_090,
            name: "fixture",
            effect: None,
            attributes,
            rack: crate::fitting::Rack::High,
            hardpoint: None,
        }
    }

    /// The refusal's key, `None` for a pass.
    fn key(r: Result<(), crate::service::ServiceError>) -> Option<String> {
        let e = r.err()?;
        let crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. }) = &e else {
            return Some(format!("{e:?}"));
        };
        let PyValue::Tuple(h) = &**header else {
            return Some(format!("{e:?}"));
        };
        let Some(PyValue::Tuple(call)) = h.get(1) else {
            return Some(format!("{e:?}"));
        };
        match call.first() {
            Some(PyValue::Bytes(k)) => Some(String::from_utf8_lossy(k).into_owned()),
            _ => Some(format!("{e:?}")),
        }
    }

    /// The target's flags and impedance refuse an activation by name.
    #[test]
    fn the_target_flags_refuse_an_activation() {
        let reg = world();
        let plain = module_type(&[]);
        let go = |m: &crate::modules::ModuleType, effect: i64, d: &Fixture| {
            key(check_activation_in(&reg, m, effect, Some(ALICE_SHIP), d))
        };
        let says = |k: Option<String>, name: &str| k.is_some_and(|k| k.contains(name));
        assert_eq!(go(&plain, TRACKING, &PLAIN), None);
        assert_eq!(go(&plain, SENSOR_BOOST, &PLAIN), None);
        assert_eq!(go(&plain, 39, &PLAIN), None, "not ours");
        let sealed = Fixture {
            no_offense: 1.0,
            ..PLAIN
        };
        let module = "DeniedActivateTargetModuleDisallowed";
        assert!(says(go(&plain, TRACKING, &sealed), module));
        assert_eq!(go(&plain, SENSOR_BOOST, &sealed), None, "help passes 872");
        let strict = module_type(&[(attr::ASSIST_NEEDS_OPEN, 1.0)]);
        assert!(says(go(&strict, SENSOR_BOOST, &sealed), module));
        let no_help = Fixture {
            no_help: 1.0,
            ..PLAIN
        };
        assert!(says(
            go(&plain, REMOTE_TRACKING, &no_help),
            "DeniedActivateTargetAssistDisallowed"
        ));
        assert_eq!(go(&plain, TRACKING, &no_help), None, "harm passes 854");
        let immune = Fixture {
            resist: 0.0,
            ..PLAIN
        };
        assert!(says(go(&plain, GUIDANCE, &immune), module));
        assert!(says(go(&plain, REMOTE_TRACKING, &immune), module));
        assert_eq!(
            key(check_activation_in(&reg, &plain, TRACKING, None, &immune)),
            None
        );
    }

    /// Bob runs an ECM burst (no target), started a moment before `now`.
    fn burst(reg: &mut Registry, cycles: i64) {
        reg.modules_mut().insert_running(Running {
            character_id: BOB,
            ship_id: BOB_SHIP,
            module_id: BOB_MOD,
            type_id: BURST_T,
            effect: effect(BURST),
            target_id: None,
            started: now() - MSEC,
            period: 5_000 * MSEC,
            repeat: 1000,
            cycles,
            stop_at: None,
        });
    }

    fn sure() -> Fixture {
        Fixture {
            jam: 1_000.0,
            ..PLAIN
        }
    }

    fn combat_lines(sent: &[Sent]) -> Vec<&Sent> {
        sent.iter()
            .filter(|(n, ..)| *n == "OnCombatMessage")
            .collect()
    }

    /// A burst breaks the locks of a ship in its radius, once per cycle, and
    /// leaves the jam state alone: it may relock at once.
    #[test]
    fn a_burst_breaks_locks_in_the_radius_without_a_jam() {
        let t = now();
        let mut reg = world();
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        reg.locks_mut().add(ALICE_SHIP, CAROL_SHIP);
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &sure());
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty(), "locks gone");
        assert!(!jammed_at(&reg, ALICE_SHIP, t), "no jam");
        assert_eq!(lock_limit(&reg, ALICE_SHIP, 5.0), 5.0, "relock allowed");
        // The source's own lock is untouched.
        assert!(reg.locks().targets_of(BOB_SHIP).contains(&ALICE_SHIP));
        let sent = pushes(&mut reg);
        assert!(!sent.iter().any(|(n, ..)| *n == "OnJamStart"));
        assert_eq!(sent.iter().filter(|(n, ..)| *n == "OnTarget").count(), 4);
        // Aggression: the burster has a weapons timer.
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err());
        // The same cycle again: nothing.
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        reconcile(&mut reg, t + MSEC, &sure());
        assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);
        // The next cycle rolls again.
        burst(&mut reg, 2);
        reconcile(&mut reg, t + 5_000 * MSEC, &sure());
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty());
    }

    /// #138: a burst that wins against a ship holding no lock still aggresses
    /// its owner (the weapons timer, the log-off hold); one that loses does
    /// not.
    #[test]
    fn a_burst_that_wins_aggresses_a_ship_with_no_lock() {
        let t = now();
        let mut reg = world();
        assert!(reg.locks().targets_of(ALICE_SHIP).is_empty());
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &Fixture { jam: 0.0, ..PLAIN });
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_ok());
        assert!(crate::crimewatch::logoff_hold(&reg, ALICE, t).is_none());
        // The next cycle wins.
        burst(&mut reg, 2);
        reconcile(&mut reg, t + 5_000 * MSEC, &sure());
        assert!(crate::crimewatch::refuse_jump(&reg, Some(BOB), t).is_err());
        assert!(
            crate::crimewatch::logoff_hold(&reg, ALICE, t + 5_000 * MSEC).is_some(),
            "the one it hit is in combat too"
        );
    }

    /// #138: a rat in the radius is hit by the NPC rule: the NPC timer on
    /// the burster and the rat's ledger.
    #[test]
    fn a_burst_that_wins_aggresses_a_rat_by_the_npc_rule() {
        let t = now();
        let mut reg = world();
        let p = reg.roster().get(ALICE).expect("in space");
        let (system, at) = (p.solar_system_id, p.motion.position);
        let id = crate::npc::spawn(&mut reg, &crate::npc::FIXTURE_RAT, system, at).expect("an id");
        struct Rat<'a>(&'a Fixture, i64);
        impl Data for Rat<'_> {
            fn ship_attr(&self, w: &Registry, ship_id: i64, a: i64) -> Option<f64> {
                if ship_id == self.1 && a == attr::SCAN_LADAR {
                    return Some(SENSOR);
                }
                self.0.ship_attr(w, ship_id, a)
            }
            fn module_attr(&self, w: &Registry, r: &Running, a: i64) -> Option<f64> {
                self.0.module_attr(w, r, a)
            }
            fn type_attr(&self, w: &Registry, ty: i64, a: i64) -> Option<f64> {
                self.0.type_attr(w, ty, a)
            }
        }
        let f = sure();
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &Rat(&f, id));
        assert!(
            reg.ext::<crate::npc_threat::Threat>()
                .standing(id, BOB_SHIP, t)
                .is_some(),
            "the rat holds the burst against Bob"
        );
        assert!(crate::crimewatch::logoff_hold(&reg, BOB, t).is_some());
    }

    /// The three-pilot world in a 0.95 system.
    fn high_sec_world() -> Registry {
        let reg = world();
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let mut content = crate::content::Content::fixture();
        content.insert(
            "universe",
            crate::celestial::Universe::parse(
                &[format!(
                    "{system}\t19000001\t29000001\t\t\t0.95\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
                )],
                &[],
                &[],
            )
            .expect("the invented row parses"),
        );
        reg.with_content(std::sync::Arc::new(content))
    }

    /// A burst that wins flags Bob in high sec only if something took it:
    /// not a cloaked ship, not an abandoned hull, not a ship that refuses
    /// hostile modules (872).
    #[test]
    fn a_burst_over_nothing_it_may_hit_is_no_crime() {
        let t = now();
        // Both neighbours cloaked.
        let mut reg = high_sec_world();
        crate::cloak::set_device_for_test(&mut reg, ALICE, ALICE_SHIP);
        crate::cloak::set_device_for_test(&mut reg, CAROL, CAROL_SHIP);
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &sure());
        assert_eq!(crate::crimewatch::flag_of(&reg, BOB, t), None, "cloaked");
        // Only an abandoned hull in the radius.
        let mut reg = high_sec_world();
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let at = reg.roster().get(ALICE).expect("in space").motion;
        reg.roster_mut().leave(ALICE);
        reg.roster_mut().leave(CAROL);
        crate::pilotless::abandon(
            &mut reg,
            crate::pilotless::Abandoned {
                ship_id: 7_001,
                type_id: 600,
                owner_id: ALICE,
                corp_id: 0,
                alliance_id: None,
                system,
                abandoned_at: t,
            },
            at,
        );
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &sure());
        assert_eq!(crate::crimewatch::flag_of(&reg, BOB, t), None, "abandoned");
        // Alice refuses hostile modules (872).
        let mut reg = high_sec_world();
        crate::cloak::set_device_for_test(&mut reg, CAROL, CAROL_SHIP);
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        burst(&mut reg, 1);
        reconcile(
            &mut reg,
            t,
            &Fixture {
                no_offense: 1.0,
                ..sure()
            },
        );
        assert_eq!(crate::crimewatch::flag_of(&reg, BOB, t), None, "872");
        assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);
    }

    /// #138: in high sec the burst is judged by the same path a targeted
    /// module is: a won roll on an unflagged capsuleer flags the burster;
    /// a lost one does not.
    #[test]
    fn a_burst_in_high_sec_flags_the_burster_as_a_targeted_module_does() {
        let t = now();
        let build = high_sec_world;
        let mut reg = build();
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &Fixture { jam: 0.0, ..PLAIN });
        assert_eq!(crate::crimewatch::flag_of(&reg, BOB, t), None, "a miss");
        let mut reg = build();
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &sure());
        assert!(
            crate::crimewatch::flag_of(&reg, BOB, t).is_some(),
            "a hit on a clean capsuleer in high sec is a crime"
        );
        // The same act by a targeted module gives the same verdict.
        let mut twin = build();
        bob(&mut twin, ECM_T, ECM, ALICE_SHIP);
        reconcile(&mut twin, t, &sure());
        assert_eq!(
            crate::crimewatch::flag_of(&reg, BOB, t),
            crate::crimewatch::flag_of(&twin, BOB, t)
        );
    }

    /// The combat log lines: names, order of args, audiences.
    #[test]
    fn a_burst_tells_both_pilots_in_the_combat_log() {
        let t = now();
        let mut reg = world();
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        burst(&mut reg, 1);
        reconcile(&mut reg, t, &sure());
        let sent = pushes(&mut reg);
        let lines = combat_lines(&sent);
        assert_eq!(lines.len(), 2);
        let mine = lines
            .iter()
            .find(|(_, _, to)| *to == Some(vec![BOB]))
            .expect("attacker line");
        assert_eq!(mine.1[0], crate::pyutil::bstr("OnECMBurstAttacker"));
        assert_eq!(
            mine.1[1],
            crate::pyutil::pydict(vec![
                ("specialObject", PyValue::Int(ALICE_SHIP)),
                ("type", PyValue::Int(BURST_T)),
            ])
        );
        let theirs = lines
            .iter()
            .find(|(_, _, to)| *to == Some(vec![ALICE]))
            .expect("defender line");
        assert_eq!(
            theirs.1[0],
            crate::pyutil::bstr("OnECMBurstDefenderFromCharacter")
        );
        assert_eq!(
            theirs.1[1],
            crate::pyutil::pydict(vec![
                ("owner", PyValue::Int(BOB)),
                ("type", PyValue::Int(BURST_T)),
            ])
        );
        // The dict is (value, key) on the wire.
        assert!(matches!(&theirs.1[1], PyValue::Dict(e) if e[0].0 == PyValue::Int(BOB)));
    }

    /// Outside the radius, with no strength to beat, protected, or immune: no
    /// effect and no line.
    #[test]
    fn a_burst_leaves_the_far_the_protected_and_the_immune_alone() {
        let t = now();
        let quiet = |reg: &mut Registry, d: &Fixture| {
            reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
            burst(reg, 1);
            reconcile(reg, t, d);
            assert_eq!(reg.locks().targets_of(ALICE_SHIP), vec![BOB_SHIP]);
            assert!(pushes(reg).is_empty());
        };
        // Beyond the radius.
        let mut reg = world();
        let origin = reg.roster().get(BOB).expect("in space").motion.position;
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion = crate::space_state::Motion::arrived(
            [origin[0] + 3.0 * BURST_REACH, origin[1], origin[2]],
            [0.0; 3],
        );
        quiet(&mut reg, &sure());
        // A weak burst loses the roll.
        let mut reg = world();
        quiet(&mut reg, &Fixture { jam: 0.0, ..PLAIN });
        // Resistance 0 is immune.
        let mut reg = world();
        quiet(
            &mut reg,
            &Fixture {
                resist: 0.0,
                jam: 1_000.0,
                ..PLAIN
            },
        );
        // Undock protection.
        let mut reg = world();
        crate::invuln::set_enabled(&mut reg, true);
        crate::invuln::grant(&mut reg, ALICE, ALICE_SHIP, t);
        let _ = reg.take_due(far());
        quiet(&mut reg, &sure());
    }
}
