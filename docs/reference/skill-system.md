# The skill system, server side

Client build **V24.01 (3619879)**. This is the complete server-side spec for
skills: what the client asks, what each answer must look like, what it does
with it, and in what order to build it. It is the fifth and last step of
round 2 in [`docs/archive/plan-docked-subsystems.md`](../archive/plan-docked-subsystems.md), and it is
what that step gets implemented from.

Written because the owner's test said **"nothing bought"**, and earlier
*"I believe the whole skill system server-sided is non functional"*. Both are
correct, and §4 says exactly why: every answer we give today is honest, the
purchase reaches us, and the one thing that makes a bought skill *exist* on the
client is a push we do not send.

Confidence markers as elsewhere in `docs/`: **[V]** read from this build's
bytecode or from a capture, **[I]** inferred, **unknown** where it is not
settled and guessing would read like a fact.

**Every line number here came from `co_lnotab`** — `tools/call_lines.py` or a
`tools/pyc27.py` disassembly — or from `findings/client-calls.tsv`'s
`call_site` column. None came from `grep -n` over `uncompyle6` output; see
[`docs/guides/debugging-the-client.md`](../guides/debugging-the-client.md) §4 for why that matters
and §"Tooling traps" for the const-index trap, which bit this document twice
(`skillsvc.__notifyevents__` and `SkillQueueService.__notifyevents__` both
decompile as lists of small integers and are really lists of event-name
strings; §7 has the real ones, read out of the class body's const table).

---

## 1. Where skills live: **the dedicated call, not the item store**

The question was whether this client learns the character's skills from the
item store (so `charMgr.ListIncludingContainers` / `invbroker` rows plus godma
attributes would carry them) or from a dedicated skill call. The answer for
V24.01 is **a dedicated call, exclusively — and the item-store route is
actively closed.** This is a refutation, not a confirmation, and it decides
the whole shape of the work: **skills are a subsystem beside the item store,
not a feature of it.**

### The ids exist, and three of them matter

| name | value | source **[V]** |
|---|---|---|
| `flagSkill` | **7** | `inventorycommon/const.py:118` |
| `flagSkillInTraining` | **61** | `inventorycommon/const.py:152` |
| `categorySkill` | **16** | `inventorycommon/const.py:357` |
| `groupFakeSkills` | 505 | `inventorycommon/const.py:1237` |
| `attributeSkillLevel` | **280** | `dogma/const.py:984` |
| `attributeSkillPoints` | **276** | `dogma/const.py:985` |
| `attributeSkillTimeConstant` ("rank") | **275** | `dogma/const.py:987` |
| `attributeSkillIsObsolete` | 2450 | `dogma/const.py:983` |
| `MAX_SKILL_LEVEL` | 5 | `characterskills/const.py:3` |
| `SKILL_POINT_MULTIPLIER` | 250 | `characterskills/const.py:4` |

So the *schema* of the real server is the folklore one: a skill is an item
owned by the character at flag 7 (61 while training) carrying dogma
attributes 280 and 276, with 275 as the type-level rank.

### But godma refuses to hold it **[V]**

`godma.StateManager.UpdateItem` (`eve/client/script/environment/godma.py:1858`)
opens with a category filter at **lines 1874–1886**:

```python
if itemCategoryID not in (categoryEntity, categoryStarbase,
        categorySovereigntyStructure, categoryShip, categoryModule,
        categoryCharge, categoryDrone, categorySubSystem, categoryFighter,
        categoryStructure, categoryStructureModule) \
        and itemGroupID != groupCharacter:
    self.godma.LogTraceback('Godma location received unexpected and unwanted item')
    return
```

`categorySkill` (16) is **not in that tuple**, and a skill's group is not
`groupCharacter`. A skill row delivered through `dogmaIM.GetAllInfo`'s
`charInfo` or through `OnGodmaPrimeItem` is logged and discarded. It never
enters `invitems`, so it never gets a `SkillLevelAttribute`, so nothing in
the client can read attribute 280 for it.

(The filter is read from `LOAD_ATTR` names, not from a const-table list, so
the decompiler's rendering of it is trustworthy. Verified against the raw
disassembly anyway.)

`dogma/attributes/skillLevelAttribute.py`'s `SkillLevelAttribute` — which
*derives* level from attribute 276 and rank 275 — is therefore unreachable for
a player's own skills on this build. It and `dogma.attributes.SkillLevelAttribute`
in `blue.marshal.globalsWhitelist` are **[I]** vestigial, or used only on the
server's own dogma instance.

### Where the flag *is* still used in the client **[V]**

Two places, neither of which reads a skill level:

- `godma.OnItemChange` (`godma.py:1507`) — a flag change to `flagSkill`,
  `flagSkillInTraining` or `flagBooster` is *exempted* from the "flag changed,
  drop the item" branch. Defensive only; the item is dropped by the category
  filter anyway.
- `godma.LocateItem` (`godma.py:2088`) — finds an item by `typeID` **and**
  `flagID in (flagSkill, flagSkillInTraining)`. A lookup helper; it reads
  `invitems`, which never contains skills, so it is dead for skills too.

### What *does* live in the item store: the skillbook **[V]**

A **skillbook you have not injected yet** is an ordinary hangar item, category
16, `flagHangar`. It reaches the client through `invCache` / `invbroker`
(which have no category filter) and is what the right-click *Inject Skill*
menu operates on, by `itemID` — see §5. So the item store carries
*uninjected* skillbooks; the skill *handler* carries *injected* skills; and
nothing joins the two on the client.

### Verdict

- The client's entire model of trained skills is `skillsvc`'s three caches,
  and all three are filled from `skillMgr2.GetMySkillHandler()`. **[V]**
- `charMgr.ListIncludingContainers` / `invbroker` rows with flag 7 would be
  **invisible**: no window lists flag 7, and godma drops them. **[I]** (that
  no window lists them; **[V]** that godma drops them).
- Server-side we are free to *store* skills as items at flag 7 — and should,
  because `docs/reference/world-state.md` §2 item 7 already plans that and because skill
  extraction/injection move real items — but the **client contract is the
  handler's dicts**, and that is what must be built.

---

## 2. The call surface

One service method and a bound object. `skillsvc.GetSkillHandler`
(`skillsvc.py:152`) is:

```python
def GetSkillHandler(self):                                          # 152
    if not self.skillHandler:                                       # 153
        self.skillHandler = session.ConnectToRemoteService('skillMgr2').GetMySkillHandler()  # 155
    return self.skillHandler                                        # 156
```

`skillMgr2.GetMySkillHandler()` takes **no arguments** and must return a bound
object. We already serve it as `skillMgr2::skillHandler`
(`service.rs:636`). The guard is truthiness and a bound-object wrapper is
truthy, so it latches.

Every method the client then calls on that object — the complete surface, from
a sweep of all seven files that reach `GetSkillHandler()`:

| method | args | shape expected | consumer | served today |
|---|---|---|---|---|
| `GetSkills` | 0 | **dict** `{typeID: entry}` | `skillsvc.py:161` → `self.myskills` | `{}` |
| `GetAllSkills` | 0 | **dict** `{typeID: entry}` | `skillsvc.py:166` → `self.mySkillsIncludingLapsed` | `{}` |
| `GetAttributes` | 0 | **dict** `{attributeID: float}`, all five, non-zero | `skillsvc.py:252` | five 20.0s |
| `GetImplants` | 0 | dict keyed by implant slot | `skillsvc.py:1131` | `{}` |
| `GetBoosters` | 0 | dict `{boosterTypeID: mutable obj}` | `skillsvc.py:1126` | `{}` |
| `GetFreeSkillPoints` | 0 | **int** | `skillsvc.py:1003`, `:1152` | the pool (`free_sp::of`) |
| `GetSkillPoints` | 0 | **int** (trained SP total) | `skillsvc.py:1151` | unanswered |
| `GetRespecInfo` | 0 | dict `nextTimedRespec`, `freeRespecs` | `skillsvc.py:927` | both keys |
| `RespecCharacter` | 1 | discarded (`POP_TOP`) | `neocom/attributes.py:475` | unanswered |
| `ApplyFreeSkillPoints` | 2 | **int** = new free points | `skillsvc.py:1045` | the new pool; refuses in the client's labels (`free_sp::apply_one`) |
| `GetFreeSkillPointsAppliedToQueue` | 0 | dict `{typeID: points}` | `skillQueueSvc.py:1004`, `applySkillPointsWindow.py:181` | the preview (`free_sp::applied_to_queue`) |
| `ApplyFreeSkillPointsToQueue` | 0 | discarded | `skillQueueSvc.py:1006` | spends on the queue in order, then `OnFreeSkillPointsChanged` |
| `GetFreeSkillPointsAppliedToSkills` | 1 | dict `{typeID: points}` | `skillQueueSvc.py:1010`, `applySkillPointsWindow.py:182` | the preview for a `(typeID, level)` list |
| `ApplyFreeSkillPointsToSkills` | 1 | discarded | `skillQueueSvc.py:1012` | spends on the list, then `OnFreeSkillPointsChanged` |
| `GetSkillHistory` | 1 (`maxresults`) | **list** of rows, §6 | `skillsvc.py:400` | `[]` |
| `CheckAndSendNotifications` | 0 | **`None` is correct** | `skillHistoryProvider.py`, discarded | `None` |
| `GetSkillChangesForISIS` | 0 | list of `(typeID, pointChange)` | `skillsvc.py:423` | `[]` |
| **`PurchaseSkills`** | 1 (list of typeIDs) | **flat list of typeIDs** | `characterskills/client/purchase.py:97` | `[]` |
| `GetSkillQueueAndFreePoints` | 0 | **2-tuple** `(queue, freePoints)` | `skillQueueSvc.py:159` | `(queue, pool)` |
| `SaveNewQueue` | 1 + kwarg `activate` | discarded; **raises** matter | `skillQueueSvc.py:227` | unanswered |
| `AbortTraining` | 0 | discarded | `skillsvc.py:918` | unanswered |
| `CharStartTrainingSkill` | 2 (`itemID`, `locationID`) | discarded — **dead**, see §5 | `skillsvc.py:890` | unanswered |
| `ExtractSkills` | 3 (`skills`, `itemID`, token) | discarded | `skillsvc.py:1137` | unanswered |
| `InjectSkillpoints` | 2 (`itemID`, `quantity`) | discarded | `skillsvc.py:1140` | unanswered |
| `CheckInjectionConstraints` | 2 | discarded; **raises** are the point | `skillsvc.py:1145` | unanswered |
| `GetDiminishedSpFromInjectors` | 3 | number | `skillsvc.py:1165` | unanswered |
| `SplitSkillInjector` | 2 | discarded | `skillsvc.py:1174` | unanswered |
| `CombineSkillInjector` | 2 | discarded | `skillsvc.py:1181` | unanswered |
| `GetSkillBundleInfo` | 1 | `UNPACK_SEQUENCE 2` | `skillsvc.py:1261` → `ui/crate/lootItem.py` | unanswered |

One neighbouring service, not on the handler: **`alphaInjectorMgr.GetNextAvailableInjection()`**,
0 args, `skillsvc.py:1168`. Its result goes straight into
`max(0, x - blue.os.GetWallclockTime())` (`skillsvc.py:1171`), so `None` is a
`TypeError`. It must be a **number** — a FILETIME; see §6's trap list.

### A census correction worth carrying

`findings/client-calls.tsv` has **23** `skillMgr2` rows, which cover
`GetMySkillHandler` plus **20** of the handler's 29 methods (`GetFreeSkillPoints`
appears twice, for two call sites). **Nine are missing:**

