//! Carrier fighters: the fighter bay, the launch tubes, load and unload,
//! launch and recall (FT-B and FT-C, `docs/lanes/ft-b.md`; spec R-DEEP §2).
//!
//! **The item is the squadron.** A fighter stack lives in the ship's fighter
//! bay (flag 158). `LoadFightersToTube` moves up to `fighterSquadronMaxSize`
//! of it into a tube (flag 159..=163, the same item or a split of it, its
//! `quantity` the squadron size); `LaunchFightersFromTubes` moves that item
//! out to the solar system and puts **one ball** in the roster under its
//! item id; `RecallFightersToTubes` flies it home and moves the item back
//! into its tube. "Bay + tubes + space = what the player owns" stays the
//! store's invariant, exactly as for drones (`crate::drones`).
//!
//! What is persistent: the items. What is not: the tube states
//! (`LOADING`, `LAUNCHING`, ...) and the squadrons' balls. A tube holding an
//! item and no task is `READY`; a restart leaves squadron items in space, and
//! [`sweep_orphans`] puts them in their owner's home hangar.
//!
//! The client's view is five pushes (`OnFighterTubeContentUpdate`,
//! `OnFighterTubeContentEmpty`, `OnFighterTubeTaskStatus`,
//! `OnFighterAddedToController`, `OnFighterRemovedFromController`; all [V],
//! `findings/push-events.tsv`, `shipFighterState.py`) and the ordinary
//! `OnItemChange`, balls and slims. Every timing is ours ([A], below): the
//! client only draws the progress bars from the times we send.

use crate::content::Content;
use crate::damage::BlueTime;
use crate::destiny::hull::Hull;
use crate::destiny::{self, Action};
use crate::fighters_data::{self, Class, FighterType, HullSlots};
use crate::item::{Edit, Slot};
use crate::notify::{Addressed, Push};
use crate::service::{Registry, Request, ServiceError};
use crate::space_state::{Motion, Presence};
use crate::typedata;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

mod abandon;
mod ability;
mod effects;
#[cfg(test)]
mod fixture;
mod skills;
mod strike;

pub use effects::{jammed_at, scrammed, web_factor};

/// `const.flagFighterBay` [V `inventorycommon/const.py:192`], also
/// `invCtrl.ShipFighterBay.locationFlag` [V `invControllers.py:657`].
pub const FLAG_FIGHTER_BAY: i64 = 158;
/// `const.fighterTubeFlags`, tube 0 first [V `inventorycommon/const.py:192-197`].
pub const FIRST_TUBE: i64 = 159;
pub const LAST_TUBE: i64 = 163;

/// `fighters.MAX_SCOOP_DISTANCE` [V `fighters/__init__.py:24`]; a squadron
/// this near its carrier needs no flight home.
pub const LAND_DISTANCE: f64 = 5000.0;
/// `fighters.DEFAULT_CONTROLLER_ORBIT_DISTANCE` [V `fighters/__init__.py:25`]:
/// where a fresh squadron orbits when its type states no orbit range.
pub const DEFAULT_ORBIT: f64 = 1500.0;
/// `UE_LOC` [V `carbon/common/lib/const.py:273`]: a localisation label as a
/// `UserError` argument.
const UE_LOC: i64 = 101;

const BLUE_PER_MS: i64 = 10_000;
/// Task durations. **[A]** R-DEEP §2.6: the client only shows progress from
/// the times we send, nothing in the build states them. Falsifier: the bar
/// visibly ends before or after the animation.
pub const LOAD_MS: i64 = 2_000;
pub const UNLOAD_MS: i64 = 2_000;
pub const LAUNCH_MS: i64 = 3_000;
pub const LAND_MS: i64 = 2_000;
/// A recall flies at the squadron's top speed for its distance, between
/// these bounds [A].
pub const RECALL_MIN_MS: i64 = 1_000;
pub const RECALL_MAX_MS: i64 = 15_000;
/// Metres off the carrier's hull a squadron appears [A], `drones::LAUNCH_CLEARANCE`'s.
pub const LAUNCH_CLEARANCE: f64 = 150.0;
/// A squadron launches at full throttle round the carrier [A].
pub const LAUNCH_SPEED_FRACTION: f64 = 1.0;

fn ms(n: i64) -> BlueTime {
    n * BLUE_PER_MS
}

/// Whether `flag` is one of the five tube flags.
pub fn is_tube_flag(flag: i64) -> bool {
    (FIRST_TUBE..=LAST_TUBE).contains(&flag)
}

/// The tube index (0..5) of a tube flag.
pub fn tube_index(flag: i64) -> i64 {
    flag - FIRST_TUBE
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A refusal in the client's own words: a `UserError` key and its argument
/// dict. Raised on a call the client does not read a reply from
/// (`LoadFightersToTube`, `Add` into the bay), and returned **as a value**
/// inside the reply dict of the two calls that read one (`Launch`, `Recall`;
/// `fightersSvc.py:104-134` calls `error.args` on each) [V].
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub key: &'static str,
    pub args: Vec<(&'static str, PyValue)>,
}

impl Refusal {
    fn key(key: &'static str) -> Refusal {
        Refusal {
            key,
            args: Vec::new(),
        }
    }

    fn with(key: &'static str, args: Vec<(&'static str, PyValue)>) -> Refusal {
        Refusal { key, args }
    }

    /// `CustomNotify`, the escape hatch for what is about this server.
    fn text(text: &str) -> Refusal {
        Refusal::with("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
    }

    /// Raised.
    pub fn to_error(&self) -> ServiceError {
        crate::modules::user_error(self.key, self.args.clone())
    }

    /// As a value in a reply dict: `eveexceptions.UserError(key, dict)`.
    /// **[A]** that the client reads a `UserError` marshalled inside a dict
    /// the way it reads one raised; falsifier: the client logs a `TypeError`
    /// in `LaunchFightersFromTubes` on the first refusal.
    pub fn to_value(&self) -> PyValue {
        eve_proto::user_error(self.key, crate::pyutil::pypairs(self.args.clone()))
    }
}

fn class_label(class: Class) -> &'static str {
    match class {
        Class::Light => "UI/Fighters/Class/Light",
        Class::Support => "UI/Fighters/Class/Support",
        Class::Heavy => "UI/Fighters/Class/Heavy",
    }
}

