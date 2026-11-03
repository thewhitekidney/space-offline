//! **Kill rights** (CW-1): earned when a criminal kills you, sold, bought
//! and activated, used up by a kill, expired.
//!
//! What the client fixes **[V]** (build 3619879):
//!
//! - **the row**: every kill right the client holds is read by attribute --
//!   `killRightID`, `fromID` (the holder, the victim), `toID` (the target,
//!   the criminal), `expiryTime`, `price` (`None` when not for sale) and
//!   `restrictedTo` (`None`, or the character, corporation or alliance it
//!   is offered to) -- `killRightsPanel.LoadPanel`, `killRights.KillRightsEntry`,
//!   `killRightTracker.GetKillRights` (filters `restrictedTo is None or in
//!   validIDs`), `bountySvc.GetBestKillRight` (`min(..., key=price)`);
//! - **the calls**: `bountyProxy.GetMyKillRights()` (rights the caller holds
//!   *or* is the target of: the panel splits them by `fromID` / `toID` and
//!   drops the expired by `expiryTime`), `GetKillRightsOnCharacters(toIDs)`
//!   and `GetBountiesAndKillRights(ownerIDs, toIDs)` (rights for sale, for
//!   the overview's "has kill right" flag, `QuickHasKillRight`),
//!   `SellKillRight(killRightID, price, restrictedTo)` (returns the rows,
//!   added to the tracker), `CancelSellKillRight(killRightID, toID)`;
//!   `killRightMgr.ActivateKillRight(killRightID)` (one's own) and
//!   `BuyKillRight(killRightID, price)` (somebody else's for sale);
//! - **the refusals**: `NoValidKillRight` ("This kill right is no longer
//!   valid", 285164), `KillRightExpired` (no text; the client drops the
//!   right on both), `KillRightNotForSale` (285383),
//!   `KillRightPriceNotCorrect {newPrice}` (287054),
//!   `CantActivateKillRightInNullSec` (285384), all `raise_sites` 0 -- the
//!   server's to raise;
//! - **the pushes**: `OnKillRightCreated(killRightID, fromID, toID,
//!   expiryTime)`, `OnKillRightUsed(killRightID, toID)`,
//!   `OnKillRightActivated()` (the target's: "Kill right activated on you.
//!   You can temporarily be attacked without CONCORD intervention", 285057)
//!   and `OnKillRightForYouSold(killRightID)` (`bountySvc`, the panel);
//! - **what activation does**: "Activate suspect flag on {charName} for 15
//!   minutes" (the activation dialog's own text) -- the target becomes a
//!   **suspect**, anyone may shoot, "alone or with friends, without CONCORD
//!   intervention" (285051);
//! - **what a kill under it does**: "{charName} has been killed using your
//!   kill right. The kill right you had on this capsuleer has now been
//!   removed" (285055); the price is "Cost per activation" (283856), so a
//!   sale outlives a purchase until the right is used.
//!
//! `[A]`, each with its falsifier in `docs/lanes/cw-1.md`: the 30-day life
//! ([`DURATION`], game-clock paced), one usable right per pair (a second
//! criminal kill does not stack), a capsule kill earns one too, the right is
//! used only by the target's **ship** dying, an activation needs both
//! pilots in space in the activator's system and the target out of a
//! capsule.

use super::{Activation, CRIMINAL_TIMEOUT, DAY, Notice, Space};
use crate::damage::BlueTime;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// The delta kind: `id -> from to expiry price restrictedTo`, and `next ->
/// n`, the allocator.
pub const KIND: &str = "killright";

const NEXT: &str = "next";

/// How long a kill right lasts: 30 days `[A]` (TQ's figure; nothing in the
/// client names it), a slow timer the game clock paces (`--time-scale`).
pub const DURATION: BlueTime = 30 * DAY;

/// `refKillRightFee` -- the journal type the client names "Kill Right"
/// (`ledger.rs`: 116 -> 285169) **[V]**.
pub const REF_KILL_RIGHT_FEE: i64 = 116;

