//! ENGINE-FIX over a real socket: what the first arena run found wrong.
//!
//! 1. **A launcher fires.** `useMissiles` has effect category 1, so the
//!    bot's `weapon_effect` never named a launcher and a missile hull never
//!    shot. It names it now, `Conn::fire` starts it on a locked target, and a
//!    missile flies and lands (the shooter is told the hit).
//! 2. **A gun stops when its target is gone.** The cycle used to run on to
//!    its end (up to a gun's whole cycle) and refuse a restart with
//!    `EffectAlreadyActive2`; it ends at once now.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s frigate and ammo, an invented
//! launcher, missile and long-cycle gun. No `[A]` value is an expected value.

use super::common::{config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_411;
const BOB: i64 = 90_000_412;
const HOME: i64 = 30_000_001;
/// `typedata::FIXTURE`'s frigate and small round.
const FRIGATE: i64 = 990_000_001;
const FIXTURE_ROUND: i64 = 990_000_005;
/// Invented: a launcher, its missile (group `MISSILE_GROUP`), and a gun with
/// a cycle far longer than the test waits.
const LAUNCHER: i64 = 990_610_101;
const MISSILE: i64 = 990_610_102;
const SLOW_GUN: i64 = 990_610_103;
const MISSILE_GROUP: i64 = 990_611;
/// The gun's cycle, ms: longer than any wait below.
const SLOW_CYCLE_MS: i64 = 40_000;
/// `useMissiles` [V-data].
const USE_MISSILES: (&str, i64) = ("useMissiles", 101);
const CHARGE_GUID: &str = "effects.FixtureMissileLaunch";

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}\
         effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t\n\
         effect\t9\t2\t1\t0\t0\t0\t0\t0\tmissileLaunching\t{CHARGE_GUID}\n\
         type\t{LAUNCHER}\t507\t7\t1\t1\t5.0\t0.6\t0.0\t1.0\t101\t12,16,40,101\t\
         51=4000.0;56=1.0;604={MISSILE_GROUP}.0;1795=8000.0\tInvented Launcher\n\
         type\t{MISSILE}\t{MISSILE_GROUP}\t8\t1\t100\t0.015\t0.0\t700.0\t300.0\t9\t9\t\
         37=3000.0;70=0.0001;114=10.0;117=20.0;281=5000.0;653=100.0;654=40.0\tInvented Missile\n\
         type\t{SLOW_GUN}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
         6=0.0;30=2.0;50=4.0;51={SLOW_CYCLE_MS}.0;54=100000.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=83.0\tInvented Slow Gun\n",
        eve_server::typedata::FIXTURE,
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_411),
        pilot(BOB, "Bob", 100_000_412),
    ];
    let content = content();
    config.content = Some(content.clone());
    let server = eve_server::server::Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, content)
}

fn ids_of(c: &mut Conn, type_id: i64) -> Vec<i64> {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.item_id)
        .collect()
}

fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = *ids_of(c, FRIGATE)
        .last()
        .expect("the frigate is in the hangar");
    c.board(hull).expect("boards the frigate");
    hull
}

fn fit(c: &mut Conn, type_id: i64) -> eve_bot::ItemRow {
    gm(c, &format!("/createitem {type_id}"));
    let spare = ids_of(c, type_id);
    let module = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .expect("the module is fitted");
    if !c.is_online(module.item_id).expect("asks") {
        c.set_online(module.item_id).expect("onlines");
    }
    module
}

fn load(c: &mut Conn, module: &eve_bot::ItemRow, round: i64) {
    gm(c, &format!("/createitem {round} 20"));
    let station = c.session().station_id().expect("docked");
    let rounds = ids_of(c, round);
    c.load_ammo(&[(module.item_id, module.flag_id)], &rounds, round, station)
        .expect("loads");
}

fn lock(c: &mut Conn, target: i64) {
    let mut state = c.lock_target(target).expect("AddTarget is answered");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while state == eve_bot::LockState::Pending && std::time::Instant::now() < deadline {
        let pushes = c.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, eve_bot::Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(target)))
        }) {
            state = eve_bot::LockState::Locked;
        }
    }
    assert_eq!(state, eve_bot::LockState::Locked, "the lock completes");
}

