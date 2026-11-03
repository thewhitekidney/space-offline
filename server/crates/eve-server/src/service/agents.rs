//! The agents: `agentMgr` and `epicArcStatus`.
//!
//! W10.5 seam S5 (`docs/content/plan.md` §6.1): moved verbatim out of
//! `service/character.rs`, with their tests, and `agent_rowset` out of
//! `service.rs`, so the agent and mission packets open this file and not the
//! character sheet's. The station agent's and the epic-arc starters'
//! constants stay in the parent for now, because `service/tests.rs` reads
//! them there.
//!
//! W13 packet CP26b-1 (`docs/content/plan.md` §6.2, C4 §3.1, §3.9): every
//! agent in the mission pool (`crate::mission_pool::Table`, the bundle's
//! `agents/agents.tsv`) is served by `GetAgents`, and every bound arm speaks
//! to the agent its moniker was bound to, `Moniker('agentMgr', agentID)`
//! **[V]** (`eveMoniker.GetAgent`, `docs/re/agents-missions.md` §3.1).
//!
//! W16 packet CP41 (C4 §7, K6): the epic-arc arms answer from
//! `crate::epic_arc`, and every arm that reads a bound agent's mission first
//! lets the arcs put their offer there (`epic_arc::prime`).

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &["agentMgr", "epicArcStatus"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Epic-arc availability per agent, called once per agent in the
        // station from the lobby's Agents tab (`dockedUI/agents:295`) and
        // from `agentUtil:43,68`, all as a truth test (lane B1): whether the
        // agent has an arc mission on offer or accepted for the caller.
        "epicArcStatus" => match req.method {
            "AgentHasEpicMissionsForCharacter" => {
                let agent = crate::pyutil::opt_id(req.args.first());
                let has = character(reg, req).zip(agent).is_some_and(|(ch, a)| {
                    crate::epic_arc::has_missions(reg, ch, a, crate::pyutil::filetime_now())
                });
                Ok(PyValue::Bool(has))
            }
            _ => Err(reg.no_method(req)),
        },
        "agentMgr" => match req.method {
            // Epic-arc mission progress, as a **dict of dicts**.
            // `epicArcController.py(95) _GetMissionStatus` is
            //
            //     status = self._missionStatusesByEpicArcID.get(epicArcID, {}) \
            //                  .get(missionID, None)
            //
            // so `None` is `AttributeError: 'NoneType' object has no attribute
            // 'get'`, raised inside `EpicArcController.__init__` -- which
            // leaves `epicArcSvc.controller` unassigned, and
            // `CheckConstructController(38)` guards on `if not
            // self.controller`, so it re-fires on every Agency refresh.
            //
            // **We did not cause this by inventing the epic-arc agents**,
            // which was my first guess and is wrong. The arcs come from
            // `iter_epic_arcs()` -- client-side FSD -- and `_GetEpicArcs(33)`
            // touches no service of ours. The call appeared because this was
            // the first session in which the Journal and the Agency were
            // opened at all: `GetMyEscalatingPathDetails`,
            // `GetIncursionGlobalReport`, `GetGlobalWorldEventTales`, the
            // `dungeonInstanceCacheMgr` methods and `GetMyEnlistment` are all
            // new in the same capture, in the same 77-second cluster.
            //
            // The thirteen agent rows are nonetheless what makes `{}`
            // *enough* rather than one layer down.
            // `contentProviderEpicArcs._ConstructContentPiece(34)` reads
            // `agent.solarsystemID`, `.agentID`, `.stationID` and
            // `.agentTypeID` off `epicArc.GetActiveAgent()`, and
            // `GetActiveMissions(380)` ends `return missions or
            // self.startNodes[:]` -- so with nothing accepted it still yields
            // a start node and asks `agents.GetAgentByID` for one of the
            // starter ids. An empty agent rowset would make that `None` and
            // the next line an `AttributeError`.
            //
            // CP41: `{epicArcID: {missionID: KeyVal}}` from `epic_arc`, still
            // `{}` for a character who never spoke to an arc agent.
            "GetMyEpicArcStatus" => Ok(match character(reg, req) {
                Some(ch) => crate::epic_arc::status(reg, ch, crate::pyutil::filetime_now()),
                None => PyValue::Dict(vec![]),
            }),
            // `epicArcController._GetMissionMessageID:65-66` is
            // `messages.get(messageType, {}).get(missionID, None)`, so a
            // dict of dicts, keyed by message type then mission id (lane
            // B1). `None` is an `AttributeError` on `.get`, inside the
            // controller's construction. CP41 fills it from the missions'
            // `messages.epicMission.*` slots (C4 §7.2).
            "GetMessagesForEpicArcMissions" => Ok(crate::epic_arc::messages(reg)),
            // A **sliceable two-element sequence of lists**,
            // `[missions, offers]`. The client does `reg.agentjournal[0]`,
            // `[:1][0]`, and `[n].extend(...)` for n in range(2) -- so a
            // one-element sequence survives the first two and breaks the
            // third, and a bare `[]` breaks immediately.
            //
            // One row per accepted mission now, with the kill mission's
            // pocket bookmark in it: `crate::mission::journal`.
            //
            // **Bound, it is that agent's rows only.** The journal merges
            // each bound answer into the unbound one, removing the bound
            // agent's row and extending both lists (`journal.py:434-447`),
            // so an answer with every agent's rows shows the others twice.
            // `bound_agent` would fall back to `AGENT_ID`, so read the bind
            // params directly: unbound stays every agent.
            "GetMyJournalDetails" => {
                let only = match req.bind_params {
                    Some(PyValue::Int(id)) => Some(*id),
                    _ => None,
                };
                Ok(crate::mission::journal(reg, req.actor, only))
            }
            // The mission-details window (`agents.py:821,830`): a dict, or
            // `None` when the bound agent has no mission, which closes it.
            "GetMissionJournalInfo" => {
                let agent = bound_agent(req, None);
                if crate::bot_panel::is_panel_agent(reg, agent) {
                    return Ok(PyValue::None);
                }
                prime_arcs(reg, req, agent);
                Ok(crate::mission::journal_info_at(reg, req.actor, agent))
            }
            // The career-agent cards (`agents.py:218-223`): a dict holding
            // **every** asked id (a missing one is a `KeyError`, `None` a
            // `TypeError`). Completed is a fixed list that is all done for
            // the caller `[A]`; falsified by a career agent shown as
            // completed while it still offers a mission.
            "GetCompletedCareerAgentIDs" => {
                let ids: Vec<i64> = match req.args.first() {
                    Some(PyValue::List(v) | PyValue::Tuple(v)) => v
                        .iter()
                        .filter_map(|x| match x {
                            PyValue::Int(i) => Some(*i),
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                let ch = req.actor;
                let ix = reg.ext::<crate::storyline::Index>();
                let done = |agent: i64| {
                    let list = ix.list(agent);
                    ch.is_some_and(|ch| {
                        !list.is_empty()
                            && list
                                .iter()
                                .all(|m| crate::storyline::list_done(reg, ch, *m))
                    })
                };
                Ok(PyValue::Dict(
                    ids.iter()
                        .map(|a| (PyValue::Bool(done(*a)), PyValue::Int(*a)))
                        .collect(),
                ))
            }
            // Cached once per session (`agents.py:363`), only tested with
            // `in` (`:368`): none disabled.
            "GetDisabledMissions" => Ok(PyValue::List(vec![])),
            // The MissionGiver window's standings line: what accepting and
            // finishing `missionID` pays the bound agent and its corp. Keys
            // must stay within {faction, corp, agent} (`standingsvc:244-252`);
            // an unknown mission is `{}`, which the window's `min`/`max`
            // survive.
            "GetStandingGainsForMission" => {
                let agent = bound_agent(req, None);
                let gains = match (
                    reg.content().try_get::<crate::mission_pool::Table>(),
                    req.args.first(),
                ) {
                    (Some(t), Some(PyValue::Int(id))) => t.agent(agent).zip(t.mission(*id)),
                    _ => None,
                }
                .map(|(a, m)| (a.id, a.corp, crate::mission_rewards::reward(reg, a, m)));
                Ok(PyValue::Dict(match gains {
                    Some((id, corp, r)) => vec![
                        (PyValue::Real(r.agent_gain), PyValue::Int(id)),
                        (PyValue::Real(r.corp_gain), PyValue::Int(corp)),
                    ],
                    None => vec![],
                }))
            }
            // **Must be a `Rowset`, never a `CRowset`.** The caller
            // opens with `GetAgents().Clone()`, and `CRowset` has no
            // `Clone` -- its method set is `Copy, GetColumnNames,
            // columns, InsertNew, Sort, Index, Filter, Rebuild, Unpack`.
            // Only `Rowset`, `IndexRowset` and `FilterRowset` have it.
            // So this is one of the few places where the two rowset
            // families are not interchangeable, and picking the wrong
            // one is an `AttributeError` rather than a shape error.
            //
            // It is then `Index`ed on `agentID` and `Filter`ed on five
            // more columns, all of which must exist even with no rows.
            // `solarsystemID` is all lowercase here. **[I]** on
            // `quality`/`isLocator`: they are TQ's historical columns and
            // the Agent Finder reads `isLocator`, so they are carried,
            // but no agent row has crossed the wire to confirm them.
            "GetAgents" => {
                let c = req.actor.and_then(|id| reg.character(id));
                Ok(agent_rowset(
                    reg.content().try_get::<crate::mission_pool::Table>(),
                    c.and_then(|c| c.station_id),
                    c.and_then(|c| c.solar_system_id),
                ))
            }
            // **The whole agent conversation is this one method.** Offer,
            // accept, decline, defer, quit and complete are all one
            // `actionID` through `DoAction`; there is no `AcceptMission`,
            // `DeclineMission` or `CompleteMission` anywhere in the client
            // tree. So this is the only mutator in the subsystem, and
            // answering it is what makes the agent window open at all.
            //
            // The shape is `((agentSaysMsg, availableActions),
            // lastActionInfo)`, consumed by an unconditional nested unpack --
            // `(a, b), c = DoAction(...)` -- so every level has to be the
            // right arity before anything is read.
            //
            // Three things are load-bearing:
            //
            // - **`agentSaysMsg` is itself a 2-tuple** `(msgInfo, contentID)`.
            //   `missioncontroller` does `_, mission_id = agent_says`, so a
            //   bare message would unpack wrongly. `msgInfo` may be a raw
            //   string, an int message id, or a `(label, args)` pair;
            //   `ProcessMessage` returns a raw string verbatim, which is why
            //   a plain sentence is the honest answer while we have no
            //   mission to describe.
            // - **`availableActions` is empty on purpose.** It is a list of
            //   `(actionID, actionData)`, and the window *auto-calls*
            //   `DoAction(firstActionID)` when the first action is
            //   RequestMission (2) or ViewMission (1). So offering a button
            //   here would immediately provoke a second `DoAction` for a
            //   mission we cannot yet supply -- the empty list is what keeps
            //   the window open and inert rather than half-offering.
            // - **`lastActionInfo`'s four keys are subscripted, not `.get`**,
            //   so all four must be present even though all four are false.
            "DoAction" => {
                // The `actionID`, or `None` for "open the conversation".
                let action = match req.args.first() {
                    Some(PyValue::Int(id)) => Some(*id),
                    _ => None,
                };
                let agent = bound_agent(req, None);
                // BOT-PANEL: the dev "Bot Control" agent speaks its own panel.
                if let Some(panel) = crate::bot_panel::answer(reg, req.actor, agent, action) {
                    return Ok(panel);
                }
                prime_arcs(reg, req, agent);
                let (answer, change) = crate::mission::converse_at(reg, req.actor, agent, action);
                if let Some(ch) = character(reg, req) {
                    crate::epic_arc::after_turn(reg, ch, agent, change);
                }
                // Last: the journal reads back what the conversation made true.
                crate::mission::tell_journal_at(reg, agent, change);
                Ok(answer)
            }
            // Both carry a real mission now. `GetMissionBriefingInfo` is
            // read as `if <result> and <result>['ContentID']`, so a truthy
            // dict commits us to both that key and `'Mission Keywords'`.
            "GetMissionBriefingInfo" => {
                let agent = bound_agent(req, None);
                if crate::bot_panel::is_panel_agent(reg, agent) {
                    return Ok(PyValue::None);
                }
                prime_arcs(reg, req, agent);
                Ok(crate::mission::briefing_info_at(reg, req.actor, agent))
            }
            "GetMissionObjectiveInfo" => {
                let agent = bound_agent(req, None);
                if crate::bot_panel::is_panel_agent(reg, agent) {
                    return Ok(PyValue::None);
                }
                prime_arcs(reg, req, agent);
                Ok(crate::mission::objective_info_at(reg, req.actor, agent))
            }
            // **`{}`, never `None`.** The guard is key *presence* --
            // `if (agentID, contentID) not in self.missionArgs` -- so an empty
            // dict stores, satisfies its own guard and is never re-asked.
            // `None` instead reaches `msgArgs.update(result)`, which raises
            // `TypeError` *outside* the only `try` in `ProcessMessage`, and
            // the player gets a blank agent window.
            //
            // When there is a mission this carries exactly eight integer
            // keys: `dungeonLocationID`, `dungeonSolarSystemID`,
            // `objectiveDestinationID`, `objectiveDestinationSystemID`,
            // `objectiveLocationID`, `objectiveTypeID`, `objectiveQuantity`
            // and `rewardTypeID`. The client names none of them anywhere --
            // the key set is declared per message by the localisation
            // corpus's own token records, which is why `tools/loctable.py id
            // <N>` is the oracle for it. `player` needs no entry:
            // `localization.Get` defaults it to `session.charid`.
            // **Answered properly, not emptily.** `{}` would render the raw
            // `{[location]objectiveDestinationID.name}` token in the offer
            // line, because `_GetConversation` primes the message arguments
            // *before* it fetches the briefing -- so the briefing shipping
            // its own copy of the keywords does not save this call.
            // The client caches the answer per `(agentID, contentID)`; the
            // bound agent's current mission is the one its prose names, so
            // the keys are that mission's (`mission::keywords_at`).
            "GetMissionKeywords" => {
                let agent = bound_agent(req, None);
                if crate::bot_panel::is_panel_agent(reg, agent) {
                    return Ok(PyValue::Dict(vec![]));
                }
                prime_arcs(reg, req, agent);
                Ok(crate::mission::keywords_at(reg, req.actor, agent))
            }
            // "Warp to" on an agent bookmark: `WarpToLocation(locationType,
            // locationNumber, warpRange, fleet, referringAgentID,
            // formationSettings)`, six positionals
            // (`movementFunctions.py:458`) **[V]**. The answer is discarded
            // (`POP_TOP`) but must still be sent. Handed to the ordinary
            // warp under `crate::mission::WARP_SUBJECT_MISSION`, which finds
            // the actor's own pocket with that agent; anything else is
            // `None` with nothing pushed, like every warp to a place we
            // cannot resolve. The agent is the bound one, else the
            // `referringAgentID` positional.
            "WarpToLocation" => {
                let agent = bound_agent(req, req.args.get(4));
                let range = crate::mission::warp_range(req.args.get(2));
                // Only a pilot who owns the pocket takes his fleet into it.
                let owns = req
                    .actor
                    .is_some_and(|a| crate::mission::warp_target(reg, a, agent).is_some());
                // A movement guard's refusal is raised (W16 CP57).
                match super::space::warp(
                    reg,
                    req,
                    crate::mission::WARP_SUBJECT_MISSION.to_vec(),
                    Some(&PyValue::Int(agent)),
                    range,
                ) {
                    Some(refusal) => Err(refusal.into_error()),
                    None => {
                        // `fleet` is positional 3 (`WarpFleetToBookmark`,
                        // `movementFunctions.py:472`) **[V]**: the fleet
                        // follows into the commander's pocket.
                        let fleet = match req.args.get(3) {
                            Some(PyValue::Bool(b)) => *b,
                            Some(PyValue::Int(n)) => *n != 0,
                            _ => false,
                        };
                        if owns {
                            crate::fleet_warp::follow_if(
                                reg,
                                req,
                                crate::mission::WARP_SUBJECT_MISSION_FLEET,
                                fleet,
                                |reg, one| {
                                    super::space::warp(
                                        reg,
                                        one,
                                        crate::mission::WARP_SUBJECT_MISSION_FLEET.to_vec(),
                                        Some(&PyValue::Int(agent)),
                                        range,
                                    )
                                },
                            );
                        }
                        Ok(PyValue::None)
                    }
                }
            }
            // The agent's own location, for the dialogue header. `None` is a
            // `TypeError`; `locationType` is omitted so the `LocationWrapper`
            // branch -- whose `cfg.evelocations.Get` is unguarded -- is never
            // taken.
            "GetAgentLocationWrap" => Ok(agent_location(reg, req)),
            // One bound call per agent in the station, from the lobby's
            // Agents tab (`station/agents/agents.py:295`), with the
            // `characterID`. **Falsy means no replay cooldown.** CP41: when
            // the bound agent starts an arc the character finished and its
            // restart interval has not passed, the time it may be replayed
            // (`epic_arc::replay_at`); else `None`.
            "GetReplayTimestamp" => {
                let ch = crate::pyutil::opt_id(req.args.first()).or(req.actor);
                let at = ch.and_then(|ch| {
                    let agent = bound_agent(req, None);
                    crate::epic_arc::replay_at(reg, ch, agent, crate::pyutil::filetime_now())
                });
                Ok(at.map_or(PyValue::None, PyValue::Int))
            }
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The caller, when they are a character this server holds.
fn character(reg: &Registry, req: &Request<'_>) -> Option<i64> {
    req.actor.filter(|id| reg.character(*id).is_some())
}

/// Let the epic arcs put their offer at `agent` before the caller reads or
/// speaks to it (`epic_arc::prime`).
fn prime_arcs(reg: &mut Registry, req: &Request<'_>, agent: i64) {
    if let Some(ch) = character(reg, req) {
        crate::epic_arc::prime(reg, ch, agent, crate::pyutil::filetime_now());
    }
}

/// The agent a bound `agentMgr` call speaks to: the moniker's bind params,
/// the bare `agentID` **[V]** (`eveMoniker.GetAgent`, `docs/reference/bound-objects.md`
/// §2.2); else `arg` when it is an id; else `crate::mission::AGENT_ID`, the
/// station agent of the no-bundle server. The client binds every arm that
/// reads this, so the last fallback serves only an unbound call (a test, or a
/// client we have not met) `[A]`; falsified by a capture of an unbound
/// `DoAction`.
fn bound_agent(req: &Request<'_>, arg: Option<&PyValue>) -> i64 {
    match (req.bind_params, arg) {
        (Some(PyValue::Int(id)), _) | (_, Some(PyValue::Int(id))) => *id,
        _ => crate::mission::AGENT_ID,
    }
}

/// `GetAgentLocationWrap`: the bound agent's station and system from the
/// pool, with the station's type from the celestial catalogue. An agent the
/// pool does not hold, or one in space, answers with the caller's own
/// station, as the no-bundle station agent always did. A station the
/// catalogue does not name takes the caller's station type `[A]`; falsified
/// by a dialogue header that shows the wrong station icon.
fn agent_location(reg: &Registry, req: &Request<'_>) -> PyValue {
    let c = req.actor.and_then(|id| reg.character(id));
    let own_type = c.map(|c| c.station_type_id);
    let pooled = reg
        .content()
        .try_get::<crate::mission_pool::Table>()
        .and_then(|t| t.agent(bound_agent(req, None)))
        .and_then(|a| a.station.zip(a.system));
    let (station, type_id, system) = match pooled {
        Some((station, system)) => (
            Some(station),
            reg.celestials()
                .get(station)
                .map(|s| s.type_id)
                .or(own_type),
            Some(system),
        ),
        None => (
            c.and_then(|c| c.station_id),
            own_type,
            c.and_then(|c| c.solar_system_id),
        ),
    };
    let int = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    pydict(vec![
        ("locationID", int(station)),
        ("typeID", int(type_id)),
        ("solarsystemID", int(system)),
    ])
}

/// The `GetAgents` columns, in row order.
pub const AGENT_COLUMNS: [&str; 10] = [
    "agentID",
    "agentTypeID",
    "divisionID",
    "corporationID",
    "factionID",
    "level",
    "stationID",
    // All lowercase, unlike the station rowset's `solarSystemID`.
    "solarsystemID",
    "quality",
    "isLocator",
];

/// One pool agent as a `GetAgents` row, in [`AGENT_COLUMNS`] order.
/// `quality` is always 0: the column is TQ's, and no client reader is known
/// to use it **[I]**.
fn pool_row(a: &crate::mission_pool::Agent) -> PyValue {
    let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    PyValue::List(vec![
        PyValue::Int(a.id),
        PyValue::Int(a.agent_type),
        PyValue::Int(a.division),
        PyValue::Int(a.corp),
        PyValue::Int(a.faction),
        PyValue::Int(a.level),
        opt(a.station),
        opt(a.system),
        PyValue::Int(0),
        PyValue::Bool(a.is_locator),
    ])
}

/// `agentMgr.GetAgents`: every agent in `pool` (all of `agents.tsv` with a
/// bundle, C4 §3.9.1), then a placeholder for each epic-arc starter the pool
/// does not hold, then, when the caller is docked and the pool does not hold
/// it, the no-bundle station agent at the caller's station. No id appears
/// twice: the client `Index`es the rowset on `agentID`.
pub fn agent_rowset(
    pool: Option<&crate::mission_pool::Table>,
    station_id: Option<i64>,
    solar_system_id: Option<i64>,
) -> PyValue {
    let held = |id: i64| pool.is_some_and(|t| t.agent(id).is_some());
    // A real `Rowset` **instance**, not `eve_rowset::Rowset`'s
    // `(header, data)` tuple -- that tuple is the `cfg.Recordset._Prime`
    // form, and this caller wants an object with methods.
    //
    // The class is old-style (`class Rowset:`, empty bases) and defines no
    // `__setstate__`, so the reader builds it with `PyInstance_NewRaw` and
    // installs our dict directly: the state is just `header` and `lines`.
    // `RowClass` is a class attribute with a default, so it must *not* be
    // sent -- `Clone()` reads `self.RowClass` and finds the class's own.
    //
    // The whitelisted name is the dotted module path, not the `__guid__`:
    // the writer builds it as `"%s.%s" % (__module__, cl_name)`.
    instance(
        "eve.common.script.sys.rowset.Rowset",
        vec![
            (
                "header",
                PyValue::List(AGENT_COLUMNS.iter().map(|c| bstr(c)).collect()),
            ),
            // **Every pool agent first.** With a bundle these are all 10,972
            // rows of `agents.tsv`, which the Agency's agent finder and every
            // station's Agents tab read (C4 §3.9.1). The thirteen epic-arc
            // starters are among them, with their real `factionID`s, so the
            // standings panel's threshold icon may now appear for them
            // (C4 §7.4) `[A]`: falsified by a Standings tab that claims an
            // arc threshold before CP41 serves the arcs.
            //
            // `Row.__init__` raises `ValueError` unless
            // `len(line) == len(header)`, so each row is exactly ten
            // elements in header order. That is the failure a later edit
            // would reintroduce, and there is a test for it.
            (
                "lines",
                PyValue::List(
                    pool.into_iter()
                        .flat_map(|t| t.agents())
                        .map(pool_row)
                        // **The epic-arc starters the pool lacks are not
                        // decoration.**
                        //
                        // `standingsUIUtil._GetEpicArcThresholdIDs`
                        // (firstlineno 265) is
                        //
                        //     for agentID in standingUIConst.EPIC_ARC_STARTER_AGENTS:
                        //         agent = sm.GetService('agents').GetAgentByID(agentID)
                        //         if ownerID in (agent.factionID, agent.corporationID):
                        //
                        // with no `is None` guard, and `agents.GetAgentByID`
                        // returns `None` for an id its index does not hold.
                        // A missing row is therefore an `AttributeError:
                        // 'NoneType' object has no attribute 'factionID'` --
                        // with `agentID = 3019415` and `ownerID = 500001`
                        // sitting in the captured locals.
                        //
                        // **It cost the whole Standings tab, on open, with no
                        // click.** The client seeds its own standing row for
                        // the character's racial faction
                        // (`standingsvc.__RefreshStandings`), so the scroll is
                        // never empty, and `standingsPanel.LoadPanel` selects
                        // that row at line 179 without the player touching
                        // anything.
                        //
                        // **Every field except the id is deliberately
                        // `None`** in a placeholder: `ownerID in (None,
                        // None)` is `False`, so no threshold icon is claimed,
                        // and `agentTypeID` is `None` so the Agency's epic-arc
                        // provider (`allAgentsByType`) gets no member whose
                        // station we cannot name. These rows exist to answer
                        // `GetAgentByID`, and for nothing else.
                        .chain(
                            EPIC_ARC_STARTER_AGENTS
                                .iter()
                                .filter(|id| !held(**id))
                                .map(|id| {
                                    PyValue::List(vec![
                                        PyValue::Int(*id),    // agentID
                                        PyValue::None,        // agentTypeID
                                        PyValue::None,        // divisionID
                                        PyValue::None,        // corporationID
                                        PyValue::None,        // factionID
                                        PyValue::Int(1),      // level
                                        PyValue::None,        // stationID
                                        PyValue::None,        // solarsystemID
                                        PyValue::Int(0),      // quality
                                        PyValue::Bool(false), // isLocator
                                    ])
                                }),
                        )
                        .chain(
                            station_id
                                .zip(solar_system_id)
                                .filter(|_| !held(STATION_AGENT_ID))
                                .map(|(station, system)| {
                                    // **The no-bundle station agent, in our
                                    // station, usable without standings**,
                                    // which `crate::mission` serves the
                                    // starter courier from. With a bundle
                                    // it is a pool row like any other.
                                    //
                                    // Level, type and division are copies of
                                    // its `npccharacters` record (see
                                    // `STATION_AGENT_LEVEL`): level 1 with
                                    // `agentType` 2 is the combination
                                    // `eveCfg.CanUseAgent` short-circuits on.
                                    // `divisionID` 24 is Security; see
                                    // `STATION_AGENT_FACTION` for the faction.
                                    PyValue::List(vec![
                                        PyValue::Int(STATION_AGENT_ID),
                                        PyValue::Int(STATION_AGENT_TYPE),
                                        PyValue::Int(STATION_AGENT_DIVISION),
                                        PyValue::Int(crate::model::STATION_AGENT_CORP),
                                        PyValue::Int(STATION_AGENT_FACTION),
                                        PyValue::Int(STATION_AGENT_LEVEL),
                                        PyValue::Int(station),
                                        PyValue::Int(system),
                                        PyValue::Int(0),
                                        PyValue::Bool(false),
                                    ])
                                }),
                        )
                        .collect(),
                ),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    /// The one character in a single-character fixture, as an actor.
    ///
    /// Taken from the fixture rather than from inside the registry: finding
    /// the actor by reading "the first character" is the thing Round 0.3
    /// removed, and a test that did it would hide its return.
    fn actor(reg: &Registry) -> Option<i64> {
        Some(reg.config().characters[0].character_id)
    }

    /// Tests for this subsystem belong here, not in the parent. That is the
    /// other half of why the split exists: two lanes adding a method each
    /// would otherwise contend for one `mod tests`.
    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    ///
    /// It used to check the name against a hand-maintained list in the
    /// parent, which is now derived from these very lists -- so that check
    /// had become a tautology. This probes the dispatcher instead, with a
    /// method that cannot exist: `NoSuchMethod` means we got to the service,
    /// `NoSuchService` means the name is advertised and falls straight
    /// through, which is a refused call, which is a hung client.
    fn call(reg: &mut Registry, service: &str, method: &str) -> Result<PyValue, ServiceError> {
        call_args(reg, service, method, &[])
    }

    /// The same, for the arms whose caller passes arguments. Nothing here
    /// reads them yet, but a test that pins "the answer does not echo the
    /// request" has to be able to make a request with something in it.
    fn call_args(
        reg: &mut Registry,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let actor = actor(reg);
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
    }

    /// **The agent dialogue opens, and every level of the nested unpack is
    /// the right arity.**
    ///
    /// `DoAction`'s answer is consumed by `(a, b), c = DoAction(...)` with no
    /// guard, and then `_, mission_id = a` -- three unpacks before anything is
    /// read. Each one is asserted here, because getting any of them wrong is a
    /// `ValueError` inside the client with nothing on our side to show for it.
    #[test]
    fn the_agent_conversation_unpacks_at_every_level() {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call(&mut reg, "agentMgr", "DoAction").unwrap();
        let PyValue::Tuple(outer) = &got else {
            panic!("DoAction is a 2-tuple, got {got:?}")
        };
        assert_eq!(outer.len(), 2, "((agentSays, actions), lastActionInfo)");

        let PyValue::Tuple(says_and_actions) = &outer[0] else {
            panic!("the first element is itself a 2-tuple")
        };
        assert_eq!(says_and_actions.len(), 2);

        // `_, mission_id = agent_says` -- so this is a 2-tuple too, not a
        // bare message.
        let PyValue::Tuple(agent_says) = &says_and_actions[0] else {
            panic!("agentSaysMsg is (msgInfo, contentID)")
        };
        assert_eq!(agent_says.len(), 2);

        // **No longer empty: there is a mission to offer.** This used to
        // assert emptiness, because an action whose button is RequestMission
        // or ViewMission makes the window auto-call `DoAction` again -- and
        // we had nothing to answer with. Accept and Decline are offered
        // directly, whose buttons do *not* trigger that auto-call, so the
        // offer costs one round trip and no invented greeting.
        let PyValue::List(actions) = &says_and_actions[1] else {
            panic!("availableActions is a list")
        };
        assert_eq!(actions.len(), 2, "Accept and Decline: {actions:?}");
        for a in actions {
            let PyValue::Tuple(pair) = a else {
                panic!("an action is (actionID, actionData)")
            };
            assert_eq!(pair.len(), 2);
        }

        // **Five keys now, as integers.** They are subscripted without
        // `.get`, so every one must be present; `loyaltyPoints` joined the
        // set with the real mission because the dialogue window reads it too.
        // `0` rather than `False` to match the specification in
        // `re/mission-model.md` -- both are falsy in Python 2, and picking
        // one and keeping it is what makes this assertion mean anything.
        for key in [
            "missionCompleted",
            "missionDeclined",
            "missionQuit",
            "missionCantReplay",
            "loyaltyPoints",
        ] {
            assert_eq!(
                crate::pyutil::dict_get(&outer[1], key.as_bytes()),
                Some(&PyValue::Int(0)),
                "lastActionInfo[{key}] is subscripted, not `.get`"
            );
        }
    }

    /// **The three mission reads carry a real mission, and none is `None`.**
    ///
    /// This used to assert all three were *empty* dicts, which was right when
    /// there was no mission. The invariant that survives is the one that
    /// matters: each is a dict rather than `None`, because `None` reaches
    /// `msgArgs.update(result)` and raises `TypeError` outside the only `try`
    /// in `ProcessMessage`, blanking the agent window.
    ///
    /// `GetMissionKeywords` being non-empty is the load-bearing part now:
    /// `{}` would render the raw `{[location]objectiveDestinationID.name}`
    /// token in the offer line rather than the destination's name.
    #[test]
    fn the_mission_reads_carry_a_real_mission_and_never_none() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "GetMissionKeywords",
            "GetMissionBriefingInfo",
            "GetMissionObjectiveInfo",
        ] {
            let got = call(&mut reg, "agentMgr", method).unwrap();
            let PyValue::Dict(entries) = &got else {
                panic!("{method} must be a dict, not {got:?}")
            };
            assert!(
                !entries.is_empty(),
                "{method} is empty; the mission would render as raw tokens"
            );
        }
        // The keyword the prose actually interpolates.
        let kw = call(&mut reg, "agentMgr", "GetMissionKeywords").unwrap();
        assert_eq!(
            crate::pyutil::dict_get(&kw, b"objectiveDestinationID"),
            Some(&PyValue::Int(crate::mission::DESTINATION_ID))
        );
    }

    /// **Accepting the mission spawns its cargo and pushes the journal
    /// invalidation.**
    ///
    /// Both halves matter. Without the cargo the briefing's own red warning
    /// -- *"Remember to move the slaves to your cargo hold before
    /// departure"* -- is a lie. Without `OnAgentMissionChange` the journal
    /// never learns about the mission, because that push is the only
    /// invalidation of its cache short of a session reset.
    #[test]
    fn accepting_spawns_the_cargo_and_tells_the_journal() {
        let mut reg = Registry::new(SandboxConfig::default());
        let before = reg.store().items().len();

        let got = call_args(
            &mut reg,
            "agentMgr",
            "DoAction",
            &[PyValue::Int(crate::mission::ACTION_ACCEPT)],
        )
        .expect("accept is answered");
        assert!(matches!(got, PyValue::Tuple(_)));

        // Four `Group of Elite Slaves`, singleton, so four separate items.
        let cargo: i64 = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == crate::mission::CARGO_TYPE_ID)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(
            cargo,
            crate::mission::CARGO_QUANTITY,
            "the briefing tells the player to load cargo that must exist"
        );
        assert!(reg.store().items().len() > before);

        let pushes: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, n)| n.to_push())
            .collect();
        assert!(
            pushes
                .iter()
                .any(|(event, args)| *event == "OnAgentMissionChange"
                    && args[0] == crate::pyutil::bstr("accepted")),
            "the journal only learns about this from the push, and the first \
             argument is the string it compares: {pushes:?}"
        );

        // Accepting twice must not double the cargo.
        call_args(
            &mut reg,
            "agentMgr",
            "DoAction",
            &[PyValue::Int(crate::mission::ACTION_ACCEPT)],
        )
        .expect("answered");
        let cargo_again: i64 = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == crate::mission::CARGO_TYPE_ID)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(cargo_again, cargo, "accepting twice must not re-spawn");
    }

    /// The agent's location omits `locationType` deliberately.
    ///
    /// `'locationType' in location` gates the `LocationWrapper` branch, whose
    /// `cfg.evelocations.Get` is **unguarded** -- the one place in this
    /// subsystem where a location that cannot be named raises instead of
    /// rendering a placeholder.
    #[test]
    fn the_agent_location_names_our_station_and_skips_the_wrapper() {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call(&mut reg, "agentMgr", "GetAgentLocationWrap").unwrap();
        for key in ["locationID", "typeID", "solarsystemID"] {
            let v = crate::pyutil::dict_get(&got, key.as_bytes());
            assert!(
                matches!(v, Some(PyValue::Int(_))),
                "{key} must be an int, got {v:?}"
            );
        }
        assert!(
            crate::pyutil::dict_get(&got, b"locationType").is_none(),
            "locationType must stay absent: it opens the one unguarded \
             cfg.evelocations.Get in the subsystem"
        );
    }
}

/// Lane B1, the breaker sweep: the epic-arc arms (`docs/lanes/breaker-sweep.md`).
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
                args: &[PyValue::Int(3_008_416)],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .unwrap_or_else(|e| panic!("{service}.{method} must be answered: {e:?}"))
    }

    /// `epicArcController._GetMissionMessageID:66` calls `.get` on it,
    /// and then `.get` on each value: a dict of dicts (CP41 fills it).
    #[test]
    fn epic_arc_messages_is_a_dict() {
        let PyValue::Dict(slots) = call("agentMgr", "GetMessagesForEpicArcMissions") else {
            panic!("a dict")
        };
        for (inner, _) in &slots {
            assert!(matches!(inner, PyValue::Dict(_)), "{inner:?}");
        }
    }

    /// `dockedUI/agents:295-296` and `agentUtil:43,68` test it for truth.
    #[test]
    fn agent_has_epic_missions_is_a_bool() {
        assert_eq!(
            call("epicArcStatus", "AgentHasEpicMissionsForCharacter"),
            PyValue::Bool(false)
        );
    }
}

