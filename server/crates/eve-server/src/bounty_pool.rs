//! **Player bounties** (GAP-BOUNTY, EVEJS-GAP svcA B9): ISK placed on a
//! character, a corporation or an alliance, shown in the Bounty Office, and
//! paid out when the target's ship dies.
//!
//! State is three delta kinds, so a save keeps it:
//!
//! - [`KIND_POOL`]: `target -> bounty updated_at`, the sum on one target.
//! - [`KIND_GIVE`]: `contribution_id -> placer target amount at` (and a
//!   `next` allocator row), what `GetMyBounties` lists. A payout shrinks the
//!   contributions to a target in step with its pool.
//! - [`KIND_HUNT`]: `owner -> claimed kills`, the Top Hunters tab, kept for
//!   the hunter, their corporation and their alliance.
//!
//! What the client fixes **[V]** (`bountySvc`, `bountyUtil`, `bountyWindow`):
//! every row is a `KeyVal` read by attribute; `corporationID` and
//! `allianceID` are tested with `hasattr`, so a missing one is **left out,
//! never sent as 0**; the minimums are `appConst.MIN_BOUNTY_AMOUNT_*`; the
//! journal line for a placement is "{o1} gave cash to {arg1}'s bounty pool"
//! (ref 16, owner1 the placer, reference the target) and for a prize
//! "{o2} got bounty prize for killing {arg1}" (ref 17, owner2 the hunter,
//! reference the victim).

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::kill_hooks::KillEvent;
use crate::notifications::{Value, data, kind};
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `id -> bounty updated_at`.
pub const KIND_POOL: &str = "bountypool";
/// `contribution_id -> placer target amount at`; `next -> n`.
pub const KIND_GIVE: &str = "bountygive";
/// `owner -> claimed kills`.
pub const KIND_HUNT: &str = "bountyhunt";

const NEXT: &str = "next";

/// `appConst.refBounty` **[V]**: ISK given to a bounty pool.
pub const REF_BOUNTY: i64 = 16;
/// `appConst.refBountyPrize` **[V]**: a prize for killing a bountied pilot.
pub const REF_BOUNTY_PRIZE: i64 = 17;

/// `appConst.MIN_BOUNTY_AMOUNT_CHAR` **[V]** (appConst.pyc:2558).
pub const MIN_BOUNTY_AMOUNT_CHAR: f64 = 1_000_000.0;
/// `appConst.MIN_BOUNTY_AMOUNT_CORP` **[V]**.
pub const MIN_BOUNTY_AMOUNT_CORP: f64 = 20_000_000.0;
/// `appConst.MIN_BOUNTY_AMOUNT_ALLIANCE` **[V]**.
pub const MIN_BOUNTY_AMOUNT_ALLIANCE: f64 = 100_000_000.0;

/// **[A]** The share of a kill's `iskLost` that a kill can claim from the
/// victim's pools: 20 %. EVE knowledge, in no client file. Falsified by a
/// reference killmail whose `bountyClaimed` is not that share of `iskLost`
/// while the pool exceeded it.
pub const CLAIM_FRACTION: f64 = 0.2;

/// **[A]** The claim is split among the character attackers by damage dealt
/// (equally when none dealt any). Falsified by a reference journal showing
/// the whole sum paid to the final blow (EvEJS pays the final blow only).
/// A kill whose final blow is not a character (an NPC, CONCORD) pays nothing.
pub const SPLIT_BY_DAMAGE: bool = true;

/// Rows per page of the top lists **[V]** (`bountyWindow` shows ten).
pub const PAGE: usize = 10;

/// A pool this small is empty (float dust after a payout).
const DUST: f64 = 0.005;

// ---- targets --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Char,
    Corp,
    Alliance,
}

/// What `id` is, when it can carry a bounty: a character, a player
/// corporation, a player alliance that exist here.
pub fn owner_kind(w: &Registry, id: i64) -> Option<Owner> {
    if crate::war::is_alliance_id(id) {
        crate::war::owner_exists(w, id).then_some(Owner::Alliance)
    } else if w.character(id).is_some() {
        Some(Owner::Char)
    } else {
        crate::service::corp::is_player_corp(w, id).then_some(Owner::Corp)
    }
}

pub fn minimum(kind: Owner) -> f64 {
    match kind {
        Owner::Char => MIN_BOUNTY_AMOUNT_CHAR,
        Owner::Corp => MIN_BOUNTY_AMOUNT_CORP,
        Owner::Alliance => MIN_BOUNTY_AMOUNT_ALLIANCE,
    }
}

// ---- pools ----------------------------------------------------------------------

fn num(v: &[String], i: usize) -> Option<f64> {
    v.get(i)?.parse().ok().filter(|x: &f64| x.is_finite())
}

