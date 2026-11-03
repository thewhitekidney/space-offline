//! **What a bot does with the new economy, docked** (L7 BOT-ECON, issue #34;
//! spec `handback/r-nes-econ/report.md` §2.7). One act, [`crate::brain::Act::Econ`],
//! every [`RECHECK`] (at once when a geode is aboard), the verbs a player has:
//!
//! 1. **Open the geodes** it holds, in the hold or the hangar
//!    (`crateService.ClaimLootFromCrateStack`, docked only: the core says so).
//!    What is inside is the module's (`eve-mod-econ`); ore and minerals go on
//!    to the ordinary sale, PLEX lands in the vault, injectors and SKIN
//!    licences in the hangar.
//! 2. **Wear a SKIN.** A bot with PLEX above its reserve buys the cheapest
//!    SKIN licence that fits the hull it flies ([`skin_offer`], `storeManager`
//!    `get_offers` and `buy_offer`), claims the redeem token
//!    (`userSvc.ClaimRedeemTokens`), activates the licence
//!    (`ActivateSkinLicense`) and applies it (`ApplySkinToShip`): visible to
//!    every human on the grid. Licences it already holds (a geode's) are
//!    activated and applied the same way. One SKIN per hull.
//! 3. **Inject.** A young bot (under [`OLD_SP`] points) uses the injectors it
//!    holds, and buys one from the market when the ask is under what the
//!    hours saved are worth to it ([`inject_worth`]); the points go to the
//!    queue as the human's "apply" button does.
//! 4. **Extract.** An old bot (at or over [`OLD_SP`]) with an empty skill
//!    queue takes [`BUCKET`] points off skills nothing needs
//!    ([`extraction_plan`]) with an extractor it bought for PLEX and sells
//!    the injector into the market's bid.
//! 5. **Sell surplus PLEX** ([`crate::plex_trade`]), and a trader speculates
//!    in it.
//!
//! **Nothing from nowhere, no GM.** Every step is a verb the client sends and
//! every unit it moves was made by a source or bought from another pilot. A
//! step the server refuses is noted and not repeated before [`BACKOFF`].
//!
//! **Every number is [A]**; the falsifier is beside each.

use crate::acquire;
use crate::geode_haul::Table;
use crate::plex_trade::{self, Move};
use crate::price_memory::Desk;
use crate::travel::Map;
use crate::verbs::{ItemRow, SellLine};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::skill::{required_in, sp_for_level};
use eve_server::skill_trading::{BUCKET, MIN_TO_EXTRACT, TYPE_EXTRACTOR, TYPE_LARGE, TYPE_SMALL};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How often a docked bot spends a turn here **[A]**. Falsified by a hangar
/// that holds unopened geodes for hours (shorter) or a docked bot that never
/// seems to be doing anything else (longer).
pub const RECHECK: Duration = Duration::from_secs(20 * 60);
/// A refused step waits this long before it is tried again **[A]**.
pub const BACKOFF: Duration = Duration::from_secs(6 * 3600);
/// The skill points that make a bot "old" **[A]**: it extracts rather than
/// injects. The owner's figure (§2.7), 5.5 M.
pub const OLD_SP: i64 = 5_500_000;
/// Skill points a bot trains in an hour **[A]**, for what an injector saves
/// it. Falsified by the soak's skill ledger.
pub const SP_PER_HOUR: f64 = 1_800.0;
/// The share of the hours' worth a bot will pay for an injector **[A]**.
pub const INJECT_MARGIN: f64 = 0.5;
/// The PLEX a SKIN purchase leaves in the vault **[A]**.
pub const SKIN_KEEP: i64 = 30;
/// Whole ISK a bot spends on an injector at most, as a share of its wallet **[A]**.
pub const INJECT_WALLET: f64 = 0.5;
/// The NES wishlist, PLEX: what the bot would buy next, for the surplus
/// rule ([`plex_trade::surplus`]) **[A]**.
pub const WISH_EXTRACTOR: i64 = 100;

/// `PLX`, the store's currency code (`nes::CURRENCY`).
const CURRENCY: &str = "PLX";
/// `store_id` the client passes to `get_offers` (ignored by the server).
const STORE_ID: i64 = 1;
/// Crates a stack opens at once, the core's `MAX_MULTI_OPEN`.
const OPEN_AT_ONCE: i64 = 30;

// ---- the state --------------------------------------------------------------------

/// What the bot remembers between turns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub checked: Option<Instant>,
    /// Ships already wearing a SKIN of ours (or one we gave up on).
    pub skinned: BTreeSet<i64>,
    /// A refused kind of step waits until this instant.
    wait: BTreeMap<&'static str, Instant>,
    /// Wallet and Unix second of the last turn, and the ISK/h EWMA.
    last_wallet: Option<(f64, u64)>,
    spent: f64,
    isk_h: f64,
    /// Geodes opened in all.
    pub opened: u64,
}

impl State {
    fn backed_off(&self, what: &'static str, now: Instant) -> bool {
        self.wait.get(what).is_some_and(|t| now < *t)
    }

