//! Combat drones: launched from a ship's drone bay, they fly, fight and come
//! home.
//!
//! Spec and evidence: `docs/lanes/drones.md`. Owned by the drones lane.
//!
//! **A drone in space is an NPC that a player owns.** It is a server-side
//! ball in the roster under its own id -- `Presence::character_id` and
//! `ship_id` are both the drone's **item** id -- with a pure brain
//! ([`decide`]) run on the world tick, exactly the shape `crate::npc` gave a
//! hostile rat. The differences are the owner and the orders: a drone does
//! nothing by itself, it does what its controller last told it
//! (`entity.CmdEngage`, `CmdReturnBay`, `CmdReturnHome`), and its volleys go
//! through `combat::fire` **as its owner's**, so the owner's combat log, the
//! victim's bars and a kill are the same code a turret uses.
//!
//! **The item is the drone.** Launching moves the item out of the bay
//! (`flagDroneBay` 87, `inventorycommon/const` **[V]**) to the solar system
//! (`flag` 0, as a wreck); scooping moves it back. Nothing is created or
//! destroyed, so "bay + space = what the player owns" is an invariant the
//! store keeps, not one this module has to remember.
//!
//! **What the client is told, and why each one** (all **[V]**, file:line in
//! `docs/lanes/drones.md` §2):
//!
//! - the ball and its slim (`AddBalls2`), `ownerID` the **character**: the
//!   client's dogma loads a drone only when `slimItem.ownerID` is the pilot
//!   (`clientDogmaLocation.py:1723`, `godma.py:2654`);
//! - `OnDroneStateChange(droneID, ownerID, controllerID, activityState,
//!   droneTypeID, controllerOwnerID, targetID)` (`michelle.py:386-388`),
//!   which is the **only** source of the drone window's "in space" list
//!   (`dronesUtil.GetDronesInLocalSpace:26-36` reads
//!   `michelle.GetDrones()`, i.e. `stateByDroneID`), and the same rows in the
//!   bootstrap's `droneState` (`michelle.py:1228-1231`);
//! - the item's `OnItemChange`, which is what makes the window reload
//!   (`dronesWindow.OnItemChange:346-359` -- only for an item entering or
//!   leaving `flagDroneBay` of the active ship).
//!
//! Drones are **not persisted** as balls, like the roster. A drone item a
//! restart left in space is swept back to its owner on the first tick
//! ([`sweep_orphans`]).

use crate::content::Content;
use crate::damage::{BlueTime, Damage, Shot};
use crate::destiny::hull::Hull;
use crate::destiny::{self, Action, Mode};
use crate::drone_skills::{self, Settings};
use crate::item::{Edit, Slot};
use crate::notify::{Addressed, Push};
use crate::service::{Registry, Request, ServiceError};
use crate::space_state::{Motion, Presence};
use crate::typedata;
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// `const.flagDroneBay` (`inventorycommon/const`) **[V]**; also
/// `invCtrl.ShipDroneBay.locationFlag` (`invControllers.py:809`) **[V]**.
pub const FLAG_DRONE_BAY: i64 = 87;

/// `const.categoryDrone` (`inventorycommon/const`) **[V]**.
pub const CATEGORY_DRONE: i64 = 18;

/// `const.groupCombatDrone` **[V]**; group 100 "Combat Drone" is in category
/// 18 in `groups.fsdbinary` **[V-data]**.
pub const GROUP_COMBAT_DRONE: i64 = 100;

/// Where a launched drone's item lives: the solar system at flag 0, as a
/// wreck does (`combat::FLAG_IN_SPACE`).
pub const FLAG_IN_SPACE: i64 = crate::combat::FLAG_IN_SPACE;

/// The **Drones** skill. Its `maxActiveDroneBonus` is 1 per level
/// (`tools/dogma.py type 3436`) **[V-data]**, added to the character's
/// `maxActiveDrones` (attribute 352) by
/// `dronesMaxActiveDroneBonusModAddMaxActiveLocationChar` **[I]** from the
/// effect's name. So a pilot without it can launch nothing.
pub const SKILL_DRONES: i64 = 3436;

/// The character's base `droneControlDistance` (attribute 458), 20 km on
/// type 1373 **[V-data]**. `[A]` no Drone Avionics: no skill extends it.
pub const CONTROL_RANGE: f64 = 20_000.0;

/// `[A]` How close a drone must be to its ship to be scooped, and where a
/// returning drone is taken back aboard. 2,500 m is
/// `combat::LOOT_RANGE`, the one "close enough to touch" this server has.
/// Falsified by a scoop the client calls too far, or a return that docks
/// visibly early.
pub const SCOOP_RANGE: f64 = 2_500.0;

/// `[A]` How far outside its ship's hull a drone appears.
pub const LAUNCH_CLEARANCE: f64 = 150.0;

/// `[A]` Inside this fraction of its optimal range an approaching drone
/// starts to orbit; between it and the optimal it keeps doing what it was
/// doing. `npc::ORBIT_ENTER_FRACTION`'s band, for the same reason.
pub const ORBIT_ENTER_FRACTION: f64 = 0.8;

/// The most volleys one drone lands in one tick (`npc::MAX_SHOTS_PER_TICK`).
pub const MAX_SHOTS_PER_TICK: u32 = 8;

/// `[A]` The `FollowBall` range a returning drone is given: close, not zero.
pub const RETURN_FOLLOW_RANGE: f64 = 50.0;

// The drone window's activity states: `appConst.entity*` **[V]**.
pub const ENTITY_IDLE: i64 = 0;
pub const ENTITY_COMBAT: i64 = 1;
pub const ENTITY_DEPARTING: i64 = 4;
pub const ENTITY_DEPARTING2: i64 = 5;
pub const ENTITY_ENGAGE: i64 = 10;
/// `appConst.entityMining` **[V]** (`eve/common/lib/appConst.pyc:1432`).
pub const ENTITY_MINING: i64 = 2;

/// Blue time is 10,000 ticks to the millisecond.
const BLUE_PER_MS: i64 = 10_000;

/// `[A]` How long a drone waits for a controller that is not in its system
/// (docked, jumped, logged off, dead) before it is taken out of space:
/// five minutes, on the blue clock. A ship that still exists gets its drone
/// back in the pilot's last station hangar; a ship that is gone loses it.
/// Falsified by a pilot who loses drones to a short dock, or by drones that
/// litter a system for longer than a player would believe.
pub const ABANDON_AFTER: BlueTime = 300 * 1000 * BLUE_PER_MS;

// ---------------------------------------------------------------------------
// Static data
// ---------------------------------------------------------------------------

/// A ship's drone bay and drone bandwidth: attributes 283
/// (`droneCapacity`, m3) and 1271 (`droneBandwidth`, Mbit/s)
/// (`dogma/const`) **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bay {
    pub ship_type_id: i64,
    pub capacity: f64,
    pub bandwidth: f64,
}

/// A hull's drone bay, read from the loaded type data (CP58e, ruling 3):
/// the ship's own attributes 283 and 1271. `None` for a type that is not a
/// ship, or a ship with neither (no bay). `[A]` nothing modifies either
/// number (no rigs, no skills).
pub fn bay_in(content: &Content, ship_type_id: i64) -> Option<Bay> {
    let table = typedata::installed_in(content).table();
    let row = table.get(ship_type_id)?;
    if row.category_id != typedata::CATEGORY_SHIP {
        return None;
    }
    let a = |id| row.attribute(id).unwrap_or(0.0);
    let bay = Bay {
        ship_type_id,
        capacity: a(typedata::attr::DRONE_CAPACITY),
        bandwidth: a(typedata::attr::DRONE_BANDWIDTH),
    };
    (bay.capacity > 0.0 || bay.bandwidth > 0.0).then_some(bay)
}
/// One kind of combat drone: its hull, its gun, how it flies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DroneType {
    pub type_id: i64,
    pub name: &'static str,
    /// Attribute 1272 `droneBandwidthUsed`.
    pub bandwidth: f64,
    pub hull: Hull,
    /// Attribute 51 `speed`: the gun's cycle, in milliseconds -- effect 10's
    /// `durationAttributeID` **[V-data]**.
    pub weapon_period_ms: i64,
    /// Attributes 114/116/117/118, on the drone itself.
    pub damage: Damage,
    /// Attribute 64 `damageMultiplier`.
    pub damage_multiplier: f64,
    /// Attribute 54 `maxRange`: effect 10's `rangeAttributeID` **[V-data]**.
    pub optimal: f64,
    /// Attribute 416 `entityFlyRange`: the orbit range.
    pub orbit_range: f64,
    /// Attribute 508 `entityCruiseSpeed` over attribute 37 `maxVelocity`.
    pub cruise_fraction: f64,
    /// Attribute 158 `falloff`, metres: the turret formula's, K-DRONE.
    pub falloff: f64,
    /// Attribute 160 `trackingSpeed`, rad/s. **0 means the type states no
    /// tracking and every volley in range hits** (`drone_skills`).
    pub tracking: f64,
    /// Attribute 620 `optimalSigRadius`, metres.
    pub sig_resolution: f64,
}

impl DroneType {
    /// One cycle's damage before resistances: the drone's own damage times
    /// its `damageMultiplier`, the turret rule `npc::NpcType::volley` uses.
    /// A [`DroneType`] from [`find_in`] has the **type's** numbers; one a
    /// launch stored in a [`Drone`] has the pilot's skills and hull bonuses
    /// in its `damage_multiplier` already (`drone_skills::stats`).
    pub fn volley(&self) -> Damage {
        let m = self.damage_multiplier;
        Damage {
            em: self.damage.em * m,
            explosive: self.damage.explosive * m,
            kinetic: self.damage.kinetic * m,
            thermal: self.damage.thermal * m,
        }
    }

    /// The gun's cycle, in blue ticks.
    pub fn weapon_period(&self) -> BlueTime {
        self.weapon_period_ms * BLUE_PER_MS
    }
}

/// Effect 10 `targetAttack`, every light drone's default effect: its `guid`
/// in `dogmaeffects` is `'effects.Laser'` **[V-data]** (`out/drones/eff.py`),
/// the same one the NPC gun sends.
pub const WEAPON_GUID: &str = "effects.Laser";

/// A light drone's hull. Every number is `tools/dogma.py type <id>` /
/// `out/drones/types.py` on build 3619879 **[V-data]** except those marked.
const fn light_hull(
    type_id: i64,
    mass: f64,
    max_velocity: f32,
    shield: f64,
    armor: f64,
    hp: f64,
) -> Hull {
    Hull {
        type_id,
        mass,
        // `types.radius` 15 for all four.
        radius: 15.0,
        max_velocity,
        // Attribute 70 is 100 on all four [V-data]: with a 3-4 t mass that
        // is a tau of 0.3-0.4 s, which is how a drone turns.
        agility: 100.0,
        shield_capacity: shield,
        shield_recharge_rate: 125_000.0,
        armor_hp: armor,
        // `[A]` a drone has no attribute 76. Server-only: the drone holds no
        // lock in this model, so nothing sweeps by it.
        max_target_range: CONTROL_RANGE,
        max_locked_targets: 8.0,
        // `[A]` no attribute 564 on a drone; never read (no locks).
        scan_resolution: 500.0,
        signature_radius: 25.0,
        // Drones do not warp.
        warp_speed_au: 0.0,
        hp,
        capacitor_capacity: 1.0,
        capacitor_recharge_rate: 2_500_000.0,
    }
}

/// Hobgoblin I, 2454: thermal. The Gallente light drone.
pub const HOBGOBLIN_I: DroneType = DroneType {
    type_id: 2454,
    name: "Hobgoblin I",
    bandwidth: 5.0,
    hull: light_hull(2454, 3000.0, 2800.0, 50.0, 90.0, 200.0),
    weapon_period_ms: 4000,
    damage: Damage {
        em: 0.0,
        explosive: 0.0,
        kinetic: 0.0,
        thermal: 20.0,
    },
    damage_multiplier: 1.6,
    optimal: 2100.0,
    orbit_range: 700.0,
    cruise_fraction: 660.0 / 2800.0,
    falloff: 0.0,
    tracking: 0.0,
    sig_resolution: 0.0,
};

/// Acolyte I, 2203: EM.
pub const ACOLYTE_I: DroneType = DroneType {
    type_id: 2203,
    name: "Acolyte I",
    bandwidth: 5.0,
    hull: light_hull(2203, 3000.0, 3850.0, 40.0, 190.0, 170.0),
    weapon_period_ms: 4000,
    damage: Damage {
        em: 20.0,
        explosive: 0.0,
        kinetic: 0.0,
        thermal: 0.0,
    },
    damage_multiplier: 1.4,
    optimal: 2400.0,
    orbit_range: 800.0,
    cruise_fraction: 830.0 / 3850.0,
    falloff: 0.0,
    tracking: 0.0,
    sig_resolution: 0.0,
};

/// Hornet I, 2464: kinetic.
pub const HORNET_I: DroneType = DroneType {
    type_id: 2464,
    name: "Hornet I",
    bandwidth: 5.0,
    hull: light_hull(2464, 3500.0, 3150.0, 180.0, 70.0, 150.0),
    weapon_period_ms: 4000,
    damage: Damage {
        em: 0.0,
        explosive: 0.0,
        kinetic: 20.0,
        thermal: 0.0,
    },
    damage_multiplier: 1.5,
    optimal: 2400.0,
    orbit_range: 800.0,
    cruise_fraction: 745.0 / 3150.0,
    falloff: 0.0,
    tracking: 0.0,
    sig_resolution: 0.0,
};

/// Warrior I, 2486: explosive.
pub const WARRIOR_I: DroneType = DroneType {
    type_id: 2486,
    name: "Warrior I",
    bandwidth: 5.0,
    hull: light_hull(2486, 4000.0, 4200.0, 100.0, 90.0, 130.0),
    weapon_period_ms: 4000,
    damage: Damage {
        em: 0.0,
        explosive: 20.0,
        kinetic: 0.0,
        thermal: 0.0,
    },
    damage_multiplier: 1.3,
    optimal: 2100.0,
    orbit_range: 700.0,
    cruise_fraction: 900.0 / 4200.0,
    falloff: 0.0,
    tracking: 0.0,
    sig_resolution: 0.0,
};

/// The four light drones' rows as they were hand-copied (W5).
///
/// **Retired by CP58e (O3, ruling 3)**: nothing in this module reads them;
/// every drone is [`find_in`]'s, from the loaded type data. They stay only
/// for [`hull`], which `destiny::hull::find_in` asks before the typedata
/// (X2b kept them: that chain is the `Content` one).
pub const TYPES: &[DroneType] = &[ACOLYTE_I, HOBGOBLIN_I, HORNET_I, WARRIOR_I];

/// A retired drone row's hull, which `destiny::hull::find_in` asks first.
/// New code asks [`hull_in`].
pub fn hull(type_id: i64) -> Option<&'static Hull> {
    TYPES.iter().find(|t| t.type_id == type_id).map(|t| &t.hull)
}

/// Drone attribute ids (`dogma/const`) **[V]**, beside the ones
/// `typedata::attr` and `damage::attr` already name.
pub mod attr {
    /// `speed`: the gun's cycle in ms, effect 10's `durationAttributeID`.
    pub const SPEED: i64 = 51;
    /// `maxRange`: effect 10's `rangeAttributeID`.
    pub const MAX_RANGE: i64 = 54;
    /// `entityFlyRange`: the orbit range.
    pub const ENTITY_FLY_RANGE: i64 = 416;
    /// `entityCruiseSpeed`.
    pub const ENTITY_CRUISE_SPEED: i64 = 508;
    /// `droneBandwidthUsed`.
    pub const DRONE_BANDWIDTH_USED: i64 = 1272;
}

/// `targetAttack`, effect 10 **[V]**: a combat drone's default effect (the
/// gun [`WEAPON_GUID`] draws). A drone type whose default effect is neither
/// this nor [`EFFECT_MINING`] (an ewar drone, a salvager) is not one this
/// server can fly.
pub const EFFECT_TARGET_ATTACK: i64 = 10;

/// `mining`, effect 17 **[V-data]**: a mining drone's default effect
/// (`crate::mining::EFFECT_MINING`). Its cycle is `durationAttributeID` 73,
/// its reach `rangeAttributeID` 54 (SOC-B10).
pub const EFFECT_MINING: i64 = crate::mining::EFFECT_MINING;

/// `duration` (73), ms: effect 17's `durationAttributeID` **[V-data]**, a
/// mining drone's cycle.
pub const ATTR_DURATION: i64 = 73;

/// Effect 17's `guid` in `dogmaeffects`, `'effects.Mining'` **[V-data]**:
/// the beam a mining drone's `OnSpecialFX` names.
pub const MINING_GUID: &str = "effects.Mining";

/// `[A]` The cruise fraction of a drone whose type has no
/// `entityCruiseSpeed` (508 defaults to 0 [V-data], and no mining drone
/// carries it): without one an orbit would be flown at no speed at all.
/// Falsified by mining drones that visibly race round their rock.
pub const CRUISE_FRACTION_DEFAULT: f64 = 0.5;

/// Whether `type_id` is a mining drone: a drone whose default effect is
/// [`EFFECT_MINING`] (SOC-B10).
pub fn is_miner(content: &Content, type_id: i64) -> bool {
    typedata::installed_in(content)
        .table()
        .get(type_id)
        .is_some_and(|r| {
            r.category_id == typedata::CATEGORY_DRONE && r.default_effect == Some(EFFECT_MINING)
        })
}

/// One drone type, from the loaded type data (CP58e, ruling 3): a
/// category-18 type whose default effect is [`EFFECT_TARGET_ATTACK`] or
/// [`EFFECT_MINING`], every number read off its own attributes (with the
/// attribute defaults the table carries). `None` for anything else.
///
/// **A mining drone** (SOC-B10) is the same record: its "gun" period is
/// its cycle ([`ATTR_DURATION`]), its optimal its `maxRange`, and its
/// damage the zero its row carries; [`is_miner`] tells the two apart.
pub fn find_in(content: &Content, type_id: i64) -> Option<DroneType> {
    let installed = typedata::installed_in(content);
    let table = installed.table();
    let row = table.get(type_id)?;
    let mining = row.default_effect == Some(EFFECT_MINING);
    if row.category_id != typedata::CATEGORY_DRONE
        || !(mining || row.default_effect == Some(EFFECT_TARGET_ATTACK))
    {
        return None;
    }
    let a = |id| table.attribute(type_id, id);
    let cruise = a(attr::ENTITY_CRUISE_SPEED);
    use crate::damage::attr as dmg;
    let max_velocity = a(typedata::attr::MAX_VELOCITY);
    Some(DroneType {
        type_id,
        name: installed.item(type_id).map_or("", |i| i.name),
        bandwidth: a(attr::DRONE_BANDWIDTH_USED),
        hull: row_hull(row, table),
        weapon_period_ms: a(if mining { ATTR_DURATION } else { attr::SPEED }) as i64,
        damage: Damage {
            em: a(dmg::EM_DAMAGE),
            explosive: a(dmg::EXPLOSIVE_DAMAGE),
            kinetic: a(dmg::KINETIC_DAMAGE),
            thermal: a(dmg::THERMAL_DAMAGE),
        },
        damage_multiplier: a(typedata::attr::DAMAGE_MULTIPLIER),
        optimal: a(attr::MAX_RANGE),
        orbit_range: orbit_range_of(a(attr::ENTITY_FLY_RANGE)),
        cruise_fraction: if max_velocity <= 0.0 {
            0.0
        } else if mining && cruise <= 0.0 {
            CRUISE_FRACTION_DEFAULT
        } else {
            cruise / max_velocity
        },
        falloff: a(crate::drone_skills::ATTR_FALLOFF),
        tracking: a(crate::drone_skills::ATTR_TRACKING),
        sig_resolution: a(crate::drone_skills::ATTR_SIG_RESOLUTION),
    })
}

/// A drone's ball numbers off its own type row: what
/// `typedata::Installed::build` serves to `destiny::hull::find_in`, so the
/// roster flies a drone at its own `maxVelocity` and the client is told the
/// same. **Without this every drone but the four hand-copied light ones
/// was the fallback hull, 1 m/s**: a Hulk's mining drones sat still in
/// space (DRONE-AI). A row with no mass (every mining drone) gets
/// [`MASS_DEFAULT`] for the same reason.
pub fn row_hull(row: &typedata::TypeRow, table: &typedata::Table) -> Hull {
    let a = |id| table.attribute(row.type_id, id);
    Hull {
        type_id: row.type_id,
        mass: if row.mass > 0.0 {
            row.mass
        } else {
            MASS_DEFAULT
        },
        radius: row.radius as f32,
        max_velocity: a(typedata::attr::MAX_VELOCITY) as f32,
        agility: match a(typedata::attr::AGILITY) as f32 {
            x if x > 0.0 => x,
            _ => AGILITY_DEFAULT,
        },
        shield_capacity: a(typedata::attr::SHIELD_CAPACITY),
        shield_recharge_rate: a(typedata::attr::SHIELD_RECHARGE),
        armor_hp: a(typedata::attr::ARMOR_HP),
        hp: a(typedata::attr::HP),
        signature_radius: a(typedata::attr::SIGNATURE_RADIUS),
        capacitor_capacity: a(typedata::attr::CAPACITOR_CAPACITY),
        capacitor_recharge_rate: a(typedata::attr::CAPACITOR_RECHARGE),
        ..DRONE_HULL_ASSUMED
    }
}

/// `[A]` the mass of a drone type whose row has none -- **every mining drone
/// has 0**, and mass 0 is a zero time constant, which the integrator reads
/// as "cannot move". 3,000 kg is the Hobgoblin I's [V-data]. Falsified by a
/// client capture of a mining drone's ball mass, or one that turns too slowly
/// or too sharply to look like a drone.
pub const MASS_DEFAULT: f64 = 3_000.0;

/// `[A]` the agility of a drone type with no attribute 70 -- **a mining
/// drone has none**, and agility 0 is a zero time constant, which the
/// integrator reads as "cannot move": the second reason the Hulks' drones sat
/// still. 100 is what all four light combat drones carry [V-data]
/// ([`light_hull`]). Falsified by a mining drone that turns too slowly or
/// too sharply to look like a drone.
pub const AGILITY_DEFAULT: f32 = 100.0;

/// `[A]` EvEJS's floor and ceiling for a drone's orbit range, metres
/// (`droneRuntime.js:198-199`; EvEJS is a hint, not a fact): a drone type
/// with no `entityFlyRange` (a mining drone) orbits at the floor, and none
/// orbits beyond the ceiling. Falsified by a client capture of a mining
/// drone's `Orbit` range, or by drones that look too close or too far.
pub const ORBIT_RANGE_DEFAULT: f64 = 500.0;
pub const ORBIT_RANGE_MAX: f64 = 5_000.0;

/// `entityFlyRange` as an orbit range: absent (0) is [`ORBIT_RANGE_DEFAULT`].
/// A range of 0 is the hulls touching, which on a mining drone left it on
/// top of its ship or rock.
fn orbit_range_of(fly_range: f64) -> f64 {
    if fly_range > 0.0 {
        fly_range.min(ORBIT_RANGE_MAX)
    } else {
        ORBIT_RANGE_DEFAULT
    }
}

/// [`find_in`]'s hull: the ball a launched drone is. For
/// `destiny::hull::find`'s `Content` chain (CP58b, X2) to fall through to.
pub fn hull_in(content: &Content, type_id: i64) -> Option<Hull> {
    find_in(content, type_id).map(|t| t.hull)
}