/// CP26b-1: `GetAgents` from the mission pool, and the bound arms.
#[cfg(test)]
mod pool_tests {
    use super::*;
    use crate::mission_pool as pool;

    /// An invented courier agent, placed at the caller's own station.
    const RUNNER: i64 = 3_999_201;
    /// An invented agent elsewhere: the courier's only destination.
    const FAR: i64 = 3_999_202;
    const COURIER: i64 = 999_301;

    fn agent(id: i64, division: i64, station: Option<i64>, system: Option<i64>) -> pool::Agent {
        pool::Agent {
            id,
            agent_type: pool::TYPE_BASIC,
            division,
            level: 1,
            corp: 1_999_001,
            faction: 599_001,
            station,
            system,
            ..pool::Agent::default()
        }
    }

    /// Invented rows: the runner at `home`, one agent far away, an epic-arc
    /// starter id with no station, and one level-1 courier.
    fn table(home: (i64, i64)) -> pool::Table {
        let courier = pool::Mission {
            id: COURIER,
            name_id: 999_701,
            template: "BasicCourier".into(),
            family: "courier".into(),
            level: Some(1),
            obj_type: Some(crate::mission::CARGO_TYPE_ID),
            obj_qty: Some(1),
            obj_volume_total: Some(1.0),
            ..pool::Mission::default()
        };
        pool::Table::new(
            vec![
                agent(RUNNER, 22, Some(home.0), Some(home.1)),
                agent(FAR, 24, Some(69_000_002), Some(39_000_002)),
                agent(EPIC_ARC_STARTER_AGENTS[0], 24, None, None),
            ],
            vec![courier],
            pool::Policy {
                parents: &[],
                drones: 599_021,
            },
        )
    }

