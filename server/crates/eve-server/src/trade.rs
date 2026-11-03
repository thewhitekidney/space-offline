//! **Direct trade between two docked pilots** (lane GAP-TRADE, spec
//! `handback/evejs-gap-svcD/report.md` §3.2). The wire is
//! `service/trade.rs`; this file is the model and the rules.
//!
//! The client's trade window (`pvptradewnd.py`) talks to a **bound object**,
//! `tradeSession`, which the server hands to the initiator as the answer to
//! `trademgr.InitiateTrade` and to the other pilot inside the
//! `OnTradeInitiate` push. One object, two connections, so it is minted by
//! the world (`Registry::mint_world_object`) and resolved by
//! `dispatch.rs` when the id is on no connection's own list.
//!
//! - **The container.** Each trade owns an escrow location
//!   (`item::escrow_location(TICKET_BASE + n)`). An offered item moves there
//!   still owned by its offerer, which is how the client tells the two sides
//!   apart (`invControllers.PlayerTrade.IsItemHere`: owner and location).
//! - **Staging persists.** Delta kind [`KIND`]:
//!   `item_id -> source_location source_flag owner container`. Written when
//!   an item enters a container, deleted when it leaves. [`recover`] runs on
//!   a load and sends every staged item home, so a restart never strands one.
//! - **Atomic completion.** Both accept (an accept is reset by *any* change:
//!   an item added, money offered), the balances are re-checked, every item
//!   moves in **one** `apply_items` plan (all or none), and only then does
//!   the ISK move. A failure before the plan moves nothing at all.
//! - **It ends** on a cancel (`Abort`, the window closing), a disconnect, an
//!   undock, or a pilot no longer docked where the trade is, and everything
//!   goes back to where it came from.
//!
//! Not persisted: the trade itself (two pilots mid-conversation do not need
//! to survive a restart; their items do).

use crate::item::{Change, Edit, Slot, escrow_location};
use crate::model::FLAG_HANGAR;
use crate::notify::{Addressed, ItemChange, Push};
use crate::service::Registry;
use eve_marshal::{DbValue, PyValue};
use std::collections::BTreeSet;
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "tradestage";

/// `appConst.refPlayerTrading` **[V]** (`cl:appConst.py:643`).
pub const REF_PLAYER_TRADING: i64 = 1;

/// The bound-object name the session's methods live under.
pub const BOUND: &str = "trademgr::session";

/// Trade tickets inside `item::escrow_location`'s one band. **[A]**: any
/// constant no other subsystem picks; contracts use their id (below 10^12)
/// and the market 10^12 + order.
pub const TICKET_BASE: i64 = 2_000_000_000_000;

/// Most item rows one trade holds. **[A]**: the client's real limit is behind
/// `CantTradeThisManyItems`; falsified by a retail trade that holds more.
pub const MAX_ITEMS: usize = 100;

/// More world objects than this and the finished ones are forgotten.
const WORLD_OBJECT_KEEP: usize = 256;

/// Why a trade call was refused. The wire turns each into its message key.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// A bare client message key (`CantTradeDifferentLocation`, ...).
    Key(&'static str),
    /// `NotEnoughMoney`.
    NotEnoughMoney { amount: f64, balance: f64 },
    /// Our words (`CustomNotify`).
    Words(&'static str),
}

/// One open trade.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub container_id: i64,
    pub station_id: i64,
    /// The initiator first.
    pub traders: [i64; 2],
    pub money: [f64; 2],
    pub accepted: [bool; 2],
    /// The world object both pilots call.
    pub object_id: String,
}

impl Trade {
    fn index_of(&self, who: i64) -> Option<usize> {
        self.traders.iter().position(|t| *t == who)
    }

    /// The other pilot.
    pub fn other(&self, who: i64) -> Option<i64> {
        self.index_of(who).map(|i| self.traders[1 - i])
    }
}

/// Every open trade.
#[derive(Debug, Clone, Default)]
pub struct Trades {
    open: Vec<Trade>,
    next: i64,
}

impl Trades {
    /// The trade this pilot is in.
    pub fn of(&self, who: i64) -> Option<&Trade> {
        self.open.iter().find(|t| t.traders.contains(&who))
    }

