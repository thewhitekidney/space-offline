<!-- Provenance: the `agents-missions` domain lane of `docs/archive/plan-the-churn.md`
     Wave 1, 2026-10-20. Read-only: nothing outside this file,
     `findings/shapes-agents-missions.tsv` and `docs/lanes/agents-missions.md`
     was written, and no cargo was run. Every client line number below came
     from `co_lnotab` via `tools/call_lines.py` / `tools/pyc27.py`, never from
     uncompyle6's output -- the offsets in this subsystem are large (the
     `agentMissionBriefing*` block decompiles at line 165 and really lives at
     214; the `agentMission*` event strings decompile at 1602 and really live
     at 1909), so a decompiler citation here would have been wrong by 49 and
     307 lines respectively. Marks are the house convention: **[V]** read from
     this build's bytecode or from this tree's code, **[I]** inferred,
     **[unknown]** not established. -->

# Agents and missions, from the client's side

Client build V24.01 (3619879). Companion files:
`findings/shapes-agents-missions.tsv` (one row per method, on the
`docs/archive/plan-the-churn.md` §4 schema), `docs/lanes/agents-missions.md` (the park
note). Read alongside `docs/re/localization.md` (the prose corpus),
`docs/re/static-corpus.md` §6.2 (`missions.fsdbinary`),
`docs/reference/bound-objects.md` §1 (the bind mechanism, which this subsystem lives
on), `docs/reference/push-path.md` §2 (the push packet) and
`docs/guides/debugging-the-client.md` §4 (the empty-answer taxonomy, which §5 below
applies rather than restates).

Citations are `<client path>.py:LINE`, the form the client's own tracebacks
use. The tree on disk is `out/pyc/<same path>.pyc`.

**The one-line summary.** The content is real and readable, the call surface
is small -- about twenty methods, most of them on one bound object -- and
**the entire offer-and-accept conversation is a single method, `DoAction`,
called on `Moniker('agentMgr', agentID)`.** Everything else on the docked path
is decoration around that one call.

---

## 1. The minimum honest vertical slice

The target: a player right-clicks an agent in the station, reads CCP's own
briefing text, presses Accept, and the mission appears in the journal — with
everything past that point failing cleanly rather than hanging.

Eight steps. Each names the calls it closes. Steps 1–6 are the slice; 7 and 8
are the two things that make "after that" clean instead of a hang.

### Step 1 — put a real agent in the station

`agentMgr.GetAgents` is already answered (`service.rs::agent_rowset`), but
every row has `stationID = None`, so `allAgents.Filter('stationID')` has no
entry for our station and the lobby's Agents tab is empty by construction
(`eve/client/script/ui/shared/dockedUI/agents.py:230`). **[V]**

Add one row with:

| column | value | why |
| --- | --- | --- |
| `agentID` | a character-range id we own | `cfg.eveowners.Get(agentID).name` must resolve, so it must be in the owners prime |
| `agentTypeID` | `2` (`agentTypeBasicAgent`, `appConst.py:105`) | keeps it out of `AGENT_TYPES_WITH_NEW_UI`, so the window class is the old HTML `AgentDialogueWindow`, which is the one documented here |
| `level` | `1` | **this is the whole standing story** — see below |
| `divisionID` | one of 22/23/24 (`appConst.py`) | `get_division_name(divisionID)` is rendered in the header |
| `stationID` | our station | the lobby filter key |
| `solarsystemID` | our system, all lowercase | `Filter('solarsystemID')`; note the spelling differs from the station rowset's `solarSystemID` |
| `corporationID`, `factionID` | an NPC corp and faction | read by `GetAgentArgs` and by the standing header |

**Level 1 is load-bearing and it removes standings from the slice entirely.**
`standingsvc.CanUseAgent` (`eve/client/script/ui/services/standingsvc.py:94`)
delegates to `eveCfg.CanUseAgent`, whose second statement is

```python
if level == 1 and agentTypeID != const.agentTypeResearchAgent and noL1Check:
    return 1
```

(`eve/common/script/sys/eveCfg.py:161`, `noL1Check` defaults to `1`). **[V]**
So a level-1 non-research agent is usable with every standing at `0.0`, and
`standingMgr.GetCharStandings` can stay empty for this slice.

Closes: `agentMgr.GetAgents` (already answered; this is a data change, not a
new arm).

### Step 2 — survive the lobby's Agents tab

Loading that tab (`dockedUI/agents.py:_load`) issues, in order: **[V]**

1. `journal.GetMyAgentJournalDetails()[:1][0]` → `agentMgr.GetMyJournalDetails`
   (`dockedUI/agents.py:107`) — already answered as `[[], []]`.
2. `agents.GetAgentsByStationID()[session.stationid]` (`:230`) — step 1. Note
   this cannot `KeyError`: `FilterRowset.__getitem__` is
   `self.items.get(i, [])` and returns an **empty `Rowset`** for a station it
   has never heard of (`eve/common/script/sys/rowset.py`, `FilterRowset`).
3. `standing.CanUseAgent(...)` (`:329`) — client-side, settled by step 1.
4. `agents.GetAgentTimers(relevantAgents)` (`:144`) → **one bound call
   `GetReplayTimestamp(characterID)` per relevant agent in the station**
   (`station/agents/agents.py:309`). This is the first bound call on the
   agent object and the first one we must answer. A falsy answer means
   "no replay timer", which is what we want.
5. `epicArcStatus.AgentHasEpicMissionsForCharacter(agentID)` (`:295`) — only
   for `agentTypeEpicArcAgent` rows. Our thirteen epic-arc starter rows have
   `agentTypeID = None`, so this is **not** reached by them; it becomes
   reachable the moment anyone gives those rows their real type.

The whole `_load` body is wrapped in `except Exception:
logger.exception(...)`, so a wrong shape here is loud in `--alerts` and costs
the tab, not the client. **[V]**

Closes: `agentMgr<agentID>.GetReplayTimestamp`, and `epicArcStatus.Agent
HasEpicMissionsForCharacter` if any epic-arc row ever gets a real type.

### Step 3 — open the conversation

Double-clicking the entry calls `agents.OpenDialogueWindow(agentID)`
(`eve/client/script/ui/shared/userentry.py:1124`), which constructs the window
and calls `wnd.InteractWithAgent()`
(`station/agents/agentDialogueWindow.py:233`). **[V]** That is where the
conversation starts, and it issues exactly three bound calls per render:

```
DoAction(None)            agentDialogueWindow.py:516
GetMissionObjectiveInfo() agentDialogueWindow.py:261
GetAgentLocationWrap()    agentDialogueWindow.py:342
```

plus `GetMissionBriefingInfo()` via the agent service
(`agentDialogueWindow.py:282` → `station/agents/agents.py:824`).

`GetAgentLocationWrap` is the trap of this step. Its result goes straight into
`LocationWrapper`, which begins `if locationType is None and 'locationType' in
location` (`station/agents/agentDialogueUtil.py:273`) — so **`None` is a
`TypeError` on the opening path**. It must be a dict with at least
`solarsystemID` (`:275`), `typeID` and `locationID` (`:305`). For a station
agent the honest answer is the agent's own station. **[V]**

Closes: `agentMgr<agentID>.DoAction`, `GetMissionObjectiveInfo`,
`GetAgentLocationWrap`, `GetMissionBriefingInfo`.

### Step 4 — answer `DoAction(None)` with an offer

The contract, read from two independent consumers: **[V]**

```python
(agentSaysMsg, availableActions), lastActionInfo = DoAction(actionID)
```

(`agentDialogueWindow.py:485`, and `missioncontroller.py:318` for the new
MissionGiver window, which additionally does `_, mission_id = agent_says` —
an **unconditional unpack of `agentSaysMsg` as a 2-tuple**.)

For "I have a mission for you", answer:

```
(( (<offerMessageID>, <contentID>),
   [ (1, 2) ] ),                      # one action: id 1, button type 2 = RequestMission
 { 'missionCompleted': 0, 'missionDeclined': 0,
   'missionQuit': 0, 'missionCantReplay': 0, 'loyaltyPoints': 0 })
```

Two things make those four `lastActionInfo` keys mandatory rather than
optional: `GetObjectiveHTML` subscripts all four with no `.get`
(`agentDialogueWindow.py:262-263`), and `_build_mission_state` iterates
`ACTION_TO_STATE` over them (`missioncontroller.py:521`). `loyaltyPoints` is
read with `.get(..., 0)` (`agentDialogueWindow.py:341`), so it is optional.
**[V]**

