//! **MAP-STATS** -- the star map's kill and jump statistics, and its pilot
//! counts (EVEJS-GAP svcC S1, S2).
//!
//! `map.GetHistory(statID, hours)` was unanswered, so seven map filters, the
//! faction-warfare and sov panels and the autopilot's "avoid podkill systems"
//! option raised. This keeps the small hourly counter that answers it, fed by
//! gate arrivals (`jump::tick`) and by deaths ([`on_kill`], a row in both
//! `kill_hooks` tables).
//!
//! **Columns** (spec S1, read off the client's filters): a row is
//! `(solarSystemID, value1, value2, value3)` and
//!
//! | stat | value1 | value2 | value3 |
//! |---|---|---|---|
//! | 1 jumps | jumps | 0 | 0 |
//! | 3 kills | every ship death, NPCs included | NPC deaths | pod deaths |
//! | 5 militia kills | militia deaths plus NPC deaths in a warzone | the NPC part | 0 |
//!
//! so the client's ship kills are `value1 - value2`.
//!
//! **Memory is bounded** by what the client can ask for: [`MAX_HOURS`] hour
//! buckets, pruned when the hour turns. A kill or jump is one `BTreeMap`
//! entry update, so 500 bots cost nothing worth measuring.
//!
//! **[A]** not persisted: a restart forgets the statistics, as it forgets
//! the roster. Falsifier: the owner wants continuity across restarts.
//! **[A]** the current partial hour counts. Falsifier: TQ serves completed
//! hours only; harmless either way.
//! **[A]** jumps count gate arrivals only, not cyno or bridge jumps.
//! **[A]** a capsule is the capsule type (670), as `killmail` tests it.

use crate::damage::BlueTime;
use crate::kill_hooks::KillEvent;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::{Column, DbRowDescriptor, DbType, DbValue, PyValue};
use std::collections::BTreeMap;

/// `appConst.mapHistoryStatJumps`, `Kills`, `FacWarKills`.
pub const STAT_JUMPS: i64 = 1;
pub const STAT_KILLS: i64 = 3;
pub const STAT_FACWAR_KILLS: i64 = 5;

/// 100-ns ticks in an hour.
const HOUR_TICKS: i64 = 36_000_000_000;
/// The client asks for 1 or 24 hours; nothing older is ever read.
pub const MAX_HOURS: i64 = 24;

/// One system's counts in one hour.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub jumps: i64,
    pub ships: i64,
    pub npcs: i64,
    pub pods: i64,
    pub militia: i64,
    pub militia_npcs: i64,
}

/// The counter: `(hour index, system) -> counts`.
#[derive(Debug, Default)]
pub struct MapStats {
    hours: BTreeMap<(i64, i64), Counts>,
    pruned_at: i64,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "map_stats",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(MapStats::default())
}

/// The hour bucket `t` falls in.
pub fn hour_of(t: BlueTime) -> i64 {
    t.div_euclid(HOUR_TICKS)
}

impl MapStats {
    /// Update `system`'s counts for the hour of `at`.
    pub fn record(&mut self, system: i64, at: BlueTime, f: impl FnOnce(&mut Counts)) {
        let h = hour_of(at);
        if h > self.pruned_at {
            self.prune(at);
        }
        f(self.hours.entry((h, system)).or_default());
    }

    /// The `(system, value1, value2, value3)` rows of `stat` over the last
    /// `hours` hours (clamped to `1..=24`), ascending by system, skipping
    /// systems whose three values are all zero.
    pub fn rows(&self, stat: i64, hours: i64, now: BlueTime) -> Vec<(i64, i64, i64, i64)> {
        if !matches!(stat, STAT_JUMPS | STAT_KILLS | STAT_FACWAR_KILLS) {
            return Vec::new();
        }
        let hi = hour_of(now);
        let lo = hi - hours.clamp(1, MAX_HOURS) + 1;
        let mut sum: BTreeMap<i64, (i64, i64, i64)> = BTreeMap::new();
        for (&(_, system), c) in self.hours.range((lo, i64::MIN)..=(hi, i64::MAX)) {
            let (a, b, p) = match stat {
                STAT_JUMPS => (c.jumps, 0, 0),
                STAT_KILLS => (c.ships + c.npcs, c.npcs, c.pods),
                _ => (c.militia + c.militia_npcs, c.militia_npcs, 0),
            };
            let e = sum.entry(system).or_default();
            *e = (e.0 + a, e.1 + b, e.2 + p);
        }
        sum.into_iter()
            .filter(|(_, v)| *v != (0, 0, 0))
            .map(|(s, (a, b, c))| (s, a, b, c))
            .collect()
    }