    fn of_mut(&mut self, who: i64) -> Option<&mut Trade> {
        self.open.iter_mut().find(|t| t.traders.contains(&who))
    }

    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    fn remove(&mut self, container_id: i64) -> Option<Trade> {
        let at = self
            .open
            .iter()
            .position(|t| t.container_id == container_id)?;
        Some(self.open.remove(at))
    }
}

/// A trade event for the pilots it names (`OnTradeCancel`, `OnTradeOffer`,
/// ...). The handler names are the client's own method names.
#[derive(Debug, Clone)]
pub struct TradeNote {
    pub to: Vec<i64>,
    pub event: &'static str,
    pub args: Vec<PyValue>,
}

impl Push for TradeNote {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (self.event, self.args.clone())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// Queue a note for the next turn, or (`scheduled`) for the clock, which is
/// what a disconnect needs: it is not a payload, so nothing drains `pending`.
fn note(reg: &mut Registry, scheduled: bool, n: TradeNote) {
    if scheduled {
        reg.schedule(Duration::ZERO, n);
    } else {
        reg.push(n);
    }
}

fn states(accepted: [bool; 2]) -> PyValue {
    PyValue::List(
        accepted
            .iter()
            .map(|a| PyValue::Int(i64::from(*a)))
            .collect(),
    )
}

fn moneys(money: [f64; 2]) -> PyValue {
    PyValue::List(money.iter().map(|m| PyValue::Real(*m)).collect())
}

fn tell_offer(reg: &mut Registry, t: &Trade, scheduled: bool) {
    note(
        reg,
        scheduled,
        TradeNote {
            to: t.traders.to_vec(),
            event: "OnTradeOffer",
            args: vec![PyValue::Int(t.container_id), states(t.accepted)],
        },
    );
}

fn stage_key(item_id: i64) -> Vec<String> {
    vec![item_id.to_string()]
}

/// A staged item's way home: `(location, flag, owner, container)`.
fn staged(reg: &Registry, item_id: i64) -> Option<(i64, i64, i64, i64)> {
    let v = reg.deltas().of(KIND).get(&stage_key(item_id))?;
    let n = |i: usize| v.get(i).and_then(|s| s.parse::<i64>().ok());
    Some((n(0)?, n(1)?, n(2)?, n(3)?))
}

fn stage(reg: &mut Registry, item_id: i64, from: Slot, container: i64) {
    reg.deltas_mut().put(
        KIND,
        stage_key(item_id),
        vec![
            from.location_id.to_string(),
            from.flag_id.to_string(),
            from.owner_id.to_string(),
            container.to_string(),
        ],
    );
}

fn unstage(reg: &mut Registry, item_id: i64) {
    reg.deltas_mut().del(KIND, &stage_key(item_id));
}

/// The ids of the items now in a trade's container, sorted.
fn staged_ids(reg: &Registry, container: i64) -> Vec<i64> {
    let mut ids: Vec<i64> = reg
        .store()
        .contents_of(container)
        .map(|i| i.item_id)
        .collect();
    ids.sort_unstable();
    ids
}

/// The container's item rows, for `List()`.
pub fn item_rows(reg: &Registry, container: i64) -> Vec<PyValue> {
    staged_ids(reg, container)
        .into_iter()
        .filter_map(|id| reg.store().get(id))
        .map(|i| reg.item_row_value(i))
        .collect()
}

/// Is the pilot in the trade's station, not in space?
fn docked_at(reg: &Registry, who: i64, station: i64) -> bool {
    reg.docked_station(who) == Some(station)
}

/// The trade `who` is in, after checking both pilots are still docked in its
/// station. A trade whose pilots are not cancelled here, and the caller is
/// told why.
pub fn live(reg: &mut Registry, who: i64) -> Result<Trade, Refusal> {
    let Some(t) = reg.trades.of(who).cloned() else {
        return Err(Refusal::Words("You are not in a trade."));
    };
    if t.traders.iter().all(|p| docked_at(reg, *p, t.station_id)) {
        Ok(t)
    } else {
        cancel(reg, t.container_id, false);
        Err(Refusal::Key("CantTradeDifferentLocation"))
    }
}

/// Open a trade between `a` (the caller) and `b`. Returns the reference to
/// the session object, and queues `OnTradeInitiate` for `b`.
pub fn initiate(reg: &mut Registry, a: i64, b: i64) -> Result<PyValue, Refusal> {
    if a == b {
        return Err(Refusal::Words("You cannot trade with yourself."));
    }
    let (Some(station), Some(other)) = (reg.docked_station(a), reg.docked_station(b)) else {
        return Err(Refusal::Key("CantTradeDifferentLocation"));
    };
    if station != other || !reg.sessions().is_online(b) {
        return Err(Refusal::Key("CantTradeDifferentLocation"));
    }
    if reg.trades.of(a).is_some() || reg.trades.of(b).is_some() {
        return Err(Refusal::Words(
            "One of you is already in a trade. Finish or cancel it first.",
        ));
    }
    reg.trades.next += 1;
    let container_id = escrow_location(TICKET_BASE + reg.trades.next);
    let (object_id, reference) =
        reg.mint_world_object(BOUND, PyValue::Tuple(vec![PyValue::Int(container_id)]));
    reg.trades.open.push(Trade {
        container_id,
        station_id: station,
        traders: [a, b],
        money: [0.0; 2],
        accepted: [false; 2],
        object_id,
    });
    let live_ids: BTreeSet<String> = reg
        .trades
        .open
        .iter()
        .map(|t| t.object_id.clone())
        .collect();
    reg.prune_world_objects(WORLD_OBJECT_KEEP, |id| live_ids.contains(id));
    reg.push(TradeNote {
        to: vec![b],
        event: "OnTradeInitiate",
        args: vec![PyValue::Int(a), reference.clone()],
    });
    Ok(reference)
}

/// Offer items from the caller's station hangar (`qty` of the one item, or
/// whole stacks). All or nothing; any change resets both accepts.
pub fn add(
    reg: &mut Registry,
    who: i64,
    item_ids: &[i64],
    qty: Option<i64>,
) -> Result<(), Refusal> {
    let t = live(reg, who)?;
    let other = t.other(who).unwrap_or(who);
    let ship = reg.character(who).map(|c| c.ship_id);
    let inside = reg.store().contents_of(t.container_id).count();
    let mut seen = BTreeSet::new();
    let mut edits = Vec::new();
    let mut sources = Vec::new();
    for &id in item_ids {
        if !seen.insert(id) {
            continue;
        }
        let Some(item) = reg.store().get(id) else {
            return Err(Refusal::Key("CantTradeDifferentLocation"));
        };
        if Some(id) == ship {
            return Err(Refusal::Key("ConCannotTradeCurrentShip"));
        }
        if item.owner_id != who {
            return Err(Refusal::Key("CantTradeFromCorpHangar"));
        }
        if item.location_id != t.station_id || item.flag_id != FLAG_HANGAR {
            return Err(Refusal::Key("CantTradeDifferentLocation"));
        }
        let to = Slot::new(who, t.container_id, FLAG_HANGAR);
        match qty {
            Some(q) if q < item.quantity && !item.singleton => {
                if q <= 0 {
                    return Err(Refusal::Words("That is not a quantity you can trade."));
                }
                edits.push(Edit::Split {
                    item_id: id,
                    quantity: q,
                    to,
                });
            }
            _ => edits.push(Edit::Move { item_id: id, to }),
        }
        sources.push(item.slot());
    }
    if edits.is_empty() {
        return Ok(());
    }
    if inside + edits.len() > MAX_ITEMS {
        return Err(Refusal::Key("CantTradeThisManyItems"));
    }
    let changes = reg
        .apply_items(&edits)
        .map_err(|_| Refusal::Words("Those items cannot be traded."))?;
    // A split's new stack is `Created`, a move is `Moved`; both go home to
    // the slot their original sat in. Edits and sources are in step, and
    // `apply` reports changes in edit order.
    let mut staged_now = Vec::new();
    let mut from = sources.iter();
    for c in &changes {
        match c {
            Change::Moved { item_id, .. } | Change::Created { item_id, .. } => {
                if let Some(src) = from.next() {
                    stage(reg, *item_id, *src, t.container_id);
                    staged_now.push(*item_id);
                }
            }
            _ => {}
        }
    }
    // The other pilot is not an owner of what moved, so `apply_items` did not
    // tell them: send the new rows so their window lists them.
    for id in &staged_now {
        if let Some(item) = reg.store().get(*id).cloned() {
            let row = reg.item_row_value(&item);
            reg.push(Addressed {
                to: Some(vec![other]),
                inner: Box::new(ItemChange::spawned(row)),
            });
        }
    }
    reset(reg, who, false);
    reg.push(TradeNote {
        to: vec![who, other],
        event: "OnPrimingNeededForTradeItems",
        args: vec![PyValue::List(
            staged_now.iter().map(|i| PyValue::Int(*i)).collect(),
        )],
    });
    Ok(())
}

/// Clear both accepts and say so.
fn reset(reg: &mut Registry, who: i64, scheduled: bool) {
    let Some(t) = reg.trades.of_mut(who) else {
        return;
    };
    t.accepted = [false; 2];
    let t = t.clone();
    tell_offer(reg, &t, scheduled);
}

/// `OfferMoney`: the caller's ISK in the trade.
pub fn offer_money(reg: &mut Registry, who: i64, amount: f64) -> Result<(), Refusal> {
    let t = live(reg, who)?;
    let balance = reg.character(who).map_or(0.0, |c| c.balance);
    if !amount.is_finite() || amount < 0.0 || amount > balance {
        return Err(Refusal::NotEnoughMoney { amount, balance });
    }
    let amount = (amount * 100.0).round() / 100.0;
    let Some(ix) = t.index_of(who) else {
        return Err(Refusal::Words("You are not in a trade."));
    };
    if let Some(tr) = reg.trades.of_mut(who) {
        tr.money[ix] = amount;
        tr.accepted = [false; 2];
    }
    let t = reg.trades.of(who).cloned().unwrap_or(t);
    reg.push(TradeNote {
        to: t.traders.to_vec(),
        event: "OnTradeMoneyOffer",
        args: vec![PyValue::Int(t.container_id), moneys(t.money)],
    });
    Ok(())
}

/// What the client says it is accepting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    pub container_id: Option<i64>,
    pub money: Vec<f64>,
    /// The item ids it read out of `tradeItems`. Empty either because there
    /// are none or because the rows were not readable; see [`make_offer`].
    pub item_ids: BTreeSet<i64>,
}

