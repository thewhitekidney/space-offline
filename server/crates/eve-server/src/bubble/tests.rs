//! The bubbles' tests. **Every id and number is invented**: types
//! 990,024,xxx, systems 30,024,xxx, a radius of twenty thousand metres and a
//! life of two minutes are round numbers. Group and effect ids appear by
//! name only, except where a row must carry them.

use super::*;
use crate::celestial::Universe;
use crate::modules::MSEC;
use crate::sov_hub::fixture as hubfx;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
const SYSTEM: i64 = 30_024_001;
const HIGH: i64 = 30_024_002;
const ALICE: i64 = 90_024_001;
const BOB: i64 = 90_024_002;
const CORP: i64 = 98_024_001;

/// Invented: the sphere's radius, metres, and the probe's life, ms.
const RADIUS: f64 = 20_000.0;
const LIFE_MS: i64 = 120_000;
/// Invented: the field generator's signature bloom, percent.
const BLOOM: f64 = 50.0;

const LAUNCHER: i64 = 990_024_001;
const PROBE: i64 = 990_024_002;
const FIELD_GEN: i64 = 990_024_003;
const SCRIPT_DISRUPT: i64 = 990_024_010;
const SCRIPT_SCRAMBLE: i64 = 990_024_011;
const NULLIFIER: i64 = 990_024_050;
const FRIGATE: i64 = 990_000_001;

const ALICE_LAUNCHER: i64 = 100_024_101;
const ALICE_PROBE: i64 = 100_024_102;
const ALICE_FIELD: i64 = 100_024_103;
/// Two more field generators, each with a focused script loaded.
const ALICE_POINT: i64 = 100_024_104;
const ALICE_POINT_SCRIPT: i64 = 100_024_105;
const ALICE_SCRAMBLER: i64 = 100_024_106;
const ALICE_SCRAMBLER_SCRIPT: i64 = 100_024_107;

/// `flagHiSlot0` **[V]**.
const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_024_000 + (who - ALICE) + 1
}

fn universe() -> Universe {
    let row = |id: i64, sec: f64| {
        format!("{id}\t19000001\t29000001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {id}")
    };
    Universe::parse(
        &[row(SYSTEM, -0.5), row(HIGH, 0.9)],
        &["19000001\t\t\t\t\tFixture region".to_owned()],
        &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
    )
    .expect("the invented rows parse")
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    c.insert("universe", universe());
    let rows = format!(
        "{}type\t{LAUNCHER}\t{GROUP_LAUNCHER}\t7\t1\t1\t50.0\t15.0\t0.0\t1.0\t101\t12,16,101\t\
         51=5000.0;56=1.0;604={GROUP_PROBE}.0;1074=1.0\tInvented Sphere Launcher\n\
         type\t{PROBE}\t{GROUP_PROBE}\t8\t1\t1\t5.0\t0.0\t1.0\t30.0\t0\t\t\
         {ATTR_RANGE}={RADIUS};{ATTR_LIFE}={LIFE_MS}.0\tInvented Probe\n\
         type\t{FIELD_GEN}\t899\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t3380\t12,16,3380\t\
         73=30000.0;{ATTR_RANGE}={RADIUS};554={BLOOM};604=908.0;105=2.0;1074=1.0\tInvented Field Generator\n\
         type\t{SCRIPT_DISRUPT}\t908\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t6849\t6849\t\tInvented Disruption Script\n\
         type\t{SCRIPT_SCRAMBLE}\t908\t8\t1\t1\t1.0\t0.0\t1.0\t1.0\t6848\t6848\t\tInvented Scrambling Script\n\
         type\t{NULLIFIER}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=300.0;70=3.1;76=21000.0;263=420.0;265=440.0;552=100.0;600=5.0;{ATTR_IMMUNE}=1.0\t\
         Invented Nullifier\n\
         attribute\t1974\t1.0\tgateScrambleStrength\n\
         effect\t101\t1\t0\t51\t0\t0\t0\t0\tuseMissiles\t\t0\n\
         effect\t3380\t1\t1\t73\t0\t103\t0\t0\twarpDisruptSphere\teffects.WarpDisruptFieldGenerating\t0\n\
         effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn row(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    owner: i64,
    flag_id: i64,
    quantity: i64,
) -> crate::item::Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    crate::item::Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: ship(owner),
        flag_id,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: t.category_id == 7,
    }
}