    fn actor(reg: &Registry) -> Option<i64> {
        Some(reg.config().characters[0].character_id)
    }

    /// The caller's station and system, read from the world, not typed in.
    fn home(reg: &Registry) -> (i64, i64) {
        let c = reg.character(actor(reg).unwrap()).unwrap();
        (c.station_id.unwrap(), c.solar_system_id.unwrap())
    }

    fn world() -> Registry {
        let reg = Registry::new(SandboxConfig::default());
        let mut c = crate::content::Content::fixture();
        c.insert("mission_pool", table(home(&reg)));
        reg.with_content(std::sync::Arc::new(c))
    }

    fn call(reg: &mut Registry, method: &str, bound: Option<i64>, args: &[PyValue]) -> PyValue {
        let bind = bound.map(PyValue::Int);
        let actor = actor(reg);
        dispatch(
            reg,
            &Request {
                service: "agentMgr",
                method,
                args,
                kwargs: &[],
                bind_params: bind.as_ref(),
                actor,
            },
        )
        .unwrap_or_else(|e| panic!("agentMgr.{method} must be answered: {e:?}"))
    }

    /// `(header, lines)` of a `GetAgents` answer.
    fn rows(rowset: &PyValue) -> (Vec<PyValue>, Vec<Vec<PyValue>>) {
        let PyValue::Instance { state, .. } = rowset else {
            panic!("a Rowset instance: {rowset:?}")
        };
        let (Some(PyValue::List(header)), Some(PyValue::List(lines))) = (
            crate::pyutil::dict_get(state, b"header"),
            crate::pyutil::dict_get(state, b"lines"),
        ) else {
            panic!("header and lines are lists")
        };
        let lines = lines
            .iter()
            .map(|l| match l {
                PyValue::List(cells) => cells.clone(),
                other => panic!("a row is a list: {other:?}"),
            })
            .collect();
        (header.clone(), lines)
    }