/// `MakeOffer`: the caller accepts what they see. A manifest that is not the
/// trade as the server holds it resets both accepts and tells the caller.
pub fn make_offer(reg: &mut Registry, who: i64, m: &Manifest) -> Result<(), Refusal> {
    let t = live(reg, who)?;
    let Some(ix) = t.index_of(who) else {
        return Err(Refusal::Words("You are not in a trade."));
    };
    let have: BTreeSet<i64> = staged_ids(reg, t.container_id).into_iter().collect();
    let money_ok = m.money.len() == 2
        && m.money
            .iter()
            .zip(t.money)
            .all(|(a, b)| (a - b).abs() < 0.005);
    // **[A]** `tradeItems` is a `set` of the window's item rows, read by
    // `itemID`. If no id could be read from a trade that has items, the rows
    // are in a shape this parser does not know, and the check is skipped
    // rather than making every trade unacceptable. Falsified by a trade
    // window whose accept never reaches the other side.
    let items_ok = (m.item_ids.is_empty() && !have.is_empty()) || m.item_ids == have;
    if m.container_id != Some(t.container_id) || !money_ok || !items_ok {
        note(
            reg,
            false,
            TradeNote {
                to: vec![who],
                event: "OnTradeOfferReset",
                args: vec![PyValue::Int(t.container_id), PyValue::Bool(true)],
            },
        );
        reset(reg, who, false);
        return Ok(());
    }
    if let Some(tr) = reg.trades.of_mut(who) {
        tr.accepted[ix] = true;
    }
    let t = reg.trades.of(who).cloned().unwrap_or(t);
    tell_offer(reg, &t, false);
    if t.accepted == [true, true] {
        complete(reg, &t);
    }
    Ok(())
}