/// The ISK on `target`.
pub fn pool(d: &Deltas, target: i64) -> f64 {
    d.of(KIND_POOL)
        .get(&vec![target.to_string()])
        .and_then(|v| num(v, 0))
        .unwrap_or(0.0)
}

fn set_pool(d: &mut Deltas, target: i64, total: f64, now: BlueTime) {
    let key = vec![target.to_string()];
    if total > DUST {
        d.put(KIND_POOL, key, vec![total.to_string(), now.to_string()]);
    } else {
        d.del(KIND_POOL, &key);
    }
}

/// Every pool, with its target, biggest first (ties by id).
pub fn pools(d: &Deltas) -> Vec<(i64, f64)> {
    let mut v: Vec<(i64, f64)> = d
        .of(KIND_POOL)
        .iter()
        .filter_map(|(k, row)| Some((k.first()?.parse().ok()?, num(row, 0)?)))
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// One contribution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Give {
    pub id: i64,
    pub placer: i64,
    pub target: i64,
    pub amount: f64,
    pub at: BlueTime,
}

fn gives(d: &Deltas) -> Vec<Give> {
    d.of(KIND_GIVE)
        .iter()
        .filter_map(|(k, v)| {
            Some(Give {
                id: k.first()?.parse().ok()?,
                placer: v.first()?.parse().ok()?,
                target: v.get(1)?.parse().ok()?,
                amount: num(v, 2)?,
                at: v.get(3)?.parse().ok()?,
            })
        })
        .collect()
}

fn put_give(d: &mut Deltas, g: &Give) {
    d.put(
        KIND_GIVE,
        vec![g.id.to_string()],
        vec![
            g.placer.to_string(),
            g.target.to_string(),
            g.amount.to_string(),
            g.at.to_string(),
        ],
    );
}

fn next_give_id(d: &mut Deltas) -> i64 {
    let key = vec![NEXT.to_owned()];
    let next = d
        .of(KIND_GIVE)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    d.put(KIND_GIVE, key, vec![(next + 1).to_string()]);
    next
}

/// Bring the contributions to `target` down to the pool's new size, each in
/// proportion, dropping the emptied.
fn shrink_gives(d: &mut Deltas, target: i64, before: f64, after: f64) {
    let ratio = if before > 0.0 { after / before } else { 0.0 };
    for mut g in gives(d).into_iter().filter(|g| g.target == target) {
        g.amount *= ratio;
        if g.amount > DUST {
            put_give(d, &g);
        } else {
            d.del(KIND_GIVE, &[g.id.to_string()]);
        }
    }
}

// ---- hunters --------------------------------------------------------------------

/// `(claimed ISK, kills)` for an owner.
pub fn hunter(d: &Deltas, owner: i64) -> (f64, i64) {
    d.of(KIND_HUNT)
        .get(&vec![owner.to_string()])
        .map_or((0.0, 0), |v| {
            (
                num(v, 0).unwrap_or(0.0),
                v.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
            )
        })
}

fn add_hunt(d: &mut Deltas, owner: i64, amount: f64) {
    let (claimed, kills) = hunter(d, owner);
    d.put(
        KIND_HUNT,
        vec![owner.to_string()],
        vec![(claimed + amount).to_string(), (kills + 1).to_string()],
    );
}