    fn back_off(&mut self, what: &'static str, now: Instant) {
        self.wait.insert(what, now + BACKOFF);
    }

    /// Fold the wallet at `now_s` into the income estimate: what it rose by,
    /// plus what this module spent, per hour, smoothed.
    fn income(&mut self, wallet: f64, now_s: u64) {
        if let Some((w0, t0)) = self.last_wallet
            && now_s > t0 + 60
        {
            let hours = (now_s - t0) as f64 / 3600.0;
            let made = ((wallet - w0 + self.spent) / hours).max(0.0);
            self.isk_h = if self.isk_h == 0.0 {
                made
            } else {
                0.7 * self.isk_h + 0.3 * made
            };
        }
        self.last_wallet = Some((wallet, now_s));
        self.spent = 0.0;
    }

    /// ISK an hour the bot has been making, smoothed (0 before two turns).
    pub fn isk_per_hour(&self) -> f64 {
        self.isk_h
    }
}

// ---- pure rules -------------------------------------------------------------------

/// One store offer as `get_offers` lists it (`nes::offers_value`).
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub id: i64,
    pub price: i64,
    /// The first product's type; 0 for a service.
    pub type_id: i64,
    pub can_purchase: bool,
}

fn int_of(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// The offers in a `get_offers` answer. A placeholder answer (no catalogue)
/// has none.
pub fn offers_of(v: &PyValue) -> Vec<Offer> {
    let PyValue::List(offers) = v else {
        return Vec::new();
    };
    offers
        .iter()
        .filter_map(|o| {
            let id = int_of(eve_server::pyutil::dict_get(o, b"id"))?;
            let price = match eve_server::pyutil::dict_get(o, b"offerPricings")? {
                PyValue::List(p) => int_of(eve_server::pyutil::dict_get(p.first()?, b"price"))?,
                _ => return None,
            };
            let type_id = match eve_server::pyutil::dict_get(o, b"products") {
                Some(PyValue::List(p)) => p
                    .first()
                    .and_then(|p| int_of(eve_server::pyutil::dict_get(p, b"typeId")))
                    .unwrap_or(0),
                _ => 0,
            };
            let can_purchase = matches!(
                eve_server::pyutil::dict_get(o, b"canPurchase"),
                Some(PyValue::Bool(true))
            );
            Some(Offer {
                id,
                price,
                type_id,
                can_purchase,
            })
        })
        .collect()
}

/// **The SKIN to buy** (pure): the cheapest offer the vault pays for,
/// leaving `keep` PLEX, whose licence fits `hull`. A licence the table does
/// not know is not a SKIN; a hull with no fitting SKIN buys nothing.
pub fn skin_offer<'a>(
    table: &Table,
    offers: &'a [Offer],
    hull: i64,
    balance: i64,
    keep: i64,
) -> Option<&'a Offer> {
    offers
        .iter()
        .filter(|o| o.can_purchase && o.type_id != 0 && o.price + keep <= balance)
        .filter(|o| {
            table
                .skins
                .skin_of(o.type_id)
                .is_some_and(|s| table.skins.fits(s, hull))
        })
        .min_by_key(|o| (o.price, o.id))
}

/// The extractor offer, if the store sells one.
pub fn extractor_offer(offers: &[Offer]) -> Option<&Offer> {
    offers
        .iter()
        .find(|o| o.type_id == TYPE_EXTRACTOR && o.can_purchase)
}

/// **Is an injector worth its price** to a bot making `isk_per_hour`? It
/// saves `points / SP_PER_HOUR` hours of training, which the bot would have
/// spent earning; it pays up to [`INJECT_MARGIN`] of that.
pub fn inject_worth(price: f64, isk_per_hour: f64, points: i64) -> bool {
    isk_per_hour > 0.0
        && price > 0.0
        && price <= isk_per_hour * (points as f64 / SP_PER_HOUR) * INJECT_MARGIN
}

/// Whether a bot may extract: old, nothing in the queue, and enough points
/// that the server allows it (`MIN_TO_EXTRACT`).
pub fn may_extract(total_sp: i64, queue_empty: bool) -> bool {
    queue_empty && total_sp >= OLD_SP.max(MIN_TO_EXTRACT)
}

/// One skill, as the extraction plan sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skill {
    pub points: i64,
    pub rank: i64,
}

