//! The character sheet, skills, clones and standings. The agents moved to
//! `service/agents.rs` (W10.5 seam S5).
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
use crate::clones::{
    CLONE_ERR_NOT_DOCKED, clone_jump, clone_state_value, destroy_jump_clone, home_of,
    home_station_candidates, install_errors, install_jump_clone, rename_jump_clone,
    self_destruct_clone, set_home_station,
};
/// The station type and the log-on revive, at the path `service.rs` names them by.
pub(crate) use crate::clones::{StationRef, restore_capsules};
use crate::contacts::{
    add_contact, block_owners, contact_list_value, delete_contacts, edit_contacts_relationship,
};
/// The attribute rule and the injector, at the paths `service.rs` and
/// `service/inventory.rs` name them by.
pub(crate) use crate::skill_queue::inject_skill_into_brain;
use crate::skill_queue::{
    STARTING_ATTRIBUTE_FLOOR, abort_training, save_new_queue, skill_queue_rows,
};
use crate::standings::char_standings_value;

/// The services dispatched here. `Registry::dispatch` consults this to route.
pub(crate) const SERVICES: &[&str] = &[
    "charMgr",
    // **The diagnostic fix, and the only reason this name is here.**
    // `skillMgr2.GetMySkillHandler` is in `Registry::OBJECT_RETURNING`, and
    // `dispatch::handle_object_returning_call` dispatches before it mints so
    // that a handle-getter with a side effect can run. It swallows
    // `NoSuchMethod` -- the expected answer for a pure handle-getter -- but
    // **not `NoSuchService`**, and `skillMgr2` was the one row of that table
    // with no module advertising it. So every login logged
    // `UNANSWERED skillMgr2.GetMySkillHandler ... answering None so the
    // client keeps going`, which was wrong twice over: nothing was answered
    // `None`, and a perfectly good object reference was minted and sent two
    // lines later. The cost was zero and the line cost a later reader real
    // time chasing it. A false diagnostic is the third instance of that class
    // in this project.
    "skillMgr2",
    "skillMgr2::skillHandler",
    "paperDollServer",
    "jumpCloneSvc",
    "jumpTimers",
    "standingMgr",
    "missionTrackerMgr",
    "expertSystemMgr",
    "home_station",
];

/// The calls [`dispatch_ro`] answers (PA-12a).
pub(crate) const READ_METHODS: &[(&str, &str)] = &[("skillMgr2", "GetMySkillHandler")];

