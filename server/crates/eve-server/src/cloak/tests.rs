//! The cloak's tests (lanes `cloak-1` and `cloak-2`). **Every id and number here is
//! invented**: a hull 9_100_001 that can cloak, cloaks 9_100_101/102, a 10 s
//! targeting delay, a 30 s reactivation delay, a 0.5 speed factor. None of
//! the module's `[A]` constants is asserted as a value: distances are placed
//! relative to [`MIN_CLOAKING_DISTANCE`] and the modes are compared with
//! themselves.

use super::*;
use crate::grid::tests::{
    ALICE, ALICE_SHIP, BELT_A_AT, BOB, BOB_SHIP, SYSTEM, Sent, added_to, drain, entries, held,
    plus, put, sent_to, settle, undock, world_of,
};
use crate::modules::{activate, deactivate};

const T0: BlueTime = 133_000_000_000_000_000;
const SEC: i64 = 1_000 * MSEC;

const HULL: i64 = 9_100_001;
const CLOAK: i64 = 9_100_101;
const COVOPS: i64 = 9_100_102;
const BURNER: i64 = 9_100_103;
const PUMP: i64 = 9_100_104;
const M_CLOAK: i64 = 9_100_201;
const M_COVOPS: i64 = 9_100_202;
const M_BURNER: i64 = 9_100_203;
const M_PUMP: i64 = 9_100_204;
const HI_SLOT_0: i64 = 27;
const E_BURN: i64 = 9_100_901;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let module = |id: i64, effect: i64, attrs: &str, name: &str| {
        format!(
            "type\t{id}\t330\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{effect}\t12,16,{effect}\t\
             {attrs}\t{name}\n"
        )
    };
    let hull = format!(
        "type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
         76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;271=0.0;272=0.5;273=0.6;\
         274=0.8;479=600000.0;482=260.0;564=640.0;600=5.0;1137=3.0;1163=1.0;1281=1.0\tInvented Cloaker\n"
    );
    let rows = [
        crate::typedata::FIXTURE,
        &hull,
        &module(
            CLOAK,
            607,
            "306=0.5;560=10000.0;669=30000.0",
            "Invented Cloak",
        ),
        &module(
            COVOPS,
            980,
            "306=1.0;560=10000.0;669=30000.0",
            "Invented Covert Cloak",
        ),
        &module(BURNER, E_BURN, "73=10000.0", "Invented Burner"),
        &module(
            STABILIZER,
            E_BURN,
            "73=10000.0;3123=1.0",
            "Invented Stabilizer",
        ),
        &module(PUMP, E_BURN, "73=10000.0", "Invented Pump"),
        "effect\t607\t1\t0\t0\t0\t0\t0\t0\tfixtureCloak\teffects.Cloaking\t0\n",
        "effect\t980\t1\t0\t0\t0\t0\t0\t0\tfixtureCovert\teffects.CloakingCovertOps\t1\n",
        "effect\t9100901\t1\t0\t73\t0\t0\t0\t0\tfixtureBurn\teffects.Burn\t0\n",
        "modifier\t607\tshipID\tItemModifier\t37\t306\t4\t\t\n",
        "modifier\t980\tshipID\tItemModifier\t37\t306\t4\t\t\n",
    ]
    .concat();
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn pilot(id: i64, name: &str, ship: i64) -> crate::model::Character {
    crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ship_type_id: HULL,
        station_id: Some(crate::grid::tests::STATION),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    }
}

/// Alice carries the cloaks, a burner and a pump; Bob carries nothing. Both
/// are undocked (their parks are modelled), Alice 100 km from the rocks and
/// Bob 50 km from Alice.
fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let mut reg = world_of(vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
    ])
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items
        .iter_mut()
        .filter(|i| i.item_id == ALICE_SHIP || i.item_id == BOB_SHIP)
    {
        i.type_id = HULL;
        i.group_id = 25;
    }
    for (n, (id, ty)) in [
        (M_CLOAK, CLOAK),
        (M_COVOPS, COVOPS),
        (M_BURNER, BURNER),
        (M_PUMP, PUMP),
    ]
    .into_iter()
    .enumerate()
    {
        let t = crate::item::find_in(&content, ty).expect("a test type");
        items.push(crate::item::Item {
            item_id: id,
            type_id: ty,
            owner_id: ALICE,
            location_id: ALICE_SHIP,
            flag_id: HI_SLOT_0 + n as i64,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 9_100_900,
        ..crate::store::Snapshot::default()
    });
    for who in [ALICE, BOB] {
        undock(&mut reg, who);
    }
    put(&mut reg, ALICE, plus(BELT_A_AT, [0.0, 100_000.0, 0.0]));
    put(&mut reg, BOB, plus(BELT_A_AT, [50_000.0, 100_000.0, 0.0]));
    let _ = settle(&mut reg);
    reg
}

