//! **Spending LP** (X-LPSTORE): a bot that earned loyalty points (a mission
//! runner's agent LP, a plexer's militia LP) turns them into ISK the way a
//! player does -- pick the LP store offer that pays best, buy what it
//! takes, redeem it, and sell the product.
//!
//! One [`spend`], docked, in these steps:
//!
//! 1. **The wallet and the stores.** `LPSvc.GetAllMyCharacterWalletLPBalances`
//!    gives `(corpID, balance)` for every corporation the pilot holds LP
//!    with [V] (`loyaltyPointsWalletSvc.py:37`); each corporation's store is
//!    `LPStoreMgr.GetAvailableOffersFromCorp` [V] (`lpStoreSvc.py:141`). An
//!    LP balance is per corporation, so an offer is redeemed at the store of
//!    the corporation that issued the LP (the store window's own rule:
//!    `GetLPBalanceForStorePurchase(storeCorpID)`).
//! 2. **The profit** ([`isk_per_lp`], pure): the product's `P_mkt x qty`
//!    less [`SALE_COST`], less the offer's ISK, less the required items at
//!    what a bot would pay for them ([`crate::pricing::RESERVATION`] x
//!    `P_mkt`, the consumer's cap), per LP. [`choose`] takes the best rate of
//!    at least [`MIN_ISK_PER_LP`], and as many times as the LP, the ISK
//!    ([`SPEND_SHARE`] of the wallet) and [`MAX_REDEEM`] allow.
//! 3. **The items.** What the offer requires is bought on the market at
//!    this station ([`crate::acquire::buy_more`]). The store reads only
//!    packaged stacks in the station hangar [V] (`lpStoreSvc.py:287-296`,
//!    `lpOfferEntry2.py:54-59`), so the bot counts what the hangar holds
//!    and redeems as many times as that covers.
//! 4. **The redeem**: `LPStoreMgr.TakeOfferForCharacter(corpID, offerID,
//!    n)` [V] (`lpStoreSvc.py:315`), truth-tested.
//! 5. **The sale**: the escort's [`crate::escort::sell_loot`], which lists
//!    the product as an ask (or sells it into a bid). A product the sale
//!    leaves in the hangar (a hull, which is not loot to the escort) is
//!    remembered per pilot and never redeemed again.
//!
//! It repeats while something pays, at most [`MAX_ROUNDS`] times a visit.
//! A refusal is a note, never an error; only a broken connection is one.
//!
//! The choice and the rates are **[A]** policy (see [`crate::plexer`]'s
//! constants for their falsifiers); the calls are [V]. **[A]**: the bot
//! redeems at whatever station it is docked at. The client opens a store
//! only at a station owned by that corporation ([V] `eveCommands.py`
//! `OpenLpstore`) and this server does not check, so a bot does not travel
//! to the corporation's station; it was not found whether Tranquility's
//! server enforces it. Falsified by a Tranquility refusal of an offer
//! taken elsewhere, or by a `GetStationsForOwner` answer this bot should
//! have flown to.

use crate::plexer::{
    LP_STORE, LP_SVC, LpOffer, MAX_REDEEM, MIN_ISK_PER_LP, SALE_COST, SPEND_SHARE, read_offers,
};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

/// The most offers one visit redeems: a guard, not a policy. Each round
/// spends at least one offer's LP, so the loop ends by itself.
pub const MAX_ROUNDS: usize = 8;

/// One offer a pilot could take: the corporation whose store lists it and
/// the LP the pilot holds with that corporation.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub corp: i64,
    pub lp: i64,
    pub offer: LpOffer,
}

/// The offer [`choose`] picked.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub corp: i64,
    pub offer: LpOffer,
    pub times: i64,
    /// ISK per LP, net.
    pub rate: f64,
}

