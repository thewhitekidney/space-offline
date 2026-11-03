//! **DOC-6 (merged with FE-SRP): the corporation's doctrine stock, its
//! ship replacement programme, its staging and its announcement**
//! (R-DOCTRINE 8.3, R-FLUIDECON FE-SRP; docs/lanes/doc-6.md).
//!
//! # U2, settled: a corporation cannot place a buy order
//!
//! The client offers "use corporation wallet" on a buy only to a pilot with
//! the accountant or trader role, and sends it as `useCorp`, the eighth of
//! `PlaceBuyOrder`'s nine arguments (`buyThisTypeWindow.py:319-327`,
//! `marketsvc.py:244-254`) **[V]**. The server reads the first seven and
//! charges the *pilot* (`service/market.rs::place_buy_order`; `isCorp` is
//! discarded in every push) **[V]**. So a bid is always a member's, and the
//! corp pays by **funding** a procurement member: the CEO gives the member
//! ISK from the Master Wallet (`account.GiveCashFromCorpAccount`, the call
//! `corp_ops::run_ceo` pays with), the member bids personally through
//! [`crate::demand::post`] with no more than that float, and what he buys he
//! puts in the corp hangar's [`DOCTRINE_DIVISION`]. The socket test
//! `corp_stock_socket` pins both halves: a `useCorp = True` bid is charged to
//! the pilot, and a funded member's bids come out of the float.
//!
//! # CORP-BUY: since then the server honours `useCorp`
//!
//! `service/market.rs` now escrows a `useCorp` order from the corporation's
//! active division, journals it there and delivers what fills to the
//! corporation's deliveries hangar (`service/market/corp.rs`). So a
//! procurement member the CEO has granted the accountant and trader roles
//! ([`authorise`]) bids with `useCorp` through [`crate::demand::post_as`],
//! against [`stock_budget`] of the Master Wallet, with no float. The float
//! remains the fallback when the grant is refused (the case above).
//!
//! # What the corp keeps
//!
//! - **The stock** ([`stock_sets`], [`stock_needs`]): hull sets of the
//!   corp's doctrine, `STOCK_DAYS x expected losses a day + the fighters who
//!   own no doctrine ship`, the hulls, modules and rounds of each role's
//!   share of them. The expected losses are the leader book's decayed loss
//!   count, floored at 1 a day for the group.
//! - **The float** ([`fund_amount`]): the CEO funds the procurement member
//!   with what the shortfall costs, never more than [`STOCK_WALLET_SHARE`] of
//!   the Master Wallet above the rent reserve, and the member returns what
//!   the stock no longer needs.
//! - **Reships** ([`kit_draw`]): a fighter with no doctrine hull draws the
//!   kit of his role's fit out of the division before he buys one.
//! - **SRP** ([`srp_verdict`], [`srp_batch`]): a member who loses a doctrine
//!   hull in a system the corp's leader has ordered a theatre to is paid
//!   [`SRP_SHARE`] of the fit's cost from the Master Wallet, never more than
//!   the killmail's `iskLost`, never twice for one kill, and never past
//!   [`SRP_CAP_DAY`] of the wallet a game day or into the rent reserve. The
//!   kill is read from `corpRegistry.GetRecentLosses` (the Kill Reports tab's
//!   call, open to every member) **[V]**, not taken on the member's word.
//! - **Staging** ([`staging_system`]): the system of the leader's busiest
//!   open theatre, else home; the CEO mails the doctrine and the staging to
//!   the corp when either changes ([`crate::mail::doctrine_text`]).
//!
//! Every constant is **[A]** with its falsifier; no test uses the value of
//! one, only its name.

use crate::corp_ops;
use crate::corpmind::{Hall, HallState};
use crate::demand::{Kind, Need};
use crate::doctrine::{Doctrine, RoleFit};
use crate::doctrine_hall::is_fighter;
use crate::doctrine_theatre::{Ship, State};
use crate::pricing::Prices;
use crate::verbs::ItemRow;
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;
use eve_server::group_contacts::Affiliation;
use std::collections::{BTreeMap, BTreeSet};

/// The hangar division of the doctrine's hulls, modules and rounds: index 2,
/// "division 3" (the builders' is [`corp_ops::BUILDER_DIVISION`], 1).
pub const DOCTRINE_DIVISION: usize = 2;
/// Days of expected losses the stock covers **[A]**. Falsified by members
/// who lose a ship and find the division empty (too low), or a division
/// holding more than a week of unused sets (too high).
pub const STOCK_DAYS: f64 = 2.0;
/// The floor of the group's expected losses a day **[A]** (R-DOCTRINE 8.3).
pub const EXPECTED_FLOOR: f64 = 1.0;
/// The share of the Master Wallet above the rent reserve that may sit in
/// the procurement float **[A]**. Falsified by a wallet that cannot pay the
/// alliance fee or SRP (too high) or a stock that never fills (too low).
pub const STOCK_WALLET_SHARE: f64 = 0.3;
/// A float under this is not moved, ISK **[A]**: dust.
pub const FLOAT_MIN: f64 = 10_000.0;
/// The share of a doctrine fit's cost SRP pays **[A]** (R-DOCTRINE 8.2).
/// Falsified by members still on non-doctrine hulls after a week with SRP
/// on (too low), or a corp wallet bleeding to zero (too high).
pub const SRP_SHARE: f64 = 0.7;
/// The share of the Master Wallet SRP may pay out a game day **[A]**.
pub const SRP_CAP_DAY: f64 = 0.15;
/// A loss older than this, game days, is no longer paid **[A]**. Falsified
/// by honest members whose loss is read later than this.
pub const SRP_WINDOW_DAYS: f64 = 2.0;
/// A theatre's system stays "ordered" this long after the leader last held
/// it open, game days **[A]**: the fight that follows an order.
pub const SRP_ORDER_GRACE_DAYS: f64 = 1.0;
/// Paid kills remembered (and persisted), oldest dropped.
pub const SRP_KEEP: usize = 512;
/// The CEO looks at SRP this often, game days **[A]**.
pub const SRP_EVERY_DAYS: f64 = 0.25;
/// The CEO funds, stages and announces this often, game days **[A]**.
pub const STAGE_EVERY_DAYS: f64 = 0.5;
/// The CEO mails the doctrine at most this often, game days **[A]**: a
/// theatre that flaps does not spam the corp.
pub const ANNOUNCE_MIN_DAYS: f64 = 1.0;
/// Most kills read a pass: the Kill Reports tab's own page.
pub const LOSSES_READ: i64 = 25;

/// What the hall keeps for this lane. Not persisted except [`Self::paid`]:
/// a restart forgets the float (the ISK is the member's, nothing is lost),
/// the ordered systems (SRP waits for the next order) and the announcement
/// (one more mail).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StockState {
    /// Kills SRP paid (persisted, bounded by [`SRP_KEEP`]).
    pub paid: BTreeSet<i64>,
    /// `corp -> (game day number, ISK paid that day)`.
    pub paid_today: BTreeMap<i64, (u64, f64)>,
    /// Systems a leader group has held an open theatre in, with the unix
    /// second the order lapses: `(group, system) -> until`.
    pub ordered: BTreeMap<(i64, i64), u64>,
    /// The procurement member's unspent float, by member.
    pub float: BTreeMap<i64, f64>,
    /// CORP-BUY: procurement members who hold the accountant and trader
    /// roles, and so bid from the corporation's own wallet (`useCorp`) with
    /// no float. Re-derived by the CEO's next staging pass after a restart.
    pub direct: BTreeSet<i64>,
    /// REVIEW-FIX: the share (0..=1) of his role's kit a member drew free
    /// from the division since his last SRP verdict. A loss of a hull the
    /// corp already handed him is paid net of it. Not persisted.
    pub kit_share: BTreeMap<i64, f64>,
    /// Units of each type the division held when the member last looked:
    /// `corp -> type -> units`.
    pub held: BTreeMap<i64, BTreeMap<i64, i64>>,
    /// The staging system each CEO last chose.
    pub staging: BTreeMap<i64, i64>,
    /// `corp -> (doctrine key, staging system, when)` last announced.
    pub announced: BTreeMap<i64, (String, Option<i64>, u64)>,
    pub srp_tried: BTreeMap<i64, u64>,
    pub stage_tried: BTreeMap<i64, u64>,
}