fn place(reg: &mut Registry, who: i64, system: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        system,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// Alice (a frigate with a sphere launcher loaded with two probes, and a
/// field generator) at the origin, Bob (a frigate) 5 km away, both in a
/// null system. Every push drained.
fn world() -> Registry {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        corporation_id: CORP,
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![pilot(ALICE, "Alice"), pilot(BOB, "Bob")],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if i.item_id == ship(ALICE) || i.item_id == ship(BOB) {
            i.type_id = FRIGATE;
            i.group_id = 25;
        }
    }
    items.extend([
        row(&content, ALICE_LAUNCHER, LAUNCHER, ALICE, HI_SLOT_0, 1),
        row(&content, ALICE_PROBE, PROBE, ALICE, HI_SLOT_0, 2),
        row(&content, ALICE_FIELD, FIELD_GEN, ALICE, HI_SLOT_0 + 1, 1),
        row(&content, ALICE_POINT, FIELD_GEN, ALICE, HI_SLOT_0 + 2, 1),
        row(
            &content,
            ALICE_POINT_SCRIPT,
            SCRIPT_DISRUPT,
            ALICE,
            HI_SLOT_0 + 2,
            1,
        ),
        row(
            &content,
            ALICE_SCRAMBLER,
            FIELD_GEN,
            ALICE,
            HI_SLOT_0 + 3,
            1,
        ),
        row(
            &content,
            ALICE_SCRAMBLER_SCRIPT,
            SCRIPT_SCRAMBLE,
            ALICE,
            HI_SLOT_0 + 3,
            1,
        ),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_025_000,
        ..crate::store::Snapshot::default()
    });
    reg.character_mut(ALICE).expect("a pilot").ship_type_id = FRIGATE;
    reg.character_mut(BOB).expect("a pilot").ship_type_id = FRIGATE;
    place(&mut reg, ALICE, SYSTEM, [0.0; 3]);
    place(&mut reg, BOB, SYSTEM, [5_000.0, 0.0, 0.0]);
    let _ = hubfx::pending(&mut reg);
    let _ = hubfx::due(&mut reg);
    reg
}

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

fn launch(reg: &mut Registry, t: i64) -> Result<PyValue, ServiceError> {
    crate::modules::activate(reg, Some(ALICE), ALICE_LAUNCHER, "useMissiles", None, 1, t)
}

fn field_on(reg: &mut Registry, t: i64) -> Result<PyValue, ServiceError> {
    crate::modules::activate(
        reg,
        Some(ALICE),
        ALICE_FIELD,
        "warpDisruptSphere",
        None,
        1,
        t,
    )
}

fn zone(centre: [f64; 3], radius: f64) -> Zone {
    Zone {
        centre,
        radius,
        source: Source::Probe {
            type_id: PROBE,
            ball_id: ID_BASE,
        },
    }
}

fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

// ---------------------------------------------------------------------------
// The probe
// ---------------------------------------------------------------------------

#[test]
fn a_launcher_sends_a_probe_that_holds_a_ship_inside_its_radius_for_its_life() {
    let mut reg = world();
    launch(&mut reg, T0).expect("activates");
    assert_eq!(probes(&reg).len(), 1);
    let p = probes(&reg)[0];
    assert_eq!(p.radius, RADIUS);
    assert_eq!(p.centre, [0.0; 3], "at the launching ship");
    assert_eq!(p.expiry, T0 + LIFE_MS * MSEC);
    assert!(
        (ID_BASE..ID_END).contains(&p.ball_id),
        "a ball id in our range"
    );
    // Bob at 5 km is inside: WarpDisrupted with the radius.
    let refusal = inside(&reg, BOB).expect("held");
    assert_eq!(refusal.user_error, WARP_DISRUPTED);
    assert!(
        refusal
            .args
            .iter()
            .any(|(v, k)| *k == crate::pyutil::bstr("range") && *v == PyValue::Real(RADIUS)),
        "the range is the sphere's: {:?}",
        refusal.args
    );
    // 30 km out is not.
    place(&mut reg, BOB, SYSTEM, [30_000.0, 0.0, 0.0]);
    assert!(inside(&reg, BOB).is_none());
    // The probe lives its two minutes and goes.
    place(&mut reg, BOB, SYSTEM, [5_000.0, 0.0, 0.0]);
    tick(&mut reg, T0 + LIFE_MS * MSEC - 1);
    assert!(inside(&reg, BOB).is_some(), "not yet");
    tick(&mut reg, T0 + LIFE_MS * MSEC);
    assert!(probes(&reg).is_empty());
    assert!(inside(&reg, BOB).is_none(), "the warp is accepted again");
}

