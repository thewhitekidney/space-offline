# Inventory: market, wallet and economy (EvEJS)

This is EvEJS's whole ISK-and-goods layer: a market with real order matching, a
player-to-player contract system, a trade window, ship insurance, an in-game
store (NES/PLEX), an LP store, and the wallet journal all of it books against.
It is roughly **26,000 lines of JavaScript** in the areas named below, plus
**~7,100 lines of Rust** in a separate market daemon process and **~28,300
lines of Rust** across three market seed builders — about **61,000 lines**
total, the largest single economic subsystem in their tree. The centre of
gravity is one 7,439-line file, `services/market/marketProxyService.js`, which
holds the client service, the matching loop, the escrow bookkeeping and five
distinct compensating-transaction sagas. Two details reframe the area: the
market's data and order state live in a **separate Rust process** the Node
server talks to over TCP (question 1), and **the only tests in the entire
EvEJS release are 15 Rust `#[test]` functions inside that daemon** — 9 in
`externalservices/market-server/src/state/transaction_tests.rs`, 2 in
`order_book_tests.rs`, 4 inline in `src/state.rs`. There are zero JavaScript
tests. Everything on the Node side, including all of the escrow and matching
logic, is unverified by construction.

## Subsystems

| subsystem | their files (count + key names) | approx LOC | status | evidence |
|---|---|---|---|---|
| Market browse (availability dict) | 2: `market/marketProxyService.js`, `market/marketTopology.js` | ~900 of 7,439 + 291 | functional | `Handle_GetStationAsks`/`GetSystemAsks`/`GetRegionBest` at `marketProxyService.js:6313,6356,6396`; shape built by `buildSummaryDict` `marketProxyService.js:354`, tuples for station/system (`:6338,:6378`), named KeyVal for region (`:6413-6418`) |
| Market order book + price history | 1: `market/marketProxyService.js` (+ daemon) | ~700 | functional | `buildOrderRowset` `marketProxyService.js:389`, `buildHistoryPair` `:486`; data from daemon RPC `GetOrders`/`GetHistories` `rpc.rs:245,259` |
| Market order placement + fees | 1: `market/marketProxyService.js`, rules in `market/marketRules.js` | ~1,100 + 340 | functional | `executeBuyRequest` `marketProxyService.js:5450`, `executeSellEntryUnlocked` `:5724`; broker fee `marketRules.js:208`, SCC `:243`, sales tax `:278` |
| **Order matching + escrow** | 1: `market/marketProxyService.js`, state in `market/marketEscrowState.js` | ~2,400 + 148 | functional but unverified | `matchSellOrdersForBuyer` `:4344`, `matchBuyOrdersForSeller` `:5132`, five sagas at `:3111,:3472,:3848,:4694`; escrow table `marketEscrowState.js:8` |
| Order expiry + crash recovery | 1: `market/marketProxyService.js` (+ daemon sweep) | ~800 | partial | `processPendingExpiryEvents` `:2223` polls daemon `GetOrderEvents`; `reconcileDurableMarketOrders` `:2707`, `sweepStrandedMarketEscrowItems` `:2618`. Daemon side `sweep_expired_orders` `rpc.rs:326` |
| Market stub fallback | 1: `market/marketService.js` | 133 | stub | every handler returns an empty rowset, e.g. `marketService.js:101-108` |
| Rust market daemon | 7 Rust: `src/state.rs`, `src/main.rs`, `src/rpc.rs`, `src/config.rs`, `crates/market-common/src/lib.rs`, 2 test files | 7,077 | functional, tested | 29 RPC methods `rpc.rs:216-357`; 17 HTTP routes `main.rs:180-199`; 15 `#[test]` |
| Market seed builders (v1/v2/v3) | 3 Rust crates: `tools/market-seed`, `market-seederv2`, `market-seederv3` | 2,529 + 2,034 + 23,743 | functional | see Static data provenance below |
| Contracts: item exchange + courier | 4 in `services/contracts/` + 2 services in `services/_other/` | 4,725 + 1,041 | functional | `contractRuntimeState.js:2082` (item exchange legs), `:2118` (courier collateral), `deliverItemExchangeAcceptance` `:2467`, `deliverCourierAcceptance` `:2604` |
| Contracts: auction | same files | — | stub | type exists (`CONTRACT_TYPE.AUCTION = 2`, `contractRuntimeState.js:74`) but `Handle_PlaceBid` returns `null` and `Handle_FinishAuction` returns `false` — `_other/contractProxyService.js:871,876`; accept path rejects it `contractRuntimeState.js:2141` |
| Contracts stub fallback | 1: `services/_other/contractMgrService.js` | 88 | stub | `Handle_CreateContract` → `null`, `contractMgrService.js:71` |
| LP store | 3: `loyalty/lpStoreOfferCatalog.js`, `loyalty/lpStoreStaticOffers.json` (9.3 MB), service in `evermarks/lpStoreMgrService.js` (+ `evermarks/lpStorePurchaseState.js`, `inventory/lpStoreItemCustody.js`) | 196 + 424 + 782 + 105 | functional (data-driven) | `Handle_GetAvailableOffersFromCorp` `lpStoreMgrService.js:360`, `Handle_TakeOfferForCharacter` `:383` |
| New Eden Store (NES) / PLEX | 8 in `services/newEdenStore/` | 3,346 | partial | `storePurchaseSettlementState.js:574` reads `plexBalance`; `Handle_BuyOffer` `FastCheckoutService.js:160`. But `kiringMgrService.js:164` `Handle_PlaceKiringOrder` is a stub, and the catalog is operator-authored, not CCP's (`storeState.js:14` `AUTHORITY_TABLE = "newEdenStore"`) |
| EverMarks | 9 in `services/evermarks/` | ~2,900 | partial | separate currency + catalog (`evermarksCatalog.js`, `evermarkPurchaseState.js`); not wired to the market |
| Insurance | 4 in `services/insurance/` | 2,000 | functional | `Handle_InsureShip` `insuranceService.js:69`; payout fires from ship destruction, not killmails — `space/shipDestruction.js:213` calls `insuranceRuntime.handleShipDestroyed` |
| Wallet + journal + market transactions | 1 (+ siblings): `account/walletState.js` | 1,206 | functional | journal ref types `walletState.js:44-82`, account keys `:36-39`, idempotency `:119` |
| Player trade window | 2 in `services/trade/` | 2,411 | functional | `Handle_MakeOffer` `tradeMgrService.js:2104`, ISK leg books `JOURNAL_ENTRY_TYPE.PLAYER_TRADING` `:1481,:1644` |
| Killmails | 8 in `services/killmail/` | 2,864 | functional — **not part of this area** | values losses from **static SDE `basePrice`**, not market prices: `getBasePrice` `killmailPreparationTasks.js:159`, used at `:275-280`. No reference to `insurance` or any market module anywhere in `services/killmail/*.js`. Belongs in the combat/PvP lane |

