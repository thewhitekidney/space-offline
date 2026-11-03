# C5: economy content

Research lane C5 (`docs/content/README.md` §3), 2026-10-25. Read-only: no
cargo, no edits to `server/` or `tools/`. Every count below was measured on
build 3619879 (`tq-offline`) with stdlib Python over `tools/dogma.py`,
`tools/fsd.py` and read-only sqlite. Scratch scripts were deleted before the
report (lane rule 9). Companion file: `findings/content-C5-params.tsv`, one
row per economic constant or knob, with its mark and source.

Marks are as in the shared brief: **[V]** read from this build, **[V2]**
EvEJS, **[I]** inferred (the inference is stated), **[A]** an assumption we
would ship (the falsifier is stated), and **unknown**.

---

## 0. The answer in one screen

1. **One price oracle feeds everything.** `P_ref(type)` is computed by an
   extractor from the corpus. It is a recursion over blueprint and reaction
   recipes and PI schematics, down to `basePrice` at the raw-material
   leaves. It is floored at the type's own reprocessing value, which closes
   arbitrage by construction. It covers **9,862 of 11,700** non-cosmetic
   market types. The server serves it as `config.GetAverageMarketPrices`,
   which is answered `{}` today. That `{}` is a live defect: the client
   computes the industry job cost and the reprocessing ISK cost from this
   dict, the server computes them from `basePrice`, and the two disagree
   (§3).
2. **NPC orders are virtual and owned by the station's corporation.** They
   are computed on read and never stored as rows. Only the amount bought or
   sold since refill is stored, as a C7-style delta. There are two tiers:
   - **Tier A is retail-true.** `npccorporations.corporationTrades` **[V]**
     lists 2,705 types across 184 corporations, and the corps own 5,184 of
     the 5,210 stations. Positive entries become NPC sell orders: BPOs,
     skillbooks and trade goods (455 of 512 skills and 1,724 of 1,861
     BPOs). Negative entries become NPC buy orders for trade goods, gas,
     ice and tokens.
   - **Tier B is a backstop for a small population, and it is an [A].** At
     a short list of hub stations, NPC sell orders for 934 buildable T1
     items sit at a **ceiling** of `1.25 × P_ref`. NPC buy orders for raw
     materials and loot sit at a **floor** of `0.6 × P_ref`. Players and
     bots trade inside that band, and the NPCs guarantee liquidity at its
     edges.
3. **Faucets and sinks use the client's own constants wherever the client
   has them.** Those are:
   - the 8 % sales tax, reduced 11 % per level of Accounting;
   - the 3 % broker fee, reduced 0.3 points per level of Broker Relations;
   - the 0.5 % SCC surcharge and the 100 ISK minimum fee;
   - insurance premium `= full × (fraction − 0.4) × 0.5`;
   - the dynamic bounty system's `(output, active)` call and its push;
   - the bounty attribute 481.

   All of these are **[V]** (§5). The server charges none of the market
   taxes today and sells no insurance.
4. **The economy stays stable with 50 bots because bot ISK is fuel, not
   wealth.** Bots earn and spend real ISK through the real wire. Their
   wallets are capped, and the excess is burned at rollover. Bot demand
   for human-made goods has a daily budget. A per-system dynamic bounty
   multiplier throttles farming. A daily `[econ]` ledger measures faucets
   minus sinks, and one bounded controller nudges two knobs (§7).

---

## 1. What the server does today

| Area | Where | State |
|---|---|---|
| Order book | `service/market.rs` `Book` (`Vec<Order>`, `Vec<Trade>`) | Player orders only: "no NPC sellers and no seeded book" (`market.rs:335-341`, an **[A]** by the market lane). Matching, escrow, expiry, persistence (`mktorder` and `mkttrade` records) and `OnOwnOrdersChanged` all work. The book is a linear scan, and closed orders and trades are never pruned. |
| Fees | `market.rs::fee_rate` | Takes the **client-supplied** broker fee, and only on resting orders. **No sales tax**, no modification fee (`docs/lanes/market.md` §P5.1). |
| Price history | `other_market_call` | `GetOld/NewPriceHistory` and `GetHistoryForManyTypeIDs` answer empty `CRowset`s. |
| Average prices | `service/base.rs:53` | `config.GetAverageMarketPrices` → `{}`. |
| Industry EIV | `industry.rs::adjusted_price` → `industry/tables.rs::ADJUSTED_PRICE` | `basePrice` for 65 types, **[I]** substitution. |
| Reprocess ISK cost | `reprocess.rs::base_price` → `BASE_PRICE` | `basePrice` for 68 types. Every gas, isotope and fullerite is 0, so a gas refine is free. |
| Bounties | `npc.rs::pay_bounty` | Pays attribute 481 in full, ref 17. No multiplier. |
| Dynamic bounty | `service/space.rs:719` | `(1.0, False)`, "the system is not running". |
| Missions | `mission.rs` | Hand-set `reward_isk` (15,000 and 45,000), ref 33. |
| Insurance | `service/inventory.rs:795-829` | Empty contracts, price `0.0`, and `InsureShip` refused. |
| LP | `service/wallet.rs:167` | Balances `[]`. No store (C4 owns offer authoring). |
| PI | `service/planet.rs:92-93` | Import and export tax charged at the customs office's rate. |
| Starting wallet | `model.rs:256` | `balance: 0.0`. ISK comes only from `/giveisk`. |
| Bots | `eve-bot` | Mining and escort roles. **No market verbs.** |

---

## 2. Corpus facts the design stands on

All **[V]**, measured. Schemas are from `findings/pyd-schemas.tsv`. Offsets
are record-relative, minus the delta, as `dogma.Table` wants them.

### 2.1 Types (`types.fsdbinary`)

- 53,288 types, 27,309 published, and **19,720 published with a
  `marketGroupID`**.
- 12,228 of the 19,720 have `basePrice == 0`.
  - 7,700 of those are cosmetic: categories 91 (SKINs, 5,091), 2118, 30
    and 63.
  - The rest are **real gear**: 2,630 modules, 461 implants and 257
    charges.
  - Even T1 modules (`metaGroupID 1`) have 763 zero-priced types.
- **So `basePrice` cannot be the price of a module.** It is fine for
  minerals (2 / 8 / 32 / 128 / 512 / 2,048 / 8,192), skillbooks (median
  3,000,000) and BPOs (median 10,000,000).
