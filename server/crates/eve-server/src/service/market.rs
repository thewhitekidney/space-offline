//! The market: orders, the browse tree, and the ticker's price history.
//!
//! Carved out of `wallet` when the market became a subsystem with work of its
//! own rather than two calls on the side of the wallet
//! (`docs/archive/plan-docked-subsystems.md`). The split is also what lets a market
//! lane and a wallet lane run in the same round without contending for one
//! file.
//!
//! Entirely machoNet: `sm.ProxySvc` is an ordinary `ConnectToRemoteService`
//! addressed at the proxy node rather than `Any`, and nothing on this path
//! imports a protobuf messenger -- so unlike the PLEX balance, every answer
//! here is ours to give.

use super::*;
use crate::econ_exchange as exchange;
use crate::pyutil::{adler32, cached_bytes};

mod corp;
use corp::Wallet;

pub(crate) const SERVICES: &[&str] = &["marketProxy"];

pub(crate) fn dispatch(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.service {
        // The market window. Entirely machoNet -- `sm.ProxySvc` is an
        // ordinary `ConnectToRemoteService` addressed at the proxy node
        // rather than `Any`, and nothing on this path imports a protobuf
        // messenger.
        // The book first: every method that reads or writes orders is
        // answered by `book_call`, below the readers, and returns `None` for
        // the rest so this `match` keeps them.
        "marketProxy" => match book_call(reg, req) {
            Some(answer) => answer,
            None => other_market_call(reg, req),
        },
        // Unreachable: `SERVICES` above gates the call. Spelled out rather
        // than `unreachable!()` so a drift between the two is an ordinary
        // error and not a panic in front of a player.
        other => Err(ServiceError::NoSuchService(other.to_owned())),
    }
}

/// The `marketProxy` methods that do not touch the order book.
fn other_market_call(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    match req.method {
        // Price history for the ticker. See `empty_price_histories`
        // for why every requested id needs an entry and why the
        // rowsets must be CRowsets.
        // CP42: the caller's region's history, `price_histories`.
        "GetHistoryForManyTypeIDs" => Ok(price_histories(
            reg,
            history_region(reg, req),
            &type_ids_of(req.args.first()),
            crate::pyutil::filetime_now(),
        )),
        // Fires immediately after the one above, unguarded, because
        // PLEX is in the ticker's default list and is split out of
        // the set before the call. Same shape, one key.
        "GetPlexHistory" => Ok(empty_price_histories(&[TYPE_PLEX])),
        // The **uncached** price-history path, which is the one the
        // market window's own chart takes:
        // `pricehistorychart.RenderTable` (pricehistorychart.py:159)
        // and `GetGraph` (:206) call `marketQuote.GetPriceHistory(typeID)`
        // directly, not `GetPriceHistoryFromCache`, so selecting a type
        // reaches these even though the ticker already filled the cache
        // for its own 31 types. `marketsvc.GetPriceHistory`
        // (marketsvc.py:378) is
        //
        // ```text
        // if typeID == typePlex:                                      # 379
        //     old = self.GetMarketProxy().GetPlexOldPriceHistory()     # 380
        //     new = self.GetMarketProxy().GetPlexNewPriceHistory()     # 381
        // else:
        //     old = self.GetMarketProxy().GetOldPriceHistory(typeID)   # 383
        //     new = self.GetMarketProxy().GetNewPriceHistory(typeID)   # 384
        // history = self.GetHistoryRowList(old, new)                   # 385
        // ```
        //
        // and `GetHistoryRowList`'s (marketsvc.py:388) *first* statement is
        // `dbutil.RowList((new.header, []))`, so `None` is
        // `AttributeError: 'NoneType' object has no attribute 'header'`
        // before anything else runs. Each half is therefore one
        // **CRowset** over the same six columns as the halves inside
        // `empty_price_histories`, whose comment carries the rest of
        // the reasoning: `RowsInit` rejects a header that is not a real
        // `blue.DBRowDescriptor`, and two empty halves make
        // `GetHistoryRowList` append exactly one synthetic row, which
        // `TickerItem.LoadData` then drops at its own `len < 7` guard
        // instead of dividing by the `lastPrice = 0.0` it would have
        // padded with.
        //
        // **None of these four has a row in `findings/client-calls.tsv`**
        // -- the two non-PLEX ones for the same reason `GetOrders` has
        // none; see the note on that arm.
        //
        // CP42: the two non-PLEX halves carry the caller's region's
        // history (`history_halves`); PLEX has none, for the
        // `GetPlexOrders` reason.
        "GetOldPriceHistory" | "GetNewPriceHistory" => {
            let type_id = arg_i64(req.args.first()).unwrap_or(0);
            let now = crate::pyutil::filetime_now();
            let (old, new) = history_halves(reg, history_region(reg, req), type_id, now);
            Ok(if req.method == "GetOldPriceHistory" {
                old
            } else {
                new
            })
        }
        "GetPlexOldPriceHistory" | "GetPlexNewPriceHistory" => Ok(empty_price_history()),
        // **The order book**: issue #2, `marketProxy GetOrders(17682)
        // -> None`, and 0.1 s later
        //
        // ```text
        // TypeError: 'NoneType' object is not subscriptable
        //   marketsvc.py(863) RefreshOrderCache
        //   marketsvc.py(785) GetOrders
        // ```
        //
        // `RefreshOrderCache` (marketsvc.py:846) reads our answer twice
        // by index and then twice through `len()`:
        //
        // ```text
        // orders = self.GetMarketProxy().GetOrders(typeID)   # line 856
        // ...
        // asks, bids = orders[0], orders[1]                  # line 863  <- the traceback
        // self.orderCache[typeID] = [asks, bids, version]    # line 864
        // self.LogInfo(..., len(orders[0]), ..., len(orders[1]), ...)  # line 866
        // ```
        //
        // so **a 2-sequence**. The arity is pinned by `BINARY_SUBSCR 0`
        // and `BINARY_SUBSCR 1` rather than by `UNPACK_SEQUENCE` -- the
        // compiler turns `a, b = x[0], x[1]` into two subscripts and a
        // `ROT_TWO` -- which only changes which exception a short
        // answer raises, not whether it raises. A tuple is the narrowest
        // honest container: the client reads exactly two fixed indices,
        // index 0 is the asks and index 1 is the bids.
        //
        // **Each half must be a `CRowset`, and `[]` is not enough.**
        // `GetOrders` calls `RefreshJumps` on the next line
        // (marketsvc.py:786), and `RefreshJumps` ends with
        //
        // ```text
        // asks.sort((lambda x, y, headers=asks.columns: SortAsks(x, y, headers)))   # line 825
        // bids.sort((lambda x, y, headers=bids.columns: SortBids(x, y, headers)))   # line 826
        // ```
        //
        // The default argument is evaluated when the lambda is *built*,
        // so `.columns` is read even for an empty half, while `.sort`
        // with a comparison function needs a real `list`. Exactly one
        // family satisfies both: `CRowset`, which subclasses `list` and
        // exposes `columns` as `self.header.Keys()`. The old-style
        // `Rowset` has `header`/`lines` and **no** `columns`, and a
        // plain list has no `columns` either -- both are
        // `AttributeError` at line 825, after the cache has already
        // been written at line 864, so the symptom would have moved
        // rather than gone.
        //
        // Empty is the honest answer: nobody has placed an order,
        // because we do not implement placing one (see the mutators
        // below). An empty `CRowset` is **falsy**, which is safe here
        // only because nothing guards this call -- `RefreshOrderCache`
        // is called unconditionally and the store at line 864 happens
        // before any read of the contents. Both halves share one
        // descriptor because `SortAsks` indexes rows positionally off
        // one `columns` list.
        //
        // **This call has no row in `findings/client-calls.tsv`**, and
        // the reason is a third census blind spot beyond the two that
        // file documents: when two remote calls sit in the two arms of
        // one `if`/`else` and store into the **same local**, only the
        // first survives. `GetPlexOrders` (line 853) is recorded and
        // `GetOrders` (line 856) is not; `GetPlexOldPriceHistory` /
        // `GetPlexNewPriceHistory` are recorded and
        // `GetOldPriceHistory` / `GetNewPriceHistory` are not. Three
        // missing methods in one file, and one of them was the reported
        // bug -- so the census is a worklist, not an inventory.
        //
        // **`GetOrders` itself is now `book_call`'s**, wrapped in a
        // `CachedMethodCallResult` so the cache above can ever move; see
        // `cached_bytes` (via `get_orders`, FX8). What stays here is
        // `GetPlexOrders`, which `RefreshOrderCache` calls *instead* for
        // `typePlex` with the same two subscripts. **[A]** that it stays empty: PLEX lives in the
        // vault, not in the item store, so no player can post a PLEX
        // order here and the empty book is the truth.
        "GetPlexOrders" => Ok(empty_order_book()),
        // `GetRegionBest`, `GetStationAsks` and `GetSystemAsks` are now
        // `book_call`'s, answered from the book (`ask_keyvals`,
        // `ask_tuples`). `GetPlexBest` stays a dict, empty for the
        // reason `GetPlexOrders` is: it needs `in` and `[]` --
        // `FilterItemsForGroupPage` (marketbase.py:2691) does
        //
        // ```text
        // plexAsks = self._GetPlexAsks()                       # 2717
        // if typePlex in plexAsks:                              # 2718
        //     self.asksForMyRange[typePlex] = plexAsks[typePlex] # 2719
        // ```
        //
        // so `None` is `TypeError: argument of type 'NoneType' is not
        // iterable`.
        "GetPlexBest" => Ok(PyValue::Dict(vec![])),
        // The corp form of `CharGetTransactions`, which `book_call`
        // answers with rows. Each row is handed to `utillib.KeyVal(row)`,
        // which accepts a dict, a `blue.DBRow` or another KeyVal and
        // raises `TypeError` on anything else -- so this is a CRowset for
        // the same reason the wallet journal is.
        // `(fromDate, accountKey)`; `None` for the key is every division
        // the caller may read (`corp::transactions`).
        "CorpGetTransactions" => Ok(corp::transactions(
            reg,
            req.actor.unwrap_or(0),
            arg_i64(req.args.get(1)),
            arg_i64(req.args.first()),
        )),
        // **Never raise from this one**, however tempting the
        // `except UserError` around it looks. `MarketUtils.StartupCheck`
        // caches a raised UserError in `reg.regionUpAt` and re-raises
        // it for two minutes *without touching the wire*, which also
        // disables Sell Item from the inventory menu. The happy path
        // binds nothing and discards the result, so None is honest.
        "StartupCheck" => Ok(PyValue::None),
        // ---- the PLEX mutators: answered, deliberately **not
        // implemented**, for the `GetPlexOrders` reason above. The rest
        // of the mutators are `book_call`'s.
        //
        // `ModifyPlexCharOrder` discards its return (marketsvc.py:338)
        // and cannot be reached without a PLEX order to modify.
        // `PlacePlexSellOrder` is read only for truth --
        // `if wereOrdersCreated: self.Close()` (sellMulti.py:509) -- so
        // `None` honestly leaves the Sell window open: nothing was
        // created. There is no store-back behind that `if`, so this is a
        // precondition and not a latch.
        "ModifyPlexCharOrder" | "PlacePlexSellOrder" => Ok(PyValue::None),
        _ => Err(reg.no_method(req)),
    }
}

/// The region whose price history a caller reads: where they browse, as for
/// `GetOrders`. `None` without a character.
fn history_region(reg: &Registry, req: &Request<'_>) -> Option<i64> {
    req.actor
        .and_then(|a| browse_place(reg, a))
        .map(|h| h.region_id)
}

/// The columns of one market-order row.
///
/// **The declared order is free; the set of names is not.** Every read in
/// the client goes through the name: `marketsvc.SortAsks` and `SortBids`
/// (marketsvc.py:36 and :63) take `headers = asks.columns` and then
/// `headers.index('price')`, and everything else reads attributes. So the
/// order below is only for a human, while a missing name is a
/// `ValueError: 'price' is not in list` inside the sort, or an
/// `AttributeError`, the moment a row exists.
///
/// Where each name is forced from:
///
/// ```text
/// price, issueDate, jumps, orderID  SortAsks       marketsvc.py:38,44,50,56  headers.index(..)
/// range                             SortBids       marketsvc.py:77  x[jmp] - max(0, x[rng])
/// solarSystemID                     RefreshJumps   marketsvc.py:821  row.solarSystemID
/// typeID, stationID, price,
///   volRemaining, duration,
///   issueDate, jumps, range         marketbase.Reload   marketbase.py:3791-3799
/// volEntered                        quote.GetQuantitySlashVolume  quote.py:576
/// minVolume                         quote.GetMinVolume            quote.py:581
/// regionID                          quote.GetRegion               quote.py:593
/// bid                               orders.PopulateScroll         orders.py:244
/// ```
///
/// Every line number in this file is decoded from `co_lnotab` with
/// `tools/pyc27.py`, never read off uncompyle6's output, which numbers its
/// own reconstruction (`docs/guides/debugging-the-client.md` §4, "Tooling traps").
///
/// `jumps` is the one that cannot be left out on the grounds that the server
/// does not know it, because the client **writes** it:
///
/// ```text
/// for row in itertools.chain(asks, bids):
///     row.jumps = getJumpCountFunc(row.solarSystemID)   # marketsvc.py:821
/// ```
///
/// and a `blue.DBRow` only accepts an attribute its descriptor declares.
/// That the real header carries a `jumps` column the server fills with a
/// placeholder is **inferred** from that line, not read off a wire capture:
/// we have never seen a non-empty order book from CCP's server.
///
/// The widths are **inferred** too, and nothing observes them while the
/// rowsets are empty -- a descriptor with no rows packs no values. Two are
/// worth stating because they are the ones that will bite: `orderID` and
/// `stationID` are `I8`, not `I4`, because modern order ids exceed 2^31 and
/// because `stationID` carries an Upwell **structure** id when the order is
/// at a structure -- which is the whole point of
/// `MarketQuote.SkipBidDueToStructureRestrictions` (marketsvc.py:530)
/// testing `idCheckers.IsStation(bid.stationID)`. `volRemaining` is a
/// double because the client writes `int(record.volRemaining)` when it
/// wants an integer out of it (`quote.GetQuantity`).
///
/// Two names are **deliberately absent**. `quote.GetIssuedBy`
/// (quote.py:631) reads `record.charID` and `quote.GetWalletDivision`
/// (quote.py:521) reads `record.keyID`, but both are only wired to the
/// `headerIssuedBy` / `headerWalletDivision` columns that
/// `corpMarketOrders.GetBuyHeaders` (corpMarketOrders.py:10) adds -- they belong to the
/// *own-orders* rowsets (`GetCharOrders`, `GetCorporationOrders`), not to a
/// public order book, where EVE has never told a buyer who placed an ask.
/// If a row ever turns up in a capture with either column, this is the
/// comment that was wrong.
fn market_order_descriptor() -> DbRowDescriptor {
    let columns = [
        ("orderID", DbType::I8),
        ("typeID", DbType::I4),
        ("regionID", DbType::I4),
        ("solarSystemID", DbType::I4),
        ("stationID", DbType::I8),
        ("price", DbType::Cy),
        ("volEntered", DbType::I4),
        ("volRemaining", DbType::R5),
        ("minVolume", DbType::I4),
        ("bid", DbType::Bool),
        ("issueDate", DbType::FileTime),
        ("duration", DbType::I4),
        ("range", DbType::I4),
        ("jumps", DbType::I4),
    ]
    .into_iter()
    .map(|(name, db_type)| Column {
        name: name.as_bytes().to_vec(),
        db_type,
    })
    .collect();
    DbRowDescriptor::new(columns).expect("the market-order columns are a valid layout")
}

/// `(asks, bids)`, both empty: the answer to `GetOrders` and `GetPlexOrders`.
///
/// Two `CRowset`s in a 2-tuple. The arm above carries the reasoning; the
/// short version is that `RefreshOrderCache` reads index 0 and index 1 and
/// takes `len()` of each (marketsvc.py:863, :866), and `RefreshJumps` then
/// reads `.columns` off each and calls `.sort(cmp)` on it
/// (marketsvc.py:825) -- `list` plus `columns`, which is `CRowset` and
/// nothing else.
fn empty_order_book() -> PyValue {
    let descriptor = market_order_descriptor();
    PyValue::Tuple(vec![
        // Index 0 is the asks: `asks, bids = orders[0], orders[1]`.
        eve_rowset::CRowset::new(descriptor.clone()).to_pyvalue(),
        eve_rowset::CRowset::new(descriptor).to_pyvalue(),
    ])
}

/// One empty price-history half, as a `CRowset`.
///
/// Shares `price_history_descriptor` with `empty_price_histories` on
/// purpose: `GetHistoryRowList` takes `new.header` and appends rows of
/// `old` into the `RowList` it builds from it, and `RowList.append` rejects
/// a row whose `__columns__` differ from the list's. Two halves built from
/// two different descriptors would pass every shape test here and raise in
/// the client.
fn empty_price_history() -> PyValue {
    eve_rowset::CRowset::new(price_history_descriptor()).to_pyvalue()
}

// ===========================================================================
// The order book: P5.1, the mutators and `OnOwnOrdersChanged`.
//
// Everything below this line is the state the readers above were answering
// "empty" for. `docs/lanes/market.md` §"P5.1" has the evidence table; the
// comments here carry the per-decision reasoning and every `[A]`.
//
// **What "buy from the market" means here**: from a player's sell order (your
// own included: nothing in the client refuses self-trade), or from an **NPC
// order** (CP35, C5 §4): `crate::econ_exchange`'s, computed on read, never a
// row here. Every reader merges them (`side_with_npc`); a fill on one mints
// (ask) or destroys (bid) the goods with no escrow, and its ISK leaves or
// enters the economy (`move_isk` toward a corporation is a no-op).
// ===========================================================================

/// `marketutil/const.py` lines 3-6, read off `LOAD_CONST`/`STORE_NAME`
/// pairs: **[V]**. `rangeConstellation` (4) is not used by any range combo
/// we have read, and is treated as a jump count like any other.
const RANGE_STATION: i64 = -1;
const RANGE_SOLAR_SYSTEM: i64 = 0;
const RANGE_REGION: i64 = 32_767;

/// `marketutil/const.py:18`, **[V]**. The client checks it before every
/// mutator (`marketsvc.py:303`, `:331`, `:336`); checked again here because
/// the GM console and a bot can call us without the client's guard.
const MAX_ORDER_PRICE: f64 = 9_223_372_036_854.0;

/// A price the market can take: positive, finite, and under the cap.
fn valid_price(price: f64) -> bool {
    price.is_finite() && price > 0.0 && price <= MAX_ORDER_PRICE
}

/// `marketutil/const.py:24-27`, **[V]**: `ORDER_STATE_OPEN`, `_EXPIRED`,
/// `_CANCELLED`. A *filled* order is `EXPIRED` with `volRemaining == 0`:
/// `marketHistory.GetEntryDataForOrder` (marketHistory.py:263-265) prints
/// "Completed" for exactly that pair and "Expired" for state 2 otherwise, and
/// there is no separate "filled" state for it to read.
const ORDER_STATE_OPEN: i64 = 0;
const ORDER_STATE_EXPIRED: i64 = 2;
const ORDER_STATE_CANCELLED: i64 = 3;

/// `const.DAY` in blue FILETIME ticks (100 ns). `duration` is in **days**:
/// `quote.GetExpiresIn` (quote.py:564) is
/// `record.issueDate + record.duration * DAY - now`. **[V]**
const DAY_TICKS: i64 = 864_000_000_000;

/// `appConst.accountingKeyCash`, **[V]** (`docs/reference/wallet-ledger.md` §5.1).
/// `marketSvc._GetPersonalTransactionKeyVal` sets `isForCorp` from
/// `accountID != buyerAccountID`, so every personal row carries the same key
/// in all three columns.
const ACCOUNT_KEY_CASH: i64 = 1000;

/// Journal entry types, **[V]** from `appConst` (`docs/reference/wallet-ledger.md`
/// §5.1). `pub(crate)` because `account.GetEntryTypes` (service/wallet.rs)
/// has to list them, or the Journal tab silently drops every row that uses
/// them (`wallet-ledger.md` §5.3) -- see `docs/lanes/market.md` for that
/// edit, which is not this file's.
pub(crate) const REF_MARKET_TRANSACTION: i64 = 2;
pub(crate) const REF_MARKET_ESCROW: i64 = 42;
pub(crate) const REF_BROKER_FEE: i64 = 46;
/// `refTransactionTax` **[V]** (`appConst`; C5 §5.1): the sales tax.
/// `account.GetEntryTypes` must list it (coordinator hunk, `service/wallet.rs`).
pub(crate) const REF_TRANSACTION_TAX: i64 = 54;

// ---- the market's sinks (CP9, E5, X-TRADE; C5 §5.1, R-ECON §2.6) --------------
//
// The server computes every fee and tax from the character's skills and
// standings; the client's `expectedBrokersFee` is read by nobody (C5 §5.1's
// "Phase 1 may keep the client's figure but clamp it to the server formula":
// clamped all the way to it). The formulas are `crate::market_fees`, shared
// with the bots' price rule. The faction-warfare fee adjustment and the SCC
// surcharge are not modelled (`docs/lanes/x-trade.md`, Not done).

use crate::market_fees::{self as fees, Standings, TYPE_ACCOUNTING, TradeSkills};
#[cfg(test)]
use crate::market_fees::{
    ACCOUNTING_SKILL_MODIFIER, BROKER_PERCENT, BROKER_RELATIONS_SKILL_MODIFIER, MINIMUM_BROKER_FEE,
    SALES_TAX_PERCENT, TYPE_BROKER_RELATIONS, TYPE_MARGIN_TRADING, TYPE_TRADE, max_orders,
};

/// `appConst.mktModificationDelay`, seconds **[V]**.
pub(crate) const MODIFICATION_DELAY_S: i64 = 300;

/// A character's trained level in `skill`, 0 when untrained, as
/// `skilllimits.GetMarketSkills` reads it (`.get(type, 0)`).
fn skill_level(reg: &Registry, who: i64, skill: i64) -> i64 {
    reg.skills_for(Some(who))
        .and_then(|s| s.get(skill))
        .map_or(0, |e| e.level.clamp(0, 5))
}

/// All of a character's market skills (`skilllimits.GetMarketSkills`).
fn trade_skills(reg: &Registry, who: i64) -> TradeSkills {
    TradeSkills::read(|t| skill_level(reg, who, t))
}

/// The sales tax on `value` at Accounting `accounting`, to the cent
/// (`market_fees::sales_tax_rate`).
pub(crate) fn sales_tax(value: f64, accounting: i64) -> f64 {
    cents(value * fees::sales_tax_rate(accounting))
}

/// The broker's fee on a new order of `value` at Broker Relations `br` and
/// no standings: `max(mktMinimumFee, value x (3% - 0.3% x BR))`.
#[cfg(test)]
pub(crate) fn broker_fee(value: f64, br: i64) -> f64 {
    fees::fee_with_floor(cents(value * fees::broker_rate(br, Standings::default())))
}

/// What `station_id`'s owner's standing toward `who` is, as the client reads
/// it (`marketsvc.FactionAndCorpToCharStandings`): only an NPC owner has
/// any. The corporation's faction comes from `npc/corporations.tsv`; an
/// owner in the NPC corporation id band that the table does not know has a
/// corporation standing and no faction one.
fn standings_with_owner(reg: &Registry, who: i64, owner: i64) -> Standings {
    if !(1_000_000..2_000_000).contains(&owner) {
        return Standings::default();
    }
    let faction = reg
        .content()
        .try_get::<crate::npc_data::Table>()
        .and_then(|t| t.corporations.get(&owner))
        .and_then(|c| c.faction_id)
        .map_or(0.0, |f| reg.npc_standing(who, f));
    Standings {
        faction,
        corp: reg.npc_standing(who, owner),
    }
}

/// The broker's commission rate `who` pays at `station_id`: Broker Relations
/// and the standings with the station's owner (`brokerFee.py`).
pub(crate) fn broker_rate_at(reg: &Registry, who: i64, station_id: i64) -> f64 {
    let skills = trade_skills(reg, who);
    let owner = station_owner(reg, station_id);
    skills.broker_rate(standings_with_owner(reg, who, owner))
}

/// The broker's fee on a new order worth `value` at `station_id`, to the
/// cent, floored at `mktMinimumFee`.
fn new_order_fee(reg: &Registry, who: i64, station_id: i64, value: f64) -> f64 {
    fees::fee_with_floor(cents(value * broker_rate_at(reg, who, station_id)))
}

/// The broker's fee on changing a resting order from `old` to `new` ISK
/// (price x remaining), to the cent, floored (`brokerFee.GetBrokerFeeInfo`).
fn modification_fee(reg: &Registry, who: i64, station_id: i64, old: f64, new: f64) -> f64 {
    let discount = trade_skills(reg, who).modification_discount();
    let rate = broker_rate_at(reg, who, station_id);
    fees::fee_with_floor(cents(fees::raw_broker_fee(Some(old), new, rate, discount)))
}

/// Refuse `adding` more resting orders past the actor's cap, with the
/// client's own dialog: `MarketExceededOrderCount`, 1146, `curCnt` and
/// `maxCnt` **[V]** key and arguments (`findings/user-errors.tsv`); that the
/// server raises it is **[A]**.
fn check_order_cap(reg: &Registry, actor: i64, adding: usize) -> Result<(), ServiceError> {
    let max = trade_skills(reg, actor).max_orders();
    let open = reg.market.open().filter(|o| o.char_id == actor).count();
    if open + adding <= max {
        return Ok(());
    }
    Err(ServiceError::Raise(eve_proto::user_error(
        "MarketExceededOrderCount",
        crate::pyutil::pypairs(vec![
            ("curCnt", PyValue::Int(open as i64)),
            ("maxCnt", PyValue::Int(max as i64)),
        ]),
    )))
}

/// Charge `seller` the sales tax on a sale worth `value`, journalled as ref
/// 54 to the station's owner (C5 §5.1). A no-op for an NPC seller, which has
/// no wallet (`move_isk`).
fn charge_sales_tax(
    reg: &mut Registry,
    seller: i64,
    wallet: Wallet,
    value: f64,
    station_owner: i64,
    order_id: i64,
    now: i64,
) {
    if reg.character(seller).is_none() {
        return;
    }
    let tax = sales_tax(value, skill_level(reg, seller, TYPE_ACCOUNTING));
    corp::pay(
        reg,
        seller,
        wallet,
        -tax,
        (REF_TRANSACTION_TAX, station_owner),
        (order_id, now),
        true,
    );
}

/// The owner of a station (`Registry::station_ref`), 0 when unknown.
fn station_owner(reg: &Registry, station_id: i64) -> i64 {
    reg.station_ref(station_id).map_or(0, |s| s.owner_id)
}

/// Who owns the goods behind a sell order while it rests. **[A]**
///
/// Not the seller, on purpose: `Registry::owned_item_rows` answers the
/// assets window with *every* item a character owns, and a stack sitting at
/// an escrow location would show up there as a "station" with id 9e9+n that
/// no `cfg.evelocations` lookup can name. Owner 1 is the id the journal
/// already uses for "the system" in a GM grant.
const ESCROW_OWNER: i64 = 1;

/// Market escrow tickets are namespaced inside `item::escrow_location`'s one
/// band, as that function's doc asks. **[A]**: any constant other subsystems
/// do not also pick; 10^12 leaves the contracts lane the whole range below.
const ESCROW_TICKET_BASE: i64 = 1_000_000_000_000;

/// The first order id this server hands out. **[A]**. Above 2^31 on
/// purpose, so that a column declared `I4` by mistake truncates visibly on
/// the very first order instead of after two billion -- `orderID` is `I8` in
/// every descriptor here.
const FIRST_ORDER_ID: i64 = 5_000_000_000;

/// The first market transaction id. **[A]**, same reasoning.
const FIRST_TRANSACTION_ID: i64 = 7_000_000_000;

/// The `OnOwnOrdersChanged` reasons. The client compares against exactly
/// one value, `'Expiry'` -- `marketbase.OnOwnOrdersChanged`
/// (marketbase.py:777) skips its reload for it and
/// `buyMultiFromBase.OnOwnOrdersChanged` (:584) acts *only* on it -- so that
/// spelling is **[V]** and the other three are **[A]**: any string that is
/// not `'Expiry'` behaves identically in every handler we have read.
const REASON_EXPIRY: &str = "Expiry";
const REASON_ADD: &str = "Add";
const REASON_MODIFY: &str = "Modify";
const REASON_CANCEL: &str = "Cancel";
const REASON_FILL: &str = "TradeFill";

/// ISK to the cent. Every amount that reaches a wallet goes through this,
/// so that a long run of fills cannot drift a balance by float noise.
fn cents(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// One market order, open or closed.
///
/// The fields are the columns of [`own_order_descriptor`] minus the three
/// the client fills in or we can derive (`jumps`, `keyID`, `isCorp`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Order {
    pub order_id: i64,
    pub char_id: i64,
    pub type_id: i64,
    pub region_id: i64,
    pub solar_system_id: i64,
    pub station_id: i64,
    pub price: f64,
    pub vol_entered: i64,
    pub vol_remaining: i64,
    pub min_volume: i64,
    pub bid: bool,
    pub issue_date: i64,
    /// Days. 0 is an immediate order, which never rests in the book.
    pub duration: i64,
    pub range: i64,
    /// ISK still held for a buy order. Always 0.0 for a sell order.
    pub escrow: f64,
    pub state: i64,
    pub last_state_change: Option<i64>,
    /// CORP-BUY: the corporation whose wallet the order spends and earns for
    /// (`PlaceBuyOrder`'s `useCorp`), 0 for a pilot's own order. `char_id`
    /// stays the pilot who issued it.
    pub corp_id: i64,
    /// The corporation wallet division (accounting key 1000..=1006) behind a
    /// corp order, 0 for a pilot's own.
    pub key_id: i64,
}

impl Order {
    fn is_open(&self) -> bool {
        self.state == ORDER_STATE_OPEN
    }

    /// When a resting order stops being one, at the game clock's `pace`
    /// (lane `clock`): `issueDate + pace.span(duration days)`. Derived, not
    /// stored, so a restart at another scale re-derives every expiry
    /// ([`Book::set_pace`] rebuilds the index). **[A]** the client shows
    /// `issueDate + duration` days; at a scale above 1 the order closes
    /// sooner than its row says. Falsified by a pilot who needs the shown
    /// expiry to hold on a compressed-time server.
    fn expires_at(&self, pace: crate::game_clock::GameClock) -> i64 {
        self.issue_date + pace.span(self.duration * DAY_TICKS)
    }

    /// The escrow location holding a sell order's goods.
    fn escrow_slot(&self) -> crate::item::Slot {
        crate::item::Slot::new(
            ESCROW_OWNER,
            crate::item::escrow_location(ESCROW_TICKET_BASE + self.order_id),
            crate::model::FLAG_HANGAR,
        )
    }
}

/// One fill: `quantity` units changed hands at `price`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Trade {
    pub transaction_id: i64,
    pub date: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub price: f64,
    pub station_id: i64,
    pub buyer_id: i64,
    pub seller_id: i64,
    /// The region the trade happened in (CP42, the history's key). 0 for a
    /// `mkttrade` line written before CP42, which carries no region; those
    /// are placed by their station ([`trade_region`]).
    pub region_id: i64,
    /// The corporation and wallet division each side paid or earned
    /// through, 0 and 0 for a pilot's own wallet. `buyer_id` and `seller_id`
    /// stay the pilots who traded. Absent from a `mkttrade` line written
    /// before the corporation transactions list: 0, a pilot's trade.
    pub buyer_corp: i64,
    pub buyer_key: i64,
    pub seller_corp: i64,
    pub seller_key: i64,
}

/// One `(region, type, day)` of price history (CP42, C5 §8): the six
/// columns of [`price_history_descriptor`] minus the date, which is the key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hist {
    pub low: f64,
    pub high: f64,
    /// Volume-weighted mean price: `sum(price * qty) / sum(qty)`, so
    /// `GetAveragePrice`'s own `avgPrice * volume` sum (marketsvc.py,
    /// `priceVolume += entry[3] * entry[4]`) gives back the ISK that moved.
    pub avg: f64,
    pub volume: i64,
    /// The `orders` column: the number of fills that day **[A]** (CCP's
    /// count of distinct orders is not observable here). Falsified by a
    /// capture whose `orders` is below its fill count.
    pub orders: i64,
}

/// How many whole UTC days of trades and closed orders the book keeps
/// (CP42, C5 §8) **[A]**: older trades live on only as [`Hist`] rows.
/// Falsified by a player missing an older entry in the Transactions tab or
/// the Market History tab.
pub(crate) const HISTORY_DAYS: i64 = 30;

/// Midnight UTC of the day `t` falls in. Blue FILETIME counts from
/// 1601-01-01 00:00 UTC, so a whole number of `DAY_TICKS` is a midnight:
/// `GetHistoryRowList` computes `midnightToday` exactly so
/// (`lastTime / const.DAY * const.DAY`, marketsvc.py:388) **[V]**.
fn day_start(t: i64) -> i64 {
    t.div_euclid(DAY_TICKS) * DAY_TICKS
}

