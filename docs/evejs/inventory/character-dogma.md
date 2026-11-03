# Inventory: character, skills and dogma (EvEJS)

EvEJS's character/skills/dogma area is ~52k lines of JavaScript across five
service trees: `services/dogma` (15,691 LOC, 25 files), `services/skills`
(11,889 LOC, 31 files), `services/character` (18,945 LOC, 36 files),
`services/fitting` (5,227 LOC, 9 files + one 1,451-line JSON authority) and
`services/settings` (304 LOC, 1 file). The real dogma *engine* is not in
`services/dogma` at all — it is `services/fitting/liveFittingState.js` (4,211
LOC), which every other module in the area imports. `services/dogma` is mostly
the `dogmaIM` RPC surface (`dogmaService.js` 4,372 LOC + `dogmaService/`
subdir, of which `moduleOperation.js` is 4,170 and `itemInfo.js` 2,130) plus
wire-payload builders. **There are no README.md files anywhere in the area and
zero test files anywhere in the release** (`find` for `*.test.js` → 0). Every
status below is read off code paths, never off a passing test. A further
caution: the one cited evidence document for the godma notification shapes,
`doc/PARITY_FITTING_NOTIFICATION_SEQUENCE.md` (referenced at
`services/_shared/godmaMultiEvent.js:8`), **is not shipped in the release** —
`doc/` has 13 files and that is not one of them.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---:|---|---|
| Dogma evaluation engine (attribute composition, operation order, stacking) | 1: `services/fitting/liveFittingState.js` | 4,211 | functional | `applyModifierGroups` groups modifiers by `attributeID:operation` and walks a fixed 9-entry `operationOrder` (`liveFittingState.js:3058`, order list `:3178-3188`); stacking denominators `exp((i/2.67)^2)` at `:1106-1108`; penalty chain `getStackedMultiplierFactor` `:2978`; base-value fallback to the SDE `defaultValue` `:3050`. Entry point `buildShipResourceState` `:3474`, merged source list `:3568-3581`. |
| Dogma brain (client-side modifier push) | 5: `brain/characterBrainRuntime.js`, `brain/providers/{fitting,implant,industry}BrainProvider.js`, `_secondary/fitting/fittingBrainBuilder.js` | ~1,150 | partial | Builds and marshals three lists of `eve.common.script.dogma.effect.BrainEffect` (`characterBrainRuntime.js:58-88`, `:218-236`); pushed as `OnServerBrainUpdated` `:281`. Only three providers (`:28-32`) — **no module/rig/subsystem/booster/fleet provider exists**, so a fitted module's bonus never reaches the client brain. |
| `brainEffectPickle.js` (pickle encoder) | 2: `brain/brainEffectPickle.js` + 3-line re-export shim `brainEffectPickle.js` | 95 | **dead code** | Hand-rolled Python pickle protocol-2 writer (`:75-87`). `grep -rn "brainEffectPickle\|buildBrainEffectPickle"` over all of `server/src` returns **only its own definition and its own shim** — zero call sites. |
| `dogmaIM` RPC surface | 10: `dogmaService.js` + `dogmaService/{itemInfo,moduleOperation,moduleRepair,targeting,weaponBanks,overload,implantsAndBoosters,droneSettings,layerDamageValues,requiredSkillLevels}.js` | ~12,500 | functional | Service name `super("dogmaIM")` `dogmaService.js:514`. 51 `Handle_*` methods incl. `GetAllInfo` (`itemInfo.js:1455`), `ShipGetInfo` `:1841`, `CharGetInfo` `:1892`, `Activate` (`moduleOperation.js:3194`), `SetModuleOnline` `:3662`, `LoadAmmo` `:3723`, `AddTarget` (`targeting.js:43`), `Overload` (`overload.js:57`), `LinkWeapons` (`weaponBanks.js:163`). |
| Implants + boosters (dogma side) | 3: `implants/activeImplantModifiers.js`, `implants/implantRuntime.js`, `dogmaService/implantsAndBoosters.js` | 1,769 | functional | `getActiveImplants` `activeImplantModifiers.js:722`, slot resolution via attribute 331 `:57`, own modifier-group applier `applyDogmaModifierGroups` `:235` (a **second, independent copy** of the engine), `applyActiveImplantAttributeBonuses` `:995`. RPCs `Handle_InjectImplant`/`DestroyImplant`/`UseBooster` `implantsAndBoosters.js:149/176/202`. |
| Overload / heat | 2: `dogmaService/overload.js`, `activationHeatState.js` | 242 | functional | Attributes heatHi 1175 / heatMed 1176 / heatLow 1177 documented and emitted (`activationHeatState.js:7-21`); four RPCs `overload.js:57-110`. |
| Module repair (nanite) | 1: `dogmaService/moduleRepair.js` | 416 | functional | `Handle_InitiateModuleRepair` `:357`, `...Many` `:378`, `StopModuleRepair` `:404`. |
| Dynamic (abyssal/mutaplasmid) items | 1: `dynamicItemService.js` | 545 | partial | Own `BaseService`; attribute overrides read back by the engine via `getDynamicItemAttributeOverrides` (`liveFittingState.js:1632`) and `parseDynamicItemCustomInfo` `:1613` (overrides stashed in an item's `customInfo` string — fragile). |
| Crystal durability / ship condition | 2: `crystalDurability.js`, `shipConditionAmounts.js` | 159 | functional | Pure policy functions; attribute IDs 9/783/784/786 (`crystalDurability.js:14-19`), 3/18/264/266 (`shipConditionAmounts.js:8`). |
| Skill state + storage | 2: `skills/skillState.js`, `skills/skillTransport.js` | 1,302 | functional | Skills live in their own `skills` table, not the item store (`readSkillsTable` `skillState.js:48`); records carry `trainedSkillLevel`/`effectiveSkillLevel`/points (`buildSkillRecord` `:274`, `normalizeSkillRecord` `:309`). |
| Skill training queue | 5: `skills/training/{skillQueueRuntime,skillQueueNotifications,skillQueueState,skillTrainingMath,skillTrainingSpeed}.js` + `skillCloneRestrictions.js` | 3,218 | functional | See "Skill training" below. `settleCharacterTraining` `skillQueueRuntime.js:629`, `getQueueSnapshot` `:821`, `scheduleQueueCompletion` `:1253`. |
| Skillbooks / injection | 2: `skills/skillbooks/skillbookRuntime.js`, `skills/skillAdminRuntime.js` | 816 | functional | `Handle_PurchaseSkills`, `Handle_InjectSkillIntoBrain` (`skillMgrService.js`); `dogmaIM` also exposes `InjectSkillIntoBrain` (`dogmaService.js:4163`). |
| Skill injectors / extractors (trading) | 4: `skills/trading/{skillTradingRuntime,skillTradingAuthority,skillTradingState,skillTradingNotifications}.js` | 1,662 | functional | RPCs `InjectSkillpoints`, `ExtractSkills`, `GetDiminishedSpFromInjectors`, `SplitSkillInjector`, `CombineSkillInjector` (`skillMgrService.js`). |
| Certificates | 3: `skills/certificates/{certificateRuntime,skillChangeTracker}.js`, `character/certificateMgrService.js` | 768 | functional | 10 `Handle_*` incl. `GetMyCertificates`, `GrantCertificate`, `BatchCertificateGrant` (`certificateMgrService.js`). Static table `TABLE.CERTIFICATES`. |
| Boosters (skill side) | 2: `skills/boosters/{boosterRuntime,boosterSkillRuntime}.js` | 861 | functional | Booster slot attribute 1087 (`activeImplantModifiers.js:58`); on inject it re-pushes dogma state (`boosterRuntime.js:455-457`). |
| Expert systems (Omega-like temporary skills) | 7: `skills/expertSystems/*.js` | 1,616 | functional | `expertSystemProjection.js` is folded into skill reads (`applyExpertSystemProjection` `skillState.js:86`); static table `expertSystems` in `tools/DatabaseCreator/staticTables`. |
| Skill plans | 2: `skills/plans/{skillPlanState,corpSkillPlanState}.js` | 903 | unknown | State modules exist; no RPC handler found in `services/skills` that exposes them — likely reached from another service tree. Not verified. |
| Character sheet / core state | 3: `character/characterState.js`, `charService.js`, `charMgrService.js` | 8,306 | functional | `characterState.js` is the central record store (5,112 LOC) and also emits `OnGodmaPrimeItem` `:1608` and `OnGodmaShipEffect` `:3338-3341`. ~80 `Handle_*` across `charMgrService`/`charService`. |
| Character attributes + respec | 3: `character/characterOwnAttributes.js`, `skills/skillMgrService.js`, `character/characterCreationData.js` | ~400 | functional | Five attributes 164-168, default 20 (`characterOwnAttributes.js:7-14`); `Handle_GetAttributes`, `Handle_GetCharacterAttributeModifiers`, `Handle_GetRespecInfo` (`skillMgrService.js`). |
| Standings | 4: `character/{standingRuntime,standingMgrService,npcStandingsAuthority,factionKillStandingRuntime}.js` | 2,353 | functional | 5 `Handle_*` (`standingMgrService.js`), 1,898-LOC runtime. |
| Portraits / paperdoll | 4: `character/{paperDollServerService,paperDollPayloads,portraitImageStore,photoUploadService}.js` | 1,116 | functional | 6 `Handle_*` incl. `ConvertAndSavePaperDoll`, `GetPaperDollPortraitDataFor` (`paperDollServerService.js`). |
| Clones (jump/medical) | outside area: `services/station/jumpCloneRuntime.js`; in-area: `character/homeStationRuntime.js`, `homeStationSelfDestructRuntime.js`, `homeStationService.js` | 820 in area | partial | `GetCloneInfo` lives in `charMgrService.js`. Pod death clears implants via `jumpCloneRuntime.clearActiveImplantsForPodDeath` (`homeStationSelfDestructRuntime.js:128`), and a clone jump re-pushes dogma (`jumpCloneRuntime.js:472-474`). The clone *system* is not in this area. |
| Character deletion / naming / notes / misc | ~15 files incl. `characterDeletionRuntime.js`, `characterNameRuntime.js`, `characterNoteState.js`, `accessGroup*`, `achievementTracker*`, `milestone*`, `season*`, `projectDiscovery*` | ~5,000 | partial-to-unknown | Mostly peripheral live-service features irrelevant to a co-op port. Not individually verified. |
| Saved fittings (char/corp/alliance) | 4: `fitting/{charFittingMgrService,corpFittingMgrService,allianceFittingMgrService,fittingMgrServiceHelpers}.js` + `fittingState.js` | 654 | functional | 7 identical `Handle_*` per scope: `GetFittings`, `SaveFitting`, `UpdateFitting`, `SaveManyFittings`, `DeleteFitting`, `DeleteManyFittings`, `UpdateNameAndDescription`. Thin CRUD over `fittingState`. |
| Hull role-bonus skill axes | 2: `fitting/hullBonusSkillAuthority.js` + `fitting/authority/hullBonusSkillAxes.json` | 1,550 | functional | SHA-256-pinned projection of SDE `typeBonus.jsonl`, build 3396210, 423 hulls / 389 with bonuses / 622 skill links (`hullBonusSkillAuthority.js:7-28`). Consumed by `buildHullBonusSkillMap` (`liveFittingState.js:1675`). |
| Charge compatibility / fitting integrity | 2: `fitting/chargeCompatibilityPolicy.js`, `fitting/fittingIntegrity.js` | 262 | functional | `evaluateModuleChargeCompatibility` `liveFittingState.js:4019`. |
| Shared settings | 1: `settings/sharedSettingsMgrService.js` | 304 | functional | **Trivial.** Two RPCs, `Handle_StoreSettingLinkAndGetID` `:263` and `Handle_GetStoredSharedSetting` `:278`; the rest is a hash-indexed blob store. Not worth porting as a subsystem; reimplement in an afternoon if the client needs it. |

## The dogma brain

**Headline: their "brain" is not an engine. It is a serialiser.** EvEJS has
*two* dogma implementations and the brain is the lesser one.

### What a "brain" is here

A brain is a **wire payload**, built fresh on demand, never stored or
evaluated. `buildCharacterBrainGrayMatter`
(`services/dogma/brain/characterBrainRuntime.js:218`) returns a
`marshalEncode`d blob of exactly three marshal lists — character effects, ship
effects, structure effects (`:231-235`). Each element is a marshal `objectex1`
named `eve.common.script.dogma.effect.BrainEffect`
(`buildBrainEffectObject` `:58-88`) whose private state dict is:

```
fromAttrib (always null), value, toItemID, modifierType, toAttribID,
operation, extras, skills
```

So one BrainEffect = "apply `operation` with `value` to attribute
`toAttribID` on item `toItemID`, scoped by `modifierType` + `extras`,
attributable to these `skills`". `fromAttrib` is hardcoded `null` (`:78`) —
they resolve the source attribute's value server-side and ship the *number*,
not the source-attribute reference the real protocol allows. The client's
native dogma engine then does the applying.

`modifierType` is a five-value vocabulary mapped from the SDE's `func`
(`BRAIN_MODIFIER_TYPE_BY_FUNC`,
`_secondary/fitting/fittingBrainBuilder.js:48-82`, independently duplicated as
`IMPLANT_MODIFIER_TYPE_BY_FUNC` at
`services/dogma/implants/activeImplantModifiers.js:75-92`):

| SDE `func` | brain `modifierType` | `extras` |
|---|---|---|
| `ItemModifier` | `M` | `[]` |
| `LocationModifier` | `L` | `[]` |
| `LocationGroupModifier` | `LG` | `[groupID]` |
| `LocationRequiredSkillModifier` | `LRS` | `[skillTypeID]` |
| `OwnerRequiredSkillModifier` | `ORS` | `[skillTypeID]` |

Note the SDE's seventh `domain` values and sixth `func` are dropped: a
modifier whose `func` is not one of those five is silently discarded
(`fittingBrainBuilder.js:118-123` returns `null`), and so is any `domain`
outside `charID`/`shipID`/`structureID` (`getNormalizedBrainDomain`
`fittingBrainBuilder.js:84-95`, duplicated as `normalizeBrainDomain`
`activeImplantModifiers.js:521`).

### What it holds, and from where

Three providers, frozen in this order (`characterBrainRuntime.js:28-32`):

1. **`FITTING_BRAIN_PROVIDER`** — delegates to
   `_secondary/fitting/fittingBrainBuilder.js:256`
   `buildFittingBrainEffectDefinitions`. Iterates the character's skills
   (`:272`), skips level 0, resolves each skill's effective attributes
   (`buildSkillEffectiveAttributes`), and for each of the skill type's
   effects emits one definition per `modifierInfo` row (`:301-324`). So this
   provider is **skills only, despite its name** — no fitted module, rig,
   subsystem or booster contributes a BrainEffect anywhere.
2. **`INDUSTRY_BRAIN_PROVIDER`** (`brain/providers/industryBrainProvider.js`,
   432 LOC) — 12 hardcoded industry skill type IDs (`:28-39`) against 9
   hardcoded industry attribute IDs (`:18-26`, collected `:42-52`). It supplies
   only `buildCharacterEffects` and `syncCharacterAttributeState` (`:420-424`)
   — no ship or structure effects, and no `func`→`modifierType` map at all.
3. **`IMPLANT_BRAIN_PROVIDER`** — `getImplantBrainEffectDefinitions`
   (`implants/activeImplantModifiers.js:682`).

Three named effects are special-cased by *string name* rather than data, in
both the brain builder and the engine: `dronedmgbonus`,
`missile{em,explosive,kinetic,thermal}dmgbonus`, `selfrof`
(`fittingBrainBuilder.js:200-238`, mirrored `liveFittingState.js:1109-1114`
and again at `liveFittingState.js:1988-2042`).

### Composition order

There is no composition in the brain. Definitions are concatenated per
provider in array order (`collectProviderDefinitions` `:109-126`) then sorted
by a tie-break comparator — `skillTypeID`, `modifierType`, `targetAttributeID`,
`operation`, `extras`, `value` (`fittingBrainBuilder.js:240-254`) — purely for
deterministic output, not for evaluation order.

**The real composition order lives in the engine**, `liveFittingState.js`.
`buildShipResourceState` (`:3474`) builds the ship's attribute map in this
sequence:

1. Base: `readShipBaseAttributeMap(shipTypeID)` `:3512`, then any SDE type
   attributes not already present `:3518-3523`, then caller
   `baseAttributeOverrides` `:3528-3531`.
2. Collect (not yet apply) from every source into one flat list:
   - skill fallback percent modifiers (`collectSkillFallbackShipAttributeModifiers` `:3316`)
   - skill-driven modifiers over the hull-bonus skill map (`:3556-3558`)
   - ship-type self modifiers, computed against a *skill-adjusted* copy of the
     base map (`:3563-3567`, with a comment explaining why: hull per-level
     bonus attributes like `shipBonusCC2` must be pre-multiplied by skill level
     first or every hull bonus silently reads as level I)
   - passive fitted-module/rig/subsystem/charge modifiers
     (`collectPassiveShipAttributeModifiers` `:2752`)
   - implant ship-domain modifiers and implant location-modifier sources
     (`:3501-3506`)
3. **Apply all of it in one pass**, globally ordered by dogma operation, not
   by source: `applyPassiveShipAttributeModifiers(derivedAttributes,
   combinedShipAttributeModifierEntries)` `:3577-3580`.

That single-pass-by-operation design is deliberate and the comment says so
(`:3551-3554`): "Dogma operation order is global across modifier sources.
Applying skills before fitted modules would let a post-percent skill miss a
later additive module bonus." **This is the correct EVE semantics and is the
single most valuable idea to port.**

Inside `applyModifierGroups` (`:3058`) entries are bucketed by
`attributeID:operation` and the buckets walked in this fixed order
(`:3178-3188`):

```
-1 PreAssignment, 0 PreMul, 2 ModAdd, 3 ModSub, 4 PostMul,
 6 PostPercent, 5 (divide), 8 PostPercentUnnerfed, 7 PostAssignment
```

Note `5` is placed **after** `6`, and `5` is treated as a reciprocal
multiplier (`factor = 1/value`, `:3131-3135`). Assignment ops take the *last*
entry in the bucket (`:3194-3199`). The base a modifier works from falls back
to the SDE's `defaultValue` for the attribute, not to zero or one, with a
comment citing the Bastion remote-assistance bug that forced it (`:3031-3056`).

### Stacking penalties

Applied in the engine; **not** applied in the brain (the brain ships raw
per-skill modifiers and relies on the client's native engine to stack them).

Engine: `STACKING_DENOMINATORS = exp((i/2.67)^2)` for i in 0..7
(`liveFittingState.js:1106-1108`) — EVE's real curve (1, 0.8691, 0.5706,
0.2830, ...). `getStackedMultiplierFactor` (`:2978`) sorts the penalised
factors ascending, splits at 1.0, and penalises the ≤1 group and the >1 group
**as two independent chains each restarting at index 0** (`:3001-3014`). That
matches pyfa's convention. Bonuses above 1 are reversed before indexing so the
strongest takes the smallest penalty (`:2997`).

**The weak point, and the thing most likely to be silently wrong:** whether a
modifier is penalised at all is decided by two things, neither of which is the
effect's own flag.

- `shouldApplyStackingPenalty(attributeID, stackingPenalized)` `:1303` →
  `stackingPenalized === true && !isDogmaAttributeStackable(attributeID)`,
  where `isDogmaAttributeStackable` reads the SDE attribute's `stackable`
  boolean `:1298`.
- `stackingPenalized` itself is supplied by the **caller**, defaulting to
  `sourceKind === "fittedModule"` (`appendDirectModifierEntries` `:1848-1850`,
  `appendLocationModifierEntries` `:2090-2093`). Skill-sourced and
  implant-sourced entries are hardcoded `stackingPenalized: false`
  (`:1904`, `:2006`, `:2026`, `:2045`, `activeImplantModifiers.js:229`,
  `:871`), self-modifiers likewise `:1904`.

So: *module → penalised, skill/implant → never penalised, gated by the
attribute's `stackable` flag.* Their SDE normalisation does not even carry a
per-modifier penalty flag — `normalizeDogmaModifier`
(`services/_shared/referenceData.js:110-136`) keeps only `domain`,
`modifiedAttributeID`, `operation`, `modifyingAttributeID`, `func`, `groupID`,
`skillTypeID`, `effectID`. Real EVE decides per effect/attribute pair, so this
heuristic will be right for the common cases (armour plates, hardeners, damage
mods) and wrong for the exceptions.

### Rebuild triggers

The brain has **no cache and no invalidation** — it is rebuilt from scratch on
every push. What exists is a monotonic version counter per character, in a
plain in-memory `Map` (`CHARACTER_BRAIN_VERSION_BY_CHARACTER_ID` `:27`,
`getNextCharacterBrainVersion` `:210-216`), incremented on each push. **It
resets to 0 on server restart**, so a reconnecting client can be handed a
*lower* version than it already holds — a plausible desync bug.

Push sites (`syncCharacterDogmaBrain` / `syncCharacterDogmaState`, grep over
`server/src`):

| trigger | call site |
|---|---|
| after every `dogmaIM.GetAllInfo` | `dogmaService.js:4294` |
| implant inject / destroy / booster use | `dogmaService/implantsAndBoosters.js:171`, `:197`, `:227` |
| skill mutation (train, inject, extract) | `skills/training/skillQueueNotifications.js:264`, `:405` |
| booster applied | `skills/boosters/boosterRuntime.js:455-457` |
| clone jump | `services/station/jumpCloneRuntime.js:472-474` |
| industry context | `services/dogma/industryBrainRuntime.js:22-23` |

Separately, `services/character/dogmaInvalidationVersion.js` is a *global*
monotonic counter (not per character) bumped on fit/unfit/online/offline/
charge/mutation/implant/booster changes, used only as a cache-key component
for the drone dogma context fingerprint — its own header documents that it is
deliberately coarse ("any character's dogma change bumps it for everyone",
`:20-22`).

### What crosses the wire, and when

Three distinct mechanisms, and **`OnGodmaItemChange` is not one of them** — it
appears nowhere in their source.

1. **Bootstrap, inside `dogmaIM.GetAllInfo`.** The brain is the *second
   element of the two-element `charInfo` value*
   (`dogmaService/itemInfo.js:1779-1790`), built by `_buildCharacterBrain`
   `:1216` → `buildBootstrapCharacterBrain(charID, 0, {shipID, structureID})`.
   The in-code comment is load-bearing: "The client seeds charBrain
   exclusively from charInfo, and without it docked MakeShipActive later
   crashes in `RemoveBrainEffects` while switching ships" (`:1788-1789`).
   Here the three effect lists are sent as **real marshal objects**, with
   version 0.
2. **Delta push, `OnServerBrainUpdated`.** `session.sendNotification(
   "OnServerBrainUpdated", idType, [payload])`
   (`characterBrainRuntime.js:281`), payload `[version, grayMatter]`
   (`buildCharacterBrainUpdatePayload` `:238-248`). `grayMatter` is a
   **nested marshal blob** — `marshalEncode` of the three lists `:231` — i.e.
   marshal inside marshal, not a flat structure. `idType` defaults to
   `"clientID"` `:277-280` but skills push it as `"charid"`
   (`skillQueueNotifications.js:266`). `OnServerBrainUpdated` is **not** in
   their marshal string table (`common/marshalStringTable.js`), so it goes on
   the wire as a literal string.
3. **Attribute-value changes, batched into one `OnMultiEvent`.** The sub-event
   that actually carries a changed attribute is
   **`OnModuleAttributeChange`** — tuple
   `(name, ownerID, itemID, attributeID, time, newValue, oldValue, time)`
   (`services/_shared/godmaMultiEvent.js:32-44`) — and effect start/stop is
   `OnGodmaShipEffect` (11-tuple, `:46-82`). These are packed as a list of
   `(subEvent, time)` pairs into a single `OnMultiEvent` with idtype
   `clientID` (`:84-110`). Skill-driven attribute changes go out this way via
   `notifySkillDogmaAttributesChangedAsMultiEvent`
   (`skillQueueNotifications.js:269-287`). A fourth notification,
   `OnGodmaPrimeItem`, primes a whole item's attribute dict
   (`characterState.js:1608`, `drone/droneRuntime.js:1173`,
   `inventory/invBrokerService.js:2164`).

   `OnModuleAttributeChange` (60) and `OnMultiEvent` (61) *are* in the marshal
   string table (`common/marshalStringTable.js:75-76`), as is `dogmaIM` (27)
   and `OnGodmaShipEffect` (58).

### `brainEffectPickle.js`: the guess is right, and it is dead

The name is exactly what it looks like. `buildBrainEffectPickle`
(`services/dogma/brain/brainEffectPickle.js:53-88`) hand-emits **Python pickle
protocol 2 bytes**: `\x80\x02` proto header, opcode `c` (GLOBAL) with
`eve.common.script.dogma.effect\nBrainEffect\n`, then `\x29\x81` (EMPTY_TUPLE,
NEWOBJ), `\x7d\x28` (EMPTY_DICT, MARK), the eight state entries, and
`\x75\x62\x2e` (SETITEMS, BUILD, STOP) `:76-86`. It has its own primitive
encoders for ASCII strings (`U`/`T` opcodes `:3-13`), ints (`K`/`M`/`J`
`:15-30`), lists (`:32-39`) and tuples (`:41-51`).

**But nothing calls it.** `grep -rn "brainEffectPickle\|buildBrainEffectPickle"`
across all of `server/src` returns only the function's own definition
(`:53`, `:91`) and the 3-line re-export shim at
`services/dogma/brainEffectPickle.js:3`. The live path uses `marshalEncode`
instead (`characterBrainRuntime.js:16`, `:231`).

**What this means for us:** it is strong evidence that someone investigated a
pickle wire format for the brain and *abandoned it* in favour of marshal. Treat
it as a historical artefact, not as a protocol finding — but keep the file: if
our build 3619879 client turns out to want pickle for `grayMatter`, 95 lines of
already-written protocol-2 emitter is a gift. Do **not** port it as part of the
working path.

### Summary for the coordinator

| question | answer |
|---|---|
| is the brain the engine we need? | **No.** `liveFittingState.js:3058` `applyModifierGroups` is. |
| does the brain evaluate anything? | No — pure serialisation. Values are pre-resolved per skill. |
| does the brain cover modules/rigs/boosters? | **No.** Three providers: skills, industry skills, implants. |
| is stacking in the brain? | No. Client-side. |
| is stacking in the engine? | Yes, correct curve, **heuristic penalty eligibility**. |
| does `brainEffectPickle.js` run? | No. Zero callers. |
| does `OnGodmaItemChange` exist? | **No.** It is `OnModuleAttributeChange` inside `OnMultiEvent`. |

## Skill training

**Storage.** Skills are a dedicated `skills` table, not item rows
(`skillState.js:48` `readSkillsTable`, `:270` `buildSkillItemId` synthesises an
itemID from `charId` + `typeId`). This **agrees with our RE** — `docs/reference/skill-system.md`
§1 concluded for V24.01 that "skills are a subsystem beside the item store, not
a feature of it". Each record carries `trainedSkillLevel`,
`effectiveSkillLevel` and points (`:296`, `:333-335`, `:423`), where
`effectiveSkillLevel` can exceed `trained` via expert-system projection
(`applyExpertSystemProjection` `:86`).

**SP formula — confirmed identical to ours.**
`getSkillPointsForLevel(skillRank, level)` at
`services/skills/training/skillTrainingMath.js:67-76`:

```js
Math.ceil(normalizedRank * 250 * 2 ** (2.5 * (normalizedLevel - 1)))
```

Our `skill.rs:245-256` `sp_for_level` is `ceil(rank * 250 * 2^(2.5*(level-1)))`.
**Byte-for-byte the same expression**, including the `ceil` and the 250
multiplier. Both derive it from the client (`skill.rs` cites
`characterskills/util.GetSPForLevelRaw`; our `docs/reference/skill-system.md` lists
`SKILL_POINT_MULTIPLIER = 250` from `characterskills/const.py:4`). Their
constants: `MAX_SKILL_LEVEL = 5`, `DEFAULT_SKILL_RANK = 1`
(`skillTrainingMath.js:36-37`). Inverse: `getSkillLevelForPoints` `:78-86`
walks 5→1. **No contradiction found.**

**Rate.** `getBaseSkillPointsPerMinute` `:136-144`:
`primary + secondary / 2` SP per minute, where primary/secondary attribute IDs
come from the skill type's own `primaryAttribute`/`secondaryAttribute`
attributes (`:128-134`), and the five learning attributes are 164 charisma,
165 intelligence, 166 memory, 167 perception, 168 willpower (`:25-29`),
default 20 each, with **implant bonuses folded in**
(`normalizeCharacterAttributes` `:88-113` → `applyActiveImplantAttributeBonuses`).
Then `× 0.5 for Alpha / 1.0 for Omega` (`getCloneTrainingMultiplier`
`:124-126`, Omega resolved from a store licence `:115-122`), then a global
server multiplier `config.skillTrainingSpeed`
(`getEffectiveSkillPointsPerMinute` `:151-156`).

A nice trick worth stealing: because the client computes its own displayed
training estimates locally from the five attributes and knows nothing of the
server multiplier, they **scale the five attributes they report** by the same
multiplier so the client's display agrees
(`skillTrainingSpeed.js:33-45`, rationale `:17-32`). Linear in the attributes,
so the displayed SP/min scales exactly and CCP's SP thresholds stay untouched.

**Accrual: lazy-projected on read, with a one-shot settle timer.** Not ticked.

- On read, `getQueueSnapshot` (`skillQueueRuntime.js:821`) →
  `buildProjectedQueueEntries` (`:439`) computes current SP from
  `getEstimatedSkillPointsAtTime(startSP, destSP, startFileTime, now, spPerMinute, rateChanges)`
  (`skillTrainingMath.js:177-200`+). That function integrates piecewise across
  **rate changes** (a skill accelerator expiring mid-training), citing the
  client's own `characterskills/skill_training.py get_skill_points_trained_at_sample_time`
  lines 147-167 (`:172-175`).
- Level-ups are committed by `settleCharacterTraining` (`:629`), a `while`
  loop that pops completed entries off the queue.
- `scheduleQueueCompletion` (`:1253`) sets **one** `setTimeout` at the current
  entry's `trainingEndTime` (clamped to ≥1000ms, ≤2^31-1, `:1270-1271`), which
  settles then reschedules (`:1273-1278`). The handle is `unref`'d `:1279-1281`.
  If a durable scheduler is bound it defers to that instead (`:1262-1264`).
- All times are Windows FILETIME; `FILETIME_TICKS_PER_MINUTE = 600000000n`
  (`skillTrainingMath.js:34`), BigInt throughout.

**What the client is told** (`skills/training/skillQueueNotifications.js`):
`OnServerSkillsChanged` (idtype `charid`, `:333`, `:474`),
`OnServerSkillsRemoved` (`clientID`, `:341`), `OnSkillLevelsTrained`
(`clientID`, `:355`), `OnSkillsChanged` (`clientID`, `:362`),
`OnFreeSkillPointsChanged` (`charid`, `:368`), `OnNewSkillQueueSaved`
(`charid`, `:374`), `OnSkillQueuePausedServer` (`clientID`, `:380`),
`OnMultipleCharactersTrainingUpdated` (`userid`, `:450`). Plus the two dogma
pushes: the brain (`:264`) and the batched `OnMultiEvent` of
`OnModuleAttributeChange` sub-events (`:269-287`, optionally deferred to
`setImmediate` `:283-286`).

**Queue RPCs** on `skillMgrService.js`: `SaveNewQueue`, `AbortTraining`,
`GetMySkillQueue`, `GetSkillQueue`, `GetSkillQueueAndFreePoints`,
`CharStartTrainingSkill(ByTypeID)`, `CharStopTrainingSkill`, plus free-SP
application (`ApplyFreeSkillPointsToQueue`/`ToSkills`) and the respec path
(`GetRespecInfo`).

## Static data provenance

**Everything dogma-relevant comes from CCP's published SDE, build 3396210,
downloaded at setup time.** `DEFAULT_SDE_URL` is
`https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-3396210-jsonl.zip`
(`tools/DatabaseCreator/database-creator.js:35`;
`tools/DatabaseCreator/CreateDatabase.bat:7` templates the same URL from
`%SDE_BUILD%`). `EnsureSde.ps1` downloads, verifies the ZIP contains the
expected members and asserts `_sde.jsonl`'s `buildNumber` matches
(`EnsureSde.ps1:68-82`, `:155-163`). `database-creator.js` then reads
`dogmaAttributes.jsonl` (`:2605`), `dogmaEffects.jsonl` (`:2608`) and
`typeDogma.jsonl` (`:2611`) and emits `<dataDir>/typeDogma/data.json`
(`:3153`). At runtime that is read by
`readStaticTable(TABLE.TYPE_DOGMA)` → `readStaticJsonTable` →
`<dataDir>/<table>/data.json` (`services/_shared/staticJsonTableReader.js:12-19`).

| static table | where theirs comes from | evidence | can we read it from the client instead? |
|---|---|---|---|
| `typeDogma` (per-type attribute values + effect lists) | CCP public SDE `typeDogma.jsonl`, build 3396210 | `database-creator.js:2611`, `:35` | **Yes.** `res:/staticdata/typedogma.fsdbinary` — already decoded, 648,726 attribute values, 54,307 effect entries, 27,160 types (`docs/re/dogma.md:11-13`). |
| `dogmaattributes` (2,871 definitions incl. `stackable`, `defaultValue`, `highIsGood`) | CCP public SDE `dogmaAttributes.jsonl` | `database-creator.js:2605`; normalised at `services/_shared/referenceData.js:138-162` (keeps `stackable` `:156`, `defaultValue` `:152`) | **Yes.** `dogmaattributes.fsdbinary`, all 2,871 (`docs/re/dogma.md:13`). |
| `dogmaeffects` + `modifierInfo` (3,427 effects / 5,178 modifier rules) | CCP public SDE `dogmaEffects.jsonl` | `database-creator.js:2608`; modifier rows normalised `referenceData.js:110-136` | **Yes.** `dogmaeffects.fsdbinary`; 5,178 `modifierInfo` entries over 3,210 effects, 48-byte element layout decoded (`docs/re/dogma.md:291`, `:318-330`, `:342`). |
| `skillTypes` (ranks, primary/secondary attribute) | SDE `types.jsonl` filtered to category 16, plus `typeDogma` attributes 275/276/280 | `TABLE.SKILL_TYPES` `referenceData.js:34`; rank read via `getTypeAttributeValue(typeID, "primaryAttribute")` `skillTrainingMath.js:129` | **Yes** — our `docs/reference/skill-system.md` already has attributes 275/276/280 from `dogma/const.py:984-987`. |
| `hullBonusSkillAxes.json` (role-bonus skill axes per hull) | **Compiled projection of SDE `typeBonus.jsonl`**, SHA-256 pinned, 423/389/622 counts | `fitting/authority/hullBonusSkillAxes.json:1-14`; guard `hullBonusSkillAuthority.js:7-28` throws if build ≠ 3396210 | **Settled — the client has it, and more of it.** `res:/staticdata/infobubbles.static`, read by `packages/infobubbles/__init__.py:8` through `fsdlite.EveStorage`, keyed by hull typeID with `types` / `roleBonuses` / `miscBonuses`. **653 hulls and 787 (hull, skill) links** against their 423/622, and build-correct by construction rather than pinned to 3396210. Do not port the projection. `docs/port-tracker.md` §14.2 has the counts and the one thing it does not settle. |
| `certificates` | SDE | `TABLE.CERTIFICATES` `referenceData.js:48` | Probably — `certificates` is a known client table; not verified here. |
| `dynamicItemAttributes` (mutaplasmid ranges) | SDE | `TABLE.DYNAMIC_ITEM_ATTRIBUTES` `referenceData.js:25` | Likely (`dynamicitemattributes` cFSD table); not verified here. |
| `CLIENT_PYTHON_EFFECT_MODIFIER_INFO` — 6 effects whose modifiers the SDE omits | **Hand-transcribed from the decompiled client's Python effect classes** | `liveFittingState.js:1366-1548`; entries for effect IDs 3380, 6063, 4921, 6208, 12126, 848 at `:1391`, `:1437`, `:1475`, `:1496`, `:1507`, `:1538`; header comment `:1366-1376` insists "transcribed from the decompiled client's own effect class, not inferred" | **This is code, not data — portable, but must be re-derived for 3619879.** Our client's Python effect classes are a different build; the values *and* the effect IDs may differ. See Build-delta hazards. |
| `SKILL_ATTRIBUTE_BONUS_FALLBACKS` — Target Management 3429 / Advanced TM 3430 grant `maxLockedTargetsBonus` 1 | **Invented**, to paper over their own simplified skill data | `liveFittingState.js:1093-1099`; the comment admits "that bonus is not present in the simplified local skill/type data we read here" | N/A — we should not need this; our `typedogma` has the real attribute values. |
| `EFFECT_CATEGORY_OVERRIDES_BY_NAME` — `warpcorestabilizeractive` forced to category 4 | **Invented build-compat hack** | `referenceData.js:169-175`, comment: "the legacy client used by this shard only exposes the module as passive" | N/A — and a direct warning that effect *categories* move between builds. |
| `skillTrainingAlphaCaps` | invented/curated authority table | `tools/DatabaseCreator/staticTables/skillTrainingAlphaCaps/` | N/A — a design choice, not client data. |

**Bottom line for the licence question:** their dogma needs no invented data.
It is CCP's SDE, re-keyed into JSON. We can substitute the player's own client
for every dogma table we care about, because we have already decoded all three.
The two things that are *not* reproducible by reading the client are
(a) `typeBonus`/hull role-bonus axes, which needs investigating, and (b) the six
hand-transcribed Python-effect modifier sets, which are code we must re-derive
against 3619879 rather than copy.

## Dependency order

```
item location/flag model  ──┬──▶ fitting (slot validation)  ──┐
                            │                                 │
skills (state + levels) ────┼──▶ dogma engine ◀───────────────┤
                            │    (liveFittingState)           │
clones ──▶ implants ────────┘                                 │
                                                              │
                      ship state in space ───────────────────▶┘
                                   │
dogma engine ──▶ dogma brain ──▶ wire push (GetAllInfo / OnServerBrainUpdated)
```

Edges, each with its reason:

1. **item location/flag model → fitting.** Required, and it is the hard
   prerequisite. `listFittedItems(charID, shipID)` (`liveFittingState.js:670`)
   selects by owner + location + fitting flag; `isShipFittingFlag` `:139`,
   `getSlotFlagsForFamily` `:634` and `getSubsystemSlotFlag` `:458` are all
   flag arithmetic. **Fitting cannot exist without flags.** Our side has
   `item.rs`; whether it models fitting flags is outside this inventory.
2. **fitting → dogma engine.** `buildShipResourceState` takes the fitted-item
   list as its primary input (`:3480-3484`) and every module/rig/subsystem
   modifier is collected from it (`collectPassiveShipAttributeModifiers`
   `:2752`).
3. **skills → dogma engine.** `resolveDogmaSkillMapForHost` `:927` is called
   before anything is applied (`:3495`), and skill-sourced modifiers are the
   first two entries of the merged list (`:3568-3570`). A skill-less engine
   computes every hull bonus at level I (see the comment at `:3559-3562`).
4. **skills → skill training.** Obvious, but note the direction of the other
   edge: training *writes* skills and then **re-pushes dogma**
   (`skillQueueNotifications.js:264`, `:405`). So training depends on the brain
   existing, not the reverse.
5. **clones → implants.** Implants are destroyed on pod death via the clone
   runtime (`homeStationSelfDestructRuntime.js:128` calls
   `jumpCloneRuntime.clearActiveImplantsForPodDeath`) and a clone jump swaps
   the active implant set and re-pushes dogma
   (`jumpCloneRuntime.js:472-474`). **Implants are implementable without
   clones** (a single implant set per character), but implants-with-clones
   requires the clone model first. `jumpCloneRuntime.js` is **outside this
   area** (`services/station/`).
6. **implants → dogma engine, and implants → skill training.** Two separate
   consumers. The engine takes implant ship-domain modifiers
   (`liveFittingState.js:3501-3506`), and training rate reads implant bonuses
   on the five learning attributes (`skillTrainingMath.js:94` →
   `applyActiveImplantAttributeBonuses`). So **a learning implant affects SP/min
   and must exist before training rates are correct.**
7. **ship state in space → dogma engine (soft).** Not required for docked
   attribute computation — `buildShipResourceState` works on store rows alone —
   but required for the *runtime* half: heat
   (`activationHeatState.js`), damage/charge
   (`shipConditionAmounts.js:8`, attributes 3/18/264/266), module activation
   cycles and overload (`moduleOperation.js`, `overload.js`), and targeting
   (`targeting.js`). **Docked dogma first, space dogma second** is a clean
   split and their code supports it (`isDockedSession` branches throughout
   `itemInfo.js`).
8. **dogma engine → dogma brain.** The brain resolves each skill's effective
   attribute values through `buildSkillEffectiveAttributes`, which is an engine
   export (`fittingBrainBuilder.js:14-18` imports it from `liveFittingState`).
   So the brain cannot be ported first.
9. **dogma brain → `GetAllInfo`.** Hard, and non-obvious: a missing brain
   crashes the client later, not at login — "docked `MakeShipActive` later
   crashes in `RemoveBrainEffects` while switching ships"
   (`itemInfo.js:1788-1789`). **Any `GetAllInfo` we ship must carry a brain,
   even an empty one.**
10. **character record → everything.** `characterState.js` holds the five
    attributes, active ship and structure (`resolveCharacterBrainTargetIDs`
    reads it `characterBrainRuntime.js:166-179`). Already present on our side
    (`service/character.rs`).
11. **saved fittings → nothing.** `char/corp/allianceFittingMgrService` are
    pure CRUD over a blob store with no dogma dependency. Portable in isolation
    at any time.
12. **settings → nothing.** Two RPCs, no dependencies.

**Recommended port order:** item flags → skills state → dogma engine
(`applyModifierGroups` + operation order + stacking) → `GetAllInfo` attribute
dict → brain serialiser → skill training queue → implants → space/runtime
dogma (heat, activation, overload, targeting) → clones.

## Effort to port, per subsystem

| subsystem | effort | why | risk |
|---|---|---|---|
| Dogma engine core (`applyModifierGroups`, operation order, `resolveModifierBaseValue`) | **M** | ~250 lines of pure arithmetic over data we already have; no I/O, no wire format. The algorithm, not the code, is the deliverable. | Operation enum. Their order places `5` after `6` and treats it as reciprocal (`:3131-3135`); our `docs/re/dogma.md:382` says the numeric meaning of `operation` is only partly settled ("Observed `{-1:18, 0:649, 2:213, ...}`", and `:394-396` explicitly lists "How the client composes modifiers … stacking-penalty order, the role of `operation`" as living in the native engine, undecoded). **If their order is wrong, every ship stat is quietly off and nothing errors.** Validate against a real client's own fitting window numbers before trusting it. |
| Stacking penalties | **S** | `exp((i/2.67)^2)` plus a two-chain sort. ~40 lines. | Penalty *eligibility* is a caller-supplied heuristic (`sourceKind === "fittedModule"`), not effect data (`:1848-1850`). Wrong for any skill or implant that really is penalised, and for modules that really are not. Silent: the fit still computes, just with wrong numbers. |
| Modifier collection from `modifierInfo` (6 `func`s × 7 `domain`s) | **M** | Their `appendDirectModifierEntries` `:1829` / `appendLocationModifierEntries` `:1976` are a readable reference, and we have all 5,178 rules decoded. | They implement **five** `func`s, not six, and **three** domains, not seven (`industryBrainProvider.js:48-95`, `fittingBrainBuilder.js:118-128`). Whatever they drop, we would be dropping too — and we have the full vocabulary (`docs/re/dogma.md:347`, `:489`: closed sets of 7 and 6). Port the *shape*, not the coverage. |
| Ship attribute composition (`buildShipResourceState`) | **L** | 200+ lines of ordering subtleties, each one a bug someone already paid for: the skill-adjusted self-modifier snapshot (`:3563-3567`), the `defaultValue` base fallback (`:3031-3056`), strategic-cruiser subsystem filtering (`:3488-3493`). | Depends on our fitted-item/flag model, which may not exist yet. Also depends on getting hull role bonuses from somewhere (see `typeBonus` row above). |
| Dogma brain serialiser | **M** | 316 lines, mostly mechanical, and the `BrainEffect` state-dict shape is spelled out (`characterBrainRuntime.js:78-86`). | The marshal-in-marshal `grayMatter` nesting (`:231`) and the `objectex1` encoding must match *our* client build. Their shape is for 3396210. `OnServerBrainUpdated` is not in any string table, so at least that part is build-stable. Also: must handle the version counter properly — theirs resets on restart (`:27`). |
| `GetAllInfo` attribute dict | **L** | `itemInfo.js` is 2,130 LOC and `Handle_GetAllInfo` alone spans ~400 with a dozen conditional bootstrap shapes (docked vs in-space vs ship-only vs controlled-structure). | The response shape is the single highest-risk wire surface in the area. Their comments cite "golden captures" and a V24.01 client raising `AttributeError` on missing `activeShipID`/`shipState`/`charInfo` (`:1769-1771`) — our build is a *different* V24.01 point release (3619879 vs 3396210) and field ordering may have moved. |
| Skill state + SP formula | **S** | We already have `sp_for_level` and it matches theirs exactly. Their `skillState.js` adds only record normalisation. | Low. The formula is client-derived on both sides. |
| Skill training queue | **M** | ~1,850 LOC but the design is simple and good: project on read, one settle timer, FILETIME BigInt. Reimplementing is faster than porting. | Rate-change integration across accelerator expiry (`skillTrainingMath.js:177`) is fiddly; getting it wrong gives wrong *estimates*, which the client displays, so it is visible rather than silent. Alpha/Omega gating (`:124-126`) is irrelevant to us and should be dropped. |
| Implants + boosters | **M** | 1,769 LOC, but `activeImplantModifiers.js` contains a *third* copy of the modifier applier (`applyDogmaModifierGroups` `:235`) that should collapse into the one engine. | Slot attribute 331 / booster slot 1087 are hardcoded (`:57-58`) — attribute IDs, exactly the build-fragile class. Implants also feed training rate, so an error here mis-scales SP/min. |
| Overload / heat | **S** | 242 LOC, well-documented, three attributes. | Attribute IDs 1175/1176/1177 hardcoded (`activationHeatState.js:7-21`). |
| Module activation / cycles | **XL** | `moduleOperation.js` is 4,170 LOC and is really the in-space combat loop, not a dogma feature. | Out of scope for a docked-first port. Defer. |
| Saved fittings (char/corp/alliance) | **S** | 654 LOC of CRUD, three near-identical copies. One implementation, three scopes. | Low. Blob shape must round-trip through the client's fitting window. |
| Standings | **M** | 2,353 LOC, self-contained. | Irrelevant to dogma; schedule independently. |
| Portraits / paperdoll | **M** | 1,116 LOC, mostly payload plumbing and image storage. | Independent. |
| Certificates | **S** | 768 LOC over an SDE table; derived from skill levels. | Needs skills first. Low risk. |
| Expert systems | **M** | 1,616 LOC. Folds into skill reads at one point (`skillState.js:86`). | A live-service monetisation feature. **Recommend skipping** for a co-op server — but note the hook point, because `effectiveSkillLevel` vs `trainedSkillLevel` exists *because* of it, and the brain reads `effectiveSkillLevel` first (`fittingBrainBuilder.js:103-108`). |
| Skill injectors/extractors, skill plans, char deletion, access groups, achievements, milestones, seasons, project discovery | **skip** | ~8,000 LOC of live-service surface with no bearing on co-op/PvP with bots. | — |
| Shared settings | **S** | Trivial, 2 RPCs. | Low. |

## Build-delta hazards

Their whole area is pinned to **SDE build 3396210**; we target client
**3619879**. Attribute and effect IDs are exactly the class of identifier that
shifts between builds, and this codebase hardcodes a great many of them.

**Hard-pinned build number (will throw on our data).**
`hullBonusSkillAuthority.js:9` `EXPECTED_SDE_BUILD = 3396210`, with two
SHA-256 pins (`:10-13`) and three exact counts (`:14-18`); the module throws at
require time if any differ (`:20-28`). Any port of hull role bonuses must
replace this guard wholesale, not adjust it.

**The mitigation they use, and it is the right one: name lookup with a numeric
fallback.** `getAttributeIDByNames("speed") || 51`
(`_secondary/fitting/fittingBrainBuilder.js:31-39`, and ~30 more in
`dogmaService.js:189-264`).
The *name* is resolved against the loaded dogma attribute table and the
literal is only a fallback. **Port this pattern, and make the fallback a hard
error rather than a silent default** — a silently-wrong attribute ID is the
worst failure mode in this whole area, because the fit still computes.

**Hardcoded attribute IDs** (non-exhaustive):

- learning: 164 charisma, 165 intelligence, 166 memory, 167 perception,
  168 willpower (`skillTrainingMath.js:25-29`, `characterOwnAttributes.js:7-13`,
  `skillTrainingSpeed.js:10`, `activeImplantModifiers.js:52-56`)
- implant slot 331, booster slot 1087 (`activeImplantModifiers.js:57-58`)
- heat 1175/1176/1177 (`activationHeatState.js:7-21`)
- condition 3 damage, 18 charge, 263 shieldCapacity, 264 shieldCharge,
  265 armorHP, 266 armorDamage, 9 structureHP (`shipConditionAmounts.js:15-20`)
- crystals 9, 783, 784, 786 (`crystalDurability.js:14-19`)
- industry: 196, 219, 385, 387, 398, 467, 1959, 2662, 2664 — listed **twice**,
  once as named constants (`brain/providers/industryBrainProvider.js:18-26`,
  collected `:42-52`) and once as a bare `Set` with no names at all
  (`_secondary/fitting/fittingBrainBuilder.js:20-30`), used there to *exclude*
  industry attributes from fitting brain effects
  (`shouldIncludeCharacterTargetAttribute` `:113-115`)
- damage/RoF: 64 damageMultiplier, 114/116/117/118 EM/Exp/Kin/Therm,
  292 damageMultiplierBonus, 293 rofBonus, 51 speed
  (`_secondary/fitting/fittingBrainBuilder.js:31-39`)
- ours, for cross-check: 275 skillTimeConstant, 276 skillPoints,
  280 skillLevel (`docs/reference/skill-system.md`, from `dogma/const.py:984-987`) —
  theirs reads 275 via the name `"primaryAttribute"`/rank path instead.

**Hardcoded effect IDs.** The six Python-effect modifier transcriptions:
**3380, 6063, 4921, 6208, 12126, 848** (`liveFittingState.js:1391`, `:1437`,
`:1475`, `:1496`, `:1507`, `:1538`). Plus `getEffectIDByNames("moduleBonusAfterburner") || 6731`
and `|| 6730` for microwarpdrive (`dogmaService.js:262-264`). **Each of the six
must be re-derived from our client's own `pythonEffects/*.py`, not copied** —
and the effect might not even have an empty `modifierInfo` in 3619879, in which
case the override must not fire (their guard at `:1565-1568` handles that:
"If CCP ever ships the rows, theirs win").

**Hardcoded skill type IDs.** 12 industry skills 3380/3387/3388/3402/3403/
3406/3409/24624/24625/45746/45748/45749
(`brain/providers/industryBrainProvider.js:28-39`);
targeting skills 3429/3430 with an *invented* bonus value
(`liveFittingState.js:1093-1099`). Skill type IDs are more stable than
attribute IDs historically, but 3380 appears as **both** a skill type ID
(Industry) and an effect ID in the Python-effect map — a collision waiting to
confuse anyone porting carelessly.

**Hardcoded category/group IDs.** Implant category 20
(`implantRuntime.js:36`), overload effect category 5
(`liveFittingState.js:1909`), passive modifier effect categories `{0, 4}`
(`:1097`), skill category 16 (ours: `inventorycommon/const.py:357`).
`EFFECT_CATEGORY_OVERRIDES_BY_NAME` forcing `warpcorestabilizeractive` to
category 4 (`referenceData.js:169-175`) is itself direct evidence that effect
categories differ between their SDE build and their client build — so expect
the same mismatch, differently placed, for ours.

**Stacking constants.** `2.67` and the 8-element denominator array
(`liveFittingState.js:1106-1108`). These are CCP's long-stable constants and
are the *least* likely thing here to have moved; the hazard is the eligibility
heuristic, not the curve.

**Dogma operation numbering.** The 9-value order (`:3178-3188`) with `5`
inserted between `6` and `8`. Our own RE flags the operation enum as the one
remaining gap (`docs/re/dogma.md:382-389`, `:547-551`). **Theirs is a
hypothesis, not a decode.** Treat it as a strong hint to test, not as an
answer.

**Wire-shape hazards.** `objectex1` encoding of `BrainEffect`
(`characterBrainRuntime.js:67-87`), the marshal-in-marshal `grayMatter`
(`:231`), the two-element `charInfo` tuple (`itemInfo.js:1779-1790`), the
`OnMultiEvent` list-of-`(subEvent, time)`-pairs (`godmaMultiEvent.js:84-98`),
and the exact 11-tuple of `OnGodmaShipEffect` (`:46-82`). All of these were
derived from 3396210 captures, and the one document that recorded those
captures is **not in the release**.

## Notes for the coordinator

**The headline, stated plainly.** We came looking for an engine and there is
one, but it is not where the brief expected. **`services/fitting/liveFittingState.js`
is the dogma engine; `services/dogma/brain/` is a serialiser for the client.**
If only one file from this entire area gets ported, it is
`applyModifierGroups` (`liveFittingState.js:3058`) together with the merged
single-pass composition at `:3568-3581`. That is the piece our
`docs/re/dogma.md:579` named as the gap — "No dogma *engine*: stacking
penalties, …" — and it is ~300 lines of arithmetic, not a system.

**The pickle guess was right, and the file is dead.** `brainEffectPickle.js`
really is a hand-rolled Python pickle protocol-2 emitter for
`eve.common.script.dogma.effect.BrainEffect` (`:75-87`), and it has **zero call
sites** in 870k lines. The live path uses marshal. Read that as: someone tried
pickle for the brain and it was not what the client wanted. Keep the file as a
spare part; do not treat it as a wire-format finding.

**`OnGodmaItemChange` does not exist in their code.** The notification that
carries a changed attribute is `OnModuleAttributeChange`, batched with
`OnGodmaShipEffect` into a single `OnMultiEvent`
(`services/_shared/godmaMultiEvent.js:32-44`, `:84-110`). Worth correcting in
whatever notes said otherwise.

**Three copies of the modifier applier.** `liveFittingState.applyModifierGroups`
(`:3058`), `activeImplantModifiers.applyDogmaModifierGroups` (`:235`), and
`homefrontWeather.js`/`structureFullPowerDogma.js` each carrying their own
`stackingPenalized` conventions. We should port **one** and give it every
caller. Their duplication is also why their stacking eligibility is
inconsistent.

**Their brain covers far less than the name suggests.** Three providers:
character skills, industry skills, implants (`characterBrainRuntime.js:28-32`).
No module, rig, subsystem, booster, or fleet provider exists. So on their
server a fitted module's bonus reaches the client only through the *attribute
values* in `GetAllInfo`/`OnModuleAttributeChange`, never as a client-side
modifier rule. **That is a legitimate architectural choice and probably the
right one for us too** — compute authoritatively on the server, ship resolved
numbers, and keep the brain minimal (skills + implants) because the client
crashes without one.

**Zero tests, and the evidence trail has a hole.** `find` for test files over
the whole repo returns 0. Several of the most delicate claims in the code —
the `OnMultiEvent` shapes, the "golden capture" field ordering in
`GetAllInfo` — cite `doc/PARITY_FITTING_NOTIFICATION_SEQUENCE.md`
(`godmaMultiEvent.js:8`), which is **not shipped**. The in-code comments are
unusually good and often cite the client's own Python by file and line, so
they are worth reading; but every wire shape should be re-verified against
3619879 before it is relied on.

**Good ideas worth stealing outright**, independent of whether we port their
code:

1. Single-pass application ordered globally by dogma operation, not by source
   (`:3551-3554` and its rationale).
2. Falling back to the SDE's own `defaultValue` for an undeclared attribute
   rather than to 0 or 1 (`:3031-3056`, with the Bastion bug that forced it).
3. Pre-multiplying a hull's per-level bonus attributes by skill level *before*
   the ship effect reads them (`:3559-3567`).
4. Scaling the five learning attributes reported to the client by the server's
   training-speed multiplier, so the client's locally-computed estimates agree
   (`skillTrainingSpeed.js:17-45`).
5. Resolving every attribute ID by *name* with a numeric fallback
   (`getAttributeIDByNames(...) || N`), which is the only reason this codebase
   has any chance against a build delta. **Make the fallback fatal in ours.**

**What to look at next, in order:**

1. `liveFittingState.js:3058-3245` (`applyModifierGroups`) and `:2978-3016`
   (`getStackedMultiplierFactor`) — read these two in full. They are the
   deliverable.
2. `liveFittingState.js:3474-3660` (`buildShipResourceState`) — the
   composition order and its comments.
3. Settle the **operation enum** (`docs/re/dogma.md:382-389`). Their ordering
   is the best available hypothesis and we can test it: compute a known fit
   both ways and compare against the client's own fitting window.
4. ~~Decide whether `typeBonus` / hull role-bonus axes can be read from the
   client's cFSD set.~~ **Settled: yes, and richer than theirs** —
   `res:/staticdata/infobubbles.static`, 653 hulls / 787 skill links against
   their 423/622. `docs/port-tracker.md` §14.2.
5. Check whether our `item.rs` models fitting flags. Everything in the fitting
   and module half of this area is blocked on it
   (`liveFittingState.js:139`, `:634`, `:670`).
6. Re-derive the six Python-effect modifier sets (`:1366-1548`) against
   3619879's `pythonEffects/*.py`. Their transcription method is sound and
   documented; the values are build-specific.

**Guesses flagged as guesses:** that `skillPlanState.js`/`corpSkillPlanState.js`
are reached from outside `services/skills` (no handler found, not chased);
that `dynamicItemAttributes` and `certificates` have cFSD equivalents in our
client (plausible from the table names, not verified); that their two-chain
stacking split matches CCP exactly (it matches pyfa's convention, which is
itself reverse-engineered). The claim that `brainEffectPickle.js` is dead is
**not** a guess — it is a complete grep over `server/src`.
