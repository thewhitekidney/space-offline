//! The map, stations, structures and search.
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
    "map",
    "stationSvc",
    "station",
    "search",
    "accessGroupBookmarkMgr",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // The station view. None of these run in the blocking
        // session-change wave, which is why an unanswered one gives a
        // loading screen rather than a black one -- and why a *wrong*
        // one here is merely a logged traceback and an empty view:
        // `viewStateSvc._OpenView` wraps `LoadView` in a bare
        // `except: log.LogException()` and its `finally` calls
        // `HideAllLoad()`.
        //
        // `GetStationInfo` is cached in `uisvc` for the whole process,
        // so this is one call per client run. The result must have
        // `.Index('stationID')`, so a real rowset; the four columns
        // below are the complete set anything reads, across all six
        // call sites. Note `solarSystemID`'s capital S -- the agent
        // rowset below spells the same idea all lowercase.
        "map" => match req.method {
            // MAP-STATS: `GetHistory(statID, hours)`, the hourly jump and
            // kill counter as a `CachedMethodCallResult` (`map_stats.rs`).
            "GetHistory" => Ok(crate::map_stats::history_answer(reg, req.args)),
            // The incursion report, **a list of mutable rows**.
            // `contentProviderIncursions.py(39) GetAllIncursions` does
            // `sorted(incursions, key=self._GetSortKey)`, and `sorted(None)`
            // is the `TypeError` in the capture.
            // `_ConstructContentPieces(32)` then has `if not incursions:
            // return`, so `[]` is reached *and* short-circuits: the Agency's
            // Incursions tab draws empty instead of throwing.
            //
            // `[]` and **not a `CRowset`**, though "report" sounds like one:
            // `journal.ShowIncursionTab` writes back onto each row --
            // `data.jumps`, `data.influenceData`, `data.security`,
            // `data.constellationID`, `data.factionID`,
            // `data.loyaltyPoints` -- and a packed `DBRow` refuses
            // `STORE_ATTR`. The day there is an incursion each row must be
            // `KeyVal`-like, so a rowset would be the wrong *family* rather
            // than merely a heavier container.
            "GetIncursionGlobalReport" => Ok(PyValue::List(vec![])),
            // `facWarSvc.GetFacWarZoneInfo(factionID)` (`facWarSvc:147`) is
            // `map.GetFacWarZoneInfo`, and its one reader is
            // `GetSolarSystemUpgradeLevel` (`facWarSvc:68`):
            // `warZoneInfo.systemUpgradeLevel.get(solarsystemID, None)`. Our
            // `None` was `AttributeError: 'NoneType' object has no attribute
            // 'systemUpgradeLevel'` in `InfoPanelFactionalWarfare` at every
            // system change (38 calls, 37 tracebacks in the 2026-10-28
            // client logs). A `KeyVal` whose `systemUpgradeLevel` is an
            // empty dict is "no system here has an upgrade level" -- true,
            // there is no warzone; `.get` then answers `None`, the caller's
            // own "unknown". `factionID` is echoed. EvEJS does the same.
            //
            // FW-6: `facwarCommon.GetDonationTax` also reads `.factionPoints`
            // and `.maxWarZonePoints` (an AttributeError on the bare KeyVal),
            // so they are the systems held over the zone's systems, never 0;
            // `systemUpgradeLevel` is the held systems' levels from their
            // iHub pools (`facwar::zone_info`).
            "GetFacWarZoneInfo" => Ok(match req.args.first() {
                Some(PyValue::Int(n)) => crate::facwar::zone_info(reg, *n),
                _ => crate::pyutil::keyval(vec![
                    ("factionID", PyValue::None),
                    ("factionPoints", PyValue::Int(0)),
                    ("maxWarZonePoints", PyValue::Int(1)),
                    ("systemUpgradeLevel", PyValue::Dict(vec![])),
                ]),
            }),
            // **Every manual stargate jump died client-side on these**
            // (`docs/lanes/stargates.md` §2). `check_cancel_stargate_jump`
            // (`jump_checks.py:22-27`) runs eight checks with no `try`, and
            // `check_triglavian:97` / `check_edencom:114` do `systemID not in
            // <answer>` -- while `mapSvc.py:660` adds two of them together.
            // Our fallback `None` made both a `TypeError` before
            // `CmdStargateJump` was ever sent. Empty lists are the truth:
            // there are no Triglavian or EDENCOM systems here.
            "GetTriglavianMinorVictorySystems"
            | "GetEdencomFortressSystems"
            | "GetEdencomMinorVictorySystems" => Ok(PyValue::List(vec![])),
            "GetSecurityModifiedSystems" => Ok(security_modified_systems()),
            // Stations per solar system, for the map's station-count star
            // colouring -- **a live lock, and the poisoned-cache variant
            // that leaves no traceback at all.**
            //
            // Three call sites guard and fill the *same* attribute,
            // `starmap.stationCountCache`, and all three compile to the
            // identical eleven instructions (read from the bytecode, because
            // the `guard` column of `findings/client-calls.tsv` undercounts
            // compound conditions and cannot be trusted for this):
            //
            // ```text
            //  18 LOAD_ATTR     stationCountCache
            //  21 LOAD_CONST    None
            //  24 COMPARE_OP    8            # `is`
            //  27 POP_JUMP_IF_FALSE 57
            //  45 CALL_FUNCTION 0             # GetStationCount()
            //  51 STORE_ATTR    stationCountCache
            // ```
            //
            // The `STORE_ATTR` is fed straight from the call, with no
            // comprehension in between, so with the `map` service routable
            // and this method unanswered our `None` was **successfully
            // stored** -- the `contractProxy` mechanism, not the LP one:
            // the cache is set to the one value that fails its own `is None`
            // guard, so the very next read re-issues the call. Forever, at
            // map-redraw rate.
            //
            // **Nothing saw it.** No exception is raised at the store, so
            // `evecap --triage` listed nothing and `--alerts` was empty; the
            // repeat count in `evecap --calls` was the only evidence it
            // existed. `starMapSvc` resets the attribute to `None` on
            // `Stop()`, and nothing else reads it: these three are the whole
            // consumer set.
            //
            // **A list of `(solarSystemID, count)` pairs, and the container
            // family is the whole point here -- a dict would be wrong.** The
            // three consumers do not agree about what they were handed:
            //
            //  - `mapFilterStationCount.py(17) _ConstructDataBySolarSystemID`
            //    and `mapViewColorHandler.py(648) ColorStarsByStationCount`
            //    both do `dict(history)`.
            //  - `starModeHandler.py(452) ColorStarsByStationCount` does
            //    `for solarSystemID, amount in history:` --
            //    `FOR_ITER` then `UNPACK_SEQUENCE 2`, twice.
            //
            // Iterating a *dict* yields its keys, so a dict satisfies the
            // first two and raises `TypeError: 'int' object is not iterable`
            // in the third. A sequence of pairs satisfies all three, because
            // `dict()` accepts exactly that. This is the same trap as the
            // map's three earlier walls (`_ConstructAssetRequirements`,
            // `mapViewData.py(112)`, `mapView.py(606)`): the method name
            // sounds like a mapping and the client is not.
            //
            // **Not a native consumer, and that was checked rather than
            // assumed**, because the map is where `beyonce.GetFormations`
            // produced a black screen by answering `[]` where `()` was
            // required. Both paths end at `_PrepareStandardColorData`, which
            // fills a Python dict of `(sizeFactor, colorValue, hint, None)`
            // tuples; the only native call downstream gets floats that
            // filter computed. So `[]` over `()` is safe, and a list is
            // right anyway -- `dict()` and `UNPACK_SEQUENCE` are both
            // relaxed about which sequence they got.
            //
            // MAP-STATIONS: the count comes from the station table
            // (`stations/stations.tsv`) plus seeded stations, as pairs. With
            // no bundle and no seeded station it is `[]`, which is not
            // `None`, so the cache latches and the loop stops.
            "GetStationCount" => Ok(PyValue::List(
                reg.station_counts()
                    .into_iter()
                    .map(|(s, n)| PyValue::Tuple(vec![PyValue::Int(s), PyValue::Int(n)]))
                    .collect(),
            )),
            // MAP-PERSONAL (`visits.rs`): the pilot's own overlays.
            "GetSolarSystemVisits" => Ok(crate::visits::visits_answer(reg, req.actor)),
            "GetMyExtraMapInfo" => Ok(crate::visits::members_answer(reg, req.actor)),
            "GetMyExtraMapInfoAgents" => Ok(crate::visits::agents_answer(reg, req.actor)),
            // No system cost index here (`industry.rs`): `iteritems()` on `{}`.
            "GetIndustryCostModifier" => Ok(PyValue::Dict(vec![])),
            // No incursions and no roaming weather.
            "GetSystemsInIncursions" | "GetAllRoamingWeatherSystems" => Ok(PyValue::List(vec![])),
            // CAP-D: `{solarSystemID: count}` of the caller's fleet's lit
            // cynosural fields (`crate::cyno::beacon_count`).
            "GetBeaconCount" => Ok(crate::cyno::beacon_count(reg, req.actor)),
            "GetStationInfo" => Ok(reg.station_info(req.actor)),
            // SOC-D1: the map's sov-changes filter (`service/sov.rs`).
            "GetRecentSovActivity" => Ok(super::sov::recent_activity(reg)),
            _ => Err(reg.no_method(req)),
        },
        "stationSvc" => match req.method {
            // Sovereignty by solar system, for the map's star colouring.
            //
            // **An empty list, and the row type is unobservable.**
            // `mapViewData.py(109) GetAllianceSolarSystems` is
            //
            //     allianceSolarSystems = {}                               # 110
            //     allianceSystemCache = \
            //         sm.RemoteSvc('stationSvc').GetAllianceSystems()     # 111
            //     for x in allianceSystemCache:                           # 112
            //         allianceSolarSystems[x.solarSystemID] = x.allianceID   # 113
            //
            // so the only property the client can observe is *iterable*.
            // With no rows the body never runs and `[]` is the complete
            // answer rather than a placeholder for a rowset we have not
            // worked out. The two fields are **attribute** reads, so real
            // sovereignty would need `KeyVal`s or `blue.DBRow`s and a list
            // of tuples would not do -- but that is unexercised today, and
            // inventing a `DBRowDescriptor` for it would be inventing
            // evidence.
            //
            // Nothing calls `.Index()`, `.header`, `len()` or `.Keys()` on
            // it, so this is not a rowset obligation and
            // `blue.pyos.XUtil_Index` is never reached. Not a native
            // consumer either, checked rather than assumed: the dict this
            // builds goes into Python dict lookups, and the one native call
            // downstream, `trinity.Tr2RuntimeInstanceData.SetData`, gets
            // floats the filter computed and never sees our value.
            //
            // **This must land together with
            // `fwWarzoneSolarsystem.GetAllWarzonesOccupationStates`.**
            // Line 116 of this same function calls it and does
            // `.itervalues()` on the result, so answering only one of the
            // two moves the identical failure four lines down. If the map
            // still dies with `'NoneType' object has no attribute
            // 'itervalues'` at `mapViewData.py(117)`, that is which half is
            // missing.
            //
            // Not cached client-side, so it fires on every filter
            // construct. Harmless -- there is no `is None` cache here to
            // deadlock -- but expect repeats when the player flips filters.
            //
            // SOC-D1: one `KeyVal(solarSystemID, allianceID)` per claimed
            // system, from the sov ledger.
            "GetAllianceSystems" => Ok(super::sov::alliance_systems(reg)),
            // GAP-STATIONSVC. `notificationUtil.CreateLocationInfoLink`
            // reads `.stationTypeID` off this [V]. `KeyVal(stationID,
            // stationName, stationTypeID, solarSystemID, constellationID,
            // regionID, ownerID, operationID, description, x, y, z)`; an
            // unknown station is `None`, as the real one's missing row.
            // [A: the name of an unseeded NPC station is "Station <id>" --
            // the client names them itself from `cfg.evelocations`;
            // falsified by a notification link that shows the placeholder.]
            "GetStation" => {
                let Some(PyValue::Int(id)) = req.args.first() else {
                    return Ok(PyValue::None);
                };
                let Some(st) = reg.station_ref(*id) else {
                    return Ok(PyValue::None);
                };
                let seeded = reg
                    .config
                    .characters
                    .iter()
                    .any(|c| c.station_id == Some(*id));
                let name = if seeded {
                    reg.config.station_name.clone()
                } else {
                    format!("Station {id}")
                };
                let [x, y, z] = reg.station_position(*id);
                Ok(crate::pyutil::keyval(vec![
                    ("stationID", PyValue::Int(st.station_id)),
                    ("stationName", PyValue::Bytes(name.into_bytes())),
                    ("stationTypeID", PyValue::Int(st.type_id)),
                    ("solarSystemID", PyValue::Int(st.solar_system_id)),
                    ("constellationID", PyValue::Int(st.constellation_id)),
                    ("regionID", PyValue::Int(st.region_id)),
                    ("ownerID", PyValue::Int(st.owner_id)),
                    ("operationID", PyValue::Int(st.operation_id)),
                    ("description", PyValue::Bytes(Vec::new())),
                    ("x", PyValue::Real(x)),
                    ("y", PyValue::Real(y)),
                    ("z", PyValue::Real(z)),
                ]))
            }
            // `lpStoreSvc.GetNearestDockableWithLPStoreForCorp` iterates
            // it [V]: `KeyVal(stationID, solarSystemID)` per station the
            // corp owns (NPC table, plus seeded stations).
            "GetStationsForOwner" => {
                let Some(PyValue::Int(owner)) = req.args.first() else {
                    return Ok(PyValue::List(vec![]));
                };
                let mut rows: std::collections::BTreeMap<i64, i64> = reg
                    .content()
                    .get::<crate::stations::Table>()
                    .ids_of_owner(*owner)
                    .into_iter()
                    .collect();
                for s in reg.stations.values().filter(|s| s.owner_id == *owner) {
                    rows.insert(s.station_id, s.solar_system_id);
                }
                Ok(PyValue::List(
                    rows.into_iter()
                        .map(|(st, sys)| {
                            crate::pyutil::keyval(vec![
                                ("stationID", PyValue::Int(st)),
                                ("solarSystemID", PyValue::Int(sys)),
                            ])
                        })
                        .collect(),
                ))
            }
            // Only assigned by `all_ui_systems:32` [V]; answered to stop
            // the UNANSWERED line. [A: no claimed systems listed.]
            "GetSystemsForAlliance" => Ok(PyValue::List(vec![])),
            "GetStationItemBits" => {
                let c = req.actor.and_then(|id| reg.character(id));
                let bits = match c {
                    Some(c) => vec![
                        PyValue::Int(c.station_owner_id),
                        PyValue::Int(c.station_id.unwrap_or(0)),
                        PyValue::Int(c.station_operation_id),
                        PyValue::Int(c.station_type_id),
                    ],
                    None => vec![PyValue::None; 4],
                };
                Ok(PyValue::Tuple(bits))
            }
            _ => Err(reg.no_method(req)),
        },
        // The lobby window, which opens on every dock on its own thread.
        // Nothing here blocks the hangar drawing, but an unanswered call
        // still hangs the panel that wanted it.
        "station" => match req.method {
            // **The cheapest thing two players can see each other
            // through**, and the first one this server answers.
            //
            // Iterated and unpacked as
            // `(charID, corpID, allianceID, warFactionID)`, so a wrong arity
            // is a `ValueError` -- caught and logged, guests tab silently
            // empty. The arity is the whole requirement, inferred from the
            // client's own unpack; evejs confirms the shape from a
            // Tranquility recording, `[[2124510715, 1000169, None, None],
            // ...]` (`services/_shared/guestLists.js:42-52`), which is a
            // list of 4-element lists with nulls for the absent ids.
            //
            // **Deduplicated by character, and that is not tidiness.** The
            // client's guest panel appends every `OnCharNowInStation`
            // without de-duping, and its removal walks the live node list
            // while mutating it, so it cannot clear a duplicate row
            // afterwards (`guestLists.js:62-74`, read off the same client).
            // `Registry::characters_at_station` returns distinct characters;
            // a character logged in twice is one guest.
            //
            // The actor is included: the real lobby lists you among the
            // guests, and leaving yourself out would have made a
            // one-player station look broken rather than empty.
            "GetGuests" => {
                // Docked-now (`docs/lanes/docked.md`): `station_id` is "my
                // station" and stays set in space.
                let Some(station) = req.actor.and_then(|id| reg.docked_station(id)) else {
                    // No actor, or an actor who is not docked. An empty
                    // list, not a raise: the panel renders it.
                    return Ok(PyValue::List(vec![]));
                };
                let guests = reg
                    .characters_at_station(station)
                    .into_iter()
                    .map(|c| {
                        PyValue::List(vec![
                            PyValue::Int(c.character_id),
                            PyValue::Int(c.corporation_id),
                            c.alliance_id.map_or(PyValue::None, PyValue::Int),
                            // `warFactionID`: the militia, else None.
                            super::facwar_mgr::war_faction(reg, c.character_id)
                                .map_or(PyValue::None, PyValue::Int),
                        ])
                    })
                    .collect();
                Ok(PyValue::List(guests))
            }
            _ => Err(reg.no_method(req)),
        },
        // Four ints, in this order, wrapped by the client as
        // `Row(['ownerID', 'itemID', 'operationID', 'stationTypeID'], bits)`
        // -- and `Row.__init__` raises unless the length is exactly 4.
        //
        // Three of the four have teeth:
        //
        // - `itemID` **must equal `session.stationid`**. `GetStationItem`
        //   re-issues this call whenever `_stationItem.itemID` differs,
        //   and `GetStationType()` is consulted on every hangar
        //   operation -- so a wrong id is a synchronous call storm, not
        //   one wrong value.
        // - `stationTypeID` must be a real FSD type:
        //   `hangarRegistry.GetHangarType` calls
        //   `evetypes.GetCategoryID`/`GetRaceID` on it, and
        //   `evetypes.GetType` raises `TypeNotFoundException` for an
        //   unknown one. `0` -- which is what the client itself
        //   substitutes when there is no station item -- therefore
        //   raises inside `SetUpModels` and the hangar never draws.
        // - `operationID` must be a key in `stationOperations.fsdbinary`.
        //   `get_station_services_for_operation` returns `None` for an
        //   unknown one and the caller does `set(...)` on it, so an
        //   invented value is a `TypeError` in the lobby window on every
        //   dock.
        //
        // All four come from the client's own `npcStations` table; see
        // `Character`.
        // Universal search. Two methods, two different empties, and
        // neither may be `None`.
        //
        // Arrives as `sm.ProxySvc('search')`, which is the same wire form
        // as `RemoteSvc` -- a plain, unbound call on a node address.
        //
        // "Nothing matched" is the *honest* answer for a name nobody here
        // has, and both callers render it properly; a name somebody has is
        // found by `service/social.rs`'s people search.
        "search" => match req.method {
            // A **flat list of ids**, not keyed by category.
            // `searchUtil.GetResultsList` returns our value unchanged, and
            // its callers are unforgiving in two of three cases:
            //
            //  - `searchOld.Search` does `if not result:` and opens a
            //    "No Owner found with ..." dialog. `None` survives this,
            //    which is why the session that found this gap showed the
            //    player a correct dialog and left no traceback.
            //  - `eveui/autocomplete/player/provider.py` does
            //    `enumerate(results)` -- `None` is a `TypeError`.
            //  - `mapSvc.FindByName` does `map(reg.GetItem, results)` --
            //    the same.
            //
            // So `[]` changes nothing the player saw and removes two
            // latent crashes.
            //
            // The real arguments, off the wire and the client's own log:
            // two positional, `(u'capsuleer', [2, 3])`, plus **three
            // keywords that neither `--calls` nor our `/2` shows**,
            // `{'hideNPC': 1, 'exact': 0, 'onlyAltName': 0}`. The list is
            // `ResultType` -- `character = 2`, `corporation = 3` -- and
            // that exact triple is produced by exactly one path in the
            // tree, `searchOld.Search(categoryID=categoryOwner,
            // hideNPC=1)`, reached from the Contracts window's Search tab.
            // **[I]** on the window; the code path is verified and unique.
            //
            // **Characters are answered now** (the contracts lane, P5.8):
            // the create-contract wizard names a private contract's
            // assignee through exactly this call
            // (`createContractWnd.py(1607)` -> `searchOld.Search(name,
            // groupCharacter, categoryOwner)` -> `GetResultsList`), and a
            // single result goes back to the wizard as the id itself
            // (`if len(result) == 1: return result[0]`, searchOld.py,
            // **[V]**).
            //
            // **And corporations, and the client's own matching rules**
            // (W9-L4): who can be found and how a name matches -- the
            // `exact` keyword's `MatchBy`, `hideNPC`, `onlyAltName` -- live
            // in `service/social.rs` beside `lookupSvc`, which asks the same
            // question for the older windows. `docs/lanes/lookup.md`.
            "QuickQuery" => Ok(super::social::search_quick_query(reg, req)),
            // A **dict keyed by `ResultType`**, and here `None` really
            // does throw. `searchUtil.GetResultsByGroupID`:
            //
            //     resultsByType = searchMgr.Query(searchStr, groupIDList, exact=searchBy)
            //     if resultsByType:
            //         _PrimeResults(resultsByType)
            //     _SortResults(resultsByType)        # <- NOT guarded
            //     return resultsByType or {}
            //
            // `_SortResults` does `.iteritems()` outside the `if`, so
            // `None` is an `AttributeError` -- and the `or {}` one line
            // later is exactly the trap in the §4 table: the code looks
            // like it tolerates a falsy answer right up until the line
            // above runs first.
            //
            // This is the **map's own search box** and People & Places.
            // The `except UserError:` around it does not catch an
            // `AttributeError`.
            //
            // `{}` is self-consistent all the way down: the prime is
            // skipped, the sort iterates nothing, and the scroll shows
            // `NoResultsReturned` -- and it is still the answer for a name
            // nobody has. Characters and corporations are found by
            // `service/social.rs` (W9-L4), with the same rules as
            // `QuickQuery`.
            "Query" => Ok(super::social::search_query(reg, req)),
            _ => Err(reg.no_method(req)),
        },
        // Bookmarks -- the Locations window, "People & Places". Its own
        // section below, because it is the one arm here that keeps state.
        "accessGroupBookmarkMgr" => bookmark_mgr(reg, req),
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

