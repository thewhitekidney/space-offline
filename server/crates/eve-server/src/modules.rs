//! Module activation: online state, cycles, and what the client is told.
//!
//! Spec: `docs/re/modules.md`. Owned by the P2.1 lane (`docs/lanes/modules.md`).
//!
//! **The whole feature is one call in and two pushes out** (spec §0): the
//! client asks `dogmaIM.Activate(itemID, effectName, target, repeat)` on the
//! bound object it already holds, discards our reply (`smb:1580`, `POP_TOP`)
//! and waits for `OnGodmaShipEffect`. That one message turns the button on and
//! the client then sweeps the cycle ramp **for ever, off its own clock**
//! (`smb:1692`, `DoActivateRampsThread`) `[V]`. `OnSpecialFX` is the separate
//! message that draws the gun in space, for the shooter and every observer.
//!
//! ## What the server is for, then
//!
//! The client animates; it does not *simulate*. So this module keeps the
//! three things only the server can:
//!
//! 1. **Which modules are online** -- `SetModuleOnline` raised for ever until
//!    now, which the plan's filter (`docs/archive/plan-subsystems.md` §5) names as the
//!    example of an empty answer that is a lie.
//! 2. **Which effects are cycling, and since when** -- so a cycle can be
//!    counted, a capacitor drained, a target lost, and a second activation
//!    refused with CCP's own key.
//! 3. **The capacitor.** The client *predicts recharge* from a `(value, time)`
//!    pair but never subtracts `capacitorNeed` itself (§4.2) `[V]`, so a drain
//!    is ours to compute and ours to push.
//!
//! ## The server does not tick the client; it ticks itself
//!
//! No per-cycle message goes to the HUD. Per-cycle *simulation* is
//! [`advance`], which is **lazy**: every entry point first brings every running
//! effect up to `now`, so the model is right even when nobody calls
//! [`tick`]. The world clock calling [`tick`] is what makes a drain or an
//! out-of-capacitor stop *timely* rather than merely correct.
//!
//! ## The seam for damage
//!
//! Every cycle that starts is recorded as a [`Cycle`] -- "this module's effect
//! cycled at this time against this target" -- and queued on [`Modules`].
//! `crate::damage` takes them with [`Modules::take_cycles`]; nothing here
//! knows what a cycle *does*. See `docs/lanes/modules.md`.
//!
//! **Every cycle that ends is queued too, on a second queue** that
//! `crate::mining` takes with [`Modules::take_completed`]: a gun's volley
//! lands when its cycle starts **[A]**, ore arrives when the laser's cycle
//! is over **[A]**. Two queues with one consumer each is the split -- combat
//! cannot eat a mining cycle and mining cannot eat a volley, by construction
//! rather than by a filter both sides have to agree on
//! (`docs/lanes/mining.md` §2).
//!
//! Short citations are the spec's: `godma` is
//! `eve/client/script/environment/godma.pyc`, `smb` is `shipmodulebutton.pyc`,
//! `bdl` is `eve/common/script/dogma/baseDogmaLocation.pyc`, all `tools/dis.py`
//! line numbers.

use crate::content::Content;
use crate::fitting::{Hardpoint, Rack};
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// `const.effectOnline` `[V]` (`dogmaeffects[16]`). The one effect every
/// module has, and the one `ModuleButton.Update` turns into
/// `ShowOnline`/`ShowOffline` (`smb:1505`).
pub const EFFECT_ONLINE: i64 = 16;
/// `const.attributeIsOnline` `[V]` (`dogmaattributes[2]`).
pub const ATTR_IS_ONLINE: i64 = 2;
/// `const.attributeCapacitorCharge`, `'charge'` `[V]` (`dogma/const.pyc:229`).
/// Charged by `rechargeRate` (55): `chargeRechargeTimeID = 55` `[V]`.
pub const ATTR_CHARGE: i64 = 18;
/// `eveexceptions.const.UE_TYPEID` `[V]` (`out/pyc/eveexceptions/const.pyc:5`).
/// **4, not 29** -- 29 is `UE_TYPEIDL`.
pub const UE_TYPEID: i64 = 4;
/// `const.MSEC`: blue time is 100 ns ticks `[V]` (`carbon/common/lib/const.pyc:277`).
/// Also `dogma.const.dgmTauConstant` (`dogma/const.pyc:103`) `[V]`.
pub const MSEC: i64 = 10_000;

/// `maxGroupActive`: how many of a group may run at once.
pub const ATTR_MAX_GROUP_ACTIVE: i64 = 763;
/// A hull's `maxShipGroupActiveID` and the limit it sets for that group.
const ATTR_MAX_SHIP_GROUP_ACTIVE_ID: i64 = 909;
const ATTR_MAX_SHIP_GROUP_ACTIVE: i64 = 910;
/// `disallowRepeatingActivation`: the module runs one cycle per activation.
const ATTR_DISALLOW_REPEATING: i64 = 1014;
/// `moduleReactivationDelay`, milliseconds.
const ATTR_REACTIVATION_DELAY: i64 = 669;
/// `const.dgmEffTarget` `[V]`: the effect needs a target, and the client asks
/// for one before it ever calls us (`smb:1541-1547`).
pub const CATEGORY_TARGET: i64 = 2;
/// `dogma.const.effectMiningLaser` `[V]` (`dogma/const.pyc:1616`): Miner I's
/// default effect, and one of the three `ship.pyc:174` turns into the
/// harvesting beam.
pub const EFFECT_MINING_LASER: i64 = 67;
/// `useMissiles` `[V-data]` (`dogmaeffects[101]`): every launcher's default
/// effect, missile or probe. Its guid is empty; a launch's FX is the
/// charge's ([`MissileFx`]).
pub const EFFECT_USE_MISSILES: i64 = 101;

/// How many [`Cycle`]s wait for a consumer before the oldest are dropped.
///
/// **[A]** a number, not a fact. Until `crate::damage` drains the queue on
/// every tick nothing reads it, and an unread queue that grows with every gun
/// cycle is a leak that only shows after a long fight. A thousand is several
/// minutes of a full rack; dropping the oldest keeps the newest, which is what
/// a consumer that wakes late wants.
const CYCLE_QUEUE_CAP: usize = 1024;

/// How many cycle boundaries one call to [`advance`] will process.
///
/// A guard, not a model: a 0.01 s cycle left alone for an hour would
/// otherwise be a third of a million iterations under the world lock. The
/// shortest cycle of any turret is seconds long, so a busy server never
/// gets near it.
const MAX_BOUNDARIES_PER_ADVANCE: usize = 10_000;

/// One effect a module can be activated with.
///
/// Everything here is **the client's own data**, resolved the way
/// `ModuleButton.GetEffectTiming` resolves it (`smb:1812`, spec §3.2): the
/// type's default effect from `typedogma`, its row from `dogmaeffects`, and
/// the *value* of `durationAttributeID` from the type's attributes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub effect_id: i64,
    /// What `dogmaIM.Activate` names it by. The client sends the **name**,
    /// not the id (`godma:2441`) `[V]`.
    pub name: &'static str,
    /// `effectCategory`: 1 activation, 2 target.
    pub category: i64,
    /// `dogmaeffects[effectID].guid`, the `OnSpecialFX` key. Empty for a
    /// launcher, whose visual comes from its charge (§6.4).
    pub guid: &'static str,
    pub is_offensive: bool,
    /// `durationAttributeID`: 51 `speed` for turrets, 73 `duration` otherwise.
    pub duration_attribute: i64,
    /// `dogmaeffects[effectID].isWarpSafe`: `Some(false)` stops the effect
    /// when its ship's warp goes active ([`stop_for_warp`]). `None` when the
    /// table has no such column (an old extract) or the row is a fixture.
    pub warp_safe: Option<bool>,
}

/// The repeat count a type allows: one when it carries
/// `disallowRepeatingActivation`, else what the client asked for.
fn repeat_allowed(t: &ModuleType, repeat: i64) -> i64 {
    if t.attribute(ATTR_DISALLOW_REPEATING)
        .is_some_and(|v| v != 0.0)
    {
        1
    } else {
        repeat
    }
}

/// One module type this server can cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleType {
    pub type_id: i64,
    pub name: &'static str,
    /// `None` for a passive module (a shield extender): it can be onlined and
    /// offlined, and nothing else. The client refuses the click itself with
    /// `TryingToActivatePassiveModule` (`smb:1084`), so we never see one.
    pub effect: Option<Effect>,
    /// `{attributeID: value}` as served in `shipInfo`.
    ///
    /// **Served, not optional.** `godma.GetAttribute` falls back to the
    /// attribute *definition's* default rather than the type's value
    /// (`service.rs::capsule_dogma_row` has the evidence), so a duration
    /// attribute left off the wire is a zero, and a zero duration is a ramp
    /// that never draws (`smb:1692`, `if durationInMilliseconds <= 0: return`).
    ///
    /// `nextActivationTime` (2729) is deliberately absent: a future value
    /// there makes the module unclickable, silently (§6.3, T14).
    pub attributes: &'static [(i64, f64)],
    /// The rack its slot effect puts it in -- `hiPower` 12, `medPower` 13,
    /// `loPower` 11 [V-data] (`docs/lanes/fitting.md` §3.3).
    pub rack: Rack,
    /// `turretFitted` (42) or `launcherFitted` (40), if it has either
    /// [V-data].
    pub hardpoint: Option<Hardpoint>,
}

impl ModuleType {
    /// The attribute's value, if served.
    pub fn attribute(&self, attribute_id: i64) -> Option<f64> {
        self.attributes
            .iter()
            .find(|(id, _)| *id == attribute_id)
            .map(|(_, v)| *v)
    }

    /// The cycle length in milliseconds: the value of the effect's duration
    /// attribute (`bdl:1021 GetEffectDuration`), or `-1` -- the client's own
    /// "no duration" sentinel (`godma:3072`) -- for an effect without one.
    pub fn duration_ms(&self) -> i64 {
        self.effect
            .and_then(|e| self.attribute(e.duration_attribute))
            .map_or(-1, |ms| ms.round() as i64)
    }

    /// `capacitorNeed` (attribute 6), the `dischargeAttributeID` of every
    /// effect in the table `[V]`. Zero for a projectile turret.
    pub fn capacitor_need(&self) -> f64 {
        self.attribute(6).unwrap_or(0.0)
    }

    /// `miningAmount` (77), cubic metres of ore one cycle cuts. Zero for
    /// anything that is not a miner.
    pub fn mining_amount(&self) -> f64 {
        self.attribute(attr::MINING_AMOUNT).unwrap_or(0.0)
    }

    /// `cpu` (50), what onlining it costs the ship's `cpuOutput`.
    pub fn cpu(&self) -> f64 {
        self.attribute(attr::CPU).unwrap_or(0.0)
    }

    /// `power` (30), what onlining it costs the ship's `powerOutput`.
    pub fn power(&self) -> f64 {
        self.attribute(attr::POWER).unwrap_or(0.0)
    }
}

/// Dogma attribute ids this module and [`FIXTURE`] use, all `[V]` from
/// `dogmaattributes`.
pub(crate) mod attr {
    pub const CAPACITOR_NEED: i64 = 6;
    pub const POWER: i64 = 30;
    pub const CPU: i64 = 50;
    pub const SPEED: i64 = 51;
    pub const MAX_RANGE: i64 = 54;
    pub const DAMAGE_MULTIPLIER: i64 = 64;
    pub const DURATION: i64 = 73;
    /// `attributeMiningAmount` `[V]` (`dogma/const.pyc:721`).
    pub const MINING_AMOUNT: i64 = 77;
    pub const CHARGE_SIZE: i64 = 128;
    pub const FALLOFF: i64 = 158;
    pub const TRACKING_SPEED: i64 = 160;
    pub const CHARGE_GROUP1: i64 = 604;
    /// `chargeRate`: rounds one cycle fires `[V]` (`dogmaattributes[56]`).
    pub const CHARGE_RATE: i64 = 56;
    /// `quantity`: a sublocation's round count `[V]` (`dogmaattributes[805]`,
    /// `godma:2191-2192` rewrites `stacksize` to it).
    pub const QUANTITY: i64 = 805;
    pub const RELOAD_TIME: i64 = 1795;
    pub const EM_DAMAGE: i64 = 114;
    pub const EXPLOSIVE_DAMAGE: i64 = 116;
    pub const KINETIC_DAMAGE: i64 = 117;
    pub const THERMAL_DAMAGE: i64 = 118;
}

/// A civilian gun's invented row: its damage `[em, explosive, kinetic,
/// thermal]` on itself and no charge; the shared numbers are invented too.
macro_rules! civilian {
    ($id:expr, $name:expr, [$em:expr, $ex:expr, $ki:expr, $th:expr]) => {
        ModuleType {
            type_id: $id,
            name: $name,
            effect: Some(Effect {
                effect_id: 10,
                name: "targetAttack",
                category: 2,
                guid: "effects.Laser",
                is_offensive: true,
                duration_attribute: attr::SPEED,
                warp_safe: None,
            }),
            attributes: &[
                (attr::SPEED, 2500.0),
                (attr::CAPACITOR_NEED, 1.5),
                (attr::MAX_RANGE, 2000.0),
                (attr::FALLOFF, 2000.0),
                (attr::TRACKING_SPEED, 600.0),
                (attr::DAMAGE_MULTIPLIER, 1.5),
                (attr::EM_DAMAGE, $em),
                (attr::EXPLOSIVE_DAMAGE, $ex),
                (attr::KINETIC_DAMAGE, $ki),
                (attr::THERMAL_DAMAGE, $th),
                (attr::RELOAD_TIME, 10.0),
                (attr::CPU, 4.0),
                (attr::POWER, 5.0),
            ],
            rack: Rack::High,
            hardpoint: Some(Hardpoint::Turret),
        }
    };
}

/// A probe launcher's invented row: `useMissiles` (no guid: the visual is
/// the probe's), probes by group (`chargeGroup1` 479), no charge size.
macro_rules! probe_launcher {
    ($id:expr, $name:expr, $cpu:expr) => {
        ModuleType {
            type_id: $id,
            name: $name,
            effect: Some(Effect {
                effect_id: 101,
                name: "useMissiles",
                category: 1,
                guid: "",
                is_offensive: false,
                duration_attribute: attr::SPEED,
                warp_safe: None,
            }),
            attributes: &[
                (attr::SPEED, 2500.0),
                (attr::DURATION, 8000.0),
                (attr::RELOAD_TIME, 8000.0),
                (attr::CHARGE_GROUP1, 479.0),
                (attr::CPU, $cpu),
                (attr::POWER, 1.0),
            ],
            rack: Rack::High,
            hardpoint: None,
        }
    };
}

/// The small projectile turret's invented row: a gun whose damage is its
/// loaded round's (`chargeGroup1` 83, one round a cycle).
const PROJECTILE_SMALL: ModuleType = ModuleType {
    type_id: 484,
    name: "Fixture Projectile Turret S",
    effect: Some(Effect {
        effect_id: 34,
        name: "projectileFired",
        category: 2,
        guid: "effects.ProjectileFired",
        is_offensive: true,
        duration_attribute: attr::SPEED,
        warp_safe: None,
    }),
    attributes: &[
        (attr::SPEED, 2500.0),
        (attr::CAPACITOR_NEED, 0.0),
        (attr::MAX_RANGE, 1200.0),
        (attr::FALLOFF, 4000.0),
        (attr::TRACKING_SPEED, 400.0),
        (attr::DAMAGE_MULTIPLIER, 2.5),
        (attr::RELOAD_TIME, 5000.0),
        (attr::CHARGE_SIZE, 1.0),
        (attr::CHARGE_GROUP1, 83.0),
        (attr::CHARGE_RATE, 1.0),
        (attr::CPU, 5.0),
        (attr::POWER, 2.0),
    ],
    rack: Rack::High,
    hardpoint: Some(Hardpoint::Turret),
};

/// The module types a server with no bundle can cycle, by the **type ids**
/// `/createitem` grants and the bots and tests fit (protocol vocabulary:
/// ids stay in code, O3), each with **invented numbers** -- no row is the
/// client's. [`find_in`] answers from here only when `Content` has no
/// bundle (ruling 3: `--content` is needed to *play*, never to *test*).
///
/// **The baked table is retired** (W18 CP58d, owner decision O3): the
/// client's own modules reach the server only from the bundle's
/// `types/typedata.tsv` (`crate::typedata::module_in`). The effects keep
/// their ids, names and guids: the client sends the name (`godma:2441`)
/// and draws the guid, so those are protocol, not tuning.
pub const FIXTURE: &[ModuleType] = &[
    ModuleType {
        type_id: 377,
        name: "Fixture Shield Extender",
        effect: None,
        attributes: &[(attr::CPU, 15.0), (attr::POWER, 3.0)],
        rack: Rack::Medium,
        hardpoint: None,
    },
    ModuleType {
        type_id: 399,
        name: "Fixture Shield Booster",
        effect: Some(Effect {
            effect_id: 4,
            name: "shieldBoosting",
            category: 1,
            guid: "effects.ShieldBoosting",
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 3000.0),
            (attr::CAPACITOR_NEED, 15.0),
            (attr::CPU, 20.0),
            (attr::POWER, 3.0),
        ],
        rack: Rack::Medium,
        hardpoint: None,
    },
    ModuleType {
        type_id: 439,
        name: "Fixture Afterburner",
        effect: Some(Effect {
            effect_id: 6731,
            name: "moduleBonusAfterburner",
            category: 1,
            guid: "effects.Afterburner",
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 8000.0),
            (attr::CAPACITOR_NEED, 15.0),
            (attr::CPU, 12.0),
            (attr::POWER, 8.0),
        ],
        rack: Rack::Medium,
        hardpoint: None,
    },
    ModuleType {
        type_id: 454,
        name: "Fixture Beam Laser",
        effect: Some(Effect {
            effect_id: 10,
            name: "targetAttack",
            category: 2,
            guid: "effects.Laser",
            is_offensive: true,
            duration_attribute: attr::SPEED,
            warp_safe: None,
        }),
        attributes: &[
            (attr::SPEED, 4000.0),
            (attr::CAPACITOR_NEED, 8.0),
            (attr::MAX_RANGE, 9000.0),
            (attr::FALLOFF, 2000.0),
            (attr::TRACKING_SPEED, 100.0),
            (attr::DAMAGE_MULTIPLIER, 4.0),
            (attr::RELOAD_TIME, 10.0),
            (attr::CHARGE_SIZE, 1.0),
            (attr::CHARGE_GROUP1, 86.0),
            (attr::CPU, 15.0),
            (attr::POWER, 10.0),
        ],
        rack: Rack::High,
        hardpoint: Some(Hardpoint::Turret),
    },
    ModuleType {
        type_id: 483,
        name: "Fixture Miner",
        effect: Some(Effect {
            effect_id: EFFECT_MINING_LASER,
            name: "miningLaser",
            category: 2,
            guid: "effects.Laser",
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 12000.0),
            (attr::CAPACITOR_NEED, 4.0),
            (attr::MAX_RANGE, 12000.0),
            (attr::MINING_AMOUNT, 20.0),
            (attr::CPU, 40.0),
            (attr::POWER, 2.0),
        ],
        rack: Rack::High,
        hardpoint: Some(Hardpoint::Turret),
    },
    PROJECTILE_SMALL,
    ModuleType {
        type_id: 486,
        name: "Fixture Projectile Turret M",
        attributes: &[
            (attr::SPEED, 3500.0),
            (attr::CAPACITOR_NEED, 0.0),
            (attr::MAX_RANGE, 1500.0),
            (attr::FALLOFF, 5000.0),
            (attr::TRACKING_SPEED, 300.0),
            (attr::DAMAGE_MULTIPLIER, 3.0),
            (attr::RELOAD_TIME, 8000.0),
            (attr::CHARGE_SIZE, 1.0),
            (attr::CHARGE_GROUP1, 83.0),
            (attr::CHARGE_RATE, 1.0),
            (attr::CPU, 8.0),
            (attr::POWER, 4.0),
        ],
        ..PROJECTILE_SMALL
    },
    civilian!(3634, "Fixture Civilian Pulse", [3.5, 0.0, 0.0, 1.5]),
    civilian!(3636, "Fixture Civilian Autocannon", [0.0, 3.5, 1.5, 0.0]),
    civilian!(3638, "Fixture Civilian Railgun", [0.0, 0.0, 3.5, 1.5]),
    civilian!(3640, "Fixture Civilian Blaster", [0.0, 0.0, 1.5, 3.5]),
    probe_launcher!(17938, "Fixture Core Probe Launcher", 10.0),
    probe_launcher!(18639, "Fixture Expanded Probe Launcher", 150.0),
    ModuleType {
        type_id: 21853,
        name: "Fixture Armor Repairer",
        effect: Some(Effect {
            effect_id: 27,
            name: "armorRepair",
            category: 1,
            guid: "effects.ArmorRepair",
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 5000.0),
            (attr::CAPACITOR_NEED, 4.0),
            (attr::CPU, 2.0),
            (attr::POWER, 2.0),
        ],
        rack: Rack::Low,
        hardpoint: None,
    },
    ModuleType {
        type_id: 21857,
        name: "Fixture Civilian Afterburner",
        effect: Some(Effect {
            effect_id: 6731,
            name: "moduleBonusAfterburner",
            category: 1,
            guid: "effects.Afterburner",
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 8000.0),
            (attr::CAPACITOR_NEED, 4.0),
            (attr::CPU, 2.0),
            (attr::POWER, 1.0),
        ],
        rack: Rack::Medium,
        hardpoint: None,
    },
    // A projected, non-weapon module for the range rules
    // (`crate::module_range`): the effect id is the client's
    // `remoteWebifierFalloff` [V-data]; every number is invented.
    ModuleType {
        type_id: 21860,
        name: "Fixture Stasis Webifier",
        effect: Some(Effect {
            effect_id: 6426,
            name: "remoteWebifierFalloff",
            category: 2,
            guid: "effects.ModifyTargetSpeed",
            // Invented as not offensive: the weapon table
            // (`combat::weapon_in`) has a row for every offensive fixture,
            // and this one is no weapon.
            is_offensive: false,
            duration_attribute: attr::DURATION,
            warp_safe: None,
        }),
        attributes: &[
            (attr::DURATION, 5000.0),
            (attr::CAPACITOR_NEED, 5.0),
            (attr::MAX_RANGE, 10000.0),
            // `falloffEffectiveness`, the projected effects' falloff.
            (2044, 5000.0),
            (attr::CPU, 2.0),
            (attr::POWER, 1.0),
        ],
        rack: Rack::Medium,
        hardpoint: None,
    },
];

/// One module type by id, from the content (W18 CP58d, ruling 3): the
/// bundle's `types/typedata.tsv` (`crate::typedata::module_in`), then, with
/// no bundle only, the invented [`FIXTURE`]. `None` for a type neither has:
/// a module this server cannot cycle.
pub fn find_in(content: &Content, type_id: i64) -> Option<&ModuleType> {
    crate::typedata::module_in(content, type_id).or_else(|| {
        if content.is_bundle() {
            None
        } else {
            fixture_row(type_id)
        }
    })
}

fn fixture_row(type_id: i64) -> Option<&'static ModuleType> {
    FIXTURE
        .binary_search_by_key(&type_id, |t| t.type_id)
        .ok()
        .map(|i| &FIXTURE[i])
}

/// [`find_in`] on the world's content, copied out so the world can be
/// borrowed mutably after (a `ModuleType` is a few words and `'static`
/// slices).
fn module_type(w: &Registry, type_id: i64) -> Option<ModuleType> {
    find_in(w.content(), type_id).copied()
}

// ---------------------------------------------------------------------------
// The capacitor
// ---------------------------------------------------------------------------

/// The capacitor after recharging from `(value, at)` to `now`.
///
/// **The client's own formula, twice over** `[V]`: `godma:2413
/// GetChargeValue` and `dogma/attributes/chargedAttribute.pyc` `Update` are
/// the same expression. Times are blue ticks; `tau` is `rechargeRate / 5` in
/// milliseconds (`godma:1821`'s `tau/5.0`), and dividing ticks by
/// `dgmTauConstant` gives milliseconds.
///
/// ```text
/// sq = sqrt(max(value / cap, 0))
/// t  = min(at - now, 0) / dgmTauConstant          # <= 0, ms
/// (1 + (sq - 1) * exp(t / tau))^2 * cap
/// ```
///
/// Computing it the client's way is the point: a push whose `newValue` was
/// computed differently from the client's prediction is a bar that jumps.
pub fn recharged(value: f64, at: i64, now: i64, capacity: f64, recharge_ms: f64) -> f64 {
    if capacity == 0.0 {
        return 0.0;
    }
    let tau = recharge_ms / 5.0;
    if tau <= 0.0 {
        return value;
    }
    let sq = (value / capacity).max(0.0).sqrt();
    let passed = (at - now).min(0) as f64 / MSEC as f64;
    (1.0 + (sq - 1.0) * (passed / tau).exp()).powi(2) * capacity
}

/// One ship's capacitor, as last changed.
///
/// Absent from [`Modules`] means **full**, which is what `GetAllInfo` seeds
/// (`service.rs::ship_attributes`, attribute 18 = 482) and the recharge
/// formula's fixed point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Charge {
    pub value: f64,
    /// Blue ticks.
    pub at: i64,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// An effect that is cycling now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Running {
    pub character_id: i64,
    pub ship_id: i64,
    pub module_id: i64,
    pub type_id: i64,
    pub effect: Effect,
    pub target_id: Option<i64>,
    /// When the first cycle started, blue ticks.
    pub started: i64,
    /// Cycle length in blue ticks; always positive -- an effect with no
    /// duration does not run (see [`activate`]).
    pub period: i64,
    /// The client's `repeat`, echoed (`godma:1738`).
    pub repeat: i64,
    /// Cycles started so far, the first included.
    pub cycles: i64,
    /// When a deactivation takes hold: the end of the cycle that was running
    /// when the player clicked. `None` while it repeats.
    pub stop_at: Option<i64>,
}

impl Running {
    /// When the next cycle would start.
    pub fn next_boundary(&self) -> i64 {
        self.started + self.cycles * self.period
    }

    /// **[A]** how many cycles the client's `repeat` asks for: `repeat <= 0`
    /// is one shot (spec §1.2, "0 means one shot") and anything else is a
    /// count. The client sends 1000 (`smb:1565`), which at three seconds a
    /// cycle is fifty minutes -- in practice, until stopped.
    fn cycles_wanted(&self) -> i64 {
        self.repeat.max(1)
    }
}

/// One cycle that started -- **the seam a damage model consumes** -- or,
/// on the second queue, one that ended (the seam `crate::mining` consumes).
///
/// "This module's effect cycled at this time against this target", and
/// nothing about what that does -- whether a gun hits, how much a repairer
/// repairs or what a miner yields is the consumer's business, read from the
/// effect id and the type. **[A]** a gun's cycle lands when the cycle
/// *starts*, which is when the capacitor is paid (spec §9 open question 2);
/// a miner's when it *ends* ([`Modules::take_completed`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cycle {
    pub character_id: i64,
    pub ship_id: i64,
    pub module_id: i64,
    pub type_id: i64,
    pub effect_id: i64,
    /// The ball the effect was aimed at; `None` for a self effect.
    pub target_id: Option<i64>,
    /// When the cycle started, blue ticks -- or, on the completed queue,
    /// when it ended.
    pub at: i64,
    /// 0 for the activation itself, then 1, 2, ...
    pub index: i64,
    /// The charge this cycle fired -- its rounds already taken out of the
    /// module (P2.3) -- or `None` for a module that cycles without one.
    /// `crate::combat::volley_fitted` reads the damage off it.
    pub charge_type_id: Option<i64>,
}