/// What the offer's required items cost per redeem when bought: `None`
/// when one has no price (the bot would not know what it pays).
pub fn item_cost(o: &LpOffer, p_mkt: &BTreeMap<i64, f64>) -> Option<f64> {
    o.req_items.iter().try_fold(0.0, |sum, &(t, qty)| {
        let p = p_mkt.get(&t).copied().filter(|p| *p > 0.0)?;
        Some(sum + p * crate::pricing::RESERVATION * qty as f64)
    })
}

/// **ISK per LP** of `o`, the product selling at `p_mkt[type]`: `None` for
/// a malformed offer or a product or required item with no price.
pub fn isk_per_lp(o: &LpOffer, p_mkt: &BTreeMap<i64, f64>) -> Option<f64> {
    if o.lp_cost <= 0 || o.qty <= 0 || o.isk_cost < 0 {
        return None;
    }
    let product = p_mkt.get(&o.type_id).copied().filter(|p| *p > 0.0)?;
    let net = product * o.qty as f64 * (1.0 - SALE_COST) - o.isk_cost as f64 - item_cost(o, p_mkt)?;
    Some(net / o.lp_cost as f64)
}

/// **The rule, pure**: the best offer among `cands`, `isk` to spend, the
/// products and items priced in `p_mkt`. The best [`isk_per_lp`] of at
/// least [`MIN_ISK_PER_LP`] (the lower corporation and offer id on a tie),
/// skipping the products in `skip` (and the offers in `skip_offers`), as
/// many times as the LP, the ISK outlay (the offer's ISK and its items
/// bought) and [`MAX_REDEEM`] allow. `None` when nothing pays or nothing
/// is affordable.
pub fn choose(
    cands: &[Candidate],
    p_mkt: &BTreeMap<i64, f64>,
    isk: f64,
    skip: &BTreeSet<i64>,
    skip_offers: &BTreeSet<(i64, i64)>,
) -> Option<Pick> {
    let mut best: Option<Pick> = None;
    for c in cands {
        let o = &c.offer;
        if skip.contains(&o.type_id) || skip_offers.contains(&(c.corp, o.offer_id)) {
            continue;
        }
        let Some(rate) = isk_per_lp(o, p_mkt) else {
            continue;
        };
        if rate < MIN_ISK_PER_LP {
            continue;
        }
        let outlay = o.isk_cost as f64 + item_cost(o, p_mkt).unwrap_or(0.0);
        let by_isk = if outlay > 0.0 {
            (isk.max(0.0) / outlay).floor().min(MAX_REDEEM as f64) as i64
        } else {
            MAX_REDEEM
        };
        let times = (c.lp / o.lp_cost).min(by_isk).min(MAX_REDEEM);
        if times < 1 {
            continue;
        }
        let better = match &best {
            None => true,
            Some(b) => {
                rate > b.rate
                    || (rate == b.rate && (c.corp, o.offer_id) < (b.corp, b.offer.offer_id))
            }
        };
        if better {
            best = Some(Pick {
                corp: c.corp,
                offer: o.clone(),
                times,
                rate,
            });
        }
    }
    best
}

