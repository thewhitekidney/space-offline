# The deep queue (from 2026-10-27)

> **Demoted 2026-11-01** — the queue from 2026-10-27, statuses last refreshed that day; nearly all of it has since landed. Current truth: `docs/plans/master-plan.md` and the issue board. Kept for its "Not done" lists, which name every lane left-over; check a line against the code before you brief it.

The owner's instruction (2026-10-27, 01:40): plan deep, keep iterating, and
"don't skimp on any single subject or task, we can keep chewing forever".
This file is the coordinator's work queue after tracker §84. It is longer
than one night on purpose. Work it wave by wave; inside a wave, packets run
in parallel on disjoint files.

**Standing rules for the coordinator:**

- Keep about **6 lanes busy** at all times. When a lane lands, start the
  next ready packet the same turn.
- Nothing is skimped. Every packet ships:
  - its server and bot halves;
  - a socket test where the harness allows;
  - a lane note with its [A]s and a human check;
  - its "Not done" list, which becomes new packets at the bottom of this
    file.
- **Models (owner, 2026-10-27):** build and research lanes run on Sonnet; each
  handback gets a very brief Opus review alongside its gate, and its
  findings become fix packets.
- After every wave:
  - a **review lane**, read-only, on that wave's diffs: correctness,
    permission checks on every new service call, persistence round trips
    and lock holds;
  - its findings become a fix lane.
- After every second wave: re-extract the bundle with `--check`, and run a
  compressed soak (`--time-scale`) once the machine has at most 2 lanes.
- The research specs (`out/research/R-*.md`) stay gitignored. A packet that
  meets an unknown gets a research lane first and is never guessed silently.
- Order inside a wave is a suggestion. A packet whose dependencies have
  landed may start early.

Status keys: ✔ landed · ◐ partly landed (the cell says what is left) · ⟳ running · ☐ queued.
Statuses brought up to date by Q-DOCS on 2026-10-27 from the lane notes in `docs/lanes/`; a packet with no lane note stays ☐ even if its code might exist.

---

## Wave C-1: the society (R-SOCIETY stage C)

| id | what | needs | status |
|---|---|---|---|
| C1 | alliances (server) | — | ✔ |
| C2 | corp and alliance contacts, coalitions, `allied()` | — | ✔ |
| C6 | killmails | — | ✔ |
| C7 | fleets: fleet warp, broadcasts, bot director | — | ✔ |
| CLOCK | `--time-scale N` | — | ✔ |
| gaps-b | encryption books at schools, mining hold, Venture | — | ✔ |
| C3 | corp completions: CEO resign, disband, roles | C1 | ✔ |
| C4 | the society mind: found and join corps and alliances, coalitions, blues and reds | C1, C2 | ✔ |
| D1 | sov ledger and map, location panel fix | C1 | ✔ |
| PERF-1 | big fights: hold max ≤ 25 ms at 30 ships on a grid | — | ✔ |
| PI2 | the PI farmer makes P2 | — | ✔ |
| FWLP | LP for FW complexes; the plexer spends LP | — | ✔ |

## Wave C-2: fighting and the law

