//! **`shipStanceMgr`** -- the one call behind the tactical destroyer's three
//! stance buttons (lane `t3-e`; the model is [`crate::stance`]).
//!
//! `SetShipStance(shipID, stanceID)` [V]
//! (`inflight/shipstance.ShipStanceButtonController.set_stance`): the result
//! is discarded, so the answer is the old stance id (what the retail
//! `ShipStance.set_stance` returns) or `None`.

use super::*;
use crate::stance::{CHANGING_KEY, Refusal};

pub(crate) const SERVICES: &[&str] = &["shipStanceMgr"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "SetShipStance" => {
            let (Some(PyValue::Int(ship)), Some(PyValue::Int(stance))) =
                (req.args.first(), req.args.get(1))
            else {
                return Err(bad_args(req, "(shipID, stanceID)"));
            };
            let Some(me) = req.actor else {
                return Err(bad_args(req, "no character"));
            };
            match crate::stance::set_stance(reg, me, *ship, *stance, crate::pyutil::filetime_now())
            {
                Ok(done) => Ok(done.and_then(|d| d.old).map_or(PyValue::None, PyValue::Int)),
                Err(Refusal::Changing) => Err(ServiceError::Raise(eve_proto::user_error(
                    CHANGING_KEY,
                    vec![],
                ))),
                Err(Refusal::NotYours) => Err(bad_args(req, "not your active ship")),
                Err(Refusal::NoSuchStance) => Err(bad_args(req, "no such stance for this hull")),
                Err(Refusal::Store) => Err(bad_args(req, "the mode item could not be made")),
            }
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
    use crate::stance::tests::{ACTOR, SHIP_TYPE, world};

    fn set(reg: &mut Registry, ship: i64, stance: i64) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "shipStanceMgr",
                method: "SetShipStance",
                args: &[PyValue::Int(ship), PyValue::Int(stance)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ACTOR),
            },
        )
    }

    /// **Over the wire**: the call, the push the client's button listens for,
    /// the mode item as a singleton at flag 156, and the attribute that
    /// changed in the sheet the client's attribute panel is served from.
    #[test]
    fn switching_over_the_wire_pushes_and_changes_the_attribute() {
        let (mut reg, ship) = world();
        let _ = reg.take_pending();
        let before = crate::dogma::sheet(&reg, ship).expect("a sheet");
        assert_eq!(
            set(&mut reg, ship, crate::stance::STANCE_SPEED),
            Ok(PyValue::None)
        );
        let pushes: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| (p.to_push(), p.audience()))
            .collect();
        let want = (
            (
                "OnStanceActive",
                vec![
                    PyValue::Int(ship),
                    PyValue::Int(crate::stance::STANCE_SPEED),
                ],
            ),
            Some(vec![ACTOR]),
        );
        assert!(pushes.contains(&want), "{pushes:?}");
        let item = reg
            .store()
            .contents_of(ship)
            .find(|i| i.flag_id == crate::stance::FLAG_HIDDEN_MODIFIERS)
            .expect("the mode item");
        assert!(item.singleton, "a module to the client");
        let after = crate::dogma::sheet(&reg, ship).expect("a sheet");
        assert_ne!(
            before.ship(crate::stance::tests::AGILITY),
            after.ship(crate::stance::tests::AGILITY),
            "the sheet moved with the mode"
        );
        let _ = SHIP_TYPE;
    }

    /// The ship's served attributes carry `stanceSwitchTime`, which the
    /// button's ramp reads from godma.
    #[test]
    fn the_served_attributes_carry_the_switch_time() {
        let (reg, _ship) = world();
        let out =
            super::super::with_stance_switch_time(reg.content(), SHIP_TYPE, PyValue::Dict(vec![]));
        let PyValue::Dict(e) = out else {
            panic!("a dict")
        };
        assert!(
            e.iter().any(|(v, k)| {
                *k == PyValue::Int(crate::stance::ATTR_STANCE_SWITCH_TIME)
                    && matches!(v, PyValue::Real(ms) if *ms > 0.0)
            }),
            "{e:?}"
        );
    }

    #[test]
    fn a_second_switch_inside_the_lock_raises_the_clients_key() {
        let (mut reg, ship) = world();
        set(&mut reg, ship, crate::stance::STANCE_SPEED).expect("the first");
        let got = set(&mut reg, ship, crate::stance::STANCE_SNIPER);
        let e = format!("{:?}", got.expect_err("refused"));
        let key = crate::stance::CHANGING_KEY
            .bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(e.contains(&key), "{e}");
    }
}
