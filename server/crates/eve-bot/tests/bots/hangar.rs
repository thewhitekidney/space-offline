//! **CAP-H over a real socket**: a fleet member puts cargo into a carrier's
//! fleet hangar in space, once the carrier's owner allows it and only within
//! range, and takes it out again.
//!
//! Alice flies an invented carrier (the typedata fixture frigate's numbers
//! plus `fleetHangarCapacity`); Bob flies an invented small hull with a
//! little ore in his hold. They form a fleet the player way, undock into
//! one system, and Bob calls the carrier's inventory exactly as the client's
//! `ShipFleetHangar` controller does: `GetInventoryFromId(carrierID)` then
//! `Add(itemID, sourceID, qty=n, flag=flagFleetHangar)`
//! (`invControllers.py:__AddItem`) **[V]**. Alice's switches are
//! `ship.ConfigureShip({key: bool})` (`shipConfigSvc.py`) **[V]**.
//!
//! **Synthetic furniture only**: the hulls are invented, the ore is the
//! shared fixture's Veldspar. No `[A]` value is an expected value: the range
//! limit is the named [`MAX_TRANSFER_DISTANCE`], the test sits well inside
//! it and well outside it plus its slack.

use super::common::{SYSTEM, config, gm, login, pilot};
use eve_bot::{Conn, FleetInvite, ItemRow};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::server::Server;
use eve_server::ship_config::{FLAG_FLEET_HANGAR, KEY_FLEET_HANGAR_FLEET, MAX_TRANSFER_DISTANCE};
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_670_001;
const BOB: i64 = 90_670_002;
/// Invented hulls. The fixture frigate's fitting numbers, a hold, and for the
/// carrier a fleet hangar.
const CARRIER: i64 = 990_670_001;
const SMALL: i64 = 990_670_002;
/// The hangar's size, m3: invented, room for a few hundred units of ore.
const HANGAR_M3: f64 = 500.0;
/// The ore: Veldspar **[V-data]** (the shared fixture's).
const VELDSPAR: i64 = 1230;
const ORE: i64 = 100;
const CARGO: i64 = 5;

fn content() -> Content {
    let hull = |id: i64, extra: &str, name: &str| {
        format!(
            "type\t{id}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
             76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;479=600000.0;\
             482=260.0;552=40.0;564=640.0;600=5.0;1137=3.0{extra}\t{name}\n"
        )
    };
    let text = format!(
        "{}{}{}",
        eve_server::typedata::FIXTURE,
        hull(CARRIER, &format!(";912={HANGAR_M3}"), "Fixture Carrier"),
        hull(SMALL, "", "Fixture Small Hull"),
    );
    let td = eve_server::typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c
}

fn text(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

/// Board a fresh hull of `type_id`, and the ore for Bob if asked.
fn fly(c: &mut Conn, type_id: i64) -> i64 {
    gm(c, &format!("/createitem {type_id}"));
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .iter()
        .find(|r| r.type_id == type_id)
        .expect("the hull")
        .item_id;
    c.board(hull).expect("boards");
    hull
}

fn stuff_in(c: &mut Conn, ship: i64, flag: i64) -> Vec<ItemRow> {
    c.ship_items(ship, Some(flag))
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == VELDSPAR)
        .collect()
}

/// `Add(itemID, sourceID, qty=n, flag=flag)` on `ship`'s inventory.
fn add(c: &mut Conn, ship: i64, item: i64, source: i64, qty: i64, flag: i64) -> Option<String> {
    let inv = c.open_ship(ship).expect("binds the inventory");
    let out = c
        .call_on(
            &inv,
            "Add",
            vec![PyValue::Int(item), PyValue::Int(source)],
            vec![
                (PyValue::Int(qty), text("qty")),
                (PyValue::Int(flag), text("flag")),
            ],
        )
        .expect("the call is answered");
    out.refusal()
}

fn configure(c: &mut Conn, key: &str, on: bool) {
    let params = c.session().location_moniker().expect("in space");
    let changes = PyValue::Dict(vec![(PyValue::Bool(on), text(key))]);
    let answer = c
        .moniker_call("ship", params, "ConfigureShip", vec![changes], vec![])
        .expect("ConfigureShip is answered");
    assert_eq!(answer.outcome.refusal(), None);
}