// ---- Bookmarks ------------------------------------------------------------
//
// The Locations window ("People & Places", Places tab) and the "Save
// Location" dialog. `docs/lanes/bookmarks.md` has the census this is built
// from: every call `bookmarkSvc` makes, its shape and its consumer.
//
// **The client's model is folders, and a bookmark cannot exist outside
// one.** `ACLPrimeBookmarks` (`bookmarkSvc.py:164`) unpacks
// `folders, bookmarks, subfolders` and, if `folders` is empty, creates a
// default folder with `AddFolder` before anything else can happen (`:176`)
// **[V]**. Every create then names a `folderID` from the dialog's combo. So
// the folder is the unit of storage here too, and the old `AddFolder`
// refusal -- honest while nothing was stored -- is what this replaces.
//
// **Personal folders only.** A shared folder is an access-group feature
// (`adminGroupID` .. `viewGroupID` name `accessGroupsMgr` groups), and this
// sandbox has no groups. A request for one is refused by name rather than
// quietly made personal, because the player would be told a folder is shared
// with people who cannot see it.
//
// Three kinds of row, all `utillib.KeyVal`: the client writes back onto
// folders (`foldersNew[id].update({...})`, `bookmarkSvc.py:418`) and onto
// bookmarks (`bookmark.memo = name`, `:631`), and `KeyVal` has both `update`
// and attribute assignment, where a packed `DBRow` refuses `STORE_ATTR`.

/// `CmdWarpToStuff`'s subject for a bookmark: `bp.CmdWarpToStuff('bookmark',
/// bookmark.bookmarkID, minRange=.., fleet=..)` at `movementFunctions.py:462`
/// and `addressBookEntries.py:163` **[V]**. The id is a *bookmark* id, which
/// is why [`bookmark_target`] exists rather than `Registry::ball_target`.
pub(crate) const WARP_SUBJECT_BOOKMARK: &[u8] = b"bookmark";

/// `appConst.ACCESS_NONE` / `ACCESS_PERSONAL` **[V]** (`appConst.py:2523-2524`).
/// A personal folder's `accessLevel` is `ACCESS_PERSONAL`: it is one of the
/// three levels `MoveOrCopyBookmarks` treats as writable (`bookmarkSvc.py:665`).
const ACCESS_NONE: i64 = 0;
const ACCESS_PERSONAL: i64 = 1;

/// `invConst.typeRegion` / `typeConstellation` / `typeSolarSystem` **[V]**
/// (`inventorycommon/const.py:1771`, `:1531`, `:1835`).
const TYPE_REGION: i64 = 3;
const TYPE_CONSTELLATION: i64 = 4;
const TYPE_SOLAR_SYSTEM: i64 = 5;

/// The client's own id ranges, which `ACLBookmarkLocation` uses to route a
/// static location to `BookmarkStaticLocation` (`bookmarkSvc.py:566-567`)
/// **[V]**: `min/maxRegion`, `Constellation`, `SolarSystem`, `Station` at
/// `inventorycommon/const.py:14-66`.
const REGIONS: std::ops::RangeInclusive<i64> = 10_000_000..=19_999_999;
const CONSTELLATIONS: std::ops::RangeInclusive<i64> = 20_000_000..=29_999_999;
const SOLAR_SYSTEMS: std::ops::RangeInclusive<i64> = 30_000_000..=39_999_999;

