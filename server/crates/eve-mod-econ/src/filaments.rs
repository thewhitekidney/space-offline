//! **Needlejack filament drops** (issue #88): an elite or heavy NPC's wreck may
//! hold one Needlejack filament. This is the **only** supply: no store, no
//! bot kit, no GM grant in closed play (`handback/r-pve-next/report.md` §2,
//! decision D2). Like PLEX and injectors, a drop is an **item** in the wreck,
//! so it is looted, hauled and lost with the ship.
//!
//! The types are not named here. The core reads them from the bundle
//! (`eve_server::random_jump::filament_types`: the filament group with a pilot
//! count), so a rebuilt bundle changes the set and no id is written. Which one
//! drops is weighted by the pilots a filament takes ([`weight`]): the small
//! gangs' filaments are the common ones, the 25-pilot ones the rare. Signal and
//! Noise are alike.
//!
//! The roll is a pure function of the world seed and the NPC's id, a bot's
//! chance is [`tunables::BOT_FILAMENT_MULT`] times a human's, and past the
//! day's count every chance is [`tunables::OVER_BUDGET_MULT`] times itself.
//!
//! **Sources not reachable yet**: as for PLEX, only a kill has a hook.

use crate::KIND;
use crate::tunables::{self, FilamentRow};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::FLAG_HANGAR;
use eve_server::module::{Deltas, KillEvent, Settled};
use eve_server::random_jump::Filament;
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the NPC's stream so this module's draws are its own.
const SALT: u64 = 0x4e44_4c4a;

fn day_key() -> Vec<String> {
    vec!["fday".to_owned()]
}