fn hunters(d: &Deltas) -> Vec<(i64, f64, i64)> {
    let mut v: Vec<(i64, f64, i64)> = d
        .of(KIND_HUNT)
        .iter()
        .filter_map(|(k, row)| {
            Some((
                k.first()?.parse().ok()?,
                num(row, 0)?,
                row.get(1)?.parse().ok()?,
            ))
        })
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

// ---- wire rows ------------------------------------------------------------------

/// The corporation and alliance a character target belongs to, as keys to
/// add; a key with no value is left out.
fn affiliation(w: &Registry, owner: i64) -> Vec<(&'static str, PyValue)> {
    let mut v = Vec::new();
    if let Some(c) = w.character(owner) {
        v.push(("corporationID", PyValue::Int(c.corporation_id)));
        if let Some(a) = c.alliance_id {
            v.push(("allianceID", PyValue::Int(a)));
        }
    }
    v
}

/// A pool as the client reads it: `targetID`, `bounty`, and the character's
/// corporation and alliance when known.
pub fn pool_row(w: &Registry, target: i64, bounty: f64) -> PyValue {
    let mut f = vec![
        ("targetID", PyValue::Int(target)),
        ("bounty", PyValue::Real(bounty)),
    ];
    f.extend(affiliation(w, target));
    crate::pyutil::keyval(f)
}

/// `GetBounties`: one `(targetID, row)` per id, 0.0 where nothing is on it.
pub fn bounties(w: &Registry, targets: &[i64]) -> PyValue {
    PyValue::List(
        targets
            .iter()
            .map(|&id| {
                PyValue::Tuple(vec![
                    PyValue::Int(id),
                    pool_row(w, id, pool(w.deltas(), id)),
                ])
            })
            .collect(),
    )
}

/// `GetMyBounties`: the caller's contributions, newest first.
pub fn mine(w: &Registry, actor: Option<i64>) -> PyValue {
    let Some(me) = actor else {
        return PyValue::List(vec![]);
    };
    let mut g: Vec<Give> = gives(w.deltas())
        .into_iter()
        .filter(|g| g.placer == me)
        .collect();
    g.sort_by_key(|g| std::cmp::Reverse(g.id));
    PyValue::List(
        g.into_iter()
            .map(|g| {
                let mut f = vec![
                    ("contributionID", PyValue::Int(g.id)),
                    ("targetID", PyValue::Int(g.target)),
                    ("amount", PyValue::Real(g.amount)),
                ];
                f.extend(affiliation(w, g.target));
                crate::pyutil::keyval(f)
            })
            .collect(),
    )
}

fn of_kind(w: &Registry, kind: Owner) -> Vec<(i64, f64)> {
    pools(w.deltas())
        .into_iter()
        .filter(|(id, _)| owner_kind(w, *id) == Some(kind))
        .collect()
}

/// `GetTop{Pilot,Corp,Alliance}Bounties(page)`: `(rows, filetime)`, the
/// page's ten biggest pools of the kind.
pub fn top(w: &Registry, kind: Owner, page: i64, now: BlueTime) -> PyValue {
    let skip = usize::try_from(page.max(0))
        .unwrap_or(0)
        .saturating_mul(PAGE);
    let rows = of_kind(w, kind)
        .into_iter()
        .skip(skip)
        .take(PAGE)
        .map(|(id, b)| pool_row(w, id, b))
        .collect();
    PyValue::Tuple(vec![PyValue::List(rows), PyValue::Int(now)])
}

/// `Search{Char,Corp,Alliance}Bounties(id)`: `[(rank, row)]` (rank counts
/// from 0) or `[]` when nothing is on `id`.
pub fn search(w: &Registry, kind: Owner, id: i64) -> PyValue {
    let all = of_kind(w, kind);
    match all.iter().position(|(t, _)| *t == id) {
        Some(rank) => PyValue::List(vec![PyValue::Tuple(vec![
            PyValue::Int(i64::try_from(rank).unwrap_or(i64::MAX)),
            pool_row(w, id, all[rank].1),
        ])]),
        None => PyValue::List(vec![]),
    }
}

/// `GetTop*BountyHunters(page)`: `(rows, filetime)`, ranked 1-based.
pub fn top_hunters(w: &Registry, kind: Owner, page: i64, now: BlueTime) -> PyValue {
    let skip = usize::try_from(page.max(0))
        .unwrap_or(0)
        .saturating_mul(PAGE);
    let rows = hunters(w.deltas())
        .into_iter()
        .filter(|(id, _, _)| owner_kind(w, *id) == Some(kind))
        .skip(skip)
        .take(PAGE)
        .enumerate()
        .map(|(i, (id, claimed, kills))| {
            let mut f = vec![
                ("bountyHunterID", PyValue::Int(id)),
                ("bountiesClaimed", PyValue::Real(claimed)),
                ("numberOfKills", PyValue::Int(kills)),
                (
                    "rowNumber",
                    PyValue::Int(i64::try_from(skip + i + 1).unwrap_or(i64::MAX)),
                ),
            ];
            f.extend(affiliation(w, id));
            crate::pyutil::keyval(f)
        })
        .collect();
    PyValue::Tuple(vec![PyValue::List(rows), PyValue::Int(now)])
}

// ---- placing --------------------------------------------------------------------

fn custom_notify(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// **`bountyProxy.AddToBounty(ownerID, amount)`**: take `amount` from
/// `placer` (journal ref 16, owner1 the placer, reference the target), add
/// it to the target's pool, tell the target (112 for a character, 113 to a
/// corporation's members, 114 to an alliance's, never the placer). Answers
/// the pool as the client caches it.
pub fn place(
    w: &mut Registry,
    placer: i64,
    target: i64,
    amount: f64,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(what) = owner_kind(w, target) else {
        return Err(custom_notify(
            "Only a character, corporation or alliance can have a bounty.",
        ));
    };
    if !amount.is_finite() || amount < minimum(what) {
        return Err(custom_notify(
            "The bounty is below the minimum for this kind of target.",
        ));
    }
    let balance = w.character(placer).map_or(0.0, |c| c.balance);
    if balance < amount {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "NotEnoughMoney",
            crate::pyutil::pypairs(vec![
                ("amount", PyValue::Real(amount)),
                ("balance", PyValue::Real(balance)),
            ]),
        )));
    }
    w.charge(placer, amount, REF_BOUNTY, target, target);
    let total = pool(w.deltas(), target) + amount;
    set_pool(w.deltas_mut(), target, total, now);
    let id = next_give_id(w.deltas_mut());
    put_give(
        w.deltas_mut(),
        &Give {
            id,
            placer,
            target,
            amount,
            at: now,
        },
    );
    let (type_id, audience) = match what {
        Owner::Char => (kind::CHAR, vec![target]),
        Owner::Corp => (kind::CORP, crate::war::owner_characters(w, target)),
        Owner::Alliance => (kind::ALLIANCE, crate::war::owner_characters(w, target)),
    };
    for to in audience.into_iter().filter(|c| *c != placer) {
        crate::notifications::notify_at(
            w,
            to,
            type_id,
            placer,
            now,
            data([
                ("bountyPlacerID", Value::Int(placer)),
                ("bounty", Value::Real(amount)),
            ]),
        );
    }
    Ok(pool_row(w, target, total))
}