/// `appConst` limits **[V]** (`appConst.py:2514-2519`). Enforced here because
/// the client checks only some of them itself, and the error keys exist for
/// the server to raise.
const MAX_BOOKMARKS_IN_PERSONAL_FOLDER: usize = 3000;
const MAX_ACTIVE_PERSONAL_BOOKMARK_FOLDERS: usize = 5;
const MAX_CREATED_BOOKMARK_FOLDERS: usize = 30;
const MAX_SUBFOLDERS: usize = 30;

/// One hour in FILETIME ticks.
const HOUR: i64 = 36_000_000_000;

/// The first id handed out. `[A]`: bookmark, folder and subfolder ids are
/// opaque dictionary keys to the client (`bookmarkCache`, `foldersNew`,
/// `subfolders`), so any positive integer will do; the only property that
/// matters is that ids are never reused, which the persisted allocator keeps.
const FIRST_ID: i64 = 1;

/// Every bookmark folder, subfolder and bookmark in the world.
///
/// **Persisted** (`bmfolder`, `bmsub`, `bookmark`, `nextbm` records in the
/// world file): a bookmark is something the player made, and "it was gone
/// after a restart" is exactly the failure `docs/reference/world-state.md` §2 puts
/// player-created state in the snapshot to prevent.
///
/// One allocator for all three kinds, so an id names one thing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bookmarks {
    folders: Vec<Folder>,
    subfolders: Vec<Subfolder>,
    marks: Vec<Mark>,
    next_id: i64,
}

#[derive(Debug, Clone, PartialEq)]
struct Folder {
    folder_id: i64,
    owner_id: i64,
    name: String,
    description: String,
    is_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Subfolder {
    subfolder_id: i64,
    folder_id: i64,
    creator_id: i64,
    name: String,
}

/// One bookmark, with exactly the attributes `NewBookmarkCreated` builds for
/// its own cache (`bookmarkSvc.py:603-618`) **[V]** -- the client's local
/// copy and ours must agree, or a reopen shows the player something other
/// than what they saved.
#[derive(Debug, Clone, PartialEq)]
struct Mark {
    bookmark_id: i64,
    folder_id: i64,
    subfolder_id: Option<i64>,
    creator_id: i64,
    /// The bookmarked object, or `None` for a point in space.
    item_id: Option<i64>,
    type_id: i64,
    /// `None` for a system, constellation or region: a bookmark of the
    /// place rather than of a point in it.
    position: Option<[f64; 3]>,
    /// The solar system, for everything that has one. `bookmarkSvc` keys
    /// `bookmarkIDsBySolarystemID` by it (`:168`) **[V]**.
    location_id: i64,
    /// The bookmark's label. The client calls it `memo` and the dialog's
    /// argument `name` (`:609`).
    memo: String,
    note: String,
    created: i64,
    expiry: Option<i64>,
}

impl Bookmarks {
    /// The record tags this type reads and writes in the world file.
    pub(crate) const RECORDS: &'static [&'static str] =
        &["bmfolder", "bmsub", "bookmark", "nextbm"];

    fn alloc(&mut self) -> i64 {
        let id = self.next_id.max(FIRST_ID);
        self.next_id = id + 1;
        id
    }

    fn folder(&self, owner: i64, folder_id: i64) -> Option<&Folder> {
        self.folders
            .iter()
            .find(|f| f.folder_id == folder_id && f.owner_id == owner)
    }

    fn folder_mut(&mut self, owner: i64, folder_id: i64) -> Option<&mut Folder> {
        self.folders
            .iter_mut()
            .find(|f| f.folder_id == folder_id && f.owner_id == owner)
    }

    fn in_folder(&self, folder_id: i64) -> impl Iterator<Item = &Mark> {
        self.marks.iter().filter(move |m| m.folder_id == folder_id)
    }

    /// Drop every bookmark whose expiry has passed. The client does the same
    /// to its own cache in `RemoveExpiredBookmarks` (`bookmarkSvc.py:187`,
    /// `expiry and expiry < now`) **[V]**; doing it here as well is what
    /// keeps an expired bookmark from coming back on the next login.
    fn prune(&mut self, now: i64) -> bool {
        let before = self.marks.len();
        self.marks.retain(|m| m.expiry.is_none_or(|e| e >= now));
        self.marks.len() != before
    }

    /// Append this type's records to a world file being written.
    ///
    /// `-1` is the `None` of an optional id and `-` the `None` of a
    /// position, for the reason `store.rs` gives for its own `-1`: an empty
    /// field cannot be told from a truncated line.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        let esc = crate::store::esc;
        for f in &self.folders {
            let _ = writeln!(
                out,
                "bmfolder\t{}\t{}\t{}\t{}\t{}",
                f.folder_id,
                f.owner_id,
                u8::from(f.is_active),
                esc(&f.name),
                esc(&f.description)
            );
        }
        for s in &self.subfolders {
            let _ = writeln!(
                out,
                "bmsub\t{}\t{}\t{}\t{}",
                s.subfolder_id,
                s.folder_id,
                s.creator_id,
                esc(&s.name)
            );
        }
        for m in &self.marks {
            let (x, y, z) = match m.position {
                Some([x, y, z]) => (x.to_string(), y.to_string(), z.to_string()),
                None => ("-".to_owned(), "-".to_owned(), "-".to_owned()),
            };
            let _ = writeln!(
                out,
                "bookmark\t{}\t{}\t{}\t{}\t{}\t{}\t{x}\t{y}\t{z}\t{}\t{}\t{}\t{}\t{}",
                m.bookmark_id,
                m.folder_id,
                m.subfolder_id.unwrap_or(-1),
                m.creator_id,
                m.item_id.unwrap_or(-1),
                m.type_id,
                m.location_id,
                m.created,
                m.expiry.unwrap_or(-1),
                esc(&m.memo),
                esc(&m.note)
            );
        }
        // Nothing at all for a world nobody has bookmarked in, so that a
        // file written before bookmarks existed reads back identical.
        if self.next_id > 0 {
            let _ = writeln!(out, "nextbm\t{}", self.next_id);
        }
    }

    /// Read one record whose tag is in [`Bookmarks::RECORDS`].
    ///
    /// **Strict**, unlike the parser's handling of unknown tags: a bookmark
    /// line we cannot read is a bookmark the player would lose, and refusing
    /// the world names the line instead.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let num = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        let opt = |i: usize| -> Result<Option<i64>, String> {
            Ok(match num(i)? {
                -1 => None,
                v => Some(v),
            })
        };
        let real = |i: usize| -> Result<Option<f64>, String> {
            match f.get(i).copied() {
                Some("-") => Ok(None),
                Some(v) => v
                    .parse::<f64>()
                    .map(Some)
                    .map_err(|_| format!("field {i} is not a number")),
                None => Err(format!("field {i} is missing")),
            }
        };
        let text = |i: usize| crate::store::unesc(f.get(i).copied().unwrap_or(""));
        match f.first().copied() {
            Some("bmfolder") => self.folders.push(Folder {
                folder_id: num(1)?,
                owner_id: num(2)?,
                is_active: num(3)? != 0,
                name: text(4),
                description: text(5),
            }),
            Some("bmsub") => self.subfolders.push(Subfolder {
                subfolder_id: num(1)?,
                folder_id: num(2)?,
                creator_id: num(3)?,
                name: text(4),
            }),
            Some("bookmark") => {
                let position = match (real(7)?, real(8)?, real(9)?) {
                    (Some(x), Some(y), Some(z)) => Some([x, y, z]),
                    (None, None, None) => None,
                    _ => return Err("a position must have all three axes or none".to_owned()),
                };
                self.marks.push(Mark {
                    bookmark_id: num(1)?,
                    folder_id: num(2)?,
                    subfolder_id: opt(3)?,
                    creator_id: num(4)?,
                    item_id: opt(5)?,
                    type_id: num(6)?,
                    position,
                    location_id: num(10)?,
                    created: num(11)?,
                    expiry: opt(12)?,
                    memo: text(13),
                    note: text(14),
                })
            }
            Some("nextbm") => self.next_id = num(1)?,
            other => return Err(format!("{other:?} is not a bookmark record")),
        }
        // **The allocator never trails its own rows**, whatever the file
        // says -- the same rule `Jobs::restore` keeps, for the same reason: a
        // reused id lands on a key the client already has cached.
        let highest = self
            .folders
            .iter()
            .map(|f| f.folder_id)
            .chain(self.subfolders.iter().map(|s| s.subfolder_id))
            .chain(self.marks.iter().map(|m| m.bookmark_id))
            .max();
        if let Some(highest) = highest {
            self.next_id = self.next_id.max(highest + 1);
        }
        Ok(())
    }
}

impl Folder {
    /// The attributes read across `bookmarkSvc`, `bookmarksPanel`,
    /// `bookmarkLocationWindow`, `bookmarkContainerWindow` and
    /// `linkedBookmarkFolderWindow` **[V]**: `folderID`, `folderName`,
    /// `description`, `isPersonal`, `isActive`, `accessLevel` and the four
    /// group ids. `creatorID` and `ownerID` are `[A]`: nothing read was
    /// seen to need them, and a missing attribute is an `AttributeError`
    /// where an extra one is nothing.
    fn row(&self) -> PyValue {
        keyval(vec![
            ("folderID", PyValue::Int(self.folder_id)),
            ("folderName", PyValue::Unicode(self.name.clone())),
            ("description", PyValue::Unicode(self.description.clone())),
            ("isPersonal", PyValue::Bool(true)),
            ("isActive", PyValue::Bool(self.is_active)),
            ("accessLevel", PyValue::Int(ACCESS_PERSONAL)),
            ("creatorID", PyValue::Int(self.owner_id)),
            ("ownerID", PyValue::Int(self.owner_id)),
            ("adminGroupID", PyValue::None),
            ("manageGroupID", PyValue::None),
            ("useGroupID", PyValue::None),
            ("viewGroupID", PyValue::None),
        ])
    }
}

impl Subfolder {
    /// `subfolderID`, `folderID`, `subfolderName` and `creatorID`, all read
    /// by `bookmarkLocationWindow.GetFolderAndSubfolderOptions:317-337`
    /// **[V]**.
    fn row(&self) -> PyValue {
        keyval(vec![
            ("subfolderID", PyValue::Int(self.subfolder_id)),
            ("folderID", PyValue::Int(self.folder_id)),
            ("subfolderName", PyValue::Unicode(self.name.clone())),
            ("creatorID", PyValue::Int(self.creator_id)),
        ])
    }
}

impl Mark {
    fn row(&self) -> PyValue {
        let opt = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
        let axis = |i: usize| self.position.map_or(PyValue::None, |p| PyValue::Real(p[i]));
        keyval(vec![
            ("bookmarkID", PyValue::Int(self.bookmark_id)),
            ("folderID", PyValue::Int(self.folder_id)),
            ("itemID", opt(self.item_id)),
            ("typeID", PyValue::Int(self.type_id)),
            ("flag", PyValue::None),
            ("memo", PyValue::Unicode(self.memo.clone())),
            ("created", PyValue::Int(self.created)),
            ("expiry", opt(self.expiry)),
            ("x", axis(0)),
            ("y", axis(1)),
            ("z", axis(2)),
            ("locationID", PyValue::Int(self.location_id)),
            ("note", PyValue::Unicode(self.note.clone())),
            ("subfolderID", opt(self.subfolder_id)),
            ("creatorID", PyValue::Int(self.creator_id)),
        ])
    }

    /// The 8-tuple both create calls return, unpacked as `bookmarkID,
    /// itemID, typeID, x, y, z, locationID, expiryDate`
    /// (`bookmarkSvc.py:569`, `:575`) **[V]**.
    fn created_tuple(&self) -> PyValue {
        let axis = |i: usize| self.position.map_or(PyValue::None, |p| PyValue::Real(p[i]));
        PyValue::Tuple(vec![
            PyValue::Int(self.bookmark_id),
            self.item_id.map_or(PyValue::None, PyValue::Int),
            PyValue::Int(self.type_id),
            axis(0),
            axis(1),
            axis(2),
            PyValue::Int(self.location_id),
            self.expiry.map_or(PyValue::None, PyValue::Int),
        ])
    }
}

/// What a new bookmark points at, before it has a folder and a name.
struct Place {
    item_id: Option<i64>,
    type_id: i64,
    position: Option<[f64; 3]>,
    location_id: i64,
}

fn bad_args(req: &Request<'_>, why: &str) -> ServiceError {
    ServiceError::BadArguments {
        service: req.service.to_owned(),
        method: req.method.to_owned(),
        why: why.to_owned(),
    }
}