The window then does something worth knowing before you design the dialogue:
**when `actionID is None` and the first available action is
`agentDialogueButtonRequestMission` (2) or `agentDialogueButtonViewMission`
(1), it immediately calls `DoAction(firstActionID)` itself**
(`agentDialogueWindow.py:493`). So opening the window costs **two**
`DoAction`s and the player sees the mission offer without clicking anything.
The server must therefore treat "the client asked for action 2" as an ordinary
request, not as a click.

Button type ids, measured: `ViewMission=1` (`appConst.py:193`),
`RequestMission=2` (`:194`), `Accept=3` (`:195`), `AcceptChoice=4`,
`AcceptRemotely=5`, `Complete=6` (`:198`), `CompleteRemotely=7`,
`Continue=8`, `Decline=9` (`:201`), `Defer=10` (`:202`), `Quit=11` (`:203`),
`StartResearch=12`, `CancelResearch=13`, `BuyDatacores=14`,
`LocateCharacter=15`, `LocateAccept=16`, `LocateReject=17`, `Yes=18`,
`No=19`. **[V]** The `actionID` in each `(actionID, actionData)` pair is
**ours to choose** — the client only echoes it back. `actionData` decides how
it is drawn: `int` → a button via `LABEL_BY_BUTTON_TYPE`
(`agentDialogueWindow.py:415`), `str`/`unicode` → an admin HTML link, `dict` →
a "special interaction" sub-briefing.

### Step 5 — make the briefing CCP's own prose

The offer response's `agentSaysMsg` is `(messageID, contentID)`. That reaches
`agents.ProcessMessage` (`station/agents/agents.py:698`), which primes
arguments and calls `localization.GetByMessageID(messageID, **msgArgs)`.
`msgArgs` is the union of **our** `GetMissionKeywords(contentID)` answer and
the client's own `GetAgentArgs(agentID)`. §2 is the whole contract; the slice
needs eight keys at most and usually one.

Closes: `agentMgr<agentID>.GetMissionKeywords`.

### Step 6 — accept, and make the journal show it

The Accept button sends `DoAction(<the actionID we paired with button type
3>)`. The answer that completes the slice is:

1. a `CallRsp` whose `agentSaysMsg` is the acceptance line and whose
   `availableActions` no longer offers Accept;
2. a **push**: `OnAgentMissionChange('accepted', agentID)`, arity 2,
   `broadcastID = 'OnAgentMissionChange'`, args `('accepted', agentID)` —
   `appConst.agentMissionAccepted = 'accepted'` (`appConst.py:1909`). **[V]**
   The packet shape is `docs/reference/push-path.md` §2: command 12, broadcast address
   `(4, b'OnAgentMissionChange', [clientID], b'*clientID')`, payload
   `(0, pickle((0, (1, args))))`;
3. a `GetMyJournalDetails` whose `[0]` now holds one 10-tuple with
   `missionState = 2` (`agentMissionStateAccepted`, `appConst.py:1928`).

**That push is the difference between "works" and "works after a relog."**
`JournalSvc.OnAgentMissionChange` (`journal.py:247`) either clears
`self.agentjournal` (when `agentID is None`) or appends the agent to
`outdatedAgentJournals`, which is the *only* thing that invalidates the
journal cache short of `OnSessionReset`. **[V]**

### Step 7 — the clean failure for everything past "accepted"

An accepted mission with an objective brings `missionTrackerMgr` into play,
and the missions info panel appears (`InfoPanelAgentMissions.IsAvailable` is
`bool(missionObjectivesTracker.GetAgentMissions())`). The three calls to
answer, in order of consequence:

- `missionTrackerMgr.GetAllMissionObjectives(agentID)` — **`enumerate()`d
  directly** (`infoPanelAgentMissions.py`, `_InitializeObjectives`), so
  `None` is a `TypeError`. `[]` is correct and safe; see §5 for why it does
  not latch and why that is acceptable.
- `missionTrackerMgr.GetMissionInfoItems(agentID)` — a list of strings, or
  falsy. Falsy is stored as `None` and means "no current objective"
  (`missionObjectivesTracker.py:278`).
- `missionTrackerMgr.UpdateAllMissions(agentList)` — discarded, inside an
  `ExceptionEater` (`:231`). Anything works; `None` is honest.

