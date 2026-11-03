//! FC-1 over real sockets: two `wing` bots follow a human FC -- a plain
//! connection, "Pilot" -- out of the station, through a stargate and into a
//! dock in the next system.
//!
//! Same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is an
//! expected value here**: the assertions are about order and kinds (a bot
//! undocks only after the FC does, both bots are on the FC's grid in the next
//! system, both end docked in the FC's station); every deadline is a generous
//! ceiling, not a claim about how fast a wing should be.
//!
//! The wing joins the way a person's friends do: the pilot forms a fleet and
//! lists it in the fleet finder (public, no approval); the bots apply and the
//! pilot's invite is accepted by `AcceptFleet::Fc`.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_bot::travel::Map;
use eve_bot::wing::{ROLE, Seat};
use eve_bot::wing_hall::WingHall;
use eve_bot::{Conn, Error};
use eve_marshal::PyValue;
use eve_server::model::Character;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SYSTEM_B: i64 = 30_000_027;
const STATION_B: i64 = 60_900_027;
const GATE_AB: i64 = 50_900_027;
const GATE_BA: i64 = 50_900_028;
const GATE_TYPE: i64 = 990_400_001;
/// Far enough from the gate that docking needs a warp.
const STATION_B_X: f64 = 6_000_000.0;

const PILOT: i64 = 90_000_801;
const PILOT_SHIP: i64 = 100_000_801;
const WINGS: [i64; 2] = [90_000_802, 90_000_803];
/// A Rifter **[V-data]**: the pilot's hull.
const RIFTER: i64 = 587;
/// `fleet_finder::INVITE_PUBLIC`: an advert everybody sees.
const PUBLIC: i64 = 8;
/// A bound the test sets for "with the FC": generous, not a claim.
const NEAR: f64 = 10_000.0;

fn catalogue() -> eve_server::celestial::Catalogue {
    let home = Character::default();
    let (a, station_a) = (SYSTEM, home.station_id.expect("docked"));
    let (constellation, region) = (home.constellation_id, home.region_id);
    let station_type = home.station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         #system\t{SYSTEM_B}\t{constellation}\t{region}\n\
         station\t{station_a}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_AB}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_BA}\t{SYSTEM_B}\n\
         gate\t{GATE_BA}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{SYSTEM_B}\t{GATE_AB}\t{a}\n\
         station\t{STATION_B}\t{station_type}\t5000.0\t{STATION_B_X}\t0.0\t0.0\t{SYSTEM_B}\t0\t0\n"
    ))
}

fn wing_bot(addr: SocketAddr, i: usize, hall: &Arc<WingHall>, map: &Arc<Map>) -> (Handle, Instant) {
    let name = format!("Wing{}", i + 1);
    let mut config = BotConfig::new(&name, addr, Pace::Fast).with_role(&ROLE);
    config.chat = false;
    config.policy.travel = Some(Arc::clone(map));
    config.policy.quick = true;
    config.policy.accept_fleet = AcceptFleet::Fc(PILOT);
    config.policy.wing = Some(Seat::new(Arc::clone(hall), Some(PILOT)));
    let started = Instant::now();
    (driver::spawn(config).0, started)
}

/// Read the pilot's pushes until `done` holds or `within` runs out.
fn pump_until(c: &mut Conn, within: Duration, mut done: impl FnMut(&mut Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        let _ = c.pump(Duration::from_millis(100));
        if done(c) {
            return true;
        }
    }
    done(c)
}

/// The surface distance from the pilot to a character's ship, if both are in
/// the pilot's park.
fn gap_to(c: &mut Conn, who: i64) -> Option<f64> {
    let w = c.world_now();
    let ship = w.ship_of(who)?.ball_id;
    w.surface_distance(w.ego?, ship)
}

fn composition_all_docked_at(c: &mut Conn, station: i64) -> bool {
    match c.fleet_composition() {
        Ok(rows) => {
            rows.len() == 3
                && rows
                    .iter()
                    .all(|r| r.station == Some(station) && r.system == Some(SYSTEM_B))
        }
        Err(_) => false,
    }
}

/// Warp to `item`, then close in until `reach` of its surface.
fn go_to(c: &mut Conn, item: i64, reach: f64) {
    c.warp_to_item(item, 0.0).expect("warps");
    let near = pump_until(c, Duration::from_secs(120), |c| {
        let w = c.world_now();
        let warping = w.ego_ball().is_some_and(|b| b.warp_commanded());
        !warping
            && w.ego
                .and_then(|e| w.surface_distance(e, item))
                .is_some_and(|g| g < reach)
    });
    if !near {
        c.follow(item, 0.0).expect("approaches");
        assert!(
            pump_until(c, Duration::from_secs(120), |c| {
                let w = c.world_now();
                w.ego
                    .and_then(|e| w.surface_distance(e, item))
                    .is_some_and(|g| g < reach)
            }),
            "the pilot reaches {item}"
        );
    }
}

