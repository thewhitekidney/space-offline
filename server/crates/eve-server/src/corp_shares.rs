//! **Corporation shares and dividends** (lane `s7`, #178; `r-corp-social`
//! §5 and S7): who holds how many of a corp's shares, and how a dividend is
//! split. The wire is `service/corp_shares.rs`; this is the store and the
//! arithmetic, with no registry.
//!
//! One delta kind carries the state:
//!
//! | kind | key | value |
//! | --- | --- | --- |
//! | [`KIND`] | `corp holder` | `n`, shares the holder (a character or a corp) keeps |
//! | [`KIND`] | `corp #` | `n`, the shares issued: the marker that the corp is founded |
//!
//! A holder with no shares has no row. The issued total is fixed at
//! [`FOUNDING_SHARES`]: creating more is a vote (S10, parked), so shares only
//! move. A corp founded before this lane has no marker; [`found`] grants it on
//! the first share call, so an old world needs no migration.
//!
//! **Dividends are whole cents.** A payout is divided in integer cents and
//! the remainder (under one cent per payee) stays in the wallet; money is
//! never created or lost by rounding. A holder's cut is
//! `cents * shares / issued`, so shares the corp keeps itself, and shares
//! held by anyone who cannot be paid, stay in the wallet too.

use crate::delta::Deltas;

/// The shares delta kind (`delta_kinds::KINDS`).
pub const KIND: &str = "corpshare";

/// Shares issued to the CEO when a corp is founded **[A: 1000, as commonly
/// remembered; the client states no figure; falsifier: reference data]**.
pub const FOUNDING_SHARES: i64 = 1000;

/// The key part that marks a founded corp and holds its issued total.
const MARKER: &str = "#";

/// Why a move is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Zero or fewer shares.
    NothingToMove,
    /// More shares than the giver keeps.
    MoreThanHeld { held: i64 },
    /// The giver and the taker are one.
    ToSelf,
}

/// One holder's change in one move: `(old, new)` share counts, where `0`
/// means no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub holder: i64,
    pub old: i64,
    pub new: i64,
}

fn key(corp: i64, holder: &str) -> Vec<String> {
    vec![corp.to_string(), holder.to_owned()]
}

fn count(v: Option<&Vec<String>>) -> i64 {
    v.and_then(|v| v.first()?.parse().ok()).unwrap_or(0)
}

/// Whether `corp` has its shares issued yet.
pub fn is_founded(d: &Deltas, corp: i64) -> bool {
    d.of(KIND).contains_key(&key(corp, MARKER))
}

/// Issue [`FOUNDING_SHARES`] of `corp` to `ceo`, once. A corp that already
/// has its marker is left as it is, whoever holds the shares now.
pub fn found(d: &mut Deltas, corp: i64, ceo: i64) {
    if is_founded(d, corp) {
        return;
    }
    d.put(KIND, key(corp, MARKER), vec![FOUNDING_SHARES.to_string()]);
    d.put(
        KIND,
        key(corp, &ceo.to_string()),
        vec![FOUNDING_SHARES.to_string()],
    );
}

/// What `holder` keeps of `corp`.
pub fn held(d: &Deltas, corp: i64, holder: i64) -> i64 {
    count(d.of(KIND).get(&key(corp, &holder.to_string())))
}

/// The shares issued by `corp`: zero while it is not founded.
pub fn issued(d: &Deltas, corp: i64) -> i64 {
    count(d.of(KIND).get(&key(corp, MARKER)))
}

/// `(holder, shares)` for `corp`, lowest holder id first.
pub fn holders(d: &Deltas, corp: i64) -> Vec<(i64, i64)> {
    let c = corp.to_string();
    let mut v: Vec<(i64, i64)> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[0] == c && k[1] != MARKER)
        .filter_map(|(k, row)| Some((k[1].parse().ok()?, count(Some(row)))))
        .filter(|(_, n)| *n > 0)
        .collect();
    v.sort();
    v
}