// ---- the stock (pure) ---------------------------------------------------------------

/// **The group's expected losses a day** (pure): the leader book's loss
/// count `lost` (decayed with `half_life_days`), as a steady rate, floored at
/// [`EXPECTED_FLOOR`]. A count that decays with half-life `h` after a
/// steady rate `r` settles at `r h / ln 2`.
pub fn expected_losses_per_day(lost: f64, half_life_days: f64) -> f64 {
    let rate = lost.max(0.0) * std::f64::consts::LN_2 / half_life_days.max(1e-9);
    rate.max(EXPECTED_FLOOR)
}

/// **How many hull sets to keep** (pure): [`STOCK_DAYS`] of the expected
/// losses `per_day` for these fighters, plus a set for each of the `fighters`
/// who own no doctrine ship (`owned_share`, `0..=1`). Whole sets, rounded up.
pub fn stock_sets(fighters: usize, per_day: f64, owned_share: f64) -> i64 {
    let unowned = fighters as f64 * (1.0 - owned_share.clamp(0.0, 1.0));
    (STOCK_DAYS * per_day.max(0.0) + unowned).ceil() as i64
}

/// **The parts to hold for `sets` ships of `d`** (pure): each role's share of
/// the ships, rounded up, times its hull, modules and rounds, less what
/// `held` already has. Hulls first, then modules, then rounds; one need
/// per type.
pub fn stock_needs(d: &Doctrine, sets: i64, held: &dyn Fn(i64) -> i64) -> Vec<Need> {
    if sets <= 0 {
        return Vec::new();
    }
    let mut want: BTreeMap<(u8, i64), i64> = BTreeMap::new();
    for (_, share, fit) in &d.roles {
        let ships = (sets as f64 * share).ceil() as i64;
        if ships <= 0 {
            continue;
        }
        *want.entry((0, fit.hull)).or_default() += ships;
        for &(t, n) in &fit.modules {
            *want.entry((1, t)).or_default() += ships * i64::from(n);
        }
        for &(t, n) in &fit.charges {
            *want.entry((2, t)).or_default() += ships * i64::from(n);
        }
    }
    want.into_iter()
        .filter_map(|((rank, t), n)| {
            let short = n - held(t);
            (short > 0).then_some(Need {
                type_id: t,
                units: short,
                kind: match rank {
                    0 => Kind::Hull,
                    1 => Kind::Module,
                    _ => Kind::Ammo,
                },
            })
        })
        .collect()
}

/// **What the CEO may put in the float** (pure): [`STOCK_WALLET_SHARE`] of
/// what the Master Wallet holds above `reserve`.
pub fn stock_budget(master: f64, reserve: f64, share: f64) -> f64 {
    share * (master - reserve).max(0.0)
}

/// REVIEW-FIX: the ISK the open bids hold in escrow (pure): price times units
/// remaining of each bid (the order's own escrow is not on the wire).
pub fn open_bid_escrow(open: &[crate::verbs::OwnOrder]) -> f64 {
    open.iter()
        .filter(|o| o.order.bid)
        .map(|o| o.order.price * o.order.vol_remaining.max(0) as f64)
        .sum()
}

/// REVIEW-FIX: **the direct-mode budget** (pure): [`STOCK_WALLET_SHARE`] of
/// the Master Wallet *plus the escrow it already lent out* above `reserve`,
/// less that escrow; never below 0.
pub fn procure_budget(master: f64, escrow: f64, reserve: f64, share: f64) -> f64 {
    let escrow = escrow.max(0.0);
    (stock_budget(master + escrow, reserve, share) - escrow).max(0.0)
}

/// **The ISK to give the procurement member** (pure): the shortfall's `cost`
/// less the `float` he already holds, inside `budget` less that float, whole
/// ISK; nothing under [`FLOAT_MIN`].
pub fn fund_amount(cost: f64, float: f64, budget: f64) -> f64 {
    let room = (budget - float).max(0.0);
    let isk = (cost - float).max(0.0).min(room).floor();
    if isk < FLOAT_MIN { 0.0 } else { isk }
}

/// **The ISK to take back** (pure): a float with nothing left to buy returns
/// whole when it is over [`FLOAT_MIN`].
pub fn float_return(float: f64, shortfall_units: i64) -> f64 {
    if shortfall_units <= 0 && float >= FLOAT_MIN {
        float.floor()
    } else {
        0.0
    }
}

/// **The procurement member** (pure): of the corp's active members, the
/// trader with the lowest id, else the courier, else the builder. `None`
/// with none of them.
pub fn procurement_member(members: &[(i64, &str)]) -> Option<i64> {
    ["trader", "courier", "builder"].iter().find_map(|career| {
        members
            .iter()
            .filter(|(_, c)| c == career)
            .map(|(id, _)| *id)
            .min()
    })
}

/// **What a fighter draws from the division** (pure): the hull when he holds
/// none, the fit's modules and rounds he lacks, each only as far as the
/// division has it. `(type, units)`.
pub fn kit_draw(
    fit: &RoleFit,
    mine: &dyn Fn(i64) -> i64,
    division: &dyn Fn(i64) -> i64,
) -> Vec<(i64, i64)> {
    let mut want: Vec<(i64, i64)> = vec![(fit.hull, 1)];
    want.extend(fit.modules.iter().map(|&(t, n)| (t, i64::from(n))));
    want.extend(fit.charges.iter().map(|&(t, n)| (t, i64::from(n))));
    want.into_iter()
        .filter_map(|(t, n)| {
            let take = (n - mine(t)).min(division(t));
            (take > 0).then_some((t, take))
        })
        .collect()
}

// ---- SRP (pure) -----------------------------------------------------------------------

/// A loss as the corp's Kill Reports list it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loss {
    pub kill_id: i64,
    pub victim: i64,
    pub hull: i64,
    pub system: i64,
    pub isk: f64,
    /// Seconds since the kill.
    pub age_s: u64,
    /// REVIEW-FIX: the final blow's pilot (0: none, an NPC), corporation and
    /// alliance, and the victim's corporation and alliance (0: none): what
    /// "hostile" is judged from.
    pub killer: i64,
    pub killer_corp: i64,
    pub killer_alliance: i64,
    pub victim_corp: i64,
    pub victim_alliance: i64,
}

/// What SRP knows when it judges a loss.
pub struct SrpCtx<'a> {
    pub paid: &'a BTreeSet<i64>,
    /// An active member of the corp?
    pub member: &'a dyn Fn(i64) -> bool,
    /// The cost of the doctrine fit built on this hull, if a standing
    /// doctrine has one.
    pub fit_cost: &'a dyn Fn(i64) -> Option<f64>,
    /// A system a theatre was ordered to.
    pub ordered: &'a dyn Fn(i64) -> bool,
    pub window_s: u64,
    /// REVIEW-FIX2: the share of the fit's cost SRP pays (the difficulty
    /// knob `SRP_SHARE`).
    pub share: f64,
    /// REVIEW-FIX: was this loss to a hostile pilot (not an NPC, not a
    /// friend)?
    pub hostile: &'a dyn Fn(&Loss) -> bool,
    /// REVIEW-FIX: the ISK this loss already returned: the insurance payout
    /// and the kit drawn free from the division.
    pub recovered: &'a dyn Fn(&Loss) -> f64,
}

