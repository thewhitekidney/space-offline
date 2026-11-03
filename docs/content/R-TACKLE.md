# R-TACKLE: warp scramblers and disruptors, an implementation spec

Research lane R-TACKLE (W12, `docs/content/plan.md` §6.2), research only
(lane rule 7). Client build **3619879** (`tq-offline`), read through
`tools/analysis.sh` (`tools/dis.py`, `tools/call_lines.py`, `tools/dogma.py`
imported in the container); the attribute and modifier rows come from the
local extract `out/typedata.tsv`, and the EffectStopper targets, which
`typedata.tsv` does not carry, from `dogmaeffects.fsdbinary` through
`tools/dogma.py`. Nothing installed on the host, no cargo. Wrote only this
file. Tree read at `b3e950c`.

Marks as `docs/content/README.md` §2: **[V]** this build's bytecode or data
(file:line from `co_lnotab`, or table and field), **[V2]** EvEJS facts,
**[I]** inferred (the inference is stated), **[A]** an assumption we ship
(with its falsifier), **unknown**. Client line numbers are `tools/dis.py`
line numbers, never uncompyle6's.

Feeds CP57 (player scram and disrupt, `src/tackle.rs`) and CP49 (NPC tackle,
`src/npc_ewar.rs`).

---

## 0. The answer in one screen

1. **One attribute decides warp: the target ship's `warpScrambleStatus`
   (104).** Every tackle effect adds its source's `warpScrambleStrength`
   (105) to it with `modAdd` (operation 2); every warp core stability source
   (the hull, an active stabiliser, capital role bonuses) does the same
   with a negative number. **The ship may not warp while it is `> 0`**
   [I, the client's own test at `autopilot.py:590`].
2. **No stacking penalty.** `modAdd` is not a nurfable operation and 104 is
   `stackable` [V]. Strengths add linearly. A Venture (base −2) under two
   Warp Disruptor I (1 each) is at 0, which is not `> 0`, so it can still
   warp. It takes a total of 3: three disruptors, or one disruptor plus one
   scrambler.
3. **The refusal is `UserError('WarpScrambled', {})`**: no arguments, text
   "External factors are preventing your warp and/or jump drive from
   responding to this command." [V key, text and empty args; I that the
   server raises this key, §3].
4. **A warp scrambler blocks microwarpdrives; a disruptor blocks nothing
   but warp.** The scrambler's effect (5934) raises `activationBlocked`
   (1349) on every target module that requires High Speed Maneuvering
   (3454). A blocked module refuses activation with
   `UserError('ModuleIsBlocked', {'moduleName': (UE_TYPEID, typeID)})` [V,
   `fittingDogmaLocation.py:612-613`]. The player scrambler's data does
   **not** reach the micro jump drive (§4.2; [A] that it blocks MJD anyway).
5. **A plain scram does not stop a stargate jump** [I]. Gates have their
   own attribute pair, `gateScrambleStatus` (1973, default −1000) and
   `gateScrambleStrength` (1974), which only the HIC focused scripts raise
   and which only capital hulls start at 0 (§5). **This corrects CP57's
   test row**, which asks a scram to refuse a gate jump.
6. **The HUD learns from three pushes**: `OnSpecialFX` with guid
   `effects.WarpScramble`/`effects.WarpDisrupt` (beam plus combat-log
   lines, already sent by `modules::activate`), a server-pushed
   `OnEwarStart(sourceBallID, moduleID, targetBallID, jammingType)` with
   `jammingType` `'warpScramblerMWD'` or `'warpScrambler'` (the buff-bar
   icon), and `OnModuleAttributeChange` for 104 on the target ship (what
   the autopilot reads) (§6).
7. **It needs no new state for players.** `Modules::running_against(ship)`
   already lists every effect aimed at a ship; the guard is a pure sum over
   it (§7). CP57 needs no `SLOT` and no logoff hook. Two coordinator hunks
   are needed: `space.rs` must raise the warp refusal it now swallows, and
   `modules::activate` needs an activation-block check.

---

## 1. Ids [V]