/// The hull numbers a drone's own attributes do not carry, all `[A]` and
/// all server-only: a drone holds no lock in this model, so no sweep reads
/// the targeting three, and drones do not warp. [`light_hull`]'s values.
const DRONE_HULL_ASSUMED: Hull = Hull {
    type_id: 0,
    mass: 0.0,
    radius: 0.0,
    max_velocity: 0.0,
    agility: 0.0,
    shield_capacity: 0.0,
    shield_recharge_rate: 0.0,
    armor_hp: 0.0,
    max_target_range: CONTROL_RANGE,
    max_locked_targets: 8.0,
    scan_resolution: 500.0,
    signature_radius: 0.0,
    warp_speed_au: 0.0,
    hp: 0.0,
    capacitor_capacity: 0.0,
    capacitor_recharge_rate: 0.0,
};

// ---------------------------------------------------------------------------
// The live table
// ---------------------------------------------------------------------------

/// What a drone was last told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Orbit the controller ship.
    Idle,
    /// Fight this ball.
    Engage(i64),
    /// Mine this rock, cycle after cycle (`CmdMineRepeatedly`, SOC-B10).
    Mine(i64),
    /// Come back aboard.
    ReturnBay,
}

/// **A standing escort order** (`CmdAssist`, `CmdGuard`): follow this
/// pilot's ship and fight for them. Independent of [`Order`]: an escorting
/// drone's order is `Engage(t)` while it fights and `Idle` while it waits, so
/// everything that reads "engaging this ball" (the NPCs' provocation rule)
/// keeps working; any command of the owner's own ends the escort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Escort {
    /// The pilot followed: the owner, or a fleetmate of the owner.
    pub character_id: i64,
    /// Guard (fight what attacks the pilot) or assist (fight what the pilot
    /// attacks).
    pub guard: bool,
}

impl Escort {
    /// The client's activity word (`droneEntry.py:150-157`) **[V]**.
    fn word(self) -> &'static str {
        if self.guard { "guard" } else { "assist" }
    }
}

/// What a drone is doing with its engines -- what the wire was last told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Stopped, or stopping.
    Stopped,
    /// Orbiting its controller at cruise speed.
    Home,
    /// Closing on its target at full speed.
    Approach,
    /// Orbiting its target at cruise speed.
    Orbit,
    /// Flying back to its controller to be scooped.
    Return,
}

/// One drone in space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drone {
    /// Its item id, ball id and roster key: one number.
    pub id: i64,
    /// Its type, as [`find_in`] read it at launch.
    pub kind: DroneType,
    /// The character who owns it, and whose volleys its are.
    pub owner_id: i64,
    /// The ship it was launched from: `controllerID` to the client.
    pub controller_id: i64,
    pub solar_system_id: i64,
    pub order: Order,
    pub phase: Phase,
    /// The ball its gun's `OnSpecialFX` is running on, if it is.
    pub firing: Option<i64>,
    /// When its next volley lands, on the blue clock.
    pub next_shot_at: BlueTime,
    /// The `activityState` and `targetID` the owner was last told.
    pub activity: i64,
    pub activity_target: Option<i64>,
    /// Since when its controller has not been in its system (docked,
    /// jumped, died): the clock [`ABANDON_AFTER`] runs on (K-DRONE).
    pub lost_since: Option<BlueTime>,
    /// The standing escort order, if any.
    pub escort: Option<Escort>,
    /// The escort the owner was last told (`OnDroneActivityChange`).
    pub told_escort: Option<Escort>,
}

impl Drone {
    /// A drone just out of the bay: told to orbit its ship, and nothing sent
    /// about its engines yet.
    pub fn launched(
        id: i64,
        kind: DroneType,
        owner_id: i64,
        controller_id: i64,
        solar_system_id: i64,
    ) -> Drone {
        Drone {
            id,
            kind,
            owner_id,
            controller_id,
            solar_system_id,
            order: Order::Idle,
            phase: Phase::Stopped,
            firing: None,
            next_shot_at: 0,
            activity: ENTITY_IDLE,
            activity_target: None,
            lost_since: None,
            escort: None,
            told_escort: None,
        }
    }

    /// The row the owner's client keeps for it ([`DroneRow`]).
    pub fn row(&self, controller_owner_id: i64) -> DroneRow {
        DroneRow {
            drone_id: self.id,
            owner_id: Some(self.owner_id),
            controller_id: Some(self.controller_id),
            activity: self.activity,
            type_id: self.kind.type_id,
            controller_owner_id: Some(controller_owner_id),
            target_id: self.activity_target,
        }
    }
}

/// Every drone in space.
///
/// **Kept inside `crate::modules::Modules`** (`Registry::modules().drones()`)
/// rather than in a field of its own, so this subsystem adds no field to the
/// shared `Registry`: a drone is the second weapon system, and the module
/// table is the world's weapon state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drones {
    live: BTreeMap<i64, Drone>,
    /// The orphan sweep has run ([`sweep_orphans`]).
    swept: bool,
    /// Each pilot's `ChangeDroneSettings` (K-DRONE); a pilot who never sent
    /// one has [`Settings::default`].
    settings: BTreeMap<i64, Settings>,
}

impl Drones {
    pub fn get(&self, id: i64) -> Option<&Drone> {
        self.live.get(&id)
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut Drone> {
        self.live.get_mut(&id)
    }

    pub fn insert(&mut self, drone: Drone) {
        self.live.insert(drone.id, drone);
    }

    pub fn remove(&mut self, id: i64) -> Option<Drone> {
        self.live.remove(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Drone> {
        self.live.values()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// A pilot's drone settings.
    pub fn settings_of(&self, character_id: i64) -> Settings {
        self.settings
            .get(&character_id)
            .copied()
            .unwrap_or_default()
    }

    /// The drones one ship controls.
    pub fn controlled_by(&self, ship_id: i64) -> impl Iterator<Item = &Drone> {
        self.live
            .values()
            .filter(move |d| d.controller_id == ship_id)
    }
}

/// Who flies a drone's roster entry, for `Registry::pilot`: the drone's own
/// type as the "ship", its owner as owner, and **no `char_id`** -- a drone
/// has no session, and `char_id` is the roster's "is anyone listening" test
/// (`npc::Pilot`). An NPC sees drones through `npc::view_of`'s own drone
/// branch, not through this.
pub fn pilot_of(w: &Registry, roster_key: i64) -> Option<crate::npc::Pilot> {
    let d = w.modules().drones().get(roster_key)?;
    let c = w.character(d.owner_id);
    Some(crate::npc::Pilot {
        ship_type_id: d.kind.type_id,
        owner_id: d.owner_id,
        corporation_id: c.map_or(0, |c| c.corporation_id),
        alliance_id: c.and_then(|c| c.alliance_id),
        char_id: None,
    })
}

/// A drone's ball: an ordinary free ball whose numbers come from its hull
/// through `destiny::hull::find_in`, which reads [`hull`] first, then the
/// world's typedata.
pub fn ball(w: &Registry, d: &Drone, motion: &Motion) -> destiny::Ball {
    let c = w.character(d.owner_id);
    destiny::ego_ball_in(
        w.content(),
        d.id,
        d.kind.type_id,
        c.and_then(|c| i32::try_from(c.corporation_id).ok())
            .unwrap_or(-1),
        c.and_then(|c| c.alliance_id)
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        motion,
    )
}

/// A drone's slim item.
///
/// - `ownerID` **the character**: `cdl.OnDroneStateChange2` and
///   `godma.OnDroneStateChange2` load the drone into the client's dogma only
///   when `slimItem.ownerID` is `session.charid` (`clientDogmaLocation.py:
///   1723`, `godma.py:2654`) **[V]**, and `michelle.AreDronesOut:619` asks
///   the same of the drone state **[V]**;
/// - `charID` `None`: the ball is not a pilot, and the owner is still primed
///   because `categoryDrone` is in `__categoryRequireOwnerPrime__`
///   (`michelle.py:1013`) **[V]**;
/// - `corpID` the owner's corporation.
pub fn slim(w: &Registry, d: &Drone) -> destiny::Slim {
    destiny::Slim {
        item_id: d.id,
        type_id: d.kind.type_id,
        owner_id: d.owner_id,
        char_id: None,
        corp_id: w.character(d.owner_id).map(|c| c.corporation_id),
        response_thresholds: None,
    }
}

// ---------------------------------------------------------------------------
// What the client is told
// ---------------------------------------------------------------------------

/// One drone as the client's `stateByDroneID` holds it.
///
/// **The same seven fields twice**: as `OnDroneStateChange`'s positional
/// arguments (`Michelle.OnDroneStateChange(droneID, ownerID, controllerID,
/// activityState, droneTypeID, controllerOwnerID, targetID)`,
/// `michelle.py:386-388`) and as one row of the bootstrap's `droneState`,
/// whose dict comprehension reads `d.droneID`, `d.ownerID`,
/// `d.controllerID`, `d.activityState`, `d.typeID`, `d.controllerOwnerID`
/// and `d.targetID` (`michelle.py:1228-1231`) **[V]**.
///
/// A row whose `ownerID` is not the pilot **and** whose `controllerID` is not
/// the pilot's ship makes the client delete its entry and scatter
/// `OnDroneControlLost` (`michelle.py:2028-2035`) **[V]** -- which is how a
/// drone scooped back aboard leaves the "in space" list: `None` for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DroneRow {
    pub drone_id: i64,
    pub owner_id: Option<i64>,
    pub controller_id: Option<i64>,
    pub activity: i64,
    pub type_id: i64,
    pub controller_owner_id: Option<i64>,
    pub target_id: Option<i64>,
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

impl DroneRow {
    /// The positional arguments of `OnDroneStateChange`.
    pub fn args(&self) -> Vec<PyValue> {
        vec![
            PyValue::Int(self.drone_id),
            opt(self.owner_id),
            opt(self.controller_id),
            PyValue::Int(self.activity),
            PyValue::Int(self.type_id),
            opt(self.controller_owner_id),
            opt(self.target_id),
        ]
    }

    /// One `droneState` entry: a `utillib.KeyVal`, read by attribute.
    /// `[A]` that TQ sends a `KeyVal` rather than a row: the comprehension
    /// reads attributes only, which both satisfy.
    pub fn to_pyvalue(&self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("droneID", PyValue::Int(self.drone_id)),
            ("ownerID", opt(self.owner_id)),
            ("controllerID", opt(self.controller_id)),
            ("activityState", PyValue::Int(self.activity)),
            ("typeID", PyValue::Int(self.type_id)),
            ("controllerOwnerID", opt(self.controller_owner_id)),
            ("targetID", opt(self.target_id)),
        ])
    }

    /// The row that tells the owner's client a drone is no longer theirs to
    /// list: back in the bay, or gone.
    pub fn gone(drone_id: i64, type_id: i64) -> DroneRow {
        DroneRow {
            drone_id,
            owner_id: None,
            controller_id: None,
            activity: ENTITY_IDLE,
            type_id,
            controller_owner_id: None,
            target_id: None,
        }
    }
}

/// `OnDroneStateChange(...)`, to the drone's owner. A plain notification,
/// not a Destiny entry: `OnDroneStateChange` is in michelle's
/// `__notifyevents__` (`michelle.py:154`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct StateChange {
    pub to: Vec<i64>,
    pub row: DroneRow,
}

impl Push for StateChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnDroneStateChange", self.row.args())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnDroneActivityChange(droneID, activityID, activity)`: which pilot a
/// drone assists or guards, or `(drone, None, None)` once it does neither.
/// The park keeps `(activity, activityID)` and deletes the entry when
/// `activity` is falsy (`michelle.py:1487-1493`); the drone window words it
/// "Assisting/Guarding <name>" (`droneEntry.py:150-157`) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityChange {
    pub to: Vec<i64>,
    pub drone: i64,
    pub escort: Option<Escort>,
}

impl Push for ActivityChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let (who, word) = match self.escort {
            Some(e) => (PyValue::Int(e.character_id), crate::pyutil::bstr(e.word())),
            None => (PyValue::None, PyValue::None),
        };
        (
            "OnDroneActivityChange",
            vec![PyValue::Int(self.drone), who, word],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Every drone row in a system, for the bootstrap's `droneState`.
pub fn rows_in(w: &Registry, solar_system_id: i64) -> Vec<DroneRow> {
    w.modules()
        .drones()
        .iter()
        .filter(|d| d.solar_system_id == solar_system_id)
        .map(|d| d.row(d.owner_id))
        .collect()
}

/// A turn's answer, or the clock: which queue a push goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// Inside a call: `Registry::push`, after the reply.
    Answer,
    /// On the world tick: `Registry::schedule`, nobody's turn.
    Clock,
}

fn send(w: &mut Registry, via: Via, p: impl Push + 'static) {
    match via {
        Via::Answer => w.push(p),
        Via::Clock => w.schedule(Duration::ZERO, p),
    }
}

/// Apply a plan and **hold its pushes** rather than queue them, so the
/// caller decides where they go and in what order. Whatever was already on
/// `pending` belongs to somebody else's turn and is put back untouched
/// (`combat::apply_on_the_clock`'s reasoning, "a house trap").
type Held = Vec<(Duration, Box<dyn Push>)>;

fn apply_held(w: &mut Registry, edits: &[Edit]) -> Option<(Vec<crate::item::Change>, Held)> {
    let before = w.take_pending();
    let applied = w.apply_items(edits);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    applied.ok().map(|c| (c, ours))
}

/// Send what [`apply_held`] held, to the owner.
fn release(w: &mut Registry, via: Via, owner: i64, held: Held) {
    for (_, p) in held {
        send(
            w,
            via,
            Addressed {
                to: Some(vec![owner]),
                inner: p,
            },
        );
    }
}

/// Everyone who should see a ball: the **characters** in a system.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// The gun's visual. **`module_id` is the drone's own ball id [V]**:
/// `Drone.FitHardpoints` keeps the turret it builds at
/// `self.modules[self.id]` (`Drone.py:109`), and `StandardWeapon` looks it
/// up as `shipBall.modules.get(self.moduleID)` (`turrets.py:96`) -- the NPC's
/// trap, the same answer.
///
/// A mining drone's beam (SOC-B10) is the same push with effect 17's
/// [`MINING_GUID`], not offensive **[A]** (the effect row's
/// `isOffensive` is 0 [V-data]; that the client draws a drone's mining
/// beam from this push is not read).
pub(crate) fn gun_fx(
    to: Vec<i64>,
    d: &Drone,
    target: i64,
    start: bool,
    mining: bool,
) -> crate::modules::SpecialFx {
    crate::modules::SpecialFx {
        to,
        ship_id: d.id,
        module_id: d.id,
        type_id: d.kind.type_id,
        target_id: Some(target),
        guid: if mining { MINING_GUID } else { WEAPON_GUID },
        is_offensive: !mining,
        start,
        duration_ms: d.kind.weapon_period_ms,
        repeat: 1000,
    }
}

fn fx(w: &mut Registry, via: Via, d: &Drone, target: i64, start: bool) {
    let to = audience(w, d.solar_system_id);
    if !to.is_empty() {
        let mining = is_miner(w.content(), d.kind.type_id);
        send(w, via, gun_fx(to, d, target, start, mining));
    }
}

/// Tell the owner whom a drone escorts, if that changed since last told.
fn tell_escort(w: &mut Registry, via: Via, id: i64) {
    let Some(d) = w.modules_mut().drones_mut().get_mut(id) else {
        return;
    };
    if d.escort == d.told_escort {
        return;
    }
    d.told_escort = d.escort;
    let push = ActivityChange {
        to: vec![d.owner_id],
        drone: id,
        escort: d.escort,
    };
    send(w, via, push);
}

/// Tell the owner a drone's state, if it changed, and remember it.
fn tell_state(w: &mut Registry, via: Via, id: i64, activity: i64, target: Option<i64>) {
    let Some(d) = w.modules_mut().drones_mut().get_mut(id) else {
        return;
    };
    if d.activity == activity && d.activity_target == target {
        return;
    }
    d.activity = activity;
    d.activity_target = target;
    let d = *d;
    send(
        w,
        via,
        StateChange {
            to: vec![d.owner_id],
            row: d.row(d.owner_id),
        },
    );
}

// ---------------------------------------------------------------------------
// The brain
// ---------------------------------------------------------------------------

/// Everything [`decide`] may know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub me: [f64; 3],
    /// The controller ship, if it is in this system.
    pub home: Option<[f64; 3]>,
    /// The ball being engaged, if there is an order to and it is still here.
    pub target: Option<[f64; 3]>,
    /// That ball's radius when ranges are measured to its surface -- a rock
    /// to mine (SOC-B10); 0 for a ship, whose ranges stay centre to centre.
    pub target_radius: f64,
    pub now: BlueTime,
    /// How far from the controller a fight may go, metres: the pilot's
    /// control range (`drone_skills::control_range`).
    pub control_range: f64,
    /// Whether an idle drone defends its ship on its own: the pilot's
    /// aggressive setting, and never a miner.
    pub aggressive: bool,
    /// What an aggressive idle drone would fight: the hostile nearest to
    /// it (or to the ship, with focus fire) that is attacking its ship or
    /// its drones, if any ([`threats_on`]).
    pub threat: Option<Threat>,
    /// For an escorting drone ([`Drone::escort`]): the ball to wait beside
    /// when there is nothing to fight (the escorted ship); `None` to wait
    /// beside the controller. Its fight, if any, is `threat`.
    pub orbit: Option<i64>,
}

/// A hostile ball an aggressive drone may take on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threat {
    pub id: i64,
    pub at: [f64; 3],
}

/// What a drone wants done. The executor in [`tick`] applies each.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intent {
    /// Close on a ball at full speed, to `range`.
    Follow {
        ball: i64,
        range: f64,
    },
    /// Circle a ball at cruise speed.
    Orbit(i64),
    Stop,
    FireStart(i64),
    FireStop(i64),
    /// One volley, landing at `at`.
    Shoot {
        target: i64,
        at: BlueTime,
    },
    /// Back aboard.
    Scoop,
    /// Tell the owner `(activityState, targetID)`.
    State {
        activity: i64,
        target: Option<i64>,
    },
}

/// **The whole policy**, pure: one drone, what it can see, and what it does
/// next.
///
/// - **No controller in the system** (it docked, jumped or died): gun off,
///   engines off, order forgotten. `[A]` the drone waits where it is; real
///   EVE leaves it in space too, reconnectable, which is not built.
/// - **Engage**: beyond the optimal range, approach; inside
///   [`ORBIT_ENTER_FRACTION`] of it, orbit; between, keep on. The gun runs
///   while the target is inside the optimal, one volley per period, each on
///   its own period boundary. A target that has left: back to idle.
/// - **Return to bay**: gun off, fly to the controller, and inside
///   [`SCOOP_RANGE`] be scooped.
/// - **Idle**: orbit the controller -- or, when the pilot's setting is
///   **aggressive** and something is attacking the ship or its drones, take
///   that on ([`View::threat`]); a fight farther than the control range
///   from the ship is let go.
///
/// `[A]`, each falsified only by "it does not feel like a drone": no
/// first-shot delay; no lock; what an aggressive drone counts as an attack
/// ([`threats_on`]). Whether a volley hits is `drone_skills::chance_to_hit`'s.
pub fn decide(drone: &Drone, view: &View) -> (Drone, Vec<Intent>) {
    let mut d = *drone;
    let mut out = Vec::new();
    let Some(home) = view.home else {
        gun_off(&mut d, &mut out);
        if d.phase != Phase::Stopped {
            out.push(Intent::Stop);
            d.phase = Phase::Stopped;
        }
        d.order = Order::Idle;
        state(&d, ENTITY_IDLE, None, &mut out);
        return (d, out);
    };
    // **An escort** fights what the tick picked for it (`threat`: the
    // escorted pilot's attackers, or their target), already within the
    // owner's reach, and waits beside the escorted ship otherwise.
    if d.escort.is_some() {
        if let Some(t) = view.threat {
            d.order = Order::Engage(t.id);
            engaged(&mut d, t.id, t.at, view, &mut out);
            return (d, out);
        }
        d.order = Order::Idle;
        gun_off(&mut d, &mut out);
        let beside = view.orbit.unwrap_or(d.controller_id);
        if d.phase != Phase::Home {
            out.push(Intent::Orbit(beside));
            d.phase = Phase::Home;
        }
        state(&d, ENTITY_IDLE, None, &mut out);
        return (d, out);
    }
    // **The leash**: a target the pilot could not have ordered the drone
    // onto (beyond the control range from the ship) is let go.
    if let (Order::Engage(_), Some(at)) = (d.order, view.target)
        && crate::space_state::distance(home, at) > view.control_range
    {
        gun_off(&mut d, &mut out);
        d.order = Order::Idle;
        d.phase = Phase::Stopped;
        state(&d, ENTITY_IDLE, None, &mut out);
        return (d, out);
    }
    // **Aggressive**: an idle drone fights what is attacking its ship.
    if d.order == Order::Idle
        && view.aggressive
        && let Some(t) = view.threat
    {
        d.order = Order::Engage(t.id);
        engaged(&mut d, t.id, t.at, view, &mut out);
        return (d, out);
    }
    match (d.order, view.target) {
        (Order::Engage(t) | Order::Mine(t), Some(at)) => {
            engaged(&mut d, t, at, view, &mut out);
            return (d, out);
        }
        (Order::Engage(_) | Order::Mine(_), None) => d.order = Order::Idle,
        (Order::ReturnBay, _) => {
            gun_off(&mut d, &mut out);
            if d.phase != Phase::Return {
                out.push(Intent::Follow {
                    ball: d.controller_id,
                    range: RETURN_FOLLOW_RANGE,
                });
                d.phase = Phase::Return;
            }
            state(&d, ENTITY_DEPARTING2, None, &mut out);
            if crate::space_state::distance(view.me, home) <= SCOOP_RANGE {
                out.push(Intent::Scoop);
            }
            return (d, out);
        }
        (Order::Idle, _) => {}
    }
    gun_off(&mut d, &mut out);
    if d.phase != Phase::Home {
        out.push(Intent::Orbit(d.controller_id));
        d.phase = Phase::Home;
    }
    state(&d, ENTITY_IDLE, None, &mut out);
    (d, out)
}

fn gun_off(d: &mut Drone, out: &mut Vec<Intent>) {
    if let Some(t) = d.firing.take() {
        out.push(Intent::FireStop(t));
    }
}

fn state(d: &Drone, activity: i64, target: Option<i64>, out: &mut Vec<Intent>) {
    if d.activity != activity || d.activity_target != target {
        out.push(Intent::State { activity, target });
    }
}

