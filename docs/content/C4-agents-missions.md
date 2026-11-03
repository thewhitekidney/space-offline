<!-- Provenance: content lane C4, 2026-10-25. Read-only on the client, the
     server and tools/: the only files written are this one and
     findings/content-C4-pools.tsv (counts only). No cargo was run. The
     corpus was read with scratch readers built on tools/fsd.py's CfsdTable
     and the record offsets in findings/pyd-schemas.tsv; bytecode with
     tools/dis.py (line numbers from co_lnotab). EvEJS was read, never run,
     and contributes numbers and vocabulary only, marked [V2]. Marks: [V]
     this build's bytecode or data, [V2] EvEJS, [I] inferred (inference
     stated), [A] an assumption we would ship (falsifier stated), unknown. -->

# C4: agents, missions, arcs and the Agency's PvE

Client build 3619879. This extends `docs/re/agents-missions.md` (the call
surface and the state machine) and `docs/re/mission-model.md` (one mission
end to end, and the `missions.fsdbinary` layout). It does not repeat them.
The counts behind every table here are in `findings/content-C4-pools.tsv`.

**Summary.** The agent→mission pool was Tranquility server data, and nobody
in this tree has rebuilt it. **The client corpus contains enough to derive
it**, so we don't have to author it:

- a Basic mission's **level** is its dungeon's `difficulty` column;
- its **offering faction** is `missions.factionID`;
- its **enemy** is `dungeons.factionID`;
- each empire's **home pirate** can be read off the faction-locked missions;
- its **division** follows from the template family;
- its **chains** are named `(k of n)` and flagged by a `nextmission` message
  slot;
- every line of dialogue it lacks has a **generic CCP line in the client**
  (`UI/Agents/DefaultMessages/*`), which the client composes itself.

Missions whose agent is fixed by data are a separate case and need nothing
derived: storyline and career lists in `npccharacters.agent.storyLineMissions`,
and the 21 epic arcs in `epicarcs.fsdbinary`. What we still author is
geography (where a destination or pocket goes), money (ISK, LP and LP-store
prices) and timers. §3 is the generator, §12 the proposals and §13 the
packets.

---

## 1. What the server has today, and the gap

- `src/mission.rs` serves **one agent** (3010110, *Alfroren Agitta*) and
  **two hard-coded missions** (`Kind::Courier` 4832, `Kind::Blockade` 2924).
  Its selection policy is "decline or complete one to get the other", marked
  `[A]` in its module docs. `Missions` is keyed by character only, so one
  agent at a time. It is not persisted. **[V]** (`mission.rs:216-305`)
- `service.rs::agent_rowset` sends 13 epic-arc starter rows with every field
  except the id set to `None`, plus the one station agent. The comment says
  *"the real faction/corp mapping is server data found nowhere in the client
  tree"*. **That is no longer true**: `npccharacters` gives each of those 13
  agents its corporation, station and type (§7.4). **[V]**
- `LPSvc` answers `[]` for both balance calls (`service/wallet.rs:167`).
  `LPStoreMgr`, `epicArcStatus.AgentHasEpicMissionsForCharacter` and
  `agentMgr.GetMessagesForEpicArcMissions` are unanswered. `GetReplayTimestamp`
  is `None`. **[V]**
- `out/celestials-all.tsv` holds **1,223 stations**, but
  `solarsystemcontent.static` has **5,210** NPC stations. The extract keeps
  only planet-orbit stations and drops moon stations. Of the 5,063 distinct
  agent stations, only **1,149** are in the extract today and **5,062** are in
  `solarsystemcontent`. Every packet below that names a station depends on
  fixing this. **[V]** (`tools/celestials.py:151` emits `planet.npcStations`
  only.)

---

## 2. What the corpus says about who offers what

### 2.1 Agents: 10,972 rows, and their type decides the source of their missions

`npccharacters.fsdbinary` rows carrying the nested `agent` struct, read with
`mission-model.md` §1.1's convention. The fields are `agentDivisionID` at
+16, `agentLevel` at +20, `agentType` at +24 and `agentLocateCharacter` at
+28 (u8), all record-relative. `storyLineMissions` is a `list<i32>` at +8,
present when bit 0x1 of the struct's presence word (+32) is set. The list
elements are **4-byte**, every id closes into `missions`, and there are 0
dangling. **[V]**

Each agent's faction is its corporation's `npccorporations.factionID`. All
10,972 agents resolve to one. **[V]** Its solar system comes from its
`stationID` through `solarsystemcontent`; 10,970 agents have a station.
**[V]**

The type names are `appConst.agentType*`, `appConst.py:105-117` **[V]**. The
per-type census is in Block 3 of the TSV. What matters for the generator:

| agentType | agents | where its missions come from |
| --- | --- | --- |
| 2 Basic | 8,775 | **the derived pool** (§3) |
| 5 CONCORD | 143 | the derived pool, with no faction-locked missions (§3.4) |
| 8 EventMission | 698 | 367 carry a fixed `storyLineMissions` list (§8). The 331 without one use the derived pool. 143 are level 5. |
| 12 Career | 100 | a fixed list of 10 missions each, divisions 25-29 (§8) |
| 6 / 7 Storyline | 651 / 11 | the storyline trigger (§6). All are level 1 in the data. |
| 10 EpicArc | 52 | `epicarcs.fsdbinary` (§7) |
| 9 FactionalWarfare | 260 | the `agentTypeID = 9` missions; **deferred** |
| 4 Research, 3 Tutorial, 11 Aura, 13 Heraldry | 244 / 14 / 12 / 12 | **deferred** (§10) |

The divisions are `npccorporationdivisions`: 22 Distribution, 23 Mining,
24 Security, 18 R&D, 25-29 the career divisions (Industrialist-Entrepreneur,
Explorer, Industrialist-Producer, Enforcer, Soldier of Fortune) and 37
Heraldry (shown as "InterBus"). **[V]** These are the only agent divisions.

`agentLocateCharacter` is set on **1,773** agents. That is the `isLocator`
column of `GetAgents`, which `service.rs` currently sets to `false` for
everyone. **[V]** No `quality` field exists, so it stays 0. **[I]**

A correction to `mission-model.md` §1.1: the third agent at our station,
*Kaeg Zkaen* (3018559), is **agentType 8, division 24**, not "agentType 24".
**[V]**

### 2.2 Missions sort into four sources, and only one needs a generator

All 2,892 missions, by `contentTemplate` (Block 4 of the TSV): **[V]**

