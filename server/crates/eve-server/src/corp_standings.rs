//! Corporation standings (CORP-S) -- what an NPC thinks of a player corp.
//!
//! **Derived, never stored.** The client's own text for the Standings tab
//! says how (`UI/Standings/CompositionWindow/NPCToPlayerCorp` **[V]**, the
//! description label `standingsPanel.ConstructRightCont` shows over the
//! composition list when the panel looks at a corporation): *"NPC standings
//! towards a player corporation are calculated from the average of all
//! members' individual standings. Only members that are in the player
//! corporation for more than seven days straight count toward the standing
//! average."* So a corp's standing from an NPC owner is
//!
//! ```text
//! sum(member's standing from that owner) / (members that have been in
//!                                           the corp for over 7 days)
//! ```
//!
//! over the members who count. Because nothing is stored, a member's
//! standing changing, a member joining and a member leaving all "recompute"
//! by construction; what this module adds is the *push* to the members who
//! are online ([`announce`], [`Around`]), because the client holds its own
//! copy (`npccorpstandings`).
//!
//! What the client does with the answers, **[V]** `standingsvc.py`:
//!
//! - `__RefreshStandings` (`:110-123`) asks `standingMgr.GetCorpStandings`
//!   only `if not IsNPC(session.corpid)`, in parallel with
//!   `GetCharStandings`, and takes a dict `{fromID: DBRow(fromID, standing)}`
//!   (a rowset is `.Index('fromID')`-ed). It re-runs on every `corpid`
//!   change, so a member joining or leaving needs no push of its own.
//! - `OnStandingSet(fromID, toID == corpid, standing)` (`:49-61`) replaces
//!   or, at zero, deletes the row. This is the push we send when a corp's
//!   derived value moves while members are in the game.
//! - `GetStanding(fromID, corpid)` (`:140-143`) reads it, and
//!   `GetStandingsDataNPCsToMyCorp` (`:223`) feeds the corp half of the
//!   Standings tab.
//! - `GetStandingCompositions(fromID, corpID)` (`:293`, reached when the
//!   selected owner is a corporation, `standingsPanel.py:86-87`) lists the
//!   members behind one number: rows with `.ownerID` and `.standing`
//!   (`standingCompositionScroll.Update`, `standingCompositionEntry`); rows
//!   whose `ownerID` is an NPC id are skipped by the client.
//!
//! **What the client does not consult.** Station office rental is checked
//! on the server (`standingsrestriction`, `corpStationMgr`); the client's
//! `_check_standings` for a corporation only fires for a service in the
//! station's restriction table, which this server does not hold (the build's
//! `stationStandingsRestrictions` has one owner and seven rules, and our
//! stations drop the service list: `docs/lanes/corp-o.md` Not done). The
//! broker fee for a corporation's market orders reads corp standings
//! (`marketsvc`), but nothing here places a corporation's orders. Agent
//! access reads the *character's* standings only (`CanUseAgent`,
//! `standingsvc.py:98-103`). So the Standings tab, `GetStanding` and the
//! composition list are the places that show these numbers.

use crate::pyutil::filetime_now;
use crate::service::Registry;
use crate::standings::is_npc_owner;
use eve_marshal::{DbRow, DbRowDescriptor, DbType, DbValue, PyValue};
use std::collections::BTreeMap;

/// How long a member must have been in the corp before counting: "more than
/// seven days straight" **[V]** (the client's text above), in FILETIME ticks
/// (100 ns).
///
/// **[I]** that "straight" resets on leaving and rejoining: it does, because
/// a join writes a fresh `start_date` (`Corporations::enrol`).
pub(crate) const TENURE_TICKS: i64 = 7 * 24 * 3_600 * 10_000_000;

/// Whether `member` has been in the corp long enough at `now`.
fn counts(start_date: i64, now: i64) -> bool {
    now.saturating_sub(start_date) > TENURE_TICKS
}

/// The members of `corporation_id` who count at `now`, in id order.
pub(crate) fn counting_members(reg: &Registry, corporation_id: i64, now: i64) -> Vec<i64> {
    reg.corporations()
        .members_of(corporation_id)
        .into_iter()
        .filter(|id| {
            reg.corporations()
                .direct(*id)
                .is_some_and(|m| counts(m.start_date, now))
        })
        .collect()
}

/// Whether this is a player corporation that exists and is open.
fn is_player_corp(reg: &Registry, corporation_id: i64) -> bool {
    reg.corporations()
        .get(corporation_id)
        .is_some_and(|c| !c.deleted)
}