/// Why a loss is not paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Paid,
    NotMember,
    NotDoctrineHull,
    NotOrdered,
    TooOld,
    NoValue,
    /// REVIEW-FIX: an NPC's kill, or a friend's: no replacement paid.
    NotHostile,
    /// REVIEW-FIX: insurance and the free kit already covered it.
    Recovered,
}

/// **Is this loss paid, and how much** (pure): verified against the
/// killmail's own fields. Only a loss to a hostile pilot is paid, and net of
/// the insurance and free kit that came back. The amount is [`SRP_SHARE`] of the fit's cost,
/// never more than the ISK the killmail says was lost, in whole cents.
pub fn srp_verdict(l: &Loss, ctx: &SrpCtx<'_>) -> std::result::Result<f64, Refusal> {
    if ctx.paid.contains(&l.kill_id) {
        return Err(Refusal::Paid);
    }
    if !(ctx.member)(l.victim) {
        return Err(Refusal::NotMember);
    }
    let Some(cost) = (ctx.fit_cost)(l.hull) else {
        return Err(Refusal::NotDoctrineHull);
    };
    if !(ctx.ordered)(l.system) {
        return Err(Refusal::NotOrdered);
    }
    if l.age_s > ctx.window_s {
        return Err(Refusal::TooOld);
    }
    if !(ctx.hostile)(l) {
        return Err(Refusal::NotHostile);
    }
    if (ctx.share * cost).is_nan() || ctx.share * cost <= 0.0 || l.isk.is_nan() || l.isk <= 0.0 {
        return Err(Refusal::NoValue);
    }
    // What the corp owes is its share of the fit less what insurance and the
    // free kit already put back; the killmail's own value still caps it.
    let isk = (ctx.share * cost - (ctx.recovered)(l).max(0.0)).min(l.isk);
    if isk.is_nan() || isk < 0.01 {
        return Err(Refusal::Recovered);
    }
    let isk = (isk * 100.0).floor() / 100.0;
    Ok(isk)
}

/// **The day's room for SRP** (pure): [`SRP_CAP_DAY`] of the Master Wallet
/// less what was paid today, and never into the rent `reserve`.
pub fn srp_room(master: f64, paid_today: f64, reserve: f64) -> f64 {
    (SRP_CAP_DAY * master - paid_today)
        .min(master - reserve)
        .max(0.0)
}

/// One payout: `(kill, the member, ISK)`.
pub type Payout = (i64, i64, f64);

/// **Who is paid what** (pure): the verified losses, oldest kill first, each
/// while `room` lasts; a payout that does not fit waits for another day. The
/// refusals are returned too, for the log.
pub fn srp_batch(
    losses: &[Loss],
    ctx: &SrpCtx<'_>,
    mut room: f64,
) -> (Vec<Payout>, Vec<(i64, Refusal)>) {
    let mut sorted: Vec<&Loss> = losses.iter().collect();
    sorted.sort_by_key(|l| l.kill_id);
    let mut seen = BTreeSet::new();
    let (mut pay, mut no) = (Vec::new(), Vec::new());
    for l in sorted {
        if !seen.insert(l.kill_id) {
            continue;
        }
        match srp_verdict(l, ctx) {
            Ok(isk) if isk <= room => {
                room -= isk;
                pay.push((l.kill_id, l.victim, isk));
            }
            Ok(_) => {}
            Err(why) => no.push((l.kill_id, why)),
        }
    }
    (pay, no)
}

// ---- staging (pure) -----------------------------------------------------------------

/// **Where the corp stages** (pure): the system of the open theatre with the
/// most commitment steps (ties: the lowest system id), else `home`.
/// `theatres` are `(system, state, steps)`; a theatre that is only watched or
/// is being given up does not pull the staging.
pub fn staging_system(theatres: &[(i64, State, u32)], home: Option<i64>) -> Option<i64> {
    theatres
        .iter()
        .filter(|(_, s, _)| matches!(s, State::Commit(_) | State::Hold | State::Surge))
        .max_by_key(|(sys, _, steps)| (*steps, std::cmp::Reverse(*sys)))
        .map(|(sys, _, _)| *sys)
        .or(home)
}

/// Does a theatre in this state count as an order SRP honours?
pub fn is_order(s: State) -> bool {
    !matches!(s, State::Watch)
}

// ---- the hall's side ----------------------------------------------------------------

/// What the stock needs of one corp, from the hall.
#[derive(Debug, Clone, PartialEq)]
pub struct StockPlan {
    pub key: String,
    pub doctrine: Doctrine,
    pub sets: i64,
}

impl HallState {
    /// The group whose book governs `corp`: its alliance when the alliance
    /// has a book, else the corp.
    pub fn leader_group(&self, corp: i64) -> i64 {
        match self.corps.get(&corp).and_then(|c| c.alliance) {
            Some(al) if self.dbooks.contains_key(&al) => al,
            _ => corp,
        }
    }

    /// The doctrine a corp stocks, and how many sets (module header).
    pub fn stock_plan(&self, corp: i64, now: u64) -> Option<StockPlan> {
        let world = self.doctrine.as_ref()?;
        let key = self.doctrine_key_of(corp)?.to_owned();
        let doctrine = world.catalog.get(world.find(&key)?)?.clone();
        let group = self.leader_group(corp);
        let book = self.dbooks.get(&group)?;
        let mine: Vec<i64> = self
            .members
            .iter()
            .filter(|(_, m)| m.corp == corp && is_fighter(&m.career))
            .map(|(c, _)| *c)
            .collect();
        let all = self
            .members
            .values()
            .filter(|m| is_fighter(&m.career) && self.leader_group(m.corp) == group)
            .count()
            .max(1);
        let n = mine.len().min(doctrine.fleet.target);
        let owned = mine
            .iter()
            .filter(|c| {
                self.readiness
                    .get(c)
                    .is_some_and(|r| r.ship == Some(Ship::Staged))
            })
            .count();
        let owned_share = if mine.is_empty() {
            1.0
        } else {
            owned as f64 / mine.len() as f64
        };
        let half = crate::doctrine_lead::LOSS_HALF_LIFE_DAYS;
        let age_days = now.saturating_sub(book.losses.at_s) as f64 / self.day.max(1) as f64;
        let lost = book.losses.lost * 0.5_f64.powf(age_days / half);
        let group_rate = expected_losses_per_day(lost, half);
        let corp_rate = group_rate * mine.len() as f64 / all as f64;
        let sets = stock_sets(n, corp_rate, owned_share);
        Some(StockPlan {
            key,
            doctrine,
            sets,
        })
    }

    /// `hull -> cost` of every fit of the doctrines a corp may fly now (the
    /// current one and the one just left).
    pub fn srp_fits(&self, corp: i64, now: u64) -> BTreeMap<i64, f64> {
        let mut out = BTreeMap::new();
        let (Some(world), Some(book)) = (
            self.doctrine.as_ref(),
            self.dbooks.get(&self.leader_group(corp)),
        ) else {
            return out;
        };
        for (key, _) in book.standing(now) {
            if let Some(d) = world.find(key).and_then(|i| world.catalog.get(i)) {
                for (_, _, fit) in &d.roles {
                    out.insert(fit.hull, fit.cost);
                }
            }
        }
        out
    }

    /// Note the systems the leader of `corp` holds an open theatre in, so a
    /// loss there is on an order; returns the staging system.
    pub fn note_orders(&mut self, corp: i64, now: u64, home: Option<i64>) -> Option<i64> {
        let group = self.leader_group(corp);
        let grace = (SRP_ORDER_GRACE_DAYS * self.day as f64) as u64;
        let theatres: Vec<(i64, State, u32)> = self
            .theatres
            .get(&group)
            .into_iter()
            .flatten()
            .map(|t| (t.system, t.state, t.steps))
            .collect();
        for (sys, st, _) in &theatres {
            if is_order(*st) {
                self.stock.ordered.insert((group, *sys), now + grace);
            }
        }
        self.stock.ordered.retain(|_, until| *until >= now);
        staging_system(&theatres, home)
    }