## The Rust market daemon  <-- question (1)

### Division of responsibility

The daemon is a **read-optimised order store with a durable expiry clock**. It
owns, and the game server does not:

* The whole market SQLite schema — 10 tables, `crates/market-common/src/lib.rs:43-239`:
  `regions`, `solar_systems`, `stations`, `market_types`, `seed_stock`,
  `seed_buy_orders`, `market_orders`, `market_order_events`,
  `region_summaries`, `system_seed_summaries`, `price_history`.
* Order identity. `market_orders.order_id` is `INTEGER PRIMARY KEY AUTOINCREMENT`
  (`lib.rs:113`); NPC seed orders get *synthetic* IDs from fixed bases
  (`SEED_SELL_ORDER_ID_BASE = 2e18`, `SEED_BUY_ORDER_ID_BASE = 4e18`,
  `lib.rs:40-41`) encoding `(station_id, type_id)`, decoded on fill at
  `state.rs:3017,3076`.
* The precomputed browse summaries, cached in memory per region/system/station
  with a generation guard (`state.rs:310-315`, `get_region_summary` `:335`).
* Order expiry as a durable sweep (`sweep_expired_orders` → `expire_due_orders_in_db`
  `state.rs:3257`), published as an append-only event log the Node server polls.
* Fill idempotency receipts: `market_fill_receipts` keyed by
  `idempotency_key TEXT PRIMARY KEY` (`state.rs:1073-1074`), replayed at
  `replay_fill_receipt` `state.rs:2925`.

The Node server owns everything the daemon has no concept of: **ISK, items,
characters, corporations, sessions, skills, taxes, standings and
notifications**. The clearest proof is that `market_orders` has an `escrow REAL`
column (`lib.rs:129`) that is **always inserted as the literal `0`**
(`state.rs:2642`) and only ever read back out in a SELECT (`state.rs:2638`).
Real escrow lives in a Node GameStore table called `marketEscrow`
(`marketEscrowState.js:8`) and, for items, at virtual locations
`MARKET_ESCROW_LOCATION_BASE + order_id` where the base is `9_200_000_000`
(`services/inventory/marketItemCustody.js:7`).

**Critically, the daemon contains no matching engine.** `fill_order_in_db`
(`state.rs:3004`) takes an order id and a quantity and decrements it; it never
looks at the other side of the book. Matching — price crossing, range checks,
order selection — is entirely Node's (`matchSellOrdersForBuyer`
`marketProxyService.js:4344`). The daemon is a ledger of order rows, not an
exchange.

### The RPC surface

Two listeners, both bound to loopback or the Docker bridge
(`config/market-server.example.toml`, `main.rs:170-204`):

* **HTTP on 40110** — 17 axum routes, `main.rs:180-199`. A sibling/diagnostic
  surface (`/health`, `/v1/manifest`, `/v1/diagnostics`, summaries, order book,
  history, owner orders, place/modify/fill/cancel, `/v1/admin/cache/rebuild`).
* **RPC on 40111** — what the game server actually uses. `rpc::serve` `rpc.rs:122`.

The RPC protocol is **newline-delimited JSON over a raw long-lived TCP socket**,
not HTTP and not JSON-RPC 2.0:

* Request: `{"id": <any>, "method": "<string>", "params": <object>}` — one line,
  `rpc.rs:36-42`, read by `read_line` `rpc.rs:152`.
* Response: `{"id", "ok": bool, "result"?, "error"?: string}` — `rpc.rs:44-52`.
  **Errors are free-form strings**, not codes.
* Requests are **pipelined and answered out of order**: each line is handed to
  its own `tokio::spawn` (`rpc.rs:165`) with a shared mutex only around the
  socket write (`rpc.rs:193`). Correlation is by `id` only.
* Client side: `MarketDaemonClient` (`services/market/marketDaemonClient.js:34`),
  one persistent socket, 1.5 s connect timeout, 15 s request timeout, 2 s
  reconnect (`:41-52`).
* There is a capability handshake: the client refuses a daemon whose `Health`
  does not advertise `idempotent-fill-v1` (`marketDaemonClient.js:9,126`;
  advertised at `rpc.rs:223`).

29 methods, `rpc.rs:216-357`. Reads: `Health`, `StartupCheck`, `GetManifest`,
`GetDiagnostics`, `GetRegionBest`, `GetSystemAsks`, `GetStationAsks`,
`GetOrders`, `GetHistory`/`GetHistories`/`GetOldPriceHistory`/`GetNewPriceHistory`,
`GetOwnerOrders`/`GetCharOrders`/`GetCorporationOrders`, `GetOrder`,
`GetOrderEvents`. Writes: `PlaceOrder`/`PlaceBuyOrder`/`PlaceSellOrder`,
`ModifyOrder`, `CancelOrder`, `CancelStationOrders`, `FillOrder`,
`AdjustSeedStock`, `RecordTrade`, `SweepExpiredOrders`,
`RebuildRegionSummaries`. Note the aliasing: several client-facing EVE method
names map to one implementation (`rpc.rs:253,265,285`).

### Why they split it, and what the real constraint was

Their own stated reasons are performance and isolation:
"faster market window reads… less pressure on the main Node server"
(`externalservices/market-server/README.md`, "What This Is For"). The daemon
even ships a `bench` subcommand that measures cold vs warm summary and order-book
latency (`main.rs:230-330`). The README is explicit that the daemon is
**optional** and the Node server boots without it — in which case the stub
`marketService.js` answers with empty rowsets and market calls surface as
"Market is currently offline" (`marketProxyService.js:1989-1997`).

The real constraint is **read volume against a dataset that does not fit a
JS-object game store**. The seeded database is ~177 MB for 23 stations /
19,352 types / 445,096 orders (figure supplied by the coordinator; the daemon's
own `manifest` carries these counts, `state.rs:273-283`). EvEJS's main
persistence is a JSON-ish in-memory `gameStore` loaded at boot
(`bootstrap.js:332` "Preload database into memory"), which is simply the wrong
shape for 445k rows plus 19k×30 history rows plus the per-type availability
dict the browse window demands on *every* market-window open. So they reached
for SQLite, and because SQLite in Node means a native module they instead put it
behind a process boundary with its own cache tier (8 read connections, WAL,
16 GB mmap, 2048-entry order-book LRU — `docker/market-server.toml`,
`state.rs:981-1010`).

