<!-- Provenance: the `mission-model` lane, 2026-10-20. Read-only on the
     client and on the server: the only files written are this one,
     `findings/corpus-missions.tsv` and `docs/lanes/mission-model.md`. No
     cargo was run and no Rust was written. `missions.fsdbinary` was decoded
     with a scratch reader driven by `findings/pyd-schemas.tsv`; client line
     numbers come from `tools/pyc27.py`'s `co_firstlineno`, never from
     uncompyle6. Marks are the house convention: **[V]** read from this
     build's bytecode, from its static data, or from this tree's code,
     **[I]** inferred, **[unknown]** not established. -->

# The mission model: what a server implements one real mission from

Client build V24.01 (3619879). Companion files:
`findings/corpus-missions.tsv` (the table's shape plus six sample rows),
`docs/lanes/mission-model.md` (the park note).

Read alongside `docs/re/agents-missions.md` (the call surface and the state
machine — this document fills in the data behind it),
`docs/re/static-corpus.md` §6.2 (the first reading of this table, corrected in
§2 and §6 below), `docs/re/dogma.md` §1.1 and §9.3 (the cFSD record model) and
`docs/re/localization.md` (the prose corpus).

**The one-line summary.** Every value the five docked agent calls need for a
real level-1 courier mission is in the client, except four: the *destination*,
the *ISK and LP payout*, the *expiry instant* and the *decline window* — and
those four were server policy on Tranquility too. **A server can offer,
brief, accept, decline and turn in CCP's own "Slave Shipment" with no
authored prose at all.**

---

## 1. One mission, completely specified

This section leads because it is the point: it is the mission a server should
ship first, with every value the five calls need, and the prose resolved so a
reader can see what the player reads.

### 1.1 The cast, all read from the client

| thing | value | where it was read |
| --- | --- | --- |
| station | **60012526**, *"Tanoo V - Moon 1 - Ammatar Consulate Bureau"*, typeID 2502, operationID 33, owner 1000126 | `solarsystemcontent.static`, system 30000001, planet 40000011, moon 40000012, key `stationName` **[V]** |
| solar system | **30000001**, *"Tanoo"*, security 0.858 | same file **[V]** |
| agent | **3010110**, *"Alfroren Agitta"* — `agentLevel` **1**, `agentType` **2** (`agentTypeBasicAgent`), `agentDivisionID` **24** (*"Security"*), `corporationID` **1000126**, `stationID` **60012526**, `storyLineMissions` absent | `npccharacters.fsdbinary`, nested `agent` struct **[V]** |
| corporation | 1000126, *"Ammatar Consulate"*, faction 500007 | already in `service.rs` **[V]** |
| mission | **4832**, *"Slave Shipment"*, `contentTemplate` `agent.missionTemplatizedContent_BasicCourierMission` | `missions.fsdbinary` **[V]** |
| `contentID` | **4832** — ours to choose, and choosing `missionID` costs nothing | `agents-missions.md` §11 **[V]** |
| destination | **60014437**, *"Tanoo IV - Trust Partners Trading Post"*, typeID 2502, owner 1000163, **same solar system** | `solarsystemcontent.static` **[V]** — but *which* station is the server's choice, see §3 |

**The server is currently using the wrong agent, and this is the first thing
to change.** `service.rs`'s `STATION_AGENT_ID` is 3,004,001. That id is a
real `npccharacters` row — *"Anfrid Uptemulf"*, corporation 1000126, station
60012526 — but its `agent` presence bit is **clear**: it is an NPC employee,
not an agent. **[V]** The three characters at 60012526 that do carry an
`agent` struct are:

| agentID | name | level | agentType | division |
| --- | --- | --- | --- | --- |
| **3010110** | Alfroren Agitta | **1** | **2** (basic) | 24 (Security) |
| 3019363 | Dovirch Anselm | 1 | 10 (epic arc) | 24 |
| 3018559 | Kaeg Zkaen | 3 | 24 | 24 |

So 3010110 is exactly the agent `agents-missions.md` §1 Step 1 asks for, and
the `agentTypeID = 2` / `level = 1` that `agent_rowset` currently *chooses*
become *copies*. `divisionID` becomes **24**, not `None`.

`service.rs`'s comment at the station-agent row says the nested-`agent`
struct read "produced implausible values, three times, under three different
base conventions" and that "the `agent` presence bit reads as absent for the
one character at this station that is most likely to be an agent". **Both
observations were correct and the conclusion drawn from them was not.**
`findings/pyd-schemas.tsv` describes the convention accurately; the presence
bit really is absent, because 3004001 really is not an agent. The convention
is: the nested struct begins at its parent's **record-relative** offset
(`agent` at record `+8`), its own fields are struct-relative off that, and
its presence word is at `parent_offset + word` — `npccharacters`'s `agent`
struct has no presence word of its own, so `agentDivisionID`,
`agentLevel`, `agentType` at record `+16`, `+20`, `+24` are unconditional.
Read that way 3010110 comes out as the table above, and `agentLevel` over all
10,972 agent rows is 1..5 with no outliers. **[V]**

### 1.2 The FSD row, in full

Mission 4832, every populated column (the rest of the 22 are absent — see
§2):

```
missionID                4832
nameID                   56901        -> 'Slave Shipment'
contentTemplate          agent.missionTemplatizedContent_BasicCourierMission
factionID                500003       (Amarr Empire)
hasStandingRewards       1
courierMission.objectiveTypeID    2665   -> 'Group of Elite Slaves', 60.0 m3, groupID 283
courierMission.objectiveQuantity     4   -> 240.0 m3 total
courierMission.objectiveSingleton    0
messages                 10 entries, below
```

Absent: `clientObjectives`, `contentTags`, `extraStandings`, `agentTypeID`,
`corporationID`, `divisionID`, `expirationTime`, `fixedLpRewardAlpha/Omega`
(present, both 0), `initialAgentGift*`, `killMission`, `missionRewards`,
`nodeGraphID`, `solarSystemID`, `stationID`.

Two notes that generalise, both **[V]**:

- **`factionID` 500003 is Amarr Empire and our agent's faction is 500007
  (Ammatar Mandate).** Nothing in the client reads `missions.factionID`, so
  this gates nothing: it is **[I]** "which faction's agents may offer this"
  and is server policy. Lore-wise the Ammatar Consulate is the Amarr vassal
  and a slave-shipping corporation, so the mismatch is cosmetic. If it
  bothers you, 206 of the 238 `BasicCourierMission` rows have no `factionID`
  at all.
- **`missionRewards` is absent, and that is not specific to 4832.** Of the
  992 `BasicKillMission` and 238 `BasicCourierMission` rows, **zero** carry a
  Credits reward, and `fixedLpRewardOmega` is nonzero only on the 78
  `HeraldryMission` rows. Fixed rewards appear only on `Storyline*` and
  `EpicArc*` missions. So **for the whole Basic family the payout is server
  policy**, which is what one would expect of a repeatable level-scaled
  mission. See §2 for where ISK hides when it is there (it is `typeID 29`).

### 1.3 The prose, resolved

All ten slots of `messages`, with the English text from
`localization_fsd_en-us.pickle`. Tokens are shown as the corpus declares
them; `{[character]agentID.name}` and `{[character]player.name}` are supplied
by the *client* (`GetAgentArgs`, and `localization.Get`'s
`kwargs['player'] = session.charid` default), so the only token this server
has to supply is `objectiveDestinationID`. **[V]**

| slot | id | text |
| --- | --- | --- |
| `messages.mission.briefing` | 134916 | *"It is important to the Empire that we get a shipment of elite slaves to `{[location]objectiveDestinationID.name}` so that the workers there do not fall any further behind schedule. `<br><br>` `<font color = #EE3B3B>`Remember to move the slaves to your cargo hold before departure.`</font>`"* |
| `messages.mission.offered.agentsays` | 134918 | *"Excellent. As you know, it is the duty of every citizen to demonstrate his or her loyalty to the state from time to time. … It seems that the construction works at `{[location]objectiveDestinationID.name}` are experiencing an unfortunate labor shortage, possibly brought on by their overly harsh personnel management methods. … Are you a loyal enough citizen to do this task and do it quickly? `<br><br>` I will upload a bookmark to your People & Places databank; use it to locate and reach your destination."* (903 characters) |
| `messages.mission.accepted.agentsays` | 134919 | *"Well said, citizen. Good luck."* |
| `messages.mission.declined.agentsays` | 134921 | *"The Empire is disappointed in you, `{[character]player.name}`."* |
| `messages.mission.completed.agentsays` | 134920 | *"You have done well, citizen."* |
| `messages.mission.extrainfo.header` | 134917 | *"Helpful Hints"* |
| `messages.mission.extrainfo.body` | 134915 | *"The easiest way to travel to a destination station quickly is to use the Overview. Stargates are marked in the Overview settings panel under the "Celestial" group. …"* (1,165 characters of genuine new-player instruction, ending *"Once the slaves are delivered, return to your agent's home station … to collect your reward."*) |
| `messages.mission.ask.charsays` | 134924 | *"Good day, `{[character]agentID.name}`. I am seeking employment."* |
| `messages.mission.accept.charsays` | 134922 | *"I am a loyal citizen. I will deliver the cargo."* |
| `messages.mission.complete.charsays` | 134923 | *"The slaves have been successfully delivered, `{[character]agentID.name}`."* |

That `extrainfo.body` is the reason this mission was chosen over the other
237: **it is CCP's own level-1 tutorial text for how to fly a courier
mission.** This is a starter mission, in the data, without anyone having to
decide that it is.

4832 has **no** `quit.charsays`, `quitted.agentsays`, `root.agentsays`,
`timeout.*` or `offerexpired.*` slot. A Quit response therefore has to be a
plain server string — which is legal and loud-free: `ProcessMessage` returns
a `basestring` `msgInfo` verbatim, before any priming
(`station/agents/agents.py:698`). **[V]**

### 1.4 The five calls, filled

All five are on `Moniker('agentMgr', 3010110)`. Shapes are
`agents-missions.md` §2–§4; this is only the content.

**(a) `DoAction(None)` — the offer.** The window's `_GetConversation`
(`agentDialogueWindow.py:484`) auto-calls `DoAction(firstActionID)` only when
`actionID is None`, `availableActions` is non-empty, the first action's
*button type* is `RequestMission` (2) or `ViewMission` (1), **and** either
`len(availableActions) == 1` or the agent is neither a locator nor a research
agent. **[V]** (`agents-missions.md` §1 Step 4 gives the first three
conditions and not the fourth.) Offering Accept first therefore costs **one**
round trip and no invented greeting:

```python
(( (134918, 4832),              # agentSaysMsg = (messageID, contentID)
   [ (2, 3),                    # our actionID 2 -> button Accept  (3)
     (3, 9) ] ),                # our actionID 3 -> button Decline (9)
 { 'missionCompleted': 0, 'missionDeclined': 0,
   'missionQuit': 0, 'missionCantReplay': 0, 'loyaltyPoints': 0 })
```

The TQ-faithful two-step — `[(1, 2)]` on `DoAction(None)`, the client
immediately calling `DoAction(1)`, and *that* returning the offer above —
needs one greeting line, and 4832 has no `root.agentsays` to supply it. Only
188 of 2,892 missions do, and **none** of the 238 `BasicCourierMission` rows.
So the one-step form is the one that ships with zero authored text. **[V]**

Then: `DoAction(2)` → `((134919, 4832), [])` plus the
`OnAgentMissionChange('accepted', 3010110)` push; `DoAction(3)` →
`((134921, 4832), [])` plus `'declined'`; after acceptance `DoAction(None)` →
`((134918, 4832), [(4, 6), (5, 11)])` (Complete, Quit); `DoAction(4)` →
`((134920, 4832), [])` with `lastActionInfo['missionCompleted'] = 1` plus
`'completed'`.

**(b) `GetMissionBriefingInfo()`.** The key *strings* are
`appConst.agentMissionBriefing*` and were read out of `appConst.pyc`'s
module code: **[V]**

```python
{ 'ContentID':           4832,     # subscripted whenever the dict is truthy
  'Mission Title ID':    56901,    # subscripted
  'Mission Briefing ID': 134916,   # the briefing slot
  'Mission Keywords':    {'objectiveDestinationID': 60014437},
  'Mission Image':       '',       # subscripted, and interpolated into HTML
  'Decline Time':        -1,       # subscripted
  'Expiration Time':     None,     # subscripted when 'Decline Time' is None
  'AcceptTimestamp':     None }    # subscripted by the new MissionGiver window
```

Five things here are new or sharper than `agents-missions.md` §4.3, all
**[V]**:

- `agents.GetMissionBriefingInfo` (`agents.py:823`) is
  `if briefingInformation and briefingInformation['ContentID']:
  self.missionArgs[(agentID, briefingInformation['ContentID'])] =
  briefingInformation['Mission Keywords']`. So `'ContentID'` is mandatory on
  a truthy dict and `'Mission Keywords'` is mandatory on a truthy
  `'ContentID'`.
- **`'Mission Image'` is mandatory and must be a string, not `None`.**
  `GetExtraMissionInfoHTML` (`agentDialogueWindow.py:347`) does
  `'<br><center>%s</center>' % briefingInformation['Mission Image']` with a
  bare `BINARY_SUBSCR`. `None` renders the literal word *"None"*, centred,
  in the agent window.
- `'Decline Time'` is **seconds** (it goes to `FmtTimeInterval` with
  `breakAt` min/sec); `-1` selects the generic 4-hour warning, message
  235491. `'Expiration Time'` is a **datetime** — its label is
  `UI/Agents/Dialogue/ThisMissionExpiresAt`, message 235574, *"This mission
  expires at `{[datetime]expireTime}`"* — so it is a blue filetime, not a
  second count.
- `'AcceptTimestamp'` **is** read, and `agents-missions.md` does not mention
  it. `OldMissionController.update_mission` (`evemissions/client/
  missioncontroller.py:317`) does `accept_time = briefing_data[
  agentMissionBriefingAcceptTimestamp] if briefing_data else None` with a
  bare `BINARY_SUBSCR`, so it is **mandatory on a truthy dict** for the new
  MissionGiver window. Our agent's `agentTypeID` 2 keeps the old HTML
  window, which never reads it — but sending `None` costs nothing and
  removes a trap for whoever later gives an agent a type in
  `AGENT_TYPES_WITH_NEW_UI`. **[V]**

  The same function independently confirms two things said above: its
  auto-`DoAction` condition is byte-for-byte `_GetConversation`'s (first
  action's button type in `(RequestMission, ViewMission)`, **and**
  `len(actions) == 1` or neither locator nor research agent), and it too
  calls `ProcessMessage(agent_says)` *before* `GetMissionBriefingInfo`.
  **[V]**

