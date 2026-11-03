//! CP34 over a real socket: a courier bot takes a courier mission from an
//! agent, hauls the cargo and turns the mission in (`courier::run`).
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): the mission pool is the
//! server's own fixture (`mission_pool::Table::fixture`) plus two invented
//! distribution agents and one invented courier mission. The giver is an
//! agent in space, so its cargo appears in the pilot's own station; the
//! addressee sits in that station, so the drop-off is the one station this
//! server docks a ship in. A haul between two stations needs the server to
//! dock at `CmdDock`'s station (`docs/lanes/cp34.md`, Not done); the way
//! there is `courier::tests`' table. The cargo type is the server's own
//! courier cargo, `mission::CARGO_TYPE_ID`.

use super::common::{SYSTEM, config, gm, login, pilot, serve};
use eve_bot::courier::{self, CourierLeg};
use eve_server::mission_pool::{self as pool, Agent, Mission, Table};
use std::net::SocketAddr;
use std::sync::Arc;

const COURIER1: i64 = 90_000_001;
const GIVER: i64 = 3_998_001;
const ADDRESSEE: i64 = 3_998_002;
const HAUL: i64 = 998_001;
/// Two units of the server's courier cargo: inside a Merlin's hold.
const UNITS: i64 = 2;
/// `division 22`: a distribution agent, whose pool is couriers (C4 §3.3)
/// [V].
const DISTRIBUTION: i64 = 22;

/// The home station, `Character::default()`'s.
fn home() -> i64 {
    eve_server::model::Character::default()
        .station_id
        .expect("the default character is docked")
}

/// The fixture pool, plus the two agents and the haul.
fn pool() -> Table {
    let fixture = Table::fixture();
    let agent = |id, station| Agent {
        id,
        agent_type: pool::TYPE_BASIC,
        division: DISTRIBUTION,
        level: 1,
        corp: 1_998_001,
        faction: 598_001,
        station,
        system: Some(SYSTEM),
        ..Agent::default()
    };
    let mut agents: Vec<Agent> = fixture.agents().cloned().collect();
    agents.push(agent(GIVER, None));
    agents.push(agent(ADDRESSEE, Some(home())));
    let mut missions: Vec<Mission> = fixture.missions().cloned().collect();
    missions.push(Mission {
        id: HAUL,
        name_id: 998_901,
        template: "BasicCourier".into(),
        family: "courier".into(),
        level: Some(1),
        obj_type: Some(eve_server::mission::CARGO_TYPE_ID),
        obj_qty: Some(UNITS),
        ..Mission::default()
    });
    Table::new(agents, missions, pool::Policy::TRANQUILITY)
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(COURIER1, "Courier1", 100_000_001)];
    let mut content = eve_server::content::Content::fixture();
    content.insert("mission_pool", pool());
    config.content = Some(Arc::new(content));
    serve(config)
}

/// **The row's test**: accept, carry, complete -- with a hull from a
/// non-bot's setup, named here.
#[test]
fn a_bot_accepts_carries_and_completes_a_courier_mission() {
    let addr = start();
    let mut c = login(addr, "Courier1");
    gm(&mut c, "/createitem merlin");

    let done = courier::run(&mut c, GIVER).expect("the courier hauls");

    assert_eq!(
        done.leg,
        CourierLeg {
            mission_id: HAUL,
            pickup_station: home(),
            pickup_system: SYSTEM,
            dropoff_station: home(),
            dropoff_system: SYSTEM,
            cargo_type: eve_server::mission::CARGO_TYPE_ID,
            quantity: UNITS,
        },
        "the leg is the drawn mission's"
    );
    assert_eq!(done.loaded, UNITS, "the cargo went into the hold");
    assert_eq!(done.unloaded, UNITS, "and out of it at the drop-off");
    assert!(done.completed, "lastActionInfo says completed");

    let cargo = eve_server::mission::CARGO_TYPE_ID;
    assert!(
        c.cargo()
            .expect("the hold lists")
            .iter()
            .all(|r| r.type_id != cargo),
        "nothing left in the hold"
    );
    let delivered: i64 = c
        .hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == cargo)
        .map(|r| r.quantity)
        .sum();
    assert_eq!(delivered, UNITS, "the cargo is in the drop-off's hangar");

    // Turned in: the agent's document is an offer again, with no leg.
    let after = c.mission_objective(GIVER).expect("the objective reads");
    assert_eq!(CourierLeg::from_objective(&after), None);
}

/// Complete is drawn only after Accept: a bot cannot press it first.
#[test]
fn complete_is_not_offered_before_accept() {
    let addr = start();
    let mut c = login(addr, "Courier1");
    let turn = c.talk_to_agent(GIVER).expect("the agent answers");
    assert_eq!(turn.content_id, Some(HAUL), "the offer is the haul");
    assert!(turn.action_for(courier::BUTTON_ACCEPT).is_some());
    match c.agent_button(GIVER, &turn, courier::BUTTON_COMPLETE) {
        Err(eve_bot::Error::NotSent(_)) => {}
        other => panic!("Complete before Accept: {other:?}"),
    }
}
