//! **The `[doc]` rollup** (DOC-9; R-DOCTRINE section 9): the doctrine layer's
//! state in the bots' periodic summary, one line per leader and one total,
//! for `tools/doctrine-report.py` to read next to the `[soc]` line.
//!
//! ```text
//! [doc] roll group=98000001 current=shield/long/kite/cruiser/hybrid since=1.4 fancy=shield/long/hybrid/cruiser
//!       switches=2 flaps=0 min_gap=3.2 fighters=9 theatres=1 committed=4 open=commit
//! [doc] roll-all leaders=3 doctrines=2 layer_range=2 switches=5 flaps=0 min_gap=3.2 span=6.5
//!       fighters=21 theatres=1 committed=4 calls=0
//! ```
//!
//! (one line each, here wrapped). `since` is game days; `committed` is the
//! ships sent to the leader's theatres and not lost; `open` the states of
//! those theatres; `min_gap` the fewest game days between two switches and
//! `span` the game days since the first switch seen (or the first choice).
//! Pure: it reads a [`HallState`] and a clock, prints
//! nothing.

use crate::corpmind::HallState;
use crate::doctrine_hall::is_fighter;
use std::collections::BTreeSet;

/// A switch back to the doctrine just left within this many game days is a
/// flap (G3).
pub const FLAP_DAYS: f64 = 4.0;

/// Switches `A -> B` followed by `B -> A` within [`FLAP_DAYS`].
pub fn flaps(switches: &[(u64, String, String)], day_s: u64) -> usize {
    let day = day_s.max(1) as f64;
    switches
        .windows(2)
        .filter(|w| {
            w[0].2 == w[1].1
                && w[1].2 == w[0].1
                && (w[1].0.saturating_sub(w[0].0)) as f64 / day <= FLAP_DAYS
        })
        .count()
}

/// The shortest gap, in game days, between consecutive switches; `None`
/// with fewer than two.
pub fn min_gap_days(switches: &[(u64, String, String)], day_s: u64) -> Option<f64> {
    switches
        .windows(2)
        .map(|w| w[1].0.saturating_sub(w[0].0) as f64 / day_s.max(1) as f64)
        .min_by(f64::total_cmp)
}

/// The `layer/range` head of a doctrine key (`doctrine::key_of`).
pub fn layer_range(key: &str) -> String {
    key.split('/').take(2).collect::<Vec<_>>().join("/")
}

