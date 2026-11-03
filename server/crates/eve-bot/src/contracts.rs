//! **Bots trade by item-exchange contract** (X-CONTRACT,
//! `docs/lanes/x-contract.md`).
//!
//! The server's half is `eve-server/src/contracts.rs` and the `contractProxy`
//! arms in `service/wallet.rs` (create with escrow, search, accept, delete,
//! the pushes). This module is the bot's half, and it calls only what the
//! client's Contracts window calls **[V]**:
//!
//! - `contractProxy.CreateContract(ContractStruct)` (the wizard's
//!   `createContractWnd.CreateContract`; the struct's seventeen fields);
//! - `contractProxy.SearchContracts(availability=, contractType=, locationID=,
//!   startNum=)` (`contractsearch.SearchContracts`);
//! - `contractProxy.AcceptContract(contractID, forCorp)` (`contracts.py`);
//! - `contractProxy.GetContractListForOwner(ownerID, status, type, issuedBy)`
//!   and `GetMyExpiredContractList(forCorp)` (`contractPanels`), then
//!   `DeleteContract(contractID)`.
//!
//! **Who, and where the decision sits** (one point per career, in
//! `driver::Bot::execute`):
//!
//! | career | decision point | what |
//! |---|---|---|
//! | builder | after `Act::Market` | [`offer_surplus`] with [`Career::Builder`]: blueprint copies the ladder does not need |
//! | explorer (any ratter or hacker that loots) | after `Act::SellLoot` | [`offer_surplus`] with [`Career::Explorer`]: loot stacks still in the hangar |
//! | trader | before `Act::Trade` | [`take_underpriced`]: a public contract here worth more than its price |
//!
//! "The market has no buyer" is read from the book: a stack is offered by
//! contract only when no bid for its type stands at this station
//! ([`plan_offers`] takes the answer as data). Items the market took have
//! left the hangar already, so only the leftovers reach here.
//!
//! The decisions ([`plan_offers`], [`pick_underpriced`]) are pure; the wire
//! functions are thin.

use crate::builder::{BLUEPRINT_LADDER, BROKER_FEE, SellLine};
use crate::escort::{self, Lot};
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// `CONTYPE_ITEMEXCHANGE` **[V]** (`contractscommon`).
pub const TYPE_ITEM_EXCHANGE: i64 = 1;
/// `conAvailPublic` **[V]**.
pub const AVAIL_PUBLIC: i64 = 0;
/// `conStatusOutstanding` **[V]**.
pub const STATUS_OUTSTANDING: i64 = 0;
/// The client's blueprint category **[V-data]**.
pub const CATEGORY_BLUEPRINT: i64 = 9;

/// The share of the reference value a bot asks for a stack by contract
/// **[A]**: under the market's own ask (the market charges a broker's fee
/// and a tax, a contract none, the contract's escrow is free) so that a
/// searching trader finds it worth taking. Falsified by contracts that sit
/// unaccepted until they expire (too high), or by traders whose profit on
/// contracts dwarfs every other deal (too low).
pub const ASK_SHARE: f64 = 0.85;

/// A trader takes a contract only when its price is at most this share of
/// the goods' reference value **[A]**. Below [`ASK_SHARE`] on purpose: a
/// bot's own contract (ask 0.85) is not "underpriced", a player's bargain
/// (the owner's example: a sale at a fraction of the price) is. Falsified
/// by traders that never find a contract (too low) or that buy at the
/// builders' own asks and starve them (too high).
pub const UNDERPRICED_BELOW: f64 = 0.7;

/// The least gain, in ISK, worth a trader's accept **[A]**.
pub const MIN_GAIN: f64 = 10_000.0;

/// A trader spends at most this share of its wallet on one contract **[A]**.
pub const SPEND_SHARE: f64 = 0.5;

/// Open contracts of one bot at a time **[A]**: escrow is the bot's own
/// stock held away, and the Contracts window caps a character too.
pub const MAX_OPEN: usize = 5;

/// How long a bot's contract runs, in minutes **[A]**: three days, within
/// the client's longest option (`contractscommon.EXPIRE_TIMES`).
pub const EXPIRE_MINUTES: i64 = 3 * 1440;

/// Which career is deciding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Career {
    /// Sells blueprint copies it has no use for.
    Builder,
    /// Sells loot no bid takes.
    Explorer,
}

/// One contract a bot will create: part of a stack for a price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub price: f64,
}

/// What a career has to sell, from its hangar rows.
///
/// - the builder: category-9 stacks the blueprint ladder does not name
///   **[A]** (a copy from invention or a reaction, which the ladder never
///   builds; the ladder's own blueprints are the builder's tools);
/// - the explorer: [`escort::lots`], loot past what the ratter keeps.
pub fn stock(career: Career, hangar: &[ItemRow]) -> Vec<Lot> {
    match career {
        Career::Builder => hangar
            .iter()
            .filter(|r| {
                r.category_id == CATEGORY_BLUEPRINT
                    && r.group_id != crate::rig::GROUP_RIG_BLUEPRINT
                    && !BLUEPRINT_LADDER.contains(&r.type_id)
            })
            .map(|r| Lot {
                item_id: r.item_id,
                type_id: r.type_id,
                quantity: r.quantity.max(1),
                whole: true,
            })
            .collect(),
        Career::Explorer => escort::lots_keeping(hangar, 0, &escort::ALL_WEAPONS),
    }
}