    fn ids(lines: &[Vec<PyValue>]) -> Vec<i64> {
        lines
            .iter()
            .map(|l| match l[0] {
                PyValue::Int(id) => id,
                ref other => panic!("agentID is an int: {other:?}"),
            })
            .collect()
    }

    /// **[V]** `Row.__init__` raises `ValueError` unless `len(line) ==
    /// len(header)`, and the header is the ten columns `Index` and `Filter`
    /// name.
    #[test]
    fn every_rowset_row_has_ten_elements() {
        let mut reg = world();
        let (header, lines) = rows(&call(&mut reg, "GetAgents", None, &[]));
        assert_eq!(header.len(), 10);
        assert_eq!(header.len(), AGENT_COLUMNS.len());
        assert!(!lines.is_empty());
        for l in &lines {
            assert_eq!(l.len(), header.len(), "{l:?}");
        }
    }

    /// The fixture's invented agent appears at its station, with its own
    /// level, type, division, corporation and faction.
    #[test]
    fn the_fixture_agent_appears_at_its_station() {
        let mut reg = world();
        let (station, system) = home(&reg);
        let (_, lines) = rows(&call(&mut reg, "GetAgents", None, &[]));
        let row = lines
            .iter()
            .find(|l| l[0] == PyValue::Int(RUNNER))
            .expect("the runner is served");
        let a = table((station, system)).agent(RUNNER).cloned().unwrap();
        assert_eq!(
            row,
            &vec![
                PyValue::Int(RUNNER),
                PyValue::Int(a.agent_type),
                PyValue::Int(a.division),
                PyValue::Int(a.corp),
                PyValue::Int(a.faction),
                PyValue::Int(a.level),
                PyValue::Int(station),
                PyValue::Int(system),
                PyValue::Int(0),
                PyValue::Bool(false),
            ]
        );
    }

