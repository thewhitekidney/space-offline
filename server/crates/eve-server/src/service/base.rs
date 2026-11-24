//! The handshake-adjacent and bookkeeping services.
//!
//! Named `base` rather than `core`: a module called `core` shadows the
//! **`core` crate**, and the first thing that breaks is `core::fmt` in the
//! parent, several hundred lines away from the cause.
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

use super::args;
use super::*;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "machoNet",
    "pingService",
    "config",
    "eventLog",
    "alert",
    "clientStatLogger",
    "securityMgr",
    "userSvc",
    "populationCap",
    "localizationServer",
    "monolithTaskTrackerMgr",
    "charUnboundMgr",
    "subscriptionMgr",
    "multiLoginBlocker",
    "XmppChatMgr",
    "chatAuthenticationService",
    "alphaInjectorMgr",
];

/// The calls [`dispatch_ro`] answers (PA-12a).
pub(crate) const READ_METHODS: &[(&str, &str)] = &[("config", "GetAverageMarketPrices")];

/// The read twin of the `config.GetAverageMarketPrices` arm: the same
/// `&Registry` helper, whose kept answer is an `Arc`; it is handed on as
/// `Shared`, so the read turn encodes it from the `Arc` after the guard drops.
pub(crate) fn dispatch_ro(
    reg: &Registry,
    _caller: Caller,
    req: &Request<'_>,
) -> Option<Result<RoValue, ServiceError>> {
    match (req.service, req.method) {
        ("config", "GetAverageMarketPrices") => Some(Ok(RoValue::Shared(
            crate::econ_prices::average_market_prices_shared(reg),
        ))),
        _ => None,
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        "machoNet" => reg.macho_net(req),
        // `skillsvc.GetNextAlphaInjectionDateTime` returns this and the client
        // computes `max(0, value - blue.os.GetWallclockTime())` and shows the
        // injector timer only when `value > now` **[V out/pyc `skillsvc.py:1168`/`:1171`,
        // `banner.py:211`]**: a FILETIME of the next permitted injection. We
        // never limit injections, so a time already past (0) means "now".
        "alphaInjectorMgr" => match req.method {
            "GetNextAvailableInjection" => Ok(PyValue::Long(0)),
            _ => Err(reg.no_method(req)),
        },
        "pingService" => match req.method {
            // The return is ignored; the call only proves the transport
            // is alive.
            "Ping" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        "config" => match req.method {
            // Stored verbatim as cfg.planetBlacklist. Synchronous: it
            // blocks login until answered.
            "GetBlackListedPlanets" => Ok(PyValue::List(vec![])),
            // Stored as cfg._averageMarketPrice and read by typeHelpers
            // (`:106-118` [V]) for the industry EIV and the reprocessing
            // value. CP31: the price oracle; `{}` with no bundle.
            "GetAverageMarketPrices" => Ok(crate::econ_prices::average_market_prices(reg)),
            // `eveCfg._LoadOldStationsInfo` reads `GetOldStationData()` and
            // builds a Recordset from its columns only when it is truthy `[V]`,
            // so a falsy return skips the whole block. There are no
            // stations in this sandbox yet, which makes None the true
            // answer rather than a stub.
            "GetOldStationData" => Ok(PyValue::None),
            // Show Info on a corporation iterates the answer and reads
            // `.solarSystemID` of each row (`infosvc.py:1618`,
            // `mapFilterSettledByCorp.py:15`): never None. NPC corps list
            // the systems of their stations; a player corp or an alliance
            // owns none (offices are not ownership) [A], so `[]`.
            "GetStationSolarSystemsByOwner" => {
                let Some(owner) = args::Args::new(req).int(0) else {
                    return Ok(PyValue::List(vec![]));
                };
                Ok(PyValue::List(
                    reg.content()
                        .get::<crate::stations::Table>()
                        .by_owner(owner)
                        .into_iter()
                        .map(|(system, count)| {
                            keyval(vec![
                                ("solarSystemID", PyValue::Int(system)),
                                ("ownerID", PyValue::Int(owner)),
                                ("stationCount", PyValue::Int(count)),
                            ])
                        })
                        .collect(),
                ))
            }
            // `cfg.eveowners` / `corptickernames` / `evelocations` prime
            // themselves through these three. `Recordset._Prime` tests
            // `type(fetch) is TupleType` first, so the `(header, data)`
            // form is the cheapest reply that works and needs no
            // whitelisted class; `eve_rowset::cfg` builds it with the
            // column order the client pre-sets on itself.
            //
            // Only the ids we can actually name come back. A key the
            // reply drops is remembered in `knownLuzers` and never asked
            // for again, which is the correct outcome for an id this
            // sandbox has never heard of -- better than inventing a name
            // for it.
            "GetMultiOwnersEx" => Ok(reg.multi_owners(req)),
            "GetMultiCorpTickerNamesEx" => Ok(reg.multi_corp_tickers(req)),
            "GetMultiLocationsEx" => Ok(reg.multi_locations(req)),
            // SOC-C1: `cfg.allianceshortnames`.
            "GetMultiAllianceShortNamesEx" => Ok(crate::alliance::multi_short_names(
                reg,
                args::Args::new(req),
            )),
            _ => Err(reg.no_method(req)),
        },
        // Fire-and-forget telemetry the client sends as it shuts down.
        // The return is ignored, but it is a CallReq, so it still has to
        // be answered -- see the note on hangs in docs/reference/login-path.md.
        "eventLog" => match req.method {
            "LogPlayerRequestedDisconnect" => Ok(PyValue::None),
            // Client performance telemetry, sent once the game is up:
            //
            //   LogClientStats(sid, stationID,
            //                  [["charsel", charID, 1, sid,
            //                    "2026.10.19 13:25:16", [ ..stats.. ]]])
            //
            // Return discarded. This is also the call that found our
            // shared-object bug: it was the first payload we ever
            // received with `mapcount > 0`, so it was the first to use a
            // `0x1B` back-reference. See `docs/archive/marshal-tags.md`.
            "LogClientStats" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        "alert" => match req.method {
            // EVE's error-report deduplication. The client calls this
            // with a hash of a stack trace before deciding whether to
            // send the trace itself, and `Alert.BeanCount` returns the
            // 2-tuple `(errorID, logMode)` -- `(None, None)` on a proxy
            // that has never seen that hash, which is every hash, for us.
            //
            // Answering it is not cosmetic. The client makes this call
            // from its exception handler, so leaving it unanswered turns
            // one client-side error into a hung call as well.
            "BeanCount" => Ok(PyValue::Tuple(vec![PyValue::None, PyValue::None])),
            // `Alert.UpdateLogModes` unpacks `changes, flooded = ...` from
            // this call: no hash changed mode, none is flooded.
            "GetLogModeForError" => Ok(PyValue::Tuple(vec![
                PyValue::Dict(vec![]),
                PyValue::List(vec![]),
            ])),
            // The client uploading a full stack trace after BeanCount
            // told it this one is new. The return is not used; what
            // matters is that the call completes, because it is made from
            // the client's exception handler and a hung call there turns
            // one error into two. The trace itself we read from the
            // client's own log (tools/client-log.sh), which has it in
            // readable form rather than marshalled.
            //
            // **The return IS used**: `alert.py:506` stores it as
            // `stacktraceLogMode[stackID[0]]`, and the next report of the same
            // hash unpacks it (`errorID, logMode = ...`, `alert.py:510`). Our
            // `None` made that a `TypeError: 'NoneType' object is not iterable`
            // -- logged as "Exception in Error Logging, preventing infinite
            // recursion" (337 times in the 2026-10-28 client logs) -- and the
            // error was never counted. `(errorID, 'B')` is the batched mode
            // (`LOGGING_BATCHED`, `alert.py:23`): the client counts locally and
            // delivers the counts with `BeanDelivery` later. `errorID` only has
            // to be non-None (None sends the tasklet into a 30 x 5 s wait,
            // `alert.py:514`); the hash itself, folded positive, is as good as
            // any. [A] nothing reads the id back; falsified by a client error
            // naming it.
            "SendClientStackTraceAlert" => {
                let error_id = match args::Args::new(req).get(0) {
                    Some(PyValue::Tuple(stack_id)) => match stack_id.first() {
                        Some(PyValue::Int(h)) => (h.unsigned_abs() as i64).max(1),
                        _ => 1,
                    },
                    _ => 1,
                };
                Ok(PyValue::Tuple(vec![
                    PyValue::Int(error_id),
                    crate::pyutil::bstr("B"),
                ]))
            }
            // The deferred counts from the batched mode above (`alert.py:347`,
            // `:352`), sent every few minutes; nothing is returned or kept.
            "BeanDelivery" | "GroupBeanDelivery" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Client-side telemetry, return discarded at all three call
        // sites. Two of them are synchronous, so it has to be answered
        // even though nothing reads it.
        //
        // The message we have seen it carry -- "Failed to get a valid
        // ballpark in time after trying N times" -- is **benign and
        // nothing to do with us**. `michelle.WaitForBallpark` polls 30
        // times at 1s for a space state that only `UpdateBallpark` sets,
        // and that is gated on `session.solarsystemid` and an in-flight
        // view. Docked, neither is true, so any caller passing
        // `doWait=True` burns 30s and gets None, which every one of them
        // handles. It is a UI component asking where we are in space
        // while there is no space.
        //
        // Reached through `sm.ProxySvc`, not `RemoteSvc`, so its CallReq
        // is addressed to a node rather than ANY.
        "clientStatLogger" => match req.method {
            "LogString" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Portraits for the character-select slots.
        //
        // **This call is one we bring on ourselves.** `EvePhoto.GetPortrait`
        // only queues a scene portrait when the image-server portrait is
        // missing *and* the session holds `ROLE_PROGRAMMER` (`[V]`: the role
        // mask is tested and the scene-portrait queue is skipped when it
        // is clear).
        //
        // We set that bit deliberately (see `docs/reference/session-roles.md`), so the
        // sandbox takes the developer path through the portrait code. The
        // answer is to serve the call, not to give up the role.
        //
        // The reply must be a **real dict**: `ScenePortraitQueue._FetchBatchData`
        // does `fetched.iteritems()` with no guard, so `None` is an
        // `AttributeError` in the worker. Empty is fine -- each value is
        // tested `if dna:` and a missing id just keeps the default
        // portrait icon.
        //
        // What must NOT happen here is a `UserError`: that is the
        // rate-limit signal, and the caller responds by pushing the batch
        // back on the queue and sleeping 5s, forever.
        // The security-modifier glyph beside each character's location.
        //
        // **This one cannot be answered with an empty container.** Its
        // caller goes straight from the call to an attribute lookup, with
        // no falsy guard anywhere in between:
        //
        //     21 CALL_FUNCTION 0
        //     30 LOAD_ATTR     Index
        //     33 LOAD_CONST    'solarSystemID'
        //     48 COMPARE_OP    6 (in)
        //     60 BINARY_SUBSCR
        //     70 LOAD_ATTR     security
        //     76 LOAD_ATTR     modifiedSecurity
        //
        // so `None`, `{}` and `[]` are all `AttributeError` -- nothing
        // but a real rowset has `.Index`. Note too that this call does
        // *not* go through `cfg.Recordset._Prime`, so the `(header, data)`
        // tuple that the owner primes use is wrong here.
        //
        // An empty `CRowset` is the honest answer: no system in this
        // sandbox has a modified security level, so `Index` yields an
        // empty index, `locationID in ...` is false and the caller
        // returns `''` -- no glyph, which is the correct rendering.
        //
        // The columns must be declared even with no rows. The pure-Python
        // `Rowset.Index` does `header.index(idName)` and raises
        // `ValueError` on a missing column whatever the row count, and
        // the native `CRowset` path hands the name to
        // `blue.pyos.XUtil_Index`, whose behaviour we cannot read. Naming
        // them costs nothing and makes both paths safe.
        //
        // `@Memoize(1)` memoises per locationID, not per session, so
        // expect one call per distinct system and more after the screen
        // closes. It has to stay cheap.
        // The same data again, under a different service, once in game.
        // Flagged here because it hangs the same way and is nearly free:
        // `SecurityService.Initialize` builds a dict comprehension over
        // `.iteritems()`, so this one wants a **dict**, not a rowset --
        // the opposite of `map.GetSecurityModifiedSystems` above. `{}`
        // gave an empty map, and the note here said that made the caller
        // "fall through to `cfg.mapSystemCache[...].pseudoSecurity`" as
        // though that were harmless. It is not: see [`modified_systems`].
        "securityMgr" => match req.method {
            "get_modified_systems" => Ok(modified_systems(reg)),
            _ => Err(reg.no_method(req)),
        },
        // Redeemable items (the "redeem" panel on the select screen).
        //
        // An empty **list**. The result is only iterated and `len()`d --
        // no `.Index`, no `.header`, no `__columns__` -- so a list is
        // enough and a rowset would be machinery for nothing.
        //
        // `None` is the trap here, and it is a subtle one: it survives
        // `GetRedeemTokenCount`, which guards with an explicit
        // `is None`, and then dies on `GET_ITER` in
        // `RedeemItemsContainer.Controller.load`, whose tasklet never
        // reaches `on_load()`. That reproduces exactly the hang we are
        // removing, one frame later.
        //
        // `[]` caches correctly, which is luck worth knowing about:
        // `redeemsvc.GetRedeemTokens` re-fetches on `reg.tokens is None`
        // rather than on falsiness, so an empty list is stored and asked
        // for once. Had CCP written `if not reg.tokens:` this would
        // re-call us on every access.
        // NOTE: `GetMultiCharactersTrainingSlots` is below, with the rest
        // of `userSvc`.
        "userSvc" => match req.method {
            // A dict: `IsAllCharacterTrainingSlotsUsed` does
            // `reg.GetMultipleCharacterTraining().items()` and then
            // `len(queues)`. `None` was an `AttributeError` on `.items`.
            // Empty means no extra training slots are in use, which is
            // true, and makes `activeQueues` the correct 1.
            "GetMultiCharactersTrainingSlots" => Ok(PyValue::Dict(vec![])),
            // NES-BACKEND: the redeem queue the store delivers into.
            "GetRedeemTokens" => Ok(crate::nes::tokens_value(reg, req.actor)),
            "ClaimRedeemTokens" => crate::nes::claim(reg, req),
            "TrashRedeemTokens" => crate::nes::trash(reg, req),
            _ => Err(reg.no_method(req)),
        },
        // The login population cap, and our first **bound object**.
        //
        // `characterSelection.TryEnterGame` does, on the click:
        //
        // it binds a `populationCap` moniker on the character and group, asks
        // for the character's load slot, and when more than one location comes
        // back offers alternative systems or moves the character `[V]`
        //
        // which is two round trips: `MachoResolveObject(bindParams)` to
        // find the node, then `MachoBindObject(bindParams, call)` with the
        // first call fused in. `dispatch::handle_bind` deals with the
        // second; both methods below are reached through it.
        //
        // `MachoResolveObject` returns a **node id**, and `None` is an
        // error the client reports as 'Resolution failure: ResolveObject
        // returned None'. We are one node, so the answer is simply us.
        //
        // `GetCharacterLoadSlot` returns a `{solarSystemID: slotKey}` map
        // of places the character may load. It is only ever `len()`-ed
        // and, when longer than one, offered to the player as a choice.
        // Empty means "no alternatives, load where you are", which is the
        // truth here and keeps the client out of
        // `MoveCharacterToNewSystem`.
        "populationCap" => match req.method {
            "GetCharacterLoadSlot" => Ok(PyValue::Dict(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // **The other half of the price of `ROLE_QA`.**
        //
        // `localizationClient.OnCharacterSessionChanged` fires this once
        // per character login, under
        //
        // a test of the role mask against `ROLE_QA`
        //
        // -- note the **all-of** form, `& X == X`, which is one of only
        // three ways the client tests the mask and the one that catches
        // people who assume every test is `& X`.
        //
        // The return value is discarded, and the handler runs under
        // `MollycoddledUthread`, so leaving it unanswered would be a
        // logged exception rather than a hang. Answered anyway: an
        // unanswered call is still a blocked tasklet, and this one fires
        // on every single login.
        //
        // Its sibling `GetAllTextChanges` is guarded by
        // `not blue.pyos.packaged`, which is false for us, so it never
        // arrives.
        "localizationServer" => match req.method {
            "UpdateLocalizationQAWrap" => Ok(PyValue::None),
            // Insider's "Manual Reload -> Localization". The client prints
            // a partial-success message when this is falsy, which is the
            // truth: we have no FSD pickle to reload.
            "ReloadFSDPickle" => Ok(PyValue::Bool(false)),
            _ => Err(reg.no_method(req)),
        },
        // Achievements. A dict with **exactly these two keys**, both
        // subscripted immediately:
        //
        // the client stores `info['completedDict']` and passes
        // `info['eventDict']` to `PopulateEventHandler` `[V]`
        //
        // `completedDict` is `{taskID: timestamp}` and `eventDict` is
        // `{eventName: count}`; both are iterated, so `{}` each.
        "monolithTaskTrackerMgr" => match req.method {
            "GetCompletedAchievementsAndClientEventCount" => Ok(pydict(vec![
                ("completedDict", PyValue::Dict(vec![])),
                ("eventDict", PyValue::Dict(vec![])),
            ])),
            // A fire-and-forget stats upload of two dicts whose return
            // value the caller discards, so `None` is not a stand-in
            // here -- it is the answer. Named explicitly all the same, so
            // it stops appearing on the UNANSWERED list and crowding out
            // the gaps that are real.
            "UpdateClientAchievmentsAndCounters" => Ok(PyValue::None),
            // `enterSpaceCamera` asks with no arguments whether to play
            // the first-time-in-space fly-in. `True` skips it. [A]: no
            // per-character warp history is kept; falsifier: the owner
            // wants a new character to see the fly-in once.
            "HasEverWarped" => Ok(PyValue::Bool(true)),
            _ => Err(reg.no_method(req)),
        },
        // In-game character creation (P-START-4, R-START §1): the three
        // calls the legacy creator makes that need the world mutably.
        "charUnboundMgr" => match req.method {
            "CreateCharacterWithDoll" => create_character_with_doll(reg, req),
            // `chooseNameSection.IsNameValid` compares the answer with its
            // `VALID` (1) and looks any other int up in its reason table
            // **[V]**. The second argument is the client's count of names
            // checked so far; nothing here limits it.
            "ValidateNameEx" => {
                let name = args::Args::new(req).text(0).unwrap_or_default();
                Ok(PyValue::Int(crate::start::validate_name(reg, &name)))
            }
            // The Randomize button (`chooseNameSection`, `GetValidRandomName
            // (raceID)`): the answer is marked valid without a check, so it
            // must be a name `ValidateNameEx` passes **[V]**.
            "GetValidRandomName" => {
                let race = args::Args::new(req).int(0).unwrap_or(0);
                let seed = crate::game_clock::clock::wall_system()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as u64);
                Ok(PyValue::Unicode(crate::start::random_name(reg, race, seed)))
            }
            // SOC-CHARDEL: the select screen's delete flow. A refusal is
            // raised, not returned: `DeleteCharacter`'s return value is shown
            // as a message key only after the "end" dialog has played.
            "PrepareCharacterForDelete" | "CancelCharacterDeletePrepare" | "DeleteCharacter" => {
                let Some(id) = args::Args::new(req).int(0) else {
                    return Err(reg.no_method(req));
                };
                let (caller, now) = (reg.caller(), crate::pyutil::filetime_now());
                let done = match req.method {
                    "PrepareCharacterForDelete" => {
                        crate::start::prepare_delete(reg, caller, id, now).map(PyValue::Int)
                    }
                    "CancelCharacterDeletePrepare" => {
                        crate::start::cancel_delete(reg, caller, id).map(|()| PyValue::None)
                    }
                    // Falsy is success.
                    _ => {
                        crate::start::delete_character(reg, caller, id, now).map(|()| PyValue::None)
                    }
                };
                done.map_err(|e| ServiceError::Raise(eve_proto::user_error(e.key(), vec![])))
            }
            // `CheckSkipTutorial` compares it with `> 1` **[V]**: the
            // characters this account may play.
            "GetNumCharacters" => {
                let caller = reg.caller();
                let n = reg
                    .config()
                    .characters
                    .iter()
                    .filter(|c| reg.may_play(caller, c))
                    .count();
                Ok(PyValue::Int(n as i64))
            }
            _ => reg.char_unbound_mgr(req),
        },
        "subscriptionMgr" => match req.method {
            "GetCloneGrade" => Ok(PyValue::Int(CLONE_STATE_OMEGA)),
            // Omega expiry, as a blue FILETIME. Cosmetic, but a visible
            // self-contradiction without it: `plexPanel.RefreshOmegaTime`
            // is
            //
            // `RefreshOmegaTime` shows the Omega header and a days-left line when
            // a time is given, and the Alpha header otherwise `[V]`
            //
            // so `None` is survivable and merely *wrong* -- the Pilot
            // License tab reads **Alpha** while `GetCloneGrade` directly
            // above says Omega.
            //
            // See [`FILETIME_2100`] for why the magnitude rather than the
            // date is the thing that matters here.
            "GetSubscriptionTime" => Ok(PyValue::Long(FILETIME_2100)),
            _ => Err(reg.no_method(req)),
        },
        "multiLoginBlocker" => match req.method {
            // Falsy raises `MultiLoginBlockedWindow` and refuses the login,
            // so **`true` stays the default**. Only called at all when
            // `computer_hash` is not None.
            //
            // This runs *before* a character is selected, so there is no
            // actor here and nothing to compare against the session table:
            // the call is about the account and the machine, and the thing it
            // would want to refuse -- a second connection claiming a
            // character that is already online -- is not knowable until
            // `SelectCharacterID`.
            //
            // So the one-controller-per-character rule belongs there rather
            // than here, and is not in this round. EvEJS builds a whole lease
            // for it (`services/online/characterControlRuntime.js`: claim,
            // renew, validate, release, with TTLs and secrets) because they
            // have a *second transport* -- a web pilot -- competing for one
            // character. We have one transport, so that would be a lock for
            // a race that does not exist. Revisit it if bots arrive on a
            // second transport.
            "Login" => Ok(PyValue::Bool(true)),
            _ => Err(reg.no_method(req)),
        },
        // Chat, which is **not machoNet**: a second protocol, a direct
        // XMPP TCP connection the client opens for itself to port 5222.
        // `crate::chat` is that server; `docs/lanes/chat.md` is its spec.
        // What these answers decide is whether the client dials, and where.
        //
        // From `xmppchatclient/xmppchatsvc.py` (lines are `co_lnotab`'s):
        //
        // `xmppchatsvc.py` (`[V]`, line numbers from the code object's own
        // table): `_CreateConnection` (`:332-344`) takes the hostname from the
        // chat manager and the address from `get_ejabberd_address`; with no
        // address it sends an error event and stops, otherwise it builds the
        // connection. `get_ejabberd_address` (`:370-375`) returns the address
        // from `GetEjabberdAddress` on the network service when it is
        // non-empty, and otherwise the manager's deprecated-prefs fallback.
        //
        // **`GetDeprecatedPrefsFallback` is the whole lever**: its value
        // becomes `ejabberd_address` verbatim and the client then does
        // `s.connect((ejabberd_address, 5222))`. So there are two outcomes:
        //
        // - **Chat off** (`Registry::chat()` is `None`, the default): both
        //   methods answer `None`. One `AttributeError` in a tasklet of its
        //   own, once per login -- `send_honey_event` reads
        //   `self.xmppConnection.hostname` off a `None`, CCP's bug on their
        //   own disabled path -- and **no socket is ever created**. That is
        //   the right answer whenever nothing listens on 5222: the jail's
        //   policy is `drop`, so a dial would hang for a SYN timeout and then
        //   raise a modal `RetryChatServerConnection` inside a `while True:`.
        //   `''` is not an escape: it is `is not None`.
        // - **Chat on**: `Hostname` is our XMPP domain and the fallback is an
        //   **IP literal** (the stub DNS answers only
        //   `*.servers.eveonline.com`): the local address the caller's own
        //   machoNet connection arrived on, unless `--chat-advertise` set
        //   one. In the jail that is `10.137.0.1`, which `jail-setup.sh`
        //   opens 5222 on and nothing else.
        //
        // `ResyncSystemChannelAccess` is read by `while channelsToJoin is
        // None:` with a `sleep(5)` (`:420-426`), so it is **never `None`**:
        // a list of `str`, `[]` only for a character with no rooms (which
        // leaves the loop after one sleep; `None` would not). It also applies
        // the caller's membership to the hub right away, so the joins that
        // follow it do not wait for the next reconcile pass.
        //
        // `AddCharacterToChannel` is the path our players' private chats
        // take: `Invite()` checks `session.role & (CHTADMINISTRATOR | GML |
        // LEGIONEER)` and the sandbox mask has all three. `GrantChannel
        // Ownership` and `EnsureResourceWarsChannelExists` discard their
        // result. `CreatePlayerOwnedChannel` makes a `player_<n>` channel
        // (lane soc-chan1, #162); `GetHelpChannel` stays unanswered: out of
        // scope (`docs/lanes/chat.md`, "Not done").
        "XmppChatMgr" => {
            let Some(hub) = reg.chat().cloned() else {
                return match req.method {
                    "Hostname" | "GetDeprecatedPrefsFallback" => Ok(PyValue::None),
                    _ => Err(reg.no_method(req)),
                };
            };
            match req.method {
                "Hostname" => Ok(bstr(crate::chat::HOSTNAME)),
                "GetDeprecatedPrefsFallback" => {
                    let client_ids: Vec<i64> = req
                        .actor
                        .map(|a| {
                            reg.sessions()
                                .of_character(a)
                                .iter()
                                .map(|e| e.client_id)
                                .collect()
                        })
                        .unwrap_or_default();
                    Ok(hub
                        .advertise_for(&client_ids)
                        .map_or(PyValue::None, |ip| bstr(&ip.to_string())))
                }
                "ResyncSystemChannelAccess" => {
                    let Some(actor) = req.actor else {
                        return Ok(PyValue::List(vec![]));
                    };
                    let rooms = crate::chat::system_rooms(reg, actor);
                    // The directory first, then the hub (the ordering rule of
                    // `chat_dir`): a reconcile pass can then never apply an
                    // older entry over this one.
                    crate::chat_dir::refresh(reg, actor);
                    hub.sync_character(actor, crate::chat::membership(reg, actor));
                    Ok(PyValue::List(rooms.iter().map(|r| bstr(r)).collect()))
                }
                "CreatePrivateChannel" => {
                    let (Some(actor), Some(other)) = (req.actor, args::Args::new(req).int(0))
                    else {
                        return Err(reg.no_method(req));
                    };
                    Ok(bstr(&hub.create_private(actor, other)))
                }
                "AddCharacterToChannel" => {
                    let a = args::Args::new(req);
                    if let (Some(room), Some(who)) = (a.text(0), a.int(1)) {
                        match req.actor {
                            // A player channel: the owner's or admin's invite,
                            // or a GM-roled pilot's.
                            Some(actor) if hub.is_player_room(&room) => {
                                hub.add_to_player(actor, chat_roled(reg), &room, who);
                            }
                            _ => {
                                hub.add_to_channel(&room, who);
                            }
                        }
                    }
                    Ok(PyValue::None)
                }
                // The Create button: the channel is `player_<n>`, owned by the
                // caller, called what they typed. The client joins it next.
                "CreatePlayerOwnedChannel" => {
                    let (Some(actor), Some(name)) = (req.actor, args::Args::new(req).text(0))
                    else {
                        return Err(reg.no_method(req));
                    };
                    Ok(bstr(&hub.create_player(actor, &name)))
                }
                // Over macho only corp and alliance rooms (`SetMotd`); a
                // player channel's subject is an XMPP message. Answered for
                // the player rooms; the derived rooms' MOTD is not kept.
                "SetMotd" => {
                    let a = args::Args::new(req);
                    let room = a.text(0).and_then(|r| crate::chat::room_name_of(&r));
                    if let (Some(actor), Some(room), Some(text)) = (req.actor, room, a.text(1)) {
                        hub.set_motd(actor, &room, &text);
                    }
                    Ok(PyValue::None)
                }
                // A GM or chat-admin pilot takes ownership of a player
                // channel: the client calls it before it opens the settings
                // window and when a password is asked (it never prompts such
                // a pilot). The role is the caller's own session role, so a
                // plain pilot who calls it by hand gets nothing.
                "GrantChannelOwnership" => {
                    let room = args::Args::new(req).text(0);
                    if let (Some(actor), Some(room)) = (req.actor, room)
                        && chat_roled(reg)
                    {
                        hub.grant_ownership(&room, actor);
                    }
                    Ok(PyValue::None)
                }
                "EnsureResourceWarsChannelExists" => Ok(PyValue::None),
                _ => Err(reg.no_method(req)),
            }
        }
        // `xmppchatsvc.GetAuthenticationToken` (`:385-396`): the XMPP
        // password, fetched over machoNet before every connect. **It must be
        // answered**: an exception here escapes `Connect`, `Login` catches
        // only `socket.error`, and the login tasklet dies. A falsy answer
        // makes the client send the literal `'ejabberd'`, which the chat
        // server refuses -- so with chat off, `None` is honest.
        "chatAuthenticationService" => match req.method {
            "GetAuthenticationToken" => match (reg.chat(), req.actor) {
                (Some(hub), Some(actor)) => Ok(bstr(&hub.mint_token(actor))),
                _ => Ok(PyValue::None),
            },
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// Every solar system whose security has been modified, keyed by id.
///
/// **This is what makes the sandbox a sandbox, and `{}` here silently
/// forbade every fight.**
///
/// `cfg.mapSystemCache` is **not** a server cache -- it is
/// `res:/staticdata/systems.static`, shipped, 8,490 rows (`eveCfg.py:795`)
/// `[V]` -- and our test system **30000001 has `securityStatus` 0.8583**,
/// which is high sec. So `securitySvc.get_modified_security_class` fell
/// through to that number, `GetSafetyLevelRestrictionForAttackingTarget`
/// ran the whole crimewatch tree, and `consider.DoAttackConfirmations`
/// returned `False`: the module never activated, **nothing reached the
/// server at all**, and the only trace anywhere was one
/// `LogInfo('Safeties activated')` in the client log. A server-side search
/// for the missing call would have found nothing, because there was no call.
///
/// `securitySvc.Initialize` (`:53-57`) does
/// `{ssid: modified_security_level(data.solarSystemID,
/// calculate_pseudo_security(data.modifiedSecurity)) for ssid, data in
/// rows.iteritems()}`, so the **values are read by attribute** -- `KeyVal`,
/// not a dict -- and the keys are solar-system ids.
///
/// **Exactly `0.0`, never `0.01`.** `calculate_pseudo_security:26` lifts
/// anything in `(0.0, 0.05)` to `0.05`, which is low sec and still gated
/// `[V]`. Zero is the only value in that neighbourhood that means what we
/// mean.
///
/// `[I]` on the cosmetic consequence: the UI will draw the system's security
/// as `0.0` with a "security modified" icon. For a server whose point is
/// players fighting each other that is arguably just true.
///
/// Derived from the characters' own systems rather than from a constant, so a
/// second system added later is covered without anyone remembering this
/// function exists.
///
/// The list is `crimewatch::declared_null_systems`, so crimewatch's "no
/// suspects in zero sec" reads the same systems the client was told about.
fn modified_systems(reg: &Registry) -> PyValue {
    PyValue::Dict(
        crate::crimewatch::declared_null_systems(reg)
            .into_iter()
            .map(|id| {
                (
                    crate::pyutil::keyval(vec![
                        ("solarSystemID", PyValue::Int(id)),
                        ("modifiedSecurity", PyValue::Real(0.0)),
                    ]),
                    PyValue::Int(id),
                )
            })
            .collect(),
    )
}

/// Whether the call in progress comes from a session with the GM or
/// chat-admin role, the two the client's chat code tests (`Invite`,
/// `_GetPasswordHelper`). A server-made call has none.
fn chat_roled(reg: &Registry) -> bool {
    match reg.caller() {
        crate::service::Caller::Session { role, .. } => {
            role & (crate::roles::ROLE_GML | crate::roles::ROLE_CHTADMINISTRATOR) != 0
        }
        crate::service::Caller::Internal => false,
    }
}

/// `charUnboundMgr.CreateCharacterWithDoll(charactername, raceID,
/// bloodlineID, genderID, ancestryID, charInfo, portraitInfo, schoolID,
/// None, qa_starter_system_id)`: ten positional arguments in this order
/// **[V]** (`ccSvc.CreateCharacterWithDoll`, R-START §1.1; EvEJS's order
/// is another build's). Answers the new `charID`; the client then selects
/// it. The doll (`charInfo`, `portraitInfo`) is kept by
/// [`crate::paperdoll::store`] once it passes the doll's shape check (a
/// refused doll is logged and the character made without one), and
/// `ancestryID` is always `ancestryUndefined` **[V]**.
///
/// A refused name raises its `CharNameInvalid*` key, a refused school
/// [`crate::start::SCHOOL_ERROR_KEY`] **[A]** and a bloodline of another
/// race [`crate::start::BLOODLINE_ERROR_KEY`] **[A]**
/// (`crate::start::CreateError::error_key`); nothing is created.
fn create_character_with_doll(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let a = args::Args::new(req);
    let (Some(name), Some(race_id), Some(bloodline_id), Some(gender_id), Some(school_id)) =
        (a.text(0), a.int(1), a.int(2), a.int(3), a.int(7))
    else {
        return Err(ServiceError::BadArguments {
            service: req.service.to_owned(),
            method: req.method.to_owned(),
            why: "want (name, raceID, bloodlineID, genderID, ancestryID, charInfo, \
                  portraitInfo, schoolID, None, qaSystemID)"
                .to_owned(),
        });
    };
    let user_id = match reg.caller() {
        Caller::Session { user_id, .. } => user_id,
        Caller::Internal => reg.config().user_id,
    };
    let new = crate::start::NewCharacter {
        race_id,
        bloodline_id,
        gender_id,
        school_id: Some(school_id),
    };
    match crate::start::create_with_doll(reg, user_id, &name, new) {
        Ok(id) => {
            crate::outln!(
                "[start] user {user_id} created {name:?}: character {id}, school {school_id}"
            );
            if let (Some(ci), Some(pi)) = (a.get(5), a.get(6))
                && let Err(why) = crate::paperdoll::store(reg, id, ci, pi)
            {
                crate::errln!("[paperdoll] character {id}: the creator's doll refused: {why}");
            }
            Ok(PyValue::Int(id))
        }
        Err(e) => {
            crate::errln!(
                "[start] user {user_id}: CreateCharacterWithDoll {name:?} refused: {e:?}"
            );
            Err(ServiceError::Raise(eve_proto::user_error(
                e.error_key(),
                Vec::new(),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

    // ---- SOC-CHARDEL: the select screen's delete flow ----------------------
    // Invented ids only. The refusal keys are the client's own (`[V]`,
    // `findings/user-errors.tsv`), so they may be asserted.

    const OWNER_A: i64 = 1;
    const OWNER_B: i64 = 2;
    const MINE: i64 = 91_000_001;
    const THEIRS: i64 = 91_000_002;

    fn deletion_world(wait_secs: i64) -> Registry {
        let c = |id: i64, ship: i64, name: &str, owner: i64| Character {
            character_id: id,
            ship_id: ship,
            name: name.to_owned(),
            owner_user_id: Some(owner),
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                c(MINE, 191_000_001, "Mine", OWNER_A),
                c(THEIRS, 191_000_002, "Theirs", OWNER_B),
            ],
            // `--accounts`: a session plays only its own.
            catch_all: None,
            ..SandboxConfig::default()
        });
        reg.rules_mut().char_delete_secs = wait_secs;
        reg
    }

    fn account(user_id: i64) -> Caller {
        Caller::Session {
            user_id,
            role: crate::roles::SANDBOX_DOCKED,
        }
    }

    fn unbound(
        reg: &mut Registry,
        caller: Caller,
        method: &str,
        id: i64,
    ) -> Result<PyValue, ServiceError> {
        let args = [PyValue::Int(id)];
        reg.dispatch_as(
            caller,
            &Request {
                service: "charUnboundMgr",
                method,
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
    }

    /// The `characterID -> deletePrepareDateTime` of the select list.
    fn select_rows(reg: &mut Registry, caller: Caller) -> Vec<(i64, PyValue)> {
        let got = unbound_plain(reg, caller, "GetCharacterSelectionData");
        let PyValue::Tuple(parts) = got else {
            panic!("a 4-tuple")
        };
        let PyValue::List(rows) = &parts[2] else {
            panic!("a list of rows")
        };
        rows.iter()
            .map(|row| {
                let PyValue::Instance { state, .. } = row else {
                    panic!("a KeyVal")
                };
                let Some(PyValue::Int(id)) = crate::pyutil::dict_get(state, b"characterID") else {
                    panic!("characterID")
                };
                let due = crate::pyutil::dict_get(state, b"deletePrepareDateTime")
                    .cloned()
                    .expect("deletePrepareDateTime");
                (*id, due)
            })
            .collect()
    }

    fn unbound_plain(reg: &mut Registry, caller: Caller, method: &str) -> PyValue {
        reg.dispatch_as(
            caller,
            &Request {
                service: "charUnboundMgr",
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .expect("answered")
    }

    fn refused_key(r: Result<PyValue, ServiceError>) -> String {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
            panic!("expected a raised UserError, got {r:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("(class, args)")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("args tuple")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    #[test]
    fn prepare_then_cancel_restores_the_row() {
        let mut reg = deletion_world(3_600);
        let me = account(OWNER_A);
        assert_eq!(select_rows(&mut reg, me)[0], (MINE, PyValue::None));
        let due = match unbound(&mut reg, me, "PrepareCharacterForDelete", MINE) {
            Ok(PyValue::Int(due)) => due,
            other => panic!("a FILETIME, got {other:?}"),
        };
        assert!(due > crate::pyutil::filetime_now(), "an hour away");
        assert_eq!(select_rows(&mut reg, me)[0], (MINE, PyValue::Int(due)));
        // Asked again, the first time stands.
        assert_eq!(
            unbound(&mut reg, me, "PrepareCharacterForDelete", MINE),
            Ok(PyValue::Int(due))
        );
        unbound(&mut reg, me, "CancelCharacterDeletePrepare", MINE).expect("cancelled");
        assert_eq!(select_rows(&mut reg, me)[0], (MINE, PyValue::None));
        // Cancelling a character that is not waiting changes nothing.
        unbound(&mut reg, me, "CancelCharacterDeletePrepare", MINE).expect("a no-op");
    }

    #[test]
    fn delete_before_the_wait_is_refused() {
        let mut reg = deletion_world(3_600);
        let me = account(OWNER_A);
        let key = |r| refused_key(r);
        // Never put up for deletion.
        assert_eq!(
            key(unbound(&mut reg, me, "DeleteCharacter", MINE)),
            "CanNotDeleteCharacterForUnknownReason"
        );
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        assert_eq!(
            key(unbound(&mut reg, me, "DeleteCharacter", MINE)),
            "CanNotDeleteCharacterForUnknownReason"
        );
        assert_eq!(select_rows(&mut reg, me).len(), 1, "still on the list");
        assert!(!crate::start::is_deleted(reg.deltas(), MINE));
    }

    #[test]
    fn another_accounts_character_is_refused() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        for method in [
            "PrepareCharacterForDelete",
            "CancelCharacterDeletePrepare",
            "DeleteCharacter",
        ] {
            assert_eq!(
                refused_key(unbound(&mut reg, me, method, THEIRS)),
                "CanNotDeleteCharacterForUnknownReason",
                "{method}"
            );
        }
        assert!(crate::start::delete_due(reg.deltas(), THEIRS).is_none());
        // A character that does not exist is the same refusal.
        assert!(unbound(&mut reg, me, "PrepareCharacterForDelete", 91_999_999).is_err());
    }

    #[test]
    fn an_online_character_is_refused() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: OWNER_A,
            character_id: Some(MINE),
        });
        assert!(reg.sessions().is_online(MINE));
        assert_eq!(
            refused_key(unbound(&mut reg, me, "DeleteCharacter", MINE)),
            "CanNotDeleteCharacterForUnknownReason"
        );
        assert!(!crate::start::is_deleted(reg.deltas(), MINE));
    }

    fn found_corp(reg: &mut Registry) {
        reg.character_mut(MINE).expect("mine").balance = 5_000_000.0;
        let uni = |t: &str| PyValue::Unicode(t.to_owned());
        let args = [
            uni("Fci Test Corp"),
            uni("FTC"),
            uni("We mine."),
            uni("http://"),
            PyValue::Real(0.1),
            PyValue::Int(419),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Bool(true),
            PyValue::Bool(false),
            PyValue::Real(0.0),
        ];
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(MINE),
        })
        .expect("founded");
    }

    #[test]
    fn a_ceo_is_refused() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        found_corp(&mut reg);
        assert!(crate::service::corp::is_ceo(&reg, MINE));
        for method in ["PrepareCharacterForDelete", "DeleteCharacter"] {
            assert_eq!(
                refused_key(unbound(&mut reg, me, method, MINE)),
                "CanNotDeleteCEO",
                "{method}"
            );
        }
        assert!(crate::start::delete_due(reg.deltas(), MINE).is_none());
    }

    #[test]
    fn a_waiting_character_made_ceo_can_still_cancel() {
        let mut reg = deletion_world(3_600);
        let me = account(OWNER_A);
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        // Handed the corp while waiting (a resignation names no wait).
        found_corp(&mut reg);
        assert!(crate::service::corp::is_ceo(&reg, MINE));
        assert_eq!(
            unbound(&mut reg, me, "CancelCharacterDeletePrepare", MINE),
            Ok(PyValue::None)
        );
        assert!(crate::start::delete_due(reg.deltas(), MINE).is_none());
        // Still no way to start or finish a delete.
        assert_eq!(
            refused_key(unbound(&mut reg, me, "PrepareCharacterForDelete", MINE)),
            "CanNotDeleteCEO"
        );
    }

    #[test]
    fn a_pilot_outside_a_player_corp_is_not_moved_by_delete() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        let school = 98_000_777;
        reg.character_mut(MINE).expect("mine").corporation_id = school;
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        unbound(&mut reg, me, "DeleteCharacter", MINE).expect("deleted");
        assert_eq!(reg.character(MINE).map(|c| c.corporation_id), Some(school));
    }

    #[test]
    fn a_bot_is_never_removed() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        reg.character_mut(MINE).expect("mine").is_bot = true;
        for method in [
            "PrepareCharacterForDelete",
            "CancelCharacterDeletePrepare",
            "DeleteCharacter",
        ] {
            assert_eq!(
                refused_key(unbound(&mut reg, me, method, MINE)),
                "CanNotDeleteCharacterForUnknownReason",
                "{method}"
            );
        }
    }

    #[test]
    fn a_deleted_character_leaves_the_select_list_but_keeps_its_name_row() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        assert_eq!(
            unbound(&mut reg, me, "DeleteCharacter", MINE),
            Ok(PyValue::None),
            "falsy is success"
        );
        assert!(select_rows(&mut reg, me).is_empty());
        assert_eq!(
            unbound_plain(&mut reg, me, "GetNumCharacters"),
            PyValue::Int(0)
        );
        // The row stays, so mail, killmails and corp history still resolve it.
        assert_eq!(
            reg.character(MINE).map(|c| c.name.as_str()),
            Some("Mine"),
            "a tombstone, not an erasure"
        );
        // The other account is untouched.
        assert_eq!(select_rows(&mut reg, account(OWNER_B)).len(), 1);
        // Nothing more can be done to it.
        assert!(unbound(&mut reg, me, "DeleteCharacter", MINE).is_err());
        assert!(unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).is_err());
    }

    #[test]
    fn a_deleted_member_leaves_its_corporation_and_its_row_stays() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        let corp = 98_000_555;
        reg.corporations.enrol_for_test(MINE, corp, 0);
        reg.character_mut(MINE).expect("mine").corporation_id = corp;
        assert!(reg.corporations.membership(MINE).is_some());
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        unbound(&mut reg, me, "DeleteCharacter", MINE).expect("deleted");
        assert!(reg.corporations.membership(MINE).is_none(), "left the corp");
        assert_eq!(
            reg.character(MINE).map(|c| c.corporation_id),
            Some(crate::model::NPC_CORPORATION_ID)
        );
    }

    #[test]
    fn it_cannot_be_selected() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        assert!(reg.character_change_as(me, MINE).is_some());
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        unbound(&mut reg, me, "DeleteCharacter", MINE).expect("deleted");
        assert!(reg.character_change_as(me, MINE).is_none());
        assert!(!reg.may_play(Caller::Internal, reg.character(MINE).expect("row")));
    }

    #[test]
    fn the_tombstone_survives_a_restart() {
        let mut reg = deletion_world(0);
        let me = account(OWNER_A);
        unbound(&mut reg, me, "PrepareCharacterForDelete", MINE).expect("prepared");
        unbound(&mut reg, me, "PrepareCharacterForDelete", THEIRS).expect_err("not mine");
        unbound(&mut reg, me, "DeleteCharacter", MINE).expect("deleted");
        // Rows are what a save keeps: a fresh world given the same rows.
        let mut again = deletion_world(0);
        for (kind, rows) in reg.deltas().kinds() {
            for (k, v) in rows.iter() {
                again.deltas_mut().put(kind, k.clone(), v.clone());
            }
        }
        assert!(crate::start::is_deleted(again.deltas(), MINE));
        assert!(select_rows(&mut again, me).is_empty());
        assert!(again.character_change_as(me, MINE).is_none());
    }

    #[test]
    fn a_wait_not_yet_run_is_carried_across_a_restart() {
        let mut reg = deletion_world(3_600);
        let me = account(OWNER_A);
        let Ok(PyValue::Int(due)) = unbound(&mut reg, me, "PrepareCharacterForDelete", MINE) else {
            panic!("prepared")
        };
        assert_eq!(crate::start::delete_due(reg.deltas(), MINE), Some(due));
        // A later change of the wait does not move a character already waiting.
        reg.rules_mut().char_delete_secs = 0;
        assert_eq!(
            unbound(&mut reg, me, "PrepareCharacterForDelete", MINE),
            Ok(PyValue::Int(due))
        );
    }

    /// **The sandbox's systems are declared exactly zero-sec, and `0.01`
    /// would not do.**
    ///
    /// `securitySvc.calculate_pseudo_security:26` lifts anything in
    /// `(0.0, 0.05)` to `0.05`, which is low sec and still gated by the
    /// client's safety tree `[V]`. So a well-meaning edit to a "nearly zero"
    /// value silently restores the bug this function was written to fix --
    /// and the symptom is that no weapon fires and *nothing reaches the
    /// server*, which is the hardest failure in this project to diagnose
    /// from our side.
    ///
    /// The other half is the shape: `securitySvc.Initialize` reads
    /// `data.solarSystemID` by **attribute**, so the values have to be
    /// `KeyVal`s and not dict entries.
    #[test]
    fn the_sandbox_systems_are_declared_exactly_zero_sec() {
        let reg = Registry::new(SandboxConfig::default());
        let PyValue::Dict(rows) = modified_systems(&reg) else {
            panic!("a dict keyed by solar-system id")
        };
        assert!(
            !rows.is_empty(),
            "a sandbox with no systems cannot be fought in"
        );
        let wanted: Vec<i64> = reg
            .config()
            .characters
            .iter()
            .filter_map(|c| c.solar_system_id)
            .collect();
        for (value, key) in &rows {
            // `(value, key)`: marshal dict entries are inverted on the wire.
            let PyValue::Int(id) = key else {
                panic!("keyed by solar-system id, got {key:?}")
            };
            assert!(wanted.contains(id), "a system nobody is in: {id}");
            // A `KeyVal` is an `Instance` whose state is the attribute
            // dict, so the attributes are one level in -- which is what
            // makes `data.solarSystemID` work on the client.
            let PyValue::Instance { state, .. } = value else {
                panic!("the values are KeyVals, read by attribute: {value:?}")
            };
            let security = crate::pyutil::dict_get(state, b"modifiedSecurity")
                .cloned()
                .unwrap_or_else(|| panic!("no modifiedSecurity on {value:?}"));
            assert_eq!(
                security,
                PyValue::Real(0.0),
                "exactly zero: anything in (0.0, 0.05) is lifted to 0.05 by \
                 calculate_pseudo_security, which is low sec and still gated"
            );
            assert_eq!(
                crate::pyutil::dict_get(state, b"solarSystemID").cloned(),
                Some(PyValue::Int(*id)),
                "read by attribute, so it has to be there"
            );
        }
    }

    /// `skillsvc` does `max(0, value - now)` on this: a number, in the past.
    #[test]
    fn the_next_alpha_injection_is_a_filetime_in_the_past() {
        let mut reg = Registry::new(SandboxConfig::default());
        let req = Request {
            service: "alphaInjectorMgr",
            method: "GetNextAvailableInjection",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        let got = dispatch(&mut reg, &req).expect("answered");
        assert_eq!(got, PyValue::Long(0));
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

    /// #139: the arms that read an id through `Args` take the same shapes
    /// they always did -- an integer, and nothing else, is an id.
    #[test]
    fn id_arguments_read_through_args_accept_only_integers() {
        let mut reg = Registry::new(SandboxConfig::default());
        let mut ask = |service: &str, method: &str, args: &[PyValue]| {
            dispatch(
                &mut reg,
                &Request {
                    service,
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: None,
                },
            )
        };
        // Not an id: the delete flow answers as an unknown method, as before.
        for bad in [PyValue::Bool(true), PyValue::None, uni_arg("7")] {
            assert!(matches!(
                ask("charUnboundMgr", "PrepareCharacterForDelete", &[bad]),
                Err(ServiceError::NoSuchMethod { .. })
            ));
        }
        assert!(matches!(
            ask("charUnboundMgr", "DeleteCharacter", &[]),
            Err(ServiceError::NoSuchMethod { .. })
        ));
        // A race that is not an integer reads as 0; the answer is still a name.
        let name = ask("charUnboundMgr", "GetValidRandomName", &[uni_arg("x")]);
        assert!(matches!(name, Ok(PyValue::Unicode(_))));
        // An owner that is not an integer owns no stations.
        let none = ask(
            "config",
            "GetStationSolarSystemsByOwner",
            &[PyValue::Bool(true)],
        );
        assert!(
            matches!(none, Ok(PyValue::List(ref l)) if l.is_empty()),
            "{none:?}"
        );
        // The name check reads a str or a unicode alike, and a miss as empty.
        let a = ask(
            "charUnboundMgr",
            "ValidateNameEx",
            &[crate::pyutil::bstr("Aaa Bbb")],
        );
        let b = ask("charUnboundMgr", "ValidateNameEx", &[uni_arg("Aaa Bbb")]);
        assert_eq!(a.expect("a"), b.expect("b"));
    }

    fn uni_arg(t: &str) -> PyValue {
        PyValue::Unicode(t.to_owned())
    }

    fn call(reg: &mut Registry, service: &str, method: &str) -> Result<PyValue, ServiceError> {
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
    }

    /// The Create button's call makes a player channel owned by the caller,
    /// and `SetMotd` over macho is the owner's alone.
    #[test]
    fn create_player_owned_channel_answers_a_room_the_caller_owns() {
        let actor = Character::default().character_id;
        let mut reg = Registry::new(SandboxConfig::default());
        let hub = crate::chat::ChatHub::new(crate::chat::ChatConfig::default());
        reg.set_chat(std::sync::Arc::clone(&hub));
        let call = |reg: &mut Registry, method: &str, args: &[PyValue], who: i64| {
            dispatch(
                reg,
                &Request {
                    service: "XmppChatMgr",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(who),
                },
            )
        };
        let room = call(
            &mut reg,
            "CreatePlayerOwnedChannel",
            &[bstr("Friends")],
            actor,
        )
        .unwrap();
        let PyValue::Bytes(room) = room else {
            panic!("a str is expected");
        };
        let room = String::from_utf8(room).unwrap();
        assert!(room.starts_with("player_"));
        let jid = bstr(&format!("{room}@conference.{}", crate::chat::HOSTNAME));
        // A stranger's SetMotd is answered and changes nothing; the owner's sticks.
        call(&mut reg, "SetMotd", &[jid.clone(), bstr("no")], actor + 1).unwrap();
        call(&mut reg, "SetMotd", &[jid, bstr("hi")], actor).unwrap();
        let rows = hub.take_dirty();
        let stored = rows.iter().find(|(k, _)| k[0] == room).unwrap();
        assert_eq!(stored.1.as_ref().unwrap()[3], "hi");
    }

    /// `GrantChannelOwnership` and `AddCharacterToChannel` on a player
    /// channel obey the caller's session role, not the call's say-so.
    #[test]
    fn channel_ownership_and_invites_follow_the_session_role() {
        use crate::service::Caller;
        let (owner, gm, plain, guest) = (11, 12, 13, 14);
        let mut reg = Registry::new(SandboxConfig::default());
        let hub = crate::chat::ChatHub::new(crate::chat::ChatConfig::default());
        reg.set_chat(std::sync::Arc::clone(&hub));
        let room = hub.create_player(owner, "Friends");
        let as_role = |reg: &mut Registry, role: i64, method: &str, args: &[PyValue], who: i64| {
            reg.dispatch_as(
                Caller::Session { user_id: 1, role },
                &Request {
                    service: "XmppChatMgr",
                    method,
                    args,
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(who),
                },
            )
        };
        hub.take_dirty();
        // A pilot without the role gets nothing from either call.
        let player = crate::roles::PLAYER_ONLY;
        as_role(
            &mut reg,
            player,
            "GrantChannelOwnership",
            &[bstr(&room)],
            plain,
        )
        .unwrap();
        let add = [bstr(&room), PyValue::Int(guest)];
        as_role(&mut reg, player, "AddCharacterToChannel", &add, plain).unwrap();
        assert!(hub.take_dirty().is_empty(), "nothing changed");
        // The GM role takes ownership; the row records it.
        as_role(
            &mut reg,
            crate::roles::ROLE_GML,
            "GrantChannelOwnership",
            &[bstr(&room)],
            gm,
        )
        .unwrap();
        let stored = hub.take_dirty();
        let row = stored[0].1.as_ref().unwrap();
        assert!(row[5].contains(&format!("{gm}:owner")), "{row:?}");
    }

    /// **Rewritten on purpose** (W8 chat lane). This test used to be
    /// `xmpp_chat_stays_unconfigured_so_the_client_never_dials`, which pinned
    /// both answers to `None` because no chat server existed and a dial out
    /// of a jail that drops 5222 is a modal retry loop. The reason it gave
    /// still holds, so it still binds **when chat is off**: that is the
    /// default, and both answers stay exactly `None` -- not merely falsy,
    /// because `''` takes the dialling branch too (`is not None`).
    #[test]
    fn with_chat_off_the_client_is_never_given_an_address() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert!(reg.chat().is_none(), "chat must be opt-in");
        for method in ["Hostname", "GetDeprecatedPrefsFallback"] {
            assert_eq!(
                call(&mut reg, "XmppChatMgr", method).unwrap(),
                PyValue::None,
                "{method} must stay None with chat off: any string, '' \
                 included, makes xmppchatsvc connect() to it"
            );
        }
    }

    /// With chat on, the fallback is the IP literal the caller's own
    /// machoNet connection arrived on, as a `str`.
    #[test]
    fn with_chat_on_the_client_is_told_where_its_own_connection_landed() {
        let actor = Character::default().character_id;
        let mut reg = Registry::new(SandboxConfig::default());
        let hub = crate::chat::ChatHub::new(crate::chat::ChatConfig::default());
        reg.set_chat(std::sync::Arc::clone(&hub));
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 7,
            session_id: 7,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(7, actor);
        hub.note_client_addr(7, std::net::Ipv4Addr::new(10, 137, 0, 1).into());
        let as_actor = |reg: &mut Registry, method: &str| {
            dispatch(
                reg,
                &Request {
                    service: "XmppChatMgr",
                    method,
                    args: &[],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(actor),
                },
            )
        };
        assert_eq!(
            as_actor(&mut reg, "Hostname").unwrap(),
            bstr(crate::chat::HOSTNAME)
        );
        assert_eq!(
            as_actor(&mut reg, "GetDeprecatedPrefsFallback").unwrap(),
            bstr("10.137.0.1")
        );
    }

    /// **Rewritten on purpose** (W8 chat lane). This test used to be
    /// `resync_system_channel_access_has_no_arm_on_purpose`: with no chat
    /// server there was no good answer, so there was no arm. There is one
    /// now, and the property it guarded is the one asserted here:
    /// `_JoinSystemChannels` loops `while channelsToJoin is None:` with a
    /// `sleep(5)` and no bound, so the answer is **never `None`** -- it is
    /// the list of rooms the world puts the caller in, as `str`s.
    #[test]
    fn resync_system_channel_access_is_a_list_and_never_none() {
        let me = Character::default();
        let mut reg = Registry::new(SandboxConfig::default());
        reg.set_chat(crate::chat::ChatHub::new(crate::chat::ChatConfig::default()));
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 7,
            session_id: 7,
            user_id: 1,
            character_id: None,
        });
        reg.session_selected_character(7, me.character_id);
        let got = dispatch(
            &mut reg,
            &Request {
                service: "XmppChatMgr",
                method: "ResyncSystemChannelAccess",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(me.character_id),
            },
        )
        .unwrap();
        assert_eq!(
            got,
            PyValue::List(vec![
                bstr(&format!("local_{}", me.solar_system_id.unwrap())),
                bstr(&format!("corp_{}", me.corporation_id)),
            ])
        );
    }

    /// The token must be answered with a `str` when chat is on -- an
    /// exception here kills the client's login tasklet -- and it binds to the
    /// caller.
    #[test]
    fn the_chat_token_is_a_string_for_the_caller() {
        let actor = Character::default().character_id;
        let mut reg = Registry::new(SandboxConfig::default());
        reg.set_chat(crate::chat::ChatHub::new(crate::chat::ChatConfig::default()));
        let req = Request {
            service: "chatAuthenticationService",
            method: "GetAuthenticationToken",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        };
        let a = dispatch(&mut reg, &req).unwrap();
        let b = dispatch(&mut reg, &req).unwrap();
        let PyValue::Bytes(t) = &a else {
            panic!("a str, got {a:?}")
        };
        assert_eq!(t.len(), 32);
        assert_ne!(a, b, "each connect gets its own token");
    }
}
