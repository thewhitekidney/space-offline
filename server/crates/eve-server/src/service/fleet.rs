//! **The fleet's remote face.** Three services meet the client here:
//!
//! - `fleetObjectHandler`, the bound object a fleet is driven through: the
//!   creator gets one from `CreateFleet()` **[V] fleetSvc.py:329**, everyone
//!   else binds `Moniker('fleetObjectHandler', fleetID)`
//!   **[V] eveMoniker.py:202-205**;
//! - `fleetProxy`, the fleet finder's front door **[V] fleetSvc.py:1799**;
//! - `fleetMgr`, the calls made with no fleet object at hand
//!   **[V] fleetSvc.py:361, 759, 955, 963**.
//!
//! The rules live in [`crate::fleet`]: this file reads arguments, picks the
//! fleet a call is about, runs the matching [`crate::fleet::Fleets`] method
//! and turns its [`crate::fleet::Effects`] into pushes and session changes.
//! It owns no state of its own.

use super::{Registry, Request, ServiceError};
use crate::fleet::{Effects, FleetError, Fleets, Member};
use crate::fleet_broadcast::Range;
use eve_marshal::PyValue;

#[cfg(test)]
mod tests;

pub(crate) const SERVICES: &[&str] = &["fleetObjectHandler", "fleetProxy", "fleetMgr"];

/// Every method the bound fleet object answers. A name outside this list is
/// `NoSuchMethod` before the caller or the fleet is looked at, so a call we
/// never built (`LoadFleetSetup`, `FinishMove`, `MassInvite`) is logged by
/// name rather than answered by accident.
const METHODS: &[&str] = &[
    "CreateFleet",
    "Init",
    "GetInitState",
    "GetFleetID",
    "GetWings",
    "GetMotd",
    "GetFleetMaxSize",
    "GetFleetComposition",
    "GetJoinRequests",
    "RejectJoinRequest",
    "Invite",
    "AcceptInvite",
    "RejectInvite",
    "LeaveFleet",
    "KickMember",
    "MakeLeader",
    "DisbandFleet",
    "MoveMember",
    "MassMoveMembers",
    "CreateWing",
    "DeleteWing",
    "ChangeWingName",
    "CreateSquad",
    "DeleteSquad",
    "ChangeSquadName",
    "SetMotdEx",
    "SetOptions",
    "SetFleetMaxSize",
    "SetAutoJoinSquadID",
    "SetTakesFleetWarp",
    "SetAcceptsRegroupValue",
    "SetAcceptsConduitJumpsValue",
    "Reconnect",
    "UpdateMemberInfo",
    "SendBroadcast",
];

impl Registry {
    /// Every fleet on this node.
    pub fn fleets(&self) -> &Fleets {
        &self.fleets
    }

    pub fn fleets_mut(&mut self) -> &mut Fleets {
        &mut self.fleets
    }

    /// Carry out what a fleet mutator asked for: its notifications go out
    /// with the answer, its session changes wait for the next turn. Fleets
    /// are not saved, so nothing is marked dirty.
    pub(crate) fn fleet_apply(&mut self, effects: Effects) {
        let Effects { pushes, sessions } = effects;
        for push in pushes {
            self.push(push);
        }
        self.fleets.queue_session_changes(sessions);
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "fleetProxy" => proxy(reg, req),
        "fleetMgr" => manager(reg, req),
        _ => fleet_object(reg, req),
    }
}

/// `fleetProxy`. The advert list is answered here, even for a connection
/// with no character: the client walks it with `.iteritems()`
/// **[V] fleetSvc.py:1799**, so it is always a dict. The rest of the finder
/// lives in `gaps2`.
fn proxy(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    if req.method == "GetAvailableFleetAds" {
        return Ok(crate::fleet_finder::available(reg, req.actor));
    }
    super::gaps2::fleet_proxy(reg, req).unwrap_or_else(|| Err(reg.no_method(req)))
}

