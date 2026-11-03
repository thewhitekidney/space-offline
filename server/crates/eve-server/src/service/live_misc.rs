//! The breaker sweep's unrouted services: typed empties for calls whose
//! consumer throws on `None`.
//!
//! Every service here was **unrouted** before lane B1, so the dispatcher
//! answered `None` with an `UNANSWERED` line, and each consumer below then
//! raised on it: `set(None)`, `None['minOutput']`, `len(None)`, `x in None`,
//! `for x in None`. None of them is content. Each arm is the empty value of
//! the exact type its consumer pins, read from the client's bytecode
//! (`tools/dis.py`, `co_lnotab` lines), and each has a test that names that
//! consumer. The survey that found them is `docs/content/C2-live-surface.md`
//! (Proposal 4); the park note is `docs/lanes/breaker-sweep.md`.
//!
//! `solarsystemInterferenceMgr.GetLocalInterferenceState` is answered by
//! [`local_interference_state`] (CP27, C2 packet K7): a quiescent `0x23`
//! `SolarsystemInterferenceState` where the client could show interference.
//!
//! Deliberately **not** answered here, so the `UNANSWERED` line keeps saying
//! so:
//!
//! - nothing now: `GetESSAgencyData` and `GetESSSystemDetails` are `crate::ess`'s
//!   (ESS-2, issue #90).

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "activityMgr",
    "dynamicResourceCacheMgr",
    "solarsystemInterferenceMgr",
    "rewardMgr",
    "custom_agency_provider",
    "shipCasterLauncherMgr",
    "shipCasterLandingPadMgr",
    "structureAssetSafety",
];

/// The dynamic-resource (ESS and DBS) settings dict.
///
/// **Every value is [A]**: the keys are [V], read from the client's
/// subscripts, and the numbers are invented to be plausible and safe.
/// *Falsified by* the real constants. Two of them carry an invariant from
/// the bytecode, which is the only reason they are not arbitrary:
///
/// - `maxOutput` must differ from `minOutput`, because
///   `mapFilterDynamicBounty:25` divides by `maxVal - minVal`;
/// - `minOutput <= equilibriumValue <= maxOutput`, because `:28` and
///   `GetESSBountyColor` treat it as a point inside that range.
///
/// The keys and their readers (all subscripts, so a missing key is a
/// `KeyError`):
///
/// | key | reader |
/// |---|---|
/// | `minOutput`, `maxOutput`, `equilibriumValue` | `mapFilterDynamicBounty:19-21`, `mapViewColorHandler:265-266`, `essFiltersCont:20-21`, `essContentCard:168-170` |
/// | `timeIntervalSeconds` | `essBountiesOutputTooltip:17` |
/// | `bountiesCut` | `essGameplayTooltip:17` |
/// | `autopaymentIntervalSeconds`, `bufferTimeSeconds`, `mainBankPayRatio` | `essMainBankTooltip:17-19` |
/// | `reserveBankMaxPayout`, `reserveBankTimeAccessFactor` | `essReserveBankTooltip:17-18`, `essSystemInfoContainer:53` |
/// | `earlyPayoutThresholdFactor`, `latePayoutThresholdFactor` | `essSystemInfoContainer:242,250` |
/// | `reservePulseIntervalSeconds` | `dynamicresources/client/ess/settings._update_from_cache_data:50` |
fn dynamic_resource_settings() -> PyValue {
    pydict(vec![
        ("minOutput", PyValue::Real(0.0)),
        ("maxOutput", PyValue::Real(2.0)),
        ("equilibriumValue", PyValue::Real(1.0)),
        ("timeIntervalSeconds", PyValue::Int(3600)),
        // The four ESS figures are `ess`'s, so the tooltip and the bank agree.
        ("bountiesCut", PyValue::Real(crate::ess::BOUNTIES_CUT)),
        (
            "autopaymentIntervalSeconds",
            PyValue::Int(crate::ess::AUTOPAY_INTERVAL_SECS),
        ),
        (
            "bufferTimeSeconds",
            PyValue::Int(crate::ess::BUFFER_TIME_SECS),
        ),
        (
            "mainBankPayRatio",
            PyValue::Real(crate::ess::MAIN_BANK_PAY_RATIO),
        ),
        ("reserveBankMaxPayout", PyValue::Real(0.0)),
        ("reserveBankTimeAccessFactor", PyValue::Real(1.0)),
        ("earlyPayoutThresholdFactor", PyValue::Real(0.25)),
        ("latePayoutThresholdFactor", PyValue::Real(0.25)),
        ("reservePulseIntervalSeconds", PyValue::Int(60)),
    ])
}

