//! Everything that only matters once undocked.
//!
//! Split out of `service.rs`, which had grown to 7374 lines in one file --
//! and that, rather than anything about the work itself, was what capped
//! parallel implementation at one agent. A lane working on this subsystem
//! now opens one file and cannot collide with a lane working on another.
//!
//! The arms are unchanged by the move: `self` became the `reg` argument and
//! nothing else. Helpers deliberately stayed in the parent for now, because
//! several are shared across domains and moving those is where a split of
//! this kind goes wrong.

use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "beyonce",
    "crimewatch",
    // The object `scanMgr.GetSystemScanMgr` hands back. Not a machoNet
    // service: the name is synthetic, and the `::` cannot collide with a
    // real one. See `Registry::OBJECT_RETURNING`.
    "scanMgr::systemScanMgr",
    "insurgencySolarsystem",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Destiny's ballpark service, and **the only answer in this file
        // whose consumer is native code rather than Python.**
        //
        // `michelle.AddBallpark` does
        // `sm.RemoteSvc('beyonce').GetFormations()` -- a *plain* service
        // call, not a bound one -- and hands the result straight to
        // `destiny.Ballpark.LoadFormations`, which lives in `blue.dll`.
        //
        // **It must be a tuple, and that is the whole reason this arm
        // exists in its own right.** The first attempt answered an empty
        // *list*, which cost a full run: the client undocked, got all the
        // way to `michelle.py(453) AddBallpark`, and died there on
        // `TypeError: must be tuple, not list` with `formations = []` in
        // the frame. The player saw a **black screen** -- session change
        // applied, station gone, no scene. An empty list and an empty
        // tuple are the same thing to every Python consumer in this file
        // and are two different types to the native one, so this is the
        // single arm where `List` vs `Tuple` is load-bearing.
        //
        // The surviving lesson is narrower than the one originally
        // written here. The prose used to say a wrong shape would take
        // the client down *without a traceback*; it did leave one,
        // because `OnPrimaryViewChanged` runs under
        // `serviceManager.MollycoddledUthread`, which catches and mails
        // it. Native consumers are still the dangerous ones -- but they
        // are dangerous because they are strict about types Python is
        // relaxed about, not because they are silent.
        //
        // It blocks nothing: `AddBallpark` is reached from
        // `michelle.OnPrimaryViewChanged`, and `ScatterEvent` dispatches
        // every handler onto its own tasklet. What it *does* gate is
        // `bpReady` -- until it is answered, `GetBallpark()` returns
        // `None` and `WaitForBallpark` burns 30 x 1 s.
        //
        // **The `Cmd*` family is the rest of this arm, and it has to be
        // echoed rather than merely answered.** Nothing on the client applies
        // a movement command locally -- `michelle.GetRemotePark()` returns
        // this bound object itself, and `CmdSetSpeedFraction` does not appear
        // in `michelle.pyc` at all **[V]**. Answering `None` and pushing
        // nothing gives a throttle slider that moves, an "Approaching..."
        // caption, a drawn trajectory line and a ship that sits still: three
        // local effects that make it look broken rather than unfinished.
        // `docs/lanes/movement.md` §1 is the proof, `crate::space_state` the
        // mapping.
        //
        // They arrive as **blocking `CallReq`s** on this object, because we
        // mint `N=` object ids and the prefix is what sets `blocking`
        // (`docs/reference/bound-objects.md` §1.6) **[V]**. So each one must be
        // answered or the client's call hangs until `machoTimeout`; the return
        // value is discarded at every call site, so `None` is the answer.
        "beyonce" => match req.method {
            "GetFormations" => Ok(PyValue::Tuple(vec![])),
            // **Fleet target tags** (`menusvc.py:2786-2789`): a commander's
            // `CmdFleetTagTarget(itemID, tag)`, `tag` None to untag; the
            // answer is unused. A refusal is dropped, the menu is
            // commander-only. `GetFleetState` is `michelle`'s lazy read of
            // `fleetState` (`michelle.py:283-291`).
            "CmdFleetTagTarget" => {
                if let (Some(actor), Some(PyValue::Int(item))) = (req.actor, req.args.first()) {
                    let tag = match req.args.get(1) {
                        Some(PyValue::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
                        Some(PyValue::Unicode(u)) => Some(u.clone()),
                        _ => None,
                    };
                    if let Ok(e) = reg.fleets_mut().tag(actor, *item, tag.as_deref()) {
                        reg.fleet_apply(e);
                    }
                }
                Ok(PyValue::None)
            }
            // **Give up a wreck's or can's loot rights** (GAP-LOOT,
            // `menuFunctions.AbandonLoot:137`, `AbandonAllLoot:152`): a list
            // of item ids; the answer is unused. Not the owner's: ignored.
            "CmdAbandonLoot" => {
                if let Some(actor) = req.actor {
                    let ids: Vec<i64> = match req.args.first() {
                        Some(PyValue::List(l) | PyValue::Tuple(l)) => l
                            .iter()
                            .filter_map(|v| match v {
                                PyValue::Int(i) => Some(*i),
                                _ => None,
                            })
                            .collect(),
                        Some(PyValue::Int(i)) => vec![*i],
                        _ => vec![],
                    };
                    crate::combat::abandon_loot(reg, actor, &ids);
                }
                Ok(PyValue::None)
            }
            "GetFleetState" => Ok(req
                .actor
                .and_then(|a| reg.fleets().state_of(a))
                .unwrap_or(PyValue::None)),
            // SOC-D1: the location panel's cyno icon (`service/sov.rs`).
            "GetCynoJammerState" => Ok(super::sov::cyno_jammer_state()),
            // "Save Location" in space: the bookmark lives in `world.rs`.
            "BookmarkLocation" => super::world::bookmark_location(reg, req),
            "BookmarkScanResult" => super::world::bookmark_scan_result(reg, req),
            // **The desync alarm, and the only way back from one.**
            // Reachable only from `Park.RequestReset` <- `RealFlushState`'s
            // failed `SynchroniseToSimulationTime`, which has already set
            // `validState = False` -- after which every batch we push is
            // silently dropped with `Events ignored` until a `SetState`
            // arrives (`michelle.py:1076`, `:1260`, `:1527`) **[V]**. The
            // retval is discarded (`michelle.py:1072` is a bare statement),
            // so the answer is `None` and the *push* is the reply that
            // matters.
            //
            // The state is rebuilt from the ship's current motion, not from
            // the undock constants, which is what makes this a recovery
            // rather than a teleport home.
            "UpdateStateRequest" => {
                resend_state(reg, req.actor);
                Ok(PyValue::None)
            }
            m if crate::space_state::is_command(m) => {
                fly(reg, req);
                Ok(PyValue::None)
            }
            // **Through a stargate**: FX now, the ship's ball gone in two
            // seconds, the session change in four (`crate::jump`).
            "CmdStargateJump" => crate::jump::cmd_stargate_jump(reg, req),
            // **To a fleet member's lit cynosural field** (CAP-E): the same
            // shape, answered `None`; the leg is `jump::arm_at`'s.
            "CmdJumpToFleetModuleBeacon" => {
                crate::jump_drive::cmd_jump_to_fleet_module_beacon(reg, req)
            }
            // **Group jump and portals** (CAP-G): the conduit jumps with
            // fleet members riding; a portal opens (no session change) and
            // fleet members jump through it.
            "CmdGroupJumpToFleetModuleBeacon" => {
                crate::jump_drive::cmd_group_jump_to_fleet_module_beacon(reg, req)
            }
            "CmdBridgeToFleetModuleBeacon" => {
                crate::jump_portal::cmd_bridge_to_fleet_module_beacon(reg, req)
            }
            "CmdJumpThroughFleet" => crate::jump_portal::cmd_jump_through_fleet(reg, req),
            // **The way back into the hangar**, and the whole of it on this
            // side: `beyonce.CmdDock(stationID, shipID)`, two arguments,
            // answered `None`. `movementFunctions.RealDock` hands the method
            // to `sessionMgr.PerformSessionChange('dock', ...)`, whose caller
            // discards the result **[V]**, so what docks the ship is the
            // session change `dispatch::session_change_for` puts in front of
            // this answer -- see `Registry::dock_change`.
            //
            // **No `RemoveBalls` push, and that is not an omission.**
            // `michelle.RemoveBallpark` -> `Park.Release` calls
            // `self.RemoveBalls(self.balls.iterkeys(), isRelease=True)`
            // itself at `michelle.py:1196` **[V]**, so the teardown is
            // already done by the time we could say anything. Sending one is
            // actively worse than useless: `RealFlushState:1575-1577` appends
            // its own `exploders` dict as the second positional argument, so
            // a server-sent `(ballIDs, exploders)` arrives as
            // `isRelease = <dict>`.
            //
            // **No `docking::can_dock`**: as CCP wrote it, it answers false
            // at every NPC station for every hull -- see the finding on it
            // in `crate::docking`. **The range is checked**, loosely
            // (`dock_policy::refuse_out_of_range`): the client enforces
            // 2,500 m before it sends (`Registry::dock_target` has the [V]
            // chain) and the server repeats it with a lag allowance.
            //
            // **At the station it names (FX1).** `stationID` is the first
            // argument of both (`RealDock:554`, `:558` [V]); a call without
            // one is the pilot's own station, which is what every dock was
            // before. A station that is not in the pilot's system -- one the
            // client could not have had in its park -- raises `NotDockable`
            // ("You cannot dock with the object you have selected", message
            // 1334, which the client never raises itself) **[V]**. Otherwise
            // `Registry::dock_at` makes it the pilot's station and moves the
            // ship into its hangar, and `Registry::dock_change`, run after
            // this answer is built, reads it from there.
            //
            // **Under a weapons timer**: `CantDockAfterAggression`, which the
            // client never raises itself either (`crate::crimewatch`). Each
            // raise returns before `with_session_change`, so no dock.
            "CmdDock" | "CmdTurboDock" => {
                // CONCORD-1b: a criminal's words come first.
                crate::concord::refuse_dock(reg, req.actor, crate::pyutil::filetime_now())?;
                crate::crimewatch::refuse_dock(reg, req.actor, crate::pyutil::filetime_now())?;
                // SOC-D3a: nor does a ship with an Entosis Link running dock.
                crate::entosis::refuse_dock(reg, req.actor)?;
                crate::cyno::refuse_dock(reg, req.actor)?;
                // NW-1: nor a ship with a siege, triage or core module running.
                crate::module_fuel::refuse_dock(reg, req.actor)?;
                // MJD-MASS: nor while a Micro Jump Drive spools.
                crate::mjd::refuse_dock(reg, req.actor)?;
                // CLOAK-1: a cloaked ship does not dock.
                crate::cloak::refuse_dock(reg, req.actor)?;
                if let Some(actor) = req.actor {
                    let named = crate::pyutil::opt_id(req.args.first());
                    // SEC-1d: an outlaw's ship is turned away from high-sec.
                    crate::sec_status::refuse_dock(
                        reg,
                        req.actor,
                        named.or_else(|| reg.character(actor).and_then(|c| c.station_id)),
                    )?;
                    // CAP-K: a hull too large for an NPC station.
                    crate::dock_policy::refuse_dock(
                        reg,
                        req.actor,
                        named.or_else(|| reg.character(actor).and_then(|c| c.station_id)),
                    )?;
                    // The client's 2,500 m gate, repeated: a modified
                    // client could name any station in the system.
                    crate::dock_policy::refuse_out_of_range(
                        reg,
                        req.actor,
                        named.or_else(|| reg.character(actor).and_then(|c| c.station_id)),
                    )?;
                    let station = named
                        .or_else(|| reg.character(actor).and_then(|c| c.station_id))
                        .and_then(|id| reg.dock_target(actor, id));
                    if !station.is_some_and(|s| reg.dock_at(actor, s)) {
                        return Err(ServiceError::Raise(eve_proto::user_error(
                            NOT_DOCKABLE,
                            vec![],
                        )));
                    }
                }
                undocked_no_longer(reg, req.actor);
                Ok(PyValue::None)
            }
            // **Align to an object, which the new geometry makes
            // answerable.** This sat in `UNIMPLEMENTED` with the note "needs
            // a celestial or bookmark position to turn into a heading", and
            // half of that is no longer true: `Registry::ball_position`
            // resolves every ball this server puts in a park, which is the
            // station and the other ships. A bookmark or a celestial we have
            // no catalogue for still answers `None` and pushes nothing, which
            // is the old behaviour for the cases that have not changed.
            "CmdAlignTo" => {
                align(reg, req);
                Ok(PyValue::None)
            }
            // Approach a saved point: the same point `align` turns toward,
            // flown to with GotoPoint (GAP-QUICKFIX B6).
            "CmdGotoBookmark" => {
                goto_bookmark(reg, req);
                Ok(PyValue::None)
            }
            // **Warp**, the last item of round 2. Both calls discard their
            // return value and both must still be *answered*, or the calling
            // tasklet blocks to `machoTimeout`.
            //
            // `CmdWarpToStuff(subject, subjectID, **kw)` is 2 positional and
            // keyword `minRange`/`fleet`; `CmdWarpToStuffAutopilot(destID)`
            // is one positional with no subject at all, which is the
            // autopilot's own route and means "an item" **[V]**.
            "CmdWarpToStuff" => {
                let subject = req
                    .args
                    .first()
                    .and_then(crate::pyutil::as_bytes)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                let min_range = crate::space_state::warp_min_range(req.kwargs);
                // A fleet warp to oneself is the regroup: the commander
                // stays (SOC-C7, `crate::fleet_warp`).
                let own = if crate::fleet_warp::is_to_self(req, &subject) {
                    None
                } else {
                    warp(reg, req, subject.clone(), req.args.get(1), min_range)
                };
                // A movement guard's refusal is raised (W16 CP57).
                match own {
                    Some(refusal) => Err(refusal.into_error()),
                    None => {
                        // `fleet=True`: the commander's fleet follows.
                        crate::fleet_warp::follow(reg, req, &subject, |reg, one| {
                            warp(reg, one, subject.clone(), one.args.get(1), min_range)
                        });
                        Ok(PyValue::None)
                    }
                }
            }
            "CmdWarpToStuffAutopilot" => {
                // Raised like the right-click warp's (W16 CP57); the
                // autopilot catches it itself (`autopilot.py:587`).
                match warp(
                    reg,
                    req,
                    crate::space_state::WARP_SUBJECT_ITEM.to_vec(),
                    req.args.first(),
                    // **The server picks the autopilot's range, and this
                    // read `0` until self-review.** The autopilot sends one
                    // positional argument and no `minRange` at all, so a
                    // server that reads the keyword gets nothing and warps
                    // the ship *onto* the object. CCP has a constant for
                    // exactly this -- `appConst.minAutoPilotWarpInDistance`
                    // -- and the behaviour needs it: an autopilot has to
                    // land near enough to a gate to jump, not on top of it.
                    // `[A]`, `docs/re/warp.md` §6.3; falsified by an
                    // autopilot that lands too far from a gate to use it.
                    crate::space_state::AUTOPILOT_WARP_IN,
                ) {
                    Some(refusal) => Err(refusal.into_error()),
                    None => Ok(PyValue::None),
                }
            }
            // Answered by name, with nothing pushed, because each needs
            // something we do not have: a bookmark position to turn into a
            // heading, or the WARP mode tail. Answering by name rather than
            // letting `no_method` do it keeps `UNANSWERED` meaning something
            // -- it would otherwise print once per throttle drag.
            m if crate::space_state::UNIMPLEMENTED.contains(&m) => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Crimewatch: timers, engagements and flagged characters.
        //
        // Bound on `(stationID, groupStation)`, and it fires in the
        // **blocking** `ProcessSessionChange` wave because `charid` is in
        // the change -- so it holds the session write lock while it waits
        // for us.
        //
        // The shape is not negotiable, because the caller destructures it
        // immediately and to three levels:
        //
        //     myCombatTimers, myEngagements, flaggedCharacters, _ = GetClientStates()
        //     weaponTimerState, pvpTimerState, npcTimerState, \
        //         criminalTimerState, disapprovalTimerState = myCombatTimers
        //     criminals, suspects = flaggedCharacters
        //
        // and each of the five timer states is further unpacked into
        // `(state, expiry)`. A peaceful capsuleer is all five timers
        // inactive, no engagements, nobody flagged.
        //
        // Getting the nesting wrong raises rather than hangs, which is
        // the better failure of the two. The timers are live now
        // (`crate::crimewatch`): an idle one is its idle state (100, 200,
        // ...), never `0`, which `HaveWeaponsTimer` reads as armed.
        "crimewatch" => match req.method {
            // The shape, the idle states and the flags are
            // `crate::crimewatch::client_states`'s; the timers are combat's.
            "GetClientStates" => Ok(crate::crimewatch::client_states(
                reg,
                req.actor,
                crate::pyutil::filetime_now(),
            )),
            // **A float, and `None` here was never tolerated** -- it was
            // silently answering a different question.
            //
            // We answered `None` for several runs and no traceback
            // followed, which looked like evidence that the caller did
            // not care. It is not. In Python 2 `None` orders *below every
            // number*, so `GetSecurityWhereAttacked`'s first test --
            // `if characterSecurity <= -4.5` -- is `True` with `None`,
            // and the client concluded that this brand-new character is a
            // -4.5 outlaw attackable by faction police down to 0.5
            // security space. No exception, no log line, just a wrong
            // answer carried into the character sheet's tooltip and the
            // agent-dialogue route warnings.
            //
            // `0.0` is also what the client's own base implementation
            // returns -- `baseDogmaLocation.GetCharacterSecurityStatus`
            // is literally `return 0.0` -- and it matters that it is a
            // float rather than an integer zero: the value reaches
            // `localization.formatters.FormatNumeric(..., decimalPlaces=1)`,
            // a native call, and is written into the dogma attribute
            // `attributePilotSecurityStatus` by
            // `_ApplyPilotSecurityStatusToShip` every time a pilot is put
            // in a hull -- which, for us, means every time the capsule is
            // boarded.
            //
            // The caching guard is `if reg.mySecurityStatus is None`, so
            // `None` also meant the value never cached and every reader
            // re-issued the bound call. The `is None` tell from
            // `docs/guides/debugging-the-client.md` section 4, again.
            //
            // W17 CP50: the caller's stored status (`npc::bounty`), still
            // the float `0.0` with no row.
            "GetMySecurityStatus" => Ok(PyValue::Real(
                req.actor.map_or(crate::npc::bounty::CLEAN, |me| {
                    crate::npc::bounty::security_status(reg, me)
                }),
            )),
            // The same number for somebody else, and **CCP's own default**:
            // `eve/common/script/dogma/baseDogmaLocation.py:2615` is literally
            // `return 0.0`, with `clientDogmaLocation.py:920` and
            // `fittingDogmaLocation.py:807` agreeing. Rule zero paid again --
            // the shared client/server code had the value.
            //
            // One of the three calls `characterInfoWindow` fires in a single
            // `uthread.parallel`, and `uthread.Parallel` re-raises in the
            // caller, so this is only cosmetic *given* the other two are
            // answered. Alone it changes which line the window dies on.
            //
            // W17 CP50: the argument's stored status (`npc::bounty`), the
            // float `0.0` for a clean record, an unknown id or no argument.
            "GetCharacterSecurityStatus" => Ok(PyValue::Real(
                crate::pyutil::opt_id(req.args.first()).map_or(crate::npc::bounty::CLEAN, |id| {
                    crate::npc::bounty::security_status(reg, id)
                }),
            )),
            // Same domain, same type, read by the character sheet's
            // security panel; iterated only.
            //
            // SEC-1a: the history (`crate::sec_status`), newest first.
            "GetSecurityStatusTransactions" => Ok(req.actor.map_or(PyValue::List(vec![]), |me| {
                crate::sec_status::transactions_value(reg, me)
            })),
            // SEC-1c: the security office's one call.
            "ApplyTagsForSecStatus" => crate::security_office::apply(reg, req),
            // CW-1: the safety setting and duels (`crate::crimewatch::service`).
            m if crate::crimewatch::service::METHODS.contains(&m) => {
                crate::crimewatch::service::dispatch(reg, req)
            }
            _ => Err(reg.no_method(req)),
        },
        // Pirate insurgencies. Iterated unconditionally by
        // `GetSolarsystemPirateFactionForCampaign` and friends, so `[]`.
        //
        // Fix this **with** `fwWarzoneSolarsystem`, not after it: it is
        // reached from `infoPanelInsurgency.IsAvailable` in the *same*
        // comprehension, so repairing one and not the other leaves the
        // panel container aborting one panel later.
        // The system scan manager, the object `scanMgr.GetSystemScanMgr`
        // hands back. See `Registry::OBJECT_RETURNING` for why the name
        // has a `::` in it.
        //
        // **This is the third live-lock variant**, and the one that breaks
        // the §4 tell. `scanSvc.GetScanMan`:
        //
        // ```text
        //     if self.remoteObject is None:
        //         self.remoteObject = sm.RemoteSvc('scanMgr').GetSystemScanMgr()
        //     return self.remoteObject
        // ```
        //
        // Our `None` is **successfully stored** -- the
        // `contractProxy.NumOutstandingContracts` mechanism, not the LP
        // one -- so the cache holds the single value that fails its own
        // guard and every reader re-issues the call. Unlike
        // `contractProxy` the throw is *not* swallowed, because the
        // dereference happens in the caller's frame
        // (`sensorSuiteService.FullyUpdateSignalTrackerDebounced`:
        // `self.scanSvc.GetScanMan().GetFullState()`), so it arrives as
        // `AttributeError: 'NoneType' object has no attribute
        // 'GetFullState'` under `uthread2/debounce.py(170) invoke` and is
        // mailed to us by `alert.SendClientStackTraceAlert`.
        //
        // And unlike both of them the cadence is **~7 s, not a frame**:
        // `FullyUpdateSignalTrackerDebounced` is `@uthread2.debounce(1.0)`,
        // so the loop is rate-limited by the decorator rather than by the
        // UI. The §4 rule "a repeat count in the *hundreds* means a live
        // lock" therefore mis-files this one as a chatty UI. The repeat
        // count was eight-ish per minute and it would have run forever.
        //
        // Worth knowing where the clock comes from, because it is ours:
        // `evedungeons/client/anomalyTracker.BaseCosmicAnomalyTracker.get_sites`
        // ends with `if IsDocked(): sm.GetService('sensorSuite')
        // .FullyUpdateSignalTrackerDebounced()`, and it only gets that far
        // because we *do* answer the nine `dungeonInstanceCacheMgr`
        // listings above. So this fires **while docked**, on the Job
        // Board's 3 s refresh, not only after an undock.
        "scanMgr::systemScanMgr" => match req.method {
            // **A 4-tuple of dicts**, and for once we have CCP's own
            // answer for "nothing here" rather than an inference.
            // `explorationscanner/signalTracker.SignalTracker.GetFullState`
            // -- the real server-side implementation, shipped in the
            // client tree -- opens with
            //
            // ```text
            //     if is_sensor_overlay_suppressed(self.solarSystemID):
            //         return ({}, {}, {}, {})
            //     ...
            //     return (anomalies, signatures, staticSites, structures)
            // ```
            //
            // so `({}, {}, {}, {})` is not a stub: it is the branch CCP's
            // own code takes for a system with no overlay, and the four
            // dicts in the live branch are built as `{}` keyed by
            // `signatureID` / `structureID`.
            //
            // The consumer pins both halves of the shape.
            // `sensorSuiteService.OnSignalTrackerFullState` does
            //
            // ```text
            //     anomalies, signatures, staticSites, structures = fullState
            // ```
            //
            // on the line after a `LogInfo`, so the **arity is exactly
            // four** and `UNPACK_SEQUENCE` runs before anything is tested
            // -- the §4 third row. Each element then goes to
            // `spacesitecontroller.ProcessSiteDataUpdate`, which iterates
            // it with `addedSites.iteritems()`, so they are **dicts and
            // not lists**. A list survives only while it is empty, because
            // the caller writes `addedSites = rawSites or dict()`; the
            // first non-empty answer would be an `AttributeError` instead.
            //
            // Nothing native touches it and nothing is written back onto
            // the values, so plain dicts are enough -- no rowset, no
            // `KeyVal`. The values, when we have sites to report, are
            // `CosmicAnomalyInfo` / `SignatureInfo` / `StaticSiteInfo` /
            // `StructureInfo` from `explorationscanner/common/`.
            //
            // With all four empty the client's loop finds nothing to add
            // and nothing to remove, skips `ProcessSiteUpdate` entirely,
            // and the overlay simply shows an empty system -- which is the
            // truth.
            //
            // W12 CP17: the four dicts are filled from the caller's system
            // by `crate::signal_tracker`; empty while it has no sites.
            "GetFullState" => Ok(crate::signal_tracker::full_state(reg, req.actor)),
            // **The directional scanner.** Not one of the nine uses of
            // `scanSvc.remoteObject`, which is why the sensor-suite lane's
            // census missed it: `directionalScanSvc.DirectionalScan:54`
            // reaches the same object through `scanSvc.GetScanMan()` and
            // calls `ConeScan(scanAngle, scanRange, *direction)` **[V]**.
            // `crate::scan::cone_scan` has the shape.
            "ConeScan" => crate::scan::cone_scan(reg, req.actor, req.args),
            // Probe work (P5.10, `docs/lanes/scan.md`). Each is
            // `GetScanMan().<method>` in `scanSvc.pyc`; `crate::scan` has
            // the call and result shapes with their lines.
            "RequestScans" => {
                crate::scan::request_scans(reg, req.actor, req.args, crate::pyutil::filetime_now())
            }
            "RecoverProbes" => {
                crate::scan::recover_probes(reg, req.actor, req.args, crate::pyutil::filetime_now())
            }
            "DestroyProbe" => Ok(crate::scan::destroy_probe(reg, req.actor, req.args)),
            "ReconnectToLostProbes" => Ok(crate::scan::reconnect(
                reg,
                req.actor,
                crate::pyutil::filetime_now(),
            )),
            // Left to `no_method`, so the first one to happen names itself
            // in the `UNANSWERED` line: `SetActivityState` and
            // `QAOverrideProbeExpiry` (returns discarded, so the
            // dispatcher's `None` is right) and `QAScanSites` (guarded by
            // `if sites:`, so `None` is survivable). No site exists for
            // `QAScanSites` to find.
            _ => Err(reg.no_method(req)),
        },
        "insurgencySolarsystem" => match req.method {
            "GetAllVisibleCampaigns" => Ok(PyValue::List(vec![])),
            // **The only confirmed in-space failure**, and it fires four
            // seconds after every undock.
            // `insurgencyCampaignSvc.py(68)`:
            //
            //     solarsystemID, campaignSnapshot = \
            //         sm.RemoteSvc('insurgencySolarsystem').GetLocalCampaignClientSnapshot()
            //     if solarsystemID != session.solarsystemid2:
            //         solarsystemID, campaignSnapshot = ...   # retries ONCE
            //     if solarsystemID == session.solarsystemid2:
            //         reg._localCampaignSnapshot = campaignSnapshot
            //
            // Two elements, by `UNPACK_SEQUENCE`, so `None` is the §4
            // third row -- a `TypeError` before anything gets to test it.
            //
            // **The first element must be the caller's own solar system**, read
            // from the session (`Registry::session_system`). A mismatch is not an
            // error: it costs a second call on every read and leaves the
            // cache at its `LOCAL_CAMPAIGN_UNKNOWN` sentinel forever,
            // which is a wasteful answer with no symptom.
            //
            // `None` is right for the snapshot itself, and this is the
            // unusual case where it is *better* than empty: the sentinel
            // for "not yet known" is the module-level string
            // `LOCAL_CAMPAIGN_UNKNOWN`, not `None`, so storing `None`
            // satisfies the `is LOCAL_CAMPAIGN_UNKNOWN` guard and stops
            // the call repeating. Every consumer branches on
            // `if snapshot is not None` or `if snapshot:`.
            "GetLocalCampaignClientSnapshot" => Ok(PyValue::Tuple(vec![
                // The caller's session system, not the first configured
                // character's home: a pilot elsewhere never matched and sat
                // at the sentinel (owner log 2026-10-29, `fsmState` on a `str`).
                PyValue::Int(req.actor.and_then(|a| reg.session_system(a)).unwrap_or(0)),
                PyValue::None,
            ])),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// `NotDockable`, `UI/Messages/1334`: "You cannot dock with the object you
/// have selected" (body 259124), no arguments, `raise_sites` 0 in the client
/// (`findings/user-errors.tsv:3069`) **[V]**. What `CmdDock` raises for a
/// station outside the pilot's system (`Registry::dock_target`).
pub(crate) const NOT_DOCKABLE: &str = "NotDockable";

/// Tell everyone watching that this ship has left the grid.
///
/// Called from the `CmdDock` arm, **before** `Registry::dock_change` takes
/// the character out of the roster -- the audience and the ball id both come
/// from the presence that is about to be removed, so the order is not
/// incidental. `dock_change` runs afterwards, from
/// `dispatch::session_change_for`.
///
/// The docking player themselves is not in the audience and must not be:
/// their own park is torn down client-side by `Park.Release`, which calls
/// `RemoveBalls` on itself **[V]**, and a push arriving after that is
/// discarded while one arriving before it is pointless.
pub(crate) fn undocked_no_longer(reg: &mut Registry, actor: Option<i64>) {
    let Some(actor) = actor else { return };
    let Some(here) = reg.roster().get(actor).copied() else {
        return;
    };
    let watching = reg.roster().others_in_system(here.solar_system_id, actor);
    let stamp = reg.next_destiny_stamp();
    // To the parks that hold the ship, as it leaves (`crate::grid::filter`):
    // a pilot on another grid never had it.
    if let Some(removed) = crate::destiny::BallsRemoved::new(watching, stamp, vec![here.ship_id]) {
        reg.push(removed);
    }
    // And the docker's own park is over: `Park.Release` empties it client
    // side, and the next undock starts a new one at its `SetState`.
    crate::grid::forget(reg, actor);
}

/// Push a fresh full state to a character, if there is one to build.
///
/// The desync recovery. Built from the ship's current motion, so it is a
/// recovery rather than a teleport home.
pub(crate) fn resend_state(reg: &mut Registry, actor: Option<i64>) {
    let Some(actor) = actor else { return };
    // Docked in a structure: the structure is the ego, never the ship.
    let state = if crate::structure_dock::docked_in(reg, actor).is_some() {
        crate::structure_dock::docked_state(reg, actor)
    } else {
        crate::destiny::bootstrap_for(reg, actor)
    };
    let Some(state) = state else {
        return;
    };
    // Mid-session: its own stamp, past what the client may still hold.
    if let Some(update) = crate::destiny::mid_session_state(reg, actor, state) {
        reg.push(update);
    }
}

/// Point the ship at an object and open the throttle.
///
/// **Align is a heading, and we already know how to push one.** The command
/// resolves `dstID` to a position, turns "where it is from here" into a
/// direction, and pushes the same `GotoDirection` a click in space pushes --
/// which `Motion::after` extends to a point [`GOTO_DISTANCE`] away, so the
/// ship flies past rather than parking on top of what it aligned to. That is
/// what align means: a course, not a destination.
///
/// **`[A]`: it also opens the throttle to full.** In the client an align is
/// followed by the ship accelerating, and the honest reading is that align
/// implies full speed rather than inheriting whatever notch the player left
/// the slider at. Unobserved, and cheap to be wrong about in either
/// direction: too much throttle is a player pulling the slider back, too
/// little is a ship that aligns and does not move. Falsified by aligning at
/// half throttle and watching the slider -- if the real server leaves it
/// alone, the slider will jump to full and the player will notice.
///
/// Two actions in one batch with one stamp, which is the shape the throttle
/// pair already proved: a standing start sends `CmdGotoDirection` and then
/// `CmdSetSpeedFraction`, and both land.
///
/// `None` and no push for a destination we cannot resolve -- a bookmark, or a
/// celestial in a system whose catalogue we do not have. The client discards
/// the return value either way, so a refusal is silent, which is right: the
/// alternative is inventing a heading, and a ship that flies off toward
/// nothing is worse than a ship that does not move.
fn align(reg: &mut Registry, req: &Request<'_>) {
    let Some(actor) = req.actor else { return };
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return;
    };
    // `dstID` is an object; with none, `bookmarkID` names a saved point.
    let there = match crate::space_state::align_destination(req.kwargs) {
        Some(destination) => reg.ball_position(destination),
        None => crate::space_state::align_bookmark(req.kwargs)
            .and_then(|id| super::world::bookmark_target(reg, actor, id))
            .map(|t| t.position),
    };
    let Some(there) = there else {
        return;
    };
    // UNDOCK-INVULN: aligning is flying.
    crate::invuln::break_for(reg, actor);
    crate::cloak::break_jump(reg, actor);
    let heading = [
        there[0] - here.motion.position[0],
        there[1] - here.motion.position[1],
        there[2] - here.motion.position[2],
    ];
    // A ship already exactly on top of what it is aligning to has no heading
    // to take, and `Motion::after` would turn the zero vector into "go to
    // where you already are". Refusing is the same answer with no push.
    if crate::space_state::unit(heading).is_none() {
        return;
    }
    let actions = vec![
        crate::destiny::Action::GotoDirection {
            ball: here.ship_id,
            direction: heading,
        },
        crate::destiny::Action::SetSpeedFraction {
            ball: here.ship_id,
            fraction: 1.0,
        },
    ];
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        for action in &actions {
            p.motion.after(*action);
        }
    }
    let stamp = reg.next_destiny_stamp();
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(here.solar_system_id, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        reg.push(update);
    }
}

/// `CmdGotoBookmark(bookmarkID)`: fly to a saved point in this system.
fn goto_bookmark(reg: &mut Registry, req: &Request<'_>) {
    let Some(actor) = req.actor else { return };
    let Some(id) = req.args.first().and_then(|v| match v {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }) else {
        return;
    };
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return;
    };
    let Some(target) = super::world::bookmark_target(reg, actor, id) else {
        return;
    };
    crate::invuln::break_for(reg, actor);
    crate::cloak::break_jump(reg, actor);
    let actions = vec![
        crate::destiny::Action::GotoPoint {
            ball: here.ship_id,
            point: target.position,
        },
        crate::destiny::Action::SetSpeedFraction {
            ball: here.ship_id,
            fraction: 1.0,
        },
    ];
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        for action in &actions {
            p.motion.after(*action);
        }
    }
    let stamp = reg.next_destiny_stamp();
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(here.solar_system_id, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, actions) {
        reg.push(update);
    }
}

