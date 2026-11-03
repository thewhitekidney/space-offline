//! **A pirate mends his name** (SEC-1c/1d, `docs/lanes/sec-1b.md`): an
//! outlaw hunter docked with Clone Soldier tags in the hangar turns them in
//! at the security office, and buys the ones it lacks where the market has
//! them. One decision, [`plan`], pure; [`run`] is the driver's side of it.
//!
//! The office is the server's: a low-sec CONCORD or DED station
//! (`security_office::office_open`). The bot does not look for one; it asks
//! wherever it is docked, at most every [`EVERY`], and a refusal is a note,
//! not a failure (`CustomNotify`: "not here"). Routing a hunter to an office
//! is not done (lane note, Not done).

use crate::{Conn, ItemRow, Result};
use eve_marshal::PyValue;
use eve_server::sec_status::is_outlaw;
use eve_server::security_office::{BANDS, GAIN_PER_TAG, requirements};
use std::collections::BTreeMap;
use std::time::Duration;

/// **[A]** The status a hunter mends toward: just out of the outlaw line,
/// where a high-sec dock opens again for his ship. Falsified by a hunter
/// who still cannot dock at -4.5 (then `is_outlaw`'s rounding is off).
pub const REPAIR_TO: f64 = -4.5;
/// **[A]** The office is asked at most this often while docked.
pub const EVERY: Duration = Duration::from_secs(600);
/// **[A]** Tags bought in one visit, at most; the rest next time.
pub const BUY_AT_MOST: i64 = 40;

/// What one visit does: the status to ask for with what is held (`None`:
/// nothing to hand in), and the tags still to be bought to reach
/// [`REPAIR_TO`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub target: Option<f64>,
    pub buy: Vec<(i64, i64)>,
}

/// **The decision**: for an outlaw's `status` and the tags `held` (type ->
/// count), the highest target the held tags reach, and the shortfall of the
/// plan to [`REPAIR_TO`] by tag type. A pilot who is not an outlaw needs
/// nothing.
pub fn plan(status: f64, held: &BTreeMap<i64, i64>) -> Plan {
    if !is_outlaw(status) {
        return Plan::default();
    }
    let Ok(full) = requirements(&BANDS, status, REPAIR_TO) else {
        return Plan::default();
    };
    let have = |t: i64| held.get(&t).copied().unwrap_or(0);
    let affordable = |target: f64| {
        requirements(&BANDS, status, target)
            .is_ok_and(|p| p.iter().all(|&(t, n)| have(t) >= i64::from(n)))
    };
    let buy: Vec<(i64, i64)> = full
        .iter()
        .map(|&(t, n)| (t, i64::from(n) - have(t)))
        .filter(|&(_, short)| short > 0)
        .collect();
    let target = if buy.is_empty() {
        Some(REPAIR_TO)
    } else {
        let steps = ((REPAIR_TO - status) / GAIN_PER_TAG).ceil() as i64;
        (1..steps)
            .rev()
            .map(|k| status + k as f64 * GAIN_PER_TAG)
            .find(|t| affordable(*t))
    };
    Plan { target, buy }
}

/// The clone tags of `hangar`, by type.
pub fn held(hangar: &[ItemRow]) -> BTreeMap<i64, i64> {
    let mut out = BTreeMap::new();
    for r in hangar
        .iter()
        .filter(|r| BANDS.iter().any(|b| b.type_id == r.type_id))
    {
        *out.entry(r.type_id).or_insert(0) += r.quantity;
    }
    out
}

/// The pilot's security status, as the client asks for it.
pub fn status(conn: &mut Conn) -> Result<Option<f64>> {
    Ok(
        match conn
            .call("crimewatch", "GetMySecurityStatus", vec![])?
            .value()
        {
            Some(PyValue::Real(r)) => Some(r),
            _ => None,
        },
    )
}

