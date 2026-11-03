//! MN-C over a real socket: a bot mines a **moon belt** rock into its hold.
//!
//! Nothing but invented furniture (the no-client-data rule): the home system
//! is an invented null-sec row, its one moon an invented celestial, and the
//! ore an invented type in the client's own first moon ore group (1884 [V],
//! `inventorycommon/const.py:1160`) carrying the attributes the server reads
//! to know a moon ore (`oreBasicType` 2711, `asteroidMetaLevel` 2699 and the
//! radius law's three, `dogma/const.py`). The belt is the server's: no rock
//! is passed in the sandbox config.
//!
//! **No `[A]` value is an expected value here.** The assertions are that the
//! ore in the hold is the moon's ore and that a rock of it was taken, never a
//! count, a yield or a place the policy chose.

use super::common::{
    SYSTEM, VELDSPAR, config, dump, gm_only_in_setup, ok, pilot, rock, serve, until,
};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_server::celestial::{Catalogue, Universe};
use eve_server::content::Content;
use eve_server::typedata::{FIXTURE, Installed, Table};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const MINER: i64 = 90_000_001;
/// Invented: the moon, and its one ore.
const MOON: i64 = 40_000_555;
const MOON_ORE: i64 = 990_601;
/// `groupUbiquitousMoonAsteroids` [V].
const MOON_ORE_GROUP: i64 = 1884;

fn universe() -> Universe {
    let row = format!(
        "{SYSTEM}\t19055001\t29055001\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture home"
    );
    Universe::parse(
        &[row],
        &["19055001\t\t\t\t\tFixture region".to_owned()],
        &["29055001\t19055001\t\t\t\tFixture constellation".to_owned()],
    )
    .expect("the invented rows parse")
}

/// The ore: 10 m3 a unit, the radius law's three numbers invented.
fn typedata() -> Installed {
    let ore = format!(
        "type\t{MOON_ORE}\t{MOON_ORE_GROUP}\t25\t1\t100\t10.0\t0.0\t4000.0\t1.0\t0\t\t\
         182=3386.0;1980=0.75;1981=600.0;2699=1.0;2711={MOON_ORE}.0;2727=5000.0\tFixture moon ore\n"
    );
    Installed::build(Table::parse(&format!("{FIXTURE}\n{ore}")).expect("parses"))
}

fn start() -> SocketAddr {
    start_with(false)
}

/// `with_ordinary`: an ordinary Veldspar rock beside the station, nearer
/// than any moon rock (MN-G's preference test).
fn start_with(with_ordinary: bool) -> SocketAddr {
    let mut config = config();
    if with_ordinary {
        config.sandbox.rocks =
            eve_server::asteroid::Field::from_rocks([rock(0, 50_000, [2_000.0, 0.0, 0.0])]);
    }
    config.sandbox.characters = vec![pilot(MINER, "Miner1", 100_000_001)];
    // A moon 120 km out from the station at the origin: its belt rings it.
    config.sandbox.celestials = Catalogue::parse(&format!(
        "moon\t{MOON}\t990021\t1500000.0\t0.0\t0.0\t120000.0\t{SYSTEM}\n"
    ));
    let mut content = Content::fixture();
    content.insert("universe", universe());
    content.insert("typedata", typedata());
    config.content = Some(Arc::new(content));
    serve(config)
}

fn bot(addr: SocketAddr, name: &str) -> Handle {
    driver::spawn(BotConfig::new(name, addr, Pace::Fast)).0
}

/// **MN-C.** A bot in a system with one moon undocks and finds rocks the
/// server laid out round it, locks one, mines, and its hold holds the moon's
/// ore -- no structure, no chunk, nothing in the sandbox's rock field.
#[test]
fn a_bot_mines_a_moon_belt_rock_into_its_hold() {
    let addr = start();
    let miner = bot(addr, "Miner1");
    assert!(
        until(Duration::from_secs(120), || miner
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Mine))),
        "the laser starts on a moon rock:\n{}",
        dump(&miner)
    );
    let has_moon_ore = || {
        miner.observation().is_some_and(|o| {
            o.cargo
                .iter()
                .any(|r| r.type_id == MOON_ORE && r.quantity > 0)
        })
    };
    assert!(
        until(Duration::from_secs(120), has_moon_ore),
        "moon ore in the hold: {:?}\n{}",
        miner.observation().map(|o| o.cargo),
        dump(&miner)
    );
    gm_only_in_setup(&miner);
    miner.stop();
}

/// **MN-G, the miner's moon preference.** With an ordinary rock beside the
/// station, nearer than the moon belt, the same miner still takes the moon's
/// ore: a free moon belt rock beats an ordinary one
/// (`eve_bot::moon_pref::prefer_moon`, one line in `pick_rock`).
#[test]
fn a_miner_prefers_a_lit_moons_rock_to_an_ordinary_one() {
    let addr = start_with(true);
    let miner = bot(addr, "Miner1");
    let has = |ty: i64| {
        miner
            .observation()
            .is_some_and(|o| o.cargo.iter().any(|r| r.type_id == ty && r.quantity > 0))
    };
    assert!(
        until(Duration::from_secs(120), || has(MOON_ORE)),
        "moon ore in the hold: {:?}\n{}",
        miner.observation().map(|o| o.cargo),
        dump(&miner)
    );
    assert!(!has(VELDSPAR), "the ordinary rock was left alone");
    miner.stop();
}