**(c) `GetMissionKeywords(4832)`** → `{'objectiveDestinationID': 60014437}`.

**`agents-missions.md` §2.1's last paragraph needs one correction:** a
briefing that ships its own keyword dict does *not* avoid this call on the
dialogue path. `_GetConversation` renders `agentSaysText =
GetMessageText(agentSaysMsg)` → `ProcessMessage` → `PrimeMessageArguments`
**before** `ReconstructLayout` → `GetBriefingHTML` →
`GetMissionBriefingInfo`. So on the offer the keywords are asked for, and the
briefing's copy only re-primes an already-primed key. **[V]** Answer it
properly; `{}` would render the raw `{tokens}` in the offer line.

**(d) `GetMissionObjectiveInfo()`.** §4.5's twelve mandatory keys, filled for
an accepted 4832:

```python
{ 'completionStatus': 0,
  'missionTitleID':   56901,
  'missionState':     2,                       # accepted
  'objectives': [ ('transport',
                   (1000126,                                   # pickupOwnerID
                    {'solarsystemID': 30000001, 'typeID': 2502,
                     'locationID': 60012526},                   # pickupLocation
                    1000163,                                    # dropoffOwnerID
                    {'solarsystemID': 30000001, 'typeID': 2502,
                     'locationID': 60014437},                   # dropoffLocation
                    {'hasCargo': 1, 'quantity': 4,
                     'typeID': 2665, 'volume': 240.0})) ],
  'dungeons':       [],
  'locations':      [30000001],                # solar system ids
  'agentGift':      [],                        # initialAgentGift* absent on 4832
  'normalRewards':  [(29, 15000, None)],       # typeID 29 = Credits; see below
  'loyaltyPoints':  0,
  'researchPoints': 0,
  'bonusRewards':   [],                        # missionRewards absent on 4832
  'collateral':     [],
  'missionExtra':   (134917, 134915),           # extrainfo header, body
  'contentID':      4832 }                      # required when missionExtra is set
```