With those three answered, an accepted mission with no objectives renders as
"Read your journal" (`CreateObjective`'s fallback, `ReadJournal`) and nothing
hangs. **[V]**

### Step 8 — the three calls that are *not* in the slice but are already being asked

These already appear or will appear on the docked path and each has a cheap
honest answer:

- `agentMgr.GetDisabledMissions` → a list. `missionID in <it>`
  (`station/agents/agents.py:364`).
- `agentMgr.GetCompletedCareerAgentIDs(agentIDs)` → `{agentID: False}` for
  every id asked. The caller `update()`s a dict and later **subscripts it
  without `.get`** (`:217`), so omitting an id that was asked for is a
  `KeyError`.
- `epicArcStatus.AgentHasEpicMissionsForCharacter(agentID)` → `False`. Not
  served today; it is an `UNANSWERED` as soon as an epic-arc agent row gets a
  real `agentTypeID`.

### What the slice does *not* need

No Destiny, no ballpark, no dungeon, no bookmark, no standings, no
`dungeonExplorationMgr`, and no `keeper`. §7 says why.

---

## 2. `GetMissionKeywords`, and the `ProcessMessage` contract

This is the join, so it gets stated twice: once as a type and once as a
mechanism.

### 2.1 The type

`Moniker('agentMgr', agentID).GetMissionKeywords(contentID)` → **a dict,
string keys to integer ids.** **[V]**
(`station/agents/agents.py:691`.)

```python
def PrimeMessageArguments(self, agentID, contentID, missionKeywords=None):
    if contentID is not None:
        if agentID is None:
            raise RuntimeError(...)
        if (agentID, contentID) not in self.missionArgs:          # :689
            if not missionKeywords:
                missionKeywords = self.GetAgentMoniker(agentID) \
                                      .GetMissionKeywords(contentID)   # :691
            self.missionArgs[(agentID, contentID)] = missionKeywords
```

Three consequences, in order of how much they cost to get wrong:

1. **`None` is a `TypeError` with no handler.** The stored value is later fed
   to `msgArgs.update(self.missionArgs[(agentID, contentID)])`
   (`agents.py:716`), which is *outside* the only `try` in `ProcessMessage`
   (that `try` wraps `localization.GetByLabel` alone). The exception escapes
   `ProcessMessage` → `GetMessageText` → `GetBriefingHTML` →
   `InteractWithAgent`, whose `finally` only hides the loading wheel. The
   player gets an empty agent window. **Answer `{}`, never `None`.** **[V]**
2. **`{}` latches correctly.** The guard is key-presence, not truthiness, so
   an empty dict is stored and never re-asked. This is the opposite of the
   `jumpCloneSvc` case in `docs/guides/debugging-the-client.md` §4 and it is the reason
   this call is safe to stub.
3. **The cache is evicted by a push, not by time.**
   `Agents.OnAgentMissionChange` deletes every `missionArgs` key for that
   agent when the action is `agentMissionOffered` or `agentMissionReset`
   (`agents.py:755` onwards, `appConst.py:1915`/`:1923`). So if the server
   changes the keywords for an existing `contentID`, it must push one of those
   two or the client keeps rendering the old substitutions.

The server can also skip the call entirely: `PrimeMessageArguments` takes
`missionKeywords` as a third argument, and `GetMissionBriefingInfo` uses that
path — the client stores `briefingInformation['Mission Keywords']` directly
into `missionArgs` keyed by `['ContentID']` (`agents.py:824`). **So a briefing
that ships its own keyword dict never provokes a `GetMissionKeywords` call at
all.** **[V]**

### 2.2 Which keys

**The client never names them.** A binary-safe grep for `dungeonLocationID`
and `objectiveDestinationID` across all 12,774 `.pyc` files returns **zero**
hits. **[V]** The key set is defined *per message* by the localisation
corpus's own token records, and `tools/loctable.py id <N>` is the oracle:

```
messageID 145165
  tokens  {'{[location]dungeonLocationID.name}': {... 'variableType': 3,
            'propertyName': 'name', 'variableName': 'dungeonLocationID'},
           '{[character]player.name}':           {... 'variableType': 0, ...}}
```

`tools/loctable.py tokens` ranks every token form in the corpus. Filtered to
the mission vocabulary it is **sixteen forms over eleven variable names, and
that is the whole list**: **[V]**

| variable | type | occurrences | who supplies it |
| --- | --- | --- | --- |
| `dungeonLocationID` | location | 1,208 | **us** |
| `agentID` | character | 615 | client (`GetAgentArgs`) |
| `agentCorpID` | npcorganization | 437 | client |
| `objectiveDestinationID` | location | 368 | **us** |
| `agentFactionID` | npcorganization | 259 + 124 | client |
| `objectiveTypeID` | item | 208 + 179 | **us** |
| `agentSolarSystemID` | location | 131 | client |
| `agentConstellationID` | location | 119 | client |
| `agentStationID` | location | 87 | client |
| `objectiveLocationID` | location | 85 | **us** |
| `dungeonSolarSystemID` | location | 80 | **us** |
| `rewardTypeID` | item | 63 | **us** |
| `objectiveDestinationSystemID` | location | 63 | **us** |
| `agentRegionID` | location | 58 | client |

Plus `objectiveQuantity`, which appears only as the `quantity=` kwarg of
`{[item]objectiveTypeID.quantityName, quantity=objectiveQuantity}` (179
occurrences) and must therefore be in the same dict. **[V]**

So **`GetMissionKeywords` owns exactly eight keys**: `dungeonLocationID`,
`dungeonSolarSystemID`, `objectiveDestinationID`,
`objectiveDestinationSystemID`, `objectiveLocationID`, `objectiveTypeID`,
`objectiveQuantity`, `rewardTypeID`. Everything else in a mission string is
either the agent (client-side, from the `GetAgents` row) or the player.

`player` needs no entry: `localization.Get` defaults
`kwargs['player'] = session.charid` when the caller omits it
(`docs/re/localization.md` §2, step 2). **[V]**

**The values are integer ids, not strings.** `{[location]X.name}` resolves
through `cfg.evelocations.Get(locationID).locationName`
(`localization/propertyHandlers/locationPropertyHandler.py`,
`_GetName`); `{[character]X.name}` through `cfg.eveowners.Get(charID)
.ownerName`; `{[item]X.name}` through `evetypes`. **A location the client
cannot name renders the literal `[no location: 12345]` and logs a warning —
it does not raise.** That is the single most useful degradation fact in this
document and §7 leans on it. **[V]**

Note the merge order: `msgArgs.update(missionArgs)` then
`msgArgs.update(agentArgs)` (`agents.py:716-718`), so **the agent keys win**
and we cannot override `agentStationID` from the server. **[V]**

### 2.3 `ProcessMessage`, and the recursion

`agents.ProcessMessage(msg, agentID)` (`agents.py:698`) accepts three forms
for `msgInfo` inside `msg = (msgInfo, contentID)`: **[V]**

| `msgInfo` | behaviour |
| --- | --- |
| a `basestring` | returned verbatim, before any priming — the escape hatch for server-authored text |
| an `int` | `localization.GetByMessageID(msgInfo, **msgArgs)`; on *any* exception, `CorruptBriefing` plus the raw untokenised text appended after a rule |
| a `(label, args)` 2-tuple | each key in `args` is merged into `msgArgs` and the whole thing goes to `localization.GetByLabel(label, **msgArgs)` |

And **the recursion, which is the part that is easy to miss**: in the tuple
form, before the merge, every `args` entry whose key is one of

```
missionBriefingText  missionOfferText  missionCompletionText
declineMessageText   locationString
```

**or whose value is itself a tuple** is replaced by
`self.ProcessMessage((value, contentID), agentID)` (`agents.py:724`). **[V]**
So a label-form message can carry a *nested* message id in any of those five
slots and the client will resolve it, with the same keyword dict, to a
rendered string before substituting it into the outer label. That is how CCP
composes "here is a mission wrapper around a briefing" without the server
rendering text.

Two notes for anyone building on it:

- `localization.GetByLabel` on an unknown label returns the literal
  `[no label: <path>]`, and an unknown message id renders
  `[no messageID: <N>]` (`docs/re/localization.md` §2). Both are visible and
  neither raises. A *missing kwarg*, by contrast, falls back to
  `GetRawByMessageID` and the player sees the raw `{tokens}`.
- The `except:` around the int form is bare and logs with
  `log.LogException`, so it reaches us through `--alerts`. A wrong keyword
  dict is therefore loud. **[V]**

---

## 3. The agent interaction, end to end

Everything below is on the docked path and needs nothing in space.

### 3.1 The bound object

`eveMoniker.GetAgent(agentID, stationID=None)` returns
`Moniker('agentMgr', agentID)` (`eve/common/script/net/eveMoniker.py`,
`GetAgent`). The `stationID` argument is **not** part of the bind: it is used
only to pre-seed the machoNet address cache so the resolve lands on the
station's node (`macho.CheckAddressCache(SERVICE_STATION, stationID)` →
`SetNodeOfAddress(SERVICE_AGENTMGR, agentID, nodeID)`). On a single-node
server it is inert. **[V]**

`agents.GetAgentMoniker(agentID)` (`station/agents/agents.py:312`) memoises
the moniker in `self.agentMonikers` and clears the whole dict after 30
minutes of no access, from a 5-minute worker (`_ClearAgentMonikersThread`).
**So the client will re-bind the agent object roughly every half hour of
idleness, and we must tolerate a second bind for the same `agentID`.** **[V]**

`bindParams` is the bare `agentID` — `docs/reference/bound-objects.md` §2.2 already has
that row. On our side `handle_bind` mints the object under the service name
`agentMgr` and `handle_bound_call` passes `bind_params` through on the
`Request`, so the `agentMgr` arm sees both the service-level and the
bound-level methods and can tell them apart by `req.bind_params.is_some()`.

### 3.2 The sequence, from right-click to a mission in the journal

| # | what the player did | call | file:line |
| --- | --- | --- | --- |
| 1 | opened the station lobby's Agents tab | `agentMgr.GetMyJournalDetails` | `dockedUI/agents.py:107` |
| 2 | — | `agentMgr.GetAgents` (once per session) | `station/agents/agents.py:110` |
| 3 | — | `agentMgr<id>.GetReplayTimestamp` × agents in station | `station/agents/agents.py:309` |
| 4 | double-clicked the agent | *(local)* `agents.OpenDialogueWindow` | `userentry.py:1124` |
| 5 | — | `agentMgr<id>.DoAction(None)` | `agentDialogueWindow.py:516` |
| 6 | — | `agentMgr<id>.DoAction(firstActionID)` — **automatic** | `agentDialogueWindow.py:493` |
| 7 | — | `agentMgr<id>.GetMissionObjectiveInfo()` | `agentDialogueWindow.py:261` |
| 8 | — | `agentMgr<id>.GetMissionBriefingInfo()` | `station/agents/agents.py:824` |
| 9 | — | `agentMgr<id>.GetMissionKeywords(contentID)` | `station/agents/agents.py:691` |
| 10 | — | `agentMgr<id>.GetAgentLocationWrap()` | `agentDialogueWindow.py:342` |
| 11 | pressed Accept | `agentMgr<id>.DoAction(acceptActionID)` | `agentDialogueWindow.py:516` |
| 12 | — | **push** `OnAgentMissionChange('accepted', agentID)` | `journal.py:247` |
| 13 | — | `agentMgr<id>.GetMyJournalDetails()` (partial refresh) | `journal.py:425` |

Steps 7–10 run on *every* `InteractWithAgent`, which means on every button
press, and also on `OnSessionChanged` with `'stationid'` in the change
(`agentDialogueWindow.py:116`) and on `OnAgentMissionChange` when the action
is `agentMissionModified` (`:129`). So the per-render cost of an open agent
window is four bound calls, and `agentMissionModified` is the push that makes
the window redraw itself.

### 3.3 The direction nobody expected: the server calls the client

`Agents.__exportedcalls__` (`station/agents/agents.py:86`) publishes
**twenty** methods the *server* may call on the client's `agents` service.
**[V]** Ten of them are dialogue and mission primitives:

```
MessageBox  YesNo  SingleChoiceBox  GetQuantity  PopupSelect  RemoteNamePopup
CheckCargoCapacity  PopupMission  ShowMissionObjectives
PopupDungeonShipRestrictionList
```