/// What one cycle's fire took out of a module: the seam between the
/// charge and `crate::remote_rep` (a cap booster's amount is its charge's, an
/// ancillary armor repairer's multiplier scales with `taken / rate`).
/// Kept per module by cycle `index` ([`Modules::fired_at`]) because the
/// charge is already consumed when the rep lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fired {
    /// The cycle's `index` (0 for the activation itself).
    pub index: i64,
    /// The charge type that fired, `None` when the module ran empty.
    pub charge_type_id: Option<i64>,
    /// Rounds taken this cycle.
    pub taken: i64,
    /// `chargeRate`: what a full cycle takes (at least 1).
    pub rate: i64,
}

/// How many [`Fired`] records a module keeps: a rep catches up at most
/// `remote_rep::MAX_CATCH_UP` cycles.
const FIRED_KEPT: usize = 16;

/// The ancillary repairers and boosters (local ASB, AAR, remote AAR, remote
/// ASB): they run unloaded, at reduced effect, and reload when switched off
/// (the client's module text, loc 286330 and 277430 `[V]`).
pub fn is_ancillary(effect_id: i64) -> bool {
    matches!(effect_id, 4_936 | 5_275 | 6_651 | 6_652)
}

/// Script groups a module may run without `[V inventorycommon/const.py]`:
/// dampeners, disruptors, boosters, computers and links take a script but
/// work without one. The HIC's group (908, HIC-SCRIPT) is among them: a
/// Warp Disruption Field Generator with nothing loaded is the bubble, with a
/// focused script it is a point (`crate::tackle::focus`).
const OPTIONAL_CHARGE_GROUPS: [f64; 7] = [907.0, 908.0, 909.0, 910.0, 911.0, 1400.0, 1569.0];

/// Does this module run with nothing loaded, because its `chargeGroup1`
/// names only optional scripts? A script has no `chargeRate`, so it is
/// never used up either ([`fire_rounds`]).
pub fn runs_empty(t: &ModuleType) -> bool {
    t.attribute(attr::CHARGE_GROUP1)
        .is_some_and(|g| OPTIONAL_CHARGE_GROUPS.contains(&g))
}

/// Every module's online state, every running effect and every drained
/// capacitor.
///
/// **Not persisted**, like `targeting::Locks`: a running effect names balls
/// in a park, and a restart empties every park. Online state is the one
/// thing a real server would keep; it is held here until fitting itself is
/// persisted, because a module whose fitting does not survive a restart has
/// no online state to keep.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Modules {
    /// Modules taken offline. **[A]** a fitted module is online unless it is
    /// in here: real EVE onlines a module when it is fitted, if the ship can
    /// power it -- which [`on_fitted`] decides at fit time and `budget`
    /// at every `SetModuleOnline`.
    offline: BTreeSet<i64>,
    running: BTreeMap<i64, Running>,
    /// Module -> when its effect last ended, blue ticks: the reactivation
    /// delay's clock (attribute 669, [`check_reactivation`]). Cleared with
    /// the module ([`Modules::forget`]).
    last_stop: BTreeMap<i64, i64>,
    /// Module -> what its recent cycles took ([`Fired`]), oldest first.
    fired: BTreeMap<i64, Vec<Fired>>,
    capacitor: BTreeMap<i64, Charge>,
    cycles: Vec<Cycle>,
    completed: Vec<Cycle>,
    /// Module -> when its reload ends, blue ticks ([`load_ammo`] in space).
    reloading: BTreeMap<i64, i64>,
    /// **Bank slaves that run without telling the pilot** (`crate::weapon_bank`):
    /// a weapon bank is one HUD button and one icon under the target, so only
    /// the master's `OnGodmaShipEffect` goes out; a slave cycles, fires and
    /// pays like any module but its effect push is withheld [A: falsified by
    /// a bank that shows one icon per launcher on the target].
    quiet: BTreeSet<i64>,
    /// **The drones in space** (`crate::drones`): the second weapon system,
    /// kept beside the first so that it costs the shared `Registry` no field
    /// of its own.
    drones: crate::drones::Drones,
}

impl Modules {
    /// Every drone in space.
    pub fn drones(&self) -> &crate::drones::Drones {
        &self.drones
    }

    pub fn drones_mut(&mut self) -> &mut crate::drones::Drones {
        &mut self.drones
    }

    /// Withhold (or restore) a module's `OnGodmaShipEffect`.
    pub fn set_quiet(&mut self, module_id: i64, quiet: bool) {
        if quiet {
            self.quiet.insert(module_id);
        } else {
            self.quiet.remove(&module_id);
        }
    }

    /// Modules whose effect pushes are withheld.
    pub fn quiet(&self) -> Vec<i64> {
        self.quiet.iter().copied().collect()
    }

    /// Forget the quiet flag of every module that no longer runs.
    pub fn forget_idle_quiet(&mut self) {
        let running = &self.running;
        self.quiet.retain(|m| running.contains_key(m));
    }

    pub fn is_online(&self, module_id: i64) -> bool {
        !self.offline.contains(&module_id)
    }

    /// Set a module's online state. `false` if it already was.
    pub fn set_online(&mut self, module_id: i64, online: bool) -> bool {
        if online {
            self.offline.remove(&module_id)
        } else {
            self.offline.insert(module_id)
        }
    }

    /// Whether `effect_id` runs on `ship_id` now.
    pub fn effect_running_on(&self, ship_id: i64, effect_id: i64) -> bool {
        self.running
            .values()
            .any(|r| r.ship_id == ship_id && r.effect.effect_id == effect_id)
    }

    /// The running effect of module `module_id`, if any.
    pub fn running(&self, module_id: i64) -> Option<&Running> {
        self.running.get(&module_id)
    }

    /// End a module's effect at `at` and remember when, for the reactivation
    /// delay. The one place a [`Running`] leaves the map.
    fn end(&mut self, module_id: i64, at: i64) -> Option<Running> {
        let r = self.running.remove(&module_id);
        if r.is_some() {
            self.last_stop.insert(module_id, at);
        }
        r
    }

    /// Remember what cycle `index` of a module fired.
    pub(crate) fn note_fired(&mut self, module_id: i64, fired: Fired) {
        let v = self.fired.entry(module_id).or_default();
        v.retain(|f| f.index != fired.index);
        v.push(fired);
        if v.len() > FIRED_KEPT {
            v.remove(0);
        }
    }

    /// What cycle `index` of a module fired, if it fired anything.
    pub fn fired_at(&self, module_id: i64, index: i64) -> Option<Fired> {
        self.fired
            .get(&module_id)?
            .iter()
            .find(|f| f.index == index)
            .copied()
    }

    /// When this module's effect last ended, blue ticks.
    pub fn last_stop(&self, module_id: i64) -> Option<i64> {
        self.last_stop.get(&module_id).copied()
    }

    /// Drop everything known about a module that left its ship -- an unfit
    /// (`docs/lanes/fitting.md` §2.5). A module refit later starts from the
    /// default again, which [`on_fitted`] then corrects for the budget.
    pub fn forget(&mut self, module_id: i64) {
        self.offline.remove(&module_id);
        self.running.remove(&module_id);
        self.last_stop.remove(&module_id);
        self.fired.remove(&module_id);
        self.reloading.remove(&module_id);
    }

    /// When this module's reload ends, if it is reloading at `now`.
    pub fn reloading_until(&self, module_id: i64, now: i64) -> Option<i64> {
        self.reloading.get(&module_id).copied().filter(|t| *t > now)
    }

    /// Drop a destroyed ship: every module that was fitted to it (`modules`,
    /// the ids the store just destroyed) and its capacitor. A cycle already
    /// recorded for it stays queued and is ignored by `crate::combat`, whose
    /// shooter test is "is this pilot still in that ship".
    pub fn forget_ship(&mut self, ship_id: i64, modules: &[i64]) {
        for m in modules {
            self.forget(*m);
        }
        self.capacitor.remove(&ship_id);
    }

    /// The ships with a capacitor on record (NEUT-NOS: the sweep of a dead
    /// NPC's, which `forget_ship` never sees).
    pub(crate) fn capacitor_ships(&self) -> Vec<i64> {
        self.capacitor.keys().copied().collect()
    }

    /// Drop one ship's capacitor record (it is full again).
    pub(crate) fn forget_capacitor(&mut self, ship_id: i64) {
        self.capacitor.remove(&ship_id);
    }

    /// The ship's capacitor as last changed, or `None` for "full".
    pub fn charge(&self, ship_id: i64) -> Option<Charge> {
        self.capacitor.get(&ship_id).copied()
    }

    /// **The damage seam**: every cycle started since the last call, oldest
    /// first.
    pub fn take_cycles(&mut self) -> Vec<Cycle> {
        std::mem::take(&mut self.cycles)
    }

    /// **The mining seam**: every cycle that *ran to its end* since the last
    /// call, oldest first, with `at` the end. A different queue from
    /// [`Self::take_cycles`], so the two consumers cannot take each other's.
    ///
    /// A cycle is complete when its boundary is reached with the pilot still
    /// in that ship in space and the target (for a targeted effect) still
    /// locked: whether the effect then repeats, stops because the player
    /// asked, runs out of `repeat` or out of capacitor for the *next* cycle.
    /// Not when the pilot left space, the lock was lost, or the module went
    /// offline mid-cycle -- those cycles were cut short **[A]**.
    pub fn take_completed(&mut self) -> Vec<Cycle> {
        std::mem::take(&mut self.completed)
    }

    /// Every running effect, whoever runs it and wherever it is aimed
    /// (K-EWAR: the targets of the electronic-warfare effects).
    pub fn running_all(&self) -> Vec<Running> {
        self.running.values().copied().collect()
    }

    /// Every running effect aimed at `target_id`.
    pub fn running_against(&self, target_id: i64) -> Vec<Running> {
        self.running
            .values()
            .filter(|r| r.target_id == Some(target_id))
            .copied()
            .collect()
    }

    /// Start `r` as if [`activate`] had: for the tests of a consumer of
    /// [`Self::running_against`] whose module types the type table cannot
    /// hold (W16 CP57, `crate::tackle`).
    #[cfg(test)]
    pub(crate) fn insert_running(&mut self, r: Running) {
        self.running.insert(r.module_id, r);
    }

    /// Set a ship's capacitor as if a drain had left it there (K-LOGI's
    /// tests of a transfer into a drained ship).
    #[cfg(test)]
    pub(crate) fn set_capacitor(&mut self, ship_id: i64, value: f64, at: i64) {
        self.capacitor.insert(ship_id, Charge { value, at });
    }

    fn record(&mut self, cycle: Cycle) {
        push_capped(&mut self.cycles, cycle);
    }

    fn complete(&mut self, cycle: Cycle) {
        push_capped(&mut self.completed, cycle);
    }
}

fn push_capped(queue: &mut Vec<Cycle>, cycle: Cycle) {
    if queue.len() >= CYCLE_QUEUE_CAP {
        queue.remove(0);
    }
    queue.push(cycle);
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnGodmaShipEffect(itemID, effectID, t, start, active, environment,
/// startTime, duration, repeat, error, actualStopTime=None, stall=True)`
/// `[V]` (`godma:1722`, order from `co_varnames`).
///
/// Sent with **eleven** positional arguments and `stall` defaulted, which is
/// exactly the shape of the client's own call at `godma:3072`.
///
/// Addressed to the pilot alone: `godma:1745` gates the whole HUD update on
/// `environment[2] == session.shipid`, so an observer's godma would do nothing
/// with it. Observers learn from [`SpecialFx`].
#[derive(Debug, Clone, PartialEq)]
pub struct ShipEffect {
    pub to: Vec<i64>,
    pub module_id: i64,
    pub effect_id: i64,
    pub character_id: i64,
    pub ship_id: i64,
    pub target_id: Option<i64>,
    /// Blue ticks. Read only by `TryRecordLastStopTime` (§9 question 6).
    pub t: i64,
    /// `1` to start, `0` to stop: `ModuleButton.Update` switches on it
    /// (`smb:1493-1497`).
    pub start: bool,
    pub duration_ms: i64,
    pub repeat: i64,
    /// `(dialogKey, paramsDict)`, or `None`. `shipUI.ProcessShipEffect` hands
    /// a non-`None` one straight to `prompt_player_fsd_dialog(error[0],
    /// error[1])` (`shipUI.pyc:1210-1211`) `[V]`: the channel for "your laser
    /// shut off, and why", with no second message (spec §7.3).
    pub error: Option<PyValue>,
}

impl ShipEffect {
    /// `[itemID, charID, shipID, targetID, None, [], effectID]` `[V]`
    /// (`godma:1198-1205`; a list, as `godma:3072` builds it). A short list
    /// is an `IndexError` inside the handler.
    pub fn environment(&self) -> PyValue {
        PyValue::List(vec![
            PyValue::Int(self.module_id),
            PyValue::Int(self.character_id),
            PyValue::Int(self.ship_id),
            self.target_id.map_or(PyValue::None, PyValue::Int),
            PyValue::None,
            PyValue::List(Vec::new()),
            PyValue::Int(self.effect_id),
        ])
    }
}

impl Push for ShipEffect {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let flag = PyValue::Int(i64::from(self.start));
        (
            "OnGodmaShipEffect",
            vec![
                PyValue::Int(self.module_id),
                PyValue::Int(self.effect_id),
                PyValue::Int(self.t),
                // `start` and `active` move together here: a repeating effect
                // that starts is active, and one that stops is not.
                flag.clone(),
                flag,
                self.environment(),
                // `startTime`: **None**, so `GetRampStartTime`'s
                // `getattr(..., now) or now` anchors the ramp on the client's
                // own clock (`smb:1765`, spec §3.3) and the sim-clock epoch
                // never matters.
                PyValue::None,
                PyValue::Int(self.duration_ms),
                PyValue::Int(self.repeat),
                // `error`: a `(key, dict)` here is shown by
                // `shipUI.ProcessShipEffect` (§7.3); `crate::mining` sends
                // one when a laser stops for a full hold or an empty rock.
                self.error.clone().unwrap_or(PyValue::None),
                // `actualStopTime`: `None`, or the client `SleepSim`s until a
                // time on a clock we have not pinned (§9 question 1).
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnSpecialFX(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
/// isOffensive, start, active, duration=-1, repeat=None, startTime=None,
/// timeFromStart=0, graphicInfo=None)` `[V]` (`fx:441`, seven sources agree,
/// spec §5.2). All fourteen sent.
///
/// The thing that draws a gun firing, for the shooter and every observer in
/// the ballpark (§5.4). `fxSequencer.AddTrigger` drops it quietly if either
/// ball is not in the receiver's park (`fx:618`, `fx:632`), so an audience a
/// little too wide is harmless.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecialFx {
    pub to: Vec<i64>,
    pub ship_id: i64,
    pub module_id: i64,
    pub type_id: i64,
    pub target_id: Option<i64>,
    pub guid: &'static str,
    pub is_offensive: bool,
    pub start: bool,
    pub duration_ms: i64,
    pub repeat: i64,
}

impl Push for SpecialFx {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let flag = PyValue::Int(i64::from(self.start));
        (
            "OnSpecialFX",
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Int(self.module_id),
                PyValue::Int(self.type_id),
                self.target_id.map_or(PyValue::None, PyValue::Int),
                // `otherTypeID`: the loaded charge's type for a gun **[A]**
                // (§9 question 3). Charges are not modelled, so `None`.
                PyValue::None,
                PyValue::Bytes(self.guid.as_bytes().to_vec()),
                PyValue::Bool(self.is_offensive),
                flag.clone(),
                flag,
                PyValue::Int(self.duration_ms),
                PyValue::Int(self.repeat),
                // `startTime`: **None**. A real time more than 15 s old makes
                // `fxSequencer` discard long one-shot guids outright
                // (`fx:485-487`).
                PyValue::None,
                PyValue::Int(0),
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    /// **Not a Destiny entry, and filtered anyway.** `fxSequencer` would drop
    /// it quietly for a ship not in the park (above), but a gun firing on
    /// another grid is still a message a pilot has no business receiving.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship_id])
    }
}

/// **Every effect drawing in space right now**, as the starts a viewer who
/// arrived late missed (`fxSequencer` keeps nothing for a ball that was not
/// in its park, `fx:412-438`), with `to` empty: player modules, NPC guns and
/// ewar beams, and drones' guns. `duration_ms` is the period and `repeat` the
/// cycles left (at least 1); the start time is the wire's `None` and
/// `timeFromStart` 0, so the tactical "attacking me" key
/// `(moduleID, guid, startTime, duration)` equals the one the later stop
/// carries. Missile launches are left out: the missile ball draws those.
pub fn active_fx_all(w: &Registry) -> Vec<SpecialFx> {
    let now = crate::pyutil::filetime_now();
    let mut out = Vec::new();
    for r in w.modules().running_all() {
        if r.effect.guid.is_empty()
            || crate::cloak::is_cloak_effect(r.effect.effect_id)
            || r.effect.effect_id == EFFECT_USE_MISSILES
            || r.stop_at.is_some_and(|t| t <= now)
        {
            continue;
        }
        out.push(SpecialFx {
            to: Vec::new(),
            ship_id: r.ship_id,
            module_id: r.module_id,
            type_id: r.type_id,
            target_id: r.target_id,
            guid: r.effect.guid,
            is_offensive: r.effect.is_offensive,
            start: true,
            duration_ms: r.period / MSEC,
            repeat: (r.cycles_wanted() - r.cycles + 1).max(1),
        });
    }
    out.extend(w.npcs().iter().filter_map(crate::npc::running_gun_fx));
    out.extend(crate::npc_ewar::running_fx(w, now));
    for d in w.modules().drones().iter() {
        if let Some(t) = d.firing {
            let mining = crate::drones::is_miner(w.content(), d.kind.type_id);
            out.push(crate::drones::gun_fx(Vec::new(), d, t, true, mining));
        }
    }
    out
}

/// [`active_fx_all`] kept to the effects whose ship is in `balls` and whose
/// target is in `balls` or absent.
pub fn active_fx(w: &Registry, balls: &std::collections::BTreeSet<i64>) -> Vec<SpecialFx> {
    let mut all = active_fx_all(w);
    all.retain(|f| fx_visible(f, balls));
    all
}

/// Whether both ends of `f` are in `balls` (a missing target is).
pub fn fx_visible(f: &SpecialFx, balls: &std::collections::BTreeSet<i64>) -> bool {
    balls.contains(&f.ship_id) && f.target_id.is_none_or(|t| balls.contains(&t))
}

/// **One missile launch's `OnSpecialFX`** (R-MISSILE §1.6, M4): a
/// [`SpecialFx`] whose `otherTypeID` is the **charge** launched, and whose
/// guid is the charge's default effect's (`missileLaunching`, 9,
/// `effects.MissileDeployment`) -- the launcher's own `useMissiles` (101)
/// has none. The client's handler only aims the launcher at the target
/// (`effects/turrets.py:178-230`) [V]; the missile ball draws the launch.
/// One per launch, `repeat` 1 **[A]** as CP45's NPC launch; it ends
/// itself, so no stop is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct MissileFx {
    pub fx: SpecialFx,
    pub charge_type_id: i64,
}

impl Push for MissileFx {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let (name, mut args) = self.fx.to_push();
        args[4] = PyValue::Int(self.charge_type_id);
        (name, args)
    }

    fn audience(&self) -> Option<Vec<i64>> {
        self.fx.audience()
    }

    fn grid(&self) -> crate::grid::GridRole {
        self.fx.grid()
    }
}

/// `OnModuleAttributeChange(ownerID, itemKey, attributeID, time, newValue,
/// oldValue, wallclockTime, scatterAttr=True)` `[V]` (`godma:1575`).
///
/// Here, only ever the capacitor: `ownerID` is the **character** -- `godma:1591`
/// drops the change unless `itemKey == session.charid or ownerID ==
/// session.charid` -- and `itemKey` the ship.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeChange {
    pub to: Vec<i64>,
    pub owner_id: i64,
    pub item_id: i64,
    pub attribute_id: i64,
    /// Blue ticks. **Becomes `oldTime` in the client's recharge prediction**,
    /// so it must be the drain's own time: a zero snaps the bar to full
    /// (§4.3, T9).
    pub time: i64,
    pub new_value: f64,
    pub old_value: f64,
}

impl Push for AttributeChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnModuleAttributeChange",
            vec![
                PyValue::Int(self.owner_id),
                PyValue::Int(self.item_id),
                PyValue::Int(self.attribute_id),
                PyValue::Int(self.time),
                PyValue::Real(self.new_value),
                PyValue::Real(self.old_value),
                // `wallclockTime`: `None` always passes
                // `_IsAttributeChangeRelevant` (`godma:2036`). **[A]** that
                // no ordering protection is needed: our pushes leave one
                // queue in the order they were made.
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// A loaded charge's key: `(shipID, flagID, typeID)`, the sublocation's
/// whole identity -- it has no item id of its own (`godma:1863`,
/// `docs/re/modules.md` §6.1) `[V]`.
pub fn charge_key(ship_id: i64, flag_id: i64, type_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(ship_id),
        PyValue::Int(flag_id),
        PyValue::Int(type_id),
    ])
}

/// **A loaded charge's round count changed**: `OnModuleAttributeChanges`
/// with one change, attribute 805 on the sublocation key.
///
/// The plural, not `OnModuleAttributeChange`, because **two** listeners need
/// it and only the plural reaches both `[V]` (`findings/push-events.tsv`):
///
/// - `godma:463` -> `OnModuleAttributeChanges` (`godma:1620`): the HUD's
///   count. A zero deletes the sublocation and scatters `OnItemChange` with
///   `{ixStackSize: old}`, which empties the button (`godma:3040-3059`,
///   `smb SetCharge`).
/// - `clientDogmaLocation:391`: the fitting window's. A new key is fitted
///   (`FitItemToLocation`), a zero is unloaded (`UnfitAndUnloadItem`).
///
/// **Shapes both of them force** `[V]`: the outer value is a **list** --
/// `cdl` calls `changes.sort(key=...)` on it -- and each change is a
/// **tuple** of eight, `(eventName, ownerID, itemKey, attributeID, time,
/// newValue, oldValue, wallclockTime)`: `cdl` unpacks eight, and godma calls
/// `OnModuleAttributeChange_(*(each[1:] + (0,)))`, which is a `TypeError` on
/// a list. `ownerID` is the pilot, as for the capacitor (`godma:1591`).
///
/// **[A]** the counts are ints: they land in the `quantity` column of a
/// sublocation row (`godma:3057`, `blue.DBRow(sublocrd, [...])`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChargeQuantity {
    pub to: Vec<i64>,
    pub owner_id: i64,
    pub ship_id: i64,
    pub flag_id: i64,
    pub type_id: i64,
    /// Blue ticks; `cdl` sorts a batch by it.
    pub time: i64,
    pub new_value: i64,
    pub old_value: i64,
}

impl Push for ChargeQuantity {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnModuleAttributeChanges",
            vec![PyValue::List(vec![PyValue::Tuple(vec![
                crate::pyutil::bstr("OnModuleAttributeChange"),
                PyValue::Int(self.owner_id),
                charge_key(self.ship_id, self.flag_id, self.type_id),
                PyValue::Int(attr::QUANTITY),
                PyValue::Int(self.time),
                PyValue::Int(self.new_value),
                PyValue::Int(self.old_value),
                // `wallclockTime`: `None`, as for the capacitor.
                PyValue::None,
            ])])],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// **A charge newly loaded**: `OnGodmaPrimeItem(locationID, row)` `[V]`
/// (`godma:123` lists it in `__notifyevents__`; handler `godma:483`).
///
/// godma cannot learn a *new* sublocation from an attribute change -- its
/// `ApplyAttributeChange` returns early for a key it has no attributes for
/// (`godma:2948`) -- so the first load is a prime of the one row, the same
/// row `GetAllInfo`'s `shipInfo` carries ([`charge_info_in`]).
/// `StateManager.OnGodmaPrimeItem` inserts it through `UpdateItem`
/// (`godma:1555`), and the service then scatters `OnItemChange` with
/// `{ixLocationID: None, ixFlag: None}` (`godma:483`-), which is what puts
/// the ammo icon and its count on the module's button (`smb AddItem` ->
/// `SetCharge`).
#[derive(Debug, Clone, PartialEq)]
pub struct PrimeCharge {
    pub to: Vec<i64>,
    pub ship_id: i64,
    pub row: PyValue,
}

impl Push for PrimeCharge {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnGodmaPrimeItem",
            vec![PyValue::Int(self.ship_id), self.row.clone()],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnChargeBeingLoadedToModule(moduleIDs, chargeTypeID, reloadTime)` `[V]`
/// (`smb:750`, `turret.py:148`; `findings/push-events.tsv`): the reload
/// animation on the button and the turret, and the `LauncherLoadDelay` line
/// in the log. `moduleIDs` is a list (`smb`: `itemID not in itemIDs`;
/// `turret` iterates it); `reloadTime` is milliseconds (`params['time'] =
/// time / 1000`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChargeLoading {
    pub to: Vec<i64>,
    pub module_ids: Vec<i64>,
    pub charge_type_id: i64,
    pub reload_ms: i64,
}

impl Push for ChargeLoading {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnChargeBeingLoadedToModule",
            vec![
                PyValue::List(self.module_ids.iter().map(|m| PyValue::Int(*m)).collect()),
                PyValue::Int(self.charge_type_id),
                PyValue::Int(self.reload_ms),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// **The HUD is rebuilt when a gun's last round is gone.**
///
/// The button's own state (`quantity`, `charge`) is set only by `SetCharge`,
/// from `SlotsContainer.InitSlots`, and `SlotsContainer.OnInvChange` re-runs
/// `InitSlots` only for an `OnItemChange` that carries `ixFlag` for a fitting
/// flag (or `ixLocationID` for a fitted module) `[V: slotsContainer.pyc
/// OnInvChange, line 240]`. The count going to zero arrives as godma's
/// `SubLoc.quantityExpelled` -> `OnItemChange {ixStackSize: old}` (`godma:3062`),
/// which that handler ignores, so a button that had run dry kept its stale
/// charge: `Update` saw `quantity != 0` and never auto-reloaded
/// (`smb:1509`), and a click sent `Activate` to us, which answered
/// `NoCharges` (the owner's log, 21:12). A charge deleted in a real server is
/// an inventory change on the charge row; ours is a sublocation, so this is
/// the same news told through the **module's** row: `OnItemChange(row,
/// {ixFlag: its flag})`, which every cache reads as "nothing moved" and the
/// HUD reads as "rebuild". **[A]** that this is enough to give the new
/// button `quantity == 0`; falsified by a dry launcher whose stop does not
/// auto-reload with auto-reload on.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRefit {
    pub to: Vec<i64>,
    pub row: PyValue,
    pub flag_id: i64,
}

impl Push for ModuleRefit {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        crate::notify::ItemChange {
            row: self.row.clone(),
            change: vec![(crate::item::IX_FLAG, self.flag_id)],
            location: PyValue::None,
        }
        .to_push()
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Any of these, so one list can carry a whole step's output in order.
#[derive(Debug, Clone, PartialEq)]
pub enum ModulePush {
    Effect(ShipEffect),
    Fx(SpecialFx),
    Launch(MissileFx),
    Attribute(AttributeChange),
    Quantity(ChargeQuantity),
    Prime(PrimeCharge),
    Loading(ChargeLoading),
    Refit(ModuleRefit),
}

impl Push for ModulePush {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        match self {
            ModulePush::Effect(p) => p.to_push(),
            ModulePush::Fx(p) => p.to_push(),
            ModulePush::Launch(p) => p.to_push(),
            ModulePush::Attribute(p) => p.to_push(),
            ModulePush::Quantity(p) => p.to_push(),
            ModulePush::Prime(p) => p.to_push(),
            ModulePush::Loading(p) => p.to_push(),
            ModulePush::Refit(p) => p.to_push(),
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        match self {
            ModulePush::Effect(p) => p.audience(),
            ModulePush::Fx(p) => p.audience(),
            ModulePush::Launch(p) => p.audience(),
            ModulePush::Attribute(p) => p.audience(),
            ModulePush::Quantity(p) => p.audience(),
            ModulePush::Prime(p) => p.audience(),
            ModulePush::Loading(p) => p.audience(),
            ModulePush::Refit(p) => p.audience(),
        }
    }