/// The engaged half of [`decide`]: movement, then the gun, then the state.
///
/// **Mining is the same flight** (SOC-B10): approach the rock, orbit it,
/// and inside the optimal (its `maxRange`, to the rock's surface) run the
/// beam, one [`Intent::Shoot`] per cycle -- which the executor turns into
/// a [`crate::mining::drone_cycle`]. A mining cycle's ore comes at its
/// **end** (the laser's rule, `crate::mining`), so the first lands one
/// period after the beam starts, not at once as a volley does.
fn engaged(d: &mut Drone, t: i64, at: [f64; 3], view: &View, out: &mut Vec<Intent>) {
    let k = d.kind;
    let mining = matches!(d.order, Order::Mine(_));
    let dist = crate::space_state::distance(view.me, at) - view.target_radius;
    let want = if dist > k.optimal {
        Phase::Approach
    } else if dist <= ORBIT_ENTER_FRACTION * k.optimal {
        Phase::Orbit
    } else if matches!(d.phase, Phase::Approach | Phase::Orbit) {
        d.phase
    } else {
        Phase::Approach
    };
    if want != d.phase {
        out.push(match want {
            Phase::Orbit => Intent::Orbit(t),
            _ => Intent::Follow {
                ball: t,
                range: k.orbit_range,
            },
        });
        d.phase = want;
    }
    if d.firing.is_some_and(|f| f != t) {
        gun_off(d, out);
    }
    if d.firing.is_none() && dist <= k.optimal {
        out.push(Intent::FireStart(t));
        d.firing = Some(t);
        d.next_shot_at = if mining {
            view.now.saturating_add(k.weapon_period().max(1))
        } else {
            view.now
        };
    } else if d.firing.is_some() && dist > k.optimal {
        gun_off(d, out);
    }
    if d.firing.is_some() {
        let period = k.weapon_period().max(1);
        let mut shots = 0;
        while view.now >= d.next_shot_at && shots < MAX_SHOTS_PER_TICK {
            out.push(Intent::Shoot {
                target: t,
                at: d.next_shot_at,
            });
            d.next_shot_at = d.next_shot_at.saturating_add(period);
            shots += 1;
        }
        if view.now >= d.next_shot_at {
            let behind = view.now - d.next_shot_at;
            d.next_shot_at += (behind / period + 1) * period;
        }
    }
    let activity = if mining {
        ENTITY_MINING
    } else if d.firing.is_some() {
        ENTITY_COMBAT
    } else {
        ENTITY_ENGAGE
    };
    state(d, activity, Some(t), out);
}

// ---------------------------------------------------------------------------
// The executor
// ---------------------------------------------------------------------------

/// **The world clock's hook** (called from `modules::tick`): forget the
/// drones that have left the roster, then one system at a time bring every
/// ball to the present, let each drone decide, and apply it. One system's
/// movement leaves as one `ActionUpdate`.
pub fn tick(w: &mut Registry, now: BlueTime) {
    if !w.modules().drones().swept {
        sweep_orphans(w);
    }
    if w.modules().drones().is_empty() {
        return;
    }
    let gone: Vec<i64> = w
        .modules()
        .drones()
        .iter()
        .filter(|d| w.roster().get(d.id).is_none())
        .map(|d| d.id)
        .collect();
    for id in gone {
        w.modules_mut().drones_mut().remove(id);
    }
    let mut systems: Vec<i64> = w
        .modules()
        .drones()
        .iter()
        .map(|d| d.solar_system_id)
        .collect();
    systems.sort_unstable();
    systems.dedup();
    for system in systems {
        tick_system(w, system, now);
    }
}

fn tick_system(w: &mut Registry, system: i64, now: BlueTime) {
    w.advance_system(system);
    let ids: Vec<i64> = w
        .modules()
        .drones()
        .iter()
        .filter(|d| d.solar_system_id == system)
        .map(|d| d.id)
        .collect();
    let mut actions = Vec::new();
    // What is attacking each controller, once a tick (K-DRONE).
    let mut threats: BTreeMap<i64, Vec<Threat>> = BTreeMap::new();
    let mut ranges: BTreeMap<i64, f64> = BTreeMap::new();
    // What each escorted pilot is attacking, once a tick.
    let mut attacking: BTreeMap<i64, Option<i64>> = BTreeMap::new();
    for id in ids {
        let Some(mut drone) = w.modules().drones().get(id).copied() else {
            continue;
        };
        reaim(w, &drone);
        let home = position_in(w, drone.controller_id, drone.solar_system_id);
        // **A controller that is not here** (docked, jumped, died, logged
        // off): the clock of [`ABANDON_AFTER`] runs, and a drone left too
        // long is taken out of space ([`abandon`]).
        match (home, drone.lost_since) {
            (None, None) => drone.lost_since = Some(now),
            (None, Some(since)) if now.saturating_sub(since) >= ABANDON_AFTER => {
                abandon(w, id);
                continue;
            }
            (Some(_), Some(_)) => drone.lost_since = None,
            _ => {}
        }
        // **An escort** follows a pilot who is here and still the owner or
        // the owner's fleetmate; otherwise the order lapses.
        let mut escort_pick_of = None;
        if let Some(e) = drone.escort {
            let ship = escort_subject(w, drone.owner_id, e.character_id)
                .ok()
                .filter(|_| {
                    w.roster()
                        .get(e.character_id)
                        .is_some_and(|p| p.solar_system_id == drone.solar_system_id)
                });
            match (ship, home) {
                (Some(ship), Some(home)) => {
                    let reach = *ranges.entry(drone.controller_id).or_insert_with(|| {
                        drone_skills::control_range(w, drone.owner_id, drone.controller_id)
                    });
                    let candidates: Vec<Threat> = if e.guard {
                        threats
                            .entry(ship)
                            .or_insert_with(|| {
                                threats_on(w, ship, e.character_id, drone.solar_system_id, now)
                            })
                            .clone()
                    } else {
                        let target = *attacking
                            .entry(ship)
                            .or_insert_with(|| assist_target(w, ship));
                        target
                            .and_then(|t| {
                                position_in(w, t, drone.solar_system_id)
                                    .map(|at| Threat { id: t, at })
                            })
                            .into_iter()
                            .collect()
                    };
                    let orbit = (ship != drone.controller_id).then_some(ship);
                    escort_pick_of = Some((
                        escort_pick(w, &drone, &candidates, home, reach),
                        orbit,
                        reach,
                    ));
                }
                (Some(ship), None) => {
                    escort_pick_of = Some((None, Some(ship), CONTROL_RANGE));
                }
                (None, _) => {
                    drone.escort = None;
                    if let Some(slot) = w.modules_mut().drones_mut().get_mut(id) {
                        slot.escort = None;
                    }
                    tell_escort(w, Via::Clock, id);
                }
            }
        }
        let settings = w.modules().drones().settings_of(drone.owner_id);
        let aggressive = settings.aggressive && !is_miner(w.content(), drone.kind.type_id);
        let threat = match (home, aggressive && drone.order == Order::Idle) {
            (Some(home), true) => {
                let list = threats.entry(drone.controller_id).or_insert_with(|| {
                    threats_on(
                        w,
                        drone.controller_id,
                        drone.owner_id,
                        drone.solar_system_id,
                        now,
                    )
                });
                let from = if settings.focus_fire {
                    home
                } else {
                    w.roster().get(id).map_or(home, |p| p.motion.position)
                };
                list.iter().copied().min_by(|a, b| {
                    crate::space_state::distance(from, a.at)
                        .total_cmp(&crate::space_state::distance(from, b.at))
                })
            }
            _ => None,
        };
        let Some(mut view) = view_of(w, &drone, now) else {
            continue;
        };
        view.aggressive = aggressive;
        view.threat = threat;
        if let Some((pick, orbit, reach)) = escort_pick_of {
            view.threat = pick;
            view.orbit = orbit;
            view.control_range = reach;
        }
        if matches!(drone.order, Order::Engage(_)) {
            view.control_range = *ranges.entry(drone.controller_id).or_insert_with(|| {
                drone_skills::control_range(w, drone.owner_id, drone.controller_id)
            });
        }
        let (next, intents) = decide(&drone, &view);
        if let Some(slot) = w.modules_mut().drones_mut().get_mut(id) {
            // The state fields are written by `tell_state`, which compares
            // against what was last *sent*.
            *slot = Drone {
                activity: slot.activity,
                activity_target: slot.activity_target,
                told_escort: slot.told_escort,
                ..next
            };
        }
        let mut mine = Vec::new();
        let mut scooped = false;
        for intent in intents {
            if intent == Intent::Scoop {
                scooped = scoop(w, Via::Clock, id).is_ok();
                continue;
            }
            apply(w, &next, intent, now, &mut mine);
        }
        // A ball scooped this tick is gone from every park by the time the
        // batch is read; an action naming it would be an error there.
        if !scooped {
            actions.extend(mine);
        }
    }
    let to = audience(w, system);
    if actions.is_empty() || to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    if let Some(update) = destiny::ActionUpdate::new(to, stamp, actions) {
        w.schedule(Duration::ZERO, update);
    }
}

/// **An approach is recorded as a goto at the target's current position,
/// re-aimed every tick**, while the wire says `FollowBall` -- `npc::reaim`'s
/// reason: `advance_character` places a `Follow` at its range from the
/// target on every read, which for a drone 15 km out is a teleport the
/// clients never see.
fn reaim(w: &mut Registry, d: &Drone) {
    let toward = match (d.phase, d.order) {
        (Phase::Approach, Order::Engage(t) | Order::Mine(t)) => t,
        (Phase::Return, _) => d.controller_id,
        _ => return,
    };
    // `ball_position`, not the roster: a rock is not in it (SOC-B10).
    let Some(at) = w.ball_position(toward) else {
        return;
    };
    if let Some(p) = w.roster_mut().get_mut(d.id) {
        p.motion.mode = Mode::Goto(at);
    }
}

/// The position of a ball in this system, if it is in the roster.
fn position_in(w: &Registry, ball: i64, system: i64) -> Option<[f64; 3]> {
    w.roster()
        .by_ship(ball)
        .filter(|p| p.solar_system_id == system)
        .map(|p| p.motion.position)
        // An anchorable unit (NW-2) is a target too, at its fixed place.
        .or_else(|| {
            w.combat()
                .wreck(ball)
                .filter(|wr| wr.solar_system_id == system && crate::anchor::is_unit(w, ball))
                .map(|wr| wr.position)
        })
}

/// A rock of the field in this system (SOC-B10).
fn rock_in(w: &Registry, rock: i64, system: i64) -> Option<crate::asteroid::Rock> {
    w.rocks()
        .get(rock)
        .copied()
        .filter(|r| r.solar_system_id == system)
}

fn view_of(w: &Registry, d: &Drone, now: BlueTime) -> Option<View> {
    let me = w.roster().get(d.id)?.motion.position;
    let (target, target_radius) = match d.order {
        Order::Engage(t) => (position_in(w, t, d.solar_system_id), 0.0),
        Order::Mine(t) => {
            rock_in(w, t, d.solar_system_id).map_or((None, 0.0), |r| (Some(r.position), r.radius))
        }
        _ => (None, 0.0),
    };
    Some(View {
        me,
        home: position_in(w, d.controller_id, d.solar_system_id),
        target,
        target_radius,
        now,
        control_range: CONTROL_RANGE,
        aggressive: false,
        threat: None,
        orbit: None,
    })
}

/// Orbit `target` at the drone's orbit range and cruise speed: the idle
/// flight, and the engaged one once on station (EvEJS: `Orbit` plus
/// `SetSpeedFraction`, `droneRuntime.js:3612-3630`).
fn orbit_home(id: i64, target: i64, k: &DroneType) -> Vec<Action> {
    vec![
        Action::Orbit {
            ball: id,
            target,
            range: k.orbit_range,
        },
        Action::SetSpeedFraction {
            ball: id,
            fraction: k.cruise_fraction,
        },
    ]
}

/// Apply one intent on the clock. `d` is the record after [`decide`].
fn apply(w: &mut Registry, d: &Drone, intent: Intent, now: BlueTime, actions: &mut Vec<Action>) {
    let id = d.id;
    let k = d.kind;
    let (wire, record): (Vec<Action>, Vec<Action>) = match intent {
        Intent::Follow { ball, range } => {
            let speed = Action::SetSpeedFraction {
                ball: id,
                fraction: 1.0,
            };
            let at = w.ball_position(ball);
            (
                vec![
                    Action::FollowBall {
                        ball: id,
                        target: ball,
                        range,
                    },
                    speed,
                ],
                at.map_or_else(Vec::new, |point| {
                    vec![Action::GotoPoint { ball: id, point }, speed]
                }),
            )
        }
        Intent::Orbit(t) => {
            let a = orbit_home(id, t, &k);
            (a.clone(), a)
        }
        Intent::Stop => {
            let a = vec![Action::Stop { ball: id }];
            (a.clone(), a)
        }
        Intent::FireStart(t) => {
            fx(w, Via::Clock, d, t, true);
            return;
        }
        Intent::FireStop(t) => {
            fx(w, Via::Clock, d, t, false);
            return;
        }
        Intent::Shoot { target, at } => {
            if matches!(d.order, Order::Mine(_)) {
                mine_cycle(w, d, target, at);
            } else {
                shoot(w, d, target, at, now);
            }
            return;
        }
        Intent::State { activity, target } => {
            tell_state(w, Via::Clock, id, activity, target);
            return;
        }
        Intent::Scoop => return,
    };
    if let Some(p) = w.roster_mut().get_mut(id) {
        for a in &record {
            p.motion.after(*a);
        }
    }
    actions.extend(wire);
}

/// **One mining cycle ended** (SOC-B10): the ore to the controller's hold
/// ([`crate::mining::drone_cycle`]). A hold with no room left -- before
/// the cut or after it -- stops the drone: beam off, order idle, so it
/// comes back to orbit its ship **[A]** (CCP's
/// `MiningDronesDeactivatedCargoHold*` keys say the drones stop; what they
/// do next is not read). A rock gone is the next tick's: [`decide`] sees
/// no target and idles.
fn mine_cycle(w: &mut Registry, d: &Drone, rock: i64, at: BlueTime) {
    let cut = crate::mining::drone_cycle(w, d.owner_id, d.controller_id, d.kind.type_id, rock, at);
    let full = matches!(
        cut,
        crate::mining::DroneCut::Full | crate::mining::DroneCut::Mined { full: true, .. }
    );
    if !full {
        return;
    }
    if let Some(t) = d.firing {
        fx(w, Via::Clock, d, t, false);
    }
    if let Some(r) = w.modules_mut().drones_mut().get_mut(d.id) {
        r.order = Order::Idle;
        r.phase = Phase::Stopped;
        r.firing = None;
    }
    tell_state(w, Via::Clock, d.id, ENTITY_IDLE, None);
}

/// One volley through `combat::fire`, **as the owner's**: `shooter` is the
/// owner's roster key, so the owner gets the combat-log line and is the
/// aggressor, while the shot itself names the drone as the attacking ball
/// and its weapon -- which is what the victim's log line and the bars show.
///
/// **Whether it hits** is `drone_skills`' turret formula on the drone's own
/// attributes and the two balls' motion ([`aim_of`]), rolled
/// deterministically from the drone and the volley's time; a miss goes
/// through `fire` as a miss (`hit_quality` 0), a wrecking hit as 6.
fn shoot(w: &mut Registry, d: &Drone, target: i64, at: BlueTime, now: BlueTime) {
    // An anchorable unit takes the raw volley, as other non-roster targets
    // (NW-2 **[A]**: no tracking or falloff roll for a fixed target).
    if w.roster().by_ship(target).is_none() && crate::anchor::is_unit(w, target) {
        if w.combat()
            .wreck(target)
            .is_some_and(|wr| wr.solar_system_id == d.solar_system_id)
        {
            let _ = crate::anchor::hit(w, target, d.kind.volley().total(), now);
        }
        return;
    }
    let Some(victim) = w.roster().by_ship(target).copied() else {
        return;
    };
    if victim.solar_system_id != d.solar_system_id {
        return;
    }
    let verdict = match aim_of(w, d, &victim) {
        Some(aim) if d.kind.tracking > 0.0 => drone_skills::verdict(
            drone_skills::chance_to_hit(&d.kind, aim),
            drone_skills::roll(d.id, at, 1),
            drone_skills::roll(d.id, at, 2),
        ),
        _ => drone_skills::Verdict {
            quality: crate::combat::HIT_QUALITY,
            multiplier: 1.0,
        },
    };
    let shot = Shot {
        at,
        attacker_ship_id: d.id,
        weapon_item_id: d.id,
        weapon_type_id: d.kind.type_id,
        target_ship_id: target,
        damage: drone_skills::scaled(d.kind.volley(), verdict.multiplier),
        hit_quality: verdict.quality,
    };
    crate::combat::fire(w, shot, d.owner_id, victim, now);
}

/// How the target looks from the drone: distance, the target's speed across
/// the line of sight relative to the drone, and its signature radius.
pub fn aim_of(w: &Registry, d: &Drone, victim: &Presence) -> Option<drone_skills::Aim> {
    let me = w.roster().get(d.id)?;
    let signature = crate::dogma::hull(w, victim.ship_id)
        .map(|h| h.signature_radius)
        .or_else(|| {
            let ty = w.pilot(victim.character_id)?.ship_type_id;
            crate::destiny::hull::find_in(w.content(), ty).map(|h| h.signature_radius)
        })
        .unwrap_or(crate::destiny::hull::FALLBACK.signature_radius);
    Some(drone_skills::Aim {
        distance: crate::space_state::distance(me.motion.position, victim.motion.position),
        transversal: drone_skills::transversal(
            me.motion.position,
            me.motion.velocity,
            victim.motion.position,
            victim.motion.velocity,
        ),
        signature,
    })
}

// ---------------------------------------------------------------------------
// Docked: the bay
// ---------------------------------------------------------------------------

/// One unit's m3: the loaded type data's, then the rest of the content's
/// item lookup (`crate::item::find_in`: the hand rows, PI's types, the
/// skillbooks).
fn volume_of(w: &Registry, type_id: i64) -> f64 {
    typedata::item_in(w.content(), type_id)
        .or_else(|| crate::item::find_in(w.content(), type_id))
        .map_or(0.0, |t| t.volume)
}

/// Free m3 in a ship's drone bay.
pub fn bay_free(w: &Registry, ship_id: i64) -> f64 {
    let capacity = drone_skills::bay_of(w, ship_id).map_or(0.0, |b| b.capacity);
    (capacity - bay_used(w, ship_id)).max(0.0)
}

/// The m3 in a ship's drone bay, each stack at [`volume_of`]: the store's
/// own `used_volume` knows only the hand rows until CP58b.
pub fn bay_used(w: &Registry, ship_id: i64) -> f64 {
    w.store()
        .contents_of(ship_id)
        .filter(|i| i.flag_id == FLAG_DRONE_BAY)
        .map(|i| volume_of(w, i.type_id) * i.quantity as f64)
        .sum()
}

/// `NotEnoughDroneBaySpace` (FSD 1342, loc args `available`, `volume`)
/// **[V]** key; it is in `invControllers._AddItem`'s refusal tuple
/// (`invControllers.py:323`), so the client recomputes the room itself and
/// re-prompts with the most that fits, as it does for the cargo hold. `[A]`
/// the args' meaning: free m3 and the m3 asked for (`fitting::Refusal::NoRoom`).
fn no_room(available: f64, volume: f64) -> ServiceError {
    crate::modules::user_error(
        "NotEnoughDroneBaySpace",
        vec![
            ("available", PyValue::Real(available)),
            ("volume", PyValue::Real(volume)),
        ],
    )
}

