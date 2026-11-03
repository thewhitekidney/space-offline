//! **Booster blueprint drops** (issue #80, owner decision D1): an elite or
//! heavy NPC's wreck may hold one **blueprint copy** of a drug booster. With
//! the NPC-sold reaction formulas (`eve_server::econ_exchange`) this is the
//! only source of the booster chain: no store, no bot kit, no GM grant in
//! closed play. Like PLEX and filaments, a drop is an **item** in the wreck,
//! so it is looted, hauled and lost with the ship.
//!
//! The types are not named here. The core selects them from the bundle
//! (`eve_server::booster_sources::blueprints_in`: the manufacturing blueprints
//! of the published group-303 types that have a duration and a side effect),
//! so a rebuilt bundle changes the set and no id is written. Every one is
//! equally likely **[A]**; the tiers share the weight. A copy has the roll's
//! runs, ME 0 and TE 0 (a copy that was never researched).
//!
//! The roll is a pure function of the world seed and the NPC's id, a bot's
//! chance is [`tunables::BOT_BPC_MULT`] times a human's, and past the day's
//! count every chance is [`tunables::OVER_BUDGET_MULT`] times itself.
//!
//! **Sources not reachable yet**: as for PLEX, only a kill has a hook.

use crate::KIND;
use crate::tunables::{self, BpcRow};
use eve_server::combat::{FLAG_IN_WRECK, Wreck};
use eve_server::industry::BlueprintState;
use eve_server::item::{Change, Edit, Slot};
use eve_server::model::FLAG_HANGAR;
use eve_server::module::{Deltas, KillEvent, Settled};
use eve_server::seed::{self, Kind, Stream};
use eve_server::service::Registry;

/// One day of blue time (100 ns ticks).
const DAY_TICKS: i64 = 864_000_000_000;

/// Mixed into the NPC's stream so this module's draws are its own.
const SALT: u64 = 0x4250_4344;

fn day_key() -> Vec<String> {
    vec!["bday".to_owned()]
}

/// Copies dropped on `day`: the stored counter, or 0 when it is another day's.
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

/// What a roll yields: the chance and the pick are drawn first (the pick as a
/// unit and the runs), so the stream's use does not depend on the outcome.
/// `Some(unit, runs)`; [`pick`] turns the unit into a blueprint.
pub fn decide(row: &BpcRow, s: &mut Stream, bot: bool, over_budget: bool) -> Option<(f64, i64)> {
    let mut chance = row.chance;
    if bot {
        chance *= tunables::BOT_BPC_MULT;
    }
    if over_budget {
        chance *= tunables::OVER_BUDGET_MULT;
    }
    let hit = s.chance(chance);
    let which = s.unit();
    let lo = row.min.max(1) as u64;
    let hi = row.max.max(row.min.max(1)) as u64;
    let runs = s.range(lo, hi + 1) as i64;
    hit.then_some((which, runs))
}

/// The blueprint a unit draw picks from `types` (sorted), or `None` if empty.
pub fn pick(types: &[i64], unit: f64) -> Option<i64> {
    let i = ((unit * types.len() as f64) as usize).min(types.len().checked_sub(1)?);
    types.get(i).copied()
}

/// The kill-hook row: an NPC killed by a character may leave a booster
/// blueprint copy in its wreck. Runs after the wreck is made
/// (`kill_hooks::NPC`).
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
    let Some(row) = tunables::bpc_row_for(npc.bounty) else {
        return;
    };
    let Some(bot) = ev.killer.and_then(|k| reg.character(k)).map(|c| c.is_bot) else {
        return;
    };
    let day = ev.when.div_euclid(DAY_TICKS);
    let over = dropped_on(reg.deltas(), day) >= tunables::DAILY_BPCS;
    let mut s = seed::stream(reg.world().seed, Kind::Loot, &[key as u64, SALT]);
    let Some((unit, runs)) = decide(row, &mut s, bot, over) else {
        return;
    };
    // Selected only on a hit: the scan of the type table is not a per-kill cost.
    let types = eve_server::booster_sources::blueprints_in(reg.content());
    let Some(type_id) = pick(&types, unit) else {
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
    leave_at(reg, to, type_id, runs, day);
}

/// Put one copy of `type_id` with `runs` runs in `wreck` and, only if the
/// store took it, count it against `day`. `true` when it did. Quiet, as PLEX's.
pub fn leave_in(reg: &mut Registry, wreck: &Wreck, type_id: i64, runs: i64, day: i64) -> bool {
    let to = Slot::new(wreck.owner_id, wreck.wreck_id, FLAG_IN_WRECK);
    leave_at(reg, to, type_id, runs, day)
}

/// As [`leave_in`], into any slot: a wreck, or a parked killer's hangar.
pub fn leave_at(reg: &mut Registry, to: Slot, type_id: i64, runs: i64, day: i64) -> bool {
    let made = reg.apply_items_quiet(&[Edit::Create {
        type_id,
        quantity: 1,
        to,
    }]);
    let Ok([Change::Created { item_id, .. }, ..]) = made.as_deref() else {
        return false;
    };
    reg.set_blueprint_state(BlueprintState {
        item_id: *item_id,
        runs_remaining: runs,
        material_efficiency: 0,
        time_efficiency: 0,
    });
    add_dropped(reg.deltas_mut(), day);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: BpcRow = BpcRow {
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
    fn a_roll_is_deterministic_and_its_runs_are_in_range() {
        for i in 0..200u64 {
            let a = decide(&ROW, &mut Stream::new(i), false, false);
            assert_eq!(a, decide(&ROW, &mut Stream::new(i), false, false));
            if let Some((u, runs)) = a {
                assert!((0.0..1.0).contains(&u), "{u}");
                assert!((3..=7).contains(&runs), "{runs}");
            }
        }
    }

    #[test]
    fn a_pick_names_only_a_listed_blueprint() {
        let types = [11, 12, 13, 14];
        for i in 0..=100 {
            let t = pick(&types, f64::from(i) / 100.0).expect("one");
            assert!(types.contains(&t), "{t}");
        }
        assert_eq!(pick(&types, 0.0), Some(11));
        assert_eq!(pick(&types, 0.999), Some(14));
        assert_eq!(pick(&[], 0.3), None);
    }

    #[test]
    fn a_bot_and_a_late_day_scale_the_chance() {
        let (human, bot, late) = (
            hits(false, false) as f64,
            hits(true, false) as f64,
            hits(false, true) as f64,
        );
        assert!((bot / human - tunables::BOT_BPC_MULT).abs() < 0.12);
        assert!((late / human - tunables::OVER_BUDGET_MULT).abs() < 0.12);
    }

    #[test]
    fn the_stream_use_does_not_depend_on_the_outcome() {
        let mut a = Stream::new(9);
        let mut b = Stream::new(9);
        let _ = decide(&ROW, &mut a, false, false);
        let _ = decide(&BpcRow { chance: 0.0, ..ROW }, &mut b, false, false);
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rows_are_ordered_from_the_top_and_a_cheap_rat_has_none() {
        assert!(
            tunables::BPC_DROPS
                .windows(2)
                .all(|w| w[0].min_bounty > w[1].min_bounty)
        );
        assert!(tunables::bpc_row_for(0.0).is_none());
        assert!(tunables::bpc_row_for(f64::MAX).is_some());
        assert!(
            tunables::BPC_DROPS
                .iter()
                .all(|r| (0.0..=1.0).contains(&r.chance) && r.min >= 1 && r.max >= r.min)
        );
    }
}