    fn is_ordered(&self, group: i64, system: i64, now: u64) -> bool {
        self.stock
            .ordered
            .get(&(group, system))
            .is_some_and(|until| *until >= now)
    }

    /// Does `me` (a fighter with no doctrine hull) draw a kit this turn?
    pub fn draws_kit(&self, me: i64, career: &str) -> bool {
        is_fighter(career)
            && self.doctrine.is_some()
            && self
                .readiness
                .get(&me)
                .is_some_and(|r| r.ship == Some(Ship::Market))
    }
}

// ---- the calls --------------------------------------------------------------------------

fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

fn master_wallet(conn: &mut Conn) -> Result<Option<f64>> {
    let a = soft(conn.call_kw(
        "account",
        "GetCashBalance",
        vec![PyValue::Int(1)],
        vec![kw("accountKey", PyValue::Int(cw::MASTER_KEY))],
    ))?;
    Ok(a.ok().and_then(|o| match o.value() {
        Some(PyValue::Real(r)) => Some(r),
        Some(PyValue::Int(n)) => Some(n as f64),
        _ => None,
    }))
}

fn units_of(rows: &[ItemRow], type_id: i64) -> i64 {
    rows.iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
        .map(|r| r.quantity)
        .sum()
}

use corp_ops::office_here;

/// **The CEO's staging pass** ([`crate::corpmind::Plan::Stage`]): note the
/// orders, choose the staging system, mail the doctrine and the staging when
/// either changed, and fund the procurement member.
pub fn run_stage(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    corp: i64,
    now: u64,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let home = conn
        .session()
        .get_i64("solarsystemid2")
        .or_else(|| conn.session().get_i64("solarsystemid"));
    let (plan, staging, announce) = {
        let mut st = hall.lock();
        let staging = st.note_orders(corp, now, home);
        let plan = st.stock_plan(corp, now);
        if let Some(s) = staging {
            st.stock.staging.insert(corp, s);
        }
        let wait = (ANNOUNCE_MIN_DAYS * st.day as f64) as u64;
        let announce = plan.as_ref().is_some_and(|p| {
            st.stock.announced.get(&corp).is_none_or(|(k, s, at)| {
                (k != &p.key || *s != staging) && now.saturating_sub(*at) >= wait
            })
        });
        (plan, staging, announce)
    };
    let Some(plan) = plan else {
        return Ok(out);
    };
    if announce {
        let fit = plan.doctrine.line().cloned();
        let line = crate::mail::doctrine(conn, hall, corp, &plan.key, fit.as_ref(), staging)?;
        hall.lock()
            .stock
            .announced
            .insert(corp, (plan.key.clone(), staging, now));
        out.extend(line);
    }
    out.extend(fund(conn, hall, me, corp, &plan)?);
    Ok(out)
}

/// **Fund the procurement member** from the Master Wallet (U2's fallback).
fn fund(conn: &mut Conn, hall: &Hall, me: i64, corp: i64, plan: &StockPlan) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let (member, float, held) = {
        let st = hall.lock();
        let recent = (corp_ops::ACTIVE_DAYS * st.day as f64) as u64;
        let newest = st
            .members
            .values()
            .filter_map(|m| m.last_turn)
            .max()
            .unwrap_or(0);
        let cands: Vec<(i64, &str)> = st
            .members
            .iter()
            .filter(|(id, m)| {
                **id != me
                    && m.corp == corp
                    && m.last_turn
                        .is_some_and(|t| newest.saturating_sub(t) <= recent)
            })
            .map(|(id, m)| (*id, m.career.as_str()))
            .collect();
        let member = procurement_member(&cands);
        (
            member,
            member
                .and_then(|m| st.stock.float.get(&m).copied())
                .unwrap_or(0.0),
            st.stock.held.get(&corp).cloned().unwrap_or_default(),
        )
    };
    revoke_stale(conn, hall, me, corp, member, &mut out)?;
    let Some(member) = member else {
        return Ok(out);
    };
    if authorise(conn, hall, me, corp, member, &mut out)? {
        return Ok(out);
    }
    let needs = stock_needs(&plan.doctrine, plan.sets, &|t| {
        held.get(&t).copied().unwrap_or(0)
    });
    let Some(master) = master_wallet(conn)? else {
        return Ok(out);
    };
    let mut prices = match soft(Prices::read(conn))? {
        Ok(p) => p,
        Err(_) => return Ok(out),
    };
    let mut cost = 0.0;
    for n in &needs {
        if let Some(p) = soft(prices.reservation(conn, n.type_id))?.ok().flatten() {
            cost += p * n.units as f64;
        }
    }
    let share = hall.lock().knobs.lead.stock_wallet;
    let budget = stock_budget(master, corp_ops::pay_reserve(), share);
    let isk = fund_amount(cost, float, budget);
    if isk > 0.0 {
        let a = conn.call_kw(
            "account",
            "GiveCashFromCorpAccount",
            vec![
                PyValue::Int(member),
                PyValue::Real(isk),
                PyValue::Int(cw::MASTER_KEY),
            ],
            vec![kw("reason", PyValue::Unicode("Doctrine stock".to_owned()))],
        );
        match soft(a)? {
            Ok(o) if o.refusal().is_none() => {
                let mut st = hall.lock();
                *st.stock.float.entry(member).or_default() += isk;
                st.act(format!(
                    "{me} funded {member} with {isk:.0} ISK of corp {corp}'s doctrine stock"
                ));
                out.push(format!("fund {member} {isk:.0}: ok"));
            }
            Ok(o) => out.push(format!("fund {member}: refused {:?}", o.refusal())),
            Err(why) => out.push(format!("fund {member}: {why}")),
        }
    }
    Ok(out)
}

/// The roles the client offers the corporation-wallet box to, and the
/// server's `useCorp` check: accountant (also the Corporation Orders tab the
/// bids are read back through) and trader **[V]**.
pub const PROCURE_ROLES: i64 = cw::ROLE_ACCOUNTANT | cw::ROLE_TRADER;

/// REVIEW-FIX: the members of `corp` that hold procurement roles
/// (`direct`) but are no longer the procurement member `keep` (pure).
pub fn stale_procurers(
    direct: &BTreeSet<i64>,
    corp_of: &dyn Fn(i64) -> Option<i64>,
    corp: i64,
    keep: Option<i64>,
) -> Vec<i64> {
    direct
        .iter()
        .copied()
        .filter(|m| Some(*m) != keep && corp_of(*m) == Some(corp))
        .collect()
}

/// REVIEW-FIX: `roles` without [`PROCURE_ROLES`], `None` when none are held.
pub fn without_procure(roles: i64) -> Option<i64> {
    (roles & PROCURE_ROLES != 0).then_some(roles & !PROCURE_ROLES)
}

/// REVIEW-FIX: **take the procurement roles back** from a member who is no
/// longer the procurement member (a job change, a doctrine that ended): the
/// accountant and trader roles reach the corporation's wallet, so they do not
/// outlive the job. A refusal leaves him in `direct` for the next pass.
fn revoke_stale(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    corp: i64,
    keep: Option<i64>,
    out: &mut Vec<String>,
) -> Result<()> {
    let stale = {
        let st = hall.lock();
        stale_procurers(
            &st.stock.direct,
            &|m| st.members.get(&m).map(|x| x.corp),
            corp,
            keep,
        )
    };
    if stale.is_empty() {
        return Ok(());
    }
    let rows = match soft(corp_ops::corp_call(
        conn,
        corp,
        "GetMembersPaged",
        vec![PyValue::Int(1)],
    ))? {
        Ok(a) => a
            .value
            .as_ref()
            .map(corp_ops::member_roles)
            .unwrap_or_default(),
        Err(_) => return Ok(()),
    };
    for member in stale {
        let held = rows.iter().find(|(id, _)| *id == member).map(|r| r.1);
        // Left the corp, or already without: nothing to take back.
        let Some(new) = held.and_then(without_procure) else {
            hall.lock().stock.direct.remove(&member);
            continue;
        };
        let mut args = vec![
            PyValue::Int(member),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Int(new),
        ];
        args.extend(std::iter::repeat_n(PyValue::None, 10));
        match soft(corp_ops::corp_call(conn, corp, "UpdateMember", args))? {
            Ok(o) if o.refusal().is_none() => {
                let mut st = hall.lock();
                st.stock.direct.remove(&member);
                st.act(format!(
                    "{me} took the procurement roles back from {member} in corp {corp}"
                ));
                out.push(format!("procurement roles of {member} revoked: ok"));
            }
            _ => out.push(format!("procurement roles of {member}: revoke refused")),
        }
    }
    Ok(())
}