All of `MessageBox`, `YesNo`, `SingleChoiceBox`, `GetQuantity`, `PopupSelect`
and `RemoteNamePopup` accept `(agentID, contentID)` and run their title and
body through `ProcessMessage` **unless the argument is already a
`basestring`** (`agents.py:419`, `:433`, `:462`, `:499`, `:514`, `:529`).
**[V]** So the real agent conversation on TQ is not a request/response
protocol at all: the server interrupts the player with CCP's prose and reads
the answer back.

`CheckCargoCapacity(mandatoryCargoUnits, extraFlagsToCheck)` is the one with
a return value we would act on: `None` when there is room, else the pair
`('mission.offeredinsufficientcargospace', rejectMessage)`
(`agents.py:387`). **[V]** That is how TQ refuses a courier mission the
player cannot carry, and it means **the cargo check is the client's, not
ours** — a fact that removes a whole category of server-side inventory
arithmetic from courier missions.

None of this is needed for §1's slice. It is recorded because it is the
cheapest route to a *conversational* agent later, and because
`eve-server` has no client-call path at all today
(`unknown` whether `conn.rs` can issue a `CallReq` to the client; the
`push.rs` design in `docs/reference/push-path.md` is notification-only).

---

## 4. The mission state machine

### 4.1 The two state vocabularies, and they are not the same

**States** — what a mission *is*, carried in `GetMyJournalDetails`'s
`missionState` and in `GetMissionObjectiveInfo['missionState']`: **[V]**

| value | name | line |
| --- | --- | --- |
| 0 | `agentMissionStateAllocated` | `appConst.py:1926` |
| 1 | `agentMissionStateOffered` | `:1927` |
| 2 | `agentMissionStateAccepted` | `:1928` |
| 3 | `agentMissionStateFailed` | `:1929` |
| 4 | `agentMissionStateCompleted` | `:1931` |
| 5 | `agentMissionStateDeclined` | `:1932` |
| 6 | `agentMissionStateQuit` | `:1933` |
| 7 | `agentMissionStateCantReplay` | `:1934` |

**Events** — what *happened*, carried as the first argument of the
`OnAgentMissionChange` push. These are **strings**, not ints: **[V]**

| string | const | line |
| --- | --- | --- |
| `accepted` | `agentMissionAccepted` | `appConst.py:1909` |
| `completed` | `agentMissionCompleted` | `:1910` |
| `declined` | `agentMissionDeclined` | `:1911` |
| `failed` | `agentMissionFailed` | `:1913` |
| `modified` | `agentMissionModified` | `:1914` |
| `offer_removed` | `agentMissionOfferRemoved` | `:1918` |
| `quit` | `agentMissionQuit` | `:1920` |
| `reset` | `agentMissionReset` | `:1923` |

and, from the same block but not individually measured here,
`dungeon_moved`, `offered`, `offer_declined`, `offer_expired`, `prolong`,
`research_started`, `research_update_ppd`, `talk_to_completed`. The literal
strings are **[V]** (verified present in `appConst.pyc` by binary grep); the
exact line of each of those eight is `unknown`.

`ON_GOING_MISSION = {agentMissionStateAllocated, agentMissionStateOffered}`
(`dockedUI/agents.py`) — so an *offer* is already a journal row with state 0
or 1, not a thing that lives only in the dialogue window. **[V]**

### 4.2 Which calls change state and which only read it

| call | effect |
| --- | --- |
| `DoAction(actionID)` | **the only mutator.** Offer, accept, decline, defer, quit, complete, start/cancel research, buy datacores, locate a character — all of them are one `actionID` through this one method |
| `RemoveOfferFromJournal()` | mutator; drops an offer row without declining it (`agents.py:855`) |
| `UpdateAllMissions(agentList)` | mutator-ish; asks the server to re-evaluate objectives (`missionObjectivesTracker.py:231`) |
| everything else | read-only |

There is no `AcceptMission`, no `DeclineMission` and no `CompleteMission`
anywhere in the client tree. **[V]**

### 4.3 Decline, and what the client tells the player about it

`GetMissionBriefingInfo['Decline Time']` drives the warning text
(`agentDialogueWindow.py:287` → `GetMissionTimeText`): **[V]**

- `-1` → `UI/Agents/StandardMission/DeclineMessageGeneric`, message 235491:
  *"Declining a mission from a particular agent more than once every 4 hours
  may result in a loss of standing with that agent..."*
- a positive number of seconds → `DeclineMessageTimeLeft`, message 235496:
  *"Declining a mission from this agent within the next {timeRemaining} will
  result in a loss of standing with this agent."*
- `None` → fall through to `'Expiration Time'`, rendered as
  `UI/Agents/Dialogue/ThisMissionExpiresAt`.

So **the 4-hour decline window and the standing penalty are server policy**;
the client only renders the countdown we hand it. Nothing on the client
computes a standing loss.

### 4.4 The replay timer

Two independent surfaces read it:

- `agentMgr<id>.GetReplayTimestamp(characterID)` → truthy means "this agent
  is on cooldown", which moves the agent from *Available to you* to *Not
  available to you* in the lobby (`dockedUI/agents.py:144`). **[V]**
- `lastActionInfo['missionCantReplay']` → **seconds remaining**, rendered as
  `UI/Agents/DefaultMessages/MissionCantReplay`, message 631990: *"Visit us
  again in {timeLeft} and we can talk more about your next opportunity."*
  (`agentDialogueWindow.py:287`). When it is truthy the window **suppresses
  every action button** (`ReconstructButtons`, `:415`) and
  `GetObjectiveHTML` returns nothing. **[V]**

`ShouldAlwaysAllowReplay()` is the override and is read nowhere but
`agents.py:595`.

### 4.5 Objectives, and their completion tests

`GetMissionObjectiveInfo` is the full objective document. Mandatory keys,
every one subscripted without a guard in `GetMissionObjectiveHTML`: **[V]**

| key | shape | line |
| --- | --- | --- |
| `completionStatus` | int; `> 0` = finished, `== 2` = GM cheat-complete | `agentDialogueUtil.py:409` |
| `missionTitleID` | message id, **or a string** | `:410`, `:411` |
| `missionState` | one of §4.1's eight | `:416` |
| `objectives` | list of `(objType, objData)` | `:437`, `:459` |
| `dungeons` | list of dicts | `:440` |
| `locations` | list of **solar system ids** | `:461` |
| `agentGift` | list of `(typeID, quantity, extra)` | `:475` |
| `normalRewards` | list of `(typeID, quantity, extra)` | `:499` |
| `loyaltyPoints` | a **number** (`> 0`) | `:501` |
| `researchPoints` | a **number** (`> 0`) | `:502` |
| `bonusRewards` | list of 5-tuples `(timeRemaining, typeID, quantity, extra, timeBonusIntervalMin)` | `:546` |
| `collateral` | list of `(typeID, quantity, extra)` | `:647` |

Optional: `importantStandings` (read with `.get`, `:406`), `missionExtra` —
a 2-tuple `(headerID, bodyID)` of message ids run through `ProcessMessage`
(`:667`) — and `contentID`, needed only when `missionExtra` is present
(`:672`).

`objectives` has exactly **three** `objType` values, and the unpack arity of
`objData` differs per type (`agentDialogueUtil.py:_ProcessObjectiveEntry`,
from `:30`): **[V]**

```
'agent'      (agentID, agentLocation)
'transport'  (pickupOwnerID, pickupLocation, dropoffOwnerID, dropoffLocation, cargo)
'fetch'      (dropoffOwnerID, dropoffLocation, cargo)
```

`cargo` is a dict with `hasCargo`, `quantity`, `typeID`, `volume`
(`:59`, `:70`, `:79`). `pickupLocation`/`dropoffLocation` are **location
dicts** in the `LocationWrapper` shape (§7.1). An unrecognised `objType`
renders as the empty string — the function just falls through — so an
**unknown objective type is silently invisible, not an error.** **[V]**

Each entry of `dungeons` is a dict: `optional` and `objectiveCompleted` and
`location` and `dungeonID` are mandatory (`:190`, `:200`, `:228`, `:239`);
`completionStatus`, `briefingMessage`, `ownerID` and `shipRestrictions` are
tested with `in` first (`:184`, `:197`, `:205`, `:230`). **[V]**

**The one hard dependency on real CCP data in this whole subsystem:**
`_ProcessDungeonData` ends with `dungeonData = GetDungeon(dunData['dungeonID'])`
and then reads `dungeonData.connections` (`agentDialogueUtil.py:248`). That is
a *client-side* FSD read of `dungeons.fsdbinary`. A `dungeonID` that is not a
real dungeon raises inside the objective-pane build. **So `dungeons` entries
must carry a genuine `dungeonID` from the 5,516 the client ships** — which is
exactly what `missions.fsdbinary`'s `dungeonID` column gives us
(`docs/re/static-corpus.md` §6.2). **[V]**

