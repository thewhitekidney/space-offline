//! The dogma engine: skills, ship bonuses and module effects modifying attributes.
//!
//! Owned by the W10 dogma lane (`docs/lanes/dogma-engine.md`).
//!
//! # What it is
//!
//! Given a pilot's skills, a hull, its fitted modules (each offline, online or
//! active) and their loaded charges, compute every attribute the way the
//! client's own Python dogma does -- **the client computes these numbers
//! itself** (its fitting window runs the same engine on the same tables), so
//! the server's job is to agree with it, not to have opinions. Every rule
//! below is read out of the client's bytecode with `tools/dis.py`; the short
//! citations are:
//!
//! | short | file under `out/pyc/` |
//! |---|---|
//! | `const` | `dogma/const.pyc` |
//! | `attribute` | `dogma/attributes/attribute.pyc` |
//! | `nurfed` | `dogma/attributes/stackingNurfedAttribute.pyc` |
//! | `factory` | `dogma/attributes/itemAttributeFactory.pyc` |
//! | `modifiers` | `dogma/effects/modifiers.pyc` |
//! | `bdl` | `eve/common/script/dogma/baseDogmaLocation.pyc` |
//! | `slim` | `dogma/items/slimDogmaItem.pyc` |
//! | `data` | `dogma/data.pyc` |
//! | `prop` | `eve/common/script/dogma/pythonEffects/propulsionModules.pyc` |
//!
//! # The model, in five rules [V]
//!
//! 1. **An item's attribute is its type's value, then every incoming
//!    modifier, applied by operator in a fixed precedence**
//!    (`attribute.Update:344`, `_ApplyModifierOperationsToVal:394`,
//!    `const.dgmOperatorPrecedence:22-31`): pre-assign, pre-mul, pre-div,
//!    add, sub, post-mul, post-div, post-percent, post-percent-unnerfed,
//!    post-assign. Before and after, it is clamped to its `maxAttributeID` /
//!    `minAttributeID` attribute of the same item; `dataType` 10-12 is then
//!    truncated, and the six CPU/powergrid attributes rounded to two places.
//! 2. **A modifier is one `modifierInfo` element of a started effect**
//!    (`modifiers.pyc`): its `domain` picks an item from the source's
//!    environment (`itemID`, `shipID`, `charID`, `otherID`), its `func` says
//!    which items of that one it reaches -- the item itself
//!    (`ItemModifier`), everything fitted in it (`LocationModifier`), those of
//!    a group (`LocationGroupModifier`), those requiring a skill
//!    (`LocationRequiredSkillModifier`), or everything the character owns that
//!    requires a skill (`OwnerRequiredSkillModifier`; `bdl:2112-2208`). The
//!    *value* is the source item's own (modified) `modifyingAttributeID`.
//! 3. **Which effects are started**: an item's passive effects (category 0)
//!    as soon as it is fitted or trained (`bdl.StartPassiveEffects:1153`,
//!    `FitItemToLocation:759`); a module's category-4 effects when it goes
//!    online (`bdl._StartEffect:998-1004`); its default activation effect
//!    while it runs. A skill's level is its `skillLevel` (280), which no
//!    modifier may touch (`skillLevelAttribute.AddIncomingModifier`).
//! 4. **Stacking penalties** (`nurfed`): an attribute whose
//!    `dogmaattributes.stackable` is false is a `StackingNurfedAttribute`
//!    (`factory:32`). Within one operator, modifiers whose operator is
//!    pre/post-mul, pre/post-div or post-percent (`const.dgmNurfableOperations`)
//!    and whose source item is **not** a skill, implant, ship, charge or
//!    subsystem (`const.dgmUnnerfedCategories`) are penalised: each becomes a
//!    factor `op(1, value)`, the factors are sorted, those `<= 1` taken
//!    strongest-first and those `> 1` largest-first, the i-th of each scaled
//!    as `(f - 1) / exp((i / 2.67)^2) + 1`, **only the first eight** of each
//!    kept, and the product multiplied in.
//! 5. **Afterburners and microwarpdrives are Python effects** (`prop`): mass
//!    `+= massAddition` (and, for an MWD, signature radius post-percent by
//!    `signatureRadiusBonus`), then `maxVelocity` post-multiplied by
//!    `1 + 0.01 * speedFactor * speedBoostFactor / shipMass`, a literal with
//!    no item -- so it **is** stacking-penalised.
//!
//! # What is `[A]`
//!
//! - The order of modifiers *within* one operator: the client keeps them in a
//!   `set`. It matters only for the two assignments; here the last in input
//!   order wins.
//! - A loaded charge counts as fitted in the ship (so a ship's location
//!   modifiers reach it), the character has no location, and the character's
//!   own type attributes are all defaults.
//! - The afterburner's literal is evaluated against the ship's mass *now*,
//!   which is the client's `RefreshEffect` steady state.
//! - Overload (category 5), remote (target-domain) and system-wide effects,
//!   implants, boosters, drones, fleet bonuses: not modelled.
//!
//! # Pure, then cached
//!
//! [`evaluate`] is a pure function of a [`Table`] and an [`Inputs`]. The
//! world-facing [`sheet`] gathers the inputs for one ship from the registry
//! and keeps the last answer per ship, **keyed by the inputs themselves** --
//! so a fit, an online change, an activation or a skill level that changes
//! is a different key and a recomputation, with no invalidation hook to
//! forget. The table is the world's own (`reg.content()`, CP58c, ruling 3)
//! and so is the cache: a world slot, [`SHEETS`] (CA7), so two worlds on two
//! tables never share an answer. A world whose table does not hold the hull
//! has no sheet, and every consumer reads its hand table exactly as before.

use crate::service::Registry;
use crate::typedata::{Domain, Func, Table, TypeRow};
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// The attribute ids this file reads by name (`dogmaattributes`) [V].
pub mod attr {
    pub const MASS: i64 = 4;
    pub const CAPACITOR_NEED: i64 = 6;
    pub const HP: i64 = 9;
    pub const POWER_OUTPUT: i64 = 11;
    /// `lowSlots`, `medSlots`, `hiSlots` (T3-D: a strategic cruiser's come
    /// from its subsystems, `Eval::subsystem_slots`).
    pub const LOW_SLOTS: i64 = 12;
    pub const MED_SLOTS: i64 = 13;
    pub const HI_SLOTS: i64 = 14;
    pub const POWER_LOAD: i64 = 15;
    pub const SPEED_FACTOR: i64 = 20;
    pub const POWER: i64 = 30;
    pub const MAX_VELOCITY: i64 = 37;
    pub const CAPACITY: i64 = 38;
    pub const CPU_OUTPUT: i64 = 48;
    pub const CPU_LOAD: i64 = 49;
    pub const CPU: i64 = 50;
    pub const SPEED: i64 = 51;
    pub const MAX_RANGE: i64 = 54;
    pub const RECHARGE_RATE: i64 = 55;
    pub const DAMAGE_MULTIPLIER: i64 = 64;
    pub const AGILITY: i64 = 70;
    pub const DURATION: i64 = 73;
    pub const MAX_TARGET_RANGE: i64 = 76;
    /// `launcherSlotsLeft`, `turretSlotsLeft` (T3-D).
    pub const LAUNCHER_SLOTS: i64 = 101;
    pub const TURRET_SLOTS: i64 = 102;
    pub const FALLOFF: i64 = 158;
    pub const VOLUME: i64 = 161;
    pub const RADIUS: i64 = 162;
    pub const TRACKING_SPEED: i64 = 160;
    pub const MAX_LOCKED_TARGETS: i64 = 192;
    pub const SHIELD_CAPACITY: i64 = 263;
    pub const ARMOR_HP: i64 = 265;
    pub const SKILL_LEVEL: i64 = 280;
    pub const SHIELD_RECHARGE_RATE: i64 = 479;
    pub const CAPACITOR_CAPACITY: i64 = 482;
    pub const SIGNATURE_RADIUS: i64 = 552;
    pub const SIGNATURE_RADIUS_BONUS: i64 = 554;
    /// `signatureRadiusBonusPercent`: the Micro Jump Drive's spool bloom.
    pub const SIGNATURE_RADIUS_BONUS_PERCENT: i64 = 973;
    pub const SCAN_RESOLUTION: i64 = 564;
    pub const SPEED_BOOST_FACTOR: i64 = 567;
    pub const WARP_SPEED_MULTIPLIER: i64 = 600;
    pub const MASS_ADDITION: i64 = 796;
    /// `boosterDuration` (330): how long a booster lasts, in milliseconds [V].
    pub const BOOSTER_DURATION: i64 = 330;
    pub const BASE_WARP_SPEED: i64 = 1281;
    /// `requiredSkill1..6`, the type's direct prerequisites. The client's
    /// `reqSkills` is `requiredskillsfortypes`, which `docs/re/dogma.md` §6
    /// shows agrees with these six attributes on every one of 9,573 rows.
    pub const REQUIRED_SKILLS: [i64; 6] = [182, 183, 184, 1285, 1289, 1290];
}

/// Effects the engine treats by id [V] (`dogmaeffects`, names in `prop`).
pub mod effect {
    /// `moduleBonusAfterburner`, `effectModuleBonusAfterburner`.
    pub const AFTERBURNER: i64 = 6731;
    /// `moduleBonusMicrowarpdrive`.
    pub const MICROWARPDRIVE: i64 = 6730;
    /// The Micro Jump Drive's effect (`microJumpDrive`, MJD-MASS).
    pub const MICRO_JUMP_DRIVE: i64 = 4921;
    /// `slotModifier` (T3-D): a fitted subsystem adds its `hiSlotModifier`,
    /// `medSlotModifier` and `lowSlotModifier` to the hull's slot counts. A
    /// Python effect with no `modifierInfo` [V-data]
    /// (`eve/common/script/dogma/slotModifier`), so it is read here.
    pub const SLOT_MODIFIER: i64 = 3774;
    /// `warpDisruptSphere` (BUBBLES-SB): a Heavy Interdictor's field.
    pub const WARP_DISRUPT_SPHERE: i64 = 3380;
    /// `hardPointModifierEffect` (T3-D): `turretHardPointModifier` and
    /// `launcherHardPointModifier` onto the hull's hardpoints; also Python.
    pub const HARDPOINT_MODIFIER: i64 = 3773;
}

/// `dogma/const.pyc:8-18` [V].
pub mod op {
    pub const PRE_ASSIGN: i64 = -1;
    pub const PRE_MUL: i64 = 0;
    pub const PRE_DIV: i64 = 1;
    pub const MOD_ADD: i64 = 2;
    pub const MOD_SUB: i64 = 3;
    pub const POST_MUL: i64 = 4;
    pub const POST_DIV: i64 = 5;
    pub const POST_PERCENT: i64 = 6;
    pub const POST_ASSIGN: i64 = 7;
    pub const POST_PERCENT_UNNERFED: i64 = 8;
}

/// `dgmOperatorPrecedence` (`const:22-31`) [V]. `dgmAssSpecialSkillRelationship`
/// (9) is not in it, so the client cannot apply one (`dgmOperatorIDtoIndex`
/// raises); neither do we.
pub const PRECEDENCE: [i64; 10] = [
    op::PRE_ASSIGN,
    op::PRE_MUL,
    op::PRE_DIV,
    op::MOD_ADD,
    op::MOD_SUB,
    op::POST_MUL,
    op::POST_DIV,
    op::POST_PERCENT,
    op::POST_PERCENT_UNNERFED,
    op::POST_ASSIGN,
];

/// `dgmNurfableOperations` (`const:51-55`) [V].
const NURFABLE: [i64; 5] = [
    op::PRE_MUL,
    op::POST_MUL,
    op::POST_PERCENT,
    op::PRE_DIV,
    op::POST_DIV,
];

/// `dgmUnnerfedCategories` (`const:59-63`) [V]: skill 16, implant 20, ship 6,
/// charge 8, subsystem 32 (`inventorycommon/const`).
const UNNERFED_CATEGORIES: [i64; 5] = [16, 20, 6, 8, 32];

/// `MAX_NURF_SEQUENCE_LENGTH` (`nurfed:9`) [V].
const MAX_NURF_SEQUENCE_LENGTH: usize = 8;

/// `cpuLoad`, `powerLoad`, `powerOutput`, `cpuOutput`, `cpu`, `power`:
/// `round(val, 2)` (`attribute.Update:383-386`) [V].
const ROUNDED: [i64; 6] = [
    attr::CPU_LOAD,
    attr::POWER_LOAD,
    attr::POWER_OUTPUT,
    attr::CPU_OUTPUT,
    attr::CPU,
    attr::POWER,
];

/// `flagImplant` [V]: where an implant sits in its character.
pub const FLAG_IMPLANT: i64 = 89;

/// `effectCategory` values (`const:92-99`) [V].
const CATEGORY_PASSIVE: i64 = 0;
/// `effectCategory` 7: a system-wide effect (an effect beacon's).
const CATEGORY_SYSTEM: i64 = 7;
const CATEGORY_ACTIVATION: i64 = 1;
const CATEGORY_ONLINE: i64 = 4;
/// `dgmEffOverload` [V] (`dogma/const.pyc`): started by the overload toggle.
const CATEGORY_OVERLOAD: i64 = 5;

/// `categoryModule` / `categoryCharge` / `categorySkill` [V].
const CATEGORY_MODULE: i64 = 7;
const CATEGORY_CHARGE: i64 = 8;
/// `categoryImplant` [V]: an implant's own category when the table lacks it.
const CATEGORY_IMPLANT: i64 = 20;