/// Every NPC owner's standing toward one player corp, at `now`: the mean of
/// the counting members' standings, zero for a member with none (the client's
/// "average of all members", **[I]** that a member without a row is a zero in
/// it). Zero results are left out, as the client deletes a zero row.
///
/// Empty for a corp that does not exist and for one with no counting member.
pub(crate) fn corp_standings(reg: &Registry, corporation_id: i64, now: i64) -> BTreeMap<i64, f64> {
    let mut out = BTreeMap::new();
    if !is_player_corp(reg, corporation_id) {
        return out;
    }
    let members = counting_members(reg, corporation_id, now);
    if members.is_empty() {
        return out;
    }
    let mut sums: BTreeMap<i64, f64> = BTreeMap::new();
    for member in &members {
        for (from_id, standing) in reg.standings.toward(*member) {
            if is_npc_owner(from_id) {
                *sums.entry(from_id).or_insert(0.0) += standing;
            }
        }
    }
    let n = members.len() as f64;
    for (from_id, sum) in sums {
        let mean = sum / n;
        if mean != 0.0 {
            out.insert(from_id, mean);
        }
    }
    out
}

/// One owner's standing toward a corp; 0.0 when none.
#[cfg(test)]
pub(crate) fn corp_standing(reg: &Registry, corporation_id: i64, from_id: i64, now: i64) -> f64 {
    corp_standings(reg, corporation_id, now)
        .get(&from_id)
        .copied()
        .unwrap_or(0.0)
}

/// The descriptor of one `npccorpstandings` row, the client's own
/// `fromStandingHeader` (`standingsvc.py:39-41`) **[V]**.
fn standing_row_descriptor() -> DbRowDescriptor {
    DbRowDescriptor::from_pairs(&[("fromID", DbType::I4), ("standing", DbType::R5)])
        .expect("two fixed-width columns are a valid layout")
}

/// `standingMgr.GetCorpStandings`: a dict `{fromID: DBRow}`, empty for a
/// character in no player corp (the client never asks then).
pub(crate) fn corp_standings_value(reg: &Registry, actor: Option<i64>, now: i64) -> PyValue {
    let corp = actor
        .and_then(|a| reg.character(a))
        .map_or(0, |c| c.corporation_id);
    let descriptor = standing_row_descriptor();
    PyValue::Dict(
        corp_standings(reg, corp, now)
            .into_iter()
            .map(|(from_id, standing)| {
                let row = DbRow::new(
                    descriptor.clone(),
                    vec![DbValue::Int(from_id), DbValue::Real(standing)],
                )
                .expect("the values match the descriptor declared above");
                (PyValue::DbRow(Box::new(row)), PyValue::Int(from_id))
            })
            .collect(),
    )
}

/// `standingMgr.GetStandingCompositions(fromID, corpID)`: a list of rows
/// `(ownerID, standing)`, one per member who counts, with that member's own
/// standing from `fromID`. A member with none is listed at 0, since a zero
/// is in the average ([`corp_standings`]). Empty for anything that is not a
/// player corp (a character, an NPC corp: there the panel shows
/// transactions, not this).
pub(crate) fn compositions_value(reg: &Registry, args: &[PyValue], now: i64) -> PyValue {
    let (Some(PyValue::Int(from_id)), Some(PyValue::Int(corp))) = (args.first(), args.get(1))
    else {
        return PyValue::List(vec![]);
    };
    if !is_player_corp(reg, *corp) {
        return PyValue::List(vec![]);
    }
    let descriptor =
        DbRowDescriptor::from_pairs(&[("ownerID", DbType::I4), ("standing", DbType::R5)])
            .expect("two fixed-width columns are a valid layout");
    PyValue::List(
        counting_members(reg, *corp, now)
            .into_iter()
            .map(|member| {
                let row = DbRow::new(
                    descriptor.clone(),
                    vec![
                        DbValue::Int(member),
                        DbValue::Real(reg.standings.get(member, *from_id)),
                    ],
                )
                .expect("the values match the descriptor declared above");
                PyValue::DbRow(Box::new(row))
            })
            .collect(),
    )
}

/// `OnStandingSet(fromID, corpID, standing)` to the corp's members -- the
/// same event the character's own standing uses (`standingsvc.py:49-61`),
/// which the client files under `npccorpstandings` when `toID == corpid`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CorpStandingSet {
    pub from_id: i64,
    pub corporation_id: i64,
    pub standing: f64,
    pub to: Vec<i64>,
}

