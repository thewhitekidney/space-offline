//! **Docking in an Upwell structure** (lane U4, issue #64; spec
//! `handback/r-upwell` §2, §5, §6, §7). [`crate::structure`] is the model,
//! [`crate::structure_space`] puts it in a system; this is what lets a pilot
//! go *into* it, live there, and come out again.
//!
//! # The model: the dockable id is `Character::station_id`
//!
//! A pilot docked in a structure has `station_id == structure id`, with the
//! structure's type, owner corporation and system in the other
//! `station_*` fields, exactly as at an NPC station (`dock_at`'s fields).
//! The hangar is then the same store address (`location_id == structure id`,
//! flag 4), the 109 `docked_station` readers keep working, and what
//! **branches** is a short list, all of which call [`is_structure`]:
//!
//! - the session keys: a dock is `{structureid: (None, id)}` and nothing
//!   else, an undock `{structureid: (id, None)}`, and a login is
//!   `stationid` None with `structureid` and `solarsystemid` set
//!   ([`Registry::character_change`]). `solarsystemid` **stays set** while
//!   docked in a structure **[V]** (`eveSessions.GetSessionValuesFromItemID`
//!   walks ship, structure, system);
//! - the inventory binds: the client asks the **solar-system** `invbroker`
//!   for `GetInventory(10014)` (`containerStructure`) or
//!   `GetInventoryFromId(structureid)` **[V]** (`invCache.py:489-492`), and
//!   [`hangar_alias`] reads both as the hangar (10004);
//! - the ball: `ball_target` and `station_position` know a structure;
//! - the ballpark bind ([`on_bind`]), the hard part, below.
//!
//! # The bind guard (spec §5)
//!
//! `on_ballpark_bind` used to mean "a ship has just arrived in space". A
//! pilot docked in a structure binds `beyonce` too, when the view is
//! `structure` (Enter Structure) **[V]** (`michelle.UpdateBallpark` keeps a
//! park only for the `inflight` and `structure` views, and the default
//! `hangar` view has none). So:
//!
//! - **Docked, no undock armed**: a bind is a docked bind. The pilot gets a
//!   `SetState` whose **ego is the structure** ([`docked_state`]; the
//!   structure's own ball is in the scene), no ship ball and no roster
//!   entry. The client is remembered as holding a live park.
//! - **Undock, no live park** (the hangar view): the call arms the undock,
//!   and the bind that follows is the ordinary arrival.
//! - **Undock, live park** (the structure view, which does **not** re-bind
//!   **[V]**, `gameui._HandleSessionChangeInSpace` only moves the ego): the
//!   ship is seated at once and the park is re-seeded with a `SetState` whose
//!   ego is the ship (`destiny::seat_into_live_park`).
//! - **A bind we did not expect** after that seating (the hangar
//!   view, the park remembered from an earlier visit): the client made a new
//!   park, so it gets a fresh state (the mark lasts until the first bind).
//!
//! # Control (lane U6C, issue #115)
//!
//! A docked pilot can take the controls (`structureControl.TakeControl`): the
//! client's `session.shipid` becomes the structure's id, which is what makes
//! it list the structure's fuel and deed bays and fit its service slots.
//! [`control`] has it. The character's own hull is untouched: only the
//! session (and what `GetAllInfo` primes) says the structure is the ship.
//!
//! # What happens to the pilots when it is unanchored
//!
//! [`evacuate`], called from the end of an unanchoring: the pilots docked are
//! undocked into space, and every item in its hangar goes to a station that
//! can take it ([`harbour`]).
//!
//! # [A]s (falsifier in each)
//!
//! - A docked bind with ego = structure and no ship ball is a valid
//!   `SetState` (`GetMyPosition` works on the structure ball). Falsifier:
//!   `fci_upwell_view` shows a traceback, a ghost ship or no ego.
//! - `GetMyHangarViewState` as a `KeyVal` with the seven attributes the
//!   consumer reads (`modularHangarBehaviours.UpdateStructureState`) is
//!   accepted. Falsifier: a traceback when the hangar loads.
//! - A ship that changed since the click is refused with a `CustomNotify`,
//!   not CCP's `DeniedShipChanged` (EvEJS hint only). Falsifier: the dialog
//!   reads wrong.
//! - Docking uses the setting 19 error as a refusal and ignores the NPC-only
//!   rules (outlaws in high-sec). Falsifier: the owner expects an outlaw
//!   turned away from a high-sec Astrahus.
//! - Items left in a structure that is unanchored go to the NPC station of
//!   the system, else to their owner's clone home. Falsifier: the owner
//!   wants asset safety (a wrap that costs ISK) or the stuff to wait.
//! - A structure with no locator rows (the data gap U0 closes) lets every
//!   hull dock and puts the ship 1.5 km off its surface. Falsifier: a ship
//!   undocking inside the model.