/// `GetAllMyCharacterWalletLPBalances`: `[(corpID, balance)]` [V], the
/// corporations with LP to spend, by corporation id.
pub fn read_balances(v: &PyValue) -> BTreeMap<i64, i64> {
    let (PyValue::List(rows) | PyValue::Tuple(rows)) = v else {
        return BTreeMap::new();
    };
    rows.iter()
        .filter_map(|r| match r {
            PyValue::Tuple(p) | PyValue::List(p) => match (p.first(), p.get(1)) {
                (Some(PyValue::Int(corp)), Some(PyValue::Int(lp))) if *lp > 0 => Some((*corp, *lp)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// What one offer redeemed.
#[derive(Debug, Clone, PartialEq)]
pub struct Redeemed {
    pub corp: i64,
    pub offer: LpOffer,
    pub times: i64,
    pub rate: f64,
    /// `(type, units)` bought on the market for it.
    pub bought: Vec<(i64, i64)>,
}

/// What one [`spend`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spend {
    /// LP held, before, over every corporation.
    pub lp: i64,
    pub redeemed: Vec<Redeemed>,
    /// The escort's sale of the outputs, when something was redeemed.
    pub sale: Option<String>,
    /// Why a round stopped short, if it did.
    pub notes: Vec<String>,
}

impl Spend {
    pub fn summary(&self) -> String {
        if self.redeemed.is_empty() {
            let mut s = format!("{} LP: nothing worth redeeming", self.lp);
            for n in &self.notes {
                s.push_str("; ");
                s.push_str(n);
            }
            return s;
        }
        let mut parts: Vec<String> = self
            .redeemed
            .iter()
            .map(|r| {
                format!(
                    "redeemed offer {} x{} with {} ({} LP each, {:.0} ISK/LP) for {} x{}{}",
                    r.offer.offer_id,
                    r.times,
                    r.corp,
                    r.offer.lp_cost,
                    r.rate,
                    r.offer.type_id,
                    r.offer.qty * r.times,
                    if r.bought.is_empty() {
                        String::new()
                    } else {
                        format!(", bought {} item stacks first", r.bought.len())
                    }
                )
            })
            .collect();
        parts.extend(self.notes.iter().cloned());
        parts.push(self.sale.clone().unwrap_or_else(|| "not sold".to_owned()));
        format!("{} LP: {}", self.lp, parts.join("; "))
    }
}

/// Products the sale left in the hangar, per pilot: never redeemed again.
/// Per process, never persisted.
static LEFT: Mutex<Option<BTreeMap<i64, BTreeSet<i64>>>> = Mutex::new(None);

fn left_of(pilot: i64) -> BTreeSet<i64> {
    let guard = LEFT.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(|m| m.get(&pilot).cloned())
        .unwrap_or_default()
}

fn leave(pilot: i64, type_id: i64) {
    let mut guard = LEFT.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(BTreeMap::new)
        .entry(pilot)
        .or_default()
        .insert(type_id);
}

fn value(c: &mut Conn, service: &str, method: &str, args: Vec<PyValue>) -> Result<PyValue> {
    let outcome = c.call(service, method, args)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("lp: {method}: {key}")));
    }
    Ok(outcome.value().unwrap_or(PyValue::None))
}

/// Units of `type_id` in the station hangar as packaged stacks: all the
/// store takes [V].
fn in_hangar(c: &mut Conn, type_id: i64) -> Result<i64> {
    Ok(c.hangar_items()?
        .iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
        .map(|r| r.quantity)
        .sum())
}

/// **Spend the LP**, docked: the module header's steps. `only` limits it
/// to those corporations (a plexer's militia); `None` is every corporation
/// the pilot holds LP with (a mission runner's agents').
pub fn spend(c: &mut Conn, only: Option<&[i64]>) -> Result<Spend> {
    if c.session().station_id().is_none() {
        return Err(Error::NotSent("lp: spend: not docked".to_owned()));
    }
    let pilot = c.session().char_id().unwrap_or(0);
    let mut out = Spend::default();
    let mut stores: BTreeMap<i64, Vec<LpOffer>> = BTreeMap::new();
    let mut skip_offers: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut market: Option<crate::pricing::Prices> = None;
    for round in 0..MAX_ROUNDS {
        let mut balances = read_balances(&value(
            c,
            LP_SVC,
            "GetAllMyCharacterWalletLPBalances",
            vec![],
        )?);
        if let Some(only) = only {
            balances.retain(|corp, _| only.contains(corp));
        }
        if round == 0 {
            out.lp = balances.values().sum();
        }
        if balances.is_empty() {
            break;
        }
        let skip = left_of(pilot);
        let mut cands = Vec::new();
        for (&corp, &lp) in &balances {
            if let std::collections::btree_map::Entry::Vacant(e) = stores.entry(corp) {
                e.insert(read_offers(&value(
                    c,
                    LP_STORE,
                    "GetAvailableOffersFromCorp",
                    vec![PyValue::Int(corp)],
                )?));
            }
            for o in &stores[&corp] {
                if o.lp_cost <= lp && !skip.contains(&o.type_id) {
                    cands.push(Candidate {
                        corp,
                        lp,
                        offer: o.clone(),
                    });
                }
            }
        }
        if cands.is_empty() {
            break;
        }
        if market.is_none() {
            market = Some(crate::pricing::Prices::read(c)?);
        }
        let prices = market.as_mut().expect("read above");
        let types: Vec<i64> = cands
            .iter()
            .flat_map(|k| {
                std::iter::once(k.offer.type_id).chain(k.offer.req_items.iter().map(|r| r.0))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let p_mkt = prices.mkt_of(c, &types)?;
        let isk = c.cash_balance()? * SPEND_SHARE;
        let Some(pick) = choose(&cands, &p_mkt, isk, &skip, &skip_offers) else {
            break;
        };
        match redeem(c, &pick) {
            Ok(done) => {
                out.redeemed.push(done);
                // The wallet and the hangar changed: read them again.
                market = None;
            }
            Err(Error::Refused(why) | Error::NotSent(why)) => {
                skip_offers.insert((pick.corp, pick.offer.offer_id));
                out.notes.push(why);
            }
            Err(e) => return Err(e),
        }
    }
    if !out.redeemed.is_empty() {
        out.sale = Some(crate::escort::sell_loot(c)?.summary());
        let hangar = c.hangar_items()?;
        for r in &out.redeemed {
            if hangar
                .iter()
                .any(|i| i.type_id == r.offer.type_id && !crate::escort::is_loot(i))
            {
                leave(pilot, r.offer.type_id);
            }
        }
    }
    Ok(out)
}

/// One pick: buy its items, redeem as often as the hangar covers.
fn redeem(c: &mut Conn, pick: &Pick) -> Result<Redeemed> {
    let o = &pick.offer;
    let mut bought = Vec::new();
    let mut times = pick.times;
    if !o.req_items.is_empty() {
        let want: Vec<(i64, i64)> = o
            .req_items
            .iter()
            .map(|&(t, each)| (t, each * pick.times))
            .collect();
        // Only the shortfall is bought: units already in the hangar count.
        let mut shop = Vec::new();
        for &(t, n) in &want {
            let have = in_hangar(c, t)?;
            if have < n {
                shop.push((t, n));
                bought.push((t, n - have));
            }
        }
        if !shop.is_empty() {
            crate::acquire::buy_more(c, &[], &shop, None)?;
        }
        for &(t, each) in &o.req_items {
            times = times.min(in_hangar(c, t)? / each.max(1));
        }
        if times < 1 {
            return Err(Error::Refused(format!(
                "lp: offer {} needs items the market did not sell",
                o.offer_id
            )));
        }
    }
    let took = value(
        c,
        LP_STORE,
        "TakeOfferForCharacter",
        vec![
            PyValue::Int(pick.corp),
            PyValue::Int(o.offer_id),
            PyValue::Int(times),
        ],
    )?;
    if took != PyValue::Bool(true) {
        return Err(Error::Refused(format!(
            "lp: the store refused offer {} x{times}",
            o.offer_id
        )));
    }
    Ok(Redeemed {
        corp: pick.corp,
        offer: o.clone(),
        times,
        rate: pick.rate,
        bought,
    })
}

#[cfg(test)]
mod tests {
    //! The rule as tables, with invented offers and products.
    use super::*;

    const CORP_A: i64 = 1_999_901;
    const CORP_B: i64 = 1_999_902;
    const ITEM_X: i64 = 990_901;
    const ITEM_Y: i64 = 990_902;
    const ITEM_Z: i64 = 990_903;
    const INPUT: i64 = 990_904;

    fn offer(offer_id: i64, type_id: i64, lp_cost: i64, isk_cost: i64) -> LpOffer {
        LpOffer {
            offer_id,
            type_id,
            qty: 1,
            lp_cost,
            isk_cost,
            req_items: Vec::new(),
        }
    }

    fn cand(corp: i64, lp: i64, offer: LpOffer) -> Candidate {
        Candidate { corp, lp, offer }
    }

    /// The price of the product that makes `o` fetch `rate` ISK per LP
    /// exactly, the inputs priced in `p`.
    fn price_for(o: &LpOffer, rate: f64, p: &BTreeMap<i64, f64>) -> f64 {
        (rate * o.lp_cost as f64 + o.isk_cost as f64 + item_cost(o, p).expect("priced"))
            / (o.qty as f64 * (1.0 - SALE_COST))
    }

    fn none() -> BTreeSet<i64> {
        BTreeSet::new()
    }

    fn no_offers() -> BTreeSet<(i64, i64)> {
        BTreeSet::new()
    }

    #[test]
    fn the_best_paying_offer_is_redeemed_as_often_as_the_lp_and_isk_allow() {
        let good = offer(1, ITEM_X, 1_000, 100_000);
        let better = offer(2, ITEM_Y, 2_000, 0);
        let poor = offer(3, ITEM_Z, 1_000, 0);
        let empty = BTreeMap::new();
        let prices = BTreeMap::from([
            (ITEM_X, price_for(&good, MIN_ISK_PER_LP * 2.0, &empty)),
            (ITEM_Y, price_for(&better, MIN_ISK_PER_LP * 3.0, &empty)),
            (ITEM_Z, price_for(&poor, MIN_ISK_PER_LP / 2.0, &empty)),
        ]);
        let cands = |lp| {
            vec![
                cand(CORP_A, lp, good.clone()),
                cand(CORP_A, lp, better.clone()),
                cand(CORP_A, lp, poor.clone()),
            ]
        };
        let pick = choose(&cands(9_000), &prices, 1.0e9, &none(), &no_offers()).expect("a pick");
        assert_eq!((pick.offer.offer_id, pick.times), (2, 4));
        // Without the better product: the good one, capped by ISK.
        let skip = BTreeSet::from([ITEM_Y]);
        let pick = choose(&cands(9_000), &prices, 250_000.0, &skip, &no_offers()).expect("a pick");
        assert_eq!((pick.offer.offer_id, pick.times), (1, 2));
        // Too little LP for any, only the poor one left, or no prices.
        assert_eq!(
            choose(&cands(999), &prices, 1.0e9, &none(), &no_offers()),
            None
        );
        let only_poor = BTreeSet::from([ITEM_X, ITEM_Y]);
        assert_eq!(
            choose(&cands(9_000), &prices, 1.0e9, &only_poor, &no_offers()),
            None
        );
        assert_eq!(
            choose(
                &cands(9_000),
                &BTreeMap::new(),
                1.0e9,
                &none(),
                &no_offers()
            ),
            None
        );
        // A refused offer is not tried again.
        let refused = BTreeSet::from([(CORP_A, 2)]);
        let pick = choose(&cands(9_000), &prices, 1.0e9, &none(), &refused).expect("a pick");
        assert_eq!(pick.offer.offer_id, 1);
    }

    #[test]
    fn the_items_an_offer_takes_cost_what_the_bot_would_pay_for_them() {
        let mut o = offer(1, ITEM_X, 100, 1_000);
        o.req_items = vec![(INPUT, 4)];
        let mut p = BTreeMap::from([(INPUT, 50.0)]);
        // No price for the product: no rate. No price for the item: none.
        assert_eq!(isk_per_lp(&o, &p), None);
        p.insert(ITEM_X, 100_000.0);
        let items = 4.0 * 50.0 * crate::pricing::RESERVATION;
        assert_eq!(item_cost(&o, &p), Some(items));
        let net = 100_000.0 * (1.0 - SALE_COST) - 1_000.0 - items;
        assert_eq!(isk_per_lp(&o, &p), Some(net / 100.0));
        p.remove(&INPUT);
        assert_eq!(isk_per_lp(&o, &p), None);
        assert_eq!(item_cost(&o, &p), None);
    }

    /// An offer whose items cost more than its product fetches is not
    /// taken; one whose items are cheap is, and the items count against the
    /// ISK budget.
    #[test]
    fn an_offer_with_items_is_taken_when_it_pays_after_the_items() {
        let mut o = offer(1, ITEM_X, 100, 0);
        o.req_items = vec![(INPUT, 10)];
        let mut p = BTreeMap::from([(INPUT, 1_000.0)]);
        let dear = price_for(&o, MIN_ISK_PER_LP / 2.0, &p);
        p.insert(ITEM_X, dear);
        let cands = vec![cand(CORP_A, 10_000, o.clone())];
        assert_eq!(choose(&cands, &p, 1.0e9, &none(), &no_offers()), None);
        let fine = price_for(&o, MIN_ISK_PER_LP * 2.0, &p);
        p.insert(ITEM_X, fine);
        let pick = choose(&cands, &p, 1.0e9, &none(), &no_offers()).expect("pays now");
        assert_eq!(pick.times, 100, "LP allows 100, the bulk limit too");
        // Ten items a time at the reservation price: the budget buys three.
        let outlay = 10.0 * 1_000.0 * crate::pricing::RESERVATION;
        let pick = choose(&cands, &p, outlay * 3.5, &none(), &no_offers()).expect("three");
        assert_eq!(pick.times, 3);
        assert_eq!(
            choose(&cands, &p, outlay * 0.9, &none(), &no_offers()),
            None
        );
    }

    /// LP is per corporation: an offer is bought with the LP of its own
    /// store, and the better rate wins across stores.
    #[test]
    fn each_offer_is_paid_with_its_own_corporations_lp() {
        let a = offer(1, ITEM_X, 1_000, 0);
        let b = offer(1, ITEM_Y, 1_000, 0);
        let e = BTreeMap::new();
        let p = BTreeMap::from([
            (ITEM_X, price_for(&a, MIN_ISK_PER_LP * 2.0, &e)),
            (ITEM_Y, price_for(&b, MIN_ISK_PER_LP * 3.0, &e)),
        ]);
        let cands = vec![cand(CORP_A, 5_000, a.clone()), cand(CORP_B, 2_000, b)];
        let pick = choose(&cands, &p, 1.0e9, &none(), &no_offers()).expect("a pick");
        assert_eq!((pick.corp, pick.times), (CORP_B, 2), "B pays more, 2000 LP");
        // The same offer id in both stores is told apart.
        let skip = BTreeSet::from([(CORP_B, 1)]);
        let pick = choose(&cands, &p, 1.0e9, &none(), &skip).expect("A's");
        assert_eq!((pick.corp, pick.times), (CORP_A, 5));
    }

    #[test]
    fn the_wallet_reads_back_without_empty_balances() {
        let v = PyValue::List(vec![
            PyValue::Tuple(vec![PyValue::Int(CORP_A), PyValue::Int(55)]),
            PyValue::Tuple(vec![PyValue::Int(CORP_B), PyValue::Int(0)]),
        ]);
        assert_eq!(read_balances(&v), BTreeMap::from([(CORP_A, 55)]));
        assert!(read_balances(&PyValue::None).is_empty());
    }

    #[test]
    fn the_summary_names_the_offer_the_items_and_the_sale() {
        let mut s = Spend {
            lp: 900,
            ..Spend::default()
        };
        assert!(s.summary().contains("nothing worth redeeming"));
        s.redeemed.push(Redeemed {
            corp: CORP_A,
            offer: offer(7, ITEM_X, 300, 0),
            times: 3,
            rate: 700.0,
            bought: vec![(INPUT, 6)],
        });
        s.sale = Some("sold".to_owned());
        let text = s.summary();
        assert!(
            text.contains("offer 7 x3") && text.contains("bought 1 item"),
            "{text}"
        );
        assert!(text.ends_with("sold"), "{text}");
    }
}
