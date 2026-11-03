//! Tests of `crate::pilotless`: the flag, the rows, the restart.

use super::*;
use crate::combat::tests::{ALICE, ALICE_SHIP, SYSTEM, alice_has_bob_locked};

fn abandoned(ship: i64) -> Abandoned {
    Abandoned {
        ship_id: ship,
        type_id: 587,
        owner_id: ALICE,
        corp_id: 98_000_001,
        alliance_id: Some(99_000_001),
        system: SYSTEM,
        abandoned_at: 5,
    }
}

/// The interactive bit is 0x08 and a pilotless ball is 0x13 [V]: the client's
/// own dump of a ball with `isInteractive` cleared is `0x1f` -> `0x17`.
#[test]
fn the_interactive_bit_is_0x08() {
    use crate::destiny::Park;
    const BASE: &[u8] = include_bytes!("../../../../../findings/destiny-ref/03-base-one-ball.bin");
    const ATTR: &[u8] =
        include_bytes!("../../../../../findings/destiny-ref/30-attr-isInteractive.bin");
    let flags = |bytes: &[u8]| Park::decode(bytes).expect("the client wrote it").balls[0].flags;
    assert_eq!(flags(BASE), crate::destiny::FLAGS_FREE_BALL);
    assert_eq!(
        flags(ATTR),
        crate::destiny::FLAGS_FREE_BALL & !FLAG_IS_INTERACTIVE
    );
    assert_eq!(FLAGS_PILOTLESS, 0x13);
    assert_eq!(FLAGS_PILOTLESS & FLAG_IS_INTERACTIVE, 0);
}

/// A restart brings an abandoned ship back where it was, from its row and
/// the store; a row whose hull is gone, or moved, or is junk, is dropped.
#[test]
fn a_restart_brings_the_ship_back_and_drops_dead_rows() {
    let mut reg = alice_has_bob_locked();
    // Alice's hull goes to (system, 0), as an eject leaves it.
    reg.apply_items(&[crate::item::Edit::Move {
        item_id: ALICE_SHIP,
        to: crate::item::Slot::new(ALICE, SYSTEM, crate::combat::FLAG_IN_SPACE),
    }])
    .expect("moved");
    reg.roster_mut().leave(ALICE);
    let at = [1500.0, -20.0, 3.5];
    abandon(
        &mut reg,
        abandoned(ALICE_SHIP),
        Motion::arrived(at, [0.0; 3]),
    );
    // Rows that must not come back.
    reg.deltas_mut().put(KIND, key(777_001), vec!["1".into()]);
    reg.deltas_mut().put(
        KIND,
        key(777_002),
        ["30000001", "0", "0", "0", "5", "1", "-"]
            .map(String::from)
            .to_vec(),
    );
    reg.deltas_mut().put(KIND, vec!["not-an-id".into()], vec![]);

    // "Restart": the in-memory half is gone, the row and the store are not.
    state_mut(&mut reg).ships.clear();
    state_mut(&mut reg).restored = false;
    reg.roster_mut().leave(ALICE_SHIP);
    assert!(!is_pilotless(&reg, ALICE_SHIP));

    restore(&mut reg);
    let a = get(&reg, ALICE_SHIP).expect("back");
    assert_eq!(
        (
            a.owner_id,
            a.type_id,
            a.corp_id,
            a.alliance_id,
            a.system,
            a.abandoned_at
        ),
        (ALICE, 587, 98_000_001, Some(99_000_001), SYSTEM, 5)
    );
    let p = reg.roster().get(ALICE_SHIP).expect("in the roster");
    assert_eq!((p.ship_id, p.solar_system_id), (ALICE_SHIP, SYSTEM));
    assert_eq!(p.motion.position, at);
    let rows: Vec<_> = reg.deltas().of(KIND).keys().cloned().collect();
    assert_eq!(rows, vec![key(ALICE_SHIP)], "the dead rows are dropped");
    // And once per process.
    release(&mut reg, ALICE_SHIP);
    restore(&mut reg);
    assert!(!is_pilotless(&reg, ALICE_SHIP));
}

/// A hull that is no longer in space where the row says (it was docked and
/// the row lingered, say) is not brought back.
#[test]
fn a_hull_that_left_the_system_is_not_restored() {
    let mut reg = alice_has_bob_locked();
    abandon(
        &mut reg,
        abandoned(ALICE_SHIP),
        Motion::arrived([0.0; 3], [0.0; 3]),
    );
    // The hull is still in its hangar, not at (system, 0).
    state_mut(&mut reg).ships.clear();
    state_mut(&mut reg).restored = false;
    reg.roster_mut().leave(ALICE_SHIP);
    restore(&mut reg);
    assert!(!is_pilotless(&reg, ALICE_SHIP));
    assert!(reg.deltas().of(KIND).is_empty());
}

/// The release takes the record, the row and the roster entry; a pilot's
/// roster entry is never touched.
#[test]
fn release_removes_the_record_the_row_and_the_roster_entry() {
    let mut reg = alice_has_bob_locked();
    abandon(
        &mut reg,
        abandoned(ALICE_SHIP),
        Motion::arrived([0.0; 3], [0.0; 3]),
    );
    assert!(reg.roster().get(ALICE_SHIP).is_some());
    assert!(release(&mut reg, ALICE_SHIP).is_some());
    assert!(reg.roster().get(ALICE_SHIP).is_none());
    assert!(reg.deltas().of(KIND).is_empty());
    assert!(reg.roster().get(ALICE).is_some(), "the pilot stays");
    assert!(release(&mut reg, ALICE_SHIP).is_none());
}
