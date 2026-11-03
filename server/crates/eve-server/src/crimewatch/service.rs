//! The `crimewatch` bound object's calls beyond the state and the security
//! status (which `service/space.rs` answers): the safety setting and duels.
//! Reached from `space.rs`'s `crimewatch` arm (coordinator seam), so the
//! service keeps one route.
//!
//! - `SetSafetyLevel(safetyLevel)` -- `crimewatchSvc.SetSafetyLevel`, the
//!   HUD's safety button; the reply is discarded and the client sets its
//!   own copy at once **[V]**. Stored ([`super::set_safety`]) and answered
//!   back by every `GetClientStates`.
//! - `StartDuelChallenge(charID)` and `RespondToDuelChallenge(fromCharID,
//!   expiryTime, accept)` -- [`super::duel`].

use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// The methods this module answers.
pub const METHODS: &[&str] = &[
    "SetSafetyLevel",
    "StartDuelChallenge",
    "RespondToDuelChallenge",
];

fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args.get(i).or_else(|| {
        req.kwargs
            .iter()
            .find_map(|(v, k)| (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(v))
    })
}

fn bad(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn truthy(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(i)) => *i != 0,
        _ => false,
    }
}

/// `crimewatch.<method>` for the methods in [`METHODS`].
pub fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let now = crate::pyutil::filetime_now();
    match req.method {
        "SetSafetyLevel" => {
            let level = match arg(req, 0, "safetyLevel") {
                Some(PyValue::Int(v)) => super::Safety::from_wire(*v),
                _ => None,
            }
            .ok_or_else(|| bad(req, "safetyLevel is not 0, 1 or 2"))?;
            if let Some(me) = req.actor {
                super::set_safety(reg, me, level);
                crate::outln!("[crimewatch] {me} sets safety to {level:?}");
            }
            Ok(PyValue::None)
        }
        "StartDuelChallenge" => {
            let target = crate::pyutil::opt_id(arg(req, 0, "charID"))
                .ok_or_else(|| bad(req, "charID is not an id"))?;
            super::duel::start(reg, req.actor, target, now)
        }
        "RespondToDuelChallenge" => {
            let from = crate::pyutil::opt_id(arg(req, 0, "fromCharID"))
                .ok_or_else(|| bad(req, "fromCharID is not an id"))?;
            let expiry = match arg(req, 1, "expiryTime") {
                Some(PyValue::Int(t)) => Some(*t),
                _ => None,
            };
            let accept = truthy(arg(req, 2, "accept"));
            super::duel::respond(reg, req.actor, from, expiry, accept, now)
        }
        _ => Err(ServiceError::NoSuchMethod {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
        }),
    }
}