#[test]
fn a_fleet_member_uses_a_carriers_fleet_hangar_in_space() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_670_001),
        eve_server::model::Character {
            // Not Alice's corporation: only the fleet route can open it.
            corporation_id: 98_670_002,
            ..pilot(BOB, "Bob", 100_670_002)
        },
    ];
    config.content = Some(Arc::new(content()));
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let mut alice = login(addr, "Alice");
    let carrier = fly(&mut alice, CARRIER);
    let mut bob = login(addr, "Bob");
    let small = fly(&mut bob, SMALL);
    gm(&mut bob, &format!("/createitem {VELDSPAR} {ORE}"));
    let ore = bob
        .hangar_items()
        .expect("the hangar lists")
        .iter()
        .find(|r| r.type_id == VELDSPAR)
        .expect("the ore")
        .item_id;
    bob.load_cargo(ore, None).expect("the ore is in the hold");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");

    // The fleet, the player way: Alice invites, Bob accepts.
    let fleet = alice.create_fleet(CARRIER).expect("the fleet");
    let seat = alice.fleet_seat(fleet, 0).expect("a seat");
    alice.fleet_invite(fleet, BOB, seat).expect("the invite");
    let invite = bob
        .pump(Duration::from_secs(5))
        .expect("reads")
        .into_iter()
        .find_map(|e| match e {
            eve_bot::Event::Push(n, a) => FleetInvite::from_push(&n, &a),
            _ => None,
        })
        .expect("Bob is invited");
    bob.accept_fleet_invite(invite.fleet_id, SMALL)
        .expect("Bob joins");

    // Side by side, at rest: well inside the transfer distance.
    let place = |bob_offset: f64| {
        let mut w = world.write().expect("the world");
        let a = w
            .roster()
            .get(ALICE)
            .expect("Alice is in space")
            .motion
            .position;
        for who in [ALICE, BOB] {
            w.roster_mut()
                .get_mut(who)
                .expect("in space")
                .motion
                .velocity = [0.0; 3];
        }
        w.roster_mut().get_mut(BOB).expect("Bob").motion.position = [a[0] + bob_offset, a[1], a[2]];
    };
    place(MAX_TRANSFER_DISTANCE / 4.0);

    // 1. Not allowed: refused with the client's key, nothing moved.
    let refusal = add(&mut bob, carrier, ore, small, 10, FLAG_FLEET_HANGAR);
    assert_eq!(refusal.as_deref(), Some("FleetHangarAccessDenied"));
    assert!(stuff_in(&mut alice, carrier, FLAG_FLEET_HANGAR).is_empty());

    // 2. Alice opens it to the fleet (the call the toggle button makes).
    configure(&mut alice, KEY_FLEET_HANGAR_FLEET, true);
    let answer = alice
        .moniker_call(
            "ship",
            alice.session().location_moniker().expect("in space"),
            "GetShipConfiguration",
            vec![PyValue::Int(carrier)],
            vec![],
        )
        .expect("the config is read");
    let Some(PyValue::Dict(entries)) = answer.value else {
        panic!("a dict: {:?}", answer.value)
    };
    assert!(
        entries
            .iter()
            .any(|(v, k)| *k == text(KEY_FLEET_HANGAR_FLEET) && *v == PyValue::Bool(true)),
        "the switch reads back: {entries:?}"
    );

    // 3. Out of range (surface to surface, past the slack): refused.
    place(MAX_TRANSFER_DISTANCE * 4.0);
    let refusal = add(&mut bob, carrier, ore, small, 10, FLAG_FLEET_HANGAR);
    assert_eq!(refusal.as_deref(), Some("CustomNotify"), "out of range");

    // 4. In range: the units move, into Alice's ownership.
    place(MAX_TRANSFER_DISTANCE / 4.0);
    assert_eq!(
        add(&mut bob, carrier, ore, small, 10, FLAG_FLEET_HANGAR),
        None
    );
    let held: i64 = stuff_in(&mut alice, carrier, FLAG_FLEET_HANGAR)
        .iter()
        .map(|r| r.quantity)
        .sum();
    assert_eq!(held, 10, "ten units are in the carrier's hangar");
    let seen_by_bob = stuff_in(&mut bob, carrier, FLAG_FLEET_HANGAR);
    assert_eq!(
        seen_by_bob.iter().map(|r| r.quantity).sum::<i64>(),
        10,
        "Bob lists the hangar he has access to"
    );
    assert!(seen_by_bob.iter().all(|r| r.owner_id == ALICE));
    let left: i64 = stuff_in(&mut bob, small, CARGO)
        .iter()
        .map(|r| r.quantity)
        .sum();
    assert_eq!(left, ORE - 10, "Bob's hold is ten short");

    // 5. And Bob takes them back: the hangar item into his own hold.
    let row = seen_by_bob[0];
    assert_eq!(add(&mut bob, small, row.item_id, carrier, 10, CARGO), None);
    assert!(stuff_in(&mut alice, carrier, FLAG_FLEET_HANGAR).is_empty());
    let back = stuff_in(&mut bob, small, CARGO);
    assert_eq!(back.iter().map(|r| r.quantity).sum::<i64>(), ORE);
    assert!(back.iter().all(|r| r.owner_id == BOB));
}
