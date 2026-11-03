//! SOC-C7 over a real socket: a director forms a fleet of two bots, warps
//! the fleet to a belt and calls a primary; both bots lock and shoot it.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **No `[A]` value is an expected value here.** The members are escort
//! bots at [`Pace::Fast`] that accept invites from their own corporation
//! (`AcceptFleet::Allied`, every sandbox pilot is in the default one); the
//! assertions are about ids and order -- who joined, who landed nearer the
//! belt than the station, whose lock is on which ship -- never a range or a
//! delay. Deadlines are generous ceilings.
//!
//! **The director and the target are plain connections**: the director is
//! `eve_bot::fleet::Director` on its own socket; "Bob" is a human stand-in
//! whose ship is the called primary (an NPC would be fought by an escort
//! uncalled, so it would prove nothing). The furniture is invented: one
//! station, one belt.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::{AcceptFleet, ActKind, Role};
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_bot::fleet::{Director, WarpTarget};
use eve_server::model::Character;
use std::net::SocketAddr;
use std::time::Duration;

const ESCORT1: i64 = 90_000_001;
const ESCORT2: i64 = 90_000_002;
const BOSS: i64 = 90_000_003;
const BOSS_SHIP: i64 = 100_000_003;
const BOB: i64 = 90_000_004;
const BOB_SHIP: i64 = 100_000_004;
/// A Rifter **[V-data]**: the director's and Bob's hull.
const RIFTER: i64 = 587;
/// An invented belt, a warp from the station.
const BELT: i64 = 40_977_001;
/// The asteroid belt's type **[V-data]** (`driver::is_belt`).
const BELT_TYPE: i64 = 15;
/// How far the belt is from the station at the origin, metres: invented.
const BELT_X: f64 = 50_000_000.0;
/// A nearer belt, the other way: where a bossless escort goes ratting
/// (the nearest belt) before it is in a fleet. Invented.
const DECOY: i64 = 40_977_002;
const DECOY_X: f64 = 10_000_000.0;

fn home() -> Character {
    Character::default()
}

/// The home system's station and one belt: ten columns, as
/// `tools/celestials.py` writes them.
fn catalogue() -> eve_server::celestial::Catalogue {
    let h = home();
    let system = h.solar_system_id.expect("the default pilot is in a system");
    let station = h.station_id.expect("the default pilot is docked");
    let (constellation, region, station_type) =
        (h.constellation_id, h.region_id, h.station_type_id);
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{system}\t{constellation}\t{region}\n\
         station\t{station}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{system}\t0\t0\n\
         belt\t{BELT}\t{BELT_TYPE}\t0.0\t-{BELT_X}\t0.0\t0.0\t{system}\t0\t0\n\
         belt\t{DECOY}\t{BELT_TYPE}\t0.0\t{DECOY_X}\t0.0\t0.0\t{system}\t0\t0\n"
    ))
}

fn start() -> SocketAddr {
    let mut c = config();
    let rifter = |id, name, ship| Character {
        ship_type_id: RIFTER,
        ..pilot(id, name, ship)
    };
    c.sandbox.characters = vec![
        pilot(ESCORT1, "Escort1", 100_000_001),
        pilot(ESCORT2, "Escort2", 100_000_002),
        rifter(BOSS, "Boss", BOSS_SHIP),
        rifter(BOB, "Bob", BOB_SHIP),
    ];
    c.sandbox.celestials = catalogue();
    serve(c)
}

/// An escort that takes invites only from its own corporation or alliance.
fn member(addr: SocketAddr, name: &str) -> Handle {
    let mut config = BotConfig::new(name, addr, Pace::Fast).with_role(Role::Escort);
    config.policy.accept_fleet = AcceptFleet::Allied;
    driver::spawn(config).0
}

/// The member's ship is nearer the belt than the station, by its last
/// observation.
fn at_the_belt(h: &Handle) -> bool {
    h.observation().and_then(|o| o.park).is_some_and(|p| {
        !p.ego_warping
            && p.belts
                .iter()
                .find(|b| b.id == BELT)
                .is_some_and(|b| p.ego.surface_to(b) < BELT_X / 2.0)
    })
}

/// The member is beside its boss, out of warp: back from its ratting.
fn with_the_boss(h: &Handle) -> bool {
    h.observation().and_then(|o| o.park).is_some_and(|p| {
        !p.ego_warping && p.boss.is_some_and(|b| p.ego.surface_to(&b) < DECOY_X / 2.0)
    })
}