/// **A wing follows its FC out of the station, through a gate, and into a
/// dock.** The pilot lists a fleet; two wing bots apply, are invited and
/// join. Docked beside him they stay; he undocks and they do (after him); he
/// jumps and both arrive on his grid in the next system; he docks at that
/// system's station and both dock there.
#[test]
fn two_wing_bots_follow_their_fc_through_a_gate_and_into_a_dock() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        Character {
            ship_type_id: RIFTER,
            ..pilot(PILOT, "Pilot", PILOT_SHIP)
        },
        pilot(WINGS[0], "Wing1", 100_000_802),
        pilot(WINGS[1], "Wing2", 100_000_803),
        // Seeds the second station: the server docks only at stations a
        // character was seeded at (or a bundle's).
        Character {
            station_id: Some(STATION_B),
            solar_system_id: Some(SYSTEM_B),
            ..pilot(90_000_804, "Bystander", 100_000_804)
        },
    ];
    cfg.sandbox.celestials = catalogue();
    let addr = serve(cfg);
    let map = Arc::new(Map::from_catalogue(&catalogue(), None));

    // The FC forms a fleet and lists it for everyone, no approval.
    let mut fc = login(addr, "Pilot");
    fc.create_fleet(RIFTER).expect("a fleet");
    let advert = PyValue::Dict(vec![
        (
            PyValue::Int(PUBLIC),
            PyValue::Bytes(b"inviteScope".to_vec()),
        ),
        (
            PyValue::Bool(false),
            PyValue::Bytes(b"joinNeedsApproval".to_vec()),
        ),
        (
            PyValue::Unicode("Wing".to_owned()),
            PyValue::Bytes(b"fleetName".to_vec()),
        ),
    ]);
    fc.call("fleetProxy", "AddFleetFinderAdvert", vec![advert])
        .expect("listed");

    let hall = Arc::new(WingHall::new());
    let bots: Vec<(Handle, Instant)> = (0..WINGS.len())
        .map(|i| wing_bot(addr, i, &hall, &map))
        .collect();
    let dumps = |bots: &[(Handle, Instant)]| {
        bots.iter()
            .map(|b| dump(&b.0))
            .collect::<Vec<_>>()
            .join("\n--\n")
    };
    let stop = |bots: &[(Handle, Instant)]| bots.iter().for_each(|b| b.0.stop());

    // 1. They find the advert, apply, are invited and join: three members.
    let joined = pump_until(&mut fc, Duration::from_secs(240), |c| {
        c.fleet_composition().is_ok_and(|rows| rows.len() == 3)
    });
    assert!(joined, "both bots join the pilot's fleet\n{}", dumps(&bots));
    for b in &bots {
        assert!(
            b.0.log().iter().any(|e| ok(e, ActKind::Wing)),
            "each applied through the finder:\n{}",
            dump(&b.0)
        );
    }

    // 2. While he is docked in their station they stay docked, however long
    // their kit takes.
    until(Duration::from_secs(45), || {
        bots.iter()
            .all(|b| b.0.log().iter().any(|e| ok(e, ActKind::Online)))
    });
    let _ = fc.pump(Duration::from_millis(500));
    for b in &bots {
        assert!(
            !b.0.log().iter().any(|e| e.kind == ActKind::Undock),
            "no undock while the FC is docked here:\n{}",
            dump(&b.0)
        );
    }

    // 3. He undocks; they undock after him and stay with him.
    let undocked_at = Instant::now();
    fc.undock().expect("undocks");
    fc.enter_space(SYSTEM).expect("the state");
    let out = pump_until(&mut fc, Duration::from_secs(180), |_| {
        bots.iter()
            .all(|b| b.0.log().iter().any(|e| ok(e, ActKind::Undock)))
    });
    assert!(out, "both undock after the FC\n{}", dumps(&bots));
    for b in &bots {
        let first =
            b.0.log()
                .iter()
                .find(|e| ok(e, ActKind::Undock))
                .map(|e| b.1 + e.at);
        assert!(first >= Some(undocked_at), "after him, not before");
    }
    let near = pump_until(&mut fc, Duration::from_secs(120), |c| {
        WINGS
            .iter()
            .all(|w| gap_to(c, *w).is_some_and(|g| g < NEAR))
    });
    assert!(near, "both within {NEAR} m of the FC\n{}", dumps(&bots));

    // 4. He jumps; both follow and land on his grid in the next system.
    go_to(&mut fc, GATE_AB, eve_server::jump::MAX_JUMP_DISTANCE);
    fc.stargate_jump(GATE_AB, GATE_BA).expect("jumps");
    fc.enter_space(SYSTEM_B).expect("the state in B");
    let arrived = pump_until(&mut fc, Duration::from_secs(240), |c| {
        WINGS.iter().all(|w| gap_to(c, *w).is_some())
    });
    assert!(
        arrived,
        "both arrive in the FC's park in system B\n{}",
        dumps(&bots)
    );

    // 5. He docks in B; both dock there.
    go_to(&mut fc, STATION_B, eve_bot::DOCK_RANGE);
    match fc.dock(STATION_B) {
        Ok(_) => {}
        Err(Error::NotSent(why)) => panic!("the pilot cannot dock: {why}"),
        Err(e) => panic!("dock: {e}"),
    }
    let docked = until(Duration::from_secs(240), || {
        composition_all_docked_at(&mut fc, STATION_B)
    });
    let report = dumps(&bots);
    stop(&bots);
    assert!(docked, "all three are docked in the FC's station\n{report}");
}