```text
PurchaseSkills                      characterskills/client/purchase.py
SaveNewQueue                        skillQueueSvc.py
GetSkillQueueAndFreePoints          skillQueueSvc.py
GetFreeSkillPointsAppliedToQueue    skillQueueSvc.py, applySkillPointsWindow.py
ApplyFreeSkillPointsToQueue         skillQueueSvc.py
GetFreeSkillPointsAppliedToSkills   skillQueueSvc.py, applySkillPointsWindow.py
ApplyFreeSkillPointsToSkills        skillQueueSvc.py
CheckAndSendNotifications           notifications/.../skillHistoryProvider.py
RespecCharacter                     eve/.../neocom/attributes.py
```

All nine for the same reason: the census's bound-object tracker only follows
`GetSkillHandler()` inside `skillsvc.py`, and every one of those is reached
from a *different* file through the same accessor. **The
accessor-in-another-file pattern is a general blind spot in that census**, not
a skill-specific one — `PurchaseSkills`, the single most important method in
this document, is one of the nine. The grep that finds the rest across the
tree is for the accessor rather than the service:
`grep -ra -l 'GetSkillHandler\|GetDogmaLM\|GetLM(' out/pyc/`.

### Not a machoNet call, despite appearances

`operations/common/skipconditions/skills.py` calls
`sm.GetService('skillMgr2').GetSkillHandler(character_id)` and then
`GetEstimatedSkillPoints()`, `GetSkillLevel(typeID)`, `GetFreeSkillPoints()`.
That is **server-side code shipped in the client tree** (`GetService`, not
`RemoteSvc`/`ConnectToRemoteService`), i.e. CCP's own node-side interface. It
is good evidence for what the real handler looks like internally and it is
**not** something we have to serve. **[V]**

---

## 3. The trained-skill read path

### Two calls, two caches, and a naming trap that will cost someone a day

```python
def RefreshMySkills(self):                                           # 158
    self.myskills = self.GetSkillHandler().GetSkills()               # 161

def RefreshMySkillsIncludingLapsed(self):                            # 163
    self.mySkillsIncludingLapsed = self.GetSkillHandler().GetAllSkills()  # 166
```

- `GetSkills()` → the character's **injected, non-lapsed** skills.
- `GetAllSkills()` → the same **plus lapsed ones** (alpha-state skills the
  character owns but whose points are withheld: `trainedSkillLevel is None`,
  `trainedSkillPoints is None`).

**`GetAllSkills` is not "every skill in the game."** The client has its own
local method with that exact name — `skillsvc.GetAllSkills` at
`skillsvc.py:342` — which enumerates `evetypes.GetTypeIDsByCategory(16)` and
builds `CharacterSkillEntry(typeID, 0, 0, rank, None)` for each. The remote
one is the character's own set. Returning all ~500 published skills from the
remote call would make `IsSkillInjected` true for every skill in the game
(`skillsvc.py:337`, the read at `:340`: `skill and skill.trainedSkillLevel is not None`), which
silently disables every Buy button and every "train required skills".
`GetSkills() ⊆ GetAllSkills()`. **[V]**

Both caches are guarded `is None` and the result is stored **directly** with
no comprehension in between (`skillsvc.py:242`, `:285`, `:290`), so `{}`
latches correctly and `None` would be the poisoned-cache live lock
(`docs/guides/debugging-the-client.md` §4, the `contractProxy` variant).

### The row shape: `CharacterSkillEntry`

`characterskills/common/character_skill_entry.py`. Five constructor fields,
stored under **underscore-prefixed** names, exposed as properties:

| private field | property | type | notes |
|---|---|---|---|
| `_typeID` | `typeID` | int | the skill's type id |
| `_trainedSkillLevel` | `trainedSkillLevel` | int 0–5, or `None` when lapsed | |
| `_skillPoints` | **`trainedSkillPoints`** | int, or `None` when lapsed | `skillPoints` itself **raises** |
| `_skillRank` | `skillRank` | int ≥ **1** | `attributeSkillTimeConstant` (275) |
| `_virtualSkillLevel` | `virtualSkillLevel` | `None` normally | expert-system level |

plus a derived `effectiveSkillLevel = max(trainedSkillLevel, virtualSkillLevel)`.

Two properties **raise `DeprecationWarning`** on access, which is an
`Exception` subclass in Python 2, so they raise for real: `skillLevel` and
`skillPoints`.