/// Swap everything. Nothing moves unless the balances hold and the one item
/// plan applies.
fn complete(reg: &mut Registry, t: &Trade) {
    let [a, b] = t.traders;
    let enough =
        |reg: &Registry, id: i64, m: f64| reg.character(id).is_some_and(|c| c.balance >= m);
    let mut edits = Vec::new();
    for id in staged_ids(reg, t.container_id) {
        let Some(item) = reg.store().get(id) else {
            continue;
        };
        let to = if item.owner_id == a { b } else { a };
        edits.push(Edit::Move {
            item_id: id,
            to: Slot::new(to, t.station_id, FLAG_HANGAR),
        });
    }
    #[cfg(test)]
    if POISON.with(std::cell::Cell::get) {
        edits.push(Edit::Destroy { item_id: -1 });
    }
    let ok =
        enough(reg, a, t.money[0]) && enough(reg, b, t.money[1]) && reg.apply_items(&edits).is_ok();
    if !ok {
        // `ItemExchangeFailed` is what the client raises for a reset with no
        // manifest error. Nothing has moved: the plan is all or none, and the
        // ISK has not been touched.
        note(
            reg,
            false,
            TradeNote {
                to: t.traders.to_vec(),
                event: "OnTradeOfferReset",
                args: vec![PyValue::Int(t.container_id), PyValue::Bool(false)],
            },
        );
        reset(reg, a, false);
        return;
    }
    for (payer, payee, m) in [(a, b, t.money[0]), (b, a, t.money[1])] {
        if m > 0.0 {
            reg.charge(payer, m, REF_PLAYER_TRADING, payee, t.container_id);
            credit(reg, payee, m, payer, t.container_id);
        }
    }
    let ids: Vec<PyValue> = edits
        .iter()
        .filter_map(|e| match e {
            Edit::Move { item_id, .. } => Some(PyValue::Int(*item_id)),
            _ => None,
        })
        .collect();
    for e in &edits {
        if let Edit::Move { item_id, .. } = e {
            unstage(reg, *item_id);
        }
    }
    reg.trades.remove(t.container_id);
    reg.mark_dirty();
    reg.push(TradeNote {
        to: t.traders.to_vec(),
        event: "OnTradeComplete",
        args: vec![PyValue::Int(t.container_id), PyValue::List(ids)],
    });
}