The **completion tests themselves are not on the client**, with one exception.
`_ProcessObjectiveEntry` computes pickup/dropoff tick marks locally from
`location['locationID'] == session.locationid` and `cargo['hasCargo']`
(`:58-70`) — i.e. the *display* of courier progress is client-side. Everything
else (`objectiveCompleted`, `completionStatus`) arrives from us.

The live objective tracker's grammar is separate and is a flat list of
strings. `CreateObjective` (`missions/client/ui/missionObjectiveData.py`)
dispatches on `str(missionInfo[0])` against a table of **28 tags**: **[V]**

```
TravelTo  MissionFetch  MissionFetchContainer  MissionFetchMine
MissionFetchMineTrigger  MissionFetchTarget  AllObjectivesComplete
TransportItemsPresent  TransportItemsMissing  MissionTransport
FetchObjectAcquiredDungeonDone  GoToGate  KillTrigger  KillAllTrigger
Destroy  DestroyLCSAndAll  DestroyAll  "Destroy all the enemies"  Attack
Approach  Hack  Salvage  TalkToAgent  "Remove item from container"
dunTriggerEffectActivated  DropItemInMissionContainer  DropOffItemsMissing
```

(plus any value of `NAME_BY_TRIGGER_TYPE`, which routes to a generic dungeon
trigger objective). `missionInfo[1]` and `[2]` are `int()`ed or `long()`ed
per tag — e.g. `TravelTo` takes a `locationID`, `MissionFetch` a `typeID` and
a `quantity`, `Attack` a `typeID` and an `itemID`. **The whole dispatch is
inside `try: ... except Exception: logger.exception(...)` and falls back to
`ReadJournal`**, so a wrong objective row is loud and harmless. **[V]**

### 4.6 Bonus expiry

`bonusRewards` entries carry their own `timeRemaining`; when it is `<= 0` the
client swaps the header for `UI/Agents/StandardMission/BonusTimePassed` and
**still lists the reward** (`agentDialogueUtil.py:581`). **[V]** So bonus
expiry is a number we recompute per call, not a state transition, and the
43,200-second column in `missions.fsdbinary`
(`docs/re/static-corpus.md` §6.2) is where the window comes from.

---

## 5. Caches, guards and cadence

`docs/guides/debugging-the-client.md` §4's rule — *severity is set by the caller's
cadence, not by the shape of the guard* — decides this table. Every guarded
cache in the subsystem:

| cache | guard | stores a falsy answer? | what paces the re-ask | verdict |
| --- | --- | --- | --- | --- |
| `agents.allAgents` (`GetAgents`) | `is None` **inside `self._getAllAgentsLock`** | the store is `.Clone()` on the result, so `None` raises *before* the store and the cache stays `None` | session reset, and nothing else; every `GetAgentByID` goes through it | `None` here is the **LPSvc live lock exactly** — a throw before the store behind a `is None` guard. The lock makes it single-flight, not single-shot. **Never answer `None`.** |
| `journal.agentjournal` (`GetMyJournalDetails`) | `is None`, under a `Semaphore` | yes — `[[], []]` is stored and latches | user-paced: journal window, lobby Agents tab, info-panel build; plus one partial refresh per `OnAgentMissionChange` | safe |
| `agents.missionArgs[(agentID, contentID)]` (`GetMissionKeywords`) | **key presence** | yes — `{}` latches | evicted only by an `offered`/`reset` push | safe, and the reason `{}` is the right stub |
| `agents._careerAgentsCompletedByID` | `all(id in cache)` | yes, if every id asked for comes back | user-paced | safe; a *partial* answer is a later `KeyError` |
| `agents._disabledMissions` | `is None`, store-back | yes — `[]` latches | property read per mission render | safe |
| `agents.agentSolarSystems[agentID]` | `not in` | yes | user-paced | safe |
| `agents.agentMonikers[agentID]` | `not in`, **plus a 30-minute TTL sweep** | n/a | the sweep is a 5-minute `SleepWallclock` worker | expect a re-bind per idle half hour |
| `journal.escalationRowset` (`GetMyEscalatingPathDetails`, `journal.py:345`) | `is None or force` | `None` is stored and fails its own guard → **poisoned cache**; a `Rowset` with no lines latches fine | journal Expeditions tab; *and* `OnEscalationsDataUpdated`, which `space.rs`'s comment records having driven at 3 s intervals from our own `ScatterEvent` | already fixed in `space.rs`; do not regress it to `None` |
| `missionObjectivesTracker.missionObjectives[agentID]` (`GetAllMissionObjectives`) | **truthiness** on a `defaultdict(lambda: None)` | **no** — `[]` never latches | one-shot 1000 ms `AutoTimer`, re-armed by `OnAgentMissionChange`, `OnBallparkSetState`, `OnSessionChanged`, `OnClientEvent_WarpFinished` | the **fourth variant**: a correct empty answer that cannot latch. Event-paced, not render-paced, so it is a re-ask and not a spin — **but the timer is the thing to watch** |
| `missionObjectivesTracker.currentAgentMissionInfo` (`GetMissionInfoItems`) | `is None` on the whole dict, then per-agent fill | stored as `missionInfo or None`, so a falsy answer stores `None` per agent and the *dict* still latches | warp finish, location change, objective pushes | safe; the per-agent `None` is the intended "no current objective" |
| `epicArcSvc.controller` (`GetMyEpicArcStatus`) | `if not self.controller` | the controller is never assigned if the call raises → re-fires | Agency refresh | `{}` required; `None` is an `AttributeError` inside `__init__` (already handled in `character.rs`) |

**The one to flag.** `GetAllMissionObjectives` is the only call in this
subsystem whose correct empty answer cannot latch, and its re-ask is driven by
a timer rather than by the player. The timer is a *one-shot* — 
`_UpdateMissionsPanelThread` sets `self.missionInfoPanelUpdater = None` as its
first statement (`infoPanelSvc.py:275`, the `AutoTimer` at `:278`, the clear at `:284`,
`MISSION_INFO_PANEL_UPDATE_DELAY_MSEC = 1000` at `:39`) — so it fires once per
re-arming event, not continuously. **[V]** But `OnBallparkSetState` and
`OnClientEvent_WarpFinished` re-arm it, so once Destiny exists this becomes a
per-warp call per accepted mission. Worth a repeat-count check in the first
capture that has an accepted mission in it.

And the one that does not exist yet but will: the missions info panel is gated
on `bool(GetAgentMissions())`, i.e. on an accepted mission existing. **Until
step 6 of §1 works, none of the `missionTrackerMgr` calls happen at all** —
which is why they are step 7 and not step 3.

---

## 6. The journal and the agent list: families and row shapes

### 6.1 `agentMgr.GetAgents` must be a `Rowset`, and the reason is one method

Already recorded in `service.rs`, restated because it is the single sharpest
family constraint in the subsystem: the caller's first act is
`GetAgents().Clone()` (`station/agents/agents.py:110`), and `CRowset` has no
`Clone`. Only `Rowset`, `IndexRowset` and `FilterRowset` do
(`eve/common/script/sys/rowset.py`, `Rowset.Clone`). **[V]**

It is then `Index`ed on `agentID` and `Filter`ed on five columns, **all of
which must exist in `header` even with no rows**, because
`IndexRowset`/`FilterRowset` resolve the column name through
`self.header.index(colname)` before they look at any line.

Two `FilterRowset` properties that matter and are easy to get wrong the other
way:

- **`FilterRowset.__getitem__` is total.** It is
  `Rowset(self.header, self.items.get(i, []), self.RowClass)`
  (`rowset.py`, `FilterRowset.__getitem__`), so
  `GetAgentsByStationID()[session.stationid]` for a station with no agents
  returns an **empty `Rowset`** — falsy, iterable, no `KeyError`. **[V]** The
  lobby tab therefore degrades correctly without us.
- **`IndexRowset.__getitem__` is not**, which is why `GetAgentByID` guards
  with `if agentID in self.allAgentsByID` (`agents.py:120`) and returns `None`
  otherwise — and why `service.rs`'s thirteen epic-arc starter rows exist.

### 6.2 `agentMgr.GetMyJournalDetails` — a 2-element list of lists