**The class is in `blue.marshal.globalsWhitelist`** — the whitelist text in
`whitelist.pyc` lists
`characterskills.common.character_skill_entry.CharacterSkillEntry` verbatim
**[V]** — so the real server marshals the real class and we can too. It is a
new-style class, so it goes on the wire as a **`0x23` ObjectEx**
(`docs/reference/object-wrapper.md` §3), not a `0x17`:

```text
PyValue::ObjectEx {
  form: ObjectForm::New,                       // tag 0x23
  header: Tuple[ Tuple[ Global(b"characterskills.common.character_skill_entry.CharacterSkillEntry") ],
                 Dict{ "_typeID": .., "_trainedSkillLevel": .., "_skillPoints": ..,
                       "_skillRank": .., "_virtualSkillLevel": .. } ],
  list: [], dict: [],
}
```

`cls.__new__(cls)` then `inst.__dict__.update(state)` — `__init__` is never
called, which is why the state keys are the private names. Both `PyValue::ObjectEx`
and `PyValue::Global` already exist in `eve-marshal` (`value.rs:101`, `:129`)
and `0x22` is already used for `blue.DBRowDescriptor`, so this is not new
encoder work.

**It must be a mutable instance, not a `DBRow`.** `_UpdateMySkillsAndReturnLevelsTrained`
does `skillInfo.timeStamp = timeStamp` (`skillsvc.py:224`, `:233`) — a plain
`setattr` on a field the class does not declare. An instance `__dict__` takes
it; a `blue.DBRow` rejects it. The class has no `__slots__`, so this works.

**The fallback, if the ObjectEx proves awkward: a `utillib.KeyVal`** carrying
`typeID`, `trainedSkillLevel`, `trainedSkillPoints`, `skillRank`,
`virtualSkillLevel` **and** `effectiveSkillLevel` (which is a property on the
real class and would have to be materialised). The only thing a KeyVal breaks
is `GetCopyWithNewSkillPoints`, called at exactly one site —
`skillsvc.py:232`, in the `virtualSkillLevel is not None` branch of the push
handler, which we never need to take. Prefer the real class; the KeyVal is a
documented escape hatch, not a plan.

### `skillRank` must be ≥ 1, and the reason is a `ZeroDivisionError`

`characterskills/util.GetSPForLevelRaw(rank, level)` is
`ceil(rank * 250 * 2 ** (2.5 * (level - 1)))` (`characterskills/util.py:47`,
constants at `characterskills/const.py:3`–`4`), and `GetLevelProgress`
(`util.py:15`) divides by `nextLevelPoints - baseLevelPoints`. With
`skillRank == 0` both are 0 and the skill-queue entry's progress bar raises.
`skillRank` is also read by `SkillPointsCurrentLevel` (`skillsvc.py:323`),
`SkillpointsNextLevel` (`:331`), `ApplyFreeSkillPoints` (`:1034`),
`characterskills/queue.GetQueueEntry` and `SkillHistoryRow.initLevels`.

The client *also* computes rank itself, from FSD, in
`skillsvc.GetSkillRank` → `godma.GetTypeAttribute(typeID, 275)`
(`skillsvc.py:860`). So the value we send is compared against FSD's in
practice, and a mismatch shows up as two different training times for the same
skill in two panels. Reading `*.fsdbinary` server side is still the open
question in `docs/archive/recon-v24.01.md`; until then a **curated rank table for the
skills the sandbox grants, defaulting to 1**, is the honest stopgap and the
inconsistency is cosmetic. **[I]** on "cosmetic".

### What reads these caches, and with what

Everything. The thirty-odd accessors on `skillsvc` all funnel through
`GetSkill` / `GetSkillIncludingLapsed` / `GetSkills` / `GetSkillsIncludingLapsed`.
The ones that decide visible behaviour:

| caller | reads | what it decides |
|---|---|---|
| `IsSkillInjected` (`:337`, read at `:340`) | `trainedSkillLevel is not None` | whether a skill is Buy-able at all |
| `MySkillLevel` (`:294`) | `trainedSkillLevel` | every prerequisite check, every fitting check |
| `MyEffectiveSkillLevelsByID` (`:314`) | `.iteritems()` + `effectiveSkillLevel` | `marketbase.GetMySkills` — the market window |
| `GetTotalSkillPointsForCharacter` (`:847`) | `trainedSkillPoints or 0` summed, + `GetFreeSkillPoints()` | the character sheet's SP total, behind an `AutoTimer(2000)` |
| `GetMissingSkillBooksFromList` (`:629`) | `not skill or trainedSkillPoints is None` | which skillbooks the Buy dialog offers |
| `skillQueueSvc.CheckCanInsertSkillAtPosition` (`:256`) | `mySkill is None` → `UserError('QueueSkillNotUploaded')` | whether a skill can enter the queue |
| `skillQueuePanelNew.AddSkillsThroughOtherEntry` (`:885`) | `mySkills.get(skillID)` is None, at `:886` | whether the **Buy** overlay opens |

### One known client bug on this path **[V]**, not ours

`skillsvc.GetSkillGroups` (`:436`) does
`ownSkillPointsByGroupID[groupID] += skill.skillPoints` at **`skillsvc.py:459`**,
and `skillPoints` is the property that raises `DeprecationWarning`. It will
raise for any character with at least one skill, whatever the server sends,
because the client's own local `GetAllSkills` builds the same class. Its only
callers are the legacy skill-tree views (`ui/skillTree/*`) and
`devtools/dna.py`. Expect a traceback if the owner opens the old skill tree,
and do not go looking for a shape of ours to blame. Marked **[V]** on the
mechanism, **[I]** on "dead code".

---

## 4. The purchase path, end to end

### The flow, from the Buy click

1. The player drops a skill into the queue in the Skills panel.
   `skillQueuePanelNew.AddSkillsThroughOtherEntry` (`:885`) reads
   `skills.GetSkillsIncludingLapsed()` at **`:886`** and, if the skill is not
   there, calls `HandleMissingSkill` (**`:889`**, defined at `:906`), which
   calls `show_skill_purchase_overlay` at **`:910`**.
2. The overlay (`skillPurchaseOverlay.py`) prices the skill **client-side**:
   `characterskills.common.get_direct_purchase_price` =
   `evetypes.GetBasePrice(typeID) * (1.0 + 0.3)`
   (`characterskills/common/purchase.py:38`–`39`, `DIRECT_PURCHASE_TAX_RATE`
   at `:9`). Availability is also client-side:
   `evetypes.IsSkillAvailableForPurchase` is "published **and** in FSD type
   list `TYPE_LIST_SKILLS_AVAILABLE_FOR_PURCHASE`" (`evetypes/__init__.py:705`,
   `:708`). **The client decides what is buyable and at what price; the server
   is never asked.**
3. **Buy** → `skills.PurchaseSkills([typeID], confirm=False)`
   (`skillPurchaseOverlay.py:52`).
   **Buy and train** → the same at `:61`, then
   `skillqueue.AddSkillToQueue(typeID, 1, position=...)` at **`:62`**.
4. `skillsvc.PurchaseSkills` (`:1238`) builds a
   `characterskills.client.SkillPurchaseContext` (`:1239`) and calls
   `characterskills.client.purchase_skills` (`:1240`).
5. `purchase_skills` (`characterskills/client/purchase.py`, def `:22`):
   - `context.check_skill_purchase_enabled()` (**`:33`**) — reads
     `globalConfig.IsSkillPurchaseEnabled(machoNet)`, i.e.
     `machoNet.GetGlobalConfig()['SkillPurchaseEnabled']`, **client-side**,
     default True. This is the honest off switch and it is a push we do not
     send (`InitClientGlobalConfig`); see §7.
   - with `confirm=False`: `context.validate_purchase(ids)` (**`:42`**) then
     `context.send_skill_purchase_request(ids)` (**`:43`**), and returns
     `context.skills_purchased or []` (**`:44`**).
   - with `confirm=True`: `prevalidate_purchase` then the modal
     `MissingSkillbooksWnd`, whose **Buy and Inject** button
     (`missingSkillbooksWnd.py:362`) calls
     `context.purchase_available_skills()` (`purchase.py:64`).
   - `validate` is entirely client-side (`characterskills/purchase.py`,
     `SkillPurchaseValidator.validate`): no duplicates, all are skills, all
     available for purchase, none already injected, and
     **`wallet.GetWealth()` covers the total** (`purchase.py:132`).