use crate::item::{Edit, Slot};
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use crate::structure::{self, Structure};
use crate::structure_profile as sp;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) mod control;

/// `containerStructure`: the id the client binds the structure's inventory
/// by **[V]** (`inventorycommon/const.py`).
pub const CONTAINER_STRUCTURE: i64 = 10_014;

/// `containerHangar`, which the inventory arms know the hangar by.
const CONTAINER_HANGAR: i64 = 10_004;

/// How far outside the structure's surface a ship appears when the type has
/// no locator rows **[A]**.
pub const FALLBACK_MARGIN: f64 = 1_500.0;

// -- the transient state ---------------------------------------------------------

/// Who is docked in a structure and what their client holds. Not persisted:
/// a restart forgets it, and a docked pilot logs in with no park, which is
/// the default hangar view.
#[derive(Debug, Default)]
pub struct Docked {
    /// Clients whose ballpark is up while docked (the `structure` view).
    live: BTreeSet<i64>,
    /// An undock was accepted and the bind that seats the ship is due.
    undocking: BTreeSet<i64>,
    /// Seated by an undock into a live park, until the first bind.
    seated: BTreeSet<i64>,
    /// Structure -> the pilot at its controls (`control`, lane U6C). Never
    /// persisted: a restart or a log-in is a pilot in his own ship.
    controls: BTreeMap<i64, i64>,
}

/// The `world_ext::SLOTS` row (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "structure_dock",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Docked::default())
}

fn forget(reg: &mut Registry, actor: i64) {
    let d = reg.ext_mut::<Docked>();
    d.live.remove(&actor);
    d.undocking.remove(&actor);
    d.seated.remove(&actor);
}

// -- what is a structure -----------------------------------------------------------

/// Whether `id` is an Upwell structure. Cheap in a world with none.
pub fn is_structure(reg: &Registry, id: i64) -> bool {
    let rows = reg.deltas().of(structure::KIND);
    !rows.is_empty() && rows.get(&structure::row_key(id)).is_some()
}

/// The structure `actor` is docked in right now.
pub fn docked_in(reg: &Registry, actor: i64) -> Option<i64> {
    reg.docked_station(actor)
        .filter(|id| is_structure(reg, *id))
}

/// The structure `actor`'s dogma sees them in: [`docked_in`] less a pilot
/// who has asked to leave (`undocking`), whose `station_id` stays the
/// structure until the ballpark bind. Priming the structure for them reaches
/// the client's `PrimeLocation(None)`, and the ship row must already say
/// the system.
pub fn dogma_host(reg: &Registry, actor: i64) -> Option<i64> {
    docked_in(reg, actor).filter(|_| !reg.ext::<Docked>().undocking.contains(&actor))
}

/// How many pilots are docked in `id`.
pub fn docked_count(reg: &Registry, id: i64) -> usize {
    if !is_structure(reg, id) {
        return 0;
    }
    reg.characters_at_station(id).len()
}

/// Why `actor` cannot use `service` where they are docked: `Some(text)` when
/// they are in a structure that does not offer it to them, `None` at a
/// station or when it is on offer (U5). Services are
/// [`sp::services_for`]'s, so U6's modules change the answer here with no
/// change in the callers.
pub fn service_refusal(reg: &Registry, actor: i64, service: i64) -> Option<&'static str> {
    let id = docked_in(reg, actor)?;
    let Some(s) = structure::get(reg.deltas(), id) else {
        return Some("This structure is gone.");
    };
    if sp::services_for(reg, &s, actor).contains_key(&service) {
        None
    } else {
        Some("This structure does not offer that service to you.")
    }
}

