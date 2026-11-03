//! Presence and the people lookups. (Notifications moved to
//! `service/notifications.rs`, NOTIF-1; mail to `service/mail.rs`, MAIL-1.)
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
    "onlineStatus",
    "petitioner",
    // The legacy name lookup behind the corp invite, give-ISK and owner
    // pickers. The unified `search` service lives in `world.rs` but asks
    // this module's `people_search` who matched.
    "lookupSvc",
];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // **The legacy name lookup: every window that predates the unified
        // search, and every method answers a list of rows.**
        //
        // Five client windows reach it, and each reads a different handful
        // of attributes off the rows -- which is why the rows are `KeyVal`s
        // (any attribute, iterable in a list, `len()`-able) rather than a
        // rowset with a fixed header. The columns each caller *reads* are
        // `[V]`; anything beyond them is `[A]` and says so below.
        //
        //  - the corp **invite** window (`corp_ui_applications.py:1143`):
        //    `LookupEvePlayerCharacters(str, 0)`, then
        //    `cfg.eveowners.Prime([each.characterID ...])` -- `characterID`.
        //  - **give ISK / pay a corp** (`noneNPCAccountOwnerDialog.py:110`):
        //    `LookupNoneNPCAccountOwners(str, 0)`, then `each.ownerID`.
        //  - the chat **owner picker** (`pcownerpickerdialog.py:94`):
        //    `LookupOwners(str, 0)`, then `ownerID`, `ownerName`, `typeID`.
        //  - `searchOld.SearchOwners` (`searchOld.py:307`/`:309`) -- the
        //    mailing-list and right-click-menu owner pickers
        //    (`evemailingListConfig`, `menuFunctions`): `LookupPCOwners` when
        //    `hideNPC`, else `LookupOwners`, both `(str, exact)`, then
        //    `o.groupID in groupIDs`, `o.ownerName`, `o.ownerID`, `o.typeID`
        //    (`:313`-`:315`). So the owner row is those four.
        //  - the GM slash commands (`svc_slash.py:172`, `:1377`):
        //    `LookupCharacters(name, 1)` -> `characterID`;
        //    `LookupCorporations(name)` -> `corporationID`.
        //
        // Every caller tests `result is None or not len(result)` and shows its
        // own "nothing found" hint, so an empty list stays the honest answer
        // for a name nobody has. The matching is `people_match`'s, with the
        // legacy `exact` flag mapped onto it -- see `LegacyExact`.
        //
        // **Still `[]`, deliberately:** `LookupAlliances` and
        // `LookupFactions` (no alliances exist; factions are client static
        // data the slash command alone asks for), `LookupStations`,
        // `LookupKnownLocationsByGroup` and `LookupLocationsByGroup` -- places,
        // not people, and `searchOld.py:61`'s celestial search reads
        // `each.itemID` off rows we would have to invent from a catalogue we
        // do not index by name. `docs/lanes/lookup.md` has the list.
        "lookupSvc" => match req.method {
            // The war wizard's search (C8); called in parallel with another
            // search, so it is missing from `client-calls.tsv`.
            "LookupWarableCorporationsOrAlliances" => {
                super::war::lookup_svc(reg, req).unwrap_or(Ok(PyValue::List(vec![])))
            }
            "LookupEvePlayerCharacters" | "LookupCharacters" => {
                Ok(lookup_rows(
                    reg,
                    req,
                    &[PeopleKind::Character],
                    false,
                    |p| {
                        keyval(vec![
                            ("characterID", PyValue::Int(p.id)),
                            // `[A]`: no caller reads it; the name is what a
                            // character row has always carried beside its id.
                            ("characterName", PyValue::Unicode(p.name.clone())),
                        ])
                    },
                ))
            }
            "LookupCorporations" => Ok(lookup_rows(
                reg,
                req,
                &[PeopleKind::Corporation],
                false,
                |p| {
                    keyval(vec![
                        ("corporationID", PyValue::Int(p.id)),
                        // `[A]`, as `characterName` above.
                        ("corporationName", PyValue::Unicode(p.name.clone())),
                    ])
                },
            )),
            // "PC" and "none-NPC account": players and the corporations
            // players made. The NPC corp a new character starts in is
            // neither -- you cannot give it ISK or put it on a mailing list.
            "LookupPCOwners" | "LookupNoneNPCAccountOwners" => Ok(lookup_rows(
                reg,
                req,
                &[PeopleKind::Character, PeopleKind::Corporation],
                true,
                owner_row,
            )),
            "LookupOwners" => Ok(lookup_rows(
                reg,
                req,
                &[PeopleKind::Character, PeopleKind::Corporation],
                false,
                owner_row,
            )),
            "LookupAlliances"
            | "LookupFactions"
            | "LookupStations"
            | "LookupKnownLocationsByGroup"
            | "LookupLocationsByGroup" => Ok(PyValue::List(vec![])),
            _ => Err(reg.no_method(req)),
        },
        // (`mailMgr` and `mailingListsMgr` moved to `service/mail.rs`, MAIL-1.)
        // (The reasoning for `XmppChatMgr` used to sit here, orphaned from
        // the arm it describes -- that service is dispatched in
        // `service/base.rs`, which is where it now lives, with the two
        // things this copy had missed: the jail `drop`s 5222 so the dial
        // hangs rather than fails fast, and `Login` raises on the same
        // `None` two lines later regardless.)
        //
        // Who on your contact list is online.
        //
        // **A `CRowset`, and the family is forced, not chosen.** The
        // caller is one line:
        //
        //     reg.onlineStatus = sm.RemoteSvc(
        //         'onlineStatus').GetInitialState().Index('contactID')
        //
        // so `.Index` is called unconditionally -- `None` was an
        // `AttributeError` -- and the index column must exist. What picks
        // the family is what happens *later*:
        //
        //     reg.onlineStatus[charID] = blue.DBRow(
        //         reg.onlineStatus.header, [charID, True])
        //
        // `blue.DBRow` takes a **`DBRowDescriptor`**, not a list of
        // names. A `util.Rowset`'s `.header` is a plain list of strings,
        // which would raise there; only the `CRowset` family carries a
        // real descriptor. The opposite of `agentMgr.GetAgents`, which
        // must be a `Rowset` because `CRowset` has no `Clone()` -- so the
        // two families are genuinely not interchangeable, in both
        // directions.
        //
        // Two columns, both read off the client: the index key
        // `contactID`, and `online`, which `GetOnlineStatus` returns as
        // `reg.onlineStatus[charID].online` and which the client itself
        // writes as a Python `True`/`False`.
        "onlineStatus" => match req.method {
            "GetInitialState" => Ok(online_status_rowset(reg)),
            // Asked per-character on a cache miss, with the character id as
            // the argument -- so unlike `GetPublicInfo`, this one has always
            // been answerable honestly and used to answer `False` flatly
            // because nobody else was ever here.
            //
            // A missing or non-integer argument is `False`, not an error: the
            // caller is a cache fill and "not online" is a valid cache entry.
            "GetOnlineStatus" => Ok(PyValue::Bool(match req.args.first() {
                Some(PyValue::Int(id)) => reg.sessions().is_online(*id),
                _ => false,
            })),
            _ => Err(reg.no_method(req)),
        },
        // Support tickets -- "petitions" in the client's older strings.
        //
        // A self-contained subsystem: 35 census sites across three client
        // files (`neocom/petition.py`, `neocom/petitionWindow.py`,
        // `neocom/help.py`) plus three sites the census cannot see, and
        // **nothing else in the game reads any of it**. So it is the one
        // window in this round that can be closed out whole.
        //
        // Two of the three are worth naming here, because they are the
        // blind spot `docs/reference/client-call-surface.md` §5 describes and they both
        // matter: `petitioner.GetClientPickerInfo`
        // (`petitionWindow.py:319`, in `PopulatePicker`, `len()` first and
        // then subscripted) and a third `GetZendeskJwtLink` site in
        // `notifications/client/notificationCenter.py`, which passes a
        // **kwarg**. Read the client, not only the TSV.
        //
        // Line numbers below are the client's own: the census `call_site`
        // column where there is a row, and otherwise `co_lnotab` decoded
        // with `tools/pyc27.py`. **Not** uncompyle6's output lines, which
        // are its own file's and do not match a traceback.
        //
        // The shape of the whole thing is set by one decision, taken from
        // the two feature switches rather than from any one method: **there
        // is no help desk here, and no browser is to be opened.** All three
        // Zendesk switches answer false, which deletes the Help Center
        // branch from `help.py` and `petitionWindow.py` -- and that branch
        // is `webbrowser.open_new(petitioner.GetZendeskJwtLink())`, i.e. the
        // *player's* browser pointed at a host we would have to name. The
        // client is jailed; its browser is not a loophole.
        "petitioner" => match req.method {
            // The petition window's category tree. **`UNPACK_SEQUENCE 4`**
            // (`petitionWindow.py:394`, in `PetitionWindow.GetCategories`),
            // so `None` raises before any `is None` test can run -- the
            // third row of `docs/guides/debugging-the-client.md` §4.
            //
            // And a 4-tuple of `None` is *also* wrong, which is why the
            // arity is not the whole answer:
            //
            // ```text
            // parentCategoryDict, childCategoryDict, descriptionDict, \
            //     self.billingCategories = ...GetCategoryHierarchicalInfo()
            // for parentID, catAndLanguage in parentCategoryDict.iteritems():
            // for childGroupID, childGroupDict in childCategoryDict.iteritems():
            // ...
            // if (result['subCategory'][0], catCountry) in self.billingCategories:
            // ...
            // text = self.descriptions[category][0]
            // ```
            //
            // `.iteritems()` on the first two, `in` with a 2-tuple key on
            // the fourth, and subscript on the third -- four **dicts**.
            // Empty invents nothing: there are no ticket categories here.
            //
            // The empties are load-bearing downstream, not merely
            // tolerated. `CategoryScreen` builds the parent combo with one
            // hardcoded option whose value is `None`, so `category` is
            // falsy and `self.descriptions[category][0]` is never reached;
            // had `descriptionDict` been a dict with keys and
            // `parentCategoryDict` empty, that subscript would `KeyError`.
            //
            // Called **twice** per window open and that is the client's
            // own logic, not a loop:
            // `if len(superCategories) < 2 or len(childCategories) < 1:` it
            // retries once with `forcedLanguage=LOCALE_SHORT_ENGLISH`.
            // Bounded at two; no third call exists.
            "GetCategoryHierarchicalInfo" => Ok(PyValue::Tuple(vec![
                PyValue::Dict(vec![]),
                PyValue::Dict(vec![]),
                PyValue::Dict(vec![]),
                PyValue::Dict(vec![]),
            ])),
            // The petition *service*'s own category list, a different
            // method and a different shape from the window's tree above.
            //
            // ```text
            // def GetCategories(self):
            //     if self.categories is None:
            //         self.categories = ...GetCategories()   # petition.py:666
            //     return self.categories
            // def GetCategory(self, id):
            //     for each in self.GetCategories():          # :671
            //         if each.categoryID == id:
            // ```
            //
            // Iterated for `.categoryID` / `.displayName`, so `None` was
            // both a `TypeError` in every caller *and* a cache that
            // cannot fill: `None` stores successfully and is the one value
            // its own `is None` guard rejects -- the
            // `NumOutstandingContracts` variant, re-issued forever.
            //
            // `[]` fixes both halves, and the second half only because the
            // guard here is `is None` and not `if not`: an empty list is
            // falsy but it is not `None`, so it **latches**. `GetCategory`
            // then returns `None` and `GetC_String` prints
            // "Unknown" -- the client's own string for exactly this.
            //
            // The queue list has the identical shape:
            //
            // ```text
            // def GetQueues(self):
            //     if self.queues is None:
            //         if self.GetIsPetitionee():
            //             queues = ...GetQueues()           # petition.py:659
            //             self.queues = queues
            //     return self.queues
            // ...
            // for each in self.GetQueues():                 # :239
            //     tabs.append([each.queueName, ..., each.queueID])
            // ```
            "GetCategories" | "GetQueues" => Ok(PyValue::List(vec![])),
            // The three petition lists and the two message lists. All five
            // are `GET_ITER` on the answer, reading `.petitionID`,
            // `.petitionerID`, `.senderID`, `.characterID`, `.logDateTime`
            // and friends off the rows -- so `None` is a `TypeError`, and
            // `[]` is an empty window with the client's own
            // "You have no Support Tickets" hint.
            //
            // A plain list, not a `CRowset`: the client only iterates and
            // reads attributes, and `LoadLogs` builds its own `CRowset`
            // from the rows it was given rather than using the answer's.
            //
            // **`GetMyPetitionsEx` does not latch, and that is honest
            // rather than fixed.** Its cache is
            //
            // ```text
            // def GetMyPetitions(self):
            //     if not self.mine:
            //         self.mine = ...GetMyPetitionsEx()  # petition.py:265
            //     return self.mine
            // ```
            //
            // -- `if not`, so an empty list fails the guard and the call
            // re-issues. There is no truthy-but-empty answer short of
            // inventing a ticket, so the re-issue stays. It is not a live
            // lock: the only callers are `ShowMyPetitions` (a tab click)
            // and `_ReloadVisible` (an `On*Petition` push we never send),
            // so the cadence is one call per player action, not per frame.
            // Contrast `jumpCloneSvc.GetShipCloneState`, where the same
            // falsy-cache shape *is* a live lock because an attribute
            // display hook re-reads it while the info window paints.
            //
            // `GetLog` and `GetPetitionMessages` carry a second failure
            // mode that `[]` also removes, and it is not a cache: their
            // callers set `self.loadinglogs = 1` / `self.loadingmessages
            // = 1` before the call and clear it only on the *last* line of
            // the function. A `TypeError` on `for pl in None` skips the
            // clear, and the re-entrancy latch is then stuck at 1 for the
            // session -- logs and messages silently never load again, with
            // one traceback hours earlier as the only trace.
            //
            // Split across two arms with one body on purpose, not for
            // style: `tools/scan_calls_served.py` matches a `|` pattern
            // list **per line**, and its regex needs the line to start with
            // a quote -- so a list rustfmt wraps onto continuation lines is
            // invisible to the census and all five rows keep reading
            // `partial` while being answered. Keep each pattern list on one
            // line, or the lane's own progress bar lies.
            "GetMyPetitionsEx" | "GetClaimedPetitions" => Ok(PyValue::List(vec![])),
            "GetPetitionQueue" | "GetPetitionMessages" | "GetLog" => Ok(PyValue::List(vec![])),
            // `len(newMessages)` first, before anything else
            // (`petition.py:128`, `CheckNewMessages`), so this one needs a
            // **sized** container and `None` has no `len()`. `[]` means no
            // unread replies, which is true, and it takes the client past
            // both branches without the "you have N new messages" dialog.
            "GetUnreadMessages" => Ok(PyValue::List(vec![])),
            // The three lists behind the submission form, all `GET_ITER`:
            // `GetCategoryProperties` (`petitionWindow.py:261`) is iterated
            // for property IDs, `PropertyPopulationInfo` (`:272`) goes to
            // `FormatForDropDown`, which does
            // `entryID, entryText = entry` per element, and
            // `GetClientPickerInfo` (`petitionWindow.py:319`, no census row)
            // is `len()`-tested, compared against 25 and then subscripted.
            //
            // `[]` is right for all three and also makes the picker say so
            // out loud: `if len(populationRecords) > 0: ... else:
            // picker.SetText(GetByLabel('.../ReportInfoNotFound'))`.
            //
            // Unreachable while `MayPetition` refuses below. Answered
            // anyway because each is one `TypeError` away and the cost is
            // a line.
            "GetCategoryProperties" | "PropertyPopulationInfo" | "GetClientPickerInfo" => {
                Ok(PyValue::List(vec![]))
            }
            // Read once, immediately, as half of a tuple key:
            //
            // ```text
            // catCountry = ...GetUserCatalogCountry()   # petitionWindow.py:208
            // if (result['subCategory'][0], catCountry) in self.billingCategories:
            // ```
            //
            // `billingCategories` is the empty dict we answer above, so no
            // key can match and the value is never otherwise used. `None`
            // is the honest answer -- there is no billing catalogue and no
            // country -- and it is an arm rather than a gap so that the
            // `UNANSWERED` line stops claiming we have not looked.
            "GetUserCatalogCountry" => Ok(PyValue::None),
            // **The refusal, and the one answer here that took a
            // decision.** `petitionWindow.py:232`, in `InputPetition`:
            //
            // ```text
            // can = ...MayPetition(categoryID, OocCharacterID)
            // if can < 0:
            //     if can == -1: prompt_player_fsd_dialog('CannotPostPetition', ...); return
            //     if can == -2: ...  # -3, -4, -6, -7 likewise
            //     return
            // ... build the subject/body form and the Submit button ...
            // ```
            //
            // We do not serve ticket *submission* -- `CreatePetition` is a
            // mutator whose result is discarded, so its effect could only
            // show up as a push, which is round 2. That makes letting the
            // form build the wrong answer: the player would type a ticket,
            // press Submit, and the client would say
            // "Support Ticket has been created successfully." on the
            // strength of a discarded return. A lie, and one we chose.
            //
            // So: refuse. `-5` is deliberate and it is the only negative
            // the client has **no dialog for** -- it falls through every
            // `==` to the bare `return`. The six it does handle all say
            // something untrue in this sandbox; checked against the real
            // strings rather than guessed, `-1` is "You can only have one
            // active Support Ticket in each category at a time.", `-3` is
            // "No character selected", `-6` "The amount of open Support
            // Tickets for this user has already been reached.". We have no
            // tickets, a character is selected, and there is no quota, so
            // every one of those is a fabricated reason. Silence beats a
            // false explanation.
            //
            // `None` would reach the same `return` -- Python 2 orders
            // `None` below every number, so `None < 0` is *true* -- but by
            // accident, through the §4 `compare` trap, with the wrong type
            // and no record of intent. A number that means "refused, no
            // reason the client can name" is the same behaviour said on
            // purpose.
            //
            // The cost is a Select button that does nothing. Named in
            // `docs/lanes/chat-petitions.md`; fixing it properly needs a
            // reason code the client does not have.
            "MayPetition" => Ok(PyValue::Int(-5)),
            // The four feature switches, all consumed as bare truth, all
            // false -- and three of the four are false because the true
            // branch dials out.
            //
            // `IsZendeskSwapEnabled` / `IsZendeskEnabled` /
            // `HasOpenTickets` gate the Help Center, whose every button
            // ends in `webbrowser.open_new(GetZendeskJwtLink())`. False
            // deletes those buttons, which is why `GetZendeskJwtLink` is
            // **deliberately not answered** below.
            //
            // `IsSerenity` is false because we are not the Chinese
            // cluster. Note the compound in `petition.py:231`:
            // `if petitioner.IsSerenity() or not
            // petitioner.IsZendeskSwapEnabled():` -- two falses make that
            // *true*, so the petition window keeps its "Create New"
            // button. Accepted: a button that opens a window with an empty
            // category list is a smaller lie than a browser aimed at a
            // host we invented. (The census records the guard here as
            // `if_not_x` on `IsSerenity()`, which is the short-circuit of
            // the `or` and not a cache.)
            "IsZendeskSwapEnabled" | "IsZendeskEnabled" | "HasOpenTickets" | "IsSerenity" => {
                Ok(PyValue::Bool(false))
            }
            // Everything else on this service is left unanswered on
            // purpose, and the purpose is that `UNANSWERED` is our only
            // alarm on the reachability argument above.
            //
            // `GetZendeskJwtLink` -- three call sites, one consumer:
            // `webbrowser.open_new(...)`. There is no answer that does not
            // open the player's browser; even `''` opens it on its home
            // page. The three Zendesk switches above remove all three
            // sites, so it should never arrive. If it does, we want to
            // know, not to have pre-answered it.
            //
            // The mutators -- `CreatePetition`, `DeletePetition`,
            // `CancelPetition`, `ClosePetition`, `ClaimPetition`,
            // `UnClaimPetition`, `EscalatePetition`, `MarkAsRead`,
            // `PetitionerChat`, `PetitioneeChat`, `UpdatePetitionRating`
            // and `AddPetitionRating` (`petition.py:509`, also missing from
            // the census) -- discard their results, so answering them buys
            // nothing without the push path. Every one of their call sites
            // needs a petition row selected in a scroll we answer empty,
            // or the submission form `MayPetition` refuses, so all of them
            // are unreachable. The `UNANSWERED` line is the test of that
            // claim.
            //
            // Role-gated rather than shape-gated, and so unreachable for a
            // second reason: `GetQueues`, `GetClaimedPetitions`,
            // `GetPetitionQueue` and `GetLog` all sit behind
            // `GetIsPetitionee()`, which is
            // `session.role & ROLE_PETITIONEE` (256). `roles::SANDBOX_DOCKED`
            // does not carry that bit. They are answered above anyway
            // because granting it is a one-constant change and `GetQueues`
            // would be a live lock the moment it happened.
            _ => Err(reg.no_method(req)),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

// ---- People search ---------------------------------------------------------
//
// Who a "find a pilot / corp" box can find, and how a name matches. Two
// services ask: `lookupSvc` above (the legacy windows) and the unified
// `search` service (`service/world.rs`), which every newer box goes through
// -- the address book, People & Places, the contract and mail recipient
// pickers, give-ISK, the fleet and bounty windows, the autocomplete on a
// recipient field. `docs/lanes/lookup.md` has the census.
//
// **Only owners `config.GetMultiOwnersEx` can name are ever returned.** Every
// consumer primes `cfg.eveowners` with the ids and then reads `.ownerName` /
// `.name` off them (`searchUtil.py:318` sorts by it; `searchOld.py:20`,
// `corp_ui_applications.py:1150`), and `Recordset.Get` raises on a miss. So
// the people here are exactly the three kinds `Registry::multi_owners`
// knows: this world's characters, the player corporations, and the NPC
// corporation characters start in.

/// `search_const.ResultType` -- the keys of `search.Query`'s dict and the
/// members of both methods' type list **[V]** (`eve/common/script/search/
/// const.py:19`-`:31`). Only the owner kinds this world has are named:
/// `agent = 1` is not, because no agent here has a name `cfg.eveowners`
/// can prime.
pub(crate) const RESULT_CHARACTER: i64 = 2;
pub(crate) const RESULT_CORPORATION: i64 = 3;

/// `search_const.max_result_count` **[V]** (`const.py:63`): at this many the
/// results window says "over the maximum", so the server's cut is here too.
const MAX_RESULT_COUNT: usize = 500;

/// `search_const.min_wildcard_length` **[V]** (`const.py:64`).
const MIN_WILDCARD_LENGTH: usize = 3;

/// `typeCharacter` / `typeCorporation` and `groupCharacter` /
/// `groupCorporation` **[V]** (`inventorycommon/const.py:1488`, `:1535`,
/// `:517`, `:552`) -- the `typeID` / `groupID` an owner row carries.
const TYPE_CHARACTER: i64 = 1373;
const TYPE_CORPORATION: i64 = 2;
const GROUP_CHARACTER: i64 = 1;
const GROUP_CORPORATION: i64 = 2;

/// `search_const.MatchBy`, the `exact=` keyword of both `search` methods
/// **[V]** (`const.py:4`-`:15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchBy {
    PartialTerms,
    ExactTerms,
    ExactPhrase,
    ExactPhraseOnly,
}

