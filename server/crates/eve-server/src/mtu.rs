//! **Things parked in space: containers launched whole, the Mobile Depot and
//! the Mobile Tractor Unit** (issue #82, lane `mtu-depot`; the second half of
//! #57, after `crate::containers`).
//!
//! A *unit* here is a store item that sits in a solar system at flag 0 with a
//! [`combat::Wreck`] record, which is what puts a ball on the grid, lists its
//! contents to a pilot beside it (`service::inventory`'s `wreck_open_to`),
//! lets "Loot All" and a drag take from it (`combat::plan_loot`) and removes
//! it with its contents when its clock runs out. A jetcan is one; so is a
//! container the pilot launched whole, and so are the two deployables.
//!
//! # What the client does
//!
//! - **Launch a container whole.** `menusvc` offers "Launch for self" for a
//!   player-deployed container (groups 12, 340, 448, 649) in the active ship
//!   that is not anchorable (`itemCheckers.OfferLaunchForSelf_Container`) [V]
//!   and the entry runs `menusvc.Jettison`, so it is `ship.Jettison(itemIDs)`
//!   again. `invItemFunctions.Jettison` unpacks `(jettisonedToCanIDs,
//!   jettisonItemList)`; **the second list is the ids that launched as
//!   themselves** (`jettison.rs` module note). That is where a container goes.
//! - **Launch a deployable.** "Launch for Self" on a Mobile Depot or MTU in the
//!   hold is `eveMisc.LaunchFromShip(items, charid, maxQty=1)`, which sends
//!   `ship.Drop([(itemID, 1)], whoseBehalfID, ignoreWarning)` and reads
//!   `{itemID: [newItemID or (label, args)]}` (`spacecomponents/client/
//!   components/deploy.py: DeployAction`, `eveMisc.py:13-59`) [V]. The same
//!   call a Sovereignty Hub is launched with (`service::sov_hub`).
//! - **Open one.** A container, a depot or an MTU in space opens as
//!   `invCtrl.ItemFloatingCargo`, whose window lists
//!   `GetInventoryFromId(itemID).List()` (`menusvc._OpenCargo`) [V]. Both
//!   deployables are in `invControllers.LOOT_GROUPS` (`groupMobileHomes` 1246,
//!   `groupAutoLooter` 1250), so the window offers "Loot All". The cargo-bay
//!   component's own check is `IsAccessibleByCharacter`: the owner, or anyone
//!   if `allowFreeForAll` (`spacecomponents/client/components/cargobay.py`) [V].
//! - **Scoop.** `ship.Scoop(objectID[, password])`; a wrong or missing password
//!   for a secure container is `ShpScoopSecureCC` (`menusvc.RealScoop`) [V]. A
//!   deployable with the scoop component is scoopable by its owner
//!   (`celestialCheckers.IsScoopable`) [V]. `ship.ScoopToMobileDepotHold(id)`
//!   is the same into flag 183 (`menusvc.ScoopToMobileDepotHold`) [V].
//!
//! Lines in the pinned build's `.pyc`, by `tools/dis.py`: `itemCheckers.
//! OfferLaunchForSelf_Container` 561, `menusvc._OpenCargo` 3598,
//! `menusvc.ScoopToMobileDepotHold` 3794, `eveMisc.LaunchFromShip` 13,
//! `deploy.DeployAction` 214, `cargobay.IsAccessibleByCharacter` 97,
//! `invControllers.IsLootable` 1618.
//!
//! # The numbers
//!
//! No figure of [`spec`] is in the source. Each is the client's own
//! `spacecomponentsbytype` row of the **base** type of the group, build
//! 3662168, read by the `deploy` section of `tools/content.py`
//! (`tools/content_deploy.py`) into the bundle's `deploy/units.tsv`, which
//! this module loads as [`TABLE`] **[V-data]**: the cargo bay (access range,
//! who may add), the activation and decay times, the deploy distance and the
//! gap to the group's own units, and for the MTU its `autoTractorBeam` (range
//! and cycle) and `autoLooter`. A bundle without the file serves no
//! deployable. The other types of a group may differ in range; this server
//! serves the base row for the whole group **[A]**, falsified by an MTU type
//! whose beam range the client shows differently. The tractor's velocity is
//! the type's dogma attribute 1045 ([`ATTR_MAX_TRACTOR_VELOCITY`]), with
//! [`DEFAULT_TRACTOR_VELOCITY`] **[A]** when the table has none.
//!
//! # What the server does
//!
//! - **Launch** ([`drop_call`], [`park`]): in space, not in warp, from the
//!   active ship's cargo, mobile depot hold or fleet hangar, one unit per
//!   stack per call, [`Spec::deploy_at`] metres from the ship, and not within
//!   [`Spec::own_group_gap`] of another unit of the group. The stack's unit
//!   becomes a singleton (`Edit::Assemble`). **Not enforced [A]:** the gap to
//!   stargates and stations (50 km), control towers, moon beacons and
//!   the ESS: this server has no position index for them here; falsified by a
//!   depot launched beside a gate. A container launched whole keeps its id and
//!   everything in it.
//! - **Rows.** A unit is persisted as delta kind [`KIND`] (`unit -> system x
//!   y z active_at decay_at`, FILETIMEs), so a restart brings it back where it
//!   was ([`restore`], called by the combat sweep before it destroys the
//!   records-less cans and wrecks) and its contents with it. A unit whose time
//!   ran out while the server was off is simply swept with the wrecks.
//! - **Time.** A deployable is **active** from its launch plus
//!   [`Spec::activate`]; before that its cargo is shut ("still activating") and
//!   an MTU does nothing. It is removed with its contents at launch plus
//!   [`Spec::decay`] (the cargo-bay `dropNonDestroyedItems` / `lootDropPercentage`
//!   rule, 50 percent of the cargo dropping as loot, is **not** modelled **[A]**:
//!   everything goes). A container launched whole lasts
//!   [`CONTAINER_LIFETIME`] **[A]** (the jetcan's two hours): retail's lifetime
//!   for a launched container is not in the client; falsified by a retail
//!   container found after two hours.
//! - **Access.** A deployable is the owner's alone ([`gate_owner`]): listing,
//!   adding (a depot only) and taking. A container in space asks for its
//!   password as in a station (`containers::gate`), and takes items from the
//!   active ship's plain holds within loot range, or gives them to the loot
//!   path. Corporation access to a deployable is not served **[A]**.
//! - **The MTU's cycle** ([`tick`]): every [`Tractor::cycle`] an active MTU
//!   (1) loots every wreck or jetcan in its access range **that its owner has
//!   loot rights to** (`combat::has_loot_right`: nothing it takes makes a
//!   suspect) into its own cargo, whole stacks first and then as much of the
//!   next as fits, and (2) pulls the nearest such wreck beyond that range and
//!   inside the tractor's range one step (velocity times the cycle) towards
//!   itself, stopping at [`Tractor::min_range`]. One beam **[A]**. The pull is
//!   the salvager's: `SetBallPosition` to everyone in the system.
//! - **Scoop by the owner** of a deployable, if its cargo is empty **[A]**
//!   (retail's rule for a loaded one is not in the client).
//!
//! Not done: the depot's fitting range (`fitting` component, 3,000 m: refitting
//! a ship from it), anchoring, destruction by weapons, a corporation's units,
//! the cargo-bay loot drop, the activation timer on the slim item.

