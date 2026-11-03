//! `keeper`: the acceleration gate between the rooms of a combat site.
//!
//! Y-COMBAT (`docs/lanes/y-combat.md`). The one call the client makes is
//! `sm.RemoteSvc('keeper').ActivateAccelerationGate(gateID)`, inside
//! `PerformSessionChange(..., violateSafetyTimer=1)`
//! (`movementFunctions.RealActivateAccelerationGate:534`) **[V]**. The rules
//! and the room are `crate::combat_site::activate`'s; this arm starts the
//! ship's warp to the room it answers, the way `CmdWarpToStuff` does, and
//! answers `None` (the call's return value is discarded **[V]**).
//!
//! The dev-tool `keeper` calls (`EditDungeon`, `PlayDungeon`, `Reset`,
//! `GetLevelEditor`) are GM scenario tooling and stay unanswered.

use super::*;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["keeper"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "ActivateAccelerationGate" => {
            let actor = req.actor.ok_or_else(|| reg.no_method(req))?;
            let gate = crate::pyutil::opt_id(req.args.first()).ok_or_else(|| reg.no_method(req))?;
            let through = crate::combat_site::activate(reg, actor, gate)
                .map_err(crate::combat_site::Shut::into_error)?;
            if let Some(here) = reg.roster().get(actor).copied() {
                super::space::warp_to_point(
                    reg,
                    actor,
                    here,
                    through.aim,
                    through.standoff,
                    crate::targeting::Delivery::WithAnswer,
                );
            }
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}