6. **The one wire call:**

   ```python
   def send_skill_purchase_request(self, skill_type_ids):           # 95
       skills_purchased = self.skill_service.GetSkillHandler().PurchaseSkills(skill_type_ids)  # 97
       self.skills_purchased.extend(skills_purchased)               # 98
       return True
   ```

   One positional argument, a list of skill type ids. The capture has it:
   `PurchaseSkills([3340, 20342, 3327, 20527]) -> None`. `list.extend(None)`
   is the `TypeError` that made the window throw.

### What the return value must be

A **flat list of the skill type ids actually purchased** — not an echo of the
request, not a bool, not a rowset. The authority is CCP's own shared logic,
`characterskills/purchase.purchase_skills`:

```python
def purchase_skills(context, skill_type_ids):
    skill_type_ids = list(skill_type_ids)
    context.validate(skill_type_ids)
    skills_to_inject = []
    try:
        for skill_type_id in skill_type_ids:
            context.pay_for_skill(skill_type_id)
            skills_to_inject.append(skill_type_id)
    finally:
        if skills_to_inject:
            context.inject_skills(skills_to_inject)
    return skills_to_inject
```

The `try/finally` is the contract: a purchase that runs out of ISK halfway
still injects and still **returns the part that succeeded**. So a partial
answer is legal and expected, and the server must inject exactly what it
returns.

### `seen`, and why no answer alone can satisfy it

```python
def WaitUntilSkillsAreAvailable(self, skillTypeIDs):          # 1242 (body 1250)
    skill_type_ids = set(skillTypeIDs)                        # 1250
    seen = set()                                              # 1251
    while skill_type_ids != seen:                             # 1252
        blue.pyos.BeNice()                                    # 1253
        for skill_type_id in skill_type_ids:                  # 1254
            if self.GetSkillIncludingLapsed(skill_type_id) is None:   # 1255
                break                                         # 1256
            seen.add(skill_type_id)                           # 1257
    sm.ScatterEvent('OnSkillsAvailable', skillTypeIDs)        # 1258
```

**`seen` is populated from `self.mySkillsIncludingLapsed`, and from nothing
else.** `GetSkillIncludingLapsed` is called at line 1255 **without `renew`**
(`skillsvc.py:284`):

```python
def GetSkillIncludingLapsed(self, skillTypeID, renew=0):      # 284
    if self.mySkillsIncludingLapsed is None or renew:         # 285
        self.RefreshMySkillsIncludingLapsed()                 # 286
    return self.mySkillsIncludingLapsed.get(skillTypeID, None) # 287
```

The cache is already non-`None` by the time the purchase runs (the Skills
panel filled it at `skillQueuePanelNew.py:886` to decide the skill was
missing). So the loop issues **no further service call**. It is not a re-ask;
it is a spin on a client-side dict.

**Therefore: it is neither a re-ask nor a push of the answer. It can only be
satisfied by state changing elsewhere on the client,** and there are exactly
two levers, both server pushes:

| lever | event | what it does | cost |
|---|---|---|---|
| **A** (preferred) | `OnServerSkillsChanged(skillInfos, event, timeStamp)` | `_UpdateMySkillsAndReturnLevelsTrained` writes `self.myskills[id] = skillInfo` (`skillsvc.py:225`) and `self.mySkillsIncludingLapsed[id] = skillInfo` (**`:226`**) | needs the entry shape of §3 on the wire |
| **B** (cheap) | `OnSkillForcedRefresh()` (0 args) | `uthread.Pool(ForceRefresh)` (`skillsvc.py:1097`) → `ForceRefresh` (`:1099`) → `Reset()` (**`:1100`**), which sets `mySkillsIncludingLapsed = None` (**`:137`**), so line 1255's next read re-asks `GetAllSkills` | needs only a correct `GetAllSkills` |

Lever B turns the loop into a genuine, bounded re-ask: one `GetAllSkills`
round-trip and the loop exits. Lever A needs no round-trip at all.

So **an honest, terminating `PurchaseSkills` is: debit the wallet, create the
skill, answer the list of ids that got through, and in the same reply push
`OnServerSkillsChanged` (or `OnSkillForcedRefresh`) for exactly those ids.**
The answer and the push are one atomic deliverable; shipping the truthful list
without the push converts today's silent no-op into a visible hang, which is
strictly worse. That is why `character.rs:815` answers `[]` today and why that
was the right call.

Mechanically the push rides the same flush: `Reply.pushes` with
`Duration::ZERO` is appended after `reply.payloads` in one `send_all`
(`conn.rs:146`–`155`), and `blue.pyos.BeNice()` yields, so the notification's
`uthread.worker` runs while the purchase tasklet spins. **[I]** that one
`BeNice` cycle is enough; the lever if it is not is to let an arm put a push
*before* its payload, which `Reply` cannot express today.

### The third caller shape, which is the dangerous one

Three call sites consume `skillsvc.PurchaseSkills`'s return, and they do not
agree on what they then wait for:

| site | waits on | consequence of a partial answer |
|---|---|---|
| `skillBtns.BuyAndTrainButton.ClickFunc` (`:341`, wait at **`:343`**) | `WaitUntilSkillsAreAvailable(skills_purchased)` — the **returned subset** | correct; the loop asks only for what we said we sold |
| `fittingScreen/skillRequirements.BuyAndTrain` (`:313`, wait at **`:316`**) | same | correct |
| `skills/client/util.buy_missing_skills` (`:28`, wait at **`:31`**) | `WaitUntilSkillsAreAvailable(missing_skill_type_ids)` — the **full request** | **hangs forever** on a partial answer: the ids we declined can never appear |
| `skillPurchaseOverlay.buy_and_train_skill` (`:61`) | nothing; goes straight to `AddSkillToQueue` (**`:62`**) | `UserError('QueueSkillNotUploaded')` from `CheckCanInsertSkillAtPosition` (`skillQueueSvc.py:256`) if the push has not landed |

**Rule that follows:** `PurchaseSkills` must be **all-or-nothing** for us.
`buy_missing_skills` waits on the request, not on the answer, so a partial
answer there is the unbounded spin. Validate everything first, then either
sell all of them and return all of them, or sell none and return `[]`. The
`[]` path is handled: `buy_missing_skills` returns `False` and skips
`AddSkillsToQueue` (`skills/client/util.py:21`).

### What the player sees today, and why "nothing bought" is the exact symptom

`[]` is falsy. `skillPurchaseOverlay.buy_skill` discards the result entirely
and closes; `MissingSkillbooksWnd._OnBuyAndInjectButton` sets `ID_YES` and
closes. No skill arrives. And because `mySkillsIncludingLapsed` is cached for
the whole session with no invalidation on this path, **reopening the Skills
window does not help either** — only a relog. "Nothing bought" is precisely
what the code does.

We do not raise instead. The only handlers on this path are
`SkillPurchaseValidator.validate`'s (which never sees the wire call) and
`handle_skill_purchase_error`, reached from `validate` only; a `UserError`
from `PurchaseSkills` escapes through `skills.client.util.buy_missing_skills`
and breaks the fitting screen's "train required skills" as well.

---

## 5. Injection, and how it differs from purchase

### Two different services, two different kinds of id

| | purchase | injection |
|---|---|---|
| call | `skillMgr2::skillHandler.PurchaseSkills(typeIDs)` | `dogmaIM.InjectSkillIntoBrain([itemIDs])` |
| ids | **type** ids | **item** ids |
| cost | ISK (basePrice × 1.3, client-computed) | the skillbook item |
| consumes an item | **no** — it creates one from nothing | **yes** |
| return | list of typeIDs | discarded (`POP_TOP`) |
| raises | nothing the client catches | `UserError`, re-raised by the client |

### The wire form **[V]**