| id | what | needs | status |
|---|---|---|---|
| C5 | **bots hunt anyone not allied** in low, null and wormhole space (owner §84): prey choice by relation, d-scan verb, tackle-first, fleet engagement through C7, retreat rules, `--society-pvp` | C4, C7 | ✔ |
| CW-1 | crimewatch completed: suspect and criminal timers, weapons and PvP timers, kill rights (earn, sell, activate), limited engagements, safety settings (green, yellow, red) and the client's safety toggle | — | ✔ |
| CONCORD-1 | CONCORD in high-sec: a criminal act brings CONCORD after the response time by security band, ship destroyed, no escape, sec status loss; faction police in empire space | CW-1 | ◐ 1a, 1b and the gate checkpoints landed (`concord-1a`, `concord-1b`, `concord-gate`); 1c faction police not |
| SEC-1 | security status: gains from rats (exists, CP50), losses from criminal acts, tags to repair it, high-sec entry limits by status (-5 and below) | CW-1 | ✔ (`sec-1`, `sec-1b`) |
| C6b | the killmail follow-ups: corp kill list (`corpRegistry.GetRecentKills`), killmail notifications, bounty fields, kill rights on the mail, ledger `kill` line pruning | C3 | ☐ |
| NOTIF-1 | the notification store: EVE mail notifications (killmail, alliance events, sov, war), the client's Notifications window | — | ✔ |
| MAIL-1 | EVE mail between players and bots, mailing lists, corp and alliance mail; bots send recruitment and diplomacy mails (C4 hooks) | NOTIF-1 | ☐ |
| C8 | **wars** (the high-sec answer to the owner's hunting rule): declare war (fee), ally in war, war HQ [A until Upwell], war targets flag red, CONCORD stands aside for war targets, surrender and mutual; bots declare by relation R | CONCORD-1, C4 | ◐ `c8-a` landed; the rest per the C8 line under "Not done" |
| REVIEW-C | review lane over C1..C8 (permissions: who may accept, kick, set contacts, declare) | wave end | ☐ |

## Wave C-3: corps that work like corps

| id | what | needs | status |
|---|---|---|---|
| CORP-W | corp wallet with divisions, tax rate on bounties and missions, corp journal, CEO and director withdrawals; alliance creation fee from the corp wallet (C1 [A] fix) | C3 | ✔ |
| CORP-O | offices: rent at a station, corp hangars with divisions, role-gated access; deliveries hangar | CORP-W | ✔ (`corp-o`) |
| CORP-T | titles, role grants in bulk, member tracking columns, corp bulletins, the recruitment ad window | C3 | ✔ |
| CORP-S | corp standings from members' NPC standings (the client's formula) | — | ☐ |
| BOT-CORP | bots use all of it: CEO bots set tax, rent an office, pay members by career, hold a corp hangar for builders | CORP-W, CORP-O, C4 | ✔ (`bot-corp`) |
| GC-SOAK | **gate GC compressed**: 48 game-hours at `--time-scale 8` with `--society --society-pvp`. PASS (R-SOCIETY): ≥ 3 corps, ≥ 2 alliances, a blue and a red pair, losses move R, nobody flaps more than once a game-day | C5, C8 | ☐ |
| GC-FIX | fix lane for what GC finds | GC-SOAK | ☐ |

## Wave D-1: sov (R-SOV)

| id | what | needs | status |
|---|---|---|---|
| D2 | the Sov Hub: launch from cargo (`ship.Drop`), the ball and slim, stand-in fuel (`/sovfuel`, fuel blocks in the hub, daily burn): **the consumer that closes the gas and ice gap** | D1 | ✔ |
| D4 | ADM and indices (military, industrial, strategic), the capital system, the dashboard reads | D1 | ✔ |
| D3a | entosis engine (effect 6063), capture of an unclaimed hub (595 s), reinforcement, vulnerability windows (18 h / ADM, EvEJS model, owner may change) | D2, D4 | ✔ |
| D3b | command nodes, the node fight, campaign scoring, the result | D3a | ✔ (`sov-d3b`) |
| D5 | sov bots: the campaign planner, prime-time scheduling, entosis and defend roles, fuel logistics (haul fuel blocks to the hub) | D3b, C5 | ☐ |
| D-HUB | hub delivery: a freighter rung on the builder ladder, or an [A] NPC hub seller at a few null stations, so bots can get a 60,000 m³ hub | D2 | ☐ |
| D6 | Ansiblex jump bridges (owner G1: a scram blocks bridges, not gates) | D2 | ☐ |
| D-MAIL | sov notifications and mails | NOTIF-1, D3a | ☐ |
| GD-SOAK | **gate GD compressed**: 7 game-days at `--time-scale 24`: ≥ 2 claims, a contested campaign with a node fight, one change of owner, G-FIGHT at `FIGHT_SIZE` | D5 | ☐ |
| GD-FIX | fix lane | GD-SOAK | ☐ |

## Wave P: performance and scale

