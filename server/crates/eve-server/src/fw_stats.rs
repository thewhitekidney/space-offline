//! **Faction Warfare kills and victory points** for the stats windows (FW-5,
//! `handback/r-fw/report.md` §6).
//!
//! `facWarSvc.GetStats_*` answer `{statKey: n}` for a pilot, a corporation,
//! an alliance and a militia, with six keys: kills and VP, for yesterday,
//! last week and in total ([`crate::service::facwar_mgr::STAT_KEYS`]). They
//! were zeros. This module counts them.
//!
//! - **One delta kind** ([`KIND`]): `group owner day -> kills vp`. `group` is
//!   a client group id (`STAT_GROUPS`: character 1, corporation 2, alliance
//!   32) or [`FACTION`] (0, ours) for a militia. `day` is the UTC day
//!   (`filetime / DAY`) the event happened on, or [`LIFETIME`] (0) for the
//!   running total. An owner's rows older than [`KEEP_DAYS`] are dropped on
//!   its next write, so the kind stays small.
//! - **Attributed at event time** ([`record`]): the pilot, their player
//!   corporation, their alliance and their militia as they are when the
//!   kill or the capture happens. Leaving a corporation later takes nothing
//!   from it.
//! - **Kills** ([`on_kill`], a `kill_hooks::PLAYER` row after
//!   `killmail_on_kill`): a pilot's ship that dies while its pilot is in a
//!   militia counts +1 for every attacker on its report whose militia is the
//!   enemy ([`crate::crimewatch::fw_enemies`]). A capsule **[A]** and a
//!   drone do not count. Anywhere in space, not only in a warzone **[A]**.
//! - **VP** ([`crate::fw_lp::pay`]): the points a completed complex is worth
//!   are split among the pilots it pays, so a militia's total is the
//!   system's own move **[A]**.
//! - **The windows** ([`stats_of`]): `Y` is the previous UTC day, `LW` the
//!   seven days before today **[A]**. Falsified by the client's window
//!   labelling them otherwise; today's events show in `Total` only.

use crate::damage::BlueTime;
use crate::service::Registry;
use crate::service::facwar_mgr::{self, STAT_GROUPS};

/// The delta kind (`delta_kinds::KINDS`): `group owner day -> kills vp`.
pub const KIND: &str = "fwstat";

/// The group of a militia's own row (the client's groups are 1, 2 and 32).
pub const FACTION: i64 = 0;

/// The `day` of the lifetime row.
pub const LIFETIME: i64 = 0;

/// FILETIME ticks in a UTC day.
pub const DAY: i64 = 864_000_000_000;

/// **[A] Days kept per owner**: seven for the week and one for yesterday,
/// and today's.
pub const KEEP_DAYS: i64 = 8;

/// The UTC day of a time. Day numbers start well above [`LIFETIME`].
pub fn day_of(t: BlueTime) -> i64 {
    (t / DAY).max(1)
}

fn key(group: i64, owner: i64, day: i64) -> Vec<String> {
    vec![group.to_string(), owner.to_string(), day.to_string()]
}