/// **Let the procurement member bid from the corp wallet** (CORP-BUY): true
/// when he holds [`PROCURE_ROLES`] (granted now when he lacks them), so
/// there is nothing to fund; false when the CEO cannot grant them and the
/// float must do.
fn authorise(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    corp: i64,
    member: i64,
    out: &mut Vec<String>,
) -> Result<bool> {
    if hall.lock().stock.direct.contains(&member) {
        return Ok(true);
    }
    let rows = match soft(corp_ops::corp_call(
        conn,
        corp,
        "GetMembersPaged",
        vec![PyValue::Int(1)],
    ))? {
        Ok(a) => a
            .value
            .as_ref()
            .map(corp_ops::member_roles)
            .unwrap_or_default(),
        Err(_) => return Ok(false),
    };
    let Some(roles) = rows.iter().find(|(id, _)| *id == member).map(|r| r.1) else {
        return Ok(false);
    };
    if roles & PROCURE_ROLES != PROCURE_ROLES {
        let mut args = vec![
            PyValue::Int(member),
            PyValue::None,
            PyValue::None,
            PyValue::None,
            PyValue::Int(roles | PROCURE_ROLES),
        ];
        args.extend(std::iter::repeat_n(PyValue::None, 10));
        let a = soft(corp_ops::corp_call(conn, corp, "UpdateMember", args))?;
        match a {
            Ok(o) if o.refusal().is_none() => {
                out.push(format!("procurement roles of {member}: ok"));
            }
            _ => return Ok(false),
        }
    }
    let mut st = hall.lock();
    st.stock.direct.insert(member);
    st.act(format!(
        "{me} let {member} bid from corp {corp}'s wallet for its doctrine stock"
    ));
    Ok(true)
}

/// **The procurement member's pass** (after [`corp_ops::run_member`]): put
/// the doctrine's parts in the division, bid for the shortfall with the
/// float, and return a float nothing needs. Docked at the corp's office.
pub fn run_procure(
    conn: &mut Conn,
    hall: &Hall,
    me: i64,
    corp: i64,
    now: u64,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let plan = {
        let st = hall.lock();
        if !st.stock.float.contains_key(&me) && !st.stock.direct.contains(&me) {
            return Ok(out);
        }
        match st.stock_plan(corp, now) {
            Some(p) => p,
            None => return Ok(out),
        }
    };
    let Some((station, office)) = office_here(conn)? else {
        return Ok(out);
    };
    let flag = co::FLAG_SAG[DOCTRINE_DIVISION];
    let parts = plan.doctrine.parts();
    // Deposit what the doctrine names.
    let hangar = conn.hangar_items()?;
    let inv = conn.open_container(office)?;
    for row in hangar
        .iter()
        .filter(|r| !r.singleton && parts.contains(&r.type_id))
    {
        let a = conn.call_on(
            &inv,
            "Add",
            vec![PyValue::Int(row.item_id), PyValue::Int(station)],
            vec![
                kw("qty", PyValue::Int(row.quantity)),
                kw("flag", PyValue::Int(flag)),
            ],
        )?;
        match a.refusal() {
            Some(key) => {
                out.push(format!("stock {}: refused {key}", row.type_id));
                break;
            }
            None => out.push(format!("stock {} of {}: ok", row.quantity, row.type_id)),
        }
    }
    let division = conn.list(&inv, Some(flag))?;
    let held: BTreeMap<i64, i64> = parts
        .iter()
        .map(|&t| (t, units_of(&division, t)))
        .filter(|(_, n)| *n > 0)
        .collect();
    hall.lock().stock.held.insert(corp, held.clone());
    let needs = stock_needs(&plan.doctrine, plan.sets, &|t| {
        held.get(&t).copied().unwrap_or(0)
    });
    let direct = hall.lock().stock.direct.contains(&me);
    let mut float = hall.lock().stock.float.get(&me).copied().unwrap_or(0.0);
    let short: i64 = needs.iter().map(|n| n.units).sum();
    let back = float_return(float, short);
    if back > 0.0 {
        let a = conn.call_kw(
            "account",
            "GiveCash",
            vec![
                PyValue::Int(corp),
                PyValue::Real(back),
                PyValue::Unicode("Doctrine stock float".to_owned()),
            ],
            vec![kw("toAccountKey", PyValue::Int(cw::MASTER_KEY))],
        );
        if let Ok(o) = soft(a)?
            && o.refusal().is_none()
        {
            float -= back;
            out.push(format!("float back {back:.0}: ok"));
        }
    } else if (float > 0.0 || direct) && !needs.is_empty() {
        let mut prices = match soft(Prices::read(conn))? {
            Ok(p) => p,
            Err(_) => return Ok(out),
        };
        // CORP-BUY: the corporation's wallet (its stock budget) or the float.
        let mut budget = if direct {
            let master = master_wallet(conn)?.unwrap_or(0.0);
            // REVIEW-FIX: the corp bids already resting hold ISK out of the
            // wallet; the 30 % is of the wallet *with* them, so repricing
            // upward over several passes cannot grow past it.
            let escrow = open_bid_escrow(&conn.corp_open_orders().unwrap_or_default());
            procure_budget(
                master,
                escrow,
                corp_ops::pay_reserve(),
                hall.lock().knobs.lead.stock_wallet,
            )
        } else {
            float
        };
        for n in &needs {
            // The budget is the whole wallet this bid may use ([`demand::post`]
            // spends `SPEND_SHARE` of what it is given).
            let wallet = budget / crate::hull_choice::SPEND_SHARE;
            let p = crate::demand::post_as(conn, &mut prices, n.type_id, n.units, wallet, direct)?;
            if p.placed > 0 {
                budget = (budget - p.placed as f64 * p.price).max(0.0);
                out.push(format!("bid {} of {}", p.placed, n.type_id));
            }
        }
        if !direct {
            float = budget;
        }
    }
    if !direct || float > 0.0 {
        hall.lock().stock.float.insert(me, float);
    }
    if !out.is_empty() {
        hall.lock()
            .act(format!("{me} tended corp {corp}'s doctrine stock"));
    }
    Ok(out)
}