    /// With no bundle the content is the pool's fixture, and every one of its
    /// agents is listed, once.
    #[test]
    fn get_agents_on_a_fixture_world_lists_every_fixture_agent() {
        let mut reg = Registry::new(SandboxConfig::default());
        let (_, lines) = rows(&call(&mut reg, "GetAgents", None, &[]));
        let served = ids(&lines);
        let fixture = pool::Table::fixture();
        for a in fixture.agents() {
            assert!(served.contains(&a.id), "agent {} is missing", a.id);
        }
        assert!(
            served.contains(&STATION_AGENT_ID),
            "the no-bundle station agent stays while the pool lacks it"
        );
        for id in EPIC_ARC_STARTER_AGENTS {
            assert!(served.contains(&id), "epic-arc starter {id} is missing");
        }
    }

    /// The client `Index`es on `agentID`: a pool row replaces the epic-arc
    /// placeholder and the station agent's row, never doubles them.
    #[test]
    fn no_agent_id_appears_twice() {
        let reg = world();
        let (station, system) = home(&reg);
        let mut t = table((station, system));
        let mut both: Vec<pool::Agent> = t.agents().cloned().collect();
        both.push(agent(STATION_AGENT_ID, 24, Some(station), Some(system)));
        t = pool::Table::new(both, vec![], pool::Policy::TRANQUILITY);
        let (_, lines) = rows(&agent_rowset(Some(&t), Some(station), Some(system)));
        let mut served = ids(&lines);
        let n = served.len();
        served.sort_unstable();
        served.dedup();
        assert_eq!(served.len(), n, "an id was served twice");
        // And the epic-arc starter in the pool is its pool row, not the
        // placeholder: a placeholder's `agentTypeID` is `None`.
        let epic = lines
            .iter()
            .find(|l| l[0] == PyValue::Int(EPIC_ARC_STARTER_AGENTS[0]))
            .unwrap();
        assert_eq!(epic[1], PyValue::Int(pool::TYPE_BASIC));
    }