The *correctness* constraint they discovered is separate and more interesting:
concurrent writes. `begin_write_transaction` uses `BEGIN IMMEDIATE` so the
writer lock is taken before any read, specifically because upgrading a WAL read
snapshot to a writer fails immediately with `BUSY_SNAPSHOT` despite
`busy_timeout` (`state.rs:990-992`, and the README's "Transaction Safety"
section). Fill + receipt + summary refresh commit together or not at all
(`commit_fill` → `transaction.commit()` `state.rs:3000`). That is the one piece
of this subsystem they wrote tests for.

### Recommendation for eve-offline

**Keep it as one process. Do not adopt the daemon, and do not adopt SQLite.**

The split buys them two things we get for free and one we must build ourselves:

1. *A real database for 445k rows.* We are a Rust binary already; a plain
   `Vec<Order>` plus a `HashMap<(region, type_id), Vec<usize>>` index is both
   smaller and faster than their SQLite-plus-LRU stack, and 445k orders is
   ~30–50 MB of owned Rust structs. This is a non-problem for us and the main
   reason their architecture does not transfer.
2. *Not loading 177 MB into a JS heap.* Also a non-problem.
3. *Serialised, atomic writes.* **This one is real and we must replicate it.**
   Their `BEGIN IMMEDIATE` is doing exactly what a single `&mut` on an in-memory
   order book does in Rust — and we get it statically instead of at runtime.
   The lesson to carry over is their *scope*: the order mutation, the
   idempotency receipt and the derived summary must be in one atomic step, or
   a crash between them leaves a visible summary that disagrees with the book.

The one thing to steal outright is the **durable summary/availability index**.
The client asks for the whole region's best-ask dict on every market-window
open (`docs/reference/market-browse.md` §4), so an O(types) precomputed index, invalidated
on fill, is load-bearing regardless of language. Their `region_summaries` table
(`lib.rs:162-173`) is the right shape for our in-memory version.

The thing **not** to copy is the two-process boundary itself: it is the direct
cause of the atomicity hole in question 2.

## Order matching and escrow  <-- question (2)

### How a buy order holds ISK

Escrow is a plain **wallet debit**, not a reservation. `executeBuyRequest`
(`marketProxyService.js:5450`):

1. Validates price/duration/min-volume/range and builds a per-character market
   context (skills, standings, station) — `:5481-5491`.
2. Pre-checks funds against the *upper bound* (reserve + broker fee + SCC
   surcharge) before matching — `ensureCharacterHasFunds` `:5509`.
3. Runs the immediate match first (below), then for whatever is left over
   issues **three separate debits**: the reserve under
   `JOURNAL_ENTRY_TYPE.MARKET_ESCROW` (42), the broker fee under `BROKERS_FEE`
   (46), and the SCC surcharge under `MARKET_PROVIDER_TAX` (149) — `:5543-5581`.
4. Only then calls the daemon's `PlaceOrder` — `:5584`.
5. Writes a local escrow record (`upsertBuyEscrowRecord` `:1316`, persisted
   through `putEscrowRecordDurable` with a synchronous table flush,
   `marketEscrowState.js:105`) and notifies the client — `:5623-5637`.

So the ISK is simply **gone from the wallet** and remembered in two places: the
`marketEscrow` GameStore record, and the journal rows. On failure the `catch` at
`:5638` cancels the daemon order, drops the escrow record, and issues three
compensating credits with the same ref types (`:5646-5684`).

### How a sell order holds items

By **moving the items to a virtual location**. `executeSellEntryUnlocked`
(`marketProxyService.js:5724`) computes
`escrowLocationID = MARKET_ESCROW_LOCATION_BASE + order_id` (`:5920-5921`, base
`9_200_000_000` at `inventory/marketItemCustody.js:7`), then:

1. Writes the escrow record **first**, in state `"placing"` (`:5956-5959`).
   Their comment at `:5949-5955` records the bug this fixes: the custody move
   flushes the item table before returning, so writing the record afterwards
   left a crash window in which items sat at the virtual location with nothing
   naming them — invisible to reconciliation, expiry and cancel, i.e. lost.
2. Moves the stack with `marketItemCustody.transferItem(..., reason: MARKET_ESCROW_PLACE)`
   (`:5966-5975`).
3. Rewrites the record as `"active"` (`:5993-5997`).

The record deliberately separates **three identities** (`:5923-5928`):
`ownerId` (the listing character, for notifications and item custody),
`sourceOwnerId/sourceLocationId/sourceFlagId` (where goods return to on cancel
or expiry — possibly a corp hangar division), and `walletOwnerId` (who gets
paid, possibly a corporation). Conflating any two is a bug they call out
explicitly, and it bites on the buy side too: at `:4487-4492` the matcher reads
item custody from the **escrow record**, never from the daemon's
`order.owner_id`, because the latter can be a corporation.

### How a match is made

`matchSellOrdersForBuyer` (`:4344`) — and its mirror
`matchBuyOrdersForSeller` (`:5132`):

1. Pull the whole region book for the type in one RPC: `GetOrders` `:4372`.
2. Filter by structure access (`createStructureMarketRowAccessScope` `:1905`),
   skip orders with an in-flight saga (`hasPendingMarketFillSagaForOrder` `:4398`),
   skip orders priced above the limit using a 0.0001 ISK epsilon (`:4403`),
   skip out-of-range orders (`isSellOrderInRange` `:4416`).
3. Per matched order, `tradedQuantity = min(remaining, available)` (`:4426`).
4. Branch on `source === "seed"` (NPC) vs player — `:4432`. Four saga builders
   cover the four cases: `buildSeedSellFillSaga` `:3111`,
   `buildSeedBuyFillSaga` `:3472`, `buildPlayerSellFillSaga` `:3848`,
   `buildPlayerBuyFillSaga` `:4694`.
5. Loop until `remainingQuantity <= 0` (`:4394`), accumulating spend (`:4638`).

**The iteration order is whatever the daemon returned, and the Node matcher
never re-sorts.** The daemon does sort: sells `ORDER BY price ASC, station_id
ASC, order_id ASC` (`state.rs:2076`, seed sells `:2043`) and buys
`ORDER BY price DESC, station_id ASC, order_id ASC` (`:2150`, seed buys `:2117`).
So price priority holds, but the **second** key is `station_id`, not time —
`order_id` (their only time proxy) is the third. Two orders at the same price
are therefore filled in station-ID order, so a newer order at a
lower-numbered station beats an older one elsewhere. That is **not** EVE's
price-time priority. It only matters once two players quote the same price at
different stations in range, which is exactly what happens in a trade hub.

