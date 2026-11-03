# The living world: doctrines, a fluid economy and relations (plan, 2026-10-28)

> **Demoted 2026-11-01** — stages 0-3 landed (lanes `doc-*`, `fe-*`, `r-rel-*`, `difficulty`). What is left, the stage 4 proof, is in `docs/plans/master-plan.md`, which is the current truth. Kept for the reasoning and the stage list.

The owner's direction (2026-10-27): a fluid, demand-driven economy; bot alliance and corp
leaders who fancy a doctrine (armour or shield, long or short range, kiting or brawling), read
what the enemy fields in a big fight and counter it, committing in stages and never everything
at once; a difficulty setting for the player; relations on four levels (personal, corp,
alliance, coalition) that every action moves, the humans' included.

Three research specs back this plan (gitignored, real ids inside): `out/research/R-DOCTRINE.md`,
`R-FLUIDECON.md`, `R-RELATIONS.md`. Each packet below names its spec section; the lane brief is
the spec section plus `briefs/wave-c.md`.

## What the research found (the reasons for the order)

1. **Turrets always hit for full damage** (`combat.rs`: `[A] every shot hits`). Range, tracking
   and signature exist only for missiles and drones. Until turrets roll to hit, long range
   against short range and kiting against brawling mean nothing, for the players too.
2. **Every combat bot flies projectiles and EMP** (`hull_choice::BOT_WEAPON`, `fit_plan`), so no
   armour, shield, rail or missile doctrine can exist yet. PROG-PVE2 (running) adds the other
   weapon systems and a tank.
3. **Prices cannot move far or fast.** Bots quote inside 0.5-2.0x the reference price and the
   trade index is clamped to the same band; a price needs 3 traded days in 7 to count; bots never
   read the live book. Builders choose by ladder rank, not margin, and make no modules.
4. **Relations are one level** (bot corp to group). No personal level, no coalition object; only
   `Kill` and `AllyFight` events are ever emitted; grudges never decay and hunting ignores them; a
   lone human in an NPC corp can never count as allied (`group_contacts::allied`).
5. **What a bot can see is already on the wire**: d-scan, the grid's slims, local, killmails
   (enemy fits), damage messages, fleet broadcasts. The specs read only these. Bots never cheat:
   difficulty never grants ISK, ships or sight the client would not give.
6. **A paper-fit evaluator exists** (`dogma::evaluate`), so the counter matrix is computed from
   mechanics (a Lanchester exchange over HP, resonances, range, tracking, signature, speed,
   ewar and logistics), not a hand table.
7. **Insurance pays at the static reference price**, so a self-destruct can profit.

## One difficulty setting

The three specs each proposed a knob; they become one: `eve-bots --difficulty
{easy|normal|hard|brutal}` (default normal), one module `eve-bot/src/difficulty.rs`, one table
`content/bots/difficulty.tsv` that a mod may override. It scales only bot behaviour: observation
noise and intel sharing, reaction delays, doctrine tenure and switch confirmations, counter
accuracy, commitment step and reserve, coalition coordination and lag, grudge rate, memory and
forgiveness, the prior toward unlisted humans, and the traders' appetite. `normal` reproduces
today's constants. The server, physics, NPCs, CONCORD and prices are identical at every level.

## Stages

Each stage's packets run in parallel; a stage starts when the packets it needs have landed.
Sizes are the specs' (S, M, L). Every packet: unit tests with invented ids, a socket test where
it touches the wire, an `fci_` scenario when the client sees a change.

### Stage 0: foundations (no new behaviour for bots yet)
- **DOC-0 K-TURRET** (server, S-M; R-DOCTRINE §12): turrets roll to hit with the client's
  chance-to-hit (range, falloff, tracking, signature), wrecking hits, hit quality in the combat
  log. Changes every fight's balance, so it gets its own soak: ratters' kills/h within 20% of
  before, or the earnings tables get a second pass.