/// `(corp, shares)` that `holder` keeps, lowest corp id first.
pub fn held_by(d: &Deltas, holder: i64) -> Vec<(i64, i64)> {
    let h = holder.to_string();
    let mut v: Vec<(i64, i64)> = d
        .of(KIND)
        .iter()
        .filter(|(k, _)| k.len() == 2 && k[1] == h)
        .filter_map(|(k, row)| Some((k[0].parse().ok()?, count(Some(row)))))
        .filter(|(_, n)| *n > 0)
        .collect();
    v.sort();
    v
}

fn set(d: &mut Deltas, corp: i64, holder: i64, n: i64) {
    let k = key(corp, &holder.to_string());
    if n > 0 {
        d.put(KIND, k, vec![n.to_string()]);
    } else {
        d.del(KIND, &k);
    }
}

/// Move `n` shares of `corp` from `from` to `to`. Nothing changes when it is
/// refused. Answers the giver's and the taker's change.
pub fn move_shares(
    d: &mut Deltas,
    corp: i64,
    from: i64,
    to: i64,
    n: i64,
) -> Result<(Change, Change), Refusal> {
    if n < 1 {
        return Err(Refusal::NothingToMove);
    }
    if from == to {
        return Err(Refusal::ToSelf);
    }
    let have = held(d, corp, from);
    if n > have {
        return Err(Refusal::MoreThanHeld { held: have });
    }
    let there = held(d, corp, to);
    set(d, corp, from, have - n);
    set(d, corp, to, there + n);
    Ok((
        Change {
            holder: from,
            old: have,
            new: have - n,
        },
        Change {
            holder: to,
            old: there,
            new: there + n,
        },
    ))
}

/// Split `cents` over `holders` (`(who, shares)`) by weight against `issued`
/// shares: each gets `cents * shares / issued`, rounded down. Holders owed
/// nothing are left out; the rest of `cents` is not paid to anyone.
pub fn split_by_weight(cents: i64, holders: &[(i64, i64)], issued: i64) -> Vec<(i64, i64)> {
    if cents < 1 || issued < 1 {
        return Vec::new();
    }
    holders
        .iter()
        .filter(|(_, n)| *n > 0)
        .filter_map(|(who, n)| {
            let cut = (i128::from(cents) * i128::from(*n) / i128::from(issued)) as i64;
            (cut > 0).then_some((*who, cut))
        })
        .collect()
}