/// **The lines** at game time `now_s`: one per doctrine book, then the
/// total. Empty when no leader has a book (the layer is asleep).
pub fn rollup(st: &HallState, now_s: u64) -> Vec<String> {
    if st.dbooks.is_empty() {
        return Vec::new();
    }
    let day = st.day.max(1) as f64;
    let mut lines = Vec::new();
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    let mut heads: BTreeSet<String> = BTreeSet::new();
    let (mut switches, mut flap_total, mut fighters_total) = (0usize, 0usize, 0usize);
    let (mut theatres_total, mut committed_total) = (0usize, 0usize);
    let mut gap: Option<f64> = None;
    let mut start: Option<u64> = None;
    for (group, book) in &st.dbooks {
        // The leader's corps: the group itself, or the corps of the alliance.
        let corps: Vec<i64> = std::iter::once(*group)
            .chain(
                st.corps
                    .values()
                    .filter(|c| c.alliance == Some(*group))
                    .map(|c| c.id),
            )
            .collect();
        let fighters = st
            .members
            .values()
            .filter(|m| corps.contains(&m.corp) && is_fighter(&m.career))
            .count();
        let theatres = st.theatres.get(group).map_or(&[][..], Vec::as_slice);
        let committed: usize = theatres
            .iter()
            .map(|t| {
                t.sent
                    .iter()
                    .filter(|s| !t.lost.contains(&s.char_id))
                    .count()
            })
            .sum();
        let open: Vec<&str> = theatres.iter().map(|t| t.state.name()).collect();
        let f = flaps(&book.switches, st.day);
        let g = min_gap_days(&book.switches, st.day);
        gap = match (gap, g) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let first = book.switches.first().map_or(book.since, |s| s.0);
        start = Some(start.map_or(first, |x| x.min(first)));
        if let Some(k) = book.current.as_deref() {
            keys.insert(k);
            heads.insert(layer_range(k));
        }
        switches += book.switches.len();
        flap_total += f;
        fighters_total += fighters;
        theatres_total += theatres.len();
        committed_total += committed;
        lines.push(format!(
            "[doc] roll group={group} current={} since={:.1} fancy={} switches={} flaps={f} \
             min_gap={} fighters={fighters} theatres={} committed={committed} open={}",
            book.current.as_deref().unwrap_or("-"),
            now_s.saturating_sub(book.since) as f64 / day,
            book.fancy.line(),
            book.switches.len(),
            g.map_or("-".to_owned(), |g| format!("{g:.1}")),
            theatres.len(),
            if open.is_empty() {
                "-".to_owned()
            } else {
                open.join(",")
            },
        ));
    }
    lines.push(format!(
        "[doc] roll-all leaders={} doctrines={} layer_range={} switches={switches} \
         flaps={flap_total} min_gap={} span={:.1} fighters={fighters_total} \
         theatres={theatres_total} committed={committed_total} calls={}",
        st.dbooks.len(),
        keys.len(),
        heads.len(),
        gap.map_or("-".to_owned(), |g| format!("{g:.1}")),
        start.map_or(0.0, |s| now_s.saturating_sub(s) as f64 / day),
        st.calls.len(),
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::Band;
    use crate::corpmind::Member;
    use crate::doctrine_lead::{DoctrineBook, Fancy};
    use crate::doctrine_theatre::{Sent, Theatre};
    use crate::mind::{TRAIT_COUNT, Traits};
    use crate::society::Satisfaction;

    const DAY: u64 = 1_000;
    const G1: i64 = 98_500_001;
    const G2: i64 = 98_500_002;

    fn switch(at_day: u64, from: &str, to: &str) -> (u64, String, String) {
        (at_day * DAY, from.to_owned(), to.to_owned())
    }

    fn pilot(corp: i64, career: &str) -> Member {
        Member {
            name: "P".to_owned(),
            traits: Traits([0.5; TRAIT_COUNT]),
            career: career.to_owned(),
            band: Band::High,
            corp,
            alliance: None,
            last_turn: None,
            satisfaction: Satisfaction::default(),
            applied: None,
            salt: 0,
        }
    }

    #[test]
    fn a_flap_is_a_return_within_four_days() {
        let s = vec![
            switch(1, "x/a/k/c/f", "y/b/k/c/f"),
            switch(3, "y/b/k/c/f", "x/a/k/c/f"),
            switch(20, "x/a/k/c/f", "y/b/k/c/f"),
            switch(30, "y/b/k/c/f", "x/a/k/c/f"),
        ];
        assert_eq!(flaps(&s, DAY), 1, "only the first return is quick");
        assert_eq!(flaps(&[], DAY), 0);
        assert_eq!(min_gap_days(&s, DAY), Some(2.0));
        assert_eq!(min_gap_days(&s[..1], DAY), None);
        assert_eq!(
            layer_range("armour/short/brawl/cruiser/hybrid"),
            "armour/short"
        );
    }

    #[test]
    fn the_rollup_names_each_leader_and_totals_them() {
        let mut st = HallState::new(DAY);
        let mut b1 = DoctrineBook::new(Fancy::draw("Invented One", 1));
        b1.current = Some("armour/short/brawl/cruiser/hybrid".to_owned());
        b1.since = 2 * DAY;
        b1.switches = vec![switch(2, "-", "armour/short/brawl/cruiser/hybrid")];
        let mut b2 = DoctrineBook::new(Fancy::draw("Invented Two", 1));
        b2.current = Some("shield/long/kite/cruiser/missile".to_owned());
        st.dbooks.insert(G1, b1);
        st.dbooks.insert(G2, b2);
        for i in 0..3 {
            st.members.insert(90_500_000 + i, pilot(G1, "ratter"));
        }
        st.members.insert(90_500_100, pilot(G1, "miner"));
        let mut t = Theatre::new(30_000_001, 0);
        t.sent = vec![
            Sent {
                char_id: 90_500_000,
                arrives_s: 0,
            },
            Sent {
                char_id: 90_500_001,
                arrives_s: 0,
            },
        ];
        t.lost.insert(90_500_001);
        st.theatres.insert(G1, vec![t]);

        let lines = rollup(&st, 5 * DAY);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with(&format!("[doc] roll group={G1} current=armour/short/")));
        assert!(lines[0].contains("since=3.0"), "{}", lines[0]);
        assert!(
            lines[0].contains("fighters=3"),
            "a miner does not fly: {}",
            lines[0]
        );
        assert!(lines[0].contains("committed=1 open=watch"), "{}", lines[0]);
        assert!(
            lines[1].contains("current=shield/long/kite/")
                && lines[1].contains("committed=0 open=-")
        );
        assert!(
            lines[2].starts_with("[doc] roll-all leaders=2 doctrines=2 layer_range=2 switches=1 flaps=0 min_gap=- span=5.0"),
            "{}",
            lines[2]
        );
        assert!(rollup(&HallState::new(DAY), 0).is_empty());
    }
}
