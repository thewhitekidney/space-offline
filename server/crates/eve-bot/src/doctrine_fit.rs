//! **Doctrine fits** (DOC-1, R-DOCTRINE 2.2 and 2.5): classifying the parts
//! the type table holds, [`fill_slots`] (the greedy fit), the paper
//! evaluation ([`profile_of`], the dogma engine on a fit at the doctrine's
//! skills) and [`catalog_in`].
//!
//! **Nothing is named by id.** A part's kind is read from the attributes and
//! effects the type carries (a plate states `armorHPBonusAdd`, a web a
//! negative `speedFactor` and an offensive default effect, a gun a turret or
//! launcher hardpoint and the charge group it takes). A weapon family is the
//! charge group it takes ([`crate::fit_plan::weapon_of_group`]). The few
//! attribute ids below are `dogmaattributes` ids **[V-data]**; the ones
//! marked **[A]** are guessed names, whose falsifier is the same: the table
//! has no part of that kind, and the catalog then has no fit that uses it
//! (never a wrong one).
//!
//! **Supersedes** the hand tables of [`crate::fit_plan`] and [`crate::tank`]
//! for doctrine fits only; those keep serving the bots that fly today until
//! DOC-2 makes them follow a [`RoleFit`].

use crate::doctrine::{
    DOCTRINE_LEVEL, Delivery, Doctrine, Ewar, FIT_RETRIES, Family, Flight, Gun, Layer, META_MAX,
    Makeable, Profile, Range, Role, RoleFit, SHARE_EWAR, SHARE_INTERDICT, SHARE_LOGI, SHARE_SCOUT,
    SHARE_TACKLE, Stance, fleet_size, key_of, layer_of, range_of, stance_of,
};
use crate::doctrine_exchange::T_FIGHT;
use crate::fit_plan::{self, Size};
use crate::hull_choice::Class;
use eve_server::dogma::{self, Inputs, ModuleInput, Sheet, State};
use eve_server::fitting::{Hardpoint, Rack};
use eve_server::missile::Aoe;
use eve_server::typedata::{
    CATEGORY_CHARGE, CATEGORY_DRONE, CATEGORY_MODULE, CATEGORY_SHIP, CATEGORY_SKILL, Func,
    Installed, Modifier, Table, TypeRow,
};
use std::collections::{BTreeMap, BTreeSet};

// ---- attribute ids [V-data] unless marked ---------------------------------

const A_CAP_NEED: i64 = 6;
const A_SPEED_FACTOR: i64 = 20;
const A_RATE: i64 = 51;
const A_RANGE: i64 = 54;
const A_DAMAGE_MULT: i64 = 64;
const A_SHIELD_BONUS: i64 = 68;
const A_DURATION: i64 = 73;
const A_ARMOUR_AMOUNT: i64 = 84;
const A_NEUT_AMOUNT: i64 = 97;
const A_SCRAM: i64 = 105;
const A_CHARGE_SIZE: i64 = 128;
const A_SENSOR: [i64; 4] = [208, 209, 210, 211];
/// Jam strengths in the order radar, ladar, magnetometric, gravimetric.
const A_JAM: [i64; 4] = [241, 239, 240, 238];
const A_CAPACITY_BONUS: i64 = 72;
const A_RANGE_BONUS: i64 = eve_server::ewar::attr::RANGE_BONUS;
/// `scanResolutionBonus`: a sensor dampener's other number.
const A_RES_BONUS: i64 = eve_server::ewar::attr::RESOLUTION_BONUS;
/// `missileDamageMultiplier`, a *pilot* attribute of the sheet.
const A_MISSILE_MULT: i64 = 212;
const A_SIG_BONUS: i64 = 554;
const A_PLATE: i64 = 1159;
const A_SPEED_BOOST: i64 = 567;
/// `emDamageResistanceBonus` .. `thermalDamageResistanceBonus` on a hardener
/// **[A]** (the ids are from memory of the table; a miss gives no hardeners).
const A_RESIST_BONUS: [i64; 4] = [984, 985, 986, 987];
/// `metaLevel` and `techLevel` **[V-data]**.
const A_META: i64 = 633;
const A_TECH: i64 = 422;
/// `missileDamageMultiplier` on a ballistic control system **[A]**.
const A_BCS: i64 = 213;
/// `warpScrambleStrength`'s default effects: the disruptor's (39, `tackle::DISRUPT`)
/// and the burst jammer's (6714) **[V-data]**.
const EFFECT_DISRUPT: i64 = 39;
const EFFECT_BURST_JAMMER: i64 = 6714;
/// `empFieldRange`: a smartbomb's reach, on the module **[V-data]**.
const A_BOMB_RANGE: i64 = eve_server::smartbomb::ATTR_RANGE;
/// A smartbomb reaching at most this far is small, at most the next medium,
/// else large **[A]**. Falsified by a bundle whose three sizes do not
/// split on range (a small bomb sized medium: the fit drops it on CPU).
const BOMB_SMALL_M: f64 = 3_500.0;
const BOMB_MEDIUM_M: f64 = 4_500.0;
/// Smartbombs a fit takes in its spare high slots **[A]**. Falsified by a
/// soak in which bombers' own bombs cost them more than they deal.
pub const BOMB_MAX: usize = 2;
/// The four damage attributes of a smartbomb, em .. thermal **[V-data]**.
const A_VOLLEY: [i64; 4] = [114, 116, 117, 118];
/// A guidance disruptor's four bonuses (missile speed, flight time,
/// explosion velocity and radius) **[V-data]**.
const A_GUIDE: [i64; 4] = [
    eve_server::ewar::attr::MISSILE_VELOCITY_BONUS,
    eve_server::ewar::attr::EXPLOSION_DELAY_BONUS,
    eve_server::ewar::attr::AOE_VELOCITY_BONUS,
    eve_server::ewar::attr::AOE_CLOUD_BONUS,
];
const A_TRACK_BONUS: i64 = eve_server::ewar::attr::TRACKING_DISRUPT;
const A_RANGE_DISRUPT: i64 = eve_server::ewar::attr::RANGE_DISRUPT;
/// The guidance bonuses that cut a missile (speed, explosion velocity; the
/// flight time and the radius bonus have the other sign or no cut).
const A_GUIDE_CUT: [i64; 2] = [
    eve_server::ewar::attr::MISSILE_VELOCITY_BONUS,
    eve_server::ewar::attr::AOE_VELOCITY_BONUS,
];
/// `powerOutput`, `power`, `cpuOutput`, `cpu` [V-data].
const FIT_BUDGET_ATTRS: [i64; 4] = [11, 30, 48, 50];
const A_CHARGE_GROUPS: std::ops::RangeInclusive<i64> = 604..=608;
/// Ship attributes the new part kinds are told apart by **[V-data]**: the
/// ship's `maxVelocity` and `agility`, `cpuOutput` and `powerOutput`, its
/// capacitor's `capacity` and `rechargeRate`, `armorHP` and `shieldCapacity`.
const A_MAX_VELOCITY: i64 = 37;
const A_AGILITY: i64 = 70;
const A_CPU_OUT: i64 = 48;
const A_PG_OUT: i64 = 11;
const A_CAP_CAPACITY: i64 = 482;
const A_CAP_RECHARGE: i64 = 55;
const A_ARMOUR_HP: i64 = 265;
const A_SHIELD_CAP: i64 = 263;
/// The hull's resonances (`kinetic`, `thermal`, `explosive`, `em`), the
/// armour's and the shield's: a damage control touches all three layers.
const A_HULL_RES: [i64; 4] = [109, 110, 111, 113];
const A_ARMOUR_RES: [i64; 4] = [267, 268, 269, 270];
const A_SHIELD_RES: [i64; 4] = [271, 272, 273, 274];
/// What a tracking enhancer or computer moves on the turrets: `trackingSpeed`,
/// `falloff`, `maxRange`; what a guidance enhancer or computer moves on the
/// missiles: `aoeVelocity`, `aoeCloudSize`, `explosionDelay` **[V-data]**.
const A_TURRET_APPLY: [i64; 3] = [160, 158, 54];
const A_MISSILE_APPLY: [i64; 3] = [653, 654, 281];
/// `rateOfFire` is the cycle of a weapon rig's target; `damageMultiplier` its
/// other.
const A_ROF: i64 = 51;
/// The rig slot effect (`rigSlot`) **[V-data]**: a rig's default effect.
pub const EFFECT_RIG_SLOT: i64 = 2663;
/// The charge group of capacitor booster charges **[V-data]**: a module that
/// takes them is a capacitor booster.
const GROUP_CAP_BOOSTER_CHARGE: i64 = 87;
/// A hull's rig capacity (`upgradeCapacity`, calibration) and a rig's cost in
/// it (`upgradeCost`) **[V-data]**.
const A_CALIBRATION: i64 = 1132;
const A_RIG_COST: i64 = 1153;
/// A hull's drone bay and bandwidth, a drone's bandwidth need
/// **[V-data]**.
const A_DRONE_BAY: i64 = 283;
const A_DRONE_BANDWIDTH: i64 = 1271;
const A_DRONE_NEED: i64 = 1272;
/// The calibration a hull states none of has **[A]** 400 (the usual T1
/// figure); falsified by a hull row with rigs and no `upgradeCapacity`.
const CALIBRATION_DEFAULT: f64 = 400.0;
/// Drones a pilot flies at once (Drones V) **[V-data]**.
pub const DRONES_ACTIVE: usize = 5;
const A_LEVELS: [i64; 6] = [277, 278, 279, 1286, 1287, 1288];
const A_SIG_RES: i64 = eve_server::drone_skills::ATTR_SIG_RESOLUTION;
const A_FALLOFF: i64 = eve_server::drone_skills::ATTR_FALLOFF;
const A_TRACKING: i64 = eve_server::drone_skills::ATTR_TRACKING;

/// `shipGroup` of the five T1 combat classes **[V-data]**: frigate,
/// destroyer, cruiser, battlecruiser, battleship; and (BP-T2-HULLS) of the
/// three T2 groups the support roles fly: interdictors (541, destroyer
/// size), heavy interdiction cruisers (894) and covert ops frigates (830).
/// A T2 hull is held to `Limits::tech_for` its role, and the three groups
/// serve [`Role::Interdict`] and [`Role::Scout`] only ([`SPECIAL_GROUPS`]).
pub const HULL_GROUPS: [(i64, Class); 8] = [
    (25, Class::Frigate),
    (420, Class::Destroyer),
    (26, Class::Cruiser),
    (419, Class::Battlecruiser),
    (27, Class::Battleship),
    (GROUP_INTERDICTOR, Class::Destroyer),
    (GROUP_HIC, Class::Cruiser),
    (GROUP_COVERT_OPS, Class::Frigate),
];
/// Interdictors, heavy interdiction cruisers and covert ops frigates
/// **[V-data]** (group ids of the client's type data).
pub const GROUP_INTERDICTOR: i64 = 541;
pub const GROUP_HIC: i64 = 894;
pub const GROUP_COVERT_OPS: i64 = 830;
/// The hull groups no line role (damage, logi, tackle, ewar) flies.
pub const SPECIAL_GROUPS: [i64; 3] = [GROUP_INTERDICTOR, GROUP_HIC, GROUP_COVERT_OPS];

/// The class of a hull group.
pub fn class_of_group(group: i64) -> Option<Class> {
    HULL_GROUPS.iter().find(|g| g.0 == group).map(|g| g.1)
}

/// The share of a hull's CPU and power the guns may take before the tank
/// is fitted **[A]**. Falsified by fits that never find room for a tank.
pub const GUN_BUDGET: f64 = 0.6;
/// Rounds taken per gun, primary and alternate **[A]** (cargo is not
/// measured here).
pub const ROUNDS_PRIMARY: u32 = 60;
pub const ROUNDS_ALTERNATE: u32 = 20;
/// An alternate round must carry this share of the primary's damage **[A]**.
pub const ALTERNATE_MIN: f64 = 0.5;
/// Most damage modules fitted **[A]**.
pub const MAX_DAMAGE_MODS: usize = 3;
/// Warp disruption probes carried per sphere launcher **[A]**: a camp
/// throws one a bubble life (`bubble::cycles`) and the launcher's bay holds
/// a few. Falsified by a dictor that runs dry inside one camp, or one that
/// hauls probes it never throws.
pub const PROBES_PER_LAUNCHER: u32 = 8;
/// Which ship attributes a skill touches is read from the tables; the
/// ship-wide ones a fit's own modules touch count as "support".
const SKILL_LEVEL_CAP: i64 = 5;

/// What limits the parts: the highest meta level and tech level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub meta_max: f64,
    /// The highest tech level of a hull the line roles fly (damage, logi,
    /// tackle, ewar).
    pub tech_max: f64,
    /// BP-T2-HULLS: the highest tech level of an interdictor, heavy
    /// interdictor or scout hull **[A]**: T2. The line stays T1 above; a T2
    /// role still appears only when the world can make the hull
    /// ([`Makeable`]). Falsified by a catalog with a dictor no builder or
    /// data site can supply (the soak's `unsourced` line names it).
    pub tech_special: f64,
    /// Fit rigs **[A]**: off, because the server fits none yet
    /// (`eve_server::fitting::Rack`: "no rig is spawnable"), and a fit that
    /// names a rig the server refuses would not load. Falsified by the day
    /// `FitAuto` takes a rig; then the default turns on.
    pub rigs: bool,
}

impl Limits {
    /// The highest tech level of a hull `role` flies.
    pub fn tech_for(&self, role: Role) -> f64 {
        match role {
            Role::Interdict | Role::Scout => self.tech_special,
            _ => self.tech_max,
        }
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            meta_max: META_MAX,
            tech_max: 1.0,
            tech_special: 2.0,
            rigs: false,
        }
    }
}

/// What a damage module aims at, read from its modifiers: the skill the
/// affected items require or the group of the affected items; `Any` when the
/// type states neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aim {
    Any,
    Skill(i64),
    Group(i64),
}

/// What a part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A gun, and its size (1 small, 2 medium, 3 large).
    Gun(Family, u8),
    ShieldExtender,
    ShieldBooster,
    Plate,
    ArmourRepairer,
    Hardener(Layer),
    /// A damage module and what it moves: only guns of that group or
    /// requiring that skill gain from it.
    DamageMod(Aim),
    Prop,
    Web,
    Scram,
    /// A warp disruptor: pins like a scrambler but does not stop a
    /// microwarpdrive.
    Disruptor,
    Painter,
    Damp,
    Ecm,
    Neut,
    RemoteRep(Layer),
    /// A tracking disruptor: cuts a turret ship's optimal, falloff and
    /// tracking speed.
    TrackDisrupt,
    /// A guidance disruptor: cuts a missile ship's reach and explosion
    /// velocity.
    GuideDisrupt,
    /// A smartbomb and its size (1 small, 2 medium, 3 large).
    Smartbomb(u8),
    /// A micro jump drive.
    Mjd,
    /// A cloaking device.
    Cloak,
    /// BP-T2-HULLS: an interdiction sphere launcher (a bubble thrown from
    /// the hull, [`eve_server::bubble::GROUP_LAUNCHER`]); an interdictor's.
    Sphere,
    /// BP-T2-HULLS: a warp disruption field generator
    /// ([`eve_server::bubble::EFFECT_FIELD`]): a heavy interdictor's bubble
    /// that moves with it.
    Field,
    /// A damage control: resonance bonuses in all three layers.
    DamageControl,
    /// Speed and agility (a nanofiber, an overdrive): a ship-wide
    /// `maxVelocity` or `agility` modifier.
    Speed,
    /// A tracking enhancer or computer: moves the turrets' tracking, falloff
    /// or range.
    Tracking,
    /// A missile guidance enhancer or computer: moves the missiles'
    /// explosion velocity, radius or flight time.
    Guidance,
    /// A capacitor battery, flux coil or power relay: the ship's capacitor.
    CapBattery,
    /// A capacitor booster: a module that takes cap booster charges.
    /// Classified only: the fitter does not yet load its charges.
    CapBooster,
    /// A co-processor or reactor control unit: the ship's CPU or power grid.
    PowerMod,
    /// A drone damage amplifier: the damage of every drone that requires the
    /// Drones skill.
    DroneMod,
    /// A combat drone. Held in the drone bay, never in a slot: a fit carries
    /// it in [`RoleFit::charges`].
    Drone,
    /// A rig (a part of the rig rack, [`Part::rig`]).
    Rig(RigKind),
}

/// What a rig does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RigKind {
    Armour,
    Shield,
    Speed,
    Cap,
    /// A weapon rig: moves the guns' damage or rate of fire.
    Weapon,
}

/// One fittable part the world can make.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub type_id: i64,
    pub kind: Kind,
    pub rack: Rack,
    pub meta: f64,
    /// How much of its kind the part gives; ranks parts of one kind.
    pub magnitude: f64,
    pub cost: f64,
    pub cpu: f64,
    pub power: f64,
    /// A rig: fitted in the rig rack, which [`Rack`] has no name for (the
    /// server fits none yet), so a rig is told apart here and [`Library::best`]
    /// never offers it for a slot; [`Library::best_rig`] does.
    pub rig: bool,
    /// A rig's cost in the hull's calibration.
    pub calibration: f64,
    /// A drone's bandwidth need (Mbit/s) and volume (m3); 0 for a module.
    pub bandwidth: f64,
    pub volume: f64,
}

/// A combat hull the world can make.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullRow {
    pub type_id: i64,
    /// The hull's group, for [`Library::can_fit`].
    pub group_id: i64,
    pub class: Class,
    /// The hull's tech level (1 when the row states none).
    pub tech: f64,
    pub hi: usize,
    pub med: usize,
    pub low: usize,
    pub turrets: usize,
    pub launchers: usize,
    pub cpu: f64,
    pub power: f64,
    pub cost: f64,
    /// Rig slots, and the calibration they share.
    pub rig: usize,
    pub calibration: f64,
    /// Drone bay (m3) and bandwidth (Mbit/s).
    pub drone_bay: f64,
    pub bandwidth: f64,
}

/// The parts and hulls of a world, classified once.
pub struct Library<'a> {
    pub inst: &'a Installed,
    pub parts: Vec<Part>,
    pub hulls: Vec<HullRow>,
    pub limits: Limits,
    prices: BTreeMap<i64, f64>,
    /// Launchers [`classify`] leaves out ([`extra_launcher_size`]), by type:
    /// known to [`Library::kind_of`] for a real fit, absent from `parts`.
    extra_guns: BTreeMap<i64, Kind>,
    /// What the world can make: gates the real fits ([`crate::doctrine_real`]).
    pub makeable: Makeable,
    /// What players fly; `None` seeds nothing.
    pub real_fits: Option<&'a crate::real_fits::RealFits>,
}

fn attr(t: &TypeRow, id: i64) -> f64 {
    t.attribute(id).unwrap_or(0.0)
}

fn cycle_attr(table: &Table, t: &TypeRow) -> i64 {
    t.default_effect
        .and_then(|e| table.effect(e))
        .map(|e| e.duration_attribute)
        .filter(|a| *a > 0)
        .unwrap_or(if t.attribute(A_RATE).is_some() {
            A_RATE
        } else {
            A_DURATION
        })
}

/// Seconds of one cycle of a type from its own numbers, `0` for none.
fn cycle_secs(table: &Table, t: &TypeRow) -> f64 {
    attr(t, cycle_attr(table, t)) / 1000.0
}

/// The attributes a damage module moves on the items it affects.
const DAMAGE_ATTRS: [i64; 6] = [A_DAMAGE_MULT, A_MISSILE_MULT, 114, 116, 117, 118];

/// What a damage module aims at: the required skill or group of the first
/// modifier that moves a damage attribute of the items it names.
fn damage_aim(table: &Table, t: &TypeRow) -> Aim {
    t.effects
        .iter()
        .flat_map(|e| table.modifiers(*e))
        .filter(|m| {
            m.modified_attribute
                .is_some_and(|a| DAMAGE_ATTRS.contains(&a))
        })
        .find_map(|m| match m.func {
            Func::LocationRequiredSkillModifier | Func::OwnerRequiredSkillModifier => {
                m.skill_type.map(Aim::Skill)
            }
            Func::LocationGroupModifier => m.group.map(Aim::Group),
            _ => None,
        })
        .unwrap_or(Aim::Any)
}

/// Whether a damage module aimed at `aim` moves the gun `gun`.
fn aim_hits(table: &Table, aim: Aim, gun: i64) -> bool {
    let Some(g) = table.get(gun) else {
        return aim == Aim::Any;
    };
    match aim {
        Aim::Any => true,
        Aim::Skill(s) => required(g).iter().any(|r| r.0 == s),
        Aim::Group(gr) => g.group_id == gr,
    }
}

/// The charge group of rockets, the round the rocket launcher takes
/// **[V-data]** (`chargeGroup1` of every `Rocket Launcher`; the T2 and faction
/// ones add a second group after it).
const GROUP_ROCKET: i64 = 387;

/// **The size of a launcher [`classify`] leaves out**: the rocket launcher
/// (group [`GROUP_ROCKET`], small) and the T2 and faction launchers that list
/// another charge group before the missile one (`classify` reads only the
/// first), sized by the first missile group they name. `None` for a module
/// with no launcher hardpoint or no such group. [`Library`] keeps these in
/// `extra_guns`, not in `parts`, so the greedy fill never picks them and the
/// catalog's own fits do not move; only a real fit that carries one counts
/// its damage.
fn extra_launcher_size(inst: &Installed, t: &TypeRow) -> Option<u8> {
    if inst.module(t.type_id)?.hardpoint != Some(Hardpoint::Launcher) {
        return None;
    }
    A_CHARGE_GROUPS
        .filter_map(|a| t.attribute(a))
        .find_map(|v| match v as i64 {
            GROUP_ROCKET | fit_plan::GROUP_LIGHT_MISSILE => Some(1),
            fit_plan::GROUP_HEAVY_MISSILE | fit_plan::GROUP_HEAVY_ASSAULT_MISSILE => Some(2),
            fit_plan::GROUP_CRUISE_MISSILE | fit_plan::GROUP_TORPEDO => Some(3),
            _ => None,
        })
}