### 1.1 Attributes (`dogmaattributes`; `tools/dogma.py`)

| id | name | default | stackable | highIsGood | dataType | role |
|---:|---|---:|:-:|:-:|---:|---|
| 103 | `warpScrambleRange` | 0 | 1 | 1 | 4 | an NPC's range (the `rangeAttributeID` of 563, 2481, 3713, 5928) |
| **104** | **`warpScrambleStatus`** | **0** | 1 | 1 | 4 | **the target ship's warp core state; the per-hull warp core strength is its negative base value** |
| **105** | **`warpScrambleStrength`** | 0 | 1 | 1 | 4 | what a source adds to 104 (a stabiliser carries a negative one) |
| 505 | `warpScrambleDuration` | 8000 | 1 | 1 | 5 | an NPC effect's duration (563, 2481, 3713, 5928) |
| 504 | `entityWarpScrambleChance` | 0 | — | — | — | NPC usage chance (C6 §2.5) |
| 1349 | `activationBlocked` | 0 | 1 | 1 | 4 | on a **module**: non-zero refuses activation |
| 1350 | `activationBlockedStrenght` (sic) | 0 | 1 | 1 | 4 | what a scrambler adds to 1349 (1.0 on every player scrambler) |
| 1470 | `maxVelocityMultiplier` | 0 | 1 | 1 | 5 | post-assigned onto the target by 5934, 6848, 6849; consumer **unknown** (no client reader) |
| 1973 | `gateScrambleStatus` | −1000 | 1 | 1 | 4 | the gate analogue of 104 (§5) |
| 1974 | `gateScrambleStrength` | 1 | 1 | 1 | 4 | what an HIC script adds to 1973 |
| 2509 / 2510 | `behaviorWarpScrambleStrength` / `behaviorWarpDisruptStrength` | 0 | — | — | — | the `behavior*` NPC family's 105 |
| 5694 | `microJumpBlocked` | 0 | 0 | 0 | 4 | no type carries it and no effect writes it [V]; ignore |

Client constant: `attributeWarpScrambleStatus = 104` (`dogma/const.py:1107`,
cited in `docs/re/warp.md` §6.1).

### 1.2 Effects (`dogmaeffects`)

| id | name | cat | duration attr | range attr | guid (python effect) | modifiers [V] |
|---:|---|:-:|---:|---:|---|---|
| **39** | `warpDisrupt` | 2 target | 73 | 54 | `effects.WarpDisrupt` | target 104 += own 105 |
| **5934** | `warpScrambleBlockMWDWithNPCEffect` | 2 | 73 | 54 | `effects.WarpScramble` | target 104 += 105; target's modules requiring **3454** get 1349 += 1350; target 1470 := 1470 (op 7); EffectStopper 6441, 6442 (fighter MWD/MJD abilities) |
| 563 | `warpScrambleForEntity` | 2 | 505 | 103 | `effects.WarpScramble` | target 104 += 105 |
| 5928 | `warpScrambleTargetMWDBlockActivationForEntity` | 2 | 505 | 103 | `effects.WarpScramble` | 104 += 105; 1349 += 1350 for modules requiring 3454 **and 4385**; EffectStopper 6441, 6442 |
| 6745 | `behaviorWarpScramble` | 2 | 2506 | 2507 | `effects.WarpScramble` | 104 += 2509; 1349 += 2509 for 3454; EffectStopper 6441, 6442 |
| 6744 | `behaviorWarpDisrupt` | 2 | 2503 | 2504 | `effects.WarpDisrupt` | 104 += 2510 |
| 2481, 3713 | `warpScrambleForStructure`, `concordWarpScramble` | 2 | 505 | 103 | `effects.WarpScramble` | 104 += 105 |
| 6848 | `shipModuleFocusedWarpScramblingScript` (HIC) | 2 | 73 | 103 | `effects.WarpScramble` | 104 += 105; 1349 += 1350 for 3454 and 4385; 1470 := 1470; **1973 += 1974**; EffectStopper 6441, 6442, **4921**, **6208** |
| 6849 | `shipModuleFocusedWarpDisruptionScript` (HIC) | 2 | 73 | 103 | `effects.WarpDisrupt` | 104 += 105; 1470 := 1470; **1973 += 1974** |
| **8121** | `warpCoreStabilizerActive` | **1 activation** | 73 | — | `effects.Invulnerability` | own ship 104 += own 105 |
| 6730 | `moduleBonusMicrowarpdrive` | 1 | 73 | — | `effects.MicroWarpDrive` | (Python) |
| 4921 | `microJumpDrive` | 1 | 73 | — | `effects.MicroJumpDriveEngage` | (Python) |

