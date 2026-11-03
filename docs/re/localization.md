# The localisation corpus: format, lookup path, and what is in it

Lane: `localization-corpus` (churn Wave 0). Reader: [`tools/loctable.py`](../../tools/loctable.py).
Park note: [`docs/lanes/localization-corpus.md`](../lanes/localization-corpus.md).

Claims are marked **[V]** (read from client bytecode, or measured from the
client's own data) or **[I]** (inferred). `unknown` is used where it is honest.

**The headline: the corpus is a plain Python 2 protocol-0 pickle, we can read
all of it with the standard library, and EVE's written content is in it.**
Mission briefings, mission offer/completion/failure dialogue, objectives text,
mission titles, agent dialogue, item descriptions, NPC corporation and faction
and bloodline descriptions, dungeon object names and every `UserError` body are
all present as English prose, keyed by integer message id.

---

## 1. Format

### 1.1 Where the files are

Ten resources named `res:/localizationFSD/localization_fsd_<lang>.pickle`, plus
`_main`. `$EVE_CLIENT/resfileindex.txt` maps each to
`$EVE_CLIENT/../ResFiles/<2hex>/<hash>_<md5>`. **[V]**

The client reads them in `localization/localizationBase.py`: **[V]**

| thing | citation |
| --- | --- |
| `Localization.FSD_CLIENT_PREFIX = 'res:/localizationFSD/localization_fsd_'` | `localizationBase.py:57` |
| `LoadLanguageData` -> main, then the language pickles | `:104` |
| `_LoadPickleData` = `blue.ResFile().Read()` then `cPickle.loads` | `:867` |
| `_ReadMainLocalizationData` | `:540` |
| `_ReadLanguageLocalizationData` -> `eveLocalization.LoadMessageData(*obj)` | `:617` |
| `OFFSET_TEXT = 0`, `OFFSET_METADATA = 1`, `OFFSET_TOKENS = 2` | `:35-37` |

(Line numbers from `tools/call_lines.py`, i.e. from `co_lnotab`, not from
uncompyle6 output.)

There is no CCP container, no compression and no header: **the resource is the
pickle**. The index's second size column is the `ResFiles` compressed size, not
anything in the file. **[V]**

### 1.2 It is a protocol-0 pickle, and it is safe to read

Disassembling the opcode stream shows the printable-ASCII protocol-0 dialect
(`(d`, `S'..'`, `V..`, `I..`, `t`, `s`, `p`). **[V]**

Neither `main` nor `en-us` contains a single `GLOBAL` (`c`), `INST` (`i`),
`OBJ` (`o`), `REDUCE` (`R`), `BUILD` (`b`) or `PERSID` (`P`) opcode, so nothing
in them can import or construct anything: every object is a dict, list, tuple,
int, unicode, str or None. **[V]**

A useful side effect, which the `wallet-ledger` lane exploited before this
reader existed: **protocol 0 is ASCII, so the file is greppable without
unpickling at all.** `grep -a 'GM Cash Transfer'` on the resource works, and
`grep -aC2` around a hit shows the `I<id>` opcode that keys it. That is the
right tool for one lookup; the reader is the right tool for anything that needs
the id -> string map, the labels, or the token records.

`tools/loctable.py` uses an
unpickler whose `find_class` raises, and audits for those opcodes before
loading, because that is a property we want to hold regardless of what a future
patch ships.

### 1.3 `localization_fsd_main.pickle` — the index

Dict with four keys. Three are read by `_ReadMainLocalizationData`: **[V]**

- `labels` — `{messageID: {'FullPath': str, 'label': str, 'messageID': int}}`,
  31,872 rows. The client builds `languageLabels['/'.join([FullPath, label])] =
  messageID` (`:577`), falling back to the bare `label` when `FullPath` is
  empty. Key constants are `localization/const.py`:
  `PICKLE_LABELS_FULLPATH = 'FullPath'`, `PICKLE_LABELS_LABEL = 'label'`.
  4,168 distinct `FullPath` values.
- `languages` — `['de','en-us','es','fr','it','ja','ko','ru','zh']`.
- `types` — `{typeName: {languageID: [propertyName,...]}}` per
  `IsValidTypeAndProperty`. **Empty (`{}`) in V24.01.** The real property
  vocabulary is in `localization/propertyHandlers/`. **[V]**

And one the client never reads: `important`, also `{}`. Presumably the
"important name" override set used by `GetImportantByMessageID`. **[I]**

### 1.4 `localization_fsd_<lang>.pickle` — one language's messages

A 2-tuple `(languageID, {messageID: (text, metaData, tokens)})`, splatted into
native code as `eveLocalization.LoadMessageData(*unPickledObject)`. The triple
is confirmed from the other side by `UpdateTextCache` (`:727`), which rebuilds
entries as `(text, metaData, parser._Tokenize(text))`, and by the
`OFFSET_TEXT/METADATA/TOKENS` constants. **[V]**

- **`text`** — the message, with `{...}` substitution tokens and the client's
  pseudo-HTML left in (`<br>`, `<b>`, `<font color=0x...>`,
  `<a href=showinfo:34>`; older strings carry literal `\r\n`).
- **`metaData`** — `None` for all 308,657 en-us messages. **[V]** Read by
  `GetMetaData` via native `GetMetaDataByID`; evidently only ever populated by
  the live-edit `UpdateTextCache` path. **[I]**
- **`tokens`** — the *pre-computed* `parser._Tokenize(text)` output: `None` when
  the text has no `{...}`, else `{tokenSourceText: record}`. 10,782 of 308,657
  en-us messages carry one, against 10,786 whose text contains a `{`. **[V]**

A token record, read from the data: **[V]**

```python
'{[numeric]port, decimalPlaces=0}': {
    'variableName': 'port',        # <- the kwarg name the caller must supply
    'variableType': 9,             # eveLocalization.VARIABLE_TYPE
    'propertyName': None,          # the attribute after the dot, or None
    'args': 512,                   # bitmask of eveLocalization.TOKEN_FLAG
    'kwargs': {'decimalPlaces': 0},
    'conditionalValues': []}
```

`variableType` numbering, derived by grouping every shipped token record by the
`[prefix]` in its own source text — so this table is measured, not guessed
**[V]** — except `4`, which is `characterlist` by elimination against
`parser.VARIABLE_TYPE_CHOOSER` **[I]**:

| n | type | n | type |
| --- | --- | --- | --- |
| 0 | `character` | 6 | `datetime` |
| 1 | `npcorganization` | 7 | `formattedtime` |
| 2 | `item` | 8 | `timeinterval` |
| 3 | `location` | 9 | `numeric` |
| 4 | `characterlist` **[I]** | 10 | `generic` (no `[prefix]`) |
| 5 | `messageid` | | |

Observed `propertyName` values per type, from the data **[V]**: `character` ->
`name`, `gender`, `nameWithPossessive`; `item` -> `name`, `quantityName`,
`nameWithArticle`, `rawName`; `location` -> `name`, `rawName`;
`npcorganization` -> `name`, `nameWithArticle`, `rawName`; `numeric` -> `isk`,
`distance`, `quantity`; `timeinterval` -> `writtenForm`, `shortWrittenForm`,
`writtenFormTwoPart`, `shortForm`.

`args` modifiers come from `parser.ARG_TO_MODIFIER`: `linkify`, `capitalize`,
`uppercase`, `lowercase`, `titlecase`, `useGrouping`. **[V]** The numeric flag
values live in native `eveLocalization.TOKEN_FLAG` and are `unknown`; the two
observed so far are `512` alongside `decimalPlaces` and `320` alongside a
conditional.

**Yes, arguments are formatted — plurals and gendered forms are in the data.**
519 token records carry `conditionalValues`: **[V]**

```
{[numeric]numFighters-> "fighter", "fighters"}        conditionalValues ['fighter','fighters']
{[character]agent.gender -> "his", "her"}             conditionalValues ['his','her']
{[numeric]numSquadrons -> "Full Squadron", "Full Squadrons"}
```

The two arms are data; the *selection* happens in native code from the argument
we supply. For a gendered form the argument is a character id and the client
looks the gender up itself (`propertyName: 'gender'`), so we send an id, not a
pronoun. For a plural the argument is the number. `localization/const.py` also
defines `GENDER_MALE/FEMALE/NEUTER` and
`GENDERS_EXACTLY_ONE_MALE / EXACTLY_ONE_FEMALE / AT_LEAST_ONE_MALE / ALL_FEMALE
/ UNDEFINED`, which is the `characterlist` collective-gender ladder. **[V]**

---

## 2. How the client resolves a string

`localization.Get(messageIDorLabel, languageID=None, **kwargs)` —
`localizationBase.py`; lines `:277`-`:312` are inside `Get`: **[V]**

1. A **string** argument is a *label path* and is looked up in
   `languageLabels` (`:283`) to get the integer message id. A miss logs
   `No label with name %r` and returns `None`; `GetByLabel` (`:338`) turns that
   into the literal `[no label: <path>]`.
2. `kwargs['player']` is defaulted to `session.charid` if the caller did not
   pass it — so second-person and gendered forms key off the **session**.
3. `self._GetByMessageID(messageID, comfylang.cerberus_code(), **kwargs)`
   (`:294`) — that is native `eveLocalization.GetMessageByID`. This is where
   the `{tokens}` are substituted from `kwargs`.
4. A leading `<localized hint="...">` wrapper is stripped, then
   `uiutil.PrepareLocalizationSafeString`.
5. **Fallback.** On `KeyError` (unknown id): `None`, which `GetByMessageID`
   (`:316`) renders as `UNRESOLVED_LOC_MARKER + ' <id>]'`, and
   `UNRESOLVED_LOC_MARKER = '[no messageID:'`
   (`localization/common/util.py`) — so the user sees literally
   `[no messageID: 12345]`. **[V]** On any *other* exception it falls back to
   `eveLocalization.GetRawByMessageID`, i.e. the untokenised raw text, so a
   missing argument degrades to visible `{tokens}` rather than a blank.

### 2.1 Language loading, and what that means for us

`_ReadLocalizationLanguagePickles` (`:814`) on a **client** loads `en-us`
plus the prefs language only; a **server**/**proxy** role loads *every*
language. `_GetPrimaryLanguage()` (`:807`) is hard-coded `'en-us'`. **[V]** We never
send text, so none of this is ours to manage — but it does mean the client has
the full English corpus resident whatever language it is set to.

### 2.2 What the server has to send

**A message id plus the dict of arguments the message's tokens name.** Nothing
else. `tools/loctable.py id <N>` prints the arguments; `tools/loctable.py
tokens` ranks every token form in the corpus (top ones:
`[character]player.name` 2,944, `[location]dungeonLocationID.name` 1,208,
`[character]agentID.name` 615, `[npcOrganization]agentCorpID.name` 437,
`[location]objectiveDestinationID.name` 368).

---

## 3. Is EVE's written content in there? Yes.

### 3.1 Counts (en-us, measured)

| measure | count |
| --- | --- |
| messages in `en-us` | **308,657** |
| characters of text | **29,529,452** |
| messages with a pre-computed token dict | 10,782 |
| messages with `<markup>` | 18,012 |
| multi-line messages | 30,083 |
| label-addressable (id appears in `main`'s `labels`) | **31,831** (10.3%) |
| numeric-id-only (no label at all) | **276,826** (89.7%) |
| labels with no message in `en-us` | 0 |

The **label-addressable 31,831 are the UI chrome** — they are exactly the
strings a `.pyc` names by path. Their top-level prefixes: `UI` 25,292, `Types`
2,302, `Notifications` 976, `Character` 773, `Campaigns` 532, `Map` 531,
`(no path)` 457, `Tooltips` 393, `MarketGroups` 389, `Overview` 78, `Aura` 50,
`PodGuide` 14, `Corporations` 12, `Dungeons` 9, `EULA` 8, `Stations` 8,
`Achievements` 3, `Events` 2, `InfoBubbles` 2. **[V]**

The **numeric-id-only 276,826 are the content**: nothing in the client's
Python can name them, so every one of them is reached through an id held in an
FSD table or sent by the server. Their shape: **[V]**

| | count |
| --- | --- |
| short names (<=40 chars, no token, no markup) | 192,225 |
| >= 400 chars | 18,606 |
| >= 400 chars **and** paragraph markup (`<br>` or `\r\n`) | 14,594 |
| contains `<br>` | 6,041 |
| contains any `{token}` | 5,253 |
| contains `{[character]player.name}` | **2,739** |
| contains `dungeonLocationID` | **1,117** |
| token dict references an agent/mission variable | **4,361** |
| token dict references an `objective*` variable | **717** |

Those last three rows are the strongest single piece of evidence and they are
*data, not a key name*: 4,361 unlabelled messages carry a pre-computed token
record whose `variableName` is one of `player`, `agentID`, `dungeonLocationID`,
`agentCorpID`, `agentFactionID`, `agentStationID`, `agentConstellationID`,
`agentSolarSystemID`, `objectiveTypeID`, `objectiveDestinationID`,
`objectiveLocationID`. A string that interpolates `dungeonLocationID` is agent
or mission prose and can be nothing else.

**Honest limits on these counts.** "Mission text" versus "item description"
versus "dungeon object name" cannot be separated from this file alone, because
the corpus is a flat id -> string map with no type column. The 192,225 short
strings are *names of things* (item names, dungeon object names, NPC names,
blueprint names, mission titles) and the ~18,600 long ones are *prose* (mission
briefings, agent dialogue, item/corp/faction/bloodline descriptions), but which
is which per row needs the FSD tables — `fsd-reader`'s lane. **A lower bound on
agent/mission prose is 4,361 and the true number is larger**, since completion
and failure lines frequently have no token at all ("Unfortunate. Your failure
shows our enemies weakness.").

The id space has three allocation blocks (`messageID >> 19`): 225,584 in
`[0, 524287]`, 81,695 in `[524288, 1048575]`, 1,378 above. They are allocation
epochs, not categories — `Tritanium` is 76482 and `Rifter` is 105320, while the
first id in block 1 is a 2024 ship SKIN. **[V]** Do not read meaning into the id
range.

### 3.2 Samples, with ids, as evidence that the reader works

Every string below came out of `tools/loctable.py`. A handful, chosen to show
one of each family — not a dump.

**Mission briefing / offer text** (id 145165, unlabelled, 1,314 chars):

> So good to see you, `{[character]player.name}`. I've been on a pretty tight
> schedule for the past two weeks, ever since we discovered the strange, and
> increasing, Serpentis activity in the system. [...] Serpentis activity on this
> scale in `{[location]dungeonLocationID.name}` is unheard of [...] The first
> thing I want you to do is undertake a reconnaisance mission for us into an
> area where we have witnessed most of the Serpentis activity.

**Agent completion / failure / decline lines**, contiguous with it:

- 145167 "Watch your back out there, `{[character]player.name}`. And whatever
  you do, don't let them take you prisoner [...]"
- 145168 "You showed some class out there, `{[character]player.name}`. Take your
  pay and report for your next assignment."
- 137915 "Unfortunate. Your failure shows our enemies weakness."
- 137918 "Hmph. I thought you'd jump to take this mission."

**Objectives text** (115759-115767, a contiguous run):

- 115759 "Explore the area, and eliminate any Serpentis vessels that you come
  upon."
- 115761 "Destroy the Workers' Quarters of the Serpentis construction site,
  then report back to your agent. The Workers' Quarters are located in the last
  area of the deadspace complex."
- 115767 "Obtain the hostages from the holding cell, escape using the
  acceleration gate, and deliver them safely to the CONCORD ship."

**Mission titles** (55355-55365): "Next of Kin (1 of 2)", "Courier Gone Missing
(2 of 2)", "King of the Hill (1 of 13)", "The Damsel In Distress" (three ids:
55358-55360), "Cargo Delivery" (five).

**Item / type descriptions**: 83284 and 93841 both carry the Rifter's
description ("The Rifter is a very powerful combat frigate..."); 83097 carries
Veldspar's, including its `<a href=showinfo:34>Tritanium</a>` link.

**NPC corporation descriptions**: 61044 HZO Refinery, 61100 Ammatar Consulate,
61104 Sisters of EVE, 61113 Food Relief — with the matching *names* in a
separate run (61244 "Sarum Family", 61282 "Ammatar Consulate", 61286 "Sisters
of EVE").

**Faction description**: 59588 "The Amarr Empire is the largest and oldest of
the four empires..."

**Bloodline / ancestry descriptions**: 59430-59432 (the Elders, the Unsullied,
the Stasis People).

**Dungeon object names**, in long contiguous runs: "Serpentis Guard Tower",
"Thukker Heavy Missile Battery", "Barren Asteroid", "Gate Beacon", "Portal
Beacon", "Partially Constructed Megathron", "Abandoned Ship Wreck",
"Retrofitted Security Center", "Amarr Mining Station Ruins". The 8,403-id run
at 267679-276081 and the 7,745-id run at 220807-228551 are of this kind.

**`UserError` bodies** — see §4.2; 2,974 message keys, e.g.
`ItemLocked` -> 259667 "Item `{item}` locked.", `JumpAttemptTooSoon` -> 259393
"You have only recently engaged in a clone jump and need to wait for
`{[numeric]hours}` hours and `{[numeric]minutes}` minutes before you can do so
again."

### 3.3 Cross-language sanity check

`de` has 305,231 messages and **does contain** id 145165 (the Serpentis
briefing, in German: "Es tut gut, Sie zu sehen, `{[character]player.name}`...")
and 59588. So CCP really does localise mission prose, and the corpus is the
same id space across languages. `it` has only 7,452 messages — a partial
translation — which explains its 1.7 MB. **[V]**

### 3.4 What this means

The most labour-intensive content in EVE — the written word — is readable
offline, today, with the standard library. We author no mission prose. What is
*not* here is the structure: which briefing belongs to which mission, which
objective to which dungeon, which description to which `typeID`. That is in
`res:/staticdata/*.fsdbinary`, and it is the `fsd-reader` lane's file set.

---

## 4. The join

Everything outside the UI reaches a string through an **integer message id
stored in an FSD table or sent by the server**. That integer is the join key.
There is no secondary key, no string key, no namespace — `messageID` is global
across the whole corpus. **[V]**

### 4.1 Types, groups, categories

`evetypes/localizationUtils.py`: **[V]**

| function | line | behaviour |
| --- | --- | --- |
| `GetLocalizedTypeName(messageID, ...)` | `:21` | `localization.GetImportantByMessageID(messageID)` |
| `GetTypeLocalizationPath(messageID)` | `:35` | `'EVE/Evetypes/Types/Names/{0:02d}'.format(messageID % 100)` |
| `GetLocalizedTypeDescription(messageID, ...)` | `:39` | `localization.GetByMessageID(messageID)` |
| `GetLocalizedGroupName` / `CategoryName` / `TypeListName` | | same, with `EVE/Evetypes/Groups/Names` etc. |

So **`types.fsdbinary`'s name and description columns hold message ids**, and
the `%100` path bucketing is why `main` has 4,168 distinct `FullPath` values.
(The path form is only used by the *offline build* loader
`localizationcache.GetMessage(path, messageID)`; a built client goes straight to
`GetByMessageID`.)

### 4.2 `UserError` — `res:/staticdata/dialogs.static`

`eve/common/script/sys/eveCfg.py:1755`: **[V]**

```python
def _LoadMessagesFromFSD():
    return fsdBinaryLoader.LoadFSDDataForCFG(
        'res:/staticdata/dialogs.static', 'res:/staticdata/dialogs.schema',
        optimize=False)
```

`cfg.messages` is keyed by the **message key string** (`ItemLocked`,
`JumpAttemptTooSoon`, `CantJumpWhileCloaked`) and each row carries `titleID`,
`bodyID`, `dialogType`, `urlAudio`, `urlIcon`, `suppressable`, `closable`.
`cfg.GetMessage(key, dict)` reads `msg.bodyID, msg.titleID` and calls
`localization.GetByMessageID(bodyID, **dict)` (`eveCfg.py:308`, via
`_GetTitleAndTextForMessage` at `:374`). On an unknown key it recurses into the
key `ErrMessageNotFound` (`_GetContentForMissingMessage`, `:361`).
`GetMessageTypeAndText` (`:333`) is the same path with `FormatConvert` applied
to any tuple-valued argument.

**So a `UserError` on the wire is a `dialogs.static` key plus a dict whose keys
are the body message's token `variableName`s.** That is the whole contract.

And the useful accident: this corpus carries the *same table* denormalised as
labels `UI/Messages/<numericID>/<Key>Title` and `UI/Messages/<numericID>/<Key>Body`
— 4,380 labels, **2,974 distinct message keys**. So the error vocabulary can be
enumerated right now, with title and body text and argument names, **without
decoding `dialogs.static` at all**:

```
EVE_CLIENT=.../CCP/EVE tools/analysis.sh python3 tools/loctable.py errors Jump
```

What is *not* in the labels: `dialogType`, `urlIcon`, `urlAudio`,
`suppressable`, `closable`. Those need `dialogs.static`. **[V]**

### 4.3 Missions, agents, objectives

`eve/client/script/ui/station/agents/agents.py:698`, `Agents.ProcessMessage`: **[V]**

An agent or mission message arrives from *us* as a 2-tuple
`(msgInfo, contentID)` and `msgInfo` may be:

- a **string** — used verbatim, no localisation;
- an **int message id** — `localization.GetByMessageID(msgInfo, **msgArgs)`;
- a **`(label, argsDict)` pair** — `localization.GetByLabel(label, **msgArgs)`,
  and any of `missionOfferText`, `missionBriefingText`,
  `missionCompletionText`, `declineMessageText`, `locationString` inside
  `argsDict` is itself recursively resolved the same way (`agents.py:723-724`),
  as is any tuple-valued entry.

`msgArgs` is assembled in `PrimeMessageArguments` (`:681`) from two sources: **[V]**

- `self.agentArgs[agentID] = self.GetAgentArgs(agentID)` — client-side;
- `self.missionArgs[(agentID, contentID)] =
  self.GetAgentMoniker(agentID).GetMissionKeywords(contentID)` (`:691`) — a
  **remote call on the agent bound object** (`eveMoniker.GetAgent(agentID,
  stationID)`, `:299`). This is the call that supplies `dungeonLocationID`,
  `objectiveDestinationID` and friends, and it is *not* in
  `findings/client-calls.tsv` — the census does not follow moniker calls.
  Flagged for `agents-missions-pve`.

On an exception the client renders
`UI/Agents/Dialogue/StandardMission/CorruptBriefing` (plus, for the int case,
the raw untokenised text after a separator) — so a wrong argument dict is
visible and diagnosable rather than silent. **[V]**

`eve/client/script/ui/station/agents/agentDialogueUtil.py` shows the dungeon
objective arm: `objectiveTextBody = sm.GetService('agents').ProcessMessage(
dunData['briefingMessage'], agentID)`, i.e. **the per-dungeon objective briefing
is also a `(msgInfo, contentID)` pair we send.** **[V]**

### 4.4 The join key, stated plainly

> **`messageID` — a plain global integer. `missions`, `objectives`,
> `conversations`, `dialogs`, `types`, `npccharacters` and the rest of
> `res:/staticdata/` reference strings by it, and so does every
> server-originated agent message. There is no other key.**

For `fsd-reader`: any `*.fsdbinary` column that is an int in the ranges
`[1, 524287]`, `[524288, 1048575]` or `[1048576, 1056505]` and whose name ends
in `ID` next to a human-readable concept (`nameID`, `descriptionID`,
`briefingID`, `titleID`, `bodyID`) is a candidate — and
`tools/loctable.py id <N>` resolves it in one call, which makes it a cheap
oracle for *verifying a decoded column*. If a column decodes to ids whose
strings all read like mission briefings, the column is the mission briefing
column. That is the composability both lanes wanted.

---

## 5. Open / `unknown`

- The numeric values of `eveLocalization.TOKEN_FLAG` (the `args` bitmask).
  Observed `512` and `320`; names from `parser.ARG_TO_MODIFIER` are known but
  the mapping is in native code. Does not block anything — the client computes
  the flags itself from the stored token text.
- `variableType 4 = characterlist` is **[I]**, by elimination; no en-us message
  uses it.
- How `main`'s empty `types` and `important` are meant to be populated. Both
  `{}` in V24.01 and neither affects us.
- Whether `conversations.fsdbinary` / `dialogs.fsdbinary` reference these ids
  directly or through another indirection — unanswerable here; `fsd-reader`
  owns those files. The *client-side* `dialogs.static` path (§4.2) is answered.
- Which of the 276,826 unlabelled ids belong to which content family. Needs the
  FSD tables; see §3.1.

---

## 6. Cross-lane verification: the `wallet-ledger` claim

The `wallet-ledger` lane derived `entryTypeNameID = 59792 + entryTypeID` by
grepping the protocol-0 pickle directly (which works — it is ASCII) and asked
for a check. Run through `tools/loctable.py id`, **it holds, and the run's end
is now exact rather than approximate**: **[V]**

| id | text | entryTypeID |
| --- | --- | --- |
| 59792 | `Undefined` | 0 |
| 59793 | `Player Trading` | 1 |
| 59794 | `Market Transaction` | 2 |
| 59795 | `GM Cash Transfer` | 3 |
| 59802 | `Player Donation` | 10 |
| 59905 | `Corporation Account Withdrawal` | 113 |
| 59910 | `Battle Loss Reward` | 118 |
| 59911 | `Modify AUR` | 119 |
| 59912 | `Respec payment` | 120 |
| 59913 | `Play`, labelled `UI/Jukebox/Play` | — not an entry type |

So the contiguous accounting-entry-name run is exactly **59792..59912, 121 ids,
entryTypeID 0..120**, and `59913` is an unrelated labelled UI string. The lane's
warning that this does not extrapolate to the 170+/190+ ref types is correct for
a concrete reason: **the run physically ends.** Treat the offset as a measured
allocation, not a formula — immediately *below* it, 59737..59791 is a second,
gappier block of entry-type **descriptions** ("Installation and runtime cost for
a manufacturing job", "Sales tax paid to the SCC for any transaction") mixed
with a set of `DUST - ...` names, and it has no clean offset against
`entryTypeID`. The real mapping is two columns of
`accountingentrytypes.fsdbinary`.

**Read, and it settles this** (commit `3690328`): `entryTypeID` at +0 and
`entryTypeNameID` at +32 of the same row, 218 rows. The message block
59792..59912 is contiguous but spans **three disjoint entry-type runs**, which
is why no single offset works past 108 — 59911/59912 belong to entry types
11001/11002, not to 119/120. Inverting the formula on message ids to recover
entry-type ids is circular and gives the wrong answer; the table is the
oracle.

### Message id 234508 — settled

It exists, it is **labelled**, and that is the more useful finding:

```
234508  UI/Generic/FormatReference/playerDonation
        "{name1} deposited cash into {name2}'s account"
        arguments: name1, name2
```

The whole wallet-journal description family is label-addressable under
**`UI/Generic/FormatReference/<refTypeName>` — 183 labels**, keyed by the ref
type's camelCase *name*, not by its numeric id. That covers the high ref types
the numeric run does not reach, so the wallet lane does not need the numeric
mapping for the journal text at all. Two to note: `undefinedType` (234498) is
`"(undefined reftype={type}, o1={o1}, o2={o2}, arg1={arg})"` and
`unknowenJournalReference` (234627, CCP's spelling) is
`"(unknown reference {ID}, {o1}, {o2}, {arg})"` — those are the strings a
player sees when we send a ref type the client cannot name, so they are a
useful tell in a screenshot. **[V]**

Enumerate it with:

```
EVE_CLIENT=.../CCP/EVE tools/analysis.sh python3 tools/loctable.py \
    labels UI/Generic/FormatReference --limit 500
```
