//! Exploration sites: the escalation journal, the anomaly trackers and the probe scanner's registration.
//!
//! Split out of `service/space.rs` by W10.5 seam S2 (`docs/content/plan.md`
//! §6.1), so a lane working on sites opens one file and cannot collide
//! with a lane working on flight. The arms and their tests moved verbatim.

use super::*;
use crate::signal_tracker as st;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "scanMgr",
    "dungeonExplorationMgr",
    "dungeonInstanceCacheMgr",
];

/// The calls [`dispatch_ro`] answers (PA-12a): the Agency lists and counts
/// the write arm below answers from `&Registry` helpers.
pub(crate) const READ_METHODS: &[(&str, &str)] = &[
    ("dungeonInstanceCacheMgr", "GetCombatAnomalyInstances"),
    ("dungeonInstanceCacheMgr", "GetOreAnomalyInstances"),
    ("dungeonInstanceCacheMgr", "GetIceBeltInstances"),
    ("dungeonInstanceCacheMgr", "GetCombatAnomaliesCount"),
    ("dungeonInstanceCacheMgr", "GetOreAnomaliesCount"),
    ("dungeonInstanceCacheMgr", "GetIceBeltsCount"),
    ("dungeonInstanceCacheMgr", "GetSignatureInstances"),
];