/// Both pilots in space in frigates; Alice carries `module` loaded with `round`.
fn duel(addr: SocketAddr, module: i64, round: i64) -> (Conn, Conn, i64, eve_bot::ItemRow) {
    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("undocks");
    bob.enter_space(HOME).expect("the park");
    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    board_frigate(&mut alice);
    let gun = fit(&mut alice, module);
    load(&mut alice, &gun, round);
    alice.undock().expect("undocks");
    alice.enter_space(HOME).expect("the park");
    (alice, bob, bob_ship, gun)
}

/// **A bot's launcher fires and a missile hits.**
#[test]
fn a_launcher_is_a_weapon_and_its_missile_lands() {
    let (addr, content) = start();
    // The bot's table names the launcher by its default effect.
    assert_eq!(
        eve_bot::weapon_effect(Some(&content), LAUNCHER),
        Some(USE_MISSILES),
        "a launcher is a weapon"
    );
    assert_eq!(
        eve_bot::weapon_effect(Some(&content), FIXTURE_ROUND),
        None,
        "a round is not"
    );
    let (mut alice, _bob, bob_ship, launcher) = duel(addr, LAUNCHER, MISSILE);
    lock(&mut alice, bob_ship);
    alice
        .fire(launcher.item_id, LAUNCHER, Some(&content), bob_ship)
        .expect("the launcher fires");
    // The module's own row says it runs (what `GunInfo::active` reads).
    assert!(
        alice
            .active_effects(launcher.item_id)
            .expect("asks")
            .contains(&USE_MISSILES.1),
        "the launcher's row lists useMissiles while it runs"
    );
    let mut seen: Vec<(String, Vec<PyValue>)> = Vec::new();
    let hit = until(Duration::from_secs(30), || {
        let _ = alice.collect_pushes(Duration::from_millis(200));
        seen.extend(alice.take_pushes());
        seen.iter().any(|(n, _)| n == "OnDamageMessage")
    });
    assert!(
        hit,
        "the shooter is told a hit: {:?}",
        seen.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
}

/// **A gun whose target is gone keeps its cycle** (GUN-CYCLE #186, which
/// replaced ENGINE-FIX's "stops at once"): after an unlock the module stays
/// active until its cycle boundary, and firing again before then is refused,
/// so a kill or an unlock cannot skip the cycle. The restart after the
/// boundary is pinned by `modules::tests::a_lost_target_does_not_free_the_gun_before_the_cycle_ends`
/// (a 40 s fixture cycle is too long to wait out here).
#[test]
fn a_gun_keeps_its_cycle_when_its_target_is_unlocked() {
    let (addr, content) = start();
    let (mut alice, _bob, bob_ship, gun) = duel(addr, SLOW_GUN, FIXTURE_ROUND);
    let effect = eve_bot::weapon_effect(Some(&content), SLOW_GUN).expect("a gun");
    lock(&mut alice, bob_ship);
    alice
        .fire(gun.item_id, SLOW_GUN, Some(&content), bob_ship)
        .expect("the gun fires");
    assert!(
        alice
            .active_effects(gun.item_id)
            .expect("asks")
            .contains(&effect.1),
        "it runs"
    );
    alice.unlock_target(bob_ship).expect("unlocks");
    let stopped = until(Duration::from_secs(3), || {
        !alice
            .active_effects(gun.item_id)
            .expect("asks")
            .contains(&effect.1)
    });
    assert!(
        !stopped,
        "the cycle runs on after the unlock ({SLOW_CYCLE_MS} ms cycle)"
    );
    lock(&mut alice, bob_ship);
    assert!(
        alice
            .fire(gun.item_id, SLOW_GUN, Some(&content), bob_ship)
            .is_err(),
        "firing again mid-cycle is refused"
    );
}

// ---------------------------------------------------------------------------
// 3. A kiter holds its band against a closing brawler
// ---------------------------------------------------------------------------

/// Invented: the fixture frigate with a top speed far above its pursuer's.
const FAST_HULL: i64 = 990_610_201;
/// The kiter's top speed against the fixture frigate's 355 m/s.
const FAST_SPEED: f64 = 900.0;
/// The band the kiter is asked to hold, metres, and how far the brawler
/// starts from it (inside the band, so the kiter has to run).
const BAND_M: f64 = 12_000.0;
const START_APART_KM: f64 = 4.0;
/// The brawler asks to be this close.
const CLOSE_M: f64 = 1_000.0;
/// How long the chase runs, seconds.
const CHASE_S: u64 = 40;

fn kite_rows() -> String {
    // The fixture frigate's row with another id and a faster hull.
    let hull = format!(
        "type\t{FAST_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37={FAST_SPEED};48=125.0;55=150000.0;70=3.1;76=21000.0;\
         101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;271=0.0;272=0.5;273=0.6;274=0.8;479=600000.0;\
         482=260.0;564=640.0;600=5.0;1137=3.0;1281=1.0\tInvented Fast Frigate\n"
    );
    format!("{}{hull}", eve_server::typedata::FIXTURE)
}

/// The distance between two ships' balls in the server's own world, after
/// bringing the system to the present.
fn gap(world: &std::sync::RwLock<eve_server::Registry>, a: i64, b: i64) -> Option<f64> {
    let mut w = world.write().expect("the world");
    w.advance_system(HOME);
    let pa = w.roster().by_ship(a)?.motion.position;
    let pb = w.roster().by_ship(b)?.motion.position;
    Some(eve_server::space_state::distance(pa, pb))
}

/// **Kiter and brawler**: the kiter orbits the brawler at the band with its
/// throttle open and is faster; the brawler closes to 1 km, throttle open.
/// The server carries both between their calls (a pair is stepped together
/// on the tick grid), and the kiter ends the chase near its band and not
/// on top of the brawler.
#[test]
fn a_kiter_holds_its_band_against_a_closing_brawler() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_411),
        pilot(BOB, "Bob", 100_000_412),
    ];
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&kite_rows()).expect("the rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    config.content = Some(Arc::new(content));
    let server = eve_server::server::Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let board = |c: &mut Conn, hull_type: i64| {
        gm(c, &format!("/createitem {hull_type}"));
        let hull = *ids_of(c, hull_type).last().expect("the hull is there");
        c.board(hull).expect("boards");
        c.undock().expect("undocks");
        c.enter_space(HOME).expect("the park");
    };
    let mut brawler = login(addr, "Bob");
    board(&mut brawler, FRIGATE);
    let brawler_ship = brawler.session().ship_id().expect("a ship");
    let mut kiter = login(addr, "Alice");
    board(&mut kiter, FAST_HULL);
    let kiter_ship = kiter.session().ship_id().expect("a ship");
    gm(&mut brawler, "/tp 25");
    gm(&mut kiter, &format!("/tp {}", 25.0 + START_APART_KM));

    let start = gap(&world, kiter_ship, brawler_ship).expect("both are in space");
    assert!(start < BAND_M, "the kiter starts inside its band: {start}");

    // Each pilot calls every second, as the arena's pilots do; the brawler
    // closes, the kiter keeps the band.
    let until_t = std::time::Instant::now() + Duration::from_secs(CHASE_S);
    let mut first = true;
    while std::time::Instant::now() < until_t {
        if first {
            brawler.orbit(kiter_ship, CLOSE_M).expect("orbits");
            kiter.orbit(brawler_ship, BAND_M).expect("orbits");
            first = false;
        }
        brawler.set_speed_fraction(1.0).expect("throttle");
        kiter.set_speed_fraction(1.0).expect("throttle");
        std::thread::sleep(Duration::from_millis(1000));
    }
    let end = gap(&world, kiter_ship, brawler_ship).expect("both are in space");
    println!("kite: start {start:.0} m, after {CHASE_S} s {end:.0} m, band {BAND_M:.0} m");
    assert!(
        end > BAND_M * 0.6 && end < BAND_M * 1.4,
        "the kiter holds near its band, {end:.0} m of {BAND_M:.0} m"
    );
}
