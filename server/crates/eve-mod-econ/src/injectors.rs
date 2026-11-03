//! **Injector drops**: an elite or heavy NPC's wreck may hold one skill
//! injector. Like PLEX, a drop is an **item** in the wreck, so it is looted,
//! hauled and lost with the ship. The mechanics of the injector itself (the
//! diminishing table, split and combine, extraction) are the core's
//! (`eve_server::skill_trading`); this is only where one comes from.
//!
//! Injectors come from here and from extraction, never from the store (owner
//! decision, #30). The roll is a pure function of the world seed and the NPC's
//! id, a bot's chance is [`tunables::BOT_INJECTOR_MULT`] times a human's, and
//! past the day's point budget every chance is [`tunables::OVER_BUDGET_MULT`]
//! times itself. Every injector is ledgered in the same step under
//! `drop:<source>` in the core's `sptrade` ledger, at its nominal points
//! (a large injector is a bucket, a small a fifth of one).
//!
//! **Sources not reachable yet**: as for PLEX, only a kill has a hook.

use crate::KIND;
use crate::tunables::{self, InjectorRow};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::FLAG_HANGAR;
use eve_server::module::{Deltas, KillEvent, Settled};
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;
use eve_server::skill_trading::{BUCKET, DIVISOR, TYPE_LARGE, TYPE_SMALL, ledger_minted};

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the NPC's stream so this module's draws are its own, and not
/// the PLEX roll's.
const SALT: u64 = 0x494e_4a58;

fn day_key() -> Vec<String> {
    vec!["spday".to_owned()]
}

/// Points minted by injector drops on `day`: the stored counter, or 0 when it
/// is another day's.
pub fn minted_on(d: &Deltas, day: i64) -> i64 {
    let Some(v) = d.of(KIND).get(&day_key()) else {
        return 0;
    };
    match (v.first().and_then(|s| s.parse::<i64>().ok()), v.get(1)) {
        (Some(stored), Some(n)) if stored == day => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Count `points` minted by another source of this module (a geode) against
/// `day`'s budget; the caller has ledgered them.
pub fn note_minted(reg: &mut Registry, day: i64, points: i64) {
    add_minted(reg.deltas_mut(), day, points);
}

fn add_minted(d: &mut Deltas, day: i64, points: i64) {
    let total = minted_on(d, day) + points;
    d.put(KIND, day_key(), vec![day.to_string(), total.to_string()]);
}

/// Which injector a roll of `row` yields: the type id, or `None`. Always draws
/// both chances, so the stream's use does not depend on the outcome.
pub fn decide(row: &InjectorRow, s: &mut Stream, bot: bool, over_budget: bool) -> Option<i64> {
    let mut scale = 1.0;
    if bot {
        scale *= tunables::BOT_INJECTOR_MULT;
    }
    if over_budget {
        scale *= tunables::OVER_BUDGET_MULT;
    }
    let large = s.chance(row.large * scale);
    let small = s.chance(row.small * scale);
    if large {
        Some(TYPE_LARGE)
    } else if small {
        Some(TYPE_SMALL)
    } else {
        None
    }
}

/// Nominal points of one injector of `type_id`.
pub fn points_of(type_id: i64) -> i64 {
    if type_id == TYPE_SMALL {
        BUCKET / DIVISOR
    } else {
        BUCKET
    }
}

/// The kill-hook row: an NPC killed by a character may leave an injector in
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
    let Some(row) = tunables::injector_row_for(npc.bounty) else {
        return;
    };
    let Some(bot) = ev.killer.and_then(|k| reg.character(k)).map(|c| c.is_bot) else {
        return;
    };
    let day = ev.when.div_euclid(DAY_TICKS);
    let over = minted_on(reg.deltas(), day) >= tunables::DAILY_SP_BUDGET;
    let mut s = seed::stream(reg.world().seed, Kind::Loot, &[key as u64, SALT]);
    let Some(type_id) = decide(row, &mut s, bot, over) else {
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
    leave_at(reg, to, row.source, type_id, day);
}

/// Put one injector of `type_id` in `wreck` and, only if the store took it,
/// ledger its points under `drop:<source>` and count them against `day`'s
/// budget. `true` when it did. Quiet, as PLEX's.
pub fn leave_in(reg: &mut Registry, wreck: &Wreck, source: &str, type_id: i64, day: i64) -> bool {
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    leave_at(reg, to, source, type_id, day)
}

/// As [`leave_in`], into any slot: a wreck, or a parked killer's hangar.
pub fn leave_at(reg: &mut Registry, to: Slot, source: &str, type_id: i64, day: i64) -> bool {
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: 1,
        to,
    }]);
    if !matches!(made.as_deref(), Ok([Change::Created { .. }, ..])) {
        return false;
    }
    let points = points_of(type_id);
    ledger_minted(reg.deltas_mut(), &format!("drop:{source}"), points);
    add_minted(reg.deltas_mut(), day, points);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: InjectorRow = InjectorRow {
        source: "t",
        min_bounty: 0.0,
        large: 0.2,
        small: 0.5,
    };

    fn count(bot: bool, over: bool) -> (usize, usize) {
        let mut l = 0;
        let mut sm = 0;
        for i in 0..4_000u64 {
            match decide(&ROW, &mut Stream::new(i), bot, over) {
                Some(TYPE_LARGE) => l += 1,
                Some(TYPE_SMALL) => sm += 1,
                _ => {}
            }
        }
        (l, sm)
    }

    #[test]
    fn a_roll_is_deterministic() {
        for i in 0..200u64 {
            assert_eq!(
                decide(&ROW, &mut Stream::new(i), false, false),
                decide(&ROW, &mut Stream::new(i), false, false)
            );
        }
    }

    #[test]
    fn a_bot_and_a_late_day_scale_every_chance() {
        let (human, bot, late) = (count(false, false), count(true, false), count(false, true));
        // 4000 draws; wide bands, the stream is fixed. A large first (0.2),
        // else a small of the other 0.8 at 0.5.
        assert!((650..950).contains(&human.0), "{human:?}");
        assert!((1_400..1_800).contains(&human.1), "{human:?}");
        // Half and a quarter, by the multipliers, whatever they are set to.
        let ratio = |a: usize, b: usize| a as f64 / b as f64;
        assert!((ratio(bot.0, human.0) - tunables::BOT_INJECTOR_MULT).abs() < 0.12);
        assert!((ratio(late.0, human.0) - tunables::OVER_BUDGET_MULT).abs() < 0.12);
    }

    #[test]
    fn the_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let _ = decide(&ROW, &mut a, false, false);
        let _ = decide(
            &InjectorRow {
                large: 0.0,
                small: 0.0,
                ..ROW
            },
            &mut b,
            false,
            false,
        );
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rows_are_ordered_from_the_top_and_a_cheap_rat_has_none() {
        assert!(
            tunables::INJECTOR_DROPS
                .windows(2)
                .all(|w| w[0].min_bounty > w[1].min_bounty)
        );
        assert!(tunables::injector_row_for(0.0).is_none());
        assert!(tunables::injector_row_for(f64::MAX).is_some());
        assert!(
            tunables::INJECTOR_DROPS
                .iter()
                .all(|r| (0.0..=1.0).contains(&r.large) && (0.0..=1.0).contains(&r.small))
        );
    }

    #[test]
    fn a_small_is_worth_a_fifth() {
        assert_eq!(points_of(TYPE_SMALL) * DIVISOR, points_of(TYPE_LARGE));
    }
}