- `'transport'` versus `'fetch'` is **[I]**. A classic courier's cargo
  appears in the agent's station on accept, so both readings are defensible;
  `'transport'` with `pickupLocation` = the agent's own station is what makes
  the client tick the pickup leg off locally
  (`agentDialogueUtil.py:58-70`, `location['locationID'] ==
  session.locationid and cargo['hasCargo']`). `'fetch'` drops the pickup leg
  and has arity 3.
- Whether `cargo['volume']` is per unit (60.0) or the total (240.0) is
  **[unknown]**; nothing in the client disambiguates it, and both render.
- `'normalRewards'` is **invented**. `typeID 29` resolves to *"Credits"* and
  is how the 374 missions that *do* ship a reward carry ISK, so a Credits row
  is the right shape; the 15,000 is policy. See §3.

**(e) `GetAgentLocationWrap()`** → unchanged from what the server already
sends, and now provably right:

```python
{'locationID': 60012526, 'typeID': 2502, 'solarsystemID': 30000001}
```

`locationType` stays omitted so the `LocationWrapper` branch whose
`cfg.evelocations.Get` is unguarded is never taken
(`agentDialogueUtil.py:273`). **[V]**

### 1.5 The whole of what the server must invent for 4832

Four values, and each was server-side on Tranquility too:

1. `objectiveDestinationID` (and its system) — §3.
2. the ISK/LP/standing payout — `missionRewards` and `fixedLpReward*` are
   absent for the entire Basic family.
3. `'Expiration Time'` — `expirationTime` is absent on 4832 (and on 72 of the
   238 `BasicCourierMission` rows).
4. `'Decline Time'` — there is no such column anywhere in
   `missions.fsdbinary`; `-1` buys the generic warning and no arithmetic.

Everything else — title, briefing, offer, acceptance, refusal, completion,
the extra-info panel, the cargo type and quantity — is two FSD columns and
the localisation corpus.

**One fallback worth knowing.** Mission **4837**, *"Salve for Sis"*, is also a
`BasicCourierMission`, is faction-free, carries an `expirationTime` of 10,080
and **seventeen** message slots including the timeout and offer-expired
emails, and **not one of its messages contains a single token**. Its briefing
is *"Deliver the secure coded package for your agent."* If you want the first
mission to need `GetMissionKeywords` to be literally `{}`, ship that one.
**[V]**

---

## 2. `missions.fsdbinary`, decoded

2,892 rows, **stride 224**, `uint32` key = `missionID`, hash on
`key % 2892`, `delta` 8. The container is `docs/re/dogma.md` §1.1; the
offsets below are `findings/pyd-schemas.tsv`'s, **record-relative and already
including the delta**, re-read against the data by an independent scratch
reader. Full table with presence bits and FK results:
`findings/corpus-missions.tsv`.

| record offset | field | type | presence bit | notes |
| --- | --- | --- | --- | --- |
| +0 | `missionID` | u32 | — | the key |
| +8 | `clientObjectives` | struct, 32 B | 0x2 | 187 rows; the AIR/Agency block |
| +32 | ↳ `objectiveChainID` | i32 | — | FK → `objectivechains`, 187/187, 21 distinct |
| +40 | `contentTags` | list\<str\> | 0x4 | 131 rows, 15 distinct tags |
| +48 | `contentTemplate` | str | — | 18 distinct; §4 |
| +56 | `extraStandings` | **dict\<u32,f32\>** | 0x80 | 41 rows; `factionID` → standing |
| +72 | `messages` | dict\<str,i32\> | 0x8000 | 2,886 rows, 17,268 entries |
| +88 | `agentTypeID` | i32 | 0x1 | FK → `agenttypes`, 238/238; only values 9 and 13 |
| +92 | `corporationID` | i32 | 0x8 | FK → `npccorporations`, 81/81, 3 distinct |
| +96 | `courierMission` | struct, 16 B | 0x10 | **1,155 rows** |
| +96 | ↳ `objectiveQuantity` | i32 | 0x1 @+108 | 1 … 100,000 |
| +100 | ↳ `objectiveTypeID` | i32 | — | FK → `types`, 1155/1155, 776 distinct |
| +104 | ↳ `objectiveSingleton` | u8 | 0x2 @+108 | |
| +112 | `divisionID` | i32 | 0x20 | **never populated** |
| +116 | `expirationTime` | i32 | 0x40 | 1,020 rows; 11 distinct, max 43,200 |
| +120 | `factionID` | i32 | 0x100 | FK → `factions`, 820/820, 15 distinct |
| +124 | `fixedLpRewardAlpha` | i32 | 0x200 | 2,892 rows, nonzero on 78 |
| +128 | `fixedLpRewardOmega` | i32 | 0x400 | same 78, exactly 10× the alpha |
| +132 | `initialAgentGiftQuantity` | i32 | 0x1000 | 183 rows |
| +136 | `initialAgentGiftTypeID` | i32 | 0x2000 | FK → `types`, 174/174 |
| +140 | `killMission` | struct, 24 B | 0x4000 | **1,664 rows** |
| +140 | ↳ `dropItemInMissionContainer` | i32 | 0x1 @+160 | FK → `types`, 3/3 |
| +144 | ↳ `dungeonID` | i32 | 0x2 @+160 | FK → `dungeons`, 1661/1661, 1,460 distinct |
| +148 | ↳ `objectiveGroupID` | i32 | 0x4 @+160 | FK → `groups`, 4/4 (all 462) |
| +152 | ↳ `objectiveQuantity` | i32 | 0x8 @+160 | 1,516 rows, max 40,000 |
| +156 | ↳ `objectiveTypeID` | i32 | 0x10 @+160 | FK → `types`, 560/560 |
| +164 | `missionRewards` | struct, 32 B | 0x10000 | 1,003 rows |
| +164 | ↳ `bonusReward.rewardQuantity` | i32 | 0x1 @+172 | max 10,600,000 |
| +168 | ↳ `bonusReward.rewardTypeID` | i32 | 0x2 @+172 | FK → `types`, 419/419 |
| +176 | ↳ `bonusTimeInterval` | i32 | 0x2 @+192 | 1…11,000 + one 10,500,000 |
| +180 | ↳ `reward.rewardQuantity` | i32 | 0x1 @+188 | max 40,000,000 |
| +184 | ↳ `reward.rewardTypeID` | i32 | 0x2 @+188 | FK → `types`, 649/649 |
| +196 | `nameID` | msgid | — | `'Mission Title ID'`; 2,892/2,892 resolve |
| +200 | `nodeGraphID` | i32 | 0x20000 | FK → `nodegraphs`, 131/131 |
| +204 | `solarSystemID` | i32 | 0x40000 | **never populated** |
| +208 | `stationID` | i32 | 0x80000 | **never populated** |
| +212 | `hasStandingRewards` | u8 | 0x800 | 1 on 2,817, 0 on 75 |
| +216 | presence bitfield | u32 | — | no bit above 0x20000 is ever set |
| +220 | padding | — | — | always zero |

