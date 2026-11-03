//! The Sovereignty Hub over the wire (SOC-D2): `ship.Drop` ("Launch for
//! Corporation"), the three fuel-access-group arms of `sovMgr`, and the
//! fuel console lines `/sovfuel` (GM) and `/boostsov` (anyone). The model
//! is `crate::sov_hub`; the lane note is `docs/lanes/sov-d2.md`.
//!
//! **Arms that live in other services' files** are one line there, calling
//! a function here (coordinator rows):
//!
//! - `ship.Drop` (`service/inventory.rs`'s `ship` moniker) -> [`drop_call`];
//! - `sovMgr.{Get,Set}SovHubFuelAccessGroup` and
//!   `IsOnLocalSovHubFuelAccessGroup` (D1's `service/sov.rs`) ->
//!   [`fuel_access`];
//! - `publicQaToolsServer.SlashCmd` (`service/gm.rs`) -> [`public_qa`];
//! - the `slash::COMMANDS` row `sovfuel` -> [`sovfuel`].

use super::*;
use crate::sov;
use crate::sov_hub;

/// Whether `actor` may launch a hub for their corp: a director, the CEO,
/// or a station manager **[A]** (the role `OfferOpenSovHubConfigWindow`
/// asks for, `session.IsStationManager` **[V]**; R-SOV §6.2 step 1).
/// Falsified by retail letting any member launch. Here because the corp
/// book is the service layer's.
pub(crate) fn may_launch(reg: &Registry, actor: i64) -> bool {
    let Some(m) = reg.corporations.membership(actor) else {
        return false;
    };
    m.roles & (corp::CORP_ROLE_DIRECTOR | sov_hub::CORP_ROLE_STATION_MANAGER) != 0
        || reg
            .corporations
            .get(m.corporation_id)
            .is_some_and(|c| c.ceo_id == actor)
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// `ship.Drop(oldItems, whoseBehalfID, ignoreWarning)`'s arm, on
/// `Moniker('ship', (solarsystemid, groupSolarSystem))`: `oldItems` is
/// `[(itemID, qty), ...]` (`eveMisc.LaunchFromShip`) **[V]**.
/// `whoseBehalfID` is `None` for "Launch for Corporation" and the
/// character for "Launch for Self" **[V]** (`menusvc`, `DeployAction`);
/// the hub's menu offers only the first (`OfferLaunchForSelf` is false for
/// a sov structure **[V]**), so it is not read.
pub(crate) fn drop_call(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(PyValue::List(v) | PyValue::Tuple(v)) = req.args.first() else {
        return Err(bad_args(
            req,
            "([(itemID, qty)], whoseBehalfID, ignoreWarning)",
        ));
    };
    let items: Vec<(i64, i64)> = v
        .iter()
        .filter_map(|e| match e {
            PyValue::Tuple(p) | PyValue::List(p) => match (p.first(), p.get(1)) {
                (Some(PyValue::Int(id)), Some(PyValue::Int(q))) => Some((*id, *q)),
                (Some(PyValue::Int(id)), _) => Some((*id, 1)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Ok(sov_hub::launch(reg, req.actor, &items))
}

/// The alliance holding the caller's own system, if any.
fn local_holder(reg: &Registry, actor: Option<i64>) -> Option<i64> {
    let system = actor.and_then(|id| reg.session_system(id))?;
    sov::holder(reg.deltas(), system)
}

/// `sovMgr`'s fuel-access-group trio, read only by the Equinox hub windows,
/// which wait on the gateway first (R-SOV §3) **[V]** (`sovSvc`):
///
/// - `GetSovHubFuelAccessGroup(solarSystemID)` -> `None`: no access list;
/// - `SetSovHubFuelAccessGroup(solarSystemID, groupID)` -> `None`, kept
///   nowhere **[A]** (access lists are gateway data). Falsified by a window
///   that reads the group back and finds it gone;
/// - `IsOnLocalSovHubFuelAccessGroup()` -> whether the caller is in the
///   alliance that holds their system **[A]**, the same rule as
///   `/sovfuel`'s. Its four callers test it for truth **[V]**
///   (`findings/client-calls.tsv`).
pub(crate) fn fuel_access(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "GetSovHubFuelAccessGroup" | "SetSovHubFuelAccessGroup" => Ok(PyValue::None),
        "IsOnLocalSovHubFuelAccessGroup" => {
            let mine = req
                .actor
                .and_then(|id| reg.character(id))
                .and_then(|c| c.alliance_id);
            let holder = local_holder(reg, req.actor);
            Ok(PyValue::Bool(mine.is_some() && mine == holder))
        }
        _ => Err(reg.no_method(req)),
    }
}

const USAGE: &str =
    "usage: /sovfuel [status [systemID|here]] | /sovfuel <blocks|all> | /sovfuel burn <days>";
const PLAYER_USAGE: &str = "usage: /boostsov [status] | /boostsov <blocks|all>";

/// What a refuel says back.
fn refuel_text(r: &sov_hub::Refuelled, reg: &Registry, system: i64) -> String {
    format!(
        "moved {} fuel blocks into hub {}{}. {}",
        r.moved,
        r.hub_id,
        if r.online_again {
            " -- it is online again"
        } else {
            ""
        },
        sov_hub::status_text(reg, system)
    )
}

/// The caller's system, docked or not.
fn here(reg: &Registry, actor: Option<i64>) -> Option<i64> {
    actor.and_then(|id| reg.session_system(id))
}

/// The words both console lines share: `status`, a block count or `all`.
/// `None` for a grammar this does not know.
fn fuel_words(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> Option<String> {
    let qty = match args {
        [] | ["status"] => {
            return Some(here(reg, actor).map_or_else(
                || "you are not in a system".to_owned(),
                |s| sov_hub::status_text(reg, s),
            ));
        }
        ["all"] => None,
        [n] => match n.replace([',', '_'], "").parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => return None,
        },
        _ => return None,
    };
    let Some(actor) = actor else {
        return Some("no character".to_owned());
    };
    Some(match sov_hub::refuel(reg, actor, qty) {
        Ok(r) => {
            let system = here(reg, Some(actor)).unwrap_or(0);
            refuel_text(&r, reg, system)
        }
        Err(e) => e,
    })
}

/// **`/sovfuel`** (a `slash::COMMANDS` row; the GM console): `/sovfuel` or
/// `/sovfuel status [system|here]` prints a hub's fuel; `/sovfuel <n>` or
/// `/sovfuel all` moves blocks from the hold into the local hub, the
/// player rule (R-SOV §6.7); `/sovfuel burn <days>` runs the rollover's fuel
/// step now for `days` days, for tests and human checks.
pub(crate) fn sovfuel(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[&str],
) -> Result<PyValue, ServiceError> {
    let text = match args {
        ["status", "here"] => fuel_words(reg, actor, &["status"]).unwrap_or_default(),
        ["status", system] => match system.parse::<i64>() {
            Ok(s) => sov_hub::status_text(reg, s),
            Err(_) => format!("{USAGE} -- '{system}' is not a system id"),
        },
        ["burn", days] => match days.parse::<u32>() {
            Ok(d) if d > 0 => {
                let burned = sov_hub::burn_now(reg, d);
                let status = here(reg, actor)
                    .map(|s| sov_hub::status_text(reg, s))
                    .unwrap_or_default();
                format!(
                    "burned {burned} fuel blocks over {d} game days across every claimed hub. {status}"
                )
            }
            _ => format!("{USAGE} -- days is a whole number above zero"),
        },
        _ => fuel_words(reg, actor, args).unwrap_or_else(|| USAGE.to_owned()),
    };
    Ok(bstr(&text))
}

/// `OnRemoteMessage('CustomNotify', {'notify': text})` to one pilot: the
/// only way back for a `/boostsov`, whose call returns nothing the client
/// reads (`PublicQaToolsClient.SlashCmd` has no `return`) **[V]**. `[A]`
/// the pushed form shows the text, as `shutdown::ShutdownMessage` assumes.
#[derive(Debug, Clone)]
pub(crate) struct FuelMessage {
    pub to: i64,
    pub text: String,
}

impl crate::notify::Push for FuelMessage {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRemoteMessage",
            vec![
                bstr("CustomNotify"),
                crate::pyutil::pydict(vec![("notify", bstr(&self.text))]),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// **`/boostsov`, the players' fuel line.** The client sends any chat line
/// starting with one of `publicQaToolsClient.allowedSlashCommands` to
/// `publicQaToolsServer.SlashCmd(line)` with **no role check**, before its
/// `ROLE_SLASH` branch (`chat.client.slash_command.handle_slash_command`)
/// **[V]**; `/boostsov` is on that list **[V]** (the pyc's constants). So
/// a pilot without the GM console types `/boostsov 500` in any chat window
/// and the blocks go in, under the same rule as `/sovfuel`. The answer is
/// a `CustomNotify` push ([`FuelMessage`]); the call itself answers `None`,
/// as every other line of that service does.
///
/// Every other line keeps the service's old answer, `None`: this arm
/// grants nothing a player could not do by hand.
pub(crate) fn public_qa(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let line = match req.args.first() {
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        Some(PyValue::Unicode(u)) => u.clone(),
        _ => String::new(),
    };
    let mut words = line.split_whitespace();
    if !words
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("/boostsov"))
    {
        return Ok(PyValue::None);
    }
    let args: Vec<&str> = words.collect();
    let text = fuel_words(reg, req.actor, &args).unwrap_or_else(|| PLAYER_USAGE.to_owned());
    if let Some(to) = req.actor {
        reg.push(FuelMessage { to, text });
    }
    Ok(PyValue::None)
}

/// **Tests only**: a player corp `corp` with `ceo` at its head, and each
/// of `members` in it with their roles, written as the world file's own
/// records (`corp::Corporations::parse_record`). For the model's tests in
/// `crate::sov_hub`, which cannot reach the corp book.
#[cfg(test)]
pub(crate) fn test_corp(reg: &mut Registry, corp: i64, ceo: i64, members: &[(i64, i64)]) {
    let corp_s = corp.to_string();
    let ceo_s = ceo.to_string();
    let line = [
        "corp",
        &corp_s,
        &ceo_s,
        &ceo_s,
        "0",
        "0.1",
        "0.1",
        "0",
        "0",
        "-1",
        "-1",
        "-1",
        "-1",
        "-1",
        "-1",
        "Invented Corp",
        "INVC",
        "",
        "",
    ];
    reg.corporations.parse_record(&line).expect("a corp record");
    for (who, roles) in members {
        let who = who.to_string();
        let roles = roles.to_string();
        reg.corporations
            .parse_record(&["corpmember", &who, &corp_s, &roles, "0"])
            .expect("a member record");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sov_hub::fixture::*;

    fn call(
        reg: &mut Registry,
        service: &'static str,
        method: &'static str,
        actor: i64,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        })
    }

    fn said(v: &PyValue) -> String {
        match v {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => format!("{other:?}"),
        }
    }

    fn slash(reg: &mut Registry, actor: i64, line: &str) -> String {
        said(&reg.slash(Some(actor), line).expect("answered"))
    }

    /// The `notify` text of every `CustomNotify` push to `who`.
    fn notices(reg: &mut Registry, who: i64) -> Vec<String> {
        pending(reg)
            .into_iter()
            .filter(|(n, a, to)| {
                *n == "OnRemoteMessage"
                    && to.as_deref() == Some(&[who][..])
                    && a[0] == bstr("CustomNotify")
            })
            .filter_map(|(_, a, _)| crate::pyutil::dict_get(&a[1], b"notify").map(said))
            .collect()
    }

    /// A captured hub with Alice beside it.
    fn claimed() -> (Registry, i64, i64) {
        let (mut reg, a, _) = world();
        let id = launched(&mut reg);
        let pair = owner(&reg, a);
        crate::sov_hub::capture(&mut reg, SYSTEM, pair).expect("captured");
        let at = fuel_spot(&reg);
        place_at(&mut reg, ALICE, at);
        let _ = reg.take_pending();
        (reg, id, a)
    }

    #[test]
    fn ship_drop_is_served_on_the_ship_moniker() {
        let (mut reg, _, _) = world();
        let items = PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(HUB_ITEM),
            PyValue::Int(1),
        ])]);
        let answer = call(
            &mut reg,
            "ship",
            "Drop",
            ALICE,
            &[items, PyValue::None, PyValue::Bool(false)],
        )
        .expect("Drop is answered");
        assert_eq!(
            answer,
            PyValue::Dict(vec![(
                PyValue::List(vec![PyValue::Int(HUB_ITEM)]),
                PyValue::Int(HUB_ITEM)
            )])
        );
        assert!(crate::sov_hub::hub(reg.deltas(), SYSTEM).is_some());
        assert!(call(&mut reg, "ship", "Drop", ALICE, &[PyValue::None]).is_err());
    }

    #[test]
    fn boostsov_fuels_through_the_ungated_qa_service_and_says_so() {
        let (mut reg, id, _) = claimed();
        let line = |s: &str| [PyValue::Bytes(s.as_bytes().to_vec())];
        let out = call(
            &mut reg,
            "publicQaToolsServer",
            "SlashCmd",
            ALICE,
            &line("/boostsov 30"),
        )
        .expect("answered");
        assert_eq!(out, PyValue::None, "the client reads nothing back");
        assert_eq!(crate::sov_hub::fuel_stock(&reg, id), 30);
        let told = notices(&mut reg, ALICE);
        assert!(
            told.iter().any(|t| t.contains("moved 30 fuel blocks")),
            "{told:?}"
        );
        // Status, in any case.
        call(
            &mut reg,
            "publicQaToolsServer",
            "SlashCmd",
            ALICE,
            &line("/BoostSov status"),
        )
        .expect("answered");
        let told = notices(&mut reg, ALICE);
        assert!(
            told.iter().any(|t| t.contains("30 fuel blocks")),
            "{told:?}"
        );
        // A refusal is told too, and moves nothing.
        call(
            &mut reg,
            "publicQaToolsServer",
            "SlashCmd",
            DAVE,
            &line("/boostsov 5"),
        )
        .expect("answered");
        assert!(
            notices(&mut reg, DAVE)
                .iter()
                .any(|t| t.contains("Only the alliance"))
        );
        assert_eq!(crate::sov_hub::fuel_stock(&reg, id), 30);
        // Every other QA line is still a no-op with no push.
        for other in ["/moveme", "/giveitem 34 1000", "/boostsovereignty"] {
            let out = call(
                &mut reg,
                "publicQaToolsServer",
                "SlashCmd",
                ALICE,
                &line(other),
            )
            .expect("answered");
            assert_eq!(out, PyValue::None);
            assert!(notices(&mut reg, ALICE).is_empty(), "{other}");
        }
        // No burn for players.
        call(
            &mut reg,
            "publicQaToolsServer",
            "SlashCmd",
            ALICE,
            &line("/boostsov burn 5"),
        )
        .expect("answered");
        assert_eq!(crate::sov_hub::fuel_stock(&reg, id), 30);
    }

    #[test]
    fn sovfuel_slash_fuels_reports_and_burns() {
        let (mut reg, id, _) = claimed();
        let out = slash(
            &mut reg,
            ALICE,
            &format!("/sovfuel {}", crate::sov_hub::FUEL_PER_DAY + 2),
        );
        assert!(out.contains("moved"), "{out}");
        let out = slash(&mut reg, ALICE, "/sovfuel");
        assert!(
            out.contains(&id.to_string()) && out.contains("claimed"),
            "{out}"
        );
        let out = slash(&mut reg, ALICE, &format!("/sovfuel status {SYSTEM}"));
        assert!(out.contains("1 game days"), "{out}");
        let out = slash(&mut reg, ALICE, "/sovfuel burn 1");
        assert!(out.contains("burned"), "{out}");
        assert_eq!(crate::sov_hub::fuel_stock(&reg, id), 2);
        let out = slash(&mut reg, ALICE, "/sovfuel burn 1");
        assert!(out.contains("offline"), "{out}");
        assert!(slash(&mut reg, ALICE, "/sovfuel burn x").starts_with("usage"));
        assert!(slash(&mut reg, ALICE, "/sovfuel two three").starts_with("usage"));
    }

    #[test]
    fn sov_mgr_fuel_access_and_structure_info_answer() {
        let (mut reg, _, _) = claimed();
        let ssid = [PyValue::Int(SYSTEM)];
        assert_eq!(
            call(&mut reg, "sovMgr", "GetSovHubFuelAccessGroup", ALICE, &ssid).unwrap(),
            PyValue::None
        );
        assert_eq!(
            call(
                &mut reg,
                "sovMgr",
                "SetSovHubFuelAccessGroup",
                ALICE,
                &[PyValue::Int(SYSTEM), PyValue::Int(7)]
            )
            .unwrap(),
            PyValue::None
        );
        for (who, yes) in [(ALICE, true), (BOB, true), (CAROL, false), (DAVE, false)] {
            assert_eq!(
                call(
                    &mut reg,
                    "sovMgr",
                    "IsOnLocalSovHubFuelAccessGroup",
                    who,
                    &[]
                )
                .unwrap(),
                PyValue::Bool(yes),
                "{who}"
            );
        }
        let (mut reg, id, _) = claimed();
        assert_eq!(
            call(
                &mut reg,
                "structureDirectory",
                "GetStructureInfo",
                ALICE,
                &[PyValue::Int(id)]
            )
            .unwrap(),
            PyValue::None
        );
    }

    #[test]
    fn sov_capture_and_sov_set_use_real_hubs() {
        let (mut reg, a, _) = world();
        let out = slash(&mut reg, ALICE, &format!("/sov capture here {a}"));
        assert!(out.contains("no launched"), "{out}");
        let id = launched(&mut reg);
        let out = slash(&mut reg, ALICE, "/sov capture here ish");
        assert!(
            out.contains("captured") && out.contains(&id.to_string()),
            "{out}"
        );
        assert_eq!(sov::holder(reg.deltas(), SYSTEM), Some(a));
        // `/sov clear` leaves the hub neutral and in space; `/sov set` takes
        // it back without a new hub.
        slash(&mut reg, ALICE, "/sov clear here");
        assert_eq!(sov::holder(reg.deltas(), SYSTEM), None);
        assert!(crate::sov_hub::hub(reg.deltas(), SYSTEM).is_some());
        slash(&mut reg, ALICE, "/sov set here ish");
        assert_eq!(sov::claim(reg.deltas(), SYSTEM).map(|c| c.hub_id), Some(id));
        // `/sov set` where nothing was launched makes a real hub.
        let (mut reg, a, _) = world();
        let out = slash(&mut reg, ALICE, &format!("/sov set {SYSTEM} {a}"));
        let hub = crate::sov_hub::hub(reg.deltas(), SYSTEM).expect("a GM hub");
        assert!(out.contains(&hub.hub_id.to_string()), "{out}");
        assert_ne!(hub.hub_id, sov::stand_in_hub_id(SYSTEM));
        assert!(reg.store().get(hub.hub_id).is_some());
        let slims = pending(&mut reg);
        assert!(
            slims
                .iter()
                .any(|(n, _, _)| *n == "ProcessSovStatusChanged")
        );
    }

    #[test]
    fn fuel_access_and_here_follow_the_roster_not_the_home() {
        let (mut reg, _, _) = claimed();
        // Everyone's station is in another system; the roster still has
        // them in SYSTEM, the claimed one.
        for who in [ALICE, BOB, CAROL, DAVE] {
            reg.character_mut(who).unwrap().solar_system_id = Some(HIGH);
        }
        assert_eq!(here(&reg, Some(ALICE)), Some(SYSTEM));
        assert_eq!(
            local_holder(&reg, Some(ALICE)),
            sov::holder(reg.deltas(), SYSTEM)
        );
        assert!(local_holder(&reg, Some(ALICE)).is_some());
        assert_eq!(
            call(
                &mut reg,
                "sovMgr",
                "IsOnLocalSovHubFuelAccessGroup",
                ALICE,
                &[]
            )
            .unwrap(),
            PyValue::Bool(true)
        );
        let status = slash(&mut reg, ALICE, "/sovfuel status");
        assert!(!status.contains("not in a system"), "{status}");
    }
}