1. **Drawn from the pool** (no agent named anywhere): `BasicKill` 992,
   `BasicCourier` 238, `BasicMining` 42, `BasicTrade` 4. None of these is
   referenced by any agent's `storyLineMissions`.
2. **Fixed lists on agents:** the `storyLineMissions` lists hold 926
   distinct missions in 1,693 references. All are `Storyline{Trade,Kill,Courier}`
   (388 / 298 / 240), and they belong to types 8 and 12 (plus one type-7
   Guristas agent).
3. **Epic arcs:** 275 of the 284 `EpicArc*` rows are in `epicarcs.fsdbinary`.
   The other 9 are orphans, all named like drafts ("Unnamed4", "GREAD5
   Mission").
4. **The storyline trigger's pool:** 263 `GenericStoryline*` rows that no
   list references. On Tranquility these were offered by type 6 and type 7
   agents after a run of Basic missions (§6).

The 55 `StorylineTrade`, 3 `StorylineKill` and 2 `StorylineCourier` rows that
no list references are **unreachable** under these rules. That is
acceptable. **[I]**

### 2.3 A mission's level is in the corpus, through its dungeon

`mission-model.md` §8 lists "a mission's level" as unknown. For every
dungeon-shaped mission it is **`dungeons.difficulty`** (record +72, presence
bit 0x10 at +112) of `killMission.dungeonID`. The evidence:

- **The dungeon names agree.** 266 dungeons carry an explicit level token
  ("lvl2", "Level 3"). **263** of them have `difficulty` equal to that level.
  The three that don't are off by one, and two of those are "lvl2 … Version"
  rows that read 3. **[V]**
- **The multi-level families come out ordered.** For example, *Gone
  Berserk* 2504/3081/3082/3083 → difficulty 1/2/3/4, and their dungeon names
  are "Gone Berserk", "…lvl2", "…lv3", "…lvl4". *The Blockade* has 25 variants
  over difficulties 1-4. **[V]**
- **The range fits the agents.** `BasicKill` difficulties are 1:261, 2:191,
  3:226, 4:180, 5:133. Level-5 agents exist (Basic 37, Event 143), and so do
  level-5 missions. `BasicMining` covers 1-4. **[V]**

**Courier missions have no dungeon.** Their level is **[A]** a cargo-volume
band: `objectiveQuantity × types.volume` (types +56, bit 0x800000) of at most
250 m³ is level 1, 1,000 m³ level 2, 4,000 m³ level 3, and more than that
level 4. **[A]** The volumes fall into two clusters with nothing between
500 and 1,000 m³. The 75 two-variant courier families are each (low, high),
and the four-variant families rise monotonically (100/200/1,000/5,000 m³).
What would falsify the bands: a Tranquility capture of a level-N agent
offering a courier outside its band. Any level-5 distribution or mining
agent draws from level 4. **[A]**

### 2.4 The offering faction, the enemy, and each empire's home pirate

- `missions.factionID`, where present, is the faction **whose agents offer the
  mission**. `dungeons.factionID` of its dungeon is **the enemy**. The cross-tab
  proves it. Mission faction 500001 (Caldari) is paired with enemy 500004
  (Gallente) 32 times outside FW and 41 times in FW. Every FW mission has
  `agentTypeID = 9` and an enemy that is the opposing empire, and its
  `factionID` is the agent's side. **[V]** The reading is **[I]**, and no
  other reading fits the 1,103 kill rows.
- **Each empire's home pirate falls out of the faction-locked missions.**
  Count the faction-locked kill missions whose enemy is a pirate faction:
  **[V]** counts, **[I]** reading.

  | offering faction | Guristas | Angel | Blood Raider | Sansha | Serpentis |
  | --- | --- | --- | --- | --- | --- |
  | Caldari 500001 | **31** | 2 | 1 | – | 2 |
  | Minmatar 500002 | 3 | **34** | 1 | 1 | – |
  | Amarr 500003 | 1 | – | **31** | 3 | – |
  | Gallente 500004 | 1 | – | 1 | – | **33** |
  | ORE 500014 | – | – | – | – | **3** |
  | Thukker 500015 | – | – | – | **3** | – |

  Read across the rows: Caldari→Guristas, Minmatar→Angel, Amarr→Blood Raider
  (Sansha second), Gallente→Serpentis. Rogue Drones (500021) are the most
  common enemy overall (203 `BasicKill`, 42 of 42 mining). They have no home
  and suit every agent.

  *Rejected alternative:* "nearest pirate HQ by jumps", using
  `npccorporations.solarSystemID`, gives Angels for 88% of agents. Angel
  corporations have HQs on every side of the map. **[V]** measured.

### 2.5 Chains: `(k of n)` in the title, and a slot that says "more follows"

Of the Basic rows, 311 have titles of the form "*Series – Part (k of n)*".
Separately, `messages.mission.completed.nextmission.agentsays` marks a
mission that has a successor. The two signals **agree on 297 of 306** chain
rows: the slot is present exactly when k < n. **[V]** Chains can **mix
families**: *Flame of Peace* goes kill → courier → kill. The series name
before " – " is the stable key, and the part names differ.

Linking rule, measured: part k+1 = same series key, same n, index k+1, and
the same difficulty when both parts are kill missions. Under this rule **75
of 92** chain starts resolve to a complete chain; at each step 150 have one
candidate, 52 have 2-4 and 35 have none. **[V]** The 17 that don't resolve
(for example *Rogue Spy*, level 5, with no parts 2-3 in the data) are
dropped from the pool. A step with several candidates is a choice of enemy
faction (§3.6).

### 2.6 Every line the mission lacks has a generic line in the client

Within the 900-mission drawn pool, the `messages` slots are uneven: **[V]**

| slot | missions that have it |
| --- | --- |
| `messages.mission.briefing` | **900 / 900** |
| `offered.agentsays` | 419 |
| `accepted.agentsays` | 419 |
| `completed.agentsays` | 335 |
| `declined.agentsays` | 320 |
| `quitted.agentsays` | 285 |
| `extrainfo.header` | 171 |

The client ships CCP's generic lines for every gap, under
`UI/Agents/DefaultMessages/*`, resolved in `localization_fsd_main`'s label
index **[V]**:

- `MissionOffer1-6` (236804-236809), for example *"Yes, I have something for
  you.<br><br>{missionBriefingText}"*;
- `MissionAccepted{Low,LowMed,MedHigh,High}N` (236738-236750);
- `MissionCompleted…N` (236751-236769);
- `MissionDeclined1-5` (236793-236797) and `MissionQuitted1-3`
  (236800-236802);
- `RootAgentSays/{Low,Medium,High}N`, the greetings;
- `RootMissionInProgress…`;
- `NoMission1-2`, `MissionTooLowStandings` (236691) and `OfferStillOpen`
  (236803);
- `MissionCompletedNextMission` (236789, `{missionCompletionText}`), the
  chain hand-off line;
- `MissionOfferExpirationEmail{Header,Body}` (263559 / 263558).

`ProcessMessage`'s label form `(label, {key: value})` resolves nested message
ids in exactly the `missionBriefingText`, `missionOfferText`,
`missionCompletionText` and `declineMessageText` slots
(`agents-missions.md` §2.3, `agents.py:724`). **[V]** So the offer for a
mission with no `offered.agentsays` is

```python
(('UI/Agents/DefaultMessages/MissionOffer3', {'missionBriefingText': 134916}), 4832)
```

and the client renders CCP's wrapper around CCP's briefing. **With that,
every mission in the pool is servable with no authored prose.** This also
removes `mission.rs`'s one-step-offer trade-off: a greeting now exists
(`RootAgentSays/*`), so the server may use the Tranquility-faithful two-step
offer (§3.8).

Tokens the pool's prose uses, by count of missions: `dungeonLocationID` 759,
`player` 756, `agentID` 363, `objectiveDestinationID` 177, `agentCorpID` 130,
`agentFactionID` 85, `agentConstellationID` 53, `agentSolarSystemID` 49,
`agentStationID` 42, `dungeonSolarSystemID` 19, `objectiveDestinationSystemID`
11, `rewardQuantity` 8. **[V]** The `agent*` and `player` tokens are the
client's own (`GetAgentArgs`). Ours are the same five location keys
`mission-model.md` §3 names, plus `rewardQuantity`/`rewardTypeID` on the 27
Basic missions that ship a `missionRewards`.

### 2.7 Standing gates, as the client computes them

`eveCfg.CanUseAgent` (`eveCfg.py:157-193`) **[V]**:

- an Aura agent is always usable (`:157`);
- a level-1 non-research agent is usable with `noL1Check` (`:161`);
- otherwise `m = (level − 1) × 2 − 1` (`:164`), and the agent is usable when
  the **max** of the faction, corp and agent standings (each skill-adjusted,
  `:177-186`) is at least `m` **and** their **min** is above −2.0 (`:188`).

That means L2 ≥ 1.0, L3 ≥ 3.0, L4 ≥ 5.0 and L5 ≥ 7.0. Research agents also
need corp standing ≥ m − 2 (`:189`). **The server must enforce the same test
in `DoAction`**, because the client uses it only to sort agents into
available and unavailable. A refusal is `(236691, None)`,
`MissionTooLowStandings`. **[A]** that this is the right refusal line; it is
CCP's line for exactly this case.

---

## 3. The generator: agent → mission pool, and an offer instantiated

This is the core deliverable. It is a pure function of the extracted corpus,
an authored policy table and a seed. It is lazy: nothing exists for an agent
until a pilot (human or bot) opens a conversation.

### 3.1 Inputs

Two extracts that a build lane writes into `out/content/` (owner: whichever
unified extractor C1 settles; packet **K1** if it lands alone):

**`out/content/agents.tsv`**, one row per agent, 10,972 rows:

```
agentID  agentTypeID  divisionID  level  corporationID  factionID
stationID  solarSystemID  isLocator  storyLineMissions(comma list)
```

`factionID` comes from the corporation. `solarSystemID` comes from the
station through `solarsystemcontent`. `nameID` is not needed:
`cfg.eveowners` names agents from the owners prime, as it already does for
3010110.

**`out/content/missions.tsv`**, one row per mission, 2,892 rows:

```
missionID nameID template family(courier|kill|mining|talk) level level_src
offerFactionID enemyFactionID dungeonID objTypeID objQty objVolumeTotal
objGroupID dropItemTypeID expireMinutes agentTypeID corporationID
chainKey chainIndex chainLen hasNextSlot giftTypeID giftQty
rewardTypeID rewardQty bonusTypeID bonusQty bonusMinutes fixedLP
hasStandingRewards extraStandings(fac:val) contentTags messages(slot=id;...)
```

How each column is computed:

- `level`: §2.3, with `level_src` ∈ {`difficulty`, `volume_band`, `none`}.
- `enemyFactionID`: `dungeons.factionID` of the mission's dungeon.
- `chainKey`: the series key from §2.5, lowercased.
- `messages`: the `dict<str,i32>` at +72, read as `mission-model.md` §2 does.

The server's existing `--celestials` input supplies station→system and
names, **once the 5,210-station fix lands** (§1). It also needs the
stargate graph, from `jumps.static` (6,989 jumps, `fromSystemID`/`toSystemID`
**[V]**), which is in `--celestials`'s `gate` rows already or as a new
`out/content/jumps.tsv`.

### 3.2 The index built at load

```
PoolKey = (kind ∈ {security, distribution, mining}, level 1..5)
pool[PoolKey]      -> Vec<missionID>      // §3.4 filters applied at build time
family[missionID]  -> familyID            // same nameID text ⇒ same family
chain_next[missionID] -> Vec<missionID>   // §2.5 rule, candidates for part k+1
home_pirate[factionID] -> factionID       // §2.4 argmax; vassals inherit, §3.4
```

How each mission is placed:

- **kind**: from `template`. `BasicKill` → security, `BasicCourier` and
  `BasicTrade` → distribution, `BasicMining` → mining.
- **excluded**: rows with `agentTypeID = 9` (pool `fw`, deferred), rows with
  `agentTypeID = 13`, kill rows with no dungeon, chain parts with k > 1, and
  chain starts whose chain does not resolve.
- **pool sizes**: in the TSV, Block 1.
  - Security L1-5: 152/126/164/119/96 missions in 75/69/87/57/45 families.
  - Distribution L1-4: 68/46/56/33.
  - Mining L1-4: 9/10/9/12.

**Division → kind**: 24 → security, 22 → distribution, 23 → mining. Any
other division is not a pool agent. An agent's key is
`(kind(division), min(level, max_level(kind)))`, where `max_level` is 5 for
security and 4 otherwise. **[A]** for the clamp.

### 3.3 Which agents use the pool

`agentType ∈ {2, 5}`, plus type 8 **without** a `storyLineMissions` list, in
divisions 22, 23 and 24. Everything else answers by its own section (§6-§8)
or with `NoMission1` (236798) while deferred.

### 3.4 The candidate filter, per agent

Given an agent with faction F and key K, take `pool[K]` and keep mission m
only if all of these hold:

1. `m.offerFactionID` is empty, or equals F, or equals `parent(F)`.
   `parent` is **[A]**: Ammatar 500007 → Amarr 500003, and Khanid 500008 →
   Amarr. CONCORD (500006) agents keep only the empty-faction rows.
2. `m.enemyFactionID ∉ {F, parent(F)}`. No agent sends you to shoot its own
   side. **[I]**
3. If `m.enemyFactionID` is an empire (500001-500004), then F is not that
   empire's vassal. **[A]**
4. `m.corporationID` is empty, or equals the agent's corporation (one
   `BasicKill`, one `BasicCourier`). **[V]** column, **[I]** meaning.