    fn grid(&self) -> crate::grid::GridRole {
        match self {
            ModulePush::Fx(p) => p.grid(),
            ModulePush::Launch(p) => p.grid(),
            _ => crate::grid::GridRole::None,
        }
    }

    fn cancel_tag(&self) -> Option<crate::notify::CancelTag> {
        match self {
            ModulePush::Effect(e) if !e.start => Some(crate::notify::CancelTag::ModuleStop {
                module: e.module_id,
            }),
            ModulePush::Fx(f) if !f.start => Some(crate::notify::CancelTag::ModuleStop {
                module: f.module_id,
            }),
            _ => None,
        }
    }
}

/// Whether a push leaves with the answer or on the clock.
///
/// An arm's pushes go on `pending`, which is flushed after the `CallRsp`; the
/// clock's go on `scheduled`, because nothing else drains `pending` for a
/// quiet client (`targeting::lock` has the story).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Via {
    Answer,
    Clock,
}

fn send(w: &mut Registry, via: Via, pushes: Vec<ModulePush>) {
    for p in pushes {
        if hushed(w, &p) {
            continue;
        }
        match via {
            Via::Answer => w.push(p),
            Via::Clock => w.schedule(Duration::ZERO, p),
        }
    }
}

/// Whether `p` is a bank slave's `OnGodmaShipEffect`, which the pilot is not
/// told (one button, one icon per bank).
fn hushed(w: &Registry, p: &ModulePush) -> bool {
    matches!(p, ModulePush::Effect(e) if w.modules().quiet.contains(&e.module_id))
}

/// `w.push` for an effect push, unless it is a bank slave's.
fn push_effect(w: &mut Registry, e: ShipEffect) {
    let p = ModulePush::Effect(e);
    if !hushed(w, &p) {
        w.push(p);
    }
}

/// The stop pair for a running effect: the HUD's and the space visual's.
fn stop_pushes(w: &Registry, r: &Running, t: i64) -> Vec<ModulePush> {
    let cloak = crate::cloak::is_cloak_effect(r.effect.effect_id);
    let mut out = vec![ModulePush::Effect(ShipEffect {
        to: vec![r.character_id],
        module_id: r.module_id,
        effect_id: r.effect.effect_id,
        character_id: r.character_id,
        ship_id: r.ship_id,
        target_id: r.target_id,
        t,
        start: false,
        // A cloak has no cycle: the client was told -1 and is told it again.
        duration_ms: if cloak { -1 } else { r.period / MSEC },
        repeat: r.repeat,
        error: None,
    })];
    if !r.effect.guid.is_empty() && !cloak {
        out.push(ModulePush::Fx(SpecialFx {
            to: observers(w, r.character_id),
            ship_id: r.ship_id,
            module_id: r.module_id,
            type_id: r.type_id,
            target_id: r.target_id,
            guid: r.effect.guid,
            is_offensive: r.effect.is_offensive,
            start: false,
            duration_ms: r.period / MSEC,
            repeat: r.repeat,
        }));
    }
    out
}

/// The launch FX for one cycle of a missile launcher that fired
/// `charge_type_id` ([`MissileFx`]); `None` for any other module, or a
/// charge that does not fly (a probe: `crate::scan` draws those).
fn launch_fx(
    w: &Registry,
    r: &Running,
    charge_type_id: Option<i64>,
    duration_ms: i64,
) -> Option<ModulePush> {
    if r.effect.effect_id != EFFECT_USE_MISSILES {
        return None;
    }
    let charge = charge_type_id?;
    let missile = crate::typedata::missile_in(w.content(), charge)?;
    if missile.guid.is_empty() {
        return None;
    }
    Some(ModulePush::Launch(MissileFx {
        fx: SpecialFx {
            to: observers(w, r.character_id),
            ship_id: r.ship_id,
            module_id: r.module_id,
            type_id: r.type_id,
            target_id: r.target_id,
            guid: missile.guid,
            is_offensive: missile.offensive,
            start: true,
            duration_ms,
            repeat: 1,
        },
        charge_type_id: charge,
    }))
}

/// Whether a running effect needs its target locked to go on: every target
/// effect, and **a missile launcher's** `useMissiles` aimed at something
/// (M4). That effect is an activation (category 1) [V-data], but the client
/// asks for a lock first because the *charge's* effect is a target one
/// (`smb:1534-1547`, `GetRelevantEffect`) [V]; **[A]** a lost lock stops
/// the launcher as it stops a gun. A probe launcher names no target.
fn needs_lock(effect: &Effect, target_id: Option<i64>) -> bool {
    effect.category == CATEGORY_TARGET
        || (effect.effect_id == EFFECT_USE_MISSILES && target_id.is_some())
}

/// The pilot and everyone else in their solar system.
fn observers(w: &Registry, character_id: i64) -> Vec<i64> {
    let mut to = vec![character_id];
    if let Some(here) = w.roster().get(character_id) {
        to.extend(
            w.roster()
                .others_in_system(here.solar_system_id, character_id),
        );
    }
    to
}

/// `OnGodmaShipEffect` for the `online` effect: no duration, no repeat --
/// `-1` and `-1`, as the client's own offline event sends (`godma:3072`).
fn online_push(character_id: i64, ship_id: i64, module_id: i64, on: bool, t: i64) -> ModulePush {
    ModulePush::Effect(ShipEffect {
        to: vec![character_id],
        module_id,
        effect_id: EFFECT_ONLINE,
        character_id,
        ship_id,
        target_id: None,
        t,
        start: on,
        duration_ms: -1,
        repeat: -1,
        error: None,
    })
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

pub(crate) fn user_error(key: &str, args: Vec<(&str, PyValue)>) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

/// `(UE_TYPEID, typeID)`, the loc-arg form the dialogs want for a module.
pub(crate) fn type_arg(type_id: i64) -> PyValue {
    PyValue::Tuple(vec![PyValue::Int(UE_TYPEID), PyValue::Int(type_id)])
}

/// The `(message key, arg)` pairs whose text reads `[item]<arg>.name`
/// (`findings/user-errors.tsv`, `loc_args`). The client's item handler wants
/// a **plain typeID** there: it turns a `(UE_TYPEID, id)` pair into the
/// type's *name* first, and `int('Mining Drone I')` is a traceback
/// (`TypeNotFoundException`, seen on `TargetTooFar` and
/// `NoDroneManagementAbilitiesLeft`) `[V]`. A pair not listed keeps the tuple.
/// Every message that names a type in this server goes through
/// [`type_arg_for`], so the rule lives here.
const PLAIN_ITEM_ARGS: &[(&str, &str)] = &[
    ("CannotActivateSuperWeaponOnNonCapitalShip", "moduleTypeID"),
    ("CantActivateDoomsDayOnNonShip", "module"),
    ("CantActivateReloadingModule", "moduleTypeID"),
    ("CantDockWhileModuleActive", "moduleType"),
    ("CantFitTooManyByGroup", "module"),
    ("CantFitTooManyByGroup", "ship"),
    ("CantJumpWhileModuleActive", "moduleType"),
    ("EffectStillActive", "moduleTypeID"),
    ("JumpNotEnoughCharge", "typeName"),
    ("JumpNotEnoughCharge2", "typeName"),
    ("JumpNotEnoughCharge3", "typeName"),
    ("ModuleNotOnline", "module"),
    ("NoDroneManagementAbilities", "typeID"),
    ("NoDroneManagementAbilitiesLeft", "item"),
    ("NotEnoughCpu", "moduleType"),
    ("NotEnoughPower", "moduleType"),
    ("TargetTooFar", "module"),
    ("VelocityTooHighToActivate", "type"),
    ("WarpScrambledByWarpDisruptionFieldGenerator", "ship"),
];

/// Whether `loc_args` (a tsv cell such as `[item]item.name,[numeric]limit`)
/// reads `arg` as an `[item]` property, so the arg is a plain typeID.
#[cfg(test)]
pub(crate) fn spec_wants_plain_item(loc_args: &str, arg: &str) -> bool {
    loc_args.split(',').any(|part| {
        part.trim()
            .strip_prefix("[item]")
            .is_some_and(|p| p.trim().split('.').next() == Some(arg))
    })
}

/// The loc arg `arg` of message `key` naming `type_id`: the plain typeID
/// where the text reads `[item]arg`, else the `(UE_TYPEID, typeID)` pair.
pub(crate) fn type_arg_for(key: &str, arg: &str, type_id: i64) -> PyValue {
    if PLAIN_ITEM_ARGS.contains(&(key, arg)) {
        PyValue::Int(type_id)
    } else {
        type_arg(type_id)
    }
}

/// Our own words, as one log line: `CustomNotify` is the generic escape hatch
/// (`findings/user-errors.tsv`), for the refusals that are about *this
/// server* rather than about EVE.
fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

// ---------------------------------------------------------------------------
// The world-facing half
// ---------------------------------------------------------------------------

/// A module fitted to the actor's current ship.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fitted {
    character_id: i64,
    ship_id: i64,
    ship_type_id: i64,
    module_id: i64,
    type_id: i64,
}

/// The module, if it is fitted to this character's ship and theirs.
///
/// **Every refusal of a module we do not hold is ours, not CCP's**: a client
/// that sends one has a fitting this server did not make, which today means
/// the fitting is client-side only. Saying so is better than a key that
/// claims something about the module.
fn fitted(w: &Registry, actor: Option<i64>, module_id: i64) -> Result<Fitted, ServiceError> {
    let Some(c) = actor.and_then(|id| w.character(id)) else {
        return Err(notify("No character is selected."));
    };
    let Some(item) = w.item(module_id) else {
        return Err(notify("This server does not know that module."));
    };
    if item.location_id != c.ship_id
        || item.owner_id != c.character_id
        || !crate::item::is_ship_fitting_flag(item.flag_id)
    {
        return Err(notify("That module is not fitted to your ship."));
    }
    Ok(Fitted {
        character_id: c.character_id,
        ship_id: c.ship_id,
        ship_type_id: c.ship_type_id,
        module_id,
        type_id: item.type_id,
    })
}

/// The capacitor of a ship at `now`, and its capacity.
///
/// Capacity (482) and recharge time (55) are the ship's **modified** values
/// when `crate::dogma` has a sheet for it -- Capacitor Management and
/// Capacitor Systems Operation reach them as the client's own fitting window
/// shows -- and the hand hull's otherwise.
pub(crate) fn capacitor_at(w: &Registry, ship_id: i64, ship_type_id: i64, now: i64) -> (f64, f64) {
    let modified = crate::dogma::hull(w, ship_id);
    let h = modified
        .as_ref()
        .or_else(|| crate::destiny::hull::find_in(w.content(), ship_type_id))
        .unwrap_or(&crate::destiny::hull::FALLBACK);
    let value = match w.modules().charge(ship_id) {
        Some(c) => recharged(
            c.value,
            c.at,
            now,
            h.capacitor_capacity,
            h.capacitor_recharge_rate,
        ),
        None => h.capacitor_capacity,
    };
    (value, h.capacitor_capacity)
}

/// Pay one cycle's capacitor, if it can be paid. Returns the push, or `Err`
/// when the capacitor is short.
///
/// **[A]** paid at the start of each cycle (spec §9 question 2:
/// `ConsumeResources` is `pass` in the shipped base class and the node's
/// override is not ours to read).
fn pay(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    ship_type_id: i64,
    need: f64,
    at: i64,
) -> Result<Option<ModulePush>, ()> {
    if need <= 0.0 {
        return Ok(None);
    }
    let (before, _) = capacitor_at(w, ship_id, ship_type_id, at);
    if before < need {
        return Err(());
    }
    let after = before - need;
    w.modules_mut()
        .capacitor
        .insert(ship_id, Charge { value: after, at });
    Ok(Some(ModulePush::Attribute(AttributeChange {
        to: vec![character_id],
        owner_id: character_id,
        item_id: ship_id,
        attribute_id: ATTR_CHARGE,
        time: at,
        new_value: after,
        old_value: before,
    })))
}

/// Who holds a ship's capacitor: its type, and the pilot to tell (`None`
/// for an NPC, which has a capacitor and no session). `None` for a ship
/// nobody models one for (a drone, a squadron).
fn capacitor_holder(w: &Registry, ship_id: i64) -> Option<(i64, Option<i64>)> {
    let key = w.roster().by_ship(ship_id)?.character_id;
    if let Some(c) = w.character(key) {
        return Some((c.ship_type_id, Some(key)));
    }
    w.npcs().get(key).map(|n| (n.kind.type_id, None))
}

/// A capacitor moved by [`give_capacitor_by`] or [`drain_capacitor_by`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Moved {
    /// How much actually moved, GJ: clipped at empty or full.
    pub amount: f64,
    /// The attribute-18 push to the pilot; `None` for an NPC.
    pub push: Option<ModulePush>,
}

fn charge_push(character_id: i64, ship_id: i64, at: i64, new: f64, old: f64) -> ModulePush {
    ModulePush::Attribute(AttributeChange {
        to: vec![character_id],
        owner_id: character_id,
        item_id: ship_id,
        attribute_id: ATTR_CHARGE,
        time: at,
        new_value: new,
        old_value: old,
    })
}

/// **Put capacitor into a ship** (K-LOGI, a remote capacitor transmitter's
/// cycle; NEUT-NOS, a nosferatu): up to `amount` more in `ship_id`'s
/// capacitor at `at`, to its capacity. `None` for a ship with no capacitor
/// model or one already full. An NPC is filled too, with nobody to tell.
pub(crate) fn give_capacitor_by(
    w: &mut Registry,
    ship_id: i64,
    amount: f64,
    at: i64,
) -> Option<Moved> {
    if amount <= 0.0 {
        return None;
    }
    let (ship_type_id, pilot) = capacitor_holder(w, ship_id)?;
    let (before, capacity) = capacitor_at(w, ship_id, ship_type_id, at);
    let after = (before + amount).min(capacity);
    if after <= before {
        return None;
    }
    w.modules_mut()
        .capacitor
        .insert(ship_id, Charge { value: after, at });
    Some(Moved {
        amount: after - before,
        push: pilot.map(|p| charge_push(p, ship_id, at, after, before)),
    })
}

/// **Take capacitor out of a ship** (FT-F, a squadron's energy neutralizer
/// cycle; NEUT-NOS, a neutralizer or nosferatu): up to `amount` less in
/// `ship_id`'s capacitor at `at`, down to nothing. `None` for a ship with no
/// capacitor model or one already empty. An NPC has a capacitor too
/// (attributes 482 and 55 of its type): it is drained with nobody to tell.
pub(crate) fn drain_capacitor_by(
    w: &mut Registry,
    ship_id: i64,
    amount: f64,
    at: i64,
) -> Option<Moved> {
    if amount <= 0.0 {
        return None;
    }
    let (ship_type_id, pilot) = capacitor_holder(w, ship_id)?;
    let (before, _) = capacitor_at(w, ship_id, ship_type_id, at);
    let after = (before - amount).max(0.0);
    if after >= before {
        return None;
    }
    w.modules_mut()
        .capacitor
        .insert(ship_id, Charge { value: after, at });
    Some(Moved {
        amount: before - after,
        push: pilot.map(|p| charge_push(p, ship_id, at, after, before)),
    })
}

/// [`give_capacitor_by`]'s push (K-LOGI): `None` for an NPC, a full ship or
/// a ship with no model.
pub(crate) fn give_capacitor(
    w: &mut Registry,
    ship_id: i64,
    amount: f64,
    at: i64,
) -> Option<ModulePush> {
    give_capacitor_by(w, ship_id, amount, at)?.push
}

/// [`drain_capacitor_by`]'s push (FT-F): `None` for an NPC, an empty ship or
/// a ship with no model.
pub(crate) fn drain_capacitor(
    w: &mut Registry,
    ship_id: i64,
    amount: f64,
    at: i64,
) -> Option<ModulePush> {
    drain_capacitor_by(w, ship_id, amount, at)?.push
}

/// **An NPC pays a cycle's capacitor** (NEUT-NOS): `true` and the capacitor
/// down by `need` when it can pay; `false` and nothing changed when it
/// cannot. A need of zero, or a type with no capacitor (482 absent), always
/// pays.
pub(crate) fn spend_npc_capacitor(
    w: &mut Registry,
    ship_id: i64,
    ship_type_id: i64,
    need: f64,
    at: i64,
) -> bool {
    if need <= 0.0 {
        return true;
    }
    let (before, capacity) = capacitor_at(w, ship_id, ship_type_id, at);
    if capacity <= 0.0 {
        return true;
    }
    if before < need {
        return false;
    }
    w.modules_mut().capacitor.insert(
        ship_id,
        Charge {
            value: before - need,
            at,
        },
    );
    true
}

/// Bring every running effect up to `now`, in time order.
///
/// **Time order across modules matters** because modules share a capacitor:
/// processing one gun's whole backlog before another's would let the first
/// drain cycles the second had already paid for. So each step takes the
/// earliest due boundary in the whole world.
///
/// At each boundary, in order:
///
/// - a deactivation that has taken hold, or a `repeat` exhausted, ends the
///   effect;
/// - a pilot who has left space (docked, disconnected, died) ends it
///   silently -- their godma is re-primed by the session change anyway;
/// - **[A]** a targeted effect whose target is no longer locked ends, as in
///   EVE, with no dialog;
/// - **[A]** a gun out of rounds ends it, with no dialog: the last round
///   fired at the boundary before, and the client auto-reloads when the
///   effect stops with an empty button (`smb:1510`) `[V]`;
/// - **[A]** a capacitor that cannot pay ends it, with no dialog (§7.4);
/// - otherwise the cycle starts: the capacitor is paid, a round is taken
///   out of the gun, and a [`Cycle`] queued naming the charge it fired.
fn advance(w: &mut Registry, now: i64, via: Via) -> usize {
    let mut out: Vec<ModulePush> = Vec::new();
    let mut steps = 0;
    // Ships whose propulsion module stopped: their top speed changed. The
    // hull each flew with before its first module ended here is kept, so
    // `motion_changed` sends only what moved.
    let mut slowed: BTreeMap<(i64, i64), Option<crate::destiny::hull::Hull>> = BTreeMap::new();
    // The drives whose cycle ended in this call: each jumps after the loop.
    let mut jumps: Vec<Running> = Vec::new();
    // The resonances of each (pilot, ship) before its first module ended
    // here: a hardener's end moves them.
    let mut defended: BTreeMap<(i64, i64), Vec<(i64, f64)>> = BTreeMap::new();
    // **A target's loss ends the effect at the end of the running cycle**
    // (GUN-CYCLE), not now: the target died, jumped, left the grid or was
    // unlocked, but the cycle that is running is paid for and runs out, as
    // EVE does and as the client's `isDeactivating` expects. The boundary
    // loop below (`target_lost`) stops it there, and the reactivation delay
    // counts from that stop. Until then an Activate is refused with
    // `EffectAlreadyActive2`, like any activation of a running module.
    // (Ending it now, ENGINE-FIX, let a gun that instakilled its target fire
    // again at once.)
    while steps < MAX_BOUNDARIES_PER_ADVANCE {
        let due = w
            .modules()
            .running
            .values()
            .filter(|r| r.next_boundary() <= now)
            .min_by_key(|r| (r.next_boundary(), r.module_id))
            .copied();
        let Some(r) = due else { break };
        steps += 1;
        if is_propulsion(r.effect.effect_id) {
            // Every branch below but the one that starts the next cycle
            // ends the effect; that one is told apart after the loop.
            note_slowed(w, &mut slowed, &r);
        }
        let at = r.next_boundary();
        let ended_by_player = r.stop_at.is_some_and(|s| at >= s);
        let exhausted = r.cycles >= r.cycles_wanted();
        let in_space = w
            .roster()
            .get(r.character_id)
            .is_some_and(|p| p.ship_id == r.ship_id);
        if !in_space {
            w.modules_mut().end(r.module_id, at);
            continue;
        }
        let target_lost = needs_lock(&r.effect, r.target_id)
            && r.target_id
                .is_none_or(|t| !w.locks().targets_of(r.ship_id).contains(&t));
        if !target_lost {
            // The cycle that began at `at - period` has run to its end, and
            // is complete whatever happens to the next one.
            w.modules_mut().complete(Cycle {
                character_id: r.character_id,
                ship_id: r.ship_id,
                module_id: r.module_id,
                type_id: r.type_id,
                effect_id: r.effect.effect_id,
                target_id: r.target_id,
                at,
                index: r.cycles - 1,
                // The completed queue is mining's, and a miner fires no
                // charge; the started cycle carries it for combat.
                charge_type_id: None,
            });
            // A Micro Jump Drive's one cycle is its spool: the jump comes
            // with its end, however the module is stopped (MJD-MASS).
            if crate::mjd::is_mjd(r.effect.effect_id) {
                jumps.push(r);
            }
        }
        if ended_by_player {
            // Its stop pair was scheduled when the player clicked.
            note_defense(w, &mut defended, &r);
            w.modules_mut().end(r.module_id, at);
            reload_ancillary(w, &r, at);
            continue;
        }
        if exhausted || target_lost {
            note_defense(w, &mut defended, &r);
            w.modules_mut().end(r.module_id, at);
            let mut stop = stop_pushes(w, &r, at);
            // A scram that holds the ship at the end of the spool fails the
            // jump, and the stop says so (`ModuleIsBlocked`).
            if crate::mjd::is_mjd(r.effect.effect_id)
                && let Some(refusal) = crate::mjd::blocked(w, &r, at)
                && let Some(ModulePush::Effect(e)) = stop.first_mut()
            {
                e.error = Some(crate::mjd::error_of(refusal));
            }
            out.extend(stop);
            continue;
        }
        let ship_type_id = w.character(r.character_id).map_or(0, |c| c.ship_type_id);
        let need = capacitor_need(w, r.ship_id, r.module_id, r.type_id);
        let has_rounds = module_type(w, r.type_id).is_none_or(|t| {
            !takes_charges(&t)
                // An ancillary repairer cycles empty, at reduced effect.
                || is_ancillary(r.effect.effect_id)
                || runs_empty(&t)
                || w.item(r.module_id).is_some_and(|m| {
                    // A cycle's worth, not one round: a gun that takes two
                    // and has one left cannot fire and must stop, not cycle
                    // on throwing nothing.
                    let rate = t
                        .attribute(attr::CHARGE_RATE)
                        .map_or(1, |x| (x.round() as i64).max(1));
                    loaded(w.store(), r.ship_id, m.flag_id).is_some_and(|c| c.quantity >= rate)
                })
        });
        if !has_rounds {
            note_defense(w, &mut defended, &r);
            w.modules_mut().end(r.module_id, at);
            out.extend(stop_pushes(w, &r, at));
            continue;
        }
        if crate::cyno::is_cyno(r.effect.effect_id) {
            // A cynosural field's next cycle is paid in ozone; a stack
            // that cannot pay it stops the module, and the field with it
            // (`crate::cyno::tick`).
            let paid = match via {
                Via::Clock => crate::sov_hub::on_the_clock(w, |w| crate::cyno::pay_cycle(w, &r)),
                Via::Answer => crate::cyno::pay_cycle(w, &r),
            };
            if let Err(error) = paid {
                note_defense(w, &mut defended, &r);
                w.modules_mut().end(r.module_id, at);
                let mut stop = stop_pushes(w, &r, at);
                if let Some(ModulePush::Effect(e)) = stop.first_mut() {
                    e.error = Some(error);
                }
                out.extend(stop);
                continue;
            }
        }
        match pay(w, r.character_id, r.ship_id, ship_type_id, need, at) {
            Err(()) => {
                note_defense(w, &mut defended, &r);
                // Out of capacitor mid-run: the module stops and says why.
                let got = capacitor_at(w, r.ship_id, ship_type_id, at).0;
                w.modules_mut().end(r.module_id, at);
                let mut stop = stop_pushes(w, &r, at);
                if let Some(ModulePush::Effect(e)) = stop.first_mut() {
                    e.error = Some(not_enough_energy(w, r.type_id, need, got));
                }
                out.extend(stop);
            }
            Ok(push) => {
                out.extend(push);
                // A siege, triage or core module's cycle is paid in its fuel
                // too (NW-1), but only once the capacitor is known to be
                // paid: a cycle stopped for want of energy burns none. The
                // stop says which fuel is missing.
                let paid = match via {
                    Via::Clock => {
                        crate::sov_hub::on_the_clock(w, |w| crate::module_fuel::pay(w, &r))
                    }
                    Via::Answer => crate::module_fuel::pay(w, &r),
                };
                if let Err(error) = paid {
                    note_defense(w, &mut defended, &r);
                    w.modules_mut().end(r.module_id, at);
                    let mut stop = stop_pushes(w, &r, at);
                    if let Some(ModulePush::Effect(e)) = stop.first_mut() {
                        e.error = Some(error);
                    }
                    out.extend(stop);
                    continue;
                }
                let fire =
                    fire_rounds(w, r.character_id, r.ship_id, r.module_id, at).unwrap_or_default();
                let charge_type_id = fire.charge_type_id;
                note_fire(w, r.module_id, r.cycles, &fire);
                out.extend(fire.pushes);
                out.extend(launch_fx(w, &r, charge_type_id, r.period / MSEC));
                let m = w.modules_mut();
                if let Some(live) = m.running.get_mut(&r.module_id) {
                    live.cycles += 1;
                }
                m.record(Cycle {
                    character_id: r.character_id,
                    ship_id: r.ship_id,
                    module_id: r.module_id,
                    type_id: r.type_id,
                    effect_id: r.effect.effect_id,
                    target_id: r.target_id,
                    at,
                    index: r.cycles,
                    charge_type_id,
                });
            }
        }
    }
    // A capacitor whose ship is not in space is forgotten: the next
    // `GetAllInfo` seeds it full (`ship_attributes`), and remembering a drain
    // the client has been told to forget is a bar that disagrees with us.
    let in_space: BTreeSet<i64> = w
        .modules()
        .capacitor
        .keys()
        .copied()
        .filter(|ship| w.roster().by_ship(*ship).is_some())
        .collect();
    w.modules_mut()
        .capacitor
        .retain(|ship, _| in_space.contains(ship));
    send(w, via, out);
    for ((character_id, ship_id), before) in defended {
        defense_changed(w, via, character_id, ship_id, &before, now);
    }
    for (character_id, ship_id) in slowed.keys().copied().collect::<Vec<_>>() {
        // Told only when the flight numbers moved: a siege ending under a
        // cycling MWD changes the mass and the speed while a propulsion
        // module still runs, and an MWD cycling on changes nothing.
        let before = slowed.remove(&(character_id, ship_id)).flatten();
        if hull_moved(before, crate::dogma::hull(w, ship_id)) {
            motion_changed(w, via, character_id, ship_id, before);
        }
    }
    for r in jumps {
        // The scram is asked again at the end of the cycle.
        if crate::mjd::blocked(w, &r, r.next_boundary()).is_none() {
            crate::mjd::jump(w, via == Via::Clock, &r);
        }
    }
    steps
}

/// Stop a running effect now, from the clock, with `error` as the reason the
/// pilot is shown -- `(dialogKey, paramsDict)`, or `None` for a silent stop.
///
/// **For the consumers of a cycle**, which learn only after the fact that the
/// effect has nothing left to act on: `crate::mining`'s full hold and empty
/// rock. The stop pair leaves on the clock queue; `false` if the module was
/// not running, or was already stopping because the player asked (its pair
/// is already queued, and a second would be a second dialog).
pub fn stop(w: &mut Registry, module_id: i64, at: i64, error: Option<PyValue>) -> bool {
    let Some(r) = w.modules().running(module_id).copied() else {
        return false;
    };
    if r.stop_at.is_some() {
        return false;
    }
    let defended = crate::fitstate::live(w, r.ship_id);
    let flew = crate::dogma::hull(w, r.ship_id);
    w.modules_mut().end(module_id, at);
    let mut out = stop_pushes(w, &r, at);
    if let Some(ModulePush::Effect(e)) = out.first_mut() {
        e.error = error;
    }
    send(w, Via::Clock, out);
    defense_changed(w, Via::Clock, r.character_id, r.ship_id, &defended, at);
    if is_propulsion(r.effect.effect_id) {
        motion_changed(w, Via::Clock, r.character_id, r.ship_id, flew);
    }
    if crate::cloak::is_cloak_effect(r.effect.effect_id) {
        // A cloak ended by some route of its own: the ship decloaks.
        crate::cloak::on_module_stopped(w, r.ship_id, at);
    }
    true
}

/// End a running effect **at the end of the cycle that is running**, with
/// `error` as the reason the pilot is shown (`None` for a silent stop): the
/// player's own deactivation path (`deactivate`), taken by the server.
///
/// **For a target that left reach** (`crate::module_range::sweep`), the twin
/// of GUN-CYCLE's `target_lost`: the cycle that runs is paid for, the module
/// stays running (an Activate is `EffectAlreadyActive2`) and the stop pair,
/// carrying `error`, is queued for the boundary. Ending it now let a module
/// whose target drifted out mid-cycle be started again at once. `false` if the
/// module was not running or is already stopping.
pub fn stop_at_boundary(
    w: &mut Registry,
    module_id: i64,
    now: i64,
    error: Option<PyValue>,
) -> bool {
    let Some(r) = w.modules().running(module_id).copied() else {
        return false;
    };
    if r.stop_at.is_some() {
        return false;
    }
    let boundary = r.next_boundary();
    if let Some(live) = w.modules_mut().running.get_mut(&module_id) {
        live.stop_at = Some(boundary);
    }
    let after = Duration::from_nanos(((boundary - now).max(0) as u64) * 100);
    let mut out = stop_pushes(w, &r, boundary);
    if let Some(ModulePush::Effect(e)) = out.first_mut() {
        e.error = error;
    }
    for p in out {
        if !hushed(w, &p) {
            w.schedule(after, p);
        }
    }
    true
}

/// **The world clock's hook.** Brings every running effect up to `now` and
/// sends what that produced on the clock's own queue. Returns how many cycle
/// boundaries it processed.
///
/// Wired from `push::spawn_world_ticker` beside
/// `targeting::sweep_out_of_range`; without it the model is still right (every
/// call advances it first) but a drain or an out-of-capacitor stop waits for
/// the player's next call.
pub fn tick(w: &mut Registry, now: i64) -> usize {
    let n = advance(w, now, Via::Clock);
    stop_for_warp(w, now);
    // A targeted module whose target left its reach stops (`crate::module_range`).
    crate::module_range::sweep(w, now);
    // A cynosural field whose module stopped comes down (CAP-D).
    crate::cyno::tick(w);
    // A warp disrupt probe whose life is over comes down (BUBBLES-SB).
    crate::bubble::tick(w, now);
    // A jump portal whose generator stopped closes (CAP-G).
    crate::jump_portal::tick(w);
    // Drones fly, fight and come home on the same clock (`crate::drones`).
    // Here rather than a line of their own in the world ticker, so the second
    // weapon system costs `push.rs` nothing; before `combat::tick` like the
    // turrets' cycles, and each volley goes out through `combat::fire` at
    // once.
    crate::drones::tick(w, now);
    // Fighter tubes' tasks, landings and the orphan sweep (`crate::fighters`).
    crate::fighters::tick(w, now);
    n
}

/// `dogmaIM.Activate(itemID, effectName, target, repeat)`.
///
/// **Returns `None`** -- the reply is discarded (`smb:1580`, `POP_TOP`) and
/// only releases the client's item lock (`godma:2439`). That is also why
/// every path here ends in a reply or a raise: a dropped call leaves the
/// module locked in `invCache` for the rest of the session (§7.5).
pub fn activate(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    effect_name: &str,
    target_id: Option<i64>,
    repeat: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    advance(w, now, Via::Answer);
    let f = fitted(w, actor, module_id)?;
    // The `online` effect comes through here too: the HUD's "Put Online" is
    // `effect.Activate()` on the online effect (`smb:_ChangeOnline`), while
    // the fitting window's is `SetModuleOnline`.
    if effect_name == "online" {
        return set_online(w, f, true, now);
    }
    if w.roster()
        .get(f.character_id)
        .is_none_or(|p| p.ship_id != f.ship_id)
    {
        return Err(notify("Modules can only be activated in space."));
    }
    if !w.modules().is_online(module_id) {
        // `ModuleNotOnline` (1231), loc arg `[item]module.name`.
        return Err(user_error(
            "ModuleNotOnline",
            vec![(
                "module",
                type_arg_for("ModuleNotOnline", "module", f.type_id),
            )],
        ));
    }
    // A warp scrambler aimed at this ship blocks its MWD (W16 CP57,
    // `crate::tackle::activation_blocked`, `ModuleIsBlocked`).
    if let Some(refusal) = crate::tackle::activation_blocked(w, f.ship_id, f.type_id, now) {
        return Err(refusal.into_error());
    }
    let Some(t) = module_type(w, f.type_id) else {
        return Err(notify("This server cannot cycle that module yet."));
    };
    let Some(effect) = t.effect.filter(|e| e.name == effect_name) else {
        return Err(notify("This server cannot cycle that effect yet."));
    };
    // HIC-SCRIPT: a field generator with a focused script is a targeted
    // point, without one a bubble that names no target.
    let (effect, target_id) = crate::tackle::focus(
        w,
        f.ship_id,
        w.item(module_id).map_or(0, |i| i.flag_id),
        effect,
        target_id,
    )?;
    // `disallowActivateOnWarp`: not from a ship in an activated warp.
    crate::module_range::check_warp(w, f.character_id, &t)?;
    if w.modules().running(module_id).is_some() {
        // CCP's own key and dict: `bdl:926` and `bdl:931` `[V]`.
        return Err(user_error(
            "EffectAlreadyActive2",
            vec![("modulename", type_arg(f.type_id))],
        ));
    }
    check_reactivation(w, f.ship_id, module_id, f.type_id, now)?;
    check_group_active(w, f.ship_id, module_id, f.type_id)?;
    // CLOAK-1: nothing but a probe launcher runs while cloaked; the cloak
    // itself has its own list.
    crate::cloak::check_other_activation(w, f.ship_id, effect.effect_id, f.type_id)?;
    let is_cloak = crate::cloak::is_cloak_effect(effect.effect_id);
    if is_cloak {
        // NW-9a: a titan that has fired a doomsday does not cloak.
        crate::superweapon::check_cloak(w, f.ship_id, now)?;
        crate::cloak::check_activation(w, f.character_id, f.ship_id, module_id, effect.effect_id)?;
    }
    if needs_lock(&effect, target_id)
        && target_id.is_none_or(|t| !w.locks().targets_of(f.ship_id).contains(&t))
    {
        // The client asks for a lock before calling (`smb:1541`), so this is
        // a lock lost in flight.
        return Err(user_error("DeniedActivateTargetNotPresent", vec![]));
    }
    // CW-1: an offensive effect the pilot's safety forbids (the client
    // refuses first; this is the backstop), `crate::crimewatch::attack_refusal`.
    if effect.is_offensive
        && let Some(target) = target_id
        && let Some(refusal) =
            crate::crimewatch::attack_refusal(w, f.character_id, Some(f.type_id), target, now)
    {
        return Err(refusal.into_error());
    }
    // NW-9a: a legacy doomsday names a capital ship that is not immune.
    if let Some(refusal) =
        crate::superweapon::legacy_refusal(w, f.type_id, effect.effect_id, target_id)
    {
        return Err(refusal.into_error());
    }
    // K-LOGI: the safety's word on helping a flagged pilot
    // (`crate::remote_rep::activation_refusal`).
    if let Some(refusal) = crate::remote_rep::activation_refusal(
        w,
        f.character_id,
        f.type_id,
        effect.effect_id,
        target_id,
        now,
    ) {
        return Err(refusal.into_error());
    }
    // Every other targeted module: the target within its reach, surface to
    // surface, both ships at now (`crate::module_range`).
    crate::module_range::check_activation(w, f.ship_id, module_id, &t, &effect, target_id)?;
    // EWAR-2C: the target's flags and impedance for the ten player kinds.
    crate::ewar::check_activation(w, &t, effect.effect_id, target_id)?;
    // UNDOCK-INVULN: an offensive module gives up the protection (every
    // refusal above leaves it alone).
    if effect.is_offensive {
        crate::invuln::break_for(w, f.character_id);
    }
    // CLOAK-2: any module but a Warp Core Stabilizer (3123) ends the gate
    // cloak; every refusal above leaves it alone.
    crate::cloak::break_for_module(w, f.character_id, f.type_id);
    if takes_charges(&t) {
        let flag_id = w.item(module_id).map_or(0, |i| i.flag_id);
        // An ancillary repairer runs without a charge (at reduced effect).
        if !is_ancillary(effect.effect_id)
            && !runs_empty(&t)
            && loaded(w.store(), f.ship_id, flag_id).is_none_or(|c| c.quantity <= 0)
        {
            // The client raises this key itself, with this dict, for a
            // launcher with nothing loaded (`smb:1539`) [V]; a turret gets
            // past that check (`GetRelevantEffect` returns its own effect,
            // `smb:1515`), so **[A]** the server refuses it in the same words.
            return Err(user_error(
                "NoCharges",
                vec![("launcher", type_arg(f.type_id))],
            ));
        }
        if w.modules().reloading_until(module_id, now).is_some() {
            // **[A]** CCP's key for it (`findings/user-errors.tsv`, loc arg
            // `[item]moduleTypeID.name`); no raise site in the client.
            return Err(user_error(
                "CantActivateReloadingModule",
                vec![(
                    "moduleTypeID",
                    type_arg_for("CantActivateReloadingModule", "moduleTypeID", f.type_id),
                )],
            ));
        }
    }
    if crate::mining::is_mining(effect.effect_id) {
        // The client checks nothing about a mining target before calling --
        // only that *a* target is locked (`smb:1541-1547`) `[V]` -- so the
        // rock (or gas cloud, SOC-B2), the range and the hold are ours to
        // check.
        crate::mining::check_activation(
            w,
            f.character_id,
            f.ship_id,
            module_id,
            f.ship_type_id,
            &t,
            target_id,
        )?;
    }
    if crate::salvage::is_salvage_effect(effect.effect_id) {
        // A salvager or a tractor beam (X-SALV): the wreck, its lock, its
        // range, its loot and the room for the salvage are ours to check.
        crate::salvage::check_activation(
            w,
            f.character_id,
            f.ship_id,
            f.ship_type_id,
            &t,
            module_id,
            target_id,
        )?;
    }
    if effect.effect_id == crate::hacking::EFFECT_DO_HACKING {
        // A data or relic analyzer (R-HACK HK2): the can, its lock, its
        // state, the skill and the range are ours to check; on success the
        // game is armed for the end of this cycle.
        crate::hacking::check_activation(w, f.character_id, f.ship_id, &t, module_id, target_id)?;
    }
    if effect.effect_id == crate::entosis::EFFECT_ENTOSIS_LINK {
        // An Entosis Link (SOC-D3a): the hub, its lock, its range, the fuel,
        // the alliance and the window are ours to check.
        crate::entosis::check_activation(
            w,
            f.character_id,
            f.ship_id,
            &t,
            module_id,
            target_id,
            now,
        )?;
    }
    if crate::cyno::is_cyno(effect.effect_id) {
        // A Cynosural Field Generator (CAP-D): the fleet, the warp, the
        // security class, the speed, the group and the ozone are ours to
        // check; the client checks none of them.
        crate::cyno::check_activation(w, f.character_id, f.ship_id, &t, module_id)?;
    }
    // NW-1: a siege, triage or core module wants its first cycle's fuel.
    crate::module_fuel::check(
        w,
        f.character_id,
        f.ship_id,
        module_id,
        effect.effect_id,
        f.type_id,
    )?;
    // BUBBLES-SB: a sphere launcher or a field generator wants a probe, the
    // skills, and a system that allows bubbles.
    crate::bubble::check_activation(
        w,
        f.character_id,
        f.ship_id,
        f.type_id,
        effect.effect_id,
        w.item(module_id).map_or(0, |i| i.flag_id),
    )?;
    // A Jump Portal Generator starts only from `Bridge To` (CAP-G): the
    // module has no destination of its own.
    crate::jump_portal::check_activation(w, effect.effect_id, module_id)?;
    // MN-D: a survey launcher wants its skills, and launches once.
    crate::moon_scan::check_activation(
        w,
        f.character_id,
        f.ship_id,
        f.type_id,
        w.item(module_id).map_or(0, |i| i.flag_id),
    )?;
    let repeat = crate::moon_scan::cycles(w.content(), f.type_id, repeat);
    // A sphere launcher launches once (BUBBLES-SB).
    let repeat = crate::bubble::cycles(w, f.type_id, repeat);
    // The Micro Jump Drive: the spool is one cycle (`disallowRepeatingActivation`
    // below); an immobile ship cannot jump.
    if crate::mjd::is_mjd(effect.effect_id) {
        crate::mjd::check_activation(w, f.ship_id)?;
    }
    // `disallowRepeatingActivation` (1014) on the type: one cycle, whatever the
    // client asked for. Scanners, analyzers and the Micro Jump Drive carry it [V-data].
    let repeat = repeat_allowed(&t, repeat);
    // **A cloak has no cycle** (duration attribute 0): it runs until it is
    // deactivated, and the client is told `-1` (no ramp).
    let (duration_ms, repeat) = if is_cloak {
        (-1, 1)
    } else {
        (duration_ms(w, f.ship_id, module_id, &t), repeat)
    };
    if duration_ms <= 0 && !is_cloak {
        // Every activatable effect in the client's table has a duration; this is the
        // `EffectCantHaveNegativeDuration` data bug CCP's own code guards
        // (`bdl:1028`), and a table row is what would cause it.
        return Err(notify("This server has no cycle time for that module."));
    }
    let running = Running {
        character_id: f.character_id,
        ship_id: f.ship_id,
        module_id,
        type_id: f.type_id,
        effect,
        target_id,
        started: now,
        period: if is_cloak {
            crate::cloak::CLOAK_PERIOD
        } else {
            duration_ms * MSEC
        },
        repeat,
        cycles: 1,
        stop_at: None,
    };
    let start = ShipEffect {
        to: vec![f.character_id],
        module_id,
        effect_id: effect.effect_id,
        character_id: f.character_id,
        ship_id: f.ship_id,
        target_id,
        t: now,
        start: true,
        duration_ms,
        repeat,
        error: None,
    };
    let need = capacitor_need(w, f.ship_id, module_id, f.type_id);
    let Ok(drain) = pay(w, f.character_id, f.ship_id, f.ship_type_id, need, now) else {
        // **[A]** retail raises rather than idling the module (docs/re/
        // modules.md §7.4); falsified by a retail capture of a silent idle.
        let got = capacitor_at(w, f.ship_id, f.ship_type_id, now).0;
        return Err(energy_error(w, f.type_id, need, got));
    };
    // Checked above: a gun that takes charges has at least one round.
    let fire = fire_rounds(w, f.character_id, f.ship_id, module_id, now).unwrap_or_default();
    w.modules_mut().fired.remove(&module_id);
    note_fire(w, module_id, 0, &fire);
    let (charge_type_id, count) = (fire.charge_type_id, fire.pushes);
    // A propulsion module changes the top speed the moment it runs: the
    // flight so far is integrated at the old one first.
    let propulsion = is_propulsion(effect.effect_id);
    if propulsion {
        w.advance_character(f.character_id);
    }
    let defended = crate::fitstate::live(w, f.ship_id);
    let flew = crate::dogma::hull(w, f.ship_id);
    w.modules_mut().running.insert(module_id, running);
    // A stop pair booked by an earlier run of this module (a click on a
    // module that then ended some other way) is stale: sent now, it would
    // tell the client this run has stopped.
    w.cancel_scheduled(|p| {
        matches!(
            p.cancel_tag(),
            Some(crate::notify::CancelTag::ModuleStop { module }) if module == module_id
        )
    });
    if propulsion {
        motion_changed(w, Via::Answer, f.character_id, f.ship_id, flew);
    }
    defense_changed(w, Via::Answer, f.character_id, f.ship_id, &defended, now);
    if crate::cyno::is_cyno(effect.effect_id) {
        // The first cycle's ozone, the field's ball and the fleet's beacon.
        crate::cyno::on_activated(w, &running);
    }
    // The first cycle's fuel, found by `check` a moment ago.
    if let Err(e) = crate::module_fuel::pay(w, &running) {
        crate::errln!("[module_fuel] module {module_id} started without its fuel: {e:?}");
    }
    w.modules_mut().record(Cycle {
        character_id: f.character_id,
        ship_id: f.ship_id,
        module_id,
        type_id: f.type_id,
        effect_id: effect.effect_id,
        target_id,
        at: now,
        index: 0,
        charge_type_id,
    });
    push_effect(w, start);
    if is_cloak {
        crate::cloak::on_activated(w, &running, now);
    }
    // **A probe launcher launches on activation** (P5.10): from the HUD, as
    // many probes as are loaded up to the eight a pilot may have out
    // `[A]`. The probe scanner window's own launch is `LaunchProbes`.
    if crate::scan::is_probe_launcher_in(w.content(), f.type_id) {
        crate::scan::launch(
            w,
            f.character_id,
            module_id,
            crate::scan::MAX_PROBES as i64,
            now,
        );
    }
    // MN-D: a survey launcher sends its probe.
    crate::moon_scan::on_activated(w, f.character_id, f.type_id, charge_type_id, now);
    // BUBBLES-SB: a sphere launcher sends its probe.
    crate::bubble::on_activated(w, f.character_id, f.type_id, charge_type_id, now);
    // A cloak's module guid has no class and names a ball the observers are
    // about to lose: no `Fx`.
    if !effect.guid.is_empty() && !is_cloak {
        let to = observers(w, f.character_id);
        w.push(ModulePush::Fx(SpecialFx {
            to,
            ship_id: f.ship_id,
            module_id,
            type_id: f.type_id,
            target_id,
            guid: effect.guid,
            is_offensive: effect.is_offensive,
            start: true,
            duration_ms,
            repeat,
        }));
    }
    if let Some(fx) = launch_fx(w, &running, charge_type_id, duration_ms) {
        w.push(fx);
    }
    if let Some(drain) = drain {
        w.push(drain);
    }
    for count in count {
        w.push(count);
    }
    Ok(PyValue::None)
}

/// `dogmaIM.Deactivate(itemID, effectName)`. Returns `None`.
///
/// **[I]** the effect stops at the end of the cycle that is running, which is
/// what EVE does and what the client is built for: `StateManager.Deactivate`
/// marks the effect `isDeactivating` before calling us (`godma:2040` in
/// uncompyle6's numbering) and only our `start=0` clears it. So the stop pair
/// is scheduled for the cycle boundary rather than sent now.
///
/// A module the client thinks is running and we do not is told to stop
/// **now** -- the only way to bring the two back into agreement.
pub fn deactivate(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    effect_name: &str,
    now: i64,
) -> Result<PyValue, ServiceError> {
    advance(w, now, Via::Answer);
    let f = fitted(w, actor, module_id)?;
    if effect_name == "online" {
        return set_online(w, f, false, now);
    }
    match w.modules().running(module_id).copied() {
        // Asked again while it is already stopping: the same refusal, with
        // the time that is left (NW-1).
        Some(r) if r.stop_at.is_some() => {
            if let Some(e) = crate::module_fuel::still_active(w, &r, r.stop_at.unwrap_or(now), now)
            {
                return Err(e);
            }
        }
        // **A cloak has no cycle boundary**: it ends now (CLOAK-1).
        Some(r) if crate::cloak::is_cloak_effect(r.effect.effect_id) => {
            if !crate::cloak::uncloak(w, r.ship_id, crate::cloak::Cause::Manual, now, false) {
                stop(w, module_id, now, None);
            }
        }
        Some(r) => {
            let boundary = r.next_boundary();
            if let Some(live) = w.modules_mut().running.get_mut(&module_id) {
                live.stop_at = Some(boundary);
            }
            let after = Duration::from_nanos(((boundary - now).max(0) as u64) * 100);
            for p in stop_pushes(w, &r, boundary) {
                if !hushed(w, &p) {
                    w.schedule(after, p);
                }
            }
            // `disallowEarlyDeactivation` (906): it still stops at the
            // boundary, and the click is told how long that is (NW-1).
            if let Some(e) = crate::module_fuel::still_active(w, &r, boundary, now) {
                return Err(e);
            }
        }
        None => {
            if let Some(effect) = module_type(w, f.type_id)
                .and_then(|t| t.effect)
                .filter(|e| e.name == effect_name)
            {
                push_effect(
                    w,
                    ShipEffect {
                        to: vec![f.character_id],
                        module_id,
                        effect_id: effect.effect_id,
                        character_id: f.character_id,
                        ship_id: f.ship_id,
                        target_id: None,
                        t: now,
                        start: false,
                        duration_ms: module_type(w, f.type_id).map_or(-1, |t| t.duration_ms()),
                        repeat: 0,
                        error: None,
                    },
                );
            }
        }
    }
    Ok(PyValue::None)
}

/// Online or offline, by any of the four routes.
///
/// - **Already in that state.** Online-when-online raises
///   `EffectAlreadyActive2`, which is CCP's key for it (`bdl:926`) and the
///   one key `clientDogmaLocation.OnlineModule` does *not* undo its optimistic
///   state for (`findings/user-errors.tsv`) `[V]`. Offline-when-offline is
///   answered `None` with the stop restated: there is no key for it, and the
///   client is already showing what is true.
/// - **Otherwise** the state changes and godma is told with the online
///   effect's `OnGodmaShipEffect` -- **[I]** which is what draws the HUD
///   button lit or dim (`smb:1505-1507`); `clientDogmaLocation` already
///   changed its own state before calling.
/// - **Going offline stops whatever was running, now** **[A]**.
fn set_online(w: &mut Registry, f: Fitted, on: bool, now: i64) -> Result<PyValue, ServiceError> {
    let actor = Some(f.character_id);
    crate::fitstate::around(w, actor, |w| set_online_inner(w, f, on, now))
}

fn set_online_inner(
    w: &mut Registry,
    f: Fitted,
    on: bool,
    now: i64,
) -> Result<PyValue, ServiceError> {
    // An overdrive or a nanofiber going on or off line changes the top speed
    // too: the flight so far is integrated at the old one.
    let before = crate::dogma::hull(w, f.ship_id);
    if before.is_some() {
        w.advance_character(f.character_id);
    }
    if on {
        // A burnt-out module stays offline until it is repaired (K-OVERHEAT).
        crate::overheat::refuse_online(w, f.module_id)?;
    }
    if on && !w.modules().is_online(f.module_id) {
        // Skills first, as the client's `online.py` does, before CPU and
        // powergrid. Only a bot or a forged call gets here: a human's own
        // dogma raised it locally (SKILLGATE).
        crate::skillgate::check(w, f.character_id, f.type_id, "OnlineHasSkillPrerequisites")?;
        budget(w, f.ship_id, f.ship_type_id, f.module_id, f.type_id)?;
    }
    let changed = set_online_kept(w, f.module_id, on);
    if !changed && on {
        return Err(user_error(
            "EffectAlreadyActive2",
            vec![("modulename", type_arg(f.type_id))],
        ));
    }
    if !on && let Some(r) = w.modules_mut().end(f.module_id, now) {
        let stop = stop_pushes(w, &r, now);
        send(w, Via::Answer, stop);
    }
    w.push(online_push(f.character_id, f.ship_id, f.module_id, on, now));
    if before.is_some() {
        let now_hull = crate::dogma::hull(w, f.ship_id);
        if before.map(|h| h.max_velocity) != now_hull.map(|h| h.max_velocity)
            || before.map(|h| h.mass) != now_hull.map(|h| h.mass)
            || before.map(|h| h.agility) != now_hull.map(|h| h.agility)
        {
            motion_changed(w, Via::Answer, f.character_id, f.ship_id, before);
        }
    }
    Ok(PyValue::None)
}

/// The `(key, dict)` of `NotEnoughEnergy` (259233): "{effectname} requires
/// {need} units of charge. The capacitor has only {got} units." The key and
/// the three args are the client's [V]; `effectname` as the module's name
/// is **[A]**.
fn not_enough_energy(w: &Registry, type_id: i64, need: f64, got: f64) -> PyValue {
    let name = crate::item::find_in(w.content(), type_id)
        .map_or_else(|| format!("type {type_id}"), |t| t.name.to_owned());
    PyValue::Tuple(vec![
        crate::pyutil::bstr("NotEnoughEnergy"),
        PyValue::Dict(crate::pyutil::pypairs(vec![
            ("effectname", crate::pyutil::bstr(&name)),
            ("need", PyValue::Real(need)),
            ("got", PyValue::Real(got)),
        ])),
    ])
}

/// [`not_enough_energy`] as the activation's refusal.
pub(crate) fn energy_error(w: &Registry, type_id: i64, need: f64, got: f64) -> ServiceError {
    let PyValue::Tuple(mut parts) = not_enough_energy(w, type_id, need, got) else {
        unreachable!("a (key, dict) pair")
    };
    let PyValue::Dict(d) = parts.remove(1) else {
        unreachable!("the dict")
    };
    ServiceError::Raise(eve_proto::user_error("NotEnoughEnergy", d))
}

/// A fitted module's attribute `a`: the dogma sheet's modified value when it
/// is not zero, else the type's own, else the module row's.
pub(crate) fn module_attr(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    a: i64,
) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a)
        .filter(|v| *v != 0.0)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, a))
        .or_else(|| find_in(w.content(), type_id).and_then(|t| t.attribute(a)))
}