/// The client's `SolarsystemInterferenceState` class, in `globalsWhitelist`
/// [V] (`eve-rowset/src/whitelist.rs`). Its `__module__` is the defining file,
/// `solarsysteminterference/solarsystemInterferenceState.py`, not the package
/// that re-exports it.
const INTERFERENCE_STATE_CLASS: &str =
    "solarsysteminterference.solarsystemInterferenceState.SolarsystemInterferenceState";

/// The quiescent interference level. **[A]** 0.0, C2 §6. *Falsified by* a
/// real state whose `_quiescentInterferenceLevel` differs.
const QUIESCENT_LEVEL: f64 = 0.0;
/// The maximum level. **[A]** 1.0, C2 §6. `__init__:25` requires it to be
/// greater than [`QUIESCENT_LEVEL`] [V], and `normalisedInterferenceLevel`
/// (`:96`) divides by the difference.
const MAX_LEVEL: f64 = 1.0;
/// The decay per tick. **[A]** 0.01, C2 §6. `__init__:21` requires `> 0` [V].
const DECREASE_PER_TICK: f64 = 0.01;
/// The decay tick in **seconds**: `_GetDecayResult:120` multiplies it by
/// `SEC` before dividing a FILETIME delta by it [V]. **[A]** 60, C2 §6.
/// `__init__:23` requires `> 0` [V].
const DECAY_TICK_PERIOD_S: i64 = 60;

/// `inventorycommon/const.py:47-57` [V]: the system-id ranges
/// `idCheckers.Is{Wormhole,AbyssalSpace,VoidSpace,HiddenSpace}System`
/// (`:93`, `:108`, `:135`, `:139`) test, both ends inclusive.
const NO_INTERFERENCE_RANGES: [std::ops::RangeInclusive<i64>; 4] = [
    31_000_000..=31_999_999, // mapWormholeSystemMin..Max
    32_000_000..=32_999_999, // mapAbyssalSystemMin..Max
    34_000_000..=34_999_999, // mapVoidSystemMin..Max
    36_000_000..=36_999_999, // mapHiddenSystemMin..Max
];

/// `appConst.factionTriglavian` [V] (`eve/common/lib/appConst.py:769`).
const FACTION_TRIGLAVIAN: i64 = 500_026;

/// `solarsysteminterference/util.SystemCanHaveInterference:7-20`, transcribed
/// [V] over the universe table: not high-sec or above
/// (`is_high_sec_or_higher_solar_system`, `SecurityClassFromLevel` of the raw
/// `securityStatus`, `eveuniverse/security.py:47-52,68`), not wormhole,
/// abyssal, void or hidden space by id, and not Triglavian
/// (`idCheckers.IsTriglavianSystem:151-154`, the system's `factionID`).
///
/// `None` for a system the table lacks: the security test needs the row.
fn system_can_have_interference(reg: &Registry, system: i64) -> Option<bool> {
    let row = reg
        .content()
        .get::<crate::celestial::Universe>()
        .system(system)?;
    let high = crate::celestial::SecurityClass::from_level(row.security)
        >= crate::celestial::SecurityClass::High;
    let by_id = NO_INTERFERENCE_RANGES.iter().any(|r| r.contains(&system));
    let triglavian = row.faction_id == Some(FACTION_TRIGLAVIAN);
    Some(!(high || by_id || triglavian))
}

/// `None` with no actor, which the client's own "unknown" also is.
fn session_system(reg: &Registry, actor: Option<i64>) -> Option<i64> {
    reg.session_system(actor?)
}