/// `fleetMgr`. `RegisterForDamageUpdates` is acknowledged and nothing more:
/// its answer is thrown away **[V] fleetSvc.py:759**. The two ranged
/// broadcasts send `(name, scope, itemID, typeID)`
/// **[V] fleetSvc.py:955, 963**.
fn manager(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        "RegisterForDamageUpdates" => Ok(PyValue::None),
        "BroadcastToBubble" => crate::fleet_broadcast::dispatch(reg, req, Range::Bubble),
        "BroadcastToSystem" => crate::fleet_broadcast::dispatch(reg, req, Range::System),
        _ => super::gaps2::fleet_mgr(reg, req).unwrap_or_else(|| Err(reg.no_method(req))),
    }
}

/// A fleet refusal as the `UserError` the client shows.
fn raise(e: FleetError) -> ServiceError {
    ServiceError::Raise(e.to_user_error())
}

/// The fleet id carried by a bound object's parameters: a bare integer, or
/// a tuple that starts with one. The creator's own object carries none.
fn bound_fleet(bind: Option<&PyValue>) -> Option<i64> {
    match bind? {
        PyValue::Int(id) => Some(*id),
        PyValue::Tuple(items) => match items.first() {
            Some(PyValue::Int(id)) => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

/// The integer at `i`, if the client sent one. A bool counts as 0 or 1;
/// `None`, an absent argument or anything else is "not given".
fn int_at(args: &[PyValue], i: usize) -> Option<i64> {
    match args.get(i)? {
        PyValue::Int(n) | PyValue::Long(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

/// The text at `i`, `str` or `unicode`; empty when there is none.
fn text_at(args: &[PyValue], i: usize) -> String {
    args.get(i)
        .and_then(crate::pyutil::as_bytes)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

/// The ids in a list or tuple of character ids (a bare id is a list of one);
/// entries that are not integers are dropped.
fn id_list(v: Option<&PyValue>) -> Vec<i64> {
    let one = |v: &PyValue| match v {
        PyValue::Int(n) | PyValue::Long(n) => Some(*n),
        _ => None,
    };
    match v {
        Some(PyValue::List(items) | PyValue::Tuple(items)) => {
            items.iter().filter_map(one).collect()
        }
        Some(v) => one(v).into_iter().collect(),
        None => Vec::new(),
    }
}

/// The member-flag attribute each opt-out setter writes: the keys of the
/// client's own `memberOptOuts` object **[V] fleetSvc.py:415-431**.
fn opt_out_attribute(method: &str) -> Option<&'static str> {
    match method {
        "SetTakesFleetWarp" => Some("acceptsFleetWarp"),
        "SetAcceptsRegroupValue" => Some("acceptsFleetRegroups"),
        "SetAcceptsConduitJumpsValue" => Some("acceptsConduitJumps"),
        _ => None,
    }
}

/// `fleetObjectHandler`: settle the method name, then the caller, then the
/// fleet, then do the call.
fn fleet_object(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    if !METHODS.contains(&req.method) {
        return Err(reg.no_method(req));
    }
    // No character, no fleet: the client has a dialog for that.
    let actor = req.actor.ok_or_else(|| raise(FleetError::NotInFleet))?;
    match req.method {
        // The object is minted after this answer, so there is no fleet yet.
        "CreateFleet" => {
            return reg
                .fleets_mut()
                .create(actor)
                .map(PyValue::Int)
                .map_err(raise);
        }
        // The broadcast module finds the sender's fleet itself.
        "SendBroadcast" => return crate::fleet_broadcast::dispatch(reg, req, Range::Fleet),
        _ => {}
    }
    let fleet_id = bound_fleet(req.bind_params)
        .or_else(|| reg.fleets().resolve_for(actor))
        .ok_or_else(|| raise(FleetError::NotInFleet))?;
    on_fleet(reg, req, actor, fleet_id)
}

/// The fleet named by `fleet_id`, for a read.
fn read_fleet(reg: &Registry, fleet_id: i64) -> Result<&crate::fleet::Fleet, ServiceError> {
    reg.fleets()
        .fleet(fleet_id)
        .ok_or_else(|| raise(FleetError::NotFound))
}

/// Apply a mutator's effects and answer its flag. The client throws most of
/// these answers away; `MoveMember`'s is tested for truth.
fn done(
    reg: &mut Registry,
    r: Result<(bool, Effects), FleetError>,
) -> Result<PyValue, ServiceError> {
    let (ok, fx) = r.map_err(raise)?;
    reg.fleet_apply(fx);
    Ok(PyValue::Bool(ok))
}

/// Apply a mutator's effects and answer `True`.
fn applied(reg: &mut Registry, r: Result<Effects, FleetError>) -> Result<PyValue, ServiceError> {
    reg.fleet_apply(r.map_err(raise)?);
    Ok(PyValue::Bool(true))
}

/// One call on a resolved fleet.
fn on_fleet(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    fleet_id: i64,
) -> Result<PyValue, ServiceError> {
    let args = req.args;
    let arg = |i: usize| int_at(args, i);
    let target = arg(0).unwrap_or(0);
    match req.method {
        // `Init(shipTypeID, setupName, adInfoData=...)`: the answer is read
        // as advert data, and a falsy one skips registering an advert
        // **[V] fleetSvc.py:331-336**.
        "Init" => {
            let station = reg.docked_station(actor);
            let system = reg.session_system(actor);
            let fx = reg
                .fleets_mut()
                .init(actor, fleet_id, arg(0), station, system)
                .map_err(raise)?;
            reg.fleet_apply(fx);
            Ok(PyValue::None)
        }
        "GetInitState" => Ok(read_fleet(reg, fleet_id)?.init_state()),
        "GetFleetID" => Ok(PyValue::Int(fleet_id)),
        "GetWings" => Ok(read_fleet(reg, fleet_id)?.wings_pyvalue()),
        // Cached by the client behind `is None`, so never `None`
        // **[V] fleetSvc.py:1872**.
        "GetMotd" => Ok(PyValue::Unicode(read_fleet(reg, fleet_id)?.motd.clone())),
        "GetFleetMaxSize" => Ok(PyValue::Int(read_fleet(reg, fleet_id)?.max_size as i64)),
        "GetFleetComposition" => composition(reg, fleet_id),
        "GetJoinRequests" => Ok(crate::fleet_finder::requests_value(reg, Some(actor))),
        "RejectJoinRequest" => {
            crate::fleet_finder::reject(reg, Some(actor), target);
            Ok(PyValue::None)
        }
        "Invite" => {
            dev_release_bot(reg, actor, fleet_id, target);
            let r = reg
                .fleets_mut()
                .invite(actor, fleet_id, target, arg(1), arg(2), arg(3));
            done(reg, r)
        }
        "AcceptInvite" => {
            let station = reg.docked_station(actor);
            let system = reg.session_system(actor);
            let r = reg
                .fleets_mut()
                .accept_invite(actor, fleet_id, arg(0), station, system);
            done(reg, r)
        }
        // `RejectInvite()`, `(True)` or `(False)` **[V] fleetSvc.py:1123,
        // 1127, 1130, 1141**: the argument changes nothing here.
        "RejectInvite" => Ok(PyValue::Bool(
            reg.fleets_mut().reject_invite(actor, fleet_id).is_some(),
        )),
        "LeaveFleet" => {
            let r = reg.fleets_mut().leave(actor, fleet_id);
            applied(reg, r)
        }
        "KickMember" => {
            let r = reg.fleets_mut().kick(actor, fleet_id, target);
            done(reg, r)
        }
        "MakeLeader" => {
            let r = reg.fleets_mut().make_leader(actor, fleet_id, target);
            done(reg, r)
        }
        "DisbandFleet" => {
            let r = reg.fleets_mut().disband(actor, fleet_id);
            applied(reg, r)
        }
        // The answer is tested for truth before the window marks the pilot
        // as moving **[V] fleetSvc.py:506-507**.
        "MoveMember" => {
            let r = reg
                .fleets_mut()
                .move_member(actor, fleet_id, target, arg(1), arg(2), arg(3));
            done(reg, r)
        }
        "MassMoveMembers" => Ok(mass_move(reg, actor, fleet_id, args)),
        "CreateWing" => {
            let (wing, fx) = reg
                .fleets_mut()
                .create_wing(actor, fleet_id)
                .map_err(raise)?;
            reg.fleet_apply(fx);
            Ok(PyValue::Int(wing))
        }
        "DeleteWing" => {
            let r = reg.fleets_mut().delete_wing(actor, fleet_id, target);
            applied(reg, r)
        }
        "ChangeWingName" => {
            let name = text_at(args, 1);
            let r = reg
                .fleets_mut()
                .change_wing_name(actor, fleet_id, target, &name);
            applied(reg, r)
        }
        "CreateSquad" => {
            let (squad, fx) = reg
                .fleets_mut()
                .create_squad(actor, fleet_id, target)
                .map_err(raise)?;
            reg.fleet_apply(fx);
            Ok(PyValue::Int(squad))
        }
        "DeleteSquad" => {
            let r = reg.fleets_mut().delete_squad(actor, fleet_id, target);
            applied(reg, r)
        }
        "ChangeSquadName" => {
            let name = text_at(args, 1);
            let r = reg
                .fleets_mut()
                .change_squad_name(actor, fleet_id, target, &name);
            applied(reg, r)
        }
        "SetMotdEx" => {
            let motd = text_at(args, 0);
            let r = reg.fleets_mut().set_motd(actor, fleet_id, &motd);
            applied(reg, r)
        }
        "SetOptions" => {
            let (free_move, auto_join) = read_options(args.first());
            let (options, fx) = reg
                .fleets_mut()
                .set_options(actor, fleet_id, free_move, auto_join)
                .map_err(raise)?;
            reg.fleet_apply(fx);
            Ok(options.to_pyvalue())
        }
        "SetFleetMaxSize" => {
            reg.fleets_mut()
                .set_max_size(actor, fleet_id, target)
                .map_err(raise)?;
            Ok(PyValue::Bool(true))
        }
        // `SetAutoJoinSquadID(None)` clears it **[V] ui/shared/fleet/fleet.py:89**.
        "SetAutoJoinSquadID" => {
            let squad = arg(0).filter(|s| *s > 0);
            let (_, fx) = reg
                .fleets_mut()
                .set_options(actor, fleet_id, None, Some(squad))
                .map_err(raise)?;
            reg.fleet_apply(fx);
            Ok(PyValue::Bool(true))
        }
        "SetTakesFleetWarp" | "SetAcceptsRegroupValue" | "SetAcceptsConduitJumpsValue" => {
            let Some(which) = opt_out_attribute(req.method) else {
                return Err(reg.no_method(req));
            };
            let on = arg(0).is_none_or(|v| v != 0);
            let r = reg.fleets_mut().set_opt_out(actor, fleet_id, which, on);
            applied(reg, r)
        }
        // Sent when the client finds a fleet id it kept across a drop, inside
        // a `try` that only logs **[V] fleetSvc.py:1615-1631**.
        "Reconnect" => {
            let fx = reg
                .fleets_mut()
                .reconnect(actor, fleet_id)
                .ok_or_else(|| raise(FleetError::NotInFleet))?;
            reg.fleet_apply(fx);
            Ok(PyValue::Bool(true))
        }
        // The composition reads where each pilot is now, so the ship type
        // the client reports after a session change is not kept
        // **[V] fleetSvc.py:1709-1719**.
        "UpdateMemberInfo" => Ok(PyValue::None),
        _ => Err(reg.no_method(req)),
    }
}

/// `MassMoveMembers(charIDs, wingID, squadID, role)`: each pilot is moved on
/// his own, and one who cannot be moved is passed over. The answer is the
/// list of those who did move, because the client hands it to
/// `OnManyFleetMembersChanging`, which iterates it
/// **[V] fleetSvc.py:518-520, ui/shared/fleet/fleet.py:146-149**. An empty
/// list is falsy, which the client reads as "nothing moved".
fn mass_move(reg: &mut Registry, actor: i64, fleet_id: i64, args: &[PyValue]) -> PyValue {
    let (wing, squad, role) = (int_at(args, 1), int_at(args, 2), int_at(args, 3));
    let mut moved = Vec::new();
    for who in id_list(args.first()) {
        let Ok((changed, fx)) = reg
            .fleets_mut()
            .move_member(actor, fleet_id, who, wing, squad, role)
        else {
            continue;
        };
        reg.fleet_apply(fx);
        if changed {
            moved.push(PyValue::Int(who));
        }
    }
    PyValue::List(moved)
}

/// The two fields `SetOptions` may change, out of the options object the
/// client sends: its own options `KeyVal` with `isFreeMove` replaced
/// **[V] fleetSvc.py:436-440**, or a plain dict. A missing key leaves that
/// field alone; `isRegistered` belongs to the fleet finder and is not taken
/// from here.
fn read_options(v: Option<&PyValue>) -> (Option<bool>, Option<Option<i64>>) {
    let attrs = match v {
        Some(PyValue::Instance { state, .. }) => &**state,
        Some(d @ PyValue::Dict(_)) => d,
        _ => return (None, None),
    };
    let get = |k: &str| crate::pyutil::dict_get(attrs, k.as_bytes());
    let free_move = get("isFreeMove").map(|v| match v {
        PyValue::Bool(b) => *b,
        PyValue::Int(n) | PyValue::Long(n) => *n >= 1,
        _ => false,
    });
    let auto_join = get("autoJoinSquadID").map(|v| match v {
        PyValue::Int(n) | PyValue::Long(n) if *n > 0 => Some(*n),
        _ => None,
    });
    (free_move, auto_join)
}

/// `GetFleetComposition()`: one row per member, read field by field by the
/// composition window **[V] fleetCompositionWnd.py:97-136**. The window
/// shows the leadership skills only when `skills` is non-empty
/// (`:113-115`), and this server does not compute them, so both lists stay
/// empty.
fn composition(reg: &Registry, fleet_id: i64) -> Result<PyValue, ServiceError> {
    let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    let rows = read_fleet(reg, fleet_id)?
        .members
        .values()
        .map(|m| {
            let (system, station, ship) = live_location(reg, m);
            crate::pyutil::keyval(vec![
                ("characterID", PyValue::Int(m.char_id)),
                ("solarSystemID", opt(system)),
                ("stationID", opt(station)),
                ("shipTypeID", opt(ship)),
                ("skills", PyValue::List(Vec::new())),
                ("skillIDs", PyValue::List(Vec::new())),
            ])
        })
        .collect();
    Ok(PyValue::List(rows))
}

/// Where a member is now: `(solar system, station, ship type)`.
///
/// The live session answers first. With no session system, a character
/// record that names a station answers from that record; with neither, the
/// values the member row took when he joined. The ship is the character's,
/// unless the record has none.
pub(crate) fn live_location(reg: &Registry, m: &Member) -> (Option<i64>, Option<i64>, Option<i64>) {
    let who = m.char_id;
    let record = reg.character(who);
    let record_station = record.and_then(|c| c.station_id);
    let (system, station) = match reg.session_system(who) {
        Some(system) => (Some(system), reg.docked_station(who)),
        None if record_station.is_some() => (None, reg.docked_station(who).or(record_station)),
        None => (m.solar_system_id, m.station_id),
    };
    let ship = record
        .map(|c| c.ship_type_id)
        .filter(|t| *t != 0)
        .or(m.ship_type_id);
    (system, station, ship)
}

/// **Dev mode only:** a human inviting a bot that sits in a fleet of bots
/// takes it out of that fleet first, so a tester can gather bots without
/// disbanding their fleets by hand. A project rule, not a client one; the
/// normal "already in a fleet" refusal applies everywhere else.
fn dev_release_bot(reg: &mut Registry, actor: i64, fleet_id: i64, invitee: i64) {
    let is_bot = |reg: &Registry, c: i64| reg.character(c).is_some_and(|c| c.is_bot);
    if !reg.config().dev || is_bot(reg, actor) || !is_bot(reg, invitee) {
        return;
    }
    let Some(old) = reg.fleets().fleet_of(invitee) else {
        return;
    };
    if old.fleet_id == fleet_id || !old.members.keys().all(|c| is_bot(reg, *c)) {
        return;
    }
    let old_id = old.fleet_id;
    if let Ok(fx) = reg.fleets_mut().leave(invitee, old_id) {
        reg.fleet_apply(fx);
    }
}
