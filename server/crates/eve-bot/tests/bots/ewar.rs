//! K-EWAR over a real socket: a player tackles another pilot.
//!
//! Alice flies a frigate with an invented warp scrambler and an invented
//! stasis webifier; Bob flies a frigate in the same system. Alice locks Bob
//! and runs both modules on him. Then:
//!
//! - **Bob cannot warp**: `CmdWarpToStuff` is refused with `WarpScrambled`
//!   (the scrambler's 105 beats the hull's stability of 0);
//! - **Bob is slower**: his client is told `maxVelocity` (37) at the web's
//!   factor, a buff-bar icon `webify`, and a `SetMaxSpeed` in the system's
//!   destiny stream;
//! - **it is aggression**: Alice, a capsuleer who tackled a capsuleer, may
//!   not dock for it (`CantDockAfterAggression`, the weapons timer);
//! - **it ends with the cycle**: Alice stops the web, and Bob is told his
//!   speed back.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only**: `typedata::FIXTURE`'s frigate and two invented
//! modules carrying the client's own effect ids and names. The numbers
//! are made up and none is an `[A]` value.

use super::common::{config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_301;
const BOB: i64 = 90_000_302;
const HOME: i64 = 30_000_001;
/// `typedata::FIXTURE`'s frigate and its top speed (attribute 37).
const FRIGATE: i64 = 990_000_001;
const FRIGATE_SPEED: f64 = 355.0;
/// Invented modules: a scrambler (strength 3) and a web (-50 percent).
const SCRAMBLER: i64 = 990_580_101;
const WEB: i64 = 990_580_102;
/// The client's own effects [V]: names and ids.
const SCRAM_EFFECT: (&str, i64) = ("warpScrambleBlockMWDWithNPCEffect", 5934);
const WEB_EFFECT: (&str, i64) = ("remoteWebifierFalloff", 6426);

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}\
         attribute\t20\t1.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t105\t0.0\twarpScrambleStrength\n\
         attribute\t2044\t0.0\tfalloffEffectiveness\n\
         type\t{SCRAMBLER}\t52\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t5934\t13,16,5934\t6=0.0;30=1.0;50=1.0;54=30000.0;73=5000.0;105=3.0;1350=1.0\tFixture Scrambler\n\
         type\t{WEB}\t65\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t6426\t13,16,6426\t6=0.0;20=-50.0;30=1.0;50=1.0;54=30000.0;73=5000.0;2044=0.0\tFixture Web\n\
         effect\t5934\t2\t1\t73\t6\t54\t0\t0\t{}\teffects.WarpScramble\n\
         effect\t6426\t2\t1\t73\t6\t54\t2044\t0\t{}\teffects.ModifyTargetSpeed\n",
        eve_server::typedata::FIXTURE,
        SCRAM_EFFECT.0,
        WEB_EFFECT.0,
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_301),
        pilot(BOB, "Bob", 100_000_302),
    ];
    config.content = Some(content());
    serve(config)
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

/// Docked, in a frigate: fit one module and online it; its item id.
fn fit(c: &mut Conn, type_id: i64) -> i64 {
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
    module.item_id
}