/// **The call can be heard**: `BroadcastToBubble` reaches only the pilots on
/// the sender's grid *at the instant it is sent*, and nothing resends it
/// (`fleet_broadcast.rs`). So the director's own view must show it landed
/// and out of warp, with both escorts and Bob out of warp within
/// [`eve_server::grid::GRID_ENTER`] of it. Waiting on a bot's own
/// observation is not enough: a bot is "at the belt" from halfway through its
/// warp, and the director may land after it, in which case the call reached
/// nobody and the bots never lock (the gate flake).
fn the_grid_is_assembled(director: &mut Director) -> bool {
    let world = director.conn.world_now();
    let Some(me) = world.ego_ball() else {
        return false;
    };
    if me.warp_commanded() {
        return false;
    }
    [ESCORT1, ESCORT2, BOB].iter().all(|c| {
        world.ship_of(*c).is_some_and(|b| {
            !b.warp_commanded()
                && eve_server::space_state::distance(me.motion.position, b.motion.position)
                    <= eve_server::grid::GRID_ENTER
        })
    })
}

fn locked(h: &Handle, target: i64) -> bool {
    h.log()
        .iter()
        .any(|e| ok(e, ActKind::Lock) && e.ids == vec![target])
}

/// A gun of the member's is on `target`.
fn fired(h: &Handle, target: i64) -> bool {
    h.log()
        .iter()
        .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&target))
}

/// **The fleet the player way**: the director creates a fleet and invites
/// two escorts, who accept (their own corporation); it fleet-warps them to
/// a belt, where Bob is; it calls Bob's ship primary and locks it; both
/// escorts lock it and shoot it -- and neither had any reason to before the call.
/// Then the director disbands and both bots are out of the fleet.
#[test]
fn a_director_forms_a_fleet_warps_it_to_a_belt_and_both_bots_lock_the_primary() {
    let addr = start();
    let bots = [member(addr, "Escort1"), member(addr, "Escort2")];
    for b in &bots {
        assert!(
            until(Duration::from_secs(90), || b
                .log()
                .iter()
                .any(|e| ok(e, ActKind::Undock))),
            "the bot sets itself up and undocks:\n{}",
            dump(b)
        );
    }

    // Bob's own setup: undock and fly to the belt.
    let mut bob = login(addr, "Bob");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");
    bob.warp_to_item(BELT, 0.0).expect("Bob warps to the belt");

    let mut boss = login(addr, "Boss");
    boss.undock().expect("the director undocks");
    boss.enter_space(SYSTEM).expect("the director's state");
    let mut director = Director::new(boss, RIFTER);

    assert!(
        director
            .form_fleet(&[ESCORT1, ESCORT2], Duration::from_secs(60))
            .expect("the fleet forms"),
        "both bots joined: {:?}\n{}\n{}",
        director.command,
        dump(&bots[0]),
        dump(&bots[1])
    );
    let fleet = director.command.fleet_id.expect("a fleet");
    for b in &bots {
        assert!(
            until(Duration::from_secs(20), || b
                .observation()
                .and_then(|o| o.fleet)
                .is_some_and(|f| f.fleet_id == fleet)),
            "the bot is in the director's fleet:\n{}",
            dump(b)
        );
    }

    // Fleeted, each bot comes back from the nearer belt to its boss.
    for b in &bots {
        assert!(
            until(Duration::from_secs(120), || {
                let _ = director.pump(Duration::from_millis(50));
                with_the_boss(b)
            }),
            "the bot rejoins the director:\n{}",
            dump(b)
        );
    }
    director
        .fleet_warp_to(WarpTarget::Item(BELT))
        .expect("the fleet warp is accepted and the director warps");
    for b in &bots {
        assert!(
            until(Duration::from_secs(120), || {
                let _ = director.pump(Duration::from_millis(50));
                at_the_belt(b)
            }),
            "the fleet warp took the bot to the belt:\n{}",
            dump(b)
        );
        assert!(
            !b.log()
                .iter()
                .any(|e| e.kind == ActKind::WarpToItem && e.ids == vec![BELT]),
            "the server moved it; it never warped to the belt itself:\n{}",
            dump(b)
        );
    }
    assert!(
        !bots.iter().any(|b| locked(b, BOB_SHIP)),
        "nobody locks Bob before the call"
    );

    assert!(
        until(Duration::from_secs(120), || {
            let _ = director.pump(Duration::from_millis(50));
            the_grid_is_assembled(&mut director)
        }),
        "the director, both bots and Bob share a grid, out of warp"
    );
    director
        .call_primary(BOB_SHIP, Some(RIFTER))
        .expect("the director locks Bob and calls him primary");
    for b in &bots {
        assert!(
            until(Duration::from_secs(90), || {
                let _ = director.pump(Duration::from_millis(50));
                locked(b, BOB_SHIP) && fired(b, BOB_SHIP)
            }),
            "the bot locks the called primary and shoots it:\n{}",
            dump(b)
        );
    }

    director.disband().expect("the director disbands");
    for b in &bots {
        assert!(
            until(Duration::from_secs(20), || b
                .observation()
                .is_some_and(|o| o.fleet.is_none() && o.primary.is_none())),
            "the bot is out of the fleet and has no primary:\n{}",
            dump(b)
        );
        b.stop();
    }
    let _ = bob.collect_pushes(Duration::from_millis(10));
}