/// The kill-right notification types (`notificationconst.py`) **[V]**, and
/// the data keys each formatter reads (`notificationUtil.formatters`)
/// **[V]**: `charID` always; `price` for the two sales; `toEntityID` for
/// the two restricted ones.
pub mod notification {
    /// To the target: "The kill right {charName} has on you has been made
    /// available to {availableToName} for {amount}" (285054).
    pub const AVAILABLE: i64 = 115;
    /// To the target: "... made available to all for {amount}" (285216).
    pub const AVAILABLE_OPEN: i64 = 116;
    /// To the holder: "As a victim of {charName}'s unlawful aggression you
    /// now have a kill right against them" (285051).
    pub const EARNED: i64 = 117;
    /// To the holder: "{charName} has been killed using your kill right"
    /// (285055).
    pub const USED: i64 = 118;
    /// To the target: "{charName} has made a kill right on you unavailable
    /// to {availableToName}" (285504).
    pub const UNAVAILABLE: i64 = 119;
    /// To the target: "... unavailable to all" (285503).
    pub const UNAVAILABLE_OPEN: i64 = 120;
}

/// Tell `to` one kill-right notification about `char_id`, through the
/// notification centre (NOTIF-1's `notify`, sender "EVE System" `[A]`).
fn tell(
    w: &mut Registry,
    to: i64,
    type_id: i64,
    char_id: i64,
    price: Option<f64>,
    to_entity: Option<i64>,
) {
    use crate::notifications::{Value, data};
    let mut d = data([("charID", Value::Int(char_id))]);
    if let Some(p) = price {
        d.push(("price".to_owned(), Value::Real(p)));
    }
    if let Some(e) = to_entity {
        d.push(("toEntityID".to_owned(), Value::Int(e)));
    }
    crate::notifications::notify(w, to, type_id, d);
}

/// One kill right: `from` may have `to` killed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KillRight {
    pub id: i64,
    /// The holder: the victim of the crime.
    pub from: i64,
    /// The target: the criminal.
    pub to: i64,
    pub expiry: BlueTime,
    /// ISK per activation while for sale.
    pub price: Option<f64>,
    /// The character, corporation or alliance the sale is open to.
    pub restricted_to: Option<i64>,
}

impl KillRight {
    pub fn usable(&self, now: BlueTime) -> bool {
        now < self.expiry
    }

    fn to_row(self) -> Vec<String> {
        let opt_f = |v: Option<f64>| v.map_or_else(|| "-".to_owned(), |p| p.to_string());
        let opt_i = |v: Option<i64>| v.map_or_else(|| "-".to_owned(), |p| p.to_string());
        vec![
            self.from.to_string(),
            self.to.to_string(),
            self.expiry.to_string(),
            opt_f(self.price),
            opt_i(self.restricted_to),
        ]
    }

    fn from_row(id: i64, v: &[String]) -> Option<KillRight> {
        let opt_f = |s: &String| {
            if s == "-" {
                Some(None)
            } else {
                s.parse().ok().map(Some)
            }
        };
        let opt_i = |s: &String| {
            if s == "-" {
                Some(None)
            } else {
                s.parse().ok().map(Some)
            }
        };
        Some(KillRight {
            id,
            from: v.first()?.parse().ok()?,
            to: v.get(1)?.parse().ok()?,
            expiry: v.get(2)?.parse().ok()?,
            price: opt_f(v.get(3)?)?,
            restricted_to: opt_i(v.get(4)?)?,
        })
    }

    /// The row as the client reads it: a `utillib.KeyVal`, by attribute.
    pub fn to_value(self) -> PyValue {
        crate::pyutil::keyval(vec![
            ("killRightID", PyValue::Int(self.id)),
            ("fromID", PyValue::Int(self.from)),
            ("toID", PyValue::Int(self.to)),
            ("expiryTime", PyValue::Int(self.expiry)),
            ("price", self.price.map_or(PyValue::None, PyValue::Real)),
            (
                "restrictedTo",
                self.restricted_to.map_or(PyValue::None, PyValue::Int),
            ),
        ])
    }
}

/// Every stored kill right, by id, the expired included.
pub fn all(w: &Registry) -> Vec<KillRight> {
    w.deltas()
        .of(KIND)
        .iter()
        .filter_map(|(k, v)| {
            let id = k.first()?.parse().ok()?;
            KillRight::from_row(id, v)
        })
        .collect()
}

/// One kill right by id, expired or not.
pub fn get(w: &Registry, id: i64) -> Option<KillRight> {
    let v = w.deltas().of(KIND).get(&vec![id.to_string()])?;
    KillRight::from_row(id, v)
}

fn put(w: &mut Registry, kr: &KillRight) {
    w.deltas_mut()
        .put(KIND, vec![kr.id.to_string()], kr.to_row());
}

