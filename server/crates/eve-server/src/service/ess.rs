//! **`essMgr`** -- the ESS main bank's panel (lane `ess-1`, issue #89; the
//! model is [`crate::ess`]).
//!
//! `dynamicResourceSvc.GetESSDataForCurrentSystem` is
//!
//! ```text
//! if GetESSFeatureFlaggedOffline(self.machoNet):
//!     return None
//! else:
//!     return sm.RemoteSvc('essMgr').GetDataForClientSolarSystem()
//! ```
//!
//! **[V]**, and the flag cannot be set from here (`machoNet.
//! InitClientGlobalConfig` is not exported, port-tracker §41.10). So the
//! answer is the data dict in a system that holds an ESS, and `None`, CCP's
//! own "no ESS here", anywhere else; every caller copes with it.
//!
//! ESS-2 (issue #90) adds the link: `AttemptLinkToMainBank`,
//! `RequestMainBankUnlink` and the theft ledger.

use super::*;

pub(crate) const SERVICES: &[&str] = &["essMgr"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetDataForClientSolarSystem" => {
            let Some(system) = req.actor.and_then(|a| reg.session_system(a)) else {
                return Ok(PyValue::None);
            };
            if !crate::ess::has_ess_in(reg, system) {
                return Ok(PyValue::None);
            }
            Ok(crate::ess::data(reg, system, crate::pyutil::filetime_now()))
        }
        // The ESS transaction history.
        // `dynamicResourceSvc.GetESSAccessHistoryForCurrentSystem:190-192`
        // packs both into `{'main': ..., 'reserve': ...}` and
        // `transaction_history_window_controller:22,41` iterates each.
        // The main one is the theft ledger of the pilot's system (ESS-2); the
        // reserve bank is not built (owner decision #96).
        "GetMainBankTheftsForClientSolarSystem" => Ok(req
            .actor
            .and_then(|a| reg.session_system(a))
            .map_or_else(|| PyValue::List(vec![]), |s| crate::ess::thefts(reg, s))),
        "GetReserveBankTheftsForClientSolarSystem" => Ok(PyValue::List(vec![])),
        // `dynamicResourceSvc.RequestESSMainBankLink:117-122`: the client
        // checks the link first, so a refusal here is a stale view; it is a
        // dialog, with the client's own code in the text's place.
        "AttemptLinkToMainBank" => {
            let Some(actor) = req.actor else {
                return Ok(PyValue::None);
            };
            match crate::ess::link(reg, actor, crate::pyutil::filetime_now()) {
                Ok(_) => Ok(PyValue::None),
                Err(no) => Err(crate::modules::user_error(
                    "CustomNotify",
                    vec![("notify", crate::pyutil::bstr(no.text()))],
                )),
            }
        }
        // `RequestESSMainBankUnlink:133-134`.
        "RequestMainBankUnlink" => {
            if let Some(actor) = req.actor {
                crate::ess::unlink(reg, actor, crate::pyutil::filetime_now());
            }
            Ok(PyValue::None)
        }
        _ => Err(reg.no_method(req)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PA: i64 = 5_001;

    fn ask(reg: &mut Registry, actor: Option<i64>, method: &str) -> PyValue {
        dispatch(
            reg,
            &Request {
                service: "essMgr",
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .unwrap_or_else(|e| panic!("essMgr.{method} must be answered: {e:?}"))
    }

    /// `dynamicResourceSvc.GetESSAccessHistoryForCurrentSystem:190-191`, read
    /// by `transaction_history_window_controller:22,41` as iterables.
    #[test]
    fn ess_thefts_are_lists() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "GetMainBankTheftsForClientSolarSystem",
            "GetReserveBankTheftsForClientSolarSystem",
        ] {
            assert_eq!(ask(&mut reg, None, method), PyValue::List(vec![]));
        }
    }

    /// No ESS in the pilot's system (or no pilot): `None`, as before.
    #[test]
    fn a_system_without_an_ess_answers_none() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(
            ask(&mut reg, None, "GetDataForClientSolarSystem"),
            PyValue::None
        );
        assert_eq!(
            ask(&mut reg, Some(PA), "GetDataForClientSolarSystem"),
            PyValue::None
        );
    }

    /// In the ESS system the pilot gets the dict; the key the client's
    /// `ess_data['essID'] is None` test reads is a number.
    #[test]
    fn a_pilot_in_an_ess_system_gets_the_data() {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![crate::model::Character {
                character_id: PA,
                name: "Alice".to_owned(),
                solar_system_id: Some(39_000_003),
                ..crate::model::Character::default()
            }];
        });
        let d = ask(&mut reg, Some(PA), "GetDataForClientSolarSystem");
        assert_eq!(
            crate::pyutil::dict_get(&d, b"essID"),
            Some(&PyValue::Int(crate::ess::ess_id(39_000_003)))
        );
    }

    /// The link and unlink calls: a refusal is a raised dialog, an unlink
    /// with nothing to end is quietly `None`, and the ledger is a list of
    /// `{characterID, amount}` for the pilot's own system.
    #[test]
    fn the_link_calls_answer() {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![crate::model::Character {
                character_id: PA,
                ship_id: 6_001,
                name: "Alice".to_owned(),
                solar_system_id: Some(39_000_003),
                ..crate::model::Character::default()
            }];
        });
        let call = |reg: &mut Registry, method: &str| {
            dispatch(
                reg,
                &Request {
                    service: "essMgr",
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(PA),
                },
            )
        };
        // Not in space: a dialog.
        assert!(matches!(
            call(&mut reg, "AttemptLinkToMainBank"),
            Err(ServiceError::Raise(_))
        ));
        assert_eq!(
            call(&mut reg, "RequestMainBankUnlink").expect("answers"),
            PyValue::None
        );
        // In range: linked, and the dict says so; then unlinked.
        use crate::space_state::{Motion, Presence};
        let at = crate::ess::ess_position(&reg, 39_000_003);
        reg.roster_mut().arrive(Presence::arriving(
            PA,
            6_001,
            39_000_003,
            Motion::arrived(at, [0.0; 3]),
        ));
        assert_eq!(
            call(&mut reg, "AttemptLinkToMainBank").expect("links"),
            PyValue::None
        );
        assert!(crate::ess::linked(&reg, PA));
        let d = call(&mut reg, "GetDataForClientSolarSystem").expect("data");
        let link = crate::pyutil::dict_get(&d, b"mainBankLink").expect("key");
        assert_eq!(
            crate::pyutil::dict_get(link, b"characterID"),
            Some(&PyValue::Int(PA))
        );
        assert!(matches!(
            call(&mut reg, "AttemptLinkToMainBank"),
            Err(ServiceError::Raise(_))
        ));
        call(&mut reg, "RequestMainBankUnlink").expect("answers");
        assert!(!crate::ess::linked(&reg, PA));
        assert_eq!(
            call(&mut reg, "GetMainBankTheftsForClientSolarSystem").expect("a list"),
            PyValue::List(vec![])
        );
    }
}