/// Where `id` is, as a station or as a structure: the one answer for a place
/// a clone, a contract or a home can name.
pub(crate) fn place_ref(reg: &Registry, id: i64) -> Option<crate::clones::StationRef> {
    match structure::get(reg.deltas(), id).filter(|_| is_structure(reg, id)) {
        Some(s) => Some(station_ref(reg, &s)),
        None => reg.station_ref(id),
    }
}

/// Whether `who` can live at `place` now: any NPC station, a structure only
/// while it exists, takes docking, and `who` holds the right (the access
/// rule, [`sp::may_dock`]). A clone, a home or a revive asks this before it
/// sends a pilot into a structure that may have turned them away since.
pub(crate) fn usable_by(reg: &Registry, who: i64, place: &crate::clones::StationRef) -> bool {
    if !is_structure(reg, place.station_id) {
        return true;
    }
    structure::get(reg.deltas(), place.station_id).is_some_and(|s| sp::may_dock(reg, &s, who))
}

/// A revival, clone jump or self-destruct moved `actor` between places:
/// what the structures on either end tell their guests, and the client's
/// park (a revived pilot is docked, so it holds none). Call it after the
/// session change is queued, off the caller's addressed-pushes turn.
pub(crate) fn clone_moved(reg: &mut Registry, actor: i64, from: Option<i64>, to: Option<i64>) {
    if from.is_none() && to.is_none() {
        return;
    }
    forget(reg, actor);
    // (The caller's change names the ship: no `shipid` of ours.)
    control::drop_control(reg, actor, false);
    if let Some(id) = from.filter(|id| is_structure(reg, *id)) {
        announce_guest(reg, actor, id, false);
        refresh_slim(reg, id);
    }
    if let Some(id) = to.filter(|id| is_structure(reg, *id)) {
        announce_guest(reg, actor, id, true);
        refresh_slim(reg, id);
    }
}

/// A structure as the seven fields that put a pilot in it.
pub(crate) fn station_ref(reg: &Registry, s: &Structure) -> crate::clones::StationRef {
    let cat = reg.celestials();
    let row = cat.system(s.system);
    crate::clones::StationRef {
        station_id: s.id,
        owner_id: s.owner_corp,
        type_id: s.type_id,
        operation_id: 0,
        solar_system_id: s.system,
        constellation_id: row.map_or(0, |r| r.constellation_id),
        region_id: row.map_or(0, |r| r.region_id),
    }
}

/// Where a structure is and how a ship leaves it: for `undock_motion`.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub position: [f64; 3],
    /// `dunRotation` in degrees, as the slim carries it.
    pub rotation: [f64; 3],
    pub radius: f64,
}

/// [`Geometry`] of the structure `id`, if it is one.
pub fn geometry(reg: &Registry, id: i64) -> Option<Geometry> {
    if !is_structure(reg, id) {
        return None;
    }
    let s = structure::get(reg.deltas(), id)?;
    Some(Geometry {
        position: s.pos,
        rotation: [s.yaw.to_degrees(), 0.0, 0.0],
        radius: crate::structure_space::radius(reg.content(), s.type_id),
    })
}

// -- the session changes ------------------------------------------------------------

fn pair(old: PyValue, new: PyValue) -> PyValue {
    PyValue::Tuple(vec![old, new])
}

/// `{structureid: (None, id)}`: the whole of a dock **[V]** (spec §2). No
/// `solarsystemid`, which stays set, and no `shipid`.
pub fn dock_change(id: i64) -> Vec<(PyValue, PyValue)> {
    entries(crate::pyutil::pypairs(vec![(
        "structureid",
        pair(PyValue::None, PyValue::Int(id)),
    )]))
}

/// `{structureid: (id, None)}`: the whole of an undock **[V]**.
pub fn undock_change(id: i64) -> Vec<(PyValue, PyValue)> {
    entries(crate::pyutil::pypairs(vec![(
        "structureid",
        pair(PyValue::Int(id), PyValue::None),
    )]))
}

fn entries(v: Vec<(PyValue, PyValue)>) -> Vec<(PyValue, PyValue)> {
    v
}

// -- the inventory alias --------------------------------------------------------------

