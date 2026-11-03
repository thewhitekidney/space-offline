//! K-LOGI over a real socket: a player's remote armor repairer heals another
//! player's damaged ship.
//!
//! Three pilots in one system. Alice shoots Bob once (one volley of an
//! invented round through the fixture autocannon, then she stops), so his
//! armour is damaged and his client is told the ring (attribute 266,
//! `armorDamage`, rising). Carol locks him and runs an invented remote armor
//! repairer carrying the client's own effect (`shipModuleRemoteArmorRepairer`,
//! 6188): Bob's client is told 266 **falling by the module's amount**, and
//! the number it ends at is the one the server's record holds (a second push
//! per cycle, the same step).
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s frigate and autocannon, an
//! invented round and an invented remote repairer. The numbers are made up
//! and none is an `[A]` value.

use super::common::{config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_311;
const BOB: i64 = 90_000_312;
const CAROL: i64 = 90_000_313;
const HOME: i64 = 30_000_001;
/// `typedata::FIXTURE`'s frigate and autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// Invented: a kinetic round and a remote armor repairer.
const ROUND: i64 = 990_590_101;
const REPAIRER: i64 = 990_590_102;
/// The round's kinetic damage: the autocannon's multiplier (2.5) makes the
/// raw volley 800, the frigate's shield (420 at 0.6 kinetic) takes 700 of
/// it and the armour the other 100.
const ROUND_DAMAGE: f64 = 320.0;
const FIRST_ARMOUR_DAMAGE: f64 = 100.0;
/// The repairer's `armorDamageAmount`.
const REPAIR_AMOUNT: f64 = 30.0;
/// The client's own effect [V]: name and id.
const REPAIR_EFFECT: (&str, i64) = ("shipModuleRemoteArmorRepairer", 6188);
/// `armorDamage` [V].
const ARMOR_DAMAGE: i64 = 266;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}\
         attribute\t84\t0.0\tarmorDamageAmount\n\
         attribute\t2044\t0.0\tfalloffEffectiveness\n\
         type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117={ROUND_DAMAGE};128=1.0\tFixture Logi Round\n\
         type\t{REPAIRER}\t325\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t6188\t12,16,6188\t6=0.0;30=1.0;50=1.0;54=30000.0;73=5000.0;84={REPAIR_AMOUNT};2044=0.0\tFixture Remote Armor Repairer\n\
         effect\t6188\t2\t0\t73\t6\t54\t2044\t0\t{}\teffects.RemoteArmourRepair\n",
        eve_server::typedata::FIXTURE,
        REPAIR_EFFECT.0,
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_311),
        pilot(BOB, "Bob", 100_000_312),
        pilot(CAROL, "Carol", 100_000_313),
    ];
    let content = content();
    config.content = Some(content.clone());
    // DOC-0: this test asserts the volley's exact armour share, so the gun
    // hits for the full amount (turret application is its own test).
    let server = eve_server::server::Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    server
        .world()
        .write()
        .expect("the world")
        .combat_mut()
        .set_turret_application(false);
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

/// Docked: create a fixture frigate and board it.
fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = *ids_of(c, FRIGATE)
        .last()
        .expect("the frigate is in the hangar");
    c.board(hull).expect("boards the frigate");
    hull
}

/// Docked, in a frigate: fit one module and online it.
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

/// Lock `target` and wait for it.
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

/// Every `armorDamage` change of `ship` pushed to `c` so far, `(new, old)`.
fn armour_changes(seen: &[(String, Vec<PyValue>)], ship: i64) -> Vec<(f64, f64)> {
    seen.iter()
        .filter(|(e, a)| {
            e == "OnModuleAttributeChange"
                && a.get(1) == Some(&PyValue::Int(ship))
                && a.get(2) == Some(&PyValue::Int(ARMOR_DAMAGE))
        })
        .filter_map(|(_, a)| match (a.get(4), a.get(5)) {
            (Some(PyValue::Real(n)), Some(PyValue::Real(o))) => Some((*n, *o)),
            _ => None,
        })
        .collect()
}

/// **Carol repairs Bob**: Alice's one volley damages the armour, Carol's
/// remote repairer takes it down by its amount a cycle.
#[test]
fn a_players_remote_armor_repairer_heals_another_players_ship() {
    let (addr, content) = start();

    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("undocks");
    bob.enter_space(HOME).expect("the park");

    let mut carol = login(addr, "Carol");
    carol.keep_pushes();
    board_frigate(&mut carol);
    let repairer = fit(&mut carol, REPAIRER);
    carol.undock().expect("undocks");
    carol.enter_space(HOME).expect("the park");

    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    board_frigate(&mut alice);
    let gun = fit(&mut alice, AUTOCANNON);
    gm(&mut alice, &format!("/createitem {ROUND} 10"));
    let station = alice.session().station_id().expect("docked");
    let rounds = ids_of(&mut alice, ROUND);
    alice
        .load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    alice.undock().expect("undocks");
    alice.enter_space(HOME).expect("the park");

    // One volley: Alice fires and stops, so the second cycle never starts.
    lock(&mut alice, bob_ship);
    alice
        .fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
        .expect("the gun fires");
    alice
        .deactivate(gun.item_id, "projectileFired")
        .expect("Deactivate is answered");

    let mut seen: Vec<(String, Vec<PyValue>)> = Vec::new();
    let hurt = until(Duration::from_secs(20), || {
        let _ = bob.collect_pushes(Duration::from_millis(200));
        seen.extend(bob.take_pushes());
        armour_changes(&seen, bob_ship).iter().any(|(n, o)| *n > *o)
    });
    assert!(
        hurt,
        "Bob's armour is told damaged: {:?}",
        seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    let damaged = armour_changes(&seen, bob_ship)
        .into_iter()
        .rfind(|(n, o)| n > o)
        .expect("a rising change")
        .0;
    assert_eq!(damaged, FIRST_ARMOUR_DAMAGE, "the volley's armour share");

    // Carol locks Bob and runs the repairer: the armour value falls.
    lock(&mut carol, bob_ship);
    carol
        .activate(repairer.item_id, REPAIR_EFFECT, Some(bob_ship), 1000)
        .expect("the remote repairer runs");
    let healed = until(Duration::from_secs(20), || {
        let _ = bob.collect_pushes(Duration::from_millis(200));
        seen.extend(bob.take_pushes());
        armour_changes(&seen, bob_ship).iter().any(|(n, o)| *n < *o)
    });
    assert!(healed, "Bob is told his armour damage fell");
    let first = armour_changes(&seen, bob_ship)
        .into_iter()
        .find(|(n, o)| n < o)
        .expect("a falling change");
    assert_eq!(
        first,
        (damaged - REPAIR_AMOUNT, damaged),
        "one cycle's step"
    );

    // It goes on: a second cycle takes a second step off.
    let again = until(Duration::from_secs(30), || {
        let _ = bob.collect_pushes(Duration::from_millis(200));
        seen.extend(bob.take_pushes());
        armour_changes(&seen, bob_ship)
            .iter()
            .any(|(n, _)| *n <= damaged - 2.0 * REPAIR_AMOUNT)
    });
    assert!(again, "the repairer cycles again");
}