/// **Which points to take** (pure): exactly [`BUCKET`], from the skills that
/// have the most to spare, none in `queued`, and never a skill below the
/// level a skill the bot holds (or has queued) still needs of it
/// (`requires(skill)` gives `(prerequisite, level)`). `None` when the
/// spare points do not reach a bucket. The server re-checks every rule.
pub fn extraction_plan(
    skills: &BTreeMap<i64, Skill>,
    queued: &BTreeSet<i64>,
    requires: impl Fn(i64) -> Vec<(i64, i64)>,
) -> Option<Vec<(i64, i64)>> {
    // The floor of each skill: the points of the highest level any held
    // or queued skill needs of it.
    let mut floor: BTreeMap<i64, i64> = BTreeMap::new();
    for &d in skills.keys().chain(queued.iter()) {
        for (need, level) in requires(d) {
            if let Some(s) = skills.get(&need) {
                let f = sp_for_level(s.rank, level.min(eve_server::skill::MAX_LEVEL));
                let e = floor.entry(need).or_insert(0);
                *e = (*e).max(f);
            }
        }
    }
    let mut spare: Vec<(i64, i64)> = skills
        .iter()
        .filter(|(t, _)| !queued.contains(t))
        .map(|(&t, s)| (t, s.points - floor.get(&t).copied().unwrap_or(0)))
        .filter(|(_, p)| *p > 0)
        .collect();
    spare.sort_by_key(|&(t, p)| (std::cmp::Reverse(p), t));
    let mut left = BUCKET;
    let mut out = Vec::new();
    for (t, p) in spare {
        if left == 0 {
            break;
        }
        let take = p.min(left);
        out.push((t, take));
        left -= take;
    }
    (left == 0).then_some(out)
}

/// **Is a turn due?** (pure) Docked, with a table to act on, and either
/// [`RECHECK`] since the last or a geode aboard.
pub fn due_at(checked: Option<Instant>, now: Instant, geode_aboard: bool, has_table: bool) -> bool {
    has_table
        && (geode_aboard || checked.is_none_or(|t| now.saturating_duration_since(t) >= RECHECK))
}

/// The brain's docked hook: a turn when [`due_at`] says so.
pub fn due(
    m: &mut crate::brain::Memory,
    o: &crate::brain::Observation,
) -> Option<Vec<crate::brain::Act>> {
    let table = m.policy.econ.clone();
    let has_table = table.has_geodes() || !table.skins.is_empty();
    let aboard = o.cargo.iter().any(|r| table.tier_of(r.type_id).is_some());
    if !due_at(m.econ.checked, o.now, aboard, has_table) {
        return None;
    }
    m.econ.checked = Some(o.now);
    Some(vec![crate::brain::Act::Econ])
}

// ---- the turn ---------------------------------------------------------------------

/// What the executor needs from the driver.
pub struct Ctx<'a> {
    pub table: &'a Table,
    pub content: Option<&'a eve_server::content::Content>,
    pub map: Option<&'a Map>,
    /// A trader speculates in PLEX.
    pub trader: bool,
    pub desk: &'a mut Desk,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A transport fault ends the turn; any other error is a verdict to note.
fn soft<T>(r: Result<T>, notes: &mut Vec<String>, what: &str) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => {
            notes.push(format!("{what}: {e}"));
            Ok(None)
        }
    }
}

fn call(conn: &mut Conn, service: &str, method: &str, args: Vec<PyValue>) -> Result<PyValue> {
    let out = conn.call(service, method, args)?;
    if let Some(key) = out.refusal() {
        return Err(Error::Refused(format!("{service}.{method}: {key}")));
    }
    Ok(out.value().unwrap_or(PyValue::None))
}

/// The vault's balance in whole PLEX (`vaultManager.get_account_balance`).
pub fn vault_balance(conn: &mut Conn) -> Result<i64> {
    match call(
        conn,
        "vaultManager",
        "get_account_balance",
        vec![eve_server::pyutil::bstr(CURRENCY)],
    )? {
        PyValue::Int(n) => Ok(n),
        PyValue::Real(r) => Ok(r as i64),
        _ => Ok(0),
    }
}

/// Everything the bot can open or use: the hangar and the ship's hold.
fn holdings(conn: &mut Conn) -> Result<Vec<ItemRow>> {
    let mut rows = conn.hangar_items()?;
    if let Some(ship) = conn.session().ship_id()
        && let Ok(cargo) = conn.ship_items(ship, Some(crate::verbs::FLAG_CARGO))
    {
        rows.extend(cargo);
    }
    Ok(rows)
}

fn open_geodes(
    conn: &mut Conn,
    table: &Table,
    st: &mut State,
    notes: &mut Vec<String>,
) -> Result<()> {
    for r in holdings(conn)? {
        if table.tier_of(r.type_id).is_none() || r.quantity < 1 {
            continue;
        }
        let Some(v) = soft(
            call(
                conn,
                "crateService",
                "ClaimLootFromCrateStack",
                vec![
                    PyValue::Int(r.item_id),
                    PyValue::Int(r.quantity.min(OPEN_AT_ONCE)),
                ],
            ),
            notes,
            "open geode",
        )?
        else {
            continue;
        };
        let opened = match v {
            PyValue::Tuple(t) => int_of(t.first()).unwrap_or(0),
            _ => 0,
        };
        st.opened += opened.max(0) as u64;
        notes.push(format!("opened {opened} geode(s) of type {}", r.type_id));
    }
    Ok(())
}

