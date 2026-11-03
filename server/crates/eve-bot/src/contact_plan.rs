//! REVIEW-FIX2 (review item 12): **one owner for the contact books.**
//!
//! Four things used to write the same books with no common rule: the graded
//! tier stance (`HallState::desired_contacts`, `effects::personal_plan`), the
//! `--friends` file (`friends::apply`) and the coalition's mutual blue
//! (R-REL-5, which settles its pairs in the same stance table as the tier).
//! Each now only makes **claims**; a [`ContactPlan`] merges them by a fixed
//! precedence and is the only thing that answers "what should this book
//! hold" ([`ContactPlan::diff`]) and "does this contact count as allied"
//! ([`ContactPlan::allied`]).
//!
//! **Precedence**, highest first: [`Source::Friends`] (a floor: the book is
//! never lowered under it, and nothing else outranks it), [`Source::Coalition`],
//! [`Source::Tier`]. A higher source wins outright; within one source the
//! stronger standing wins (a Friends pair keeps the higher).
//!
//! **Allied** means: a Friends or Coalition claim above 0, or a Tier claim at
//! the blue bar ([`crate::society::BLUE_STANDING`]). A bot-written Tier +5
//! (Friendly, with hours) is a colour in the book, not an exemption from
//! hunts: that waits for the tier logic's Allied step **[A]** (falsified by a
//! Friendly human the owner expects the bots to spare).

use crate::society::BLUE_STANDING;
use std::collections::BTreeMap;

/// Who made a claim; the order is the precedence (later wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Tier,
    Coalition,
    Friends,
}

/// One claim on `(owner, contact)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Claim {
    pub standing: f64,
    pub source: Source,
}

/// The merged claims.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContactPlan {
    claims: BTreeMap<(i64, i64), Claim>,
}

impl ContactPlan {
    pub fn new() -> ContactPlan {
        ContactPlan::default()
    }

    /// Add a claim; the precedence decides what stays.
    pub fn claim(&mut self, owner: i64, contact: i64, standing: f64, source: Source) {
        let new = Claim { standing, source };
        match self.claims.get(&(owner, contact)) {
            Some(old) if old.source > source => {}
            Some(old) if old.source == source && !stronger(new.standing, old.standing, source) => {}
            _ => {
                self.claims.insert((owner, contact), new);
            }
        }
    }

    /// The merged claim on `(owner, contact)`.
    pub fn get(&self, owner: i64, contact: i64) -> Option<Claim> {
        self.claims.get(&(owner, contact)).copied()
    }

    /// The book `owner` should hold: `contact -> standing`.
    pub fn book(&self, owner: i64) -> BTreeMap<i64, f64> {
        self.claims
            .range((owner, i64::MIN)..=(owner, i64::MAX))
            .map(|(&(_, c), claim)| (c, claim.standing))
            .collect()
    }

    /// Does `owner` count `contact` as allied (see the module doc)?
    pub fn allied(&self, owner: i64, contact: i64) -> bool {
        self.get(owner, contact).is_some_and(|c| match c.source {
            Source::Friends | Source::Coalition => c.standing > 0.0,
            Source::Tier => c.standing >= BLUE_STANDING,
        })
    }

    /// The standing a `personal` closure may answer for `allied_with`: the
    /// plan's standing only when the plan calls the contact allied.
    pub fn allied_standing(&self, owner: i64, contact: i64) -> Option<f64> {
        self.allied(owner, contact)
            .then(|| self.get(owner, contact).map(|c| c.standing))
            .flatten()
    }

    /// **The calls** that bring `actual` (the book as read) to the plan for
    /// `owner`: `(contact, Some(standing))` to add or edit, `(contact, None)`
    /// to remove. A Friends claim is a floor (an actual standing at or above
    /// it stands); a contact no claim names is removed only when
    /// `may_remove(contact)`.
    pub fn diff(
        &self,
        owner: i64,
        actual: &BTreeMap<i64, f64>,
        may_remove: &dyn Fn(i64) -> bool,
    ) -> Vec<(i64, Option<f64>)> {
        let want = self.book(owner);
        let mut out: Vec<(i64, Option<f64>)> = want
            .iter()
            .filter(|(c, s)| {
                let floor = self
                    .get(owner, **c)
                    .is_some_and(|k| k.source == Source::Friends);
                match actual.get(c) {
                    None => true,
                    Some(a) if floor => *a < **s - 1e-9,
                    Some(a) => (*a - **s).abs() > 1e-9,
                }
            })
            .map(|(c, s)| (*c, Some(*s)))
            .collect();
        for c in actual.keys() {
            if !want.contains_key(c) && may_remove(*c) {
                out.push((*c, None));
            }
        }
        out
    }
}