- **FE-MEASURE + FE-GUARD-A** (server and tools, S-M; R-FLUIDECON §4, §5 G2-G3): price rows,
  a basket index, book depth, stock and destroyed/minted in `/econ` and its TSV; a wash-trade
  filter on the index; insurance capped at the market index.
- **REL-FIX** (server, S; R-RELATIONS §1 D2, §6 R-REL-2): `allied()` honours a personal contact
  on the character; the kill report carries every attacker, not only the final blow.
- **DOC-1 model + matrix** (bot, M-L; R-DOCTRINE §2-3): `Profile`, `Doctrine`, the catalog, the
  greedy fit, the exchange function and the `[doc] matrix` log. Pure.
- **R-REL-1 pure model** (bot, M; R-RELATIONS §2): four levels, spill up and compose down, the
  17 facts, fairness, betrayal, coalition blocs. Pure.

### Stage 1: bots can fly any doctrine and see the field
- **DOC-2 fit and buy** (M; after DOC-1 and PROG-PVE2): any hull, weapon family and tank from
  the role fit; ammo for any gun; bids with units.
- **DOC-3 sense** (M; after DOC-1): the battle picture and enemy memory from what the client shows.
- **FE-VALUE** (M): the value model (spot, history, averagePrice), wider bands with per-step and
  per-day move caps, seller markup on bid pressure, buyer need rules.
- **FE-BUILD + DOC-7** (M, one lane): builders choose by margin; module, ammo, plate and extender
  rungs; doctrine parts climb the ladder; "parts with no source" logged.
- **R-REL-2 observation** (M; after REL-FIX): facts from killmails, contracts, mail, wars, fleets.

### Stage 2: leaders, members and demand
- **DOC-4 leader and theatre** (L; after DOC-1, DOC-3): the doctrine choice (skills, supply,
  cost, counter, taste, inertia), the per-theatre response machine with available strength,
  staged commitment, a reserve, reship and travel delays.
- **DOC-5 members comply** (M): skill plans bend, the doctrine hull outranks taste,
  replace-on-loss.
- **FE-DEMAND + FE-CAREER** (M): one demand channel (bids with size) from doctrines, losses,
  fuel and upgrades; careers choose by realised ISK/h with hysteresis.
- **R-REL-3 effects + R-REL-4 humans** (M-L, two lanes): graded contacts the client shows,
  invitations, repricing and refusal, answering `NeedBackup`; the humans' actions move their
  standing; a `--friends` file of starting standings for the owner's friends.

### Stage 3: corps, coalitions and traders
- **DOC-6 + FE-SRP** (M, one lane): doctrine stock in corp hangars, ship replacement, staging.
- **FE-TRADE** (M): price memory, speculation within caps, regional arbitrage by haulers.
- **R-REL-5 coalition, war, sov** (M-L): coalitions form and break, wars declared by relation.
- **DIFFICULTY** (S-M): the one module above (DOC-8 + R-REL-6 Temper + FE G5).

### Stage 4: prove it
- **Soaks**: R-DOCTRINE gates G1-G10 (no flapping, counters chosen, staged commitment, builders
  follow, difficulty ordering), R-FLUIDECON scenarios (baseline, doctrine switch: the basket
  rises 15% or more within 4 game days and settles by day 20; war: destroyed/minted at least 0.6;
  regional spread narrows; speculation damps overshoot), R-RELATIONS' 11 `[rel]` criteria
  (friends never hunted, betrayal works, memory decays).
- **DOC-9 arena and report**, **FE-CTL** (a sink/source controller, only if Stage 4 shows drift),
  a read-only review lane over permissions and abuse loops.

## Decisions for the owner

1. **DOC-0 changes PvP for the humans too**: turrets will miss as in the real game. Default on,
   behind a feature flag. (Recommended: on.)
2. **`--friends`**: a file of the owner's and friends' characters with a starting standing
   (default Friendly, +3).
3. **Who builds what first**: the plan runs Stage 0's five packets at once.