/// What a visit came to, for the act log.
pub fn run(
    conn: &mut Conn,
    closed: bool,
    trips: Option<crate::travel::Trips<'_>>,
) -> Result<String> {
    let Some(status) = status(conn)? else {
        return Ok("no security status".to_owned());
    };
    if !is_outlaw(status) {
        return Ok(format!("status {status:.2}: no repair needed"));
    }
    let mut p = plan(status, &held(&conn.hangar_items()?));
    let mut note = String::new();
    if closed && !p.buy.is_empty() {
        let mut want: Vec<(i64, i64)> = Vec::new();
        let mut room = BUY_AT_MOST;
        for &(t, short) in &p.buy {
            let n = short.min(room);
            room -= n;
            if n > 0 {
                let have = held(&conn.hangar_items()?).get(&t).copied().unwrap_or(0);
                want.push((t, have + n));
            }
        }
        let got = crate::acquire::buy_more(conn, &[], &want, trips)?;
        note = format!("; {}", got.summary());
        p = plan(status, &held(&conn.hangar_items()?));
    }
    let Some(target) = p.target else {
        return Ok(format!("status {status:.2}: no tags to hand in{note}"));
    };
    let done = conn.call(
        "crimewatch",
        "ApplyTagsForSecStatus",
        vec![PyValue::Real(target), PyValue::Real(status)],
    )?;
    Ok(match done.refusal() {
        Some(key) => format!("the office refused: {key}{note}"),
        None => format!("handed in tags: {status:.2} -> {target:.2}{note}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(band: usize) -> i64 {
        BANDS[band].type_id
    }

    fn held_of(rows: &[(usize, i64)]) -> BTreeMap<i64, i64> {
        rows.iter().map(|&(b, n)| (tag(b), n)).collect()
    }

    #[test]
    fn a_pilot_who_is_not_an_outlaw_needs_nothing() {
        assert_eq!(plan(-3.0, &held_of(&[(2, 9)])), Plan::default());
        assert_eq!(plan(0.0, &BTreeMap::new()), Plan::default());
    }

    #[test]
    fn with_every_tag_in_hand_the_target_is_the_repair_line() {
        // -6.0 to REPAIR_TO crosses the Recruiter band [-8,-5) and the
        // Transporter band [-5,-2): hand-computed with the client's rule.
        let want = requirements(&BANDS, -6.0, REPAIR_TO).expect("a plan");
        let mut held = BTreeMap::new();
        for (t, n) in &want {
            held.insert(*t, i64::from(*n));
        }
        let p = plan(-6.0, &held);
        assert_eq!(p.target, Some(REPAIR_TO));
        assert!(p.buy.is_empty());
    }

    #[test]
    fn short_of_tags_the_target_is_what_the_held_ones_reach_and_the_rest_is_bought() {
        // Two Recruiter tags and nothing else from -6.0: +1.0 -> -5.0 is
        // the most two tags reach; the tags beyond are the shortfall.
        let p = plan(-6.0, &held_of(&[(1, 2)]));
        assert_eq!(p.target, Some(-5.0));
        let full = requirements(&BANDS, -6.0, REPAIR_TO).expect("a plan");
        let need: i64 = full.iter().map(|&(_, n)| i64::from(n)).sum();
        assert_eq!(p.buy.iter().map(|b| b.1).sum::<i64>(), need - 2);
    }

    #[test]
    fn with_no_tags_there_is_nothing_to_hand_in_and_everything_to_buy() {
        let p = plan(-6.0, &BTreeMap::new());
        assert_eq!(p.target, None);
        assert!(!p.buy.is_empty());
    }

    #[test]
    fn held_counts_only_the_clone_tags_across_stacks() {
        let row = |ty: i64, q: i64| ItemRow {
            item_id: ty * 10 + q,
            type_id: ty,
            owner_id: 1,
            location_id: 2,
            flag_id: crate::FLAG_HANGAR,
            quantity: q,
            group_id: 0,
            category_id: 0,
            singleton: false,
        };
        let got = held(&[
            row(tag(1), 2),
            row(tag(1), 3),
            row(tag(3), 1),
            row(990_001, 7),
        ]);
        assert_eq!(got, held_of(&[(1, 5), (3, 1)]));
    }
}