/// One operator of `dgmOperators` (`const:72-82`) [V]: `ret` is the value so
/// far. An unknown operator leaves it alone.
pub fn operate(op: i64, ret: f64, value: f64) -> f64 {
    match op {
        op::PRE_ASSIGN | op::POST_ASSIGN => value,
        op::PRE_MUL | op::POST_MUL => ret * value,
        op::PRE_DIV | op::POST_DIV => ret / value,
        op::MOD_ADD => ret + value,
        op::MOD_SUB => ret - value,
        op::POST_PERCENT | op::POST_PERCENT_UNNERFED => ret * (100.0 + value) / 100.0,
        _ => ret,
    }
}

/// `NurfDenominators[i] = exp((i / 2.67) ** 2)` (`nurfed:10`) [V].
pub fn nurf_denominator(i: usize) -> f64 {
    ((i as f64 / 2.67).powi(2)).exp()
}

/// `StackingOperator((value, denom)) = (value - 1) * (1 / denom) + 1`
/// (`nurfed:17-18`) [V].
fn stacking_operator(value: f64, denom: f64) -> f64 {
    (value - 1.0) * (1.0 / denom) + 1.0
}

/// One incoming modifier, reduced to what the arithmetic needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Incoming {
    pub op: i64,
    pub value: f64,
    /// Whether it goes into the penalised set (`nurfed.AddIncomingModifier`).
    pub penalised: bool,
}

/// `_ApplyModifierOperationsToVal`, both classes in one (`attribute:394`,
/// `nurfed:132`) [V]: for each operator in precedence order, the unpenalised
/// modifiers one after another, then the penalised ones as one stacked
/// product.
///
/// ```
/// use eve_server::dogma::{apply, op, Incoming};
/// // +10% then x2, in precedence order whatever the input order.
/// let v = apply(100.0, &[
///     Incoming { op: op::POST_MUL, value: 2.0, penalised: false },
///     Incoming { op: op::POST_PERCENT, value: 10.0, penalised: false },
/// ]);
/// assert!((v - 220.0).abs() < 1e-9);
/// ```
pub fn apply(base: f64, mods: &[Incoming]) -> f64 {
    let mut val = base;
    for &o in &PRECEDENCE {
        for m in mods.iter().filter(|m| m.op == o && !m.penalised) {
            val = operate(o, val, m.value);
        }
        let mut factors: Vec<f64> = mods
            .iter()
            .filter(|m| m.op == o && m.penalised)
            .map(|m| operate(o, 1.0, m.value))
            .collect();
        if factors.is_empty() {
            continue;
        }
        factors.sort_by(|a, b| a.total_cmp(b));
        // `bisect.bisect(factors, 1.0)`: bisect_right, so a factor of
        // exactly 1.0 is on the "penalty" side -- where it changes nothing.
        let split = factors.partition_point(|f| *f <= 1.0);
        let below = factors[..split]
            .iter()
            .take(MAX_NURF_SEQUENCE_LENGTH)
            .enumerate();
        let above = factors[split..]
            .iter()
            .rev()
            .take(MAX_NURF_SEQUENCE_LENGTH)
            .enumerate();
        let product: f64 = below
            .chain(above)
            .map(|(i, f)| stacking_operator(*f, nurf_denominator(i)))
            .product();
        val *= product;
    }
    val
}

/// How a fitted module stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    Offline,
    Online,
    /// Online with its overload effect on, not running (K-OVERHEAT).
    Overloaded,
    /// Online and running its default effect.
    Active,
    /// Running its default effect with its overload effect on (K-OVERHEAT).
    ActiveOverloaded,
}

impl State {
    /// Running its default effect.
    pub fn running(self) -> bool {
        matches!(self, State::Active | State::ActiveOverloaded)
    }

    /// Its overload effect (category 5) is on.
    pub fn overloaded(self) -> bool {
        matches!(self, State::Overloaded | State::ActiveOverloaded)
    }
}

/// One fitted module, and what is loaded in it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleInput {
    pub item_id: i64,
    pub type_id: i64,
    pub state: State,
    pub charge_type: Option<i64>,
}

/// Everything a ship's attributes depend on. **The cache key.**
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Inputs {
    pub ship_type: i64,
    /// `(skill typeID, level)`, ascending by type.
    pub skills: Vec<(i64, i64)>,
    /// Ascending by item id.
    pub modules: Vec<ModuleInput>,
    /// The type of every implant in the pilot's active clone, ascending
    /// (K-MSKILL). Items at `flagImplant` (89) in the character.
    pub implants: Vec<i64>,
    /// The pilot's active boosters, ascending by type (BOOST). A cache key, so
    /// deterministic; an expired one is simply not here.
    pub boosters: Vec<BoosterInput>,
    /// The type of the effect beacon of the system the ship is flying in
    /// (SYS-2, `crate::system_effect`); `None` docked, in k-space and for an
    /// NPC hull [A: falsifier, a capture of a Sleeper's changed stats].
    pub beacon: Option<i64>,
}

/// One active booster as the engine sees it: its type and the side effects
/// that were rolled in when it was taken. A side effect (an effect whose
/// `fittingUsageChanceAttributeID` is set) runs only when it is in
/// [`BoosterInput::side_effects`]; the rest of the booster's effects always run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BoosterInput {
    pub type_id: i64,
    pub side_effects: Vec<i64>,
}

/// The answer: every attribute of the ship, its modules and their charges
/// that is either on the type or modified by something.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    pub ship_type: i64,
    ship: BTreeMap<i64, f64>,
    modules: BTreeMap<i64, BTreeMap<i64, f64>>,
    /// Keyed by the **module's** item id.
    charges: BTreeMap<i64, BTreeMap<i64, f64>>,
    /// The pilot's own attributes (K-MSKILL): `missileDamageMultiplier`.
    character: BTreeMap<i64, f64>,
    /// Defaults of every attribute read through a getter, for an attribute
    /// neither the type nor any modifier names.
    defaults: BTreeMap<i64, f64>,
}

impl Sheet {
    /// A ship attribute, modified.
    pub fn ship(&self, attribute_id: i64) -> f64 {
        self.ship
            .get(&attribute_id)
            .or_else(|| self.defaults.get(&attribute_id))
            .copied()
            .unwrap_or(0.0)
    }

    /// A ship attribute the sheet computed, `None` for one it did not list.
    pub fn ship_opt(&self, attribute_id: i64) -> Option<f64> {
        self.ship.get(&attribute_id).copied()
    }

    /// Every ship attribute this sheet computed, ascending.
    pub fn ship_attributes(&self) -> &BTreeMap<i64, f64> {
        &self.ship
    }

    /// A fitted module's attribute the sheet computed, `None` for one it did
    /// not list (or a module not on it).
    pub fn module_opt(&self, item_id: i64, attribute_id: i64) -> Option<f64> {
        self.modules.get(&item_id)?.get(&attribute_id).copied()
    }

    /// The pilot's attribute, modified: what implants, rigs and ballistic
    /// control systems do to `missileDamageMultiplier` (K-MSKILL).
    pub fn character(&self, attribute_id: i64) -> f64 {
        self.character
            .get(&attribute_id)
            .or_else(|| self.defaults.get(&attribute_id))
            .copied()
            .unwrap_or(0.0)
    }

    /// A fitted module's attribute, modified. `None` for a module not on
    /// this sheet.
    pub fn module(&self, item_id: i64, attribute_id: i64) -> Option<f64> {
        let m = self.modules.get(&item_id)?;
        Some(
            m.get(&attribute_id)
                .or_else(|| self.defaults.get(&attribute_id))
                .copied()
                .unwrap_or(0.0),
        )
    }

    /// Every attribute this sheet computed for a module, ascending.
    pub fn module_attributes(&self, item_id: i64) -> Option<&BTreeMap<i64, f64>> {
        self.modules.get(&item_id)
    }

    /// The attribute of the charge loaded in module `item_id`.
    pub fn charge(&self, module_item_id: i64, attribute_id: i64) -> Option<f64> {
        let m = self.charges.get(&module_item_id)?;
        Some(
            m.get(&attribute_id)
                .or_else(|| self.defaults.get(&attribute_id))
                .copied()
                .unwrap_or(0.0),
        )
    }

    /// The fitting window's `cpuLoad`/`powerLoad`: the sum over the online
    /// modules' modified `cpu`/`power`, optionally leaving one out (the one
    /// being onlined). `(cpu, power)`.
    pub fn load(&self, inputs: &Inputs, except: Option<i64>) -> (f64, f64) {
        let mut cpu = 0.0;
        let mut power = 0.0;
        for m in inputs
            .modules
            .iter()
            .filter(|m| m.state != State::Offline && Some(m.item_id) != except)
        {
            cpu += self.module(m.item_id, attr::CPU).unwrap_or(0.0);
            power += self.module(m.item_id, attr::POWER).unwrap_or(0.0);
        }
        (cpu, power)
    }
}

/// Ship attributes a consumer reads, computed even when the hull's type row
/// does not carry them so the getter never needs the table.
const SHIP_READS: &[i64] = &[
    attr::MASS,
    attr::HP,
    attr::POWER_OUTPUT,
    attr::MAX_VELOCITY,
    attr::CAPACITY,
    attr::CPU_OUTPUT,
    attr::RECHARGE_RATE,
    attr::AGILITY,
    attr::MAX_TARGET_RANGE,
    attr::RADIUS,
    attr::MAX_LOCKED_TARGETS,
    attr::SHIELD_CAPACITY,
    attr::ARMOR_HP,
    attr::SHIELD_RECHARGE_RATE,
    attr::CAPACITOR_CAPACITY,
    attr::SIGNATURE_RADIUS,
    attr::SCAN_RESOLUTION,
    // The resistances to projected effects, so a hull with no row for one
    // reads its default 1 and not 0 (EWAR-2B: 2113 disruptors, 2135 remote
    // tracking, 2253 ECM, 2045 energy warfare; 2112 sensor dampeners, 2114
    // target painters, 2115 webifiers, the ones a module's own 2138 names:
    // a hull with no 2114 row read 0 and refused every painter as immune).
    2045,
    2112,
    2113,
    2114,
    2115,
    2135,
    2253,
    attr::WARP_SPEED_MULTIPLIER,
    attr::BASE_WARP_SPEED,
    271,
    272,
    273,
    274,
    267,
    268,
    269,
    270,
    113,
    111,
    109,
    110,
];

/// Module and charge attributes a consumer reads.
const MODULE_READS: &[i64] = &[
    attr::CAPACITOR_NEED,
    attr::POWER,
    attr::CPU,
    attr::SPEED,
    attr::MAX_RANGE,
    attr::DAMAGE_MULTIPLIER,
    attr::DURATION,
    attr::FALLOFF,
    attr::TRACKING_SPEED,
    114,
    116,
    117,
    118,
    // A missile's flight and explosion (K-MSKILL): velocity, explosion
    // delay, explosion velocity, explosion radius, damage reduction factor.
    37,
    281,
    653,
    654,
    1353,
    // A HIC script's `gateScrambleStrength` (HIC-SCRIPT): no module row
    // carries it, so only the attribute's default of 1 gives a point its
    // strength.
    1974,
];

/// The pilot's attributes a consumer reads (K-MSKILL): 212
/// `missileDamageMultiplier`.
const CHARACTER_READS: &[i64] = &[212];

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

const CHAR: usize = 0;
const SHIP: usize = 1;
/// `groupPropulsionModule` [V]: what a field generator slows.
const GROUP_PROPULSION: i64 = 46;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Char,
    Ship,
    Skill(i64),
    /// An implant of this type, in the active clone (K-MSKILL).
    Implant(i64),
    Module(i64),
    /// Loaded in the module with this item id.
    Charge(i64),
}

struct Node<'t> {
    kind: Kind,
    row: Option<&'t TypeRow>,
    category: i64,
    group: i64,
    req_skills: Vec<i64>,
    /// The item this one is fitted in (`IterFittedItems`).
    location: Option<usize>,
    /// `otherID`: module <-> charge.
    other: Option<usize>,
    /// `IsOwnerModifiable`: a charge in the pilot's ship (`chargeDogmaItem`
    /// `:21-36`) [V]; a module, ship or skill is not (`slim:383`).
    owner_modifiable: bool,
    /// A booster's rolled side effects; `None` for every node that is not one.
    rolled: Option<Vec<i64>>,
}

