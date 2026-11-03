//! **The gatherers' settlement**: `miner`, `ice_miner` and `gas_huffer`
//! (R-ECON §1.5: "a miner's ore into its hangar and sold into bids at its
//! station").
//!
//! 1. **The rig** ([`rig_of`]): the fitted mining modules of the active hull
//!    -- lasers for ore and ice (`modules::EFFECT_MINING_LASER`), gas
//!    harvesters for clouds (`mining::EFFECT_MINING_CLOUDS`) -- their
//!    `miningAmount` and cycle from the type table, and the hold of the hull
//!    (its mining hold and its cargo). A hull with none mines nothing: an
//!    agent cannot do what its pilot could not.
//! 2. **The cut** ([`rules::mined_m3`], [`rules::cut_plan`]): the m3 the rig
//!    mines in the work hours, spent on **the home system's real rocks**.
//!    A laid-out system is bitten through `mining::settle_bite` (the pilots
//!    there see the rock shrink or go); a cold one through
//!    `asteroid::settle_bite_cold`, the `rock` delta the next layout reads.
//!    The ore **goes to the hangar** (the live bot unloads there). It never
//!    exists unless a rock gave it up, and the rocks are reset in full by the
//!    rollover (the daily full reset), so the budget is the day's field.
//!    Each agent starts at its own belt ([`belt_first`]).
//! 3. **The refinery** ([`shop::refine`]): ore and ice in the hangar go
//!    through the station's real plan, tax and all, the way
//!    `builder::sell_goods` begins.
//! 4. **The sale** ([`shop::sell_where`]): raw materials, ore and gas that
//!    some bid takes at the floor are sold into it, the way
//!    `builder::sell_goods` ends. A mineral nobody bids for stays in the
//!    hangar for the next settlement, or for the pilot.

use super::{Agent, Stuff, rules, shop};
use crate::asteroid::Rock;
use crate::damage::BlueTime;
use crate::item::Item;
use crate::service::Registry;

/// `groupIce` **[V-data]**: an ice type in a rock or a hold.
pub const GROUP_ICE: i64 = 465;

/// `categoryAsteroid` **[V]**: ore and ice.
const CATEGORY_ASTEROID: i64 = 25;

/// The effect that cuts what a gatherer gathers.
fn effect_of(stuff: Stuff) -> i64 {
    match stuff {
        Stuff::Gas => crate::mining::EFFECT_MINING_CLOUDS,
        Stuff::Ore | Stuff::Ice => crate::modules::EFFECT_MINING_LASER,
    }
}

/// Whether a rock's type is the stuff `stuff` gathers.
pub(super) fn is_stuff(reg: &Registry, stuff: Stuff, type_id: i64) -> bool {
    let Some(t) = crate::item::find_in(reg.content(), type_id) else {
        return false;
    };
    let gas = crate::mining::is_gas(reg.content(), type_id);
    match stuff {
        Stuff::Gas => gas,
        Stuff::Ice => !gas && t.group_id == GROUP_ICE,
        Stuff::Ore => !gas && t.group_id != GROUP_ICE && t.category_id == CATEGORY_ASTEROID,
    }
}

/// **What the active hull mines with**, `None` when it has nothing for
/// `stuff`.
pub(super) fn rig_of(reg: &Registry, character: i64, stuff: Stuff) -> Option<rules::MiningRig> {
    let c = reg.character(character)?;
    let want = effect_of(stuff);
    let mut lasers = 0u32;
    let (mut amount, mut cycle_s) = (0.0, 0.0);
    for m in crate::modules::slim_modules(reg.store(), c.ship_id) {
        let Some(t) = crate::modules::find_in(reg.content(), m.type_id) else {
            continue;
        };
        if t.effect.map(|e| e.effect_id) != Some(want) || t.mining_amount() <= 0.0 {
            continue;
        }
        let ms = t.duration_ms();
        if ms <= 0 {
            continue;
        }
        if lasers == 0 {
            amount = t.mining_amount();
            cycle_s = ms as f64 / 1000.0;
        }
        lasers += 1;
    }
    if lasers == 0 {
        return None;
    }
    let cargo = crate::item::find_in(reg.content(), c.ship_type_id).map_or(0.0, |t| t.capacity);
    let hold = cargo + crate::mining_hold::capacity(reg.content(), c.ship_type_id);
    Some(rules::MiningRig {
        lasers,
        amount_m3: amount,
        cycle_s,
        hold_m3: hold,
    })
}