#[test]
fn the_probe_is_a_local_ball_and_its_slim_carries_the_start_time() {
    let mut reg = world();
    launch(&mut reg, T0).expect("activates");
    let p = probes(&reg)[0];
    let balls = balls_in(&reg, SYSTEM);
    assert_eq!(balls.len(), 1);
    assert_eq!(balls[0].0.ball_id, p.ball_id);
    assert_eq!(balls[0].1.type_id, PROBE);
    assert!(balls_in(&reg, HIGH).is_empty());
    assert!(
        crate::grid::local_balls(&reg, SYSTEM)
            .iter()
            .any(|(id, _)| *id == p.ball_id),
        "the grid sees it"
    );
    let extras = slim_extras(&reg, &[p.ball_id, 1]);
    assert_eq!(
        extras,
        vec![(
            p.ball_id,
            vec![("warpDisruptionStartTime", PyValue::Int(T0))]
        )]
    );
    // And the scene a pilot binds to holds it.
    let scene = crate::destiny::scene(&reg, SYSTEM, 0, None, 1);
    assert!(
        scene.park.balls.iter().any(|b| b.ball_id == p.ball_id),
        "in scene"
    );
}

#[test]
fn a_launcher_launches_once_and_spends_one_round() {
    let mut reg = world();
    launch(&mut reg, T0).expect("activates");
    assert_eq!(
        crate::modules::loaded(reg.store(), ship(ALICE), HI_SLOT_0).map(|c| c.quantity),
        Some(1)
    );
    assert_eq!(cycles(&reg, LAUNCHER, 5), 1);
    assert_eq!(cycles(&reg, FIELD_GEN, 5), 5);
}

#[test]
fn bubbles_are_refused_in_empire_space_and_in_warp() {
    let mut reg = world();
    reg.character_mut(ALICE).expect("a pilot").solar_system_id = Some(HIGH);
    place(&mut reg, ALICE, HIGH, [0.0; 3]);
    let e = launch(&mut reg, T0).expect_err("refused");
    assert_eq!(key(&e), "CustomNotify");
    let e = field_on(&mut reg, T0).expect_err("refused");
    assert_eq!(key(&e), "CustomNotify");
    assert!(probes(&reg).is_empty());
}

#[test]
fn a_launcher_takes_only_probes() {
    let mut reg = world();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if i.item_id == ALICE_PROBE {
            i.type_id = FRIGATE;
            i.group_id = 25;
        }
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_025_000,
        ..crate::store::Snapshot::default()
    });
    assert!(on_activated(&mut reg, ALICE, LAUNCHER, Some(FRIGATE), T0).is_none());
    assert!(probes(&reg).is_empty());
    assert!(on_activated(&mut reg, ALICE, FIELD_GEN, Some(PROBE), T0).is_none());
}

#[test]
fn probe_ids_are_clear_of_every_other_range() {
    let ours = ID_BASE..ID_END;
    let clear = |lo: i64, hi: i64| hi <= ours.start || lo >= ours.end;
    assert!(clear(crate::moon_scan::ID_BASE, crate::moon_scan::ID_END));
    assert!(clear(crate::cyno::FIELD_ID_BASE, crate::cyno::FIELD_ID_END));
    assert!(clear(crate::missile::ID_BASE, crate::missile::ID_END));
    assert!(clear(
        crate::command_nodes::NODE_ID_BASE,
        crate::command_nodes::NODE_ID_END
    ));
    assert!(clear(crate::econ_exchange::NPC_ORDER_BASE, i64::MAX));
    assert!(clear(crate::site::INSTANCE_BASE, crate::site::INSTANCE_END));
}