### 3.5 Selection: deterministic, family-uniform, enemy-weighted

```
rng   = splitmix64(world_seed ^ hash(characterID, agentID, offer_seq))
fams  = distinct family(m) over the candidates, minus the last 3 families
        this character completed or declined with this agent  [A]
f     = fams[rng % len(fams)]                         // uniform over families
vars  = candidates in family f
w(m)  = 4 if m.enemy == home_pirate(F or parent(F))
        2 if m.enemy == 500021 (Rogue Drones)
        1 otherwise                                   [A] weights
m     = weighted pick over vars with rng
```

Family-uniform is the point. *The Blockade* alone has 25 variants and
would otherwise crowd out the 74 other level-1 families. `offer_seq` is
per (character, agent) and increments on every new offer, so a restart
reproduces the same offer (C7's seeding requirement). Bots call the same
function. They are pilots, not a special case.

### 3.6 Chains

When a chain start is completed, the next offer from the same agent is
`chain_next[m]`, chosen with w(·) and ties broken by the same enemy as part
k, then by lower missionID. The turn-in line is `(('UI/Agents/DefaultMessages/MissionCompletedNextMission',
{'missionCompletionText': completed_or_generic}), contentID)` when the
mission has no `nextmission` slot of its own. Declining a part k > 1 ends
the chain. **[A]**

### 3.7 Instantiating the offer: the five things the server owns

| owned value | rule |
| --- | --- |
| **courier destination** (`objectiveDestinationID`, `…SystemID`) | a station whose system is d jumps from the agent's, where d is in **[L−1, L+1]**, clamped at 0. Prefer, in order: the agent's faction's stations in the same security band (≥0.45 versus below); any faction, same band; any station. Exclude the agent's own station. **[A]**, with the **[V2]** alternative of band [L, L+2]. Level 1 at 0 jumps reproduces today's 4832 → 60014437. |
| **kill site** (`dungeonLocationID`, `dungeonSolarSystemID`) | a system in **[0, L−1]** jumps, same security band; the pocket is `crate::dungeon::spawn` of `killMission.dungeonID`'s room layout, with NPCs from C6's spawn derivation. The token value is the **solar system id**, as `mission.rs` already sends. **[A]** |
| **cargo** | `objTypeID × objQty` into the agent-station hangar on accept, as `mission.rs` does for 4832. `objectiveSingleton` stays **unknown** (`mission-model.md` §8). |
| **expiry** | `expireMinutes` when present (1,020 rows), else 7 days for an accepted mission and 7 days for an offer. **[A]** The briefing's `'Expiration Time'` is a blue filetime. |
| **reward** | §4. |

Plus `initialAgentGift*` (183 rows; 5 in the Basic pool): the items go to
the hangar on accept and are listed in `agentGift`.

### 3.8 The conversation, state by state

The `msgInfo` for each `DoAction` answer. `S(slot)` is the mission's own
message id when it has that slot. `G(label, …)` is the label form of §2.6.
The mood suffix (`Low`, `LowMed`, `MedHigh`, `High`) is **[A]**: standing
toward the agent < 0, < 2, < 5 and ≥ 5. The variant number is `rng % N`.

| state / action | agentSays |
| --- | --- |
| no mission, `DoAction(None)` | `G(RootAgentSays/<mood>N)` with actions `[(1, RequestMission=2)]`; the client then auto-calls `DoAction(1)` (`agents-missions.md` §1 Step 4) |
| offer | `S(offered)` or `G(MissionOfferN, missionBriefingText=S(briefing))`; actions Accept (3), Decline (9) |
| offer re-opened | `G(OfferStillOpen, missionOfferText=<the offer above>)` |
| accepted | `S(accepted)` or `G(MissionAccepted<mood>N)` |
| declined | `S(declined)` or `G(MissionDeclinedN)` |
| in progress, `DoAction(None)` | `S(root.missioninprogress)` or `G(RootMissionInProgress<mood>N)`; actions Complete (6), Quit (11) |
| completed | `S(completed)` or `G(MissionCompleted<mood>N)`; chain: §3.6 |
| quit | `S(quitted)` or `G(MissionQuittedN)` |
| standings too low | `G(MissionTooLowStandings)`, no actions |
| nothing left (empty candidate set) | `G(NoMission1)` |
| offer expired, as EVE-mail | `S(offerexpired.email.*)` or `MissionOfferExpirationEmail{Header,Body}` |

### 3.9 Wire changes the generator implies

1. **`agentMgr.GetAgents`** returns all 10,972 rows from `agents.tsv`, in
   the same `Rowset` instance and with the same ten columns
   (`isLocator ← agentLocateCharacter`). The 13 epic-arc rows become real
   (§7.4). This also populates the Agency's agent finder, which reads only
   this rowset, and the lobby's Agents tab at every station.
2. **Every `Moniker('agentMgr', agentID)` routes to one generic handler.**
   Today it is hard-wired to `AGENT_ID`. `Missions` becomes a map keyed by
   (characterID, agentID): a `Progress { missionID, state, offer_seq,
   pocket, accepted_at, expires_at, history[3] }`.
3. **`GetMyJournalDetails`** lists every (agent, mission) in state 0-2. That
   means several rows, with offers in `[0]` and in states 0/1
   (`ON_GOING_MISSION`, `agents-missions.md` §4.1).
4. **`OnAgentMissionChange(event, agentID)`** carries the real agent id,
   which is a constant today.

---

## 4. Rewards, loyalty points and standings

The Basic family carries **no** reward (`mission-model.md` §1.2). Rule set
**[V2]**, which we would ship as **[A]**:

- **ISK, bonus ISK and bonus window** per `(scale, level)`: the table once
  quoted here was overruled by ruling 9; the shipped values are in
  `content/agents/rewards.tsv`.

- **LP = base × max(0.1, 1.6288 − round(security, 1))** **[V2]**, credited
  to the agent's corporation. No LP for Storyline, GenericStoryline, EpicArc
  or Research missions. **[V2]**
- **A mission's own `missionRewards` wins** where present (27 Basic rows;
  ISK is `typeID 29`). `fixedLpReward*` wins where nonzero (only Heraldry).
  **[V]** columns.
