//! **Command bursts: the fleet boosts** (K-BOOST). Lane note
//! `docs/lanes/k-boost.md`; marks as `docs/content/README.md` §2.
//!
//! A command burst module with its charge applies the charge's warfare buffs
//! to the fleet around it for a duration, through the client's own *dbuff*
//! system (`dbuff/`, below), and the buffs move the ship's attributes through
//! the dogma engine ([`crate::dogma::evaluate_with`]).
//!
//! # What the client says [V]
//!
//! All of it from `out/pyc/dbuff/` and `dogma/const.pyc`; the numbers of each
//! collection are the bundle's `dbuffs/*.tsv` ([`crate::dbuff_data`]).
//!
//! - **The module's buffs** (`common/baseBurstEffect.py`,
//!   `GetModuleBuffValues`): for each of the four pairs `(dbuffNID,
//!   dbuffNValue)` = `(2468, 2469)`, `(2470, 2471)`, `(2472, 2473)`, `(2536,
//!   2537)` of the **module's own, modified** attributes, an id above zero
//!   names a collection and the value is its strength. The duration is the
//!   module's `buffDuration` (2535), milliseconds.
//! - **The charge sets them**: effect 6737 `chargeBonusWarfareCharge` (a
//!   passive effect of the charge, `typedata` modifier rows) assigns the
//!   charge's own `dbuffNID` onto the module (`PostAssignment`, domain
//!   `otherID`) and multiplies the module's `dbuffNValue` by the charge's
//!   `dbuffNMultiplier` (2596-2599, `PostMul`). The dogma engine does both, so
//!   [`burst_of`] reads the finished numbers off the module. The client's
//!   fitting window shows the same pair (`client/uiFormatters.py`
//!   `_GetAttributeMappings`: a charge by its multipliers, a module by its
//!   values).
//! - **Which effect**: the five `moduleBonusWarfareLink*` effects 6732-6736
//!   (`dogma/const.pyc` `effectModuleBonusWarfareLink{Armor,Shield,Info,
//!   Mining,Skirmish}`), `dogmaXP.CommandBurst*Effect`. The client's own
//!   (ghost-fitting) `Start` applies the buffs **to the source ship**
//!   (`ApplyBuffToSourceShip`, `eve/client/script/ui/shared/fittingScreen/
//!   pythonEffects.py`), so the booster is always boosted.
//! - **The buff** (`common/registry.py`): a *dynamic buff* per ship and
//!   collection with an expiry `now + duration` (`gametime.GetSimTime() +
//!   int(duration)`, the duration in filetime ticks = ms x `MSEC`); a second
//!   input to a live buff goes through the collection's aggregator
//!   (`common/aggregators.py`, `Minimum` or `Maximum`: [`Aggregator`]); an
//!   expiry discards inputs older than now.
//! - **What it does to the ship** (`ModifierApplier.ApplyToDogmaItem`): the
//!   collection's operator with the buff's output value, on each *item
//!   modifier* attribute of the ship itself, and each *location* modifier
//!   attribute of what is fitted in it (all of it, a group of it, or what
//!   requires a skill). The value's source has no item, so on an attribute
//!   that is not stackable it is stacking-penalised
//!   (`stackingNurfedAttribute._ShouldNotNurf`: `attribute.item is None`,
//!   `DynamicBuffAttribute`).
//! - **What the client is told** (`michelle.py` `OnDbuffUpdated(shipID,
//!   dbuffState)`, a destiny action in `nonDestinyCriticalFunctions`;
//!   `dbuff/client/incomingDbuffTracker.py`): `dbuffState` is a list of
//!   `(collectionID, (value, expiryTime))` (`registry.GetDbuffStateForClient`
//!   -> `Aggregator.GetStateForClient`: the current output and the final
//!   expiry); it is read only when `shipID` is the pilot's own, and a falsy
//!   state resets it. The buff bar shows a slot while one of its collections
//!   is in the tracker (`buffButtons.py` `_ShouldBeVisible`,
//!   `buffBarConst.DBUFFS_BY_SLOT`) with a timer to the latest expiry.
//! - **`OnFleetBoosted(moduleTypeID, numBoosted)`** (`dbuffClient.py`): the
//!   booster's prompt `OnSuccessfulFleetBoost`.
//!
//! # What is not in the client, so [A]
//!
//! The client has the effect's *Start* for the source ship only; who else
//! gets the buffs is server code. Each choice is a named const or rule, and
//! the lane note lists the falsifier of each:
//!
//! - **[A] recipients**: the booster's fleet members (`crate::fleet`), in the
//!   booster's solar system, within the module's modified `maxRange` (54),
//!   centre to centre ([`recipients`]).
//! - **[A] the pulse**: every cycle of the module applies the buffs again,
//!   from the cycle's start, so a module left running keeps the buffs up
//!   ([`reconcile`]); one that is stopped leaves its buffs to run out.
//! - **[A] a ship that leaves** the park (docks, jumps, dies, logs off) loses
//!   its buffs, as the client's tracker does on a ship or location change
//!   (`IncomingDbuffTracker.OnSessionChanged`).
//! - **[A] `numBoosted`**: the fleet members boosted other than the booster,
//!   told once when a module's first pulse boosts at least one.
//!
//! # What this holds
//!
//! The world slot [`SLOT`] holds each boosted ship's aggregators (never
//! persisted: a restart empties every park), what its pilot was last told
//! and the [`Applied`] buffs the dogma engine reads ([`applied`]). Like
//! `ewar`, a ship's numbers move only here, on the clock.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use eve_marshal::PyValue;

use crate::damage::BlueTime;
use crate::dbuff_data::{Aggregate, Dbuffs};
use crate::dogma::{Inputs, Sheet};
use crate::modules::{MSEC, Running};
use crate::notify::Push;
use crate::service::Registry;
use crate::tackle::{Data, Live};
use crate::typedata::Table;
use crate::world_ext::{Boxed, SlotSpec};

/// `effectModuleBonusWarfareLinkArmor`, `...Shield`, `...Skirmish`, `...Info`,
/// `...Mining` [V `dogma/const.pyc`].
pub const EFFECT_ARMOR: i64 = 6732;
pub const EFFECT_SHIELD: i64 = 6733;
pub const EFFECT_SKIRMISH: i64 = 6734;
pub const EFFECT_INFO: i64 = 6735;
pub const EFFECT_MINING: i64 = 6736;
/// The five burst effects.
pub const EFFECTS: [i64; 5] = [
    EFFECT_ARMOR,
    EFFECT_SHIELD,
    EFFECT_SKIRMISH,
    EFFECT_INFO,
    EFFECT_MINING,
];

/// Dogma attribute ids [V `dogma/const.pyc`, `dogmaattributes`].
pub mod attr {
    /// `attributeDbuff1ID..4ID` with their `...Value`s: the pairs on a burst
    /// module (`dbuffAttributeValueMappings`).
    pub const BUFFS: [(i64, i64); 4] = [(2468, 2469), (2470, 2471), (2472, 2473), (2536, 2537)];
    /// `attributeBuffDuration`, ms.
    pub const BUFF_DURATION: i64 = 2535;
    /// `maxRange`: the module's reach.
    pub const MAX_RANGE: i64 = 54;
    pub const MAX_VELOCITY: i64 = 37;
}

/// Whether `effect_id` is a command burst.
pub fn is_burst(effect_id: i64) -> bool {
    EFFECTS.contains(&effect_id)
}

// ---------------------------------------------------------------------------
// The aggregator
// ---------------------------------------------------------------------------

/// One collection's timed inputs on one ship [V `dbuff/common/aggregators.py`
/// `AggregatorMinimum`/`AggregatorMaximum`]: sorted by expiry, each later one
/// weaker than the one before, so the first is the output now and the last
/// is when the buff finally ends.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregator {
    mode: Aggregate,
    /// `(expiry, value)`, ascending by expiry.
    inputs: Vec<(BlueTime, f64)>,
}

impl Aggregator {
    pub fn new(mode: Aggregate, expiry: BlueTime, value: f64) -> Aggregator {
        Aggregator {
            mode,
            inputs: vec![(expiry, value)],
        }
    }

    /// `v` is at least as good as `w` in this mode: `<=` for a minimum.
    fn at_least(&self, v: f64, w: f64) -> bool {
        match self.mode {
            Aggregate::Minimum => v <= w,
            Aggregate::Maximum => v >= w,
        }
    }

    /// Strictly better.
    fn better(&self, v: f64, w: f64) -> bool {
        self.at_least(v, w) && v != w
    }