/// What kind of part a module row is, and how much of it (`None`: not one
/// of the kinds a doctrine fits).
pub fn classify(inst: &Installed, t: &TypeRow) -> Option<(Kind, f64)> {
    // A rig has no rack (`Installed::module` knows none): read from its modifiers.
    if t.has_effect(EFFECT_RIG_SLOT) {
        return rig_kind(inst.table(), t);
    }
    let m = inst.module(t.type_id)?;
    let table = inst.table();
    let secs = cycle_secs(table, t).max(1e-9);
    let offensive = t
        .default_effect
        .and_then(|e| table.effect(e))
        .is_some_and(|e| e.is_offensive);
    let has = |a: i64| t.attribute(a).is_some_and(|v| v != 0.0);
    // The sphere launcher takes probes, not damage: before the guns.
    if t.group_id == eve_server::bubble::GROUP_LAUNCHER {
        return Some((Kind::Sphere, 1.0));
    }
    // Guns: a hardpoint, the charge group it takes names the family.
    if let Some(h) = m.hardpoint {
        let group = A_CHARGE_GROUPS
            .filter_map(|a| t.attribute(a))
            .map(|v| v as i64)
            .next()?;
        let weapon = fit_plan::weapon_of_group(group)?;
        let family = Family::of_weapon(weapon);
        let (family, size) = match (h, family) {
            (Hardpoint::Launcher, Family::Missile) => {
                let s = match group {
                    fit_plan::GROUP_LIGHT_MISSILE => 1,
                    fit_plan::GROUP_HEAVY_MISSILE => 2,
                    fit_plan::GROUP_CRUISE_MISSILE => 3,
                    // Heavy assault missiles are a medium round, torpedoes a
                    // large one.
                    fit_plan::GROUP_HEAVY_ASSAULT_MISSILE => 2,
                    fit_plan::GROUP_TORPEDO => 3,
                    _ => return None,
                };
                (Family::Missile, s)
            }
            (Hardpoint::Turret, Family::Projectile | Family::Hybrid | Family::Laser) => {
                let s = t.attribute(A_CHARGE_SIZE)? as i64;
                if !(1..=3).contains(&s) {
                    return None;
                }
                (family, s as u8)
            }
            _ => return None,
        };
        // A launcher ranks by the damage of its best round per second (the
        // sizes and the light launchers' rapid variants are then comparable);
        // a turret by its damage multiplier per second.
        let magnitude = if family == Family::Missile {
            let best = rounds_of(table, t.type_id)
                .into_iter()
                .map(|r| damage_vec(table, r).iter().sum::<f64>())
                .fold(0.0, f64::max);
            best.max(1e-6) / secs
        } else {
            attr(t, A_DAMAGE_MULT).max(0.0) / secs
        };
        return Some((Kind::Gun(family, size), magnitude));
    }
    let rack = m.rack;
    // Effects of modules that are not told apart by an attribute: the
    // smartbomb, the cloak, the micro jump drive and the two disruptors
    // (before the high rack returns, since bombs and cloaks sit there).
    match t.default_effect {
        Some(eve_server::smartbomb::EFFECT) => {
            let reach = attr(t, A_BOMB_RANGE);
            let size = if reach <= BOMB_SMALL_M {
                1
            } else if reach <= BOMB_MEDIUM_M {
                2
            } else {
                3
            };
            let volley: f64 = A_VOLLEY.iter().map(|a| attr(t, *a)).sum();
            return Some((Kind::Smartbomb(size), volley / secs));
        }
        Some(e) if eve_server::cloak::is_cloak_effect(e) => return Some((Kind::Cloak, 1.0)),
        Some(eve_server::bubble::EFFECT_FIELD) => return Some((Kind::Field, 1.0)),
        Some(eve_server::dogma::effect::MICRO_JUMP_DRIVE) => return Some((Kind::Mjd, 1.0)),
        Some(eve_server::ewar::TRACKING) => {
            let m = attr(t, eve_server::ewar::attr::TRACKING_DISRUPT).abs()
                + attr(t, eve_server::ewar::attr::RANGE_DISRUPT).abs();
            return Some((Kind::TrackDisrupt, m));
        }
        Some(eve_server::ewar::GUIDANCE) => {
            let m: f64 = A_GUIDE.iter().map(|a| attr(t, *a).abs()).sum();
            return Some((Kind::GuideDisrupt, m));
        }
        _ => {}
    }
    // Remote repair and neuts live in the high rack.
    if rack == Rack::High {
        if has(A_ARMOUR_AMOUNT) {
            return Some((
                Kind::RemoteRep(Layer::Armour),
                attr(t, A_ARMOUR_AMOUNT) / secs,
            ));
        }
        if has(A_SHIELD_BONUS) {
            return Some((
                Kind::RemoteRep(Layer::Shield),
                attr(t, A_SHIELD_BONUS) / secs,
            ));
        }
        if has(A_NEUT_AMOUNT) {
            return Some((Kind::Neut, attr(t, A_NEUT_AMOUNT) / secs));
        }
        return None;
    }
    // A burst jammer carries the jam strengths but has no target: it is no
    // ECM (and no part a doctrine fits yet).
    if t.default_effect == Some(EFFECT_BURST_JAMMER) {
        return None;
    }
    if A_JAM.iter().any(|a| has(*a)) {
        let best = A_JAM.iter().map(|a| attr(t, *a)).fold(0.0, f64::max);
        return Some((Kind::Ecm, best));
    }
    if has(A_SCRAM) {
        let kind = if t.default_effect == Some(EFFECT_DISRUPT) {
            Kind::Disruptor
        } else {
            Kind::Scram
        };
        return Some((kind, attr(t, A_SCRAM)));
    }
    if offensive && attr(t, A_SPEED_FACTOR) < 0.0 {
        return Some((Kind::Web, -attr(t, A_SPEED_FACTOR)));
    }
    if offensive && attr(t, A_SIG_BONUS) > 0.0 {
        return Some((Kind::Painter, attr(t, A_SIG_BONUS)));
    }
    if offensive && (attr(t, A_RANGE_BONUS) < 0.0 || attr(t, A_RES_BONUS) < 0.0) {
        let m = attr(t, A_RANGE_BONUS).abs() + attr(t, A_RES_BONUS).abs();
        return Some((Kind::Damp, m));
    }
    if matches!(
        t.default_effect,
        Some(dogma::effect::AFTERBURNER | dogma::effect::MICROWARPDRIVE)
    ) {
        return Some((Kind::Prop, attr(t, A_SPEED_BOOST).abs()));
    }
    if rack == Rack::Medium && has(A_SHIELD_BONUS) {
        return Some((Kind::ShieldBooster, attr(t, A_SHIELD_BONUS) / secs));
    }
    if rack == Rack::Low && has(A_ARMOUR_AMOUNT) {
        return Some((Kind::ArmourRepairer, attr(t, A_ARMOUR_AMOUNT) / secs));
    }
    if A_RESIST_BONUS.iter().any(|a| has(*a)) {
        let m: f64 = A_RESIST_BONUS.iter().map(|a| attr(t, *a).abs()).sum();
        let layer = if rack == Rack::Medium {
            Layer::Shield
        } else {
            Layer::Armour
        };
        return Some((Kind::Hardener(layer), m));
    }
    if rack == Rack::Low && has(A_PLATE) {
        return Some((Kind::Plate, attr(t, A_PLATE)));
    }
    if rack == Rack::Medium && has(A_CAPACITY_BONUS) {
        return Some((Kind::ShieldExtender, attr(t, A_CAPACITY_BONUS)));
    }
    if rack == Rack::Low && (has(A_DAMAGE_MULT) || has(A_BCS)) {
        return Some((
            Kind::DamageMod(damage_aim(table, t)),
            attr(t, A_DAMAGE_MULT).max(attr(t, A_BCS)),
        ));
    }
    if offensive {
        return None;
    }
    if rack == Rack::Medium
        && A_CHARGE_GROUPS
            .filter_map(|a| t.attribute(a))
            .any(|g| g as i64 == GROUP_CAP_BOOSTER_CHARGE)
    {
        return Some((Kind::CapBooster, 1.0));
    }
    by_modifiers(table, t)
}

/// The `(modified attribute, modifying attribute's value)` pairs of a type's
/// ship-wide modifiers (`Domain::Ship`, an item modifier) and of its skill
/// modifiers (what it does to every item that requires a skill).
fn modifiers_of(table: &Table, t: &TypeRow) -> Vec<Modifier> {
    t.effects
        .iter()
        .flat_map(|e| table.modifiers(*e).iter().copied())
        .collect()
}

/// How much a set of modifiers moves, read from the module's own numbers.
fn weight(t: &TypeRow, mods: &[Modifier], f: impl Fn(&Modifier) -> bool) -> f64 {
    mods.iter()
        .filter(|m| f(m))
        .filter_map(|m| m.modifying_attribute)
        .map(|a| attr(t, a).abs())
        .sum()
}

fn on_ship(m: &Modifier, attrs: &[i64]) -> bool {
    m.domain == eve_server::typedata::Domain::Ship
        && m.func == Func::ItemModifier
        && m.modified_attribute.is_some_and(|a| attrs.contains(&a))
}

fn on_skill(m: &Modifier, attrs: &[i64]) -> bool {
    matches!(
        m.func,
        Func::LocationRequiredSkillModifier | Func::OwnerRequiredSkillModifier
    ) && m.modified_attribute.is_some_and(|a| attrs.contains(&a))
}

/// **The kinds a module is told apart by what its effects modify**, never by
/// id: a damage control moves the resonances of all three layers; a
/// nanofiber or an overdrive the ship's speed or agility; a co-processor or
/// reactor control unit the CPU or power grid; a battery or flux coil the
/// capacitor; a tracking enhancer or computer the turrets' application; a
/// guidance enhancer or computer the missiles'; a drone damage amplifier the
/// damage of the drones that require the Drones skill. **[A]** that the
/// bundle's effects carry these modifiers on the ship or skill domains;
/// falsified by a module of the kind that classifies as nothing (the catalog
/// then has no fit that uses it, never a wrong one).
fn by_modifiers(table: &Table, t: &TypeRow) -> Option<(Kind, f64)> {
    let mods = modifiers_of(table, t);
    let ship = |attrs: &[i64]| mods.iter().any(|m| on_ship(m, attrs));
    let skill = |attrs: &[i64]| mods.iter().any(|m| on_skill(m, attrs));
    let sum = |attrs: &[i64]| weight(t, &mods, |m| on_ship(m, attrs) || on_skill(m, attrs));
    if ship(&A_HULL_RES) && ship(&A_ARMOUR_RES) && ship(&A_SHIELD_RES) {
        let all: Vec<i64> = [A_HULL_RES, A_ARMOUR_RES, A_SHIELD_RES].concat();
        return Some((Kind::DamageControl, sum(&all)));
    }
    if ship(&[A_MAX_VELOCITY, A_AGILITY]) {
        return Some((Kind::Speed, sum(&[A_MAX_VELOCITY, A_AGILITY])));
    }
    if ship(&[A_CPU_OUT, A_PG_OUT]) {
        return Some((Kind::PowerMod, sum(&[A_CPU_OUT, A_PG_OUT])));
    }
    if ship(&[A_CAP_CAPACITY, A_CAP_RECHARGE]) {
        return Some((Kind::CapBattery, sum(&[A_CAP_CAPACITY, A_CAP_RECHARGE])));
    }
    if skill(&A_TURRET_APPLY) {
        return Some((Kind::Tracking, sum(&A_TURRET_APPLY)));
    }
    if skill(&A_MISSILE_APPLY) {
        return Some((Kind::Guidance, sum(&A_MISSILE_APPLY)));
    }
    let drones = |m: &Modifier| {
        on_skill(m, &[A_DAMAGE_MULT]) && m.skill_type == Some(eve_server::drones::SKILL_DRONES)
    };
    if mods.iter().any(drones) {
        return Some((Kind::DroneMod, weight(t, &mods, drones)));
    }
    None
}

/// **A rig's kind**, from what it modifies: armour or shield hit points, the
/// ship's speed, its capacitor, or the damage and rate of fire of the guns.
fn rig_kind(table: &Table, t: &TypeRow) -> Option<(Kind, f64)> {
    let mods = modifiers_of(table, t);
    let sum = |attrs: &[i64]| weight(t, &mods, |m| on_ship(m, attrs) || on_skill(m, attrs));
    // A rig's drawback moves another ship attribute (armour rigs slow the
    // ship, speed rigs thin the armour): the benefit is the larger number.
    let options: [(RigKind, &[i64]); 5] = [
        (RigKind::Armour, &[A_ARMOUR_HP]),
        (RigKind::Shield, &[A_SHIELD_CAP]),
        (RigKind::Speed, &[A_MAX_VELOCITY, A_AGILITY]),
        (RigKind::Cap, &[A_CAP_CAPACITY, A_CAP_RECHARGE]),
        (RigKind::Weapon, &[A_DAMAGE_MULT, A_ROF]),
    ];
    let (kind, magnitude) = options
        .iter()
        .map(|(k, a)| (*k, sum(a)))
        .filter(|(_, m)| *m > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    Some((Kind::Rig(kind), magnitude))
}

/// Whether a part of kind `have` serves a want of kind `want`: the same
/// kind, or (for missiles) a launcher one size down: a rapid light missile
/// launcher on a cruiser, which the exact-size rule left out. The ranking
/// ([`Part::magnitude`]: damage per second) then decides.
fn serves(have: Kind, want: Kind) -> bool {
    match (have, want) {
        (Kind::Gun(Family::Missile, a), Kind::Gun(Family::Missile, b)) => a <= b && a + 1 >= b,
        _ => have == want,
    }
}

impl<'a> Library<'a> {
    /// Classify every published module and combat hull the world can make.
    pub fn build(
        inst: &'a Installed,
        makeable: &Makeable,
        price: &dyn Fn(i64) -> f64,
        limits: Limits,
    ) -> Library<'a> {
        let table = inst.table();
        let mut parts = Vec::new();
        let mut hulls = Vec::new();
        let mut prices = BTreeMap::new();
        let mut extra_guns = BTreeMap::new();
        for t in table.types().filter(|t| t.published) {
            if !makeable.has(t.type_id) {
                continue;
            }
            match t.category_id {
                CATEGORY_MODULE => {
                    let meta = attr(t, A_META);
                    if meta > limits.meta_max {
                        continue;
                    }
                    let Some((kind, magnitude)) = classify(inst, t) else {
                        if let Some(size) = extra_launcher_size(inst, t) {
                            extra_guns.insert(t.type_id, Kind::Gun(Family::Missile, size));
                            prices.insert(t.type_id, price(t.type_id));
                        }
                        continue;
                    };
                    let rig = matches!(kind, Kind::Rig(_));
                    if rig && !limits.rigs {
                        continue;
                    }
                    let rack = inst.module(t.type_id).map_or(Rack::High, |m| m.rack);
                    let cost = price(t.type_id);
                    prices.insert(t.type_id, cost);
                    parts.push(Part {
                        type_id: t.type_id,
                        kind,
                        rack,
                        meta,
                        magnitude,
                        cost,
                        cpu: attr(t, 50),
                        power: attr(t, 30),
                        rig,
                        calibration: attr(t, A_RIG_COST),
                        bandwidth: 0.0,
                        volume: 0.0,
                    });
                }
                CATEGORY_DRONE => {
                    if attr(t, A_META) > limits.meta_max {
                        continue;
                    }
                    let Some(dps) = drone_dps(t) else { continue };
                    let cost = price(t.type_id);
                    prices.insert(t.type_id, cost);
                    parts.push(Part {
                        type_id: t.type_id,
                        kind: Kind::Drone,
                        rack: Rack::High,
                        meta: attr(t, A_META),
                        magnitude: dps,
                        cost,
                        cpu: 0.0,
                        power: 0.0,
                        rig: false,
                        calibration: 0.0,
                        bandwidth: attr(t, A_DRONE_NEED),
                        volume: t.volume,
                    });
                }
                CATEGORY_SHIP => {
                    let Some(class) = class_of_group(t.group_id) else {
                        continue;
                    };
                    let tech = t.attribute(A_TECH).unwrap_or(1.0);
                    if tech > limits.tech_max.max(limits.tech_special) {
                        continue;
                    }
                    let Some(f) = inst.hull_fit(t.type_id) else {
                        continue;
                    };
                    let cost = price(t.type_id);
                    prices.insert(t.type_id, cost);
                    hulls.push(HullRow {
                        type_id: t.type_id,
                        group_id: t.group_id,
                        class,
                        tech,
                        hi: f.hi.max(0) as usize,
                        med: f.med.max(0) as usize,
                        low: f.low.max(0) as usize,
                        turrets: f.turrets.max(0) as usize,
                        launchers: f.launchers.max(0) as usize,
                        cpu: f.cpu_output,
                        power: f.power_output,
                        cost,
                        rig: f.rig.max(0) as usize,
                        calibration: t.attribute(A_CALIBRATION).unwrap_or(CALIBRATION_DEFAULT),
                        drone_bay: attr(t, A_DRONE_BAY),
                        bandwidth: attr(t, A_DRONE_BANDWIDTH),
                    });
                }
                CATEGORY_CHARGE => {
                    prices.insert(t.type_id, price(t.type_id));
                }
                _ => {}
            }
        }
        // Rounds are priced even when not Makeable-listed as modules.
        Library {
            inst,
            parts,
            hulls,
            limits,
            prices,
            extra_guns,
            makeable: makeable.clone(),
            real_fits: None,
        }
    }

    /// **The hulls a role flies** (BP-T2-HULLS): within the role's tech
    /// level ([`Limits::tech_for`]); an interdictor or heavy interdictor for
    /// [`Role::Interdict`]; any hull but those for a scout; and for the line
    /// roles, none of the three special groups ([`SPECIAL_GROUPS`]).
    pub fn hulls_for(&self, role: Role) -> impl Iterator<Item = &HullRow> {
        let tech = self.limits.tech_for(role);
        self.hulls.iter().filter(move |h| {
            h.tech <= tech
                && match role {
                    Role::Interdict => matches!(h.group_id, GROUP_INTERDICTOR | GROUP_HIC),
                    Role::Scout => !matches!(h.group_id, GROUP_INTERDICTOR | GROUP_HIC),
                    _ => !SPECIAL_GROUPS.contains(&h.group_id),
                }
        })
    }

    pub fn price(&self, type_id: i64) -> f64 {
        self.prices.get(&type_id).copied().unwrap_or(0.0)
    }

    pub fn kind_of(&self, type_id: i64) -> Option<Kind> {
        self.parts
            .iter()
            .find(|p| p.type_id == type_id)
            .map(|p| p.kind)
            .or_else(|| self.extra_guns.get(&type_id).copied())
    }

    /// Whether `part` may be fitted to `hull`: a module that names hull
    /// groups or types (`canFitShipGroup`, `canFitShipType`) fits only
    /// those; one that names none fits any hull. The server refuses the
    /// same fit with the same rule (`eve_server::fitting::hull_allows`), so
    /// the two cannot drift.
    pub fn can_fit(&self, part: &Part, hull: &HullRow) -> bool {
        let Some(t) = self.inst.table().get(part.type_id) else {
            return true;
        };
        eve_server::fitting::hull_allows(|a| t.attribute(a), hull.group_id, hull.type_id)
    }

    /// The best affordable part of a kind in a rack that fits `hull`:
    /// highest meta, then magnitude, then cheapest, then lowest id.
    #[cfg(test)]
    fn best(
        &self,
        hull: &HullRow,
        want: impl Fn(Kind) -> bool,
        rack: Rack,
        left: (f64, f64),
    ) -> Option<Part> {
        self.best_where(hull, |p| want(p.kind), rack, left, &BTreeMap::new())
    }

    /// [`Library::best`] over a predicate on the part, and with the part
    /// costs `costs` (the sheet's, at the doctrine skills) where it has them.
    fn best_where(
        &self,
        hull: &HullRow,
        want: impl Fn(&Part) -> bool,
        rack: Rack,
        left: (f64, f64),
        costs: &Costs,
    ) -> Option<Part> {
        self.parts
            .iter()
            .filter(|p| {
                let (cpu, power) = cost_of(costs, p);
                p.rack == rack
                    && !p.rig
                    && p.kind != Kind::Drone
                    && want(p)
                    && cpu <= left.0
                    && power <= left.1
                    && self.can_fit(p, hull)
            })
            .copied()
            .max_by(|a, b| {
                a.meta
                    .total_cmp(&b.meta)
                    .then(a.magnitude.total_cmp(&b.magnitude))
                    .then(b.cost.total_cmp(&a.cost))
                    .then(b.type_id.cmp(&a.type_id))
            })
    }
}

/// A part's `(cpu, power)` as the sheet reads it at the doctrine skills.
type Costs = BTreeMap<i64, (f64, f64)>;

fn cost_of(costs: &Costs, p: &Part) -> (f64, f64) {
    costs.get(&p.type_id).copied().unwrap_or((p.cpu, p.power))
}

