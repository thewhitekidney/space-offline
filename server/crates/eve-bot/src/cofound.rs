//! **CO-FOUND**: the co-founding pool. Bot corps never formed because the
//! fee plus [`society::FOUND_RESERVE`] (about 1.85 M ISK) is more than a
//! bot earns (`docs/lanes/bot-tune.md` §2), and nothing may appear from
//! nowhere, so there are no grants. Traited bots instead **pool their own
//! ISK**: the others pledge to one chosen founder with real
//! `account.GiveCash` transfers; the founder founds when it holds the line
//! (the ordinary [`society::wants_to_found`] rule, which now sees a fatter
//! wallet), and the pledgers apply and are accepted. If nothing is founded
//! by the deadline, the founder refunds every pledge with real transfers.
//! Both directions are journalled and logged as society events.
//!
//! Everything here is **pure**: the planner ([`step`]) reads the pool and
//! the candidates and says what one bot does; `corpmind` executes it (the
//! one decision point is [`crate::corpmind::HallState::plan`]).
//!
//! The [A]s are named consts; each says what falsifies it.

use crate::corpmind::CORP_FEE;
use crate::society;

/// A pledger keeps this much for its own ship and ammo **[A]**. Falsified
/// by pledgers that cannot fly a ship after pledging.
pub const PLEDGE_FLOOR: f64 = 100_000.0;
/// A pledge is at most this share of the wallet above the floor **[A]**.
/// Falsified by a pool that never fills at the bots' real wallets (too
/// low) or by pledgers left unable to work (too high).
pub const PLEDGE_SHARE: f64 = 0.5;
/// No pledge below this (no dust) **[A]**.
pub const PLEDGE_MIN: f64 = 1_000.0;
/// A pool that has founded nothing this many game-days after its first
/// pledge is refunded **[A]**. Falsified by a founder that regularly
/// founds on day 8.
pub const POOL_DEADLINE_DAYS: f64 = 7.0;
/// The first pledge waits until the candidate set has been unchanged this
/// many game-days **[A]**: bots register with the hall as they log in, and
/// a founder chosen from the first few would be kept (pledges stand) even
/// when a better one registers a moment later. Falsified by a `pjournal`
/// whose founder is not the most diligent candidate of the settled set, or
/// by a pool that waits so long that nothing is ever pledged.
pub const SETTLE_DAYS: f64 = 0.25;
/// Journal rows kept.
pub const JOURNAL_KEEP: usize = 256;
/// The `GiveCash` reason texts (the client caps a reason at 40).
pub const PLEDGE_REASON: &str = "co-founding pledge";
pub const REFUND_REASON: &str = "co-founding refund";

/// One pledge: ISK moved from `from` to the founder `to`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pledge {
    pub from: i64,
    pub to: i64,
    pub amount: f64,
    pub at: u64,
}

/// What a journal row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A pledge paid, pledger to founder.
    Pledge,
    /// A pledge paid back, founder to pledger.
    Refund,
    /// A refund the founder could only part pay (it spent the rest).
    RefundShort,
    /// The founder founded: the pledge is the corp's.
    Spent,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pledge => "pledge",
            Kind::Refund => "refund",
            Kind::RefundShort => "refund-short",
            Kind::Spent => "spent",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        [Kind::Pledge, Kind::Refund, Kind::RefundShort, Kind::Spent]
            .into_iter()
            .find(|k| k.name() == s)
    }
}

/// One journal row.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub at: u64,
    pub kind: Kind,
    pub from: i64,
    pub to: i64,
    pub amount: f64,
}

/// A bot that could found and is corpless: the pool's candidates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cand {
    pub id: i64,
    pub diligence: f64,
    pub aggression: f64,
}

/// The pool (one per hall).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pool {
    /// The founder pledges go to; kept while pledges stand.
    pub founder: Option<i64>,
    /// When the first standing pledge was made.
    pub since: u64,
    pub pledges: Vec<Pledge>,
    /// Pledgers of a corp that was founded: `pledger -> corp`. They apply
    /// there and the CEO accepts them.
    pub pledged: std::collections::BTreeMap<i64, i64>,
    pub journal: Vec<Entry>,
    /// Each bot's last wallet read `(isk, when)`. Not persisted.
    pub wallets: std::collections::BTreeMap<i64, (f64, u64)>,
    /// The candidate ids last seen and when that set last changed (`None`
    /// until the hall has looked). Not persisted.
    pub cands: Vec<i64>,
    pub cands_at: Option<u64>,
}