/// Buy offer `offer`, claim every redeem token it left, and say what the
/// hangar gained (the token list is read back, not assumed).
fn buy_and_claim(conn: &mut Conn, offer: &Offer) -> Result<()> {
    let kw: Vec<(PyValue, PyValue)> = Vec::new();
    let out = conn.call_kw(
        "storeManager",
        "buy_offer",
        vec![
            PyValue::Int(offer.id),
            eve_server::pyutil::bstr(CURRENCY),
            PyValue::Int(1),
        ],
        kw,
    )?;
    match out.value() {
        Some(PyValue::Bool(false)) | None => {
            return Err(Error::Refused(format!(
                "buy_offer {} answered {:?}",
                offer.id,
                out.refusal()
            )));
        }
        _ => {}
    }
    claim_tokens(conn)
}

fn claim_tokens(conn: &mut Conn) -> Result<()> {
    let tokens = call(conn, "userSvc", "GetRedeemTokens", vec![])?;
    let PyValue::List(list) = tokens else {
        return Ok(());
    };
    let ids: Vec<PyValue> = list
        .iter()
        .filter_map(|t| int_of(acquire::field(t, "tokenID", 0)))
        .map(|id| PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let me = conn.session().char_id().unwrap_or(0);
    call(
        conn,
        "userSvc",
        "ClaimRedeemTokens",
        vec![PyValue::List(ids), PyValue::Int(me), PyValue::Bool(true)],
    )?;
    Ok(())
}

/// Activate every SKIN licence in the hangar and apply one that fits the ship
/// flown, once per ship. Returns what it applied.
fn wear(conn: &mut Conn, table: &Table, st: &mut State, notes: &mut Vec<String>) -> Result<()> {
    let Some(ship) = conn.session().ship_id() else {
        return Ok(());
    };
    let rows = conn.hangar_items()?;
    let Some(hull) = rows.iter().find(|r| r.item_id == ship).map(|r| r.type_id) else {
        return Ok(());
    };
    let licences: Vec<&ItemRow> = rows
        .iter()
        .filter(|r| table.skins.licence(r.type_id).is_some())
        .collect();
    if licences.is_empty() {
        return Ok(());
    }
    let ids = PyValue::List(licences.iter().map(|r| PyValue::Int(r.item_id)).collect());
    if soft(
        call(conn, "shipCosmeticsMgr", "ActivateSkinLicense", vec![ids]),
        notes,
        "activate SKIN",
    )?
    .is_none()
    {
        return Ok(());
    }
    if st.skinned.contains(&ship) {
        return Ok(());
    }
    let fitting = licences
        .iter()
        .filter_map(|r| table.skins.skin_of(r.type_id))
        .find(|&s| table.skins.fits(s, hull));
    if let Some(skin) = fitting
        && soft(
            call(
                conn,
                "shipCosmeticsMgr",
                "ApplySkinToShip",
                vec![PyValue::Int(ship), PyValue::Int(skin)],
            ),
            notes,
            "apply SKIN",
        )?
        .is_some()
    {
        st.skinned.insert(ship);
        notes.push(format!("wearing SKIN {skin} on ship {ship}"));
    }
    Ok(())
}

/// The skills as the server lists them: `(handler, points by type)`.
fn read_skills(conn: &mut Conn) -> Result<(crate::ObjectRef, BTreeMap<i64, Skill>)> {
    let handler = acquire::skill_handler(conn)?;
    let out = conn.call_on(&handler, "GetSkills", vec![], vec![])?;
    let Some(PyValue::Dict(entries)) = out.value() else {
        return Err(Error::Refused("GetSkills: not a dict".to_owned()));
    };
    let skills = entries
        .iter()
        .filter_map(|(entry, key)| {
            let PyValue::Int(t) = key else { return None };
            Some((
                *t,
                Skill {
                    points: int_of(acquire::field(entry, "_skillPoints", 0))?,
                    rank: int_of(acquire::field(entry, "_skillRank", 0)).unwrap_or(1),
                },
            ))
        })
        .collect();
    Ok((handler, skills))
}

/// Sell `units` of `type_id` from the hangar into the best standing bid at
/// this station, at `min` or better. Returns the units offered.
fn sell_into_bid(conn: &mut Conn, type_id: i64, units: i64, min: f64) -> Result<i64> {
    let station = conn
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("sell: not docked".to_owned()))?;
    let mut left = units;
    let mut lines = Vec::new();
    for r in conn
        .hangar_items()?
        .into_iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
    {
        if left <= 0 {
            break;
        }
        let q = r.quantity.min(left);
        left -= q;
        lines.push(SellLine {
            station_id: station,
            type_id,
            item_id: r.item_id,
            price: min,
            quantity: q,
        });
    }
    if lines.is_empty() {
        return Ok(0);
    }
    let n = lines.iter().map(|l| l.quantity).sum();
    conn.sell(&lines, 0, crate::builder::BROKER_FEE)?;
    Ok(n)
}

/// The book of `type_id`, or nothing when the market will not say (noted).
fn book_of(
    conn: &mut Conn,
    type_id: i64,
    notes: &mut Vec<String>,
) -> Result<Option<crate::verbs::Book>> {
    soft(conn.market_orders(type_id), notes, "market")
}