#[test]
fn ids_wrap_inside_the_range_and_skip_a_live_probe() {
    let mut b = Bubbles::default();
    assert_eq!(b.mint(), ID_BASE);
    b.probes.push(Probe {
        ball_id: ID_BASE + 1,
        type_id: PROBE,
        owner: ALICE,
        corporation_id: CORP,
        system: SYSTEM,
        centre: [0.0; 3],
        radius: RADIUS,
        launched: 0,
        expiry: 1,
    });
    assert_eq!(b.mint(), ID_BASE + 2);
}

// ---------------------------------------------------------------------------
// The field
// ---------------------------------------------------------------------------

#[test]
fn a_running_field_holds_others_but_never_its_own_ship_and_blooms_the_signature() {
    let mut reg = world();
    let before = crate::dogma::hull(&reg, ship(ALICE))
        .expect("a sheet")
        .signature_radius;
    field_on(&mut reg, T0).expect("activates");
    let after = crate::dogma::hull(&reg, ship(ALICE))
        .expect("a sheet")
        .signature_radius;
    assert_eq!(after, before * (1.0 + BLOOM / 100.0));
    let refusal = inside(&reg, BOB).expect("Bob is inside the field");
    assert_eq!(refusal.user_error, WARP_SCRAMBLED_BY_FIELD);
    assert!(inside(&reg, ALICE).is_none(), "not its own ship");
    // The field moves with the ship: Alice flies 100 km off and Bob is free.
    place(&mut reg, ALICE, SYSTEM, [100_000.0, 0.0, 0.0]);
    assert!(inside(&reg, BOB).is_none());
    // And it ends with the module.
    place(&mut reg, ALICE, SYSTEM, [0.0; 3]);
    assert!(inside(&reg, BOB).is_some());
    crate::modules::deactivate(
        &mut reg,
        Some(ALICE),
        ALICE_FIELD,
        "warpDisruptSphere",
        T0 + 1,
    )
    .expect("deactivates");
    crate::modules::tick(&mut reg, T0 + 30_000 * MSEC + 1);
    assert!(inside(&reg, BOB).is_none(), "the field is down");
}

// ---------------------------------------------------------------------------
// Immunity
// ---------------------------------------------------------------------------

#[test]
fn a_bubble_immune_ship_is_neither_held_nor_dragged() {
    let mut reg = world();
    launch(&mut reg, T0).expect("activates");
    assert!(inside(&reg, BOB).is_some());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if i.item_id == ship(BOB) {
            i.type_id = NULLIFIER;
        }
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_025_000,
        ..crate::store::Snapshot::default()
    });
    reg.character_mut(BOB).expect("a pilot").ship_type_id = NULLIFIER;
    assert!(immune(&reg, ship(BOB)));
    assert!(inside(&reg, BOB).is_none(), "not held");
    place(&mut reg, BOB, SYSTEM, [-1.0e9, 0.0, 0.0]);
    // A bubble on his line, 100 km short of a destination at +x.
    assert!(drag(&reg, BOB, [-1.0e9 + 2.0e8, 0.0, 0.0], 0.0).is_none());
    assert!(!immune(&reg, ship(ALICE)));
}

// ---------------------------------------------------------------------------
// The drag
// ---------------------------------------------------------------------------

#[test]
fn a_warp_through_a_bubble_lands_at_its_near_edge() {
    let from = [-1.0e10, 0.0, 0.0];
    let gate = [1_000_000.0, 0.0, 0.0];
    let z = zone([900_000.0, 0.0, 0.0], RADIUS);
    let landed = drag_in(&[z], 1, from, gate).expect("dragged");
    assert!(close(landed, [880_000.0, 0.0, 0.0], 1.0), "{landed:?}");
}

#[test]
fn a_bubble_more_than_500_km_short_of_the_destination_does_not_drag() {
    let from = [-1.0e10, 0.0, 0.0];
    let gate = [1_000_000.0, 0.0, 0.0];
    let far = zone([400_000.0, 0.0, 0.0], RADIUS);
    assert!(drag_in(&[far], 1, from, gate).is_none());
    let beside = zone([900_000.0, 2.0 * RADIUS, 0.0], RADIUS);
    assert!(drag_in(&[beside], 1, from, gate).is_none(), "off the line");
    let behind = zone([2_000_000.0, 0.0, 0.0], RADIUS);
    assert!(drag_in(&[behind], 1, from, gate).is_none(), "past the gate");
}