/// What one bot does for the pool this turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Pledge {
        to: i64,
        amount: f64,
    },
    /// The founder pays every standing pledge back.
    Refund,
}

/// The line the founder must hold: the fee and the reserve.
pub fn line() -> f64 {
    CORP_FEE + society::FOUND_RESERVE
}

/// **The founder** (deterministic): the candidate with the most diligence,
/// then the most aggression, then the lowest id.
pub fn choose_founder(cands: &[Cand]) -> Option<i64> {
    cands
        .iter()
        .min_by(|a, b| {
            b.diligence
                .total_cmp(&a.diligence)
                .then(b.aggression.total_cmp(&a.aggression))
                .then(a.id.cmp(&b.id))
        })
        .map(|c| c.id)
}

/// **What still has to be pledged**: the line less the founder's wallet and
/// the pledges made since it was read.
pub fn gap(line: f64, founder_wallet: f64, unread_pledges: f64) -> f64 {
    (line - founder_wallet - unread_pledges).max(0.0)
}

/// **Pledge sizing**: [`PLEDGE_SHARE`] of the wallet above [`PLEDGE_FLOOR`],
/// never more than the gap, whole ISK; 0 below [`PLEDGE_MIN`].
pub fn pledge_amount(wallet: f64, gap: f64) -> f64 {
    let a = (((wallet - PLEDGE_FLOOR).max(0.0) * PLEDGE_SHARE).min(gap)).floor();
    if a >= PLEDGE_MIN { a } else { 0.0 }
}

/// **The found trigger**: the founder holds the line.
pub fn found_ready(wallet: f64) -> bool {
    wallet >= line()
}

/// Has the pool's deadline (on the game clock) passed?
pub fn deadline_passed(since: u64, now: u64, day: u64) -> bool {
    now.saturating_sub(since) as f64 >= POOL_DEADLINE_DAYS * day as f64
}

/// **Refund due**: pledges stand and either the deadline passed or the
/// founder can no longer found (corped, or its traits or the corp count
/// say no).
pub fn refund_due(standing: bool, founder_valid: bool, since: u64, now: u64, day: u64) -> bool {
    standing && (!founder_valid || deadline_passed(since, now, day))
}

/// **What `me` does for the pool** (pure). `cands` are the corpless
/// founding-trait bots; `my_wallet` is `me`'s wallet when read.
pub fn step(
    pool: &Pool,
    cands: &[Cand],
    me: i64,
    my_wallet: Option<f64>,
    now: u64,
    day: u64,
) -> Option<Step> {
    let me_cand = cands.iter().any(|c| c.id == me);
    let standing = !pool.pledges.is_empty();
    if standing && pool.founder == Some(me) {
        return refund_due(standing, me_cand, pool.since, now, day).then_some(Step::Refund);
    }
    if !me_cand {
        return None;
    }
    // A first pledge waits for the candidate set to settle, so the founder
    // is the best of everyone who is logging in, not of the first to arrive.
    if !standing
        && pool
            .cands_at
            .is_some_and(|t| (now.saturating_sub(t) as f64) < SETTLE_DAYS * day as f64)
    {
        return None;
    }
    let founder = if standing {
        pool.founder?
    } else {
        choose_founder(cands)?
    };
    if founder == me
        || !cands.iter().any(|c| c.id == founder)
        || (standing && deadline_passed(pool.since, now, day))
    {
        return None;
    }
    let (fw, read) = *pool.wallets.get(&founder)?;
    let unread: f64 = pool
        .pledges
        .iter()
        // `>=`: a pledge in the second of the read may or may not be in
        // it; counted twice at worst, which only delays a top-up.
        .filter(|p| p.to == founder && p.at >= read)
        .map(|p| p.amount)
        .sum();
    // Each of the other candidates covers its share of the gap, so
    // pledgers deciding at the same moment do not all pay all of it; a
    // short share is topped up on a later turn.
    // A bot that has pledged tops up toward the whole gap: halving it
    // again each turn would never reach the line.
    let topping_up = pool.pledges.iter().any(|p| p.from == me && p.to == founder);
    let others = if topping_up {
        1.0
    } else {
        cands.iter().filter(|c| c.id != founder).count().max(1) as f64
    };
    let amount = pledge_amount(my_wallet?, gap(line(), fw, unread) / others);
    (amount > 0.0).then_some(Step::Pledge {
        to: founder,
        amount,
    })
}

impl Pool {
    /// The hall saw these candidates at `now`; a changed set restarts the
    /// settling wait.
    pub fn observe(&mut self, ids: Vec<i64>, now: u64) {
        if self.cands_at.is_none() || self.cands != ids {
            self.cands = ids;
            self.cands_at = Some(now);
        }
    }