use crate::combat::{self, LOOT_RANGE, LOOT_RANGE_SLACK, Wreck};
use crate::damage::BlueTime;
use crate::fitting::{FLAG_CARGO, Refusal};
use crate::item::{Change, Edit, Item, Slot};
use crate::model::Character;
use crate::notify::Addressed;
use crate::service::{Registry, Request, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`): `unit -> system x
/// y z active_at decay_at`.
pub const KIND: &str = "spunit";

/// `groupMobileHomes` [V] (`inventorycommon/const.py`): the Mobile Depot.
pub const GROUP_MOBILE_DEPOT: i64 = 1246;
/// `groupAutoLooter` [V]: the Mobile Tractor Unit.
pub const GROUP_MTU: i64 = 1250;
/// `flagMobileDepotHold` [V].
pub const FLAG_MOBILE_DEPOT_HOLD: i64 = 183;
/// The flags a deployable may be launched from: cargo, the mobile depot hold
/// and the fleet hangar (`itemCheckers.OfferLaunchForSelf` asks for the cargo
/// hold, the infrastructure hangar or the fleet hangar) [V].
pub(crate) const LAUNCH_FLAGS: [i64; 3] = [
    FLAG_CARGO,
    FLAG_MOBILE_DEPOT_HOLD,
    crate::ship_config::FLAG_FLEET_HANGAR,
];

/// `attributeMaxTractorVelocity` [V].
pub const ATTR_MAX_TRACTOR_VELOCITY: i64 = 1045;
/// Metres per second when the type table has no tractor velocity **[A]**;
/// falsified by an MTU type whose attribute 1045 is read as zero.
pub const DEFAULT_TRACTOR_VELOCITY: f64 = 1_000.0;

/// How long a container launched whole stays, on the blue clock **[A]**.
pub const CONTAINER_LIFETIME: BlueTime = combat::WRECK_LIFETIME;

/// A blue second, in ticks of 100 ns.
const SEC: BlueTime = 10_000_000;

/// The containers' flag inside, as `containers` has it.
const FLAG_IN_CONTAINER: i64 = crate::containers::FLAG_IN_CONTAINER;

/// The tractor and looter of an MTU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tractor {
    /// Farthest wreck it reaches, metres.
    pub max_range: f64,
    /// It stops pulling at this distance, metres.
    pub min_range: f64,
    /// One cycle, blue ticks.
    pub cycle: BlueTime,
}

/// What the client's space components say of a deployable group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub group_id: i64,
    /// `activate.durationSeconds`, blue ticks.
    pub activate: BlueTime,
    /// `decay.durationSeconds`, blue ticks.
    pub decay: BlueTime,
    /// `cargoBay.accessRange`, metres.
    pub access_range: f64,
    /// `cargoBay.allowUserAdd`.
    pub allow_user_add: bool,
    /// `deploy.deployAtRange`, metres from the ship.
    pub deploy_at: f64,
    /// `deploy.minDistanceFromOwnGroup`, metres.
    pub own_group_gap: f64,
    /// `autoTractorBeam` and `autoLooter`; the depot has neither.
    pub tractor: Option<Tractor>,
}