fn remove(w: &mut Registry, id: i64) {
    let key = vec![id.to_string()];
    if w.deltas().of(KIND).contains_key(&key) {
        w.deltas_mut().del(KIND, &key);
    }
}

/// Forget every right that has expired. Run on every read and write, so the
/// store never grows past the live rights.
fn prune(w: &mut Registry, now: BlueTime) {
    let dead: Vec<i64> = all(w)
        .into_iter()
        .filter(|kr| !kr.usable(now))
        .map(|kr| kr.id)
        .collect();
    for id in dead {
        remove(w, id);
    }
}

fn next_id(w: &mut Registry) -> i64 {
    let key = vec![NEXT.to_owned()];
    let next = w
        .deltas()
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    w.deltas_mut().put(KIND, key, vec![(next + 1).to_string()]);
    next
}

/// **Grant `from` a kill right on `to`**, from `now`: stored, and both told
/// (`OnKillRightCreated`). `None` when `from` already holds a usable one on
/// `to` `[A]` (one per pair) or the two are the same.
pub fn grant(w: &mut Registry, from: i64, to: i64, now: BlueTime) -> Option<KillRight> {
    if from == to {
        return None;
    }
    prune(w, now);
    if all(w).iter().any(|kr| kr.from == from && kr.to == to) {
        return None;
    }
    let span = crate::game_clock::of(w).span(DURATION);
    let kr = KillRight {
        id: next_id(w),
        from,
        to,
        expiry: now.saturating_add(span),
        price: None,
        restricted_to: None,
    };
    put(w, &kr);
    crate::outln!(
        "[crimewatch] kill right {}: {} may have {} killed",
        kr.id,
        from,
        to
    );
    for c in [from, to] {
        w.schedule(
            Duration::ZERO,
            Notice::new(
                c,
                "OnKillRightCreated",
                vec![
                    PyValue::Int(kr.id),
                    PyValue::Int(from),
                    PyValue::Int(to),
                    PyValue::Int(kr.expiry),
                ],
            ),
        );
    }
    tell(w, from, notification::EARNED, to, None, None);
    Some(kr)
}

/// **The kill hook** (from `crimewatch::on_kill`): a capsuleer's ship or
/// capsule died.
///
/// - **Used**: the victim's *ship* (not the capsule `[A]`) died while a kill
///   right on them was activated -- the right is removed, and its holder,
///   its activator and the target are told (`OnKillRightUsed`).
/// - **Earned**: the killer committed a criminal act against the victim
///   within the criminal timer before the kill (`crimewatch::commit` keeps
///   the record) -- "As a victim of {charName}'s unlawful aggression you now
///   have a kill right against them" (285051) **[V]**.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    let victim = ev.victim_character;
    let now = ev.when;
    if ev.ship_type != crate::damage::CAPSULE_TYPE_ID
        && let Some(act) = w.combat().crimewatch().activation(victim, now)
    {
        used(w, act, victim);
    }
    let Some(killer) = ev
        .killer
        .filter(|k| *k != victim && w.character(*k).is_some())
    else {
        return;
    };
    let criminal = w
        .combat()
        .crimewatch()
        .criminal_acts
        .get(&(killer, victim))
        .is_some_and(|at| now < *at + CRIMINAL_TIMEOUT);
    if criminal {
        grant(w, victim, killer, now);
    }
}