fn refuse(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

/// `CustomInfo` with our own text: the generic server-supplied modal, for a
/// refusal no specific key describes.
fn refuse_with(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomInfo",
        crate::pyutil::pypairs(vec![("info", crate::pyutil::bstr(text))]),
    ))
}

fn id_arg(req: &Request<'_>, i: usize, what: &str) -> Result<i64, ServiceError> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Ok(*n),
        _ => Err(bad_args(
            req,
            &format!("argument {i} ({what}) is not an id"),
        )),
    }
}

fn text_arg(v: Option<&PyValue>) -> String {
    match v {
        Some(PyValue::Unicode(s)) => s.clone(),
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    }
}

fn bool_arg(v: Option<&PyValue>) -> bool {
    match v {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

/// `subfolderID`, which both create calls pass **by keyword**
/// (`CALL_FUNCTION 261`: five positionals and one keyword,
/// `bookmarkSvc.py:569`, `:575`) **[V]**, and the rest positionally.
fn subfolder_arg(req: &Request<'_>, positional: usize) -> Option<i64> {
    crate::pyutil::opt_id(
        crate::pyutil::pairs_get(req.kwargs, b"subfolderID").or(req.args.get(positional)),
    )
}

/// `bookmarkConst.BOOKMARK_EXPIRY_*` **[V]** (`bookmarkConst.py:16-19`):
/// `0` none, `1` four hours, `2` two days, `3` twenty-four hours -- note the
/// order, which is not ascending. The dialog sends the code, not a time
/// (`bookmarkLocationWindow.py:415`), and the server answers the date.
fn expiry_date(code: Option<&PyValue>, now: i64) -> Option<i64> {
    let hours = match code {
        Some(PyValue::Int(1)) => 4,
        Some(PyValue::Int(2)) => 48,
        Some(PyValue::Int(3)) => 24,
        _ => return None,
    };
    Some(now + hours * HOUR)
}

/// A subfolder must belong to the folder it is named with; anything else is
/// dropped to "no subfolder" rather than refused, because the bookmark
/// itself is still worth keeping.
fn checked_subfolder(b: &Bookmarks, folder_id: i64, subfolder_id: Option<i64>) -> Option<i64> {
    subfolder_id.filter(|id| {
        b.subfolders
            .iter()
            .any(|s| s.subfolder_id == *id && s.folder_id == folder_id)
    })
}

/// Store a new bookmark and answer the 8-tuple. Shared by the static and
/// the in-space create, which differ only in how they find the [`Place`].
fn create_mark(
    reg: &mut Registry,
    req: &Request<'_>,
    actor: i64,
    place: Place,
) -> Result<PyValue, ServiceError> {
    let folder_id = id_arg(req, 1, "folderID")?;
    if reg.bookmarks.folder(actor, folder_id).is_none() {
        return Err(refuse("BookmarkFolderNoLongerThere"));
    }
    if reg.bookmarks.in_folder(folder_id).count() >= MAX_BOOKMARKS_IN_PERSONAL_FOLDER {
        return Err(refuse("CannotCreateMoreBookmarksInFolder"));
    }
    let now = crate::pyutil::filetime_now();
    let subfolder_id = checked_subfolder(&reg.bookmarks, folder_id, subfolder_arg(req, 5));
    let mark = Mark {
        bookmark_id: reg.bookmarks.alloc(),
        folder_id,
        subfolder_id,
        creator_id: actor,
        item_id: place.item_id,
        type_id: place.type_id,
        position: place.position,
        location_id: place.location_id,
        memo: text_arg(req.args.get(2)),
        note: text_arg(req.args.get(3)),
        created: now,
        expiry: expiry_date(req.args.get(4), now),
    };
    let answer = mark.created_tuple();
    reg.bookmarks.marks.push(mark);
    reg.dirty = true;
    Ok(answer)
}

/// Where a **static** location is: a station, a system, a constellation or
/// a region. `None` for anything we cannot place.
fn static_place(reg: &Registry, actor: i64, item_id: i64) -> Option<Place> {
    // The id ranges first: these need no catalogue at all. A bookmark of a
    // system, constellation or region has no position and is its own
    // location -- `IsCoordinateBookmark` (`bookmarkUtil.py:144`) then sees
    // `itemID == locationID` with `x` of `None` and correctly says "not a
    // point" **[V]**. `[A]`: that `locationID` is the item itself for a
    // constellation or region, which have no solar system to name.
    for (range, type_id) in [
        (&SOLAR_SYSTEMS, TYPE_SOLAR_SYSTEM),
        (&CONSTELLATIONS, TYPE_CONSTELLATION),
        (&REGIONS, TYPE_REGION),
    ] {
        if range.contains(&item_id) {
            return Some(Place {
                item_id: Some(item_id),
                type_id,
                position: None,
                location_id: item_id,
            });
        }
    }
    // A station somebody here is based at: the one table that knows a
    // station's type and system together.
    if let Some(c) = reg
        .config()
        .characters
        .iter()
        .find(|c| c.station_id == Some(item_id))
    {
        return Some(Place {
            item_id: Some(item_id),
            type_id: c.station_type_id,
            position: reg.ball_target(item_id).map(|t| t.position),
            location_id: c.solar_system_id?,
        });
    }
    // Any other station in the celestial catalogue: its own system when the
    // station table knows it, else (a one-system catalogue) the system the
    // actor's session names -- not the home station's, which is stale in space.
    let c = reg.config().celestials.get(item_id)?;
    if c.kind != crate::celestial::Kind::Station {
        return None;
    }
    Some(Place {
        item_id: Some(item_id),
        type_id: c.type_id,
        position: Some(c.position),
        location_id: reg
            .station_ref(item_id)
            .map(|s| s.solar_system_id)
            .or_else(|| reg.session_system(actor))?,
    })
}

/// `beyonce.BookmarkLocation(itemID, folderID, name, comment, expiry,
/// subfolderID=)` -- the in-space create (`bookmarkSvc.py:575`) **[V]**.
///
/// **Called from `service/space.rs`'s `beyonce` arm**, which is where the
/// client sends it: `ACLBookmarkLocation` routes everything that is not a
/// region, constellation, system, station or structure to the *ballpark*
/// (`:566-575`), and the commonest case is "Save Location" in space, which
/// passes the player's own ship (`addressbookService.py:475`) **[V]**.
///
/// Three outcomes:
///
/// - the player's own ship: a **coordinate bookmark** at its position, with
///   `itemID` `None` and `typeID` `typeSolarSystem` -- the shape
///   `IsCoordinateBookmark` recognises (`bookmarkUtil.py:144`) **[V]**;
/// - a celestial the park serves: a bookmark *of that object*, with its type
///   and position, so Show Info and warp-to resolve the object itself;
/// - anything else we can place (another ship, a rock): a coordinate
///   bookmark at where it is now. `[A]`: a ship moves and a bookmark does
///   not, so the honest bookmark of a ship is the point it was at.
pub(crate) fn bookmark_location(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character on this session"));
    };
    let item_id = id_arg(req, 0, "itemID")?;
    reg.advance_character(actor);
    // The client checks for a remote park before it calls (`:572-573`); this
    // is the server's half of the same rule, with the same key.
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(refuse("BkmMustBeInFlight"));
    };
    let place = if item_id == here.ship_id {
        Place {
            item_id: None,
            type_id: TYPE_SOLAR_SYSTEM,
            position: Some(here.motion.position),
            location_id: here.solar_system_id,
        }
    } else if let Some(c) = reg
        .config()
        .celestials
        .get(item_id)
        .filter(|c| c.kind.is_served())
    {
        Place {
            item_id: Some(item_id),
            type_id: c.type_id,
            position: Some(c.position),
            location_id: here.solar_system_id,
        }
    } else if let Some(t) = reg.ball_target(item_id) {
        Place {
            item_id: None,
            type_id: TYPE_SOLAR_SYSTEM,
            position: Some(t.position),
            location_id: here.solar_system_id,
        }
    } else {
        return Err(refuse_with(
            "This server cannot place that object, so it cannot be bookmarked.",
        ));
    };
    create_mark(reg, req, actor, place)
}

/// `beyonce.BookmarkScanResult(locationID, name, comment, resultID, folderID,
/// expiry, subfolderID=)` -- "Save Location" on a probe-scanner result
/// (`ACLBookmarkScanResult`, GAP-QUICKFIX B6). The answer is
/// `BookmarkLocation`'s 8-tuple, and a falsy bookmark id would be a silent
/// no-op in the client, so every failure is a refusal.
///
/// The point is the result's target where it is now (`scan::warp_target`),
/// stored as a coordinate bookmark. `[A]`: the client's `locationID` is the
/// system the scan was run in; falsifier: a result saved from another
/// system's window.
pub(crate) fn bookmark_scan_result(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(bad_args(req, "no character on this session"));
    };
    let location_id = id_arg(req, 0, "locationID")?;
    let folder_id = id_arg(req, 4, "folderID")?;
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(refuse("BkmMustBeInFlight"));
    };
    if here.solar_system_id != location_id {
        return Err(refuse("BookmarkNotAvailable"));
    }
    let Some(target) = req
        .args
        .get(3)
        .and_then(|r| crate::scan::warp_target(reg, actor, r))
    else {
        return Err(refuse("BookmarkNotAvailable"));
    };
    if reg.bookmarks.folder(actor, folder_id).is_none() {
        return Err(refuse("BookmarkFolderNoLongerThere"));
    }
    if reg.bookmarks.in_folder(folder_id).count() >= MAX_BOOKMARKS_IN_PERSONAL_FOLDER {
        return Err(refuse("CannotCreateMoreBookmarksInFolder"));
    }
    let now = crate::pyutil::filetime_now();
    let subfolder_id = checked_subfolder(&reg.bookmarks, folder_id, subfolder_arg(req, 6));
    let mark = Mark {
        bookmark_id: reg.bookmarks.alloc(),
        folder_id,
        subfolder_id,
        creator_id: actor,
        item_id: None,
        type_id: TYPE_SOLAR_SYSTEM,
        position: Some(target.position),
        location_id,
        memo: text_arg(req.args.get(1)),
        note: text_arg(req.args.get(2)),
        created: now,
        expiry: expiry_date(req.args.get(5), now),
    };
    let answer = mark.created_tuple();
    reg.bookmarks.marks.push(mark);
    reg.dirty = true;
    Ok(answer)
}

/// Where a warp to a bookmark goes, for `CmdWarpToStuff('bookmark', id)`.
///
/// `None` -- and so no warp -- for a bookmark that is not the actor's, that
/// is in another system, or that has no position (a system or region
/// bookmark, which the client offers "Set Destination" for, not "Warp To").
///
/// A bookmark *of an object* the park can resolve warps to the object, so a
/// station bookmark gets the station's standoff and a planet's its warp-in
/// point, exactly as a right-click on the object would. A coordinate
/// bookmark aims at its point with no radius. `[A]`: the warp site's
/// standoff floor (`WARP_STANDOFF`, 1 km) still applies to it, so a warp to
/// a coordinate bookmark lands 1 km short of the point where the real server
/// lands on it -- `docs/lanes/bookmarks.md` has the one-line change in
/// `space.rs` that would remove it.
pub(crate) fn bookmark_target(reg: &Registry, actor: i64, bookmark_id: i64) -> Option<Target> {
    let mark = reg
        .bookmarks
        .marks
        .iter()
        .find(|m| m.bookmark_id == bookmark_id)?;
    reg.bookmarks.folder(actor, mark.folder_id)?;
    if reg.roster().get(actor)?.solar_system_id != mark.location_id {
        return None;
    }
    if let Some(target) = mark.item_id.and_then(|id| reg.ball_target(id)) {
        return Some(target);
    }
    // A point is its own warp-in point: the ship lands on it, with no
    // standoff floor (GAP-QUICKFIX B6).
    let position = mark.position?;
    Some(Target {
        position,
        radius: 0.0,
        warp_in: Some(position),
    })
}

/// Every `accessGroupBookmarkMgr` method [`bookmark_mgr`] answers.
const BOOKMARK_METHODS: &[&str] = &[
    "GetMyActiveBookmarks",
    "AddFolder",
    "UpdateFolder",
    "DeleteFolder",
    "RemoveFromKnownFolders",
    "UpdateKnownFolderState",
    "AddToKnownFolders",
    "GetFolderInfo",
    "BookmarkStaticLocation",
    "UpdateBookmark",
    "DeleteBookmarks",
    "CreateSubfolder",
    "UpdateSubfolder",
    "DeleteSubfolder",
    "MoveBookmarksToFolderAndSubfolder",
    "CopyBookmarksToFolderAndSubfolder",
    "SearchFoldersWithAdminAccess",
];

