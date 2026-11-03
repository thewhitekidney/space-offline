//! **Smartbombs** (lane BUBBLES-SB, issue #24; effect 38 `empWave`, range
//! attribute 99 `empFieldRange`, damage in 114/116/117/118 on the module
//! **[V-data]**).
//!
//! A smartbomb has no target. Every cycle it damages each ball in the solar
//! system whose centre is within [`ATTR_RANGE`] of the bomber's, the bomber's
//! own ship excepted. Every ball is a roster entry, so a victim may be a
//! player, an NPC, a launched drone (the owner's decision, #127), a fighter
//! squadron [A: untested] or an abandoned hull (a boarded hull's owner is its
//! victim, #78); each one is a volley of its own through [`crate::combat::fire`],
//! which is aggression against that victim, logs both combat lines and kills
//! when it must. The cycle's first volley lands at activation, as a gun's
//! does.
//!
//! # Assumed, with a falsifier each
//!
//! - **[A]** the range is from the bomber's centre to the victim's centre.
//!   Falsifier: a TQ ship whose hull edge is inside the field is hit while
//!   its centre is outside.
//! - **[A]** every victim takes the full volley, unreduced by signature or
//!   speed (a smartbomb has no application roll), then its own resistances
//!   in [`crate::combat::fire`]. Falsifier: a TQ smartbomb that does less to a
//!   frigate than to a battleship.
//! - **[A]** `crimewatch::attack_refusal` is asked once per victim, and a
//!   victim it refuses is spared (no hit, no crime): a smartbomb in a system
//!   where a pilot may not shoot a given neighbour leaves that neighbour
//!   alone, instead of making each one a separate crime. Falsifier: TQ
//!   flags the bomber criminal for the neighbour it spared.
//! - **[A]** a ship in warp, a ship that is the bomber's own, and a ship in
//!   another system are never hit. Falsifier: a TQ ship in warp takes damage.
//! - **[A]** a smartbomb hits corp and fleet mates as it hits anyone.
//!   Falsifier: TQ spares fleet mates.
//! - **[A]** the bomber's own drones are hit too, and a hit drone is aggression
//!   against its owner as a hit on the owner's ship is (the owner's own drone
//!   is none, `crimewatch::aggression`). No test asserts either. Falsifier: TQ
//!   smartbombs spare the bomber's own drones, or a hit drone flags nobody.

use crate::damage::{Damage, Shot, attr};
use crate::modules::Cycle;
use crate::service::Registry;

/// `empWave` **[V-data]**.
pub const EFFECT: i64 = 38;
/// `empFieldRange` **[V-data]**: the field's reach, metres.
pub const ATTR_RANGE: i64 = 99;

/// Whether `effect_id` is the smartbomb's.
pub fn is_smartbomb(effect_id: i64) -> bool {
    effect_id == EFFECT
}

/// A module attribute: the dogma sheet's value (skills, rigs, overheat),
/// else the type's own.
fn value(w: &Registry, c: &Cycle, attribute_id: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, c.ship_id, c.module_id, attribute_id)
        .or_else(|| crate::typedata::attribute_in(w.content(), c.type_id, attribute_id))
}

/// The module's volley: the four damage attributes as it has them.
pub fn volley(w: &Registry, c: &Cycle) -> Damage {
    let d = |a: i64| value(w, c, a).unwrap_or(0.0).max(0.0);
    Damage {
        em: d(attr::EM_DAMAGE),
        explosive: d(attr::EXPLOSIVE_DAMAGE),
        kinetic: d(attr::KINETIC_DAMAGE),
        thermal: d(attr::THERMAL_DAMAGE),
    }
}

/// **Who this cycle hits**: every presence in the bomber's system but his own
/// ship, not in warp, within range (module header **[A]**s).
pub fn victims(w: &Registry, c: &Cycle) -> Vec<crate::space_state::Presence> {
    let Some(here) = w.roster().get(c.character_id).copied() else {
        return Vec::new();
    };
    if here.ship_id != c.ship_id || here.motion.warp_active() {
        return Vec::new();
    }
    let Some(range) = value(w, c, ATTR_RANGE).filter(|r| *r > 0.0) else {
        return Vec::new();
    };
    w.roster()
        .in_system(here.solar_system_id)
        .into_iter()
        .filter(|p| p.ship_id != c.ship_id && !p.motion.warp_active())
        .filter(|p| !crate::cloak::is_cloaked(w, p.ship_id))
        .filter(|p| crate::space_state::distance(here.motion.position, p.motion.position) <= range)
        .collect()
}

/// **One cycle of a smartbomb**: a volley at each victim the gate lets
/// through. Returns how many were hit.
pub fn on_cycle(w: &mut Registry, c: &Cycle, now: crate::damage::BlueTime) -> usize {
    let damage = volley(w, c);
    if damage.total() <= 0.0 {
        return 0;
    }
    let mut hit = 0;
    for victim in victims(w, c) {
        if crate::crimewatch::attack_refusal(
            w,
            c.character_id,
            Some(c.type_id),
            victim.ship_id,
            now,
        )
        .is_some()
        {
            continue;
        }
        let shot = Shot {
            at: c.at,
            attacker_ship_id: c.ship_id,
            weapon_item_id: c.module_id,
            weapon_type_id: c.type_id,
            target_ship_id: victim.ship_id,
            damage,
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(w, shot, c.character_id, victim, now);
        hit += 1;
    }
    hit + units_hit(w, c, damage.total(), now)
}

/// The anchorable units (NW-2) within the field take the raw points **[A]**
/// (no resists, no aggression). Falsifier: TQ smartbombs spare deployables.
fn units_hit(w: &mut Registry, c: &Cycle, points: f64, now: crate::damage::BlueTime) -> usize {
    let Some(here) = w.roster().get(c.character_id).copied() else {
        return 0;
    };
    if here.ship_id != c.ship_id || here.motion.warp_active() {
        return 0;
    }
    let Some(range) = value(w, c, ATTR_RANGE).filter(|r| *r > 0.0) else {
        return 0;
    };
    let ids: Vec<i64> = w
        .combat()
        .wrecks()
        .filter(|wr| wr.solar_system_id == here.solar_system_id)
        .filter(|wr| crate::space_state::distance(here.motion.position, wr.position) <= range)
        .map(|wr| wr.wreck_id)
        .filter(|id| crate::anchor::is_unit(w, *id))
        .collect();
    let mut n = 0;
    for id in ids {
        if crate::anchor::hit(w, id, points, now).is_some() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests;