/// The bundle's deployables file and its header (`tools/content_deploy.py`).
pub const UNITS_FILE: &str = "deploy/units.tsv";
pub const UNITS_HEADER: &str = "#group_id\tactivate_s\tdecay_s\taccess_range\tallow_user_add\tdeploy_at\town_group_gap\ttractor_max\ttractor_min\tcycle_s\tinhibit_range\tmin_gate_dist\tmax_security";
/// The header with `inhibit_range` but not the placement columns (NW-4's
/// first form), and the one before it; bundles with either still load.
pub const UNITS_HEADER_V2: &str = "#group_id\tactivate_s\tdecay_s\taccess_range\tallow_user_add\tdeploy_at\town_group_gap\ttractor_max\ttractor_min\tcycle_s\tinhibit_range";
pub const UNITS_HEADER_V1: &str = "#group_id\tactivate_s\tdecay_s\taccess_range\tallow_user_add\tdeploy_at\town_group_gap\ttractor_max\ttractor_min\tcycle_s";

/// The Mobile Cynosural Inhibitor's row (NW-4, `cynoInhibitor` component):
/// its group, how long it takes to activate, how long it lives after that,
/// and the range inside which it refuses a cyno. Not a [`Spec`]: it has no
/// cargo bay, and [`spec`] must stay `None` for it so the depot and tractor
/// code never treats it as a container.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inhibitor {
    pub group_id: i64,
    /// `activate.durationSeconds`, blue ticks.
    pub activate: BlueTime,
    /// `decay.durationSeconds`, blue ticks (0 when the row has none).
    pub decay: BlueTime,
    /// `cynoInhibitor.range`, metres.
    pub range: f64,
    /// `deploy.minDistanceFromOwnGroup`, metres: the spacing between two
    /// inhibitors (0 when the row has none).
    pub spacing: f64,
    /// `deploy.minDistanceFromStargatesAndStations`, metres: the distance a
    /// gate, station or Upwell structure must be (0 when none).
    pub from_gate: f64,
    /// `deploy.maxSecurityLevel`: a system more secure than this refuses a
    /// launch (`None` when the row has none).
    pub max_security: Option<f64>,
}

/// The [`Spec`] of every deployable group, from the bundle (module note).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Specs(Vec<Spec>, Vec<Inhibitor>);

impl Specs {
    /// Build from the bundle's data lines. A short or unreadable line is an
    /// error naming it; a group twice is too.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Specs, String> {
        let mut out: Vec<Spec> = Vec::new();
        let mut inhibitors: Vec<Inhibitor> = Vec::new();
        for (n, l) in lines.iter().enumerate() {
            let bad = || format!("{UNITS_FILE} data line {}: {:?}", n + 1, l.as_ref());
            let f: Vec<&str> = l.as_ref().split('\t').collect();
            if !matches!(f.len(), 10 | 11 | 13) {
                return Err(bad());
            }
            let num = |i: usize| {
                f[i].parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
            };
            let secs = |i: usize| num(i).map(|s| (s * SEC as f64) as BlueTime);
            if f.get(10).is_some_and(|r| !r.is_empty()) {
                let group_id: i64 = f[0].parse().map_err(|_| bad())?;
                if inhibitors.iter().any(|i| i.group_id == group_id) {
                    return Err(format!("{UNITS_FILE}: group {group_id} twice"));
                }
                inhibitors.push(Inhibitor {
                    group_id,
                    activate: secs(1).ok_or_else(bad)?,
                    decay: secs(2).ok_or_else(bad)?,
                    range: num(10).filter(|r| *r > 0.0).ok_or_else(bad)?,
                    spacing: num(6).ok_or_else(bad)?,
                    from_gate: match f.get(11) {
                        Some(v) if !v.is_empty() => num(11).ok_or_else(bad)?,
                        _ => 0.0,
                    },
                    max_security: match f.get(12) {
                        Some(v) if !v.is_empty() => Some(
                            v.parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite())
                                .ok_or_else(bad)?,
                        ),
                        _ => None,
                    },
                });
                continue;
            }
            let tractor = if f[7].is_empty() {
                None
            } else {
                Some(Tractor {
                    max_range: num(7).ok_or_else(bad)?,
                    min_range: num(8).ok_or_else(bad)?,
                    cycle: secs(9).ok_or_else(bad)?,
                })
            };
            let spec = Spec {
                group_id: f[0].parse().map_err(|_| bad())?,
                activate: secs(1).ok_or_else(bad)?,
                decay: secs(2).ok_or_else(bad)?,
                access_range: num(3).ok_or_else(bad)?,
                allow_user_add: f[4] == "1",
                deploy_at: num(5).ok_or_else(bad)?,
                own_group_gap: num(6).ok_or_else(bad)?,
                tractor,
            };
            if out.iter().any(|s| s.group_id == spec.group_id) {
                return Err(format!("{UNITS_FILE}: group {} twice", spec.group_id));
            }
            out.push(spec);
        }
        Ok(Specs(out, inhibitors))
    }
}