// ---- paying out -----------------------------------------------------------------

/// Take up to `cap` from the victim's pools, character first, then the
/// corporation, then the alliance. Answers what was taken.
pub fn drain(w: &mut Registry, owners: [Option<i64>; 3], cap: f64, now: BlueTime) -> f64 {
    let mut left = cap.max(0.0);
    let mut taken = 0.0;
    for target in owners.into_iter().flatten() {
        if left <= 0.0 {
            break;
        }
        let before = pool(w.deltas(), target);
        let take = before.min(left);
        if take <= 0.0 {
            continue;
        }
        let after = before - take;
        set_pool(w.deltas_mut(), target, after, now);
        shrink_gives(
            w.deltas_mut(),
            target,
            before,
            if after > DUST { after } else { 0.0 },
        );
        left -= take;
        taken += take;
    }
    taken
}

/// `total` split among `shares` (`who`, weight) in proportion to the
/// weights; equally when every weight is 0. The parts add up to `total`.
pub fn split(total: f64, shares: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let weight: f64 = shares.iter().map(|s| s.1.max(0.0)).sum();
    let n = shares.len() as f64;
    shares
        .iter()
        .map(|&(who, w)| {
            let part = if weight > 0.0 {
                total * w.max(0.0) / weight
            } else {
                total / n
            };
            (who, part)
        })
        .collect()
}

/// One paid hunter: the character and the corporation and alliance they
/// fought for.
struct Hunter {
    character: i64,
    corporation: Option<i64>,
    alliance: Option<i64>,
    damage: f64,
}

/// The character attackers of `r`, one entry each, the victim never among
/// them.
fn hunters_of(r: &crate::killmail::Report) -> Vec<Hunter> {
    let mut out: Vec<Hunter> = Vec::new();
    for a in std::iter::once(&r.final_blow).chain(r.attackers.iter()) {
        let Some(c) = a.character.filter(|c| *c != r.victim) else {
            continue;
        };
        match out.iter_mut().find(|h| h.character == c) {
            Some(h) => h.damage += a.damage as f64,
            None => out.push(Hunter {
                character: c,
                corporation: a.corporation,
                alliance: a.alliance,
                damage: a.damage as f64,
            }),
        }
    }
    out
}

/// Credit `hunter` with `amount` (corporation tax skimmed as for any prize):
/// balance, a ref-17 row (owner2 the hunter, reference the victim),
/// `OnAccountChange` on the clock.
fn pay(w: &mut Registry, hunter: i64, victim: i64, amount: f64, now: BlueTime) {
    use crate::service::corp_wallet::{self, Via};
    let skim = corp_wallet::skim(w, Via::Clock, hunter, amount, REF_BOUNTY_PRIZE, victim);
    let net = amount - skim.map_or(0.0, |s| s.tax);
    let description = corp_wallet::stamped("", REF_BOUNTY_PRIZE, skim);
    let Some(c) = w.character_mut(hunter) else {
        return;
    };
    c.balance += net;
    let balance = c.balance;
    w.journal_mut().record(
        now,
        REF_BOUNTY_PRIZE,
        victim,
        hunter,
        victim,
        net,
        balance,
        &description,
    );
    w.mark_dirty();
    w.schedule(
        std::time::Duration::ZERO,
        crate::notify::Addressed {
            to: Some(vec![hunter]),
            inner: Box::new(crate::notify::AccountChange {
                owner_id: hunter,
                balance,
            }),
        },
    );
}