- **Standing on turn-in.** Per level, agent / corp gains are
  0.010/0.018, 0.014/0.026, 0.020/0.038, 0.028/0.055 and 0.036/0.072
  **[V2]**. They replace `mission.rs`'s flat 0.10 / 0.02 **[A]**. Quitting
  costs the agent −0.041 and the corp −0.0035 **[V2]**, from one Tranquility
  capture. A second decline inside 4 h costs half of that **[A]**. Storyline
  missions add `extraStandings` (41 rows, `{factionID: f32}`) toward that
  faction **[V]** column, plus 0.1 × level toward the agent's faction
  **[A]**.
- **On the wire:**
  - `lastActionInfo['loyaltyPoints']` and `GetMissionObjectiveInfo
    ['loyaltyPoints']` carry the number;
  - `LPSvc.GetAllMyCharacterWalletLPBalances` answers `[(corpID, balance),
    …]` (the shape is already `[V]` in `wallet.rs`);
  - push **`OnLPChange(changeType, issuerCorpID, lpBefore, lpAfter)`**,
    4 args (`loyaltyPointsWalletSvc.py:67`) **[V]**. The handler discards
    `changeType` and refreshes. **[V]**
  - the `bonusRewards` 5-tuple carries the bonus.

All money numbers are C5's to rebalance. These are the starting point, not
the economy.

