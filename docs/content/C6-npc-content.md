# C6: NPCs as content — derive the authored gap, do not hand-write it

Lane C6, research only (lane rules 1, 7, 8, 9). Client build **3619879**
(`tq-offline`), read through `tools/analysis.sh` with scripts piped on stdin
(`python3 -I -`), nothing installed on the host. Tree read: HEAD `9ba25e7`
plus the W10 working copies of `npc.rs` and `dungeon.rs` (belt rats), read
and not touched. Wrote only this file and
`findings/content-C6-npc-matrix.tsv`.

Marks as `docs/content/README.md` §2: **[V]** this build's bytecode or data
(file and field), **[V2]** EvEJS facts, **[I]** inferred (inference stated),
**[A]** an assumption we ship (with its falsifier), **unknown**. Every
`[V]` number below was measured in this lane over the whole table named
unless it cites another doc.

---

## 0. The answer in one screen

1. **The 504 spawn tables and the 71 fleet types are the wrong target.**
   All 882 `reactiveSpawnComponent` carriers are event, Faction Warfare,
   insurgency, Pochven, Homefront and Zarzakh spawners (730 "Cloud" and 136
   "Invisible Beacon" types); **not one is a belt or an anomaly spawner**
   [V] (§2.3). Rebuilding those 504 ids would buy live-ops content we are
   not building. What a universe that "feels inhabited" needs is belt rats,
   anomaly and complex rats, and mission rats, and for those CCP's server
   tables are absent *and* not referenced by id from anything in the client.
2. **The corpus carries a grammar we can generate from.** Every pirate NPC
   type sits in a group named `<context> <faction> <class>`: `Asteroid Angel
   Cartel Frigate`, `Deadspace Guristas Battleship`, `Asteroid Serpentis
   Commander Cruiser` [V]. Six pirate factions (Angel, Blood Raiders,
   Guristas, Sansha, Serpentis, Rogue Drones) have the **same 17-group
   Asteroid template with identical bounty ladders** (frigates 4,000-37,500
   ISK in every faction, battleships 0.5-1.85 M, officers 17.3-33.9 M) [V],
   and 168 parsed groups in all (`findings/content-C6-npc-matrix.tsv`).
   Within a group, a **name-prefix ladder** gives the tier (`Angel` →
   `Arch Angel`; `Domination` for commanders; officers by name), and the
   same prefixes name the loot (`Domination *` faction modules,
   `Mizuro's *` officer modules, `Angel Copper Tag`, `Gist*` deadspace
   modules) [V]. Two independent keys cross-check the faction:
   `types.wreckTypeID` ("Angel Small Wreck") and the graphic's
   `sofFactionName` (`angelbase`, `guristas`, …: 96-100% agreement for the
   five human pirate factions, 66% for Rogue Drones, whose hulls reuse
   empire graphics) [V].
3. **So the authored part shrinks to a policy of perhaps 40 numbers**
   (budgets by security, roll chances, squad sizes), written once in Rust,
   and **every row — which types, which loot, which behaviour — is a query
   over an extract**. The rules are R1-R8 in §3; the extractor is §6.
4. **Behaviour is mostly in the type's dogma already.** 3,496 entities carry
   `AI_ShouldUseTargetSwitching`, 3,175 `entityAttackDelayMin/Max` (a
   first-shot delay), 5,776 `entityChaseMaxDistance`, 4,043 the chase-delay
   chances, and the role-defining ewar/tank/missile effects are dogma effects
   on the type [V] (§2.5). A behaviour profile is a function of the type,
   using CCP's own ten-role vocabulary (`const/behaviorroles.py` [V],
   `docs/re/npc-behaviors.md` §2.7).
5. **Capability gating makes it grow safely.** The derived table carries
   what each type *needs* (missiles, scram, web, neut, remote reps, tank);
   the spawner only draws types whose needs the server implements. Today
   that is "turret only", which reproduces the hand-picked Angel list in
   `npc::ANGEL_CARTEL` as a query, for all six factions at once; each later
   packet (missiles, ewar, tank) widens the pool with no table edit.

---

## 1. What exists today (read, not redone)

- **`npc.rs`** [V, code]: `NpcType` (hull, one gun, ranges, cruise, bounty,
  wreck, equipment counts, a static kit), `ANGEL_HIJACKER` hand row,
  `Faction { owner_corp_id, groups[(group, wreck)], frigates, destroyers,
  cruisers, equipment, cargo }`, one const `ANGEL_CARTEL`, `faction_for(_) ->
  &ANGEL_CARTEL`, `derive(row, table, faction)` reading 20 attributes from
  `--typedata`, `loot(kind, npc_id)` seeded by the NPC id, `pay_bounty`
  (attr 481, journal type 17), and the pure `decide(&Npc, &View)` brain
  (idle → nearest ship in 20 km or a retaliation lock; approach/orbit band;
  fire per period; disengage). One target, no ewar, no tank, no missiles, no
  first-shot delay, no leash.
- **`dungeon.rs`** [V, code]: mission pockets (`spawn(kind, count)` on a
  10 km ring) and belt rats (`tick_belts`: lazy per occupied system, 75 km
  belt range, 8 km ring, 60 s vacancy despawn, 5 min respawn,
  `COMPOSITIONS` of `Size`s rolled from `(belt_id, spawns)`).
- **`combat.rs::kill_npc`** puts `npc::loot` into a store wreck; loot types
  must be grantable by the item store (`item::find_type` → `SPAWNABLE`,
  planets, skills, then `typedata::grantable`). `tools/typedata.py`
  extracts categories 6, 7, 8, 11, 16, 18 — **not 17 (commodities)**, so
  tags and salvage cannot drop yet [V, code].
- **`eve-bot`** [V, code]: the escort role (`escort.rs`) fights NPCs from the
  park: `Hostile { ball, type_id, name, shooting }`, `pick_target` = the
  engaged one, else the one shooting the boss or us, else nearest within
  60 km of the boss. It knows nothing about what an NPC *is*.