The family is forced from four directions, and no rowset satisfies all of
them: **[V]**

```python
self.agentjournal = sm.RemoteSvc('agentMgr').GetMyJournalDetails()  # journal.py:413
for mission in self.agentjournal[0]: ...                            # :414
self.agentjournal[0].remove(mission)                                # :436
for n in range(2): self.agentjournal[n].extend(parallelResults[i][n])  # :447
for missionData in self.agentjournal[:1][0]: ...                    # :451
```

`[0]`, `[1]`, a slice, `.remove()` and `.extend()`. So: **the outer value is
a sliceable 2-sequence and each element is a `list`.** A `CRowset` would pass
`[0]`/slice/`extend` and fail `remove` with the row semantics the caller
expects; a `Rowset` fails the subscript. `[[], []]` is what `character.rs`
answers today and it is right.

**`[1]` is never read by anything in the client tree**, as far as this lane
could find — it is only `extend`ed in the partial-refresh path. The `[I]`
reading is that it is the offers list; TQ's own naming is `unknown`. It must
exist and be a list, and that is all the client requires.

Each element of `[0]` is a **10-tuple**, unpacked in four places with
different variable names; the names below are the union: **[V]**

```
(missionState, importantMission, missionType, missionNameID,
 agentID, expirationTime, bookmarks, remoteOfferable,
 remoteCompletable, contentID)
```

- `journal.py:475` (`GetMyAgentJournalBookmarks`) and `:494` (`CheckUndock`)
- `dockedUI/agents.py:107` (the lobby tab)
- `missionObjectivesTracker.py` (`GetAgentMissions`, `_UpdateAgentMissions`,
  `RegisterMissionTracking`)
- `journal.py:(IsMissionActiveWithAgent)` unpacks ten positionally with eight
  of them discarded

`missionNameID` may be an int message id **or a string** —
`_GetAgentMissionName` does `isinstance(missionNameID, (int, long))` and
`GetByMessageID` only in the int case (`infoPanelAgentMissions.py`), and
`ActiveMissionController(*missionData)` takes exactly ten positional
arguments. `missionType` is one of the
`appConst.missionType*` **label paths** (`'UI/Agents/MissionTypes/Courier'`
and friends), not an int. **[I]** on `missionType` — the constants are
strings in `appConst` and the journal renders them, but no row has crossed the
wire.

`bookmarks` is a list and **its elements must accept attribute assignment**:

```python
if getattr(b, 'originalHint', None) is None:
    b.originalHint = b.hint          # journal.py:417, STORE_ATTR
b.hint = GetByLabel(b.originalHint, location=b.locationID,
                    index=b.locationNumber + 1)          # :418
```

**That `STORE_ATTR` rules out a packed `DbRow`**, which refuses attribute
assignment (`docs/reference/dbrow.md`). `bookmarkSvc.GetAgentBookmarks` then does
`c = bm.copy()` and assigns four more attributes to the copy
(`bookmarkSvc.py:126-131`), so the element needs a `.copy()` that returns
something equally assignable. `utillib.KeyVal` satisfies both. **[V]** on the
constraint; **[I]** that `KeyVal` is what TQ sends.

### 6.3  `dungeonExplorationMgr.GetMyEscalatingPathDetails` — old-style `Rowset`

`journal.OnEscalatingPathMessage` reads `exp1.header` and `exp1.lines`
directly to build `IndexRowset(exp1.header, exp1.lines, 'instanceID')`
(`journal.py:345` for the call). So this one is the `header`/`lines` family
and **not** a `CRowset`: a `CRowset` has `.columns`, not `.header`. **[V]**
Already answered correctly in `space.rs`.

### 6.4 The rest

`GetCompletedCareerAgentIDs` is a plain `dict` (`.update()` then subscript).
`GetMyEpicArcStatus` is a `dict` of `dict`s. `GetDisabledMissions` only needs
`in`. `GetMissionKeywords`, `GetMissionBriefingInfo`,
`GetMissionObjectiveInfo` and `GetMissionJournalInfo` are all plain `dict`s
with **string** keys — note that the `appConst.agentMissionBriefing*`
constants are themselves strings with spaces in them
(`'Mission Title ID'`, `'Decline Time'`, `appConst.py:214-221`), not integer
indices, which is the first thing a reader of the decompiled source gets
wrong.

Nothing in this subsystem wants a `CIndexedRowset`, a `CFilterRowset` or a
`DbRow`.

---

## 7. The pushes

Cross-referenced against `findings/push-events.tsv`. The push mechanism, the
broadcast address and the `(0, pickle((0, (1, args))))` payload nesting are
`docs/reference/push-path.md` §2 and are not restated.

### 7.1 Required for a mission to be visible without a relog

**`OnAgentMissionChange(missionState, agentID)` — arity 2, and it is the only
one in this list.** **[V]**

Seven handlers are declared for it, which is the most of any event in this
subsystem:

| handler | file:line | what it does |
| --- | --- | --- |
| `JournalSvc` | `journal.py:247` | `agentID is None` → clear `self.agentjournal` entirely; else append to `outdatedAgentJournals`. **This is the only journal-cache invalidation that is not a session reset.** Then blinks the neocom, unless the event is one of `declined`/`completed`/`quit`/`offer_declined`/`dungeon_moved`, and re-scatters as `OnAgentMissionChanged` |
| `Agents` | `station/agents/agents.py:755` | on `offer_removed`/`reset`/`talk_to_completed`, closes and forgets the dialogue window; on `offered`/`reset`, **evicts that agent's `missionArgs`**; plays the accept/complete sounds; on `completed`, drops the career-agent completion cache |
| `AgentDialogueWindow` | `agentDialogueWindow.py:118` | on `modified` only, re-runs `InteractWithAgent()` — i.e. four bound calls |
| `MissionDetailsWindow` | `missionDetailsWindow.py:78` | rebuilds the mission journal window |
| `JournalWindow` | `journal.py:1360` | reloads the visible tab |
| `BrowserResearch` | `industry/browserResearch.py:90` | discards both arguments |
| `OldMissionController` | `evemissions/client/missioncontroller.py:614` | the new MissionGiver's controller |

The first argument is the **event string** from §4.1, not a state int — the
parameter is named `missionState` in `JournalSvc` and `actionID` in `Agents`,
and both compare it against the `agentMission*` string constants. **[V]** The
name in the census is misleading; the type is a string.

`agentID = None` is a legitimate argument and means "refresh everything".

**The `OnAgentMissionChanged` (past tense) event is client-local**: it is
`sm.ScatterEvent`ed by `JournalSvc.OnAgentMissionChange` and by
`MissionObjectivesTracker.OnMissionsUpdated`, with `local_emitters=1` in the
census. We do not send it; sending `OnAgentMissionChange` produces it. **[V]**
Eight handlers hang off it, including the lobby `AgentsPanel` (`dockedUI/
agents.py:215`) and the `MissionObjectivesTracker` (`:174`), so one push
redraws the lobby tab, the Agency, the Job Board and the tracker.

### 7.2 Polish

| event | arity | args | what redraws | needed for |
| --- | --- | --- | --- | --- |
| `OnMissionsUpdated` | 1 | `missions` — a list of dicts with keys `agentID` and `info` | the live objective tracker; also re-scatters `OnAgentMissionChange('modified', agentID)` per changed mission | live objective progress. `info` is the flat string list of §4.5, or falsy. `iterate+elem[]` — **a list is mandatory, `None` is a `TypeError`** |
| `OnAgentMissionObjectivesCompleted` | 1 | `agentID` | re-fetches `GetMissionInfoItems` for that agent | objective completion feedback |
| `OnAgentMissionObjectivesFailed` | 1 | `agentID` | same | failure feedback |
| `OnStandingSet` | 3 | `fromID, toID, standing` | the lobby `AgentsPanel` reloads (`dockedUI/agents.py:221`); `standingsvc` updates its cache | mission reward standings |
| `OnNPCStandingChange` | 3 | `fromID, newStanding, oldStanding` | the standings panel, the Agency's unlocked-agents list | standing gain after turn-in |
| `OnIncomingAgentMessage` | 2 | `agentID, message` | `transmissionMgr.ShowAgentAlert` | the in-space agent nag |
| `OnEscalatingPathMessage` | 1 | `instanceID` | the journal Expeditions popup. **Reads `GetEscalations().header`**, so it will `AttributeError` against a `None` escalation rowset | escalations, which are post-Destiny |
| `OnAgentSlashCommandExecuted` | 1 | `agentID` | reopens the dialogue window | GM tooling only |