fn num(row: &[String], i: usize) -> i64 {
    row.get(i).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Add `kills` and `vp` to one owner's row of `day` and of the lifetime, and
/// drop the owner's days that fell out of [`KEEP_DAYS`].
fn add(reg: &mut Registry, group: i64, owner: i64, kills: i64, vp: i64, now: BlueTime) {
    let today = day_of(now);
    for day in [LIFETIME, today] {
        let k = key(group, owner, day);
        let (k0, v0) = reg
            .deltas()
            .of(KIND)
            .get(&k)
            .map_or((0, 0), |r| (num(r, 0), num(r, 1)));
        reg.deltas_mut().put(
            KIND,
            k,
            vec![(k0 + kills).to_string(), (v0 + vp).to_string()],
        );
    }
    let stale: Vec<Vec<String>> = reg
        .deltas()
        .of(KIND)
        .range(key(group, owner, 0)..=key(group, owner, i64::MAX))
        .filter_map(|(k, _)| {
            let day: i64 = k.get(2)?.parse().ok()?;
            (day != LIFETIME && day <= today - KEEP_DAYS).then(|| k.clone())
        })
        .collect();
    for k in stale {
        reg.deltas_mut().del(KIND, &k);
    }
}

/// **Count an event for a pilot**: `kills` and `vp` go to the pilot, to
/// their player corporation and alliance, and to their militia, at `now`.
/// Nothing for a pilot in no militia.
pub fn record(reg: &mut Registry, ch: i64, kills: i64, vp: i64, now: BlueTime) {
    let Some(faction) = facwar_mgr::my_faction(reg, ch, now) else {
        return;
    };
    if kills == 0 && vp == 0 {
        return;
    }
    add(reg, STAT_GROUPS[0], ch, kills, vp, now);
    if let Some(corp) = reg.corporations().membership(ch).map(|m| m.corporation_id) {
        add(reg, STAT_GROUPS[1], corp, kills, vp, now);
    }
    if let Some(alliance) = reg.character(ch).and_then(|c| c.alliance_id) {
        add(reg, STAT_GROUPS[2], alliance, kills, vp, now);
    }
    add(reg, FACTION, faction, kills, vp, now);
}

/// The six numbers of one owner, in `STAT_KEYS` order: `killsY killsLW
/// killsTotal vpY vpLW vpTotal`. Zeros for an owner with no row.
pub fn stats_of(reg: &Registry, group: i64, owner: i64, now: BlueTime) -> [i64; 6] {
    let today = day_of(now);
    let mut out = [0; 6];
    for (k, v) in reg
        .deltas()
        .of(KIND)
        .range(key(group, owner, 0)..=key(group, owner, i64::MAX))
    {
        let Some(day) = k.get(2).and_then(|d| d.parse::<i64>().ok()) else {
            continue;
        };
        let (kills, vp) = (num(v, 0), num(v, 1));
        let slots: &[usize] = if day == LIFETIME {
            &[2]
        } else if day == today - 1 {
            &[0, 1]
        } else if day >= today - 7 && day < today {
            &[1]
        } else {
            &[]
        };
        for s in slots {
            out[*s] += kills;
            out[*s + 3] += vp;
        }
    }
    out
}

/// Every owner of `group` with a row, and its [`stats_of`].
pub fn owners(reg: &Registry, group: i64, now: BlueTime) -> Vec<(i64, [i64; 6])> {
    let g = group.to_string();
    let mut seen: Vec<i64> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter(|k| k.first() == Some(&g))
        .filter_map(|k| k.get(1)?.parse().ok())
        .collect();
    seen.dedup();
    seen.sort_unstable();
    seen.dedup();
    seen.into_iter()
        .map(|o| (o, stats_of(reg, group, o, now)))
        .collect()
}

/// `(top, all)` of a group: per key the largest owner's number, and the sum.
pub fn top_and_all(reg: &Registry, group: i64, now: BlueTime) -> ([i64; 6], [i64; 6]) {
    let (mut top, mut all) = ([0; 6], [0; 6]);
    for (_, s) in owners(reg, group, now) {
        for i in 0..6 {
            top[i] = top[i].max(s[i]);
            all[i] += s[i];
        }
    }
    (top, all)
}

/// **The `kill_hooks::PLAYER` row** (after `killmail_on_kill`, which writes
/// the report this reads): a militia pilot's ship died to the enemy militia.
pub fn on_kill(w: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    if ev.ship_type == crate::damage::CAPSULE_TYPE_ID {
        return;
    }
    let Some(report) = crate::killmail::involving(w, ev.victim_character, None, 1)
        .into_iter()
        .find(|r| r.victim == ev.victim_character && r.time == ev.when)
    else {
        return;
    };
    let mut credited: Vec<i64> = std::iter::once(&report.final_blow)
        .chain(report.attackers.iter())
        .filter_map(|a| a.character)
        .filter(|a| *a != ev.victim_character)
        .filter(|a| crate::crimewatch::fw_enemies(w, *a, ev.victim_character))
        .collect();
    credited.sort_unstable();
    credited.dedup();
    for a in credited {
        record(w, a, 1, 0, ev.when);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::facwar::{Table, Warzone};
    use crate::model::{Character, SandboxConfig};
    use std::sync::Arc;

    const A: i64 = 599_701;
    const B: i64 = 599_702;
    const C: i64 = 599_703;
    const S1: i64 = 39_000_701;
    const S2: i64 = 39_000_702;
    const NOW: BlueTime = 134_000_000_000_000_000;

    fn ch(n: i64) -> i64 {
        90_000_700 + n
    }

    fn world() -> Registry {
        let row = |id: i64, f: i64| {
            format!(
                "{id}\t19000701\t29000701\t{f}\t\t0.3\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}"
            )
        };
        let mut c = Content::fixture();
        c.insert(
            "facwar",
            Table::new(vec![Warzone {
                id: 96,
                factions: [A, B],
                hqs: [S1, S2],
            }])
            .expect("ok"),
        );
        c.insert(
            "universe",
            crate::celestial::Universe::parse(&[row(S1, A), row(S2, B)], &[], &[])
                .expect("the fixture universe"),
        );
        let mut npc = crate::npc_data::Table::default();
        for (f, corp) in [(A, 1_999_701), (B, 1_999_702)] {
            npc.factions.insert(
                f,
                crate::npc_data::Faction {
                    faction_id: f,
                    corporation_id: corp + 10,
                    militia_corporation_id: Some(corp),
                    ..crate::npc_data::Faction::default()
                },
            );
        }
        c.insert("npc_data", npc);
        let pilot = |n: i64| Character {
            character_id: ch(n),
            ship_id: ch(n) + 100,
            solar_system_id: Some(S1),
            ..Character::default()
        };
        Registry::new(SandboxConfig {
            characters: (1..=5).map(pilot).collect(),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c))
    }

    fn enlist(w: &mut Registry, who: i64, faction: i64) {
        facwar_mgr::enlist(w, who, faction, NOW).expect("enlists");
        let _ = w.take_pending();
    }

    #[test]
    fn day_of_is_utc_and_never_the_lifetime_row() {
        assert_eq!(day_of(0), 1);
        assert_eq!(day_of(DAY * 5 + 1), 5);
    }

    #[test]
    fn a_pilot_in_no_militia_counts_for_nothing() {
        let mut w = world();
        record(&mut w, ch(1), 1, 10, NOW);
        assert!(w.deltas().of(KIND).is_empty());
    }

    #[test]
    fn a_capture_adds_vp_split_among_the_pilots() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        enlist(&mut w, ch(2), B);
        // A holds S1 and B attacks it: B's pilots capturing a complex there.
        let archetype = crate::facwar::VP_BY_ARCHETYPE[0].0;
        let vp = crate::facwar::vp_for(archetype).expect("a complex");
        let inside = [ch(1), ch(2)];
        let _ = crate::fw_lp::pay(&mut w, S1, 1, archetype, Some(A), &inside, NOW);
        let share = vp / 2;
        assert_eq!(stats_of(&w, 1, ch(1), NOW)[5], share);
        assert_eq!(stats_of(&w, 1, ch(2), NOW)[5], share);
        assert_eq!(stats_of(&w, FACTION, B, NOW)[5], share * 2);
        assert_eq!(stats_of(&w, FACTION, A, NOW)[5], 0);
    }

    #[test]
    fn yesterday_week_total_buckets_roll() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        let d = |n: i64| NOW + n * DAY;
        record(&mut w, ch(1), 1, 5, d(0));
        // Same day: in Total only.
        assert_eq!(stats_of(&w, 1, ch(1), d(0)), [0, 0, 1, 0, 0, 5]);
        // The next day it is yesterday and in the week.
        assert_eq!(stats_of(&w, 1, ch(1), d(1)), [1, 1, 1, 5, 5, 5]);
        // Three days on: the week and Total.
        assert_eq!(stats_of(&w, 1, ch(1), d(3)), [0, 1, 1, 0, 5, 5]);
        // Past the week: Total only.
        assert_eq!(stats_of(&w, 1, ch(1), d(9)), [0, 0, 1, 0, 0, 5]);
        // A write nine days on prunes the old day row, not the lifetime.
        record(&mut w, ch(1), 1, 0, d(9));
        let rows = w.deltas().of(KIND).keys().filter(|k| k[0] == "1").count();
        assert_eq!(rows, 2, "lifetime and the new day");
        assert_eq!(stats_of(&w, 1, ch(1), d(9))[2], 2);
    }

    #[test]
    fn top_and_all_over_two_pilots() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        enlist(&mut w, ch(2), B);
        record(&mut w, ch(1), 3, 10, NOW);
        record(&mut w, ch(2), 1, 40, NOW);
        let (top, all) = top_and_all(&w, 1, NOW);
        assert_eq!((top[2], top[5]), (3, 40));
        assert_eq!((all[2], all[5]), (4, 50));
        let (top, all) = top_and_all(&w, 32, NOW);
        assert_eq!((top, all), ([0; 6], [0; 6]));
    }

    #[test]
    fn fwstat_round_trips() {
        let mut w = world();
        enlist(&mut w, ch(1), B);
        record(&mut w, ch(1), 2, 7, NOW);
        let mut again = world();
        for (k, v) in w.deltas().of(KIND).clone() {
            again.deltas_mut().put(KIND, k, v);
        }
        enlist(&mut again, ch(1), B);
        assert_eq!(stats_of(&again, 1, ch(1), NOW), stats_of(&w, 1, ch(1), NOW));
        assert_eq!(stats_of(&again, 1, ch(1), NOW)[2], 2);
    }

    fn event(victim: i64, ship_type: i64) -> crate::kill_hooks::KillEvent {
        crate::kill_hooks::KillEvent {
            victim_character: victim,
            victim_ship: victim + 100,
            ship_type,
            killer: None,
            system: S1,
            npc: None,
            when: NOW,
            park: Vec::new(),
        }
    }

    fn report(victim: i64, attackers: &[i64]) -> crate::killmail::Report {
        let at = |c: i64| crate::killmail::Attacker {
            character: Some(c),
            corporation: None,
            alliance: None,
            faction: None,
            ship_type: None,
            weapon_type: None,
            damage: 1,
            security: None,
        };
        crate::killmail::Report {
            kill_id: 1,
            time: NOW,
            system: S1,
            victim,
            victim_corporation: 1,
            victim_alliance: None,
            ship_type: 1,
            damage_taken: 1,
            isk_lost: None,
            isk_destroyed: None,
            final_blow: at(attackers[0]),
            attackers: attackers[1..].iter().map(|c| at(*c)).collect(),
            items: Vec::new(),
            bounty_claimed: None,
            kill_right_supplied: None,
        }
    }

    #[test]
    fn a_kill_of_the_enemy_militia_counts_for_every_attacker_and_his_corp() {
        let mut w = world();
        enlist(&mut w, ch(1), A);
        enlist(&mut w, ch(2), B);
        enlist(&mut w, ch(3), B);
        enlist(&mut w, ch(4), A);
        crate::killmail::put(&mut w, &report(ch(1), &[ch(2), ch(3), ch(4)]));
        on_kill(&mut w, &event(ch(1), 587));
        assert_eq!(stats_of(&w, 1, ch(2), NOW)[2], 1);
        assert_eq!(stats_of(&w, 1, ch(3), NOW)[2], 1);
        assert_eq!(stats_of(&w, FACTION, B, NOW)[2], 2);
        assert_eq!(stats_of(&w, 1, ch(4), NOW)[2], 0, "his own side");
        assert_eq!(stats_of(&w, FACTION, A, NOW)[2], 0);
    }

    #[test]
    fn a_pod_and_a_neutral_do_not_count() {
        let mut w = world();
        enlist(&mut w, ch(1), A);
        enlist(&mut w, ch(2), B);
        crate::killmail::put(&mut w, &report(ch(1), &[ch(2)]));
        on_kill(&mut w, &event(ch(1), crate::damage::CAPSULE_TYPE_ID));
        assert!(w.deltas().of(KIND).is_empty(), "a capsule");
        // A victim in no militia, and an attacker in none.
        crate::killmail::put(&mut w, &report(ch(5), &[ch(2)]));
        on_kill(&mut w, &event(ch(5), 587));
        crate::killmail::put(&mut w, &report(ch(1), &[ch(5)]));
        on_kill(&mut w, &event(ch(1), 587));
        assert!(w.deltas().of(KIND).is_empty());
        let _ = C;
    }
}