- `docs/re/npc-behaviors.md` §1.4/§6.1 (no `behaviortrees`, spawn lists or
  loot tables ship), `dungeon-model.md` §5-6 (spawn model in the dungeon
  record, `npcGroupingID` points outside the corpus, eight dangling loot
  table references), `evejs-data.md` §4.1 (EvEJS's 5,639 hand profiles; the
  fourteen-field behaviour vocabulary; "profiles become a join"). This doc
  builds on those and corrects one claim of the last (§2.3).

---

## 2. What the corpus actually holds for NPCs (measured)

### 2.1 The group grammar [V]

`groups.fsdbinary` (category 11, Entity) names 1,611 groups; 7,418 entity
types [V]. A regular expression `^(Asteroid|Deadspace|Mission|Ghost
Sites|Roaming|Incursion|Insurgency) (<faction>) (<class>)$` with the faction
alternatives `Angel Cartel | Blood Raiders? | Guristas | Sansha'?s(
Nation)? | Serpentis | Rogue Drones? | Mordu'?s( Legion)?` parses **168
groups** [V]. Classes: `Frigate, Destroyer, Cruiser, BattleCruiser,
Battleship, Hauler, Officer, Officer Frigate, Officer Cruiser, Commander
<class>, Dreadnought, Titan, Carrier, Supercarrier, Swarm`.

| context | what it is [I from names, V for counts] | pirate factions |
|---|---|---|
| `Asteroid` | belt and anomaly rats, 17 groups per faction | the six, plus 3 Mordu commander groups |
| `Deadspace` | complex and dungeon rats (5 classes) | the six |
| `Mission` | mission rats | Mordu only among pirates (the empire `Mission <empire>` groups are the rest) |
| `Ghost Sites`, `Roaming`, `Incursion`, `Insurgency` | special content | various |

`types.factionID` is **not** usable: absent on 6,649 entity types, 0 on
711, 1 on 58 [V].

**One sample row of the matrix** (the rest is the findings TSV):
`Angel Cartel / Asteroid / Frigate / group 550 / 32 types / bounty
4,000-37,500 / 10 with a missile / 12 with ewar / Angel Small Wreck`.

### 2.2 Ladders inside a group [V]

Read off group 550 and its siblings (all six factions have the same shape):

- **Bounty rises one step per hull** inside a prefix (Angel frigates:
  4,000, 5,000, … 13,000) and **the elite prefix starts above the base**
  (`Arch Angel Hijacker` 24,000 … `Arch Angel Impaler` 37,500).
- **Prefix ladder per faction**, read as the name minus its last word
  (base / elite / commander / deadspace stem):

  | faction | base | elite | commander (faction-module prefix) | deadspace stems (frig/dest/cruiser/BC/BS) |
  |---|---|---|---|---|
  | Angel | `Angel` | `Arch Angel` | `Domination` | Gistii / Gistior / Gistum / Gistatis / Gist |
  | Blood Raiders | `Blood` | `Elder Blood` | `Dark Blood` | Corpii / Corpior / Corpum / Corpatis / Corpus |
  | Guristas | `Guristas` | `Dire Guristas` | `Dread Guristas` | Pithi / Pithior / Pithum / Pithatis / Pith |
  | Sansha | `Sansha's` | `Sansha's Loyal` | `True Sansha's` | Centii / Centior / Centum / Centatis / Centus |
  | Serpentis | `Serpentis` | `Guardian` | `Shadow Serpentis` | Coreli / Corelior / Corelum / Corelatis / Core |
  | Rogue Drones | per-hull names, `Strain` elite | — | `Sentient` | per-hull names |

- **Alias rows.** `Cyber`/`Psycho`/`Degenerate` (Angel), `Warrior`/
  `Sellsword` (Blood), `Outlaw`/`Ace` (Guristas), `Crook`/`Marauder`
  (Serpentis) duplicate a base or commander row number for number (same
  bounty, HP, ranges, effects) under another name [V for Angel, I for the
  rest from the counts]. A generator should collapse identical
  `(bounty, hp, attackRange, effects)` tuples to the lowest typeID, or it
  will double their weight.
- `entityEquipmentMin` can be **negative** (-4, -6 on deadspace types) [V];
  `npc::derive` already reads a negative minimum as zero.

### 2.3 The reactive spawners are not belt content [V]

`spacecomponentsbytype.reactiveSpawnComponent` (offset 696, presence
`+1824 & 0x40`) is on **882 types**: 730 group "Cloud", 136 "Invisible
Beacon", 9 "Large Collidable Structure", 4 "Interstellar Shipcaster
Beacon", 2 "Irregular Carrier", 1 other [V]. Their names are FW mission and
complex spawners, "Union Day", "Crimson Harvest", "Horde Mode", "Pochven
Wormholes - Enemy Spawner", "Insurgency <pirate> Roaming Spawner - CT2..CB5",
"Zarzakh <pirate> Roaming Spawner", "Abyssal Retrieval" and the like.
`ownerID` is a **faction id on 597 of them** (500021 ×137, 500027 ×99, …)
and a corporation on the rest [V]. `spawnDataByDungeonID` is non-empty on
110 and `spawnDataByFactionID` on 38 [V].

So the 504 `spawnTableID`s and 71 `initialSpawnNpcFleetTypeID`s
(`evejs-data.md` §4.1) belong to live-ops and warfare content. **Correction
to `evejs-data.md` §4.1's recommendation** ("author only the three missing
tables"): authoring them buys none of belts, anomalies or missions. Keep
reading the 18 numbers on that component (interval, points, threat range)
for whenever FW or insurgency is built — C2 ranks those — and author
belts and dungeons by rule instead.

### 2.4 Dungeon spawns: faction and parameters yes, composition no [V]

Over `dungeons.fsdbinary` (decoder: `fsd.cfsd_dict`, offsets in
`findings/corpus-dungeons.tsv`):

- `dungeon.factionID`: 500021 ×1,349, Guristas 537, Angel 518, Blood 476,
  Serpentis 469, Sansha 428, then the four empires (288-345 each) [V].
- Spawn entities per faction: Angel 3,842, Guristas 3,448, Sansha 3,383,
  Serpentis 3,304, Blood 3,132 [V].