`OnDungeonEntered`/`OnDungeonExited` (arity 2), `OnEnteringDungeonRoom`
(arity 5) and `OnDungeonMessage` (arity 3) belong to the dungeon lane and are
unreachable while docked.

**Prefix discipline.** All of these begin `On`, so they dispatch through
`ScatterEventWithoutTheStars` — each handler in its own `uthread.worker`,
blocking allowed, and `ExceptionWrapper.NotifyUp` swallows anything that
escapes (`docs/reference/push-path.md` §2.3). **So no agent push we can send can kill
the client's receive tasklet**, and a handler is free to call back to us —
which `JournalSvc` immediately does, via the partial journal refresh. Expect
`GetMyJournalDetails` on the bound object to arrive *as a consequence of* the
push.

---

## 8. Bookmarks, the mission location, and what Destiny blocks

### 8.1 The bookmark row

A mission's bookmarks are not a separate call. They ride in
`GetMyJournalDetails`'s mission 10-tuple at index 6, and the client turns them
into People & Places entries itself:

```python
agentMenu = sm.GetService('journal').GetMyAgentJournalBookmarks()   # bookmarkSvc.py:124
for missionNameID, bms, agentID in agentMenu:
    for bm in bms:
        c = bm.copy()
        ...
        c.bookmarkID = ('agentmissions', c.agentID, c.locationType,
                        c.locationNumber)                            # :131
        bookmarks[c.bookmarkID] = c
```

`GetAllBookmarks` is `GetBookmarks()` merged with `GetAgentBookmarks()`
(`bookmarkSvc.py:140`). **[V]** So **mission bookmarks never touch the
bookmark service's own storage** — they are derived from the journal on every
read, and there is no `AddBookmark` for them.

The attribute set a bookmark row must carry, collected from every consumer:
**[V]**

| attribute | read by | notes |
| --- | --- | --- |
| `hint` | `journal.py:418`, `bookmarkSvc.py:127` | **a localisation *label path*, not text** — see below |
| `locationID` | `journal.py:418`, `bookmarkChecker.py` | the solar system, for the hint and for the range checks |
| `locationNumber` | `journal.py:419`, `bookmarkSvc.py:131` | 0-based; the hint renders `index = locationNumber + 1` |
| `locationType` | `journal.py:494`, `bookmarkSvc.py:131`, `bookmarkChecker.py:138` | one of `'agenthomebase'`, `'objective'`, `'objective.source'`, `'objective.destination'` |
| `itemID` | `journal.py:494`, `bookmarkChecker` | compared against a station id on undock |
| `agentID` | `bookmarkSvc.py:131`, `bookmarkChecker` | also the `IsAgentBookmark` test |
| `typeID`, `x`, `y`, `z` | `bookmarkChecker` | in-space only |
| `folderID`, `creatorID` | `bookmarkChecker.IsEditableWithCharacterRoles` | menu gating only |
| `originalHint` | written by the client | must be **assignable** (§6.2) |
| `isAgentBase` | `missionObjectivesTracker.GetAgentMissions` | optional, tested with `in bm.__dict__` |

**`hint` is a label path** and the client renders it with two keyword
arguments:

```python
b.hint = GetByLabel(b.originalHint, location=b.locationID,
                    index=b.locationNumber + 1)     # journal.py:418
```

The label vocabulary is in the corpus and is exactly the shape that implies:
**[V]**

```
235205  UI/Agents/Bookmarks/AgentBase            'Agent Home Base - {[location]location.name}'
235221  UI/Agents/Bookmarks/Objective/Objective  'Objective - {[location]location.name}'
235213  UI/Agents/Bookmarks/Objective/Sequence   'Objective #{[numeric]index} - {[location]location.name}'
235219  UI/Agents/Bookmarks/Objective/Pickup     'Objective (Pick Up) - {[location]location.name}'
235211  UI/Agents/Bookmarks/Objective/DropOff
235226  UI/Agents/Bookmarks/Encounter/Encounter  'Encounter - {[location]location.name}'
235228  UI/Agents/Bookmarks/Encounter/Deadspace
235234  UI/Agents/Bookmarks/Mining/Mining
235222  UI/Agents/Bookmarks/Reward/Reward
```

— twenty-eight labels under `UI/Agents/Bookmarks/`, in the families
`AgentBase`, `Objective/*`, `Encounter/*`, `Mining/*`, `Reward/*`, each with
plain, `Sequence`, `Deadspace` and `AgentBase` variants. **So the server sends
a label path and an id, and CCP's own bookmark naming comes out.** The
`{[numeric]index}` token is why `index` is passed, and it is why the plain and
`Sequence` variants exist.

### 8.2 The `LocationWrapper` dict — the other location shape

Distinct from a bookmark row, and used by `GetAgentLocationWrap`, by the
`transport`/`fetch` objective entries and by each `dungeons` entry's
`location`. `LocationWrapper` (`agentDialogueUtil.py:272`) reads: **[V]**

| key | required? | line |
| --- | --- | --- |
| `solarsystemID` | **yes** | `:275` |
| `typeID` | **yes** | `:296`, `:305`, `:309` |
| `locationID` | **yes** | `:297`, `:305`, `:310`, `:312` |
| `locationType` | optional, tested with `in` | `:273` |
| `coords` | optional 3-tuple; **if present, `agentID` becomes required** | `:290`, `:292` |
| `referringAgentID` | optional | `:293` |
| `shipTypeID` | optional; swaps the name for `UI/Agents/Items/ItemLocation` | `:307` |

The name comes from `cfg.evelocations.Get(location['locationID'])
.locationName` (`:312`).

### 8.3 What happens to a location that does not exist in space

This is the question that bounds the subsystem, and the answer is better than
expected.

**Nothing on the docked path resolves a mission location through Destiny or
the ballpark.** Three independent degradation paths, all verified:

1. **In prose.** `{[location]X.name}` goes through
   `LocationPropertyHandler._GetName`, whose `except KeyError` returns the
   literal `'[no location: %d]' % locationID` and logs a warning. **No
   exception reaches the caller.** **[V]**
2. **In a bookmark name.** Same handler, via the `UI/Agents/Bookmarks/*`
   labels. Same degradation.
3. **In `LocationWrapper`.** `cfg.evelocations.Get` is *not* wrapped there, so
   an unknown `locationID` raises — but it raises inside the objective-pane
   build, which the window's own `try` does not cover. **This one needs the
   location to be primable**, which means in `cfg.evelocations`, which means a
   station, system, constellation, region or celestial we can name. It does
   **not** mean the location exists in a ballpark.

And the in-space consumers refuse to run while docked. `BookmarkChecker
.getDistanceToActiveShip` returns early when `self.GetBallpark() is None`
(`bookmarkChecker.py`), `GetEntryPoint` is only called from inside that
function (`:138`), and `GetSecurityWarning` compares against
`session.solarsystemid2`, which is `None` in a station
(`agentDialogueUtil.py`, `GetSecurityWarning`).

### 8.4 So: useful while docked, versus blocked on Destiny

**Useful while docked — all of §1's slice, and more than that:**

- the lobby Agents tab, with available/unavailable buckets and replay timers
- the whole agent dialogue: header, standing, division, LP, CCP's briefing
  prose with real substitutions, action buttons, the decline warning and the
  replay countdown
- offer, accept, decline, defer, quit, complete — every state transition,
  because they are all `DoAction`
- the mission journal window and the journal's mission list
- mission bookmarks in People & Places, with CCP's own names
- rewards, bonus rewards, collateral, gifted items, LP and research points
- the courier undock check (`journal.CheckUndock` → `agents
  .CheckCourierCargo`, `journal.py:495`) — which is docked-side and is the
  one place a courier mission's cargo is actually enforced
- the objective *document* (`GetMissionObjectiveInfo`), including dungeon
  entries, as long as the `dungeonID` is a real one

**Blocked on Destiny, and not worth building before it:**

- every completion test that is not "am I docked here with this cargo":
  kill, destroy, hack, salvage, approach, gate traversal
- `GetEntryPoint`, warp-to and approach from a mission bookmark
- `GetSecurityWarning`'s route check, which needs `solarsystemid2` and the
  pathfinder
- the live objective tracker's in-space half: `IsInActiveDungeonID`,
  `OnDungeonEntered`, `OnEnteringDungeonRoom`, the trigger objectives