**The record is now fully accounted for.** 22 named fields, four presence
words (+108 `courierMission`, +160 `killMission`, +192 `missionRewards`,
+216 top level), one `u32` key, two 8-byte dict tails and four bytes of
padding = 224. There is no unexplained slot left, which is the strongest
check available on the layout and is why §6 treats it as one.

Five readings deserve their own line:

- **`messages` at +72 is a `dict<str,i32>` with 16-byte entries** —
  `{char* key, int32 value}`, the key a payload-relative pointer to a
  length-prefixed string. **[V]** confirmed by raw bytes: mission 1210's one
  entry is `885c000000000000 fc27020000000000` → key pointer 23688, length
  25, value 141308.
- **A cFSD dict field occupies 16 bytes, not 8**: the pointer, then the
  dict's *total entry count*. `extraStandings` spans +56…+71 and `messages`
  +72…+87, and the count at +64/+80 equals the decoded entry count on every
  row. **[V]** This is what `findings/pyd-schemas.tsv` leaves implicit and
  what makes +64 and +80 look like unclaimed columns. `static-corpus.md`
  §6.2's "small enums at +80 and +192" are these two: +80 is `messages`'s
  entry count, +192 is `missionRewards`'s presence word.
- **`extraStandings` is `dict<u32 factionID, f32>` with 8-byte entries**, not
  `dict<str,f64>`. **[V]** All 41 rows have exactly one entry, every key is a
  valid `factionID` (500001…500019) and the six distinct values are
  1.5, 2.0, 5.0, 7.0, 10.0, 30.0.
- **ISK is `typeID 29`, *"Credits"*.** There is no ISK column. 374 of the
  1,003 `missionRewards` rows have `reward.rewardTypeID == 29`, up to
  40,000,000 of it; the rest are real items (`Expanded Cargohold I`,
  `1MN Afterburner I`, `Venture`, skill books). **[V]** So
  `static-corpus.md` §6.2's "ISK reward, bonus, collateral" is one reward
  pair, one bonus-reward pair and a bonus *time*: **there is no collateral
  column in this table at all**, and the "one to 100,000 (loyalty points, by
  magnitude)" is `courierMission.objectiveQuantity`.
- **There is no mission level and no mission-to-agent join** for ordinary
  missions. Level comes from the *agent* (`npccharacters.agent.agentLevel`);
  the only mission→agent edge in the client is
  `npccharacters.agent.storyLineMissions`, a `list<i32>` covering 926
  distinct missions over 1,693 references. Which agent offers which Basic
  mission was Tranquility's `agtMissions`/`agtOfferDetails` and is **not in
  the client**. **[V]** The server picks, freely.

Units: `expirationTime`'s eleven values are 0, 120, 240, 480, 720, 1440,
2880, 4320, 5760, 10080, 43200 — read as **minutes** that is 2 h, 4 h, 8 h,
12 h, 1 d, 2 d, 3 d, 4 d, **7 d** and **30 d**, which are EVE's own mission
windows; read as seconds it is 2 min … 12 h. Minutes is **[I]** and
`static-corpus.md`'s "43,200-second bonus window" is **[I] and probably
wrong on two counts**: it is minutes, and it is the mission's own expiry, not
the bonus window. The bonus window is `missionRewards.bonusTimeInterval`,
1…11,000 (**[I]** minutes), with a single 10,500,000 outlier.

---

## 3. The eight `GetMissionKeywords` ids, sourced

`agents-missions.md` §2.2 establishes the key set from the corpus's own token
records and that the client names none of them. For mission 4832 only
`objectiveDestinationID` appears, but the sourcing question is worth
answering for all eight, because it decides which of them a server can ever
fill from FSD.

| key | for 4832 | general source |
| --- | --- | --- |
| `objectiveDestinationID` | **server: 60014437** | **not in `missions.fsdbinary`.** No column holds a courier destination — `stationID` and `solarSystemID` exist and are never populated. The server picks a station and must make it nameable. |
| `objectiveDestinationSystemID` | **server: 30000001** | same; the chosen station's `solarSystemID` from `solarsystemcontent.static` |
| `objectiveTypeID` | **2665**, from FSD | `courierMission.objectiveTypeID` for the 1,155 courier/trade/heraldry missions; `killMission.objectiveTypeID` for the 560 kill/mining missions that have one |
| `objectiveQuantity` | **4**, from FSD | `courierMission.objectiveQuantity` / `killMission.objectiveQuantity`; 2,671 of 2,892 missions have one of the two |
| `dungeonLocationID` | n/a | **server.** The mission gives `killMission.dungeonID` (1,661 rows, FK-closed into `dungeons`), which is the dungeon *template*, not a location. Where the dungeon is placed in space is the server's; this is the most-used token in the corpus (1,208 occurrences) and it is entirely ours. |
| `dungeonSolarSystemID` | n/a | **server**, same reason |
| `objectiveLocationID` | n/a | **server**; 85 corpus occurrences, no FSD column |
| `rewardTypeID` | n/a | `missionRewards.reward.rewardTypeID` when present (649 rows) — **but never for a Basic mission**, so for the Basic family it is **server** |

**So: five of the eight are always the server's (`dungeonLocationID`,
`dungeonSolarSystemID`, `objectiveDestinationID`,
`objectiveDestinationSystemID`, `objectiveLocationID`), two come straight out
of the mission row (`objectiveTypeID`, `objectiveQuantity`), and one
(`rewardTypeID`) comes out of the row for storyline and epic missions and
from the server for every Basic one.** All five of the server's are
*locations*, which is the same observation from the other side: **the client
ships the content and the server owns the geography.**

### Nameability, which is cheaper than it looks

`{[location]X.name}` resolves through `cfg.evelocations.Get(id)
.locationName`, whose handler's `except KeyError` returns the literal
`'[no location: %d]' % id` and logs — it does not raise
(`agents-missions.md` §8.3). **[V]** So a wrong id is a visible blemish, not
a failure. But it does not have to be wrong, because `cfg.evelocations` is a
**server-primed** rowset: a miss issues `config.GetMultiLocationsEx(ids)`
with the columns `locationID:I8, locationName:WSTR, solarSystemID:I8,
x:R5, y:R5, z:R5, locationNameID:I4` (`docs/reference/login-path.md` §9). **[V]**