fn best_bid_here(book: &crate::verbs::Book, station: i64) -> Option<f64> {
    book.bids
        .iter()
        .filter(|b| b.station_id == station && b.vol_remaining > 0)
        .map(|b| b.price)
        .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))))
}

fn best_ask_here(book: &crate::verbs::Book, station: i64) -> Option<f64> {
    book.asks
        .iter()
        .filter(|a| a.station_id == station && a.vol_remaining > 0 && !a.bid)
        .map(|a| a.price)
        .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.min(p))))
}

/// Inject what is held (young), or sell what is held (old); buy one when it
/// is worth the hours.
fn injectors(
    conn: &mut Conn,
    st: &mut State,
    total_sp: i64,
    handler: &crate::ObjectRef,
    wallet: f64,
    now: Instant,
    notes: &mut Vec<String>,
) -> Result<()> {
    let station = conn.session().station_id().unwrap_or(0);
    let old = total_sp >= OLD_SP;
    let held: Vec<ItemRow> = holdings(conn)?
        .into_iter()
        .filter(|r| r.type_id == TYPE_LARGE || r.type_id == TYPE_SMALL)
        .collect();
    if !old {
        let mut points = 0;
        for r in &held {
            if soft(
                conn.call_on(
                    handler,
                    "InjectSkillpoints",
                    vec![PyValue::Int(r.item_id), PyValue::Int(r.quantity)],
                    vec![],
                )
                .and_then(|o| match o.refusal() {
                    Some(k) => Err(Error::Refused(k)),
                    None => Ok(()),
                }),
                notes,
                "inject",
            )?
            .is_some()
            {
                points += r.quantity
                    * if r.type_id == TYPE_LARGE {
                        BUCKET
                    } else {
                        BUCKET / 5
                    };
            }
        }
        if points > 0 {
            let spent = acquire::apply_free_to_queue(conn)?;
            notes.push(format!("injected about {points} points, {spent} applied"));
            return Ok(());
        }
        // Nothing held: buy one when the ask is under what the hours are worth.
        if !st.backed_off("buy-injector", now) && st.isk_per_hour() > 0.0 {
            let book = book_of(conn, TYPE_LARGE, notes)?;
            if let Some(ask) = book.as_ref().and_then(|b| best_ask_here(b, station))
                && inject_worth(ask, st.isk_per_hour(), BUCKET)
                && ask <= wallet * INJECT_WALLET
            {
                match conn.buy(station, TYPE_LARGE, ask, 1, 0, crate::builder::BROKER_FEE) {
                    Ok(row) if row.vol_remaining < 1 => {
                        st.spent += ask;
                        notes.push(format!("bought an injector at {ask:.0}"));
                    }
                    Ok(_) => st.back_off("buy-injector", now),
                    Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
                    Err(e) => {
                        st.back_off("buy-injector", now);
                        notes.push(format!("buy injector: {e}"));
                    }
                }
            } else {
                st.back_off("buy-injector", now);
            }
        }
    } else if !held.is_empty() {
        for r in &held {
            let book = book_of(conn, r.type_id, notes)?;
            if let Some(bid) = book.as_ref().and_then(|b| best_bid_here(b, station))
                && let Some(n) = soft(
                    sell_into_bid(conn, r.type_id, r.quantity, bid),
                    notes,
                    "sell injector",
                )?
                && n > 0
            {
                notes.push(format!("sold {n} injector(s) at {bid:.0}"));
            }
        }
    }
    Ok(())
}

/// An old bot with an idle queue: extractor, extraction, sale.
#[allow(clippy::too_many_arguments)]
fn extract(
    conn: &mut Conn,
    cx: &Ctx<'_>,
    st: &mut State,
    skills: &BTreeMap<i64, Skill>,
    handler: &crate::ObjectRef,
    offers: &[Offer],
    balance: i64,
    now: Instant,
    notes: &mut Vec<String>,
) -> Result<()> {
    let total: i64 = skills.values().map(|s| s.points).sum();
    let queue = acquire::skill_queue(conn)?;
    if !may_extract(total, queue.is_empty()) || st.backed_off("extract", now) {
        return Ok(());
    }
    let Some(content) = cx.content else {
        return Ok(());
    };
    let queued: BTreeSet<i64> = queue.iter().map(|q| q.0).collect();
    let Some(take) = extraction_plan(skills, &queued, |s| required_in(content, s).to_vec()) else {
        st.back_off("extract", now);
        return Ok(());
    };
    // The extractor: held, or bought with PLEX the bot can spare.
    let mut have = holdings(conn)?
        .into_iter()
        .find(|r| r.type_id == TYPE_EXTRACTOR);
    if have.is_none()
        && let Some(o) = extractor_offer(offers).filter(|o| o.price + plex_trade::KEEP <= balance)
        && soft(buy_and_claim(conn, o), notes, "buy extractor")?.is_some()
    {
        have = holdings(conn)?
            .into_iter()
            .find(|r| r.type_id == TYPE_EXTRACTOR);
    }
    let Some(extractor) = have else {
        st.back_off("extract", now);
        return Ok(());
    };
    let dict = PyValue::Dict(
        take.iter()
            .map(|&(t, p)| (PyValue::Int(p), PyValue::Int(t)))
            .collect(),
    );
    let done = soft(
        conn.call_on(
            handler,
            "ExtractSkills",
            vec![dict, PyValue::Int(extractor.item_id), PyValue::None],
            vec![],
        )
        .and_then(|o| match o.refusal() {
            Some(k) => Err(Error::Refused(k)),
            None => Ok(()),
        }),
        notes,
        "extract",
    )?;
    if done.is_none() {
        st.back_off("extract", now);
        return Ok(());
    }
    notes.push(format!(
        "extracted {BUCKET} points from {} skill(s)",
        take.len()
    ));
    let station = conn.session().station_id().unwrap_or(0);
    let book = book_of(conn, TYPE_LARGE, notes)?;
    if let Some(bid) = book.as_ref().and_then(|b| best_bid_here(b, station))
        && let Some(n) = soft(
            sell_into_bid(conn, TYPE_LARGE, 1, bid),
            notes,
            "sell injector",
        )?
        && n > 0
    {
        notes.push(format!("sold the injector at {bid:.0}"));
    }
    Ok(())
}