/// Filaments dropped on `day`: the stored counter, or 0 when it is another
/// day's.
pub fn dropped_on(d: &Deltas, day: i64) -> i64 {
    let Some(v) = d.of(KIND).get(&day_key()) else {
        return 0;
    };
    match (v.first().and_then(|s| s.parse::<i64>().ok()), v.get(1)) {
        (Some(stored), Some(n)) if stored == day => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn add_dropped(d: &mut Deltas, day: i64) {
    let total = dropped_on(d, day) + 1;
    d.put(KIND, day_key(), vec![day.to_string(), total.to_string()]);
}

/// How common a filament is among the drops: the reciprocal of the pilots it
/// takes (so a 5-pilot one drops five times as often as a 25-pilot one).
pub fn weight(f: &Filament) -> f64 {
    1.0 / f.pilots.max(1) as f64
}

/// Which filament a roll of `row` yields, or `None`. Always draws the chance
/// first and the pick second, so the stream's use does not depend on the
/// outcome.
pub fn decide(
    row: &FilamentRow,
    types: &[Filament],
    s: &mut Stream,
    bot: bool,
    over_budget: bool,
) -> Option<i64> {
    let mut chance = row.chance;
    if bot {
        chance *= tunables::BOT_FILAMENT_MULT;
    }
    if over_budget {
        chance *= tunables::OVER_BUDGET_MULT;
    }
    let hit = s.chance(chance);
    let mut pick = s.unit() * types.iter().map(weight).sum::<f64>();
    let chosen = types.iter().find(|f| {
        pick -= weight(f);
        pick < 0.0
    });
    hit.then_some(chosen.or(types.last())?.type_id)
}

/// The kill-hook row: an NPC killed by a character may leave a filament in
/// its wreck. Runs after the wreck is made (`kill_hooks::NPC`).
pub fn on_npc_kill(reg: &mut Registry, ev: &KillEvent) {
    roll(reg, ev, ev.victim_character, None);
}

/// The settle-hook row: an NPC killed in an offline settlement rolls the
/// same table, keyed by the kill's seed, and pays the killer's hangar
/// (`settle_hooks`).
pub fn on_settled(reg: &mut Registry, s: &Settled) {
    let Some(killer) = s.ev.killer else { return };
    let hangar = Slot::new(killer, s.station, FLAG_HANGAR);
    roll(reg, &s.ev, s.seed, Some(hangar));
}

/// One roll for `ev`, keyed by `key`: into the wreck of `ev`, or into
/// `hangar` when there is no wreck.
fn roll(reg: &mut Registry, ev: &KillEvent, key: i64, hangar: Option<Slot>) {
    let Some(npc) = ev.npc else { return };
    let Some(row) = tunables::filament_row_for(npc.bounty) else {
        return;
    };
    let Some(bot) = ev.killer.and_then(|k| reg.character(k)).map(|c| c.is_bot) else {
        return;
    };
    let types = eve_server::random_jump::filament_types(reg.content());
    if types.is_empty() {
        return;
    }
    let day = ev.when.div_euclid(DAY_TICKS);
    let over = dropped_on(reg.deltas(), day) >= tunables::DAILY_FILAMENTS;
    let mut s = seed::stream(reg.world().seed, Kind::Loot, &[key as u64, SALT]);
    let Some(type_id) = decide(row, &types, &mut s, bot, over) else {
        return;
    };
    let to = match hangar {
        Some(h) => h,
        None => {
            let Some(wreck) = reg
                .combat()
                .wrecks()
                .find(|w| w.ship_id == ev.victim_ship)
                .copied()
            else {
                return;
            };
            Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK)
        }
    };
    leave_at(reg, to, type_id, day);
}

/// Put one filament of `type_id` in `wreck` and, only if the store took it,
/// count it against `day`. `true` when it did. Quiet, as PLEX's.
pub fn leave_in(reg: &mut Registry, wreck: &Wreck, type_id: i64, day: i64) -> bool {
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    leave_at(reg, to, type_id, day)
}

/// As [`leave_in`], into any slot: a wreck, or a parked killer's hangar.
pub fn leave_at(reg: &mut Registry, to: Slot, type_id: i64, day: i64) -> bool {
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: 1,
        to,
    }]);
    if !matches!(made.as_deref(), Ok([Change::Created { .. }, ..])) {
        return false;
    }
    add_dropped(reg.deltas_mut(), day);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const ROW: FilamentRow = FilamentRow {
        source: "t",
        min_bounty: 0.0,
        chance: 0.5,
    };

    fn kind(type_id: i64, pilots: i64) -> Filament {
        Filament {
            type_id,
            pilots,
            signal: false,
            radius_m: 6_000.0,
            spool: Duration::from_secs(30),
        }
    }

    fn types() -> Vec<Filament> {
        vec![kind(11, 5), kind(12, 15), kind(13, 25)]
    }

    fn rolls(bot: bool, over: bool) -> Vec<Option<i64>> {
        (0..6_000u64)
            .map(|i| decide(&ROW, &types(), &mut Stream::new(i), bot, over))
            .collect()
    }

    #[test]
    fn a_roll_is_deterministic_and_names_a_listed_type() {
        for i in 0..200u64 {
            let a = decide(&ROW, &types(), &mut Stream::new(i), false, false);
            assert_eq!(a, decide(&ROW, &types(), &mut Stream::new(i), false, false));
            assert!(a.is_none_or(|t| [11, 12, 13].contains(&t)));
        }
        assert_eq!(decide(&ROW, &[], &mut Stream::new(1), false, false), None);
    }

    #[test]
    fn small_gangs_filaments_are_the_common_ones() {
        let all = rolls(false, false);
        let n = |t: i64| all.iter().filter(|x| **x == Some(t)).count() as f64;
        // 1/5 : 1/15 : 1/25 -> 15 : 5 : 3 of 23, of the 3000 that hit.
        assert!((n(11) / n(12) - 3.0).abs() < 0.5, "{} {}", n(11), n(12));
        assert!(
            (n(12) / n(13) - 5.0 / 3.0).abs() < 0.4,
            "{} {}",
            n(12),
            n(13)
        );
        let hits = all.iter().flatten().count();
        assert!((2_700..3_300).contains(&hits), "{hits}");
    }

    #[test]
    fn a_bot_and_a_late_day_scale_the_chance() {
        let hits = |a: Vec<Option<i64>>| a.iter().flatten().count() as f64;
        let (human, bot, late) = (
            hits(rolls(false, false)),
            hits(rolls(true, false)),
            hits(rolls(false, true)),
        );
        assert!((bot / human - tunables::BOT_FILAMENT_MULT).abs() < 0.12);
        assert!((late / human - tunables::OVER_BUDGET_MULT).abs() < 0.12);
    }

    #[test]
    fn the_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let _ = decide(&ROW, &types(), &mut a, false, false);
        let _ = decide(
            &FilamentRow { chance: 0.0, ..ROW },
            &types(),
            &mut b,
            false,
            false,
        );
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rows_are_ordered_from_the_top_and_a_cheap_rat_has_none() {
        assert!(
            tunables::FILAMENT_DROPS
                .windows(2)
                .all(|w| w[0].min_bounty > w[1].min_bounty)
        );
        assert!(tunables::filament_row_for(0.0).is_none());
        assert!(tunables::filament_row_for(f64::MAX).is_some());
        assert!(
            tunables::FILAMENT_DROPS
                .iter()
                .all(|r| (0.0..=1.0).contains(&r.chance))
        );
    }
}
