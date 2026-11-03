//! **BE-3 over a real socket**: the `eject` and `board_space` verbs, the
//! client's own `ship.Eject()` and `ship.Board(shipID, oldShipID)` on the
//! solar system's `ship` moniker.
//!
//! Alice undocks in a Rifter-typed hull beside Bob (also in a hull) and
//! ejects. Bob's park sees the hull's slim lose its `charID`; Bob boards it
//! (`OfferBoardShip`'s gates are the verb's own), and its ownership passes to
//! him. Same limitation as every test here: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.

use super::common::{SYSTEM, config, gm, login, pilot, serve};
use eve_bot::Conn;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_951;
const BOB: i64 = 90_000_952;
/// The fixture's Rifter [V-data], spawnable by `/createitem`.
const RIFTER: i64 = 587;

/// Read `c`'s pushes until `done` says so or `within` runs out.
fn watch(c: &mut Conn, within: Duration, mut done: impl FnMut(&Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done(c) {
            return true;
        }
        let _ = c.pump(Duration::from_millis(250));
    }
    done(c)
}

/// Board a fresh Rifter in the hangar and return its id.
fn fly(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {RIFTER}"));
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .iter()
        .find(|r| r.type_id == RIFTER)
        .expect("the hull")
        .item_id;
    c.board(hull).expect("boards");
    hull
}

#[test]
fn a_pilot_ejects_and_another_boards_the_abandoned_hull() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_951),
        pilot(BOB, "Bob", 100_000_952),
    ];
    let addr = serve(config);

    let mut alice = login(addr, "Alice");
    let hull = fly(&mut alice);
    let mut bob = login(addr, "Bob");
    let bobs_hull = fly(&mut bob);
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");
    assert!(
        watch(&mut bob, Duration::from_secs(30), |c| c
            .world()
            .get(hull)
            .is_some()),
        "Alice's hull reaches Bob's park"
    );

    // A piloted ship is not offered: nothing is sent.
    assert!(
        matches!(bob.board_space(hull), Err(eve_bot::Error::NotSent(_))),
        "a ship with a pilot is not boardable"
    );

    let left = alice.eject().expect("Alice ejects");
    assert_eq!(left, hull, "the verb names the hull she left");
    assert_ne!(
        alice.session().ship_id(),
        Some(hull),
        "she is in the capsule"
    );
    assert!(
        watch(&mut bob, Duration::from_secs(30), |c| c
            .world()
            .get(hull)
            .and_then(|b| b.slim)
            .is_some_and(|s| s.char_id.is_none())),
        "the hull's slim lost its pilot in Bob's park"
    );

    // A capsule cannot eject.
    assert!(
        matches!(alice.eject(), Err(eve_bot::Error::Refused(_))),
        "the server refuses a capsule"
    );

    bob.board_space(hull)
        .expect("Bob boards the abandoned hull");
    assert_eq!(bob.session().ship_id(), Some(hull));
    let _ = bobs_hull;
}