/// The read twin of the `skillMgr2` arm: the answer is discarded by
/// `handle_object_returning_call` anyway.
pub(crate) fn dispatch_ro(
    _reg: &Registry,
    _caller: Caller,
    req: &Request<'_>,
) -> Option<Result<RoValue, ServiceError>> {
    match (req.service, req.method) {
        ("skillMgr2", "GetMySkillHandler") => Some(Ok(RoValue::Owned(PyValue::None))),
        _ => None,
    }
}

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // **Routed so that the dispatcher stops lying about it.** The
        // answer is discarded: `handle_object_returning_call` dispatches,
        // throws the result away and mints the bound reference, which is the
        // whole contract of a handle-getter. What changes is that
        // `w.dispatch` now returns `Ok` instead of `NoSuchService`, so no
        // `UNANSWERED` line is written for a call that was always answered
        // correctly. See this module's `SERVICES`.
        "skillMgr2" => match req.method {
            "GetMySkillHandler" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // `_FetchBatchData` runs a second loop over the same batch and
        // calls this for every non-NPC id, unconditionally:
        //
        //     314 LOAD_FAST   charID
        //     317 LOAD_FAST   charInfoCache
        //     320 COMPARE_OP  7 (not in)
        //     ...
        //     372 LOAD_ATTR   GetPublicInfo
        //
        // so answering the batch above without answering this one would
        // only move the hang downstream. The caller stores the result and,
        // because our batch is empty, never reads it -- `if not charInfo
        // or not dna: continue`. None is therefore safe *here*.
        //
        // That reasoning was right for the portrait caller and wrong for
        // the second caller it anticipated -- the character sheet, which
        // is where the debt came due.
        //
        // `characterOverviewPanel.ApplyAttributes` opens with
        //
        //     charinfo = reg.charMgr.GetPublicInfo(session.charid)
        //
        // and threads that one value through five frames without ever
        // re-fetching or testing it, down to
        // `FormatDateTime(value=charinfo.createDateTime, ...)`. So `None`
        // was `AttributeError: 'NoneType' object has no attribute
        // 'createDateTime'` and the whole window failed to open.
        //
        // **This one never reached the UNANSWERED list**, because we were
        // answering it. That is the §3a case in its purest form: a call we
        // answer leaves no trace in any server-side instrument, so the
        // only evidence was the traceback the client mailed us. It is also
        // why the fix for it cost a relaunch and the fix for a missing
        // service costs none -- the missing one announces itself.
        //
        // A **KeyVal**, not a rowset of either family: every consumer in
        // the tree does attribute access, `hasattr`, truthiness or stores
        // it in a dict. Nothing calls `.Index`, `.Clone`, `.header` or
        // subscripts a column. The client itself substitutes a
        // `utillib.KeyVal` for this exact value on the NPC path
        // (`scenePortraitRenderer.GetNPCCharInfo`), which settles it.
        "charMgr" => match req.method {
            // `gatekeeper.character`, the twin of `charUnboundMgr.GetCohortsForUser`
            // (`gatekeeper/client/service.py`): `cohortID in <answer>`. Not yet
            // seen in a client log -- answered so the first call is not a
            // `TypeError`. No experiment cohorts here.
            "GetCohortsForCharacter" => Ok(PyValue::List(vec![])),
            // **The id is honoured now.** This used to answer about the
            // actor and discard the argument, which was right for one player
            // and a visible lie for two -- asking about Bob and being told
            // Alice's corporation.
            //
            // `GetPublicInfo(charID)`, one positional argument, **[V]** at
            // three independent call sites:
            // `crimewatchSvc.GetSlimItemDataForCharID` (`:213`),
            // `ccSvc.GetSchoolAndSchoolCorpInfoForChar` (`:213`) and
            // `scenePortraitRenderer`. crimewatch takes its `charID` from a
            // ballpark `slimItem`, so the id really can be any character in
            // space and not just the caller.
            //
            // **`None` is not an option here, and that is what makes the
            // unknown branch below necessary rather than defensive padding.**
            // crimewatch does `pubInfo.corporationID`, `.warFactionID` and
            // `.allianceID` as three consecutive attribute reads with no
            // `is None` test and no `try` anywhere above them, so a `None`
            // answer is an `AttributeError` on the overview path. Returning
            // it was safe only for as long as we ignored the argument: the
            // actor is always in the roster, so the branch never fired.
            // Honouring the id is what makes it reachable.
            "GetPublicInfo" => {
                // The argument, falling back to the actor. A non-integer
                // argument is treated as absent rather than refused: the
                // callers above all pass a bare id, so anything else is a
                // client we do not understand and the actor is the better
                // guess than a raise.
                let who = match req.args.first() {
                    Some(PyValue::Int(id)) => Some(*id),
                    _ => req.actor,
                };
                let opt = |v: Option<i64>| v.map(PyValue::Int).unwrap_or(PyValue::None);
                let Some(c) = who.and_then(|id| reg.character(id)) else {
                    // A character we have never heard of. **Our roster is
                    // the world**, so this is either a stale id the client
                    // cached or a question asked before selection -- in
                    // either case there is no honest answer, and the least
                    // dishonest one is a truthy KeyVal carrying the three
                    // fields crimewatch reads unguarded.
                    //
                    // The corporation is the NPC corp on purpose: it keeps
                    // `idCheckers.IsPlayerCorporation` false, so nothing
                    // downstream takes a player-corp branch about somebody
                    // who does not exist.
                    return Ok(keyval(vec![
                        ("characterID", opt(who)),
                        (
                            "corporationID",
                            PyValue::Int(crate::model::NPC_CORPORATION_ID),
                        ),
                        ("allianceID", PyValue::None),
                        ("warFactionID", PyValue::None),
                    ]));
                };
                Ok(keyval(vec![
                    ("characterID", PyValue::Int(c.character_id)),
                    ("corporationID", PyValue::Int(c.corporation_id)),
                    ("allianceID", opt(c.alliance_id)),
                    // FW-1: the militia the pilot serves, or None;
                    // `crimewatchSvc.GetSlimItemDataForCharID` copies it
                    // through.
                    (
                        "warFactionID",
                        opt(super::facwar_mgr::war_faction(reg, c.character_id)),
                    ),
                    ("createDateTime", PyValue::Int(c.create_date_time)),
                    ("raceID", PyValue::Int(c.race_id)),
                    ("bloodlineID", PyValue::Int(c.bloodline_id)),
                    ("gender", PyValue::Int(c.gender_id)),
                    // **`schoolID`, because the home-station panel needs it
                    // after all** (found by the P5.9 lane): the station
                    // right-click "Set Home Station" runs
                    // `homestation/client/service.get_school_hq_id`
                    // (`:175-180`), which reads
                    // `GetPublicInfo(charid).schoolID` unguarded, then
                    // `schools.get_school(id).corporationID` and that
                    // corporation's `stationID`. Absent, the menu dies
                    // client-side before reaching us. 11 is a key of this
                    // build's `schools.fsdbinary` **[V]** (23 keys: 11-25,
                    // 31-38), so `get_school` cannot `KeyError`; that it is
                    // *this character's* school is **[A]** -- the school
                    // rows' `corporationID`/`raceID` are not decoded yet, and
                    // a wrong school only moves the "school HQ" marker.
                    // Since P-START-2 it is the character's own school
                    // (`Character::school_id`, `model::DEFAULT_SCHOOL_ID`
                    // for one no school placed).
                    ("schoolID", PyValue::Int(c.school_id)),
                    // **[I]** `ancestryID` is deliberately absent, and the
                    // absence is safer than a guess. The paragraph below was
                    // written about both fields:
                    // `characterdata.schools.get_school` is a bare dict
                    // subscript over `res:/staticData/schools.fsdbinary`,
                    // so an invented id is a `KeyError` rather than a
                    // wrong label. Nothing in the character-sheet overview
                    // reads either field, and **both** functions that do
                    // are dead in V24.01 -- `GetRaceIconHint`, and
                    // `ccSvc.GetSchoolAndSchoolCorpInfoForChar` (`:213`),
                    // which reads `charInfo.schoolID` unguarded and is
                    // defined with no `LOAD_ATTR` of its name anywhere in
                    // the tree. The paths outside them that want `schoolID`
                    // guard with `hasattr`, which a KeyVal answers correctly
                    // by raising AttributeError.
                    // `schoolID` was added on exactly those terms (above).
                ]))
            }
            // The character info window's own read, and **it is not
            // `GetPublicInfo` with a 3 on the end**: different shape,
            // different consumer.
            //
            // `characterInfoWindow.construct_header_character:80-83` fires
            // this, `corpRegistry.GetInfoWindowDataForChar` and
            // `crimewatch.GetCharacterSecurityStatus` in **one
            // `uthread.parallel`**, and `uthread.Parallel` re-raises in the
            // caller (`:797`, with no `exceptionHandler` passed anywhere) --
            // so all three share a fate and answering one of them alone
            // changes which line the window dies on rather than fixing it.
            // That is why they land in one commit.
            //
            // **A `CRowset` of exactly one row**, subscripted `info[0]`:
            // `CRowset` subclasses `list`, so `[0]` is the single row. A
            // one-element `List` of a `KeyVal` satisfies every reader this
            // was checked against -- `[0]` works on a list, attribute access
            // works on a `KeyVal`, and nothing touches `.header`, `.Index`,
            // `.columns` or `len()`. The list is the honest cheap answer and
            // is what this sends; the descriptor is fidelity we would be
            // inventing column types for.
            //
            // **`militiaFactionID` is the trap.** `:216` reads it as a plain
            // `LOAD_ATTR`, so a row without it raises after the window has
            // already drawn its header. `description` (`:89`) and
            // `bloodlineID` (`:300`) are also unguarded; `startDateTime`
            // (`:90`) is read but its *use* at `:201` is guarded by
            // `is not None`, so the column must exist and the value may not.
            //
            // `medal1GraphicID` is **omitted**: it is read with
            // `getattr(..., None)` and only decides whether a decorations
            // icon is drawn, so a KeyVal's `AttributeError` is the right
            // answer to it.
            //
            // `None` was honest here and cost only the Bio tab, the
            // membership age and the militia block -- this is an improvement
            // rather than a repair, which is why it is the cheap half of the
            // info window.
            "GetPublicInfo3" => {
                let who = opt_id(req.args.first()).or(req.actor);
                let Some(c) = who.and_then(|id| reg.character(id)) else {
                    // Unknown character: `None` is what the four readers all
                    // test for, and inventing a row about somebody who does
                    // not exist is worse than the empty they handle.
                    return Ok(PyValue::None);
                };
                Ok(PyValue::List(vec![keyval(vec![
                    ("corporationID", PyValue::Int(c.corporation_id)),
                    ("bounty", PyValue::Int(0)),
                    ("title", bstr("")),
                    ("description", bstr("")),
                    // A blue FILETIME, and the same `long`-not-`int`
                    // constraint as `createDateTime`: `:201` puts it through
                    // the age arithmetic only when it is not None.
                    ("startDateTime", PyValue::Int(c.create_date_time)),
                    // Not in a militia. Required as an attribute, free as a
                    // value.
                    ("militiaFactionID", PyValue::None),
                    ("bloodlineID", PyValue::Int(c.bloodline_id)),
                ])]))
            }
            // The EverMark donation panel's corp-membership check. One
            // attribute, `corporationDateTime`, a blue FILETIME --
            // subtracted from `GetWallclockTime()` and divided by a day,
            // so the same `long`-not-`int` constraint as `createDateTime`
            // applies.
            //
            // `None` is explicitly tolerated here (`if char_info is None:
            // return False`), so this is not what was looping -- but it
            // was being called in lockstep with the LP balance, once per
            // iteration, which is how it ended up with the same count.
            // Answering it properly costs nothing and removes it from the
            // next log.
            //
            // The verdict is still False either way: the check ends with
            // `idCheckers.IsPlayerCorporation(session.corpid)`, and corp
            // 1000001 is an NPC corp.
            // `corp_dlg_edit_member` General tab: characterID, gender,
            // raceID and createDateTime are read. The fields are public
            // anyway (GetPublicInfo), so no corp check. An unknown id is
            // refused: `get_race_name(None)` would raise in the client.
            "GetPrivateInfo" => {
                let found = match req.args.first() {
                    Some(PyValue::Int(id)) => reg.character(*id),
                    _ => None,
                };
                let Some(c) = found else {
                    return Err(ServiceError::Raise(eve_proto::user_error(
                        "CustomNotify",
                        crate::pyutil::pypairs(vec![(
                            "notify",
                            crate::pyutil::bstr("That character is not known here."),
                        )]),
                    )));
                };
                Ok(keyval(vec![
                    ("characterID", PyValue::Int(c.character_id)),
                    ("gender", PyValue::Int(c.gender_id)),
                    ("createDateTime", PyValue::Int(c.create_date_time)),
                    ("raceID", PyValue::Int(c.race_id)),
                    ("bloodlineID", PyValue::Int(c.bloodline_id)),
                    ("ancestryID", PyValue::None),
                ]))
            }
            "GetPrivateInfoOnCorpChange" => {
                let Some(c) = req.actor.and_then(|id| reg.character(id)) else {
                    return Ok(PyValue::None);
                };
                Ok(keyval(vec![(
                    "corporationDateTime",
                    PyValue::Int(c.create_date_time),
                )]))
            }
            // **A 2-tuple of numbers, unpacked before anything is
            // tested** -- the §4 third-row trap:
            //
            //     asset_net_worth = None
            //     try:
            //         inv_container = reg.inv_cache.GetInventory(const.containerGlobal)
            //         asset_worth, plex_worth = inv_container.GetAssetWorth()
            //     ...
            //     if asset_net_worth is not None:
            //
            // The `is not None` guard four lines later is what makes
            // `None` look safe; `UNPACK_SEQUENCE 2` runs first.
            //
            // This arrives on a **bound object**, and the container is
            // `containerGlobal` (10002) -- a second magic container,
            // distinct from the `containerHangar` (10004) that
            // `invbroker::inventory` keys on, and bound on `charMgr`
            // rather than on `invbroker`. Note the bind params are
            // `(charID, containerGlobal)`, so the container is the
            // **second** element here where an inventory carries it
            // first: `container_of` would return the charID. We do not
            // need to switch on it -- `charMgr` has one such container --
            // but the next one will.
            //
            // Both values are laundered through `or 0` downstream, so
            // zeros cost nothing.
            //
            // Same 12-hour `settings.char.ui` cache as the contract
            // escrow: the net-worth number in the UI may lag the fix.
            "GetAssetWorth" => Ok(PyValue::Tuple(vec![PyValue::Real(0.0), PyValue::Real(0.0)])),
            // The AFK reporter, and the one call so far that arrives with
            // **keyword arguments and no positional ones** -- which is why
            // the server log named it `SetActivityStatus/0`.
            //
            // `gameui._afk_timer_loop` (`gameui.py`, firstlineno 307) polls
            // once a second and, five minutes after the last UI event:
            //
            //     if not reg._is_afk and dt_since_active > AFK_THRESHOLD:
            //         reg._is_afk = True
            //         ...
            //         sm.RemoteSvc('charMgr').SetActivityStatus(
            //             status=appConst.PLAYER_STATUS_AFK,
            //             extraInfo=datetimeutils.datetime_to_filetime(...))
            //
            // with the mirror call on the way back to ACTIVE. The return
            // value is discarded at both call sites, so `None` is right
            // and no shape is owed.
            //
            // It is **not** on the path into the game, and seeing it in the
            // log is a clue about elapsed time rather than about progress:
            // it means the client has been sitting untouched for five
            // minutes, which on a stuck loading screen it certainly has.
            // Worth answering anyway. The loop runs in its own
            // `uthread.worker`, so leaving it unanswered does not freeze
            // the UI -- but it does wedge that worker forever, and with it
            // every later AFK transition.
            "SetActivityStatus" => Ok(PyValue::None),
            // A **mutable mapping** of settingKey -> value.
            // `characterSettingsSvc` subscripts it, tests `in`, assigns
            // into it and `del`s from it. The trap is that `Get` guards
            // with `except KeyError`, which does *not* catch the
            // `TypeError` that `None[key]` raises -- so `None` broke
            // chat-filter initialisation. Nothing caches the failure
            // (`if reg._settings is None`), so it re-issued on every
            // single read.
            "GetCharacterSettings" => Ok(match req.actor {
                Some(ch) => crate::char_settings::settings_value(reg, ch),
                None => PyValue::Dict(vec![]),
            }),
            // The home station, and **the call that was blocking the map.**
            // `mapView.py(605) ShowMyHomeStation`:
            //
            //     homeStationRow = sm.GetService('charactersheet').GetHomeStationRow()
            //     homeStationID = homeStationRow.stationID            # 606
            //     if not homeStationID or self.destroyed: return      # 607
            //     solarSystemID = homeStationRow.solarSystemID        # 610
            //
            // `mapViewSolarSystem.py(656)`, `mapView.py(431)
            // LookAtHomeStation`, `mapFilterOreAnomalies.py(67)` and
            // `charactersheet.py(63) GetHomeStation` read the same three names
            // and no others -- the union of their `co_names` is `stationID`,
            // `solarSystemID`, `stationTypeID`. Nothing subscripts it, reads
            // `.header`/`.lines` or sizes it, so `KeyVal` is the family. A
            // packed `DbRow` would be wrong outright: rows of this kind get
            // `STORE_ATTR` written onto them elsewhere, and `DBRow` refuses
            // that.
            //
            // **The id is not a free choice.** `home_station
            // .get_home_station` already tells the character sheet which
            // station is home, so the two must agree. They used to both read
            // `characters.first()`, which is a safety that only held for one
            // player; now they both read **the same actor**, which is the
            // same guarantee for any number. A disagreement would show the
            // player two different homes in two windows and log nothing.
            //
            // Naming the real station rather than `None` is safe, and that
            // was checked: `IsStation(60012526)` is true, so
            // `ui.GetStationStaticInfo` goes to `cfg.stations.Get` -- the
            // `npcStations` table this station was read out of -- and the
            // region and layout-node lookups use the client's own map FSD.
            // `None` would merely have hidden the marker.
            //
            // Both now read [`home_of`] (the clones section), which is the
            // set home station when there is one and the docked-in station
            // otherwise -- the same rule `home_station.get_home_station`
            // follows, so the two windows still cannot disagree.
            "GetHomeStationRow" => {
                let Some((home, _)) = req.actor.and_then(|id| home_of(reg, id)) else {
                    return Ok(PyValue::None);
                };
                Ok(keyval(vec![
                    ("stationID", PyValue::Int(home.station_id)),
                    ("solarSystemID", PyValue::Int(home.solar_system_id)),
                    ("stationTypeID", PyValue::Int(home.type_id)),
                ]))
            }
            // Stations holding this character's assets. **A bound call**, on
            // the same `(charID, 10002)` object as `GetAssetWorth`.
            //
            // **A `CRowset`, because `.Index()` is called on it.**
            // `assetsWindow.py(561)` and `corp_ui_accounts.py(936)` do
            // `.ListStations().Index('stationID')` and then treat the result
            // as a dict (`in`, `.get(id)`) -- only the `CRowset` path yields a
            // `CIndexedRowset`, which subclasses `dict`. **So this answer
            // reaches native code**, `blue.pyos.XUtil_Index`, like every
            // CRowset we send.
            //
            // Not a live lock, unusually for an unanswered cache.
            // `invCache.py(909)` is `self.cachedStations = (now,
            // self.moniker.ListStations())` behind a five-minute stamp, so the
            // tuple stored our `None` successfully and the client waited the
            // cache out instead of spinning. The cost was one `TypeError` at
            // `assetsinfo.py(163) GetStations`.
            //
            // **Empty is a lie we are choosing deliberately:** there is a
            // Capsule in that hangar. The truthful answer is one row, and it
            // owes two more bound methods on this same object that are
            // unreached only because this one returned `None` --
            // `ListStationItems(stationID)` and `ListIncludingContainers()`,
            // the second of which **latches** on `self.listedAll = 1`, so a
            // wrong answer there is cached for the session. Those three belong
            // in one change, not three.
            "ListStations" => Ok(station_assets_rowset(reg, req.actor)),
            // Issue #3's `charMgr` half: every item this character owns,
            // anywhere, for the assets window's cross-station search. The
            // third bound method on the same `(charID, containerGlobal)`
            // object -- `invCache.py(577)` is
            // `Moniker('charMgr', (session.charid, containerGlobal))`,
            // which is why a `charMgr` arm answers what the census filed
            // under `invbroker`.
            //
            // The capture:
            //
            // ```text
            // [380] 108624.933ms OK  charMgr*  ListIncludingContainers [N=1:22]()  -> None
            // ```
            //
            // `invCache.py(963)`:
            //
            // ```text
            // realItems = self.moniker.ListIncludingContainers()
            // for realItem in realItems:
            //     self.cachedItemsAll[realItem.itemID] = realItem
            // ...
            // self.listedAll = 1
            // ```
            //
            // So: iterated, `.itemID` read as an **attribute**, and
            // `len()` taken one line later in
            // `assetsWindow.py(531) Search_thread`. A plain **list of
            // `blue.DBRow`**, which is what `invbroker::inventory.List`
            // already answers with.
            //
            // **The shape is not this lane's to choose.** The window spans
            // two services -- this one and `invbroker.GetContainerContents`
            // -- and `invCache` merges both into one dict keyed by
            // `realItem.itemID`, so the rows must agree. They come from
            // `Registry::owned_item_rows()`, which both lanes call and
            // neither defines; `docs/archive/plan-docked-subsystems.md` calls it
            // the round's one cross-lane contract. `GetFakeRowset` in
            // `eveAssets/assetSearching.py(89)` is the downstream check on
            // it: it reads `itemID`, `typeID`, `quantity`, `locationID`
            // and `flagID` off each row and packs them into a `CRowset` of
            // its own, and every one of those is in `item_columns()`.
            //
            // Two things this does *not* finish, both deliberate:
            //
            // - `listedAll = 1` is set after the loop, so the old `None`
            //   left it unset and the assets window re-asked on every
            //   search. The latch now takes, with a five-minute TTL from
            //   `_DeleteCachedItemsAll`.
            // - the capsule this answer carries still will not *appear*.
            //   `Search_thread` filters the results through
            //   `ListStations().Index('stationID')` and `continue`s on any
            //   station it does not find, and `ListStations` above is
            //   deliberately empty. The two belong in one change, as that
            //   arm's comment says; `station_assets_rowset()` lives in
            //   `service.rs`, which this lane does not own.
            "ListIncludingContainers" => Ok(PyValue::List(reg.owned_item_rows(req.actor))),
            // `GetInventory(containerGlobal).ListStationItems(stationID)`:
            // the caller's items at one station, iterated for `.typeID`,
            // `.itemID`, `.quantity`, `.flagID` by the assets window's
            // station expansion and the market quote's item picker
            // (assetsWindow.py:939, quote.py:436, **[V]** as call sites). The
            // rows are the ones `ListIncludingContainers` sends, narrowed to
            // `locationID == stationID`: a hangar stack, an assembled hull.
            // What sits inside those (cargo, fitted modules) is not at the
            // station and is left out **[A]**: the client reads the contents
            // of a container through the container's own inventory.
            "ListStationItems" => {
                let station = match req.args.first() {
                    Some(PyValue::Int(n)) => *n,
                    _ => 0,
                };
                Ok(PyValue::List(match req.actor {
                    Some(me) => reg
                        .store()
                        .owned_by(me)
                        .filter(|i| i.location_id == station)
                        .map(|i| reg.item_row_value(i))
                        .collect(),
                    None => Vec::new(),
                }))
            }
            // Called after every personal split and trash, and after a
            // delivery: `InvalidateItemLocation` ends in
            // `GetInventory(containerGlobal).InvalidateStationItemsCache(
            // stationID)` with the answer dropped (invItemFunctions.py:299,
            // **[V]**). The server keeps no such cache, so there is nothing
            // to drop.
            "InvalidateStationItemsCache" => Ok(PyValue::None),
            // The Bio tab, as a plain **string** -- not a row, not a
            // 1-tuple. `bioPanel.LoadBioFromServer` is
            //
            //     bio = sm.RemoteSvc('charMgr').GetCharacterDescription(session.charid)
            //     reg.bioClient = bio if bio is not None else ''
            //
            // and `LoadPanel` hands it straight to `edit.SetValue`. `None`
            // is explicitly tolerated -- the first row of the §4 table --
            // so this arm fixes no traceback. What it fixes is that a bio
            // the player types is still there when they reopen the window.
            // **The argument is already there and we used to drop it.** The
            // client calls this as `GetCharacterDescription(session.charid)`,
            // so the protocol was per character the whole time; the server
            // held one string. Answering the actor's is the 0.3 fix -- the
            // *argument* is still unread, for the same reason as
            // `GetPublicInfo` above, and the empty string is the right answer
            // for a character who has never saved one.
            "GetCharacterDescription" => Ok(PyValue::Unicode(
                req.actor
                    .and_then(|id| reg.character_description(id))
                    .unwrap_or_default(),
            )),
            // `bioPanel._AutoSaveBio` is
            //
            //     try:
            //         sm.RemoteSvc('charMgr').SetCharacterDescription(newbio)
            //     except:
            //         reg.ReloadBioFromServer()
            //         raise
            //
            // with `POP_TOP` on the result: the value is discarded and the
            // whole contract is "do not raise". It fires from
            // `UnloadPanel` and `Close` under `uthread.new`, so a raise
            // here would be a bare re-raise on a detached tasklet.
            "SetCharacterDescription" => {
                let text = match req.args.first() {
                    Some(PyValue::Unicode(u)) => u.clone(),
                    Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                    _ => String::new(),
                };
                // No actor, nothing saved -- and still not a raise, because
                // the whole contract of this call is "do not raise".
                if let Some(id) = req.actor {
                    reg.set_character_description(id, text);
                }
                Ok(PyValue::None)
            }
            // The write and delete halves of `GetCharacterSettings` above
            // (`characterSettingsSvc.Save/Delete`, `crate::char_settings`):
            //
            //     def Save(self, settingKey, value):
            //         if value is None or value == '':
            //             reg.Delete(settingKey)
            //         ...
            //         reg.charMgr.SaveCharacterSetting(settingKey, value)
            //
            // The client calls Save even after an empty-value Delete, so an
            // empty or missing value here means delete. The return is
            // discarded at the only call site of each, so `None` stays the
            // answer; an over-size or over-count value is dropped (the
            // client refuses over 102400 itself) rather than raised, as
            // nothing reads a raise here. The value is YAML text for the
            // fleet and mail settings, stored as the text it is.
            "SaveCharacterSetting" | "DeleteCharacterSetting" => {
                let text = |v: Option<&PyValue>| match v {
                    Some(PyValue::Unicode(u)) => Some(u.clone()),
                    Some(PyValue::Bytes(b)) => Some(String::from_utf8_lossy(b).into_owned()),
                    _ => None,
                };
                if let (Some(ch), Some(key)) = (req.actor, text(req.args.first())) {
                    match (req.method, text(req.args.get(1))) {
                        ("SaveCharacterSetting", Some(value)) => {
                            crate::char_settings::save(reg, ch, &key, &value);
                        }
                        _ => {
                            crate::char_settings::delete(reg, ch, &key);
                        }
                    }
                }
                Ok(PyValue::None)
            }
            // **Remote code execution, by design -- and we now accept.**
            // The only call in the table where our answer is *executable*;
            // `docs/reference/code-push.md` is about this one arm.
            //
            // `carbonui/services/settingsSvc.py`, `UpdateSettingsStatistics`
            // (code object firstlineno 198, body lines 205-211 -- line
            // numbers from the `.pyc`'s own lnotab via `tools/pyc27.py`,
            // not from uncompyle6, which prints 136 for this function):
            //
            //     205  code, verified = Crypto.Verify(
            //              sm.RemoteSvc('charMgr').GetSettingsInfo())
            //     206  if not verified:
            //     207      raise RuntimeError('Failed verifying blob')
            //     208  SettingsInfo.__code__ = marshal.loads(code)
            //     209  ret = SettingsInfo()
            //     210  if len(ret) > 0:
            //     211      sm.RemoteSvc('charMgr').LogSettings(ret)
            //
            // `Crypto.Verify` is `evecrypto/placebo.py(22)` for us
            // (`cryptoPack` is Placebo, see `up.sh`) and is one line:
            // `return (signedData[0], signedData[1] == 0)`. So the answer
            // is the 2-tuple `(blob, 0)` -- element 1 equal to `0` is what
            // makes `verified` true, element 0 is the blob -- and `blob` is
            // a stdlib-`marshal`ed **Python 2.7 code object** that the
            // client installs as `SettingsInfo.__code__` and then calls.
            // Not `blue.marshal`: `crate::pycode` emits CPython's own
            // format and shares nothing with `wire`.
            //
            // **What `None` here actually did.** The previous version of
            // this arm answered `None` and this comment predicted it would
            // cost "exactly one isolated log record". It cost a
            // `TypeError: 'NoneType' object is not subscriptable` raised
            // inside `placebo.verify` at `signedData[0]`, on the frames
            //
            //     settingsSvc.py(215) OnCharacterSessionChanged
            //     settingsSvc.py(205) UpdateSettingsStatistics
            //     placebo.py(22)      verify
            //
            // and we saw it only because the client **mails us its
            // tracebacks** -- nothing on our side logs a call we answer
            // (`docs/guides/debugging-the-client.md` §3a). The "isolated" half of
            // the prediction held: 215 is `OnCharacterSessionChanged`,
            // which fires once per session under `MollycoddledUthread`, so
            // it was one traceback and not a live lock. The "log record"
            // half was wrong -- the exception happens two frames *below*
            // the line that would have handled a bad shape gracefully,
            // because `verify` subscripts before anything is checked.
            //
            // **Why an empty dict is the honest blob.** The real
            // statistics function is server-side at CCP and is not in the
            // client tree, so its content is unrecoverable; what the
            // client does with the result is the whole specification, and
            // line 210 is `len(ret) > 0`. An empty mapping says "this
            // server collects no settings statistics", makes the test
            // false, and suppresses the `LogSettings` follow-up. See
            // `pycode::return_empty_dict` for why each field of the code
            // object has the value it has.
            //
            // It goes on the wire as `Bytes`, Python 2 `str`: the blob is
            // 109 bytes with NULs throughout, which `marshal.loads` wants
            // verbatim. A `Unicode` here would be re-encoded and arrive
            // corrupt.
            "GetSettingsInfo" => Ok(PyValue::Tuple(vec![
                // **The one place a client-CI scenario enters the client.**
                // With no `--scenario` this is the constant blob it has
                // always been; with one, the same blob plus an `execfile`
                // of the scenario. The name was validated at startup, so a
                // failure here cannot happen -- and if it somehow did, the
                // plain blob is the right fallback at *this* point, because
                // refusing to answer `GetSettingsInfo` breaks login for a
                // reason the player cannot act on. `cli.rs` is
                // where a bad name is a fatal error.
                PyValue::Bytes(
                    reg.config()
                        .scenario
                        .as_deref()
                        .and_then(|name| crate::pycode::settings_info_scenario_func(name).ok())
                        .unwrap_or_else(crate::pycode::settings_info_func),
                ),
                // Must be `0`, not `False`: `placebo.verify` compares with
                // `== 0`, which `False` also satisfies in Python 2 -- but
                // the real server signs with `placebo.sign`, which returns
                // `(data, 0)`, so the int is what the client has always
                // been handed.
                PyValue::Int(0),
            ])),
            // The follow-up to the above, at `settingsSvc.py(211)`. Our
            // blob returns `{}` so `len(ret) > 0` is false and this is
            // never reached -- it exists so that a future non-empty blob,
            // or a build whose `SettingsInfo` differs, does not meet a
            // refused call.
            //
            // `None` is right: the bytecode is
            // `CALL_FUNCTION 1; POP_TOP` at offsets 120 and 123, so the
            // return value is discarded on the spot, and nothing else in
            // the tree calls `LogSettings`. We do not even look at the
            // argument -- settings statistics are a telemetry channel for
            // CCP's cluster and there is nothing here that would read
            // them.
            "LogSettings" => Ok(PyValue::None),
            // An **object with two list attributes**, not a list and not
            // a rowset. `addressbookService.GetContacts` reads exactly
            // two things off it:
            //
            //     for each in addressbook.addresses:
            //     for each in addressbook.blocked:
            //
            // and nothing else in the file touches it.
            //
            // It arrives through `uthread.parallel`, so it is one of
            // three or four concurrent calls whose results are unpacked
            // positionally -- which is why answering it late or not at
            // all took the whole address book with it. The rows, and the
            // calls that change them, are in `crate::contacts`
            // ([`contact_list_value`]).
            "GetContactList" => Ok(contact_list_value(reg, req.actor)),
            // The address book's writes. One function per client call;
            // every one answers `None`, because every caller discards the
            // return (`addressbookService.py:792/809/831/874/941/975/1000`)
            // and updates its own cache itself.
            "AddContact" | "EditContact" => add_contact(reg, req),
            "EditContactsRelationshipID" => edit_contacts_relationship(reg, req),
            "DeleteContacts" => delete_contacts(reg, req),
            "BlockOwners" => block_owners(reg, req, true),
            "UnblockOwners" => block_owners(reg, req, false),
            // The address book's contact labels. **One answer, three call
            // sites, and a live lock that `{}` closes.**
            //
            // `addressbookService.py(1213/1217/1221)` keeps three separate
            // caches filled from the same method:
            //
            // ```text
            // def GetContactLabels(self, contactType, updateVarible=True):
            //     uthread.Lock(self, 'labels')
            //     try:
            //         ...
            //         if contactType == 'contact':
            //             if self.labels is None:
            //                 self.labels = self._GetSvcForContactType(contactType).GetLabels()
            //             return self.labels
            //         if contactType == 'corpcontact':
            //             if self.corporateLabels is None: ...
            //         if contactType == 'alliancecontact':
            //             if self.allianceLabels is None: ...
            // ```
            //
            // Only the first is ours: `_GetSvcForContactType` returns
            // `sm.RemoteSvc('charMgr')` for `'contact'` and the client-side
            // `corp` and `alliance` services for the other two, which reach
            // `corpRegistry.GetLabels` and `allianceRegistry.GetLabels` in
            // other modules. Same shape, three names, one fix each.
            //
            // **`None` was the compounding failure from
            // `docs/guides/debugging-the-client.md` §4 in its `contractProxy` form**,
            // and the bytecode is what settles that rather than the shape
            // of the source (`tools/pyc27.py`):
            //
            // ```text
            // 1212  52 LOAD_ATTR     labels
            // 1212  55 LOAD_CONST    None
            // 1212  58 COMPARE_OP    8              # `is`
            // 1213  79 CALL_FUNCTION                # GetLabels()
            // 1213  85 STORE_ATTR    labels         # <- fed straight from the call
            // 1214  94 LOAD_ATTR     labels
            // 1214  97 RETURN_VALUE
            // ```
            //
            // Nothing between the call and the store can raise, so the
            // store **completes** and the cache holds the one value its own
            // `is None` guard rejects: the next read re-issues, forever, at
            // whatever rate the window rebuilds, while every consumer of
            // the returned `None` raises a frame later. This is *not* the
            // LP variant where the assignment never runs -- that
            // distinction matters because it means a fix has to make the
            // stored value pass the guard, not merely avoid throwing. `{}`
            // is not `None`, so the cache latches on the first call and the
            // call happens once per session.
            //
            // **A dict, and no rowset family survives the consumers.**
            // Read every one of them and the operations are all
            // mapping-only, on both sides of the service boundary:
            //
            // - `contactsForm.py(277)` `for each in myLabels.itervalues():`
            // - `addressbookService.CheckLabelName` / `GetAssignLabelMenu`
            //   / `GetRemoveLabelMenu` take `.values()` and read
            //   `each.labelID`, `each.name`
            // - `GetLabelNameList` does `myLabels.get(labelID, None)` then
            //   `label.name`, `label.color`
            // - `EditContactLabel` does `if labelID in labels:` then
            //   `labels[labelID].name = name`
            // - `CreateContactLabel` does
            //   `labels[labelID] = utillib.KeyVal(...)` at line 1197 --
            //   **item assignment into our answer**, which rules out every
            //   immutable shape and is why the values are `KeyVal`s rather
            //   than `DBRow`s when there are any.
            //
            // `{}` is also the *correct* answer rather than a stand-in: a
            // new character has no labels of their own, and the address
            // book's static groups come from
            // `contactsForm.GetStaticLabelsGroups`, not from here -- the
            // same division `mailMgr.GetLabels` relies on in
            // `service/social.rs`.
            // GAPS-2: the character's own contact labels.
            "GetLabels" => Ok(crate::contact_labels::labels_value(
                reg,
                req.actor.unwrap_or(0),
            )),
            // The character sheet's combat log (SOC-C6).
            "GetRecentShipKillsAndLosses" => {
                super::killmail::recent_ship_kills_and_losses(reg, req)
            }
            // GAPS-2: notes, the notepad, contact labels.
            _ => match super::gaps2::char_mgr(reg, req) {
                Some(answer) => answer,
                None => Err(reg.no_method(req)),
            },
        },
        "skillMgr2::skillHandler" => match req.method {
            // A plain **list**, not a rowset of either family. Both
            // consumers only iterate it and read named attributes off
            // each row; nothing calls `.Index`, `.Clone`, `.Filter`,
            // `len()` or `.sort()` on it anywhere in the tree.
            //
            // This one is worth more than it looks. It is the FIRST
            // statement of `notificationUI._NotificationProvider`:
            //
            //     skillNotifications = SkillHistoryProvider(...).provide()
            //     restOfNotifications = sm.GetService('notificationSvc')
            //         .GetAllFormattedNotifications(...)
            //
            // so a throw here means `GetAllFormattedNotifications` is
            // never reached and the **entire** neocom notification centre
            // is empty of every kind of notification, not just skills.
            // `notificationCache` also stays `None`, so the panel's
            // control bar is disabled and the unread badge never appears.
            //
            // Cached on `is None` in two layers at once
            // (`notificationUI.notificationCache` and
            // `skillsvc.skillHistory`), so a bad answer re-issues this
            // *and* `CheckAndSendNotifications` on every open of the bell.
            //
            // **[I]** Non-empty rows carry `skillTypeID`,
            // `absolutePoints`, `logDate` (a win64 filetime -- it is both
            // `FmtDate`-formatted and compared against
            // `lastSeenMessageTime`) and `eventTypeID` (a key in
            // `appConst.skillEvent*`, else the UI says "Unknown").
            "GetSkillHistory" => Ok(PyValue::List(vec![])),
            // A **dict**, keyed by `boosterTypeID` -- `.iteritems()` and
            // `.itervalues()` are both called on it, so a list is an
            // `AttributeError` exactly as `None` is.
            //
            // The reason this one hid: `crimewatchSvc.GetMyBoosters` does
            //
            //     if hasattr(myGodmaItem, 'boosters'):
            //
            // and **Python 2's `hasattr` swallows every exception**, so
            // the `AttributeError` our `None` caused was eaten and the
            // function quietly returned `[]`. That is why it sat in the
            // server log with no matching traceback -- a silent repeat,
            // once per HUD rebuild, because `skillsvc.boosters` is cached
            // on `is None` and the cache could never fill.
            //
            // It is not silent everywhere: `attributes.Load`, the
            // character sheet's Attributes tab, has no `hasattr` guard.
            //
            // **[I]** Non-empty values must be *mutable* -- `godma` does
            // `setattr(booster, 'typeID', booster.boosterTypeID)`, which
            // a `blue.DBRow` would reject -- so a `KeyVal` each, carrying
            // `boosterTypeID`, `expiryTime` and `boosterDuration`.
            "GetBoosters" => Ok(crate::boosters::get_boosters(reg, req.actor)),
            // **`None` is already correct**, and saying so is the point.
            // The one call site is the last statement of
            // `SkillHistoryProvider.__init__` and the result is `POP_TOP`
            // -- discarded, never stored or tested. It is a side-effect
            // trigger asking the node to push pending skill
            // notifications, so there is nothing to return until we have
            // notifications to push. The only requirement is that it must
            // not *raise*, because `provide()` runs on the same tasklet
            // immediately after.
            "CheckAndSendNotifications" => Ok(PyValue::None),
            // **[I]** on the call name only -- not yet observed on the
            // wire from us, but the shape is not in doubt. Two separate
            // exceptions in one run came from
            // `skillsvc.MyEffectiveSkillLevelsByID` doing `.iteritems()`
            // on the result, under `marketbase.GetMySkills`. A dict, and
            // it is the market window that breaks without it.
            "GetSkills" => Ok(reg.skill_dict(req.actor)),
            // Destructured on arrival as `(skillQueue, freeSkillPoints)`
            // (`skillQueueSvc.PrimeSkillQueue` / `BeginTransaction`, `:93`,
            // `:117`). The queue is real now: see the `Skill training`
            // section below. `freeSkillPoints` is compared `> 0` behind an
            // `is not None` guard; the second item is the free-points pool
            // (`crate::free_sp`), 0 when the pilot has none.
            "GetSkillQueueAndFreePoints" => Ok(PyValue::Tuple(vec![
                skill_queue_rows(reg, req.actor),
                PyValue::Int(req.actor.map_or(0, |id| crate::free_sp::of(reg, id))),
            ])),
            // The unallocated skill points (`crate::free_sp`): the pool, the
            // two previews (`{typeID: points}`) and the three applies.
            "GetFreeSkillPointsAppliedToQueue" => Ok(crate::free_sp::queue_preview(reg, req)),
            "GetFreeSkillPointsAppliedToSkills" => crate::free_sp::skills_preview(reg, req),
            "ApplyFreeSkillPoints" => crate::free_sp::apply_one(reg, req),
            // Skill trading (`crate::skill_trading`): injectors in, an
            // extractor out, split and combine.
            "CheckInjectionConstraints" => crate::skill_trading::check_injection(reg, req),
            "GetDiminishedSpFromInjectors" => crate::skill_trading::diminished_sp(reg, req),
            "InjectSkillpoints" => crate::skill_trading::inject(reg, req),
            "SplitSkillInjector" => crate::skill_trading::split(reg, req),
            "CombineSkillInjector" => crate::skill_trading::combine(reg, req),
            "ExtractSkills" => crate::skill_trading::extract(reg, req),
            "ApplyFreeSkillPointsToQueue" => Ok(crate::free_sp::apply_to_queue(reg, req)),
            "ApplyFreeSkillPointsToSkills" => crate::free_sp::apply_to_skills(reg, req),
            // The skill queue's three writers. All in the `Skill training`
            // section below, with their call sites.
            "SaveNewQueue" => save_new_queue(reg, req),
            "AbortTraining" => abort_training(reg, req),
            // The trained SP total, as an **int**: `skillsvc.py:1153` adds
            // it to the free points (the skill extractor), so `None` was a
            // `TypeError`. The same sum the character sheet makes from
            // `GetAllSkills` itself.
            "GetSkillPoints" => Ok(PyValue::Int(
                req.actor
                    .and_then(|id| reg.skills.get(&id))
                    .map_or(0, |s| s.total_points()),
            )),
            // **These next two must ship together.** Half this fix is a
            // worse state than none of it.
            //
            // `GetTotalSkillPointsForCharacter` is two statements:
            //
            //     trained = sum([s.trainedSkillPoints or 0
            //                    for _, s in reg.GetSkillsIncludingLapsed().iteritems()])
            //     unallocated = reg.GetFreeSkillPoints()
            //
            // backed by two separate `is None` caches over two separate
            // unserved calls. Answering only `GetAllSkills` moves the
            // traceback exactly one line, to `int + NoneType`.
            //
            // Worse than that: the character sheet's
            // `ConstructCharacterTotalSkillPoints` calls this once and
            // *then* installs `AutoTimer(2000, ...)`. Today the first
            // call throws, so the timer is never installed and we see one
            // call per sheet open. The moment the first call succeeds, a
            // 0.5 Hz timer runs for the rest of the session -- and any
            // `is None` cache still holding a `None` becomes the
            // frame-dropping live lock from `docs/guides/debugging-the-client.md` §4.
            // `Int(0)` fills the cache; `None` would be the loop.
            // The same map as `GetSkills`. The two differ only once
            // *lapsed* skills exist, which needs clone grades, which we do
            // not have -- so answering the same thing from both is correct
            // today and is said out loud rather than left to look like a
            // copy-paste.
            "GetAllSkills" => Ok(reg.skill_dict(req.actor)),
            "GetFreeSkillPoints" => Ok(PyValue::Int(
                req.actor.map_or(0, |id| crate::free_sp::of(reg, id)),
            )),
            // **A dict that must have all five keys** -- the Attributes
            // panel subscripts it directly, `[attributeID]`, so `{}` is a
            // `KeyError` and `None` is an `AttributeError` one frame
            // earlier. The fourth row of the §4 table in its subscript
            // form rather than its `.Index()` form.
            //
            // And the values must be **non-zero**, which is the part that
            // would have been easy to get wrong: `skillsvc` feeds each one
            // into `ApplyAttributeChange` for the pilot's own dogma
            // attributes, and every training estimate divides by them.
            // Zero is a divide-by-zero; `None` is worse. 20 is EVE's
            // self-consistent baseline for a new character.
            //
            // **The training clock reads the same function**
            // ([`skill_attribute`]), so the rate the server trains at and the
            // rate the client draws its countdown from cannot disagree.
            "GetAttributes" => Ok(PyValue::Dict(
                [
                    ATTR_CHARISMA,
                    ATTR_INTELLIGENCE,
                    ATTR_MEMORY,
                    ATTR_PERCEPTION,
                    ATTR_WILLPOWER,
                ]
                .into_iter()
                // Dict entries are (value, key) in wire order.
                .map(|id| {
                    let value = req.actor.map_or(STARTING_ATTRIBUTE_FLOOR, |c| {
                        crate::skill_queue::client_attribute(reg, c, id)
                    });
                    (PyValue::Real(value), PyValue::Int(id))
                })
                .collect(),
            )),
            // Keyed by implant slot and `.iteritems()`-ed. Called on the
            // same path as `GetAttributes`, one line before it, so it
            // would have been the next traceback.
            // The head's implants, from the items at `flagImplant`
            // (`crate::implants`, which has the shape and its citations).
            "GetImplants" => Ok(crate::implants::get_implants(reg, req.actor)),
            // Subscripted by name, both keys, so neither `{}` nor `None`
            // works. `nextTimedRespec` is a blue FILETIME -- the same
            // `long`-not-`int` constraint as `createDateTime` -- and
            // pointing it at the character's birth date says "a respec
            // has been available since you were created", which is true.
            "GetRespecInfo" => Ok(crate::skill_queue::respec_info(reg, req.actor)),
            "RespecCharacter" => {
                crate::skill_queue::respec_character(reg, req.actor, req.args.first())
            }
            // A list of `(typeID, pointChange)` pairs, iterated inside a
            // double-checked `is None` cache -- so `None` is a `TypeError`
            // *and* a re-issue, once per ship node drawn in ISIS.
            "GetSkillChangesForISIS" => Ok(PyValue::List(vec![])),
            // Issue #5, step 1: *"skills buy and inject non functional"*.
            // The capture has
            //
            // ```text
            // [437] 141959.663ms OK  skillMgr2*  PurchaseSkills
            //       [N=1:8]([3340, 20342, 3327, 20527])  -> None
            // ```
            //
            // against `characterskills/client/purchase.py(98)`:
            //
            // ```text
            // def send_skill_purchase_request(self, skill_type_ids):
            //     skills_purchased = self.skill_service.GetSkillHandler().PurchaseSkills(skill_type_ids)
            //     self.skills_purchased.extend(skills_purchased)
            //     return True
            // ```
            //
            // `list.extend` on `None` is `TypeError: 'NoneType' object is
            // not iterable`, so the answer has to be **iterable**. It is
            // *not* an echo of the argument: `characterskills/purchase.py`
            // -- the logic both sides share -- returns `skills_to_inject`,
            // the subset that `pay_for_skill` actually got through, built
            // in a `try/finally` so a partial purchase still returns the
            // part that succeeded. A **flat list of skill type ids**, and
            // ours is empty because we bought nothing.
            //
            // **A list of the requested ids would hang the client, not
            // please it**, and that is the part worth writing down.
            // `skillsvc.py(1242)`:
            //
            // ```text
            // def WaitUntilSkillsAreAvailable(self, skillTypeIDs):
            //     skill_type_ids = set(skillTypeIDs)
            //     seen = set()
            //     while skill_type_ids != seen:
            //         blue.pyos.BeNice()
            //         for skill_type_id in skill_type_ids:
            //             if self.GetSkillIncludingLapsed(skill_type_id) is None:
            //                 break
            //             seen.add(skill_type_id)
            // ```
            //
            // and `skills/client/util.py(28)` reaches it as
            // `bought_skills = skill_service.PurchaseSkills(...)` /
            // `if not bought_skills: return False` /
            // `WaitUntilSkillsAreAvailable(missing_skill_type_ids)`. There
            // is no timeout and no escape: a truthy answer is an unbounded
            // `BeNice()` spin, because the skill never becomes available
            // until `GetAllSkills` carries it, which is round 2. So empty
            // is both the honest answer and the only safe one, and the
            // falsy path is the one the client already handles -- that
            // caller returns False and skips `AddSkillsToQueue`.
            //
            // **What the player sees is nothing, and that is a known
            // gap.** `MissingSkillbooksWnd._OnBuyAndInjectButton` takes
            // `success` from `purchase_available_skills`, which returns
            // True on `send_skill_purchase_request` completing, so the
            // window sets `ID_YES` and closes. No skill arrives and no
            // message is shown. We do not raise instead, because the only
            // paths that catch are `SkillPurchaseValidator.validate`'s
            // (which never sees this call) and nothing around
            // `send_skill_purchase_request`, so a `UserError` would escape
            // through `skills/client/util.buy_missing_skills` and break the
            // fitting screen's "train required skills" as well.
            //
            // The client's own switch for this is
            // `globalConfig.IsSkillPurchaseEnabled`, read from
            // `machoNet.GetGlobalConfig()['SkillPurchaseEnabled']`
            // (`globalConfig/const.py(60)`, default True), and
            // `purchase_skills` calls `check_skill_purchase_enabled` first
            // -- which raises `UserError('SkillPurchaseDisabled')`, a
            // label the client already carries. That is the honest "off"
            // switch, in the same spirit as `ValidateInstallJumpClone`
            // above, but it is a push: `InitClientGlobalConfig` is called
            // *on* the client and we never send it. Roadmap, not this arm.
            // **Real now, and the push in `purchase_skills` is half of
            // the answer** -- see its doc comment for why the honest list
            // without the push would be worse than the empty list this used
            // to return.
            "PurchaseSkills" => {
                let ids: Vec<i64> = match req.args.first() {
                    Some(PyValue::List(items)) | Some(PyValue::Tuple(items)) => items
                        .iter()
                        .filter_map(|v| match v {
                            PyValue::Int(i) => Some(*i),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                Ok(PyValue::List(
                    reg.purchase_skills(req.actor, &ids)
                        .into_iter()
                        .map(PyValue::Int)
                        .collect(),
                ))
            }
            _ => Err(reg.no_method(req)),
        },
        "paperDollServer" => match req.method {
            "GetPaperDollDataBatch" => Ok(PyValue::Dict(vec![])),
            // Reached from `GetPortraitSettings`, which unpacks two
            // values and swallows any exception. **[I]** -- inferred from
            // `ccSvc`'s `data, _ = ...GetPaperDollPortraitDataFor(charID)`.
            "GetPaperDollPortraitDataFor" => Ok(PyValue::Tuple(vec![PyValue::None, PyValue::None])),
            // The avatar on the character sheet. `avatarHeaderPreview`
            // calls this **synchronously**:
            //
            //     dna = sm.RemoteSvc('paperDollServer').GetPaperDollData(session.charid)
            //
            // `None` means no DNA, which means no avatar is rendered --
            // the window survives, because `PreviewCharacter` runs under
            // `uthread.new`, but the panel is blank. That is the honest
            // answer for now: a paperdoll DNA is a real structure we do
            // not model, and inventing one would be a different kind of
            // wrong from leaving it empty.
            //
            // **[I]** The shape when we do model it: `_FetchBatchData`'s
            // guard is `if not charInfo or not dna: continue`, so falsy is
            // a supported state throughout.
            "GetPaperDollData" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // Player-owned structures. Both methods take the solar system id
        // and both results are only ever iterated.
        //
        // `GetMyDockableStructures` is a flat collection of structure
        // ids; the client's own fallback one line above the call is
        // `set()`, and `[]` is behaviourally identical for the two things
        // it does with it (iterate, and `in`). We send a list because the
        // marshal writer has no set form.
        // Jump clones. **The call that makes the whole character sheet
        // unusable, not just the Jump Clones tab.**
        //
        // It arrives as a bound call, and in practice fused onto the
        // bind: the capture shows `jumpCloneSvc MachoBindObject(
        // (60012526, 15), ("GetCloneState", (), {}))`, the bindParams
        // being `(session.stationid, const.groupStation)` from
        // `clonejumpsvc.py(84) GetLM`. `handle_bind` dispatches the fused
        // call through this match, so no bind-path entry is needed and
        // one arm serves both forms.
        //
        // **Why one answer is worth three tracebacks.**
        // `characterPanel.py(36) ConstructLayout` sets
        // `reg.isConstructed = True` on line 39, builds
        // `JumpClonesPanel(...)` on line 48, and assigns
        // `reg.tabs = TabGroup(...)` as its **last** statement. The
        // panel's controller `__init__` ends with
        // `if not reg.is_jump_available: ...`, which walks
        // `remaining_time_until_next_jump` -> `next_jump_timestamp` ->
        // `last_jump_timestamp` -> `LastCloneJumpTime` -> `GetCloneState`
        // and raises at line 94. So `reg.tabs` is never assigned while
        // `isConstructed` is already `True`, and the latch makes it
        // permanent: every later tab click dies at `characterPanel.py(78)`
        // in `LoadPanel` and `characterPanel.py(84)` in `UnloadPanel`.
        // That is the difference between a character sheet that opens and
        // one that throws on every click.
        "jumpCloneSvc" => match req.method {
            "GetCloneState" => Ok(clone_state_value(reg, req.actor)),
            // `shipconfig.ShowCloneFacility` does `for each in cloneRS:`
            // and `infosvc.GetCurrentShipCloneCount` takes `len()` of
            // `GetShipClones()`, both unguarded -- so `None` is two more
            // tracebacks waiting on the ship-config clone facility.
            //
            // **And an empty answer here is a re-ask we are choosing on
            // purpose.** `clonejumpsvc.py(98)` caches it, and the guard is
            // truthiness rather than `is None`:
            //
            // ```text
            // def GetShipClones(self):
            //     shipID = eveCfg.GetActiveShip()
            //     if not self.shipJumpClones or shipID != self.shipJustClonesShipID:
            //         lm = self.GetLM()
            //         self.shipJustClonesShipID = shipID
            //         self.shipJumpClones = lm.GetShipCloneState()
            //     return self.shipJumpClones
            // ```
            //
            // `class CRowset(list)` -- **confirmed** in
            // `carbon/common/script/sys/crowset.pyc` -- so a rowset with no
            // rows is *falsy*, `not self.shipJumpClones` stays true, and
            // every call re-issues.
            //
            // **Read from the bytecode, not assumed**, because which
            // live-lock variant this is decides what a fix would have to
            // guarantee (`tools/pyc27.py`, which decodes `co_lnotab`; the
            // line numbers in this comment come from there and not from
            // uncompyle6's reconstructed output, which renumbers):
            //
            // ```text
            //  102  15 LOAD_ATTR     shipJumpClones
            //  102  18 UNARY_NOT                      # <- truthiness, not `is None`
            //  105  64 CALL_FUNCTION                  # GetShipCloneState()
            //  105  70 STORE_ATTR    shipJumpClones   # <- fed straight from the call
            // ```
            //
            // Nothing sits between the `CALL_FUNCTION` and the
            // `STORE_ATTR`, so the store **completes**. That makes this a
            // *third* mechanism in the `docs/guides/debugging-the-client.md` §4
            // family: the LP cache never ran its assignment,
            // `contractProxy`'s ran and stored the one value that fails its
            // own `is None` guard, and this one runs and stores a perfectly
            // correct answer that is merely falsy. So the thing to look at
            // is never the store -- it is the **guard**. There is no
            // traceback and no `UNANSWERED` line, because we answer it: the
            // census found it by reading the client, not by running it.
            //
            // **The cache cannot be made to latch honestly, and we are not
            // going to fake it.** Truthiness in Python 2 is `__nonzero__`
            // then `__len__`, and every container the client would accept
            // here derives its truth from its length -- `CRowset` from
            // `list`, `CIndexedRowset` and `CFilterRowset` from `dict`, and
            // `eve/common/script/sys/rowset.py(72)`'s old-style `Rowset`
            // defines `__len__` too. So a truthy answer means **a row**,
            // and a row means a jump clone that does not exist:
            // `shipconfig.py(143)` would render it as a scroll entry with
            // `cfg.eveowners.Get(each['ownerID'])` and
            // `GetCurrentShipCloneCount` would report it in the Jump Clones
            // status bar. An honest re-ask beats an invented clone.
            //
            // What makes that cheap rather than a frame-dropping loop is
            // the *cadence*, which is the §4 rule for telling the two
            // apart. All three consumers of `GetShipClones` are
            // user-paced, and none sits behind a render handler, an
            // `AutoTimer`, a `debounce` or a `BufferedCall`:
            //
            // - `shipconfig.py(135) ShowCloneFacility` -- the Ship
            //   Configuration window's Clone Facility tab, on tab select
            //   and on `OnShipCloneJumpUpdate`. It iterates and `len()`s,
            //   and it has its own empty-state text at line 151,
            //   `NoClonesInstalledAtShip`, so empty *displays correctly*.
            // - `infosvc.py(710) GetCurrentShipCloneCount` -- `len()`,
            //   reached from `GetStatusBarDataForAttribute` (line 774) while
            //   *constructing* the ship info window's Attributes and Fitting
            //   panels, and only after `total == 0.0` fails for
            //   `attributeMaxJumpClones`.
            // - `clonejumpsvc.py(143) DestroyInstalledClone` -- guarded by
            //   `if shipClones:` before `.Index('jumpCloneID')`, so falsy
            //   is the path that is already correct.
            //
            // So the cost is one extra call per panel build, forever, and
            // it ends the day a real clone exists. Revisit if a capture
            // ever shows this call in the hundreds: that would mean a
            // consumer we have not found.
            //
            // **`GetStationCloneState` costs us nothing today.**
            // `GetStationClones` has the simpler `if not
            // self.stationJumpClones:` guard, so it would re-ask the same
            // way -- but nothing in the tree calls it. The only reference
            // is `clonejumpsvc.__exportedcalls__`, which is the list of
            // methods the *cluster* may call on this client service, so
            // there is no client-side consumer to drive a loop. Confirmed
            // by `grep -ral GetStationClones out/pyc/`, which matches
            // `clonejumpsvc.pyc` and nothing else. It shares the arm
            // because it shares the shape, not because it shares a caller.
            "GetShipCloneState" => Ok(crate::ship_clone::state_value(reg, req.actor)),
            "GetStationCloneState" => Ok(ship_clones_rowset()),
            // `if cost > 0.0:` -- and Python 2 orders `None` *below* every
            // number, so `None` takes the right branch with no error and
            // is a wrong answer with no symptom at all. `FmtISK(None)` in
            // `OnShipJumpCloneInstallationOffered` is less forgiving.
            "GetPriceForClone" => Ok(PyValue::Real(0.0)),
            // **A list of one label path, and no longer empty.**
            //
            // `ValidateInstallJumpClone` wraps only the *call* in
            // `except Exception`; the `for label in error_labels:` that
            // follows is outside it, so `None` raises where the client's
            // own handler cannot catch it. But an empty list means "no
            // objections", which *enables the Install Clone button* -- and
            // clicking it reaches `InstallCloneInStation`, which we cannot
            // perform. The capture has two such clicks 1.5 s apart, both
            // answered `None`, no traceback: **the button did nothing and
            // said nothing.**
            //
            // `InstallCloneBanner.update` is
            //
            //     errors = reg.controller.validate_install_clone()
            //     if errors:
            //         reg.install_button.Disable()
            //         reg.install_button.LoadTooltipPanel = functools.partial(
            //             load_errors_tooltip, errors=errors)
            //     else:
            //         reg.install_button.Enable()
            //
            // and `clonejumpsvc.ValidateInstallJumpClone` renders each
            // element with `GetByLabel(label)` for a `basestring`, or
            // `GetByLabel(label, **args)` for a `(label, args)` pair. So
            // the honest answer is a greyed-out button with the reason in
            // its tooltip, rather than a dead click.
            //
            // A Python 2 `str` is required, not a tuple: the
            // `isinstance(label, basestring)` branch is what picks the
            // one-argument `GetByLabel`, and anything else is unpacked as
            // a pair inside an `ExceptionEater` and silently dropped.
            //
            // **Clone install works now (P5.9), so this is real**: empty
            // when an install would succeed, else the reason, as one of the
            // client's own `UI/Medical/Errors/` labels. See
            // [`install_errors`] for which and why.
            "ValidateInstallJumpClone" => Ok(PyValue::List(
                req.actor
                    .map(|id| install_errors(reg, id))
                    .unwrap_or_else(|| vec![CLONE_ERR_NOT_DOCKED])
                    .into_iter()
                    .map(bstr)
                    .collect(),
            )),
            // The Install button. The client does `POP_TOP` on the result;
            // what it needs is `OnJumpCloneCacheInvalidated`, because
            // nothing on its side drops the cached list. [`install_jump_clone`].
            "InstallCloneInStation" => install_jump_clone(reg, req),
            // The same install, from a pilot docked in a structure (U5):
            // `InstallCloneInStation` when `session.stationid` is set, this
            // when only `structureid` is (`clonejumpsvc.py:156-158`). The
            // station of [`install_jump_clone`] is the structure.
            "InstallCloneInStructure" => install_jump_clone(reg, req),
            // `clonejumpsvc.GetNumClonesInPilotsStructure`, asked by the
            // "take this module offline" prompt of a cloning bay (`eveMisc.
            // _GetExtraParams`): how many jump clones the structure holds.
            // `None` outside one, which the client reads as "no count".
            "GetNumClonesInPilotsStructure" => Ok(req
                .actor
                .and_then(|a| crate::structure_dock::docked_in(reg, a))
                .map_or(PyValue::None, |id| {
                    PyValue::Int(crate::clones::clones_at(reg, id) as i64)
                })),
            // `clonejumpsvc.py(143) DestroyInstalledClone` -> `lm.
            // DestroyInstalledClone(cloneID)` at `:188`, result discarded.
            // GAPS-2: a clone kept in a ship is destroyed by its own rule.
            "DestroyInstalledClone" => match super::gaps2::destroy_ship_clone(reg, req) {
                Some(answer) => answer,
                None => destroy_jump_clone(reg, req),
            },
            // `clonejumpsvc.py(80)` -> `lm.SetJumpCloneName(cloneID,
            // newName)` at `:82`, result discarded.
            "SetJumpCloneName" => rename_jump_clone(reg, req),
            // `clonejumpsvc.py(258) _CloneJump` hands this to
            // `PerformSessionChange('clonejump', lm.CloneJump, destLocationID,
            // cloneID, cost, confirmed)` at `:263`: the session change *is*
            // the jump, like the dock. [`clone_jump`].
            "CloneJump" => clone_jump(reg, req),
            // GAPS-2: installing a clone in a ship.
            _ => match super::gaps2::jump_clone(reg, req) {
                Some(answer) => answer,
                None => Err(reg.no_method(req)),
            },
        },
        // The jump-fatigue timers, and a **3-tuple that is splatted
        // before anything is tested**:
        //
        //     reg.OnJumpTimersUpdated(doAlert=False, *reg.crimewatchSvc.GetJumpTimers())
        //
        // into `OnJumpTimersUpdated(self, jumpActivation, jumpFatigue,
        // lastUpdated, doAlert=False)`. The client said so in as many
        // words: `TypeError: OnJumpTimersUpdated() argument after * must
        // be a sequence, not NoneType`.
        //
        // **Zeros, not `None`s.** Both are falsy, so the HUD takes the
        // `DeleteTimer` branch either way -- but
        // `markerTooltipUtil._ReactivationTimerAfterJump` does
        // `max(0, jumpFatigueExpiryTime - now)` with no truthiness guard,
        // and in Python 2 `None - int` is a `TypeError`. That path is off
        // the docked route; zeros cost nothing and close it.
        //
        // The throw was at statement 8 of 10 in
        // `OnCombatTimersUpdated`, so fixing it also lets
        // `OnWarStatusUpdated` run -- which is the second
        // `warRegistry.GetWars` trigger, and we already serve that.
        "jumpTimers" => match req.method {
            // CAP-F: the real timers (`jump_fatigue`), zeros when over or
            // when there is no acting character (the shape above).
            "GetTimers" => {
                let now = crate::pyutil::filetime_now();
                let t = match req.actor {
                    Some(c) => crate::jump_fatigue::timers(reg, c, now),
                    None => crate::jump_fatigue::Timers::default(),
                };
                Ok(PyValue::Tuple(t.wire()))
            }
            _ => Err(reg.no_method(req)),
        },
        // Standings: who likes whom. Started **lazily** -- nothing in the
        // tree lists `standing` in `__dependencies__` except `spaceView`
        // and the resource-wars service -- so it comes up the first time
        // something calls `sm.GetService('standing')`. On the docked path
        // that is the station Lobby's Agents tab:
        //
        //     def find_available_and_unavailable_agents(...):
        //         epic_arc_status_service = sm.RemoteSvc('epicArcStatus')
        //         standing_service = sm.GetService('standing')
        //
        // which is unconditional and *before* the per-agent loop, so it
        // fires even though our `agentMgr.GetAgents` rowset is empty.
        // Service startup is serialised in `ServiceManager.GetService`,
        // so every other `GetService('standing')` queues behind the first.
        //
        // `Standing.Run` calls `__RefreshStandings`, which issues the
        // first three methods below back to back on one tasklet --
        // so expect them in a single wave:
        //
        //     def __RefreshStandings(self):
        //         if not session.charid: return
        //         standingsMgr = sm.RemoteSvc('standingMgr')
        //         tmp = standingsMgr.GetNPCNPCStandings()
        //         reg.npcnpcstandingsto = tmp.Filter('toID')
        //         reg.npcnpcstandingsfrom = tmp.Filter('fromID')
        //         if idCheckers.IsNPC(session.corpid):
        //             reg.npccharstandings = standingsMgr.GetCharStandings()
        //             reg.npccorpstandings = {}
        //         else:
        //             ret = uthread.parallel([...])
        //
        // `ProcessSessionChange` re-runs it whenever `charid` or `corpid`
        // changes, i.e. on every entry into the game.
        //
        // One trap that is not in this file: straight after the calls,
        // `__RefreshStandings` does
        //
        //     for factionID in appConst.factions:
        //         if factionID not in reg.npccharstandings and \
        //            factionID == appConst.factionByRace[session.raceID]:
        //
        // -- a **bare subscript** on `factionByRace`, which has exactly
        // four keys (raceCaldari=1, raceMinmatar=2, raceAmarr=4,
        // raceGallente=8). With our empty `npccharstandings` the loop body
        // is reached, so a `raceID` outside those four is a `KeyError`
        // *after* we answered correctly. Ours is 1.
        "standingMgr" => match req.method {
            // **Must be a real rowset object**, and this is the strictest
            // kind of caller there is: `.Filter(...)` is called on the
            // result twice, unconditionally, with no truthiness test and
            // no `isinstance` escape. `None`, `{}`, `[]` and the
            // `(header, data)` prime tuple all raise `AttributeError`.
            //
            // `Rowset.Filter` builds `FilterRowset(reg.header,
            // reg.lines, colname)`, whose `__init__` does
            // `header.index(idName)` -- so both names have to be *in* the
            // header even with no rows.
            //
            // Three columns, all read off the client:
            //   `fromID`   -- Filter key, and the dict key in GetStanding
            //   `toID`     -- Filter key, and read as `each.toID`
            //   `standing` -- read as `standing.standing`
            //
            // `Filter('toID')` is pure overhead for us:
            // `npcnpcstandingsto` is written in `Run`/`__RefreshStandings`
            // and **never read anywhere in this build**. The call exists,
            // so the column has to.
            //
            // Empty is the right content: `fromID in npcnpcstandingsfrom`
            // is then false and `GetStanding` falls through to
            // `appConst.contactNeutralStanding` (0).
            "GetNPCNPCStandings" => Ok(npc_npc_standings()),
            // A **plain dict**, and uniquely for this project we can prove
            // a dict is wanted rather than infer it -- the caller says so:
            //
            //     if not isinstance(reg.npccharstandings, types.DictType):
            //         reg.npccharstandings = reg.npccharstandings.Index('fromID')
            //
            // so a dict skips the rowset path entirely. Every later
            // consumer is dict-compatible (`in`, `[k]`, iteration,
            // `__setitem__`).
            //
            // **Non-empty now** (P5.7): the values are `DBRow`s over the
            // client's own `('fromID' I4, 'standing' R5)` header, which is
            // what `_AppendNPCCharStanding` puts in this same dict -- so the
            // dict path above is kept and the `CRowset` guess this comment
            // used to carry is not needed. See [`char_standings_value`] in
            // `crate::standings`.
            "GetCharStandings" => Ok(char_standings_value(reg, req.actor)),
            // Not reached by our character, and worth knowing why rather
            // than discovering it later. The `uthread.parallel` branch
            // above runs only when `not idCheckers.IsNPC(session.corpid)`,
            // and `IsNPC` is `9999 < id < 90000000`; our corp is 1000001,
            // an NPC corp, so the client sets `npccorpstandings = {}`
            // locally and never asks. Answered anyway, same shape, for the
            // day a player corp exists.
            "GetCorpStandings" => Ok(crate::corp_standings::corp_standings_value(
                reg,
                req.actor,
                crate::pyutil::filetime_now(),
            )),
            // Off the docked path -- the character sheet's Standings panel
            // -- but cheap to answer.
            //
            // Must be **`list`-like, not a `Rowset`**: `standingsGraph`
            // does `reg.transactions.reverse()`, which a `Rowset` has no
            // method for. A plain list and a `CRowset` (a `list` subclass)
            // both work; empty is not iterated at all.
            //
            // **[I]** Non-empty rows are read for `modification`,
            // `eventTypeID`, `eventDateTime`, `fromID`, `int_1..3` and
            // `msg`, and must allow attribute *assignment* -- the caller
            // does `each.modification = float(each.modification * 10.0)`.
            // That is the set of fields the client reads, not necessarily
            // the whole row.
            "GetStandingTransactions" => Ok(PyValue::List(vec![])),
            // Also off the docked path: only when the Standings panel is
            // looking at a corporation. Iterated and nothing else, so
            // empty needs no columns. **[I]** rows carry `ownerID` and
            // `standing`.
            "GetStandingCompositions" => Ok(crate::corp_standings::compositions_value(
                reg,
                req.args,
                crate::pyutil::filetime_now(),
            )),
            _ => Err(reg.no_method(req)),
        },
        // The live mission tracker (`missions/client/
        // missionObjectivesTracker.py`). Reached only once the journal has
        // an accepted mission in it, which it now can. A plain
        // `sm.RemoteSvc` held on an attribute, not a bound object
        // (`docs/re/agents-missions.md` §10.2).
        "missionTrackerMgr" => match req.method {
            // `missionObjectives[agentID]` is guarded on truthiness and
            // stored as returned (`:309-311`) **[V]**: an empty list is a
            // correct answer that does not latch, re-asked once per
            // re-arming event (`docs/re/agents-missions.md` §5).
            "GetAllMissionObjectives" => Ok(PyValue::List(vec![])),
            // `missionInfo or None` is stored per agent (`:278-279`) **[V]**:
            // `None` is "no current objective", which is honest -- we push
            // no live objective rows.
            "GetMissionInfoItems" => Ok(PyValue::None),
            // Returned straight to `IsInActiveMissionDungeon` (`:320`)
            // **[V]**. `False` is `[A]`: we raise no `OnDungeonEntered`, so
            // nothing would agree with a `True`.
            "IsInActiveDungeonID" => Ok(PyValue::Bool(false)),
            // Inside `ExceptionEater` and its result discarded (`:229-231`)
            // **[V]**: an ask to re-evaluate objectives we evaluate lazily.
            "UpdateAllMissions" => Ok(PyValue::None),
            _ => Err(reg.no_method(req)),
        },
        // The character sheet's expert-system chip. Iterated by
        // `expertSystems.client.util`, and cached on `is None`, so the
        // usual pair of reasons not to answer `None`.
        "expertSystemMgr" => match req.method {
            "GetMyExpertSystems" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // The home station, and **a machoNet service despite the
        // snake_case name** -- it is reached through
        // `ServiceManager.Instance().RemoteSvc('home_station')` like any
        // other, and it is not the protobuf gateway.
        //
        // This is the character sheet's *default first tab*
        // (`GetDefaultPanelID` falls back to `PANEL_HOME_STATION`), so it
        // is not an optional corner of the window.
        //
        //     data = reg.remote.get_home_station()
        //     reg._home_station = Station(station_id=data.id,
        //                                  type_id=data.type_id,
        //                                  solar_system_id=data.solar_system_id,
        //                                  is_fallback=data.is_fallback)
        //
        // Four attributes, read unconditionally -- there is no "no home
        // station" branch, and `_UpdateHomeStationText` goes on to read
        // `reg.home_station.id` with no guard either. So "nowhere to call
        // home" still has to be a real object, and `is_fallback` is how
        // the client is told it is a default rather than a choice. Until the
        // player sets one, we point it at the station they are docked in,
        // which is both true and the thing the player would expect; once
        // they have, it is that station and `is_fallback` is false. The
        // clones section ([`home_of`]) owns the rule.
        "home_station" => match req.method {
            "get_home_station" => {
                let Some((home, fallback)) = req.actor.and_then(|id| home_of(reg, id)) else {
                    return Ok(PyValue::None);
                };
                Ok(keyval(vec![
                    ("id", PyValue::Int(home.station_id)),
                    ("type_id", PyValue::Int(home.type_id)),
                    ("solar_system_id", PyValue::Int(home.solar_system_id)),
                    ("is_fallback", PyValue::Bool(fallback)),
                ]))
            }
            // The Select Home Station page. [`home_station_candidates`].
            "get_home_station_candidates" => Ok(home_station_candidates(reg, req.actor)),
            // **[V]** `ui/page/select.py(390) RemoteChangeInfoCard.load`:
            // `if next_remote_change_time is None: self.display = False`.
            // `None` is "no cooldown", which is true: a remote change is
            // never accepted here (see [`set_home_station`]), so none is
            // ever running.
            "get_next_remote_change_time" => Ok(PyValue::None),
            // `homestation/client/service.py(150)` -> `remote.
            // set_home_station(new_home_station_id, allow_remote)` at `:155`,
            // result discarded. [`set_home_station`].
            "set_home_station" => set_home_station(reg, req),
            // `service.py(160)`: `sessionMgr.PerformSessionChange('clonejump',
            // remote.self_destruct_clone)` -- the method itself is the
            // session-change function. [`self_destruct_clone`].
            "self_destruct_clone" => self_destruct_clone(reg, req),
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

    /// Every byte string anywhere in a value, for asserting on the column
    /// names of a rowset whose descriptor is itself an `ObjectEx`. A debug
    /// rendering prints them as numbers, so they have to be looked for as
    /// bytes.
    fn collect_bytes(v: &PyValue, out: &mut Vec<Vec<u8>>) {
        match v {
            PyValue::Bytes(b) => out.push(b.clone()),
            PyValue::Tuple(items) | PyValue::List(items) => {
                for i in items {
                    collect_bytes(i, out);
                }
            }
            PyValue::Instance { state, .. } => collect_bytes(state, out),
            PyValue::Dict(entries) => {
                for (v, k) in entries {
                    collect_bytes(k, out);
                    collect_bytes(v, out);
                }
            }
            PyValue::ObjectEx {
                header, list, dict, ..
            } => {
                collect_bytes(header, out);
                for i in list {
                    collect_bytes(i, out);
                }
                for (k, v) in dict {
                    collect_bytes(k, out);
                    collect_bytes(v, out);
                }
            }
            _ => {}
        }
    }

    /// A `CRowset`'s `(descriptor, rows)`, which live in its ObjectEx
    /// header tuple rather than in the list-append stream.
    fn crowset_args(v: &PyValue) -> (PyValue, Vec<PyValue>) {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("expected a CRowset ObjectEx, got {v:?}")
        };
        let PyValue::Tuple(head) = header.as_ref() else {
            panic!("a CRowset header is (class, args), got {header:?}")
        };
        let PyValue::Tuple(args) = &head[1] else {
            panic!("a CRowset's args must be a tuple, got {:?}", head[1])
        };
        assert_eq!(args.len(), 2, "CRowset(descriptor, rows)");
        let PyValue::List(rows) = &args[1] else {
            panic!("a CRowset's rows must be a list, got {:?}", args[1])
        };
        (args[0].clone(), rows.clone())
    }

    /// Both clone-state calls must answer a **`CRowset` with no rows and
    /// its columns declared**, and this test exists to pin the *falsy*
    /// part on purpose rather than by accident.
    ///
    /// `clonejumpsvc.py(102)` guards its cache with `if not
    /// self.shipJumpClones`, and `class CRowset(list)` -- so our empty
    /// answer is falsy, the cache never latches, and the client re-asks
    /// once per Clone Facility tab build and once per ship info panel
    /// build. That re-ask is a documented choice: see the arm. A truthy
    /// answer would require a row, and a row is a jump clone that does not
    /// exist, which `shipconfig.ShowCloneFacility` would then draw.
    ///
    /// So: if someone ever adds a row to silence the re-ask, this test
    /// fails and points at the comment. If the sandbox grows a real clone,
    /// this test is the one to change, deliberately.
    ///
    /// The columns are pinned because `shipconfig.py(143)` reads
    /// `each['ownerID']` and `each['jumpCloneID']` **by column name**, and
    /// `CFilterRowset.Build` resolves a column name before it looks at the
    /// row count -- so they have to be declared even with nothing in them.
    #[test]
    fn both_clone_state_calls_are_an_empty_crowset_with_its_columns_named() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in ["GetShipCloneState", "GetStationCloneState"] {
            let got = call(&mut reg, "jumpCloneSvc", method).unwrap();
            let (descriptor, rows) = crowset_args(&got);
            assert!(
                rows.is_empty(),
                "{method} must stay empty: a row here is an invented jump \
                 clone that shipconfig.ShowCloneFacility would render"
            );

            let mut names: Vec<Vec<u8>> = Vec::new();
            collect_bytes(&descriptor, &mut names);
            for column in ["jumpCloneID", "ownerID", "locationID", "typeID"] {
                assert!(
                    names.iter().any(|n| n == column.as_bytes()),
                    "{method} must declare {column} even with no rows, because \
                     the client resolves column names before row counts: {:?}",
                    names
                        .iter()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    /// `PurchaseSkills` must answer a **list**, and an empty one.
    ///
    /// `characterskills/client/purchase.py(98)` does
    /// `self.skills_purchased.extend(<our answer>)`, so `None` is
    /// `TypeError: 'NoneType' object is not iterable` -- the traceback in
    /// issue #5.
    ///
    /// The second half is the one that changed, and the rule survived the
    /// change rather than being dropped.
    ///
    /// `skills/client/util.py(28)` reads the same answer as `if not
    /// bought_skills: return False` and otherwise calls
    /// `skillsvc.WaitUntilSkillsAreAvailable`, which is a
    /// `while skill_type_ids != seen: blue.pyos.BeNice()` loop with no
    /// timeout. **`seen` is filled from the client's own already-populated
    /// cache, so that loop issues no service call at all** -- no answer of
    /// ours, at any shape or cadence, can end it. Only a push that resets the
    /// cache can.
    ///
    /// So while skills could not exist, the empty list was the only safe
    /// answer, and this test asserted emptiness. Now that they can, the
    /// invariant is not "empty" but **"a non-empty list never ships without
    /// the push"** -- because the truthful list on its own would be a
    /// 100%-CPU tasklet, which is strictly worse than the silent no-op the
    /// empty list gave us.
    #[test]
    fn a_purchase_that_grants_anything_also_pushes_the_refresh() {
        let mut reg = Registry::new(SandboxConfig::default());
        // Real skill type ids, read out of the client's own `types.fsdbinary`:
        // 3300 Gunnery, 3302 Small Projectile Turret, 3449 Navigation. 3327
        // (Spaceship Command) is deliberately absent -- it is in the starter
        // set, so asking for it would be refused as already known and that is
        // a different test.
        let got = call_args(
            &mut reg,
            "skillMgr2::skillHandler",
            "PurchaseSkills",
            &[PyValue::List(vec![PyValue::Int(3302), PyValue::Int(3443)])],
        )
        .unwrap();
        let PyValue::List(bought) = &got else {
            panic!("PurchaseSkills is `extend`ed, so it must be a list, got {got:?}")
        };
        assert_eq!(bought.len(), 2, "both are real skills and neither is known");
        assert_eq!(
            reg.take_pending()
                .iter()
                .map(|(_, n)| n.to_push())
                .collect::<Vec<_>>(),
            vec![("OnSkillForcedRefresh", vec![])],
            "a non-empty answer MUST be accompanied by the push: without it \
             the client busy-waits forever on a cache only the push resets"
        );
    }

    /// A refusal is the empty list, and it carries no push.
    ///
    /// The all-or-nothing rule: `buy_missing_skills` waits on the *request*
    /// list rather than our answer, so declining one id of several leaves the
    /// client spinning on the ones we declined -- exactly as if we had
    /// declined all of them. So a partially-valid request grants nothing.
    #[test]
    fn a_partially_valid_purchase_grants_nothing_and_pushes_nothing() {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call_args(
            &mut reg,
            "skillMgr2::skillHandler",
            "PurchaseSkills",
            // One real skill and one id that is not a skill at all.
            &[PyValue::List(vec![PyValue::Int(3302), PyValue::Int(20527)])],
        )
        .unwrap();
        let PyValue::List(bought) = &got else {
            panic!("must be a list, got {got:?}")
        };
        assert!(
            bought.is_empty(),
            "all or nothing: a partial answer is the same spin as a refusal"
        );
        assert!(
            reg.take_pending().is_empty(),
            "nothing changed, so there is nothing to tell the client"
        );
    }

    /// `charMgr.ListIncludingContainers` must answer a **list of item
    /// rows**, from the shared helper and not from a shape of its own.
    ///
    /// `invCache.py(963)` iterates it and reads `realItem.itemID` as an
    /// attribute; `assetsWindow.py(531)` takes `len()` of it one line
    /// later. `None` was `TypeError: 'NoneType' object is not iterable`.
    ///
    /// The row shape is the round's one cross-lane contract: this window
    /// merges our answer with `invbroker.GetContainerContents` into one
    /// dict keyed by `itemID`, so both must come from
    /// `Registry::owned_item_rows()`. This test compares against that
    /// helper directly, so defining a local row shape here fails it.
    #[test]
    fn list_including_containers_is_the_shared_owned_item_rows() {
        let mut reg = Registry::new(SandboxConfig::default());
        let expected = reg.owned_item_rows(actor(&reg));
        let got = call(&mut reg, "charMgr", "ListIncludingContainers").unwrap();
        let PyValue::List(rows) = &got else {
            panic!(
                "ListIncludingContainers is iterated and len()'d, so it must be a list, got {got:?}"
            )
        };
        assert_eq!(
            rows, &expected,
            "the row shape belongs to Registry::owned_item_rows(), shared with \
             invbroker.GetContainerContents; a local shape splits the assets window"
        );

        // Every row must be a `blue.DBRow` carrying `itemID`, because the
        // client's cache key is `realItem.itemID` -- an attribute read, not
        // a subscript, which only the DBRow family answers.
        for row in rows {
            let PyValue::DbRow(r) = row else {
                panic!("item rows must be blue.DBRow for `realItem.itemID`, got {row:?}")
            };
            assert!(
                r.descriptor().index_of(b"itemID").is_some(),
                "invCache.py(963) keys its cache on realItem.itemID"
            );
        }
    }

    /// `charMgr.GetLabels` must answer a **dict**, and `{}` rather than
    /// `None` is what stops the loop.
    ///
    /// `addressbookService.GetContactLabels` caches on `if self.labels is
    /// None:` and the assignment *succeeds*, so `None` stores the one value
    /// that fails its own guard: the cache never latches and every rebuild
    /// re-issues the call, while every consumer of the returned `None`
    /// raises. An empty dict latches on the first call.
    ///
    /// The family is forced by `CreateContactLabel`, which does
    /// `labels[labelID] = utillib.KeyVal(...)` **into our answer**, and by
    /// `contactsForm.py(277)`'s `.itervalues()` -- so no rowset of either
    /// family survives, and a list is an `AttributeError` exactly as `None`
    /// is.
    #[test]
    fn get_labels_is_an_empty_dict_so_the_address_book_cache_latches() {
        let mut reg = Registry::new(SandboxConfig::default());
        let got = call(&mut reg, "charMgr", "GetLabels").unwrap();
        assert_eq!(
            got,
            PyValue::Dict(vec![]),
            "a new character has no labels of their own; the static groups come \
             from contactsForm.GetStaticLabelsGroups, not from us"
        );
        assert_ne!(
            got,
            PyValue::None,
            "None is stored successfully by an `is None` guard, which is the \
             poisoned-cache live lock"
        );
    }

    /// `charMgr.GetSettingsInfo` must answer the exact shape
    /// `evecrypto/placebo.py(22)` subscripts:
    ///
    /// ```text
    /// def verify(signedData):
    ///     return (signedData[0], signedData[1] == 0)
    /// ```
    ///
    /// A `None` here was `TypeError: 'NoneType' object is not subscriptable`
    /// two frames below `settingsSvc.py(205)`, and we only learned that
    /// because the client mailed us the traceback. This test is the thing
    /// that fails instead, next time.
    #[test]
    fn get_settings_info_answers_a_blob_and_a_zero() {
        let mut reg = Registry::new(SandboxConfig::default());
        let PyValue::Tuple(t) = call(&mut reg, "charMgr", "GetSettingsInfo").unwrap() else {
            panic!("GetSettingsInfo must answer a 2-sequence; placebo.verify subscripts it");
        };
        assert_eq!(
            t.len(),
            2,
            "signedData[0] and signedData[1] must both exist"
        );

        // signedData[1] == 0 is what sets `verified`; anything else and
        // settingsSvc.py(207) raises RuntimeError('Failed verifying blob').
        assert_eq!(t[1], PyValue::Int(0));

        // signedData[0] goes straight to `marshal.loads`, so it has to be a
        // Python 2 `str` -- Bytes, not Unicode -- and it has to be the code
        // object pycode emits, byte for byte.
        assert_eq!(t[0], PyValue::Bytes(crate::pycode::settings_info_func()));

        // The two bytes that make it the right *kind* of code object, spelled
        // out here so this test fails on its own evidence rather than only
        // on a cross-module equality: TYPE_CODE, and BUILD_MAP as the first
        // opcode. `BUILD_MAP 0; RETURN_VALUE` is what makes
        // settingsSvc.py(210)'s `len(ret) > 0` false.
        let PyValue::Bytes(blob) = &t[0] else {
            unreachable!()
        };
        // Byte 22 is the first byte of co_code: after the 'c' tag, the four
        // i32 header fields, the 's' tag and the i32 length.
        assert_eq!(blob[0], b'c', "marshal TYPE_CODE");
        assert_eq!(blob[22], 105, "BUILD_MAP");
        assert_eq!(blob[25], 83, "RETURN_VALUE");
    }

    /// `settingsSvc.py(211)` calls this when the blob returns something
    /// non-empty. Ours does not, so this is unreachable today -- but an
    /// unregistered method is a refused call, and the point of answering it
    /// is that a future change to the blob cannot hang a client.
    ///
    /// `None` is correct because the call site is `CALL_FUNCTION 1; POP_TOP`.
    #[test]
    fn log_settings_is_answered_rather_than_refused() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(
            call(&mut reg, "charMgr", "LogSettings").unwrap(),
            PyValue::None
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

    /// `GetPublicInfo3` is a **one-element sequence**, because the client
    /// subscripts `info[0]`.
    ///
    /// And the row carries `militiaFactionID`, which `:216` reads as a plain
    /// `LOAD_ATTR` -- the trap in this shape. The property is "every
    /// attribute the window reads unguarded is present", not "the row has
    /// seven fields".
    #[test]
    fn public_info3_is_one_row_carrying_every_unguarded_attribute() {
        let mut reg = Registry::new(SandboxConfig::default());
        let actor = actor(&reg).expect("the fixture has a character");
        let answer = call_args(
            &mut reg,
            "charMgr",
            "GetPublicInfo3",
            &[PyValue::Int(actor)],
        )
        .expect("GetPublicInfo3 answers");
        let PyValue::List(rows) = &answer else {
            panic!("the client does `info[0]`, so a sequence: {answer:?}")
        };
        assert_eq!(rows.len(), 1, "a CRowset of exactly one row");
        let PyValue::Instance { state, .. } = &rows[0] else {
            panic!("the row is an attribute bag: {:?}", rows[0])
        };
        for name in [
            "description",
            "startDateTime",
            "militiaFactionID",
            "bloodlineID",
            "corporationID",
        ] {
            assert!(
                crate::pyutil::dict_get(state, name.as_bytes()).is_some(),
                "{name} is read unguarded by characterInfoWindow"
            );
        }
        // Read with `getattr(..., None)`, so its absence is the right answer
        // and a value would turn a decorations icon on for nobody.
        assert!(
            crate::pyutil::dict_get(state, b"medal1GraphicID").is_none(),
            "medal1GraphicID is a getattr with a default"
        );
    }

    /// The call that was never broken, and the diagnostic that said it was.
    ///
    /// `skillMgr2.GetMySkillHandler` is reached through
    /// `OBJECT_RETURNING`, which dispatches, discards the answer and mints
    /// the reference. `handle_object_returning_call` swallows `NoSuchMethod`
    /// but reports every other error -- and this service was in no module's
    /// `SERVICES`, so it reported `NoSuchService` on **every login** for a
    /// call that was answered correctly two lines later.
    ///
    /// The property is that the dispatcher reaches an arm. Asserting the
    /// `None` would be asserting the thing the caller throws away.
    #[test]
    fn skill_mgr2_is_routed_so_the_diagnostic_stops_crying_wolf() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert!(
            matches!(
                call(&mut reg, "skillMgr2", "GetMySkillHandler"),
                Ok(PyValue::None)
            ),
            "the handle-getter must reach an arm, or every login logs an \
             UNANSWERED line for a call that worked"
        );
        assert!(
            matches!(
                call(&mut reg, "skillMgr2", "__no_such_method__"),
                Err(ServiceError::NoSuchMethod { .. })
            ),
            "and the service must not swallow everything else"
        );
    }
}