fn notify(text: &str) -> ServiceError {
    crate::modules::user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// One of my ships, assembled, in this station's hangar.
fn parked_here(w: &Registry, owner: i64, station_id: i64, ship_id: i64) -> bool {
    w.store().get(ship_id).is_some_and(|s| {
        s.owner_id == owner
            && s.category_id == crate::model::CATEGORY_SHIP
            && s.singleton
            && s.location_id == station_id
            && s.flag_id == crate::model::FLAG_HANGAR
    })
}

/// **Into a drone bay**, docked: `Add(itemID, sourceID, qty=n, flag=87)` on
/// the ship's inventory -- the drone bay window is `invCtrl.ShipDroneBay`,
/// whose `locationFlag` is `flagDroneBay` (`invControllers.py:805-809`) and
/// whose drops go through the same `_AddItem` as the cargo hold's **[V]**.
///
/// What may go in, `[A]` beyond the keys: a **drone** (category 18; anything
/// else is `ItemCannotBeInDroneBay`, FSD 992, no args), from the station
/// hangar or this ship's cargo hold, into my active ship or another of mine
/// parked here, and only while docked; a split inside the bay is a split.
/// The room is the bay's `droneCapacity` (attribute 283) minus what is in it.
pub fn to_bay(
    w: &mut Registry,
    actor: Option<i64>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    let plan = plan_to_bay(w, actor, ship_id, item_id, quantity)?;
    if plan.is_empty() {
        return Ok(());
    }
    w.apply_items(&plan)
        .map(|_| ())
        .map_err(|e| notify(&format!("That move could not be made: {e}")))
}

/// [`to_bay`]'s decision, pure.
pub fn plan_to_bay(
    w: &Registry,
    actor: Option<i64>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, ServiceError> {
    let c = actor
        .and_then(|a| w.character(a))
        .ok_or_else(|| notify("No character."))?;
    let owner = c.character_id;
    let station = w
        .docked_station(owner)
        .ok_or_else(|| notify("Drones can only be loaded while docked."))?;
    if ship_id != c.ship_id && !parked_here(w, owner, station, ship_id) {
        return Err(notify(
            "Only your own ship's drone bay can take drones here.",
        ));
    }
    let item = w
        .store()
        .get(item_id)
        .cloned()
        .ok_or_else(|| notify("That item is not here."))?;
    if item.owner_id != owner || crate::item::is_loaded_charge(&item) {
        return Err(notify("That item is not yours to move."));
    }
    if item.category_id != CATEGORY_DRONE {
        return Err(crate::modules::user_error("ItemCannotBeInDroneBay", vec![]));
    }
    let to = Slot::new(owner, ship_id, FLAG_DRONE_BAY);
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    if item.slot() == to {
        return Ok(part.map_or_else(Vec::new, |q| {
            vec![Edit::Split {
                item_id,
                quantity: q,
                to,
            }]
        }));
    }
    let from_hangar = item.location_id == station && item.flag_id == crate::model::FLAG_HANGAR;
    let from_cargo = item.location_id == ship_id && item.flag_id == crate::fitting::FLAG_CARGO;
    if !from_hangar && !from_cargo {
        return Err(notify(
            "Drones go into the bay from the hangar or the cargo hold.",
        ));
    }
    let n = part.unwrap_or(item.quantity);
    let unit = volume_of(w, item.type_id);
    let free = bay_free(w, ship_id);
    if crate::fitting::units_that_fit(free, unit) < n {
        return Err(no_room(free, unit * n as f64));
    }
    Ok(match part {
        Some(q) => vec![Edit::Split {
            item_id,
            quantity: q,
            to,
        }],
        None => vec![Edit::Move { item_id, to }],
    })
}

/// Whether an item sits in a drone bay: what routes a drop on the hangar
/// here rather than to `fitting::plan_unfit`, which knows fitting slots and
/// the cargo hold only.
pub fn in_a_bay(w: &Registry, item_id: i64) -> bool {
    w.store()
        .get(item_id)
        .is_some_and(|i| i.flag_id == FLAG_DRONE_BAY)
}

/// **Out of a drone bay into the station hangar**, docked: `Add(itemID,
/// shipID, qty=n, flag=4)` on the hangar, the same call an unfit is
/// (`fittingSlotController.py:235`, P5.12's table) **[V]**. From my active
/// ship's bay or a parked one's; a part of a stack is a split.
pub fn to_hangar(
    w: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    let plan = plan_to_hangar(w, actor, item_id, quantity)?;
    w.apply_items(&plan)
        .map(|_| ())
        .map_err(|e| notify(&format!("That move could not be made: {e}")))
}

/// [`to_hangar`]'s decision, pure.
pub fn plan_to_hangar(
    w: &Registry,
    actor: Option<i64>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, ServiceError> {
    let c = actor
        .and_then(|a| w.character(a))
        .ok_or_else(|| notify("No character."))?;
    let owner = c.character_id;
    let station = w
        .docked_station(owner)
        .ok_or_else(|| notify("Drones can only be unloaded while docked."))?;
    let item = w
        .store()
        .get(item_id)
        .cloned()
        .ok_or_else(|| notify("That item is not here."))?;
    let ship = item.location_id;
    if item.owner_id != owner
        || item.flag_id != FLAG_DRONE_BAY
        || (ship != c.ship_id && !parked_here(w, owner, station, ship))
    {
        return Err(notify("That item is not in one of your drone bays here."));
    }
    let to = Slot::new(owner, station, crate::model::FLAG_HANGAR);
    Ok(match quantity {
        Some(q) if !item.singleton && q > 0 && q < item.quantity => vec![Edit::Split {
            item_id,
            quantity: q,
            to,
        }],
        _ => vec![Edit::Move { item_id, to }],
    })
}

// ---------------------------------------------------------------------------
// In space: launch, orders, scoop
// ---------------------------------------------------------------------------

/// The drones this pilot may have out at once: the Drones skill's level
/// ([`SKILL_DRONES`]).
pub fn max_active(w: &Registry, character_id: i64) -> i64 {
    drone_skills::max_active(w, character_id)
}

/// One refusal in a multi-drone answer: `(label, {args})`, which the client
/// raises as `UserError(label, args)` (`eveMisc.LaunchFromShip:97-109`,
/// `droneFunctions.HandleMultipleCallError:29`, `:41`) **[V]**.
fn refusal(label: &str, args: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr(label),
        PyValue::Dict(crate::pyutil::pypairs(args)),
    ])
}

fn refusal_text(text: &str) -> PyValue {
    refusal("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// `ship.LaunchDrones([(itemID, qty), ...], whoseBehalfID, ignoreWarning)`,
/// on `Moniker('ship', (solarsystemid, groupSolarSystem))`
/// (`eveMisc.LaunchFromShip:79`, `eveMoniker.GetShipAccess:76-78`) **[V]**.
///
/// **Answers `{itemID: [entry, ...]}`**: each entry is a launched drone's
/// new id or a `(label, args)` refusal; `LaunchFromShip` sorts them by type
/// (`:94-100`), scatters `OnItemLaunch` with the ids and raises each
/// refusal on its own tasklet (`:101-109`) **[V]**.
///
/// One unit at a time, `[A]` in the order sent: the first that cannot go
/// stops its stack (the rest would be refused for the same reason). Refused:
/// not the pilot's own bay; not a drone this server can fly; past the Drones
/// skill's count (`NoDroneManagementAbilities` with no skill,
/// `NoDroneManagementAbilitiesLeft` at the limit); past the hull's bandwidth
/// (`MaxBandwidthExceeded`) -- keys **[V]** FSD, raise sites and arg
/// meanings `[A]`. `[A]` no required-skill check beyond the count: a pilot
/// with Drones I flies a Hobgoblin without Light Drone Operation.
pub fn launch(w: &mut Registry, actor: Option<i64>, items: &[(i64, i64)]) -> PyValue {
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    let mut entry =
        |item_id: i64, v: Vec<PyValue>| answer.push((PyValue::List(v), PyValue::Int(item_id)));
    let Some(c) = actor.and_then(|a| w.character(a)).cloned() else {
        for (id, _) in items {
            entry(*id, vec![refusal_text("No character.")]);
        }
        return PyValue::Dict(answer);
    };
    let owner = c.character_id;
    w.advance_character(owner);
    let here = w.roster().get(owner).copied();
    let Some(here) = here.filter(|p| p.ship_id == c.ship_id) else {
        for (id, _) in items {
            entry(
                *id,
                vec![refusal_text("Drones can only be launched in space.")],
            );
        }
        return PyValue::Dict(answer);
    };
    let ship_id = c.ship_id;
    let bandwidth = drone_skills::bay_of(w, ship_id).map_or(0.0, |b| b.bandwidth);
    let limit = max_active(w, owner);
    let ship_radius = typedata::hull_in(w.content(), c.ship_type_id)
        .or_else(|| crate::destiny::hull::find_in(w.content(), c.ship_type_id))
        .map_or(50.0, |h| f64::from(h.radius));
    let mut launched: Vec<i64> = Vec::new();
    let mut held: Held = Vec::new();
    for &(item_id, qty) in items {
        let mut out = Vec::new();
        for _ in 0..qty.max(1) {
            let out_now: Vec<&Drone> = w.modules().drones().controlled_by(ship_id).collect();
            let count = out_now.len() as i64;
            let used: f64 = out_now.iter().map(|d| d.kind.bandwidth).sum();
            let Some(item) = w
                .store()
                .get(item_id)
                .cloned()
                .filter(|i| i.slot() == Slot::new(owner, ship_id, FLAG_DRONE_BAY))
            else {
                if out.is_empty() {
                    out.push(refusal_text("That drone is not in your drone bay."));
                }
                break;
            };
            let Some(kind) = find_in(w.content(), item.type_id) else {
                out.push(refusal_text(
                    "This server has no flight data for that drone.",
                ));
                break;
            };
            // `[A]` the skills the drone's type requires (Light Drone
            // Operation for a light drone, ...): the client's launch asks
            // nothing, but a pilot without them could not have fitted it.
            if let Some((skill, level)) = drone_skills::missing_skill(w, owner, kind.type_id) {
                let skill_name =
                    typedata::item_in(w.content(), skill).map_or("a skill", |t| t.name);
                out.push(refusal_text(&format!(
                    "You need {skill_name} {level} to launch the {}.",
                    kind.name
                )));
                break;
            }
            if limit <= 0 {
                out.push(refusal(
                    "NoDroneManagementAbilities",
                    vec![(
                        "typeID",
                        crate::modules::type_arg_for(
                            "NoDroneManagementAbilities",
                            "typeID",
                            kind.type_id,
                        ),
                    )],
                ));
                break;
            }
            if count >= limit {
                out.push(refusal(
                    "NoDroneManagementAbilitiesLeft",
                    vec![
                        (
                            "item",
                            crate::modules::type_arg_for(
                                "NoDroneManagementAbilitiesLeft",
                                "item",
                                kind.type_id,
                            ),
                        ),
                        ("limit", PyValue::Int(limit)),
                    ],
                ));
                break;
            }
            if used + kind.bandwidth > bandwidth + 1e-9 {
                out.push(refusal(
                    "MaxBandwidthExceeded",
                    vec![
                        ("bandwidthNeeded", PyValue::Real(kind.bandwidth)),
                        ("droneBandwidthUsed", PyValue::Real(used)),
                        ("droneName", crate::modules::type_arg(kind.type_id)),
                    ],
                ));
                break;
            }
            // Launched with this pilot's skills and this hull's bonuses
            // (`drone_skills::stats`), fixed until it is scooped.
            let kind = drone_skills::stats(w, ship_id, &kind);
            let to = Slot::new(owner, here.solar_system_id, FLAG_IN_SPACE);
            let edit = if item.quantity > 1 {
                Edit::Split {
                    item_id,
                    quantity: 1,
                    to,
                }
            } else {
                Edit::Move { item_id, to }
            };
            let Some((changes, pushes)) = apply_held(w, &[edit]) else {
                out.push(refusal_text("That drone could not be launched."));
                break;
            };
            held.extend(pushes);
            let drone_id = changes
                .iter()
                .find_map(|ch| match ch {
                    crate::item::Change::Created { item_id, .. } => Some(*item_id),
                    _ => None,
                })
                .unwrap_or(item_id);
            // Round the ship, one slot per drone it has out. `[A]`.
            let angle = std::f64::consts::TAU * (count as f64) / 5.0;
            let r = ship_radius + LAUNCH_CLEARANCE;
            let at = here.motion.position;
            let position = [at[0] + r * angle.cos(), at[1], at[2] + r * angle.sin()];
            w.roster_mut().arrive(Presence::arriving(
                drone_id,
                drone_id,
                here.solar_system_id,
                Motion::arrived(position, [0.0; 3]),
            ));
            // **Out of the bay already orbiting its ship**, in the ball the
            // client is given. `[A]` EvEJS does this (`droneRuntime.js:
            // 5006-5011`, `:5067`); falsified by a client that mishandles an
            // Orbit-mode AddBalls2 ball (the drone would then wait for the
            // tick's `Orbit` call, which is still sent if the phase is reset).
            if let Some(p) = w.roster_mut().get_mut(drone_id) {
                for a in orbit_home(drone_id, ship_id, &kind) {
                    p.motion.after(a);
                }
            }
            let mut fresh = Drone::launched(drone_id, kind, owner, ship_id, here.solar_system_id);
            fresh.phase = Phase::Home;
            w.modules_mut().drones_mut().insert(fresh);
            seed_damage(w, drone_id);
            launched.push(drone_id);
            out.push(PyValue::Int(drone_id));
            if item.quantity <= 1 {
                break;
            }
        }
        entry(item_id, out);
    }
    announce_launch(w, owner, here.solar_system_id, &launched);
    release(w, Via::Answer, owner, held);
    PyValue::Dict(answer)
}

/// The news of a launch, in the order the client needs it: the balls (with
/// their slims), then each drone's state to its owner -- so the window that
/// reloads on the item move finds them in `stateByDroneID` -- and the item
/// rows last (the caller's [`release`]).
fn announce_launch(w: &mut Registry, owner: i64, system: i64, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let now = crate::pyutil::filetime_now();
    let mut balls = Vec::new();
    let mut extra = Vec::new();
    let mut rows = Vec::new();
    for id in ids {
        let (Some(d), Some(p)) = (
            w.modules().drones().get(*id).copied(),
            w.roster().get(*id).copied(),
        ) else {
            continue;
        };
        balls.push(ball(w, &d, &p.motion));
        extra.push((
            slim(w, &d),
            Some(crate::combat::damage_state(w, *id, now).to_pyvalue()),
        ));
        rows.push(d.row(owner));
    }
    let to = audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(added) = destiny::BallsAdded::new(
        to,
        stamp,
        destiny::Park {
            current_time: stamp,
            balls,
        },
        extra,
    ) {
        w.push(added);
    }
    for row in rows {
        w.push(StateChange {
            to: vec![owner],
            row,
        });
    }
}

/// A drone this pilot may command: theirs, and flown from their ship.
fn commandable(w: &Registry, actor: i64, drone_id: i64) -> Result<Drone, PyValue> {
    let ship = w.character(actor).map(|c| c.ship_id);
    w.modules()
        .drones()
        .get(drone_id)
        .copied()
        .filter(|d| d.owner_id == actor && Some(d.controller_id) == ship)
        .ok_or_else(|| refusal_text("That drone is not under your control."))
}

/// `{droneID: (label, args)}` for the refusals, `{}` when all went: the
/// shape `HandleMultipleCallError` reads -- `if not len(ret): return`
/// (`droneFunctions.py:24-25`) **[V]**.
fn errors(refused: Vec<(i64, PyValue)>) -> PyValue {
    PyValue::Dict(
        refused
            .into_iter()
            .map(|(id, e)| (e, PyValue::Int(id)))
            .collect(),
    )
}

/// `entity.CmdEngage(droneIDs, targetID)` (`droneFunctions.EngageTarget:109`)
/// **[V]**: the target is the pilot's active target, checked by the client
/// (`:82-84`). Refused, `[A]` in our words: not the pilot's drone; a target
/// not in this system; a target beyond the pilot's drone control range
/// ([`CONTROL_RANGE`]) from the ship.
pub fn engage(w: &mut Registry, actor: Option<i64>, ids: &[i64], target: i64) -> PyValue {
    let Some(actor) = actor else {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal_text("No character.")))
                .collect(),
        );
    };
    w.advance_character(actor);
    let ship = w.roster().get(actor).copied();
    let target_at = ship.and_then(|s| position_in(w, target, s.solar_system_id));
    // CW-1: an attack the pilot's safety forbids, every drone refused in the
    // client's own words (`crate::crimewatch::attack_refusal`).
    let drone_type = ids
        .first()
        .and_then(|id| w.modules().drones().get(*id))
        .map(|d| d.kind.type_id);
    if let Some(r) = crate::crimewatch::attack_refusal(
        w,
        actor,
        drone_type,
        target,
        crate::pyutil::filetime_now(),
    ) {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal(r.key, r.args.clone())))
                .collect(),
        );
    }
    let mut refused = Vec::new();
    for &id in ids {
        let d = match commandable(w, actor, id) {
            Ok(d) => d,
            Err(e) => {
                refused.push((id, e));
                continue;
            }
        };
        let (Some(s), Some(at)) = (ship, target_at) else {
            refused.push((id, refusal_text("Your drones cannot see that target.")));
            continue;
        };
        if target == d.controller_id || target == id {
            refused.push((id, refusal_text("Drones do not attack their own ship.")));
            continue;
        }
        if is_miner(w.content(), d.kind.type_id) {
            refused.push((id, refusal_text("Mining drones do not attack.")));
            continue;
        }
        if crate::space_state::distance(s.motion.position, at) > CONTROL_RANGE {
            refused.push((
                id,
                refusal_text("That target is beyond your drone control range."),
            ));
            continue;
        }
        if let Some(t) = d.firing.filter(|t| *t != target) {
            fx(w, Via::Answer, &d, t, false);
            if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
                r.firing = None;
            }
        }
        if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
            // A new target is a new flight: whatever the engines were doing
            // was about another ball, so the next tick orders afresh.
            if r.order != Order::Engage(target) {
                r.phase = Phase::Stopped;
            }
            r.order = Order::Engage(target);
            r.escort = None;
        }
        tell_escort(w, Via::Answer, id);
        tell_state(w, Via::Answer, id, ENTITY_ENGAGE, Some(target));
    }
    errors(refused)
}

/// `entity.CmdMineRepeatedly(droneIDs, targetID)`
/// (`droneFunctions.MineRepeatedly:175`) **[V]**: the target is the pilot's
/// active target, checked by the client (`:170-172`, else
/// `DroneCommandRequiresActiveTarget`); the answer goes to
/// `HandleMultipleCallError` with `'MultiDroneCmdResult'` (`:176`) [V], so
/// the shape is [`engage`]'s. Refused, `[A]` in our words: not the pilot's
/// drone; not a mining drone ([`is_miner`]); a target that is not a rock
/// (or cloud) in this system; a rock whose surface is beyond the pilot's
/// drone control range ([`CONTROL_RANGE`]) from the ship. A gas cloud is
/// accepted here and cuts nothing (`crate::mining::cuts`).
pub fn mine(w: &mut Registry, actor: Option<i64>, ids: &[i64], target: i64) -> PyValue {
    let Some(actor) = actor else {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal_text("No character.")))
                .collect(),
        );
    };
    w.advance_character(actor);
    let ship = w.roster().get(actor).copied();
    let rock = ship.and_then(|s| rock_in(w, target, s.solar_system_id));
    let mut refused = Vec::new();
    for &id in ids {
        let d = match commandable(w, actor, id) {
            Ok(d) => d,
            Err(e) => {
                refused.push((id, e));
                continue;
            }
        };
        if !is_miner(w.content(), d.kind.type_id) {
            refused.push((id, refusal_text("That drone cannot mine.")));
            continue;
        }
        let (Some(s), Some(r)) = (ship, rock) else {
            refused.push((id, refusal_text("Your drones can only mine asteroids.")));
            continue;
        };
        if crate::space_state::distance(s.motion.position, r.position) - r.radius > CONTROL_RANGE {
            refused.push((
                id,
                refusal_text("That asteroid is beyond your drone control range."),
            ));
            continue;
        }
        if let Some(t) = d.firing.filter(|t| *t != target) {
            fx(w, Via::Answer, &d, t, false);
            if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
                r.firing = None;
            }
        }
        if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
            if r.order != Order::Mine(target) {
                r.phase = Phase::Stopped;
            }
            r.order = Order::Mine(target);
            r.escort = None;
        }
        tell_escort(w, Via::Answer, id);
        tell_state(w, Via::Answer, id, ENTITY_MINING, Some(target));
    }
    errors(refused)
}

/// `entity.CmdReturnBay(droneIDs)` (`droneFunctions.ReturnToDroneBay:207`)
/// **[V]**: come home and be scooped. The fight stops now; the flight is the
/// next tick's.
pub fn return_bay(w: &mut Registry, actor: Option<i64>, ids: &[i64]) -> PyValue {
    reorder(w, actor, ids, Order::ReturnBay, ENTITY_DEPARTING2)
}

/// `entity.CmdReturnHome(droneIDs)` ("Return and Orbit",
/// `droneFunctions.ReturnAndOrbit:200`) **[V]**: stop fighting, orbit the
/// ship. `[A]` `entityDeparting` is the state shown while it flies back --
/// the tick turns it to idle once the orbit is ordered.
pub fn return_home(w: &mut Registry, actor: Option<i64>, ids: &[i64]) -> PyValue {
    reorder(w, actor, ids, Order::Idle, ENTITY_DEPARTING)
}

fn reorder(
    w: &mut Registry,
    actor: Option<i64>,
    ids: &[i64],
    order: Order,
    activity: i64,
) -> PyValue {
    let Some(actor) = actor else {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal_text("No character.")))
                .collect(),
        );
    };
    let mut refused = Vec::new();
    for &id in ids {
        let d = match commandable(w, actor, id) {
            Ok(d) => d,
            Err(e) => {
                refused.push((id, e));
                continue;
            }
        };
        if let Some(t) = d.firing {
            fx(w, Via::Answer, &d, t, false);
        }
        if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
            r.firing = None;
            r.order = order;
            r.escort = None;
            // Re-issue the movement on the next tick even if the phase name
            // would say it is already under way (an orbit of the *target*).
            if r.phase != Phase::Return {
                r.phase = Phase::Stopped;
            }
        }
        tell_escort(w, Via::Answer, id);
        tell_state(w, Via::Answer, id, activity, None);
    }
    errors(refused)
}

/// The most drones that may assist one pilot, all owners together
/// (`appConst.maxDroneAssist`) **[V]**.
pub const MAX_DRONE_ASSIST: usize = 20;

/// Why a pilot cannot be escorted by `actor`'s drones, in the client's own
/// keys (it refuses first by itself, `droneFunctions.py:98-140`; this is the
/// backstop): the pilot must be the actor or a fleetmate, in a ship that is
/// not a capsule, in the actor's system. `Ok` is the pilot's ship.
fn escort_subject(w: &Registry, actor: i64, subject: i64) -> Result<i64, PyValue> {
    let member = subject == actor
        || w.fleets()
            .fleet_of(actor)
            .is_some_and(|f| f.members.contains_key(&subject));
    if !member {
        return Err(refusal(
            "DroneCommandRequiresShipPilotedFleetMember",
            vec![],
        ));
    }
    let here = w.roster().get(actor).map(|p| p.solar_system_id);
    let Some(p) = w
        .roster()
        .get(subject)
        .filter(|p| Some(p.solar_system_id) == here)
        .filter(|_| w.character(subject).is_some())
    else {
        return Err(refusal(
            "DroneCommandRequiresShipPilotedFleetMember",
            vec![],
        ));
    };
    if w.pilot(subject)
        .is_some_and(|k| k.ship_type_id == crate::damage::CAPSULE_TYPE_ID)
    {
        return Err(refusal("DroneCommandRequiresShipButNotCapsule", vec![]));
    }
    Ok(p.ship_id)
}

/// `entity.CmdAssist(assistID, droneIDs)` and `entity.CmdGuard(guardID,
/// droneIDs)` (`droneFunctions.py:98-140`) **[V]**: from now on the drones
/// follow that pilot and fight for them -- an assisting drone attacks what
/// the pilot attacks, a guarding one what attacks the pilot (the tick's
/// [`escort_pick`]). Refused, per drone: not the actor's; a pilot who is
/// not the actor or a fleetmate ([`escort_subject`]); a mining drone; and,
/// for assist, more than [`MAX_DRONE_ASSIST`] drones already on that pilot
/// (the wording is `[A]`).
pub fn escort(
    w: &mut Registry,
    actor: Option<i64>,
    subject: i64,
    ids: &[i64],
    guard: bool,
) -> PyValue {
    let Some(actor) = actor else {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal_text("No character.")))
                .collect(),
        );
    };
    let subject_ship = escort_subject(w, actor, subject);
    let mut on_them = w
        .modules()
        .drones()
        .iter()
        .filter(|d| {
            d.escort
                == Some(Escort {
                    character_id: subject,
                    guard: false,
                })
        })
        .count();
    let mut refused = Vec::new();
    for &id in ids {
        let d = match commandable(w, actor, id) {
            Ok(d) => d,
            Err(e) => {
                refused.push((id, e));
                continue;
            }
        };
        if let Err(e) = &subject_ship {
            refused.push((id, e.clone()));
            continue;
        }
        if is_miner(w.content(), d.kind.type_id) {
            refused.push((id, refusal_text("Mining drones do not attack.")));
            continue;
        }
        let wanted = Escort {
            character_id: subject,
            guard,
        };
        if !guard && d.escort != Some(wanted) {
            if on_them >= MAX_DRONE_ASSIST {
                refused.push((
                    id,
                    refusal_text("That pilot already has the most assisting drones allowed."),
                ));
                continue;
            }
            on_them += 1;
        }
        if let Some(t) = d.firing {
            fx(w, Via::Answer, &d, t, false);
        }
        if let Some(r) = w.modules_mut().drones_mut().get_mut(id) {
            r.firing = None;
            r.order = Order::Idle;
            r.phase = Phase::Stopped;
            r.escort = Some(wanted);
        }
        tell_escort(w, Via::Answer, id);
        tell_state(w, Via::Answer, id, ENTITY_IDLE, None);
    }
    errors(refused)
}

/// The drone commands this server does not do (`CmdSalvage`, `CmdUnanchor`,
/// `CmdAbandonDrone`): the client reads the answer with `len(ret)`
/// (`droneFunctions.HandleMultipleCallError`), so `None` raises in the
/// window; each drone the pilot owns is refused in plain words instead.
pub fn unsupported(w: &Registry, actor: Option<i64>, ids: &[i64]) -> PyValue {
    errors(
        ids.iter()
            .map(|id| {
                let why = match actor.map(|a| commandable(w, a, *id)) {
                    Some(Ok(_)) => refusal_text("This server does not do that yet."),
                    Some(Err(e)) => e,
                    None => refusal_text("No character."),
                };
                (*id, why)
            })
            .collect(),
    )
}

/// **What an escorting drone should fight this tick**, if anything
/// (`[A]` throughout, falsified by drones that sit out a fight their
/// escorted pilot is in, or that join one they should not):
///
/// - **guard**: the hostile nearest the drone among [`threats_on`] the
///   escorted ship;
/// - **assist**: the ball the pilot's most recent offensive module is
///   aimed at, else the one their own drones engage ([`assist_target`]);
///
/// either only within the owner's drone control range of the controller
/// ship, never the owner's own ship or drones, never a mining drone's, and
/// never what the owner's safety forbids ([`crate::crimewatch::attack_refusal`],
/// asked for a target the drone is not already engaging).
fn escort_pick(
    w: &Registry,
    d: &Drone,
    candidates: &[Threat],
    home: [f64; 3],
    reach: f64,
) -> Option<Threat> {
    if is_miner(w.content(), d.kind.type_id) {
        return None;
    }
    let from = w.roster().get(d.id).map_or(home, |p| p.motion.position);
    candidates
        .iter()
        .copied()
        .filter(|t| t.id != d.controller_id && t.id != d.id)
        .filter(|t| {
            w.modules()
                .drones()
                .get(t.id)
                .is_none_or(|o| o.owner_id != d.owner_id)
        })
        .filter(|t| crate::space_state::distance(home, t.at) <= reach)
        .filter(|t| {
            d.order == Order::Engage(t.id)
                || crate::crimewatch::attack_refusal(
                    w,
                    d.owner_id,
                    Some(d.kind.type_id),
                    t.id,
                    crate::pyutil::filetime_now(),
                )
                .is_none()
        })
        .min_by(|a, b| {
            crate::space_state::distance(from, a.at)
                .total_cmp(&crate::space_state::distance(from, b.at))
        })
}