/// **The fit budget at the doctrine's skills** (the engine's numbers, not the
/// table's raw ones): the hull's `(cpuOutput, powerOutput)` and the `(cpu,
/// power)` need of every part that fits it. A pilot's CPU Management, Power
/// Grid Management and Weapon Upgrades work are in them.
fn budget_of(lib: &Library<'_>, hull: &HullRow) -> ((f64, f64), Costs) {
    let table = lib.inst.table();
    let raw = (hull.cpu, hull.power);
    let cands: Vec<&Part> = lib
        .parts
        .iter()
        .filter(|p| !p.rig && p.kind != Kind::Drone && lib.can_fit(p, hull))
        .collect();
    let types: Vec<i64> = std::iter::once(hull.type_id)
        .chain(cands.iter().map(|p| p.type_id))
        .collect();
    let skills = skills_for(table, &types, DOCTRINE_LEVEL);
    let inputs = |modules: Vec<ModuleInput>| Inputs {
        ship_type: hull.type_id,
        skills: skills.clone(),
        modules,
        implants: Vec::new(),
        boosters: Vec::new(),
        beacon: None,
    };
    let Some(bare) = dogma::evaluate(table, &inputs(Vec::new())) else {
        return (raw, Costs::new());
    };
    let out = (
        bare.ship(dogma::attr::CPU_OUTPUT),
        bare.ship(dogma::attr::POWER_OUTPUT),
    );
    let modules: Vec<ModuleInput> = cands
        .iter()
        .enumerate()
        .map(|(i, p)| ModuleInput {
            item_id: i as i64 + 1,
            type_id: p.type_id,
            state: State::Online,
            charge_type: None,
        })
        .collect();
    let mut costs = Costs::new();
    if let Some(sheet) = dogma::evaluate(table, &inputs(modules.clone())) {
        for (m, p) in modules.iter().zip(&cands) {
            costs.insert(
                p.type_id,
                (
                    mod_attr(&sheet, table, m, dogma::attr::CPU),
                    mod_attr(&sheet, table, m, dogma::attr::POWER),
                ),
            );
        }
    }
    (out, costs)
}

// ---- rigs and drones -------------------------------------------------------

impl Library<'_> {
    /// The best rig of a kind whose calibration cost is within `calibration`:
    /// highest meta, then magnitude, then cheapest, then lowest id.
    fn best_rig(&self, hull: &HullRow, want: RigKind, calibration: f64) -> Option<Part> {
        self.parts
            .iter()
            .filter(|p| {
                p.rig
                    && p.kind == Kind::Rig(want)
                    && p.calibration <= calibration
                    && self.can_fit(p, hull)
            })
            .copied()
            .max_by(|a, b| {
                a.meta
                    .total_cmp(&b.meta)
                    .then(a.magnitude.total_cmp(&b.magnitude))
                    .then(b.cost.total_cmp(&a.cost))
                    .then(b.type_id.cmp(&a.type_id))
            })
    }

    /// Whether the library has a part of `kind` for `rack` that fits `hull`.
    fn has(&self, hull: &HullRow, kind: Kind, rack: Rack) -> bool {
        self.parts
            .iter()
            .any(|p| p.rack == rack && !p.rig && p.kind == kind && self.can_fit(p, hull))
    }
}

/// A combat drone's damage per second from its own numbers (damage
/// multiplier times the four damage attributes over its rate of fire), the
/// part's ranking; `None` for a drone that does no damage (a miner, a
/// logistics or ewar drone).
fn drone_dps(t: &TypeRow) -> Option<f64> {
    let volley: f64 = A_VOLLEY.iter().map(|a| attr(t, *a)).sum();
    let secs = attr(t, A_RATE) / 1000.0;
    (volley > 0.0 && secs > 0.0).then(|| volley * attr(t, A_DAMAGE_MULT).max(1.0) / secs)
}

/// **The drones a hull carries**: the combat drone whose active flight (as
/// many as the bandwidth, the bay and [`DRONES_ACTIVE`] allow) deals the most,
/// at a tie the cheaper, then the lower id. `None` for a hull with no bay or
/// bandwidth, or no drone that fits them.
pub fn drones_for(lib: &Library<'_>, hull: &HullRow) -> Option<(Part, u8)> {
    if hull.drone_bay <= 0.0 || hull.bandwidth <= 0.0 {
        return None;
    }
    lib.parts
        .iter()
        .filter(|p| {
            p.kind == Kind::Drone
                && p.bandwidth > 0.0
                && p.bandwidth <= hull.bandwidth
                && p.volume > 0.0
                && p.volume <= hull.drone_bay
        })
        .map(|p| {
            let n = ((hull.bandwidth / p.bandwidth).floor() as usize)
                .min((hull.drone_bay / p.volume).floor() as usize)
                .min(DRONES_ACTIVE);
            (*p, n)
        })
        .filter(|(_, n)| *n > 0)
        .max_by(|a, b| {
            (a.0.magnitude * a.1 as f64)
                .total_cmp(&(b.0.magnitude * b.1 as f64))
                .then(b.0.cost.total_cmp(&a.0.cost))
                .then(b.0.type_id.cmp(&a.0.type_id))
        })
        .map(|(p, n)| (p, n as u8))
}

// ---- rounds ---------------------------------------------------------------

/// **Every round `gun` takes**, from a bare table: [`crate::ammo::rounds_for`]
/// without a `Content` (the charge groups the gun names and its size), the
/// published charges only, ascending by id.
pub fn rounds_of(table: &Table, gun: i64) -> Vec<i64> {
    let Some(g) = table.get(gun) else {
        return Vec::new();
    };
    let groups: Vec<i64> = crate::ammo::ATTR_CHARGE_GROUPS
        .filter_map(|a| g.attribute(a))
        .map(|v| v as i64)
        .collect();
    let size = g.attribute(crate::ammo::ATTR_CHARGE_SIZE);
    table
        .types()
        .filter(|t| t.published && t.category_id == CATEGORY_CHARGE)
        .filter_map(|t| {
            let r = crate::ammo::Round {
                type_id: t.type_id,
                group_id: t.group_id,
                size: t
                    .attribute(crate::ammo::ATTR_CHARGE_SIZE)
                    .or(size)
                    .unwrap_or(0.0),
            };
            crate::ammo::takes(&groups, size, &r).then_some(t.type_id)
        })
        .collect()
}

/// **The probe a sphere launcher throws** (BP-T2-HULLS): of the charges of
/// the group the launcher names ([`eve_server::bubble::GROUP_PROBE`]), those
/// the world can make (a charge is priced only when `Makeable`), the
/// cheapest, then the lower id. A probe states no damage, so
/// [`choose_rounds`] cannot pick it.
pub fn probe_for(lib: &Library<'_>, launcher: i64) -> Option<i64> {
    let table = lib.inst.table();
    rounds_of(table, launcher)
        .into_iter()
        .filter(|r| {
            table.get(*r).map(|t| t.group_id) == Some(eve_server::bubble::GROUP_PROBE)
                && lib.prices.contains_key(r)
                && table.attribute(*r, A_META) <= lib.limits.meta_max
        })
        .min_by(|a, b| lib.price(*a).total_cmp(&lib.price(*b)).then(a.cmp(b)))
}

fn damage_vec(table: &Table, round: i64) -> [f64; 4] {
    [114, 116, 117, 118].map(|a| table.attribute(round, a))
}

/// The primary round (most damage; at a tie the cheaper, then the lower id)
/// and an alternate of a different damage type that carries at least
/// [`ALTERNATE_MIN`] of its damage.
pub fn choose_rounds(lib: &Library<'_>, gun: i64) -> Option<(i64, Option<i64>)> {
    let table = lib.inst.table();
    let rounds: Vec<(i64, [f64; 4])> = rounds_of(table, gun)
        .into_iter()
        .filter(|r| table.attribute(*r, A_META) <= lib.limits.meta_max)
        .map(|r| (r, damage_vec(table, r)))
        .filter(|(_, d)| d.iter().sum::<f64>() > 0.0)
        .collect();
    let total = |d: &[f64; 4]| d.iter().sum::<f64>();
    let primary = rounds.iter().copied().max_by(|a, b| {
        total(&a.1)
            .total_cmp(&total(&b.1))
            .then(lib.price(b.0).total_cmp(&lib.price(a.0)))
            .then(b.0.cmp(&a.0))
    })?;
    let unit = |d: &[f64; 4]| {
        let n = d.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-9);
        d.map(|x| x / n)
    };
    let pu = unit(&primary.1);
    let alt = rounds
        .iter()
        .copied()
        .filter(|(r, d)| *r != primary.0 && total(d) >= ALTERNATE_MIN * total(&primary.1))
        .map(|(r, d)| {
            let u = unit(&d);
            (r, u.iter().zip(pu).map(|(a, b)| a * b).sum::<f64>())
        })
        .filter(|(_, cos)| *cos < 0.999)
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|(r, _)| r);
    Some((primary.0, alt))
}

// ---- skills ---------------------------------------------------------------

/// `(skill, level)` of a type's direct requirements.
fn required(t: &TypeRow) -> Vec<(i64, i64)> {
    dogma::attr::REQUIRED_SKILLS
        .iter()
        .zip(A_LEVELS)
        .filter_map(|(s, l)| {
            let skill = t.attribute(*s)? as i64;
            (skill > 0).then(|| (skill, t.attribute(l).map_or(1, |v| v as i64)))
        })
        .collect()
}

/// **The skills a doctrine pilot has**: every requirement of the hull,
/// modules and rounds, and the support skills that move the fit's numbers --
/// a skill with an effect that modifies items requiring one of those skills
/// (Surgical Strike on the gunnery skill) or a ship attribute the fit's own
/// parts modify (Shield Management on shield capacity) -- each at least
/// `level`. Ascending by skill type, as `dogma::Inputs` wants.
pub fn skills_for(table: &Table, types: &[i64], level: i64) -> Vec<(i64, i64)> {
    let level = level.clamp(1, SKILL_LEVEL_CAP);
    let mut out: BTreeMap<i64, i64> = BTreeMap::new();
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    for &ty in types {
        let Some(t) = table.get(ty) else { continue };
        for (s, l) in required(t) {
            let e = out.entry(s).or_insert(0);
            *e = (*e).max(l.max(level));
        }
        for &eff in &t.effects {
            for m in table.modifiers(eff) {
                if m.domain == eve_server::typedata::Domain::Ship
                    && let Some(a) = m.modified_attribute
                {
                    touched.insert(a);
                }
            }
        }
    }
    let wanted: BTreeSet<i64> = out.keys().copied().collect();
    let supports = |m: &Modifier| -> bool {
        // Every skill that moves the fit budget: CPU Management, Power Grid
        // Management, Weapon Upgrades and the like, found by what they
        // modify (a hull's outputs, a module's need).
        m.modified_attribute
            .is_some_and(|a| FIT_BUDGET_ATTRS.contains(&a))
            || (matches!(
                m.func,
                Func::LocationRequiredSkillModifier | Func::OwnerRequiredSkillModifier
            ) && m.skill_type.is_some_and(|s| wanted.contains(&s)))
            || (m.domain == eve_server::typedata::Domain::Ship
                && m.func == Func::ItemModifier
                && m.modified_attribute.is_some_and(|a| touched.contains(&a)))
    };
    for s in table.types().filter(|t| t.category_id == CATEGORY_SKILL) {
        let helps = s
            .effects
            .iter()
            .any(|e| table.modifiers(*e).iter().any(&supports));
        if helps {
            out.entry(s.type_id).or_insert(level);
        }
    }
    out.into_iter().collect()
}

// ---- the paper evaluation -------------------------------------------------

/// Module inputs for a fit: item ids 1.. in order; guns online, everything
/// with a default effect running (a kite lives on its propulsion).
fn module_inputs(
    lib: &Library<'_>,
    modules: &[(i64, u8)],
    charge: Option<i64>,
) -> Vec<(ModuleInput, Option<Kind>)> {
    let table = lib.inst.table();
    let mut out = Vec::new();
    let mut id = 0;
    for &(ty, n) in modules {
        let kind = lib.kind_of(ty);
        for _ in 0..n {
            id += 1;
            let gun = matches!(kind, Some(Kind::Gun(..)));
            let runs = table.get(ty).is_some_and(|t| t.default_effect.is_some());
            out.push((
                ModuleInput {
                    item_id: id,
                    type_id: ty,
                    state: if runs
                        && !gun
                        && !matches!(
                            kind,
                            Some(Kind::Mjd | Kind::Cloak | Kind::Sphere | Kind::Field)
                        ) {
                        State::Active
                    } else {
                        State::Online
                    },
                    charge_type: if gun { charge } else { None },
                },
                kind,
            ));
        }
    }
    out
}

/// A module's number: the sheet's when it computed one, else the type's.
fn mod_attr(sheet: &Sheet, table: &Table, m: &ModuleInput, a: i64) -> f64 {
    sheet
        .module_opt(m.item_id, a)
        .unwrap_or_else(|| table.attribute(m.type_id, a))
}

/// **The paper evaluation**: the dogma engine on `hull` with `modules` and
/// `charge` loaded in every gun at `skills`, read into a [`Profile`].
/// `None` when the table does not hold the hull.
pub fn profile_of(
    lib: &Library<'_>,
    hull: i64,
    modules: &[(i64, u8)],
    charge: Option<i64>,
    skills: &[(i64, i64)],
) -> Option<Profile> {
    profile_with_drones(lib, hull, modules, charge, skills, &[])
}

/// The first item id of the phantom modules that carry the drones.
const DRONE_ITEM_BASE: i64 = 1_000_000;

/// [`profile_of`] with `drones` (`(type, count)`) out of the bay: each flies
/// as the charge of a phantom module, as [`eve_server::drone_skills::stats`]
/// does, so the drone skills, the hull's drone bonuses and the fitted drone
/// damage amplifiers apply through the engine; the specialisation skills'
/// python effect (1730) is applied after it, as `stats` does. Their damage
/// counts in [`Profile::dps`], and with no gun a drone's reach is the
/// delivery.
pub fn profile_with_drones(
    lib: &Library<'_>,
    hull: i64,
    modules: &[(i64, u8)],
    charge: Option<i64>,
    skills: &[(i64, i64)],
    drones: &[(i64, u8)],
) -> Option<Profile> {
    let table = lib.inst.table();
    let fitted = module_inputs(lib, modules, charge);
    let phantoms: Vec<(i64, i64, u8)> = drones
        .iter()
        .enumerate()
        .map(|(i, &(ty, n))| (DRONE_ITEM_BASE + i as i64, ty, n))
        .collect();
    let inputs = Inputs {
        ship_type: hull,
        skills: skills.to_vec(),
        modules: fitted
            .iter()
            .map(|(m, _)| m.clone())
            .chain(phantoms.iter().map(|&(id, ty, _)| ModuleInput {
                item_id: id,
                type_id: 0,
                state: State::Offline,
                charge_type: Some(ty),
            }))
            .collect(),
        implants: Vec::new(),
        boosters: Vec::new(),
        beacon: None,
    };
    let sheet = dogma::evaluate(table, &inputs)?;
    let (cpu, pg) = sheet.load(&inputs, None);
    let mut p = Profile::blank(dogma::profile_from(&sheet));
    p.cpu_ok = cpu <= sheet.ship(dogma::attr::CPU_OUTPUT) + 1e-6;
    p.pg_ok = pg <= sheet.ship(dogma::attr::POWER_OUTPUT) + 1e-6;
    p.speed = sheet.ship(dogma::attr::MAX_VELOCITY);
    p.sig = sheet.ship(dogma::attr::SIGNATURE_RADIUS);
    p.agility = sheet.ship(dogma::attr::AGILITY);
    p.lock_range = sheet.ship(dogma::attr::MAX_TARGET_RANGE);
    p.sensor = A_SENSOR.map(|a| table.attribute(hull, a));
    let cap = sheet.ship(dogma::attr::CAPACITOR_CAPACITY);
    let recharge_s = sheet.ship(dogma::attr::RECHARGE_RATE) / 1000.0;
    p.cap_regen = if recharge_s > 0.0 {
        2.5 * cap / recharge_s
    } else {
        0.0
    };

    let mut cap_use = 0.0;
    let mut web_left = 1.0;
    let mut e = Ewar::default();
    let mut pin: Option<f64> = None;
    let mut flight = None;
    let mut gun = None;
    for (m, kind) in &fitted {
        let Some(kind) = kind else { continue };
        let ty = table.get(m.type_id)?;
        let secs = mod_attr(&sheet, table, m, cycle_attr(table, ty)) / 1000.0;
        let per_s = |amount: f64| if secs > 0.0 { amount / secs } else { 0.0 };
        if m.state.running() || matches!(kind, Kind::Gun(..)) {
            cap_use += per_s(mod_attr(&sheet, table, m, A_CAP_NEED).max(0.0));
        }
        let range = mod_attr(&sheet, table, m, A_RANGE);
        let off = |a| table.attribute(m.type_id, a);
        match *kind {
            Kind::Gun(family, _) => {
                let mult = if family == Family::Missile {
                    sheet.character(A_MISSILE_MULT)
                } else {
                    mod_attr(&sheet, table, m, A_DAMAGE_MULT)
                };
                let dmg = [114, 116, 117, 118].map(|a| {
                    let v = sheet
                        .charge(m.item_id, a)
                        .unwrap_or_else(|| m.charge_type.map_or(0.0, |c| table.attribute(c, a)));
                    per_s(v * mult)
                });
                for (sum, d) in p.dps.iter_mut().zip(dmg) {
                    *sum += d;
                }
                if family == Family::Missile {
                    let c = |a| sheet.charge(m.item_id, a).unwrap_or(0.0);
                    flight = Some(Flight {
                        reach: c(37) * c(281) / 1000.0,
                        aoe: Aoe {
                            cloud: c(654),
                            velocity: c(653),
                            drf: c(1353),
                        },
                    });
                } else {
                    gun = Some(Gun {
                        optimal: range,
                        falloff: mod_attr(&sheet, table, m, A_FALLOFF),
                        tracking: mod_attr(&sheet, table, m, A_TRACKING),
                        sig_res: mod_attr(&sheet, table, m, A_SIG_RES),
                    });
                }
            }
            Kind::ShieldBooster => p.rep_local[0] += per_s(off(A_SHIELD_BONUS)),
            Kind::ArmourRepairer => p.rep_local[1] += per_s(off(A_ARMOUR_AMOUNT)),
            Kind::RemoteRep(layer) => {
                let a = if layer == Layer::Shield {
                    A_SHIELD_BONUS
                } else {
                    A_ARMOUR_AMOUNT
                };
                p.remote_rep += per_s(off(a));
                p.remote_layer = Some(layer);
                p.remote_range = p.remote_range.max(range);
            }
            Kind::Web => {
                web_left *= 1.0 - (-off(A_SPEED_FACTOR) / 100.0).clamp(0.0, 1.0);
                pin = Some(pin.map_or(range, |r| r.min(range)));
            }
            Kind::Scram => {
                e.scram = true;
                pin = Some(pin.map_or(range, |r| r.min(range)));
            }
            Kind::Disruptor => {
                pin = Some(pin.map_or(range, |r| r.min(range)));
                e.point_range = e.point_range.max(range);
            }
            Kind::TrackDisrupt => {
                let cut = |a| (-off(a) / 100.0).clamp(0.0, 1.0);
                e.track_cut = 1.0 - (1.0 - e.track_cut) * (1.0 - cut(A_TRACK_BONUS));
                e.opt_cut = 1.0 - (1.0 - e.opt_cut) * (1.0 - cut(A_RANGE_DISRUPT));
                e.track_range = e.track_range.max(range);
                e.track_falloff = e.track_falloff.max(off(eve_server::ewar::attr::FALLOFF));
            }
            Kind::GuideDisrupt => {
                let cut = A_GUIDE_CUT
                    .iter()
                    .map(|a| (-off(*a) / 100.0).clamp(0.0, 1.0))
                    .fold(0.0, f64::max);
                e.guide_cut = 1.0 - (1.0 - e.guide_cut) * (1.0 - cut);
                e.guide_range = e.guide_range.max(range);
                e.guide_falloff = e.guide_falloff.max(off(eve_server::ewar::attr::FALLOFF));
            }
            Kind::Smartbomb(_) => {
                for (sum, a) in e.bomb.iter_mut().zip(A_VOLLEY) {
                    *sum += per_s(mod_attr(&sheet, table, m, a));
                }
                e.bomb_range = e.bomb_range.max(mod_attr(&sheet, table, m, A_BOMB_RANGE));
            }
            Kind::Mjd => p.mjd = true,
            Kind::Painter => {
                e.paint += off(A_SIG_BONUS) / 100.0;
                e.paint_range = e.paint_range.max(range);
            }
            Kind::Damp => {
                e.damp = (e.damp + (-off(A_RANGE_BONUS) / 100.0).max(0.0)).min(1.0);
                e.damp_range = e.damp_range.max(range);
            }
            Kind::Ecm => {
                for (i, a) in A_JAM.iter().enumerate() {
                    e.ecm[i] += off(*a);
                }
                e.ecm_range = e.ecm_range.max(range);
                e.ecm_falloff = e.ecm_falloff.max(off(eve_server::ewar::attr::FALLOFF));
            }
            Kind::Neut => {
                e.neut += per_s(off(A_NEUT_AMOUNT));
                e.neut_range = e.neut_range.max(range);
            }
            _ => {}
        }
    }
    // The drones: the engine's numbers for the drone as a charge, then the
    // specialisation skills' bonus for the drones that require them.
    let mut drone_gun = None;
    for &(id, ty, n) in &phantoms {
        let c = |a: i64| {
            sheet
                .charge(id, a)
                .filter(|v| *v != 0.0)
                .unwrap_or_else(|| table.attribute(ty, a))
        };
        let required = eve_server::drone_skills::required_skills(table, ty);
        let mut mult = c(A_DAMAGE_MULT);
        for &(skill, level) in skills {
            let Some(row) = table.get(skill) else {
                continue;
            };
            if row.has_effect(eve_server::drone_skills::EFFECT_DRONE_DMG_BONUS)
                && required.iter().any(|(s, _)| *s == skill)
            {
                mult *= 1.0
                    + row
                        .attribute(eve_server::drone_skills::ATTR_DAMAGE_BONUS)
                        .unwrap_or(0.0)
                        * level as f64
                        / 100.0;
            }
        }
        let secs = c(A_RATE) / 1000.0;
        if secs <= 0.0 {
            continue;
        }
        for (sum, a) in p.dps.iter_mut().zip([114, 116, 117, 118]) {
            *sum += f64::from(n) * c(a) * mult / secs;
        }
        drone_gun = drone_gun.or(Some(Gun {
            optimal: c(A_RANGE),
            falloff: c(A_FALLOFF),
            tracking: c(A_TRACKING),
            sig_res: c(A_SIG_RES),
        }));
    }
    e.web_slow = 1.0 - web_left;
    e.pin_range = pin.unwrap_or(0.0);
    p.ewar = e;
    let gun = gun.or(drone_gun);
    p.delivery = match (gun, flight) {
        (_, Some(f)) => Delivery::Missile(f),
        (Some(g), None) => Delivery::Turret(g),
        _ => Delivery::None,
    };
    let net = cap_use - p.cap_regen;
    p.cap_ok = net <= 0.0 || cap / net >= T_FIGHT;
    Some(p)
}