/// This table's registration row: one line in `content_tables::TABLES`.
pub const TABLE: crate::content::TableSpec = crate::content::TableSpec {
    name: "deployables",
    load: boxed_load,
    fixture: boxed_fixture,
};

fn boxed_load(src: &crate::content::Sources) -> Result<crate::content::Boxed, String> {
    let Some(b) = src.bundle else {
        return Ok(Box::new(Specs::default()));
    };
    let lines = match b.data_lines(UNITS_FILE, UNITS_HEADER) {
        Ok(l) => l,
        Err(first) => b
            .data_lines(UNITS_FILE, UNITS_HEADER_V2)
            .or_else(|_| b.data_lines(UNITS_FILE, UNITS_HEADER_V1))
            .map_err(|_| first)?,
    };
    if lines.is_empty() {
        src.note(crate::content::Origin::Fixture);
    }
    Ok(Box::new(Specs::parse(&lines)?))
}

/// Invented rows for tests: round numbers, not the client's.
pub mod fixture {
    pub fn units() -> Vec<String> {
        [
            // A depot: 30 s to activate, 1,000 s to decay, 2 km access.
            "1246\t30\t1000\t2000\t1\t200\t4000\t\t\t",
            // An MTU: 6 s, 500 s, 2 km access, no user add, a beam of
            // 100 km to 1 km with a 4 s cycle.
            "1250\t6\t500\t2000\t0\t900\t3000\t100000\t1000\t4",
            // A cyno inhibitor (invented): 7 s to activate, 900 s to live, a
            // 25 km range, 40 km from its own kind, 15 km from a gate, a
            // system no more secure than 0.3.
            "1249\t7\t900\t0\t0\t0\t40000\t\t\t\t25000\t15000\t0.3",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
    }
}

fn boxed_fixture() -> crate::content::Boxed {
    Box::new(Specs::parse(&fixture::units()).expect("the fixture rows are well formed"))
}

/// The [`Spec`] of a deployable group in `content`, **[V-data]** from the
/// base type's `spacecomponentsbytype` row (module note). `None` for a group
/// that is not one, and for a bundle without `deploy/units.tsv`.
pub fn spec(content: &crate::content::Content, group_id: i64) -> Option<Spec> {
    content
        .try_get::<Specs>()?
        .0
        .iter()
        .find(|s| s.group_id == group_id)
        .copied()
}

/// The [`Inhibitor`] row of a group in `content` (NW-4); `None` for any
/// other group and for a bundle whose `deploy/units.tsv` lacks it.
pub fn inhibitor(content: &crate::content::Content, group_id: i64) -> Option<Inhibitor> {
    content
        .try_get::<Specs>()?
        .1
        .iter()
        .find(|i| i.group_id == group_id)
        .copied()
}

/// A launched deployable: a singleton of a group with a [`Spec`].
pub fn is_unit(content: &crate::content::Content, i: &Item) -> bool {
    i.singleton && spec(content, i.group_id).is_some()
}

/// A unit or a container with a record in space this run.
pub fn in_space(reg: &Registry, id: i64) -> bool {
    reg.combat().wreck(id).is_some()
}

// ---------------------------------------------------------------------------
// The slot
// ---------------------------------------------------------------------------

/// When each MTU's next cycle is due, and when the rows were last checked.
#[derive(Debug, Default)]
struct Units {
    next_cycle: BTreeMap<i64, BlueTime>,
    next_prune: BlueTime,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec { name: "mtu", make };

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Units::default())
}

// ---------------------------------------------------------------------------
// Errors and small readers
// ---------------------------------------------------------------------------

pub(crate) fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

fn refused(r: Refusal) -> ServiceError {
    r.to_error()
}