    fn accepted(reg: &Registry, agent: i64) -> bool {
        let me = actor(reg).unwrap();
        crate::mission::progress(reg, me, agent).state == crate::mission::State::Accepted
    }

    /// `DoAction` on `Moniker('agentMgr', agentID)` changes that agent's
    /// mission and no other, and the journal push names it.
    #[test]
    fn do_action_speaks_to_the_bound_agent() {
        let mut reg = world();
        call(&mut reg, "DoAction", Some(RUNNER), &[]);
        call(
            &mut reg,
            "DoAction",
            Some(RUNNER),
            &[PyValue::Int(crate::mission::ACTION_ACCEPT)],
        );
        assert!(accepted(&reg, RUNNER));
        assert!(!accepted(&reg, crate::mission::AGENT_ID));
        let pushes: Vec<_> = reg
            .take_pending()
            .iter()
            .map(|(_, n)| n.to_push())
            .collect();
        assert!(
            pushes
                .iter()
                .any(|(event, args)| *event == "OnAgentMissionChange"
                    && args[0] == bstr("accepted")
                    && args[1] == PyValue::Int(RUNNER)),
            "{pushes:?}"
        );
    }

    /// The three mission reads answer for the bound agent's mission.
    #[test]
    fn the_mission_reads_follow_the_bound_agent() {
        let mut reg = world();
        let brief = call(&mut reg, "GetMissionBriefingInfo", Some(RUNNER), &[]);
        assert_eq!(
            crate::pyutil::dict_get(&brief, b"ContentID"),
            Some(&PyValue::Int(COURIER))
        );
        let kw = call(
            &mut reg,
            "GetMissionKeywords",
            Some(RUNNER),
            &[PyValue::Int(COURIER)],
        );
        assert_eq!(
            crate::pyutil::dict_get(&kw, b"objectiveDestinationID"),
            Some(&PyValue::Int(69_000_002)),
            "FAR's station is the only other one in the pool"
        );
        let obj = call(&mut reg, "GetMissionObjectiveInfo", Some(RUNNER), &[]);
        assert_eq!(
            crate::pyutil::dict_get(&obj, b"contentID"),
            Some(&PyValue::Int(COURIER))
        );
    }