And the names are in the client's own static data:
`solarsystemcontent.static` gives each NPC station's `stationName` verbatim —
*"Tanoo IV - Trust Partners Trading Post"*, *"Tanoo V - Moon 1 - Ammatar
Consulate Bureau"* — along with its `solarSystemID`, `typeID`, `ownerID`,
`operationID` and position. **[V]** So the server answers
`GetMultiLocationsEx([60014437])` out of a file it already reads and the
briefing names a real station.

Recommended destination for 4832: **60014437, in Tanoo itself.** It is the
other station in system 30000001, so `objectiveDestinationSystemID` is the
system the player is already in, `'locations'` has one entry, and the whole
mission needs exactly one extra `evelocations` row. The cross-system
alternative, if you want the courier to be a journey: 60012529 in system
30000056, another Ammatar Consulate station with two level-1 agents of the
same corporation. Both verified present in `solarsystemcontent.static`.
**[V]**

---

## 4. `contentTemplate` and the objective tables

### 4.1 Courier versus kill is a partition, with no exceptions

`contentTemplate` is a string, 18 distinct values, and it stands in **exact**
correspondence with which of the two nested structs is present:

| family | templates | rows | struct |
| --- | --- | --- | --- |
| courier-shaped | `Basic/Storyline/GenericStoryline/EpicArc/Research` × `Courier`/`Trade`, plus `Heraldry` | **1,155** | `courierMission`, never `killMission` |
| dungeon-shaped | `Basic/Storyline/GenericStoryline/EpicArc` × `Kill`, plus `BasicMining` | **1,664** | `killMission`, never `courierMission` |
| conversation-only | `EpicArcTalkToAgent` (53), `EpicArcAgentInteraction` (20) | **73** | neither |

1,155 + 1,664 + 73 = 2,892, zero exceptions. **[V]** So the distinction a
server needs is already in the presence bits, and `contentTemplate` adds the
*flavour*: storyline vs basic vs epic vs research, which is what decides the
replay rules and the standing effects — both server policy.

Note that **mining missions use `killMission`**: a dungeon plus an
`objectiveTypeID`/`objectiveQuantity` that is ore, not a kill.

`contentTemplate` also maps onto `appConst.missionType*`, which are
localisation **label paths**, not ints — `missionTypeCourier =
'UI/Agents/MissionTypes/Courier'` → message 235247 → *"Courier"*. **[V]**
That closes `agents-missions.md` §11's "`missionType`'s wire type" unknown
from the data side: the journal column's value is a label path, 23 constants
covering the 18 templates, and the mapping is in
`findings/corpus-missions.tsv`'s last block. For 4832 it is
`'UI/Agents/MissionTypes/Courier'`. Whether TQ sent the path or an int the
client mapped is still **[unknown]** — but the path is what the constants
are, so it is the thing to send.

### 4.2 `objectives`, `objectivechains`, `objectivetypes`, `objectivetasktypes`

**These are not the classic mission objective tables.** `objectives.fsdbinary`
has **eight rows**, and all eight are new-player-experience widgets: **[V]**

```
  1  [AIR NPE] 4.1 Before warp   title 'Warp to mission site'  dungeonID 9245
  3  [AIR NPE] 4.2               title 'Mine asteroids'
  5  [AIR NPE] 4.3               title 'Investigate signal'
  6  [AIR NPE] 4.4               title 'Dock at AIR station'
  7  [NES Intro] Return to station
  8  [NES Intro] Do the mission  title 'Complete the Tutorial'
 11  [MAP Onboarding] Return to Station
 12  [MAP onboarding] Do the mission
```

The four tables are the **Agency / AIR objective-chain engine**: a
`clientObjectives.objectiveChainID` names an `objectivechains` row (111
keys), whose `objectives` list holds entries with an `objectiveType` FK into
`objectivetypes` (95 keys), whose `tasks` carry a `taskType` FK into
`objectivetasktypes` (53 keys), plus `triggers`, `inputParametersMap` and
`taskOverrides` dicts of strings. Only **187 of 2,892** missions carry a
`clientObjectives` block at all, and **none** of them is a `Basic*` mission:
112 `StorylineKill`, 48 `StorylineTrade`, 20 `StorylineCourier`, 7
`EpicArcKill`. **[V]**

**So a classic mission's objective does not come from these tables.** It
comes from `courierMission` / `killMission` + the `dungeonID` join, and the
objective *document* the client renders is `GetMissionObjectiveInfo`, which
the server composes (§1.4). This lane did not decode the four Agency tables
beyond their keys, strides and field lists; they are not needed for a Basic
mission and they are a subsystem of their own.

### 4.3 What this lane needs from the `dungeon-model` lane

Staying on the mission side of the join, as briefed. For a kill mission the
mission row gives `killMission.dungeonID` and nothing else about space.
What a server needs from the dungeon side, in priority order:

1. **Whether `dungeons.<id>.missionBriefingID` (record +100, bit 0x200 —
   `findings/pyd-schemas.tsv`) is a second, dungeon-side briefing message
   id**, and whether it ever disagrees with
   `missions.messages['messages.mission.briefing']`. If it is a real
   alternative the `dungeons` entries of `GetMissionObjectiveInfo` can carry
   `briefingMessage` out of FSD rather than from us.
2. **`dungeons.<id>.connections`**, because `_ProcessDungeonData`
   (`agentDialogueUtil.py:248`) reads it *client-side* after
   `GetDungeon(dunData['dungeonID'])` — so a `dungeonID` must be real or the
   objective pane raises. The 1,460 distinct `killMission.dungeonID` values
   all close into `dungeons`, so this is satisfied; it is the one hard data
   dependency in the whole subsystem.
3. **Whether `dungeonobjectives` (85 rows) keys by `dungeonID`**, and whether
   its own `clientObjectives.objectiveChainID` is in the same space as
   `missions.clientObjectives.objectiveChainID`. If it is, the two Agency
   blocks compose and the mission's `overrides` dict is exactly what the name
   suggests.