- Pirate dungeons by archetype: 2 "Landmarks" 672, 20 (missions, no title)
  473, 24 "Combat Sites" 331, 31 "Escalation" 311, 44/45 (relic/data, no
  spawns) 409, others small [V]. `difficulty` on combat sites: 1-5, roughly
  even (59/65/56/67/78) [V].
- On objects that carry entities, `guardCommand` respawn `(min, max)` is
  `(3600, 3600)` ×4,012, `(86400, 86400)` ×2,184, `(2880, 2880)` ×1,541;
  `aggressionRange` 250 km ×3,102, 30 km ×1,409, 20 km ×746 [V].
  `triggerSpawn` is set on 4,725 of 17,109 pirate entities [V].
- **The entity has no typeID.** `entityNameID` names 4,578 entities, and
  only **371 (8%) equal an entity type's name** exactly [V] ("Crimson
  Elder", "Corpii Yeoman" are squad display names, not types). It is not a
  composition key. `npcGroupingID` stays unknown (`dungeon-model.md` §5.5).
- **Placed category-11 objects are real NPCs with a type** — 7,862
  `Destructible Sentry Gun` objects (e.g. `Angel Cruise Missile Battery`,
  `Tower Sentry Bloodraider III`), 278 `Deadspace Overseer's Structure`, 54
  `Deadspace Overseer's Sentry` [V]. These need no derivation at all.

### 2.5 Behaviour and loot numbers on the type [V]

Counts of entity types carrying each attribute (category 11):

| attribute (id) | types | reading for us |
|---|---:|---|
| `entityChaseMaxDistance` (665) | 5,776 | leash distance [I from name]; on asteroid pirates 0-17,250 m |
| `entityAttackDelayMin/Max` (475/476) | 3,148/3,175 | first-shot delay window, ms [I]; e.g. 5,000-10,000 |
| `entityChaseMaxDelay/Duration/…Chance` (580-583) | ~4,040 | 10,000 ms each on 742 of 882 asteroid pirates |
| `entityReactionFactor` (466) | 3,175 | 0.3-0.9 [I: reaction speed] |
| `AI_ShouldUseTargetSwitching` / `AI_ChanceToNotTargetSwitch` (1648/1651) | 3,496 | 1.0 / 0.0 on 881 of 882 asteroid pirates |
| `AI_TargetSwitchTimer` (1416) | 24 | default 60,000 ms = CCP's 60 s retarget cooldown (`npc-behaviors.md` §2.8) |
| `AI_ShouldUseSignatureRadius` (1650) | 3,484 | 1.0 on 881/882 [I: weigh targets by signature] |
| `maxAttackTargets` | — | 1 on 777, 6 on 15 asteroid pirates |
| `entityMissileTypeID` (507), `missileDamageMultiplier` (212), `missileLaunchDuration` (506) | 2,346 / 2,006 / 2,768 | the missile it fires: e.g. 213 Nova Light Missile, 206 Nova Heavy, 2508 Nova Torpedo |
| `entityWarpScrambleChance` (504), `warpScrambleRange` (103), `warpScrambleStrength` (105) | 1,893 / 1,513 / 780 | tackle |
| `modifyTargetSpeedChance` (512) | 1,429 | web |
| `entityShieldBoostAmount/Duration/DelayChance` (637/636/639), `entityArmorRepair*` (631/630/638) | ~1,500 / ~2,300 | self-repair |
| `entityKillBounty` (481), `entitySecurityStatusKillBonus` (252), `entitySecurityMaxGain` (563), `entityFactionLoss` (562) | 4,455 / 4,894 / 3,794 / 5,658 | bounty, sec gain, standings loss |
| `entityLootCountMin/Max` (250/251), `entityEquipmentMin/Max/GroupMax` (456/457/465), `entityLootValueMin/Max` (248/249) | 3,718 / 936; 5,389 / 5,375 / 744; 82 / 32 | loot counts |
| `entityGroupRespawnChance` (640) | 1,915 | 0.0 on 432, 1.0 on 356 asteroid pirates [I] |
| `entityOverviewShipGroupId` (1766) | 3,481 | the player ship group it presents as: 25 Frigate, 26 Cruiser, 27 Battleship, 28 Industrial [V] |

Effects on entity types, top: `targetAttack` (10) 4,889,
`missileLaunchingForEntity` (569) 1,914, the `entityShield/ArmorBoosting*`
family ~2,200, `modifyTargetSpeed2` (575) 440, `warpScrambleForEntity`
(563) 318, `entityEnergyNeutralizerFalloff` (6691) 205, `entityTargetPaint`
(1879) 64, `entitySensorDampen` (1878) 98, `entityECMFalloff` (6695) 89,
plus the newer `npcBehavior*`/`behavior*` family (6741-6757, 6884, 6990)
that matches `COMBAT_EFFECTS_BY_ROLE` in `const/behaviorroles.py`
(`npc-behaviors.md` §2.7) [V].

### 2.6 Loot vocabulary in the corpus [V]

Published types whose name starts with a faction's prefix, by
(category, metaGroupID):

- commander prefix → **faction modules and charges**: `Domination` 86
  modules (7, meta 4) + 32 charges; `Dark Blood` 64 + 32; `Dread Guristas`
  54 + 56; `True Sansha` 74; `Shadow Serpentis` 71.
- elite prefix → **faction ammo**: `Arch Angel` 32 charges (8, meta 4).
- officer first name + `'s` → **officer modules** (meta 5): `Mizuro's` 21,
  `Cormack's` 28, `Estamel's` 23, `Draclira's` 27, `Selynne's` 27,
  `Unit D-34343's` 9, …
- deadspace stems → **deadspace modules** (meta 6): `Gist` 36, `Gistum` 27,
  `Gistii` 12, `Corpus` 28, `Pith` 28, `Centus` 20, `Core` 28, …
- **Criminal tags** (group "Criminal Tags", category 17):
  `<base or commander prefix> <metal> Tag`, with `basePrice` 1,000 / 2,250 /
  4,500 / 6,000 / 8,250 on the five live tiers per base faction, 0 on the
  retired ones [V].