fn alice_at() -> [f64; 3] {
    plus(BELT_A_AT, [0.0, 100_000.0, 0.0])
}

fn go(reg: &mut Registry, module: i64, effect: &str, t: i64) -> Result<PyValue, ServiceError> {
    activate(reg, Some(ALICE), module, effect, None, 1000, t)
}

fn cloak_on(reg: &mut Registry, t: i64) {
    go(reg, M_CLOAK, "fixtureCloak", t).expect("the cloak engages");
}

/// The key of a raised `UserError`.
fn key(e: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let PyValue::Bytes(k) = &args[0] else {
        panic!("odd key")
    };
    String::from_utf8_lossy(k).into_owned()
}

fn dict(e: &ServiceError) -> Vec<(PyValue, PyValue)> {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let PyValue::Dict(d) = &args[1] else {
        panic!("odd dict")
    };
    d.clone()
}

fn names(out: &[Sent], who: i64) -> Vec<String> {
    out.iter()
        .filter(|(to, _, _)| to.contains(&who))
        .flat_map(|(_, n, a)| {
            if *n == "DoDestinyUpdate" {
                entries(a).into_iter().map(|(n, _)| n).collect()
            } else {
                vec![(*n).to_owned()]
            }
        })
        .collect()
}

fn fx_guids(out: &[Sent], who: i64) -> Vec<String> {
    sent_to(out, who)
        .into_iter()
        .filter(|(n, _)| n == "OnSpecialFX")
        .filter_map(|(_, a)| match &a[5] {
            PyValue::Bytes(g) => Some(String::from_utf8_lossy(g).into_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_cloak_engages_for_its_owner_and_vanishes_for_the_observer() {
    let mut reg = world();
    assert!(held(&reg, BOB).contains(&ALICE_SHIP), "Bob sees Alice");
    cloak_on(&mut reg, T0);
    let out = drain(&mut reg);
    // Alice: the ball call, then the shimmer; her HUD gets a start with no
    // cycle (-1); the module's own guid is not pushed to anyone.
    let alice = sent_to(&out, ALICE);
    let cloaked: Vec<_> = alice.iter().filter(|(n, _)| n == "CloakBall").collect();
    assert_eq!(cloaked.len(), 1);
    assert_eq!(
        cloaked[0].1,
        vec![
            PyValue::Int(ALICE_SHIP),
            PyValue::Int(i64::from(MODE_NORMAL))
        ]
    );
    assert_eq!(fx_guids(&out, ALICE), vec!["effects.Cloak".to_owned()]);
    let godma = out
        .iter()
        .find(|(to, n, a)| {
            *n == "OnGodmaShipEffect" && to.contains(&ALICE) && a[3] == PyValue::Int(1)
        })
        .expect("the start");
    assert_eq!(godma.2[7], PyValue::Int(-1), "no ramp");
    // Bob: exactly the ball call, and the ball is out of his park.
    assert_eq!(
        names(&out, BOB)
            .iter()
            .filter(|n| *n == "CloakBall")
            .count(),
        1
    );
    assert!(fx_guids(&out, BOB).is_empty());
    assert!(!held(&reg, BOB).contains(&ALICE_SHIP));
    assert!(
        held(&reg, ALICE).contains(&ALICE_SHIP),
        "the owner keeps hers"
    );
    // The 306 factor halved the top speed, told to Alice by SetMaxSpeed.
    assert!(
        alice.iter().any(|(n, a)| n == "SetMaxSpeed"
            && matches!(a[1], PyValue::Real(v) if (v - 177.5).abs() < 0.01)),
        "{alice:?}"
    );
}

#[test]
fn the_reconcile_does_not_put_the_cloaked_ship_back() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let _ = drain(&mut reg);
    let out = settle(&mut reg);
    assert!(!added_to(&out, BOB).contains(&ALICE_SHIP));
    assert!(!held(&reg, BOB).contains(&ALICE_SHIP));
    // A fresh arrival's state does not hold her either.
    let set = crate::grid::grid_of(&reg, SYSTEM, BOB_SHIP, plus(alice_at(), [0.0, 10.0, 0.0]));
    assert!(!set.contains(&ALICE_SHIP));
}

#[test]
fn nobody_locks_a_cloaked_ship_and_a_cloaked_pilot_locks_nothing() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let e = crate::targeting::lock(&mut reg, BOB, ALICE_SHIP).expect_err("target cloaked");
    assert_eq!(key(&e), "DeniedTargetingTargetCloaked");
    let e = crate::targeting::lock(&mut reg, ALICE, BOB_SHIP).expect_err("locker cloaked");
    assert_eq!(key(&e), "DeniedTargetingCloaked");
}

#[test]
fn a_ship_that_comes_within_range_decloaks_it_and_gets_it_back() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let _ = drain(&mut reg);
    // Far: nothing happens.
    tick(&mut reg, Instant::now(), T0 + SEC);
    assert!(is_cloaked(&reg, ALICE_SHIP));
    // Surface to surface well inside the limit.
    put(
        &mut reg,
        BOB,
        plus(alice_at(), [MIN_CLOAKING_DISTANCE / 2.0, 0.0, 0.0]),
    );
    tick(&mut reg, Instant::now(), T0 + 2 * SEC);
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    assert!(
        reg.modules().running(M_CLOAK).is_none(),
        "the module stopped"
    );
    let out = drain(&mut reg);
    let alice = names(&out, ALICE);
    assert!(alice.contains(&"UncloakBall".to_owned()), "{alice:?}");
    assert_eq!(fx_guids(&out, ALICE), vec!["effects.Uncloak".to_owned()]);
    let forced = out
        .iter()
        .find(|(to, n, _)| *n == "OnCloakModuleForceDeactivated" && to.contains(&ALICE))
        .expect("the forced-decloak push");
    assert_eq!(
        forced.2[0],
        crate::pyutil::bstr("CloakDeactivatedByProximityToType")
    );
    let PyValue::Dict(d) = &forced.2[1] else {
        panic!("a dict")
    };
    assert_eq!(
        crate::pyutil::pairs_get(d, b"proximityTypeID"),
        Some(&PyValue::Int(HULL))
    );
    // Bob gets her back, and the effect after the add.
    assert!(added_to(&out, BOB).contains(&ALICE_SHIP));
    assert!(held(&reg, BOB).contains(&ALICE_SHIP));
    let bob = names(&out, BOB);
    let add = bob.iter().position(|n| n == "AddBalls2").expect("an add");
    let fx = bob.iter().position(|n| n == "OnSpecialFX").expect("the fx");
    assert!(add < fx, "{bob:?}");
    assert_eq!(fx_guids(&out, BOB), vec!["effects.Uncloak".to_owned()]);
}

#[test]
fn after_a_decloak_the_targeting_delay_holds_the_lock_back() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    deactivate(&mut reg, Some(ALICE), M_CLOAK, "fixtureCloak", T0 + 2 * SEC).expect("deactivates");
    // 560 = 10 s from the stop.
    assert_eq!(
        lock_refusal(&reg, ALICE_SHIP, BOB_SHIP, T0 + 7 * SEC),
        Some("DeniedTargetAfterCloak")
    );
    assert_eq!(
        lock_refusal(&reg, ALICE_SHIP, BOB_SHIP, T0 + 13 * SEC),
        None
    );
}

