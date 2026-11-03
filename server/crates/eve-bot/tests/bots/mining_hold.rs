//! **GAPS-B, the mining hold** over a real socket: a laser cycle on a hull
//! with `generalMiningHoldCapacity` puts its ore in the mining hold (flag
//! 134), not the cargo hold, and the hangar's `Add` takes it home.
//!
//! **Synthetic furniture only** (the no-client-data rule): the hull is
//! invented (the typedata fixture's frigate numbers plus a mining hold);
//! so is the laser; the rock is the shared fixture rock. GM lines are a
//! non-bot's setup, named at the call site.

use super::common::{ROCK, SYSTEM, VELDSPAR, config, gm, login, pilot, rock, serve, until};
use eve_bot::{Conn, LockState};
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::mining_hold::{ATTR_GENERAL_MINING_HOLD_CAPACITY, FLAG_GENERAL_MINING_HOLD};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MINER1: i64 = 90_000_901;
/// An invented hull: the fixture frigate's numbers, a mining hold.
const DIGGER_HULL: i64 = 990_910_001;
/// Its mining hold, m3: more than a Miner I cycle cuts.
const HOLD_M3: f64 = 1_000.0;
/// An invented mining laser (the laser's effect, 67 [V-data]): its cycle,
/// range and cut are this test's own numbers.
const DIGGER_LASER: i64 = 990_910_002;
const LASER_CYCLE_MS: u64 = 5_000;

fn content() -> Content {
    let hull = format!(
        "type\t{DIGGER_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
         76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;479=600000.0;\
         482=260.0;552=40.0;564=640.0;600=5.0;1137=3.0;\
         {ATTR_GENERAL_MINING_HOLD_CAPACITY}={HOLD_M3}\tFixture Digger Hull\n"
    );
    // The laser's effect row (the typedata fixture has none): duration
    // attribute 73, range 54, as `mining.rs` reads them.
    let effect = "effect\t67\t2\t0\t73\t6\t54\t0\t0\tminingLaser\teffects.Mining\n";
    let laser = format!(
        "type\t{DIGGER_LASER}\t54\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t\
         6=1.0;30=1.0;50=1.0;54=15000.0;73={LASER_CYCLE_MS}.0;77=10.0\tFixture Digger Laser\n"
    );
    let text = format!("{}{effect}{hull}{laser}", eve_server::typedata::FIXTURE);
    let td = eve_server::typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![pilot(MINER1, "Miner1", 100_000_901)];
    config.sandbox.rocks =
        eve_server::asteroid::Field::from_rocks([rock(0, 100_000, [0.0, 0.0, 120_000.0])]);
    config.content = Some(Arc::new(content()));
    serve(config)
}

/// The Veldspar units at `flag` in `ship`.
fn veldspar(c: &mut Conn, ship: i64, flag: i64) -> i64 {
    c.ship_items(ship, Some(flag))
        .expect("the ship lists")
        .iter()
        .filter(|r| r.type_id == VELDSPAR && r.flag_id == flag)
        .map(|r| r.quantity)
        .sum()
}

/// **The packet's socket test**: board the hull, mine the rock, and the
/// cycle's ore lands in the mining hold (the cargo hold stays empty); docked,
/// the hangar's `Add` empties the mining hold.
#[test]
fn a_laser_cycle_fills_the_mining_hold_and_unload_empties_it() {
    let addr = start();
    let mut c = login(addr, "Miner1");
    let station = c.session().station_id().expect("docked");
    gm(&mut c, &format!("/createitem {DIGGER_HULL}"));
    gm(&mut c, &format!("/createitem {DIGGER_LASER}"));
    let hangar = c.hangar_items().expect("the hangar lists");
    let hull = hangar
        .iter()
        .find(|r| r.type_id == DIGGER_HULL)
        .expect("the hull")
        .item_id;
    let laser = hangar
        .iter()
        .find(|r| r.type_id == DIGGER_LASER)
        .expect("the laser")
        .item_id;
    c.board(hull).expect("boards");
    let laser = c.fit_auto(&[laser]).expect("fits")[0].item_id;

    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("a state");
    c.follow(ROCK, 1_000.0).expect("follows");
    c.request_state().expect("a fresh state");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut state = c.lock_target(ROCK).expect("AddTarget is answered");
    while state == LockState::Pending && Instant::now() < deadline {
        let pushes = c.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, eve_bot::Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(ROCK)))
        }) {
            state = LockState::Locked;
        }
    }
    assert_eq!(state, LockState::Locked);
    c.mine(laser, ROCK).expect("the laser starts");

    let cycle = Duration::from_millis(LASER_CYCLE_MS);
    assert!(
        until(cycle + Duration::from_secs(10), || veldspar(
            &mut c,
            hull,
            FLAG_GENERAL_MINING_HOLD
        ) > 0),
        "ore in the mining hold after a cycle"
    );
    c.deactivate(laser, eve_bot::MINING_LASER.0)
        .expect("Deactivate is answered");
    assert_eq!(
        veldspar(&mut c, hull, eve_bot::FLAG_CARGO),
        0,
        "the cargo hold is not used while the mining hold has room"
    );

    c.follow(station, 0.0).expect("approaches");
    c.request_state().expect("a fresh state");
    c.dock(station).expect("docks");
    let row = c
        .ship_items(hull, Some(FLAG_GENERAL_MINING_HOLD))
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == VELDSPAR)
        .expect("the stack");
    c.unload(row.item_id, None)
        .expect("unloads the mining hold");
    assert_eq!(veldspar(&mut c, hull, FLAG_GENERAL_MINING_HOLD), 0);
    assert!(
        c.hangar_items()
            .expect("lists")
            .iter()
            .any(|r| r.item_id == row.item_id && r.flag_id == eve_bot::FLAG_HANGAR),
        "the hangar has it"
    );
}