| id | what | needs | status |
|---|---|---|---|
| PERF-2 | whatever PERF-1 could not reach, plus the push fan-out for 50+ ships, missiles in flight at scale, and drones at scale | PERF-1 | ☐ |
| PERF-SAVE | save time with killmails, contacts, alliances and sov in the world: measure, then incremental saves if the off-lock saver passes 1 s | — | ✔ (`perf-save`: 39 ms at 50 characters, no incremental saves needed) |
| PERF-MEM | bot process memory per bot (bots-rss.tsv), shared tables between bots, target ≤ 15 MB per bot | — | ☐ |
| E1 | lazy settlement of logged-off careers (R-ECON E12) | — | ✔ (`e1-settle`) |
| E2 | sov upkeep and ADM decay while nobody is on | D4 | ☐ |
| E3 | release soaks at 100 and 200 bots with `--society`, homes over 10+ systems | PERF-2, E1 | ☐ |
| TICK-1 | per-system tick budgets: systems with nobody in them tick less | — | ☐ |

## Wave X: economy depth (every resource feeds manufacturing)

| id | what | needs | status |
|---|---|---|---|
| X-SALV | salvaging: salvager module, wreck salvage tables, rigs manufacturing from salvage; the ratter salvages | — | ✔ |
| X-RIG | rigs fitted by bots (calibration), rig BPCs from relic sites (HK3 loot) used | X-SALV | ☐ |
| X-DECRYPT | decryptors in invention (HK3 loot); faction BPCs by the site's pirate faction | — | ◐ decryptors in invention landed in `t3-a`; faction BPCs not |
| X-T3 | ancient relics (Sleeper and Talocan) → T3 reverse engineering → subsystems and T3 cruisers [research first: R-T3] | R-T3 | ◐ `t3-a` (relics, invention, extraction, datacores); T3-D/E/F/H not started |
| X-PI3 | PI P3 and P4, POCO customs offices (tax, NPC-owned), PI for builders' components | PI2 | ☐ |
| X-MOON | moon ore and reactions (deferred by S-5; research R-MOON first, staged without Upwell: an [A] NPC refinery path) | R-MOON | ◐ MN-A, B, C, F, G landed (`mn-a`, `mn-b`, `mn-fg`); MN-D, E, H not |
| X-CAP | capital ships: capital components, the build ladder top rungs, jump drives and cyno (research R-CAP) | R-CAP | ◐ CAP-B, C, D, E, F, G, H, K and CAP-LOOT landed; bots through CAP-L, cap-a and the R-CAP rest not |
| X-FREIGHT | freighters and jump freighters for haulers and D-HUB | — | ☐ |
| X-TRADE | fix the NPC trade-goods loop that loses money at Accounting 0: bots train Accounting and Broker Relations, traders price in tax | — | ✔ |
| X-CONTRACT | item-exchange and auction contracts for bots (sell BPCs, ships, loot) | — | ☐ |
| X-INSURE | bots insure ships (CP44 exists); losses are paid | — | ✔ (`x-insure`) |
| X-LPSTORE | mission runners and FW pilots spend LP; LP offers feed the market | FWLP | ☐ |
| X-SKILL | a skill plan per career: bots train what their career needs, buy books, use the queue fully | — | ✔ (`x-skill`) |
| X-ICE | ice tier by band (B1 follow-up); one Badger hull constant for ice and PI | — | ☐ |
| X-ECONREPORT | a `/econ` page per region and a market-health line in `[econ]`: sources and sinks by class (explore, kill, mine, build) | — | ☐ |

## Wave K: combat depth

| id | what | needs | status |
|---|---|---|---|
| K-MSKILL | missile skills and implants (damage, flight time, explosion radius and velocity) | — | ✔ |
| K-FX | explosion FX for ships, cans and wrecks; missile impact FX | — | ☐ |
| K-DRONE | combat drones for bots (ratters and hunters): launch, engage, recall, drone damage application | — | ✔ |
| K-EWAR | ECM, damps, painters, webs and scrams by bots; the client effects | — | ✔ (`k-ewar`) |
| K-LOGI | logistics: remote reps and cap transfers; logi bots in fleets | C7 | ✔ (`k-logi`) |
| K-RATAI | NPC AI: rats switch targets, warp out, scram and web by rat class; Sleepers in wormholes | — | ☐ |
| K-OVERHEAT | overheating: heat damage and the rack bar | — | ☐ |
| K-BOOST | command bursts (fleet boosts) | C7 | ☐ |
| K-POD | pods: capsule kill, clone jump to the medical clone, implant loss | C6 | ✔ (`k-pod`, `implants`) |