#[test]
fn a_manual_deactivate_ends_the_cloak_now_and_the_module_waits_its_reactivation_delay() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let _ = drain(&mut reg);
    deactivate(&mut reg, Some(ALICE), M_CLOAK, "fixtureCloak", T0 + 2 * SEC).expect("deactivates");
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    assert!(
        reg.modules().running(M_CLOAK).is_none(),
        "not at a boundary"
    );
    let out = drain(&mut reg);
    assert!(names(&out, ALICE).contains(&"UncloakBall".to_owned()));
    let stop = out
        .iter()
        .find(|(to, n, a)| {
            *n == "OnGodmaShipEffect" && to.contains(&ALICE) && a[3] == PyValue::Int(0)
        })
        .expect("the stop");
    assert_eq!(stop.2[7], PyValue::Int(-1));
    // 669 = 30 s from the stop.
    let e = go(&mut reg, M_CLOAK, "fixtureCloak", T0 + 10 * SEC).expect_err("too soon");
    assert_eq!(key(&e), "ModuleReactivationDelayed2");
    go(&mut reg, M_CLOAK, "fixtureCloak", T0 + 33 * SEC).expect("later");
    assert!(is_cloaked(&reg, ALICE_SHIP));
}

#[test]
fn the_activation_refusals() {
    // A lock held on us.
    let mut reg = world();
    reg.locks_mut().add(BOB_SHIP, ALICE_SHIP);
    let e = go(&mut reg, M_CLOAK, "fixtureCloak", T0).expect_err("targeted");
    assert_eq!(key(&e), "CantCloakTargeted");
    // Too close: the distance the message carries is the limit.
    let mut reg = world();
    put(&mut reg, BOB, plus(alice_at(), [1_000.0, 0.0, 0.0]));
    let e = go(&mut reg, M_CLOAK, "fixtureCloak", T0).expect_err("too near");
    assert_eq!(key(&e), "CantCloakProximity");
    let d = dict(&e);
    assert_eq!(
        crate::pyutil::pairs_get(&d, b"distance"),
        Some(&PyValue::Real(MIN_CLOAKING_DISTANCE))
    );
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    // A hull that cannot cloak.
    let mut reg = world();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
        i.type_id = 990_000_001;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 9_100_900,
        ..crate::store::Snapshot::default()
    });
    let e = go(&mut reg, M_CLOAK, "fixtureCloak", T0).expect_err("unsuitable");
    assert_eq!(key(&e), "CantCloakTooShipUnsuitable");
    // A second cloak.
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let e = go(&mut reg, M_COVOPS, "fixtureCovert", T0 + SEC).expect_err("two");
    assert_eq!(key(&e), "CantCloakTooManyDevices");
}

