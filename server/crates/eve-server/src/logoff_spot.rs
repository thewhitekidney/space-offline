//! **Where a ship was when its log-off hold ended** (lane `login-space`,
//! spec R-P1-SPECS SPEC 2).
//!
//! A pilot who logs off in space leaves the ship there for the hold
//! (`crate::crimewatch::logged_off`). When the hold ends the ship leaves the
//! roster (`crimewatch::release_holds`), and before this lane nothing
//! remembered where it had been: the next login put the pilot in their home
//! station, a free teleport even across systems. Now the release writes the
//! spot down, and the login (`Registry::character_change`) puts the pilot
//! back in space there, the ballpark bind (`destiny::on_ballpark_bind`)
//! seats the ship at it, and `invuln::grant_restoring` protects it.
//!
//! The spot is a delta row, so it survives a restart without `store.rs`
//! knowing about it: `char -> ship system x y z` (kind [`KIND`]).
//!
//! `[A]` The restore leaves the character row alone (the select screen still
//! shows home); falsified by a character screen that must show the spot.

use crate::service::Registry;

/// The delta kind (`delta_kinds::KINDS`): `char -> ship system x y z`.
pub const KIND: &str = "logoffspot";

/// A ship's resting place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub ship_id: i64,
    pub system: i64,
    pub position: [f64; 3],
}

fn key(c: i64) -> Vec<String> {
    vec![c.to_string()]
}

fn parse(row: &[String]) -> Option<Spot> {
    let f = |i: usize| row.get(i)?.parse::<f64>().ok();
    let n = |i: usize| row.get(i)?.parse::<i64>().ok();
    Some(Spot {
        ship_id: n(0)?,
        system: n(1)?,
        position: [f(2)?, f(3)?, f(4)?],
    })
}

/// The spot `c` would be restored to: `None` unless the row names the ship
/// the character has now and a system the world knows (a killed ship's row,
/// a new hull, or a removed system is inert).
pub fn get(w: &Registry, c: i64) -> Option<Spot> {
    let spot = parse(w.deltas().of(KIND).get(&key(c))?)?;
    (w.character(c)?.ship_id == spot.ship_id
        && crate::jump::system_row(w, c, spot.system).is_some())
    .then_some(spot)
}

/// Write down where `c`'s ship is now. **No roster entry means clear**: a
/// held ship killed in the hold has no place to come back to.
pub fn record(w: &mut Registry, c: i64) {
    // ABYSS-1: a pilot in an abyssal run keeps the spot they came from; the
    // pocket does not outlive the server.
    if crate::abyss::in_run(w, c) {
        return;
    }
    w.advance_character(c);
    let Some(p) = w.roster().get(c).copied() else {
        clear(w, c);
        return;
    };
    let [x, y, z] = p.motion.position;
    w.deltas_mut().put(
        KIND,
        key(c),
        vec![
            p.ship_id.to_string(),
            p.solar_system_id.to_string(),
            x.to_string(),
            y.to_string(),
            z.to_string(),
        ],
    );
}

/// Write `c`'s spot as given (the Abyss writes the origin of a pilot who goes
/// in, so a restart or a log-off puts the ship back where it came from).
pub fn set(w: &mut Registry, c: i64, spot: Spot) {
    let [x, y, z] = spot.position;
    w.deltas_mut().put(
        KIND,
        key(c),
        vec![
            spot.ship_id.to_string(),
            spot.system.to_string(),
            x.to_string(),
            y.to_string(),
            z.to_string(),
        ],
    );
}

/// Forget `c`'s spot.
pub fn clear(w: &mut Registry, c: i64) {
    if w.deltas().of(KIND).contains_key(&key(c)) {
        w.deltas_mut().del(KIND, &key(c));
    }
}

/// The bind's read: the spot, removed. `None` (and nothing removed) while the
/// ship is in the roster, where the rejoin path owns it, **and while the
/// character is in a jump**: the arrival the jump armed (an abyssal room, or
/// the way home out of one) owns the bind, and the row, which holds the
/// origin, would put the ship back where it started. A jump dropped on a
/// disconnect (`jump::tick`) is gone by the next login, and the row restores.
pub fn take(w: &mut Registry, c: i64) -> Option<Spot> {
    if w.roster().is_in_space(c) || w.jumps().contains(c) {
        return None;
    }
    let spot = get(w, c);
    clear(w, c);
    spot
}

/// After a world load. The rows are the state (read on demand by [`get`]), so
/// there is nothing to rebuild; a row whose ship or system is gone is inert
/// and is overwritten or cleared by the next write.
pub fn restore(_w: &mut Registry) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_state::{Motion, Presence};

    // Invented ids.
    const ALICE: i64 = 1_000_001;
    const SHIP: i64 = 10_001;
    const HOME: i64 = 30_001;

    fn world() -> Registry {
        Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: SHIP,
                solar_system_id: Some(HOME),
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        })
    }

    fn park(reg: &mut Registry, at: [f64; 3]) {
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            SHIP,
            HOME,
            Motion::arrived(at, [0.0; 3]),
        ));
    }

    #[test]
    fn record_get_and_take_round_trip() {
        let mut reg = world();
        park(&mut reg, [1.5, -2.25, 3.0]);
        record(&mut reg, ALICE);
        reg.roster_mut().leave(ALICE);
        let spot = get(&reg, ALICE).expect("recorded");
        assert_eq!(spot.system, HOME);
        assert_eq!(spot.position, [1.5, -2.25, 3.0]);
        assert_eq!(take(&mut reg, ALICE), Some(spot));
        assert_eq!(take(&mut reg, ALICE), None, "taken once");
    }

    #[test]
    fn no_roster_entry_clears() {
        let mut reg = world();
        park(&mut reg, [1.0, 2.0, 3.0]);
        record(&mut reg, ALICE);
        reg.roster_mut().leave(ALICE);
        record(&mut reg, ALICE);
        assert!(get(&reg, ALICE).is_none());
        assert!(reg.deltas().of(KIND).is_empty());
    }

    #[test]
    fn another_ship_makes_the_row_inert() {
        let mut reg = world();
        park(&mut reg, [1.0, 2.0, 3.0]);
        record(&mut reg, ALICE);
        reg.roster_mut().leave(ALICE);
        reg.character_mut(ALICE).unwrap().ship_id = SHIP + 1;
        assert!(get(&reg, ALICE).is_none());
    }

    #[test]
    fn take_leaves_a_ship_in_space_alone() {
        let mut reg = world();
        park(&mut reg, [1.0, 2.0, 3.0]);
        record(&mut reg, ALICE);
        assert!(take(&mut reg, ALICE).is_none());
        assert!(!reg.deltas().of(KIND).is_empty());
    }
}