/// **The sale decision**: of `lots`, those with a reference price and no
/// bid at this station ([`has_bid`]), each asked at
/// `price x quantity x` [`ASK_SHARE`], at most `room` of them, the most
/// valuable first.
pub fn plan_offers(
    lots: &[Lot],
    prices: &BTreeMap<i64, f64>,
    has_bid: &dyn Fn(i64) -> bool,
    room: usize,
) -> Vec<Offer> {
    let mut offers: Vec<Offer> = lots
        .iter()
        .filter(|l| l.quantity > 0 && !has_bid(l.type_id))
        .filter_map(|l| {
            let unit = *prices.get(&l.type_id)?;
            (unit > 0.0).then(|| Offer {
                item_id: l.item_id,
                type_id: l.type_id,
                quantity: l.quantity,
                price: (unit * l.quantity as f64 * ASK_SHARE).round(),
            })
        })
        .filter(|o| o.price >= 1.0)
        .collect();
    offers.sort_by(|a, b| b.price.total_cmp(&a.price).then(a.item_id.cmp(&b.item_id)));
    offers.truncate(room);
    offers
}

// ---- the wire: the client's calls ------------------------------------------------

fn kw(name: &str) -> PyValue {
    eve_server::pyutil::bstr(name)
}

/// `ContractStruct`, as it marshals: a `0x23` object whose state is the
/// wizard's seventeen keywords **[V]** (`createContractWnd.CreateContract`,
/// `contractStruct.py`); that a new-style instance reduces to `((cls,),
/// state)` is **[I]**, as in the server's reader.
pub fn contract_struct(station: i64, offer: &Offer) -> PyValue {
    contract_struct_for(station, offer, None)
}

/// [`contract_struct`], private to `assignee` when given (`isPrivate`,
/// `assignedToID`: the wizard's "assign to" field **[V]**).
pub fn contract_struct_for(station: i64, offer: &Offer, assignee: Option<i64>) -> PyValue {
    let pair = |a: i64, b: i64| PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)]);
    let state = eve_server::pyutil::pydict(vec![
        ("contractType", PyValue::Int(TYPE_ITEM_EXCHANGE)),
        ("isPrivate", PyValue::Bool(assignee.is_some())),
        ("assignedToID", PyValue::Int(assignee.unwrap_or(0))),
        ("minutesExpire", PyValue::Int(EXPIRE_MINUTES)),
        ("numDays", PyValue::Int(0)),
        ("startStationID", PyValue::Int(station)),
        ("destinationID", PyValue::Int(0)),
        ("price", PyValue::Real(offer.price)),
        ("reward", PyValue::Real(0.0)),
        ("collateral", PyValue::Real(0.0)),
        ("title", PyValue::Unicode(String::new())),
        ("description", PyValue::Unicode(String::new())),
        (
            "itemList",
            PyValue::List(vec![pair(offer.item_id, offer.quantity)]),
        ),
        ("startStationDivision", PyValue::Int(4)),
        ("requestItemTypeList", PyValue::List(vec![])),
        ("forCorp", PyValue::Bool(false)),
        ("multiContract", PyValue::Bool(false)),
    ]);
    PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(
                b"contracts.contractStruct.ContractStruct".to_vec(),
            )]),
            state,
        ])),
        list: vec![],
        dict: vec![],
    }
}

/// `CreateContract` answers the list of new ids (`ShowCreatedContracts` does
/// `len()` and `[0]`, createContractWnd.py) **[V]**. The id, or the
/// refusal's key.
pub fn create(
    conn: &mut Conn,
    station: i64,
    offer: &Offer,
) -> Result<std::result::Result<i64, String>> {
    create_for(conn, station, offer, None)
}

/// [`create`], private to `assignee` when given.
pub fn create_for(
    conn: &mut Conn,
    station: i64,
    offer: &Offer,
    assignee: Option<i64>,
) -> Result<std::result::Result<i64, String>> {
    let out = conn.call(
        "contractProxy",
        "CreateContract",
        vec![contract_struct_for(station, offer, assignee)],
    )?;
    if let Some(key) = out.refusal() {
        return Ok(Err(key));
    }
    Ok(match out.value() {
        Some(PyValue::List(ids)) => match ids.first() {
            Some(PyValue::Int(id)) => Ok(*id),
            other => Err(format!("CreateContract answered {other:?}")),
        },
        other => Err(format!("CreateContract answered {other:?}")),
    })
}

fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    crate::dict_get(crate::attributes(v), name)
}

fn int(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        PyValue::Real(r) => Some(r.round() as i64),
        PyValue::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn num(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(n) => Some(*n as f64),
        PyValue::Real(r) => Some(*r),
        _ => None,
    }
}

/// One public item exchange as `SearchContracts` shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub contract_id: i64,
    pub issuer_id: i64,
    pub station_id: i64,
    pub price: f64,
    /// `(typeID, quantity)` of what the acceptor receives.
    pub gives: Vec<(i64, i64)>,
    /// REVIEW-FIX2: `(typeID, quantity)` the issuer asks for (the lines not in
    /// the crate): a want-to-buy **[A]** (item-exchange lines with `inCrate`
    /// false are the issuer's wants).
    pub wants: Vec<(i64, i64)>,
    /// True when the issuer asks for items back, or any line is not a plain
    /// crate line: a bot cannot fill it.
    pub asks_more: bool,
}