impl MatchBy {
    /// The wire value. Absent is the client's own default, `partial_terms`
    /// (every `searchUtil` entry point defaults `searchBy` to it, `:67`,
    /// `:302`, `:323`) **[V]**. Anything else is `None`: `IsMatch` raises
    /// `RuntimeError('Unknown value passed in for exact flag')` on it, and
    /// "nothing matched" is the server-side equivalent.
    fn from_wire(v: Option<&PyValue>) -> Option<Self> {
        match v {
            None | Some(PyValue::None) => Some(MatchBy::PartialTerms),
            Some(PyValue::Int(0)) | Some(PyValue::Bool(false)) => Some(MatchBy::PartialTerms),
            Some(PyValue::Int(1)) | Some(PyValue::Bool(true)) => Some(MatchBy::ExactTerms),
            Some(PyValue::Int(2)) => Some(MatchBy::ExactPhrase),
            Some(PyValue::Int(3)) => Some(MatchBy::ExactPhraseOnly),
            _ => None,
        }
    }
}

/// **`searchUtil.IsMatch`, ported line for line** (`searchUtil.py:401`-`:443`)
/// **[V]**. It is the client's own statement of what each `MatchBy` means,
/// used where the client filters locally; that the *server* matched the
/// same way is `[A]`, and the obvious one to take -- the alternative is two
/// definitions of "matches" inside one client.
///
///  - both sides lowercased and stripped (`:411`, `:412`);
///  - `partial_terms` with no term of at least `min_wildcard_length`
///    becomes `exact_terms` (`:406`-`:409`). The client skips that for a
///    "concise language" (CJK) search string; we have no `languageID`, so
///    it always applies **[A]**;
///  - `partial_terms`: every search term is a prefix of some word of the
///    name (`:414`-`:426`);
///  - `exact_terms`: every search term is a whole word of the name (`:428`);
///  - `exact_phrase`: the terms appear as consecutive words (`:431`-`:439`);
///  - `exact_phrase_only`: the whole string equals the name (`:441`).
fn people_match(search: &str, candidate: &str, by: MatchBy) -> bool {
    let search = search.trim().to_lowercase();
    let by = if by == MatchBy::PartialTerms
        && !search
            .split_whitespace()
            .any(|t| t.chars().count() >= MIN_WILDCARD_LENGTH)
    {
        MatchBy::ExactTerms
    } else {
        by
    };
    terms_match(&search, &candidate.trim().to_lowercase(), by)
}