4. Nothing else. A courier mission needs no dungeon at all.

---

## 5. Docked versus Destiny, for 4832 specifically

`agents-missions.md` §8.4's claim — *a courier mission is almost entirely
servable today, and a combat mission is servable up to the moment the player
undocks* — is **confirmed for 4832**, with one correction and one caveat.

**Servable today, with nothing in space:** the lobby Agents tab with agent
3010110 in it; the dialogue window's header, standing line, division name and
LP; `DoAction(None)` → the 903-character offer with the destination
substituted; the briefing, the *"Helpful Hints"* extra-info panel, the
Accept/Decline buttons and the decline warning; accept, decline, quit and
turn-in, all four being `DoAction`; the journal row and the mission journal
window; the People & Places bookmark, named from
`UI/Agents/Bookmarks/Objective/DropOff` → *"Objective (Drop Off) -
`{[location]location.name}`"*; the reward list; and the objective document
with its `transport` entry and its pickup tick already green, because
`location['locationID'] == session.locationid` is true in the agent's own
station. **[V]**

**The correction.** §8.4 lists "the courier undock check
(`journal.CheckUndock` → `agents.CheckCourierCargo`)" as servable. It is —
but it is also the *only* completion test that can ever fire for 4832, and
it fires on **undock**, not on delivery. Delivery is "be docked at 60014437
with four `Group of Elite Slaves` in the hold", and the server cannot know
that until the player docks, which needs Destiny. So 4832 is offerable,
acceptable, declinable and **turn-in-able only by fiat** today: the server
can accept `DoAction(Complete)` without verifying anything, which is a GM
cheat-complete (`completionStatus == 2`) wearing a normal hat. That is a
deliberate choice a reader should make knowingly, not a thing the data
decides.

**The caveat.** The courier *items* have to exist. Four `Group of Elite
Slaves` (typeID 2665, 60 m3 each) must appear in the player's hangar at
60012526 on accept, or the briefing's own red text — *"Remember to move the
slaves to your cargo hold before departure"* — is a lie and
`CheckCourierCargo` has nothing to check. `item.rs` already spawns items into
60012526 by name and quantity, so this is an existing capability, not a new
one. It is still the one piece of *state* a courier mission needs that an
offer alone does not.

**Blocked on Destiny and not worth building first:** everything in §8.4's
second list, plus, for this mission specifically, nothing else. A courier
mission touches no dungeon, no ballpark, no acceleration gate and no kill
test. That is why it, and not a kill mission, is the one to ship — even
though our level-1 agent's division is Security.

---

## 6. How this was validated

**Calibration set.** The two missions the `agents-missions` lane had already
worked end to end: **1084** (*"Attack the Angel Hideout"*, 13 message slots,
`killMission.dungeonID` 240) and **1210** (*"Vengeance"*, one slot,
`messages.mission.briefing` = 141308, dungeon 301). The reader was built
until it reproduced both exactly, including 1084's six quoted slot→id pairs
(briefing 127515, offered 127523, accept.charsays 127518, declined 127516,
offer-expired header 127527). Those two rows are the only ones used to
*build* the reader.

**Test set: everything else.** Five independent checks, none of which the
calibration could have tuned:

1. **The 27-label vocabulary and its counts.** The reader produces **17,268
   entries over 2,892 missions in exactly 27 distinct labels**, and the
   per-label counts match `docs/re/dogma.md` §9.3's independently derived
   figures on all twelve it publishes (2885 / 1983 / 1432 / 1292 / 1015 /
   934 / 919 / 916 / 823 / 692 / 626 / 582). Two readers, two methods, same
   numbers. **[V]**
2. **Localisation closure.** `nameID` resolves in `en-us` for **2,892 of
   2,892**; the message ids resolve for **17,268 of 17,268**, and all 17,268
   are distinct — no message id is shared between two slots or two missions.
   Partial resolution is the symptom of a wrong offset, and there is none.
   **[V]**
3. **FK closure, thirteen columns, zero misses.** `corporationID`→
   `npccorporations` 81/81, `factionID`→`factions` 820/820, `agentTypeID`→
   `agenttypes` 238/238, `nodeGraphID`→`nodegraphs` 131/131,
   `initialAgentGiftTypeID`→`types` 174/174, `killMission.dungeonID`→
   `dungeons` 1661/1661, `killMission.objectiveTypeID`→`types` 560/560,
   `killMission.objectiveGroupID`→`groups` 4/4,
   `killMission.dropItemInMissionContainer`→`types` 3/3,
   `courierMission.objectiveTypeID`→`types` 1155/1155,
   `missionRewards.reward.rewardTypeID`→`types` 649/649,
   `missionRewards.bonusReward.rewardTypeID`→`types` 419/419,
   `clientObjectives.objectiveChainID`→`objectivechains` 187/187. **[V]**
4. **The record adds up.** Every one of the 224 bytes is claimed by a named
   field, a presence word, a dict's entry-count tail or four bytes of
   padding, and the three never-populated columns were confirmed *as such*
   rather than assumed: their presence bits are clear on all 2,892 rows
   **and** the raw `i32` at +112, +204 and +208 is zero on all 2,892. The
   same test shows +220 is padding. **[V]**
5. **The partition.** `contentTemplate`'s courier/kill/neither split against
   the two struct presence bits is 1,155 / 1,664 / 73 with no row in two
   buckets and no row in none. A wrong presence bit would show up here as a
   mission with both structs or neither. **[V]**