/// `maxGroupActive` (763) and the hull's `maxShipGroupActiveID` / `...Active`
/// pair (909 / 910): the client's own check, `fittingDogmaLocation.
/// _CheckIfTooManyModulesInGroupAreActive` [V]. The fitted modules of this
/// module's group that are active -- one whose stop is scheduled still counts,
/// the client's `IsActive` holds until the stop lands -- must number fewer
/// than the limit, or the activation is `EffectCrowdedOut{module, count}`
/// (the module is a bare type id, as the client sends it).
pub fn check_group_active(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
) -> Result<(), ServiceError> {
    let group = w.item(module_id).map_or(0, |i| i.group_id);
    let mut max_active = module_attr(w, ship_id, module_id, type_id, ATTR_MAX_GROUP_ACTIVE)
        .map_or(0, |v| v.round() as i64);
    if let Some(hull) = w.item(ship_id).map(|s| s.type_id) {
        let hull_attr = |a| crate::typedata::attribute_in(w.content(), hull, a);
        if hull_attr(ATTR_MAX_SHIP_GROUP_ACTIVE_ID).is_some_and(|g| g.round() as i64 == group) {
            max_active = max_active
                .max(hull_attr(ATTR_MAX_SHIP_GROUP_ACTIVE).map_or(0, |v| v.round() as i64));
        }
    }
    if max_active <= 0 {
        return Ok(());
    }
    let active = w
        .store()
        .contents_of(ship_id)
        .filter(|i| i.group_id == group && w.modules().running(i.item_id).is_some())
        .count() as i64;
    if active >= max_active {
        return Err(user_error(
            "EffectCrowdedOut",
            vec![
                ("module", PyValue::Int(type_id)),
                ("count", PyValue::Int(active)),
            ],
        ));
    }
    Ok(())
}

