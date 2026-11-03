//! SESSION-GM over a real socket: the Escape menu's log off returns to the
//! select screen and the same connection can pick a character again, and
//! `/tr me <id>` is a session change the connection follows with no
//! reconnect. Both sides marshal with `eve-marshal`, so this proves the
//! server's behaviour (the change, its keys, the answer's value), not the
//! client's reading of it: `fci_session_gm` is the client half.
//!
//! **Synthetic furniture**: two systems and a planet in each, made-up ids.

use super::common::{config, pilot, serve};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::time::Duration;

const ALICE: i64 = 90_000_001;
const ALICE_SHIP: i64 = 100_000_001;
const A: i64 = 30_000_001;
const B: i64 = 30_000_003;
const PLANET_B: i64 = 40_000_102;

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", ALICE_SHIP)];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{A}\t20000001\t10000001\n\
         #system\t{B}\t20000002\t10000002\n\
         planet\t40000002\t11\t500.0\t100000000.0\t0.0\t0.0\t{A}\t0\t0\n\
         planet\t{PLANET_B}\t11\t500.0\t0.0\t100000000.0\t0.0\t{B}\t0\t0\n"
    ));
    serve(config)
}

fn alice(addr: SocketAddr) -> eve_bot::Conn {
    let mut c = eve_bot::Conn::connect(addr, Duration::from_secs(5)).expect("connects");
    c.handshake().expect("handshake");
    c.select_character(ALICE).expect("a character");
    c
}

#[test]
fn log_off_answers_true_with_no_change_and_the_character_can_be_picked_again() {
    let mut c = alice(start());
    let out = c
        .call("userSvc", "UserLogOffCharacter", vec![])
        .expect("answered");
    // The client's `if PerformSessionChange(...)` needs a truthy answer.
    assert_eq!(out.value(), Some(PyValue::Bool(true)), "{out:?}");
    // The client clears its own session: a change from here raises in it.
    assert!(out.session_change.is_none(), "{out:?}");
    // Back on the select screen: the same connection selects again.
    let again = c.select_character(ALICE).expect("selects again");
    let change = again.session_change.expect("a login change");
    assert_eq!(change.new_i64("charid"), Some(ALICE));
}

#[test]
fn tr_to_another_system_is_a_session_change_with_no_reconnect() {
    let mut c = alice(start());
    let out = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(format!("/tr me {PLANET_B}").into_bytes())],
        )
        .expect("answered");
    let change = out.session_change.expect("the move is a session change");
    assert_eq!(change.new_i64("solarsystemid"), Some(B), "{change:?}");
    assert_eq!(change.new_i64("solarsystemid2"), Some(B), "{change:?}");
    assert!(change.cleared("stationid"), "{change:?}");
    // The bind that follows lands the ship in B.
    let world = c.enter_space(B).expect("a state in the new system");
    assert!(
        world.ego == ALICE_SHIP && !world.park.balls.is_empty(),
        "a scene in system B"
    );
}