fn used(w: &mut Registry, act: Activation, target: i64) {
    w.combat_mut().crimewatch_mut().activations.remove(&target);
    let Some(kr) = get(w, act.kill_right_id) else {
        return;
    };
    remove(w, kr.id);
    crate::outln!(
        "[crimewatch] kill right {} used: {} died, activated by {}",
        kr.id,
        target,
        act.activator
    );
    tell(w, kr.from, notification::USED, kr.to, None, None);
    let mut to = vec![kr.from, act.activator, kr.to];
    to.sort_unstable();
    to.dedup();
    for c in to {
        w.schedule(
            Duration::ZERO,
            Notice::new(
                c,
                "OnKillRightUsed",
                vec![PyValue::Int(kr.id), PyValue::Int(kr.to)],
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

fn refuse(key: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(key, vec![]))
}

fn notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// The ids a character may buy for: itself, its corporation, its alliance
/// (`bountySvc.GetKillRights`' `validIDs`) **[V]**.
fn buyer_ids(w: &Registry, actor: i64) -> Vec<i64> {
    let mut ids = vec![actor];
    if let Some(c) = w.character(actor) {
        ids.push(c.corporation_id);
        ids.extend(c.alliance_id);
    }
    ids
}

fn for_sale_to(kr: &KillRight, ids: &[i64]) -> bool {
    kr.price.is_some() && kr.restricted_to.is_none_or(|r| ids.contains(&r))
}

/// `bountyProxy.GetMyKillRights()`: every usable right the caller holds or
/// is the target of.
pub fn get_my(w: &mut Registry, actor: Option<i64>, now: BlueTime) -> PyValue {
    prune(w, now);
    let Some(me) = actor else {
        return PyValue::List(vec![]);
    };
    PyValue::List(
        all(w)
            .into_iter()
            .filter(|kr| kr.from == me || kr.to == me)
            .map(KillRight::to_value)
            .collect(),
    )
}

/// The rights on `to_ids` the caller could buy: for sale, open to them.
/// `GetKillRightsOnCharacters(toIDs)` and the second half of
/// `GetBountiesAndKillRights`.
pub fn on_characters(
    w: &mut Registry,
    actor: Option<i64>,
    to_ids: &[i64],
    now: BlueTime,
) -> PyValue {
    prune(w, now);
    let ids = actor.map_or_else(Vec::new, |a| buyer_ids(w, a));
    PyValue::List(
        all(w)
            .into_iter()
            .filter(|kr| to_ids.contains(&kr.to) && for_sale_to(kr, &ids))
            .map(KillRight::to_value)
            .collect(),
    )
}

/// `bountyProxy.SellKillRight(killRightID, price, restrictedTo)`: the holder
/// offers a right at `price` per activation, to everyone or to one owner.
/// Returns the right as a one-row list (`bountySvc.SellKillRight` adds each
/// to its tracker) **[V]**; the target is told (`OnKillRightForYouSold`
/// [I]).
pub fn sell(
    w: &mut Registry,
    actor: Option<i64>,
    id: i64,
    price: f64,
    restricted_to: Option<i64>,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    prune(w, now);
    let Some(mut kr) = get(w, id).filter(|kr| Some(kr.from) == actor) else {
        return Err(refuse("NoValidKillRight"));
    };
    if !price.is_finite() || price < 0.0 {
        return Err(notify("A kill right's price cannot be negative."));
    }
    kr.price = Some(price);
    kr.restricted_to = restricted_to;
    put(w, &kr);
    w.schedule(
        Duration::ZERO,
        Notice::new(kr.to, "OnKillRightForYouSold", vec![PyValue::Int(kr.id)]),
    );
    let kind = match restricted_to {
        Some(_) => notification::AVAILABLE,
        None => notification::AVAILABLE_OPEN,
    };
    tell(w, kr.to, kind, kr.from, Some(price), restricted_to);
    Ok(PyValue::List(vec![kr.to_value()]))
}

/// `bountyProxy.CancelSellKillRight(killRightID, toID)`: the sale ends. A
/// stale id is answered quietly -- the client has already dropped it.
pub fn cancel(w: &mut Registry, actor: Option<i64>, id: i64, now: BlueTime) -> PyValue {
    prune(w, now);
    if let Some(mut kr) = get(w, id).filter(|kr| Some(kr.from) == actor) {
        let was = (kr.price, kr.restricted_to);
        kr.price = None;
        kr.restricted_to = None;
        put(w, &kr);
        w.schedule(
            Duration::ZERO,
            Notice::new(kr.to, "OnKillRightForYouSold", vec![PyValue::Int(kr.id)]),
        );
        // Only a sale that was on is withdrawn.
        match was {
            (Some(_), Some(r)) => tell(w, kr.to, notification::UNAVAILABLE, kr.from, None, Some(r)),
            (Some(_), None) => tell(
                w,
                kr.to,
                notification::UNAVAILABLE_OPEN,
                kr.from,
                None,
                None,
            ),
            _ => {}
        }
    }
    PyValue::None
}

/// May `activator` exercise `kr` now? Both in space in one system that is not
/// zero sec, the target out of a capsule `[A]`.
fn check_activation(
    w: &Registry,
    activator: i64,
    kr: &KillRight,
    now: BlueTime,
) -> Result<(), ServiceError> {
    if !kr.usable(now) {
        return Err(refuse("KillRightExpired"));
    }
    if activator == kr.to {
        return Err(refuse("NoValidKillRight"));
    }
    let Some(here) = w.roster().get(activator).map(|p| p.solar_system_id) else {
        return Err(notify("You must be in space to activate a kill right."));
    };
    if super::space_of(w, here) == Space::Zero {
        return Err(refuse("CantActivateKillRightInNullSec"));
    }
    let target = w.roster().get(kr.to).copied();
    if target.is_none_or(|t| t.solar_system_id != here) {
        return Err(notify(
            "The kill right's target is not in space in your solar system.",
        ));
    }
    if w.character(kr.to)
        .is_some_and(|c| c.ship_type_id == crate::damage::CAPSULE_TYPE_ID)
    {
        return Err(notify(
            "A kill right cannot be activated on a pilot in a capsule.",
        ));
    }
    Ok(())
}

/// The target becomes a suspect for fifteen minutes, the activation is
/// remembered for the kill that uses it, and the target is told
/// (`OnKillRightActivated`).
fn begin(w: &mut Registry, kr: &KillRight, activator: i64, now: BlueTime) {
    super::suspect(w, kr.to, now);
    w.combat_mut().crimewatch_mut().activations.insert(
        kr.to,
        Activation {
            kill_right_id: kr.id,
            activator,
            until: now + CRIMINAL_TIMEOUT,
        },
    );
    crate::outln!(
        "[crimewatch] kill right {} activated by {} on {}",
        kr.id,
        activator,
        kr.to
    );
    w.schedule(
        Duration::ZERO,
        Notice::new(kr.to, "OnKillRightActivated", vec![]),
    );
}

/// `killRightMgr.ActivateKillRight(killRightID)`: the holder exercises their
/// own right, free. `None` on success (the reply is discarded).
pub fn activate(
    w: &mut Registry,
    actor: Option<i64>,
    id: i64,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(me) = actor else {
        return Err(refuse("NoValidKillRight"));
    };
    let Some(kr) = get(w, id).filter(|kr| kr.from == me) else {
        return Err(refuse("NoValidKillRight"));
    };
    check_activation(w, me, &kr, now)?;
    begin(w, &kr, me, now);
    Ok(PyValue::None)
}

/// `killRightMgr.BuyKillRight(killRightID, price)`: somebody else's right,
/// for sale to the buyer, bought for one activation at the price the buyer
/// saw (`KillRightPriceNotCorrect {newPrice}` when it changed) and paid to
/// the holder (journal type "Kill Right", [`REF_KILL_RIGHT_FEE`]).
pub fn buy(
    w: &mut Registry,
    actor: Option<i64>,
    id: i64,
    price: f64,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(me) = actor else {
        return Err(refuse("NoValidKillRight"));
    };
    let Some(kr) = get(w, id) else {
        return Err(refuse("NoValidKillRight"));
    };
    if !kr.usable(now) {
        return Err(refuse("KillRightExpired"));
    }
    let Some(asking) = kr.price.filter(|_| for_sale_to(&kr, &buyer_ids(w, me))) else {
        return Err(refuse("KillRightNotForSale"));
    };
    if (asking - price).abs() > 0.5 {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "KillRightPriceNotCorrect",
            crate::pyutil::pypairs(vec![("newPrice", PyValue::Real(asking))]),
        )));
    }
    check_activation(w, me, &kr, now)?;
    let balance = w.character(me).map_or(0.0, |c| c.balance);
    if balance < asking {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "NotEnoughMoney",
            crate::pyutil::pypairs(vec![
                ("amount", PyValue::Real(asking)),
                ("balance", PyValue::Real(balance)),
            ]),
        )));
    }
    if me != kr.from && asking > 0.0 {
        w.charge(me, asking, REF_KILL_RIGHT_FEE, kr.from, kr.id);
        w.charge(kr.from, -asking, REF_KILL_RIGHT_FEE, me, kr.id);
    }
    begin(w, &kr, me, now);
    Ok(PyValue::None)
}

/// Every int in an id collection the client sends: a list, a tuple, or a
/// `set` (marshalled as a reduce, its members in the header's arguments).
pub fn ids_in(v: Option<&PyValue>) -> Vec<i64> {
    fn walk(v: &PyValue, out: &mut Vec<i64>) {
        match v {
            PyValue::Int(id) => out.push(*id),
            PyValue::List(xs) | PyValue::Tuple(xs) => xs.iter().for_each(|x| walk(x, out)),
            PyValue::ObjectEx { header, list, .. } => {
                walk(header, out);
                list.iter().for_each(|x| walk(x, out));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(v) = v {
        walk(v, &mut out);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A number argument: an int or a float.
pub fn number(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Real(r) => Some(*r),
        _ => None,
    }
}