### Is it atomic? No — it is a compensating saga

This is the heart of the design and the part most likely to be subtly wrong.
`executePlayerSellFillSagaUnlocked` (`:4097`) runs eight steps, each with its own
idempotency key, each persisting the saga to durable storage after it succeeds:

| # | step | line | idempotency key | flag set |
|---|---|---|---|---|
| 1 | debit buyer `grossCost`, ref 2 (`MARKET_TRANSACTION`) | `:4118` | `<op>:wallet:buyer-debit` | `buyerDebited` |
| 2 | per-item custody move escrow → buyer hangar | `:4141` | `leg.forwardKey` | `leg.destinationItem` |
| 3 | credit seller `grossCost`, ref 2 | `:4162` | `<op>:wallet:seller-credit` | `sellerCredited` |
| 4 | debit seller sales tax, ref 54 (`TRANSACTION_TAX`) | `:4182` | `<op>:wallet:tax-debit` | `taxDebited` |
| 5 | credit buyer price-improvement refund, ref 42 | `:4202` | `<op>:wallet:price-improvement-refund` | `priceImprovementRefunded` |
| 6 | **daemon `FillOrder`** | `:4223` | `<op>:daemon-fill` | `daemonAttempted` |
| 7 | daemon `FillOrder` on the crossing order | `:4234` | `<op>:crossing-daemon-fill` | `crossingDaemonAttempted` |
| 8 | remove or update the escrow record | `:4269-4285` | — | — |

Note the ordering: **ISK and items move before the daemon is told the order was
filled.** Step 6 is last-but-two. If the process dies between steps 1 and 6, the
buyer has paid and holds the goods while the daemon still shows the order open
at full volume. Recovery is by replay: `recoverPendingPlayerSellFillSagas`
`:4330` walks every escrow record with a `pendingFill` on boot and re-runs the
saga, and each completed step is skipped by its flag. The daemon's own
`market_fill_receipts` table makes step 6 safely replayable
(`state.rs:2925-2949`); a mismatched replay errors with
`"idempotency receipt does not match fill request"`.

Two subtleties worth carrying:

* `marketEscrowState.js:37-43` documents a real failure they hit: the saga
  idempotency counter lives in the Node GameStore while receipts live in the
  daemon's SQLite. Restoring a GameStore backup while keeping the daemon
  database makes every key collide, and *every market trade fails* until the
  counter catches up. Their fix is a per-run UUID namespace
  (`runIdempotencyNamespace` `:44`, `randomUUID()` `:59`).
* `canCompensateRejectedPrimaryFill` `:2007` decides rollback vs retry, and it
  refuses to compensate once `fillResponse` exists but will compensate if the
  request provably never left the socket (`marketDaemonRequestSent === false`,
  `marketDaemonClient.js:25`). An outcome-unknown error schedules a durable
  recovery job instead (`queueDurableMarketFillRecovery` `:2350`).

Compensation itself (`compensatePlayerSellFillSaga` `:3971`) reverses the wallet
legs and moves items back, keyed off the same flags.

### Partial fill

Partial fill is first-class on both sides. The daemon computes
`next_remaining = vol_remaining - fill_quantity` and sets state to `"filled"`
only at zero (`state.rs:3206-3211`); over-fill is rejected outright
(`"fill quantity exceeds remaining order volume"`, `state.rs:3202`). A
non-`open` order returns a zero-quantity response rather than an error
(`state.rs:3177-3199`). On the Node side, a partially filled order keeps its
escrow record with `remainingQuantity` and `escrowAmount` rewritten
(`:4274-4285`, and for the crossing order `:4246-4259`); only a full fill
removes it (`:4269-4273`). Escrow items are consumed leg by leg, and
`buildPlayerSellFillSaga` refuses to start if the escrowed stacks do not cover
the traded quantity (`:3909-3913`).

### The journal

Every money movement goes through `debitCharacterWallet` `:1607` /
`creditCharacterWallet` `:1660`, which write a journal row carrying
`entryTypeID`, `ownerID1`, `ownerID2`, `referenceID` and an `idempotencyKey`.
The ref types used by the market are, from `walletState.js:44-82`:
`MARKET_TRANSACTION: 2`, `MARKET_ESCROW: 42`, `BROKERS_FEE: 46`,
`TRANSACTION_TAX: 54`, `MARKET_PROVIDER_TAX: 149`. Separately, a **market
transaction row** (a different table from the journal) is written for each side
via `recordMarketTransactionForWalletOwner` `:1175`, called twice per player
trade at `:4568` and `:4584`, each carrying the other side's
`journalRefID` so the client can cross-link. Journal writes are flushed at saga
boundaries (`flushCharacterWalletSagaBoundary` `:3072`).

### How the client is told

One notification does almost all the work: **`OnOwnOrdersChanged`**, sent per
session via `notifyOwnOrdersChanged` (`:998`, `session.sendNotification(...)`
`:1003`). Its payload is a list of order KeyVals built by
`buildOrderNotificationEntry` `:956` — 17 named fields matching
`OWNER_ORDER_HEADER` — plus a **reason string** and an `isCorp` flag. The reason
strings are `"Created"`, `"Modified"`, `"Cancelled"`, `"Filled"`,
`"PartialFill"`, `"Expiry"` (`:227-234`). A fill sends `"Filled"` or
`"PartialFill"` depending on the daemon's returned state (`:4612`), and it is
sent to **the seller**, keyed on `sellerCharacterID` (`:4601`).

Item delivery to the buyer uses a second notification,
**`OnMarketItemReceived`** (`notifyMarketItemReceived` `:1091`,
`session.sendNotification("OnMarketItemReceived", "charid", [itemID, typeID, flagID, locationID])`
`:1097-1102`), driven from `notifyBuyerDelivery` `:2910`.

One wrinkle for immediate (duration-0) buys: since no order persists, they
synthesise a throwaway order row to make the client's multibuy window behave.
`buildImmediateMarketRefreshOrder` `:1011` sends a fake order with
`order_id: 0` and state `"filled"`; `buildImmediateBuyOrder` `:1064` allocates
IDs from a clock-seeded range starting at `8e15` (`:1056`) to avoid colliding
with daemon IDs near 2.26e18, and their comment at `:1058-1063` cites the
client's own `buyMultiFromBase._PerformTrade` lines 296–311 and
`_ProccessExpiredOrders` 569–590 as the reason.

## Static data provenance

