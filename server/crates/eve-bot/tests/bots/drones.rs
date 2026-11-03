//! K-DRONE over a real socket: a ratter that flies a drone hull carries
//! combat drones, launches them when it picks a fight, kills a rat with
//! them, and recalls them when it is quiet.
//!
//! **Invented rows only** (the no-client-data rule): the hull, the drone and
//! the numbers below are made up for this test and carry the real type ids
//! only so the bot's own constants name them. No `[A]` value is an expected
//! value: the assertions are about kinds, ids and order.

use super::common::{SYSTEM, config, dump, gm, login, ok, pilot, serve, until};
use eve_bot::brain::{ActKind, Role};
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const BOT: i64 = 90_000_001;
const ALICE: i64 = 90_000_002;
const ALICE_SHIP: i64 = 100_000_002;
const HIJACKER: i64 = eve_server::npc::FIXTURE_RAT.type_id;
const RIFTER: i64 = 587;

/// The drone hull's rows: a cruiser with the escort's slots and a drone bay.
/// Invented numbers.
const ROWS: &str = "\
type\t629\t26\t6\t1\t1\t11000000.0\t200.0\t1000000.0\t100.0\t0\t\t9=500.0;11=900.0;12=4.0;13=4.0;14=5.0;37=300.0;48=400.0;55=300000.0;70=1.0;76=50000.0;101=1.0;102=4.0;192=5.0;263=500.0;265=500.0;283=40.0;479=500000.0;482=500.0;552=100.0;564=300.0;600=3.0;1137=3.0;1271=40.0;1281=3.0\tFixture Drone Cruiser
type\t484\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=83.0\tFixture Gatling
type\t185\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t114=7.0;117=3.0;128=1.0\tFixture Rounds
type\t2454\t100\t18\t1\t1\t5.0\t0.0\t3000.0\t15.0\t10\t10\t9=100.0;37=2800.0;51=3000.0;54=3000.0;64=3.0;70=100.0;118=30.0;158=2000.0;160=2.0;263=50.0;265=90.0;416=700.0;479=125000.0;482=1.0;508=660.0;552=25.0;620=25.0;1272=5.0\tFixture Light Drone
";

fn content() -> Arc<eve_server::content::Content> {
    let text = format!("{}{ROWS}", eve_server::typedata::FIXTURE);
    let table = eve_server::typedata::Table::parse(&text).expect("the rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(c)
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(BOT, "Rat1", 100_000_001),
        eve_server::model::Character {
            ship_type_id: RIFTER,
            ..pilot(ALICE, "Alice", ALICE_SHIP)
        },
    ];
    config.content = Some(content());
    serve(config)
}

fn ratter(addr: SocketAddr) -> Handle {
    let mut config = BotConfig::new("Rat1", addr, Pace::Fast).with_role(Role::Escort);
    config.policy.combat_drones = true;
    driver::spawn(config).0
}

/// **A ratter launches drones that kill a rat, and recalls them.**
#[test]
fn a_ratter_launches_drones_that_kill_a_rat_and_recalls_them() {
    let addr = start();
    let bot = ratter(addr);
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Undock))),
        "the bot sets itself up and undocks:\n{}",
        dump(&bot)
    );

    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    gm(&mut alice, &format!("/entity deploy 1 {HIJACKER}"));
    let mut rat = None;
    let mut drone_seen = false;
    let gone = until(Duration::from_secs(150), || {
        let _ = alice.pump(Duration::from_millis(100));
        if rat.is_none() {
            rat = alice
                .world()
                .balls()
                .find(|b| b.type_id() == Some(HIJACKER))
                .map(|b| b.ball_id);
        }
        drone_seen |= alice.world().balls().any(|b| b.type_id() == Some(2454));
        rat.is_some_and(|r| alice.world().get(r).is_none())
    });
    let rat = rat.expect("the deploy put a rat in the park");
    assert!(gone, "the rat dies:\n{}", dump(&bot));
    assert!(drone_seen, "drones were in space:\n{}", dump(&bot));

    let log = bot.log();
    let engaged = log
        .iter()
        .position(|e| ok(e, ActKind::Drone) && e.ids == vec![rat]);
    assert!(
        engaged.is_some(),
        "the drones were sent at the rat:\n{}",
        dump(&bot)
    );
    let engaged = engaged.expect("checked");
    assert!(
        until(Duration::from_secs(60), || bot.log()[engaged..].iter().any(
            |e| ok(e, ActKind::Drone) && e.what.contains("CmdReturnBay")
        )),
        "the drones are recalled afterwards:\n{}",
        dump(&bot)
    );
    // And they came aboard: no drone ball is left in Alice's park.
    assert!(
        until(Duration::from_secs(30), || {
            let _ = alice.pump(Duration::from_millis(100));
            !alice.world().balls().any(|b| b.type_id() == Some(2454))
        }),
        "no drone is left in space:\n{}",
        dump(&bot)
    );
    let _ = PyValue::None;
    bot.stop();
}