/// Whether the inventory bound with these parameters is the hangar of the
/// structure `actor` is docked in: `GetInventory(10014)` (what
/// `GetInventory(containerHangar)` becomes when `session.structureid` is
/// set) or `GetInventoryFromId(structureid)` **[V]** (`invCache.py:489`).
/// The answer is the hangar's id, 10004, to read the request as.
pub fn hangar_alias(reg: &Registry, actor: Option<i64>, container: Option<i64>) -> Option<i64> {
    let id = container?;
    if id == CONTAINER_STRUCTURE {
        return Some(CONTAINER_HANGAR);
    }
    let station = reg.character(actor?)?.station_id?;
    (station == id && is_structure(reg, id)).then_some(CONTAINER_HANGAR)
}

// -- the hangar view state -------------------------------------------------------------

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// The `StructureHangarViewState` the hangar scene reads **[V]**
/// (`structureHangarViewState.py`; the consumer is
/// `modularHangarBehaviours.UpdateStructureState`): `upkeepState`,
/// `operatingState`, `damageState` `(shield, armor, hull)`, and the timer's
/// `timerProgress`, `timerIsProgressing`, `timerIsPaused`, `timerEndAt`. A
/// `KeyVal` with those attributes **[A]** (module header).
pub fn hangar_state(s: &Structure, now: i64) -> PyValue {
    let (sh, ar, hu) = s.damage.unwrap_or((1.0, 1.0, 1.0));
    let progress = match (s.timer.start, s.timer.end) {
        (Some(a), Some(b)) if b > a => {
            let at = s.timer.paused.unwrap_or(now);
            let p = (at - a) as f64 / (b - a) as f64;
            PyValue::Real(p.clamp(0.0, 1.0))
        }
        _ => PyValue::None,
    };
    crate::pyutil::keyval(vec![
        (
            "upkeepState",
            PyValue::Int(
                s.upkeep
                    .map_or(structure::Upkeep::Full.code(), |u| u.code()),
            ),
        ),
        ("operatingState", PyValue::Int(s.state.code())),
        (
            "damageState",
            PyValue::Tuple(vec![
                PyValue::Real(sh),
                PyValue::Real(ar),
                PyValue::Real(hu),
            ]),
        ),
        ("timerProgress", progress),
        (
            "timerIsProgressing",
            PyValue::Bool(s.timer.start.is_some() && s.timer.paused.is_none()),
        ),
        (
            "timerIsPaused",
            PyValue::Bool(s.timer.start.is_some() && s.timer.paused.is_some()),
        ),
        ("timerEndAt", opt(s.timer.end)),
    ])
}

/// The state for a caller who is not docked in a structure: an online hull
/// with nothing wrong, so the scene call cannot fail.
pub fn default_hangar_state() -> PyValue {
    crate::pyutil::keyval(vec![
        ("upkeepState", PyValue::Int(structure::Upkeep::Full.code())),
        (
            "operatingState",
            PyValue::Int(structure::State::ShieldVulnerable.code()),
        ),
        (
            "damageState",
            PyValue::Tuple(vec![
                PyValue::Real(1.0),
                PyValue::Real(1.0),
                PyValue::Real(1.0),
            ]),
        ),
        ("timerProgress", PyValue::None),
        ("timerIsProgressing", PyValue::Bool(false)),
        ("timerIsPaused", PyValue::Bool(false)),
        ("timerEndAt", PyValue::None),
    ])
}

/// `OnHangarViewStateUpdated(structureID, state)` to the pilots docked in it.
#[derive(Debug, Clone, PartialEq)]
pub struct HangarStateUpdated {
    pub to: Vec<i64>,
    pub id: i64,
    pub state: PyValue,
}

impl Push for HangarStateUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnHangarViewStateUpdated",
            vec![PyValue::Int(self.id), self.state.clone()],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Tell the pilots docked in `s` how it now looks from inside.
pub fn announce_hangar_state(reg: &mut Registry, s: &Structure) {
    let to: Vec<i64> = reg
        .characters_at_station(s.id)
        .iter()
        .map(|c| c.character_id)
        .collect();
    if to.is_empty() {
        return;
    }
    let state = hangar_state(s, crate::pyutil::filetime_now());
    reg.push(HangarStateUpdated {
        to,
        id: s.id,
        state,
    });
}