- escalations (`dungeonExplorationMgr`) and the anomaly/signature trackers
- `keeper.ActivateAccelerationGate`, i.e. moving between dungeon rooms

**The honest summary: a courier mission is almost entirely servable today, and
a combat mission is servable up to the moment the player undocks.** The player
can be offered "Serpentis Spies", read CCP's 1,314-character briefing, accept
it, see the bookmark and the rewards, and then undock into nothing. Which is
the same wall every other subsystem hits, and not one this lane can move.

---

## 9. A finding the lane did not expect: the message-slot vocabulary

`missions.fsdbinary` carries, per mission, a list of `(key, messageID)`
records. `docs/re/static-corpus.md` §6.2 describes them as 16-byte records
`{uint32 key, uint32 messageID}`. They are in fact **two `uint64`s, and the
first is a payload-relative pointer to a name string.** **[V]**

A regex scan of the decoded payload for `messages\.[A-Za-z0-9_.]+` yields
**exactly 27 slot names**: **[V]**

```
messages.mission.briefing
messages.mission.offered.agentsays
messages.mission.accept.charsays
messages.mission.accepted.agentsays
messages.mission.decline.charsays
messages.mission.declined.agentsays
messages.mission.ask.charsays
messages.mission.complete.charsays
messages.mission.completed.agentsays
messages.mission.completed.nextmission.agentsays
messages.mission.quit.charsays
messages.mission.quitted.agentsays
messages.mission.option1.charsays   (also option2, option3, option4)
messages.mission.extrainfo.header
messages.mission.extrainfo.body
messages.mission.timeout.emailheader
messages.mission.timeout.emailbody
messages.mission.offerexpired.email.header
messages.mission.offerexpired.email.body
messages.root.agentsays
messages.root.missioninprogress.agentsays
messages.epicMission.journalText.chapterTitle
messages.epicMission.journalText.inProgressMessage
messages.epicMission.journalText.completedMessage
```

Verified end to end on two missions: **[V]**

- mission 1210, `agent.missionTemplatizedContent_BasicKillMission`,
  `dungeonID = 301`, one record →
  `messages.mission.briefing` → message **141308** → *"I have terrible news,
  {[character]player.name}. A major personnel convoy carrying countless
  {[npcOrganization]agentFactionID.name} members was completely oblit..."*
- mission 1084, same template, six records resolving to
  `messages.mission.briefing` (127515), `messages.mission.offered.agentsays`
  (127523), `messages.mission.accept.charsays` (127518, *"Sure, I'm up for
  it."*), `messages.mission.declined.agentsays` (127516), and the
  offer-expired email header (127527).

**This is the missing map between the mission table and the call surface.**
Read across to §3 and §4:

| FSD slot | where it goes |
| --- | --- |
| `messages.mission.offered.agentsays` | `DoAction`'s `agentSaysMsg` while the mission is offered |
| `messages.mission.briefing` | `GetMissionBriefingInfo['Mission Briefing ID']` |
| `messages.mission.accepted.agentsays` | `DoAction`'s `agentSaysMsg` after an accept |
| `messages.mission.declined.agentsays` | `DoAction`'s `agentSaysMsg` after a decline |
| `messages.mission.completed.agentsays` | `DoAction`'s `agentSaysMsg` after a turn-in |
| `messages.mission.extrainfo.header` / `.body` | `GetMissionObjectiveInfo['missionExtra']`, which is exactly a `(headerID, bodyID)` pair |
| `messages.mission.*.charsays` | the *player's* line; rendered by the new MissionGiver / conversation UI, not by `AgentDialogueWindow` |
| `messages.root.agentsays` | the agent's greeting with no mission in play |
| `messages.mission.timeout.*` / `offerexpired.email.*` | EVE-mail, i.e. the `social-mail` lane |

`missionNameID` (column `+196`) is the `'Mission Title ID'`, and it resolves
for all 2,892 missions. So **a server can serve a real mission's whole
conversation out of two FSD columns and the localisation corpus, with no
authored text at all.**

Caveat, stated because this lane is not the owner of that table: the
per-mission `messages` *list* read through `tools/fsd.py`'s current column
map mis-resolves on some rows (a handful of pointers land in adjacent
variable-length data and produce garbage strings). The **27-name vocabulary**
above is from a direct payload scan and is solid; the **per-mission mapping**
is solid for 1084 and 1210 and should be re-derived by the `mission-model`
lane rather than trusted wholesale from here.

---

## 10. Corrections to documents already in the tree

1. **`docs/reference/bound-objects.md` §2.3 and §4.7 are wrong about `agentMgr`.** They
   record it as having "constructor sites but no method call sites", and say
   that `eveMoniker.GetAgent` "has **no** references anywhere else in the
   tree" and "appears to be dead code". `GetAgent` has **two** call sites in
   `station/agents/agents.py:317` and `:319`, and **fourteen** methods are
   called on the resulting moniker from six modules. The scanner missed them
   for exactly the reason its own §4.9 predicted: `GetAgentMoniker` stores the
   moniker in a **dict** (`self.agentMonikers[agentID]`) and hands it out from
   there, which the taint scanner does not model. `missionTrackerMgr` is
   missing its four methods for the same reason (`self.missionTrackerMgr =
   sm.RemoteSvc('missionTrackerMgr')`, an attribute-held handle). **This is
   the single largest known gap in that inventory and it is the gap this lane
   was created to fill.**
2. **`findings/client-calls.tsv` mislabels `missionTrackerMgr` as `moniker`.**
   It is a plain `sm.RemoteSvc` service call whose handle is held on an
   attribute (`missionObjectivesTracker.py`, `Initialize`), not a bound
   object. The `kind` column says `moniker` and the `notes` say "handle via
   attr .missionTrackerMgr"; the handle is a service connection.
3. **`findings/remote-services.txt` lists `missionTrackerMgr` with
   `methods(0)`.** There are four: `GetAllMissionObjectives`,
   `GetMissionInfoItems`, `IsInActiveDungeonID`, `UpdateAllMissions`.
4. **`docs/re/static-corpus.md` §6.2's message-record description is off by
   half.** See §9: two `uint64`s, the first a string pointer, not two
   `uint32`s.

---

## 11. What is `unknown`

Listed so nobody re-derives it by accident, and so nobody mistakes a gap for
a fact.

- **`GetDungeonShipRestrictions`'s shape.** The only call site is
  `activeMissionController.py:106`, inside `_FetchDisallowedTypeIDs`, and this
  lane did not read it. Not on the docked path.
- **`GetMyJournalDetails`'s `[1]`.** No consumer anywhere in the tree reads
  it. It must be a list. "Offers" is **[I]**.
- **`missionType`'s wire type.** The `appConst.missionType*` constants are
  label-path strings, and the journal renders the column, but no row has
  crossed the wire to confirm that the server sends the path rather than an
  int the client maps.
- **`quality` and `isLocator` on the agent row.** Carried by `service.rs`
  because the Agent Finder reads `isLocator`; no agent row has crossed the
  wire. Inherited **[I]**.
- **The exact `appConst.py` lines for eight of the sixteen `agentMission*`
  event strings** (`dungeon_moved`, `offered`, `offer_declined`,
  `offer_expired`, `prolong`, `research_started`, `research_update_ppd`,
  `talk_to_completed` — the last is measured at `:1924`, the others are not).
  The *strings* are **[V]**.
- **Whether `eve-server` can issue a `CallReq` to the client at all**, which
  is what §3.3's twenty exported calls would need. `docs/reference/push-path.md` is
  notification-only.
- **`epicArcStatus.GetService`.** `findings/remote-services.txt` lists it as a
  method of that service. It is almost certainly a scanner artefact from
  `sm.RemoteSvc('epicArcStatus')` being stored and then `.GetService` read off
  something else, but this lane did not confirm it.
- **Which `lastActionInfo` keys exist beyond the five named here.** Five are
  read (`missionCompleted`, `missionDeclined`, `missionQuit`,
  `missionCantReplay`, `loyaltyPoints`); whether TQ sends more is unknowable
  from the client.
- **The `contentID` namespace.** It is passed opaquely everywhere: it keys
  `missionArgs`, it is `GetMissionBriefingInfo['ContentID']`, and it is the
  second half of every `(msgInfo, contentID)` pair. Nothing in the client
  computes with it. Whether it equals `missionID` from `missions.fsdbinary` is
  **[I] probably yes** and unverified — and it does not matter to the client
  either way, which is why the slice can choose it freely.