/// **What a pilot is attacking** `[A]`: the target of their most recently
/// started running offensive module; failing that, the ball their own
/// (non-escorting) drones are engaging. Falsified by TQ drones that wait for
/// an explicit Engage.
pub fn assist_target(w: &Registry, ship: i64) -> Option<i64> {
    w.modules()
        .running_all()
        .into_iter()
        .filter(|r| r.ship_id == ship && r.effect.is_offensive)
        .filter_map(|r| r.target_id.map(|t| (r.started, t)))
        .max_by_key(|(started, _)| *started)
        .map(|(_, t)| t)
        .or_else(|| {
            w.modules()
                .drones()
                .controlled_by(ship)
                .filter(|d| d.escort.is_none())
                .find_map(|d| match d.order {
                    Order::Engage(t) => Some(t),
                    _ => None,
                })
        })
}

/// `ship.ScoopDrone(droneIDs)` (`droneFunctions.RealScoopToDroneBay:214`)
/// **[V]**: aboard now, if close enough ([`SCOOP_RANGE`], `[A]`).
pub fn scoop_now(w: &mut Registry, actor: Option<i64>, ids: &[i64]) -> PyValue {
    let Some(actor) = actor else {
        return errors(
            ids.iter()
                .map(|id| (*id, refusal_text("No character.")))
                .collect(),
        );
    };
    let mut refused = Vec::new();
    for &id in ids {
        let d = match commandable(w, actor, id) {
            Ok(d) => d,
            Err(e) => {
                refused.push((id, e));
                continue;
            }
        };
        w.advance_system(d.solar_system_id);
        let (Some(me), Some(home)) = (
            w.roster().get(id).map(|p| p.motion.position),
            position_in(w, d.controller_id, d.solar_system_id),
        ) else {
            refused.push((id, refusal_text("That drone is not near your ship.")));
            continue;
        };
        if crate::space_state::distance(me, home) > SCOOP_RANGE {
            refused.push((
                id,
                refusal_text("That drone is too far away to scoop. Recall it first."),
            ));
            continue;
        }
        if let Err(e) = scoop(w, Via::Answer, id) {
            refused.push((id, e));
        }
    }
    errors(refused)
}

/// Keep a scooped drone's armour and structure damage with the item.
fn stash_damage(w: &mut Registry, id: i64) {
    let now = crate::pyutil::filetime_now();
    let Some(rec) = w.combat_mut().take_ship(id) else {
        return;
    };
    let r = rec.rings(now);
    crate::repair::put(
        w,
        id,
        crate::repair::Hurt {
            armor: r.armor_damage,
            structure: r.damage,
        },
    );
}

/// Give a launched drone the damage it was scooped with, if any.
fn seed_damage(w: &mut Registry, id: i64) {
    let Some(h) = crate::repair::hurt(w, id) else {
        return;
    };
    let Some(hull) = hull_of(w, id) else {
        return;
    };
    let now = crate::pyutil::filetime_now();
    let profile = crate::damage::Profile::from_hull_in(w.content(), &hull);
    w.combat_mut().seed_ship(
        id,
        crate::damage::ShipDamage::with_damage(profile, now, h.armor, h.structure),
    );
    crate::repair::put(w, id, crate::repair::Hurt::default());
}

/// **Back aboard**: the item into its controller's drone bay, the ball out
/// of every park, the owner's drone row cleared. In that order on the wire:
/// the gun's stop, `RemoveBalls`, `OnDroneStateChange` with no owner and no
/// controller, then the item rows.
fn scoop(w: &mut Registry, via: Via, id: i64) -> Result<(), PyValue> {
    let d = w
        .modules()
        .drones()
        .get(id)
        .copied()
        .ok_or_else(|| refusal_text("That drone is not in space."))?;
    let unit = volume_of(w, d.kind.type_id);
    let free = bay_free(w, d.controller_id);
    if crate::fitting::units_that_fit(free, unit) < 1 {
        return Err(refusal(
            "NotEnoughDroneBaySpace",
            vec![
                ("available", PyValue::Real(free)),
                ("volume", PyValue::Real(unit)),
            ],
        ));
    }
    let to = Slot::new(d.owner_id, d.controller_id, FLAG_DRONE_BAY);
    let (_, held) = apply_held(w, &[Edit::Move { item_id: id, to }])
        .ok_or_else(|| refusal_text("That drone could not be scooped."))?;
    if let Some(t) = d.firing {
        fx(w, via, &d, t, false);
    }
    // **A scoop does not mend.** The health record dies with the ball (the
    // combat tick forgets a ship that has left the roster), so a drone
    // brought home hurt would launch whole. Its armour and structure damage
    // is stashed under the item, as a dock stashes a ship's
    // (`repair::on_dock`), and the next launch seeds it back; the station
    // repair shop mends it for ISK, as it does for any drone in the bay.
    // Shield is not kept: it refills on its own in under a minute. `[A]`;
    // falsified by retail, where a scooped drone keeps its shield charge.
    stash_damage(w, id);
    let to_park = audience(w, d.solar_system_id);
    w.roster_mut().leave(id);
    w.modules_mut().drones_mut().remove(id);
    let stamp = w.next_destiny_stamp();
    if let Some(removed) = destiny::BallsRemoved::new(to_park, stamp, vec![id]) {
        send(w, via, removed);
    }
    send(
        w,
        via,
        StateChange {
            to: vec![d.owner_id],
            row: DroneRow::gone(id, d.kind.type_id),
        },
    );
    release(w, via, d.owner_id, held);
    Ok(())
}

/// **A drone shot to pieces** -- `combat::kill`'s drone branch. In this
/// order, all on the clock (a death is nobody's turn):
///
/// 1. its gun's `OnSpecialFX` stop, if it was firing;
/// 2. **one [`destiny::Kill`]** to `park`: `RemoveBall(droneID, True)`, the
///    terminal singular that plays the explosion (`michelle.py:1762`)
///    **[V]**, with **no wreck** -- every light drone's `types.wreckTypeID`
///    is absent (`tools/dogma.py` `types` rows for 2203/2454/2464/2486,
///    build 3619879) **[V-data]** -- and no capsule;
/// 3. `TargetLost` with `'Exploding'` to whoever had it locked
///    (`targeting::left_space`), while the roster still finds the ball;
/// 4. out of the roster, the drone table and the health table;
/// 5. `OnDroneStateChange` with no owner and no controller to the owner --
///    the row that deletes it from `stateByDroneID`, and so from the drone
///    window's "in space" list (`michelle.py:2028-2035`) **[V]**, the same
///    row a scoop sends;
/// 6. the item destroyed, its `OnItemChange` to the owner.
///
/// `[A]` the owner's combat log says nothing about the loss: `combat::fire`
/// writes a victim's line only for a character's ship.
pub fn destroyed(w: &mut Registry, id: i64, park: Vec<i64>) {
    let Some(d) = w.modules().drones().get(id).copied() else {
        return;
    };
    if let Some(t) = d.firing {
        fx(w, Via::Clock, &d, t, false);
    }
    let stamp = w.next_destiny_stamp();
    if let Some(k) = destiny::Kill::new(park, stamp, id, None, None) {
        w.schedule(Duration::ZERO, k);
    }
    crate::targeting::left_space(
        w,
        id,
        crate::targeting::Reason::Exploding,
        crate::targeting::Delivery::OnTheClock,
    );
    w.roster_mut().leave(id);
    w.modules_mut().drones_mut().remove(id);
    send(
        w,
        Via::Clock,
        StateChange {
            to: vec![d.owner_id],
            row: DroneRow::gone(id, d.kind.type_id),
        },
    );
    match apply_held(w, &[Edit::Destroy { item_id: id }]) {
        Some((_, held)) => release(w, Via::Clock, d.owner_id, held),
        None => crate::outln!("[drones] the item of destroyed drone {id} could not be removed"),
    }
}

/// **Drone items a restart left in space**, swept once per process: a drone
/// item in a solar system (`idCheckers.IsSolarSystem`, 30,000,000 up to
/// 40,000,000 **[V]**) with no live drone record is a drone whose ball the
/// restart lost. `[A]` it goes to its owner's home station hangar -- the
/// controller ship may be anywhere, and a hangar always takes it.
fn sweep_orphans(w: &mut Registry) {
    w.modules_mut().drones_mut().swept = true;
    let orphans: Vec<(i64, i64)> = w
        .store()
        .items()
        .iter()
        .filter(|i| {
            i.category_id == CATEGORY_DRONE
                && (30_000_000..40_000_000).contains(&i.location_id)
                && w.modules().drones().get(i.item_id).is_none()
        })
        .map(|i| (i.item_id, i.owner_id))
        .collect();
    for (item_id, owner) in orphans {
        let Some(station) = w.character(owner).and_then(|c| c.station_id) else {
            continue;
        };
        let to = Slot::new(owner, station, crate::model::FLAG_HANGAR);
        if let Some((_, held)) = apply_held(w, &[Edit::Move { item_id, to }]) {
            release(w, Via::Clock, owner, held);
        }
    }
}

/// **What an aggressive drone may fight**, for one controller ship: the
/// hostile balls in its system that are attacking the ship or one of its
/// drones, within the pilot's reach of the ship. `[A]` an attacker is
///
/// - an **NPC** whose current target is the ship or one of its drones; or
/// - a **pilot** other than the owner, outside the owner's corporation, who
///   holds a lock on the ship or a drone **and** has a live weapons timer
///   (`crimewatch`) -- a lock alone is also what a logistics pilot has.
///
/// Falsified by drones that sit out a fight they should join, or that go
/// for a friend who merely targeted their ship.
pub fn threats_on(
    w: &Registry,
    controller: i64,
    owner: i64,
    system: i64,
    now: BlueTime,
) -> Vec<Threat> {
    let Some(home) = position_in(w, controller, system) else {
        return Vec::new();
    };
    let ours: std::collections::BTreeSet<i64> = w
        .modules()
        .drones()
        .controlled_by(controller)
        .map(|d| d.id)
        .chain([controller])
        .collect();
    let reach = drone_skills::control_range(w, owner, controller);
    let owner_corp = w.character(owner).map(|c| c.corporation_id);
    let mut out = Vec::new();
    for p in w.roster().in_system(system) {
        if ours.contains(&p.ship_id) || p.character_id == owner {
            continue;
        }
        if crate::space_state::distance(home, p.motion.position) > reach {
            continue;
        }
        let attacking = if let Some(n) = w.npcs().get(p.character_id) {
            n.target.is_some_and(|t| ours.contains(&t))
        } else if w.modules().drones().get(p.character_id).is_some() {
            // Another pilot's drone: it is a drone, not the pilot; the
            // pilot is judged on their own ship.
            false
        } else if let Some(c) = w.character(p.character_id) {
            Some(c.corporation_id) != owner_corp
                && crate::crimewatch::has_weapons_timer(w, p.character_id, now)
                && w.locks()
                    .targets_of(p.ship_id)
                    .iter()
                    .any(|t| ours.contains(t))
        } else {
            false
        };
        if attacking {
            out.push(Threat {
                id: p.ship_id,
                at: p.motion.position,
            });
        }
    }
    out
}

/// **A drone left too long** ([`ABANDON_AFTER`]): out of space. If its
/// controller ship still exists it goes to the pilot's last station's
/// hangar (`Character.station_id`, the sweep's rule); if the ship is gone
/// the drone is **lost**: the item is destroyed. Told to the park and the
/// owner like a scoop.
fn abandon(w: &mut Registry, id: i64) {
    let Some(d) = w.modules().drones().get(id).copied() else {
        return;
    };
    let ship_alive = w
        .store()
        .get(d.controller_id)
        .is_some_and(|s| s.owner_id == d.owner_id);
    let station = w.character(d.owner_id).and_then(|c| c.station_id);
    let edit = match (ship_alive, station) {
        (true, Some(station)) => Edit::Move {
            item_id: id,
            to: Slot::new(d.owner_id, station, crate::model::FLAG_HANGAR),
        },
        _ => Edit::Destroy { item_id: id },
    };
    let Some((_, held)) = apply_held(w, &[edit]) else {
        crate::outln!("[drones] drone {id} could not be taken out of space");
        return;
    };
    if let Some(t) = d.firing {
        fx(w, Via::Clock, &d, t, false);
    }
    let to_park = audience(w, d.solar_system_id);
    crate::targeting::left_space(
        w,
        id,
        crate::targeting::Reason::Exploding,
        crate::targeting::Delivery::OnTheClock,
    );
    w.roster_mut().leave(id);
    w.modules_mut().drones_mut().remove(id);
    let stamp = w.next_destiny_stamp();
    if let Some(removed) = destiny::BallsRemoved::new(to_park, stamp, vec![id]) {
        send(w, Via::Clock, removed);
    }
    send(
        w,
        Via::Clock,
        StateChange {
            to: vec![d.owner_id],
            row: DroneRow::gone(id, d.kind.type_id),
        },
    );
    release(w, Via::Clock, d.owner_id, held);
}

/// `dogmaIM.ChangeDroneSettings({1275: aggressive, 1297: focusFire})`
/// (`godma:2704-2708`, `:2722`) [V]: the pilot's settings, kept for every
/// drone of theirs. Answers `None`, which the client discards.
pub fn change_settings(w: &mut Registry, actor: Option<i64>, arg: &PyValue) -> PyValue {
    if let Some(actor) = actor {
        let now = w.modules().drones().settings_of(actor);
        let next = now.changed_by(arg);
        w.modules_mut().drones_mut().settings.insert(actor, next);
    }
    PyValue::None
}

/// `ChangeDroneSettings`' arm for `service/inventory.rs`.
pub fn settings_call(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let empty = PyValue::None;
    Ok(change_settings(
        w,
        req.actor,
        req.args.first().unwrap_or(&empty),
    ))
}

/// A drone's hull as it flies: the launch's, with the pilot's Navigation
/// and Durability. For `dogma::hull`'s drone arm.
pub fn hull_of(w: &Registry, id: i64) -> Option<crate::destiny::hull::Hull> {
    w.modules().drones().get(id).map(|d| d.kind.hull)
}

/// A drone's health profile as it flies, for `dogma::profile`'s drone arm:
/// the type's resonances over the launch's shield, armour and hull points.
pub fn profile_of(w: &Registry, id: i64) -> Option<crate::damage::Profile> {
    w.modules()
        .drones()
        .get(id)
        .map(|d| crate::damage::Profile::from_hull_in(w.content(), &d.kind.hull))
}

// ---------------------------------------------------------------------------
// The wire: `entity`, and two methods of `ship`
// ---------------------------------------------------------------------------

/// The services dispatched here: the **`entity`** bound object,
/// `Moniker('entity', session.solarsystemid2)` (`eveMoniker.GetEntityAccess:
/// 92-95`) **[V]**, which carries every drone command.
pub const SERVICES: &[&str] = &["entity"];

/// A list of ints, from a list or tuple argument.
fn ids_arg(v: Option<&PyValue>) -> Option<Vec<i64>> {
    match v? {
        PyValue::List(v) | PyValue::Tuple(v) => Some(
            v.iter()
                .filter_map(|i| match i {
                    PyValue::Int(n) => Some(*n),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn bad_args(req: &Request<'_>, want: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: format!("expected {want}"),
    }
}

/// `entity`'s methods. `CmdAssist` and `CmdGuard` are [`escort`]; `CmdSalvage`,
/// `CmdUnanchor` and `CmdAbandonDrone` answer a refusal per drone
/// ([`unsupported`]) and `CmdReconnectToDrones` answers `{}` (nothing to
/// reconnect to: a drone is never left in space), because the client reads
/// each answer with `len()` and `.iteritems()` and `None` raises
/// (`droneFunctions.py:18-25,255-260`).
pub fn dispatch(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "CmdEngage" => {
            let ids =
                ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs, targetID)"))?;
            let Some(PyValue::Int(target)) = req.args.get(1) else {
                return Err(bad_args(req, "(droneIDs, targetID)"));
            };
            Ok(engage(w, req.actor, &ids, *target))
        }
        "CmdMineRepeatedly" => {
            let ids =
                ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs, targetID)"))?;
            let Some(PyValue::Int(target)) = req.args.get(1) else {
                return Err(bad_args(req, "(droneIDs, targetID)"));
            };
            Ok(mine(w, req.actor, &ids, *target))
        }
        "CmdReturnBay" => {
            let ids = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs,)"))?;
            Ok(return_bay(w, req.actor, &ids))
        }
        "CmdAssist" | "CmdGuard" => {
            let ids =
                ids_arg(req.args.get(1)).ok_or_else(|| bad_args(req, "(pilotID, droneIDs)"))?;
            let Some(PyValue::Int(who)) = req.args.first() else {
                return Err(bad_args(req, "(pilotID, droneIDs)"));
            };
            Ok(escort(w, req.actor, *who, &ids, req.method == "CmdGuard"))
        }
        "CmdSalvage" | "CmdUnanchor" => {
            let ids =
                ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs, targetID)"))?;
            Ok(unsupported(w, req.actor, &ids))
        }
        "CmdAbandonDrone" => {
            let ids = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs,)"))?;
            Ok(unsupported(w, req.actor, &ids))
        }
        "CmdReconnectToDrones" => Ok(PyValue::Dict(vec![])),
        "CmdReturnHome" => {
            let ids = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs,)"))?;
            Ok(return_home(w, req.actor, &ids))
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