/// PLEX on the market: a trader speculates, anyone sells a vault surplus.
#[allow(clippy::too_many_arguments)]
fn plex_market(
    conn: &mut Conn,
    cx: &mut Ctx<'_>,
    st: &mut State,
    balance: i64,
    wish: i64,
    wallet: f64,
    now: Instant,
    notes: &mut Vec<String>,
) -> Result<()> {
    let station = conn.session().station_id().unwrap_or(0);
    let Some(region) = cx.map.and_then(|m| m.region_of_station(station)) else {
        return Ok(());
    };
    let Some(book) = book_of(conn, eve_server::nes_vault::TYPE_PLEX, notes)? else {
        return Ok(());
    };
    let day = cx.desk.today();
    cx.desk
        .memory
        .record(region, eve_server::nes_vault::TYPE_PLEX, &book, day);
    let (ask, bid) = (best_ask_here(&book, station), best_bid_here(&book, station));
    if let Some(v) = bid.or(ask) {
        cx.desk
            .memory
            .observe(region, eve_server::nes_vault::TYPE_PLEX, v, day);
    }
    cx.desk.touch();
    let anchor = cx
        .desk
        .memory
        .anchor(region, eve_server::nes_vault::TYPE_PLEX)
        .copied();
    let held: i64 = conn
        .hangar_items()?
        .iter()
        .filter(|r| r.type_id == eve_server::nes_vault::TYPE_PLEX)
        .map(|r| r.quantity)
        .sum();
    // A trader's lots first.
    if cx.trader {
        match plex_trade::decide(anchor.as_ref(), ask, bid, held, wallet) {
            Move::Buy { units, price } => {
                if let Some(row) = soft(
                    conn.buy(
                        station,
                        eve_server::nes_vault::TYPE_PLEX,
                        price,
                        units,
                        0,
                        crate::builder::BROKER_FEE,
                    ),
                    notes,
                    "buy PLEX",
                )? {
                    let got = units - row.vol_remaining.clamp(0, units);
                    st.spent += got as f64 * price;
                    notes.push(format!("bought {got} PLEX under the anchor at {price:.0}"));
                }
            }
            Move::Sell { units, price } => {
                if let Some(n) = soft(
                    sell_into_bid(conn, eve_server::nes_vault::TYPE_PLEX, units, price),
                    notes,
                    "sell PLEX",
                )? {
                    notes.push(format!("sold {n} PLEX over the anchor at {price:.0}"));
                }
            }
            Move::Hold => {}
        }
    }
    // Surplus in the vault, for everyone.
    let extra = plex_trade::surplus(balance, wish);
    if let Move::Sell { units, price } =
        plex_trade::dump(anchor.as_ref(), bid, extra.min(plex_trade::LOT * 3))
        && !st.backed_off("sell-plex", now)
    {
        let hangar = conn.open_hangar()?;
        let me = conn.session().station_id().unwrap_or(0);
        let moved = soft(
            conn.call_on(
                &hangar,
                "WithdrawPlexFromVault",
                vec![
                    PyValue::Int(units),
                    PyValue::Int(me),
                    PyValue::Int(0),
                    PyValue::Int(crate::verbs::FLAG_HANGAR),
                    PyValue::None,
                ],
                vec![],
            )
            .and_then(|o| match o.refusal() {
                Some(k) => Err(Error::Refused(k)),
                None => Ok(()),
            }),
            notes,
            "withdraw PLEX",
        )?;
        if moved.is_none() {
            st.back_off("sell-plex", now);
            return Ok(());
        }
        if let Some(n) = soft(
            sell_into_bid(conn, eve_server::nes_vault::TYPE_PLEX, units, price),
            notes,
            "sell surplus PLEX",
        )? {
            notes.push(format!("sold {n} surplus PLEX at {price:.0}"));
        }
    }
    Ok(())
}

