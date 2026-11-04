# Server-only content: what the client never ships

**The answer.** The client ships what the universe *is*: every system, station,
gate, belt and its ore, every dungeon with its rooms, objects and spawn points,
every mission with its prose, every NPC type with its numbers, every recipe,
and most of the rules (taxes, insurance, fatigue, damage, skills, sov indices).
What only the server holds falls into three groups:
- **placement and pacing:** how many sites a system has, where rocks sit, when
  things respawn;
- **tables that the client points at by id but never receives:** loot tables,
  the NPC fleet templates behind a dungeon spawn, spawn triggers, LP store
  offers, and the per-system, per-moon and per-planet shard data (wormhole
  statics, moon ores, PI resource maps);
- **money and reputation numbers:** mission rewards, standing gains, insurance
  prices, ESS and interference settings, FW victory points, and the security
  loss for crimes.

Almost all of it is already replaced. We derive from the client wherever the
client has the facts, and author the rest as small, marked tables
(`content/`, `docs/content/plan.md` §2). **A capture would add something we
cannot get any other way for 17 items** (§4). For most of them it would only
make a number match Tranquility (TQ) where any reasonable number already plays.
The ones that would change how the game feels are:
- the exact NPC squads in dungeons and missions;
- the dungeon wave triggers;
- the escalation chains;
- abyssal waves;
- FW victory-point values;
- probe-scan strength against ships.

Even for those, one capture shows what one run produced, not the table behind
it. Taking a capture is a new owner decision: the client never gets a route to
CCP, a live capture would come from a later client build than the pinned one,
and its contents are CCP's data, so they could live only in `out/`.

## How to read this

**Source kind**, one for each item:
- **derivable:** client data (`res:/staticdata`, the type and dogma tables) or
  client code (the bytecode the client runs) decides the answer. We compute it,
  extract it, or copy the rule.
- **design:** a tuning or policy choice. The client accepts any value, and
  CCP's value is only their taste. A capture would show samples at best (one
  drop, one board, one spawn), never the rule behind them.
- **capture-only:** an exact TQ value or table that neither client data nor
  client code reveals, *and* that a capture would actually show us, either
  because it travels on the wire or because a few observations pin it down.
  Matching TQ would need a capture. Some of these also have a public CCP web
  API that serves the same values; that is noted where it applies. Using it
  is also an owner decision.

**Marks:**
- `[V]`: verified, with the file:line or doc section given.
- `[I]`: inferred, with what it is inferred from.
- `[A]`: assumed, with what would falsify it.

Facts taken from EvEJS are marked `[A]` until the client confirms them. Paths
starting `src/` are under `server/crates/eve-server/`. Issue numbers are
public issues, checked against the `issue` column of `docs/status/progress.tsv`.

## 3. The main table

