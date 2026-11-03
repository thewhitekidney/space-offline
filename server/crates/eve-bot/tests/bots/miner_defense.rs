//! MINER-DEFENSE over a real socket: a mining bot whose hull has a drone
//! bay is aggressed by a rat, launches its drones, sends them at the rat,
//! keeps mining, and recalls the drones once the rat is dead.
//!
//! **Invented rows only**: the hull (it wears the Merlin's type id so the
//! GM setup flies it), the drone and every number below are made up. No
//! `[A]` value is an expected value: the assertions are about kinds, ids and
//! order.

use super::common::{ROCK, SYSTEM, config, dump, gm, login, ok, pilot, rock, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const BOT: i64 = 90_000_001;
const ALICE: i64 = 90_000_002;
const HIJACKER: i64 = eve_server::npc::FIXTURE_RAT.type_id;
const HULL: i64 = 603;
const DRONE: i64 = 2183;

/// A miner hull with a 40 m3 bay and 40 Mbit/s, its laser and a medium drone.
const ROWS: &str = "\
type\t603\t26\t6\t1\t1\t11000000.0\t200.0\t1000000.0\t100.0\t0\t\t9=500.0;11=900.0;12=4.0;13=4.0;14=5.0;37=300.0;48=400.0;55=300000.0;70=1.0;76=50000.0;101=1.0;102=4.0;192=5.0;263=500.0;265=500.0;283=40.0;479=500000.0;482=500.0;552=100.0;564=300.0;600=3.0;1137=3.0;1271=40.0;1281=3.0\tFixture Mining Barge
type\t483\t483\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t67\t67\t6=0.0;30=1.0;50=1.0;54=10000.0;73=60000.0;77=40.0\tFixture Laser
type\t2454\t100\t18\t1\t1\t5.0\t0.0\t3000.0\t15.0\t10\t10\t9=100.0;37=2800.0;51=3000.0;54=3000.0;64=3.0;70=100.0;118=30.0;158=2000.0;160=2.0;263=50.0;265=90.0;416=700.0;479=125000.0;482=1.0;508=660.0;552=25.0;620=25.0;1272=5.0\tFixture Light Drone
type\t2183\t100\t18\t1\t1\t10.0\t0.0\t5000.0\t15.0\t10\t10\t9=100.0;37=2800.0;51=3000.0;54=3000.0;64=3.0;70=100.0;118=30.0;158=2000.0;160=2.0;263=50.0;265=90.0;416=700.0;479=125000.0;482=1.0;508=660.0;552=25.0;620=25.0;1272=10.0\tFixture Medium Drone
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
        pilot(BOT, "Miner1", 100_000_001),
        pilot(ALICE, "Alice", 100_000_002),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks(
        (0..2).map(|i| rock(i, 100_000, [3_000.0 + i as f64 * 2_000.0, 0.0, 3_000.0])),
    );
    config.content = Some(content());
    serve(config)
}

fn miner(addr: SocketAddr) -> Handle {
    let mut config = BotConfig::new("Miner1", addr, Pace::Fast);
    config.policy.combat_drones = true;
    config.content = Some(content());
    driver::spawn(config).0
}

/// **A miner is aggressed, launches drones, they kill the rat, and come
/// home.**
#[test]
fn an_aggressed_miner_launches_drones_that_kill_the_rat_and_recalls_them() {
    let addr = start();
    let bot = miner(addr);
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Mine))),
        "the bot sets itself up and mines:\n{}",
        dump(&bot)
    );
    assert!(
        bot.log()
            .iter()
            .any(|e| e.kind == ActKind::Drone && e.what.contains("defence drones")),
        "the defence drones were stocked before the undock:\n{}",
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
        drone_seen |= alice.world().balls().any(|b| b.type_id() == Some(DRONE));
        rat.is_some_and(|r| alice.world().get(r).is_none())
    });
    let rat = rat.expect("the deploy put a rat in the park");
    assert!(gone, "the rat dies:\n{}", dump(&bot));
    assert!(drone_seen, "drones were in space:\n{}", dump(&bot));
    let log = bot.log();
    let engaged = log
        .iter()
        .position(|e| ok(e, ActKind::Drone) && e.ids == vec![rat])
        .unwrap_or_else(|| panic!("the drones were sent at the rat:\n{}", dump(&bot)));
    assert!(
        until(Duration::from_secs(60), || bot.log()[engaged..].iter().any(
            |e| ok(e, ActKind::Drone) && e.what.contains("CmdReturnBay")
        )),
        "the drones are recalled afterwards:\n{}",
        dump(&bot)
    );
    assert!(
        until(Duration::from_secs(30), || {
            let _ = alice.pump(Duration::from_millis(100));
            !alice.world().balls().any(|b| b.type_id() == Some(DRONE))
        }),
        "no drone is left in space:\n{}",
        dump(&bot)
    );
    let _ = (ROCK, HULL);
    bot.stop();
}