/// Enter warp toward an object.
///
/// **One message, and the client does the rest.** The native engine runs
/// align, activate, accelerate, cruise, decelerate and park with no further
/// server input, and ends the warp itself -- `Ballpark::Evolve`'s arrival
/// branch snaps the position, zeroes the velocity, fires `DST_WARPEXIT` and
/// tail-jumps into `SetMode(DSTBALL_STOP)` **[V]**. So there is no phase
/// machine here and no timer.
///
/// `docs/re/warp.md` is the spec. Three things it settles that this function
/// depends on:
///
/// - the **true** destination goes on the wire, not the offset one: the
///   client applies `minimumRange` itself, so pre-offsetting lands the ship
///   twice as short;
/// - `warpFactor` is milli-AU/s and the native clamps `<= 0` to `1`, which
///   is 0.001 AU/s and looks exactly like a hang -- so
///   `destiny::warp_factor_in` clamps to a visible speed instead;
/// - the batch is **not** time-synchronised beyond one distinct stamp.
///
/// **The tunnel is sent**, and the reason it was withheld for a round turned
/// out to be a misreading of our own notes. `docs/re/warp.md` §8 gave nine
/// arguments; `docs/re/jump-gates.md` §6 gave "fourteen positionals". Those
/// are not two claims about one function -- `Park.OnSpecialFX` has fourteen
/// parameters of which **five carry defaults** (`argcount 15` counting
/// `self`, `MAKE_FUNCTION 5` in the `Park` class body) **[V]**, so nine
/// required positionals and fourteen parameters are the same fact. See
/// `destiny::SpecialFx`, which carries the evidence and the three early
/// returns in `fxSequencer.OnSpecialFX` that this clears.
///
/// Anything whose destination we cannot resolve -- a bookmark, a scan
/// result, a celestial in a system we have no catalogue for -- is answered
/// `None` with nothing pushed, which is exactly what `CmdWarpToStuff` did
/// when it sat in `UNIMPLEMENTED`.
///
/// A `UserError` would be the better answer and is deliberately not taken
/// yet: `CmdWarpToStuffAutopilot`'s call site wraps itself in
/// `try/except UserError` **[V]** but `CmdWarpToStuff`'s does not, so a raise
/// on the ordinary right-click path has no established catcher and the cost
/// of being wrong about that is an unhandled exception in the player's
/// client. Silence is the same thing the player saw yesterday.
///
/// **A movement guard's refusal is the exception** (W16 CP57): it is
/// returned, and the callers raise it. `docs/content/R-TACKLE.md` §3 found
/// the catcher this paragraph was missing -- an uncaught `UserError` on the
/// right-click path reaches `eve_exception_handler.handle_exception`, which
/// shows it as the dialog (`eve_exception_handler.py:47-48`) [V] -- and the
/// autopilot's `except UserError` reads `warpScrambleStatus` itself.
pub(super) fn warp(
    reg: &mut Registry,
    req: &Request<'_>,
    subject: Vec<u8>,
    destination: Option<&PyValue>,
    min_range: f64,
) -> Option<crate::movement_guard::Refusal> {
    warp_guarded(
        reg,
        req,
        subject,
        destination,
        min_range,
        &crate::module::plan().warp_guards,
    )
}

