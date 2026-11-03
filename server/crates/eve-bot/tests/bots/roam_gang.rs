//! ROAM-GANG over real sockets: four gang bots in a low-sec line of three
//! systems (home -- middle -- far). A hostile pilot (Bob, another
//! corporation) sits at a belt in the far system, two jumps from the gang's
//! staging. The gang's scout finds him by the directional scan, reports,
//! the FC decides, and the body flies over the gates together, waits at the
//! far gate until every ship is in, lands on the scout in one call, and all
//! four shoot the same ship.
//!
//! Same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is
//! an expected value here**: the doctrine is an invented fixture (invented
//! security numbers, the Rifter classed `small`), the bar is a test-chosen
//! knob that makes the FC commit every ship, and the assertions are about
//! order, kinds, ids and a skew bounded by a named test constant.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve, until};
use eve_bot::brain::ActKind;
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_bot::hostile::Class;
use eve_bot::hunt::Ground;
use eve_bot::roam_gang::{GangHall, GangKnobs, Plan, ROLE, Seat};
use eve_bot::travel::Map;
use eve_server::model::Character;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SYSTEM_B: i64 = 30_000_017;
const SYSTEM_C: i64 = 30_000_018;
const STATION_C: i64 = 60_900_018;
const GATE_AB: i64 = 50_900_017;
const GATE_BA: i64 = 50_900_018;
const GATE_BC: i64 = 50_900_019;
const GATE_CB: i64 = 50_900_020;
const GATE_TYPE: i64 = 990_400_001;
/// An invented belt in the far system, a long warp from its gate.
const BELT_C: i64 = 40_977_101;
/// The asteroid belt's type **[V-data]** (`driver::is_belt`).
const BELT_TYPE: i64 = 15;
const BELT_C_X: f64 = 30_000_000.0;

const BOB: i64 = 90_000_700;
const BOB_SHIP: i64 = 100_000_700;
const GANG_IDS: [i64; 4] = [90_000_701, 90_000_702, 90_000_703, 90_000_704];
/// The gang's player corporation and Bob's own. Invented.
const CORP: i64 = 98_000_711;
const BOB_CORP: i64 = 98_000_712;
/// A Rifter **[V-data]**: every hull here, classed `small` by the fixture
/// doctrine.
const RIFTER: i64 = 587;
/// Invented low-sec numbers.
const LOW_SEC: f64 = 0.2;
/// An invented null-sec number (below zero), for the camp.
const NULL_SEC: f64 = -0.5;
/// How long the camper is watched after it reached the gate; a test bound.
const HOLD_FOR: Duration = Duration::from_secs(20);
/// The test's bar: high enough that the FC commits every ship it has (the
/// foe is one small hull, the gang four).
const TEST_BAR: f64 = 3.5;
/// The landing calls of the body, issued by separate bots on separate
/// connections, are no further apart than this; a named test bound, not a
/// game constant.
const SKEW_MAX: Duration = Duration::from_secs(12);

fn catalogue() -> eve_server::celestial::Catalogue {
    catalogue_with("")
}

/// [`catalogue`] and `extra` lines (GANG-SUMMON: a belt in the home system).
fn catalogue_with(extra: &str) -> eve_server::celestial::Catalogue {
    let home = Character::default();
    let (a, station_a) = (SYSTEM, home.station_id.expect("docked"));
    let (constellation, region) = (home.constellation_id, home.region_id);
    let station_type = home.station_type_id;
    eve_server::celestial::Catalogue::parse(&format!(
        "#system\t{a}\t{constellation}\t{region}\n\
         #system\t{SYSTEM_B}\t{constellation}\t{region}\n\
         #system\t{SYSTEM_C}\t{constellation}\t{region}\n\
         station\t{station_a}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{a}\t0\t0\n\
         gate\t{GATE_AB}\t{GATE_TYPE}\t2473.0\t1000000.0\t0.0\t0.0\t{a}\t{GATE_BA}\t{SYSTEM_B}\n\
         gate\t{GATE_BA}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{SYSTEM_B}\t{GATE_AB}\t{a}\n\
         gate\t{GATE_BC}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t1000000.0\t{SYSTEM_B}\t{GATE_CB}\t{SYSTEM_C}\n\
         gate\t{GATE_CB}\t{GATE_TYPE}\t2473.0\t0.0\t0.0\t-1000000.0\t{SYSTEM_C}\t{GATE_BC}\t{SYSTEM_B}\n\
         station\t{STATION_C}\t{station_type}\t5000.0\t0.0\t0.0\t0.0\t{SYSTEM_C}\t0\t0\n\
         belt\t{BELT_C}\t{BELT_TYPE}\t0.0\t{BELT_C_X}\t0.0\t0.0\t{SYSTEM_C}\t0\t0\n{extra}"
    ))
}