/// The `accessGroupBookmarkMgr` arm.
///
/// Every reply shape below is the one its call site unpacks, cited where it
/// is decided; `docs/lanes/bookmarks.md` has the table.
fn bookmark_mgr(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    // `SearchFoldersWithAdminAccess` is the one call that needs no actor:
    // admin access is a shared-folder idea and there are none.
    // `bookmarksPanel.GetAdminFolders:709` iterates the answer **[V]**.
    if req.method == "SearchFoldersWithAdminAccess" {
        return Ok(PyValue::List(vec![]));
    }
    let Some(actor) = req.actor else {
        // An unknown method is still `NoSuchMethod` without a character, so
        // that a name we do not answer is reported as one.
        return Err(if BOOKMARK_METHODS.contains(&req.method) {
            bad_args(req, "no character on this session")
        } else {
            reg.no_method(req)
        });
    };
    match req.method {
        // `folders, bookmarks, subfolders = ...` (`bookmarkSvc.py:164`)
        // **[V]**: every folder the player has, active or not -- the window
        // lists inactive ones under their own heading -- and the bookmarks
        // and subfolders of the active ones, which is what `AddToKnownFolders`
        // and `UpdateKnownFolderState` also hand back only when active
        // (`:337`, `:457`).
        "GetMyActiveBookmarks" => {
            if reg.bookmarks.prune(crate::pyutil::filetime_now()) {
                reg.dirty = true;
            }
            let b = &reg.bookmarks;
            let mine: Vec<&Folder> = b.folders.iter().filter(|f| f.owner_id == actor).collect();
            let active: Vec<i64> = mine
                .iter()
                .filter(|f| f.is_active)
                .map(|f| f.folder_id)
                .collect();
            Ok(PyValue::Tuple(vec![
                PyValue::List(mine.iter().map(|f| f.row()).collect()),
                PyValue::List(
                    b.marks
                        .iter()
                        .filter(|m| active.contains(&m.folder_id))
                        .map(Mark::row)
                        .collect(),
                ),
                PyValue::List(
                    b.subfolders
                        .iter()
                        .filter(|s| active.contains(&s.folder_id))
                        .map(Subfolder::row)
                        .collect(),
                ),
            ]))
        }
        // `AddFolder(isPersonal, folderName, description, adminGroupID,
        // manageGroupID, useGroupID, viewGroupID)`, answered with the folder
        // row, which `CreateBookmarkFolder` stores under `folder.folderID`
        // (`bookmarkSvc.py:399-401`) **[V]**.
        //
        // This replaces a `CustomInfo` refusal that was honest while no
        // folder could be stored; now one can.
        "AddFolder" => {
            if req.args.len() < 3 {
                return Err(bad_args(
                    req,
                    "expected isPersonal, folderName, description",
                ));
            }
            if !bool_arg(req.args.first()) {
                return Err(refuse_with(
                    "Shared bookmark folders need access groups, which this server does not have yet. \
                     Create a personal folder instead.",
                ));
            }
            let mine = reg.bookmarks.folders.iter().filter(|f| f.owner_id == actor);
            if mine.clone().count() >= MAX_CREATED_BOOKMARK_FOLDERS {
                return Err(refuse("TooManyCreatedFolders"));
            }
            // `[A]`: a new folder is active unless that would exceed the
            // active-personal limit, in which case it is made inactive
            // rather than refused -- the client has a confirmation for
            // exactly that outcome (`ConfirmCreateAsInactivePersonalFolder`).
            let is_active =
                mine.filter(|f| f.is_active).count() < MAX_ACTIVE_PERSONAL_BOOKMARK_FOLDERS;
            let folder = Folder {
                folder_id: reg.bookmarks.alloc(),
                owner_id: actor,
                name: text_arg(req.args.get(1)),
                description: text_arg(req.args.get(2)),
                is_active,
            };
            let row = folder.row();
            reg.bookmarks.folders.push(folder);
            reg.dirty = true;
            Ok(row)
        }
        // `accessLevel = UpdateFolder(folderID, folderName, description,
        // ...)`, compared `== ACCESS_NONE` to decide whether the folder is
        // still ours (`bookmarkSvc.py:413-416`) **[V]**. So an unknown folder
        // answers `ACCESS_NONE`, which is the client's own path for "gone".
        "UpdateFolder" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let name = text_arg(req.args.get(1));
            let description = text_arg(req.args.get(2));
            let Some(f) = reg.bookmarks.folder_mut(actor, folder_id) else {
                return Ok(PyValue::Int(ACCESS_NONE));
            };
            f.name = name;
            f.description = description;
            reg.dirty = true;
            Ok(PyValue::Int(ACCESS_PERSONAL))
        }
        // `DeleteFolder(folderID)`, value discarded (`:425`) **[V]**. The
        // folder's bookmarks and subfolders go with it, as the client's own
        // `RemoveFolderContentFromCache` assumes (`:427`).
        "DeleteFolder" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_some() {
                b.folders.retain(|f| f.folder_id != folder_id);
                b.marks.retain(|m| m.folder_id != folder_id);
                b.subfolders.retain(|s| s.folder_id != folder_id);
                reg.dirty = true;
            }
            Ok(PyValue::None)
        }
        // Forgetting a *shared* folder; value discarded (`:431`) **[V]**. A
        // personal folder is not "known", it is owned, so there is nothing
        // to forget and nothing is deleted.
        "RemoveFromKnownFolders" => Ok(PyValue::None),
        // `folder, bookmarks, subfolders = UpdateKnownFolderState(folderID,
        // isActive)` (`:450`) and the same triple from `AddToKnownFolders`
        // (`:335`) **[V]**. An unknown folder keeps the old answer,
        // `FolderAccessDenied`, which `UpdateFolderActiveState` catches and
        // turns into "drop the folder and refresh" (`:452-454`).
        "UpdateKnownFolderState" | "AddToKnownFolders" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let is_active = bool_arg(req.args.get(1));
            let Some(f) = reg.bookmarks.folder_mut(actor, folder_id) else {
                return Err(refuse("FolderAccessDenied"));
            };
            if f.is_active != is_active {
                f.is_active = is_active;
                reg.dirty = true;
            }
            let b = &reg.bookmarks;
            let row = b
                .folder(actor, folder_id)
                .map_or(PyValue::None, Folder::row);
            let (marks, subs) = if is_active {
                (
                    b.in_folder(folder_id).map(Mark::row).collect(),
                    b.subfolders
                        .iter()
                        .filter(|s| s.folder_id == folder_id)
                        .map(Subfolder::row)
                        .collect(),
                )
            } else {
                (vec![], vec![])
            };
            Ok(PyValue::Tuple(vec![
                row,
                PyValue::List(marks),
                PyValue::List(subs),
            ]))
        }
        // The folder row for a folder the client does not have cached
        // (`:491`); `ShowBookmarkFolderInfo` catches `FolderAccessDenied`
        // and shows its own "no access" text (`:511-513`) **[V]**.
        "GetFolderInfo" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            reg.bookmarks
                .folder(actor, folder_id)
                .map(Folder::row)
                .ok_or_else(|| refuse("FolderAccessDenied"))
        }
        // `BookmarkStaticLocation(itemID, folderID, name, comment, expiry,
        // subfolderID=)` -> the 8-tuple (`bookmarkSvc.py:568-569`) **[V]**.
        // Reached for a region, constellation, system, station or
        // structure; the in-space case goes to `beyonce` instead.
        "BookmarkStaticLocation" => {
            let item_id = id_arg(req, 0, "itemID")?;
            let Some(place) = static_place(reg, actor, item_id) else {
                return Err(refuse_with(
                    "This server does not know where that is, so it cannot be bookmarked yet.",
                ));
            };
            create_mark(reg, req, actor, place)
        }
        // `UpdateBookmark(bookmarkID, folderID, name, note, subfolderID,
        // newFolderID, expiryCancel)`, value discarded; the client then
        // writes the same five fields onto its cached row (`:629-635`)
        // **[V]**, so ours must end up identical or a reopen disagrees.
        "UpdateBookmark" => {
            let bookmark_id = id_arg(req, 0, "bookmarkID")?;
            let folder_id = id_arg(req, 1, "folderID")?;
            let new_folder_id = crate::pyutil::opt_id(req.args.get(5)).unwrap_or(folder_id);
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_none() || b.folder(actor, new_folder_id).is_none() {
                return Err(refuse("BookmarkFolderNoLongerThere"));
            }
            let subfolder_id =
                checked_subfolder(b, new_folder_id, crate::pyutil::opt_id(req.args.get(4)));
            let Some(m) = b
                .marks
                .iter_mut()
                .find(|m| m.bookmark_id == bookmark_id && m.folder_id == folder_id)
            else {
                return Err(refuse("BookmarkNotAvailable"));
            };
            m.memo = text_arg(req.args.get(2));
            m.note = text_arg(req.args.get(3));
            m.subfolder_id = subfolder_id;
            m.folder_id = new_folder_id;
            if bool_arg(req.args.get(6)) {
                m.expiry = None;
            }
            reg.dirty = true;
            Ok(PyValue::None)
        }
        // `deleted = DeleteBookmarks(folderID, bookmarkIDs)`, then
        // `deletedBookmarks.extend(deleted)` and `CouldNotDeleteBookmark`
        // if nothing at all was deleted (`:647-650`) **[V]**. So the answer
        // is the ids actually removed, and an empty list is the client's own
        // refusal path. `bookmarkIDs` is a Python `set`, which arrives as a
        // reduce object; `type_ids_of` already reads that shape.
        "DeleteBookmarks" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let wanted = super::type_ids_of(req.args.get(1));
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_none() {
                return Ok(PyValue::List(vec![]));
            }
            let mut deleted = Vec::new();
            b.marks.retain(|m| {
                let hit = m.folder_id == folder_id && wanted.contains(&m.bookmark_id);
                if hit {
                    deleted.push(PyValue::Int(m.bookmark_id));
                }
                !hit
            });
            if !deleted.is_empty() {
                reg.dirty = true;
            }
            Ok(PyValue::List(deleted))
        }
        // `subfolder = CreateSubfolder(folderID, subfolderName)`, stored
        // under `subfolder.subfolderID` if truthy (`:525-527`) **[V]**.
        "CreateSubfolder" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_none() {
                return Err(refuse("BookmarkFolderNoLongerThere"));
            }
            if b.subfolders
                .iter()
                .filter(|s| s.folder_id == folder_id)
                .count()
                >= MAX_SUBFOLDERS
            {
                return Err(refuse("CannotCreateMoreSubfoldersInFolder"));
            }
            let sub = Subfolder {
                subfolder_id: b.alloc(),
                folder_id,
                creator_id: actor,
                name: text_arg(req.args.get(1)),
            };
            let row = sub.row();
            b.subfolders.push(sub);
            reg.dirty = true;
            Ok(row)
        }
        // `if UpdateSubfolder(folderID, subfolderID, name): ... else: raise
        // UserError('BookmarkSubfolderNoLongerThere')` (`:531-535`) **[V]**:
        // a boolean, and `False` is the client's own refusal.
        "UpdateSubfolder" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let subfolder_id = id_arg(req, 1, "subfolderID")?;
            let name = text_arg(req.args.get(2));
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_none() {
                return Ok(PyValue::Bool(false));
            }
            let Some(s) = b
                .subfolders
                .iter_mut()
                .find(|s| s.subfolder_id == subfolder_id && s.folder_id == folder_id)
            else {
                return Ok(PyValue::Bool(false));
            };
            s.name = name;
            reg.dirty = true;
            Ok(PyValue::Bool(true))
        }
        // `deletedBookmarkIDs = DeleteSubfolder(folderID, subfolderID)`,
        // iterated and evicted from the cache (`:546-551`) **[V]** -- so the
        // subfolder's bookmarks are deleted with it, which the client warns
        // about first (`ConfirmDeleteSubfolder`).
        "DeleteSubfolder" => {
            let folder_id = id_arg(req, 0, "folderID")?;
            let subfolder_id = id_arg(req, 1, "subfolderID")?;
            let b = &mut reg.bookmarks;
            if b.folder(actor, folder_id).is_none() {
                return Ok(PyValue::List(vec![]));
            }
            let mut deleted = Vec::new();
            b.marks.retain(|m| {
                let hit = m.folder_id == folder_id && m.subfolder_id == Some(subfolder_id);
                if hit {
                    deleted.push(PyValue::Int(m.bookmark_id));
                }
                !hit
            });
            b.subfolders
                .retain(|s| !(s.subfolder_id == subfolder_id && s.folder_id == folder_id));
            reg.dirty = true;
            Ok(PyValue::List(deleted))
        }
        // `rows, message = MoveBookmarksToFolderAndSubfolder(oldFolderID,
        // folderID, subfolderID, bookmarkIDs)`; each row's `bookmarkID`,
        // `folderID` and `subfolderID` are written onto the cache, and a
        // non-`None` message is shown with `prompt_player_fsd_dialog(*message)`
        // (`:671-677`) **[V]**.
        "MoveBookmarksToFolderAndSubfolder" => {
            let (old, dest, sub, ids) = move_args(reg, req, actor)?;
            let room = MAX_BOOKMARKS_IN_PERSONAL_FOLDER
                .saturating_sub(reg.bookmarks.in_folder(dest).count());
            let mut rows = Vec::new();
            let mut refused = false;
            for m in reg.bookmarks.marks.iter_mut() {
                if m.folder_id != old || !ids.contains(&m.bookmark_id) {
                    continue;
                }
                if old != dest && rows.len() >= room {
                    refused = true;
                    continue;
                }
                m.folder_id = dest;
                m.subfolder_id = sub;
                rows.push(keyval(vec![
                    ("bookmarkID", PyValue::Int(m.bookmark_id)),
                    ("folderID", PyValue::Int(dest)),
                    ("subfolderID", sub.map_or(PyValue::None, PyValue::Int)),
                ]));
            }
            if !rows.is_empty() {
                reg.dirty = true;
            }
            Ok(PyValue::Tuple(vec![
                PyValue::List(rows),
                partial_message(refused, "NotAllBookmarksMoved"),
            ]))
        }
        // `newBookmarks, message = CopyBookmarksToFolderAndSubfolder(...)`,
        // where `newBookmarks` is walked with `itervalues()` and merged with
        // `bookmarkCache.update(newBookmarks)` (`:682-687`) **[V]** -- a
        // **dict** of new id to row, not a list.
        "CopyBookmarksToFolderAndSubfolder" => {
            let (old, dest, sub, ids) = move_args(reg, req, actor)?;
            let room = MAX_BOOKMARKS_IN_PERSONAL_FOLDER
                .saturating_sub(reg.bookmarks.in_folder(dest).count());
            let sources: Vec<Mark> = reg
                .bookmarks
                .in_folder(old)
                .filter(|m| ids.contains(&m.bookmark_id))
                .cloned()
                .collect();
            let refused = sources.len() > room;
            let now = crate::pyutil::filetime_now();
            let mut new = Vec::new();
            for source in sources.into_iter().take(room) {
                let copy = Mark {
                    bookmark_id: reg.bookmarks.alloc(),
                    folder_id: dest,
                    subfolder_id: sub,
                    creator_id: actor,
                    created: now,
                    ..source
                };
                new.push((copy.row(), PyValue::Int(copy.bookmark_id)));
                reg.bookmarks.marks.push(copy);
            }
            if !new.is_empty() {
                reg.dirty = true;
            }
            Ok(PyValue::Tuple(vec![
                PyValue::Dict(new),
                partial_message(refused, "NotAllBookmarksCopied"),
            ]))
        }
        _ => Err(reg.no_method(req)),
    }
}