**The traps, and whether they were sprung.** The double-`delta` read: avoided
by using `findings/pyd-schemas.tsv`'s offsets *verbatim* and never mixing
them with `tools/dogma.py`'s `struct_at` convention; the check that catches it
is (4), because a doubled delta leaves holes and overruns. The
`volume`-that-was-really-`capacity` trap: the defence is that no column here
was named from its *values* — every name came out of the loader — and the two
places where a value-based reading was needed (`expirationTime`'s and
`bonusTimeInterval`'s units) are marked **[I]** and left in minutes with the
seconds reading stated. The adjacency trap that `dogma.md` §9.3 quantified
(`messages` at +72 reading right from +56 for 2,851 of 2,892 rows) is
sidestepped entirely: this reader goes to +72, and it decodes the 41
`extraStandings` rows the shortcut breaks on.

**One attribution corrected.** `static-corpus.md` §6.2 says message **145165**
is "reachable from mission 1210 *Vengeance*". It is not. 145165 is
`messages.mission.offered.agentsays` of mission **15673**, *"Whispers in the
Dark - First Contact (1 of 4)"*, a `GenericStorylineKillMission` with dungeon
5280 — and the text is indeed the Serpentis one, tokens
`{[character]player.name}` and `{[location]dungeonLocationID.name}`. That
lane found the id by scanning the arena and attributed it to the wrong
record, which is precisely the failure mode the arena invites. Mission 1210's
single message is briefing 141308. **[V]**

---

## 7. Patches this lane would want, as diffs

This lane owns none of these files.

**(a) `findings/pyd-schemas.tsv` — `missions.extraStandings`' key and value
types.**

```diff
-missions	extraStandings	56	dict<str,f64>	V	getattro chain, _PyObject_New(cfsd.dict); optional(bit=0x80,word=+216)
+missions	extraStandings	56	dict<u32,f32>	V	getattro chain, _PyObject_New(cfsd.dict); optional(bit=0x80,word=+216); 8-byte entries {u32 factionID, f32 standing}; all 41 rows decode and every key closes into factions
```

The offset is right; the key is a `u32 factionID` and the value an `f32`, in
8-byte entries. Read as `dict<str,f64>` the key pointer is 4.6e18 and the
value 1.5e-323 — garbage, but garbage that looks like a presence-bit problem
rather than a type problem, which is why it is worth writing down.

**(b) `findings/pyd-schemas.tsv` — record a dict field's width.** The
`#record` note explains `delta` beautifully and says nothing about field
*widths*, so a reader cannot tell that the eight bytes after a `dict` slot
are part of the dict. Suggested addition to the per-table `#record` note:

```diff
-...stride confirmed by the imul immediate in the row factory
+...stride confirmed by the imul immediate in the row factory. Field widths: a
+str or list field is one 8-byte pointer; a DICT FIELD IS 16 BYTES -- the
+pointer, then the dict's total entry count -- so the slot 8 bytes after a
+dict is not a column.
```

**(c) `findings/pyd-schemas.tsv` — flag the three dead columns.** `divisionID`
@+112, `solarSystemID` @+204 and `stationID` @+208 are in the loader and are
never populated in V24.01. A reader who needs a courier destination will find
`stationID` and believe they have it. One word in `how_derived` —
`never-populated-in-V24.01` — saves that.

**(d) `tools/fsd.py` — `CFSD_COLUMNS['missions']`.** Not included here
because `tools/fsd.py` has no `missions` entry in this tree; if one is added,
the shape to add is `findings/corpus-missions.tsv`'s first block, and the
three things to avoid are `messages` at +56 (that is `extraStandings`),
`dungeonID` at +144 described as a top-level column (it is
`killMission.dungeonID`), and treating a dict slot as 8 bytes.

**(e) `server/.../service.rs` — the station agent.** `STATION_AGENT_ID`
should be **3010110** (*Alfroren Agitta*), and with it `divisionID` becomes
**24** instead of `None`, `agentTypeID = 2` and `level = 1` become copies
rather than choices, and the comment about the nested-struct convention being
unsettled can go: the convention is `findings/pyd-schemas.tsv`'s, and 3004001
reads as a non-agent because it is one. This lane writes no Rust, so that is
stated and not done.

---

## 8. What is `unknown`

- **Which agent offers which Basic mission.** Not in the client, in any
  table. Tranquility's `agtMissions`/`agtOfferDetails`. The server picks, and
  nothing client-side can contradict it.
- **A mission's level.** There is no level column. `npccharacters.agent
  .agentLevel` is the only level in the client's data and it belongs to the
  agent. Which of the 238 `BasicCourierMission` rows were level 1 on TQ is
  **[unknown]**; 4832 is argued to be one from its *"Helpful Hints"*
  extra-info text and its 240 m3 of cargo, which is evidence, not proof.
- **`expirationTime`'s and `bonusTimeInterval`'s units.** Minutes is **[I]**
  from the value sets (10,080 = 7 days, 43,200 = 30 days).
- **`'transport'` versus `'fetch'`** for a courier objective, and whether
  `cargo['volume']` is per unit or total. Both render; neither is pinned.
- **`objectiveSingleton`'s meaning.** **[I]** "give the item unpackaged /
  as a single stack". 1,155 rows carry it, 0 and 1 both occur.
- **`clientObjectives.contextParameters` and `.overrides`**, and the four
  Agency objective tables' contents. Keys, strides and field lists are in
  `findings/pyd-schemas.tsv`; the semantics were not read. Not needed for a
  Basic mission.
- **`agentTypeID` on a mission row.** 238 rows, only two distinct values, 9
  (`agentTypeFactionalWarfareAgent`) and 13 (`agentTypeHeraldry`), both
  FK-closed into `agenttypes`. **[I]** it restricts the mission to that agent
  type; untested.
- **`dungeons.missionBriefingID`.** Exists in the loader (record +100). Owned
  by the `dungeon-model` lane; see §4.3.
- **`'Mission Image'`'s content when it is not empty.** It is interpolated
  into `'<br><center>%s</center>'`, so an HTML fragment. No example was
  found.
- **`'AcceptTimestamp'`'s epoch.** It is subscripted by the new MissionGiver
  controller and passed straight to `mission.update_info(...)`; whether it is
  a blue filetime or a unix second count was not followed.
- **Whether `missionType` crosses the wire as a label path or an int.**
  The constants are label paths; no row has crossed the wire. Inherited
  **[unknown]** from `agents-missions.md` §11, now with the full 18-template
  mapping in `findings/corpus-missions.tsv` for whichever it turns out to be.