/// The target a modifier element aims at, from its function and the node its
/// domain names. `None` for a function the engine does not apply.
fn aim_of(m: &crate::typedata::Modifier, domain: usize) -> Option<Aim> {
    Some(match m.func {
        Func::ItemModifier => Aim::Item(domain),
        Func::LocationModifier => Aim::Location {
            loc: domain,
            group: None,
            skill: None,
        },
        Func::LocationGroupModifier => Aim::Location {
            loc: domain,
            group: Some(m.group?),
            skill: None,
        },
        Func::LocationRequiredSkillModifier => Aim::Location {
            loc: domain,
            group: None,
            skill: Some(m.skill_type?),
        },
        Func::OwnerRequiredSkillModifier => Aim::Owner {
            owner: domain,
            skill: m.skill_type?,
        },
        Func::EffectStopper => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Aim {
    Item(usize),
    Location {
        loc: usize,
        group: Option<i64>,
        skill: Option<i64>,
    },
    Owner {
        owner: usize,
        skill: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Source {
    Attr(usize, i64),
    /// A warfare buff's output value (K-BOOST): a literal with no item.
    Fixed(f64),
    /// `prop._AddSpeedMultiplierModifier` of this module.
    Speed(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Mod {
    op: i64,
    attr: i64,
    aim: Aim,
    source: Source,
    /// The source item's category, `None` for a literal (`item is None`).
    source_category: Option<i64>,
}

impl Mod {
    /// Whether this modifier lands on the pilot's own attributes.
    fn aims_char(&self) -> bool {
        matches!(self.aim, Aim::Item(i) if i == CHAR)
    }
}

struct Eval<'t> {
    table: &'t Table,
    nodes: Vec<Node<'t>>,
    levels: BTreeMap<usize, i64>,
    mods: Vec<Mod>,
    memo: HashMap<(usize, i64), f64>,
    busy: BTreeSet<(usize, i64)>,
}

impl<'t> Eval<'t> {
    /// `need_hull`: refuse a hull the table does not hold ([`evaluate`]).
    /// A pilot-only evaluation ([`pilot_bonus`]) has no ship, and the ship
    /// node is then a bare one no modifier of the pilot's reaches.
    fn build(table: &'t Table, inputs: &Inputs, need_hull: bool) -> Option<Eval<'t>> {
        let ship_row = table.get(inputs.ship_type);
        if need_hull && ship_row.is_none() {
            return None;
        }
        let req = |row: Option<&TypeRow>| -> Vec<i64> {
            row.map_or_else(Vec::new, |r| {
                attr::REQUIRED_SKILLS
                    .iter()
                    .filter_map(|a| r.attribute(*a))
                    .filter(|v| *v > 0.0)
                    .map(|v| v as i64)
                    .collect()
            })
        };
        let mut nodes = vec![
            Node {
                kind: Kind::Char,
                row: None,
                category: 1,
                group: 1,
                req_skills: Vec::new(),
                location: None,
                other: None,
                owner_modifiable: false,
                rolled: None,
            },
            Node {
                kind: Kind::Ship,
                row: ship_row,
                category: ship_row.map_or(crate::typedata::CATEGORY_SHIP, |r| r.category_id),
                group: ship_row.map_or(0, |r| r.group_id),
                req_skills: req(ship_row),
                location: None,
                other: None,
                owner_modifiable: false,
                rolled: None,
            },
        ];
        let mut levels = BTreeMap::new();
        for &(skill, level) in &inputs.skills {
            let row = table.get(skill);
            levels.insert(nodes.len(), level);
            nodes.push(Node {
                kind: Kind::Skill(skill),
                row,
                category: row.map_or(16, |r| r.category_id),
                group: row.map_or(0, |r| r.group_id),
                req_skills: req(row),
                location: Some(CHAR),
                other: None,
                owner_modifiable: false,
                rolled: None,
            });
        }
        for &implant in &inputs.implants {
            let row = table.get(implant);
            nodes.push(Node {
                kind: Kind::Implant(implant),
                row,
                category: row.map_or(CATEGORY_IMPLANT, |r| r.category_id),
                group: row.map_or(0, |r| r.group_id),
                req_skills: req(row),
                location: Some(CHAR),
                other: None,
                owner_modifiable: false,
                rolled: None,
            });
        }
        for b in &inputs.boosters {
            let row = table.get(b.type_id);
            nodes.push(Node {
                kind: Kind::Implant(b.type_id),
                row,
                category: row.map_or(CATEGORY_IMPLANT, |r| r.category_id),
                group: row.map_or(0, |r| r.group_id),
                req_skills: req(row),
                location: Some(CHAR),
                other: None,
                owner_modifiable: false,
                rolled: Some(b.side_effects.clone()),
            });
        }
        for m in &inputs.modules {
            let row = table.get(m.type_id);
            let at = nodes.len();
            nodes.push(Node {
                kind: Kind::Module(m.item_id),
                row,
                category: row.map_or(CATEGORY_MODULE, |r| r.category_id),
                group: row.map_or(0, |r| r.group_id),
                req_skills: req(row),
                location: Some(SHIP),
                other: None,
                owner_modifiable: false,
                rolled: None,
            });
            if let Some(c) = m.charge_type {
                let crow = table.get(c);
                let ci = nodes.len();
                nodes.push(Node {
                    kind: Kind::Charge(m.item_id),
                    row: crow,
                    category: crow.map_or(CATEGORY_CHARGE, |r| r.category_id),
                    group: crow.map_or(0, |r| r.group_id),
                    req_skills: req(crow),
                    location: Some(SHIP),
                    other: Some(at),
                    owner_modifiable: true,
                    rolled: None,
                });
                nodes[at].other = Some(ci);
            }
        }
        let mut e = Eval {
            table,
            nodes,
            levels,
            mods: Vec::new(),
            memo: HashMap::new(),
            busy: BTreeSet::new(),
        };
        e.gather(inputs);
        Some(e)
    }

    /// The live warfare buffs of the ship (K-BOOST): each collection's
    /// modifiers with the buff's output value, from a source with no item
    /// (`registry.ModifierApplier.ApplyToDogmaItem`, `DynamicBuffAttribute`):
    /// item modifiers on the ship itself, the three location kinds on what is
    /// fitted in it.
    fn add_dbuffs(&mut self, dbuffs: &[crate::dbuff::Applied]) {
        for b in dbuffs {
            let mut push = |attr: i64, aim: Aim| {
                self.mods.push(Mod {
                    op: b.op,
                    attr,
                    aim,
                    source: Source::Fixed(b.value),
                    source_category: None,
                });
            };
            for a in &b.item {
                push(*a, Aim::Item(SHIP));
            }
            for a in &b.location {
                push(
                    *a,
                    Aim::Location {
                        loc: SHIP,
                        group: None,
                        skill: None,
                    },
                );
            }
            for (a, g) in &b.group {
                push(
                    *a,
                    Aim::Location {
                        loc: SHIP,
                        group: Some(*g),
                        skill: None,
                    },
                );
            }
            for (a, s) in &b.skill {
                push(
                    *a,
                    Aim::Location {
                        loc: SHIP,
                        group: None,
                        skill: Some(*s),
                    },
                );
            }
        }
    }

    /// Start every effect rule 3 says is started, and turn each
    /// `modifierInfo` element into a [`Mod`].
    fn gather(&mut self, inputs: &Inputs) {
        let states: BTreeMap<i64, State> = inputs
            .modules
            .iter()
            .map(|m| (m.item_id, m.state))
            .collect();
        for i in 0..self.nodes.len() {
            let Some(row) = self.nodes[i].row else {
                continue;
            };
            let state = match self.nodes[i].kind {
                Kind::Module(id) => states.get(&id).copied(),
                _ => None,
            };
            for &effect_id in &row.effects {
                let Some(e) = self.table.effect(effect_id) else {
                    continue;
                };
                let started = match (e.category, state) {
                    (CATEGORY_PASSIVE, _) => true,
                    (CATEGORY_ONLINE, Some(s)) => s >= State::Online,
                    (CATEGORY_ACTIVATION, Some(s)) if s.running() => {
                        // A jump portal generator has no default effect
                        // [V-data: `defaultEffectID` 0], so its one activation
                        // effect (2152 / 3674) is the one that runs.
                        row.default_effect == Some(effect_id)
                            || (row.default_effect.is_none()
                                && crate::jump_portal::is_portal(effect_id))
                    }
                    (CATEGORY_OVERLOAD, Some(s)) => s.overloaded(),
                    _ => false,
                };
                if !started {
                    continue;
                }
                // A booster's side effect runs only when it was rolled.
                if e.usage_chance_attribute.is_some_and(|a| a > 0)
                    && self.nodes[i]
                        .rolled
                        .as_ref()
                        .is_some_and(|r| !r.contains(&effect_id))
                {
                    continue;
                }
                if effect_id == effect::AFTERBURNER || effect_id == effect::MICROWARPDRIVE {
                    self.propulsion(i, effect_id == effect::MICROWARPDRIVE);
                    continue;
                }
                // The Micro Jump Drive's Python effect (`BaseMicroJumpDrive`)
                // [V]: while it spools, the signature blooms by 973 percent.
                // It carries no `modifierInfo`.
                if effect_id == effect::MICRO_JUMP_DRIVE {
                    self.mods.push(Mod {
                        op: op::POST_PERCENT,
                        attr: attr::SIGNATURE_RADIUS,
                        aim: Aim::Item(SHIP),
                        source: Source::Attr(i, attr::SIGNATURE_RADIUS_BONUS_PERCENT),
                        source_category: Some(self.nodes[i].category),
                    });
                    continue;
                }
                // The Warp Disruption Field Generator's Python effect
                // (`warpDisruptSphere`, BUBBLES-SB) [V]: percent bonuses to
                // the ship's mass, top speed and signature, and to the speed
                // factor of its propulsion modules (group 46). It carries no
                // `modifierInfo`; this build's II module has 0 for all but
                // the signature's 50.
                if effect_id == effect::WARP_DISRUPT_SPHERE {
                    self.warp_disrupt_sphere(i);
                    continue;
                }
                // The client's `migratedeffects` Python effects: a skill's
                // damage and rate-of-fire bonuses, which carry no
                // `modifierInfo` (K-MSKILL).
                if effect_id == effect::SLOT_MODIFIER || effect_id == effect::HARDPOINT_MODIFIER {
                    self.subsystem_slots(i, effect_id == effect::HARDPOINT_MODIFIER);
                    continue;
                }
                if let Some(m) = crate::missile_skills::migrated(effect_id) {
                    if let Kind::Skill(skill) = self.nodes[i].kind {
                        self.migrated(i, skill, m);
                    }
                    continue;
                }
                for m in self.table.modifiers(effect_id) {
                    if let Some(m) = self.resolve(i, m) {
                        self.mods.push(m);
                    }
                }
            }
        }
    }

    /// One `modifierInfo` element from item `i`, or `None` for one this
    /// engine does not apply (rule 2, and the `[A]`s in the module header).
    fn resolve(&self, i: usize, m: &crate::typedata::Modifier) -> Option<Mod> {
        let op = m.operation?;
        if !PRECEDENCE.contains(&op) {
            return None;
        }
        let modified = m.modified_attribute?;
        let modifying = m.modifying_attribute?;
        let domain = match m.domain {
            Domain::Item => i,
            Domain::Ship => SHIP,
            Domain::Char => CHAR,
            // `bdl.StartPassiveEffects:1204`: an otherID effect with no other
            // is not started.
            Domain::Other => self.nodes[i].other?,
            Domain::Structure | Domain::Target => return None,
        };
        let aim = aim_of(m, domain)?;
        Some(Mod {
            op,
            attr: modified,
            aim,
            source: Source::Attr(i, modifying),
            source_category: Some(self.nodes[i].category),
        })
    }

    /// The system's effect beacon (SYS-2): each modifier of each category 7
    /// effect of the beacon type, valued by the beacon's own attribute as a
    /// literal (the beacon is no item here, like a warfare buff's source).
    /// Only `shipID` and `charID` reach anything; `itemID` is the beacon
    /// itself. Stacking-penalised like any non-skill source [V: a beacon is
    /// category 2, `_ShouldNotNurf`].
    fn add_beacon(&mut self, beacon: i64) {
        let Some(row) = self.table.get(beacon) else {
            return;
        };
        for &effect_id in &row.effects {
            if self
                .table
                .effect(effect_id)
                .is_none_or(|e| e.category != CATEGORY_SYSTEM)
            {
                continue;
            }
            for m in self.table.modifiers(effect_id) {
                let domain = match m.domain {
                    Domain::Ship => SHIP,
                    Domain::Char => CHAR,
                    _ => continue,
                };
                let (Some(op), Some(modified), Some(modifying)) =
                    (m.operation, m.modified_attribute, m.modifying_attribute)
                else {
                    continue;
                };
                let (Some(aim), Some(value)) = (aim_of(m, domain), row.attribute(modifying)) else {
                    continue;
                };
                if !PRECEDENCE.contains(&op) {
                    continue;
                }
                self.mods.push(Mod {
                    op,
                    attr: modified,
                    aim,
                    source: Source::Fixed(value),
                    source_category: None,
                });
            }
        }
    }

    /// A `migratedeffects` effect of skill `skill` (node `i`): its modifier
    /// aims at everything of the pilot's (`AddOwnerRequiredSkillModifier`,
    /// `charID`) or of the ship (`AddLocationRequiredSkillModifier`,
    /// `shipID`) that requires the skill itself, valued by the skill's own
    /// attribute [V] (`migratedeffects.pyc`, K-MSKILL).
    fn migrated(&mut self, i: usize, skill: i64, m: crate::missile_skills::Migrated) {
        let aim = match m.reach {
            crate::missile_skills::Reach::Pilot => Aim::Owner { owner: CHAR, skill },
            crate::missile_skills::Reach::Ship => Aim::Location {
                loc: SHIP,
                group: None,
                skill: Some(skill),
            },
        };
        self.mods.push(Mod {
            op: m.operation,
            attr: m.modified,
            aim,
            source: Source::Attr(i, m.modifying),
            source_category: Some(self.nodes[i].category),
        });
    }

    /// `slotModifier` / `hardPointModifierEffect` of a fitted subsystem
    /// (node `module`): each of the hull's slot or hardpoint counts takes
    /// the subsystem's matching modifier attribute, `dgmAssModAdd` onto the
    /// ship (T3-D; `crate::subsystems::SLOT_MODIFIERS` names the pairs [V]).
    fn subsystem_slots(&mut self, module: usize, hardpoints: bool) {
        let pairs: &[(i64, i64)] = if hardpoints {
            &crate::subsystems::HARDPOINT_MODIFIERS
        } else {
            &crate::subsystems::SLOT_MODIFIERS
        };
        let category = Some(self.nodes[module].category);
        for &(ship_attr, modifier) in pairs {
            self.mods.push(Mod {
                op: op::MOD_ADD,
                attr: ship_attr,
                aim: Aim::Item(SHIP),
                source: Source::Attr(module, modifier),
                source_category: category,
            });
        }
    }

    /// `prop.Afterburner.Start` / `Microwarpdrive` (rule 5).
    fn propulsion(&mut self, module: usize, mwd: bool) {
        let category = Some(self.nodes[module].category);
        self.mods.push(Mod {
            op: op::MOD_ADD,
            attr: attr::MASS,
            aim: Aim::Item(SHIP),
            source: Source::Attr(module, attr::MASS_ADDITION),
            source_category: category,
        });
        if mwd {
            self.mods.push(Mod {
                op: op::POST_PERCENT,
                attr: attr::SIGNATURE_RADIUS,
                aim: Aim::Item(SHIP),
                source: Source::Attr(module, attr::SIGNATURE_RADIUS_BONUS),
                source_category: category,
            });
        }
        self.mods.push(Mod {
            op: op::POST_MUL,
            attr: attr::MAX_VELOCITY,
            aim: Aim::Item(SHIP),
            source: Source::Speed(module),
            source_category: None,
        });
    }

    /// `dogmaXP.WarpDisruptSphere` **[V]**: on the ship, a percent bonus to
    /// mass (1131), maximum velocity (1076) and signature radius (554); on
    /// the ship's propulsion modules (group 46), to their speed boost factor
    /// (1270) and speed factor (1164).
    fn warp_disrupt_sphere(&mut self, module: usize) {
        let category = Some(self.nodes[module].category);
        for (modified, modifying) in [
            (attr::MASS, 1_131),
            (attr::MAX_VELOCITY, 1_076),
            (attr::SIGNATURE_RADIUS, attr::SIGNATURE_RADIUS_BONUS),
        ] {
            self.mods.push(Mod {
                op: op::POST_PERCENT,
                attr: modified,
                aim: Aim::Item(SHIP),
                source: Source::Attr(module, modifying),
                source_category: category,
            });
        }
        for (modified, modifying) in [
            (attr::SPEED_BOOST_FACTOR, 1_270),
            (attr::SPEED_FACTOR, 1_164),
        ] {
            self.mods.push(Mod {
                op: op::POST_PERCENT,
                attr: modified,
                aim: Aim::Location {
                    loc: SHIP,
                    group: Some(GROUP_PROPULSION),
                    skill: None,
                },
                source: Source::Attr(module, modifying),
                source_category: category,
            });
        }
    }

    fn aims_at(&self, aim: Aim, t: usize) -> bool {
        let n = &self.nodes[t];
        match aim {
            Aim::Item(i) => i == t,
            Aim::Location { loc, group, skill } => {
                n.location == Some(loc)
                    && group.is_none_or(|g| n.group == g)
                    && skill.is_none_or(|s| n.req_skills.contains(&s))
            }
            Aim::Owner { owner, skill } => {
                owner == CHAR && n.owner_modifiable && n.req_skills.contains(&skill)
            }
        }
    }

    /// The type's value (`attribute.__init__:253`): `typedogma`, else the
    /// attribute's default -- except mass, capacity, volume and radius,
    /// which `data.get_type_attributes:268-272` takes from `types` [V].
    fn base(&self, t: usize, attribute_id: i64) -> f64 {
        let Some(row) = self.nodes[t].row else {
            return self.table.default_value(attribute_id);
        };
        match attribute_id {
            attr::MASS => row.mass,
            attr::CAPACITY => row.capacity,
            attr::VOLUME => row.volume,
            attr::RADIUS => row.radius,
            _ => row
                .attribute(attribute_id)
                .unwrap_or_else(|| self.table.default_value(attribute_id)),
        }
    }

    fn source_value(&mut self, s: Source) -> Option<f64> {
        match s {
            Source::Attr(i, a) => Some(self.value(i, a)),
            Source::Fixed(v) => Some(v),
            Source::Speed(m) => {
                let factor = self.value(m, attr::SPEED_FACTOR);
                let boost = self.value(m, attr::SPEED_BOOST_FACTOR);
                let mass = self.value(SHIP, attr::MASS);
                // Python would raise on a zero mass; the effect then never
                // starts [A].
                (mass != 0.0).then(|| 1.0 + 0.01 * factor * boost / mass)
            }
        }
    }

    /// `Attribute.Update` (rule 1).
    fn value(&mut self, t: usize, attribute_id: i64) -> f64 {
        if let Some(v) = self.memo.get(&(t, attribute_id)) {
            return *v;
        }
        if attribute_id == attr::SKILL_LEVEL
            && let Some(level) = self.levels.get(&t)
        {
            return *level as f64;
        }
        let mut base = self.base(t, attribute_id);
        if !self.busy.insert((t, attribute_id)) {
            // A modifier chain that loops back on itself: the value before
            // any modifier [A]. The client's lazy graph never closes a loop
            // on any data this has been run against.
            return base;
        }
        let meta = self.table.attr_meta(attribute_id);
        let cap = (meta.max_attribute != 0).then(|| self.value(t, meta.max_attribute));
        let floor = (meta.min_attribute != 0).then(|| self.value(t, meta.min_attribute));
        if let Some(c) = cap {
            base = base.min(c);
        }
        if let Some(f) = floor {
            base = base.max(f);
        }
        let matching: Vec<Mod> = self
            .mods
            .iter()
            .filter(|m| m.attr == attribute_id && self.aims_at(m.aim, t))
            .copied()
            .collect();
        let mut incoming = Vec::with_capacity(matching.len());
        for m in matching {
            let Some(value) = self.source_value(m.source) else {
                continue;
            };
            if matches!(m.op, op::PRE_DIV | op::POST_DIV) && value == 0.0 {
                // A ZeroDivisionError in the client [A].
                continue;
            }
            let penalised = !meta.stackable
                && NURFABLE.contains(&m.op)
                && m.source_category
                    .is_none_or(|c| !UNNERFED_CATEGORIES.contains(&c));
            incoming.push(Incoming {
                op: m.op,
                value,
                penalised,
            });
        }
        let mut val = apply(base, &incoming);
        if let Some(c) = cap {
            val = val.min(c);
        }
        if let Some(f) = floor {
            val = val.max(f);
        }
        if (10..=12).contains(&meta.data_type) {
            val = val.trunc();
        }
        if ROUNDED.contains(&attribute_id) {
            val = (val * 100.0).round() / 100.0;
        }
        self.busy.remove(&(t, attribute_id));
        self.memo.insert((t, attribute_id), val);
        val
    }

    /// Every attribute of item `t` worth reporting: the type's own, the four
    /// `types` columns, the consumers' reads, and whatever a modifier aims at.
    fn all_of(&mut self, t: usize, reads: &[i64]) -> BTreeMap<i64, f64> {
        let mut ids: BTreeSet<i64> = reads.iter().copied().collect();
        if let Some(row) = self.nodes[t].row {
            ids.extend(row.attributes.iter().map(|(id, _)| *id));
        }
        ids.extend([attr::MASS, attr::CAPACITY, attr::VOLUME, attr::RADIUS]);
        let aimed: Vec<i64> = self
            .mods
            .iter()
            .filter(|m| self.aims_at(m.aim, t))
            .map(|m| m.attr)
            .collect();
        ids.extend(aimed);
        ids.into_iter().map(|a| (a, self.value(t, a))).collect()
    }
}

/// **The engine**: every attribute of the ship in `inputs`, its modules and
/// their charges. `None` when the table does not hold the hull.
pub fn evaluate(table: &Table, inputs: &Inputs) -> Option<Sheet> {
    evaluate_with(table, inputs, &[])
}

/// [`evaluate`] with the ship's live warfare buffs (K-BOOST, `crate::dbuff`).
pub fn evaluate_with(
    table: &Table,
    inputs: &Inputs,
    dbuffs: &[crate::dbuff::Applied],
) -> Option<Sheet> {
    let mut e = Eval::build(table, inputs, true)?;
    e.add_dbuffs(dbuffs);
    if let Some(beacon) = inputs.beacon {
        e.add_beacon(beacon);
    }
    let ship = e.all_of(SHIP, SHIP_READS);
    let char_ids: BTreeSet<i64> = CHARACTER_READS
        .iter()
        .copied()
        .chain(e.mods.iter().filter(|m| m.aims_char()).map(|m| m.attr))
        .collect();
    let character = char_ids
        .into_iter()
        .map(|a| (a, e.value(CHAR, a)))
        .collect();
    let mut modules = BTreeMap::new();
    let mut charges = BTreeMap::new();
    for t in 0..e.nodes.len() {
        match e.nodes[t].kind {
            Kind::Module(id) => {
                modules.insert(id, e.all_of(t, MODULE_READS));
            }
            Kind::Charge(id) => {
                charges.insert(id, e.all_of(t, MODULE_READS));
            }
            _ => {}
        }
    }
    let defaults = SHIP_READS
        .iter()
        .chain(MODULE_READS)
        .chain(CHARACTER_READS)
        .map(|a| (*a, table.default_value(*a)))
        .collect();
    Some(Sheet {
        ship_type: inputs.ship_type,
        ship,
        modules,
        charges,
        character,
        defaults,
    })
}

/// **What the pilot's implants add to the pilot's own attributes**, with no
/// ship in the question (IMPL): `attributes` are character attributes (the
/// five training ones, 164-168), and each answer is the engine's value for
/// that attribute on the character minus the attribute's `defaultValue`, so
/// it is exactly the sum of the modifiers that reach it (an implant's
/// `...BonusModAdd...LocationChar` effects are `charID` `ItemModifier`s,
/// operator 2, from the implant's own `charismaBonus`... attribute [V-data]).
/// The same engine, rules and stacking as a ship's sheet: implants are an
/// unnerfed category, so they never stack-penalise.
pub fn pilot_bonus(
    table: &Table,
    skills: &[(i64, i64)],
    implants: &[i64],
    boosters: &[BoosterInput],
    attributes: &[i64],
) -> BTreeMap<i64, f64> {
    let inputs = Inputs {
        ship_type: 0,
        skills: skills.to_vec(),
        modules: Vec::new(),
        implants: implants.to_vec(),
        boosters: boosters.to_vec(),
        beacon: None,
    };
    let Some(mut e) = Eval::build(table, &inputs, false) else {
        return BTreeMap::new();
    };
    attributes
        .iter()
        .map(|&a| (a, e.value(CHAR, a) - table.default_value(a)))
        .collect()
}

/// What taking a booster would do, worked out by the engine so the pilot's
/// skills (Biology on the duration, Neurotoxin Recovery on the chances) count.
#[derive(Debug, Clone, PartialEq)]
pub struct BoosterStats {
    /// `boosterDuration` (330), modified; `0` for a type that has none.
    pub duration_ms: i64,
    /// Each side effect and its chance in `0..=1`, in the type's effect order.
    pub chances: Vec<(i64, f64)>,
}

/// **A booster's duration and side-effect chances** for this pilot ([`BoosterStats`]),
/// with the candidate as a node with no side effect rolled. The chance is the
/// engine's value of the effect's `fittingUsageChanceAttributeID` on that node,
/// a probability (unit 105, a fraction) **[A]**; falsifier: a booster whose chance
/// attribute reads above 1. `None` for a type the table does not hold.
pub fn booster_stats(
    table: &Table,
    skills: &[(i64, i64)],
    implants: &[i64],
    boosters: &[BoosterInput],
    type_id: i64,
) -> Option<BoosterStats> {
    let row = table.get(type_id)?;
    let mut all = boosters.to_vec();
    all.retain(|b| b.type_id != type_id);
    all.push(BoosterInput {
        type_id,
        side_effects: Vec::new(),
    });
    all.sort();
    let inputs = Inputs {
        ship_type: 0,
        skills: skills.to_vec(),
        modules: Vec::new(),
        implants: implants.to_vec(),
        beacon: None,
        boosters: all,
    };
    let mut e = Eval::build(table, &inputs, false)?;
    let at = e
        .nodes
        .iter()
        .position(|n| n.rolled.is_some() && n.kind == Kind::Implant(type_id))?;
    let duration_ms = match row.attribute(attr::BOOSTER_DURATION) {
        Some(_) => e.value(at, attr::BOOSTER_DURATION).round().max(0.0) as i64,
        None => 0,
    };
    let chances = row
        .effects
        .iter()
        .filter_map(|&id| {
            let a = table
                .effect(id)?
                .usage_chance_attribute
                .filter(|a| *a > 0)?;
            Some((id, e.value(at, a).clamp(0.0, 1.0)))
        })
        .collect();
    Some(BoosterStats {
        duration_ms,
        chances,
    })
}

/// Where a brain modifier lands, in the client's own terms
/// (`effect.py:5-9` `'M'`, `'L'`, `'LG'`, `'LRS'`, `'ORS'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainTarget {
    /// An `ItemModifier` on the pilot's ship.
    ShipItem,
    /// An `ItemModifier` on the pilot.
    CharItem,
    /// A `LocationModifier` on what is fitted to the ship.
    ShipLocation,
    /// A `LocationGroupModifier` on the ship's fittings of this group.
    ShipGroup(i64),
    /// A `LocationRequiredSkillModifier` on the ship's fittings that
    /// require this skill.
    ShipSkill(i64),
    /// An `OwnerRequiredSkillModifier`: the pilot's charges that require
    /// this skill.
    CharOwnerSkill(i64),
}

/// One cross-item modifier a skill or an implant makes, with its value
/// resolved: what the client's dogma cannot work out for itself, because it
/// holds neither the pilot's skills nor their implants as items (GAP-BRAIN).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrainMod {
    /// The skill or implant it comes from.
    pub source_type: i64,
    pub target: BrainTarget,
    pub op: i64,
    /// The modified attribute.
    pub attr: i64,
    pub value: f64,
}

/// **The brain view of the engine** (GAP-BRAIN): every [`Mod`] whose source
/// is one of the pilot's skills or implants and which lands on the pilot or
/// the ship, valued as the sheet values it. A view of the evaluator's own
/// collection, not a second engine, so the client's numbers are the server's
/// by construction.
///
/// Left out: a modifier aimed at its own item (a skill's `skillLevel`
/// plumbing), one aimed at another item, one whose application changes
/// nothing (`+0`, `x1`), and any modifier from a hull or module (the client
/// has those items and works them out itself). Skill and implant categories
/// are unnerfed, so none of these is stacking-penalised: the client's brain
/// literals say the same (`forceUnnurfed`).
pub fn brain_mods(
    table: &Table,
    skills: &[(i64, i64)],
    implants: &[i64],
    boosters: &[BoosterInput],
) -> Vec<BrainMod> {
    let inputs = Inputs {
        ship_type: 0,
        skills: skills.to_vec(),
        modules: Vec::new(),
        implants: implants.to_vec(),
        boosters: boosters.to_vec(),
        beacon: None,
    };
    let Some(mut e) = Eval::build(table, &inputs, false) else {
        return Vec::new();
    };
    let mods = e.mods.clone();
    let mut out = Vec::new();
    for m in mods {
        let Source::Attr(i, modifying) = m.source else {
            continue;
        };
        let source_type = match e.nodes[i].kind {
            Kind::Skill(t) | Kind::Implant(t) => t,
            _ => continue,
        };
        let target = match m.aim {
            Aim::Item(t) if t == i => continue,
            Aim::Item(SHIP) => BrainTarget::ShipItem,
            Aim::Item(CHAR) => BrainTarget::CharItem,
            Aim::Location {
                loc: SHIP,
                group: None,
                skill: None,
            } => BrainTarget::ShipLocation,
            Aim::Location {
                loc: SHIP,
                group: Some(g),
                skill: None,
            } => BrainTarget::ShipGroup(g),
            Aim::Location {
                loc: SHIP,
                group: None,
                skill: Some(s),
            } => BrainTarget::ShipSkill(s),
            Aim::Owner { owner: CHAR, skill } => BrainTarget::CharOwnerSkill(skill),
            _ => continue,
        };
        let value = e.value(i, modifying);
        let identity = match m.op {
            op::MOD_ADD | op::MOD_SUB | op::POST_PERCENT | op::POST_PERCENT_UNNERFED => {
                value == 0.0
            }
            op::PRE_MUL | op::POST_MUL | op::PRE_DIV | op::POST_DIV => value == 1.0,
            _ => false,
        };
        if identity || (matches!(m.op, op::PRE_DIV | op::POST_DIV) && value == 0.0) {
            continue;
        }
        out.push(BrainMod {
            source_type,
            target,
            op: m.op,
            attr: m.attr,
            value,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// The world-facing half
// ---------------------------------------------------------------------------

/// The pilot of `ship_id`: whoever has it as their active ship.
fn pilot_of(w: &Registry, ship_id: i64) -> Option<i64> {
    w.roster()
        .by_ship(ship_id)
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .or_else(|| {
            w.config()
                .characters
                .iter()
                .find(|c| c.ship_id == ship_id)
                .map(|c| c.character_id)
        })
}

/// What a ship's attributes depend on, read off the registry: its type, its
/// pilot's skills, every assembled module at a fitting flag with its state
/// (`crate::modules`: offline, online, or running an effect) and the type of
/// the charge loaded in it.
pub fn inputs(w: &Registry, ship_id: i64) -> Option<Inputs> {
    let ship = w.store().get(ship_id)?;
    let skills = pilot_skills(w, ship_id)
        .map(|s| s.collect())
        .unwrap_or_default();
    Some(Inputs {
        ship_type: ship.type_id,
        skills,
        modules: module_inputs(w, ship_id),
        implants: implant_types(w, ship_id),
        boosters: booster_inputs(w, ship_id),
        beacon: beacon_inputs(w, ship_id),
    })
}

/// [`Inputs::beacon`] of a ship: the effect beacon of the system its pilot
/// is flying in. Only a ship in space has one: the roster holds a pilot only
/// while they are in a system's space, so docked ships, ships in a hangar
/// and NPC hulls (no pilot) read `None` [A].
fn beacon_inputs(w: &Registry, ship_id: i64) -> Option<i64> {
    let pilot = pilot_of(w, ship_id)?;
    let p = w.roster().get(pilot).filter(|p| p.ship_id == ship_id)?;
    crate::system_effect::beacon_of(w.content(), p.solar_system_id).map(|r| r.beacon_type_id)
}

/// The pilot's `(skill type, level)` pairs, in the skill sheet's order.
fn pilot_skills(w: &Registry, ship_id: i64) -> Option<impl Iterator<Item = (i64, i64)> + '_> {
    pilot_of(w, ship_id)
        .and_then(|c| w.skills_for(Some(c)))
        .map(|s| s.entries().map(|e| (e.type_id, e.level)))
}

/// Whether the module is running as a focused HIC script.
fn focused_running(w: &Registry, module_id: i64) -> bool {
    w.modules()
        .running(module_id)
        .is_some_and(|r| crate::tackle::is_focused(&r.effect))
}

/// [`Inputs::modules`] of a ship, sorted by item id.
fn module_inputs(w: &Registry, ship_id: i64) -> Vec<ModuleInput> {
    let m = w.modules();
    let mut modules: Vec<ModuleInput> = w
        .store()
        .contents_of(ship_id)
        .filter(|i| {
            i.singleton
                && crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
        })
        .map(|i| ModuleInput {
            item_id: i.item_id,
            type_id: i.type_id,
            state: match (
                m.running(i.item_id).is_some(),
                m.is_online(i.item_id),
                crate::overheat::is_overloaded(w, ship_id, i.item_id),
            ) {
                // A field generator aimed by a focused script (HIC-SCRIPT)
                // is a point: its sphere effect does not run.
                (true, _, _) if focused_running(w, i.item_id) => State::Online,
                (true, _, true) => State::ActiveOverloaded,
                (true, _, false) => State::Active,
                (false, true, true) => State::Overloaded,
                (false, true, false) => State::Online,
                (false, false, _) => State::Offline,
            },
            charge_type: crate::modules::loaded(w.store(), ship_id, i.flag_id).map(|c| c.type_id),
        })
        .collect();
    modules.sort_by_key(|m| m.item_id);
    modules
}

/// [`Inputs::implants`] of a ship's pilot, sorted.
fn implant_types(w: &Registry, ship_id: i64) -> Vec<i64> {
    let mut implants: Vec<i64> = pilot_of(w, ship_id)
        .map(|c| {
            w.store()
                .contents_of(c)
                .filter(|i| i.flag_id == FLAG_IMPLANT)
                .map(|i| i.type_id)
                .collect()
        })
        .unwrap_or_default();
    implants.sort_unstable();
    implants
}

/// [`Inputs::boosters`] of a ship's pilot, sorted: the rows of
/// `crate::boosters` that have not run out.
fn booster_inputs(w: &Registry, ship_id: i64) -> Vec<BoosterInput> {
    pilot_of(w, ship_id)
        .map(|c| crate::boosters::inputs_of(w, c))
        .unwrap_or_default()
}

/// How many ships' sheets are kept. Beyond it the cache is emptied, which
/// costs one recomputation each.
const CACHE_CAP: usize = 4096;

/// One ship's kept sheet and what it was computed from: the inputs, and the
/// ship's live warfare buffs (K-BOOST).
type Cached = (Inputs, Vec<crate::dbuff::Applied>, Arc<Sheet>);

/// **The per-ship sheet cache**, one per world (W18 CP58c, CA7): it was a
/// process-wide static keyed on one process-wide typedata, so a second world
/// on another table would have read the first one's sheets. Behind a
/// `Mutex` because [`sheet`] reads through `&Registry`. Derived state only:
/// nothing persists, and an empty cache costs one recomputation per ship.
#[derive(Debug, Default)]
pub struct Sheets {
    cache: Mutex<HashMap<i64, Cached>>,
}

/// The registration row in `world_ext::SLOTS` (coordinator-only). `make`
/// reads nothing: the cache fills on first use.
pub const SHEETS: SlotSpec = SlotSpec {
    name: "dogma_sheets",
    make: |_| Box::new(Sheets::default()) as Boxed,
};

/// **The ship's sheet**, from the world's typedata table
/// (`typedata::installed_in(reg.content())`). `None` for a ship that is not
/// an item, or for a hull the table does not hold -- and every consumer
/// then reads its hand table as before.
///
/// Cached per ship **by its [`Inputs`]** in the world's [`Sheets`]: a sheet
/// is reused only while nothing it depends on has changed.
pub fn sheet(w: &Registry, ship_id: i64) -> Option<Arc<Sheet>> {
    let table = crate::typedata::installed_in(w.content()).table();
    let buffs = crate::dbuff::applied(w, ship_id);
    let cache = &w.ext::<Sheets>().cache;
    // **The hit is checked without building the inputs** (PERF-HOLDS): the
    // skill list is hundreds of pairs, and copying it into a new `Inputs`
    // just to compare it with the kept one was most of a `dogma::hull`
    // (about 1.5 us per moving ship per advance). The same comparison,
    // field by field; a kept entry exists only for a ship type that passed
    // the category test below, and the table is the world's own.
    let ship_type = w.store().get(ship_id).map(|i| i.type_id);
    let (modules, implants, boosters, beacon) = (
        module_inputs(w, ship_id),
        implant_types(w, ship_id),
        booster_inputs(w, ship_id),
        beacon_inputs(w, ship_id),
    );
    if let Some(ship_type) = ship_type
        && let Ok(c) = cache.lock()
        && let Some((key, held, s)) = c.get(&ship_id)
        && key.ship_type == ship_type
        && *held == buffs
        && match pilot_skills(w, ship_id) {
            Some(it) => it.eq(key.skills.iter().copied()),
            None => key.skills.is_empty(),
        }
        && key.modules == modules
        && key.implants == implants
        && key.boosters == boosters
        && key.beacon == beacon
    {
        return Some(Arc::clone(s));
    }
    let inputs = inputs(w, ship_id)?;
    // Ships only: a drone or a wreck is an item with a type in the table
    // too, and keeps its own tables.
    if table.get(inputs.ship_type)?.category_id != crate::typedata::CATEGORY_SHIP {
        return None;
    }
    // The ship's live warfare buffs are part of the key (K-BOOST).
    let s = Arc::new(evaluate_with(table, &inputs, &buffs)?);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= CACHE_CAP {
            c.clear();
        }
        c.insert(ship_id, (inputs, buffs, Arc::clone(&s)));
    }
    Some(s)
}

/// The ship's [`crate::destiny::hull::Hull`] with every dogma-modified field
/// replaced: what `destiny`, `space_state`, the capacitor and the served
/// attributes read instead of the hand table's unfitted numbers.
pub fn hull(w: &Registry, ship_id: i64) -> Option<crate::destiny::hull::Hull> {
    // A drone flies with its pilot's skills (K-DRONE).
    if let Some(h) = crate::drones::hull_of(w, ship_id) {
        return Some(h);
    }
    // A fighter squadron flies on its type's numbers (`crate::fighters`).
    if let Some(h) = crate::fighters::hull_of(w, ship_id) {
        return Some(h);
    }
    let s = sheet(w, ship_id)?;
    let mut h = hull_from(w.content(), &s);
    // An NPC web on the ship (W18 CP49): 1.0 unwebbed.
    h.max_velocity = (f64::from(h.max_velocity)
        * crate::npc_ewar::speed_factor(w, ship_id)
        * crate::ewar::speed_factor(w, ship_id)) as f32;
    // A target painter on the ship (K-EWAR): 1.0 unpainted.
    h.signature_radius *= crate::ewar::signature_factor(w, ship_id);
    Some(h)
}

/// [`hull`] from a sheet already in hand, over the content's hull for the
/// fields dogma does not model (`destiny::hull::of_in`). Pure.
pub fn hull_from(content: &crate::content::Content, s: &Sheet) -> crate::destiny::hull::Hull {
    let mut h = *crate::destiny::hull::of_in(content, s.ship_type);
    h.type_id = s.ship_type;
    h.mass = s.ship(attr::MASS);
    h.max_velocity = s.ship(attr::MAX_VELOCITY) as f32;
    h.agility = s.ship(attr::AGILITY) as f32;
    h.shield_capacity = s.ship(attr::SHIELD_CAPACITY);
    h.shield_recharge_rate = s.ship(attr::SHIELD_RECHARGE_RATE);
    h.armor_hp = s.ship(attr::ARMOR_HP);
    h.hp = s.ship(attr::HP);
    h.max_target_range = s.ship(attr::MAX_TARGET_RANGE);
    h.max_locked_targets = s.ship(attr::MAX_LOCKED_TARGETS);
    h.scan_resolution = s.ship(attr::SCAN_RESOLUTION);
    h.signature_radius = s.ship(attr::SIGNATURE_RADIUS);
    h.warp_speed_au = s.ship(attr::BASE_WARP_SPEED) * s.ship(attr::WARP_SPEED_MULTIPLIER);
    h.capacitor_capacity = s.ship(attr::CAPACITOR_CAPACITY);
    h.capacitor_recharge_rate = s.ship(attr::RECHARGE_RATE);
    h
}

/// The ship's damage profile -- HP and all twelve resonances, modified.
pub fn profile_from(s: &Sheet) -> crate::damage::Profile {
    use crate::damage::{Damage, attr as d};
    let layer = |ids: [i64; 4]| Damage {
        em: s.ship(ids[0]),
        explosive: s.ship(ids[1]),
        kinetic: s.ship(ids[2]),
        thermal: s.ship(ids[3]),
    };
    crate::damage::Profile {
        shield_capacity: s.ship(attr::SHIELD_CAPACITY),
        shield_recharge_rate: s.ship(attr::SHIELD_RECHARGE_RATE),
        armor_hp: s.ship(attr::ARMOR_HP),
        hp: s.ship(attr::HP),
        shield_resonance: layer(d::SHIELD_RESONANCE),
        armor_resonance: layer(d::ARMOR_RESONANCE),
        hull_resonance: layer(d::HULL_RESONANCE),
    }
}

/// [`profile_from`] for a ship in the registry.
pub fn profile(w: &Registry, ship_id: i64) -> Option<crate::damage::Profile> {
    // A drone's shield, armour and hull points carry its pilot's Drone
    // Durability (K-DRONE).
    if let Some(p) = crate::drones::profile_of(w, ship_id) {
        return Some(p);
    }
    // A fighter squadron's is one fighter's (`crate::fighters`, FT-E).
    if let Some(p) = crate::fighters::profile_of(w, ship_id) {
        return Some(p);
    }
    sheet(w, ship_id).map(|s| profile_from(&s))
}

/// One fitted module's attribute, modified.
pub fn module_attribute(
    w: &Registry,
    ship_id: i64,
    module_id: i64,
    attribute_id: i64,
) -> Option<f64> {
    sheet(w, ship_id)?.module(module_id, attribute_id)
}

#[cfg(test)]
mod sweep;

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture in `tools/typedata.py`'s format. **Not the client's
    /// numbers**: ids from 980,000,000 and round values, so every expected
    /// number below is the client's *formula* applied by hand to this data.
    /// The modifier shapes are the real ones (`docs/lanes/dogma-engine.md`
    /// §2 lists the effects they are modelled on).
    const FIXTURE: &str = "\
attribute\t4\t0.0\tmass
attribute\t9\t0.0\thp
attribute\t20\t1.0\tspeedFactor
attribute\t30\t0.0\tpower
attribute\t37\t0.0\tmaxVelocity
attribute\t48\t0.0\tcpuOutput
attribute\t50\t0.0\tcpu
attribute\t51\t0.0\tspeed
attribute\t64\t1.0\tdamageMultiplier
attribute\t113\t1.0\temDamageResonance
attribute\t182\t0.0\trequiredSkill1
attribute\t204\t1.0\tspeedMultiplier
attribute\t265\t0.0\tarmorHP
attribute\t267\t1.0\tarmorEmDamageResonance
attribute\t270\t1.0\tarmorThermalDamageResonance
attribute\t280\t0.0\tskillLevel
attribute\t292\t0.0\tdamageMultiplierBonus
attribute\t424\t0.0\tcpuOutputBonus2
attribute\t460\t0.0\tshipBonusMF
attribute\t552\t100.0\tsignatureRadius
attribute\t554\t0.0\tsignatureRadiusBonus
attribute\t567\t0.0\tspeedBoostFactor
attribute\t796\t0.0\tmassAddition
attribute\t1159\t0.0\tarmorHPBonusAdd
attribute\t1527\t1.0\tarmorMaxDamageResonance
attribute\t2033\t1000000.0\tspeedLimit
attribute\t2112\t1.0\tsensorDampenerResistance
attribute\t2113\t1.0\tweaponDisruptionResistance
attribute\t2114\t1.0\ttargetPainterResistance
attribute\t2115\t1.0\tstasisWebifierResistance
attribute\t2135\t1.0\tremoteAssistanceImpedance
attrmeta\t4\t0\t1\t0\t0\t9
attrmeta\t37\t0\t1\t2033\t0\t4
attrmeta\t48\t1\t1\t0\t0\t4
attrmeta\t51\t0\t0\t0\t0\t4
attrmeta\t64\t0\t1\t0\t0\t5
attrmeta\t265\t1\t1\t0\t0\t4
attrmeta\t267\t0\t0\t1527\t0\t5
attrmeta\t552\t0\t0\t0\t0\t5
type\t980000001\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t9001,9002\t9=300.0;37=400.0;48=100.0;265=400.0;267=0.5;460=-10.0\tTest Frigate
type\t980000010\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9101\t292=5.0\tTest Small Turret Skill
type\t980000011\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9102\t424=5.0\tTest CPU Skill
type\t980000012\t257\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9103\t\tTest Frigate Skill
type\t980000013\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9104\t292=2.0\tTest Owner Skill
type\t980000014\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9100,9107\t292=3.0\tTest Fitting Skill
type\t980000030\t738\t20\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t9105\t1159=50.0\tTest Implant
type\t980000020\t55\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t12,16\t30=5.0;50=10.0;51=4000.0;64=2.0;182=980000010.0\tTest Autocannon
type\t980000021\t46\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t6731\t13,16,6731\t20=100.0;567=1000000.0;796=500000.0\tTest Afterburner
type\t980000022\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t11,16,9200\t267=0.8\tTest Damage Control
type\t980000023\t59\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t11,16,9201\t204=0.9;64=1.1\tTest Gyrostabilizer
type\t980000024\t329\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t11,16,9202\t1159=100.0;796=50000.0\tTest Plate
type\t980000025\t46\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t6730\t13,16,6730\t20=500.0;567=1000000.0;796=500000.0;554=500.0\tTest MWD
type\t980000027\t589\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t12,16,2152\t20=-100.0\tTest Portal Generator
type\t980000026\t650\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t4921\t12,16,4921\t73=12000.0;973=400.0\tTest MJD
effect\t11\t0\t0\t0\t0\t0\t0\t0\tloPower\t
effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t
effect\t13\t0\t0\t0\t0\t0\t0\t0\tmedPower\t
effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t
effect\t6730\t1\t0\t73\t6\t0\t0\t0\tmoduleBonusMicrowarpdrive\teffects.MicroWarpDrive
effect\t6731\t1\t0\t73\t6\t0\t0\t0\tmoduleBonusAfterburner\teffects.Afterburner
effect\t2152\t1\t0\t1002\t6\t0\t0\t0\tjumpPortalGeneration\teffects.JumpPortal
effect\t4921\t1\t0\t73\t0\t0\t0\t0\tmicroJumpDrive\teffects.MicroJumpDriveEngage
effect\t9001\t0\t0\t0\t0\t0\t0\t0\tshipBonusROF\t
effect\t9002\t0\t0\t0\t0\t0\t0\t0\tshipNothing\t
effect\t9100\t0\t0\t0\t0\t0\t0\t0\tskillEffect\t
effect\t9101\t0\t0\t0\t0\t0\t0\t0\tturretSkill\t
effect\t9102\t0\t0\t0\t0\t0\t0\t0\tcpuSkill\t
effect\t9103\t0\t0\t0\t0\t0\t0\t0\tfrigateSkill\t
effect\t9104\t0\t0\t0\t0\t0\t0\t0\townerSkill\t
effect\t9105\t0\t0\t0\t0\t0\t0\t0\timplantEffect\t
effect\t9107\t0\t0\t0\t0\t0\t0\t0\tfittingSkill\t
effect\t9200\t4\t0\t0\t0\t0\t0\t0\tdamageControl\t
effect\t9201\t4\t0\t0\t0\t0\t0\t0\tgyro\t
effect\t9202\t4\t0\t0\t0\t0\t0\t0\tplate\t
modifier\t2152\tshipID\tItemModifier\t37\t20\t6\t\t
modifier\t9001\tshipID\tLocationRequiredSkillModifier\t51\t460\t6\t980000010\t
modifier\t9100\titemID\tItemModifier\t280\t276\t2\t\t
modifier\t9101\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t9101\tshipID\tLocationRequiredSkillModifier\t64\t292\t6\t980000010\t
modifier\t9102\titemID\tItemModifier\t424\t280\t0\t\t
modifier\t9102\tshipID\tItemModifier\t48\t424\t6\t\t
modifier\t9103\tshipID\tItemModifier\t460\t280\t0\t\t
modifier\t9104\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t9107\titemID\tItemModifier\t292\t280\t0\t\t
modifier\t9104\tcharID\tOwnerRequiredSkillModifier\t64\t292\t6\t980000010\t
modifier\t9105\tshipID\tItemModifier\t265\t1159\t2\t\t
modifier\t9105\tcharID\tItemModifier\t212\t1159\t2\t\t
modifier\t9107\tshipID\tLocationModifier\t30\t292\t6\t\t
modifier\t9107\tshipID\tLocationGroupModifier\t50\t292\t6\t\t55
modifier\t9200\tshipID\tItemModifier\t267\t267\t0\t\t
modifier\t9201\tshipID\tLocationGroupModifier\t51\t204\t4\t\t55
modifier\t9201\tshipID\tLocationGroupModifier\t64\t64\t4\t\t55
modifier\t9202\tshipID\tItemModifier\t265\t1159\t2\t\t
modifier\t9202\tshipID\tItemModifier\t4\t796\t2\t\t
";

    const FRIGATE: i64 = 980_000_001;
    const TURRET_SKILL: i64 = 980_000_010;
    const CPU_SKILL: i64 = 980_000_011;
    const FRIGATE_SKILL: i64 = 980_000_012;
    const SHIP_BONUS_MF: i64 = 460;
    const OWNER_SKILL: i64 = 980_000_013;
    const FITTING_SKILL: i64 = 980_000_014;
    const IMPLANT: i64 = 980_000_030;
    const GUN: i64 = 980_000_020;
    const AB: i64 = 980_000_021;
    const DC: i64 = 980_000_022;
    const GYRO: i64 = 980_000_023;
    const PLATE: i64 = 980_000_024;
    const MWD: i64 = 980_000_025;
    const MJD: i64 = 980_000_026;
    const PORTAL: i64 = 980_000_027;

    fn table() -> Table {
        Table::parse(FIXTURE).expect("fixture parses")
    }

    fn module(item_id: i64, type_id: i64, state: State) -> ModuleInput {
        ModuleInput {
            item_id,
            type_id,
            state,
            charge_type: None,
        }
    }

    fn fit(skills: &[(i64, i64)], modules: Vec<ModuleInput>) -> Sheet {
        evaluate(
            &table(),
            &Inputs {
                ship_type: FRIGATE,
                skills: skills.to_vec(),
                modules,
                ..Inputs::default()
            },
        )
        .expect("the frigate is in the fixture")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1.0)
    }

    /// A running jump portal generator (no default effect in the data) holds
    /// the ship still: `maxVelocity` post-percent by its `speedFactor` -100.
    #[test]
    fn a_running_portal_generator_holds_the_ship_still() {
        let free = fit(&[], vec![module(1, PORTAL, State::Online)]);
        assert_eq!(free.ship(attr::MAX_VELOCITY), 400.0);
        let held = fit(&[], vec![module(1, PORTAL, State::Active)]);
        assert_eq!(held.ship(attr::MAX_VELOCITY), 0.0);
    }

    #[test]
    fn the_operators_are_the_clients_lambdas() {
        assert_eq!(operate(op::PRE_ASSIGN, 5.0, 2.0), 2.0);
        assert_eq!(operate(op::POST_ASSIGN, 5.0, 2.0), 2.0);
        assert_eq!(operate(op::PRE_MUL, 5.0, 2.0), 10.0);
        assert_eq!(operate(op::POST_DIV, 5.0, 2.0), 2.5);
        assert_eq!(operate(op::MOD_ADD, 5.0, 2.0), 7.0);
        assert_eq!(operate(op::MOD_SUB, 5.0, 2.0), 3.0);
        assert_eq!(operate(op::POST_PERCENT, 200.0, -10.0), 180.0);
        assert_eq!(operate(op::POST_PERCENT_UNNERFED, 200.0, 50.0), 300.0);
    }

    #[test]
    fn precedence_is_the_clients_not_the_input_order() {
        // Added after the pre-multiply, before the post-multiply.
        let v = apply(
            10.0,
            &[
                Incoming {
                    op: op::POST_MUL,
                    value: 3.0,
                    penalised: false,
                },
                Incoming {
                    op: op::MOD_ADD,
                    value: 5.0,
                    penalised: false,
                },
                Incoming {
                    op: op::PRE_MUL,
                    value: 2.0,
                    penalised: false,
                },
            ],
        );
        assert_eq!(v, (10.0 * 2.0 + 5.0) * 3.0);
        // Post-assign beats everything; pre-assign is overwritten by what
        // follows it.
        let v = apply(
            10.0,
            &[
                Incoming {
                    op: op::POST_ASSIGN,
                    value: 7.0,
                    penalised: false,
                },
                Incoming {
                    op: op::POST_PERCENT_UNNERFED,
                    value: 100.0,
                    penalised: false,
                },
            ],
        );
        assert_eq!(v, 7.0);
    }

    #[test]
    fn the_stacking_penalty_is_the_clients_series() {
        let nerf = |values: &[f64]| {
            let mods: Vec<Incoming> = values
                .iter()
                .map(|v| Incoming {
                    op: op::POST_MUL,
                    value: *v,
                    penalised: true,
                })
                .collect();
            apply(1.0, &mods)
        };
        // One: untouched (denominator exp(0) = 1).
        assert!(close(nerf(&[1.1]), 1.1));
        // Two bonuses: the larger first at full strength.
        let want = 1.2 * (1.0 + 0.1 / (1.0f64 / 2.67).powi(2).exp());
        assert!(close(nerf(&[1.1, 1.2]), want));
        // Bonuses and penalties are two separate series.
        let want = 1.2 * 0.5;
        assert!(close(nerf(&[1.2, 0.5]), want));
        // Two penalties: the strongest (smallest factor) first.
        let want = 0.5 * (1.0 + (0.8 - 1.0) / nurf_denominator(1));
        assert!(close(nerf(&[0.8, 0.5]), want));
        // Only eight of a kind count: a ninth identical bonus changes nothing.
        assert!(close(nerf(&[1.1; 9]), nerf(&[1.1; 8])));
        assert!(nerf(&[1.1; 8]) > nerf(&[1.1; 7]));
    }

    #[test]
    fn an_unfitted_hull_is_its_type() {
        let s = fit(&[], vec![]);
        assert_eq!(s.ship(attr::MAX_VELOCITY), 400.0);
        assert_eq!(s.ship(attr::CPU_OUTPUT), 100.0);
        assert_eq!(s.ship(attr::ARMOR_HP), 400.0);
        // Mass from the `types` column, not `typedogma`.
        assert_eq!(s.ship(attr::MASS), 1_000_000.0);
        // Not on the type: the attribute's default.
        assert_eq!(s.ship(attr::SIGNATURE_RADIUS), 100.0);
    }

    #[test]
    fn a_skill_scales_by_its_level_and_reaches_the_ship() {
        // cpuOutputBonus2 5 x level 4 = 20 -> cpuOutput +20%.
        let s = fit(&[(CPU_SKILL, 4)], vec![]);
        assert!(close(s.ship(attr::CPU_OUTPUT), 120.0));
        // And no level, no bonus.
        let s = fit(&[(CPU_SKILL, 0)], vec![]);
        assert_eq!(s.ship(attr::CPU_OUTPUT), 100.0);
    }

    #[test]
    fn a_skill_bonus_reaches_only_modules_requiring_it() {
        let gun = module(1, GUN, State::Online);
        let ab = module(2, AB, State::Online);
        let s = fit(&[(TURRET_SKILL, 5)], vec![gun, ab]);
        // damageMultiplierBonus 5 x 5 = 25% on the gun, which requires it.
        assert!(close(s.module(1, attr::DAMAGE_MULTIPLIER).unwrap(), 2.5));
        // The afterburner does not require the skill.
        assert_eq!(s.module(2, attr::DAMAGE_MULTIPLIER), Some(1.0));
    }

    #[test]
    fn a_ship_bonus_is_scaled_by_the_hull_skill_and_aimed_by_required_skill() {
        // shipBonusMF -10 x level 3 = -30% on `speed` of modules requiring
        // the turret skill.
        let s = fit(&[(FRIGATE_SKILL, 3)], vec![module(1, GUN, State::Online)]);
        assert!(close(s.module(1, attr::SPEED).unwrap(), 4000.0 * 0.7));
        // Without the hull skill the bonus is the type's own -10, unscaled:
        // nothing multiplies it.
        let s = fit(&[], vec![module(1, GUN, State::Online)]);
        assert!(close(s.module(1, attr::SPEED).unwrap(), 4000.0 * 0.9));
    }

    #[test]
    fn online_effects_need_online_and_offline_does_nothing() {
        let s = fit(&[], vec![module(1, PLATE, State::Offline)]);
        assert_eq!(s.ship(attr::ARMOR_HP), 400.0);
        let s = fit(&[], vec![module(1, PLATE, State::Online)]);
        assert_eq!(s.ship(attr::ARMOR_HP), 500.0);
        assert_eq!(s.ship(attr::MASS), 1_050_000.0);
    }

    #[test]
    fn a_damage_control_premultiplies_a_resonance_clamped_to_its_max() {
        let s = fit(&[], vec![module(1, DC, State::Online)]);
        assert!(close(s.ship(267), 0.5 * 0.8));
    }

    #[test]
    fn modules_stack_with_a_penalty_skills_and_ships_without() {
        // Three gyros: damageMultiplier x1.1 each, penalised.
        let gyros: Vec<ModuleInput> = (1..=3).map(|i| module(i, GYRO, State::Online)).collect();
        let mut mods = gyros.clone();
        mods.push(module(10, GUN, State::Online));
        let s = fit(&[(TURRET_SKILL, 5)], mods);
        let stacked = (0..3)
            .map(|i| 1.0 + 0.1 / nurf_denominator(i))
            .product::<f64>();
        // The skill's +25% is post-percent from a skill: never penalised.
        let want = 2.0 * 1.25 * stacked;
        assert!(close(s.module(10, attr::DAMAGE_MULTIPLIER).unwrap(), want));
        // Rate of fire: speedMultiplier 0.9 three times, also penalised.
        let rof = (0..3)
            .map(|i| 1.0 - 0.1 / nurf_denominator(i))
            .product::<f64>();
        assert!(close(
            s.module(10, attr::SPEED).unwrap(),
            4000.0 * 0.9 * rof
        ));
    }

    #[test]
    fn an_afterburner_runs_the_clients_python_formula() {
        let idle = fit(&[], vec![module(1, AB, State::Online)]);
        assert_eq!(idle.ship(attr::MAX_VELOCITY), 400.0);
        assert_eq!(idle.ship(attr::MASS), 1_000_000.0);
        let on = fit(&[], vec![module(1, AB, State::Active)]);
        let mass = 1_000_000.0 + 500_000.0;
        assert_eq!(on.ship(attr::MASS), mass);
        let want = 400.0 * (1.0 + 0.01 * 100.0 * 1_000_000.0 / mass);
        assert!(close(on.ship(attr::MAX_VELOCITY), want));
    }

    #[test]
    fn a_microwarpdrive_also_blooms_the_signature() {
        let on = fit(&[], vec![module(1, MWD, State::Active)]);
        assert!(close(on.ship(attr::SIGNATURE_RADIUS), 100.0 * 6.0));
        let mass = 1_500_000.0;
        let want = 400.0 * (1.0 + 0.01 * 500.0 * 1_000_000.0 / mass);
        assert!(close(on.ship(attr::MAX_VELOCITY), want));
    }

    #[test]
    fn a_micro_jump_drive_blooms_the_signature_while_it_spools() {
        // `BaseMicroJumpDrive`: signatureRadius post-percent by attribute
        // 973, and nothing else (no mass, no speed).
        let idle = fit(&[], vec![module(1, MJD, State::Online)]);
        assert!(close(idle.ship(attr::SIGNATURE_RADIUS), 100.0));
        let on = fit(&[], vec![module(1, MJD, State::Active)]);
        assert!(close(on.ship(attr::SIGNATURE_RADIUS), 100.0 * 5.0));
        assert_eq!(on.ship(attr::MASS), 1_000_000.0);
        assert_eq!(on.ship(attr::MAX_VELOCITY), 400.0);
    }

    #[test]
    fn two_propulsion_literals_are_penalised_as_module_effects() {
        let both = fit(
            &[],
            vec![module(1, AB, State::Active), module(2, MWD, State::Active)],
        );
        let mass = 1_000_000.0 + 2.0 * 500_000.0;
        let mut f: [f64; 2] = [
            1.0 + 0.01 * 100.0 * 1_000_000.0 / mass,
            1.0 + 0.01 * 500.0 * 1_000_000.0 / mass,
        ];
        f.sort_by(|a, b| b.total_cmp(a));
        let want = 400.0 * f[0] * (1.0 + (f[1] - 1.0) / nurf_denominator(1));
        assert!(close(both.ship(attr::MAX_VELOCITY), want));
    }

    #[test]
    fn the_cpu_load_sums_online_modules_only() {
        let s = fit(
            &[],
            vec![
                module(1, GUN, State::Online),
                module(2, GUN, State::Offline),
                module(3, GUN, State::Active),
            ],
        );
        let inputs = Inputs {
            ship_type: FRIGATE,
            skills: vec![],
            modules: vec![
                module(1, GUN, State::Online),
                module(2, GUN, State::Offline),
                module(3, GUN, State::Active),
            ],
            ..Inputs::default()
        };
        assert_eq!(s.load(&inputs, None), (20.0, 10.0));
        assert_eq!(s.load(&inputs, Some(1)), (10.0, 5.0));
    }

    /// EWAR-2B/2C: a hull with no row for the impedances reads their default
    /// 1, not 0 (which would make it immune to every disruptor and refuse
    /// every remote tracking computer).
    #[test]
    fn a_hull_with_no_impedance_row_reads_the_default() {
        let s = fit(&[], vec![]);
        assert_eq!(s.ship(2113), 1.0);
        assert_eq!(s.ship(2135), 1.0);
        // The resistances a module names in its own 2138: a hull with no row
        // for 2114 (the Rifter) must not read 0, which refused every target
        // painter as DeniedActivateTargetModuleDisallowed (fci_ewar).
        for named in [2112, 2114, 2115] {
            assert_eq!(s.ship(named), 1.0, "attribute {named}");
        }
    }

    #[test]
    fn a_hull_the_table_does_not_hold_has_no_sheet() {
        let none = evaluate(
            &table(),
            &Inputs {
                ship_type: 587,
                ..Inputs::default()
            },
        );
        assert!(none.is_none());
    }

    #[test]
    fn the_sheet_becomes_a_hull_and_a_profile() {
        let s = fit(
            &[],
            vec![
                module(1, PLATE, State::Online),
                module(2, DC, State::Online),
            ],
        );
        let h = hull_from(&crate::content::Content::fixture(), &s);
        assert_eq!(h.type_id, FRIGATE);
        assert_eq!(h.armor_hp, 500.0);
        assert_eq!(h.mass, 1_050_000.0);
        assert_eq!(h.max_velocity, 400.0);
        let p = profile_from(&s);
        assert_eq!(p.armor_hp, 500.0);
        assert!(close(p.armor_resonance.em, 0.4));
        assert_eq!(p.armor_resonance.thermal, 1.0);
    }

    /// A world whose typedata table is `text`, with one assembled
    /// [`FRIGATE`] in the first character's hangar.
    fn world_on(text: &str) -> (Registry, i64) {
        let mut content = crate::content::Content::fixture();
        content.insert(
            crate::typedata::TABLE.name,
            crate::typedata::Installed::build(Table::parse(text).expect("parses")),
        );
        let mut reg =
            Registry::new(crate::model::SandboxConfig::default()).with_content(Arc::new(content));
        let c = reg.config().characters[0].clone();
        let ship = 100_000_901;
        let mut items = reg.store().items().to_vec();
        items.push(crate::item::Item {
            item_id: ship,
            type_id: FRIGATE,
            owner_id: c.character_id,
            location_id: c.station_id.expect("docked"),
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: 25,
            category_id: crate::typedata::CATEGORY_SHIP,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: ship + 1,
            ..crate::store::Snapshot::default()
        });
        (reg, ship)
    }

    /// CP58c (CA7): the sheet is read from the world's own table and cached
    /// in the world's own [`Sheets`] slot. Two worlds with the same ship id
    /// and the same inputs but different tables each get their own table's
    /// answer -- a process-wide cache keyed on the inputs alone would have
    /// handed the second world the first one's sheet. A repeat read in one
    /// world is the cached sheet, and a world whose table lacks the hull has
    /// none.
    #[test]
    fn the_sheet_cache_is_per_world_and_reads_the_worlds_table() {
        let other = FIXTURE.replace("37=400.0", "37=300.0");
        assert_ne!(other, FIXTURE, "the variant changes the hull's speed");
        let (a, ship) = world_on(FIXTURE);
        let (b, ship_b) = world_on(&other);
        assert_eq!(ship, ship_b);
        let inputs_a = inputs(&a, ship).expect("an item");
        assert_eq!(inputs_a, inputs(&b, ship).expect("an item"));

        let sa = sheet(&a, ship).expect("the hull is in a's table");
        let sb = sheet(&b, ship).expect("the hull is in b's table");
        assert_eq!(*sa, evaluate(&table(), &inputs_a).expect("evaluates"));
        let tb = Table::parse(&other).expect("parses");
        assert_eq!(*sb, evaluate(&tb, &inputs_a).expect("evaluates"));
        assert_ne!(sa.ship(attr::MAX_VELOCITY), sb.ship(attr::MAX_VELOCITY));
        assert!(Arc::ptr_eq(&sa, &sheet(&a, ship).expect("cached")));

        // The default fixture world: its table does not hold FRIGATE.
        let (mut c, ship_c) = world_on(FIXTURE);
        c = c.with_content(Arc::new(crate::content::Content::fixture()));
        assert!(sheet(&c, ship_c).is_none());
    }

    // ---- GAP-BRAIN: the brain view ----------------------------------------

    fn brain(skills: &[(i64, i64)], implants: &[i64]) -> Vec<BrainMod> {
        brain_mods(&table(), skills, implants, &[])
    }

    #[test]
    fn a_ship_cpu_bonus_skill_becomes_one_ship_item_mod_with_the_level_scaled_value() {
        // cpuOutputBonus2 5 x level 4 = 20; the skill's own plumbing
        // (skillLevel onto itself) is not in the brain.
        assert_eq!(
            brain(&[(CPU_SKILL, 4)], &[]),
            vec![BrainMod {
                source_type: CPU_SKILL,
                target: BrainTarget::ShipItem,
                op: op::POST_PERCENT,
                attr: attr::CPU_OUTPUT,
                value: 20.0,
            }]
        );
    }

    #[test]
    fn a_self_modifier_on_the_skill_and_a_do_nothing_level_are_not_in_the_brain() {
        // Level 0: +0% changes nothing, so it is not sent.
        assert!(brain(&[(CPU_SKILL, 0)], &[]).is_empty());
        // But a x0 is not nothing: the sheet zeroes the hull bonus of a
        // level-0 hull skill, so the brain must too.
        let m = brain(&[(FRIGATE_SKILL, 0)], &[]);
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].op, m[0].attr, m[0].value),
            (op::PRE_MUL, SHIP_BONUS_MF, 0.0)
        );
    }

    #[test]
    fn a_location_required_skill_modifier_becomes_a_ship_skill_mod() {
        let m = brain(&[(TURRET_SKILL, 5)], &[]);
        assert_eq!(
            m,
            vec![BrainMod {
                source_type: TURRET_SKILL,
                target: BrainTarget::ShipSkill(TURRET_SKILL),
                op: op::POST_PERCENT,
                attr: attr::DAMAGE_MULTIPLIER,
                value: 25.0,
            }]
        );
    }

    #[test]
    fn an_owner_required_skill_modifier_goes_to_the_character() {
        let m = brain(&[(OWNER_SKILL, 3)], &[]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].target, BrainTarget::CharOwnerSkill(TURRET_SKILL));
        assert_eq!(m[0].value, 6.0);
    }

    #[test]
    fn location_and_group_modifiers_keep_their_kind() {
        let m = brain(&[(FITTING_SKILL, 2)], &[]);
        assert_eq!(m.len(), 2);
        assert!(
            m.iter()
                .any(|b| b.target == BrainTarget::ShipLocation && b.attr == attr::POWER)
        );
        assert!(
            m.iter()
                .any(|b| b.target == BrainTarget::ShipGroup(55) && b.attr == attr::CPU)
        );
        assert!(m.iter().all(|b| b.value == 6.0));
    }

    #[test]
    fn an_implant_reaches_both_the_ship_and_the_pilot() {
        let m = brain(&[], &[IMPLANT]);
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|b| b.target == BrainTarget::ShipItem
            && b.attr == attr::ARMOR_HP
            && b.op == op::MOD_ADD
            && b.value == 50.0
            && b.source_type == IMPLANT));
        assert!(
            m.iter()
                .any(|b| b.target == BrainTarget::CharItem && b.attr == 212)
        );
    }

    /// **The client's arithmetic over the brain is the server's sheet.** The
    /// "client" here is the ship's base value, the brain's modifiers aimed at
    /// it (never penalised, like the brain literals) and, for a module, the
    /// hull's own bonus working off the brain-modified ship attribute.
    #[test]
    fn the_brain_applied_to_a_hull_gives_the_numbers_of_the_sheet() {
        let t = table();
        let skills = [
            (TURRET_SKILL, 5),
            (CPU_SKILL, 4),
            (FRIGATE_SKILL, 3),
            (FITTING_SKILL, 2),
        ];
        let sheet = fit(&skills, vec![module(1, GUN, State::Online)]);
        let mods = brain(&skills, &[]);
        let hull = t.get(FRIGATE).expect("hull");
        let client_ship = |a: i64| {
            let incoming: Vec<Incoming> = mods
                .iter()
                .filter(|m| m.target == BrainTarget::ShipItem && m.attr == a)
                .map(|m| Incoming {
                    op: m.op,
                    value: m.value,
                    penalised: false,
                })
                .collect();
            let base = hull.attribute(a).unwrap_or_else(|| t.default_value(a));
            apply(base, &incoming)
        };
        // Ship attributes the skills reach.
        let mut reached = 0;
        for m in mods.iter().filter(|m| m.target == BrainTarget::ShipItem) {
            reached += 1;
            let want = sheet.ship(m.attr);
            assert!(close(client_ship(m.attr), want), "attr {}", m.attr);
        }
        assert!(reached >= 2, "cpu output and the hull bonus attribute");
        assert!(close(client_ship(attr::CPU_OUTPUT), 120.0));
        // The gun: its damage from the turret skill (LRS), its rate of fire
        // from the hull bonus the frigate skill scaled (460 -> speed).
        let gun = t.get(GUN).expect("gun");
        let damage = apply(
            gun.attribute(attr::DAMAGE_MULTIPLIER).expect("damage"),
            &mods
                .iter()
                .filter(|m| {
                    m.target == BrainTarget::ShipSkill(TURRET_SKILL)
                        && m.attr == attr::DAMAGE_MULTIPLIER
                })
                .map(|m| Incoming {
                    op: m.op,
                    value: m.value,
                    penalised: false,
                })
                .collect::<Vec<_>>(),
        );
        assert!(close(
            damage,
            sheet.module(1, attr::DAMAGE_MULTIPLIER).expect("gun")
        ));
        let speed = apply(
            gun.attribute(attr::SPEED).expect("speed"),
            &[Incoming {
                op: op::POST_PERCENT,
                value: client_ship(SHIP_BONUS_MF),
                penalised: false,
            }],
        );
        assert!(close(speed, sheet.module(1, attr::SPEED).expect("gun")));
    }

    /// The numbers the owner asked about: how big the brain is and what it
    /// costs to build, for a pilot with every skill of the table at level 5.
    /// Printed, not asserted on time (a loaded gate would flake it).
    #[test]
    fn a_pilot_with_every_skill_has_a_small_brain_built_fast() {
        let t = table();
        let skills: Vec<(i64, i64)> = [
            TURRET_SKILL,
            CPU_SKILL,
            FRIGATE_SKILL,
            OWNER_SKILL,
            FITTING_SKILL,
        ]
        .iter()
        .map(|s| (*s, 5))
        .collect();
        let start = std::time::Instant::now();
        let mods = brain_mods(&t, &skills, &[IMPLANT], &[]);
        eprintln!("brain_mods: {} mods in {:?}", mods.len(), start.elapsed());
        assert!(!mods.is_empty());
    }

    // ---- SYS-2: the system's effect beacon. Invented ids and values; the
    // beacon type 990101 is the one the system-effects fixture names for the
    // system 990001. ----

    const BEACON: i64 = 990_101;
    const CHARGE: i64 = 980_000_040;

    /// The base fixture plus a beacon with one modifier of each shape, and a
    /// charge that requires the turret skill.
    fn beacon_table() -> Table {
        let extra = "\
type\t990101\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t99300\t99401=0.5;99402=50.0;99403=2.0;99404=0.9;99405=3.0\tTest Beacon
type\t980000040\t85\t8\t1\t1\t0.0\t0.0\t0.0\t0.1\t0\t\t64=4.0;182=980000010.0\tTest Charge
effect\t99300\t7\t0\t0\t0\t0\t0\t0\ttestBeaconEffect\t
modifier\t99300\tshipID\tItemModifier\t37\t99401\t4\t\t
modifier\t99300\tshipID\tItemModifier\t552\t99402\t6\t\t
modifier\t99300\tshipID\tLocationGroupModifier\t64\t99403\t4\t\t55
modifier\t99300\tshipID\tLocationRequiredSkillModifier\t51\t99404\t4\t980000010\t
modifier\t99300\tcharID\tOwnerRequiredSkillModifier\t64\t99405\t4\t980000010\t
";
        Table::parse(&format!("{FIXTURE}{extra}")).expect("the beacon fixture parses")
    }

    fn fit_in(beacon: Option<i64>, modules: Vec<ModuleInput>) -> Sheet {
        evaluate(
            &beacon_table(),
            &Inputs {
                ship_type: FRIGATE,
                modules,
                beacon,
                ..Inputs::default()
            },
        )
        .expect("the frigate is in the fixture")
    }

    #[test]
    fn a_beacon_multiplies_and_percents_the_ship_and_changes_nothing_else() {
        let plain = fit_in(None, vec![]);
        let hot = fit_in(Some(BEACON), vec![]);
        assert!(close(hot.ship(attr::MAX_VELOCITY), 400.0 * 0.5));
        assert!(close(hot.ship(attr::SIGNATURE_RADIUS), 100.0 * 1.5));
        // Untouched attributes read as they did.
        assert_eq!(hot.ship(9), plain.ship(9));
        assert_eq!(hot.ship(4), plain.ship(4));
    }

    #[test]
    fn no_beacon_and_a_beacon_the_table_does_not_hold_leave_the_sheet_identical() {
        let modules = vec![module(1, GUN, State::Online)];
        let plain = fit_in(None, modules.clone());
        assert_eq!(fit_in(Some(990_999), modules.clone()), plain);
        // The same inputs without the field are the unmodified fixture sheet.
        assert_eq!(
            plain,
            evaluate(
                &table(),
                &Inputs {
                    ship_type: FRIGATE,
                    modules,
                    ..Inputs::default()
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn group_and_skill_aims_reach_the_fitted_guns_and_stack_with_a_module() {
        let guns = vec![module(1, GUN, State::Online)];
        let plain = fit_in(None, guns.clone());
        let hot = fit_in(Some(BEACON), guns);
        // Group 55: the gun's damage multiplier doubles.
        assert!(close(
            hot.module(1, 64).unwrap(),
            plain.module(1, 64).unwrap() * 2.0
        ));
        // The skill aim: the gun requires the turret skill.
        assert!(close(
            hot.module(1, 51).unwrap(),
            plain.module(1, 51).unwrap() * 0.9
        ));
        // With a gyrostabilizer (x0.9 on the same attribute) the two factors
        // stack with the penalty, not as 0.81.
        let both = fit_in(
            Some(BEACON),
            vec![
                module(1, GUN, State::Online),
                module(2, GYRO, State::Online),
            ],
        );
        let gyro_only = fit_in(
            None,
            vec![
                module(1, GUN, State::Online),
                module(2, GYRO, State::Online),
            ],
        );
        let want = plain.module(1, 51).unwrap() * 0.9 * (1.0 + (0.9 - 1.0) / nurf_denominator(1));
        assert!(close(both.module(1, 51).unwrap(), want));
        assert!(both.module(1, 51).unwrap() > plain.module(1, 51).unwrap() * 0.81);
        assert!(both.module(1, 51).unwrap() < gyro_only.module(1, 51).unwrap());
    }

    #[test]
    fn an_owner_aim_reaches_a_loaded_charge_but_not_the_guns() {
        let loaded = ModuleInput {
            charge_type: Some(CHARGE),
            ..module(1, GUN, State::Online)
        };
        let plain = fit_in(None, vec![loaded.clone()]);
        let hot = fit_in(Some(BEACON), vec![loaded]);
        assert!(close(
            hot.charge(1, 64).unwrap(),
            plain.charge(1, 64).unwrap() * 3.0
        ));
        // The gun itself took only the group aim (x2), not the owner's x3.
        assert!(close(
            hot.module(1, 64).unwrap(),
            plain.module(1, 64).unwrap() * 2.0
        ));
    }

    /// `drone_skills::stats` reads a drone as a phantom charge of the ship, so the
    /// owner aim covers a drone's damage the same way.
    #[test]
    fn the_beacon_is_part_of_the_key_and_a_phantom_charge_takes_the_owner_aim() {
        let inputs = Inputs {
            ship_type: FRIGATE,
            modules: vec![ModuleInput {
                charge_type: Some(CHARGE),
                ..module(1, GUN, State::Online)
            }],
            beacon: Some(BEACON),
            ..Inputs::default()
        };
        let none = Inputs {
            beacon: None,
            ..inputs.clone()
        };
        assert_ne!(inputs, none, "the beacon is part of the cache key");
        let t = beacon_table();
        let (a, b) = (evaluate(&t, &inputs).unwrap(), evaluate(&t, &none).unwrap());
        assert!(close(
            a.charge(1, 64).unwrap(),
            b.charge(1, 64).unwrap() * 3.0
        ));
    }

    /// The world half: a ship in space in a beacon system has it, a jump
    /// changes the cached sheet, and one in a system without a row has none.
    #[test]
    fn the_sheet_follows_the_ship_into_and_out_of_a_beacon_system() {
        use crate::space_state::{Motion, Presence};
        let (mut reg, ship) = world_on(&format!(
            "{FIXTURE}{}",
            "type\t990101\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t99300\t99401=0.5\tTest Beacon\n\
             effect\t99300\t7\t0\t0\t0\t0\t0\t0\ttestBeaconEffect\t\n\
             modifier\t99300\tshipID\tItemModifier\t37\t99401\t4\t\t\n"
        ));
        let pilot = reg.config().characters[0].character_id;
        reg.roster_mut().arrive(Presence::arriving(
            pilot,
            ship,
            990_001,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert_eq!(inputs(&reg, ship).unwrap().beacon, Some(BEACON));
        let hot = sheet(&reg, ship).unwrap();
        assert!(close(hot.ship(attr::MAX_VELOCITY), 200.0));
        assert!(Arc::ptr_eq(&hot, &sheet(&reg, ship).unwrap()));

        // A jump to a system with no row.
        reg.roster_mut().arrive(Presence::arriving(
            pilot,
            ship,
            990_999,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert_eq!(inputs(&reg, ship).unwrap().beacon, None);
        assert!(close(
            sheet(&reg, ship).unwrap().ship(attr::MAX_VELOCITY),
            400.0
        ));
    }
}