// ---- fill_slots -----------------------------------------------------------

/// The wants of one rack, in the order they are filled.
type Wants = Vec<(Kind, usize)>;

fn skim(w: &mut Wants, kind: Kind, n: usize) {
    if n > 0 {
        w.push((kind, n));
    }
}

/// The utility modules a template puts in the mids.
fn utility(tmpl: Stance) -> Vec<Kind> {
    match tmpl {
        Stance::Brawl => vec![Kind::Scram, Kind::Web, Kind::Prop],
        Stance::Skirmish => vec![Kind::Prop, Kind::Web],
        Stance::Kite => vec![Kind::Prop, Kind::Disruptor],
    }
}

/// What a role wants in each rack (high, mid, low) before the parts that
/// exist are looked at.
fn wants(
    h: &HullRow,
    family: Family,
    layer: Layer,
    tmpl: Stance,
    role: Role,
) -> (Wants, Wants, Wants) {
    let (mut hi, mut mid, mut low) = (Vec::new(), Vec::new(), Vec::new());
    let hardpoints = if family == Family::Missile {
        h.launchers
    } else {
        h.turrets
    };
    // A drone boat's damage is its drones'; its highs take no gun of the family.
    let guns = if family == Family::Drone {
        0
    } else {
        hardpoints.min(h.hi)
    };
    match role {
        Role::Dps | Role::Command => {
            skim(&mut hi, Kind::Gun(family, size_of(h.class)), guns);
            for k in utility(tmpl) {
                skim(&mut mid, k, 1);
            }
            // A big hull that keeps its distance carries a jump drive.
            if tmpl != Stance::Brawl && matches!(h.class, Class::Battlecruiser | Class::Battleship)
            {
                skim(&mut mid, Kind::Mjd, 1);
            }
        }
        Role::Logi => {
            skim(&mut hi, Kind::RemoteRep(layer), h.hi);
            skim(&mut mid, Kind::Prop, 1);
        }
        Role::Tackle => {
            skim(&mut hi, Kind::Gun(family, size_of(h.class)), guns);
            for k in utility(Stance::Brawl) {
                skim(&mut mid, k, 1);
            }
        }
        Role::Interdict => {
            // The bubble first (a hull may carry only one of the two: the
            // sphere launcher or the field generator, `Library::can_fit`),
            // then the guns the highs have left, then the tackle.
            skim(&mut hi, Kind::Sphere, 1);
            skim(&mut hi, Kind::Field, 1);
            skim(
                &mut hi,
                Kind::Gun(family, size_of(h.class)),
                guns.min(h.hi.saturating_sub(1)),
            );
            for k in utility(Stance::Brawl) {
                skim(&mut mid, k, 1);
            }
        }
        Role::Scout => {
            skim(&mut hi, Kind::Cloak, 1);
            skim(&mut mid, Kind::Prop, 1);
        }
        Role::Ewar => {
            skim(&mut hi, Kind::Neut, h.hi.min(2));
            skim(&mut mid, Kind::Ecm, h.med.min(2));
            skim(&mut mid, Kind::Painter, 1);
            skim(&mut mid, Kind::TrackDisrupt, 1);
            skim(&mut mid, Kind::GuideDisrupt, 1);
            skim(&mut mid, Kind::Damp, 1);
            skim(&mut mid, Kind::Prop, 1);
        }
    }
    let used_mid: usize = mid.iter().map(|w| w.1).sum();
    let rem_mid = h.med.saturating_sub(used_mid);
    match layer {
        Layer::Shield | Layer::Hull => {
            if rem_mid >= 4 && role != Role::Logi {
                skim(&mut mid, Kind::ShieldBooster, 1);
            }
            let hard = usize::from(rem_mid >= 3);
            skim(&mut mid, Kind::Hardener(Layer::Shield), hard);
            let booster = usize::from(rem_mid >= 4 && role != Role::Logi);
            skim(
                &mut mid,
                Kind::ShieldExtender,
                rem_mid.saturating_sub(hard + booster),
            );
            if matches!(
                role,
                Role::Dps | Role::Command | Role::Tackle | Role::Interdict
            ) {
                skim(
                    &mut low,
                    Kind::DamageMod(Aim::Any),
                    h.low.min(MAX_DAMAGE_MODS),
                );
            }
        }
        Layer::Armour => {
            let rep = usize::from(h.low >= 3 && role != Role::Logi);
            let hard = usize::from(h.low >= 4);
            skim(&mut low, Kind::ArmourRepairer, rep);
            skim(&mut low, Kind::Hardener(Layer::Armour), hard);
            let rest = h.low.saturating_sub(rep + hard);
            let dmg = if matches!(role, Role::Dps | Role::Command) {
                (rest / 3).max(usize::from(rest >= 2)).min(MAX_DAMAGE_MODS)
            } else {
                0
            };
            skim(&mut low, Kind::Plate, rest - dmg);
            skim(&mut low, Kind::DamageMod(Aim::Any), dmg);
        }
    }
    (hi, mid, low)
}

/// Take `need` slots from a rack's wants: from the kinds `from` in order, a
/// kind never below its `floor`.
fn squeeze(w: &mut Wants, mut need: usize, from: &[(Kind, usize)]) {
    for &(kind, floor) in from {
        for e in w.iter_mut().filter(|e| e.0 == kind) {
            let take = need.min(e.1.saturating_sub(floor));
            e.1 -= take;
            need -= take;
        }
    }
    w.retain(|e| e.1 > 0);
}

/// Cut `w` to at most `room` slots, from the end.
fn trim(w: &mut Wants, room: usize) {
    let mut total: usize = w.iter().map(|e| e.1).sum();
    while total > room {
        let Some(last) = w.last_mut() else { break };
        last.1 -= 1;
        total -= 1;
        if last.1 == 0 {
            w.pop();
        }
    }
}

/// **The part kinds the greedy fit once left out** (PART-KINDS), per layer and
/// stance, added to the mids and lows `wants` made and only for kinds the
/// library holds for that rack (so a world without one fits as before). The
/// slots come from the tank's own surplus: plates and extenders first, the
/// damage mods down to one; a repairer, a hardener, a booster and the
/// utility stay.
///
/// - Lows: a damage control on every hull of three lows or more; a line ship
///   adds, by stance, speed (kite at four lows, skirmish at five), a tracking
///   enhancer (turret families) or a guidance enhancer (missiles) the same
///   way, a power-grid mod at six, and two drone amplifiers for a drone boat.
/// - Mids: a line ship that keeps its distance a tracking or guidance
///   computer (four mids); a brawler, which runs scrambler, web and
///   propulsion on its capacitor, one capacitor battery (four mids).
///
/// **[A]** the thresholds; falsified by a soak in which these fits lose to
/// the old ones on value (`value`).
fn extras(
    lib: &Library<'_>,
    h: &HullRow,
    family: Family,
    tmpl: Stance,
    role: Role,
    (hi, mut mid, mut low): (Wants, Wants, Wants),
) -> (Wants, Wants, Wants) {
    let line = matches!(role, Role::Dps | Role::Command);
    let apply = match family {
        Family::Missile => Some(Kind::Guidance),
        Family::Drone => None,
        _ => Some(Kind::Tracking),
    };
    let distance = |lows: usize| match tmpl {
        Stance::Kite => h.low >= lows.saturating_sub(1).max(1),
        Stance::Skirmish => h.low >= lows,
        Stance::Brawl => false,
    };
    let mut lo: Wants = Vec::new();
    if h.low >= 3 {
        lo.push((Kind::DamageControl, 1));
    }
    if line {
        if family == Family::Drone {
            lo.push((Kind::DroneMod, if h.low >= 4 { 2 } else { 1 }));
        }
        if h.low >= 6 {
            lo.push((Kind::PowerMod, 1));
        }
        if distance(5) {
            lo.push((Kind::Speed, 1));
            if let Some(k) = apply {
                lo.push((k, 1));
            }
        }
    }
    lo.retain(|(k, _)| lib.has(h, *k, Rack::Low));
    let used: usize = low.iter().map(|e| e.1).sum();
    let need: usize = lo.iter().map(|e| e.1).sum();
    squeeze(
        &mut low,
        (used + need).saturating_sub(h.low),
        &[
            (Kind::Plate, 0),
            (Kind::ShieldExtender, 0),
            (Kind::DamageMod(Aim::Any), 1),
        ],
    );
    // What the tank's surplus could not make room for is dropped, the last
    // (lowest priority) extra first: never more wants than slots.
    let kept: usize = low.iter().map(|e| e.1).sum();
    trim(&mut lo, h.low.saturating_sub(kept));
    lo.extend(low);
    let mut mi: Wants = Vec::new();
    if line
        && h.med >= 4
        && matches!(tmpl, Stance::Kite | Stance::Skirmish)
        && let Some(k) = apply
    {
        mi.push((k, 1));
    }
    if tmpl == Stance::Brawl && h.med >= 4 {
        mi.push((Kind::CapBattery, 1));
    }
    mi.retain(|(k, _)| lib.has(h, *k, Rack::Medium));
    let need: usize = mi.iter().map(|e| e.1).sum();
    let used: usize = mid.iter().map(|e| e.1).sum();
    squeeze(
        &mut mid,
        (used + need).saturating_sub(h.med),
        &[(Kind::ShieldExtender, 0)],
    );
    let kept: usize = mid.iter().map(|e| e.1).sum();
    trim(&mut mi, h.med.saturating_sub(kept));
    mid.extend(mi);
    (hi, mid, lo)
}

/// **The rigs a hull takes** (when [`Limits::rigs`]): the tank of the layer
/// first in a brawler, the layer's tank beside speed in a skirmisher, and a
/// kiter adds a weapon rig; an ewar ship a capacitor rig. Kinds the library
/// has no rig of are skipped by the caller.
fn rig_wants(
    lib: &Library<'_>,
    h: &HullRow,
    layer: Layer,
    tmpl: Stance,
    role: Role,
) -> Vec<(RigKind, usize)> {
    if !lib.limits.rigs || h.rig == 0 {
        return Vec::new();
    }
    let tank = match layer {
        Layer::Armour => RigKind::Armour,
        Layer::Shield | Layer::Hull => RigKind::Shield,
    };
    let line = matches!(role, Role::Dps | Role::Command);
    let mut out: Vec<(RigKind, usize)> = Vec::new();
    let mut left = h.rig;
    let mut take = |k: RigKind, n: usize, left: &mut usize| {
        let n = n.min(*left);
        if n > 0 {
            out.push((k, n));
            *left -= n;
        }
    };
    if line && tmpl == Stance::Kite {
        take(RigKind::Weapon, 1, &mut left);
    }
    if line && tmpl != Stance::Brawl && h.rig >= 2 {
        take(RigKind::Speed, 1, &mut left);
    }
    if role == Role::Ewar {
        take(RigKind::Cap, 1, &mut left);
    }
    let rest = left;
    take(tank, rest, &mut left);
    out
}

fn size_of(c: Class) -> u8 {
    match fit_plan::size_of_class(c) {
        Size::Small => 1,
        Size::Medium => 2,
        Size::Large => 3,
    }
}

/// **The greedy fit** (R-DOCTRINE 2.5) of one role on one hull: the guns of
/// the hull's size first (the best the hull's CPU and power can carry
/// [`GUN_BUDGET`]), then the template's utility, then the tank of the layer,
/// then what is left of the slots. Parts are the highest meta the world can
/// make; at a tie the larger, then the cheaper. The fit is checked on the
/// engine; an overflow drops the last module and retries ([`FIT_RETRIES`]).
/// `None` when no fit passes.
pub fn fill_slots(
    lib: &Library<'_>,
    hull: &HullRow,
    family: Family,
    layer: Layer,
    tmpl: Stance,
    role: Role,
) -> Option<RoleFit> {
    let line = matches!(role, Role::Dps | Role::Command);
    // The drones of a line ship (the whole damage of a drone boat, the
    // supplement of a gun ship with a bay): in the bay, never a slot.
    let drones = if line { drones_for(lib, hull) } else { None };
    if family == Family::Drone && line && drones.is_none() {
        return None;
    }
    let (hi, mid, low) = wants(hull, family, layer, tmpl, role);
    // PART-KINDS: damage control, speed, tracking and guidance, capacitor,
    // power, drone amplifiers in the mids and lows; rigs after the lows.
    let (hi, mid, low) = extras(lib, hull, family, tmpl, role, (hi, mid, low));
    let want_guns = hi.iter().any(|w| matches!(w.0, Kind::Gun(..)));
    if line && !want_guns && family != Family::Drone {
        return None;
    }
    let (out, costs) = budget_of(lib, hull);
    let cost = |p: &Part| cost_of(&costs, p);
    let mut left = out;
    let mut chosen: Vec<(i64, u8)> = Vec::new();
    let mut order: Vec<i64> = Vec::new();
    let mut charge = None;
    let mut alt = None;
    let put = |chosen: &mut Vec<(i64, u8)>, order: &mut Vec<i64>, p: &Part, n: usize| {
        match chosen.iter_mut().find(|c| c.0 == p.type_id) {
            Some(c) => c.1 += n as u8,
            None => chosen.push((p.type_id, n as u8)),
        }
        order.extend(std::iter::repeat_n(p.type_id, n));
    };
    // Highs.
    for (kind, mut n) in hi {
        if matches!(kind, Kind::Gun(..)) {
            // The best gun n of which fit the budget; fewer guns before none.
            let share = (left.0 * GUN_BUDGET, left.1 * GUN_BUDGET);
            let pick = loop {
                if n == 0 {
                    break None;
                }
                let one = (share.0 / n as f64, share.1 / n as f64);
                if let Some(p) =
                    lib.best_where(hull, |p| serves(p.kind, kind), Rack::High, one, &costs)
                {
                    break Some(p);
                }
                n -= 1;
            };
            let Some(p) = pick else { continue };
            let rounds = choose_rounds(lib, p.type_id)?;
            charge = Some(rounds.0);
            alt = rounds.1;
            let c = cost(&p);
            left = (left.0 - c.0 * n as f64, left.1 - c.1 * n as f64);
            put(&mut chosen, &mut order, &p, n);
        } else {
            for _ in 0..n {
                let Some(p) = lib.best_where(hull, |p| p.kind == kind, Rack::High, left, &costs)
                else {
                    break;
                };
                let c = cost(&p);
                left = (left.0 - c.0, left.1 - c.1);
                put(&mut chosen, &mut order, &p, 1);
            }
        }
    }
    if want_guns && charge.is_none() && matches!(role, Role::Dps | Role::Command) {
        return None;
    }
    for (rack, ws) in [(Rack::Medium, mid), (Rack::Low, low)] {
        for (kind, n) in ws {
            for _ in 0..n {
                // A damage module must move the chosen gun.
                let gun = chosen
                    .iter()
                    .find(|c| matches!(lib.kind_of(c.0), Some(Kind::Gun(..))))
                    .map(|c| c.0);
                let table = lib.inst.table();
                let hits = |p: &Part| match (kind, p.kind) {
                    (Kind::DamageMod(_), Kind::DamageMod(aim)) => {
                        gun.map_or(aim == Aim::Any, |g| aim_hits(table, aim, g))
                    }
                    _ => p.kind == kind,
                };
                // No scrambler the hull may carry: a disruptor still pins.
                let pick = lib.best_where(hull, hits, rack, left, &costs).or_else(|| {
                    (kind == Kind::Scram)
                        .then(|| {
                            lib.best_where(hull, |p| p.kind == Kind::Disruptor, rack, left, &costs)
                        })
                        .flatten()
                });
                let Some(p) = pick else {
                    break;
                };
                let c = cost(&p);
                left = (left.0 - c.0, left.1 - c.1);
                put(&mut chosen, &mut order, &p, 1);
            }
        }
    }
    // Spare highs of a brawler take smartbombs (last, so a fit that does not
    // hold them on capacitor or power loses them first).
    if tmpl == Stance::Brawl && matches!(role, Role::Dps | Role::Command) {
        let guns: usize = chosen
            .iter()
            .filter(|c| matches!(lib.kind_of(c.0), Some(Kind::Gun(..))))
            .map(|c| usize::from(c.1))
            .sum();
        let spare = hull.hi.saturating_sub(guns).min(BOMB_MAX);
        let size = size_of(hull.class);
        for _ in 0..spare {
            let Some(p) = lib.best_where(
                hull,
                |p| p.kind == Kind::Smartbomb(size),
                Rack::High,
                left,
                &costs,
            ) else {
                break;
            };
            let c = cost(&p);
            left = (left.0 - c.0, left.1 - c.1);
            put(&mut chosen, &mut order, &p, 1);
        }
    }
    // Rigs last, within the hull's calibration.
    let mut calibration = hull.calibration;
    for (rig, n) in rig_wants(lib, hull, layer, tmpl, role) {
        for _ in 0..n {
            let Some(p) = lib.best_rig(hull, rig, calibration) else {
                break;
            };
            calibration -= p.calibration;
            put(&mut chosen, &mut order, &p, 1);
        }
    }
    if order.is_empty() {
        return None;
    }
    let drone_list: Vec<(i64, u8)> = drones.iter().map(|(p, n)| (p.type_id, *n)).collect();
    // The engine's check; drop the last module added and retry.
    let mut tries = 0;
    loop {
        // A sphere launcher throws probes: the cheapest the world can make.
        // No probe, no bubble, no fit (never one that cannot launch). Chosen
        // before the skills, so the probe's own skill (Astrometrics) is read.
        let launcher = chosen
            .iter()
            .find(|c| lib.kind_of(c.0) == Some(Kind::Sphere))
            .map(|c| c.0);
        let probe = match launcher {
            Some(l) => Some(probe_for(lib, l)?),
            None => None,
        };
        let types: Vec<i64> = std::iter::once(hull.type_id)
            .chain(chosen.iter().map(|c| c.0))
            .chain(charge)
            .chain(alt)
            .chain(probe)
            .chain(drone_list.iter().map(|d| d.0))
            .collect();
        let skills = skills_for(lib.inst.table(), &types, DOCTRINE_LEVEL);
        let profile =
            profile_with_drones(lib, hull.type_id, &chosen, charge, &skills, &drone_list)?;
        if profile.ok() {
            let mut charges = Vec::new();
            let guns: u32 = chosen
                .iter()
                .filter(|c| matches!(lib.kind_of(c.0), Some(Kind::Gun(..))))
                .map(|c| u32::from(c.1))
                .sum();
            if let Some(c) = charge {
                charges.push((c, ROUNDS_PRIMARY * guns.max(1)));
            }
            if let Some(c) = alt {
                charges.push((c, ROUNDS_ALTERNATE * guns.max(1)));
            }
            for &(d, n) in &drone_list {
                charges.push((d, u32::from(n)));
            }
            // The probes go last, so the damage round stays the fit's first.
            if let Some(p) = probe {
                let spheres: u32 = chosen
                    .iter()
                    .filter(|c| lib.kind_of(c.0) == Some(Kind::Sphere))
                    .map(|c| u32::from(c.1))
                    .sum();
                charges.push((p, PROBES_PER_LAUNCHER * spheres));
            }
            let cost = lib.price(hull.type_id)
                + chosen
                    .iter()
                    .map(|c| lib.price(c.0) * f64::from(c.1))
                    .sum::<f64>()
                + charges
                    .iter()
                    .map(|c| lib.price(c.0) * f64::from(c.1))
                    .sum::<f64>();
            return Some(RoleFit {
                hull: hull.type_id,
                modules: chosen,
                charges,
                skills,
                profile,
                cost,
            });
        }
        tries += 1;
        if tries > FIT_RETRIES {
            return None;
        }
        // Drop the last module added (the lowest priority).
        let last = order.pop()?;
        if let Some(i) = chosen.iter().position(|c| c.0 == last) {
            if chosen[i].1 > 1 {
                chosen[i].1 -= 1;
            } else {
                chosen.remove(i);
            }
        }
        if chosen.is_empty() {
            return None;
        }
    }
}

