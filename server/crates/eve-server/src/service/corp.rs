//! Corporations, offices and wars.
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
    "corpRegistry",
    "officeManager",
    // CORP-O: the item-locking service the office inventory asks.
    "itemLocking",
    "corpStationMgr",
    // Not a name we had. See the arm: its `GetMedalsReceived` is what cost
    // the character info window its tabs.
    "corporationSvc",
    // Not a name we had either: anything about a corp that is not your own,
    // including the corporation info window's first line. See `corpmgr`.
    "corpmgr",
    // The votes tab asks it (`voting.CanViewVotes`); see its arm.
    "voteManager",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // Wars. Bound on `(corpID or allianceID, 1)` and reached on the
        // ordinary docked-login path -- `war_cso.OnSessionChanged` runs
        // whenever `corpid` is in the session change, which it always is.
        //
        // Note the two round trips here look different from the others in
        // a capture: the client binds with **no fused call**
        // (`MachoBindObject(bindParams, None)`), then calls `GetWars` on
        // the reference it gets back. That second call carries no service
        // name at all, only the object id, which is why
        // `dispatch::handle_bound_call` has to exist.
        //
        // The result is iterated with `.itervalues()` and each value's
        // `.warID` read, so a dict is required and an empty one is both
        // valid and true: nobody is at war in this sandbox.
        // The corporation registry. Bound on a **bare scalar** corpID,
        // not a tuple -- unlike `warRegistry` and `allianceRegistry`
        // next door, which do take tuples.
        //
        // `GetAggressionSettings` is a **second hard gate** in the
        // blocking wave, and it was easy to miss: it is the back half of
        // the same `crimewatchSvc.ProcessSessionChange` that calls
        // `crimewatch.GetClientStates`, in a separate top-level
        // `if 'corpid' in change:`. Our session change carries `corpid`,
        // so we summon it ourselves. Unanswered, the session-change
        // tasklet never finishes and the write lock is never released.
        //
        // The reply is an old-style instance of
        // `crimewatch.corp_aggression.settings.AggressionSettings`,
        // whose entire state is two single-underscore attributes. The
        // module's own constants say what to send:
        //
        //     ALWAYS_ENABLED  = AggressionSettings(0, None)
        //     ALWAYS_DISABLED = AggressionSettings(None, 0)
        //
        // and its own default picker, which ships in the client tree,
        // chooses between them:
        //
        //     if not IsNPCCorporation(corpID): return ALWAYS_ENABLED
        //     else:                            return ALWAYS_DISABLED
        //
        // Our corp is an NPC corp, so we mirror that and send
        // `ALWAYS_DISABLED`. `None` would also survive this login --
        // every reader sits behind `not IsNPC(session.corpid)` -- but it
        // makes the client re-fetch on every miss and would break the
        // corp window's Home tab.
        //
        // Useful property of this wave: `NotifySessionChange` wraps each
        // listener in `try/except Exception: log.LogException()`, so a
        // *wrong shape* here is logged and swallowed and the wave carries
        // on. Only silence hangs.
        "corpRegistry" => match crate::group_contacts::corp_registry(reg, req)
            .or_else(|| super::killmail::corp_registry(reg, req))
            .or_else(|| corp_registry(reg, req))
        {
            Some(answer) => answer,
            None => npc_corp_registry(reg, req),
        },
        "corpmgr" => corpmgr(reg, req),
        // **No voting exists here**: no vote cases, no sanctionable actions.
        // `corporationWindow` builds the Votes panels only when this is
        // truthy, so `False` is the true answer and keeps the tab from
        // calling `GetVoteCasesByCorporation` and the rest, which are
        // unanswered. The client caches it (`canViewVotes`) **[V]**
        // (`voting.py:145`). Falsified by a voting lane landing.
        "voteManager" => match req.method {
            "CanViewVotes" => Ok(PyValue::Bool(false)),
            _ => Err(reg.no_method(req)),
        },
        // **A service we did not have, and the character info window needs
        // it.** `medals.GetMedalsReceivedWithFlag:99` unpacks `GetMedalsReceived`
        // as a pair (`UNPACK_SEQUENCE 2`, three independent sites), and a
        // `None` took the info window's tabs and buttons with it. The medal
        // calls are `service/corp_medals.rs`'s (lane `soc-medals`, #166); the
        // pair's second half is a `CRowset` the client calls `.Filter` on.
        "corporationSvc" => match req.method {
            "GetMedalsReceived"
            | "GetAllCorpMedals"
            | "GetMedalDetails"
            | "GetRecipientsOfMedal"
            | "CreateMedal"
            | "GiveMedalToCharacters"
            | "SetMedalStatus" => super::corp_medals::corporation_svc(reg, req),
            // FW-1: `facWarSvc.IsCharacterInFwCooldown` iterates this and
            // `fwWarzoneDashboard._GetTimeServed` reads `[0].startDate`, so
            // `None` (the unanswered default) was a `TypeError` in the
            // enlist window **[V code, I effect]**. Newest first, never empty.
            "GetEmploymentRecord" => {
                let who = int_arg(arg(req, 0, "characterID")).or(req.actor);
                Ok(PyValue::List(employment_record(reg, who)))
            }
            _ => Err(reg.no_method(req)),
        },
        // CORP-O: offices are `service/corp_office.rs`'s.
        "officeManager" => super::corp_office::office_manager(reg, req),
        "itemLocking" => super::corp_office::item_locking(reg, req),
        // The station services standing gate -- **and `None` is already
        // the right answer**, which is worth stating rather than
        // discovering twice.
        //
        // The protocol here is exceptions, not return values:
        //
        //     try:
        //         reg._DoStandingCheckForServices(stationServiceID)
        //         result = None
        //     except Exception as e:
        //         result = e
        //     ...
        //     if result is not None: raise result
        //
        // The return value is discarded entirely; only a raised
        // `UserError` means anything. So `None` *is* "standing check
        // passed, service accessible", and the result is cached for five
        // minutes.
        //
        // The argument is the service id. The one we see is **17**,
        // `appConst.stationServiceFitting` -- this call is the fitting
        // window's gate, and answering it is what lets the window open.
        "corpStationMgr" => match req.method {
            "DoStandingCheckForStationService" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// `corpRegistry`'s arms from before player corporations, which every
/// character still reaches: the login wave's, the info window's, and the
/// panels that open on any corp. The player-corp half is [`corp_registry`],
/// asked first.
fn npc_corp_registry(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // The expel picker iterates this, an empty list for now (S10);
        // the other share calls are `service/corp_shares.rs`'s (S7, #178).
        "GetMemberIDsWithMoreThanAvgShares" => Ok(PyValue::List(vec![])),
        // **The one that was aborting `ActivateView` after the view
        // had already opened.** A dict keyed by corporationID whose
        // values are lists of applications:
        //
        //     def GetMyApplicationsWithStatus(self, status):
        //         applications = reg.GetMyApplications()
        //         if 0 == len(applications):
        //             return applications
        //
        // `len`, `in`, `__setitem__`, key iteration and subscript --
        // all satisfied by `{}`, and none of them by `None`.
        //
        // Why it mattered out of all proportion to a corp menu: the
        // chain is `ActivateView` -> `UpdateOverlays` -> `OpenView`
        // -> `CreateNeocom` -> ... -> `GetMyApplicationsWithStatus`,
        // and `UpdateOverlays()` sits **outside** the `try` that
        // wraps `_OpenPrimaryView`. The enclosing `try` catches only
        // `eveexceptions.UserError`, so a `TypeError` here leaves
        // `ActivateView` through the `finally` and skips everything
        // after it -- including
        //
        //     sm.ScatterEvent('OnClientReady', newInfo.name)
        //
        // which is the event that wakes the rest of the client. So
        // this single `None` silently suppressed `OnClientReady`.
        // Correct the claim in `docs/archive/entering-the-game.md` 2.3 while you
        // are here: the bare `except` protects `LoadView`, not the
        // whole of `ActivateView`.
        //
        // **And it was a live lock as well as a `TypeError`**, which
        // the original note missed. `bco_applications.py(77) GetMyApplications` is
        //
        //     if self.myApplications is None or forceUpdate:
        //         self.myApplications = self.GetCorpRegistry().GetMyApplications()
        //
        // -- the store completes, so `None` was written to the cache
        // that guards it with `is None` and *every* read re-issued the
        // call, not just every neocom refresh. Same mechanism as
        // `GetTitles` below. Note the guard is a compound condition, so
        // this row reads `guard=none` in `findings/client-calls.tsv`:
        // the census's nearest-jump detector cannot see the `is None`
        // half, which is the documented false negative to expect in
        // that column.
        //
        // `{}` is not `None`, so the cache latches on the first call and
        // the loop stops. With player corps it carries the caller's open
        // applications and invitations: [`my_applications`].
        "GetMyApplications" => Ok(my_applications(reg, req.actor)),
        // Corporation titles -- **the second live lock in this file's
        // services, on a service we already answer five methods of.**
        //
        // `bco_titles.py(71) TitlesO.GetTitles`, from the bytecode
        // rather than from the `guard` column of
        // `findings/client-calls.tsv` (which undercounts, and is not
        // trustworthy for a decision like this):
        //
        // ```text
        //   3 LOAD_ATTR     titles
        //   6 LOAD_CONST    None
        //   9 COMPARE_OP    8            # `is`
        //  12 POP_JUMP_IF_FALSE 146
        //  27 CALL_FUNCTION 0             # GetCorpRegistry().GetTitles()
        //  33 STORE_ATTR    titles        # <- completes
        //  51 LOAD_ATTR     itervalues    # <- raises, two ops later
        // ```
        //
        // **Correction to the brief that opened this lane**, because it
        // changes which variant this is: the store at 33 is *not* inside
        // the failing expression. It completes, and `None` is written to
        // `self.titles`; the `AttributeError` comes from the
        // `.itervalues()` at 51, in the untitled-title fixup that
        // follows. So this is the `contractProxy` poisoned-cache
        // mechanism, not the LP "the assignment never runs" one -- the
        // outcome is identical (the cache holds the single value that
        // fails its own `is None` guard, so the next read re-issues) but
        // "the assignment never runs" is not what to look for here.
        // Unlike `contractProxy` there is no `return` in a `finally`, so
        // the exception does escape; every UI caller wraps its load in
        // `except: log.LogException()`, which means a dead panel plus a
        // mailed traceback rather than silence. The *loop*, however, is
        // silent: nothing counts the re-ask but `evecap --calls`.
        //
        // **A dict keyed by titleID, and it must be a real `dict`.**
        // Every consumer in the tree agrees on the container and that is
        // the whole requirement with no rows:
        //
        //  - `bco_titles.GetTitles` -- `.itervalues()`, then
        //    `self.titles[title.titleID] = title` (`STORE_SUBSCR`).
        //  - `bco_titles.OnTitleChanged` -- `key in`, `del`,
        //    `self.titles[key] = title`, and `example.header` off a row.
        //  - `bco_members.GetMyGrantableRoles` -- `.iteritems()`.
        //  - `corp_ui_member_titles` (`.iteritems()`, `.get()`,
        //    `[titleID].titleName`), `corp_dlg_edit_member`,
        //    `corp_ui_member_newroles`, `corp_ui_member_find`,
        //    `corp_ui_member_view_task_mgt` -- all `.itervalues()`.
        //  - `corp_ui_member_listentry` -- `len(titles)`, and
        //    `titlesByID[1 << columnNumber]`, which is where the keys
        //    being **powers of two** shows: a titleID is one bit of
        //    `member.titleMask`, consistent with the fixup's
        //    `int(math.log(titleID, 2) + 1)`.
        //
        // So **not** the `CIndexedRowset` family, even though it
        // subclasses `dict` and `GetSharesByShareholder` next door is a
        // `CRowset`: the fixup does `STORE_ATTR titleName` on a row, and
        // a packed `blue.DBRow` refuses that -- the same reason
        // `map.GetIncursionGlobalReport` is a list of mutable rows. The
        // day this has content the rows must be `KeyVal`-like, carrying
        // `titleID`, `titleName`, `titleMask` and the ten role masks
        // `UpdateTitle` round-trips.
        //
        // **`{}` is the honest answer, not a stub.** Our corp is an NPC
        // corp and has no titles; there is no corp state to read from.
        // `{}` is not `None`, so the guard latches on the first call and
        // the loop stops; `len`, `in`, `.get`, `.iteritems()` and
        // `.itervalues()` are all satisfied, the fixup genexpr iterates
        // nothing, and every title scroll and title picker draws empty.
        // No consumer guards this with `if not titles:`, which is what
        // would have made an empty dict re-ask -- that was checked, and
        // it is the trap that `CRowset`-subclasses-`list` set elsewhere.
        "GetTitles" => Ok(PyValue::Dict(vec![])),
        // The caller's own corp: `ALWAYS_DISABLED` for the NPC corp, as
        // above, and the founder's friendly-fire choice for a player corp
        // ([`aggression_settings`]).
        "GetAggressionSettings" => {
            let corp_id = req
                .actor
                .and_then(|id| reg.character(id))
                .map_or(crate::model::NPC_CORPORATION_ID, |c| c.corporation_id);
            Ok(aggression_settings(reg, corp_id))
        }
        // Fires right after, on its own tasklet, from
        // `corp.OnSessionChanged` -> `CorporationMembersO` ->
        // `__PrimeEveOwners`, again because `corpid` is in the
        // change. Iterated, `.ownerID` read, and each row spliced
        // into `cfg.eveowners.data` as `list(owner) + [None]` --
        // so a row has **four** columns, `(ownerID, ownerName,
        // typeID, gender)`, and the appended `None` becomes the
        // fifth, `ownerNameID`.
        //
        // Empty is safe: `__memberIDs` ends up empty, `GetMember`
        // returns None and `GetMyGrantableRoles` handles that. **[I]**
        // on the four-column claim -- it is inferred from the append,
        // tightly but not directly. Still empty for the NPC corp; a player
        // corp's members are [`eve_owners`].
        "GetEveOwners" => Ok(eve_owners(reg, req.actor)),
        // **The live lock a real client found, and the only one in this
        // round that costs a feature rather than a frame.**
        //
        // `bco_corporations.CorporationsO` caches the answer and the cache
        // is what every other corporation read goes through. Our `None`
        // broke the latch, not just the call:
        //
        // ```text
        //  58  if self.corporationByCorporationID is not None and corpid in ... and not new:
        //  59      return self.corporationByCorporationID[corpid]
        //  60  if corpid == session.corpid:
        //  61      corporation = self.GetCorpRegistry().GetCorporation()
        //  ...
        //  83  self.corporationByCorporationID = IndexRowset(
        //          corporation.header, [corporation.line], 'corporationID')
        // ```
        //
        // `LOAD_ATTR header` at `:83` raises **inside** the statement that
        // would have assigned the cache, so `corporationByCorporationID`
        // stays `None`, the guard at `:58` never short-circuits, and
        // every one of the ~40 delegate call sites re-issues for ever.
        // That is why the live session shows this call **five times** --
        // and each raise lands in its own caller, so the station Offices
        // panel and the HUD info panel each break separately. Verified by
        // disassembly before this arm was written.
        //
        // **A `util.Row`, not a dict, a rowset or a `KeyVal`.** The
        // `:83` branch is written for exactly that pair of attributes, and
        // the branch ahead of it tests `type(corporation) == blue.DBRow`
        // first -- so a `DbRow` would work too and would need a
        // descriptor per column. `Row` is the cheaper shape the client is
        // specified to accept, and both branches end in the same
        // `IndexRowset`. **[I]** that Tranquility sends the `DBRow`.
        //
        // **No arguments**: the client calls this only for its own corp,
        // and routes any other corp id to `corpmgr.GetCorporations`
        // (plural) -- which is a different service we do not serve, and
        // which is where the `agents.py:562` `allAgentsByStationID[...]`
        // subscript gets its row from. That subscript is therefore **not**
        // a constraint on this arm; it reads the *school's* corporation.
        "GetCorporation" => {
            let corp_id = reg
                .character(req.actor.unwrap_or_default())
                .map_or(crate::model::NPC_CORPORATION_ID, |c| c.corporation_id);
            Ok(corporation_row(reg, corp_id))
        }
        // The character info window's corporation half, and **the one
        // call in this group whose container family is forced rather
        // than chosen**: `base_corporation.py:1048` does
        // `STORE_ATTR title1` on what we send. `carbon...sys.row.Row` has
        // `__slots__ = ('header', 'line')` and a `blue.DBRow` is not an
        // attribute bag either, so both raise on the assignment. A
        // `utillib.KeyVal` is an old-style class with no `__setstate__`,
        // so the reader does `instance.__dict__.update(state)` and every
        // key is a plain assignable attribute.
        //
        // Nineteen attributes, and **every one raises if absent** rather
        // than reading as `None`: `corpID` and `allianceID` at
        // `bountyWindow.py:513` and `killReport.py:580`, `title` at
        // `characterInfoWindow.py:121` and `inControlCont.py:63`, and
        // `title1..16` at `base_corporation.py:1047..1092` whenever the
        // caller passes `acceptBlank` falsy.
        //
        // `None` is the right *value* for a title nobody holds: the
        // `acceptBlank=1` consumers use `getattr(..., None)` and skip
        // falsy ones, and the `acceptBlank=0` wrapper substitutes
        // "Untitled N" itself from localisation id 60547. A **missing**
        // attribute is the only failure.
        "GetInfoWindowDataForChar" => {
            let who = opt_id(req.args.first()).or(req.actor);
            let (corp_id, alliance_id) = who
                .and_then(|id| reg.character(id))
                .map_or((crate::model::NPC_CORPORATION_ID, None), |c| {
                    (c.corporation_id, c.alliance_id)
                });
            let held = who.and_then(|id| reg.corporations.direct(id));
            let mut attrs = vec![
                ("corpID", PyValue::Int(corp_id)),
                ("allianceID", opt_int(alliance_id)),
                // **[V2]** only; one slot, and free.
                ("factionID", PyValue::None),
                (
                    "title",
                    bstr(who.map_or("", |id| reg.corporations.title(id))),
                ),
            ];
            for n in 1..=16 {
                // `title1` .. `title16`. Leaked as a `&'static str` because
                // `keyval` takes one and these are built per call; sixteen
                // 7-byte allocations once per info window is not a cost
                // worth a lifetime parameter for.
                //
                // CORP-T: the name of each title the pilot holds (empty for
                // an unnamed one, which the client shows as "Untitled n"),
                // `None` for the rest.
                let id = crate::corp_titles::title_id(n - 1);
                let name = held
                    .filter(|m| m.title_mask & id != 0)
                    .map_or(PyValue::None, |m| {
                        PyValue::Unicode(reg.corporations.title_book.get(m.corporation_id, id).name)
                    });
                attrs.push((
                    Box::leak(format!("title{n}").into_boxed_str()) as &'static str,
                    name,
                ));
            }
            Ok(keyval(attrs))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `None` as an id, or the id.
fn opt_int(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

/// One corporation as the `util.Row` `bco_corporations.LoadCorporation`
/// expects: a header of column names and a parallel line of values.
///
/// **The header is the specification.** `Row.__getattr__` is
/// `self.line[self.header.index(name)]`, so a name that is not in the header
/// is a `ValueError` at the moment some unrelated panel reads it -- which is
/// why the list is long and why every name in it is cited. The `[V]` names
/// are read by name somewhere in the client; the rest are **[V2]** and cost
/// one slot each, which is the cheapest insurance in this file.
///
/// Three values matter beyond being present:
///
/// 1. **`corporationID` must be the int.** `IndexRowset` keys on it and
///    `rowset.py:264` raises `ValueError` for a column it cannot find.
/// 2. **`ceoID` must not be the player.** Eighteen sites compare it with
///    `session.charid` -- `base_corporation.UserIsCEO`, `voting.py:161`,
///    the member list -- and an NPC corporation whose CEO is the player hands
///    them corporation management they should not have. The value is an NPC
///    id (`idCheckers.IsSystemOrNPC` is `id < 90000000`); **[I]** on this
///    specific one, `[V]` on the requirement.
/// 3. **`tickerName` must match `config.GetMultiCorpTickerNamesEx`**, which
///    already answers `"CAPS"` -- two different strings for one corporation
///    is two different tickers on screen depending on which panel drew it.
///
/// `division1..7` and `walletDivision2..7` are **`None` on purpose**:
/// `bco_corporations.py:168-181` reads each as
/// `corp.divisionN or localization.GetByLabel(...)`, a
/// `JUMP_IF_TRUE_OR_POP`, so `None` makes the client supply its own default
/// name. `walletDivision1` is never read (division 1 is the Master Wallet)
/// and is here for the shape.
///
/// `stationID` is `None` because this server does not model corporation
/// headquarters. `stationController.IsMyHQ` compares it and simply never
/// matches, which is honest; inventing our own station would claim that an
/// NPC corporation is headquartered where the player happens to be docked,
/// and `mapObjects.db` says station 60012526 belongs to corporation 1000126.
///
/// **A player corporation fills the same header with its own values**
/// ([`CorpView`]): its name and ticker, the founder as `ceoID` and
/// `creatorID` -- which is what makes `UserIsCEO` true for exactly them --
/// the logo the form picked, and its real member count. `memberLimit` is
/// `[A]` 9999: Tranquility derives it from the CEO's Corporation Management
/// skill, and the only reader here is the edit form's label.
fn corporation_row(reg: &Registry, corp_id: i64) -> PyValue {
    let v = corp_view(reg, corp_id);
    let player = is_player_corp(reg, corp_id);
    let opt = |o: Option<i64>| o.map_or(PyValue::None, PyValue::Int);
    let mut header: Vec<&str> = vec![
        "corporationID",
        "corporationName",
        "tickerName",
        "description",
        "url",
        "taxRate",
        "loyaltyPointTaxRate",
        "isRecruiting",
        "memberCount",
        "memberLimit",
        "shares",
        "ceoID",
        "creatorID",
        "stationID",
        "allianceID",
        "allowWar",
        "shape1",
        "shape2",
        "shape3",
        "color1",
        "color2",
        "color3",
    ];
    let mut line: Vec<PyValue> = vec![
        PyValue::Int(corp_id),
        bstr(&v.name),
        // The same ticker `multi_corp_tickers` sends.
        bstr(&v.ticker),
        bstr(&v.description),
        bstr(&v.url),
        PyValue::Real(v.tax_rate),
        PyValue::Real(v.lp_tax_rate),
        PyValue::Int(i64::from(v.recruiting)),
        PyValue::Int(v.member_count),
        PyValue::Int(if player { 9999 } else { 0 }),
        PyValue::Int(0),
        PyValue::Int(v.ceo_id),
        PyValue::Int(v.creator_id),
        PyValue::None,
        // The corp's alliance (SOC-C1), `None` for the NPC corp.
        opt(crate::alliance::of_corp(reg.deltas(), corp_id)),
        // `allowWar` (C8): the war permit.
        crate::war::permit_value(reg, corp_id),
        opt(v.shapes[0]),
        opt(v.shapes[1]),
        opt(v.shapes[2]),
        opt(v.colors[0]),
        opt(v.colors[1]),
        opt(v.colors[2]),
    ];
    // `division1..7`, then `walletDivision1..7`: the client's own `or`
    // supplies every default name.
    let names = super::corp_wallet::name_values(reg, corp_id);
    for n in 1..=7 {
        header.push(Box::leak(format!("division{n}").into_boxed_str()));
        line.push(names[n - 1].clone());
        header.push(Box::leak(format!("walletDivision{n}").into_boxed_str()));
        line.push(names[6 + n].clone());
    }
    // **[V2]** only: not read by any client path the recon lane could reach,
    // and a header slot is cheaper than the call that finds out otherwise.
    // Two of them carry a player corp's own value.
    for extra in [
        "factionID",
        "warFactionID",
        "membership",
        "deleted",
        "friendlyFire",
        "allowedMemberRaceIDs",
        "corporationType",
        "minimumJoinStanding",
        "sendCharTerminationMessage",
        "createDate",
        "aggressionEnableAfter",
        "aggressionDisableAfter",
        "applicationsEnabled",
        "typeface",
    ] {
        header.push(extra);
        line.push(match extra {
            "friendlyFire" if player => PyValue::Int(i64::from(v.friendly_fire)),
            "createDate" => opt(v.create_date),
            // FW-1: the militia a player corp serves.
            "warFactionID" if player => opt_int(super::facwar_mgr::owner_faction(reg, corp_id)),
            "deleted" if v.deleted => PyValue::Int(1),
            _ => PyValue::None,
        });
    }
    match eve_rowset::Row::new(&header, line) {
        Ok(row) => row.to_pyvalue(),
        // `Row::new` checks the two sequences line up, exactly as the
        // client's `__init__` does. A mismatch here is a bug in the table
        // above, and `None` is what we sent before this arm existed -- so it
        // degrades to the old behaviour rather than to a panic in a
        // connection thread.
        Err(_) => PyValue::None,
    }
}

/// The ticker every corporation here has, and the one place it is written.
///
/// `multi_corp_tickers` answers this for `config.GetMultiCorpTickerNamesEx`;
/// `corporation_row` sends it as `tickerName`. Two strings would be two
/// tickers on screen for one corporation.
pub(crate) const CORP_TICKER: &str = "CAPS";

/// The CEO of every corporation in this sandbox.
///
/// **Its only hard requirement is that it is not the player**, because
/// eighteen client sites compare `ceoID` with `session.charid` to decide
/// whether to offer corporation management. Below 90,000,000, so
/// `idCheckers.IsSystemOrNPC` is true of it and the client routes it through
/// its own local fetcher rather than asking us for a character sheet.
/// **[I]** on the value; `[V]` on the constraint.
pub(crate) const NPC_CEO_ID: i64 = 3_004_093;

// ---------------------------------------------------------------------------
// Player corporations
// ---------------------------------------------------------------------------
//
// Friends form a corp together: one of them creates it (`AddCorporation`),
// invites the others (`InsertInvitation`) or accepts their applications
// (`InsertApplication` + `UpdateApplicationOffer`), and each new member's
// session moves to the new `corpid`. Everything else in the server already
// reads `Character::corporation_id` -- the slim's `corpID`, the guest list,
// `charMgr.GetPublicInfo`, the info window -- so moving that one field, and
// telling the session, is what makes a corp visible to everybody else.
// `docs/lanes/corp.md` has the client reading behind every arm below.

/// `inventorycommon/const.py`: `minPlayerCorporation = 98000000`,
/// `maxPlayerCorporation = 98999999`, the range
/// `idCheckers.IsPlayerCorporation` accepts without asking `cfg.eveowners`
/// **[V]**. The first corp gets `MIN + 1`; nothing reads the bottom of the
/// range specially, it is simply an id nobody could mistake for a sentinel.
pub(crate) const MIN_PLAYER_CORPORATION_ID: i64 = 98_000_000;
pub(crate) const MAX_PLAYER_CORPORATION_ID: i64 = 98_999_999;

/// `appConst.corporationStartupCost = 1599800` **[V]**. The client's own
/// gate (`bco_members.MemberCanCreateCorporation`) compares the wallet with
/// this before it opens the form, so charging anything else would be a fee
/// the player was never shown.
pub(crate) const CORPORATION_STARTUP_COST: f64 = 1_599_800.0;

/// `appConst.refCorporationRegistrationFee = 39` **[V]**: the journal row
/// the fee is recorded under.
const REF_CORPORATION_REGISTRATION_FEE: i64 = 39;

/// `appConst.corpNameMaxLenTQ = 50` -- the create form's `maxLength`
/// (`corp_dlg_editcorpdetails.CorpDetails.ApplyAttributes`) **[V]**.
const CORP_NAME_MAX_LEN: usize = 50;
/// The ticker edit's `maxLength=5` in the same form **[V]**.
const CORP_TICKER_MAX_LEN: usize = 5;
/// The description editor's `maxLength=4000` in the same form **[V]**
/// (`corp_dlg_editcorpdetails`, `descEdit`).
const CORP_DESCRIPTION_MAX_LEN: usize = 4000;
/// The URL edit's `maxLength=2048` in the same form **[V]**.
const CORP_URL_MAX_LEN: usize = 2048;
/// The member title entry's `maxLength=100` in the edit-member dialog **[V]**
/// (`corp_dlg_edit_member`, `general_title`).
const MEMBER_TITLE_MAX_LEN: usize = 100;

/// `s` cut to `max` characters (the client's own `maxLength`, which a
/// well-behaved client never exceeds: SEC-FIX 4).
fn capped(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_owned(),
        None => s,
    }
}

/// `appConst.corpRoleDirector` and `corpRolePersonnelManager` **[V]**.
pub(crate) const CORP_ROLE_DIRECTOR: i64 = 1;
pub(crate) const CORP_ROLE_PERSONNEL_MANAGER: i64 = 128;

/// What a founder holds.
///
/// **[A]** on the exact mask: Tranquility gives a CEO every role. Two are
/// enough for everything this server answers, and both are tested by the
/// client as *bits*, not implied by director: `bco_applications.GetApplications`
/// returns `{}` unless `corprole & corpRolePersonnelManager` is set, and
/// `characterCheckers.OfferSendCorpInvite` tests `IsPersonnelManager()`.
/// Director buys the member-tracking detail columns and the expel menu
/// (`OfferExpelCorpMember` -> `IsCorpDirector()`). Every other role would
/// unlock a window whose calls we do not answer (hangars, wallets).
pub(crate) const CEO_ROLES: i64 = CORP_ROLE_DIRECTOR
    | CORP_ROLE_PERSONNEL_MANAGER
    | crate::corp_wallet::CEO_WALLET_ROLES
    | crate::corp_office::CEO_OFFICE_ROLES;

/// `appConst.crpApplication*` **[V]**.
const APP_APPLIED_BY_CHARACTER: i64 = 0;
const APP_RENEGOTIATED_BY_CHARACTER: i64 = 1;
const APP_ACCEPTED_BY_CHARACTER: i64 = 2;
const APP_REJECTED_BY_CHARACTER: i64 = 3;
const APP_REJECTED_BY_CORPORATION: i64 = 4;
const APP_ACCEPTED_BY_CORPORATION: i64 = 6;
const APP_WITHDRAWN_BY_CHARACTER: i64 = 7;
const APP_INVITED_BY_CORPORATION: i64 = 8;
/// `crpApplicationEndStatuses` **[V]**: an application in one of these is
/// history (`GetMyOldApplications`), not something either side can act on.
const APP_END_STATUSES: [i64; 4] = [
    APP_REJECTED_BY_CORPORATION,
    APP_WITHDRAWN_BY_CHARACTER,
    APP_ACCEPTED_BY_CHARACTER,
    APP_REJECTED_BY_CHARACTER,
];
/// `appConst.crpApplicationMaxSize = 1000` **[V]**; `CheckApplication`
/// refuses longer text client-side, so a longer one is a client we do not
/// understand.
const APP_TEXT_MAX: usize = 1000;

/// One player corporation.
#[derive(Debug, Clone, PartialEq)]
pub struct Corporation {
    pub corporation_id: i64,
    pub name: String,
    pub ticker: String,
    pub description: String,
    pub url: String,
    pub tax_rate: f64,
    pub lp_tax_rate: f64,
    pub ceo_id: i64,
    pub creator_id: i64,
    /// Blue FILETIME, like every date the client formats.
    pub create_date: i64,
    pub recruiting: bool,
    pub aggression: Aggression,
    /// `shape1..3`, `color1..3`: the logo the create form picked. Stored and
    /// echoed in the corporation row; nothing here draws them.
    pub shapes: [Option<i64>; 3],
    pub colors: [Option<i64>; 3],
    /// Closed (SOC-C3): its last CEO resigned with nobody to take over. A
    /// closed corp keeps its row, so its id, name and ticker still answer
    /// owner lookups (killmails, employment records, alliance history), but
    /// it has no members, takes no applications and is not listed.
    /// `deleted` in the corporation row and the public info **[V]** (the
    /// info window reads it). **[A]** that its name and ticker stay taken.
    pub deleted: bool,
}

/// One character's membership of a player corporation. A character with no
/// entry is in the corporation `SandboxConfig` gave them -- the NPC corp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Member {
    pub corporation_id: i64,
    /// `rolesAtAll`: the member row's `roles`.
    pub roles: i64,
    /// Blue FILETIME: the member-tracking "Joined" column.
    pub start_date: i64,
    /// The other seven role masks of a member row, as the roles window
    /// edits them (SOC-C3): `grantableRoles`, `rolesAtHQ`,
    /// `grantableRolesAtHQ`, `rolesAtBase`, `grantableRolesAtBase`,
    /// `rolesAtOther`, `grantableRolesAtOther` -- [`crate::corp_roles::MASK_NAMES`]
    /// `[1..]`. The location masks are stored and sent back as set; with no
    /// HQ, base or office here the client's derived `corprole` is
    /// `rolesAtAll | rolesAtOther` **[V]** (`basesession`).
    pub grantable_roles: i64,
    pub roles_at_hq: i64,
    pub grantable_roles_at_hq: i64,
    pub roles_at_base: i64,
    pub grantable_roles_at_base: i64,
    pub roles_at_other: i64,
    pub grantable_roles_at_other: i64,
    /// `titleMask`: one bit per title. No corp here has titles yet
    /// (`GetTitles` is `{}`), so this only round-trips.
    pub title_mask: i64,
    /// `blockRoles`: "Remove all roles" sets it for yourself **[V]**; a
    /// member who blocks roles is offered none and cannot be made CEO
    /// ([`crate::corp_roles`]).
    pub block_roles: bool,
}

impl Member {
    /// A fresh membership: these roles, nothing else.
    fn new(corporation_id: i64, roles: i64, start_date: i64) -> Member {
        Member {
            corporation_id,
            roles,
            start_date,
            grantable_roles: 0,
            roles_at_hq: 0,
            grantable_roles_at_hq: 0,
            roles_at_base: 0,
            grantable_roles_at_base: 0,
            roles_at_other: 0,
            grantable_roles_at_other: 0,
            title_mask: 0,
            block_roles: false,
        }
    }

    /// The eight masks in [`crate::corp_roles::MASK_NAMES`] order.
    pub fn masks(&self) -> crate::corp_roles::Masks {
        [
            self.roles,
            self.grantable_roles,
            self.roles_at_hq,
            self.grantable_roles_at_hq,
            self.roles_at_base,
            self.grantable_roles_at_base,
            self.roles_at_other,
            self.grantable_roles_at_other,
        ]
    }

    fn set_masks(&mut self, m: &crate::corp_roles::Masks) {
        self.roles = m[0];
        self.grantable_roles = m[1];
        self.roles_at_hq = m[2];
        self.grantable_roles_at_hq = m[3];
        self.roles_at_base = m[4];
        self.grantable_roles_at_base = m[5];
        self.roles_at_other = m[6];
        self.grantable_roles_at_other = m[7];
    }

    /// The four masks a session carries: `rolesAtAll`, `rolesAtHQ`,
    /// `rolesAtBase`, `rolesAtOther`.
    pub fn session_masks(&self) -> [i64; 4] {
        [
            self.roles,
            self.roles_at_hq,
            self.roles_at_base,
            self.roles_at_other,
        ]
    }
}

/// An application to join, or an invitation -- one row, two directions,
/// told apart by `status` exactly as the client does.
#[derive(Debug, Clone, PartialEq)]
pub struct Application {
    pub application_id: i64,
    pub character_id: i64,
    pub corporation_id: i64,
    pub status: i64,
    pub date: i64,
    pub text: String,
}

/// Every player corporation, its members, and the applications between
/// characters and corps. Persisted as opaque records, like `Contacts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Corporations {
    corps: std::collections::BTreeMap<i64, Corporation>,
    members: std::collections::BTreeMap<i64, Member>,
    pub(super) applications: std::collections::BTreeMap<i64, Application>,
    /// Members' free-text titles (`title` on a member row), set from the
    /// edit-member dialog (SOC-C3). Absent is the empty title.
    titles: std::collections::BTreeMap<i64, String>,
    /// Every corp's named titles and their role masks (CORP-T).
    title_book: crate::corp_titles::TitleBook,
    /// The last id handed out; `0` means none yet. Survives a world whose
    /// every corp is gone, so an id is never reused.
    last_corp_id: i64,
    last_application_id: i64,
}

/// Why a corp action was refused, as the client's own message key.
type Refusal = ServiceError;

fn refuse(key: &str) -> Refusal {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

fn refuse_with(key: &str, args: Vec<(&str, PyValue)>) -> Refusal {
    ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
}

impl Corporations {
    /// The record tags this type reads and writes in the world file.
    pub(crate) const RECORDS: &'static [&'static str] = &[
        "corp",
        "corpmember",
        "corpapp",
        "corpnext",
        crate::corp_titles::TitleBook::RECORD,
    ];

    pub fn get(&self, corporation_id: i64) -> Option<&Corporation> {
        self.corps.get(&corporation_id)
    }

    /// One corp for editing (CORP-W: the details form).
    pub(super) fn get_mut(&mut self, corporation_id: i64) -> Option<&mut Corporation> {
        self.corps.get_mut(&corporation_id)
    }

    /// Tests only (CORP-W): a membership to edit, and a member without the
    /// invitation dance.
    #[cfg(test)]
    pub(crate) fn member_mut(&mut self, character_id: i64) -> Option<&mut Member> {
        self.members.get_mut(&character_id)
    }

    #[cfg(test)]
    pub(crate) fn enrol_for_test(&mut self, character_id: i64, corporation_id: i64, roles: i64) {
        self.enrol(character_id, corporation_id, roles, 134_116_992_000_000_000);
    }

    /// Every player corporation, in id order. The people search
    /// (`service/social.rs`) finds corps by name through this.
    pub fn all(&self) -> impl Iterator<Item = &Corporation> + '_ {
        self.corps.values().filter(|c| !c.deleted)
    }

    /// A member's free-text title, empty when none was set.
    pub fn title(&self, character_id: i64) -> &str {
        self.titles.get(&character_id).map_or("", String::as_str)
    }

    /// The player corp this character is in, if any, with the roles they
    /// **can use**: their own masks with the roles of every title they hold
    /// on top (CORP-T). Every permission check reads this one; anything that
    /// edits or shows the member row reads [`Self::direct`].
    pub fn membership(&self, character_id: i64) -> Option<Member> {
        let mut m = self.direct(character_id)?;
        if m.title_mask != 0 {
            let held = self.title_book.held(m.corporation_id, m.title_mask);
            m.set_masks(&crate::corp_titles::merge(&m.masks(), &held));
        }
        Some(m)
    }

    /// The membership as stored: the member row's own masks, no titles.
    pub fn direct(&self, character_id: i64) -> Option<Member> {
        self.members.get(&character_id).copied()
    }

    /// The four session role keys a member's roles come to, titles
    /// included, for a membership as it stands or stood.
    fn session_of(&self, m: &Member) -> [i64; 4] {
        let mut eff = *m;
        if m.title_mask != 0 {
            let held = self.title_book.held(m.corporation_id, m.title_mask);
            eff.set_masks(&crate::corp_titles::merge(&m.masks(), &held));
        }
        eff.session_masks()
    }

    /// Everyone in one corp, in id order.
    pub fn members_of(&self, corporation_id: i64) -> Vec<i64> {
        self.members
            .iter()
            .filter(|(_, m)| m.corporation_id == corporation_id)
            .map(|(id, _)| *id)
            .collect()
    }

    fn name_taken(&self, name: &str) -> bool {
        self.corps
            .values()
            .any(|c| c.name.eq_ignore_ascii_case(name))
    }

    fn ticker_taken(&self, ticker: &str) -> bool {
        // The NPC corp's ticker too: `config.GetMultiCorpTickerNamesEx` is
        // keyed by corp id, but two corps showing one ticker is the confusion
        // tickers exist to prevent.
        ticker.eq_ignore_ascii_case(CORP_TICKER)
            || self
                .corps
                .values()
                .any(|c| c.ticker.eq_ignore_ascii_case(ticker))
    }

    fn next_corp_id(&mut self) -> i64 {
        self.last_corp_id = self.last_corp_id.max(MIN_PLAYER_CORPORATION_ID) + 1;
        self.last_corp_id
    }

    fn next_application_id(&mut self) -> i64 {
        self.last_application_id += 1;
        self.last_application_id
    }

    /// Put a character in a corp, with these roles. Replaces any previous
    /// membership.
    fn enrol(&mut self, character_id: i64, corporation_id: i64, roles: i64, now: i64) {
        self.members
            .insert(character_id, Member::new(corporation_id, roles, now));
        self.titles.remove(&character_id);
    }

    /// Leave every player corp: the membership and its title.
    fn drop_member(&mut self, character_id: i64) {
        self.members.remove(&character_id);
        self.titles.remove(&character_id);
    }

    /// The open (non-final) application between this pair, if any.
    fn open_between(&self, character_id: i64, corporation_id: i64) -> Option<&Application> {
        self.applications.values().find(|a| {
            a.character_id == character_id
                && a.corporation_id == corporation_id
                && !APP_END_STATUSES.contains(&a.status)
        })
    }

    /// Append this type's records to a world file being written.
    ///
    /// ```text
    /// corp       <id> <ceo> <creator> <created> <tax> <lptax> <recruit> <ff>
    ///            <shape1..3> <color1..3> <name> <ticker> <description> <url>
    ///            [<deleted>]
    /// corpmember <characterID> <corpID> <roles> <startDate>
    ///            [<grantable> <atHQ> <grantableAtHQ> <atBase> <grantableAtBase>
    ///             <atOther> <grantableAtOther> <titleMask> <blockRoles> <title>]
    /// corpapp    <id> <characterID> <corpID> <status> <date> <text>
    /// corpnext   <lastCorpID> <lastApplicationID>
    /// corptitle  <corpID> <titleID> <name> <8 masks>      (CORP-T)
    /// ```
    ///
    /// `-1` for an absent shape or colour. Nothing at all for a world with no
    /// player corp, so an older file reads back identical. The bracketed
    /// tails are SOC-C3's: always written, optional on read, so a file from
    /// before them loads with every extra mask 0 and no corp closed.
    pub(crate) fn render(&self, out: &mut String) {
        use crate::store::esc;
        use std::fmt::Write as _;
        if self.corps.is_empty()
            && self.members.is_empty()
            && self.applications.is_empty()
            && self.last_corp_id == 0
            && self.last_application_id == 0
        {
            return;
        }
        let o = |v: Option<i64>| v.unwrap_or(-1);
        for c in self.corps.values() {
            let _ = writeln!(
                out,
                "corp\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                c.corporation_id,
                c.ceo_id,
                c.creator_id,
                c.create_date,
                c.tax_rate,
                c.lp_tax_rate,
                u8::from(c.recruiting),
                u8::from(c.aggression == Aggression::founded(true)),
                o(c.shapes[0]),
                o(c.shapes[1]),
                o(c.shapes[2]),
                o(c.colors[0]),
                o(c.colors[1]),
                o(c.colors[2]),
                esc(&c.name),
                esc(&c.ticker),
                esc(&c.description),
                esc(&c.url),
                u8::from(c.deleted),
                o(c.aggression.enable_after),
                o(c.aggression.disable_after),
            );
        }
        self.title_book.render(out);
        for (id, m) in &self.members {
            let _ = writeln!(
                out,
                "corpmember\t{id}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                m.corporation_id,
                m.roles,
                m.start_date,
                m.grantable_roles,
                m.roles_at_hq,
                m.grantable_roles_at_hq,
                m.roles_at_base,
                m.grantable_roles_at_base,
                m.roles_at_other,
                m.grantable_roles_at_other,
                m.title_mask,
                u8::from(m.block_roles),
                esc(self.title(*id)),
            );
        }
        for a in self.applications.values() {
            let _ = writeln!(
                out,
                "corpapp\t{}\t{}\t{}\t{}\t{}\t{}",
                a.application_id,
                a.character_id,
                a.corporation_id,
                a.status,
                a.date,
                esc(&a.text)
            );
        }
        let _ = writeln!(
            out,
            "corpnext\t{}\t{}",
            self.last_corp_id, self.last_application_id
        );
    }

    /// Read one record whose tag is in [`Corporations::RECORDS`]. Strict,
    /// like the contacts: a line we cannot read is a corp or a membership the
    /// players would silently lose.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        use crate::store::unesc;
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        let real = |i: usize| -> Result<f64, String> {
            f.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|r| r.is_finite())
                .ok_or_else(|| format!("field {i} is not a number"))
        };
        let opt = |i: usize| -> Result<Option<i64>, String> {
            let v = num(i)?;
            Ok((v != -1).then_some(v))
        };
        let text = |i: usize| -> Result<String, String> {
            f.get(i)
                .map(|v| unesc(v))
                .ok_or_else(|| format!("field {i} is missing"))
        };
        match f.first().copied() {
            Some("corp") => {
                let c = Corporation {
                    corporation_id: num(1)?,
                    ceo_id: num(2)?,
                    creator_id: num(3)?,
                    create_date: num(4)?,
                    tax_rate: real(5)?,
                    lp_tax_rate: real(6)?,
                    recruiting: num(7)? != 0,
                    // Columns 20-21 are the timed settings; an older row has
                    // only the founding flag.
                    aggression: if f.len() > 21 {
                        Aggression {
                            enable_after: opt(20)?,
                            disable_after: opt(21)?,
                        }
                    } else {
                        Aggression::founded(num(8)? != 0)
                    },
                    shapes: [opt(9)?, opt(10)?, opt(11)?],
                    colors: [opt(12)?, opt(13)?, opt(14)?],
                    name: text(15)?,
                    ticker: text(16)?,
                    description: text(17)?,
                    url: text(18)?,
                    deleted: f.len() > 19 && num(19)? != 0,
                };
                if !(MIN_PLAYER_CORPORATION_ID..=MAX_PLAYER_CORPORATION_ID)
                    .contains(&c.corporation_id)
                {
                    return Err(format!(
                        "{} is not a player corporation id",
                        c.corporation_id
                    ));
                }
                self.last_corp_id = self.last_corp_id.max(c.corporation_id);
                self.corps.insert(c.corporation_id, c);
            }
            Some("corpmember") => {
                let id = num(1)?;
                let mut m = Member::new(num(2)?, num(3)?, num(4)?);
                if f.len() > 5 {
                    m.grantable_roles = num(5)?;
                    m.roles_at_hq = num(6)?;
                    m.grantable_roles_at_hq = num(7)?;
                    m.roles_at_base = num(8)?;
                    m.grantable_roles_at_base = num(9)?;
                    m.roles_at_other = num(10)?;
                    m.grantable_roles_at_other = num(11)?;
                    m.title_mask = num(12)?;
                    m.block_roles = num(13)? != 0;
                    let title = text(14)?;
                    if !title.is_empty() {
                        self.titles.insert(id, title);
                    }
                }
                self.members.insert(id, m);
            }
            Some("corpapp") => {
                let a = Application {
                    application_id: num(1)?,
                    character_id: num(2)?,
                    corporation_id: num(3)?,
                    status: num(4)?,
                    date: num(5)?,
                    text: text(6)?,
                };
                self.last_application_id = self.last_application_id.max(a.application_id);
                self.applications.insert(a.application_id, a);
            }
            Some("corpnext") => {
                self.last_corp_id = self.last_corp_id.max(num(1)?);
                self.last_application_id = self.last_application_id.max(num(2)?);
            }
            Some(crate::corp_titles::TitleBook::RECORD) => self.title_book.parse_record(f)?,
            other => return Err(format!("{other:?} is not a corporation record")),
        }
        Ok(())
    }
}

