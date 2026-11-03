//! Factional warfare: the warzones, their headquarters, who holds each
//! system and its victory points (enlistment is `service/facwar_mgr.rs`,
//! W16 CP53a).
//!
//! W12 packet CP18 answers the warzone arms from `crate::facwar::Table`
//! (`content/facwar/warzones.tsv`) and fixes lane B1's three breakers
//! (`docs/content/R-FW.md` §0 items 2-4). W17 packet CP53b fills the
//! occupation dicts from `crate::facwar`'s systems and answers
//! `fwVictoryPointMgr` (R-FW §1's sibling remotes, §3).
//!
//! Split out of `service/space.rs` by W10.5 seam S2 (`docs/content/plan.md`
//! §6.1), so a lane working on factional warfare opens one file and cannot collide
//! with a lane working on flight. The arms and their tests moved verbatim.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
///
/// `fwCharacterEnlistmentMgr` moved to `service/facwar_mgr.rs` (W16 CP53a),
/// beside the enlistment rows its `GetMyEnlistment` reads.
pub(crate) const SERVICES: &[&str] = &["fwWarzoneSolarsystem", "fwVictoryPointMgr"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Factional warfare's occupation state: a **2-tuple whose first
        // element we do not get to choose**.
        //
        //     solarsystemID, occupationState = sm.RemoteSvc(
        //         'fwWarzoneSolarsystem').GetLocalOccupationState()
        //     if solarsystemID != session.solarsystemid2:
        //         solarsystemID, occupationState = <the same call again>
        //     if solarsystemID == session.solarsystemid2:
        //         reg._localOccupationSystemAndState = (...)
        //     else:
        //         raise RuntimeError(...)
        //
        // Three teeth, and all of them bite:
        //   1. `UNPACK_SEQUENCE 2` runs before any test, so `None` is a
        //      `TypeError`.
        //   2. the first element is compared against the live session, so
        //      `(None, None)` or `(0, None)` buys a **second** round trip
        //      and then raises anyway -- worse than useless.
        //   3. returning the matching id is what makes the call stop
        //      repeating. The client's own "unknown" constant is
        //      `(None, None)`, which can never match, which is why a
        //      capture shows this call three times: not a retry, a cache
        //      that cannot fill.
        //
        // The second element may be `None` -- `IsWarzoneSolarSystem` only
        // does `bool(occupationState)`, and the readers of `.occupierID`
        // guard first -- so `None` is the honest "not in a warzone".
        //
        // Left unanswered this removed **every info panel** from the
        // station screen, not just the FW one:
        // `_GetCurrentlyAvailableInfoPanelClasses` is a comprehension
        // calling `IsAvailable()` on each panel class, so one throw
        // aborts the whole reconstruct.
        "fwWarzoneSolarsystem" => match req.method {
            // Every warzone's occupation state, for the map's **default**
            // star filter -- `VIEWMODE_COLOR_DEFAULT` is
            // `STARMODE_FACTIONEMPIRE`, so this is the 100% path on a
            // first map open, not one filter among sixty.
            //
            // **A dict of dicts, keyed by every warzone** (CP18, R-FW §0
            // item 2 and §3). B1 answered `{}`, which this comment once
            // defended; it is a `KeyError` on the FW dashboard map, because
            // `mapData.GetWarzoneSystems:59` and `:123` subscript
            // `occupationStates[warzoneId]` unguarded. So the outer keys are
            // the table's warzones, and the inner dicts hold every warzone
            // system's `OccupationState` (CP53b, `facwar::all_occupation_states`).
            // `fwWarzoneSvc.GetAllOccupationStates` is a one-line
            // passthrough to us, and `mapViewData.py(116)` consumes it:
            //
            //     for occupationStatesBySolarsystem in \
            //             occupationStatesByWarzone.itervalues():      # 117
            //         for solarsystemID, occupationState in \
            //                 occupationStatesBySolarsystem.iteritems():  # 118
            //             allianceSolarSystems[solarsystemID] = \
            //                 occupationState.occupierID               # 119
            //
            // Outer dict keyed by warzone, inner keyed by solar system,
            // values carrying `.occupierID`. `None` is an `AttributeError`
            // on `.itervalues()`; `[]` would survive the outer loop by
            // accident and still be the wrong type.
            // `GetOccupationState` reads it the same way, which is a
            // second independent reading of the shape.
            //
            // **Land this with `stationSvc.GetAllianceSystems`, not after
            // it** -- they are lines 111 and 116 of one function.
            //
            // Empty inner dicts are also simply *true*: no system is
            // occupied yet, and the map falls back to the static empire
            // data in `res:/staticdata/starMapCache.pickle`. Unlike
            // `GetLocalOccupationState` below, this one is **not**
            // `@Memoize`d client-side, so it re-crosses the wire on every
            // filter construct.
            "GetAllWarzonesOccupationStates" => Ok(crate::facwar::all_occupation_states(reg)),
            // The same dict of dicts, uncached (`fwWarzoneSvc:113` is a
            // passthrough beside the cached one at `:105`).
            "GetAllWarzonesOccupationStatesUncached" => {
                Ok(crate::facwar::all_occupation_states(reg))
            }
            // **Must be non-empty** (lane B1). `fwWarzoneSvc
            // .GetWarzoneIdForFaction:203-207` looks the faction up in the
            // values (`factionID in teams`) and otherwise returns
            // `random.choice(allWarzones.keys())`, so `{}` is an `IndexError`
            // and `None` an `AttributeError` on `.iteritems()`. Its callers
            // are the dashboard (`fwWarzoneDashboard:139`), the enlistment
            // gauge (`warzoneGaugeCont:57`) and the Theaters of War landing
            // page (`landingPagePanel:156,162`).
            "GetAllWarzones" => Ok(table(reg).all_warzones()),
            // `fwWarzoneSvc.GetFactionHQSystem:211-212` is `in` then a
            // subscript, so `None` is a `TypeError`. **A missing faction is
            // not safe either**: the result reaches
            // `shipCasterVisualization:50` as `cfg.eveowners.Get(hq).name`
            // whenever a faction has no shipcaster, which with
            // `shipCasterLauncherMgr` answering `[]` is every faction. So
            // every faction in a warzone gets a real solar system.
            "GetHQSystemIDs" => Ok(table(reg).hq_system_ids()),
            // **The calling session's system** (R-FW §0 item 4). B1 answered
            // the first configured character's, which with two characters,
            // or after a jump, is the `RuntimeError` above. The state is the
            // system's `OccupationState`, or `None` outside the warzones.
            "GetLocalOccupationState" => {
                let system = session_system(reg, req.actor);
                let state = system.and_then(|s| crate::facwar::local_state(reg, s));
                Ok(PyValue::Tuple(vec![
                    system.map_or(PyValue::None, PyValue::Int),
                    state.unwrap_or(PyValue::None),
                ]))
            }
            _ => Err(reg.no_method(req)),
        },
        // R-FW §1's sibling remotes (CP53b). Both shapes are [V]:
        // `fwVictoryPointSvc:48,51` unpacks `GetLocalVictoryPointState` into
        // `(ssid, state)`, retrying once when `ssid` is not its own
        // `session.solarsystemid2`, so element 0 is the caller's system as in
        // `GetLocalOccupationState`; the state is `None` outside the
        // warzones (`infoPanelLocationInfo:642` asks only inside one).
        // `GetAllVictoryPointStates` (`:59`) is read with `.get`.
        "fwVictoryPointMgr" => match req.method {
            "GetLocalVictoryPointState" => {
                let system = session_system(reg, req.actor);
                let state = system.and_then(|s| crate::facwar::vp_state(reg, s));
                Ok(PyValue::Tuple(vec![
                    system.map_or(PyValue::None, PyValue::Int),
                    state.unwrap_or(PyValue::None),
                ]))
            }
            "GetAllVictoryPointStates" => Ok(crate::facwar::all_vp_states(reg)),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The warzone table (`content/facwar/warzones.tsv`, CP18).
fn table(reg: &Registry) -> &crate::facwar::Table {
    reg.content().get::<crate::facwar::Table>()
}

/// `None` with no actor, which the client's own "unknown" also is.
fn session_system(reg: &Registry, actor: Option<i64>) -> Option<i64> {
    reg.session_system(actor?)
}

/// Lane B1, the breaker sweep: one test per arm, each asserting the type the
/// consumer pins (`docs/lanes/breaker-sweep.md`). [A] values are not asserted.
#[cfg(test)]
mod breaker_sweep_tests {
    use super::*;

    fn call_as(reg: &mut Registry, actor: Option<i64>, service: &str, method: &str) -> PyValue {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .unwrap_or_else(|e| panic!("{service}.{method} must be answered: {e:?}"))
    }

    fn call(service: &str, method: &str) -> PyValue {
        call_as(
            &mut Registry::new(SandboxConfig::default()),
            None,
            service,
            method,
        )
    }

    /// R-FW §0 item 2: `mapData.GetWarzoneSystems:59,123` subscripts
    /// `occupationStates[warzoneId]`, so both the cached and the uncached
    /// (`fwWarzoneSvc:113`) answers hold a dict under every warzone key.
    #[test]
    fn occupation_states_hold_every_warzone_key() {
        let reg = Registry::new(SandboxConfig::default());
        let ids: Vec<i64> = table(&reg).warzones().iter().map(|w| w.id).collect();
        assert!(!ids.is_empty());
        for method in [
            "GetAllWarzonesOccupationStates",
            "GetAllWarzonesOccupationStatesUncached",
        ] {
            let got = call("fwWarzoneSolarsystem", method);
            let kv = got.dict_as_kv().expect("a dict: .itervalues()");
            for id in &ids {
                assert!(
                    kv.iter()
                        .any(|(k, v)| **k == PyValue::Int(*id) && matches!(v, PyValue::Dict(_))),
                    "{method}: warzone {id} missing: {got:?}"
                );
            }
        }
    }

    /// R-FW §0 item 4, `fwWarzoneSvc:74-79`: element 0 is compared with the
    /// caller's own `session.solarsystemid2`, so it is the caller's system --
    /// not the first character's, and the roster's once they are in space.
    #[test]
    fn local_occupation_state_is_the_session_system() {
        const A: i64 = 90_000_001;
        const B: i64 = 90_000_002;
        let pilot = |id: i64, sys: i64| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: id + 100,
            solar_system_id: Some(sys),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![pilot(A, 39_000_011), pilot(B, 39_000_012)],
            ..SandboxConfig::default()
        });
        let first = |v: PyValue| match v {
            PyValue::Tuple(t) if t.len() == 2 => t[0].clone(),
            other => panic!("UNPACK_SEQUENCE 2: {other:?}"),
        };
        let local = |reg: &mut Registry, who| {
            first(call_as(
                reg,
                Some(who),
                "fwWarzoneSolarsystem",
                "GetLocalOccupationState",
            ))
        };
        assert_eq!(local(&mut reg, B), PyValue::Int(39_000_012));
        assert_eq!(local(&mut reg, A), PyValue::Int(39_000_011));
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                B,
                B + 100,
                39_000_013,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        assert_eq!(local(&mut reg, B), PyValue::Int(39_000_013), "jumped");
    }

    /// CP18 retires B1's warzone and HQ consts; the table is the one source.
    #[test]
    fn b1_consts_are_gone() {
        let src = include_str!("facwar.rs");
        // Spelled in halves so this test does not match itself.
        for gone in [
            concat!("const ", "WARZONES:"),
            concat!("const ", "HQ_SYSTEMS:"),
            concat!("const ", "FACTION_"),
            concat!("fn ", "all_warzones()"),
        ] {
            assert!(!src.contains(gone), "{gone}");
        }
    }

    /// `fwWarzoneSvc.GetWarzoneIdForFaction:203-207`: `.iteritems()`, then
    /// `factionID in teams`, then `random.choice(keys())`, so a **non-empty**
    /// dict of int keys to lists of faction ids. Which ids is [A]/[I] and is
    /// not asserted.
    #[test]
    fn all_warzones_is_a_non_empty_dict_of_faction_lists() {
        let got = call("fwWarzoneSolarsystem", "GetAllWarzones");
        let kv = got.dict_as_kv().expect("a dict: .iteritems() at :204");
        assert!(!kv.is_empty(), "{{}} is an IndexError at :207");
        for (k, v) in kv {
            assert!(matches!(k, PyValue::Int(_)), "warzone ids are ints: {k:?}");
            match v {
                PyValue::List(teams) => {
                    assert!(!teams.is_empty());
                    assert!(teams.iter().all(|t| matches!(t, PyValue::Int(_))));
                }
                other => panic!("`factionID in teams` wants a list: {other:?}"),
            }
        }
    }

    /// `fwWarzoneSvc.GetFactionHQSystem:211-212` (`in`, then subscript), and
    /// `shipCasterVisualization:50` needs a real id for every warzone
    /// faction. The ids are [A] and not asserted, only that each faction in
    /// `GetAllWarzones` has one.
    #[test]
    fn hq_system_ids_cover_every_warzone_faction() {
        let hqs = call("fwWarzoneSolarsystem", "GetHQSystemIDs");
        let hqs = hqs.dict_as_kv().expect("a dict: `in` at :211");
        let warzones = call("fwWarzoneSolarsystem", "GetAllWarzones");
        for (_, teams) in warzones.dict_as_kv().expect("a dict") {
            let PyValue::List(teams) = teams else {
                panic!("{teams:?}")
            };
            for faction in teams {
                let hq = hqs
                    .iter()
                    .find(|(k, _)| *k == faction)
                    .unwrap_or_else(|| panic!("{faction:?} has no HQ"));
                assert!(matches!(hq.1, PyValue::Int(_)), "{hq:?}");
            }
        }
    }
}