/// The listings in a `SearchContracts` answer: `ret.contracts`, each with
/// `.contract` (`contractID`, `type`, `status`, `issuerID`, `startStationID`,
/// `price`, `reward`) and `.items` (`itemTypeID`, `quantity`, `inCrate`)
/// **[V]**. Rows that are not outstanding item exchanges, or do not read,
/// are skipped.
pub fn listings_of(answer: &PyValue) -> Vec<Listing> {
    let Some(PyValue::List(found)) = attr(answer, "contracts") else {
        return Vec::new();
    };
    found
        .iter()
        .filter_map(|d| {
            let c = attr(d, "contract")?;
            if int(attr(c, "type"))? != TYPE_ITEM_EXCHANGE
                || int(attr(c, "status"))? != STATUS_OUTSTANDING
            {
                return None;
            }
            let mut gives = Vec::new();
            let mut wants = Vec::new();
            let mut asks_more = num(attr(c, "reward")).unwrap_or(0.0) > 0.0;
            if let Some(PyValue::List(items)) = attr(d, "items") {
                for l in items {
                    let line = (
                        int(attr(l, "itemTypeID"))?,
                        int(attr(l, "quantity"))?.max(1),
                    );
                    match attr(l, "inCrate") {
                        Some(PyValue::Bool(true)) => gives.push(line),
                        _ => {
                            wants.push(line);
                            asks_more = true;
                        }
                    }
                }
            }
            Some(Listing {
                contract_id: int(attr(c, "contractID"))?,
                issuer_id: int(attr(c, "issuerID")).unwrap_or(0),
                station_id: int(attr(c, "startStationID"))?,
                price: num(attr(c, "price")).unwrap_or(0.0),
                gives,
                wants,
                asks_more,
            })
        })
        .collect()
}

/// `SearchContracts(availability=public, contractType=item exchange,
/// locationID=station, startNum=0)`: the Search tab's keywords **[V]**
/// (`contractsearch.py`); which ones a bot sends is **[A]**.
pub fn search_here(conn: &mut Conn, station: i64) -> Result<Vec<Listing>> {
    let out = conn.call_kw(
        "contractProxy",
        "SearchContracts",
        vec![],
        vec![
            (PyValue::Int(AVAIL_PUBLIC), kw("availability")),
            (PyValue::Int(TYPE_ITEM_EXCHANGE), kw("contractType")),
            (PyValue::Int(station), kw("locationID")),
            (PyValue::Int(0), kw("startNum")),
        ],
    )?;
    if let Some(key) = out.refusal() {
        return Err(Error::Refused(format!("SearchContracts: {key}")));
    }
    Ok(out.value().as_ref().map(listings_of).unwrap_or_default())
}

/// The ids of the character's own contracts a list answer holds.
fn ids_of(answer: &PyValue) -> Vec<i64> {
    match attr(answer, "contracts") {
        Some(PyValue::List(rows)) => rows
            .iter()
            .filter_map(|c| int(attr(c, "contractID")))
            .collect(),
        _ => Vec::new(),
    }
}

/// How many contracts the character has outstanding, as issuer
/// (`GetContractListForOwner(me, status 0, None, issuedBy=False)`, the My
/// Contracts panel's call **[V]**).
pub fn open_count(conn: &mut Conn, me: i64) -> Result<usize> {
    let out = conn.call(
        "contractProxy",
        "GetContractListForOwner",
        vec![
            PyValue::Int(me),
            PyValue::Int(STATUS_OUTSTANDING),
            PyValue::None,
            PyValue::Bool(false),
        ],
    )?;
    Ok(out.value().as_ref().map(ids_of).map_or(0, |v| v.len()))
}

/// Delete the character's expired contracts, which returns their items
/// (`GetMyExpiredContractList(False)`, then `DeleteContract`) **[V]**.
/// Returns how many were deleted.
pub fn tidy(conn: &mut Conn) -> Result<usize> {
    let out = conn.call(
        "contractProxy",
        "GetMyExpiredContractList",
        vec![PyValue::Bool(false)],
    )?;
    let mut deleted = 0;
    for id in out.value().as_ref().map(ids_of).unwrap_or_default() {
        let gone = conn.call("contractProxy", "DeleteContract", vec![PyValue::Int(id)])?;
        if gone.refusal().is_none() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// **A career's sale** (docked): tidy expired contracts, then offer the
/// stock no bid takes, up to [`MAX_OPEN`] open in all. A refusal is a note,
/// never a failed act. Returns the log line.
pub fn offer_surplus(conn: &mut Conn, career: Career) -> Result<String> {
    offer_surplus_for(conn, career, None)
}

/// A buyer a bot offers its best stack to privately: an allied pilot
/// (R-REL-3, [`crate::effects::Gate::Discount`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Buyer {
    pub id: i64,
    pub gate: crate::effects::Gate,
}

/// The cheapest a private offer to an ally goes, as a share of the public
/// ask **[A]**: the discount never eats the seller's margin. Falsified by
/// builders selling to allies at a loss.
pub const ALLY_FLOOR_SHARE: f64 = 0.8;

/// [`offer_surplus`] with `buyer`: the most valuable stack goes to that
/// pilot as a private contract at the gate's price, the rest stays public
/// at [`ASK_SHARE`].
pub fn offer_surplus_for(conn: &mut Conn, career: Career, buyer: Option<Buyer>) -> Result<String> {
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Ok("contracts: not docked".to_owned());
    };
    let freed = tidy(conn)?;
    let lots = stock(career, &conn.hangar_items()?);
    if lots.is_empty() {
        return Ok(format!(
            "contracts: nothing to offer ({freed} expired deleted)"
        ));
    }
    let prices: BTreeMap<i64, f64> = conn.adjusted_prices()?.into_iter().collect();
    let room = MAX_OPEN.saturating_sub(open_count(conn, me)?);
    let mut bids: BTreeMap<i64, bool> = BTreeMap::new();
    for l in &lots {
        if prices.contains_key(&l.type_id) && !bids.contains_key(&l.type_id) {
            let any = conn
                .market_orders(l.type_id)?
                .bids
                .iter()
                .any(|b| b.station_id == station);
            bids.insert(l.type_id, any);
        }
    }
    let offers = plan_offers(
        &lots,
        &prices,
        &|t| bids.get(&t).copied().unwrap_or(true),
        room,
    );
    let mut made = Vec::new();
    for (i, o) in offers.iter().enumerate() {
        let (offer, to) = match buyer {
            Some(b) if i == 0 => match b.gate.ask(o.price, o.price * ALLY_FLOOR_SHARE) {
                Some(price) => (
                    Offer {
                        price: price.round(),
                        ..*o
                    },
                    Some(b.id),
                ),
                None => continue,
            },
            _ => (*o, None),
        };
        let o = &offer;
        match create_for(conn, station, o, to)? {
            Ok(id) => made.push(format!(
                "#{id} {}x{} for {:.0}{}",
                o.quantity,
                o.type_id,
                o.price,
                to.map_or(String::new(), |t| format!(" to {t}"))
            )),
            Err(why) => made.push(format!("refused {}: {why}", o.type_id)),
        }
    }
    Ok(format!(
        "contracts: {} offered ({}), {freed} expired deleted",
        offers.len(),
        made.join("; ")
    ))
}