/// [`warp`], checked against `guards` rather than
/// [`crate::movement_guard::WARP_GUARDS`]: the seam's own test drives it with
/// a local table (W10.5 seam S12).
///
/// `Some` only for a guard's refusal; every other early return (no actor,
/// nothing to warp to) is `None` with nothing pushed, as before, and the
/// `?`s below are those.
pub(crate) fn warp_guarded(
    reg: &mut Registry,
    req: &Request<'_>,
    subject: Vec<u8>,
    destination: Option<&PyValue>,
    min_range: f64,
    guards: &[crate::movement_guard::Guard],
) -> Option<crate::movement_guard::Refusal> {
    let actor = req.actor?;
    // **A scan result's id is a string** (`ABC-123`), so it cannot go
    // through `opt_id`; its `crate::warp_subject` row resolves it from the
    // raw argument. `crate::scan::WARP_SUBJECT_SCAN` has the call site.
    let is_scan = subject.as_slice() == crate::scan::WARP_SUBJECT_SCAN;
    let scan_destination = destination.filter(|_| is_scan).cloned();
    let destination = if is_scan {
        0
    } else {
        crate::pyutil::opt_id(destination)?
    };
    reg.advance_character(actor);
    // **Refused before anything changes** (W10.5 seam S12), with nothing
    // pushed; the refusal goes back to be raised (W16 CP57).
    if let Some(refusal) = crate::movement_guard::check(guards, reg, actor) {
        return Some(refusal);
    }
    // UNDOCK-INVULN: a warp gives up the protection.
    crate::invuln::break_for(reg, actor);
    crate::cloak::break_jump(reg, actor);
    // **A ship warped to is carried to now first** (mine-range): the roster
    // holds a ship where it was at its last command, so a bot sent to a pilot
    // who has been flying since landed where that pilot *was* -- beside a belt
    // the pilot had left 150 km behind -- and mined from there.
    let flown = match subject.as_slice() {
        s if s == crate::space_state::WARP_SUBJECT_CHAR => Some(destination),
        s if s == crate::space_state::WARP_SUBJECT_ITEM => {
            reg.roster().by_ship(destination).map(|p| p.character_id)
        }
        _ => None,
    };
    if let Some(character) = flown.filter(|c| *c != actor) {
        reg.advance_character(character);
    }
    let here = reg.roster().get(actor).copied()?;
    // The subjects and what their ids name: `crate::warp_subject` (W10.5
    // seam S8), whose rows were this function's `match subject`.
    let target = crate::warp_subject::resolve(
        crate::warp_subject::RESOLVERS,
        reg,
        &subject,
        actor,
        destination,
        scan_destination.as_ref(),
    );
    let target = target?;
    // **Never inside the thing you warped to**, and computed **once**.
    //
    // The client offsets the destination by `minimumRange` from the centre we
    // send, so a floor of the target's own radius plus a standoff is what
    // keeps the ship out of the model -- a player warped to the station and
    // landed 16 km inside it when this was the player's `0`. A larger request
    // wins, because "warp to within 100 km" is an instruction and this is
    // only a floor.
    //
    // It is a `let` and not an expression inside the action because **two**
    // things need it now: the action, and the arrival batch at the far end.
    // Written twice, the second copy used the player's raw `min_range` and
    // the reconciliation would have snapped the ship to the planet's
    // *centre* at the end of every warp -- reintroducing, from the other
    // side, the exact defect the standoff exists to prevent.
    // `a_warp_schedules_its_own_arrival` caught it before it shipped.
    //
    // **Aim at the client's own warp-in point when the target has one.**
    // Until 2026-10-24 this aimed at the centre with a `radius + 1000` floor
    // and nothing else, which put a ship 1 km above a 5,060 km planet -- on
    // its surface, as the owner reported from playing. Worse, the floor was
    // applied *along the approach*, so the landing point depended on where
    // the pilot came from and two pilots warping to one planet from
    // different systems landed in different places.
    //
    // The client has the real rule, and `crate::warp_in` carries it with the
    // bytecode citations: one point per body, shared by everyone, and
    // `minRange` measured from that point. So when a warp-in point exists it
    // *is* the destination and needs no floor -- it already clears the body
    // by thousands of kilometres. The old floor survives only for the
    // targets that have no warp-in point (ships, stations, belts,
    // stargates), where it is still the thing that keeps a ship out of a
    // station model.
    let (aim, floor) = match target.warp_in {
        Some(point) => (point, 0.0),
        None => (
            target.position,
            target.radius + crate::space_state::WARP_STANDOFF,
        ),
    };
    let standoff = min_range.max(floor);
    // **The minimum warp distance** (WARP-MIN): the landing point must be
    // further than 150 km from the ship, as the client's own menu gate
    // requires (`sessionChecker.isInWarpRange`). A target on grid, a fleet
    // member beside you included, is approached, not warped to.
    if let Some(refusal) = too_short(here.motion.position, aim, standoff) {
        return Some(refusal);
    }
    // **A warp bubble on the line drags the ship out of warp at its edge**
    // (BUBBLES-SB, `crate::bubble::drag`): the landing point becomes the
    // entry point, with no standoff. Every pilot of a fleet warp is dragged
    // on his own line, as each reaches here through `warp`.
    let (aim, standoff) = match crate::bubble::drag(reg, actor, aim, standoff) {
        Some(edge) => (edge, 0.0),
        None => (aim, standoff),
    };
    warp_to_point(
        reg,
        actor,
        here,
        aim,
        standoff,
        crate::targeting::Delivery::WithAnswer,
    );
    None
}

/// The refusal for a warp that would land within
/// [`crate::space_state::MIN_WARP_DISTANCE`] of `from`, `None` otherwise.
///
/// **The key is `CustomNotify`, `[A]`**: the client has no `UserError` for
/// this (the menu simply does not offer the entry; `findings/user-errors.tsv`
/// has only `WarpTooShort`, which is about a disrupted warp). Falsified by
/// a client string that names a minimum-distance refusal.
pub(crate) fn too_short(
    from: [f64; 3],
    aim: [f64; 3],
    standoff: f64,
) -> Option<crate::movement_guard::Refusal> {
    let arrival = crate::destiny::warp_arrival(from, aim, standoff);
    if crate::space_state::distance(from, arrival) > crate::space_state::MIN_WARP_DISTANCE {
        return None;
    }
    Some(crate::movement_guard::Refusal {
        user_error: "CustomNotify",
        args: crate::pyutil::pypairs(vec![(
            "notify",
            crate::pyutil::bstr("You cannot warp to a destination closer than 150 km."),
        )]),
    })
}