    /// Drop every bucket older than the 24 hours the client can ask for.
    pub fn prune(&mut self, now: BlueTime) {
        let h = hour_of(now);
        self.pruned_at = h;
        self.hours = self.hours.split_off(&(h - MAX_HOURS + 1, i64::MIN));
    }
}

/// A kill hook (rows in `kill_hooks::PLAYER` and `NPC`): count the death.
///
/// The player table also runs for a drone's death, whose "victim" is no
/// character, so that adds nothing.
pub fn on_kill(w: &mut crate::service::Registry, ev: &KillEvent) {
    let system = ev.system;
    let now = crate::pyutil::filetime_now();
    if ev.npc.is_some() {
        let warzone = w.ext::<crate::facwar::Facwar>().system(system).is_some();
        w.ext_mut::<MapStats>().record(system, now, |c| {
            c.npcs += 1;
            if warzone {
                c.militia_npcs += 1;
            }
        });
        return;
    }
    if w.character(ev.victim_character).is_none() {
        return;
    }
    let pod = ev.ship_type == crate::damage::CAPSULE_TYPE_ID;
    let militia = crate::service::facwar_mgr::my_faction(w, ev.victim_character, now).is_some();
    w.ext_mut::<MapStats>().record(system, now, |c| {
        if pod {
            c.pods += 1;
        } else {
            c.ships += 1;
        }
        if militia {
            c.militia += 1;
        }
    });
}

/// A gate arrival in `system` (called from `jump::tick`).
pub fn on_jump(w: &mut crate::service::Registry, system: i64) {
    let now = crate::pyutil::filetime_now();
    w.ext_mut::<MapStats>()
        .record(system, now, |c| c.jumps += 1);
}

/// `map.GetHistory(statID, hours)`: a `CRowset` of the four columns,
/// wrapped as a `CachedMethodCallResult` so `UpdatePodKillList` (which
/// re-reads only when the cached-call version moves) sees changes. The
/// version is the adler32 of the marshalled rowset, so it moves exactly when
/// the numbers do (the same wrapper as `marketProxy.GetOrders`).
pub fn history_answer(w: &crate::service::Registry, args: &[PyValue]) -> PyValue {
    let int = |i: usize, default: i64| match args.get(i) {
        Some(PyValue::Int(n)) => *n,
        _ => default,
    };
    let now = crate::pyutil::filetime_now();
    let rows = w.ext::<MapStats>().rows(int(0, 0), int(1, 1), now);
    let columns = ["solarSystemID", "value1", "value2", "value3"]
        .into_iter()
        .map(|name| Column {
            name: name.as_bytes().to_vec(),
            db_type: DbType::I4,
        })
        .collect();
    let descriptor = DbRowDescriptor::new(columns).expect("the history columns are a valid layout");
    let mut rowset = eve_rowset::CRowset::new(descriptor);
    for (s, a, b, c) in rows {
        // I4: a count past i32 saturates rather than failing the row.
        let v = |n: i64| DbValue::Int(n.min(i64::from(i32::MAX)));
        rowset
            .push_values(vec![v(s), v(a), v(b), v(c)])
            .expect("the history row matches its descriptor");
    }
    let bytes = crate::wire::dumps(&rowset.to_pyvalue());
    let version = crate::pyutil::adler32(&bytes);
    crate::pyutil::cached_bytes(bytes, version, now)
}

