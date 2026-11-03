//! **Overload and heat** (K-OVERHEAT). Lane note `docs/lanes/k-overheat.md`;
//! marks as `docs/content/README.md` §2.
//!
//! A module with an overload effect (category 5) can be run hot: its overload
//! bonuses apply, the ship's rack warms while it cycles, and every cycle it
//! may damage the modules around it until one burns out and goes offline.
//! The client draws all of it from what the server says; the numbers are the
//! client's own and the rolls are ours.
//!
//! # What the client says [V]
//!
//! From `eve/client/script/environment/godma.pyc`, `eve/client/script/dogma/
//! clientDogmaLocation.pyc`, `dogma/attributes/heatAttribute.pyc`,
//! `dogma/items/shipDogmaItem.pyc` and the ship HUD
//! (`shipHud/{overloadBtn,heatGauges,slotsContainer,activeShipController}`,
//! `shipModuleButton/shipmodulebutton`):
//!
//! - **The calls** (`godma:2014-2067`): `dogmaIM.Overload(itemID,
//!   effectID)`, `StopOverload(itemID, effectID)`, `OverloadRack(itemID)` and
//!   `StopOverloadRack(itemID)`; the rack calls **return the module ids** the
//!   toggle acts on. Every id returned is put in `pendingOverloading` (or
//!   `pendingStopOverloading`), which only an `OnGodmaShipEffect` for that
//!   module's **overload effect** clears (`godma:1411-1420`): a module named
//!   and never told flashes for ever. So a rack call returns, and tells,
//!   exactly the modules it changed.
//! - **The effect is the client's own data**: the type's category-5 effect
//!   (`dgmEffOverload`) is in the static dogma tables, so the client also
//!   starts it locally (`clientDogmaLocation.Overload`, `StartEffect` is never
//!   user-started for it, `baseDogmaLocation.StartEffect_PreChecks`) and
//!   computes the overload bonuses itself. `OnGodmaShipEffect(itemID,
//!   overloadEffectID, ..., start, active, env, ...)` sets the effect's
//!   `isActive`, which lights the module's overload button
//!   (`ModuleButton.Update` -> `ShowOverload`) and the rack button
//!   (`CheckOverloadRackBtnState`: lit when no module of the rack is inactive).
//! - **The menu is gated by the skill on the client**: the entries show only
//!   when the pilot's Thermodynamics (`typeThermodynamics`) level is at least
//!   the module's `requiredThermoDynamicsSkill` (1212); the server's refusal
//!   is the same key the client has, `DontHaveThermoDynamicsSkill`
//!   (`skillLevel`).
//! - **Heat is computed by the client** between server words
//!   (`heatAttribute.CalculateHeat`, ported as [`calculate_heat`]): the ship's
//!   `heatHi` / `heatMed` / `heatLow` (1175-1177) climb towards the rack's
//!   `heatCapacity*` at `incoming * heatGenerationMultiplier` per second while
//!   `incoming > 0`, and decay by `exp(-t * heatDissipationRate*)` when it is
//!   zero. `incoming` is the sum of `heatAbsorbtionRateModifier` (1180) of the
//!   modules the client has been **told** are heating the rack:
//!   `OnHeatAdded(heatID, moduleID)` / `OnHeatRemoved(heatID, moduleID)`
//!   (`clientDogmaLocation:1301-1320`, `findings/push-events.tsv`). A value
//!   the server sends as an attribute change is `SetBaseValue`d
//!   (`clientDogmaLocation:339`). So the server's words are: the value, then
//!   the add or the remove; and the same value again now and then to cancel
//!   drift. The gauges read `GetHeatValues` = value / capacity per rack.
//! - **Module damage** is the module's `damage` attribute (3) in hit points
//!   (`hp` 9): `OnModuleAttributeChanges` of an item key and attribute 3 go
//!   through `OnDamageStateChanged` / `OnAttributes('damage')`, and the
//!   module's button shows `damage / hp` (`UpdateModuleDamage`,
//!   `slotsContainer.OnAttributes`). **A module whose damage reaches its `hp`
//!   cannot be put online**: `baseDogmaLocation.StartEffect_PreChecks` raises
//!   `ModuleTooDamagedToBeOnlined`.
//! - **Repair** (`godma:2130-2190`): `InitiateModuleRepair(itemID)` -> `True`
//!   or `False`, `InitiateModuleRepairMany(itemIDs)` -> `(success,
//!   missingCharges, failed)`, `StopModuleRepair(itemID)`. The client waits
//!   `damage / moduleRepairRate` minutes (character attribute 1267) and then
//!   calls `StopModuleRepair` itself. Refusals the client has keys for:
//!   `CanNotRepairWhileModuleIsOnline`, `CanNotRepairWhileModuleIsActive`
//!   (`moduleName`), `CanNotRepairWhileModulesOverloaded`,
//!   `NotEnoughRepairMaterialToFinishAllRepairs` (`typeName`: the Nanite
//!   Repair Paste, `typeNaniteRepairPaste`), and the overload refusal while a
//!   repair runs, `CanNotOverloadModuleOnShipThatHasModuleRepair`.
//!
//! # What the client does not say, so [A]
//!
//! The roll is server code. From EvEJS's facts (a different codebase, facts
//! only): a rack's heat is 0..capacity; every cycle that **ends** with an
//! overloaded module cycling rolls, for every other module of the rack,
//! `heat / capacity * attenuation^slotDistance` (the ship's
//! `heatAttenuation*`, 1259 / 1261 / 1262), and a hit does the cycling
//! module's `heatDamage` (1211) hit points. The Thermodynamics bonus (the
//! skill's own `thermodynamicsHeatDamage`, 1229, a post-percent on 1211) is in
//! the ship's sheet, so it is never applied a second time here. Each is a named
//! constant or a row of the bundle below; the lane note lists the falsifiers.
//!
//! - **[A] a toggle takes hold at once**, also on a module mid-cycle (TQ holds
//!   it to the next cycle). Falsified by the green light flashing until the
//!   cycle end on the live client.
//! - **[A] the cycle length does not change while it repeats**: the overload
//!   rate-of-fire bonus applies to a module activated while overloaded; one
//!   running when it is overloaded keeps its cycle until it is activated
//!   again. (`modules::Running::period` is fixed at activation.)
//! - **[A] heat and the overload state end with the ship's time in space**:
//!   docking, jumping and dying clear them.
//! - **[A] damage and repairs are kept in memory** (like the online state),
//!   not persisted.
//! - **[A]** the repair's paste count ([`PASTE_FOR_FULL_REPAIR`]) and the
//!   fallbacks used only when a type row lacks a number ([`DEFAULT_*`]).