// -- the guests --------------------------------------------------------------------------

/// `OnCharacterEnteredStructure(char, corp, alliance, faction)` or
/// `OnCharacterLeftStructure(char)` **[V]**
/// (`structureGuests`, `dockedUI/guests.py`).
#[derive(Debug, Clone, PartialEq)]
pub struct GuestMoved {
    pub to: Vec<i64>,
    pub character: i64,
    pub corp: i64,
    pub alliance: Option<i64>,
    pub arrived: bool,
}

impl Push for GuestMoved {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        if self.arrived {
            (
                "OnCharacterEnteredStructure",
                vec![
                    PyValue::Int(self.character),
                    PyValue::Int(self.corp),
                    opt(self.alliance),
                    PyValue::None,
                ],
            )
        } else {
            (
                "OnCharacterLeftStructure",
                vec![PyValue::Int(self.character)],
            )
        }
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// The frame telling the others docked in `structure_id` that `actor` came
/// or went, built but not queued (the departure path puts it on the clock
/// queue, as `Registry::station_guest_frame` does). The guest list is asked
/// once (`GetGuests`), so this is what keeps it right. `None` when nobody
/// else is there.
pub fn guest_frame(
    reg: &Registry,
    actor: i64,
    structure_id: i64,
    arrived: bool,
) -> Option<GuestMoved> {
    let me = reg.character(actor)?;
    let to: Vec<i64> = reg
        .characters_at_station(structure_id)
        .iter()
        .map(|c| c.character_id)
        .filter(|id| *id != actor)
        .collect();
    if to.is_empty() {
        return None;
    }
    Some(GuestMoved {
        to,
        character: actor,
        corp: me.corporation_id,
        alliance: me.alliance_id,
        arrived,
    })
}

fn announce_guest(reg: &mut Registry, actor: i64, structure_id: i64, arrived: bool) {
    if let Some(n) = guest_frame(reg, actor, structure_id, arrived) {
        reg.push(n);
    }
}

/// The login form of [`announce_guest`]: the pilot is docked in a structure
/// now. `Registry::announce_station_guest` calls it, and is silent for a
/// structure.
pub fn announce_login(reg: &mut Registry, actor: i64) {
    // A new login is a new client: it holds no park and has asked for nothing.
    forget(reg, actor);
    if let Some(id) = docked_in(reg, actor) {
        announce_guest(reg, actor, id, true);
    }
}

/// The log-off form: the frame for the clock queue, while the pilot is still
/// docked.
pub fn logoff_frame(reg: &Registry, actor: i64) -> Option<GuestMoved> {
    let id = docked_in(reg, actor)?;
    guest_frame(reg, actor, id, false)
}

/// The slim says how many are inside (`docked`): tell everyone who has the
/// ball.
fn refresh_slim(reg: &mut Registry, id: i64) {
    if let Some(s) = structure::get(reg.deltas(), id) {
        crate::structure_space::announce_slim(reg, &s);
    }
}

// -- Dock ----------------------------------------------------------------------------------

/// `DockingRequestDenied` with its `reason` argument (FSD 808, a text), so
/// the client's message has something to fill in.
fn denied(reason: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        crate::dock_policy::DENIED,
        crate::pyutil::pypairs(vec![("reason", PyValue::Unicode(reason.to_owned()))]),
    ))
}

fn not_dockable() -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        crate::service::space::NOT_DOCKABLE,
        Vec::new(),
    ))
}

fn notify(msg: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(msg.to_owned()))]),
    ))
}