### Universe and space

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Systems, celestials, stations, gates, security | ids and balls (`AddBalls`); it resolves the rest itself | `solarsystemcontent.static`, `systems.static`, `regions`, `constellations` | the bundle's `universe/*.tsv` (`tools/content.py`) | derivable | [V] C1 §2.1, `docs/content/schema.md` §2 | |
| Dock and undock geometry | the undock position in the ball updates | per station type in `graphiclocations`; the station's own position and rotation | `universe/dock-locators.tsv` | derivable | [V] schema.md `universe/dock-locators.tsv`; [I] the join, `docs/re/evejs-data.md` §4.4 | |
| What ore each belt holds | rock balls of the right ore | the rock count per ore for every belt | `belts/rocks.tsv`, `src/asteroid.rs` | derivable | [V] C3 §0 item 5, §1 | |
| Where each rock sits in a belt | rock positions in `AddBalls` | nothing | seeded positions, `src/asteroid.rs:24` | design | [A] `asteroid.rs:24`; falsified by a client that refuses a rock off some locator | |
| Belt depletion and regrowth | rock counts (the client grades depletion at 0.9, 0.3 and 0) | the grading thresholds only | full reset at downtime | design | [V] C3 §0 item 5, §7.3 | #15 |
| Wormhole type physics (mass, lifetime, jump mass) | slim fields and the jump's refusals | dogma attributes per wormhole type | `src/wormhole.rs`, `src/wormhole_jump.rs` | derivable | [V] R-WH §1 | |
| Wormhole statics per system | the hole ball and its slim item | nothing; the dungeon blocks give a class-level pattern only | a class-level rule (`content/wormholes/statics.tsv`) and a seeded draw per system (`src/wormhole.rs:9-21`) | capture-only | [V] R-WH §0, `docs/re/evejs-data.md` §4.3 | |
| K162 rules, wormhole collapse, mass regeneration | the slim's size and age enums; collapse pushes | the enums; the regeneration attribute without its cycle | R-WH §4 rules | design | [A] R-WH §4.2; [V] that the regeneration cycle is unknown, R-WH §8 item 3 | |
| Wormhole system effects | the effect beacon's dogma on the ship | the system's effect beacon and its effects | `universe/system-effects.tsv`, `src/system_effect.rs` | derivable | [V] `system_effect.rs:1-8` | |
| Moon composition | survey results (an ore fraction per moon) | the moon-ore groups and rarity attributes; no per-moon table | seeded draw from the world seed, `src/moon.rs:23-28` | capture-only | [V] `moon.rs:23-28`; `docs/re/evejs-data.md` §4.5 | #52, #55 |
| Planetary resource maps | `planetMgr.GetResourceData`: a coefficient blob that native client code evaluates | the evaluator only (native code) | `data: None` (`src/service/planet.rs:940-944`); extraction yields use one assumed field value (`src/planet.rs:60`) | capture-only | [V] `planet.rs:3221-3237` | |
| Landmarks, observatory and odd-system lists | beacons in space | a small landmarks table; no observatory list | nothing | design | [V] `docs/re/evejs-data.md` §4.8 | |
| Time dilation | a factor in the destiny stream | the client's handling of the factor | not built | design | [V] `docs/status/progress.tsv` "Time dilation: missing" | |