/// Every order the world has seen, and every trade.
///
/// Lives on `Registry` as `market` and in `store::Snapshot` as `market`,
/// rendering and parsing its own records the way `world::Bookmarks` does.
/// Closed orders stay, because `GetMarketOrderHistory` is their only reader
/// and it is supposed to show them.
///
/// **Indexed (E3, R-ECON §1.4 P4).** Open and closed orders live in two maps
/// by id, so a closed order leaves the hot book: [`Book::side`], [`Book::open`]
/// and the expiry sweep never visit one. Beside the open map sit two indexes
/// that only [`Book::insert`] and [`Book::update`] write, which are the only
/// ways in: [`SideKey`]s, ordered best first within each `(region, type,
/// side)`, so a side is one range read and no sort; and `(expires_at, id)`,
/// so the sweep reads only what is due. Each write also moves the stamp of
/// the order's `(region, type)` ([`Book::stamp`]), which is all the
/// `GetOrders` cache compares.
#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    /// The open orders by id: the hot book.
    open: std::collections::BTreeMap<i64, Order>,
    /// Every order no longer open (expired, filled, cancelled) by id. Read by
    /// `GetMarketOrderHistory`, the world file and the rollover's prune only.
    closed: std::collections::BTreeMap<i64, Order>,
    /// One [`SideKey`] per open order.
    sides: std::collections::BTreeSet<SideKey>,
    /// `(expires_at, order_id)` per open order.
    expiry: std::collections::BTreeSet<(i64, i64)>,
    trades: Vec<Trade>,
    /// `(region, type, day) -> Hist`, folded from `trades` at the rollover
    /// (CP42). Outlives the trades it was folded from.
    history: std::collections::BTreeMap<(i64, i64, i64), Hist>,
    next_order_id: i64,
    next_transaction_id: i64,
    /// Per-`(region, type)` change stamps. Not state.
    stamps: Stamps,
    /// `GetOrders`' answers (FX8). Not state: a clone starts empty and it
    /// never makes two books unequal, so snapshots ignore it.
    cache: BookCache,
    /// Lane clock: the pace the `expiry` index was built at. Not state.
    pace: crate::game_clock::GameClock,
    /// The ledger log (E4): every trade and every order that closes is
    /// appended to it, and [`Book::render`] leaves them out of the world
    /// file. Not state, like the stamps.
    log: crate::ledger::Log,
}

/// Where an open order sits in [`Book`]'s side index: `(region, type, bid,
/// rank, issue_date, order_id)`. `rank` is [`price_rank`], so the tuple order
/// within one `(region, type, bid)` is [`best_first`]'s.
type SideKey = (i64, i64, bool, i64, i64, i64);

/// `price` as an `i64` whose order is `f64::total_cmp`'s (the same bit trick
/// the standard library uses), reversed for a bid: best first either way.
fn price_rank(price: f64, bid: bool) -> i64 {
    let bits = price.to_bits() as i64;
    let rank = bits ^ (((bits >> 63) as u64) >> 1) as i64;
    if bid { !rank } else { rank }
}

fn side_key(o: &Order) -> SideKey {
    (
        o.region_id,
        o.type_id,
        o.bid,
        price_rank(o.price, o.bid),
        o.issue_date,
        o.order_id,
    )
}

/// The change stamp of every `(region, type)` an order write touched: a
/// fresh number from one counter on every write, so a stamp read twice is
/// equal only if nothing in that `(region, type)` changed in between. Not
/// state: never persisted, never compared.
#[derive(Debug, Clone, Default)]
struct Stamps {
    next: u64,
    by: std::collections::BTreeMap<(i64, i64), u64>,
    /// Orders [`Book::side`] has read, for the tests that check it reads
    /// only open ones. Relaxed: a count, not a synchronisation.
    visits: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PartialEq for Stamps {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Stamps {
    fn touch(&mut self, region_id: i64, type_id: i64) {
        self.next += 1;
        self.by.insert((region_id, type_id), self.next);
    }
}

impl Default for Book {
    fn default() -> Self {
        Book {
            open: std::collections::BTreeMap::new(),
            closed: std::collections::BTreeMap::new(),
            sides: std::collections::BTreeSet::new(),
            expiry: std::collections::BTreeSet::new(),
            trades: Vec::new(),
            history: std::collections::BTreeMap::new(),
            next_order_id: FIRST_ORDER_ID,
            next_transaction_id: FIRST_TRANSACTION_ID,
            stamps: Stamps::default(),
            cache: BookCache::default(),
            log: crate::ledger::Log::default(),
            pace: crate::game_clock::GameClock::REAL,
        }
    }
}

impl Book {
    fn mint_order_id(&mut self) -> i64 {
        let id = self.next_order_id;
        self.next_order_id += 1;
        id
    }

    /// One order by id, open or closed: two map reads.
    fn get(&self, order_id: i64) -> Option<&Order> {
        self.open
            .get(&order_id)
            .or_else(|| self.closed.get(&order_id))
    }

    /// Add an order, open or closed, to its map and indexes. An order with
    /// the id of one already held replaces it.
    fn insert(&mut self, o: Order) {
        self.take(o.order_id);
        self.stamps.touch(o.region_id, o.type_id);
        if o.is_open() {
            self.sides.insert(side_key(&o));
            self.expiry.insert((o.expires_at(self.pace), o.order_id));
            self.open.insert(o.order_id, o);
        } else {
            // E4: a closed order goes to the log each time it is written;
            // the last line of an id wins on load.
            self.log.append(order_date(&o), order_line(&o));
            self.closed.insert(o.order_id, o);
        }
    }

    /// Remove an order from its map and indexes.
    fn take(&mut self, order_id: i64) -> Option<Order> {
        if let Some(o) = self.open.remove(&order_id) {
            self.sides.remove(&side_key(&o));
            self.expiry.remove(&(o.expires_at(self.pace), o.order_id));
            self.stamps.touch(o.region_id, o.type_id);
            return Some(o);
        }
        self.closed.remove(&order_id)
    }

    /// **The only way to change an order**: `f` runs on it out of the book,
    /// and it goes back in through [`Self::insert`], so a price, state or
    /// expiry change moves it in every index and moves its stamp. `None`
    /// for an unknown id.
    fn update<R>(&mut self, order_id: i64, f: impl FnOnce(&mut Order) -> R) -> Option<R> {
        let mut o = self.take(order_id)?;
        let r = f(&mut o);
        o.order_id = order_id;
        self.insert(o);
        Some(r)
    }

    /// The open orders, by id. Never visits a closed one.
    fn open(&self) -> impl Iterator<Item = &Order> {
        self.open.values()
    }

    /// The closed orders, by id.
    fn closed(&self) -> impl Iterator<Item = &Order> {
        self.closed.values()
    }

    /// Every order, open and closed, by id.
    fn all(&self) -> impl Iterator<Item = &Order> {
        let mut open = self.open.values().peekable();
        let mut closed = self.closed.values().peekable();
        std::iter::from_fn(move || match (open.peek(), closed.peek()) {
            (Some(a), Some(b)) if b.order_id < a.order_id => closed.next(),
            (Some(_), _) => open.next(),
            (None, _) => closed.next(),
        })
    }

    /// Lane clock: index every open order's expiry at `pace` from now on.
    /// The index is rebuilt, so an order indexed at the old pace is found.
    fn set_pace(&mut self, pace: crate::game_clock::GameClock) {
        self.pace = pace;
        self.expiry = self
            .open
            .values()
            .map(|o| (o.expires_at(pace), o.order_id))
            .collect();
    }

    /// The ids of the open orders whose time is up at `now`, soonest first.
    fn due(&self, now: i64) -> Vec<i64> {
        self.expiry
            .range(..=(now, i64::MAX))
            .map(|&(_, id)| id)
            .collect()
    }

    /// The change stamp of `(region, type)`: equal across two reads only if
    /// no order of that type in that region was written in between.
    fn stamp(&self, region_id: i64, type_id: i64) -> u64 {
        self.stamps
            .by
            .get(&(region_id, type_id))
            .copied()
            .unwrap_or(0)
    }

    fn record_trade(&mut self, mut t: Trade) {
        t.transaction_id = self.next_transaction_id;
        self.next_transaction_id += 1;
        self.log.append(t.date, trade_line(&t));
        self.trades.push(t);
    }

    /// The open orders on one side of one type, best first: one range of the
    /// side index, already in order. Visits only those orders.
    ///
    /// Best is the client's own order: `SortAsks` (marketsvc.py:36) is price
    /// ascending then `issueDate`, and `SortBids` (:63) price descending then
    /// `issueDate`. **[V]** for the comparison keys; the client also sorts by
    /// `jumps`, which we cannot know, so ties beyond `issueDate` fall back to
    /// `orderID` like its own last key.
    fn side(&self, type_id: i64, region_id: i64, bid: bool) -> Vec<&Order> {
        let lo = (region_id, type_id, bid, i64::MIN, i64::MIN, i64::MIN);
        let hi = (region_id, type_id, bid, i64::MAX, i64::MAX, i64::MAX);
        self.sides
            .range(lo..=hi)
            .filter_map(|k| {
                self.stamps
                    .visits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.open.get(&k.5)
            })
            .collect()
    }

    /// The record tags this type writes into the world file. Same contract
    /// as `world::Bookmarks::RECORDS`: `store.rs` hands every line whose tag
    /// is listed here to [`Book::parse_record`], and the tags are prefixed so
    /// they cannot collide with another subsystem's.
    pub(crate) const RECORDS: &'static [&'static str] =
        &["mktnext", "mktorder", "mkttrade", "mkthist"];

    /// The durable form: one line per order and per trade, plus the two
    /// allocators. Tab-separated numbers, the `store.rs` house format, so
    /// `grep mktorder` answers "where did my order go".
    ///
    /// **Persisted because escrow is.** A resting buy order holds ISK that has
    /// already left the wallet, and a resting sell order holds goods that have
    /// already left the hangar (they sit at an escrow location in the item
    /// store, which *is* persisted). A world that kept the items and the
    /// debits but dropped the book would strand both.
    ///
    /// **With the ledger log attached (E4) only the open orders, the
    /// allocators and `mkthist` are written**: trades and closed orders are
    /// in the log, appended as they happen, and never in the snapshot.
    ///
    /// Nothing at all for a world nobody has traded in, so a file written
    /// before the market existed reads back identical.
    pub(crate) fn render(&self, out: &mut String) {
        use std::fmt::Write as _;
        if self.open.is_empty()
            && self.closed.is_empty()
            && self.trades.is_empty()
            && self.history.is_empty()
        {
            return;
        }
        let _ = writeln!(
            out,
            "mktnext\t{}\t{}",
            self.next_order_id, self.next_transaction_id
        );
        let logged = self.log.is_attached();
        let orders: Box<dyn Iterator<Item = &Order>> = if logged {
            Box::new(self.open())
        } else {
            Box::new(self.all())
        };
        for o in orders {
            out.push_str(&order_line(o));
        }
        if !logged {
            for t in &self.trades {
                out.push_str(&trade_line(t));
            }
        }
        // `mkthist <region> <type> <day> <low> <high> <avg> <vol> <n>`,
        // C5 §8's layout.
        for ((region, ty, day), h) in &self.history {
            let _ = writeln!(
                out,
                "mkthist\t{region}\t{ty}\t{day}\t{}\t{}\t{}\t{}\t{}",
                h.low, h.high, h.avg, h.volume, h.orders,
            );
        }
    }

    /// Read one record whose tag is in [`Book::RECORDS`].
    ///
    /// **Strict**, like `Bookmarks::parse_record`: an order line we cannot
    /// read is escrowed ISK or goods that would silently vanish, so the world
    /// is refused and the line named instead.
    pub(crate) fn parse_record(&mut self, f: &[&str]) -> Result<(), String> {
        let int = |i: usize| -> Result<i64, String> {
            f.get(i)
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| format!("field {i} is not an integer"))
        };
        let real = |i: usize| -> Result<f64, String> {
            f.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .ok_or_else(|| format!("field {i} is not a number"))
        };
        match f.first().copied() {
            Some("mktnext") => {
                self.next_order_id = self.next_order_id.max(int(1)?);
                self.next_transaction_id = self.next_transaction_id.max(int(2)?);
            }
            Some("mktorder") => {
                let o = parse_order(f)?;
                // An allocator that trails its own rows would hand out an
                // id twice, whatever order the lines come in.
                self.next_order_id = self.next_order_id.max(o.order_id + 1);
                // Two lines for one id would be two escrows for one order:
                // refused like any other line we cannot read.
                if self.get(o.order_id).is_some() {
                    return Err(format!("order {} appears twice", o.order_id));
                }
                self.insert(o);
            }
            Some("mkttrade") => {
                let t = parse_trade(f)?;
                self.next_transaction_id = self.next_transaction_id.max(t.transaction_id + 1);
                self.trades.push(t);
            }
            Some("mkthist") => {
                let h = Hist {
                    low: real(4)?,
                    high: real(5)?,
                    avg: real(6)?,
                    volume: int(7)?,
                    orders: int(8)?,
                };
                self.history.insert((int(1)?, int(2)?, int(3)?), h);
            }
            other => return Err(format!("{other:?} is not a market record")),
        }
        Ok(())
    }
}

/// One order as a `mktorder` line, newline included: the world file's
/// record and the ledger log's (E4). `crate::ledger::meta` reads its id
/// (1), owner (2), issue date (12), escrow (15) and last state change (17).
fn order_line(o: &Order) -> String {
    format!(
        "mktorder\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        o.order_id,
        o.char_id,
        o.type_id,
        o.region_id,
        o.solar_system_id,
        o.station_id,
        o.price,
        o.vol_entered,
        o.vol_remaining,
        o.min_volume,
        o.bid as u8,
        o.issue_date,
        o.duration,
        o.range,
        o.escrow,
        o.state,
        o.last_state_change.unwrap_or(-1),
        o.corp_id,
        o.key_id,
    )
}

/// One trade as a `mkttrade` line, newline included. `crate::ledger::meta`
/// reads its id (1), date (2), buyer (7) and seller (8).
fn trade_line(t: &Trade) -> String {
    format!(
        "mkttrade\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        t.transaction_id,
        t.date,
        t.type_id,
        t.quantity,
        t.price,
        t.station_id,
        t.buyer_id,
        t.seller_id,
        t.region_id,
        t.buyer_corp,
        t.buyer_key,
        t.seller_corp,
        t.seller_key,
    )
}

/// When a closed order closed: its last state change, else its issue date.
/// Names its ledger day and drives the CP42 prune.
fn order_date(o: &Order) -> i64 {
    o.last_state_change.unwrap_or(o.issue_date)
}

fn field_int(f: &[&str], i: usize) -> Result<i64, String> {
    f.get(i)
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("field {i} is not an integer"))
}

fn field_real(f: &[&str], i: usize) -> Result<f64, String> {
    f.get(i)
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or_else(|| format!("field {i} is not a number"))
}

/// The inverse of [`order_line`], for one line split on tabs.
fn parse_order(f: &[&str]) -> Result<Order, String> {
    let int = |i| field_int(f, i);
    let real = |i| field_real(f, i);
    Ok(Order {
        order_id: int(1)?,
        char_id: int(2)?,
        type_id: int(3)?,
        region_id: int(4)?,
        solar_system_id: int(5)?,
        station_id: int(6)?,
        price: real(7)?,
        vol_entered: int(8)?,
        vol_remaining: int(9)?,
        min_volume: int(10)?,
        bid: int(11)? != 0,
        issue_date: int(12)?,
        duration: int(13)?,
        range: int(14)?,
        escrow: real(15)?,
        state: int(16)?,
        last_state_change: match int(17)? {
            -1 => None,
            v => Some(v),
        },
        // CORP-BUY: appended fields, absent from an older file (a pilot's order).
        corp_id: f.get(18).and_then(|v| v.parse().ok()).unwrap_or(0),
        key_id: f.get(19).and_then(|v| v.parse().ok()).unwrap_or(0),
    })
}

/// The inverse of [`trade_line`], for one line split on tabs.
fn parse_trade(f: &[&str]) -> Result<Trade, String> {
    let int = |i| field_int(f, i);
    Ok(Trade {
        transaction_id: int(1)?,
        date: int(2)?,
        type_id: int(3)?,
        quantity: int(4)?,
        price: field_real(f, 5)?,
        station_id: int(6)?,
        buyer_id: int(7)?,
        seller_id: int(8)?,
        // Absent before CP42: 0, placed by its station.
        region_id: if f.len() > 9 { int(9)? } else { 0 },
        // Absent before the corporation transactions list: a pilot's trade.
        buyer_corp: if f.len() > 10 { int(10)? } else { 0 },
        buyer_key: if f.len() > 11 { int(11)? } else { 0 },
        seller_corp: if f.len() > 12 { int(12)? } else { 0 },
        seller_key: if f.len() > 13 { int(13)? } else { 0 },
    })
}

/// Whether the CP42 window keeps a trade, the window opening at `from`.
fn trade_kept(t: &Trade, from: i64) -> bool {
    day_start(t.date) >= from
}

/// Whether the CP42 window keeps a closed order: inside the window, or
/// still holding escrow (none should; `sweep` and cancel zero it).
fn closed_kept(o: &Order, from: i64) -> bool {
    o.escrow != 0.0 || day_start(order_date(o)) >= from
}

impl Registry {
    /// Lane clock: the market's expiry index at the game clock's pace
    /// (`crate::game_clock::set`).
    pub(crate) fn set_market_pace(&mut self, pace: crate::game_clock::GameClock) {
        self.market.set_pace(pace);
    }

    /// The first instant whose market rows a rollover at `now` has **not**
    /// folded into `mkthist`: today's midnight (the CP42 step folds every
    /// whole day before it). `ledger::Rules` drops a bot's market row only
    /// before it, and only `BOT_LEDGER_DAYS` after its day; the history
    /// reader takes `mkthist` for every folded day ([`history_of`], E5), so
    /// the row is no longer needed. A human's rows keep the CP42 RAM window
    /// ([`HISTORY_DAYS`]) and the log's six months.
    pub(crate) fn market_ledger_from(&self, now: i64) -> i64 {
        day_start(now)
    }

    /// Whether `ledger::Rules` at `now` drops a market log line: the one rule
    /// for RAM and the files (E4), here for RAM.
    fn ledger_drops(rules: &crate::ledger::Rules, line: &str) -> bool {
        crate::ledger::meta(line).is_some_and(|m| rules.drops(&m))
    }

    /// **The market's half of `ledger::open`** (E4): `lines` are the
    /// `mkttrade` and `mktorder` lines of the log. Trades and closed orders
    /// the book holds from a pre-E4 world file and the log does not are
    /// appended to `log` (migrated); then the book's trades and closed
    /// orders become the log's, last line of an id winning, inside the CP42
    /// window as the rollover at `at` left it. An order open in the
    /// snapshot stays open: the snapshot is what its escrow agrees with.
    /// Then `log` is attached. Returns `(migrated, unreadable)`.
    pub(crate) fn market_ledger_open(
        &mut self,
        lines: &[&str],
        log: &crate::ledger::Log,
        at: i64,
    ) -> (usize, usize) {
        let mut trades: std::collections::BTreeMap<i64, Trade> = Default::default();
        let mut closed: std::collections::BTreeMap<i64, Order> = Default::default();
        let mut torn = 0;
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("mkttrade") => match parse_trade(&f) {
                    Ok(t) => {
                        trades.insert(t.transaction_id, t);
                    }
                    Err(_) => torn += 1,
                },
                Some("mktorder") => match parse_order(&f) {
                    Ok(o) if !o.is_open() => {
                        closed.insert(o.order_id, o);
                    }
                    _ => torn += 1,
                },
                _ => torn += 1,
            }
        }
        let book = &mut self.market;
        let mut migrated = 0;
        for t in &book.trades {
            if let std::collections::btree_map::Entry::Vacant(v) = trades.entry(t.transaction_id) {
                log.append(t.date, trade_line(t));
                v.insert(t.clone());
                migrated += 1;
            }
        }
        for o in book.closed.values() {
            if let std::collections::btree_map::Entry::Vacant(v) = closed.entry(o.order_id) {
                log.append(order_date(o), order_line(o));
                v.insert(o.clone());
                migrated += 1;
            }
        }
        let from = day_start(at) - HISTORY_DAYS * DAY_TICKS;
        book.log = log.clone();
        let rules = crate::ledger::Rules::of(self, at);
        let book = &mut self.market;
        book.trades = trades
            .into_values()
            .filter(|t| trade_kept(t, from) && !Self::ledger_drops(&rules, &trade_line(t)))
            .collect();
        if let Some(t) = book.trades.last() {
            book.next_transaction_id = book.next_transaction_id.max(t.transaction_id + 1);
        }
        let old: Vec<i64> = book.closed.keys().copied().collect();
        for id in old {
            book.take(id);
        }
        for o in closed.into_values() {
            book.next_order_id = book.next_order_id.max(o.order_id + 1);
            let gone = !closed_kept(&o, from) || Self::ledger_drops(&rules, &order_line(&o));
            if book.open.contains_key(&o.order_id) || gone {
                continue;
            }
            book.insert(o);
        }
        (migrated, torn)
    }
}

/// The client's order of one side, best first: `SortAsks` (marketsvc.py:36)
/// is price ascending then `issueDate`, `SortBids` (:63) price descending
/// then `issueDate`. **[V]** for the keys; the client also sorts by `jumps`,
/// which we cannot know, so ties beyond `issueDate` fall back to `orderID`
/// like its own last key.
fn best_first(a: &Order, b: &Order, bid: bool) -> std::cmp::Ordering {
    let by_price = if bid {
        b.price.total_cmp(&a.price)
    } else {
        a.price.total_cmp(&b.price)
    };
    by_price
        .then(a.issue_date.cmp(&b.issue_date))
        .then(a.order_id.cmp(&b.order_id))
}

// ---- NPC orders (CP35) ---------------------------------------------------------

/// An NPC order as a book order: the station's corporation is `charID`
/// (never sent on the public book), no escrow, open.
fn npc_order(o: &exchange::NpcOrder) -> Order {
    Order {
        order_id: o.order_id,
        char_id: o.corp_id,
        type_id: o.type_id,
        region_id: o.region_id,
        solar_system_id: o.system_id,
        station_id: o.station_id,
        price: o.price,
        vol_entered: o.vol,
        vol_remaining: o.remaining,
        min_volume: 1,
        bid: o.side == exchange::Side::Bid,
        issue_date: o.issue_date,
        duration: exchange::DURATION_DAYS,
        // C5 §4.1 **[A]**: station range for both sides.
        range: RANGE_STATION,
        escrow: 0.0,
        state: ORDER_STATE_OPEN,
        last_state_change: None,
        corp_id: 0,
        key_id: 0,
    }
}

/// Whether an NPC order can be shown and filled: an ask must be a type the
/// item store can mint (`item::find_in`, PI's types included), or a buyer
/// would pay for nothing.
fn npc_fillable(content: &crate::content::Content, o: &exchange::NpcOrder) -> bool {
    o.side == exchange::Side::Bid || crate::item::find_in(content, o.type_id).is_some()
}

/// The NPC orders of one side in a region (of `only` that type), as book
/// orders.
fn npc_side(reg: &Registry, region_id: i64, only: Option<i64>, bid: bool, now: i64) -> Vec<Order> {
    let all = exchange::orders(reg, region_id, only, now);
    let mine = all
        .iter()
        .filter(|o| (o.side == exchange::Side::Bid) == bid);
    let content = reg.content();
    let mut out: Vec<Order> = mine
        .filter(|o| npc_fillable(content, o))
        .map(npc_order)
        .collect();
    // DEV-MODE: the dev seller's ask on a named type, at every station of
    // the region (synthesised, never stored).
    if reg.config().dev
        && !bid
        && let Some(ty) = only
    {
        out.extend(dev_asks(reg, region_id, None, Some(ty)));
    }
    out
}

/// The dev seller's asks as book orders (`--dev` only).
fn dev_asks(reg: &Registry, region_id: i64, station: Option<i64>, only: Option<i64>) -> Vec<Order> {
    let content = reg.content();
    exchange::dev_orders(reg, region_id, station, only)
        .iter()
        .filter(|o| crate::item::find_in(content, o.type_id).is_some())
        .map(npc_order)
        .collect()
}

/// [`Book::side`] with the NPC orders of that side merged in, best first.
fn side_with_npc(reg: &Registry, type_id: i64, region_id: i64, bid: bool, now: i64) -> Vec<Order> {
    let mut v: Vec<Order> = reg
        .market
        .side(type_id, region_id, bid)
        .into_iter()
        .cloned()
        .collect();
    v.extend(npc_side(reg, region_id, Some(type_id), bid, now));
    v.sort_by(|a, b| best_first(a, b, bid));
    v
}

impl Registry {
    /// Every market trade at or after `since`, for the NPC exchange's
    /// rollover (multipliers and the trading index, CP35).
    pub(crate) fn market_fills_since(&self, since: i64) -> Vec<exchange::Fill> {
        self.market
            .trades
            .iter()
            .filter(|t| t.date >= since)
            .map(|t| exchange::Fill {
                date: t.date,
                station_id: t.station_id,
                type_id: t.type_id,
                quantity: t.quantity,
                price: t.price,
                buyer_id: t.buyer_id,
                seller_id: t.seller_id,
            })
            .collect()
    }

    /// Every open player or bot order (FE-MEASURE: book depth, stock on
    /// sale, spot). NPC orders are computed, not resting, and are not here.
    pub(crate) fn market_open_orders(&self) -> Vec<exchange::BookRow> {
        self.market
            .open()
            .map(|o| exchange::BookRow {
                char_id: o.char_id,
                type_id: o.type_id,
                region_id: o.region_id,
                price: o.price,
                remaining: o.vol_remaining,
                bid: o.bid,
            })
            .collect()
    }

    /// Rest orders as the book would hold them, for a test of its readers.
    #[cfg(test)]
    pub(crate) fn seed_market_orders(&mut self, rows: Vec<exchange::BookRow>) {
        for r in rows {
            let order_id = self.market.next_order_id;
            self.market.next_order_id += 1;
            self.market.insert(Order {
                order_id,
                char_id: r.char_id,
                type_id: r.type_id,
                region_id: r.region_id,
                solar_system_id: 0,
                station_id: 0,
                price: r.price,
                vol_entered: r.remaining,
                vol_remaining: r.remaining,
                min_volume: 1,
                bid: r.bid,
                issue_date: 1,
                duration: 90,
                range: RANGE_STATION,
                escrow: 0.0,
                state: ORDER_STATE_OPEN,
                last_state_change: None,
                corp_id: 0,
                key_id: 0,
            });
        }
    }

    /// Record trades as fills would, for a test of their readers.
    #[cfg(test)]
    pub(crate) fn seed_market_fills(&mut self, fills: Vec<exchange::Fill>) {
        for f in fills {
            self.market.record_trade(Trade {
                transaction_id: 0,
                date: f.date,
                type_id: f.type_id,
                quantity: f.quantity,
                price: f.price,
                station_id: f.station_id,
                buyer_id: f.buyer_id,
                seller_id: f.seller_id,
                region_id: 0,
                ..Default::default()
            });
        }
    }
}

// ---- price history and pruning (CP42) --------------------------------------
//
// C5 §8 and packet E7. **The trades are the truth and `mkthist` is their
// fold**: a reader aggregates the live trades of a type on the fly and takes
// a `mkthist` row only for a day whose trades are gone, so a day reads the
// same before and after the rollover folds it. The rollover
// ([`Registry::market_history_rollover`]) folds every complete UTC day, then
// drops the trades and closed orders older than [`HISTORY_DAYS`]: `Book` is
// `Vec`-scanned and fully rendered on every save, so without this a market
// that bots trade in grows without bound.
//
// **Old and new are split at today, not at 30 days** -- C5 §8's 30-day
// split was **[A]**, and the client says otherwise. `GetHistoryRowList`
// (marketsvc.py:388) walks `old` padding each gap with the last price, then
// pads from the last old row **up to yesterday** (`while lastTime <
// midnightToday - const.DAY`), and only then appends `new` -- or, with `new`
// empty, one synthetic row stamped now. So `old` is every whole day up to
// yesterday and `new` is today **[V]**; a 30-day `new` would be drawn after
// a run of padding that already reached yesterday.
//
// No day is fabricated: a type nobody traded has two empty halves and the
// flat chart `docs/reference/market-browse.md` §5 item 14 calls honest. Every row has
// `avgPrice > 0` (`valid_price`), and padding repeats a real row's price, so
// the ticker's `hist[-1] / hist[-7]` (ticker.py, `TickerItem.LoadData`)
// never divides by zero.

/// The region a trade happened in: its own, or for a `mkttrade` line from
/// before CP42 its station's -- an order there, the station table, the NPC
/// exchange's. `None` leaves the trade out of every region's history.
fn trade_region(reg: &Registry, t: &Trade) -> Option<i64> {
    if t.region_id != 0 {
        return Some(t.region_id);
    }
    reg.market
        .all()
        .find(|o| o.station_id == t.station_id)
        .map(|o| o.region_id)
        .or_else(|| reg.station_ref(t.station_id).map(|s| s.region_id))
        .or_else(|| exchange::place(reg, t.station_id).map(|(_, region)| region))
}

/// The trades `keep` selects, folded into one [`Hist`] per
/// `(region, type, day)`. NPC fills are trades like any other (C5 §8).
fn aggregate(
    reg: &Registry,
    keep: impl Fn(&Trade) -> bool,
) -> std::collections::BTreeMap<(i64, i64, i64), Hist> {
    // (low, high, ISK, volume, fills)
    type Sums = (f64, f64, f64, i64, i64);
    let mut sums: std::collections::BTreeMap<(i64, i64, i64), Sums> =
        std::collections::BTreeMap::new();
    // REVIEW-FIX: a wash fill (`econ_prices::same_group`, the predicate the
    // trading index uses) is no history either: the bots' price reader and
    // the client's chart both fold this, so a self-trade at 3x moves neither.
    for t in reg
        .market
        .trades
        .iter()
        .filter(|t| keep(t) && !crate::econ_prices::same_group(reg, t.buyer_id, t.seller_id))
    {
        let Some(region) = trade_region(reg, t) else {
            continue;
        };
        let e = sums
            .entry((region, t.type_id, day_start(t.date)))
            .or_insert((f64::MAX, f64::MIN, 0.0, 0, 0));
        e.0 = e.0.min(t.price);
        e.1 = e.1.max(t.price);
        e.2 += t.price * t.quantity as f64;
        e.3 += t.quantity;
        e.4 += 1;
    }
    sums.into_iter()
        .filter(|(_, s)| s.3 > 0)
        .map(|(k, (low, high, isk, volume, orders))| {
            let avg = isk / volume as f64;
            (
                k,
                Hist {
                    low,
                    high,
                    avg,
                    volume,
                    orders,
                },
            )
        })
        .collect()
}

/// Every day of one type's history in one region: the `mkthist` row of
/// every folded day, and the live trades' fold for the days not yet folded
/// (E5). A folded day is read from its row alone, because its trades may be
/// gone in part: a bot's leave RAM `BOT_LEDGER_DAYS` after its day, a
/// human's after [`HISTORY_DAYS`] (`ledger::Rules`).
fn history_of(reg: &Registry, region: i64, type_id: i64) -> Vec<(i64, Hist)> {
    let mut days: std::collections::BTreeMap<i64, Hist> = reg
        .market
        .history
        .range((region, type_id, i64::MIN)..=(region, type_id, i64::MAX))
        .map(|(&(_, _, day), h)| (day, *h))
        .collect();
    for ((r, _, day), h) in aggregate(reg, |t| t.type_id == type_id) {
        if r == region {
            days.entry(day).or_insert(h);
        }
    }
    days.into_iter().collect()
}

/// One price-history row, in `price_history_descriptor`'s wire order.
fn hist_row(day: i64, h: &Hist) -> Vec<DbValue> {
    // ISK x 10,000: `DBTYPE_CY`, as in `book_row`.
    let cy = |x: f64| DbValue::Int((x * 10_000.0).round() as i64);
    vec![
        DbValue::Int(day),
        cy(h.low),
        cy(h.high),
        cy(h.avg),
        DbValue::Int(h.volume),
        DbValue::Int(h.orders),
    ]
}

/// `(old, new)` for one type in `region` at `now`: every day before today,
/// ascending as `GetHistoryRowList`'s padding needs, and today. Both empty
/// with no region.
fn history_halves(
    reg: &Registry,
    region: Option<i64>,
    type_id: i64,
    now: i64,
) -> (PyValue, PyValue) {
    let descriptor = price_history_descriptor();
    let mut old = eve_rowset::CRowset::new(descriptor.clone());
    let mut new = eve_rowset::CRowset::new(descriptor);
    let today = day_start(now);
    for (day, h) in region.map_or_else(Vec::new, |r| history_of(reg, r, type_id)) {
        let half = if day < today { &mut old } else { &mut new };
        half.push_values(hist_row(day, &h))
            .expect("a history row matches price_history_descriptor");
    }
    (old.to_pyvalue(), new.to_pyvalue())
}

/// `{typeID: (old, new)}`, one entry per id asked for (see
/// `empty_price_histories` for why every id needs one).
fn price_histories(reg: &Registry, region: Option<i64>, type_ids: &[i64], now: i64) -> PyValue {
    PyValue::Dict(
        type_ids
            .iter()
            .map(|&id| {
                let (old, new) = history_halves(reg, region, id, now);
                // Dict entries are (value, key) in wire order.
                (PyValue::Tuple(vec![old, new]), PyValue::Int(id))
            })
            .collect(),
    )
}

impl Registry {
    /// **The CP42 rollover step**: fold every complete UTC day's trades into
    /// `mkthist`, then drop the trades, and the closed orders, whose day is
    /// more than [`HISTORY_DAYS`] before today, or that `ledger::Rules`
    /// drops (a bot's, `BOT_LEDGER_DAYS` after its day, E5). Its count is
    /// the rows dropped.
    ///
    /// Idempotent: a day is folded once, from all its trades, and never
    /// again (a folded day's row stays, since its trades may since have gone
    /// in part). A catch-up after N days down folds all N before it prunes.
    /// A closed order still holding escrow is kept (none should be; `sweep`
    /// and cancel zero it).
    pub(crate) fn market_history_rollover(&mut self, d: &crate::rollover::Day) -> usize {
        let today = day_start(d.now);
        let mut folded = aggregate(self, |t| day_start(t.date) < today);
        let cutoff = today - HISTORY_DAYS * DAY_TICKS;
        let rules = crate::ledger::Rules::of(self, d.now);
        let book = &mut self.market;
        folded.retain(|k, _| !book.history.contains_key(k));
        let changed = !folded.is_empty();
        book.history.extend(folded);
        let before = book.trades.len() + book.closed.len();
        book.trades
            .retain(|t| trade_kept(t, cutoff) && !Self::ledger_drops(&rules, &trade_line(t)));
        // Open orders are never pruned, and only open ones are indexed, so
        // the closed map is all there is to prune.
        book.closed
            .retain(|_, o| closed_kept(o, cutoff) && !Self::ledger_drops(&rules, &order_line(o)));
        let pruned = before - (book.trades.len() + book.closed.len());
        if changed || pruned > 0 {
            self.dirty = true;
        }
        pruned
    }
}