pub(crate) fn plan_refused(e: crate::item::EditError) -> ServiceError {
    crate::outln!("[mtu] plan refused: {e}");
    notify("The server could not move that item.")
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

fn ids_of(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|i| match i {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn int_kwarg(req: &Request<'_>, name: &str) -> Option<i64> {
    int(crate::pyutil::pairs_get(req.kwargs, name.as_bytes()))
}

/// Run `f` and send whatever the store pushed on the clock's behalf, the way
/// the salvager does from a tick.
fn on_clock<R>(w: &mut Registry, f: impl FnOnce(&mut Registry) -> R) -> R {
    let before = w.take_pending();
    let out = f(w);
    let ours = w.take_pending();
    for (after, p) in before {
        w.push_later(after, Addressed { to: None, inner: p });
    }
    for (_, p) in ours {
        w.schedule(Duration::ZERO, Addressed { to: None, inner: p });
    }
    out
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One row, read back.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    system: i64,
    at: [f64; 3],
    active_at: BlueTime,
    decay_at: BlueTime,
}

fn key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

fn remember(reg: &mut Registry, id: i64, row: Row) {
    reg.deltas_mut().put(
        KIND,
        key(id),
        vec![
            row.system.to_string(),
            format!("{:?}", row.at[0]),
            format!("{:?}", row.at[1]),
            format!("{:?}", row.at[2]),
            row.active_at.to_string(),
            row.decay_at.to_string(),
        ],
    );
}

fn parse_row(v: &[String]) -> Option<Row> {
    let f = |i: usize| v.get(i)?.parse::<f64>().ok();
    let n = |i: usize| v.get(i)?.parse::<i64>().ok();
    Some(Row {
        system: n(0)?,
        at: [f(1)?, f(2)?, f(3)?],
        active_at: n(4)?,
        decay_at: n(5)?,
    })
}

fn row_of(reg: &Registry, id: i64) -> Option<Row> {
    reg.deltas()
        .of(KIND)
        .get(&key(id))
        .and_then(|v| parse_row(v))
}

/// Forget a unit's row (it was scooped or is gone).
pub fn forget(reg: &mut Registry, id: i64) {
    if row_of(reg, id).is_some() {
        reg.deltas_mut().del(KIND, &key(id));
    }
}

/// Is the deployable open for business? Before its activation it is not.
pub fn is_active(reg: &Registry, id: i64, now: BlueTime) -> bool {
    row_of(reg, id).is_some_and(|r| now >= r.active_at)
}

// ---------------------------------------------------------------------------
// Parking an item in space
// ---------------------------------------------------------------------------

/// Where and by whom an item is put into space.
#[derive(Debug, Clone, Copy)]
pub struct Parked {
    pub owner: i64,
    pub corp: i64,
    /// The launching ship, `launcherID`.
    pub ship_id: i64,
    pub system: i64,
    pub at: [f64; 3],
}

/// Put `item_id` into space at `p` with a record that lives `lifetime`, and
/// the row that brings it back after a restart. An assembled item (a
/// container) keeps its id; a packaged one (a deployable) is assembled, one
/// unit off the stack with a new id. The id it has now. `activate` is the
/// time before it is active.
pub fn park(
    reg: &mut Registry,
    item_id: i64,
    p: &Parked,
    lifetime: BlueTime,
    activate: BlueTime,
) -> Result<i64, ServiceError> {
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .ok_or_else(|| refused(Refusal::NotAvailable))?;
    let to = Slot::new(p.owner, p.system, combat::FLAG_IN_SPACE);
    let edit = if item.singleton {
        Edit::Move { item_id, to }
    } else {
        Edit::Assemble { item_id, to }
    };
    let changes = reg.apply_items(&[edit]).map_err(plan_refused)?;
    let id = changes
        .iter()
        .find_map(|c| match c {
            Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .unwrap_or(item_id);
    let now = crate::pyutil::filetime_now();
    combat::add_wreck(
        reg,
        Wreck {
            wreck_id: id,
            type_id: item.type_id,
            solar_system_id: p.system,
            position: p.at,
            ship_id: p.ship_id,
            owner_id: p.owner,
            corp_id: p.corp,
            fleet_id: combat::fleet_of(reg, p.owner),
            abandoned: false,
            expires_at: now.saturating_add(lifetime),
        },
    );
    remember(
        reg,
        id,
        Row {
            system: p.system,
            at: p.at,
            active_at: now.saturating_add(activate),
            decay_at: now.saturating_add(lifetime),
        },
    );
    Ok(id)
}

/// Bring every parked unit back after a restart: a record for each row whose
/// item is still where the row says, so the combat sweep that follows does
/// not destroy it as an orphan. A row whose item is gone is dropped.
pub fn restore(w: &mut Registry) {
    let rows: Vec<(i64, Row)> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse::<i64>().ok()?, parse_row(v)?)))
        .collect();
    for (id, row) in rows {
        let Some(item) = w.item(id).cloned() else {
            forget(w, id);
            continue;
        };
        if item.location_id != row.system || w.combat().wreck(id).is_some() {
            continue;
        }
        let corp = w
            .character(item.owner_id)
            .map_or(item.owner_id, |c| c.corporation_id);
        combat::add_wreck(
            w,
            Wreck {
                wreck_id: id,
                type_id: item.type_id,
                solar_system_id: row.system,
                position: row.at,
                ship_id: id,
                owner_id: item.owner_id,
                corp_id: corp,
                fleet_id: combat::fleet_of(w, item.owner_id),
                abandoned: false,
                expires_at: row.decay_at,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Launching a deployable: ship.Drop
// ---------------------------------------------------------------------------

/// `ship.Drop([(itemID, qty)], whoseBehalfID, ignoreWarning)` for a Mobile
/// Depot or an MTU; `None` when none of the items is one, which carries on to
/// the Sovereignty Hub's. `{itemID: [newItemID]}` or `{itemID: [(label,
/// args)]}` (`eveMisc.LaunchFromShip`) [V]; a `PyValue::Dict` is (value, key).
pub(crate) fn drop_call(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Option<Result<PyValue, ServiceError>> {
    let list = match req.args.first() {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v,
        _ => return None,
    };
    let items: Vec<i64> = list
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => int(p.first()),
            PyValue::Int(id) => Some(*id),
            _ => None,
        })
        .collect();
    if !items.iter().any(|id| {
        reg.item(*id)
            .is_some_and(|i| spec(reg.content(), i.group_id).is_some())
    }) {
        return None;
    }
    let mut answer: Vec<(PyValue, PyValue)> = Vec::new();
    for id in items {
        let entry = match req.actor.map(|a| launch_one(reg, a, id)) {
            Some(Ok(new_id)) => PyValue::Int(new_id),
            Some(Err(why)) => refusal(&why),
            None => refusal("You must be in space to do that."),
        };
        answer.push((PyValue::List(vec![entry]), PyValue::Int(id)));
    }
    Some(Ok(PyValue::Dict(answer)))
}

/// One refusal in a launch answer: `(label, {args})`, raised by the client as
/// `UserError(label, args)` [V].
pub(crate) fn refusal(text: &str) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr("CustomNotify"),
        PyValue::Dict(crate::pyutil::pypairs(vec![(
            "notify",
            crate::pyutil::bstr(text),
        )])),
    ])
}

/// The checks of a launch and the launch. The new unit's id, or the reason.
pub fn launch_one(reg: &mut Registry, actor: i64, item_id: i64) -> Result<i64, String> {
    let c = reg
        .character(actor)
        .cloned()
        .ok_or_else(|| "You must be in space to do that.".to_owned())?;
    reg.advance_character(actor);
    let here = reg
        .roster()
        .get(actor)
        .copied()
        .filter(|p| p.ship_id == c.ship_id)
        .ok_or_else(|| "You must be in space to do that.".to_owned())?;
    let item = reg
        .store()
        .get(item_id)
        .cloned()
        .filter(|i| {
            i.owner_id == actor && i.location_id == c.ship_id && LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .ok_or_else(|| "That is not in your ship's hold.".to_owned())?;
    let spec =
        spec(reg.content(), item.group_id).ok_or_else(|| "That cannot be launched.".to_owned())?;
    if here.motion.warp_active() {
        return Err("You cannot launch that while you are in warp.".to_owned());
    }
    let at = [
        here.motion.position[0] + spec.deploy_at,
        here.motion.position[1],
        here.motion.position[2],
    ];
    let system = here.solar_system_id;
    let too_close = reg.combat().wrecks().any(|wr| {
        wr.solar_system_id == system
            && wr.wreck_id != item_id
            && reg
                .item(wr.wreck_id)
                .is_some_and(|i| i.group_id == spec.group_id)
            && crate::space_state::distance(wr.position, at) < spec.own_group_gap
    });
    if too_close {
        return Err(format!(
            "You cannot launch that within {} km of another of its kind.",
            (spec.own_group_gap / 1000.0).ceil() as i64
        ));
    }
    let id = park(
        reg,
        item_id,
        &Parked {
            owner: actor,
            corp: c.corporation_id,
            ship_id: c.ship_id,
            system,
            at,
        },
        spec.decay,
        spec.activate,
    )
    .map_err(|_| "The server could not launch that.".to_owned())?;
    crate::outln!(
        "[mtu] {} launched group {} as {id} in {system}",
        actor,
        spec.group_id
    );
    Ok(id)
}

// ---------------------------------------------------------------------------
// Reach, access and moving things in
// ---------------------------------------------------------------------------

/// The pilot, their ship's position and the record of what they reach for;
/// refused when they are not in space beside it.
pub(crate) fn beside(
    reg: &mut Registry,
    actor: i64,
    unit_id: i64,
    range: f64,
) -> Result<(Character, Wreck), ServiceError> {
    let c = reg
        .character(actor)
        .cloned()
        .ok_or_else(|| notify("No character is selected."))?;
    reg.advance_character(actor);
    let wr = reg
        .combat()
        .wreck(unit_id)
        .copied()
        .ok_or_else(|| refused(Refusal::NotAvailable))?;
    let me = reg
        .roster()
        .get(actor)
        .filter(|p| p.ship_id == c.ship_id && p.solar_system_id == wr.solar_system_id)
        .copied()
        .ok_or_else(|| refused(Refusal::NotAvailable))?;
    let radius = crate::destiny::hull::of_in(reg.content(), c.ship_type_id).radius;
    let surface = crate::space_state::distance(me.motion.position, wr.position)
        - f64::from(radius)
        - f64::from(crate::destiny::WRECK_RADIUS);
    if surface > range {
        return Err(refused(Refusal::TooFar { maxdist: range }));
    }
    Ok((c, wr))
}

/// A deployable is its owner's alone, and shut until it is active.
pub fn gate_owner(reg: &Registry, actor: i64, unit: &Item) -> Result<(), ServiceError> {
    if unit.owner_id != actor {
        return Err(notify("That is not yours to open."));
    }
    if !is_active(reg, unit.item_id, crate::pyutil::filetime_now()) {
        return Err(notify("That is still activating."));
    }
    Ok(())
}

/// What a bound inventory on a deployable answers, or `None` for what
/// carries on.
pub(crate) fn unit_call(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    unit: &Item,
) -> Option<Result<PyValue, ServiceError>> {
    let sp = spec(reg.content(), unit.group_id)?;
    match req.method {
        "GetCapacity" => Some(Ok(crate::pyutil::keyval(vec![
            (
                "capacity",
                PyValue::Real(crate::containers::capacity(reg, unit)),
            ),
            (
                "used",
                PyValue::Real(crate::containers::used_volume(reg, unit.item_id)),
            ),
        ]))),
        "List" | "ListByFlags" | "ListIncludingContainers" => {
            gate_owner(reg, actor, unit).err().map(Err)
        }
        "Add" | "MultiAdd" | "StackAll" => Some(
            gate_owner(reg, actor, unit)
                .and_then(|()| {
                    if sp.allow_user_add {
                        Ok(())
                    } else {
                        Err(notify("Nothing can be put into that by hand."))
                    }
                })
                .and_then(|()| space_add(reg, req, actor, unit, sp.access_range)),
        ),
        _ => None,
    }
}

/// `Add`, `MultiAdd` or `StackAll` on a container or a depot in space: the
/// pilot's own items from their active ship's plain holds into it.
pub(crate) fn space_add(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    target: &Item,
    range: f64,
) -> Result<PyValue, ServiceError> {
    let (c, _) = beside(reg, actor, target.item_id, range + LOOT_RANGE_SLACK)?;
    if req.method == "StackAll" {
        return crate::containers::stack_all(reg, actor, target);
    }
    let (ids, quantity, single) = if req.method == "Add" {
        (
            int(req.args.first()).into_iter().collect(),
            int_kwarg(req, "qty"),
            true,
        )
    } else {
        (ids_of(req.args.first()), None, false)
    };
    let mut first = None;
    for id in ids {
        let step = plan_add(reg, &c, target, id, quantity)
            .and_then(|edits| reg.apply_items(&edits).map(|_| ()).map_err(plan_refused));
        if let Err(e) = step {
            if single {
                return Err(e);
            }
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok(PyValue::None),
    }
}

/// The edits that put `quantity` of `item_id` (all when `None`) into
/// `target`, or why not.
fn plan_add(
    reg: &Registry,
    c: &Character,
    target: &Item,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, ServiceError> {
    let item = reg
        .store()
        .get(item_id)
        .ok_or_else(|| refused(Refusal::NotAvailable))?;
    if item.owner_id != c.character_id
        || item.location_id != c.ship_id
        || !crate::fitting::is_plain_hold(item.flag_id)
        || crate::item::is_loaded_charge(item)
    {
        return Err(refused(Refusal::NotAvailable));
    }
    if item.singleton
        && (item.category_id == crate::model::CATEGORY_SHIP
            || crate::containers::is_container(item)
            || is_unit(reg.content(), item))
    {
        return Err(refused(Refusal::Unsupported(
            "That cannot be put into a container in space.",
        )));
    }
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    let n = part.unwrap_or(item.quantity);
    let unit_volume = crate::item::find_in(reg.content(), item.type_id).map_or(0.0, |t| t.volume);
    let free = (crate::containers::capacity(reg, target)
        - crate::containers::used_volume(reg, target.item_id))
    .max(0.0);
    if crate::fitting::units_that_fit(free, unit_volume) < n {
        return Err(refused(Refusal::NoRoom {
            available: free,
            volume: unit_volume * n as f64,
        }));
    }
    let to = Slot::new(c.character_id, target.item_id, FLAG_IN_CONTAINER);
    Ok(vec![match part {
        Some(quantity) => Edit::Split {
            item_id,
            quantity,
            to,
        },
        None => Edit::Move { item_id, to },
    }])
}

// ---------------------------------------------------------------------------
// Scooping a deployable
// ---------------------------------------------------------------------------

/// `ship.Scoop(unitID)` for a deployable (the caller found it is one): its
/// owner, beside it, with an empty cargo, into the `flag` hold of the active
/// ship. The unit leaves space and keeps its id.
pub fn scoop_unit(
    reg: &mut Registry,
    actor: i64,
    unit: &Item,
    flag: i64,
) -> Result<(), ServiceError> {
    let sp = spec(reg.content(), unit.group_id).ok_or_else(|| notify("You cannot scoop that."))?;
    if unit.owner_id != actor {
        return Err(notify("That is not yours to scoop."));
    }
    let (c, _) = beside(reg, actor, unit.item_id, LOOT_RANGE + LOOT_RANGE_SLACK)?;
    let _ = sp;
    if reg.store().contents_of(unit.item_id).next().is_some() {
        return Err(notify("Empty it before you scoop it."));
    }
    crate::fitting::check_room_in_hold(
        reg.content(),
        reg.store(),
        c.ship_id,
        unit.type_id,
        1,
        flag,
    )
    .map_err(refused)?;
    reg.apply_items(&[Edit::Move {
        item_id: unit.item_id,
        to: Slot::new(actor, c.ship_id, flag),
    }])
    .map_err(plan_refused)?;
    combat::release_wreck(reg, unit.item_id);
    forget(reg, unit.item_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// The MTU's cycle
// ---------------------------------------------------------------------------

/// A wreck or a jetcan an MTU may work on: not a unit, not an assembled
/// container.
fn eligible(w: &Registry, wr: &Wreck) -> bool {
    w.item(wr.wreck_id).is_some_and(|i| {
        i.group_id == combat::GROUP_WRECK
            || (i.group_id == crate::jettison::GROUP_CARGO_CONTAINER
                && i.type_id == crate::jettison::JETCAN_TYPE)
    })
}

/// The world clock's hook, beside `combat::tick`: drop the rows of gone
/// units, then run every active MTU whose cycle is due.
pub fn tick(w: &mut Registry, now: BlueTime) {
    if w.deltas().of(KIND).is_empty() {
        return;
    }
    if now >= w.ext::<Units>().next_prune {
        w.ext_mut::<Units>().next_prune = now.saturating_add(5 * SEC);
        let gone: Vec<i64> = w
            .deltas()
            .of(KIND)
            .keys()
            .filter_map(|k| k.first()?.parse::<i64>().ok())
            .filter(|id| w.item(*id).is_none())
            .collect();
        for id in gone {
            forget(w, id);
            w.ext_mut::<Units>().next_cycle.remove(&id);
        }
    }
    let due: Vec<(i64, Spec, Tractor)> = w
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse::<i64>().ok()?, parse_row(v)?)))
        .filter(|(_, row)| now >= row.active_at)
        .filter_map(|(id, _)| {
            let item = w.item(id)?;
            let sp = spec(w.content(), item.group_id)?;
            Some((id, sp, sp.tractor?))
        })
        .filter(|(id, _, t)| {
            w.ext::<Units>()
                .next_cycle
                .get(id)
                .is_none_or(|due| now >= *due)
                && w.combat().wreck(*id).is_some()
                && t.cycle > 0
        })
        .collect();
    for (id, sp, t) in due {
        w.ext_mut::<Units>()
            .next_cycle
            .insert(id, now.saturating_add(t.cycle));
        cycle(w, id, &sp, &t);
    }
}

/// One cycle of one MTU: loot what is in reach, then pull the nearest wreck.
fn cycle(w: &mut Registry, id: i64, sp: &Spec, t: &Tractor) {
    let (Some(unit), Some(me)) = (w.item(id).cloned(), w.combat().wreck(id).copied()) else {
        return;
    };
    let owner = unit.owner_id;
    let mut candidates: Vec<(f64, Wreck)> = w
        .combat()
        .wrecks()
        .filter(|wr| wr.wreck_id != id && wr.solar_system_id == me.solar_system_id)
        .filter(|wr| eligible(w, wr) && combat::has_loot_right(w, owner, wr))
        .map(|wr| (crate::space_state::distance(me.position, wr.position), *wr))
        .filter(|(d, _)| *d <= t.max_range)
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    // 1. Loot what is within the cargo's reach.
    for (d, wr) in &candidates {
        if *d > sp.access_range {
            break;
        }
        loot_into(w, &unit, wr);
    }
    // 2. Pull the nearest one that is out of reach, one step.
    let Some((dist, wr)) = candidates.iter().find(|(d, _)| *d > sp.access_range) else {
        return;
    };
    let velocity =
        crate::typedata::attribute_in(w.content(), unit.type_id, ATTR_MAX_TRACTOR_VELOCITY)
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_TRACTOR_VELOCITY);
    let seconds = t.cycle as f64 / SEC as f64;
    let step = crate::salvage::tractor_step(velocity, seconds, *dist, t.min_range);
    if step <= 0.0 {
        return;
    }
    let Some(dir) = crate::space_state::unit([
        me.position[0] - wr.position[0],
        me.position[1] - wr.position[1],
        me.position[2] - wr.position[2],
    ]) else {
        return;
    };
    let point = [
        wr.position[0] + dir[0] * step,
        wr.position[1] + dir[1] * step,
        wr.position[2] + dir[2] * step,
    ];
    if !combat::set_wreck_position(w, wr.wreck_id, point) {
        return;
    }
    let to: Vec<i64> = w
        .roster()
        .in_system(wr.solar_system_id)
        .iter()
        .map(|p| p.character_id)
        .filter(|c| w.character(*c).is_some())
        .collect();
    let stamp = w.next_destiny_stamp();
    if let Some(u) = crate::destiny::ActionUpdate::new(
        to,
        stamp,
        vec![crate::destiny::Action::SetBallPosition {
            ball: wr.wreck_id,
            point,
        }],
    ) {
        w.schedule(Duration::ZERO, u);
    }
}

/// Everything of `wr` that fits, into `unit`'s cargo, as the owner's.
fn loot_into(w: &mut Registry, unit: &Item, wr: &Wreck) {
    let mut free = (crate::containers::capacity(w, unit)
        - crate::containers::used_volume(w, unit.item_id))
    .max(0.0);
    let owner = unit.owner_id;
    let mut edits = Vec::new();
    for i in w.store().contents_of(wr.wreck_id) {
        let per = crate::item::find_in(w.content(), i.type_id).map_or(0.0, |t| t.volume);
        let fit = crate::fitting::units_that_fit(free, per).min(i.quantity);
        if fit <= 0 {
            continue;
        }
        free -= per * fit as f64;
        let to = Slot::new(owner, unit.item_id, FLAG_IN_CONTAINER);
        edits.push(if fit == i.quantity {
            Edit::Move {
                item_id: i.item_id,
                to,
            }
        } else {
            Edit::Split {
                item_id: i.item_id,
                quantity: fit,
                to,
            }
        });
    }
    if edits.is_empty() {
        return;
    }
    if let Err(e) = on_clock(w, |w| w.apply_items(&edits)) {
        crate::outln!("[mtu] the loot was refused: {e}");
        return;
    }
    combat::looted(w, owner, wr.wreck_id);
}

#[cfg(test)]
mod tests;