// ---- blueprint copies from the hackers' data sites -------------------------------

/// What a copy is on the wire: `quantity -2` (`industry::BlueprintItem::
/// wire_quantity`, an original is -1) **[V]**. The store holds one item of
/// quantity 1, which is what a contract line names.
pub const WIRE_COPY: i64 = -2;

/// The flat price of a data-site copy, in ISK **[A]**: the market takes no
/// copy (`RepackageBeforeSelling`: a blueprint is a singleton), and no
/// reference price names one, so the hacker asks a token sum that the corp's
/// builder pays without noticing. Falsified by copies that sit unaccepted
/// (too high) or builders that bleed ISK into hackers (too low is harmless).
pub const COPY_PRICE: f64 = 50_000.0;

/// The most copies a builder takes from contracts in one cycle **[A]**.
pub const COPIES_PER_CYCLE: usize = 3;

/// Is `row` a blueprint copy (not an original, not a rig blueprint, which
/// the rig builder buys by its own rule).
pub fn is_copy(row: &ItemRow) -> bool {
    row.category_id == CATEGORY_BLUEPRINT
        && row.quantity == WIRE_COPY
        && row.group_id != crate::rig::GROUP_RIG_BLUEPRINT
}

/// **The hacker's offer of copies** (pure): one public contract per copy,
/// at [`COPY_PRICE`], at most `room`, lowest item id first.
pub fn plan_copy_offers(hangar: &[ItemRow], room: usize) -> Vec<Offer> {
    let mut copies: Vec<&ItemRow> = hangar.iter().filter(|r| is_copy(r)).collect();
    copies.sort_by_key(|r| r.item_id);
    copies
        .into_iter()
        .take(room)
        .map(|r| Offer {
            item_id: r.item_id,
            type_id: r.type_id,
            quantity: 1,
            price: COPY_PRICE,
        })
        .collect()
}

/// **A hacker's copies, offered** (docked): tidy expired contracts, then a
/// public item exchange for each copy in the hangar, up to [`MAX_OPEN`] open
/// in all. The market refuses a blueprint, so the contract is the only way a
/// copy reaches a builder. A refusal is a note. Returns the log line.
pub fn offer_copies(conn: &mut Conn) -> Result<String> {
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Ok("copies: not docked".to_owned());
    };
    let hangar = conn.hangar_items()?;
    if !hangar.iter().any(is_copy) {
        return Ok("copies: none held".to_owned());
    }
    tidy(conn)?;
    let room = MAX_OPEN.saturating_sub(open_count(conn, me)?);
    let mut made = Vec::new();
    for o in plan_copy_offers(&hangar, room) {
        match create(conn, station, &o)? {
            Ok(id) => made.push(format!("#{id} {}", o.type_id)),
            Err(why) => made.push(format!("refused {}: {why}", o.type_id)),
        }
    }
    Ok(format!("copies: offered {}", made.join("; ")))
}

/// **The builder's pick of copies** (pure): public single-copy listings
/// whose blueprint `makes_wanted` (a recipe for a product a doctrine wants),
/// not its own, priced at most [`COPY_PRICE`] and within [`SPEND_SHARE`] of
/// the wallet, cheapest first, at most [`COPIES_PER_CYCLE`].
pub fn pick_copies<'a>(
    listings: &'a [Listing],
    makes_wanted: &dyn Fn(i64) -> bool,
    wallet: f64,
    me: i64,
) -> Vec<&'a Listing> {
    let mut v: Vec<&Listing> = listings
        .iter()
        .filter(|l| l.issuer_id != me && !l.asks_more && l.price <= COPY_PRICE)
        .filter(|l| l.price <= wallet * SPEND_SHARE)
        .filter(|l| matches!(l.gives[..], [(t, 1)] if makes_wanted(t)))
        .collect();
    v.sort_by(|a, b| {
        a.price
            .total_cmp(&b.price)
            .then(a.contract_id.cmp(&b.contract_id))
    });
    v.truncate(COPIES_PER_CYCLE);
    v
}