/// Whether this id is a player corporation **that exists here** and is
/// not closed.
pub(crate) fn is_player_corp(reg: &Registry, corporation_id: i64) -> bool {
    reg.corporations
        .get(corporation_id)
        .is_some_and(|c| !c.deleted)
}

/// A player corp's name, for `config.GetMultiOwnersEx`. `None` for anything
/// else, including the NPC corp, which keeps its own branch.
pub(crate) fn player_corp_name(reg: &Registry, corporation_id: i64) -> Option<String> {
    reg.corporations.get(corporation_id).map(|c| c.name.clone())
}

/// A player corp's ticker, for `config.GetMultiCorpTickerNamesEx`.
pub(crate) fn player_corp_ticker(reg: &Registry, corporation_id: i64) -> Option<String> {
    reg.corporations
        .get(corporation_id)
        .map(|c| c.ticker.clone())
}

/// The four session role keys for this character's login change:
/// `rolesAtAll`, `rolesAtHQ`, `rolesAtBase`, `rolesAtOther`, their masks in
/// a player corp (SOC-C3: the roles window sets all four), zeros in the NPC
/// corp.
///
/// Non-zero puts the derived `corprole` into the login change, and
/// `invCache.ProcessSessionChange` then makes a blocking
/// `officeManager.GetCorpOfficeAtLocation()` (`docs/lanes/session-and-binding.md`
/// §3). That is safe now **because** the remote half it reaches,
/// `officeManager.GetMyCorporationsOffices`, is answered (`[]`, above):
/// `GetCorpOfficeAtLocation` is client code over that list **[V]**
/// (`officeManager.py`), and an empty one returns `None`.
pub(crate) fn session_role_masks(reg: &Registry, character_id: i64) -> [i64; 4] {
    reg.corporations
        .membership(character_id)
        .map_or([0; 4], |m| m.session_masks())
}

/// After a world file is loaded: make every stored membership true of the
/// character, which is the field everything else reads.
///
/// A member whose corp is gone (a hand-edited file) is put back in the NPC
/// corp rather than left pointing at nothing.
pub(crate) fn sync_characters(reg: &mut Registry) {
    let pairs: Vec<(i64, i64)> = reg
        .corporations
        .members
        .iter()
        .map(|(id, m)| (*id, m.corporation_id))
        .collect();
    for (id, corp) in pairs {
        let corp = if is_player_corp(reg, corp) {
            corp
        } else {
            reg.corporations.drop_member(id);
            crate::model::NPC_CORPORATION_ID
        };
        if let Some(c) = reg.character_mut(id) {
            c.corporation_id = corp;
        }
    }
    // CEOs founded before CORP-W lack the wallet roles (CORP-T).
    heal_all_ceos(reg);
    // Then every character's alliance, from the corp it is now in (SOC-C1).
    crate::alliance::sync_characters(reg);
}

/// The session change that moves a character to another corp.
///
/// `corpid` and the four `rolesAt*` -- **never `corprole`**, which the
/// client derives from them (`basesession.RecalculateDependantAttributesWithChanges`,
/// `rolesAtAll | rolesAtOther` when no HQ/base matches) **[V]**. The old
/// value is `None` because `ComputeNotifySets` rewrites it, as for every
/// other change this server sends. A pair whose new value equals the
/// client's is dropped by the client, so sending all four is harmless.
fn corp_change(corporation_id: i64, roles: i64) -> Vec<(PyValue, PyValue)> {
    let pair = |v: i64| PyValue::Tuple(vec![PyValue::None, PyValue::Int(v)]);
    let mut change = crate::pyutil::pypairs(vec![("corpid", pair(corporation_id))]);
    change.extend(roles_change([roles, 0, 0, 0]));
    change
}

/// The four role keys of a session change, in [`Member::session_masks`]
/// order. A member's roles changing in place (SOC-C3) sends only these.
fn roles_change(masks: [i64; 4]) -> Vec<(PyValue, PyValue)> {
    let pair = |v: i64| PyValue::Tuple(vec![PyValue::None, PyValue::Int(v)]);
    crate::pyutil::pypairs(vec![
        ("rolesAtAll", pair(masks[0])),
        ("rolesAtHQ", pair(masks[1])),
        ("rolesAtBase", pair(masks[2])),
        ("rolesAtOther", pair(masks[3])),
    ])
}

/// The delta kind of a pilot's corporation history: `char start_filetime ->
/// corp`. Written by [`move_character`] only. FW-1.
pub const HIST_KIND: &str = "corphist";

/// Newest rows kept per pilot.
const HIST_KEEP: usize = 64;

/// `corporationSvc.GetEmploymentRecord` rows, newest first: `(startDate,
/// corporationID)`. The oldest row is the corporation the pilot was created
/// in, written with the first move; a pilot who never moved has one
/// synthetic row for the current corporation.
pub(crate) fn employment_rows(reg: &Registry, who: i64) -> Vec<(i64, i64)> {
    let mut rows: Vec<(i64, i64)> = reg
        .deltas()
        .of(HIST_KIND)
        .iter()
        .filter(|(k, _)| k.first().is_some_and(|c| c.parse() == Ok(who)))
        .filter_map(|(k, v)| Some((k.get(1)?.parse().ok()?, v.first()?.parse().ok()?)))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    if rows.is_empty()
        && let Some(c) = reg.character(who)
    {
        rows.push((c.create_date_time, c.corporation_id));
    }
    rows
}

fn employment_record(reg: &Registry, who: Option<i64>) -> Vec<PyValue> {
    let Some(who) = who else {
        return vec![];
    };
    employment_rows(reg, who)
        .into_iter()
        .map(|(start, corp)| {
            keyval(vec![
                ("corporationID", PyValue::Int(corp)),
                ("startDate", PyValue::Int(start)),
                // [A] a deleted player corp; falsifier: the history list
                // greying out a live corporation.
                (
                    "deleted",
                    PyValue::Bool(reg.corporations.get(corp).is_some_and(|c| c.deleted)),
                ),
            ])
        })
        .collect()
}

/// Record a corporation change: the first move also writes the row for the
/// corporation the pilot was created in.
fn record_history(reg: &mut Registry, who: i64, old: i64, new: i64, now: i64) {
    let Some(created) = reg.character(who).map(|c| c.create_date_time) else {
        return;
    };
    if reg
        .deltas()
        .of(HIST_KIND)
        .keys()
        .all(|k| k.first().map(String::as_str) != Some(who.to_string().as_str()))
    {
        reg.deltas_mut().put(
            HIST_KIND,
            vec![who.to_string(), created.to_string()],
            vec![old.to_string()],
        );
    }
    reg.deltas_mut().put(
        HIST_KIND,
        vec![who.to_string(), now.to_string()],
        vec![new.to_string()],
    );
    let rows = employment_rows(reg, who);
    for (start, _) in rows.into_iter().skip(HIST_KEEP) {
        reg.deltas_mut()
            .del(HIST_KIND, &[who.to_string(), start.to_string()]);
    }
}

/// A deleted character leaves its player corporation the way an expelled one
/// does (SOC-CHARDEL): the move, then the corp's "member left" notice. The
/// caller has refused a CEO.
pub(crate) fn leave_for_delete(reg: &mut Registry, character_id: i64) {
    let left = reg
        .corporations
        .membership(character_id)
        .map(|m| m.corporation_id);
    // Only a member of a player corp leaves anything; a school or militia
    // pilot stays where it is.
    let Some(c) = left else { return };
    move_character(reg, character_id, None);
    super::notifications::corp_events::corp_member_left(reg, character_id, c, false);
}

/// Move a character between corps: the membership, the character's own
/// field, the session, and the other members' member lists.
///
/// `to` is `None` for "back to the NPC corp". Never fails: every refusal
/// has been made by the caller, in the same turn, under the same lock.
fn move_character(reg: &mut Registry, character_id: i64, to: Option<(i64, i64)>) {
    move_character_to(reg, character_id, to, None);
}

/// A pilot enlists in a militia from an NPC corporation: they sit in the
/// militia's own NPC corporation, no membership (FW-2). The same session
/// change carries `warfactionid`.
pub(crate) fn join_npc_corp(reg: &mut Registry, character_id: i64, corp: i64) {
    move_character_to(reg, character_id, None, Some(corp));
}

/// [`move_character`], where "back to the NPC corp" may name which one.
fn move_character_to(
    reg: &mut Registry,
    character_id: i64,
    to: Option<(i64, i64)>,
    npc: Option<i64>,
) {
    let now = crate::pyutil::filetime_now();
    let war_before = super::facwar_mgr::war_faction(reg, character_id);
    let militia_before = super::facwar_mgr::militia_of_corp(reg, character_id);
    let old = reg
        .character(character_id)
        .map_or(crate::model::NPC_CORPORATION_ID, |c| c.corporation_id);
    // CORP-S: the corps' derived NPC standings before the move, to tell the
    // members who stay if a leaver or a joiner changes them.
    let standings_before = [
        crate::corp_standings::before(reg, old),
        crate::corp_standings::before(reg, to.map_or(0, |(corp, _)| corp)),
    ];
    // A pilot who leaves a corp at war cannot slip into another for a week
    // (C8, `CharacterDodgedWar`).
    if to.is_none_or(|(corp, _)| corp != old) && is_player_corp(reg, old) {
        crate::war_deal::on_member_left(reg, character_id, old);
    }
    let (new, roles) = match to {
        Some((corp, roles)) => {
            reg.corporations.enrol(character_id, corp, roles, now);
            (corp, roles)
        }
        None => {
            reg.corporations.drop_member(character_id);
            (npc.unwrap_or(crate::model::NPC_CORPORATION_ID), 0)
        }
    };
    // The new corp's alliance comes with it (SOC-C1): the field the slims
    // and the login read, and the `allianceid` key in the same change.
    let old_alliance = reg.character(character_id).and_then(|c| c.alliance_id);
    let new_alliance = crate::alliance::of_corp(reg.deltas(), new);
    if let Some(c) = reg.character_mut(character_id) {
        c.corporation_id = new;
        c.alliance_id = new_alliance;
    }
    reg.dirty = true;
    if old != new {
        record_history(reg, character_id, old, new, now);
    }
    for was in standings_before {
        crate::corp_standings::announce(reg, was, Some(character_id));
    }
    let mut change = corp_change(new, roles);
    if old_alliance != new_alliance {
        change.extend(super::alliance::session_change(old_alliance, new_alliance));
    }
    // A move that changes whose war the pilot fights (a militia corporation,
    // or a player corporation in FW) carries `warfactionid` in the same
    // change, as `PerformSessionChange` waits for it (FW-2).
    let war_after = super::facwar_mgr::war_faction(reg, character_id);
    if let Some(war) = super::facwar_mgr::war_change(war_before, war_after) {
        change.extend(war);
    }
    reg.queue_session_change(character_id, change);
    if let Some(f) = militia_before
        && super::facwar_mgr::militia_of_corp(reg, character_id) != Some(f)
    {
        super::facwar_mgr::retired(reg, character_id, f, now);
    }
    if war_before != war_after {
        super::facwar_mgr::announce(reg, character_id, war_before, now);
    }
    // The member lists of both corps, if they are player corps. The mover's
    // own list rebuilds from the session change (`bco_members.DoSessionChanging`
    // drops it on `corpid`).
    for corp in [old, new] {
        if old == new || !is_player_corp(reg, corp) {
            continue;
        }
        let to: Vec<i64> = reg
            .corporations
            .members_of(corp)
            .into_iter()
            .filter(|id| *id != character_id)
            .collect();
        if !to.is_empty() {
            reg.push(MemberChanged {
                to,
                corporation_id: corp,
                member_id: character_id,
                old_corporation_id: old,
                new_corporation_id: new,
            });
        }
    }
}

/// A string argument, `str` or `unicode`. `None` for anything else.
fn text_arg(v: Option<&PyValue>) -> Option<String> {
    match v? {
        PyValue::Unicode(s) => Some(s.clone()),
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }
}

