//! CONCORD-1a over a real socket: the criminal in high sec is destroyed.
//!
//! The scenario is CW-1's (`crimewatch::fight`): Alice, at red safety, shoots
//! Bob's ship in an invented 0.9 system. This test then watches what CONCORD
//! does about it: the offender is warned, the squad arrives on grid for the
//! other pilots, and Alice's ship is destroyed.
//!
//! Invented rows only: the world has no CONCORD types, so the server's
//! no-bundle fixtures stand in (`concord::FIXTURES`). The same limitation as
//! every test here: both ends marshal with `eve-marshal`.

use super::crimewatch::fight;
use eve_marshal::PyValue;
use std::time::Duration;

use super::common::until;

/// Does `v` carry the byte string `want` anywhere inside?
fn mentions(v: &PyValue, want: &[u8]) -> bool {
    match v {
        PyValue::Bytes(b) => b.as_slice() == want,
        PyValue::List(items) | PyValue::Tuple(items) => items.iter().any(|i| mentions(i, want)),
        _ => false,
    }
}

/// Alice's capsule: the sandbox's `pilot(ALICE, "Alice", 100_000_201)` gives
/// her pod the id she was created with.
const ALICE_CAPSULE: i64 = 100_000_201;

/// **A gank ends with the ganker's ship destroyed.** After the criminal
/// kill Alice is told CONCORD's warning, CONCORD's ships appear in Carol's
/// park (an `AddBalls2` after the fight's own), and Alice's ship is gone.
///
/// The fight happens on the arrival grid of the invented gate B, which holds
/// a gate checkpoint (lane concord-gate), so CONCORD answers with no delay
/// and Alice may already be in her capsule when `fight()` returns: the test
/// waits for the capsule, not for a change.
#[test]
fn concord_destroys_the_criminal_in_high_sec() {
    let mut f = fight();

    let dead = until(Duration::from_secs(60), || {
        let _ = f.alice.collect_pushes(Duration::from_millis(100));
        f.alice_pushes.extend(f.alice.take_pushes());
        f.alice.session().ship_id() == Some(ALICE_CAPSULE)
    });
    assert!(dead, "Alice's ship is destroyed by CONCORD");

    assert!(
        f.alice_pushes.iter().any(|(e, a)| e == "OnRemoteMessage"
            && a.first()
                == Some(&eve_marshal::PyValue::Bytes(
                    eve_server::concord::WARNING_KEY.as_bytes().to_vec()
                ))),
        "the warning reached the offender: {:?}",
        f.alice_pushes.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );

    let _ = f.carol.collect_pushes(Duration::from_millis(300));
    f.carol_pushes.extend(f.carol.take_pushes());
    assert!(
        f.carol_pushes
            .iter()
            .any(|(e, a)| e == "DoDestinyUpdate" && a.iter().any(|x| mentions(x, b"AddBalls2"))),
        "the squad appeared in the system's park"
    );
}

/// **A criminal is refused the dock in the criminal flag's own words.**
/// Alice, flagged criminal by the gank, asks to dock (the station is
/// Bob's; the refusal comes before any range or station check). Her ship
/// may be dead by now: the flag outlives it and a capsule is refused too.
#[test]
fn a_criminal_is_refused_the_dock_with_the_criminal_flags_words() {
    let mut f = fight();
    let outcome = f
        .alice
        .bound_call(
            "CmdDock",
            vec![
                PyValue::Int(f.bob_station),
                PyValue::Int(f.alice.session().ship_id().expect("a ship")),
            ],
        )
        .expect("the dock is answered");
    assert_eq!(
        outcome.refusal().as_deref(),
        Some(eve_server::concord::REFUSE_DOCK)
    );
}