/// The warp itself, once the destination is a point: the `WarpTo` and the
/// throttle in one batch, the tunnel, and the arrival on the clock.
///
/// Split out of [`warp`] for its second caller, the **emergency warp** of a
/// pilot whose connection dropped in space ([`emergency_warp`]). Nobody asked
/// for that one, so there is no payload turn for its pushes to ride out on,
/// and `delivery` says which queue they take -- the same question, and the
/// same answer, as `crate::targeting::Delivery`'s.
pub(super) fn warp_to_point(
    reg: &mut Registry,
    actor: i64,
    here: crate::space_state::Presence,
    aim: [f64; 3],
    standoff: f64,
    delivery: crate::targeting::Delivery,
) {
    let ship_type_id = reg.character(actor).map_or(0, |c| c.ship_type_id);
    // **A new warp retires the old warp's arrival batch** (GRID-VIS). Each
    // warp schedules a late `SetBallPosition` + `SetBallVelocity 0` + `Stop`
    // at its own arrival, padded by three align times. One still pending when
    // the ship warps again fired mid-flight (or on the grid the ship had just
    // reached): the client moved the ball back to the OLD arrival point and
    // stopped it -- a teleport, in some other belt, while the park still held
    // the ball. Seen in the real client (201000103: WarpTo at tick 120, then a
    // SetBallPosition to the previous destination at 121 and again at 186).
    let old_ship = here.ship_id;
    reg.cancel_scheduled(|p| is_warp_settle(p, old_ship));
    let warp = crate::destiny::Action::WarpTo {
        ball: here.ship_id,
        target: aim,
        min_range: standoff,
        warp_factor: crate::destiny::warp_factor_in(reg.content(), ship_type_id),
    };
    // **And the throttle, or the ship aligns for three minutes.**
    //
    // `AlignForWarp` activates the warp only when the ball is pointed at the
    // target *and* `speed^2 > (0.75 * maxVelocity)^2` **[V]**, and until
    // then it steers the ball with the ordinary goto integrator, whose
    // commanded speed is `speedFraction * maxVelocity` **[V]**. A ball whose
    // `speedFraction` is 0 -- which is every ball that has undocked and been
    // told nothing -- therefore sits still, never aligns, and warps only
    // when the client's own 180-tick watchdog force-activates it. The player
    // sees "Aligning" for three minutes in a pod whose align time is 0.09
    // seconds. `a_warp_opens_the_throttle_so_the_client_can_align` has the
    // addresses and the constants.
    //
    // 1.0 and not "whatever the slider was at": the gate is 0.75 of top
    // speed and a goto only approaches its commanded speed from below, so
    // anything at or under 0.75 could never cross it. Warping *is* full
    // throttle, in the client's arithmetic.
    let throttle = crate::destiny::Action::SetSpeedFraction {
        ball: here.ship_id,
        fraction: 1.0,
    };
    let actions = vec![warp, throttle];
    // **The stamp is taken before the motion is written, not after.** It used
    // to be allocated below, which was fine while `after(WarpTo)` teleported
    // and needed no clock. The record now carries a warp, and a warp's whole
    // state is "when did this activate" -- so the stamp has to exist before
    // there is anything to stamp.
    let stamp = reg.next_destiny_stamp();
    // The activation must be predicted from the same fitted hull the pilot's
    // ball carries (`destiny::pilot_ball`), or the server's tunnel outlives
    // the client's (owner, Charon, 2026-10-29: 84 s vs 23 s). [A] fitted warp
    // speed is not modelled: `warp_factor_in` stays the table's, and that is
    // the factor the client is sent too.
    let hull = reg
        .flight_hull(actor)
        .unwrap_or(*crate::destiny::hull::of_in(reg.content(), ship_type_id));
    let align_s = crate::destiny::hull::align_seconds(&hull);
    // The arrival point, computed **once** and used three times: to refine the
    // motion below, to length the warp, and to reconcile at the far end.
    // Three separate derivations of an arrival point is how §44.6's two-copy
    // defect happened twice; one derivation is how it stops.
    let arrival = crate::destiny::warp_arrival(here.motion.position, aim, standoff);
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        for action in &actions {
            p.motion.after(*action);
        }
        // **Refining the aligning warp into one that knows its own future.**
        //
        // `Motion::after` set the honest minimum for an action with no ship
        // attached: aligning, stamp negative, no length. Two facts turn that
        // into something `advanced_at` can integrate, and only this function
        // has them -- the hull's align time and the flight's length.
        //
        // **The stamp is the activation tick, in the future**, and that is
        // the design decision worth stating. This server integrates motion
        // *lazily*, only when somebody reads it (`advance_character`,
        // `destiny::on_ballpark_bind`); the clock does not walk ships
        // forward. So a warp commanded now and first observed thirty seconds
        // later arrives as a single `dt` of thirty seconds -- and an
        // activation *detected* by watching the speed cross Carbon's 0.75
        // gate would fire at the end of that step, reporting a ship that had
        // barely moved when it should be three AU away. Carbon can use the
        // gate because it ticks every second; we cannot, because we tick when
        // asked.
        //
        // So activation is **predicted** from `align_seconds` rather than
        // detected -- the same number `warp_flight_seconds` already uses for
        // the arrival batch below, so there is one model of this ship and not
        // two. `advanced_at` aligns while `now` is before the stamp and flies
        // the curve after it.
        //
        // **The `[A]` this leaves, stated plainly:** an observer who binds
        // during the align reads a *positive* stamp whose instant has not
        // arrived, where the client would have written a negative one, so its
        // `IsPreparingWarp` would say "warping" while the ball is still
        // turning. Bounded, and only for a third party: the pilot's own
        // client has the real `WarpTo` action and simulates the align itself,
        // so this can mislead somebody else's park for the seconds between
        // command and activation, and our next update corrects it.
        // Falsifiable by binding a second client during a battleship's align
        // -- the longest window this has -- and seeing whether the bracket
        // shows a warp too early.
        if let crate::destiny::Mode::Warp {
            goto,
            effect_stamp,
            total_distance,
            ..
        } = &mut p.motion.mode
        {
            *effect_stamp = stamp.saturating_add(align_s.ceil().max(0.0) as u32) as i32;
            *goto = arrival;
            *total_distance = crate::space_state::distance(p.motion.position, arrival);
        }
    }
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(here.solar_system_id, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to.clone(), stamp, actions) {
        send(reg, delivery, update);
    }
    // **The tunnel.** `destiny::SpecialFx` has the evidence; the short
    // version is that `Park.OnSpecialFX` takes nine positionals before its
    // five defaults begin, `effects.Warping` is not a travel effect so the
    // subway service does not swallow it, and `Warping.Prepare` turns the
    // event into a departure flash for everyone who is not the pilot -- so
    // one event serves the whole audience.
    //
    // A second notification rather than a tenth entry in the batch above,
    // and the same stamp: equal stamps in consecutive batches cost the
    // client a `range(0)` loop in `SynchroniseToSimulationTime` and nothing
    // else (`Registry::next_destiny_stamp`), while `OnSpecialFX` is in
    // `nonDestinyCriticalFunctions` and is not time-synchronised at all.
    //
    // **No `start = 0` at arrival**, deliberately: `Warping.WarpLoop` ends
    // the effect itself when the ball stops accelerating
    // (`Warp.py:631`, `:654`) **[V]**, and we have no scheduler to fire one
    // at the arrival tick anyway (`docs/port-tracker.md` §30.4).
    if let Some(fx) = crate::destiny::SpecialFx::new(
        to.clone(),
        stamp,
        here.ship_id,
        crate::destiny::FX_WARPING,
        true,
    ) {
        send(reg, delivery, fx);
    }
    // **And the arrival, scheduled on the clock.**
    //
    // `docs/re/warp.md` §1 step 4: one batch at the arrival tick, in this
    // order, making the server's position authoritative again. It is
    // reconciliation and not animation -- the client reaches mode `STOP` by
    // itself at its own computed arrival tick, and `Stop`'s implementation is
    // `if ball.mode != 2: StopBall(ball)` **[V]**, a no-op on a ball that has
    // already arrived, which is the normal case.
    //
    // **It still corrects a real disagreement.** Both sides compute the
    // arrival point as `target + dir * min_range`, but the client uses the
    // ship's position at *activation* and we use it at *command*, and those
    // differ by however far the ship drifted while aligning. On a "warp to
    // within 100 km" that is kilometres. Whoever sends `SetBallPosition` last
    // is right, and it should be us.
    //
    // The stamp is deliberately **not** chosen here -- see
    // `crate::notify::Push::restamp`. `Registry::take_due` binds it when the
    // batch leaves, which is what makes a batch scheduled minutes ahead
    // monotonic with everything sent in between.
    // `aim`, not `target.position` -- the same pair the action above was
    // built from. Passing the centre here while the action carried the
    // warp-in point is exactly the two-copy defect the comment above warns
    // about, in its third incarnation: it put the reconciliation at the
    // planet's centre and `a_warp_schedules_its_own_arrival` failed on the
    // first run, which is the only reason this line is right.
    let flight = crate::destiny::warp_flight_seconds(
        crate::space_state::distance(here.motion.position, arrival),
        crate::destiny::warp_factor_in(reg.content(), ship_type_id),
        align_s,
    );
    // **Split by audience** (CAPTURE-UNDOCK, EVEJS-MOTION `warpBuilders.js`:
    // 644-653 as the report records it). The pilot's own client flies the
    // warp and stops by itself at its own arrival; a `SetBallPosition` +
    // `SetBallVelocity 0` + `Stop` sent to it is a snap of its own ship on a
    // predicted timer. It gets only the massive demotion -- the landing is
    // where a ball that is massive by default would otherwise collide with
    // what it lands among. Observers hold a ball the server integrates, so
    // they get the full reconciliation.
    let pilot_batch = vec![crate::destiny::Action::SetBallMassive {
        ball: here.ship_id,
        massive: false,
    }];
    let observer_batch = vec![
        crate::destiny::Action::SetBallPosition {
            ball: here.ship_id,
            point: arrival,
        },
        crate::destiny::Action::SetBallVelocity {
            ball: here.ship_id,
            velocity: [0.0; 3],
        },
        crate::destiny::Action::Stop { ball: here.ship_id },
        // The client's arrival sets `isMassive` again on its copy of the ball
        // (`docs/re/warp.md` §7.1, `0x18004309b`), so an observer's warper
        // lands massive and collides with what it lands among unless it is
        // demoted again, as the pilot's batch does. After the `Stop`, which
        // is what lets the arrival finish first.
        crate::destiny::Action::SetBallMassive {
            ball: here.ship_id,
            massive: false,
        },
    ];
    // Stamp zero, and it never reaches the wire: `take_due` restamps every
    // batch it releases. Passing the current stamp instead would be a value
    // that looks meaningful and is not, which is worse.
    let after = std::time::Duration::from_secs_f64(flight);
    if let Some(update) = crate::destiny::ActionUpdate::new(vec![actor], 0, pilot_batch) {
        reg.schedule(after, update);
    }
    // **A bot is not a client**: it has no simulation to fly its own warp and
    // learns that it landed from this batch (`eve-bot` waits for the `Stop`),
    // so a bot pilot is in the full batch's audience like everyone else.
    let bot = reg.character(actor).is_some_and(|c| c.is_bot);
    let observers: Vec<i64> = to
        .iter()
        .copied()
        .filter(|id| bot || *id != actor)
        .collect();
    if !observers.is_empty()
        && let Some(update) = crate::destiny::ActionUpdate::new(observers, 0, observer_batch)
    {
        reg.schedule(after, update);
    }
}

/// One push, onto the queue `delivery` names: `pending` for an answer's
/// flush, the clock for a warp nobody asked for.
fn send(
    reg: &mut Registry,
    delivery: crate::targeting::Delivery,
    n: impl crate::notify::Push + 'static,
) {
    match delivery {
        crate::targeting::Delivery::WithAnswer => reg.push(n),
        crate::targeting::Delivery::OnTheClock => reg.schedule(std::time::Duration::ZERO, n),
    }
}

/// **The emergency warp**: a pilot whose connection drops in space warps
/// off by themselves, to a point nobody chose, and the ship sits there out
/// its log-off hold (`crate::crimewatch::logged_off`, which decides whether
/// it goes at all).
///
/// That it exists in this build is **[V]**: dogma ships
/// `attributeDoesNotEmergencyWarp` (`dogma/const.py`), and the client offers
/// "Disable Emergency Warp" (573385, `DISABLE_EMERGENCY_WARP` in
/// `characterSettingsConsts.py`). Everything about *where* is **[A]**:
/// [`crate::crimewatch::EMERGENCY_WARP_DISTANCE`] in the direction
/// [`crate::crimewatch::emergency_heading`] draws, flown like any other warp
/// (aligned, full throttle, the tunnel for everyone watching). Not taken
/// here, `[A]` too: the per-character "Disable" setting (`charMgr` does not
/// store character settings) and the attribute (no hull we serve sets it).
/// A warp disruptor is: the movement guards run first (W16 CP57).
///
/// **A ship already in warp keeps going** where it was going: the pilot's
/// last command stands, and a second `WarpTo` mid-flight would bend a warp
/// the observers are already drawing.
///
/// On the clock, all of it: a disconnect is not a payload (`server.rs:402`),
/// so nothing would ever drain `pending` for it -- the reason
/// `Registry::session_closed` gives three times.
pub(super) fn emergency_warp(reg: &mut Registry, actor: i64, now: crate::damage::BlueTime) {
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return;
    };
    // Commanded, not active: a warp already commanded keeps its course.
    if here.motion.warp_commanded() {
        return;
    }
    // **A scrammed ship stays put** (W16 CP57): the same guards as a
    // pilot's own warp, and nobody to tell. [A] that the emergency warp
    // obeys a scram; falsified by a live client whose ship warps off
    // scrammed when its pilot drops.
    if crate::movement_guard::check(&crate::module::plan().warp_guards, reg, actor).is_some() {
        crate::outln!(
            "[crimewatch] character {actor}: no emergency warp, ship {} is scrammed",
            here.ship_id
        );
        return;
    }
    let heading = crate::crimewatch::emergency_heading(here.ship_id, now);
    let distance = crate::crimewatch::EMERGENCY_WARP_DISTANCE;
    let p = here.motion.position;
    let aim = [
        p[0] + heading[0] * distance,
        p[1] + heading[1] * distance,
        p[2] + heading[2] * distance,
    ];
    crate::outln!(
        "[crimewatch] character {actor}: emergency warp of ship {}",
        here.ship_id
    );
    warp_to_point(
        reg,
        actor,
        here,
        aim,
        0.0,
        crate::targeting::Delivery::OnTheClock,
    );
}

/// Whether a scheduled push is the arrival reconciliation of `ship`'s warp:
/// one `DoDestinyUpdate` holding exactly `SetBallPosition`,
/// `SetBallVelocity`, `Stop`, `SetBallMassive` for that ball (the observers' batch
/// `warp_to_point` builds), or exactly `SetBallMassive` for it (the
/// pilot's).
pub(crate) fn is_warp_settle(p: &dyn crate::notify::Push, ship: i64) -> bool {
    p.cancel_tag() == Some(crate::notify::CancelTag::WarpSettle { ball: ship })
}

/// The old test, on `to_push`: the oracle for [`is_warp_settle`].
#[cfg(test)]
pub(crate) fn is_warp_settle_slow(p: &dyn crate::notify::Push, ship: i64) -> bool {
    let (name, args) = p.to_push();
    if name != "DoDestinyUpdate" {
        return false;
    }
    let Some(PyValue::List(entries)) = args.first() else {
        return false;
    };
    let is = |entries: &[PyValue], want: &[&[u8]]| {
        entries.len() == want.len()
            && entries.iter().zip(want).all(|(e, want)| {
                let PyValue::Tuple(e) = e else { return false };
                let Some(PyValue::Tuple(call)) = e.get(1) else {
                    return false;
                };
                let (Some(n), Some(PyValue::Tuple(a))) = (call.first(), call.get(1)) else {
                    return false;
                };
                crate::pyutil::as_bytes(n) == Some(*want) && a.first() == Some(&PyValue::Int(ship))
            })
    };
    is(
        entries,
        &[
            b"SetBallPosition",
            b"SetBallVelocity",
            b"Stop",
            b"SetBallMassive",
        ],
    ) || is(entries, &[b"SetBallMassive"])
}