```python
def InjectSkillIntoBrain(self, skillX):                              # 892
    skillIDList = [skill.itemID for skill in skillX]                 # 897
    if not skillIDList: return
    for skill in skillX:
        if not evetypes.IsPublished(skill.typeID):
            raise UserError('ItemNotASkill', {'skillName': skill.typeID})
    try:
        self.godma.GetDogmaLM().InjectSkillIntoBrain(skillIDList)    # 906
    except UserError as e:
        raise
    else:
        for skill in skillX:
            sm.GetService('notificationSvc').MakeAndScatterNotification(
                notificationTypeSkillInjected, data={'typeID': skill.typeID})
        PlaySound(uiconst.SOUND_ADD_OR_USE)
```

`godma.GetDogmaLM()` is `eveMoniker.CharGetDogmaLocation()` (`godma.py:1330`),
which binds **`dogmaIM`** — `bindParams` `(solarsystemID, groupSolarSystem)` or
`(stationID, groupStation)`, per `docs/reference/bound-objects.md` §2.2. `InjectSkillIntoBrain`
is already in that doc's `dogmaIM` method table with arity 1. One argument: a
**list of item ids**.

Entry points **[V]**:

- The right-click menu: `menuSvcExtras/invItemFunctions.InjectSkillIntoBrain`
  (`:96`) → `sm.StartService('skills').InjectSkillIntoBrain(invItems)` (`:97`).
- The right-click **Train Now**: `invItemFunctions.TrainNow` (`:87`) — and this
  one is worth reading in full, because it tells us our deadline:

  ```python
  def TrainNow(invItems):                                      # 87
      if len(invItems) > 1:
          prompt_player_fsd_dialog('TrainMoreTheOne'); return
      InjectSkillIntoBrain(invItems)                           # 91
      blue.pyos.synchro.SleepWallclock(500)                     # 92
      sm.GetService('skillqueue').TrainSkillNow(invItems[0].typeID, 1)  # 93
  ```

  **CCP's own code sleeps 500 ms waiting for the push.** `TrainSkillNow`
  (`skillQueueSvc.py:1234`) goes through `CheckCanInsertSkillAtPosition`
  (`:256`), which raises `QueueSkillNotUploaded` if
  `GetSkillIncludingLapsed` is still `None`. So the injection push has a
  half-second budget, and `OnServerSkillsChanged` is the only thing that fills
  that gap.
- `skillQueuePanelNew` and `tooltip/itemBtns` reach the same `skillsvc` method.

### Server-side semantics **[I]**

Injecting should: delete (or re-flag) the skillbook item at `flagHangar`,
create the skill at `flagSkill` with `skillPoints = 0`, `skillLevel = 0`, and
push `OnServerSkillsChanged` plus `OnItemChange` for the consumed book. The
item half is round 2 step 2 and the push half is step 4; this is why skills
are step 5.

**Purchase is injection plus a wallet debit minus the item.** The real
server's `pay_for_skill` + `inject_skills` split (`characterskills/purchase.py`)
says so: the purchase path never materialises a skillbook item at all, it
debits and injects. So a server-side `inject(charID, typeID)` primitive serves
both, and `PurchaseSkills` is `debit(price) + inject()` per id.

### `CharStartTrainingSkill` is dead on this build **[V]**

`skillsvc.Train` (`:883`) is the only caller of
`CharStartTrainingSkill(skillX.itemID, skillX.locationID)` (`:890`), and a
sweep of the tree finds **no caller of `skillsvc.Train`**. It is also
internally broken: its confirm dialog reads `skill.skillLevel`
(`skillsvc.py:888`), the property that raises. Do not implement it. The live
training path is `SaveNewQueue`.

---

## 6. The training queue

### Read

`skillQueueSvc.GetSkillQueueAndFreeSPsFromServer` (`:158`):

```python
return self.skills.GetSkillHandler().GetSkillQueueAndFreePoints()   # 159
```

