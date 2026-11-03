//! **Kill marks** (lane `gap-smalls`, EVEJS-GAP svcD §3.6): the stickers on a
//! hull for every pilot it has killed.
//!
//! The client asks `shipKillCounter.GetItemKillCountPlayer(itemID)` for the
//! hangar and Show Info, and paints `min(n, 999)` marks (`baseHangarBehaviours`
//! and `panelOverview`); in space the ball reads the slim item's `kills`
//! (`playerShip`) [V].
//!
//! A count belongs to the **ship item** that fired the killing volley, in the
//! delta kind [`KIND`] (`ship_id -> n`). [`on_kill`] is a
//! [`crate::kill_hooks::PLAYER`] row: it adds one when a character's **ship**
//! died to another character (a drone's owner counts). A capsule [A: a pod
//! kill earns no mark; falsifier: a TQ capture of a ship's `kills` rising on
//! a pod], an NPC, a self-kill and a drone's death add none. A dead ship's
//! row is dropped.

use crate::kill_hooks::KillEvent;
use crate::service::Registry;
use eve_marshal::PyValue;

/// The delta kind: `ship_id -> n`.
pub const KIND: &str = "killmark";

/// The marks on a ship.
pub fn count(w: &Registry, ship_id: i64) -> i64 {
    w.deltas()
        .of(KIND)
        .get(&vec![ship_id.to_string()])
        .and_then(|v| v.first())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// The character that earned a kill by roster key `killer`: the pilot, or a
/// drone's owner. `None` for an NPC.
fn killing_character(w: &Registry, killer: i64) -> Option<i64> {
    w.pilot(killer)?.char_id
}

/// **The `kill_hooks::PLAYER` row.**
pub fn on_kill(w: &mut Registry, ev: &KillEvent) {
    // The dead ship's marks go with it.
    w.deltas_mut().del(KIND, &[ev.victim_ship.to_string()]);
    if ev.ship_type == crate::damage::CAPSULE_TYPE_ID || w.character(ev.victim_character).is_none()
    {
        return;
    }
    let Some(killer) = ev.killer.and_then(|k| killing_character(w, k)) else {
        return;
    };
    if killer == ev.victim_character {
        return;
    }
    let Some(ship) = w.roster().get(killer).map(|p| p.ship_id) else {
        return;
    };
    let n = count(w, ship) + 1;
    w.deltas_mut()
        .put(KIND, vec![ship.to_string()], vec![n.to_string()]);
    crate::self_destruct::refresh_slim(w, killer);
}

/// **The `shipKillCounter.GetItemKillCountPlayer` answer.**
pub fn count_call(w: &Registry, args: &[PyValue]) -> PyValue {
    let ship = match args.first() {
        Some(PyValue::Int(i)) => *i,
        _ => return PyValue::Int(0),
    };
    PyValue::Int(count(w, ship))
}

/// **The `slim_extra::SOURCES` row**: `kills` on a ship with marks.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    ids.iter()
        .filter_map(|id| {
            let n = count(w, *id);
            (n > 0).then(|| (*id, vec![("kills", PyValue::Int(n))]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::tests::{ALICE, ALICE_POD, ALICE_SHIP, BOB, BOB_SHIP, alice_has_bob_locked};

    fn event(victim: i64, ship: i64, ship_type: i64, killer: Option<i64>) -> KillEvent {
        KillEvent {
            victim_character: victim,
            victim_ship: ship,
            ship_type,
            killer,
            system: 0,
            npc: None,
            when: 0,
            park: Vec::new(),
        }
    }

    #[test]
    fn a_player_kill_marks_the_killers_ship_and_only_that() {
        let mut reg = alice_has_bob_locked();
        on_kill(&mut reg, &event(BOB, BOB_SHIP, 587, Some(ALICE)));
        assert_eq!(count(&reg, ALICE_SHIP), 1);
        on_kill(&mut reg, &event(BOB, BOB_SHIP, 587, Some(ALICE)));
        assert_eq!(count(&reg, ALICE_SHIP), 2);
        assert_eq!(count(&reg, BOB_SHIP), 0);
        assert_eq!(
            count_call(&reg, &[PyValue::Int(ALICE_SHIP)]),
            PyValue::Int(2)
        );
        let extras = slim_extras(&reg, &[ALICE_SHIP, BOB_SHIP]);
        assert_eq!(extras, vec![(ALICE_SHIP, vec![("kills", PyValue::Int(2))])]);
    }

    #[test]
    fn a_capsule_an_npc_and_a_self_kill_add_none() {
        let mut reg = alice_has_bob_locked();
        let pod = crate::damage::CAPSULE_TYPE_ID;
        on_kill(&mut reg, &event(BOB, ALICE_POD, pod, Some(ALICE)));
        on_kill(&mut reg, &event(BOB, BOB_SHIP, 587, Some(7_777)));
        on_kill(&mut reg, &event(BOB, BOB_SHIP, 587, None));
        on_kill(&mut reg, &event(ALICE, ALICE_SHIP, 587, Some(ALICE)));
        assert_eq!(count(&reg, ALICE_SHIP), 0);
        assert!(slim_extras(&reg, &[ALICE_SHIP, BOB_SHIP]).is_empty());
    }

    #[test]
    fn a_dead_ships_marks_are_dropped() {
        let mut reg = alice_has_bob_locked();
        on_kill(&mut reg, &event(BOB, BOB_SHIP, 587, Some(ALICE)));
        on_kill(&mut reg, &event(ALICE, ALICE_SHIP, 587, Some(BOB)));
        assert_eq!(count(&reg, ALICE_SHIP), 0, "dropped with the ship");
        assert_eq!(count(&reg, BOB_SHIP), 1, "Bob's ship earned its own");
    }
}