/// **The `kill_hooks::PLAYER` row**, after `killmail_on_kill`: a character's
/// ship died; if a bounty is on the pilot, their corporation or alliance,
/// take [`CLAIM_FRACTION`] of the loss from the pools, pay the character
/// attackers, put the sum on the report (`bountyClaimed`), tell the victim
/// (111) and each hunter (14).
pub fn on_kill(w: &mut Registry, ev: &KillEvent) {
    let victim = ev.victim_character;
    let Some(c) = w.character(victim) else {
        return;
    };
    let owners = [Some(victim), Some(c.corporation_id), c.alliance_id];
    if owners
        .iter()
        .flatten()
        .all(|t| pool(w.deltas(), *t) <= DUST)
    {
        return;
    }
    let Some(mut report) = crate::killmail::involving(w, victim, None, 1)
        .into_iter()
        .find(|r| r.victim == victim && r.time == ev.when)
    else {
        return;
    };
    // An NPC or CONCORD final blow pays nothing; so does an unpriced loss.
    let Some(lost) = report.isk_lost.filter(|v| v.is_finite() && *v > 0.0) else {
        return;
    };
    if report.final_blow.character.is_none_or(|f| f == victim) {
        return;
    }
    let hunters = hunters_of(&report);
    if hunters.is_empty() {
        return;
    }
    let taken = drain(w, owners, CLAIM_FRACTION * lost, ev.when);
    if taken <= 0.0 {
        return;
    }
    let weights: Vec<(i64, f64)> = hunters
        .iter()
        .map(|h| (h.character, if SPLIT_BY_DAMAGE { h.damage } else { 1.0 }))
        .collect();
    for (h, (_, part)) in hunters.iter().zip(split(taken, &weights)) {
        pay(w, h.character, victim, part, ev.when);
        for owner in [Some(h.character), h.corporation, h.alliance]
            .into_iter()
            .flatten()
        {
            add_hunt(w.deltas_mut(), owner, part);
        }
        crate::notifications::notify_at(
            w,
            h.character,
            kind::BOUNTY_CLAIM_MSG,
            crate::notifications::SENDER_SYSTEM,
            ev.when,
            data([
                ("charID", Value::Int(victim)),
                ("amount", Value::Real(part)),
            ]),
        );
    }
    crate::notifications::notify_at(
        w,
        victim,
        kind::BOUNTY_YOUR_BOUNTY_CLAIMED,
        crate::notifications::SENDER_SYSTEM,
        ev.when,
        data([
            ("victimID", Value::Int(victim)),
            ("bounty", Value::Real(taken)),
        ]),
    );
    report.bounty_claimed = Some(taken);
    crate::killmail::put(w, &report);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Character;
    use crate::service::Request;

    const PLACER: i64 = 7_001;
    const TARGET: i64 = 7_002;
    const OTHER: i64 = 7_003;
    const HUNTER: i64 = 7_004;
    const HUNTER2: i64 = 7_005;
    const RICH: f64 = 5_000_000_000.0;

    fn world() -> Registry {
        crate::testkit::registry_with(|c| {
            c.characters = [PLACER, TARGET, OTHER, HUNTER, HUNTER2]
                .into_iter()
                .map(|id| Character {
                    character_id: id,
                    name: format!("Pilot {id}"),
                    ship_id: id + 10_000_000,
                    balance: RICH,
                    ..Character::default()
                })
                .collect();
        })
    }

    fn call(
        reg: &mut Registry,
        method: &str,
        actor: i64,
        args: Vec<PyValue>,
    ) -> Result<PyValue, ServiceError> {
        let req = Request {
            service: "bountyProxy",
            method,
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        };
        reg.dispatch(&req)
    }

    fn balance(reg: &Registry, who: i64) -> f64 {
        reg.character(who).expect("a pilot").balance
    }

    fn notes(reg: &Registry, who: i64, type_id: i64) -> usize {
        crate::notifications::of(reg.deltas(), who)
            .iter()
            .filter(|n| n.type_id == type_id)
            .count()
    }

    fn rows_of(v: &PyValue) -> &Vec<PyValue> {
        let PyValue::List(rows) = v else {
            panic!("a list, got {v:?}")
        };
        rows
    }

    fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        let PyValue::Instance { state, .. } = v else {
            panic!("not a KeyVal: {v:?}")
        };
        state.dict_as_kv().and_then(|kv| {
            kv.into_iter().find_map(|(k, val)| {
                (crate::pyutil::as_bytes(k) == Some(name.as_bytes())).then_some(val)
            })
        })
    }

    fn place(reg: &mut Registry, target: i64, amount: f64) -> Result<PyValue, ServiceError> {
        call(
            reg,
            "AddToBounty",
            PLACER,
            vec![PyValue::Int(target), PyValue::Real(amount)],
        )
    }

    /// Below the minimum for the kind of target: refused, nothing taken.
    #[test]
    fn below_the_minimum_is_refused_and_the_wallet_is_unchanged() {
        let mut reg = world();
        assert!(place(&mut reg, TARGET, MIN_BOUNTY_AMOUNT_CHAR - 1.0).is_err());
        assert_eq!(balance(&reg, PLACER), RICH);
        assert_eq!(pool(reg.deltas(), TARGET), 0.0);
        assert!(reg.journal().entries().is_empty());
        // Something that is no character, corporation or alliance.
        assert!(place(&mut reg, 7_999, MIN_BOUNTY_AMOUNT_ALLIANCE).is_err());
        // More than the wallet holds: refused whole.
        assert!(place(&mut reg, TARGET, RICH + 1.0).is_err());
        assert_eq!(balance(&reg, PLACER), RICH);
    }

    /// Two placements on one pilot: the pool sums, "my bounties" lists both
    /// newest first, the wallet is debited twice with ref 16, and the target
    /// is told twice. The answer names the pilot's corporation.
    #[test]
    fn two_placements_sum_and_are_listed_debited_and_notified() {
        let mut reg = world();
        let a = MIN_BOUNTY_AMOUNT_CHAR;
        let b = 2.0 * MIN_BOUNTY_AMOUNT_CHAR;
        let answer = place(&mut reg, TARGET, a).expect("placed");
        assert_eq!(attr(&answer, "targetID"), Some(&PyValue::Int(TARGET)));
        assert!(attr(&answer, "corporationID").is_some());
        assert!(attr(&answer, "allianceID").is_none(), "left out, not 0");
        let answer = place(&mut reg, TARGET, b).expect("placed");
        assert_eq!(attr(&answer, "bounty"), Some(&PyValue::Real(a + b)));
        assert_eq!(pool(reg.deltas(), TARGET), a + b);
        assert_eq!(balance(&reg, PLACER), RICH - a - b);

        let mine = call(&mut reg, "GetMyBounties", PLACER, vec![]).expect("answered");
        let ids: Vec<i64> = rows_of(&mine)
            .iter()
            .map(|r| match attr(r, "contributionID") {
                Some(PyValue::Int(i)) => *i,
                other => panic!("contributionID {other:?}"),
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] > ids[1], "newest first, ids increasing");
        let theirs = call(&mut reg, "GetMyBounties", TARGET, vec![]).expect("answered");
        assert!(rows_of(&theirs).is_empty());

        let debits: Vec<_> = reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == REF_BOUNTY)
            .collect();
        assert_eq!(debits.len(), 2);
        assert!(
            debits
                .iter()
                .all(|e| { e.owner_id_1 == PLACER && e.reference_id == TARGET && e.amount < 0.0 })
        );
        assert_eq!(notes(&reg, TARGET, kind::CHAR), 2);
        assert_eq!(notes(&reg, PLACER, kind::CHAR), 0);
    }

    /// The top list is a `(rows, filetime)` pair ordered by bounty; a search
    /// answers `(rank, row)`; `GetBounties` and `GetBountiesAndKillRights`
    /// carry the pool.
    #[test]
    fn top_search_and_bounties_read_the_pools() {
        let mut reg = world();
        place(&mut reg, TARGET, MIN_BOUNTY_AMOUNT_CHAR).expect("placed");
        place(&mut reg, OTHER, 3.0 * MIN_BOUNTY_AMOUNT_CHAR).expect("placed");
        let got = call(
            &mut reg,
            "GetTopPilotBounties",
            PLACER,
            vec![PyValue::Int(0)],
        )
        .expect("answered");
        let PyValue::Tuple(parts) = got else {
            panic!("a 2-tuple")
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(parts[1], PyValue::Int(_)));
        let order: Vec<_> = rows_of(&parts[0])
            .iter()
            .map(|r| attr(r, "targetID").cloned())
            .collect();
        assert_eq!(
            order,
            vec![Some(PyValue::Int(OTHER)), Some(PyValue::Int(TARGET))]
        );
        let found = call(
            &mut reg,
            "SearchCharBounties",
            PLACER,
            vec![PyValue::Int(TARGET)],
        )
        .expect("answered");
        let PyValue::Tuple(hit) = &rows_of(&found)[0] else {
            panic!("a (rank, row) pair")
        };
        assert_eq!(hit[0], PyValue::Int(1));
        let none = call(
            &mut reg,
            "SearchCharBounties",
            PLACER,
            vec![PyValue::Int(HUNTER)],
        )
        .expect("answered");
        assert!(rows_of(&none).is_empty());

        let both = call(
            &mut reg,
            "GetBountiesAndKillRights",
            PLACER,
            vec![
                PyValue::List(vec![PyValue::Int(TARGET)]),
                PyValue::List(vec![]),
            ],
        )
        .expect("answered");
        let PyValue::Tuple(both) = both else {
            panic!("a pair")
        };
        let PyValue::Tuple(row) = &rows_of(&both[0])[0] else {
            panic!("a (targetID, row) pair")
        };
        assert_eq!(
            attr(&row[1], "bounty"),
            Some(&PyValue::Real(MIN_BOUNTY_AMOUNT_CHAR))
        );
    }

    /// A corporation's members, but not the placer, are told (113).
    #[test]
    fn a_corp_target_notifies_every_member_but_the_placer() {
        let mut reg = world();
        for who in [OTHER, TARGET, PLACER] {
            crate::service::corp::dev_join(&mut reg, who, "BNTY");
        }
        let corp = reg.character(OTHER).expect("a pilot").corporation_id;
        assert_eq!(owner_kind(&reg, corp), Some(Owner::Corp));
        place(&mut reg, corp, MIN_BOUNTY_AMOUNT_CORP - 1.0).expect_err("below the corp minimum");
        place(&mut reg, corp, MIN_BOUNTY_AMOUNT_CORP).expect("placed");
        assert_eq!(notes(&reg, OTHER, kind::CORP), 1);
        assert_eq!(notes(&reg, TARGET, kind::CORP), 1);
        assert_eq!(notes(&reg, PLACER, kind::CORP), 0, "not the placer");
        assert_eq!(notes(&reg, OTHER, kind::CHAR), 0);
    }

    /// Pools, contributions and hunter stats are deltas: a world rebuilt from
    /// the rows reads the same.
    #[test]
    fn placed_bounties_survive_a_restart() {
        let mut reg = world();
        place(&mut reg, TARGET, MIN_BOUNTY_AMOUNT_CHAR).expect("placed");
        place(&mut reg, TARGET, MIN_BOUNTY_AMOUNT_CHAR).expect("placed");
        add_hunt(reg.deltas_mut(), HUNTER, 123.0);
        let mut again = world();
        for k in [KIND_POOL, KIND_GIVE, KIND_HUNT] {
            for (key, v) in reg.deltas().of(k).clone() {
                again.deltas_mut().put(k, key, v);
            }
        }
        assert_eq!(pool(again.deltas(), TARGET), 2.0 * MIN_BOUNTY_AMOUNT_CHAR);
        let (before, after) = (
            call(&mut reg, "GetMyBounties", PLACER, vec![]).expect("answered"),
            call(&mut again, "GetMyBounties", PLACER, vec![]).expect("answered"),
        );
        assert_eq!(rows_of(&after).len(), 2);
        assert_eq!(before, after);
        assert_eq!(hunter(again.deltas(), HUNTER), (123.0, 1));
        // The allocator carries on past the saved ids.
        let next = next_give_id(again.deltas_mut());
        assert!(gives(again.deltas()).iter().all(|g| g.id < next));
    }

    // ---- the payout ------------------------------------------------------------

    use crate::killmail::{Attacker, Report};

    const LOSS: f64 = 10_000.0;
    const WHEN: BlueTime = 1_234_567;

    fn attacker(character: Option<i64>, damage: i64) -> Attacker {
        Attacker {
            character,
            corporation: Some(1_000_009),
            alliance: None,
            faction: None,
            ship_type: None,
            weapon_type: None,
            damage,
            security: None,
        }
    }

    /// A report of TARGET's loss with `final_blow` and `others`, kept so the
    /// hook finds it.
    fn report(reg: &mut Registry, final_blow: Attacker, others: Vec<Attacker>) -> KillEvent {
        let r = Report {
            kill_id: 1,
            time: WHEN,
            system: 5,
            victim: TARGET,
            victim_corporation: reg.character(TARGET).expect("a pilot").corporation_id,
            victim_alliance: None,
            ship_type: 587,
            damage_taken: 100,
            isk_lost: Some(LOSS),
            isk_destroyed: Some(LOSS),
            final_blow,
            attackers: others,
            items: Vec::new(),
            bounty_claimed: None,
            kill_right_supplied: None,
        };
        crate::killmail::put(reg, &r);
        KillEvent {
            victim_character: TARGET,
            victim_ship: 1,
            ship_type: 587,
            killer: Some(HUNTER),
            system: 5,
            npc: None,
            when: WHEN,
            park: Vec::new(),
        }
    }

    /// A kill claims the victim's pool, up to the claim share of the loss,
    /// pays the attacker, marks the report, tells both, and a smaller pool
    /// is emptied.
    #[test]
    fn a_kill_claims_the_pool_pays_the_hunter_and_marks_the_report() {
        let mut reg = world();
        let big = 100.0 * LOSS;
        place(&mut reg, TARGET, big.max(MIN_BOUNTY_AMOUNT_CHAR)).expect("placed");
        let before = pool(reg.deltas(), TARGET);
        let hunter_before = balance(&reg, HUNTER);
        let ev = report(&mut reg, attacker(Some(HUNTER), 100), vec![]);
        on_kill(&mut reg, &ev);

        let after = pool(reg.deltas(), TARGET);
        let taken = before - after;
        assert!(taken > 0.0, "the pool shrank");
        assert!(
            (taken - CLAIM_FRACTION * LOSS).abs() < 1e-6,
            "capped by the rule"
        );
        assert!((balance(&reg, HUNTER) - hunter_before - taken).abs() < 1e-6);
        let prize: Vec<_> = reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == REF_BOUNTY_PRIZE)
            .collect();
        assert_eq!(prize.len(), 1);
        assert_eq!(
            (prize[0].owner_id_2, prize[0].reference_id),
            (HUNTER, TARGET)
        );
        let kept = crate::killmail::get(&reg, 1).expect("kept");
        assert_eq!(kept.bounty_claimed, Some(taken));
        assert_eq!(notes(&reg, TARGET, kind::BOUNTY_YOUR_BOUNTY_CLAIMED), 1);
        assert_eq!(notes(&reg, HUNTER, kind::BOUNTY_CLAIM_MSG), 1);
        let (claimed, kills) = hunter(reg.deltas(), HUNTER);
        assert!((claimed - taken).abs() < 1e-6 && kills == 1);
        // "My bounties" shrank with the pool.
        let left: f64 = gives(reg.deltas()).iter().map(|g| g.amount).sum();
        assert!((left - after).abs() < 1e-3);
    }

    /// A pool smaller than the cap is emptied, and its row goes.
    #[test]
    fn a_small_pool_is_emptied() {
        let mut reg = world();
        let small = CLAIM_FRACTION * LOSS / 2.0;
        set_pool(reg.deltas_mut(), TARGET, small, WHEN);
        let ev = report(&mut reg, attacker(Some(HUNTER), 1), vec![]);
        on_kill(&mut reg, &ev);
        assert_eq!(pool(reg.deltas(), TARGET), 0.0);
        assert!(reg.deltas().of(KIND_POOL).is_empty());
        assert_eq!(
            crate::killmail::get(&reg, 1).unwrap().bounty_claimed,
            Some(small)
        );
    }

    /// The split follows damage, whatever the sum; the parts add up.
    #[test]
    fn the_split_follows_damage() {
        let parts = split(1000.0, &[(1, 75.0), (2, 25.0)]);
        assert!((parts[0].1 - 3.0 * parts[1].1).abs() < 1e-9);
        assert!((parts.iter().map(|p| p.1).sum::<f64>() - 1000.0).abs() < 1e-9);
        let even = split(100.0, &[(1, 0.0), (2, 0.0)]);
        assert_eq!(even[0].1, even[1].1);

        let mut reg = world();
        set_pool(reg.deltas_mut(), TARGET, 1.0e9, WHEN);
        let ev = report(
            &mut reg,
            attacker(Some(HUNTER), 75),
            vec![attacker(Some(HUNTER2), 25)],
        );
        on_kill(&mut reg, &ev);
        let a = hunter(reg.deltas(), HUNTER).0;
        let b = hunter(reg.deltas(), HUNTER2).0;
        assert!(
            a > 0.0 && b > 0.0,
            "both are paid, not the final blow alone"
        );
        assert!((a - 3.0 * b).abs() < 1e-6);
    }

    /// An NPC or CONCORD final blow pays nothing and leaves the pool; so
    /// does a death with no bounty.
    #[test]
    fn no_character_final_blow_no_payout() {
        let mut reg = world();
        set_pool(reg.deltas_mut(), TARGET, 1.0e9, WHEN);
        let ev = report(
            &mut reg,
            attacker(None, 100),
            vec![attacker(Some(HUNTER), 50)],
        );
        on_kill(&mut reg, &ev);
        assert_eq!(pool(reg.deltas(), TARGET), 1.0e9);
        assert!(
            crate::killmail::get(&reg, 1)
                .unwrap()
                .bounty_claimed
                .is_none()
        );
        assert_eq!(notes(&reg, TARGET, kind::BOUNTY_YOUR_BOUNTY_CLAIMED), 0);
    }

    /// Nobody is paid for their own death.
    #[test]
    fn the_victim_is_never_a_hunter() {
        let mut reg = world();
        set_pool(reg.deltas_mut(), TARGET, 1.0e9, WHEN);
        let ev = report(&mut reg, attacker(Some(TARGET), 100), vec![]);
        on_kill(&mut reg, &ev);
        assert_eq!(pool(reg.deltas(), TARGET), 1.0e9);
    }
}