    /// `MergeNewTimedInput` [V]: an input that lasts no longer than one at
    /// least as strong is dropped; otherwise it joins, and the inputs it
    /// beats are dropped (the shorter-lived that are not stronger, the
    /// longer-lived that are not weaker, in the client's own comparison).
    pub fn merge(&mut self, expiry: BlueTime, value: f64) {
        if self
            .inputs
            .iter()
            .any(|(e, v)| *e >= expiry && self.at_least(*v, value))
        {
            return;
        }
        let mut result: Vec<(BlueTime, f64)> = self
            .inputs
            .iter()
            .copied()
            .filter(|(e, v)| *e < expiry && self.better(*v, value))
            .collect();
        result.push((expiry, value));
        result.extend(
            self.inputs
                .iter()
                .copied()
                .filter(|(e, v)| *e > expiry && self.better(value, *v)),
        );
        self.inputs = result;
    }

    /// `DiscardExpiredTimedInputs` [V]: inputs whose expiry is before `now`.
    pub fn discard_expired(&mut self, now: BlueTime) {
        while self.inputs.first().is_some_and(|(e, _)| *e < now) {
            self.inputs.remove(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// `GetCurrentOutputValue`: the first input's value.
    pub fn value(&self) -> Option<f64> {
        self.inputs.first().map(|(_, v)| *v)
    }

    /// `GetFinalExpiryTime`: the last input's expiry.
    pub fn final_expiry(&self) -> Option<BlueTime> {
        self.inputs.last().map(|(e, _)| *e)
    }
}

// ---------------------------------------------------------------------------
// What the dogma engine reads
// ---------------------------------------------------------------------------

/// One live buff, resolved against its collection: what the dogma engine
/// applies (`ModifierApplier.ApplyToDogmaItem`). Plain data, so the engine
/// stays pure and the sheet cache can key on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub collection: i64,
    /// A [`crate::dogma::op`] id.
    pub op: i64,
    /// The aggregator's output.
    pub value: f64,
    /// Attributes of the ship itself.
    pub item: Vec<i64>,
    /// Attributes of everything fitted.
    pub location: Vec<i64>,
    /// `(attribute, group)` of what is fitted.
    pub group: Vec<(i64, i64)>,
    /// `(attribute, skill)` of what is fitted and needs the skill.
    pub skill: Vec<(i64, i64)>,
}

/// **The ship's live buffs** for the dogma engine: what the last
/// [`reconcile`] settled. Empty for a ship nothing boosts.
pub fn applied(w: &Registry, ship: i64) -> Vec<Applied> {
    w.ext::<Boosts>()
        .ships
        .get(&ship)
        .map_or_else(Vec::new, |s| s.applied.clone())
}

/// **`dbuffState` for a `SetState`**: what `ship`'s pilot was last told, in
/// [`DbuffUpdated::state_value`]'s shape (`[(collection, (value, expiry))]`),
/// so a re-sync restores the ewar and boost HUD it would otherwise reset.
/// Empty when nothing applies. From the told entries rather than
/// [`applied`], which has no expiry.
pub fn state_value_of(w: &Registry, ship: i64) -> PyValue {
    let state = w
        .ext::<Boosts>()
        .ships
        .get(&ship)
        .map_or_else(Vec::new, |s| s.told.clone());
    DbuffUpdated {
        to: 0,
        stamp: 0,
        ship,
        state,
    }
    .state_value()
}

// ---------------------------------------------------------------------------
// The state
// ---------------------------------------------------------------------------

/// One entry of `dbuffState`: `(collection, value, final expiry)`.
type Entry = (i64, f64, BlueTime);

/// One boosted ship.
#[derive(Debug, Clone, PartialEq)]
struct ShipBuffs {
    buffs: BTreeMap<i64, Aggregator>,
    /// What its pilot was last told (empty: nothing, or reset).
    told: Vec<Entry>,
    /// What the engine reads.
    applied: Vec<Applied>,
}

/// **The slot**: the boosted ships, and the cycle of each burst module
/// already pulsed.
#[derive(Debug, Default)]
pub struct Boosts {
    ships: BTreeMap<i64, ShipBuffs>,
    /// Burst module -> the cycles already pulsed.
    cycles: BTreeMap<i64, i64>,
}

impl Boosts {
    /// The ships holding a buff, ascending (for the tests).
    pub fn boosted(&self) -> Vec<i64> {
        self.ships.keys().copied().collect()
    }
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "dbuff",
    make: |_| Box::new(Boosts::default()) as Boxed,
};

// ---------------------------------------------------------------------------
// A burst, read off its module
// ---------------------------------------------------------------------------

/// One burst module's pulse: the buffs, how long they last and how far they
/// reach.
#[derive(Debug, Clone, PartialEq)]
pub struct Burst {
    /// `(collection, value)`, ascending by collection.
    pub buffs: Vec<(i64, f64)>,
    pub duration_ms: f64,
    pub range: f64,
}

/// What a running burst module gives: the module's own modified attributes
/// [V `GetBuffValuesAndDurationMillis`]. `None` when it names no collection
/// (an empty module, or no charge: the charge's effect assigns the ids) or
/// has no duration.
pub fn burst_of(w: &Registry, r: &Running, d: &dyn Data) -> Option<Burst> {
    let mut buffs = BTreeMap::new();
    for (id_attr, value_attr) in attr::BUFFS {
        let id = d.module_attr(w, r, id_attr).unwrap_or(0.0) as i64;
        if id > 0 {
            // [V]: a repeated id keeps the later pair, as the client's dict.
            buffs.insert(id, d.module_attr(w, r, value_attr).unwrap_or(0.0));
        }
    }
    let duration_ms = d.module_attr(w, r, attr::BUFF_DURATION).unwrap_or(0.0);
    if buffs.is_empty() || duration_ms <= 0.0 {
        return None;
    }
    Some(Burst {
        buffs: buffs.into_iter().collect(),
        duration_ms,
        range: d.module_attr(w, r, attr::MAX_RANGE).unwrap_or(0.0),
    })
}

/// **Who a burst reaches** **[A]**: the booster's own ship, and the ship of
/// every other member of his fleet who is in his system and within `range`
/// of him, centre to centre. Ascending by ship id, the booster's among them.
pub fn recipients(w: &Registry, r: &Running, range: f64) -> Vec<i64> {
    let mut out = BTreeSet::new();
    out.insert(r.ship_id);
    let Some(source) = w.roster().get(r.character_id).copied() else {
        return out.into_iter().collect();
    };
    if source.ship_id != r.ship_id {
        return Vec::new();
    }
    if let Some(fleet) = w.fleets().fleet_of(r.character_id) {
        for member in fleet.members.keys().copied() {
            let Some(p) = w.roster().get(member) else {
                continue;
            };
            if member != r.character_id
                && p.solar_system_id == source.solar_system_id
                && crate::space_state::distance(source.motion.position, p.motion.position) <= range
            {
                out.insert(p.ship_id);
            }
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// The pushes
// ---------------------------------------------------------------------------

/// `OnDbuffUpdated(shipID, dbuffState)` [V `michelle.py`]: **a destiny action,
/// not a notification**, in `nonDestinyCriticalFunctions` (so it needs no
/// time sync and may repeat a stamp), one entry per batch, as
/// `destiny::DamageStateChange`. To the pilot alone: the client reads it only
/// for its own ship.
#[derive(Debug, Clone, PartialEq)]
pub struct DbuffUpdated {
    to: i64,
    stamp: u32,
    ship: i64,
    state: Vec<Entry>,
}

impl DbuffUpdated {
    /// The client's shape: `[(collectionID, (value, expiryTime)), ...]`.
    pub fn state_value(&self) -> PyValue {
        PyValue::List(
            self.state
                .iter()
                .map(|(c, v, e)| {
                    PyValue::Tuple(vec![
                        PyValue::Int(*c),
                        PyValue::Tuple(vec![PyValue::Real(*v), PyValue::Int(*e)]),
                    ])
                })
                .collect(),
        )
    }
}

impl Push for DbuffUpdated {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    /// `("DoDestinyUpdate", [[(stamp, ("OnDbuffUpdated", (shipID, state)))],
    /// 0])`: the envelope of every destiny batch.
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnDbuffUpdated"),
                PyValue::Tuple(vec![PyValue::Int(self.ship), self.state_value()]),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship])
    }
}

/// `OnFleetBoosted(moduleTypeID, numBoosted)` [V `dbuffClient.py`]: a plain
/// notification to the booster.
#[derive(Debug, Clone, PartialEq)]
pub struct Boosted {
    pub to: i64,
    pub module_type: i64,
    pub num_boosted: i64,
}

impl Push for Boosted {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnFleetBoosted",
            vec![
                PyValue::Int(self.module_type),
                PyValue::Int(self.num_boosted),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

fn schedule<P: Push + 'static>(w: &mut Registry, p: P) {
    w.schedule(Duration::ZERO, p);
}

/// `OnModuleAttributeChange` to the pilot of `key`, the item being the ship
/// or one of its modules.
pub(crate) fn attribute_push(w: &mut Registry, key: i64, c: &Change, now: BlueTime) {
    schedule(
        w,
        crate::modules::ModulePush::Attribute(crate::modules::AttributeChange {
            to: vec![key],
            owner_id: key,
            item_id: c.item,
            attribute_id: c.attribute,
            time: now,
            new_value: c.new,
            old_value: c.old,
        }),
    );
}

/// `SetMaxSpeed(ship, max)` to everyone in the ship's system (the update's
/// grid role keeps it to the parks that hold the ball).
pub(crate) fn speed_push(w: &mut Registry, key: i64, ship: i64, max: f64) {
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

// ---------------------------------------------------------------------------
// The attribute changes
// ---------------------------------------------------------------------------

/// One attribute of the ship or of a fitted module that a change of buffs
/// moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The ship's id, or the module's.
    pub item: i64,
    pub attribute: i64,
    pub old: f64,
    pub new: f64,
}

/// **What moves** when the live buffs go from `old` to `new` on the ship in
/// `inputs`: every attribute either list names, on the ship (item modifiers)
/// and on each fitted module (the location modifiers), whose engine value
/// differs. Pure: the engine run three times over `table`, the third without
/// buffs, because a sheet lists only what a modifier aims at or a type row
/// carries -- an attribute a buff no longer aims at reads from the unbuffed
/// sheet, or its default.
pub fn attribute_changes(
    table: &Table,
    ship: i64,
    inputs: &Inputs,
    old: &[Applied],
    new: &[Applied],
) -> Vec<Change> {
    let run = |applied: &[Applied]| crate::dogma::evaluate_with(table, inputs, applied);
    let (Some(before), Some(after), Some(base)) = (run(old), run(new), run(&[])) else {
        return Vec::new();
    };
    let ship_val = |s: &Sheet, a: i64| {
        s.ship_opt(a)
            .or_else(|| base.ship_opt(a))
            .unwrap_or_else(|| table.default_value(a))
    };
    let module_val = |s: &Sheet, m: i64, a: i64| {
        s.module_opt(m, a)
            .or_else(|| base.module_opt(m, a))
            .unwrap_or_else(|| table.default_value(a))
    };
    let mut out = Vec::new();
    let mut ship_attrs: BTreeSet<i64> = BTreeSet::new();
    let mut module_attrs: BTreeSet<i64> = BTreeSet::new();
    for a in old.iter().chain(new) {
        ship_attrs.extend(a.item.iter().copied());
        module_attrs.extend(a.location.iter().copied());
        module_attrs.extend(a.group.iter().map(|g| g.0));
        module_attrs.extend(a.skill.iter().map(|s| s.0));
    }
    for a in ship_attrs {
        let (o, n) = (ship_val(&before, a), ship_val(&after, a));
        if o != n {
            out.push(Change {
                item: ship,
                attribute: a,
                old: o,
                new: n,
            });
        }
    }
    for m in &inputs.modules {
        for a in &module_attrs {
            let (o, n) = (
                module_val(&before, m.item_id, *a),
                module_val(&after, m.item_id, *a),
            );
            if o != n {
                out.push(Change {
                    item: m.item_id,
                    attribute: *a,
                    old: o,
                    new: n,
                });
            }
        }
    }
    out
}

/// **What moves** when the ship's inputs go from `old` to `new` (a booster
/// taken or run out), under the live `buffs`: every ship attribute and every
/// attribute of a module on both sides either sheet computed whose value
/// differs. Pure; the engine run twice.
pub fn attribute_changes_inputs(
    table: &Table,
    ship: i64,
    old: &Inputs,
    new: &Inputs,
    buffs: &[Applied],
) -> Vec<Change> {
    let (Some(before), Some(after)) = (
        crate::dogma::evaluate_with(table, old, buffs),
        crate::dogma::evaluate_with(table, new, buffs),
    ) else {
        return Vec::new();
    };
    let value = |s: &Sheet, a: i64| s.ship_opt(a).unwrap_or_else(|| table.default_value(a));
    let attrs: BTreeSet<i64> = before
        .ship_attributes()
        .keys()
        .chain(after.ship_attributes().keys())
        .copied()
        .collect();
    let mut out = Vec::new();
    for a in attrs {
        let (o, n) = (value(&before, a), value(&after, a));
        if o != n {
            out.push(Change {
                item: ship,
                attribute: a,
                old: o,
                new: n,
            });
        }
    }
    for m in &new.modules {
        let (Some(was), Some(now)) = (
            before.module_attributes(m.item_id),
            after.module_attributes(m.item_id),
        ) else {
            continue;
        };
        let attrs: BTreeSet<i64> = was.keys().chain(now.keys()).copied().collect();
        for a in attrs {
            let get = |t: &BTreeMap<i64, f64>| {
                t.get(&a).copied().unwrap_or_else(|| table.default_value(a))
            };
            let (o, n) = (get(was), get(now));
            if o != n {
                out.push(Change {
                    item: m.item_id,
                    attribute: a,
                    old: o,
                    new: n,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's stage**, after `remote_rep`: [`reconcile`] on the
/// server's own data.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    reconcile(w, now, &Live);
}

/// Bring every boosted ship up to `now`: the pulse of each new burst cycle,
/// the expiry of each buff that ran out, and what each pilot is told.
pub fn reconcile(w: &mut Registry, now: BlueTime, d: &dyn Data) {
    pulse(w, now, d);
    let ships: Vec<i64> = w.ext::<Boosts>().ships.keys().copied().collect();
    for ship in ships {
        settle(w, ship, now);
    }
}

/// Every running burst module whose cycle is new: its buffs onto each
/// recipient's aggregators. Nothing is told here ([`settle`] does).
fn pulse(w: &mut Registry, now: BlueTime, d: &dyn Data) {
    let running: Vec<Running> = w
        .modules()
        .running_all()
        .into_iter()
        .filter(|r| is_burst(r.effect.effect_id))
        .collect();
    // A module no longer running is forgotten, so a later activation is a
    // first pulse again.
    {
        let live: BTreeSet<i64> = running.iter().map(|r| r.module_id).collect();
        w.ext_mut::<Boosts>().cycles.retain(|m, _| live.contains(m));
    }
    for r in running {
        if r.stop_at.is_some_and(|s| s <= now) {
            continue;
        }
        let seen = w.ext::<Boosts>().cycles.get(&r.module_id).copied();
        if seen.is_some_and(|n| n >= r.cycles) {
            continue;
        }
        w.ext_mut::<Boosts>().cycles.insert(r.module_id, r.cycles);
        let Some(burst) = burst_of(w, &r, d) else {
            continue;
        };
        let reached = recipients(w, &r, burst.range);
        let expiry = now + (burst.duration_ms * MSEC as f64) as i64;
        let table: Vec<(i64, f64, Aggregate)> = {
            let Some(t) = w.content().try_get::<Dbuffs>() else {
                continue;
            };
            burst
                .buffs
                .iter()
                .filter_map(|(c, v)| {
                    t.collection(*c)
                        .filter(|c| !c.is_empty())
                        .map(|col| (*c, *v, col.aggregate))
                })
                .collect()
        };
        if table.is_empty() {
            continue;
        }
        for ship in &reached {
            if w.roster().by_ship(*ship).is_none() {
                continue;
            }
            let state = w
                .ext_mut::<Boosts>()
                .ships
                .entry(*ship)
                .or_insert_with(|| ShipBuffs {
                    buffs: BTreeMap::new(),
                    told: Vec::new(),
                    applied: Vec::new(),
                });
            for (c, v, mode) in &table {
                match state.buffs.get_mut(c) {
                    Some(a) => a.merge(expiry, *v),
                    None => {
                        state.buffs.insert(*c, Aggregator::new(*mode, expiry, *v));
                    }
                }
            }
        }
        if seen.is_none() {
            let others = reached.iter().filter(|s| **s != r.ship_id).count() as i64;
            if others > 0 && w.character(r.character_id).is_some() {
                schedule(
                    w,
                    Boosted {
                        to: r.character_id,
                        module_type: r.type_id,
                        num_boosted: others,
                    },
                );
            }
        }
    }
}

/// One ship: its expired inputs dropped, its pilot told the new state, the
/// engine given the new buffs and the pilot told every attribute that moved.
fn settle(w: &mut Registry, ship: i64, now: BlueTime) {
    let Some(key) = w.roster().by_ship(ship).map(|p| p.character_id) else {
        // Gone (docked, jumped, died, logged off): the session change
        // re-primes the ship, and the client's tracker resets itself.
        w.ext_mut::<Boosts>().ships.remove(&ship);
        return;
    };
    let Some(mut state) = w.ext::<Boosts>().ships.get(&ship).cloned() else {
        return;
    };
    for a in state.buffs.values_mut() {
        a.discard_expired(now);
    }
    state.buffs.retain(|_, a| !a.is_empty());
    let entries: Vec<Entry> = state
        .buffs
        .iter()
        .filter_map(|(c, a)| Some((*c, a.value()?, a.final_expiry()?)))
        .collect();
    let resolved: Vec<Applied> = {
        let table = w.content().try_get::<Dbuffs>();
        entries
            .iter()
            .filter_map(|(c, v, _)| {
                let col = table?.collection(*c)?;
                Some(Applied {
                    collection: *c,
                    op: col.operation,
                    value: *v,
                    item: col.item.clone(),
                    location: col.location.clone(),
                    group: col.group.clone(),
                    skill: col.skill.clone(),
                })
            })
            .collect()
    };
    let pilot = w.character(key).is_some().then_some(key);
    if entries != state.told
        && let Some(to) = pilot
    {
        let stamp = w.next_destiny_stamp();
        schedule(
            w,
            DbuffUpdated {
                to,
                stamp,
                ship,
                state: entries.clone(),
            },
        );
    }
    if resolved != state.applied {
        let changes = changes_for(w, ship, &state.applied, &resolved);
        let speed = changes
            .iter()
            .any(|c| c.item == ship && c.attribute == attr::MAX_VELOCITY);
        if speed {
            // Integrated to now at the old speed, before the slot changes
            // what `dogma::hull` and `mover` answer (as `ewar`).
            w.advance_character(key);
        }
        state.applied = resolved;
        state.told = entries;
        let idle = state.buffs.is_empty() && state.told.is_empty();
        if idle {
            w.ext_mut::<Boosts>().ships.remove(&ship);
        } else {
            w.ext_mut::<Boosts>().ships.insert(ship, state);
        }
        if speed && let Some(h) = crate::dogma::hull(w, ship) {
            speed_push(w, key, ship, f64::from(h.max_velocity));
        }
        refresh_record(w, ship, key, now);
        if pilot.is_some() {
            for c in &changes {
                attribute_push(w, key, c, now);
            }
        }
        return;
    }
    state.told = entries;
    if state.buffs.is_empty() && state.told.is_empty() {
        w.ext_mut::<Boosts>().ships.remove(&ship);
    } else {
        w.ext_mut::<Boosts>().ships.insert(ship, state);
    }
}

/// A shield charge that moved by less than this is the curve's rounding, not
/// a change worth a push.
const RING_EPSILON: f64 = 1e-6;

/// **A ship that was already hit** keeps the profile of its first hit
/// (`combat::on_volley`), so the buffs reach it here: its record takes the
/// ship's profile now (hit points and resonances as the buffs leave them),
/// the park is told the bars, the pilot the shield's charge if the new
/// capacity clamped it. A ship nothing has hit has no record and is read
/// fresh at its first hit.
pub(crate) fn refresh_record(w: &mut Registry, ship: i64, key: i64, now: BlueTime) {
    if let Some(profile) = crate::dogma::profile(w, ship) {
        apply_profile(w, ship, key, now, profile);
    }
}

/// [`refresh_record`] with the profile in hand.
fn apply_profile(
    w: &mut Registry,
    ship: i64,
    key: i64,
    now: BlueTime,
    profile: crate::damage::Profile,
) {
    let Some(record) = w.combat_mut().ship_mut(ship) else {
        return;
    };
    let before = record.rings(now).shield_charge;
    record.set_profile(profile, now);
    let after = record.rings(now).shield_charge;
    let Some(here) = w.roster().get(key).copied() else {
        return;
    };
    let mut park = vec![key];
    park.extend(w.roster().others_in_system(here.solar_system_id, key));
    park.retain(|id| w.character(*id).is_some());
    let bars = crate::combat::damage_state(w, ship, now);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::DamageStateChange::new(park, stamp, ship, bars) {
        schedule(w, p);
    }
    if (before - after).abs() > RING_EPSILON && w.character(key).is_some() {
        schedule(
            w,
            crate::combat::RingUpdate {
                to: vec![key],
                owner_id: key,
                ship_id: ship,
                change: crate::damage::RingChange {
                    attribute_id: crate::damage::attr::SHIELD_CHARGE,
                    old: before,
                    new: after,
                },
            },
        );
    }
}

/// [`attribute_changes`] for a ship in the registry; none for a ship the
/// dogma engine has no sheet for (no typedata, or not a pilot's hull).
fn changes_for(w: &Registry, ship: i64, old: &[Applied], new: &[Applied]) -> Vec<Change> {
    let table = crate::typedata::installed_in(w.content()).table();
    let Some(inputs) = crate::dogma::inputs(w, ship) else {
        return Vec::new();
    };
    if table
        .get(inputs.ship_type)
        .is_none_or(|t| t.category_id != crate::typedata::CATEGORY_SHIP)
    {
        return Vec::new();
    }
    attribute_changes(table, ship, &inputs, old, new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dogma::ModuleInput;
    use crate::model::{Character, SandboxConfig};
    use crate::modules::Effect;

    // Invented ids and round fixture values (lane rule): the effect and
    // attribute ids are the client's own [V]; every collection id, type id
    // and number below is made up.
    const ALICE: i64 = 1_590_001;
    const BOB: i64 = 1_590_002;
    const CAROL: i64 = 1_590_003;
    const DAVE: i64 = 1_590_004;
    const ALICE_SHIP: i64 = 159_000_001;
    const BOB_SHIP: i64 = 159_000_002;
    const CAROL_SHIP: i64 = 159_000_003;
    const DAVE_SHIP: i64 = 159_000_004;
    const BURST_MOD: i64 = 159_100_001;
    const BURST_T: i64 = 990_590_001;

    /// Invented collections.
    const C_PCT_MIN: i64 = 990_591;
    const C_PCT_MAX: i64 = 990_592;
    const C_LOC: i64 = 990_593;
    const C_GROUP: i64 = 990_594;
    const C_SKILL: i64 = 990_595;
    /// Invented attributes and groups and skills.
    const A_SHIP: i64 = 990_601;
    const A_SHIP2: i64 = 990_602;
    const A_MOD: i64 = 990_603;
    const A_MOD2: i64 = 990_604;
    const G_GUN: i64 = 990_611;
    const G_OTHER: i64 = 990_612;
    const S_GUN: i64 = 990_621;
    const S_OTHER: i64 = 990_622;
    /// Invented types: the hull, a gun and a repairer.
    const HULL_T: i64 = 990_631;
    const GUN_T: i64 = 990_632;
    const REP_T: i64 = 990_633;

    const BURST_PCT: f64 = -8.0;
    const DURATION_MS: f64 = 60_000.0;
    const RANGE: f64 = 15_000.0;
    const PERIOD_MS: i64 = 30_000;
    const HULL_BASE: f64 = 1_000.0;

    /// The burst module's numbers: collection `C_PCT_MIN` at [`BURST_PCT`].
    struct Fixture {
        id_attr: i64,
        value_attr: i64,
        collection: i64,
        value: f64,
        duration: f64,
    }

    const PLAIN: Fixture = Fixture {
        id_attr: 2468,
        value_attr: 2469,
        collection: C_PCT_MIN,
        value: BURST_PCT,
        duration: DURATION_MS,
    };

    impl Data for Fixture {
        fn ship_attr(&self, _: &Registry, _: i64, _: i64) -> Option<f64> {
            None
        }
        fn module_attr(&self, _: &Registry, r: &Running, attribute_id: i64) -> Option<f64> {
            if r.module_id != BURST_MOD {
                return None;
            }
            match attribute_id {
                a if a == self.id_attr => Some(self.collection as f64),
                a if a == self.value_attr => Some(self.value),
                attr::BUFF_DURATION => Some(self.duration),
                attr::MAX_RANGE => Some(RANGE),
                _ => None,
            }
        }
        fn type_attr(&self, _: &Registry, _: i64, _: i64) -> Option<f64> {
            None
        }
    }

    fn table() -> String {
        // Operators: PostPercent (-> 6) and ModAdd (2) are the dogma ids.
        let row = |id: i64, op: &str, agg: &str| format!("{id}\t{op}\t{agg}\tShowNormal\tX\tx");
        let c = [
            row(C_PCT_MIN, "PostPercent", "Minimum"),
            row(C_PCT_MAX, "PostPercent", "Maximum"),
            row(C_LOC, "PostPercent", "Minimum"),
            row(C_GROUP, "PostPercent", "Minimum"),
            row(C_SKILL, "PostPercent", "Minimum"),
        ];
        c.join("\n")
    }

    fn dbuffs() -> Dbuffs {
        let mods = vec![
            format!("{C_PCT_MIN}\titem\t{A_SHIP}\t\t"),
            format!("{C_PCT_MAX}\titem\t{A_SHIP}\t\t"),
            format!("{C_LOC}\tlocation\t{A_MOD}\t\t"),
            format!("{C_GROUP}\tgroup\t{A_MOD}\t{G_GUN}\t"),
            format!("{C_SKILL}\tskill\t{A_MOD2}\t\t{S_GUN}"),
        ];
        let c: Vec<String> = table().lines().map(str::to_owned).collect();
        Dbuffs::parse(&c, &mods).expect("the fixture collections parse")
    }

    fn effect(effect_id: i64) -> Effect {
        Effect {
            effect_id,
            name: "fixture",
            category: 1,
            guid: "",
            is_offensive: false,
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

    /// Alice, Bob, Carol and Dave in one system; Bob 5 km and Carol 20 km
    /// from Alice (out of the burst's range), Dave 1 km but in no fleet.
    /// Alice, Bob and Carol in one fleet.
    fn world() -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let mut content = crate::content::Content::fixture();
        content.insert("dbuffs", dbuffs());
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                ch(ALICE, ALICE_SHIP, "Alice"),
                ch(BOB, BOB_SHIP, "Bob"),
                ch(CAROL, CAROL_SHIP, "Carol"),
                ch(DAVE, DAVE_SHIP, "Dave"),
            ],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content));
        for c in [ALICE, BOB, CAROL, DAVE] {
            crate::destiny::on_ballpark_bind(&mut reg, Some(c));
        }
        let origin = reg.roster().get(ALICE).expect("in space").motion.position;
        for (c, dx) in [
            (ALICE, 0.0),
            (BOB, 5_000.0),
            (CAROL, 20_000.0),
            (DAVE, 1_000.0),
        ] {
            let p = reg.roster_mut().get_mut(c).expect("in space");
            p.motion = crate::space_state::Motion::arrived(
                [origin[0] + dx, origin[1], origin[2]],
                [0.0; 3],
            );
            p.since = crate::space_state::now_seconds();
        }
        let id = reg.fleets_mut().create(ALICE).expect("created");
        reg.fleets_mut()
            .init(ALICE, id, Some(HULL_T), None, None)
            .expect("init");
        for who in [BOB, CAROL] {
            reg.fleets_mut()
                .invite(ALICE, id, who, None, None, None)
                .expect("invited");
            reg.fleets_mut()
                .accept_invite(who, id, Some(HULL_T), None, None)
                .expect("accepted");
        }
        let _ = reg.take_pending();
        let _ = reg.take_due(far());
        reg
    }

    /// Alice runs the burst module, its cycle `cycles`, started at `at`.
    fn run(reg: &mut Registry, effect_id: i64, cycles: i64, at: i64) {
        reg.modules_mut().insert_running(Running {
            character_id: ALICE,
            ship_id: ALICE_SHIP,
            module_id: BURST_MOD,
            type_id: BURST_T,
            effect: effect(effect_id),
            target_id: None,
            started: at,
            period: PERIOD_MS * MSEC,
            repeat: 1000,
            cycles,
            stop_at: None,
        });
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

    /// `(new, old)` of the `OnModuleAttributeChange` for `id`, if sent.
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

    /// The `OnDbuffUpdated` entries sent to `who`, decoded to
    /// `(ship, [(collection, value, expiry)])`.
    fn dbuff_told(sent: &[Sent], who: i64) -> Vec<(i64, Vec<Entry>)> {
        let mut out = Vec::new();
        for (n, a, to) in sent {
            if *n != "DoDestinyUpdate" || to.as_deref() != Some(&[who][..]) {
                continue;
            }
            let PyValue::List(batch) = &a[0] else {
                continue;
            };
            for e in batch {
                let PyValue::Tuple(e) = e else { continue };
                let PyValue::Tuple(call) = &e[1] else {
                    continue;
                };
                if call[0] != crate::pyutil::bstr("OnDbuffUpdated") {
                    continue;
                }
                let PyValue::Tuple(args) = &call[1] else {
                    continue;
                };
                let (PyValue::Int(ship), PyValue::List(list)) = (&args[0], &args[1]) else {
                    continue;
                };
                let rows = list
                    .iter()
                    .map(|row| {
                        let PyValue::Tuple(row) = row else {
                            panic!("a tuple")
                        };
                        let PyValue::Tuple(ve) = &row[1] else {
                            panic!("(value, expiry)")
                        };
                        match (&row[0], &ve[0], &ve[1]) {
                            (PyValue::Int(c), PyValue::Real(v), PyValue::Int(e)) => (*c, *v, *e),
                            other => panic!("shape: {other:?}"),
                        }
                    })
                    .collect();
                out.push((*ship, rows));
            }
        }
        out
    }

    // -- the aggregator ------------------------------------------------------

    /// A minimum keeps the stronger (lower) value while the weaker input
    /// outlives it only as the tail; an input no stronger and no longer
    /// lived is dropped.
    #[test]
    fn a_minimum_aggregator_keeps_the_stronger_and_the_longer() {
        let mut a = Aggregator::new(Aggregate::Minimum, 100, -8.0);
        // Weaker (-5) and shorter: dropped.
        a.merge(90, -5.0);
        assert_eq!(a.inputs, [(100, -8.0)]);
        // Weaker but lasts longer: queued after, the output is still -8.
        a.merge(200, -5.0);
        assert_eq!(a.inputs, [(100, -8.0), (200, -5.0)]);
        assert_eq!(a.value(), Some(-8.0));
        assert_eq!(a.final_expiry(), Some(200));
        // Stronger and longer: it replaces both.
        a.merge(300, -10.0);
        assert_eq!(a.inputs, [(300, -10.0)]);
        // The first expires, the next takes over.
        let mut b = Aggregator::new(Aggregate::Minimum, 100, -8.0);
        b.merge(200, -5.0);
        b.discard_expired(101);
        assert_eq!(b.value(), Some(-5.0));
        b.discard_expired(201);
        assert!(b.is_empty());
        assert_eq!(b.value(), None);
    }

    #[test]
    fn a_maximum_aggregator_is_the_mirror() {
        let mut a = Aggregator::new(Aggregate::Maximum, 100, 8.0);
        a.merge(90, 5.0);
        assert_eq!(a.inputs, [(100, 8.0)], "weaker and shorter: dropped");
        a.merge(200, 5.0);
        assert_eq!(a.inputs, [(100, 8.0), (200, 5.0)]);
        a.merge(300, 10.0);
        assert_eq!(a.inputs, [(300, 10.0)]);
        // An equal value that lasts longer replaces the shorter one.
        let mut b = Aggregator::new(Aggregate::Maximum, 100, 8.0);
        b.merge(150, 8.0);
        assert_eq!(b.inputs, [(150, 8.0)]);
        // An expiry exactly now still counts (`expiry < now` discards).
        b.discard_expired(150);
        assert!(!b.is_empty());
    }

    // -- the burst -----------------------------------------------------------

    #[test]
    fn the_five_burst_effects_are_the_clients() {
        for e in EFFECTS {
            assert!(is_burst(e));
        }
        assert!(!is_burst(6737), "the charge's own effect is not a burst");
        assert!(!is_burst(6753), "a titan generator is not here");
    }

    #[test]
    fn a_burst_is_read_off_the_modified_module() {
        let mut reg = world();
        run(&mut reg, EFFECT_ARMOR, 1, now());
        let r = *reg.modules().running(BURST_MOD).expect("running");
        let b = burst_of(&reg, &r, &PLAIN).expect("a burst");
        assert_eq!(b.buffs, [(C_PCT_MIN, BURST_PCT)]);
        assert_eq!(b.duration_ms, DURATION_MS);
        assert_eq!(b.range, RANGE);
        // No collection id (no charge): nothing to apply.
        let bare = Fixture {
            collection: 0,
            ..PLAIN
        };
        assert_eq!(burst_of(&reg, &r, &bare), None);
        // No duration: nothing either.
        let none = Fixture {
            duration: 0.0,
            ..PLAIN
        };
        assert_eq!(burst_of(&reg, &r, &none), None);
        // The fourth pair is read like the first.
        let fourth = Fixture {
            id_attr: 2536,
            value_attr: 2537,
            ..PLAIN
        };
        assert_eq!(
            burst_of(&reg, &r, &fourth).map(|b| b.buffs),
            Some(vec![(C_PCT_MIN, BURST_PCT)])
        );
    }

    #[test]
    fn a_burst_reaches_the_booster_and_the_fleet_in_range_only() {
        let mut reg = world();
        run(&mut reg, EFFECT_SHIELD, 1, now());
        let r = *reg.modules().running(BURST_MOD).expect("running");
        assert_eq!(
            recipients(&reg, &r, RANGE),
            [ALICE_SHIP, BOB_SHIP],
            "Carol is 20 km out, Dave is in no fleet"
        );
        assert_eq!(
            recipients(&reg, &r, 25_000.0),
            [ALICE_SHIP, BOB_SHIP, CAROL_SHIP]
        );
        assert_eq!(
            recipients(&reg, &r, 0.0),
            [ALICE_SHIP],
            "the booster always"
        );
        // A booster in no fleet boosts himself alone.
        let id = reg
            .fleets()
            .fleet_of(ALICE)
            .map(|f| f.fleet_id)
            .expect("in one");
        reg.fleets_mut().leave(ALICE, id).expect("leaves");
        assert_eq!(recipients(&reg, &r, RANGE), [ALICE_SHIP]);
    }

    #[test]
    fn a_pilot_in_another_system_is_not_reached() {
        let mut reg = world();
        reg.roster_mut()
            .get_mut(BOB)
            .expect("in space")
            .solar_system_id += 1;
        run(&mut reg, EFFECT_INFO, 1, now());
        let r = *reg.modules().running(BURST_MOD).expect("running");
        assert_eq!(recipients(&reg, &r, RANGE), [ALICE_SHIP]);
    }

    // -- the pulse and the clock --------------------------------------------

    #[test]
    fn a_pulse_tells_each_recipient_and_the_booster_once() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let sent = pushes(&mut reg);
        let expiry = t + (DURATION_MS * MSEC as f64) as i64;
        assert_eq!(
            dbuff_told(&sent, ALICE),
            [(ALICE_SHIP, vec![(C_PCT_MIN, BURST_PCT, expiry)])]
        );
        assert_eq!(
            dbuff_told(&sent, BOB),
            [(BOB_SHIP, vec![(C_PCT_MIN, BURST_PCT, expiry)])]
        );
        assert!(dbuff_told(&sent, CAROL).is_empty(), "out of range");
        assert!(dbuff_told(&sent, DAVE).is_empty(), "in no fleet");
        // The booster hears how many others were boosted, once.
        let boosted: Vec<_> = sent
            .iter()
            .filter(|(n, ..)| *n == "OnFleetBoosted")
            .collect();
        assert_eq!(boosted.len(), 1, "{sent:?}");
        assert_eq!(
            boosted[0].1,
            [PyValue::Int(BURST_T), PyValue::Int(1)],
            "one other ship, not the booster"
        );
        assert_eq!(boosted[0].2, Some(vec![ALICE]));
        // The same cycle again changes nothing and tells nothing.
        reconcile(&mut reg, t + MSEC, &PLAIN);
        assert!(pushes(&mut reg).is_empty());
        assert_eq!(reg.ext::<Boosts>().boosted(), [ALICE_SHIP, BOB_SHIP]);
    }

    /// FX-REPLAY: a `SetState`'s `dbuffState` is what the pilot was last told.
    #[test]
    fn state_value_of_is_the_told_state_in_the_wire_shape() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let _ = pushes(&mut reg);
        let expiry = t + (DURATION_MS * MSEC as f64) as i64;
        assert_eq!(
            state_value_of(&reg, BOB_SHIP),
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(C_PCT_MIN),
                PyValue::Tuple(vec![PyValue::Real(BURST_PCT), PyValue::Int(expiry)]),
            ])])
        );
        assert_eq!(state_value_of(&reg, CAROL_SHIP), PyValue::List(vec![]));
    }

    #[test]
    fn the_buff_falls_off_after_its_duration_and_the_pilot_is_told() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let _ = pushes(&mut reg);
        let end = t + (DURATION_MS * MSEC as f64) as i64;
        // Still on at its last moment.
        reconcile(&mut reg, end, &PLAIN);
        assert!(dbuff_told(&pushes(&mut reg), BOB).is_empty());
        assert_eq!(applied(&reg, BOB_SHIP).len(), 1);
        // Gone a tick after: the state is falsy (an empty list) and the
        // engine's list is empty.
        reconcile(&mut reg, end + 1, &PLAIN);
        let sent = pushes(&mut reg);
        assert_eq!(dbuff_told(&sent, BOB), [(BOB_SHIP, vec![])]);
        assert_eq!(dbuff_told(&sent, ALICE), [(ALICE_SHIP, vec![])]);
        assert!(applied(&reg, BOB_SHIP).is_empty());
        assert!(
            reg.ext::<Boosts>().boosted().is_empty(),
            "nothing left held"
        );
    }

    #[test]
    fn each_cycle_pulses_again_and_extends_the_buff() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let _ = pushes(&mut reg);
        // The second cycle, half a minute on.
        let t2 = t + PERIOD_MS * MSEC;
        run(&mut reg, EFFECT_ARMOR, 2, t);
        reconcile(&mut reg, t2, &PLAIN);
        let sent = pushes(&mut reg);
        let expiry = t2 + (DURATION_MS * MSEC as f64) as i64;
        assert_eq!(
            dbuff_told(&sent, BOB),
            [(BOB_SHIP, vec![(C_PCT_MIN, BURST_PCT, expiry)])],
            "the same value, a later end"
        );
        assert!(
            !sent.iter().any(|(n, ..)| *n == "OnFleetBoosted"),
            "the prompt is for the first pulse"
        );
    }