/// Every push kept until one named `event` whose arguments satisfy `args`
/// has arrived, or twenty seconds.
fn until_push(
    c: &mut Conn,
    event: &str,
    args: impl Fn(&[PyValue]) -> bool,
) -> Vec<(String, Vec<PyValue>)> {
    let mut seen: Vec<(String, Vec<PyValue>)> = Vec::new();
    let found = until(Duration::from_secs(20), || {
        let _ = c.collect_pushes(Duration::from_millis(200));
        seen.extend(c.take_pushes());
        seen.iter().any(|(e, a)| e == event && args(a))
    });
    assert!(
        found,
        "no {event} with the wanted arguments: {:?}",
        seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    seen
}

/// `OnModuleAttributeChange(ownerID, itemKey, attributeID, time, new, old)`
/// for `attribute` of `ship`.
fn attribute_of(ship: i64, attribute: i64) -> impl Fn(&[PyValue]) -> bool {
    move |a| a.get(1) == Some(&PyValue::Int(ship)) && a.get(2) == Some(&PyValue::Int(attribute))
}

/// **Scram and web a pilot**: he cannot warp, he is told his speed fell, the
/// webber has a weapons timer, and Bob is told his speed back when the web
/// stops.
#[test]
fn a_player_scrams_and_webs_another_and_he_cannot_warp_and_is_slower() {
    let addr = start();

    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("undocks");
    bob.enter_space(HOME).expect("the park");

    let mut alice = login(addr, "Alice");
    alice.keep_pushes();
    board_frigate(&mut alice);
    let scrambler = fit(&mut alice, SCRAMBLER);
    let web = fit(&mut alice, WEB);
    let station = alice.session().station_id().expect("docked");
    alice.undock().expect("undocks");
    alice.enter_space(HOME).expect("the park");

    // Alice locks Bob (the lock takes its seconds), then runs both.
    let mut state = alice.lock_target(bob_ship).expect("AddTarget is answered");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while state == eve_bot::LockState::Pending && std::time::Instant::now() < deadline {
        let pushes = alice.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, eve_bot::Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(bob_ship)))
        }) {
            state = eve_bot::LockState::Locked;
        }
    }
    assert_eq!(state, eve_bot::LockState::Locked, "Alice locks Bob");
    alice
        .activate(scrambler, SCRAM_EFFECT, Some(bob_ship), 1000)
        .expect("the scrambler runs");
    alice
        .activate(web, WEB_EFFECT, Some(bob_ship), 1000)
        .expect("the web runs");

    // Bob is told: the icon, and his top speed halved.
    let seen = until_push(
        &mut bob,
        "OnModuleAttributeChange",
        attribute_of(bob_ship, 37),
    );
    let slowed = seen
        .iter()
        .find(|(e, a)| e == "OnModuleAttributeChange" && attribute_of(bob_ship, 37)(a))
        .map(|(_, a)| a.clone())
        .expect("the 37 push");
    assert_eq!(slowed[4], PyValue::Real(FRIGATE_SPEED * 0.5), "{slowed:?}");
    assert_eq!(slowed[5], PyValue::Real(FRIGATE_SPEED));
    let seen_all: Vec<(String, Vec<PyValue>)> = seen;
    assert!(
        seen_all
            .iter()
            .any(|(e, a)| e == "OnEwarStart"
                && a.get(3) == Some(&PyValue::Bytes(b"webify".to_vec()))),
        "the webify icon: {:?}",
        seen_all.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );

    // He cannot warp: the scram's strength beats his hull's stability.
    let refused = bob
        .bound_call_kw(
            "CmdWarpToStuff",
            vec![PyValue::Bytes(b"char".to_vec()), PyValue::Int(ALICE)],
            vec![(PyValue::Real(0.0), PyValue::Bytes(b"minRange".to_vec()))],
        )
        .expect("answered");
    assert_eq!(
        refused.refusal().as_deref(),
        Some("WarpScrambled"),
        "a scrambled ship cannot warp"
    );

    // Alice may not dock for it: the weapons timer is aggression.
    let key = "CantDockAfterAggression";
    match alice.dock(station) {
        Ok(o) => assert_eq!(o.refusal().as_deref(), Some(key), "a tackler may not dock"),
        Err(e) => assert!(e.to_string().contains(key), "a tackler may not dock: {e}"),
    }

    // The web stops with its cycle; Bob is told his speed back.
    alice
        .deactivate(web, WEB_EFFECT.0)
        .expect("Deactivate is answered");
    let seen = until_push(&mut bob, "OnModuleAttributeChange", |a| {
        attribute_of(bob_ship, 37)(a) && a.get(4) == Some(&PyValue::Real(FRIGATE_SPEED))
    });
    assert!(
        seen.iter().any(|(e, a)| e == "OnEwarEnd"
            && a.get(3) == Some(&PyValue::Bytes(b"webify".to_vec()))),
        "the webify icon ends"
    );
}
