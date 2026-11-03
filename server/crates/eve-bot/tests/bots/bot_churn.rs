//! **BOT-CHURN over a real socket**: in the owner's dev world a miner and an
//! Orca undocked and docked again 6-10 s later, over and over. The cause was
//! courtesy (`brain::courtesy`): the owner's client, a human outside the
//! bots' fleet, was in the park within 100 km, and every civilian role steps
//! aside for one -- it goes home, which is a dock at the mouth.
//!
//! - **Control**: a miner with courtesy on and no dev roster docks again with
//!   the human in the park, and its dock's note names courtesy and the
//!   distance (the reason every dock now logs).
//! - **A dev-roster miner** with the same human stays out: it mines, and for
//!   a minute after its undock it never docks, unless its hold is full.
//!
//! The Orca's twin is in `prog_mine.rs` (it has the invented command hull).
//! No `[A]` value is an expected value: kinds, order and the reasons' words.

use super::common::{SYSTEM, config, dump, login, ok, pilot, rock, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const BOT: i64 = 90_000_701;
const ALICE: i64 = 90_000_702;

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(BOT, "Miner1", 100_000_701),
        pilot(ALICE, "Alice", 100_000_702),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks(
        (0..2).map(|i| rock(i, 100_000, [3_000.0 + i as f64 * 2_000.0, 0.0, 3_000.0])),
    );
    serve(config)
}

/// The invented roster entry a dev world gives every roster bot.
pub(super) fn dev_bot() -> Arc<eve_bot::dev::DevBot> {
    Arc::new(eve_bot::dev::DevBot {
        group: "invented".to_owned(),
        ticker: "INV".to_owned(),
        skills: eve_bot::dev::Skills::All5,
        skill_ids: None,
        hull: None,
        home: None,
        boss: false,
        hall: Arc::default(),
    })
}

fn miner(addr: SocketAddr, dev: bool) -> Handle {
    let mut config = BotConfig::new("Miner1", addr, Pace::Fast);
    config.policy.courtesy = true;
    config.policy.dev = dev.then(dev_bot);
    config.policy.combat_drones = false;
    driver::spawn(config).0
}

/// A human in the park: undocked, out of the bots' marker.
fn human(addr: SocketAddr) -> eve_bot::Conn {
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    alice
}

fn docks(h: &Handle) -> Vec<String> {
    h.log()
        .iter()
        .filter(|e| e.kind == ActKind::Dock)
        .map(|e| e.what.clone())
        .collect()
}

/// **Control**: courtesy on, no dev roster: the miner docks for the human,
/// and says so.
#[test]
fn a_miner_with_courtesy_docks_for_a_human_and_logs_why() {
    let addr = start();
    let mut alice = human(addr);
    let bot = miner(addr, false);
    let docked = until(Duration::from_secs(90), || {
        let _ = alice.pump(Duration::from_millis(100));
        docks(&bot).iter().any(|w| w.contains("courtesy"))
    });
    bot.stop();
    assert!(docked, "it steps aside for the human:\n{}", dump(&bot));
    let note = docks(&bot).remove(0);
    assert!(note.contains("why: courtesy: a human"), "{note}");
    assert!(note.contains("km away"), "{note}");
}

/// **A dev-roster miner stays out**: it mines, and for sixty seconds after
/// its first undock it does not dock, unless its hold is full (then the
/// dock says so).
#[test]
fn a_dev_roster_miner_with_a_human_in_the_park_undocks_and_stays_out() {
    let addr = start();
    let mut alice = human(addr);
    let bot = miner(addr, true);
    assert!(
        until(Duration::from_secs(90), || {
            let _ = alice.pump(Duration::from_millis(100));
            bot.log().iter().any(|e| ok(e, ActKind::Mine))
        }),
        "the bot sets itself up and mines:\n{}",
        dump(&bot)
    );
    let from = Instant::now();
    while from.elapsed() < Duration::from_secs(60) {
        let _ = alice.pump(Duration::from_millis(200));
    }
    bot.stop();
    for note in docks(&bot) {
        assert!(
            note.contains("HoldFull"),
            "a dock with no full hold: {note}\n{}",
            dump(&bot)
        );
    }
    assert!(
        !bot.log().iter().any(|e| e.what.contains("courtesy")),
        "no courtesy in a dev world:\n{}",
        dump(&bot)
    );
}