- Market types by category, top five: 91 SKIN 5,091; 7 Module 3,876; 9
  Blueprint 1,861; 17 Commodity 1,584; 2118 1,067.
- `metaGroupID` over market types: None 10,793; 4 (faction) 1,982; 1 (T1)
  1,974; 17 1,483; 2 (T2) 1,019; 5 (officer) 615; 6 (deadspace) 362; 3
  (storyline) 224.

### 2.2 Recipes

- `blueprints.static` (fsdlite sqlite, `docs/lanes/industry.md` §2.1) has
  5,100 blueprints. With reactions there are 4,978 product recipes; 4,227
  of those products are on the market. 1,879 blueprints are themselves on
  the market.
- `schematics.fsdbinary` (via the existing `dogma.SCHEMAS`) has 68 PI
  output recipes.
- `typematerials.fsdbinary` has 9,866 rows. 7,791 of them are market
  types. The layout is `docs/lanes/reprocess.md` §2.2: `struct_at 8`,
  `size 32`, presence at `+16`.
- A census of `material cost / basePrice` over priced products (2,116
  types) gives p5 0.011, p25 0.355, **median 1.0**, p75 1.66 and p95 33.
  `basePrice` and material cost agree for half the catalogue and wildly
  disagree for the rest.
- A census of `reprocess value at 100 % / basePrice` over 3,782 types gives
  p50 0.36 and p75 1.0. **936 types are worth more as minerals than their
  `basePrice`, and 376 of them even at a 50 % yield.** An NPC that sold at
  `basePrice` would be an infinite ISK faucet through the refinery (§6).

### 2.3 `npccorporations.fsdbinary`

283 corporations. Columns are read with a hand schema from the pyd row:

- `struct_at 8`, `size 224`, presence word at 212;
- `corporationTrades` at 8 is a dict whose entry stride is 16, with a
  `u32` key at 0 and an `f64` value at 8;
- `lpOfferTables` at 80, `factionID` at 152, `stationID` at 200,
  `taxRate` (`f32`) at 204, `size` and `extent` are `str`s at 104 and 56.

The decoding is validated: every key resolves to a type name, such as
Holoreels, Frozen Food or Antibiotics.

**`corporationTrades`** is `{typeID: value}` with values in [−1, 1]:

- 184 corporations have it, covering 2,705 distinct types;
- 2,394 types have a positive entry somewhere and 348 a negative one;
- positive by category: 9 BPO 1,791; 16 skill 470; 17 commodity 339;
  7 module 46; 8 charge 21;
- negative by category: 17 commodity 326 (trade goods, gas, ice,
  tokens); 66 9; 25 7;
- the commonest values are 0.083 (7,794 entries), 0.333, −0.091, 1.0,
  −0.1, 0.25 and −1.0;
- example: Caldari Navy (1000035), owner of Jita 4-4, sells 361 types (350
  of them BPOs) and buys 56.

The **sign** reads as supply versus demand. That is **[I]**, from what each
side contains: the positive side is BPOs and skillbooks, which NPCs sell in
retail, and the negative side is trade goods and tokens, which they buy.
The **magnitude's** meaning is **unknown**.

Other columns:

- `taxRate` is 0.0 for 254 corporations, 0.11 for 27 and 0.10 for 2. **[I]**:
  this is the corp member income tax, not a market tax.
- `lpOfferTables` is set for 181 corporations, with 76 distinct table ids.
  The offers themselves are not in the client (C4).
- `exchangeRates` is set for one corporation.

### 2.4 Stations (`bin64/staticdata/mapObjects.db`, `npcStations`)

- 5,210 stations in 1,754 systems, owned by 185 corporations, with 62
  operation types. 3,340 are in high-sec, 1,338 in low-sec and 532 in
  null-sec. 40 regions have stations.
- `stationservices` has 27 services, with Market = 7, Insurance = 21, LP
  Store = 25, Black Market = 8 and Stock Exchange = 9.
- Joined through `stationoperations.services`:
  - **every station has Market (5,210)**;
  - 4,998 have Insurance;
  - 5,209 have an LP Store;
  - 2,259 have a Factory.
- `stationoperations` also carries `border`, `corridor`, `fringe`, `hub`
  and `ratio` floats per operation. For example, Assembly Plant (op 14) is
  `0.3 / 0.1 / 0.0 / 0.6 / 0.55`. Their meaning is **unknown**. **[I]**:
  they are the old placement weights.
- **5,184 of the 5,210 stations are owned by a corporation with trades.**
  The median region therefore sees 1,460 NPC-traded types, and the maximum
  is 2,202.
- **The sandbox home station 60012526** is owned by 1000126, which trades
  only 5 types. Its neighbour 60014437 is owned by 1000163, which trades
  7. **With Tier A alone, a new pilot at home sees an almost empty
  market.** That is why Tier B exists.
- The trade hubs are **[V]** stations:

  | Station | System | Owner |
  |---|---|---|
  | 60003760 | 30000142 | 1000035 |
  | 60008494 | 30002187 | 1000086 |
  | 60011866 | 30002659 | 1000120 |
  | 60004588 | 30002510 | 1000049 |
  | 60005686 | 30002053 | 1000057 |

  All five have reprocessing efficiency 0.5.

### 2.5 Bounties (`typedogma` attribute 481 `entityKillBounty`)

- 2,873 types carry one, all in category 11.
- Percentiles: p10 9,750; p25 30,000; p50 100,000; p75 500,000; p90
  2,000,000; p99 33,875,000. The maximum is 250,000,000.
- The server already pays it (`npc.rs:1330`).

---

## 3. The price oracle

### 3.1 Why it is the first thing to build

**[V]** The client's `cfg._averageMarketPrice` is filled by
`config.GetAverageMarketPrices()`. The call is made at
`eveCfg.py:528` (synchronous variant) and at `eveCfg.py:680`
(`GetAveragePricesThread`, 2 s after login), and is read through
`CfgAverageMarketPrice` (`eveCfg.py:1325`).

`inventorycommon/typeHelpers.py` reads it:

- `GetAdjustedAveragePrice` (`:106-110`) returns
  `_averageMarketPrice()[typeID].adjustedPrice`, and `None` on a
  `KeyError`;
- `GetAveragePrice` (`:113-118`) returns `….averagePrice`, except type
  79785, which is hard-coded to 1000.

