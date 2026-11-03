//! `repairSvc`: the Repair Shop window (GAP-REPAIR; maths in
//! [`crate::repair`]).
//!
//! The client binds `Moniker('repairSvc', (location, location, group))`
//! (`base_repairshop.py:44-60`); the bind is generic, and the docked station
//! comes from the acting character. Calls (`[V]`, `base_repairshop.py`):
//!
//! - `GetRepairQuotes(itemIDs)` -> `{itemID: [row]}`; a row is read by
//!   attribute for `itemID typeID groupID damage maxHealth repairable
//!   costToRepairOneUnitOfDamage`. `[I]` the client ships a header list of
//!   integers over these names; keyval rows serve attribute reads either way.
//! - `RepairItemsInStation(itemIDs, payment)`, the answer dropped.
//! - `RepairItemsInStructure(itemIDs)` (U5): the same repair, the whole quote
//!   paid, for a pilot docked in a structure that offers the service.
//!
//! `GetDamageReports` and `DamageModules` are never called by this client.

use super::*;
use crate::repair::{self as shop, Refusal};

pub(crate) const SERVICES: &[&str] = &["repairSvc"];

fn int_list(v: Option<&PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn row(l: &shop::Line) -> PyValue {
    crate::pyutil::keyval(vec![
        ("itemID", PyValue::Int(l.item_id)),
        ("typeID", PyValue::Int(l.type_id)),
        ("groupID", PyValue::Int(l.group_id)),
        ("damage", PyValue::Real(l.hurt.total())),
        ("maxHealth", PyValue::Real(l.max_health)),
        ("repairable", PyValue::Bool(true)),
        ("costToRepairOneUnitOfDamage", PyValue::Real(l.unit_cost)),
    ])
}

fn error(r: &Refusal) -> ServiceError {
    ServiceError::Raise(match r {
        Refusal::NotDocked => eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![(
                "notify",
                crate::pyutil::bstr("Repairs are done in stations."),
            )]),
        ),
        Refusal::NoService(why) => eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(why))]),
        ),
        // `[A]` the wording; the client's own popup already asks for a full
        // payment, so this is only a hand-made call.
        Refusal::PartialWithModule => eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![(
                "notify",
                crate::pyutil::bstr("Modules must be repaired in full."),
            )]),
        ),
        Refusal::Funds { required, balance } => eve_proto::user_error(
            "NotEnoughMoney",
            crate::pyutil::pypairs(vec![
                ("amount", PyValue::Real(*required)),
                ("balance", PyValue::Real(*balance)),
            ]),
        ),
    })
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetRepairQuotes" => {
            let ids = int_list(req.args.first());
            let station = req.actor.and_then(|a| reg.docked_station(a));
            let actor = req.actor.unwrap_or(0);
            let entries = ids
                .into_iter()
                .map(|id| {
                    let rows: Vec<PyValue> = station
                        .map(|s| shop::lines_for(reg, actor, s, id))
                        .unwrap_or_default()
                        .iter()
                        .map(row)
                        .collect();
                    (PyValue::List(rows), PyValue::Int(id))
                })
                .collect();
            Ok(PyValue::Dict(entries))
        }
        "RepairItemsInStation" => {
            let Some(actor) = req.actor else {
                return Ok(PyValue::None);
            };
            let ids = int_list(req.args.first());
            let payment = match req.args.get(1) {
                Some(PyValue::Real(r)) => *r,
                Some(PyValue::Int(n)) => *n as f64,
                _ => 0.0,
            };
            shop::repair_items(reg, actor, &ids, payment).map_err(|r| error(&r))?;
            Ok(PyValue::None)
        }
        "RepairItemsInStructure" => {
            let Some(actor) = req.actor else {
                return Ok(PyValue::None);
            };
            let ids = int_list(req.args.first());
            shop::repair_all(reg, actor, &ids).map_err(|r| error(&r))?;
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}