/// **A builder collects copies** (docked): accept the public listings
/// [`pick_copies`] names, so the hackers' data-site copies reach the one who
/// builds from them. A refused accept is skipped. Returns how many.
pub fn collect_copies(conn: &mut Conn, makes_wanted: &dyn Fn(i64) -> bool) -> Result<usize> {
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Ok(0);
    };
    let listings = search_here(conn, station)?;
    let wallet = conn.cash_balance()?;
    let mut taken = 0;
    for l in pick_copies(&listings, makes_wanted, wallet, me) {
        let out = conn.call(
            "contractProxy",
            "AcceptContract",
            vec![PyValue::Int(l.contract_id), PyValue::Bool(false)],
        )?;
        if out.refusal().is_none() {
            taken += 1;
        }
    }
    Ok(taken)
}

/// **The buy decision**: the listing with the greatest gain among those a
/// bot can fill and afford: no ask of its own items, not its own, every line
/// priced, price at most [`UNDERPRICED_BELOW`] of the value, gain at least
/// [`MIN_GAIN`], price within [`SPEND_SHARE`] of the wallet.
pub fn pick_underpriced<'a>(
    listings: &'a [Listing],
    prices: &BTreeMap<i64, f64>,
    wallet: f64,
    me: i64,
) -> Option<&'a Listing> {
    pick_underpriced_gated(listings, prices, wallet, me, &|_| {
        crate::effects::Gate::Normal
    })
}

/// [`pick_underpriced`] through the counterparty gate (R-REL-3,
/// R-RELATIONS §3.3): `gate(issuer)` is what the bot makes of that seller.
/// A refused seller's contracts are not looked at; a wary seller's must be
/// cheaper by the premium it would have charged.
pub fn pick_underpriced_gated<'a>(
    listings: &'a [Listing],
    prices: &BTreeMap<i64, f64>,
    wallet: f64,
    me: i64,
    gate: &dyn Fn(i64) -> crate::effects::Gate,
) -> Option<&'a Listing> {
    listings
        .iter()
        .filter(|l| l.issuer_id != me && !l.asks_more && !l.gives.is_empty() && l.price > 0.0)
        .filter(|l| gate(l.issuer_id).bar(UNDERPRICED_BELOW).is_some())
        .filter(|l| l.price <= wallet * SPEND_SHARE)
        .filter_map(|l| {
            let value = l
                .gives
                .iter()
                .map(|(t, q)| prices.get(t).map(|p| p * *q as f64))
                .sum::<Option<f64>>()?;
            let bar = gate(l.issuer_id).bar(UNDERPRICED_BELOW)?;
            (l.price <= value * bar && value - l.price >= MIN_GAIN).then_some((l, value - l.price))
        })
        .max_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then(b.0.contract_id.cmp(&a.0.contract_id))
        })
        .map(|(l, _)| l)
}

/// What [`take_underpriced`] did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Taken {
    pub contract_id: Option<i64>,
    pub paid: f64,
    /// `(typeID, quantity)` received.
    pub got: Vec<(i64, i64)>,
    pub listed: usize,
    pub note: String,
}

/// **A trader's accept** (docked): search the public item exchanges at this
/// station, take the best underpriced one, and list what came at the
/// reference price ([`ASK_SHARE`]) so the profit is realised through the
/// market. `prices` is the reference table (`GetAverageMarketPrices`).
pub fn take_underpriced_with(conn: &mut Conn, prices: &BTreeMap<i64, f64>) -> Result<Taken> {
    take_underpriced_gated(conn, prices, None)
}