Readers found by string search:

- `industryCommon` (EIV, so the job-cost preview);
- `reprocessing/util` and `reprocessing/ui/*` (the ISK cost and the output
  value);
- `raffles`;
- `behaviors/targetEvaluation`;
- `playerprogression/plexprogressionnotifier`.

So today:

- the industry window previews an EIV of 0, while the server charges an
  EIV computed from `basePrice` (`industry.rs:879`);
- the reprocessing window shows 0 output value.

**The fix is to answer the dict, from the same table the server charges
from.**

**Shape**:

- `{typeID: KeyVal(adjustedPrice=float, averagePrice=float)}`;
- the attribute names are **[V]**, from `typeHelpers`;
- dict-of-`KeyVal` is **[V2]**: EvEJS's live probe saw 9,339 rows
  (`docs/lanes/evejs-probe.md` §5.2);
- `averagePrice` is the trading index (§3.4), and `adjustedPrice` is
  `P_ref` **[A]**. In retail, "adjusted" is a smoothed long-run figure.
  The falsifier is a capture whose two values differ in kind.

### 3.2 The recursion (extractor rule)

`P_ref(t)` is defined first-match, with memoisation and a depth cap of 12.
The cap is never hit in the census.

1. **Recipe.** When `t` is the product of a `manufacturing` or `reaction`
   activity in `blueprints.static`, and every material prices above 0:
   `Σ P_ref(m) · qty(m) / productQuantity`. The quantities are the base
   ones, at ME 0, exactly as the client's EIV does
   (`docs/lanes/industry.md` §4.1).
2. **PI.** When `t` is a `schematics` output and every input prices:
   `Σ P_ref(in) · qty / outQty`.
3. **Base.** Otherwise `basePrice`, when it is above 0.
4. **Parent.** Otherwise, when `t` has a `variationParentTypeID` with a
   price: `P_ref(parent) × META_MULT[metaGroupID]`.
   - `META_MULT` is an authored table:
     `{1: 1, 2: 1, 3: 2, 4: 5, 5: 20, 6: 8, 14: 1}`. **[A]**: the data's own
     `basePrice` ratios are too noisy to fit; meta 4 has a p10–p90 of
     0.5–3.3.
   - The falsifier is a faction module that the players think is
     mispriced. The fix is to tune the table, not the code.
5. **Override.** `content/econ/price-overrides.tsv` (§3.5).
6. **Floor (always applied last):**
   `P_ref(t) = max(P_ref(t), Σ P_ref(m) · q(m) / portionSize(t))` over
   `typematerials.materials`. The census shows that this lifts 330 types:
   153 modules, 46 ore, 32 drones, 29 category 39, 22 implants and 14
   materials.

**Coverage:** of the 11,700 non-cosmetic market types (categories
91/2118/30/63/2100/66/87 excluded), **9,862 are priced and 1,838 are not**.
The unpriced ones are:

- 419 commodities: salvage, faction tags, strongboxes and NPC items;
- 330 T1 and 320 T2 **rigs**, whose recipes need salvage (`basePrice` 0);
- 228 + 92 + 92 implants (the pirate sets);
- a long tail.

**Salvage is the keystone**: pricing its 41 types prices around 650 rigs.

Sample outcomes, not bulk:

| Type | `P_ref` |
|---|---|
| Rifter | 256,000 |
| Drake | 33.4 M |
| Damage Control II | 39,845 |
| 10MN Afterburner II | 260,144 |

These are internally consistent and well below retail. That is correct for
an isolated economy: only relative prices matter.

### 3.3 Extractor