/// `IsMatch` after its two preambles: both strings already lowercased.
fn terms_match(search: &str, candidate: &str, by: MatchBy) -> bool {
    let wanted: Vec<&str> = search.split_whitespace().collect();
    let words: Vec<&str> = candidate.split_whitespace().collect();
    match by {
        MatchBy::PartialTerms => wanted
            .iter()
            .all(|t| words.iter().any(|w| w.starts_with(t))),
        MatchBy::ExactTerms => wanted.iter().all(|t| words.contains(t)),
        MatchBy::ExactPhrase => words.windows(wanted.len().max(1)).any(|w| w == wanted),
        MatchBy::ExactPhraseOnly => search == candidate,
    }
}

/// The two kinds of owner a person-search can find here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeopleKind {
    Character,
    Corporation,
}

/// One findable owner.
#[derive(Debug, Clone)]
struct Findable {
    id: i64,
    name: String,
    kind: PeopleKind,
    /// The NPC corp. `hideNPC` and the "PC owner" lookups drop it.
    npc: bool,
    /// A corporation's ticker -- its "alt name" (`onlyAltName`, and the
    /// legacy `exact=-1`).
    ticker: Option<String>,
}

impl Findable {
    fn result_type(&self) -> i64 {
        match self.kind {
            PeopleKind::Character => RESULT_CHARACTER,
            PeopleKind::Corporation => RESULT_CORPORATION,
        }
    }
}