/// The payee's half of a paid trade: ISK in, one journal row in the payee's
/// **own** wallet (payer in `owner_id_1`, payee in `owner_id_2`, the amount
/// positive, so `Entry::wallet()` answers the payee), and the balance push.
/// `Registry::charge(payee, -m, ..)` put the payee in `owner_id_1` with a
/// positive amount, which filed the row under the payer.
fn credit(reg: &mut Registry, payee: i64, m: f64, payer: i64, container: i64) {
    let Some(c) = reg.character_mut(payee) else {
        return;
    };
    c.balance += m;
    let balance = c.balance;
    reg.mark_dirty();
    reg.journal_mut().record(
        crate::pyutil::filetime_now(),
        REF_PLAYER_TRADING,
        payer,
        payee,
        container,
        m,
        balance,
        "",
    );
    reg.push(crate::notify::AccountChange {
        owner_id: payee,
        balance,
    });
}

/// `Abort`: the caller (or the window closing) ends the trade. A call with
/// no trade is the closing window of a finished one and is not an error.
pub fn abort(reg: &mut Registry, who: i64) {
    if let Some(t) = reg.trades.of(who).cloned() {
        cancel(reg, t.container_id, false);
    }
}

/// End a trade and send everything home. `scheduled` for a disconnect.
fn cancel(reg: &mut Registry, container: i64, scheduled: bool) {
    let Some(t) = reg.trades.remove(container) else {
        return;
    };
    return_items(reg, container, false);
    note(
        reg,
        scheduled,
        TradeNote {
            to: t.traders.to_vec(),
            event: "OnTradeCancel",
            args: vec![PyValue::Int(container)],
        },
    );
}

/// Move every item in `container` back to its staged source and drop its
/// rows. One plan; if it will not apply (an item vanished), one by one, so a
/// single bad row never strands the others. `quiet` for a load, which has
/// nobody to tell. Returns how many went home.
fn return_items(reg: &mut Registry, container: i64, quiet: bool) -> usize {
    let ids = staged_ids(reg, container);
    let plan: Vec<(i64, Edit)> = ids
        .iter()
        .filter_map(|id| {
            let (loc, flag, owner, _) = staged(reg, *id)?;
            Some((
                *id,
                Edit::Move {
                    item_id: *id,
                    to: Slot::new(owner, loc, flag),
                },
            ))
        })
        .collect();
    let apply = |reg: &mut Registry, edits: &[Edit]| {
        if quiet {
            reg.apply_items_quiet(edits).is_ok()
        } else {
            reg.apply_items(edits).is_ok()
        }
    };
    let edits: Vec<Edit> = plan.iter().map(|(_, e)| e.clone()).collect();
    let mut home = Vec::new();
    if apply(reg, &edits) {
        home = plan.iter().map(|(id, _)| *id).collect();
    } else {
        for (id, e) in &plan {
            if apply(reg, std::slice::from_ref(e)) {
                home.push(*id);
            }
        }
    }
    for id in &home {
        unstage(reg, *id);
    }
    home.len()
}