fn class_args(class: Class, type_id: i64) -> Vec<(&'static str, PyValue)> {
    vec![
        ("newType", PyValue::Int(type_id)),
        (
            "className",
            PyValue::Tuple(vec![
                PyValue::Int(UE_LOC),
                crate::pyutil::bstr(class_label(class)),
            ]),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Type data
// ---------------------------------------------------------------------------

/// A fighter type's numbers, from its attributes (`typedata`, category 87).
pub fn fighter_type(content: &Content, type_id: i64) -> Option<FighterType> {
    let row = typedata::installed_in(content).table().get(type_id)?;
    if row.category_id != fighters_data::CATEGORY_FIGHTER {
        return None;
    }
    fighters_data::fighter_type(|a| row.attribute(a))
}

/// A hull's tubes, class slots and bay (attributes 2216-2219, 2737-2739,
/// 2055).
pub fn hull_slots(content: &Content, ship_type_id: i64) -> HullSlots {
    match typedata::installed_in(content).table().get(ship_type_id) {
        Some(row) => HullSlots::from_attrs(|a| row.attribute(a)),
        None => HullSlots::default(),
    }
}

/// Cubic metres one fighter takes (the type's `volume`).
fn volume_of(w: &Registry, type_id: i64) -> f64 {
    typedata::item_in(w.content(), type_id)
        .or_else(|| crate::item::find_in(w.content(), type_id))
        .map_or(0.0, |t| t.volume)
}

/// The m3 in a ship's fighter bay. Capacity is the hull's `fighterCapacity`
/// (attribute 2055), `volume * stacksize` per stack [V R-DEEP §2.2 rule 5].
pub fn bay_used(w: &Registry, ship_id: i64) -> f64 {
    w.store()
        .contents_of(ship_id)
        .filter(|i| i.flag_id == FLAG_FIGHTER_BAY)
        .map(|i| volume_of(w, i.type_id) * i.quantity as f64)
        .sum()
}

/// A ship's fighter bay capacity **as fitted**: the hull's `fighterCapacity`
/// (2055) with Fighter Hangar Management and anything else the dogma engine
/// puts on it ([`skills::ship_slots`], FT-H).
pub fn bay_capacity(w: &Registry, ship_id: i64) -> f64 {
    skills::ship_slots(w, ship_id).capacity
}

/// `{capacity, used}` of a ship's fighter bay, for `GetCapacity(flag=158)`.
pub fn bay_capacity_value(w: &Registry, ship_id: i64) -> PyValue {
    crate::pyutil::keyval(vec![
        ("capacity", PyValue::Real(bay_capacity(w, ship_id))),
        ("used", PyValue::Real(bay_used(w, ship_id))),
    ])
}

fn no_room(available: f64, volume: f64) -> Refusal {
    // `[A]` the args are free and wanted m3, as `NotEnoughDroneBaySpace`'s
    // are (`drones::no_room`); the client recomputes them itself
    // (`invControllers._AddItem`, `NotEnoughFighterBaySpace` is in its list).
    Refusal::with(
        "NotEnoughFighterBaySpace",
        vec![
            ("available", PyValue::Real(available)),
            ("volume", PyValue::Real(volume)),
        ],
    )
}

// ---------------------------------------------------------------------------
// The fighter bay: into it, out of it
// ---------------------------------------------------------------------------

/// Whether an item sits in a fighter bay: routes a drop on the hangar here.
pub fn in_a_bay(w: &Registry, item_id: i64) -> bool {
    w.store()
        .get(item_id)
        .is_some_and(|i| i.flag_id == FLAG_FIGHTER_BAY)
}

fn parked_here(w: &Registry, owner: i64, station_id: i64, ship_id: i64) -> bool {
    w.store().get(ship_id).is_some_and(|s| {
        s.owner_id == owner
            && s.category_id == crate::model::CATEGORY_SHIP
            && s.singleton
            && s.location_id == station_id
            && s.flag_id == crate::model::FLAG_HANGAR
    })
}

/// **Into a fighter bay**, docked: `Add(itemID, sourceID, qty=n, flag=158)`
/// on the ship's inventory (the bay window's drops go through the same
/// `_AddItem` as the cargo hold's, `invControllers.py:219`, `:657`) [V].
///
/// What may go in: a category-87 item (anything else is
/// `ItemCannotBeInFighterBay`, `inventoryFlagsCommon` flag data
/// `allowCategories (87,)` [V]), from the hangar or the ship's cargo hold,
/// into the pilot's active ship or another of theirs parked here, while
/// docked; a hull with no bay answers `ShipHasNoFighterBay` [V key]. The room
/// is `fighterCapacity` minus the stacks there.
pub fn plan_to_bay(
    w: &Registry,
    actor: Option<i64>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, Refusal> {
    let c = actor
        .and_then(|a| w.character(a))
        .ok_or_else(|| Refusal::text("No character."))?;
    let owner = c.character_id;
    let station = w
        .docked_station(owner)
        .ok_or_else(|| Refusal::text("Fighters can only be loaded into the bay while docked."))?;
    if ship_id != c.ship_id && !parked_here(w, owner, station, ship_id) {
        return Err(Refusal::text(
            "Only your own ship's fighter bay can take fighters here.",
        ));
    }
    if w.item(ship_id).is_none() {
        return Err(Refusal::text("That ship is not here."));
    }
    if !skills::ship_slots(w, ship_id).has_bay() {
        return Err(Refusal::key("ShipHasNoFighterBay"));
    }
    let item = w
        .store()
        .get(item_id)
        .cloned()
        .ok_or_else(|| Refusal::text("That item is not here."))?;
    if item.owner_id != owner || crate::item::is_loaded_charge(&item) {
        return Err(Refusal::text("That item is not yours to move."));
    }
    if item.category_id != fighters_data::CATEGORY_FIGHTER {
        return Err(Refusal::key("ItemCannotBeInFighterBay"));
    }
    let to = Slot::new(owner, ship_id, FLAG_FIGHTER_BAY);
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
        return Err(Refusal::text(
            "Fighters go into the bay from the hangar or the cargo hold.",
        ));
    }
    let n = part.unwrap_or(item.quantity);
    let unit = volume_of(w, item.type_id);
    let free = (bay_capacity(w, ship_id) - bay_used(w, ship_id)).max(0.0);
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

/// [`plan_to_bay`], applied.
pub fn to_bay(
    w: &mut Registry,
    actor: Option<i64>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    let plan = plan_to_bay(w, actor, ship_id, item_id, quantity).map_err(|r| r.to_error())?;
    if plan.is_empty() {
        return Ok(());
    }
    w.apply_items(&plan)
        .map(|_| ())
        .map_err(|e| Refusal::text(&format!("That move could not be made: {e}")).to_error())
}

/// **Out of a fighter bay into the station hangar**, docked: the same `Add`
/// an unfit is [V].
pub fn plan_to_hangar(
    w: &Registry,
    actor: Option<i64>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, Refusal> {
    let c = actor
        .and_then(|a| w.character(a))
        .ok_or_else(|| Refusal::text("No character."))?;
    let owner = c.character_id;
    let station = w
        .docked_station(owner)
        .ok_or_else(|| Refusal::text("Fighters can only be unloaded while docked."))?;
    let item = w
        .store()
        .get(item_id)
        .cloned()
        .ok_or_else(|| Refusal::text("That item is not here."))?;
    let ship = item.location_id;
    if item.owner_id != owner
        || item.flag_id != FLAG_FIGHTER_BAY
        || (ship != c.ship_id && !parked_here(w, owner, station, ship))
    {
        return Err(Refusal::text(
            "That item is not in one of your fighter bays here.",
        ));
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

/// [`plan_to_hangar`], applied.
pub fn to_hangar(
    w: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<(), ServiceError> {
    let plan = plan_to_hangar(w, actor, item_id, quantity).map_err(|r| r.to_error())?;
    w.apply_items(&plan)
        .map(|_| ())
        .map_err(|e| Refusal::text(&format!("That move could not be made: {e}")).to_error())
}

// ---------------------------------------------------------------------------
// The live table
// ---------------------------------------------------------------------------

/// A tube's task, as `OnFighterTubeTaskStatus` names it [V
/// `fighterConst.py:11-19`]. `EMPTY` and `READY` are not tasks: no task is
/// the rest state (empty without an item in the tube, ready with one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Loading,
    Unloading,
    Launching,
    InSpace,
    Recalling,
    Landing,
    Refuelling,
}

impl Status {
    /// The string the client compares with `TUBE_STATE_*`.
    pub fn wire(self) -> &'static str {
        match self {
            Status::Loading => "LOADING",
            Status::Unloading => "UNLOADING",
            Status::Launching => "LAUNCHING",
            Status::InSpace => "INSPACE",
            Status::Recalling => "RECALLING",
            Status::Landing => "LANDING",
            Status::Refuelling => "REFUELLING",
        }
    }
}

/// One tube's task, from `start` until `until` (`None`: until something
/// happens, as in space).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Task {
    pub status: Status,
    pub start: BlueTime,
    pub until: Option<BlueTime>,
}

/// One squadron in space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Squadron {
    /// Its item id, ball id and roster key.
    pub id: i64,
    pub type_id: i64,
    pub owner_id: i64,
    /// The carrier: `controllerID`.
    pub ship_id: i64,
    pub tube: i64,
    pub solar_system_id: i64,
    /// Fighters in it (the item's quantity at launch).
    pub size: i64,
}

/// The world's fighter state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    tasks: BTreeMap<(i64, i64), Task>,
    live: BTreeMap<i64, Squadron>,
    /// What each flying squadron is doing: its movement order, its abilities,
    /// charges and cooldowns (FT-E, `ability.rs`).
    rt: BTreeMap<i64, ability::Runtime>,
    /// What the squadrons' electronic warfare holds against targets, and what
    /// each target's client was told of it (FT-F, `effects.rs`).
    ew: effects::Ewar,
    /// Squadrons nobody controls, and since when (FT-G, `abandon.rs`).
    abandoned: BTreeMap<i64, BlueTime>,
    /// Bombs in flight and kamikaze runs (FT-G, `strike.rs`).
    strikes: strike::Strikes,
    swept: bool,
}

impl State {
    pub fn task(&self, ship_id: i64, tube: i64) -> Option<&Task> {
        self.tasks.get(&(ship_id, tube))
    }

    pub fn squadron(&self, id: i64) -> Option<&Squadron> {
        self.live.get(&id)
    }

    /// Whether a squadron was abandoned and nobody has scooped it.
    pub fn is_abandoned(&self, id: i64) -> bool {
        self.abandoned.contains_key(&id)
    }

    pub fn squadrons(&self) -> impl Iterator<Item = &Squadron> {
        self.live.values()
    }

    /// The squadrons one carrier has out.
    pub fn of_ship(&self, ship_id: i64) -> impl Iterator<Item = &Squadron> {
        self.live.values().filter(move |s| s.ship_id == ship_id)
    }
}

/// The registration row (`world_ext::SLOTS`, coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "fighters",
    make: |_| Box::new(State::default()) as Boxed,
};

fn state(w: &Registry) -> &State {
    w.ext::<State>()
}

fn state_mut(w: &mut Registry) -> &mut State {
    w.ext_mut::<State>()
}

// ---------------------------------------------------------------------------
// What the client is told
// ---------------------------------------------------------------------------

/// One push to one pilot.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub to: i64,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `OnFighterTubeContentUpdate(tubeFlagID, fighterID, fighterTypeID,
    /// squadronSize)` [V `shipFighterState.py:93`].
    Content {
        tube: i64,
        fighter: i64,
        type_id: i64,
        size: i64,
    },
    /// `OnFighterTubeContentEmpty(tubeFlagID)`: the handler `del`s the tube,
    /// so only ever after an update [V `:98`].
    Empty { tube: i64 },
    /// `OnFighterTubeTaskStatus(tubeFlagID, statusID, start, end)` [V `:144`];
    /// times are filetimes, `None` for READY and EMPTY.
    Task {
        tube: i64,
        status: &'static str,
        start: Option<i64>,
        end: Option<i64>,
    },
    /// `OnFighterAddedToController(fighterID, fighterTypeID, tubeFlagID,
    /// squadronSize, (chargeStates, cooldownStates))` [V `:109`].
    Added {
        fighter: i64,
        type_id: i64,
        tube: i64,
        size: i64,
        charges: Vec<(i64, i64)>,
    },
    /// `OnFighterRemovedFromController(fighterID, tubeFlagID)`: `pop`s, so
    /// only after an add [V `:118`].
    Removed { fighter: i64, tube: i64 },
    /// `OnFighterAbilitySlotActivated(fighterID, abilitySlotID,
    /// remainingChargeCount | None, startSimTime, durationMs, cooldown |
    /// None)`, `cooldown` a `(start, end)` pair of sim times [V
    /// `shipFighterState.py:OnFighterAbilitySlotActivated`].
    Activated {
        fighter: i64,
        slot: i64,
        charges: Option<i64>,
        start: i64,
        duration_ms: i64,
        cooldown: Option<(i64, i64)>,
    },
    /// `OnFighterAbilitySlotDeactivated(fighterID, abilitySlotID,
    /// failureReason | None)`; the reason a `UserError` [V `:OnFighterAbility
    /// SlotDeactivated`].
    Deactivated {
        fighter: i64,
        slot: i64,
        reason: Option<Refusal>,
    },
    /// `OnInSpaceSquadronSizeChanged(fighterID, squadronSize)` [V `:OnInSpace
    /// SquadronSizeChanged`].
    SizeChanged { fighter: i64, size: i64 },
}

impl Push for Event {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let int = PyValue::Int;
        let time = |t: Option<i64>| t.map_or(PyValue::None, PyValue::Int);
        match &self.kind {
            Kind::Content {
                tube,
                fighter,
                type_id,
                size,
            } => (
                "OnFighterTubeContentUpdate",
                vec![int(*tube), int(*fighter), int(*type_id), int(*size)],
            ),
            Kind::Empty { tube } => ("OnFighterTubeContentEmpty", vec![int(*tube)]),
            Kind::Task {
                tube,
                status,
                start,
                end,
            } => (
                "OnFighterTubeTaskStatus",
                vec![
                    int(*tube),
                    crate::pyutil::bstr(status),
                    time(*start),
                    time(*end),
                ],
            ),
            Kind::Added {
                fighter,
                type_id,
                tube,
                size,
                charges,
            } => (
                "OnFighterAddedToController",
                vec![
                    int(*fighter),
                    int(*type_id),
                    int(*tube),
                    int(*size),
                    ability_states(charges),
                ],
            ),
            Kind::Removed { fighter, tube } => (
                "OnFighterRemovedFromController",
                vec![int(*fighter), int(*tube)],
            ),
            Kind::Activated {
                fighter,
                slot,
                charges,
                start,
                duration_ms,
                cooldown,
            } => (
                "OnFighterAbilitySlotActivated",
                vec![
                    int(*fighter),
                    int(*slot),
                    charges.map_or(PyValue::None, PyValue::Int),
                    int(*start),
                    int(*duration_ms),
                    cooldown.map_or(PyValue::None, |(a, b)| {
                        PyValue::Tuple(vec![PyValue::Int(a), PyValue::Int(b)])
                    }),
                ],
            ),
            Kind::Deactivated {
                fighter,
                slot,
                reason,
            } => (
                "OnFighterAbilitySlotDeactivated",
                vec![
                    int(*fighter),
                    int(*slot),
                    reason.as_ref().map_or(PyValue::None, Refusal::to_value),
                ],
            ),
            Kind::SizeChanged { fighter, size } => (
                "OnInSpaceSquadronSizeChanged",
                vec![int(*fighter), int(*size)],
            ),
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `(chargeStates {slot: count}, cooldownStates {slot: (start, end)})`
/// [V `shipFighterState._UpdateAbilitySlotStates`]. Wire dicts are
/// value-then-key.
fn ability_states(charges: &[(i64, i64)]) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Dict(
            charges
                .iter()
                .map(|(slot, count)| (PyValue::Int(*count), PyValue::Int(*slot)))
                .collect(),
        ),
        PyValue::Dict(Vec::new()),
    ])
}

/// Which queue a push goes on: inside a call, or the world clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Answer,
    Clock,
}

fn send(w: &mut Registry, via: Via, p: impl Push + 'static) {
    match via {
        Via::Answer => w.push(p),
        Via::Clock => w.schedule(Duration::ZERO, p),
    }
}

fn tell(w: &mut Registry, via: Via, to: i64, kind: Kind) {
    send(w, via, Event { to, kind });
}

fn task_event(owner: i64, tube: i64, task: Option<&Task>) -> Event {
    let (status, start, end) = match task {
        Some(t) => (t.status.wire(), Some(t.start), t.until),
        None => ("READY", None, None),
    };
    Event {
        to: owner,
        kind: Kind::Task {
            tube,
            status,
            start,
            end,
        },
    }
}

/// Set a tube's task and tell the pilot.
fn set_task(w: &mut Registry, via: Via, owner: i64, ship: i64, tube: i64, task: Task) {
    state_mut(w).tasks.insert((ship, tube), task);
    send(w, via, task_event(owner, tube, Some(&task)));
}

/// End a tube's task (back to rest) and tell the pilot: `READY` with an item
/// in the tube, `EMPTY` without.
fn clear_task(w: &mut Registry, via: Via, owner: i64, ship: i64, tube: i64) {
    state_mut(w).tasks.remove(&(ship, tube));
    let loaded = tube_item(w, ship, tube).is_some();
    let ev = Event {
        to: owner,
        kind: Kind::Task {
            tube,
            status: if loaded { "READY" } else { "EMPTY" },
            start: None,
            end: None,
        },
    };
    send(w, via, ev);
}

type Held = Vec<(Duration, Box<dyn Push>)>;

/// Apply a plan and hold its pushes (`drones::apply_held`'s reasoning: what
/// was already pending belongs to somebody else's turn).
fn apply_held(w: &mut Registry, edits: &[Edit]) -> Option<(Vec<crate::item::Change>, Held)> {
    let before = w.take_pending();
    let applied = w.apply_items(edits);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    applied.ok().map(|c| (c, ours))
}

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