## Wave Y: exploration and space

| id | what | needs | status |
|---|---|---|---|
| Y-ROAM | the hacker and explorer roam to other systems for sites (HK4 follow-up) | — | ✔ |
| Y-COMBAT | combat signatures and DED escalations for bots | — | ☐ |
| Y-WH | wormhole careers: a WH resident corp, sleeper sites, gas in WH, hole rolling, mass limits | — | ☐ |
| Y-HACK | hacking balance (tier 4 at 53 % against ~60 %), `CORE_CONTENTS` preview, loot spew | — | ☐ |
| Y-ESCAL | expeditions and escalations as notifications | NOTIF-1 | ☐ |
| Y-MAP | map statistics: kills, jumps and NPC kills per system, which the client's map shows | C6 | ☐ |

## Wave F: friends and the client

| id | what | needs | status |
|---|---|---|---|
| F-PATH | friends path: the review plan's IP22-a/b and IP42 state checked; an end-to-end dry run of the friend kit in a container (no client) | — | ☐ |
| F-BUILD | newest client build (O17): a build-delta study after 3628513, the marshal and service differences, a compatibility plan | — | ☐ |
| F-CI | client-ci scenarios for the human checks a script can drive (`docs/archive/plan-client-ci.md`): turn #60-#63 and the C and D checks into scenarios where possible | — | ✔ |
| F-CENSUS | census follow-ups once the owner has run #59 (unjailed play only after V2) | owner #59 | ☐ |
| F-HIST | history rewrite tooling for O2: a dry-run script that purges tables.rs from history in a scratch clone and shows the result; **never run on the real repo without the owner** | — | ✔ |

## Wave Q: quality, always on

| id | what | when | status |
|---|---|---|---|
| Q-REVIEW | a review lane after each wave (above) | each wave | ☐ |
| Q-SEC | a security review of every new service call: role checks, owner checks, id spoofing, oversized inputs | each wave | ✔ once (`sec-fix`); each wave |
| Q-FUZZ | fuzz the marshal decoder and the new service argument parsers with seeded random inputs (std only) | once, then per wave | ✔ once (`q-fuzz`); per wave |
| Q-FLAKE | run the workspace tests 5 times; fix every flake | weekly | ✔ (`q-flake`, `q-flake2`); weekly |
| Q-DOCS | fold lane notes into the subsystem docs (`docs/reference/server.md`, `docs/reference/world-state.md`, `wallet-ledger.md`), and refresh `docs/guides/human-checks.md` cards | each wave | ✔ this wave (`q-docs`); each wave |
| Q-FORMAT | world-format migration policy: the delta kinds hash changed 6 times tonight; decide when STORE_FORMAT bumps and test an old world loading | once | ✔ (`q-format`, `q-backup`) |

## Research lanes (read-only, out/research)

| id | question |
|---|---|
| R-T3 | T3 reverse engineering and subsystems in this build |
| R-MOON | moon mining and reactions without Upwell structures |
| R-CAP | capitals, jump drives, cynos, jump fatigue |
| R-WAR | the war calls and pushes this client uses (warRegistry, war HQ) |
| R-CONCORD | CONCORD and crimewatch timers and the client's crimewatch UI |
| R-NOTIF | the notification types and their shapes |

## New packets from "Not done" lists

Append here as lanes report. Each line names its source lane.