/// The quiescent `SolarsystemInterferenceState` for the caller's system.
///
/// **The state.** The six keys are the instance `__dict__` that
/// `__init__:32-37` assigns [V]. `__init__` never runs on the client
/// ([`crate::pyutil::new_style`]), so this upholds its validators
/// (`:21-30`): decrease and period `> 0`, max `>` quiescent, and
/// quiescent `<=` level `<=` max. The level *is* the quiescent level, so
/// `CalculateStateAtTime:143-144` returns the state unchanged and the decay
/// fields are never read; `normalisedInterferenceLevel` (`:96`) is 0 and
/// `interferenceBand` (`:101-102`) is `INTERFERENCE_BAND_NONE`.
///
/// **Where.** The consumers that read `.normalisedInterferenceLevel` without
/// a `None` guard are the location tooltip (`infoPanelLocationInfo:482`)
/// and nothing else; it and the indicator
/// (`interference_indicator._RefreshInterferenceLevel:59-62`, guarded) are
/// shown only where `SystemCanHaveInterference`. So:
///
/// - a system that can have interference: the state;
/// - one that cannot: `None`. **[A]** that the real server has no state
///   there. Harmless either way: `GetLocalInterferenceStateNow:40` and
///   `GetSolarSystemInterferenceBand:55` guard `is not None` and fall back
///   to the bands (`{}`, so "none"). *Falsified by* a capture showing a
///   state in high-sec.
/// - no actor, or a system the table lacks (no bundle): the state, because
///   `None` there would break the tooltip if the system is low or null.
fn local_interference_state(reg: &Registry, system: Option<i64>) -> PyValue {
    if system.and_then(|s| system_can_have_interference(reg, s)) == Some(false) {
        return PyValue::None;
    }
    crate::pyutil::new_style(
        INTERFERENCE_STATE_CLASS,
        vec![
            ("_interferenceLevel", PyValue::Real(QUIESCENT_LEVEL)),
            (
                "_lastDecayTickTimestamp",
                PyValue::Long(crate::pyutil::filetime_now()),
            ),
            (
                "_interferenceDecreasePerDecayTick",
                PyValue::Real(DECREASE_PER_TICK),
            ),
            ("_decayTickPeriod", PyValue::Int(DECAY_TICK_PERIOD_S)),
            ("_maxInterferenceLevel", PyValue::Real(MAX_LEVEL)),
            (
                "_quiescentInterferenceLevel",
                PyValue::Real(QUIESCENT_LEVEL),
            ),
        ],
    )
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // The "new activities" news feed (`activitiesSvc`). All four methods
        // go through `_GetRemoteActivitiesService:37`.
        // GAP-STATIONSVC. `assetSafety.py:50-51` iterates both [V] (the
        // Assets window's Asset Safety tab, the corp Accounts tab); no
        // structure exists, so nothing is in safety. The other methods of
        // these services are structure-only and stay unanswered.
        "structureAssetSafety" => match req.method {
            "GetItemsInSafetyForCharacter" | "GetItemsInSafetyForCorp" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        "activityMgr" => match req.method {
            // `LoadActivityStatus:45-46` is `set(statuses)`, so `None` is a
            // `TypeError` and the empty is a list. The elements are the
            // activity ids already seen (`OnActivitySeen:141` adds to it).
            "GetActivityBadgeStatuses" => Ok(PyValue::List(vec![])),
            // `OnActivitySeen:138-141`: a falsy answer returns early before
            // the id is recorded locally, so `True` is what makes the red dot
            // go away. Nothing is persisted: the next login shows it again.
            "ActivitySeen" => Ok(PyValue::Bool(true)),
            // `CheckIfLoginOpenWindowEnabled:125` toggles the window open when
            // this `== 0`, so **`1` means "do not auto-pop"** [V]. That path
            // is gated on `AmOnChineseServer` (`:119`), so it is also moot.
            "GetActivitiesPopState" => Ok(PyValue::Int(1)),
            // `OnStatusChanged:167-169`: a falsy answer reverts the checkbox.
            "SetActivitiesPopState" => Ok(PyValue::Bool(true)),
            _ => Err(reg.no_method(req)),
        },
        // ESS and dynamic-bounty data. Reached with the ESS flagged off: the
        // map's dynamic-bounty filter and the Agency's ESS tooltips call it
        // directly, not through the `essMgr` flag check.
        "dynamicResourceCacheMgr" => match req.method {
            "GetDynamicResourceSettings" => Ok(dynamic_resource_settings()),
            // `mapFilterDynamicBounty:23-24` iterates it and subscripts each
            // key back out: `{solarSystemID: float}`. Empty = no star sized.
            "GetDBSMapData" => Ok(PyValue::Dict(vec![])),
            // `contentProviderESS:44` does `.iteritems()`:
            // `{solarSystemID: {...}}`. Empty = no ESS content pieces.
            "GetESSAgencyData" => Ok(crate::ess::agency_data(reg, crate::pyutil::filetime_now())),
            // `essSystemInfoContainer:45-53`: five keys, from `crate::ess`.
            "GetESSSystemDetails" => {
                let system = match req.args.first() {
                    Some(PyValue::Int(n)) => *n,
                    _ => 0,
                };
                Ok(crate::ess::system_details(
                    reg,
                    system,
                    crate::pyutil::filetime_now(),
                ))
            }
            // `dynamicResourceSvc.GetESSReserveBankKeysForCurrentSystem:199`
            // iterates it as typeIDs.
            "GetESSReserveBankKeys" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // Solar-system interference. Nothing generates interference yet, so
        // every system is at its quiescent level and in band "none".
        "solarsystemInterferenceMgr" => match req.method {
            "GetLocalInterferenceState" => Ok(local_interference_state(
                reg,
                session_system(reg, req.actor),
            )),
            // `solarsystemInterferenceSvc.GetSolarSystemInterferenceBand:60-61`
            // is `if solarsystemID in bands: return bands[solarsystemID]`,
            // falling back to `INTERFERENCE_BAND_NONE` (`:64`). So `{}` is
            // "no interference anywhere", and `None` is a `TypeError`.
            "GetAllInterferenceBands" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // Incursion rewards.
        "rewardMgr" => match req.method {
            // `lpIncursionsPanel:112` stores it and `UpdateIncursionLPLog:150`
            // iterates it (`journal:1124` is the same path).
            "GetRewardLPLogs" => Ok(PyValue::List(vec![])),
            // `incursionSvc.GetDelayedRewards:315` calls `.get(groupID)` on it.
            "GetDelayedRewardsByGroupIDs" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // Pirate strongholds and other server-authored Agency content.
        // `agency/client/custom_content.get_custom_content:16-17` does
        // `len(content_data)` before anything else, and `:23` iterates it.
        "custom_agency_provider" => match req.method {
            "get_content_data" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // **Not in C2's list**: the factional-warfare dashboard reaches these
        // as soon as `GetAllWarzones` stops throwing, so they are the next
        // breakers on the same screen.
        "shipCasterLauncherMgr" => match req.method {
            // `warzoneMapPanel:488` is `factionID in factionsWithShipcaster`
            // and `shipCasterVisualization:129` is `not in`: a list of
            // faction ids. Empty = no faction has built one, which takes the
            // "not built yet" view.
            "GetFactionsWithShipcaster" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        "shipCasterLandingPadMgr" => match req.method {
            // `shipCasterVisualization:123` is `sorted(landingPads, key=...)`.
            "GetFactionLandingPads" => Ok(PyValue::List(vec![])),
            // `warzoneMapPanel:527` iterates it, reading `.factionID`.
            "GetAllLandingPads" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(service: &str, method: &str) -> PyValue {
        let mut reg = Registry::new(SandboxConfig::default());
        dispatch(
            &mut reg,
            &Request {
                service,
                method,
                args: &[PyValue::Int(30_000_001)],
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

    fn is_empty_dict(v: &PyValue) -> bool {
        matches!(v, PyValue::Dict(items) if items.is_empty())
    }

    /// GAP-STATIONSVC: the structure stubs are typed empties.
    #[test]
    fn structure_stubs_answer_lists() {
        for (s, m) in [
            ("structureAssetSafety", "GetItemsInSafetyForCharacter"),
            ("structureAssetSafety", "GetItemsInSafetyForCorp"),
        ] {
            assert!(is_empty_list(&call(s, m)), "{s}.{m}");
        }
    }

    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`: `NoSuchMethod` means we got to the service.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!("{s} is advertised here but answers {other:?}"),
            }
        }
    }

    /// `activitiesSvc.LoadActivityStatus:46` is `set(statuses)`.
    #[test]
    fn activity_badge_statuses_is_a_list_for_set() {
        assert!(is_empty_list(&call(
            "activityMgr",
            "GetActivityBadgeStatuses"
        )));
    }

    /// `activitiesSvc.OnActivitySeen:139` returns early on a falsy answer.
    #[test]
    fn activity_seen_is_true() {
        assert_eq!(call("activityMgr", "ActivitySeen"), PyValue::Bool(true));
    }

    /// `activitiesSvc.CheckIfLoginOpenWindowEnabled:125` auto-opens on `== 0`.
    #[test]
    fn activities_pop_state_is_a_nonzero_int() {
        assert!(matches!(
            call("activityMgr", "GetActivitiesPopState"),
            PyValue::Int(n) if n != 0
        ));
    }

    /// `activitiesSvc.OnStatusChanged:168` reverts the checkbox on falsy.
    #[test]
    fn set_activities_pop_state_is_true() {
        assert_eq!(
            call("activityMgr", "SetActivitiesPopState"),
            PyValue::Bool(true)
        );
    }

    /// Every key some client reader subscripts must be present, and the two
    /// invariants the bytecode imposes must hold. The values themselves are
    /// [A] and deliberately not asserted.
    ///
    /// Readers: `mapFilterDynamicBounty:19-21,25`, `essBountiesOutputTooltip:17`,
    /// `essGameplayTooltip:17`, `essMainBankTooltip:17-19`,
    /// `essReserveBankTooltip:17-18`, `essSystemInfoContainer:53,242,250`,
    /// `ess/settings:50`.
    #[test]
    fn dynamic_resource_settings_is_a_dict_with_every_subscripted_key() {
        let got = call("dynamicResourceCacheMgr", "GetDynamicResourceSettings");
        let kv = got
            .dict_as_kv()
            .expect("a dict: every reader subscripts it");
        let get = |key: &str| {
            kv.iter()
                .find(|(k, _)| **k == PyValue::Bytes(key.as_bytes().to_vec()))
                .map(|(_, v)| (*v).clone())
                .unwrap_or_else(|| panic!("{key} is subscripted by a reader and must be present"))
        };
        for key in [
            "minOutput",
            "maxOutput",
            "equilibriumValue",
            "timeIntervalSeconds",
            "bountiesCut",
            "autopaymentIntervalSeconds",
            "bufferTimeSeconds",
            "mainBankPayRatio",
            "reserveBankMaxPayout",
            "reserveBankTimeAccessFactor",
            "earlyPayoutThresholdFactor",
            "latePayoutThresholdFactor",
            "reservePulseIntervalSeconds",
        ] {
            let v = get(key);
            assert!(
                matches!(v, PyValue::Int(_) | PyValue::Real(_)),
                "{key} feeds arithmetic or a timedelta and must be a number: {v:?}"
            );
        }
        let num = |key: &str| match get(key) {
            PyValue::Real(f) => f,
            PyValue::Int(i) => i as f64,
            other => panic!("{key}: {other:?}"),
        };
        let (lo, hi, eq) = (num("minOutput"), num("maxOutput"), num("equilibriumValue"));
        assert!(lo < hi, "mapFilterDynamicBounty:25 divides by max - min");
        assert!(
            lo <= eq && eq <= hi,
            "the equilibrium sits inside the range"
        );
    }

    /// `mapFilterDynamicBounty:23-24` iterates and subscripts it.
    #[test]
    fn dbs_map_data_is_a_dict() {
        assert!(is_empty_dict(&call(
            "dynamicResourceCacheMgr",
            "GetDBSMapData"
        )));
    }

    /// `contentProviderESS:44` does `.iteritems()`.
    #[test]
    fn ess_agency_data_is_a_dict() {
        assert!(matches!(
            call("dynamicResourceCacheMgr", "GetESSAgencyData"),
            PyValue::Dict(_)
        ));
    }

    /// `dynamicResourceSvc.GetESSReserveBankKeysForCurrentSystem:199` iterates.
    #[test]
    fn ess_reserve_bank_keys_is_a_list() {
        assert!(is_empty_list(&call(
            "dynamicResourceCacheMgr",
            "GetESSReserveBankKeys"
        )));
    }

    /// `solarsystemInterferenceSvc.GetSolarSystemInterferenceBand:60` is `in`.
    #[test]
    fn interference_bands_is_a_dict() {
        assert!(is_empty_dict(&call(
            "solarsystemInterferenceMgr",
            "GetAllInterferenceBands"
        )));
    }

    /// An invented pilot for the interference tests.
    const PILOT: i64 = 90_000_001;

    /// `GetLocalInterferenceState` for [`PILOT`] standing in `system` (on
    /// `universe` when given, else the fixture content's universe).
    fn local_state_in(system: i64, universe: Option<crate::celestial::Universe>) -> PyValue {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: PILOT,
                name: "pilot".into(),
                ..crate::model::Character::default()
            }],
            ..SandboxConfig::default()
        });
        if let Some(u) = universe {
            let mut content = crate::content::Content::fixture();
            content.insert("universe", u);
            reg = reg.with_content(std::sync::Arc::new(content));
        }
        reg.character_mut(PILOT).expect("the pilot").solar_system_id = Some(system);
        dispatch(
            &mut reg,
            &Request {
                service: "solarsystemInterferenceMgr",
                method: "GetLocalInterferenceState",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(PILOT),
            },
        )
        .expect("GetLocalInterferenceState is answered")
    }

    /// The fixture universe's first system in `band`.
    fn fixture_system(band: crate::celestial::Band) -> i64 {
        crate::celestial::Universe::fixture()
            .systems()
            .find(|s| s.band() == band)
            .map(|s| s.id)
            .unwrap_or_else(|| panic!("the fixture has a {band:?} system"))
    }

    /// A number out of the state dict, by key.
    fn state_num(state: &PyValue, key: &str) -> f64 {
        match crate::pyutil::dict_get(state, key.as_bytes()) {
            Some(PyValue::Real(f)) => *f,
            Some(PyValue::Int(i)) => *i as f64,
            other => panic!("{key} must be a number: {other:?}"),
        }
    }

    /// The answer in a null-sec system is a `0x23` instance of the
    /// whitelisted class whose `__dict__` is exactly what `__init__:32-37`
    /// assigns, and which upholds every validator `__init__:21-30` would
    /// have run [V]. It is at its quiescent level, so `CalculateStateAtTime`
    /// returns it as is (`:143-144`) and `normalisedInterferenceLevel`
    /// (`:96`, read by `infoPanelLocationInfo:482`) divides by a non-zero
    /// range.
    #[test]
    fn the_interference_state_keys_satisfy_the_class_invariants() {
        let got = local_state_in(fixture_system(crate::celestial::Band::Null), None);
        let PyValue::ObjectEx {
            form,
            header,
            list,
            dict,
        } = &got
        else {
            panic!("a 0x23 instance, not {got:?}");
        };
        assert_eq!(*form, eve_marshal::ObjectForm::New);
        assert!(list.is_empty() && dict.is_empty());
        let PyValue::Tuple(parts) = &**header else {
            panic!("{header:?}")
        };
        assert_eq!(
            parts[0],
            PyValue::Tuple(vec![PyValue::Global(
                INTERFERENCE_STATE_CLASS.as_bytes().to_vec()
            )])
        );
        assert!(eve_rowset::is_whitelisted(
            INTERFERENCE_STATE_CLASS.as_bytes()
        ));
        let state = &parts[1];
        let mut keys: Vec<Vec<u8>> = state
            .dict_as_kv()
            .expect("the state is a dict")
            .iter()
            .map(|(k, _)| match k {
                PyValue::Bytes(b) => b.clone(),
                other => panic!("a byte-string key, not {other:?}"),
            })
            .collect();
        keys.sort();
        let mut want: Vec<Vec<u8>> = [
            "_interferenceLevel",
            "_lastDecayTickTimestamp",
            "_interferenceDecreasePerDecayTick",
            "_decayTickPeriod",
            "_maxInterferenceLevel",
            "_quiescentInterferenceLevel",
        ]
        .iter()
        .map(|k| k.as_bytes().to_vec())
        .collect();
        want.sort();
        assert_eq!(keys, want, "__init__:32-37 assigns exactly these");

        let n = |k| state_num(state, k);
        assert!(n("_interferenceDecreasePerDecayTick") > 0.0, "__init__:21");
        assert!(n("_decayTickPeriod") > 0.0, "__init__:23");
        assert!(
            n("_maxInterferenceLevel") > n("_quiescentInterferenceLevel"),
            "__init__:25"
        );
        assert!(
            n("_interferenceLevel") <= n("_maxInterferenceLevel"),
            "__init__:27"
        );
        assert!(
            n("_interferenceLevel") >= n("_quiescentInterferenceLevel"),
            "__init__:29"
        );
        assert!(
            n("_interferenceLevel") <= n("_quiescentInterferenceLevel"),
            "isAtQuiescentLevel (:86): nothing generates interference yet"
        );
        assert!(
            matches!(
                crate::pyutil::dict_get(state, b"_lastDecayTickTimestamp"),
                Some(PyValue::Long(_))
            ),
            "_GetDecayResult:116 subtracts it from a FILETIME"
        );
    }

    /// `SystemCanHaveInterference:7-20` [V], over the universe table: low
    /// and null sec get the state, high sec does not. (Wormhole space is by
    /// id, below.)
    #[test]
    fn the_state_follows_the_tables_security() {
        use crate::celestial::Band;
        for band in [Band::Low, Band::Null] {
            assert!(
                matches!(
                    local_state_in(fixture_system(band), None),
                    PyValue::ObjectEx { .. }
                ),
                "{band:?}: the tooltip reads it unguarded"
            );
        }
        assert_eq!(
            local_state_in(fixture_system(Band::High), None),
            PyValue::None,
            "high sec: SystemCanHaveInterference is false"
        );
    }

    /// `IsTriglavianSystem:151-153` is the system's `factionID` [V]: an
    /// invented null-sec row owned by the Triglavians has no interference.
    #[test]
    fn a_triglavian_system_has_no_interference_state() {
        const SYS: i64 = 39_000_101;
        let row = |faction: &str| {
            format!("{SYS}\t19000001\t29000001\t{faction}\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx")
        };
        let universe = |faction: &str| {
            crate::celestial::Universe::parse(&[row(faction)], &[], &[]).expect("a valid row")
        };
        assert_eq!(
            local_state_in(SYS, Some(universe(&FACTION_TRIGLAVIAN.to_string()))),
            PyValue::None
        );
        assert!(matches!(
            local_state_in(SYS, Some(universe(""))),
            PyValue::ObjectEx { .. }
        ));
    }

    /// The client's own id ranges, not the table, rule out wormhole, abyssal,
    /// void and hidden space [V]; a system the table lacks still gets the
    /// state, so a server with no bundle never breaks the tooltip.
    #[test]
    fn id_ranges_rule_out_space_without_interference() {
        let reg = Registry::new(SandboxConfig::default());
        let fixture = crate::celestial::Universe::fixture();
        let missing = (1..)
            .map(|n| 30_000_000 + n)
            .find(|id| fixture.system(*id).is_none())
            .expect("an id outside the fixture");
        assert_eq!(system_can_have_interference(&reg, missing), None);
        assert!(matches!(
            local_state_in(missing, None),
            PyValue::ObjectEx { .. }
        ));
        for r in &NO_INTERFERENCE_RANGES {
            let id = *r.start();
            let row = format!("{id}\t19000001\t29000001\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx");
            let u = crate::celestial::Universe::parse(&[row], &[], &[]).expect("a valid row");
            assert_eq!(local_state_in(id, Some(u)), PyValue::None, "{id}");
        }
    }

    /// `lpIncursionsPanel.UpdateIncursionLPLog:150` iterates it.
    #[test]
    fn reward_lp_logs_is_a_list() {
        assert!(is_empty_list(&call("rewardMgr", "GetRewardLPLogs")));
    }

    /// `incursionSvc.GetDelayedRewards:315` calls `.get` on it.
    #[test]
    fn delayed_rewards_is_a_dict() {
        assert!(is_empty_dict(&call(
            "rewardMgr",
            "GetDelayedRewardsByGroupIDs"
        )));
    }

    /// `agency/client/custom_content.get_custom_content:17` is `len(...)`.
    #[test]
    fn custom_agency_content_is_a_list() {
        assert!(is_empty_list(&call(
            "custom_agency_provider",
            "get_content_data"
        )));
    }

    /// `warzoneMapPanel:488` is `in`, `shipCasterVisualization:129` `not in`.
    #[test]
    fn factions_with_shipcaster_is_a_list() {
        assert!(is_empty_list(&call(
            "shipCasterLauncherMgr",
            "GetFactionsWithShipcaster"
        )));
    }

    /// `shipCasterVisualization:123` sorts it; `warzoneMapPanel:527` iterates.
    #[test]
    fn landing_pads_are_lists() {
        for method in ["GetFactionLandingPads", "GetAllLandingPads"] {
            assert!(
                is_empty_list(&call("shipCasterLandingPadMgr", method)),
                "{method}"
            );
        }
    }
}