/// Apply one movement command and push the action that carries it out.
///
/// Four steps, and each early return is a case where doing nothing is right:
/// a connection that is nobody, a character the roster does not know,
/// arguments we could not read (never a guess -- a wrong action is worse than
/// a missing one), and a batch the client would discard.
///
/// The push drains **after** the `CallRsp` in the same flush, which is the
/// ordering `destiny::on_ballpark_bind` already relies on.
fn fly(reg: &mut Registry, req: &Request<'_>) {
    let Some(actor) = req.actor else { return };
    // **Where the ship is now, before deciding what the command means.**
    // `Motion::after` turns a heading into a point relative to the position
    // it finds, so a stale position made every command compound the error of
    // the one before it. Integrating here is also what makes the *next*
    // observer's state true.
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        // Not in space. A `Cmd*` from a docked client is not a thing the
        // client sends, and answering it with a push would put a ball in
        // somebody's park that nothing would ever remove.
        return;
    };
    let Some(action) = crate::space_state::command(here.ship_id, req.method, req.args) else {
        return;
    };
    // **A ship in the warp tunnel takes no orders** (MOVE-VALIDATE). Past
    // activation (the predicate the arrival scheduling uses) the command is
    // answered and nothing is pushed or recorded; before it, the ship is
    // only aligning, and a `Stop` is a warp cancel (the same cancel
    // `movement_guard::abort_warps_refused_by` does, below).
    // **[A]** what the native client does with a `Stop` on a warping ball;
    // falsified by carbonengine's `Ballpark::Stop` having no warp guard (then
    // the tunnel would end on a Stop and this refusal is the wrong call).
    if here
        .motion
        .warp_active_at(crate::space_state::now_seconds())
    {
        return;
    }
    // A follow or orbit needs a ball the pilot's system can resolve, as
    // `CmdAlignTo` does; an unknown id would tether to nothing.
    if let crate::destiny::Action::FollowBall { target, .. }
    | crate::destiny::Action::Orbit { target, .. } = action
        && reg.ball_position(target).is_none()
    {
        return;
    }
    // UNDOCK-INVULN: flying the ship gives up the protection.
    crate::invuln::break_for(reg, actor);
    crate::cloak::break_jump(reg, actor);
    // **Remember before pushing.** The blob we build for the next desync
    // recovery, or for another pilot arriving later, is built from this
    // record and from nothing else.
    if let Some(p) = reg.roster_mut().get_mut(actor) {
        p.motion.after(action);
    }
    // **The pilot has the ship back: the warp's late reconciliation is off.**
    // A client takes none of these while it is warping, so one that arrives
    // means the warp is over *for the pilot*; the scheduled arrival batch
    // (`SetBallPosition` + `SetBallVelocity 0` + `Stop`, padded to run late on
    // purpose) would otherwise land seconds after the order and snap the ship
    // back to the arrival point and stop it. Seen in the real client: a
    // `CmdFollowBall` right after landing, the ship stopped and 25 km from the
    // target 200 s later (FCI-FIX2, fci_concord, 2026-10-28).
    if matches!(
        action,
        crate::destiny::Action::Stop { .. } if here.motion.warp_commanded()
    ) || matches!(
        action,
        crate::destiny::Action::FollowBall { .. }
            | crate::destiny::Action::Orbit { .. }
            | crate::destiny::Action::GotoPoint { .. }
            | crate::destiny::Action::GotoDirection { .. }
    ) {
        let ship = here.ship_id;
        reg.cancel_scheduled(|p| is_warp_settle(p, ship));
    }
    let stamp = reg.next_destiny_stamp();
    // **Everyone watching, not just the pilot.** A Destiny action names the
    // ball it acts on, so the identical batch is correct for every park that
    // contains that ball -- one push, one stamp, one encoding. The pilot
    // stays in the audience: dropping them in favour of "fan out to the
    // others" would break the tier that already works.
    let mut to = vec![actor];
    to.extend(reg.roster().others_in_system(here.solar_system_id, actor));
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, vec![action]) {
        reg.push(update);
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    /// `GetLocalCampaignClientSnapshot` answers the *caller's* session system
    /// (the client retries once on a mismatch), not the first character's.
    #[test]
    fn the_campaign_snapshot_names_the_callers_system() {
        const FIRST: i64 = 1_000_011;
        const SECOND: i64 = 1_000_012;
        let ch = |id, home| crate::model::Character {
            character_id: id,
            name: format!("P{id}"),
            ship_id: id + 100_000_000,
            station_id: Some(60_007_000),
            solar_system_id: Some(home),
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(FIRST, 7_001), ch(SECOND, 7_001)],
            ..SandboxConfig::default()
        });
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                SECOND,
                SECOND + 1_000,
                7_002,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        let snapshot = |reg: &mut Registry, who| {
            dispatch(
                reg,
                &Request {
                    service: "insurgencySolarsystem",
                    method: "GetLocalCampaignClientSnapshot",
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(who),
                },
            )
            .expect("answered")
        };
        let want = |system| PyValue::Tuple(vec![PyValue::Int(system), PyValue::None]);
        assert_eq!(snapshot(&mut reg, FIRST), want(7_001), "docked");
        assert_eq!(
            snapshot(&mut reg, SECOND),
            want(7_002),
            "in space elsewhere"
        );
    }

    /// MOVE-VALIDATE: a `CmdStop` in the tunnel is refused; while still
    /// aligning it is a warp cancel (Stop pushed, arrival batch dropped).
    /// Follow of a ball the system cannot resolve is refused.
    #[test]
    fn stop_in_warp_is_refused_but_cancels_an_align() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        let ch = |id, name: &str, ship| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(ALICE, "Alice", 100_000_001), ch(BOB, "Bob", 100_000_002)],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let station_id = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("home");
        let cmd = |reg: &mut Registry, method: &'static str, args: &[PyValue]| {
            let _ = dispatch(
                reg,
                &Request {
                    service: "beyonce",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            )
            .expect("answered");
        };
        let stops = |reg: &mut Registry| {
            reg.take_pending()
                .iter()
                .filter(|(_, p)| {
                    let (n, a) = p.to_push();
                    n == "DoDestinyUpdate" && action_args(&a, b"Stop").is_some()
                })
                .count()
        };
        let warp = [PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)];
        cmd(&mut reg, "CmdWarpToStuff", &warp);
        let _ = reg.take_pending();
        assert!(reg.roster().get(ALICE).unwrap().motion.warp_commanded());

        // Past activation: nothing pushed, still a warp.
        let mut active = reg.roster().get(ALICE).unwrap().motion;
        if let crate::destiny::Mode::Warp { effect_stamp, .. } = &mut active.mode {
            *effect_stamp = crate::space_state::now_seconds() as i32 - 1;
        }
        reg.roster_mut().get_mut(ALICE).unwrap().motion = active;
        cmd(&mut reg, "CmdStop", &[]);
        assert_eq!(stops(&mut reg), 0);
        assert!(reg.roster().get(ALICE).unwrap().motion.warp_active());

        // Aligning: a Stop goes out and the warp is over.
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        cmd(&mut reg, "CmdWarpToStuff", &warp);
        let _ = reg.take_pending();
        cmd(&mut reg, "CmdStop", &[]);
        assert_eq!(stops(&mut reg), 1);
        assert!(!reg.roster().get(ALICE).unwrap().motion.warp_commanded());

        // A follow of a ball nobody can resolve pushes nothing.
        cmd(
            &mut reg,
            "CmdFollowBall",
            &[PyValue::Int(424_242), PyValue::Real(100.0)],
        );
        assert!(reg.take_pending().is_empty());
    }

    /// The fixture of the warp-activation tests: a pilot whose fitted agility
    /// (81) differs from the type table's, in space 300 km-ish from home,
    /// warping to its own station. Returns the registry, the hull item and the
    /// warp's `(stamp_before, effect_stamp, total_distance)`.
    fn fitted_warp() -> (Registry, i64, i32, i32, f64) {
        use crate::subsystems::tests::{ACTOR, SUBS, fit, world};
        let (mut reg, _, hull) = world(0);
        fit(&mut reg, hull, SUBS[0]).expect("the first bonus subsystem");
        fit(&mut reg, hull, SUBS[1]).expect("the second");
        crate::destiny::on_ballpark_bind(&mut reg, Some(ACTOR));
        reg.slash(Some(ACTOR), "/tp 300").expect("answered");
        let station_id = reg
            .character(ACTOR)
            .and_then(|c| c.station_id)
            .expect("home");
        let _ = reg.take_pending();
        let before = crate::space_state::now_seconds() as i32;
        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ACTOR),
            },
        )
        .expect("answered");
        let Some(crate::destiny::Mode::Warp {
            effect_stamp,
            total_distance,
            ..
        }) = reg.roster().get(ACTOR).map(|p| p.motion.mode)
        else {
            panic!("the pilot is warping");
        };
        (reg, hull, before, effect_stamp, total_distance)
    }

    /// The server predicts the warp's activation from the **fitted** hull
    /// the pilot's ball carries, not the type table's (owner, Charon,
    /// 2026-10-29: 84 s predicted, 23 s real).
    #[test]
    fn a_warp_activates_after_the_fitted_align_time() {
        use crate::destiny::hull;
        use crate::subsystems::tests::ACTOR;
        crate::test_clock::freeze(std::time::Duration::from_secs(1_800_000_000));
        let (reg, _, before, effect_stamp, _) = fitted_warp();
        let fitted = reg.flight_hull(ACTOR).expect("a hull");
        let table = *hull::of_in(reg.content(), crate::subsystems::tests::HULL);
        let want = hull::align_seconds(&fitted).ceil() as i32;
        let table_s = hull::align_seconds(&table).ceil() as i32;
        assert_ne!(want, table_s, "the fixture must tell the two apart");
        let got = effect_stamp - before;
        // The stamp is taken at or just after `before`.
        assert!((want..=want + 1).contains(&got), "{got} vs fitted {want}");
        crate::test_clock::thaw();
    }

    /// The owner's symptom: after the flight the server must not still
    /// think the ship is in warp.
    #[test]
    fn a_warp_is_over_when_its_flight_is_over() {
        use crate::subsystems::tests::ACTOR;
        crate::test_clock::freeze(std::time::Duration::from_secs(1_800_000_000));
        let (mut reg, _, _, effect_stamp, total) = fitted_warp();
        // Walked a second at a time from the activation tick, as the client
        // would see it: the warp ends, and the server agrees with it.
        let mut over_after = None;
        for t in 0..600u64 {
            crate::test_clock::freeze(std::time::Duration::from_secs(effect_stamp as u64 + t));
            reg.advance_character(ACTOR);
            if !reg.roster().get(ACTOR).unwrap().motion.warp_active() {
                over_after = Some(t);
                break;
            }
        }
        let t = over_after.expect("the warp ends");
        // [A] the curve's own length for this invented short warp; only that
        // it ends, counted from the activation tick, is asserted.
        assert!(t < 600, "{t} s after activation, total {total}");
        let motion = reg.roster().get(ACTOR).unwrap().motion;
        assert!(
            matches!(motion.mode, crate::destiny::Mode::Stop),
            "{:?}",
            motion.mode
        );
        crate::test_clock::thaw();
    }

    /// **The true destination goes on the wire, not the offset one.**
    ///
    /// The costliest thing to get wrong in this feature, and invisible in a
    /// one-ship eyeball test: the client applies `minimumRange` itself, so a
    /// server that helpfully pre-offsets the target lands every "warp to
    /// within 100 km" **200 km** short. Asserted against the pushed
    /// arguments, because the wire is where the property lives.
    ///
    /// The tempting wrong version asserts the ship ends up `min_range` from
    /// the target, which is true of the right implementation *and* of the
    /// doubled one.
    #[test]
    fn a_warp_sends_the_true_destination_and_the_offset_separately() {
        const ALICE: i64 = 1_000_001;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        // Past the minimum warp distance (WARP-MIN): the server refuses a short warp.
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");

        const WITHIN: f64 = 100_000.0;
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)],
                // **Wire order: the value first, the key second.** A
                // marshal dict pair is `(value, key)`, and building these the
                // other way round is what let `kwarg` read the wrong slot for
                // the whole life of this feature -- see §39 and the note on
                // `space_state::kwarg`.
                kwargs: &[(PyValue::Real(WITHIN), PyValue::Bytes(b"minRange".to_vec()))],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(
            answer.expect("answered"),
            PyValue::None,
            "the retval is discarded"
        );

        // One batch, one entry, and its arguments are the whole assertion.
        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a destiny batch went out");
        let args = warp_args(&batch.1).expect("the batch holds a WarpTo");
        assert_eq!(
            (&args[1], &args[2], &args[3]),
            (
                &PyValue::Real(0.0),
                &PyValue::Real(0.0),
                &PyValue::Real(0.0)
            ),
            "the station is at the origin, so the true destination is three \
             zeros; anything else means we pre-offset it and the ship will \
             land twice as short: {args:?}"
        );
        assert_eq!(
            args[4],
            PyValue::Real(WITHIN),
            "and the offset travels on its own"
        );
        assert!(
            matches!(args[5], PyValue::Int(f) if f >= 1_000),
            "with a warp factor that is not a hang: {:?}",
            args[5]
        );
        assert_eq!(args.len(), 6, "six arguments, both optionals sent");
    }

    /// **A warp has to open the throttle, or the ship aligns for three
    /// minutes and then warps anyway.**
    ///
    /// The client's align phase has **two** gates and both must pass
    /// (`AlignForWarp`, `_destiny.dll` `0x1800374d7`-`0x18003750c`) **[V]**:
    ///
    /// ```text
    /// |1 - dot(heading, normalize(velocity))| < 0.01   (0x1800ad808)
    /// speed^2 > (0.75 * maxVelocity)^2                 (0.5625 @ 0x1800ad848)
    /// ```
    ///
    /// and the steering it applies until they do is the ordinary sub-warp
    /// goto integrator, whose commanded speed is `speedFraction *
    /// maxVelocity` -- `0x1800388c2 movss xmm3,[r8+0xf4]` (speedFraction),
    /// `0x1800388cb movss xmm8,[r8+0xe0]` (maxVelocity), `0x180038907
    /// mulsd xmm8,xmm3` **[V]**.
    ///
    /// So a ball with `speedFraction == 0` never accelerates, never aligns
    /// -- a zero velocity does not even normalise, so `dot` is 0 and the
    /// first gate reads `|1 - 0| < 0.01`, false forever -- and the warp
    /// starts only when the client's own 180-tick watchdog gives up and
    /// force-activates it (`cmp eax, 0xb4` at `0x180037522`, with the
    /// `AlignForWarp: Forcing warp as %I64d has been aligning for %d ticks.`
    /// string behind it) **[V]**. At one tick a second that is three minutes
    /// of "Aligning", identically for a pod and for a freighter, which is
    /// exactly what the player reported and the opposite of what hull agility
    /// is supposed to mean.
    ///
    /// Every other warp test here reads the `WarpTo` arguments, which were
    /// right, so none of them could see this. The throttle is a *second*
    /// entry in the batch and that is what this asserts.
    ///
    /// **Nothing in the client zeroes `speedFraction` afterwards**, which is
    /// why the record keeps it too: `WarpTo` writes `0x88`, `0x90`, `0x98`,
    /// `0xf8` and the goto target, `StopBall` writes `0x88`, `0xb8`, `0x90`,
    /// `0x98`, `0xf8`, and `SetMode` writes only `0x238` -- not one of them
    /// touches `0xf4` **[V]**.
    #[test]
    fn a_warp_opens_the_throttle_so_the_client_can_align() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        // Past the minimum warp distance (WARP-MIN): the server refuses a short warp.
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        // A standing start, which is the case that hangs: this is what a
        // ship that has undocked and been told nothing looks like.
        assert_eq!(
            reg.roster()
                .get(ALICE)
                .expect("in space")
                .motion
                .speed_fraction,
            0.0,
            "the premise: the throttle is shut before the warp"
        );
        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");

        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered");

        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a destiny batch went out");
        assert!(
            warp_args(&batch.1).is_some(),
            "the warp itself still goes out"
        );
        let throttle =
            action_args(&batch.1, b"SetSpeedFraction").expect("the same batch opens the throttle");
        assert_eq!(
            throttle.first(),
            Some(&PyValue::Int(ALICE_SHIP)),
            "and it is the warping ship's throttle: {throttle:?}"
        );
        // Strictly above 0.75, not merely non-zero. The gate is
        // `speed^2 > (0.75 * maxVelocity)^2` and a goto's speed only
        // *approaches* `speedFraction * maxVelocity` from below, so a
        // throttle of exactly 0.75 converges on the threshold and never
        // crosses it -- "warp at half speed" cannot work, by the client's
        // own arithmetic.
        let fraction = match throttle.get(1) {
            Some(PyValue::Real(f)) => *f,
            other => panic!("a fraction is a Real: {other:?}"),
        };
        assert!(
            fraction > 0.75,
            "a throttle of {fraction} can never reach 0.75 * maxVelocity, so \
             the client would align until its 180-tick watchdog fired"
        );

        // And the record agrees with the ball, because the blob handed to the
        // next observer carries `speedFraction` and the client never clears
        // it.
        assert_eq!(
            reg.roster()
                .get(ALICE)
                .expect("still in space")
                .motion
                .speed_fraction,
            fraction as f32,
            "the server's copy of the throttle has to match the client's"
        );
    }

    /// **The autopilot's warp-in range comes from the server, and a warp
    /// never comes out inside the thing it aimed at.**
    ///
    /// Two findings in one test because the floor and the request interact.
    ///
    /// `CmdWarpToStuffAutopilot` sends one positional argument and no
    /// keywords, so reading `minRange` from the keywords got `0.0`. And a
    /// warp aimed at an object's centre flies the ship *into the model* --
    /// a player warped to the station and landed 16 km inside it, which is
    /// how both of these were found.
    ///
    /// So the rule is `max(what was asked, radius + standoff)`, and the
    /// target here is a **ship**, deliberately: against the station both
    /// numbers collapse onto the floor and the difference between the two
    /// paths stops being visible.
    #[test]
    fn a_warp_clears_the_target_and_the_autopilot_brings_its_own_range() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        const BOB_SHIP: i64 = 100_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: 100_000_001,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: BOB_SHIP,
                    ..crate::model::Character::default()
                },
            ],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        // Past the minimum warp distance (WARP-MIN): the server refuses a short warp.
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");
        let bob_radius = reg.ball_target(BOB_SHIP).expect("Bob is a ball").radius;

        let min_range_of = |reg: &mut Registry, method: &str, args: &[PyValue]| -> f64 {
            dispatch(
                reg,
                &Request {
                    service: "beyonce",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            )
            .expect("answered");
            let batch = reg
                .take_pending()
                .into_iter()
                .map(|(_, p)| p.to_push())
                .find(|(name, _)| *name == "DoDestinyUpdate")
                .expect("a destiny batch went out");
            match warp_args(&batch.1).expect("a WarpTo")[4] {
                PyValue::Real(r) => r,
                ref other => panic!("minimumRange is a Real: {other:?}"),
            }
        };

        // Warping to a ship: the autopilot's own 10 km beats the hull's
        // radius plus a standoff, and the right-click default does not.
        let autopilot = min_range_of(
            &mut reg,
            "CmdWarpToStuffAutopilot",
            &[PyValue::Int(BOB_SHIP)],
        );
        let right_click = min_range_of(
            &mut reg,
            "CmdWarpToStuff",
            &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(BOB_SHIP)],
        );
        assert_eq!(
            autopilot,
            crate::space_state::AUTOPILOT_WARP_IN,
            "the autopilot sends no minRange, so the server supplies one"
        );
        assert!(
            right_click >= bob_radius + crate::space_state::WARP_STANDOFF,
            "{right_click} would put the ship inside a hull of radius {bob_radius}"
        );
        assert!(
            autopilot > right_click,
            "the autopilot asks to stop further out than a plain warp does"
        );

        // And against the station, whose radius dwarfs both, the floor wins.
        let station = min_range_of(
            &mut reg,
            "CmdWarpToStuff",
            &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)],
        );
        let radius = f64::from(crate::destiny::STATION_RADIUS);
        assert!(
            station >= radius + crate::space_state::WARP_STANDOFF,
            "{station} is inside a {radius} m station: this is the defect a \
             player found by warping into the middle of one"
        );
        // Still inside docking range, which is the point of the standoff.
        assert!(
            station - radius < 2_500.0,
            "landing {} m off the hull is outside docking range",
            station - radius
        );
    }

    /// **A warp that would land inside the minimum warp distance is refused**
    /// with a `UserError`, and pushes nothing -- a fleet member on grid
    /// included (WARP-MIN). A member far enough away still works
    /// (`warping_to_a_fleet_member_takes_a_character_id`).
    #[test]
    fn a_warp_inside_the_minimum_distance_is_refused() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: 100_000_001,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: 100_000_002,
                    ..crate::model::Character::default()
                },
            ],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        // Both ships sit at the undock point: Bob is on grid with Alice.
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"char".to_vec()), PyValue::Int(BOB)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert!(
            matches!(answer, Err(ServiceError::Raise(_))),
            "a warp to a member on grid is refused: {answer:?}"
        );
        assert!(reg.take_pending().is_empty(), "a refusal pushes nothing");
        // The pure rule: the landing point, not the target, is measured.
        let from = [0.0; 3];
        let far = [crate::space_state::MIN_WARP_DISTANCE * 2.0, 0.0, 0.0];
        assert!(too_short(from, far, 0.0).is_none());
        assert!(too_short(from, far, far[0]).is_some(), "lands on top of us");
        assert!(too_short(from, [1_000.0, 0.0, 0.0], 0.0).is_some());
    }

    /// **A warp to a ship that has been flying goes to where it is now**
    /// (mine-range). The roster holds a ship where it was at its last
    /// command; a bot sent to a pilot who had flown 150 km since landed where
    /// that pilot *was*, beside the belt they had left, and mined from there.
    #[test]
    fn warping_to_a_flying_ship_goes_to_where_it_is_now() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        const BOB_SHIP: i64 = 100_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: 100_000_001,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: BOB_SHIP,
                    ..crate::model::Character::default()
                },
            ],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        // Alice far away at rest; Bob last spoke a minute ago, moving.
        let far = 1.0e9;
        {
            let p = reg.roster_mut().get_mut(ALICE).expect("in space");
            p.motion = crate::space_state::Motion::arrived([far, 0.0, 0.0], [0.0; 3]);
            p.since = crate::space_state::now_seconds();
            let p = reg.roster_mut().get_mut(BOB).expect("in space");
            p.motion = crate::space_state::Motion::arrived([0.0; 3], [800.0, 0.0, 0.0]);
            p.since = crate::space_state::now_seconds() - 60.0;
        }
        let _ = reg.take_pending();
        for (subject, id) in [(b"char".to_vec(), BOB), (b"item".to_vec(), BOB_SHIP)] {
            let answer = dispatch(
                &mut reg,
                &Request {
                    service: "beyonce",
                    method: "CmdWarpToStuff",
                    args: &[PyValue::Bytes(subject.clone()), PyValue::Int(id)],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            );
            assert_eq!(answer.expect("answered"), PyValue::None);
            let batch = reg
                .take_pending()
                .into_iter()
                .map(|(_, p)| p.to_push())
                .find(|(name, _)| *name == "DoDestinyUpdate")
                .expect("a WarpTo");
            let args = warp_args(&batch.1).expect("a WarpTo");
            let now_at = reg.roster().get(BOB).expect("in space").motion.position;
            assert!(now_at[0] > 10.0, "Bob has flown on: {now_at:?}");
            assert_eq!(
                args[1],
                PyValue::Real(now_at[0]),
                "{subject:?}: the destination is where Bob is now, not where he last spoke"
            );
        }
    }

    /// **Warping to a fleet member works, and its id is a character id.**
    ///
    /// Observed on the wire as `CmdWarpToStuff("char", 90000001)` `[V]` when
    /// one fleet member warps to another. Only `'item'` was handled, so every
    /// fleet warp answered `None` and pushed nothing -- two players reported
    /// being unable to warp to each other, with no trace on either side.
    ///
    /// The ids are the trap: `'item'` carries a **ship** id and `'char'` a
    /// **character** id, and in this fixture they are different numbers on
    /// purpose, so a mix-up cannot pass.
    #[test]
    fn warping_to_a_fleet_member_takes_a_character_id() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        const BOB_SHIP: i64 = 100_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: 100_000_001,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: BOB_SHIP,
                    ..crate::model::Character::default()
                },
            ],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        // Put Bob somewhere distinctive so the destination is checkable.
        reg.slash(Some(BOB), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        let bob_at = reg.roster().get(BOB).expect("in space").motion.position;

        let answer = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"char".to_vec()), PyValue::Int(BOB)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(answer.expect("answered"), PyValue::None);
        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a fleet warp has to push a WarpTo");
        let args = warp_args(&batch.1).expect("a WarpTo");
        assert_eq!(
            (&args[1], &args[2], &args[3]),
            (
                &PyValue::Real(bob_at[0]),
                &PyValue::Real(bob_at[1]),
                &PyValue::Real(bob_at[2])
            ),
            "the destination is where Bob's ship is: {args:?}"
        );

        // A character id that nobody is flying resolves to nothing, and a
        // *ship* id passed as a character is one of those.
        for bad in [PyValue::Int(BOB_SHIP), PyValue::Int(424_242)] {
            dispatch(
                &mut reg,
                &Request {
                    service: "beyonce",
                    method: "CmdWarpToStuff",
                    args: &[PyValue::Bytes(b"char".to_vec()), bad.clone()],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            )
            .expect("answered");
            assert!(
                reg.take_pending().is_empty(),
                "a 'char' subject naming {bad:?} is not a warp"
            );
        }
    }

    /// **The warp tunnel goes out, and its nine arguments are the client's
    /// own.**
    ///
    /// This shipped without the effect for one round, on the stated grounds
    /// that two lanes disagreed about `OnSpecialFX`'s arity and a wrong arity
    /// raises inside the handler. **They did not disagree.**
    /// `Park.OnSpecialFX` is `argcount 15` -- `self` plus fourteen -- with
    /// **five** defaults, which is `MAKE_FUNCTION 5` at offset 564 of the
    /// `Park` class body, its five `LOAD_CONST`s being `-1, None, None, 0,
    /// None` **[V]**. Fourteen parameters and nine required positionals are
    /// the same reading stated two ways.
    ///
    /// So the assertion is the nine values at `spaceMgr.py:268`, where the
    /// client fakes this very event for itself on the reconnect-into-warp
    /// path **[V]**:
    ///
    /// ```text
    /// (ball.id, None, None, None, None, 'effects.Warping', 0, 1, 0)
    /// ```
    ///
    /// The `None`s are asserted individually rather than by length, because
    /// the failure this guards against is a positional slot filled with the
    /// wrong thing -- `targetID` set to the warp destination, say, which
    /// would read as a module firing at a planet.
    ///
    /// And the audience is everyone in the system, not the pilot: a warp the
    /// pilot alone can see is the defect the fleet-warp round found in a
    /// different feature.
    #[test]
    fn a_warp_sends_the_tunnel_effect_to_everyone_watching() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        const BOB: i64 = 1_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: ALICE_SHIP,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: 100_000_002,
                    ..crate::model::Character::default()
                },
            ],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        // Past the minimum warp distance (WARP-MIN): the server refuses a short warp.
        reg.slash(Some(ALICE), "/tp 300").expect("answered");
        let _ = reg.take_pending();
        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");

        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(station_id)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered");

        let pending = reg.take_pending();
        let mut fx = None;
        let mut warp_stamp = None;
        for (_, push) in &pending {
            let (name, args) = push.to_push();
            if name != "DoDestinyUpdate" {
                continue;
            }
            if let Some(found) = action_args(&args, b"OnSpecialFX") {
                fx = Some((found, push.audience()));
            }
            if action_args(&args, b"WarpTo").is_some() {
                warp_stamp = Some(stamp_of(&args));
            }
        }
        let (fx, audience) = fx.expect("the warp sends an OnSpecialFX");

        assert_eq!(fx.len(), 9, "nine positionals, then five defaults: {fx:?}");
        assert_eq!(fx[0], PyValue::Int(ALICE_SHIP), "shipID");
        assert_eq!(
            fx[1],
            PyValue::None,
            "moduleID -- and None is what keeps \
             michelle.py:2090's per-module loop out of this"
        );
        assert_eq!(fx[2], PyValue::None, "moduleTypeID");
        assert_eq!(fx[3], PyValue::None, "targetID");
        assert_eq!(fx[4], PyValue::None, "otherTypeID");
        assert_eq!(
            fx[5],
            crate::pyutil::bstr("effects.Warping"),
            "the guid, as a Python 2 str: it is a dict key in \
             fxSequencer's classification table"
        );
        assert_eq!(fx[6], PyValue::Int(0), "isOffensive");
        assert_eq!(fx[7], PyValue::Int(1), "start");
        assert_eq!(fx[8], PyValue::Int(0), "active");

        let mut audience = audience.expect("the effect names its audience");
        audience.sort_unstable();
        assert_eq!(
            audience,
            vec![ALICE, BOB],
            "the pilot gets a tunnel and the observer gets a WarpOut flash \
             out of the same event (Warp.py:43-48)"
        );

        // Same stamp as the warp it belongs to. Equal stamps across batches
        // are free, and a *later* one would put the tunnel after the ship.
        assert_eq!(
            stamp_of_fx(&pending),
            warp_stamp.expect("the warp went out too"),
            "the effect rides the warp's stamp"
        );
    }

    /// The stamp of the first entry of a `DoDestinyUpdate` batch.
    fn stamp_of(args: &[PyValue]) -> i64 {
        let PyValue::List(entries) = &args[0] else {
            panic!("the batch is a sequence: {args:?}")
        };
        let PyValue::Tuple(first) = &entries[0] else {
            panic!("an entry is a tuple")
        };
        match first[0] {
            PyValue::Int(s) => s,
            ref other => panic!("a stamp is an Int: {other:?}"),
        }
    }

    fn stamp_of_fx(pending: &[(std::time::Duration, Box<dyn crate::notify::Push>)]) -> i64 {
        for (_, push) in pending {
            let (name, args) = push.to_push();
            if name == "DoDestinyUpdate" && action_args(&args, b"OnSpecialFX").is_some() {
                return stamp_of(&args);
            }
        }
        panic!("no OnSpecialFX in the batch")
    }

    /// **The record flies, and it lands.**
    ///
    /// `a_warp_schedules_its_own_arrival` below proves the *client* is told
    /// where to be. This proves the **server's own record** gets there, which
    /// is a different claim and was false until §59: `Motion::after(WarpTo)`
    /// teleported, so there was no flight to get wrong.
    ///
    /// The clock is supplied rather than read. A warp's position is a function
    /// of time since *activation*, not of the step
    /// (`crate::destiny::warp_trajectory`), so fast-forwarding the way the
    /// rest of these tests do -- winding `p.since` back -- moves `dt` and
    /// leaves the warp exactly where it was. `advanced_at` takes `now` for
    /// this reason, and this test is why that parameter exists.
    ///
    /// Three instants, three claims: still aligning before the activation
    /// stamp, somewhere strictly between the ends after it, and parked at the
    /// arrival with `Mode::Stop` once the flight time has passed.
    #[test]
    fn the_servers_own_record_flies_the_warp_and_parks() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000001\t11\t3000000.0\t598391482800.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let planet = reg
            .celestials()
            .served()
            .next()
            .map(|c| c.id)
            .expect("the planet this test just put in the catalogue");

        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(planet)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered");

        let m = reg.roster().get(ALICE).expect("in space").motion;
        let crate::destiny::Mode::Warp {
            goto,
            effect_stamp,
            total_distance,
            ..
        } = m.mode
        else {
            panic!("a warp command must leave a Mode::Warp, got {:?}", m.mode)
        };
        assert!(
            total_distance > 1.0e11,
            "a four-AU warp should be lengthed in the hundreds of billions of \
             metres, got {total_distance}"
        );
        let hull = crate::destiny::hull::of_in(reg.content(), 0);
        let activation = f64::from(effect_stamp);
        let started = m.position;
        let gap = |a: [f64; 3], b: [f64; 3]| crate::space_state::distance(a, b);

        // Before activation: aligning, so moving but nowhere near the warp.
        let aligning = m.advanced_at(hull, 0.5, activation - 0.5);
        assert!(
            gap(aligning.position, started) < 1.0e6,
            "an aligning ship has not covered a thousand kilometres: {} m",
            gap(aligning.position, started),
        );

        // Mid-flight: strictly between the two ends.
        let flying = m.advanced_at(hull, 10.0, activation + 10.0);
        let covered = gap(flying.position, started);
        assert!(
            covered > 1.0e6 && covered < total_distance,
            "ten seconds in, the ship should be underway and not arrived: \
             covered {covered} of {total_distance}"
        );

        // And long after: parked at the goto, mode Stop.
        let landed = m.advanced_at(hull, 3600.0, activation + 3600.0);
        assert!(
            gap(landed.position, goto) < 1.0,
            "the record must park at its own goto: {} m away",
            gap(landed.position, goto),
        );
        assert_eq!(
            landed.mode,
            crate::destiny::Mode::Stop,
            "dropout leaves the ball stopped"
        );
        assert_eq!(landed.velocity, [0.0; 3]);
        let _ = reg.take_pending();
        let _ = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(7200));
    }

    /// **A warp schedules its own arrival, and the batch is the spec's step
    /// 4.**
    ///
    /// `docs/re/warp.md` §1.1 step 4: at the arrival tick, one batch holding
    /// `SetBallPosition`, `SetBallVelocity` and `Stop`, **in that order**.
    /// The order is not cosmetic -- `Stop` first would halt the ball before
    /// the position landed, and the client would hold a ship at the wrong
    /// place with no velocity to carry it anywhere.
    ///
    /// Three properties, and the third is the one that was impossible before
    /// this round:
    ///
    /// 1. the batch is **scheduled**, not pushed -- nothing goes out now;
    /// 2. it names the same point [`crate::destiny::warp_arrival`] computes,
    ///    so the reconciliation agrees with what the server already believes;
    /// 3. it is due **after** the warp could possibly have finished. An early
    ///    arrival batch aborts a warp in flight; a late one lands on a ball
    ///    already stopped, where `Stop` is `if ball.mode != 2: StopBall(ball)`
    ///    and therefore a no-op **[V]**.
    #[test]
    fn a_warp_schedules_its_own_arrival() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        const BOB: i64 = 1_000_002;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                crate::model::Character {
                    character_id: ALICE,
                    name: "Alice".to_owned(),
                    ship_id: ALICE_SHIP,
                    ..crate::model::Character::default()
                },
                crate::model::Character {
                    character_id: BOB,
                    name: "Bob".to_owned(),
                    ship_id: 100_000_002,
                    ..crate::model::Character::default()
                },
            ],
            // One planet, four AU out. The catalogue is a file at runtime
            // (`crate::celestial`), so a test that wants a warp destination
            // AU away has to bring its own -- and an AU-scale destination is
            // the point: a hop across the station would not exercise the
            // flight model at all.
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000001\t11\t3000000.0\t598391482800.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        let planet = reg
            .celestials()
            .served()
            .next()
            .map(|c| (c.id, c.position))
            .expect("the planet this test just put in the catalogue");

        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(planet.0)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered");

        assert_eq!(
            reg.scheduled_count(),
            2,
            "the arrival is on the clock, not in this turn's drain: one batch \
             for the pilot, one for the observer"
        );
        let immediate = reg.take_pending();
        assert!(!immediate.is_empty(), "the warp itself still goes out now");

        // Nothing is due yet: the flight has not happened.
        let now = std::time::Instant::now();
        assert!(
            reg.take_due(now).is_empty(),
            "an arrival released at command time would abort the warp"
        );

        // And at the far end, the three actions in order.
        let due = reg.take_due(now + std::time::Duration::from_secs(3600));
        assert_eq!(due.len(), 2, "one batch per audience");
        // **Split by audience** (CAPTURE-UNDOCK): the pilot's own client
        // flies and stops its warp, so it is never snapped; it gets only the
        // massive demotion.
        let pilot = due
            .iter()
            .find(|p| p.audience() == Some(vec![ALICE]))
            .expect("a batch for the pilot alone");
        let (_, pilot_args) = pilot.to_push();
        assert_eq!(
            action_args(&pilot_args, b"SetBallMassive").expect("the demotion"),
            vec![PyValue::Int(ALICE_SHIP), PyValue::Int(0)]
        );
        assert!(
            action_args(&pilot_args, b"SetBallPosition").is_none()
                && action_args(&pilot_args, b"Stop").is_none(),
            "the pilot is never snapped to the predicted arrival"
        );
        let observers = due
            .iter()
            .find(|p| p.audience() == Some(vec![BOB]))
            .expect("a batch for the observer alone");
        let (name, args) = observers.to_push();
        assert_eq!(name, "DoDestinyUpdate");
        let PyValue::List(entries) = &args[0] else {
            panic!("the batch is a sequence: {args:?}")
        };
        let names: Vec<Vec<u8>> = entries
            .iter()
            .filter_map(|e| {
                let PyValue::Tuple(e) = e else { return None };
                let PyValue::Tuple(call) = &e[1] else {
                    return None;
                };
                crate::pyutil::as_bytes(&call[0]).map(|b| b.to_vec())
            })
            .collect();
        assert_eq!(
            names,
            vec![
                b"SetBallPosition".to_vec(),
                b"SetBallVelocity".to_vec(),
                b"Stop".to_vec(),
                b"SetBallMassive".to_vec()
            ],
            "position, then velocity, then stop -- stopping first would park \
             the ball before the position landed -- then the demotion the \
             client's arrival undid"
        );
        let settle = action_args(&args, b"SetBallPosition").expect("a position");
        // **Against the destination the server holds**, not against a second
        // copy of `warp_arrival`'s arithmetic: a test that recomputed the
        // formula would pass just as happily if both copies were wrong. The
        // property that matters is that the reconciliation and the roster
        // agree, because disagreeing is the whole failure it exists to fix.
        //
        // **This read `motion.position` until §59**, and that was the same
        // property while `Motion::after(WarpTo)` teleported -- the roster's
        // position *was* the arrival point from the instant of the command.
        // Now the roster holds where the ship actually is, which during the
        // align is the station mouth, and the destination lives in the warp's
        // own `goto`. So the comparison moves to `goto` and the property is
        // unchanged: one arrival point, derived once, named by both the batch
        // and the mode.
        //
        // It failed on the first run after the flip with the two ends of the
        // trip side by side in the diff -- 4 AU against the station mouth --
        // which is the clearest possible statement of what had changed.
        let crate::destiny::Mode::Warp { goto: believed, .. } =
            reg.roster().get(ALICE).expect("still in space").motion.mode
        else {
            panic!("a warp command must leave a Mode::Warp behind it")
        };
        assert_eq!(
            (&settle[1], &settle[2], &settle[3]),
            (
                &PyValue::Real(believed[0]),
                &PyValue::Real(believed[1]),
                &PyValue::Real(believed[2])
            ),
            "the batch must name the position the server holds, or the \
             reconciliation reconciles to the wrong place"
        );
        // And it is not the origin, which is what a silently-failed
        // destination lookup would have left here.
        assert!(
            crate::space_state::distance(believed, [0.0; 3]) > 1.0e6,
            "a warp to a planet must end up AU away, not at the origin: \
             {believed:?}"
        );
    }

    /// A bot pilot has no simulation of its own, so the arrival batch names
    /// it too (CAPTURE-UNDOCK); a human alone gets only the demotion.
    #[test]
    fn a_bot_pilot_is_still_sent_the_arrival_batch() {
        const ALICE: i64 = 1_000_001;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: 100_000_001,
                is_bot: true,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000001\t11\t3000000.0\t598391482800.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let planet = reg
            .celestials()
            .served()
            .next()
            .map(|c| c.id)
            .expect("the planet");
        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(planet)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("answered");
        let due = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(3600));
        let has = |name: &[u8]| {
            due.iter()
                .any(|p| action_args(&p.to_push().1, name).is_some())
        };
        assert!(has(b"SetBallMassive") && has(b"SetBallPosition") && has(b"Stop"));
    }

    /// **An order after landing cancels the warp's late reconciliation**
    /// (FCI-FIX2): the real client's ship, told to follow a ball right after
    /// it landed, was stopped and snapped back seven seconds later by the
    /// scheduled `SetBallPosition`/`SetBallVelocity`/`Stop`.
    #[test]
    fn a_follow_after_landing_cancels_the_arrival_batch() {
        const ALICE: i64 = 1_000_001;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: 100_000_001,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000001\t11\t3000000.0\t598391482800.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let planet = reg
            .celestials()
            .served()
            .next()
            .map(|c| c.id)
            .expect("planet");
        let call = |reg: &mut Registry, method: &'static str, args: &[PyValue]| {
            dispatch(
                reg,
                &Request {
                    service: "beyonce",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            )
            .expect("answered")
        };
        call(
            &mut reg,
            "CmdWarpToStuff",
            &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(planet)],
        );
        assert_eq!(reg.scheduled_count(), 1, "the arrival is on the clock");
        // A throttle change is not taking the ship back: the batch stays.
        call(&mut reg, "CmdSetSpeedFraction", &[PyValue::Real(1.0)]);
        assert_eq!(reg.scheduled_count(), 1, "a throttle keeps the arrival");
        // A follow is.
        call(
            &mut reg,
            "CmdFollowBall",
            &[PyValue::Int(planet), PyValue::Real(1000.0)],
        );
        assert_eq!(reg.scheduled_count(), 0, "the reconciliation is cancelled");
    }

    /// **A second warp cancels the first warp's arrival batch** (GRID-VIS):
    /// the stale batch snapped the ship back to the old destination and
    /// stopped it in the middle of the new flight.
    #[test]
    fn a_second_warp_retires_the_first_warps_arrival_batch() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000001\t11\t3000000.0\t598391482800.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let ids: Vec<i64> = reg.celestials().served().map(|c| c.id).collect();
        assert!(!ids.is_empty(), "a planet");
        for id in [ids[0], ids[0]] {
            let _ = dispatch(
                &mut reg,
                &Request {
                    service: "beyonce",
                    method: "CmdWarpToStuff",
                    args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(id)],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(ALICE),
                },
            )
            .expect("answered");
        }
        assert_eq!(
            reg.scheduled_count(),
            1,
            "one arrival batch, the newest warp's"
        );
    }

    /// The argument tuple of the first `WarpTo` in a `DoDestinyUpdate`'s
    /// arguments, if there is one.
    /// **A warp to a planet aims at the client's own warp-in point, not at
    /// the planet.**
    ///
    /// The regression guard for the defect the owner found by playing: a
    /// warp to a 5,060 km planet landed the ship 1 km above its surface,
    /// because the server aimed at the centre with a `radius + 1000` floor.
    /// `crate::warp_in` carries the client's rule and its citations; this
    /// test is the one that would notice if the warp path stopped using it.
    ///
    /// The planet here is the sandbox's own 40000002, with the radius and
    /// position from `out/celestials-30000001.tsv`, so the expected distance
    /// is the real one and not a round number chosen to make a test pass.
    #[test]
    fn a_warp_to_a_planet_aims_at_its_warp_in_point_and_not_at_the_planet() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        const PLANET: i64 = 40_000_002;
        const RADIUS: f64 = 5_060_000.0;
        let centre = [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0];

        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "planet\t40000002\t11\t5060000.0\t161891117336.0\t21288951986.0\t-73529712226.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();

        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(PLANET)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a destiny batch went out");
        let warp = warp_args(&batch.1).expect("a WarpTo");
        let point = match (&warp[1], &warp[2], &warp[3]) {
            (PyValue::Real(x), PyValue::Real(y), PyValue::Real(z)) => [*x, *y, *z],
            other => panic!("the goto is three Reals: {other:?}"),
        };

        // It is the warp-in point, to the metre.
        let want = crate::warp_in::planet_warp_in(PLANET, centre, RADIUS);
        assert_eq!(point, want, "the warp did not aim at the warp-in point");

        // And, stated as the defect rather than as the formula: the point we
        // aim at is thousands of kilometres clear of the planet, where the
        // old behaviour was one kilometre.
        let clearance = crate::space_state::distance(point, centre) - RADIUS;
        assert!(
            clearance > 4_000_000.0,
            "clearance is {clearance} m; the defect this guards against was              1,000 m above a {RADIUS} m surface"
        );

        // With a warp-in point there is no radius floor left to apply: the
        // point already clears the body, so `minimumRange` is what the
        // player asked for -- nothing -- and the ship stops *on* the point.
        match warp[4] {
            PyValue::Real(r) => assert_eq!(
                r, 0.0,
                "a right-click warp to a planet asks for no offset, so the \
                 ship should land on the shared point everyone lands on"
            ),
            ref other => panic!("minimumRange is a Real: {other:?}"),
        }
    }

    /// **A warp to the sun aims at `GetSunWarpInPoint`** (the sun fix): the
    /// star is a served ball now, and `warp_in::warp_in_point` gives it the
    /// client's sun rule. Invented ids and numbers.
    #[test]
    fn a_warp_to_the_star_aims_at_the_sun_warp_in_point() {
        const ALICE: i64 = 1_000_001;
        const ALICE_SHIP: i64 = 100_000_001;
        const STAR: i64 = 40_000_101;
        const RADIUS: f64 = 6_000_000.0;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: crate::celestial::Catalogue::parse(
                "star\t40000101\t6\t6000000.0\t0.0\t0.0\t0.0",
            ),
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let _ = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdWarpToStuff",
                args: &[PyValue::Bytes(b"item".to_vec()), PyValue::Int(STAR)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a destiny batch went out");
        let warp = warp_args(&batch.1).expect("a WarpTo");
        let point = match (&warp[1], &warp[2], &warp[3]) {
            (PyValue::Real(x), PyValue::Real(y), PyValue::Real(z)) => [*x, *y, *z],
            other => panic!("the goto is three Reals: {other:?}"),
        };
        assert_eq!(point, crate::warp_in::sun_warp_in([0.0; 3], RADIUS));
    }

    fn warp_args(args: &[PyValue]) -> Option<Vec<PyValue>> {
        action_args(args, b"WarpTo")
    }

    /// The arguments of one named action inside a `DoDestinyUpdate` batch.
    ///
    /// A batch holds more than one entry -- a warp is the `WarpTo` *and* the
    /// throttle that lets the client align -- so a reader that assumed one
    /// entry could not see the second.
    fn action_args(args: &[PyValue], name: &[u8]) -> Option<Vec<PyValue>> {
        let PyValue::List(entries) = args.first()? else {
            return None;
        };
        for entry in entries {
            let PyValue::Tuple(pair) = entry else {
                continue;
            };
            let PyValue::Tuple(call) = pair.get(1)? else {
                continue;
            };
            if crate::pyutil::as_bytes(call.first()?) != Some(name) {
                continue;
            }
            let PyValue::Tuple(inner) = call.get(1)? else {
                continue;
            };
            return Some(inner.clone());
        }
        None
    }

    /// A warp factor is milli-AU/s and never slow enough to look like a hang.
    ///
    /// The native clamps anything `<= 0` to `1`, which is 0.001 AU/s -- four
    /// hours to cross a system, and indistinguishable from a server that has
    /// stopped answering. The property is a floor over every hull, not one
    /// hull's number.
    #[test]
    fn no_hull_warps_slowly_enough_to_look_like_a_hang() {
        let content = crate::content::Content::fixture();
        for h in crate::destiny::hull::FIXTURE {
            let factor = crate::destiny::warp_factor_in(&content, h.type_id);
            assert!(
                factor >= 1_000,
                "type {} warps at {factor} milli-AU/s",
                h.type_id
            );
        }
        assert!(
            crate::destiny::warp_factor_in(&content, 424_242) >= 1_000,
            "and an unknown hull takes the fallback rather than a zero"
        );
    }

    /// "Warp to within X" arrives X from the target, on the side it came
    /// from; a plain warp arrives at it.
    #[test]
    fn the_arrival_point_is_offset_back_along_the_approach() {
        let from = [0.0, 0.0, 1.0e9];
        let target = [0.0; 3];
        assert_eq!(
            crate::destiny::warp_arrival(from, target, 0.0),
            target,
            "a plain warp lands on the thing"
        );
        let within = crate::destiny::warp_arrival(from, target, 100_000.0);
        assert!(
            (crate::space_state::distance(target, within) - 100_000.0).abs() < 1.0,
            "{within:?} is not 100 km from the target"
        );
        assert!(
            within[2] > 0.0,
            "and it is on the side the ship came from: {within:?}"
        );
        assert_eq!(
            crate::destiny::warp_arrival(target, target, 100_000.0),
            target,
            "a ship on top of its destination has no approach to offset along"
        );
    }

    /// **Aligning to the station points the ship at the station.**
    ///
    /// The property is the *sign of the dot product*: after the align, the
    /// ship's commanded heading has a positive component toward the thing it
    /// aligned to. That is what "align" means, and it is true of any correct
    /// implementation -- it does not pin the magnitude, which is
    /// `GOTO_DISTANCE`'s business, nor the exact point, which is arithmetic.
    ///
    /// The tempting wrong version asserts the GOTO point equals the station's
    /// position, which is the *opposite* of what align does: a ship that
    /// stops at what it aligned to has not aligned, it has approached.
    #[test]
    fn aligning_to_the_station_points_the_ship_at_it() {
        const ALICE: i64 = 1_000_001;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let station_id = reg
            .character(ALICE)
            .expect("Alice exists")
            .station_id
            .expect("docked somewhere");
        let from = reg.roster().get(ALICE).expect("in space").motion.position;

        let answer = dispatch(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdAlignTo",
                args: &[],
                // Keyword-only, and exactly one of the two is set.
                kwargs: &[
                    // Wire order, `(value, key)`; see `space_state::kwarg`.
                    (PyValue::Int(station_id), PyValue::Bytes(b"dstID".to_vec())),
                    (PyValue::None, PyValue::Bytes(b"bookmarkID".to_vec())),
                ],
                bind_params: None,
                actor: Some(ALICE),
            },
        );
        assert_eq!(
            answer.expect("answered"),
            PyValue::None,
            "the retval is discarded"
        );

        let motion = reg.roster().get(ALICE).expect("in space").motion;
        let crate::destiny::Mode::Goto(point) = motion.mode else {
            panic!("an align sets a course: {motion:?}")
        };
        // Toward the station, measured as "the course and the station lie on
        // the same side of us".
        let to_station = [
            -from[0], -from[1], -from[2], // the station is at the origin
        ];
        let course = [point[0] - from[0], point[1] - from[1], point[2] - from[2]];
        let dot = (0..3).map(|i| to_station[i] * course[i]).sum::<f64>();
        assert!(
            dot > 0.0,
            "the course {course:?} points away from the station at the origin"
        );
        // And past it, not at it: align is a course, not a destination.
        let course_length = crate::space_state::distance(from, point);
        let station_distance = crate::space_state::distance(from, [0.0; 3]);
        assert!(
            course_length > station_distance,
            "the ship would stop at the station instead of flying past it: \
             course {course_length} m against a station {station_distance} m away"
        );
        assert!(
            (motion.speed_fraction - 1.0).abs() < 1e-6,
            "[A] an align opens the throttle: {}",
            motion.speed_fraction
        );
        let _ = reg.take_pending();
    }

    /// An align we cannot resolve pushes nothing and still answers.
    ///
    /// A bookmark, and a destination id that names no ball. Both used to be
    /// the whole of `CmdAlignTo`'s behaviour, and both still are: the client
    /// discards the return value, so the only observable is that nothing is
    /// pushed. Inventing a heading would send a ship off toward nothing,
    /// which is worse than a ship that does not move.
    #[test]
    fn an_align_with_nothing_to_align_to_pushes_nothing() {
        const ALICE: i64 = 1_000_001;
        let alice = crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let before = reg.roster().get(ALICE).expect("in space").motion;

        for kwargs in [
            // A bookmark, which this server has no catalogue for.
            vec![
                (PyValue::None, PyValue::Bytes(b"dstID".to_vec())),
                (PyValue::Int(7), PyValue::Bytes(b"bookmarkID".to_vec())),
            ],
            // A destination that names no ball in the park.
            vec![(PyValue::Int(424_242), PyValue::Bytes(b"dstID".to_vec()))],
            // No keywords at all.
            vec![],
        ] {
            let answer = dispatch(
                &mut reg,
                &Request {
                    service: "beyonce",
                    method: "CmdAlignTo",
                    args: &[],
                    kwargs: &kwargs,
                    bind_params: None,
                    actor: Some(ALICE),
                },
            );
            assert_eq!(answer.expect("answered"), PyValue::None);
            assert!(
                reg.take_pending().is_empty(),
                "pushed something for {kwargs:?}"
            );
            assert_eq!(
                reg.roster().get(ALICE).expect("in space").motion.mode,
                before.mode,
                "and left the course alone for {kwargs:?}"
            );
        }
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
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// Somebody else's security status is a **float**, and it is CCP's own
    /// default.
    ///
    /// `baseDogmaLocation.py:2615` is literally `return 0.0` -- shared
    /// client/server code, which is rule zero paying off again. The float
    /// matters: the value reaches arithmetic and a formatter, and an int
    /// there is a different code path in Python 2.
    ///
    /// One of the three calls `characterInfoWindow` fires in a single
    /// `uthread.parallel`; `uthread.Parallel` re-raises in the caller, so the
    /// three share a fate and this is only cosmetic *given* the other two.
    #[test]
    fn another_characters_security_status_is_a_float() {
        let mut reg = Registry::new(SandboxConfig::default());
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "crimewatch",
                method: "GetCharacterSecurityStatus",
                args: &[PyValue::Int(90_000_002)],
                kwargs: &[],
                bind_params: None,
                actor: Some(90_000_001),
            },
        )
        .expect("crimewatch answers");
        assert_eq!(answer, PyValue::Real(0.0));
        // The same answer for a caller who is nobody: a clean record is not
        // a fact about who is asking.
        let anon = dispatch(
            &mut reg,
            &Request {
                service: "crimewatch",
                method: "GetCharacterSecurityStatus",
                args: &[PyValue::Int(90_000_002)],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .expect("crimewatch answers");
        assert_eq!(anon, PyValue::Real(0.0));
    }

    /// **The scan manager answers `ConeScan`, and a probe result's id
    /// warps.** `ConeScan` arrives on the bound `scanMgr::systemScanMgr`
    /// (`directionalScanSvc.DirectionalScan:54`); `CmdWarpToStuff('scan',
    /// 'ABC-123')` carries a **string** id (`menusvc._WarpXToScanResult:
    /// 1700`), which `opt_id` would have dropped in silence.
    #[test]
    fn a_cone_scan_is_routed_and_a_scan_result_is_warpable() {
        const ALICE: i64 = 1_000_001;
        const BOB: i64 = 1_000_002;
        let ch = |id: i64, ship: i64, name: &str| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(ALICE, 100_000_001, "Alice"), ch(BOB, 100_000_002, "Bob")],
            ..SandboxConfig::default()
        });
        for (who, ship, x) in [(ALICE, 100_000_001, 0.0), (BOB, 100_000_002, 1.0e10)] {
            reg.roster_mut()
                .arrive(crate::space_state::Presence::arriving(
                    who,
                    ship,
                    30_000_001,
                    crate::space_state::Motion::arrived([x, 0.0, 0.0], [0.0; 3]),
                ));
        }
        let req = |service, method, args| Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        let cone = [
            PyValue::Real(std::f64::consts::TAU),
            PyValue::Real(2.0e10),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ];
        let rows = dispatch(&mut reg, &req("scanMgr::systemScanMgr", "ConeScan", &cone))
            .expect("answered");
        assert!(
            matches!(&rows, PyValue::List(r) if !r.is_empty()),
            "{rows:?}"
        );

        let _ = reg.take_pending();
        let id = PyValue::Bytes(crate::scan::target_id(100_000_002).into_bytes());
        let warp = [PyValue::Bytes(b"scan".to_vec()), id];
        dispatch(&mut reg, &req("beyonce", "CmdWarpToStuff", &warp)).expect("answered");
        let batch = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .find(|(name, _)| *name == "DoDestinyUpdate")
            .expect("a warp went out");
        let args = warp_args(&batch.1).expect("a WarpTo");
        assert_eq!(args[1], PyValue::Real(1.0e10), "aimed at Bob: {args:?}");
    }
}