fn created(changes: &[crate::item::Change]) -> Option<i64> {
    changes.iter().find_map(|ch| match ch {
        crate::item::Change::Created { item_id, .. } => Some(*item_id),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Tubes
// ---------------------------------------------------------------------------

/// What a tube holds in the store: `(item id, type id, size)`.
fn tube_item(w: &Registry, ship: i64, tube: i64) -> Option<(i64, i64, i64)> {
    w.store()
        .contents_of(ship)
        .find(|i| i.flag_id == tube)
        .map(|i| (i.item_id, i.type_id, i.quantity))
}

/// A ship's loaded tubes as the client's `fightersInTubes` rows, tube order.
fn loaded_tubes(w: &Registry, ship: i64) -> Vec<(i64, i64, i64, i64)> {
    let mut rows: Vec<(i64, i64, i64, i64)> = w
        .store()
        .contents_of(ship)
        .filter(|i| is_tube_flag(i.flag_id))
        .map(|i| (i.flag_id, i.item_id, i.type_id, i.quantity))
        .collect();
    rows.sort_unstable();
    rows
}

/// What each used tube holds, for the class rules: loaded tubes, and tubes
/// whose squadron is in space.
fn occupancy(w: &Registry, ship: i64) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = loaded_tubes(w, ship)
        .into_iter()
        .map(|(tube, _, type_id, _)| (tube, type_id))
        .collect();
    out.extend(state(w).of_ship(ship).map(|s| (s.tube, s.type_id)));
    out.sort_unstable();
    out.dedup();
    out
}

fn pilot_ship(w: &Registry, actor: Option<i64>) -> Result<(i64, i64, i64), Refusal> {
    let c = actor
        .and_then(|a| w.character(a))
        .ok_or_else(|| Refusal::text("No character."))?;
    Ok((c.character_id, c.ship_id, c.ship_type_id))
}

/// The tube checks the client makes first and a modified client skips
/// (R-DEEP §2.2 rule 4: tube *n* exists only when *n* < `fighterTubes`).
fn check_tube(slots: &HullSlots, tube: i64, new_type: i64, class: Class) -> Result<(), Refusal> {
    if !is_tube_flag(tube) || tube_index(tube) >= slots.tubes {
        // `[A]` no key names "this tube does not exist"; the class key is the
        // nearest. Falsifier: none needed, it is a refusal either way.
        return Err(Refusal::with(
            "CannotLoadFighterTubeNoTubesAvailableForClass",
            class_args(class, new_type),
        ));
    }
    Ok(())
}

/// **The five load rules** [V `ghostFittingSvc.FitFighterToTube`,
/// `_CheckEmptyTubeCanAcceptFighter`; R-DEEP §2.2]:
///
/// 1. one tube holds one **type** (`CannotLoadFighterTubeDifferentTypeAlreadyLoaded`);
/// 2. a tube holds at most `fighterSquadronMaxSize` (`...SquadronIsFull`);
/// 3. an empty tube takes a class only if the hull has slots for it
///    (`...NoTubesExistForClass`), tubes in use < `fighterTubes` and tubes
///    already holding that class < the class's slots (`...NoTubesAvailableForClass`);
/// 4. tube *n* exists only when *n* < `fighterTubes`;
/// 5. the source is a stack in this ship's fighter bay
///    (`CannotLoadFighterSourceNotInFighterBay`).
///
/// Counted in **tubes**, as the live squadron window does
/// (`fighterInvCont.GetSquadronTypes` [V]); the ghost-fitting simulator
/// counts fighters, which reads as a client quirk. `[A]` the server follows
/// the window.
pub fn plan_load(
    w: &Registry,
    actor: Option<i64>,
    fighter_id: i64,
    tube: i64,
) -> Result<Vec<Edit>, Refusal> {
    let (owner, ship, _) = pilot_ship(w, actor)?;
    let slots = skills::ship_slots(w, ship);
    if !slots.has_bay() {
        return Err(Refusal::key("ShipHasNoFighterBay"));
    }
    // Rule 5.
    let item = w
        .store()
        .get(fighter_id)
        .filter(|i| i.owner_id == owner && i.location_id == ship && i.flag_id == FLAG_FIGHTER_BAY)
        .cloned()
        .ok_or_else(|| Refusal::key("CannotLoadFighterSourceNotInFighterBay"))?;
    let ft = fighter_type(w.content(), item.type_id)
        .ok_or_else(|| Refusal::key("ItemCannotBeInFighterBay"))?;
    // Rule 4.
    check_tube(&slots, tube, item.type_id, ft.class)?;
    if state(w).task(ship, tube).is_some() {
        return Err(Refusal::key("FighterTubeIsBusy"));
    }
    let to = Slot::new(owner, ship, tube);
    let whole = |edit_whole: Edit, edit_part: Edit, n: i64| {
        if n == item.quantity {
            edit_whole
        } else {
            edit_part
        }
    };
    match tube_item(w, ship, tube) {
        Some((occupant, occupant_type, held)) => {
            // Rule 1.
            if occupant_type != item.type_id {
                return Err(Refusal::with(
                    "CannotLoadFighterTubeDifferentTypeAlreadyLoaded",
                    vec![
                        ("loadedType", PyValue::Int(occupant_type)),
                        ("newType", PyValue::Int(item.type_id)),
                    ],
                ));
            }
            // Rule 2.
            let room = ft.squadron_max - held;
            if room < 1 {
                return Err(Refusal::with(
                    "CannotLoadFighterTubeSquadronIsFull",
                    vec![("loadedType", PyValue::Int(occupant_type))],
                ));
            }
            let n = room.min(item.quantity);
            Ok(vec![
                whole(
                    Edit::Destroy {
                        item_id: fighter_id,
                    },
                    Edit::Consume {
                        item_id: fighter_id,
                        quantity: n,
                    },
                    n,
                ),
                Edit::Grow {
                    item_id: occupant,
                    quantity: n,
                },
            ])
        }
        None => {
            // Rule 3.
            let limit = slots.slots_for(ft.class);
            if limit < 1 {
                return Err(Refusal::with(
                    "CannotLoadFighterTubeNoTubesExistForClass",
                    class_args(ft.class, item.type_id),
                ));
            }
            let used = occupancy(w, ship);
            let of_class = used
                .iter()
                .filter(|(_, t)| fighter_type(w.content(), *t).is_some_and(|f| f.class == ft.class))
                .count() as i64;
            if used.len() as i64 >= slots.tubes || of_class >= limit {
                return Err(Refusal::with(
                    "CannotLoadFighterTubeNoTubesAvailableForClass",
                    class_args(ft.class, item.type_id),
                ));
            }
            let n = ft.squadron_max.min(item.quantity);
            Ok(vec![whole(
                Edit::Move {
                    item_id: fighter_id,
                    to,
                },
                Edit::Split {
                    item_id: fighter_id,
                    quantity: n,
                    to,
                },
                n,
            )])
        }
    }
}

/// `fighterMgr.LoadFightersToTube(fighterID, tubeFlagID)` [V
/// `fightersSvc.py:88`, reply discarded]: the move, the new content, and the
/// tube `LOADING` for [`LOAD_MS`].
pub fn load(
    w: &mut Registry,
    actor: Option<i64>,
    fighter_id: i64,
    tube: i64,
) -> Result<(), Refusal> {
    let plan = plan_load(w, actor, fighter_id, tube)?;
    let (owner, ship, _) = pilot_ship(w, actor)?;
    let occupant = tube_item(w, ship, tube).map(|(id, _, _)| id);
    let changes = w
        .apply_items(&plan)
        .map_err(|e| Refusal::text(&format!("That load could not be made: {e}")))?;
    let item_id = occupant.or_else(|| created(&changes)).unwrap_or(fighter_id);
    if let Some((id, type_id, size)) = tube_item(w, ship, tube) {
        debug_assert_eq!(id, item_id);
        tell(
            w,
            Via::Answer,
            owner,
            Kind::Content {
                tube,
                fighter: id,
                type_id,
                size,
            },
        );
    }
    let now = crate::pyutil::filetime_now();
    set_task(
        w,
        Via::Answer,
        owner,
        ship,
        tube,
        Task {
            status: Status::Loading,
            start: now,
            until: Some(now + ms(LOAD_MS)),
        },
    );
    Ok(())
}

/// `fighterMgr.UnloadTubeToFighterBay(tubeFlagID)` [V `fightersSvc.py:93`,
/// reply discarded; the client calls it only for a tube that holds
/// something]: the tube's stack back into the bay (merged into a stack of the
/// same type there), the tube `UNLOADING` for [`UNLOAD_MS`].
pub fn unload(w: &mut Registry, actor: Option<i64>, tube: i64) -> Result<(), Refusal> {
    let (owner, ship, _) = pilot_ship(w, actor)?;
    if !is_tube_flag(tube) {
        return Err(Refusal::text("That is not a launch tube."));
    }
    if let Some(t) = state(w).task(ship, tube)
        && t.status != Status::Loading
    {
        return Err(Refusal::key("FighterTubeIsBusy"));
    }
    let (item_id, type_id, size) =
        tube_item(w, ship, tube).ok_or_else(|| Refusal::text("That tube is empty."))?;
    let unit = volume_of(w, type_id);
    let free = (bay_capacity(w, ship) - bay_used(w, ship)).max(0.0);
    if crate::fitting::units_that_fit(free, unit) < size {
        return Err(no_room(free, unit * size as f64));
    }
    let bay = Slot::new(owner, ship, FLAG_FIGHTER_BAY);
    let same = w
        .store()
        .contents_of(ship)
        .find(|i| i.flag_id == FLAG_FIGHTER_BAY && i.type_id == type_id && !i.singleton)
        .map(|i| i.item_id);
    let plan = match same {
        Some(stack) => vec![
            Edit::Destroy { item_id },
            Edit::Grow {
                item_id: stack,
                quantity: size,
            },
        ],
        None => vec![Edit::Move { item_id, to: bay }],
    };
    w.apply_items(&plan)
        .map_err(|e| Refusal::text(&format!("That unload could not be made: {e}")))?;
    tell(w, Via::Answer, owner, Kind::Empty { tube });
    let now = crate::pyutil::filetime_now();
    set_task(
        w,
        Via::Answer,
        owner,
        ship,
        tube,
        Task {
            status: Status::Unloading,
            start: now,
            until: Some(now + ms(UNLOAD_MS)),
        },
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// GetFightersForShip
// ---------------------------------------------------------------------------

/// `fighterMgr.GetFightersForShip()` [V `fightersSvc.py:85`, unpacked as a
/// 3-tuple]: `(fightersInTubes, fightersInSpace, abilityStates)`. The wire
/// row is `(tubeFlagID, fighterItemID, fighterTypeID, squadronSize)`; the
/// client reshuffles it into a namedtuple of another order
/// (`shipFighterState._UpdateStateForShip`) [V]. A ship that carries none is
/// `([], [], {})`: all three loops run zero times and the third must be a
/// dict.
pub fn fighters_for_ship(w: &Registry, actor: Option<i64>) -> PyValue {
    let ship = actor.and_then(|a| w.character(a)).map(|c| c.ship_id);
    let Some(ship) = ship else {
        return PyValue::Tuple(vec![
            PyValue::List(vec![]),
            PyValue::List(vec![]),
            PyValue::Dict(vec![]),
        ]);
    };
    let row = |tube: i64, item: i64, type_id: i64, size: i64| {
        PyValue::Tuple(vec![
            PyValue::Int(tube),
            PyValue::Int(item),
            PyValue::Int(type_id),
            PyValue::Int(size),
        ])
    };
    let in_tubes: Vec<PyValue> = loaded_tubes(w, ship)
        .into_iter()
        .map(|(tube, item, type_id, size)| row(tube, item, type_id, size))
        .collect();
    let mut space: Vec<&Squadron> = state(w).of_ship(ship).collect();
    space.sort_by_key(|s| s.tube);
    let in_space: Vec<PyValue> = space
        .iter()
        .map(|s| row(s.tube, s.id, s.type_id, s.size))
        .collect();
    let states: Vec<(PyValue, PyValue)> = space
        .iter()
        .map(|s| {
            (
                ability_states(&charges_of(w, s.type_id)),
                PyValue::Int(s.id),
            )
        })
        .collect();
    PyValue::Tuple(vec![
        PyValue::List(in_tubes),
        PyValue::List(in_space),
        PyValue::Dict(states),
    ])
}

/// A fighter's charge counts per ability slot, full: the slots' `charges`
/// column. `[A]` a squadron starts full; falsifier: the ability button shows
/// no charges until first use.
fn charges_of(w: &Registry, type_id: i64) -> Vec<(i64, i64)> {
    w.content()
        .try_get::<fighters_data::Fighters>()
        .map(|t| {
            t.slots_of(type_id)
                .iter()
                .filter_map(|s| s.charges.map(|c| (s.slot, c)))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The ball
// ---------------------------------------------------------------------------

/// The numbers of a squadron's ball, from its type's attributes. Server-only
/// fields are `[A]` as for drones: a squadron holds no lock, so the targeting
/// three are never read.
pub fn hull(content: &Content, type_id: i64) -> Option<Hull> {
    let table = typedata::installed_in(content).table();
    let row = table.get(type_id)?;
    if row.category_id != fighters_data::CATEGORY_FIGHTER {
        return None;
    }
    let a = |id| table.attribute(type_id, id);
    Some(Hull {
        type_id,
        mass: row.mass,
        radius: row.radius as f32,
        max_velocity: a(typedata::attr::MAX_VELOCITY) as f32,
        agility: a(typedata::attr::AGILITY) as f32,
        shield_capacity: a(typedata::attr::SHIELD_CAPACITY),
        shield_recharge_rate: a(typedata::attr::SHIELD_RECHARGE),
        armor_hp: a(typedata::attr::ARMOR_HP),
        max_target_range: 0.0,
        max_locked_targets: 0.0,
        scan_resolution: 0.0,
        signature_radius: a(typedata::attr::SIGNATURE_RADIUS),
        warp_speed_au: 0.0,
        hp: a(typedata::attr::HP),
        capacitor_capacity: 0.0,
        capacitor_recharge_rate: 0.0,
    })
}

/// A flying squadron's hull with its pilot's skills and its carrier's bonuses
/// in it ([`skills::numbers`], fixed at launch), before any running ability
/// or web: speed, shield, armour, hull points and signature.
pub fn base_hull(w: &Registry, id: i64) -> Option<Hull> {
    let s = state(w).squadron(id)?;
    let mut h = hull(w.content(), s.type_id)?;
    let a = |attribute: i64| skills::attr_of(w, id, s.type_id, attribute);
    h.max_velocity = a(typedata::attr::MAX_VELOCITY) as f32;
    h.shield_capacity = a(typedata::attr::SHIELD_CAPACITY);
    h.armor_hp = a(typedata::attr::ARMOR_HP);
    h.hp = a(typedata::attr::HP);
    h.signature_radius = a(typedata::attr::SIGNATURE_RADIUS);
    Some(h)
}

/// A flying squadron's hull, for `dogma::hull`'s fighter arm (coordinator).
pub fn hull_of(w: &Registry, id: i64) -> Option<Hull> {
    let mut h = base_hull(w, id)?;
    // A running afterburner, microwarpdrive or evasion, and a web on it
    // (FT-F).
    let m = effects::mobility(w, id);
    h.max_velocity =
        (f64::from(h.max_velocity) * m.speed * crate::ewar::speed_factor(w, id)) as f32;
    h.signature_radius *= m.signature;
    Some(h)
}

/// Who flies a squadron's roster entry, for `Registry::pilot` (coordinator):
/// its own type as the "ship", its owner as owner, no `char_id` (it has no
/// session), as `drones::pilot_of`.
pub fn pilot_of(w: &Registry, roster_key: i64) -> Option<crate::npc::Pilot> {
    let s = state(w).squadron(roster_key)?;
    let c = w.character(s.owner_id);
    Some(crate::npc::Pilot {
        ship_type_id: s.type_id,
        owner_id: s.owner_id,
        corporation_id: c.map_or(0, |c| c.corporation_id),
        alliance_id: c.and_then(|c| c.alliance_id),
        char_id: None,
    })
}

/// A squadron's ball.
pub fn ball(w: &Registry, s: &Squadron, motion: &Motion) -> Option<destiny::Ball> {
    let h = base_hull(w, s.id)?;
    let c = w.character(s.owner_id);
    Some(destiny::ego_ball_of(
        &h,
        s.id,
        c.and_then(|c| i32::try_from(c.corporation_id).ok())
            .unwrap_or(-1),
        c.and_then(|c| c.alliance_id)
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        motion,
    ))
}

/// A squadron's slim. `ownerID` is **the character**: the client's dogma
/// loads a drone or fighter only when it is the pilot's
/// (`clientDogmaLocation OnItemChange`, `_WasLaunchingFighter`) [V, R-DEEP
/// §2.5]; `fighter.squadronSize` rides in [`slim_extras`].
pub fn slim(w: &Registry, s: &Squadron) -> destiny::Slim {
    destiny::Slim {
        item_id: s.id,
        type_id: s.type_id,
        owner_id: s.owner_id,
        char_id: None,
        corp_id: w.character(s.owner_id).map(|c| c.corporation_id),
        response_thresholds: None,
    }
}

/// The slim key the swarm count is read from: `fighters.SQUADRON_SIZE_SLIMITEM_NAME`
/// [V `fighters/__init__.py:30`]; `FighterSquadron` does
/// `getattr(slimItem, 'fighter.squadronSize')` [V `fighters/client:GetSquadronSizeFromSlimItem`].
pub const SQUADRON_SIZE_KEY: &str = "fighter.squadronSize";

/// **The S9 source** (`slim_extra::SOURCES`, coordinator): a squadron's slim
/// carries its size.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter_map(|id| state(w).squadron(*id))
        .map(|s| (s.id, vec![(SQUADRON_SIZE_KEY, PyValue::Int(s.size))]))
        .collect()
}

/// A squadron's roster entry, for the bootstrap's balls (coordinator).
pub fn in_system(w: &Registry, system: i64) -> Vec<Squadron> {
    state(w)
        .squadrons()
        .filter(|s| s.solar_system_id == system)
        .copied()
        .collect()
}

/// Everyone who should see a ball: the characters in a system.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

fn position_in(w: &Registry, ball: i64, system: i64) -> Option<[f64; 3]> {
    w.roster()
        .by_ship(ball)
        .filter(|p| p.solar_system_id == system)
        .map(|p| p.motion.position)
}

fn send_actions(w: &mut Registry, via: Via, system: i64, actions: Vec<Action>) {
    let to = audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(update) = destiny::ActionUpdate::new(to, stamp, actions) {
        send(w, via, update);
    }
}

// ---------------------------------------------------------------------------
// Launch
// ---------------------------------------------------------------------------

/// A `{key: value-or-None}` reply: wire dicts are value-then-key.
fn reply(entries: Vec<(i64, Option<Refusal>)>) -> PyValue {
    PyValue::Dict(
        entries
            .into_iter()
            .map(|(k, r)| (r.map_or(PyValue::None, |r| r.to_value()), PyValue::Int(k)))
            .collect(),
    )
}

/// `fighterMgr.LaunchFightersFromTubes(tubeFlagIDs)` [V `fightersSvc.py:104`]:
/// answers `{tubeFlagID: None | UserError}` (the client calls `.iteritems()`
/// and `error.args` on each); the client has already filtered to `READY`
/// tubes and the server checks again.
///
/// Per tube: the carrier is in space and not warping
/// (`CannotOpenFighterTubesWhileWarping` [V key]); the tube is `READY`
/// (`FighterTubeIsBusy`); it holds a squadron. The item moves from
/// `(ship, tube)` to the solar system **with the old location in the
/// `OnItemChange`**, which is how the client knows it launched
/// (`clientDogmaUtilFunc._DroneOrFighterWasLaunched` [V, R-DEEP §2.5]); the
/// ball joins the roster under the item id, orbiting its carrier.
///
/// The pilot's skills are checked ([`skills::gate`], FT-H);
/// cloaking (`CannotOpenFighterTubesWhileCloaked`) is not: there is no cloak
/// state to read.
pub fn launch(w: &mut Registry, actor: Option<i64>, tubes: &[i64]) -> PyValue {
    let mut answer: Vec<(i64, Option<Refusal>)> = Vec::new();
    let Some((owner, ship, ship_type)) = pilot_ship(w, actor).ok() else {
        for t in tubes {
            answer.push((*t, Some(Refusal::text("No character."))));
        }
        return reply(answer);
    };
    w.advance_character(owner);
    let here = w.roster().get(owner).copied().filter(|p| p.ship_id == ship);
    let slots = skills::ship_slots(w, ship);
    let ship_radius = typedata::hull_in(w.content(), ship_type)
        .or_else(|| destiny::hull::find_in(w.content(), ship_type))
        .map_or(50.0, |h| f64::from(h.radius));
    let mut held: Held = Vec::new();
    let mut fresh: Vec<i64> = Vec::new();
    for &tube in tubes {
        let outcome = (|| -> Result<(), Refusal> {
            let here =
                here.ok_or_else(|| Refusal::text("Fighters can only be launched in space."))?;
            if here.motion.warp_active() {
                return Err(Refusal::key("CannotOpenFighterTubesWhileWarping"));
            }
            if !is_tube_flag(tube) || tube_index(tube) >= slots.tubes {
                return Err(Refusal::text("That launch tube does not exist."));
            }
            if state(w).task(ship, tube).is_some() {
                return Err(Refusal::key("FighterTubeIsBusy"));
            }
            let (item_id, type_id, size) =
                tube_item(w, ship, tube).ok_or_else(|| Refusal::text("That tube is empty."))?;
            fighter_type(w.content(), type_id)
                .ok_or_else(|| Refusal::text("This server has no data for that fighter."))?;
            // FT-H: the skills the type requires.
            skills::gate(w, owner, type_id)?;
            let to = Slot::new(owner, here.solar_system_id, crate::combat::FLAG_IN_SPACE);
            let (_, pushes) = apply_held(w, &[Edit::Move { item_id, to }])
                .ok_or_else(|| Refusal::text("That squadron could not be launched."))?;
            held.extend(pushes);
            // Round the ship, one slot per tube [A].
            let angle = std::f64::consts::TAU * (tube_index(tube) as f64) / 5.0;
            let r = ship_radius + LAUNCH_CLEARANCE;
            let at = here.motion.position;
            let position = [at[0] + r * angle.cos(), at[1], at[2] + r * angle.sin()];
            w.roster_mut().arrive(Presence::arriving(
                item_id,
                item_id,
                here.solar_system_id,
                Motion::arrived(position, [0.0; 3]),
            ));
            state_mut(w).live.insert(
                item_id,
                Squadron {
                    id: item_id,
                    type_id,
                    owner_id: owner,
                    ship_id: ship,
                    tube,
                    solar_system_id: here.solar_system_id,
                    size,
                },
            );
            let runtime = ability::Runtime::launched(w, type_id, ship);
            state_mut(w).rt.insert(item_id, runtime);
            fresh.push(item_id);
            Ok(())
        })();
        answer.push((tube, outcome.err()));
    }
    announce_launch(w, owner, &fresh);
    release(w, Via::Answer, owner, held);
    reply(answer)
}

/// The news of a launch, in the order the client needs it: the balls (with
/// their slims and the squadron size), the tubes' content and status, the
/// controller event, then the movement; the item rows last (the caller's
/// [`release`]).
fn announce_launch(w: &mut Registry, owner: i64, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let now = crate::pyutil::filetime_now();
    let mut balls = Vec::new();
    let mut extra = Vec::new();
    let mut system = 0;
    let mut sent: Vec<Squadron> = Vec::new();
    for id in ids {
        let (Some(s), Some(p)) = (
            state(w).squadron(*id).copied(),
            w.roster().get(*id).copied(),
        ) else {
            continue;
        };
        let Some(b) = ball(w, &s, &p.motion) else {
            continue;
        };
        system = s.solar_system_id;
        balls.push(b);
        extra.push((
            slim(w, &s),
            Some(crate::combat::damage_state(w, *id, now).to_pyvalue()),
        ));
        sent.push(s);
    }
    if sent.is_empty() {
        return;
    }
    let to = audience(w, system);
    let stamp = w.next_destiny_stamp();
    let slim_ids: Vec<i64> = sent.iter().map(|s| s.id).collect();
    let extras = slim_extras(w, &slim_ids);
    if let Some(added) = destiny::BallsAdded::new(
        to,
        stamp,
        destiny::Park {
            current_time: stamp,
            balls,
        },
        extra,
    ) {
        w.push(added.with_extras(extras));
    }
    for s in &sent {
        let ft = fighter_type(w.content(), s.type_id);
        tell(w, Via::Answer, owner, Kind::Empty { tube: s.tube });
        set_task(
            w,
            Via::Answer,
            owner,
            s.ship_id,
            s.tube,
            Task {
                status: Status::Launching,
                start: now,
                until: Some(now + ms(LAUNCH_MS)),
            },
        );
        tell(
            w,
            Via::Answer,
            owner,
            Kind::Added {
                fighter: s.id,
                type_id: s.type_id,
                tube: s.tube,
                size: s.size,
                charges: charges_of(w, s.type_id),
            },
        );
        let range = ft
            .map(|f| f.orbit_range)
            .filter(|r| *r > 0.0)
            .unwrap_or(DEFAULT_ORBIT);
        let actions = vec![
            Action::Orbit {
                ball: s.id,
                target: s.ship_id,
                range,
            },
            Action::SetSpeedFraction {
                ball: s.id,
                fraction: LAUNCH_SPEED_FRACTION,
            },
        ];
        if let Some(p) = w.roster_mut().get_mut(s.id) {
            for a in &actions {
                p.motion.after(*a);
            }
        }
        send_actions(w, Via::Answer, s.solar_system_id, actions);
    }
}

// ---------------------------------------------------------------------------
// Recall and landing
// ---------------------------------------------------------------------------

/// `fighterMgr.RecallFightersToTubes(fighterIDs)` [V `fightersSvc.py:124`]:
/// answers `{fighterID: None | UserError}`; on `None` the client itself sets
/// the tube `RECALLING`. Refused: not a squadron of the pilot's ship
/// (`CannotControlFighterNotControlledByYourShip` [V key]); not `INSPACE`
/// (`FighterTubeIsBusy`). The squadron flies to its carrier at top speed
/// (time: distance over speed between [`RECALL_MIN_MS`] and
/// [`RECALL_MAX_MS`], [A]) and lands: see [`tick`].
///
/// `CannotRecallFighterWarpScrambled` is not raised: no scramble state is
/// read here.
pub fn recall(w: &mut Registry, actor: Option<i64>, ids: &[i64]) -> PyValue {
    let mut answer: Vec<(i64, Option<Refusal>)> = Vec::new();
    let Some((owner, ship, _)) = pilot_ship(w, actor).ok() else {
        for id in ids {
            answer.push((*id, Some(Refusal::text("No character."))));
        }
        return reply(answer);
    };
    let now = crate::pyutil::filetime_now();
    for &id in ids {
        let Some(s) = state(w).squadron(id).copied().filter(|s| s.ship_id == ship) else {
            answer.push((
                id,
                Some(Refusal::key("CannotControlFighterNotControlledByYourShip")),
            ));
            continue;
        };
        if state(w).task(ship, s.tube).map(|t| t.status) != Some(Status::InSpace) {
            answer.push((id, Some(Refusal::key("FighterTubeIsBusy"))));
            continue;
        }
        w.advance_system(s.solar_system_id);
        let distance = match (
            position_in(w, id, s.solar_system_id),
            position_in(w, ship, s.solar_system_id),
        ) {
            (Some(a), Some(b)) => crate::space_state::distance(a, b),
            _ => 0.0,
        };
        let speed = base_hull(w, id).map_or(0.0, |h| f64::from(h.max_velocity));
        let travel = if speed > 0.0 {
            (distance / speed * 1000.0) as i64
        } else {
            RECALL_MAX_MS
        }
        .clamp(RECALL_MIN_MS, RECALL_MAX_MS);
        state_mut(w).tasks.insert(
            (ship, s.tube),
            Task {
                status: Status::Recalling,
                start: now,
                until: Some(now + ms(travel)),
            },
        );
        let actions = vec![
            Action::FollowBall {
                ball: id,
                target: ship,
                range: 0.0,
            },
            Action::SetSpeedFraction {
                ball: id,
                fraction: 1.0,
            },
        ];
        if let Some(p) = w.roster_mut().get_mut(id) {
            for a in &actions {
                p.motion.after(*a);
            }
        }
        send_actions(w, Via::Answer, s.solar_system_id, actions);
        answer.push((id, None));
    }
    let _ = owner;
    reply(answer)
}

/// Take a squadron out of space: its ball removed, its controller event, its
/// item back into its tube (or, with its carrier gone, its owner's home
/// hangar), the tube's content told. `forced` (the carrier left, died or
/// logged off) lands it at once and the tube is `READY`; otherwise the tube
/// goes `LANDING`, the order the client needs: the status first, because
/// `OnFighterRemovedFromController` sets the tube `EMPTY` unless it is
/// `LANDING` or `REFUELLING` [V `shipFighterState.py:118-126`].
fn land(w: &mut Registry, via: Via, id: i64, forced: bool, now: BlueTime) {
    let Some(s) = state_mut(w).live.remove(&id) else {
        return;
    };
    state_mut(w).rt.remove(&id);
    effects::source_gone(w, id);
    if !forced {
        set_task(
            w,
            via,
            s.owner_id,
            s.ship_id,
            s.tube,
            Task {
                status: Status::Landing,
                start: now,
                until: Some(now + ms(LAND_MS)),
            },
        );
    } else {
        state_mut(w).tasks.remove(&(s.ship_id, s.tube));
    }
    let to_park = audience(w, s.solar_system_id);
    w.roster_mut().leave(id);
    let stamp = w.next_destiny_stamp();
    if let Some(removed) = destiny::BallsRemoved::new(to_park, stamp, vec![id]) {
        send(w, via, removed);
    }
    tell(
        w,
        via,
        s.owner_id,
        Kind::Removed {
            fighter: id,
            tube: s.tube,
        },
    );
    let carrier = w.store().get(s.ship_id).is_some();
    let dest = if carrier {
        Some(Slot::new(s.owner_id, s.ship_id, s.tube))
    } else {
        w.character(s.owner_id)
            .and_then(|c| c.station_id)
            .map(|st| Slot::new(s.owner_id, st, crate::model::FLAG_HANGAR))
    };
    let Some(to) = dest else {
        return;
    };
    if let Some((_, held)) = apply_held(w, &[Edit::Move { item_id: id, to }]) {
        release(w, via, s.owner_id, held);
    }
    if carrier {
        tell(
            w,
            via,
            s.owner_id,
            Kind::Content {
                tube: s.tube,
                fighter: id,
                type_id: s.type_id,
                size: s.size,
            },
        );
        if forced {
            clear_task(w, via, s.owner_id, s.ship_id, s.tube);
        }
    }
}

/// Fighters a restart left in space, swept once per process: a category-87
/// item in a solar system with no live squadron goes to its owner's home
/// hangar. `[A]` the hangar, not the bay: the carrier it came from is not
/// recorded (squadrons are not persisted), and a hangar always takes it.
fn sweep_orphans(w: &mut Registry) {
    state_mut(w).swept = true;
    let orphans: Vec<(i64, i64)> = w
        .store()
        .items()
        .iter()
        .filter(|i| {
            i.category_id == fighters_data::CATEGORY_FIGHTER
                && (30_000_000..40_000_000).contains(&i.location_id)
                && state(w).squadron(i.item_id).is_none()
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

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The world clock's hook** (`modules::tick`, coordinator): the orphan
/// sweep once; every squadron whose carrier is no longer in its system
/// (docked, jumped, died, logged off) lands at once; every tube task whose
/// time is up moves on:
/// `LOADING -> READY`, `UNLOADING -> EMPTY`, `LAUNCHING -> INSPACE`,
/// `RECALLING -> LANDING` (the squadron lands), `LANDING -> REFUELLING` for
/// the type's `fighterRefuelingTime` (attribute 2426 [V-data]),
/// `REFUELLING -> READY`.
pub fn tick(w: &mut Registry, now: BlueTime) {
    if !state(w).swept {
        sweep_orphans(w);
    }
    // Abilities first, and before the early return: a holder's last squadron
    // may be gone with its holds still to be told to their targets.
    ability::run(w, now);
    // Bombs that are due and kamikaze runs (FT-G).
    strike::run(w, now);
    // Squadrons nobody controls that have lain long enough (FT-G).
    abandon::tick(w, now);
    if state(w).live.is_empty() && state(w).tasks.is_empty() {
        return;
    }
    let lost: Vec<i64> = state(w)
        .squadrons()
        .filter(|s| !state(w).is_abandoned(s.id))
        .filter(|s| {
            position_in(w, s.ship_id, s.solar_system_id).is_none() || w.roster().get(s.id).is_none()
        })
        .map(|s| s.id)
        .collect();
    for id in lost {
        land(w, Via::Clock, id, true, now);
    }
    let due: Vec<((i64, i64), Task)> = state(w)
        .tasks
        .iter()
        .filter(|(_, t)| t.until.is_some_and(|u| u <= now))
        .map(|(k, t)| (*k, *t))
        .collect();
    for ((ship, tube), task) in due {
        let owner = w.store().get(ship).map_or(0, |s| s.owner_id);
        let owner = if owner == 0 {
            state(w)
                .squadrons()
                .find(|s| s.ship_id == ship && s.tube == tube)
                .map_or(0, |s| s.owner_id)
        } else {
            owner
        };
        match task.status {
            Status::Loading | Status::Unloading | Status::Refuelling => {
                clear_task(w, Via::Clock, owner, ship, tube);
            }
            Status::Launching => {
                set_task(
                    w,
                    Via::Clock,
                    owner,
                    ship,
                    tube,
                    Task {
                        status: Status::InSpace,
                        start: now,
                        until: None,
                    },
                );
            }
            Status::Recalling => {
                let squadron = state(w)
                    .squadrons()
                    .find(|s| s.ship_id == ship && s.tube == tube)
                    .copied();
                match squadron {
                    Some(s) => land(w, Via::Clock, s.id, false, now),
                    None => clear_task(w, Via::Clock, owner, ship, tube),
                }
            }
            Status::Landing => {
                let refuel = tube_item(w, ship, tube)
                    .and_then(|(_, t, _)| fighter_type(w.content(), t))
                    .map_or(0.0, |f| f.refuel_ms);
                set_task(
                    w,
                    Via::Clock,
                    owner,
                    ship,
                    tube,
                    Task {
                        status: Status::Refuelling,
                        start: now,
                        until: Some(now + ms(refuel as i64)),
                    },
                );
            }
            Status::InSpace => {}
        }
    }
}

// ---------------------------------------------------------------------------
// The wire: fighterMgr
// ---------------------------------------------------------------------------

fn bad_args(req: &Request<'_>, want: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: format!("expected {want}"),
    }
}

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

/// **A squadron's health ran out** (`kill_hooks::PLAYER`, coordinator):
/// [`ability::destroyed`].
pub fn destroyed(w: &mut Registry, id: i64, park: Vec<i64>) {
    ability::destroyed(w, id, park);
}

/// A flying squadron's health profile, for `dogma::profile`'s fighter arm
/// (coordinator): [`ability::profile_of`].
pub fn profile_of(w: &Registry, id: i64) -> Option<crate::damage::Profile> {
    ability::profile_of(w, id)
}

/// `fighterMgr`, a plain remote service (`sm.RemoteSvc('fighterMgr')`,
/// `fightersSvc.py`) [V]. Movement and the ability calls are FT-E
/// (`ability.rs`); abandon and scoop are FT-G (`abandon.rs`).
pub fn dispatch(w: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetFightersForShip" => Ok(fighters_for_ship(w, req.actor)),
        "LoadFightersToTube" => {
            let (Some(PyValue::Int(fighter)), Some(PyValue::Int(tube))) =
                (req.args.first(), req.args.get(1))
            else {
                return Err(bad_args(req, "(fighterID, tubeFlagID)"));
            };
            load(w, req.actor, *fighter, *tube).map_err(|r| r.to_error())?;
            Ok(PyValue::None)
        }
        "UnloadTubeToFighterBay" => {
            let Some(PyValue::Int(tube)) = req.args.first() else {
                return Err(bad_args(req, "(tubeFlagID,)"));
            };
            unload(w, req.actor, *tube).map_err(|r| r.to_error())?;
            Ok(PyValue::None)
        }
        "LaunchFightersFromTubes" => {
            let tubes = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(tubeFlagIDs,)"))?;
            Ok(launch(w, req.actor, &tubes))
        }
        "RecallFightersToTubes" => {
            let ids = ids_arg(req.args.first()).ok_or_else(|| bad_args(req, "(fighterIDs,)"))?;
            Ok(recall(w, req.actor, &ids))
        }
        "ExecuteMovementCommandOnFighters" => {
            let ids = ids_arg(req.args.first())
                .ok_or_else(|| bad_args(req, "(fighterIDs, command, *args)"))?;
            let command = match req.args.get(1) {
                Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                Some(PyValue::Unicode(u)) => u.clone(),
                _ => return Err(bad_args(req, "(fighterIDs, command, *args)")),
            };
            let args = req.args.get(2..).unwrap_or(&[]);
            ability::movement(w, req.actor, &ids, &command, args).map_err(|e| match e {
                ability::MoveError::Refused(r) => r.to_error(),
                ability::MoveError::Args(why) => bad_args(req, why),
            })?;
            Ok(PyValue::None)
        }
        "CmdActivateAbilitySlots" => {
            let (Some(ids), Some(PyValue::Int(slot))) =
                (ids_arg(req.args.first()), req.args.get(1))
            else {
                return Err(bad_args(req, "(fighterIDs, abilitySlotID, *args)"));
            };
            let args = req.args.get(2..).unwrap_or(&[]);
            Ok(ability::activate(w, req.actor, &ids, *slot, args))
        }
        "CmdDeactivateAbilitySlots" => {
            let (Some(ids), Some(PyValue::Int(slot))) =
                (ids_arg(req.args.first()), req.args.get(1))
            else {
                return Err(bad_args(req, "(fighterIDs, abilitySlotID)"));
            };
            Ok(ability::deactivate(w, req.actor, &ids, *slot))
        }
        "CmdAbandonFighter" => {
            let Some(PyValue::Int(id)) = req.args.first() else {
                return Err(bad_args(req, "(fighterID,)"));
            };
            abandon::abandon(w, req.actor, *id).map_err(|r| r.to_error())?;
            Ok(PyValue::None)
        }
        "CmdScoopAbandonedFighterFromSpace" => {
            let (Some(PyValue::Int(id)), Some(PyValue::Int(flag))) =
                (req.args.first(), req.args.get(1))
            else {
                return Err(bad_args(req, "(fighterID, toFlagID)"));
            };
            abandon::scoop(w, req.actor, *id, *flag).map_err(|r| r.to_error())?;
            Ok(PyValue::None)
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fighters_data::attr as fa;
    use std::time::Instant;

    pub(super) const ALICE: i64 = 90_000_001;
    pub(super) const ALICE_SHIP: i64 = 100_000_001;
    pub(super) const SYSTEM: i64 = 30_000_001;
    pub(super) const STATION: i64 = 60_012_526;
    /// Invented rows, **not the client's numbers**: a carrier hull with 3
    /// tubes (2 light, 1 support, no heavy) and a 100 m3 bay, a hull with no
    /// bay, two light fighters, a support and a heavy one.
    pub(super) const CARRIER: i64 = 990_590_001;
    pub(super) const NO_BAY: i64 = 990_590_002;
    pub(super) const LIGHT_A: i64 = 990_590_010;
    pub(super) const LIGHT_B: i64 = 990_590_011;
    pub(super) const SUPPORT: i64 = 990_590_012;
    pub(super) const HEAVY: i64 = 990_590_013;
    /// A module row of `typedata::FIXTURE`: something that is not a fighter.
    pub(super) const GUN_TYPE: i64 = 990_000_002;
    /// Alice's bay stack, her hangar stack, a gun in the hangar.
    pub(super) const BAY: i64 = 100_000_100;
    pub(super) const HANGAR: i64 = 100_000_110;
    pub(super) const GUN: i64 = 100_000_120;
    pub(super) const BAY_CAPACITY: f64 = 100.0;
    pub(super) const LIGHT_VOLUME: f64 = 5.0;
    pub(super) const LIGHT_MAX: i64 = 9;
    /// Test fixture refuel time of LIGHT_A, ms.
    pub(super) const REFUEL_MS: i64 = 4_000;

    pub(super) const ROWS: &str = "\
type\t990590001\t99590\t6\t1\t1\t50000.0\t400.0\t10000000.0\t200.0\t0\t\t37=200.0;70=1.0;76=20000.0;2216=3.0;2217=2.0;2218=1.0;2055=100.0\tFixture Carrier
type\t990590002\t99590\t6\t1\t1\t20000.0\t100.0\t1000000.0\t40.0\t0\t\t37=300.0\tFixture No Bay Hull
type\t990590010\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=100.0;552=30.0;2212=1.0;2215=9.0;2223=2000.0;2426=4000.0\tFixture Light Fighter
type\t990590011\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;2212=1.0;2215=9.0\tFixture Other Light Fighter
type\t990590012\t99592\t87\t1\t1\t10.0\t0.0\t1000.0\t12.0\t0\t\t37=2000.0;2213=1.0;2215=6.0\tFixture Support Fighter
type\t990590013\t99593\t87\t1\t1\t20.0\t0.0\t1000.0\t12.0\t0\t\t37=1500.0;2214=1.0;2215=3.0\tFixture Heavy Fighter
";

    pub(super) fn content() -> crate::content::Content {
        let mut c = crate::content::Content::fixture();
        let text = format!(
            "{}{ROWS}{}{}",
            typedata::FIXTURE,
            ability::tests::rows(),
            fixture::rows()
        );
        let table = typedata::Table::parse(&text).expect("the test rows parse");
        c.insert("typedata", typedata::Installed::build(table));
        c.insert("fighters_data", ability::tests::table());
        c
    }

    /// Alice, docked at [`STATION`], in `hull`, with `bay` as (type, count)
    /// stacks in her fighter bay, 40 light fighters and a gun in her hangar.
    pub(super) fn world(hull: i64, bay: &[(i64, i64)]) -> Registry {
        world_with_bob(hull, bay, None)
    }

    pub(super) const BOB: i64 = 90_000_002;
    pub(super) const BOB_SHIP: i64 = 100_000_002;

    /// [`world`], and Bob docked beside her in `bob_hull` when there is one.
    pub(super) fn world_with_bob(hull: i64, bay: &[(i64, i64)], bob_hull: Option<i64>) -> Registry {
        let content = std::sync::Arc::new(content());
        let mut characters = vec![crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            ship_type_id: hull,
            ..crate::model::Character::default()
        }];
        if let Some(h) = bob_hull {
            characters.push(crate::model::Character {
                character_id: BOB,
                name: "Bob".to_owned(),
                ship_id: BOB_SHIP,
                ship_type_id: h,
                ..crate::model::Character::default()
            });
        }
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters,
            ..crate::model::SandboxConfig::default()
        })
        .with_content(content.clone());
        let mut items = reg.store().items().to_vec();
        for (ship, ty) in [(ALICE_SHIP, Some(hull)), (BOB_SHIP, bob_hull)] {
            let Some(ty) = ty else { continue };
            for i in items.iter_mut().filter(|i| i.item_id == ship) {
                let t = typedata::item_in(&content, ty).expect("a test hull");
                i.type_id = t.type_id;
                i.group_id = t.group_id;
            }
        }
        let stack = |item_id: i64, ty: i64, location_id: i64, flag_id: i64, quantity: i64| {
            let t = typedata::item_in(&content, ty).expect("a test type");
            crate::item::Item {
                item_id,
                type_id: ty,
                owner_id: ALICE,
                location_id,
                flag_id,
                quantity,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton: false,
            }
        };
        for (n, (ty, count)) in bay.iter().enumerate() {
            items.push(stack(
                BAY + n as i64,
                *ty,
                ALICE_SHIP,
                FLAG_FIGHTER_BAY,
                *count,
            ));
        }
        items.push(stack(
            HANGAR,
            LIGHT_A,
            STATION,
            crate::model::FLAG_HANGAR,
            40,
        ));
        items.push(stack(GUN, GUN_TYPE, STATION, crate::model::FLAG_HANGAR, 1));
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_200,
            ..crate::store::Snapshot::default()
        });
        let _ = reg.take_pending();
        reg
    }

    pub(super) fn undock(reg: &mut Registry) {
        destiny::on_ballpark_bind(reg, Some(ALICE));
        let _ = reg.take_pending();
    }

    pub(super) type Sent = (Option<Vec<i64>>, &'static str, Vec<PyValue>);

    pub(super) fn pending(reg: &mut Registry) -> Vec<Sent> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (name, args) = p.to_push();
                (p.audience(), name, args)
            })
            .collect()
    }

    pub(super) fn due(reg: &mut Registry) -> Vec<Sent> {
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| {
                let (name, args) = p.to_push();
                (p.audience(), name, args)
            })
            .collect()
    }

    pub(super) fn named<'a>(sent: &'a [Sent], name: &str) -> Vec<&'a Sent> {
        sent.iter().filter(|(_, n, _)| *n == name).collect()
    }

    pub(super) fn call(
        reg: &mut Registry,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "fighterMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    }

    pub(super) fn ints(v: &[i64]) -> PyValue {
        PyValue::List(v.iter().map(|i| PyValue::Int(*i)).collect())
    }

    pub(super) fn load_to(
        reg: &mut Registry,
        fighter: i64,
        tube: i64,
    ) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "LoadFightersToTube",
            &[PyValue::Int(fighter), PyValue::Int(tube)],
        )
    }

    /// The key of a raised `UserError`.
    pub(super) fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("a raised UserError, got {e:?}")
        };
        let PyValue::Tuple(h) = &**header else {
            panic!("header")
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("args")
        };
        let PyValue::Bytes(k) = &args[0] else {
            panic!("key")
        };
        String::from_utf8(k.clone()).expect("utf8")
    }

    /// The key of a `UserError` value in a reply dict.
    pub(super) fn key_of_value(v: &PyValue) -> String {
        key_of(&ServiceError::Raise(v.clone()))
    }

    /// Settle every timed task: all the clock steps a squadron's life needs.
    pub(super) fn settle(reg: &mut Registry) -> BlueTime {
        let mut now = crate::pyutil::filetime_now();
        for _ in 0..6 {
            now += ms(60_000);
            tick(reg, now);
        }
        now
    }

    pub(super) fn tube_of(reg: &Registry, tube: i64) -> Option<(i64, i64)> {
        tube_item(reg, ALICE_SHIP, tube).map(|(_, t, n)| (t, n))
    }

    pub(super) fn bay_count(reg: &Registry, ty: i64) -> i64 {
        reg.store()
            .contents_of(ALICE_SHIP)
            .filter(|i| i.flag_id == FLAG_FIGHTER_BAY && i.type_id == ty)
            .map(|i| i.quantity)
            .sum()
    }

    pub(super) fn fighter_total(reg: &Registry) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.category_id == 87)
            .map(|i| i.quantity)
            .sum()
    }

    // -- the bay ------------------------------------------------------------

    #[test]
    fn bay_refuses_a_non_fighter() {
        let reg = world(CARRIER, &[]);
        let r = plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, GUN, None).expect_err("a gun");
        assert_eq!(r.key, "ItemCannotBeInFighterBay");
    }

    #[test]
    fn bay_capacity_counts_stack_volume() {
        let mut reg = world(CARRIER, &[(LIGHT_B, 5)]);
        // 5 in the bay = 25 m3 of 100; the 40 in the hangar do not fit whole.
        let whole = plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, HANGAR, None).expect_err("full");
        assert_eq!(whole.key, "NotEnoughFighterBaySpace");
        let free = BAY_CAPACITY - 5.0 * LIGHT_VOLUME;
        let max = (free / LIGHT_VOLUME) as i64;
        assert!(plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, HANGAR, Some(max + 1)).is_err());
        to_bay(&mut reg, Some(ALICE), ALICE_SHIP, HANGAR, Some(max)).expect("exactly the room");
        assert_eq!(bay_count(&reg, LIGHT_A), max);
        assert!((bay_used(&reg, ALICE_SHIP) - BAY_CAPACITY).abs() < 1e-9);
    }

    #[test]
    fn a_pod_has_no_fighter_bay() {
        let reg = world(NO_BAY, &[]);
        let r = plan_to_bay(&reg, Some(ALICE), ALICE_SHIP, HANGAR, Some(1)).expect_err("no bay");
        assert_eq!(r.key, "ShipHasNoFighterBay");
        let r = plan_load(&reg, Some(ALICE), HANGAR, FIRST_TUBE).expect_err("no tubes");
        assert_eq!(r.key, "ShipHasNoFighterBay");
    }

    #[test]
    fn list_fighter_bay_returns_only_flag_158() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 9), (SUPPORT, 2)]);
        load_to(&mut reg, BAY + 1, FIRST_TUBE).expect("a support squadron");
        let expected = bay_capacity_value(&reg, ALICE_SHIP);
        let bind = PyValue::Tuple(vec![PyValue::Int(ALICE_SHIP), PyValue::Int(0)]);
        let mut read = |method: &str, args: &[PyValue]| {
            reg.dispatch(&Request {
                service: "invbroker::inventory",
                method,
                args,
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(ALICE),
            })
            .expect("answered")
        };
        let PyValue::List(rows) = read("ListFighterBay", &[]) else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1, "the tube's squadron is not in the bay");
        assert_eq!(
            read("GetCapacity", &[PyValue::Int(FLAG_FIGHTER_BAY)]),
            expected,
            "the bay flag asks for the bay's own capacity"
        );
    }

    #[test]
    fn get_capacity_for_the_bay_flag_is_the_hulls_fighter_capacity() {
        let reg = world(CARRIER, &[(LIGHT_A, 4)]);
        let v = bay_capacity_value(&reg, ALICE_SHIP);
        assert_eq!(
            v,
            crate::pyutil::keyval(vec![
                ("capacity", PyValue::Real(BAY_CAPACITY)),
                ("used", PyValue::Real(4.0 * LIGHT_VOLUME)),
            ])
        );
    }

    // -- load and unload ----------------------------------------------------

    #[test]
    fn load_splits_the_bay_stack_to_max_squadron_size() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 20)]);
        assert_eq!(load_to(&mut reg, BAY, FIRST_TUBE), Ok(PyValue::None));
        assert_eq!(tube_of(&reg, FIRST_TUBE), Some((LIGHT_A, LIGHT_MAX)));
        assert_eq!(bay_count(&reg, LIGHT_A), 20 - LIGHT_MAX);
        assert_eq!(fighter_total(&reg), 20 + 40, "nothing created or lost");
        // A stack that is all one squadron moves whole.
        let mut reg = world(CARRIER, &[(LIGHT_A, 4)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("a short squadron");
        assert_eq!(tube_of(&reg, FIRST_TUBE), Some((LIGHT_A, 4)));
        assert_eq!(bay_count(&reg, LIGHT_A), 0);
        assert_eq!(reg.store().get(BAY).map(|i| i.flag_id), Some(FIRST_TUBE));
    }

    #[test]
    fn load_tells_the_pilot_the_content_then_the_loading_task() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 20)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        let sent = pending(&mut reg);
        let content = named(&sent, "OnFighterTubeContentUpdate");
        assert_eq!(content.len(), 1);
        let (to, _, args) = content[0];
        assert_eq!(to.as_deref(), Some(&[ALICE][..]));
        assert_eq!(args[0], PyValue::Int(FIRST_TUBE));
        assert_eq!(args[2], PyValue::Int(LIGHT_A));
        assert_eq!(args[3], PyValue::Int(LIGHT_MAX));
        let task = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(task.len(), 1);
        assert_eq!(task[0].2[1], crate::pyutil::bstr("LOADING"));
        assert_eq!(task[0].2.len(), 4, "tube, status, start, end");
        let pos = |n: &str| sent.iter().position(|(_, name, _)| *name == n);
        assert!(pos("OnFighterTubeContentUpdate") < pos("OnFighterTubeTaskStatus"));
        // And the loading ends in READY.
        tick(
            &mut reg,
            crate::pyutil::filetime_now() + ms(LOAD_MS + 1_000),
        );
        let sent = due(&mut reg);
        let task = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(task[0].2[1], crate::pyutil::bstr("READY"));
        assert!(state(&reg).task(ALICE_SHIP, FIRST_TUBE).is_none());
    }

    #[test]
    fn a_top_up_grows_the_loaded_tube() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 4)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("first");
        let mut items = reg.store().items().to_vec();
        let t = typedata::item_in(reg.content(), LIGHT_A).expect("type");
        items.push(crate::item::Item {
            item_id: 100_000_150,
            type_id: LIGHT_A,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: FLAG_FIGHTER_BAY,
            quantity: 20,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_200,
            ..crate::store::Snapshot::default()
        });
        settle(&mut reg);
        load_to(&mut reg, 100_000_150, FIRST_TUBE).expect("a top-up");
        assert_eq!(tube_of(&reg, FIRST_TUBE), Some((LIGHT_A, LIGHT_MAX)));
        assert_eq!(bay_count(&reg, LIGHT_A), 20 - (LIGHT_MAX - 4));
        settle(&mut reg);
        let full = load_to(&mut reg, 100_000_150, FIRST_TUBE).expect_err("full");
        assert_eq!(key_of(&full), "CannotLoadFighterTubeSquadronIsFull");
    }

    #[test]
    fn second_type_in_a_loaded_tube_is_refused() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 4), (LIGHT_B, 4)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("first");
        settle(&mut reg);
        let e = load_to(&mut reg, BAY + 1, FIRST_TUBE).expect_err("a second type");
        assert_eq!(
            key_of(&e),
            "CannotLoadFighterTubeDifferentTypeAlreadyLoaded"
        );
    }

    #[test]
    fn class_slot_limit_per_hull() {
        let mut reg = world(
            CARRIER,
            &[(LIGHT_A, 9), (LIGHT_B, 9), (SUPPORT, 6), (HEAVY, 3)],
        );
        // 2 light slots: two light tubes load, a third light squadron cannot
        // (and neither can a heavy: the hull has none).
        load_to(&mut reg, BAY, FIRST_TUBE).expect("light 1");
        load_to(&mut reg, BAY + 1, FIRST_TUBE + 1).expect("light 2");
        settle(&mut reg);
        let e = load_to(&mut reg, BAY + 3, FIRST_TUBE + 2).expect_err("no heavy slot");
        assert_eq!(key_of(&e), "CannotLoadFighterTubeNoTubesExistForClass");
        // The support squadron takes the third tube.
        load_to(&mut reg, BAY + 2, FIRST_TUBE + 2).expect("support");
        assert_eq!(tube_of(&reg, FIRST_TUBE + 2), Some((SUPPORT, 6)));
    }

    #[test]
    fn class_slots_count_tubes_not_fighters() {
        // Two light slots, three tubes: two light tubes of any size use the
        // class up; a third light squadron is refused with a free tube left.
        let mut reg = world(CARRIER, &[(LIGHT_A, 9), (LIGHT_B, 2), (LIGHT_A, 3)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("1");
        load_to(&mut reg, BAY + 1, FIRST_TUBE + 1).expect("2");
        settle(&mut reg);
        let e = load_to(&mut reg, BAY + 2, FIRST_TUBE + 2).expect_err("a third light tube");
        assert_eq!(key_of(&e), "CannotLoadFighterTubeNoTubesAvailableForClass");
    }

    #[test]
    fn tube_beyond_fighter_tubes_is_refused() {
        let mut reg = world(CARRIER, &[(SUPPORT, 6)]);
        // The hull has 3 tubes: 159, 160, 161. 162 and a non-tube flag are out.
        for tube in [FIRST_TUBE + 3, LAST_TUBE, 87, 5] {
            let e = load_to(&mut reg, BAY, tube).expect_err("no such tube");
            assert_eq!(key_of(&e), "CannotLoadFighterTubeNoTubesAvailableForClass");
        }
        assert_eq!(bay_count(&reg, SUPPORT), 6);
    }

    #[test]
    fn load_from_a_non_bay_item_is_refused() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 9)]);
        for source in [HANGAR, GUN, 999_999] {
            let e = load_to(&mut reg, source, FIRST_TUBE).expect_err("not in the bay");
            assert_eq!(key_of(&e), "CannotLoadFighterSourceNotInFighterBay");
        }
    }

    #[test]
    fn unload_returns_the_stack_and_merges() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 20)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        settle(&mut reg);
        let _ = pending(&mut reg);
        let _ = due(&mut reg);
        call(
            &mut reg,
            "UnloadTubeToFighterBay",
            &[PyValue::Int(FIRST_TUBE)],
        )
        .expect("unloaded");
        assert_eq!(tube_of(&reg, FIRST_TUBE), None);
        assert_eq!(bay_count(&reg, LIGHT_A), 20, "merged back into one stack");
        let stacks = reg
            .store()
            .contents_of(ALICE_SHIP)
            .filter(|i| i.flag_id == FLAG_FIGHTER_BAY)
            .count();
        assert_eq!(stacks, 1);
        assert_eq!(fighter_total(&reg), 20 + 40);
        let sent = pending(&mut reg);
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        assert!(names.contains(&"OnFighterTubeContentEmpty"), "{names:?}");
        let task = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(task[0].2[1], crate::pyutil::bstr("UNLOADING"));
        // Unloading an empty tube is refused, and the busy tube is too.
        let e = call(
            &mut reg,
            "UnloadTubeToFighterBay",
            &[PyValue::Int(FIRST_TUBE)],
        )
        .expect_err("busy");
        assert_eq!(key_of(&e), "FighterTubeIsBusy");
        settle(&mut reg);
        let _ = due(&mut reg);
        assert!(
            call(
                &mut reg,
                "UnloadTubeToFighterBay",
                &[PyValue::Int(FIRST_TUBE)]
            )
            .is_err()
        );
    }

    #[test]
    fn unload_into_a_full_bay_is_refused() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 9)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        settle(&mut reg);
        // Fill the bay with the other type, to the brim.
        let rest = ((BAY_CAPACITY / LIGHT_VOLUME) as i64).max(1);
        to_bay(&mut reg, Some(ALICE), ALICE_SHIP, HANGAR, Some(rest)).expect("fills the bay");
        let e = call(
            &mut reg,
            "UnloadTubeToFighterBay",
            &[PyValue::Int(FIRST_TUBE)],
        )
        .expect_err("no room");
        assert_eq!(key_of(&e), "NotEnoughFighterBaySpace");
        assert_eq!(tube_of(&reg, FIRST_TUBE), Some((LIGHT_A, 9)));
    }

    #[test]
    fn get_fighters_for_ship_row_order_is_tube_item_type_size() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 4), (SUPPORT, 6)]);
        load_to(&mut reg, BAY + 1, FIRST_TUBE + 1).expect("support");
        load_to(&mut reg, BAY, FIRST_TUBE).expect("light");
        let PyValue::Tuple(parts) = call(&mut reg, "GetFightersForShip", &[]).expect("rows") else {
            panic!("a 3-tuple")
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            PyValue::List(vec![
                PyValue::Tuple(vec![
                    PyValue::Int(FIRST_TUBE),
                    PyValue::Int(BAY),
                    PyValue::Int(LIGHT_A),
                    PyValue::Int(4),
                ]),
                PyValue::Tuple(vec![
                    PyValue::Int(FIRST_TUBE + 1),
                    PyValue::Int(BAY + 1),
                    PyValue::Int(SUPPORT),
                    PyValue::Int(6),
                ]),
            ]),
            "(tubeFlagID, fighterItemID, fighterTypeID, squadronSize), by tube"
        );
        assert_eq!(parts[1], PyValue::List(vec![]));
        assert_eq!(parts[2], PyValue::Dict(vec![]));
    }

    // -- launch and recall --------------------------------------------------

    /// Alice in space with a light squadron READY in tube 0, and the
    /// squadron's item id (a split of the bay stack, so a new id).
    pub(super) fn launched_world() -> (Registry, i64) {
        let mut reg = world(CARRIER, &[(LIGHT_A, 20)]);
        undock(&mut reg);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        settle(&mut reg);
        let _ = pending(&mut reg);
        let _ = due(&mut reg);
        let sq = tube_item(&reg, ALICE_SHIP, FIRST_TUBE).expect("loaded").0;
        (reg, sq)
    }

    pub(super) fn launch_tube(reg: &mut Registry, tube: i64) -> PyValue {
        call(reg, "LaunchFightersFromTubes", &[ints(&[tube])]).expect("answered")
    }

    #[test]
    fn launch_moves_the_item_out_of_the_ship_with_old_location_in_change() {
        let (mut reg, sq) = launched_world();
        let answer = launch_tube(&mut reg, FIRST_TUBE);
        assert_eq!(
            answer,
            PyValue::Dict(vec![(PyValue::None, PyValue::Int(FIRST_TUBE))])
        );
        let item = reg.store().get(sq).expect("the squadron is its item");
        assert_eq!(
            (item.location_id, item.flag_id, item.quantity),
            (SYSTEM, crate::combat::FLAG_IN_SPACE, LIGHT_MAX)
        );
        let sent = pending(&mut reg);
        let change = named(&sent, "OnItemChange");
        assert!(!change.is_empty());
        let (_, _, args) = change
            .iter()
            .find(|(_, _, a)| matches!(&a[0], PyValue::DbRow(_)))
            .expect("the item row");
        let PyValue::Dict(pairs) = &args[1] else {
            panic!("the change dict")
        };
        assert!(
            pairs.contains(&(
                PyValue::Int(ALICE_SHIP),
                PyValue::Int(crate::item::IX_LOCATION_ID)
            )),
            "the old location is the ship: {pairs:?}"
        );
    }

    #[test]
    fn launch_reply_is_a_dict_keyed_by_tube() {
        let (mut reg, _) = launched_world();
        let answer = call(
            &mut reg,
            "LaunchFightersFromTubes",
            &[ints(&[FIRST_TUBE, FIRST_TUBE + 1])],
        )
        .expect("answered");
        let PyValue::Dict(pairs) = answer else {
            panic!("a dict")
        };
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(PyValue::None, PyValue::Int(FIRST_TUBE))));
        let empty = pairs
            .iter()
            .find(|(_, k)| *k == PyValue::Int(FIRST_TUBE + 1))
            .expect("the empty tube is answered too");
        assert_eq!(key_of_value(&empty.0), "CustomNotify");
    }

    #[test]
    fn launch_refused_while_warping_is_a_user_error_value() {
        let (mut reg, sq) = launched_world();
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = destiny::Mode::Warp {
            // Activated just now: `advance` has a long flight ahead of it.
            effect_stamp: destiny::tick() as i32,
            total_distance: 4.0 * 149_597_870_700.0,
            goto: [4.0 * 149_597_870_700.0, 0.0, 0.0],
            stop_distance: 0.0,
            warp_factor: 3000,
        };
        let PyValue::Dict(pairs) = launch_tube(&mut reg, FIRST_TUBE) else {
            panic!("a dict")
        };
        assert_eq!(
            key_of_value(&pairs[0].0),
            "CannotOpenFighterTubesWhileWarping"
        );
        assert_eq!(reg.store().get(sq).map(|i| i.location_id), Some(ALICE_SHIP));
    }

    #[test]
    fn launch_while_docked_or_busy_is_refused_per_tube() {
        let mut reg = world(CARRIER, &[(LIGHT_A, 9)]);
        load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        settle(&mut reg);
        let PyValue::Dict(pairs) = launch_tube(&mut reg, FIRST_TUBE) else {
            panic!("a dict")
        };
        assert_eq!(key_of_value(&pairs[0].0), "CustomNotify", "docked");
        assert_eq!(
            reg.store().get(BAY).map(|i| i.location_id),
            Some(ALICE_SHIP)
        );
        // In space, a tube still LOADING is busy.
        let (mut reg, _) = launched_world();
        let tube = FIRST_TUBE + 1;
        let mut items = reg.store().items().to_vec();
        let t = typedata::item_in(reg.content(), LIGHT_B).expect("type");
        items.push(crate::item::Item {
            item_id: 100_000_160,
            type_id: LIGHT_B,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: FLAG_FIGHTER_BAY,
            quantity: 4,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: false,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 100_000_200,
            ..crate::store::Snapshot::default()
        });
        load_to(&mut reg, 100_000_160, tube).expect("loaded");
        let PyValue::Dict(pairs) = launch_tube(&mut reg, tube) else {
            panic!("a dict")
        };
        assert_eq!(key_of_value(&pairs[0].0), "FighterTubeIsBusy");
    }

    #[test]
    fn a_launched_squadron_is_a_ball_whose_slim_carries_the_dotted_size() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let p = reg.roster().get(sq).expect("a ball under the item id");
        assert_eq!((p.ship_id, p.solar_system_id), (sq, SYSTEM));
        let s = *state(&reg).squadron(sq).expect("live");
        assert_eq!(
            (s.owner_id, s.ship_id, s.tube, s.size),
            (ALICE, ALICE_SHIP, FIRST_TUBE, LIGHT_MAX)
        );
        assert_eq!(
            slim_extras(&reg, &[sq, 12345]),
            vec![(sq, vec![("fighter.squadronSize", PyValue::Int(LIGHT_MAX))])]
        );
        assert_eq!(slim(&reg, &s).owner_id, ALICE, "the character owns it");
        assert_eq!(
            reg.pilot(sq).map(|p| (p.ship_type_id, p.owner_id)),
            Some((LIGHT_A, ALICE))
        );
        assert!(crate::dogma::hull(&reg, sq).is_some());
    }

    #[test]
    fn launch_tells_the_balls_then_the_tube_then_the_controller_then_the_item() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let sent = pending(&mut reg);
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        let at = |n: &str| {
            names
                .iter()
                .position(|x| *x == n)
                .unwrap_or_else(|| panic!("{n}: {names:?}"))
        };
        assert!(at("DoDestinyUpdate") < at("OnFighterTubeContentEmpty"));
        assert!(at("OnFighterTubeContentEmpty") < at("OnFighterTubeTaskStatus"));
        assert!(at("OnFighterTubeTaskStatus") < at("OnFighterAddedToController"));
        let added = named(&sent, "OnFighterAddedToController");
        assert_eq!(
            added[0].2[..4],
            [
                PyValue::Int(sq),
                PyValue::Int(LIGHT_A),
                PyValue::Int(FIRST_TUBE),
                PyValue::Int(LIGHT_MAX)
            ]
        );
        let PyValue::Tuple(states) = &added[0].2[4] else {
            panic!("(chargeStates, cooldownStates)")
        };
        assert_eq!(states.len(), 2);
        let task = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(task[0].2[1], crate::pyutil::bstr("LAUNCHING"));
        assert!(names.contains(&"OnItemChange"));
    }

    #[test]
    fn get_fighters_for_ship_lists_squadrons_in_space() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let PyValue::Tuple(parts) = call(&mut reg, "GetFightersForShip", &[]).expect("rows") else {
            panic!("3-tuple")
        };
        assert_eq!(parts[0], PyValue::List(vec![]));
        assert_eq!(
            parts[1],
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(FIRST_TUBE),
                PyValue::Int(sq),
                PyValue::Int(LIGHT_A),
                PyValue::Int(LIGHT_MAX),
            ])])
        );
        let PyValue::Dict(states) = &parts[2] else {
            panic!("a dict")
        };
        assert_eq!(states.len(), 1);
    }

    #[test]
    fn launching_becomes_in_space_and_recall_then_refuel_returns_the_tube_to_ready() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let _ = pending(&mut reg);
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0 + ms(LAUNCH_MS + 1_000));
        assert_eq!(
            state(&reg).task(ALICE_SHIP, FIRST_TUBE).map(|t| t.status),
            Some(Status::InSpace)
        );
        let _ = due(&mut reg);
        // Recall: `{fighterID: None}`, the client sets RECALLING itself.
        assert_eq!(
            call(&mut reg, "RecallFightersToTubes", &[ints(&[sq])]).expect("answered"),
            PyValue::Dict(vec![(PyValue::None, PyValue::Int(sq))])
        );
        assert_eq!(
            state(&reg).task(ALICE_SHIP, FIRST_TUBE).map(|t| t.status),
            Some(Status::Recalling)
        );
        let _ = pending(&mut reg);
        // The flight ends: the status LANDING first, then the controller
        // event, then the item is back in its tube.
        let t1 = t0 + ms(LAUNCH_MS + RECALL_MAX_MS + 5_000);
        tick(&mut reg, t1);
        let sent = due(&mut reg);
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        let at = |n: &str| {
            names
                .iter()
                .position(|x| *x == n)
                .unwrap_or_else(|| panic!("{n}: {names:?}"))
        };
        let landing = sent
            .iter()
            .position(|(_, n, a)| {
                *n == "OnFighterTubeTaskStatus" && a[1] == crate::pyutil::bstr("LANDING")
            })
            .expect("LANDING");
        assert!(landing < at("OnFighterRemovedFromController"), "{names:?}");
        assert!(at("OnFighterRemovedFromController") < at("OnFighterTubeContentUpdate"));
        assert!(reg.roster().get(sq).is_none(), "the ball is gone");
        assert_eq!(tube_of(&reg, FIRST_TUBE), Some((LIGHT_A, LIGHT_MAX)));
        assert_eq!(
            reg.store().get(sq).map(|i| (i.location_id, i.flag_id)),
            Some((ALICE_SHIP, FIRST_TUBE))
        );
        // LANDING -> REFUELLING for the type's refuel time -> READY.
        let t2 = t1 + ms(LAND_MS + 1_000);
        tick(&mut reg, t2);
        let sent = due(&mut reg);
        let refuel = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(refuel[0].2[1], crate::pyutil::bstr("REFUELLING"));
        let (PyValue::Int(start), PyValue::Int(end)) = (&refuel[0].2[2], &refuel[0].2[3]) else {
            panic!("times")
        };
        assert_eq!(end - start, ms(REFUEL_MS));
        tick(&mut reg, t2 + ms(REFUEL_MS + 1_000));
        let sent = due(&mut reg);
        assert_eq!(
            named(&sent, "OnFighterTubeTaskStatus")[0].2[1],
            crate::pyutil::bstr("READY")
        );
        assert!(state(&reg).task(ALICE_SHIP, FIRST_TUBE).is_none());
        // And it launches again.
        let PyValue::Dict(pairs) = launch_tube(&mut reg, FIRST_TUBE) else {
            panic!("a dict")
        };
        assert_eq!(pairs[0].0, PyValue::None);
    }

    #[test]
    fn recall_refuses_a_stranger_and_a_squadron_not_in_space_yet() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let PyValue::Dict(pairs) =
            call(&mut reg, "RecallFightersToTubes", &[ints(&[sq, 999])]).expect("answered")
        else {
            panic!("a dict")
        };
        let by_id = |id: i64| {
            pairs
                .iter()
                .find(|(_, k)| *k == PyValue::Int(id))
                .expect("answered")
        };
        assert_eq!(
            key_of_value(&by_id(sq).0),
            "FighterTubeIsBusy",
            "still LAUNCHING"
        );
        assert_eq!(
            key_of_value(&by_id(999).0),
            "CannotControlFighterNotControlledByYourShip"
        );
    }

    #[test]
    fn jump_recalls_every_squadron_first() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        let _ = pending(&mut reg);
        // The carrier leaves the system (a jump, a dock, a logoff).
        reg.roster_mut().leave(ALICE);
        tick(&mut reg, crate::pyutil::filetime_now());
        assert!(reg.roster().get(sq).is_none());
        assert!(state(&reg).squadron(sq).is_none());
        assert_eq!(
            reg.store().get(sq).map(|i| (i.location_id, i.flag_id)),
            Some((ALICE_SHIP, FIRST_TUBE)),
            "back in the tube it left"
        );
        assert!(state(&reg).task(ALICE_SHIP, FIRST_TUBE).is_none());
        let sent = due(&mut reg);
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        assert!(
            names.contains(&"OnFighterRemovedFromController"),
            "{names:?}"
        );
        let last = named(&sent, "OnFighterTubeTaskStatus");
        assert_eq!(
            last.last().expect("a status").2[1],
            crate::pyutil::bstr("READY")
        );
        assert_eq!(fighter_total(&reg), 20 + 40);
    }

    #[test]
    fn restart_sweeps_orphan_squadrons_to_the_hangar() {
        let (mut reg, sq) = launched_world();
        launch_tube(&mut reg, FIRST_TUBE);
        // A restart: the live table is gone, the item is still in space.
        *state_mut(&mut reg) = State::default();
        reg.roster_mut().leave(sq);
        reg.roster_mut().leave(ALICE);
        let mut reg2 = reg;
        // The home station is the character's.
        reg2.character_mut(ALICE).expect("alice").station_id = Some(STATION);
        tick(&mut reg2, crate::pyutil::filetime_now());
        assert_eq!(
            reg2.store().get(sq).map(|i| (i.location_id, i.flag_id)),
            Some((STATION, crate::model::FLAG_HANGAR))
        );
        assert_eq!(fighter_total(&reg2), 20 + 40);
    }

    #[test]
    fn a_squadron_flies_on_its_types_numbers() {
        let c = content();
        let h = hull(&c, LIGHT_A).expect("a fighter hull");
        let row = typedata::installed_in(&c)
            .table()
            .get(LIGHT_A)
            .expect("row");
        assert_eq!(h.max_velocity, 3000.0);
        assert_eq!(h.hp, 100.0);
        assert_eq!((h.mass, h.radius), (row.mass, row.radius as f32));
        assert!(hull(&c, CARRIER).is_none(), "a ship is not a fighter");
        let ft = fighter_type(&c, LIGHT_A).expect("a fighter");
        assert_eq!(ft.squadron_max, LIGHT_MAX);
        assert_eq!(ft.refuel_ms as i64, REFUEL_MS);
        assert_eq!(ft.orbit_range, 2000.0);
        let _ = fa::TUBES;
    }

    #[test]
    fn an_unknown_method_is_no_such_method() {
        let mut reg = world(CARRIER, &[]);
        assert!(matches!(
            call(&mut reg, "CmdNoSuchThing", &[PyValue::Int(1)]),
            Err(ServiceError::NoSuchMethod { .. })
        ));
    }
}