#[test]
fn another_module_stops_at_its_boundary_and_none_starts_while_cloaked() {
    let mut reg = world();
    go(&mut reg, M_BURNER, "fixtureBurn", T0).expect("the burner");
    cloak_on(&mut reg, T0 + SEC);
    let r = reg
        .modules()
        .running(M_BURNER)
        .copied()
        .expect("still running");
    assert_eq!(r.stop_at, Some(r.next_boundary()), "stops at its boundary");
    let e = go(&mut reg, M_PUMP, "fixtureBurn", T0 + 2 * SEC).expect_err("refused");
    assert_eq!(key(&e), "DeniedActivateCloaked");
}

#[test]
fn only_the_covert_cloak_warps_and_a_cloaked_ship_does_not_jump_dock_or_log_off() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    assert_eq!(
        warp_guard(&reg, ALICE).map(|r| r.user_error),
        Some("CantWarpWhileCloaked")
    );
    assert_eq!(
        jump_guard(&reg, ALICE).map(|r| r.user_error),
        Some("CantJumpWhileCloaked")
    );
    assert!(warp_guard(&reg, BOB).is_none() && jump_guard(&reg, BOB).is_none());
    let e = refuse_dock(&reg, Some(ALICE)).expect_err("no dock");
    assert_eq!(key(&e), "CantDockWhileCloaked");
    let out = crate::safe_logoff::start(&mut reg, Some(ALICE), T0).expect("answers");
    let PyValue::List(labels) = out else {
        panic!("a list")
    };
    assert!(!labels.is_empty(), "the cloak is a failed condition");

    let mut reg = world();
    go(&mut reg, M_COVOPS, "fixtureCovert", T0).expect("covert");
    assert!(warp_guard(&reg, ALICE).is_none(), "the covert cloak warps");
    // Active warp past a ball at 1 km: it does not decloak.
    reg.roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .mode = crate::destiny::Mode::Warp {
        goto: [1.0e9, 0.0, 0.0],
        effect_stamp: 1,
        total_distance: 1.0e9,
        stop_distance: 0.0,
        warp_factor: 4_000,
    };
    put_beside(&mut reg);
    tick(&mut reg, Instant::now(), T0 + SEC);
    assert!(is_cloaked(&reg, ALICE_SHIP));
}