// ---- where an order may trade ----------------------------------------------

/// Where a trade happens: a station and the system and region around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    station_id: i64,
    solar_system_id: i64,
    region_id: i64,
}

/// Is `there` within `range` of an order resting at `order`?
///
/// The client's own test is `jumps <= range or (range == rangeStation and
/// stationID == stationID)` (marketsvc.py:519, **[V]**), with `jumps`
/// computed by its pathfinder. We have no jump graph, so **[A]**: a jump
/// range of 1..=40 (and `rangeConstellation`) is treated as "same solar
/// system", which can only ever refuse a trade the real server would allow,
/// never allow one it would refuse.
fn in_range(order: Place, range: i64, there: Place) -> bool {
    if order.region_id != there.region_id {
        return false;
    }
    match range {
        RANGE_STATION => order.station_id == there.station_id,
        RANGE_REGION => true,
        RANGE_SOLAR_SYSTEM => order.solar_system_id == there.solar_system_id,
        // A jump count, or `rangeConstellation`: see above.
        _ => order.solar_system_id == there.solar_system_id,
    }
}

/// The client's own minimum-volume rule, marketsvc.py:523, **[V]**:
/// `minVolume <= amount or (volRemaining < minVolume and amount >= volRemaining)`.
fn min_volume_ok(order: &Order, amount: i64) -> bool {
    order.min_volume <= amount
        || (order.vol_remaining < order.min_volume && amount >= order.vol_remaining)
}

/// Where the acting character can place an order: their own station. **[A]**
/// Remote order placement needs the station table, which we do not have
/// beyond the character's own row -- and a buy from a *remote ask* still
/// works, because that ask carries its own place (see `place_of_station`).
fn home_of(reg: &Registry, actor: i64) -> Option<Place> {
    let c = reg.character(actor)?;
    Some(Place {
        station_id: c.station_id?,
        solar_system_id: c.solar_system_id?,
        region_id: c.region_id,
    })
}

/// [`home_of`], only while the actor is docked in it.
///
/// `home_of` is "my station" (`Character::station_id`, which stays set on
/// undock) and is right for *browsing* -- the region and system the market
/// window lists. **Placing** at a station by being in it needs docked-now
/// (`Registry::docked_station`, `docs/lanes/docked.md`): before this, a pilot
/// in a belt could put a sell order up from the home hangar.
fn docked_place(reg: &Registry, actor: i64) -> Option<Place> {
    let docked = reg.docked_station(actor)?;
    home_of(reg, actor).filter(|h| h.station_id == docked)
}

/// Where the acting character is *browsing*: the station they are docked in,
/// else (in space, or in a jump) the system their session names with its
/// region and no station. A pilot in another region lists that region's
/// book, not the home station's (`docs/lanes/stale-system.md`). **[A]**
/// Station-range asks are empty in space; falsifier: a TQ capture of
/// `GetStationAsks` undocked.
fn browse_place(reg: &Registry, actor: i64) -> Option<Place> {
    if let Some(p) = docked_place(reg, actor) {
        return Some(p);
    }
    let system = reg.session_system(actor)?;
    Some(Place {
        station_id: 0,
        solar_system_id: system,
        region_id: crate::jump::system_row(reg, actor, system)?.region_id,
    })
}

/// A station we can place: the one the actor is docked in, or any station an
/// order already sits at. The second is how a buy from a *remote ask* works
/// from space **[A]**, as in retail.
fn place_of_station(reg: &Registry, actor: i64, station_id: i64) -> Option<Place> {
    if let Some(here) = docked_place(reg, actor).filter(|h| h.station_id == station_id) {
        return Some(here);
    }
    reg.market
        .all()
        .find(|o| o.station_id == station_id)
        .map(|o| Place {
            station_id,
            solar_system_id: o.solar_system_id,
            region_id: o.region_id,
        })
        .or_else(|| {
            exchange::place(reg, station_id).map(|(system, region)| Place {
                station_id,
                solar_system_id: system,
                region_id: region,
            })
        })
}

// ---- refusals ----------------------------------------------------------------

/// A `UserError` by key with no arguments. Every key used here is checked
/// against `findings/user-errors.tsv`; *which* key a given refusal deserves
/// is **[A]** unless its comment says otherwise.
fn mkt_error(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

/// `CustomNotify`, dialog 766, notify/logline: the free-text escape hatch,
/// for refusals no shipped key describes.
fn mkt_notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// `MktNotEnoughCash`, dialog 1210, info/modal, with the three loc args the
/// row declares: `[numeric]balance.quantity`, `[numeric]cashAmount.quantity`,
/// `[numeric]tax.quantity`. Key and argument names **[V]**; that this is the
/// key the real server raises for a short wallet is **[A]**.
fn not_enough_cash(balance: f64, needed: f64, fee: f64) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "MktNotEnoughCash",
        crate::pyutil::pypairs(vec![
            ("balance", PyValue::Real(balance)),
            ("cashAmount", PyValue::Real(needed)),
            ("tax", PyValue::Real(fee)),
        ]),
    ))
}

// ---- argument readers -------------------------------------------------------

fn arg_i64(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Bool(b) => Some(*b as i64),
        PyValue::Real(r) if r.is_finite() => Some(*r as i64),
        _ => None,
    }
}

fn arg_f64(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Real(r) if r.is_finite() => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// One attribute of a `utillib.KeyVal` argument. A `KeyVal` is an old-style
/// instance whose state is its attribute dict (`pyutil::keyval`'s doc), so
/// this is `dict_get` one level down. A plain dict is accepted too.
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(_) => crate::pyutil::dict_get(v, name.as_bytes()),
        PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, name.as_bytes()),
        _ => None,
    }
}

fn seq(v: Option<&PyValue>) -> &[PyValue] {
    match v {
        Some(PyValue::List(items)) | Some(PyValue::Tuple(items)) => items,
        _ => &[],
    }
}

// ---- moving ISK and goods ---------------------------------------------------

/// Add `delta` ISK to a character's wallet, journal it, and tell them.
///
/// `Registry::charge` is the debit the rest of the tree uses, but it always
/// tells the **acting** session -- right for a fee, wrong for the seller
/// whose ask a buyer just filled. This is the same three halves with the
/// push addressed to the wallet's owner. A no-op for a zero amount, which is
/// `Registry::charge`'s own rule (a zero-ISK row is noise).
///
/// Journal owners: the payer in `owner_id_1`, the payee in `owner_id_2`.
/// **[A]**.
fn move_isk(
    reg: &mut Registry,
    owner: i64,
    delta: f64,
    entry_type_id: i64,
    counterparty: i64,
    reference_id: i64,
    now: i64,
) {
    let delta = cents(delta);
    if delta == 0.0 {
        return;
    }
    let Some(c) = reg.character_mut(owner) else {
        return;
    };
    c.balance = cents(c.balance + delta);
    let balance = c.balance;
    let (from, to) = if delta < 0.0 {
        (owner, counterparty)
    } else {
        (counterparty, owner)
    };
    reg.dirty = true;
    reg.journal.record(
        now,
        entry_type_id,
        from,
        to,
        reference_id,
        delta,
        balance,
        "",
    );
    reg.push(WalletChanged {
        to: owner,
        change: crate::notify::AccountChange {
            owner_id: owner,
            balance,
        },
    });
}

fn balance_of(reg: &Registry, owner: i64) -> f64 {
    reg.character(owner).map_or(0.0, |c| c.balance)
}

/// Move `quantity` units of `item_id` to `to`, whole or split, and tell both
/// owners.
///
/// `Registry::apply_items` already pushes the generic `OnItemChange` and the
/// location invalidation -- to the **acting** session, with the hard-coded
/// `{ixLocationID: 0}` change dict. That dict makes a removal inexpressible
/// (`docs/lanes/inventory.md`, "A removal cannot be expressed"): the
/// client's `old` row is the new row with the change applied
/// (inventorysvc.py:65-71), so the seller's hangar computes `wasHere ==
/// False` and the sold stack stays on screen. Two addressed pushes close
/// that for the market's own moves, **[V]** mechanism, see [`ItemMoved`].
fn move_goods(
    reg: &mut Registry,
    actor: i64,
    item_id: i64,
    quantity: i64,
    to: crate::item::Slot,
) -> Result<Option<i64>, ServiceError> {
    let Some(before) = reg.store().get(item_id).cloned() else {
        return Err(mkt_error("MktOrderChanged"));
    };
    let whole = quantity >= before.quantity;
    let edit = if whole {
        crate::item::Edit::Move { item_id, to }
    } else {
        crate::item::Edit::Split {
            item_id,
            quantity,
            to,
        }
    };
    // Refused only if the store changed under a plan validated in this same
    // turn, which cannot happen under the world lock -- but if it does, the
    // client's own "the order changed, look again" key is the honest answer.
    let changes = reg
        .apply_items(&[edit])
        .map_err(|_| mkt_error("MktOrderChanged"))?;
    // The stack that arrived: the moved item itself, or the split's new one.
    let arrived = if whole {
        Some(item_id)
    } else {
        changes.iter().find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
    };
    // The sender's hangar loses a whole stack: tell the sender, with a change
    // dict that names where it was so `wasHere` is true.
    if whole
        && before.owner_id != ESCROW_OWNER
        && let Some(item) = reg.store().get(item_id).cloned()
    {
        let row = reg.item_row_value(&item);
        reg.push(ItemMoved {
            to: before.owner_id,
            row,
            change: vec![
                (ITEM_IX_OWNER, before.owner_id),
                (ITEM_IX_LOCATION, before.location_id),
                (ITEM_IX_FLAG, before.flag_id),
            ],
        });
    }
    // The receiver gains a stack: tell the receiver, the same way
    // `notify::ItemChange` tells the actor, plus the invalidation.
    if to.owner_id != ESCROW_OWNER
        && to.owner_id != actor
        && let Some(item) = arrived.and_then(|id| reg.store().get(id).cloned())
    {
        let row = reg.item_row_value(&item);
        reg.push(ItemMoved {
            to: to.owner_id,
            row,
            change: vec![(ITEM_IX_LOCATION, 0)],
        });
        reg.push(LocationsChanged {
            to: to.owner_id,
            location_ids: vec![to.location_id],
        });
    }
    Ok(arrived)
}

/// The goods behind a sell order: the one stack at its escrow location.
fn escrow_stack(reg: &Registry, order: &Order) -> Option<i64> {
    reg.store()
        .at(order.escrow_slot())
        .map(|i| i.item_id)
        .next()
}

/// Give a closed or shrunk order back to its owner: unspent escrow for a
/// buy, the unsold goods for a sell.
fn release(reg: &mut Registry, actor: i64, order: &Order, now: i64) -> Result<(), ServiceError> {
    if order.bid {
        corp::pay(
            reg,
            order.char_id,
            Wallet::of(order),
            order.escrow,
            (REF_MARKET_ESCROW, ESCROW_OWNER),
            (order.order_id, now),
            false,
        );
    } else if let Some(stack) = escrow_stack(reg, order) {
        let home =
            crate::item::Slot::new(order.char_id, order.station_id, crate::model::FLAG_HANGAR);
        move_goods(reg, actor, stack, i64::MAX, home)?;
    }
    Ok(())
}

/// Close every resting order whose time is up. Lazy: run at the top of every
/// `marketProxy` call rather than on a timer, because `docs/reference/market-browse.md`
/// item 3 is explicit that `OnOwnOrdersChanged` on a server timer is a
/// refresh loop *we* would be driving. **[A]** that nobody notices an order
/// expiring a few minutes late.
fn sweep(reg: &mut Registry, actor: i64, now: i64) -> Result<(), ServiceError> {
    for id in reg.market.due(now) {
        let Some((held, closed)) = reg.market.update(id, |order| {
            // What it held, before the book stops holding it.
            let held = order.clone();
            order.state = ORDER_STATE_EXPIRED;
            order.last_state_change = Some(now);
            order.escrow = 0.0;
            (held, order.clone())
        }) else {
            continue;
        };
        reg.dirty = true;
        release(reg, actor, &held, now)?;
        corp::tell(reg, &closed, vec![own_order_row(&closed)], REASON_EXPIRY);
    }
    Ok(())
}

/// REVIEW-FIX: close every open corporation order whose issuer no longer
/// holds the corporation's trade roles (left, kicked, role revoked, corp
/// closed): the escrow goes back to the order's own division and its journal,
/// goods to the issuer's hangar. Run with [`sweep`], so every path that
/// changes a membership is covered without a hook in each.
fn reconcile_corp_orders(reg: &mut Registry, now: i64) -> Result<(), ServiceError> {
    let lost: Vec<i64> = reg
        .market
        .open()
        .filter(|o| o.corp_id != 0 && !corp::has_access(reg, o))
        .map(|o| o.order_id)
        .collect();
    for id in lost {
        let Some((held, closed)) = reg.market.update(id, |order| {
            let held = order.clone();
            order.state = ORDER_STATE_CANCELLED;
            order.last_state_change = Some(now);
            order.escrow = 0.0;
            (held, order.clone())
        }) else {
            continue;
        };
        reg.dirty = true;
        release(reg, held.char_id, &held, now)?;
        corp::tell(reg, &closed, vec![own_order_row(&closed)], REASON_CANCEL);
    }
    Ok(())
}

// ---- matching ----------------------------------------------------------------

/// Buy `quantity` of `type_id` at up to `price`, from asks in `range` of
/// `place`; rest the remainder for `duration` days, or drop it if 0.
///
/// Validated in full before anything moves, so a refusal changes nothing:
/// the fills, the escrow for the remainder and the broker's fee on it are
/// all checked against the wallet first. Fills are at the **ask's** price
/// and the goods arrive at the ask's station -- standard EVE, **[A]**.
#[allow(clippy::too_many_arguments)]
fn buy(
    reg: &mut Registry,
    actor: i64,
    place: Place,
    type_id: i64,
    price: f64,
    quantity: i64,
    range: i64,
    min_volume: i64,
    duration: i64,
    wallet: Wallet,
    now: i64,
) -> Result<Order, ServiceError> {
    if quantity < 1 || !valid_price(price) {
        return Err(mkt_notify("That is not an order the market can take."));
    }
    // Plan the fills against a read of the book.
    let mut plan: Vec<(Order, i64)> = Vec::new();
    let mut left = quantity;
    for ask in side_with_npc(reg, type_id, place.region_id, false, now) {
        if left == 0 || ask.price > price {
            break;
        }
        let ask_place = Place {
            station_id: ask.station_id,
            solar_system_id: ask.solar_system_id,
            region_id: ask.region_id,
        };
        if !in_range(place, range, ask_place) {
            continue;
        }
        let take = left.min(ask.vol_remaining);
        // O5-e: a bot's fill on a human's ask stops at the day's demand
        // budget (`econ_ledger::demand_take`); bot-bot fills never do.
        let planned: f64 = plan
            .iter()
            .filter(|(a, _)| a.char_id == ask.char_id)
            .map(|(a, q)| *q as f64 * a.price)
            .sum();
        let take =
            crate::econ_ledger::demand_take(reg, actor, ask.char_id, ask.price, take, planned);
        if take == 0 || !min_volume_ok(&ask, take) {
            continue;
        }
        plan.push((ask, take));
        left -= take;
    }
    let filled = quantity - left;
    if duration == 0 && filled == 0 {
        // `MktOrderDidNotMatch`, dialog 1214, info/modal, no args. **[A]**
        // that an immediate order matching nothing is refused rather than
        // silently expired.
        return Err(mkt_error("MktOrderDidNotMatch"));
    }
    let spend: f64 = plan.iter().map(|(a, q)| cents(*q as f64 * a.price)).sum();
    let (escrow, fee) = if duration > 0 && left > 0 {
        check_order_cap(reg, actor, 1)?;
        let escrow = cents(left as f64 * price);
        (escrow, new_order_fee(reg, actor, place.station_id, escrow))
    } else {
        (0.0, 0.0)
    };
    corp::ensure(reg, actor, wallet, spend + escrow, fee)?;

    // **Visibility** (X-TRADE): a resting bid's range reaches only as far as
    // the pilot's Visibility allows (`buyThisTypeWindow._GetAvailableRanges`
    // offers no more); a hand-built call asking for more gets what the client
    // would have offered **[V]**.
    let range = if duration > 0 {
        fees::allowed_buy_range(range, skill_level(reg, actor, fees::TYPE_VISIBILITY))
    } else {
        range
    };
    let order_id = reg.market.mint_order_id();
    // Execute the fills.
    for (ask, take) in &plan {
        let (take, npc) = (*take, exchange::is_npc(ask.order_id));
        let to = corp::goods_slot(wallet, actor, ask.station_id);
        let arrived;
        if npc {
            // An NPC ask **mints** the goods: no escrow stack (C5 §4.1).
            // Packaged, as a market hull always is: an assembled one makes
            // the client ask `cfg.evelocations` for a name nothing primed,
            // and its inventory window stops loading.
            let made = reg
                .apply_items(&[crate::item::Edit::CreatePackaged {
                    type_id,
                    quantity: take,
                    to,
                }])
                .map_err(|_| mkt_error("MktOrderChanged"))?;
            arrived = made.iter().find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            });
            if !exchange::is_dev(ask.order_id) {
                let at = (ask.station_id, type_id, exchange::Side::Ask);
                exchange::take(reg, at, ask.vol_remaining, take, now);
            }
        } else {
            let stack = escrow_stack(reg, ask).ok_or_else(|| mkt_error("MktOrderChanged"))?;
            arrived = move_goods(reg, actor, stack, take, to)?;
        }
        // A personal purchase: `OnMarketItemReceived`, which the client acts
        // on only for another station's hangar. A corporation's lands in
        // deliveries (flag 4 is what the client filters on).
        if !wallet.is_corp()
            && let Some(item_id) = arrived
        {
            reg.push(MarketItemReceived {
                to: actor,
                item_id,
                type_id,
                flag_id: crate::model::FLAG_HANGAR,
                location_id: ask.station_id,
            });
        }
        // A corporation's purchase lands in its deliveries hangar.
        corp::arrived(reg, wallet, ask.station_id);
        let value = cents(take as f64 * ask.price);
        let seller = Wallet::of(ask);
        corp::pay(
            reg,
            actor,
            wallet,
            -value,
            (REF_MARKET_TRANSACTION, seller.owner(ask.char_id)),
            (order_id, now),
            // An ask whose owner is no character (a module's NPC seed,
            // `Registry::seed_ask`) takes the ISK out of the economy, as an
            // NPC ask does; for every character's ask this is `npc` as before.
            npc || (reg.character(ask.char_id).is_none() && !seller.is_corp()),
        );
        // To an NPC corporation this is a no-op: the ISK leaves the economy.
        corp::pay(
            reg,
            ask.char_id,
            seller,
            value,
            (REF_MARKET_TRANSACTION, wallet.owner(actor)),
            (ask.order_id, now),
            false,
        );
        // The seller pays the sales tax on what they sold (CP9).
        let owner = station_owner(reg, ask.station_id);
        charge_sales_tax(reg, ask.char_id, seller, value, owner, ask.order_id, now);
        // The dev seller leaves no trade: the price history, the index and
        // the bots' price memory never see its 100 ISK.
        if !exchange::is_dev(ask.order_id) {
            reg.market.record_trade(Trade {
                transaction_id: 0,
                date: now,
                type_id,
                quantity: take,
                price: ask.price,
                station_id: ask.station_id,
                buyer_id: actor,
                seller_id: ask.char_id,
                region_id: ask.region_id,
                buyer_corp: wallet.corp,
                buyer_key: wallet.key,
                seller_corp: seller.corp,
                seller_key: seller.key,
            });
        }
        if !npc {
            fill(reg, ask.order_id, take, 0.0, now);
        }
    }

    let mut order = Order {
        order_id,
        char_id: actor,
        type_id,
        region_id: place.region_id,
        solar_system_id: place.solar_system_id,
        station_id: place.station_id,
        price,
        vol_entered: quantity,
        vol_remaining: left,
        min_volume: min_volume.max(1),
        bid: true,
        issue_date: now,
        duration,
        corp_id: wallet.corp,
        key_id: wallet.key,
        range,
        escrow,
        state: ORDER_STATE_OPEN,
        last_state_change: None,
    };
    if duration > 0 && left > 0 {
        corp::pay(
            reg,
            actor,
            wallet,
            -escrow,
            (REF_MARKET_ESCROW, ESCROW_OWNER),
            (order_id, now),
            false,
        );
        let station_owner = reg.character(actor).map_or(0, |c| c.station_owner_id);
        corp::pay(
            reg,
            actor,
            wallet,
            -fee,
            (REF_BROKER_FEE, station_owner),
            (order_id, now),
            true,
        );
        reg.market.insert(order.clone());
    } else {
        // Immediate, or a resting order that filled completely on arrival:
        // it is born closed. Only a resting one is kept -- the history
        // window skips `duration < 1` (marketHistory.GetScrollList), so an
        // immediate order has no reader after its push.
        order.state = ORDER_STATE_EXPIRED;
        order.last_state_change = Some(now);
        if duration > 0 {
            reg.market.insert(order.clone());
        }
    }
    reg.dirty = true;
    Ok(order)
}

/// Take `quantity` off a resting order after a fill, spend `escrow_spent`
/// of its escrow, close it if empty, and tell its owner.
fn fill(reg: &mut Registry, order_id: i64, quantity: i64, escrow_spent: f64, now: i64) {
    let Some((closed, snapshot)) = reg.market.update(order_id, |o| {
        o.vol_remaining -= quantity;
        o.escrow = cents(o.escrow - escrow_spent);
        let closed = o.vol_remaining <= 0;
        if closed {
            o.vol_remaining = 0;
            o.state = ORDER_STATE_EXPIRED;
            o.last_state_change = Some(now);
        }
        (closed, o.clone())
    }) else {
        return;
    };
    reg.dirty = true;
    // A bid closed by its last fill may hold a cent of rounding: give it back.
    if closed && snapshot.bid && snapshot.escrow > 0.0 {
        reg.market.update(order_id, |o| o.escrow = 0.0);
        corp::pay(
            reg,
            snapshot.char_id,
            Wallet::of(&snapshot),
            snapshot.escrow,
            (REF_MARKET_ESCROW, ESCROW_OWNER),
            (order_id, now),
            false,
        );
    }
    corp::tell(reg, &snapshot, vec![own_order_row(&snapshot)], REASON_FILL);
}

/// One validated line of a `PlaceMultiSellOrder` item list.
struct SellLine {
    item_id: i64,
    type_id: i64,
    price: f64,
    quantity: i64,
    place: Place,
}

/// Read and check one sell line: the item must be the actor's, of the type
/// the line names, in their hangar at the line's station, packaged, and hold
/// the quantity. Every check is **[A]** in its key; the attribute names are
/// **[V]**, `sellMulti.GetValidatedItem` (sellMulti.py:541-549):
/// `stationID, typeID, itemID, price, quantity, officeID, delta,
/// rawBrokerFeePercentage`.
fn sell_line(reg: &Registry, actor: i64, v: &PyValue) -> Result<SellLine, ServiceError> {
    let item_id = arg_i64(attr(v, "itemID")).ok_or_else(|| mkt_error("MktAsksMustSpecifyItems"))?;
    let station_id = arg_i64(attr(v, "stationID")).unwrap_or(0);
    let type_id = arg_i64(attr(v, "typeID")).unwrap_or(0);
    let price = arg_f64(attr(v, "price")).unwrap_or(0.0);
    let quantity = arg_i64(attr(v, "quantity")).unwrap_or(0);
    let item = reg
        .store()
        .get(item_id)
        .ok_or_else(|| mkt_notify("That item is no longer there."))?;
    if item.singleton {
        // `RepackageBeforeSelling`, dialog 1489, `[item]item.name` -- the
        // key and argument the client itself raises at marketsvc.py:308.
        return Err(ServiceError::Raise(eve_proto::user_error(
            "RepackageBeforeSelling",
            crate::pyutil::pypairs(vec![("item", PyValue::Int(item.type_id))]),
        )));
    }
    if item.owner_id != actor
        || item.type_id != type_id
        || item.location_id != station_id
        || item.flag_id != crate::model::FLAG_HANGAR
    {
        return Err(mkt_notify(
            "Only items in your own station hangar can be sold from here.",
        ));
    }
    // E7's rule (R-ECON §2.4, O5-b): the rookie ship and its fit are minted
    // free, so none may be sold, to a player or an NPC, or it would compete
    // with production. `MktInvalidMarketTypeAsk`, 2127, "The item ... cannot
    // be sold on the market.", `[item]itemType.name` **[V]** key and argument
    // (`findings/user-errors.tsv`); the server raising it is **[A]**.
    let starter = reg.content().get::<crate::starter_ship::Table>().types();
    if starter.binary_search(&type_id).is_ok() {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "MktInvalidMarketTypeAsk",
            crate::pyutil::pypairs(vec![("itemType", PyValue::Int(type_id))]),
        )));
    }
    if quantity < 1 || quantity > item.quantity {
        return Err(mkt_notify("You do not have that many to sell."));
    }
    if !valid_price(price) {
        return Err(mkt_notify("That is not a price the market can take."));
    }
    // **Docked there**, not merely "a station with an order in it": a sell
    // takes goods out of a hangar, and remote selling is the Marketing
    // skill's, which is not modelled -- with no Marketing it is the station
    // you are in **[A]**. `place_of_station`'s remote branch would have let a
    // pilot in space sell from home whenever anything was listed there.
    let place = docked_place(reg, actor)
        .filter(|p| p.station_id == station_id)
        .ok_or_else(|| mkt_notify("You can only sell at the station you are docked in."))?;
    Ok(SellLine {
        item_id,
        type_id,
        price,
        quantity,
        place,
    })
}

/// Sell one validated line: fill bids in range at their price, delivering
/// to the **seller's** station (**[A]**, standard EVE), then rest the
/// remainder as an ask with its goods in escrow, or leave it in the hangar
/// for an immediate sell. Returns whether anything happened.
fn sell(
    reg: &mut Registry,
    actor: i64,
    line: &SellLine,
    duration: i64,
    wallet: Wallet,
    now: i64,
) -> Result<bool, ServiceError> {
    let mut plan: Vec<(Order, i64)> = Vec::new();
    let mut left = line.quantity;
    for bid in side_with_npc(reg, line.type_id, line.place.region_id, true, now) {
        if left == 0 || bid.price < line.price {
            break;
        }
        let bid_place = Place {
            station_id: bid.station_id,
            solar_system_id: bid.solar_system_id,
            region_id: bid.region_id,
        };
        if !in_range(bid_place, bid.range, line.place) {
            continue;
        }
        let take = left.min(bid.vol_remaining);
        if !min_volume_ok(&bid, take) {
            continue;
        }
        plan.push((bid, take));
        left -= take;
    }
    let rest = duration > 0 && left > 0;
    let fee = if rest {
        new_order_fee(
            reg,
            actor,
            line.place.station_id,
            cents(left as f64 * line.price),
        )
    } else {
        0.0
    };
    corp::ensure(reg, actor, wallet, 0.0, fee)?;
    if plan.is_empty() && !rest {
        return Ok(false);
    }

    let order_id = reg.market.mint_order_id();
    for (bid, take) in &plan {
        let (take, bid_price) = (*take, bid.price);
        let npc = exchange::is_npc(bid.order_id);
        if npc {
            // An NPC bid **destroys** the goods: nobody receives them, and
            // the ISK paid below is minted (C5 §4.1).
            let whole = reg
                .store()
                .get(line.item_id)
                .is_some_and(|i| take >= i.quantity);
            let edit = if whole {
                crate::item::Edit::Destroy {
                    item_id: line.item_id,
                }
            } else {
                crate::item::Edit::Consume {
                    item_id: line.item_id,
                    quantity: take,
                }
            };
            reg.apply_items(&[edit])
                .map_err(|_| mkt_error("MktOrderChanged"))?;
            let at = (bid.station_id, line.type_id, exchange::Side::Bid);
            exchange::take(reg, at, bid.vol_remaining, take, now);
        } else {
            let buyer = Wallet::of(bid);
            let to = corp::goods_slot(buyer, bid.char_id, line.place.station_id);
            move_goods(reg, actor, line.item_id, take, to)?;
            corp::arrived(reg, buyer, line.place.station_id);
        }
        let value = cents(take as f64 * bid_price);
        // The buyer already paid, into escrow, when the bid was placed; the
        // fill only spends it. The seller is paid now.
        corp::pay(
            reg,
            actor,
            wallet,
            value,
            (REF_MARKET_TRANSACTION, Wallet::of(bid).owner(bid.char_id)),
            (order_id, now),
            npc,
        );
        let owner = station_owner(reg, line.place.station_id);
        charge_sales_tax(reg, actor, wallet, value, owner, order_id, now);
        reg.market.record_trade(Trade {
            transaction_id: 0,
            date: now,
            type_id: line.type_id,
            quantity: take,
            price: bid_price,
            station_id: line.place.station_id,
            buyer_id: bid.char_id,
            seller_id: actor,
            region_id: line.place.region_id,
            buyer_corp: bid.corp_id,
            buyer_key: bid.key_id,
            seller_corp: wallet.corp,
            seller_key: wallet.key,
        });
        if !npc {
            fill(reg, bid.order_id, take, value, now);
        }
    }
    let mut order = Order {
        order_id,
        char_id: actor,
        type_id: line.type_id,
        region_id: line.place.region_id,
        solar_system_id: line.place.solar_system_id,
        station_id: line.place.station_id,
        price: line.price,
        vol_entered: line.quantity,
        vol_remaining: left,
        min_volume: 1,
        bid: false,
        issue_date: now,
        duration,
        // A sell order sells from its own station only; the column is read
        // by `SortBids` alone, so its value on an ask is never compared.
        corp_id: wallet.corp,
        key_id: wallet.key,
        range: RANGE_STATION,
        escrow: 0.0,
        state: ORDER_STATE_OPEN,
        last_state_change: None,
    };
    if rest {
        let escrow_slot = order.escrow_slot();
        move_goods(reg, actor, line.item_id, left, escrow_slot)?;
        let station_owner = reg.character(actor).map_or(0, |c| c.station_owner_id);
        corp::pay(
            reg,
            actor,
            wallet,
            -fee,
            (REF_BROKER_FEE, station_owner),
            (order_id, now),
            true,
        );
        reg.market.insert(order.clone());
    } else {
        order.state = ORDER_STATE_EXPIRED;
        order.last_state_change = Some(now);
        if duration > 0 {
            reg.market.insert(order.clone());
        }
    }
    reg.dirty = true;
    let reason = if order.is_open() {
        REASON_ADD
    } else {
        REASON_EXPIRY
    };
    corp::tell(reg, &order, vec![own_order_row(&order)], reason);
    Ok(true)
}

/// The acting character's open order, or the refusal the client has a key
/// for. `MktNonExistingOrder` (1209) and `MktNotOwner` (1211), no args;
/// which situation earns which is **[A]**.
fn own_open_order(reg: &Registry, actor: i64, order_id: i64) -> Result<Order, ServiceError> {
    let o = reg
        .market
        .get(order_id)
        .filter(|o| o.is_open())
        .ok_or_else(|| mkt_error("MktNonExistingOrder"))?;
    if !corp::may_manage(reg, actor, o) {
        return Err(mkt_error("MktNotOwner"));
    }
    Ok(o.clone())
}

// ---- the readers' rows --------------------------------------------------------

/// The public book's row for one order: [`market_order_descriptor`]'s
/// fourteen columns, `jumps` a placeholder the client overwrites
/// (marketsvc.py:821).
fn book_row(o: &Order) -> Vec<DbValue> {
    vec![
        DbValue::Int(o.order_id),
        DbValue::Int(o.type_id),
        DbValue::Int(o.region_id),
        DbValue::Int(o.solar_system_id),
        DbValue::Int(o.station_id),
        // **ISK x 10,000**: `DBTYPE_CY`'s getter divides on read
        // (`docs/reference/dbrow.md` §3), the same scaling the journal does.
        DbValue::Int((o.price * 10_000.0).round() as i64),
        DbValue::Int(o.vol_entered),
        DbValue::Real(o.vol_remaining as f64),
        DbValue::Int(o.min_volume),
        DbValue::Bool(o.bid),
        DbValue::Int(o.issue_date),
        DbValue::Int(o.duration),
        DbValue::Int(o.range),
        DbValue::Int(0),
    ]
}

/// The columns of an **own** order: the public fourteen plus what only the
/// owner sees.
///
/// | column | read at | |
/// |---|---|---|
/// | `charID` | orders.py:342 (`if order.charID == session.charid:` gates Cancel and Modify) | **[V]** |
/// | `escrow` | personalMarketOrders.py:56-57 | **[V]** |
/// | `orderState`, `lastStateChange`, `isCorp` | marketHistory.py:263-271, `any(x.isCorp ...)` | **[V]** |
/// | `keyID` | quote.GetWalletDivision (quote.py:521), corp headers only | **[V]** read, value **[A]** |
///
/// The family -- a `CRowset` of `DBRow`s -- is **[A]**: nothing on the own
/// order path reads `.columns` or `.Index` (`docs/reference/market-browse.md` §6 asked
/// exactly that), and a `DBRow` answers every attribute read above as well as
/// the nine `ModifyCharOrder` arguments `marketsvc.ModifyOrder` reads off it
/// (marketsvc.py:333). The same rows are the elements of the
/// `OnOwnOrdersChanged` push.
fn own_order_descriptor() -> DbRowDescriptor {
    let mut columns: Vec<Column> = market_order_descriptor().columns().to_vec();
    for (name, db_type) in [
        ("charID", DbType::I4),
        ("escrow", DbType::Cy),
        ("keyID", DbType::I4),
        ("isCorp", DbType::Bool),
        ("orderState", DbType::I4),
        ("lastStateChange", DbType::FileTime),
    ] {
        columns.push(Column {
            name: name.as_bytes().to_vec(),
            db_type,
        });
    }
    DbRowDescriptor::new(columns).expect("the own-order columns are a valid layout")
}