---

## 5. The LP store

### 5.1 What the client asks, and the shape it wants [V]

- `LPStoreMgr.GetAvailableOffersFromCorp(corpID)` is cached per corporation
  while the `is None` check holds (`lpStoreSvc.py:140-144`). It answers a
  **list of `utillib.KeyVal`**. The client calls `data.Get(name, 0)` and
  `data.Set('sort_…', v)` on each element (`lpStoreSvc.py:526`, `:538`), and
  `KeyVal.Get`/`Set` exist (`utillib/__init__.py:59`, `:62`).
- The keys read are `offerID`, `corpID`, `typeID`, `qty`, `lpCost`,
  `iskCost`, `akCost` (read as `Get`, default 0) and `reqItems`.
  `reqItems` is a list of `(typeID, qty)`, subscripted `[0]`/`[1]` at
  `:372`. `requiredStandings` is optional and is a dict with `'value'`,
  read at `:533-534`. The cost names come from `lpstoreRequirement.py:45-123`
  (`lpCost`, `iskCost`, `akCost`).
- `LPStoreMgr.TakeOfferForCharacter(corpID, offerID, numberOfOffers)` is
  truth-tested: on success it shows `LPStoreOfferAccepted`
  (`lpStoreSvc.py:315-324`). The server then debits LP and ISK, removes
  `reqItems`, creates `typeID × qty × n` in the hangar, and pushes
  `OnLPChange` and `OnAccountChange`.
- `TakeOfferForCorporation` (corp wallets) is deferred.

### 5.2 Authoring: per offer table, from a rule, not by hand

`npccorporations.lpOfferTables` (`list<i32>` at +88) gives **181 of 283**
corporations between 1 and 7 table ids. There are **76 distinct tables**, and
168 of the 178 agent corporations have at least one. **[V]** Each table's
membership classifies it. The offers inside are server data, and none ship.

| table class (by member corporations) | examples | contents rule |
| --- | --- | --- |
| **universal**: table 62 on 162 corporations; 57, 58, 59 and 66 on 43-63 mixed-faction corporations | 62 | attribute and hardwiring implants. These are the named-brand meta-0 families whose corporation name prefixes the item: Zainou (219 types), Inherent Implants (106), Eifyr and Co. (87), Poteque (14), Genolution (6) **[V]** counts. |
| **empire bulk**: one empire, 10-45 corporations | 3, 5, 6, 7, 9, 10, 11, 13, 14, 15, 61, 67 | that empire's racial T1 ship and module blueprint copies [A] |
| **navy**: one empire, military main activity (`mainActivityID` 5), 4-10 corporations | 4 Amarr, 8 Caldari, 12 Gallente, 16 Minmatar | the faction items named for that navy: "Caldari Navy …" 137, "Republic Fleet …" 156, "Imperial Navy …" 131, "Federation Navy …" 132, all published with a market group **[V]** |
| **minor or pirate faction, one or two corporations** | 18-24 Ammatar and Khanid, 26-27 Mordu's, 28-29 Syndicate, 31 ORE, 33-34 Thukker, 38 and 101-106 Sisters, 40-52 pirates, 214-217 Malakim and Commando Guri | items with that faction's prefix: "Ammatar Navy", "Khanid Navy", "Dark Blood", "Domination", "Dread Guristas", "True Sansha", "Shadow Serpentis", "Sisters", "Thukker", "Syndicate", "ORE" |
| **FW militia** | 79-92 | deferred with FW |
| **special** | 94 and 113 CONCORD/DED, 107-111 war reserves, 212 Triglavian, 213 Paragon, 237, 240 | deferred **[A]** |

**Prices [A]:** `lpCost = round_sig2(V / 1000)` with a floor of 100.
`iskCost = lpCost × 1,000`. V is the ISK value: the item's own `basePrice`,
or its `variationParentTypeID`'s `basePrice` × 20 for meta-4 faction items
(their own `basePrice` is mostly 0) **[V]**. 1,000 ISK per LP is the
conventional exchange rate, and C5 owns it. `reqItems` for navy items is
**[A]** one of the T1 parent (`variationParentTypeID`), matching the
Tranquility pattern.

**Where it lives.** The rule runs in the extractor and writes
`out/content/lp_offers.tsv` with the columns `tableID offerID typeID qty
lpCost iskCost reqItems`. offerID is `tableID × 10,000 + n`, so ids stay
stable when the type set is stable. The repo holds only an authored
override file, `content/lp_overrides.tsv` (`tableID typeID action
lpCost iskCost`), so no client-derived list is committed. A corporation's
offers are the union of its tables' offers.

---

## 6. The storyline trigger

