//! Capital docking policy at NPC stations (CAP-K, `R-CAP.md` §3.7).
//!
//! **What the client says, and what it does not.** The client has no rule
//! about which hull may dock at an NPC station **[V]**: `ballparkCommon.
//! docking.CanDock` is called only from `undockQuestions.
//! IsOkToUndockFromStructureInShipThatCannotRedock` (guarded by
//! `session.structureid`) and the boarding-moment QA tool, so it never runs
//! for a station, and `hangarRegistry` only picks a hangar model. What the
//! client *does* hold is the vocabulary for a refusal and the size classes:
//!
//! - **The refusal key [V]**: `DockingRequestDenied` with
//!   `reason = (UE_LOC, 'UI/Station/StationManagment/ShipTooLarge')`,
//!   "Your ship is too large to dock here". `structureDocking.Dock` compares
//!   `e.dict['reason']` with exactly that tuple to decide whether to scatter
//!   `OnDockingRequestWasDenied`, so the tuple form is the client's own.
//!   `UE_LOC` is 101 (`carbon/common/lib/const.py`).
//! - **The size classes [V]**: `docking.GROUP_MAPPING` (our
//!   [`crate::docking::UndockCategory`]) and, for structures,
//!   `ONE_WAY_UNDOCKS_BY_STRUCTURE_TYPE` (Raitaru and Azbel take no
//!   Capital or Supercapital back in; Sotiyo no Supercapital) and
//!   `EXCLUDED_DOCKS_BY_STRUCTURE_TYPE` (Tatara refuses groups 547, 5120,
//!   1538, 485, 4594). That is the client's own list of hulls it treats as
//!   un-dockable somewhere, and it is a list of **Capital**-category groups.
//!
//! The rule below is therefore server policy built from those two facts:
//!
//! 1. A hull in the Capital or Supercapital undock category is refused at
//!    an NPC station with the `ShipTooLarge` denial, **except**
//! 2. group 883 (Capital Industrial, the Rorqual), which docks **[A]**;
//!    falsifier: the owner, or a retail check of a Rorqual in a station.
//!    The client maps 883 to the Capital category but excludes it from
//!    nothing, and no client file says whether a station takes it.
//! 3. **A2 switch** (`/dock-capitals`, default on): a Capital-category hull
//!    (not Supercapital **[A]**, falsifier: the owner) may dock at an NPC
//!    station in null-sec, the one place a capital pilot would otherwise
//!    have nowhere to live **[A]**, falsifier: the owner. An unknown system
//!    is not null-sec.
//!
//! Freighters and Jump Freighters are Large and always dock. The switch is
//! the world's, in a `world_ext` slot, and is not persisted: a restart
//! returns it to its default.

use crate::celestial::SecurityClass;
use crate::content::Content;
use crate::docking::{UndockCategory, undock_category_by_type};
use crate::item;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;

/// `DockingRequestDenied`, `UI/Messages/808` **[V]**.
pub const DENIED: &str = "DockingRequestDenied";

/// The `reason` the client itself compares against **[V]**.
pub const TOO_LARGE_LABEL: &str = "UI/Station/StationManagment/ShipTooLarge";

/// `const.UE_LOC` **[V]**: a localisation label as a message argument.
pub const UE_LOC: i64 = 101;

/// `groupCapitalIndustrialShip`: the Rorqual's group, which docks **[A]**.
pub const CAPITAL_INDUSTRIAL_GROUP: i64 = 883;

/// The switch's value in a new world: capitals dock in null-sec **[A]**.
pub const DEFAULT_CAPITALS_IN_NULL: bool = true;

/// The world's docking switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// `/dock-capitals`: Capital-category hulls dock at null-sec NPC
    /// stations.
    pub capitals_in_null: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            capitals_in_null: DEFAULT_CAPITALS_IN_NULL,
        }
    }
}

/// The `world_ext::SLOTS` row (coordinator-only).
pub const SLOT: SlotSpec = SlotSpec {
    name: "dock_policy",
    make,
};

fn make(_: &Content) -> Boxed {
    Box::new(Policy::default())
}

/// **The rule, pure.** Whether a hull of this undock `category` and `group`
/// may dock at an NPC station in a system of this security `class`.
pub fn may_dock(
    category: UndockCategory,
    group_id: i64,
    class: Option<SecurityClass>,
    capitals_in_null: bool,
) -> bool {
    if group_id == CAPITAL_INDUSTRIAL_GROUP {
        return true;
    }
    match category {
        UndockCategory::Capital => capitals_in_null && class == Some(SecurityClass::Zero),
        UndockCategory::Supercapital => false,
        UndockCategory::Default | UndockCategory::Small | UndockCategory::Large => true,
    }
}