/// `(oldFolderID, folderID, subfolderID, bookmarkIDs)`, checked: both
/// folders must be the actor's.
fn move_args(
    reg: &Registry,
    req: &Request<'_>,
    actor: i64,
) -> Result<(i64, i64, Option<i64>, Vec<i64>), ServiceError> {
    let old = id_arg(req, 0, "oldFolderID")?;
    let dest = id_arg(req, 1, "folderID")?;
    let b = &reg.bookmarks;
    if b.folder(actor, old).is_none() || b.folder(actor, dest).is_none() {
        return Err(refuse("BookmarkFolderNoLongerThere"));
    }
    let sub = checked_subfolder(b, dest, crate::pyutil::opt_id(req.args.get(2)));
    Ok((old, dest, sub, super::type_ids_of(req.args.get(3))))
}

/// The `message` half of a move or copy: `None`, or the argument tuple for
/// `prompt_player_fsd_dialog(*message)`. Both keys exist and take no
/// arguments (`findings/user-errors.tsv`).
fn partial_message(refused: bool, key: &str) -> PyValue {
    if refused {
        PyValue::Tuple(vec![crate::pyutil::bstr(key)])
    } else {
        PyValue::None
    }
}

/// An empty `dbutil.CRowset` of modified-security systems.
///
/// Three columns and no rows: see the `map` arm for why the columns have to
/// be named even when there is nothing to put in them. `security` and
/// `modifiedSecurity` are `R5` -- which is CCP's own name for the 8-byte
/// double, not a typo for `R8` -- because `get_font_icon_text` compares them
/// as floats.
fn security_modified_systems() -> PyValue {
    let columns = [
        ("solarSystemID", DbType::I4),
        ("security", DbType::R5),
        ("modifiedSecurity", DbType::R5),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    let descriptor =
        DbRowDescriptor::new(columns).expect("the modified-security columns are a valid layout");
    eve_rowset::CRowset::new(descriptor).to_pyvalue()
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
    /// The one character in a single-character fixture, as an actor.
    ///
    /// Taken from the fixture rather than from inside the registry: finding
    /// the actor by reading "the first character" is the thing Round 0.3
    /// removed, and a test that did it would hide its return.
    fn actor(reg: &Registry) -> Option<i64> {
        Some(reg.config().characters[0].character_id)
    }

    fn call(reg: &mut Registry, service: &str, method: &str) -> Result<PyValue, ServiceError> {
        let actor = actor(reg);
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
    }

    fn call_args(reg: &mut Registry, service: &str, method: &str, args: &[PyValue]) -> PyValue {
        dispatch(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
        )
        .expect("answered")
    }

    fn kv_attr(v: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal expected, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .cloned()
            .unwrap_or_else(|| panic!("{name} missing"))
    }

    fn station_row(id: i64, system: i64, owner: i64) -> crate::service::character::StationRef {
        crate::service::character::StationRef {
            station_id: id,
            owner_id: owner,
            type_id: 1990,
            operation_id: 7,
            solar_system_id: system,
            constellation_id: 4,
            region_id: 5,
        }
    }

    /// A bookmark of a catalogued station nobody is based at lands in that
    /// station's own system, whether the caller is docked or in space in
    /// another system -- not in the caller's home (`docs/lanes/stale-system.md`).
    #[test]
    fn a_station_bookmark_names_the_stations_system() {
        use crate::space_state::{Motion, Presence};
        const ME: i64 = 90_000_901;
        const HOME: i64 = 60_007_001;
        const FAR: i64 = 60_007_002;
        let config = SandboxConfig {
            celestials: crate::celestial::Catalogue::parse(&format!(
                "#system\t7001\t10\t100\n#system\t7002\t11\t101\n\
             station\t{FAR}\t1990\t0.0\t1.0\t2.0\t3.0\t7002\t0\t0\n"
            )),
            characters: vec![crate::model::Character {
                character_id: ME,
                name: "Me".to_owned(),
                station_id: Some(HOME),
                solar_system_id: Some(7_001),
                ..crate::model::Character::default()
            }],
            ..SandboxConfig::default()
        };

        let mut reg = Registry::new(config);
        reg.stations.insert(FAR, station_row(FAR, 7_002, 11));
        let at = |reg: &Registry| static_place(reg, ME, FAR).map(|p| p.location_id);
        assert_eq!(at(&reg), Some(7_002), "docked at home");
        reg.roster_mut().arrive(Presence::arriving(
            ME,
            ME + 1_000,
            7_003,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert_eq!(at(&reg), Some(7_002), "in space");
    }

    /// `stationSvc.GetStation` carries `stationTypeID` (notificationUtil).
    #[test]
    fn get_station_has_the_type_the_client_reads() {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.stations.insert(7001, station_row(7001, 3001, 11));
        let got = call_args(&mut reg, "stationSvc", "GetStation", &[PyValue::Int(7001)]);
        assert_eq!(kv_attr(&got, "stationTypeID"), PyValue::Int(1990));
        assert_eq!(kv_attr(&got, "solarSystemID"), PyValue::Int(3001));
        assert_eq!(kv_attr(&got, "ownerID"), PyValue::Int(11));
        assert!(matches!(kv_attr(&got, "x"), PyValue::Real(_)));
        let none = call_args(&mut reg, "stationSvc", "GetStation", &[PyValue::Int(7999)]);
        assert_eq!(none, PyValue::None);
    }

    /// `GetStationsForOwner` lists both of a corp's stations as KeyVals.
    #[test]
    fn stations_for_owner_lists_both() {
        let mut reg = Registry::new(SandboxConfig::default());
        reg.stations.insert(7001, station_row(7001, 3001, 11));
        reg.stations.insert(7002, station_row(7002, 3002, 11));
        reg.stations.insert(7003, station_row(7003, 3003, 12));
        let got = call_args(
            &mut reg,
            "stationSvc",
            "GetStationsForOwner",
            &[PyValue::Int(11)],
        );
        let PyValue::List(rows) = got else {
            panic!("a list")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(kv_attr(&rows[0], "stationID"), PyValue::Int(7001));
        assert_eq!(kv_attr(&rows[1], "solarSystemID"), PyValue::Int(3002));
        let none = call_args(
            &mut reg,
            "stationSvc",
            "GetStationsForOwner",
            &[PyValue::Int(99)],
        );
        assert_eq!(none, PyValue::List(vec![]));
    }

    /// The alliance typed empty: a list where it is iterated. (The
    /// structure ones moved to `service/structures.rs` with their service.)
    #[test]
    fn alliance_typed_empties() {
        let mut reg = Registry::new(SandboxConfig::default());
        let a = call_args(
            &mut reg,
            "stationSvc",
            "GetSystemsForAlliance",
            &[PyValue::Int(5)],
        );
        assert_eq!(a, PyValue::List(vec![]));
    }

    /// **The jump checks' three `map` calls answer lists**, or every manual
    /// stargate jump dies client-side: `jump_checks.py:97,114` test
    /// membership in the answer and `mapSvc.py:660` adds two of them, so
    /// `None` is a `TypeError` before `CmdStargateJump` is sent [V].
    #[test]
    fn the_jump_checks_map_calls_answer_lists() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "GetTriglavianMinorVictorySystems",
            "GetEdencomFortressSystems",
            "GetEdencomMinorVictorySystems",
        ] {
            assert_eq!(
                call(&mut reg, "map", method).expect(method),
                PyValue::List(vec![]),
                "{method}"
            );
        }
    }

    fn three_stations() -> Registry {
        let mut stations = crate::stations::Table::default();
        for (id, system, owner, op) in [
            (60_001, 30_001, 1_001, 7),
            (60_002, 30_001, 1_002, 8),
            (60_003, 30_002, 1_001, 9),
        ] {
            stations.insert(
                id,
                crate::stations::Station {
                    system_id: system,
                    region_id: 1,
                    owner_id: owner,
                    operation_id: op,
                },
            );
        }
        let mut content = crate::content::Content::fixture();
        content.insert("stations", stations);
        Registry::new(SandboxConfig {
            characters: vec![],
            ..SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(content))
    }

    fn ints(v: &eve_rowset::CRowset, row: usize) -> Vec<i64> {
        v.rows()[row]
            .values()
            .iter()
            .map(|c| match c {
                eve_marshal::DbValue::Int(n) => *n,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// **MAP-STATIONS**: the whole table is in `GetStationInfo` and
    /// `GetStationCount`, the latter as tuples.
    #[test]
    fn the_map_lists_every_station_and_counts_them_per_system() {
        let mut reg = three_stations();
        let rs = reg.station_rowset(None);
        assert_eq!(rs.len(), 3);
        assert_ne!(
            call_args(&mut reg, "map", "GetStationInfo", &[]),
            PyValue::None
        );
        assert_eq!(ints(&rs, 1), vec![60_002, 30_001, 1_002, 8]);
        let count = call_args(&mut reg, "map", "GetStationCount", &[]);
        let pair = |s, n| PyValue::Tuple(vec![PyValue::Int(s), PyValue::Int(n)]);
        assert_eq!(count, PyValue::List(vec![pair(30_001, 2), pair(30_002, 1)]));
    }

    /// With no table, a seeded station is still listed and counted.
    #[test]
    fn an_empty_table_still_lists_a_seeded_station() {
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![],
            ..SandboxConfig::default()
        });
        reg.stations.insert(7001, station_row(7001, 3001, 11));
        let rs = reg.station_rowset(None);
        assert_eq!(rs.len(), 1);
        assert_eq!(ints(&rs, 0), vec![7001, 3001, 11, 7]);
        assert_eq!(
            call_args(&mut reg, "map", "GetStationCount", &[]),
            PyValue::List(vec![PyValue::Tuple(vec![
                PyValue::Int(3001),
                PyValue::Int(1)
            ])])
        );
    }

    /// **MAP-PERSONAL**: corp members are `None` in an NPC corp, and a player
    /// corp lists the online mates but not the caller or the offline.
    #[test]
    fn map_corp_members_follow_the_corp() {
        const ME: i64 = 90_000_001;
        const MATE: i64 = 90_000_002;
        const GONE: i64 = 90_000_003;
        let characters = [ME, MATE, GONE]
            .into_iter()
            .map(|id| Character {
                character_id: id,
                ship_id: id + 100,
                solar_system_id: Some(3001),
                ..Character::default()
            })
            .collect();
        let mut reg = Registry::new(SandboxConfig {
            characters,
            ..SandboxConfig::default()
        });
        for (n, id) in [ME, MATE].into_iter().enumerate() {
            let n = n as i64 + 1;
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        let ask = |reg: &mut Registry| crate::visits::members_answer(reg, Some(ME));
        assert_eq!(ask(&mut reg), PyValue::None);
        for id in [ME, MATE, GONE] {
            reg.corporations.enrol_for_test(id, 98_000_001, 0);
        }
        assert_ne!(ask(&mut reg), PyValue::None);
        let rs = crate::visits::members_rowset(&reg, Some(ME)).expect("a player corp");
        assert_eq!(rs.len(), 1);
        assert_eq!(ints(&rs, 0), vec![MATE, 3001]);
    }

    /// The three empty answers keep the client's container families.
    #[test]
    fn the_map_empties_are_a_dict_and_two_lists() {
        let mut reg = Registry::new(SandboxConfig::default());
        assert_eq!(
            call(&mut reg, "map", "GetIndustryCostModifier").unwrap(),
            PyValue::Dict(vec![])
        );
        for m in ["GetSystemsInIncursions", "GetAllRoamingWeatherSystems"] {
            assert_eq!(call(&mut reg, "map", m).unwrap(), PyValue::List(vec![]));
        }
    }

    /// **MAP-STATS**: `map.GetHistory` is a `CachedMethodCallResult` whose
    /// rowset has the four columns the filters read (spec S1).
    #[test]
    fn get_history_answers_a_cached_four_column_rowset() {
        let mut reg = Registry::new(SandboxConfig::default());
        let args = [PyValue::Int(3), PyValue::Int(24)];
        let actor = actor(&reg);
        let req = Request {
            service: "map",
            method: "GetHistory",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor,
        };
        let answer = dispatch(&mut reg, &req).unwrap();
        let PyValue::Instance { class, state } = answer else {
            panic!("an instance")
        };
        assert_eq!(class, crate::pyutil::CACHED_METHOD_CALL_RESULT.as_bytes());
        let PyValue::Tuple(t) = *state else {
            panic!("tuple")
        };
        let PyValue::Bytes(b) = &t[1] else {
            panic!("bytes")
        };
        crate::wire::loads(b).unwrap();
        for col in ["solarSystemID", "value1", "value2", "value3"] {
            assert!(b.windows(col.len()).any(|w| w == col.as_bytes()), "{col}");
        }
    }

    /// **MAP-PILOTS**: a key other than "EVE" returns the client's default.
    #[test]
    fn cluster_statistics_echo_the_default_for_another_key() {
        let mut reg = Registry::new(SandboxConfig::default());
        let args = [
            PyValue::Bytes(b"OTHER".to_vec()),
            PyValue::Tuple(vec![PyValue::Int(7)]),
        ];
        let actor = actor(&reg);
        let req = Request {
            service: "machoNet",
            method: "GetClusterGameStatisticsForClient",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor,
        };
        assert_eq!(reg.dispatch(&req).unwrap(), args[1]);
        let args = [PyValue::Bytes(b"EVE".to_vec()), args[1].clone()];
        let req = Request { args: &args, ..req };
        assert!(matches!(reg.dispatch(&req).unwrap(), PyValue::Tuple(t) if t.len() == 3));
    }

    /// `map.GetStationCount` must break the live lock *and* survive the one
    /// consumer out of three that unpacks it.
    ///
    /// Two assertions, and the first one is the one that matters. The guard
    /// at `mapFilterStationCount.py(16)`, `mapViewColorHandler.py(647)` and
    /// `starModeHandler.py(451)` is `is None` (`COMPARE_OP 8`), and the
    /// store that follows it **completes**, so `None` is the one value that
    /// fails its own guard and every read re-issues the call. That failure
    /// raises nothing, logs nothing and appears in no instrument we own --
    /// so "not `None`" is the property to assert here, exactly as
    /// truthiness is the property to assert against an `if not x:` guard. A
    /// test that only checked "it is a list" would pass for the broken
    /// answer on a day someone changed the arm to `None`.
    ///
    /// The second guards the container family. `starModeHandler.py(452)`
    /// does `FOR_ITER` then `UNPACK_SEQUENCE 2`; iterating a dict yields
    /// keys, so the obvious-looking `PyValue::Dict` raises there while
    /// passing the `dict(history)` of the other two call sites.
    #[test]
    fn get_station_count_latches_the_cache_and_comes_apart_in_pairs() {
        let mut reg = Registry::new(SandboxConfig::default());
        let answer = call(&mut reg, "map", "GetStationCount").unwrap();

        assert_ne!(
            answer,
            PyValue::None,
            "`starmap.stationCountCache` is guarded with `is None` and the \
             store completes, so None poisons the cache and the map re-asks \
             forever, silently"
        );

        let PyValue::List(rows) = answer else {
            panic!(
                "GetStationCount must be a sequence of pairs, not a mapping: \
                 starModeHandler.py(452) does `for solarSystemID, amount in \
                 history`, and iterating a dict yields its keys"
            )
        };
        for row in &rows {
            let pair = match row {
                PyValue::Tuple(p) | PyValue::List(p) => p,
                other => panic!("each row is unpacked as a 2-sequence, got {other:?}"),
            };
            assert_eq!(
                pair.len(),
                2,
                "UNPACK_SEQUENCE 2 at starModeHandler.py(452), twice"
            );
        }
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

    // ---- bookmarks ------------------------------------------------------

    const HOME_SYSTEM: i64 = 30_000_001;
    const HOME_STATION: i64 = 60_012_526;

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.to_owned())
    }

    fn call_as(
        reg: &mut Registry,
        actor: i64,
        service: &str,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service,
            method,
            args,
            kwargs,
            bind_params: None,
            actor: Some(actor),
        };
        if service == "beyonce" && method == "BookmarkScanResult" {
            bookmark_scan_result(reg, &req)
        } else if service == "beyonce" {
            bookmark_location(reg, &req)
        } else {
            dispatch(reg, &req)
        }
    }

    fn me(reg: &Registry) -> i64 {
        actor(reg).expect("the fixture has a character")
    }

    /// A Python `set`, as it arrives: tag 0x22 around `__builtin__.set`.
    fn set_of(ids: &[i64]) -> PyValue {
        PyValue::ObjectEx {
            form: eve_marshal::ObjectForm::Call,
            header: Box::new(PyValue::Tuple(vec![
                PyValue::Global(b"__builtin__.set".to_vec()),
                PyValue::Tuple(vec![PyValue::List(
                    ids.iter().map(|i| PyValue::Int(*i)).collect(),
                )]),
            ])),
            list: Vec::new(),
            dict: Vec::new(),
        }
    }

    fn attr<'a>(row: &'a PyValue, name: &str) -> &'a PyValue {
        let PyValue::Instance { state, .. } = row else {
            panic!("a KeyVal row, got {row:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("the client reads `{name}` and the row has none: {row:?}"))
    }

    /// `CreateDefaultFolder`'s own call (`bookmarkSvc.py:407`): personal,
    /// a name, an empty description and four `None` group ids.
    fn add_folder(reg: &mut Registry, actor: i64, name: &str) -> i64 {
        let mut args = vec![PyValue::Bool(true), u(name), u("")];
        args.extend([PyValue::None, PyValue::None, PyValue::None, PyValue::None]);
        let row = call_as(
            reg,
            actor,
            "accessGroupBookmarkMgr",
            "AddFolder",
            &args,
            &[],
        )
        .unwrap();
        let PyValue::Int(id) = attr(&row, "folderID") else {
            panic!("folderID is a dict key in foldersNew")
        };
        *id
    }

    /// `folders, bookmarks, subfolders`, as three vectors.
    fn prime(reg: &mut Registry, actor: i64) -> [Vec<PyValue>; 3] {
        let got = call_as(
            reg,
            actor,
            "accessGroupBookmarkMgr",
            "GetMyActiveBookmarks",
            &[],
            &[],
        )
        .unwrap();
        let PyValue::Tuple(parts) = got else {
            panic!("UNPACK_SEQUENCE 3 at bookmarkSvc.py:164")
        };
        let [PyValue::List(f), PyValue::List(b), PyValue::List(s)] =
            <[PyValue; 3]>::try_from(parts).expect("exactly three parts")
        else {
            panic!("each part is iterated and `len()`ed")
        };
        [f, b, s]
    }

    fn bookmark_station(reg: &mut Registry, actor: i64, folder: i64, name: &str) -> PyValue {
        call_as(
            reg,
            actor,
            "accessGroupBookmarkMgr",
            "BookmarkStaticLocation",
            &[
                PyValue::Int(HOME_STATION),
                PyValue::Int(folder),
                u(name),
                u("a note"),
                PyValue::Int(0),
            ],
            &[(PyValue::None, crate::pyutil::bstr("subfolderID"))],
        )
        .unwrap()
    }

    fn created_id(created: &PyValue) -> i64 {
        let PyValue::Tuple(t) = created else {
            panic!("UNPACK_SEQUENCE 8 at bookmarkSvc.py:569, got {created:?}")
        };
        assert_eq!(
            t.len(),
            8,
            "bookmarkID, itemID, typeID, x, y, z, locationID, expiryDate"
        );
        let PyValue::Int(id) = t[0] else {
            panic!("`if bookmarkID:` at :577 needs a truthy id")
        };
        assert_ne!(
            id, 0,
            "`if bookmarkID:` at :577: a zero id is a silent no-op"
        );
        id
    }

    /// The first thing the window does with an empty answer is create a
    /// folder (`bookmarkSvc.py:175-176`), and the folder must be there the
    /// next time the client primes -- which is what "reopening the window"
    /// and "logging in again" both are.
    #[test]
    fn the_default_folder_the_client_creates_is_there_on_the_next_prime() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        assert!(prime(&mut reg, me)[0].is_empty());

        let folder = add_folder(&mut reg, me, "Personal Locations");
        let [folders, _, _] = prime(&mut reg, me);
        assert_eq!(folders.len(), 1);
        let row = &folders[0];
        assert_eq!(attr(row, "folderID"), &PyValue::Int(folder));
        // The attributes the Places tab and the Save Location dialog read
        // off a folder, every one of them unguarded.
        assert_eq!(attr(row, "isPersonal"), &PyValue::Bool(true));
        assert_eq!(attr(row, "folderName"), &u("Personal Locations"));
        for name in [
            "isActive",
            "accessLevel",
            "description",
            "adminGroupID",
            "manageGroupID",
            "useGroupID",
            "viewGroupID",
        ] {
            attr(row, name);
        }
    }

    /// Docked "Save Location": a station bookmark, listed on the next prime
    /// with the attributes `NewBookmarkCreated` puts on its own copy.
    #[test]
    fn a_station_bookmark_is_listed_under_its_solar_system() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let folder = add_folder(&mut reg, me, "Personal Locations");
        let created = bookmark_station(&mut reg, me, folder, "Home");
        let id = created_id(&created);
        let PyValue::Tuple(t) = &created else {
            unreachable!()
        };
        // `bookmarkIDsBySolarystemID[bm.locationID]` (`:168`).
        assert_eq!(t[6], PyValue::Int(HOME_SYSTEM));
        assert_eq!(t[1], PyValue::Int(HOME_STATION));

        let [_, marks, _] = prime(&mut reg, me);
        assert_eq!(marks.len(), 1);
        let row = &marks[0];
        assert_eq!(attr(row, "bookmarkID"), &PyValue::Int(id));
        assert_eq!(attr(row, "folderID"), &PyValue::Int(folder));
        assert_eq!(attr(row, "memo"), &u("Home"));
        assert_eq!(attr(row, "note"), &u("a note"));
        assert_eq!(attr(row, "locationID"), &PyValue::Int(HOME_SYSTEM));
        for name in [
            "itemID",
            "typeID",
            "flag",
            "created",
            "expiry",
            "x",
            "y",
            "z",
            "subfolderID",
            "creatorID",
        ] {
            attr(row, name);
        }
    }

    /// In-space "Save Location" sends the player's own ship
    /// (`addressbookService.py:475`) to `beyonce.BookmarkLocation`, and the
    /// answer must be what `IsCoordinateBookmark` calls a point:
    /// `typeID == typeSolarSystem` and an `x` (`bookmarkUtil.py:144`).
    /// Then a warp to it aims at that point.
    #[test]
    fn a_location_saved_in_space_is_a_point_you_can_warp_to() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let ship = reg.config().characters[0].ship_id;
        let here = [1.0e9, -2.5e8, 3.0e7];
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                me,
                ship,
                HOME_SYSTEM,
                crate::space_state::Motion::arrived(here, [0.0; 3]),
            ));
        let folder = add_folder(&mut reg, me, "Personal Locations");

        let created = call_as(
            &mut reg,
            me,
            "beyonce",
            "BookmarkLocation",
            &[
                PyValue::Int(ship),
                PyValue::Int(folder),
                u("spot"),
                u(""),
                PyValue::Int(0),
            ],
            &[(PyValue::None, crate::pyutil::bstr("subfolderID"))],
        )
        .unwrap();
        let id = created_id(&created);
        let PyValue::Tuple(t) = &created else {
            unreachable!()
        };
        assert_eq!(t[2], PyValue::Int(TYPE_SOLAR_SYSTEM));
        assert_eq!(
            t[3..6],
            [
                PyValue::Real(here[0]),
                PyValue::Real(here[1]),
                PyValue::Real(here[2])
            ]
        );
        assert_eq!(t[6], PyValue::Int(HOME_SYSTEM));

        let target = bookmark_target(&reg, me, id).expect("a point in this system is warpable");
        assert_eq!(target.position, here);

        // From another system the same bookmark is not a warp target: the
        // client offers "Set Destination" there, not "Warp To".
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                me,
                ship,
                HOME_SYSTEM + 1,
                crate::space_state::Motion::arrived(here, [0.0; 3]),
            ));
        assert_eq!(bookmark_target(&reg, me, id), None);
    }

    /// Docked, the ballpark half of the create has no park to bookmark in,
    /// and refuses with the client's own key for that (`bookmarkSvc.py:573`).
    #[test]
    fn saving_a_point_in_space_while_docked_is_refused_by_name() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let ship = reg.config().characters[0].ship_id;
        let folder = add_folder(&mut reg, me, "Personal Locations");
        let err = call_as(
            &mut reg,
            me,
            "beyonce",
            "BookmarkLocation",
            &[
                PyValue::Int(ship),
                PyValue::Int(folder),
                u("spot"),
                u(""),
                PyValue::Int(0),
            ],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err,
            ServiceError::Raise(eve_proto::user_error("BkmMustBeInFlight", vec![]))
        );
    }

    /// Rename, then delete: the two edits the Places tab offers on a row.
    ///
    /// `UpdateBookmark`'s seven positionals are `bookmarkSvc.py:629`, and
    /// `DeleteBookmarks` gets a `set` (`:640-647`) and must answer the ids it
    /// removed, because an empty answer is `CouldNotDeleteBookmark` (`:650`).
    #[test]
    fn a_bookmark_can_be_renamed_and_then_deleted() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let folder = add_folder(&mut reg, me, "Personal Locations");
        let id = created_id(&bookmark_station(&mut reg, me, folder, "Home"));

        call_as(
            &mut reg,
            me,
            "accessGroupBookmarkMgr",
            "UpdateBookmark",
            &[
                PyValue::Int(id),
                PyValue::Int(folder),
                u("Renamed"),
                u("new note"),
                PyValue::None,
                PyValue::Int(folder),
                PyValue::Bool(false),
            ],
            &[],
        )
        .unwrap();
        let [_, marks, _] = prime(&mut reg, me);
        assert_eq!(attr(&marks[0], "memo"), &u("Renamed"));
        assert_eq!(attr(&marks[0], "note"), &u("new note"));

        let deleted = call_as(
            &mut reg,
            me,
            "accessGroupBookmarkMgr",
            "DeleteBookmarks",
            &[PyValue::Int(folder), set_of(&[id])],
            &[],
        )
        .unwrap();
        assert_eq!(deleted, PyValue::List(vec![PyValue::Int(id)]));
        assert!(prime(&mut reg, me)[1].is_empty());
    }

    /// Bookmarks are the player's own work, so they go through the world
    /// file: a real `FileStore` round trip, through `to_snapshot` and
    /// `apply_snapshot`, the path a restart takes.
    #[test]
    fn bookmarks_survive_the_world_file() {
        use crate::store::Store as _;
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let folder = add_folder(&mut reg, me, "Tabs\tand\nnewlines %");
        bookmark_station(&mut reg, me, folder, "Home");
        let before = prime(&mut reg, me);

        let path = std::env::temp_dir().join(format!(
            "eve-offline-bookmarks-{}-{:?}.world",
            std::process::id(),
            std::thread::current().id()
        ));
        let store = crate::store::FileStore::new(&path);
        store.save(&reg.to_snapshot()).unwrap();
        let loaded = store.load().unwrap().expect("the file was just written");
        let _ = std::fs::remove_file(&path);

        let mut again = Registry::new(SandboxConfig::default());
        again.apply_snapshot(loaded);
        assert_eq!(prime(&mut again, me)[..2], before[..2]);
        // And the allocator moved on rather than restarting: a reused id
        // would land on a key the client already holds.
        let next = add_folder(&mut again, me, "second");
        assert!(next > folder);
    }

    /// The text records read back exactly, including an empty position.
    #[test]
    fn bookmark_records_round_trip() {
        let mut b = Bookmarks::default();
        let folder = b.alloc();
        b.folders.push(Folder {
            folder_id: folder,
            owner_id: 7,
            name: "a\tb".to_owned(),
            description: String::new(),
            is_active: false,
        });
        let sub = b.alloc();
        b.subfolders.push(Subfolder {
            subfolder_id: sub,
            folder_id: folder,
            creator_id: 7,
            name: "sub".to_owned(),
        });
        for (position, subfolder_id) in [(None, None), (Some([0.5, -1.0e12, 3.0]), Some(sub))] {
            let bookmark_id = b.alloc();
            b.marks.push(Mark {
                bookmark_id,
                folder_id: folder,
                subfolder_id,
                creator_id: 7,
                item_id: position.map_or(Some(HOME_SYSTEM), |_| None),
                type_id: TYPE_SOLAR_SYSTEM,
                position,
                location_id: HOME_SYSTEM,
                memo: "m%".to_owned(),
                note: "line\nbreak".to_owned(),
                created: 133_000_000_000_000_000,
                expiry: Some(133_000_000_000_000_001),
            });
        }
        let mut text = String::new();
        b.render(&mut text);
        let mut back = Bookmarks::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Bookmarks::RECORDS.contains(&f[0]));
            back.parse_record(&f).unwrap();
        }
        assert_eq!(back, b);
    }

    /// One character's folders are not another's: neither visible nor
    /// writable. `FolderAccessDenied` is the key the client catches for an
    /// unreadable folder (`bookmarkSvc.py:511-512`).
    #[test]
    fn another_characters_folder_is_neither_listed_nor_writable() {
        let alice = crate::model::Character::default();
        let bob = crate::model::Character {
            character_id: alice.character_id + 1,
            ship_id: alice.ship_id + 1,
            ..crate::model::Character::default()
        };
        let (a, b) = (alice.character_id, bob.character_id);
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        });
        let folder = add_folder(&mut reg, a, "Alice's");
        bookmark_station(&mut reg, a, folder, "Home");

        assert!(prime(&mut reg, b).iter().all(Vec::is_empty));
        assert_eq!(
            call_as(
                &mut reg,
                b,
                "accessGroupBookmarkMgr",
                "GetFolderInfo",
                &[PyValue::Int(folder)],
                &[]
            ),
            Err(ServiceError::Raise(eve_proto::user_error(
                "FolderAccessDenied",
                vec![]
            )))
        );
        let err = call_as(
            &mut reg,
            b,
            "accessGroupBookmarkMgr",
            "BookmarkStaticLocation",
            &[
                PyValue::Int(HOME_STATION),
                PyValue::Int(folder),
                u("x"),
                u(""),
                PyValue::Int(0),
            ],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, ServiceError::Raise(_)), "{err:?}");
        assert_eq!(
            prime(&mut reg, a)[1].len(),
            1,
            "Alice's bookmark is untouched"
        );
    }

    /// An expired bookmark is dropped by the client's own cache
    /// (`RemoveExpiredBookmarks`, `bookmarkSvc.py:187`); it must not come
    /// back from us on the next prime.
    #[test]
    fn an_expired_bookmark_is_not_sent_again() {
        let mut reg = Registry::new(SandboxConfig::default());
        let me = me(&reg);
        let folder = add_folder(&mut reg, me, "Personal Locations");
        bookmark_station(&mut reg, me, folder, "Home");
        reg.bookmarks.marks[0].expiry = Some(1);
        assert!(prime(&mut reg, me)[1].is_empty());
    }

    /// A call with no arguments is a malformed call, not a refusal: the
    /// shared census in `service/tests.rs` probes `AddFolder` and
    /// `UpdateKnownFolderState` bare and checks every `UserError` it sees.
    #[test]
    fn a_bare_call_is_bad_arguments_rather_than_a_user_error() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "AddFolder",
            "UpdateKnownFolderState",
            "BookmarkStaticLocation",
        ] {
            match call(&mut reg, "accessGroupBookmarkMgr", method) {
                Err(ServiceError::BadArguments { .. }) => {}
                other => panic!("{method} with no arguments answered {other:?}"),
            }
        }
    }

    /// The contract wizard's assignee search (`searchOld.Search` with
    /// `categoryOwner`): one match is the id itself, which the wizard keeps
    /// as `assigneeID`; a list that names no character group finds nothing.
    #[test]
    fn the_owner_search_finds_a_character_by_name() {
        let alice = Character {
            character_id: 90_000_001,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..Character::default()
        };
        let bob = Character {
            character_id: 90_000_002,
            name: "Bob".to_owned(),
            ship_id: 100_000_002,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice, bob],
            ..SandboxConfig::default()
        });
        let owners = PyValue::List(vec![PyValue::Int(2), PyValue::Int(3)]);
        let got = call_as(
            &mut reg,
            90_000_001,
            "search",
            "QuickQuery",
            &[u("bob"), owners],
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::List(vec![PyValue::Int(90_000_002)]));
        let got = call_as(
            &mut reg,
            90_000_001,
            "search",
            "QuickQuery",
            &[u("bob"), PyValue::List(vec![PyValue::Int(3)])],
            &[],
        )
        .unwrap();
        assert_eq!(got, PyValue::List(vec![]));
    }

    /// GAP-QUICKFIX B6: a scan result saved as a coordinate bookmark, and a
    /// point bookmark is its own warp-in point.
    #[test]
    fn a_scan_result_saves_as_a_point_you_can_warp_onto() {
        let bob = Character {
            character_id: 90_000_077,
            name: "Bob".to_owned(),
            ship_id: 100_000_077,
            ..Character::default()
        };
        let mut cfg = SandboxConfig::default();
        cfg.characters.push(bob);
        let mut reg = Registry::new(cfg);
        let me = me(&reg);
        let ship = reg.config().characters[0].ship_id;
        let there = [4.0e9, 1.0e6, -2.0e6];
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                me,
                ship,
                HOME_SYSTEM,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                90_000_077,
                100_000_077,
                HOME_SYSTEM,
                crate::space_state::Motion::arrived(there, [0.0; 3]),
            ));
        let folder = add_folder(&mut reg, me, "Personal Locations");
        let rid = PyValue::Bytes(crate::scan::target_id(100_000_077).into_bytes());
        let args = |loc: i64| {
            [
                PyValue::Int(loc),
                u("Sig"),
                u(""),
                rid.clone(),
                PyValue::Int(folder),
                PyValue::Int(0),
            ]
        };
        let created = call_as(
            &mut reg,
            me,
            "beyonce",
            "BookmarkScanResult",
            &args(HOME_SYSTEM),
            &[],
        )
        .unwrap();
        let PyValue::Tuple(t) = &created else {
            panic!("an 8-tuple")
        };
        assert_eq!(t.len(), 8);
        assert_eq!(t[1], PyValue::None);
        assert_eq!(t[2], PyValue::Int(TYPE_SOLAR_SYSTEM));
        assert_eq!(t[3], PyValue::Real(there[0]));
        let id = created_id(&created);
        let target = bookmark_target(&reg, me, id).expect("warpable");
        assert_eq!(target.warp_in, Some(there));
        // Align to the saved point: the course turns toward it.
        let kwargs = [
            (PyValue::None, crate::pyutil::bstr("dstID")),
            (PyValue::Int(id), crate::pyutil::bstr("bookmarkID")),
        ];
        let req = Request {
            service: "beyonce",
            method: "CmdAlignTo",
            args: &[],
            kwargs: &kwargs,
            bind_params: None,
            actor: Some(me),
        };
        reg.dispatch(&req).expect("answered");
        let motion = reg.roster().get(me).expect("in space").motion;
        assert!(
            matches!(motion.mode, crate::destiny::Mode::Goto(p) if p[0] > 0.0),
            "{:?}",
            motion.mode
        );
        // The wrong system is a refusal, never a falsy id.
        assert!(
            call_as(
                &mut reg,
                me,
                "beyonce",
                "BookmarkScanResult",
                &args(HOME_SYSTEM + 1),
                &[],
            )
            .is_err()
        );
    }
}