Destructured immediately as `(queue, freeSkillPoints)` by `PrimeSkillQueue`
(`:93`) and `BeginTransaction` (`:117`). So: a **2-sequence**. A tuple.
`freeSkillPoints` is then `> 0`-compared behind `is not None` in `PrimeSkillQueue` (def `:93`, `SetFreeSkillPoints` at `:96`)
and compared bare in `BeginTransaction` (def `:117`, `SetFreeSkillPoints` at
`:148`); `None > 0` is `False` in Python 2, so `0` is safe and
`None` would merely be silent — but `GetFreeSkillPoints` must also be an int
(§7's table) so keep them consistent.

### Row shape

The elements are `utillib.KeyVal`s. CCP's own constructor is
`characterskills/queue.SkillQueueEntry` (`characterskills/queue.py:45`), and
`GetQueueEntry` (`:21`) is what the client itself builds locally, so the server
form must match it field for field:

| field | type | read by |
|---|---|---|
| `queuePosition` | int, 0-based | `skillQueuePanelNew.py:533`, `:536` |
| `trainingTypeID` | int | everywhere; `skillQueueSvc.py:167`, `:1126`, `:1514` |
| `trainingToLevel` | int 1–5 | `skillQueueSvc.py:170`, `:1127`, `:1207` |
| `trainingStartSP` | int | `skillQueuePanelNew.py:748` |
| `trainingDestinationSP` | int | `skillQueuePanelNew.py:748` |
| `trainingStartTime` | **blue FILETIME** or `None` | `skillQueueSvc.py:1565` (`GetStartTimeOfQueue`, def `:1560`) |
| `trainingEndTime` | **blue FILETIME** or `None` | `skillQueueSvc.py:166`, `:1512`, `:1558` |

**Mutable, not a `DBRow`.** `OnSkillQueuePausedServer` (`skillQueueSvc.py:1212`)
does `skillEntry.trainingStartTime = skillEntry.trainingEndTime = None` at
`:1224`. A `KeyVal` takes that; a `DBRow` does not.

`SkillInTraining` (`:1506`, the read at `:1512`) is
`len(queue) and queue[0].trainingEndTime` →
truthy end time means "training". So **`trainingEndTime = None` is how you say
"queued but paused / not started"**, and a real end time on entry 0 is how you
say "training now".

### Write

`skillQueueSvc.CommitTransaction` (`:201`):

```python
queueInfo = {idx: (x.trainingTypeID, x.trainingToLevel)
             for idx, x in enumerate(self.skillQueue)}
skillHandler = self.skills.GetSkillHandler()
skillHandler.SaveNewQueue(queueInfo, activate=activate)              # 227
```

- One positional argument: a **dict keyed by integer position**, values
  `(typeID, toLevel)` 2-tuples. Not a list.
- One **keyword** argument, `activate`, a bool. Our dispatcher carries kwargs
  (`service.rs:123`, `dispatch.rs:255`), so this needs no new plumbing.
- The return is discarded.
- **The raises are load-bearing.** `CommitTransaction` catches, reads
  `getattr(e, 'msg', None)`, and if it is **not** one of
  `'UserAlreadyHasSkillInTraining'`, `'SkillInQueueRequiresOmegaCloneState'`,
  `'SkillInQueueOverAlphaSpTrainingSize'`, it calls `RollbackTransaction()`,
  scatters `OnSkillQueueRefreshed` and re-raises. So any *other* error of ours
  silently reverts the player's queue edit.

**The queue is client-optimistic.** `CommitTransaction` keeps the locally built
`self.skillQueue` with locally computed timestamps and only clears
`cachedSkillQueue`. So a saved queue *displays* immediately with no push at
all. `OnNewSkillQueueSaved(newQueue)` (`skillQueueSvc.py:1228`) exists to
replace it with the server's authoritative version:

```python
def OnNewSkillQueueSaved(self, newQueue):                            # 1228
    if self.IsTransactionOpen():
        self.cachedSkillQueue = newQueue
    self.skillQueue = newQueue
    sm.ScatterEvent('OnSkillQueueChanged')
```

Same row shape as the read. Optional for the owner's test; required for the
queue to survive a second client.

### The FILETIME trap: every time field in this subsystem

`docs/archive/architecture-sweep.md` ("A `PyValue::Long` that forces tag `0x2F`"): our
encoder picks the integer tag by magnitude, so anything under 2³¹ arrives as a
Python `int`, and `FormatDateTime`/`FmtDate` open with `isinstance(value, long)`
and return `None` on the else branch — a blank field with nothing in our log.
Every time field in the skill system, and whether it hits the trap:

| field | where | formatted? | trap? |
|---|---|---|---|
| `GetSkillHistory` rows' **`logDate`** | `skillHistoryRow.py`, `skillHistoryProvider.provide` | **yes** — `charsheet/skillHistoryPanel.pyc` imports `FmtDate`; also `created=` on a `Notification` | **YES.** Any honest recent date is > 2³¹, so in practice safe; a 0 or a small sentinel is silently blank. Never answer a small one. |
| `GetRespecInfo['nextTimedRespec']` | `attributes.AttributeRespecWindow` | yes | **YES** — already handled with `FILETIME_2100` (`character.rs`) |
| queue `trainingEndTime` / `trainingStartTime` | `skillQueueSvc.py:166`, `:1512`, `:1558`, `:1565`; `skillQueuePanelNew.py:748` | **no** — only subtracted from `blue.os.GetWallclockTime()` and truth-tested | no, but **must be a number or `None`**: `x - GetWallclockTime()` on a non-number is a `TypeError`, and 0 would read as "ended in 1601" and render a nonsense countdown |
| character row `trainingStartTime`, `trainingEndTime`, `queueEndTime` | `loginCharacterselection/charselData.GetSkillInTrainingInfo` (`:218`, `:227`, `:228`) and the char-select counters | **[I]** yes, the char-select "time left" counters | **YES** — all three are `None` today (`service.rs:411`–`413`), which is correct and must stay correct unless a real FILETIME is available |
| `GetBoosters` values' `expiryTime` | `skillsvc._GetSkillAcceleratorBoosters` (`:880`), `ui/control/entries/implant.pyc` | **[I]** countdown, arithmetic | no, but must be a number; `{}` today avoids it entirely |
| `alphaInjectorMgr.GetNextAvailableInjection()` | `skillsvc.py:1168`, `:1171` | no — `max(0, x - GetWallclockTime())` | no, but `None` is a `TypeError`. Answer a FILETIME. |
| the `timeStamp` argument of `OnServerSkillsChanged` / `OnServerSkillsRemoved` | compared `getattr(currentSkill,'timeStamp',0) > timeStamp` (`skillsvc.py:213`, `:229`) | no | no, but it **must be a number and must increase**: a value ≤ a stored one makes the handler `continue` and **drop the update**, which is a silent lost push. Use `blue.os.GetWallclockTime()`-scale FILETIMEs. |

**The rule for this subsystem: every timestamp we emit is a blue FILETIME
(100 ns ticks since 1601), of FILETIME magnitude, or `None`. Never 0, never a
Unix epoch second.** The one that will bite is `timeStamp` on the push,
because getting it wrong loses the push with no error anywhere.

---

## 7. The pushes

Cross-referenced against `findings/push-events.tsv` (3,177 handlers). `decl` is
that file's count of declaring handlers; `local_emitters=0` means the only way
the event ever fires is a server push.

### Required for a purchase to become visible without a relog

| event | arity | args | handler | what redraws |
|---|---|---|---|---|
| **`OnServerSkillsChanged`** | 3 | `skillInfos`, `event`, `timeStamp` | `skillsvc.py:186`, `decl=2`, `local_emitters=0` | **everything.** Writes both skill caches (`:225`, `:226`), resets the skill-history cache (`:188`), calls `skillqueue.OnServerSkillsChanged` (`:189`), and scatters `OnSkillsChanged` (`:197`) — which has **46** handlers: the Skills catalogue, the skill-plan panels, the queue panel, fitting, industry, certificates, ship tree, the market's fitting search. |

That is the whole required set. One push.

**Its arguments, precisely:**

- `skillInfos` — a **dict** `{typeID: CharacterSkillEntry}` (§3's shape,
  mutable). `.iteritems()`-ed at `skillsvc.py:210`; also `contains`-tested by
  the second handler, `freelanceprojects/client/repository.py:856` (the tsv
  marks that one `raises-on-shape`), so a dict satisfies both.
- `event` — a string or `None`. `if event: sm.ScatterEvent(event, skillInfos)`
  (`:195`–`:196`). **`None` is correct**; any name we pass must be an existing
  client notify event or `ScatterEventWithoutTheStars` logs two errors. Pass
  `None`.
- `timeStamp` — a monotonically increasing FILETIME. See §6's last row: too
  small and the push is silently dropped.

**Two traps in the handler, both ours to avoid:**

1. `_UpdateMySkillsAndReturnLevelsTrained` dispatches on
   `skillInfo.trainedSkillPoints >= 0` (`:211`). An entry with
   `trainedSkillPoints = None` falls through (`None >= 0` is `False`) to
   `virtualSkillLevel is not None` (`:227`) and, if that is also `None`, to
   **`del self.myskills[typeID]`** (`:237`) — a `KeyError` for a skill the
   cache does not have. So **never push an entry with
   `trainedSkillPoints = None`** unless you mean "remove this skill".
2. For a freshly injected skill, `trainedSkillLevel = 0`, so `endLevel = 1`,
   `startLevel = 1`, and `xrange(1, 1)` is empty (`:220`–`:222`): **no**
   "skill trained" sound and no notification. That is correct — a bought book
   is not a trained level.

### The cheap alternative

| event | arity | handler | effect |
|---|---|---|---|
| `OnSkillForcedRefresh` | **0** | `skillsvc.py:1096`, `decl=1`, `local_emitters=0` | `uthread.Pool` → `ForceRefresh` → `Reset()` → every `skillsvc` cache to `None`, then `ScatterEvent('OnSkillQueueRefreshed')` |

`OnSkillQueueRefreshed` has `decl=5`, and the Skills catalogue panel is one of
them (`skillsCataloguePanel.py:186`), as is the queue panel
(`skillQueuePanelNew.py:1209`). So a 0-argument push plus a correct
`GetAllSkills` **is** enough to make a bought skill appear and to terminate
`WaitUntilSkillsAreAvailable`. It costs: a re-ask of `GetSkills`,
`GetAllSkills`, `GetAttributes`, `GetBoosters`, `GetImplants`,
`GetFreeSkillPoints`, `GetRespecInfo` and a re-bind of
`skillMgr2.GetMySkillHandler` (because `Reset` clears `skillHandler` too,
`:141`). All eight are already answered. It is asynchronous
(`uthread.Pool`), so it races the 500 ms budget in §5's `TrainNow` — **[I]**
it wins comfortably, but `OnServerSkillsChanged` has no race at all.

**Recommendation: implement `OnSkillForcedRefresh` first** (it is a
zero-argument notification and needs no new marshal shape), then replace it
with `OnServerSkillsChanged` once the entry shape is on the wire. The first is
one evening; the second is correct.

### Optional polish, in descending order of value

| event | arity | args | handler | notes |
|---|---|---|---|---|
| `OnFreeSkillPointsChanged` | 1 | `newFreeSkillPoints` | `skillsvc.py:997`, `local_emitters=0` | → `SetFreeSkillPoints` → `OnFreeSkillPointsChanged_Local` (`decl=5`). Needed only when the server grants unallocated SP. |
| `OnNewSkillQueueSaved` | 1 | `newQueue` (same rows as §6) | `skillQueueSvc.py:1228`, `local_emitters=0` | makes the queue server-authoritative; the client already shows its own optimistic copy |
| `OnServerSkillsRemoved` | 2 | `skillInfos`, `timeStamp` | `skillsvc.py:199`, `local_emitters=0` | extraction / GM removal only |
| `OnSkillQueuePausedServer` | 0+`*args` | — | `skillQueueSvc.py:1212`, `local_emitters=0` | blanks every entry's times and plays a sound |
| `OnRespecInfoChanged` | 0+`*args` | — | `skillsvc.py:930`, `local_emitters=0` | after a remap |
| `OnServerBoostersChanged` / `OnServerImplantsChanged` | 0+`*args` | — | `skillsvc.py:1103`, `:1109` | boosters/implants, not skills |
| `OnSkillBundleInjected` | 2 | `numberOfSkills`, `sp` | `skillsvc.py:1272` | the "skill bundle" crate flow |
| `OnLogonSkillsTrained` | 2 | `skillInfos`, `canTrain` | `skillsvc.py:168`, `local_emitters=0` | **do not send yet** — see below |

**`OnLogonSkillsTrained` wants a different row shape, and this is a trap.** Its
handler reads `skillInfo.currentLevel` and `skillInfo.skillLevel`
(`skillsvc.py:175`) — and `skillLevel` on a `CharacterSkillEntry` *raises*. So
its `skillInfos` values are **[I]** `KeyVal`s with `currentLevel` and
`skillLevel`, a shape we have no second witness for. Sending
`CharacterSkillEntry`s there throws on the first iteration. It only plays a
sound and shows "trained while you were away" notifications; leave it unsent
and mark the shape **unknown**.

### Two events the client scatters locally — we must not send them

`OnSkillsChanged` (46 handlers) and `OnSkillLevelsTrained` are scattered by
`skillsvc` itself (`:197`, `:193`). `OnSkillQueueChanged`, with **15**
declaring handlers and the single widest redraw in the subsystem, is also
local (`skillQueueSvc.py:1232`, among others; the tsv marks it
`local_emitters=1`). Pushing them directly would skip
the cache update that is the whole point.

### And one global-config push that is the honest "off" switch

`globalConfig.IsSkillPurchaseEnabled` reads
`machoNet.GetGlobalConfig()['SkillPurchaseEnabled']`, default True, and
`purchase_skills` checks it *first* (`characterskills/client/purchase.py:33`),
raising `UserError('SkillPurchaseDisabled')` — a label the client already has,
with no wire call at all. It arrives via `InitClientGlobalConfig`, which we
never send. If we ever want skill purchase to be honestly *off* rather than
silently broken, that push is the way, and it is cheaper than minting an error
per call.

---

## 8. Live-lock audit of every recommended answer

Per `docs/guides/debugging-the-client.md` §4: a live lock's severity is set by the
**caller's cadence**, not by the shape of the guard. For each answer: does it
feed a guarded cache, which variant, and what paces the re-ask.

| call | guard | recommended answer latches? | variant if wrong | what paces the re-ask |
|---|---|---|---|---|
| `GetMySkillHandler` | `if not self.skillHandler` (`:153`) — truthiness | **yes**, a bound object is truthy | — | one bind per session, or per `Reset()` |
| `GetSkills` | `is None` (def `:241`) | **yes** — `{}` is not `None`, and `:161` stores it directly | `None` → poisoned cache (`contractProxy` variant) **and** `.iteritems()` `AttributeError` in `MyEffectiveSkillLevelsByID` | `MySkillLevel` is `@telemetry.ZONE_METHOD` on fitting/market/industry redraw paths — effectively **render-paced**. A `None` here is a frame-dropping loop. |
| `GetAllSkills` | `is None` (`:285`, `:290`) | **yes** | same | `GetSkillIncludingLapsed` on every queue insert check and every `WaitUntilSkillsAreAvailable` iteration |
| `GetAttributes` | `is None` (`:247`) | **yes** — the store at `:252` precedes the `.iteritems()` at `:253` | `None` → poisoned **and** `[attributeID]` subscript `TypeError`; **`0.0` values → `ZeroDivisionError`** in `GetSkillpointsPerMinute` | `GetRawTrainingTimeForSkillLevel` runs per queue entry per redraw — **render-paced** |
| `GetBoosters` / `GetImplants` | `is None` (defs `:1124`, `:1129`; calls `:1126`, `:1131`) | **yes** — `{}` | `None` → poisoned, and **invisible**: `crimewatchSvc.GetMyBoosters` wraps the read in `hasattr`, which swallows every exception in Python 2 | HUD rebuild |
| `GetFreeSkillPoints` | `is None` (def `:1000`, store at `:1003`) | **yes** — `0` is not `None` | `None` → poisoned | `ConstructCharacterTotalSkillPoints` installs `AutoTimer(2000, …)` **after** the first success, so once it works this is a 0.5 Hz throttle — the `scanMgr` variant: small count, constant interval, forever |
| `GetSkillPoints` | none | n/a | `None` → `TypeError` on `skillPoints + freeSkillPoints` (`skillsvc.py:1153`) | user-paced (extractor activation) |
| `GetRespecInfo` | `is None` (def `:921`, call at `:927`) | **yes** | `None` → poisoned + `KeyError` on both subscripts | character-sheet Attributes tab open |
| `GetSkillHistory` | `is None` (def `:397`, call at `:400`) + a second layer in `notificationUI.notificationCache` | **yes** — `[]` | `None` → poisoned in **two** layers, and kills the entire notification centre (it is the first statement of `_NotificationProvider`) | bell open — **user-paced** |
| `GetSkillChangesForISIS` | caller's double-checked `is None` | **yes** — `[]` | `None` → `TypeError` in the `for typeID, pointChange in …` **and** a re-issue | once per ship node drawn in ISIS — **render-paced** |
| `GetSkillQueueAndFreePoints` | none | n/a | `None` → `TypeError` on the 2-unpack in `PrimeSkillQueue` (`:93`) / `BeginTransaction` (`:117`) | `Run`, session change, and each queue transaction — **user-paced** |
| **`PurchaseSkills`** | **none — not a cache at all** | n/a | **a fifth variant, and the worst in the project so far.** A truthy answer with no accompanying push puts `WaitUntilSkillsAreAvailable` into `while skill_type_ids != seen: blue.pyos.BeNice()` (`:1252`–`:1257`) with **no timeout, no re-ask and no escape**. `BeNice` yields but never sleeps, so the tasklet burns a core and *no answer at any cadence can end it* — the loop reads a client-side dict. | **nothing paces it.** It is not throttled by a render handler, a `debounce`, a `BufferedCall` or an `AutoTimer`; it is bounded only by the arrival of a push. This is why the only safe answers are `[]` (today) or "the truthful list **plus** the push, in the same reply". |
| `SaveNewQueue` | none | `None` | a non-`UserError`, or a `UserError` whose `msg` is not one of three names, makes `CommitTransaction` roll back and re-raise — the player's queue edit silently reverts | user-paced (queue edit) |
| `CheckAndSendNotifications` | none, `POP_TOP` | `None` **is** the answer | must not raise: `provide()` runs on the same tasklet immediately after | bell open |
| `alphaInjectorMgr.GetNextAvailableInjection` | none | a FILETIME | `None` → `TypeError` in the subtraction | banner refresh — **[I]** timer-paced, so worth getting right |

Two general conclusions:

- **Every guard in `skillsvc` is `is None`, not truthiness.** That is lucky and
  it is the opposite of `jumpCloneSvc.GetShipCloneState`: here the honest empty
  answer *does* latch, so there is no "correct answer that is itself the live
  lock" anywhere in this subsystem. Which is exactly why the spin lives in
  `PurchaseSkills`' consumer instead of in a cache.
- **The two render-paced ones are `GetSkills` and `GetAttributes`**, and
  `GetAttributes` is the one where a *plausible* wrong answer (0.0) is a
  `ZeroDivisionError` rather than a loop. Keep the five 20.0s.

---

## 9. What we answer today, and what each answer costs

`server/crates/eve-server/src/service/character.rs:572`–`816`. Twelve
methods, all of them honest, none of them wrong:

```text
GetSkillHistory            -> []
GetBoosters                -> {}
CheckAndSendNotifications  -> None
GetSkills                  -> {}
GetSkillQueueAndFreePoints -> ([], 0)
GetAllSkills               -> {}
GetFreeSkillPoints         -> 0
GetAttributes              -> {164..168: 20.0}  (charisma..willpower)
GetImplants                -> {}
GetRespecInfo              -> {nextTimedRespec: FILETIME_2100, freeRespecs: 0}
GetSkillChangesForISIS     -> []
PurchaseSkills             -> []
```

The other **seventeen** of the twenty-nine are unanswered, and therefore
answered `None` by `Dispatcher::unhandled`. Of those, the ones that will throw the
moment the owner touches the feature:

- `GetSkillPoints` → `TypeError` in the skill-extractor window.
- `SaveNewQueue` → `None` is actually fine (discarded); the queue saves
  client-side and nothing persists.
- `GetFreeSkillPointsAppliedToQueue` / `ToSkills` → `if pointsBySkillTypeID`
  on `None` is falsy, so they no-op quietly.
- `alphaInjectorMgr.GetNextAvailableInjection` → `TypeError` in the alpha
  injector banner.
- `dogmaIM.InjectSkillIntoBrain` → discarded, so right-click Inject is a
  silent no-op exactly like Buy.

---

## 10. Implementation order

The owner's test for done is: **buy a skill, see it trained, relaunch, still
trained.** Steps 1–4 are the minimum path to the first three clauses; step 5 is
the fourth.

### 1. The skill table in the world, and two honest reads

**Closes:** `GetSkills`, `GetAllSkills`, `GetSkillPoints`.
**Needs:** round 2 step 1 (`Arc<RwLock<World>>`) — without it two connections
are two sets of skills and a reconnect discards them.

A per-character map `typeID -> (trainedSkillLevel, trainedSkillPoints, skillRank)`.
Encode an entry as the `0x23` ObjectEx of §3. `GetSkills` and `GetAllSkills`
both return it as a dict keyed by type id (they differ only once lapsed skills
exist, which needs clone grades, which we do not have — answer the same map
from both and say so in a comment). `GetSkillPoints` is the sum of
`trainedSkillPoints`. Seed it from the sandbox config so there is something to
look at before anything can be bought.

**Observable:** the Skills catalogue shows the seeded skills as injected, with
levels; the character sheet's SP total is non-zero; `marketbase.GetMySkills`
stops being empty. No buying yet.

### 2. `OnSkillForcedRefresh`, as a `Notify` variant

**Closes:** nothing new; it makes step 3 terminate.
**Needs:** one arm of `Notify` (`service.rs:497`) and one arm of
`pending_pushes` (`dispatch.rs:939`). Zero arguments, so no marshal work.

**Observable:** nothing on its own. Ship it with step 3.

### 3. `PurchaseSkills` for real

**Closes:** `PurchaseSkills`.
**Needs:** steps 1 and 2, plus the wallet (round 2 step 3) if the debit is to
be real. Without the wallet, sell for free and say so in the comment — the
client has already checked the balance client-side and will not notice.

All-or-nothing, per §4: validate every id (is a skill, not already in the map),
then create every one at level 0 / 0 SP, then return the full list, then queue
the `OnSkillForcedRefresh` push in the **same** `Reply`. Keep the existing test
that `PurchaseSkills` never echoes the request *without* a push — invert it
into "the reply that carries a non-empty list also carries a push".

**Observable: the owner buys a skill in the Skills window and it appears as
injected, in that session, without a relog.** "Buy and train" also stops
raising `QueueSkillNotUploaded`. This is the step that answers "nothing
bought".

### 4. `OnServerSkillsChanged`, replacing step 2's push

**Closes:** the 500 ms race in `invItemFunctions.TrainNow`, and the seven
re-asks `Reset()` causes.
**Needs:** the entry shape already built in step 1, plus a monotonic FILETIME
source for `timeStamp`.

Push `{typeID: entry}`, `event = None`, `timeStamp = now`. Keep
`OnSkillForcedRefresh` as the fallback path for anything that changes skills
wholesale.

**Observable:** right-click → Inject Skill and right-click → Train Now both
work on a skillbook in the hangar (once step 6 gives us skillbook items);
nothing flickers; the catalogue updates without re-fetching eight calls.

### 5. Persistence

**Closes:** the fourth clause of the owner's test.
**Needs:** `docs/reference/world-state.md` §3's `Store` seam (its step 2, the snapshot file).
`docs/reference/world-state.md` §2 already lists skills as item 7 and already names the catch:
*"store `trainingStart`/`End` and compute SP on read; a restart that loses the
start time loses the training."*

**Observable: relaunch, still trained.** Note the ambiguity in the owner's
test: relaunching only the **client** needs step 1 (shared world) and nothing
more, because the server process keeps the map. `tools/up.sh` restarts the
server with the client, so in practice the owner's "relaunch" restarts both —
and that needs this step. Say which one is being claimed.

### 6. The training queue, server side

**Closes:** `SaveNewQueue`, `GetSkillQueueAndFreePoints` with real rows,
`OnNewSkillQueueSaved`, `AbortTraining`.
**Needs:** steps 1 and 5 (a queue that does not survive a restart is worse than
none, because the player will have planned around it).

Store the queue as `[(typeID, toLevel)]` plus the start time of entry 0;
compute `trainingStartTime`/`trainingEndTime` on read from `skillRank`, the
five attributes and `GetSkillPointsPerMinute(primary, secondary/2)`
(`characterskills/util.py:36`). Emit `KeyVal`s per §6, FILETIMEs per §6's rule.
Then the *tick*: when `trainingEndTime` passes, bump the skill and push
`OnServerSkillsChanged` — which is the first genuinely time-dependent thing in
the world and belongs on `push.rs`'s existing ticker.

**Observable:** a skill actually trains while the client is open, with the
level-up sound and the notification, and the queue survives a restart.

### 7. Everything else, in no particular hurry

`GetSkillHistory` rows (watch `logDate`'s magnitude), the free-SP family,
`RespecCharacter`, the skill extractor/injector family, `GetSkillBundleInfo`,
`alphaInjectorMgr.GetNextAvailableInjection`. None of them is on the path to
the owner's test, and `GetSkillHistory` is the only one that improves anything
visible (the notification centre).

### Explicitly not to be built

- `CharStartTrainingSkill` — dead on this build (§5).
- `OnLogonSkillsTrained` — shape unknown and it would throw (§7).
- `skillsvc.GetSkillGroups`'s path — raises on this build whatever we send (§3).
- Skills as godma items — the client drops them (§1).

---

## 11. Open, and deliberately unknown

- **The `OnLogonSkillsTrained` row shape.** `currentLevel` + `skillLevel`, on
  something that is not a `CharacterSkillEntry`. One witness, no second.
  **unknown.**
- **`GetSkillBundleInfo`'s 2-tuple.** `UNPACK_SEQUENCE 2`; the two members are
  not named anywhere we can see. **unknown.**
- **`GetImplants`' key space.** "Keyed by implant slot and `.iteritems()`-ed"
  is all `character.rs` established and all we can confirm. **unknown**
  beyond that.
- **`GetDiminishedSpFromInjectors`' return.** A number; the diminishing curve
  is server-side. **unknown.**
- **`ExtractSkills`' access token.** `sm.GetService('connection').GetAccessToken()`
  is passed as the third argument. What the real server does with it:
  **unknown**, and it is a reason to leave extraction alone.
- **`skillRank` from FSD, server side.** Blocked on the `.fsdbinary` reader
  (`docs/archive/recon-v24.01.md`). A curated table is the stopgap; the rank must be ≥ 1 or
  the queue panel raises `ZeroDivisionError` (§3).
- **`basePrice` from FSD, server side.** Same blocker. The client computes the
  price and pre-checks the balance, so we can charge something else and the
  only symptom is a wallet that disagrees with the quote. **[I]**
- **Whether one `BeNice()` cycle is enough** for a zero-delay push appended
  after the `CallRsp` to reach `mySkillsIncludingLapsed` before the spin
  notices. **[I]** yes. If it is not, the lever is a push emitted *before* the
  reply's payloads, which `dispatch::Reply` cannot express today.
- **Lapsed skills.** `trainedSkillLevel is None` / `trainedSkillPoints is None`
  is the alpha-clone shape. We have no clone grades, so we never produce one —
  and §7's trap 1 says a push carrying one is a `KeyError`. Left out
  deliberately.