/// `structureDocking.Dock(structureID, shipID)`: every gate, then the dock.
///
/// The client has already flown to within 2,500 m (`NavigateSystemTo`) and
/// shows the button only for an online structure it may dock at, so these
/// are the server repeating them for a client that was not honest:
/// the criminal and weapons-timer gates `CmdDock` has, the ship the client
/// named, the system, the state, the profile (setting 19, the error
/// `StructureDockingDenied` **[V name]**), the range, and the hull
/// ([`crate::docking::can_dock`], only for a type with locator rows).
pub fn dock(
    reg: &mut Registry,
    actor: i64,
    structure_id: i64,
    ship_id: Option<i64>,
) -> Result<(), ServiceError> {
    let now = crate::pyutil::filetime_now();
    crate::concord::refuse_dock(reg, Some(actor), now)?;
    crate::crimewatch::refuse_dock(reg, Some(actor), now)?;
    crate::entosis::refuse_dock(reg, Some(actor))?;
    crate::cyno::refuse_dock(reg, Some(actor))?;
    crate::module_fuel::refuse_dock(reg, Some(actor))?;
    crate::mjd::refuse_dock(reg, Some(actor))?;
    crate::cloak::refuse_dock(reg, Some(actor))?;
    let Some(s) = structure::get(reg.deltas(), structure_id) else {
        return Err(not_dockable());
    };
    let Some(c) = reg.character(actor) else {
        return Err(not_dockable());
    };
    if ship_id.is_some_and(|ship| ship != c.ship_id) {
        return Err(denied("Your active ship changed. Try again."));
    }
    let (ship, ship_type) = (c.ship_id, c.ship_type_id);
    // In space, in its system. A pilot already docked has no business here.
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(not_dockable());
    };
    if here.solar_system_id != s.system {
        return Err(not_dockable());
    }
    if !s.can_dock() {
        return Err(denied("This structure is not accepting ships."));
    }
    if !sp::has_access(reg, &s, actor, sp::SETTING_HOUSING_CAN_DOCK) {
        return Err(ServiceError::Raise(eve_proto::user_error(
            sp::access_error(sp::SETTING_HOUSING_CAN_DOCK),
            Vec::new(),
        )));
    }
    refuse_out_of_range(reg, ship, &s)?;
    // The hull: only where the type's locator rows say which hulls it takes.
    let table = reg.content().get::<crate::docking::Table>();
    if crate::docking::undock_directions(table, s.type_id).is_some()
        && !crate::docking::can_dock(table, reg.content(), s.type_id, ship_type)
    {
        return Err(crate::dock_policy::too_large());
    }
    seat_in(reg, actor, &s);
    Ok(())
}

/// The client's 2,500 m, repeated with the NPC rule's allowance.
fn refuse_out_of_range(reg: &Registry, ship: i64, s: &Structure) -> Result<(), ServiceError> {
    let Some(me) = reg.ball_target(ship) else {
        return Ok(());
    };
    let d = crate::space_state::distance(me.position, s.pos)
        - me.radius
        - crate::structure_space::radius(reg.content(), s.type_id);
    if d > crate::dock_policy::DOCK_RANGE + crate::dock_policy::DOCK_RANGE_SLACK {
        crate::outln!(
            "[dock] out of range: ship {ship} is {d:.0} m from structure {}",
            s.id
        );
        return Err(not_dockable());
    }
    Ok(())
}

/// Put the pilot and their ship into the structure: the same steps
/// `Registry::dock_change` takes at a station, and the session change.
fn seat_in(reg: &mut Registry, actor: i64, s: &Structure) {
    let sref = station_ref(reg, s);
    if !reg.dock_at(actor, sref) {
        return;
    }
    // The observers lose the ship, and the pilot's own park is over.
    crate::service::space::undocked_no_longer(reg, Some(actor));
    crate::targeting::left_space(
        reg,
        actor,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::WithAnswer,
    );
    crate::repair::on_dock(reg, actor);
    reg.roster_mut().leave(actor);
    forget(reg, actor);
    reg.queue_session_change(actor, dock_change(s.id));
    announce_guest(reg, actor, s.id, true);
    refresh_slim(reg, s.id);
}

/// A GM teleport took a pilot out of the structure `id` (`relocate`): the
/// client's session change is the caller's; this tells the rest.
pub(crate) fn left_by_teleport(reg: &mut Registry, actor: i64, id: i64) {
    forget(reg, actor);
    control::drop_control(reg, actor, true);
    announce_guest(reg, actor, id, false);
    refresh_slim(reg, id);
}

// -- Undock ------------------------------------------------------------------------------------