/// The home system's rocks, and whether the system is laid out. A system
/// nobody is in is not: its rocks are the generated ones with their deltas
/// applied ([`crate::asteroid::Field::plan`]).
fn rocks_in(reg: &Registry, system: i64) -> (Vec<Rock>, bool) {
    if reg.rocks().is_materialised(system) {
        (reg.rocks().in_system(system).copied().collect(), true)
    } else {
        (reg.rocks().plan(system, reg.deltas()).rocks, false)
    }
}

/// Settle one gatherer for `work_h` hours.
pub(super) fn settle(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    stuff: Stuff,
    work_h: f64,
    _now: BlueTime,
    notes: &mut Vec<String>,
) {
    if work_h > 0.0 {
        cut(reg, character, a, stuff, work_h, notes);
    }
    // The hangar is worked even with no hours left: yesterday's ore still
    // refines, and a bid that appeared since still fills.
    if stuff != Stuff::Gas {
        shop::refine(
            reg,
            character,
            a.station,
            |i: &Item| i.category_id == CATEGORY_ASTEROID,
            notes,
        );
    }
    let gas: Vec<i64> = shop::stacks(reg, character, a.station)
        .iter()
        .filter(|i| crate::mining::is_gas(reg.content(), i.type_id))
        .map(|i| i.type_id)
        .collect();
    shop::sell_where(
        reg,
        character,
        a.station,
        |i: &Item| {
            crate::econ_exchange::FLOOR_GROUPS.contains(&i.group_id)
                || i.category_id == CATEGORY_ASTEROID
                || gas.contains(&i.type_id)
        },
        notes,
    );
}

/// **Which belt a miner reaches first**: the rocks grouped by belt (belt id
/// order, rock id order inside), the groups rotated to start at belt
/// `mix(character) % n_belts`, so agents of one system spread over its belts
/// instead of all draining the first.
pub(super) fn belt_first(rocks: &mut [Rock], character: i64) {
    rocks.sort_by_key(|r| (r.belt_id, r.id));
    let mut belts: Vec<i64> = rocks.iter().map(|r| r.belt_id).collect();
    belts.dedup();
    if belts.len() < 2 {
        return;
    }
    let start =
        belts[(crate::seed::mix(0x005E_771E, &[character as u64]) % belts.len() as u64) as usize];
    let at = rocks.iter().position(|r| r.belt_id == start).unwrap_or(0);
    rocks.rotate_left(at);
}

/// The mining half: rig, m3, rocks, ore into the hangar.
fn cut(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    stuff: Stuff,
    work_h: f64,
    notes: &mut Vec<String>,
) {
    let Some(rig) = rig_of(reg, character, stuff) else {
        notes.push(format!(
            "no {} modules on the active hull",
            effect_name(stuff)
        ));
        return;
    };
    let m3 = rules::mined_m3(&rig, work_h);
    let Some(system) = reg.character(character).and_then(|c| c.solar_system_id) else {
        notes.push("no home system".to_owned());
        return;
    };
    let (rocks, laid_out) = rocks_in(reg, system);
    let mut found: Vec<Rock> = rocks
        .iter()
        .filter(|r| is_stuff(reg, stuff, r.type_id))
        .copied()
        .collect();
    belt_first(&mut found, character);
    let mut views = Vec::new();
    for r in &found {
        let Some(t) = crate::item::find_in(reg.content(), r.type_id) else {
            continue;
        };
        views.push(rules::RockView {
            id: r.id,
            quantity: r.quantity,
            unit_m3: t.volume,
        });
    }
    if views.is_empty() {
        notes.push(format!(
            "no {} in the home system's rocks",
            effect_name(stuff)
        ));
        return;
    }
    let at = crate::pyutil::filetime_now();
    let mut gained: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for (id, units) in rules::cut_plan(&views, m3) {
        let Some(rock) = found.iter().find(|r| r.id == id).copied() else {
            continue;
        };
        let volume = crate::item::find_in(reg.content(), rock.type_id).map_or(0.0, |t| t.volume);
        let taken = if laid_out {
            crate::mining::settle_bite(reg, &rock, units, volume, at)
        } else {
            let taken = crate::asteroid::settle_bite_cold(reg, &rock, units);
            crate::sov_index::on_mined(reg, system, taken as f64 * volume);
            taken
        };
        if taken > 0 {
            *gained.entry(rock.type_id).or_default() += taken;
        }
    }
    for (type_id, units) in gained {
        if let Err(why) = shop::add(reg, character, a.station, type_id, units) {
            notes.push(format!("{units} of {type_id} lost: {why}"));
        }
    }
}

fn effect_name(stuff: Stuff) -> &'static str {
    match stuff {
        Stuff::Ore => "mining laser",
        Stuff::Ice => "ice harvester",
        Stuff::Gas => "gas harvester",
    }
}