/// A pilot undocks: their trade ends.
pub fn on_leave(reg: &mut Registry, who: Option<i64>) {
    let Some(who) = who else { return };
    if let Some(t) = reg.trades.of(who).cloned() {
        cancel(reg, t.container_id, false);
    }
}

/// The last session of a pilot closed (`session_hooks::ON_LOGOFF`).
pub fn on_logoff(reg: &mut Registry, who: i64) {
    if let Some(t) = reg.trades.of(who).cloned() {
        cancel(reg, t.container_id, true);
    }
}

/// A world load: every staged item goes home, whatever trade held it.
/// Returns whether anything changed (so the load marks the world dirty).
pub fn recover(reg: &mut Registry) -> bool {
    let rows: Vec<(i64, i64)> = reg
        .deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| Some((k.first()?.parse().ok()?, v.get(3)?.parse().ok()?)))
        .collect();
    if rows.is_empty() {
        return false;
    }
    let containers: BTreeSet<i64> = rows.iter().map(|(_, c)| *c).collect();
    for c in containers {
        return_items(reg, c, true);
    }
    // A row whose item is gone, or sits elsewhere, is dead weight.
    for (id, _) in rows {
        if staged(reg, id).is_some() {
            unstage(reg, id);
        }
    }
    true
}

// ---- reading the client's manifest -------------------------------------------

fn pyint(v: &PyValue) -> Option<i64> {
    match v {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

fn pyreal(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Real(r) => Some(*r),
        PyValue::Int(n) => Some(*n as f64),
        _ => None,
    }
}

/// Every `itemID` carried by item rows anywhere inside `v` (a set of rows
/// arrives as an object holding a list).
fn collect_item_ids(v: &PyValue, out: &mut BTreeSet<i64>) {
    match v {
        PyValue::DbRow(row) => {
            if let Some(ix) = row.descriptor().index_of(b"itemID")
                && let Some(DbValue::Int(n)) = row.values().get(ix)
            {
                out.insert(*n);
            }
        }
        PyValue::Instance { state, .. } => collect_item_ids(state, out),
        PyValue::Dict(entries) => match crate::pyutil::pairs_get(entries, b"itemID") {
            Some(PyValue::Int(n)) => {
                out.insert(*n);
            }
            _ => entries
                .iter()
                .for_each(|(val, _)| collect_item_ids(val, out)),
        },
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            collect_item_ids(header, out);
            list.iter().for_each(|i| collect_item_ids(i, out));
            dict.iter().for_each(|(val, _)| collect_item_ids(val, out));
        }
        PyValue::Tuple(items) | PyValue::List(items) => {
            items.iter().for_each(|i| collect_item_ids(i, out));
        }
        _ => {}
    }
}

/// One attribute of a `KeyVal` (an instance, or a plain dict).
fn attr<'a>(v: &'a PyValue, name: &[u8]) -> Option<&'a PyValue> {
    match v {
        PyValue::Instance { state, .. } => attr(state, name),
        PyValue::Dict(entries) => crate::pyutil::pairs_get(entries, name),
        PyValue::ObjectEx { dict, .. } => crate::pyutil::pairs_get(dict, name),
        _ => None,
    }
}

/// Read `MakeOffer`'s `KeyVal(tradeContainerID, money, tradeItems)`.
pub fn parse_manifest(v: &PyValue) -> Manifest {
    let mut m = Manifest {
        container_id: attr(v, b"tradeContainerID").and_then(pyint),
        ..Manifest::default()
    };
    if let Some(PyValue::List(items) | PyValue::Tuple(items)) = attr(v, b"money") {
        m.money = items.iter().filter_map(pyreal).collect();
    }
    if let Some(items) = attr(v, b"tradeItems") {
        collect_item_ids(items, &mut m.item_ids);
    }
    m
}

// Test seam: when set, the swap's item plan carries one edit that cannot
// apply, so the atomicity of `apply_items` can be seen from outside.
#[cfg(test)]
thread_local! {
    pub(crate) static POISON: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