    fn note(&mut self, e: Entry) {
        if self.journal.len() >= JOURNAL_KEEP {
            self.journal.remove(0);
        }
        self.journal.push(e);
    }

    /// A pledge was paid.
    pub fn pledge(&mut self, from: i64, to: i64, amount: f64, now: u64) {
        if self.pledges.is_empty() {
            self.since = now;
        }
        self.founder = Some(to);
        self.pledges.push(Pledge {
            from,
            to,
            amount,
            at: now,
        });
        self.note(Entry {
            at: now,
            kind: Kind::Pledge,
            from,
            to,
            amount,
        });
    }

    /// `paid` of pledge `idx` went back; a part payment leaves the rest
    /// standing. Returns the pledge as it was.
    pub fn refunded(&mut self, idx: usize, paid: f64, now: u64) -> Pledge {
        let p = self.pledges[idx].clone();
        let short = paid < p.amount;
        if short {
            self.pledges[idx].amount -= paid;
        } else {
            self.pledges.remove(idx);
        }
        self.note(Entry {
            at: now,
            kind: if short {
                Kind::RefundShort
            } else {
                Kind::Refund
            },
            from: p.to,
            to: p.from,
            amount: paid,
        });
        if self.pledges.is_empty() {
            self.founder = None;
        }
        p
    }