#[test]
fn the_nearest_of_two_bubbles_drags_and_a_destination_inside_one_does_not() {
    let from = [-1.0e10, 0.0, 0.0];
    let gate = [1_000_000.0, 0.0, 0.0];
    let near = zone([700_000.0, 0.0, 0.0], RADIUS);
    let far = zone([900_000.0, 0.0, 0.0], RADIUS);
    let landed = drag_in(&[far, near], 1, from, gate).expect("dragged");
    assert!(close(landed, [680_000.0, 0.0, 0.0], 1.0), "{landed:?}");
    // A gate inside a bubble: the warp lands as usual.
    let on_gate = zone(gate, RADIUS);
    assert!(drag_in(&[on_gate], 1, from, gate).is_none());
}

#[test]
fn a_field_does_not_drag_its_own_ship() {
    let from = [-1.0e10, 0.0, 0.0];
    let gate = [1_000_000.0, 0.0, 0.0];
    let own = Zone {
        centre: [900_000.0, 0.0, 0.0],
        radius: RADIUS,
        source: Source::Field {
            ship: 7,
            character: 1,
            ship_type: FRIGATE,
        },
    };
    assert!(drag_in(&[own], 7, from, gate).is_none());
    assert!(drag_in(&[own], 8, from, gate).is_some());
}

#[test]
fn a_pilot_warping_through_a_probe_is_dragged_by_the_warp_itself() {
    let mut reg = world();
    place(&mut reg, BOB, SYSTEM, [-1.0e10, 0.0, 0.0]);
    // A probe on his line, near a destination 1,000 km out.
    reg.ext_mut::<Bubbles>().probes.push(Probe {
        ball_id: ID_BASE,
        type_id: PROBE,
        owner: ALICE,
        corporation_id: CORP,
        system: SYSTEM,
        centre: [900_000.0, 0.0, 0.0],
        radius: RADIUS,
        launched: T0,
        expiry: T0 + LIFE_MS * MSEC,
    });
    let edge = drag(&reg, BOB, [1_000_000.0, 0.0, 0.0], 0.0).expect("dragged");
    assert!(close(edge, [880_000.0, 0.0, 0.0], 1.0), "{edge:?}");
    // The standoff moves the arrival point, so the line's end.
    assert!(drag(&reg, BOB, [1_000_000.0, 0.0, 0.0], 50_000.0).is_some());
    assert!(drag(&reg, BOB, [1_000_000.0, 0.0, 0.0], 500_000.0).is_none());
    // The warp guard stands in the table.
    assert!(crate::movement_guard::WARP_GUARDS.contains(&(inside as crate::movement_guard::Guard)));
}

// ---------------------------------------------------------------------------
// The focused scripts (HIC-SCRIPT)
// ---------------------------------------------------------------------------

fn point_on(
    reg: &mut Registry,
    module: i64,
    target: Option<i64>,
    t: i64,
) -> Result<PyValue, ServiceError> {
    crate::modules::activate(reg, Some(ALICE), module, "warpDisruptSphere", target, 1, t)
}

fn lock_bob(reg: &mut Registry) {
    reg.locks_mut().add(ship(ALICE), ship(BOB));
}

#[test]
fn a_field_generator_with_no_script_runs_empty_and_names_no_target() {
    let mut reg = world();
    lock_bob(&mut reg);
    // A target handed to a bubble is dropped: it is a sphere, not a point.
    point_on(&mut reg, ALICE_FIELD, Some(ship(BOB)), T0).expect("activates empty");
    let r = *reg.modules().running(ALICE_FIELD).expect("running");
    assert_eq!(r.target_id, None);
    assert!(!crate::tackle::is_focused(&r.effect));
    assert_eq!(zones(&reg, SYSTEM).len(), 1);
}