/// Split `cents` evenly over `members`, rounded down to a whole cent each.
/// Nobody is paid when the cut would be under a cent.
pub fn split_evenly(cents: i64, members: &[i64]) -> Vec<(i64, i64)> {
    let n = members.len() as i64;
    if n == 0 || cents / n < 1 {
        return Vec::new();
    }
    members.iter().map(|m| (*m, cents / n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented ids: nothing here is a client row.
    const CORP: i64 = 98_000_781;
    const OTHER: i64 = 98_000_782;
    const ALICE: i64 = 90_000_781;
    const BOB: i64 = 90_000_782;
    const CAROL: i64 = 90_000_783;

    #[test]
    fn founding_grants_once() {
        let mut d = Deltas::default();
        assert!(!is_founded(&d, CORP));
        found(&mut d, CORP, ALICE);
        assert_eq!(held(&d, CORP, ALICE), FOUNDING_SHARES);
        assert_eq!(issued(&d, CORP), FOUNDING_SHARES);
        let four_tenths = FOUNDING_SHARES * 4 / 10;
        move_shares(&mut d, CORP, ALICE, BOB, four_tenths).unwrap();
        // A second call, even for another CEO, changes nothing.
        found(&mut d, CORP, CAROL);
        assert_eq!(held(&d, CORP, CAROL), 0);
        assert_eq!(held(&d, CORP, ALICE), FOUNDING_SHARES - four_tenths);
        assert_eq!(
            holders(&d, CORP),
            vec![(ALICE, FOUNDING_SHARES - four_tenths), (BOB, four_tenths)]
        );
    }

    #[test]
    fn a_move_leaves_the_total_alone() {
        let mut d = Deltas::default();
        found(&mut d, CORP, ALICE);
        let all = FOUNDING_SHARES;
        let (a, b) = move_shares(&mut d, CORP, ALICE, BOB, all / 4).unwrap();
        assert_eq!(
            (a.old, a.new, b.old, b.new),
            (all, all - all / 4, 0, all / 4)
        );
        move_shares(&mut d, CORP, ALICE, BOB, all - all / 4).unwrap();
        // A holder at zero has no row.
        assert_eq!(held_by(&d, ALICE), vec![]);
        assert_eq!(held_by(&d, BOB), vec![(CORP, FOUNDING_SHARES)]);
        let sum: i64 = holders(&d, CORP).iter().map(|h| h.1).sum();
        assert_eq!(sum, issued(&d, CORP));
    }

    #[test]
    fn moving_more_than_held_is_refused() {
        let mut d = Deltas::default();
        found(&mut d, CORP, ALICE);
        assert_eq!(
            move_shares(&mut d, CORP, ALICE, BOB, FOUNDING_SHARES + 1),
            Err(Refusal::MoreThanHeld {
                held: FOUNDING_SHARES
            })
        );
        assert_eq!(
            move_shares(&mut d, CORP, BOB, ALICE, 1),
            Err(Refusal::MoreThanHeld { held: 0 })
        );
        assert_eq!(
            move_shares(&mut d, CORP, ALICE, BOB, 0),
            Err(Refusal::NothingToMove)
        );
        assert_eq!(
            move_shares(&mut d, CORP, ALICE, BOB, -5),
            Err(Refusal::NothingToMove)
        );
        assert_eq!(
            move_shares(&mut d, CORP, ALICE, ALICE, 1),
            Err(Refusal::ToSelf)
        );
        // Nothing moved by any of them.
        assert_eq!(held(&d, CORP, ALICE), FOUNDING_SHARES);
        assert_eq!(held(&d, CORP, BOB), 0);
    }

    #[test]
    fn holdings_are_per_corporation() {
        let mut d = Deltas::default();
        found(&mut d, CORP, ALICE);
        found(&mut d, OTHER, ALICE);
        move_shares(&mut d, CORP, ALICE, BOB, 10).unwrap();
        assert_eq!(held(&d, OTHER, ALICE), FOUNDING_SHARES);
        assert_eq!(
            held_by(&d, ALICE),
            vec![(CORP, FOUNDING_SHARES - 10), (OTHER, FOUNDING_SHARES)]
        );
    }

    #[test]
    fn shareholders_are_paid_by_weight() {
        // 1000 issued: 600 and 300 held by payees, 100 kept by the corp.
        let paid = split_by_weight(10_000, &[(ALICE, 600), (BOB, 300)], 1000);
        assert_eq!(paid, vec![(ALICE, 6000), (BOB, 3000)]);
        // The 100 shares nobody can be paid for leave 1000 cents behind.
        let sum: i64 = paid.iter().map(|p| p.1).sum();
        assert_eq!(10_000 - sum, 1000);
        // Rounding is down and never over: 1 cent over 3 shares of 1000.
        let paid = split_by_weight(999, &[(ALICE, 500), (BOB, 499), (CAROL, 1)], 1000);
        assert_eq!(paid, vec![(ALICE, 499), (BOB, 498)]);
        assert!(paid.iter().map(|p| p.1).sum::<i64>() <= 999);
    }

    #[test]
    fn a_huge_payout_does_not_overflow() {
        let paid = split_by_weight(i64::MAX, &[(ALICE, 1000)], 1000);
        assert_eq!(paid, vec![(ALICE, i64::MAX)]);
    }

    #[test]
    fn members_split_in_whole_cents() {
        // 1000 cents over three: 333 each, one cent left behind.
        let paid = split_evenly(1000, &[ALICE, BOB, CAROL]);
        assert_eq!(paid, vec![(ALICE, 333), (BOB, 333), (CAROL, 333)]);
        assert_eq!(1000 - paid.iter().map(|p| p.1).sum::<i64>(), 1);
        // Under a cent each pays nobody.
        assert_eq!(split_evenly(2, &[ALICE, BOB, CAROL]), vec![]);
        assert_eq!(split_evenly(100, &[]), vec![]);
    }
}