**[V2]** rule: complete **16** Basic missions (any division) for agents of
faction F at level L, and a storyline offer of level L appears. The counter
is keyed (character, F, L).

Server rule **[A]**:

- On the 16th completion, pick the nearest type-6 or type-7 agent of faction
  F by jumps (`agents.tsv` gives 651 + 11, and every empire has 82-142
  type-6 agents, Block 3).
- Draw from the `GenericStoryline*` rows at level L: 263 rows; levels by
  §2.3 are kill 7/14/53/65/4 and courier 37/35/4/39. Filter by
  `offerFactionID ∈ {empty, F}`.
- Create the offer, `OnAgentMissionChange('offered', thatAgentID)`. The
  `offered` string is **[V]** (`agents-missions.md` §4.1), and the offer
  row shows in the journal's offers list.
- **Exception to the standing gate**: storyline agents are level 1 in the
  data, so `CanUseAgent` passes regardless.
- Storyline completion pays faction standing (§4) and no LP.
- The `ImportantMission` and `ImportantStandingsWarning` labels exist
  (610737, 235555) for the briefing **[V]**, and decline and quit use the
  `…Important…` titles.

---

## 7. Epic arcs

### 7.1 `epicarcs.fsdbinary`, decoded, with one correction to the tree

21 arcs, stride 56. `missions` at +16 is **`dict<u32 missionID, struct>`**,
not `dict<str,…>` as `findings/pyd-schemas.tsv` has it. Each entry is
32 bytes: a `u64` key holding the missionID, then the 24-byte value. That is
the same correction `mission-model.md` §7(a) made for
`missions.extraStandings`. The value struct holds `nextMissions` (list<i32>,
bit 0x2), `agentID` at +8, `failMissionID` at +12 (bit 0x1) and the
presence word at +16. **[V]** The other columns:

- `arcRestartInterval` at +32: 129,600 on the real arcs and 1 on the
  Pilot Certification arcs. 129,600 minutes is 90 days; EvEJS uses 90 days
  **[V2]**, so minutes is **[I]**.
- `epicArcNameID` at +36, `factionID` at +40 (bit 0x2), `iconID` at +44
  (bit 0x4).

All 275 mission ids close into `missions`, every `nextMissions` target is
inside its own arc, and every agent is an `npccharacters` agent. **[V]**

| arc | name | faction | missions | level | start (agent) |
| --- | --- | --- | --- | --- | --- |
| 29 | The Blood-Stained Stars | Sisters 500016 | 64 | 1 | 14117 (3019356 Sister Alitura) |
| 40 | Penumbra | Caldari | 40 | 4 | 14849 (3019396) |
| 48 | Right to Rule | Amarr | 24 | 4 | 14916 (3019415) |
| 52 | Syndication | Gallente | 27 | 4 | 14831 (3019412) |
| 53 | Wildfire | Minmatar | 19 | 4 | 14851 (3019411) |
| 55 | Smash and Grab | Guristas | 20 | 3 | 4 starts (3019463, …) |
| 56 | Angel Sound | Angel | 22 | 3 | 4 starts (3019466, …) |
| 128 | Fractured Legacy | ORE | 3 | 1 | 17130 (3020089) |
| 132 | Visions of Greatness | 500028 | 8 | 1 | 17188 (3020233) |
| 64, 67-77 | Pilot Certification Course × 12 | – | 4 each | 1 | fail-mission loop; restart 1 |

The start rule is the client's own: `get_all_start_missions` is
`missions.keys() − ∪ nextMissions` (`evemissions/common/epic_arcs_data.py:78`)
**[V]**. The 12 Pilot Certification arcs have no start under it, because
`failMissionID` loops back. `IsValidArc` hides any arc whose restart
interval is ≤ 1 (`epicArcController.py:41-42`) **[V]**. They are Aura NPE
content, and are **deferred**.

Our station 60012526 holds **Dovirch Anselm (3019363)**, an agent in
*The Blood-Stained Stars*. The arc's 12 agents lie 0-29 jumps from Tanoo,
and its start (Sister Alitura) is 17 jumps away. **[V]** It is the natural
first co-op arc: level 1, 64 missions, and it passes through our own
station.

### 7.2 The calls and their shapes [V]

- `agentMgr.GetMyEpicArcStatus()` → `{epicArcID: {missionID: status}}`,
  where `status` has `.acceptedDate`, `.completedDate`, `.quitDate` and
  `.nameID` (`epicArcController.py:95-103`, attribute reads, so a `KeyVal`).
  Missing ids are `None` (`.get`).
- `agentMgr.GetMessagesForEpicArcMissions()` → `{slotName: {missionID:
  messageID}}` with `slotName ∈ {'messages.epicMission.journalText.
  chapterTitle', …inProgressMessage, …completedMessage}`
  (`epic_arcs_data.py` constants, read at `epicArcController.py:65-66`). It
  is **derived entirely from `missions.messages`**: 278 `EpicArc*` rows
  carry `epicMission.*` slots.
- `epicArcStatus.AgentHasEpicMissionsForCharacter(agentID)` → truthy when
  that agent has an available or active arc mission for the caller
  (`agentUtil.py:43`, `:68`).
- Push **`OnEpicJournalChange()`**, no arguments (`epicArcSvc.py:42`), on any
  arc-state change. `OnAgentMissionChange` covers the per-mission journal.

### 7.3 The server state machine

Per (character, arc), keep `{missionID: (accepted, completed, quit)}` plus
`completedAt`. The rules **[A]**:

- The start agent offers the start mission.
- Completing m offers `nextMissions[m]` from *that* mission's `agentID`, a
  different agent each time. When there are several next missions, the arc
  **branches**, and the player picks by talking to one of the next agents.
  17 rows branch. **[V]** count.
- Quitting or failing offers `failMissionID` where set.
- The arc closes when an end mission (no `nextMissions`) completes; the
  restart time is then `completedAt + arcRestartInterval` minutes.
  `GetReplayTimestamp(characterID)` on the start agent returns it, and
  `lastActionInfo['missionCantReplay']` carries the seconds left (message
  631990, `agents-missions.md` §4.4). `MissionConfirmArcRestart` (236718)
  is CCP's line for the restart.
- `EpicArcTalkToAgent` (53) and `EpicArcAgentInteraction` (20) have no
  objective struct. They complete on talking to the next agent: the
  objective is `('agent', (agentID, agentLocation))` (`agents-missions.md`
  §4.5). Kill and courier arc missions use §3.7, and their fixed
  `missionRewards` apply (183 of 186 kill rows carry one).