/// `reactivationDelay` (669, milliseconds): a module whose effect ended less
/// than that long ago is refused with `ModuleReactivationDelayed2`. The
/// client's catcher reads `timeSinceLastStop` (ms) and `itemID` to draw the
/// button's cooldown ring (`shipmodulebutton` 1606-1617 [V]); the message
/// reads `delay` and `item`. **[A]** `delay` is the time left in blue ticks
/// and `item` the `(UE_TYPEID, id)` pair; falsified by a dialog that shows an
/// absurd interval or a bare number for the module.
fn check_reactivation(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    now: i64,
) -> Result<(), ServiceError> {
    let Some(delay_ms) = module_attr(w, ship_id, module_id, type_id, ATTR_REACTIVATION_DELAY)
        .map(|v| v.round() as i64)
        .filter(|d| *d > 0)
    else {
        return Ok(());
    };
    let Some(stopped) = w.modules().last_stop(module_id) else {
        return Ok(());
    };
    let until = stopped + delay_ms * MSEC;
    if now >= until {
        return Ok(());
    }
    Err(user_error(
        "ModuleReactivationDelayed2",
        vec![
            (
                "timeSinceLastStop",
                PyValue::Int((now - stopped).max(0) / MSEC),
            ),
            ("itemID", PyValue::Int(module_id)),
            ("delay", PyValue::Int(until - now)),
            ("item", type_arg(type_id)),
        ],
    ))
}

/// **A ship whose warp has gone active shuts off its non-warp-safe effects**
/// (`isWarpSafe` false on the effect row: afterburners and microwarpdrives
/// among them). The instant is the one `targeting` drops locks at,
/// `Motion::warp_active_at`. The pilot is
/// told `EffectDeactivationWarping` (259511, no args). An effect whose row
/// carries no `isWarpSafe` (`None`) is left running.
fn stop_for_warp(w: &mut Registry, now: i64) {
    let wall = crate::space_state::now_seconds();
    let doomed: Vec<i64> = w
        .modules()
        .running
        .values()
        .filter(|r| r.stop_at.is_none() && r.effect.warp_safe == Some(false))
        .filter(|r| {
            w.roster()
                .get(r.character_id)
                .is_some_and(|p| p.ship_id == r.ship_id && p.motion.warp_active_at(wall))
        })
        .map(|r| r.module_id)
        .collect();
    for module_id in doomed {
        let error = PyValue::Tuple(vec![
            crate::pyutil::bstr("EffectDeactivationWarping"),
            PyValue::Dict(Vec::new()),
        ]);
        stop(w, module_id, now, Some(error));
    }
}

/// The effects that change the ship's top speed while they run:
/// `moduleBonusAfterburner` / `moduleBonusMicrowarpdrive` (`crate::dogma`
/// rule 5), and the cynosural field's, whose `speedFactor` of -100 is a
/// post-percent on `maxVelocity` **[V-data]** (`crate::cyno`: the ship is
/// held still while its field is lit).
fn is_propulsion(effect_id: i64) -> bool {
    effect_id == crate::dogma::effect::AFTERBURNER
        || effect_id == crate::dogma::effect::MICROWARPDRIVE
        || crate::cyno::is_cyno(effect_id)
        || crate::jump_portal::is_portal(effect_id)
        // A siege, triage or bastion module and the industrial cores: speed
        // -100 and a mass multiplier (NW-1).
        || crate::module_fuel::is_siege_like(effect_id)
        // The cloak's `maxVelocityModifier` is a PostMul on `maxVelocity`.
        || crate::cloak::is_cloak_effect(effect_id)
}

/// Whether the flight numbers of two hulls differ: top speed, mass or
/// agility. Without a hull on either side there is nothing to say; with only
/// one, the park is told everything.
fn hull_moved(
    before: Option<crate::destiny::hull::Hull>,
    now: Option<crate::destiny::hull::Hull>,
) -> bool {
    match (before, now) {
        (Some(b), Some(h)) => {
            (f64::from(b.max_velocity) - f64::from(h.max_velocity)).abs() > 1e-6
                || (b.mass - h.mass).abs() > 1e-6
                || (f64::from(b.agility) - f64::from(h.agility)).abs() > 1e-6
        }
        (None, None) => false,
        _ => true,
    }
}

/// Remember the hull `r`'s ship flies with before `r` ends (first one wins).
fn note_slowed(
    w: &Registry,
    slowed: &mut BTreeMap<(i64, i64), Option<crate::destiny::hull::Hull>>,
    r: &Running,
) {
    slowed
        .entry((r.character_id, r.ship_id))
        .or_insert_with(|| crate::dogma::hull(w, r.ship_id));
}

/// Remember the resonances and sensors of `r`'s ship before `r` ends
/// (first one wins).
fn note_defense(w: &Registry, defended: &mut BTreeMap<(i64, i64), Vec<(i64, f64)>>, r: &Running) {
    defended
        .entry((r.character_id, r.ship_id))
        .or_insert_with(|| crate::fitstate::live(w, r.ship_id));
}

/// Tell the pilot's godma each resonance and sensor number
/// ([`crate::fitstate::live`]) of the ship that differs from
/// `before` (taken before a module started or ended), and refresh the combat
/// record so the next volley uses it. The fitting window's defense panel
/// reads them off godma (`crate::fitstate`); a hardener starts at the click
/// and ends on the clock at its cycle boundary, so this mirrors
/// [`speed_changed`] rather than wrapping the dispatch.
// TODO [A] overheat (`overheat.rs`) changes a hardener's resists while it is
// overloaded; not pushed yet. Falsifier: the resist in the fitting window
// does not change when a hardener is overloaded.
fn defense_changed(
    w: &mut Registry,
    via: Via,
    character_id: i64,
    ship_id: i64,
    before: &[(i64, f64)],
    now: i64,
) {
    if w.roster()
        .get(character_id)
        .is_none_or(|here| here.ship_id != ship_id)
    {
        return;
    }
    let old: BTreeMap<i64, f64> = before.iter().copied().collect();
    let mut moved = false;
    for (id, new) in crate::fitstate::live(w, ship_id) {
        let was = old.get(&id).copied().unwrap_or(1.0);
        if (new - was).abs() <= 1e-9 {
            continue;
        }
        moved = true;
        let change = AttributeChange {
            to: vec![character_id],
            owner_id: character_id,
            item_id: ship_id,
            attribute_id: id,
            time: now,
            new_value: new,
            old_value: was,
        };
        match via {
            Via::Answer => w.push(change),
            Via::Clock => w.schedule(Duration::ZERO, change),
        }
    }
    if moved {
        crate::dbuff::refresh_record(w, ship_id, character_id, now);
    }
}

/// Tell the pilot's park, and everyone else's in the system, what moved in
/// the ship's flight numbers: `SetBallAgility`, `SetBallMass` and
/// `SetMaxSpeed`, in that order and in one batch, each only when it moved by
/// more than 1e-6 against `before` -- the hull the ship flew with before the
/// change -- or all three when `before` is `None` (MJD-MASS). The client's
/// ego ball learns a new mass or agility from nothing else
/// (`flightControls` is the only Python caller of the setters). Only when
/// `crate::dogma` has a sheet: without one nothing changed. A pilot not in
/// space has no ball to tell. The godma pushes always go, for the tooltips.
fn motion_changed(
    w: &mut Registry,
    via: Via,
    character_id: i64,
    ship_id: i64,
    before: Option<crate::destiny::hull::Hull>,
) {
    let Some(h) = crate::dogma::hull(w, ship_id) else {
        return;
    };
    let Some(here) = w.roster().get(character_id).copied() else {
        return;
    };
    if here.ship_id != ship_id {
        return;
    }
    // **The pilot's godma holds the ship's mass, top speed and agility as
    // the row at load gave them** (`service.rs::ship_attributes_of`), and the
    // propulsion tooltip reads them (`moduleButtonTooltip.AddPropulsionModuleInfo`:
    // `maxVelocity` divided by the boost ratio while the module runs, times
    // it while it does not) [V]. Unpushed, the tooltip kept the figure from
    // before the burn: ~220 m/s until something re-primed the ship (RIG-CAL).
    for (id, v) in [
        (crate::dogma::attr::MASS, h.mass),
        (crate::dogma::attr::MAX_VELOCITY, f64::from(h.max_velocity)),
        (crate::dogma::attr::AGILITY, f64::from(h.agility)),
    ] {
        let change = AttributeChange {
            to: vec![character_id],
            owner_id: character_id,
            item_id: ship_id,
            attribute_id: id,
            time: crate::pyutil::filetime_now(),
            new_value: v,
            old_value: v,
        };
        match via {
            Via::Answer => w.push(change),
            Via::Clock => w.schedule(Duration::ZERO, change),
        }
    }
    let mut to = vec![character_id];
    to.extend(
        w.roster()
            .others_in_system(here.solar_system_id, character_id),
    );
    to.retain(|id| w.character(*id).is_some());
    let stamp = w.next_destiny_stamp();
    let moved = |was: Option<f64>, now: f64| was.is_none_or(|was| (now - was).abs() > 1e-6);
    let mut actions = Vec::new();
    if moved(before.map(|b| f64::from(b.agility)), f64::from(h.agility)) {
        actions.push(crate::destiny::Action::SetBallAgility {
            ball: ship_id,
            agility: f64::from(h.agility),
        });
    }
    if moved(before.map(|b| b.mass), h.mass) {
        actions.push(crate::destiny::Action::SetBallMass {
            ball: ship_id,
            mass: h.mass,
        });
    }
    if moved(
        before.map(|b| f64::from(b.max_velocity)),
        f64::from(h.max_velocity),
    ) {
        actions.push(crate::destiny::Action::SetMaxSpeed {
            ball: ship_id,
            max: f64::from(h.max_velocity),
        });
    }
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        match via {
            Via::Answer => w.push(update),
            Via::Clock => w.schedule(Duration::ZERO, update),
        }
    }
}

/// [`motion_changed`] on the answer path, for the tests of `crate::mjd`.
#[cfg(test)]
pub(crate) fn motion_changed_for_test(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    before: Option<crate::destiny::hull::Hull>,
) {
    motion_changed(w, Via::Answer, character_id, ship_id, before);
}

/// **Stop every effect running on a ship, now** (BE-2): the pilot is leaving
/// it, and a hull nobody flies must not keep cycling a gun. The stop pair of
/// each goes to the pilot's client and the observers', as going offline does
/// ([`set_online`]). Module and capacitor state are kept (`forget_ship` is
/// not called): the hull may be boarded again.
pub fn stop_ship(w: &mut Registry, ship_id: i64, now: i64) {
    let running: Vec<i64> = w
        .modules()
        .running_all()
        .iter()
        .filter(|r| r.ship_id == ship_id)
        .map(|r| r.module_id)
        .collect();
    for module_id in running {
        if let Some(r) = w.modules_mut().end(module_id, now) {
            let stop = stop_pushes(w, &r, now);
            send(w, Via::Answer, stop);
        }
    }
}

/// The cycle time of a fitted module's effect, in ms: the **modified** value
/// of the effect's duration attribute (rate-of-fire skills, ship bonuses,
/// gyrostabilizers) when `crate::dogma` has a sheet, the type's own
/// otherwise -- `ModuleButton.GetEffectTiming` reads the same attribute off
/// the client's own dogma item (`smb:1812`).
fn duration_ms(w: &Registry, ship_id: i64, module_id: i64, t: &ModuleType) -> i64 {
    t.effect
        .and_then(|e| crate::dogma::module_attribute(w, ship_id, module_id, e.duration_attribute))
        .map_or_else(|| t.duration_ms(), |ms| ms.round() as i64)
}

/// `capacitorNeed` (6) of a fitted module, modified (Fuel Conservation,
/// High Speed Maneuvering, ...) when there is a sheet.
fn capacitor_need(w: &Registry, ship_id: i64, module_id: i64, type_id: i64) -> f64 {
    crate::dogma::module_attribute(w, ship_id, module_id, attr::CAPACITOR_NEED)
        .or_else(|| find_in(w.content(), type_id).map(|t| t.capacitor_need()))
        .unwrap_or(0.0)
}

/// Would onlining this module overrun the ship's CPU or powergrid?
///
/// **CCP's own test, shipped in the client** (`online.py:56,62`) [V]: a
/// module with a non-zero `cpu` is refused when `cpuLoad + cpuRequired >
/// cpuOutput + 1e-06`, then the same for power. The load is the sum over the
/// ship's *online* fitted modules other than this one; the outputs are
/// `crate::fitting::HULLS`' raw hull values (no skills yet, spec §12 risk 3).
/// A hull or module this server has no numbers for is not refused.
///
/// The keys are CCP's; the argument dict is the dialog's own `loc_args`
/// (`findings/user-errors.tsv` rows 1341/1348) **[A]** -- the real server
/// fills them in code we do not have.
fn budget(
    w: &Registry,
    ship_id: i64,
    ship_type_id: i64,
    module_id: i64,
    type_id: i64,
) -> Result<(), ServiceError> {
    let (Some(h), Some(t)) = (
        crate::fitting::hull_in(w.content(), ship_type_id),
        find_in(w.content(), type_id),
    ) else {
        return Ok(());
    };
    // **The modified numbers when there are any** (`crate::dogma`): CPU
    // Management and Power Grid Management on the outputs, Weapon Upgrades
    // and friends on each module's need -- what the client's own fitting
    // window compares (`online.py:56,62` reads `cpuOutput`/`cpu` off its
    // dogma items).
    if let (Some(sheet), Some(inputs)) = (
        crate::dogma::sheet(w, ship_id),
        crate::dogma::inputs(w, ship_id),
    ) {
        let (cpu, power) = sheet.load(&inputs, Some(module_id));
        let need_cpu = sheet.module(module_id, attr::CPU).unwrap_or(t.cpu());
        let need_power = sheet.module(module_id, attr::POWER).unwrap_or(t.power());
        return refuse_over(
            type_id,
            (cpu, need_cpu, sheet.ship(crate::dogma::attr::CPU_OUTPUT)),
            (
                power,
                need_power,
                sheet.ship(crate::dogma::attr::POWER_OUTPUT),
            ),
        );
    }
    let m = w.modules();
    let (mut cpu, mut power) = (0.0, 0.0);
    for i in w.store().contents_of(ship_id).filter(|i| {
        i.item_id != module_id
            && i.singleton
            && crate::item::is_ship_fitting_flag(i.flag_id)
            && m.is_online(i.item_id)
    }) {
        if let Some(o) = find_in(w.content(), i.type_id) {
            cpu += o.cpu();
            power += o.power();
        }
    }
    refuse_over(
        type_id,
        (cpu, t.cpu(), h.cpu_output),
        (power, t.power(), h.power_output),
    )
}

/// `online.py:56,62`'s two comparisons, each `(load, need, total)`.
fn refuse_over(
    type_id: i64,
    (cpu, need_cpu, cpu_output): (f64, f64, f64),
    (power, need_power, power_output): (f64, f64, f64),
) -> Result<(), ServiceError> {
    let over = |key: &str, load: f64, need: f64, total: f64| {
        user_error(
            key,
            vec![
                ("moduleType", type_arg_for(key, "moduleType", type_id)),
                ("remaining", PyValue::Real(total - load)),
                ("require", PyValue::Real(need)),
                ("total", PyValue::Real(total)),
            ],
        )
    };
    if need_cpu != 0.0 && cpu + need_cpu > cpu_output + 1e-06 {
        return Err(over("NotEnoughCpu", cpu, need_cpu, cpu_output));
    }
    if need_power != 0.0 && power + need_power > power_output + 1e-06 {
        return Err(over("NotEnoughPower", power, need_power, power_output));
    }
    Ok(())
}

/// A module has just been fitted to `ship_id`: decide whether it starts
/// online.
///
/// **The client onlines it itself** -- our `OnItemChange` reaches
/// `cdl.OnItemChange` -> `FitItem` -> `_OnlineModuleIfApplicable` ->
/// `dogmaIM.SetModuleOnline` (`cdl:705,751,768,855`) [V] -- so all the
/// server decides is the state that call meets. Online if the budget allows
/// (the call then gets `EffectAlreadyActive2`, which the client swallows
/// [V]); offline if not, so the call is refused with `NotEnoughCpu` /
/// `NotEnoughPower` and the client undoes its own optimistic online [V].
/// Nothing is pushed.
pub fn on_fitted(w: &mut Registry, ship_id: i64, ship_type_id: i64, module_id: i64) {
    let type_id = w.store().get(module_id).map_or(0, |i| i.type_id);
    // An unskilled module goes on and stays offline (SKILLGATE): the
    // client's own `OnlineModule` would refuse it.
    let owner = w.store().get(module_id).map_or(0, |i| i.owner_id);
    let fits = crate::skillgate::missing(w, owner, type_id).is_empty()
        && budget(w, ship_id, ship_type_id, module_id, type_id).is_ok();
    set_online_kept(w, module_id, fits);
}

/// **Offline what the pilot cannot use** (SKILLGATE): every online module of
/// `ship_id` whose skills `character` lacks goes offline and stops. The
/// client's embarkation does the same and swallows the refusal
/// (`_OnCharacterEmbarkation`) [V]. Run on boarding, and at login for worlds
/// saved before the gate. A module stays offline after the skill is trained
/// until it is put online [A: retail re-onlines it via `onlineByShip`].
pub fn offline_unskilled(w: &mut Registry, character: i64, ship_id: i64) {
    let now = crate::pyutil::filetime_now();
    for (_, id, type_id) in online_candidates(w, ship_id) {
        if w.modules().is_online(id) && !crate::skillgate::missing(w, character, type_id).is_empty()
        {
            set_online_kept(w, id, false);
            w.modules_mut().end(id, now);
        }
    }
}

/// The ship's modules that can be online, `(flag, item, type)` by slot.
fn online_candidates(w: &Registry, ship_id: i64) -> Vec<(i64, i64, i64)> {
    let mut candidates: Vec<(i64, i64, i64)> = w
        .store()
        .contents_of(ship_id)
        .filter(|i| i.singleton && (11..=34).contains(&i.flag_id))
        .filter(|i| {
            match crate::typedata::installed_in(w.content())
                .table()
                .get(i.type_id)
            {
                Some(row) => row.has_effect(EFFECT_ONLINE),
                None => i.category_id == crate::typedata::CATEGORY_MODULE,
            }
        })
        .map(|i| (i.flag_id, i.item_id, i.type_id))
        .collect();
    candidates.sort_unstable();
    candidates
}

/// The delta kind of a module taken offline: `module_id -> 1`
/// (`delta_kinds::KINDS`). The online state used to live only in
/// [`Modules`], so a restart put every offlined module back online: the
/// owner's offlined launcher showed lit on the module bar after the dev
/// world was restarted.
pub const OFFLINE_KIND: &str = "modoff";

/// [`Modules::set_online`], written through to the world file. `false` if
/// the module already was in that state.
pub fn set_online_kept(w: &mut Registry, module_id: i64, online: bool) -> bool {
    let changed = w.modules_mut().set_online(module_id, online);
    let key = vec![module_id.to_string()];
    if online {
        w.deltas_mut().del(OFFLINE_KIND, &key);
    } else {
        w.deltas_mut().put(OFFLINE_KIND, key, vec!["1".to_owned()]);
    }
    changed
}

/// **`ship.Undock(onlineModules={flag: itemID})`**: the client's own word on
/// which modules it holds online when it leaves the station. Its dogma
/// brings modules online itself and may drop one the server keeps online (an
/// unskilled CPU check), so a module the server holds online that the client
/// left out goes offline here, and the bar then agrees. The other way, a
/// listed module the server holds offline goes online only if the overheat
/// and the CPU/powergrid checks both pass: the client's word never beats the
/// budget. Demotions run first, so they free budget for the promotions, which
/// go in ascending slot order.
///
/// The ship is the actor's own, never `args[0]`. A call without the kwarg (a
/// bot) changes nothing. Nothing is pushed: the in-space `GetAllInfo` that
/// follows primes the bar from the server's state.
///
/// **[A]** the real server honours `onlineModules` at undock (EvEJS reads it
/// only to promote). Falsifier: a launcher the owner took offline in the
/// fitting window lights up again after undocking, or one the client left
/// out still fires.
///
/// **Not wired into `ship.Undock`** (2026-10-29): the bots send
/// `onlineModules={}`, as the client capture of a ship with nothing online
/// does, and honouring it took every bot's modules offline (the gate's
/// escort and missioner tests). The login brain alone fixes the owner's
/// case. Wiring it needs the bots to send their real online list first.
pub fn sync_undock_online(w: &mut Registry, actor: Option<i64>, kwargs: &[(PyValue, PyValue)]) {
    let Some(PyValue::Dict(listed)) = crate::space_state::kwarg(kwargs, "onlineModules") else {
        return;
    };
    // `{flag: itemID}`; a marshal dict holds `(value, key)`.
    let listed: BTreeSet<i64> = listed
        .iter()
        .filter_map(|(v, _)| match v {
            PyValue::Int(id) => Some(*id),
            _ => None,
        })
        .collect();
    let Some(c) = actor.and_then(|id| w.character(id)) else {
        return;
    };
    let (character, ship_id, ship_type_id) = (c.character_id, c.ship_id, c.ship_type_id);
    // The ship's modules that can be online, by slot.
    let candidates = online_candidates(w, ship_id);
    if candidates.is_empty() {
        return;
    }
    let now = crate::pyutil::filetime_now();
    crate::fitstate::around(w, actor, |w| {
        for &(_, id, _) in &candidates {
            if !listed.contains(&id) && w.modules().is_online(id) {
                set_online_kept(w, id, false);
                w.modules_mut().end(id, now);
            }
        }
        for &(_, id, type_id) in &candidates {
            if listed.contains(&id)
                && !w.modules().is_online(id)
                && crate::overheat::refuse_online(w, id).is_ok()
                && crate::skillgate::missing(w, character, type_id).is_empty()
                && budget(w, ship_id, ship_type_id, id, type_id).is_ok()
            {
                set_online_kept(w, id, true);
            }
        }
    });
}

/// After a world load: every module the file holds offline is offline
/// again. A row whose item is gone (unfitted, destroyed) is dropped.
pub fn restore_offline(w: &mut Registry) {
    let rows: Vec<Vec<String>> = w.deltas().of(OFFLINE_KIND).keys().cloned().collect();
    for key in rows {
        // Load never drops a row (the store's round-trip rule: every listed
        // kind comes out as it went in); a row for a module that is gone is
        // inert and goes when that id is next written.
        let Some(id) = key.first().and_then(|k| k.parse::<i64>().ok()) else {
            continue;
        };
        if w.store().get(id).is_some() {
            w.modules_mut().set_online(id, false);
        }
    }
}

/// `dogmaIM.SetModuleOnline(shipID, moduleID)` and
/// `TakeModuleOffline(shipID, moduleID)`, the fitting window's pair
/// (`clientDogmaLocation.OnlineModule` / `OfflineModule`) `[V]`. The ship id
/// is ours to know and is not trusted.
pub fn set_module_online(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    on: bool,
    now: i64,
) -> Result<PyValue, ServiceError> {
    advance(w, now, Via::Answer);
    let f = fitted(w, actor, module_id)?;
    set_online(w, f, on, now)
}

/// The fitted modules of this character's ship, as `shipInfo` entries.
///
/// **`(value, key)` pairs**, ready to extend the dict `GetAllInfo` already
/// builds around the hull -- marshal dict entries are inverted on the wire.
///
/// Each row is the `ItemGetInfo` shape `capsule_dogma_row` uses, and three of
/// its parts are load-bearing `[V]`:
///
/// - **`invItem.singleton` must be truthy** or `godma:1939` drops the module
///   from the HUD, silently (T13). A fitted module that is still a stack in
///   the store is therefore *left out* rather than dressed up: the row would
///   contradict what `invbroker` lists for the same item.
/// - **`attributes` must carry the duration attribute** (§0.1.3), or the
///   ramp never draws.
/// - **`activeEffects` is `{effectID: [env0..env6, startTime, duration,
///   repeat]}`** (`godma:1995-1999`), and it is how a module that is online --
///   or cycling -- when the client primes is shown as such. The client replays
///   each entry through `OnGodmaShipEffect` itself.
///
/// **Loaded charges are here too, as sublocations** (P2.3): keyed by the
/// `(shipID, flagID, typeID)` tuple and built by [`charge_info_in`].
/// `PrimeLocation` hands every row to `UpdateItem(row.invItem, row)`, which
/// takes the sublocation branch on the tuple id alone (`godma:1858`-) `[V]`;
/// the ship itself is primed first (its id is in `PrimeLocation`'s priority
/// list), and a sublocation whose ship is not yet known is skipped.
pub fn ship_info_entries(w: &Registry, c: &crate::model::Character) -> Vec<(PyValue, PyValue)> {
    let mut out: Vec<(PyValue, PyValue)> = w
        .store()
        .contents_of(c.ship_id)
        .filter(|i| i.singleton && crate::item::is_ship_fitting_flag(i.flag_id))
        .map(|i| (module_info(w, c, i), PyValue::Int(i.item_id)))
        .collect();
    out.extend(
        w.store()
            .contents_of(c.ship_id)
            .filter(|i| i.owner_id == c.character_id && crate::item::is_loaded_charge(i))
            .map(|i| {
                (
                    charge_info_in(w.content(), i),
                    charge_key(i.location_id, i.flag_id, i.type_id),
                )
            }),
    );
    out
}

/// What another pilot's client draws on this ship: its slim's `modules`
/// (`crate::destiny::SlimModule` has the client evidence).
///
/// **Exactly the modules [`ship_info_entries`] primes** -- fitted (a fitting
/// flag), assembled (`singleton`) -- **and no loaded charge**, which shares a
/// module's flag in the store but is a sublocation to the client. In flag
/// order, then id, so the same fit always reads the same; the client sorts by
/// slot itself (`turretSet.py:475`).
pub fn slim_modules(store: &crate::item::Store, ship_id: i64) -> Vec<crate::destiny::SlimModule> {
    let mut out: Vec<crate::destiny::SlimModule> = store
        .contents_of(ship_id)
        .filter(|i| {
            i.singleton
                && crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::rigs::is_rig_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
        })
        .map(|i| crate::destiny::SlimModule {
            item_id: i.item_id,
            type_id: i.type_id,
            flag_id: i.flag_id,
        })
        .collect();
    out.sort_by_key(|m| (m.flag_id, m.item_id));
    out
}