- **The ammo an NPC was shooting**: matching each turret NPC's damage vector
  (attrs 114/116/117/118 × 64) to the nearest T1 charge of its size by
  cosine gives `Projectile Ammo` for every Angel frigate, `Frequency
  Crystal` for Blood and Sansha, `Hybrid Charge` for Guristas, and
  projectile/hybrid for Serpentis (15/13) [V for the computation; the
  claim that NPCs "use" ammo is I]. Faction damage profiles over all their
  turret types: Angel em/ex/kin/th = .15/.40/.35/.11, Blood .54/0/0/.46,
  Guristas 0/0/.54/.46, Sansha .58/0/0/.42, Serpentis 0/0/.49/.51 [V].
- T1 + meta weapon pools by charge group and size (`chargeGroup1..3`,
  `chargeSize`, metaGroup 1): projectile 25 per size, crystal 25, hybrid 30;
  light-missile launchers 9 [V].

### 2.7 Who owns a faction's ships, and where it lives [V]

- `factions.corporationID` (executor): Angel → **Dominations 1000138**,
  Blood → Blood Raiders 1000134, Guristas → Guristas 1000127, Sansha →
  True Power 1000162, Serpentis → Serpentis Corporation 1000135, Rogue
  Drones → Infested Regions Hiveminds 1000287 [V]. The code uses
  **Archangels 1000124** for Angel; the corpus's only belt-like Angel
  spawner ("Zarzakh Angel Roaming Spawner") is owned by Archangels, and the
  Guristas one by the executor [V]. Rule R1 below picks "owner of the
  faction's roaming spawner, else the executor" [I].
- `factions.solarSystemID` (HQ) regions: Guristas Venal, Angel Curse,
  Sansha Stain, Serpentis Fountain, Blood Raiders The Bleak Lands, Rogue
  Drones Genesis, Mordu Pure Blind [V]. `regions.factionID` is a pirate on
  Curse, Venal, Stain only [V].
- **Empire → pirate, from the corpus:** mission dungeons named `… (Amarr)`
  etc. carry the pirate `factionID`: Amarr→Blood Raiders 7/7,
  Caldari→Guristas 6/7, Gallente→Serpentis 6/6, Minmatar→Angel 6/7 [V].
  A region-graph BFS from the HQs does **not** reproduce this (it gives The
  Forge → Rogue Drones, Heimatar → Blood Raiders) [V], so geography alone is
  rejected as a rule.

---

## 3. Derivation rules (the core deliverable)

Every rule names its inputs (file and field), its output, and what stays
`[A]`. Rolls are deterministic: `r = roll(mix(world_seed, KIND, key, day,
slot))`, with `mix` and `world_seed` from C7 §4.1; until C7 lands,
`npc::roll(seed, salt)` as today.

### R1. The faction table — author nothing, extract one row per faction

For each `F` in the six pirate factions (and Mordu, Drifters, Triglavians,
Sleepers later):

| column | rule |
|---|---|
| `factionID`, `name` | `factions` key and `nameID` |
| `owner_corp` | owner of the reactive spawner named `Zarzakh <F> Roaming Spawner` if one exists, else `factions.corporationID` [I] |
| `groups` | every category-11 group the §2.1 grammar parses to `F`, with its `context` and `class` |
| `base_prefix`, `elite_prefix`, `commander_prefix`, `deadspace_stems[5]` | §2.2: the most common name-minus-last-word in the `Asteroid F Frigate`, its second-ranked bounty band, `Asteroid F Commander Frigate`, and `Deadspace F <class>` groups respectively |
| `officers` | type names in `Asteroid F Officer*` groups, first token |
| `wrecks[class]` | the modal `types.wreckTypeID` per (context, class) |
| `sof_faction` | modal graphic `sofFactionName` (cross-check only) |
| `damage_profile` | the mean of `(114,116,117,118) × 64` over the faction's turret types |
| `ammo_group` | the charge group that wins the §2.6 cosine vote over the faction's Asteroid types, per size |
| `tank` | `shield` if most types carry an `entityShieldBoosting*`/`shieldBoostingForEntities` effect, else `armor` |
| `tags[5]` | `Criminal Tags` types named `<base_prefix> * Tag` with `basePrice > 0`, ordered by price |

Falsifier tests (§8): each human pirate faction's group set agrees with
its `sofFactionName` on ≥ 95% of types (Rogue Drones excluded, 66% [V]); every `wrecks[class]` names a type
whose name starts with the faction's wreck stem.

### R2. The NPC type — extend `npc::derive` to every pirate type

`NpcType` gains fields read from the type, all **[V-data]** values with
**[I]** meanings where noted:

```
faction: FactionKey, context: Context, class: Class, tier: Tier
role: Role                       // R8
missile: Option<Missile { type_id: 507, multiplier: 212, period_ms: 506,
                          velocity_mul: 645, flight_mul: 646 }>
scram: Option<Scram { range: 103, strength: 105, chance: 504 }>
web: Option<Web { chance: 512, ... }>
ewar: Vec<EffectId>              // TP / damp / ECM / neut / TD / GD, from effects
self_rep: Option<SelfRep { shield|armor, amount: 637|631, period_ms: 636|630,
                           delay_chance: 639|638 }>
attack_delay_ms: (475, 476)      // [I] first-shot window
chase_max: 665, reaction: 466    // [I] leash, [I] reaction
target_switch: 1648 == 1.0, prefer_signature: 1650 == 1.0
switch_timer_ms: 1416 (default 60,000)
max_targets: maxAttackTargets
sec_gain: 252, sec_cap: 563, faction_loss: 562
loot_count: (250, 251), equipment: (456, 457, 465)
thresholds: spacecomponentsbytype.entityStandings (per type, not the const)
needs: Capability bitset         // TURRET, MISSILE, SCRAM, WEB, EWAR, NEUT, SELF_REP, REMOTE_REP
```

`tier` is the prefix rung (Base, Elite, Commander, Officer, Deadspace(stem
index)). `needs` is computed from the effects list. **The hand row
`ANGEL_HIJACKER` stays** as the no-extract fallback, as today.

### R3. Which pirates own a system