Every one of 39, 5934, 563, 5928, 6744, 6745, 6848, 6849 is `isOffensive`;
none is `electronicChance`/`propulsionChance`/`rangeChance` [V]. 3713
(CONCORD) is not offensive.

Other writers of 104, all `modAdd` on the own ship [V]: cyno/jump portal/
clone-jump/industrial core/siege/triage/bastion effects (by 852), the
networked/integrated sensor arrays (by 105), capital role bonuses
6591-6594, 6647-6650, mass entangler 7142. Writers of 105 on the source
module: `roleBonusJustScramblerStrength` 6358 (modAdd by 2091, modules
requiring 3449), `eliteBonusJustWarpScramblerStrengthBonusBlops3` 12569,
`shipBonusWarpScramblerStrengthG5` 11452 (postPercent, group 52).

### 1.3 Types, the numbers a fixture should mirror (not copy)

| type | group | default effect | 105 | 54 range | 73 cycle | 6 cap | 1350 |
|---|---:|---:|---:|---:|---:|---:|---:|
| Warp Disruptor I (3242) | 52 | 39 | 1 | 20,000 | 5,000 | 25 | — |
| Warp Disruptor II (3244) | 52 | 39 | 1 | 24,000 | 5,000 | 30 | — |
| Warp Scrambler I (447) | 52 | 5934 | 2 | 7,500 | 5,000 | 5 | 1 |
| Warp Scrambler II (448) | 52 | 5934 | 2 | 9,000 | 5,000 | 6 | 1 |
| Heavy Warp Scrambler I (40750) | 52 | 5934 | 6 | 8,500 | 5,000 | 40 | 1 |
| Warp Core Stabilizer I (10998) | 315 | **8121** | **−2** | — | 10,000 | 80 | — |
| 5MN Microwarpdrive I (434) | 46 | 6730 | requires skill (182) **3454** |||||
| Medium Micro Jump Drive (33915) | 1189 | 4921 | requires skill (182) **4385** |||||

The Warp Core Stabilizer is an **active** module in this build: default
effect 8121 (category 1), `disallowRepeatingActivation` (1014) = 1,
`moduleReactivationDelay` (669) = 150,000 ms [V]. Its −2 counts only while
it runs.

