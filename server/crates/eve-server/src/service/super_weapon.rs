//! **`superWeaponMgr`** (NW-9a; the model is [`crate::superweapon`]).
//!
//! `positionalControl._StartTargetedEffect` calls
//! `ActivateSinglePointTargetedModule(moduleID, effectID, destination)` and
//! `_StartSlasherEffect` calls `ActivateSlashModule(moduleID, p0, p1)` [V];
//! both discard the answer (`findings/client-calls.tsv`). Only the first
//! reaches the Lance (a cylinder); the second is the Reaper's slice (9b).

use super::*;

pub(crate) const SERVICES: &[&str] = &["superWeaponMgr"];

/// A point as the client marshals it **[A]**: a 3-tuple or list of numbers.
fn point(v: Option<&PyValue>) -> Option<[f64; 3]> {
    let items = match v? {
        PyValue::Tuple(t) | PyValue::List(t) => t,
        _ => return None,
    };
    let n = |x: &PyValue| match x {
        PyValue::Real(f) => Some(*f),
        PyValue::Int(i) => Some(*i as f64),
        _ => None,
    };
    match items.as_slice() {
        [x, y, z] => Some([n(x)?, n(y)?, n(z)?]),
        _ => None,
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "ActivateSinglePointTargetedModule" => {
            let (Some(PyValue::Int(module)), Some(dest)) =
                (req.args.first(), point(req.args.get(2)))
            else {
                return Err(bad_args(req, "(moduleID, effectID, (x, y, z))"));
            };
            crate::superweapon::fire_lance(
                reg,
                req.actor,
                *module,
                dest,
                crate::pyutil::filetime_now(),
            )?;
            Ok(PyValue::None)
        }
        "ActivateSlashModule" => {
            let (Some(PyValue::Int(module)), Some(p0), Some(p1)) = (
                req.args.first(),
                point(req.args.get(1)),
                point(req.args.get(2)),
            ) else {
                return Err(bad_args(req, "(moduleID, (x, y, z), (x, y, z))"));
            };
            crate::superweapon::fire_slash(
                reg,
                req.actor,
                *module,
                p0,
                p1,
                crate::pyutil::filetime_now(),
            )?;
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superweapon::tests::{AIM, ALICE, ALICE_LANCE, ship, world};

    #[test]
    fn the_service_decodes_the_clients_call() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        let args = [
            PyValue::Int(ALICE_LANCE),
            PyValue::Int(0),
            PyValue::Tuple(vec![
                PyValue::Real(AIM[0]),
                PyValue::Real(AIM[1]),
                PyValue::Int(0),
            ]),
        ];
        let out = dispatch(
            &mut reg,
            &Request {
                service: "superWeaponMgr",
                method: "ActivateSinglePointTargetedModule",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(out, Ok(PyValue::None));
        assert!(crate::superweapon::immobile_at(&reg, ship(ALICE), now));
    }

    #[test]
    fn the_service_decodes_the_slash_call() {
        let mut reg = world();
        let args = [
            PyValue::Int(ALICE_LANCE),
            PyValue::Tuple(vec![PyValue::Real(1.0), PyValue::Real(2.0)]),
            PyValue::Int(0),
        ];
        let out = dispatch(
            &mut reg,
            &Request {
                service: "superWeaponMgr",
                method: "ActivateSlashModule",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert!(matches!(out, Err(ServiceError::BadArguments { .. })));
    }
}
