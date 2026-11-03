//! Sovereignty over the wire (SOC-D1): `sovMgr`, `devIndexManager`, the sov
//! arms other services hand over, and the `/sov` GM command. The ledger,
//! the rows and the pushes are `crate::sov`; the client reading is
//! `docs/lanes/sov-d1.md` §2.
//!
//! **Arms that live in other services' files** are one line there, calling
//! a function here:
//!
//! - `stationSvc.GetAllianceSystems` and `map.GetRecentSovActivity`
//!   (`service/world.rs`) -> [`alliance_systems`], [`recent_activity`];
//! - `beyonce.GetCynoJammerState` (`service/space.rs`) -> [`cyno_jammer_state`];
//! - `allianceRegistry` (`service/alliance.rs`) asks [`alliance_registry`]
//!   first, as it asks SOC-C2's contacts.

use super::*;
use crate::sov;

/// The services dispatched here.
pub(crate) const SERVICES: &[&str] = &["sovMgr", "devIndexManager"];

fn int_arg(req: &Request<'_>, i: usize) -> Option<i64> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

/// The caller's own system: `session.solarsystemid2`, which is the
/// character's system docked or not.
fn local_system(reg: &Registry, req: &Request<'_>) -> Option<i64> {
    req.actor.and_then(|id| reg.session_system(id))
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match (req.service, req.method) {
        // `sovSvc.GetSystemSovereigntyInfo` caches the answer **for ever**
        // per system, until `ProcessSovStatusChanged` replaces it **[V]**;
        // FW systems never get here (the client answers them itself).
        ("sovMgr", "GetSystemSovereigntyInfo") => {
            let c = int_arg(req, 0).and_then(|s| sov::claim(reg.deltas(), s));
            Ok(sov::claim_info(c.as_ref()))
        }
        // 15-second memo; the hub menus and `structureDeployment` gate on
        // it being non-`None` **[V]**.
        ("sovMgr", "GetInfrastructureHubInfo") => {
            let c = int_arg(req, 0).and_then(|s| sov::claim(reg.deltas(), s));
            Ok(sov::hub_info(c.as_ref()))
        }
        // The Show Info sovereignty tab, the constellation panel and the
        // dashboard (30-second memo) **[V]**.
        ("sovMgr", "GetSovStructuresInfoForSolarSystem") => {
            Ok(int_arg(req, 0).map_or(PyValue::List(vec![]), |s| structure_rows(reg, s)))
        }
        // **Unserved before D1**: `infoPanelLocationInfo.UpdateSOVLocationInfo`
        // asks it in every claimable system, through `sovSvc`'s memo, when
        // the system is `session.solarsystemid2` **[V]**.
        ("sovMgr", "GetSovStructuresInfoForLocalSolarSystem") => {
            Ok(local_system(reg, req).map_or(PyValue::List(vec![]), |s| structure_rows(reg, s)))
        }
        // SOC-D4: the indices (`crate::sov_index`). `{}` for an unclaimed
        // system reads as "no index" to every `.get` in
        // `sovSvc.GetIndexInfoForSolarsystem`, which the location panel calls
        // in every claimable system **[V]**.
        ("devIndexManager", "GetDevelopmentIndicesForSystem") => Ok(int_arg(req, 0)
            .map_or(PyValue::Dict(vec![]), |s| {
                crate::sov_index::dev_indices(reg, s)
            })),
        ("devIndexManager", "GetAllDevelopmentIndices") => {
            Ok(crate::sov_index::all_dev_indices(reg))
        }
        // SOC-D2: the Equinox fuel windows' access-group trio.
        (
            "sovMgr",
            "GetSovHubFuelAccessGroup"
            | "SetSovHubFuelAccessGroup"
            | "IsOnLocalSovHubFuelAccessGroup",
        ) => super::sov_hub::fuel_access(reg, req),
        _ => Err(reg.no_method(req)),
    }
}

/// A system's structure rows now, on the world's clock (the ADM's
/// strategic index counts game time).
fn structure_rows(reg: &Registry, system: i64) -> PyValue {
    sov::structure_rows_at(
        reg.deltas(),
        system,
        crate::pyutil::filetime_now(),
        crate::game_clock::of(reg),
    )
}

/// The acting character is a director (or the CEO) of `alliance`'s
/// executor corp: who may set the prime hour and the capital.
fn executor_director(reg: &Registry, actor: Option<i64>, alliance: Option<i64>) -> bool {
    alliance
        .and_then(|a| {
            crate::alliance::Alliances::load(reg.deltas())
                .get(a)
                .and_then(|x| x.executor_corp_id)
        })
        .is_some_and(|exec| is_director_of(reg, actor, exec))
}

/// `stationSvc.GetAllianceSystems()`: the map's star colours.
pub(crate) fn alliance_systems(reg: &Registry) -> PyValue {
    sov::alliance_systems(reg.deltas())
}

/// `map.GetRecentSovActivity()`: the map's sov-changes filter.
pub(crate) fn recent_activity(reg: &Registry) -> PyValue {
    sov::recent_activity_rows(reg.deltas(), crate::pyutil::filetime_now())
}

/// `beyonce.GetCynoJammerState()`: `None` is "not jammed"
/// (`cynoJammerIcon.UpdateIconState`) **[V]**. **Unserved before D1**: the
/// location panel draws the icon in every claimable system, whose
/// `_UpdateFromServer` asks this unbound (`sm.RemoteSvc('beyonce')`) **[V]**.
/// There are no cyno jammers.
pub(crate) fn cyno_jammer_state() -> PyValue {
    PyValue::None
}

/// The acting character is a director (or the CEO) of `corp`.
fn is_director_of(reg: &Registry, actor: Option<i64>, corp: i64) -> bool {
    let Some(me) = actor else { return false };
    let Some(m) = reg.corporations.membership(me) else {
        return false;
    };
    m.corporation_id == corp
        && (m.roles & corp::CORP_ROLE_DIRECTOR != 0
            || reg.corporations.get(corp).is_some_and(|c| c.ceo_id == me))
}

/// The sov arms of `allianceRegistry`, given the alliance the call is about
/// (the moniker's bind, else the caller's). `None` for any other method.
pub(crate) fn alliance_registry(
    reg: &mut Registry,
    req: &Request<'_>,
    alliance: Option<i64>,
) -> Option<Result<PyValue, ServiceError>> {
    let now = crate::pyutil::filetime_now();
    Some(match req.method {
        // `sovSvc.GetSovereigntyStructuresInfoForAlliance` unpacks three
        // **[V]**: the Claimed Systems tab, the dashboard, the Sov Hubs page.
        "GetAllianceSovereigntyStructuresInfo" => Ok(match alliance {
            Some(a) => {
                sov::alliance_structures_at(reg.deltas(), a, now, crate::game_clock::of(reg))
            }
            None => sov::alliance_structures(&crate::delta::Deltas::default(), 0),
        }),
        // `all_ui_sovereignty.UpdatePrimeInfo`; `None` reads as "Unknown".
        "GetPrimeTimeInfo" => Ok(alliance.map_or(PyValue::None, |a| {
            sov::prime_time_info(&sov::prime_hour(reg.deltas(), a, now))
        })),
        // The window offers the menu only to a director of the executor corp
        // (`_IsDirectorInExecCorp`) **[V]**; the same rule here.
        "SetPrimeHour" => {
            let hour = int_arg(req, 0);
            let executor = alliance.and_then(|a| {
                crate::alliance::Alliances::load(reg.deltas())
                    .get(a)
                    .and_then(|x| x.executor_corp_id)
            });
            let (Some(a), Some(exec), Some(hour)) = (alliance, executor, hour) else {
                return Some(Err(ServiceError::BadArguments {
                    service: req.service.to_owned(),
                    method: req.method.to_owned(),
                    why: "an alliance and an hour".to_owned(),
                }));
            };
            if !is_director_of(reg, req.actor, exec) {
                return Some(Err(ServiceError::Raise(eve_proto::user_error(
                    "CrpAccessDenied",
                    crate::pyutil::pypairs(vec![(
                        "reason",
                        PyValue::Unicode(
                            "Only a director of the executor corporation can set this.".to_owned(),
                        ),
                    )]),
                ))));
            }
            let delay = crate::game_clock::span(reg, sov::CHANGE_PRIMETIME_DELAY);
            match sov::set_prime_hour(reg.deltas_mut(), a, hour, now, delay) {
                Ok(_) => Ok(PyValue::None),
                Err(why) => Err(ServiceError::BadArguments {
                    service: req.service.to_owned(),
                    method: req.method.to_owned(),
                    why: why.to_owned(),
                }),
            }
        }
        // SOC-D4: the capital (`crate::sov_index`). The Claimed Systems tab
        // asks on every reload (`GetMyCapitalSystem`) **[V]**.
        "GetCapitalSystemInfo" => Ok(crate::sov_index::capital_system_info(reg, alliance)),
        "SetCapitalSystem" | "CancelCapitalSystemTransition" => {
            let allowed = executor_director(reg, req.actor, alliance);
            return crate::sov_index::capital_call(reg, req.method, req.args, alliance, allowed);
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// /sov
// ---------------------------------------------------------------------------

const USAGE: &str = "usage: /sov set <systemID|here> <allianceID|shortName|none> | /sov clear <systemID|here> | /sov capture <systemID|here> <allianceID|shortName> | /sov info [systemID|here] | /sov index [systemID|here] [military industrial] | /sov capital <systemID|here> [now] | /sov entosis [systemID|here] | /sov window [systemID|here] | /sov reinforce <systemID|here> [now] | /sov resolve <systemID|here> attackers|defenders | /sov prime <allianceID|shortName> <hour> | /sov nodes [systemID|here] | /sov nodes spawn [systemID|here] | /sov nodes capture <nodeID> attackers|defenders";

fn parse_system(reg: &Registry, actor: Option<i64>, word: &str) -> Result<i64, String> {
    if word == "here" {
        return actor
            .and_then(|id| reg.session_system(id))
            .ok_or_else(|| "you are not in a system".to_owned());
    }
    word.parse::<i64>()
        .map_err(|_| format!("'{word}' is not a system id"))
}

/// An alliance by id or short name, with its executor corp.
fn parse_alliance(reg: &Registry, word: &str) -> Result<(i64, i64), String> {
    let st = crate::alliance::Alliances::load(reg.deltas());
    let found = match word.parse::<i64>() {
        Ok(id) => st.get(id),
        Err(_) => st
            .all()
            .find(|a| !a.deleted && a.short_name.eq_ignore_ascii_case(word)),
    };
    let a = found
        .filter(|a| !a.deleted)
        .ok_or_else(|| format!("no open alliance '{word}'"))?;
    let exec = a
        .executor_corp_id
        .ok_or_else(|| format!("alliance {} has no executor corp", a.alliance_id))?;
    Ok((a.alliance_id, exec))
}

fn info_text(reg: &Registry, system: i64) -> String {
    let claimable = reg
        .content()
        .try_get::<crate::celestial::Universe>()
        .is_some_and(|u| sov::claimable(u, system));
    match sov::claim(reg.deltas(), system) {
        None => format!("system {system}: no hub (claimable: {claimable})"),
        Some(c) => format!(
            "system {system}: hub {} {} owner corp {} alliance {} claimed at {} (claimable: {claimable})",
            c.hub_id,
            c.state.as_str(),
            c.corporation_id,
            c.alliance_id
                .map_or_else(|| "none".to_owned(), |a| a.to_string()),
            c.claim_time
                .map_or_else(|| "-".to_owned(), |t| t.to_string()),
        ),
    }
}

/// Set or clear a claim, announce it, and say what happened.
fn apply(reg: &mut Registry, system: i64, owner: Option<(i64, i64)>) -> String {
    let now = crate::pyutil::filetime_now();
    // SOC-D2: a claim stands on a real hub; one is made where none was
    // launched (the stand-in id only when the type table lacks the hub).
    if owner.is_some() {
        crate::sov_hub::ensure_hub(reg, system);
    }
    match sov::set_claim(reg.deltas_mut(), system, owner, now) {
        Some(change) => {
            sov::announce(reg, &change);
            crate::sov_hub::claim_changed(reg, system);
            format!("{} (was alliance {:?})", info_text(reg, system), change.old)
        }
        None => format!("{} (no change)", info_text(reg, system)),
    }
}

/// `/sov set <system> <alliance|none>`, `/sov clear <system>`, `/sov info
/// [system]`: claims for tests and setup (R-SOV §9 D1). A set skips the hub
/// launch and the capture (D2, D3) and gives the system to the alliance's
/// executor corp; only a claimable system takes one.
pub(crate) fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let text = match args {
        ["set", system, alliance] => match parse_system(reg, actor, system) {
            Err(e) => format!("{USAGE} -- {e}"),
            Ok(system) if alliance.eq_ignore_ascii_case("none") => apply(reg, system, None),
            Ok(system) => {
                let claimable = reg
                    .content()
                    .try_get::<crate::celestial::Universe>()
                    .is_some_and(|u| sov::claimable(u, system));
                if !claimable {
                    format!(
                        "system {system} is not claimable (null-sec, no faction, not a wormhole)"
                    )
                } else {
                    match parse_alliance(reg, alliance) {
                        Ok(owner) => apply(reg, system, Some(owner)),
                        Err(e) => format!("{USAGE} -- {e}"),
                    }
                }
            }
        },
        // SOC-D2: what D3a's entosis completion will do to a neutral hub.
        ["capture" | "claim", system, alliance] => {
            match (
                parse_system(reg, actor, system),
                parse_alliance(reg, alliance),
            ) {
                (Ok(system), Ok(owner)) => match crate::sov_hub::capture(reg, system, owner) {
                    Ok(change) => format!(
                        "{} (captured; was alliance {:?})",
                        info_text(reg, system),
                        change.old
                    ),
                    Err(e) => e,
                },
                (Err(e), _) | (_, Err(e)) => format!("{USAGE} -- {e}"),
            }
        }
        ["clear", system] => match parse_system(reg, actor, system) {
            Ok(system) => apply(reg, system, None),
            Err(e) => format!("{USAGE} -- {e}"),
        },
        ["info"] => match parse_system(reg, actor, "here") {
            Ok(system) => info_text(reg, system),
            Err(e) => format!("{USAGE} -- {e}"),
        },
        ["info", system] => match parse_system(reg, actor, system) {
            Ok(system) => info_text(reg, system),
            Err(e) => format!("{USAGE} -- {e}"),
        },
        // SOC-D3a: `/sov entosis|window|reinforce|resolve ...`.
        [word, ..] if crate::entosis::SLASH_WORDS.contains(word) => {
            crate::entosis::slash(reg, actor, args)
        }
        // SOC-D4: `/sov index ...`, `/sov capital ...`.
        [word, ..] if crate::sov_index::SLASH_WORDS.contains(word) => {
            crate::sov_index::slash(reg, actor, args)
        }
        _ => USAGE.to_owned(),
    };
    Ok(bstr(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alliance::Alliances;

    // Invented ids; the system is whichever one the fixture universe offers
    // as claimable, so no fixture id is written here.
    const CEO: i64 = 90_200_001;
    const CORP: i64 = 98_200_001;

    fn text(v: &PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => format!("{other:?}"),
        }
    }

    fn world() -> (Registry, i64, i64) {
        world_with(Vec::new())
    }

    fn world_with(characters: Vec<crate::model::Character>) -> (Registry, i64, i64) {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters,
            ..crate::model::SandboxConfig::default()
        });
        let system = reg
            .content()
            .get::<crate::celestial::Universe>()
            .systems()
            .map(|s| s.id)
            .find(|id| sov::claimable(reg.content().get::<crate::celestial::Universe>(), *id))
            .expect("the fixture universe has a claimable system");
        let mut st = Alliances::load(reg.deltas());
        let alliance = st
            .create(CORP, CEO, "Invented Sov Pact", "ISOV", "", "", 1)
            .expect("created");
        st.save(reg.deltas_mut());
        (reg, system, alliance)
    }

    fn slash_text(reg: &mut Registry, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        text(&slash(reg, None, &words).expect("answered"))
    }

    #[test]
    fn sov_slash_set_round_trips_through_the_delta() {
        let (mut reg, system, alliance) = world();
        let out = slash_text(&mut reg, &format!("set {system} {alliance}"));
        assert!(out.contains(&alliance.to_string()), "{out}");
        let row = reg
            .deltas()
            .of(sov::KIND)
            .get(&vec![system.to_string()])
            .cloned()
            .expect("a sov row");
        assert_eq!(row[0], alliance.to_string());
        assert_eq!(row[1], CORP.to_string());
        // Through a world save and load: the rows are plain strings.
        let mut copy = crate::delta::Deltas::default();
        for (kind, rows) in reg.deltas().kinds() {
            for (k, v) in rows {
                copy.put_verbatim(kind, k.clone(), v.clone());
            }
        }
        assert_eq!(sov::holder(&copy, system), Some(alliance));
        assert_eq!(sov::claims_of(&copy, alliance).len(), 1);
        // By short name, again: no change. Then cleared.
        let out = slash_text(&mut reg, &format!("set {system} isov"));
        assert!(out.contains("no change"), "{out}");
        slash_text(&mut reg, &format!("clear {system}"));
        assert_eq!(sov::holder(reg.deltas(), system), None);
        assert_eq!(sov::recent_activity(reg.deltas(), i64::MAX / 2).len(), 0);
        assert_eq!(
            sov::recent_activity(reg.deltas(), crate::pyutil::filetime_now()).len(),
            2
        );
    }

    #[test]
    fn sov_slash_refuses_an_unclaimable_system_and_an_unknown_alliance() {
        let (mut reg, system, alliance) = world();
        let out = slash_text(&mut reg, &format!("set 1 {alliance}"));
        assert!(out.contains("not claimable"), "{out}");
        let out = slash_text(&mut reg, &format!("set {system} 99100999"));
        assert!(out.contains("no open alliance"), "{out}");
        assert!(reg.deltas().of(sov::KIND).is_empty());
        assert!(slash_text(&mut reg, "frobnicate").starts_with("usage"));
    }

    #[test]
    fn sov_mgr_and_station_svc_answer_from_the_ledger() {
        let (mut reg, system, alliance) = world();
        let call =
            |reg: &mut Registry, service: &'static str, method: &'static str, args: &[PyValue]| {
                reg.dispatch(&Request {
                    service,
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                })
                .expect("answered")
            };
        let ssid = [PyValue::Int(system)];
        assert_eq!(
            call(&mut reg, "sovMgr", "GetSystemSovereigntyInfo", &ssid),
            PyValue::None
        );
        assert_eq!(
            call(
                &mut reg,
                "sovMgr",
                "GetSovStructuresInfoForSolarSystem",
                &ssid
            ),
            PyValue::List(vec![])
        );
        assert_eq!(
            call(
                &mut reg,
                "sovMgr",
                "GetSovStructuresInfoForLocalSolarSystem",
                &[]
            ),
            PyValue::List(vec![])
        );
        assert_eq!(
            call(
                &mut reg,
                "devIndexManager",
                "GetDevelopmentIndicesForSystem",
                &ssid
            ),
            PyValue::Dict(vec![])
        );
        slash_text(&mut reg, &format!("set {system} {alliance}"));
        assert!(matches!(
            call(&mut reg, "sovMgr", "GetSystemSovereigntyInfo", &ssid),
            PyValue::ObjectEx { .. }
        ));
        assert!(matches!(
            call(&mut reg, "sovMgr", "GetInfrastructureHubInfo", &ssid),
            PyValue::ObjectEx { .. }
        ));
        let PyValue::List(rows) = call(&mut reg, "stationSvc", "GetAllianceSystems", &[]) else {
            panic!()
        };
        assert_eq!(rows.len(), 1);
        let PyValue::List(rows) = call(&mut reg, "map", "GetRecentSovActivity", &[]) else {
            panic!()
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            call(&mut reg, "beyonce", "GetCynoJammerState", &[]),
            PyValue::None
        );
    }

    #[test]
    fn local_structures_follow_a_pilot_in_space() {
        use crate::space_state::{Motion, Presence};
        const PILOT: i64 = 90_200_050;
        const ELSEWHERE: i64 = 30_900_777;
        let (mut reg, system, alliance) = {
            // Docked (last) in a system other than the claimed one.
            let (_, system, _) = world();
            world_with(vec![crate::model::Character {
                character_id: PILOT,
                name: "pilot".to_owned(),
                ship_id: PILOT + 1_000,
                solar_system_id: Some(system + 1),
                ..crate::model::Character::default()
            }])
        };
        slash_text(&mut reg, &format!("set {system} {alliance}"));
        let local = |reg: &mut Registry| {
            reg.dispatch(&Request {
                service: "sovMgr",
                method: "GetSovStructuresInfoForLocalSolarSystem",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(PILOT),
            })
            .expect("answered")
        };
        let arrive = |reg: &mut Registry, at: i64| {
            reg.roster_mut().arrive(Presence::arriving(
                PILOT,
                PILOT + 1_000,
                at,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        };
        // In space in the claimed system: one row naming the alliance.
        arrive(&mut reg, system);
        let PyValue::List(rows) = local(&mut reg) else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 1);
        assert!(format!("{rows:?}").contains(&alliance.to_string()));
        // In space elsewhere the claimed docked system no longer counts.
        arrive(&mut reg, ELSEWHERE);
        assert_eq!(local(&mut reg), PyValue::List(vec![]));
    }

    #[test]
    fn session_system_is_roster_then_station() {
        use crate::space_state::{Motion, Presence};
        const DOCKED: i64 = 90_200_051;
        const NOWHERE: i64 = 90_200_052;
        const DOCKED_AT: i64 = 30_900_801;
        const SPACE_AT: i64 = 30_900_802;
        let chr = |id: i64, at: Option<i64>| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: id + 1_000,
            solar_system_id: at,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![chr(DOCKED, Some(DOCKED_AT)), chr(NOWHERE, None)],
            ..crate::model::SandboxConfig::default()
        });
        assert_eq!(reg.session_system(DOCKED), Some(DOCKED_AT));
        assert_eq!(reg.session_system(NOWHERE), None);
        reg.roster_mut().arrive(Presence::arriving(
            DOCKED,
            DOCKED + 1_000,
            SPACE_AT,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert_eq!(reg.session_system(DOCKED), Some(SPACE_AT));
    }

    #[test]
    fn slash_here_reads_the_system_a_pilot_in_space_is_in() {
        use crate::space_state::{Motion, Presence};
        const PILOT: i64 = 90_200_060;
        let (mut reg, system, alliance) = {
            let (_, system, _) = world();
            world_with(vec![crate::model::Character {
                character_id: PILOT,
                name: "pilot".to_owned(),
                ship_id: PILOT + 1_000,
                solar_system_id: Some(system + 1),
                ..crate::model::Character::default()
            }])
        };
        reg.roster_mut().arrive(Presence::arriving(
            PILOT,
            PILOT + 1_000,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let say = |reg: &mut Registry, line: &str| {
            let words: Vec<&str> = line.split_whitespace().collect();
            text(&slash(reg, Some(PILOT), &words).expect("answered"))
        };
        say(&mut reg, &format!("set here {alliance}"));
        assert!(sov::claim(reg.deltas(), system).is_some(), "set here");
        assert!(sov::claim(reg.deltas(), system + 1).is_none());
        for line in [
            "info here",
            "index here",
            "nodes here",
            "entosis here",
            "window here",
        ] {
            let out = say(&mut reg, line);
            assert!(!out.contains("not in a system"), "{line}: {out}");
        }
    }
}