#[test]
fn a_focused_script_makes_the_module_a_point_and_not_a_sphere() {
    let mut reg = world();
    point_on(&mut reg, ALICE_POINT, None, T0).expect_err("a point wants a target");
    lock_bob(&mut reg);
    point_on(&mut reg, ALICE_POINT, Some(ship(BOB)), T0).expect("activates");
    let r = *reg.modules().running(ALICE_POINT).expect("running");
    assert_eq!(r.target_id, Some(ship(BOB)));
    assert_eq!(
        r.effect.effect_id, EFFECT_FIELD,
        "the module's own id stays"
    );
    assert_eq!(r.effect.category, crate::modules::CATEGORY_TARGET);
    assert_eq!(r.effect.guid, "effects.WarpDisrupt");
    assert!(zones(&reg, SYSTEM).is_empty(), "no sphere");
    // The sphere's signature bloom does not run on a point.
    let hull = crate::dogma::hull(&reg, ship(ALICE))
        .expect("a sheet")
        .signature_radius;
    let mut plain = world();
    let bare = crate::dogma::hull(&plain, ship(ALICE))
        .expect("a sheet")
        .signature_radius;
    assert_eq!(hull, bare);
    field_on(&mut plain, T0).expect("activates");
    assert!(
        crate::dogma::hull(&plain, ship(ALICE))
            .expect("a sheet")
            .signature_radius
            > bare
    );
}

#[test]
fn a_disruption_script_holds_the_target_within_its_range_and_gate_scrambles() {
    use crate::tackle::{self, Live};
    let mut reg = world();
    lock_bob(&mut reg);
    point_on(&mut reg, ALICE_POINT, Some(ship(BOB)), T0).expect("activates");
    let found = tackle::sources(&reg, ship(BOB), T0, &Live);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].effect_id, tackle::FOCUSED_DISRUPT);
    assert_eq!(found[0].strength, 2.0);
    assert!(
        tackle::scrammed_by(&reg, BOB, T0, &Live).is_some(),
        "Bob cannot warp"
    );
    // The gate strength is the attribute's default of one when the module
    // names none; the rule reads it.
    let gate: Vec<f64> = tackle::gate_sources(&reg, ship(BOB), T0, &Live)
        .iter()
        .map(|s| s.strength)
        .collect();
    assert_eq!(gate, vec![1.0]);
    // A point does not block the MWD: only the scrambling script does.
    assert!(
        tackle::jamming_type(found[0].effect_id) == Some("warpScrambler"),
        "the buff-bar icon"
    );
    // Out of range it lets go.
    place(&mut reg, BOB, SYSTEM, [RADIUS + 5_000.0, 0.0, 0.0]);
    assert!(tackle::scrammed_by(&reg, BOB, T0, &Live).is_none());
}

#[test]
fn a_point_will_not_start_beyond_the_field_range() {
    let mut reg = world();
    lock_bob(&mut reg);
    place(&mut reg, BOB, SYSTEM, [RADIUS * 2.0, 0.0, 0.0]);
    let e = point_on(&mut reg, ALICE_POINT, Some(ship(BOB)), T0).expect_err("too far");
    assert_eq!(key(&e), "TargetTooFar");
}

#[test]
fn a_scrambling_script_is_a_scrambler_that_blocks_the_mwd() {
    use crate::tackle::{self, Live};
    let mut reg = world();
    lock_bob(&mut reg);
    point_on(&mut reg, ALICE_SCRAMBLER, Some(ship(BOB)), T0).expect("activates");
    let found = tackle::sources(&reg, ship(BOB), T0, &Live);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].effect_id, tackle::FOCUSED_SCRAMBLE);
    assert_eq!(
        tackle::jamming_type(found[0].effect_id),
        Some("warpScramblerMWD")
    );
}

#[test]
fn a_running_field_is_an_aggression_once_per_cycle() {
    let mut reg = world();
    field_on(&mut reg, T0).expect("activates");
    assert!(
        crate::crimewatch::refuse_jump(&reg, Some(ALICE), T0).is_ok(),
        "no timer before the clock runs"
    );
    tick(&mut reg, T0 + 1);
    assert!(
        crate::crimewatch::refuse_jump(&reg, Some(ALICE), T0 + 1).is_err(),
        "Bob is inside, so Alice has a weapons timer"
    );
}

#[test]
fn a_field_with_nobody_inside_is_no_aggression() {
    let mut reg = world();
    place(&mut reg, BOB, SYSTEM, [RADIUS * 3.0, 0.0, 0.0]);
    field_on(&mut reg, T0).expect("activates");
    tick(&mut reg, T0 + 1);
    assert!(crate::crimewatch::refuse_jump(&reg, Some(ALICE), T0 + 1).is_ok());
}