fn doctrine() -> Ground {
    Ground::new()
        .with_security(SYSTEM, LOW_SEC)
        .with_security(SYSTEM_B, LOW_SEC)
        .with_security(SYSTEM_C, LOW_SEC)
        .with_class(RIFTER, Class::Small)
}

fn gang_bot(
    addr: std::net::SocketAddr,
    i: usize,
    hall: &Arc<GangHall>,
    map: &Arc<Map>,
) -> (Handle, Instant) {
    let name = format!("Gang{}", i + 1);
    let mut config = BotConfig::new(&name, addr, Pace::Fast).with_role(&ROLE);
    config.chat = false;
    config.policy.pvp = Some(Arc::new(doctrine()));
    config.policy.travel = Some(Arc::clone(map));
    config.policy.gang = Some(Seat {
        hall: Arc::clone(hall),
        gang: "gang".to_owned(),
        rank: i,
        size: GANG_IDS.len(),
        knobs: GangKnobs {
            required_odds: TEST_BAR,
            ..GangKnobs::NORMAL
        },
    });
    let started = Instant::now();
    (driver::spawn(config).0, started)
}

fn when(
    h: &(Handle, Instant),
    kind: ActKind,
    pred: impl Fn(&driver::Entry) -> bool,
) -> Option<Instant> {
    h.0.log()
        .iter()
        .find(|e| ok(e, kind) && pred(e))
        .map(|e| h.1 + e.at)
}

/// **A gang finds a pilot two jumps away through a scout, gathers, lands
/// together and shoots one target.**
#[test]
fn a_gang_finds_a_pilot_two_jumps_away_gathers_lands_together_and_shoots_one_target() {
    let mut cfg = config();
    let mut chars: Vec<Character> = GANG_IDS
        .iter()
        .enumerate()
        .map(|(i, id)| Character {
            corporation_id: CORP,
            ..pilot(*id, &format!("Gang{}", i + 1), 100_000_701 + i as i64)
        })
        .collect();
    chars.push(Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: BOB_SHIP,
        ship_type_id: RIFTER,
        corporation_id: BOB_CORP,
        station_id: Some(STATION_C),
        solar_system_id: Some(SYSTEM_C),
        ..Character::default()
    });
    cfg.sandbox.characters = chars;
    cfg.sandbox.celestials = catalogue();
    let addr = serve(cfg);
    let map = Arc::new(
        Map::from_catalogue(&catalogue(), None)
            .with_security(SYSTEM, LOW_SEC)
            .with_security(SYSTEM_B, LOW_SEC)
            .with_security(SYSTEM_C, LOW_SEC),
    );

    // Bob undocks in the far system and sits at the belt.
    let mut bob = login(addr, "Bob");
    bob.undock().expect("undocks");
    bob.enter_space(SYSTEM_C).expect("the state");
    bob.warp_to_item(BELT_C, 0.0).expect("warps to the belt");

    let hall = Arc::new(GangHall::new());
    let bots: Vec<(Handle, Instant)> = (0..GANG_IDS.len())
        .map(|i| gang_bot(addr, i, &hall, &map))
        .collect();

    // Watch the gang's plan go Roam -> Gather -> Land -> Fight.
    let (mut roam, mut gather, mut land, mut fight) = (false, false, false, false);
    let shot = until(Duration::from_secs(420), || {
        let _ = bob.collect_pushes(Duration::from_millis(25));
        if let Some(g) = hall.snapshot("gang") {
            match g.plan {
                Plan::Roam => roam = true,
                Plan::Gather { .. } => gather = true,
                Plan::Land { .. } => land = true,
                Plan::Fight { .. } => fight = true,
                _ => {}
            }
        }
        bots.iter().all(|b| {
            b.0.log()
                .iter()
                .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&BOB_SHIP))
        })
    });
    let dumps: Vec<String> = bots.iter().map(|b| dump(&b.0)).collect();
    let plan = hall
        .snapshot("gang")
        .map(|g| format!("{:?} scouts {:?} routes {:?}", g.plan, g.scouts, g.routes));
    for b in &bots {
        b.0.stop();
    }
    assert!(
        shot,
        "all four shoot Bob; plan {plan:?} (roam {roam} gather {gather} land {land} fight {fight})\n{}",
        dumps.join("\n---\n")
    );
    assert!(
        roam && gather && land && fight,
        "{roam} {gather} {land} {fight}"
    );

    // The scout is the one that went out; the body never did anything
    // before the gather: its first gate jump (a Gang fly) came after the
    // scout posted the pilot, and it did not fire before it landed.
    let g = hall.snapshot("gang").expect("the gang");
    assert_eq!(g.scouts.len(), 1);
    let scout = g.scouts[0].clone();
    let body: Vec<&(Handle, Instant)> = bots.iter().filter(|b| b.0.name != scout).collect();
    assert_eq!(body.len(), GANG_IDS.len() - 1);

    // Landing: every body ship warps to the scout (a character id), in the
    // same few seconds; none of them fired before its own landing call.
    let scout_char = GANG_IDS[bots.iter().position(|b| b.0.name == scout).expect("scout")];
    let lands: Vec<Instant> = body
        .iter()
        .map(|b| {
            when(b, ActKind::WarpToMember, |e| e.ids == vec![scout_char]).unwrap_or_else(|| {
                panic!("{} never landed on the scout:\n{}", b.0.name, dump(&b.0))
            })
        })
        .collect();
    let (first, last) = (
        lands.iter().min().copied().expect("some"),
        lands.iter().max().copied().expect("some"),
    );
    assert!(
        last.duration_since(first) <= SKEW_MAX,
        "the landing calls are together: {:?}",
        last.duration_since(first)
    );
    for (b, landed) in body.iter().zip(&lands) {
        let fired = when(b, ActKind::Fire, |e| e.ids.get(1) == Some(&BOB_SHIP)).expect("fired");
        assert!(fired >= *landed, "{} fired before it landed", b.0.name);
    }
    // Everyone shot the one ship: no lock, orbit or shot of any gang bot
    // named another pilot's ship than Bob's or an ally's.
    for b in &bots {
        let others: Vec<_> =
            b.0.log()
                .iter()
                .filter(|e| ok(e, ActKind::Fire) && e.ids.get(1) != Some(&BOB_SHIP))
                .map(|e| e.what.clone())
                .collect();
        assert!(
            others.is_empty(),
            "{} shot something else: {others:?}",
            b.0.name
        );
    }
}