    /// The dialogue header names the bound agent's own station and system.
    #[test]
    fn the_agent_location_is_the_bound_agents() {
        let mut reg = world();
        let got = call(&mut reg, "GetAgentLocationWrap", Some(FAR), &[]);
        assert_eq!(
            crate::pyutil::dict_get(&got, b"locationID"),
            Some(&PyValue::Int(69_000_002))
        );
        assert_eq!(
            crate::pyutil::dict_get(&got, b"solarsystemID"),
            Some(&PyValue::Int(39_000_002))
        );
        assert!(matches!(
            crate::pyutil::dict_get(&got, b"typeID"),
            Some(PyValue::Int(_))
        ));
        assert!(crate::pyutil::dict_get(&got, b"locationType").is_none());
    }
}

/// CP41: the epic-arc arms over the fixture arcs (`epic_arc::fixture_content`).
#[cfg(test)]
mod epic_arc_tests {
    use super::*;

    /// The fixture's first start mission and its agent.
    const START: i64 = 999_501;
    const AGENT_START: i64 = 3_999_401;
    /// The hidden arc's agent.
    const AGENT_HIDDEN: i64 = 3_999_411;

    fn world() -> (Registry, i64) {
        let c = crate::epic_arc::fixture_content();
        let reg = Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(c));
        let me = reg.config().characters[0].character_id;
        (reg, me)
    }

    fn call(
        reg: &mut Registry,
        me: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
        bound: Option<i64>,
    ) -> PyValue {
        let bind = bound.map(PyValue::Int);
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: bind.as_ref(),
                actor: Some(me),
            },
        )
        .unwrap_or_else(|e| panic!("{service}.{method}: {e:?}"))
    }

    /// `GetMyEpicArcStatus` is `{}` for a new character [V], and holds the
    /// start once the start agent's offer is accepted.
    #[test]
    fn status_is_empty_then_holds_the_accepted_start() {
        let (mut reg, me) = world();
        let s = call(&mut reg, me, "agentMgr", "GetMyEpicArcStatus", &[], None);
        assert_eq!(s, PyValue::Dict(vec![]));
        call(&mut reg, me, "agentMgr", "DoAction", &[], Some(AGENT_START));
        let accept = [PyValue::Int(crate::mission::ACTION_ACCEPT)];
        call(
            &mut reg,
            me,
            "agentMgr",
            "DoAction",
            &accept,
            Some(AGENT_START),
        );
        let s = call(&mut reg, me, "agentMgr", "GetMyEpicArcStatus", &[], None);
        let PyValue::Dict(arcs) = &s else {
            panic!("a dict: {s:?}")
        };
        assert_eq!(arcs.len(), 1);
        let PyValue::Dict(ms) = &arcs[0].0 else {
            panic!("a dict of dicts: {s:?}")
        };
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].1, PyValue::Int(START), "keyed by the start mission");
    }

    /// The start agent has an arc mission for a new character; the hidden
    /// arc's agent never does, and its replay timer is falsy.
    #[test]
    fn agent_has_epic_missions_and_replay_answer_per_agent() {
        let (mut reg, me) = world();
        let has = |reg: &mut Registry, agent| {
            call(
                reg,
                me,
                "epicArcStatus",
                "AgentHasEpicMissionsForCharacter",
                &[PyValue::Int(agent)],
                None,
            )
        };
        assert_eq!(has(&mut reg, AGENT_START), PyValue::Bool(true));
        assert_eq!(has(&mut reg, AGENT_HIDDEN), PyValue::Bool(false));
        let replay = call(
            &mut reg,
            me,
            "agentMgr",
            "GetReplayTimestamp",
            &[PyValue::Int(me)],
            Some(AGENT_START),
        );
        assert_eq!(replay, PyValue::None);
    }
}