/// **A fighter's reship draw**: the kit of his role's fit out of the
/// division, before he buys any of it. Docked at the corp's office.
pub fn run_draw(conn: &mut Conn, hall: &Hall, me: i64, corp: i64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let Some(fit) = hall.plan_for(me).map(|p| p.fit) else {
        return Ok(out);
    };
    let Some((_, office)) = office_here(conn)? else {
        return Ok(out);
    };
    let flag = co::FLAG_SAG[DOCTRINE_DIVISION];
    let inv = conn.open_container(office)?;
    let division = conn.list(&inv, Some(flag))?;
    let mine = conn.hangar_items()?;
    let draw = kit_draw(&fit, &|t| units_of(&mine, t), &|t| units_of(&division, t));
    let hangar = conn.open_hangar()?;
    let mut drawn = 0i64;
    for (type_id, mut n) in draw {
        let mut stacks: Vec<&ItemRow> = division
            .iter()
            .filter(|r| r.type_id == type_id && !r.singleton)
            .collect();
        stacks.sort_by_key(|r| std::cmp::Reverse(r.quantity));
        for s in stacks {
            if n <= 0 {
                break;
            }
            let q = s.quantity.min(n);
            let a = conn.call_on(
                &hangar,
                "Add",
                vec![PyValue::Int(s.item_id), PyValue::Int(office)],
                vec![
                    kw("qty", PyValue::Int(q)),
                    kw("flag", PyValue::Int(eve_server::model::FLAG_HANGAR)),
                ],
            )?;
            if let Some(key) = a.refusal() {
                out.push(format!("draw {type_id}: refused {key}"));
                return Ok(out);
            }
            n -= q;
            drawn += q;
            out.push(format!("draw {q} of {type_id}: ok"));
        }
    }
    if drawn > 0 {
        // REVIEW-FIX: what share of his kit came free, for SRP to net out.
        let total = 1
            + fit.modules.iter().map(|&(_, n)| i64::from(n)).sum::<i64>()
            + fit.charges.iter().map(|&(_, n)| i64::from(n)).sum::<i64>();
        let share = kit_share_of(drawn, total);
        let mut st = hall.lock();
        let e = st.stock.kit_share.entry(me).or_insert(0.0);
        *e = e.max(share);
    }
    if !out.is_empty() {
        hall.lock()
            .act(format!("{me} drew a kit from corp {corp}'s doctrine stock"));
    }
    Ok(out)
}

/// The losses of a `corpRegistry.GetRecentLosses` answer, as [`Loss`]es;
/// `now_filetime` is the clock the kill times are read against.
pub fn losses_of(rows: &[PyValue], now_filetime: i64) -> Vec<Loss> {
    rows.iter()
        .filter_map(|r| {
            let kill_id = crate::observe::int(r, "killID")?;
            let time = crate::observe::int(r, "killTime").unwrap_or(now_filetime);
            Some(Loss {
                kill_id,
                victim: crate::observe::int(r, "victimCharacterID")?,
                hull: crate::observe::int(r, "victimShipTypeID")?,
                system: crate::observe::int(r, "solarSystemID")?,
                isk: crate::observe::real(r, "iskLost").unwrap_or(0.0),
                age_s: (now_filetime.saturating_sub(time).max(0) / 10_000_000) as u64,
                killer: crate::observe::int(r, "finalCharacterID").unwrap_or(0),
                killer_corp: crate::observe::int(r, "finalCorporationID").unwrap_or(0),
                killer_alliance: crate::observe::int(r, "finalAllianceID").unwrap_or(0),
                victim_corp: crate::observe::int(r, "victimCorporationID").unwrap_or(0),
                victim_alliance: crate::observe::int(r, "victimAllianceID").unwrap_or(0),
            })
        })
        .collect()
}

/// **The CEO's SRP pass** ([`crate::corpmind::Plan::Srp`]): read the corp's
/// losses, judge each, pay the verified ones from the Master Wallet.
/// REVIEW-FIX: the share of a kit of `total` units that `drawn` units were
/// (pure, `0..=1`).
pub fn kit_share_of(drawn: i64, total: i64) -> f64 {
    if total <= 0 {
        return 0.0;
    }
    (drawn.max(0) as f64 / total as f64).clamp(0.0, 1.0)
}

/// REVIEW-FIX: is a loss a hostile pilot's kill (pure over `hostile_to`)? An
/// NPC (no killing pilot) is not; nor is a pilot of the victim's own *player*
/// corporation (two starters in one NPC corporation are strangers); the rest
/// is the hall's own rule.
pub fn srp_is_hostile(l: &Loss, hostile_to: impl FnOnce() -> bool) -> bool {
    let own_corp = l.killer_corp == l.victim_corp
        && l.killer_corp != 0
        && !(1_000_000..=1_999_999).contains(&l.killer_corp);
    l.killer > 0 && !own_corp && hostile_to()
}