| static data | where theirs comes from | evidence | can we read it from the client instead? |
|---|---|---|---|
| Market **group tree** and each group's type list | nowhere — the server never sends it | `marketService.js:66-84` and `marketProxyService.js:6293` both return an **empty** `GetMarketGroups` rowset | **Yes, and we already proved it.** The client builds the tree from `res:/staticdata/marketGroups.fsdbinary` — `docs/reference/market-browse.md` §4, independently confirmed by EvEJS shipping an empty rowset and no complaints |
| Regions / systems / stations / constellations | `_local/gameStore/data`, produced by `tools/DatabaseCreator` from the installed client | `tools/market-seed/config/market-seed.example.toml:6` and `market-seederv3/config/...:12` both read `static_data_dir = "../../_local/gameStore/data"` | Yes — same origin, client FSD |
| Type metadata: `group_id`, `category_id`, `market_group_id`, `name`, `base_price`, `volume`, `portion_size`, `published` | same `_local/gameStore/data` | schema `crates/market-common/src/lib.rs:71-82` | Yes — client FSD |
| Seeded NPC stock + prices (v1) | **invented** from each type's SDE `basePrice` plus markup/jitter | `docker/market-seed.toml`: `seed_markup_percent = 8.0`, `station_jitter_percent = 1.5`, `price_floor = 100.0`, `fallback_base_price = 10000.0` | N/A — synthetic; we'd invent our own |
| Seeded stock + prices (v2) | **real Tranquility order snapshot, downloaded** | `tools/market-seederv2/src/main.rs:286` → `https://data.everef.net/market-orders/index.json`; README: "Downloads the real Tranquility market snapshot" | No — this is live-server data, not in the client |
| Seeded stock + prices (v3) | computed from **manufacturing cost** via blueprint trees, from a **530 KB price manifest committed to the repo**, plus a TQ snapshot overlay for the NPC catalog | `tools/market-seederv3/data/price-manifest.json` (530,538 bytes); endpoints `src/config.rs:854` (EVE Ref index) and `:874` (`https://esi.evetech.net/latest/markets/prices/`); README: "the price manifest is committed, so you can go straight from classify to cost and build with no network at all" | Partly — blueprint/material data is client FSD, but the committed manifest is CCP-derived captured prices |
| Price history (chart data) | v3 synthesises ~30 days of drift around the seeded price | `market-seederv3/src/history.rs` (846 lines); README: "a month of synthetic chart per seeded type"; v1: `history_days_seeded = 30` in `docker/market-seed.toml` | N/A — synthetic. The client has no history FSD; the server must supply it |
| **LP store offers** | a **9.3 MB cached dump of CCP's ESI loyalty endpoints** | `services/loyalty/lpStoreStaticOffers.json` header: `"sourceAuthority": "CCP ESI tranquility"`, `"generatedAt": "2026-07-05..."`, `"datasource": "tranquility"`, 198 candidate corps / 178 fetched, with per-corp ETags and `Expires` headers | **Probably not — needs checking.** Modern LP offers are server-side on TQ and I found no client FSD for them. Marked a **guess**: someone should grep the client's `res:/staticdata` for a loyalty/offers table before we conclude we must author our own |
| NES / PLEX store catalog | **operator-authored**, edited with their own GUI tool, stored in the game store | `storeState.js:14` `AUTHORITY_TABLE = "newEdenStore"`; `tools/NewEdenStoreEditor/` (`editor.js`, `build_seed_authority.js`) | N/A — not CCP data; we'd author our own |
| Ship insurance prices | derived from type `basePrice` by a fixed fraction | `insurancePriceAuthority.js:16` `BASE_INSURANCE_FRACTION = 0.4`, `:18` `INSURANCE_FRACTION_INCREMENT = 0.5`, table `shipInsurancePrices` `:15` | Yes — `basePrice` is client FSD |
| Killmail loss values | type `basePrice`, **not** market prices | `killmailPreparationTasks.js:159-161`, used `:275-280` | Yes — client FSD |

### The three seed engines, side by side

* **v1 — `tools/market-seed`** (2,529 Rust LOC). Fully offline. Reads client-derived
  static data, invents prices from `basePrice` with markup and jitter, seeds many
  stations, has presets ("Jita + New Caldari", "Full universe", "Quick smoke").
  **Needs no network. Ships no CCP data** beyond what the client already has.
* **v2 — `tools/market-seederv2`** (2,034 LOC). **Needs the network**: downloads the
  latest EVE Ref bzip2 market-order CSV. Station orders only — player-structure
  orders are dropped. Collapses many TQ orders per `(station, type, side)` into
  one top-of-book row, because `seed_stock`/`seed_buy_orders` are keyed
  `(station_id, type_id)` and are *not* per-order tables (README, "Normal Run").
  Writes **nothing** into `market_orders`, which stays player/escrow-backed.
  **Ships no data; fetches CCP-derived data at build time.**
* **v3 — `tools/market-seederv3`** (23,743 LOC, by far the largest — 3,361 in
  `build.rs`, 3,357 in `audit.rs`, 3,161 in `cost.rs`). Two layers: a Jita 4-4
  index priced at 3× build cost / bought back at 1×, computed bottom-up through
  blueprint trees from a committed price manifest; underneath, a TQ NPC catalog
  overlay (BPOs, skillbooks, PI command centers, trade goods). **Ships CCP-derived
  data** (`data/price-manifest.json`, 530 KB of captured prices). Needs the network
  only for `refresh-prices` (ESI) and `snapshot-info --download` (EVE Ref);
  `build` never downloads. Notably it **audits itself and refuses to install a
  build that would let players print ISK** (README step 6) — a real design idea
  worth stealing for any seeded market, since a 3×/1× spread with free hauling is
  an ISK faucet unless you check every arbitrage path.

Licensing note for the coordinator: AGPL covers their *code*. The committed
`price-manifest.json` and the 9.3 MB `lpStoreStaticOffers.json` are **CCP-derived
data**, not their code, and our former AGPL relicensing does not clear them. Treat both
as "do not vendor" until someone looks at it properly.

## Dependency order

Inside this area:

1. **Wallet + journal** before everything. Every other subsystem books against
   it; the market alone uses five ref types. *Our side already has this* —
   `server/crates/eve-server/src/ledger.rs`.
2. **Item custody / location model** before market *order placement*, and it is
   **outside this area**. A sell order is a custody move to a virtual location
   (`:5966`), and the whole escrow design depends on items having an owner, a
   location, a flag, a stack size, a singleton bit, and a *transfer operation with
   an idempotency key* (`marketItemCustody.transferItem` taking `idempotencyKey`,
   `:4150`). You cannot build sell orders, contracts, courier packages or LP-store
   payouts without it. This is the single largest external prerequisite.