- C1: a ship in space keeps its old allianceID until it warps in again (also for corps); GetDaysInAlliance, GetAllianceMembersOlderThan.
- C2: group contact labels; bots set group contacts (C4); a socket test of the push to a second member.
- C6: corp kill list; notifications; bounty and LP fields; ledger pruning (all in C6b).
- C7: another bot as a primary (C5); SendBroadcast; invite reject notice.
- gaps-b: drag into the mining hold; Mining Frigate skill in the kit; schema.md reader note.
- CLOCK: a socket test of the flag through the binary; runtime reload re-sets the clock.
- B10: GM setup gives drones; orphaned drones swept on reconnect.
- HK3: an `explore` class in `[econ]`; datacore reference prices.
- PI2: upgrade an existing P1 colony to P2 in place; `restarts` can give a P2 colony's second extractor the wrong resource; the `pi_farmer` signal reads only P1 prices; goods left in the office after a refused import.
- FWLP: the LP spend has no socket test (no militia LP store fixture); LP for FW kills; frontline/command/rearguard multipliers; the stale "No LP" in the facwar.rs module doc.
- D1: hub id placeholder until D2; `sov` in GM autocompletion; map.GetCurrentSovData (no caller).
- C3: titles (GetTitles, UpdateTitle(s), DeleteTitle) in CORP-T; KickOutMembers; GetMemberIDsByQuery; editing corp details; ExecuteActions GIVE and SET baseID.
- PERF-1: `Roster::by_ship` and `Registry::character` linear scans; `grid::reconcile_in` O(viewers × local balls); soak.sh should judge the `[world] tick health` hold; tick 0 warm-up of all hot systems at once (29-32 ms).
- C4: the alliance fee (1 B ISK from the CEO's wallet) blocks bot alliances until CORP-W (corp wallets) or a fee decision; bots don't use the director role; `/society` GM view; a hall misses a human-made CEO change until restart.
- r-law: CONCORD-1a (L), 1b (M), 1c (M) after cw-1; C8-a (L), b, c (M), d, e (S), f (M) wars per out/research/R-WAR.md (500 M/week, 24 h spool-up, war HQ stand-in as a virtual type-27 item [A], `allowWar` permit, `--society-wars`). Decision taken: war wall times follow --time-scale (soak-only; scale 1 for play).
- NOTIF-1: corp joined/left militia (58/59) when a request settles; insurance expiry; player bounties and kill rights; server-side delete pushes; bots read their notifications.
- D4: untuned points per NPC and per m³ and the decay rate (GD-SOAK settles); catch-up rollover decays one day only (E2 does offline decay).
- Q-FORMAT: BACKUPS_KEPT=5 lets a night of kind-adding upgrades prune the last pre-upgrade backup: always keep the oldest backup of the current STORE_FORMAT besides the 5 newest (fix packet Q-BACKUP).
- c4-fix: the_kill_set_is_bounded repeats the trim loop instead of calling the production code (make it call it); Stances pruning by dead target (corpless pilots).
- **SEC-FIX (priority, after merge-e)** from Q-SEC (Opus, 2026-10-27 ~02:45), most severe first:
  1. alliance.rs:671 + group_contacts.rs:440: executor check uses the bound alliance id; a non-executor director edits the alliance contact book via a made-up id. Check the caller's own alliance and refuse unknown ids.
  2. alliance.rs:630/698: executor support counts at once; one vote beats 0 and ties keep the holder, so a joining corp takes the executor seat. Founders' corps vote for themselves by default, and a change needs a majority held for the client's wait (verify).
  3. corp_wallet.rs:661/:749: GiveCash reason uncapped (MB strings into both journals). Cap at the client's limit.
  4. alliance description/url (Create and Update), corp description/url, member titles: no length caps. Cap at the client's limits.
  5. fleet_broadcast.rs:166-181: raw client args relayed to every member. Validate the types and sizes the client sends, and rate-limit.
  6. killmail.rs:107: GetKillMail ignores the hash, so ids are guessable. Check the client's SHA-1 hash (NOTIF-1 computes it).
  7. alliance.rs:428: alliance names allow control characters and markup. Use cw::check_name rules.
  8. group_contacts.rs:550: any id above 1, unbounded book. Verify the id exists and cap the book size.
  9. corp.rs close_corp: wallet divisions untouched on close, so members' ISK vanishes. Pay out to the CEO, or refuse a close with a balance (the client's rule).
  10. killmail.rs:476: `involving` parses 2,000 rows under the lock per call. Index by character.
- merge-d: hunters treat high-sec war targets as prey (needs a GetWars poll in the driver plus enemy-owner plumbing).

### Landed outside the tables above (2026-10-27, Q-DOCS)