/// Bob at 1 km of Alice, leaving Alice's motion alone.
fn put_beside(reg: &mut Registry) {
    let at = reg.roster().get(ALICE).expect("in space").motion.position;
    let p = reg.roster_mut().get_mut(BOB).expect("in space");
    p.motion.position = plus(at, [1_000.0, 0.0, 0.0]);
    p.since = crate::space_state::now_seconds();
}

#[test]
fn a_cloaked_ship_is_on_nobody_elses_dscan() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let seen_by_bob = crate::scan::balls_in_system(&mut reg, SYSTEM, BOB);
    assert!(!seen_by_bob.iter().any(|s| s.id == ALICE_SHIP));
    let seen_by_alice = crate::scan::balls_in_system(&mut reg, SYSTEM, ALICE);
    assert!(seen_by_alice.iter().any(|s| s.id == ALICE_SHIP));
}

#[test]
fn the_ego_ball_carries_the_cloak_byte_and_it_round_trips() {
    let mut reg = world();
    let motion = reg.roster().get(ALICE).expect("in space").motion;
    let before = crate::destiny::pilot_ball(&reg, ALICE_SHIP, HULL, 0, 0, &motion);
    assert_eq!(before.cloak, 0);
    cloak_on(&mut reg, T0);
    let ball = crate::destiny::pilot_ball(&reg, ALICE_SHIP, HULL, 0, 0, &motion);
    assert_eq!(ball.cloak, MODE_NORMAL);
    let park = crate::destiny::Park {
        current_time: 1,
        balls: vec![ball],
    };
    let again = crate::destiny::Park::decode(&park.encode()).expect("decodes");
    assert_eq!(again.balls[0].cloak, MODE_NORMAL);
}

#[test]
fn a_cloak_whose_module_is_gone_ends_and_one_whose_ship_left_is_forgotten() {
    let mut reg = world();
    cloak_on(&mut reg, T0);
    reg.modules_mut().forget(M_CLOAK);
    tick(&mut reg, Instant::now(), T0 + SEC);
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    assert!(recalibrating(&reg, ALICE_SHIP, T0 + 2 * SEC));
    // Gone from space: forgotten, nothing pushed.
    let mut reg = world();
    cloak_on(&mut reg, T0);
    let _ = drain(&mut reg);
    let _ = reg.roster_mut().leave(ALICE);
    tick(&mut reg, Instant::now(), T0 + SEC);
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    assert!(drain(&mut reg).is_empty());
}

#[test]
fn a_follower_of_a_cloaked_ship_is_stopped_in_the_model() {
    let mut reg = world();
    let bob = reg.roster_mut().get_mut(BOB).expect("in space");
    bob.motion.after(crate::destiny::Action::FollowBall {
        ball: BOB_SHIP,
        target: ALICE_SHIP,
        range: 500.0,
    });
    cloak_on(&mut reg, T0);
    assert_eq!(
        reg.roster().get(BOB).expect("in space").motion.mode,
        crate::destiny::Mode::Stop
    );
}

// ---------------------------------------------------------------------------
// CLOAK-2: the gate cloak
// ---------------------------------------------------------------------------

/// A module with attribute 3123 (a stabilizer) and one without.
const STABILIZER: i64 = 9_100_105;

/// Bob in space, Alice arriving through a gate beside him, protection on.
fn arriving() -> (Registry, Vec<Sent>) {
    let mut reg = world_of(vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
    ])
    .with_content(std::sync::Arc::new(content()));
    crate::invuln::set_enabled(&mut reg, true);
    undock(&mut reg, BOB);
    put(&mut reg, BOB, alice_at());
    let _ = settle(&mut reg);
    crate::jump::arm_arrival(
        &mut reg,
        ALICE,
        SYSTEM,
        plus(alice_at(), [1_000.0, 0.0, 0.0]),
    );
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    let out = drain(&mut reg);
    (reg, out)
}