```
pirate(system) =
  0. the faction whose HQ (factions.solarSystemID) is in this region, when
     the region has no empire factionID                       [V]  (Fountain → Serpentis,
                                                                    Pure Blind → Mordu)
  1. regions.factionID if it is a pirate                      [V]  (Curse, Venal, Stain)
  2. else the pirate owning the most NPC stations in the region,
     when the region has any pirate-owned station             [V]  (Delve → Blood Raiders 13/13;
                                                                    Fountain → Angel 8 vs Serpentis 4, see [A] below)
  3. else EMPIRE_PIRATE[systems.factionID or regions.factionID] [V] (§2.7 vote:
     Amarr→Blood, Caldari→Guristas, Gallente→Serpentis, Minmatar→Angel)
  4. else ROGUE_DRONES                                         [A]
```

- Minor empires (Ammatar 500007, Khanid 500008, Syndicate, Thukker, ORE,
  SoCT) have no mission vote: map each to its region's nearest empire
  neighbour's pirate [A].
- **A secondary pirate**: a region adjacent (one region hop) to a pirate's
  HQ or sov region also spawns that pirate with weight 1/3 [A]. This is
  what puts Sansha into Tash-Murkon (adjacent to Stain [V]) and Angels into
  Derelik and Great Wildlands. Falsified by an owner who knows a region and
  says the pirates are wrong; the table is an extract, so fixing a rule is
  one line.
- Fountain is why step 0 exists: its HQ is Serpentis [V] while its
  stations lean Angel [V]. Step 0 is limited to regions without an empire
  `factionID` because two HQs sit in empire space (Blood Raiders in The
  Bleak Lands, Rogue Drones in Genesis [V]) and step 3 already answers
  those [I].
- Wormhole, Pochven and Abyssal regions: no belt pirates (Sleepers/Drifters
  are other content) [A].

Output: `out/content/npc/regions.tsv`, `regionID → [(factionID, weight)]`.
`npc::faction_for(system)` becomes a lookup in it.

### R4. Belt spawn tables — a budget, not a list

A belt spawn is generated, not looked up:

```
B(sec)      = log-linear between anchors [A]:
                sec ≥ 0.9 → 12,000   0.5 → 60,000   0.0 → 600,000   -1.0 → 3,000,000 ISK of bounty
classes(sec)= hisec: Frigate, Destroyer (+Cruiser if sec < 0.7)
              lowsec: Frigate .. BattleCruiser
              null:   Cruiser .. Battleship  (+Frigate escorts)          [A]
prefixes    = Base; + Elite when sec < 0.45                               [A]
pool        = types of pirate(system) in Asteroid groups of those classes and
              prefixes, collapsed per §2.2 aliases, filtered by
              needs ⊆ SUPPORTED (§0.5), with entityKillBounty > 0
squad       = lead: the dearest pool type with bounty ≤ B/2;
              then fill with types of bounty ≤ the remaining budget,
              cheapest-weighted, until the budget is spent or 2..6 ships [A]
specials    = commander roll: Commander <class> groups, p = 0 hisec,
              0.02 lowsec, 0.05 null; officer: p = 0.002 at sec ≤ 0.0;
              hauler: Hauler group, p = 0.10 at sec < 0.45            [A]
```

- `COMPOSITIONS` in `dungeon.rs` is replaced by this; the current
  "2-3 frigates, a destroyer sometimes, a cruiser rarely" is what B(1.0)
  produces from the Angel pool (Hijacker 4k + Rogue 5k + Outlaw 6k ≈
  15k), so behaviour at the sandbox does not change [I].
- The respawn and vacancy timers stay where they are, but read
  `entityGroupRespawnChance` (640) as the chance a cleared belt refills on
  the next eligible tick [I].
- **Belt rocks and rats are both C3's census;** C3 owns *how many* belts
  hold rats, C6 owns *what* they hold.

### R5. Dungeon and anomaly "fleet types" — the entity is a squad slot

For an authored dungeon (combat site, escalation, complex, mission
pocket that names a real `dungeonID`):

```
for each object with entities in each room:
   faction  = dungeon.factionID                                   [V]
   context  = Deadspace if archetype ∈ {Combat Sites 24, Escalation 31,
              complexes 33-36, the named 3/4/6/8/14/16/21}, else Asteroid [A]
   budget   = D(difficulty) [A]: 1 → 40k, 2 → 120k, 3 → 400k, 4 → 1.2M, 5 → 3.5M,
              split evenly over the dungeon's spawn entities
   squad    = R4's fill over (faction, context) with that budget, 1..5 ships
   position = object.position + jitter(guardCommand.jitterRadius)  [V field]
   aggression range, orbit range, respawn min/max,
   entitySpawnOnce, entityDelaySpawn, triggerSpawn                 [V fields]
   behaviour = entity.behaviorName if it is a ScriptedBehaviorLoader key
               (dps, tackler, antidps, antilogistics, fighter, sleep) [V];
               else R8 from the type
```

- `triggerSpawn` entities wait for a trigger that is not in the corpus
  (`dungeon-model.md` §5.3). Rule [A]: **the next room's gate activation or
  the death of the room's last non-trigger squad**, whichever comes first.
- Placed `Destructible Sentry Gun` and `Overseer` objects are spawned as
  their own typeID with R2's profile and `orbit` replaced by "stationary"
  [V for the type, A for "stationary"].
- An entity whose `entityNameID` resolves is shown with that name if and
  when we implement `dynamicEntityName` (`npc-behaviors.md` §4.3); until
  then ignored [A].
- **The 504 spawn tables / 71 fleet types**, if FW or insurgency is ever
  built: `spawnTableID` resolves to the same R4 fill over `(ownerID's
  faction, context from the spawner's name tokens "Frigates/Cruiser/
  Battleship/Elite Tackler/Commander Boss", budget = initialPointAmount ×
  K)` with K [A]. Not before C2 ranks that content.

### R6. Loadouts — what it fights with is its dogma

An entity's weapons, tank and ewar *are* its dogma attributes and effects
(R2); there is no fitted-module loadout to author [V: entity damage sits on
the hull, `docs/lanes/npc.md` §3.2]. "Loadout" only matters twice:

1. **Visual**: the gun drawn is `gfxTurretID`/`gfxBoosterID` on the hull
   and the `OnSpecialFX` guid is the default effect's (as today) [V].
2. **Loot**: R7 makes the wreck plausibly hold *what it was flying*.

The EvEJS `presentationTypeID`/`shipTypeID` split (`evejs-data.md` §4.1) is
**not needed** for pirate entities: they already present as player ship
groups through `entityOverviewShipGroupId` [V].

### R7. Loot tables — by faction, size, tier and bounty

```
count       = equipment: roll in [max(456,0), 457], at most 465 per group
              loot:      roll in [250, max(251, 250)]                     [V counts, I meaning]
size        = Frigate/Destroyer → 1, Cruiser/BC → 2, Battleship+ → 3
pool(type)  =
  T1/meta equipment (always):
     weapons of the faction's ammo_group at size (meta 1, metaLevelOld 0-4),
     tank modules of the faction's tank (Shield Booster/Extender or Armor
     Repairer/Plates) at size, propulsion at size (1MN/10MN/100MN), and the
     module group of each ewar it carries (Warp Scrambler 52, Stasis Web 65,
     Target Painter 379, ECM 201, Sensor Dampener 208, Neutralizer 71,
     Weapon Disruptor 291)                                            [V pools, A choice]
     metaLevelOld drawn ≤ tier cap: Base 0-1, Elite 0-3, Commander+ 0-4 [A]
  ammo (loot count):
     the NPC's own entityMissileTypeID if any, else the §2.6 nearest charge
     to its damage vector, at size; quantity 20-100 per stack × size     [I, A]
  faction loot (Commander tier): metaGroup 4 types named <commander_prefix> *
     at size where size applies, p = 0.25 per wreck [A]
  officer loot: metaGroup 5 types named <officer first name>'s *, 1-2 per kill [A]
  deadspace loot (Deadspace context, BS/overseer only): metaGroup 6 types
     named <stem> * , p = 0.10 [A]
  tag: one of tags[5], tier = clamp(bounty rank decile / 2, 0, 4),
     p = 0.5 for Base/Elite at sec < 0.45 [A]
  salvage: not loot (needs a salvager; category 17 too) — later packet
```

- The value guard `entityLootValueMin/Max` (248/249) is on 82/32 types
  only; where present, re-roll until the summed `basePrice` falls inside it
  [I].
- **Deterministic per NPC**, as `npc::loot` is today: `roll(npc_id, salt)`.
- The pools are extracted once (§6) as `(faction, size, kind, tier) →
  [typeID]`; the policy numbers above live in code.
- **What the generator writes for the equipment kinds** (#123,
  `content_npc_regions.py` `loot_rows`): every published module of metaGroup 1
  (named, compact and enduring modules are metaGroup 1 with a meta level above
  0 [V: the market table], so they drop up to the tier cap) as `base` rows:
  `weapon` as above; `tank` for the faction's `tank` (shield: booster and
  extender groups; armor: repairer and plate groups) at the size its name
  gives; `prop` for the propulsion group at the size of its `<n>MN`
  (1 and 5 small, 10 and 50 medium, 100 and 500 large); `ewar` for the module
  groups its NPCs' `needs` name (SCRAM, WEB and NEUT one group each; EWAR the
  painter, ECM, damper and disruptor groups; a size-free module is in all three
  sizes). A module whose name gives no size is in no tank or prop pool.
  `[A]` the size words and plate mm ranges (`module_size`) and the EWAR
  choice; falsified by a hull class holding a module of another class, or a
  faction whose rats never use a group listed. No seller is added.
- Item-store reach: every pool type must be grantable; categories 7, 8 are
  in `--typedata` today, **17 (tags) is not** [V, code].

### R8. Behaviour profiles — role from effects, numbers from attributes

```
role(type) = first match:                                         (CCP's roles [V], order [A])
   LOGISTIC  if any remote-repair effect (6741, 6742, 3852, 3855, 6165, 6687)
   TACKLER   if warpScrambleForEntity / behaviorWarpScramble / behaviorWarpDisrupt
             or modifyTargetSpeed2 / npcBehaviorWebifier
   EWAR      if TP / damp / ECM / TD / GD / neut effects
   DPS       otherwise
policy(type) =
   acquire:  as today (nearest in aggression range, or retaliation), but
             score = 1/distance × (prefer_signature ? f(sig, own class) : 1)  [I]
   first shot after uniform(attack_delay_ms) once locked              [I]
   retarget every switch_timer_ms when target_switch                  [V numbers]
   orbit at entityFlyRange (416); approach band as today              [V/A]
   leash: give up and return to spawn when > chase_max from the spawn
          point, if chase_max > 0                                      [I]
   TACKLER: apply scram/web on the target in range (separate packet)
   EWAR:    apply its effect on the target (separate packet)
   LOGISTIC: rep the lowest-health ally in range (separate packet)
   self-rep: every period, with probability 1 − delay_chance, heal amount
```

This is the EvEJS fourteen-field vocabulary (`evejs-data.md` §4.1)
answered from the type: `thinkIntervalMs` = the world tick;
`autoAggro` = true; `targetPreference` = R8 score; `orbitDistanceMeters` =
416; `aggressionRangeMeters` = guardCommand or 20 km on belts [A];
`leashRangeMeters` = 665; `returnToHomeWhenIdle` = true [A];
`autoActivateWeapons` = true [V2 vocabulary, our values].

---

## 4. Authoring format: what is extracted, what is written by hand

**Policy, not rows.** The repo holds code and a handful of tuning
constants; every type id, name and number comes from `out/`.