    #[test]
    fn a_stopped_burst_leaves_its_buffs_to_run_out() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let _ = pushes(&mut reg);
        reg.modules_mut().forget(BURST_MOD);
        reconcile(&mut reg, t + MSEC, &PLAIN);
        assert_eq!(applied(&reg, BOB_SHIP).len(), 1, "still on");
        assert!(reg.ext::<Boosts>().cycles.is_empty());
        // Started again: a first pulse again, and the prompt again.
        run(&mut reg, EFFECT_ARMOR, 1, t + 2 * MSEC);
        reconcile(&mut reg, t + 2 * MSEC, &PLAIN);
        assert!(
            pushes(&mut reg)
                .iter()
                .any(|(n, ..)| *n == "OnFleetBoosted")
        );
    }

    #[test]
    fn a_burst_that_has_stopped_taking_hold_does_not_pulse() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t - MSEC);
        let mut r = *reg.modules().running(BURST_MOD).expect("running");
        r.stop_at = Some(t - 1);
        reg.modules_mut().insert_running(r);
        reconcile(&mut reg, t, &PLAIN);
        assert!(pushes(&mut reg).is_empty());
        assert!(reg.ext::<Boosts>().boosted().is_empty());
    }

    #[test]
    fn a_ship_that_leaves_loses_its_buffs() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &PLAIN);
        let _ = pushes(&mut reg);
        reg.roster_mut().leave(BOB);
        reconcile(&mut reg, t + MSEC, &PLAIN);
        assert_eq!(reg.ext::<Boosts>().boosted(), [ALICE_SHIP]);
        assert!(applied(&reg, BOB_SHIP).is_empty());
    }

    #[test]
    fn a_collection_the_table_lacks_is_ignored() {
        let mut reg = world();
        let t = now();
        run(&mut reg, EFFECT_ARMOR, 1, t);
        let unknown = Fixture {
            collection: 990_999,
            ..PLAIN
        };
        reconcile(&mut reg, t, &unknown);
        assert!(pushes(&mut reg).is_empty());
        assert!(reg.ext::<Boosts>().boosted().is_empty());
    }

    #[test]
    fn two_bursts_of_one_collection_aggregate_per_the_collection() {
        // Two boosters' pulses on one ship: the Maximum collection keeps the
        // larger, the Minimum the lower. Driven through the aggregators the
        // pulse builds.
        let mut reg = world();
        let t = now();
        let expiry = t + (DURATION_MS * MSEC as f64) as i64;
        let state = ShipBuffs {
            buffs: BTreeMap::from([
                (C_PCT_MAX, Aggregator::new(Aggregate::Maximum, expiry, 5.0)),
                (C_PCT_MIN, Aggregator::new(Aggregate::Minimum, expiry, -5.0)),
            ]),
            told: Vec::new(),
            applied: Vec::new(),
        };
        reg.ext_mut::<Boosts>().ships.insert(BOB_SHIP, state);
        let more = Fixture {
            collection: C_PCT_MAX,
            value: 9.0,
            ..PLAIN
        };
        run(&mut reg, EFFECT_ARMOR, 1, t);
        reconcile(&mut reg, t, &more);
        let a = applied(&reg, BOB_SHIP);
        let max = a.iter().find(|a| a.collection == C_PCT_MAX).expect("there");
        assert_eq!(max.value, 9.0, "a maximum keeps the larger");
        let min = a.iter().find(|a| a.collection == C_PCT_MIN).expect("there");
        assert_eq!(min.value, -5.0, "the other collection is untouched");
    }

    // -- a ship that was already hit ------------------------------------------

    fn profile(shield: f64, armor: f64) -> crate::damage::Profile {
        crate::damage::Profile {
            shield_capacity: shield,
            shield_recharge_rate: 600_000.0,
            armor_hp: armor,
            hp: 500.0,
            shield_resonance: crate::damage::Damage::default(),
            armor_resonance: crate::damage::Damage::default(),
            hull_resonance: crate::damage::Damage::default(),
        }
    }

    /// A buff that raises the pool: the record takes the new profile, the
    /// park is told the bars, and the shield charge (absolute) is unchanged.
    #[test]
    fn a_hit_ships_record_takes_the_buffed_profile_and_the_park_is_told() {
        let mut reg = world();
        let t = now();
        let mut hurt = crate::damage::ShipDamage::full(profile(400.0, 300.0), t);
        hurt.set_profile(profile(400.0, 300.0), t);
        reg.combat_mut().set_ship(BOB_SHIP, hurt);
        apply_profile(&mut reg, BOB_SHIP, BOB, t, profile(440.0, 330.0));
        let record = *reg.combat().ship(BOB_SHIP).expect("a record");
        let rings = record.rings(t);
        assert_eq!(rings.shield_capacity, 440.0);
        assert_eq!(rings.armor_hp, 330.0);
        assert!(
            (rings.shield_charge - 400.0).abs() < RING_EPSILON,
            "the charge is the absolute number"
        );
        let sent = pushes(&mut reg);
        assert!(
            sent.iter().any(|(n, _, to)| *n == "DoDestinyUpdate"
                && to.as_ref().is_some_and(|t| t.contains(&BOB))),
            "the bars: {sent:?}"
        );
        assert!(
            !sent.iter().any(|(n, ..)| *n == "OnModuleAttributeChange"),
            "no ring: nothing clamped"
        );
    }

    /// A buff that ends: the pool shrinks, the charge is clamped to it and
    /// the pilot is told the ring.
    #[test]
    fn an_ending_buff_clamps_the_shield_charge_and_tells_the_ring() {
        let mut reg = world();
        let t = now();
        reg.combat_mut().set_ship(
            BOB_SHIP,
            crate::damage::ShipDamage::full(profile(440.0, 330.0), t),
        );
        apply_profile(&mut reg, BOB_SHIP, BOB, t, profile(400.0, 300.0));
        let record = *reg.combat().ship(BOB_SHIP).expect("a record");
        assert_eq!(record.rings(t).shield_charge, 400.0);
        let sent = pushes(&mut reg);
        let ring = attribute_told(&sent, crate::damage::attr::SHIELD_CHARGE);
        assert_eq!(ring, Some((400.0, 440.0)), "{sent:?}");
    }

    /// A ship nothing has hit has no record to refresh.
    #[test]
    fn a_ship_nothing_has_hit_has_no_record_to_refresh() {
        let mut reg = world();
        apply_profile(&mut reg, BOB_SHIP, BOB, now(), profile(440.0, 330.0));
        assert!(reg.combat().ship(BOB_SHIP).is_none());
        assert!(pushes(&mut reg).is_empty());
    }

    // -- the push shape ------------------------------------------------------

    #[test]
    fn the_dbuff_update_is_one_destiny_entry_with_the_clients_shape() {
        let p = DbuffUpdated {
            to: ALICE,
            stamp: 7,
            ship: ALICE_SHIP,
            state: vec![(C_PCT_MIN, -8.0, 123)],
        };
        let (name, args) = p.to_push();
        assert_eq!(name, "DoDestinyUpdate");
        assert_eq!(args[1], PyValue::Int(0));
        let PyValue::List(batch) = &args[0] else {
            panic!("a list")
        };
        assert_eq!(batch.len(), 1);
        assert_eq!(
            batch[0],
            PyValue::Tuple(vec![
                PyValue::Int(7),
                PyValue::Tuple(vec![
                    crate::pyutil::bstr("OnDbuffUpdated"),
                    PyValue::Tuple(vec![
                        PyValue::Int(ALICE_SHIP),
                        PyValue::List(vec![PyValue::Tuple(vec![
                            PyValue::Int(C_PCT_MIN),
                            PyValue::Tuple(vec![PyValue::Real(-8.0), PyValue::Int(123)]),
                        ])]),
                    ]),
                ]),
            ])
        );
        assert_eq!(p.audience(), Some(vec![ALICE]));
    }

    // -- the engine: one test per kind of modifier ---------------------------

    /// A hull with one ship attribute, a gun (a group and a skill) and a
    /// repairer (another group, no skill), each with one module attribute.
    fn typedata_rows() -> String {
        let m = |mid: i64| (mid, mid);
        let _ = m;
        format!(
            "\
attribute\t{A_SHIP}\t0.0\tshipAttr\n\
attribute\t{A_SHIP2}\t0.0\tshipAttr2\n\
attribute\t{A_MOD}\t0.0\tmodAttr\n\
attribute\t{A_MOD2}\t0.0\tmodAttr2\n\
attrmeta\t{A_SHIP}\t1\t1\t0\t0\t0\n\
attrmeta\t{A_SHIP2}\t0\t1\t0\t0\t0\n\
attrmeta\t{A_MOD}\t1\t1\t0\t0\t0\n\
attrmeta\t{A_MOD2}\t1\t1\t0\t0\t0\n\
type\t{HULL_T}\t25\t6\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t{A_SHIP}={HULL_BASE};{A_SHIP2}={HULL_BASE}\tFixture Hull\n\
type\t{GUN_T}\t{G_GUN}\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t{A_MOD}=100.0;{A_MOD2}=100.0;182={S_GUN}.0\tFixture Gun\n\
type\t{REP_T}\t{G_OTHER}\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t{A_MOD}=100.0;{A_MOD2}=100.0;182={S_OTHER}.0\tFixture Rep\n",
        )
    }

    fn engine_table() -> crate::typedata::Table {
        crate::typedata::Table::parse(&typedata_rows()).expect("the fixture parses")
    }

    fn engine_inputs() -> Inputs {
        Inputs {
            ship_type: HULL_T,
            modules: vec![
                ModuleInput {
                    item_id: 1,
                    type_id: GUN_T,
                    state: crate::dogma::State::Online,
                    charge_type: None,
                },
                ModuleInput {
                    item_id: 2,
                    type_id: REP_T,
                    state: crate::dogma::State::Online,
                    charge_type: None,
                },
            ],
            ..Inputs::default()
        }
    }

    fn one(c: i64, op: i64, value: f64) -> Applied {
        let t = dbuffs();
        let col = t.collection(c).expect("a collection");
        Applied {
            collection: c,
            op,
            value,
            item: col.item.clone(),
            location: col.location.clone(),
            group: col.group.clone(),
            skill: col.skill.clone(),
        }
    }

    fn changed(c: &[Change], item: i64, attribute: i64) -> Option<(f64, f64)> {
        c.iter()
            .find(|c| c.item == item && c.attribute == attribute)
            .map(|c| (c.old, c.new))
    }

    /// **Item modifier**: the ship's own attribute, post-percent.
    #[test]
    fn an_item_modifier_moves_the_ships_attribute() {
        let t = engine_table();
        let b = one(C_PCT_MIN, crate::dogma::op::POST_PERCENT, BURST_PCT);
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &[b]);
        assert_eq!(
            changed(&c, ALICE_SHIP, A_SHIP),
            Some((HULL_BASE, HULL_BASE * (100.0 + BURST_PCT) / 100.0))
        );
        assert_eq!(c.len(), 1, "nothing else moved: {c:?}");
    }

    /// **A booster on the pilot** moves the ship's attributes (BOOST): the
    /// same diff, over two inputs a booster apart, and a side effect that was
    /// not rolled moves nothing.
    #[test]
    fn a_booster_taken_or_run_out_moves_the_ships_attribute() {
        const BOOSTER_T: i64 = 990_492_011;
        const MAIN: i64 = 990_492_901;
        const SIDE: i64 = 990_492_902;
        let text = format!(
            "{}type\t{BOOSTER_T}\t303\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t{MAIN},{SIDE}\t\
             1087=1.0;177=10.0;178=-50.0;1089=1.0\tFixture Booster\n\
             effect\t{MAIN}\t0\t0\t0\t0\t0\t0\t0\tfixtureMain\t\t1\t0\n\
             modifier\t{MAIN}\tshipID\tItemModifier\t{A_SHIP}\t177\t6\t\t\n\
             effect\t{SIDE}\t0\t0\t0\t0\t0\t0\t0\tfixtureSide\t\t1\t1089\n\
             modifier\t{SIDE}\tshipID\tItemModifier\t{A_SHIP}\t178\t6\t\t\n",
            typedata_rows()
        );
        let t = crate::typedata::Table::parse(&text).expect("parses");
        let with = |side: Vec<i64>| Inputs {
            boosters: vec![crate::dogma::BoosterInput {
                type_id: BOOSTER_T,
                side_effects: side,
            }],
            ..engine_inputs()
        };
        let none = engine_inputs();
        let unrolled = attribute_changes_inputs(&t, ALICE_SHIP, &none, &with(vec![]), &[]);
        assert_eq!(
            changed(&unrolled, ALICE_SHIP, A_SHIP),
            Some((HULL_BASE, HULL_BASE * 1.1)),
            "the main effect only"
        );
        let rolled = attribute_changes_inputs(&t, ALICE_SHIP, &none, &with(vec![SIDE]), &[]);
        assert_eq!(
            changed(&rolled, ALICE_SHIP, A_SHIP),
            Some((HULL_BASE, HULL_BASE * 1.1 * 0.5)),
            "the side effect too"
        );
        // Running out is the same diff backwards.
        let back = attribute_changes_inputs(&t, ALICE_SHIP, &with(vec![]), &none, &[]);
        assert_eq!(
            changed(&back, ALICE_SHIP, A_SHIP),
            Some((HULL_BASE * 1.1, HULL_BASE))
        );
        assert!(attribute_changes_inputs(&t, ALICE_SHIP, &none, &none, &[]).is_empty());
    }

    /// **Location modifier**: every fitted module's attribute.
    #[test]
    fn a_location_modifier_moves_every_fitted_module() {
        let t = engine_table();
        let b = one(C_LOC, crate::dogma::op::POST_PERCENT, 10.0);
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &[b]);
        assert_eq!(changed(&c, 1, A_MOD), Some((100.0, 110.0)));
        assert_eq!(changed(&c, 2, A_MOD), Some((100.0, 110.0)));
        assert_eq!(changed(&c, ALICE_SHIP, A_MOD), None, "not the hull");
    }

    /// **Location-group modifier**: the modules of one group.
    #[test]
    fn a_group_modifier_moves_only_that_group() {
        let t = engine_table();
        let b = one(C_GROUP, crate::dogma::op::POST_PERCENT, -20.0);
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &[b]);
        assert_eq!(changed(&c, 1, A_MOD), Some((100.0, 80.0)));
        assert_eq!(changed(&c, 2, A_MOD), None, "the other group");
    }

    /// **Location-required-skill modifier**: the modules that need the skill.
    #[test]
    fn a_skill_modifier_moves_only_what_needs_the_skill() {
        let t = engine_table();
        let b = one(C_SKILL, crate::dogma::op::POST_PERCENT, 50.0);
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &[b]);
        assert_eq!(changed(&c, 1, A_MOD2), Some((100.0, 150.0)));
        assert_eq!(changed(&c, 2, A_MOD2), None, "needs another skill");
    }

    /// Every operator of the nine the table can name does what the dogma
    /// engine's `operate` says, on the ship's own attribute.
    #[test]
    fn each_operator_applies_through_the_engine() {
        use crate::dogma::{op, operate};
        let t = engine_table();
        for (o, v) in [
            (op::PRE_ASSIGN, 7.0),
            (op::PRE_MUL, 1.5),
            (op::PRE_DIV, 4.0),
            (op::MOD_ADD, 25.0),
            (op::MOD_SUB, 25.0),
            (op::POST_MUL, 0.5),
            (op::POST_DIV, 2.0),
            (op::POST_PERCENT, 12.0),
            (op::POST_ASSIGN, 33.0),
        ] {
            // The unpenalised attribute (`A_SHIP2` is not stackable).
            let mut b = one(C_PCT_MIN, o, v);
            b.item = vec![A_SHIP2];
            let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &[b]);
            let want = operate(o, HULL_BASE, v);
            if want == HULL_BASE {
                assert!(c.is_empty(), "operator {o}");
            } else {
                assert_eq!(
                    changed(&c, ALICE_SHIP, A_SHIP2),
                    Some((HULL_BASE, want)),
                    "operator {o}"
                );
            }
        }
    }

    /// The buff's source has no item, so a stackable-false attribute
    /// penalises a second buff on it (`_ShouldNotNurf`), and a stackable one
    /// does not.
    #[test]
    fn a_second_buff_is_stacking_penalised_on_a_non_stackable_attribute() {
        let t = engine_table();
        let on = |c: i64, v: f64, a: i64| {
            let mut b = one(c, crate::dogma::op::POST_PERCENT, v);
            b.item = vec![a];
            b
        };
        // `A_SHIP2` is not stackable.
        let both = [on(C_PCT_MIN, -20.0, A_SHIP2), on(C_PCT_MAX, -20.0, A_SHIP2)];
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &both);
        let (_, got) = changed(&c, ALICE_SHIP, A_SHIP2).expect("moved");
        let mods = [crate::dogma::Incoming {
            op: crate::dogma::op::POST_PERCENT,
            value: -20.0,
            penalised: true,
        }; 2];
        assert_eq!(got, crate::dogma::apply(HULL_BASE, &mods));
        assert!(got > HULL_BASE * 0.8 * 0.8, "the second counts for less");
        // The same two buffs on the stackable attribute multiply in full.
        let both = [on(C_PCT_MIN, -20.0, A_SHIP), on(C_PCT_MAX, -20.0, A_SHIP)];
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[], &both);
        let (_, full) = changed(&c, ALICE_SHIP, A_SHIP).expect("moved");
        assert_eq!(full, HULL_BASE * 0.8 * 0.8);
    }

    /// The way back: when the buff is gone the attribute reads its base again
    /// even though no modifier aims at it any more.
    #[test]
    fn an_expired_buff_reads_the_base_value_back() {
        let t = engine_table();
        let b = one(C_LOC, crate::dogma::op::POST_PERCENT, 10.0);
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[b], &[]);
        assert_eq!(changed(&c, 1, A_MOD), Some((110.0, 100.0)));
        // An attribute on no type row and in no reads falls to its default.
        let mut other = one(C_LOC, crate::dogma::op::POST_PERCENT, 10.0);
        other.location = vec![990_699];
        let c = attribute_changes(&t, ALICE_SHIP, &engine_inputs(), &[other], &[]);
        assert_eq!(changed(&c, 1, 990_699), None, "0 -> 0 is no change");
    }

    #[test]
    fn nothing_changes_when_the_lists_are_equal() {
        let t = engine_table();
        let b = one(C_PCT_MIN, crate::dogma::op::POST_PERCENT, BURST_PCT);
        let c = attribute_changes(
            &t,
            ALICE_SHIP,
            &engine_inputs(),
            std::slice::from_ref(&b),
            std::slice::from_ref(&b),
        );
        assert!(c.is_empty());
    }
}