use crate::damage::BlueTime;
use crate::fitting::Rack;
use crate::modules::{EFFECT_ONLINE, MSEC, ShipEffect, type_arg, type_arg_for, user_error};
use crate::notify::Push;
use crate::seed::Stream;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// `dgmEffOverload` [V].
pub const CATEGORY_OVERLOAD: i64 = 5;
/// `attributeDamage` and `attributeHp` [V].
pub const ATTR_DAMAGE: i64 = 3;
pub const ATTR_HP: i64 = 9;
/// `attributeHeatHi`, `Med`, `Low` [V] (`dogma/const.pyc`).
pub const ATTR_HEAT: [i64; 3] = [1175, 1176, 1177];
/// `attributeHeatCapacityHi/Med/Low` [V].
pub const ATTR_CAPACITY: [i64; 3] = [1178, 1199, 1200];
/// `attributeHeatDissipationRateHi/Med/Low` [V].
pub const ATTR_DISSIPATION: [i64; 3] = [1179, 1196, 1198];
/// `attributeHeatAttenuationHi/Med/Low` [V].
pub const ATTR_ATTENUATION: [i64; 3] = [1259, 1261, 1262];
/// `attributeHeatGenerationMultiplier` [V].
pub const ATTR_GENERATION: i64 = 1224;
/// `attributeHeatAbsorbtionRateModifier`: what one module adds to the rack's
/// incoming heat while it heats it [V] (`OnHeatAdded`).
pub const ATTR_HEAT_RATE: i64 = 1180;
/// `attributeHeatDamage`: hit points one heat hit does [V-data].
pub const ATTR_HEAT_DAMAGE: i64 = 1211;
/// `attributeRequiredThermoDynamicsSkill` [V].
pub const ATTR_REQUIRED_THERMO: i64 = 1212;
/// `attributeThermodynamicsHeatDamage`, on the skill [V-data].
pub const ATTR_THERMO_DAMAGE: i64 = 1229;
/// `attributeModuleRepairRate` [V], the pilot's, in hit points a minute.
pub const ATTR_REPAIR_RATE: i64 = 1267;
/// `attributeMiningAmount` [V]: a mining module has no heat system (EvEJS).
pub const ATTR_MINING_AMOUNT: i64 = 77;
/// `inventorycommon.const.typeThermodynamics` and `typeNaniteRepairPaste` [V].
pub const TYPE_THERMODYNAMICS: i64 = 28164;
pub const TYPE_NANITE_PASTE: i64 = 28668;
/// `flagCargo` [V].
const FLAG_CARGO: i64 = 5;

/// **[A]** used only when a ship's type row names no capacity (EvEJS's
/// fallback, 100): the rack's top.
pub const DEFAULT_CAPACITY: f64 = 100.0;
/// **[A]** when a ship's row names no dissipation rate (EvEJS's, per second).
pub const DEFAULT_DISSIPATION: f64 = 0.01;
/// **[A]** when a ship's row names no attenuation (EvEJS's).
pub const DEFAULT_ATTENUATION: f64 = 0.63;
/// **[A]** when a module's row names no hit points (EvEJS's).
pub const DEFAULT_MODULE_HP: f64 = 40.0;
/// **[A]** when the character table gives no `moduleRepairRate`.
pub const DEFAULT_REPAIR_RATE: f64 = 1.0;
/// **[A]** how much Nanite Repair Paste a full repair takes. The client's own
/// count (`FindNumUnitsToRepairModule`) needs base prices this server does not
/// hold at this seam; falsified by the client's confirmation number differing.
pub const PASTE_FOR_FULL_REPAIR: f64 = 10.0;
/// **[A]** how often a heating or warm rack's value is told again, to cancel
/// the drift between the client's clock and ours (EvEJS: five seconds).
pub const SYNC: Duration = Duration::from_secs(5);
/// **[A]** the seed of the roll stream.
const SEED: u64 = 0x4845_4154_0000_0001;

// ---------------------------------------------------------------------------
// The client's heat formula
// ---------------------------------------------------------------------------

/// `heatAttribute.CalculateHeat` [V], over `dt_ms` milliseconds: towards the
/// capacity while something heats the rack, decaying otherwise, and nought
/// once the decay rounds to nought.
pub fn calculate_heat(
    current: f64,
    dt_ms: f64,
    incoming: f64,
    dissipation: f64,
    generation: f64,
    capacity: f64,
) -> f64 {
    if incoming < 5e-8 {
        let c = current * (-dt_ms / 1000.0 * dissipation).exp();
        if c.round() <= 0.0 { 0.0 } else { c }
    } else {
        let e = (-dt_ms / 1000.0 * incoming * generation).exp();
        capacity - capacity * e + current * e
    }
}

/// The chance one roll hits a module `distance` slots from the cycling one.
pub fn hit_chance(heat: f64, capacity: f64, attenuation: f64, distance: i64) -> f64 {
    if capacity <= 0.0 {
        return 0.0;
    }
    (heat / capacity).clamp(0.0, 1.0) * attenuation.clamp(0.0, 1.0).powi(distance.max(0) as i32)
}

