//! **LP transfers to a corporation** (GAPS-2, X-GAPS item 4; lane note
//! `docs/lanes/gaps-2.md`): giving loyalty points to a player corporation,
//! from a character's own wallet or from a corporation's.
//!
//! The wallet window's "Donate LP" tabs (`personalDonateLPPanel`,
//! `corpDonateLPPanel`, both on `donateLPPanel`) pick one of the donor's
//! issuer corporations and a receiving corporation, and the service sends
//! one of two calls whose answers are dropped [V]
//! (`loyaltyPointsWalletSvc.py:107-123`):
//!
//! - `LPSvc.TransferLPFromMyWalletToOtherCorp(receiverCorpID, issuerCorpID,
//!   lpAmount)` -- from the caller's own LP with that issuer;
//! - `LPSvc.TransferLPFromMyCorpWalletToOtherCorp(receiverCorpID,
//!   issuerCorpID, lpAmount)` -- from the caller's corporation's LP.
//!
//! The panels offer only player corporations that have members
//! (`_filter_valid_corps`: `IsPlayerCorporation` and `memberCount > 0`), and
//! for the corporate tab not the donor's own corporation [V]. The balances
//! the panels show are `LPSvc.GetAllMyCharacterWalletLPBalances` (answered
//! since CP37b) and `GetAllMyCorporationWalletLPBalances` (an empty list
//! until now: [`corp_balances`]), as `(issuerCorpID, balance)` pairs
//! (`loyaltyPointsWalletSvc._LoadCorporationLPData:46-49`) [V]. The client
//! refreshes on pushes: `OnLPChange(changeType, issuerCorpID, lpBefore,
//! lpAfter)` for a character's wallet (`crate::loyalty::LpChange`) and
//! `OnCorpLPChange(reason, issuerCorpID)` for a corporation's (`:61`) [V].
//!
//! **What this server adds**, stored as delta kind [`KIND`]
//! (`receiving_corp issuer_corp -> balance`, only non-zero rows): a
//! corporation's LP wallet, which nothing credited before. It is credited by
//! these transfers and nothing else today.
//!
//! The rules are ours **[A]**, the client names none (it checks nothing but
//! the amount against the balance it shows, `donateLPPanel._transfer_lp`):
//!
//! - the amount is positive and not more than the donor holds;
//! - the receiver is a live player corporation, and not the donor's own for a
//!   corporate donation;
//! - a corporation's LP is moved by a director or an accountant
//!   ([`may_move_corp_lp`]), the wallet's own view of who handles the
//!   corporation's money (`corp_wallet::Standing`).
//!
//! Falsified by a retail refusal of a transfer these accept (or the reverse).
//! A refusal is `CustomNotify`; nothing moves.

use crate::loyalty;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::collections::BTreeMap;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "corplp";

/// `OnLPChange`'s first argument for a transfer. The handler discards it
/// (`loyaltyPointsWalletSvc.OnLPChange:67-69`) [V]; the name is ours.
pub const CHANGE_TRANSFER: &str = "transfer";

fn key(corp: i64, issuer: i64) -> Vec<String> {
    vec![corp.to_string(), issuer.to_string()]
}

/// A corporation's LP with one issuer; 0 when there is no row.
pub fn corp_balance(reg: &Registry, corp: i64, issuer: i64) -> i64 {
    reg.deltas()
        .of(KIND)
        .get(&key(corp, issuer))
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Every non-zero balance of a corporation, `(issuer, balance)` by issuer.
pub fn corp_balances(reg: &Registry, corp: i64) -> Vec<(i64, i64)> {
    let me = corp.to_string();
    reg.deltas()
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.first() == Some(&me))
        .filter_map(|(k, v)| {
            let issuer = k.get(1)?.parse().ok()?;
            let bal: i64 = v.first()?.parse().ok()?;
            (bal != 0).then_some((issuer, bal))
        })
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect()
}

/// Add `delta` (negative debits) to a corporation's LP; `None`, nothing
/// changed, when it would go below zero or overflow.
fn corp_adjust(reg: &mut Registry, corp: i64, issuer: i64, delta: i64) -> Option<(i64, i64)> {
    let before = corp_balance(reg, corp, issuer);
    let after = before.checked_add(delta).filter(|a| *a >= 0)?;
    if delta != 0 {
        if after == 0 {
            reg.deltas_mut().del(KIND, &key(corp, issuer));
        } else {
            reg.deltas_mut()
                .put(KIND, key(corp, issuer), vec![after.to_string()]);
        }
    }
    Some((before, after))
}

fn refuse(text: &str) -> ServiceError {
    ServiceError::Raise(eve_proto::user_error(
        "CustomNotify",
        crate::pyutil::pypairs(vec![("notify", crate::pyutil::bstr(text))]),
    ))
}

/// The player corporation `ch` is in, and whether they may move its LP.
pub fn may_move_corp_lp(reg: &Registry, ch: i64) -> Option<(i64, bool)> {
    let m = reg.corporations().membership(ch)?;
    let corp = reg
        .corporations()
        .get(m.corporation_id)
        .filter(|c| !c.deleted)?;
    let roles = m.roles | m.roles_at_other;
    let allowed = corp.ceo_id == ch
        || roles & (crate::corp_wallet::ROLE_DIRECTOR | crate::corp_wallet::ROLE_ACCOUNTANT) != 0;
    Some((corp.corporation_id, allowed))
}