/// Lane fight-stall-fix, step 4: the warp-settle tag names the pushes the
/// old test on `to_push` named.
#[cfg(test)]
mod cancel_tag_tests {
    use super::*;
    use crate::destiny::{Action, ActionUpdate};

    fn batch(actions: Vec<Action>) -> Box<dyn crate::notify::Push> {
        Box::new(ActionUpdate::new(vec![1], 1, actions).expect("a batch"))
    }

    fn observers(ball: i64) -> Vec<Action> {
        vec![
            Action::SetBallPosition {
                ball,
                point: [1.0, 2.0, 3.0],
            },
            Action::SetBallVelocity {
                ball,
                velocity: [0.0; 3],
            },
            Action::Stop { ball },
            Action::SetBallMassive {
                ball,
                massive: false,
            },
        ]
    }

    /// A push whose arguments cannot be built.
    #[derive(Debug)]
    struct Bomb;

    impl crate::notify::Push for Bomb {
        fn to_push(&self) -> (&'static str, Vec<PyValue>) {
            panic!("the scheduled-queue scan built a push's arguments");
        }
    }

    #[test]
    fn the_tag_agrees_with_the_slow_test_on_every_shape() {
        let mut mixed = observers(10);
        mixed[2] = Action::Stop { ball: 11 };
        let mut pushes = vec![
            // The observers' batch and the pilot's lone massive flag.
            batch(observers(10)),
            batch(vec![Action::SetBallMassive {
                ball: 10,
                massive: true,
            }]),
            // Near misses: one entry on another ball, a short batch, a
            // reordered batch, other single actions, a longer batch.
            batch(mixed),
            batch(observers(10)[..3].to_vec()),
            batch(observers(10).into_iter().rev().collect()),
            batch(vec![Action::Stop { ball: 10 }]),
            batch(vec![Action::GotoPoint {
                ball: 10,
                point: [0.0; 3],
            }]),
            batch([observers(10), observers(10)].concat()),
            batch(observers(12)),
        ];
        // Readdressed, as a kill's item pushes are.
        pushes.push(Box::new(crate::notify::Addressed {
            to: Some(vec![5]),
            inner: batch(observers(10)),
        }));
        let mut hits = 0;
        for p in &pushes {
            for ship in [10, 11, 12, 99] {
                let (fast, slow) = (
                    is_warp_settle(p.as_ref(), ship),
                    is_warp_settle_slow(p.as_ref(), ship),
                );
                assert_eq!(fast, slow, "{p:?} ship {ship}");
                hits += usize::from(fast);
            }
        }
        assert!(hits >= 3, "the table reaches the true arms: {hits}");
    }