/// [`take_underpriced_with`] through the counterparty gate: `gate_of(issuer,
/// affiliation)` is what the bot makes of a seller, asked after the seller's
/// affiliation (`GetPublicInfo`) is known. `None`: every seller is normal.
pub fn take_underpriced_gated(
    conn: &mut Conn,
    prices: &BTreeMap<i64, f64>,
    gate_of: Option<&dyn Fn(i64, Option<crate::fleet::Affiliation>) -> crate::effects::Gate>,
) -> Result<Taken> {
    let (Some(station), Some(me)) = (conn.session().station_id(), conn.session().char_id()) else {
        return Ok(Taken {
            note: "contracts: not docked".to_owned(),
            ..Taken::default()
        });
    };
    let listings = search_here(conn, station)?;
    let wallet = conn.cash_balance()?;
    // Who sells: each issuer once, with the affiliation the server tells.
    let mut gates: BTreeMap<i64, crate::effects::Gate> = BTreeMap::new();
    for l in &listings {
        let Some(gate_of) = gate_of else { break };
        if l.issuer_id == me || gates.contains_key(&l.issuer_id) {
            continue;
        }
        let aff = match conn.affiliation_of(l.issuer_id) {
            Ok(a) => Some(a),
            Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
            Err(_) => None,
        };
        gates.insert(l.issuer_id, gate_of(l.issuer_id, aff));
    }
    let gate = |issuer: i64| {
        gates
            .get(&issuer)
            .copied()
            .unwrap_or(crate::effects::Gate::Normal)
    };
    let Some(pick) = pick_underpriced_gated(&listings, prices, wallet, me, &gate).cloned() else {
        return Ok(Taken {
            note: format!("contracts: {} listed, none underpriced", listings.len()),
            ..Taken::default()
        });
    };
    let before = stock_of(&conn.hangar_items()?, &pick.gives);
    let out = conn.call(
        "contractProxy",
        "AcceptContract",
        vec![PyValue::Int(pick.contract_id), PyValue::Bool(false)],
    )?;
    if let Some(key) = out.refusal() {
        return Ok(Taken {
            note: format!("accept {}: {key}", pick.contract_id),
            ..Taken::default()
        });
    }
    let hangar = conn.hangar_items()?;
    let after = stock_of(&hangar, &pick.gives);
    // Offer what arrived, never more than the contract gave.
    let mut lines = Vec::new();
    for &(type_id, qty) in &pick.gives {
        let mut left = qty.min(
            after.get(&type_id).copied().unwrap_or(0) - before.get(&type_id).copied().unwrap_or(0),
        );
        let Some(unit) = prices.get(&type_id) else {
            continue;
        };
        for r in hangar
            .iter()
            .filter(|r| r.type_id == type_id && !r.singleton)
        {
            if left <= 0 {
                break;
            }
            let take = left.min(r.quantity);
            lines.push(SellLine {
                station_id: station,
                type_id,
                item_id: r.item_id,
                price: (unit * ASK_SHARE * 100.0).round() / 100.0,
                quantity: take,
            });
            left -= take;
        }
    }
    let mut listed = 0;
    if !lines.is_empty() {
        match conn.sell(&lines, escort::LIST_DAYS, BROKER_FEE) {
            Ok(()) => listed = lines.len(),
            Err(Error::Refused(_) | Error::NotSent(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Taken {
        contract_id: Some(pick.contract_id),
        paid: pick.price,
        got: pick.gives.clone(),
        listed,
        note: format!(
            "contracts: took #{} for {:.0}, listed {listed} stacks",
            pick.contract_id, pick.price
        ),
    })
}

/// [`take_underpriced_with`] at the live reference prices.
pub fn take_underpriced(conn: &mut Conn) -> Result<Taken> {
    let prices: BTreeMap<i64, f64> = conn.adjusted_prices()?.into_iter().collect();
    take_underpriced_with(conn, &prices)
}

/// Units of each wanted type in the hangar (stackable rows).
fn stock_of(hangar: &[ItemRow], wanted: &[(i64, i64)]) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    for &(t, _) in wanted {
        let n: i64 = hangar
            .iter()
            .filter(|r| r.type_id == t && !r.singleton)
            .map(|r| r.quantity)
            .sum();
        out.insert(t, n);
    }
    out
}

#[cfg(test)]
mod tests {
    //! Invented ids and prices; the named consts stand for the [A] values.
    use super::*;

    const ME: i64 = 90_000_901;
    const OTHER: i64 = 90_000_902;
    const GOOD: i64 = 990_900_001;
    const OTHER_GOOD: i64 = 990_900_002;
    const BP: i64 = 990_900_003;
    const STATION: i64 = 60_900_901;
    const UNIT: f64 = 1_000.0;
    const RICH: f64 = 1.0e9;

    fn prices() -> BTreeMap<i64, f64> {
        BTreeMap::from([(GOOD, UNIT), (OTHER_GOOD, UNIT * 10.0), (BP, 50_000.0)])
    }

    fn lot(item_id: i64, type_id: i64, quantity: i64) -> Lot {
        Lot {
            item_id,
            type_id,
            quantity,
            whole: true,
        }
    }

    fn listing(id: i64, price: f64, gives: &[(i64, i64)]) -> Listing {
        Listing {
            contract_id: id,
            issuer_id: OTHER,
            station_id: STATION,
            price,
            gives: gives.to_vec(),
            wants: Vec::new(),
            asks_more: false,
        }
    }

    fn row(item_id: i64, type_id: i64, category_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: ME,
            location_id: STATION,
            flag_id: 4,
            quantity,
            group_id: 0,
            category_id,
            singleton: false,
        }
    }

    #[test]
    fn a_stack_no_bid_takes_is_asked_below_its_reference() {
        let lots = [lot(1, GOOD, 100)];
        let offers = plan_offers(&lots, &prices(), &|_| false, MAX_OPEN);
        assert_eq!(
            offers,
            vec![Offer {
                item_id: 1,
                type_id: GOOD,
                quantity: 100,
                price: (UNIT * 100.0 * ASK_SHARE).round(),
            }]
        );
        assert!(offers[0].price < UNIT * 100.0);
    }

    #[test]
    fn a_stack_with_a_bid_or_no_price_is_not_offered() {
        let lots = [lot(1, GOOD, 10), lot(2, 990_999_999, 10)];
        assert!(plan_offers(&lots, &prices(), &|t| t == GOOD, MAX_OPEN).is_empty());
        assert!(plan_offers(&lots[1..], &prices(), &|_| false, MAX_OPEN).is_empty());
    }

    #[test]
    fn the_most_valuable_stacks_go_first_and_room_caps_them() {
        let lots = [lot(1, GOOD, 1), lot(2, OTHER_GOOD, 1), lot(3, BP, 1)];
        let offers = plan_offers(&lots, &prices(), &|_| false, 2);
        assert_eq!(offers.iter().map(|o| o.item_id).collect::<Vec<_>>(), [3, 2]);
        assert!(plan_offers(&lots, &prices(), &|_| false, 0).is_empty());
    }

    #[test]
    fn the_builder_offers_blueprints_off_its_ladder_only() {
        let on_ladder = BLUEPRINT_LADDER[0];
        let hangar = [
            row(1, on_ladder, CATEGORY_BLUEPRINT, 1),
            row(2, BP, CATEGORY_BLUEPRINT, 1),
            row(3, GOOD, 4, 50),
        ];
        let lots = stock(Career::Builder, &hangar);
        assert_eq!(lots.iter().map(|l| l.item_id).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn a_hackers_copies_are_offered_one_contract_each_at_the_flat_price() {
        let hangar = [
            row(5, BP, CATEGORY_BLUEPRINT, WIRE_COPY),
            row(4, OTHER_GOOD, CATEGORY_BLUEPRINT, WIRE_COPY),
            row(3, BP, CATEGORY_BLUEPRINT, -1),
            row(2, GOOD, 4, 50),
        ];
        let offers = plan_copy_offers(&hangar, MAX_OPEN);
        assert_eq!(offers.iter().map(|o| o.item_id).collect::<Vec<_>>(), [4, 5]);
        assert!(
            offers
                .iter()
                .all(|o| o.quantity == 1 && o.price == COPY_PRICE)
        );
        assert_eq!(plan_copy_offers(&hangar, 1).len(), 1);
        assert!(plan_copy_offers(&hangar, 0).is_empty());
    }

    #[test]
    fn a_builder_takes_only_copies_of_what_it_wants_at_a_fair_price() {
        let wants = |t: i64| t == BP;
        let mut cheap = listing(1, COPY_PRICE, &[(BP, 1)]);
        cheap.issuer_id = OTHER;
        let other_type = listing(2, COPY_PRICE, &[(GOOD, 1)]);
        let dear = listing(3, COPY_PRICE * 10.0, &[(BP, 1)]);
        let mut mine = listing(4, COPY_PRICE, &[(BP, 1)]);
        mine.issuer_id = ME;
        let bundle = listing(5, COPY_PRICE, &[(BP, 1), (GOOD, 1)]);
        let mut asks = listing(6, COPY_PRICE, &[(BP, 1)]);
        asks.asks_more = true;
        let all = [other_type, dear, mine, bundle, asks, cheap.clone()];
        assert_eq!(pick_copies(&all, &wants, RICH, ME), vec![&cheap]);
        // A wallet that cannot spare the price takes nothing.
        assert!(pick_copies(&all, &wants, COPY_PRICE, ME).is_empty());
    }

    #[test]
    fn an_underpriced_contract_is_picked_and_a_fair_one_is_not() {
        let bargain = listing(1, UNIT * 100.0 * UNDERPRICED_BELOW * 0.5, &[(GOOD, 100)]);
        let fair = listing(2, UNIT * 100.0 * ASK_SHARE, &[(GOOD, 100)]);
        let both = [fair.clone(), bargain.clone()];
        assert_eq!(pick_underpriced(&both, &prices(), RICH, ME), Some(&bargain));
        assert_eq!(pick_underpriced(&[fair], &prices(), RICH, ME), None);
    }

    #[test]
    fn a_hostile_sellers_bargain_is_not_taken_and_a_wary_ones_must_be_cheaper() {
        use crate::effects::{Gate, WARY_PREMIUM};
        const FOE: i64 = 90_000_988;
        const SHY: i64 = 90_000_989;
        let value = UNIT * 100.0;
        let mut a = listing(1, value * UNDERPRICED_BELOW * 0.5, &[(GOOD, 100)]);
        a.issuer_id = FOE;
        let mut b = listing(2, value * UNDERPRICED_BELOW * 0.9, &[(GOOD, 100)]);
        b.issuer_id = SHY;
        let gate = |who: i64| match who {
            FOE => Gate::Refuse,
            SHY => Gate::Premium(WARY_PREMIUM),
            _ => Gate::Normal,
        };
        let all = [a.clone(), b.clone()];
        // 0.9 of the bar is over the wary bar (bar / premium), so neither.
        assert_eq!(
            pick_underpriced_gated(&all, &prices(), RICH, ME, &gate),
            None
        );
        // A normal seller at the same price would have been taken.
        let mut n = b.clone();
        n.issuer_id = 90_000_990;
        assert_eq!(
            pick_underpriced_gated(&[n.clone()], &prices(), RICH, ME, &gate),
            Some(&n)
        );
        // A wary seller far enough under the bar is taken.
        let mut c = listing(
            3,
            value * UNDERPRICED_BELOW / WARY_PREMIUM * 0.5,
            &[(GOOD, 100)],
        );
        c.issuer_id = SHY;
        assert_eq!(
            pick_underpriced_gated(&[c.clone()], &prices(), RICH, ME, &gate),
            Some(&c)
        );
    }

    #[test]
    fn a_private_offer_names_its_buyer_and_a_public_one_does_not() {
        let o = Offer {
            item_id: 1,
            type_id: GOOD,
            quantity: 1,
            price: 5.0,
        };
        let private = contract_struct_for(STATION, &o, Some(90_000_977));
        let get = |v: &PyValue, n: &str| attr(&v_state(v), n).cloned();
        assert_eq!(
            get(&private, "assignedToID"),
            Some(PyValue::Int(90_000_977))
        );
        assert_eq!(get(&private, "isPrivate"), Some(PyValue::Bool(true)));
        let public = contract_struct(STATION, &o);
        assert_eq!(get(&public, "isPrivate"), Some(PyValue::Bool(false)));
        assert_eq!(get(&public, "assignedToID"), Some(PyValue::Int(0)));
    }

    #[test]
    fn the_best_gain_wins_and_ties_go_to_the_older_contract() {
        let a = listing(5, 1_000.0, &[(OTHER_GOOD, 10)]);
        let b = listing(4, 1_000.0, &[(OTHER_GOOD, 10)]);
        let small = listing(3, 1_000.0, &[(GOOD, 50)]);
        let all = [a, b.clone(), small];
        assert_eq!(pick_underpriced(&all, &prices(), RICH, ME), Some(&b));
    }

    #[test]
    fn a_trader_skips_what_it_cannot_fill_afford_or_value() {
        let cheap = UNIT * 100.0 * 0.1;
        let mut asks = listing(1, cheap, &[(GOOD, 100)]);
        asks.asks_more = true;
        let mut own = listing(2, cheap, &[(GOOD, 100)]);
        own.issuer_id = ME;
        let unpriced = listing(3, cheap, &[(990_999_999, 100)]);
        let free = listing(4, 0.0, &[(GOOD, 100)]);
        let dear = listing(5, cheap, &[(GOOD, 100)]);
        let all = [asks, own, unpriced, free];
        assert_eq!(pick_underpriced(&all, &prices(), RICH, ME), None);
        // Too poor: the price is over the spend share of the wallet.
        assert_eq!(
            pick_underpriced(&[dear], &prices(), cheap / SPEND_SHARE - 1.0, ME),
            None
        );
    }

    #[test]
    fn a_gain_under_the_floor_is_not_worth_an_accept() {
        let price = UNIT * UNDERPRICED_BELOW - 1.0;
        let l = listing(1, price, &[(GOOD, 1)]);
        assert!(UNIT - price < MIN_GAIN);
        assert_eq!(pick_underpriced(&[l], &prices(), RICH, ME), None);
    }

    #[test]
    fn a_search_answer_reads_into_listings() {
        use eve_server::pyutil::keyval;
        let line = |t: i64, q: i64, in_crate: bool| {
            keyval(vec![
                ("itemTypeID", PyValue::Int(t)),
                ("quantity", PyValue::Int(q)),
                ("inCrate", PyValue::Bool(in_crate)),
            ])
        };
        let contract = |id: i64, kind: i64, status: i64| {
            keyval(vec![
                ("contractID", PyValue::Int(id)),
                ("type", PyValue::Int(kind)),
                ("status", PyValue::Int(status)),
                ("issuerID", PyValue::Int(OTHER)),
                ("startStationID", PyValue::Int(STATION)),
                ("price", PyValue::Real(5_000.0)),
                ("reward", PyValue::Real(0.0)),
            ])
        };
        let entry = |c: PyValue, items: Vec<PyValue>| {
            keyval(vec![
                ("contract", c),
                ("items", PyValue::List(items)),
                ("bids", PyValue::List(vec![])),
            ])
        };
        let answer = keyval(vec![(
            "contracts",
            PyValue::List(vec![
                entry(
                    contract(1, TYPE_ITEM_EXCHANGE, 0),
                    vec![line(GOOD, 7, true)],
                ),
                entry(contract(2, 3, 0), vec![line(GOOD, 7, true)]),
                entry(
                    contract(3, TYPE_ITEM_EXCHANGE, 4),
                    vec![line(GOOD, 7, true)],
                ),
                entry(
                    contract(4, TYPE_ITEM_EXCHANGE, 0),
                    vec![line(GOOD, 7, true), line(OTHER_GOOD, 1, false)],
                ),
            ]),
        )]);
        let got = listings_of(&answer);
        assert_eq!(got.len(), 2, "a courier and a finished one are skipped");
        assert_eq!(got[0].gives, vec![(GOOD, 7)]);
        assert!(!got[0].asks_more);
        assert!(
            got[1].asks_more,
            "a requested line is one a bot cannot fill"
        );
    }

    #[test]
    fn the_struct_carries_the_offer_in_the_wizards_fields() {
        let offer = Offer {
            item_id: 7,
            type_id: GOOD,
            quantity: 3,
            price: 1_234.0,
        };
        let v = contract_struct(STATION, &offer);
        let get = |n: &str| attr(&v_state(&v), n).cloned();
        assert_eq!(get("contractType"), Some(PyValue::Int(TYPE_ITEM_EXCHANGE)));
        assert_eq!(get("assignedToID"), Some(PyValue::Int(0)));
        assert_eq!(get("startStationID"), Some(PyValue::Int(STATION)));
        assert_eq!(get("price"), Some(PyValue::Real(1_234.0)));
        assert_eq!(get("minutesExpire"), Some(PyValue::Int(EXPIRE_MINUTES)));
        assert_eq!(
            get("itemList"),
            Some(PyValue::List(vec![PyValue::List(vec![
                PyValue::Int(7),
                PyValue::Int(3)
            ])]))
        );
        assert_eq!(get("forCorp"), Some(PyValue::Bool(false)));
    }

    /// The reduce state of the `0x23` object the struct is.
    fn v_state(v: &PyValue) -> PyValue {
        let PyValue::ObjectEx { header, .. } = v else {
            panic!("an object")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a reduce tuple")
        };
        parts[1].clone()
    }
}