// ---- the catalog ----------------------------------------------------------

fn value(f: &RoleFit) -> f64 {
    f.profile.total_dps() * f.profile.ehp_mean()
}

/// The better of two fits by `score`; ties by cheaper, then lower hull id.
fn better<'f>(a: &'f RoleFit, b: &'f RoleFit, score: &dyn Fn(&RoleFit) -> f64) -> &'f RoleFit {
    let o = score(a)
        .total_cmp(&score(b))
        .then(b.cost.total_cmp(&a.cost))
        .then(b.hull.cmp(&a.hull));
    if o.is_ge() { a } else { b }
}

/// The best fit of a support role for a class, family and layer.
fn support(
    lib: &Library<'_>,
    class: Class,
    family: Family,
    layer: Layer,
    role: Role,
) -> Option<RoleFit> {
    let hulls: Vec<&HullRow> = match role {
        // Tackle is flown by the smallest hulls that exist.
        Role::Tackle => {
            let smallest = lib.hulls_for(role).map(|h| h.class).min()?;
            lib.hulls_for(role)
                .filter(|h| h.class == smallest)
                .collect()
        }
        // A bubble is thrown or generated by a hull of its own group, no
        // bigger than the line (a destroyer-size dictor serves a frigate line).
        Role::Interdict => lib
            .hulls_for(role)
            .filter(|h| h.class <= class.max(Class::Destroyer))
            .collect(),
        // The eye is the smallest hull a cloak fits.
        Role::Scout => {
            let cloaks = |h: &&HullRow| {
                lib.parts
                    .iter()
                    .any(|p| p.kind == Kind::Cloak && lib.can_fit(p, h))
            };
            let smallest = lib.hulls_for(role).filter(cloaks).map(|h| h.class).min()?;
            lib.hulls_for(role)
                .filter(cloaks)
                .filter(|h| h.class == smallest)
                .collect()
        }
        _ => lib.hulls_for(role).filter(|h| h.class == class).collect(),
    };
    let fam = |h: &HullRow| match role {
        Role::Tackle if family == Family::Missile && h.launchers == 0 => Family::Projectile,
        // The launcher hardpoint of a dictor is the bubble's.
        Role::Interdict if family == Family::Missile => Family::Projectile,
        _ => family,
    };
    let score: Box<dyn Fn(&RoleFit) -> f64> = match role {
        Role::Logi => Box::new(|f| f.profile.remote_rep * f.profile.ehp_mean()),
        Role::Tackle => Box::new(|f| {
            let e = &f.profile.ewar;
            (f64::from(u8::from(e.scram)) + e.web_slow + 0.01)
                * f.profile.speed.max(1.0)
                * f.profile.ehp_mean()
        }),
        // The sturdiest hull that holds a bubble.
        Role::Interdict => Box::new(|f| f.profile.ehp_mean()),
        // The fastest hull with the smallest signature.
        Role::Scout => Box::new(|f| f.profile.speed.max(1.0) / f.profile.sig.max(1.0)),
        _ => Box::new(|f| {
            let e = &f.profile.ewar;
            (e.ecm.iter().sum::<f64>()
                + e.paint
                + e.damp
                + e.neut / 100.0
                + e.track_cut
                + e.guide_cut)
                * f.profile.ehp_mean()
        }),
    };
    let mut best: Option<RoleFit> = None;
    for h in hulls {
        let Some(f) = fill_slots(lib, h, fam(h), layer, Stance::Brawl, role) else {
            continue;
        };
        let useful = match role {
            Role::Logi => f.profile.remote_rep > 0.0,
            Role::Tackle => f.profile.ewar.pin_range > 0.0 || f.profile.ewar.scram,
            Role::Interdict => f
                .modules
                .iter()
                .any(|m| matches!(lib.kind_of(m.0), Some(Kind::Sphere | Kind::Field))),
            Role::Scout => f
                .modules
                .iter()
                .any(|m| lib.kind_of(m.0) == Some(Kind::Cloak)),
            _ => score(&f) > 0.0,
        };
        if !useful {
            continue;
        }
        best = Some(match best {
            Some(b) => better(&b, &f, &*score).clone(),
            None => f,
        });
    }
    best
}

/// **The catalog** over a classified library (see [`crate::doctrine::catalog`]).
pub fn catalog_in(lib: &Library<'_>) -> Vec<Doctrine> {
    catalog_with(lib, &crate::doctrine_real::seeds(lib))
}