| artefact | where | owner | contents |
|---|---|---|---|
| `out/content/npc/types.tsv` | extracted | §6 extractor | one row per category-11 type in a parsed pirate group: R2's columns plus `faction, context, class, tier, alias_of, needs, role` |
| `out/content/npc/factions.tsv` | extracted | §6 | R1's row per faction (prefixes, owner corp, wrecks, ammo group, tank, tags) |
| `out/content/npc/regions.tsv` | extracted | §6 | R3's `regionID → [(factionID, weight)]` |
| `out/content/npc/loot.tsv` | extracted | §6 | R7's pools: `faction, size, kind, tier, typeID, metaLevel` |
| `out/content/npc/dungeon_spawns.tsv` | extracted | §6 | R5's per-entity slots: `dungeonID, roomID, objectID, entityID, faction, archetype, difficulty, x, y, z, jitter, aggression, orbit, respawn_min, respawn_max, spawn_once, delay_spawn, trigger_spawn, behaviorName, groupBehaviorName` (plus placed sentries with their typeID) |
| `npc_gen.rs` constants | repo | the C6 build packet | `B(sec)` anchors, class windows, special chances, `D(difficulty)`, squad sizes, loot probabilities, minor-empire map, drone fallback — each `[A]` with its falsifier in a doc comment |

TSV because the server is std-only and every other extract is TSV
(`out/typedata.tsv`, `celestials-*.tsv`); one versioned header line per
file as `tools/typedata.py` does. If C1's unified `--content DIR` lands,
these are `DIR/npc/*.tsv`.

---

## 5. How it plugs in

### 5.1 `eve-server`

- **`npc.rs`**: `NpcType` gains R2's fields; `Faction` becomes a loaded row
  (R1) instead of a `const`; `FACTIONS`/`ANGEL_CARTEL` stay as the
  no-extract fallback; `faction_for(system)` reads R3; `derive` reads the
  new columns; `loot` implements R7 over the pools; `decide` grows R8's
  first-shot delay, retarget timer and leash (pure, table-tested like
  today). Ewar, missiles, tank and logistics are separate intents
  (`Intent::Scram`, `Intent::Missile`, `Intent::SelfRep`, …) so each
  packet adds one and widens `SUPPORTED`.
- **`dungeon.rs`**: `fill` calls `npc_gen::belt_squad(system_sec, faction,
  seed)` (R4) instead of `COMPOSITIONS`; mission pockets call
  `npc_gen::dungeon_squads(dungeonID, seed)` (R5) when the extract has the
  dungeon, else the current ring.
- **`combat.rs`**: unchanged call (`npc::loot`), plus `pay_bounty` gains the
  security-status credit from 252/563 later (crimewatch packet).
- **`scan.rs`**: `NPC_GROUPS` reads `n.kind.group_id` (already noted in the
  belt-rats parked note).
- A new **`npc_gen.rs`** owns the policy constants and the pure generators,
  so `npc.rs` and `dungeon.rs` change only at their call sites.

### 5.2 `eve-bot`

The bot is a player; a player's client has the same static data, so a bot
may read `out/content/npc/types.tsv` without cheating [I: everything in it
is in the client]. Three uses, in priority order:

1. **Target priority** (`escort::pick_target`): after "engaged" and
   "shooting the boss or us", prefer `role == TACKLER` (a scram keeps the
   fleet from leaving), then the lowest time-to-kill (`hp / our dps`), and
   skip `class == Hauler` unless nothing else is left. [A]
2. **Ammo choice**: load the charge whose damage the faction resists least
   (resist attrs on the type) — the escort today always loads EMP. [A]
3. **Retreat and belt choice**: estimate incoming DPS from the visible
   hostiles' profiles; go home above a threshold; a ratting bot picks belts
   in systems whose `B(sec)` it can handle. [A]

`brain::Hostile` gains `profile: Option<NpcProfile>` filled by the driver
from the TSV by `type_id`; the brain stays pure.

---

## 6. The extractor

**`tools/content.py npc`** (or a `tools/npc_content.py` if C1's unified
tool is not adopted): reads `types`, `groups`, `typedogma`,
`dogmaeffects`, `graphicids`, `factions`, `npccorporations`,
`spacecomponentsbytype` (entityStandings at +1404, reactiveSpawnComponent
at +696), `regions.static`, `systems.static`, `solarsystemcontent.static`
(NPC station `ownerID`s, nested under planets and moons), `dungeons`,
`archetypes`, `missions` names via `dungeons`, and writes the five files of
§4. Every reader it needs already exists in `tools/dogma.py` and
`tools/fsd.py` (`cfsd_dict` for dungeon rooms/objects/entities); the
offsets are `findings/pyd-schemas.tsv` and `findings/corpus-dungeons.tsv`.
Run time with the whole `solarsystemcontent.static` (92 MB): a minute in
the analysis container [I].

---

## 7. Size for 2-10 humans and ~50 bots

- Belt rats: a hot system (C7) holds its occupied belts' squads, 2-6 ships
  each; 50 bots spread over ~10 systems with ~3 occupied belts each is
  ~150 live NPCs — inside what the tick already runs (`npc::tick` is one
  pass per system) [I].
- Dungeons: one pocket per accepted mission or warped-to anomaly; a level 4
  combat site at `D(4)` ≈ 1.2 M of bounty is ~4-10 ships [A].
- Extract size: types.tsv ≈ 1,000 pirate rows × ~60 columns, loot.tsv a few
  thousand rows, dungeon_spawns.tsv ≈ 17,000 pirate entity slots plus
  ~8,000 placed sentries — all small [I].

---

## 8. Tests the build packets should carry (no `[A]` asserted)

- **R1** on the real extract: six factions parsed; for each of the five
  human pirates, ≥ 95% of its types share one `sofFactionName`; each class's wreck name starts with the
  faction's wreck stem; the commander prefix finds ≥ 50 metaGroup-4 types.