/// `structureDocking.Undock(structureID, shipID, ignoreContraband)`. The
/// session change is `session_change_for`'s (it goes in front of the
/// answer); this is the rest: the checks, and where the ship appears.
pub fn undock(
    reg: &mut Registry,
    actor: i64,
    structure_id: i64,
    ship_id: Option<i64>,
) -> Result<(), ServiceError> {
    if docked_in(reg, actor) != Some(structure_id) {
        return Err(not_dockable());
    }
    let Some(c) = reg.character(actor) else {
        return Err(not_dockable());
    };
    // A pilot at the controls leaves them first: his session's ship is the
    // structure, and the client hides the button (`station/base.py`).
    if control::controlling(reg, actor).is_some() {
        return Err(notify(
            "Release control of the structure before you undock.",
        ));
    }
    if ship_id.is_some_and(|ship| ship != c.ship_id) {
        return Err(notify("Your active ship changed. Try again."));
    }
    let ship = c.ship_id;
    if !crate::subsystems::may_undock(reg.store(), ship) {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "TooFewSubSystemsToUndock",
            Vec::new(),
        )));
    }
    release(reg, actor, false);
    Ok(())
}

/// Whether this pilot's client holds a ballpark while docked (the `structure`
/// view), so nothing will bind when the ship leaves.
pub(crate) fn has_live_park(reg: &mut Registry, actor: i64) -> bool {
    reg.ext_mut::<Docked>().live.contains(&actor)
}

/// A pilot put out of the structure by a GM move inside its system: the ship
/// is seated into the park the client holds, as an undock does.
pub(crate) fn undock_in_place(reg: &mut Registry, actor: i64) {
    control::drop_control(reg, actor, true);
    release(reg, actor, true);
}

/// The undock proper, without the checks: the ship leaves the hangar the way
/// [`Registry::undock_change`] leaves a station. `evict` is an unanchoring
/// putting the pilot out: the ship is seated at once whether or not the
/// client holds a park, because the structure will be gone by the bind.
fn release(reg: &mut Registry, actor: i64, evict: bool) {
    crate::trade::on_leave(reg, Some(actor));
    crate::repair::on_undock(reg, actor);
    let Some(id) = reg.character(actor).and_then(|c| c.station_id) else {
        return;
    };
    let live = reg.ext_mut::<Docked>().live.remove(&actor) || evict;
    if live {
        // The park is up and will not be rebuilt: the ship goes into it now.
        crate::destiny::seat_into_live_park(reg, actor);
        reg.ext_mut::<Docked>().seated.insert(actor);
    } else {
        reg.ext_mut::<Docked>().undocking.insert(actor);
    }
    // The pilot is in space now (live) or about to be (the bind), so the
    // guests hear it either way.
    if let Some(n) = guest_frame(reg, actor, id, false) {
        reg.push(n);
    }
    refresh_slim(reg, id);
}

// -- the bind ------------------------------------------------------------------------------------

/// The scene a pilot docked in a structure is sent when they bind: every
/// ball the system has, with **the structure as the ego**. `None` for a
/// pilot who is not docked in one.
pub fn docked_state(reg: &mut Registry, actor: i64) -> Option<crate::destiny::Bootstrap> {
    let id = docked_in(reg, actor)?;
    let s = structure::get(reg.deltas(), id)?;
    reg.advance_system(s.system);
    let stamp = reg.next_destiny_stamp();
    let mut state = crate::destiny::scene(reg, s.system, s.id, None, stamp);
    let keep = crate::grid::grid_of(reg, s.system, 0, s.pos);
    state.retain_balls(|b| keep.contains(b));
    Some(state)
}

/// `on_ballpark_bind`'s first question: is this bind one of a pilot whose
/// station is a structure? `true` means it was handled here and the
/// ordinary arrival must not run.
pub fn on_bind(w: &mut Registry, actor: i64) -> bool {
    // A bind after an undock (or an eviction) seated the ship into
    // a park the client kept is the client having made a new one: it gets
    // the scene again, and the ship stays where it is. Whatever the
    // character's station is by now.
    if w.ext_mut::<Docked>().seated.remove(&actor) && w.roster().is_in_space(actor) {
        if let Some(state) = crate::destiny::bootstrap_for(w, actor)
            && state.every_slim_has_a_ball()
            && let Some(update) = crate::destiny::mid_session_state(w, actor, state)
        {
            w.push(update);
        }
        return true;
    }
    let Some(station) = w.character(actor).and_then(|c| c.station_id) else {
        return false;
    };
    if !is_structure(w, station) || w.docked_station(actor).is_none() {
        return false;
    }
    // Docked. An armed undock is the ordinary arrival (`false`); anything
    // else is a docked bind.
    if w.ext_mut::<Docked>().undocking.remove(&actor) {
        return false;
    }
    if let Some(state) = docked_state(w, actor)
        && state.every_slim_has_a_ball()
    {
        w.ext_mut::<Docked>().live.insert(actor);
        w.push(crate::destiny::StateUpdate {
            to: vec![actor],
            state,
        });
    }
    true
}

