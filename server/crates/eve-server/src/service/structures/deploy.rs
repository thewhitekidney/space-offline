//! `structureDeployment`: `Anchor`, `Unanchor`, `CancelUnanchor` and
//! `RenameStructure` (lane U3, issue #63). The logic is
//! [`crate::structure_space`]; this is the arguments and the role.
//!
//! The roles: anchoring and renaming need the station-manager role (the
//! client's own `Deploy` tests `corprole & corpRoleStationManager`; a
//! director or the CEO passes too, `super::manager_corp`), and so does a
//! decommission. The browser's menu shows Decommission to a director only
//! (`structureEntryController.CanUnanchor`) **[V]**; the server is the
//! looser rule, which only ever matters to a hand-made call. **[A]**
//! falsifier: a station manager who is refused in a retail capture.
//!
//! The replies are all `None` **[V]** (`structureDeployment.ConfirmDeployment`
//! and the menu callbacks drop them); a refusal is a `CustomNotify`.

use super::*;
use crate::structure_space as space;

pub(super) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "Anchor" => anchor(reg, req),
        "Unanchor" => {
            let id = int(req.args.first()).ok_or_else(|| bad_args(req, "Unanchor(structureID)"))?;
            let corp = manager_corp(reg, req.actor)?;
            space::unanchor(reg, corp, id).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        "CancelUnanchor" => {
            let id = int(req.args.first())
                .ok_or_else(|| bad_args(req, "CancelUnanchor(structureID)"))?;
            let corp = manager_corp(reg, req.actor)?;
            space::cancel_unanchor(reg, corp, id).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        "RenameStructure" => {
            let (Some(id), Some(name)) = (int(req.args.first()), text(req.args.get(1))) else {
                return Err(bad_args(req, "RenameStructure(structureID, name)"));
            };
            let corp = manager_corp(reg, req.actor)?;
            space::rename(reg, corp, id, &name).map_err(|e| notify(&e))?;
            Ok(PyValue::None)
        }
        other => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: other.to_owned(),
        }),
    }
}

/// `Anchor(itemID, x, z, yaw, profileID, name, bio, reinforceWeekday,
/// reinforceHour, extraConfig)` **[V]**.
fn anchor(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    const SHAPE: &str = "Anchor(itemID, x, z, yaw, profileID, name, bio, weekday, hour, extra)";
    let a = &req.args;
    let (Some(item_id), Some(x), Some(z), Some(yaw)) = (
        int(a.first()),
        real(a.get(1)),
        real(a.get(2)),
        real(a.get(3)),
    ) else {
        return Err(bad_args(req, SHAPE));
    };
    let (Some(weekday), Some(hour)) = (int(a.get(7)), int(a.get(8))) else {
        return Err(bad_args(req, SHAPE));
    };
    let args = space::AnchorArgs {
        item_id,
        x,
        z,
        yaw,
        profile: int(a.get(4)),
        name: text(a.get(5)).unwrap_or_default(),
        bio: text(a.get(6)).unwrap_or_default(),
        reinforce_weekday: weekday,
        reinforce_hour: hour,
    };
    let corp = manager_corp(reg, req.actor)?;
    let actor = req.actor.unwrap_or_default();
    space::anchor(reg, actor, corp, &args).map_err(|e| notify(&e))?;
    Ok(PyValue::None)
}