3. **Market browse (availability dict)** before order books are useful — the
   client will not even display a type whose `typeID` is absent from
   `asksForMyRange` (`docs/reference/market-browse.md` §1). Browse needs only static data
   plus a summary index; it does **not** need custody or matching. This is why it
   is the right first step and why we already have it.
4. **Order listing + placement** needs wallet (fees) and custody (sell side).
5. **Matching + escrow** needs placement, custody, wallet *and* a station/system/
   region topology with jump distances for order ranges
   (`marketTopology.js:107` `getJumpCount`, `:223` `isSellOrderInRange`). Range
   is not cosmetic: a buy order at range Region must match sells across the
   region, which needs real adjacency (`ensureAdjacency` `marketTopology.js:23`).
6. **Skills** before correct fees — broker fee, sales tax and max order count all
   read skill levels (`marketRules.js:105-125`, eleven trade skills). Shippable
   with flat rates first; the client shows the fee before you confirm, and
   `validateExpectedBrokerFeePercentage` (`:1523`) *rejects the order* if the
   server's computed rate disagrees with the client's, so a wrong rate is a hard
   failure, not a cosmetic one.
7. **Standings** before exact broker fees (`marketRules.js:161-162`). Low priority.
8. **Corporations** before corp market orders, and **before contracts in any
   useful form**. Contracts need: corp wallets with account keys
   (`contractRuntimeState.js` wallet refs), corp hangar divisions and item flags
   (`CORP_CONTRACT_ITEM_FLAGS` `:127`), and corp role masks
   (`CORP_ROLE_CONTRACT_MANAGER = 72057594037927936n` `:62`). Personal-only
   contracts are possible as a subset; their own market code has a
   `assertPersonalMarketOnly` guard for exactly that staging
   (`marketProxyService.js:1742`).
9. **Insurance** needs the wallet, the item/ship model and a **ship-destruction
   hook** — and it does **not** need killmails. In EvEJS the payout fires from
   `space/shipDestruction.js:213` calling `insuranceRuntime.handleShipDestroyed`,
   and `grep insurance services/killmail/*.js` returns nothing. It also needs an
   inventory-mutation hook to void contracts when a ship is repackaged or moved
   (`itemStore.js:424-429` → `handleInventoryMutation`).
10. **NES needs PLEX, and PLEX is a third currency.** `walletState.js:88-93`
    gives every character a `plexBalance` (default 2222) alongside `balance`
    (ISK) and `aurBalance` (Aurum), with account keys `CASH: 1000` and `AURUM: 1200`
    (`:36-39`) and currency codes `ISK: 1`, `AURUM: 2` (`:84-87`). The NES
    debits `plexBalance` (`storePurchaseSettlementState.js:574`) and writes a
    separate PLEX transaction log (`plexVaultLogState`, `walletState.js:9-15`).
    So: NES → PLEX balance + PLEX log → wallet. It does *not* need the market
    (their NES is not a PLEX market).
11. **LP store needs loyalty-point balances per NPC corporation**, which come
    from missions/agents — outside this area. Our `wallet.rs:141` already stubs
    `LPSvc.GetAllMyCharacterWalletLPBalances`. It also needs custody for payout
    and the offer catalog (see provenance).
12. **Killmails are independent of all of the above** and should move to another
    lane. They need the item model and war/standing records
    (`killmailState.js:22-23` pulls `warRuntimeState`), and value losses from
    `basePrice`. Nothing in the market, wallet or insurance path reads them.

Cross-cutting: **structure (Upwell) market access** gates a surprising amount —
`ensureStructureMarketServiceAccess` `:1887`, and cancelling all a structure's
orders when its market service goes offline `:2163`. Skippable for NPC stations
only, but then `getMarketCounterpartyOwnerID` `:1129` must fall back to the
station's `corporationID`.

## Effort to port, per subsystem