/// Is `new` the stronger claim of one `source`? Friends and Coalition keep
/// the higher standing; Tier the larger magnitude (a red outweighs a mild
/// blue of the same strength, ties to the new one).
fn stronger(new: f64, old: f64, source: Source) -> bool {
    match source {
        Source::Friends | Source::Coalition => new > old,
        Source::Tier => new.abs() >= old.abs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::society;

    // Invented ids.
    const ME: i64 = 90_000_001;
    const A: i64 = 90_000_002;
    const B: i64 = 90_000_003;
    const C: i64 = 90_000_004;
    const D: i64 = 90_000_005;

    /// The review's test: all three writers active over one book.
    #[test]
    fn review_fix2_three_writers_merge_to_one_book_with_fixed_precedence() {
        let mut p = ContactPlan::new();
        // Tier: A Friendly (+5), B red (-5), C Allied (+10), D red.
        p.claim(ME, A, 5.0, Source::Tier);
        p.claim(ME, B, -5.0, Source::Tier);
        p.claim(ME, C, 10.0, Source::Tier);
        p.claim(ME, D, -10.0, Source::Tier);
        // Friends floor: B (the file says +3) and A at +3.
        p.claim(ME, B, 3.0, Source::Friends);
        p.claim(ME, A, 3.0, Source::Friends);
        // Coalition: D is a coalition partner (+10).
        p.claim(ME, D, 10.0, Source::Coalition);
        // Order of claims does not matter.
        let mut q = ContactPlan::new();
        q.claim(ME, D, 10.0, Source::Coalition);
        q.claim(ME, A, 3.0, Source::Friends);
        q.claim(ME, B, 3.0, Source::Friends);
        q.claim(ME, D, -10.0, Source::Tier);
        q.claim(ME, C, 10.0, Source::Tier);
        q.claim(ME, B, -5.0, Source::Tier);
        q.claim(ME, A, 5.0, Source::Tier);
        assert_eq!(p, q);
        let book = p.book(ME);
        assert_eq!(book.get(&A), Some(&3.0), "friends outrank the tier's +5");
        assert_eq!(book.get(&B), Some(&3.0), "a tier red never beats the floor");
        assert_eq!(book.get(&C), Some(&10.0));
        assert_eq!(book.get(&D), Some(&10.0), "the coalition outranks the tier");
        // Allied: friends, coalition, tier at blue; a tier +5 alone is not.
        assert!(p.allied(ME, A) && p.allied(ME, B) && p.allied(ME, C) && p.allied(ME, D));
        let mut only_tier = ContactPlan::new();
        only_tier.claim(ME, A, 5.0, Source::Tier);
        assert!(!only_tier.allied(ME, A), "a bot-written +5 is not allied");
        // ... and the server's own rule, fed through the plan, agrees.
        let aff = |id| eve_server::group_contacts::Affiliation {
            character_id: id,
            corporation_id: 1_000_001,
            alliance_id: None,
        };
        let me = eve_server::group_contacts::Affiliation {
            character_id: ME,
            corporation_id: 98_000_001,
            alliance_id: None,
        };
        assert!(
            society::hostile_to_with(
                me,
                aff(A),
                |_, _| None,
                |c| only_tier.allied_standing(ME, c)
            ),
            "a Friendly human is still hunted before the Allied step"
        );
        assert!(!society::hostile_to_with(
            me,
            aff(A),
            |_, _| None,
            |c| p.allied_standing(ME, c)
        ));
        // The diff: a floor stands, a tier standing is edited, a gone claim
        // is removed only if it may be.
        let actual: BTreeMap<i64, f64> = [(A, 5.0), (B, 1.0), (C, 5.0), (77, 2.0)].into();
        let diff = p.diff(ME, &actual, &|c| c == 77);
        assert!(
            !diff.iter().any(|(c, _)| *c == A),
            "5 >= the 3 floor: {diff:?}"
        );
        assert!(
            diff.contains(&(B, Some(3.0))),
            "raised to the floor: {diff:?}"
        );
        assert!(diff.contains(&(C, Some(10.0))));
        assert!(diff.contains(&(D, Some(10.0))));
        assert!(diff.contains(&(77, None)));
        let keep = p.diff(ME, &actual, &|_| false);
        assert!(!keep.iter().any(|(_, s)| s.is_none()), "{keep:?}");
    }
}