/// Everyone a search can find, sorted by name so a list reads the same way
/// twice. `[A]` on the order -- `search.Query`'s window re-sorts by name
/// itself (`searchUtil.py:318`), and the other consumers draw what they get.
fn findables(reg: &Registry) -> Vec<Findable> {
    let mut out: Vec<Findable> = reg
        .config()
        .characters
        .iter()
        .map(|c| Findable {
            id: c.character_id,
            name: c.name.clone(),
            kind: PeopleKind::Character,
            npc: false,
            ticker: None,
        })
        .collect();
    out.extend(reg.corporations.all().map(|c| Findable {
        id: c.corporation_id,
        name: c.name.clone(),
        kind: PeopleKind::Corporation,
        npc: false,
        ticker: Some(c.ticker.clone()),
    }));
    // The NPC corp, by the very rule `multi_owners` names it by: a corp id
    // some character is in that is not a player corp. One name for all of
    // them, as there.
    let mut npc_corps: Vec<i64> = reg
        .config()
        .characters
        .iter()
        .map(|c| c.corporation_id)
        .filter(|id| !corp::is_player_corp(reg, *id))
        .collect();
    npc_corps.sort_unstable();
    npc_corps.dedup();
    out.extend(npc_corps.into_iter().map(|id| Findable {
        id,
        name: reg.config().corporation_name.clone(),
        kind: PeopleKind::Corporation,
        npc: true,
        ticker: Some(corp::CORP_TICKER.to_owned()),
    }));
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    out
}