fn rack_index(r: Rack) -> usize {
    match r {
        Rack::High => 0,
        Rack::Medium => 1,
        Rack::Low => 2,
    }
}

const RACKS: [Rack; 3] = [Rack::High, Rack::Medium, Rack::Low];

/// One rack's numbers, off the ship's type row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RackNumbers {
    pub capacity: f64,
    pub dissipation: f64,
    pub attenuation: f64,
    pub generation: f64,
}

fn type_row(w: &Registry, type_id: i64) -> Option<&crate::typedata::TypeRow> {
    crate::typedata::installed_in(w.content())
        .table()
        .get(type_id)
}

/// The numbers of `rack` as the ship's dogma sheet has them (SYS-4): a
/// beacon, a skill or a rig that moves the heat capacity, dissipation,
/// attenuation or generation reaches the roll. One path: an attribute the
/// sheet does not list (or no sheet, when the type data is absent) reads the
/// default.
pub fn rack_numbers(sheet: Option<&crate::dogma::Sheet>, rack: Rack) -> RackNumbers {
    let i = rack_index(rack);
    let get = |a: i64, d: f64| sheet.and_then(|s| s.ship_opt(a)).unwrap_or(d);
    RackNumbers {
        capacity: get(ATTR_CAPACITY[i], DEFAULT_CAPACITY).max(1.0),
        dissipation: get(ATTR_DISSIPATION[i], DEFAULT_DISSIPATION),
        attenuation: get(ATTR_ATTENUATION[i], DEFAULT_ATTENUATION),
        generation: get(ATTR_GENERATION, 1.0),
    }
}

// ---------------------------------------------------------------------------
// What a module type says
// ---------------------------------------------------------------------------

/// The type's overload effect (category 5), if it has one.
pub fn overload_effect(w: &Registry, type_id: i64) -> Option<i64> {
    let table = crate::typedata::installed_in(w.content()).table();
    table.get(type_id)?.effects.iter().copied().find(|e| {
        table
            .effect(*e)
            .is_some_and(|r| r.category == CATEGORY_OVERLOAD)
    })
}

fn type_attr(w: &Registry, type_id: i64, attribute: i64) -> Option<f64> {
    type_row(w, type_id)?.attribute(attribute)
}

/// Hit points of a module type.
pub fn module_hp(w: &Registry, type_id: i64) -> f64 {
    type_attr(w, type_id, ATTR_HP)
        .filter(|h| *h > 0.0)
        .unwrap_or(DEFAULT_MODULE_HP)
}

fn is_mining(w: &Registry, type_id: i64) -> bool {
    type_attr(w, type_id, ATTR_MINING_AMOUNT).is_some_and(|a| a > 0.0)
}

/// A fitted module's attribute as the ship's dogma sheet has it (SYS-4):
/// beacons, skills, ship bonuses and rigs reach it. The sheet already carries
/// every passive effect of the pilot's trained skills, Thermodynamics
/// included, so nothing is applied on top of the value read here.
fn module_attr(sheet: Option<&crate::dogma::Sheet>, module: i64, attribute: i64) -> Option<f64> {
    sheet.and_then(|s| s.module_opt(module, attribute))
}

/// [`module_attr`] for the cost measurement in `dogma::sweep`.
#[cfg(test)]
pub(crate) fn module_attr_for_bench(
    w: &Registry,
    ship: i64,
    module: i64,
    attribute: i64,
) -> Option<f64> {
    module_attr(crate::dogma::sheet(w, ship).as_deref(), module, attribute)
}