- **R2**: the derived 10013 still equals `ANGEL_HIJACKER` field for field
  (the belt-rats lane's check, extended); a type with effect 563 has
  `needs ∋ SCRAM`; a type with 507 > 0 has `missile.is_some()`.
- **R3**: Curse → Angel, Venal → Guristas, Stain → Sansha (step 1, [V]);
  a system with `factionID` 500002 → Angel (step 3, [V] vote). Weights are
  not asserted.
- **R4/R5** compare the code with itself: a squad's bounty sum ≤ its budget;
  no type outside `SUPPORTED`; same seed → same squad; every squad member
  is of `pirate(system)`.
- **R7**: every pool type is grantable (`item::find_type`); loot of one NPC
  id is stable across calls.
- **R8** as tables of state → intents, as `decide` is tested today: no
  `Shoot` before the delay window opens; a retarget fires at the timer;
  leash returns the NPC when the target drags it past `chase_max`.

---

## Proposals

1. **Do not author the 504 spawn tables or 71 fleet types.** They are
   live-ops/FW/insurgency spawner content (§2.3); park them until C2 ranks
   that content, and record the correction to `evejs-data.md` §4.1.
2. **Generate NPC content from the group grammar** (§2.1) with the eight
   rules R1-R8; the repo holds only policy constants (`npc_gen.rs`), every
   row is extracted to `out/content/npc/` (§4).
3. **Belt spawns are a bounty budget by security** (R4), drawn from the
   system's pirate faction's Asteroid groups, filtered by server
   capability; `COMPOSITIONS` goes away and the sandbox's behaviour stays
   the same.
4. **Region → pirate by the four-step rule R3**, using the corpus's own
   empire → pirate vote from `(Empire)` mission dungeons; drone regions and
   minor empires are the only `[A]` rows.
5. **Behaviour comes from the type** (R8): role from effects in CCP's own
   ten-role vocabulary; first-shot delay, retarget timer, signature
   preference and leash from attributes 475/476, 1416/1648, 1650, 665.
6. **Loot from prefixes and pools** (R7): T1/meta by faction weapon family,
   tank and ewar at the NPC's size; faction, officer and deadspace modules
   by the faction's own name prefixes; tags by bounty rank; ammo it was
   firing. Add category 17 to the type extract so tags can drop.
7. **Capability gating** (`needs ⊆ SUPPORTED`) is the rule that lets each
   later NPC packet (missiles, ewar, tank, logistics) widen content with no
   data edit.
8. **Bots read the same type table** as a player client would, for target
   priority, ammo and retreat (§5.2), keeping `brain` pure.
9. **Keep Archangels as the Angel owner** (R1's roaming-spawner rule), the
   executor corp for the rest, until the client shows otherwise.

## Packets

| # | packet | owns | tests |
|---|---|---|---|
| N1 | **NPC content extractor**: `tools/content.py npc` (or `tools/npc_content.py`) writing the five `out/content/npc/*.tsv`; add category 17 to `tools/typedata.py` `CATEGORIES` (coordinate with the dogma lane, which owns that line) | `tools/npc_content.py`, `tools/test_npc_content.py` | R1 and R3 checks of §8 on the real client (skipped without one); header/version line; alias collapse |
| N2 | **Loader and derived types**: read `types.tsv`/`factions.tsv`/`regions.tsv`; `NpcType` gains R2's fields; `Faction` from rows; `faction_for` from R3 | `npc.rs` (type and faction half), a new `npc_data.rs` loader | R2 checks; no-extract fallback still the hand Hijacker; loader rejects a bad header |
| N3 | **Generators**: `npc_gen.rs` with R4 (belt squads), R5 (dungeon squads, sentries), R7 (loot pools), all pure; `dungeon.rs::fill` and pocket spawn call them | `npc_gen.rs`, `dungeon.rs` (`fill`, pocket `spawn`) | R4/R5/R7 checks of §8 |
| N4 | **Brain upgrades, turret-only**: first-shot delay, retarget timer, signature score, leash, self-repair | `npc.rs` (`decide`, `apply`) | R8 tables |
| N5 | **Missiles** (`Intent::Missile`, flight time from velocity × flight multipliers, `OnSpecialFX` missile guid); `SUPPORTED |= MISSILE` | `npc.rs`, a new `npc_missile.rs` | volley lands after flight time; damage = missile type's × 212 |
| N6 | **Tackle and ewar** (scram/web first: `Intent::Scram/Web`, warp refusal, speed factor; then TP/damp/ECM/neut) | `npc.rs`, `npc_ewar.rs`; one seam in `space_state` for warp refusal (coordinator) | a scrammed player's warp is refused; web halves max velocity per attrs |
| N7 | **Bot reads profiles**: `Hostile.profile`, tackler-first priority, ammo by resist, DPS-based retreat | `eve-bot/src/escort.rs`, `eve-bot/src/profile.rs` | brain tables: tackler chosen over nearer DPS ship; retreat above threshold |
| N8 | **Bounty side effects**: security-status gain (252/563) and faction standings loss (562) | `npc.rs::pay_bounty`, crimewatch module (coordinator decides) | a kill raises sec by the type's bonus, capped |

N1 → N2 → N3 is the critical path; N4 can run beside N3; N5-N8 are
independent after N2.

## unknown

1. **Real belt compositions by security** — CCP's per-band tables are
   server-side; R4's anchors are `[A]` and the morning test round is the
   falsifier.
2. **What `npcGroupingID` names** (`dungeon-model.md` §5.5) and **what a
   `triggerSpawn` waits for** (§5.3) — R5's trigger rule is `[A]`.
3. **Which corporation owns belt rats** for Blood/Guristas/Sansha/Serpentis
   (executor assumed) and whether Angel is Archangels or Dominations.
4. **Exact meanings** of `entityAttackDelayMin/Max`, `entityChaseMax*`,
   `entityReactionFactor`, `entityGroupRespawnChance`,
   `entityEquipmentGroupMax`, `entityStrength` (4.0 on 116 asteroid
   pirates) — names only, no client reader found.
5. **`entityBounty.bountyMultipliersByOwner`** (spacecomponents +336) —
   not decoded in this lane; may be the ESS/insurgency bounty modifier.
6. **The drone regions and minor-empire pirates** — no corpus vote; `[A]`.
7. **Whether the client renders `entityNameID` squad names** on a spawned
   NPC through `dynamicEntityName` without further calls.
8. **Mission rats** for empire-vs-pirate missions: missions name a
   `dungeonID` whose `factionID` R5 can use; whether mission pockets should
   use `Mission` groups or `Deadspace` groups is `[A]` (R5 uses Deadspace
   for pirates because pirate `Mission` groups exist only for Mordu).