impl crate::notify::Push for CorpStandingSet {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnStandingSet",
            vec![
                PyValue::Int(self.from_id),
                PyValue::Int(self.corporation_id),
                PyValue::Real(self.standing),
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

/// A corp's derived standings as they stood, to be compared after a change.
#[derive(Debug, Clone, Default)]
pub(crate) struct Before {
    corporation_id: i64,
    standings: BTreeMap<i64, f64>,
}

/// Take the corp's standings now, before a change.
pub(crate) fn before(reg: &Registry, corporation_id: i64) -> Before {
    Before {
        corporation_id,
        standings: corp_standings(reg, corporation_id, filetime_now()),
    }
}

/// Compare the corp's standings with `before` and push every owner whose
/// value moved (to zero as 0.0, which the client reads as "delete") to the
/// corp's current members except `exclude`, who is changing corp and
/// refreshes from the session change. Returns how many owners changed.
pub(crate) fn announce(reg: &mut Registry, was: Before, exclude: Option<i64>) -> usize {
    let corp = was.corporation_id;
    let now = corp_standings(reg, corp, filetime_now());
    let to: Vec<i64> = reg
        .corporations()
        .members_of(corp)
        .into_iter()
        .filter(|id| Some(*id) != exclude)
        .collect();
    let mut owners: Vec<i64> = was.standings.keys().chain(now.keys()).copied().collect();
    owners.sort_unstable();
    owners.dedup();
    let mut changed = 0;
    for from_id in owners {
        let old = was.standings.get(&from_id).copied().unwrap_or(0.0);
        let new = now.get(&from_id).copied().unwrap_or(0.0);
        if old == new {
            continue;
        }
        changed += 1;
        if !to.is_empty() {
            reg.push(CorpStandingSet {
                from_id,
                corporation_id: corp,
                standing: new,
                to: to.clone(),
            });
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};

    const CORP: i64 = 98_000_777;
    const NPC_A: i64 = 1_000_777;
    const NPC_B: i64 = 500_777;
    const NOW: i64 = 140_000_000_000_000_000;
    const DAY: i64 = 24 * 3_600 * 10_000_000;

    fn world() -> Registry {
        crate::testkit::registry_with(|c: &mut SandboxConfig| {
            c.characters = [90_000_701, 90_000_702, 90_000_703]
                .into_iter()
                .map(|id| Character {
                    character_id: id,
                    name: format!("Pilot {id}"),
                    ship_id: id + 10_000_000,
                    ..Character::default()
                })
                .collect();
        })
    }

    /// Put the pilots in corp `CORP`, each joined `joined[i]` ticks ago
    /// relative to `at`, the way a world file would (`corp` and `corpmember`
    /// records through a snapshot).
    fn corp_at(reg: &mut Registry, at: i64, joined: &[(i64, i64)]) {
        let mut snap = reg.to_snapshot();
        let corp = CORP.to_string();
        let first = joined[0].0.to_string();
        let line: Vec<&str> = vec![
            "corp",
            &corp,
            &first,
            &first,
            "0",
            "0.1",
            "0.1",
            "1",
            "0",
            "-1",
            "-1",
            "-1",
            "-1",
            "-1",
            "-1",
            "Invented Corp",
            "INVC",
            "",
            "",
        ];
        snap.corporations.parse_record(&line).expect("a corp");
        for (id, ago) in joined {
            let (id, start) = (id.to_string(), (at - ago).to_string());
            snap.corporations
                .parse_record(&["corpmember", &id, &corp, "0", &start])
                .expect("a member");
        }
        reg.apply_snapshot(snap);
    }

    /// The three pilots, joined `days[i]` days before [`NOW`].
    fn corp_with(reg: &mut Registry, days: [i64; 3]) {
        let joined: Vec<(i64, i64)> = [90_000_701, 90_000_702, 90_000_703]
            .into_iter()
            .zip(days)
            .map(|(id, d)| (id, d * DAY))
            .collect();
        corp_at(reg, NOW, &joined);
    }

    #[test]
    fn the_standing_is_the_mean_of_counting_members_and_a_missing_row_is_zero() {
        let mut reg = world();
        corp_with(&mut reg, [30, 10, 9]);
        reg.standings.set(90_000_701, NPC_A, 6.0);
        reg.standings.set(90_000_702, NPC_A, 3.0);
        // The third has no row from NPC_A: a zero in the mean of three.
        let s = corp_standings(&reg, CORP, NOW);
        assert_eq!(s.get(&NPC_A).copied(), Some(3.0));
        assert_eq!(corp_standing(&reg, CORP, NPC_B, NOW), 0.0);
        assert!(!s.contains_key(&NPC_B));
    }

    #[test]
    fn a_member_of_seven_days_or_less_does_not_count() {
        let mut reg = world();
        // Exactly seven days is not "more than seven days".
        corp_with(&mut reg, [30, 7, 1]);
        reg.standings.set(90_000_701, NPC_A, 4.0);
        reg.standings.set(90_000_702, NPC_A, -10.0);
        reg.standings.set(90_000_703, NPC_A, -10.0);
        assert_eq!(corp_standing(&reg, CORP, NPC_A, NOW), 4.0);
        // A day later the second has crossed the line.
        assert_eq!(
            corp_standing(&reg, CORP, NPC_A, NOW + DAY + 1),
            (4.0 - 10.0) / 2.0
        );
        assert_eq!(counting_members(&reg, CORP, NOW), vec![90_000_701]);
    }

    #[test]
    fn nothing_counts_before_the_week_and_player_owners_are_ignored() {
        let mut reg = world();
        corp_with(&mut reg, [1, 2, 3]);
        reg.standings.set(90_000_701, NPC_A, 9.0);
        assert!(corp_standings(&reg, CORP, NOW).is_empty());
        // Not a corp at all.
        assert!(corp_standings(&reg, 98_123_456, NOW).is_empty());
    }

    #[test]
    fn a_zero_mean_is_left_out() {
        let mut reg = world();
        corp_with(&mut reg, [30, 30, 30]);
        reg.standings.set(90_000_701, NPC_A, 2.0);
        reg.standings.set(90_000_702, NPC_A, -2.0);
        assert!(corp_standings(&reg, CORP, NOW).is_empty());
    }

    #[test]
    fn compositions_list_each_counting_member_with_their_own_standing() {
        let mut reg = world();
        corp_with(&mut reg, [30, 30, 2]);
        reg.standings.set(90_000_701, NPC_A, 5.0);
        let v = compositions_value(&reg, &[PyValue::Int(NPC_A), PyValue::Int(CORP)], NOW);
        let PyValue::List(rows) = v else {
            panic!("a list, got {v:?}")
        };
        let got: Vec<(i64, f64)> = rows
            .iter()
            .map(|r| {
                let PyValue::DbRow(row) = r else {
                    panic!("a row, got {r:?}")
                };
                let (DbValue::Int(o), DbValue::Real(s)) = (&row.values()[0], &row.values()[1])
                else {
                    panic!("(ownerID, standing)")
                };
                (*o, *s)
            })
            .collect();
        assert_eq!(got, vec![(90_000_701, 5.0), (90_000_702, 0.0)]);
        // Anything that is not a player corp, or a malformed call, is empty.
        assert_eq!(
            compositions_value(&reg, &[PyValue::Int(NPC_A), PyValue::Int(1_000_001)], NOW),
            PyValue::List(vec![])
        );
        assert_eq!(compositions_value(&reg, &[], NOW), PyValue::List(vec![]));
    }

    #[test]
    fn the_call_answers_for_the_callers_corp() {
        let mut reg = world();
        corp_with(&mut reg, [30, 30, 30]);
        reg.standings.set(90_000_701, NPC_A, 6.0);
        let PyValue::Dict(rows) = corp_standings_value(&reg, Some(90_000_702), NOW) else {
            panic!("a dict")
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, PyValue::Int(NPC_A));
        let PyValue::DbRow(row) = &rows[0].0 else {
            panic!("a row")
        };
        assert_eq!(row.values()[1], DbValue::Real(2.0));
        // A character in the NPC corp gets an empty dict.
        let mut reg = world();
        reg.standings.set(90_000_701, NPC_A, 6.0);
        assert_eq!(
            corp_standings_value(&reg, Some(90_000_701), NOW),
            PyValue::Dict(vec![])
        );
    }

    /// The push when a member's standing moves: every member hears it with
    /// the new mean, and the event is the client's own `OnStandingSet` aimed
    /// at the corp id.
    #[test]
    fn a_change_is_announced_to_the_members_with_the_new_mean() {
        let mut reg = world();
        // Long enough ago that the real clock (the announce reads it) counts
        // them too.
        let real = filetime_now();
        corp_at(
            &mut reg,
            real,
            &[(90_000_701, 30 * DAY), (90_000_702, 30 * DAY)],
        );
        let was = before(&reg, CORP);
        reg.standings.set(90_000_701, NPC_A, 8.0);
        assert_eq!(announce(&mut reg, was, None), 1);
        let pushes = reg.take_pending();
        assert_eq!(pushes.len(), 1);
        let (event, args) = pushes[0].1.to_push();
        let audience = pushes[0].1.audience();
        assert_eq!(event, "OnStandingSet");
        assert_eq!(
            args,
            vec![PyValue::Int(NPC_A), PyValue::Int(CORP), PyValue::Real(4.0)]
        );
        assert_eq!(audience, Some(vec![90_000_701, 90_000_702]));
        // Nothing moved: nothing sent.
        let was = before(&reg, CORP);
        assert_eq!(announce(&mut reg, was, None), 0);
        assert!(reg.take_pending().is_empty());
    }
}