    #[test]
    fn a_grid_add_and_a_state_are_not_warp_settles() {
        let ball = crate::destiny::wreck_ball(4242, [0.0; 3]);
        let slim = crate::destiny::Slim {
            item_id: 4242,
            type_id: 670,
            owner_id: 1,
            char_id: None,
            corp_id: None,
            response_thresholds: None,
        };
        let park = || crate::destiny::Park {
            current_time: 1,
            balls: vec![ball],
        };
        let added = crate::destiny::BallsAdded::new(vec![1], 1, park(), vec![(slim, None)])
            .expect("an add");
        let state = crate::destiny::StateUpdate {
            to: vec![1],
            state: crate::destiny::Bootstrap::new(1, park(), 4242, vec![slim], Vec::new()),
        };
        for p in [&added as &dyn crate::notify::Push, &state] {
            assert!(!is_warp_settle(p, 4242));
            assert!(!is_warp_settle_slow(p, 4242));
        }
    }

    /// The recall sites skip a push that is nothing they look for without
    /// building it.
    #[test]
    fn the_scan_does_not_build_unrelated_pushes() {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.schedule(std::time::Duration::from_secs(60), Bomb);
        reg.schedule(
            std::time::Duration::from_secs(60),
            ActionUpdate::new(vec![1], 1, observers(10)).expect("a batch"),
        );
        assert_eq!(reg.cancel_scheduled(|p| is_warp_settle(p, 11)), 0);
        assert_eq!(reg.cancel_scheduled(|p| is_warp_settle(p, 10)), 1);
        assert_eq!(reg.scheduled_count(), 1, "the bomb waits for its turn");
    }
}

