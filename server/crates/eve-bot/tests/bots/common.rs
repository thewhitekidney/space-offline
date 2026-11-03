//! The fixture every bot test shares: one in-process server, one poll loop,
//! and the plain-connection helpers a non-bot's setup uses.
//!
//! **Synthetic furniture only** (the no-client-data rule): the rocks here
//! have made-up ids, places and a fixture radius law; the one ore type is
//! Veldspar's id [V-data].

use eve_bot::Conn;
use eve_bot::brain::ActKind;
use eve_bot::driver::{Entry, Handle, Verdict};
use eve_marshal::PyValue;
use eve_server::server::{Config, Server};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The sandbox's home system, `Character::default()`'s.
pub const SYSTEM: i64 = 30_000_001;
/// Veldspar [V-data]: the ore every fixture rock is.
pub const VELDSPAR: i64 = 1230;
/// An invented ship type a test pilot reports to the fleet's `Init` and
/// `AcceptInvite`: the server only stores it on the member row, so no real
/// hull is needed.
pub const FLEET_SHIP: i64 = 900_000_001;
/// The first fixture rock's id; the next ones count up from it.
pub const ROCK: i64 = eve_server::asteroid::MIN_ROCK_ID;

/// A server config on a free loopback port, nothing else set.
pub fn config() -> Config {
    Config::new("127.0.0.1:0".parse().expect("literal"))
}

/// **The server fixture.** Binds `config`, serves every connection on a
/// thread of its own for the rest of the test binary, and returns where.
pub fn serve(config: Config) -> SocketAddr {
    serve_with_chat(config).0
}

/// [`serve`], and the port chat **actually** bound, when `config` turns it
/// on. Give chat port 0 and read it back here: probing a free port and
/// handing it over races every other test binding at once (tracker §83).
pub fn serve_with_chat(config: Config) -> (SocketAddr, Option<u16>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let chat = server.chat_addrs().first().map(SocketAddr::port);
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, chat)
}

/// A sandbox character in her capsule at the home station.
pub fn pilot(id: i64, name: &str, ship: i64) -> eve_server::model::Character {
    eve_server::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ..eve_server::model::Character::default()
    }
}

/// One Veldspar rock in the home system: `ROCK + i`, `quantity` units, at
/// `position`. A fixture law, not a claim about any ore.
pub fn rock(i: usize, quantity: i64, position: [f64; 3]) -> eve_server::asteroid::Rock {
    let law = eve_server::asteroid::Law {
        growth: 0.5,
        unit_size: 90.0,
        max_radius: 16_255.0,
    };
    eve_server::asteroid::Rock {
        id: ROCK + i as i64,
        type_id: VELDSPAR,
        belt_id: 40_000_003,
        solar_system_id: SYSTEM,
        quantity,
        radius: law.radius(quantity),
        position,
        law,
    }
}

/// Poll until `done` says so or `within` runs out; `false` on the timeout,
/// so the assertion at the call site can say what was waited for.
pub fn until(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

pub fn ok(e: &Entry, kind: ActKind) -> bool {
    e.kind == kind && e.verdict == Verdict::Ok
}

/// For a failing assertion: a bot's act log, one line per entry.
pub fn dump(h: &Handle) -> String {
    h.log()
        .iter()
        .map(|e| format!("  {:?} {:?} {} {:?}", e.at, e.kind, e.what, e.verdict))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Logged in and selected **by name**, the way the bots do it, as a plain
/// connection: a human stand-in. The token is the bots' default, never the
/// name (ruling 2, IP41).
pub fn login(addr: SocketAddr, name: &str) -> Conn {
    let mut c = Conn::connect(addr, Duration::from_secs(10)).expect("connects");
    c.handshake_as(eve_bot::driver::DEV_TOKEN)
        .expect("handshake");
    c.select_character_named(name).expect("selects by name");
    c
}

/// A non-bot's setup: a GM command, named at the call site.
pub fn gm(c: &mut Conn, line: &str) {
    c.call(
        "slash",
        "SlashCmd",
        vec![PyValue::Bytes(line.as_bytes().to_vec())],
    )
    .expect("the slash is answered");
}

/// **T9, as a property every bot test checks**: each `slash` in a bot's act
/// log comes before that bot's first `Undock`.
pub fn gm_only_in_setup(h: &Handle) {
    let log = h.log();
    if let Some(at) = log.iter().position(|e| e.kind == ActKind::Undock) {
        assert!(
            !log[at..].iter().any(|e| e.kind == ActKind::Slash),
            "a slash after the first undock:\n{}",
            dump(h)
        );
    }
}
