//! World-event tales, for the Job Board.
//!
//! Split out of `service/space.rs` by W10.5 seam S2 (`docs/content/plan.md`
//! §6.1), so a lane working on tales opens one file and cannot collide
//! with a lane working on flight. The arms and their tests moved verbatim.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["taleMgr"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // World-event tales, for the Job Board's World Events tab. The
        // provider iterates the answer and then **string-subscripts** each
        // element -- `tale_info['taleID']`,
        // `tale_info['managerSolarSystemID']` -- so the rows are dicts rather
        // than attribute rows, and `[]` is the empty rather than `{}`.
        // `None` is `TypeError: not iterable`.
        //
        // The client wrapper is `@Memoize(15)`, so this is bounded whatever
        // we answer. The rows are `crate::tales::Table`'s world events (CP51);
        // with no table registered the tab shows nothing, which is true.
        "taleMgr" => match req.method {
            "GetGlobalWorldEventTales" => Ok(reg
                .content()
                .try_get::<crate::tales::Table>()
                .map_or(PyValue::List(vec![]), |t| t.global_rows())),
            // `tale_service.get_active_tales_by_template_id:154` iterates it
            // and string-subscripts each element (`raw_data['tale_id']`), so
            // the empty is a list. The callers are the Vanguard map filter
            // and the community progress bar.
            "get_active_tales_by_template" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// Lane B1, the breaker sweep: one test per arm, each asserting the type the
/// consumer pins (`docs/lanes/breaker-sweep.md`). [A] values are not asserted.
#[cfg(test)]
mod breaker_sweep_tests {
    use super::*;

    fn call(service: &str, method: &str) -> PyValue {
        let mut reg = Registry::new(SandboxConfig::default());
        dispatch(
            &mut reg,
            &Request {
                service,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .unwrap_or_else(|e| panic!("{service}.{method} must be answered: {e:?}"))
    }

    fn is_empty_list(v: &PyValue) -> bool {
        matches!(v, PyValue::List(items) if items.is_empty())
    }

    /// CP51: with a tales table, the Job Board rows are its world events;
    /// a tale without `objectiveChainID` is not among them.
    #[test]
    fn global_world_event_tales_come_from_the_table() {
        assert!(is_empty_list(&call("taleMgr", "GetGlobalWorldEventTales")));
        let tale = |id, chain| crate::tales::Tale {
            tale_id: id,
            template_id: 7,
            template_class_id: crate::tales::WORLD_EVENTS,
            objective_chain_id: chain,
            location_id: 100,
            manager_solar_system_id: 101,
            name_id: 5,
            description_id: None,
            aggressor_faction_id: None,
            end_time: None,
            scene_types: vec![],
        };
        let mut content = crate::content::Content::fixture();
        content.insert(
            "tales",
            crate::tales::Table {
                tales: vec![tale(1, Some(40)), tale(2, None)],
            },
        );
        let mut reg =
            Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content));
        let req = Request {
            service: "taleMgr",
            method: "GetGlobalWorldEventTales",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        let PyValue::List(rows) = dispatch(&mut reg, &req).unwrap() else {
            panic!("not a list")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            crate::pyutil::dict_get(&rows[0], b"taleID"),
            Some(&PyValue::Int(1))
        );
    }

    /// `tale_service.get_active_tales_by_template_id:154` iterates it.
    #[test]
    fn active_tales_by_template_is_a_list() {
        assert!(is_empty_list(&call(
            "taleMgr",
            "get_active_tales_by_template"
        )));
    }
}