// -- the end of a structure ------------------------------------------------------------------------

/// Where what `owner` left in a structure goes: the NPC station of its
/// system, else the owner's clone home when that is not a structure (a
/// corporation's is its CEO's), else the nearest NPC station by
/// constellation, then region, then the lowest id. `None` only when the
/// server knows no NPC station at all.
pub(crate) fn harbour(
    reg: &Registry,
    s: &Structure,
    owner: i64,
) -> Option<crate::clones::StationRef> {
    if let Some(id) = crate::structure_space::station_in(reg, s.system)
        && let Some(r) = reg.station_ref(id)
    {
        return Some(r);
    }
    // A corporation owns no clone: its CEO's home stands in.
    let person = reg.corporations().get(owner).map_or(owner, |c| c.ceo_id);
    if let Some((r, _)) = crate::clones::home_of(reg, person)
        && reg.station_ref(r.station_id).is_some()
    {
        return Some(r);
    }
    let here = reg.celestials().system(s.system);
    let (constellation, region) = here.map_or((0, 0), |r| (r.constellation_id, r.region_id));
    reg.npc_station_refs().into_iter().min_by_key(|r| {
        let rank = if r.solar_system_id == s.system {
            0
        } else if constellation != 0 && r.constellation_id == constellation {
            1
        } else if region != 0 && r.region_id == region {
            2
        } else {
            3
        };
        (rank, r.station_id)
    })
}

/// A structure is leaving space: undock the pilots in it, and move what is
/// stored there. Call it **before** the row goes (the ejection needs its
/// place).
pub fn evacuate(reg: &mut Registry, s: &Structure) {
    for c in reg
        .characters_at_station(s.id)
        .iter()
        .map(|c| c.character_id)
        .collect::<Vec<_>>()
    {
        // One change: the seat's `shipid` back to the hull, then the undock.
        let mut change = control::drop_control_change(reg, c).unwrap_or_default();
        change.extend(undock_change(s.id));
        release(reg, c, true);
        reg.queue_session_change(c, change);
    }
    // Every item that lives there, by owner. A ship in space that last left
    // this hangar is an item here too.
    let stored: Vec<(i64, i64, i64)> = reg
        .store()
        .contents_of(s.id)
        .map(|i| (i.item_id, i.owner_id, i.flag_id))
        .collect();
    for (item_id, owner, flag) in stored {
        let Some(to) = harbour(reg, s, owner) else {
            crate::errln!(
                "[structure] item {item_id} of {owner} has nowhere to go from {}",
                s.id
            );
            continue;
        };
        let slot = Slot::new(owner, to.station_id, flag);
        if let Err(e) = reg.apply_items(&[Edit::Move { item_id, to: slot }]) {
            crate::errln!("[structure] moving {item_id} out of {}: {e}", s.id);
        }
    }
    // Nobody's home station is a structure that is gone.
    crate::clones::forget_homes_at(reg, s.id);
    // Everyone based here is based at the harbour instead, so the next login
    // does not name a structure that is gone.
    let based: Vec<i64> = reg
        .config()
        .characters
        .iter()
        .filter(|c| c.station_id == Some(s.id))
        .map(|c| c.character_id)
        .collect();
    for who in based {
        let Some(to) = harbour(reg, s, who) else {
            continue;
        };
        if let Some(c) = reg.character_mut(who) {
            to.seat(c);
        }
        crate::clones::rehome_capsule(reg, who, s.id, to);
        // (The `seated` mark stays: the bind that follows needs it.)
        let d = reg.ext_mut::<Docked>();
        d.live.remove(&who);
        d.undocking.remove(&who);
    }
    reg.mark_dirty();
}

#[cfg(test)]
mod tests;