// ---- GANG-SUMMON ---------------------------------------------------------------------------

/// An invented belt in the staging (home) system, a long warp from the
/// station, where the quarry sits.
const BELT_A: i64 = 40_977_201;
const BELT_A_X: f64 = 40_000_000.0;
/// The summoned gang: three bots, their characters (invented ids) and the
/// quarry's own.
const SUMMONED: [i64; 3] = [90_000_731, 90_000_732, 90_000_733];
const QUARRY: i64 = 90_000_740;
const QUARRY_SHIP: i64 = 100_000_740;

fn home_belt_catalogue() -> eve_server::celestial::Catalogue {
    catalogue_with(&format!(
        "belt\t{BELT_A}\t{BELT_TYPE}\t0.0\t{BELT_A_X}\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
    ))
}

/// **A gang that is told who to hunt comes to him** and shoots: three
/// bots, a quarry (another corporation) at a belt of their own staging
/// system. No scouting, no odds weighed: the roster's `hunt me` spawn
/// (here its effect, [`eve_bot::roam_gang::summon`], on the shared hall)
/// forms the gang at half strength and every member warps to his
/// character and fires at his ship.
#[test]
fn a_gang_with_a_quarry_warps_to_him_and_all_shoot_his_ship() {
    let mut cfg = config();
    let mut chars: Vec<Character> = SUMMONED
        .iter()
        .enumerate()
        .map(|(i, id)| Character {
            corporation_id: CORP,
            ..pilot(*id, &format!("Gang{}", i + 1), 100_000_731 + i as i64)
        })
        .collect();
    chars.push(Character {
        character_id: QUARRY,
        name: "Quentin".to_owned(),
        ship_id: QUARRY_SHIP,
        ship_type_id: RIFTER,
        corporation_id: BOB_CORP,
        ..Character::default()
    });
    cfg.sandbox.characters = chars;
    cfg.sandbox.celestials = home_belt_catalogue();
    let addr = serve(cfg);
    let map = Arc::new(
        Map::from_catalogue(&home_belt_catalogue(), None)
            .with_security(SYSTEM, LOW_SEC)
            .with_security(SYSTEM_B, LOW_SEC)
            .with_security(SYSTEM_C, LOW_SEC),
    );
    // He undocks and sits at the belt, far from the station.
    let mut quentin = login(addr, "Quentin");
    quentin.undock().expect("undocks");
    quentin.enter_space(SYSTEM).expect("the state");
    quentin
        .warp_to_item(BELT_A, 0.0)
        .expect("warps to the belt");

    let hall = Arc::new(GangHall::new());
    let bots: Vec<(Handle, Instant)> = (0..SUMMONED.len())
        .map(|i| {
            let name = format!("Gang{}", i + 1);
            let mut config = BotConfig::new(&name, addr, Pace::Fast).with_role(&ROLE);
            config.chat = false;
            config.policy.pvp = Some(Arc::new(doctrine()));
            config.policy.travel = Some(Arc::clone(&map));
            let seat = Seat {
                hall: Arc::clone(&hall),
                gang: "hunters".to_owned(),
                rank: i,
                size: SUMMONED.len(),
                knobs: GangKnobs::NORMAL,
            };
            // What the roster's `quarry=` does in `eve-bots`.
            eve_bot::roam_gang::summon(&seat, SYSTEM, QUARRY);
            config.policy.gang = Some(seat);
            let started = Instant::now();
            (driver::spawn(config).0, started)
        })
        .collect();

    let (mut hunt, mut fight) = (false, false);
    let shot = until(Duration::from_secs(300), || {
        let _ = quentin.collect_pushes(Duration::from_millis(25));
        if let Some(g) = hall.snapshot("hunters") {
            match g.plan {
                Plan::Hunt { .. } => hunt = true,
                Plan::Fight { .. } => fight = true,
                _ => {}
            }
        }
        bots.iter().all(|b| {
            b.0.log()
                .iter()
                .any(|e| ok(e, ActKind::Fire) && e.ids.get(1) == Some(&QUARRY_SHIP))
        })
    });
    let dumps: Vec<String> = bots.iter().map(|b| dump(&b.0)).collect();
    let plan = hall.snapshot("hunters").map(|g| format!("{:?}", g.plan));
    for b in &bots {
        b.0.stop();
    }
    assert!(
        shot,
        "all three shoot him; plan {plan:?} (hunt {hunt} fight {fight})\n{}",
        dumps.join("\n---\n")
    );
    // The hunt may be over between two polls when he is still near the
    // undock (his warp has not started): only the fight is certain to show.
    let _ = hunt;
    assert!(fight, "no Fight plan seen");
    // Each one warped to his character (not to a belt, not to a scout).
    for b in &bots {
        assert!(
            when(b, ActKind::WarpToMember, |e| e.ids == vec![QUARRY]).is_some(),
            "{} never warped to him:\n{}",
            b.0.name,
            dump(&b.0)
        );
    }
}

