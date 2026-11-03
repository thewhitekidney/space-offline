//! **PLEX drops**: an elite NPC's wreck may hold a stack of PLEX. A drop is
//! an **item** in the wreck (`FLAG_IN_WRECK`), so it is looted, hauled and
//! lost with the ship; the vault is never touched here.
//!
//! The roll is a pure function of the world seed and the NPC's id
//! ([`eve_server::seed::stream`], `Kind::Loot`), so a replay drops the same
//! thing. A bot's chance is [`tunables::BOT_PLEX_MULT`] times a human's, and
//! past the day's budget every chance is [`tunables::OVER_BUDGET_MULT`] times
//! itself. Every unit is ledgered under `drop:<source>` in the same step.
//!
//! **Sources not reachable yet** (they need a core hook each, listed in the
//! lane report): a combat site's or an escalation's completion, and a won
//! data or relic can. Only a kill has a hook today. A kill by a drone has no
//! character killer and drops nothing.

use crate::KIND;
use crate::tunables::{self, DropRow};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::FLAG_HANGAR;
use eve_server::module::Deltas;
use eve_server::module::{KillEvent, Settled};
use eve_server::nes_vault;
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the NPC's stream so this module's draws are its own.
const SALT: u64 = 0x504c_4558;

fn day_key() -> Vec<String> {
    vec!["day".to_owned()]
}

/// PLEX minted by drops on `day` (a day number): the stored counter, or 0
/// when it is another day's.
pub fn minted_on(d: &Deltas, day: i64) -> i64 {
    let Some(v) = d.of(KIND).get(&day_key()) else {
        return 0;
    };
    match (v.first().and_then(|s| s.parse::<i64>().ok()), v.get(1)) {
        (Some(stored), Some(n)) if stored == day => n.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Count `units` minted by another source of this module (a geode) against
/// `day`'s budget; the caller has ledgered them.
pub fn note_minted(reg: &mut Registry, day: i64, units: i64) {
    add_minted(reg.deltas_mut(), day, units);
}

fn add_minted(d: &mut Deltas, day: i64, units: i64) {
    let total = minted_on(d, day) + units;
    d.put(KIND, day_key(), vec![day.to_string(), total.to_string()]);
}

/// The quantity a roll of `row` yields, or `None` for no drop. Always draws
/// the chance first and the quantity second, so the stream's use does not
/// depend on the outcome.
pub fn decide(row: &DropRow, s: &mut Stream, bot: bool, over_budget: bool) -> Option<i64> {
    let mut chance = row.chance;
    if bot {
        chance *= tunables::BOT_PLEX_MULT;
    }
    if over_budget {
        chance *= tunables::OVER_BUDGET_MULT;
    }
    let hit = s.chance(chance);
    let lo = row.min.max(1) as u64;
    let hi = row.max.max(row.min.max(1)) as u64;
    let qty = s.range(lo, hi + 1) as i64;
    hit.then_some(qty)
}

/// The kill-hook row: an NPC killed by a character may leave PLEX in its
/// wreck. Runs after the wreck is made (`kill_hooks::NPC`).
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
    let Some(row) = tunables::row_for(npc.bounty) else {
        return;
    };
    let Some(bot) = ev.killer.and_then(|k| reg.character(k)).map(|c| c.is_bot) else {
        return;
    };
    let day = ev.when.div_euclid(DAY_TICKS);
    let over = minted_on(reg.deltas(), day) >= tunables::DAILY_BUDGET;
    let mut s = seed::stream(reg.world().seed, Kind::Loot, &[key as u64, SALT]);
    let Some(qty) = decide(row, &mut s, bot, over) else {
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
    leave_at(reg, to, row.source, qty, day);
}

/// Put `qty` PLEX in `wreck` and, only if the store took it, ledger it under
/// `drop:<source>` and count it against `day`'s budget. `true` when it did.
///
/// Quiet: nobody owns the wreck's contents, and a looter's client lists the
/// wreck when it opens it (as the wreck's own loot is made).
pub fn leave_in(reg: &mut Registry, wreck: &Wreck, source: &str, qty: i64, day: i64) -> bool {
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    leave_at(reg, to, source, qty, day)
}

/// As [`leave_in`], into any slot: a wreck, or a parked killer's hangar.
pub fn leave_at(reg: &mut Registry, to: Slot, source: &str, qty: i64, day: i64) -> bool {
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id: nes_vault::TYPE_PLEX,
        quantity: qty,
        to,
    }]);
    if !matches!(made.as_deref(), Ok([Change::Created { .. }, ..])) {
        return false;
    }
    nes_vault::ledger_in(reg.deltas_mut(), &format!("drop:{source}"), qty);
    add_minted(reg.deltas_mut(), day, qty);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: DropRow = DropRow {
        source: "t",
        min_bounty: 0.0,
        chance: 0.5,
        min: 3,
        max: 7,
    };

    fn hits(bot: bool, over: bool) -> usize {
        (0..4_000u64)
            .filter(|i| decide(&ROW, &mut Stream::new(*i), bot, over).is_some())
            .count()
    }

    #[test]
    fn a_roll_is_deterministic_and_in_range() {
        for i in 0..200u64 {
            let a = decide(&ROW, &mut Stream::new(i), false, false);
            assert_eq!(a, decide(&ROW, &mut Stream::new(i), false, false));
            if let Some(q) = a {
                assert!((3..=7).contains(&q), "{q}");
            }
        }
    }

    #[test]
    fn a_bot_rolls_half_as_often_and_a_late_day_a_quarter() {
        let (human, bot, late) = (hits(false, false), hits(true, false), hits(false, true));
        // 4000 draws at 0.5, 0.25 and 0.125; wide bands, the stream is fixed.
        assert!((1_800..2_200).contains(&human), "{human}");
        assert!((850..1_150).contains(&bot), "{bot}");
        assert!((400..600).contains(&late), "{late}");
    }

    #[test]
    fn the_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let _ = decide(&ROW, &mut a, false, false);
        let _ = decide(&DropRow { chance: 0.0, ..ROW }, &mut b, false, false);
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rows_are_ordered_from_the_top_and_a_cheap_rat_has_none() {
        assert!(
            tunables::DROPS
                .windows(2)
                .all(|w| w[0].min_bounty > w[1].min_bounty)
        );
        assert!(tunables::row_for(0.0).is_none());
        assert!(tunables::row_for(f64::MAX).is_some());
        assert!(
            tunables::DROPS
                .iter()
                .all(|r| { (0.0..=1.0).contains(&r.chance) && r.min >= 1 && r.max >= r.min })
        );
    }
}
