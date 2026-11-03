//! **The fighters' settlement**: `ratter`, `site_runner` and `fw_plexer`
//! (R-ECON §1.5: "a ratter's bounties and loot"; §2.2 "ammo about 200 rounds
//! a kill ... a goods sink").
//!
//! 1. **The guns** ([`guns_of`]): the active hull's fitted turrets and the
//!    charge loaded in each, with the server's own per-shot arithmetic
//!    (`combat::volley_fitted`: the dogma sheet's modified damage and
//!    multiplier) and the gun's cycle. The dominant charge type -- most guns,
//!    lowest type id on a tie -- is the ammunition; a gun with another
//!    charge, or none, does not count. No guns, no fight.
//! 2. **The rounds**: what the guns hold loaded plus the spares in the
//!    hangar and the hull's cargo. A fight ends when the next squad needs more
//!    rounds than are left ([`rules::Stop::Ammo`]): the agent re-arms only
//!    when its pilot does.
//! 3. **The squads** ([`rules::fight`]): seeded draws -- `belt_squad_in` for a
//!    belt, the same pool drawn [`SITE_SEC_DROP`] harder for an anomaly --
//!    each member's hit points the hull's shield, armor and structure
//!    (`NpcType::hull`), killed at the guns' damage a second times
//!    [`rules::HIT_EFFICIENCY`]. The seed is `(world seed, character, run,
//!    k)`, so the same settlement draws the same squads.
//! 4. **The kills**: each member's bounty is booked through the dynamic
//!    bounty system (`npc::bounty::book_kill`: DBS multiplier, DBS drain,
//!    corp-tax skim, the ref-85 row with its `NBL` description), then the
//!    pending payouts are flushed; its loot is `npc::loot::loot_in`'s seeded
//!    draw (the same function a live wreck uses), straight into the hangar; the
//!    kill's security credit and the system's military index are the live
//!    hooks'.
//! 5. **The rounds are consumed** from the spares first, then the loaded
//!    stacks: a goods sink the closed-loop ledger counts.
//! 6. **The loot is sold** into the bids at the station ([`shop::sell_where`]):
//!    modules, charges and tags that some bid takes at the floor, never the
//!    ammunition or a fitted type.
//!
//! **Not settled**: the hull's death. A live ratter loses ships; an offline
//! one does not (lane note "Not done"), so the goods sink of ship losses is
//! the live bots' alone.

use super::{Agent, Ground, rules, shop};
use crate::damage::BlueTime;
use crate::item::{Item, Slot};
use crate::service::Registry;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// How much harder than the belt an anomaly's squads are drawn, in security
/// **[A]**: a site is the pool of a system 0.3 less secure. Falsified by a
/// live site runner's bounty an hour against [`Ground::Site`]'s settled one
/// in the `[soc]` ledger.
pub const SITE_SEC_DROP: f64 = 0.3;

/// The hit points of a member the NPC table cannot resolve **[A]**: a
/// frigate's, so it is never free to kill.
pub const FALLBACK_EHP: f64 = 2_000.0;

/// The guns of a fit, grouped under their ammunition.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Guns {
    /// The charge type that is fired.
    pub ammo: i64,
    pub battery: rules::Battery,
    /// Every gun type fitted (never sold).
    pub fitted: BTreeSet<i64>,
    /// The loaded charge stacks of the guns that fire `ammo`: `(item, qty)`.
    pub loaded: Vec<(i64, i64)>,
}

/// **The active hull's guns**, `None` when none can fire.
pub(super) fn guns_of(reg: &Registry, character: i64) -> Option<Guns> {
    let c = reg.character(character)?;
    let ship = c.ship_id;
    // (gun item, gun type, charge item, charge type, qty, volley, cycle s)
    let mut guns = Vec::new();
    let mut fitted = BTreeSet::new();
    for m in crate::modules::slim_modules(reg.store(), ship) {
        let Some(mt) = crate::modules::find_in(reg.content(), m.type_id) else {
            continue;
        };
        if crate::modules::is_launcher(mt)
            || crate::combat::weapon_in(reg.content(), m.type_id).is_none()
        {
            continue;
        }
        fitted.insert(m.type_id);
        let Some(charge) = crate::modules::loaded(reg.store(), ship, m.flag_id) else {
            continue;
        };
        let ms = mt.duration_ms();
        if ms <= 0 {
            continue;
        }
        let Some(volley) =
            crate::combat::volley_fitted(reg, ship, m.item_id, m.type_id, Some(charge.type_id))
        else {
            continue;
        };
        guns.push((
            charge.item_id,
            charge.type_id,
            charge.quantity,
            volley.total(),
            ms as f64 / 1000.0,
        ));
    }
    let mut by_type: BTreeMap<i64, usize> = BTreeMap::new();
    for g in &guns {
        *by_type.entry(g.1).or_default() += 1;
    }
    // Most guns; the lowest type id on a tie (BTreeMap order + strict >).
    let (ammo, _) = by_type
        .iter()
        .fold(None::<(i64, usize)>, |best, (&t, &n)| match best {
            Some((_, bn)) if bn >= n => best,
            _ => Some((t, n)),
        })?;
    let mine: Vec<_> = guns.iter().filter(|g| g.1 == ammo).collect();
    let dps: f64 = mine.iter().map(|g| g.3 / g.4).sum();
    let shots: f64 = mine.iter().map(|g| 1.0 / g.4).sum();
    if !rules::positive(dps) {
        return None;
    }
    let mut loaded: Vec<(i64, i64)> = mine.iter().map(|g| (g.0, g.2)).collect();
    loaded.sort_unstable();
    loaded.dedup();
    Some(Guns {
        ammo,
        battery: rules::Battery {
            dps,
            shots_per_s: shots,
        },
        fitted,
        loaded,
    })
}