`tools/content.py prices` writes the following (or a stand-alone
`tools/econ.py` if C1's unified extractor is not adopted):

- **`out/content/prices.tsv`**, one row per priced market type:
  - columns: `typeID refPrice source floorPrice categoryID groupID
    metaGroupID portionSize`;
  - `source` is one of `recipe | reaction | pi | base | parent | override`,
    and `floorPrice` is the item-6 value;
  - about 9.9k rows. It is CCP-derived, so it goes in `out/` and is never
    committed.
- **`out/content/npc-trades.tsv`**: `corporationID typeID value`, about
  18k entries, from `corporationTrades`.
- **`out/content/stations.tsv`**: `stationID solarSystemID regionID ownerID
  operationID services reprocEff reprocTake`, 5,210 rows. `services` is the
  bitmask of service ids from `stationoperations`. This replaces the
  per-station TODO in `reprocess.rs` (`docs/lanes/industry.md` §2.5) as
  well.
- **`out/content/recipes.tsv`** is optional. It is needed only if the
  server must re-derive prices after a balance change. It is not needed in
  phase 1.

### 3.4 The trading index (`averagePrice`)

- `avg(t)` is an EWMA of the volume-weighted daily mean trade price, with
  α = 0.2 per day **[A]**. NPC fills count.
- It is seeded at `P_ref` and clamped to `[0.5, 2.0] × P_ref` **[A]**.
- The server keeps it in a persisted record, `mktidx <typeID> <avg> <day>`,
  written only for types that have traded. Absent means `avg = P_ref`.
- **The server uses `P_ref`, never `avg`, for EIV, reprocessing tax and
  insurance.** That keeps the sinks predictable and stops a single trade
  from manipulating them.

### 3.5 Authored overrides (committed: our numbers, not CCP's)

**`content/econ/price-overrides.tsv`**, with columns `typeID price why`, is
seeded by rule:

- **Salvage (group 754, 41 types):** price by tier, with
  `T(i) = 2,000 × 3^tier` **[A]**.
  - Tier is the order in which the salvage first appears in rig recipes,
    sorted by rig `metaGroupID`: T1-only salvage is tier 0, and T2-only
    salvage is tier 2.
  - The table is authored once from that ordering. It cannot be derived
    automatically, because salvage drop rates are server-side (C6).
- **Faction tags, strongboxes and NPC trade items** in commodity groups
  with no recipe: leave them unpriced. They are LP-store and mission
  inputs; C4 decides their value through the LP rule, §5.6.
- **Pirate implant sets:** `P_ref(slot 1 Alpha mid-grade) = 20 M`, then
  ×1.5 per slot and ×3 per grade. The base is **[A]**.

Overrides are applied before the floor (step 6), so the floor still guards
them.

---

## 4. The NPC exchange

### 4.1 What an NPC order is

An NPC order is a `market::Order`. The differences are these:

- `char_id` is the station owner's corporation id, from 1,000,000 to
  2,000,000. On the public book it is never sent to the client: the
  `charID` column is absent by design (`market.md` §1).
- The order is **virtual**: it is computed from
  `(stations.tsv, npc-trades.tsv, prices.tsv, hub list, day, delta)` when a
  reader asks.
- Its id is `NPC_ORDER_BASE + station_index × 2^20 + type_slot × 2 +
  is_bid`, with `NPC_ORDER_BASE = 10^13` **[A]**.
  - The id span is 5,210 × 2^20, about 5.5e9, so a base between the
    player band (`FIRST_ORDER_ID` 5e9, `market.rs:408`) and the
    transaction band (7e9) would overlap them. 10^13 is clear of both.
  - `station_index` is the row's position in `stations.tsv`, sorted by
    `stationID`.
  - The id is day-independent. The refill state carries the time instead
    (§4.4).
- **On a fill** there is no escrow on either side:
  - an NPC ask **mints** the goods into the buyer's hangar at the ask's
    station (`item::Edit::Create`), and the buyer's ISK leaves the
    economy;
  - an NPC bid **destroys** the goods, and ISK is minted to the seller;
  - `move_isk` toward an NPC corp is already a no-op on the NPC side
    (`reg.character_mut(corp)` is `None`, `market.rs:881-923`), so the
    journal row is written for the player only. That is the faucet or sink,
    and it is what is wanted.
- The range is `rangeStation` (−1) for both sides **[A]**. Retail NPC bids
  often had region range, but a station range keeps hauling meaningful and
  keeps the band per hub.
- `duration` is 90 **[A]**, and `issueDate` is the day's rollover time.

### 4.2 Tier A: retail-true NPC trade

For every station `s` whose owner `c` has `corporationTrades`, and every
`(type, v)` in it:

- **`v > 0`: an NPC sell order at `s`.**
  - Price is `basePrice` for categories 9 and 16 (BPOs and skillbooks).
    **[I]**: those are the retail NPC prices, the medians are 10 M and 3 M,
    and 9 + 4 of them are 0, which means no order.
  - Every other type gets `1.10 × P_ref` **[A]**.
  - Volume is `TIER_A_STOCK` = 1,000 for categories 9 and 16, which is in
    effect unlimited. Others get `ceil(TIER_A_VALUE / price)` clamped to
    [5, 5,000], with `TIER_A_VALUE` = 20 M ISK **[A]**.
- **`v < 0`: an NPC buy order at `s`.**
  - Price is `0.90 × P_ref` **[A]**, and the volume rule is the same.
  - Most of these are trade goods with a `basePrice`, so they are priced.
    Unpriced types are skipped.
- `|v|` is **not** used in phase 1, because its meaning is unknown (§2.3).
  The falsifier for any use of it is a reading of CCP's data from which
  `|v|` is derived.

This alone puts:

- every skillbook and BPO a player can expect on sale somewhere, at retail
  prices;
- 2–7 trade goods in a typical station;
- about 1,460 types per region.

It is the "inhabited" look of the market browse tree (`docs/reference/market-browse.md`
§4), and it is honest: those NPCs really trade.

### 4.3 Tier B: the small-population backstop **[A]**

Tier B applies at **hub stations**, `--econ-hubs`. The default is the five
§2.4 hubs plus the sandbox home station 60012526. The operator adds the
humans' home stations.

- **Ceiling asks.** These are T1 ships, modules, charges and drones
  (categories 6, 7, 8 and 18, `metaGroupID` 1 or None, `techLevel` 1) that
  have a recipe and a price: **934 types** (476 modules, 268 charges, 143
  ships and 47 drones).
  - Price is `m_ask × P_ref`, with `m_ask` starting at **1.25**.
  - Volume is `ceil(TIER_B_VALUE / price)` clamped to [2, 10,000], with
    `TIER_B_VALUE` = 20 M per type per hub per day. That gives about 78
    Rifters.
- **Floor bids.** These cover:
  - minerals (group 18, 15 types), ice products (423, 7), moon materials
    (427, 20), salvage (754, 41, once priced), and PI P0–P4 (groups
    1032–1035, 1040–1042, about 83);
  - every **loot-only module**: category 7 or 8, `metaGroupID` in
    {1, 3, 4, 5, 6} and **no recipe**, which is the C6 drop pool.

  Price is `m_bid × P_ref`, with `m_bid` starting at **0.60**. Volume is
  the same rule, with `TIER_B_VALUE`.
- **No NPC bids for anything buildable, and no NPC asks for anything above
  T1.** Building, T2, faction and officer gear stay player and bot goods.
  This keeps industry worth doing, and it is what makes the arbitrage
  proof in §6 short.

**What Tier B buys us.**

- With 2 humans the market still clears. Mined minerals always have a
  buyer at 0.6, and a lost Rifter can always be replaced at 1.25.
- A human or bot industrialist who sells at 1.1 undercuts the NPCs.
- The NPC orders are the visible, honest edge of the band.
- **The falsifier** is "nobody builds anything because the NPCs are good
  enough". Then lower the ceiling to 1.15 or raise `m_ask` faster (§4.5).

### 4.4 Refill: lazy, hourly trickle, no tick

The stored state is one C7 delta per touched order:
`npcorder <orderID> <remaining> <touchedAt>`.

- On read, `remaining(now) = min(vol, remaining + vol × hours(now −
  touchedAt) / 24)` **[A]**. That is a full refill in 24 h with no timer.
- The record is deleted when `remaining == vol` and at the C7 rollover
  (C7 §4.2 step 4). This matches C7's
  `order_id(seed, day, station, type)` model, except that our id does not
  embed the day, because the order is the same standing order every day.
  C7's id embeds the day, and S1 must pick one. **Recommendation: no day in
  the id.** The client's `GetOrders` cache is keyed by `orderID`, and a
  daily id change would look like a cancel plus a new order.
- **The falsifier** is a player who buys out a hub, logs off for 2 h and
  comes back to find it a twelfth refilled. That is intended. If they
  complain, shorten the period.

### 4.5 Demand-responsive multipliers (bounded)

At C7 rollover, for each Tier B `(hub, type, side)` that traded that day,
let `f = units filled / vol`:

- **Ask:**
  - if `f ≥ 0.8`, `m_ask ← min(1.60, m_ask × 1.05)`;
  - if `f ≤ 0.1`, `m_ask ← max(1.15, m_ask / 1.02)`.
- **Bid:**
  - if `f ≥ 0.8`, `m_bid ← max(0.40, m_bid × 0.95)`, because players are
    dumping;
  - if `f ≤ 0.1`, `m_bid ← min(0.70, m_bid × 1.02)`.
- The record is `npcmult <hub> <typeID> <side> <m>`, stored only when it
  differs from the default. The step is deterministic, with no RNG.
- **Invariant, by the bounds:** `m_bid ≤ 0.70 < 1.15 ≤ m_ask`, always.

Tier A prices do not move. They are the static NPC world.

### 4.6 Readers that must merge NPC orders

These are all in `service/market.rs`, in the market lane's file. Each merge
is a call into a new `econ::exchange` module.

| Call | Merge |
|---|---|
| `GetOrders(typeID)` | Append the NPC rows of `(session region, typeID)` to both `CRowset` halves. Same descriptor and same `CachedMethodCallResult` wrapper. |
| `GetRegionBest` | `{typeID: KeyVal(price, volRemaining, typeID, stationID)}`. Take the best of the player and NPC asks per type, from a per-region index built at load: 40 regions × ≤ 2.2k types. |
| `GetStationAsks` / `GetSystemAsks` | The same, as 4-tuples (`docs/reference/market-browse.md` §4: the 4-sequence is forced by `BestByOrder(*order)`). |
| `PlaceBuyOrder` / `BuyMultipleItems` | `Book::side` must yield NPC asks in price order. The fill branches on `is_npc(order)`: mint instead of `escrow_stack`. |
| `PlaceMultiSellOrder` | Bids include NPC bids. The fill destroys instead of delivering. |
| `GetCharOrders` / history | Unchanged: an NPC order is never the actor's. |

An NPC fill **is** recorded as a `Trade`, with `seller_id` or `buyer_id`
set to the corporation id, so the transactions tab and the index (§3.4) see
it.

---

## 5. Faucets and sinks: each one specified

### 5.1 Market taxes (sinks): use the client's formula

All **[V]**, from `marketutil/const.py` and `marketutil/skilllimits.py`.

| Constant | Value | Where |
|---|---|---|
| `marketCommissionPercentage` (broker base) | 3.0 | `const.py:7` |
| `marketSccCommissionPercentage` | 0.5 | `const.py:8` |
| `BROKER_RELATIONS_SKILL_MODIFIER` | 0.3 (points per level) | `const.py:10` |
| `ACCOUNTING_SKILL_MODIFIER` | 0.11 | `const.py:11` |
| `MARKET_FACTION_STANDING_MULTIPLIER` | 0.0003 | `const.py:13` |
| `MARKET_NPC_CORP_STANDING_MULTIPLIER` | 0.0002 | `const.py:14` |
| `MKT_TRANSACTION_TAX_DEFAULT` | 8.0 (%), read through a feature flag `'market-transaction-tax'` | `const.py:33-34`; `marketsvc.py:112-114` |
| `mktMinimumFee` | 100 ISK | `appConst` |
| `mktModificationDelay` | 300 s | `appConst` |
| max orders | `5 + 4·Trade + 8·Retail + 16·Wholesale + 32·Tycoon` | `skilllimits.py:82` |
| sales tax | `taxRate/100 × (1 − 0.11·Accounting) × (1 − GetLocationDiscount(faction, system))` | `skilllimits.py:86-88` |

The server spec:

- **Sales tax** applies on every sell-side fill, NPC bids included. It is
  `round(value × 0.08 × (1 − 0.11·L_acc), 2)`, journal ref
  `refTransactionTax` = 54 **[V]**, payee the station owner. Today it is
  not charged.
- **Broker fee** applies to resting orders. The server computes
  `max(100, value × (0.03 − 0.003·L_br − standings terms))`. On a mismatch
  with the client's `expectedBrokersFee` beyond 1e-6 it raises
  `MktBrokersFeeUnexpected2`, and the key is **[A]** (`market.rs:858`).
  Phase 1 may keep the client's figure but clamp it to the server
  formula.
- Add ref 54 to `account.GetEntryTypes` (`wallet-ledger.md` §5.3: an
  unknown ref drops the row).

### 5.2 Industry and reprocessing (sinks): switch the leaf price

- `industry::adjusted_price` and `reprocess::base_price` both become
  `econ::prices::p_ref`. The tables in `industry/tables.rs` and
  `reprocess/tables.rs` stay as the fallback for a server without
  `--content`, the existing pattern.
- This fixes the gas-refine-is-free edge (`reprocess.rs:654`), because
  gas gets a recipe-derived or base price wherever the corpus allows one.
- A **system cost index** is still absent. Keep a constant
  `COST_INDEX = 0.02` for manufacturing and 0.02 for science **[A]** as a
  sink knob, and send it through the facility modifiers the industry lane
  already answers. The falsifier is the job cost tooltip disagreeing with
  the charge. Fix that in the same packet.
- **[V] constants already in the tree:** `SCC_SURCHARGE_RATE` 0.04,
  `INDUSTRY_SECURITY_TAX_RATE` 0.05 (safe-sec only), and the reprocess
  take 0.05 or 0.025 per station.

### 5.3 Bounties (faucet) and the dynamic bounty system

- **[V]** `dynamicBountyMgr.GetOutputForClientSolarSystem()` → `(output,
  dbsIsActive)` (`dynamicResourceSvc.py:104-106`). The client memoizes it
  for 5,400 s (`:92`).
- **[V]** The push `OnSystemDynamicBountyStateChanged(systemID, rate)`
  primes that cache with `(rate, True)` and fires
  `OnDynamicBountyUpdate_Local` (`:201-208`).
- **Server:** a per-system multiplier `d_s` ∈ [0.25, 1.25] **[A]**,
  default 1.0, stored as `dbs <system> <d> <touchedAt>` only when it is
  not 1.0.
  - The payout is `bounty481 × d_s × DBS_GLOBAL`.
  - Each payout lowers `d_s` by `paid / POOL_s`, with
    `POOL_s = 200 M × (1.5 − sec)` ISK **[A]**.
  - `d_s` recovers toward 1.0 at 4 % an hour, lazily on read, like §4.4.
  - Push to pilots in system when `d_s` moves by ≥ 0.05.
  - Answer `(d_s, True)`.
- **Falsifier:** "my system's bounties collapsed after an hour". In that
  case raise `POOL_s`.
- The NPC bounty **security tax** of 0.2 is **[V]** (`security/common/const.py:5`).
  Where CCP applies it is **unknown**, so it is not applied.

### 5.4 Mission rewards (faucet)

C4 owns the amounts. The C5 rule is that a mission's ISK reward is
`k_L × median bounty of its spawn`, with `k_L` = 1.0, 1.5, 2.0 and 3.0 for
L1–L4 **[A]**, so missions and ratting pay comparably. The ISK bonus is
`0.5 × reward` if done in time. Refs are 33 and 34 **[V]**.

### 5.5 Insurance (premium sink, payout faucet)

**[V]**, from `eve/common/script/util/insuranceConst.py`:

- `BASE_INSURANCE_FRACTION` 0.4 and `INSURANCE_FRACTION_INCREMENT` 0.5
  (`:5-6`);
- packages Basic 0.5, Standard 0.6, Bronze 0.7, Silver 0.8, Gold 0.9 and
  Platinum 1.0 (`:33-39`);
- `price(fraction) = full × (fraction − 0.4) × 0.5` (`:67-72`), so
  Platinum costs 30 % of `full` and Basic 5 %;
- `base_insurance.GetQuoteForShip` (`:206-216`) builds the quote rowset
  from `insuranceSvc.GetInsurancePrice(typeID)`, which is `full`.

The server spec:

- `GetInsurancePrice(typeID) = 0.8 × P_ref(hull)` **[A]**. The 0.8 is below
  material cost, so build, insure and self-destruct is a loss (§6).
- The payout is `payout_ratio × full`, **[I]** from the field name
  `payout_ratio`. Journal ref `refInsurance` = 19 **[V]**.
- No payout when:
  - the killer is the owner;
  - the killer shares fleet or corp with the owner;
  - the killer is CONCORD.

  These voids are **[A]**, mirroring retail's anti-fraud rule.
- **The base payout is a faucet** (#134, ins-base) **[A]**: an *uninsured*
  loss of a priced hull also pays its owner at death
  `BASE_PAYOUT_FRACTION (0.4) × full`, with no premium paid. Each recipient
  is voided on its own (a thief destroying a boarded hull is paid nothing,
  nor is the holder), a capsule or a rookie ship pays nothing, and one hull
  pays once. What the voids do not catch is two friends in different
  corporations and fleets trading kills: the base is then ISK per hull
  lost, bounded by the hull cost (§6.4).
- Contracts last 12 weeks **[A]**.
- This is a packet of its own (insurance contract rows, a loss hook). The
  shapes are already pinned in `service/inventory.rs:784-829`.

### 5.6 LP and the LP store (a sink with a faucet in disguise)

C4 owns the offers. C5 gives the pricing rule so that offers neither print
nor burn ISK:

- every offer satisfies `P_ref(product) × qty ≈ LP × R + ISK + Σ
  P_ref(required items)`, with `R` = 1,000 ISK/LP **[A]**;
- the ISK part is 40 % of the left side **[A]**, which makes the store a
  real sink;
- a mission's LP reward is `0.5 × ISK reward / R` **[A]**.

The falsifier is players converting LP at well over R. Then cut `R`. The
balances are already answered by `LPSvc` (`wallet.rs:167`).

### 5.7 Other sinks already present or cheap

- **Skillbooks and BPOs** (Tier A, §4.2) are the largest natural sink.
  Skill injection from a book exists (`service/character.rs:4847`).
- **PI** customs taxes exist.
- **Clone and jump fees** are `refJumpCloneInstallationFee` 55 and
  `refCloneTransfer` 52 **[V]**. Leave them as they are.

### 5.8 Starting wallet (faucet)

New characters get **1,000,000 ISK** **[A]**, journal ref 3 (GM transfer)
or a dedicated "starting funds" row. That is enough for a Tier B frigate,
fittings and a skillbook or two. The falsifier is a new player who cannot
undock with something useful, or one who never needs to earn.

---

## 6. Arbitrage invariants (the anti-exploit proof)

Let `Y` be the best achievable refine yield.

- Scrap is `0.5 × (1 + 0.02 × 5)` = 0.55.
- Ore at max skills is about 0.69 **[I]**, from the reprocess lane's
  formula. That is ore only, and ore has no NPC ask.

The guarantees, each a test in the econ packet:

1. **Buy from NPC ask, then reprocess** loses money. The ask is at least
   `1.10 × P_ref(t)`, by the Tier A rule or Tier B's ≥ 1.15. The output is
   at most `Y × Σ P(m) q / portion`, which is at most `Y × P_ref(t)` by the
   step-6 floor. Then that output is sold at most at an NPC bid of
   `0.70 × P(m)`. So the gain is at most `0.55 × 0.70 = 0.385 ×` against a
   cost of at least 1.10 ×. **Without the floor, 128 types violate this
   even at ask = 1.0 × `basePrice`** (§2.2 census).
2. **Buy NPC materials, build, sell to NPC bid** is impossible: there are
   no NPC bids on buildable types (§4.3).
3. **Buy at NPC ask and sell to NPC bid of the same type** is impossible:
   `m_bid ≤ 0.70 < 1.10 ≤` the ask multiplier, and Tier A's ask and bid
   are 1.10 and 0.90.
   - Corner case: a Tier A buy and a Tier A sell of the same type at
     different stations in one region. The census shows 2,394 positive
     and 348 negative types out of 2,705, so at least 37 types have both
     signs somewhere. **Rule: if a type has both signs in one region,
     drop the bid.**
4. **Build, insure Platinum and self-destruct** loses money.
   - The payout is `1.0 × 0.8 × P_ref`.
   - The premium is `0.3 × 0.8 × P_ref = 0.24 × P_ref`, so the net is
     `0.56 × P_ref`.
   - Against that, the hull's materials sold to NPC floors fetch at most
     `0.70 × P_ref`. The insurance route is worse.
   - The voids in §5.5 kill self-destruct entirely.
   - The uninsured base (§5.5, 0.4 × `full` = 0.32 × `P_ref`) is below
     the hull's build cost (about `P_ref`), so losing a hull on purpose to
     a friend outside the voids still loses at least 0.68 × `P_ref`.
5. **Bounty farming** is bounded per system by DBS (§5.3).

---

## 7. Stability with 2–10 humans and about 50 bots

### 7.1 The problem in numbers **[A]**

These are order-of-magnitude figures, given to size the knobs. They are not
predictions.

- **Bots.** Take 30 ratting bots at an effective 300k ISK an hour in
  high-sec belts (belt frigates are 4k–30k each, `npc.rs:239`, §2.5 p10–p25).
  Running all day, that is about **216 M ISK a day** into bot wallets.
- **Humans.** 5 humans at 2 h a day and about 1 M an hour is about
  **10 M a day**.
- Left alone, bots are **95 %** of the faucet. If bots spend into human
  sell orders without limit, human wealth is set by bot behaviour, not by
  play.

### 7.2 Rule: bot ISK is fuel, not wealth

- Bots are ordinary wallets on the ordinary wire. C7 §6.1: no privilege.
- **Wallet cap.** At rollover, bot balances above `BOT_WALLET_CAP` = 50 M
  **[A]** are burned. The journal ref is unknown; use the GM-transfer ref
  3 with description "upkeep". The bot marker is C7 packet L8,
  `Character::is_bot`.
- **Demand budget.** A bot buying from a **human** order is limited to
  `BOT_DEMAND = 4 M × humans_active_today / bots` ISK a day per bot
  **[A]**, so bots inject at most about 4 M a day per active human into
  human hands. Purchases from NPC orders are unlimited, since those are
  sinks.
- **Bot roles.** Each role is a brain in `eve-bot`, using the real
  `marketProxy` verbs:

  | Role | Produces | Sells to | Buys |
  |---|---|---|---|
  | Miner (exists) | ore, then minerals at the hub refinery | human bids ≥ the NPC floor, else the NPC floor | Tier B mining frigate and lasers on loss |
  | Ratter / escort (exists) | bounties, loot | NPC floor for loot modules | ammo, hull replacement (Tier B or human) |
  | Builder (new) | T1 hulls and modules from minerals bought on the market | resting asks at `1.10 × P_ref`, under the Tier B ceiling | minerals from human or bot asks first, else none (there are no NPC mineral asks) |
  | Trader (later) | nothing | hauls between hubs inside the band | — |

  The builder is the role that makes the market feel inhabited: there are
  orders that are not NPCs, and their prices move.

### 7.3 The daily ledger and the controller

At rollover, sum the journal by ref into `econ <day> <ref> <sum>` records.
The faucet refs are 17, 33/34, 19, NPC-bid fills and 3 (starting funds).
The sink refs are NPC-ask fills, 46, 54, 56, 127, 96/97, insurance premiums
and the bot burn. **Human-held money supply** is `M_h = Σ balances of
non-bot characters`.

**The controller** is **[A]**, runs once a day, and steps at most 0.05 per
day:

- if `M_h` has grown more than 3 % a day for 3 days, `DBS_GLOBAL ← max(0.6,
  DBS_GLOBAL − 0.05)` and Tier B `m_bid` defaults drop 0.02;
- if `M_h` has shrunk more than 2 % a day for 3 days, the reverse, with a
  cap of 1.2 on `DBS_GLOBAL`;
- the dead-band does nothing, which is the expected state.

Phase 1 ships the ledger and **no controller**. The controller is enabled
only after a week of `[econ]` lines shows a drift. The falsifier is a week
of logs with flat `M_h`, in which case the controller is never needed.

### 7.4 Observability

- One `[econ] day N faucet=… sink=… net=… M_h=… M_bots=… npc_fills=…
  trades=… dbs_min=…` line at rollover, beside C7's `[live]`.
- `/econ` (GM) prints the line and the top ten types by NPC fill value.
- `/econ price <type>` prints `P_ref`, `avg`, its source and the Tier
  multipliers at the actor's hub.

---

## 8. Price history and pruning

- **Readers:** `GetOldPriceHistory`, `GetNewPriceHistory` and
  `GetHistoryForManyTypeIDs`. The positional columns `historyDate,
  lowPrice, highPrice, avgPrice, volume, orders` are **[V]**, pinned in
  `market.rs`.
- **Rows:** one per `(region, type, day)` with any trade, NPC fills
  included, aggregated at rollover into
  `mkthist <region> <type> <day> <low> <high> <avg> <vol> <n>`.
- **Old/new split:** "new" is the last 30 days and "old" is everything
  before **[A]**.
- **No fabricated history.** A type with no trades shows a flat chart,
  which is honest (`docs/reference/market-browse.md` §5 item 14).
- **Pruning** is required before bots trade, because `Book` is
  `Vec`-scanned and fully rendered on every save:
  - `mkttrade` rows older than 30 days are deleted at rollover, since
    their aggregates live on in `mkthist`;
  - closed `mktorder` rows older than 30 days are deleted as well.
  - The falsifier for the 30 days is a player missing an older entry in
    the Transactions tab.

---

## Proposals

1. **Adopt the price oracle `P_ref`** (§3.2): the recipe, PI, base, parent,
   override and reprocess-floor rule. Make it the **single source** for
   `config.GetAverageMarketPrices`, industry EIV, the reprocessing ISK cost
   and insurance. This fixes the client/server EIV disagreement that exists
   today.
2. **Serve `config.GetAverageMarketPrices` as `{typeID:
   KeyVal(adjustedPrice=P_ref, averagePrice=avg)}`** for every priced type.
3. **NPC orders are virtual**, owned by the station's corporation, with ids
   `10^13 + station_index·2^20 + slot·2 + bid`. Their state is only an
   `npcorder` delta with a lazy hourly trickle refill. Fills mint and
   destroy items and ISK.
4. **Tier A from `corporationTrades`** at every owning station: BPOs and
   skillbooks at `basePrice`, other sells at 1.10 P, buys at 0.90 P. Ignore
   `|v|`.
5. **Tier B at `--econ-hubs`** (5 empire hubs, the home station and
   operator additions):
   - ceiling asks on 934 buildable T1 types at 1.25 P;
   - floor bids on raw materials, salvage, PI and loot-only modules at
     0.60 P;
   - never an NPC bid on a buildable type;
   - multipliers bounded to [1.15, 1.60] and [0.40, 0.70].
6. **Charge the client's own market taxes** (§5.1): sales tax 8 % ×
   (1 − 0.11·Accounting) as ref 54, and the broker fee by the server
   formula with the 100 ISK minimum.
7. **DBS as the bounty governor** (§5.3): a per-system multiplier with a
   pool and lazy recovery, served on the existing
   `dynamicBountyMgr` arm with the `OnSystemDynamicBountyStateChanged`
   push.
8. **Insurance on the [V] formula** with `full = 0.8 × P_ref(hull)` and
   anti-fraud voids.
9. **Bot ISK is fuel**: a 50 M wallet cap burned at rollover, a daily
   demand budget into human orders, and the miner and ratter bots sell to
   the market. Add a builder bot.
10. **The `[econ]` ledger from day one; the controller only when the ledger
    shows drift.**
11. **Starting wallet of 1 M ISK.**
12. **Prune market history at 30 days into `mkthist` aggregates** before
    bots are allowed to trade.
13. **Commit `content/econ/price-overrides.tsv` and `content/econ/params.tsv`**
    (our numbers, not CCP's). All CCP-derived tables go to `out/content/`.

## Packets

Each packet owns files (lane rule 1). Packets that need a hunk in a file
another lane owns name that hunk. The coordinator applies it, or it goes in
as a seam commit first.

| # | Packet | Owns | Hunks elsewhere | Depends | Tests |
|---|---|---|---|---|---|
| **E1** | Economy extractor | `tools/econ.py`, or the `prices`/`trades`/`stations` verbs of C1's `tools/content.py` | none | — | Python self-check mode: Tritanium = 2.0; every priced type ≥ its floor; Rifter source is `recipe`; the corp-trade key decode resolves names (spot ids, no bulk in the repo) |
| **E2** | Price oracle in the server | new `src/econ/prices.rs` (load `prices.tsv`, `mktidx` record, `p_ref`, `avg`) | `service/base.rs` (the `GetAverageMarketPrices` arm); `industry.rs::adjusted_price` and `reprocess.rs::base_price` delegate when loaded; `store.rs` record tag | E1 | the answer is a dict of KeyVal with both attributes; without `--content` the old tables still answer; the EWMA clamps to [0.5, 2]; EIV of a fixture blueprint equals the client formula over `p_ref` |
| **E3** | NPC exchange | new `src/econ/exchange.rs` (station index, Tier A/B generation, ids, `npcorder` and `npcmult` deltas, lazy refill, rollover step) | `service/market.rs`: `Book::side` merge, the NPC branch in `buy` and `sell` fills, the readers in §4.6 | E2, C7 L2 and L5 | buying from an NPC ask mints the goods and debits ISK with no escrow stack; selling to an NPC bid destroys the goods; refill after 6 h is a quarter; ids are disjoint from the 5e9 and 7e9 bands; **the arbitrage invariants of §6 as property tests over a fixture catalogue**; `GetRegionBest` includes NPC asks |
| **E4** | Market taxes | `service/market.rs` (the market lane's file; one packet, not shared) | `service/wallet.rs` `GetEntryTypes` (+54) | — | sales tax at Accounting 0 and 5; broker minimum 100; ref 54 journaled to the seller only |
| **E5** | DBS | new `src/econ/dbs.rs` | `service/space.rs` arm, `npc.rs::pay_bounty` multiplier, push registration | C7 L1 | the pool drains and recovers lazily; the push fires on a ≥ 0.05 step; the answer is a `(float, bool)` tuple |
| **E6** | Insurance | new `service/insurance.rs` (moved out of `inventory.rs`); a contract record in the store | `inventory.rs` (remove the arm), the ship-loss hook in `combat.rs` | E2 | quote rowset values equal the `[V]` formula; payout on an NPC kill; no payout on self or fleet kills; the contract expires |
| **E7** | History and pruning | `service/market.rs` `Book` (`mkthist`, prune) | `store.rs` record tag | C7 L5 | the aggregate equals the trades; pruning keeps 30 days; a world file without `mkthist` loads |
| **E8** | Ledger and `/econ` | new `src/econ/ledger.rs`, a GM slash module | rollover registration | C7 L5 and L6 | ref classification is total over the refs in use; the bot burn caps at 50 M and journals it |
| **E9** | Bot market roles | `eve-bot/src/verbs.rs` market verbs (`sell`, `buy`, `orders`), a `builder` brain | none server-side | E3; **after the W10 bot lane lands** | the miner sells minerals and the wallet rises; the builder posts asks under the ceiling; the demand budget is respected over a simulated day |
| **E10** | Authored overrides | `content/econ/price-overrides.tsv`, `content/econ/params.tsv` | — | E1 | the extractor applies overrides before the floor; rigs gain coverage (about 650 types) |

**Order:** E1 → E2, the client-visible fix. Then E4, which is independent.
Then E3, which needs C7 L2 and L5. Then E7 → E8 → E5 → E6 → E9 → E10.

## unknown

- The meaning of `corporationTrades` **magnitudes**, and whether the sign
  really is supply versus demand (§2.3). It is **[I]** from content only.
- The meaning of `stationoperations.border/corridor/fringe/hub/ratio`.
- `npcstationtaxmodifier.fsdbinary`: its pyd stride is 24, but
  `fsd.CfsdTable` reports no stride, so it was not decoded here. The
  reprocess lane says it is not the standings curve (`reprocess.md` §3.5).
- Whether CCP's `adjustedPrice` and `averagePrice` differ in kind, which
  needs a capture.
- Where `NPC_BOUNTY_SECURITY_TAX_RATE` (0.2) and
  `MARKET_SECURITY_TAX_RATE` (0.05) apply in this build.
- Whether `insuranceSvc.GetInsurancePrices` is a dict (its consumer is
  `pass`).
- Whether NPC corporation ids resolve through `config.GetMultiOwnersEx` for
  the journal's counterparty name (`service/base.rs` "only the ids we can
  name").
- The real refine yields at max skills for scrap and ore in this build.
  §6 uses 0.55 and 0.69 **[I]**.
- The salvage tier ordering in §3.5 needs one authoring pass. Drop rates
  are C6's.
- Every **[A]** number in §4, §5 and §7 is a starting value. None of them
  may be pinned in a test (`eve-offline-assumptions-are-ok`). Tests pin
  the invariants and the `[V]` formulas only.