/// `machoNet.GetClusterGameStatisticsForClient('EVE', ...)`: the triple
/// `(sol, sta, divisor)` the pilot filters unpack (`mapSvc.py:407-421`).
///
/// Dicts keyed by `solarSystemID - 30000000`: `sol` counts every pilot in the
/// system, docked ones included; `sta` counts the docked. Known space only.
/// **[A]** divisor 1 and instant counts (TQ averages samples); no falsifier
/// needed, the client only divides.
pub fn pilot_counts(w: &crate::service::Registry) -> PyValue {
    let mut sol: BTreeMap<i64, i64> = BTreeMap::new();
    let mut sta: BTreeMap<i64, i64> = BTreeMap::new();
    for c in w.sessions().online_characters() {
        let (system, docked) = match w.roster().get(c) {
            Some(p) => (p.solar_system_id, false),
            None => match w.character(c).and_then(|ch| ch.solar_system_id) {
                Some(s) => (s, true),
                None => continue,
            },
        };
        if !(30_000_000..31_000_000).contains(&system) {
            continue;
        }
        let key = system - 30_000_000;
        *sol.entry(key).or_default() += 1;
        if docked {
            *sta.entry(key).or_default() += 1;
        }
    }
    let dict = |m: BTreeMap<i64, i64>| {
        // A marshal dict stores (value, key).
        PyValue::Dict(
            m.into_iter()
                .map(|(k, v)| (PyValue::Int(v), PyValue::Int(k)))
                .collect(),
        )
    };
    PyValue::Tuple(vec![dict(sol), dict(sta), PyValue::Int(1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS: i64 = 30_900_001;
    const T: BlueTime = 133_000_000_000_000_000;

    fn stats_with(f: impl FnOnce(&mut MapStats)) -> MapStats {
        let mut s = MapStats::default();
        f(&mut s);
        s
    }

    #[test]
    fn a_jump_now_is_in_both_windows() {
        let s = stats_with(|s| s.record(SYS, T, |c| c.jumps += 1));
        assert_eq!(s.rows(1, 1, T), vec![(SYS, 1, 0, 0)]);
        assert_eq!(s.rows(1, 24, T), vec![(SYS, 1, 0, 0)]);
    }

    #[test]
    fn a_jump_two_hours_ago_is_only_in_the_day() {
        let s = stats_with(|s| s.record(SYS, T - 2 * HOUR_TICKS, |c| c.jumps += 1));
        assert!(s.rows(1, 1, T).is_empty());
        assert_eq!(s.rows(1, 24, T), vec![(SYS, 1, 0, 0)]);
    }

    #[test]
    fn an_npc_death_is_no_ship_kill() {
        let s = stats_with(|s| s.record(SYS, T, |c| c.npcs += 1));
        // ship kills are value1 - value2.
        assert_eq!(s.rows(3, 24, T), vec![(SYS, 1, 1, 0)]);
    }

    #[test]
    fn a_capsule_is_a_pod_and_a_ship_is_a_ship() {
        let s = stats_with(|s| s.record(SYS, T, |c| c.pods += 1));
        assert_eq!(s.rows(3, 1, T), vec![(SYS, 0, 0, 1)]);
        let s = stats_with(|s| s.record(SYS, T, |c| c.ships += 1));
        assert_eq!(s.rows(3, 1, T), vec![(SYS, 1, 0, 0)]);
    }

    #[test]
    fn militia_kills_have_their_own_columns() {
        let s = stats_with(|s| {
            s.record(SYS, T, |c| {
                c.militia += 1;
                c.militia_npcs += 2;
            })
        });
        assert_eq!(s.rows(5, 1, T), vec![(SYS, 3, 2, 0)]);
    }

    #[test]
    fn an_unknown_stat_has_no_rows() {
        let s = stats_with(|s| s.record(SYS, T, |c| c.jumps += 1));
        assert!(s.rows(99, 24, T).is_empty());
    }

    #[test]
    fn prune_drops_old_buckets_and_record_prunes_by_itself() {
        let mut s = stats_with(|s| s.record(SYS, T - 30 * HOUR_TICKS, |c| c.jumps += 1));
        s.prune(T);
        assert!(s.hours.is_empty());
        let mut s = stats_with(|s| s.record(SYS, T - 30 * HOUR_TICKS, |c| c.jumps += 1));
        s.record(SYS + 1, T, |c| c.jumps += 1);
        assert_eq!(
            s.hours.len(),
            1,
            "the 30 h old bucket went with the next record"
        );
    }

    #[test]
    fn the_buckets_are_bounded_by_hours_not_events() {
        let mut s = MapStats::default();
        for i in 0..10_000 {
            s.record(SYS, T + i, |c| c.jumps += 1);
        }
        assert!(s.hours.len() <= 2);
    }

    mod world {
        use super::*;
        use crate::model::{Character, SandboxConfig};
        use crate::service::Registry;
        use crate::session_table::SessionEntry;
        use crate::space_state::{Motion, Presence};

        const A: i64 = 30_900_001;
        const B: i64 = 30_900_002;
        const WH: i64 = 31_900_001;

        fn world() -> Registry {
            let chars = (1..=5)
                .map(|n| Character {
                    character_id: 90_000_000 + n,
                    ship_id: 90_000_100 + n,
                    solar_system_id: Some(if n == 5 { WH } else { A }),
                    ..Character::default()
                })
                .collect();
            let mut reg = Registry::new(SandboxConfig {
                characters: chars,
                ..SandboxConfig::default()
            });
            // 1..=4 online (5 is offline).
            for n in 1..=4 {
                reg.session_opened(SessionEntry {
                    client_id: n,
                    session_id: n,
                    user_id: n,
                    character_id: Some(90_000_000 + n),
                });
            }
            // 1 docked in A, 2 in space in A, 3 in space in B, 4 in a wormhole.
            let fly = |reg: &mut Registry, n: i64, system: i64| {
                reg.roster_mut().arrive(Presence::arriving(
                    90_000_000 + n,
                    90_000_100 + n,
                    system,
                    Motion::arrived([0.0; 3], [0.0; 3]),
                ));
            };
            fly(&mut reg, 2, A);
            fly(&mut reg, 3, B);
            fly(&mut reg, 4, WH);
            reg
        }

        #[test]
        fn pilot_counts_are_keyed_by_id_minus_thirty_million() {
            let reg = world();
            let PyValue::Tuple(t) = pilot_counts(&reg) else {
                panic!("a tuple")
            };
            let pairs = |v: &PyValue| -> Vec<(i64, i64)> {
                let PyValue::Dict(d) = v else {
                    panic!("a dict")
                };
                d.iter()
                    .map(|(v, k)| match (k, v) {
                        (PyValue::Int(k), PyValue::Int(v)) => (*k, *v),
                        _ => panic!("ints"),
                    })
                    .collect()
            };
            assert_eq!(pairs(&t[0]), vec![(900_001, 2), (900_002, 1)]);
            assert_eq!(pairs(&t[1]), vec![(900_001, 1)]);
            assert_eq!(t[2], PyValue::Int(1));
        }

        fn event(victim: i64, ship_type: i64) -> KillEvent {
            KillEvent {
                victim_character: victim,
                victim_ship: victim + 100,
                ship_type,
                killer: None,
                system: A,
                npc: None,
                when: T,
                park: vec![],
            }
        }

        #[test]
        fn a_drone_death_adds_nothing() {
            let mut reg = world();
            on_kill(&mut reg, &event(90_999_999, 900_001));
            assert!(reg.ext::<MapStats>().hours.is_empty());
        }

        #[test]
        fn a_pilots_ship_and_pod_are_counted_apart() {
            let mut reg = world();
            on_kill(&mut reg, &event(90_000_002, 900_001));
            on_kill(&mut reg, &event(90_000_002, crate::damage::CAPSULE_TYPE_ID));
            let now = crate::pyutil::filetime_now();
            assert_eq!(reg.ext::<MapStats>().rows(3, 1, now), vec![(A, 1, 0, 1)]);
        }

        #[test]
        fn the_history_answer_is_a_cached_result_whose_version_tracks_events() {
            let mut reg = world();
            on_jump(&mut reg, A);
            let parts = |a: &PyValue| match a {
                PyValue::Instance { class, state } => {
                    assert_eq!(class, crate::pyutil::CACHED_METHOD_CALL_RESULT.as_bytes());
                    let PyValue::Tuple(t) = &**state else {
                        panic!("tuple")
                    };
                    let PyValue::List(l) = &t[2] else {
                        panic!("list")
                    };
                    (l[1].clone(), t[1].clone())
                }
                _ => panic!("a CachedMethodCallResult"),
            };
            let args = [PyValue::Int(1), PyValue::Int(24)];
            let (v1, b1) = parts(&history_answer(&reg, &args));
            let (v2, _) = parts(&history_answer(&reg, &args));
            assert_eq!(v1, v2);
            let PyValue::Bytes(bytes) = b1 else {
                panic!("bytes")
            };
            crate::wire::loads(&bytes).expect("the rowset marshals back");
            assert!(bytes.windows(6).any(|w| w == b"value3"));
            on_jump(&mut reg, B);
            let (v3, _) = parts(&history_answer(&reg, &args));
            assert_ne!(v1, v3);
        }
    }
}