| subsystem | effort | why | risk |
|---|---|---|---|
| Market browse availability dict | **S** | One summary index over seeded stock; we already answer the three calls and already know the exact required shapes. The daemon's `region_summaries` is a good model | The station/system forms need a **4-tuple** `(price, volRemaining, typeID, stationID)` while region needs a **named-field object**. EvEJS's code independently confirms this split (`marketProxyService.js:6338,:6378` tuple vs `:6413-6418` named) — matching `docs/reference/market-browse.md` §4. Get it backwards and the client raises `TypeError` the first time a player touches the Range radio |
| Order book + per-type price history | **S–M** | Flat rowsets; history is synthesisable. Our `empty_order_book()` already has the header | `jumps` is position-dependent (`buildOrderRowset` options `currentStationID`, `:389`), so any cached response must be scoped per solar system or players see wrong distances |
| Price history generator | **S** | 30 days of drift around the seed price; theirs is 846 LOC but conceptually trivial | Low |
| Seeded NPC stock (our own v1-equivalent) | **M** | Read client FSD `basePrice`, apply markup, emit per-station stock. No network, no CCP data | An unbounded 3×/1× spread is an ISK faucet. Steal v3's audit idea (`market-seederv3/src/audit.rs`), not its 3,357 lines |
| Order placement + fee model | **M** | Validation, fees, journal rows. Rules are 340 lines and readable | `validateExpectedBrokerFeePercentage` (`:1523`): the server must agree with the client's *own* computed fee to within `BROKER_RATE_EPSILON = 0.000001` (`:255`) or the order is rejected. The client computes it from skills + standings, so a flat-rate server silently fails to place orders for any character with Broker Relations trained |
| **Order matching + escrow** | **L–XL** | The real work. ~2,400 LOC of saga logic that we must *redesign*, not port: in one process the eight-step saga collapses to one `&mut` transaction over the book, the wallet and item custody. Simpler than theirs, but it is a from-scratch design | Three ways to be silently wrong: (a) **custody identity** — if we key item ownership off the order's owner rather than a separate escrow record, corp orders deliver goods to the wrong place (`:4487-4492`); (b) **partial fill** — escrow quantity and order remainder must be updated in the same step or escrow leaks; (c) **crash atomicity** — their durable-flush-per-step design exists because their boundary is non-atomic. If ours is atomic we can drop sagas entirely, but we must then make the persistence boundary atomic too, or we reintroduce the same hole with none of their recovery machinery |
| Order expiry + escrow return | **M** | A durable timer per order plus "return goods to `sourceLocationId`/`sourceFlagId`" | Their `sweepStrandedMarketEscrowItems` `:2618` and `resolveStrandedEscrowOwnerHomeStationID` `:2562` exist because escrow items *do* get stranded. Needs a boot-time reconciliation pass from day one, not later |
| Market transaction log (wallet tab) | **S** | A second table beside the journal, two rows per trade | `MAX_MARKET_TRANSACTION_ENTRIES = 2000` (`walletState.js:95`) — a cap, not a feature. Each side's row carries the *other* side's `journalRefID` (`:4579,:4595`) |
| Insurance | **M** | 2,000 LOC but self-contained: price from `basePrice × fraction`, a contract table, a payout on destruction | Needs a ship-destruction hook and an inventory-mutation hook to void on repackage (`itemStore.js:424`). `STATION_INSURANCE_SERVICE_MASK = 1048576` (`insuranceRuntime.js:78`) is a station-service bitmask that must match the client's FSD |
| Player trade window | **M** | 2,411 LOC, two-party offer/accept with ISK + items. Independent of the market | Classic double-accept race; both sides' items and ISK must move in one step. Books ref type 1 (`PLAYER_TRADING`) |
| Contracts: item exchange | **L** | Needs custody, corp wallets, a searchable public list, and plastic-wrap packaging | `PLASTIC_WRAP_TYPE_ID = 3468` (`contractRuntimeState.js:59`) and `CONTRACT_ESCROW_LOCATION_BASE = 9_300_000_000` (`:57`) are build-sensitive. `CONTRACT_ID_START = 980000000` is an arbitrary ID floor that must not collide with ours |
| Contracts: courier | **L** | Collateral escrow, a wrapped package item that travels, and completion at a destination station | Collateral is real ISK held across an arbitrary time span; failure/expiry must pay it out correctly or players lose money permanently |
| Contracts: auction | **XL / skip** | Theirs is a stub (`PlaceBid` → `null`, `_other/contractProxyService.js:871`). We would be building it from nothing | Nothing to port. Skip |
| LP store | **M, blocked on data** | The code is small (424 + 782 LOC); the hard part is the offer catalog | **Blocked**: theirs is a 9.3 MB CCP ESI dump we should not vendor. Also needs per-corp LP balances from missions. Do not start until someone checks whether the client ships a loyalty-offers FSD table |
| NES / PLEX store | **M** | Catalog is operator-authored (good — no data problem), plus a PLEX balance and log | Introduces a **third currency** with its own account key and transaction log. Low value for a co-op/PvP server with friends; the end goal does not obviously need a cash shop |
| EverMarks | **S / skip** | A fourth currency with its own catalog, ~2,900 LOC | No gameplay value for our end goal. Skip |
| Killmails | **M, different lane** | Independent of the economy; values from `basePrice` | Does not belong here. Reassign |

## Build-delta hazards

Their target is client build **3396210**; ours is **3619879**. Everything below
is a constant read off their 3396210 behaviour and must be re-verified against
our client's FSD rather than copied.

**Type IDs.** `PLEX_TYPE_ID = 44992` (`marketProxyService.js:221`);
`PLASTIC_WRAP_TYPE_ID = 3468` (`contractRuntimeState.js:59`); eleven trade skill
type IDs — Trade 3443, Retail 3444, Broker Relations 3446, Visibility 3447,
Procurement 16594, Daytrading 16595, Wholesale 16596, Margin Trading 16597,
Marketing 16598, Accounting 16622, Tycoon 18580 (`marketRules.js:29-39`).

**Location IDs.** Zarzakh solar system `30100000` and pirate faction IDs
`500018`, `500019` (`marketRules.js:52,133`) — a 50% tax discount hardcoded to
one system. Daemon bench defaults region `10000002` (The Forge), system
`30000142` (Jita), station `60003760` (Jita 4-4) — `main.rs:50-54`; v3 seeds
Jita 4-4 exclusively.

**Synthetic ID ranges — collision hazards.** `SEED_SELL_ORDER_ID_BASE = 2e18`,
`SEED_BUY_ORDER_ID_BASE = 4e18` (`crates/market-common/src/lib.rs:40-41`);
market escrow virtual locations `9_200_000_000` to `9_300_000_000`
(`inventory/marketItemCustody.js:7,12`); contract escrow locations from
`9_300_000_000` (`contractRuntimeState.js:57`) — **immediately adjacent to the
market range, with no gap**; contract IDs from `980000000`
(`contractRuntimeState.js:56`); immediate-buy order IDs from `8e15` seeded off
the wall clock (`marketProxyService.js:1056`). If we adopt any of these we
inherit the adjacency. Pick our own ranges with real gaps.

**Order range sentinels.** `RANGE_STATION = -1`, `RANGE_SOLAR_SYSTEM = 0`,
`RANGE_CONSTELLATION = 4`, `RANGE_REGION = 32767`
(`marketTopology.js:7-10`). `32767` also appears as the `range_value` default in
the daemon schema (`lib.rs:127`). These are client protocol constants.

**Durations.** Allowed order durations `{0, 1, 3, 7, 14, 30, 90}`
(`marketRules.js:53`) — the client's dropdown; a value outside the set is
rejected. Insurance runs 12 weeks (`insuranceRuntime.js:77`).

**Tax and fee constants.** Broker commission 3.0%, SCC commission 0.5%,
transaction tax 8.0%, minimum broker fee 100 ISK, minimum SCC surcharge 25 ISK,
max order price 9,223,372,036,854.0 (`marketRules.js:41-46`); Broker Relations
modifier 0.3, Accounting modifier 0.11, faction standing multiplier 0.0003, NPC
corp standing multiplier 0.0002 (`:47-50`); modification fee discount
`0.5 + 0.06 × Margin Trading` (`:154`); structure market tax fallback 1.0% and
max 11.5% (`marketProxyService.js:145-146`); insurance base fraction 0.4 with
0.5 increments (`insurancePriceAuthority.js:16,18`); daemon price floor 100.0
(`lib.rs:39`). **All of these are balance values CCP has changed across builds.**
The broker fee one is load-bearing, not cosmetic — see the
`validateExpectedBrokerFeePercentage` risk above.

**Wallet key numbers.** Account keys `CASH: 1000`, `AURUM: 1200`
(`walletState.js:36-39`); the daemon defaults `wallet_division` to `1000`
(`lib.rs:116`); currency codes `ISK: 1`, `AURUM: 2` (`:84-87`); journal cap
`MAX_JOURNAL_ENTRIES = 100` (`:94`), transaction cap 2000 (`:95`).