/// The spare rounds of `ammo`: `(hangar stacks, cargo stacks)` as
/// `(item, qty)`, ascending by item id.
pub(super) fn spares(reg: &Registry, character: i64, station: i64, ammo: i64) -> Vec<(i64, i64)> {
    let ship = reg.character(character).map_or(0, |c| c.ship_id);
    let mut v: Vec<(i64, i64)> = [
        Slot::new(character, station, crate::model::FLAG_HANGAR),
        Slot::new(character, ship, crate::fitting::FLAG_CARGO),
    ]
    .into_iter()
    .flat_map(|s| {
        reg.store()
            .at(s)
            .filter(|i| i.type_id == ammo && !i.singleton)
            .map(|i| (i.item_id, i.quantity))
            .collect::<Vec<_>>()
    })
    .collect();
    v.sort_unstable();
    v
}

/// One squad of the ground, for `parts` (the seed's tail).
fn squad_for(reg: &Registry, ground: Ground, system: i64, parts: &[u64]) -> crate::npc_gen::Squad {
    match ground {
        Ground::Belt => crate::npc_gen::belt_squad_in(reg, system, parts),
        Ground::Site => {
            let sec = reg
                .content()
                .get::<crate::celestial::Universe>()
                .system(system)
                .map_or(1.0, |s| s.security);
            let mut all = vec![system as u64];
            all.extend_from_slice(parts);
            crate::npc_gen::belt_squad(
                reg.content().get::<crate::npc_data::Table>(),
                crate::npc::pirate_of(reg, system),
                (sec - SITE_SEC_DROP).max(-1.0),
                reg.world().seed,
                &all,
            )
        }
    }
}

/// A member's hit points: shield, armor and structure of its hull.
fn ehp_of(reg: &mut Registry, type_id: i64) -> f64 {
    crate::npc::resolve(reg, type_id).map_or(FALLBACK_EHP, |k| {
        k.hull.shield_capacity + k.hull.armor_hp + k.hull.hp
    })
}

/// The seed id of the `m`-th member of the `k`-th squad of run `run`: only
/// a stream key for the loot draw, `splitmix`-mixed so no two members share
/// one. Positive, and far outside any live NPC id.
pub(super) fn npc_seed(character: i64, run: u64, k: usize, m: usize) -> i64 {
    let mut z = (character as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(run.rotate_left(17))
        .wrapping_add((k as u64).rotate_left(33))
        .wrapping_add((m as u64).rotate_left(49))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) >> 2) as i64 | (1 << 61)
}

/// Settle one fighter for `work_h` hours.
pub(super) fn settle(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    ground: Ground,
    work_h: f64,
    _now: BlueTime,
    notes: &mut Vec<String>,
) {
    let Some(guns) = guns_of(reg, character) else {
        notes.push("no loaded guns on the active hull".to_owned());
        sell_loot(reg, character, a, None, notes);
        return;
    };
    if work_h > 0.0 {
        fight_squads(reg, character, a, ground, &guns, work_h, notes);
    }
    sell_loot(reg, character, a, Some(&guns), notes);
}