### 7.4 The 13 epic-arc rows in `GetAgents` stop being placeholders

`EPIC_ARC_STARTER_AGENTS` in `service.rs` are the client's own
(`standingUIConst`). Seven of them are the arc starts in the table above,
and the other six are the alternate starts of arcs 55 and 56. **[V]** Their
real rows come out of `agents.tsv`. With real `factionID`s the
standings-panel threshold icon appears, which is now **correct**, because
the arcs exist. That is a behaviour change to flag in `service/tests.rs`'s
expectations.

---

## 8. Agents with a fixed mission list: career and event agents

- **Career agents (type 12)**: 100 agents in divisions 25-29, 20 per
  division (one career path each, at the school corporations). Each has a
  10-mission `storyLineMissions` list, for example *Cash Flow for
  Capsuleers (1..10 of 10)*. The list is **stored in hash order, not play
  order**, so sort by the `(k of n)` index in the title. **[V]** Rule
  **[A]**: offer the lowest uncompleted index; completed career missions are
  not replayable. Across all `Storyline*` rows, 131 carry `contentTags`
  (`career_path_*`, `activity_*`) and 187 carry a `clientObjectives` AIR
  chain. Serving the classic agent window does not need the AIR chain
  (`mission-model.md` §4.2); the Agency's objective tracker would need it,
  and it is deferred.
- **Event agents with lists (type 8, 367 agents)**: these are lists of
  `Storyline*` missions; 302 lists are unique to one agent. Same rule:
  play order by the `(k of n)` index, or missionID when no index exists.
  Replay **[A]**: once per character.

---

## 9. `conversations` and `conversationagents`: not agent missions

- `conversations.fsdbinary`: 1,275 conversations, 1,482 lines, every `text`
  id resolving. **[V]** `lines` is `list<struct LineOfDialogue>` stored
  **inline**, 72 bytes per element: `agentID` at +48, `text` at +60, and the
  presence word at +64. Reading the list as a list of pointers fails. **[V]**
- `conversationagents.fsdbinary`: 44 speakers, keyed by their own small ids
  (Aura = 5, Elias Peltonnen = 2026). These are **not** `npccharacters` ids.
  The same Elias Peltonnen is agent 3020089 in `npccharacters`.
- Line counts by speaker: Aura 685, and the Agency event commanders 79-82
  each. The conversations are NPE and Agency-event talking heads
  ("Combat Aura 4.1", "Handoff 3: Set Destination").

The one server hook is the push **`OnStartConversation(conversation_id,
show_continue_button)`** (`conversationservice.py:94`, 2 args) **[V]**,
normally fired by the AIR objective-chain and nodegraph engine. For C4 the
decision is: **do not use conversations for agent missions.** They belong to
the deferred AIR/NPE lane. One cheap use, **[A]**: push the matching
conversation when an Elias Peltonnen arc (128) mission is offered. That
needs a hand map, and is optional.

---

## 10. Deferred, with the reason

| feature | why it waits | what it would need |
| --- | --- | --- |
| Factional warfare agents (260) and the `fw` pool (160 missions, 40 per level) | needs militia membership (`MissionNotSameFaction`, 236681) and the warzone | C2's warzone surface |
| Research agents (244) | research points, datacores; a separate economy | C5 |
| Heraldry, Paragon (78 missions, 12 IRIS agents) | cosmetics; the reward is SKIN delivery | nothing for co-op |
| Aura and Pilot Certification (12 arcs) | hidden by `IsValidArc`; AIR NPE content | the NPE lane |
| Tutorial agents (14) | superseded by AIR NPE | – |
| AIR objective chains (`clientObjectives`, 187 rows) | the Agency's tracker; the classic window works without them | its own decode lane |
| Corp-wallet LP purchases | corp roles | C5 |

---

## 11. Corrections to files in the tree (this lane owns none of them)

1. `findings/pyd-schemas.tsv`, row `epicarcs missions`: the type is
   **`dict<u32 missionID, struct>`**, with 32-byte entries (u64 key, then a
   24-byte value). Read as `dict<str,…>`, the keys come out as `None` or
   garbage. **[V]**
2. `findings/pyd-schemas.tsv`, the `conversations lines` note: the elements
   are **inline 72-byte structs**, not pointers. **[V]**
3. `docs/re/mission-model.md` §1.1: Kaeg Zkaen (3018559) is agentType **8**,
   division 24. §8's "a mission's level" is answered for dungeon missions by
   `dungeons.difficulty` (§2.3 here).
4. `service.rs::agent_rowset`'s comment, *"the real faction/corp mapping is
   server data found nowhere in the client tree"*: it is in `npccharacters`
   plus `npccorporations` (§7.4).
5. `tools/celestials.py:151` emits `planet.npcStations` only. The 5,210
   stations include moon stations: 1,223 are emitted now, and 1,149 of the
   5,063 agent stations are covered. That blocks every agent-placement
   packet. The owner is the C1/C3 extractor. **[V]**

---

## 12. Proposals

1. **Derive the pool; don't author it.** Adopt §3: level from
   `dungeons.difficulty` (courier volume bands as **[A]**), kind from the
   template, faction filters from `missions.factionID` and
   `dungeons.factionID`, home pirate from the faction-locked cross-tab.
   Deterministic, family-uniform selection.
2. **Use CCP's generic agent lines for every missing slot**, through the
   label form of `ProcessMessage`. Never author mission prose. Switch to the
   two-step offer with a `RootAgentSays` greeting.
3. **Serve all 10,972 agents in `GetAgents`**, route every `agentMgr`
   moniker to one handler, and key mission state by (character, agent).
4. **Rewards from one authored table**, `(scale, level) → ISK, bonus, LP
   base, bonus window, standing gains`, seeded with the §4 **[V2]** numbers.
   LP scales with security as `1.6288 − sec`. C5 may retune it.
5. **The LP store is a derivation rule in the extractor plus a small
   override file.** Contents come from the table class (§5.2), with prices
   at 1,000 ISK/LP from base value.
6. **Ship epic arcs straight from `epicarcs.fsdbinary`**, the 8 valid arcs,
   with the state machine in §7.3. *The Blood-Stained Stars* is the first
   co-op target, because one of its agents is in our station.
7. **Storyline trigger at 16** per (character, faction, level), offered by
   the nearest type 6 or 7 agent.
8. **Bots use the same generator API** as humans. The bot verb is "take the
   offer from agent X", and nothing about selection is bot-specific.
