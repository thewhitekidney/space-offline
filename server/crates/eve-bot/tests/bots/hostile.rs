//! CP56 over a real socket: a hostile bot -- brain, driver, verbs --
//! hunting a human against an in-process server.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is
//! an expected value here**: the policy table is an invented fixture, the
//! security an invented number on the sandbox's system, and the assertions
//! are about kinds and ids.
//!
//! **The human is a plain connection** ("Alice", in a Rifter). The bot
//! sees her ship's type through its slim, which is what the policy reads.

use super::common::{SYSTEM, config, dump, gm_only_in_setup, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Pace};
use eve_bot::hostile::{self, Class, Rules, Table};
use eve_bot::roles;
use std::sync::Arc;
use std::time::Duration;

const HOSTILE: i64 = 90_000_101;
const ALICE: i64 = 90_000_102;
const ALICE_SHIP: i64 = 100_000_102;
/// A Rifter **[V-data]**: Alice's hull, classed `small` by the fixture.
const RIFTER: i64 = 587;

/// The row is registered, not a civilian, and refused without the opt-in.
#[test]
fn the_hostile_row_is_registered_opt_in_and_not_civilian() {
    let row = roles::parse(hostile::ROLE.name()).expect("--loop hostile parses");
    assert!(std::ptr::eq(row, &hostile::ROLE));
    assert!(!row.civilian);
    assert!(hostile::check_opt_in([row], false).is_err());
    assert!(hostile::check_opt_in([row], true).is_ok());
}

/// **In lawful space, a human the table allows is shot.** Alice undocks
/// in her Rifter and waits at the station; the bot -- with an invented
/// table that engages small hulls, and the sandbox's system given an
/// invented zero-sec security -- sets itself up, undocks onto her grid, and
/// its guns go on her ship. Its token is never its name (CA19).
#[test]
fn a_hostile_bot_fires_on_a_human_in_lawful_space() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(HOSTILE, "Hostile1", 100_000_101),
        eve_server::model::Character {
            ship_type_id: RIFTER,
            ..pilot(ALICE, "Alice", ALICE_SHIP)
        },
    ];
    let addr = serve(cfg);

    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");

    let table = Table::parse(&format!(
        "{}\nsmall\t-1.0\t0.0\t1\t[A] invented\n",
        hostile::HEADER
    ))
    .expect("the fixture parses");
    let rules = Rules::new(table)
        .with_security(SYSTEM, -0.5)
        .with_class(RIFTER, Class::Small);
    let mut config = BotConfig::new("Hostile1", addr, Pace::Fast).with_role(&hostile::ROLE);
    assert_ne!(config.token, config.name, "the name is never the token");
    config.policy.hostile = Some(Arc::new(rules));
    let bot = driver::spawn(config).0;

    let fired = until(Duration::from_secs(120), || {
        let _ = alice.collect_pushes(Duration::from_millis(50));
        bot.log()
            .iter()
            .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&ALICE_SHIP))
    });
    assert!(fired, "the bot's guns go on Alice:\n{}", dump(&bot));
    gm_only_in_setup(&bot);
    bot.stop();
}