/// `ship.LaunchDrones(oldItems, whoseBehalfID, ignoreWarning)`'s arm:
/// `oldItems` is `[(itemID, qty), ...]` (`eveMisc.LaunchFromShip:64-76`)
/// **[V]**.
pub fn launch_call(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(PyValue::List(v) | PyValue::Tuple(v)) = req.args.first() else {
        return Err(bad_args(
            req,
            "([(itemID, qty)], whoseBehalfID, ignoreWarning)",
        ));
    };
    let items: Vec<(i64, i64)> = v
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => match (p.first(), p.get(1)) {
                (Some(PyValue::Int(id)), Some(PyValue::Int(q))) => Some((*id, *q)),
                (Some(PyValue::Int(id)), _) => Some((*id, 1)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Ok(launch(w, req.actor, &items))
}

/// `ship.ScoopDrone(droneIDs)`'s arm.
pub fn scoop_call(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let ids = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(droneIDs,)"))?;
    Ok(scoop_now(w, req.actor, &ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const ALICE_SHIP: i64 = 100_000_001;
    const BOB_SHIP: i64 = 100_000_002;
    /// Alice's stack of drones.
    const STACK: i64 = 100_000_100;
    /// Something that is not a drone, in Alice's hangar.
    const GUN: i64 = 100_000_101;
    const SYSTEM: i64 = 30_000_001;
    const STATION: i64 = 60_012_526;
    /// Invented rows (CP58e): a hull with a big bay, one with room and
    /// bandwidth for one drone, one with no bay, a combat drone, and a
    /// drone whose default effect is not a gun.
    const BIG_BAY: i64 = 990_580_001;
    const ONE_BAY: i64 = 990_580_002;
    const NO_BAY: i64 = 990_580_003;
    const DRONE: i64 = 990_580_010;
    const NOT_A_GUN: i64 = 990_580_011;
    /// A combat drone with tracking, and the invented skills around it: the
    /// Drones-like skill, a Light-Operation-like one (effect 1730), an
    /// Interfacing-like one and an Avionics-like one (effect 504).
    const TRACK_DRONE: i64 = 990_580_013;
    const SK_DRONES: i64 = 990_581_003;
    const SK_LIGHT: i64 = 990_581_004;
    const SK_IFACE: i64 = 990_581_005;
    const SK_AVIONICS: i64 = 990_581_006;
    /// A module row of `typedata::FIXTURE`: something that is not a drone.
    const GUN_TYPE: i64 = 990_000_002;

    /// `typedata::FIXTURE` and the rows above, in the tool's own format.
    /// **Not the client's numbers.**
    const ROWS: &str = "\
type\t990580001\t99580\t6\t1\t1\t50000.0\t400.0\t10000000.0\t200.0\t0\t990581103\t9=1000.0;283=60.0;658=10.0;1271=50.0\tFixture Carrier Hull
type\t990580002\t99580\t6\t1\t1\t20000.0\t100.0\t1000000.0\t40.0\t0\t\t9=300.0;283=6.0;1271=6.0\tFixture One Drone Hull
type\t990580003\t99580\t6\t1\t1\t20000.0\t100.0\t1000000.0\t40.0\t0\t\t9=300.0\tFixture No Bay Hull
type\t990580010\t100\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t10\t10\t9=111.0;37=1000.0;51=3000.0;54=2000.0;64=2.0;70=50.0;117=16.0;263=40.0;265=60.0;416=500.0;479=100000.0;482=1.0;508=250.0;552=20.0;1272=5.0\tFixture Gun Drone
type\t990580011\t99581\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t99\t99\t37=1000.0;1272=5.0\tFixture Digging Drone
type\t990580012\t99582\t18\t1\t1\t5.0\t0.0\t0.0\t5.0\t17\t17\t9=40.0;37=400.0;54=5000.0;73=60000.0;77=25.0;1272=5.0\tFixture Mining Drone
type\t990580013\t100\t18\t1\t1\t4.0\t0.0\t2000.0\t10.0\t10\t10,1730\t9=100.0;37=1000.0;51=3000.0;54=2000.0;64=2.0;70=50.0;117=10.0;158=1000.0;160=2.0;182=990581003.0;183=990581004.0;263=40.0;265=60.0;277=1.0;278=2.0;416=500.0;479=100000.0;482=1.0;508=250.0;552=20.0;620=25.0;1272=5.0\tFixture Tracking Drone
type\t990581003\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990581100\t\tFixture Drones Skill
type\t990581004\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990581100,990581101,1730\t182=990581003.0;277=1.0;292=5.0\tFixture Light Operation
type\t990581005\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990581100,990581101,990581102\t182=990581003.0;277=1.0;292=10.0\tFixture Interfacing
type\t990581006\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990581100,990581104,504\t182=990581003.0;277=1.0;459=5000.0\tFixture Avionics
type\t990580020\t99583\t25\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Ore
attribute\t64\t1.0\tdamageMultiplier
attribute\t280\t0.0\tskillLevel
attribute\t292\t0.0\tdamageMultiplierBonus
effect\t990581100\t0\t0\t0\t0\t0\t0\t0\tfixtureSkillEffect\t
effect\t990581101\t0\t0\t0\t0\t0\t0\t0\tfixtureLevelTimesBonus\t
effect\t990581103\t0\t0\t0\t0\t0\t0\t0\tfixtureHullDroneBonus\t
effect\t990581104\t0\t0\t0\t0\t0\t0\t0\tfixtureLevelTimesRange\t
effect\t990581102\t0\t0\t0\t0\t0\t0\t0\tfixtureInterfacing\t
modifier\t990581101\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t990581103\tcharID\tOwnerRequiredSkillModifier\t64\t658\t6\t990581003\t
modifier\t990581104\titemID\tItemModifier\t459\t280\t0\t\t
modifier\t990581102\tcharID\tOwnerRequiredSkillModifier\t64\t292\t6\t990581003\t
";

    fn content() -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        let text = format!("{}{ROWS}", typedata::FIXTURE);
        let table = typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", typedata::Installed::build(table));
        c
    }

    /// The test drone, as [`find_in`] reads it.
    fn kind() -> DroneType {
        find_in(&content(), DRONE).expect("the test drone flies")
    }

    /// Alice in a `hull` with `bay` drones in its drone bay and Drones at
    /// `skill`, Bob in a hull with no bay; both docked. A stack of 20 more
    /// drones and a gun sit in Alice's hangar. The world reads [`content`].
    fn world(hull: i64, bay: i64, skill: i64) -> Registry {
        world_with(hull, bay, &[(SKILL_DRONES, skill)], DRONE)
    }

    /// [`world`] with any skills and any drone type in Alice's stacks.
    fn world_with(hull: i64, bay: i64, trained: &[(i64, i64)], drone_type: i64) -> Registry {
        let pilot = |id: i64, name: &str, ship: i64, ty: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: ty,
            ..crate::model::Character::default()
        };
        let content = std::sync::Arc::new(content());
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice", ALICE_SHIP, hull),
                pilot(BOB, "Bob", BOB_SHIP, NO_BAY),
            ],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(content.clone());
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut() {
            let ty = match i.item_id {
                ALICE_SHIP => hull,
                BOB_SHIP => NO_BAY,
                _ => continue,
            };
            let t = typedata::item_in(&content, ty).expect("a test hull");
            i.type_id = t.type_id;
            i.group_id = t.group_id;
        }
        let hob = typedata::item_in(&content, drone_type).expect("the test drone");
        let drone =
            |item_id: i64, location_id: i64, flag_id: i64, quantity: i64| crate::item::Item {
                item_id,
                type_id: hob.type_id,
                owner_id: ALICE,
                location_id,
                flag_id,
                quantity,
                group_id: hob.group_id,
                category_id: hob.category_id,
                singleton: false,
            };
        if bay > 0 {
            items.push(drone(STACK, ALICE_SHIP, FLAG_DRONE_BAY, bay));
        }
        items.push(drone(STACK + 10, STATION, crate::model::FLAG_HANGAR, 20));
        let gun = typedata::item_in(&content, GUN_TYPE).expect("a fixture module");
        items.push(crate::item::Item {
            item_id: GUN,
            type_id: gun.type_id,
            owner_id: ALICE,
            location_id: STATION,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: gun.group_id,
            category_id: gun.category_id,
            singleton: false,
        });
        let mut skills = crate::skill::Skills::new();
        for &(skill, level) in trained.iter().filter(|(_, l)| *l > 0) {
            // The invented skills are in no skill table: a literal type.
            let t = crate::skill::SkillType {
                type_id: skill,
                name: "Fixture Skill",
                group_id: 273,
                rank: 1,
                primary: 166,
                secondary: 167,
            };
            skills.grant_type(&t, level);
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: STACK + 11,
            skills: vec![(ALICE, skills.entries().cloned().collect())],
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        reg
    }

    fn undock(reg: &mut Registry, who: i64) {
        destiny::on_ballpark_bind(reg, Some(who));
        let _ = reg.take_pending();
    }

    /// Every test drone Alice owns, by where it is: (bay, space, hangar).
    fn census(reg: &Registry) -> (i64, i64, i64) {
        let mut out = (0, 0, 0);
        for i in reg
            .store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ALICE && i.type_id == DRONE)
        {
            if i.flag_id == FLAG_DRONE_BAY {
                out.0 += i.quantity;
            } else if i.location_id == SYSTEM && i.flag_id == FLAG_IN_SPACE {
                out.1 += i.quantity;
            } else {
                out.2 += i.quantity;
            }
        }
        out
    }

    /// One queued push: `(audience, name, args)`.
    type Sent = (Option<Vec<i64>>, &'static str, Vec<PyValue>);

    fn pending(reg: &mut Registry) -> Vec<Sent> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (name, args) = p.to_push();
                (p.audience(), name, args)
            })
            .collect()
    }

    fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience().unwrap_or_default(), name, args)
            })
            .collect()
    }

    /// The new drone ids `launch` answered for the stack.
    fn launched_ids(answer: &PyValue) -> Vec<i64> {
        let PyValue::Dict(pairs) = answer else {
            panic!("a dict")
        };
        pairs
            .iter()
            .filter(|(_, k)| *k == PyValue::Int(STACK))
            .flat_map(|(v, _)| match v {
                PyValue::List(v) => v.clone(),
                _ => vec![],
            })
            .filter_map(|e| match e {
                PyValue::Int(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn launch_n(reg: &mut Registry, n: i64) -> Vec<i64> {
        let answer = launch(reg, Some(ALICE), &[(STACK, n)]);
        launched_ids(&answer)
    }

    // -- static data --------------------------------------------------------

    /// **A drone type is its row in the loaded type data** (CP58e, ruling
    /// 3): every number is the row's own attribute, by id.
    #[test]
    fn a_drone_type_is_read_from_the_loaded_type_data() {
        let c = content();
        let t = typedata::installed_in(&c).table();
        let a = |id| t.attribute(DRONE, id);
        let k = find_in(&c, DRONE).expect("a combat drone");
        let row = t.get(DRONE).expect("the row");
        assert_eq!(k.type_id, DRONE);
        assert_eq!(k.name, row.name);
        assert_eq!(k.bandwidth, a(attr::DRONE_BANDWIDTH_USED));
        assert_eq!(k.weapon_period_ms, a(attr::SPEED) as i64);
        assert_eq!(k.optimal, a(attr::MAX_RANGE));
        assert_eq!(k.orbit_range, a(attr::ENTITY_FLY_RANGE));
        assert_eq!(k.damage_multiplier, a(typedata::attr::DAMAGE_MULTIPLIER));
        assert_eq!(k.damage.kinetic, a(crate::damage::attr::KINETIC_DAMAGE));
        assert_eq!(
            k.cruise_fraction,
            a(attr::ENTITY_CRUISE_SPEED) / a(typedata::attr::MAX_VELOCITY)
        );
        assert_eq!((k.hull.mass, k.hull.radius), (row.mass, row.radius as f32));
        assert_eq!(k.hull.hp, a(typedata::attr::HP));
        assert_eq!(k.hull.warp_speed_au, 0.0, "drones do not warp");
        assert_eq!(hull_in(&c, DRONE), Some(k.hull));
    }

    /// Only a drone whose default effect is the gun flies: not a drone
    /// with another default effect, not the fixture's drone with none, not
    /// a hull, not an unknown id.
    #[test]
    fn only_a_combat_drone_is_a_drone_type() {
        let c = content();
        for id in [NOT_A_GUN, 990_000_006, BIG_BAY, GUN_TYPE, 1] {
            assert!(find_in(&c, id).is_none(), "{id}");
        }
        // Without a bundle the server flies no drone at all.
        assert!(find_in(&crate::content::Content::fixture(), DRONE).is_none());
    }

    /// A bay is the hull's own `droneCapacity` and `droneBandwidth`; a hull
    /// with neither, or a type that is not a hull, has none.
    #[test]
    fn a_bay_is_the_hulls_own_capacity_and_bandwidth() {
        let c = content();
        let t = typedata::installed_in(&c).table();
        let b = bay_in(&c, BIG_BAY).expect("a bay");
        assert_eq!(
            b.capacity,
            t.attribute(BIG_BAY, typedata::attr::DRONE_CAPACITY)
        );
        assert_eq!(
            b.bandwidth,
            t.attribute(BIG_BAY, typedata::attr::DRONE_BANDWIDTH)
        );
        assert!(bay_in(&c, NO_BAY).is_none());
        assert!(bay_in(&c, DRONE).is_none());
    }

    // -- docked: the bay ----------------------------------------------------

    #[test]
    fn drones_go_into_the_bay_up_to_its_capacity_and_no_further() {
        let mut reg = world(BIG_BAY, 0, 5);
        let room = bay_in(reg.content(), BIG_BAY).expect("a bay").capacity;
        let unit = typedata::item_in(reg.content(), DRONE).expect("row").volume;
        let fits = crate::fitting::units_that_fit(room, unit);
        to_bay(&mut reg, Some(ALICE), ALICE_SHIP, STACK + 10, Some(fits)).expect("they fit");
        assert_eq!(census(&reg), (fits, 0, 20 - fits));
        // One more is refused, and nothing moves.
        let e = to_bay(&mut reg, Some(ALICE), ALICE_SHIP, STACK + 10, Some(1));
        assert!(matches!(e, Err(ServiceError::Raise(_))));
        assert_eq!(census(&reg), (fits, 0, 20 - fits));
        assert!(bay_used(&reg, ALICE_SHIP) <= room);
    }

    #[test]
    fn only_a_drone_goes_into_the_bay() {
        let mut reg = world(BIG_BAY, 0, 5);
        assert!(to_bay(&mut reg, Some(ALICE), ALICE_SHIP, GUN, None).is_err());
        assert_eq!(
            reg.store().get(GUN).map(|i| i.flag_id),
            Some(crate::model::FLAG_HANGAR)
        );
    }

    #[test]
    fn a_hull_without_a_bay_takes_no_drones() {
        let mut reg = world(NO_BAY, 0, 5);
        assert!(to_bay(&mut reg, Some(ALICE), ALICE_SHIP, STACK + 10, Some(1)).is_err());
        assert_eq!(census(&reg), (0, 0, 20));
    }

    #[test]
    fn drones_come_out_of_the_bay_into_the_hangar() {
        let mut reg = world(BIG_BAY, 5, 5);
        to_hangar(&mut reg, Some(ALICE), STACK, Some(2)).expect("a split");
        assert_eq!(census(&reg), (3, 0, 22));
        to_hangar(&mut reg, Some(ALICE), STACK, None).expect("the rest");
        assert_eq!(census(&reg), (0, 0, 25));
    }

    #[test]
    fn the_bay_is_loaded_only_while_docked() {
        let mut reg = world(BIG_BAY, 0, 5);
        undock(&mut reg, ALICE);
        assert!(to_bay(&mut reg, Some(ALICE), ALICE_SHIP, STACK + 10, Some(1)).is_err());
        assert_eq!(census(&reg), (0, 0, 20));
    }

    // -- in space: launch ---------------------------------------------------

    #[test]
    fn a_launch_moves_drones_from_the_bay_into_space_and_conserves_them() {
        let mut reg = world(BIG_BAY, 5, 5);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 3);
        assert_eq!(ids.len(), 3);
        assert_eq!(census(&reg), (2, 3, 20));
        for id in &ids {
            let p = reg.roster().get(*id).expect("a ball in the roster");
            assert_eq!(p.ship_id, *id);
            assert_eq!(p.solar_system_id, SYSTEM);
            let d = reg.modules().drones().get(*id).expect("a live drone");
            assert_eq!((d.owner_id, d.controller_id), (ALICE, ALICE_SHIP));
            let item = reg.store().get(*id).expect("the drone is its item");
            assert_eq!((item.location_id, item.quantity), (SYSTEM, 1));
        }
    }

    /// The pushes the client needs, to the people who need them: the balls
    /// to the system, `OnDroneStateChange` with its seven positional
    /// arguments to the owner (`michelle.py:386-388`) [V], and the item rows.
    #[test]
    fn a_launch_tells_the_system_about_the_ball_and_the_owner_about_the_drone() {
        let mut reg = world(BIG_BAY, 5, 5);
        undock(&mut reg, ALICE);
        undock(&mut reg, BOB);
        let ids = launch_n(&mut reg, 1);
        let sent = pending(&mut reg);
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        let balls = names
            .iter()
            .position(|n| *n == "DoDestinyUpdate")
            .expect("balls");
        let state = names
            .iter()
            .position(|n| *n == "OnDroneStateChange")
            .expect("state");
        let item = names
            .iter()
            .position(|n| *n == "OnItemChange")
            .expect("item");
        assert!(balls < state && state < item, "{names:?}");
        let (to, _, _) = &sent[balls];
        let mut to = to.clone().expect("addressed");
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB]);
        let (to, _, args) = &sent[state];
        assert_eq!(to.as_deref(), Some(&[ALICE][..]));
        assert_eq!(args.len(), 7);
        assert_eq!(args[0], PyValue::Int(ids[0]));
        assert_eq!(args[1], PyValue::Int(ALICE));
        assert_eq!(args[2], PyValue::Int(ALICE_SHIP));
        assert_eq!(args[4], PyValue::Int(DRONE));
    }

    #[test]
    fn no_more_drones_launch_than_the_drones_skill_allows() {
        let mut reg = world(BIG_BAY, 10, 2);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 10);
        let limit = max_active(&reg, ALICE);
        assert_eq!(ids.len() as i64, limit);
        assert_eq!(census(&reg), (10 - limit, limit, 20));
        // And a second launch adds none.
        assert!(launch_n(&mut reg, 1).is_empty());
        assert_eq!(census(&reg), (10 - limit, limit, 20));
    }

    #[test]
    fn without_the_skill_nothing_launches() {
        let mut reg = world(BIG_BAY, 5, 0);
        undock(&mut reg, ALICE);
        assert!(launch_n(&mut reg, 1).is_empty());
        assert_eq!(census(&reg), (5, 0, 20));
    }

    /// The test hull's bandwidth is one test drone's.
    #[test]
    fn no_more_drones_launch_than_the_hull_has_bandwidth_for() {
        let mut reg = world(ONE_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let bw = bay_in(reg.content(), ONE_BAY).expect("a bay").bandwidth;
        let ids = launch_n(&mut reg, 1);
        let used: f64 = reg
            .modules()
            .drones()
            .controlled_by(ALICE_SHIP)
            .map(|d| d.kind.bandwidth)
            .sum();
        assert_eq!(ids.len(), 1);
        assert!(used <= bw);
    }

    #[test]
    fn nothing_launches_while_docked() {
        let mut reg = world(BIG_BAY, 5, 5);
        assert!(launch_n(&mut reg, 1).is_empty());
        assert_eq!(census(&reg), (5, 0, 20));
        assert!(reg.modules().drones().is_empty());
    }

    // -- the seam -----------------------------------------------------------

    /// A drone's roster entry is flown by nobody and owned by its pilot: the
    /// hull is the drone's, so the server integrates it as one.
    #[test]
    fn a_drone_is_piloted_by_nobody_and_owned_by_its_owner() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        let p = reg.pilot(id).expect("the drone's pilot");
        assert_eq!(p.ship_type_id, DRONE);
        assert_eq!(p.owner_id, ALICE);
        assert_eq!(p.char_id, None);
    }

    /// A pilot who arrives finds the drone: its ball and slim in the state,
    /// its row in `droneState`, and every slim with a ball.
    #[test]
    fn the_bootstrap_carries_the_drones_in_the_system() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        let _ = reg.take_pending();
        let b = destiny::bootstrap_for(&mut reg, BOB).expect("Bob's state");
        assert!(b.every_slim_has_a_ball());
        let s = b.slims.iter().find(|s| s.item_id == id).expect("a slim");
        assert_eq!((s.type_id, s.owner_id), (DRONE, ALICE));
        assert_eq!(b.drones.len(), 1);
        assert_eq!(b.drones[0].drone_id, id);
        assert_eq!(b.drones[0].owner_id, Some(ALICE));
        assert_eq!(b.drones[0].controller_id, Some(ALICE_SHIP));
    }

    // -- the brain ----------------------------------------------------------

    fn a_drone() -> Drone {
        Drone::launched(200_000_000, kind(), ALICE, ALICE_SHIP, SYSTEM)
    }

    fn view(target: Option<[f64; 3]>) -> View {
        View {
            me: [0.0; 3],
            home: Some([300.0, 0.0, 0.0]),
            target,
            target_radius: 0.0,
            now: 1_000,
            control_range: CONTROL_RANGE,
            aggressive: false,
            threat: None,
            orbit: None,
        }
    }

    #[test]
    fn an_idle_drone_orbits_its_ship_once() {
        let (d, out) = decide(&a_drone(), &view(None));
        assert_eq!(out, vec![Intent::Orbit(ALICE_SHIP)]);
        assert_eq!(d.phase, Phase::Home);
        let (_, again) = decide(&d, &view(None));
        assert!(again.is_empty());
    }

    #[test]
    fn an_engaged_drone_beyond_optimal_approaches_without_firing() {
        let d = Drone {
            order: Order::Engage(7),
            ..a_drone()
        };
        let far = [kind().optimal * 3.0, 0.0, 0.0];
        let (d, out) = decide(&d, &view(Some(far)));
        assert!(out.contains(&Intent::Follow {
            ball: 7,
            range: kind().orbit_range
        }));
        assert!(
            !out.iter()
                .any(|i| matches!(i, Intent::FireStart(_) | Intent::Shoot { .. }))
        );
        assert_eq!(d.firing, None);
    }

    #[test]
    fn an_engaged_drone_in_optimal_orbits_fires_and_shoots_once_per_period() {
        let d = Drone {
            order: Order::Engage(7),
            ..a_drone()
        };
        let near = [kind().optimal * 0.5, 0.0, 0.0];
        let (d, out) = decide(&d, &view(Some(near)));
        assert!(out.contains(&Intent::Orbit(7)));
        assert!(out.contains(&Intent::FireStart(7)));
        let shots = |out: &[Intent]| {
            out.iter()
                .filter(|i| matches!(i, Intent::Shoot { .. }))
                .count()
        };
        assert_eq!(shots(&out), 1);
        let period = kind().weapon_period();
        let later = View {
            now: 1_000 + 3 * period,
            ..view(Some(near))
        };
        let (_, out) = decide(&d, &later);
        assert_eq!(shots(&out), 3);
    }

    #[test]
    fn a_drone_whose_target_is_gone_stops_its_gun_and_goes_home() {
        let d = Drone {
            order: Order::Engage(7),
            firing: Some(7),
            phase: Phase::Orbit,
            ..a_drone()
        };
        let (d, out) = decide(&d, &view(None));
        assert!(out.contains(&Intent::FireStop(7)));
        assert!(out.contains(&Intent::Orbit(ALICE_SHIP)));
        assert_eq!(d.order, Order::Idle);
        assert_eq!(d.firing, None);
    }

    #[test]
    fn a_drone_without_its_ship_stops() {
        let d = Drone {
            phase: Phase::Home,
            ..a_drone()
        };
        let v = View {
            home: None,
            ..view(None)
        };
        let (d, out) = decide(&d, &v);
        assert!(out.contains(&Intent::Stop));
        assert_eq!(d.phase, Phase::Stopped);
    }

    #[test]
    fn a_returning_drone_is_scooped_only_near_its_ship() {
        let d = Drone {
            order: Order::ReturnBay,
            ..a_drone()
        };
        let far = View {
            home: Some([SCOOP_RANGE * 2.0, 0.0, 0.0]),
            ..view(None)
        };
        let (d, out) = decide(&d, &far);
        assert!(!out.contains(&Intent::Scoop));
        assert!(out.contains(&Intent::Follow {
            ball: ALICE_SHIP,
            range: RETURN_FOLLOW_RANGE
        }));
        let near = View {
            home: Some([SCOOP_RANGE * 0.5, 0.0, 0.0]),
            ..view(None)
        };
        let (_, out) = decide(&d, &near);
        assert!(out.contains(&Intent::Scoop));
    }

    // -- the whole loop -----------------------------------------------------

    /// One Angel Hijacker `offset` metres beyond a drone, along x.
    fn rat_beyond(reg: &mut Registry, drone: i64, offset: f64) -> i64 {
        let at = reg.roster().get(drone).expect("in space").motion.position;
        crate::npc::spawn(
            reg,
            &crate::npc::ANGEL_HIJACKER,
            SYSTEM,
            [at[0] + offset, at[1], at[2]],
        )
        .expect("an NPC")
    }

    /// Launch, engage a rat inside optimal, and let the clock run: the gun's
    /// visual names the drone as its module [V], the owner gets the combat
    /// log, the rat dies, and the drone comes back to orbit its ship.
    #[test]
    fn drones_kill_a_rat_on_the_clock_as_their_owner() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        let rat = rat_beyond(&mut reg, id, kind().optimal * 0.4);
        let _ = reg.take_pending();
        let refused = engage(&mut reg, Some(ALICE), &[id], rat);
        assert_eq!(refused, PyValue::Dict(vec![]));
        let sent = pending(&mut reg);
        assert!(sent.iter().any(|(_, n, a)| *n == "OnDroneStateChange"
            && a[3] == PyValue::Int(ENTITY_ENGAGE)
            && a[6] == PyValue::Int(rat)));
        let t0 = crate::pyutil::filetime_now();
        let period = kind().weapon_period();
        let mut log = Vec::new();
        for k in 0..40 {
            tick(&mut reg, t0 + k * period);
            log.extend(due(&mut reg));
            if reg.roster().get(rat).is_none() {
                break;
            }
        }
        assert!(reg.roster().get(rat).is_none(), "the rat is dead");
        let fx = log
            .iter()
            .find(|(_, n, _)| *n == "OnSpecialFX")
            .expect("the gun's visual");
        assert_eq!(fx.2[0], PyValue::Int(id), "shipID");
        assert_eq!(fx.2[1], PyValue::Int(id), "moduleID is the drone");
        assert!(
            log.iter()
                .any(|(to, n, _)| *n == "OnDamageMessage" && to == &vec![ALICE])
        );
        // Next tick: no target, so home.
        tick(&mut reg, t0 + 41 * period);
        let d = reg.modules().drones().get(id).copied().expect("still out");
        assert_eq!(d.order, Order::Idle);
        assert_eq!(d.firing, None);
        assert_eq!(census(&reg), (0, 1, 20));
    }

    /// The `(funcName, args)` of every entry in a `DoDestinyUpdate`.
    fn destiny_calls(args: &[PyValue]) -> Vec<(Vec<u8>, Vec<PyValue>)> {
        let PyValue::List(entries) = &args[0] else {
            panic!("a batch is a list")
        };
        entries
            .iter()
            .map(|e| {
                let PyValue::Tuple(e) = e else {
                    panic!("(stamp, call)")
                };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!("(name, args)")
                };
                let (PyValue::Bytes(name), PyValue::Tuple(a)) = (&call[0], &call[1]) else {
                    panic!("(str, args)")
                };
                (name.clone(), a.clone())
            })
            .collect()
    }

    /// **A drone shot to zero is gone** (`drones.md` §4): Bob's volleys go
    /// through `combat::fire` as any ship's do, and the killing one sends
    /// `RemoveBall(droneID, True)` -- the terminal singular that plays the
    /// explosion (`michelle.py:1762`) [V] -- alone, because a light drone
    /// has no `wreckTypeID` [V-data]; Bob's box is lost with `'Exploding'`;
    /// Alice's client is sent the row that drops it from the drone window
    /// (no owner, no controller: `michelle.py:2028-2035`) [V]; and the item
    /// is destroyed -- what Alice owns is one drone fewer, nowhere else.
    #[test]
    fn a_drone_shot_to_zero_explodes_and_is_gone() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        undock(&mut reg, BOB);
        let id = launch_n(&mut reg, 1)[0];
        assert_eq!(census(&reg), (0, 1, 20));
        reg.locks_mut().add(BOB_SHIP, id);
        let _ = reg.take_pending();
        let _ = due(&mut reg);
        let t0 = crate::pyutil::filetime_now();
        for _ in 0..1_000 {
            let Some(victim) = reg.roster().get(id).copied() else {
                break;
            };
            let shot = Shot {
                at: t0,
                attacker_ship_id: BOB_SHIP,
                weapon_item_id: 1,
                weapon_type_id: GUN_TYPE,
                target_ship_id: id,
                damage: Damage {
                    em: 0.0,
                    explosive: 0.0,
                    kinetic: 50.0,
                    thermal: 0.0,
                },
                hit_quality: crate::combat::HIT_QUALITY,
            };
            crate::combat::fire(&mut reg, shot, BOB, victim, t0);
        }
        assert!(reg.roster().get(id).is_none(), "out of space");
        assert!(reg.modules().drones().get(id).is_none(), "out of the table");
        assert!(reg.combat().ship(id).is_none(), "no health record");
        assert_eq!(census(&reg), (0, 0, 20), "destroyed, not moved");
        assert!(reg.store().get(id).is_none());

        let out = due(&mut reg);
        let kill = out
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate" && destiny_calls(a).iter().any(|(f, _)| f == b"RemoveBall")
            })
            .expect("a kill batch");
        let mut park = kill.0.clone();
        park.sort_unstable();
        assert_eq!(park, vec![ALICE, BOB]);
        let calls = destiny_calls(&kill.2);
        assert_eq!(calls.len(), 1, "no wreck, no capsule");
        assert_eq!(calls[0].1, vec![PyValue::Int(id), PyValue::Bool(true)]);
        let row = out
            .iter()
            .find(|(to, n, a)| {
                *n == "OnDroneStateChange" && to == &vec![ALICE] && a[0] == PyValue::Int(id)
            })
            .expect("Alice's drone window is told");
        assert_eq!((&row.2[1], &row.2[2]), (&PyValue::None, &PyValue::None));
        let lost = out
            .iter()
            .find(|(to, n, a)| {
                *n == "OnTarget" && to == &vec![BOB] && a[0] == PyValue::Bytes(b"lost".to_vec())
            })
            .expect("Bob's target box is told");
        assert_eq!(lost.2[1], PyValue::Int(id));
        assert_eq!(lost.2[2], PyValue::Bytes(b"Exploding".to_vec()));
        assert!(
            out.iter().all(|(to, _, _)| !to.contains(&id)),
            "nothing addressed to the dead drone"
        );
        // The next tick has nothing left to fly.
        tick(&mut reg, t0);
        assert!(reg.modules().drones().is_empty());
    }

    #[test]
    fn nobody_engages_with_someone_elses_drones() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        undock(&mut reg, BOB);
        let id = launch_n(&mut reg, 1)[0];
        let refused = engage(&mut reg, Some(BOB), &[id], ALICE_SHIP);
        let PyValue::Dict(pairs) = refused else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, PyValue::Int(id));
        assert_eq!(
            reg.modules().drones().get(id).map(|d| d.order),
            Some(Order::Idle)
        );
    }

    /// Recall: the clock flies it home and takes it aboard -- the item back
    /// in the bay, the ball out of every park, and the owner's row cleared
    /// with no owner and no controller (`michelle.py:2028-2035`) [V].
    #[test]
    fn a_recalled_drone_is_scooped_by_the_clock_and_conserved() {
        let mut reg = world(BIG_BAY, 2, 5);
        undock(&mut reg, ALICE);
        undock(&mut reg, BOB);
        let ids = launch_n(&mut reg, 2);
        let _ = reg.take_pending();
        assert_eq!(census(&reg), (0, 2, 20));
        let refused = return_bay(&mut reg, Some(ALICE), &ids);
        assert_eq!(refused, PyValue::Dict(vec![]));
        let _ = reg.take_pending();
        tick(&mut reg, crate::pyutil::filetime_now());
        let sent = due(&mut reg);
        assert_eq!(census(&reg), (2, 0, 20));
        for id in &ids {
            assert!(reg.roster().get(*id).is_none());
            assert!(reg.modules().drones().get(*id).is_none());
            assert!(sent.iter().any(|(to, n, a)| *n == "OnDroneStateChange"
                && to == &vec![ALICE]
                && a[0] == PyValue::Int(*id)
                && a[1] == PyValue::None
                && a[2] == PyValue::None));
        }
        let removed = sent
            .iter()
            .filter(|(_, n, _)| *n == "DoDestinyUpdate")
            .count();
        assert!(removed >= 1);
        assert!(
            sent.iter()
                .any(|(to, n, _)| *n == "OnItemChange" && to == &vec![ALICE])
        );
        // And they launch again, from wherever in the bay they landed.
        let bay: Vec<(i64, i64)> = reg
            .store()
            .at(Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY))
            .map(|i| (i.item_id, i.quantity))
            .collect();
        launch(&mut reg, Some(ALICE), &bay);
        assert_eq!(census(&reg), (0, 2, 20));
    }

    #[test]
    fn a_scoop_takes_a_drone_near_its_ship_and_refuses_one_far_away() {
        let mut reg = world(BIG_BAY, 2, 5);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 2);
        // Move the second far out.
        if let Some(p) = reg.roster_mut().get_mut(ids[1]) {
            p.motion = Motion::arrived([SCOOP_RANGE * 10.0, 0.0, 0.0], [0.0; 3]);
        }
        let refused = scoop_now(&mut reg, Some(ALICE), &ids);
        let PyValue::Dict(pairs) = refused else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, PyValue::Int(ids[1]));
        assert_eq!(census(&reg), (1, 1, 20));
        assert!(reg.modules().drones().get(ids[0]).is_none());
        assert!(reg.modules().drones().get(ids[1]).is_some());
    }

    /// **A scoop does not mend**: the damage a drone carries home is kept
    /// with the item, comes back at the next launch, and a whole drone
    /// leaves nothing behind.
    #[test]
    fn a_scooped_drone_keeps_its_damage_until_the_next_launch() {
        let mut reg = world(BIG_BAY, 2, 5);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 2);
        let (hurt, whole) = (ids[0], ids[1]);
        let now = crate::pyutil::filetime_now();
        let profile = profile_of(&reg, hurt).expect("a profile");
        reg.combat_mut().seed_ship(
            hurt,
            crate::damage::ShipDamage::with_damage(profile, now, 7.0, 3.0),
        );
        let refused = scoop_now(&mut reg, Some(ALICE), &ids);
        assert_eq!(refused, PyValue::Dict(vec![]), "both came home");
        assert!(
            reg.combat().ship(hurt).is_none(),
            "the record went with the ball"
        );
        let h = crate::repair::hurt(&reg, hurt).expect("stashed with the item");
        assert_eq!((h.armor, h.structure), (7.0, 3.0));
        assert!(
            crate::repair::hurt(&reg, whole).is_none(),
            "nothing for a whole drone"
        );

        let _ = launch(&mut reg, Some(ALICE), &[(hurt, 1), (whole, 1)]);
        let r = reg
            .combat()
            .ship(hurt)
            .expect("seeded at launch")
            .rings(now);
        assert_eq!((r.armor_damage, r.damage), (7.0, 3.0));
        assert!(
            reg.combat().ship(whole).is_none(),
            "a whole drone has no record"
        );
        assert!(
            crate::repair::hurt(&reg, hurt).is_none(),
            "the record is the truth in space"
        );
    }

    /// A restart forgets every ball; a drone item it left in space goes
    /// home rather than vanishing from every count.
    #[test]
    fn a_drone_item_left_in_space_by_a_restart_is_swept_home() {
        let mut reg = world(BIG_BAY, 1, 5);
        let mut items = reg.store().items().to_vec();
        for i in items.iter_mut().filter(|i| i.item_id == STACK) {
            i.location_id = SYSTEM;
            i.flag_id = FLAG_IN_SPACE;
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: STACK + 11,
            ..crate::store::Snapshot::default()
        });
        assert_eq!(census(&reg), (0, 1, 20));
        tick(&mut reg, crate::pyutil::filetime_now());
        assert_eq!(census(&reg), (0, 0, 21));
        assert_eq!(
            reg.store().get(STACK).map(|i| (i.location_id, i.flag_id)),
            Some((STATION, crate::model::FLAG_HANGAR))
        );
    }

    // -- the wire -----------------------------------------------------------

    fn request<'a>(
        service: &'a str,
        method: &'a str,
        args: &'a [PyValue],
        actor: Option<i64>,
    ) -> Request<'a> {
        Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor,
        }
    }

    #[test]
    fn the_entity_object_answers_engage_and_recall_with_an_error_dict() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let args = [PyValue::List(vec![PyValue::Int(999)])];
        let launched = launch_call(
            &mut reg,
            &request(
                "ship",
                "LaunchDrones",
                &[
                    PyValue::List(vec![PyValue::Tuple(vec![
                        PyValue::Int(STACK),
                        PyValue::Int(1),
                    ])]),
                    PyValue::None,
                    PyValue::Int(0),
                ],
                Some(ALICE),
            ),
        )
        .expect("an answer");
        let id = launched_ids(&launched)[0];
        let ids = [PyValue::List(vec![PyValue::Int(id)])];
        for method in ["CmdReturnHome", "CmdReturnBay"] {
            let r = reg
                .dispatch(&request("entity", method, &ids, Some(ALICE)))
                .expect("answered");
            assert_eq!(r, PyValue::Dict(vec![]), "{method}");
        }
        // An unknown drone is one refusal, keyed by its id.
        let r = dispatch(
            &mut reg,
            &request("entity", "CmdReturnHome", &args, Some(ALICE)),
        )
        .expect("answered");
        let PyValue::Dict(pairs) = r else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
    }

    // -- mining drones (SOC-B10) ---------------------------------------------

    /// The invented mining drone and ore of [`ROWS`].
    const MINING_DRONE: i64 = 990_580_012;
    const ORE: i64 = 990_580_020;
    const ROCK: i64 = crate::asteroid::MIN_ROCK_ID;
    /// A fixture law, not any ore's.
    const LAW: crate::asteroid::Law = crate::asteroid::Law {
        growth: 0.5,
        unit_size: 90.0,
        max_radius: 16_255.0,
    };

    /// Alice in space in the big-bay hull with `n` mining drones launched,
    /// and one rock of `quantity` units whose surface is `gap` metres from
    /// the first drone. Answers the drone ids.
    fn mining_world(quantity: i64, gap: f64, n: i64) -> (Registry, Vec<i64>) {
        let mut reg = world(BIG_BAY, 0, 5);
        reg.apply_items(&[Edit::Create {
            type_id: MINING_DRONE,
            quantity: n,
            to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
        }])
        .expect("the bay takes them");
        let stack = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == MINING_DRONE)
            .map(|i| i.item_id)
            .expect("the stack");
        undock(&mut reg, ALICE);
        let ids = launched_ids_of(&launch(&mut reg, Some(ALICE), &[(stack, n)]), stack);
        assert_eq!(ids.len() as i64, n, "every mining drone flies");
        let at = reg.roster().get(ids[0]).expect("in space").motion.position;
        let radius = LAW.radius(quantity);
        *reg.rocks_mut() = crate::asteroid::Field::from_rocks([crate::asteroid::Rock {
            id: ROCK,
            type_id: ORE,
            belt_id: 40_000_003,
            solar_system_id: SYSTEM,
            quantity,
            radius,
            position: [at[0] + radius + gap, at[1], at[2]],
            law: LAW,
        }]);
        let _ = reg.take_pending();
        let _ = due(&mut reg);
        (reg, ids)
    }

    fn launched_ids_of(answer: &PyValue, stack: i64) -> Vec<i64> {
        let PyValue::Dict(pairs) = answer else {
            panic!("a dict")
        };
        pairs
            .iter()
            .filter(|(_, k)| *k == PyValue::Int(stack))
            .flat_map(|(v, _)| match v {
                PyValue::List(v) => v.clone(),
                _ => vec![],
            })
            .filter_map(|e| match e {
                PyValue::Int(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn ore_in_hold(reg: &Registry) -> i64 {
        reg.store()
            .at(Slot::new(ALICE, ALICE_SHIP, crate::fitting::FLAG_CARGO))
            .filter(|i| i.type_id == ORE)
            .map(|i| i.quantity)
            .sum()
    }

    /// **A mining drone is a drone type**, read off its own row: its cycle
    /// is attribute 73, its optimal `maxRange`, it does no damage, and an
    /// orbit has a speed even though the row has no cruise speed.
    #[test]
    fn a_mining_drone_flies_with_its_cycle_and_its_range() {
        let c = content();
        let t = typedata::installed_in(&c).table();
        let k = find_in(&c, MINING_DRONE).expect("a mining drone flies");
        assert!(is_miner(&c, MINING_DRONE));
        assert!(!is_miner(&c, DRONE));
        assert_eq!(
            k.weapon_period_ms,
            t.attribute(MINING_DRONE, ATTR_DURATION) as i64
        );
        assert_eq!(k.optimal, t.attribute(MINING_DRONE, attr::MAX_RANGE));
        assert_eq!(k.volley().total(), 0.0);
        assert_eq!(k.cruise_fraction, CRUISE_FRACTION_DEFAULT);
    }

    /// **`CmdMineRepeatedly` and the clock**: the drone is told mining
    /// (`entityMining`, the rock as target), its beam names the drone as its
    /// module with the mining guid, and each cycle's end puts
    /// `floor(miningAmount / volume)` units of the rock's ore into the
    /// controller's hold -- the first one period after the beam starts, and
    /// what leaves the rock is what arrives.
    #[test]
    fn mining_drones_cut_ore_into_their_ships_hold_on_their_cycle() {
        let q0 = 100_000;
        let (mut reg, ids) = mining_world(q0, 1_000.0, 2);
        let answer = dispatch(
            &mut reg,
            &request(
                "entity",
                "CmdMineRepeatedly",
                &[
                    PyValue::List(ids.iter().map(|i| PyValue::Int(*i)).collect()),
                    PyValue::Int(ROCK),
                ],
                Some(ALICE),
            ),
        )
        .expect("answered");
        assert_eq!(answer, PyValue::Dict(vec![]));
        let sent = pending(&mut reg);
        assert!(sent.iter().any(|(to, n, a)| *n == "OnDroneStateChange"
            && to.as_deref() == Some(&[ALICE][..])
            && a[3] == PyValue::Int(ENTITY_MINING)
            && a[6] == PyValue::Int(ROCK)));

        let k = find_in(reg.content(), MINING_DRONE).expect("kind");
        let period = k.weapon_period();
        let per_cycle = crate::mining::units_in(
            typedata::attribute_in(
                reg.content(),
                MINING_DRONE,
                crate::mining::ATTR_MINING_AMOUNT,
            )
            .expect("miningAmount"),
            typedata::item_in(reg.content(), ORE).expect("ore").volume,
        );
        assert!(per_cycle > 0);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        let log = due(&mut reg);
        assert_eq!(ore_in_hold(&reg), 0, "nothing before a cycle ends");
        let beam = log
            .iter()
            .find(|(_, n, a)| *n == "OnSpecialFX" && a[0] == PyValue::Int(ids[0]))
            .expect("the beam starts");
        assert_eq!(beam.2[1], PyValue::Int(ids[0]), "moduleID is the drone");
        assert_eq!(beam.2[5], PyValue::Bytes(MINING_GUID.as_bytes().to_vec()));
        tick(&mut reg, t0 + period);
        assert_eq!(ore_in_hold(&reg), 2 * per_cycle, "one cycle each");
        tick(&mut reg, t0 + 2 * period);
        assert_eq!(ore_in_hold(&reg), 4 * per_cycle);
        assert_eq!(
            reg.rocks().get(ROCK).expect("rock").quantity,
            q0 - ore_in_hold(&reg),
            "ore is conserved"
        );
        assert!(
            ids.iter()
                .all(|id| reg.modules().drones().get(*id).map(|d| d.order)
                    == Some(Order::Mine(ROCK)))
        );
    }

    /// **A mining drone flies to a rock beyond its reach**: the type rows
    /// have no mass, and a ball with no mass cannot move, so a drone
    /// launched farther from the rock than its optimal range never got
    /// there. Every other mining test starts a kilometre away.
    #[test]
    fn a_mining_drone_closes_on_a_rock_beyond_its_reach() {
        crate::test_clock::freeze(std::time::Duration::from_secs(1_800_000_000));
        let reach = find_in(&content(), MINING_DRONE).expect("kind").optimal;
        let (mut reg, ids) = mining_world(100_000, reach + 3_000.0, 1);
        let r = mine(&mut reg, Some(ALICE), &ids, ROCK);
        assert_eq!(r, PyValue::Dict(vec![]));
        let rock = reg.rocks().get(ROCK).expect("rock").position;
        let gap = |reg: &Registry| {
            let p = reg.roster().get(ids[0]).expect("in space").motion.position;
            (0..3).map(|i| (p[i] - rock[i]).powi(2)).sum::<f64>().sqrt()
        };
        let start = gap(&reg);
        for _ in 0..300 {
            crate::test_clock::step(std::time::Duration::from_secs(1));
            tick(&mut reg, crate::pyutil::filetime_now());
            if ore_in_hold(&reg) > 0 {
                break;
            }
        }
        let end = gap(&reg);
        crate::test_clock::thaw();
        assert!(end < start, "the drone flew from {start} m to {end} m");
        assert!(ore_in_hold(&reg) > 0, "and cut ore");
    }

    /// **A rock mined out**: every unit in the hold, the rock gone, and the
    /// drone back to orbiting its ship on the next tick.
    #[test]
    fn a_rock_mined_out_sends_the_mining_drone_home() {
        let (mut reg, ids) = mining_world(300, 1_000.0, 1);
        let r = mine(&mut reg, Some(ALICE), &ids, ROCK);
        assert_eq!(r, PyValue::Dict(vec![]));
        let period = find_in(reg.content(), MINING_DRONE)
            .expect("kind")
            .weapon_period();
        let t0 = crate::pyutil::filetime_now();
        for k in 0..=4 {
            tick(&mut reg, t0 + k * period);
        }
        assert_eq!(ore_in_hold(&reg), 300, "every unit, and no more");
        assert!(reg.rocks().get(ROCK).is_none());
        let d = reg.modules().drones().get(ids[0]).copied().expect("out");
        assert_eq!(d.order, Order::Idle);
        assert_eq!(d.firing, None);
    }

    /// **A full hold stops the drone**: the hold is never over capacity, and
    /// the drone is idle with its beam off.
    #[test]
    fn a_full_hold_stops_the_mining_drone() {
        let (mut reg, ids) = mining_world(100_000, 1_000.0, 1);
        let capacity = typedata::item_in(reg.content(), BIG_BAY)
            .expect("hull")
            .capacity;
        let unit = typedata::item_in(reg.content(), ORE).expect("ore").volume;
        // Room for less than one cycle.
        reg.apply_items(&[Edit::Create {
            type_id: ORE,
            quantity: crate::mining::units_in(capacity, unit) - 10,
            to: Slot::new(ALICE, ALICE_SHIP, crate::fitting::FLAG_CARGO),
        }])
        .expect("filled");
        mine(&mut reg, Some(ALICE), &ids, ROCK);
        let period = find_in(reg.content(), MINING_DRONE)
            .expect("kind")
            .weapon_period();
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        tick(&mut reg, t0 + period);
        assert!(
            reg.store()
                .used_volume_in(reg.content(), ALICE_SHIP, crate::fitting::FLAG_CARGO)
                <= capacity + 1e-9
        );
        let d = reg.modules().drones().get(ids[0]).copied().expect("out");
        assert_eq!(d.order, Order::Idle, "stopped by the full hold");
        assert_eq!(d.firing, None);
    }

    /// The refusals, each keyed by the drone: a combat drone told to mine,
    /// a mining drone told to engage, and a target that is not a rock here.
    #[test]
    fn mining_is_for_mining_drones_on_rocks_only() {
        let (mut reg, ids) = mining_world(100_000, 1_000.0, 1);
        // Not a rock.
        let r = mine(&mut reg, Some(ALICE), &ids, BOB_SHIP);
        let PyValue::Dict(pairs) = r else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
        // A mining drone does not engage: Bob in space beside it.
        undock(&mut reg, BOB);
        let r = engage(&mut reg, Some(ALICE), &ids, BOB_SHIP);
        let PyValue::Dict(pairs) = r else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            reg.modules().drones().get(ids[0]).map(|d| d.order),
            Some(Order::Idle)
        );
        // A combat drone does not mine.
        reg.apply_items(&[Edit::Create {
            type_id: DRONE,
            quantity: 1,
            to: Slot::new(ALICE, ALICE_SHIP, FLAG_DRONE_BAY),
        }])
        .expect("bay");
        let stack = reg
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == DRONE && i.flag_id == FLAG_DRONE_BAY)
            .map(|i| i.item_id)
            .expect("stack");
        let gun = launched_ids_of(&launch(&mut reg, Some(ALICE), &[(stack, 1)]), stack);
        let r = mine(&mut reg, Some(ALICE), &gun, ROCK);
        let PyValue::Dict(pairs) = r else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 1);
    }

    // -- K-DRONE: skills, hull bonuses, hitting, aggression, loss -------------

    /// Alice with every skill the tracking drone wants, at these levels.
    fn skilled(light: i64, iface: i64) -> Registry {
        world_with(
            BIG_BAY,
            3,
            &[
                (SKILL_DRONES, 5),
                (SK_DRONES, 1),
                (SK_LIGHT, light),
                (SK_IFACE, iface),
            ],
            TRACK_DRONE,
        )
    }

    fn row_attr(reg: &Registry, type_id: i64, attr: i64) -> f64 {
        typedata::installed_in(reg.content())
            .table()
            .attribute(type_id, attr)
    }

    fn census_of(reg: &Registry, ty: i64) -> (i64, i64, i64) {
        let mut out = (0, 0, 0);
        for i in reg
            .store()
            .items()
            .iter()
            .filter(|i| i.owner_id == ALICE && i.type_id == ty)
        {
            if i.flag_id == FLAG_DRONE_BAY {
                out.0 += i.quantity;
            } else if i.location_id == SYSTEM && i.flag_id == FLAG_IN_SPACE {
                out.1 += i.quantity;
            } else {
                out.2 += i.quantity;
            }
        }
        out
    }

    /// **A drone flies with its pilot's skills and its hull's bonus**: the
    /// Interfacing-like skill (a modifier in the data, level times its
    /// bonus), the Light-Operation-like one (the Python effect 1730, which
    /// the engine does not know), and the carrier's role bonus on the
    /// owner's drones -- multiplying the type's own `damageMultiplier`.
    #[test]
    fn a_launched_drone_carries_its_pilots_skills_and_its_hulls_bonus() {
        let (light, iface) = (2, 3);
        let mut reg = skilled(light, iface);
        undock(&mut reg, ALICE);
        let id = launch_n_of(&mut reg, 1)[0];
        let d = reg.modules().drones().get(id).copied().expect("out");
        let base = row_attr(&reg, TRACK_DRONE, 64);
        let bonus =
            |skill: i64, level: i64| 1.0 + row_attr(&reg, skill, 292) * level as f64 / 100.0;
        let hull = 1.0 + row_attr(&reg, BIG_BAY, 658) / 100.0;
        let want = base * bonus(SK_LIGHT, light) * bonus(SK_IFACE, iface) * hull;
        assert!(
            (d.kind.damage_multiplier - want).abs() < 1e-9,
            "{} vs {want}",
            d.kind.damage_multiplier
        );
        let v = d.kind.volley();
        assert!((v.kinetic - row_attr(&reg, TRACK_DRONE, 117) * want).abs() < 1e-9);
        assert_eq!(d.kind.tracking, row_attr(&reg, TRACK_DRONE, 160));
        assert_eq!(d.kind.sig_resolution, row_attr(&reg, TRACK_DRONE, 620));
    }

    /// An untrained Interfacing leaves only the other two factors.
    #[test]
    fn a_pilot_without_a_bonus_skill_gets_the_other_factors_only() {
        let mut reg = skilled(2, 0);
        undock(&mut reg, ALICE);
        let id = launch_n_of(&mut reg, 1)[0];
        let d = reg.modules().drones().get(id).copied().expect("out");
        let want = row_attr(&reg, TRACK_DRONE, 64)
            * (1.0 + row_attr(&reg, SK_LIGHT, 292) * 2.0 / 100.0)
            * (1.0 + row_attr(&reg, BIG_BAY, 658) / 100.0);
        assert!((d.kind.damage_multiplier - want).abs() < 1e-9);
    }

    /// A drone whose required skills the pilot lacks is refused, and
    /// nothing leaves the bay.
    #[test]
    fn a_drone_needs_its_required_skills_to_launch() {
        let mut reg = world_with(
            BIG_BAY,
            3,
            &[(SKILL_DRONES, 5), (SK_DRONES, 1), (SK_LIGHT, 1)],
            TRACK_DRONE,
        );
        undock(&mut reg, ALICE);
        let level = row_attr(&reg, TRACK_DRONE, 278) as i64;
        assert_eq!(
            drone_skills::missing_skill(&reg, ALICE, TRACK_DRONE),
            Some((SK_LIGHT, level))
        );
        assert!(launch_n_of(&mut reg, 1).is_empty());
        assert_eq!(census_of(&reg, TRACK_DRONE), (3, 0, 20));
    }

    /// Drone Avionics-like skills extend the control range per level.
    #[test]
    fn avionics_extend_the_control_range_per_level() {
        let reg = world_with(BIG_BAY, 0, &[(SK_AVIONICS, 3), (SK_DRONES, 1)], TRACK_DRONE);
        let per = row_attr(&reg, SK_AVIONICS, 459);
        assert_eq!(
            drone_skills::control_range(&reg, ALICE, ALICE_SHIP),
            CONTROL_RANGE + per * 3.0
        );
        let none = world_with(BIG_BAY, 0, &[], TRACK_DRONE);
        assert_eq!(
            drone_skills::control_range(&none, ALICE, ALICE_SHIP),
            CONTROL_RANGE
        );
    }

    /// The drone's health as it flies is the launch's: `dogma` asks.
    #[test]
    fn a_drones_hull_and_profile_are_the_launched_ones() {
        let mut reg = skilled(2, 3);
        undock(&mut reg, ALICE);
        let id = launch_n_of(&mut reg, 1)[0];
        let h = hull_of(&reg, id).expect("a drone");
        assert_eq!(h.hp, row_attr(&reg, TRACK_DRONE, 9));
        let p = profile_of(&reg, id).expect("a drone");
        assert_eq!(p.armor_hp, row_attr(&reg, TRACK_DRONE, 265));
        assert!(hull_of(&reg, ALICE_SHIP).is_none());
        assert_eq!(crate::dogma::hull(&reg, id), Some(h));
    }

    fn launch_n_of(reg: &mut Registry, n: i64) -> Vec<i64> {
        launch_n(reg, n)
    }

    // -- aggressive and passive ---------------------------------------------

    fn threatened(aggressive: bool) -> (Drone, View) {
        let at = [1_000.0, 0.0, 0.0];
        (
            a_drone(),
            View {
                aggressive,
                threat: Some(Threat { id: 777, at }),
                ..view(None)
            },
        )
    }

    /// **An aggressive idle drone takes on what attacks its ship**; a
    /// passive one orbits and does nothing.
    #[test]
    fn an_aggressive_idle_drone_engages_a_threat_and_a_passive_one_does_not() {
        let (d, v) = threatened(true);
        let (d, out) = decide(&d, &v);
        assert_eq!(d.order, Order::Engage(777));
        assert!(
            out.iter().any(|i| matches!(
                i,
                Intent::State {
                    target: Some(777),
                    ..
                }
            )),
            "{out:?}"
        );
        let (d, v) = threatened(false);
        let (d, out) = decide(&d, &v);
        assert_eq!(d.order, Order::Idle);
        assert!(out.contains(&Intent::Orbit(ALICE_SHIP)));
    }

    /// A drone told to return to the bay is not diverted.
    #[test]
    fn a_recalled_drone_ignores_threats() {
        let (d, v) = threatened(true);
        let d = Drone {
            order: Order::ReturnBay,
            ..d
        };
        let (d, _) = decide(&d, &v);
        assert_eq!(d.order, Order::ReturnBay);
    }

    /// A fight dragged beyond the control range is let go.
    #[test]
    fn a_target_beyond_the_control_range_is_let_go() {
        let d = Drone {
            order: Order::Engage(9),
            ..a_drone()
        };
        let far = View {
            target: Some([CONTROL_RANGE * 2.0, 0.0, 0.0]),
            ..view(None)
        };
        let (d, _) = decide(&d, &far);
        assert_eq!(d.order, Order::Idle);
    }

    /// `ChangeDroneSettings` keeps the pilot's two flags; the default is
    /// aggressive without focus fire.
    #[test]
    fn the_settings_call_keeps_the_pilots_flags() {
        let mut reg = world(BIG_BAY, 1, 5);
        assert_eq!(
            reg.modules().drones().settings_of(ALICE),
            Settings {
                aggressive: true,
                focus_fire: false
            }
        );
        let sent = PyValue::Dict(vec![
            (
                PyValue::Bool(false),
                PyValue::Int(drone_skills::ATTR_AGGRESSIVE),
            ),
            (
                PyValue::Bool(true),
                PyValue::Int(drone_skills::ATTR_FOCUS_FIRE),
            ),
        ]);
        assert_eq!(change_settings(&mut reg, Some(ALICE), &sent), PyValue::None);
        assert_eq!(
            reg.modules().drones().settings_of(ALICE),
            Settings {
                aggressive: false,
                focus_fire: true
            }
        );
        assert_eq!(reg.modules().drones().settings_of(BOB), Settings::default());
    }

    /// In the world: a rat whose gun is on Alice draws an aggressive
    /// pilot's idle drone within a tick; a passive pilot's stays home.
    #[test]
    fn an_npc_attacking_the_ship_draws_aggressive_drones_in() {
        for (aggressive, engaged) in [(true, true), (false, false)] {
            let mut reg = world(BIG_BAY, 1, 5);
            undock(&mut reg, ALICE);
            let id = launch_n(&mut reg, 1)[0];
            let rat = rat_beyond(&mut reg, id, kind().optimal * 0.4);
            if !aggressive {
                let sent = PyValue::Dict(vec![(
                    PyValue::Bool(false),
                    PyValue::Int(drone_skills::ATTR_AGGRESSIVE),
                )]);
                change_settings(&mut reg, Some(ALICE), &sent);
            }
            let t0 = crate::pyutil::filetime_now();
            tick(&mut reg, t0);
            assert_eq!(
                reg.modules().drones().get(id).map(|d| d.order),
                Some(Order::Idle),
                "nobody is shooting yet"
            );
            if let Some(n) = reg.npcs().get(rat).copied() {
                reg.npcs_mut().insert(crate::npc::Npc {
                    target: Some(ALICE_SHIP),
                    ..n
                });
            }
            assert_eq!(threats_on(&reg, ALICE_SHIP, ALICE, SYSTEM, t0).len(), 1);
            tick(&mut reg, t0 + 1);
            let order = reg.modules().drones().get(id).map(|d| d.order);
            assert_eq!(
                order == Some(Order::Engage(rat)),
                engaged,
                "{aggressive}: {order:?}"
            );
        }
    }

    // -- hitting -----------------------------------------------------------

    /// With tracking in the data, the drones kill a rat all the same.
    #[test]
    fn tracking_drones_still_kill_a_rat() {
        let mut reg = skilled(2, 3);
        undock(&mut reg, ALICE);
        let id = launch_n_of(&mut reg, 1)[0];
        let rat = rat_beyond(&mut reg, id, 500.0);
        assert_eq!(
            engage(&mut reg, Some(ALICE), &[id], rat),
            PyValue::Dict(vec![])
        );
        let t0 = crate::pyutil::filetime_now();
        let period = reg
            .modules()
            .drones()
            .get(id)
            .expect("out")
            .kind
            .weapon_period();
        for k in 0..400 {
            tick(&mut reg, t0 + k * period);
            if reg.roster().get(rat).is_none() {
                break;
            }
        }
        assert!(reg.roster().get(rat).is_none(), "the rat is dead");
    }

    // -- a controller that is gone -------------------------------------------

    /// **A drone left alone is taken out of space** after [`ABANDON_AFTER`]:
    /// a ship that still exists gets it back in the hangar, and nothing
    /// happens before then.
    #[test]
    fn a_drone_whose_ship_left_is_returned_to_the_hangar_after_the_grace() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        reg.roster_mut().leave(ALICE);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        assert_eq!(census(&reg), (0, 1, 20));
        assert_eq!(
            reg.modules().drones().get(id).and_then(|d| d.lost_since),
            Some(t0)
        );
        tick(&mut reg, t0 + ABANDON_AFTER - 1);
        assert_eq!(census(&reg), (0, 1, 20), "still waiting");
        tick(&mut reg, t0 + ABANDON_AFTER);
        assert!(reg.modules().drones().get(id).is_none());
        assert!(reg.roster().get(id).is_none());
        assert_eq!(census(&reg), (0, 0, 21), "back in the hangar");
    }

    /// A controller that comes back in time keeps its drones.
    #[test]
    fn a_controller_that_returns_in_time_keeps_its_drones() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        let ship = reg.roster().get(ALICE).copied().expect("in space");
        reg.roster_mut().leave(ALICE);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        reg.roster_mut().arrive(ship);
        tick(&mut reg, t0 + 1);
        assert_eq!(
            reg.modules().drones().get(id).and_then(|d| d.lost_since),
            None
        );
        tick(&mut reg, t0 + ABANDON_AFTER * 2);
        assert!(reg.modules().drones().get(id).is_some());
    }

    /// **A drone whose ship is destroyed is lost**: the item goes, and what
    /// Alice owns is one drone fewer.
    #[test]
    fn a_drone_whose_ship_is_gone_is_lost() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        if let Some(d) = reg.modules_mut().drones_mut().get_mut(id) {
            d.controller_id = 123_456_789;
        }
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        tick(&mut reg, t0 + ABANDON_AFTER);
        assert!(reg.modules().drones().get(id).is_none());
        assert!(reg.store().get(id).is_none(), "the item is destroyed");
        assert_eq!(census(&reg), (0, 0, 20));
    }

    // -- DRONE-AI: drones that fly --------------------------------------------

    /// **The root cause of the Hulks' still drones**: a drone's ball is its
    /// own type row's, not the 1 m/s fallback hull. The mining drone is not
    /// one of the four hand-copied light drones.
    #[test]
    fn every_drone_type_has_a_ball_that_flies() {
        let c = content();
        for id in [DRONE, MINING_DRONE] {
            let h = destiny::hull::find_in(&c, id).expect("a hull");
            let row = find_in(&c, id).expect("a drone").hull;
            assert_eq!(h.max_velocity, row.max_velocity, "{id}");
            assert!(h.max_velocity > 100.0, "{id} is not the 1 m/s fallback");
            assert_ne!(h, &destiny::hull::FALLBACK);
            assert!(h.mass > 0.0, "{id} has a mass");
            assert!(row.mass > 0.0, "{id} row hull has a mass");
        }
        let h = destiny::hull::find_in(&c, MINING_DRONE).expect("a hull");
        let mut m = Motion::arrived([0.0; 3], [0.0; 3]);
        m.after(Action::GotoPoint {
            ball: 1,
            point: [50_000.0, 0.0, 0.0],
        });
        m.after(Action::SetSpeedFraction {
            ball: 1,
            fraction: 1.0,
        });
        let m = m.advanced(h, 10.0);
        assert!(m.position[0] > 500.0, "flew {} m in 10 s", m.position[0]);
    }

    /// A drone type with no `entityFlyRange` (a mining drone) orbits at the
    /// default, not touching its ship or rock; one with a range keeps it.
    #[test]
    fn a_drone_without_a_fly_range_orbits_at_the_default() {
        let c = content();
        let mining = find_in(&c, MINING_DRONE).expect("a mining drone");
        assert_eq!(mining.orbit_range, ORBIT_RANGE_DEFAULT);
        assert_eq!(kind().orbit_range, 500.0, "its own entityFlyRange");
        assert_eq!(orbit_range_of(90_000.0), ORBIT_RANGE_MAX);
    }

    /// **A launched drone is orbiting its owner at once** -- in the roster,
    /// and in the ball the client is given -- whoever launched it, with no
    /// tick between (EvEJS replays the ORBIT at the AddBalls stamp).
    #[test]
    fn a_launched_drone_orbits_its_owner_from_the_start() {
        let mut reg = world(BIG_BAY, 2, 5);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 2);
        let k = kind();
        for id in ids {
            let p = reg.roster().get(id).copied().expect("in space");
            assert_eq!(
                p.motion.mode,
                Mode::Orbit {
                    ball_id: ALICE_SHIP,
                    range: k.orbit_range as f32
                }
            );
            assert!((f64::from(p.motion.speed_fraction) - k.cruise_fraction).abs() < 1e-6);
            let d = reg.modules().drones().get(id).copied().expect("a drone");
            assert_eq!((d.order, d.phase), (Order::Idle, Phase::Home));
            let ball = ball(&reg, &d, &p.motion);
            assert_eq!(ball.mode, p.motion.mode);
        }
    }

    /// An engaged drone far out flies at the target at full speed; once
    /// inside 80 % of its optimal it orbits at cruise and fires; thrown
    /// back out of the optimal its gun goes off and no volley is fired.
    #[test]
    fn an_engaged_drone_closes_orbits_and_fires_only_in_range() {
        let k = kind();
        let engaged = Drone {
            order: Order::Engage(7),
            phase: Phase::Home,
            ..a_drone()
        };
        let far = [k.optimal * 4.0, 0.0, 0.0];
        let (d, out) = decide(&engaged, &view(Some(far)));
        assert_eq!(d.phase, Phase::Approach);
        assert!(!out.iter().any(|i| matches!(i, Intent::Shoot { .. })));
        let edge = [k.optimal + 10.0, 0.0, 0.0];
        let (d, out) = decide(&d, &view(Some(edge)));
        assert_eq!(d.firing, None);
        assert!(
            !out.iter().any(|i| matches!(
                i,
                Intent::FireStart(_) | Intent::Shoot { .. } | Intent::Orbit(_)
            )),
            "still closing: {out:?}"
        );
        let inside = [k.optimal * 0.5, 0.0, 0.0];
        let (d, out) = decide(&d, &view(Some(inside)));
        assert_eq!(d.phase, Phase::Orbit);
        assert!(out.contains(&Intent::Orbit(7)) && out.contains(&Intent::FireStart(7)));
        // Knocked out of range: the gun stops, and time passing fires nothing.
        let later = View {
            now: 1_000 + 5 * k.weapon_period(),
            ..view(Some(edge))
        };
        let (d, out) = decide(&d, &later);
        assert_eq!(d.firing, None);
        assert!(out.contains(&Intent::FireStop(7)));
        assert!(!out.iter().any(|i| matches!(i, Intent::Shoot { .. })));
    }

    // -- assist and guard (GAP-DRONECMD) -------------------------------------

    /// Alice and Bob in one fleet, both in space.
    fn fleeted(reg: &mut Registry) {
        undock(reg, ALICE);
        undock(reg, BOB);
        let id = reg.fleets_mut().create(ALICE).expect("created");
        reg.fleets_mut()
            .init(ALICE, id, Some(BIG_BAY), None, None)
            .expect("init");
        reg.fleets_mut()
            .invite(ALICE, id, BOB, None, None, None)
            .expect("invited");
        reg.fleets_mut()
            .accept_invite(BOB, id, Some(NO_BAY), None, None)
            .expect("accepted");
        let _ = reg.take_pending();
    }

    fn refused_with(answer: &PyValue, label: &str) -> bool {
        let PyValue::Dict(pairs) = answer else {
            return false;
        };
        pairs.iter().any(|(e, _)| match e {
            PyValue::Tuple(t) => t.first() == Some(&crate::pyutil::bstr(label)),
            _ => false,
        })
    }

    #[test]
    fn every_entity_command_answers_a_dict_never_none() {
        let mut reg = world(BIG_BAY, 2, 5);
        undock(&mut reg, ALICE);
        let ids = launch_n(&mut reg, 2);
        let list = PyValue::List(ids.iter().map(|i| PyValue::Int(*i)).collect());
        let calls: Vec<(&str, Vec<PyValue>)> = vec![
            ("CmdAssist", vec![PyValue::Int(ALICE), list.clone()]),
            ("CmdGuard", vec![PyValue::Int(ALICE), list.clone()]),
            ("CmdSalvage", vec![list.clone(), PyValue::Int(777)]),
            ("CmdUnanchor", vec![list.clone(), PyValue::Int(777)]),
            ("CmdAbandonDrone", vec![list.clone()]),
            ("CmdReconnectToDrones", vec![list.clone()]),
        ];
        for (method, args) in &calls {
            let answer = dispatch(&mut reg, &request("entity", method, args, Some(ALICE)))
                .unwrap_or_else(|e| panic!("{method}: {e:?}"));
            assert!(matches!(answer, PyValue::Dict(_)), "{method}: {answer:?}");
        }
        // The three that do nothing say so, once per drone.
        for method in ["CmdSalvage", "CmdUnanchor", "CmdAbandonDrone"] {
            let args = if method == "CmdAbandonDrone" {
                vec![list.clone()]
            } else {
                vec![list.clone(), PyValue::Int(777)]
            };
            let PyValue::Dict(d) =
                dispatch(&mut reg, &request("entity", method, &args, Some(ALICE)))
                    .expect("answered")
            else {
                panic!("a dict")
            };
            assert_eq!(d.len(), ids.len(), "{method}");
        }
    }

    #[test]
    fn assist_on_a_non_fleet_member_is_refused_with_the_client_key() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        undock(&mut reg, BOB);
        let id = launch_n(&mut reg, 1)[0];
        let answer = escort(&mut reg, Some(ALICE), BOB, &[id], false);
        assert!(
            refused_with(&answer, "DroneCommandRequiresShipPilotedFleetMember"),
            "{answer:?}"
        );
        assert_eq!(reg.modules().drones().get(id).and_then(|d| d.escort), None);
        // Themselves is always allowed.
        assert_eq!(
            escort(&mut reg, Some(ALICE), ALICE, &[id], false),
            PyValue::Dict(vec![])
        );
    }

    /// A rat on Bob's ship draws Alice's guarding drone, which flies and
    /// fights as an engaged one (so the rat sees it as provoked).
    #[test]
    fn guarding_drones_engage_an_npc_attacking_the_guarded_ship() {
        let mut reg = world(BIG_BAY, 1, 5);
        fleeted(&mut reg);
        let id = launch_n(&mut reg, 1)[0];
        let rat = rat_beyond(&mut reg, id, kind().optimal * 0.4);
        assert_eq!(
            escort(&mut reg, Some(ALICE), BOB, &[id], true),
            PyValue::Dict(vec![])
        );
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        assert_eq!(
            reg.modules().drones().get(id).map(|d| d.order),
            Some(Order::Idle),
            "nobody attacks Bob yet"
        );
        if let Some(n) = reg.npcs().get(rat).copied() {
            reg.npcs_mut().insert(crate::npc::Npc {
                target: Some(BOB_SHIP),
                ..n
            });
        }
        tick(&mut reg, t0 + 1);
        let d = reg.modules().drones().get(id).copied().expect("out");
        assert_eq!(d.order, Order::Engage(rat));
        assert_eq!(d.escort.map(|e| e.guard), Some(true), "the order stands");
    }

    /// The assist rule is `[A]`: Bob's running offensive module's target.
    #[test]
    fn an_assist_order_engages_the_target_assist_target_returns() {
        let mut reg = world(BIG_BAY, 1, 5);
        fleeted(&mut reg);
        let id = launch_n(&mut reg, 1)[0];
        let rat = rat_beyond(&mut reg, id, kind().optimal * 0.4);
        assert_eq!(
            escort(&mut reg, Some(ALICE), BOB, &[id], false),
            PyValue::Dict(vec![])
        );
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0);
        assert_eq!(
            reg.modules().drones().get(id).map(|d| d.order),
            Some(Order::Idle)
        );
        assert_eq!(assist_target(&reg, BOB_SHIP), None);
        reg.modules_mut().insert_running(crate::modules::Running {
            character_id: BOB,
            ship_id: BOB_SHIP,
            module_id: 990_000_900,
            type_id: GUN_TYPE,
            effect: crate::modules::Effect {
                effect_id: 990_000_901,
                name: "fixture",
                category: crate::modules::CATEGORY_TARGET,
                guid: "",
                is_offensive: true,
                duration_attribute: 73,
                warp_safe: None,
            },
            target_id: Some(rat),
            started: t0,
            period: 5_000,
            repeat: 1000,
            cycles: 1,
            stop_at: None,
        });
        assert_eq!(assist_target(&reg, BOB_SHIP), Some(rat));
        tick(&mut reg, t0 + 1);
        assert_eq!(
            reg.modules().drones().get(id).map(|d| d.order),
            Some(Order::Engage(rat))
        );
        // The owner's own command ends the escort.
        assert_eq!(
            return_home(&mut reg, Some(ALICE), &[id]),
            PyValue::Dict(vec![])
        );
        assert_eq!(reg.modules().drones().get(id).and_then(|d| d.escort), None);
    }

    #[test]
    fn the_21st_assisting_drone_is_refused() {
        let mut reg = world(BIG_BAY, 1, 5);
        fleeted(&mut reg);
        let id = launch_n(&mut reg, 1)[0];
        for n in 0..MAX_DRONE_ASSIST as i64 {
            reg.modules_mut().drones_mut().insert(Drone {
                escort: Some(Escort {
                    character_id: BOB,
                    guard: false,
                }),
                ..Drone::launched(980_000_000 + n, kind(), BOB, BOB_SHIP, SYSTEM)
            });
        }
        let answer = escort(&mut reg, Some(ALICE), BOB, &[id], false);
        let PyValue::Dict(d) = &answer else {
            panic!("a dict")
        };
        assert_eq!(d.len(), 1, "{answer:?}");
        assert_eq!(reg.modules().drones().get(id).and_then(|d| d.escort), None);
        // Guarding is not capped.
        assert_eq!(
            escort(&mut reg, Some(ALICE), BOB, &[id], true),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn activity_change_is_pushed_and_cleared() {
        let mut reg = world(BIG_BAY, 1, 5);
        undock(&mut reg, ALICE);
        let id = launch_n(&mut reg, 1)[0];
        let _ = pending(&mut reg);
        escort(&mut reg, Some(ALICE), ALICE, &[id], false);
        let sent = pending(&mut reg);
        let hit = sent
            .iter()
            .find(|(_, n, _)| *n == "OnDroneActivityChange")
            .expect("told");
        assert_eq!(hit.0, Some(vec![ALICE]));
        assert_eq!(
            hit.2,
            vec![
                PyValue::Int(id),
                PyValue::Int(ALICE),
                crate::pyutil::bstr("assist")
            ]
        );
        return_bay(&mut reg, Some(ALICE), &[id]);
        let sent = pending(&mut reg);
        let hit = sent
            .iter()
            .find(|(_, n, _)| *n == "OnDroneActivityChange")
            .expect("cleared");
        assert_eq!(hit.2, vec![PyValue::Int(id), PyValue::None, PyValue::None]);
    }
}