fn int_arg(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn real_arg(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(r) => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// Positional argument `i`, or the keyword of that name. The create form
/// passes the last three by keyword (`applicationsEnabled=`,
/// `friendlyFireEnabled=`, `loyaltyPointTaxRate=`) and `bco_corporations`
/// re-passes everything positionally; either reaches us.
fn arg<'a>(req: &'a Request<'_>, i: usize, name: &str) -> Option<&'a PyValue> {
    req.args.get(i).or_else(|| {
        // The wire pairs are `(value, key)`: `pairs_get` is the one reader
        // that knows it (tracker §39; found again by CORP-W).
        crate::pyutil::pairs_get(req.kwargs, name.as_bytes())
    })
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

/// `corpRegistry.AddCorporation(corporationName, tickerName, description,
/// url, iskTaxRate, shape1, shape2, shape3, color1, color2, color3,
/// typeface, applicationsEnabled, friendlyFireEnabled, loyaltyPointTaxRate)`
/// -- `bco_corporations.AddCorporation` **[V]**, fifteen positionals.
///
/// **The return value is discarded** (`CreateCorp.Submit` calls it as a
/// statement and closes the window) **[V]**, so the whole effect is the
/// session change: the client learns it is in a new corp from `corpid`, and
/// every corp panel re-reads from there.
///
/// Refusals are the client's own keys, each of which the form raises or
/// `ShowCreateCorpForm` prompts for itself -- so a player who reaches us past
/// those gates sees the message they would have seen first:
/// `CanOnlyCreateCorpInStation`, `CEOCannotCreateCorporation`,
/// `NotEnoughMoney {amount, balance}`, `EnterCorporationName`,
/// `EnterTickerName`, `CorpNameInvalidMaxLength`, `CorpTickerNameInvalid`,
/// `CorpNameInvalidTaken`, `CorpTickerNameInvalidTaken` -- every one present
/// in `findings/user-errors.tsv` **[V]**.
///
/// `[A]` the server does not re-check the Corporation Management skill the
/// client gates the form on: it is a client-side gate in this build, and the
/// skill buys nothing else here.
fn add_corporation(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let name = text_arg(arg(req, 0, "corporationName"))
        .ok_or_else(|| bad_args(req, "corporationName is not a string"))?;
    let ticker = text_arg(arg(req, 1, "tickerName"))
        .ok_or_else(|| bad_args(req, "tickerName is not a string"))?;
    let (name, ticker) = (name.trim().to_owned(), ticker.trim().to_owned());
    if reg.docked_station(actor).is_none() {
        return Err(refuse("CanOnlyCreateCorpInStation"));
    }
    if let Some(m) = reg.corporations.membership(actor)
        && reg
            .corporations
            .get(m.corporation_id)
            .is_some_and(|c| c.ceo_id == actor)
    {
        return Err(refuse("CEOCannotCreateCorporation"));
    }
    if name.is_empty() {
        return Err(refuse("EnterCorporationName"));
    }
    if ticker.is_empty() {
        return Err(refuse("EnterTickerName"));
    }
    if name.chars().count() > CORP_NAME_MAX_LEN {
        return Err(refuse("CorpNameInvalidMaxLength"));
    }
    // SEC-FIX 7: no control characters or markup in a name.
    if name.chars().any(crate::corp_wallet::bad_name_char) {
        return Err(refuse("CorpNameInvalidSomeChar"));
    }
    // `[A]` on the alphabet: Tranquility's rules are server-side and not in
    // the client. Letters, digits, `.`, `-` and `_` keep a ticker printable
    // in every place it is drawn between square brackets.
    if ticker.chars().count() > CORP_TICKER_MAX_LEN
        || !ticker
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return Err(refuse("CorpTickerNameInvalid"));
    }
    if reg.corporations.name_taken(&name)
        || name.eq_ignore_ascii_case(&reg.config().corporation_name)
    {
        return Err(refuse("CorpNameInvalidTaken"));
    }
    if reg.corporations.ticker_taken(&ticker) {
        return Err(refuse("CorpTickerNameInvalidTaken"));
    }
    let balance = reg.character(actor).map_or(0.0, |c| c.balance);
    if balance < CORPORATION_STARTUP_COST {
        return Err(refuse_with(
            "NotEnoughMoney",
            vec![
                ("amount", PyValue::Real(CORPORATION_STARTUP_COST)),
                ("balance", PyValue::Real(balance)),
            ],
        ));
    }
    // Rates arrive as fractions (`GetValue() / 100.0` in the form) **[V]**;
    // clamped rather than refused, because the form's own spinner bounds them.
    let rate = |v: Option<f64>| v.unwrap_or(0.0).clamp(0.0, 1.0);
    let opt = |i: usize, name: &str| int_arg(arg(req, i, name));
    let now = crate::pyutil::filetime_now();
    let corporation_id = reg.corporations.next_corp_id();
    let corp = Corporation {
        corporation_id,
        name,
        ticker,
        description: capped(
            text_arg(arg(req, 2, "description")).unwrap_or_default(),
            CORP_DESCRIPTION_MAX_LEN,
        ),
        url: capped(
            text_arg(arg(req, 3, "url")).unwrap_or_default(),
            CORP_URL_MAX_LEN,
        ),
        tax_rate: rate(real_arg(arg(req, 4, "iskTaxRate"))),
        shapes: [opt(5, "shape1"), opt(6, "shape2"), opt(7, "shape3")],
        colors: [opt(8, "color1"), opt(9, "color2"), opt(10, "color3")],
        recruiting: opt(12, "applicationsEnabled").is_none_or(|v| v != 0),
        aggression: Aggression::founded(opt(13, "friendlyFireEnabled").is_some_and(|v| v != 0)),
        lp_tax_rate: rate(real_arg(arg(req, 14, "loyaltyPointTaxRate"))),
        ceo_id: actor,
        creator_id: actor,
        create_date: now,
        deleted: false,
    };
    reg.corporations.corps.insert(corporation_id, corp);
    // The fee goes to the station's owner, the counterparty a docked
    // character has. `[A]`: Tranquility books it to the SCC.
    let counterparty = reg.character(actor).map_or(0, |c| c.station_owner_id);
    reg.charge(
        actor,
        CORPORATION_STARTUP_COST,
        REF_CORPORATION_REGISTRATION_FEE,
        counterparty,
        corporation_id,
    );
    move_character(reg, actor, Some((corporation_id, CEO_ROLES)));
    // S7: the founding grant of shares goes to the CEO.
    crate::corp_shares::found(reg.deltas_mut(), corporation_id, actor);
    Ok(PyValue::None)
}

/// `GetSuggestedTickerNames(corporationName)`: a list of rows read for
/// `.tickerName` and nothing else (`CorpDetails.PickTicker`) **[V]**; empty
/// is `NoCorpTickerNameSuggestions`, the client's own prompt.
///
/// Initials, then the first letters, then the first letters with a digit --
/// upper case, at most five, none taken. `[A]` on the recipe; the client only
/// lists what we send.
fn suggested_tickers(reg: &Registry, name: &str) -> Vec<String> {
    let words: Vec<String> = name
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_uppercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = words.concat();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    let mut candidates: Vec<String> = Vec::new();
    for c in [
        initials
            .chars()
            .take(CORP_TICKER_MAX_LEN)
            .collect::<String>(),
        letters.chars().take(CORP_TICKER_MAX_LEN).collect(),
        letters.chars().take(3).collect(),
    ] {
        if !c.is_empty() && !candidates.contains(&c) {
            candidates.push(c);
        }
    }
    let base: String = letters.chars().take(CORP_TICKER_MAX_LEN - 1).collect();
    if !base.is_empty() {
        for d in 1..=9 {
            candidates.push(format!("{base}{d}"));
        }
    }
    candidates
        .into_iter()
        .filter(|t| !reg.corporations.ticker_taken(t))
        .take(5)
        .collect()
}

/// The actor's player corp and their membership, or `None` in the NPC corp.
fn my_corp(reg: &Registry, actor: Option<i64>) -> Option<(i64, Member)> {
    let m = reg.corporations.membership(actor?)?;
    Some((m.corporation_id, m))
}

pub(crate) fn is_ceo(reg: &Registry, character_id: i64) -> bool {
    reg.corporations
        .membership(character_id)
        .and_then(|m| reg.corporations.get(m.corporation_id))
        .is_some_and(|c| c.ceo_id == character_id)
}

/// `GetEveOwners`: one row per member, spliced into `cfg.eveowners.data` as
/// `list(owner) + [None]` (`bco_members.__PrimeEveOwners`) **[V]** -- so a
/// row must be **iterable** as well as carry `.ownerID`. A `util.Row` is
/// both: `__getattr__` by header name and `__getitem__` by index, which is
/// the old sequence protocol `list()` uses **[V]** (`carbon/.../row.py`).
/// Four columns, because the appended `None` becomes the fifth of
/// `cfg.eveowners`' five (`OWNER_COLUMNS`: `ownerNameID`).
///
/// **This is the member list.** `GetMemberIDs` is these ids, and
/// `sessionChecker.isPilotInSameCorpAs` -- which gates "Send Corp Invite" and
/// "Expel" -- is `ownerID in GetMemberIDs()` **[V]**.
///
/// The NPC corp still answers `[]`: nothing here wants a same-NPC-corp
/// member list, and it is what every login has been answered with.
fn eve_owners(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some((corp, _)) = my_corp(reg, actor) else {
        return PyValue::List(vec![]);
    };
    let rows = reg
        .corporations
        .members_of(corp)
        .into_iter()
        .filter_map(|id| reg.character(id))
        .filter_map(|c| {
            eve_rowset::Row::new(
                &["ownerID", "ownerName", "typeID", "gender"],
                vec![
                    PyValue::Int(c.character_id),
                    PyValue::Unicode(c.name.clone()),
                    PyValue::Int(CHARACTER_TYPE_ID),
                    PyValue::Int(c.gender_id),
                ],
            )
            .ok()
            .map(|r| r.to_pyvalue())
        })
        .collect();
    PyValue::List(rows)
}

/// One member, as `GetMember`/`GetMembersByIds` answer it.
///
/// **A `util.Row`**, because `PagedCorpMembers.OnCorporationMemberChanged`
/// writes into it with `member[key] = newValue` **[V]** and `Row.__setitem__`
/// takes a column name. The columns are the ones read by name:
/// `GetMyGrantableRoles` (`roles`, the four `grantableRoles*`, `titleMask`),
/// `MemberBlocksRoles` (`blockRoles`), `UpdateMember`'s round trip (the
/// rest) **[V]** -- `bco_members.py`.
fn member_row(reg: &Registry, character_id: i64) -> Option<PyValue> {
    // The row's own masks: the client adds the titles' roles itself
    // ("role by title", `corp_dlg_edit_member`) **[V]**.
    let m = reg.corporations.direct(character_id)?;
    let header = [
        "characterID",
        "corporationID",
        "title",
        "titleMask",
        "roles",
        "grantableRoles",
        "rolesAtHQ",
        "grantableRolesAtHQ",
        "rolesAtBase",
        "grantableRolesAtBase",
        "rolesAtOther",
        "grantableRolesAtOther",
        "baseID",
        "startDateTime",
        "divisionID",
        "squadronID",
        "blockRoles",
        "accountKey",
    ];
    // The eight masks as stored (SOC-C3). A CEO's or director's row is
    // redrawn with every role by the client itself
    // (`RoleAccessEntry.GetMembersListData`) **[V]**, and an active CEO's
    // grants come from `GetMyGrantableRoles`, not from this row.
    let mut line = vec![
        PyValue::Int(character_id),
        PyValue::Int(m.corporation_id),
        PyValue::Unicode(reg.corporations.title(character_id).to_owned()),
        PyValue::Int(m.title_mask),
    ];
    line.extend(m.masks().into_iter().map(PyValue::Int));
    line.extend([
        PyValue::None,
        PyValue::Int(m.start_date),
        PyValue::None,
        PyValue::None,
        // `MemberBlocksRoles` reads it when not `None` **[V]**.
        PyValue::Int(i64::from(m.block_roles)),
        PyValue::Int(1000),
    ]);
    eve_rowset::Row::new(&header, line)
        .ok()
        .map(|r| r.to_pyvalue())
}

/// `GetMemberTrackingInfo` (directors) and `...Simple` (everyone): the
/// Members tab. `CorpMemberTracking.ShowMemberTracking` **[V]** iterates it
/// and reads `characterID`, `corporationID`, `baseID`, `locationID`,
/// `roles`, `grantableRoles`, `lastOnline`, `title`, `startDateTime`,
/// `shipTypeID`, `factionID`, `logonDateTime`, `logoffDateTime` -- nothing
/// else, all attribute reads, so a list of `Row`s.
///
/// `lastOnline` is in **hours**, negative for "online now", `None` for
/// "more than a month" (`GetLastLoggedOnText`) **[V]**.
fn member_tracking(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some((corp, _)) = my_corp(reg, actor) else {
        return PyValue::List(vec![]);
    };
    let now = crate::pyutil::filetime_now();
    let header = [
        "characterID",
        "corporationID",
        "baseID",
        "locationID",
        "roles",
        "grantableRoles",
        "lastOnline",
        "title",
        "startDateTime",
        "shipTypeID",
        "factionID",
        "logonDateTime",
        "logoffDateTime",
    ];
    let mut rows = Vec::new();
    for id in reg.corporations.members_of(corp) {
        let (Some(c), Some(m)) = (reg.character(id), reg.corporations.membership(id)) else {
            continue;
        };
        let online = reg.sessions().is_online(id);
        let last_online = if online {
            PyValue::Real(-1.0)
        } else if c.logoff_date > 0 {
            PyValue::Real((now - c.logoff_date).max(0) as f64 / 36_000_000_000.0)
        } else {
            PyValue::None
        };
        let line = vec![
            PyValue::Int(id),
            PyValue::Int(corp),
            PyValue::None,
            reg.docked_station(id)
                .or_else(|| reg.session_system(id))
                .map_or(PyValue::None, PyValue::Int),
            PyValue::Int(m.roles),
            PyValue::Int(m.grantable_roles),
            last_online,
            PyValue::Unicode(reg.corporations.title(id).to_owned()),
            PyValue::Int(m.start_date),
            PyValue::Int(c.ship_type_id),
            PyValue::None,
            PyValue::None,
            if c.logoff_date > 0 {
                PyValue::Int(c.logoff_date)
            } else {
                PyValue::None
            },
        ];
        if let Ok(r) = eve_rowset::Row::new(&header, line) {
            rows.push(r.to_pyvalue());
        }
    }
    PyValue::List(rows)
}

/// One application as the client reads it: `applicationID`, `characterID`,
/// `corporationID`, `applicationText`, `status`, `applicationDateTime`,
/// `responseText` -- every attribute read off an application in
/// `corp_ui_applications.py` and `bco_applications.py` **[V]**. A `KeyVal`,
/// because `GetCorpApplicationEntries` puts them in a `set` and a `KeyVal`
/// hashes by identity.
fn application_value(a: &Application) -> PyValue {
    keyval(vec![
        ("applicationID", PyValue::Int(a.application_id)),
        ("characterID", PyValue::Int(a.character_id)),
        ("corporationID", PyValue::Int(a.corporation_id)),
        ("applicationText", PyValue::Unicode(a.text.clone())),
        ("status", PyValue::Int(a.status)),
        ("applicationDateTime", PyValue::Int(a.date)),
        ("responseText", PyValue::Unicode(String::new())),
        ("customMessage", PyValue::Unicode(String::new())),
    ])
}

/// A dict of lists keyed by an int, the shape both `GetMyApplications`
/// (keyed by corporationID) and `GetApplications` (by characterID) have:
/// `len`, `in`, `[k]`, `[k] = list` and key iteration **[V]**.
/// `PyValue::Dict` entries are `(value, key)`.
fn grouped(rows: Vec<(i64, PyValue)>) -> PyValue {
    let mut by: std::collections::BTreeMap<i64, Vec<PyValue>> = std::collections::BTreeMap::new();
    for (k, v) in rows {
        by.entry(k).or_default().push(v);
    }
    PyValue::Dict(
        by.into_iter()
            .map(|(k, v)| (PyValue::List(v), PyValue::Int(k)))
            .collect(),
    )
}

/// `GetMyApplications`: `{corporationID: [application, ...]}` -- every
/// application of the caller's that is still open. The end statuses are
/// history and go out through `GetMyOldApplications`.
fn my_applications(reg: &Registry, actor: Option<i64>) -> PyValue {
    let Some(me) = actor else {
        return PyValue::Dict(vec![]);
    };
    grouped(
        reg.corporations
            .applications
            .values()
            .filter(|a| a.character_id == me && !APP_END_STATUSES.contains(&a.status))
            .map(|a| (a.corporation_id, application_value(a)))
            .collect(),
    )
}

/// Who hears about an application: the applicant, and the corp's members
/// who may act on it.
///
/// **Only personnel managers on the corp side**, and the reason is a client
/// bug worth not tripping: `ApplicationsO.UpdateApplications` does
/// `self.corpApplications[applicantID] = ...` after `GetApplications`, which
/// returns `{}` *without setting the cache* for a member lacking
/// `corpRolePersonnelManager` -- so the store hits `None` and raises inside
/// the notify chain **[V]** (`bco_applications.py`).
fn application_audience(reg: &Registry, a: &Application) -> Vec<i64> {
    let mut to = vec![a.character_id];
    for id in reg.corporations.members_of(a.corporation_id) {
        let roles = reg.corporations.membership(id).map_or(0, |m| m.roles);
        if roles & CORP_ROLE_PERSONNEL_MANAGER != 0 && id != a.character_id {
            to.push(id);
        }
    }
    to
}

fn tell_application(reg: &mut Registry, a: &Application) {
    let to = application_audience(reg, a);
    reg.push(ApplicationChanged {
        to,
        corporation_id: a.corporation_id,
        applicant_id: a.character_id,
        application_id: a.application_id,
        row: application_value(a),
    });
}

/// `InsertApplication(corporationID, applicationText)` -- the character side.
/// `InsertInvitation(characterID)` -- the corp side, a personnel manager
/// inviting somebody (`base_corporation.InviteToJoinCorp`, reached from the
/// character menu's "Send Corp Invite") **[V]**.
///
/// Both return values are discarded by their callers **[V]**; the push is
/// what redraws both sides.
fn insert_application(
    reg: &mut Registry,
    character_id: i64,
    corporation_id: i64,
    status: i64,
    text: String,
) -> Result<PyValue, ServiceError> {
    if !is_player_corp(reg, corporation_id) {
        return Err(refuse_with(
            "CustomInfo",
            vec![("info", bstr("That corporation does not take applications."))],
        ));
    }
    if reg.character(character_id).map(|c| c.corporation_id) == Some(corporation_id) {
        let name = reg
            .corporations
            .get(corporation_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        return Err(refuse_with(
            "CanNotJoinCorpAlreadyAMember",
            vec![("corpName", PyValue::Unicode(name))],
        ));
    }
    crate::war_deal::may_rejoin(reg, character_id, corporation_id).map_err(refuse)?;
    if reg
        .corporations
        .open_between(character_id, corporation_id)
        .is_some()
    {
        return Err(refuse_with(
            "CustomInfo",
            vec![(
                "info",
                bstr(
                    "There is already an open application between this pilot and this corporation.",
                ),
            )],
        ));
    }
    if text.chars().count() > APP_TEXT_MAX {
        return Err(refuse_with(
            "CustomInfo",
            vec![("info", bstr("The application text is too long."))],
        ));
    }
    let a = Application {
        application_id: reg.corporations.next_application_id(),
        character_id,
        corporation_id,
        status,
        date: crate::pyutil::filetime_now(),
        text,
    };
    reg.corporations
        .applications
        .insert(a.application_id, a.clone());
    reg.dirty = true;
    tell_application(reg, &a);
    Ok(PyValue::None)
}

/// `UpdateApplicationOffer(applicationID, characterID, corporationID,
/// applicationText, status, customMessage='', applicationDateTime=None)`
/// (`bco_applications.UpdateApplicationOffer`) **[V]**.
///
/// Which side may set which status, from the buttons the client draws
/// (`CorpApplicationEntry`, `corp_ui_applications.py`) **[V]**:
///
/// | who | from | to |
/// | --- | --- | --- |
/// | applicant | invited (8) or accepted-by-corp (6) | accepted (2) -- **joins** -- or rejected (3) |
/// | applicant | applied (0/1) | withdrawn (7) |
/// | personnel manager | applied (0/1) | accepted-by-corp (6) or rejected (4) |
/// | personnel manager | invited (8) | rejected (4) -- `[A]`, withdrawing an invite |
///
/// **Joining is the applicant's own click on 2**, never the corp's 6: the
/// applicant's entry shows "Accept" for exactly `(6, 8)`
/// (`node.myView and node.application.status in (...)`) **[V]**.
fn update_application(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let id = int_arg(arg(req, 0, "applicationID"))
        .ok_or_else(|| bad_args(req, "applicationID is not an int"))?;
    let status =
        int_arg(arg(req, 4, "status")).ok_or_else(|| bad_args(req, "status is not an int"))?;
    let Some(mut a) = reg.corporations.applications.get(&id).cloned() else {
        return Err(refuse_with(
            "CustomInfo",
            vec![("info", bstr("That application no longer exists."))],
        ));
    };
    let denied = || {
        refuse_with(
            "CrpAccessDenied",
            vec![(
                "reason",
                bstr("You cannot make that change to this application."),
            )],
        )
    };
    let manager = reg.corporations.membership(actor).is_some_and(|m| {
        m.corporation_id == a.corporation_id && m.roles & CORP_ROLE_PERSONNEL_MANAGER != 0
    });
    let from = a.status;
    let allowed = if actor == a.character_id {
        matches!(
            (from, status),
            (
                APP_INVITED_BY_CORPORATION | APP_ACCEPTED_BY_CORPORATION,
                APP_ACCEPTED_BY_CHARACTER | APP_REJECTED_BY_CHARACTER
            ) | (
                APP_APPLIED_BY_CHARACTER | APP_RENEGOTIATED_BY_CHARACTER,
                APP_WITHDRAWN_BY_CHARACTER
            )
        )
    } else if manager {
        matches!(
            (from, status),
            (
                APP_APPLIED_BY_CHARACTER | APP_RENEGOTIATED_BY_CHARACTER,
                APP_ACCEPTED_BY_CORPORATION | APP_REJECTED_BY_CORPORATION
            ) | (APP_INVITED_BY_CORPORATION, APP_REJECTED_BY_CORPORATION)
        )
    } else {
        false
    };
    if !allowed {
        return Err(denied());
    }
    if status == APP_ACCEPTED_BY_CHARACTER {
        // `CeoCanNotJoinACorp` is the client's own refusal for this
        // (`ApplyForMembership`) **[V]**; a CEO leaving would orphan a corp.
        if is_ceo(reg, actor) {
            return Err(refuse("CrpCantQuitCEO"));
        }
        if !is_player_corp(reg, a.corporation_id) {
            return Err(denied());
        }
    }
    let from = a.status;
    a.status = status;
    reg.corporations.applications.insert(id, a.clone());
    reg.dirty = true;
    if status == APP_ACCEPTED_BY_CHARACTER {
        // `[A]`: the character's other open applications stay open, as
        // they would for a player who later changes corp again.
        move_character(reg, actor, Some((a.corporation_id, 0)));
    }
    tell_application(reg, &a);
    // NOTIF-1: the sixth argument of `UpdateApplicationOffer` is the
    // rejection's custom message (`bco_applications`) **[V]**.
    let custom = text_arg(arg(req, 5, "customMessage")).unwrap_or_default();
    super::notifications::corp_events::corp_application(
        reg,
        actor,
        a.character_id,
        a.corporation_id,
        Some(from),
        status,
        &a.text,
        &custom,
    );
    Ok(PyValue::None)
}

/// `KickOutMember(characterID)`: leaving when it is yourself
/// (`base_corporation.KickOut` wraps it in
/// `PerformSessionChange('corp.kickself', ...)`) and expelling when it is
/// not **[V]**.
fn kick_out(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let target = opt_id(req.args.first()).unwrap_or(actor);
    expel(reg, actor, target)?;
    Ok(PyValue::None)
}

/// The rule `KickOutMember` enforces and `CanBeKickedOut` reports.
///
/// Keys `CrpCantQuitDefaultCorporation` and `CrpCantQuitCEO` are the
/// client's own, both in `findings/user-errors.tsv` with no arguments
/// **[V]**. A CEO cannot leave: they hand the corp on, or close it, with
/// `ResignFromCEO` first (SOC-C3).
fn can_be_kicked_out(reg: &Registry, actor: i64, target: i64) -> Result<(), ServiceError> {
    let Some(m) = reg.corporations.membership(target) else {
        // Retire (FW-2): the militia corporation is the one NPC corp a pilot
        // leaves, and only they can, themselves.
        if actor == target && super::facwar_mgr::militia_of_corp(reg, target).is_some() {
            return Ok(());
        }
        return Err(refuse("CrpCantQuitDefaultCorporation"));
    };
    if is_ceo(reg, target) {
        return Err(refuse("CrpCantQuitCEO"));
    }
    if actor != target {
        let director = reg.corporations.membership(actor).is_some_and(|a| {
            a.corporation_id == m.corporation_id && a.roles & CORP_ROLE_DIRECTOR != 0
        });
        if !director {
            return Err(refuse_with(
                "CrpAccessDenied",
                vec![("reason", bstr("Only a director can expel a member."))],
            ));
        }
    }
    Ok(())
}

/// `corpmgr.GetPublicInfo(corpID)` -- the corporation info window's header
/// and Attributes tab.
///
/// **A `KeyVal`**, because `update_description_caption_and_sub_caption`
/// assigns `self.corp_info.shares = ...` onto it **[V]**
/// (`corporationInfoWindow.py`). The attributes are every name read off it
/// there and in `infosvc` (`corporationID`, `ceoID`, `creatorID`,
/// `allianceID`, `description`, `deleted`, `stationID`, `url`, `shares`, and
/// the `getattr(..., 0.0)` loop's `tickerName`, `memberCount`, `taxRate`,
/// `loyaltyPointTaxRate`, `friendlyFire`, plus `allowWar`) **[V]**.
fn public_info(reg: &Registry, corporation_id: i64) -> PyValue {
    let v = corp_view(reg, corporation_id);
    keyval(vec![
        ("corporationID", PyValue::Int(corporation_id)),
        ("corporationName", PyValue::Unicode(v.name.clone())),
        ("name", PyValue::Unicode(v.name.clone())),
        ("tickerName", PyValue::Unicode(v.ticker.clone())),
        ("ceoID", PyValue::Int(v.ceo_id)),
        ("creatorID", PyValue::Int(v.creator_id)),
        (
            "allianceID",
            opt_int(crate::alliance::of_corp(reg.deltas(), corporation_id)),
        ),
        ("description", PyValue::Unicode(v.description.clone())),
        ("url", PyValue::Unicode(v.url.clone())),
        ("deleted", PyValue::Int(i64::from(v.deleted))),
        ("stationID", PyValue::None),
        ("shares", PyValue::Int(0)),
        ("memberCount", PyValue::Int(v.member_count)),
        ("taxRate", PyValue::Real(v.tax_rate)),
        ("loyaltyPointTaxRate", PyValue::Real(v.lp_tax_rate)),
        ("friendlyFire", PyValue::Int(i64::from(v.friendly_fire))),
        ("allowWar", crate::war::permit_value(reg, corporation_id)),
    ])
}

/// Everything the corporation row and the public info need, for either kind
/// of corp.
struct CorpView {
    name: String,
    ticker: String,
    description: String,
    url: String,
    tax_rate: f64,
    lp_tax_rate: f64,
    recruiting: bool,
    friendly_fire: bool,
    member_count: i64,
    ceo_id: i64,
    creator_id: i64,
    create_date: Option<i64>,
    shapes: [Option<i64>; 3],
    colors: [Option<i64>; 3],
    deleted: bool,
}

fn corp_view(reg: &Registry, corporation_id: i64) -> CorpView {
    match reg.corporations.get(corporation_id) {
        Some(c) => CorpView {
            name: c.name.clone(),
            ticker: c.ticker.clone(),
            description: c.description.clone(),
            url: c.url.clone(),
            tax_rate: c.tax_rate,
            lp_tax_rate: c.lp_tax_rate,
            recruiting: c.recruiting,
            friendly_fire: c.aggression.legal_at(crate::pyutil::filetime_now()),
            member_count: reg.corporations.members_of(corporation_id).len() as i64,
            ceo_id: c.ceo_id,
            creator_id: c.creator_id,
            create_date: Some(c.create_date),
            shapes: c.shapes,
            colors: c.colors,
            deleted: c.deleted,
        },
        // A militia's own NPC corporation names itself (FW-2).
        None if reg
            .ext::<crate::service::facwar_mgr::Index>()
            .militia_by_corp
            .contains_key(&corporation_id) =>
        {
            let npc = reg
                .content()
                .try_get::<crate::npc_data::Table>()
                .and_then(|t| t.corporations.get(&corporation_id));
            CorpView {
                name: npc.map_or_else(|| reg.config().corporation_name.clone(), |c| c.name.clone()),
                ticker: npc.map_or_else(|| CORP_TICKER.to_owned(), |c| c.ticker.clone()),
                description: String::new(),
                url: String::new(),
                tax_rate: 0.0,
                lp_tax_rate: 0.0,
                recruiting: false,
                friendly_fire: false,
                member_count: reg
                    .config()
                    .characters
                    .iter()
                    .filter(|c| c.corporation_id == corporation_id)
                    .count() as i64,
                ceo_id: NPC_CEO_ID,
                creator_id: NPC_CEO_ID,
                create_date: None,
                shapes: [None; 3],
                colors: [None; 3],
                deleted: false,
            }
        }
        // The NPC corp, exactly as `corporation_row` described it before
        // player corps existed.
        None => CorpView {
            name: reg.config().corporation_name.clone(),
            ticker: CORP_TICKER.to_owned(),
            description: String::new(),
            url: String::new(),
            tax_rate: 0.0,
            lp_tax_rate: 0.0,
            recruiting: false,
            friendly_fire: false,
            member_count: 1,
            ceo_id: NPC_CEO_ID,
            creator_id: NPC_CEO_ID,
            create_date: None,
            shapes: [None; 3],
            colors: [None; 3],
            deleted: false,
        },
    }
}

/// `AggressionSettings` for one corp: the module's own two constants
/// (`ALWAYS_ENABLED = (0, None)`, `ALWAYS_DISABLED = (None, 0)`) **[V]**.
/// The NPC corp is always disabled, as its default picker says; a player
/// corp follows the friendly-fire box on the create form -- `[A]` that the
/// box means exactly this.
fn aggression_settings(reg: &Registry, corporation_id: i64) -> PyValue {
    let a = reg
        .corporations
        .get(corporation_id)
        .map_or(Aggression::founded(false), |c| c.aggression);
    let t = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    instance(
        "crimewatch.corp_aggression.settings.AggressionSettings",
        vec![
            ("_enableAfter", t(a.enable_after)),
            ("_disableAfter", t(a.disable_after)),
        ],
    )
}

/// The wait before a friendly-fire change takes effect: 24 h, as a
/// game-clock span. `[A]`; falsifier: a Tranquility capture or a client
/// string with another delay (EvEJS uses 24 h).
const FF_CHANGE_DELAY: i64 = 24 * 3600 * 10_000_000;

/// A corporation's timed friendly-fire setting: the client's
/// `AggressionSettings` (`_enableAfter`, `_disableAfter`, FILETIMEs or
/// `None`). The founding states are its `ALWAYS_ENABLED = (0, None)` and
/// `ALWAYS_DISABLED = (None, 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aggression {
    pub enable_after: Option<i64>,
    pub disable_after: Option<i64>,
}

impl Aggression {
    pub fn founded(legal: bool) -> Aggression {
        if legal {
            Aggression {
                enable_after: Some(0),
                disable_after: None,
            }
        } else {
            Aggression {
                enable_after: None,
                disable_after: Some(0),
            }
        }
    }

    /// Is friendly fire legal at `now`? Of the two times already passed the
    /// later one wins; an enable with no disable passed is legal.
    /// `[A]` that this is `settings.pyc:28-37`'s rule (read from the spec,
    /// not the bytecode); falsifier: a pending change that shows the wrong
    /// state in the Home tab.
    pub fn legal_at(&self, now: i64) -> bool {
        let en = self.enable_after.filter(|t| *t <= now);
        let dis = self.disable_after.filter(|t| *t <= now);
        match (en, dis) {
            (Some(e), Some(d)) => e > d,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// A change is pending when either time is still in the future.
    pub fn pending_at(&self, now: i64) -> bool {
        self.enable_after.is_some_and(|t| t > now) || self.disable_after.is_some_and(|t| t > now)
    }
}

/// `corpRegistry.RegisterNewAggressionSettings(legal)`: the director's
/// friendly-fire toggle. The change takes effect [`FF_CHANGE_DELAY`] later.
fn register_aggression(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = actor else {
        return Err(denied("No character on this session."));
    };
    let Some(m) = reg.corporations.membership(actor) else {
        return Ok(aggression_settings(reg, crate::model::NPC_CORPORATION_ID));
    };
    let corp_id = m.corporation_id;
    let allowed = m.roles & CORP_ROLE_DIRECTOR != 0 || is_ceo(reg, actor);
    let Some(corp) = reg.corporations.get(corp_id) else {
        // The NPC corp: nothing to change.
        return Ok(aggression_settings(reg, corp_id));
    };
    if !allowed {
        return Err(denied("Only a director can change friendly fire."));
    }
    let desired = match req.args.first() {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => return Err(bad_args(req, "argument 0 (legal) is not a bool")),
    };
    let now = crate::pyutil::filetime_now();
    let before = corp.aggression;
    if before.legal_at(now) == desired && !before.pending_at(now) {
        return Ok(aggression_settings(reg, corp_id));
    }
    let due = now + crate::game_clock::span(reg, FF_CHANGE_DELAY);
    let after = if desired {
        Aggression {
            enable_after: Some(due),
            disable_after: Some(0),
        }
    } else {
        Aggression {
            enable_after: Some(0),
            disable_after: Some(due),
        }
    };
    if let Some(c) = reg.corporations.corps.get_mut(&corp_id) {
        c.aggression = after;
    }
    reg.dirty = true;
    let settings = aggression_settings(reg, corp_id);
    let to = reg.corporations.members_of(corp_id);
    reg.push(AggressionChanged {
        to: to.clone(),
        settings: settings.clone(),
    });
    let (was, is) = (before.legal_at(now), after.legal_at(now));
    if was != is {
        reg.push(CorporationChanged {
            to,
            corporation_id: corp_id,
            change: vec![
                (
                    "corporationID",
                    PyValue::Int(corp_id),
                    PyValue::Int(corp_id),
                ),
                (
                    "friendlyFire",
                    PyValue::Int(i64::from(was)),
                    PyValue::Int(i64::from(is)),
                ),
            ],
        });
    }
    Ok(settings)
}

/// `OnCorpAggressionSettingsChange(settings)` to every online member.
#[derive(Debug, Clone, PartialEq)]
pub struct AggressionChanged {
    pub to: Vec<i64>,
    pub settings: PyValue,
}

impl crate::notify::Push for AggressionChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorpAggressionSettingsChange",
            vec![self.settings.clone()],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `corpmgr`, the unbound half of corporations: anything about a corp that is
/// not your own. It was not a service we had, so the corporation info window
/// raised on its first line (`reconstruct_info_window` ->
/// `GetPublicInfo`) for every corp, the NPC one included.
fn corpmgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp_arg = || opt_id(req.args.first());
    match req.method {
        "GetPublicInfo" => {
            let id = corp_arg().ok_or_else(|| bad_args(req, "corpID is not an id"))?;
            Ok(public_info(reg, id))
        }
        // `bco_corporations.GetCorporation` for a corp that is not
        // `session.corpid` **[V]** -- the same `util.Row` our own corp gets,
        // because `LoadCorporation` appends it to the `IndexRowset` it built
        // from the first one, column by column.
        "GetCorporations" => {
            let id = corp_arg().ok_or_else(|| bad_args(req, "corpID is not an id"))?;
            Ok(corporation_row(reg, id))
        }
        // `base_corporation.GetAggressionSettings(corpID)`, cached per corp.
        "GetAggressionSettings" => {
            let id = corp_arg().ok_or_else(|| bad_args(req, "corpID is not an id"))?;
            Ok(aggression_settings(reg, id))
        }
        // `{corpID: settings}` for the ids it did not have cached
        // (`GetAggressionSettingsForCorps`). `[A]` on the container: the
        // caller merges it into its own dict by key.
        "GetAggressionSettingsForCorps" => {
            let ids: Vec<i64> = match req.args.first() {
                Some(PyValue::List(v) | PyValue::Tuple(v)) => {
                    v.iter().filter_map(|x| opt_id(Some(x))).collect()
                }
                _ => Vec::new(),
            };
            Ok(PyValue::Dict(
                ids.into_iter()
                    .map(|id| (aggression_settings(reg, id), PyValue::Int(id)))
                    .collect(),
            ))
        }
        // An int (`characterInfoWindow`, `createContractWnd`). `[A]`: the NPC
        // corp for a character we do not know, as `charMgr.GetPublicInfo`
        // answers.
        "GetCorporationIDForCharacter" => {
            let id = opt_id(req.args.first()).or(req.actor);
            Ok(PyValue::Int(
                id.and_then(|id| reg.character(id))
                    .map_or(crate::model::NPC_CORPORATION_ID, |c| c.corporation_id),
            ))
        }
        // The Corporation > Assets tab (`corp_ui_accounts.ShowAssets`).
        "GetAssetInventory" => Ok(super::corp_office::asset_inventory(reg, req).to_pyvalue()),
        "GetAssetInventoryForLocation" => {
            Ok(super::corp_office::asset_inventory_for_location(reg, req).to_pyvalue())
        }
        _ => Err(reg.no_method(req)),
    }
}

/// The player-corp arms of `corpRegistry`. `None` for a method this half
/// does not answer, so the caller falls through to the NPC-era arms.
fn corp_registry(reg: &mut Registry, req: &Request<'_>) -> Option<Result<PyValue, ServiceError>> {
    let actor = req.actor;
    Some(match req.method {
        // Declaring war, the ally price and paying a bill (C8).
        "DeclareWarAgainst" | "CharGetAllyBaseCost" | "PayBill" => {
            return super::war::corp_registry(reg, req);
        }
        // The wallet's corpRegistry half (CORP-W).
        m if super::corp_wallet::CORP_REGISTRY_METHODS.contains(&m) => {
            super::corp_wallet::corp_registry(reg, req)
        }
        // Founding, joining and leaving an alliance (SOC-C1).
        m if super::alliance::CORP_REGISTRY_METHODS.contains(&m) => {
            super::alliance::corp_registry(reg, req)
        }
        // The structure browser's default reinforcement (U2-UPWELL).
        "GetStructureReinforceDefault" | "SetStructureReinforceDefault" => {
            return super::structures::corp_registry(reg, req);
        }
        // Shares and dividends (S7, #178).
        m if super::corp_shares::CORP_REGISTRY_METHODS.contains(&m) => {
            super::corp_shares::corp_registry(reg, req)
        }
        "AddCorporation" => add_corporation(reg, req),
        "RegisterNewAggressionSettings" => register_aggression(reg, req, actor),
        "GetSuggestedTickerNames" => {
            let name = text_arg(arg(req, 0, "corporationName")).unwrap_or_default();
            Ok(PyValue::List(
                suggested_tickers(reg, &name)
                    .into_iter()
                    .map(|t| keyval(vec![("tickerName", PyValue::Unicode(t))]))
                    .collect(),
            ))
        }
        // `None` for somebody not in the caller's corp, which
        // `GetMyGrantableRoles` handles (`if member is not None`) **[V]**.
        "GetMember" => {
            let id = opt_id(req.args.first()).or(actor);
            let mine = my_corp(reg, actor).map(|(c, _)| c);
            Ok(id
                .filter(|id| reg.corporations.membership(*id).map(|m| m.corporation_id) == mine)
                .and_then(|id| member_row(reg, id))
                .unwrap_or(PyValue::None))
        }
        // A list, indexed `[0]` by `PagedCorpMembers` and iterated for
        // `.characterID` **[V]**.
        "GetMembersByIds" => {
            let ids: Vec<i64> = match req.args.first() {
                Some(PyValue::List(v) | PyValue::Tuple(v)) => {
                    v.iter().filter_map(|x| opt_id(Some(x))).collect()
                }
                _ => Vec::new(),
            };
            let mine = my_corp(reg, actor).map(|(c, _)| c);
            Ok(PyValue::List(
                ids.into_iter()
                    .filter(|id| {
                        mine.is_some()
                            && reg.corporations.membership(*id).map(|m| m.corporation_id) == mine
                    })
                    .filter_map(|id| member_row(reg, id))
                    .collect(),
            ))
        }
        "GetMemberTrackingInfo" | "GetMemberTrackingInfoSimple" => Ok(member_tracking(reg, actor)),
        // `(canLeave, error, errorDetails)`, unpacked into three names by
        // `corp_ui_home` before it draws the Quit button **[V]** -- the only
        // reader; `ResignFromCorp` raises `UserError(error, errorDetails)`
        // itself when `canLeave` is false.
        "CanLeaveCurrentCorporation" => {
            let Some(me) = actor else {
                return Some(Err(bad_args(req, "no character")));
            };
            let verdict = if reg.corporations.membership(me).is_none() {
                Some("CrpCantQuitDefaultCorporation")
            } else if is_ceo(reg, me) {
                Some("CrpCantQuitCEO")
            } else {
                None
            };
            Ok(PyValue::Tuple(match verdict {
                None => vec![PyValue::Bool(true), PyValue::None, PyValue::None],
                Some(key) => vec![PyValue::Bool(false), bstr(key), PyValue::Dict(vec![])],
            }))
        }
        // A truthiness test in `base_corporation.KickOut` **[V]**.
        "CanBeKickedOut" => {
            let Some(me) = actor else {
                return Some(Err(bad_args(req, "no character")));
            };
            let target = opt_id(req.args.first()).unwrap_or(me);
            Ok(PyValue::Bool(can_be_kicked_out(reg, me, target).is_ok()))
        }
        "KickOutMember" => kick_out(reg, req),
        "InsertApplication" => {
            let Some(me) = actor else {
                return Some(Err(bad_args(req, "no character")));
            };
            let Some(corp) = opt_id(req.args.first()) else {
                return Some(Err(bad_args(req, "corporationID is not an id")));
            };
            let text = text_arg(req.args.get(1)).unwrap_or_default();
            let answer = insert_application(reg, me, corp, APP_APPLIED_BY_CHARACTER, text.clone());
            if answer.is_ok() {
                super::notifications::corp_events::corp_application(
                    reg,
                    me,
                    me,
                    corp,
                    None,
                    APP_APPLIED_BY_CHARACTER,
                    &text,
                    "",
                );
            }
            answer
        }
        "InsertInvitation" => {
            let Some(me) = actor else {
                return Some(Err(bad_args(req, "no character")));
            };
            let Some(target) = opt_id(req.args.first()) else {
                return Some(Err(bad_args(req, "characterID is not an id")));
            };
            let Some((corp, m)) = my_corp(reg, Some(me)) else {
                return Some(Err(refuse("CrpCantInvite")));
            };
            if m.roles & CORP_ROLE_PERSONNEL_MANAGER == 0 || reg.character(target).is_none() {
                return Some(Err(refuse("CrpCantInvite")));
            }
            let answer =
                insert_application(reg, target, corp, APP_INVITED_BY_CORPORATION, String::new());
            if answer.is_ok() {
                super::notifications::corp_events::corp_application(
                    reg,
                    me,
                    target,
                    corp,
                    None,
                    APP_INVITED_BY_CORPORATION,
                    "",
                    "",
                );
            }
            answer
        }
        "UpdateApplicationOffer" => update_application(reg, req),
        // `{characterID: [application, ...]}` for the caller's corp. The
        // client asks only with `corpRolePersonnelManager` **[V]**; asked
        // without it we answer what the client would have answered itself.
        "GetApplications" => {
            let mine = my_corp(reg, actor)
                .filter(|(_, m)| m.roles & CORP_ROLE_PERSONNEL_MANAGER != 0)
                .map(|(c, _)| c);
            Ok(grouped(
                reg.corporations
                    .applications
                    .values()
                    .filter(|a| {
                        Some(a.corporation_id) == mine && !APP_END_STATUSES.contains(&a.status)
                    })
                    .map(|a| (a.character_id, application_value(a)))
                    .collect(),
            ))
        }
        // Plain lists, filtered by `.status` (`GetMyOldApplicationsWithStatus`,
        // `GetOldApplicationsWithStatus`) **[V]**.
        "GetMyOldApplications" => Ok(PyValue::List(
            reg.corporations
                .applications
                .values()
                .filter(|a| Some(a.character_id) == actor && APP_END_STATUSES.contains(&a.status))
                .map(application_value)
                .collect(),
        )),
        "GetOldApplications" => {
            let mine = my_corp(reg, actor).map(|(c, _)| c);
            Ok(PyValue::List(
                reg.corporations
                    .applications
                    .values()
                    .filter(|a| {
                        Some(a.corporation_id) == mine && APP_END_STATUSES.contains(&a.status)
                    })
                    .map(application_value)
                    .collect(),
            ))
        }
        // `bco_applications.GetCorpWelcomeMail` caches behind `is None`, and
        // `welcomeMailWindow` reads `.welcomeMail`, `.characterID` and
        // `.changeDate` off it **[V]** -- `docs/lanes/world-corp.md` listed
        // the unserved arm as a live lock waiting for its window.
        "GetCorpWelcomeMail" => Ok(keyval(vec![
            ("welcomeMail", PyValue::Unicode(String::new())),
            ("characterID", PyValue::None),
            ("changeDate", PyValue::None),
        ])),
        // The notice board: `GetBulletins` (iterated, each row wrapped in a
        // `KeyVal`), `AddBulletin` (also the edit, by keyword),
        // `DeleteBulletin`, `UpdateBulletinOrder` **[V]**, in
        // `crate::bulletins` (SMALL-SOCIAL).
        "GetBulletins" | "AddBulletin" | "DeleteBulletin" | "UpdateBulletinOrder" => {
            let owner = req
                .actor
                .and_then(|a| reg.corporations.membership(a))
                .map(|m| m.corporation_id);
            crate::bulletins::call(reg, req, crate::bulletins::Scope::Corp, owner)
                .unwrap_or_else(|| Err(reg.no_method(req)))
        }
        // Truthiness tests on the corp Home tab for a player corp **[V]**
        // (`corp_ui_home`). `[A]` on the defaults: Tranquility's.
        "DoesMyCorpAcceptStructures" | "DoesCorpRestrictCorpMails" => Ok(PyValue::Bool(false)),
        // CEO succession and closing a corp (SOC-C3): [`resign_from_ceo`].
        "ResignFromCEO" => resign_from_ceo(reg, req),
        // `len(...)` of it, for the `AskResignAsCEO` prompt's
        // `memberCount` (`base_corporation.ResignFromCEO`) **[V]**: a list,
        // the same ids `ResignFromCEO(None)` answers.
        "GetNumberOfPotentialCEOs" => Ok(PyValue::List(
            actor
                .filter(|me| is_ceo(reg, *me))
                .and_then(|me| my_corp(reg, Some(me)))
                .map(|(corp, _)| {
                    crate::corp_roles::potential_ceos(
                        reg.corporations.get(corp).map_or(0, |c| c.ceo_id),
                        &candidates(reg, corp),
                    )
                })
                .unwrap_or_default()
                .into_iter()
                .map(PyValue::Int)
                .collect(),
        )),
        // Roles editing (SOC-C3): the edit-member dialog, the roles window
        // and the task-management tab.
        "UpdateMember" => update_member(reg, req),
        "UpdateMembers" => update_members(reg, req),
        "ExecuteActions" => execute_actions(reg, req),
        "GetMembersPaged" => members_paged(reg, req),
        // Titles, bulk expulsion, the member search and the logo (CORP-T).
        "GetTitles" => match get_titles(reg, actor) {
            Some(titles) => Ok(titles),
            None => return None,
        },
        "UpdateTitle" => update_title(reg, req),
        "UpdateTitles" => update_titles(reg, req),
        "DeleteTitle" => delete_title(reg, req),
        "KickOutMembers" => kick_out_members(reg, req),
        "GetMemberIDsByQuery" => member_ids_by_query(reg, req),
        "UpdateLogo" => update_logo(reg, req),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// SOC-C3: CEO succession, closing a corp, roles editing
// ---------------------------------------------------------------------------

/// Every member of a corp as [`crate::corp_roles`] sees them.
fn candidates(reg: &Registry, corporation_id: i64) -> Vec<crate::corp_roles::Candidate> {
    reg.corporations
        .members_of(corporation_id)
        .into_iter()
        .filter_map(|id| {
            // A waiting pilot is not handed a corp.
            if crate::start::delete_due(reg.deltas(), id).is_some() {
                return None;
            }
            let m = reg.corporations.membership(id)?;
            Some(crate::corp_roles::Candidate {
                character_id: id,
                director: m.roles & CORP_ROLE_DIRECTOR != 0,
                blocks_roles: m.block_roles,
                start_date: m.start_date,
            })
        })
        .collect()
}

fn character_name(reg: &Registry, character_id: i64) -> PyValue {
    PyValue::Unicode(
        reg.character(character_id)
            .map(|c| c.name.clone())
            .unwrap_or_default(),
    )
}

fn denied(reason: &str) -> ServiceError {
    refuse_with("CrpAccessDenied", vec![("reason", bstr(reason))])
}

/// `corpRegistry.ResignFromCEO(newCeoID)`, the client's two-step flow
/// (`base_corporation.ResignFromCEO`, `__ResignFromCEO`) **[V]**:
///
/// 1. Docked only (`CrpCanNotChangeCorpInSpace`) and CEO only
///    (`OnlyCEOCanResign`): both are the client's own prompts, raised here
///    for a caller that skipped them (a bot).
/// 2. `ResignFromCEO(None)` inside `PerformSessionChange('corp.resignceo')`.
///    A list answer is the successors to pick from: the client primes their
///    names, shows `SelectReplacementCEO`, and calls again with the pick.
///    `None` means done -- the corp closed.
/// 3. `ResignFromCEO(pick)`: the seat moves ([`hand_over`]).
///
/// Who may succeed, and when the corp closes instead, is
/// [`crate::corp_roles::resign`]. A corp in an alliance is never closed
/// (`CanNotDestroyCorpInAlliance`, the client's own refusal for a lone CEO
/// **[V]**): leave the alliance first, through C1's `allianceRegistry`. A
/// hand-over leaves the alliance untouched -- the executor is a corp, not a
/// CEO (`alliance.rs`).
///
/// The bots' corp mind (lane soc-c4) calls this the same way: `None`, then
/// the first id of the list, which is the longest-serving director when
/// there is one.
fn resign_from_ceo(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    use crate::corp_roles::{Resign, ResignRefused};
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let Some((corp, _)) = my_corp(reg, Some(actor)).filter(|_| is_ceo(reg, actor)) else {
        return Err(refuse("OnlyCEOCanResign"));
    };
    if reg.docked_station(actor).is_none() {
        return Err(refuse("CrpCanNotChangeCorpInSpace"));
    }
    let pick = opt_id(arg(req, 0, "newCeoID"));
    let in_alliance = crate::alliance::of_corp(reg.deltas(), corp).is_some();
    let outcome = crate::corp_roles::resign(actor, pick, &candidates(reg, corp), in_alliance)
        .map_err(|why| match why {
            ResignRefused::InAlliance => refuse("CanNotDestroyCorpInAlliance"),
            ResignRefused::NotAMember => denied("That pilot is not a member of your corporation."),
            ResignRefused::Unwilling => refuse_with(
                "CrpRolesDenied",
                vec![("member", character_name(reg, pick.unwrap_or_default()))],
            ),
        })?;
    match outcome {
        Resign::Candidates(ids) => Ok(PyValue::List(ids.into_iter().map(PyValue::Int).collect())),
        Resign::HandOver(to) => {
            hand_over(reg, corp, actor, to);
            Ok(PyValue::None)
        }
        Resign::Close => {
            close_corp(reg, corp);
            Ok(PyValue::None)
        }
    }
}

/// The seat moves from `from` to `to`.
///
/// - `ceoID` changes: `OnCorporationChanged(corpID, {'corporationID': (id,
///   id), 'ceoID': (old, new)})` to every member. The client's handler sets
///   the column on its cached corp row and, for `ceoID` in its own corp,
///   resets the corp window and reloads the buttons of whoever is in the
///   pair (`bco_corporations.OnCorporationChanged`) **[V]**. The key column
///   rides along as `(id, id)` because `GetAddRemoveFromChange` reads a
///   change whose every old value is `None` as an add **[V]** (`bco_base`).
/// - The new CEO gains [`CEO_ROLES`] on top of what they held **[A]**
///   (Tranquility's CEO holds every role): the member rows change
///   (`OnCorporationMemberChanged` with the masks) and their session gains
///   the roles.
/// - The old CEO keeps the roles they held **[A]**: they stay a director
///   until the new CEO says otherwise. Falsified by a Tranquility capture
///   where a resigning CEO's roles drop.
fn hand_over(reg: &mut Registry, corporation_id: i64, from: i64, to: i64) {
    if let Some(c) = reg.corporations.corps.get_mut(&corporation_id) {
        c.ceo_id = to;
    }
    reg.dirty = true;
    let members = reg.corporations.members_of(corporation_id);
    reg.push(CorporationChanged {
        to: members.clone(),
        corporation_id,
        change: vec![
            (
                "corporationID",
                PyValue::Int(corporation_id),
                PyValue::Int(corporation_id),
            ),
            ("ceoID", PyValue::Int(from), PyValue::Int(to)),
        ],
    });
    if let Some(m) = reg.corporations.direct(to) {
        let before = reg.corporations.session_of(&m);
        let mut new = m.masks();
        new[crate::corp_roles::ROLES] |= CEO_ROLES;
        store_member_masks(reg, corporation_id, to, &m, new);
        sync_session(reg, to, before);
    }
    super::notifications::corp_events::corp_new_ceo(reg, corporation_id, from, to);
}

/// Write a member's eight masks, tell the corp, and send the member the
/// session keys that moved.
fn store_member_masks(
    reg: &mut Registry,
    corporation_id: i64,
    character_id: i64,
    old: &Member,
    new: crate::corp_roles::Masks,
) {
    let mut m = *old;
    m.set_masks(&new);
    let mut change = Vec::new();
    for (i, name) in crate::corp_roles::MASK_NAMES.iter().enumerate() {
        let (was, now) = (old.masks()[i], new[i]);
        if was != now {
            change.push((*name, PyValue::Int(was), PyValue::Int(now)));
        }
    }
    if change.is_empty() {
        return;
    }
    reg.corporations.members.insert(character_id, m);
    reg.dirty = true;
    tell_member_change(reg, corporation_id, character_id, change);
}

/// Send a member the four session role keys if what they can use differs
/// from `before` (titles included, CORP-T). Call it after every change to
/// their masks, their title mask or a title they hold.
fn sync_session(reg: &mut Registry, character_id: i64, before: [i64; 4]) {
    let Some(m) = reg.corporations.direct(character_id) else {
        return;
    };
    let now = reg.corporations.session_of(&m);
    if now != before {
        reg.queue_session_change(character_id, roles_change(now));
    }
}

/// `OnCorporationMemberChanged(corpID, memberID, {column: (old, new)})` to
/// every member: `PagedCorpMembers` sets each column it has on its cached
/// row (`member[key] = new`) when `corporationID` is not in the change
/// **[V]** (`bco_members`).
fn tell_member_change(
    reg: &mut Registry,
    corporation_id: i64,
    member_id: i64,
    change: Vec<(&'static str, PyValue, PyValue)>,
) {
    let to = reg.corporations.members_of(corporation_id);
    if !to.is_empty() {
        reg.push(MemberFieldsChanged {
            to,
            corporation_id,
            member_id,
            change,
        });
    }
}

/// Close a corp: every member back to the NPC corp (the CEO last), its open
/// applications and invitations rejected, the row kept with `deleted` set.
///
/// The client's own warning says Tranquility "acquires" a closed corp's
/// assets ("all your corporation's assets will be acquired by the local
/// authorities", `CrpDestroyNonEmptyCorpWarning`) **[V]**, and the client has
/// no refusal for a balance (the only ones are alliance, structures,
/// contracts, industry jobs and freelance projects). **SEC-FIX 9 [A]**: the
/// wallet divisions are not left holding ISK nobody can reach: every
/// division's balance is paid out to the CEO, who is the last to leave, with
/// a journal row on both sides. Falsified by the owner wanting the ISK to
/// vanish as the warning says. The application rows end as
/// rejected-by-corporation (4) **[A]**.
fn close_corp(reg: &mut Registry, corporation_id: i64) {
    let ceo = reg.corporations.get(corporation_id).map(|c| c.ceo_id);
    if let Some(ceo) = ceo {
        for offset in 0..crate::corp_wallet::DIVISIONS as i64 {
            let key = crate::corp_wallet::MASTER_KEY + offset;
            let cents = crate::corp_wallet::balance(reg.deltas(), corporation_id, key);
            if cents <= 0 {
                continue;
            }
            let isk = crate::corp_wallet::to_isk(cents);
            let took = super::corp_wallet::transfer(
                reg,
                corporation_id,
                key,
                -isk,
                crate::corp_wallet::REF_CORPORATION_ACCOUNT_WITHDRAWAL,
                (corporation_id, ceo),
                ceo,
                false,
            );
            if took.is_some() {
                reg.charge(
                    ceo,
                    -isk,
                    crate::corp_wallet::REF_CORPORATION_ACCOUNT_WITHDRAWAL,
                    corporation_id,
                    ceo,
                );
            }
        }
    }
    let mut members = reg.corporations.members_of(corporation_id);
    members.sort_by_key(|id| Some(*id) == ceo);
    for id in members {
        move_character(reg, id, None);
    }
    let open: Vec<i64> = reg
        .corporations
        .applications
        .values()
        .filter(|a| a.corporation_id == corporation_id && !APP_END_STATUSES.contains(&a.status))
        .map(|a| a.application_id)
        .collect();
    for id in open {
        let Some(a) = reg.corporations.applications.get_mut(&id) else {
            continue;
        };
        a.status = APP_REJECTED_BY_CORPORATION;
        let a = a.clone();
        tell_application(reg, &a);
    }
    if let Some(c) = reg.corporations.corps.get_mut(&corporation_id) {
        c.deleted = true;
    }
    reg.corporations.title_book.clear_corp(corporation_id);
    reg.dirty = true;
    // Its wars end with it (C8, `ENDED_CORP_DELETED`).
    crate::war::on_corp_closed(reg, corporation_id);
}

/// Apply one member edit, checked by [`crate::corp_roles::check_edit`].
///
/// Refusals: `CrpRolesDenied {member}` for a member who blocks roles
/// **[V]** (key and text), `CrpAccessDenied {reason}` for the rest **[V]**
/// (key; the reasons are ours).
fn apply_edit(
    reg: &mut Registry,
    actor: i64,
    target: i64,
    edit: &crate::corp_roles::Edit,
) -> Result<(), ServiceError> {
    use crate::corp_roles::{Authority, Denied};
    let Some((corp, me)) = my_corp(reg, Some(actor)) else {
        return Err(denied("You are not in a player corporation."));
    };
    let Some(old) = reg
        .corporations
        .direct(target)
        .filter(|m| m.corporation_id == corp)
    else {
        return Err(denied("That pilot is not a member of your corporation."));
    };
    let session_before = reg.corporations.session_of(&old);
    // **[A]** a CEO is always the active CEO here: a CEO who dropped their
    // own director role could otherwise never take it back.
    let authority = if is_ceo(reg, actor) {
        Authority::ActiveCeo
    } else if me.roles & CORP_ROLE_DIRECTOR != 0 {
        Authority::Director
    } else {
        Authority::Member([
            me.grantable_roles,
            me.grantable_roles_at_hq,
            me.grantable_roles_at_base,
            me.grantable_roles_at_other,
        ])
    };
    crate::corp_roles::check_edit(
        actor,
        authority,
        target,
        is_ceo(reg, target),
        old.block_roles,
        &old.masks(),
        edit,
    )
    .map_err(|why| match why {
        Denied::BlocksRoles => refuse_with(
            "CrpRolesDenied",
            vec![("member", character_name(reg, target))],
        ),
        Denied::NotYourChoice => denied("Only a member can block their own roles."),
        Denied::CeoUntouchable => denied("Only the CEO can change the CEO's roles."),
        Denied::DirectorOnlyByCeo => denied("Only the CEO can grant or revoke Director."),
        Denied::NotGrantable => denied("You cannot grant that role."),
    })?;
    // Title, title mask and the block flag first: they never move the
    // session.
    let mut change: Vec<(&'static str, PyValue, PyValue)> = Vec::new();
    let mut m = old;
    if let Some(title) = edit.title.clone().map(|t| capped(t, MEMBER_TITLE_MAX_LEN)) {
        let title = &title;
        let was = reg.corporations.title(target).to_owned();
        if *title != was {
            if title.is_empty() {
                reg.corporations.titles.remove(&target);
            } else {
                reg.corporations.titles.insert(target, title.clone());
            }
            change.push((
                "title",
                PyValue::Unicode(was),
                PyValue::Unicode(title.clone()),
            ));
        }
    }
    if let Some(mask) = edit.title_mask.filter(|v| *v != old.title_mask) {
        change.push((
            "titleMask",
            PyValue::Int(old.title_mask),
            PyValue::Int(mask),
        ));
        m.title_mask = mask;
    }
    if let Some(block) = edit.block_roles.filter(|v| *v != old.block_roles) {
        change.push((
            "blockRoles",
            PyValue::Int(i64::from(old.block_roles)),
            PyValue::Int(i64::from(block)),
        ));
        m.block_roles = block;
    }
    if !change.is_empty() {
        reg.corporations.members.insert(target, m);
        reg.dirty = true;
        tell_member_change(reg, corp, target, change);
    }
    let new = edit.apply_masks(&m.masks());
    store_member_masks(reg, corp, target, &m, new);
    sync_session(reg, target, session_before);
    Ok(())
}

/// `UpdateMember(charID, title, divisionID, squadronID, roles,
/// grantableRoles, rolesAtHQ, grantableRolesAtHQ, rolesAtBase,
/// grantableRolesAtBase, rolesAtOther, grantableRolesAtOther, baseID,
/// titleMask, blockRoles)` -- fifteen positionals, re-passed by
/// `bco_members.UpdateMember` **[V]**. `None` is "unchanged": the edit
/// dialog sends `title` only for a CEO or equivalent and `titleMask` only
/// when it moved; `RemoveAllRoles` sends zeros and `blockRoles=1` **[V]**.
/// `divisionID`, `squadronID` and `baseID` are ignored **[A]**: no corp
/// here has divisions, squadrons or bases. The return is discarded.
fn update_member(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let target = opt_id(arg(req, 0, "charIDToUpdate"))
        .ok_or_else(|| bad_args(req, "charIDToUpdate is not an id"))?;
    let mut edit = crate::corp_roles::Edit {
        title: text_arg(arg(req, 1, "title")),
        title_mask: int_arg(arg(req, 13, "titleMask")),
        block_roles: int_arg(arg(req, 14, "blockRoles")).map(|v| v != 0),
        ..Default::default()
    };
    for (i, name) in crate::corp_roles::MASK_NAMES.iter().enumerate() {
        edit.masks[i] = int_arg(arg(req, 4 + i, name));
    }
    apply_edit(reg, actor, target, &edit)?;
    Ok(PyValue::None)
}

/// The columns and lines of a rowset the client sent: its `Rowset`
/// instance (`header`, `lines` in the instance dict), or a bare `(header,
/// lines)` pair.
fn sent_rowset(v: Option<&PyValue>) -> Option<(Vec<String>, Vec<Vec<PyValue>>)> {
    let (header, lines) = match v? {
        PyValue::Instance { state, .. } => (
            crate::pyutil::dict_get(state, b"header")?,
            crate::pyutil::dict_get(state, b"lines")?,
        ),
        PyValue::Tuple(parts) | PyValue::List(parts) if parts.len() == 2 => (&parts[0], &parts[1]),
        _ => return None,
    };
    let (PyValue::List(h) | PyValue::Tuple(h)) = header else {
        return None;
    };
    let names = h
        .iter()
        .map(|n| crate::pyutil::as_bytes(n).map(|b| String::from_utf8_lossy(b).into_owned()))
        .collect::<Option<Vec<String>>>()?;
    let (PyValue::List(ls) | PyValue::Tuple(ls)) = lines else {
        return None;
    };
    let rows = ls
        .iter()
        .map(|l| match l {
            PyValue::List(cells) | PyValue::Tuple(cells) => Some(cells.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some((names, rows))
}

/// `UpdateMembers(rows)`: the roles window's Save, a `Rowset` whose header
/// is `characterID, title, divisionID, squadronID`, the eight masks,
/// `baseID, titleMask` (`corp_ui_member_newroles.GetRowHeader`) **[V]**.
/// Its `title` is the int `0`, so it is never a title change; `titleMask`
/// is `None` when unchanged **[V]** (`_CreateRowsToUpdate`). Rows are
/// applied in order and the first refusal stops the rest.
fn update_members(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let (names, lines) =
        sent_rowset(arg(req, 0, "rows")).ok_or_else(|| bad_args(req, "rows is not a rowset"))?;
    let col = |line: &[PyValue], name: &str| -> Option<PyValue> {
        names
            .iter()
            .position(|n| n == name)
            .and_then(|i| line.get(i).cloned())
    };
    for line in &lines {
        let target = opt_id(col(line, "characterID").as_ref())
            .ok_or_else(|| bad_args(req, "a row without a characterID"))?;
        let mut edit = crate::corp_roles::Edit {
            title: text_arg(col(line, "title").as_ref()),
            title_mask: int_arg(col(line, "titleMask").as_ref()),
            ..Default::default()
        };
        for (i, name) in crate::corp_roles::MASK_NAMES.iter().enumerate() {
            edit.masks[i] = int_arg(col(line, name).as_ref());
        }
        apply_edit(reg, actor, target, &edit)?;
    }
    Ok(PyValue::None)
}

/// `appConst.CTV_ADD = 1`, `CTV_REMOVE = 2`, `CTV_SET = 3`, `CTV_GIVE = 4`
/// **[V]**. (`CTV_COMMS = 5` never reaches us: `bco_members.ExecuteActions`
/// filters it out.)
const CTV_ADD: i64 = 1;
const CTV_REMOVE: i64 = 2;
const CTV_SET: i64 = 3;
const CTV_GIVE: i64 = 4;
/// `appConst.CTPG_CASH = 6` and `CTPG_SHARES = 7` **[V]**: what a GIVE
/// action's property is (an integer, not a column name).
const CTPG_CASH: i64 = 6;
const CTPG_SHARES: i64 = 7;

/// One task-management action: `[verb, property, value]`.
struct Action {
    verb: i64,
    /// The column name for add, remove and set.
    name: String,
    /// The property as an integer: what a give action names.
    id: Option<i64>,
    int: Option<i64>,
    real: Option<f64>,
}

fn custom_info(text: &str) -> ServiceError {
    refuse_with("CustomInfo", vec![("info", bstr(text))])
}

/// `ExecuteActions(targetIDs, actions)`: the task-management tab's "Commit
/// actions on targets", each action `[verb, property, value]`
/// (`corp_ui_member_view_task_mgt.ExecuteActions`) **[V]**.
///
/// - **Add and remove** work on the eight masks and `titleMask` -- the
///   tab's `bitmaskablesList` **[V]** -- through the same checks as a
///   dialog edit.
/// - **Set `baseID`** is the tab's only set: the picker offers the corp's
///   offices plus "-" (`None`). This server has no offices, so clearing is
///   a no-op that succeeds and naming a station is refused **[A]**.
/// - **Give cash** pays each target the amount from the caller's active
///   wallet division, exactly as `account.GiveCashFromCorpAccount` does
///   (the take role, the balance and the journal rows are CORP-W's).
///   **Give shares** is refused: this server has no shares.
///
/// Everything refusable without a side effect is refused before the first
/// change. The return is discarded or passed through **[V]**; we answer
/// `None`.
fn execute_actions(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character"));
    };
    let ids: Vec<i64> = match arg(req, 0, "targetIDs") {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => {
            v.iter().filter_map(|x| opt_id(Some(x))).collect()
        }
        _ => return Err(bad_args(req, "targetIDs is not a list")),
    };
    let actions: Vec<Action> = match arg(req, 1, "actions") {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|a| match a {
                PyValue::List(p) | PyValue::Tuple(p) if p.len() == 3 => Some(Action {
                    verb: int_arg(p.first())?,
                    name: text_arg(p.get(1)).unwrap_or_default(),
                    id: int_arg(p.get(1)),
                    int: int_arg(p.get(2)),
                    real: real_arg(p.get(2)),
                }),
                _ => None,
            })
            .collect(),
        _ => return Err(bad_args(req, "actions is not a list")),
    };
    for a in &actions {
        match (a.verb, a.id) {
            (CTV_GIVE, Some(CTPG_SHARES)) => {
                return Err(custom_info("This server has no corporation shares."));
            }
            (CTV_GIVE, Some(CTPG_CASH)) => {
                if !a.real.is_some_and(|v| v.is_finite() && v > 0.0) {
                    return Err(custom_info("Give an amount of ISK above zero."));
                }
            }
            (CTV_GIVE, _) => return Err(custom_info("That cannot be given.")),
            (CTV_SET, _) if a.name == "baseID" && a.int.is_some_and(|v| v != 0) => {
                return Err(custom_info(
                    "The corporation has no office there to be a member's base.",
                ));
            }
            _ => {}
        }
    }
    for target in &ids {
        let Some(m) = reg.corporations.direct(*target) else {
            continue;
        };
        let mut masks = m.masks();
        let mut title_mask = m.title_mask;
        for a in &actions {
            let slot = if a.name == "titleMask" {
                &mut title_mask
            } else if let Some(i) = crate::corp_roles::mask_index(&a.name) {
                &mut masks[i]
            } else {
                continue;
            };
            let value = a.int.unwrap_or(0);
            match a.verb {
                CTV_ADD => *slot |= value,
                CTV_REMOVE => *slot &= !value,
                _ => {}
            }
        }
        let edit = crate::corp_roles::Edit {
            masks: masks.map(Some),
            title_mask: (title_mask != m.title_mask).then_some(title_mask),
            ..Default::default()
        };
        apply_edit(reg, actor, *target, &edit)?;
    }
    for a in actions
        .iter()
        .filter(|a| a.verb == CTV_GIVE && a.id == Some(CTPG_CASH))
    {
        for target in &ids {
            let key = crate::corp_wallet::active_key(reg.deltas(), actor);
            let args = [
                PyValue::Int(*target),
                PyValue::Real(a.real.unwrap_or(0.0)),
                PyValue::Int(key),
            ];
            let give = Request {
                service: "account",
                method: "GiveCashFromCorpAccount",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            };
            if let Some(answer) = super::corp_wallet::account(reg, &give) {
                answer?;
            }
        }
    }
    Ok(PyValue::None)
}

/// Members per page: `PagedResultSet`'s and `PagedCollection`'s default
/// `perPage=50` **[V]** (`pagedCollection.py`).
const MEMBERS_PER_PAGE: usize = 50;

/// `GetMembersPaged(page)`: the roles window's member list
/// (`corp_ui_member_newroles` -> `PagedCorpMembers.PopulatePage(1)`)
/// **[V]**. A `pagedCollection.PagedResultSet` -- new-style, in
/// `globalsWhitelist` -- whose `collection` are member rows like
/// `GetMember`'s, `totalCount` the corp's size, `page` the one asked for
/// (1-based: `PopulatePage` slices `perPage * (page - 1)`) **[V]**.
fn members_paged(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let page = int_arg(arg(req, 0, "page")).unwrap_or(1).max(1);
    let ids = my_corp(reg, req.actor)
        .map(|(corp, _)| reg.corporations.members_of(corp))
        .unwrap_or_default();
    let skip = usize::try_from(page - 1)
        .unwrap_or(0)
        .saturating_mul(MEMBERS_PER_PAGE);
    let rows: Vec<PyValue> = ids
        .iter()
        .skip(skip)
        .take(MEMBERS_PER_PAGE)
        .filter_map(|id| member_row(reg, *id))
        .collect();
    Ok(crate::pyutil::new_style(
        "eve.common.script.util.pagedCollection.PagedResultSet",
        vec![
            ("collection", PyValue::List(rows)),
            ("totalCount", PyValue::Int(ids.len() as i64)),
            ("page", PyValue::Int(page)),
            ("perPage", PyValue::Int(MEMBERS_PER_PAGE as i64)),
        ],
    ))
}

// ---------------------------------------------------------------------------
// CORP-T: titles, bulk expulsion, the member search, the logo, old CEOs
// ---------------------------------------------------------------------------

/// The corp of a CEO or a director (by the roles they can use, titles
/// included), or the refusal. Titles, the logo and the bulk expulsion are a
/// director's: the client offers their windows to `corpRoleDirector` only
/// (`CorpTitles.Load`, `IsCorpDirector`, `check_is_role_director`) **[V]**.
fn director_corp(reg: &Registry, actor: Option<i64>, what: &str) -> Result<i64, ServiceError> {
    let Some(actor) = actor else {
        return Err(denied("There is no character to act for."));
    };
    match reg.corporations.membership(actor) {
        Some(m) if m.roles & CORP_ROLE_DIRECTOR != 0 || is_ceo(reg, actor) => Ok(m.corporation_id),
        Some(_) => Err(denied(&format!("Only a director can {what}."))),
        None => Err(denied("You are not in a player corporation.")),
    }
}

/// One title as the `util.Row` the client keeps: `titleID`, `titleName`, the
/// eight masks **[V]** (`bco_titles`).
fn title_row(id: i64, t: &crate::corp_titles::Title) -> PyValue {
    let mut line = vec![PyValue::Int(id), PyValue::Unicode(t.name.clone())];
    line.extend(t.masks.into_iter().map(PyValue::Int));
    eve_rowset::Row::new(&crate::corp_titles::COLUMNS, line)
        .map(|r| r.to_pyvalue())
        .unwrap_or(PyValue::None)
}

/// `GetTitles()` for a member of a player corp: `{titleID: Row}` with all
/// sixteen slots. An unnamed slot has an empty name and the client names it
/// "Untitled n" itself (`bco_titles.GetTitles`) **[V]**. `None` outside a
/// player corp, so the NPC-era `{}` answers.
fn get_titles(reg: &Registry, actor: Option<i64>) -> Option<PyValue> {
    let (corp, _) = my_corp(reg, actor)?;
    Some(PyValue::Dict(
        reg.corporations
            .title_book
            .all(corp)
            .into_iter()
            .map(|(id, t)| (title_row(id, &t), PyValue::Int(id)))
            .collect(),
    ))
}

/// `OnTitleChanged(corpID, titleID, change)`, `change = {column: (old, new)}`
/// (`TitlesO.OnTitleChanged`) **[V]**. A slot is never added or removed: the
/// change holds only the columns that moved, and an old value is never
/// `None`, so the client reads it as an edit.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleChanged {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub title_id: i64,
    pub change: Vec<(&'static str, PyValue, PyValue)>,
}

impl crate::notify::Push for TitleChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnTitleChanged",
            vec![
                PyValue::Int(self.corporation_id),
                PyValue::Int(self.title_id),
                change_dict(&self.change),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Store a title, tell the corp, and give every holder the session keys
/// their new roles come to. With `clear_holders` the slot's bit is also
/// taken off every member (a deleted title).
fn apply_title(
    reg: &mut Registry,
    corp: i64,
    id: i64,
    new: crate::corp_titles::Title,
    clear_holders: bool,
) {
    let old = reg.corporations.title_book.get(corp, id);
    let holders: Vec<(i64, Member, [i64; 4])> = reg
        .corporations
        .members_of(corp)
        .into_iter()
        .filter_map(|c| {
            let m = reg.corporations.direct(c)?;
            (m.title_mask & id != 0).then(|| (c, m, reg.corporations.session_of(&m)))
        })
        .collect();
    if clear_holders {
        for (c, m, _) in &holders {
            let mut cleared = *m;
            cleared.title_mask &= !id;
            reg.corporations.members.insert(*c, cleared);
            tell_member_change(
                reg,
                corp,
                *c,
                vec![(
                    "titleMask",
                    PyValue::Int(m.title_mask),
                    PyValue::Int(cleared.title_mask),
                )],
            );
        }
    }
    if old != new {
        let mut change: Vec<(&'static str, PyValue, PyValue)> = Vec::new();
        if old.name != new.name {
            change.push((
                crate::corp_titles::COLUMNS[1],
                PyValue::Unicode(old.name.clone()),
                PyValue::Unicode(new.name.clone()),
            ));
        }
        for (i, name) in crate::corp_roles::MASK_NAMES.iter().enumerate() {
            if old.masks[i] != new.masks[i] {
                change.push((name, PyValue::Int(old.masks[i]), PyValue::Int(new.masks[i])));
            }
        }
        reg.corporations.title_book.set(corp, id, new);
        let to = reg.corporations.members_of(corp);
        reg.push(TitleChanged {
            to,
            corporation_id: corp,
            title_id: id,
            change,
        });
    }
    reg.dirty = true;
    for (c, _, before) in holders {
        sync_session(reg, c, before);
    }
}

/// A title edit read off the wire: the id, the name, the eight masks.
fn title_from_line(
    req: &Request<'_>,
    line: &[PyValue],
) -> Result<(i64, crate::corp_titles::Title), ServiceError> {
    let id = int_arg(line.first()).ok_or_else(|| bad_args(req, "titleID is not an id"))?;
    if !crate::corp_titles::is_title_id(id) {
        return Err(bad_args(req, "titleID is not one of the sixteen titles"));
    }
    let name = text_arg(line.get(1)).ok_or_else(|| bad_args(req, "titleName is not a string"))?;
    let mut masks = [0; 8];
    for (i, m) in masks.iter_mut().enumerate() {
        *m = int_arg(line.get(2 + i))
            .ok_or_else(|| bad_args(req, "a role mask is not an integer"))?;
    }
    Ok((id, crate::corp_titles::Title::sanitized(&name, masks)))
}

/// `UpdateTitle(titleID, titleName, roles, grantableRoles, rolesAtHQ,
/// grantableRolesAtHQ, rolesAtBase, grantableRolesAtBase, rolesAtOther,
/// grantableRolesAtOther)` **[V]** (`bco_titles.UpdateTitle`).
fn update_title(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp = director_corp(reg, req.actor, "edit titles")?;
    let (id, title) = title_from_line(req, req.args)?;
    apply_title(reg, corp, id, title, false);
    Ok(PyValue::None)
}

/// The lines of a rowset the client sent, whatever its header holds:
/// `CorpTitles.SaveChanges` builds `Rowset([10, 11, ..., 19])`, a header of
/// integers, so the columns are read by position **[V]**.
fn sent_lines(v: Option<&PyValue>) -> Option<Vec<Vec<PyValue>>> {
    let lines = match v? {
        PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, b"lines")?,
        PyValue::Tuple(parts) | PyValue::List(parts) if parts.len() == 2 => &parts[1],
        _ => return None,
    };
    let (PyValue::List(ls) | PyValue::Tuple(ls)) = lines else {
        return None;
    };
    ls.iter()
        .map(|l| match l {
            PyValue::List(cells) | PyValue::Tuple(cells) => Some(cells.clone()),
            _ => None,
        })
        .collect()
}

/// `UpdateTitles(rows)`: the title tab's Save, one line per changed title in
/// [`crate::corp_titles::COLUMNS`] order **[V]**. All lines are read before
/// any is applied.
fn update_titles(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp = director_corp(reg, req.actor, "edit titles")?;
    let lines =
        sent_lines(arg(req, 0, "titles")).ok_or_else(|| bad_args(req, "titles is not a rowset"))?;
    let mut edits = Vec::new();
    for line in &lines {
        edits.push(title_from_line(req, line)?);
    }
    for (id, title) in edits {
        apply_title(reg, corp, id, title, false);
    }
    Ok(PyValue::None)
}

/// `DeleteTitle(titleID)`: the slot goes back to unnamed and roleless, and
/// no member holds it any more. The client has no button for it
/// (`bco_titles.DeleteTitle` has no caller in the UI) **[V]**.
fn delete_title(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp = director_corp(reg, req.actor, "delete titles")?;
    let id =
        int_arg(arg(req, 0, "titleID")).ok_or_else(|| bad_args(req, "titleID is not an id"))?;
    if !crate::corp_titles::is_title_id(id) {
        return Err(bad_args(req, "titleID is not one of the sixteen titles"));
    }
    apply_title(reg, corp, id, crate::corp_titles::Title::default(), true);
    Ok(PyValue::None)
}

/// Expel one member: the shared tail of `KickOutMember` and
/// `KickOutMembers`.
fn expel(reg: &mut Registry, actor: i64, target: i64) -> Result<(), ServiceError> {
    can_be_kicked_out(reg, actor, target)?;
    let left = reg
        .corporations
        .membership(target)
        .map(|m| m.corporation_id);
    move_character(reg, target, None);
    if let Some(c) = left {
        super::notifications::corp_events::corp_member_left(reg, target, c, target != actor);
    }
    Ok(())
}

/// `KickOutMembers(characterIDs)`: the multi-select expel
/// (`KickOutMultipleMembers`) **[V]**. The answer is subscripted by
/// `'kicked'` and `'notKicked'`, each a list of character ids. The client
/// refuses a list holding the caller (`CorpCantKickSelfInMultiKick`) and a
/// non-director before asking; the server raises the same for a caller that
/// skipped it. A pilot who cannot be expelled (the CEO, a stranger) is
/// `notKicked`; the rest still go.
fn kick_out_members(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let actor = req.actor.ok_or_else(|| bad_args(req, "no character"))?;
    let ids: Vec<i64> = match arg(req, 0, "characterIds") {
        Some(PyValue::List(v) | PyValue::Tuple(v)) => {
            v.iter().filter_map(|x| opt_id(Some(x))).collect()
        }
        _ => return Err(bad_args(req, "characterIds is not a list")),
    };
    if ids.contains(&actor) {
        return Err(refuse("CorpCantKickSelfInMultiKick"));
    }
    let corp = director_corp(reg, Some(actor), "expel members")?;
    let (mut kicked, mut not_kicked) = (Vec::new(), Vec::new());
    for id in ids {
        if kicked.contains(&id) || not_kicked.contains(&id) {
            continue;
        }
        let mine = reg.corporations.membership(id).map(|m| m.corporation_id) == Some(corp);
        if mine && expel(reg, actor, id).is_ok() {
            kicked.push(id);
        } else {
            not_kicked.push(id);
        }
    }
    let list = |v: Vec<i64>| PyValue::List(v.into_iter().map(PyValue::Int).collect());
    Ok(crate::pyutil::pydict(vec![
        ("kicked", list(kicked)),
        ("notKicked", list(not_kicked)),
    ]))
}

/// A search value: an integer, or text.
fn query_value(v: &PyValue) -> Option<crate::corp_titles::Value> {
    use crate::corp_titles::Value;
    match v {
        PyValue::Int(n) => Some(Value::Int(*n)),
        other => text_arg(Some(other)).map(Value::Text),
    }
}

/// The search form's terms: `[property, operator, value]` first, then
/// `[join, property, operator, value]` **[V]** (`ExecuteQuery`).
fn query_clauses(
    req: &Request<'_>,
    v: Option<&PyValue>,
) -> Result<Vec<crate::corp_titles::Clause>, ServiceError> {
    let Some(PyValue::List(terms) | PyValue::Tuple(terms)) = v else {
        return Err(bad_args(req, "query is not a list"));
    };
    let mut out = Vec::new();
    for t in terms {
        let (PyValue::List(p) | PyValue::Tuple(p)) = t else {
            return Err(bad_args(req, "a query term is not a list"));
        };
        let (join, rest) = match p.len() {
            3 => (None, &p[..]),
            4 => (int_arg(p.first()), &p[1..]),
            _ => return Err(bad_args(req, "a query term has the wrong length")),
        };
        let (Some(property), Some(operator), Some(value)) = (
            text_arg(rest.first()),
            int_arg(rest.get(1)),
            rest.get(2).and_then(query_value),
        ) else {
            return Err(bad_args(req, "a query term is malformed"));
        };
        out.push(crate::corp_titles::Clause {
            join,
            property,
            operator,
            value,
        });
    }
    Ok(out)
}

/// `GetMemberIDsByQuery(query, includeImplied, searchTitles)`: the "find
/// member in role" search; the answer is a list of character ids that
/// `PopulateView` shows **[V]** (`corp_ui_member_find`). Scoped to the
/// caller's own corp **[A]**: any member may ask.
fn member_ids_by_query(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some((corp, _)) = my_corp(reg, req.actor) else {
        return Ok(PyValue::List(vec![]));
    };
    let query = query_clauses(req, arg(req, 0, "query"))?;
    let truthy = |v: Option<&PyValue>| int_arg(v).is_some_and(|n| n != 0);
    let opts = crate::corp_titles::Options {
        include_implied: truthy(arg(req, 1, "includeImplied")),
        search_titles: truthy(arg(req, 2, "searchTitles")),
    };
    let mut out = Vec::new();
    for id in reg.corporations.members_of(corp) {
        let (Some(direct), Some(effective)) =
            (reg.corporations.direct(id), reg.corporations.membership(id))
        else {
            continue;
        };
        let subject = crate::corp_titles::Subject {
            character_id: id,
            name: reg
                .character(id)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            direct: direct.masks(),
            from_titles: reg.corporations.title_book.held(corp, direct.title_mask),
            title_mask: direct.title_mask,
            start_date: direct.start_date,
            director: effective.roles & CORP_ROLE_DIRECTOR != 0 || is_ceo(reg, id),
        };
        if crate::corp_titles::matches(&subject, &query, opts) {
            out.push(PyValue::Int(id));
        }
    }
    Ok(PyValue::List(out))
}

/// `UpdateLogo(shape1, shape2, shape3, color1, color2, color3, typeface)`
/// from the corp-details form **[V]** (`CorpDetails.Submit`). A director
/// edits it. The form asks the pilot to accept `corpLogoChangeCost` first;
/// **[A]** the server charges nothing (the cost is not in this repository).
/// `typeface` is `None` in the form and is not stored. Every member's
/// cached corporation row is told with `OnCorporationChanged`
/// (`bco_corporations`: a logo column in the change reloads the logo).
fn update_logo(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let corp = director_corp(reg, req.actor, "change the logo")?;
    let value = |i: usize, name: &str| {
        let v = arg(req, i, name);
        v.map(|v| int_arg(Some(v)))
    };
    let mut new_shapes = [None; 3];
    let mut new_colors = [None; 3];
    for i in 0..3 {
        new_shapes[i] = value(i, &format!("shape{}", i + 1)).flatten();
        new_colors[i] = value(3 + i, &format!("color{}", i + 1)).flatten();
    }
    let Some(c) = reg.corporations.get_mut(corp) else {
        return Err(denied("You are not in a player corporation."));
    };
    const SHAPES: [&str; 3] = ["shape1", "shape2", "shape3"];
    const COLORS: [&str; 3] = ["color1", "color2", "color3"];
    let opt = |o: Option<i64>| o.map_or(PyValue::None, PyValue::Int);
    let mut change: Vec<(&'static str, PyValue, PyValue)> = Vec::new();
    for i in 0..3 {
        if c.shapes[i] != new_shapes[i] {
            change.push((SHAPES[i], opt(c.shapes[i]), opt(new_shapes[i])));
            c.shapes[i] = new_shapes[i];
        }
        if c.colors[i] != new_colors[i] {
            change.push((COLORS[i], opt(c.colors[i]), opt(new_colors[i])));
            c.colors[i] = new_colors[i];
        }
    }
    if !change.is_empty() {
        // A change whose every old value is `None` reads as an add
        // (`GetAddRemoveFromChange`), so the id rides along.
        change.insert(0, ("corporationID", PyValue::Int(corp), PyValue::Int(corp)));
        reg.dirty = true;
        let to = reg.corporations.members_of(corp);
        reg.push(CorporationChanged {
            to,
            corporation_id: corp,
            change,
        });
    }
    Ok(PyValue::None)
}

/// Give a CEO who lacks any of [`CEO_ROLES`] all of them (CORP-W added the
/// accountant and the seven take roles, so a CEO founded before it never
/// saw the Corporation wallet tab open). A CEO who chose "Remove all roles"
/// (`blockRoles`) is left alone **[A]**. `push` tells the corp's members;
/// the world load passes `false` because nobody is online. Returns whether
/// the CEO changed.
fn heal_one(reg: &mut Registry, character_id: i64, push: bool) -> bool {
    let Some(m) = reg.corporations.direct(character_id) else {
        return false;
    };
    let is_live_ceo = reg
        .corporations
        .get(m.corporation_id)
        .is_some_and(|c| !c.deleted && c.ceo_id == character_id);
    if !is_live_ceo || m.block_roles || m.roles & CEO_ROLES == CEO_ROLES {
        return false;
    }
    let mut new = m.masks();
    new[crate::corp_roles::ROLES] |= CEO_ROLES;
    if push {
        store_member_masks(reg, m.corporation_id, character_id, &m, new);
    } else {
        let mut healed = m;
        healed.set_masks(&new);
        reg.corporations.members.insert(character_id, healed);
        reg.dirty = true;
    }
    true
}

/// Login: heal the CEO who is signing in, before the login change reads
/// their session role keys ([`session_role_masks`]), so the change carries
/// the wallet roles. No session change is queued: the login change *is*
/// the session. The corp's other members hear `OnCorporationMemberChanged`.
pub(crate) fn heal_ceo_roles(reg: &mut Registry, character_id: i64) -> bool {
    heal_one(reg, character_id, true)
}

/// World load: heal every corp's CEO ([`heal_one`], silent).
fn heal_all_ceos(reg: &mut Registry) {
    let ceos: Vec<i64> = reg
        .corporations
        .corps
        .values()
        .filter(|c| !c.deleted)
        .map(|c| c.ceo_id)
        .collect();
    for ceo in ceos {
        heal_one(reg, ceo, false);
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

/// `OnCorporationApplicationChanged(corpID, applicantID, applicationID,
/// newApplication)` -- four positionals, `ApplicationsO.OnCorporationApplicationChanged`
/// **[V]**. Never with `newApplication = None`: the client's
/// `UpdateApplicationSet` appends the `None` itself on that branch, a bug
/// we do not need to reach.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplicationChanged {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub applicant_id: i64,
    pub application_id: i64,
    pub row: PyValue,
}

impl crate::notify::Push for ApplicationChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorporationApplicationChanged",
            vec![
                PyValue::Int(self.corporation_id),
                PyValue::Int(self.applicant_id),
                PyValue::Int(self.application_id),
                self.row.clone(),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnCorporationMemberChanged(corporationID, memberID, change)` with
/// `change = {'corporationID': (old, new)}` -- `bco_members` adds the id to
/// or drops it from `__memberIDs` on exactly that key, and
/// `PagedCorpMembers` fetches the newcomer with `GetMembersByIds` **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberChanged {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub member_id: i64,
    pub old_corporation_id: i64,
    pub new_corporation_id: i64,
}

impl crate::notify::Push for MemberChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorporationMemberChanged",
            vec![
                PyValue::Int(self.corporation_id),
                PyValue::Int(self.member_id),
                // `(value, key)`: `{'corporationID': (old, new)}`.
                PyValue::Dict(vec![(
                    PyValue::Tuple(vec![
                        PyValue::Int(self.old_corporation_id),
                        PyValue::Int(self.new_corporation_id),
                    ]),
                    bstr("corporationID"),
                )]),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnCorporationChanged(corpID, change)`, `change = {column: (old, new)}`
/// -- `bco_corporations.OnCorporationChanged` **[V]**. Sent when the CEO
/// changes (SOC-C3).
#[derive(Debug, Clone, PartialEq)]
pub struct CorporationChanged {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub change: Vec<(&'static str, PyValue, PyValue)>,
}

fn change_dict(change: &[(&'static str, PyValue, PyValue)]) -> PyValue {
    // `(value, key)`.
    PyValue::Dict(
        change
            .iter()
            .map(|(k, old, new)| (PyValue::Tuple(vec![old.clone(), new.clone()]), bstr(k)))
            .collect(),
    )
}

impl crate::notify::Push for CorporationChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorporationChanged",
            vec![PyValue::Int(self.corporation_id), change_dict(&self.change)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `OnCorporationMemberChanged(corpID, memberID, change)` for a member who
/// stays: roles, titles, `blockRoles` (SOC-C3). [`MemberChanged`] is the
/// join-or-leave form.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberFieldsChanged {
    pub to: Vec<i64>,
    pub corporation_id: i64,
    pub member_id: i64,
    pub change: Vec<(&'static str, PyValue, PyValue)>,
}

impl crate::notify::Push for MemberFieldsChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorporationMemberChanged",
            vec![
                PyValue::Int(self.corporation_id),
                PyValue::Int(self.member_id),
                change_dict(&self.change),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// DEV-MODE (`/devcorp`): put `actor` in the player corporation whose ticker
/// is `ticker`, founding it first (no fee, the actor its CEO) when none has
/// that ticker. The answer says what happened.
pub(crate) fn dev_join(reg: &mut Registry, actor: i64, ticker: &str) -> String {
    let existing = reg
        .corporations
        .corps
        .values()
        .find(|c| !c.deleted && c.ticker.eq_ignore_ascii_case(ticker))
        .map(|c| c.corporation_id);
    if let Some(id) = existing {
        move_character(reg, actor, Some((id, 0)));
        return format!("dev: joined corporation {id} [{ticker}]");
    }
    let corporation_id = reg.corporations.next_corp_id();
    let corp = Corporation {
        corporation_id,
        name: format!("Dev {ticker}"),
        ticker: ticker.to_owned(),
        description: String::new(),
        url: String::new(),
        tax_rate: 0.0,
        shapes: [None, None, None],
        colors: [None, None, None],
        recruiting: true,
        aggression: Aggression::founded(false),
        lp_tax_rate: 0.0,
        ceo_id: actor,
        creator_id: actor,
        create_date: crate::pyutil::filetime_now(),
        deleted: false,
    };
    reg.corporations.corps.insert(corporation_id, corp);
    move_character(reg, actor, Some((corporation_id, CEO_ROLES)));
    crate::corp_shares::found(reg.deltas_mut(), corporation_id, actor);
    format!("dev: founded corporation {corporation_id} [{ticker}]")
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;

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

    /// `corpRegistry.GetTitles` must break the live lock, and the assertion
    /// that does that is "not `None`".
    ///
    /// `bco_titles.py(72)` guards `self.titles` with `is None`
    /// (`COMPARE_OP 8`), stores our answer at `STORE_ATTR titles`, and two
    /// opcodes later reads `.itervalues()` off it. So `None` is stored
    /// first and raises second: the cache now holds the one value that
    /// fails its own guard, and every title scroll, title picker and
    /// `GetMyGrantableRoles` call re-issues the call. Nothing counts that
    /// but `evecap --calls`.
    ///
    /// The container check is secondary but real: `len()`, `in`, `.get()`,
    /// `.iteritems()`, `.itervalues()` and `titlesByID[1 << n]` are all
    /// used across the consumers, so a dict and not a list -- and the keys
    /// are ints, one bit of `member.titleMask` each.
    #[test]
    fn get_titles_latches_the_cache_and_is_a_mapping() {
        let mut reg = Registry::new(SandboxConfig::default());
        let answer = call(&mut reg, "corpRegistry", "GetTitles").unwrap();

        assert_ne!(
            answer,
            PyValue::None,
            "bco_titles.py(73) stores this and then reads .itervalues() off \
             it, so None poisons `self.titles` and the corp window re-asks \
             forever"
        );

        let PyValue::Dict(entries) = answer else {
            panic!(
                "GetTitles must be a mapping: corp_ui_member_listentry does \
                 `titlesByID[1 << columnNumber]` and bco_titles does \
                 `self.titles[title.titleID] = title`"
            )
        };
        // Dict entries are `(value, key)` in wire order -- see `PyValue::Dict`.
        for (_value, key) in &entries {
            assert!(
                matches!(key, PyValue::Int(_)),
                "a titleID is a single bit of member.titleMask, so an int"
            );
        }
    }

    /// The Shareholders panel calls `.itervalues()` on the first and the expel
    /// picker iterates the second, so neither may be `None` (S1, #158).
    #[test]
    fn the_shareholder_calls_are_empty_containers() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(
            call(&mut reg, "corpRegistry", "GetShareholders").unwrap(),
            PyValue::Dict(vec![])
        );
        assert_eq!(
            call(
                &mut reg,
                "corpRegistry",
                "GetMemberIDsWithMoreThanAvgShares"
            )
            .unwrap(),
            PyValue::List(vec![])
        );
    }

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
    /// Read a `util.Row`'s header and line back out of the marshal form.
    fn row_parts(v: &PyValue) -> (Vec<String>, Vec<PyValue>) {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("a util.Row is an ObjectEx, got {v:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("the reduce header is a tuple")
        };
        // `(callable, args)`, and the args carry `(header, line)`.
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("the ctor args are a tuple")
        };
        let names = match &args[0] {
            PyValue::List(v) | PyValue::Tuple(v) => v
                .iter()
                .map(|n| match n {
                    PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    other => panic!("a column name is a str: {other:?}"),
                })
                .collect(),
            other => panic!("the header is a sequence: {other:?}"),
        };
        let line = match &args[1] {
            PyValue::List(v) | PyValue::Tuple(v) => v.clone(),
            other => panic!("the line is a sequence: {other:?}"),
        };
        (names, line)
    }

    /// **The header is the specification, so the test is the header.**
    ///
    /// `Row.__getattr__` is `self.line[self.header.index(name)]`, so every
    /// name some panel reads must be present or that panel raises a
    /// `ValueError` at the moment it draws. Each name below is one the recon
    /// lane cited to a client read site.
    #[test]
    fn the_corporation_row_carries_every_name_the_client_reads() {
        let mut reg = Registry::new(SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "corpRegistry",
                method: "GetCorporation",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
        .expect("GetCorporation answers");
        assert_ne!(
            answer,
            PyValue::None,
            "`None` is the live lock: LoadCorporation raises on `.header` \
             before it can latch its cache, so all ~40 readers re-issue"
        );
        let (names, line) = row_parts(&answer);
        assert_eq!(
            names.len(),
            line.len(),
            "Row.__init__ raises ValueError on a mismatch"
        );
        for want in [
            "corporationID",
            "corporationName",
            "tickerName",
            "description",
            "url",
            "taxRate",
            "loyaltyPointTaxRate",
            "isRecruiting",
            "memberCount",
            "memberLimit",
            "shares",
            "ceoID",
            "creatorID",
            "stationID",
            "allianceID",
            "allowWar",
            "shape1",
            "color1",
            "division1",
            "division7",
            "walletDivision2",
            "walletDivision7",
        ] {
            assert!(
                names.iter().any(|n| n == want),
                "{want} is read by name in the client and is not in the header"
            );
        }
        let at = |name: &str| line[names.iter().position(|n| n == name).expect(name)].clone();
        // The key column of the `IndexRowset` the client builds from this.
        assert_eq!(
            at("corporationID"),
            PyValue::Int(crate::model::NPC_CORPORATION_ID)
        );
        // **Eighteen sites compare `ceoID` with `session.charid`.** The
        // player must not be the CEO of an NPC corporation.
        assert_ne!(
            at("ceoID"),
            PyValue::Int(actor),
            "the player would be handed corporation management"
        );
        let PyValue::Int(ceo) = at("ceoID") else {
            panic!("ceoID is an int")
        };
        assert!(
            ceo < 90_000_000,
            "idCheckers.IsSystemOrNPC is `id < 90000000`, and a CEO that \
             looks like a player is a character sheet we would be asked for"
        );
        // A float: it reaches `FormatNumeric`.
        assert!(matches!(at("taxRate"), PyValue::Real(_)));
        // The client's own `or` supplies every division name.
        assert_eq!(at("division1"), PyValue::None);
    }

    /// One corporation, one ticker, whichever panel asks.
    ///
    /// `config.GetMultiCorpTickerNamesEx` and this row both carry
    /// `tickerName`, and they are different code paths in different modules.
    /// Two literals would show two tickers for one corporation depending on
    /// which panel drew it -- the class of bug that is invisible in a test of
    /// either side alone.
    #[test]
    fn the_corporation_ticker_is_the_same_string_in_both_places() {
        let mut reg = Registry::new(SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "corpRegistry",
                method: "GetCorporation",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
        .expect("GetCorporation answers");
        let (names, line) = row_parts(&answer);
        let ticker = line[names
            .iter()
            .position(|n| n == "tickerName")
            .expect("ticker")]
        .clone();
        let from_config = reg.dispatch(&Request {
            service: "config",
            method: "GetMultiCorpTickerNamesEx",
            args: &[PyValue::List(vec![PyValue::Int(
                crate::model::NPC_CORPORATION_ID,
            )])],
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        });
        let rendered = format!("{from_config:?}");
        let PyValue::Bytes(t) = &ticker else {
            panic!("tickerName is a str, got {ticker:?}")
        };
        let t = String::from_utf8_lossy(t).into_owned();
        assert!(
            rendered.contains(&t),
            "GetCorporation says {t:?} and GetMultiCorpTickerNamesEx does not \
             carry it: {rendered}"
        );
    }

    /// The info-window data has to be an **attribute bag**, with all sixteen
    /// title slots present.
    ///
    /// `base_corporation.py:1048` does `STORE_ATTR title1` on what we send,
    /// which a `util.Row` (`__slots__`) and a `DBRow` both refuse -- so the
    /// family is forced, not chosen. And `title1..16` raise when **absent**
    /// while `None` is the correct *value* for a title nobody holds: the
    /// client substitutes "Untitled N" itself.
    #[test]
    fn the_info_window_data_is_a_keyval_with_all_sixteen_titles() {
        let mut reg = Registry::new(SandboxConfig::default());
        let actor = reg.config().characters[0].character_id;
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "corpRegistry",
                method: "GetInfoWindowDataForChar",
                args: &[PyValue::Int(actor)],
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
        .expect("GetInfoWindowDataForChar answers");
        let PyValue::Instance { class, state } = &answer else {
            panic!("a KeyVal is an old-style instance, got {answer:?}")
        };
        assert_eq!(class, crate::pyutil::KEYVAL_CLASS.as_bytes());
        let attr = |name: &[u8]| crate::pyutil::dict_get(state, name);
        assert_eq!(
            attr(b"corpID"),
            Some(&PyValue::Int(crate::model::NPC_CORPORATION_ID))
        );
        assert!(
            attr(b"allianceID").is_some(),
            "read unguarded in two windows"
        );
        assert!(attr(b"title").is_some());
        for n in 1..=16 {
            let name = format!("title{n}");
            assert_eq!(
                attr(name.as_bytes()),
                Some(&PyValue::None),
                "{name} must be present, and None is the honest value"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Player corporations
    // -----------------------------------------------------------------------

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;

    /// Two pilots docked in the default station, each able to pay for a
    /// corp.
    fn two() -> Registry {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            balance: 5_000_000.0,
            ..Character::default()
        };
        let bob = Character {
            character_id: BOB,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            balance: 5_000_000.0,
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        })
    }

    fn on(
        reg: &mut Registry,
        actor: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            },
        )
    }

    /// A keyword argument arrives as `(value, key)`, the order the codec
    /// decodes it in (tracker §39): `arg` must find it by name.
    #[test]
    fn a_keyword_argument_in_wire_order_is_read_by_name() {
        let kwargs = [(
            PyValue::Int(7),
            PyValue::Bytes(b"loyaltyPointTaxRate".to_vec()),
        )];
        let req = Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &[],
            kwargs: &kwargs,
            bind_params: None,
            actor: Some(ALICE),
        };
        assert_eq!(arg(&req, 9, "loyaltyPointTaxRate"), Some(&PyValue::Int(7)));
        assert_eq!(arg(&req, 9, "applicationsEnabled"), None);
    }

    fn uni(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    /// `bco_corporations.AddCorporation`'s fifteen positionals, as the create
    /// form sends them.
    fn found(
        reg: &mut Registry,
        actor: i64,
        name: &str,
        ticker: &str,
    ) -> Result<i64, ServiceError> {
        let args = [
            uni(name),
            uni(ticker),
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
        on(reg, actor, "corpRegistry", "AddCorporation", &args)?;
        Ok(reg.character(actor).expect("a pilot").corporation_id)
    }

    /// The message key of a raised `UserError`.
    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            panic!("not a raised exception: {e:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("an exception header is (class, args)")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's args are a tuple")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    /// GAP-QUICKFIX B4: the friendly-fire toggle is timed.
    #[test]
    fn the_friendly_fire_toggle_takes_effect_after_the_delay() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        assert!(!reg.corp_friendly_fire(corp));
        let t0 = crate::pyutil::filetime_now();
        let got = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "RegisterNewAggressionSettings",
            &[PyValue::Bool(true)],
        )
        .expect("the CEO may");
        let PyValue::Instance { class, state } = &got else {
            panic!("an AggressionSettings instance")
        };
        assert!(eve_rowset::is_whitelisted(class));
        let field = |k: &str| crate::pyutil::dict_get(state, k.as_bytes()).cloned();
        let delay = crate::game_clock::span(&reg, FF_CHANGE_DELAY);
        let Some(PyValue::Int(enable)) = field("_enableAfter") else {
            panic!("an enable time")
        };
        assert!(enable >= t0 + delay);
        assert_eq!(field("_disableAfter"), Some(PyValue::Int(0)));
        // Still illegal until the time comes; legal after it.
        assert!(!reg.corp_friendly_fire(corp));
        let a = reg.corporations.get(corp).expect("corp").aggression;
        assert!(a.legal_at(enable + 1));
        // A repeat of the pending request is a new timer only if the state
        // differs; asking for what is legal now changes nothing further.
        let again = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "RegisterNewAggressionSettings",
            &[PyValue::Bool(false)],
        )
        .expect("answered");
        let PyValue::Instance { .. } = again else {
            panic!("an instance")
        };
    }

    #[test]
    fn aggression_rule_is_the_later_passed_time() {
        let on_ = Aggression::founded(true);
        let off = Aggression::founded(false);
        assert!(on_.legal_at(5) && !off.legal_at(5));
        let to_on = Aggression {
            enable_after: Some(100),
            disable_after: Some(0),
        };
        assert!(!to_on.legal_at(99) && to_on.legal_at(100));
        let to_off = Aggression {
            enable_after: Some(0),
            disable_after: Some(100),
        };
        assert!(to_off.legal_at(99) && !to_off.legal_at(100));
        assert!(to_on.pending_at(99) && !to_on.pending_at(100));
    }

    /// SEC-FIX 7: a corporation name has no control characters or markup.
    #[test]
    fn a_corporation_name_with_markup_or_control_characters_is_refused() {
        let mut reg = two();
        for name in [
            "Tabs\tand\nnewlines",
            "<b>Bold</b>",
            "A&B Mining",
            "Say \"hi\"",
        ] {
            let e = found(&mut reg, ALICE, name, "BAD").expect_err(name);
            assert_eq!(key_of(&e), "CorpNameInvalidSomeChar", "{name:?}");
        }
    }

    /// SEC-FIX 4: the client's `maxLength`s hold on the server too.
    #[test]
    fn description_url_and_member_title_are_cut_to_the_clients_limits() {
        let mut reg = two();
        let long = |n: usize| uni(&"d".repeat(n));
        let args = [
            uni("Rock Botherers"),
            uni("ROCK"),
            long(1_000_000),
            long(1_000_000),
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
        on(&mut reg, ALICE, "corpRegistry", "AddCorporation", &args).expect("founded");
        let corp = reg.character(ALICE).expect("a pilot").corporation_id;
        let c = reg.corporations.get(corp).expect("the corp");
        assert_eq!(c.description.chars().count(), CORP_DESCRIPTION_MAX_LEN);
        assert_eq!(c.url.chars().count(), CORP_URL_MAX_LEN);
        // The CEO titles themselves: `UpdateMember`'s second positional.
        let mut args = vec![PyValue::Int(ALICE), long(1_000_000)];
        args.extend(std::iter::repeat_n(PyValue::None, 13));
        on(&mut reg, ALICE, "corpRegistry", "UpdateMember", &args).expect("titled");
        assert_eq!(
            reg.corporations.title(ALICE).chars().count(),
            MEMBER_TITLE_MAX_LEN
        );
    }

    fn pushes(reg: &mut Registry, event: &str) -> Vec<(Vec<i64>, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .filter(|(_, p)| p.to_push().0 == event)
            .map(|(_, p)| (p.audience().unwrap_or_default(), p.to_push().1))
            .collect()
    }

    fn attr(v: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("{name} is read by the client and is absent"))
            .clone()
    }

    /// **The id has to be one `idCheckers.IsPlayerCorporation` accepts on
    /// its own**: `minPlayerCorporation <= id <= maxPlayerCorporation`
    /// (98,000,000..98,999,999, `inventorycommon/const.py`). Outside it the
    /// client asks `cfg.eveowners` -- or, below 2,000,000, decides it is an
    /// NPC corp, and every `not IsNPC(session.corpid)` branch stays shut.
    #[test]
    fn a_new_corp_gets_an_id_the_client_calls_a_player_corporation() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        assert!(
            (98_000_000..=98_999_999).contains(&corp),
            "{corp} is outside idCheckers' player-corporation range"
        );
        let second = found(&mut reg, BOB, "Ore Feelings", "ORE").expect("founded");
        assert_ne!(corp, second, "two corps, two ids");
    }

    /// **The whole effect of `AddCorporation` is the session change**: its
    /// return value is discarded by `CreateCorp.Submit`. So: the fee
    /// (`corporationStartupCost`, the number the client's own gate compared
    /// the wallet with), the founder's corp, and `corpid` in the change --
    /// with `corprole` **absent**, because the client derives it from the
    /// `rolesAt*` it is sent and a sent one is overwritten.
    #[test]
    fn founding_charges_the_startup_cost_and_moves_the_founder_by_session_change() {
        let mut reg = two();
        let _ = reg.take_session_changes();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        let alice = reg.character(ALICE).expect("Alice");
        assert_eq!(alice.balance, 5_000_000.0 - 1_599_800.0);
        assert_eq!(alice.corporation_id, corp);
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1, "one change, for the founder");
        let (who, change) = &changes[0];
        assert_eq!(*who, ALICE);
        assert_eq!(
            crate::pyutil::pairs_get(change, b"corpid"),
            Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(corp)]))
        );
        assert!(crate::pyutil::pairs_get(change, b"rolesAtAll").is_some());
        assert!(
            crate::pyutil::pairs_get(change, b"corprole").is_none(),
            "corprole is derived by the client from rolesAt*"
        );
        // And `UserIsCEO` -- `GetCorporation().ceoID == session.charid` --
        // is now true for her and only her.
        let row = on(&mut reg, ALICE, "corpRegistry", "GetCorporation", &[]).expect("a row");
        let (names, line) = row_parts(&row);
        let at = |n: &str| line[names.iter().position(|x| x == n).expect(n)].clone();
        assert_eq!(at("corporationID"), PyValue::Int(corp));
        assert_eq!(at("ceoID"), PyValue::Int(ALICE));
        assert_eq!(at("corporationName"), bstr("Rock Botherers"));
        assert_eq!(at("tickerName"), bstr("ROCK"));
        assert!(reg.take_dirty(), "a corp is durable state");
    }

    /// Each refusal is a key the client already has a message for, and a
    /// refused call changes nothing -- not the wallet, not the corp.
    #[test]
    fn refused_foundings_say_why_and_change_nothing() {
        let mut reg = two();
        reg.character_mut(BOB).expect("Bob").balance = 1_000.0;
        let e = found(&mut reg, BOB, "Broke", "BRK").expect_err("cannot pay");
        assert_eq!(key_of(&e), "NotEnoughMoney");
        assert_eq!(reg.character(BOB).expect("Bob").balance, 1_000.0);
        assert_eq!(
            reg.character(BOB).expect("Bob").corporation_id,
            crate::model::NPC_CORPORATION_ID
        );

        found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        reg.character_mut(BOB).expect("Bob").balance = 5_000_000.0;
        let e = found(&mut reg, BOB, "rock botherers", "RB").expect_err("name taken");
        assert_eq!(key_of(&e), "CorpNameInvalidTaken");
        let e = found(&mut reg, BOB, "Other", "rock").expect_err("ticker taken");
        assert_eq!(key_of(&e), "CorpTickerNameInvalidTaken");
        let e = found(&mut reg, BOB, "Other", "TOOLONG").expect_err("six letters");
        assert_eq!(
            key_of(&e),
            "CorpTickerNameInvalid",
            "the form's maxLength is 5"
        );
        assert_eq!(reg.character(BOB).expect("Bob").balance, 5_000_000.0);

        // `CreateCorp.Submit` refuses outside a station itself; so do we.
        reg.character_mut(BOB).expect("Bob").station_id = None;
        let e = found(&mut reg, BOB, "Other", "OTH").expect_err("not docked");
        assert_eq!(key_of(&e), "CanOnlyCreateCorpInStation");
    }

    /// The Members tab's `locationID`: the station while docked, the system the
    /// pilot is in once undocked elsewhere (`docs/lanes/stale-system.md`).
    #[test]
    fn member_tracking_location_follows_the_pilot() {
        const STATION: i64 = 5_001;
        const HOME: i64 = 7_001;
        const NEXT: i64 = 7_002;
        let mut reg = two();
        found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        {
            let c = reg.character_mut(ALICE).expect("alice");
            c.station_id = Some(STATION);
            c.solar_system_id = Some(HOME);
        }
        let location = |reg: &Registry| {
            let PyValue::List(rows) = member_tracking(reg, Some(ALICE)) else {
                panic!("a list");
            };
            let (names, line) = row_parts(&rows[0]);
            let at = names
                .iter()
                .position(|n| n == "locationID")
                .expect("column");
            line[at].clone()
        };
        assert_eq!(location(&reg), PyValue::Int(STATION), "docked");
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                ALICE,
                100_000_001,
                NEXT,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        assert_eq!(location(&reg), PyValue::Int(NEXT), "in space elsewhere");
    }

    /// **Invite, accept, and both screens agree.**
    ///
    /// The founder invites (`InsertInvitation(charID)`, the character menu's
    /// "Send Corp Invite"); the invitee finds it in `GetMyApplications` under
    /// the corp's id with status 8 (`crpApplicationInvitedByCorporation`) and
    /// accepts by setting 2 (`crpApplicationAcceptedByCharacter`) -- the one
    /// button the client draws for `(6, 8)` in their own view. Joining moves
    /// their session, and the founder's member list hears
    /// `OnCorporationMemberChanged` with `{'corporationID': (old, new)}`.
    #[test]
    fn an_invited_pilot_accepts_and_joins() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();

        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "InsertInvitation",
            &[PyValue::Int(BOB)],
        )
        .expect("the founder may invite");
        let told = pushes(&mut reg, "OnCorporationApplicationChanged");
        assert_eq!(told.len(), 1);
        assert!(told[0].0.contains(&BOB), "the invitee hears of it");
        assert_eq!(
            told[0].1.len(),
            4,
            "(corpID, applicantID, applicationID, row)"
        );
        assert_eq!(told[0].1[0], PyValue::Int(corp));
        assert_eq!(told[0].1[1], PyValue::Int(BOB));

        let mine = on(&mut reg, BOB, "corpRegistry", "GetMyApplications", &[]).expect("a dict");
        let PyValue::Dict(entries) = &mine else {
            panic!("a dict keyed by corporationID: {mine:?}")
        };
        assert_eq!(entries.len(), 1);
        let (PyValue::List(apps), key) = &entries[0] else {
            panic!("each value is a list of applications")
        };
        assert_eq!(*key, PyValue::Int(corp), "keyed by corporationID");
        let app = &apps[0];
        assert_eq!(attr(app, "status"), PyValue::Int(8));
        assert_eq!(attr(app, "characterID"), PyValue::Int(BOB));
        let id = attr(app, "applicationID");

        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &[
                id,
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni(""),
                PyValue::Int(2),
            ],
        )
        .expect("the invitee accepts");
        assert_eq!(reg.character(BOB).expect("Bob").corporation_id, corp);
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, BOB);
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0].1, b"corpid"),
            Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(corp)]))
        );
        let pending = reg.take_pending();
        let member: Vec<_> = pending
            .iter()
            .filter(|(_, p)| p.to_push().0 == "OnCorporationMemberChanged")
            .collect();
        assert_eq!(member.len(), 1);
        assert_eq!(member[0].1.audience(), Some(vec![ALICE]));
        let args = member[0].1.to_push().1;
        assert_eq!(args[1], PyValue::Int(BOB));
        assert_eq!(
            crate::pyutil::dict_get(&args[2], b"corporationID"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(crate::model::NPC_CORPORATION_ID),
                PyValue::Int(corp)
            ]))
        );
        // The invitation is history now, not an open offer.
        let PyValue::Dict(open) =
            on(&mut reg, BOB, "corpRegistry", "GetMyApplications", &[]).expect("a dict")
        else {
            panic!("a dict")
        };
        assert!(open.is_empty(), "accepted is an end status");
    }

    /// The other direction: Bob applies, the corp accepts (6), and Bob still
    /// has to accept (2) -- the corp's acceptance is an offer, not a join.
    /// And neither side may make the other's move.
    #[test]
    fn an_application_needs_the_corp_and_then_the_applicant() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "InsertApplication",
            &[PyValue::Int(corp), uni("I have a Venture.")],
        )
        .expect("applied");
        let PyValue::Dict(theirs) =
            on(&mut reg, ALICE, "corpRegistry", "GetApplications", &[]).expect("a dict")
        else {
            panic!("a dict keyed by characterID")
        };
        assert_eq!(theirs[0].1, PyValue::Int(BOB), "keyed by characterID");
        let PyValue::List(apps) = &theirs[0].0 else {
            panic!("a list")
        };
        let id = attr(&apps[0], "applicationID");
        assert_eq!(attr(&apps[0], "status"), PyValue::Int(0));
        let offer = |status: i64| {
            vec![
                id.clone(),
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni("I have a Venture."),
                PyValue::Int(status),
            ]
        };

        let e = on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &offer(6),
        )
        .expect_err("an applicant cannot accept on the corp's behalf");
        assert_eq!(key_of(&e), "CrpAccessDenied");
        let e = on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &offer(2),
        )
        .expect_err("nor join before the corp has said yes");
        assert_eq!(key_of(&e), "CrpAccessDenied");

        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateApplicationOffer",
            &offer(6),
        )
        .expect("the corp accepts");
        assert_eq!(
            reg.character(BOB).expect("Bob").corporation_id,
            crate::model::NPC_CORPORATION_ID,
            "6 is an offer"
        );
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &offer(2),
        )
        .expect("the applicant accepts");
        assert_eq!(reg.character(BOB).expect("Bob").corporation_id, corp);
    }

    /// **The member list is `GetEveOwners`**, and each row is spliced into
    /// `cfg.eveowners.data` as `list(owner) + [None]` -- so it must iterate
    /// to exactly four values, `ownerID` first, and carry `.ownerID`. A
    /// `util.Row` does both. `isPilotInSameCorpAs`, which gates "Send Corp
    /// Invite" and "Expel", is membership of these ids.
    #[test]
    fn the_member_list_has_every_member_as_a_four_column_owner_row() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "InsertInvitation",
            &[PyValue::Int(BOB)],
        )
        .expect("invited");
        let mine = on(&mut reg, BOB, "corpRegistry", "GetMyApplications", &[]).expect("a dict");
        let PyValue::Dict(entries) = &mine else {
            panic!("a dict")
        };
        let PyValue::List(apps) = &entries[0].0 else {
            panic!("a list")
        };
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &[
                attr(&apps[0], "applicationID"),
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni(""),
                PyValue::Int(2),
            ],
        )
        .expect("joined");

        let PyValue::List(owners) =
            on(&mut reg, ALICE, "corpRegistry", "GetEveOwners", &[]).expect("a list")
        else {
            panic!("a list")
        };
        let mut ids = Vec::new();
        for o in &owners {
            let (names, line) = row_parts(o);
            assert_eq!(
                line.len(),
                4,
                "list(owner) + [None] must make cfg.eveowners' five"
            );
            assert_eq!(names[0], "ownerID");
            ids.push(line[0].clone());
        }
        assert_eq!(ids, vec![PyValue::Int(ALICE), PyValue::Int(BOB)]);

        // The Members tab: one row each, read by attribute.
        let PyValue::List(tracked) = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "GetMemberTrackingInfo",
            &[],
        )
        .expect("a list") else {
            panic!("a list")
        };
        assert_eq!(tracked.len(), 2);
        for row in &tracked {
            let (names, _) = row_parts(row);
            for want in [
                "characterID",
                "corporationID",
                "baseID",
                "locationID",
                "roles",
                "grantableRoles",
                "lastOnline",
                "title",
                "startDateTime",
                "shipTypeID",
                "factionID",
                "logonDateTime",
                "logoffDateTime",
            ] {
                assert!(
                    names.iter().any(|n| n == want),
                    "ShowMemberTracking reads {want}"
                );
            }
        }
        // And what everybody else's client learns: the corp has a name and a
        // ticker, and Bob's public info names his new corp.
        let owners = reg.dispatch(&Request {
            service: "config",
            method: "GetMultiOwnersEx",
            args: &[PyValue::List(vec![PyValue::Int(corp)])],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        });
        assert!(
            format!("{owners:?}").contains("Rock Botherers"),
            "{owners:?}"
        );
        let tickers = reg.dispatch(&Request {
            service: "config",
            method: "GetMultiCorpTickerNamesEx",
            args: &[PyValue::List(vec![PyValue::Int(corp)])],
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        });
        assert!(format!("{tickers:?}").contains("ROCK"), "{tickers:?}");
        let info = reg
            .dispatch(&Request {
                service: "charMgr",
                method: "GetPublicInfo",
                args: &[PyValue::Int(BOB)],
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            })
            .expect("public info");
        assert_eq!(attr(&info, "corporationID"), PyValue::Int(corp));
    }

    /// FW-2: a corp serving a militia gives a joiner `warfactionid` in the
    /// same session change as `corpid`, and takes it away on leaving.
    #[test]
    fn a_corp_in_fw_gives_a_joiner_warfactionid() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        reg.deltas_mut().put(
            super::facwar_mgr::CORP_KIND,
            vec![corp.to_string()],
            vec!["599101".to_owned(), "1".to_owned(), "0".to_owned()],
        );
        reg.take_session_changes();
        move_character(&mut reg, BOB, Some((corp, 0)));
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0].1, b"warfactionid"),
            Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(599_101)]))
        );
        move_character(&mut reg, BOB, None);
        let changes = reg.take_session_changes();
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0].1, b"warfactionid"),
            Some(&PyValue::Tuple(vec![PyValue::Int(599_101), PyValue::None]))
        );
    }

    /// Leaving: `KickOutMember(session.charid)` inside
    /// `PerformSessionChange('corp.kickself', ...)`. The member goes back to
    /// the NPC corp by session change; the CEO cannot go at all
    /// (`CrpCantQuitCEO`); and `CanLeaveCurrentCorporation` is the
    /// three-tuple `corp_ui_home` unpacks before it draws the Quit button.
    #[test]
    fn a_member_can_leave_and_the_ceo_cannot() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "InsertInvitation",
            &[PyValue::Int(BOB)],
        )
        .expect("invited");
        let mine = on(&mut reg, BOB, "corpRegistry", "GetMyApplications", &[]).expect("a dict");
        let PyValue::Dict(entries) = &mine else {
            panic!("a dict")
        };
        let PyValue::List(apps) = &entries[0].0 else {
            panic!("a list")
        };
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &[
                attr(&apps[0], "applicationID"),
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni(""),
                PyValue::Int(2),
            ],
        )
        .expect("joined");
        let _ = reg.take_session_changes();

        let PyValue::Tuple(verdict) = on(
            &mut reg,
            BOB,
            "corpRegistry",
            "CanLeaveCurrentCorporation",
            &[],
        )
        .expect("asked") else {
            panic!("(canLeave, error, errorDetails)")
        };
        assert_eq!(verdict.len(), 3);
        assert_eq!(verdict[0], PyValue::Bool(true));
        let PyValue::Tuple(ceo) = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "CanLeaveCurrentCorporation",
            &[],
        )
        .expect("asked") else {
            panic!("a tuple")
        };
        assert_eq!(ceo[0], PyValue::Bool(false));
        assert_eq!(ceo[1], bstr("CrpCantQuitCEO"));

        let e = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "KickOutMember",
            &[PyValue::Int(ALICE)],
        )
        .expect_err("the CEO cannot leave");
        assert_eq!(key_of(&e), "CrpCantQuitCEO");
        assert!(
            reg.take_session_changes().is_empty(),
            "a refusal changes no session"
        );

        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "KickOutMember",
            &[PyValue::Int(BOB)],
        )
        .expect("left");
        assert_eq!(
            reg.character(BOB).expect("Bob").corporation_id,
            crate::model::NPC_CORPORATION_ID
        );
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0].1, b"corpid"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(crate::model::NPC_CORPORATION_ID)
            ]))
        );
        let e = on(
            &mut reg,
            BOB,
            "corpRegistry",
            "KickOutMember",
            &[PyValue::Int(BOB)],
        )
        .expect_err("nobody leaves the NPC corp");
        assert_eq!(key_of(&e), "CrpCantQuitDefaultCorporation");
    }

    /// The corporation info window opens on `corpmgr.GetPublicInfo`, and the
    /// window assigns `corp_info.shares = ...` onto what we send -- so an
    /// attribute bag, with every name the header and the Attributes tab read.
    #[test]
    fn public_info_is_a_keyval_with_what_the_info_window_reads() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        let info = on(
            &mut reg,
            BOB,
            "corpmgr",
            "GetPublicInfo",
            &[PyValue::Int(corp)],
        )
        .expect("corpmgr is served");
        let PyValue::Instance { class, .. } = &info else {
            panic!("a KeyVal: the window stores .shares onto it")
        };
        assert_eq!(class, crate::pyutil::KEYVAL_CLASS.as_bytes());
        for name in [
            "corporationID",
            "ceoID",
            "creatorID",
            "allianceID",
            "description",
            "deleted",
            "stationID",
            "url",
            "shares",
        ] {
            let _ = attr(&info, name);
        }
        assert_eq!(attr(&info, "corporationID"), PyValue::Int(corp));
        assert_eq!(attr(&info, "ceoID"), PyValue::Int(ALICE));
        // Another corp's full row, for `GetCorporation(corpid)` when it is
        // not `session.corpid`: the same header as our own.
        let row = on(
            &mut reg,
            BOB,
            "corpmgr",
            "GetCorporations",
            &[PyValue::Int(corp)],
        )
        .expect("a row");
        let (names, line) = row_parts(&row);
        assert_eq!(names.len(), line.len());
        let own = on(&mut reg, BOB, "corpRegistry", "GetCorporation", &[]).expect("a row");
        assert_eq!(
            row_parts(&own).0,
            names,
            "LoadCorporation appends by its first header"
        );
    }

    /// **A member logs back in holding their corp**: the membership survives
    /// the world file, is made true of the character on load, and the login
    /// session change carries the roles -- non-zero for the founder, which is
    /// what lets the client's `IsPersonnelManager()` offer the invite menu
    /// again.
    #[test]
    fn a_corp_and_its_members_survive_the_world_file() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "InsertApplication",
            &[PyValue::Int(corp), uni("hi")],
        )
        .expect("applied");
        let dir = std::env::temp_dir().join(format!(
            "eve-corps-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("a directory to write in");
        let store = crate::store::FileStore::new(dir.join("world.txt"));
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
        let snap = crate::store::Store::load(&store)
            .expect("read the world")
            .expect("a world was written");
        let _ = std::fs::remove_dir_all(&dir);

        let mut fresh = two();
        assert_eq!(
            fresh.character(ALICE).expect("Alice").corporation_id,
            crate::model::NPC_CORPORATION_ID,
            "or this proves nothing"
        );
        fresh.apply_snapshot(snap);
        assert_eq!(fresh.corporations, reg.corporations);
        assert_eq!(fresh.character(ALICE).expect("Alice").corporation_id, corp);
        let login = fresh.character_change(ALICE).expect("a login change");
        let Some(PyValue::Tuple(pair)) = crate::pyutil::pairs_get(&login, b"rolesAtAll") else {
            panic!("rolesAtAll is in the login change")
        };
        assert_ne!(pair[1], PyValue::Int(0), "the founder's roles");
        assert_eq!(
            crate::pyutil::pairs_get(&login, b"corpid"),
            Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(corp)]))
        );
        // A corp founded after the restart does not reuse the id.
        fresh.character_mut(BOB).expect("Bob").balance = 5_000_000.0;
        let next = found(&mut fresh, BOB, "Ore Feelings", "ORE").expect("founded");
        assert!(next > corp);
    }

    #[test]
    fn corporations_round_trip_through_their_records() {
        let mut reg = two();
        // The name itself cannot hold a control character any more
        // (SEC-FIX 7); the application text still can.
        let corp = found(&mut reg, ALICE, "Tabs and newlines", "T-1").expect("founded");
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "InsertApplication",
            &[PyValue::Int(corp), uni("a\tb")],
        )
        .expect("applied");
        let mut text = String::new();
        reg.corporations.render(&mut text);
        let mut back = Corporations::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Corporations::RECORDS.contains(&f[0]), "{line}");
            back.parse_record(&f).expect("a record we wrote");
        }
        assert_eq!(back, reg.corporations);
        let mut empty = String::new();
        Corporations::default().render(&mut empty);
        assert!(empty.is_empty(), "an older world file reads back identical");
    }

    /// `OnCorporationApplicationChanged` goes to the applicant and to the
    /// corp's personnel managers **only**: the client's
    /// `UpdateApplications` stores into a cache that `GetApplications` leaves
    /// `None` for a member without the role, so telling them raises inside
    /// the notify chain.
    #[test]
    fn application_pushes_skip_members_who_cannot_see_applications() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        // Bob joins with no roles.
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "InsertInvitation",
            &[PyValue::Int(BOB)],
        )
        .expect("invited");
        let mine = on(&mut reg, BOB, "corpRegistry", "GetMyApplications", &[]).expect("a dict");
        let PyValue::Dict(entries) = &mine else {
            panic!("a dict")
        };
        let PyValue::List(apps) = &entries[0].0 else {
            panic!("a list")
        };
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &[
                attr(&apps[0], "applicationID"),
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni(""),
                PyValue::Int(2),
            ],
        )
        .expect("joined");
        // A third pilot applies.
        const CAROL: i64 = 90_000_003;
        reg.config.characters.push(Character {
            character_id: CAROL,
            name: "Carol".to_owned(),
            ship_id: 100_000_003,
            ..Character::default()
        });
        let _ = reg.take_pending();
        on(
            &mut reg,
            CAROL,
            "corpRegistry",
            "InsertApplication",
            &[PyValue::Int(corp), uni("")],
        )
        .expect("applied");
        let told = pushes(&mut reg, "OnCorporationApplicationChanged");
        assert_eq!(told.len(), 1);
        assert!(told[0].0.contains(&CAROL));
        assert!(told[0].0.contains(&ALICE), "the founder manages personnel");
        assert!(!told[0].0.contains(&BOB), "Bob has no personnel role");
    }

    // -----------------------------------------------------------------------
    // SOC-C3: succession, closing, roles
    // -----------------------------------------------------------------------

    /// Alice founds a corp and Bob joins it by invitation. The queues are
    /// drained.
    /// NOTIF-1's corp hooks fire from the real calls: the invitation tells
    /// the pilot, and a kick tells the pilot and the CEO.
    #[test]
    fn corp_calls_send_their_notifications() {
        use crate::service::notifications::corp_events::kind;
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let types = |reg: &Registry, who| -> Vec<i64> {
            crate::notifications::of(reg.deltas(), who)
                .iter()
                .map(|n| n.type_id)
                .collect()
        };
        assert!(types(&reg, BOB).contains(&kind::CORP_APP_INVITED));
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "KickOutMember",
            &[PyValue::Int(BOB)],
        )
        .expect("kicked");
        assert!(types(&reg, BOB).contains(&kind::CORP_KICKED));
        assert!(types(&reg, ALICE).contains(&kind::CHAR_LEFT_CORP));
    }

    fn alice_and_bob_in_one_corp() -> (Registry, i64) {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "InsertInvitation",
            &[PyValue::Int(BOB)],
        )
        .expect("invited");
        let id = reg
            .corporations
            .applications
            .values()
            .find(|a| a.character_id == BOB)
            .expect("the invitation")
            .application_id;
        on(
            &mut reg,
            BOB,
            "corpRegistry",
            "UpdateApplicationOffer",
            &[
                PyValue::Int(id),
                PyValue::Int(BOB),
                PyValue::Int(corp),
                uni(""),
                PyValue::Int(2),
            ],
        )
        .expect("accepted");
        assert_eq!(reg.character(BOB).expect("Bob").corporation_id, corp);
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        (reg, corp)
    }

    fn resign(reg: &mut Registry, actor: i64, pick: Option<i64>) -> Result<PyValue, ServiceError> {
        on(
            reg,
            actor,
            "corpRegistry",
            "ResignFromCEO",
            &[pick.map_or(PyValue::None, PyValue::Int)],
        )
    }

    /// `UpdateMember`'s fifteen positionals with only the masks, title mask
    /// and block flag given.
    fn update_member(
        reg: &mut Registry,
        actor: i64,
        target: i64,
        masks: [Option<i64>; 8],
        block: Option<bool>,
    ) -> Result<PyValue, ServiceError> {
        let mut args = vec![
            PyValue::Int(target),
            PyValue::None,
            PyValue::None,
            PyValue::None,
        ];
        args.extend(masks.iter().map(|m| m.map_or(PyValue::None, PyValue::Int)));
        args.push(PyValue::None);
        args.push(PyValue::None);
        args.push(block.map_or(PyValue::None, |b| PyValue::Int(i64::from(b))));
        on(reg, actor, "corpRegistry", "UpdateMember", &args)
    }

    fn roles_only(roles: i64) -> [Option<i64>; 8] {
        let mut m = [None; 8];
        m[0] = Some(roles);
        m
    }

    /// An invented role bit that is not director or personnel manager.
    const SOME_ROLE: i64 = 1 << 9;

    #[test]
    fn a_ceo_resigns_to_a_member_who_then_holds_the_seat_and_the_roles() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();

        // The prompt's count, and the first call's answer: the same ids.
        let count = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "GetNumberOfPotentialCEOs",
            &[],
        )
        .expect("a list");
        assert_eq!(count, PyValue::List(vec![PyValue::Int(BOB)]));
        let listed = resign(&mut reg, ALICE, None).expect("the candidates");
        assert_eq!(listed, PyValue::List(vec![PyValue::Int(BOB)]));
        assert_eq!(reg.corporations.get(corp).expect("corp").ceo_id, ALICE);
        assert!(
            reg.take_session_changes().is_empty(),
            "listing moves nothing"
        );

        // The pick.
        assert_eq!(
            resign(&mut reg, ALICE, Some(BOB)).expect("handed over"),
            PyValue::None
        );
        assert_eq!(reg.corporations.get(corp).expect("corp").ceo_id, BOB);
        let bob = reg.corporations.membership(BOB).expect("Bob is a member");
        assert_eq!(
            bob.roles & CEO_ROLES,
            CEO_ROLES,
            "the new CEO holds the CEO roles"
        );

        // Bob's session gains the roles without asking.
        let changes = reg.take_session_changes();
        let bobs: Vec<_> = changes.iter().filter(|(id, _)| *id == BOB).collect();
        assert_eq!(bobs.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(&bobs[0].1, b"rolesAtAll"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(bob.roles)
            ]))
        );

        // Both hear `ceoID` move, with the key column riding along.
        let told = pushes(&mut reg, "OnCorporationChanged");
        assert_eq!(told.len(), 1);
        assert!(told[0].0.contains(&ALICE) && told[0].0.contains(&BOB));
        assert_eq!(told[0].1[0], PyValue::Int(corp));
        let change = &told[0].1[1];
        assert_eq!(
            crate::pyutil::dict_get(change, b"ceoID"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(ALICE),
                PyValue::Int(BOB)
            ]))
        );
        assert_eq!(
            crate::pyutil::dict_get(change, b"corporationID"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(corp),
                PyValue::Int(corp)
            ]))
        );

        // The rows say so too, and Alice may now leave.
        let row = on(&mut reg, ALICE, "corpRegistry", "GetCorporation", &[]).expect("a row");
        let (names, line) = row_parts(&row);
        let ceo = names.iter().position(|n| n == "ceoID").expect("ceoID");
        assert_eq!(line[ceo], PyValue::Int(BOB));
        let leave = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "CanLeaveCurrentCorporation",
            &[],
        )
        .expect("a verdict");
        assert_eq!(
            leave,
            PyValue::Tuple(vec![PyValue::Bool(true), PyValue::None, PyValue::None])
        );
        // And Alice is no longer the CEO.
        assert_eq!(
            key_of(&resign(&mut reg, ALICE, None).unwrap_err()),
            "OnlyCEOCanResign"
        );
    }

    #[test]
    fn a_lone_ceo_resigning_closes_the_corp() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        let _ = reg.take_session_changes();
        assert_eq!(
            on(
                &mut reg,
                ALICE,
                "corpRegistry",
                "GetNumberOfPotentialCEOs",
                &[]
            )
            .expect("a list"),
            PyValue::List(vec![])
        );
        assert_eq!(
            resign(&mut reg, ALICE, None).expect("closed"),
            PyValue::None
        );

        // Alice is back in the NPC corp, by session change.
        assert_eq!(
            reg.character(ALICE).expect("Alice").corporation_id,
            crate::model::NPC_CORPORATION_ID
        );
        assert!(reg.corporations.membership(ALICE).is_none());
        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            crate::pyutil::pairs_get(&changes[0].1, b"corpid"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(crate::model::NPC_CORPORATION_ID)
            ]))
        );

        // The corp is closed, not forgotten: it keeps its name for owner
        // lookups, says `deleted`, takes no applications and is not listed.
        let c = reg.corporations.get(corp).expect("the row stays");
        assert!(c.deleted);
        assert!(!is_player_corp(&reg, corp));
        assert_eq!(
            player_corp_name(&reg, corp).as_deref(),
            Some("Rock Botherers")
        );
        assert_eq!(reg.corporations.all().count(), 0);
        let info = on(
            &mut reg,
            BOB,
            "corpmgr",
            "GetPublicInfo",
            &[PyValue::Int(corp)],
        )
        .expect("public info");
        assert_eq!(attr(&info, "deleted"), PyValue::Int(1));
        assert!(
            on(
                &mut reg,
                BOB,
                "corpRegistry",
                "InsertApplication",
                &[PyValue::Int(corp), uni("hi")]
            )
            .is_err(),
            "a closed corp takes no applications"
        );
        // Founding again gets a fresh id.
        let next = found(&mut reg, ALICE, "Ore Feelings", "ORE").expect("founded again");
        assert!(next > corp);
    }

    #[test]
    fn nobody_willing_closes_the_corp_members_and_all() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        // Bob removes his roles and blocks new ones ("Remove all roles").
        update_member(&mut reg, BOB, BOB, [Some(0); 8], Some(true)).expect("Bob may");
        assert!(reg.corporations.membership(BOB).expect("Bob").block_roles);
        let _ = reg.take_session_changes();

        // He cannot be picked...
        assert_eq!(
            key_of(&resign(&mut reg, ALICE, Some(BOB)).unwrap_err()),
            "CrpRolesDenied"
        );
        // ...and nobody else is willing, so the corp closes with him in it.
        assert_eq!(
            resign(&mut reg, ALICE, None).expect("closed"),
            PyValue::None
        );
        for who in [ALICE, BOB] {
            assert_eq!(
                reg.character(who).expect("a pilot").corporation_id,
                crate::model::NPC_CORPORATION_ID
            );
        }
        assert!(reg.corporations.get(corp).expect("row").deleted);
        let moved: Vec<i64> = reg
            .take_session_changes()
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(moved, vec![BOB, ALICE], "members first, the CEO last");
    }

    /// SEC-FIX 9: closing a corp pays every division's balance to the CEO
    /// instead of leaving it orphaned in the wallet deltas.
    #[test]
    fn closing_a_corp_pays_its_wallet_divisions_to_the_ceo() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        let key2 = crate::corp_wallet::MASTER_KEY + 2;
        for (key, isk) in [(crate::corp_wallet::MASTER_KEY, 1_000.0), (key2, 250.5)] {
            crate::corp_wallet::adjust(
                reg.deltas_mut(),
                corp,
                key,
                crate::corp_wallet::to_cents(isk),
            )
            .expect("a credit");
        }
        let before = reg.character(ALICE).expect("Alice").balance;
        assert_eq!(
            resign(&mut reg, ALICE, None).expect("closed"),
            PyValue::None
        );
        assert_eq!(crate::corp_wallet::total(reg.deltas(), corp), 0);
        let after = reg.character(ALICE).expect("Alice").balance;
        assert_eq!(after - before, 1_250.5, "the CEO got the whole balance");
        assert_eq!(
            crate::corp_wallet::all_rows(reg.deltas(), corp)
                .iter()
                .filter(|r| r.ref_type == crate::corp_wallet::REF_CORPORATION_ACCOUNT_WITHDRAWAL)
                .count(),
            2,
            "one corp journal row a division"
        );
    }

    #[test]
    fn a_corp_in_an_alliance_is_never_closed() {
        let mut reg = two();
        let corp = found(&mut reg, ALICE, "Rock Botherers", "ROCK").expect("founded");
        // CORP-W: the corporation's Master Wallet pays the fee.
        crate::corp_wallet::adjust(
            reg.deltas_mut(),
            corp,
            crate::corp_wallet::MASTER_KEY,
            crate::corp_wallet::to_cents(crate::alliance::ALLIANCE_CREATION_COST),
        )
        .expect("a credit");
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "CreateAlliance",
            &[uni("Invented Pact"), uni("INVP"), uni(""), uni("")],
        )
        .expect("an alliance");
        assert_eq!(
            key_of(&resign(&mut reg, ALICE, None).unwrap_err()),
            "CanNotDestroyCorpInAlliance"
        );
    }

    #[test]
    fn resigning_is_for_a_docked_ceo_and_a_pick_from_the_corp() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        assert_eq!(
            key_of(&resign(&mut reg, BOB, None).unwrap_err()),
            "OnlyCEOCanResign"
        );
        assert_eq!(
            key_of(&resign(&mut reg, ALICE, Some(ALICE)).unwrap_err()),
            "CrpAccessDenied"
        );
        assert_eq!(
            key_of(&resign(&mut reg, ALICE, Some(90_000_777)).unwrap_err()),
            "CrpAccessDenied"
        );
    }

    #[test]
    fn the_ceo_grants_roles_and_the_member_row_and_session_follow() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        let mut masks = roles_only(SOME_ROLE);
        masks[6] = Some(SOME_ROLE); // rolesAtOther
        update_member(&mut reg, ALICE, BOB, masks, None).expect("the CEO grants");
        let bob = reg.corporations.membership(BOB).expect("Bob");
        assert_eq!((bob.roles, bob.roles_at_other), (SOME_ROLE, SOME_ROLE));

        let changes = reg.take_session_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, BOB);
        for (key, want) in [
            ("rolesAtAll", SOME_ROLE),
            ("rolesAtOther", SOME_ROLE),
            ("rolesAtHQ", 0),
        ] {
            assert_eq!(
                crate::pyutil::pairs_get(&changes[0].1, key.as_bytes()),
                Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(want)])),
                "{key}"
            );
        }
        let told = pushes(&mut reg, "OnCorporationMemberChanged");
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].1[0], PyValue::Int(corp));
        assert_eq!(told[0].1[1], PyValue::Int(BOB));
        assert_eq!(
            crate::pyutil::dict_get(&told[0].1[2], b"roles"),
            Some(&PyValue::Tuple(vec![
                PyValue::Int(0),
                PyValue::Int(SOME_ROLE)
            ]))
        );
        assert!(crate::pyutil::dict_get(&told[0].1[2], b"corporationID").is_none());

        // The member row carries it.
        let row = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "GetMember",
            &[PyValue::Int(BOB)],
        )
        .expect("a row");
        let (names, line) = row_parts(&row);
        let at = |n: &str| line[names.iter().position(|x| x == n).expect(n)].clone();
        assert_eq!(at("roles"), PyValue::Int(SOME_ROLE));
        assert_eq!(at("rolesAtOther"), PyValue::Int(SOME_ROLE));
        assert_eq!(at("blockRoles"), PyValue::Int(0));

        // The login change carries all four masks.
        let login = reg.character_change(BOB).expect("a login change");
        assert_eq!(
            crate::pyutil::pairs_get(&login, b"rolesAtOther"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(SOME_ROLE)
            ]))
        );
    }

    #[test]
    fn role_edits_follow_the_clients_grant_rules() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        // A plain member grants nothing.
        assert_eq!(
            key_of(&update_member(&mut reg, BOB, ALICE, roles_only(SOME_ROLE), None).unwrap_err()),
            "CrpAccessDenied"
        );
        // Bob made director; a director still cannot touch the CEO, nor
        // take director from himself... he can drop his own roles, though.
        update_member(&mut reg, ALICE, BOB, roles_only(CORP_ROLE_DIRECTOR), None)
            .expect("director");
        assert_eq!(
            key_of(&update_member(&mut reg, BOB, ALICE, roles_only(0), None).unwrap_err()),
            "CrpAccessDenied"
        );
        update_member(&mut reg, BOB, BOB, roles_only(0), None).expect("dropping your own");
        assert_eq!(reg.corporations.membership(BOB).expect("Bob").roles, 0);
        // Somebody who blocks roles is told why.
        update_member(&mut reg, BOB, BOB, [None; 8], Some(true)).expect("blocking your own");
        assert_eq!(
            key_of(&update_member(&mut reg, ALICE, BOB, roles_only(SOME_ROLE), None).unwrap_err()),
            "CrpRolesDenied"
        );
        // Nobody blocks for somebody else.
        assert_eq!(
            key_of(&update_member(&mut reg, ALICE, BOB, [None; 8], Some(false)).unwrap_err()),
            "CrpAccessDenied"
        );
    }

    /// The roles window's Save: a `Rowset` with `GetRowHeader`'s fourteen
    /// columns, `title` the int 0 and `titleMask` `None`.
    #[test]
    fn update_members_reads_the_roles_windows_rowset() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let header: Vec<PyValue> = [
            "characterID",
            "title",
            "divisionID",
            "squadronID",
            "roles",
            "grantableRoles",
            "rolesAtHQ",
            "grantableRolesAtHQ",
            "rolesAtBase",
            "grantableRolesAtBase",
            "rolesAtOther",
            "grantableRolesAtOther",
            "baseID",
            "titleMask",
        ]
        .iter()
        .map(|n| bstr(n))
        .collect();
        let mut line = vec![
            PyValue::Int(BOB),
            PyValue::Int(0),
            PyValue::Int(0),
            PyValue::Int(0),
        ];
        line.extend([SOME_ROLE, SOME_ROLE, 0, 0, 0, 0, 0, 0].map(PyValue::Int));
        line.extend([PyValue::None, PyValue::None]);
        let rows = PyValue::Instance {
            class: b"eve.common.script.sys.rowset.Rowset".to_vec(),
            state: Box::new(crate::pyutil::pydict(vec![
                ("header", PyValue::List(header)),
                ("lines", PyValue::List(vec![PyValue::List(line)])),
            ])),
        };
        on(&mut reg, ALICE, "corpRegistry", "UpdateMembers", &[rows]).expect("saved");
        let bob = reg.corporations.membership(BOB).expect("Bob");
        assert_eq!((bob.roles, bob.grantable_roles), (SOME_ROLE, SOME_ROLE));
        assert_eq!(reg.corporations.title(BOB), "", "an int title is no title");
    }

    #[test]
    fn task_management_adds_and_removes_roles_on_every_target() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let act = |verb: i64, value: i64| {
            PyValue::List(vec![PyValue::Int(verb), bstr("roles"), PyValue::Int(value)])
        };
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "ExecuteActions",
            &[
                PyValue::List(vec![PyValue::Int(BOB), PyValue::Int(ALICE)]),
                PyValue::List(vec![act(CTV_ADD, SOME_ROLE)]),
            ],
        )
        .expect("added");
        assert_eq!(
            reg.corporations.membership(BOB).expect("Bob").roles,
            SOME_ROLE
        );
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "ExecuteActions",
            &[
                PyValue::List(vec![PyValue::Int(BOB)]),
                PyValue::List(vec![act(CTV_REMOVE, SOME_ROLE)]),
            ],
        )
        .expect("removed");
        assert_eq!(reg.corporations.membership(BOB).expect("Bob").roles, 0);
        let give = PyValue::List(vec![
            PyValue::Int(CTV_GIVE),
            PyValue::Int(6),
            PyValue::Int(1),
        ]);
        assert!(
            on(
                &mut reg,
                ALICE,
                "corpRegistry",
                "ExecuteActions",
                &[
                    PyValue::List(vec![PyValue::Int(BOB)]),
                    PyValue::List(vec![give])
                ],
            )
            .is_err()
        );
    }

    /// A page number from the client far past the end is an empty page,
    /// never an overflow (review of SOC-C3).
    #[test]
    fn members_paged_takes_any_page_number() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        for page in [i64::MAX, 1 << 62, 0, -5] {
            on(
                &mut reg,
                ALICE,
                "corpRegistry",
                "GetMembersPaged",
                &[PyValue::Int(page)],
            )
            .expect("a page");
        }
    }

    /// `PagedCorpMembers.PopulatePage(1)` -> a `PagedResultSet` whose
    /// `collection` is member rows.
    #[test]
    fn members_paged_is_a_paged_result_set_of_member_rows() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let page = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "GetMembersPaged",
            &[PyValue::Int(1)],
        )
        .expect("a page");
        let PyValue::ObjectEx { header, .. } = &page else {
            panic!("a new-style instance: {page:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("((cls,), state)")
        };
        assert_eq!(
            parts[0],
            PyValue::Tuple(vec![PyValue::Global(
                b"eve.common.script.util.pagedCollection.PagedResultSet".to_vec()
            )])
        );
        let state = &parts[1];
        assert_eq!(
            crate::pyutil::dict_get(state, b"totalCount"),
            Some(&PyValue::Int(2))
        );
        assert_eq!(
            crate::pyutil::dict_get(state, b"page"),
            Some(&PyValue::Int(1))
        );
        let Some(PyValue::List(rows)) = crate::pyutil::dict_get(state, b"collection") else {
            panic!("a list collection")
        };
        assert_eq!(rows.len(), 2);
        let (names, _) = row_parts(&rows[0]);
        assert!(names.iter().any(|n| n == "grantableRolesAtOther"));
    }

    #[test]
    fn role_masks_titles_and_closed_corps_survive_the_records() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        let mut masks = [Some(0); 8];
        masks[3] = Some(SOME_ROLE);
        update_member(&mut reg, ALICE, BOB, masks, None).expect("granted");
        reg.corporations
            .titles
            .insert(BOB, "Rock\tWrangler".to_owned());
        update_member(&mut reg, BOB, BOB, [None; 8], Some(true)).expect("blocked");
        let mut text = String::new();
        reg.corporations.render(&mut text);
        let mut back = Corporations::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            back.parse_record(&f).expect("a record we wrote");
        }
        assert_eq!(back, reg.corporations);

        // A line from before SOC-C3 still reads, with the extras at rest.
        let mut old = Corporations::default();
        old.parse_record(&["corpmember", "90000003", &corp.to_string(), "1", "0"])
            .expect("an older member line");
        let m = old.membership(90_000_003).expect("read");
        assert_eq!((m.roles, m.grantable_roles, m.block_roles), (1, 0, false));
    }

    // -----------------------------------------------------------------------
    // CORP-T: titles, bulk expulsion, the member search, the logo, old CEOs
    // -----------------------------------------------------------------------

    /// An invented role bit that no real role uses.
    const TITLE_ROLE: i64 = 1 << 41;
    const OTHER_TITLE_ROLE: i64 = 1 << 42;

    /// `UpdateTitle`'s ten positionals.
    fn update_title_call(
        reg: &mut Registry,
        actor: i64,
        id: i64,
        name: &str,
        masks: [i64; 8],
    ) -> Result<PyValue, ServiceError> {
        let mut args = vec![PyValue::Int(id), uni(name)];
        args.extend(masks.map(PyValue::Int));
        on(reg, actor, "corpRegistry", "UpdateTitle", &args)
    }

    fn masks_of(roles: i64) -> [i64; 8] {
        let mut m = [0; 8];
        m[0] = roles;
        m
    }

    /// `UpdateMember` with only the title mask given.
    fn set_title_mask(
        reg: &mut Registry,
        actor: i64,
        target: i64,
        mask: i64,
    ) -> Result<PyValue, ServiceError> {
        let mut args = vec![PyValue::Int(target)];
        args.extend(std::iter::repeat_n(PyValue::None, 12));
        args.push(PyValue::Int(mask));
        args.push(PyValue::None);
        on(reg, actor, "corpRegistry", "UpdateMember", &args)
    }

    fn session_roles_sent(reg: &mut Registry, who: i64) -> Vec<i64> {
        reg.take_session_changes()
            .into_iter()
            .filter(|(c, _)| *c == who)
            .filter_map(
                |(_, change)| match crate::pyutil::pairs_get(&change, b"rolesAtAll")? {
                    PyValue::Tuple(p) => int_arg(p.get(1)),
                    _ => None,
                },
            )
            .collect()
    }

    fn titles_answer(reg: &mut Registry, who: i64) -> Vec<(i64, Vec<String>, Vec<PyValue>)> {
        let PyValue::Dict(entries) = on(reg, who, "corpRegistry", "GetTitles", &[]).unwrap() else {
            panic!("a mapping")
        };
        entries
            .iter()
            .map(|(row, key)| {
                let (names, line) = row_parts(row);
                (int_arg(Some(key)).expect("an int key"), names, line)
            })
            .collect()
    }

    #[test]
    fn a_corp_has_sixteen_unnamed_title_rows_and_the_npc_corp_none() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let rows = titles_answer(&mut reg, BOB);
        assert_eq!(rows.len(), 16);
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        assert_eq!(ids, (0..16).map(|n| 1i64 << n).collect::<Vec<_>>());
        let (_, names, line) = &rows[2];
        assert_eq!(
            names.as_slice(),
            [
                "titleID",
                "titleName",
                "roles",
                "grantableRoles",
                "rolesAtHQ",
                "grantableRolesAtHQ",
                "rolesAtBase",
                "grantableRolesAtBase",
                "rolesAtOther",
                "grantableRolesAtOther"
            ]
        );
        assert_eq!(line[0], PyValue::Int(4));
        assert_eq!(line[1], uni(""), "the client names an unnamed title");
        // A pilot in no player corp gets the NPC-era empty mapping.
        let mut lone = two();
        assert_eq!(
            on(&mut lone, ALICE, "corpRegistry", "GetTitles", &[]).unwrap(),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn a_director_names_a_title_and_its_holders_gain_its_roles() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        set_title_mask(&mut reg, ALICE, BOB, 4).expect("Bob holds title 3");
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        update_title_call(&mut reg, ALICE, 4, "Hauler", masks_of(TITLE_ROLE)).expect("named");
        // The row, the push, and Bob's session.
        let rows = titles_answer(&mut reg, BOB);
        let (_, _, line) = rows.iter().find(|r| r.0 == 4).unwrap();
        assert_eq!(line[1], uni("Hauler"));
        assert_eq!(line[2], PyValue::Int(TITLE_ROLE));
        let told = pushes(&mut reg, "OnTitleChanged");
        assert_eq!(told.len(), 1);
        assert!(told[0].0.contains(&ALICE) && told[0].0.contains(&BOB));
        assert_eq!(told[0].1[0], PyValue::Int(corp));
        assert_eq!(told[0].1[1], PyValue::Int(4));
        let PyValue::Dict(change) = &told[0].1[2] else {
            panic!("a change dict")
        };
        // `(value, key)`; the name and the one mask that moved, nothing else.
        let keys: Vec<&PyValue> = change.iter().map(|(_, k)| k).collect();
        assert_eq!(keys, [&bstr("titleName"), &bstr("roles")]);
        assert_eq!(
            change[0].0,
            PyValue::Tuple(vec![uni(""), uni("Hauler")]),
            "an old value is never None, so the client reads an edit"
        );
        assert_eq!(session_roles_sent(&mut reg, BOB), vec![TITLE_ROLE]);
        // The member row keeps the direct roles; permission checks see both.
        let bob = reg.corporations.direct(BOB).unwrap();
        assert_eq!(bob.roles, 0);
        assert_eq!(reg.corporations.membership(BOB).unwrap().roles, TITLE_ROLE);
        assert_eq!(
            session_role_masks(&reg, BOB)[0],
            TITLE_ROLE,
            "the login change carries them"
        );
        // Changing the title's roles moves the holder's session again.
        update_title_call(&mut reg, ALICE, 4, "Hauler", masks_of(OTHER_TITLE_ROLE)).unwrap();
        assert_eq!(session_roles_sent(&mut reg, BOB), vec![OTHER_TITLE_ROLE]);
        // Taking the title off drops them.
        set_title_mask(&mut reg, ALICE, BOB, 0).unwrap();
        assert_eq!(session_roles_sent(&mut reg, BOB), vec![0]);
        // A title nobody holds moves no session.
        update_title_call(&mut reg, ALICE, 4, "Hauler", masks_of(TITLE_ROLE)).unwrap();
        assert!(session_roles_sent(&mut reg, BOB).is_empty());
    }

    #[test]
    fn a_title_cannot_carry_director_and_only_a_director_edits_titles() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        update_title_call(
            &mut reg,
            ALICE,
            1,
            "Boss",
            masks_of(CORP_ROLE_DIRECTOR | TITLE_ROLE),
        )
        .unwrap();
        let rows = titles_answer(&mut reg, BOB);
        assert_eq!(rows[0].2[2], PyValue::Int(TITLE_ROLE), "no director bit");
        assert_eq!(
            key_of(&update_title_call(&mut reg, BOB, 1, "Mine", masks_of(0)).unwrap_err()),
            "CrpAccessDenied"
        );
        assert!(matches!(
            update_title_call(&mut reg, ALICE, 3, "Two bits", masks_of(0)),
            Err(ServiceError::BadArguments { .. })
        ));
        assert!(matches!(
            update_title_call(&mut reg, ALICE, 1 << 16, "Too far", masks_of(0)),
            Err(ServiceError::BadArguments { .. })
        ));
        // A director who is not the CEO may edit, and holding the director
        // role through a title is not enough to make one: no director bit.
        update_member(&mut reg, ALICE, BOB, roles_only(CORP_ROLE_DIRECTOR), None).unwrap();
        update_title_call(&mut reg, BOB, 2, "By Bob", masks_of(0)).expect("a director");
    }

    #[test]
    fn update_titles_reads_the_title_tabs_positional_rowset() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        // `Rowset([10, ..., 19])`: an integer header, read by position.
        let header: Vec<PyValue> = (10..20).map(PyValue::Int).collect();
        let mut one = vec![PyValue::Int(1), uni("Miner")];
        one.extend(masks_of(TITLE_ROLE).map(PyValue::Int));
        let mut two_ = vec![PyValue::Int(8), uni("Scout")];
        two_.extend(masks_of(OTHER_TITLE_ROLE).map(PyValue::Int));
        let rows = |lines: Vec<Vec<PyValue>>| PyValue::Instance {
            class: b"eve.common.script.sys.rowset.Rowset".to_vec(),
            state: Box::new(crate::pyutil::pydict(vec![
                ("header", PyValue::List(header.clone())),
                (
                    "lines",
                    PyValue::List(lines.into_iter().map(PyValue::List).collect()),
                ),
            ])),
        };
        // One bad line stops the whole save.
        let mut bad = one.clone();
        bad[0] = PyValue::Int(5);
        assert!(
            on(
                &mut reg,
                ALICE,
                "corpRegistry",
                "UpdateTitles",
                &[rows(vec![one.clone(), bad])],
            )
            .is_err()
        );
        assert!(reg.corporations.title_book.is_empty());
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "UpdateTitles",
            &[rows(vec![one, two_])],
        )
        .expect("saved");
        let answer = titles_answer(&mut reg, ALICE);
        assert_eq!(answer[0].2[1], uni("Miner"));
        assert_eq!(answer[3].2[1], uni("Scout"));
        assert_eq!(answer[3].2[2], PyValue::Int(OTHER_TITLE_ROLE));
        assert_eq!(pushes(&mut reg, "OnTitleChanged").len(), 2);
    }

    #[test]
    fn deleting_a_title_resets_the_slot_and_takes_it_off_its_holders() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        update_title_call(&mut reg, ALICE, 2, "Gone", masks_of(TITLE_ROLE)).unwrap();
        set_title_mask(&mut reg, ALICE, BOB, 2 | 8).unwrap();
        let _ = reg.take_session_changes();
        let _ = reg.take_pending();
        on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "DeleteTitle",
            &[PyValue::Int(2)],
        )
        .expect("deleted");
        let rows = titles_answer(&mut reg, BOB);
        assert_eq!(rows[1].2[1], uni(""));
        assert_eq!(rows[1].2[2], PyValue::Int(0));
        assert_eq!(reg.corporations.direct(BOB).unwrap().title_mask, 8);
        assert_eq!(session_roles_sent(&mut reg, BOB), vec![0]);
        assert_eq!(pushes(&mut reg, "OnTitleChanged").len(), 1);
        assert_eq!(
            key_of(
                &on(
                    &mut reg,
                    BOB,
                    "corpRegistry",
                    "DeleteTitle",
                    &[PyValue::Int(8)]
                )
                .unwrap_err()
            ),
            "CrpAccessDenied"
        );
    }

    #[test]
    fn a_title_holder_can_use_the_roles_their_title_grants() {
        // The wallet reads `membership()`: a title with the director bit
        // would be refused above, so use the personnel role (invitations).
        let (mut reg, _) = alice_and_bob_in_one_corp();
        let invite = |reg: &mut Registry| {
            on(
                reg,
                BOB,
                "corpRegistry",
                "InsertInvitation",
                &[PyValue::Int(ALICE)],
            )
        };
        assert_eq!(key_of(&invite(&mut reg).unwrap_err()), "CrpCantInvite");
        update_title_call(
            &mut reg,
            ALICE,
            1,
            "HR",
            masks_of(CORP_ROLE_PERSONNEL_MANAGER),
        )
        .unwrap();
        set_title_mask(&mut reg, ALICE, BOB, 1).unwrap();
        // Alice is already in the corp, so the invitation is refused for a
        // different reason: the personnel check passed.
        let e = invite(&mut reg).unwrap_err();
        assert_ne!(key_of(&e), "CrpCantInvite");
    }

    #[test]
    fn the_info_window_names_the_titles_a_pilot_holds() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        update_title_call(&mut reg, ALICE, 2, "Scout", masks_of(0)).unwrap();
        set_title_mask(&mut reg, ALICE, BOB, 2 | 4).unwrap();
        let info = on(
            &mut reg,
            ALICE,
            "corpRegistry",
            "GetInfoWindowDataForChar",
            &[PyValue::Int(BOB)],
        )
        .unwrap();
        assert_eq!(attr(&info, "title2"), uni("Scout"));
        assert_eq!(attr(&info, "title3"), uni(""), "held but unnamed");
        assert_eq!(attr(&info, "title1"), PyValue::None);
    }

    #[test]
    fn titles_and_the_member_search_survive_the_records() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        update_title_call(&mut reg, ALICE, 2, "Tab\tTitle", masks_of(TITLE_ROLE)).unwrap();
        let mut text = String::new();
        reg.corporations.render(&mut text);
        assert!(text.lines().any(|l| l.starts_with("corptitle\t")));
        let mut back = Corporations::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            back.parse_record(&f).expect("a record we wrote");
        }
        assert_eq!(back, reg.corporations);
        assert_eq!(back.title_book.get(corp, 2).name, "Tab\tTitle");
        // A closed corp drops its titles.
        close_corp(&mut reg, corp);
        assert!(reg.corporations.title_book.is_empty());
    }

    fn kick_many(reg: &mut Registry, actor: i64, ids: &[i64]) -> Result<PyValue, ServiceError> {
        on(
            reg,
            actor,
            "corpRegistry",
            "KickOutMembers",
            &[PyValue::List(
                ids.iter().map(|i| PyValue::Int(*i)).collect(),
            )],
        )
    }

    fn ids_at(v: &PyValue, key: &str) -> Vec<i64> {
        let PyValue::Dict(entries) = v else {
            panic!("a dict, got {v:?}")
        };
        let (PyValue::List(items), _) = entries
            .iter()
            .find(|(_, k)| *k == bstr(key))
            .unwrap_or_else(|| panic!("{key} is subscripted by the client"))
        else {
            panic!("a list")
        };
        items.iter().filter_map(|x| int_arg(Some(x))).collect()
    }

    #[test]
    fn kicking_out_many_expels_those_who_can_go_and_reports_the_rest() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        const STRANGER: i64 = 90_000_099;
        // A plain member may not, whoever they name.
        assert_eq!(
            key_of(&kick_many(&mut reg, BOB, &[ALICE]).unwrap_err()),
            "CrpAccessDenied"
        );
        // The caller in the list is the client's own refusal.
        assert_eq!(
            key_of(&kick_many(&mut reg, ALICE, &[ALICE, BOB]).unwrap_err()),
            "CorpCantKickSelfInMultiKick"
        );
        // A director cannot expel the CEO; a stranger is not kicked either.
        update_member(&mut reg, ALICE, BOB, roles_only(CORP_ROLE_DIRECTOR), None).unwrap();
        let a = kick_many(&mut reg, BOB, &[ALICE, STRANGER, ALICE]).unwrap();
        assert!(ids_at(&a, "kicked").is_empty());
        assert_eq!(ids_at(&a, "notKicked"), vec![ALICE, STRANGER]);
        // The CEO expels Bob; the corp keeps one member.
        let _ = reg.take_pending();
        let a = kick_many(&mut reg, ALICE, &[BOB, STRANGER]).unwrap();
        assert_eq!(ids_at(&a, "kicked"), vec![BOB]);
        assert_eq!(ids_at(&a, "notKicked"), vec![STRANGER]);
        assert!(reg.corporations.membership(BOB).is_none());
        assert_eq!(reg.corporations.members_of(corp), vec![ALICE]);
        assert_eq!(
            reg.character(BOB).unwrap().corporation_id,
            crate::model::NPC_CORPORATION_ID
        );
    }

    fn query(
        reg: &mut Registry,
        actor: i64,
        terms: Vec<PyValue>,
        implied: i64,
        titles: i64,
    ) -> Vec<i64> {
        let PyValue::List(ids) = on(
            reg,
            actor,
            "corpRegistry",
            "GetMemberIDsByQuery",
            &[
                PyValue::List(terms),
                PyValue::Int(implied),
                PyValue::Int(titles),
            ],
        )
        .unwrap() else {
            panic!("a list of ids")
        };
        ids.iter().filter_map(|x| int_arg(Some(x))).collect()
    }

    fn term(items: Vec<PyValue>) -> PyValue {
        PyValue::List(items)
    }

    #[test]
    fn the_member_search_answers_roles_titles_names_and_joins() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        update_title_call(&mut reg, ALICE, 4, "Hauler", masks_of(TITLE_ROLE)).unwrap();
        set_title_mask(&mut reg, ALICE, BOB, 4).unwrap();
        update_member(&mut reg, ALICE, BOB, roles_only(SOME_ROLE), None).unwrap();
        let role = |r: i64| term(vec![bstr("roles"), PyValue::Int(7), PyValue::Int(r)]);
        // Direct roles only, then with the title's.
        assert_eq!(
            query(&mut reg, ALICE, vec![role(SOME_ROLE)], 0, 0),
            vec![BOB]
        );
        assert!(query(&mut reg, ALICE, vec![role(TITLE_ROLE)], 0, 0).is_empty());
        assert_eq!(
            query(&mut reg, ALICE, vec![role(TITLE_ROLE)], 0, 1),
            vec![BOB]
        );
        // "Include implied": the CEO and directors count as holding all.
        assert_eq!(
            query(&mut reg, ALICE, vec![role(TITLE_ROLE)], 1, 0),
            vec![ALICE]
        );
        // Names, titles, and a join.
        let name = |op: i64, t: &str| term(vec![bstr("characterID"), PyValue::Int(op), uni(t)]);
        assert_eq!(query(&mut reg, ALICE, vec![name(9, "ob")], 0, 0), vec![BOB]);
        let title = term(vec![bstr("titleMask"), PyValue::Int(7), PyValue::Int(4)]);
        assert_eq!(query(&mut reg, BOB, vec![title.clone()], 0, 0), vec![BOB]);
        let or_alice = term(vec![
            PyValue::Int(1),
            bstr("characterID"),
            PyValue::Int(11),
            uni("alice"),
        ]);
        assert_eq!(
            query(&mut reg, ALICE, vec![title.clone(), or_alice], 0, 0),
            vec![ALICE, BOB]
        );
        let and_alice = term(vec![
            PyValue::Int(2),
            bstr("characterID"),
            PyValue::Int(11),
            uni("alice"),
        ]);
        assert!(query(&mut reg, ALICE, vec![title, and_alice], 0, 0).is_empty());
        // A malformed term is a bad call, not an empty answer.
        assert!(
            on(
                &mut reg,
                ALICE,
                "corpRegistry",
                "GetMemberIDsByQuery",
                &[
                    PyValue::List(vec![term(vec![PyValue::Int(1)])]),
                    PyValue::Int(0),
                    PyValue::Int(0)
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn a_director_changes_the_logo_and_the_corp_hears_of_it() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        let logo = |reg: &mut Registry, who: i64, shape: i64| {
            on(
                reg,
                who,
                "corpRegistry",
                "UpdateLogo",
                &[
                    PyValue::Int(shape),
                    PyValue::None,
                    PyValue::None,
                    PyValue::Int(7),
                    PyValue::None,
                    PyValue::None,
                    PyValue::None,
                ],
            )
        };
        assert_eq!(
            key_of(&logo(&mut reg, BOB, 3).unwrap_err()),
            "CrpAccessDenied"
        );
        logo(&mut reg, ALICE, 3).expect("the CEO");
        let c = reg.corporations.get(corp).unwrap();
        assert_eq!(
            (c.shapes, c.colors),
            ([Some(3), None, None], [Some(7), None, None])
        );
        let told = pushes(&mut reg, "OnCorporationChanged");
        assert_eq!(told.len(), 1);
        assert!(told[0].0.contains(&BOB));
        let PyValue::Dict(change) = &told[0].1[1] else {
            panic!("a change")
        };
        let keys: Vec<&PyValue> = change.iter().map(|(_, k)| k).collect();
        assert_eq!(
            keys,
            [&bstr("corporationID"), &bstr("shape1"), &bstr("color1")]
        );
        // The same logo again changes nothing and says nothing.
        logo(&mut reg, ALICE, 3).unwrap();
        assert!(pushes(&mut reg, "OnCorporationChanged").is_empty());
    }

    #[test]
    fn task_management_gives_cash_from_the_corp_and_refuses_what_it_cannot_do() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        crate::corp_wallet::adjust(reg.deltas_mut(), corp, 1000, 1_000_000).expect("funded");
        let before = reg.character(BOB).unwrap().balance;
        let give = |reg: &mut Registry, actor: i64, property: i64, value: PyValue| {
            on(
                reg,
                actor,
                "corpRegistry",
                "ExecuteActions",
                &[
                    PyValue::List(vec![PyValue::Int(BOB)]),
                    PyValue::List(vec![PyValue::List(vec![
                        PyValue::Int(CTV_GIVE),
                        PyValue::Int(property),
                        value,
                    ])]),
                ],
            )
        };
        give(&mut reg, ALICE, CTPG_CASH, PyValue::Real(250.0)).expect("paid");
        assert_eq!(reg.character(BOB).unwrap().balance, before + 250.0);
        assert_eq!(
            crate::corp_wallet::balance(reg.deltas(), corp, 1000),
            1_000_000 - 25_000
        );
        // Without the take role the wallet refuses, as it does for the call.
        assert!(give(&mut reg, BOB, CTPG_CASH, PyValue::Real(1.0)).is_err());
        assert_eq!(reg.character(BOB).unwrap().balance, before + 250.0);
        // Shares, a nothing amount and an unknown property are refused
        // before anything moves.
        for (prop, v) in [
            (CTPG_SHARES, PyValue::Int(5)),
            (CTPG_CASH, PyValue::Real(0.0)),
            (99, PyValue::Int(1)),
        ] {
            assert_eq!(
                key_of(&give(&mut reg, ALICE, prop, v).unwrap_err()),
                "CustomInfo"
            );
        }
        // Set baseID: clearing is a no-op, a station is refused.
        let set = |reg: &mut Registry, v: PyValue| {
            on(
                reg,
                ALICE,
                "corpRegistry",
                "ExecuteActions",
                &[
                    PyValue::List(vec![PyValue::Int(BOB)]),
                    PyValue::List(vec![PyValue::List(vec![
                        PyValue::Int(CTV_SET),
                        bstr("baseID"),
                        v,
                    ])]),
                ],
            )
        };
        set(&mut reg, PyValue::None).expect("clearing");
        assert_eq!(
            key_of(&set(&mut reg, PyValue::Int(60_000_004)).unwrap_err()),
            "CustomInfo"
        );
    }

    #[test]
    fn a_ceo_from_before_the_wallet_roles_gains_them_on_load_and_on_login() {
        let (mut reg, corp) = alice_and_bob_in_one_corp();
        let old_roles = CORP_ROLE_DIRECTOR | CORP_ROLE_PERSONNEL_MANAGER;
        let age = |reg: &mut Registry| {
            reg.corporations.member_mut(ALICE).unwrap().roles = old_roles;
        };
        // World load.
        age(&mut reg);
        sync_characters(&mut reg);
        assert_eq!(reg.corporations.direct(ALICE).unwrap().roles, CEO_ROLES);
        assert_eq!(reg.corporations.direct(BOB).unwrap().roles, 0, "not a CEO");
        // Login: the change that carries the session keys has them.
        age(&mut reg);
        let _ = reg.take_pending();
        assert!(reg.corporations.get(corp).is_some());
        reg.heal_ceo_roles_at_login(ALICE);
        let login = reg.character_change(ALICE).expect("a login change");
        assert_eq!(
            crate::pyutil::pairs_get(&login, b"rolesAtAll"),
            Some(&PyValue::Tuple(vec![
                PyValue::None,
                PyValue::Int(CEO_ROLES)
            ]))
        );
        // The corp's other members hear of it; no stray session change.
        let told = pushes(&mut reg, "OnCorporationMemberChanged");
        assert_eq!(told.len(), 1);
        assert!(reg.take_session_changes().is_empty());
        // Healed once: nothing more to do.
        assert!(!heal_ceo_roles(&mut reg, ALICE));
        assert!(pushes(&mut reg, "OnCorporationMemberChanged").is_empty());
    }

    #[test]
    fn a_ceo_who_removed_all_roles_or_a_closed_corp_is_left_alone() {
        let (mut reg, _) = alice_and_bob_in_one_corp();
        update_member(&mut reg, ALICE, ALICE, [Some(0); 8], Some(true)).expect("removed");
        sync_characters(&mut reg);
        assert_eq!(reg.corporations.direct(ALICE).unwrap().roles, 0);
        assert!(!heal_ceo_roles(&mut reg, ALICE));
        // A non-CEO is never healed.
        assert!(!heal_ceo_roles(&mut reg, BOB));
        // Nor is a character in no corp.
        let mut lone = two();
        assert!(!heal_ceo_roles(&mut lone, ALICE));
    }

    /// FW-1: `corporationSvc.GetEmploymentRecord` is a list the enlist
    /// window iterates and `fwWarzoneDashboard` indexes at `[0]`, newest
    /// first, and never empty.
    #[test]
    fn the_employment_record_is_newest_first_and_never_empty() {
        const WHO: i64 = 90_000_901;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: WHO,
                ship_id: 100_000_901,
                ..crate::model::Character::default()
            }],
            ..SandboxConfig::default()
        });
        let rows = |reg: &mut Registry| {
            let got = dispatch(
                reg,
                &Request {
                    service: "corporationSvc",
                    method: "GetEmploymentRecord",
                    args: &[PyValue::Int(WHO)],
                    kwargs: &[],
                    bind_params: None,
                    actor: Some(WHO),
                },
            )
            .expect("answered");
            let PyValue::List(rows) = got else {
                panic!("a list, not None: {got:?}")
            };
            rows
        };
        assert_eq!(rows(&mut reg).len(), 1, "the synthetic first row");
        for (start, corp) in [("100", "7"), ("300", "9")] {
            reg.deltas_mut().put(
                HIST_KIND,
                vec![WHO.to_string(), start.into()],
                vec![corp.into()],
            );
        }
        let starts: Vec<PyValue> = rows(&mut reg)
            .iter()
            .map(|r| match r {
                PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, b"startDate")
                    .cloned()
                    .expect("startDate"),
                other => panic!("a KeyVal: {other:?}"),
            })
            .collect();
        assert_eq!(starts, vec![PyValue::Int(300), PyValue::Int(100)]);
    }

    #[test]
    fn a_corp_move_adds_a_row() {
        const WHO: i64 = 90_000_901;
        const NEW: i64 = 98_000_777;
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: WHO,
                ship_id: 100_000_901,
                ..crate::model::Character::default()
            }],
            ..SandboxConfig::default()
        });
        let old = reg.character(WHO).expect("pilot").corporation_id;
        move_character(&mut reg, WHO, Some((NEW, 0)));
        let rows = employment_rows(&reg, WHO);
        assert_eq!(rows.len(), 2, "the birth corp and the new one");
        assert_eq!(rows[0].1, NEW);
        assert_eq!(rows[1].1, old);
        move_character(&mut reg, WHO, None);
        assert_eq!(employment_rows(&reg, WHO).len(), 3);
    }
}