/// [`catalog_in`] with the real fits already built: they stand in the
/// candidates beside the greedy fits, so the per-key best by value decides.
pub fn catalog_with(lib: &Library<'_>, seeds: &crate::doctrine_real::RealSeeds) -> Vec<Doctrine> {
    struct Cand {
        class: Class,
        family: Family,
        layer: Layer,
        fit: RoleFit,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for h in lib.hulls_for(Role::Dps) {
        for family in [
            Family::Projectile,
            Family::Hybrid,
            Family::Laser,
            Family::Missile,
            Family::Drone,
        ] {
            let hardpoints = match family {
                Family::Missile => h.launchers,
                // A drone boat needs a bay, not a hardpoint.
                Family::Drone => usize::from(drones_for(lib, h).is_some()),
                _ => h.turrets,
            };
            if hardpoints == 0 {
                continue;
            }
            for layer in [Layer::Shield, Layer::Armour] {
                for tmpl in Stance::ALL {
                    if let Some(fit) = fill_slots(lib, h, family, layer, tmpl, Role::Dps) {
                        cands.push(Cand {
                            class: h.class,
                            family,
                            layer,
                            fit,
                        });
                    }
                }
            }
        }
    }
    for (family, _, s) in seeds.iter() {
        cands.push(Cand {
            class: s.class,
            family,
            layer: layer_of(&s.fit.profile.defence),
            fit: s.fit.clone(),
        });
    }
    let mut speeds: Vec<f64> = cands.iter().map(|c| c.fit.profile.speed).collect();
    speeds.sort_by(f64::total_cmp);
    let median = speeds.get(speeds.len() / 2).copied().unwrap_or(0.0);

    let mut memo: BTreeMap<(Class, Family, Layer, Role), Option<RoleFit>> = BTreeMap::new();
    let mut best: BTreeMap<String, Doctrine> = BTreeMap::new();
    for c in cands {
        let mut get = |role: Role| -> Option<RoleFit> {
            memo.entry((c.class, c.family, c.layer, role))
                .or_insert_with(|| support(lib, c.class, c.family, c.layer, role))
                .clone()
        };
        let logi = get(Role::Logi);
        let tackle = get(Role::Tackle);
        let ewar = get(Role::Ewar);
        let interdict = get(Role::Interdict);
        let scout = get(Role::Scout);
        let shares = [
            (Role::Logi, logi.as_ref().map_or(0.0, |_| SHARE_LOGI)),
            (Role::Tackle, tackle.as_ref().map_or(0.0, |_| SHARE_TACKLE)),
            (Role::Ewar, ewar.as_ref().map_or(0.0, |_| SHARE_EWAR)),
            (
                Role::Interdict,
                interdict.as_ref().map_or(0.0, |_| SHARE_INTERDICT),
            ),
            (Role::Scout, scout.as_ref().map_or(0.0, |_| SHARE_SCOUT)),
        ];
        let tackle_share = shares[1].1;
        let dps_share = 1.0 - shares.iter().map(|s| s.1).sum::<f64>();
        let reach = c.fit.profile.reach();
        let range = range_of(reach);
        let stance = stance_of(c.fit.profile.speed, median, range, tackle_share);
        let layer = layer_of(&c.fit.profile.defence);
        let key = key_of(layer, range, stance, c.class, c.family);
        let mut roles = vec![(Role::Dps, dps_share, c.fit.clone())];
        for ((role, share), fit) in shares
            .into_iter()
            .zip([logi, tackle, ewar, interdict, scout])
        {
            if let Some(f) = fit {
                roles.push((role, share, f));
            }
        }
        let d = Doctrine {
            key: key.clone(),
            class: c.class,
            family: c.family,
            layer,
            active: c.fit.profile.rep_rate() > 0.0,
            range,
            stance,
            roles,
            fleet: fleet_size(c.class),
        };
        let keep = match best.get(&key) {
            None => true,
            Some(old) => {
                let (a, b) = (d.line().map(value), old.line().map(value));
                a.partial_cmp(&b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        old.cost_per_ship()
                            .partial_cmp(&d.cost_per_ship())
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
                    .is_gt()
            }
        };
        if keep {
            best.insert(key, d);
        }
    }
    best.into_values().collect()
}

/// A range label of a gun's reach, for callers that hold only a profile.
pub fn range_label(p: &Profile) -> Range {
    range_of(p.reach())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::*;
    use crate::doctrine_exchange::{Features, dominant, matrix, matrix_line};
    use eve_server::damage::ShipDamage;

    fn lib_of(inst: &Installed) -> Library<'_> {
        Library::build(
            inst,
            &Makeable::all(),
            &|t| 100.0 + (t % 7) as f64,
            Limits::default(),
        )
    }

    fn hull_of<'l>(lib: &'l Library<'_>, id: i64) -> &'l HullRow {
        lib.hulls.iter().find(|h| h.type_id == id).expect("hull")
    }

    #[test]
    fn parts_are_read_from_the_table_by_what_they_state() {
        let inst = world();
        let lib = lib_of(&inst);
        let kinds: BTreeSet<String> = lib.parts.iter().map(|p| format!("{:?}", p.kind)).collect();
        for want in [
            "Gun(Projectile, 1)",
            "Gun(Hybrid, 2)",
            "Gun(Laser, 3)",
            "Gun(Missile, 2)",
            "ShieldExtender",
            "Plate",
            "ArmourRepairer",
            "ShieldBooster",
            "DamageMod(Skill(970000001))",
            "Prop",
            "Web",
            "Scram",
            "Painter",
            "Damp",
            "Ecm",
            "Neut",
            "RemoteRep(Armour)",
            "RemoteRep(Shield)",
            "Hardener(Shield)",
            "Hardener(Armour)",
        ] {
            assert!(kinds.contains(want), "{want} in {kinds:?}");
        }
        // The named plate is above the meta cap and is not offered.
        assert!(lib.parts.iter().all(|p| p.type_id != NAMED_PLATE));
        // Five T1 hulls' worth: six rows in three classes.
        assert_eq!(lib.hulls.len(), 6);
    }

    /// The invented world with extra tackle rows: a disruptor (default
    /// effect 39), a burst jammer (6714, jam strengths, no target) and a
    /// scrambler only a cruiser group may carry.
    const DISRUPTOR: i64 = 970_004_001;
    const BURST: i64 = 970_004_002;
    const CRUISER_SCRAM: i64 = 970_004_003;
    const CRUISER_GROUP: i64 = 26;

    fn tackle_world() -> Installed {
        let mut text = world_text();
        let mut line = |s: String| {
            text.push_str(&s);
            text.push('\n');
        };
        line("effect\t39\t2\t1\t73\t6\t54\t0\t0\twarpScrambler\t".to_owned());
        line("effect\t6714\t1\t0\t73\t6\t0\t0\t0\tburstJammer\t".to_owned());
        for (id, name, fx, attrs) in [
            (DISRUPTOR, "Disruptor", 39, "105=1.0;54=20000.0"),
            (BURST, "Burst", 6714, "241=3.0;239=3.0;240=3.0;238=3.0"),
            (
                CRUISER_SCRAM,
                "Cruiser scrambler",
                9320,
                &format!("105=3.0;54=9000.0;1298={CRUISER_GROUP}"),
            ),
        ] {
            line(format!(
                "type\t{id}\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t{fx}\t13,16,9320\t\
                 6=5.0;30=2.0;50=15.0;73=5000.0;{attrs};633=0.0\t{name}"
            ));
        }
        Installed::build(eve_server::typedata::Table::parse(&text).expect("parses"))
    }

    #[test]
    fn a_disruptor_is_its_own_kind() {
        let inst = tackle_world();
        let lib = lib_of(&inst);
        assert_eq!(lib.kind_of(DISRUPTOR), Some(Kind::Disruptor));
        // The scrambler of the plain world is still a scrambler.
        assert!(lib.parts.iter().any(|p| p.kind == Kind::Scram));
    }

    #[test]
    fn a_burst_jammer_is_not_an_ecm() {
        let inst = tackle_world();
        let lib = lib_of(&inst);
        assert_eq!(lib.kind_of(BURST), None);
        assert!(lib.parts.iter().any(|p| p.kind == Kind::Ecm));
    }

    #[test]
    fn a_restricted_module_stays_off_other_hulls() {
        let inst = tackle_world();
        let lib = lib_of(&inst);
        let part = lib
            .parts
            .iter()
            .find(|p| p.type_id == CRUISER_SCRAM)
            .expect("part");
        assert!(lib.can_fit(part, hull_of(&lib, CRUISER)));
        assert!(!lib.can_fit(part, hull_of(&lib, FRIGATE)));
        // No restriction named: any hull.
        let free = lib.parts.iter().find(|p| p.type_id == DISRUPTOR).unwrap();
        assert!(lib.can_fit(free, hull_of(&lib, FRIGATE)));
        // `best` honours it: the restricted scrambler is chosen on the
        // cruiser (it outranks the plain one by strength) and never on the
        // frigate.
        let rack = Rack::Medium;
        let on = |h: &HullRow| lib.best(h, |k| k == Kind::Scram, rack, (1e9, 1e9));
        assert_eq!(
            on(hull_of(&lib, CRUISER)).map(|p| p.type_id),
            Some(CRUISER_SCRAM)
        );
        assert_ne!(
            on(hull_of(&lib, FRIGATE)).map(|p| p.type_id),
            Some(CRUISER_SCRAM)
        );
    }

    /// The invented world with a medium smartbomb, a jump drive, a cloak,
    /// a tracking disruptor and a guidance disruptor.
    const BOMB: i64 = 970_005_001;
    const JUMP: i64 = 970_005_002;
    const CLOAK: i64 = 970_005_003;
    const TRACKD: i64 = 970_005_004;
    const GUIDED: i64 = 970_005_005;

    fn kit_world() -> Installed {
        let mut text = world_text();
        let mut line = |s: String| {
            text.push_str(&s);
            text.push('\n');
        };
        line("effect\t38\t1\t1\t73\t6\t0\t0\t0\tempWave\t".to_owned());
        line("effect\t607\t1\t0\t73\t6\t0\t0\t0\tcloaking\t".to_owned());
        line("effect\t4921\t1\t0\t73\t6\t0\t0\t0\tmicroJumpDrive\t".to_owned());
        line("effect\t6424\t2\t1\t73\t6\t54\t0\t0\ttrackingDisruptor\t".to_owned());
        line("effect\t6423\t2\t1\t73\t6\t54\t0\t0\tguidanceDisruptor\t".to_owned());
        for (id, name, fx, rack, attrs) in [
            (BOMB, "Bomb", 38, 12, "99=4000.0;116=60.0;117=40.0"),
            (JUMP, "Jump", 4921, 13, "54=100000.0"),
            (CLOAK, "Cloak", 607, 12, ""),
            (
                TRACKD,
                "Tracking disruptor",
                6424,
                13,
                "54=20000.0;2044=10000.0;767=-20.0;351=-10.0",
            ),
            (
                GUIDED,
                "Guidance disruptor",
                6423,
                13,
                "54=20000.0;2044=10000.0;547=-10.0;847=-30.0;848=20.0",
            ),
        ] {
            line(format!(
                "type\t{id}\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t{fx}\t{rack},16,{fx}\t\
                 6=5.0;30=2.0;50=15.0;73=5000.0;{attrs};633=0.0\t{name}"
            ));
        }
        Installed::build(eve_server::typedata::Table::parse(&text).expect("parses"))
    }

    #[test]
    fn the_new_kinds_are_read_from_their_effects() {
        let inst = kit_world();
        let lib = lib_of(&inst);
        assert_eq!(lib.kind_of(BOMB), Some(Kind::Smartbomb(2)));
        assert_eq!(lib.kind_of(JUMP), Some(Kind::Mjd));
        assert_eq!(lib.kind_of(CLOAK), Some(Kind::Cloak));
        assert_eq!(lib.kind_of(TRACKD), Some(Kind::TrackDisrupt));
        assert_eq!(lib.kind_of(GUIDED), Some(Kind::GuideDisrupt));
    }

    #[test]
    fn a_brawler_takes_a_bomb_in_its_spare_high_and_it_is_counted() {
        let inst = kit_world();
        let lib = lib_of(&inst);
        let hull = hull_of(&lib, CRUISER);
        let fit = fill_slots(
            &lib,
            hull,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
            Role::Dps,
        )
        .expect("fit");
        assert!(fit.modules.iter().any(|m| m.0 == BOMB), "{:?}", fit.modules);
        assert!(fit.profile.ewar.bomb.iter().sum::<f64>() > 0.0);
        assert!(fit.profile.ewar.bomb_range > 0.0);
        // A kite keeps its highs for guns: no bomb.
        let kite = fill_slots(
            &lib,
            hull,
            Family::Projectile,
            Layer::Armour,
            Stance::Kite,
            Role::Dps,
        )
        .expect("fit");
        assert!(!kite.modules.iter().any(|m| m.0 == BOMB));
    }

    #[test]
    fn only_a_big_hull_that_keeps_distance_takes_a_jump_drive() {
        let inst = kit_world();
        let lib = lib_of(&inst);
        let take = |hull: i64, tmpl| {
            let fit = fill_slots(
                &lib,
                hull_of(&lib, hull),
                Family::Projectile,
                Layer::Armour,
                tmpl,
                Role::Dps,
            )
            .expect("fit");
            (fit.modules.iter().any(|m| m.0 == JUMP), fit.profile.mjd)
        };
        assert_eq!(take(BATTLESHIP, Stance::Kite), (true, true));
        assert_eq!(take(BATTLESHIP, Stance::Brawl), (false, false));
        assert_eq!(take(CRUISER, Stance::Kite), (false, false));
    }

    #[test]
    fn an_ewar_ship_carries_the_disruptors_and_reads_their_cuts() {
        let inst = kit_world();
        let lib = lib_of(&inst);
        let fit = fill_slots(
            &lib,
            hull_of(&lib, BATTLESHIP),
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
            Role::Ewar,
        )
        .expect("fit");
        let e = &fit.profile.ewar;
        assert!(fit.modules.iter().any(|m| m.0 == TRACKD));
        assert!(fit.modules.iter().any(|m| m.0 == GUIDED));
        assert!(e.track_cut > 0.0 && e.opt_cut > 0.0 && e.track_range > 0.0);
        assert!(e.guide_cut > 0.0 && e.guide_range > 0.0);
    }

    #[test]
    fn a_part_with_no_source_is_not_in_the_catalog() {
        let inst = world();
        let all = lib_of(&inst);
        let ghost = all
            .parts
            .iter()
            .find(|p| p.kind == Kind::Web)
            .map(|p| p.type_id)
            .expect("a web");
        let others: Vec<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            .filter(|t| *t != ghost)
            .collect();
        let lib = Library::build(
            &inst,
            &Makeable::of(others),
            &|t| 100.0 + (t % 7) as f64,
            Limits::default(),
        );
        assert!(lib.parts.iter().all(|p| p.type_id != ghost));
        assert!(all.parts.iter().any(|p| p.type_id == ghost));
    }

    #[test]
    fn a_fit_is_rejected_over_its_power_grid() {
        let inst = world();
        let lib = lib_of(&inst);
        let gun = SMALL_PROJECTILE;
        // Three guns on a hull with no CPU: the engine says no.
        let p = profile_of(&lib, CRAMPED, &[(gun, 3)], None, &[]).unwrap();
        assert!(!p.cpu_ok && !p.ok());
        // The greedy fit finds no gun the hull can carry.
        assert!(
            fill_slots(
                &lib,
                hull_of(&lib, CRAMPED),
                Family::Projectile,
                Layer::Armour,
                Stance::Skirmish,
                Role::Dps
            )
            .is_none()
        );
        // Plates overflow a frigate's power grid.
        let big_plate = lib
            .parts
            .iter()
            .filter(|p| p.kind == Kind::Plate)
            .max_by(|a, b| a.power.total_cmp(&b.power))
            .unwrap();
        let p = profile_of(&lib, FRIGATE, &[(big_plate.type_id, 3)], None, &[]).unwrap();
        assert!(!p.pg_ok);
        // And the same hull fits fine within its grid.
        let f = fill_slots(
            &lib,
            hull_of(&lib, FRIGATE),
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
            Role::Dps,
        )
        .expect("a frigate fit");
        assert!(f.profile.ok());
    }

    #[test]
    fn paper_ehp_equals_the_damage_engines_ehp() {
        let inst = world();
        let lib = lib_of(&inst);
        let cat = catalog_in(&lib);
        assert!(!cat.is_empty());
        for d in &cat {
            for (_, _, fit) in &d.roles {
                let p = &fit.profile;
                for (t, ehp) in p.ehp().into_iter().enumerate() {
                    let pure = |amount: f64| {
                        let mut v = eve_server::damage::Damage::ZERO;
                        match t {
                            0 => v.em = amount,
                            1 => v.explosive = amount,
                            2 => v.kinetic = amount,
                            _ => v.thermal = amount,
                        }
                        v
                    };
                    let mut a = ShipDamage::full(p.defence, 0);
                    assert!(
                        a.apply(pure(ehp * 0.999), 0).death.is_none(),
                        "{} {t}",
                        d.key
                    );
                    let mut b = ShipDamage::full(p.defence, 0);
                    assert!(
                        b.apply(pure(ehp * 1.001), 0).death.is_some(),
                        "{} {t}",
                        d.key
                    );
                }
            }
        }
    }

    #[test]
    fn a_catalog_has_every_label() {
        let inst = world();
        let lib = lib_of(&inst);
        let cat = catalog_in(&lib);
        let keys: BTreeSet<&str> = cat.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys.len(), cat.len(), "one doctrine per key");
        let has = |f: &dyn Fn(&Doctrine) -> bool| cat.iter().any(f);
        for layer in [Layer::Shield, Layer::Armour] {
            assert!(has(&|d| d.layer == layer), "{layer:?}");
        }
        for range in Range::ALL {
            assert!(has(&|d| d.range == range), "{range:?}: {keys:?}");
        }
        for stance in Stance::ALL {
            assert!(has(&|d| d.stance == stance), "{stance:?}: {keys:?}");
        }
        for family in [
            Family::Projectile,
            Family::Hybrid,
            Family::Laser,
            Family::Missile,
        ] {
            assert!(has(&|d| d.family == family), "{family:?}");
        }
        assert!(
            has(&|d| d.active) && has(&|d| !d.active),
            "reps and buffers"
        );
        for d in &cat {
            let sum: f64 = d.roles.iter().map(|r| r.1).sum();
            assert!((sum - 1.0).abs() < 1e-9, "{} shares {sum}", d.key);
            assert!(d.roles.iter().all(|(_, _, f)| f.profile.ok()), "{}", d.key);
            assert!(d.cost_per_ship() > 0.0 && d.line().is_some());
        }
        // A world with no parts has no doctrines.
        let none = Library::build(&inst, &Makeable::of([]), &|_| 1.0, Limits::default());
        assert!(catalog_in(&none).is_empty());
    }

    #[test]
    fn skills_follow_the_fit_and_support_skills_join_once() {
        let inst = world();
        let lib = lib_of(&inst);
        let f = fill_slots(
            &lib,
            hull_of(&lib, FRIGATE),
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
            Role::Dps,
        )
        .unwrap();
        let ids: Vec<i64> = f.skills.iter().map(|s| s.0).collect();
        assert!(
            ids.contains(&GUNNERY) && ids.contains(&HULL_SKILL),
            "{ids:?}"
        );
        assert!(
            ids.contains(&SPECIALISATION),
            "the specialisation moves the guns: {ids:?}"
        );
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, ids, "ascending and once each");
        assert!(f.skills.iter().all(|s| s.1 >= DOCTRINE_LEVEL));
        // The specialisation raises the fit's damage: with it, more dps.
        let without: Vec<_> = f
            .skills
            .iter()
            .copied()
            .filter(|s| s.0 != SPECIALISATION)
            .collect();
        let weak = profile_of(
            &lib,
            f.hull,
            &f.modules,
            f.charges.first().map(|c| c.0),
            &without,
        )
        .unwrap();
        assert!(f.profile.total_dps() > weak.total_dps());
    }

    #[test]
    fn rounds_follow_the_guns_group_and_size_and_an_alternate_differs() {
        let inst = world();
        let lib = lib_of(&inst);
        let rounds = rounds_of(inst.table(), SMALL_PROJECTILE);
        assert_eq!(rounds.len(), 2, "two small projectile rounds: {rounds:?}");
        let (primary, alt) = choose_rounds(&lib, SMALL_PROJECTILE).unwrap();
        let alt = alt.expect("an alternate damage type");
        assert_ne!(primary, alt);
    }

    #[test]
    fn a_fast_fit_with_a_prop_outruns_one_without() {
        let inst = world();
        let lib = lib_of(&inst);
        let prop = lib
            .parts
            .iter()
            .find(|p| p.kind == Kind::Prop)
            .unwrap()
            .type_id;
        let bare = profile_of(&lib, FRIGATE, &[], None, &[]).unwrap();
        let with = profile_of(&lib, FRIGATE, &[(prop, 1)], None, &[]).unwrap();
        assert!(with.speed > bare.speed, "{} vs {}", with.speed, bare.speed);
    }

    #[test]
    fn no_doctrine_dominates() {
        let inst = world();
        let lib = lib_of(&inst);
        let cat = catalog_in(&lib);
        let on = Features::default();
        let dom = dominant(&cat, on);
        let m = matrix(&cat, on);
        let line = matrix_line(&cat, &m, &[], on);
        println!("{line}");
        assert!(
            dom.is_empty(),
            "{:?}: {line}",
            dom.iter().map(|i| &cat[*i].key).collect::<Vec<_>>()
        );
        assert!(line.ends_with("dominant: none"));
    }
    // ---- BP-T2-HULLS: interdictors, heavy interdictors and scouts ----------

    /// Invented T2 rows: a dictor (group 541), a heavy interdictor (894), a
    /// covert ops frigate (830), the sphere launcher (589) and its probes
    /// (548), the field generator (effect 3380), a covert cloak, the skill
    /// the launcher needs at level 5.
    const PROP_JAM: i64 = 970_006_001;
    const DICTOR: i64 = 970_006_100;
    const HIC: i64 = 970_006_101;
    const COVERT: i64 = 970_006_102;
    const SPHERE: i64 = 970_006_200;
    const FIELD: i64 = 970_006_201;
    const COVERT_CLOAK: i64 = 970_006_202;
    const ASTRO: i64 = 970_006_901;
    const PROBE_DEAR: i64 = 970_006_300;
    const PROBE_CHEAP: i64 = 970_006_301;
    const DICTOR_GROUP: i64 = 541;
    const HIC_GROUP: i64 = 894;
    const COVERT_GROUP: i64 = 830;

    fn t2_world() -> Installed {
        let mut text = world_text();
        let mut line = |s: String| {
            text.push_str(&s);
            text.push('\n');
        };
        line(format!(
            "type\t{PROP_JAM}\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100\t\tPropulsion Jamming"
        ));
        line(format!(
            "type\t{ASTRO}\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100\t\tAstrometrics"
        ));
        line("effect\t607\t1\t0\t73\t6\t0\t0\t0\tcloaking\t".to_owned());
        line("effect\t3380\t1\t0\t73\t6\t0\t0\t0\twarpDisruptSphere\t".to_owned());
        // (id, group, name, [hi, med, low, turrets, launchers], [cpu, pg],
        //  [shield, armour, hull], [speed, sig, lock, mass])
        for (id, group, name, slots, grid, hp, motion) in [
            (
                DICTOR,
                DICTOR_GROUP,
                "Dictor",
                [5.0, 4.0, 3.0, 3.0, 1.0],
                [250.0, 100.0],
                [900.0, 700.0, 600.0],
                [450.0, 60.0, 60000.0, 1.2e6],
            ),
            (
                HIC,
                HIC_GROUP,
                "Heavy dictor",
                [5.0, 5.0, 4.0, 3.0, 0.0],
                [500.0, 4000.0],
                [3000.0, 3000.0, 2000.0],
                [250.0, 150.0, 70000.0, 1.2e7],
            ),
            (
                COVERT,
                COVERT_GROUP,
                "Eye",
                [3.0, 3.0, 2.0, 1.0, 0.0],
                [200.0, 40.0],
                [300.0, 300.0, 200.0],
                [500.0, 40.0, 50000.0, 1.2e6],
            ),
        ] {
            let [hi, med, low, turrets, launchers] = slots;
            let [cpu, pg] = grid;
            let [shield, armour, hull] = hp;
            let [speed, sig, lock, mass] = motion;
            line(format!(
                "type\t{id}\t{group}\t6\t1\t1\t1000.0\t100.0\t{mass}\t30.0\t0\t\
                 \t9={hull};11={pg};12={low};13={med};14={hi};37={speed};48={cpu};55=150000.0;70=3.0;\
                 76={lock};101={launchers};102={turrets};182={HULL_SKILL};192=5.0;208=14.0;209=0.0;\
                 210=0.0;211=0.0;263={shield};265={armour};277=1.0;422=2.0;479=600000.0;482=500.0;\
                 552={sig};564=500.0;1137=3.0\t{name}"
            ));
        }
        for (id, group, fx, rack, attrs, name) in [
            (
                SPHERE,
                589,
                "101",
                "12,16,40,101",
                format!("182={PROP_JAM};277=5.0;51=4000.0;604=548.0;1298={DICTOR_GROUP}"),
                "Sphere launcher",
            ),
            (
                FIELD,
                899,
                "3380",
                "12,16,3380",
                format!("73=5000.0;1298={HIC_GROUP}"),
                "Field generator",
            ),
            (
                COVERT_CLOAK,
                60,
                "607",
                "12,16,607",
                format!("1298={COVERT_GROUP}"),
                "Covert cloak",
            ),
        ] {
            line(format!(
                "type\t{id}\t{group}\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t{fx}\t{rack}\t\
                 6=0.0;30=5.0;50=10.0;{attrs};633=0.0\t{name}"
            ));
        }
        for (id, name) in [(PROBE_DEAR, "Probe"), (PROBE_CHEAP, "Cheap probe")] {
            line(format!(
                "type\t{id}\t548\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t182={ASTRO};277=1.0;633=0.0\t{name}"
            ));
        }
        Installed::build(eve_server::typedata::Table::parse(&text).expect("parses"))
    }

    /// A library over `t2_world` whose makeable set is everything but
    /// `without`, priced so `PROBE_CHEAP` is cheaper.
    fn t2_lib<'a>(inst: &'a Installed, without: &[i64], limits: Limits) -> Library<'a> {
        let others: Vec<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            .filter(|t| !without.contains(t))
            .collect();
        Library::build(
            inst,
            &Makeable::of(others),
            &|t| {
                if t == PROBE_CHEAP {
                    1.0
                } else {
                    100.0 + (t % 7) as f64
                }
            },
            limits,
        )
    }

    fn fit_of(lib: &Library<'_>, hull: i64, role: Role) -> Option<RoleFit> {
        fill_slots(
            lib,
            hull_of(lib, hull),
            Family::Projectile,
            Layer::Shield,
            Stance::Brawl,
            role,
        )
    }

    #[test]
    fn a_frigate_line_gets_no_heavy_interdictor() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let seat = |class| {
            support(
                &lib,
                class,
                Family::Projectile,
                Layer::Shield,
                Role::Interdict,
            )
            .map(|f| f.hull)
        };
        assert_eq!(seat(Class::Frigate), Some(DICTOR));
        assert_eq!(seat(Class::Destroyer), Some(DICTOR));
        assert!(seat(Class::Cruiser).is_some());
        let hulls = |class: Class| {
            lib.hulls_for(Role::Interdict)
                .filter(|h| h.class <= class.max(Class::Destroyer))
                .map(|h| h.type_id)
                .collect::<Vec<_>>()
        };
        assert!(!hulls(Class::Frigate).contains(&HIC));
        assert!(hulls(Class::Cruiser).contains(&HIC));
    }

    #[test]
    fn the_bubble_parts_are_read_from_the_group_and_the_effect() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        assert_eq!(lib.kind_of(SPHERE), Some(Kind::Sphere));
        assert_eq!(lib.kind_of(FIELD), Some(Kind::Field));
        assert_eq!(lib.kind_of(COVERT_CLOAK), Some(Kind::Cloak));
        // The launcher fits a dictor and the generator a heavy interdictor,
        // each its own group and no other (`can_fit` picks sphere or field).
        let part = |id| *lib.parts.iter().find(|p| p.type_id == id).unwrap();
        assert!(lib.can_fit(&part(SPHERE), hull_of(&lib, DICTOR)));
        assert!(!lib.can_fit(&part(SPHERE), hull_of(&lib, HIC)));
        assert!(lib.can_fit(&part(FIELD), hull_of(&lib, HIC)));
        assert!(!lib.can_fit(&part(FIELD), hull_of(&lib, DICTOR)));
        assert!(!lib.can_fit(&part(COVERT_CLOAK), hull_of(&lib, DICTOR)));
    }

    #[test]
    fn the_tech_two_hulls_belong_to_their_roles_only() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let ids = |role| -> Vec<i64> { lib.hulls_for(role).map(|h| h.type_id).collect() };
        assert_eq!(ids(Role::Interdict), [DICTOR, HIC]);
        assert!(!ids(Role::Scout).contains(&DICTOR));
        for role in [Role::Dps, Role::Logi, Role::Tackle, Role::Ewar] {
            assert!(ids(role).iter().all(|h| ![DICTOR, HIC, COVERT].contains(h)));
        }
        // Even a line raised to tech 2 does not fly the role hulls.
        let high = t2_lib(
            &inst,
            &[],
            Limits {
                tech_max: 2.0,
                ..Limits::default()
            },
        );
        assert!(
            high.hulls_for(Role::Dps)
                .all(|h| ![DICTOR, HIC, COVERT].contains(&h.type_id))
        );
        // The role's own cap: at tech 1 there is no dictor and no eye.
        let line_only = t2_lib(
            &inst,
            &[],
            Limits {
                tech_special: 1.0,
                ..Limits::default()
            },
        );
        assert!(line_only.hulls.iter().all(|h| h.tech <= 1.0));
        assert_eq!(line_only.hulls_for(Role::Interdict).count(), 0);
    }

    #[test]
    fn a_dictor_carries_the_launcher_and_probes_a_heavy_interdictor_the_field() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let dictor = fit_of(&lib, DICTOR, Role::Interdict).expect("dictor fit");
        assert!(dictor.modules.iter().any(|m| m.0 == SPHERE));
        assert!(!dictor.modules.iter().any(|m| m.0 == FIELD));
        // The cheapest probe, after the damage round (the fit's first charge
        // stays the gun's, `fit_plan::fit_from_role` reads that one).
        let (probe, n) = *dictor.charges.last().expect("charges");
        assert_eq!(probe, PROBE_CHEAP);
        assert_eq!(n, PROBES_PER_LAUNCHER);
        assert!(dictor.charges.len() > 1);
        let gun_round = dictor.charges[0].0;
        assert_ne!(gun_round, probe);
        let table = inst.table();
        let gun = crate::fit_plan::fit_from_role(table, &dictor).expect("gun fit");
        assert_eq!(gun.round, gun_round);
        // The order that buys the fit asks for the probes too.
        let order = crate::acquire::Order::from_fit(&dictor);
        assert!(order.charges.iter().any(|c| c.0 == probe));
        assert!(order.modules.iter().any(|m| m.0 == SPHERE));
        let hic = fit_of(&lib, HIC, Role::Interdict).expect("hic fit");
        assert!(hic.modules.iter().any(|m| m.0 == FIELD));
        assert!(!hic.modules.iter().any(|m| m.0 == SPHERE));
        assert!(hic.charges.iter().all(|c| c.0 != PROBE_CHEAP));
    }

    #[test]
    fn the_skill_plan_keeps_the_launchers_level_five() {
        const { assert!(DOCTRINE_LEVEL < 5) };
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let fit = fit_of(&lib, DICTOR, Role::Interdict).expect("dictor fit");
        let level = |s| fit.skills.iter().find(|x| x.0 == s).map(|x| x.1);
        assert_eq!(level(PROP_JAM), Some(5));
        // The probe's own skill is read, so it is planned.
        assert_eq!(level(ASTRO), Some(DOCTRINE_LEVEL));
        // The guns' skill is only at the doctrine's level.
        assert_eq!(level(GUNNERY), Some(DOCTRINE_LEVEL));
    }

    #[test]
    fn a_launcher_without_a_makeable_probe_is_no_fit() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[PROBE_DEAR, PROBE_CHEAP], Limits::default());
        assert!(fit_of(&lib, DICTOR, Role::Interdict).is_none());
        // The field needs none.
        assert!(fit_of(&lib, HIC, Role::Interdict).is_some());
    }

    #[test]
    fn the_scout_is_the_smallest_hull_a_cloak_fits() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let eye = fit_of(&lib, COVERT, Role::Scout).expect("scout fit");
        assert_eq!(
            eye.modules.iter().filter(|m| m.0 == COVERT_CLOAK).count(),
            1
        );
        let scout = support(
            &lib,
            Class::Cruiser,
            Family::Projectile,
            Layer::Shield,
            Role::Scout,
        );
        assert_eq!(scout.map(|f| f.hull), Some(COVERT));
    }

    #[test]
    fn a_t2_role_appears_only_when_the_hull_is_supplied() {
        let inst = t2_world();
        let roles = |without: &[i64]| -> Vec<(Role, f64, i64)> {
            let lib = t2_lib(&inst, without, Limits::default());
            let cat = catalog_in(&lib);
            assert!(!cat.is_empty());
            let mut seen = Vec::new();
            for d in &cat {
                let sum: f64 = d.roles.iter().map(|r| r.1).sum();
                assert!((sum - 1.0).abs() < 1e-9, "{}: shares sum {sum}", d.key);
                for (r, share, f) in &d.roles {
                    if matches!(r, Role::Interdict | Role::Scout) {
                        seen.push((*r, *share, f.hull));
                    }
                }
            }
            seen
        };
        // Nothing supplies the hulls: the line stays as it was, the shares
        // go to the damage dealers.
        let none = roles(&[DICTOR, HIC, COVERT]);
        assert!(none.is_empty(), "{none:?}");
        // A dictor alone gives the interdict role, the eye still no.
        let dictor = roles(&[HIC, COVERT]);
        assert!(!dictor.is_empty());
        assert!(
            dictor
                .iter()
                .all(|r| r.0 == Role::Interdict && r.2 == DICTOR && r.1 == SHARE_INTERDICT)
        );
        // All three: both roles.
        let all = roles(&[]);
        assert!(all.iter().any(|r| r.0 == Role::Scout && r.1 == SHARE_SCOUT));
        assert!(all.iter().any(|r| r.0 == Role::Interdict));
    }

    #[test]
    fn no_doctrine_dominates_with_the_t2_roles() {
        let inst = t2_world();
        let lib = t2_lib(&inst, &[], Limits::default());
        let cat = catalog_in(&lib);
        let on = Features::default();
        assert!(dominant(&cat, on).is_empty());
    }

    /// A dictor found only as a data-site copy: no original to buy, so no
    /// rung ([`crate::doctrine_parts::doctrine_rungs`]), and the builder's lab
    /// builds the copy once held ([`crate::builder::t2_candidates`]).
    #[test]
    fn a_dictor_bpc_makes_no_rung_and_is_built_once_held() {
        use crate::builder::t2_candidates;
        use crate::doctrine_parts::{Source, Sources, doctrine_rungs, makeable, wanted_parts};
        use eve_server::industry;
        const HERE: i64 = 60_000_001;
        let inst = t2_world();
        let bp = |p: i64| p + 1_000;
        let recipes = industry::Table {
            blueprints: vec![industry::Blueprint {
                blueprint_type_id: bp(DICTOR),
                product_type_id: DICTOR,
                product_quantity: 1,
                time: 600,
                max_production_limit: 10,
                materials: vec![(970_006_900, 5)],
                skills: vec![],
            }],
            ..industry::Table::default()
        };
        let sold: BTreeSet<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            // Only the dictor is wanted: the heavy interdictor is nobody's.
            .filter(|t| *t != DICTOR && *t != HIC)
            .collect();
        let dropped = BTreeSet::new();
        let found = BTreeSet::from([bp(DICTOR)]);
        let src = Sources {
            recipes: &recipes,
            sold: Some(&sold),
            dropped: Some(&dropped),
            copies: Some(&found),
            sourcing: None,
        };
        assert_eq!(src.source_of(DICTOR), Source::Copy(bp(DICTOR)));
        let made = makeable(&src);
        assert!(made.has(DICTOR) && made.has(SPHERE));
        let lib = Library::build(&inst, &made, &|t| 100.0 + (t % 7) as f64, Limits::default());
        let doc = catalog_in(&lib)
            .into_iter()
            .find(|d| d.roles.iter().any(|r| r.0 == Role::Interdict))
            .expect("a doctrine with a dictor");
        let wanted = wanted_parts(&doc);
        assert!(wanted.get(&DICTOR).is_some_and(|n| *n >= 1));
        assert!(wanted.contains_key(&PROBE_CHEAP) || wanted.contains_key(&PROBE_DEAR));
        // No rung for the copy-only hull.
        let rungs = doctrine_rungs(&recipes, &wanted, &src, &BTreeSet::new());
        assert!(
            rungs.iter().flatten().all(|b| *b != bp(DICTOR)),
            "{rungs:?}"
        );
        // A held copy is built because the doctrine wants its product.
        let copy = crate::verbs::BlueprintRow {
            item_id: 7,
            type_id: bp(DICTOR),
            location_id: HERE,
            flag_id: industry::FLAG_HANGAR,
            runs: 3,
            material_efficiency: 0,
            job_id: None,
        };
        let want: BTreeSet<i64> = wanted.keys().copied().collect();
        let held = [copy];
        let built = t2_candidates(&held, HERE, &recipes, &want);
        assert_eq!(built.len(), 1);
        let nothing = t2_candidates(&held, HERE, &recipes, &BTreeSet::new());
        assert!(nothing.is_empty(), "an unwanted copy is left alone");
    }

    // ---- PART-KINDS: part kinds, rigs, drones and missile families --------
    //
    // Invented rows (ids above 970,000,000; every number round). The shapes
    // of the modifiers are the engine's own.

    const DC: i64 = 970_006_001;
    const NANO: i64 = 970_006_002;
    const COPROC: i64 = 970_006_003;
    const REACTOR: i64 = 970_006_004;
    const BATTERY: i64 = 970_006_005;
    const TRACK_ENH: i64 = 970_006_006;
    const TRACK_COMP: i64 = 970_006_007;
    const GUIDE_ENH: i64 = 970_006_008;
    const GUIDE_COMP: i64 = 970_006_009;
    const BOOSTER: i64 = 970_006_010;
    const DDA: i64 = 970_006_011;
    const RIG_ARMOUR: i64 = 970_006_020;
    const RIG_SHIELD: i64 = 970_006_021;
    const RIG_SPEED: i64 = 970_006_022;
    const RIG_WEAPON: i64 = 970_006_023;
    const DRONE_SMALL: i64 = 970_006_030;
    const DRONE_BIG: i64 = 970_006_031;
    const DRONE_MINER: i64 = 970_006_032;
    const HAM_LAUNCHER: i64 = 970_006_040;
    const TORP_LAUNCHER: i64 = 970_006_041;
    const RAPID_LAUNCHER: i64 = 970_006_042;
    const ROCKET_LAUNCHER: i64 = 970_006_043;
    const T2_TORP_LAUNCHER: i64 = 970_006_044;
    const ROCKET_ROUND: i64 = 970_006_052;
    const LIGHT_ROUND: i64 = 970_006_053;
    const MISSILE_HULL: i64 = 970_006_100;
    const DRONE_HULL: i64 = 970_006_101;
    const TORP_HULL: i64 = 970_006_102;

    fn kinds_world() -> Installed {
        let mut text = world_text();
        let mut line = |s: String| {
            text.push_str(&s);
            text.push('\n');
        };
        // Passive effects: id -> [(domain, func, modified, modifying, op, skill)]
        let ship = "shipID\tItemModifier";
        let skill = "shipID\tLocationRequiredSkillModifier";
        let owner = "charID\tOwnerRequiredSkillModifier";
        type Mod = (String, i64, i64, String);
        let mut fx: Vec<(i64, &str, Vec<Mod>)> = Vec::new();
        let m = |d: &str, a: i64, b: i64, sk: &str| (d.to_owned(), a, b, sk.to_owned());
        // a damage control: all three layers (hull, armour, shield)
        let mut dc = Vec::new();
        for (i, a) in [109, 110, 111, 113].into_iter().enumerate() {
            dc.push(m(ship, a, 974 + i as i64, ""));
        }
        for (i, a) in [267, 268, 269, 270].into_iter().enumerate() {
            dc.push(m(ship, a, 9_601 + i as i64, ""));
        }
        for (i, a) in [271, 272, 273, 274].into_iter().enumerate() {
            dc.push(m(ship, a, 9_611 + i as i64, ""));
        }
        fx.push((9401, "damageControl", dc));
        fx.push((
            9402,
            "nanofiber",
            vec![m(ship, 37, 9_701, ""), m(ship, 70, 9_702, "")],
        ));
        fx.push((9403, "coProcessor", vec![m(ship, 48, 9_703, "")]));
        fx.push((9404, "reactorControl", vec![m(ship, 11, 9_704, "")]));
        fx.push((9405, "capBattery", vec![m(ship, 482, 9_705, "")]));
        fx.push((9406, "trackingEnh", vec![m(skill, 160, 9_706, "970000001")]));
        fx.push((9407, "guidanceEnh", vec![m(skill, 653, 9_707, "970000001")]));
        fx.push((9409, "droneAmp", vec![m(owner, 64, 9_709, "3436")]));
        fx.push((9410, "armourRig", vec![m(ship, 265, 9_710, "")]));
        fx.push((9411, "shieldRig", vec![m(ship, 263, 9_710, "")]));
        fx.push((
            9412,
            "speedRig",
            vec![m(ship, 37, 9_701, ""), m(ship, 265, 9_712, "")],
        ));
        fx.push((9413, "weaponRig", vec![m(skill, 64, 9_713, "970000001")]));
        line("effect\t2663\t0\t0\t0\t0\t0\t0\t0\trigSlot\t".to_owned());
        for (id, name, mods) in &fx {
            line(format!("effect\t{id}\t4\t0\t0\t0\t0\t0\t0\t{name}\t"));
            for (domain, a, b, sk) in mods {
                line(format!("modifier\t{id}\t{domain}\t{a}\t{b}\t6\t{sk}\t"));
            }
        }
        // modules: (id, rack effect, name, extra effects, attrs)
        let mut module = |id: i64, rack: i64, name: &str, fxs: &str, attrs: &str| {
            line(format!(
                "type\t{id}\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t{rack},16{fxs}\t\
                 30=1.0;50=10.0;{attrs};633=0.0\t{name}"
            ));
        };
        let dc_attrs = "974=-30.0;975=-30.0;976=-30.0;977=-30.0;9601=-20.0;9602=-20.0;\
                        9603=-20.0;9604=-20.0;9611=-20.0;9612=-20.0;9613=-20.0;9614=-20.0";
        module(DC, 11, "Damage control", ",9401", dc_attrs);
        module(NANO, 11, "Nanofiber", ",9402", "9701=10.0;9702=-15.0");
        module(COPROC, 11, "Co-processor", ",9403", "9703=25.0");
        module(REACTOR, 11, "Reactor control", ",9404", "9704=25.0");
        module(BATTERY, 13, "Cap battery", ",9405", "9705=30.0");
        module(TRACK_ENH, 11, "Tracking enhancer", ",9406", "9706=10.0");
        module(TRACK_COMP, 13, "Tracking computer", ",9406", "9706=15.0");
        module(GUIDE_ENH, 11, "Guidance enhancer", ",9407", "9707=10.0");
        module(GUIDE_COMP, 13, "Guidance computer", ",9407", "9707=15.0");
        module(BOOSTER, 13, "Cap booster", "", "604=87.0");
        module(DDA, 11, "Drone amplifier", ",9409", "9709=20.0");
        // rigs: the rig slot effect is their default; calibration cost 100
        let mut rig = |id: i64, name: &str, fxs: &str, attrs: &str| {
            line(format!(
                "type\t{id}\t773\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663{fxs}\t\
                 1153=100.0;{attrs};633=0.0\t{name}"
            ));
        };
        rig(RIG_ARMOUR, "Armour rig", ",9410", "9710=15.0");
        rig(RIG_SHIELD, "Shield rig", ",9411", "9710=15.0");
        rig(RIG_SPEED, "Speed rig", ",9412", "9701=20.0;9712=-10.0");
        rig(RIG_WEAPON, "Weapon rig", ",9413", "9713=10.0");
        // drones: group 100, category 18; bandwidth need 1272, volume 5 / 25
        for (id, name, vol, bw, dmg, rate) in [
            (DRONE_SMALL, "Small drone", 5.0, 5.0, 20.0, 4000.0),
            (DRONE_BIG, "Big drone", 25.0, 25.0, 90.0, 5000.0),
        ] {
            line(format!(
                "type\t{id}\t100\t18\t1\t1\t{vol}\t0.0\t3500.0\t15.0\t0\t\t\
                 51={rate};54=2400.0;64=1.5;117={dmg};158=2000.0;160=2.0;182=3436.0;620=25.0;\
                 1272={bw};633=0.0\t{name}"
            ));
        }
        // a mining drone does no damage: no combat part
        line(format!(
            "type\t{DRONE_MINER}\t101\t18\t1\t1\t5.0\t0.0\t3500.0\t15.0\t0\t\t\
             51=4000.0;182=3436.0;1272=5.0;633=0.0\tMining drone"
        ));
        // launchers: the heavy assault one takes group 772, the torpedo 89,
        // the rapid light launcher the light group 384 at a fast cycle
        for (id, name, group, rate, cpu, pg) in [
            (HAM_LAUNCHER, "HAM launcher", 772, 4000.0, 20.0, 12.0),
            (TORP_LAUNCHER, "Torpedo launcher", 89, 9000.0, 40.0, 40.0),
            (RAPID_LAUNCHER, "Rapid launcher", 384, 1500.0, 30.0, 10.0),
        ] {
            line(format!(
                "type\t{id}\t507\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,40,101\t\
                 6=0.0;30={pg};50={cpu};51={rate};182={GUNNERY};277=1.0;604={group};633=0.0\t{name}"
            ));
        }
        // the rocket launcher names group 387 (not one of the five); the T2
        // torpedo launcher lists another group first, then the torpedo group
        for (id, name, groups) in [
            (ROCKET_LAUNCHER, "Rocket launcher", "604=387.0"),
            (
                T2_TORP_LAUNCHER,
                "T2 torpedo launcher",
                "604=657.0;606=89.0",
            ),
        ] {
            line(format!(
                "type\t{id}\t507\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,40,101\t\
                 6=0.0;30=10.0;50=20.0;51=3000.0;182={GUNNERY};277=1.0;{groups};633=0.0\t{name}"
            ));
        }
        line(format!(
            "type\t{ROCKET_ROUND}\t387\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t0\t\t117=100.0;37=3000.0;\
             281=6000.0;653=100.0;654=200.0;1353=0.5;633=0.0\tRocket"
        ));
        line(format!(
            "type\t{LIGHT_ROUND}\t384\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t0\t\t117=50.0;37=3000.0;\
             281=6000.0;653=100.0;654=200.0;1353=0.5;633=0.0\tLight round"
        ));
        for (i, (group, dmg)) in [(772, "117=300.0"), (89, "117=900.0")]
            .into_iter()
            .enumerate()
        {
            line(format!(
                "type\t{}\t{group}\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t0\t\t{dmg};37=3000.0;\
                 281=6000.0;653=100.0;654=200.0;1353=0.5;633=0.0\tAssault round {i}",
                970_006_050 + i as i64
            ));
        }
        // hulls: a missile cruiser, a drone cruiser (a bay, three turrets), a
        // missile battleship
        for (id, group, name, hi, med, low, turrets, launchers, extra) in [
            (MISSILE_HULL, 26, "Hotel", 5, 4, 5, 0, 5, ""),
            (
                DRONE_HULL,
                26,
                "India",
                4,
                4,
                5,
                3,
                0,
                "283=75.0;1271=75.0;",
            ),
            (TORP_HULL, 27, "Juliet", 8, 6, 7, 0, 5, ""),
        ] {
            let big = if group == 27 { 100.0 } else { 1.0 };
            line(format!(
                "type\t{id}\t{group}\t6\t1\t1\t1000.0\t100.0\t1.2e7\t30.0\t0\t\t\
                 9=1500.0;11={};12={low};13={med};14={hi};37=240.0;48={};55=150000.0;70=3.0;\
                 76=60000.0;101={launchers};102={turrets};182={HULL_SKILL};192=5.0;208=14.0;\
                 209=0.0;210=0.0;211=0.0;263=1500.0;265=1800.0;277=1.0;479=600000.0;482=500.0;\
                 552=125.0;564=500.0;1137=3.0;{extra}\t{name}",
                600.0 * big,
                400.0 * big
            ));
        }
        Installed::build(eve_server::typedata::Table::parse(&text).expect("parses"))
    }

    fn lib_with<'a>(inst: &'a Installed, limits: Limits) -> Library<'a> {
        Library::build(inst, &Makeable::all(), &|t| 100.0 + (t % 7) as f64, limits)
    }

    /// The same library less one part (`Makeable` without it).
    fn lib_without<'a>(inst: &'a Installed, ty: &[i64], limits: Limits) -> Library<'a> {
        let others: Vec<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            .filter(|t| !ty.contains(t))
            .collect();
        Library::build(
            inst,
            &Makeable::of(others),
            &|t| 100.0 + (t % 7) as f64,
            limits,
        )
    }

    fn kfit(lib: &Library<'_>, hull: i64, family: Family, layer: Layer, tmpl: Stance) -> RoleFit {
        fill_slots(lib, hull_of(lib, hull), family, layer, tmpl, Role::Dps).expect("a fit")
    }

    #[test]
    fn the_new_part_kinds_are_read_from_what_they_modify() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let k = |t| lib.kind_of(t);
        assert_eq!(k(DC), Some(Kind::DamageControl));
        assert_eq!(k(NANO), Some(Kind::Speed));
        assert_eq!(k(COPROC), Some(Kind::PowerMod));
        assert_eq!(k(REACTOR), Some(Kind::PowerMod));
        assert_eq!(k(BATTERY), Some(Kind::CapBattery));
        assert_eq!(k(TRACK_ENH), Some(Kind::Tracking));
        assert_eq!(k(TRACK_COMP), Some(Kind::Tracking));
        assert_eq!(k(GUIDE_ENH), Some(Kind::Guidance));
        assert_eq!(k(GUIDE_COMP), Some(Kind::Guidance));
        assert_eq!(k(BOOSTER), Some(Kind::CapBooster));
        assert_eq!(k(DDA), Some(Kind::DroneMod));
        // The enhancer is a low part, the computer a mid one.
        let rack = |t| lib.parts.iter().find(|p| p.type_id == t).map(|p| p.rack);
        assert_eq!(rack(TRACK_ENH), Some(Rack::Low));
        assert_eq!(rack(TRACK_COMP), Some(Rack::Medium));
        // The gyro of the plain world is still a damage mod, a hardener a hardener.
        assert!(
            lib.parts
                .iter()
                .any(|p| matches!(p.kind, Kind::DamageMod(_)))
        );
        assert!(
            lib.parts
                .iter()
                .any(|p| p.kind == Kind::Hardener(Layer::Armour))
        );
    }

    #[test]
    fn rigs_are_classified_by_their_benefit_and_gated_by_the_limit() {
        let inst = kinds_world();
        // Off by default: the server fits none.
        let off = lib_with(&inst, Limits::default());
        assert!(off.parts.iter().all(|p| !p.rig));
        let lib = lib_with(
            &inst,
            Limits {
                rigs: true,
                ..Limits::default()
            },
        );
        assert_eq!(lib.kind_of(RIG_ARMOUR), Some(Kind::Rig(RigKind::Armour)));
        assert_eq!(lib.kind_of(RIG_SHIELD), Some(Kind::Rig(RigKind::Shield)));
        // The speed rig's drawback on the armour is smaller than its benefit.
        assert_eq!(lib.kind_of(RIG_SPEED), Some(Kind::Rig(RigKind::Speed)));
        assert_eq!(lib.kind_of(RIG_WEAPON), Some(Kind::Rig(RigKind::Weapon)));
        // A rig is never offered for a slot.
        let hull = hull_of(&lib, CRUISER);
        for rack in [Rack::High, Rack::Medium, Rack::Low] {
            assert!(
                lib.best(hull, |k| matches!(k, Kind::Rig(_)), rack, (1e9, 1e9))
                    .is_none()
            );
        }
    }

    #[test]
    fn a_fit_fills_the_rig_slots_within_the_calibration() {
        let inst = kinds_world();
        let lib = lib_with(
            &inst,
            Limits {
                rigs: true,
                ..Limits::default()
            },
        );
        let rigs = |f: &RoleFit| -> Vec<Kind> {
            f.modules
                .iter()
                .filter_map(|m| lib.kind_of(m.0))
                .filter(|k| matches!(k, Kind::Rig(_)))
                .collect()
        };
        let brawl = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert_eq!(rigs(&brawl), vec![Kind::Rig(RigKind::Armour)]);
        let n: u8 = brawl
            .modules
            .iter()
            .filter(|m| matches!(lib.kind_of(m.0), Some(Kind::Rig(_))))
            .map(|m| m.1)
            .sum();
        // Three rig slots, calibration 400, 100 each: three rigs.
        assert_eq!(n, 3);
        // A kiter swaps one for a weapon rig and one for a speed rig.
        let kite = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Shield,
            Stance::Kite,
        );
        let ks = rigs(&kite);
        assert!(ks.contains(&Kind::Rig(RigKind::Weapon)), "{ks:?}");
        assert!(ks.contains(&Kind::Rig(RigKind::Speed)), "{ks:?}");
        assert!(ks.contains(&Kind::Rig(RigKind::Shield)), "{ks:?}");
        // And with the limit off no rig is fitted.
        let none = lib_with(&inst, Limits::default());
        let f = kfit(
            &none,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(
            f.modules
                .iter()
                .all(|m| !none.parts.iter().any(|p| p.type_id == m.0 && p.rig))
        );
    }

    #[test]
    fn a_damage_control_is_fitted_and_its_resists_are_counted() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let with = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(with.modules.iter().any(|m| m.0 == DC), "{:?}", with.modules);
        let bare = lib_without(&inst, &[DC], Limits::default());
        let without = kfit(
            &bare,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(with.profile.ehp_mean() > without.profile.ehp_mean());
    }

    #[test]
    fn a_kiter_takes_speed_and_tracking_and_a_brawler_does_not() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let kite = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
        );
        let has = |f: &RoleFit, t: i64| f.modules.iter().any(|m| m.0 == t);
        assert!(has(&kite, NANO), "{:?}", kite.modules);
        // Four lows are tank and speed's; the tracking part is the mid computer.
        assert!(
            has(&kite, TRACK_COMP) || has(&kite, TRACK_ENH),
            "{:?}",
            kite.modules
        );
        let bare = lib_without(&inst, &[NANO], Limits::default());
        let slow = kfit(
            &bare,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
        );
        assert!(
            kite.profile.speed > slow.profile.speed,
            "the engine counts the nanofiber"
        );
        let brawl = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(!has(&brawl, NANO) && !has(&brawl, TRACK_ENH) && !has(&brawl, TRACK_COMP));
        // A missile ship takes the guidance part, not the tracking one.
        let m = kfit(
            &lib,
            MISSILE_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Skirmish,
        );
        assert!(has(&m, GUIDE_ENH) || has(&m, GUIDE_COMP), "{:?}", m.modules);
        assert!(!has(&m, TRACK_ENH) && !has(&m, TRACK_COMP));
    }

    #[test]
    fn a_brawler_carries_a_capacitor_battery_and_a_power_mod_makes_room() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let brawl = kfit(
            &lib,
            CRUISER,
            Family::Projectile,
            Layer::Shield,
            Stance::Brawl,
        );
        assert!(
            brawl.modules.iter().any(|m| m.0 == BATTERY),
            "{:?}",
            brawl.modules
        );
        let wide = kfit(
            &lib,
            TORP_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Skirmish,
        );
        assert!(
            wide.modules.iter().any(|m| m.0 == COPROC || m.0 == REACTOR),
            "{:?}",
            wide.modules
        );
        assert!(wide.profile.ok());
    }

    #[test]
    fn a_tank_keeps_its_repairer_and_hardener_when_the_new_parts_take_slots() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let f = kfit(
            &lib,
            MISSILE_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Skirmish,
        );
        let kinds: Vec<Kind> = f
            .modules
            .iter()
            .filter_map(|m| lib.kind_of(m.0).map(|k| (k, m.1)))
            .flat_map(|(k, n)| std::iter::repeat_n(k, usize::from(n)))
            .collect();
        let rack_of = |t: i64| lib.parts.iter().find(|p| p.type_id == t).map(|p| p.rack);
        let lows: usize = f
            .modules
            .iter()
            .filter(|m| rack_of(m.0) == Some(Rack::Low))
            .map(|m| usize::from(m.1))
            .sum();
        assert!(lows <= 5, "no more lows than the hull has: {kinds:?}");
        assert!(kinds.contains(&Kind::ArmourRepairer));
        assert!(kinds.contains(&Kind::Hardener(Layer::Armour)));
    }

    #[test]
    fn heavy_assault_and_torpedo_launchers_are_guns_of_their_size() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        assert_eq!(
            lib.kind_of(HAM_LAUNCHER),
            Some(Kind::Gun(Family::Missile, 2))
        );
        assert_eq!(
            lib.kind_of(TORP_LAUNCHER),
            Some(Kind::Gun(Family::Missile, 3))
        );
        assert_eq!(
            fit_plan::weapon_of_group(fit_plan::GROUP_HEAVY_ASSAULT_MISSILE),
            Some(crate::hull_choice::Weapon::Missile)
        );
        assert_eq!(
            fit_plan::weapon_of_group(fit_plan::GROUP_TORPEDO),
            Some(crate::hull_choice::Weapon::Missile)
        );
        // A cruiser takes the HAM launcher (its round does more per second),
        // a battleship the torpedo launcher, and both count their damage.
        let cruiser = kfit(
            &lib,
            MISSILE_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(
            cruiser.modules.iter().any(|m| m.0 == HAM_LAUNCHER),
            "{:?}",
            cruiser.modules
        );
        assert!(cruiser.profile.total_dps() > 0.0);
        let ship = kfit(
            &lib,
            TORP_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(
            ship.modules.iter().any(|m| m.0 == TORP_LAUNCHER),
            "{:?}",
            ship.modules
        );
        assert!(matches!(ship.profile.delivery, Delivery::Missile(_)));
    }

    #[test]
    fn a_rapid_light_launcher_is_no_longer_barred_from_a_cruiser() {
        let inst = kinds_world();
        // Without the HAM launcher a cruiser's best launcher is the rapid
        // light one (its round per second beats the heavy launcher's).
        let lib = lib_without(&inst, &[HAM_LAUNCHER], Limits::default());
        assert_eq!(
            lib.kind_of(RAPID_LAUNCHER),
            Some(Kind::Gun(Family::Missile, 1))
        );
        let f = kfit(
            &lib,
            MISSILE_HULL,
            Family::Missile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(
            f.modules.iter().any(|m| m.0 == RAPID_LAUNCHER),
            "{:?}",
            f.modules
        );
        // A frigate's size-1 launcher want is unchanged; a battleship does not
        // drop two sizes.
        assert!(serves(
            Kind::Gun(Family::Missile, 2),
            Kind::Gun(Family::Missile, 3)
        ));
        assert!(!serves(
            Kind::Gun(Family::Missile, 1),
            Kind::Gun(Family::Missile, 3)
        ));
        assert!(!serves(
            Kind::Gun(Family::Hybrid, 1),
            Kind::Gun(Family::Hybrid, 2)
        ));
    }

    #[test]
    fn a_drone_boat_flies_drones_and_their_damage_is_counted() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        assert_eq!(lib.kind_of(DRONE_SMALL), None.or(Some(Kind::Drone)));
        // A mining drone does no damage: not a combat part.
        assert!(lib.parts.iter().all(|p| p.type_id != DRONE_MINER));
        let (drone, n) = drones_for(&lib, hull_of(&lib, DRONE_HULL)).expect("a drone");
        // Bandwidth 75: five small ones (25 Mbit/s) or three big ones (75)
        // -- the three big deal 3 * 90 / 5 s against 5 * 20 / 4 s.
        assert_eq!((drone.type_id, n), (DRONE_BIG, 3));
        assert!(drones_for(&lib, hull_of(&lib, CRUISER)).is_none(), "no bay");
        let f = kfit(
            &lib,
            DRONE_HULL,
            Family::Drone,
            Layer::Armour,
            Stance::Skirmish,
        );
        assert!(f.charges.contains(&(DRONE_BIG, 3)), "{:?}", f.charges);
        assert!(
            f.modules
                .iter()
                .all(|m| !matches!(lib.kind_of(m.0), Some(Kind::Gun(..))))
        );
        let dps = f.profile.total_dps();
        assert!(dps > 3.0 * 90.0 / 5.0, "x1.5 multiplier: {dps}");
        // profile_of alone counts none; with the drones it does.
        let bare = profile_of(&lib, DRONE_HULL, &[], None, &[]).unwrap();
        assert_eq!(bare.total_dps(), 0.0);
        let flown =
            profile_with_drones(&lib, DRONE_HULL, &[], None, &[], &[(DRONE_BIG, 3)]).unwrap();
        assert!(
            (flown.total_dps() - 3.0 * 90.0 * 1.5 / 5.0).abs() < 1e-6,
            "{}",
            flown.total_dps()
        );
        assert!(matches!(flown.delivery, Delivery::Turret(_)));
        // The amplifier counts through the engine, as the drones' charge modifier.
        assert!(f.modules.iter().any(|m| m.0 == DDA), "{:?}", f.modules);
        let bare_lib = lib_without(&inst, &[DDA], Limits::default());
        let plain = kfit(
            &bare_lib,
            DRONE_HULL,
            Family::Drone,
            Layer::Armour,
            Stance::Skirmish,
        );
        assert!(dps > plain.profile.total_dps());
        // A hull with no bay gets no drone fit.
        assert!(
            fill_slots(
                &lib,
                hull_of(&lib, CRUISER),
                Family::Drone,
                Layer::Armour,
                Stance::Brawl,
                Role::Dps
            )
            .is_none()
        );
    }

    #[test]
    fn a_gun_ship_with_a_bay_adds_its_drones_and_the_catalog_has_a_drone_line() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        let f = kfit(
            &lib,
            DRONE_HULL,
            Family::Projectile,
            Layer::Armour,
            Stance::Brawl,
        );
        assert!(f.charges.iter().any(|c| c.0 == DRONE_BIG));
        assert!(f.charges[0].0 != DRONE_BIG, "the damage round stays first");
        let cat = catalog_in(&lib);
        assert!(
            cat.iter().any(|d| d.family == Family::Drone),
            "{:?}",
            cat.iter().map(|d| &d.key).collect::<Vec<_>>()
        );
    }

    // ---- fit budget and damage-mod aim (invented rows) ---------------------

    const CPU_SKILL: i64 = 970_007_001;
    const VORTON_SKILL: i64 = 970_007_002;
    const ROOMY: i64 = 970_007_100;
    const ROOMY_BIG: i64 = 970_007_101;
    const VORTON_MOD: i64 = 970_007_200;

    /// The plain world plus a CPU skill (+10% cpuOutput a level), a hull
    /// whose raw CPU holds two small guns in the gun budget and a vorton-only
    /// damage module that beats the plain ones on every rank.
    fn budget_world() -> Installed {
        let mut text = world_text();
        let mut line = |s: String| {
            text.push_str(&s);
            text.push('\n');
        };
        line("attribute\t424\t0\tcpuOutputBonus".to_owned());
        line("effect\t9401\t0\t0\t0\t0\t0\t0\t0\tcpuManagement\t".to_owned());
        line("effect\t9410\t4\t0\t0\t0\t0\t0\t0\tvortonMod\t".to_owned());
        line("modifier\t9401\titemID\tItemModifier\t424\t280\t0\t\t".to_owned());
        line("modifier\t9401\tshipID\tItemModifier\t48\t424\t6\t\t".to_owned());
        line(format!(
            "modifier\t9410\tshipID\tLocationRequiredSkillModifier\t64\t64\t4\t{VORTON_SKILL}\t"
        ));
        line(format!(
            "type\t{CPU_SKILL}\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9401\t424=10.0\tCpu"
        ));
        // 3 highs, 3 turrets, 3 lows; 18 CPU, huge grid. A small gun needs 5.
        // The big twin has CPU to spare for the lows.
        for (id, cpu) in [(ROOMY, 18.0), (ROOMY_BIG, 180.0)] {
            line(format!(
                "type\t{id}\t25\t6\t1\t1\t1000.0\t100.0\t1.2e6\t30.0\t0\t\
             \t9=400.0;11=100000.0;12=3.0;13=3.0;14=3.0;37=400.0;48={cpu};55=150000.0;70=3.0;\
             76=40000.0;101=0.0;102=3.0;182={HULL_SKILL};192=5.0;208=14.0;209=0.0;\
             210=0.0;211=0.0;263=300.0;265=250.0;277=1.0;479=600000.0;482=500.0;\
             552=35.0;564=500.0;1137=3.0\tRoomy"
            ));
        }
        line(format!(
            "type\t{VORTON_MOD}\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t11,16,9410\t\
             30=1.0;50=1.0;64=9.0;633=0.0\tVorton stabiliser"
        ));
        Installed::build(eve_server::typedata::Table::parse(&text).expect("parses"))
    }

    fn guns_in(lib: &Library<'_>, f: &RoleFit) -> usize {
        f.modules
            .iter()
            .filter(|m| matches!(lib.kind_of(m.0), Some(Kind::Gun(..))))
            .map(|m| usize::from(m.1))
            .sum()
    }

    #[test]
    fn the_budget_is_the_skilled_cpu_not_the_raw_one() {
        let inst = budget_world();
        let lib = lib_of(&inst);
        let hull = hull_of(&lib, ROOMY);
        let gun = lib
            .parts
            .iter()
            .find(|p| p.type_id == SMALL_PROJECTILE)
            .unwrap();
        // Raw: three guns do not fit the gun share, two do.
        assert!(hull.cpu * GUN_BUDGET / 3.0 < gun.cpu);
        assert!(hull.cpu * GUN_BUDGET / 2.0 >= gun.cpu);
        let skills = skills_for(inst.table(), &[ROOMY, SMALL_PROJECTILE], DOCTRINE_LEVEL);
        assert!(skills.iter().any(|s| s.0 == CPU_SKILL), "{skills:?}");
        let (out, _) = budget_of(&lib, hull);
        assert!(out.0 > hull.cpu, "{out:?}");
        let f = fill_slots(
            &lib,
            hull,
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
            Role::Dps,
        )
        .expect("a fit");
        assert_eq!(guns_in(&lib, &f), 3);
    }

    #[test]
    fn a_damage_module_must_move_the_gun_it_sits_beside() {
        let inst = budget_world();
        let lib = lib_of(&inst);
        assert_eq!(
            lib.kind_of(VORTON_MOD),
            Some(Kind::DamageMod(Aim::Skill(VORTON_SKILL)))
        );
        let f = fill_slots(
            &lib,
            hull_of(&lib, ROOMY_BIG),
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
            Role::Dps,
        )
        .expect("a fit");
        let mods: Vec<Kind> = f
            .modules
            .iter()
            .filter_map(|m| lib.kind_of(m.0))
            .filter(|k| matches!(k, Kind::DamageMod(_)))
            .collect();
        assert!(!mods.is_empty(), "the gun's own module is fitted");
        assert!(
            f.modules.iter().all(|m| m.0 != VORTON_MOD),
            "a vorton-only module is never fitted beside a projectile gun"
        );
        assert!(mods.contains(&Kind::DamageMod(Aim::Skill(GUNNERY))));
    }

    // ---- real fits seed the catalog (items 4 and 5) -------------------------

    /// One invented killmail-fits row: every module in the high column.
    fn real_row(
        hull: i64,
        hash: &str,
        count: u32,
        mods: &[(i64, u8)],
        charge: (i64, i64),
    ) -> String {
        let hi: Vec<String> = mods
            .iter()
            .flat_map(|(t, n)| std::iter::repeat_n(t.to_string(), usize::from(*n)))
            .collect();
        format!(
            "{hull}\t{hash}\t{count}\t0.5\t{}\t\t\t\t\t{}:{}\t\tInvented\n",
            hi.join(","),
            charge.0,
            charge.1
        )
    }

    /// The greedy frigate fit with its plates swapped for the named plate
    /// (above the meta cap, so the greedy fit never holds it).
    fn named_plate_fit(lib: &Library<'_>) -> (RoleFit, Vec<(i64, u8)>) {
        let g = fill_slots(
            lib,
            hull_of(lib, FRIGATE),
            Family::Projectile,
            Layer::Armour,
            Stance::Skirmish,
            Role::Dps,
        )
        .expect("greedy fit");
        let mods = g
            .modules
            .iter()
            .map(|m| {
                if lib.kind_of(m.0) == Some(Kind::Plate) {
                    (NAMED_PLATE, m.1)
                } else {
                    *m
                }
            })
            .collect();
        (g, mods)
    }

    fn doctrine_with_modules<'d>(cat: &'d [Doctrine], mods: &[(i64, u8)]) -> Option<&'d Doctrine> {
        let sorted = |m: &[(i64, u8)]| {
            let mut v = m.to_vec();
            v.sort();
            v
        };
        let want = sorted(mods);
        cat.iter().find(|d| {
            d.roles
                .iter()
                .any(|r| r.0 == Role::Dps && sorted(&r.2.modules) == want)
        })
    }

    #[test]
    fn a_passing_real_row_beats_a_weaker_greedy_fit() {
        let inst = world();
        let mut lib = lib_of(&inst);
        let (g, mods) = named_plate_fit(&lib);
        assert!(mods != g.modules, "the greedy fit has a plate to swap");
        let gun = g
            .modules
            .iter()
            .map(|m| m.0)
            .find(|t| matches!(lib.kind_of(*t), Some(Kind::Gun(..))))
            .unwrap();
        let text = real_row(FRIGATE, "r1", 10, &mods, (gun, g.charges[0].0));
        let table = crate::real_fits::parse(&text).unwrap();
        // Without the table no doctrine holds the named plate.
        assert!(doctrine_with_modules(&catalog_in(&lib), &mods).is_none());
        lib.real_fits = Some(&table);
        let seeds = crate::doctrine_real::seeds(&lib);
        assert!(!seeds.is_empty(), "the row passes the engine");
        let cat = catalog_in(&lib);
        let d = doctrine_with_modules(&cat, &mods).expect("the real fit stands in the catalog");
        let dps = d.roles.iter().find(|r| r.0 == Role::Dps).unwrap();
        assert!(dps.2.profile.ok());
        let base = catalog_in(&lib_of(&inst));
        let weaker = base.iter().find(|b| b.key == d.key).expect("same key");
        assert!(
            d.line().map(value) > weaker.line().map(value),
            "the real row is the better by value"
        );
    }

    #[test]
    fn a_row_with_an_unmakeable_part_never_appears() {
        let inst = world();
        let all = lib_of(&inst);
        let (g, mods) = named_plate_fit(&all);
        let gun = g
            .modules
            .iter()
            .map(|m| m.0)
            .find(|t| matches!(all.kind_of(*t), Some(Kind::Gun(..))))
            .unwrap();
        let table =
            crate::real_fits::parse(&real_row(FRIGATE, "r1", 10, &mods, (gun, g.charges[0].0)))
                .unwrap();
        let others: Vec<i64> = inst
            .table()
            .types()
            .map(|t| t.type_id)
            .filter(|t| *t != NAMED_PLATE)
            .collect();
        let mut lib = Library::build(
            &inst,
            &Makeable::of(others),
            &|t| 100.0 + (t % 7) as f64,
            Limits::default(),
        );
        lib.real_fits = Some(&table);
        assert!(crate::doctrine_real::seeds(&lib).is_empty());
        let cat = catalog_in(&lib);
        assert!(doctrine_with_modules(&cat, &mods).is_none());
        assert!(cat.iter().all(|d| {
            d.roles
                .iter()
                .all(|r| r.2.modules.iter().all(|m| m.0 != NAMED_PLATE))
        }));
    }

    #[test]
    fn pick_varies_by_seed_among_a_hulls_passing_rows() {
        let inst = world();
        let mut lib = lib_of(&inst);
        let (g, named) = named_plate_fit(&lib);
        let gun = g
            .modules
            .iter()
            .map(|m| m.0)
            .find(|t| matches!(lib.kind_of(*t), Some(Kind::Gun(..))))
            .unwrap();
        let ch = g.charges[0].0;
        let text = real_row(FRIGATE, "r1", 10, &named, (gun, ch))
            + &real_row(FRIGATE, "r2", 10, &g.modules, (gun, ch));
        let table = crate::real_fits::parse(&text).unwrap();
        lib.real_fits = Some(&table);
        let seeds = crate::doctrine_real::seeds(&lib);
        let (_, layer, _) = seeds.iter().next().unwrap();
        let drawn: BTreeSet<Vec<(i64, u8)>> = [0.0, 0.99]
            .iter()
            .filter_map(|r| seeds.pick(FRIGATE, Family::Projectile, layer, *r))
            .map(|f| f.modules.clone())
            .collect();
        assert_eq!(drawn.len(), 2, "two seeds draw two fits");
        let rolls: BTreeSet<u64> = (1..=8)
            .map(|c| crate::doctrine_assign::roll_of(7, c).to_bits())
            .collect();
        assert!(rolls.len() > 4, "bots draw differently");
        assert!(
            rolls
                .iter()
                .all(|b| (0.0..1.0).contains(&f64::from_bits(*b)))
        );
    }

    // ---- DOC-REAL-LAUNCHERS: missile and drone fits seed too ---------------

    /// One real row of `hull`: `hi` modules, `drones` as `type x count`.
    fn launcher_row(
        hull: i64,
        hash: &str,
        hi: &[i64],
        charge: Option<(i64, i64)>,
        drones: &[(i64, u8)],
    ) -> String {
        let ch = charge.map_or(String::new(), |c| format!("{}:{}", c.0, c.1));
        let dr: Vec<String> = drones.iter().map(|d| format!("{}x{}", d.0, d.1)).collect();
        let hi: Vec<String> = hi.iter().map(|t| t.to_string()).collect();
        format!(
            "{hull}\t{hash}\t10\t0.5\t{}\t\t\t\t\t{ch}\t{}\tInvented\n",
            hi.join(","),
            dr.join(",")
        )
    }

    fn built(
        lib: &Library<'_>,
        text: &str,
    ) -> Result<(Family, RoleFit), crate::doctrine_real::Dropped> {
        let table = crate::real_fits::parse(text).unwrap();
        let hull = *table.by_hull.keys().next().unwrap();
        crate::doctrine_real::try_build(lib, &table.by_hull[&hull][0])
    }

    #[test]
    fn rocket_rapid_and_t2_launcher_fits_build_as_missile_fits() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        // The classifier leaves the rocket and the T2 launcher out; the
        // library knows them as guns for a real fit but keeps them out of
        // the greedy parts.
        for (t, size) in [(ROCKET_LAUNCHER, 1), (T2_TORP_LAUNCHER, 3)] {
            assert_eq!(lib.kind_of(t), Some(Kind::Gun(Family::Missile, size)));
            assert!(lib.parts.iter().all(|p| p.type_id != t), "not a part");
        }
        assert_eq!(
            lib.kind_of(RAPID_LAUNCHER),
            Some(Kind::Gun(Family::Missile, 1))
        );
        assert_eq!(lib.kind_of(DC), Some(Kind::DamageControl));
        for (name, launcher, round) in [
            ("rocket", ROCKET_LAUNCHER, ROCKET_ROUND),
            ("t2 torp", T2_TORP_LAUNCHER, 970_006_051),
            ("rapid", RAPID_LAUNCHER, LIGHT_ROUND),
        ] {
            let row = launcher_row(
                MISSILE_HULL,
                name,
                &[launcher, launcher, launcher],
                Some((launcher, round)),
                &[],
            );
            let (family, fit) = built(&lib, &row).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(family, Family::Missile, "{name}");
            assert_eq!(fit.charges[0].0, round, "{name}: the row's round is first");
            assert_eq!(fit.charges[0].1, ROUNDS_PRIMARY * 3, "{name}");
            assert!(fit.profile.total_dps() > 0.0, "{name}");
        }
        // With no charge in the row the greedy choice names one.
        let row = launcher_row(MISSILE_HULL, "bare", &[ROCKET_LAUNCHER], None, &[]);
        let (_, fit) = built(&lib, &row).unwrap();
        assert_eq!(fit.charges[0].0, ROCKET_ROUND);
    }

    #[test]
    fn a_drone_only_fit_builds_on_the_drone_profile() {
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        // Bandwidth 75 flies three big drones of the five carried; the
        // amplifier counts through the engine.
        let row = launcher_row(
            DRONE_HULL,
            "dr",
            &[DDA],
            None,
            &[(DRONE_SMALL, 2), (DRONE_BIG, 5)],
        );
        let (family, fit) = built(&lib, &row).unwrap();
        assert_eq!(family, Family::Drone);
        assert_eq!(fit.charges, vec![(DRONE_BIG, 3)], "the flight, not the bay");
        assert!(fit.skills.iter().any(|s| s.0 == 3436), "drone skill");
        let bare = launcher_row(DRONE_HULL, "bare", &[REACTOR], None, &[(DRONE_BIG, 3)]);
        let (_, plain) = built(&lib, &bare).unwrap();
        assert!(fit.profile.total_dps() > plain.profile.total_dps());
        assert!(fit.profile.total_dps() > 3.0 * 90.0 / 5.0);
        // A gun fit that also carries drones keeps the gun's family and flies them.
        let gun = lib
            .parts
            .iter()
            .find(|p| matches!(p.kind, Kind::Gun(Family::Missile, _)))
            .unwrap()
            .type_id;
        let both = launcher_row(
            DRONE_HULL,
            "both",
            &[gun],
            Some((gun, 970_006_050)),
            &[(DRONE_BIG, 3)],
        );
        // (the drone hull has no launcher hardpoints: it still builds, the engine does not count them)
        let (family, fit) = built(&lib, &both).unwrap();
        assert_eq!(family, Family::Missile);
        assert!(fit.charges.contains(&(DRONE_BIG, 3)), "{:?}", fit.charges);
    }

    #[test]
    fn a_fit_with_no_weapon_is_dropped_with_a_named_reason() {
        use crate::doctrine_real::Dropped;
        let inst = kinds_world();
        let lib = lib_with(&inst, Limits::default());
        // A plate-less utility fit: no gun, no drones.
        let row = launcher_row(MISSILE_HULL, "u", &[DC, NANO], None, &[]);
        assert_eq!(built(&lib, &row).unwrap_err(), Dropped::NoWeapon);
        // Drones on a hull with no bay fly none.
        let row = launcher_row(MISSILE_HULL, "d", &[DC], None, &[(DRONE_BIG, 3)]);
        assert_eq!(built(&lib, &row).unwrap_err(), Dropped::NoWeapon);
        // A mining drone is no combat drone.
        let row = launcher_row(DRONE_HULL, "m", &[DC], None, &[(DRONE_MINER, 5)]);
        assert_eq!(built(&lib, &row).unwrap_err(), Dropped::NoWeapon);
        // A hull the engine does not hold.
        let row = launcher_row(970_999_999, "h", &[ROCKET_LAUNCHER], None, &[]);
        assert_eq!(built(&lib, &row).unwrap_err(), Dropped::NoProfile);
        assert!(!Dropped::Fails.name().is_empty());
    }

    #[test]
    fn missile_and_drone_rows_reach_the_seeds() {
        let inst = kinds_world();
        let mut lib = lib_with(&inst, Limits::default());
        let text = launcher_row(
            MISSILE_HULL,
            "r",
            &[ROCKET_LAUNCHER, ROCKET_LAUNCHER],
            Some((ROCKET_LAUNCHER, ROCKET_ROUND)),
            &[],
        ) + &launcher_row(DRONE_HULL, "d", &[DDA], None, &[(DRONE_BIG, 3)]);
        let table = crate::real_fits::parse(&text).unwrap();
        lib.real_fits = Some(&table);
        let seeds = crate::doctrine_real::seeds(&lib);
        let fams: BTreeSet<Family> = seeds.iter().map(|s| s.0).collect();
        assert_eq!(fams, BTreeSet::from([Family::Missile, Family::Drone]));
    }

    #[test]
    fn a_missing_fits_file_seeds_nothing() {
        assert_eq!(
            crate::real_fits::load(std::path::Path::new("/nonexistent/probe/fits.tsv")),
            Ok(None)
        );
        let inst = world();
        let lib = lib_of(&inst);
        assert!(crate::doctrine_real::seeds(&lib).is_empty());
    }
}