fn fight_squads(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    ground: Ground,
    guns: &Guns,
    work_h: f64,
    notes: &mut Vec<String>,
) {
    let Some(system) = reg.character(character).and_then(|c| c.solar_system_id) else {
        notes.push("no home system".to_owned());
        return;
    };
    let spare = spares(reg, character, a.station, guns.ammo);
    let rounds_have: i64 =
        spare.iter().map(|s| s.1).sum::<i64>() + guns.loaded.iter().map(|s| s.1).sum::<i64>();
    let mut drawn: Vec<crate::npc_gen::Squad> = Vec::new();
    let (fight, killed) = rules::fight(&guns.battery, rounds_have, work_h * rules::HOUR_S, |k| {
        let squad = squad_for(reg, ground, system, &[character as u64, a.runs, k as u64]);
        let ehp = squad.members.iter().map(|&t| ehp_of(reg, t)).sum();
        let stat = rules::SquadStat {
            ehp,
            members: squad.members.len(),
        };
        drawn.push(squad);
        stat
    });
    notes.push(format!(
        "{} squads, {} rounds, stopped on {:?}",
        fight.squads, fight.rounds, fight.stopped
    ));
    let at = crate::pyutil::filetime_now();
    let mut loot: BTreeMap<i64, i64> = BTreeMap::new();
    for (k, squad) in drawn.iter().take(killed.len()).enumerate() {
        for (m, &type_id) in squad.members.iter().enumerate() {
            let Some(kind) = crate::npc::resolve(reg, type_id) else {
                continue;
            };
            if kind.bounty.is_finite() && kind.bounty > 0.0 {
                crate::npc::bounty::book_kill(
                    reg,
                    character,
                    system,
                    type_id,
                    kind.bounty,
                    Duration::ZERO,
                );
            }
            for (t, n) in crate::npc::loot_in(reg, kind, npc_seed(character, a.runs, k, m), system)
            {
                *loot.entry(t).or_default() += n;
            }
            if let Some(g) = crate::npc::bounty::sec_gain_of(reg.content(), type_id) {
                crate::npc::bounty::credit_kill(reg, character, g);
            }
            let ev = crate::kill_hooks::KillEvent {
                victim_character: 0,
                victim_ship: 0,
                ship_type: type_id,
                killer: Some(character),
                system,
                npc: Some(kind),
                when: at,
                park: Vec::new(),
            };
            crate::sov_index::on_npc_kill(reg, &ev);
            // A module's drop tables roll for the settled kill too, and pay
            // the hangar (no wreck exists).
            crate::settle_hooks::fire(
                reg,
                &crate::settle_hooks::Settled {
                    ev,
                    station: a.station,
                    seed: npc_seed(character, a.runs, k, m),
                },
            );
        }
    }
    crate::npc::bounty::flush(reg, crate::pyutil::filetime_now());
    for (t, n) in loot {
        if let Err(why) = shop::add(reg, character, a.station, t, n) {
            notes.push(format!("{n} of loot {t} lost: {why}"));
        }
    }
    consume(reg, &spare, &guns.loaded, fight.rounds, notes);
}

/// Fire `rounds`: the spares first (hangar then cargo, ascending), then the
/// loaded stacks.
pub(super) fn consume(
    reg: &mut Registry,
    spare: &[(i64, i64)],
    loaded: &[(i64, i64)],
    rounds: i64,
    notes: &mut Vec<String>,
) {
    let mut left = rounds;
    for &(item, qty) in spare.iter().chain(loaded.iter()) {
        if left < 1 {
            break;
        }
        let n = left.min(qty);
        match shop::take(reg, item, n) {
            Ok(()) => left -= n,
            Err(why) => notes.push(format!("rounds not consumed: {why}")),
        }
    }
}

/// Sell the loot: modules, charges and tags a bid takes, never the
/// ammunition or a gun type fitted.
fn sell_loot(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    guns: Option<&Guns>,
    notes: &mut Vec<String>,
) {
    let hull = reg.character(character).map_or(0, |c| c.ship_type_id);
    let (ammo, fitted) = guns.map_or((0, BTreeSet::new()), |g| (g.ammo, g.fitted.clone()));
    shop::sell_where(
        reg,
        character,
        a.station,
        |i: &Item| {
            is_loot(i) && i.type_id != ammo && i.type_id != hull && !fitted.contains(&i.type_id)
        },
        notes,
    );
}

/// **Loot**, as the live ratter sees its hangar (`escort::is_loot`): a
/// packaged module or charge, or a tag.
pub(super) fn is_loot(i: &Item) -> bool {
    !i.singleton
        && (crate::econ_exchange::LOOT_CATEGORIES.contains(&i.category_id)
            || crate::econ_exchange::TAG_GROUPS.contains(&i.group_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loot_seed_is_deterministic_distinct_and_positive() {
        let mut seen = BTreeSet::new();
        for ch in [90_000_001, 90_000_002] {
            for run in 0..3 {
                for k in 0..5 {
                    for m in 0..4 {
                        let s = npc_seed(ch, run, k, m);
                        assert!(s > 0);
                        assert_eq!(s, npc_seed(ch, run, k, m));
                        assert!(seen.insert(s), "{ch} {run} {k} {m} collides");
                    }
                }
            }
        }
    }
}