These lanes have notes but no row: `ship-pool` (combat bots fly what exists),
`co-found` and `q-flake2` (the co-founding pool and its flake), `bot-tune`
and `gb-fix` (what failed gate GB: builders, gas, corps), `ft-a` and `ft-b`
(fighters: data, bay, tubes, launch and recall), `map` (MAP-A to MAP-D: centres
file, light-year distance, d-scan clamp), `cap-loot` (sources for the capital
commodities), `sec-fix` and `sec-1b` (SEC-FIX, tag drops), `q-fuzz`, `perf-save`.

### New packets from the 2026-10-27 lanes' "Not done" lists (Q-DOCS)

Each line names its source lane; the lane note holds the detail.

- **mn-a:** structure bonuses and remote reactions beyond the distance check (MN-H).
- **t3-a:** T3-D/E/F/H not started; subsystem items are stackable in `typedata::Installed::build`, T3-D must decide; optionally add `t3_datacore` to `hacking_loot::TIER_QUANTITY_CLASSES`; bundle regeneration (`content.py --sections types,explore`) in main.
- **q-fuzz:** the handshake with `--accounts` is not driven (needs a live account book); allocation is asserted bounded, not measured.
- **perf-save:** incremental saves and the `Arc`-shared rows only if the soak shows a hold over 1 s on a quiet machine.
- **concord-1a:** CONCORD-1c (faction police, the outlaw dock refusal); the neutraliser effect 6691; the warp-in animation (CONCORD-FX); the insurance log line 343.
- **concord-1b:** board, cloak and beacon refusals are unwired (`refuse_beacon_jump` is wired in CAP-E; board and cloak need in-space boarding and a cloak); destroying CONCORD (`KillConcord`) is not charged.
- **concord-gate:** a socket test of the 15 s respawn; checkpoint ships do not move (idle anchor orbit); the commander-at-checkpoint choice (A1) awaits the owner; station police screens; bots' gate-fly tests see extra balls.
- **ship-pool:** fitting by weapon type (missile, hybrid, laser); drones beyond Hobgoblin I; hull skills for a bigger class; mixed weapon types.
- **map:** register `CENTERS_TABLE` consumers (CAP-A reads `Centers`); a `Safest` weighted route; a real-data connectivity test over the 5,268 gated systems.
- **cap-loot:** the fifth can of a low-sec ghost site also giving 57450; the 11 types' per-site "first hacked guarantees".
- **x-skill:** a reactor plan; trips for books; plan rows tied to the career's real bottleneck (gun skill by fitted turret type).
- **sec-fix:** the personal contact book (`contacts.rs`) has no cap and no existence check; the corp description edit arm must use `CORP_DESCRIPTION_MAX_LEN` when it lands.
- **sec-1b:** a socket test for the tag drop; route a hunter to an office station (SEC-1d, `travel` over `office_open`); a standing bid for tags.
- **ft-a / ft-b:** `/fit me` fighters; a bot carrier role (see ft-gh); the unwired refusals (`CannotOpenFighterTubesWhileCloaked`, `CannotRecallFighterWarpScrambled`, and four more).
- **ft-gh:** a bot carrier-pilot career (CAP-L): a bot with a built carrier, bay stocked with fighters that builders made (nothing from nowhere), launching, attacking with the squadron's ability, recalling, and training the fighter skills; the fleet hangar as a scoop destination; theft and aggression for scooping another pilot's squadron; a ball for a bomb in flight and the kamikaze's missile mode; skills fixed at launch, a mid-flight retrain applying to the next launch.
- **gb-fix:** ratters buy only EMP S, which a school-system builder cannot make; a builder past the starter rung needs other systems' ores (stage D: sov and moons) or a mineral trade between systems; corps from earnings.
- **bot-tune:** a socket test of `pick_round`; the other careers' priors; a miner that mines other systems' ores.
- **cap-f:** no login push by design; a socket test needs a jump over the wire (now possible with CAP-E).
- **k-ewar:** tracking and guidance disruptors (6424, 6423); ECM bursts and ECCM; bots buying tackle modules; NPC scram beyond `tackle::status` (K-RATAI); a painter does not shorten lock time; a jammed bot's mind does not react.
- **cap-k:** teach bots (CAP-L) to avoid the refusal; Upwell structures (`can_dock` unwired); persist the switch if wanted.
- **co-found:** a founder that stays offline holds its pledges; no re-election while pledges stand.
- **q-flake2:** a bot registering after a pledge stands never becomes founder of that pool (by design).
- **q-flake:** the cause was not proven; if the flake returns, capture the bot's `dump()` (`failed(Lock) >= GIVE_UP_AFTER`, sticky `Homing::Recovery`); a bot that missed a primary call is never told again.
- **e1-settle:** offline rules for `mission_runner`, `explorer`, `pirate`, `soldier`, `sov_grunt`, `wormhole_diver`; a builder's invention and ladder offline; persist the `Book` totals; E3 (100 and 200 bot soaks) is the gate.
- **sov-d3b:** a mail for a successful defence; loot from a fallen hub (`sovHubLossDrops`); nodes following ADM [A]; a node in a system with no pilot spawns silently.
- **cap-b:** the maintenance bay and other specialised holds have capacity but no drop wiring (CAP-H did the bay); fuel was unused until CAP-E.
- **f-ci:** a bot hook in the harness (`client-ci.sh` starts no bots); a second-client role split; the hacking WIN solver; moving a character to null or low sec; a kill-then-report scenario.
- **k-pod:** per-clone implants were done by `implants`; the socket test for a pod death is still missing.
- **x-insure:** a server tick hook for expiry on the minute; corp-owned ships and refund on `UnInsureShip`; bots do not renew mid-flight.
- **cap-d:** cyno jammers and inhibitors, the structure checks, the module's 30 s reactivation delay on the server, deployable and structure beacons, `CantBeaconJumpGlobalCriminalFlag` coverage.
- **mn-b:** MN-D survey probe and moon scan; MN-E refine and compress verification; MN-H Upwell; bots mining where the scan says (MN-G follow-up); no belt for a cold system (`econ_agents`); a belt is not on the Agency or scanner list.
- **corp-o:** the office-rental station service flag and the standing check; offices in structures; the member base picker (`corp.rs` `baseID`); `deliver_to_corp` has no producer; `DeliverToCorpMember` and `DeliverToSelf`; the rent bill message and notification; HQ and base roles; BOT-CORP stocking its hangar.
- **implants:** the charBrain in `GetAllInfo` so the client's fitting window shows bonuses; boosters (`GetBoosters`, `UseBooster`); remap (`RespecCharacter`); a learner career that buys and plugs implants; a recorded real-client capture of plug and unplug.
- **k-logi:** the remote-rep combat-log lines; bots transferring capacitor; NPC remote repairs; ancillary and mutadaptive repairers; a fleet roster for wards; a dedicated logi hull (a `KIT` change); K-OVERHEAT and K-BOOST.
- **cap-h:** boarding from the maintenance bay in space (the ballpark ship switch); scoop to fleet hangar or bay (no jettison or scoop exists); packaged ships in the bay; the retail class gate; pruning stale `shipcfg` rows; corp-hangar and structure fleet hangars.
- **cap-e:** the capacitor (`jumpDriveCapacitorNeed`, unit unknown); `effects.JumpDriveIn` on arrival; `CantJumpWhileModuleActive` for siege, triage, bastion; cyno jammers; a CONCORD-state unit test for `CantBeaconJumpGlobalCriminalFlag`.
- **mn-fg:** a Pyerite price watch in the first soak (moon ore floods the mineral market); a goo hauler; cold-system moon mining; a per-bot wallet gate; ranking by `P_mkt` history; polymer prices; a biochemical reactor; reaction ME and structure bonuses (MN-H).
- **bot-corp:** deliver at the builder's cycle completion (a hook in `builder.rs`); revoke roles and demote directors; pay members that left; office rent and the alliance fee compete with pay; a second page of members.
- **cap-c:** a reactor career and a P3/P4 farmer (the missing rungs); part-by-part hull buying; a T2 hull path; a socket test.
- **cap-g:** the generator's hull capacitor (`jumpPortalCapacitorNeed`); deployable and structure beacons; jump cloak; Ansiblex bridges (D6).
- **Q-DOCS (this lane):** `docs/content/schema.md` is current for the files lanes added; nothing else missed. The next Q-DOCS should fold the next wave in the same three docs (`docs/reference/server.md` §8, `docs/reference/world-state.md`, `docs/reference/wallet-ledger.md` §11).