**Per-hull warp core strength** = −(the hull's own 104) [V]. Over the 570
category-6 types: 562 have 0 (absent or 0.0), 8 have −2 (Venture, Venture
Consortium Issue, Metamorphosis; Bustard, Impel, Mastodon, Occator,
Torrent), and NPC/special hulls go to −10 … −1000 (Orca −10, the four
empire freighters −1000). The Prospect and the blockade runners carry no
104 in this build.

NPC scram types (C6): 563 on 318 types, with 105 = 1 on the rows checked
(e.g. 2845 *Outuni Mesen*: 103 = 24,000, 105 = 1, 505 = 5,000, 504 = 1).

---

## 2. The rule

```
status(ship) = sheet(ship).ship(104)                  // hull base + own WCS (active)
                                                      //  + own-ship role/module effects
             + Σ over r in modules.running_against(ship)
                   where r.effect.effect_id ∈ TACKLE_EFFECTS
                   and r is not past its stop_at:
                   strength(r)                        // the source's modified 105
                                                      //  (2509/2510 for 6745/6744)
warp refused        ⇔ status(ship) > 0
MWD activation refused ⇔ Σ 1350 of running 5934/5928/6745/6848 against ship > 0
                          and the module requires skill 3454
```

- **Additive, unpenalised** [V]. `const.dgmNurfableOperations` is {preMul,
  postMul, postPercent, preDiv, postDiv}; `modAdd` (2) is not in it, and
  104 is `stackable` anyway (`docs/lanes/dogma-engine.md` §1, the
  `stackingNurfedAttribute._ShouldNotNurf` row). No eight-cap, no order.
- **`> 0`, not `>= 0`** [I]. The only client reader of 104 is the
  autopilot's explanation: `if item.warpScrambleStatus > 0` at
  `eve/client/script/parklife/autopilot.py:590` (in `AP_WarpToDestID`,
  `:572`). With the hull default 0, one disruptor (1) refuses; a Venture
  (−2) needs a total strength of 3. Falsified by a Venture that cannot
  warp under one Warp Scrambler I.
- **The source's 105 is its modified value**: `dogma::sheet(source_ship)
  .module(module_id, 105)`, which picks up 6358/11452/12569. Without a
  sheet (no `--typedata`), the type's own 105 [A].
- **When it applies** [I]: for as long as the effect runs, from the cycle's
  start to its stop, the way every target effect in `bdl` is a started
  effect's modifiers. Our `Running` holds exactly that interval.
- **Range** [A]: a running tackle module whose target is beyond the
  module's modified `maxRange` (54; 103 for an NPC effect) at a cycle
  boundary stops, like a lost lock at `modules.rs:1483`. Not in the client.
  Falsified by a live client showing a cycle that runs on out of range.

`TACKLE_EFFECTS` = {39, 5934} for players (CP57) and {563, 5928, 6744,
6745} for NPCs (CP49). 2481, 3713, 6848, 6849 have no carrier we spawn.

---

## 3. The warp refusal

**Key `WarpScrambled`**, fsd dialog 1682, `notify`, body message 258783,
**no loc args** [V, `findings/user-errors.tsv:4209`; text resolved with
`tools/dogma.py`]:

> External factors are preventing your warp and/or jump drive from
> responding to this command.

The client raises no such key itself, so the server is its source [I]. The
match is by text, and nothing else in the vocabulary fits: the siblings
are `WarpDisrupted` (`object`, `range`: "You are within a warp disruption
zone. Get {range} meters from {object} to warp.", the bubble, out of
scope), `WarpScrambledByWarpDisruptionFieldGenerator` (also bubbles) and
`GateScrambled` (§5).

How the client takes it [V]:

- **Autopilot** (`CmdWarpToStuffAutopilot`), `autopilot.py:587-595`:
  `except UserError` at `:587`, `sys.exc_clear()` at `:588`, reads
  `godma.GetItem(session.shipid).warpScrambleStatus` at `:589-590`, and if
  `> 0` turns the autopilot off with "Autopilot cannot warp while warp
  scrambled." at `:591`; `'WarpDisrupted' in e.msg` gives the bubble line
  at `:592-593`. **It never shows the dialog**, so the autopilot path is
  only right if 104 on the client's ship item is current (§6.3).
- **Right-click and keyboard warps** (`CmdWarpToStuff`) have no catcher;
  an uncaught `UserError` reaches `eve_exception_handler.handle_exception`
  and is shown by `prompt_player_fsd_dialog(exc.msg, exc.dict)`
  (`eve_exception_handler.py:47-48`; the chain is `docs/re/errors.md`
  §1.3). **So raising is safe**, and the comment above
  `service/space.rs::warp` that keeps refusals silent for fear of an
  unhandled exception is answered by that chain.

Wire: `Refusal { user_error: "WarpScrambled", args: vec![] }`.

---

## 4. What blocks MWD and MJD

### 4.1 Microwarpdrive [V data, V client rule]

- **Warp Scrambler (5934)** and the NPC effects **5928** and **6745**:
  `LocationRequiredSkillModifier` on the target ship, skill **3454** (High
  Speed Maneuvering), `activationBlocked` (1349) += `activationBlockedStrenght`
  (1350, which is 1.0 on every player scrambler; 6745 uses 2509).
- **Warp Disruptor (39)**, 563, 6744: no 1349 modifier, so the **MWD stays
  usable** under a disruptor.
- The client's own pre-check, in its fitting simulator, is the rule the
  server applies [V]: `fittingDogmaLocation.StartEffect_PreChecks`
  (`:607`): for an activation or target effect on an online module,
  `if GetAttributeValue(itemID, attributeActivationBlocked) != 0`
  (`:612`) `raise UserError('ModuleIsBlocked', {'moduleName': (UE_TYPEID,
  typeID)})` (`:613`). Text (257706): "External factors are preventing
  your {moduleName} from responding to this command".
- **A running MWD when the scram lands** [A]: stop it at once with
  `modules::stop(w, mwd, now, Some(('ModuleIsBlocked', dict)))`. The data
  has no EffectStopper for 6730 (5934's two stoppers are fighter
  abilities), so the live server must do it in Python (`effects.WarpScramble`
  is a server-side effect). Falsified by a live client whose MWD keeps
  cycling under a scram.

### 4.2 Micro jump drive

- **Data** [V]: only **5928** (NPC), **6848** (HIC script) touch modules
  requiring 4385 (Micro Jump Drive Operation), and only 6848 stops a
  running `microJumpDrive` (4921) and `microJumpPortalDrive` (6208). The
  player's **5934 does not** name 4385.
- **[A] a player scrambler blocks MJD too** (5934 treated as 5928). It is
  the TQ behaviour players expect, and the server-side Python effect is
  where CCP could be doing it. Falsified by a recorded live session where
  an MJD activates under a Warp Scrambler I. A disruptor never blocks the
  MJD.
- Moot until an MJD exists: no file in `src/` handles 4921 today. CP57
  writes the rule and a test over a fixture module that requires 4385.

---

## 5. Gate jumps and docking

- **A scram or a disruption does not refuse a stargate jump** [I]. The
  data gives gates their own pair: `gateScrambleStatus` (1973) defaults
  to **−1000**, set to **0 only on capital hulls** (groups 30, 485, 547,
  659, 883, 1538, 4594, 5120 [V]), and only the HIC scripts 6848/6849 add
  `gateScrambleStrength` (1974 = 1) to it. A design where 104 also refused
  gates would not need the second pair. The `WarpScrambled` text's "jump
  drive" is the capital jump drive (`jumpDrive*`, 866-899), not a gate.
  EvEJS's ordered stargate checks agree: weapons timer, then
  "capital hull + scripted-HIC gate scramble", and no scram check [V2,
  `docs/evejs/review/features.md` rows 11-12]. Falsified by a live client
  that cannot jump a gate while pointed and with no weapons timer.
- So `JUMP_GUARDS` gets **`tackle::gate_scrambled`**: refuse when
  `gateScrambleStatus` (hull 1973 + Σ 1974 of running 6848/6849) `> 0`,
  with **`UserError('GateScrambled', {})`** (dialog 4657, body 504506,
  no args: "External factors are preventing your ship from activating
  the gate.") [V key and text; I raise]. Nothing we spawn runs 6848/6849,
  so it is a row that never fires yet; it is worth the five lines because
  a later HIC needs no new seam.
- The pointed pilot's real gate stopper is the weapons timer they earn by
  shooting back, which `crimewatch::refuse_jump` already raises.
- Docking is not refused by a scram [I]. No guard.

**CP57's test row should read**: "refuses warp through `WARP_GUARDS` at
strength > stability; a lower strength does not; a gate jump under the
same scram is allowed". If the coordinator keeps the plan's wording, a
scram refusing a gate is an [A] against this lane's [I].

---

## 6. What the client is told

### 6.1 `OnSpecialFX` (already sent) [V]

`modules::activate` already pushes `OnSpecialFX` with the effect's guid
when it is non-empty. The tactical service turns that into the combat-log
lines: `if guid == 'effects.WarpDisrupt': _SendWarpDisruptionMessages`
(`tactical.py:455-456`), `elif guid == 'effects.WarpScramble':
_SendWarpScrambleMessages` (`:457-458`), gated on `start` and on the
pilot being the shooter or the target (`:453-454`). Those pick
`OnWarpScrambleAttacker/Defender/Other` (1685/1683/1684) and
`OnWarpDisrupt*` (4981/4980/4982) client-side. **Nothing extra to send.**
`isOffensive = True` also marks the shooter as attacking the target
(`tactical.py:415-434`, `threatAttackingMe`).

### 6.2 `OnEwarStart` / `OnEwarEnd`: the buff-bar icon [V shape; I server-sent]

`OnEwarStart(sourceBallID, moduleID, targetBallID, jammingType)`
(`tactical.py:1041`) and `OnEwarEnd` with the same four (`:1063`) are
declared `__notifyevents__` with **no local emitter**
(`findings/push-events.tsv:1061-1068`), so the server sends them [I].
`tactical` acts only when `targetBallID == session.shipid` (`:1051`):
it records `jammers[source][jammingType]` and
`jammersByJammingType[jammingType].add((sourceBallID, moduleID))`
(`:1052-1058`) and scatters `OnEwarStartFromTactical` (`:1060`), which
the buff bar draws (`buffBarContainer.py:118`). The overview also listens
(`overviewWindow.py:524`, `UpdateEwarStateOnItemID(sourceBallID)`).

`jammingType` is `dogma.effects.GetEwarTypeByEffectID(effectID)`'s answer
(`dogma/effects/__init__.py:153-165`), from `BUFF_SLOTS_BY_EFFECT_ID` [V]:

| effect | jammingType | buff hint label |
|---|---|---|
| 39 `warpDisrupt` (`:181`), 563 (`:183`), 3713 (`:185`), 2481 (`:195`), 6744 (`:262`), 6849 (`:169`) | `'warpScrambler'` | `UI/Inflight/EwarHints/WarpScrambled` |
| 5934 (`:187`), 5928 (`:198`), 6745 (`:263`), 6848 (`:170`) | `'warpScramblerMWD'` | `UI/Inflight/EwarHints/WarpScrambledMWD` |

(Strings are `buffBarConst.Slot_WarpScrambler` / `Slot_WarpScramblerMWD`,
`buffBarConst.py:11-12`.) Send `OnEwarStart` to the **target's pilot** when
a tackle effect starts on them and `OnEwarEnd` when it stops, as a
`Push` with `audience = [target_character]` [A audience: the target alone,
which is the only receiver whose handler acts]. A `jammingType` of `None`
is logged as an error and dropped (`:1042-1044`), so never send one.

**Entering a ballpark already scrambled** [V]: `michelle.SetState` replays
each `bag.effectStates` entry through `OnSpecialFX` and `ScatterEwars`
(`michelle.py:1271-1273`), which scatters `OnEwarOnConnect(shipID,
moduleID, moduleTypeID, targetID, guid)` (`:2100`/`:2102`), and tactical
derives the jammingType from the module type's default effect
(`tactical.py:1079-1088`, `FindEwarTypeFromModuleTypeID :1091`). So if
our `SetState` bag carries running effects in `effectStates`, the icon
comes back on reconnect with no extra push; if it does not, a pilot who
relogs into a scram has no icon [I]. Not blocking for CP57.

### 6.3 `OnModuleAttributeChange` for 104 [I]

The autopilot reads `warpScrambleStatus` off the godma ship item
(`autopilot.py:589-590`), so the target's ship item must carry the
current 104: push `OnModuleAttributeChange(ownerID=target character,
itemKey=target ship, 104, now, new, old, None)` (the existing
`modules::AttributeChange`; `godma:1591` drops it unless `ownerID ==
session.charid`) whenever the status changes. Also serve 104 in the ship's
`GetAllInfo` attributes, which the sheet already does once 104 is in
`dogma::SHIP_READS` or on the hull row.

### 6.4 Not needed

`OnJamStart`/`OnJamEnd` (`godma.py:250/262`) are ECM's (with a timer);
tackle does not use them.

---

## 7. How it plugs in

### 7.1 `src/tackle.rs` (CP57, new)

```rust
pub const DISRUPT: i64 = 39;              // effects.WarpDisrupt
pub const SCRAMBLE: i64 = 5934;           // effects.WarpScramble, blocks MWD
pub const PLAYER_EFFECTS: &[i64] = &[DISRUPT, SCRAMBLE];
pub const ATTR_STATUS: i64 = 104;
pub const ATTR_STRENGTH: i64 = 105;
pub const ATTR_ACTIVATION_BLOCKED: i64 = 1349;
pub const ATTR_BLOCK_STRENGTH: i64 = 1350;
pub const ATTR_GATE_STATUS: i64 = 1973;
pub const SKILL_HIGH_SPEED_MANEUVERING: i64 = 3454;
pub const SKILL_MICRO_JUMP_DRIVE: i64 = 4385;

/// Σ strength of tackle effects running against `ship_id`, not past stop_at.
pub fn incoming(w: &Registry, ship_id: i64, now: i64, effects: &[i64]) -> f64;
/// `sheet(ship).ship(104)` (0.0 without a sheet) + `incoming`.
pub fn status(w: &Registry, ship_id: i64, now: i64) -> f64;
/// WARP_GUARDS row: Some(Refusal{"WarpScrambled", []}) iff status > 0.
pub fn scrammed(w: &Registry, character_id: i64) -> Option<Refusal>;
/// JUMP_GUARDS row: GateScrambled iff gate status > 0 (never, today).
pub fn gate_scrambled(w: &Registry, character_id: i64) -> Option<Refusal>;
/// The activation check: Some(ModuleIsBlocked) for a module requiring 3454
/// (or 4385, [A]) on a ship under a running 5934.
pub fn activation_blocked(w: &Registry, ship_id: i64, type_id: i64, now: i64)
    -> Option<Refusal>;
```

- `scrammed` resolves `character_id → roster → ship_id`; a pilot not in
  space is never scrammed. `now` comes from `crate::pyutil::filetime_now()`
  (the guard signature carries no clock, and `jump.rs` uses the same).
- `Running` is filtered by `stop_at.is_none_or(|s| s > now)`: the module
  map is advanced lazily, so a stopped-but-unswept entry must not count.
- Strength per running effect: `sheet(r.ship_id).module(r.module_id, 105)`,
  else `find(r.type_id)`'s 105, else 0.
- **No `SLOT`, no `ON_LOGOFF`**: the state is `Modules::running`, which
  already ends on a lost lock at a cycle boundary (`modules.rs:1483`) and
  on death (`combat.rs:813`). The plan's `SLOTS += tackle::SLOT` and
  `ON_LOGOFF += tackle::on_logoff` rows can be dropped. CP57 then does not
  depend on S14.

### 7.2 Registration rows (CP57)

- `movement_guard::WARP_GUARDS` += `tackle::scrammed`
- `movement_guard::JUMP_GUARDS` += `tackle::gate_scrambled` (not
  `scrammed`; §5)

### 7.3 Coordinator hunks

1. **`service/space.rs::warp_guarded`** (`:688`): the guard's refusal is
   dropped today (`if check(..).is_some() { return; }`). Raise it instead:
   `warp`/`warp_guarded` return `Result<(), ServiceError>` and the arms
   that call them (`CmdWarpToStuff`, `CmdWarpToStuffAutopilot`) answer
   `Err(refusal.into_error())`. Still before any state change. §3 is why
   raising is safe. The S12 test then asserts the error is
   `WarpScrambled`'s shape, not just "nothing moved".
2. **`modules::activate`** (`modules.rs:1633`): after the
   `ModuleNotOnline` check and before the capacitor is paid, call
   `crate::tackle::activation_blocked(w, f.ship_id, f.type_id, now)` and
   return its refusal as the `UserError`. A table of activation guards
   (`modules::ACTIVATION_GUARDS: &[fn(&Registry, i64, i64, i64) ->
   Option<Refusal>]`, mirroring S12) is the seam-shaped version, and CP49
   would add its NPC effects through the same function, not a second row.
3. **`modules::activate`, the start/stop of a tackle effect**: when the
   effect is in `tackle::PLAYER_EFFECTS`, push `OnEwarStart` to the target
   pilot, push 104's `AttributeChange` to them, and for 5934 stop the
   target's running MWD (§4.1 [A]). The mirror on every stop path
   (`stop_pushes`) sends `OnEwarEnd` and the 104 change back. The neat
   form is one hook pair, `tackle::on_effect_start/stop(w, &Running, now)`,
   called from `activate` and `stop_pushes`' callers.
4. **`dogma::SHIP_READS`** += 104, so a hull that lacks the row still
   serves 0 and the WCS's −2 shows.

### 7.4 `src/dogma.rs`

The engine already evaluates the stabiliser right (8121 is the module's
default activation effect, started while `State::Active`; `ItemModifier`
on `shipID`, `modAdd`). It does **not** model target-domain modifiers
(`Domain::Target` returns `None`, `dogma.rs:672`; EffectStopper too,
`:695`), and should not start: the incoming half lives in `tackle.rs` as
a sum, which is all `modAdd` needs. If dogma ever models remote effects,
`incoming` is the function it replaces.

### 7.5 CP49 (NPC tackle)

Same sum, other effects: an NPC's tackle is "running" when its brain has
`Intent::Scram` on a target in range (`103`), with strength `105` (563,
5928) or `2509`/`2510` (6745/6744). `npc_ewar::scrammed` computes the
NPC half and **`tackle::status` should call it**, so there is one
`> 0` test over the total; registering two independent guards would let
an NPC disruptor (1) and a player disruptor (1) each fail to beat a
Venture's −2 when together they do. So: CP49 adds no `WARP_GUARDS` row;
it adds `npc_ewar::incoming(w, ship_id, now) -> f64` and one term in
`tackle::status`. 5928 and 6745 block MWD through `activation_blocked`.
`OnEwarStart`'s `moduleID` for an NPC is [A] the NPC's ball id; its
`jammingType` comes from the table in §6.2.

---

## 8. Tests CP57 can write without a client number

All from a fixture table with real shapes and round ids (the dogma-engine
lane's pattern), never a copied client value [lane rule; [A]s never in a
test]:

1. a fixture disruptor (effect 39, 105 = 1) running against a hull with no
   104: `scrammed` refuses with `WarpScrambled` and empty args; stop the
   module and it allows.
2. a hull with 104 = −2: one strength-2 source allows (0 is not `> 0`), a
   second strength-1 source refuses: the sum, the strict inequality, no
   penalty.
3. an active fixture stabiliser (105 = −2) on the target cancels a
   strength-2 source; offline, it does not.
4. a source whose 105 a fixture hull bonus raises by modAdd: the modified
   value counts.
5. a fixture MWD requiring 3454 refuses to activate under a 5934 source
   with `ModuleIsBlocked` and `moduleName = (4, typeID)`; under a 39
   source it activates.
6. a gate jump under the same 5934 source is not refused by
   `JUMP_GUARDS`.
7. a source whose `stop_at` is in the past counts for nothing.

---

## 9. Open, and how to close each

| item | mark | closes with |
|---|---|---|
| server raises `WarpScrambled` for a scrammed warp | I (text) | first client test: the floating notify text on a refused warp |
| `> 0` threshold | I (`autopilot.py:590`) | a Venture under one Scrambler I warps |
| scram does not refuse a gate jump | I | a pointed pilot with no weapons timer jumps |
| running MWD stops when scrammed | A | live behaviour; the HUD button turns off |
| player scram blocks MJD | A | as above, for the MJD |
| out-of-range scram stops at the boundary | A | a scram cycle beyond 7.5 km |
| `OnEwarStart` audience = target only | A | the attacker's overview shows no ewar marker for his own scram |
| `maxVelocityMultiplier` (1470) post-assign | unknown | no client reader; ignore |