fn own_order_values(o: &Order) -> Vec<DbValue> {
    let mut v = book_row(o);
    v.extend([
        DbValue::Int(o.char_id),
        DbValue::Int((o.escrow * 10_000.0).round() as i64),
        DbValue::Int(if o.corp_id == 0 {
            ACCOUNT_KEY_CASH
        } else {
            o.key_id
        }),
        DbValue::Bool(o.corp_id != 0),
        DbValue::Int(o.state),
        // `None` for an order still open: marketHistory.py:267 tests
        // `lastStateChange is None` and substitutes the expiry itself.
        o.last_state_change.map_or(DbValue::Null, DbValue::Int),
    ]);
    v
}

/// One own order as a `blue.DBRow`, for the push.
fn own_order_row(o: &Order) -> PyValue {
    PyValue::DbRow(Box::new(
        DbRow::new(own_order_descriptor(), own_order_values(o))
            .expect("an own-order row matches its descriptor"),
    ))
}

fn own_orders_rowset<'a>(orders: impl Iterator<Item = &'a Order>) -> PyValue {
    let mut rowset = eve_rowset::CRowset::new(own_order_descriptor());
    for o in orders {
        rowset
            .push_values(own_order_values(o))
            .expect("an own-order row matches its descriptor");
    }
    rowset.to_pyvalue()
}

/// `(asks, bids)` for one type in one region, as `GetOrders` answers it --
/// before the cache wrapper.
fn order_book(reg: &Registry, type_id: i64, region_id: i64, now: i64) -> PyValue {
    let descriptor = market_order_descriptor();
    let mut halves = Vec::new();
    for bid in [false, true] {
        let mut rowset = eve_rowset::CRowset::new(descriptor.clone());
        for o in side_with_npc(reg, type_id, region_id, bid, now) {
            rowset
                .push_values(book_row(&o))
                .expect("a book row matches market_order_descriptor");
        }
        halves.push(rowset.to_pyvalue());
    }
    PyValue::Tuple(halves)
}

// ---- the GetOrders cache (FX8) -----------------------------------------------

/// How many `(region, type)` books the cache holds before it starts over
/// [A]: a market window shows one type at a time, so a player browses a
/// handful; falsified by a soak whose `GetOrders` rebuilds stay high.
const BOOK_CACHE_MAX: usize = 64;

/// `GetOrders`' marshalled answers per `(region, type)` (port-tracker §78:
/// the region's NPC book was rebuilt on every call, and its transient set
/// off glibc's arenas).
///
/// **Invalidation is by comparison, not by a counter** that every writer
/// would have to bump. An entry is used only when nothing it was built from
/// has changed:
/// - the player orders of that type in that region (both sides), by the
///   book's [`Book::stamp`] for `(region, type)`: [`Book::insert`] and
///   [`Book::update`], the only writers, move it, so a placed, filled,
///   modified, cancelled or expired order rebuilds the book. Checking it is
///   one map read: no order is visited, closed or open (E3);
/// - the NPC side's inputs for that `(region, type)` only
///   ([`exchange::stamp`], E5 P6): its slots' `npcorder` and `npcmult` delta
///   rows, `last_rollover` (each NPC order's `issueDate`, and the econ
///   rollover) and the trickle period `now` falls in. An NPC fill on another
///   type keeps this entry.
///
/// The content tables the NPC book also reads are fixed at boot.
#[derive(Default)]
struct BookCache {
    entries: std::collections::BTreeMap<(i64, i64), CachedBook>,
    /// Books built, for the tests.
    builds: u64,
}

impl Clone for BookCache {
    fn clone(&self) -> Self {
        BookCache::default()
    }
}

impl PartialEq for BookCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for BookCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BookCache({} books)", self.entries.len())
    }
}

/// One cached book: the [`Book::stamp`] of the player orders and the
/// [`exchange::Stamp`] of the NPC orders it was built from, and the answer.
struct CachedBook {
    stamp: u64,
    npc: exchange::Stamp,
    bytes: Vec<u8>,
    version: i64,
}

/// `GetOrders`' answer: [`order_book`] wrapped by [`cached_bytes`], built
/// only when [`BookCache`] holds no current copy.
fn get_orders(reg: &mut Registry, type_id: i64, region_id: i64, now: i64) -> PyValue {
    let key = (region_id, type_id);
    let stamp = reg.market.stamp(region_id, type_id);
    let npc = exchange::stamp(reg, region_id, type_id, now);
    let hit = reg
        .market
        .cache
        .entries
        .get(&key)
        .filter(|e| e.stamp == stamp && e.npc == npc)
        .map(|e| (e.bytes.clone(), e.version));
    let (bytes, version) = hit.unwrap_or_else(|| {
        let bytes = crate::wire::dumps(&order_book(reg, type_id, region_id, now));
        let version = adler32(&bytes);
        let c = &mut reg.market.cache;
        if c.entries.len() >= BOOK_CACHE_MAX && !c.entries.contains_key(&key) {
            c.entries.clear();
        }
        c.builds += 1;
        let entry = CachedBook {
            stamp,
            npc,
            bytes: bytes.clone(),
            version,
        };
        c.entries.insert(key, entry);
        (bytes, version)
    });
    cached_bytes(bytes, version, now)
}

/// The cheapest open ask per type among `orders`.
fn best_asks<'a>(orders: impl Iterator<Item = &'a Order>) -> Vec<&'a Order> {
    // One map lookup per order: the dev catalogue is large and this runs under
    // the world lock.
    let mut at: std::collections::HashMap<i64, usize> = Default::default();
    let mut best: Vec<&Order> = Vec::new();
    for o in orders.filter(|o| !o.bid) {
        match at.get(&o.type_id) {
            Some(&i) if o.price < best[i].price => best[i] = o,
            Some(_) => {}
            None => {
                at.insert(o.type_id, best.len());
                best.push(o);
            }
        }
    }
    best
}

/// `{typeID: (price, volRemaining, typeID, stationID)}` for
/// `GetStationAsks`/`GetSystemAsks`: a **4-sequence**, because
/// `ConvertTuplesToBestByOrders` does `BestByOrder(*order)` and `BestByOrder`
/// is `namedtuple('BestByOrder', ['price', 'volRemaining', 'typeID',
/// 'stationID'])` (marketutil/__init__.py:6). **[V]**
fn ask_tuples<'a>(orders: impl Iterator<Item = &'a Order>) -> PyValue {
    PyValue::Dict(
        best_asks(orders)
            .into_iter()
            .map(|o| {
                (
                    PyValue::Tuple(vec![
                        PyValue::Real(o.price),
                        PyValue::Int(o.vol_remaining),
                        PyValue::Int(o.type_id),
                        PyValue::Int(o.station_id),
                    ]),
                    PyValue::Int(o.type_id),
                )
            })
            .collect(),
    )
}

/// `{typeID: ask}` for `GetRegionBest`. Not converted by the client, read as
/// `price`, `volRemaining` and `stationID`, the last with no default: the
/// browse tree's `FilterItemsForGroupPage` asks for the jumps to it
/// (`docs/reference/market-browse.md` §3), so a `KeyVal` with those names is
/// enough. Family **[A]**, names **[V]**.
fn ask_keyvals<'a>(orders: impl Iterator<Item = &'a Order>) -> PyValue {
    PyValue::Dict(
        best_asks(orders)
            .into_iter()
            .map(|o| {
                (
                    keyval(vec![
                        ("price", PyValue::Real(o.price)),
                        ("volRemaining", PyValue::Int(o.vol_remaining)),
                        ("stationID", PyValue::Int(o.station_id)),
                    ]),
                    PyValue::Int(o.type_id),
                )
            })
            .collect(),
    )
}

/// The market-transaction columns: twelve for `CharGetTransactions`, and for
/// `CorpGetTransactions` (`corp`) a thirteenth, `keyID`. The panel probes
/// `keyID` with `hasattr` (`marketTransactionsPanel.py:43`): without it a
/// corporation row has no `keyString` and the scroll entry raises, and with
/// it a personal row would be read as a corporation's. `docs/reference/
/// wallet-ledger.md` §2.3 has who reads each.
fn transaction_descriptor(corp: bool) -> DbRowDescriptor {
    let mut columns = vec![
        ("transactionID", DbType::I8),
        ("transactionDate", DbType::FileTime),
        ("typeID", DbType::I4),
        ("quantity", DbType::I4),
        ("price", DbType::Cy),
        ("clientID", DbType::I4),
        ("stationID", DbType::I8),
        ("accountID", DbType::I4),
        ("buyerID", DbType::I4),
        ("sellerID", DbType::I4),
        ("buyerAccountID", DbType::I4),
        ("sellerAccountID", DbType::I4),
    ];
    if corp {
        columns.push(("keyID", DbType::I4));
    }
    let columns = columns
        .into_iter()
        .map(|(name, db_type)| Column {
            name: name.as_bytes().to_vec(),
            db_type,
        })
        .collect();
    DbRowDescriptor::new(columns).expect("the market-transaction columns are a valid layout")
}

/// `CharGetTransactions(fromDate)`: the actor's trades, either side.
///
/// `clientID` is overwritten client-side (`marketSvc.py:55-59`) and sent as
/// the counterparty. A side that traded through a corporation wallet sends 0
/// as its account, so the client's `isForCorp` (`accountID != ` that side's
/// account, `marketSvc.py:53-55`) is true and the personal list drops the
/// row. `fromDate` `None` is "everything" **[A]**.
fn transactions_for(book: &Book, actor: i64, from: Option<i64>) -> PyValue {
    let mut rowset = eve_rowset::CRowset::new(transaction_descriptor(false));
    for t in book
        .trades
        .iter()
        .filter(|t| t.buyer_id == actor || t.seller_id == actor)
        .filter(|t| from.is_none_or(|f| t.date >= f))
    {
        let client = if t.buyer_id == actor {
            t.seller_id
        } else {
            t.buyer_id
        };
        rowset
            .push_values(vec![
                DbValue::Int(t.transaction_id),
                DbValue::Int(t.date),
                DbValue::Int(t.type_id),
                DbValue::Int(t.quantity),
                DbValue::Int((t.price * 10_000.0).round() as i64),
                DbValue::Int(client),
                DbValue::Int(t.station_id),
                DbValue::Int(ACCOUNT_KEY_CASH),
                DbValue::Int(t.buyer_id),
                DbValue::Int(t.seller_id),
                DbValue::Int(if t.buyer_corp != 0 {
                    0
                } else {
                    ACCOUNT_KEY_CASH
                }),
                DbValue::Int(if t.seller_corp != 0 {
                    0
                } else {
                    ACCOUNT_KEY_CASH
                }),
            ])
            .expect("a transaction row matches its descriptor");
    }
    rowset.to_pyvalue()
}

// ---- the cache wrapper ----------------------------------------------------------

// ---- pushes ----------------------------------------------------------------------

/// `OnOwnOrdersChanged(orders, reason, isCorp)`, addressed to the order's
/// owner.
///
/// **`orders` must be iterable, and `None` is not legal** -- this corrects
/// `docs/reference/push-events.md` and `docs/reference/market-browse.md`, which both say a `None`
/// payload "just invalidates". It does in the guarded loop at
/// marketsvc.py:131, and then line 143 calls
/// `_IsAtLeastOneNonImmediate(orders)`, which iterates it unguarded
/// (marketsvc.py:148) -- `TypeError` inside the notify chain. **[V]**
///
/// Each element is read for `.typeID` (marketsvc.py:133,
/// marketbase.py:775), `.duration` (marketsvc.py:148), `.orderID` and
/// `.volRemaining` (buyMultiFromBase.py:588, :598). An own-order `DBRow`
/// carries all four. `isCorp` is discarded by every handler; `False`.
#[derive(Debug, Clone, PartialEq)]
struct OwnOrdersChanged {
    to: i64,
    orders: Vec<PyValue>,
    reason: &'static str,
}

impl crate::notify::Push for OwnOrdersChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnOwnOrdersChanged",
            vec![
                PyValue::List(self.orders.clone()),
                crate::pyutil::bstr(self.reason),
                PyValue::Bool(false),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnAccountChange`, addressed. `notify::AccountChange` carries the shape
/// and its evidence; this only adds the audience, because a fill pays a
/// character who is not the one calling.
#[derive(Debug, Clone, PartialEq)]
struct WalletChanged {
    to: i64,
    change: crate::notify::AccountChange,
}

impl crate::notify::Push for WalletChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        crate::notify::Push::to_push(&self.change)
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `ixOwnerID`, `ixLocationID`, `ixFlag`: the positions of those columns in
/// `service.rs::item_columns`, which `notify::ItemChange`'s doc pins against
/// the client's hard-coded indices.
const ITEM_IX_OWNER: i64 = 2;
const ITEM_IX_LOCATION: i64 = 3;
const ITEM_IX_FLAG: i64 = 4;

/// `OnItemChange(item, change, None)`, addressed, with a change dict of our
/// choosing.
///
/// `InventorySvc.OnItemChange` builds `old = blue.DBRow(item)` and then
/// `old[k] = v` for every entry of `change` (inventorysvc.py:65-71), and the
/// station hangar's `IsItemHere` is `locationID == session.stationid and
/// ownerID == session.charid and flagID == flagHangar`
/// (invControllers.py:1115). So naming the old owner, location and flag
/// makes `wasHere` true and `isHere` false -- a removal, which
/// `notify::ItemChange`'s fixed `{3: 0}` cannot say. **[V]** mechanism.
#[derive(Debug, Clone, PartialEq)]
struct ItemMoved {
    to: i64,
    row: PyValue,
    change: Vec<(i64, i64)>,
}

impl crate::notify::Push for ItemMoved {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnItemChange",
            vec![
                self.row.clone(),
                // `(value, key)`: marshal dict entries are inverted.
                PyValue::Dict(
                    self.change
                        .iter()
                        .map(|&(k, v)| (PyValue::Int(v), PyValue::Int(k)))
                        .collect(),
                ),
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnMarketItemReceived(itemID, typeID, flag, locationID)` [V,
/// `marketsvc.py:134`]: the client marks the item unseen only for a buy
/// delivered to another station's hangar, and ignores the local ones itself
/// (`_is_remote_item_purchase_in_hangar`). That TQ sends it for every buy is
/// `[A]` (EvEJS does); falsifier: local buys badge without it.
#[derive(Debug, Clone, PartialEq)]
struct MarketItemReceived {
    to: i64,
    item_id: i64,
    type_id: i64,
    flag_id: i64,
    location_id: i64,
}

impl crate::notify::Push for MarketItemReceived {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnMarketItemReceived",
            vec![
                PyValue::Int(self.item_id),
                PyValue::Int(self.type_id),
                PyValue::Int(self.flag_id),
                PyValue::Int(self.location_id),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

/// `OnInvalidateCacheForLocations`, addressed. Shape from
/// `notify::InvalidateLocations`.
#[derive(Debug, Clone, PartialEq)]
struct LocationsChanged {
    to: i64,
    location_ids: Vec<i64>,
}

impl crate::notify::Push for LocationsChanged {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        crate::notify::Push::to_push(&crate::notify::InvalidateLocations {
            location_ids: self.location_ids.clone(),
        })
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }
}

// ---- the mutator arms ----------------------------------------------------------

/// The acting character, or the refusal for a call with none.
fn actor_of(req: &Request<'_>) -> Result<i64, ServiceError> {
    req.actor
        .ok_or_else(|| mkt_notify("The market needs a character."))
}

/// `PlaceBuyOrder(stationID, typeID, price, quantity, orderRange, minVolume,
/// duration, useCorp, expectedBrokersFee)` -- nine positional arguments,
/// marketsvc.py:313, **[V]**. The return is discarded (`POP_TOP` at :313),
/// so the whole visible effect is the pushes. `duration == 0` is an
/// immediate buy (buyThisTypeWindow.Buy sets it so), which ends in one
/// `'Expiry'` push; a resting order gets `'Add'`.
fn place_buy_order(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let actor = actor_of(req)?;
    let a = req.args;
    let station_id = arg_i64(a.first()).unwrap_or(0);
    let type_id = arg_i64(a.get(1)).unwrap_or(0);
    let price = arg_f64(a.get(2)).unwrap_or(0.0);
    let quantity = arg_i64(a.get(3)).unwrap_or(0);
    let range = arg_i64(a.get(4)).unwrap_or(RANGE_STATION);
    let min_volume = arg_i64(a.get(5)).unwrap_or(1);
    let duration = arg_i64(a.get(6)).unwrap_or(0).max(0);
    // `expectedBrokersFee` (a.get(8)) is not read: the server's fee is charged.
    // `useCorp` (a.get(7)): the corporation's active division pays (CORP-BUY).
    let wallet = corp::wallet_for(reg, actor, a.get(7))?;
    let place = place_of_station(reg, actor, station_id).ok_or_else(|| {
        mkt_notify("You can only place buy orders at the station you are docked in.")
    })?;
    let order = buy(
        reg, actor, place, type_id, price, quantity, range, min_volume, duration, wallet, now,
    )?;
    let reason = if duration == 0 || order.vol_remaining == 0 {
        REASON_EXPIRY
    } else {
        REASON_ADD
    };
    corp::tell(reg, &order, vec![own_order_row(&order)], reason);
    Ok(PyValue::None)
}

/// `BuyMultipleItems(stationID, itemList, useCorp)`, marketsvc.py:317, each
/// item `GetBuyItemInfo`'s `KeyVal(stationID, typeID, price, quantity,
/// minVolume, delta)` (marketutil/orderInfo.py). **[V]**
///
/// Every line is an **immediate** buy at the station (**[A]** range: the
/// window prices from `GetBestAskPriceInStationAndNumberOrders`,
/// buytItemEntry.py:50, which is station-only). The answer is the created
/// orders, iterated for `.typeID` (marketsvc.py:318) and `.orderID`
/// (buyMultiFromBase.py:331) -- and then **every one of them must come back
/// in an `'Expiry'` push**, because that push is the only thing that clears
/// `activeOrders` and re-enables the window
/// (buyMultiFromBase.py:583-611). `volRemaining` on it is what did not fill,
/// and the window re-adds exactly that as a failed line (:598-599). So a
/// line that matched nothing is not refused, it is expired whole.
///
/// The whole list is costed against the wallet first; a short wallet
/// refuses everything rather than buying a prefix of it.
fn buy_multiple_items(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let actor = actor_of(req)?;
    let station_id = arg_i64(req.args.first()).unwrap_or(0);
    let wallet = corp::wallet_for(reg, actor, req.args.get(2))?;
    let place = place_of_station(reg, actor, station_id)
        .ok_or_else(|| mkt_notify("You can only buy at the station you are docked in."))?;
    let lines: Vec<(i64, f64, i64, i64)> = seq(req.args.get(1))
        .iter()
        .map(|v| {
            (
                arg_i64(attr(v, "typeID")).unwrap_or(0),
                arg_f64(attr(v, "price")).unwrap_or(0.0),
                arg_i64(attr(v, "quantity")).unwrap_or(0),
                arg_i64(attr(v, "minVolume")).unwrap_or(1),
            )
        })
        .collect();
    // Cost every line at its limit price: an upper bound on what the fills
    // can spend, so the check cannot pass and a later fill then overdraw.
    let worst: f64 = lines.iter().map(|&(_, p, q, _)| cents(p * q as f64)).sum();
    corp::ensure(reg, actor, wallet, worst, 0.0)?;
    let mut created = Vec::new();
    let mut first: Option<Order> = None;
    for (type_id, price, quantity, min_volume) in lines {
        let order = match buy(
            reg,
            actor,
            place,
            type_id,
            price,
            quantity,
            RANGE_STATION,
            min_volume,
            0,
            wallet,
            now,
        ) {
            Ok(order) => order,
            // Nothing matched: the order still exists for the window, whole.
            Err(_) => Order {
                order_id: reg.market.mint_order_id(),
                char_id: actor,
                type_id,
                region_id: place.region_id,
                solar_system_id: place.solar_system_id,
                station_id: place.station_id,
                price,
                vol_entered: quantity,
                vol_remaining: quantity.max(0),
                min_volume: min_volume.max(1),
                bid: true,
                issue_date: now,
                duration: 0,
                range: RANGE_STATION,
                escrow: 0.0,
                state: ORDER_STATE_EXPIRED,
                last_state_change: Some(now),
                corp_id: wallet.corp,
                key_id: wallet.key,
            },
        };
        created.push(own_order_row(&order));
        first.get_or_insert(order);
    }
    if let Some(first) = first {
        corp::tell(reg, &first, created.clone(), REASON_EXPIRY);
    }
    Ok(PyValue::List(created))
}

/// `PlaceMultiSellOrder(itemList, useCorp, duration, expectedBrokersFee)`,
/// marketsvc.py:322, read **only for truth**: `if wereOrdersCreated:
/// self.Close()` (sellMulti.py:509). So `True` when anything sold or rested,
/// `False` otherwise -- shape **[A]**, truthiness **[V]**.
///
/// Every line is validated before any line trades, so one bad stack refuses
/// the batch instead of selling half of it. An immediate sell (duration 0)
/// that matched no bid at all is `MktOrderDidNotMatch` **[A]**, the same
/// refusal as the immediate buy.
fn place_multi_sell_order(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let actor = actor_of(req)?;
    let duration = arg_i64(req.args.get(2)).unwrap_or(0).max(0);
    // `useCorp` (args[1]): the corporation's active division takes the fees and
    // the proceeds; the goods are still the pilot's (CORP-BUY **[A]**).
    let wallet = corp::wallet_for(reg, actor, req.args.get(1))?;
    // `expectedBrokersFee` (args[3]) is not read: the server's fee is charged.
    let lines = seq(req.args.first())
        .iter()
        .map(|v| sell_line(reg, actor, v))
        .collect::<Result<Vec<_>, _>>()?;
    if lines.is_empty() {
        return Err(mkt_error("MktAsksMustSpecifyItems"));
    }
    // Two lines naming one stack would sell it twice.
    for (i, l) in lines.iter().enumerate() {
        if lines[..i].iter().any(|m| m.item_id == l.item_id) {
            return Err(mkt_notify("The same stack is listed twice."));
        }
    }
    // Every line may rest, so the cap is checked for all of them before any
    // trades **[A]**: a line that would fill whole still counts.
    if duration > 0 {
        check_order_cap(reg, actor, lines.len())?;
    }
    let mut any = false;
    for line in &lines {
        any |= sell(reg, actor, line, duration, wallet, now)?;
    }
    if !any && duration == 0 {
        return Err(mkt_error("MktOrderDidNotMatch"));
    }
    Ok(PyValue::Bool(any))
}

/// `ModifyCharOrder(orderID, newPrice, bid, stationID, solarSystemID, price,
/// range, volRemaining, issueDate)`, marketsvc.py:333 -- nine positional,
/// read off the client's own row of the order. **[V]** Return discarded.
///
/// The client sends what it *thinks* the order is (`price`,
/// `volRemaining`); if either moved since -- a fill landed while the dialog
/// was open -- the change is refused with `MktOrderChanged` (1212), whose
/// name says exactly that. **[A]** on using it so. A buy order's escrow is
/// re-sized to the new price, debiting or refunding the difference. **[A]**:
/// no matching on modify, issue date unchanged. The broker charges the
/// modification fee ([`modification_fee`], X-TRADE). A
/// modify inside `mktModificationDelay` of the last one is refused
/// ([`modification_wait`], E5).
fn modify_char_order(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let actor = actor_of(req)?;
    let a = req.args;
    let order_id = arg_i64(a.first()).unwrap_or(0);
    let new_price = cents(arg_f64(a.get(1)).unwrap_or(0.0));
    let order = own_open_order(reg, actor, order_id)?;
    if let Some(seen) = arg_f64(a.get(5))
        && cents(seen) != cents(order.price)
    {
        return Err(mkt_error("MktOrderChanged"));
    }
    if let Some(seen) = arg_i64(a.get(7))
        && seen != order.vol_remaining
    {
        return Err(mkt_error("MktOrderChanged"));
    }
    if !valid_price(new_price) {
        return Err(mkt_notify("That is not a price the market can take."));
    }
    let wait = modification_wait(&order, now, crate::game_clock::of(reg));
    if wait > 0 {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "MktOrderDelay",
            crate::pyutil::pypairs(vec![("delay", crate::pyutil::bstr(&wait_text(wait)))]),
        )));
    }
    // The broker's modification fee (`brokerFee.GetBrokerFeeInfo` with an
    // old amount, X-TRADE) **[V]**: charged on top of any escrow growth, and
    // checked with it so a refusal changes nothing.
    let old_amount = cents(order.vol_remaining as f64 * order.price);
    let new_amount = cents(order.vol_remaining as f64 * new_price);
    let fee = modification_fee(reg, actor, order.station_id, old_amount, new_amount);
    let wallet = Wallet::of(&order);
    if order.bid {
        let new_escrow = cents(order.vol_remaining as f64 * new_price);
        let delta = cents(new_escrow - order.escrow);
        corp::ensure(reg, actor, wallet, delta, fee)?;
        corp::pay(
            reg,
            order.char_id,
            wallet,
            -delta,
            (REF_MARKET_ESCROW, ESCROW_OWNER),
            (order_id, now),
            false,
        );
        reg.market.update(order_id, |o| o.escrow = new_escrow);
    } else {
        corp::ensure(reg, actor, wallet, 0.0, fee)?;
    }
    let owner = station_owner(reg, order.station_id);
    corp::pay(
        reg,
        order.char_id,
        wallet,
        -fee,
        (REF_BROKER_FEE, owner),
        (order_id, now),
        true,
    );
    let Some(row) = reg.market.update(order_id, |o| {
        o.price = new_price;
        // The last operation on an open order, for the next modify's wait.
        o.last_state_change = Some(now);
        own_order_row(o)
    }) else {
        return Err(mkt_error("MktNonExistingOrder"));
    };
    reg.dirty = true;
    corp::tell(reg, &order, vec![row], REASON_MODIFY);
    Ok(PyValue::None)
}

/// Seconds before `order` may be modified again at `now`: `mktModificationDelay`
/// **[V]** after its last operation, which is its last modify, else its
/// issue **[A]**: `MktOrderDelay`'s text is "so soon after your last
/// operation on it". An open order's `lastStateChange` is otherwise `None`,
/// and only the history window reads it (marketHistory.py:267), so a modify
/// keeps its time there; falsified by a TQ capture that moves `issueDate`
/// on a modify instead.
///
/// Lane clock: the delay is `pace.span(MODIFICATION_DELAY_S)`, so five
/// minutes at scale 1 and five seconds at scale 60.
fn modification_wait(order: &Order, now: i64, pace: crate::game_clock::GameClock) -> i64 {
    let last = order.last_state_change.unwrap_or(order.issue_date);
    let ready = last + pace.span(MODIFICATION_DELAY_S * 10_000_000);
    ((ready - now).max(0) + 9_999_999) / 10_000_000
}

/// `MktOrderDelay`'s `{delay}`, a plain token (`findings/user-errors.tsv`):
/// minutes and seconds **[A]** in its wording.
fn wait_text(secs: i64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} seconds"),
        (m, 0) => format!("{m} minutes"),
        (m, s) => format!("{m} minutes {s} seconds"),
    }
}

/// `CancelCharOrder(orderID, regionID)`, marketsvc.py:328, from
/// `quote.CancelOffer` (quote.py:328) after the `CancelMarketOrder` dialog.
/// **[V]** Return discarded. The order closes as `ORDER_STATE_CANCELLED`, its
/// escrow or goods go home, and its owner is told.
fn cancel_char_order(
    reg: &mut Registry,
    req: &Request<'_>,
    now: i64,
) -> Result<PyValue, ServiceError> {
    let actor = actor_of(req)?;
    let order_id = arg_i64(req.args.first()).unwrap_or(0);
    let order = own_open_order(reg, actor, order_id)?;
    reg.market.update(order_id, |o| {
        o.state = ORDER_STATE_CANCELLED;
        o.last_state_change = Some(now);
        o.escrow = 0.0;
    });
    reg.dirty = true;
    release(reg, actor, &order, now)?;
    let closed = reg.market.get(order_id).cloned().unwrap_or(order);
    corp::tell(reg, &closed, vec![own_order_row(&closed)], REASON_CANCEL);
    Ok(PyValue::None)
}

/// Every open ask in the browsing region, the players' and the NPCs'; nothing
/// without a place.
fn region_asks(reg: &Registry, home: Option<Place>, now: i64) -> Vec<Order> {
    let Some(h) = home else {
        return Vec::new();
    };
    let mut v: Vec<Order> = reg
        .market
        .open()
        .filter(|o| !o.bid && o.region_id == h.region_id)
        .cloned()
        .collect();
    v.extend(npc_side(reg, h.region_id, None, false, now));
    if reg.config().dev {
        v.extend(dev_asks(
            reg,
            h.region_id,
            Some(h.station_id).filter(|s| *s != 0),
            None,
        ));
    }
    v
}

/// Every `marketProxy` arm that reads or writes the book. `None` for a
/// method this does not own, so the caller's `match` keeps the rest.
fn book_call(reg: &mut Registry, req: &Request<'_>) -> Option<Result<PyValue, ServiceError>> {
    let now = crate::pyutil::filetime_now();
    let owned = matches!(
        req.method,
        "GetOrders"
            | "GetRegionBest"
            | "GetStationAsks"
            | "GetSystemAsks"
            | "GetCharOrders"
            | "GetMarketOrderHistory"
            | "CharGetTransactions"
            | "GetCharEscrow"
            | "PlaceBuyOrder"
            | "BuyMultipleItems"
            | "PlaceMultiSellOrder"
            | "ModifyCharOrder"
            | "CancelCharOrder"
            | "GetCorporationOrders"
    );
    if !owned {
        return None;
    }
    if let Err(e) = sweep(reg, req.actor.unwrap_or(0), now) {
        return Some(Err(e));
    }
    if let Err(e) = reconcile_corp_orders(reg, now) {
        return Some(Err(e));
    }
    let home = req.actor.and_then(|a| browse_place(reg, a));
    let actor = req.actor.unwrap_or(0);
    Some(match req.method {
        "GetOrders" => {
            let type_id = arg_i64(req.args.first()).unwrap_or(0);
            let region = home.map_or(0, |h| h.region_id);
            Ok(get_orders(reg, type_id, region, now))
        }
        // The three ask lookups merge the region's NPC asks (C5 §4.6).
        "GetRegionBest" => {
            let asks = region_asks(reg, home, now);
            Ok(ask_keyvals(asks.iter()))
        }
        "GetSystemAsks" => {
            let asks = region_asks(reg, home, now);
            Ok(ask_tuples(asks.iter().filter(|o| {
                home.is_some_and(|h| o.solar_system_id == h.solar_system_id)
            })))
        }
        "GetStationAsks" => {
            let asks = region_asks(reg, home, now);
            Ok(ask_tuples(asks.iter().filter(|o| {
                home.is_some_and(|h| o.station_id == h.station_id)
            })))
        }
        // A pilot's own orders; the corporation's are `GetCorporationOrders`.
        "GetCharOrders" => Ok(own_orders_rowset(
            reg.market
                .open()
                .filter(|o| o.char_id == actor && o.corp_id == 0),
        )),
        "GetMarketOrderHistory" => Ok(own_orders_rowset(
            reg.market.closed().filter(|o| o.char_id == actor),
        )),
        "CharGetTransactions" => Ok(transactions_for(
            &reg.market,
            actor,
            arg_i64(req.args.first()),
        )),
        // `GetMyMarketEscrow` (marketsvc.py:448) reads both attributes
        // through `float(x or 0)`. `itemsEscrow` is the asking value of the
        // goods on sale **[A]**: the name says items, and ISK is the only
        // unit the caller adds it to.
        "GetCharEscrow" => {
            let mine = || {
                reg.market
                    .open()
                    .filter(|o| o.char_id == actor && o.corp_id == 0)
            };
            let isk: f64 = mine().filter(|o| o.bid).map(|o| o.escrow).sum();
            let items: f64 = mine()
                .filter(|o| !o.bid)
                .map(|o| o.price * o.vol_remaining as f64)
                .sum();
            Ok(keyval(vec![
                ("iskEscrow", PyValue::Real(cents(isk))),
                ("itemsEscrow", PyValue::Real(cents(items))),
            ]))
        }
        // The corporation's orders, for an accountant (the client builds the
        // tab for no one else, marketOrdersWnd.py:34); CORP-BUY.
        "GetCorporationOrders" => Ok(corp::corporation_orders(reg, actor)),
        "PlaceBuyOrder" => place_buy_order(reg, req, now),
        "BuyMultipleItems" => buy_multiple_items(reg, req, now),
        "PlaceMultiSellOrder" => place_multi_sell_order(reg, req, now),
        "ModifyCharOrder" => modify_char_order(reg, req, now),
        "CancelCharOrder" => cancel_char_order(reg, req, now),
        _ => return None,
    })
}

// ---- E1: the offline agents' two doors into the book -----------------------
//
// `crate::econ_agents` settles a logged-off bot's career through the same
// book a client call reaches (R-ECON §1.5: "settlement must use the same pure
// rules ... market fills through `service/market.rs`"). These are the whole
// of its surface here: an immediate sell into the bids at the seller's own
// station, and an immediate buy from the asks there. Both are the arms'
// bodies with the client's argument parsing taken off, so the sales tax, the
// NPC-bid destruction, the demand budget and the trade record are the live
// ones; neither rests an order (no broker fee, no escrow).