9. **Fix the station extract to 5,210 stations before any packet below
   lands** (§11.5).
10. **Defer** FW, research, heraldry, Aura/NPE, AIR objective chains and
    conversations.

---

## 13. Packets

Ranked by what they unblock for co-op plus bots. Each packet owns its files
alone (lane rule 1).

**K1: content extract for agents and missions.**
- **Owns:** a `tools/content_agents.py` module, or a sub-command of C1's
  unified extractor, plus `out/content/agents.tsv`, `missions.tsv`,
  `epicarcs.tsv` and `lp_offers.tsv`.
- **Writes:** the §3.1 and §5.2 columns.
- **Tests** (Python, scratch-free, against the installed client):
  - row counts: 10,972 agents, 2,892 missions, 275 arc missions over 21 arcs;
  - mission 4832 is `courier`, level 1 by volume band;
  - 2924 is `kill`, level 1, enemy 500011;
  - 2504/3081/3082/3083 are levels 1/2/3/4;
  - arc 128's start is 17130;
  - agent 3010110 has faction 500007, system 30000001, `isLocator` 0.
- **Depends on** the station fix (§11.5), unless K1 reads
  `solarsystemcontent` itself.

**K2: `src/mission_pool.rs`, the generator (pure, no I/O).**
- **Owns:** `mission_pool.rs`.
- **API:**
  - `Pool::load(agents.tsv, missions.tsv, jumps)`;
  - `Pool::key(agent)`;
  - `Pool::offer(seed, char, agent, offer_seq, history) -> MissionId`;
  - `Pool::destination(agent, level, rng)`;
  - `Pool::site(agent, level, rng)`;
  - `Pool::next_in_chain(m, prev_enemy)`;
  - `reward(scale, level, security) -> Reward`.
- **Tests:**
  - the same inputs give the same offer;
  - the family-uniform draw over 10k draws keeps the Blockade family under
    5% at L1;
  - a Caldari agent never gets an enemy of 500001;
  - an Ammatar agent accepts Amarr-locked rows;
  - L5 distribution draws from L4;
  - a chain start is followed by its part 2;
  - a level-1 courier from 60012526 can resolve to 0 jumps;
  - LP at security 0.9 is base × 0.7288.

**K3: generalize `mission.rs` to N agents × N missions.**
- **Owns:** `mission.rs`. Coordinate the `service/character.rs` agentMgr
  arm as one hand-off edit.
- **Changes:** replace `Kind`/`COURIER`/`BLOCKADE` with a `Spec` built from
  a `missions.tsv` row; key `Missions` by (character, agent); compose the
  conversation per §3.8; enforce `CanUseAgent` (§2.7); emit
  `GetMyJournalDetails` for all agents.
- **Tests:**
  - the existing ten `mission.rs` tests still pass, now with 4832 and 2924
    *drawn* rather than hard-coded;
  - a mission with no `offered` slot produces the `MissionOfferN` label
    tuple with `missionBriefingText`;
  - an L2 agent refuses at standing 0.9 and accepts at 1.0;
  - two agents' missions coexist in the journal.

**K4: the full agent rowset.**
- **Owns:** `service.rs::agent_rowset`. It is a hot file, so this goes
  through the coordinator.
- **Changes:** 10,972 rows from `agents.tsv`; the 13 epic-arc rows become
  real rows.
- **Tests:**
  - the `Rowset` instance shape is unchanged;
  - every row has 10 elements;
  - agent 3019415's `factionID` is 500003;
  - the station filter for 60012526 returns 3 agents.

**K5: rewards, LP wallet and the LP store.**
- **Owns:** `service/wallet.rs` (the `LPSvc` and `LPStoreMgr` arms) and a
  new `src/loyalty.rs` (balances, debit, credit, the `OnLPChange` push).
- **Tests:**
  - the balance list is `(corpID, balance)` pairs;
  - an offer is a `KeyVal` with all 8 keys;
  - `TakeOfferForCharacter` debits LP and ISK and creates the item;
  - insufficient LP answers falsy;
  - a mission turn-in credits LP to the agent's corporation and pushes
    `OnLPChange` with 4 args.

**K6: epic arcs.**
- **Owns:** a new `src/epic_arc.rs` plus the `epicArcStatus` service arm.
- **Changes:** §7.2 calls, §7.3 state machine, the `OnEpicJournalChange`
  push.
- **Tests:**
  - `GetMyEpicArcStatus` is `{}` for a new character and holds one entry
    after the start is accepted;
  - completing 17130 offers 17131 or 17132;
  - an end mission sets the 90-day replay;
  - the message map holds 278 missions;
  - Pilot Certification arcs are never offered.

**K7: the storyline trigger and the career and event lists.**
- **Owns:** a new `src/storyline.rs`.
- **Tests:**
  - the 16th L1 Caldari completion creates an offer at the nearest type-6
    Caldari agent and pushes `'offered'`;
  - a career agent offers *(1 of 10)* first, whatever the hash order.

**K8: bot verbs.**
- **Owns:** `eve-bot/src/verbs.rs`, after the W10 lane releases it.
- **Changes:** "talk to agent, accept, run, turn in" over the same wire
  calls.
- **Tests:** a bot completes 4832 against a live server in the existing bot
  harness.

---

## 14. unknown

- **Courier mission level.** No column holds it. The volume bands in §2.3
  are **[A]**.
- **`objectiveSingleton`** and `cargo['volume']` per unit versus total.
  Inherited from `mission-model.md` §8.
- **Whether `GetMyEpicArcStatus`'s `status` must be a `KeyVal`** or whether
  any attribute-bearing object works. Attribute access is **[V]**; the class
  is **[I]**.
- **`requiredStandings`' full shape** beyond `['value']`, for example which
  owner the standing is toward.
- **Whether `GetMyJournalDetails[1]`** (the offers list) is where a
  storyline offer appears. Inherited **[I]** from `agents-missions.md` §11.
- **The 75 two-variant courier families**: whether the pair is (L1, L3) or
  (L2, L4).
- **The 55 unreferenced `StorylineTrade` rows**: whether any Tranquility
  agent offered them.
- **The remaining client fields of the LP offer**: `lootItems` is read by
  `lpOfferEntry2` with `Get`, and its shape was not followed.
- **The CONCORD-enemy (500006) kill variants** (16 rows): what they meant on
  Tranquility. They are treated as neutral enemies.
- **The Tranquility LP-store contents and prices.** Not in the client. §5.2
  is a rule, not a recovery.