**Journal ref type IDs.** 30 constants at `walletState.js:44-82`. The
market-relevant ones are 2, 42, 46, 54, 149; contracts and insurance add 1, 10,
19, 41. Our `ledger.rs:43` has `REF_GM_CASH_TRANSFER = 3` and
`ENTRY_TYPE_NAME_ID_BASE = 59_792` with a documented hard ceiling at entry type
**120** (`ledger.rs:61`). **`MARKET_PROVIDER_TAX = 149` is above that
ceiling** — the `59792 + id` name mapping does not reach it, so an SCC surcharge
row will render with a **blank Type column** unless we read the real mapping out
of `accountingentrytypes.fsdbinary`. That is a concrete, already-documented trap
waiting for the first market order we place.

**Corp role masks.** Trader `18014398509481984n`
(`marketProxyService.js:226`), Contract Manager `72057594037927936n`
(`contractRuntimeState.js:62`), Accountant `256n` / Junior Accountant
`4503599627370496n` (`insuranceRuntime.js:79-80`). Bit positions in a client
enum.

**Station service bitmask.** `STATION_INSURANCE_SERVICE_MASK = 1048576`
(`insuranceRuntime.js:78`).

**Daemon-internal, build-independent but version-coupled.**
`MARKET_SCHEMA_VERSION = 1` (`lib.rs:33`), ports 40110/40111, and the
capability string `idempotent-fill-v1` the Node client hard-requires
(`marketDaemonClient.js:9`). Irrelevant if we do not take the daemon.

## Notes for the coordinator

**Surprises.**

1. **The daemon's tests are the only tests in the release.** 15 `#[test]`
   functions, all inside `externalservices/market-server`, all about SQLite
   transaction behaviour. Zero JS tests anywhere in `server/`. Read that as a
   signal: the authors found concurrent market writes hard enough to warrant
   tests and found nothing else worth testing. The Node-side escrow sagas — the
   part most likely to lose a player's ISK — are completely unverified.
2. **The daemon has no matching engine.** I expected to find one. `fill_order_in_db`
   (`state.rs:3004`) decrements a named order and nothing more. All matching is
   Node's. This means the daemon is much less interesting than its existence
   suggests, and the split is about *read throughput against 445k rows*, not
   about exchange semantics.
3. **The `escrow` column is dead.** Always written as literal `0`
   (`state.rs:2642`). Real escrow is a Node-side wallet debit plus a virtual item
   location. Anyone skimming the daemon schema would conclude the opposite.
4. **ISK and items move before the daemon is told.** Step 6 of 8
   (`marketProxyService.js:4223`). Every crash window in between is covered only
   by replay-on-boot. This is the direct cost of the two-process split and the
   strongest argument for our single-binary design.
5. **Their market code independently confirms our `docs/reference/market-browse.md`.**
   Tuples for `GetStationAsks`/`GetSystemAsks`, named-field objects for
   `GetRegionBest` (`marketProxyService.js:6338,:6378` vs `:6413-6418`). Two
   codebases, reverse-engineered separately, same asymmetry. That raises my
   confidence in our browse doc considerably.
6. **Killmails do not belong in this area.** They value losses from static
   `basePrice` (`killmailPreparationTasks.js:159`) and have no reference to
   insurance or any market module. Insurance payout rides
   `space/shipDestruction.js:213`. Reassign killmails to the combat lane; the
   only thing the economy needs from them is nothing.
7. **The LP store is a 9.3 MB CCP ESI dump.** `"sourceAuthority": "CCP ESI
   tranquility"`, with per-corp ETags and HTTP `Expires` headers preserved in
   the file. Our former AGPL relicensing covers their code, not CCP's data. Same for
   v3's 530 KB committed price manifest.
8. **Contract auctions are a stub** despite 4,168 lines of contract runtime.
   Item exchange and courier are real.
9. **Their fill order is price-then-station, not price-then-time.** The
   daemon's book sorts `price ASC, station_id ASC, order_id ASC`
   (`state.rs:2076`). At equal price, the lower station ID wins regardless of
   who listed first. That is a genuine behaviour difference from EVE and the
   kind of thing nobody notices until two players undercut each other in a hub.
   Cheap for us to get right: sort by `(price, issued_at, order_id)`.
10. **They ship both a stub and a real implementation of market and contracts**
   (`marketService.js` / `marketProxyService.js`, `contractMgrService.js` /
   `contractProxyService.js`), auto-registered by filename
   (`bootstrap.js:226` matches `*Service.js`). The stubs answer the older
   `market`/`contractMgr` service names and exist so the client does not throw
   when the daemon is absent.

**Unknowns I did not resolve.**

* Whether our client (3619879) ships any FSD table for LP store offers. This
  decides whether the LP store is a port or a from-scratch content problem.
* `matchBuyOrdersForSeller` (`:5132`, ~320 LOC) and the "crossing order after
  modify" paths (`:6149`, `:6221`) I read only structurally. If we implement
  order modification, those are where price-improvement refunds and re-matching
  live, and they look like the fiddliest code in the file.
* ~~The real mapping for journal ref types above 120, needed for
  `MARKET_PROVIDER_TAX = 149`.~~ **Settled** (commit `3690328`). Read from
  `accountingentrytypes.fsdbinary`'s own two id columns, 218 rows, now a
  sorted table in `ledger.rs`. The arithmetic `59792 + id` holds for
  `0..=108` minus `{100, 104, 105}` and nowhere else; the cap of 120 we had
  was not conservatively blanking, it was printing twelve real names against
  the wrong entry types. 149 is in the table.

**What to look at next, in order.**

1. Decide the **escrow model** before writing any matching code. Theirs
   (debit-and-remember + virtual item location) is sound and survives crashes;
   it is also the one design decision that everything else hangs off. Our
   version should keep the three-identity separation
   (`marketProxyService.js:5923-5928`) and drop the sagas.
2. Fix the **ref type 149 name-id gap** in `ledger.rs` before the first market
   order, or SCC surcharge rows render blank.
3. Build the **availability summary index** next — it is small, it unblocks a
   visibly working market window, and it is the one piece of their architecture
   that transfers directly.
4. Scope **item custody** with whoever owns the inventory lane. Market sell
   orders, contracts and LP payouts all need a transfer primitive with an
   idempotency key and an explicit expected-source check
   (`marketItemCustody.transferItem({ item, expectedSource, ... })`, `:5966`).
   This is the real critical path, and it is outside this area.
5. Leave contracts, LP, NES and EverMarks alone for now. Contracts are a week
   each and need corporations; LP is data-blocked; NES and EverMarks are a cash
   shop our end goal does not need.