impl Registry {
    /// Sell up to `quantity` of the stack `item_id` (the actor's, in the
    /// hangar of the station it is docked in) into the bids at or above
    /// `floor` per unit. `Ok(true)` when anything filled. The caller reads
    /// what moved from the stack and the wallet.
    pub(crate) fn settle_sell(
        &mut self,
        actor: i64,
        item_id: i64,
        quantity: i64,
        floor: f64,
        now: i64,
    ) -> Result<bool, ServiceError> {
        let Some(item) = self.store().get(item_id).cloned() else {
            return Err(mkt_notify("That item is no longer there."));
        };
        let station = self.docked_station(actor).unwrap_or(0);
        let v = crate::pyutil::pydict(vec![
            ("itemID", PyValue::Int(item_id)),
            ("stationID", PyValue::Int(station)),
            ("typeID", PyValue::Int(item.type_id)),
            ("price", PyValue::Real(floor)),
            ("quantity", PyValue::Int(quantity.min(item.quantity))),
        ]);
        let line = sell_line(self, actor, &v)?;
        sell(self, actor, &line, 0, Wallet::PILOT, now)
    }

    /// List `quantity` of the stack `item_id` as an ask at `price` for `days`
    /// days (the broker's fee, the order cap and the escrow are the live
    /// ones), filling any bid at or above `price` first. `Ok(true)` when
    /// anything happened.
    pub(crate) fn settle_list(
        &mut self,
        actor: i64,
        item_id: i64,
        quantity: i64,
        price: f64,
        days: i64,
        now: i64,
    ) -> Result<bool, ServiceError> {
        check_order_cap(self, actor, 1)?;
        let Some(item) = self.store().get(item_id).cloned() else {
            return Err(mkt_notify("That item is no longer there."));
        };
        let station = self.docked_station(actor).unwrap_or(0);
        let v = crate::pyutil::pydict(vec![
            ("itemID", PyValue::Int(item_id)),
            ("stationID", PyValue::Int(station)),
            ("typeID", PyValue::Int(item.type_id)),
            ("price", PyValue::Real(price)),
            ("quantity", PyValue::Int(quantity.min(item.quantity))),
        ]);
        let line = sell_line(self, actor, &v)?;
        sell(self, actor, &line, days.max(1), Wallet::PILOT, now)
    }

    /// **A module's finite NPC stock** (`eve-mod-econ`'s PLEX seed): rest one
    /// ask of `quantity` x `type_id` at `price` in `station_id`, owned by the
    /// station's NPC corporation, for `days` days, and make the goods in its
    /// escrow stack. Returns `(order id, owning corporation)`. The goods are
    /// **new**: the caller ledgers them. It never restocks (there is no
    /// refill row), and a buyer's ISK is paid to a corporation that has no
    /// wallet, so it leaves the economy as a sink. `Err` names why not (no
    /// such station, no NPC owner, a bad price or quantity, the store
    /// refusing the type), and nothing was made. `near` is a character at
    /// that station (whose home gives its place); `0` for none, when the
    /// exchange table must know the station. It is issued at the wall clock.
    pub fn seed_ask(
        &mut self,
        near: i64,
        station_id: i64,
        type_id: i64,
        quantity: i64,
        price: f64,
        days: i64,
    ) -> Result<(i64, i64), String> {
        let now = crate::pyutil::filetime_now();
        if quantity < 1 || days < 1 || !valid_price(price) {
            return Err("a seed ask needs a quantity, a duration and a price".to_owned());
        }
        let owner = station_owner(self, station_id);
        if owner == 0 {
            return Err(format!("station {station_id} has no NPC owner"));
        }
        let place = place_of_station(self, near, station_id)
            .ok_or_else(|| format!("station {station_id} is not in the market"))?;
        let order_id = self.market.mint_order_id();
        let order = Order {
            order_id,
            char_id: owner,
            type_id,
            region_id: place.region_id,
            solar_system_id: place.solar_system_id,
            station_id,
            price: cents(price),
            vol_entered: quantity,
            vol_remaining: quantity,
            min_volume: 1,
            bid: false,
            issue_date: now,
            duration: days,
            range: RANGE_STATION,
            escrow: 0.0,
            state: ORDER_STATE_OPEN,
            last_state_change: None,
            corp_id: 0,
            key_id: 0,
        };
        self.apply_items_quiet(&[crate::item::Edit::Create {
            type_id,
            quantity,
            to: order.escrow_slot(),
        }])
        .map_err(|e| format!("the store refused the seed stack: {e:?}"))?;
        self.market.insert(order);
        self.dirty = true;
        Ok((order_id, owner))
    }

    /// The sales tax rate `actor` pays on a sale: the live formula
    /// ([`sales_tax`] per ISK) at the actor's Accounting level.
    pub(crate) fn settle_sales_tax_rate(&self, actor: i64) -> f64 {
        sales_tax(1.0, skill_level(self, actor, TYPE_ACCOUNTING))
    }

    /// Whether `actor` has an open ask for `type_id` at `station`: one ask
    /// per (type, station) is the live builder's rule.
    pub(crate) fn settle_has_ask(&self, actor: i64, type_id: i64, station: i64) -> bool {
        self.market.open().any(|o| {
            o.char_id == actor
                && !o.bid
                && o.is_open()
                && o.type_id == type_id
                && o.station_id == station
        })
    }

    /// Buy up to `quantity` of `type_id` from the asks in the actor's own
    /// station at or under `cap` per unit; the goods arrive in its hangar.
    /// `Ok(false)` when nothing matched.
    pub(crate) fn settle_buy(
        &mut self,
        actor: i64,
        type_id: i64,
        cap: f64,
        quantity: i64,
        now: i64,
    ) -> Result<bool, ServiceError> {
        let Some(place) = docked_place(self, actor) else {
            return Err(mkt_notify(
                "You can only buy at the station you are docked in.",
            ));
        };
        match buy(
            self,
            actor,
            place,
            type_id,
            cap,
            quantity,
            RANGE_STATION,
            1,
            0,
            Wallet::PILOT,
            now,
        ) {
            Ok(_) => Ok(true),
            Err(ServiceError::Raise(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]
    use super::*;
    use crate::pyutil::CACHED_METHOD_CALL_RESULT;

    /// Lane clock: scale 1, what every test here ran at before.
    const REAL: crate::game_clock::GameClock = crate::game_clock::GameClock::REAL;

    /// The cheapest ask per type survives, in first-seen order, and bids
    /// never count.
    #[test]
    fn best_asks_keeps_the_cheapest_per_type_in_first_seen_order() {
        let o = |order_id: i64, type_id: i64, price: f64, bid: bool| Order {
            order_id,
            char_id: 1,
            type_id,
            region_id: 2,
            solar_system_id: 3,
            station_id: 4,
            price,
            vol_entered: 1,
            vol_remaining: 1,
            min_volume: 1,
            bid,
            issue_date: 0,
            duration: 1,
            range: 0,
            escrow: 0.0,
            state: ORDER_STATE_OPEN,
            last_state_change: None,
            corp_id: 0,
            key_id: 0,
        };
        let orders = [
            o(1, 70, 30.0, false),
            o(2, 60, 5.0, false),
            o(3, 70, 10.0, false),
            o(4, 70, 20.0, false),
            o(5, 60, 1.0, true),
        ];
        let best = best_asks(orders.iter());
        let got: Vec<(i64, i64)> = best.iter().map(|b| (b.type_id, b.order_id)).collect();
        assert_eq!(got, vec![(70, 3), (60, 2)]);
    }

    /// Every service this module advertises must reach an arm in *this*
    /// module's `dispatch`.
    #[test]
    fn every_service_here_is_routed_to_this_module() {
        let mut reg = Registry::new(SandboxConfig::default());
        for s in SERVICES {
            let req = Request {
                service: s,
                method: "__no_such_method_exists__",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: None,
            };
            match dispatch(&mut reg, &req) {
                Err(ServiceError::NoSuchMethod { .. }) => {}
                other => panic!(
                    "{s} is advertised by this module but its dispatch answers {other:?} \
                     to a method that cannot exist"
                ),
            }
        }
    }

    /// Call one `marketProxy` method with no arguments.
    fn call(method: &str) -> PyValue {
        let mut reg = Registry::new(SandboxConfig::default());
        let req = Request {
            service: "marketProxy",
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        let v = dispatch(&mut reg, &req)
            .unwrap_or_else(|e| panic!("marketProxy.{method} answered {e:?}"));
        unwrap_cached(v)
    }

    /// What the client's `GetResult` hands its caller: the inner value of a
    /// `CachedMethodCallResult`, or the value itself. `blue.marshal.Load` of
    /// the second state element (objectCaching.py:880), done with our own
    /// decoder -- so every shape test below also proves the bytes decode.
    fn unwrap_cached(v: PyValue) -> PyValue {
        match &v {
            PyValue::Instance { class, state } if class == CACHED_METHOD_CALL_RESULT.as_bytes() => {
                let PyValue::Tuple(parts) = state.as_ref() else {
                    panic!("a CachedMethodCallResult's state is its __getstate__ triple");
                };
                let PyValue::Bytes(bytes) = &parts[1] else {
                    panic!("the result travels as marshal bytes");
                };
                crate::wire::loads(bytes).expect("the cached result decodes")
            }
            _ => v,
        }
    }

    /// The class name out of a `0x22`/`0x23` ObjectEx header, so a test can
    /// say "this is a CRowset" without duplicating the encoder.
    fn class_of(v: &PyValue) -> Option<String> {
        let PyValue::ObjectEx { header, .. } = v else {
            return None;
        };
        let callable = match header.as_ref() {
            PyValue::Tuple(items) => items.first()?,
            _ => return None,
        };
        match callable {
            PyValue::Global(name) => Some(String::from_utf8_lossy(name).into_owned()),
            PyValue::Tuple(items) => match items.first()? {
                PyValue::Global(name) => Some(String::from_utf8_lossy(name).into_owned()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Issue #2. `RefreshOrderCache` reads `orders[0]` and `orders[1]`
    /// (marketsvc.py:863) and then `len()` of each (marketsvc.py:866), so
    /// the arity is not negotiable: a 1-sequence is an `IndexError` where
    /// `None` was a `TypeError`, and the cache is written at line 864 either
    /// way only if both reads succeed.
    #[test]
    fn get_orders_is_a_two_sequence() {
        for method in ["GetOrders", "GetPlexOrders"] {
            match call(method) {
                PyValue::Tuple(items) => assert_eq!(
                    items.len(),
                    2,
                    "{method} must answer exactly (asks, bids); marketsvc.py:863 \
                     reads index 0 and index 1 and nothing else"
                ),
                other => panic!("{method} must answer a 2-sequence, not {other:?}"),
            }
        }
    }

    /// Both halves must be `CRowset`s, because `RefreshJumps` reads
    /// `asks.columns` while building the sort lambda's default argument and
    /// then calls `asks.sort(cmp)` (marketsvc.py:825). `CRowset` subclasses
    /// `list` *and* exposes `columns`; a plain list, a tuple and the
    /// old-style `Rowset` each fail one of the two.
    #[test]
    fn both_halves_of_the_order_book_are_crowsets() {
        for method in ["GetOrders", "GetPlexOrders"] {
            let PyValue::Tuple(items) = call(method) else {
                panic!("{method} must answer a 2-sequence");
            };
            for (i, half) in items.iter().enumerate() {
                assert_eq!(
                    class_of(half).as_deref(),
                    Some("carbon.common.script.sys.crowset.CRowset"),
                    "{method}[{i}] must be a CRowset: marketsvc.py:825 reads \
                     .columns off it and calls list.sort on it"
                );
            }
        }
    }

    /// The two halves share one descriptor. `SortAsks` indexes rows
    /// *positionally* off one `columns` list, so two halves with different
    /// column orders would sort by different fields.
    #[test]
    fn the_two_halves_share_one_descriptor() {
        let PyValue::Tuple(items) = call("GetOrders") else {
            panic!("GetOrders must answer a 2-sequence");
        };
        assert_eq!(
            items[0], items[1],
            "asks and bids must be built from the same descriptor"
        );
    }

    /// Every column the client names. `headers.index(name)` raises
    /// `ValueError` for a name that is not declared, and `row.jumps = ...`
    /// (marketsvc.py:821) raises for a `blue.DBRow` whose descriptor has no
    /// `jumps`, so this list is the contract rather than a convenience.
    #[test]
    fn order_rows_declare_every_column_the_client_names() {
        let names: Vec<String> = market_order_descriptor()
            .names()
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        for required in [
            // marketsvc.SortAsks / SortBids, by headers.index(..)
            "price",
            "issueDate",
            "jumps",
            "orderID",
            "range",
            // marketsvc.RefreshJumps, read and written
            "solarSystemID",
            // marketbase.Reload and the quote.GetXxx column formatters
            "typeID",
            "stationID",
            "volRemaining",
            "volEntered",
            "minVolume",
            "duration",
            "regionID",
            // orders.PopulateScroll: `if order.bid:`
            "bid",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "the order rowset must declare {required:?}; it has {names:?}"
            );
        }
    }

    /// `GetPlexOrders` is the same answer as `GetOrders`: `RefreshOrderCache`
    /// calls it *instead* for `typePlex` and subscripts it identically.
    #[test]
    fn plex_orders_have_the_same_shape_as_orders() {
        assert_eq!(call("GetOrders"), call("GetPlexOrders"));
    }

    /// The range forms and `GetPlexBest` are **dicts**: the client assigns
    /// into one (`self.asksForMyRange[typePlex] = ...`, marketbase.py:2719)
    /// and tests `typePlex in plexAsks` on the other, and no rowset family
    /// has `__setitem__`.
    #[test]
    fn the_ask_lookups_are_dicts() {
        for method in [
            "GetRegionBest",
            "GetStationAsks",
            "GetSystemAsks",
            "GetPlexBest",
        ] {
            assert!(
                matches!(call(method), PyValue::Dict(_)),
                "{method} must be a dict: marketbase.py:2719 assigns into it \
                 and marketbase.py:2718 tests `in` on it"
            );
        }
    }

    /// `GetHistoryRowList`'s first statement is
    /// `dbutil.RowList((new.header, []))`, so each half of the uncached
    /// price-history path is a `CRowset` whose header is a real
    /// `DBRowDescriptor` -- and all four halves must agree on it, because
    /// `RowList.append` compares `row.__columns__` with the list's.
    #[test]
    fn price_history_halves_are_crowsets_over_one_descriptor() {
        let expect = eve_rowset::CRowset::new(price_history_descriptor()).to_pyvalue();
        for method in [
            "GetOldPriceHistory",
            "GetNewPriceHistory",
            "GetPlexOldPriceHistory",
            "GetPlexNewPriceHistory",
        ] {
            let got = call(method);
            assert_eq!(
                class_of(&got).as_deref(),
                Some("carbon.common.script.sys.crowset.CRowset"),
                "{method} must be a CRowset; `new.header` is read before \
                 anything else in GetHistoryRowList"
            );
            assert_eq!(
                got, expect,
                "{method} must share the descriptor that empty_price_histories \
                 uses, or RowList.append rejects the rows"
            );
        }
    }

    /// The six price-history columns are read **positionally** by
    /// `pricehistorychart.RenderTable` (`rec[0]`..`rec[5]`) and by
    /// `marketsvc.GetAveragePrice` (`entry[3] * entry[4]`), so the order is
    /// the contract here, unlike the order book.
    #[test]
    fn price_history_columns_are_in_wire_order() {
        let names: Vec<String> = price_history_descriptor()
            .names()
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "historyDate",
                "lowPrice",
                "highPrice",
                "avgPrice",
                "volume",
                "orders"
            ]
        );
    }

    /// The PLEX mutators stay answered and **falsy**: `if wereOrdersCreated:
    /// self.Close()` (sellMulti.py:509) is the only read, and no PLEX order
    /// can exist here.
    #[test]
    fn the_plex_mutators_report_that_nothing_happened() {
        for method in ["PlacePlexSellOrder", "ModifyPlexCharOrder"] {
            assert_eq!(call(method), PyValue::None, "{method}");
        }
    }

    /// Own-order lists are iterated for `.orderID` and tested for truth, so
    /// none of them may be `None`. `GetCharOrders` and
    /// `GetMarketOrderHistory` are rowsets of rows now; the corp form stays a
    /// list.
    #[test]
    fn own_order_lists_are_iterable() {
        for method in ["GetCharOrders", "GetMarketOrderHistory"] {
            assert_eq!(
                class_of(&call(method)).as_deref(),
                Some("carbon.common.script.sys.crowset.CRowset"),
                "{method}"
            );
        }
        assert!(matches!(call("GetCorporationOrders"), PyValue::List(_)));
    }

    // ---- the order book --------------------------------------------------

    const ALICE: i64 = 90_000_001;
    const BOB: i64 = 90_000_002;
    const STATION: i64 = 60_003_760;
    const SYSTEM: i64 = 30_000_142;
    const REGION: i64 = 10_000_002;
    const TRITANIUM: i64 = 34;

    /// Two pilots docked in one station, each with a million ISK.
    fn market() -> Registry {
        let mut config = SandboxConfig::default();
        let pilot = |id: i64, name: &str, ship: i64| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            station_id: Some(STATION),
            solar_system_id: Some(SYSTEM),
            region_id: REGION,
            balance: 1_000_000.0,
            ..Character::default()
        };
        config.characters = vec![
            pilot(ALICE, "Alice", 100_000_001),
            pilot(BOB, "Bob", 100_000_002),
        ];
        Registry::new(config)
    }

    fn hangar(who: i64) -> crate::item::Slot {
        crate::item::Slot::new(who, STATION, crate::model::FLAG_HANGAR)
    }

    /// **E11 (O5-e)**: a bot buying off a human's ask stops at the day's
    /// demand budget, and the refusal is the ordinary no-match; the same ask
    /// from a bot is never limited.
    #[test]
    fn a_bot_fill_on_a_human_ask_stops_at_the_demand_budget() {
        use crate::econ_ledger::BOT_DEMAND_PER_HUMAN;
        let mut reg = market();
        let price = BOT_DEMAND_PER_HUMAN / 40.0;
        let bob = reg.character_mut(BOB).unwrap();
        bob.is_bot = true;
        bob.balance = BOT_DEMAND_PER_HUMAN * 4.0;
        let item = stack(&mut reg, ALICE, 100);
        sell(&mut reg, ALICE, item, price, 100, 1).unwrap();
        buy(&mut reg, BOB, price, 100, 0).unwrap();
        assert_eq!(units(&reg, BOB), 40, "the budget buys 40");
        assert!(
            buy(&mut reg, BOB, price, 1, 0).is_err(),
            "past the cap the fill is refused"
        );
        assert_eq!(units(&reg, BOB), 40);
        // A human buyer is never limited, and nor is a bot seller.
        reg.character_mut(ALICE).unwrap().is_bot = true;
        buy(&mut reg, BOB, price, 60, 0).unwrap();
        assert_eq!(units(&reg, BOB), 100, "bot-bot fills are never limited");
    }

    /// **A seeded ask** (`Registry::seed_ask`): a finite stack rests at the
    /// station corporation's name, a buyer takes it for ISK and gets the
    /// goods from escrow, the ISK leaves the economy (the corporation has no
    /// wallet), and the ask never refills.
    #[test]
    fn a_seeded_ask_is_finite_and_its_isk_leaves_the_economy() {
        const NPC_CORP: i64 = 1_000_090;
        let mut reg = market();
        reg.stations.insert(
            STATION,
            character::StationRef {
                station_id: STATION,
                owner_id: NPC_CORP,
                type_id: 0,
                operation_id: 0,
                solar_system_id: SYSTEM,
                constellation_id: 0,
                region_id: REGION,
            },
        );
        assert!(
            reg.seed_ask(ALICE, STATION, TRITANIUM, 0, 5.0, 10).is_err(),
            "no quantity"
        );
        assert!(
            reg.seed_ask(ALICE, STATION, TRITANIUM, 10, 0.0, 10)
                .is_err()
        );
        assert!(reg.seed_ask(ALICE, 12, TRITANIUM, 10, 5.0, 10).is_err());
        assert_eq!(reg.market.open().count(), 0, "a refusal made nothing");
        let (order, owner) = reg
            .seed_ask(ALICE, STATION, TRITANIUM, 10, 5.0, 10)
            .expect("seeded");
        assert_eq!(owner, NPC_CORP);
        let o = reg.market.get(order).expect("the order");
        assert_eq!(
            (o.char_id, o.vol_remaining, o.bid, o.station_id),
            (NPC_CORP, 10, false, STATION)
        );
        let before = balance(&reg, BOB);
        buy(&mut reg, BOB, 5.0, 4, 0).expect("buys four");
        assert_eq!(units(&reg, BOB), 4);
        assert_eq!(balance(&reg, BOB), before - 20.0);
        assert_eq!(reg.market.get(order).map(|o| o.vol_remaining), Some(6));
        buy(&mut reg, BOB, 5.0, 100, 0).expect("buys the rest");
        assert_eq!(units(&reg, BOB), 10, "ten and no more");
        assert!(buy(&mut reg, BOB, 5.0, 1, 0).is_err(), "never restocks");
    }

    /// Put a stack in someone's hangar and answer its id.
    fn stack(reg: &mut Registry, who: i64, quantity: i64) -> i64 {
        let changes = reg
            .apply_items(&[crate::item::Edit::Create {
                type_id: TRITANIUM,
                quantity,
                to: hangar(who),
            }])
            .expect("tritanium is a known type");
        let _ = reg.take_pending();
        changes
            .iter()
            .find_map(|c| match c {
                crate::item::Change::Created { item_id, .. } => Some(*item_id),
                _ => None,
            })
            .expect("a created stack")
    }

    /// Units of tritanium someone holds in their hangar.
    fn units(reg: &Registry, who: i64) -> i64 {
        reg.store()
            .at(hangar(who))
            .filter(|i| i.type_id == TRITANIUM)
            .map(|i| i.quantity)
            .sum()
    }

    fn balance(reg: &Registry, who: i64) -> f64 {
        reg.character(who).map(|c| c.balance).unwrap_or_default()
    }

    fn as_(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        dispatch(
            reg,
            &Request {
                service: "marketProxy",
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: Some(who),
            },
        )
    }

    /// `sellMulti.GetValidatedItem`'s KeyVal (sellMulti.py:541-549).
    fn sell_line(item_id: i64, price: f64, quantity: i64) -> PyValue {
        keyval(vec![
            ("stationID", PyValue::Int(STATION)),
            ("typeID", PyValue::Int(TRITANIUM)),
            ("itemID", PyValue::Int(item_id)),
            ("price", PyValue::Real(price)),
            ("quantity", PyValue::Int(quantity)),
            ("officeID", PyValue::None),
            ("delta", PyValue::Real(0.0)),
            ("rawBrokerFeePercentage", PyValue::Real(0.0)),
        ])
    }

    /// `PlaceMultiSellOrder(itemList, useCorp, duration, expectedBrokersFee)`.
    fn sell(
        reg: &mut Registry,
        who: i64,
        item: i64,
        price: f64,
        qty: i64,
        days: i64,
    ) -> Result<PyValue, ServiceError> {
        as_(
            reg,
            who,
            "PlaceMultiSellOrder",
            &[
                PyValue::List(vec![sell_line(item, price, qty)]),
                PyValue::Bool(false),
                PyValue::Int(days),
                PyValue::Real(0.0),
            ],
        )
    }

    /// `PlaceBuyOrder`'s nine arguments, marketsvc.py:313.
    fn buy(
        reg: &mut Registry,
        who: i64,
        price: f64,
        qty: i64,
        days: i64,
    ) -> Result<PyValue, ServiceError> {
        as_(
            reg,
            who,
            "PlaceBuyOrder",
            &[
                PyValue::Int(STATION),
                PyValue::Int(TRITANIUM),
                PyValue::Real(price),
                PyValue::Int(qty),
                PyValue::Int(RANGE_STATION),
                PyValue::Int(1),
                PyValue::Int(days),
                PyValue::Bool(false),
                PyValue::Real(0.0),
            ],
        )
    }

    /// The rows of a `CRowset` ObjectEx: `(callable, (descriptor, [rows]))`.
    fn rows(v: &PyValue) -> Vec<DbRow> {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("not a rowset: {v:?}");
        };
        let PyValue::Tuple(h) = header.as_ref() else {
            panic!("no header");
        };
        let PyValue::Tuple(args) = &h[1] else {
            panic!("no args");
        };
        let PyValue::List(rows) = &args[1] else {
            panic!("no rows");
        };
        rows.iter()
            .map(|r| match r {
                PyValue::DbRow(r) => (**r).clone(),
                other => panic!("not a row: {other:?}"),
            })
            .collect()
    }

    fn col(row: &DbRow, name: &str) -> DbValue {
        let i = row
            .descriptor()
            .index_of(name.as_bytes())
            .unwrap_or_else(|| panic!("no column {name}"));
        row.values()[i].clone()
    }

    /// `(asks, bids)` as the client sees them after `GetResult`.
    fn book(reg: &mut Registry, who: i64) -> (Vec<DbRow>, Vec<DbRow>) {
        let v = as_(reg, who, "GetOrders", &[PyValue::Int(TRITANIUM)]).expect("GetOrders");
        let PyValue::Tuple(halves) = unwrap_cached(v) else {
            panic!("GetOrders is a 2-sequence");
        };
        (rows(&halves[0]), rows(&halves[1]))
    }

    /// Every `OnOwnOrdersChanged` queued, as `(audience, orders, reason)`.
    fn order_pushes(reg: &mut Registry) -> Vec<(Option<Vec<i64>>, PyValue, PyValue)> {
        reg.take_pending()
            .into_iter()
            .filter_map(|(_, p)| {
                let (name, args) = p.to_push();
                (name == "OnOwnOrdersChanged")
                    .then(|| (p.audience(), args[0].clone(), args[1].clone()))
            })
            .collect()
    }

    /// **Without the cache wrapper the market window can never update.**
    /// `RefreshOrderCache` stores a new book only when
    /// `GetCachedMethodCallVersion` moved (marketsvc.py:857-861), and that is
    /// `0` for an answer nobody marked cachable (objectCaching.py:527). So
    /// `GetOrders` is a `CachedMethodCallResult` whose state is the
    /// `__getstate__` triple (objectCaching.py:860), and its version must
    /// change when -- and only when -- the book does.
    #[test]
    fn get_orders_is_cached_with_a_version_that_follows_the_book() {
        let mut reg = market();
        let version = |reg: &mut Registry| {
            let v = as_(reg, ALICE, "GetOrders", &[PyValue::Int(TRITANIUM)]).unwrap();
            let PyValue::Instance { class, state } = v else {
                panic!("GetOrders must be a CachedMethodCallResult");
            };
            assert_eq!(class, CACHED_METHOD_CALL_RESULT.as_bytes());
            let PyValue::Tuple(parts) = *state else {
                panic!("state is (details, result, version)");
            };
            assert_eq!(parts.len(), 3);
            assert!(matches!(parts[0], PyValue::Dict(_)), "details is a dict");
            assert!(
                matches!(parts[1], PyValue::Bytes(_)),
                "result is marshal bytes"
            );
            let PyValue::List(v) = &parts[2] else {
                panic!("version is a list: UpdateVersionCheckPeriod assigns into it");
            };
            v[1].clone()
        };
        let empty = version(&mut reg);
        assert_eq!(
            version(&mut reg),
            empty,
            "an unchanged book keeps its version"
        );
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, 5.0, 10, 1).unwrap();
        assert_ne!(
            version(&mut reg),
            empty,
            "a new order must move the version"
        );
    }

    /// zlib's adler32, which the client's own constructor uses
    /// (objectCaching.py:857). The reference value is zlib's for "Wikipedia".
    #[test]
    fn adler32_is_zlibs() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    /// A sell order rests on the **asks** side, index 0
    /// (`asks, bids = orders[0], orders[1]`, marketsvc.py:863), and its goods
    /// leave the seller's hangar.
    #[test]
    fn a_sell_order_is_an_ask_and_takes_the_goods() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 100);
        let created = sell(&mut reg, ALICE, item, 5.0, 100, 3).unwrap();
        assert!(
            !matches!(created, PyValue::None | PyValue::Bool(false)),
            "truthy, so sellMulti.py:509 closes the window"
        );
        let (asks, bids) = book(&mut reg, BOB);
        assert_eq!((asks.len(), bids.len()), (1, 0));
        assert_eq!(col(&asks[0], "bid"), DbValue::Bool(false));
        assert_eq!(units(&reg, ALICE), 0);
    }

    /// Put a pilot in space the way a real undock does: the ballpark bind
    /// seeds the roster.
    fn launch(reg: &mut Registry, who: i64) {
        crate::destiny::on_ballpark_bind(reg, Some(who));
        let _ = reg.take_pending();
        assert_eq!(reg.docked_station(who), None, "in space");
    }

    /// **Nothing is sold from the home hangar while in space** -- not even
    /// at a station whose book already has an order in it, which used to be
    /// enough. `Character::station_id` stays set on undock
    /// (`docs/lanes/docked.md`). The docked half is
    /// `a_sell_order_is_an_ask_and_takes_the_goods`.
    #[test]
    fn in_space_nothing_is_sold_from_the_home_hangar() {
        let mut reg = market();
        buy(&mut reg, BOB, 1.0, 10, 1).unwrap();
        let item = stack(&mut reg, ALICE, 100);
        launch(&mut reg, ALICE);
        let before = balance(&reg, ALICE);
        assert!(matches!(
            sell(&mut reg, ALICE, item, 5.0, 100, 3),
            Err(ServiceError::Raise(_))
        ));
        assert!(matches!(
            sell(&mut reg, ALICE, item, 1.0, 10, 0),
            Err(ServiceError::Raise(_))
        ));
        assert_eq!(units(&reg, ALICE), 100);
        assert_eq!(balance(&reg, ALICE), before);
        let (asks, bids) = book(&mut reg, BOB);
        assert_eq!((asks.len(), bids.len()), (0, 1));
    }

    /// **A resting buy order at a station with an empty book needs the
    /// buyer docked there.** The docked half is
    /// `a_buy_order_rests_shows_as_mine_and_cancelling_refunds_it`.
    #[test]
    fn in_space_no_buy_order_is_placed_at_an_empty_home_market() {
        let mut reg = market();
        launch(&mut reg, BOB);
        assert!(matches!(
            buy(&mut reg, BOB, 4.0, 10, 1),
            Err(ServiceError::Raise(_))
        ));
        assert_eq!(balance(&reg, BOB), 1_000_000.0);
        let (asks, bids) = book(&mut reg, ALICE);
        assert_eq!((asks.len(), bids.len()), (0, 0));
    }

