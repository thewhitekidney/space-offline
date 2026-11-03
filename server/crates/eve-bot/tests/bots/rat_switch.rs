//! K-RATAI over a real socket: a switching rat turns from one pilot to a
//! second, newer aggressor.
//!
//! Alice and Bob fly in one system. A rat (invented type, the fixture
//! `npc/` row 990300001 with its own switch attributes 1648 and 1416) is
//! spawned near Alice and locks her. Alice locks it back; for several
//! switch timers it keeps her. Bob then locks it: at its next timer it
//! drops Alice and locks Bob, and the lock graph holds the rat on Bob. Invented rows only; no `[A]` value is asserted (the rule
//! "newest aggressor" is the thing under test, named in `npc::brain`).

use super::common::{self, SYSTEM, config, pilot};
use eve_server::{Server, typedata};
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_401;
const BOB: i64 = 90_000_402;
const ALICE_SHIP: i64 = 100_000_401;
const BOB_SHIP: i64 = 100_000_402;
const RAT: i64 = 990_300_001;
/// The rat's switch timer, attribute 1416, in ms.
const TIMER_MS: u64 = 2_000;

fn content() -> std::sync::Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{RAT}\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10\t\
         9=300.0;37=300.0;51=2500.0;64=1.0;76=60000.0;114=0.1;192=3.0;247=15000.0;\
         263=500.0;265=500.0;416=1000.0;508=200.0;552=30.0;564=500.0;\
         1416={TIMER_MS}.0;1648=1.0\tInvented Rat\n",
        typedata::FIXTURE
    );
    let mut c = eve_server::content::Content::fixture();
    let table = typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", typedata::Installed::build(table));
    std::sync::Arc::new(c)
}

fn in_space(addr: std::net::SocketAddr, who: i64) -> eve_bot::Conn {
    let mut c = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("connects");
    c.handshake().expect("handshake");
    c.select_character(who).expect("a character");
    c.undock().expect("undocks");
    c.enter_space(SYSTEM).expect("reads the world");
    c
}

fn target_of(world: &std::sync::RwLock<eve_server::service::Registry>, rat: i64) -> Option<i64> {
    world
        .read()
        .expect("world")
        .npcs()
        .get(rat)
        .and_then(|n| n.target)
}

#[test]
fn a_rat_turns_to_its_newer_aggressor() {
    let mut config = config();
    config.sandbox.characters = vec![
        eve_server::model::Character {
            ship_type_id: 587,
            ..pilot(ALICE, "Alice", ALICE_SHIP)
        },
        eve_server::model::Character {
            ship_type_id: 587,
            ..pilot(BOB, "Bob", BOB_SHIP)
        },
    ];
    config.content = Some(content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let mut alice = in_space(addr, ALICE);
    let rat = {
        let mut w = world.write().expect("world");
        let at = w
            .roster()
            .by_ship(ALICE_SHIP)
            .expect("alice flies")
            .motion
            .position;
        let kind = eve_server::npc::resolve(&mut w, RAT).expect("the rat derives");
        eve_server::npc::spawn(&mut w, kind, SYSTEM, [at[0] + 4_000.0, at[1], at[2]])
            .expect("a rat")
    };
    assert!(
        common::until(Duration::from_secs(30), || target_of(&world, rat)
            == Some(ALICE_SHIP)),
        "the rat locks Alice"
    );
    alice.lock(rat).expect("alice locks the rat");
    // Several timers pass with only Alice aggressing: it keeps her.
    let end = Instant::now() + Duration::from_millis(3 * TIMER_MS + 500);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(target_of(&world, rat), Some(ALICE_SHIP));
    }

    let mut bob = in_space(addr, BOB);
    bob.lock(rat).expect("bob locks the rat");
    assert!(
        common::until(Duration::from_secs(30), || target_of(&world, rat)
            == Some(BOB_SHIP)),
        "the rat turns to Bob, its newest aggressor"
    );
    let w = world.read().expect("world");
    assert!(
        w.locks().targets_of(rat).contains(&BOB_SHIP),
        "locked on Bob"
    );
    assert!(
        !w.locks().targets_of(rat).contains(&ALICE_SHIP),
        "and let go of Alice"
    );
}