/// One fitted module's `ItemGetInfo` row -- what [`ship_info_entries`]
/// primes and what `dogmaIM.ItemGetInfo(itemID)` answers when godma meets a
/// module fitted after the prime (`godma:1949`, `docs/lanes/fitting.md` §6).
/// One builder for both, so the two cannot drift.
pub fn module_info(w: &Registry, c: &crate::model::Character, i: &crate::item::Item) -> PyValue {
    let m = w.modules();
    let t = find_in(w.content(), i.type_id);
    let online = m.is_online(i.item_id);
    // The **modified** value of each served attribute when `crate::dogma`
    // has a sheet, so the HUD's cycle ramp runs at the cycle the server
    // runs; the same attribute list either way.
    let sheet = crate::dogma::sheet(w, c.ship_id);
    let mut attributes: Vec<(PyValue, PyValue)> = t
        .map(|t| t.attributes)
        .or_else(|| crate::rigs::attributes_in(w.content(), i.type_id))
        .unwrap_or_default()
        .iter()
        .map(|(id, v)| {
            let v = sheet
                .as_ref()
                .and_then(|s| s.module(i.item_id, *id))
                .unwrap_or(*v);
            (PyValue::Real(v), PyValue::Int(*id))
        })
        .collect();
    // A subsystem (T3-D) has no `ModuleType`: its attributes are the
    // sheet's (the type's own, bonuses applied) and it is never online or
    // offline -- passive, so no `isOnline`.
    let subsystem = i.category_id == crate::subsystems::CATEGORY_SUBSYSTEM;
    if subsystem && let Some(of) = sheet.as_ref().and_then(|s| s.module_attributes(i.item_id)) {
        attributes = of
            .iter()
            .map(|(id, v)| (PyValue::Real(*v), PyValue::Int(*id)))
            .collect();
    }
    // What heat has done to it, in hit points (K-OVERHEAT).
    let damage = crate::overheat::module_damage(w, i.item_id);
    if damage > 0.0 {
        attributes.push((
            PyValue::Real(damage),
            PyValue::Int(crate::overheat::ATTR_DAMAGE),
        ));
    }
    // A stance mode (T3-E) is passive too: no `isOnline`, no online effect.
    let stance = i.group_id == crate::stance::GROUP_STANCE_MODE;
    let online = online && !stance;
    if !subsystem && !stance {
        attributes.push((
            PyValue::Real(if online { 1.0 } else { 0.0 }),
            PyValue::Int(ATTR_IS_ONLINE),
        ));
    }
    let line = |effect_id: i64, target: Option<i64>, duration: i64, repeat: i64| {
        PyValue::List(vec![
            PyValue::Int(i.item_id),
            PyValue::Int(c.character_id),
            PyValue::Int(c.ship_id),
            target.map_or(PyValue::None, PyValue::Int),
            PyValue::None,
            PyValue::List(Vec::new()),
            PyValue::Int(effect_id),
            PyValue::None,
            PyValue::Int(duration),
            PyValue::Int(repeat),
        ])
    };
    let mut active: Vec<(PyValue, PyValue)> = Vec::new();
    if online {
        active.push((
            line(EFFECT_ONLINE, None, -1, -1),
            PyValue::Int(EFFECT_ONLINE),
        ));
    }
    if let Some(r) = m.running(i.item_id).filter(|r| r.stop_at.is_none()) {
        active.push((
            line(r.effect.effect_id, r.target_id, r.period / MSEC, r.repeat),
            PyValue::Int(r.effect.effect_id),
        ));
    }
    // A module run hot when the client primes shows it so (K-OVERHEAT).
    active.extend(crate::overheat::overload_lines(w, c, i.item_id, i.type_id));
    crate::pyutil::keyval(vec![
        ("itemID", PyValue::Int(i.item_id)),
        ("invItem", w.item_row_value(i)),
        ("attributes", PyValue::Dict(attributes)),
        ("activeEffects", PyValue::Dict(active)),
        // `None` for both, as for the hull: no attribute on a module
        // is charged, so neither time is read for anything but a
        // fallback to the client's own clock.
        ("time", PyValue::None),
        ("wallclockTime", PyValue::None),
    ])
}

// ---------------------------------------------------------------------------
// Charges (P2.3)
// ---------------------------------------------------------------------------
//
// **What a loaded charge is, on each side.** In the store it is a stack of
// the charge type at `(ship, module's flag)` -- `crate::item::is_loaded_charge`
// -- so rounds are conserved and persisted like any other item, and a ship
// that dies takes them with it. To the client it is a *sublocation*,
// `(shipID, flagID, typeID)` with its count in attribute 805, and nothing
// else (`docs/re/modules.md` §6.1) `[V]`. `Registry::apply_items` never
// sends one as an item row; the three pushes above are how the client
// learns about it: [`PrimeCharge`] for a new one, [`ChargeQuantity`] for
// every count that moves, and [`ChargeLoading`] for the reload animation.

/// Does this module type take charges? Every gun with a `chargeGroup1`
/// (604) does `[V]` -- `cfg.IsChargeCompatible` keys the ammo menu on the
/// same groups -- and only those: the civilian guns carry their damage on
/// themselves and name no group. A group of `0` is no group: **[A]** that
/// the client's tables never carry one, and harmless if they do.
pub fn takes_charges(t: &ModuleType) -> bool {
    // MN-D: a survey launcher names its probes in chargeGroup2.
    crate::moon_scan::takes_charges(t)
}

/// Is this a launcher (M4)? `useMissiles` and a charge group: what it
/// launches is its charge's business (`crate::typedata::missile_in`; a
/// probe is `crate::scan`'s).
pub fn is_launcher(t: &ModuleType) -> bool {
    t.effect.is_some_and(|e| e.effect_id == EFFECT_USE_MISSILES) && takes_charges(t)
}

/// The charge loaded into `ship_id` at `flag_id`, if any.
pub fn loaded(
    store: &crate::item::Store,
    ship_id: i64,
    flag_id: i64,
) -> Option<&crate::item::Item> {
    store
        .contents_of(ship_id)
        .find(|i| i.flag_id == flag_id && crate::item::is_loaded_charge(i))
}

/// How many rounds of `charge_type_id` a `module_type_id` holds: the
/// module's `capacity` over the round's `volume`, the client's own
/// arithmetic including its epsilon (`smb GetChargeReloadInfo`:
/// `int(GetCapacity(module) / GetVolume(charge) - quantity + 1e-07)`) `[V]`.
/// A 125mm Gatling (0.5 m3) holds 200 rounds of S ammo (0.0025 m3); the
/// float quotient is 199.99999999999997, which is what the epsilon is for.
/// Both types from the content (`crate::item::find_in`).
pub fn rounds_capacity_in(content: &Content, module_type_id: i64, charge_type_id: i64) -> i64 {
    rounds_of(
        crate::item::find_in(content, module_type_id),
        crate::item::find_in(content, charge_type_id),
    )
}

/// How many rounds of charge type `c` a module of type `m` holds:
/// `capacity / volume`, as [`rounds_capacity_in`] counts them; 0 when
/// either type is unknown or the charge has no volume. Public for a caller
/// that looks its types up elsewhere (`eve-bot`'s no-content fallback).
pub fn rounds_of(m: Option<&crate::item::ItemType>, c: Option<&crate::item::ItemType>) -> i64 {
    match (m, c) {
        (Some(m), Some(c)) if c.volume > 0.0 => (m.capacity / c.volume + 1e-07) as i64,
        _ => 0,
    }
}

/// A loaded charge's `ItemGetInfo` row -- what `shipInfo` primes and what
/// [`PrimeCharge`] carries.
///
/// The shape is the module row's ([`module_info`]); what godma reads off a
/// sublocation row `[V]`: `itemID` (the tuple, which is what makes it a
/// sublocation, `godma:1858`-), `attributes`, which **must** hold 805 or the
/// row is refused with `QUANTITY MISSING` (`godma:1904`), `activeEffects`
/// (`RefreshItemEffects`), `time` and `wallclockTime`.
///
/// **[A]** `invItem` is `None`: `UpdateItem`'s sublocation branch builds its
/// own row from the key and never reads it, and `isSubLocation` is decided
/// by `infoz.itemID` when the item is `None` (`godma:1858`-). The charge's
/// own damage and size are served beside the count -- `[A]` that they are
/// shown anywhere; they are the client's own numbers either way.
pub fn charge_info_in(content: &Content, i: &crate::item::Item) -> PyValue {
    charge_row(i, crate::combat::charge_in(content, i.type_id))
}

fn charge_row(i: &crate::item::Item, charge: Option<&crate::combat::ChargeType>) -> PyValue {
    let mut attributes = vec![(PyValue::Int(i.quantity), PyValue::Int(attr::QUANTITY))];
    if let Some(c) = charge {
        for (id, v) in [
            (attr::EM_DAMAGE, c.damage.em),
            (attr::EXPLOSIVE_DAMAGE, c.damage.explosive),
            (attr::KINETIC_DAMAGE, c.damage.kinetic),
            (attr::THERMAL_DAMAGE, c.damage.thermal),
            (attr::CHARGE_SIZE, c.charge_size),
        ] {
            attributes.push((PyValue::Real(v), PyValue::Int(id)));
        }
    }
    crate::pyutil::keyval(vec![
        ("itemID", charge_key(i.location_id, i.flag_id, i.type_id)),
        ("invItem", PyValue::None),
        ("attributes", PyValue::Dict(attributes)),
        ("activeEffects", PyValue::Dict(Vec::new())),
        ("time", PyValue::None),
        ("wallclockTime", PyValue::None),
    ])
}

/// A plan for a loaded charge was refused by the store: our bug.
/// `shipState`, the 4-tuple `ship.Board` answers and `GetAllInfo` carries:
/// `(instanceCache, instanceFlagQuantityCache, wbData, heatStates)`,
/// unpacked unguarded by `_MakeShipActive` (`cdl:305`) [V].
///
/// Slots 0, 2 and 3 are `{}`, `None`, `{}` -- the evidence for each is on
/// `Registry::all_info`. **Slot 1 is the loaded charges**
/// ([`instance_flag_quantity_cache`]).
pub fn ship_state(store: &crate::item::Store, ship_id: i64) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Dict(vec![]),
        instance_flag_quantity_cache(store, ship_id),
        PyValue::None,
        PyValue::Dict(vec![]),
    ])
}