    /// `OnOwnOrdersChanged`'s first argument is iterated **unguarded** by
    /// `_IsAtLeastOneNonImmediate` (marketsvc.py:143, :148), so it is never
    /// `None`, and each element answers `.typeID`, `.duration`, `.orderID`
    /// and `.volRemaining` (marketsvc.py:133, :148; buyMultiFromBase.py:588,
    /// :598).
    #[test]
    fn own_order_pushes_carry_rows_the_handlers_can_read() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, 5.0, 10, 1).unwrap();
        let pushes = order_pushes(&mut reg);
        assert!(!pushes.is_empty(), "placing an order must tell the owner");
        for (to, orders, _) in pushes {
            assert_eq!(to, Some(vec![ALICE]));
            let PyValue::List(orders) = orders else {
                panic!("orders must be a list, never None");
            };
            for o in orders {
                let PyValue::DbRow(row) = o else {
                    panic!("an order element must answer attribute reads");
                };
                for name in ["typeID", "duration", "orderID", "volRemaining"] {
                    assert!(
                        row.descriptor().index_of(name.as_bytes()).is_some(),
                        "{name}"
                    );
                }
            }
        }
    }

    /// Buying from an ask moves goods to the buyer and ISK from buyer to
    /// seller, tells the **seller** about their order (an addressed push),
    /// and loses no goods: every unit is either in a hangar or still on the
    /// ask. The amounts are not pinned -- fill price and fees are `[A]`.
    #[test]
    fn buying_from_an_ask_moves_goods_and_isk_between_the_two() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 100);
        sell(&mut reg, ALICE, item, 5.0, 100, 3).unwrap();
        let _ = reg.take_pending();
        let seller_before = balance(&reg, ALICE);
        buy(&mut reg, BOB, 5.0, 40, 0).unwrap();
        assert_eq!(units(&reg, BOB), 40);
        let (asks, _) = book(&mut reg, BOB);
        assert_eq!(col(&asks[0], "volRemaining"), DbValue::Real(60.0));
        assert!(balance(&reg, BOB) < 1_000_000.0);
        assert!(balance(&reg, ALICE) > seller_before);
        let to_alice = order_pushes(&mut reg)
            .into_iter()
            .any(|(to, _, _)| to == Some(vec![ALICE]));
        assert!(
            to_alice,
            "the seller's order changed; the seller must hear it"
        );
    }

    /// A buy order rests on the **bids** side (index 1) and is the buyer's
    /// own order in `GetCharOrders`, whose `charID` gates Cancel and Modify
    /// (orders.py:342). Cancelling it returns exactly the ISK that
    /// `GetCharEscrow` said it held.
    #[test]
    fn a_buy_order_rests_shows_as_mine_and_cancelling_refunds_it() {
        let mut reg = market();
        buy(&mut reg, BOB, 4.0, 10, 1).unwrap();
        assert!(
            balance(&reg, BOB) < 1_000_000.0,
            "the escrow left the wallet"
        );
        let placed = balance(&reg, BOB);
        let escrow = as_(&mut reg, BOB, "GetCharEscrow", &[]).unwrap();
        let PyValue::Real(held) = attr(&escrow, "iskEscrow").cloned().unwrap() else {
            panic!("iskEscrow is read through float() (marketsvc.py:453)");
        };
        assert!(held > 0.0);
        let (asks, bids) = book(&mut reg, ALICE);
        assert_eq!((asks.len(), bids.len()), (0, 1));
        let mine = rows(&as_(&mut reg, BOB, "GetCharOrders", &[]).unwrap());
        assert_eq!(mine.len(), 1);
        assert_eq!(col(&mine[0], "charID"), DbValue::Int(BOB));
        let DbValue::Int(order_id) = col(&mine[0], "orderID") else {
            panic!("orderID is an integer");
        };
        assert!(rows(&as_(&mut reg, ALICE, "GetCharOrders", &[]).unwrap()).is_empty());
        // Not Alice's to cancel.
        assert!(
            as_(
                &mut reg,
                ALICE,
                "CancelCharOrder",
                &[PyValue::Int(order_id), PyValue::Int(REGION)]
            )
            .is_err()
        );
        as_(
            &mut reg,
            BOB,
            "CancelCharOrder",
            &[PyValue::Int(order_id), PyValue::Int(REGION)],
        )
        .unwrap();
        assert_eq!(balance(&reg, BOB), placed + held);
        let (_, bids) = book(&mut reg, ALICE);
        assert!(bids.is_empty());
        let history = rows(&as_(&mut reg, BOB, "GetMarketOrderHistory", &[]).unwrap());
        assert_eq!(
            col(&history[0], "orderState"),
            DbValue::Int(ORDER_STATE_CANCELLED),
            "marketHistory.py:270 prints Cancelled for state 3"
        );
    }

    /// Cancelling a sell order brings the goods home.
    #[test]
    fn cancelling_a_sell_order_returns_the_goods() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 25);
        sell(&mut reg, ALICE, item, 5.0, 25, 3).unwrap();
        let DbValue::Int(order_id) = col(&book(&mut reg, ALICE).0[0], "orderID") else {
            panic!("orderID");
        };
        as_(
            &mut reg,
            ALICE,
            "CancelCharOrder",
            &[PyValue::Int(order_id), PyValue::Int(REGION)],
        )
        .unwrap();
        assert_eq!(units(&reg, ALICE), 25);
        assert!(book(&mut reg, ALICE).0.is_empty());
    }

    /// Selling into a bid delivers to the bidder and pays the seller, and the
    /// bid shrinks by what it bought.
    #[test]
    fn selling_into_a_bid_pays_the_seller_and_delivers_to_the_bidder() {
        let mut reg = market();
        buy(&mut reg, BOB, 6.0, 30, 1).unwrap();
        let item = stack(&mut reg, ALICE, 10);
        let before = balance(&reg, ALICE);
        sell(&mut reg, ALICE, item, 6.0, 10, 0).unwrap();
        assert_eq!(units(&reg, BOB), 10);
        assert_eq!(units(&reg, ALICE), 0);
        assert!(balance(&reg, ALICE) > before);
        let (_, bids) = book(&mut reg, ALICE);
        assert_eq!(col(&bids[0], "volRemaining"), DbValue::Real(20.0));
    }

    /// Modifying a price is visible on the next read. The nine arguments are
    /// the client's own view of the order (marketsvc.py:333).
    #[test]
    fn modifying_an_order_changes_its_price() {
        let mut reg = market();
        buy(&mut reg, BOB, 4.0, 10, 1).unwrap();
        let mine = rows(&as_(&mut reg, BOB, "GetCharOrders", &[]).unwrap());
        let DbValue::Int(order_id) = col(&mine[0], "orderID") else {
            panic!("orderID");
        };
        age(&mut reg, order_id);
        modify(&mut reg, BOB, order_id, 4.0, 4.5).unwrap();
        let mine = rows(&as_(&mut reg, BOB, "GetCharOrders", &[]).unwrap());
        assert_eq!(
            col(&mine[0], "price"),
            DbValue::Int(45_000),
            "CY is ISK x 10,000"
        );
    }

    /// **Multibuy only re-enables when every order it was handed comes back
    /// in an `'Expiry'` push**: `_PerformTrade` adds the returned `orderID`s
    /// to `activeOrders` (buyMultiFromBase.py:331), and only
    /// `OnOwnOrdersChanged` with `reason == 'Expiry'` (:584) removes them
    /// (:601). The return is iterated for `.typeID` (marketsvc.py:318).
    #[test]
    fn every_multibuy_order_comes_back_expired() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 5);
        sell(&mut reg, ALICE, item, 2.0, 5, 3).unwrap();
        let _ = reg.take_pending();
        let line = |q: i64| {
            keyval(vec![
                ("stationID", PyValue::Int(STATION)),
                ("typeID", PyValue::Int(TRITANIUM)),
                ("price", PyValue::Real(2.0)),
                ("quantity", PyValue::Int(q)),
                ("minVolume", PyValue::Int(1)),
                ("delta", PyValue::Real(0.0)),
            ])
        };
        let created = as_(
            &mut reg,
            BOB,
            "BuyMultipleItems",
            &[
                PyValue::Int(STATION),
                PyValue::List(vec![line(8)]),
                PyValue::Bool(false),
            ],
        )
        .unwrap();
        let PyValue::List(created) = created else {
            panic!("BuyMultipleItems is iterated");
        };
        assert_eq!(created.len(), 1);
        let PyValue::DbRow(row) = &created[0] else {
            panic!("orders answer .typeID and .orderID");
        };
        let id = col(row, "orderID");
        assert_eq!(col(row, "typeID"), DbValue::Int(TRITANIUM));
        let expired: Vec<DbRow> = order_pushes(&mut reg)
            .into_iter()
            .filter(|(to, _, reason)| {
                *to == Some(vec![BOB]) && *reason == crate::pyutil::bstr("Expiry")
            })
            .flat_map(|(_, orders, _)| match orders {
                PyValue::List(o) => o,
                _ => vec![],
            })
            .map(|o| match o {
                PyValue::DbRow(r) => *r,
                other => panic!("{other:?}"),
            })
            .collect();
        let back = expired
            .iter()
            .find(|r| col(r, "orderID") == id)
            .expect("the order expired");
        // Five filled; the three that did not are what the window re-adds
        // as a failed line (buyMultiFromBase.py:598-599).
        assert_eq!(col(back, "volRemaining"), DbValue::Real(3.0));
        assert_eq!(units(&reg, BOB), 5);
    }

    /// `GetStationAsks`/`GetSystemAsks` values are **4-sequences**, because
    /// `ConvertTuplesToBestByOrders` splats them into `BestByOrder`
    /// (marketutil/__init__.py:6), keyed by `typeID` (marketbase.py:2684).
    #[test]
    fn station_and_system_asks_are_four_tuples_keyed_by_type() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 3);
        sell(&mut reg, ALICE, item, 9.0, 3, 1).unwrap();
        for method in ["GetStationAsks", "GetSystemAsks"] {
            let PyValue::Dict(entries) = as_(&mut reg, BOB, method, &[]).unwrap() else {
                panic!("{method} is a dict");
            };
            assert_eq!(entries.len(), 1);
            let (value, key) = &entries[0];
            assert_eq!(*key, PyValue::Int(TRITANIUM));
            assert!(
                matches!(value, PyValue::Tuple(t) if t.len() == 4),
                "{method}"
            );
        }
    }

    /// The book survives the world file: render, parse every line back, and
    /// get the same book -- allocators included, so no id is handed out
    /// twice after a restart.
    #[test]
    fn the_book_survives_the_world_file() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 50);
        sell(&mut reg, ALICE, item, 5.0, 50, 3).unwrap();
        buy(&mut reg, BOB, 5.0, 20, 0).unwrap();
        buy(&mut reg, BOB, 1.0, 7, 2).unwrap();
        let mut text = String::new();
        reg.market.render(&mut text);
        let mut back = Book::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            assert!(Book::RECORDS.contains(&f[0]), "{line}");
            back.parse_record(&f).unwrap();
        }
        assert_eq!(back, reg.market);
        let mut empty = String::new();
        Book::default().render(&mut empty);
        assert!(empty.is_empty(), "an untraded world writes no market lines");
    }

    /// **`StartupCheck` must never raise.** `MarketUtils.StartupCheck`
    /// caches a raised `UserError` in `reg.regionUpAt` and re-raises it for
    /// two minutes without touching the wire, which also disables Sell Item
    /// from the inventory right-click menu. This is the one arm in the file
    /// where a raise is a two-minute subsystem outage rather than a
    /// traceback, so the test exists to stop someone "improving" it.
    #[test]
    fn startup_check_never_raises() {
        assert_eq!(call("StartupCheck"), PyValue::None);
    }

    // ---- NPC orders (CP35) -----------------------------------------------

    const NPC_CORP: i64 = 1_999_001;
    use crate::pyutil::dict_get;

    /// The market world with one NPC station at `STATION`. With `ask`, the
    /// exchange reads tritanium's row as a blueprint its owner sells (O5-a,
    /// a Tier A ask); without, as a loot-only module at a hub (O5-d, a Tier B
    /// floor bid). The rows and the price are invented; the item store still
    /// mints tritanium.
    fn npc_market(ask: bool) -> Registry {
        let (group_id, base_price) = (18, 2.0);
        let (category_id, meta_group_id, tech_level) = if ask {
            (exchange::CATEGORY_BLUEPRINT, None, None)
        } else {
            (
                exchange::LOOT_CATEGORIES[0],
                Some(exchange::LOOT_META_GROUPS[2]),
                Some(1),
            )
        };
        let row = exchange::TypeRow {
            group_id,
            category_id,
            base_price,
            meta_group_id,
            tech_level,
        };
        let types = std::collections::BTreeMap::from([(TRITANIUM, row)]);
        let (station_id, system_id, region_id) = (STATION, SYSTEM, REGION);
        let owner_id = NPC_CORP;
        let station = exchange::Station {
            station_id,
            system_id,
            region_id,
            owner_id,
        };
        let trades: Vec<_> = ask
            .then_some((NPC_CORP, TRITANIUM, 1.0))
            .into_iter()
            .collect();
        let hubs: &[i64] = if ask { &[] } else { &[STATION] };
        let table = exchange::Table::new(vec![station], &trades, types, hubs).expect("builds");
        let line = format!("{TRITANIUM}\t5.0\tbase\t4.0\t4\t18\t\t1");
        let prices = crate::econ_prices::Table::parse(&[line]).expect("parses");
        let mut c = crate::content::Content::fixture();
        c.insert("econ_exchange", table);
        c.insert("econ_prices", prices);
        market().with_content(std::sync::Arc::new(c))
    }

    /// The one NPC order on tritanium, and what its `npcorder` row says is
    /// left of it (`None` with no row).
    fn npc_tritanium(reg: &Registry) -> (exchange::NpcOrder, Option<i64>) {
        let now = crate::pyutil::filetime_now();
        let v = exchange::orders(reg, REGION, Some(TRITANIUM), now);
        assert_eq!(v.len(), 1, "{v:?}");
        let o = v[0].clone();
        let k = [
            o.station_id.to_string(),
            o.type_id.to_string(),
            o.side.as_str().into(),
        ];
        let row = reg
            .deltas()
            .of(exchange::KIND)
            .get(&k[..])
            .map(|r| r[0].parse().unwrap());
        (o, row)
    }

    fn tritanium_anywhere(reg: &Registry) -> i64 {
        let items = reg.store().items().iter();
        items
            .filter(|i| i.type_id == TRITANIUM)
            .map(|i| i.quantity)
            .sum()
    }

    /// **CP35**: buying from an NPC ask mints the goods into the buyer's
    /// hangar and debits the ISK, with no escrow stack anywhere; the order
    /// is never a book row, and its stock falls by what was bought.
    #[test]
    fn buying_from_an_npc_ask_mints_the_goods_with_no_escrow() {
        let mut reg = npc_market(true);
        let (ask, None) = npc_tritanium(&reg) else {
            panic!("a full order has no row")
        };
        assert_eq!(ask.side, exchange::Side::Ask);
        let before = balance(&reg, ALICE);
        buy(&mut reg, ALICE, ask.price, 100, 0).expect("fills from the NPC");
        assert_eq!(
            (units(&reg, ALICE), tritanium_anywhere(&reg)),
            (100, 100),
            "minted"
        );
        assert_eq!(balance(&reg, ALICE), cents(before - 100.0 * ask.price));
        assert_eq!(npc_tritanium(&reg).1, Some(ask.vol - 100));
        assert!(reg.market.all().all(|o| !exchange::is_npc(o.order_id)));
        let t = reg.market.trades.last().expect("a trade");
        assert_eq!(
            (t.seller_id, t.buyer_id, t.quantity),
            (NPC_CORP, ALICE, 100)
        );
        let (asks, _) = book(&mut reg, ALICE);
        assert!(
            asks.iter()
                .any(|r| col(r, "orderID") == DbValue::Int(ask.order_id))
        );
    }

    /// `OnItemChange` and `OnMarketItemReceived` pushes addressed to `who`, in
    /// order, as `(name, args)`.
    fn item_pushes(reg: &mut Registry, who: i64) -> Vec<(&'static str, Vec<PyValue>)> {
        reg.take_pending()
            .into_iter()
            .filter(|(_, p)| p.audience().is_none_or(|a| a.contains(&who)))
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| *n == "OnItemChange" || *n == "OnMarketItemReceived")
            .collect()
    }

    fn items_tuple(station: i64) -> PyValue {
        PyValue::Tuple(vec![
            PyValue::Bytes(b"Station".to_vec()),
            PyValue::Int(station),
            PyValue::Bytes(b"StationItems".to_vec()),
        ])
    }

    /// The new-items dot: a buy in the station the buyer is docked in tells
    /// the client's inventory badging which storage it landed in, and sends
    /// `OnMarketItemReceived` for the delivered stack.
    #[test]
    fn a_local_npc_buy_names_the_badging_storage_and_the_receipt() {
        let mut reg = npc_market(true);
        let (ask, _) = npc_tritanium(&reg);
        let _ = reg.take_pending();
        buy(&mut reg, ALICE, ask.price, 10, 0).expect("fills");
        let pushes = item_pushes(&mut reg, ALICE);
        let change = pushes
            .iter()
            .find(|(n, _)| *n == "OnItemChange")
            .expect("an OnItemChange");
        assert_eq!(
            change.1[1],
            PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(3))])
        );
        assert_eq!(change.1[2], items_tuple(STATION));
        let got = pushes
            .iter()
            .find(|(n, _)| *n == "OnMarketItemReceived")
            .expect("an OnMarketItemReceived");
        let item = reg
            .store()
            .at(hangar(ALICE))
            .find(|i| i.type_id == TRITANIUM)
            .expect("arrived")
            .item_id;
        assert_eq!(
            got.1,
            vec![
                PyValue::Int(item),
                PyValue::Int(TRITANIUM),
                PyValue::Int(4),
                PyValue::Int(STATION)
            ]
        );
    }

    /// A buy delivered to a station the buyer is not docked in carries no
    /// storage (it would badge the wrong station), and the receipt still
    /// names the delivery station.
    #[test]
    fn a_remote_buy_has_no_storage_but_still_a_receipt() {
        let mut reg = npc_market(true);
        let (ask, _) = npc_tritanium(&reg);
        reg.character_mut(ALICE).unwrap().station_id = Some(STATION + 1);
        let _ = reg.take_pending();
        buy(&mut reg, ALICE, ask.price, 10, 0).expect("fills remotely");
        let pushes = item_pushes(&mut reg, ALICE);
        let change = pushes
            .iter()
            .find(|(n, _)| *n == "OnItemChange")
            .expect("an OnItemChange");
        assert_eq!(change.1[2], PyValue::None);
        let got = pushes
            .iter()
            .find(|(n, _)| *n == "OnMarketItemReceived")
            .expect("an OnMarketItemReceived");
        assert_eq!(got.1[3], PyValue::Int(STATION));
    }

    /// A player's ask: the goods come out of escrow (owner 1), and the
    /// buyer's push still names the storage.
    #[test]
    fn a_player_ask_buy_names_the_badging_storage() {
        let mut reg = market();
        let item = stack(&mut reg, BOB, 50);
        sell(&mut reg, BOB, item, 2.0, 50, 1).expect("listed");
        let _ = reg.take_pending();
        buy(&mut reg, ALICE, 2.0, 50, 0).expect("fills the whole stack");
        let pushes = item_pushes(&mut reg, ALICE);
        let change = pushes
            .iter()
            .find(|(n, a)| {
                *n == "OnItemChange"
                    && matches!(&a[1], PyValue::Dict(d) if d.contains(&(PyValue::Int(1), PyValue::Int(2))))
            })
            .expect("an OnItemChange from escrow (ixOwnerID: 1)");
        assert_eq!(change.1[2], items_tuple(STATION));
        assert!(pushes.iter().any(|(n, _)| *n == "OnMarketItemReceived"));
    }

    /// **CP35**: selling to an NPC bid destroys the goods (a part stack is
    /// consumed, a whole one destroyed) and pays the seller.
    #[test]
    fn selling_to_an_npc_bid_destroys_the_goods() {
        let mut reg = npc_market(false);
        let (bid, _) = npc_tritanium(&reg);
        assert_eq!(bid.side, exchange::Side::Bid);
        let item = stack(&mut reg, ALICE, 50);
        let before = balance(&reg, ALICE);
        sell(&mut reg, ALICE, item, bid.price, 20, 0).expect("part sold to the NPC");
        assert_eq!(units(&reg, ALICE), 30);
        sell(&mut reg, ALICE, item, bid.price, 30, 0).expect("the rest sold");
        assert!(reg.store().get(item).is_none(), "destroyed");
        assert_eq!(tritanium_anywhere(&reg), 0, "nobody received them");
        let (v20, v30) = (cents(20.0 * bid.price), cents(30.0 * bid.price));
        let tax = sales_tax(v20, 0) + sales_tax(v30, 0);
        assert_eq!(balance(&reg, ALICE), cents(before + v20 + v30 - tax));
        assert_eq!(npc_tritanium(&reg).1, Some(bid.vol - 50));
        let (_, bids) = book(&mut reg, ALICE);
        assert!(
            bids.iter()
                .any(|r| col(r, "orderID") == DbValue::Int(bid.order_id))
        );
    }

    // ---- the GetOrders cache (FX8) ----------------------------------------

    /// `GetOrders` straight through [`get_orders`] at a chosen `now`, as the
    /// client sees it, with the cache's build count after the call.
    fn cached_book(reg: &mut Registry, now: i64) -> (PyValue, u64) {
        let v = unwrap_cached(get_orders(reg, TRITANIUM, REGION, now));
        (v, reg.market.cache.builds)
    }

    /// **FX8** (port-tracker §78): an unchanged book is answered from the
    /// cache, equal to the first answer, with no second build; through
    /// `dispatch` too.
    #[test]
    fn get_orders_reuses_an_unchanged_book() {
        let mut reg = npc_market(true);
        let now = crate::pyutil::filetime_now();
        let (first, n) = cached_book(&mut reg, now);
        assert_eq!(n, 1);
        assert_eq!(cached_book(&mut reg, now), (first.clone(), 1));
        let (asks, _) = book(&mut reg, ALICE);
        assert!(!asks.is_empty(), "the NPC ask is on the book");
        let built = reg.market.cache.builds;
        book(&mut reg, ALICE);
        assert_eq!(reg.market.cache.builds, built, "the second call reused it");
        assert_eq!(reg.market.clone(), reg.market, "the cache is not state");
    }

    /// **FX8**: a player order placed, then filled, between two calls
    /// rebuilds the book, and the answer shows the change.
    #[test]
    fn a_player_order_or_fill_invalidates_the_cached_book() {
        let mut reg = market();
        let now = crate::pyutil::filetime_now();
        let (empty, n0) = cached_book(&mut reg, now);
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, 7.0, 10, 3).expect("rests");
        let (placed, n1) = cached_book(&mut reg, now);
        assert_eq!(n1, n0 + 1);
        assert_ne!(placed, empty);
        buy(&mut reg, BOB, 7.0, 4, 0).expect("fills part of the ask");
        let (filled, n2) = cached_book(&mut reg, now);
        assert_eq!(n2, n1 + 1);
        assert_ne!(filled, placed);
        assert_eq!(cached_book(&mut reg, now), (filled, n2));
    }

    /// **FX8**: an NPC fill (an `npcorder` row), the econ rollover and a new
    /// trickle period each rebuild the book.
    #[test]
    fn npc_state_and_time_invalidate_the_cached_book() {
        let mut reg = npc_market(true);
        let now = crate::pyutil::filetime_now();
        let (ask, _) = npc_tritanium(&reg);
        let (full, n0) = cached_book(&mut reg, now);
        buy(&mut reg, ALICE, ask.price, 100, 0).expect("fills from the NPC");
        let (taken, n1) = cached_book(&mut reg, now);
        assert_eq!(n1, n0 + 1);
        assert_ne!(taken, full, "the ask's volRemaining fell");
        let mut w = *reg.world();
        w.last_rollover += 1;
        reg.deltas_mut().set_world(w);
        assert_eq!(cached_book(&mut reg, now).1, n1 + 1, "rollover");
        let later = now + exchange::TRICKLE_PERIOD;
        assert_eq!(cached_book(&mut reg, later).1, n1 + 2, "trickle period");
        assert_eq!(cached_book(&mut reg, later).1, n1 + 2);
    }

    /// **CP35**: `GetRegionBest` includes NPC asks, and a cheaper player ask
    /// still wins its type.
    #[test]
    fn get_region_best_includes_npc_asks() {
        let mut reg = npc_market(true);
        let (ask, _) = npc_tritanium(&reg);
        let best = |reg: &mut Registry, who| {
            let v = as_(reg, who, "GetRegionBest", &[]).expect("answers");
            let PyValue::Dict(e) = v else {
                panic!("a dict")
            };
            let (PyValue::Instance { state, .. }, _) = &e[0] else {
                panic!("a KeyVal")
            };
            dict_get(state, b"price").cloned()
        };
        assert_eq!(best(&mut reg, ALICE), Some(PyValue::Real(ask.price)));
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, ask.price / 2.0, 10, 3).expect("rests");
        assert_eq!(best(&mut reg, BOB), Some(PyValue::Real(ask.price / 2.0)));
    }

    // ---- price history and pruning (CP42) ----------------------------------

    /// An invented second type, so a fold that mixes types shows.
    const OTHER_TYPE: i64 = 35;

    /// Record one trade `days` whole days before today's midnight, `secs`
    /// seconds into that day.
    fn trade_on(reg: &mut Registry, days: i64, secs: i64, type_id: i64, price: f64, qty: i64) {
        let today = day_start(crate::pyutil::filetime_now());
        reg.market.record_trade(Trade {
            transaction_id: 0,
            date: today - days * DAY_TICKS + secs * 10_000_000,
            type_id,
            quantity: qty,
            price,
            station_id: STATION,
            buyer_id: BOB,
            seller_id: ALICE,
            region_id: REGION,
            ..Default::default()
        });
    }

    fn rollover(reg: &mut Registry) -> usize {
        let now = crate::pyutil::filetime_now();
        reg.market_history_rollover(&crate::rollover::Day {
            day: 1,
            days: 1,
            now,
        })
    }

    /// `(date, low, high, avg, volume, orders)` of every row of one half.
    fn history_rows(reg: &mut Registry, method: &str, type_id: i64) -> Vec<Vec<DbValue>> {
        let v = as_(reg, ALICE, method, &[PyValue::Int(type_id)]).unwrap();
        rows(&v)
            .iter()
            .map(|r| {
                [
                    "historyDate",
                    "lowPrice",
                    "highPrice",
                    "avgPrice",
                    "volume",
                    "orders",
                ]
                .iter()
                .map(|c| col(r, c))
                .collect()
            })
            .collect()
    }

    /// The fold of a list of `(price, qty)`, computed here from C5 §8's
    /// definitions and not by `aggregate`.
    fn expect_row(day: i64, fills: &[(f64, i64)]) -> Vec<DbValue> {
        let cy = |x: f64| DbValue::Int((x * 10_000.0).round() as i64);
        let low = fills.iter().map(|f| f.0).fold(f64::MAX, f64::min);
        let high = fills.iter().map(|f| f.0).fold(f64::MIN, f64::max);
        let vol: i64 = fills.iter().map(|f| f.1).sum();
        let isk: f64 = fills.iter().map(|f| f.0 * f.1 as f64).sum();
        vec![
            DbValue::Int(day),
            cy(low),
            cy(high),
            cy(isk / vol as f64),
            DbValue::Int(vol),
            DbValue::Int(fills.len() as i64),
        ]
    }

    /// REVIEW-FIX: a pilot's fill to himself is no history: the day reads
    /// from the real fills only, before and after the rollover.
    #[test]
    fn a_self_trade_is_left_out_of_the_history() {
        let mut reg = market();
        let today = day_start(crate::pyutil::filetime_now());
        trade_on(&mut reg, 1, 60, TRITANIUM, 5.0, 10);
        reg.market.record_trade(Trade {
            transaction_id: 0,
            date: today - DAY_TICKS + 120 * 10_000_000,
            type_id: TRITANIUM,
            quantity: 100,
            price: 50.0,
            station_id: STATION,
            buyer_id: ALICE,
            seller_id: ALICE,
            region_id: REGION,
            ..Default::default()
        });
        for pass in ["live trades", "after the rollover"] {
            assert_eq!(
                history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM),
                vec![expect_row(today - DAY_TICKS, &[(5.0, 10)])],
                "{pass}"
            );
            rollover(&mut reg);
        }
    }

    /// C5 §8: one row per `(region, type, day)` with a trade, low/high/
    /// volume-weighted average/volume/fills; old is every day before today
    /// and new is today (`GetHistoryRowList`, marketsvc.py:388). The
    /// readers answer the same before and after the rollover folds, and the
    /// folded rows survive the world file.
    #[test]
    fn the_aggregate_equals_the_trades() {
        let mut reg = market();
        let today = day_start(crate::pyutil::filetime_now());
        trade_on(&mut reg, 2, 60, TRITANIUM, 4.0, 10);
        trade_on(&mut reg, 2, 3_600, TRITANIUM, 6.0, 30);
        trade_on(&mut reg, 2, 7_200, OTHER_TYPE, 100.0, 1);
        trade_on(&mut reg, 1, 60, TRITANIUM, 5.0, 7);
        trade_on(&mut reg, 0, 0, TRITANIUM, 8.0, 3);
        let old = vec![
            expect_row(today - 2 * DAY_TICKS, &[(4.0, 10), (6.0, 30)]),
            expect_row(today - DAY_TICKS, &[(5.0, 7)]),
        ];
        let new = vec![expect_row(today, &[(8.0, 3)])];
        for pass in ["live trades", "after the rollover"] {
            assert_eq!(
                history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM),
                old,
                "{pass}"
            );
            assert_eq!(
                history_rows(&mut reg, "GetNewPriceHistory", TRITANIUM),
                new,
                "{pass}"
            );
            rollover(&mut reg);
        }
        // Today is not folded; the two whole days of tritanium and one of the
        // other type are.
        let folded: Vec<_> = reg.market.history.keys().copied().collect();
        assert_eq!(
            folded,
            [
                (REGION, TRITANIUM, today - 2 * DAY_TICKS),
                (REGION, TRITANIUM, today - DAY_TICKS),
                (REGION, OTHER_TYPE, today - 2 * DAY_TICKS),
            ]
        );
        // The ticker's batch call answers the same halves per type.
        let many = as_(
            &mut reg,
            ALICE,
            "GetHistoryForManyTypeIDs",
            &[PyValue::List(vec![PyValue::Int(OTHER_TYPE)])],
        )
        .unwrap();
        let PyValue::Dict(entries) = many else {
            panic!("not a dict")
        };
        assert_eq!(entries.len(), 1);
        let PyValue::Tuple(halves) = &entries[0].0 else {
            panic!("not a pair")
        };
        assert_eq!(rows(&halves[0]).len(), 1);
        assert!(rows(&halves[1]).is_empty());
        // And it survives the world file.
        let mut text = String::new();
        reg.market.render(&mut text);
        assert!(text.lines().any(|l| l.starts_with("mkthist\t")));
        let mut back = Book::default();
        for line in text.lines() {
            back.parse_record(&line.split('\t').collect::<Vec<_>>())
                .unwrap();
        }
        assert_eq!(back, reg.market);
    }

    /// The rollover drops trades and closed orders whose day is more than
    /// `HISTORY_DAYS` before today and keeps the day exactly `HISTORY_DAYS`
    /// back; an open order is kept however old, and a dropped day's history
    /// still reads from its `mkthist` row.
    #[test]
    fn pruning_keeps_history_days() {
        let mut reg = market();
        let today = day_start(crate::pyutil::filetime_now());
        trade_on(&mut reg, HISTORY_DAYS + 1, 0, TRITANIUM, 3.0, 1);
        trade_on(&mut reg, HISTORY_DAYS, 0, TRITANIUM, 4.0, 1);
        trade_on(&mut reg, 0, 0, TRITANIUM, 5.0, 1);
        let order = |id: i64, state: i64, days: i64| Order {
            order_id: id,
            char_id: ALICE,
            type_id: TRITANIUM,
            region_id: REGION,
            solar_system_id: SYSTEM,
            station_id: STATION,
            price: 1.0,
            vol_entered: 1,
            vol_remaining: 1,
            min_volume: 1,
            bid: false,
            issue_date: today - days * DAY_TICKS,
            duration: 90,
            range: RANGE_STATION,
            escrow: 0.0,
            state,
            last_state_change: (state != ORDER_STATE_OPEN).then_some(today - days * DAY_TICKS),
            corp_id: 0,
            key_id: 0,
        };
        for o in [
            order(1, ORDER_STATE_CANCELLED, HISTORY_DAYS + 1),
            order(2, ORDER_STATE_EXPIRED, HISTORY_DAYS),
            order(3, ORDER_STATE_OPEN, HISTORY_DAYS + 1),
        ] {
            reg.market.insert(o);
        }
        let before = history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM);
        assert_eq!(rollover(&mut reg), 2, "one trade and one order");
        let days: Vec<i64> = reg
            .market
            .trades
            .iter()
            .map(|t| day_start(t.date))
            .collect();
        assert_eq!(days, [today - HISTORY_DAYS * DAY_TICKS, today]);
        let ids: Vec<i64> = reg.market.all().map(|o| o.order_id).collect();
        assert_eq!(ids, [2, 3]);
        assert_eq!(
            history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM),
            before
        );
        assert_eq!(
            rollover(&mut reg),
            0,
            "a second rollover the same day drops nothing"
        );
    }

    /// A world file from before CP42: no `mkthist` line, and a `mkttrade`
    /// line with no region. It loads, and the trade is placed by the order
    /// at its station.
    #[test]
    fn a_world_with_no_history_loads() {
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, 5.0, 10, 3).unwrap();
        buy(&mut reg, BOB, 5.0, 4, 0).unwrap();
        let mut text = String::new();
        reg.market.render(&mut text);
        assert!(!text.contains("mkthist"));
        let mut back = Book::default();
        for line in text.lines() {
            let mut f: Vec<&str> = line.split('\t').collect();
            if f[0] == "mkttrade" {
                f.truncate(9);
            }
            back.parse_record(&f).unwrap();
        }
        assert!(back.history.is_empty());
        assert_eq!(back.trades[0].region_id, 0);
        let today = day_start(crate::pyutil::filetime_now());
        reg.market = back;
        assert_eq!(
            history_rows(&mut reg, "GetNewPriceHistory", TRITANIUM),
            [expect_row(today, &[(5.0, 4)])]
        );
    }

    // ---- the indexed book (E3) ----------------------------------------------

    /// A fixed-seed generator (Knuth's MMIX LCG): std only, the same book
    /// on every run.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, n: u64) -> i64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) % n) as i64
        }
    }

    /// An invented order: small id spaces so sides collide and prices tie.
    fn gen_order(g: &mut Lcg, order_id: i64) -> Order {
        let bid = g.next(2) == 1;
        let vol = 1 + g.next(5);
        Order {
            order_id,
            char_id: 1 + g.next(3),
            type_id: 10 + g.next(3),
            region_id: 1 + g.next(2),
            solar_system_id: 100,
            station_id: 1000 + g.next(2),
            price: [0.5, 1.0, 1.0, 2.25, 7.0][g.next(5) as usize],
            vol_entered: vol,
            vol_remaining: vol,
            min_volume: 1,
            bid,
            issue_date: g.next(4) * DAY_TICKS,
            duration: 1 + g.next(3),
            range: RANGE_STATION,
            escrow: if bid { 1.0 } else { 0.0 },
            state: ORDER_STATE_OPEN,
            last_state_change: None,
            corp_id: 0,
            key_id: 0,
        }
    }

    /// The pre-E3 book, as the reference: every order in a `Vec`, sides by
    /// a filter and a sort.
    fn linear_side(all: &[Order], type_id: i64, region_id: i64, bid: bool) -> Vec<Order> {
        let mut v: Vec<Order> = all
            .iter()
            .filter(|o| o.is_open() && o.bid == bid && o.type_id == type_id)
            .filter(|o| o.region_id == region_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| best_first(a, b, bid));
        v
    }

    fn indexed_side(book: &Book, type_id: i64, region_id: i64, bid: bool) -> Vec<Order> {
        book.side(type_id, region_id, bid)
            .into_iter()
            .cloned()
            .collect()
    }

    const KEYS: [(i64, i64); 6] = [(1, 10), (1, 11), (1, 12), (2, 10), (2, 11), (2, 12)];

    /// **E3**: over a generated run of inserts, fills, modifies, cancels,
    /// expiries and prunes, `get`, `side`, `open`, `all` and `due` answer
    /// exactly as a linear scan of a `Vec` would, a `(region, type)` whose
    /// sides changed always has a new stamp, and the world file round-trips.
    #[test]
    fn the_indexed_book_answers_like_a_linear_reference() {
        let mut g = Lcg(0x00E3_B00C);
        let mut book = Book::default();
        let mut reference: Vec<Order> = Vec::new();
        let mut next_id = 1;
        for step in 0..1500 {
            let before: Vec<(Vec<Order>, Vec<Order>, u64)> = KEYS
                .iter()
                .map(|&(r, t)| {
                    let (asks, bids) = (
                        linear_side(&reference, t, r, false),
                        linear_side(&reference, t, r, true),
                    );
                    (asks, bids, book.stamp(r, t))
                })
                .collect();
            let target = (!reference.is_empty())
                .then(|| reference[g.next(reference.len() as u64) as usize].order_id);
            let kind = g.next(3);
            let price = [0.5, 1.0, 3.0][g.next(3) as usize];
            let change = |o: &mut Order| match kind {
                0 => {
                    o.vol_remaining -= 1;
                    if o.vol_remaining <= 0 {
                        o.vol_remaining = 0;
                        o.state = ORDER_STATE_EXPIRED;
                        o.last_state_change = Some(step);
                    }
                }
                1 => o.price = price,
                _ => {
                    o.state = ORDER_STATE_CANCELLED;
                    o.last_state_change = Some(step);
                }
            };
            match (g.next(10), target) {
                (0..=4, _) | (_, None) => {
                    let mut o = gen_order(&mut g, next_id);
                    next_id += 1;
                    // Some are born closed, as an immediate order is.
                    if g.next(5) == 0 {
                        o.state = ORDER_STATE_EXPIRED;
                    }
                    book.insert(o.clone());
                    reference.push(o);
                }
                (5..=8, Some(id)) => {
                    change(reference.iter_mut().find(|o| o.order_id == id).unwrap());
                    book.update(id, change).unwrap();
                }
                _ => {
                    // Expire what is due at a random time, then prune half
                    // the closed orders, as the sweep and the rollover do.
                    let now = g.next(8) * DAY_TICKS;
                    let mut want: Vec<(i64, i64)> = reference
                        .iter()
                        .filter(|o| o.is_open() && o.expires_at(REAL) <= now)
                        .map(|o| (o.expires_at(REAL), o.order_id))
                        .collect();
                    want.sort();
                    let due = book.due(now);
                    assert_eq!(due, want.iter().map(|w| w.1).collect::<Vec<_>>());
                    for id in due {
                        let close = |o: &mut Order| o.state = ORDER_STATE_EXPIRED;
                        close(reference.iter_mut().find(|o| o.order_id == id).unwrap());
                        book.update(id, close).unwrap();
                    }
                    reference.retain(|o| o.is_open() || o.order_id % 2 == 0);
                    book.closed.retain(|id, _| id % 2 == 0);
                }
            }
            for o in &reference {
                assert_eq!(book.get(o.order_id), Some(o), "step {step}");
            }
            assert_eq!(book.get(next_id), None);
            let mut by_id = reference.clone();
            by_id.sort_by_key(|o| o.order_id);
            assert_eq!(book.all().cloned().collect::<Vec<_>>(), by_id);
            let open: Vec<Order> = by_id.iter().filter(|o| o.is_open()).cloned().collect();
            assert_eq!(book.open().cloned().collect::<Vec<_>>(), open);
            for (&(r, t), (asks, bids, stamp)) in KEYS.iter().zip(&before) {
                let now = (
                    linear_side(&reference, t, r, false),
                    linear_side(&reference, t, r, true),
                );
                assert_eq!(indexed_side(&book, t, r, false), now.0, "step {step}");
                assert_eq!(indexed_side(&book, t, r, true), now.1, "step {step}");
                if (&now.0, &now.1) != (asks, bids) {
                    assert_ne!(book.stamp(r, t), *stamp, "step {step}: a stale stamp");
                }
            }
        }
        assert!(
            book.closed.len() > 10 && book.open.len() > 10,
            "the run made both"
        );
        let mut text = String::new();
        book.render(&mut text);
        let mut back = Book::default();
        for line in text.lines() {
            back.parse_record(&line.split('\t').collect::<Vec<_>>())
                .unwrap();
        }
        assert_eq!(back, book);
        for &(r, t) in &KEYS {
            assert_eq!(
                indexed_side(&back, t, r, false),
                indexed_side(&book, t, r, false)
            );
        }
    }

    fn visits(book: &Book) -> u64 {
        book.stamps
            .visits
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// An invented order of `TRITANIUM` in `REGION`, open or closed.
    fn tritanium_order(order_id: i64, open: bool, bid: bool) -> Order {
        Order {
            order_id,
            char_id: ALICE,
            type_id: TRITANIUM,
            region_id: REGION,
            solar_system_id: SYSTEM,
            station_id: STATION,
            price: 1.0 + order_id as f64,
            vol_entered: 5,
            vol_remaining: if open { 5 } else { 0 },
            min_volume: 1,
            bid,
            issue_date: crate::pyutil::filetime_now(),
            duration: 90,
            range: RANGE_STATION,
            escrow: 0.0,
            state: if open {
                ORDER_STATE_OPEN
            } else {
                ORDER_STATE_EXPIRED
            },
            last_state_change: None,
            corp_id: 0,
            key_id: 0,
        }
    }

    /// **Browsing follows the pilot**: `GetOrders` and `GetSystemAsks` list the
    /// region and system the session names, not the home station's
    /// (`docs/lanes/stale-system.md`). Docked, the home region; in the roster
    /// in another region, that region's book, and the system ask is that
    /// system's.
    #[test]
    fn browsing_follows_the_pilot_into_another_region() {
        use crate::space_state::{Motion, Presence};
        const SYS_A: i64 = 7_001;
        const SYS_B: i64 = 7_002;
        const STATION_B: i64 = 60_007_002;
        let config = SandboxConfig {
            celestials: crate::celestial::Catalogue::parse(
                "#system\t7001\t10\t100\n#system\t7002\t11\t101\n",
            ),
            characters: vec![Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: 100_000_001,
                station_id: Some(STATION),
                solar_system_id: Some(SYS_A),
                region_id: 100,
                ..Character::default()
            }],
            ..SandboxConfig::default()
        };

        let mut reg = Registry::new(config);
        let ask = |id, region, system, station| Order {
            region_id: region,
            solar_system_id: system,
            station_id: station,
            ..tritanium_order(id, true, false)
        };
        reg.market.insert(ask(1, 100, SYS_A, STATION));
        reg.market.insert(ask(2, 101, SYS_B, STATION_B));
        let ids = |reg: &mut Registry| -> Vec<DbValue> {
            book(reg, ALICE)
                .0
                .iter()
                .map(|r| col(r, "orderID"))
                .collect()
        };
        assert_eq!(
            ids(&mut reg),
            vec![DbValue::Int(1)],
            "docked: the home region"
        );
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            100_000_001,
            SYS_B,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        assert_eq!(ids(&mut reg), vec![DbValue::Int(2)], "in space: B's region");
        let PyValue::Dict(asks) = as_(&mut reg, ALICE, "GetSystemAsks", &[]).unwrap() else {
            panic!("GetSystemAsks is a dict");
        };
        assert_eq!(asks.len(), 1, "only B's system");
        assert!(
            matches!(&asks[0].0, PyValue::Tuple(t) if t[3] == PyValue::Int(STATION_B)),
            "{asks:?}"
        );
    }

    /// **E3**: `side` reads the open orders of its side and nothing else,
    /// however many closed orders of the same type and region there are.
    #[test]
    fn side_never_visits_a_closed_order() {
        let mut book = Book::default();
        for id in 1..=200 {
            book.insert(tritanium_order(id, id % 50 == 0, false));
        }
        book.insert(tritanium_order(201, true, true));
        let start = visits(&book);
        let asks = book.side(TRITANIUM, REGION, false);
        assert_eq!(asks.len(), 4);
        assert_eq!(visits(&book) - start, 4, "only the four open asks");
        assert!(book.side(TRITANIUM, REGION + 1, false).is_empty());
        assert_eq!(visits(&book) - start, 4);
    }

    /// **E3** (port-tracker §78's follow-up): a `GetOrders` cache hit is
    /// validated by the `(region, type)` stamp alone and visits no order; a
    /// build visits only the open ones; a closed order written later does
    /// not leave the open sides stale.
    #[test]
    fn get_orders_cache_validation_visits_no_order() {
        let mut reg = market();
        for id in 1..=300 {
            reg.market
                .insert(tritanium_order(FIRST_ORDER_ID + id, id <= 2, id == 2));
        }
        let now = crate::pyutil::filetime_now();
        let start = visits(&reg.market);
        let (first, n) = cached_book(&mut reg, now);
        let built = visits(&reg.market) - start;
        assert_eq!(built, 2, "a build reads the two open orders only");
        assert_eq!(cached_book(&mut reg, now), (first.clone(), n));
        assert_eq!(visits(&reg.market) - start, built, "a hit reads none");
        reg.market
            .update(FIRST_ORDER_ID + 1, |o| o.price += 1.0)
            .unwrap();
        let (moved, n2) = cached_book(&mut reg, now);
        assert_eq!(n2, n + 1, "a modify rebuilds");
        assert_ne!(moved, first);
    }

    // ---- the ledger log (E4) -----------------------------------------------

    /// The world file and its ledger log, saved and loaded as `server.rs`
    /// does: `FileStore`, `apply_snapshot`, `ledger::open`.
    fn reload(reg: &Registry, file: &std::path::Path) -> Registry {
        reg.journal().log().sync();
        let store = crate::store::FileStore::new(file);
        crate::store::Store::save(&store, &reg.to_snapshot()).expect("saves");
        let mut back = market();
        back.apply_snapshot(crate::store::Store::load(&store).unwrap().expect("a world"));
        crate::ledger::open(&mut back, &crate::ledger::dir_of(file)).expect("opens");
        back
    }

    /// `ledger::meta` reads the id, date and owners of the lines this
    /// module writes, so retention cannot misread a market row.
    #[test]
    fn the_ledger_reads_the_market_lines() {
        let mut o = tritanium_order(FIRST_ORDER_ID + 1, false, true);
        o.last_state_change = Some(o.issue_date + 7);
        let m = crate::ledger::meta(&order_line(&o)).expect("an order line");
        assert_eq!(
            (m.id, m.date, m.owners, m.held),
            (o.order_id, o.issue_date + 7, vec![ALICE], false)
        );
        o.last_state_change = None;
        o.escrow = 2.5;
        let m = crate::ledger::meta(&order_line(&o)).expect("an order line");
        assert_eq!((m.date, m.held), (o.issue_date, true));
        let t = Trade {
            transaction_id: 12,
            date: 99,
            type_id: TRITANIUM,
            quantity: 3,
            price: 4.5,
            station_id: STATION,
            buyer_id: BOB,
            seller_id: ALICE,
            region_id: REGION,
            buyer_corp: 7,
            buyer_key: 1001,
            seller_corp: 0,
            seller_key: 0,
        };
        let m = crate::ledger::meta(&trade_line(&t)).expect("a trade line");
        assert_eq!(
            (m.kind, m.id, m.date, m.owners),
            (crate::ledger::Kind::Market, 12, 99, vec![BOB, ALICE])
        );
        let line = trade_line(&t);
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        assert_eq!(parse_trade(&f), Ok(t));
    }

    /// **E4**: trades and closed orders go to the log and never into the
    /// world file; the world file plus the log reloads to the same book and
    /// journal, a closed order rewritten after it closed included.
    #[test]
    fn trades_and_closed_orders_go_to_the_log_and_a_reload_equals_ram() {
        let dir = crate::testkit::scratch_dir("e4-market");
        let file = dir.as_ref().join("world.txt");
        let mut reg = market();
        crate::ledger::open(&mut reg, &crate::ledger::dir_of(&file)).expect("opens");
        trade_on(&mut reg, 0, 10, TRITANIUM, 5.0, 2);
        trade_on(&mut reg, 1, 10, TRITANIUM, 6.0, 1);
        let open = tritanium_order(FIRST_ORDER_ID + 1, true, false);
        reg.market.insert(open.clone());
        reg.market
            .insert(tritanium_order(FIRST_ORDER_ID + 2, false, true));
        // Inserted by hand, so the allocator is moved by hand.
        reg.market.next_order_id = FIRST_ORDER_ID + 3;
        reg.market
            .update(FIRST_ORDER_ID + 2, |o| o.vol_entered = 6)
            .unwrap();
        reg.charge(ALICE, 10.0, 2, BOB, 0).expect("charged");
        let mut text = String::new();
        reg.to_snapshot().market.render(&mut text);
        assert!(!text.contains("mkttrade"), "{text}");
        let orders: Vec<&str> = text.lines().filter(|l| l.starts_with("mktorder")).collect();
        assert_eq!(orders, [order_line(&open).trim_end()]);
        assert!(reg.to_snapshot().journal.is_empty());
        let back = reload(&reg, &file);
        assert_eq!(back.market, reg.market);
        assert_eq!(back.journal().entries(), reg.journal().entries());
        assert_eq!(
            back.market.get(FIRST_ORDER_ID + 2).unwrap().vol_entered,
            6,
            "the last line wins"
        );
    }

    /// A pre-E4 world file with trades and closed orders loads, moves them
    /// into the log, and its next save holds them no longer.
    #[test]
    fn a_pre_e4_world_with_trades_in_the_snapshot_loads() {
        let dir = crate::testkit::scratch_dir("e4-market-migrate");
        let file = dir.as_ref().join("world.txt");
        let mut old = market();
        trade_on(&mut old, 0, 10, TRITANIUM, 5.0, 2);
        old.market
            .insert(tritanium_order(FIRST_ORDER_ID + 1, false, false));
        old.market.next_order_id = FIRST_ORDER_ID + 2;
        old.charge(ALICE, 10.0, 2, BOB, 0).expect("charged");
        let mut text = String::new();
        old.to_snapshot().market.render(&mut text);
        assert!(
            text.contains("mkttrade") && text.contains("mktorder"),
            "detached: as before"
        );
        let back = reload(&old, &file);
        assert_eq!(back.market, old.market);
        assert_eq!(back.journal().entries(), old.journal().entries());
        let mut after = String::new();
        back.to_snapshot().market.render(&mut after);
        assert!(
            !after.contains("mkttrade") && !after.contains("mktorder"),
            "{after}"
        );
        // And the log alone carries them through the next reload.
        let again = reload(&back, &file);
        assert_eq!(again.market, old.market);
        assert_eq!(again.journal().entries(), old.journal().entries());
    }

    /// A reload leaves out what the last rollover's CP42 window dropped,
    /// as RAM did, while the log keeps a human's trade for the client.
    #[test]
    fn a_reload_keeps_the_market_window_the_last_rollover_kept() {
        let dir = crate::testkit::scratch_dir("e4-market-window");
        let file = dir.as_ref().join("world.txt");
        let mut reg = market();
        crate::ledger::open(&mut reg, &crate::ledger::dir_of(&file)).expect("opens");
        trade_on(&mut reg, HISTORY_DAYS + 1, 0, TRITANIUM, 3.0, 1);
        trade_on(&mut reg, 0, 0, TRITANIUM, 5.0, 1);
        let now = crate::pyutil::filetime_now();
        let mut world = *reg.world();
        world.last_rollover = now;
        reg.deltas_mut().set_world(world);
        assert_eq!(rollover(&mut reg), 1);
        let back = reload(&reg, &file);
        assert_eq!(back.market, reg.market);
        let files = std::fs::read_dir(crate::ledger::dir_of(&file))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "tsv")
            })
            .count();
        assert_eq!(files, 2, "both days' files are still on disk");
    }

    // ---- the market's sinks (CP9, E5) ---------------------------------------

    /// The key of a raised `UserError`.
    fn raised_key(err: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
            panic!("not a raised exception: {err:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a 0x22 header is a tuple")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's header args are a tuple")
        };
        let key = crate::pyutil::as_bytes(&args[0]).expect("a key");
        String::from_utf8_lossy(key).into_owned()
    }

    /// Make an order's last operation `MODIFICATION_DELAY_S` old.
    fn age(reg: &mut Registry, order_id: i64) {
        let back = MODIFICATION_DELAY_S * 10_000_000;
        reg.market.update(order_id, |o| {
            o.issue_date -= back;
            o.last_state_change = o.last_state_change.map(|t| t - back);
        });
    }

    /// `ModifyCharOrder`'s nine arguments, the client's view of a buy order
    /// of 10 at `seen` (marketsvc.py:333).
    fn modify(
        reg: &mut Registry,
        who: i64,
        order_id: i64,
        seen: f64,
        price: f64,
    ) -> Result<PyValue, ServiceError> {
        as_(
            reg,
            who,
            "ModifyCharOrder",
            &[
                PyValue::Int(order_id),
                PyValue::Real(price),
                PyValue::Bool(true),
                PyValue::Int(STATION),
                PyValue::Int(SYSTEM),
                PyValue::Real(seen),
                PyValue::Int(RANGE_STATION),
                PyValue::Real(10.0),
                PyValue::Int(0),
            ],
        )
    }

    /// Give `who` `level` in `skill`.
    fn train(reg: &mut Registry, who: i64, skill: i64, level: i64) {
        let mut entries: Vec<crate::skill::SkillEntry> = reg
            .skills_for(Some(who))
            .map(|s| s.entries().cloned().collect())
            .unwrap_or_default();
        entries.retain(|e| e.type_id != skill);
        entries.push(crate::skill::SkillEntry {
            type_id: skill,
            level,
            points: 0,
            rank: 1,
        });
        reg.skills
            .insert(who, crate::skill::Skills::restore(entries));
    }

    /// `who`'s journal rows of `entry_type_id`, as `(amount, reference_id)`.
    fn journal_rows(reg: &Registry, who: i64, entry_type_id: i64) -> Vec<f64> {
        reg.journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == entry_type_id && e.wallet() == who)
            .map(|e| e.amount)
            .collect()
    }

    /// **CP9**: the sales tax equals the client's formula at Accounting 0
    /// and 5 (`skilllimits.py:86-87`), and a sale books it as one ref-54 row
    /// to the seller only, both selling into a bid and an ask being bought.
    #[test]
    fn sales_tax_equals_the_client_formula_and_is_journalled_as_ref_54_to_the_seller() {
        let formula = |v: f64, l: i64| {
            cents(v * SALES_TAX_PERCENT / 100.0 * (1.0 - ACCOUNTING_SKILL_MODIFIER * l as f64))
        };
        for (v, l) in [(1_000.0, 0), (1_000.0, 5), (123_456.78, 3), (0.5, 0)] {
            assert_eq!(sales_tax(v, l), formula(v, l), "{v} at {l}");
        }
        assert!(sales_tax(1_000.0, 5) < sales_tax(1_000.0, 0));
        // Into a bid: Alice sells to Bob's bid.
        let mut reg = market();
        train(&mut reg, ALICE, TYPE_ACCOUNTING, 5);
        buy(&mut reg, BOB, 6.0, 30, 1).unwrap();
        let item = stack(&mut reg, ALICE, 10);
        let before = balance(&reg, ALICE);
        sell(&mut reg, ALICE, item, 6.0, 10, 0).unwrap();
        let tax = sales_tax(60.0, 5);
        assert_eq!(journal_rows(&reg, ALICE, REF_TRANSACTION_TAX), [-tax]);
        assert!(journal_rows(&reg, BOB, REF_TRANSACTION_TAX).is_empty());
        assert_eq!(balance(&reg, ALICE), cents(before + 60.0 - tax));
        // An ask bought: Alice's resting ask, Bob buying.
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 100);
        sell(&mut reg, ALICE, item, 5.0, 100, 3).unwrap();
        let before = balance(&reg, ALICE);
        buy(&mut reg, BOB, 5.0, 40, 0).unwrap();
        let tax = sales_tax(200.0, 0);
        assert_eq!(journal_rows(&reg, ALICE, REF_TRANSACTION_TAX), [-tax]);
        assert!(journal_rows(&reg, BOB, REF_TRANSACTION_TAX).is_empty());
        assert_eq!(balance(&reg, ALICE), cents(before + 200.0 - tax));
    }

    /// **CP9**: the broker fee equals the server formula, its minimum is
    /// `mktMinimumFee`, Broker Relations lowers it, and whatever fraction
    /// the client sends as `expectedBrokersFee` is clamped to it.
    #[test]
    fn the_broker_fee_equals_the_server_formula_and_a_client_figure_is_clamped() {
        for (v, br) in [(100_000.0, 0), (100_000.0, 5), (10.0, 0), (3_333.0, 0)] {
            let rate = (BROKER_PERCENT - BROKER_RELATIONS_SKILL_MODIFIER * br as f64) / 100.0;
            assert_eq!(broker_fee(v, br), cents(v * rate).max(MINIMUM_BROKER_FEE));
        }
        assert_eq!(broker_fee(10.0, 0), MINIMUM_BROKER_FEE);
        let fee_paid = |client: f64, br: i64| {
            let mut reg = market();
            train(&mut reg, BOB, TYPE_BROKER_RELATIONS, br);
            let args = [
                PyValue::Int(STATION),
                PyValue::Int(TRITANIUM),
                PyValue::Real(100.0),
                PyValue::Int(1_000),
                PyValue::Int(RANGE_STATION),
                PyValue::Int(1),
                PyValue::Int(3),
                PyValue::Bool(false),
                PyValue::Real(client),
            ];
            as_(&mut reg, BOB, "PlaceBuyOrder", &args).expect("rests");
            journal_rows(&reg, BOB, REF_BROKER_FEE)
        };
        for client in [0.0, 0.03, 0.9, -1.0] {
            assert_eq!(fee_paid(client, 0), [-broker_fee(100_000.0, 0)], "{client}");
        }
        assert_eq!(fee_paid(0.0, 5), [-broker_fee(100_000.0, 5)]);
        // A sell order pays it too, on the value that rests.
        let mut reg = market();
        let item = stack(&mut reg, ALICE, 10);
        sell(&mut reg, ALICE, item, 5.0, 10, 3).unwrap();
        let fee = broker_fee(50.0, 0);
        assert_eq!(journal_rows(&reg, ALICE, REF_BROKER_FEE), [-fee]);
    }

    /// A resting buy of `qty` at `price` with `range`, `PlaceBuyOrder`.
    fn rest_buy(
        reg: &mut Registry,
        who: i64,
        price: f64,
        qty: i64,
        range: i64,
    ) -> Result<PyValue, ServiceError> {
        as_(
            reg,
            who,
            "PlaceBuyOrder",
            &[
                PyValue::Int(STATION),
                PyValue::Int(TRITANIUM),
                PyValue::Real(price),
                PyValue::Int(qty),
                PyValue::Int(range),
                PyValue::Int(1),
                PyValue::Int(3),
                PyValue::Bool(false),
                PyValue::Real(0.0),
            ],
        )
    }

    /// **X-TRADE**: a standing with the station owner (and its faction)
    /// lowers the broker's commission by `0.0003 x faction + 0.0002 x corp`
    /// (`brokerFee.py:85`), on top of Broker Relations; no standing, no
    /// change. The owner is the fixture pilots' station owner.
    #[test]
    fn standings_with_the_station_owner_lower_the_brokers_fee() {
        const FACTION: i64 = 500_990;
        let owner = Character::default().station_owner_id;
        let mut table = crate::npc_data::Table::default();
        table.corporations.insert(
            owner,
            crate::npc_data::Corporation {
                corp_id: owner,
                faction_id: Some(FACTION),
                ..Default::default()
            },
        );
        let mut content = crate::content::Content::fixture();
        content.insert("npc_data", table);
        let mut reg = market().with_content(std::sync::Arc::new(content));
        let value = 300_000.0;
        let plain = broker_rate_at(&reg, BOB, STATION);
        assert_eq!(plain, fees::broker_rate(0, Standings::default()));
        reg.set_npc_standing(BOB, owner, 5.0);
        reg.set_npc_standing(BOB, FACTION, 3.0);
        train(&mut reg, BOB, TYPE_BROKER_RELATIONS, 2);
        let liked = broker_rate_at(&reg, BOB, STATION);
        let want = (BROKER_PERCENT - 2.0 * BROKER_RELATIONS_SKILL_MODIFIER) / 100.0
            - 3.0 * fees::FACTION_STANDING_MULTIPLIER
            - 5.0 * fees::NPC_CORP_STANDING_MULTIPLIER;
        assert!((liked - want).abs() < 1e-12, "{liked} vs {want}");
        // The fee charged is that rate on the escrow, journalled as ref 46.
        rest_buy(&mut reg, BOB, 1_000.0, 300, RANGE_STATION).expect("rests");
        assert_eq!(
            journal_rows(&reg, BOB, REF_BROKER_FEE),
            [-fees::fee_with_floor(cents(value * want))]
        );
        // Alice, with no standings, pays the plain rate.
        rest_buy(&mut reg, ALICE, 1_000.0, 300, RANGE_STATION).expect("rests");
        assert_eq!(
            journal_rows(&reg, ALICE, REF_BROKER_FEE),
            [-broker_fee(value, 0)]
        );
    }

    /// **X-TRADE**: changing a resting order costs the modification fee,
    /// `new x rate x (1 - (0.5 + 0.06 x Margin Trading))` (plus the
    /// commission on any growth), floored at the minimum; Margin Trading
    /// lowers it; a refusal for want of ISK changes nothing.
    #[test]
    fn modifying_an_order_costs_the_modification_fee_that_margin_trading_lowers() {
        let fee_of = |margin: i64, new_price: f64| {
            let mut reg = market();
            train(&mut reg, BOB, TYPE_MARGIN_TRADING, margin);
            rest_buy(&mut reg, BOB, 1_000.0, 300, RANGE_STATION).expect("rests");
            let order_id = reg.market.open().next().expect("the order").order_id;
            age(&mut reg, order_id);
            let paid_before = journal_rows(&reg, BOB, REF_BROKER_FEE).len();
            as_(
                &mut reg,
                BOB,
                "ModifyCharOrder",
                &[
                    PyValue::Int(order_id),
                    PyValue::Real(new_price),
                    PyValue::Bool(true),
                    PyValue::Int(STATION),
                    PyValue::Int(SYSTEM),
                    PyValue::Real(1_000.0),
                    PyValue::Int(RANGE_STATION),
                    PyValue::Int(300),
                    PyValue::Int(0),
                ],
            )
            .expect("modifies");
            let rows = journal_rows(&reg, BOB, REF_BROKER_FEE);
            assert_eq!(rows.len(), paid_before + 1, "one fee row for the modify");
            -rows[rows.len() - 1]
        };
        let rate = fees::broker_rate(0, Standings::default());
        // A cut: 300 x 900 = 270,000 at the discounted modification fee only.
        for margin in [0, 5] {
            let discount = fees::modification_discount(margin);
            let want = cents(270_000.0 * rate * (1.0 - discount));
            assert_eq!(fee_of(margin, 900.0), want, "margin {margin}");
        }
        assert!(fee_of(5, 900.0) < fee_of(0, 900.0));
        // A raise adds the commission on the growth.
        let want =
            cents(330_000.0 * rate * (1.0 - fees::modification_discount(0)) + 30_000.0 * rate);
        assert_eq!(fee_of(0, 1_100.0), want);
        // A tiny order is held to the minimum.
        let mut reg = market();
        rest_buy(&mut reg, BOB, 4.0, 10, RANGE_STATION).unwrap();
        let order_id = reg.market.open().next().unwrap().order_id;
        age(&mut reg, order_id);
        let before = balance(&reg, BOB);
        modify(&mut reg, BOB, order_id, 4.0, 3.0).expect("modifies");
        assert_eq!(
            balance(&reg, BOB),
            cents(before + 10.0 - MINIMUM_BROKER_FEE)
        );
        // Too poor for the fee: refused, nothing moved.
        let mut reg = market();
        rest_buy(&mut reg, BOB, 4.0, 10, RANGE_STATION).unwrap();
        let order_id = reg.market.open().next().unwrap().order_id;
        age(&mut reg, order_id);
        reg.character_mut(BOB).unwrap().balance = MINIMUM_BROKER_FEE / 2.0;
        assert!(modify(&mut reg, BOB, order_id, 4.0, 3.0).is_err());
        assert_eq!(reg.market.get(order_id).unwrap().price, 4.0);
        assert_eq!(balance(&reg, BOB), MINIMUM_BROKER_FEE / 2.0);
    }

    /// **X-TRADE**: a resting buy order's range is held to the pilot's
    /// Visibility (`buyThisTypeWindow._GetAvailableRanges`): the station
    /// with none, the region at V.
    #[test]
    fn a_resting_buys_range_is_held_to_visibility() {
        let range_of = |visibility: i64, asked: i64| {
            let mut reg = market();
            train(&mut reg, BOB, fees::TYPE_VISIBILITY, visibility);
            rest_buy(&mut reg, BOB, 1.0, 10, asked).expect("rests");
            reg.market.open().next().expect("the order").range
        };
        assert_eq!(range_of(0, RANGE_REGION), RANGE_STATION);
        assert_eq!(range_of(1, RANGE_REGION), RANGE_SOLAR_SYSTEM);
        assert_eq!(range_of(2, 10), 5);
        assert_eq!(range_of(5, RANGE_REGION), RANGE_REGION);
        assert_eq!(range_of(0, RANGE_STATION), RANGE_STATION);
    }

    /// **X-TRADE (the loop)**: the sales tax the server actually takes
    /// decides a thin loop: an invented ask and bid 6 % apart lose money at
    /// Accounting 0 and make it at Accounting IV. The bots' price rule uses
    /// the same formula (`market_fees`, `eve-bot` `trader::tests`).
    #[test]
    fn the_sales_tax_the_server_takes_decides_a_thin_loop() {
        // An invented loop: buy at 100.00, sell into a bid of 106.00.
        let (ask, bid) = (100.0, 106.0);
        let net = |accounting: i64| bid - sales_tax(bid, accounting) - ask;
        assert!(net(0) < 0.0, "8 % tax eats a 6 % spread: {}", net(0));
        assert!(net(4) > 0.0, "Accounting IV: {}", net(4));
        assert!(net(5) > net(4));
    }

    /// **E5**: an order past `5 + 4·Trade + 8·Retail + 16·Wholesale +
    /// 32·Tycoon` open orders is refused with `MarketExceededOrderCount`,
    /// and moves nothing; an immediate order is never counted.
    #[test]
    fn an_order_past_the_max_order_formula_is_refused() {
        assert_eq!(max_orders(0, 0, 0, 0), 5);
        assert_eq!(max_orders(5, 5, 5, 5), 5 + 20 + 40 + 80 + 160);
        let mut reg = market();
        let cap = max_orders(0, 0, 0, 0);
        for _ in 0..cap {
            buy(&mut reg, BOB, 1.0, 1, 3).expect("under the cap");
        }
        let before = balance(&reg, BOB);
        let e = buy(&mut reg, BOB, 1.0, 1, 3).expect_err("over the cap");
        assert_eq!(raised_key(&e), "MarketExceededOrderCount");
        assert_eq!(balance(&reg, BOB), before, "nothing moved");
        let item = stack(&mut reg, BOB, 10);
        let e = sell(&mut reg, BOB, item, 9.0, 10, 3).expect_err("a sell too");
        assert_eq!(raised_key(&e), "MarketExceededOrderCount");
        assert_eq!(units(&reg, BOB), 10);
        // An immediate buy that matches nothing rests nothing: not the cap.
        let e = buy(&mut reg, BOB, 1.0, 1, 0).expect_err("nothing to match");
        assert_eq!(raised_key(&e), "MktOrderDidNotMatch");
        train(&mut reg, BOB, TYPE_TRADE, 1);
        let more = max_orders(1, 0, 0, 0) - cap;
        for _ in 0..more {
            buy(&mut reg, BOB, 1.0, 1, 3).expect("Trade raised the cap");
        }
        assert!(buy(&mut reg, BOB, 1.0, 1, 3).is_err());
    }

    /// **E5**: a modify inside `mktModificationDelay` of the order's last
    /// operation is refused with `MktOrderDelay` and changes nothing; after
    /// it, the modify lands and restarts the wait.
    #[test]
    fn a_modify_inside_the_modification_delay_is_refused() {
        let mut reg = market();
        buy(&mut reg, BOB, 4.0, 10, 1).unwrap();
        let mine = rows(&as_(&mut reg, BOB, "GetCharOrders", &[]).unwrap());
        let DbValue::Int(order_id) = col(&mine[0], "orderID") else {
            panic!("orderID");
        };
        let e = modify(&mut reg, BOB, order_id, 4.0, 4.5).expect_err("too soon");
        assert_eq!(raised_key(&e), "MktOrderDelay");
        assert_eq!(reg.market.get(order_id).unwrap().price, 4.0);
        age(&mut reg, order_id);
        modify(&mut reg, BOB, order_id, 4.0, 4.5).expect("after the delay");
        let e = modify(&mut reg, BOB, order_id, 4.5, 5.0).expect_err("the wait restarts");
        assert_eq!(raised_key(&e), "MktOrderDelay");
        let o = reg.market.get(order_id).unwrap().clone();
        assert_eq!(o.price, 4.5);
        let now = o.last_state_change.expect("the modify's time");
        let ticks = MODIFICATION_DELAY_S * 10_000_000;
        assert_eq!(modification_wait(&o, now, REAL), MODIFICATION_DELAY_S);
        assert_eq!(modification_wait(&o, now + ticks - 1, REAL), 1);
        assert_eq!(modification_wait(&o, now + ticks, REAL), 0);
        assert_eq!(wait_text(299), "4 minutes 59 seconds");
    }

    /// Lane clock: at scale N an order expires N times sooner, the index
    /// built before `set_pace` is rebuilt, and the modify delay is N times
    /// shorter.
    #[test]
    fn at_scale_n_an_order_expires_and_unlocks_n_times_sooner() {
        let clock = crate::game_clock::GameClock::new(4).unwrap();
        let issued = 9_000 * DAY_TICKS;
        let o = Order {
            order_id: 77,
            char_id: ALICE,
            type_id: TRITANIUM,
            region_id: REGION,
            solar_system_id: SYSTEM,
            station_id: STATION,
            price: 1.0,
            vol_entered: 1,
            vol_remaining: 1,
            min_volume: 1,
            bid: false,
            issue_date: issued,
            duration: 2,
            range: RANGE_STATION,
            escrow: 0.0,
            state: ORDER_STATE_OPEN,
            last_state_change: None,
            corp_id: 0,
            key_id: 0,
        };
        let mut book = Book::default();
        book.insert(o.clone());
        assert_eq!(book.due(issued + 2 * DAY_TICKS - 1), Vec::<i64>::new());
        assert_eq!(book.due(issued + 2 * DAY_TICKS), vec![77]);
        book.set_pace(clock);
        let fast = issued + 2 * DAY_TICKS / 4;
        assert_eq!(book.due(fast - 1), Vec::<i64>::new());
        assert_eq!(book.due(fast), vec![77]);
        book.update(77, |o| o.state = ORDER_STATE_EXPIRED).unwrap();
        assert!(
            book.due(issued + 3 * DAY_TICKS).is_empty(),
            "the index lost it"
        );

        let ticks = MODIFICATION_DELAY_S * 10_000_000;
        assert_eq!(modification_wait(&o, issued + ticks / 4 - 1, clock), 1);
        assert_eq!(modification_wait(&o, issued + ticks / 4, clock), 0);
        assert_eq!(
            modification_wait(&o, issued, clock),
            MODIFICATION_DELAY_S / 4
        );
    }

    /// **E5 (E7's finding)**: a starter type may not be listed for sale, to
    /// a player bid, an NPC bid or as an ask: `MktInvalidMarketTypeAsk`, and
    /// the stack stays.
    #[test]
    fn a_starter_type_cannot_be_sold() {
        let mut starter = crate::starter_ship::Table::default();
        starter.add_fit(&[(OTHER_TYPE, TRITANIUM)]);
        let mut c = crate::content::Content::fixture();
        c.insert(crate::starter_ship::TABLE.name, starter);
        let mut reg = market().with_content(std::sync::Arc::new(c));
        buy(&mut reg, BOB, 6.0, 30, 1).unwrap();
        let item = stack(&mut reg, ALICE, 10);
        for days in [0, 3] {
            let e = sell(&mut reg, ALICE, item, 6.0, 10, days).expect_err("starter");
            assert_eq!(raised_key(&e), "MktInvalidMarketTypeAsk");
        }
        assert_eq!(units(&reg, ALICE), 10);
        assert_eq!(units(&reg, BOB), 0);
    }

    /// Two bots docked in one station: the market world whose rows the
    /// ledger treats as a bot's.
    fn bot_market() -> Registry {
        let mut config = market().config().clone();
        for c in &mut config.characters {
            c.is_bot = true;
        }
        Registry::new(config)
    }

    /// **E5 (E4's Not done)**: a folded day is read from `mkthist`, so a
    /// bot's trades leave RAM `BOT_LEDGER_DAYS` after their day (the rule
    /// `ledger::Rules` applies to the log too) and the history reads the
    /// same; a human's stay for `HISTORY_DAYS`.
    #[test]
    fn bot_market_rows_drop_after_bot_ledger_days_and_the_history_reads_mkthist() {
        let mut reg = bot_market();
        let gone = crate::ledger::BOT_LEDGER_DAYS + 1;
        trade_on(&mut reg, gone, 60, TRITANIUM, 4.0, 10);
        trade_on(&mut reg, gone - 1, 60, TRITANIUM, 5.0, 3);
        trade_on(&mut reg, 0, 0, TRITANIUM, 8.0, 3);
        let old = history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM);
        assert_eq!(old.len(), 2);
        assert_eq!(rollover(&mut reg), 1, "the bot trade past BOT_LEDGER_DAYS");
        let days: Vec<i64> = reg.market.trades.iter().map(|t| t.price as i64).collect();
        assert_eq!(days, [5, 8]);
        assert_eq!(history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM), old);
        // The same rule says so for the log's lines.
        let now = crate::pyutil::filetime_now();
        let rules = crate::ledger::Rules::of(&reg, now);
        let t = &reg.market.trades[0];
        assert!(!Registry::ledger_drops(&rules, &trade_line(t)));
        let mut old_trade = t.clone();
        old_trade.date -= DAY_TICKS;
        assert!(Registry::ledger_drops(&rules, &trade_line(&old_trade)));
        // A human's trade of the same age stays.
        let mut reg = market();
        trade_on(&mut reg, gone, 60, TRITANIUM, 4.0, 10);
        assert_eq!(rollover(&mut reg), 0);
    }

    /// **E5**: a folded day keeps its `mkthist` row: a trade of that day
    /// recorded after the fold changes neither the row nor the reader, and a
    /// second rollover does not refold it from what is left.
    #[test]
    fn the_history_reader_takes_mkthist_for_a_folded_day() {
        let mut reg = market();
        let today = day_start(crate::pyutil::filetime_now());
        trade_on(&mut reg, 1, 60, TRITANIUM, 4.0, 10);
        rollover(&mut reg);
        let row = reg.market.history[&(REGION, TRITANIUM, today - DAY_TICKS)];
        let old = history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM);
        trade_on(&mut reg, 1, 120, TRITANIUM, 9.0, 1);
        assert_eq!(history_rows(&mut reg, "GetOldPriceHistory", TRITANIUM), old);
        rollover(&mut reg);
        assert_eq!(
            reg.market.history[&(REGION, TRITANIUM, today - DAY_TICKS)],
            row
        );
    }

    /// **E5 (P6)**: an NPC fill on another type keeps a cached book.
    #[test]
    fn an_npc_fill_on_another_type_keeps_the_cached_book() {
        let mut reg = npc_market(true);
        let now = crate::pyutil::filetime_now();
        let (_, n0) = cached_book(&mut reg, now);
        let at = (STATION, OTHER_TYPE, exchange::Side::Ask);
        exchange::take(&mut reg, at, 10, 1, now);
        assert_eq!(cached_book(&mut reg, now).1, n0, "another type's row");
        let at = (STATION, TRITANIUM, exchange::Side::Ask);
        exchange::take(&mut reg, at, 1_000, 1, now);
        assert_eq!(cached_book(&mut reg, now).1, n0 + 1, "its own row");
    }

    // ---- CORP-BUY: a corporation's orders -------------------------------------

    /// Invented roles, from `corp_wallet`'s named constants.
    const TRADER: i64 = crate::corp_wallet::ROLE_TRADER;
    const ACCOUNTANT: i64 = crate::corp_wallet::ROLE_ACCOUNTANT;
    /// ISK in the invented corporation's Master Wallet.
    const FLOAT: f64 = 100_000.0;

    /// Alice founds a corporation and is its accountant; Bob is a member with
    /// `bob_roles`. The Master Wallet holds [`FLOAT`]. Answers the corp id.
    fn corp_world(reg: &mut Registry, bob_roles: i64) -> i64 {
        reg.character_mut(ALICE).unwrap().balance = 5_000_000.0;
        let u = |s: &str| PyValue::Unicode(s.to_owned());
        let args = vec![
            u("Buy Test"),
            u("BUYT"),
            u("We trade."),
            u("http://"),
            PyValue::Real(0.0),
            PyValue::Int(419),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Bool(true),
            PyValue::Bool(false),
            PyValue::Real(0.0),
        ];
        reg.dispatch(&Request {
            service: "corpRegistry",
            method: "AddCorporation",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("founded");
        let corp = reg.character(ALICE).expect("a pilot").corporation_id;
        reg.corporations.enrol_for_test(BOB, corp, bob_roles);
        reg.corporations.member_mut(ALICE).unwrap().roles |= ACCOUNTANT | TRADER;
        crate::corp_wallet::adjust(
            reg.deltas_mut(),
            corp,
            crate::corp_wallet::MASTER_KEY,
            crate::corp_wallet::to_cents(FLOAT),
        )
        .expect("funded");
        let _ = reg.take_pending();
        corp
    }

    fn corp_isk(reg: &Registry, corp: i64) -> f64 {
        crate::service::corp_wallet::corp_balance(reg, corp, crate::corp_wallet::MASTER_KEY)
    }

    /// `PlaceBuyOrder` with the `useCorp` box ticked.
    fn corp_buy(
        reg: &mut Registry,
        who: i64,
        price: f64,
        qty: i64,
        days: i64,
    ) -> Result<PyValue, ServiceError> {
        as_(
            reg,
            who,
            "PlaceBuyOrder",
            &[
                PyValue::Int(STATION),
                PyValue::Int(TRITANIUM),
                PyValue::Real(price),
                PyValue::Int(qty),
                PyValue::Int(RANGE_STATION),
                PyValue::Int(1),
                PyValue::Int(days),
                PyValue::Bool(true),
                PyValue::Real(0.0),
            ],
        )
    }

    fn delivered(reg: &Registry, corp: i64) -> i64 {
        reg.store()
            .at(crate::item::Slot::new(
                corp,
                STATION,
                crate::corp_office::FLAG_CORP_DELIVERIES,
            ))
            .filter(|i| i.type_id == TRITANIUM)
            .map(|i| i.quantity)
            .sum()
    }

    /// The message key of a raised `UserError`.
    fn key_of(e: &ServiceError) -> String {
        let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
            return format!("{e:?}");
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("an exception header is (class, args)")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's args are a tuple")
        };
        match &args[0] {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("the key is a str: {other:?}"),
        }
    }

    fn denied(e: &ServiceError) -> bool {
        key_of(e) == "CrpAccessDenied"
    }

    // ---- CorpGetTransactions ---------------------------------------------------

    /// `CorpGetTransactions(None, key)` as `who`.
    fn corp_tx(reg: &mut Registry, who: i64, key: PyValue) -> Vec<DbRow> {
        rows(&as_(reg, who, "CorpGetTransactions", &[PyValue::None, key]).unwrap())
    }

    /// The client's own buy test (`marketSvc._GetCorpTransactionKeyVal`).
    fn reads_as_buy(row: &DbRow) -> bool {
        col(row, "accountID") == col(row, "buyerAccountID")
    }

    /// A trade the corporation made through `key`, as `buyer` or as seller.
    fn corp_trade(reg: &mut Registry, corp: i64, key: i64, as_buyer: bool) {
        let (bc, bk, sc, sk) = if as_buyer {
            (corp, key, 0, 0)
        } else {
            (0, 0, corp, key)
        };
        reg.market.record_trade(Trade {
            date: crate::pyutil::filetime_now(),
            type_id: TRITANIUM,
            quantity: 4,
            price: 2.5,
            station_id: STATION,
            buyer_id: if as_buyer { ALICE } else { BOB },
            seller_id: if as_buyer { BOB } else { ALICE },
            region_id: REGION,
            buyer_corp: bc,
            buyer_key: bk,
            seller_corp: sc,
            seller_key: sk,
            ..Default::default()
        });
    }

    #[test]
    fn a_corp_buy_lists_under_its_division() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let item = stack(&mut reg, BOB, 50);
        sell(&mut reg, BOB, item, 10.0, 50, 7).unwrap();
        corp_buy(&mut reg, ALICE, 10.0, 30, 0).unwrap();
        let mine = corp_tx(&mut reg, ALICE, PyValue::None);
        assert_eq!(mine.len(), 1);
        let r = &mine[0];
        assert!(reads_as_buy(r));
        assert_eq!(col(r, "accountID"), DbValue::Int(1000));
        assert_eq!(col(r, "buyerAccountID"), DbValue::Int(1000));
        assert_eq!(col(r, "sellerAccountID"), DbValue::Int(0));
        assert_eq!(col(r, "keyID"), DbValue::Int(1000));
        assert_eq!(
            col(r, "buyerID"),
            DbValue::Int(ALICE),
            "the pilot, not {corp}"
        );
        assert_eq!(col(r, "sellerID"), DbValue::Int(BOB));
        assert_eq!(col(r, "quantity"), DbValue::Int(30));
        assert_eq!(col(r, "price"), DbValue::Int(100_000));
        // The pilot's own list is the same trade, from the pilot's side, but
        // it is the corporation's: the buyer account is 0, so `isForCorp`.
        let own = rows(&as_(&mut reg, ALICE, "CharGetTransactions", &[PyValue::None]).unwrap());
        assert_eq!(own.len(), 1);
        assert!(own[0].descriptor().index_of(b"keyID").is_none());
        assert_ne!(col(&own[0], "accountID"), col(&own[0], "buyerAccountID"));
        assert_eq!(col(&own[0], "buyerAccountID"), DbValue::Int(0));
    }

    #[test]
    fn a_corp_sale_row_carries_its_division_as_key_id() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        corp_trade(&mut reg, corp, 1002, false);
        let r = corp_tx(&mut reg, ALICE, PyValue::None);
        assert_eq!(r.len(), 1);
        assert!(!reads_as_buy(&r[0]));
        assert_eq!(col(&r[0], "keyID"), DbValue::Int(1002));
        // The seller's own list: the sale went through the corporation, so
        // it is `isForCorp`; the buyer's side (a pilot) is not.
        let own = rows(&as_(&mut reg, ALICE, "CharGetTransactions", &[PyValue::None]).unwrap());
        assert_eq!(col(&own[0], "sellerAccountID"), DbValue::Int(0));
        assert_eq!(col(&own[0], "buyerAccountID"), DbValue::Int(1000));
    }

    #[test]
    fn the_any_key_lists_all_divisions() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        corp_trade(&mut reg, corp, 1000, true);
        corp_trade(&mut reg, corp, 1002, true);
        assert_eq!(corp_tx(&mut reg, ALICE, PyValue::None).len(), 2);
        let one = corp_tx(&mut reg, ALICE, PyValue::Int(1002));
        assert_eq!(one.len(), 1);
        assert_eq!(col(&one[0], "accountID"), DbValue::Int(1002));
        assert!(corp_tx(&mut reg, ALICE, PyValue::Int(1001)).is_empty());
        assert!(
            corp_tx(&mut reg, ALICE, PyValue::Int(5)).is_empty(),
            "not a division"
        );
        // `fromDate` drops the older rows.
        let later = crate::pyutil::filetime_now() + DAY_TICKS;
        let v = as_(
            &mut reg,
            ALICE,
            "CorpGetTransactions",
            &[PyValue::Int(later), PyValue::None],
        )
        .unwrap();
        assert!(rows(&v).is_empty());
    }

    /// The trap: the master division is 1000, the number a pilot's cash side
    /// would carry too, so a corp sale to a pilot must not read as a buy.
    #[test]
    fn a_corp_sale_is_not_read_as_a_buy() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        corp_trade(&mut reg, corp, 1000, false);
        let mine = corp_tx(&mut reg, ALICE, PyValue::None);
        assert_eq!(mine.len(), 1);
        let r = &mine[0];
        assert!(!reads_as_buy(r));
        assert_eq!(col(r, "accountID"), DbValue::Int(1000));
        assert_eq!(col(r, "sellerAccountID"), DbValue::Int(1000));
        assert_eq!(col(r, "buyerAccountID"), DbValue::Int(0));
        assert_eq!(col(r, "buyerID"), DbValue::Int(BOB));
        assert_eq!(col(r, "sellerID"), DbValue::Int(ALICE));
        // Two divisions of one corporation trading with each other: one row
        // per side, one a buy and one a sale, both under their own key.
        reg.market.record_trade(Trade {
            date: crate::pyutil::filetime_now(),
            type_id: TRITANIUM,
            quantity: 1,
            price: 1.0,
            station_id: STATION,
            buyer_id: ALICE,
            seller_id: ALICE,
            region_id: REGION,
            buyer_corp: corp,
            buyer_key: 1001,
            seller_corp: corp,
            seller_key: 1001,
            ..Default::default()
        });
        let both = corp_tx(&mut reg, ALICE, PyValue::Int(1001));
        assert_eq!(both.len(), 2);
        assert_eq!(both.iter().filter(|r| reads_as_buy(r)).count(), 1);
    }

    #[test]
    fn an_old_trade_line_still_parses() {
        let f = ["mkttrade", "3", "99", "34", "2", "1.5", "50", "7", "8"];
        let t = parse_trade(&f).unwrap();
        assert_eq!(
            (t.buyer_corp, t.buyer_key, t.seller_corp, t.seller_key),
            (0, 0, 0, 0)
        );
        // One from before the corporation fields but after CP42's region.
        let f = ["mkttrade", "3", "99", "34", "2", "1.5", "50", "7", "8", "9"];
        assert_eq!(parse_trade(&f).unwrap().region_id, 9);
        assert_eq!(parse_trade(&f).unwrap().seller_key, 0);
    }

    #[test]
    fn no_role_no_rows() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        corp_trade(&mut reg, corp, 1000, true);
        // Bob is a member with no roles and sees nothing. (A pilot outside
        // any corporation, or in another one, gets nothing by the same
        // `standing_of` / `t.*_corp == corp` checks; no test here builds one.)
        assert!(corp_tx(&mut reg, BOB, PyValue::None).is_empty());
        assert!(corp_tx(&mut reg, BOB, PyValue::Int(1000)).is_empty());
        // A trade through a pilot's own wallet is not the corporation's.
        reg.market.record_trade(Trade {
            date: crate::pyutil::filetime_now(),
            type_id: TRITANIUM,
            quantity: 1,
            price: 1.0,
            station_id: STATION,
            buyer_id: ALICE,
            seller_id: BOB,
            region_id: REGION,
            ..Default::default()
        });
        assert_eq!(corp_tx(&mut reg, ALICE, PyValue::None).len(), 1);
        // A junior accountant reads every division; a bare trader may not.
        reg.corporations.member_mut(BOB).unwrap().roles =
            crate::corp_wallet::ROLE_JUNIOR_ACCOUNTANT;
        assert_eq!(corp_tx(&mut reg, BOB, PyValue::None).len(), 1);
        reg.corporations.member_mut(BOB).unwrap().roles = TRADER;
        assert!(corp_tx(&mut reg, BOB, PyValue::None).is_empty());
    }

    #[test]
    fn a_corp_bid_escrows_from_the_division_and_cancelling_refunds_it() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let pilot = balance(&reg, ALICE);
        corp_buy(&mut reg, ALICE, 10.0, 100, 7).unwrap();
        assert_eq!(balance(&reg, ALICE), pilot, "the pilot paid nothing");
        let held = corp_isk(&reg, corp);
        assert!(
            held <= FLOAT - 1_000.0,
            "the escrow left the division: {held}"
        );
        assert!(rows(&as_(&mut reg, ALICE, "GetCharOrders", &[]).unwrap()).is_empty());
        let theirs = rows(&as_(&mut reg, ALICE, "GetCorporationOrders", &[]).unwrap());
        assert_eq!(theirs.len(), 1);
        assert_eq!(col(&theirs[0], "keyID"), DbValue::Int(1000));
        assert_eq!(col(&theirs[0], "isCorp"), DbValue::Bool(true));
        assert_eq!(col(&theirs[0], "charID"), DbValue::Int(ALICE));
        // The corp journal records the escrow and the broker's fee.
        let refs: Vec<i64> = crate::corp_wallet::all_rows(reg.deltas(), corp)
            .iter()
            .map(|r| r.ref_type)
            .collect();
        assert!(refs.contains(&REF_MARKET_ESCROW), "{refs:?}");
        let DbValue::Int(order_id) = col(&theirs[0], "orderID") else {
            panic!("orderID is an integer");
        };
        as_(
            &mut reg,
            ALICE,
            "CancelCharOrder",
            &[PyValue::Int(order_id), PyValue::Int(REGION)],
        )
        .unwrap();
        assert_eq!(
            corp_isk(&reg, corp),
            held + 1_000.0,
            "the escrow came back to the division"
        );
        assert_eq!(balance(&reg, ALICE), pilot);
        assert!(rows(&as_(&mut reg, ALICE, "GetCorporationOrders", &[]).unwrap()).is_empty());
    }

    #[test]
    fn a_seller_filling_a_corp_bid_delivers_to_the_corps_deliveries_hangar() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        corp_buy(&mut reg, ALICE, 10.0, 100, 7).unwrap();
        let before = corp_isk(&reg, corp);
        let bob = balance(&reg, BOB);
        let item = stack(&mut reg, BOB, 40);
        sell(&mut reg, BOB, item, 10.0, 40, 0).unwrap();
        assert_eq!(delivered(&reg, corp), 40);
        assert_eq!(units(&reg, ALICE), 0, "not the pilot's hangar");
        assert!(balance(&reg, BOB) > bob, "the seller was paid");
        assert_eq!(
            corp_isk(&reg, corp),
            before,
            "the escrow paid it, not the wallet"
        );
        let open = rows(&as_(&mut reg, ALICE, "GetCorporationOrders", &[]).unwrap());
        assert_eq!(col(&open[0], "volRemaining"), DbValue::Real(60.0));
    }

    #[test]
    fn an_immediate_corp_buy_pays_from_the_division_into_deliveries() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let item = stack(&mut reg, BOB, 50);
        sell(&mut reg, BOB, item, 10.0, 50, 7).unwrap();
        let pilot = balance(&reg, ALICE);
        corp_buy(&mut reg, ALICE, 10.0, 30, 0).unwrap();
        assert_eq!(delivered(&reg, corp), 30);
        assert_eq!(corp_isk(&reg, corp), FLOAT - 300.0);
        assert_eq!(balance(&reg, ALICE), pilot);
        let tagged: Vec<_> = crate::corp_wallet::all_rows(reg.deltas(), corp)
            .into_iter()
            .filter(|r| r.ref_type == REF_MARKET_TRANSACTION)
            .collect();
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].owner1, corp);
        assert_eq!(tagged[0].owner2, BOB);
    }

    #[test]
    fn a_member_without_the_role_or_the_corp_is_refused_and_nothing_moves() {
        let mut reg = market();
        // Bob is in no player corporation yet.
        let e = corp_buy(&mut reg, BOB, 10.0, 10, 7).unwrap_err();
        assert!(denied(&e), "{e:?}");
        let corp = corp_world(&mut reg, 0);
        let (pilot, wallet) = (balance(&reg, BOB), corp_isk(&reg, corp));
        let e = corp_buy(&mut reg, BOB, 10.0, 10, 7).unwrap_err();
        assert!(denied(&e), "{e:?}");
        assert_eq!((balance(&reg, BOB), corp_isk(&reg, corp)), (pilot, wallet));
        assert!(reg.market.open().next().is_none(), "no order exists");
        // A trader may place one; the Corporation Orders tab is the accountant's.
        reg.corporations.member_mut(BOB).unwrap().roles = TRADER;
        corp_buy(&mut reg, BOB, 10.0, 10, 7).unwrap();
        assert!(corp_isk(&reg, corp) < wallet);
        let seen = as_(&mut reg, BOB, "GetCorporationOrders", &[]).unwrap();
        assert!(matches!(seen, PyValue::List(ref v) if v.is_empty()));
    }

    /// REVIEW-FIX: a trader who loses the role (or the corporation) keeps no
    /// reach: his open corp bid is cancelled with its escrow back in the
    /// division, and the hand-built modify is refused.
    #[test]
    fn a_corp_order_dies_with_its_issuers_role() {
        let mut reg = market();
        let corp = corp_world(&mut reg, TRADER);
        corp_buy(&mut reg, BOB, 10.0, 100, 7).unwrap();
        let held = corp_isk(&reg, corp);
        let id = reg.market.open().next().expect("his bid").order_id;
        // Still a trader: he may modify it.
        let modify = |reg: &mut Registry| {
            as_(
                reg,
                BOB,
                "ModifyCharOrder",
                &[
                    PyValue::Int(id),
                    PyValue::Real(9.0),
                    PyValue::Int(100),
                    PyValue::Int(STATION),
                    PyValue::Bool(true),
                    PyValue::Int(TRITANIUM),
                    PyValue::Int(RANGE_STATION),
                    PyValue::Int(1),
                    PyValue::Int(7),
                    PyValue::Bool(true),
                ],
            )
        };
        let _ = modify(&mut reg);
        // The role is revoked: the next market call closes the order.
        reg.corporations.member_mut(BOB).unwrap().roles = 0;
        let seen = rows(&as_(&mut reg, ALICE, "GetCorporationOrders", &[]).unwrap());
        assert!(seen.is_empty(), "the order is gone");
        assert!(reg.market.open().next().is_none());
        assert!(
            corp_isk(&reg, corp) > held,
            "the escrow went back to the division"
        );
        assert!(modify(&mut reg).is_err(), "no reach left");
    }

    #[test]
    fn a_division_that_cannot_cover_the_escrow_refuses_with_not_enough_money_corp() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let e = corp_buy(&mut reg, ALICE, 1_000_000.0, 100, 7).unwrap_err();
        assert_eq!(key_of(&e), "NotEnoughMoneyCorp");
        assert_eq!(corp_isk(&reg, corp), FLOAT);
        assert!(reg.market.open().next().is_none());
    }

    #[test]
    fn a_corp_sell_order_takes_its_proceeds_and_its_fees_on_the_division() {
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let bid = corp_buy(&mut reg, ALICE, 10.0, 10, 7);
        assert!(bid.is_ok());
        // Bob's own bid, then Alice sells into it for the corporation.
        buy(&mut reg, BOB, 20.0, 10, 7).unwrap();
        let before = corp_isk(&reg, corp);
        let pilot = balance(&reg, ALICE);
        let item = stack(&mut reg, ALICE, 10);
        as_(
            &mut reg,
            ALICE,
            "PlaceMultiSellOrder",
            &[
                PyValue::List(vec![sell_line(item, 20.0, 10)]),
                PyValue::Bool(true),
                PyValue::Int(0),
                PyValue::Real(0.0),
            ],
        )
        .unwrap();
        assert!(corp_isk(&reg, corp) > before, "the sale paid the division");
        assert_eq!(balance(&reg, ALICE), pilot, "not the pilot");
        assert_eq!(units(&reg, BOB), 10);
    }

    #[test]
    fn a_corp_order_survives_the_world_file() {
        let mut reg = market();
        corp_world(&mut reg, 0);
        corp_buy(&mut reg, ALICE, 10.0, 10, 7).unwrap();
        let o = reg.market.open().next().cloned().unwrap();
        assert_ne!(o.corp_id, 0);
        let line = order_line(&o);
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        assert_eq!(parse_order(&f).unwrap(), o);
        // A line written before CORP-BUY has no such fields: a pilot's order.
        let old = f[..18].to_vec();
        let p = parse_order(&old).unwrap();
        assert_eq!((p.corp_id, p.key_id), (0, 0));
    }

    // ---- DEV-MODE: the dev seller ------------------------------------------

    /// A market with one station and one material type that no NPC asks
    /// (no trade list), `dev` or not.
    fn dev_market(dev: bool) -> Registry {
        let row = exchange::TypeRow {
            group_id: 18,
            category_id: 4,
            base_price: 2.0,
            meta_group_id: None,
            tech_level: None,
        };
        let types = std::collections::BTreeMap::from([(TRITANIUM, row)]);
        let station = exchange::Station {
            station_id: STATION,
            system_id: SYSTEM,
            region_id: REGION,
            owner_id: 1_999_001,
        };
        let table = exchange::Table::new(vec![station], &[], types, &[]).expect("builds");
        let mut c = crate::content::Content::fixture();
        c.insert("econ_exchange", table);
        let mut config = SandboxConfig {
            dev,
            ..SandboxConfig::default()
        };
        config.characters = vec![Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            station_id: Some(STATION),
            solar_system_id: Some(SYSTEM),
            region_id: REGION,
            balance: 1_000_000.0,
            ..Character::default()
        }];
        Registry::new(config).with_content(std::sync::Arc::new(c))
    }

    fn asks_of(reg: &mut Registry) -> Vec<DbRow> {
        book(reg, ALICE).0
    }

    /// **DEV-MODE**: an unseeded type at the station shows a dev ask of 1000
    /// at 100 ISK; buying takes it into the hangar through the ordinary
    /// path (wallet, journal), the stock is 1000 again, and the price
    /// history and trades know nothing of it.
    #[test]
    fn a_dev_buy_delivers_at_100_and_restocks() {
        let mut reg = dev_market(true);
        let asks = asks_of(&mut reg);
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert_eq!(
            col(&asks[0], "price"),
            DbValue::Int(1_000_000),
            "100 ISK x 10,000"
        );
        assert_eq!(
            col(&asks[0], "volRemaining"),
            DbValue::Real(exchange::DEV_STOCK as f64)
        );
        assert!(exchange::is_dev(match col(&asks[0], "orderID") {
            DbValue::Int(i) => i,
            other => panic!("{other:?}"),
        }));
        let before = balance(&reg, ALICE);
        buy(&mut reg, ALICE, exchange::DEV_PRICE, 300, 0).expect("fills from the dev seller");
        assert_eq!(units(&reg, ALICE), 300);
        assert_eq!(
            balance(&reg, ALICE),
            cents(before - 300.0 * exchange::DEV_PRICE)
        );
        assert!(
            !journal_rows(&reg, ALICE, REF_MARKET_TRANSACTION).is_empty(),
            "the wallet journal has the purchase"
        );
        let asks = asks_of(&mut reg);
        assert_eq!(
            col(&asks[0], "volRemaining"),
            DbValue::Real(exchange::DEV_STOCK as f64)
        );
        assert!(reg.market.trades.is_empty(), "no trade, so no price index");
        // And again, past one lot's worth: it is full every time.
        buy(&mut reg, ALICE, exchange::DEV_PRICE, 1_000, 0).expect("and again");
        assert_eq!(units(&reg, ALICE), 1_300);
    }

    /// A server that is not `--dev` shows no dev seller, whatever its tables.
    #[test]
    fn a_normal_server_shows_no_dev_seller() {
        let mut reg = dev_market(false);
        assert!(asks_of(&mut reg).is_empty());
        assert!(buy(&mut reg, ALICE, exchange::DEV_PRICE, 10, 0).is_err());
        assert_eq!(units(&reg, ALICE), 0);
    }

    /// The region best-ask lookup (the bots' and the window's) lists the
    /// dev ask beside a real player's cheaper order.
    #[test]
    fn a_players_order_still_shows_beside_the_dev_ask() {
        let mut reg = dev_market(true);
        let item = stack(&mut reg, ALICE, 50);
        sell(&mut reg, ALICE, item, 60.0, 50, 7).expect("a player ask");
        let asks = asks_of(&mut reg);
        let prices: Vec<DbValue> = asks.iter().map(|r| col(r, "price")).collect();
        assert_eq!(prices, vec![DbValue::Int(600_000), DbValue::Int(1_000_000)]);
    }

    /// **FITTING-MARKET**: the fitting window's multibuy buys from the dev
    /// seller through `BuyMultipleItems`, and the order comes back as a row
    /// the window can read (`.typeID`, `.orderID`, marketsvc.py:318).
    #[test]
    fn a_multibuy_buys_from_the_dev_seller() {
        let mut reg = dev_market(true);
        let line = keyval(vec![
            ("stationID", PyValue::Int(STATION)),
            ("typeID", PyValue::Int(TRITANIUM)),
            ("price", PyValue::Real(exchange::DEV_PRICE)),
            ("quantity", PyValue::Int(250)),
            ("minVolume", PyValue::Int(1)),
            ("delta", PyValue::Real(0.0)),
        ]);
        let before = balance(&reg, ALICE);
        let created = as_(
            &mut reg,
            ALICE,
            "BuyMultipleItems",
            &[
                PyValue::Int(STATION),
                PyValue::List(vec![line]),
                PyValue::Bool(false),
            ],
        )
        .expect("the multibuy is answered");
        let PyValue::List(created) = created else {
            panic!("BuyMultipleItems is iterated");
        };
        assert_eq!(created.len(), 1);
        assert_eq!(units(&reg, ALICE), 250);
        assert_eq!(
            balance(&reg, ALICE),
            cents(before - 250.0 * exchange::DEV_PRICE)
        );
    }

    /// **FITTING-MARKET**: `GetRegionBest` carries what the client reads
    /// (`price`, `volRemaining` and, for the jumps, `stationID`:
    /// marketbase.py `FilterItemsForGroupPage`); and 12,000 types of them
    /// still compress to a small fraction.
    #[test]
    fn region_best_carries_what_the_browse_tree_reads_and_compresses() {
        let orders: Vec<Order> = (0..12_000i64)
            .map(|i| Order {
                order_id: exchange::DEV_ORDER_BASE + i,
                char_id: 0,
                type_id: 1_000 + i,
                region_id: REGION,
                solar_system_id: SYSTEM,
                station_id: STATION,
                price: 100.0,
                vol_entered: 1_000,
                vol_remaining: 1_000,
                min_volume: 1,
                bid: false,
                issue_date: 0,
                duration: 0,
                range: RANGE_STATION,
                escrow: 0.0,
                state: ORDER_STATE_OPEN,
                last_state_change: None,
                corp_id: 0,
                key_id: 0,
            })
            .collect();
        let v = ask_keyvals(orders.iter());
        let PyValue::Dict(entries) = &v else {
            panic!("a dict");
        };
        let (PyValue::Instance { state, .. }, _) = &entries[0] else {
            panic!("a KeyVal");
        };
        assert!(dict_get(state, b"price").is_some());
        assert!(dict_get(state, b"volRemaining").is_some());
        assert_eq!(
            dict_get(state, b"stationID").cloned(),
            Some(PyValue::Int(STATION))
        );
        let plain = crate::wire::dumps(&v);
        let packed = crate::push::compress_payload(&plain);
        assert!(
            packed.len() * 10 < plain.len(),
            "{} of {}",
            packed.len(),
            plain.len()
        );
        assert_eq!(eve_inflate::inflate_zlib(&packed).expect("inflates"), plain);
        crate::errln!(
            "GetRegionBest 12000 types: plain {} compressed {}",
            plain.len(),
            packed.len()
        );
    }

    /// **FITTING-MARKET**: corporation fittings. Any member reads them; the
    /// CEO and a fitting manager write them; a plain member does not.
    #[test]
    fn corp_fittings_need_the_fitting_manager_role() {
        use crate::fitting_store as fs;
        let mut reg = market();
        let corp = corp_world(&mut reg, 0);
        let fit = keyval(vec![
            ("shipTypeID", PyValue::Int(9_001)),
            ("name", PyValue::Unicode("corp fit".into())),
            ("description", PyValue::Unicode(String::new())),
            (
                "fitData",
                PyValue::List(vec![PyValue::Tuple(vec![
                    PyValue::Int(9_002),
                    PyValue::Int(11),
                    PyValue::Int(1),
                ])]),
            ),
        ]);
        // Bob is a member with no roles: he may not save.
        assert!(fs::save(&mut reg, BOB, corp, &fit, 0).is_err());
        // Alice founded it, so she is the CEO.
        let id = fs::save(&mut reg, ALICE, corp, &fit, 0).expect("the CEO saves");
        // Bob reads it, but cannot delete it.
        assert_eq!(fs::fittings(&reg, corp).len(), 1);
        assert!(!fs::delete(&mut reg, BOB, corp, id));
        // With the role he can.
        reg.corporations.member_mut(BOB).unwrap().roles |= fs::CORP_ROLE_FITTING_MANAGER;
        assert!(fs::delete(&mut reg, BOB, corp, id));
    }
}
