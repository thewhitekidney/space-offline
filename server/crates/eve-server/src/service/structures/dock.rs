//! `structureDocking` and `structureHangarViewMgr` (lane U4, issue #64). The
//! logic is [`crate::structure_dock`]; this is the arguments.
//!
//! - `structureDocking.Dock(structureID, shipID)` **[V]**
//!   (`structureDocking.Dock`, `RequestDocking`): a plain service call, not a
//!   `PerformSessionChange`. The answer is `None`; the session change is
//!   queued by the dock and goes out in front of it.
//! - `structureDocking.Undock(structureID, shipID, ignoreContraband=)`
//!   **[V]**: inside `PerformSessionChange('undock', ...)`. `shipID` is
//!   `None` for a pilot controlling the structure. The session change
//!   (`{structureid: (id, None)}`) is `dispatch::session_change_for`'s.
//! - `structureControl` (U6C, [`crate::structure_dock::control`]):
//!   `TakeControl(structureID)` and `ReleaseControl()` are bare `RemoteSvc`
//!   calls whose answer is discarded; `EjectFromStructure(shipID=None)` and
//!   `BoardStructure(structureID)` run inside `PerformSessionChange('board')`;
//!   `GetStructurePilot(structureID)` is the character id at the controls or
//!   `None` **[V]** (`structureControl.py`).
//! - `structureHangarViewMgr.GetMyHangarViewState()` **[V]**
//!   (`structureHangarViewSvc`): the state the hangar scene reads, for the
//!   structure the caller is docked in. `None` would raise
//!   `AttributeError` in `UpdateStructureState`, so a pilot who is not docked
//!   gets the default full-health state.

use super::*;
use crate::structure_dock::{self as dock, control};

pub(super) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match (req.service, req.method) {
        ("structureDocking", "Dock") => {
            let (Some(actor), Some(id)) = (req.actor, int(req.args.first())) else {
                return Err(bad_args(req, "Dock(structureID, shipID)"));
            };
            dock::dock(reg, actor, id, int(req.args.get(1)))?;
            Ok(PyValue::None)
        }
        ("structureDocking", "Undock") => {
            let (Some(actor), Some(id)) = (req.actor, int(req.args.first())) else {
                return Err(bad_args(req, "Undock(structureID, shipID)"));
            };
            dock::undock(reg, actor, id, int(req.args.get(1)))?;
            Ok(PyValue::None)
        }
        ("structureControl", "TakeControl") => {
            let (Some(actor), Some(id)) = (req.actor, int(req.args.first())) else {
                return Err(bad_args(req, "TakeControl(structureID)"));
            };
            control::take(reg, actor, id)?;
            Ok(PyValue::None)
        }
        ("structureControl", "ReleaseControl") => {
            let Some(actor) = req.actor else {
                return Err(bad_args(req, "ReleaseControl()"));
            };
            control::release(reg, actor, None)?;
            Ok(PyValue::None)
        }
        ("structureControl", "EjectFromStructure") => {
            let Some(actor) = req.actor else {
                return Err(bad_args(req, "EjectFromStructure(shipID)"));
            };
            control::release(reg, actor, int(req.args.first()))?;
            Ok(PyValue::None)
        }
        ("structureControl", "BoardStructure") => Err(control::board()),
        ("structureControl", "GetStructurePilot") => {
            let Some(id) = int(req.args.first()) else {
                return Err(bad_args(req, "GetStructurePilot(structureID)"));
            };
            Ok(control::pilot_of(reg, id).map_or(PyValue::None, PyValue::Int))
        }
        ("structureHangarViewMgr", "GetMyHangarViewState") => {
            let now = crate::pyutil::filetime_now();
            let s = req
                .actor
                .and_then(|a| dock::docked_in(reg, a))
                .and_then(|id| structure::get(reg.deltas(), id));
            Ok(match s {
                Some(s) => dock::hangar_state(&s, now),
                None => dock::default_hangar_state(),
            })
        }
        _ => Err(reg.no_method(req)),
    }
}