/// A live player corporation with members: what the panels offer.
fn receivable(reg: &Registry, corp: i64) -> bool {
    reg.corporations().get(corp).is_some_and(|c| !c.deleted)
        && !reg.corporations().members_of(corp).is_empty()
}

fn checked(amount: Option<i64>) -> Result<i64, ServiceError> {
    match amount {
        Some(n) if n > 0 => Ok(n),
        _ => Err(refuse("That is not an amount of loyalty points.")),
    }
}

/// `TransferLPFromMyWalletToOtherCorp(receiver, issuer, amount)`.
pub fn from_player(
    reg: &mut Registry,
    actor: Option<i64>,
    receiver: i64,
    issuer: i64,
    amount: Option<i64>,
) -> Result<(), ServiceError> {
    let amount = checked(amount)?;
    let Some(me) = actor else {
        return Err(refuse("No character is selected."));
    };
    if !receivable(reg, receiver) {
        return Err(refuse("That corporation cannot receive loyalty points."));
    }
    if loyalty::balance(reg, me, issuer) < amount {
        return Err(refuse("You do not have that many loyalty points."));
    }
    let Some((_, after)) = corp_adjust(reg, receiver, issuer, amount) else {
        return Err(refuse("That corporation cannot hold more."));
    };
    // Cannot fail: the balance was checked above.
    loyalty::adjust(reg, me, issuer, -amount, CHANGE_TRANSFER);
    tell_corps(reg, &[(receiver, issuer)]);
    let _ = after;
    Ok(())
}

/// `TransferLPFromMyCorpWalletToOtherCorp(receiver, issuer, amount)`.
pub fn from_corp(
    reg: &mut Registry,
    actor: Option<i64>,
    receiver: i64,
    issuer: i64,
    amount: Option<i64>,
) -> Result<(), ServiceError> {
    let amount = checked(amount)?;
    let Some(me) = actor else {
        return Err(refuse("No character is selected."));
    };
    let Some((mine, allowed)) = may_move_corp_lp(reg, me) else {
        return Err(refuse("Only a member of a player corporation can do that."));
    };
    if !allowed {
        return Err(refuse(
            "You need the Director or Accountant role to move the corporation's loyalty points.",
        ));
    }
    if receiver == mine || !receivable(reg, receiver) {
        return Err(refuse("That corporation cannot receive loyalty points."));
    }
    if corp_balance(reg, mine, issuer) < amount {
        return Err(refuse(
            "The corporation does not have that many loyalty points.",
        ));
    }
    if corp_adjust(reg, receiver, issuer, amount).is_none() {
        return Err(refuse("That corporation cannot hold more."));
    }
    corp_adjust(reg, mine, issuer, -amount);
    tell_corps(reg, &[(mine, issuer), (receiver, issuer)]);
    Ok(())
}

/// `OnCorpLPChange(reason, issuerCorpID)` to every member of each corporation.
fn tell_corps(reg: &mut Registry, which: &[(i64, i64)]) {
    for &(corp, issuer) in which {
        let to = reg.corporations().members_of(corp);
        if !to.is_empty() {
            reg.push(CorpLpChange {
                to,
                issuer_corp_id: issuer,
            });
        }
    }
}

/// `OnCorpLPChange(reason, issuerCorpID)` [V] (`loyaltyPointsWalletSvc.py:61`:
/// two arguments; the one-argument form raised `TypeError` in the client,
/// found by `fci_gaps2` 2026-10-28). `reason` is unused by the client; a string like `OnLPChange`'s change type [A].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpLpChange {
    pub to: Vec<i64>,
    pub issuer_corp_id: i64,
}

impl Push for CorpLpChange {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCorpLPChange",
            vec![
                crate::pyutil::bstr("donation"),
                PyValue::Int(self.issuer_corp_id),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// `GetAllMyCorporationWalletLPBalances`: `(issuer, balance)` tuples of the
/// caller's corporation, `[]` for a character in none.
pub fn corp_balances_value(reg: &Registry, actor: Option<i64>) -> PyValue {
    let rows = actor
        .and_then(|a| reg.corporations().membership(a))
        .map(|m| corp_balances(reg, m.corporation_id))
        .unwrap_or_default();
    PyValue::List(
        rows.into_iter()
            .map(|(issuer, bal)| PyValue::Tuple(vec![PyValue::Int(issuer), PyValue::Int(bal)]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's `OnCorpLPChange(self, reason, issuerCorpID)` takes two
    /// arguments; one raised a `TypeError` in the real client (FCI-FIX2).
    #[test]
    fn the_corp_lp_push_carries_reason_and_issuer() {
        let p = CorpLpChange {
            to: vec![1],
            issuer_corp_id: 1_000_077,
        };
        assert_eq!(
            p.to_push(),
            (
                "OnCorpLPChange",
                vec![crate::pyutil::bstr("donation"), PyValue::Int(1_000_077)]
            )
        );
    }
}