pub fn run_srp(conn: &mut Conn, hall: &Hall, me: i64, corp: i64, now: u64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let a = soft(conn.moniker_call(
        "corpRegistry",
        PyValue::Int(corp),
        "GetRecentLosses",
        vec![PyValue::Int(LOSSES_READ), PyValue::None],
        vec![],
    ))?;
    let rows = match a {
        Ok(a) => match a.value {
            Some(PyValue::List(rows)) => rows,
            _ => return Ok(out),
        },
        Err(why) => {
            out.push(format!("srp: {why}"));
            return Ok(out);
        }
    };
    let losses = losses_of(&rows, eve_server::pyutil::filetime_now());
    if losses.is_empty() {
        return Ok(out);
    }
    let Some(master) = master_wallet(conn)? else {
        return Ok(out);
    };
    // REVIEW-FIX: what insurance paid back is estimated from the insurer's
    // own quote for each lost hull (`GetInsurancePrice`) and the package the
    // victim's career buys ([`crate::insure::level_for`]) **[A]**.
    let mut full: BTreeMap<i64, f64> = BTreeMap::new();
    for l in &losses {
        if full.contains_key(&l.hull) {
            continue;
        }
        let price = soft(conn.call(
            "insuranceSvc",
            "GetInsurancePrice",
            vec![PyValue::Int(l.hull)],
        ))?
        .ok()
        .and_then(|o| match o.value() {
            Some(PyValue::Real(r)) => Some(r),
            Some(PyValue::Int(n)) => Some(n as f64),
            _ => None,
        })
        .unwrap_or(0.0);
        full.insert(l.hull, price);
    }
    let (pay, no, day_no) = {
        let st = hall.lock();
        let day_no = now / st.day.max(1);
        let fits = st.srp_fits(corp, now);
        let group = st.leader_group(corp);
        let recent = (corp_ops::ACTIVE_DAYS * st.day as f64) as u64;
        let seen: BTreeSet<i64> = st
            .members
            .iter()
            .filter(|(_, m)| {
                m.corp == corp && m.last_turn.is_some_and(|t| now.saturating_sub(t) <= recent)
            })
            .map(|(id, _)| *id)
            .collect();
        let today = match st.stock.paid_today.get(&corp) {
            Some((d, isk)) if *d == day_no => *isk,
            _ => 0.0,
        };
        let room = srp_room(master, today, corp_ops::pay_reserve());
        let ordered: BTreeSet<i64> = losses
            .iter()
            .map(|l| l.system)
            .filter(|s| st.is_ordered(group, *s, now))
            .collect();
        let ctx = SrpCtx {
            paid: &st.stock.paid,
            member: &|c| seen.contains(&c),
            fit_cost: &|h| fits.get(&h).copied(),
            ordered: &|s| ordered.contains(&s),
            window_s: (SRP_WINDOW_DAYS * st.day as f64) as u64,
            share: st.knobs.lead.srp_share,
            hostile: &|l| {
                let victim = Affiliation {
                    character_id: l.victim,
                    corporation_id: l.victim_corp,
                    alliance_id: Some(l.victim_alliance).filter(|a| *a > 0),
                };
                let killer = Affiliation {
                    character_id: l.killer,
                    corporation_id: l.killer_corp,
                    alliance_id: Some(l.killer_alliance).filter(|a| *a > 0),
                };
                srp_is_hostile(l, || st.hostile_to(victim, killer))
            },
            recovered: &|l| {
                let career = st.members.get(&l.victim).map_or("", |m| m.career.as_str());
                let insured =
                    crate::insure::level_for(career) * full.get(&l.hull).copied().unwrap_or(0.0);
                let kit = st.stock.kit_share.get(&l.victim).copied().unwrap_or(0.0);
                let cost = fits.get(&l.hull).copied().unwrap_or(0.0);
                insured + kit.clamp(0.0, 1.0) * st.knobs.lead.srp_share * cost
            },
        };
        let (pay, no) = srp_batch(&losses, &ctx, room);
        (pay, no, day_no)
    };
    for (id, why) in no {
        if why != Refusal::Paid {
            out.push(format!("srp kill {id}: {why:?}"));
        }
    }
    for (kill, victim, isk) in pay {
        let a = conn.call_kw(
            "account",
            "GiveCashFromCorpAccount",
            vec![
                PyValue::Int(victim),
                PyValue::Real(isk),
                PyValue::Int(cw::MASTER_KEY),
            ],
            vec![kw(
                "reason",
                PyValue::Unicode(format!("Ship replacement, kill {kill}")),
            )],
        );
        match soft(a)? {
            Ok(o) if o.refusal().is_none() => {
                let mut st = hall.lock();
                st.stock.paid.insert(kill);
                st.stock.kit_share.remove(&victim);
                while st.stock.paid.len() > SRP_KEEP {
                    st.stock.paid.pop_first();
                }
                let e = st.stock.paid_today.entry(corp).or_insert((day_no, 0.0));
                if e.0 != day_no {
                    *e = (day_no, 0.0);
                }
                e.1 += isk;
                st.dirty = true;
                st.act(format!(
                    "{me} paid {victim} {isk:.0} ISK of corp {corp}'s ship replacement (kill {kill})"
                ));
                out.push(format!("srp {victim} {isk:.0}: ok"));
            }
            Ok(o) => out.push(format!("srp {victim}: refused {:?}", o.refusal())),
            Err(why) => out.push(format!("srp {victim}: {why}")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::brick_doc;

    const HULL: i64 = 990_600_001;
    const GUN: i64 = 990_600_002;
    const ROUND: i64 = 990_600_003;
    const SYSTEM: i64 = 990_600_100;
    const KILLER: i64 = 990_600_200;
    const KILLER_CORP: i64 = 990_600_201;
    const VICTIM_CORP: i64 = 990_600_202;

    fn fit() -> RoleFit {
        let mut f = brick_doc().roles[0].2.clone();
        f.hull = HULL;
        f.modules = vec![(GUN, 2)];
        f.charges = vec![(ROUND, 10)];
        f.cost = 1_000_000.0;
        f
    }

    fn doctrine() -> Doctrine {
        let mut d = brick_doc();
        d.roles = vec![(crate::doctrine::Role::Dps, 1.0, fit())];
        d
    }

    #[test]
    fn stock_target_follows_expected_losses() {
        let half = crate::doctrine_lead::LOSS_HALF_LIFE_DAYS;
        // No losses: the floor.
        assert_eq!(expected_losses_per_day(0.0, half), EXPECTED_FLOOR);
        // More losses, a higher rate; a tiny count stays at the floor.
        let busy = expected_losses_per_day(30.0, half);
        assert!(busy > expected_losses_per_day(3.0, half));
        assert!(busy > EXPECTED_FLOOR);
        assert_eq!(expected_losses_per_day(0.1, half), EXPECTED_FLOOR);
        // The sets follow the rate, and fighters who own a ship need none.
        let calm = stock_sets(6, EXPECTED_FLOOR, 1.0);
        let war = stock_sets(6, busy, 1.0);
        assert!(war > calm);
        assert_eq!(calm, (STOCK_DAYS * EXPECTED_FLOOR).ceil() as i64);
        assert_eq!(
            stock_sets(6, EXPECTED_FLOOR, 0.0) - calm,
            6,
            "one set for each fighter with no ship"
        );
        assert_eq!(stock_sets(0, 0.0, 0.0), 0);
        // The parts: hulls, then modules, then rounds, less what is held.
        let d = doctrine();
        let needs = stock_needs(&d, 3, &|_| 0);
        let get = |t: i64| needs.iter().find(|n| n.type_id == t).map(|n| n.units);
        assert_eq!(get(HULL), Some(3));
        assert_eq!(get(GUN), Some(6));
        assert_eq!(get(ROUND), Some(30));
        assert_eq!(needs[0].kind, Kind::Hull);
        assert_eq!(needs.last().unwrap().kind, Kind::Ammo);
        let less = stock_needs(&d, 3, &|t| if t == HULL { 3 } else { 1 });
        assert!(less.iter().all(|n| n.type_id != HULL));
        assert_eq!(less.iter().find(|n| n.type_id == GUN).unwrap().units, 5);
        assert!(stock_needs(&d, 0, &|_| 0).is_empty());
    }

    fn loss(kill: i64, victim: i64, isk: f64) -> Loss {
        Loss {
            kill_id: kill,
            victim,
            hull: HULL,
            system: SYSTEM,
            isk,
            age_s: 10,
            killer: KILLER,
            killer_corp: KILLER_CORP,
            killer_alliance: 0,
            victim_corp: VICTIM_CORP,
            victim_alliance: 0,
        }
    }

    #[test]
    fn srp_is_capped_and_verified() {
        let paid: BTreeSet<i64> = [7].into_iter().collect();
        let member = |c: i64| c != 99;
        let cost = |h: i64| (h == HULL).then_some(1_000_000.0);
        let ordered = |s: i64| s == SYSTEM;
        let c = SrpCtx {
            paid: &paid,
            member: &member,
            fit_cost: &cost,
            ordered: &ordered,
            window_s: 1_000,
            share: SRP_SHARE,
            hostile: &|l| srp_is_hostile(l, || true),
            recovered: &|_| 0.0,
        };
        // The amount: a share of the fit's cost, and never above the loss.
        let ok = srp_verdict(&loss(1, 5, 5_000_000.0), &c).unwrap();
        assert_eq!(ok, SRP_SHARE * 1_000_000.0);
        let low = srp_verdict(&loss(2, 5, 100_000.0), &c).unwrap();
        assert_eq!(low, 100_000.0, "the killmail's own value caps it");
        // Each refusal.
        assert_eq!(srp_verdict(&loss(7, 5, 1e6), &c), Err(Refusal::Paid));
        assert_eq!(srp_verdict(&loss(3, 99, 1e6), &c), Err(Refusal::NotMember));
        let mut other = loss(4, 5, 1e6);
        other.hull = 1;
        assert_eq!(srp_verdict(&other, &c), Err(Refusal::NotDoctrineHull));
        let mut away = loss(5, 5, 1e6);
        away.system = SYSTEM + 1;
        assert_eq!(srp_verdict(&away, &c), Err(Refusal::NotOrdered));
        let mut old = loss(6, 5, 1e6);
        old.age_s = 1_001;
        assert_eq!(srp_verdict(&old, &c), Err(Refusal::TooOld));
        assert_eq!(srp_verdict(&loss(8, 5, 0.0), &c), Err(Refusal::NoValue));
        // The batch: oldest first, each kill once, inside the room.
        let each = SRP_SHARE * 1_000_000.0;
        let losses = vec![
            loss(12, 5, 1e7),
            loss(10, 5, 1e7),
            loss(10, 5, 1e7),
            loss(11, 6, 1e7),
        ];
        let (pay, _) = srp_batch(&losses, &c, 2.5 * each);
        assert_eq!(
            pay.iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![10, 11],
            "two fit, the third waits"
        );
        assert!(pay.iter().map(|p| p.2).sum::<f64>() <= 2.5 * each);
        let (none, _) = srp_batch(&losses, &c, 0.0);
        assert!(none.is_empty());
        // The room: a share of the wallet a day, never into the reserve.
        assert_eq!(srp_room(1e9, 0.0, 0.0), SRP_CAP_DAY * 1e9);
        assert_eq!(srp_room(1e9, SRP_CAP_DAY * 1e9, 0.0), 0.0);
        assert_eq!(srp_room(1e9, 0.0, 1e9 - 10.0), 10.0);
        assert_eq!(srp_room(5.0, 0.0, 100.0), 0.0);
    }

    #[test]
    fn a_corp_buy_uses_the_corp_wallet() {
        // The corp pays by funding: from the Master Wallet above the rent
        // reserve, a share of it at most, and only the shortfall.
        let reserve = corp_ops::pay_reserve();
        let master = reserve + 100_000_000.0;
        let budget = stock_budget(master, reserve, STOCK_WALLET_SHARE);
        assert_eq!(budget, STOCK_WALLET_SHARE * 100_000_000.0);
        assert_eq!(
            stock_budget(reserve / 2.0, reserve, STOCK_WALLET_SHARE),
            0.0
        );
        assert_eq!(fund_amount(1_000_000.0, 0.0, budget), 1_000_000.0);
        assert_eq!(
            fund_amount(1e12, 0.0, budget),
            budget,
            "never past the budget"
        );
        assert_eq!(fund_amount(1_000_000.0, 600_000.0, budget), 400_000.0);
        assert_eq!(
            fund_amount(1_000_000.0, 1_000_000.0, budget),
            0.0,
            "a float that covers it is not topped up"
        );
        assert_eq!(fund_amount(1e12, budget, budget), 0.0, "a full float");
        assert_eq!(fund_amount(FLOAT_MIN / 2.0, 0.0, budget), 0.0, "dust");
        // What nothing needs goes back whole.
        assert_eq!(float_return(500_000.0, 0), 500_000.0);
        assert_eq!(float_return(500_000.0, 3), 0.0);
        assert_eq!(float_return(1.0, 0), 0.0);
        // The buyer: a trader first, then a courier, then a builder.
        let m = [(5, "builder"), (3, "courier"), (9, "trader"), (4, "trader")];
        assert_eq!(procurement_member(&m), Some(4));
        assert_eq!(procurement_member(&m[..2]), Some(3));
        assert_eq!(procurement_member(&[(1, "hunter")]), None);
    }

    #[test]
    fn a_fighter_draws_only_what_the_division_has() {
        let f = fit();
        let division = |t: i64| match t {
            HULL | GUN => 1,
            _ => 0,
        };
        let draw = kit_draw(&f, &|_| 0, &division);
        assert_eq!(draw, vec![(HULL, 1), (GUN, 1)]);
        let have = |t: i64| i64::from(t == HULL);
        assert_eq!(kit_draw(&f, &have, &division), vec![(GUN, 1)]);
        assert!(kit_draw(&f, &|_| 99, &division).is_empty());
    }

    #[test]
    fn the_corp_stages_at_the_busiest_open_theatre_else_home() {
        let t = [
            (30, State::Watch, 9),
            (20, State::Commit(1), 1),
            (10, State::Surge, 3),
            (40, State::Disengage, 5),
        ];
        assert_eq!(staging_system(&t, Some(1)), Some(10));
        assert_eq!(staging_system(&t[..1], Some(1)), Some(1));
        assert_eq!(staging_system(&[], None), None);
        assert!(is_order(State::Hold) && !is_order(State::Watch));
    }

    #[test]
    fn losses_are_read_by_column() {
        let row = |kill: i64| {
            eve_server::pyutil::keyval(vec![
                ("killID", PyValue::Int(kill)),
                ("killTime", PyValue::Int(100 * 10_000_000)),
                ("victimCharacterID", PyValue::Int(5)),
                ("victimShipTypeID", PyValue::Int(HULL)),
                ("solarSystemID", PyValue::Int(SYSTEM)),
                ("iskLost", PyValue::Real(2.5)),
            ])
        };
        let got = losses_of(&[row(1), row(2)], 160 * 10_000_000);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].age_s, 60);
        assert_eq!((got[0].victim, got[0].hull, got[0].isk), (5, HULL, 2.5));
    }

    #[test]
    fn review_fix_the_budget_counts_the_escrow_already_out() {
        let reserve = 10_000.0;
        let master = 1_000_000.0;
        let first = procure_budget(master, 0.0, reserve, STOCK_WALLET_SHARE);
        assert_eq!(first, stock_budget(master, reserve, STOCK_WALLET_SHARE));
        // After a pass the bids hold `first` and the wallet shrank by it:
        // the same wallet-with-escrow gives the same ceiling, no new room.
        assert_eq!(
            procure_budget(master - first, first, reserve, STOCK_WALLET_SHARE),
            0.0
        );
        let half = first / 2.0;
        assert!(
            (procure_budget(master - half, half, reserve, STOCK_WALLET_SHARE) - half).abs() < 1e-6
        );
        // Escrow above the share leaves nothing, never a negative.
        assert_eq!(
            procure_budget(master, 10.0 * master, reserve, STOCK_WALLET_SHARE),
            0.0
        );
        let order = |price: f64, units: i64, bid: bool| crate::verbs::OwnOrder {
            order: crate::verbs::BookOrder {
                order_id: 1,
                type_id: 2,
                station_id: 3,
                price,
                vol_remaining: units,
                bid,
            },
            solar_system_id: 4,
            range: 0,
            issue_date: 0,
            last_state_change: None,
        };
        assert_eq!(
            open_bid_escrow(&[order(2.0, 5, true), order(9.0, 9, false)]),
            10.0
        );
    }

    #[test]
    fn review_fix_a_member_who_stops_being_procurement_loses_the_roles() {
        let direct: BTreeSet<i64> = [5, 6, 7].into_iter().collect();
        let corp_of = |m: i64| Some(if m == 7 { 2 } else { 1 });
        assert_eq!(stale_procurers(&direct, &corp_of, 1, Some(5)), vec![6]);
        assert_eq!(stale_procurers(&direct, &corp_of, 1, None), vec![5, 6]);
        let other = PROCURE_ROLES | 1 << 40;
        assert_eq!(without_procure(other), Some(1 << 40));
        assert_eq!(without_procure(1 << 40), None);
    }

    #[test]
    fn review_fix_srp_pays_hostile_kills_net_of_what_came_back() {
        let paid = BTreeSet::new();
        let cost = |h: i64| (h == HULL).then_some(1_000_000.0);
        let base = SRP_SHARE * 1_000_000.0;
        let run = |hostile: bool, back: f64, l: &Loss| {
            let member = |_: i64| true;
            let ordered = |_: i64| true;
            let c = SrpCtx {
                paid: &paid,
                member: &member,
                fit_cost: &cost,
                ordered: &ordered,
                window_s: 1_000,
                share: SRP_SHARE,
                hostile: &|l| srp_is_hostile(l, || hostile),
                recovered: &|_| back,
            };
            srp_verdict(l, &c)
        };
        let l = loss(1, 5, 5_000_000.0);
        assert_eq!(run(true, 0.0, &l), Ok(base));
        // Insurance and the free kit come off the top.
        assert_eq!(run(true, 0.4 * base, &l), Ok(base - 0.4 * base));
        assert_eq!(run(true, base, &l), Err(Refusal::Recovered));
        assert_eq!(run(true, 2.0 * base, &l), Err(Refusal::Recovered));
        // A friend's kill is not paid.
        assert_eq!(run(false, 0.0, &l), Err(Refusal::NotHostile));
        // Nor an NPC's: no killing pilot.
        let mut npc = loss(2, 5, 5_000_000.0);
        npc.killer = 0;
        assert_eq!(run(true, 0.0, &npc), Err(Refusal::NotHostile));
        // A pilot of an NPC corporation is a stranger, and counts.
        let mut starter = loss(3, 5, 5_000_000.0);
        starter.killer_corp = 1_000_125;
        starter.victim_corp = 1_000_125;
        assert_eq!(run(true, 0.0, &starter), Ok(base));
        // Nor one by his own corporation.
        let mut own = loss(4, 5, 5_000_000.0);
        own.killer_corp = own.victim_corp;
        assert_eq!(run(true, 0.0, &own), Err(Refusal::NotHostile));
        assert_eq!(kit_share_of(3, 6), 0.5);
        assert_eq!(kit_share_of(9, 6), 1.0);
        assert_eq!(kit_share_of(1, 0), 0.0);
    }
}