/// The search string, as both services send it: the first positional
/// argument, `str` or `unicode`.
fn search_string(req: &Request<'_>) -> String {
    req.args
        .first()
        .and_then(crate::pyutil::as_bytes)
        .map(|b| String::from_utf8_lossy(b).trim().to_owned())
        .unwrap_or_default()
}

/// The `ResultType` list, the second positional argument of both `search`
/// methods.
fn result_types(req: &Request<'_>) -> Vec<i64> {
    match req.args.get(1) {
        Some(PyValue::List(v)) | Some(PyValue::Tuple(v)) => v
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(i) => Some(*i),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A keyword flag: `1`/`True` set, anything else (absent, `0`, `None`) not.
fn flag(req: &Request<'_>, name: &[u8]) -> bool {
    matches!(
        crate::pyutil::pairs_get(req.kwargs, name),
        Some(PyValue::Bool(true)) | Some(PyValue::Int(1..))
    )
}

/// Who matched, by `ResultType`, in the order the types were asked for.
/// Empty types are left out.
///
/// `hide_npc` and `only_alt_name` are `search.QuickQuery`'s keywords
/// (`searchUtil.py:327`) **[V]** as names; what they do is `[A]` from the
/// names and their callers: `searchOld.py:37`-`:38` sets `hideNPC` for a
/// corporation search filtered to player corps, and `:35`-`:36` sets
/// `onlyAltName` for `exact == -1`, the ticker search (`SearchOwners`
/// refuses more than five characters on that path, `:288`, which is a
/// ticker's length).
fn people_search(
    reg: &Registry,
    search: &str,
    types: &[i64],
    by: MatchBy,
    hide_npc: bool,
    only_alt_name: bool,
) -> Vec<(i64, Vec<i64>)> {
    if search.is_empty() {
        return Vec::new();
    }
    let everyone = findables(reg);
    let mut out: Vec<(i64, Vec<i64>)> = Vec::new();
    for &t in types {
        if out.iter().any(|(seen, _)| *seen == t) {
            continue;
        }
        let ids: Vec<i64> = everyone
            .iter()
            .filter(|f| f.result_type() == t && !(hide_npc && f.npc))
            .filter(|f| {
                if only_alt_name {
                    f.ticker
                        .as_deref()
                        .is_some_and(|tk| people_match(search, tk, by))
                } else {
                    people_match(search, &f.name, by)
                }
            })
            .map(|f| f.id)
            .take(MAX_RESULT_COUNT)
            .collect();
        if !ids.is_empty() {
            out.push((t, ids));
        }
    }
    out
}

/// `search.QuickQuery(str, types, hideNPC=, onlyAltName=, exact=)`
/// (`searchUtil.py:327`) **[V]**: a **flat list of ids**, returned to the
/// caller unchanged by `GetResultsList`. Its callers iterate it, `enumerate`
/// it and take `result[0]` when there is one (`searchOld.Search`).
pub(crate) fn search_quick_query(reg: &Registry, req: &Request<'_>) -> PyValue {
    let Some(by) = MatchBy::from_wire(crate::pyutil::pairs_get(req.kwargs, b"exact")) else {
        return PyValue::List(vec![]);
    };
    let found = people_search(
        reg,
        &search_string(req),
        &result_types(req),
        by,
        flag(req, b"hideNPC"),
        flag(req, b"onlyAltName"),
    );
    PyValue::List(
        found
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .map(PyValue::Int)
            .collect(),
    )
}

/// `search.Query(str, types, exact=)` (`searchUtil.py:306`) **[V]**: a
/// **dict keyed by `ResultType`**, each value a list of ids. The values must
/// be real lists, because `_SortResults` calls `.sort(key=...)` on them in
/// place (`:316`-`:320`); `_PrimeOwners` then `.get(t, [])`s each owner type
/// (`:363`) and `war_cso.py:382` does the same for corporations, so a type
/// with nobody in it can simply be absent.
pub(crate) fn search_query(reg: &Registry, req: &Request<'_>) -> PyValue {
    let Some(by) = MatchBy::from_wire(crate::pyutil::pairs_get(req.kwargs, b"exact")) else {
        return PyValue::Dict(vec![]);
    };
    let found = people_search(
        reg,
        &search_string(req),
        &result_types(req),
        by,
        false,
        false,
    );
    PyValue::Dict(
        found
            .into_iter()
            .map(|(t, ids)| {
                (
                    PyValue::List(ids.into_iter().map(PyValue::Int).collect()),
                    PyValue::Int(t),
                )
            })
            .collect(),
    )
}

/// The legacy `lookupSvc` `exact` argument (`(str, exact)` at every call
/// site). Three values reach us: `0` from every window, `1` from the slash
/// command (`svc_slash.py:172`), and `-1` from `SearchOwners`' ticker path
/// (`searchOld.py:288`) **[V]**. What they mean is `[A]`:
///
///  - `0`: a prefix match on the words of the name -- `partial_terms`
///    **without** the three-character floor, because these windows only
///    refuse an empty string (`corp_ui_applications.py:1139`) and a
///    two-letter name would otherwise be unfindable;
///  - `1`: the whole name (`exact_phrase_only`);
///  - `-1`: a corporation's ticker, whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyExact {
    Prefix,
    Name,
    Ticker,
}

impl LegacyExact {
    fn from_wire(v: Option<&PyValue>) -> Self {
        match v {
            Some(PyValue::Int(-1)) => LegacyExact::Ticker,
            Some(PyValue::Bool(true)) => LegacyExact::Name,
            Some(PyValue::Int(n)) if *n > 0 => LegacyExact::Name,
            _ => LegacyExact::Prefix,
        }
    }

    fn matches(self, search: &str, f: &Findable) -> bool {
        let search = search.trim().to_lowercase();
        match self {
            LegacyExact::Prefix => {
                terms_match(&search, &f.name.to_lowercase(), MatchBy::PartialTerms)
            }
            LegacyExact::Name => f.name.trim().to_lowercase() == search,
            LegacyExact::Ticker => f
                .ticker
                .as_deref()
                .is_some_and(|t| t.to_lowercase() == search),
        }
    }
}

/// One `lookupSvc` answer: everyone of `kinds` whose name matches, as rows.
fn lookup_rows(
    reg: &Registry,
    req: &Request<'_>,
    kinds: &[PeopleKind],
    players_only: bool,
    row: impl Fn(&Findable) -> PyValue,
) -> PyValue {
    let search = search_string(req);
    if search.is_empty() {
        return PyValue::List(vec![]);
    }
    let exact = LegacyExact::from_wire(req.args.get(1));
    PyValue::List(
        findables(reg)
            .iter()
            .filter(|f| kinds.contains(&f.kind) && !(players_only && f.npc))
            .filter(|f| exact.matches(&search, f))
            .take(MAX_RESULT_COUNT)
            .map(row)
            .collect(),
    )
}

/// The owner row: the four attributes `searchOld.SearchOwners` reads
/// (`searchOld.py:313`-`:315`) **[V]**, which cover the chat picker's three
/// (`pcownerpickerdialog.py:101`-`:114`) and give-ISK's one.
fn owner_row(f: &Findable) -> PyValue {
    let (type_id, group_id) = match f.kind {
        PeopleKind::Character => (TYPE_CHARACTER, GROUP_CHARACTER),
        PeopleKind::Corporation => (TYPE_CORPORATION, GROUP_CORPORATION),
    };
    keyval(vec![
        ("ownerID", PyValue::Int(f.id)),
        ("ownerName", PyValue::Unicode(f.name.clone())),
        ("typeID", PyValue::Int(type_id)),
        ("groupID", PyValue::Int(group_id)),
    ])
}

/// One row per character with a live session: `(contactID, online)`.
///
/// Empty until Round 0.3 -- "there is nobody else here" was true and is not
/// any more. Everyone online rather than only the caller's contacts: the
/// client indexes this by `charID` and reads `.online` on a cache miss, and
/// a contact list we do not have yet cannot narrow it.
fn online_status_rowset(reg: &Registry) -> PyValue {
    let columns = [("contactID", DbType::I4), ("online", DbType::Bool)]
        .into_iter()
        .map(|(name, db_type)| Column {
            name: name.as_bytes().to_vec(),
            db_type,
        })
        .collect();
    let descriptor =
        DbRowDescriptor::new(columns).expect("the online-status columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);
    for id in reg.sessions().online_characters() {
        rowset
            .push_values(vec![DbValue::Int(id), DbValue::Bool(true)])
            .expect("the online-status row matches the descriptor declared above");
    }
    rowset.to_pyvalue()
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

    /// **Every lookup answers a list, because several of them are iterated.**
    ///
    /// The search box threw rather than finding nothing, which is the
    /// difference between "no results" and a traceback. Asserted across all
    /// eleven rather than spot-checked, because they share one arm and the
    /// failure mode of splitting that arm later is that one name quietly
    /// falls through to `NoSuchMethod` -- a refused call, which is a hung
    /// client.
    #[test]
    fn every_lookup_answers_an_empty_list_rather_than_none() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "LookupCharacters",
            "LookupCorporations",
            "LookupAlliances",
            "LookupFactions",
            "LookupEvePlayerCharacters",
            "LookupNoneNPCAccountOwners",
            "LookupOwners",
            "LookupPCOwners",
            "LookupStations",
            "LookupKnownLocationsByGroup",
            "LookupLocationsByGroup",
        ] {
            assert_eq!(
                call(&mut reg, "lookupSvc", method).unwrap_or_else(|e| panic!("{method}: {e}")),
                PyValue::List(vec![]),
                "{method} is iterated or compared; None raises"
            );
        }
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

    /// `GetCategoryHierarchicalInfo` is consumed by `UNPACK_SEQUENCE 4`
    /// and then by `.iteritems()` on two of the four, so the arity and the
    /// element type are both forced and neither is a free choice.
    ///
    /// A 4-tuple of `None` -- the §4 third-row answer that is right for
    /// `ship.GetShipFittingInfo` -- is **wrong here**, which is why this
    /// asserts the elements and not just the length.
    #[test]
    fn petition_categories_unpack_into_four_dicts() {
        let mut reg = Registry::new(SandboxConfig::default());
        let answer = call(&mut reg, "petitioner", "GetCategoryHierarchicalInfo").unwrap();

        let PyValue::Tuple(parts) = &answer else {
            panic!(
                "petitionWindow.py:394 does `a, b, c, d = GetCategoryHierarchicalInfo()`, \
                 got {answer:?}"
            )
        };
        assert_eq!(parts.len(), 4, "UNPACK_SEQUENCE 4");
        for (i, part) in parts.iter().enumerate() {
            assert!(
                matches!(part, PyValue::Dict(_)),
                "element {i} must be a dict: the first two are read with \
                 `.iteritems()`, the third is subscripted and the fourth is \
                 tested with `(a, b) in it`; got {part:?}"
            );
        }
    }

    /// The two `is None` caches on this service must latch, and "not
    /// `None`" is the property that decides it -- not the container type.
    ///
    /// `petition.py` holds both:
    ///
    /// ```text
    /// if self.categories is None:
    ///     self.categories = sm.RemoteSvc('petitioner').GetCategories()
    /// ...
    /// if self.queues is None:
    ///     if self.GetIsPetitionee():
    ///         queues = sm.RemoteSvc('petitioner').GetQueues()
    ///         self.queues = queues
    /// ```
    ///
    /// `None` stores successfully and is the one value its own guard
    /// rejects, so every read re-issues the call -- the
    /// `NumOutstandingContracts` variant, which throws nothing and shows up
    /// in no instrument we own. An empty list is falsy but it is not
    /// `None`, so it latches; that is the whole reason these are `[]` and
    /// the reason a test that only checked `matches!(.., List(_))` would
    /// pass for the broken answer.
    #[test]
    fn petition_is_none_caches_latch() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in ["GetCategories", "GetQueues"] {
            let answer = call(&mut reg, "petitioner", method).unwrap();
            assert_ne!(
                answer,
                PyValue::None,
                "{method} is cached behind `is None` and the store completes, \
                 so None poisons the cache for the session"
            );
            assert!(
                matches!(answer, PyValue::List(_)),
                "{method} is iterated for `.categoryID` / `.queueName`, so it \
                 must be a sequence; got {answer:?}"
            );
        }
    }

    /// The petition and message lists are all `GET_ITER`, so the one thing
    /// that must hold is that they are sequences rather than `None`.
    ///
    /// **These are deliberately falsy and this test says so**, which is the
    /// opposite of the usual rule for an `if not x:` guard. `GetMyPetitions`
    /// is `if not self.mine: self.mine = GetMyPetitionsEx()`, so an empty
    /// answer never latches and the call re-issues -- and there is no
    /// truthy-but-empty answer that does not invent a support ticket. The
    /// re-issue is bounded by player action (a tab click, or an
    /// `On*Petition` push we do not send), not by frame rate, so it is
    /// wasteful rather than a live lock. If a future capture ever shows
    /// `GetMyPetitionsEx` in the hundreds, the cause is a new caller, not
    /// this answer.
    #[test]
    fn petition_lists_are_iterable_and_honestly_empty() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "GetMyPetitionsEx",
            "GetClaimedPetitions",
            "GetPetitionQueue",
            "GetPetitionMessages",
            "GetLog",
            "GetUnreadMessages",
            "GetCategoryProperties",
            "PropertyPopulationInfo",
            "GetClientPickerInfo",
        ] {
            let answer = call(&mut reg, "petitioner", method).unwrap();
            let PyValue::List(rows) = &answer else {
                panic!(
                    "{method} is iterated (and `GetUnreadMessages` / \
                     `GetClientPickerInfo` have `len()` taken first), so None \
                     is a TypeError; got {answer:?}"
                )
            };
            assert!(
                rows.is_empty(),
                "{method} must invent no rows: we serve no support desk"
            );
        }
    }

    /// `MayPetition` refuses, and it refuses with the one negative code the
    /// client has no message for.
    ///
    /// Three properties, each of which would be a real regression:
    /// it is an **integer** (`consumer = compare`, and §4's trap is that
    /// Python 2 orders `None` below every number, so a `None` here changes
    /// a branch with no error at all); it is **negative**, so the
    /// submission form the client would otherwise build -- and then claim
    /// success for, from a discarded `CreatePetition` -- is never reached;
    /// and it is **not one of the six codes with a dialog**, every one of
    /// which states a quota or a character problem that is untrue here.
    #[test]
    fn may_petition_refuses_without_fabricating_a_reason() {
        let mut reg = Registry::new(SandboxConfig::default());
        let answer = call(&mut reg, "petitioner", "MayPetition").unwrap();

        let PyValue::Int(code) = answer else {
            panic!(
                "petitionWindow.py:232 does `if can < 0:`; Python 2 compares \
                 None below every number, so a non-integer here picks a branch \
                 silently. Got {answer:?}"
            )
        };
        assert!(
            code < 0,
            "a non-negative code builds the submission form, and submitting it \
             makes the client print 'Support Ticket has been created \
             successfully.' for a CreatePetition whose result is discarded"
        );
        for spoken in [-1, -2, -3, -4, -6, -7] {
            assert_ne!(
                code, spoken,
                "InputPetition has a CannotPostPetition dialog for {spoken}, and \
                 all six state a quota or a character error that is false here"
            );
        }
    }

    /// The feature switches are false, and false is what keeps the player's
    /// browser shut.
    ///
    /// Every Help Center button in `help.py` and `petitionWindow.py` ends
    /// in `webbrowser.open_new(petitioner.GetZendeskJwtLink())`. Truthy
    /// here draws those buttons; the jail does not contain the player's
    /// browser, so this is the one place on this service where a wrong
    /// answer reaches outside the machine.
    #[test]
    fn the_zendesk_switches_are_off() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "IsZendeskSwapEnabled",
            "IsZendeskEnabled",
            "HasOpenTickets",
            "IsSerenity",
        ] {
            assert_eq!(
                call(&mut reg, "petitioner", method).unwrap(),
                PyValue::Bool(false),
                "{method} gates a Help Center button whose action opens the \
                 player's browser at a host we would have to name"
            );
        }
    }

    /// `GetZendeskJwtLink` and the mutators stay unanswered, and that is a
    /// decision rather than a backlog.
    ///
    /// `GetZendeskJwtLink` has exactly one consumer at all three of its
    /// call sites -- `webbrowser.open_new(...)` -- and no answer avoids
    /// opening a browser, `''` included. The mutators discard their
    /// results, so they cannot be finished without the push path, and every
    /// one of their call sites needs either a petition row from a scroll we
    /// answer empty or the form `MayPetition` refuses. Leaving them
    /// unanswered keeps `[svc] UNANSWERED` as the alarm on that
    /// reachability argument; pre-answering them would delete the only
    /// instrument that could tell us it was wrong.
    #[test]
    fn the_browser_link_and_the_mutators_are_left_unanswered() {
        let mut reg = Registry::new(SandboxConfig::default());
        for method in [
            "GetZendeskJwtLink",
            "CreatePetition",
            "DeletePetition",
            "CancelPetition",
            "ClosePetition",
            "ClaimPetition",
            "UnClaimPetition",
            "EscalatePetition",
            "MarkAsRead",
            "PetitionerChat",
            "PetitioneeChat",
            "UpdatePetitionRating",
            "AddPetitionRating",
        ] {
            match call(&mut reg, "petitioner", method) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{method} must stay unanswered until round 2's push path: \
                     answering it ({other:?}) either opens the player's browser \
                     or reports a mutation that did not happen"
                ),
            }
        }
    }

    // ---- People search ----------------------------------------------------

    /// Two pilots, the NPC corp they start in, and one player corp.
    fn people() -> Registry {
        let pilot = |id: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: id + 10_000_000,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![
                pilot(90_000_001, "Alice Ashfall"),
                pilot(90_000_002, "Bob Brightwater"),
            ],
            ..SandboxConfig::default()
        });
        reg.corporations
            .parse_record(&[
                "corp",
                "98000001",
                "90000002",
                "90000002",
                "0",
                "0.1",
                "0.1",
                "1",
                "0",
                "-1",
                "-1",
                "-1",
                "-1",
                "-1",
                "-1",
                "Bright Miners",
                "BRITE",
                "",
                "",
            ])
            .expect("a well-formed corp record");
        reg
    }

    fn ask(
        reg: &mut Registry,
        service: &str,
        method: &str,
        args: &[PyValue],
        kwargs: &[(PyValue, PyValue)],
    ) -> PyValue {
        dispatch_any(
            reg,
            &Request {
                service,
                method,
                args,
                kwargs,
                bind_params: None,
                actor: Some(90_000_001),
            },
        )
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    }

    /// `lookupSvc` is here; `search` routes through `world.rs` into this
    /// module, so the test goes in by the same door the client does.
    fn dispatch_any(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
        if req.service == "search" {
            crate::service::world::dispatch(reg, req)
        } else {
            dispatch(reg, req)
        }
    }

    fn u(s: &str) -> PyValue {
        PyValue::Unicode(s.into())
    }

    fn attr(v: &PyValue, name: &str) -> PyValue {
        let PyValue::Instance { state, .. } = v else {
            panic!("a KeyVal, got {v:?}")
        };
        crate::pyutil::dict_get(state, name.as_bytes())
            .unwrap_or_else(|| panic!("{name} is read by the client and is absent"))
            .clone()
    }

    fn rows(v: PyValue) -> Vec<PyValue> {
        match v {
            PyValue::List(rows) => rows,
            other => panic!("iterated and len()ed by every caller; got {other:?}"),
        }
    }

    /// The corp invite window: `LookupEvePlayerCharacters(str, 0)` and then
    /// `each.characterID` (`corp_ui_applications.py:1143`, `:1148`) -- a
    /// list of rows, each carrying the id.
    #[test]
    fn the_corp_invite_search_answers_rows_carrying_character_id() {
        let mut reg = people();
        let got = rows(ask(
            &mut reg,
            "lookupSvc",
            "LookupEvePlayerCharacters",
            &[u("Bob Brightwater"), PyValue::Int(0)],
            &[],
        ));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(attr(&got[0], "characterID"), PyValue::Int(90_000_002));
    }

    /// `searchOld.SearchOwners` reads `groupID`, `ownerName`, `ownerID` and
    /// `typeID` off every row (`searchOld.py:313`-`:315`); the chat picker
    /// reads the last three. The type and group are the client's own
    /// constants (`inventorycommon/const.py`). `LookupPCOwners` is the
    /// `hideNPC` branch of the same function (`searchOld.py:306`-`:309`), so
    /// it does not offer the NPC corp that `LookupOwners` does.
    #[test]
    fn the_owner_lookups_answer_owner_rows_and_only_pc_ones_hide_the_npc_corp() {
        let mut reg = people();
        let npc_name = reg.config().corporation_name.clone();
        let mut one = |method: &str, needle: &str| -> Vec<PyValue> {
            rows(ask(
                &mut reg,
                "lookupSvc",
                method,
                &[u(needle), PyValue::Int(0)],
                &[],
            ))
        };

        let pilot = one("LookupOwners", "Bob Brightwater");
        assert_eq!(pilot.len(), 1, "{pilot:?}");
        assert_eq!(attr(&pilot[0], "ownerID"), PyValue::Int(90_000_002));
        assert_eq!(attr(&pilot[0], "ownerName"), u("Bob Brightwater"));
        assert_eq!(attr(&pilot[0], "typeID"), PyValue::Int(1373));
        assert_eq!(attr(&pilot[0], "groupID"), PyValue::Int(1));

        let corp = one("LookupPCOwners", "Bright Miners");
        assert_eq!(corp.len(), 1, "{corp:?}");
        assert_eq!(attr(&corp[0], "ownerID"), PyValue::Int(98_000_001));
        assert_eq!(attr(&corp[0], "ownerName"), u("Bright Miners"));
        assert_eq!(attr(&corp[0], "typeID"), PyValue::Int(2));
        assert_eq!(attr(&corp[0], "groupID"), PyValue::Int(2));

        let npc = one("LookupOwners", &npc_name);
        assert_eq!(npc.len(), 1, "{npc:?}");
        assert_eq!(
            attr(&npc[0], "ownerID"),
            PyValue::Int(crate::model::NPC_CORPORATION_ID)
        );
        assert_eq!(one("LookupPCOwners", &npc_name), vec![]);
    }

    /// `search.Query` answers a dict keyed by `ResultType` whose values are
    /// lists (`_SortResults` sorts them in place, `searchUtil.py:316`).
    #[test]
    fn the_unified_search_answers_ids_keyed_by_result_type() {
        let mut reg = people();
        let got = ask(
            &mut reg,
            "search",
            "Query",
            &[
                u("bri"),
                PyValue::List(vec![PyValue::Int(2), PyValue::Int(3)]),
            ],
            &[(PyValue::Int(0), u("exact"))],
        );
        assert_eq!(
            got,
            PyValue::Dict(vec![
                (
                    PyValue::List(vec![PyValue::Int(90_000_002)]),
                    PyValue::Int(2)
                ),
                (
                    PyValue::List(vec![PyValue::Int(98_000_001)]),
                    PyValue::Int(3)
                ),
            ])
        );
        // Only the types asked for.
        let corps_only = ask(
            &mut reg,
            "search",
            "Query",
            &[u("bri"), PyValue::List(vec![PyValue::Int(3)])],
            &[],
        );
        assert_eq!(
            corps_only,
            PyValue::Dict(vec![(
                PyValue::List(vec![PyValue::Int(98_000_001)]),
                PyValue::Int(3)
            )])
        );
    }

    /// `search.QuickQuery` is a flat list of ids (`GetResultsList` returns
    /// it unchanged and its callers iterate it and take `result[0]`).
    #[test]
    fn the_quick_search_is_a_flat_list_of_ids() {
        let mut reg = people();
        let got = ask(
            &mut reg,
            "search",
            "QuickQuery",
            &[
                u("Bright"),
                PyValue::List(vec![PyValue::Int(2), PyValue::Int(3)]),
            ],
            &[(PyValue::Int(0), u("exact"))],
        );
        assert_eq!(
            got,
            PyValue::List(vec![PyValue::Int(90_000_002), PyValue::Int(98_000_001)])
        );
    }

    /// `searchUtil.IsMatch`, case by case (`searchUtil.py:401`-`:443`).
    #[test]
    fn the_match_rules_are_the_clients_is_match() {
        let name = "Bob Brightwater";
        // partial_terms: every term a prefix of some word.
        assert!(people_match("bob bri", name, MatchBy::PartialTerms));
        assert!(!people_match("bob wat", name, MatchBy::PartialTerms));
        // ... and with no term of three letters it is exact_terms.
        assert!(!people_match("bo", name, MatchBy::PartialTerms));
        assert!(people_match("bo", "Bo Diddley", MatchBy::PartialTerms));
        // exact_terms: whole words, any order.
        assert!(people_match("brightwater bob", name, MatchBy::ExactTerms));
        assert!(!people_match("bob bright", name, MatchBy::ExactTerms));
        // exact_phrase: consecutive whole words.
        assert!(people_match("b c", "a b c d", MatchBy::ExactPhrase));
        assert!(!people_match("c b", "a b c d", MatchBy::ExactPhrase));
        // exact_phrase_only: the whole name.
        assert!(people_match(
            " BOB brightwater ",
            name,
            MatchBy::ExactPhraseOnly
        ));
        assert!(!people_match("bob", name, MatchBy::ExactPhraseOnly));
    }
}