/// The read twin of the `dungeonInstanceCacheMgr` arms above: the same
/// `signal_tracker` calls.
pub(crate) fn dispatch_ro(
    reg: &Registry,
    _caller: Caller,
    req: &Request<'_>,
) -> Option<Result<RoValue, ServiceError>> {
    if req.service != "dungeonInstanceCacheMgr" {
        return None;
    }
    Some(Ok(RoValue::Owned(match req.method {
        "GetCombatAnomalyInstances" => st::agency_instances(reg, req.actor, st::COMBAT),
        "GetOreAnomalyInstances" => st::agency_instances(reg, req.actor, st::ORE),
        "GetIceBeltInstances" => st::agency_instances(reg, req.actor, st::ICE),
        "GetCombatAnomaliesCount" => st::agency_counts(reg, st::COMBAT),
        "GetOreAnomaliesCount" => st::agency_counts(reg, st::ORE),
        "GetIceBeltsCount" => st::agency_counts(reg, st::ICE),
        "GetFactionWarfareInstances" => st::fw_instances(reg),
        "GetSignatureInstances" => st::agency_counts(reg, st::Want::Signatures),
        _ => return None,
    })))
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Escalating paths, and **the live lock with the biggest multiplier
        // we have found.** `journal.py(343) GetEscalations`:
        //
        //     if self.escalationRowset is None or force:
        //         self.escalationRowset = \
        //             sm.RemoteSvc('dungeonExplorationMgr').GetMyEscalatingPathDetails()
        //         sm.ScatterEvent('OnEscalationsDataUpdated')
        //     return self.escalationRowset
        //
        // The `contractProxy.NumOutstandingContracts` variant, not the LP
        // one: the assignment **completes** and stores the one value that
        // fails its own guard, so nothing throws and `--triage` reports
        // nothing at all.
        //
        // The 26 calls are not 26 consumers. There is exactly one caller of
        // the remote method, and the loop closes on itself in the Job Board's
        // escalation provider: `REFRESH_BUFFER_MSEC = 3000`,
        // `OnEscalationsDataUpdated` calls `refresh_jobs`, which is
        // `@uthread2.BufferedCall(delay=3000)`, which reaches
        // `_get_all_content`, which calls `GetEscalations()` again.
        // **Our own `ScatterEvent` is the clock.** The capture's timestamps
        // are 3.00 s apart from 79.9 s to logoff; it would have run forever.
        "dungeonExplorationMgr" => match req.method {
            "GetMyEscalatingPathDetails" => Ok(crate::escalation::rowset(reg, req.actor)),
            _ => Err(reg.no_method(req)),
        },
        // The anomaly trackers, all eight the same shape: **a dict keyed by
        // solar system id**, values either a list of dungeon rows or a plain
        // count. The client states the empty itself -- one branch of
        // `anomalyTracker.py` literally reads `return {}`.
        //
        // Cosmetic, and said so plainly: `get_sites(45)` wraps the fetch in a
        // bare `except` that logs and substitutes `{}`. But `_unpack` at line
        // 60 is **outside** that try, and both its forms throw on `None` --
        // `instances.iteritems()` and `defaultdict(list, instances)` -- so
        // the exception escapes, leaves `self._fetch_time` set, and the next
        // `get_sites` spins `uthread2.Yield()` for up to five seconds before
        // clearing it. `_is_dirty` is cleared before the fetch, so there is
        // no refetch loop: bounded, invisible, and cheaper to answer than to
        // explain.
        //
        // Four arrived in one capture; the other four are the same overrides
        // of the same base method, answered together so the next window to
        // open does not produce a fifth log line.
        //
        // W14 CP29: the combat, ore and ice lists and counts and the
        // signature counts come from the live sites (`signal_tracker`'s
        // Agency half): rows for the caller's own system, ints elsewhere.
        // The other five have nothing that makes their sites yet.
        "dungeonInstanceCacheMgr" => match req.method {
            "GetCombatAnomalyInstances" => Ok(st::agency_instances(reg, req.actor, st::COMBAT)),
            "GetOreAnomalyInstances" => Ok(st::agency_instances(reg, req.actor, st::ORE)),
            "GetIceBeltInstances" => Ok(st::agency_instances(reg, req.actor, st::ICE)),
            // FW-6: rows only, never ints (`fw_instances`).
            "GetFactionWarfareInstances" => Ok(st::fw_instances(reg)),
            "GetBulwarkMiningSiteCounts"
            | "GetHomefrontSiteInstances"
            | "GetPirateInsurgencyInstances"
            | "GetTriglavianSiteInstances" => Ok(PyValue::Dict(vec![])),
            // **The unknown-site counts, and the 5 s stall** (lane B1).
            // `anomalyTracker.get_unknown_sites:215-221` fetches inside a
            // `try`, but `_unpack(sites_from_server)` at `:221` is outside
            // it, and `_unpack` does `.iteritems()`. With `None` the throw
            // skips the `_fetch_time_unknown = 0` reset at `:222`, and the
            // next reader spins `uthread2.Yield()` for up to 5 s (`:207-210`).
            // `{solarSystemID: count}` over every system (K4, CP29).
            "GetCombatAnomaliesCount" => Ok(st::agency_counts(reg, st::COMBAT)),
            "GetOreAnomaliesCount" => Ok(st::agency_counts(reg, st::ORE)),
            "GetIceBeltsCount" => Ok(st::agency_counts(reg, st::ICE)),
            // `get_count_in_range:185-187` stores it and returns `or {}`, so
            // `None` is survivable; `{}` is the same type and logs nothing.
            "GetOreAnomaliesCountInRange" => Ok(PyValue::Dict(vec![])),
            // `contentProviderSignatures._GetAllDungeonInstances:24` does
            // `.iteritems()` on it (an int value means "n unknown sites",
            // `:27-28`); `mapFilterSignatures:7` hands it to the analog map
            // filter as its data dict. Ints only (CP29): the client puts its
            // own system's sensor-suite sites in place of the count.
            "GetSignatureInstances" => Ok(st::agency_counts(reg, st::Want::Signatures)),
            // `warzoneMapPanel.ConstructBattlefieldMarkers:543` does
            // `.iteritems()` and reads `[0].dungeonNameID` per system
            // (`:548-553`): `{solarSystemID: [row, ...]}`.
            "GetFactionWarfareBattlefieldInstances" => Ok(PyValue::Dict(vec![])),
            // `pirateinsurgency/client/dashboard/map:124-125` is
            // `solarSystemID in heists and heists[solarSystemID]`: a dict
            // (C2 had the container as unknown).
            "GetPirateInsurgencyIceHeistInstances" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // The probe scanner / sensor overlay. Two methods in the whole
        // client tree -- `SignalTrackerRegister` and `GetSystemScanMgr`,
        // and nothing else: `grep -ra scanMgr out/pyc/` reaches only
        // `scanSvc.py`, `sensorSuiteService.py`, the server-side
        // `explorationscanner/signalTracker.py` and the two
        // `marshalstrings` tables.
        //
        // `GetSystemScanMgr` is not here. It returns an **object**, so it
        // is answered a layer up by `handle_object_returning_call`; its
        // methods are the `scanMgr::systemScanMgr` arm below.
        "scanMgr" => match req.method {
            // **The return is discarded** -- a bare expression statement
            // with `POP_TOP` -- so `None` is the answer rather than a
            // stand-in. Re-verified for this build at both of its two call
            // sites: `sensorSuiteService.OnEnterSpace` (a statement inside
            // the `try`, whose value never reaches a name) and
            // `eve/devtools/script/svc_slash.py`'s `register` subcommand,
            // likewise a bare statement. Nothing in the tree reads a
            // result from it, so `None` stays correct.
            //
            // **Do not reach for a raise here**, even though the call
            // site has an `except UserError`. That handler treats
            // `msg == 'UnMachoDestination'` as "the session changed,
            // bail", and would silently skip the three `LoadSites` calls
            // that follow -- bookmarks, missions and shared sites. A
            // raise would cost more than the gap did.
            //
            // W12 CP17: the registration is now real -- the caller is marked
            // new in its system and gets `OnSignalTrackerFullState` on the
            // next tick (`crate::signal_tracker`, CCP's `DoUpdateTick`).
            "SignalTrackerRegister" => Ok(crate::signal_tracker::register(reg, req.actor)),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These two tests span services S2 split across `sites`, `facwar`,
    /// `tales` and `space`, so they route the way the registry does, through
    /// `DOMAINS`, rather than through this module's `dispatch` alone. Their
    /// bodies are unchanged by the move.
    fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        reg.dispatch(req)
    }

    /// The two live locks this module answers, and why neither empty is a
    /// free choice.
    ///
    /// Neither of these ever produced a traceback: one stores our `None`
    /// successfully behind an `is None` guard and re-issues every 3 s
    /// forever, the other throws inside an `UNPACK_SEQUENCE` before its three
    /// assignments run. `evecap --triage` reported nothing for either. The
    /// repeat count in `--calls` was the only evidence, which is exactly what
    /// the live-lock section of `docs/guides/debugging-the-client.md` says to watch.
    #[test]
    fn the_journal_and_the_militia_cannot_spin() {
        let mut reg = Registry::new(SandboxConfig::default());
        let call = |reg: &mut Registry, service: &str, method: &str| {
            dispatch(
                reg,
                &Request {
                    service,
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            )
            .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
        };

        // Any non-`None` breaks the 3-second loop, but the shape is chosen
        // for the consumers: `IndexRowset(x.header, x.lines, 'instanceID')`
        // resolves the column before counting rows, so a bare `[]` would
        // stop the loop and still be wrong.
        let esc = call(
            &mut reg,
            "dungeonExplorationMgr",
            "GetMyEscalatingPathDetails",
        );
        let PyValue::Instance { state, .. } = &esc else {
            panic!("a Rowset instance, not a bare list: {esc:?}")
        };
        let header = crate::pyutil::dict_get(state, b"header").expect("a header");
        let PyValue::List(cols) = header else {
            panic!("header is a list of column names")
        };
        assert!(
            cols.contains(&bstr("instanceID")),
            "instanceID must be declared even with no rows; IndexRowset \
             resolves it before it looks at the row count"
        );

        // Three elements, unpacked before anything is tested. `None` here is
        // the LP live lock. A tuple of `None` is enough *because* the
        // client's sentinel is the string 'FACTION_ID_NOT_SET'.
        let enl = call(&mut reg, "fwCharacterEnlistmentMgr", "GetMyEnlistment");
        let PyValue::Tuple(parts) = &enl else {
            panic!("UNPACK_SEQUENCE 3 needs a sequence: {enl:?}")
        };
        assert_eq!(parts.len(), 3, "UNPACK_SEQUENCE 3, exactly");

        // Dicts, because `_unpack` runs outside the caller's `except`.
        assert_eq!(
            call(
                &mut reg,
                "dungeonInstanceCacheMgr",
                "GetOreAnomalyInstances"
            ),
            PyValue::Dict(vec![])
        );
        // A list, because the provider string-subscripts each element.
        assert_eq!(
            call(&mut reg, "taleMgr", "GetGlobalWorldEventTales"),
            PyValue::List(vec![])
        );
    }

    /// The sensor overlay's two calls, and the one that must be an object.
    ///
    /// `GetSystemScanMgr` is answered a layer up, so what is asserted here
    /// is the half this module owns: that the registry declares it
    /// object-returning at all (a data answer of any shape is an
    /// `AttributeError` on the client, because `scanSvc` only ever calls
    /// uppercase methods on the result), and that the object's
    /// `GetFullState` is the 4-tuple of dicts
    /// `sensorSuiteService.OnSignalTrackerFullState` unpacks.
    #[test]
    fn the_sensor_overlay_gets_an_object_and_a_four_tuple_of_dicts() {
        let mut reg = Registry::new(SandboxConfig::default());
        let call = |reg: &mut Registry, service: &str, method: &str| {
            dispatch(
                reg,
                &Request {
                    service,
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            )
        };

        // The object route, not a value. If this ever stops being declared,
        // `dispatch` falls through to the registry, finds no arm, and
        // answers `None` -- which is the bug this arm exists for.
        assert_eq!(
            Registry::object_returning("scanMgr", "GetSystemScanMgr"),
            Some("scanMgr::systemScanMgr"),
            "the client calls methods on the result; it cannot be data"
        );
        // ... and because it is answered there, it must *not* also have an
        // arm here, or we would be minting an object and computing a value
        // for the same call.
        assert!(
            matches!(
                call(&mut reg, "scanMgr", "GetSystemScanMgr"),
                Err(ServiceError::NoSuchMethod { .. })
            ),
            "GetSystemScanMgr belongs to handle_object_returning_call"
        );

        // `anomalies, signatures, staticSites, structures = fullState`, on
        // the line after the `LogInfo` and before anything is tested, so
        // the arity is load-bearing and `None` is a `TypeError` there.
        let full = call(&mut reg, "scanMgr::systemScanMgr", "GetFullState")
            .expect("GetFullState must be answered");
        let PyValue::Tuple(parts) = &full else {
            panic!("UNPACK_SEQUENCE 4 needs a sequence: {full:?}")
        };
        assert_eq!(parts.len(), 4, "UNPACK_SEQUENCE 4, exactly");
        for (i, part) in parts.iter().enumerate() {
            assert_eq!(
                part,
                &PyValue::Dict(vec![]),
                "element {i} is iterated with .iteritems() in \
                 spacesitecontroller.ProcessSiteDataUpdate, so it is a dict; \
                 a list only survives while it is empty"
            );
        }

        // The registration's return is discarded at both of its call sites,
        // so `None` is the answer and not a stand-in -- but it must still be
        // *answered*, because an unanswered call is a hang and this one sits
        // in front of the three `LoadSites` calls in `OnEnterSpace`.
        assert_eq!(
            call(&mut reg, "scanMgr", "SignalTrackerRegister").expect("answered"),
            PyValue::None
        );
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

    fn is_empty_dict(v: &PyValue) -> bool {
        matches!(v, PyValue::Dict(items) if items.is_empty())
    }

    /// A dict of `{int: int}`: what `_unpack` and the signature provider
    /// turn into `[None] * n` (CP29 fills them from the live sites).
    fn is_count_dict(v: &PyValue) -> bool {
        let int = |x: &PyValue| matches!(x, PyValue::Int(_));
        matches!(v, PyValue::Dict(items) if items.iter().all(|(n, s)| int(n) && int(s)))
    }

    /// `anomalyTracker.get_unknown_sites:221` runs `_unpack` (`.iteritems()`)
    /// outside its `try`, so these three must be dicts.
    #[test]
    fn unknown_site_counts_are_dicts() {
        for method in [
            "GetCombatAnomaliesCount",
            "GetIceBeltsCount",
            "GetOreAnomaliesCount",
        ] {
            assert!(
                is_count_dict(&call("dungeonInstanceCacheMgr", method)),
                "{method}"
            );
        }
    }

    /// `anomalyTracker.get_count_in_range:185-187` stores it as a dict.
    #[test]
    fn ore_count_in_range_is_a_dict() {
        assert!(is_empty_dict(&call(
            "dungeonInstanceCacheMgr",
            "GetOreAnomaliesCountInRange"
        )));
    }

    /// `contentProviderSignatures._GetAllDungeonInstances:24` `.iteritems()`.
    #[test]
    fn signature_instances_is_a_dict() {
        assert!(is_count_dict(&call(
            "dungeonInstanceCacheMgr",
            "GetSignatureInstances"
        )));
    }

    /// `warzoneMapPanel.ConstructBattlefieldMarkers:543` `.iteritems()`.
    #[test]
    fn fw_battlefield_instances_is_a_dict() {
        assert!(is_empty_dict(&call(
            "dungeonInstanceCacheMgr",
            "GetFactionWarfareBattlefieldInstances"
        )));
    }

    /// `pirateinsurgency/client/dashboard/map:124` is `in` plus a subscript.
    #[test]
    fn ice_heist_instances_is_a_dict() {
        assert!(is_empty_dict(&call(
            "dungeonInstanceCacheMgr",
            "GetPirateInsurgencyIceHeistInstances"
        )));
    }
}