    /// The founder founded `corp`: its pledges are spent, and the pledgers
    /// are its applicants.
    pub fn founded(&mut self, founder: i64, corp: i64, now: u64) {
        if self.founder != Some(founder) {
            return;
        }
        for p in std::mem::take(&mut self.pledges) {
            self.pledged.insert(p.from, corp);
            self.note(Entry {
                at: now,
                kind: Kind::Spent,
                from: p.from,
                to: p.to,
                amount: p.amount,
            });
        }
        self.founder = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 100;

    fn cand(id: i64, diligence: f64, aggression: f64) -> Cand {
        Cand {
            id,
            diligence,
            aggression,
        }
    }

    fn pool_with(founder: i64, wallet: f64) -> Pool {
        let mut p = Pool::default();
        p.wallets.insert(founder, (wallet, 0));
        p
    }

    #[test]
    fn the_founder_is_the_most_diligent_then_the_most_aggressive_then_the_lowest_id() {
        let cs = [cand(5, 0.7, 0.9), cand(3, 0.8, 0.1), cand(4, 0.8, 0.5)];
        assert_eq!(choose_founder(&cs), Some(4));
        let tie = [cand(9, 0.8, 0.5), cand(2, 0.8, 0.5)];
        assert_eq!(choose_founder(&tie), Some(2));
        assert_eq!(choose_founder(&[]), None);
    }

    #[test]
    fn a_pledge_is_a_share_above_the_floor_capped_by_the_gap_and_never_dust() {
        let w = PLEDGE_FLOOR + 400_000.0;
        assert_eq!(
            pledge_amount(w, f64::MAX),
            (400_000.0 * PLEDGE_SHARE).floor()
        );
        assert_eq!(pledge_amount(w, 5_000.0), 5_000.0);
        assert_eq!(pledge_amount(PLEDGE_FLOOR, 1e9), 0.0);
        assert_eq!(pledge_amount(w, PLEDGE_MIN - 1.0), 0.0);
        assert_eq!(gap(100.0, 60.0, 50.0), 0.0);
        assert_eq!(gap(100.0, 60.0, 10.0), 30.0);
    }

    #[test]
    fn the_others_pledge_to_the_founder_and_the_founder_does_not() {
        let cs = [cand(1, 0.9, 0.5), cand(2, 0.6, 0.5), cand(3, 0.5, 0.5)];
        let pool = pool_with(1, 1_000_000.0);
        let w = PLEDGE_FLOOR + 800_000.0;
        match step(&pool, &cs, 2, Some(w), 10, DAY) {
            Some(Step::Pledge { to: 1, amount }) => {
                assert_eq!(
                    amount,
                    pledge_amount(w, gap(line(), 1_000_000.0, 0.0) / 2.0)
                )
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(step(&pool, &cs, 1, Some(w), 10, DAY), None);
        // The founder's wallet unread: nobody pledges blind.
        assert_eq!(step(&Pool::default(), &cs, 2, Some(w), 10, DAY), None);
        // A corped bot (not a candidate) does not pledge.
        assert_eq!(step(&pool, &cs, 9, Some(w), 10, DAY), None);
    }

    #[test]
    fn pledges_made_since_the_founders_read_count_against_the_gap() {
        let cs = [cand(1, 0.9, 0.5), cand(2, 0.6, 0.5)];
        let mut pool = pool_with(1, 1_000_000.0);
        let need = line() - 1_000_000.0;
        pool.pledge(3, 1, need, 5);
        assert_eq!(step(&pool, &cs, 2, Some(9e9), 6, DAY), None);
        // Once the founder has read a wallet that holds them, the line
        // is met and the found trigger fires.
        pool.wallets.insert(1, (1_000_000.0 + need, 7));
        assert!(found_ready(1_000_000.0 + need));
        assert!(!found_ready(1_000_000.0 + need - 1.0));
        assert_eq!(step(&pool, &cs, 2, Some(9e9), 8, DAY), None);
    }

    #[test]
    fn a_first_pledge_waits_for_the_candidate_set_to_settle() {
        let mut pool = Pool::default();
        let late = [cand(1, 0.9, 0.5), cand(2, 0.6, 0.5), cand(3, 0.7, 0.5)];
        // Two candidates are seen at 10, the best one registers at 11.
        pool.observe(vec![2, 3], 10);
        pool.observe(vec![1, 2, 3], 11);
        pool.observe(vec![1, 2, 3], 12);
        assert_eq!(pool.cands_at, Some(11), "an unchanged set does not restart");
        pool.wallets.insert(1, (0.0, 12));
        let settled = 11 + (SETTLE_DAYS * DAY as f64).ceil() as u64;
        assert_eq!(step(&pool, &late, 2, Some(9e9), settled - 1, DAY), None);
        assert!(matches!(
            step(&pool, &late, 2, Some(9e9), settled, DAY),
            Some(Step::Pledge { to: 1, .. })
        ));
    }

    #[test]
    fn at_the_deadline_the_founder_refunds_and_nobody_pledges_more() {
        let cs = [cand(1, 0.9, 0.5), cand(2, 0.6, 0.5)];
        let mut pool = pool_with(1, 1_000_000.0);
        pool.pledge(2, 1, 50_000.0, 100);
        let early = 100 + (POOL_DEADLINE_DAYS * DAY as f64) as u64 - 1;
        let due = early + 1;
        assert_eq!(step(&pool, &cs, 1, Some(1e6), early, DAY), None);
        assert_eq!(step(&pool, &cs, 1, Some(1e6), due, DAY), Some(Step::Refund));
        assert_eq!(step(&pool, &cs, 2, Some(9e9), due, DAY), None);
        // A founder that can no longer found refunds at once.
        assert_eq!(
            step(&pool, &[cand(2, 0.6, 0.5)], 1, Some(1e6), 101, DAY),
            Some(Step::Refund)
        );
        // No pledges, nothing to refund.
        assert_eq!(step(&Pool::default(), &cs, 1, Some(1e6), due, DAY), None);
    }

    #[test]
    fn refunds_and_foundings_settle_the_pool_and_journal_both_directions() {
        let mut pool = pool_with(1, 0.0);
        pool.pledge(2, 1, 100.0, 10);
        pool.pledge(3, 1, 200.0, 11);
        assert_eq!((pool.founder, pool.since), (Some(1), 10));
        // Part refund leaves the rest standing.
        pool.refunded(0, 40.0, 50);
        assert_eq!(pool.pledges[0].amount, 60.0);
        pool.refunded(0, 60.0, 51);
        pool.refunded(0, 200.0, 52);
        assert!(pool.pledges.is_empty() && pool.founder.is_none());
        let kinds: Vec<Kind> = pool.journal.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Pledge,
                Kind::Pledge,
                Kind::RefundShort,
                Kind::Refund,
                Kind::Refund
            ]
        );
        let back = &pool.journal[3];
        assert_eq!((back.from, back.to), (1, 2));
        // A founding makes the pledgers the corp's applicants.
        pool.pledge(2, 1, 100.0, 60);
        pool.founded(9, 77, 61);
        assert_eq!(pool.pledges.len(), 1);
        pool.founded(1, 77, 62);
        assert!(pool.pledges.is_empty());
        assert_eq!(pool.pledged.get(&2), Some(&77));
        assert_eq!(pool.journal.last().map(|e| e.kind), Some(Kind::Spent));
    }

    #[test]
    fn reasons_fit_the_clients_cap() {
        assert!(PLEDGE_REASON.len() <= 40 && REFUND_REASON.len() <= 40);
    }
}