/// **One turn** (module header). A transport fault is the only error; every
/// refusal is a line in the note.
pub fn turn(conn: &mut Conn, st: &mut State, cx: &mut Ctx<'_>) -> Result<String> {
    if conn.session().station_id().is_none() {
        return Err(Error::NotSent("econ: not docked".to_owned()));
    }
    let now = Instant::now();
    let mut notes: Vec<String> = Vec::new();
    let wallet = conn.cash_balance()?;
    st.income(wallet, unix_now());

    open_geodes(conn, cx.table, st, &mut notes)?;
    wear(conn, cx.table, st, &mut notes)?;

    let balance = vault_balance(conn)?;
    let offers = match soft(
        call(
            conn,
            "storeManager",
            "get_offers",
            vec![PyValue::Int(STORE_ID)],
        ),
        &mut notes,
        "store",
    )? {
        Some(v) => offers_of(&v),
        None => Vec::new(),
    };

    // A SKIN for the hull flown, when the vault allows.
    if let Some(ship) = conn.session().ship_id()
        && !st.skinned.contains(&ship)
        && !st.backed_off("buy-skin", now)
        && let Some(hull) = conn
            .hangar_items()?
            .iter()
            .find(|r| r.item_id == ship)
            .map(|r| r.type_id)
    {
        match skin_offer(cx.table, &offers, hull, balance, SKIN_KEEP) {
            Some(o) => {
                if soft(buy_and_claim(conn, o), &mut notes, "buy SKIN")?.is_some() {
                    notes.push(format!("bought SKIN offer {} for {} PLEX", o.id, o.price));
                    wear(conn, cx.table, st, &mut notes)?;
                } else {
                    st.back_off("buy-skin", now);
                }
            }
            None => st.back_off("buy-skin", now),
        }
    }

    // Skill trading, when the skill handler answers.
    if let Some((handler, skills)) = soft(read_skills(conn), &mut notes, "skills")? {
        let total: i64 = skills.values().map(|s| s.points).sum();
        injectors(conn, st, total, &handler, wallet, now, &mut notes)?;
        let balance = vault_balance(conn)?;
        extract(
            conn, cx, st, &skills, &handler, &offers, balance, now, &mut notes,
        )?;
    }

    // PLEX on the market.
    let balance = vault_balance(conn)?;
    plex_market(
        conn,
        cx,
        st,
        balance,
        WISH_EXTRACTOR,
        wallet,
        now,
        &mut notes,
    )?;

    Ok(if notes.is_empty() {
        "econ: nothing to do".to_owned()
    } else {
        format!("econ: {}", notes.join("; "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: i64, price: i64, type_id: i64) -> Offer {
        Offer {
            id,
            price,
            type_id,
            can_purchase: true,
        }
    }

    fn skin_table() -> Table {
        let lic = [
            "990001\t991\t10\t0\t0\t0.01\tLicence A".to_owned(),
            "990002\t991\t11\t0\t0\t0.01\tLicence B".to_owned(),
        ];
        let skn = ["10\t1\t0\t4000".to_owned(), "11\t1\t0\t5000".to_owned()];
        Table::with_skins(eve_server::skins::Skins::parse(&lic, &skn).expect("fixture rows"))
    }

    #[test]
    fn the_cheapest_skin_that_fits_the_hull_is_the_one_bought() {
        let t = skin_table();
        let offers = [
            offer(1, 80, 990_001),
            offer(2, 50, 990_002),
            offer(3, 20, 5),
        ];
        let hull_a = 4000;
        // 990001 fits hull 4000; 990002 fits 5000; type 5 is no licence.
        assert_eq!(
            skin_offer(&t, &offers, hull_a, 500, 30).map(|o| o.id),
            Some(1)
        );
        assert_eq!(
            skin_offer(&t, &offers, 5000, 500, 30).map(|o| o.id),
            Some(2)
        );
        assert_eq!(
            skin_offer(&t, &offers, 7, 500, 30),
            None,
            "no SKIN for that hull"
        );
    }

    #[test]
    fn a_skin_is_bought_only_with_plex_to_spare() {
        let t = skin_table();
        let offers = [offer(1, 80, 990_001)];
        assert!(skin_offer(&t, &offers, 4000, 109, 30).is_none());
        assert!(skin_offer(&t, &offers, 4000, 110, 30).is_some());
        let mut o = offers[0].clone();
        o.can_purchase = false;
        assert!(
            skin_offer(&t, &[o], 4000, 999, 30).is_none(),
            "a greyed offer"
        );
    }

    #[test]
    fn an_injector_is_worth_its_price_by_the_hours_it_saves() {
        // 500 000 points at 1 800 an hour is 277 h; half of the income of
        // that is the most the bot pays.
        let cap = 2_000_000.0 * (BUCKET as f64 / SP_PER_HOUR) * INJECT_MARGIN;
        assert!(inject_worth(cap * 0.99, 2_000_000.0, BUCKET));
        assert!(!inject_worth(cap * 1.01, 2_000_000.0, BUCKET));
        assert!(
            !inject_worth(1.0, 0.0, BUCKET),
            "a bot with no income buys none"
        );
        assert!(!inject_worth(0.0, 5e6, BUCKET));
    }

    #[test]
    fn only_an_old_bot_with_an_idle_queue_extracts() {
        assert!(may_extract(OLD_SP, true));
        assert!(!may_extract(OLD_SP - 1, true));
        assert!(!may_extract(OLD_SP, false));
    }

    fn sk(points: i64) -> Skill {
        Skill { points, rank: 1 }
    }

    #[test]
    fn the_extraction_takes_exactly_a_bucket_from_the_skills_with_most_to_spare() {
        let skills: BTreeMap<i64, Skill> =
            [(1, sk(300_000)), (2, sk(900_000)), (3, sk(100_000))].into();
        let plan = extraction_plan(&skills, &BTreeSet::new(), |_| vec![]).expect("a plan");
        assert_eq!(plan.iter().map(|p| p.1).sum::<i64>(), BUCKET);
        assert_eq!(plan[0], (2, 500_000));
        assert_eq!(plan.len(), 1);
        // A bucket needs the pilot's points to reach it.
        let thin: BTreeMap<i64, Skill> = [(1, sk(100_000)), (2, sk(200_000))].into();
        assert_eq!(extraction_plan(&thin, &BTreeSet::new(), |_| vec![]), None);
    }

    #[test]
    fn a_queued_skill_and_a_prerequisite_in_use_are_not_touched() {
        // Skill 2 is needed at level 3 by skill 1; rank 1 level 3 is 2 829 points.
        let skills: BTreeMap<i64, Skill> = [(1, sk(900_000)), (2, sk(600_000))].into();
        let need = |s: i64| if s == 1 { vec![(2, 3)] } else { vec![] };
        let plan = extraction_plan(&skills, &BTreeSet::new(), need).expect("a plan");
        let from_two: i64 = plan.iter().filter(|p| p.0 == 2).map(|p| p.1).sum();
        let floor = sp_for_level(1, 3);
        assert!(600_000 - from_two >= floor, "skill 2 stays at level 3");
        assert_eq!(plan.iter().map(|p| p.1).sum::<i64>(), BUCKET);
        // A queued skill is not a source.
        let queued: BTreeSet<i64> = [1].into();
        let plan = extraction_plan(&skills, &queued, |_| vec![]).expect("a plan from 2 alone");
        assert!(plan.iter().all(|p| p.0 != 1));
    }

    #[test]
    fn a_turn_is_due_after_the_recheck_or_with_a_geode_aboard() {
        let now = Instant::now();
        assert!(due_at(None, now, false, true));
        assert!(!due_at(Some(now), now, false, true));
        assert!(
            due_at(Some(now), now, true, true),
            "a geode aboard is opened at once"
        );
        assert!(due_at(Some(now), now + RECHECK, false, true));
        assert!(
            !due_at(None, now, true, false),
            "no table, nothing to act on"
        );
    }

    #[test]
    fn the_income_estimate_adds_back_what_was_spent() {
        let mut s = State::default();
        s.income(1_000_000.0, 1_000);
        assert_eq!(s.isk_per_hour(), 0.0);
        s.spent = 500_000.0;
        // Wallet up 1 M and 0.5 M spent in an hour: 1.5 M an hour.
        s.income(2_000_000.0, 1_000 + 3_600);
        assert!((s.isk_per_hour() - 1_500_000.0).abs() < 1.0);
    }

    #[test]
    fn a_refused_step_waits_out_its_backoff() {
        let mut s = State::default();
        let now = Instant::now();
        assert!(!s.backed_off("extract", now));
        s.back_off("extract", now);
        assert!(s.backed_off("extract", now + Duration::from_secs(60)));
        assert!(!s.backed_off("extract", now + BACKOFF));
    }

    #[test]
    fn offers_are_read_from_the_stores_answer_and_a_placeholder_has_none() {
        use eve_server::pyutil::{bstr, pydict};
        let offer = pydict(vec![
            ("id", PyValue::Int(77)),
            (
                "offerPricings",
                PyValue::List(vec![pydict(vec![("price", PyValue::Int(50))])]),
            ),
            (
                "products",
                PyValue::List(vec![pydict(vec![("typeId", PyValue::Int(990_001))])]),
            ),
            ("canPurchase", PyValue::Bool(true)),
            ("name", bstr("x")),
        ]);
        let got = offers_of(&PyValue::List(vec![offer]));
        assert_eq!(
            got,
            vec![Offer {
                id: 77,
                price: 50,
                type_id: 990_001,
                can_purchase: true
            }]
        );
        let placeholder = PyValue::List(vec![pydict(vec![("products", PyValue::List(vec![]))])]);
        assert!(offers_of(&placeholder).is_empty());
    }
}