/// FX1: `CmdDock` docks at the station it names (`docs/lanes/fx1.md`).
#[cfg(test)]
mod dock_tests {
    use super::*;

    /// Invented ids (no client data in tests): Alice's own station is the
    /// `Character` default's; a second station in her system, known because
    /// Bob was seeded at it; a third in another system, Carol's.
    const DOCK_ALICE: i64 = 1_000_001;
    const DOCK_ALICE_SHIP: i64 = 100_000_001;
    const DOCK_BOB: i64 = 1_000_002;
    const DOCK_CAROL: i64 = 1_000_003;
    const SECOND: i64 = 69_000_002;
    const ELSEWHERE: i64 = 69_000_003;
    const FAR_SYSTEM: i64 = 39_000_009;

    fn two_stations() -> Registry {
        let alice = crate::model::Character {
            character_id: DOCK_ALICE,
            name: "Alice".to_owned(),
            ship_id: DOCK_ALICE_SHIP,
            ..crate::model::Character::default()
        };
        let bob = crate::model::Character {
            character_id: DOCK_BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            station_id: Some(SECOND),
            station_owner_id: 1_999_002,
            station_type_id: 990_002,
            station_operation_id: 99,
            ..alice.clone()
        };
        let carol = crate::model::Character {
            character_id: DOCK_CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_003,
            station_id: Some(ELSEWHERE),
            solar_system_id: Some(FAR_SYSTEM),
            ..alice.clone()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice, bob, carol],
            ..SandboxConfig::default()
        });
        // In space: the ballpark bind puts her ship in the roster.
        crate::destiny::on_ballpark_bind(&mut reg, Some(DOCK_ALICE));
        let _ = reg.take_pending();
        reg
    }

    fn cmd_dock(reg: &mut Registry, station: i64) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "beyonce",
                method: "CmdDock",
                args: &[PyValue::Int(station), PyValue::Int(DOCK_ALICE_SHIP)],
                kwargs: &[],
                bind_params: None,
                actor: Some(DOCK_ALICE),
            },
        )
    }

    /// `(old, new)` of one session-change key.
    fn change_of(change: &[(PyValue, PyValue)], key: &str) -> Option<PyValue> {
        crate::pyutil::pairs_get(change, key.as_bytes()).cloned()
    }

    fn pair(old: Option<i64>, new: Option<i64>) -> PyValue {
        let v = |x: Option<i64>| x.map_or(PyValue::None, PyValue::Int);
        PyValue::Tuple(vec![v(old), v(new)])
    }

    /// **Docking at a second station puts the pilot there, and there is
    /// where they undock from** (`docs/port-tracker.md` §72.1). The
    /// session change names the station `CmdDock` named; the ship is in
    /// that station's hangar; `station_id` and `docked_station` both answer
    /// it; the undock leaves from it. Then back home, the same way.
    #[test]
    fn cmd_dock_at_a_second_station_docks_there_and_undocks_from_it() {
        let mut reg = two_stations();
        let home = reg.character(DOCK_ALICE).and_then(|c| c.station_id);
        assert_ne!(home, Some(SECOND), "setup: two different stations");

        assert_eq!(cmd_dock(&mut reg, SECOND).expect("docks"), PyValue::None);
        // `dispatch::session_change_for` runs this after the answer.
        let change = reg.dock_change(Some(DOCK_ALICE)).expect("a dock change");
        assert_eq!(
            change_of(&change, "stationid"),
            Some(pair(None, Some(SECOND)))
        );

        let alice = reg.character(DOCK_ALICE).expect("Alice").clone();
        let bob = reg.character(DOCK_BOB).expect("Bob").clone();
        assert_eq!(alice.station_id, Some(SECOND), "her station now");
        assert_eq!(
            (
                alice.station_owner_id,
                alice.station_type_id,
                alice.station_operation_id
            ),
            (
                bob.station_owner_id,
                bob.station_type_id,
                bob.station_operation_id
            ),
            "and every field of it, not only the id"
        );
        assert_eq!(reg.docked_station(DOCK_ALICE), Some(SECOND));
        let ship = reg.store().get(DOCK_ALICE_SHIP).expect("her ship");
        assert_eq!(
            (ship.location_id, ship.flag_id),
            (SECOND, crate::model::FLAG_HANGAR),
            "the ship is in the second station's hangar"
        );

        let out = reg
            .undock_change(Some(DOCK_ALICE))
            .expect("an undock change");
        assert_eq!(change_of(&out, "stationid"), Some(pair(Some(SECOND), None)));

        // And home again: the first station is still a station she can use.
        crate::destiny::on_ballpark_bind(&mut reg, Some(DOCK_ALICE));
        let _ = reg.take_pending();
        cmd_dock(&mut reg, home.expect("a home")).expect("docks at home");
        reg.dock_change(Some(DOCK_ALICE)).expect("a dock change");
        assert_eq!(reg.docked_station(DOCK_ALICE), home);
        assert_eq!(
            reg.store().get(DOCK_ALICE_SHIP).map(|i| i.location_id),
            home,
            "the ship came with her"
        );
    }

    /// **A station outside the pilot's system, or one this world does not
    /// know, raises `NotDockable` and changes nothing**: the client could
    /// not have had it in its park (`Registry::dock_target`). A raise
    /// returns ahead of `with_session_change`, so no dock goes out.
    #[test]
    fn cmd_dock_refuses_a_station_outside_the_pilots_system() {
        let mut reg = two_stations();
        let before = reg.character(DOCK_ALICE).expect("Alice").clone();
        for station in [ELSEWHERE, 69_000_404] {
            let err = cmd_dock(&mut reg, station).expect_err("refused");
            let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = &err else {
                panic!("not a raise: {err:?}")
            };
            let PyValue::Tuple(parts) = header.as_ref() else {
                panic!("a 0x22 header is a tuple")
            };
            let PyValue::Tuple(args) = &parts[1] else {
                panic!("UserError's args are a tuple")
            };
            assert_eq!(
                crate::pyutil::as_bytes(&args[0]),
                Some(NOT_DOCKABLE.as_bytes()),
                "station {station}"
            );
            assert_eq!(
                format!("{:?}", reg.character(DOCK_ALICE)),
                format!("{:?}", Some(&before)),
                "nothing about her changed, station {station}"
            );
            assert!(reg.roster().is_in_space(DOCK_ALICE), "still in space");
            assert_eq!(
                reg.store().get(DOCK_ALICE_SHIP).map(|i| i.location_id),
                before.station_id,
                "the ship did not move"
            );
        }
    }
}