### NPCs and spawns

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| NPC stats, bounty, security gain, tackle and ewar chances, missiles | balls whose type the client knows; damage and FX pushes | per-type dogma (bounty, chase and attack-delay windows, scram and web chances, missile type) | `npc/types.tsv`, `npc/bounty.tsv`, `src/npc_data.rs` | derivable | [V] C6 §2.5 | |
| Aggression thresholds, threat range, default AI name, spawner timing vocabulary | nothing on the wire | `spacecomponentsbytype` per type | `npc/components.tsv` | derivable | [V] C1 §2.6, `docs/re/npc-behaviors.md` §6.2 | |
| Behaviour trees (CCP's AI data) | movement, lock and FX pushes, as for players | the engine and its task vocabulary; **no tree data** | our brain (`src/npc/brain.rs`) driven by the per-type numbers | design | [V] `docs/re/npc-behaviors.md` §1.4 | |
| Group aggression, a drone's aggression counting as its owner's | locks and pushes | the rule, in the shipped `behaviors` code | followed | derivable | [V] `docs/re/npc-behaviors.md` §5.4 | |
| Belt rat composition | rat balls in belts | none of the client's spawner rows is a belt spawner; the NPC group-name grammar and bounty ladders are there | budget squads from the grammar, `src/npc_gen.rs`, `src/belts.rs` | design | [V] C6 §0 items 1-3, §2.3 | |
| Which pirates own a region | rat faction in belts and sites | faction HQs; empire-to-pirate links in mission dungeons | `npc/regions.tsv` by rule, plus `content/npc/region-pirates.tsv` overrides | derivable | [I] from C6 §2.7 (HQs plus mission-dungeon factions); overrides are [A] | |
| Squad behind a dungeon or mission spawn point (`npcGroupingID`) | the ships at that spawn point | the spawn point, its faction and an id that points outside the corpus | budget squads per entity, `npc_gen::dungeon_squads` (C6 R5) | capture-only | [V] `docs/re/dungeon-model.md` §5.5, C6 §2.4 | |
| Dungeon spawn triggers (which event releases which wave) | waves arriving | two booleans (`triggerSpawn`, `objectTriggerSpawn`) and the trigger vocabulary; no trigger wiring | our trigger rule, `src/site_spawn.rs` | capture-only | [V] `docs/re/dungeon-model.md` §5.3 | |
| Live-event and warfare spawn tables (reactive spawners) | event and insurgency spawns | spawner parameters; table and fleet-template ids point outside the corpus | nothing (not built) | capture-only | [V] C6 §2.3 | |
| In-site respawn delay, aggression range, orbit range | timing and behaviour | `guardCommand` on each dungeon object | read from `dungeons/spawns.tsv` | derivable | [V] `docs/re/npc-behaviors.md` §6.1 | |
| CONCORD response time and squad size | ships arriving | CONCORD's corporation id; nothing on timing | owner decision (1 s at 1.0 rising to 10 s), squads [A] (`src/concord.rs:122-132`) | design | [V] `concord.rs:122-127` (owner decision); [A] `:129-132` | |
| Faction police, sentry guns: placement and targeting | ships and guns firing | the groups and the sentry switch delay; no placement or targeting rule | `npc/police.tsv`, `src/sentry.rs` | design | [V] `sentry.rs:1-10` | |

### Loot and drops

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| NPC wreck loot | wreck contents | references to loot-table ids from five tables; **no table of loot tables** | `npc/loot.tsv` pools by faction, size, tier and bounty; `src/npc/loot.rs` | design | [V] `docs/re/dungeon-model.md` §6; [A] the rates, `loot.rs:6-9` | |
| Data and relic can loot | can contents after a hack | nothing | `explore/loot.tsv`, `src/hacking_loot.rs` | design | [V] `hacking_loot.rs` header | |
| Combat site end cache | a wreck in the last room | a reference on a handful of objects that points at nothing | `explore/site-loot.tsv`, `src/combat_site.rs:64-70` | design | [V] `combat_site.rs:64-70` | |
| Wreck type per hull | the wreck ball | the wreck type on each type | read from the type data | derivable | [V] C1 §2.6 | |
| Salvage from a wreck | salvage items | the salvage group; no yield table | `salvage/yields.tsv`, made by an extractor rule | design | [V] schema.md `salvage/yields.tsv` ("the whole table is [A]") | |
| Geode contents, booster blueprint drops | items in a container or wreck | the crates; no booster blueprint source | `content/econ/geode-loot.tsv`, `src/booster_sources.rs` (owner decisions) | design | [V] `booster_sources.rs` header | |
| Abyssal loot | the cache at the end of a room | nothing | not built | design | [V] progress.tsv "Abyss depth: missing" | #4, #5, #6, #62 |

### Missions and agents

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Agents: where they sit, level, division, type | `GetAgents` rows | `npccharacters` | `agents/agents.tsv` | derivable | [V] C1 §2.7, C4 §1 | |
| Which missions an agent offers | the offer | the pool was TQ server data, but the client holds enough to derive it (level, offering and enemy faction, division, chains) | `src/mission_pool.rs` | derivable | [V] C4 summary, §2-§3 | |
| Mission content: briefing, objectives, pocket | the mission's message ids and dungeon | `missions`, `dungeons` | `agents/missions.tsv`, `agents/mission-messages.tsv` | derivable | [V] `docs/re/mission-model.md` §1-§2 | |
| Courier destinations, where a kill pocket is placed | bookmarks and locations | nothing | our placement rule | design | [V] C4 summary ("geography ... we still author") | |
| Mission ISK, time bonus and LP | `normalRewards`, `bonusRewards`, `loyaltyPoints` in the offer | Basic missions carry no reward; a few carry their own | `content/agents/rewards.tsv` over a bounty-derived base, `src/mission_rewards.rs:16-25` | capture-only | [V] C4 §4, `docs/content/rulings.md` 9 | |
| Standing gain on completion, the cost of declining | standings changes | the client's standing gates and caps; no gain values | our per-level gains, `agent_gain` and `corp_gain` in `content/agents/rewards.tsv` | capture-only | [A] the rewards table (ruling 9: our numbers); C4 §4 records EvEJS's figures, one of them from a single TQ capture | |
| Storyline trigger (every Nth mission) | a storyline offer | the storyline missions and agents | every Nth Basic mission, `src/storyline.rs` | design | [A] C4 §6 (the count is EvEJS's) | |
| Epic arcs | arc status and journal messages | `epicarcs`: the graph, agents and messages | `agents/epic-arcs.tsv`, `src/epic_arc.rs` | derivable | [V] C4 §7 | #28 |
| LP store offers | `LPStoreMgr.GetAvailableOffersFromCorp`: a list of offers | which offer tables each corporation has (ids only); no offers | offers by rule per table class, with prices from `P_ref`, plus `content/agents/lp-overrides.tsv` | capture-only | [V] C4 §5.2; `docs/re/evejs-data.md` §4.2(c) (also on a public CCP API) | |
| Agents in space | agent rows with no station, a ball, a warp | placement for every one (`agentsinspace`) | nothing | derivable | [V] C2 §9 (the slim field that binds the ball to the agent is unknown) | |

### Sites and dungeons

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| What a site is: archetype, difficulty, faction, scan class, rooms, objects | site records and balls | `dungeons`, `archetypes`, the probe-class table | `dungeons/*.tsv`, `src/dungeon_data.rs`, `src/site.rs` | derivable | [V] C3 §0 items 1-4 | |
| How many sites per system, where, how often they respawn | the signal tracker's full state and deltas | nothing | `content/population.tsv`, `src/population.rs` | design | [V] C3 §0 item 1 | |
| The site protocol and the signature deviation formula | `OnSignalTracker*` pushes | CCP's own signal tracker, shipped in the client | `src/signal_tracker.rs` mirrors it (`:144-147`) | derivable | [V] C2 §2.1, `signal_tracker.rs:144` | |
| Objective chains inside a site | three dungeon-tracking pushes | the chains, run by the client from static data | `src/dungeon_tracking.rs` | derivable | [V] `dungeon_tracking.rs:1-10` | |
| Probe scan strength and accuracy against ships | result strength and position | nothing ("not in the client at all") | [A] numbers in `src/scan.rs` | capture-only | [V] `scan.rs:18-21` | |
| Hacking: board generation, node strengths, utilities | `OnHackingTurnComplete` events | nothing ("the client is only a view") | `src/hacking.rs` (`Rules::SHIPPED`, every number [A]) | design | [V] `hacking.rs:5-9` | |
| Which site escalates to which, and how often | an escalation notice and a site | nothing says which dungeon escalates to which | same faction, one difficulty up, chance [A] (`src/combat_site.rs:72-78`, `src/escalation.rs`) | capture-only | [V] `combat_site.rs:76-78` | |
| Abyssal rooms: count, timer, clear rule | gates and timers | the room count, durations and the targets list | `src/abyss.rs` | derivable | [V] `abyss.rs:44-52` | |
| Abyssal waves per room and tier | NPCs in each room | nothing | not built (rooms are empty, the gate stands) | capture-only | [V] `abyss.rs:52-54`, progress.tsv | #4, #5, #6, #62 |

### Economy and market

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Sales tax, broker fee, order limits | charges that agree with its own preview | the formulas and constants | `src/market_fees.rs` | derivable | [V] C5 §5.1, `market_fees.rs` header | |
| Reference prices (`config.GetAverageMarketPrices`) | a price per type; drives the job cost and reprocessing previews | base prices and recipes; no market | the price oracle over recipes, `econ/prices.tsv`, `src/econ_prices.rs`, `src/service/base.rs:74` | design | [V] C5 §0 item 1; TQ's figures are its players' market | #94 |
| What NPC corporations buy and sell, and where | NPC orders in the book | `npccorporations.corporationTrades` | `npc/trades.tsv`, `src/econ_exchange.rs` | derivable | [V] C5 §0 item 2 | |
| NPC order prices and stock | order rows | base prices | asks and bids on base price; stock [A] | derivable | [I] the price rule was checked against public TQ order data (`econ_exchange.rs:184-193`); stock [A] `:197-200` | |
| Insurance formula and packages | quotes | the constants and the premium formula | `src/service/insurance.rs` | derivable | [V] C5 §5.5 | |
| Insurance price per hull (`GetInsurancePrice`) | one number per hull | nothing | `0.8 × P_ref` | capture-only | [A] `service/insurance.rs:14-16, 62-65`; also on a public CCP API (`docs/re/evejs-data.md` §4.6) | #26 |
| Bounty paid per NPC | wallet journal rows | the bounty on the type | paid in full, times the system multiplier | derivable | [V] C5 §5.3 | |
| Dynamic bounty multiplier: how ratting drains it and how it recovers | `(output, active)` and a push | the call, the push, the 5,400 s cache | `src/econ_dbs.rs`, [A] pool and recovery rate | design | [V] `econ_dbs.rs` header; [A] C5 §5.3 | |
| ESS and DBS settings (`GetDynamicResourceSettings`: cut, pay ratio, intervals, output bounds) | one dict of constants | the keys only | every value [A] (`src/service/live_misc.rs:39`, `src/ess.rs:64-70`) | capture-only | [V] C2 §6; `ess.rs:64-67` names "a reference server's settings payload" as the falsifier | #20, #60 |
| ESS bank behaviour (links, theft, payout) | ESS pushes | the error codes, link range and linkable list | `src/ess.rs` | design | [A] `ess.rs:68-94` | |
| Industry system cost index | facility cost modifiers | nothing | no index (`src/service/world.rs:176`) | design | [V] `src/industry.rs:289-292` | |
| Price history | history rows | nothing | from our own trades | design | [I] C5 §8: history is our own trades | |
| Starting wallet | a journal row | the journal entry type only | `rules.start_isk` (`src/start.rs:21`) | design | [V] `start.rs:65-70` (entry type); amount [A] | |
| In-game store offers and PLEX prices | `storeManager` offers | nothing | `content/store/*.tsv`, `src/nes.rs` | design | [V] `nes.rs` header | |

### Industry

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Blueprints, reactions, invention, PI schematics | job and colony state | `blueprints.static`, schematics | `industry/blueprints.tsv`, `planets/pi-statics.tsv` | derivable | [V] C1 §2.4, §2.8 | #25 |
| Reprocessing and compression | items out | `typematerials`, `compressibletypes` | `types/materials*.tsv`, `types/compress.tsv` | derivable | [V] C1 §2.3 | |
| PI simulation, customs tax | colony state | the client runs the same simulation (`planetCommon`, `ecuPin`) | `src/planet.rs` | derivable | [V] `planet.rs:20-26` | |
| Booster reaction formulas and blueprints: where they come from | NPC orders, drops | no source in the client | owner decision: NPC-sold formulas, dropped blueprint copies | design | [V] `booster_sources.rs` header | |
| Mutaplasmid ranges | rolled modules | the ranges per mutaplasmid (`dynamicitemattributes`) | not built | derivable | [V] `findings/content-C1-map.tsv` (`dynamicitemattributes` row) | #68, #69, #70, #71 |
| Mutaplasmid roll distribution | rolled values | nothing | not built | design | [I] the client holds ranges only, from the C1 map | #68 |

### Factional warfare, sovereignty and world events

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Warzones and their systems | `GetAllWarzones`, occupation states | the warzone system list (`warzonespositiondata`) | `src/facwar.rs`, `content/facwar/warzones.tsv` | derivable | [V] R-FW §0 item 1 | |
| FW HQ systems | `GetHQSystemIDs` | nothing | authored [A] | design | [A] R-FW §7; falsified by the dashboard naming other HQs | |
| Victory points per complex, the flip threshold, flip timing | `VictoryPointState` (`_vpScore`, `_vpThreshold`) and pushes | the state's shape only | [A] (`src/facwar.rs:33`, `:354`) | capture-only | [V] R-FW §7 ("server-only, and no client artifact holds them") | #75, #76, #8 |
| LP for FW kills and complexes | `OnLPChange` | nothing | `src/fw_lp.rs`, ratio [A] | design | [A] progress.tsv ("ratio and split are assumptions") | #27 |
| Interference constants (decay per tick, period, maximum) | `SolarsystemInterferenceState` | the class and its validators | [A] 0.0 / 1.0 / 0.01 / 60 s (`src/service/live_misc.rs:96-106`) | capture-only | [V] C2 §6 | |
| Sov indices and ADM | development indices | the formulas (`devIndexUtil`, `occupancyCalculator`) | `src/sov_index.rs` | derivable | [V] `sov_index.rs:68-91` | |
| Sov campaign scoring (how far one node moves the score) | campaign scores | nothing | [A] in `src/sov_campaign.rs` | design | [V] `sov_campaign.rs:63` ("Not in the client") | #91, #92 |
| Sov hub upgrades, skyhooks, mercenary dens | gateway requests | the request and response shapes (the protobuf in the client) | typed empty answers (`src/gateway.rs`) | design | [V] progress.tsv | #77, #78, #79, #80, #81, #82, #83, #84 |
| World events (tales) | `OnTaleData` with an objective chain | the objective chains | the push seam, no tales authored (`src/tales.rs:19-22`) | design | [V] `tales.rs:19-22` | |
| Incursions, insurgency, live-event schedules | tale pushes, campaign snapshots | the templates and the client side | parked or CCP's "off" answers | design | [V] C2 §7, progress.tsv ("Incursions: parked") | |

### Seasons, achievements, login rewards

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Achievement definitions | gateway messages | `achievements.static`, `achievementevents` | not served (empty) | derivable | [V] `findings/content-C1-map.tsv` rows; C2 §8 | |
| Seasons and challenges: what each is | season data, `Challenge` objects | `challenges`, `challengeconditions`, `challengeevents` | CCP's "off" answers | derivable | [V] C1 map rows; C2 §8 | |
| Which season, challenge or login track runs, and when | season and campaign state | the tracks (`seasonalcampaignsloginrewards`); no schedule | CCP's "off" answers | design | [V] C2 §8 | |
| New-activities news | `activityMgr` | `newactivities`, with date windows | answered (`src/service/live_misc.rs`) | derivable | [V] C2 §8 | |
| Daily, career and corporation goals | gateway messages | career paths and corporation-goal templates; the daily goals' source is unknown | empty gateway answers (`src/gateway.rs`) | see §6 | [V] C2 §1.1 | |

### Corporations and social

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| NPC corporations, their factions, taxes and relations | standings, corp info | `npccorporations`, `factions` | `npc/corporations.tsv`, `npc/factions.tsv` | derivable | [V] C1 §2.6 | |
| Player corporation and alliance costs and rules | wallet charges, refusals | some constants (the corporation founding cost) | `src/service/corp.rs:618`; others [A] (`src/alliance.rs`) | design | [V] `service/corp.rs:618`; [A] `alliance.rs:209` | |
| Static chat channel names | channel ids | localisation message ids | derivable by the localisation reader | derivable | [I] `docs/re/evejs-data.md` §4.10 | |
| Agent and NPC portraits | images from CCP's online image service, not the game server | a few portrait resources | none: a blank portrait must not hang the window | design | [I] `docs/re/evejs-data.md` §4.9 | |
| Player bounty payout share | Bounty Office rows | nothing | [A] share (`src/bounty_pool.rs:51-57`) | design | [A] `bounty_pool.rs:51` | |
| Recruitment adverts, voting | player-made content | nothing | empty answers | design | [V] progress.tsv | #102, #103 |

### Character creation and skills

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Races, bloodlines, schools, start station, starter skills | creation answers | `races` (with skills and ship), `bloodlines`, `schools` | `agents/races.tsv` etc., `src/start.rs` | derivable | [V] `start.rs` header (starter skills from `races[r].skills`) | |
| Rookie ship fit | the minted ship | the hull per race; no fit | `content/econ/starter-fit.tsv` | design | [V] `starter_ship.rs:24-27` | |
| Random name button | a valid name | nothing | our name pool (`src/start.rs:819-835`) | design | [V] `start.rs:819-825` | |
| Skill training, attributes, implants | queue and SP | the formulas and dogma | `src/skill_queue.rs` | derivable | [V] progress.tsv ("Skill queue: client-proved") | |
| Security status loss for crimes | `OnSecurityStatusUpdate` | the bands and outlaw rules; no loss amounts | [A] fractions, `src/sec_status.rs:90-115` | capture-only | [V] `sec_status.rs:51-83` for what is the client's; [A] `:90-115` | |
| Security status gain for NPC kills | `OnSecurityStatusUpdate` | per-type gain and cap attributes | applied from the type | derivable | [V] C6 §2.5 (the attributes); [I] the formula's form | |
| Jump fatigue | timers | the formulas (`jumpFatigue/const.py`) | `src/jump_fatigue.rs` | derivable | [V] `jump_fatigue.rs:12-30`; [A] that the server applies the tooltip's formula (`:36-40`) | |

### Anything else

| item | what the client expects | what is in the client | what we do today | kind | mark | issue |
|---|---|---|---|---|---|---|
| Wire shapes of populated answers | the shape its consumer reads | the consumer code (what it subscripts, unpacks or iterates) | answered as read (`docs/reference/client-call-surface.md`) | derivable | [V] client-call-surface.md §3 | |
| Gateway message schemas | protobuf | the client's own `.proto` descriptors | `src/gateway.rs` | derivable | [V] `gateway.rs:12-18` | |
| Downtime hour | the reset countdown | the client's countdown | 11:00 UTC by ruling | design | [V] `docs/content/rulings.md` 7 | |
| Warfare links (command bursts) | buff values | `dbuffcollections` | `dbuffs/*.tsv`, `src/dbuff_data.rs` | derivable | [V] `dbuff_data.rs` header | |

## 4. Capture-only, in full

| # | item | what exactly is unknown | what a capture must contain | why the client cannot tell us | how much it matters |
|---|---|---|---|---|---|
| 1 | Squads behind dungeon and mission spawn points | which ships the server puts behind each `npcGroupingID` | the `AddBalls` of each pocket on first arrival, for many dungeons, ideally twice each to separate fixed squads from random ones | the id points outside the corpus (`docs/re/dungeon-model.md` §5.5) | **medium**: sites play with budget squads of the right faction and size; veterans would notice "the wrong ships". The most useful capture item, and it needs one capture per dungeon of interest. |
| 2 | Dungeon spawn triggers | which kill, timer or proximity releases which wave | a full run per dungeon: kill order and wave arrivals with timing | only two booleans ship (`dungeon-model.md` §5.3) | low to medium: wave pacing in missions and DED sites |
| 3 | Live-event and warfare spawn tables | the tables and fleet templates behind the reactive spawners | event or insurgency spawns as they happen | ids point outside the corpus (C6 §2.3) | **none now**: that content is not being built |
| 4 | Escalation pairing and chance | which site escalates to which, and how often | many completions of each site and the escalation notices | nothing links dungeons (`combat_site.rs:76-78`) | low: our ladder (same faction, one step harder) plays the same |
| 5 | Abyssal waves | the NPC sets per room, tier and weather | runs at each tier | nothing ships (`abyss.rs:52-54`) | medium once abyss depth (#4, #5, #6, #62) is built; until then rooms are empty |
| 6 | Mission rewards | TQ's ISK, bonus and LP per mission | the offer windows of many missions per level | Basic missions carry none (C4 §4) | low: ruling 9 chose our own numbers, scaled to bounties |
| 7 | Mission standing gains and decline penalty | gain per level for agent and corporation | standings before and after a completion and a decline, per level | the client applies what it is sent | low: our own per-level gains are in use (ruling 9) |
| 8 | LP store offers | TQ's offer list and prices per offer table | `GetAvailableOffersFromCorp` for each corporation of interest | the client holds table ids only (C4 §5.2) | low to medium: players know faction-item LP prices. A public CCP API serves the same list (`evejs-data.md` §4.2(c)) |
| 9 | Insurance price per hull | `GetInsurancePrice` per type | one call per hull | the formula is the client's, the base value is not | low: tied to our price oracle on purpose (C5 §5.5). Also on a public CCP API |
| 10 | ESS and DBS settings | cut, pay ratio, autopay interval, buffer time, output bounds | **one** `GetDynamicResourceSettings` reply | the client shows whatever it is sent (`ess.rs:64-67`) | low, and the cheapest item: one reply settles all of it |
| 11 | Interference constants | quiescent and maximum level, decay per tick, tick period | **one** `GetLocalInterferenceState` reply in low or null sec | the state is sent, not computed (C2 §6) | low: a tooltip and the map filter |
| 12 | FW victory points | VP per complex type, the flip threshold, timing | `VictoryPointState` before and after completing complexes; a flip | R-FW §7: "server-only" | **medium**: it sets the pace of the one built-in sides-fight-over-systems loop (#75, #76, #8) |
| 13 | Security loss for crimes | loss per kind of act, by security | `OnSecurityStatusUpdate` after each kind of act | the client renders the value | low to medium: outlaw pacing in PvP. It needs crimes committed on TQ |
| 14 | Probe scan strength against ships | signal strength and position error by probe, skills and signature | result pushes from scans at known settings | "not in the client at all" (`scan.rs:18-21`) | **medium**: hunting is core PvP. Sites use the client's own formula already |
| 15 | Wormhole statics per system | which static each wormhole system holds | the hole's slim item in each system | shard data (R-WH §0) | low: our class rule gives the same play; only someone navigating by TQ knowledge notices |
| 16 | Moon composition | ore fractions per moon | survey results per moon | shard data (`moon.rs:23-28`) | low: our moons are seeded and stable |
| 17 | PI resource maps | the coefficient blob per planet and resource | `GetResourceData` per planet | the client only evaluates it (`planet.rs:3221-3237`) | low to medium: PI runs, but extraction quotes rest on one assumed value. Generating our own maps also needs the blob format, which only the native evaluator documents (§6) |

**Two notes.**
- **One capture is a sample.** Items 1, 2, 4, 5 and 14 need many runs to tell
  a fixed table from a random draw. Items 10 and 11 need one reply each.
- **A capture is not a fix for "design" rows.** Loot, belt rats, site counts,
  hacking boards and behaviour trees are server tables too, but a capture
  shows only what happened once. Matching TQ's drop rates would take thousands
  of samples, and the client plays the same with any reasonable rate.

## 5. Not server-only after all

| people might assume | what settles it | mark |
|---|---|---|
| NPC aggression, threat range, respawn timing and warp-in range are server AI data | per type in `spacecomponentsbytype`, and the spawn record on each dungeon object | [V] `docs/re/evejs-data.md` §4.1 ("the correction"), `npc-behaviors.md` §6.1 |
| NPC behaviour has to be captured | the engine and role vocabulary ship; chase, attack delay, target switching, ewar and repair numbers are dogma on the type | [V] C6 §0 item 4, §2.5 |
| The agent-to-mission pool is lost TQ data | it can be derived from the client's mission, dungeon and agent tables | [V] C4 summary |
| Mission text needs a capture | every briefing resolves from the client's own message ids, with generic lines where a mission has none | [V] `docs/re/mission-model.md` §1.3, C4 §2.6 |
| Site objectives need server logic | the chains run in the client from static data; the server sends three pushes | [V] `dungeon_tracking.rs:1-10` |
| Dungeon layouts are server-side | about 5,500 dungeons with rooms, objects and spawn points ship | [V] `docs/re/dungeon-model.md` §1 |
| Probe deviation for sites is a server secret | CCP's own signal tracker ships in the client | [V] `signal_tracker.rs:144` |
| The FW warzone system list is server data | `warzonespositiondata` | [V] R-FW §0 item 1 |
| Market taxes and broker fees are server-side | the client's own `marketutil` | [V] C5 §5.1 |
| Insurance is a server table | the fractions and the premium formula are the client's; only the base value is not | [V] C5 §5.5 |
| What NPC stations buy and sell | `corporationTrades` | [V] C5 §0 item 2 |
| Dock and undock points | the station type's locators plus the station's position | [V] schema.md; [I] `evejs-data.md` §4.4 |
| Belt contents | a rock count per ore for every belt | [V] C3 §1 |
| Bounties | a bounty attribute on every NPC type | [V] C5 §2.5 |
| Sov development indices and ADM | `devIndexUtil` and the occupancy calculator | [V] `sov_index.rs:68-91` |
| Gateway messages are opaque | their protobuf descriptors are in the client | [V] `gateway.rs:12-18` |
| Static chat channels | two localisation ids per channel | [I] `evejs-data.md` §4.10 |
| Wormhole physics | dogma attributes per wormhole type | [V] R-WH §1 |

## 6. Gaps in our knowledge

| gap | why it is open | what would settle it |
|---|---|---|
| Daily, career and corporation goals: are the definitions in the client? | gateway-only; C1 maps career paths and corporation-goal templates but nothing for daily goals | read the gateway response protobuf and the career and goals packages for a static-data loader |
| What `npcGroupingID` points at | no table in the corpus fits (`dungeon-model.md` §5.5) | a capture (§4 item 1), or a client table found later |
| What the few `additionalLootId` values and the crate loot ids point at | they close into nothing in the corpus | none in the client; design either way |
| The cycle of wormhole mass regeneration, and what the target-distribution attribute means | no client table is keyed by them (R-WH §8) | a capture, or a client consumer found by grep |
| Whether hidden warzone rows can be contested | R-FW §7 | a capture of the dashboard |
| Which slim field binds an in-space agent's ball to its agent | C2 Unknown | read the agent-in-space client code |
| Whether and where TQ applies the 0.2 bounty security tax | the constant ships, its use does not (C5 §5.3) | a TQ wallet journal of a bounty |
| `CustomContentData` and `Challenge` wire forms | C2 Unknown | their class `__init__` bodies, as was done for the other classes |
| The PI resource blob format | only native code evaluates it (`planet.rs:3221-3237`) | reverse-engineer the native evaluator; needed even for maps of our own |
| The real TQ HQ systems | R-FW §7 | the FW dashboard on any client with a live answer |