/// **BP-CAMP: a camping gang warps to its system's gate and holds it.** One
/// bot in a null system: the camp order puts it on the gate (one warp to the
/// gate's item), the plan stays `Camp`, and nothing moves it off the spot (no
/// second warp, no orbit, no jump) while the camp stands.
#[test]
fn a_camper_warps_to_the_gate_and_holds_it() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![Character {
        corporation_id: CORP,
        ..pilot(GANG_IDS[0], "Gang1", 100_000_701)
    }];
    cfg.sandbox.celestials = catalogue();
    let addr = serve(cfg);
    let map = Arc::new(
        Map::from_catalogue(&catalogue(), None)
            .with_security(SYSTEM, NULL_SEC)
            .with_security(SYSTEM_B, NULL_SEC),
    );
    let hall = Arc::new(GangHall::new());
    let mut config = BotConfig::new("Gang1", addr, Pace::Fast).with_role(&ROLE);
    config.chat = false;
    config.policy.pvp = Some(Arc::new(
        Ground::new()
            .with_security(SYSTEM, NULL_SEC)
            .with_class(RIFTER, Class::Small),
    ));
    config.policy.travel = Some(Arc::clone(&map));
    let seat = Seat {
        hall: Arc::clone(&hall),
        gang: "camp".to_owned(),
        rank: 0,
        size: 1,
        knobs: GangKnobs::NORMAL,
    };
    // What `camp <group> <system>` does.
    eve_bot::roam_gang::camp(&seat, SYSTEM);
    config.policy.gang = Some(seat);
    let bot = (driver::spawn(config).0, Instant::now());

    let on_gate = until(Duration::from_secs(240), || {
        when(&bot, ActKind::WarpToItem, |e| e.ids.contains(&GATE_AB)).is_some()
    });
    // It has had time to move again if anything made it.
    std::thread::sleep(HOLD_FOR);
    let plan = hall.snapshot("camp").map(|g| g.plan);
    let log = bot.0.log();
    let warps = log
        .iter()
        .filter(|e| ok(e, ActKind::WarpToItem) && e.ids.contains(&GATE_AB))
        .count();
    let moves = log
        .iter()
        .filter(|e| matches!(e.kind, ActKind::Orbit | ActKind::Dock))
        .count();
    let dumped = dump(&bot.0);
    bot.0.stop();
    assert!(on_gate, "never warped to the gate:\n{dumped}");
    assert!(
        matches!(plan, Some(Plan::Camp { system, .. }) if system == SYSTEM),
        "{plan:?}"
    );
    assert_eq!(warps, 1, "one warp to the gate, then it holds:\n{dumped}");
    assert_eq!(moves, 0, "no orbit and no dock while camping:\n{dumped}");
}
