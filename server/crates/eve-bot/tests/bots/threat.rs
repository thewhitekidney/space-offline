//! THREAT over a real socket: a human locks a mining bot's ship and the bot
//! warps home and docks, saying why in its act log.
//!
//! **No `[A]` value is an expected value**: the assertions are about kinds
//! and the reason's text. Alice is a plain connection; the ids are invented.

use super::common::{ROCK, SYSTEM, config, dump, login, ok, pilot, rock, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Pace};
use std::time::Duration;

const BOT: i64 = 90_000_001;
const ALICE: i64 = 90_000_002;

fn start() -> std::net::SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(BOT, "Miner1", 100_000_001),
        pilot(ALICE, "Alice", 100_000_002),
    ];
    config.sandbox.rocks = eve_server::asteroid::Field::from_rocks(
        (0..2).map(|i| rock(i, 100_000, [3_000.0 + i as f64 * 2_000.0, 0.0, 3_000.0])),
    );
    serve(config)
}

/// **A player's lock sends a miner home**: the bot mines; Alice undocks onto
/// its grid and locks its ship; the bot's next dock gives the threat as its
/// reason.
#[test]
fn a_player_lock_makes_a_mining_bot_dock_with_the_reason() {
    let addr = start();
    let mut config = BotConfig::new("Miner1", addr, Pace::Fast);
    config.policy.threat = true;
    let bot = driver::spawn(config).0;
    assert!(
        until(Duration::from_secs(90), || bot
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Mine))),
        "the bot sets itself up and mines:\n{}",
        dump(&bot)
    );
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    let mut locked = false;
    let docked = until(Duration::from_secs(120), || {
        let _ = alice.pump(Duration::from_millis(100));
        if !locked {
            let ball = alice
                .world()
                .balls()
                .find(|b| b.slim.and_then(|s| s.char_id) == Some(BOT) && b.ball_id != ROCK)
                .map(|b| b.ball_id);
            if let Some(ball) = ball {
                locked = alice.lock_target(ball).is_ok();
            }
        }
        bot.log()
            .iter()
            .any(|e| e.kind == ActKind::Dock && e.what.contains("threat"))
    });
    assert!(locked, "Alice locked the bot's ship:\n{}", dump(&bot));
    assert!(docked, "the bot docks because of the lock:\n{}", dump(&bot));
    bot.stop();
}