/// The denial the client knows: `DockingRequestDenied` with the
/// `ShipTooLarge` reason.
pub fn too_large() -> ServiceError {
    // A marshal dict's pairs are `(value, key)`: `pypairs` knows.
    ServiceError::Raise(eve_proto::user_error(
        DENIED,
        crate::pyutil::pypairs(vec![(
            "reason",
            PyValue::Tuple(vec![
                PyValue::Int(UE_LOC),
                crate::pyutil::bstr(TOO_LARGE_LABEL),
            ]),
        )]),
    ))
}

/// `Err(too_large)` when `actor`'s active ship may not dock at `station`.
/// Call it in the `CmdDock` arm after the criminal gates, before the dock.
/// A station this world does not know, or an actor it does not know, is
/// let through: `dock_target` refuses the first with `NotDockable`.
pub fn refuse_dock(
    reg: &Registry,
    actor: Option<i64>,
    station: Option<i64>,
) -> Result<(), ServiceError> {
    let (Some(actor), Some(station)) = (actor, station) else {
        return Ok(());
    };
    let (Some(c), Some(st)) = (reg.character(actor), reg.station_ref(station)) else {
        return Ok(());
    };
    let content = reg.content();
    let category = undock_category_by_type(content, c.ship_type_id);
    let group = item::find_in(content, c.ship_type_id).map_or(0, |t| t.group_id);
    let class = content
        .get::<crate::celestial::Universe>()
        .security_class(st.solar_system_id);
    if may_dock(category, group, class, reg.ext::<Policy>().capitals_in_null) {
        Ok(())
    } else {
        Err(too_large())
    }
}

/// The client sends `CmdDock` only inside this surface distance, metres:
/// `menusvc.DockStation` -> `GetCloseAndTryCommand`'s `interactionRange`
/// (`DEFAULT_INTERACTION_RANGE`, `menusvc.py:158`) and `autopilot.
/// InInteractionRange` (`surfaceDist < interactionRange`, `:682`) **[V]**.
pub const DOCK_RANGE: f64 = 2_500.0;

/// What the server adds to [`DOCK_RANGE`] before it refuses, metres. The
/// client measures on its own interpolated ball, a tick or a round trip
/// behind ours, and a ship that closes at a few hundred m/s (or an MWD's
/// thousand) moves that far between the client's check and our reading.
/// `[A]`; falsified by an honest dock refused with `NotDockable` (the
/// `[dock] out of range` log line gives the distance), or by a modified
/// client docking from 4 km.
pub const DOCK_RANGE_SLACK: f64 = 1_500.0;

/// The surface distance from `actor`'s ship to `station`, when the world
/// can say: the pilot is in space and the station is a catalogued ball.
/// A station known only as somebody's home has no position (the origin is
/// a stand-in), so it has no distance.
pub fn station_surface_distance(reg: &Registry, actor: i64, station: i64) -> Option<f64> {
    reg.celestials()
        .get(station)
        .filter(|c| c.kind == crate::celestial::Kind::Station)?;
    let me = reg.ball_target(reg.roster().get(actor)?.ship_id)?;
    let st = reg.ball_target(station)?;
    Some(crate::space_state::distance(me.position, st.position) - me.radius - st.radius)
}

/// `Err(NotDockable)` when `actor` is in space farther from the station's
/// surface than the client would ever have sent the dock from
/// ([`DOCK_RANGE`] + [`DOCK_RANGE_SLACK`]). A pilot not in space (the
/// window between `ship.Undock` and the bind), or a station with no
/// position, is let through. Call it in the `CmdDock` arm after the
/// policy gates, before `dock_target`.
pub fn refuse_out_of_range(
    reg: &Registry,
    actor: Option<i64>,
    station: Option<i64>,
) -> Result<(), ServiceError> {
    let (Some(actor), Some(station)) = (actor, station) else {
        return Ok(());
    };
    match station_surface_distance(reg, actor, station) {
        Some(d) if d > DOCK_RANGE + DOCK_RANGE_SLACK => {
            crate::outln!("[dock] out of range: {actor} is {d:.0} m from station {station}");
            Err(ServiceError::Raise(eve_proto::user_error(
                crate::service::space::NOT_DOCKABLE,
                vec![],
            )))
        }
        _ => Ok(()),
    }
}

/// `/dock-capitals [on|off]`: let Capital-category hulls dock at null-sec
/// NPC stations (the A2 switch), or not; no word reports it. A
/// `slash::COMMANDS` row (coordinator).
pub fn slash(
    reg: &mut Registry,
    _actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    match words {
        [] => {}
        ["on"] => reg.ext_mut::<Policy>().capitals_in_null = true,
        ["off"] => reg.ext_mut::<Policy>().capitals_in_null = false,
        _ => return say("usage: /dock-capitals [on|off]"),
    }
    if reg.ext::<Policy>().capitals_in_null {
        say("capitals dock at null-sec NPC stations")
    } else {
        say("capitals do not dock at NPC stations")
    }
}

#[cfg(test)]
mod tests;