/// `instanceFlagQuantityCache`: **`{shipID: {flagID: row}}`**, one row per
/// loaded charge, and how the *fitting window's* dogma location learns what
/// is loaded when a ship becomes active (a board, a login).
///
/// Every read of it, all `[V]`:
///
/// - `_MakeShipActive` stores slot 1 (`cdl:305`), loads the ship, and
///   clears it again at the end (`ClearInstanceCache`, `cdl:351`) -- it
///   lives for the length of one activation;
/// - loading the ship (`locationDogmaItem.OnItemLoaded`, `:47`) calls
///   `LoadSublocations(shipID)`, which walks
///   `cache.get(shipID, {}).iteritems()` as `(flagID, sublocation)` and
///   loads `(sublocation[0], sublocation[1], sublocation[2])` as the
///   charge's key (`baseDogmaLocation.py:1579`-`:1580`) -- so positions 0-2
///   **are** `(shipID, flagID, typeID)`, the sublocation tuple godma keys on;
/// - that charge item's `DBLessDogmaItem.OnItemLoaded` sets attribute 805
///   from `GetQuantityFromCache(locationID, flagID)`
///   (`dblessDogmaItem.py:35`-`:36`), which is
///   `cache[locationID][flagID].quantity` (`baseDogmaLocation.py:310`) --
///   so the row **also** has a `quantity` attribute. A `KeyError` there is
///   swallowed (`:37`), which is why `{}` never raised: it just loaded no
///   charge.
///
/// A row both indexable and attribute-addressed is a `blue.DBRow`, the
/// type every item row we send already is, and which the client subscripts
/// positionally elsewhere (`undockingSvc.py:195`). **[A]** that this is the
/// type TQ sends, and the column names other than `quantity` (only the
/// positions are read). Nothing loaded is `{}`, the value that has always
/// worked: `LoadSublocations` reads it through `.get(shipID, {})`.
pub fn instance_flag_quantity_cache(store: &crate::item::Store, ship_id: i64) -> PyValue {
    use eve_marshal::{Column, DbRow, DbRowDescriptor, DbType, DbValue};
    let charges: Vec<&crate::item::Item> = store
        .contents_of(ship_id)
        .filter(|i| crate::item::is_loaded_charge(i))
        .collect();
    if charges.is_empty() {
        return PyValue::Dict(vec![]);
    }
    let columns: Vec<Column> = [
        ("locationID", DbType::I8),
        ("flagID", DbType::I2),
        ("typeID", DbType::I4),
        ("quantity", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor =
        DbRowDescriptor::new(columns).expect("the sublocation columns are a valid layout");
    // A marshal dict entry is `(value, key)`.
    let by_flag = charges
        .iter()
        .map(|i| {
            let row = DbRow::new(
                descriptor.clone(),
                vec![
                    DbValue::Int(i.location_id),
                    DbValue::Int(i.flag_id),
                    DbValue::Int(i.type_id),
                    DbValue::Int(i.quantity),
                ],
            )
            .expect("four values for four columns");
            (PyValue::DbRow(Box::new(row)), PyValue::Int(i.flag_id))
        })
        .collect();
    PyValue::Dict(vec![(PyValue::Dict(by_flag), PyValue::Int(ship_id))])
}

fn charge_plan_refused(e: crate::item::EditError) -> ServiceError {
    crate::outln!("[modules] charge plan refused: {e}");
    notify("The server could not move those charges.")
}

/// What [`fire_rounds`] did for one cycle.
#[derive(Debug, Default)]
struct Fire {
    /// The charge that fired; `None` for a module that takes none, or an
    /// ancillary repairer running empty.
    charge_type_id: Option<i64>,
    /// Rounds taken out of the module.
    taken: i64,
    /// `chargeRate` (at least 1), `0` for a module that takes no charges.
    rate: i64,
    pushes: Vec<ModulePush>,
}

/// Remember a cycle's fire for `crate::remote_rep` ([`Fired`]).
fn note_fire(w: &mut Registry, module_id: i64, index: i64, fire: &Fire) {
    if fire.charge_type_id.is_some() || fire.rate > 0 {
        w.modules_mut().note_fired(
            module_id,
            Fired {
                index,
                charge_type_id: fire.charge_type_id,
                taken: fire.taken,
                rate: fire.rate.max(1),
            },
        );
    }
}

/// Take `rate` rounds out of the module for the cycle starting at `at`.
///
/// `Ok(fire)` with a charge -- the charge fired, its count pushed; with none
/// -- the module takes no charges and cycles as before, or is an ancillary
/// repairer running empty (`taken` 0); `Err(())` -- it takes charges and has
/// fewer than one cycle's worth, so the cycle cannot start. **An ancillary
/// repairer takes `min(rate, loaded)`** and never refuses
/// ([`is_ancillary`]).
///
/// The edit is applied with `Registry::apply_items_quiet`: a round fired is
/// not an inventory event the client wants as one (no `OnItemChange`, no
/// location invalidation every three seconds per gun), and the count it does
/// want is the [`ChargeQuantity`] returned here.
fn fire_rounds(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    at: i64,
) -> Result<Fire, ()> {
    let Some(item) = w.item(module_id).cloned() else {
        return Err(());
    };
    let Some(t) = module_type(w, item.type_id) else {
        return Ok(Fire::default());
    };
    if !takes_charges(&t) {
        return Ok(Fire::default());
    }
    let ancillary = t.effect.is_some_and(|e| is_ancillary(e.effect_id));
    // `chargeRate`, rounds per cycle [V]. A gun that takes charges and does
    // not name a rate is a crystal user (the laser): **[A]** a crystal is
    // not used up -- it degrades, which is not modelled.
    let rate = t
        .attribute(attr::CHARGE_RATE)
        .map_or(0, |r| r.round() as i64);
    let Some(charge) = loaded(w.store(), ship_id, item.flag_id).cloned() else {
        return if ancillary {
            Ok(Fire {
                rate: rate.max(1),
                ..Fire::default()
            })
        } else if runs_empty(&t) {
            Ok(Fire::default())
        } else {
            Err(())
        };
    };
    if charge.quantity < rate.max(1) && !ancillary {
        return Err(());
    }
    if rate == 0 {
        return Ok(Fire {
            charge_type_id: Some(charge.type_id),
            ..Fire::default()
        });
    }
    let take = rate.min(charge.quantity);
    w.apply_items_quiet(&[crate::item::Edit::Consume {
        item_id: charge.item_id,
        quantity: take,
    }])
    .map_err(|_| ())?;
    let left = charge.quantity - take;
    let mut out = vec![ModulePush::Quantity(ChargeQuantity {
        to: vec![character_id],
        owner_id: character_id,
        ship_id,
        flag_id: item.flag_id,
        type_id: charge.type_id,
        time: at,
        new_value: left,
        old_value: charge.quantity,
    })];
    if left < rate {
        // The last cycle's worth: the HUD button is rebuilt without its
        // charge ([`ModuleRefit`]).
        out.push(ModulePush::Refit(ModuleRefit {
            to: vec![character_id],
            row: w.item_row_value(&item),
            flag_id: item.flag_id,
        }));
    }
    Ok(Fire {
        charge_type_id: Some(charge.type_id),
        taken: take,
        rate,
        pushes: out,
    })
}

/// **An ancillary repairer switched off empty reloads** (the client's module
/// text, loc 286330 and 277430 `[V]`): with nothing loaded and charges in
/// the ship's cargo, the first type that fits goes in through [`load_one`],
/// and in space that starts the reload timer as [`load_ammo`] does. A running
/// module cannot be loaded, so this runs when the stop lands, not when the
/// player clicks.
fn reload_ancillary(w: &mut Registry, r: &Running, at: i64) {
    if !is_ancillary(r.effect.effect_id) {
        return;
    }
    let Some(flag_id) = w.item(r.module_id).map(|i| i.flag_id) else {
        return;
    };
    if loaded(w.store(), r.ship_id, flag_id).is_some() {
        return;
    }
    let Ok(f) = fitted(w, Some(r.character_id), r.module_id) else {
        return;
    };
    let mut by_type: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for i in w.store().contents_of(r.ship_id) {
        if i.flag_id == crate::fitting::FLAG_CARGO
            && i.category_id == crate::item::CATEGORY_CHARGE
            && i.owner_id == r.character_id
        {
            by_type.entry(i.type_id).or_default().push(i.item_id);
        }
    }
    for ids in by_type.values() {
        if let Ok(Some(charge_type_id)) = load_one(w, f, ids, r.ship_id, at) {
            start_reload(
                w,
                r.character_id,
                &[r.module_id],
                charge_type_id,
                reload_ms_of(w, f.type_id),
                at,
            );
            return;
        }
    }
}

/// Where loaded rounds may come from for this pilot, and where returned
/// ones go: the station hangar they are docked at, or their ship's cargo.
///
/// `docked` is `Registry::docked_station`, **not** `Character::station_id`:
/// that is "my station" and stays set in space, so the hangar used to feed
/// a gun in a belt and take back what it unloaded (`docs/lanes/docked.md`).
fn charge_home(
    c: &crate::model::Character,
    docked: Option<i64>,
    location_id: i64,
) -> Option<crate::item::Slot> {
    if docked == Some(location_id) {
        Some(crate::item::Slot::new(
            c.character_id,
            location_id,
            crate::model::FLAG_HANGAR,
        ))
    } else if location_id == c.ship_id {
        Some(crate::item::Slot::new(
            c.character_id,
            location_id,
            crate::fitting::FLAG_CARGO,
        ))
    } else {
        None
    }
}

/// `dogmaIM.LoadAmmo(shipID, moduleID(s), chargeItemIDs, chargeLocationID)`
/// -- four positional `[V]`, and **the reply is discarded** (`cdl:1201`,
/// `cdl:1206`, both `POP_TOP`), so it is `None`.
///
/// Every client route ends here: drag ammo onto the HUD button
/// (`smb:1143 OnDropData` -> `cdl:1209 DropLoadChargeToModule`) or onto the
/// fitting window's slot (`fittingSlotController FitCharges`), the
/// right-click ammo menu and auto-reload (`smb:840 AutoReload` ->
/// `cdl:1175 LoadAmmoTypeToModule`, cargo only) all call
/// `cdl:1187 LoadChargesToModule` with **one** module id; "load to all" is
/// `cdl:1204 LoadAmmoToModules` with several.
///
/// Per module, one plan: the rounds come off the named stacks (in order)
/// until the module is full ([`rounds_capacity_in`]); a different type already
/// loaded goes back to where the new rounds came from, first. Then the
/// client is told in the sublocation's shape (header of this section).
///
/// **[A]** in station a load is instant; in space it is a reload: the
/// rounds are in at once, `OnChargeBeingLoadedToModule` plays the
/// animation, and the module refuses to activate until `reloadTime` (1795)
/// has passed (`CantActivateReloadingModule`). `nextActivationTime` (2729)
/// is still not served -- the trap in `docs/re/modules.md` §6.3 is a
/// module left unclickable for ever by a time on the wrong clock.
pub fn load_ammo(
    w: &mut Registry,
    actor: Option<i64>,
    module_ids: &[i64],
    charge_ids: &[i64],
    charge_location_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    advance(w, now, Via::Answer);
    let mut loaded_type = None;
    let mut reload_ms = 0;
    let mut reloaded = Vec::new();
    for &module_id in module_ids {
        let f = fitted(w, actor, module_id)?;
        let Some(charge_type) = load_one(w, f, charge_ids, charge_location_id, now)? else {
            continue;
        };
        loaded_type = Some(charge_type);
        reloaded.push(module_id);
        reload_ms = reload_ms.max(
            find_in(w.content(), f.type_id)
                .and_then(|t| t.attribute(attr::RELOAD_TIME))
                .map_or(0, |ms| ms.round() as i64),
        );
    }
    if let (Some(charge_type_id), Some(character_id)) = (loaded_type, actor) {
        start_reload(w, character_id, &reloaded, charge_type_id, reload_ms, now);
    }
    Ok(PyValue::None)
}

/// A module type's `reloadTime` (1795), milliseconds.
fn reload_ms_of(w: &Registry, module_type_id: i64) -> i64 {
    find_in(w.content(), module_type_id)
        .and_then(|t| t.attribute(attr::RELOAD_TIME))
        .map_or(0, |ms| ms.round() as i64)
}

/// In space, rounds just loaded are a reload: the modules refuse to
/// activate until `reload_ms` has passed, and the client plays the
/// animation. In a station a load is instant.
fn start_reload(
    w: &mut Registry,
    character_id: i64,
    reloaded: &[i64],
    charge_type_id: i64,
    reload_ms: i64,
    now: i64,
) {
    let in_space = w
        .roster()
        .get(character_id)
        .zip(w.character(character_id))
        .is_some_and(|(p, c)| p.ship_id == c.ship_id);
    if !in_space || reload_ms <= 0 {
        return;
    }
    let until = now + reload_ms * MSEC;
    for m in reloaded {
        w.modules_mut().reloading.insert(*m, until);
    }
    w.push(ModulePush::Loading(ChargeLoading {
        to: vec![character_id],
        module_ids: reloaded.to_vec(),
        charge_type_id,
        reload_ms,
    }));
}

/// Load one module. `Ok(Some(type))` if rounds went in, `Ok(None)` if it
/// was already full of that type.
fn load_one(
    w: &mut Registry,
    f: Fitted,
    charge_ids: &[i64],
    charge_location_id: i64,
    now: i64,
) -> Result<Option<i64>, ServiceError> {
    let Some(c) = w.character(f.character_id).cloned() else {
        return Err(notify("No character is selected."));
    };
    // Any named charge group, not `takes_charges`: an ancillary booster names
    // its paste or cap boosters in chargeGroup2 (owner: a Capital Ancillary
    // Shield Booster could not be loaded).
    let t = module_type(w, f.type_id)
        .filter(crate::moon_scan::can_load)
        .ok_or_else(|| notify("That module takes no charges."))?;
    if w.modules().running(f.module_id).is_some() {
        // **[A]** CCP's key for "that charge is in use"; no argument.
        return Err(user_error("CannotAccessChargeWhileInUse", vec![]));
    }
    let home = charge_home(&c, w.docked_station(c.character_id), charge_location_id)
        .ok_or_else(|| notify("Charges can only be loaded from your hangar or your cargo."))?;
    let flag_id = w.item(f.module_id).map_or(0, |i| i.flag_id);
    // The stacks the call names that are really there: mine, at `home`, of
    // one type (the first one's), not themselves loaded.
    let mut sources: Vec<crate::item::Item> = Vec::new();
    for id in charge_ids {
        let Some(i) = w.item(*id) else { continue };
        if i.owner_id != c.character_id
            || i.location_id != home.location_id
            || i.flag_id != home.flag_id
            || i.category_id != crate::item::CATEGORY_CHARGE
            || sources.first().is_some_and(|s| s.type_id != i.type_id)
            || sources.iter().any(|s| s.item_id == i.item_id)
        {
            continue;
        }
        sources.push(i.clone());
    }
    let Some(first) = sources.first() else {
        // The client's own key for an empty selection (`cdl:1193`) [V].
        return Err(user_error("CannotLoadNotEnoughCharges", vec![]));
    };
    let charge_type = first.type_id;
    let charge_group = first.group_id;
    // **A module with no `chargeSize` takes its group by group alone**: a
    // scan probe launcher has none, and neither has a probe [V-data]
    // (`tools/dogma.py type 17938`, `30028`), so the size test below could
    // never pass for one (P5.10, `crate::scan`).
    let fits = crate::moon_scan::accepts_group(&t, charge_group)
        && match t.attribute(attr::CHARGE_SIZE) {
            None => true,
            // A charge that does no damage (paste, cap boosters) is not in
            // the weapon table: read its own `chargeSize` instead, or it
            // could never be loaded.
            Some(size) => crate::combat::charge_in(w.content(), charge_type)
                .map(|ct| ct.charge_size)
                .or_else(|| {
                    crate::typedata::attribute_in(w.content(), charge_type, attr::CHARGE_SIZE)
                })
                .is_some_and(|have| size == have),
        };
    if !fits {
        return Err(notify("That charge does not fit that module."));
    }
    let capacity = rounds_capacity_in(w.content(), f.type_id, charge_type);
    let current = loaded(w.store(), f.ship_id, flag_id).cloned();
    let same = current.as_ref().filter(|i| i.type_id == charge_type);
    let room = capacity - same.map_or(0, |i| i.quantity);
    if room <= 0 {
        return Ok(None);
    }
    let mut plan = Vec::new();
    let mut take = 0;
    for s in &sources {
        if take == room {
            break;
        }
        let n = s.quantity.min(room - take);
        plan.push(crate::item::Edit::Consume {
            item_id: s.item_id,
            quantity: n,
        });
        take += n;
    }
    let mut out = Vec::new();
    if let Some(old) = current.as_ref().filter(|i| i.type_id != charge_type)
        && home.flag_id == crate::fitting::FLAG_CARGO
    {
        // The rounds coming back must fit in the hold the new ones leave --
        // **the hold is never over capacity** (P5.12). The `take` new rounds
        // free their room first.
        let vol = |t: i64| crate::item::find_in(w.content(), t).map_or(0.0, |x| x.volume);
        let free =
            crate::fitting::cargo_free_in(w, home.location_id) + vol(charge_type) * take as f64;
        if crate::fitting::units_that_fit(free, vol(old.type_id)) < old.quantity {
            return Err(crate::fitting::Refusal::NoRoom {
                available: free,
                volume: vol(old.type_id) * old.quantity as f64,
            }
            .to_error());
        }
    }
    if let Some(old) = current.as_ref().filter(|i| i.type_id != charge_type) {
        // **[A]** the other type goes back where the new rounds came from.
        plan.push(crate::item::Edit::Destroy {
            item_id: old.item_id,
        });
        plan.push(crate::item::Edit::Create {
            type_id: old.type_id,
            quantity: old.quantity,
            to: home,
        });
        out.push(ModulePush::Quantity(ChargeQuantity {
            to: vec![c.character_id],
            owner_id: c.character_id,
            ship_id: f.ship_id,
            flag_id,
            type_id: old.type_id,
            time: now,
            new_value: 0,
            old_value: old.quantity,
        }));
    }
    match same {
        Some(s) => plan.push(crate::item::Edit::Grow {
            item_id: s.item_id,
            quantity: take,
        }),
        None => plan.push(crate::item::Edit::Create {
            type_id: charge_type,
            quantity: take,
            to: crate::item::Slot::new(c.character_id, f.ship_id, flag_id),
        }),
    }
    w.apply_items(&plan).map_err(charge_plan_refused)?;
    let was = same.map_or(0, |s| s.quantity);
    if same.is_none()
        && let Some(new) = loaded(w.store(), f.ship_id, flag_id)
    {
        out.push(ModulePush::Prime(PrimeCharge {
            to: vec![c.character_id],
            ship_id: f.ship_id,
            row: charge_info_in(w.content(), new),
        }));
    }
    // For godma a prime is the whole news and this is a redundant update it
    // logs and drops (`godma:2948`, `oldValue == newValue`); for the fitting
    // window it is the only news -- `cdl:391` fits a key it has not seen.
    out.push(ModulePush::Quantity(ChargeQuantity {
        to: vec![c.character_id],
        owner_id: c.character_id,
        ship_id: f.ship_id,
        flag_id,
        type_id: charge_type,
        time: now,
        new_value: was + take,
        old_value: was,
    }));
    send(w, Via::Answer, out);
    Ok(Some(charge_type))
}

/// Where `UnloadAmmo`'s `destination` names: a container id, or
/// `(locationID, ownerID, flag)` (`cdl:1328`, `fittingSlotController
/// UnfitCharge`) `[V]`. Only the hangar the pilot is docked at and their
/// own ship's cargo are places this server keeps charges.
fn unload_target(
    c: &crate::model::Character,
    docked: Option<i64>,
    destination: &PyValue,
) -> Option<crate::item::Slot> {
    let location_id = match destination {
        PyValue::Int(id) => *id,
        PyValue::Tuple(parts) | PyValue::List(parts) => match parts.first() {
            Some(PyValue::Int(id)) => *id,
            _ => return None,
        },
        _ => return None,
    };
    charge_home(c, docked, location_id)
}

/// `dogmaIM.UnloadAmmo(shipID, moduleID(s), destination[, quantity])` --
/// three positional from the fitting window and the module menu
/// (`cdl:1328`, `:1330`, `:1377`), four from a drag of the charge itself
/// (`cdl:1366`, `UnloadAmmoToContainer`) `[V]`.
///
/// Answers **the ids of the stacks the rounds landed in**, a list: one
/// caller reads `itemID[0]` as the unloaded stack before loading it into
/// another slot (`fittingSlotController FitCharges`), the others discard it
/// or test it for truth (`findings/client-calls.tsv`). **[A]** a fresh
/// stack per module, never merged into one already there.
///
/// A module with nothing loaded is skipped rather than refused (a weapon
/// bank unloads every member, loaded or not). Into the cargo hold, rounds
/// that do not fit are refused with `NotEnoughCargoSpace` (P5.12) -- an
/// earlier member of a bank may already have unloaded.
pub fn unload_ammo(
    w: &mut Registry,
    actor: Option<i64>,
    module_ids: &[i64],
    destination: &PyValue,
    quantity: Option<i64>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    advance(w, now, Via::Answer);
    let mut landed = Vec::new();
    for &module_id in module_ids {
        let f = fitted(w, actor, module_id)?;
        let Some(c) = w.character(f.character_id).cloned() else {
            return Err(notify("No character is selected."));
        };
        let to = unload_target(&c, w.docked_station(c.character_id), destination)
            .ok_or_else(|| notify("Charges can only be unloaded to your hangar or your cargo."))?;
        if w.modules().running(module_id).is_some() {
            return Err(user_error("CannotAccessChargeWhileInUse", vec![]));
        }
        let flag_id = w.item(module_id).map_or(0, |i| i.flag_id);
        let Some(charge) = loaded(w.store(), f.ship_id, flag_id).cloned() else {
            continue;
        };
        let n = quantity
            .filter(|q| *q > 0)
            .map_or(charge.quantity, |q| q.min(charge.quantity));
        if to.flag_id == crate::fitting::FLAG_CARGO {
            // **The hold is never over capacity** (P5.12). The client
            // catches this key in `UnloadAmmoToContainer`
            // (`clientDogmaLocation.py:1369`) [V].
            crate::fitting::check_cargo_room_in(
                w.content(),
                w.store(),
                to.location_id,
                charge.type_id,
                n,
            )
            .map_err(|r| r.to_error())?;
        }
        let changes = w
            .apply_items(&[
                crate::item::Edit::Consume {
                    item_id: charge.item_id,
                    quantity: n,
                },
                crate::item::Edit::Create {
                    type_id: charge.type_id,
                    quantity: n,
                    to,
                },
            ])
            .map_err(charge_plan_refused)?;
        landed.extend(changes.iter().filter_map(|ch| match ch {
            crate::item::Change::Created { item_id, .. } => Some(PyValue::Int(*item_id)),
            _ => None,
        }));
        w.push(ModulePush::Quantity(ChargeQuantity {
            to: vec![c.character_id],
            owner_id: c.character_id,
            ship_id: f.ship_id,
            flag_id,
            type_id: charge.type_id,
            time: now,
            new_value: charge.quantity - n,
            old_value: charge.quantity,
        }));
    }
    Ok(PyValue::List(landed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented message table (`loc_args` cells in the tsv's form): the
    /// arg is a plain typeID exactly where the text reads `[item]arg`, and
    /// the `(UE_TYPEID, id)` pair everywhere else.
    #[test]
    fn the_arg_form_follows_the_message_text() {
        let table: &[(&str, &str, bool)] = &[
            ("[item]item.name,[numeric]limit", "item", true),
            ("[item]item.name,[numeric]limit", "limit", false),
            (
                "[numeric]distance,[item] gear.nameWithArticle",
                "gear",
                true,
            ),
            ("[item]a.name,[item]b.name", "b", true),
            ("moduleName", "moduleName", false),
            ("item,ship", "ship", false),
            ("-", "item", false),
        ];
        for (spec, arg, plain) in table {
            assert_eq!(spec_wants_plain_item(spec, arg), *plain, "{spec} / {arg}");
        }
    }

    /// `type_arg_for` follows the rule for the keys it lists, and every
    /// listed pair is unique.
    #[test]
    fn type_arg_for_sends_a_plain_id_only_for_listed_item_args() {
        assert_eq!(
            type_arg_for("NoDroneManagementAbilitiesLeft", "item", 990_001),
            PyValue::Int(990_001)
        );
        assert_eq!(
            type_arg_for("EffectAlreadyActive2", "modulename", 990_001),
            type_arg(990_001)
        );
        assert_eq!(
            type_arg_for("NoDroneManagementAbilitiesLeft", "limit", 990_001),
            type_arg(990_001)
        );
        for (i, p) in PLAIN_ITEM_ARGS.iter().enumerate() {
            assert!(!PLAIN_ITEM_ARGS[..i].contains(p), "{p:?} listed twice");
        }
    }

    #[test]
    fn a_type_with_disallow_repeating_runs_one_cycle() {
        let base = module(483);
        let once = ModuleType {
            attributes: &[(ATTR_DISALLOW_REPEATING, 1.0)],
            ..base
        };
        let free = ModuleType {
            attributes: &[(ATTR_DISALLOW_REPEATING, 0.0)],
            ..base
        };
        assert_eq!(repeat_allowed(&once, 1000), 1);
        assert_eq!(repeat_allowed(&free, 1000), 1000);
        assert_eq!(
            repeat_allowed(
                &ModuleType {
                    attributes: &[],
                    ..base
                },
                7
            ),
            7
        );
    }

    /// The fixture content's row (`FIXTURE`: invented numbers, O3).
    fn module(type_id: i64) -> ModuleType {
        *find_in(&Content::fixture(), type_id).expect("a fixture module")
    }

    fn effect(start: bool) -> ShipEffect {
        ShipEffect {
            to: vec![7],
            module_id: 100_000_010,
            effect_id: 34,
            character_id: 7,
            ship_id: 100_000_001,
            target_id: Some(100_000_002),
            t: 1,
            start,
            duration_ms: 3750,
            repeat: 1000,
            error: None,
        }
    }

    /// `OnGodmaShipEffect`'s field order and its sentinels `[V]` (§2.2,
    /// `godma:1722`; template `godma:3072`).
    #[test]
    fn the_confirmation_has_the_handlers_field_order() {
        let (event, args) = effect(true).to_push();
        assert_eq!(event, "OnGodmaShipEffect");
        assert_eq!(
            args.len(),
            11,
            "eleven positional, `stall` defaulted, as godma:3072"
        );
        assert_eq!(args[0], PyValue::Int(100_000_010), "itemID is the module");
        assert_eq!(args[1], PyValue::Int(34), "effectID");
        assert_eq!(args[3], PyValue::Int(1), "start");
        assert_eq!(args[4], PyValue::Int(1), "active");
        assert_eq!(
            args[6],
            PyValue::None,
            "startTime None anchors on the client's clock"
        );
        assert_eq!(args[7], PyValue::Int(3750), "duration in ms");
        assert_eq!(args[8], PyValue::Int(1000), "repeat echoed");
        assert_eq!(args[9], PyValue::None, "no error");
        assert_eq!(args[10], PyValue::None, "no actualStopTime");

        let (_, stop) = effect(false).to_push();
        assert_eq!(
            stop[3],
            PyValue::Int(0),
            "start=0 is what SetIdle switches on"
        );
        assert_eq!(stop[4], PyValue::Int(0));
    }

    /// The environment is a seven-element list; index 2 is the ship, which
    /// gates the HUD (`godma:1745`), and index 3 the target (`godma:1739`)
    /// `[V]`.
    #[test]
    fn the_environment_names_module_pilot_ship_target_and_effect() {
        let PyValue::List(env) = effect(true).environment() else {
            panic!("a list, as godma:3072 builds it");
        };
        assert_eq!(env.len(), 7);
        assert_eq!(env[0], PyValue::Int(100_000_010));
        assert_eq!(env[1], PyValue::Int(7));
        assert_eq!(env[2], PyValue::Int(100_000_001));
        assert_eq!(env[3], PyValue::Int(100_000_002));
        assert_eq!(env[4], PyValue::None);
        assert_eq!(env[5], PyValue::List(Vec::new()));
        assert_eq!(env[6], PyValue::Int(34));
    }

    /// `OnSpecialFX` sends all fourteen fields, in the order seven sources
    /// agree on `[V]` (§5.2).
    #[test]
    fn the_space_visual_has_fourteen_fields() {
        let (event, args) = SpecialFx {
            to: vec![7, 8],
            ship_id: 100_000_001,
            module_id: 100_000_010,
            type_id: 486,
            target_id: Some(100_000_002),
            guid: "effects.ProjectileFired",
            is_offensive: true,
            start: true,
            duration_ms: 3750,
            repeat: 1000,
        }
        .to_push();
        assert_eq!(event, "OnSpecialFX");
        assert_eq!(args.len(), 14);
        assert_eq!(args[0], PyValue::Int(100_000_001), "shipID");
        assert_eq!(args[1], PyValue::Int(100_000_010), "moduleID");
        assert_eq!(args[2], PyValue::Int(486), "moduleTypeID");
        assert_eq!(args[3], PyValue::Int(100_000_002), "targetID");
        assert_eq!(args[5], PyValue::Bytes(b"effects.ProjectileFired".to_vec()));
        assert_eq!(args[7], PyValue::Int(1), "start");
        assert_eq!(args[9], PyValue::Int(3750), "duration");
        assert_eq!(args[11], PyValue::None, "startTime");
        assert_eq!(args[12], PyValue::Int(0), "timeFromStart");
    }

    /// A capacitor push is owned by the character and keyed by the ship, on
    /// attribute 18 -- `godma:1591` drops it otherwise `[V]` (§4.3).
    #[test]
    fn a_capacitor_change_is_the_characters_on_the_ship() {
        let (event, args) = AttributeChange {
            to: vec![7],
            owner_id: 7,
            item_id: 100_000_001,
            attribute_id: ATTR_CHARGE,
            time: 5,
            new_value: 230.0,
            old_value: 250.0,
        }
        .to_push();
        assert_eq!(event, "OnModuleAttributeChange");
        assert_eq!(args.len(), 7, "seven positional, scatterAttr defaulted");
        assert_eq!(args[0], PyValue::Int(7), "ownerID is the character");
        assert_eq!(args[1], PyValue::Int(100_000_001), "itemKey is the ship");
        assert_eq!(args[2], PyValue::Int(18));
        assert_eq!(args[3], PyValue::Int(5), "the change's own time");
        assert_eq!(args[4], PyValue::Real(230.0));
        assert_eq!(args[5], PyValue::Real(250.0));
    }

    /// T8's numbers `[V]`: 125 of 250 with `rechargeRate` 125000 ms reaches
    /// 75% at 19.6 s and 90% at 43.5 s, on the client's formula.
    #[test]
    fn the_capacitor_recharges_on_the_clients_curve() {
        let at = 1_000_000_000;
        let after = |ms: i64| recharged(125.0, at, at + ms * MSEC, 250.0, 125_000.0);
        assert!((after(19_555) / 250.0 - 0.75).abs() < 1e-3);
        assert!((after(43_545) / 250.0 - 0.90).abs() < 1e-3);
        assert!((after(0) - 125.0).abs() < 1e-9, "no time, no recharge");
        assert!(
            (recharged(250.0, at, at + 10 * 60_000 * MSEC, 250.0, 125_000.0) - 250.0).abs() < 1e-9,
            "full is the fixed point"
        );
        assert!(after(1_000) > 125.0, "it rises, not falls");
    }

    /// The worked examples, resolved the way `GetEffectTiming` resolves
    /// them `[V]` (§0.3): the effect row (id, name, category, guid) and the
    /// **value of its `durationAttributeID`** as the cycle -- `speed` for a
    /// turret, `duration` for a miner -- with `capacitorNeed` as the cost.
    #[test]
    fn the_worked_examples_resolve_to_the_spec_numbers() {
        let ac = module(486);
        let e = ac.effect.expect("an activatable effect");
        assert_eq!(
            (e.effect_id, e.name, e.category),
            (34, "projectileFired", 2)
        );
        assert_eq!(e.guid, "effects.ProjectileFired");
        assert_eq!(e.duration_attribute, attr::SPEED);
        assert_eq!(Some(ac.duration_ms() as f64), ac.attribute(attr::SPEED));
        assert_eq!(ac.capacitor_need(), 0.0, "a projectile turret needs none");

        let miner = module(483);
        let e = miner.effect.expect("a miner");
        assert_eq!((e.effect_id, e.duration_attribute), (67, attr::DURATION));
        assert_eq!(
            Some(miner.duration_ms() as f64),
            miner.attribute(attr::DURATION)
        );
        assert_eq!(
            Some(miner.capacitor_need()),
            miner.attribute(attr::CAPACITOR_NEED)
        );

        let beam = module(454);
        assert_eq!(Some(beam.duration_ms() as f64), beam.attribute(attr::SPEED));
        assert!(beam.capacitor_need() > 0.0, "a laser pays capacitor");
    }

    /// No row serves `nextActivationTime` (2729): a future value makes the
    /// module unclickable, silently (§6.3, T14) `[V]`. And every activatable
    /// row serves its own duration attribute (§0.1.3) `[V]`.
    #[test]
    fn every_row_serves_its_duration_and_none_serves_next_activation() {
        for t in FIXTURE {
            assert!(t.attribute(2729).is_none(), "{} serves 2729", t.name);
            if let Some(e) = t.effect {
                assert!(
                    t.attribute(e.duration_attribute).is_some_and(|v| v > 0.0),
                    "{} has no duration",
                    t.name
                );
            }
        }
        assert!(
            FIXTURE.windows(2).all(|p| p[0].type_id < p[1].type_id),
            "sorted, for the binary search"
        );
    }

    /// Every module `/createitem` can grant is in the fixture, so none of
    /// them is a module a server with no bundle cannot prime.
    #[test]
    fn every_spawnable_module_is_known() {
        let c = Content::fixture();
        for t in crate::item::SPAWNABLE.iter().filter(|t| t.category_id == 7) {
            assert!(
                find_in(&c, t.type_id).is_some(),
                "{} ({})",
                t.name,
                t.type_id
            );
        }
    }

    /// With a bundle the fixture never answers: a type the bundle's table
    /// lacks is a module this server cannot cycle (ruling 3).
    #[test]
    fn with_a_bundle_only_the_bundle_answers() {
        let c = Content::fixture();
        assert!(!c.is_bundle());
        assert!(find_in(&c, 483).is_some(), "the fixture, with no bundle");
        assert!(
            find_in(&c, 990_000_002).is_some(),
            "the content's own type table answers first"
        );
        assert!(find_in(&c, 1).is_none());
    }

    // ---- against a world ----------------------------------------------

    const ALICE: i64 = 1_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const GUN: i64 = 100_000_050;
    /// `flagHiSlot0` `[V]` (`inventorycommon/const.pyc:122-145`).
    const HI_SLOT_0: i64 = 27;

    /// Alice in space in her ship, with a 200mm AutoCannon I fitted to high
    /// slot 0, as a singleton.
    ///
    /// **The ship is a Rifter (587), not the seeded capsule**: onlining now
    /// checks the hull's `cpuOutput` (`online.py:56`) [V], and a capsule's is
    /// 1 -- a gun on a pod was a fixture only a server without that check
    /// could hold.
    fn alice_with_a_gun() -> Registry {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ship_type_id: 587,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        });
        let rifter = *crate::item::find_in(reg.content(), 587).expect("Rifter is spawnable");
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = rifter.type_id;
            i.group_id = rifter.group_id;
        }
        items.push(crate::item::Item {
            item_id: GUN,
            type_id: 486,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT_0,
            quantity: 1,
            group_id: 55,
            category_id: 7,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: GUN + 1,
            ..crate::store::Snapshot::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        reg
    }

    fn pending(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .collect()
    }

    /// The `UserError` key a refusal raises: `(class, (msg, dict))`.
    fn raised_key(e: ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("a raise, got {e:?}");
        };
        let PyValue::Tuple(parts) = *header else {
            panic!("the exception header is (class, args)");
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError takes (msg, dict)");
        };
        match &args[0] {
            PyValue::Bytes(key) => String::from_utf8_lossy(key).into_owned(),
            other => panic!("the key is a str, got {other:?}"),
        }
    }

    /// `SetModuleOnline` stops raising: it answers `None` and tells godma
    /// with the online effect, start=1.
    #[test]
    fn set_module_online_answers_and_tells_godma() {
        let mut reg = alice_with_a_gun();
        reg.modules_mut().set_online(GUN, false);
        let ok = set_module_online(&mut reg, Some(ALICE), GUN, true, 1).expect("answered");
        assert_eq!(ok, PyValue::None);
        assert!(reg.modules().is_online(GUN));
        let sent = pending(&mut reg);
        let (event, args) = &sent[0];
        assert_eq!(*event, "OnGodmaShipEffect");
        assert_eq!(args[1], PyValue::Int(EFFECT_ONLINE));
        assert_eq!(args[3], PyValue::Int(1));
    }

    /// **Offline survives a restart**: the fitting window's and the HUD's
    /// offline are written to the world file's delta rows, a load puts the
    /// module back offline, onlining removes the row, and a row whose item
    /// is gone is dropped.
    #[test]
    fn an_offlined_module_stays_offline_across_a_reload() {
        let mut reg = alice_with_a_gun();
        deactivate(&mut reg, Some(ALICE), GUN, "online", 1).expect("answered");
        assert!(!reg.modules().is_online(GUN));
        let key = vec![GUN.to_string()];
        assert!(reg.deltas().of(OFFLINE_KIND).contains_key(&key));
        // A fresh process: the in-memory state is gone, the rows are not.
        reg.modules_mut().set_online(GUN, true);
        reg.deltas_mut()
            .put(OFFLINE_KIND, vec!["123".to_owned()], vec!["1".to_owned()]);
        restore_offline(&mut reg);
        assert!(!reg.modules().is_online(GUN));
        // A row for a module that is gone is kept (load drops nothing) and
        // changes nothing.
        assert!(
            reg.deltas()
                .of(OFFLINE_KIND)
                .contains_key(&vec!["123".to_owned()])
        );
        set_module_online(&mut reg, Some(ALICE), GUN, true, 2).expect("answered");
        assert!(!reg.deltas().of(OFFLINE_KIND).contains_key(&key));
    }

    /// Onlining an online module is `EffectAlreadyActive2` -- CCP's key, and
    /// the one the fitting window does not revert on `[V]`.
    #[test]
    fn onlining_twice_is_ccps_already_active_key() {
        let mut reg = alice_with_a_gun();
        reg.modules_mut().set_online(GUN, true);
        let e = set_module_online(&mut reg, Some(ALICE), GUN, true, 1).unwrap_err();
        assert_eq!(raised_key(e), "EffectAlreadyActive2");
    }

    /// An offline module refuses with `ModuleNotOnline` `[V]` (§7.2).
    #[test]
    fn an_offline_module_will_not_activate() {
        let mut reg = alice_with_a_gun();
        reg.modules_mut().set_online(GUN, false);
        let e = activate(&mut reg, Some(ALICE), GUN, "projectileFired", None, 1000, 1).unwrap_err();
        assert_eq!(raised_key(e), "ModuleNotOnline");
    }

    /// The happy path for a self-targeted effect: the reply is `None` and the
    /// first push is `OnGodmaShipEffect(start=1)` naming the module, with the
    /// duration from the type's own attribute `[V]` (§0.2).
    #[test]
    fn an_activation_is_confirmed_with_the_cycle_time() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 100);
        reg.modules_mut().set_online(GUN, true);
        // A gun is a targeted effect; take a lock so the effect has one.
        let target = 100_000_099;
        reg.locks_mut().add(ALICE_SHIP, target);
        let ok = activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            1,
        )
        .expect("answered");
        assert_eq!(ok, PyValue::None, "the reply is discarded: smb:1580");
        let sent = pending(&mut reg);
        let (event, args) = &sent[0];
        assert_eq!(*event, "OnGodmaShipEffect");
        assert_eq!(args[0], PyValue::Int(GUN));
        assert_eq!(args[1], PyValue::Int(34));
        assert_eq!(args[3], PyValue::Int(1));
        assert_eq!(args[7], PyValue::Int(module(486).duration_ms()));
        assert_eq!(args[8], PyValue::Int(1000));
        assert!(
            sent.iter().any(|(e, a)| *e == "OnSpecialFX"
                && a[5] == PyValue::Bytes(b"effects.ProjectileFired".to_vec())),
            "the gun is drawn in space"
        );

        // And a second activation is CCP's own refusal (`bdl:926`).
        let e = activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            2,
        )
        .unwrap_err();
        assert_eq!(raised_key(e), "EffectAlreadyActive2");
    }

    /// A fitted singleton module is primed in `shipInfo` with its duration
    /// attribute -- the two preconditions of §0.1 `[V]`.
    #[test]
    fn a_fitted_module_is_primed_with_its_duration() {
        let reg = alice_with_a_gun();
        let c = reg.character(ALICE).expect("Alice").clone();
        let entries = ship_info_entries(&reg, &c);
        assert_eq!(entries.len(), 1);
        let (row, key) = &entries[0];
        assert_eq!(*key, PyValue::Int(GUN));
        let attributes = format!("{row:?}");
        let speed = PyValue::Real(module(486).duration_ms() as f64);
        assert!(
            attributes.contains(&format!("{speed:?}")),
            "speed, the duration attribute, is served"
        );
    }

    // ---- fitting (`docs/lanes/fitting.md` §3, tests 21-22) ----

    const MERLIN: i64 = 100_000_060;
    /// `flagMedSlot0` [V].
    const MED_SLOT_0: i64 = 19;

    /// Alice docked in a Merlin (603), with these `(item, type, flag)`
    /// modules fitted as singletons.
    fn alice_in_a_merlin(modules: &[(i64, i64, i64)]) -> Registry {
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        });
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let merlin = *crate::item::find_in(reg.content(), 603).expect("Merlin is spawnable");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: MERLIN,
            type_id: 603,
            owner_id: ALICE,
            location_id: station,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: merlin.group_id,
            category_id: merlin.category_id,
            singleton: true,
        });
        for &(item_id, type_id, flag_id) in modules {
            let t = *crate::item::find_in(reg.content(), type_id).expect("spawnable module");
            items.push(crate::item::Item {
                item_id,
                type_id,
                owner_id: ALICE,
                location_id: MERLIN,
                flag_id,
                quantity: 1,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: true,
            });
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: MERLIN + 100,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = MERLIN;
        c.ship_type_id = 603;
        reg
    }

    /// Test 21: CCP's own online test (`online.py:56`) [V] against the
    /// hull's `cpuOutput`: miners fill a Merlin's CPU until one more leaves
    /// no room for a shield extender, which is refused and stays offline;
    /// with one miner offline it fits. The counts come from the rows.
    #[test]
    fn onlining_past_the_cpu_output_is_refused() {
        let (miner, extender) = (module(483), module(377));
        let out = crate::fitting::hull_in(&Content::fixture(), 603)
            .expect("Merlin")
            .cpu_output;
        let n = ((out - extender.cpu()) / miner.cpu()).floor() as i64 + 1;
        assert!(n as f64 * miner.cpu() + extender.cpu() > out);
        assert!((n - 1) as f64 * miner.cpu() + extender.cpu() <= out);
        assert!(n <= 8, "the miners fit the high rack's flags");

        let ext = 100_000_070;
        let mut fit: Vec<(i64, i64, i64)> = (0..n)
            .map(|k| (100_000_061 + k, 483, HI_SLOT_0 + k))
            .collect();
        fit.push((ext, 377, MED_SLOT_0));
        let mut reg = alice_in_a_merlin(&fit);
        reg.modules_mut().set_online(ext, false);
        assert!(set_module_online(&mut reg, Some(ALICE), ext, true, 1).is_err());
        assert!(!reg.modules().is_online(ext), "a refusal changes nothing");

        set_module_online(&mut reg, Some(ALICE), 100_000_061, false, 2).expect("offlined");
        set_module_online(&mut reg, Some(ALICE), ext, true, 3).expect("now it fits");
        assert!(reg.modules().is_online(ext));
    }

    /// The `kwargs` of `ship.Undock(onlineModules={flag: itemID})`, the
    /// dict as the marshal holds it: `(value, key)`.
    fn undock_kwargs(listed: &[(i64, i64)]) -> Vec<(PyValue, PyValue)> {
        let dict = PyValue::Dict(
            listed
                .iter()
                .map(|&(flag, id)| (PyValue::Int(id), PyValue::Int(flag)))
                .collect(),
        );
        vec![(dict, crate::pyutil::bstr("onlineModules"))]
    }

    /// A Merlin with two miners, one in each of the first two high slots.
    const MINER_A: i64 = 100_000_061;
    const MINER_B: i64 = 100_000_062;
    fn merlin_with_two_miners() -> Registry {
        alice_in_a_merlin(&[(MINER_A, 483, HI_SLOT_0), (MINER_B, 483, HI_SLOT_0 + 1)])
    }

    #[test]
    fn undock_takes_offline_what_the_client_left_out() {
        let mut reg = merlin_with_two_miners();
        assert!(reg.modules().is_online(MINER_B));
        sync_undock_online(
            &mut reg,
            Some(ALICE),
            &undock_kwargs(&[(HI_SLOT_0, MINER_A)]),
        );
        assert!(reg.modules().is_online(MINER_A));
        assert!(!reg.modules().is_online(MINER_B));
        assert!(
            reg.deltas()
                .of(OFFLINE_KIND)
                .contains_key(&vec![MINER_B.to_string()]),
            "the modoff row is written"
        );
    }

    #[test]
    fn undock_onlines_a_listed_module_only_within_budget() {
        let (miner, extender) = (module(483), module(377));
        let out = crate::fitting::hull_in(&Content::fixture(), 603)
            .expect("Merlin")
            .cpu_output;
        let n = ((out - extender.cpu()) / miner.cpu()).floor() as i64 + 1;
        let ext = 100_000_070;
        let mut fit: Vec<(i64, i64, i64)> = (0..n)
            .map(|k| (100_000_061 + k, 483, HI_SLOT_0 + k))
            .collect();
        fit.push((ext, 377, MED_SLOT_0));
        let mut reg = alice_in_a_merlin(&fit);
        reg.modules_mut().set_online(ext, false);
        let mut listed: Vec<(i64, i64)> =
            (0..n).map(|k| (HI_SLOT_0 + k, 100_000_061 + k)).collect();
        listed.push((MED_SLOT_0, ext));

        // Every miner is listed, so there is no room: the client's word
        // does not beat the budget.
        sync_undock_online(&mut reg, Some(ALICE), &undock_kwargs(&listed));
        assert!(!reg.modules().is_online(ext));
        assert!((0..n).all(|k| reg.modules().is_online(100_000_061 + k)));

        // One miner left out comes offline first and frees the room.
        listed.remove(0);
        sync_undock_online(&mut reg, Some(ALICE), &undock_kwargs(&listed));
        assert!(!reg.modules().is_online(100_000_061));
        assert!(reg.modules().is_online(ext));
    }

    #[test]
    fn undock_without_the_kwarg_changes_nothing() {
        let mut reg = merlin_with_two_miners();
        sync_undock_online(&mut reg, Some(ALICE), &[]);
        assert!(reg.modules().is_online(MINER_A) && reg.modules().is_online(MINER_B));
        assert!(reg.deltas().of(OFFLINE_KIND).is_empty());
    }

    #[test]
    fn undock_ignores_a_module_on_another_ship() {
        let mut reg = merlin_with_two_miners();
        let other = 100_000_090;
        let t = *crate::item::find_in(reg.content(), 483).expect("miner");
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: other,
            type_id: 483,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT_0,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: other + 1,
            ..crate::store::Snapshot::default()
        });
        reg.modules_mut().set_online(other, false);
        sync_undock_online(
            &mut reg,
            Some(ALICE),
            &undock_kwargs(&[(HI_SLOT_0, MINER_A), (HI_SLOT_0 + 1, other)]),
        );
        assert!(
            !reg.modules().is_online(other),
            "not this ship's: untouched"
        );
        assert!(reg.modules().is_online(MINER_A));
        assert!(!reg.modules().is_online(MINER_B));
    }

    /// Not wired into `ship.Undock`: the bots (like the client capture of a
    /// ship with nothing online) send `onlineModules={}`, which would take
    /// every module offline. The undock leaves the online state alone.
    #[test]
    fn ship_undock_leaves_the_online_state_alone() {
        let mut reg = merlin_with_two_miners();
        let kwargs = undock_kwargs(&[]);
        let r = reg
            .dispatch(&crate::service::Request {
                service: "ship",
                method: "Undock",
                args: &[],
                kwargs: &kwargs,
                bind_params: None,
                actor: Some(ALICE),
            })
            .expect("undocks");
        assert_eq!(r, PyValue::None);
        assert!(reg.modules().is_online(MINER_A));
        assert!(reg.modules().is_online(MINER_B));
    }

    /// Test 22: a module fitted by `Edit::Assemble` -- a new singleton id
    /// split off a stack -- is primed, which is the seam between fitting and
    /// P2.1.
    #[test]
    fn a_module_assembled_into_a_slot_is_primed() {
        let mut reg = alice_in_a_merlin(&[]);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let t = *crate::item::find_in(reg.content(), 484).expect("125mm");
        let stack = 100_000_080;
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: stack,
            type_id: 484,
            owner_id: ALICE,
            location_id: station,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 2,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: stack + 1,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = MERLIN;
        c.ship_type_id = 603;
        let changes = reg
            .apply_items(&[crate::item::Edit::Assemble {
                item_id: stack,
                to: crate::item::Slot::new(ALICE, MERLIN, HI_SLOT_0),
            }])
            .expect("assembled");
        let fitted = changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a new id");
        let c = reg.character(ALICE).expect("Alice").clone();
        let keys: Vec<PyValue> = ship_info_entries(&reg, &c)
            .into_iter()
            .map(|(_, k)| k)
            .collect();
        assert_eq!(keys, vec![PyValue::Int(fitted)]);
    }

    // ---- charges (P2.3) ----

    const EMP_S: i64 = 185;
    const PHASED_PLASMA_S: i64 = 184;
    /// `quantity` [V] (`dogmaattributes[805]`).
    const QUANTITY: i64 = 805;

    /// Put `n` rounds straight into a slot, as a loaded charge -- the
    /// store's shape for one (`crate::item::is_loaded_charge`).
    fn put_rounds(reg: &mut Registry, ship: i64, flag: i64, type_id: i64, n: i64) {
        reg.apply_items(&[crate::item::Edit::Create {
            type_id,
            quantity: n,
            to: crate::item::Slot::new(ALICE, ship, flag),
        }])
        .expect("loaded");
        let _ = reg.take_pending();
    }

    /// `n` rounds of `type_id` in Alice's station hangar; the new stack's id.
    fn hangar_rounds(reg: &mut Registry, type_id: i64, n: i64) -> i64 {
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity: n,
                to: crate::item::Slot::new(ALICE, station, crate::model::FLAG_HANGAR),
            }])
            .expect("spawned");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a stack")
    }

    /// Every round of `type_id` Alice owns, wherever it is.
    fn rounds_owned(reg: &Registry, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ALICE && i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    fn loaded_count(reg: &Registry, ship: i64, flag: i64) -> Option<(i64, i64)> {
        loaded(reg.store(), ship, flag).map(|i| (i.type_id, i.quantity))
    }

    const MERLIN_GUN: i64 = 100_000_070;

    /// The capacity the HUD counts against: the module's `capacity` over
    /// the round's `volume`, with the client's epsilon (`smb
    /// GetChargeReloadInfo`) [V], from the content's item rows. A module is
    /// not a round.
    #[test]
    fn a_gun_holds_its_capacity_over_the_rounds_volume() {
        let c = Content::fixture();
        let item = |id| *crate::item::find_in(&c, id).expect("an item row");
        for gun in [484, 486] {
            for round in [EMP_S, PHASED_PLASMA_S] {
                let want = (item(gun).capacity / item(round).volume + 1e-07) as i64;
                assert!(want > 0);
                assert_eq!(rounds_capacity_in(&c, gun, round), want);
            }
        }
        assert_eq!(
            rounds_capacity_in(&c, 484, 3640),
            0,
            "a module is not a round"
        );
    }

    /// **Rounds are conserved by a load**: what leaves the hangar is what is
    /// in the gun, up to the gun's capacity. And godma is told in the
    /// sublocation's shapes [V]: a prime of the `(shipID, flagID, typeID)`
    /// row carrying attribute 805 (`godma:1904` refuses it without), and an
    /// `OnModuleAttributeChanges` list of eight-tuples (`cdl:391`
    /// `changes.sort`, unpack 8; `godma:1620` `each[1:] + (0,)`).
    #[test]
    fn loading_from_the_hangar_moves_rounds_into_the_gun_and_tells_godma() {
        let mut reg = alice_in_a_merlin(&[(MERLIN_GUN, 484, HI_SLOT_0)]);
        let full = rounds_capacity_in(reg.content(), 484, EMP_S);
        assert!(full < 500, "more rounds than the gun holds");
        let stack = hangar_rounds(&mut reg, EMP_S, 500);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let ok =
            load_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &[stack], station, 1).expect("loaded");
        assert_eq!(ok, PyValue::None, "the reply is discarded: cdl:1201");
        assert_eq!(loaded_count(&reg, MERLIN, HI_SLOT_0), Some((EMP_S, full)));
        assert_eq!(reg.item(stack).map(|i| i.quantity), Some(500 - full));
        assert_eq!(rounds_owned(&reg, EMP_S), 500, "no round made or lost");

        let sent = pending(&mut reg);
        let key = charge_key(MERLIN, HI_SLOT_0, EMP_S);
        let (_, prime) = sent
            .iter()
            .find(|(e, _)| *e == "OnGodmaPrimeItem")
            .expect("a new sublocation is primed");
        assert_eq!(prime[0], PyValue::Int(MERLIN), "locationID is the ship");
        let row = format!("{:?}", prime[1]);
        assert!(
            row.contains(&format!("{key:?}")),
            "keyed by the tuple: {row}"
        );
        assert!(
            row.contains(&format!(
                "{:?}",
                (PyValue::Int(full), PyValue::Int(QUANTITY))
            )),
            "attribute 805 carries the count: {row}"
        );
        let (_, changes) = sent
            .iter()
            .find(|(e, _)| *e == "OnModuleAttributeChanges")
            .expect("the fitting window's news");
        let PyValue::List(list) = &changes[0] else {
            panic!("a list: cdl sorts it")
        };
        let PyValue::Tuple(change) = &list[0] else {
            panic!("a tuple: godma adds (0,) to its tail")
        };
        assert_eq!(change.len(), 8);
        assert_eq!(change[1], PyValue::Int(ALICE), "ownerID is the pilot");
        assert_eq!(change[2], key);
        assert_eq!(change[3], PyValue::Int(QUANTITY));
        assert_eq!(change[5], PyValue::Int(full));
        assert!(
            !sent.iter().any(|(e, a)| *e == "OnItemChange"
                && format!("{a:?}").contains(&format!("Int({HI_SLOT_0})"))
                && format!("{a:?}").contains(&format!("Int({EMP_S})"))),
            "the loaded charge is never an item row"
        );

        // Topping up a full gun moves nothing.
        load_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &[stack], station, 2).expect("answered");
        assert_eq!(loaded_count(&reg, MERLIN, HI_SLOT_0), Some((EMP_S, full)));
        assert_eq!(rounds_owned(&reg, EMP_S), 500);
    }

    /// **Rounds are conserved by a change of ammo**: loading Phased Plasma
    /// into a gun full of EMP takes the EMP out and puts the plasma in, and
    /// neither type gains or loses a round.
    #[test]
    fn switching_ammo_conserves_both_types() {
        let mut reg = alice_in_a_merlin(&[(MERLIN_GUN, 484, HI_SLOT_0)]);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let full = rounds_capacity_in(reg.content(), 484, PHASED_PLASMA_S);
        assert!((150..250).contains(&full), "the item rows' shape");
        let emp = hangar_rounds(&mut reg, EMP_S, 150);
        let pp = hangar_rounds(&mut reg, PHASED_PLASMA_S, 250);
        load_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &[emp], station, 1).expect("EMP in");
        let _ = reg.take_pending();
        load_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &[pp], station, 2).expect("plasma in");
        assert_eq!(
            loaded_count(&reg, MERLIN, HI_SLOT_0),
            Some((PHASED_PLASMA_S, full))
        );
        assert_eq!(rounds_owned(&reg, EMP_S), 150);
        assert_eq!(rounds_owned(&reg, PHASED_PLASMA_S), 250);
        let sent = pending(&mut reg);
        let emptied = sent.iter().any(|(e, a)| {
            *e == "OnModuleAttributeChanges"
                && format!("{a:?}").contains(&format!("{:?}", charge_key(MERLIN, HI_SLOT_0, EMP_S)))
                && format!("{a:?}").contains("Int(0)")
        });
        assert!(emptied, "the EMP sublocation is counted down to zero");
    }

    /// **Rounds are conserved by an unload**, and the answer names the
    /// stack they landed in (`FitCharges` reads `itemID[0]`) [V].
    #[test]
    fn unloading_returns_every_round_and_names_the_stack() {
        let mut reg = alice_in_a_merlin(&[(MERLIN_GUN, 484, HI_SLOT_0)]);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let stack = hangar_rounds(&mut reg, EMP_S, 120);
        load_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &[stack], station, 1).expect("loaded");
        let _ = reg.take_pending();
        let destination = PyValue::Tuple(vec![
            PyValue::Int(station),
            PyValue::Int(ALICE),
            PyValue::Int(crate::model::FLAG_HANGAR),
        ]);
        let PyValue::List(ids) =
            unload_ammo(&mut reg, Some(ALICE), &[MERLIN_GUN], &destination, None, 2)
                .expect("unloaded")
        else {
            panic!("a list of ids")
        };
        assert_eq!(loaded_count(&reg, MERLIN, HI_SLOT_0), None);
        assert_eq!(rounds_owned(&reg, EMP_S), 120);
        let PyValue::Int(landed) = ids[0] else {
            panic!("an item id")
        };
        let at = reg.item(landed).expect("the stack exists");
        assert_eq!((at.location_id, at.quantity), (station, 120));
    }

    /// **A gun with nothing loaded does not cycle**: whatever the refusal,
    /// nothing runs and no cycle is queued for combat to turn into damage.
    #[test]
    fn an_empty_gun_does_not_cycle() {
        let mut reg = alice_with_a_gun();
        reg.modules_mut().set_online(GUN, true);
        let target = 100_000_099;
        reg.locks_mut().add(ALICE_SHIP, target);
        let r = activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            1,
        );
        assert!(r.is_err(), "refused");
        assert!(reg.modules().running(GUN).is_none());
        assert!(reg.modules_mut().take_cycles().is_empty());
    }

    /// **GUN-CYCLE: a target's loss does not end the running cycle.** The
    /// owner's bug: a gun that instakilled its target was free to fire again
    /// at once. The module keeps running to the end of its cycle, an Activate
    /// before that is refused, and at the boundary it stops and works again.
    #[test]
    fn a_lost_target_does_not_free_the_gun_before_the_cycle_ends() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 10);
        reg.modules_mut().set_online(GUN, true);
        let (target, next) = (100_000_099, 100_000_098);
        reg.locks_mut().add(ALICE_SHIP, target);
        reg.locks_mut().add(ALICE_SHIP, next);
        let t0 = 1_000 * MSEC;
        let go = |reg: &mut Registry, on: i64, now: i64| {
            activate(
                reg,
                Some(ALICE),
                GUN,
                "projectileFired",
                Some(on),
                1000,
                now,
            )
        };
        go(&mut reg, target, t0).expect("fires");
        let period = reg.modules().running(GUN).expect("running").period;
        // The target dies a moment into the cycle.
        reg.locks_mut().remove(ALICE_SHIP, target);
        let mid = t0 + period / 2;
        let e = go(&mut reg, next, mid).expect_err("still cycling");
        assert_eq!(raised_key(e), "EffectAlreadyActive2");
        assert!(reg.modules().running(GUN).is_some(), "the cycle runs on");
        // At the boundary it stops, and then it can be activated again.
        tick(&mut reg, t0 + period);
        assert!(reg.modules().running(GUN).is_none(), "stopped at the end");
        go(&mut reg, next, t0 + period).expect("free after the cycle");
    }

    /// **One round per cycle** -- `chargeRate` 1 [V] -- named on the cycle
    /// for combat, counted down on the HUD, and **no cycle without a round**:
    /// three rounds are three cycles that fired EMP S, however long the
    /// trigger is held, and every round is accounted for.
    #[test]
    fn each_cycle_fires_one_round_and_none_fires_without_one() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 3);
        reg.modules_mut().set_online(GUN, true);
        let target = 100_000_099;
        reg.locks_mut().add(ALICE_SHIP, target);
        let t0 = 1_000 * MSEC;
        activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            t0,
        )
        .expect("fires");
        assert_eq!(loaded_count(&reg, ALICE_SHIP, HI_SLOT_0), Some((EMP_S, 2)));
        let sent = pending(&mut reg);
        let counted = sent.iter().any(|(e, a)| {
            *e == "OnModuleAttributeChanges"
                && format!("{a:?}")
                    .contains(&format!("{:?}", charge_key(ALICE_SHIP, HI_SLOT_0, EMP_S)))
        });
        assert!(counted, "the HUD's count moves");

        // Ten minutes of the gun's cycle.
        tick(&mut reg, t0 + 600_000 * MSEC);
        let cycles = reg.modules_mut().take_cycles();
        assert_eq!(cycles.len(), 3, "{cycles:?}");
        assert!(cycles.iter().all(|c| c.charge_type_id == Some(EMP_S)));
        assert_eq!(loaded_count(&reg, ALICE_SHIP, HI_SLOT_0), None);
        assert_eq!(rounds_owned(&reg, EMP_S), 0, "three fired, none left over");
    }

    /// **The last round rebuilds the HUD button and the module stops at the
    /// end of that cycle**: the owner's "keeps cycling, no reload" -- the
    /// count going to zero is ignored by `OnInvChange`, so the button kept a
    /// stale charge. The module's own row goes out with `ixFlag`, once, with
    /// the round that empties it and not before; the stop pair follows at the
    /// next boundary; a `Deactivate` while it cycles is honoured; and a
    /// reload afterwards starts it again.
    #[test]
    fn the_last_round_rebuilds_the_button_and_the_gun_stops_after_it() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 2);
        reg.modules_mut().set_online(GUN, true);
        let target = 100_000_099;
        reg.locks_mut().add(ALICE_SHIP, target);
        let t0 = 1_000 * MSEC;
        let refits = |sent: &[(&'static str, Vec<PyValue>)]| {
            sent.iter().filter(|(e, _)| *e == "OnItemChange").count()
        };
        let due = |reg: &mut Registry| -> Vec<(&'static str, Vec<PyValue>)> {
            reg.take_due(std::time::Instant::now() + Duration::from_secs(1))
                .into_iter()
                .map(|p| p.to_push())
                .collect()
        };
        activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            t0,
        )
        .expect("fires");
        assert_eq!(refits(&pending(&mut reg)), 0, "a round is left: no rebuild");
        let period = reg.modules().running(GUN).expect("running").period;
        // The second cycle fires the last round.
        tick(&mut reg, t0 + period);
        assert_eq!(loaded_count(&reg, ALICE_SHIP, HI_SLOT_0), None);
        assert!(reg.modules().running(GUN).is_some(), "the last cycle runs");
        let sent = due(&mut reg);
        assert_eq!(refits(&sent), 1, "{sent:?}");
        // The next boundary has nothing to fire: it stops.
        tick(&mut reg, t0 + 2 * period);
        assert!(reg.modules().running(GUN).is_none(), "stopped, not cycling");

        // Reload and run again; a Deactivate mid-cycle ends it at the
        // boundary.
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 5);
        let t1 = t0 + 3 * period;
        activate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            Some(target),
            1000,
            t1,
        )
        .expect("fires again");
        deactivate(
            &mut reg,
            Some(ALICE),
            GUN,
            "projectileFired",
            t1 + period / 2,
        )
        .expect("honoured");
        tick(&mut reg, t1 + 3 * period);
        assert!(reg.modules().running(GUN).is_none());
        assert_eq!(
            loaded_count(&reg, ALICE_SHIP, HI_SLOT_0),
            Some((EMP_S, 4)),
            "one cycle ran, then it stopped"
        );
    }

    /// A loaded charge is primed in `shipInfo` as a sublocation, keyed by
    /// its tuple, with 805 -- so a relog shows the count [V] (§6.1).
    #[test]
    fn a_loaded_charge_is_primed_as_a_sublocation() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 77);
        let c = reg.character(ALICE).expect("Alice").clone();
        let entries = ship_info_entries(&reg, &c);
        let key = charge_key(ALICE_SHIP, HI_SLOT_0, EMP_S);
        let (row, _) = entries
            .iter()
            .find(|(_, k)| *k == key)
            .expect("the charge is primed");
        assert!(
            format!("{row:?}")
                .contains(&format!("{:?}", (PyValue::Int(77), PyValue::Int(QUANTITY)))),
            "with its count"
        );
        assert!(
            entries.iter().any(|(_, k)| *k == PyValue::Int(GUN)),
            "the gun is still primed as itself"
        );
    }

    /// The rounds stay with their gun: whatever an unfit of a loaded module
    /// answers, it never leaves a charge in a slot with no module.
    #[test]
    fn an_unfit_never_strands_the_rounds() {
        let mut reg = alice_in_a_merlin(&[(MERLIN_GUN, 484, HI_SLOT_0)]);
        put_rounds(&mut reg, MERLIN, HI_SLOT_0, EMP_S, 50);
        let c = reg.character(ALICE).cloned();
        if let Ok(plan) = crate::fitting::plan_unfit(&reg, c.as_ref(), MERLIN_GUN, None) {
            reg.apply_items(&plan).expect("applied");
        }
        let module_here = reg
            .item(MERLIN_GUN)
            .is_some_and(|m| m.location_id == MERLIN && m.flag_id == HI_SLOT_0);
        let rounds_here = loaded_count(&reg, MERLIN, HI_SLOT_0).is_some();
        assert!(!rounds_here || module_here, "stranded rounds");
        assert_eq!(rounds_owned(&reg, EMP_S), 50);
    }

    /// `n` units of `type_id` in a ship's cargo hold; the new stack's id.
    fn cargo_rounds(reg: &mut Registry, ship: i64, type_id: i64, n: i64) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id,
                quantity: n,
                to: crate::item::Slot::new(ALICE, ship, crate::fitting::FLAG_CARGO),
            }])
            .expect("spawned");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a stack")
    }

    /// **The in-space reload path, once ammo is in the hold**: the client's
    /// auto-reload lists the hold (`cdl:1182`, `List(flagCargo)`) and sends
    /// `LoadAmmo(shipID, moduleID, [ids], shipID)` -- the ship as
    /// `chargeLocationID` (`cdl:1184` -> `:1201`) [V]. The gun fills from
    /// the hold, no round is made or lost, and the reload is announced.
    #[test]
    fn in_space_a_gun_reloads_from_the_cargo_hold() {
        let mut reg = alice_with_a_gun();
        let stack = cargo_rounds(&mut reg, ALICE_SHIP, EMP_S, 300);
        load_ammo(&mut reg, Some(ALICE), &[GUN], &[stack], ALICE_SHIP, 1).expect("reloaded");
        let full = rounds_capacity_in(reg.content(), 486, EMP_S);
        assert_eq!(
            loaded_count(&reg, ALICE_SHIP, HI_SLOT_0),
            Some((EMP_S, full))
        );
        assert_eq!(reg.item(stack).map(|i| i.quantity), Some(300 - full));
        assert_eq!(rounds_owned(&reg, EMP_S), 300);
        let sent = pending(&mut reg);
        assert!(
            sent.iter()
                .any(|(e, _)| *e == "OnChargeBeingLoadedToModule"),
            "{sent:?}"
        );
    }

    /// **In space the station hangar is out of reach for rounds**, both
    /// ways. `Character::station_id` stays set when the pilot undocks, so a
    /// gun in a belt used to fill from the hangar at home and empty back
    /// into it (`docs/lanes/docked.md`). The docked halves of both are
    /// `loading_from_the_hangar_moves_rounds_into_the_gun_and_tells_godma`
    /// and the unload test beside it.
    #[test]
    fn in_space_the_hangar_neither_feeds_a_gun_nor_takes_its_rounds() {
        let mut reg = alice_with_a_gun();
        assert_eq!(reg.docked_station(ALICE), None);
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("a home station");
        let stack = hangar_rounds(&mut reg, EMP_S, 300);
        assert!(load_ammo(&mut reg, Some(ALICE), &[GUN], &[stack], station, 1).is_err());
        assert_eq!(loaded_count(&reg, ALICE_SHIP, HI_SLOT_0), None);
        assert_eq!(reg.item(stack).map(|i| i.quantity), Some(300));

        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 100);
        let destination = PyValue::Tuple(vec![
            PyValue::Int(station),
            PyValue::Int(ALICE),
            PyValue::Int(crate::model::FLAG_HANGAR),
        ]);
        assert!(unload_ammo(&mut reg, Some(ALICE), &[GUN], &destination, None, 2).is_err());
        assert_eq!(
            loaded_count(&reg, ALICE_SHIP, HI_SLOT_0),
            Some((EMP_S, 100))
        );
        assert_eq!(rounds_owned(&reg, EMP_S), 400, "no round made or lost");
    }

    /// **The hold is never over capacity**, unloading included: rounds that
    /// do not fit stay in the gun, and nothing is made or lost.
    #[test]
    fn unloading_into_a_full_hold_is_refused_and_keeps_the_rounds() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 100);
        let free = crate::fitting::cargo_free_in(&reg, ALICE_SHIP);
        let trit = crate::item::find_in(reg.content(), 34)
            .expect("Tritanium")
            .volume;
        cargo_rounds(
            &mut reg,
            ALICE_SHIP,
            34,
            crate::fitting::units_that_fit(free, trit),
        );
        let r = unload_ammo(
            &mut reg,
            Some(ALICE),
            &[GUN],
            &PyValue::Int(ALICE_SHIP),
            None,
            1,
        );
        assert!(r.is_err(), "no room");
        assert_eq!(
            loaded_count(&reg, ALICE_SHIP, HI_SLOT_0),
            Some((EMP_S, 100))
        );
        assert_eq!(rounds_owned(&reg, EMP_S), 100);
        let used =
            reg.store()
                .used_volume_in(reg.content(), ALICE_SHIP, crate::fitting::FLAG_CARGO);
        assert!(used <= crate::fitting::cargo_capacity_in(&reg, ALICE_SHIP) + 1e-9);
    }

    /// `shipState[1]` names every loaded charge of the ship, keyed by ship
    /// then flag, and nothing of any other ship [V shape:
    /// `baseDogmaLocation.py:310`, `:1579`].
    #[test]
    fn the_quantity_cache_lists_each_loaded_slot_once() {
        let mut reg = alice_with_a_gun();
        put_rounds(&mut reg, ALICE_SHIP, HI_SLOT_0, EMP_S, 33);
        let PyValue::Tuple(state) = ship_state(reg.store(), ALICE_SHIP) else {
            panic!("a 4-tuple")
        };
        assert_eq!(state.len(), 4);
        let text = format!("{:?}", state[1]);
        assert!(text.contains("Int(33)"), "{text}");
        assert_eq!(
            ship_state(reg.store(), ALICE_SHIP + 1),
            PyValue::Tuple(vec![
                PyValue::Dict(vec![]),
                PyValue::Dict(vec![]),
                PyValue::None,
                PyValue::Dict(vec![]),
            ])
        );
    }

    // ---- no fitting in space (P2.5) ----

    /// **A module cannot leave its slot in space**: not to the station
    /// hangar (an unfit) and not to the cargo hold. Either would change the
    /// ship under observers whose clients drew its guns once, at arrival,
    /// and do not redraw them on a slim change (`ship.py:304-323`) [V].
    #[test]
    fn in_space_a_fitted_module_stays_in_its_slot() {
        let reg = alice_with_a_gun();
        assert!(reg.roster().is_in_space(ALICE));
        let c = reg.character(ALICE).cloned();
        assert_eq!(
            crate::fitting::plan_unfit(&reg, c.as_ref(), GUN, None),
            Err(crate::fitting::Refusal::NotDocked)
        );
        assert!(crate::fitting::plan_to_cargo(&reg, c.as_ref(), ALICE_SHIP, GUN, None).is_err());
        assert_eq!(
            slim_modules(reg.store(), ALICE_SHIP),
            vec![crate::destiny::SlimModule {
                item_id: GUN,
                type_id: 486,
                flag_id: HI_SLOT_0,
            }]
        );
    }

    /// Docked, the same pilot may refit: the refusal is about space, not
    /// about the pilot.
    #[test]
    fn docked_the_in_space_refusal_does_not_apply() {
        let reg = alice_in_a_merlin(&[(MERLIN_GUN, 484, HI_SLOT_0)]);
        let c = reg.character(ALICE).cloned().expect("Alice");
        assert_eq!(crate::fitting::refuse_in_space(&reg, &c), Ok(()));
        assert!(crate::fitting::plan_unfit(&reg, Some(&c), MERLIN_GUN, None).is_ok());
    }
}

#[cfg(test)]
mod defense_tests;
#[cfg(test)]
mod local_tank_tests;
#[cfg(test)]
mod rules_tests;