/// What a cycling module adds to its rack's heat: its modified `heatRate`.
fn heat_rate(sheet: Option<&crate::dogma::Sheet>, module: i64) -> f64 {
    module_attr(sheet, module, ATTR_HEAT_RATE).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// The slot
// ---------------------------------------------------------------------------

/// One rack of one ship.
#[derive(Debug, Clone, Default, PartialEq)]
struct RackState {
    value: f64,
    /// Blue ticks the value was computed at.
    at: i64,
    /// The modules the pilot's client has been told heat this rack.
    told: BTreeSet<i64>,
    /// Blue ticks the value was last sent.
    synced: i64,
    /// The value last sent.
    sent: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct ShipHeat {
    overloaded: BTreeSet<i64>,
    racks: [RackState; 3],
    /// Module -> the cycle count already rolled for.
    seen: BTreeMap<i64, i64>,
}

impl ShipHeat {
    fn is_idle(&self) -> bool {
        self.overloaded.is_empty()
            && self.seen.is_empty()
            && self
                .racks
                .iter()
                .all(|r| r.value <= 0.0 && r.told.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Repair {
    started: i64,
    /// Blue ticks the whole repair takes.
    duration: i64,
    /// Damage when it started.
    from: f64,
}

/// **The slot**: every ship's overloaded modules and racks, every damaged
/// module and every repair. Never persisted.
#[derive(Debug)]
pub struct Heat {
    ships: BTreeMap<i64, ShipHeat>,
    damage: BTreeMap<i64, f64>,
    repairs: BTreeMap<i64, Repair>,
    rolls: Stream,
}

impl Default for Heat {
    fn default() -> Self {
        Heat {
            ships: BTreeMap::new(),
            damage: BTreeMap::new(),
            repairs: BTreeMap::new(),
            rolls: Stream::new(SEED),
        }
    }
}

impl Heat {
    /// Set a module's damage as a hit left it (for the tests).
    #[cfg(test)]
    pub(crate) fn set_damage(&mut self, module_id: i64, hp: f64) {
        self.damage.insert(module_id, hp);
    }

    /// Take a module's damage out (a dock stashes it, `crate::repair`).
    pub(crate) fn take_damage(&mut self, module_id: i64) -> f64 {
        self.repairs.remove(&module_id);
        self.damage.remove(&module_id).unwrap_or(0.0)
    }

    /// Put a module's damage back (an undock, `crate::repair`).
    pub(crate) fn put_damage(&mut self, module_id: i64, hp: f64) {
        if hp > 0.0 {
            self.damage.insert(module_id, hp);
        }
    }

    /// A rack's heat as last worked out (for the tests).
    #[cfg(test)]
    pub(crate) fn rack_value(&self, ship_id: i64, rack: Rack) -> f64 {
        self.ships
            .get(&ship_id)
            .map_or(0.0, |s| s.racks[rack_index(rack)].value)
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "overheat",
    make: |_| Box::new(Heat::default()) as Boxed,
};

/// Whether `module_id`'s overload effect is on.
pub fn is_overloaded(w: &Registry, ship_id: i64, module_id: i64) -> bool {
    w.ext::<Heat>()
        .ships
        .get(&ship_id)
        .is_some_and(|s| s.overloaded.contains(&module_id))
}

/// A module's damage in hit points.
pub fn module_damage(w: &Registry, module_id: i64) -> f64 {
    w.ext::<Heat>()
        .damage
        .get(&module_id)
        .copied()
        .unwrap_or(0.0)
}

/// Whether a module is burnt out: its damage has reached its hit points.
pub fn burnt_out(w: &Registry, module_id: i64) -> bool {
    let d = module_damage(w, module_id);
    d > 0.0
        && w.item(module_id)
            .is_some_and(|i| d >= module_hp(w, i.type_id))
}

/// **`ModuleTooDamagedToBeOnlined`**, the backstop of the client's own check
/// (`baseDogmaLocation.StartEffect_PreChecks`): a burnt-out module is not put
/// online again until it is repaired.
pub fn refuse_online(w: &Registry, module_id: i64) -> Result<(), ServiceError> {
    if burnt_out(w, module_id) {
        return Err(user_error("ModuleTooDamagedToBeOnlined", vec![]));
    }
    Ok(())
}

/// A module that left its ship (an unfit, a loss): its damage and repair stay
/// with the item, its overload does not.
pub fn forget_overload(w: &mut Registry, module_id: i64) {
    for s in w.ext_mut::<Heat>().ships.values_mut() {
        s.overloaded.remove(&module_id);
        s.seen.remove(&module_id);
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnHeatAdded(heatID, moduleID)` / `OnHeatRemoved` [V]: a module starts or
/// stops heating a rack, for the client's own heat attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct HeatSource {
    pub to: i64,
    pub heat_id: i64,
    pub module_id: i64,
    pub added: bool,
}

impl Push for HeatSource {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.added {
                "OnHeatAdded"
            } else {
                "OnHeatRemoved"
            },
            vec![PyValue::Int(self.heat_id), PyValue::Int(self.module_id)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// One attribute change of the batch [`Changes`] carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// A ship or a module.
    pub item_id: i64,
    pub attribute_id: i64,
    pub new_value: f64,
    pub old_value: f64,
}

/// `OnModuleAttributeChanges`: a **list of eight-tuples** `(eventName,
/// ownerID, itemKey, attributeID, time, newValue, oldValue, wallclockTime)`
/// (`clientDogmaLocation:391` unpacks eight, godma's `OnModuleAttributeChanges`
/// reaches the HUD's `damage` listener through `OnAttributes`): the heat
/// values (a `SetBaseValue`, `cdl:339`) and the damage of a module.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    pub to: i64,
    pub time: i64,
    pub changes: Vec<Change>,
}

impl Push for Changes {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let list = self
            .changes
            .iter()
            .map(|c| {
                PyValue::Tuple(vec![
                    crate::pyutil::bstr("OnModuleAttributeChange"),
                    PyValue::Int(self.to),
                    PyValue::Int(c.item_id),
                    PyValue::Int(c.attribute_id),
                    PyValue::Int(self.time),
                    PyValue::Real(c.new_value),
                    PyValue::Real(c.old_value),
                    PyValue::None,
                ])
            })
            .collect();
        ("OnModuleAttributeChanges", vec![PyValue::List(list)])
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Answer,
    Clock,
}

fn emit(w: &mut Registry, via: Via, p: impl Push + 'static) {
    match via {
        Via::Answer => w.push(p),
        Via::Clock => w.schedule(Duration::ZERO, p),
    }
}

fn effect_push(
    pilot: i64,
    ship: i64,
    module_id: i64,
    effect_id: i64,
    start: bool,
    t: i64,
) -> ShipEffect {
    ShipEffect {
        to: vec![pilot],
        module_id,
        effect_id,
        character_id: pilot,
        ship_id: ship,
        target_id: None,
        t,
        start,
        // An overload effect has no cycle of its own: -1 and -1, as the
        // online effect's.
        duration_ms: -1,
        repeat: -1,
        error: None,
    }
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

struct Fitted {
    pilot: i64,
    ship: i64,
    module: i64,
    type_id: i64,
}

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// The module, if it is fitted to the actor's ship and the actor is in space
/// in it (`overload` is a state of a ship that flies).
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
        pilot: c.character_id,
        ship: c.ship_id,
        module: module_id,
        type_id: item.type_id,
    })
}

fn in_space(w: &Registry, f: &Fitted) -> bool {
    w.roster().get(f.pilot).is_some_and(|p| p.ship_id == f.ship)
}

fn thermo_level(w: &Registry, pilot: i64) -> i64 {
    w.skills_for(Some(pilot))
        .and_then(|s| s.get(TYPE_THERMODYNAMICS))
        .map_or(0, |e| e.level)
}

fn repairing_on(w: &Registry, ship: i64) -> bool {
    w.ext::<Heat>()
        .repairs
        .keys()
        .any(|m| w.item(*m).is_some_and(|i| i.location_id == ship))
}

/// What stops a module being overloaded now.
fn refuse_overload(w: &Registry, f: &Fitted) -> Result<i64, ServiceError> {
    let Some(effect) = overload_effect(w, f.type_id) else {
        return Err(notify("That module cannot be overloaded."));
    };
    if is_mining(w, f.type_id) {
        return Err(notify("That module cannot be overloaded."));
    }
    if !in_space(w, f) {
        return Err(notify("Modules can only be overloaded in space."));
    }
    if !w.modules().is_online(f.module) {
        return Err(user_error(
            "ModuleNotOnline",
            vec![(
                "module",
                type_arg_for("ModuleNotOnline", "module", f.type_id),
            )],
        ));
    }
    if burnt_out(w, f.module) {
        return Err(notify("That module is too damaged to overload."));
    }
    let need = type_attr(w, f.type_id, ATTR_REQUIRED_THERMO).unwrap_or(0.0) as i64;
    if thermo_level(w, f.pilot) < need {
        return Err(user_error(
            "DontHaveThermoDynamicsSkill",
            vec![("skillLevel", PyValue::Int(need.max(1)))],
        ));
    }
    if repairing_on(w, f.ship) {
        return Err(user_error(
            "CanNotOverloadModuleOnShipThatHasModuleRepair",
            vec![("moduleName", type_arg(f.type_id))],
        ));
    }
    Ok(effect)
}

fn switch(w: &mut Registry, f: &Fitted, effect: i64, on: bool, now: i64, via: Via) -> bool {
    let changed = {
        let h = w.ext_mut::<Heat>();
        let s = h.ships.entry(f.ship).or_default();
        if on {
            s.overloaded.insert(f.module)
        } else {
            s.seen.remove(&f.module);
            s.overloaded.remove(&f.module)
        }
    };
    if changed {
        emit(
            w,
            via,
            effect_push(f.pilot, f.ship, f.module, effect, on, now),
        );
    }
    changed
}

/// `dogmaIM.Overload(itemID, effectID)` [V]. Returns the module id.
pub fn overload(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    effect_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let f = fitted(w, actor, module_id)?;
    let effect = refuse_overload(w, &f)?;
    if effect_id != 0 && effect_id != effect {
        return Err(notify("That module cannot be overloaded."));
    }
    switch(w, &f, effect, true, now, Via::Answer);
    settle(w, f.ship, now, Via::Answer);
    Ok(PyValue::Int(module_id))
}

/// `dogmaIM.StopOverload(itemID, effectID)` [V]. Returns the module id.
pub fn stop_overload(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    effect_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let f = fitted(w, actor, module_id)?;
    let effect = overload_effect(w, f.type_id).unwrap_or(effect_id);
    switch(w, &f, effect, false, now, Via::Answer);
    settle(w, f.ship, now, Via::Answer);
    Ok(PyValue::Int(module_id))
}

/// The fitted modules of the rack `module_id` sits in that have an overload
/// effect, by slot.
fn rack_modules(w: &Registry, f: &Fitted) -> Vec<i64> {
    let Some((rack, _)) = w.item(f.module).and_then(|i| Rack::of_flag(i.flag_id)) else {
        return Vec::new();
    };
    let mut v: Vec<(i64, i64)> = w
        .store()
        .contents_of(f.ship)
        .filter(|i| {
            i.singleton
                && Rack::of_flag(i.flag_id).is_some_and(|(r, _)| r == rack)
                && overload_effect(w, i.type_id).is_some()
        })
        .map(|i| (i.flag_id, i.item_id))
        .collect();
    v.sort_unstable();
    v.into_iter().map(|(_, id)| id).collect()
}

/// `dogmaIM.OverloadRack(itemID)` [V]: every module of the rack that can be
/// overloaded, and the list of the ones it changed (the client flashes each
/// one it is given until told).
pub fn overload_rack(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let anchor = fitted(w, actor, module_id)?;
    let mut changed = Vec::new();
    let mut first_refusal = None;
    for m in rack_modules(w, &anchor) {
        let f = Fitted {
            module: m,
            type_id: w.item(m).map_or(0, |i| i.type_id),
            ..anchor_copy(&anchor)
        };
        match refuse_overload(w, &f) {
            Ok(effect) => {
                if switch(w, &f, effect, true, now, Via::Answer) {
                    changed.push(PyValue::Int(m));
                }
            }
            Err(e) => first_refusal = first_refusal.or(Some(e)),
        }
    }
    settle(w, anchor.ship, now, Via::Answer);
    match (changed.is_empty(), first_refusal) {
        (true, Some(e)) => Err(e),
        _ => Ok(PyValue::List(changed)),
    }
}

fn anchor_copy(f: &Fitted) -> Fitted {
    Fitted {
        pilot: f.pilot,
        ship: f.ship,
        module: f.module,
        type_id: f.type_id,
    }
}

/// `dogmaIM.StopOverloadRack(itemID)` [V]: the modules of the rack that were
/// overloaded, each told.
pub fn stop_overload_rack(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let anchor = fitted(w, actor, module_id)?;
    let mut changed = Vec::new();
    for m in rack_modules(w, &anchor) {
        let f = Fitted {
            module: m,
            type_id: w.item(m).map_or(0, |i| i.type_id),
            ..anchor_copy(&anchor)
        };
        let effect = overload_effect(w, f.type_id).unwrap_or(0);
        if switch(w, &f, effect, false, now, Via::Answer) {
            changed.push(PyValue::Int(m));
        }
    }
    settle(w, anchor.ship, now, Via::Answer);
    Ok(PyValue::List(changed))
}

/// The `activeEffects` rows of a module whose overload effect is on, in the
/// shape `modules::module_info` builds (a pilot who logs in mid-flight sees the
/// button lit).
pub fn overload_lines(
    w: &Registry,
    c: &crate::model::Character,
    module_id: i64,
    type_id: i64,
) -> Vec<(PyValue, PyValue)> {
    if !is_overloaded(w, c.ship_id, module_id) {
        return Vec::new();
    }
    let Some(effect) = overload_effect(w, type_id) else {
        return Vec::new();
    };
    let line = PyValue::List(vec![
        PyValue::Int(module_id),
        PyValue::Int(c.character_id),
        PyValue::Int(c.ship_id),
        PyValue::None,
        PyValue::None,
        PyValue::List(Vec::new()),
        PyValue::Int(effect),
        PyValue::None,
        PyValue::Int(-1),
        PyValue::Int(-1),
    ]);
    vec![(line, PyValue::Int(effect))]
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's stage**, after `modules`: every ship's racks and every
/// repair brought up to `now`.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    finish_repairs(w, now);
    let ships: Vec<i64> = w.ext::<Heat>().ships.keys().copied().collect();
    for ship in ships {
        settle(w, ship, now, Via::Clock);
    }
}

/// The modules of `ship` heating `rack` now: overloaded, online and cycling,
/// with each one's rate.
fn heating(
    w: &Registry,
    sheet: Option<&crate::dogma::Sheet>,
    ship: i64,
    rack: Rack,
    overloaded: &BTreeSet<i64>,
) -> BTreeMap<i64, f64> {
    overloaded
        .iter()
        .filter_map(|m| {
            let i = w.item(*m)?;
            if i.location_id != ship || Rack::of_flag(i.flag_id).map(|(r, _)| r) != Some(rack) {
                return None;
            }
            if !w.modules().is_online(*m) || w.modules().running(*m).is_none() {
                return None;
            }
            Some((*m, heat_rate(sheet, *m)))
        })
        .collect()
}

/// A module hit: how many hit points, which module.
struct Hit {
    module: i64,
    hp: f64,
}

/// Bring one ship up to `now`: heat advanced at what the client has been
/// told, the cycles since last looked rolled, burnt-out modules taken
/// offline, and the pilot told what changed.
fn settle(w: &mut Registry, ship: i64, now: i64, via: Via) {
    let Some(pilot) = w.roster().by_ship(ship).map(|p| p.character_id) else {
        // Gone from space (docked, jumped, died): heat and overload end.
        w.ext_mut::<Heat>().ships.remove(&ship);
        return;
    };
    let Some(mut st) = w.ext::<Heat>().ships.get(&ship).cloned() else {
        return;
    };
    // The ship's sheet, once: every number below reads it.
    let sheet = crate::dogma::sheet(w, ship);
    let sheet = sheet.as_deref();
    // A module that left the ship, went offline or burnt out stops.
    let stale: Vec<i64> = st
        .overloaded
        .iter()
        .copied()
        .filter(|m| {
            w.item(*m).is_none_or(|i| i.location_id != ship)
                || !w.modules().is_online(*m)
                || burnt_out(w, *m)
        })
        .collect();
    for m in stale {
        st.overloaded.remove(&m);
        st.seen.remove(&m);
        if let Some(i) = w.item(m).cloned()
            && i.location_id == ship
            && let Some(effect) = overload_effect(w, i.type_id)
        {
            emit(w, via, effect_push(pilot, ship, m, effect, false, now));
        }
    }
    let mut sends: Vec<Change> = Vec::new();
    let mut sources: Vec<HeatSource> = Vec::new();
    let mut hits: Vec<Hit> = Vec::new();
    for rack in RACKS {
        let i = rack_index(rack);
        let nums = rack_numbers(sheet, rack);
        let heaters = heating(w, sheet, ship, rack, &st.overloaded);
        let told_rate: f64 = st.racks[i].told.iter().map(|m| heat_rate(sheet, *m)).sum();
        let r = &mut st.racks[i];
        if r.at > 0 && now > r.at {
            r.value = calculate_heat(
                r.value,
                (now - r.at) as f64 / MSEC as f64,
                told_rate,
                nums.dissipation,
                nums.generation,
                nums.capacity,
            )
            .clamp(0.0, nums.capacity);
        }
        r.at = now;
        // The cycles that ended since last looked, each rolled at the heat
        // the rack has now.
        let mut rolls: Vec<(i64, i64)> = Vec::new();
        for m in heaters.keys() {
            let cycles = w.modules().running(*m).map_or(0, |run| run.cycles);
            match st.seen.insert(*m, cycles) {
                Some(before) if cycles > before => rolls.push((*m, cycles - before)),
                _ => {}
            }
        }
        st.seen
            .retain(|m, _| heaters.contains_key(m) || !rack_holds(w, *m, rack));
        let value = st.racks[i].value;
        for (m, n) in rolls {
            let Some(src) = w.item(m).cloned() else {
                continue;
            };
            // The sheet's heatDamage already holds Thermodynamics.
            let dmg = module_attr(sheet, m, ATTR_HEAT_DAMAGE)
                .unwrap_or(0.0)
                .max(0.0);
            if dmg <= 0.0 {
                continue;
            }
            let (_, src_slot) = Rack::of_flag(src.flag_id).unwrap_or((rack, 0));
            let candidates: Vec<(i64, i64)> = w
                .store()
                .contents_of(ship)
                .filter(|c| {
                    c.singleton
                        && Rack::of_flag(c.flag_id).is_some_and(|(r, _)| r == rack)
                        && !is_mining(w, c.type_id)
                })
                .filter_map(|c| {
                    let (_, slot) = Rack::of_flag(c.flag_id)?;
                    Some((c.item_id, (slot - src_slot).abs()))
                })
                .collect();
            for _ in 0..n {
                for (c, distance) in &candidates {
                    if burnt_out(w, *c) {
                        continue;
                    }
                    let p = hit_chance(value, nums.capacity, nums.attenuation, *distance);
                    if p > 0.0 && w.ext_mut::<Heat>().rolls.chance(p) {
                        hits.push(Hit {
                            module: *c,
                            hp: dmg,
                        });
                    }
                }
            }
        }
        // What the pilot's client is holding, against what is heating.
        let now_set: BTreeSet<i64> = heaters.keys().copied().collect();
        let added: Vec<i64> = now_set.difference(&st.racks[i].told).copied().collect();
        let removed: Vec<i64> = st.racks[i].told.difference(&now_set).copied().collect();
        let due = now - st.racks[i].synced >= SYNC.as_nanos() as i64 / 100;
        let warm = value > 0.0 || !now_set.is_empty() || !st.racks[i].told.is_empty();
        if !added.is_empty() || !removed.is_empty() || (due && warm) {
            sends.push(Change {
                item_id: ship,
                attribute_id: ATTR_HEAT[i],
                new_value: value,
                old_value: st.racks[i].sent,
            });
            for m in removed {
                sources.push(HeatSource {
                    to: pilot,
                    heat_id: ATTR_HEAT[i],
                    module_id: m,
                    added: false,
                });
            }
            for m in added {
                sources.push(HeatSource {
                    to: pilot,
                    heat_id: ATTR_HEAT[i],
                    module_id: m,
                    added: true,
                });
            }
            st.racks[i].told = now_set;
            st.racks[i].synced = now;
            st.racks[i].sent = value;
        }
    }
    if !sends.is_empty() {
        emit(
            w,
            via,
            Changes {
                to: pilot,
                time: now,
                changes: sends,
            },
        );
    }
    for s in sources {
        emit(w, via, s);
    }
    // The hits, and what burns out.
    let mut burnt: Vec<i64> = Vec::new();
    let mut damage_changes: Vec<Change> = Vec::new();
    for h in hits {
        let Some(item) = w.item(h.module).cloned() else {
            continue;
        };
        let hp = module_hp(w, item.type_id);
        let before = module_damage(w, h.module);
        if before >= hp {
            continue;
        }
        let after = (before + h.hp).min(hp);
        w.ext_mut::<Heat>().damage.insert(h.module, after);
        damage_changes.push(Change {
            item_id: h.module,
            attribute_id: ATTR_DAMAGE,
            new_value: after,
            old_value: before,
        });
        if after >= hp {
            burnt.push(h.module);
        }
    }
    w.ext_mut::<Heat>().ships.insert(ship, st);
    if !damage_changes.is_empty() {
        emit(
            w,
            via,
            Changes {
                to: pilot,
                time: now,
                changes: damage_changes,
            },
        );
    }
    for m in burnt {
        burn_out(w, pilot, ship, m, now, via);
    }
    let idle = w
        .ext::<Heat>()
        .ships
        .get(&ship)
        .is_some_and(ShipHeat::is_idle);
    if idle {
        w.ext_mut::<Heat>().ships.remove(&ship);
    }
}

fn rack_holds(w: &Registry, module: i64, rack: Rack) -> bool {
    w.item(module)
        .and_then(|i| Rack::of_flag(i.flag_id))
        .is_some_and(|(r, _)| r == rack)
}

/// A module's damage reached its hit points: it stops, its overload ends and
/// it goes offline, and it stays offline until repaired.
fn burn_out(w: &mut Registry, pilot: i64, ship: i64, module: i64, now: i64, via: Via) {
    if let Some(s) = w.ext_mut::<Heat>().ships.get_mut(&ship) {
        s.overloaded.remove(&module);
        s.seen.remove(&module);
    }
    if let Some(effect) = w.item(module).and_then(|i| overload_effect(w, i.type_id)) {
        emit(w, via, effect_push(pilot, ship, module, effect, false, now));
    }
    crate::modules::stop(w, module, now, None);
    if crate::modules::set_online_kept(w, module, false) {
        emit(
            w,
            via,
            effect_push(pilot, ship, module, EFFECT_ONLINE, false, now),
        );
        // The client's dogma takes a module offline only on an `isOnline`
        // attribute change to 0 (`clientDogmaLocation.OnModuleAttributeChanges`
        // stops the online effect); the godma effect above updates godma's
        // view alone, so dogma kept the module online.
        emit(
            w,
            via,
            Changes {
                to: pilot,
                time: now,
                changes: vec![Change {
                    item_id: module,
                    attribute_id: crate::modules::ATTR_IS_ONLINE,
                    new_value: 0.0,
                    old_value: 1.0,
                }],
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------

fn paste_stacks(w: &Registry, pilot: i64, ship: i64) -> Vec<(i64, i64)> {
    w.store()
        .items()
        .iter()
        .filter(|i| {
            i.location_id == ship
                && i.flag_id == FLAG_CARGO
                && i.type_id == TYPE_NANITE_PASTE
                && i.owner_id == pilot
        })
        .map(|i| (i.item_id, i.quantity))
        .collect()
}

/// The paste a repair of `damage` of `hp` takes.
pub fn paste_needed(damage: f64, hp: f64) -> i64 {
    if hp <= 0.0 {
        return 0;
    }
    ((damage / hp).clamp(0.0, 1.0) * PASTE_FOR_FULL_REPAIR).ceil() as i64
}

fn refuse_repair(w: &Registry, f: &Fitted) -> Result<(), ServiceError> {
    let name = || vec![("moduleName", type_arg(f.type_id))];
    if w.modules().running(f.module).is_some() {
        return Err(user_error("CanNotRepairWhileModuleIsActive", name()));
    }
    if w.modules().is_online(f.module) {
        return Err(user_error("CanNotRepairWhileModuleIsOnline", name()));
    }
    let overloaded = w
        .ext::<Heat>()
        .ships
        .get(&f.ship)
        .is_some_and(|s| !s.overloaded.is_empty());
    if overloaded {
        return Err(user_error("CanNotRepairWhileModulesOverloaded", vec![]));
    }
    Ok(())
}

/// How one module's repair began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Begun {
    Started,
    /// Nothing damaged, or already being repaired.
    Nothing,
    /// The paste in the hold is short.
    NoPaste,
}

fn begin_repair(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    now: i64,
) -> Result<Begun, ServiceError> {
    let f = fitted(w, actor, module_id)?;
    let damage = module_damage(w, module_id);
    if damage <= 0.0 || w.ext::<Heat>().repairs.contains_key(&module_id) {
        return Ok(Begun::Nothing);
    }
    refuse_repair(w, &f)?;
    let hp = module_hp(w, f.type_id);
    let units = paste_needed(damage, hp);
    let stacks = paste_stacks(w, f.pilot, f.ship);
    if stacks.iter().map(|(_, q)| *q).sum::<i64>() < units {
        return Ok(Begun::NoPaste);
    }
    let mut left = units;
    let mut edits = Vec::new();
    for (id, q) in stacks {
        if left <= 0 {
            break;
        }
        let take = left.min(q);
        edits.push(if take == q {
            crate::item::Edit::Destroy { item_id: id }
        } else {
            crate::item::Edit::Consume {
                item_id: id,
                quantity: take,
            }
        });
        left -= take;
    }
    if !edits.is_empty() && w.apply_items(&edits).is_err() {
        return Err(notify("The repair paste could not be taken."));
    }
    let rate = crate::typedata::installed_in(w.content())
        .table()
        .default_value(ATTR_REPAIR_RATE);
    let rate = if rate > 0.0 {
        rate
    } else {
        DEFAULT_REPAIR_RATE
    };
    // `damage / rate` minutes (`godma:2172`).
    let duration = (damage / rate * 60_000.0 * MSEC as f64) as i64;
    w.ext_mut::<Heat>().repairs.insert(
        module_id,
        Repair {
            started: now,
            duration,
            from: damage,
        },
    );
    Ok(Begun::Started)
}

/// `dogmaIM.InitiateModuleRepair(itemID)`: `True` begun, `False` nothing to
/// repair [V] (`godma:2150`).
pub fn initiate_repair(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    match begin_repair(w, actor, module_id, now)? {
        Begun::Started => Ok(PyValue::Bool(true)),
        Begun::Nothing => Ok(PyValue::Bool(false)),
        Begun::NoPaste => {
            let paste = PyValue::Tuple(vec![
                PyValue::Int(crate::modules::UE_TYPEID),
                PyValue::Int(TYPE_NANITE_PASTE),
            ]);
            Err(user_error(
                "NotEnoughRepairMaterialToFinishAllRepairs",
                vec![("typeName", paste)],
            ))
        }
    }
}

/// `dogmaIM.InitiateModuleRepairMany(itemIDs)` -> `(success, missingCharges,
/// failed)` [V] (`godma:2130`): the modules begun, the ones the paste ran out
/// for, and the ones refused.
pub fn initiate_repair_many(
    w: &mut Registry,
    actor: Option<i64>,
    modules: &[i64],
    now: i64,
) -> Result<PyValue, ServiceError> {
    let (mut ok, mut missing, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for m in modules {
        match begin_repair(w, actor, *m, now) {
            Ok(Begun::Started) => ok.push(PyValue::Int(*m)),
            Ok(Begun::NoPaste) => missing.push(PyValue::Int(*m)),
            Ok(Begun::Nothing) | Err(_) => failed.push(PyValue::Int(*m)),
        }
    }
    Ok(PyValue::Tuple(vec![
        PyValue::List(ok),
        PyValue::List(missing),
        PyValue::List(failed),
    ]))
}

/// Settle a repair as of `now`: the damage left is what the time spent
/// repaired. `None` if the module was not being repaired.
fn settle_repair(w: &mut Registry, module: i64, now: i64) -> Option<(f64, f64)> {
    let r = w.ext::<Heat>().repairs.get(&module).cloned()?;
    let spent = (now - r.started).max(0) as f64;
    let done = if r.duration <= 0 {
        1.0
    } else {
        (spent / r.duration as f64).clamp(0.0, 1.0)
    };
    let left = (r.from * (1.0 - done)).max(0.0);
    let before = module_damage(w, module);
    let h = w.ext_mut::<Heat>();
    h.repairs.remove(&module);
    if left <= 0.0 {
        h.damage.remove(&module);
    } else {
        h.damage.insert(module, left);
    }
    Some((left, before))
}

fn tell_damage(w: &mut Registry, module: i64, left: f64, before: f64, now: i64, via: Via) {
    let Some(pilot) = w
        .item(module)
        .and_then(|i| w.roster().by_ship(i.location_id).map(|p| p.character_id))
    else {
        return;
    };
    emit(
        w,
        via,
        Changes {
            to: pilot,
            time: now,
            changes: vec![Change {
                item_id: module,
                attribute_id: ATTR_DAMAGE,
                new_value: left,
                old_value: before,
            }],
        },
    );
}

/// `dogmaIM.StopModuleRepair(itemID)` [V] (`godma:2182`, the client calls it
/// when its own timer ends): what was repaired so far is kept.
pub fn stop_repair(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    now: i64,
) -> Result<PyValue, ServiceError> {
    fitted(w, actor, module_id)?;
    if let Some((left, before)) = settle_repair(w, module_id, now) {
        tell_damage(w, module_id, left, before, now, Via::Answer);
    }
    Ok(PyValue::None)
}

fn finish_repairs(w: &mut Registry, now: i64) {
    let due: Vec<i64> = w
        .ext::<Heat>()
        .repairs
        .iter()
        .filter(|(_, r)| now - r.started >= r.duration)
        .map(|(m, _)| *m)
        .collect();
    for m in due {
        if let Some((left, before)) = settle_repair(w, m, now) {
            tell_damage(w, m, left, before, now, Via::Clock);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
