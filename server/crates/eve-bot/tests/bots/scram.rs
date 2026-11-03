//! BOT-SCRAM over a real socket: an NPC warp scrambler beside a mining bot
//! whose hull carries combat drones. The bot is told `OnEwarStart(...,
//! "warpScrambler")`, sends its drones at the scrambler, and never
//! reconnects (the server answers a dropped session in space with an
//! emergency warp).
//!
//! **The NPC kit** is the server's own, reachable from here: `npc_data`'s
//! invented fixture table (type 990300002 is its `TURRET,SCRAM` tackler) and
//! invented typedata for it (the numbers of `npc_ewar`'s tests, which are
//! made up). The miner is `miner_defense`'s: invented hull, laser, drone.
//! No `[A]` value is an expected value.

use super::common::{SYSTEM, config, dump, login, ok, pilot, rock, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_server::server::Server;
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const BOT: i64 = 90_000_011;
const ALICE: i64 = 90_000_012;
/// `npc_data::Table::fixture()`'s tackler.
const TACKLER: i64 = 990_300_002;

const ROWS: &str = "\
type\t603\t26\t6\t1\t1\t11000000.0\t200.0\t1000000.0\t100.0\t0\t\t9=500.0;11=900.0;12=4.0;13=4.0;14=5.0;37=300.0;48=400.0;55=300000.0;70=1.0;76=50000.0;101=1.0;102=4.0;192=5.0;263=500.0;265=500.0;283=40.0;479=500000.0;482=500.0;552=100.0;564=300.0;600=3.0;1137=3.0;1271=40.0;1281=3.0\tFixture Mining Barge
type\t483\t483\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t67\t67\t6=0.0;30=1.0;50=1.0;54=10000.0;73=60000.0;77=40.0\tFixture Laser
type\t2454\t100\t18\t1\t1\t5.0\t0.0\t3000.0\t15.0\t10\t10\t9=100.0;37=2800.0;51=3000.0;54=3000.0;64=3.0;70=100.0;118=30.0;158=2000.0;160=2.0;263=50.0;265=90.0;416=700.0;479=125000.0;482=1.0;508=660.0;552=25.0;620=25.0;1272=5.0\tFixture Light Drone
type\t2183\t100\t18\t1\t1\t10.0\t0.0\t5000.0\t15.0\t10\t10\t9=100.0;37=2800.0;51=3000.0;54=3000.0;64=3.0;70=100.0;118=30.0;158=2000.0;160=2.0;263=50.0;265=90.0;416=700.0;479=125000.0;482=1.0;508=660.0;552=25.0;620=25.0;1272=10.0\tFixture Medium Drone
type\t990300002\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10,563,575\t114=2.0;103=10000.0;105=2.0;504=1.0;505=4000.0;512=1.0;20=-40.0;513=4000.0;514=10000.0;9=300.0\tFixture Tackler
";

fn content() -> Arc<eve_server::content::Content> {
    let text = format!("{}{ROWS}", eve_server::typedata::FIXTURE);
    let table = eve_server::typedata::Table::parse(&text).expect("the rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    c.insert("npc_data", eve_server::npc_data::Table::fixture());
    Arc::new(c)
}

fn start() -> (SocketAddr, Arc<RwLock<Registry>>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(BOT, "Miner1", 100_000_011),
        pilot(ALICE, "Alice", 100_000_012),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks(
        (0..2).map(|i| rock(i, 100_000, [3_000.0 + i as f64 * 2_000.0, 0.0, 3_000.0])),
    );
    config.content = Some(content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn miner(addr: SocketAddr) -> Handle {
    let mut config = BotConfig::new("Miner1", addr, Pace::Fast);
    config.policy.combat_drones = true;
    config.content = Some(content());
    driver::spawn(config).0
}

/// **Scrambled, the miner fights the scrambler and does not reconnect.**
#[test]
fn a_scrambled_miner_sends_its_drones_at_the_scrambler_and_never_reconnects() {
    let (addr, world) = start();
    let bot = miner(addr);
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Mine))),
        "the bot sets itself up and mines:\n{}",
        dump(&bot)
    );
    let reconnects = |bot: &Handle| {
        bot.log()
            .iter()
            .filter(|e| e.kind == ActKind::Reconnect)
            .count()
    };
    let before = reconnects(&bot);

    // A watcher, to see the park from outside.
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    // The tackler is put beside the miner's ship, with the server's own
    // entry points (`/entity deploy` places it beside the GM).
    {
        let mut w = world.write().expect("world");
        let kind = eve_server::npc::resolve(&mut w, TACKLER).expect("the fixture tackler");
        let at = w
            .roster()
            .get(BOT)
            .expect("the miner is in space")
            .motion
            .position;
        let at = [at[0] + 3_000.0, at[1], at[2]];
        eve_server::npc::spawn(&mut w, kind, SYSTEM, at).expect("an id");
    }
    let mut npc = None;
    let settled = until(Duration::from_secs(120), || {
        let _ = alice.pump(Duration::from_millis(100));
        if npc.is_none() {
            npc = alice
                .world()
                .balls()
                .find(|b| b.type_id() == Some(TACKLER))
                .map(|b| b.ball_id);
        }
        // The drones went at the scrambler, or the scrambler is dead.
        npc.is_some_and(|n| {
            bot.log()
                .iter()
                .any(|e| ok(e, ActKind::Drone) && e.ids == vec![n])
        })
    });
    let npc = npc.expect("the deploy put a tackler in the park");
    assert!(
        settled,
        "the drones were sent at the scrambler:\n{}",
        dump(&bot)
    );

    // The next stretch of the fight: no reconnect, and the bot is on grid
    // or the scrambler is dead.
    let end = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < end {
        let _ = alice.pump(Duration::from_millis(200));
        if alice.world().get(npc).is_none() {
            break;
        }
    }
    assert_eq!(reconnects(&bot), before, "no reconnect:\n{}", dump(&bot));
    assert!(
        alice.world().get(npc).is_none() || alice.world().balls().any(|b| b.type_id() == Some(603)),
        "the scrambler died or the miner is still on grid:\n{}",
        dump(&bot)
    );
    bot.stop();
}