#[test]
fn a_gate_arrival_is_cloaked_with_a_timer_and_hidden_from_those_already_there() {
    let (mut reg, out) = arriving();
    let timer: Vec<_> = out
        .iter()
        .filter(|(to, n, _)| *n == "OnJumpCloakUpdated" && to.contains(&ALICE))
        .collect();
    assert_eq!(timer.len(), 1);
    assert_eq!(timer[0].2[0], PyValue::Int(ALICE_SHIP));
    assert_eq!(timer[0].2[2], PyValue::Int(JUMP_CLOAK_DURATION));
    assert!(!names(&out, ALICE).contains(&"OnInvulunOnUndockingUpdated".to_owned()));
    assert!(is_cloaked(&reg, ALICE_SHIP));
    let motion = reg.roster().get(ALICE).expect("in space").motion;
    let ball = crate::destiny::pilot_ball(&reg, ALICE_SHIP, HULL, 0, 0, &motion);
    assert_eq!(ball.cloak, MODE_RESTORE);
    assert!(
        !added_to(&out, BOB).contains(&ALICE_SHIP),
        "Bob gets no add"
    );
    let after = settle(&mut reg);
    assert!(
        !added_to(&after, BOB).contains(&ALICE_SHIP),
        "nor a reconcile"
    );
    // No device refusals: the gate cloak does not stop a warp or a dock.
    assert!(warp_guard(&reg, ALICE).is_none());
    assert!(refuse_dock(&reg, Some(ALICE)).is_ok());
}

#[test]
fn an_undock_still_gets_the_invulnerability_and_no_cloak() {
    let mut reg2 = world_of(vec![pilot(ALICE, "Alice", ALICE_SHIP)])
        .with_content(std::sync::Arc::new(content()));
    crate::invuln::set_enabled(&mut reg2, true);
    let out = undock(&mut reg2, ALICE);
    assert!(names(&out, ALICE).contains(&"OnInvulunOnUndockingUpdated".to_owned()));
    assert!(!is_cloaked(&reg2, ALICE_SHIP));
}

#[test]
fn a_movement_order_breaks_the_gate_cloak_and_the_observer_gets_the_ball() {
    let (mut reg, _) = arriving();
    assert!(break_jump(&mut reg, ALICE));
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    let out = drain(&mut reg);
    let names_a = names(&out, ALICE);
    assert!(names_a.contains(&"UncloakBall".to_owned()), "{names_a:?}");
    assert!(names_a.contains(&"OnJumpCloakCancelled".to_owned()));
    assert_eq!(fx_guids(&out, ALICE), vec!["effects.Uncloak".to_owned()]);
    assert!(added_to(&out, BOB).contains(&ALICE_SHIP));
    // No recalibration afterwards, and a second break finds nothing.
    assert!(!recalibrating(&reg, ALICE_SHIP, 0));
    assert!(!break_jump(&mut reg, ALICE));
}

#[test]
fn the_gate_cloak_runs_out_at_its_time_and_a_leaver_is_forgotten() {
    let (mut reg, _) = arriving();
    let Kind::Jump { until } = reg.ext::<State>().by_ship[&ALICE_SHIP].kind else {
        panic!("a gate cloak");
    };
    tick(&mut reg, Instant::now(), until - SEC);
    assert!(is_cloaked(&reg, ALICE_SHIP));
    tick(&mut reg, Instant::now(), until);
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    let names_a = names(&drain(&mut reg), ALICE);
    assert!(names_a.contains(&"OnJumpCloakCancelled".to_owned()));
    // A ship that left space is forgotten with nothing pushed.
    let (mut reg, _) = arriving();
    let _ = reg.roster_mut().leave(ALICE);
    tick(&mut reg, Instant::now(), T0);
    assert!(!is_cloaked(&reg, ALICE_SHIP));
    assert!(drain(&mut reg).is_empty());
}

#[test]
fn a_module_breaks_the_gate_cloak_unless_it_carries_3123() {
    let (mut reg, _) = arriving();
    assert!(!break_for_module(&mut reg, ALICE, STABILIZER));
    assert!(is_cloaked(&reg, ALICE_SHIP));
    assert!(break_for_module(&mut reg, ALICE, BURNER));
    assert!(!is_cloaked(&reg, ALICE_SHIP));
}

#[test]
fn nobody_locks_a_gate_cloaked_ship_until_it_breaks() {
    let (mut reg, _) = arriving();
    assert!(
        lock_refusal(&reg, BOB_SHIP, ALICE_SHIP, T0).is_some(),
        "hidden"
    );
    assert!(break_jump(&mut reg, ALICE));
    assert!(lock_refusal(&reg, BOB_SHIP, ALICE_SHIP, T0).is_none());
}
