//! The Micro Jump Drive's tests, and the ones for `SetBallMass` /
//! `SetBallAgility` that ride on the same change. **Every id and number here
//! is invented**: the types are 990,641,xxx, the systems 30,641,xxx, the
//! spool and the bloom are round numbers. The client's effect ids appear by
//! name only.

use super::*;
use crate::destiny::Mode;
use crate::modules::{Effect, MSEC};
use crate::sov_hub::fixture as hubfx;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
/// Invented: a spool of twelve seconds.
const SPOOL_MS: i64 = 12_000;
const SPOOL: i64 = SPOOL_MS * MSEC;
/// Invented: the bloom, in percent.
const BLOOM: f64 = 400.0;
/// Invented: the module's reactivation delay, ms.
const REACTIVATION_MS: f64 = 180_000.0;
/// Invented: the MWD's mass addition.
const MASS_ADDITION: f64 = 500_000.0;

const SYSTEM: i64 = 30_641_001;
const ALICE: i64 = 90_641_001;
const BOB: i64 = 90_641_002;
const CAROL: i64 = 90_641_003;
const CORP: i64 = 98_641_001;

const MJD: i64 = 990_641_001;
const SCRAMBLER: i64 = 990_641_002;
const MWD: i64 = 990_641_003;
const IMMOBILE: i64 = 990_641_050;
const FRIGATE: i64 = 990_000_001;
const GROUP_MJD: i64 = 990_641;

const ALICE_MJD: i64 = 100_641_101;
const ALICE_MWD: i64 = 100_641_102;
const BOB_SCRAM: i64 = 100_641_103;
const CAROL_MJD: i64 = 100_641_104;

/// `flagMedSlot0` **[V]**.
const MED_SLOT_0: i64 = 19;

const SCRAMBLE: i64 = 5934;

fn ship(who: i64) -> i64 {
    100_641_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}attribute\t973\t0.0\tsignatureRadiusBonusPercent\n\
         attribute\t20\t0.0\tspeedFactor\n\
         type\t{IMMOBILE}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=0.0;70=3.1;76=21000.0;263=420.0;265=440.0;552=100.0;600=5.0\tInvented Immobile\n\
         type\t{MJD}\t{GROUP_MJD}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t4921\t13,16,4921\t\
         73={SPOOL_MS}.0;182=4385.0;973={BLOOM};669={REACTIVATION_MS};763=1.0;906=1.0;1014=1.0\tInvented Micro Jump Drive\n\
         type\t{SCRAMBLER}\t{GROUP_MJD}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{SCRAMBLE}\t13,16,{SCRAMBLE}\t\
         73=5000.0;1350=1.0;105=2.0\tInvented Scrambler\n\
         type\t{MWD}\t46\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t6730\t13,16,6730\t\
         73=5000.0;20=500.0;567=1000000.0;796={MASS_ADDITION};554=500.0\tInvented MWD\n\
         effect\t4921\t1\t0\t73\t0\t0\t0\t0\tmicroJumpDrive\teffects.MicroJumpDriveEngage\t0\n\
         effect\t{SCRAMBLE}\t2\t1\t73\t0\t54\t0\t0\twarpScrambleForEntity\teffects.WarpScramble\n\
         effect\t6730\t1\t0\t73\t6\t0\t0\t0\tmoduleBonusMicrowarpdrive\teffects.MicroWarpDrive\n\
         effect\t13\t0\t0\t0\t0\t0\t0\t0\tmedPower\t\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn item(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    owner: i64,
    flag_id: i64,
) -> crate::item::Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    crate::item::Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: ship(owner),
        flag_id,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    }
}

fn place(reg: &mut Registry, who: i64, at: [f64; 3], velocity: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    let mut motion = Motion::arrived(at, velocity);
    if velocity != [0.0; 3] {
        // Flying along its heading.
        motion.mode = Mode::Goto([at[0] + 1.0e9, at[1], at[2]]);
        motion.speed_fraction = 1.0;
    }
    reg.roster_mut()
        .arrive(Presence::arriving(who, ship(who), SYSTEM, motion));
}

/// Alice (a frigate with an MJD and an MWD) at the origin flying +x at
/// 100 m/s, Bob (a frigate with a scrambler) 50 km down +x and at rest,
/// Carol (an immobile hull with an MJD). Every push drained.
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
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if i.item_id == ship(ALICE) || i.item_id == ship(BOB) {
            i.type_id = FRIGATE;
            i.group_id = 25;
        }
        if i.item_id == ship(CAROL) {
            i.type_id = IMMOBILE;
            i.group_id = 25;
        }
    }
    items.extend([
        item(&content, ALICE_MJD, MJD, ALICE, MED_SLOT_0),
        item(&content, ALICE_MWD, MWD, ALICE, MED_SLOT_0 + 1),
        item(&content, BOB_SCRAM, SCRAMBLER, BOB, MED_SLOT_0),
        item(&content, CAROL_MJD, MJD, CAROL, MED_SLOT_0),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_642_000,
        ..crate::store::Snapshot::default()
    });
    reg.character_mut(ALICE).expect("a pilot").ship_type_id = FRIGATE;
    reg.character_mut(BOB).expect("a pilot").ship_type_id = FRIGATE;
    reg.character_mut(CAROL).expect("a pilot").ship_type_id = IMMOBILE;
    place(&mut reg, ALICE, [0.0; 3], [100.0, 0.0, 0.0]);
    place(&mut reg, BOB, [50_000.0, 0.0, 0.0], [0.0; 3]);
    place(&mut reg, CAROL, [0.0, 0.0, 300_000.0], [0.0; 3]);
    let _ = hubfx::pending(&mut reg);
    let _ = hubfx::due(&mut reg);
    reg
}

fn pushes(reg: &mut Registry) -> Vec<hubfx::Sent> {
    let mut v = hubfx::pending(reg);
    v.extend(hubfx::due(reg));
    v
}

/// Every string and number in a value, joined.
fn flat(v: &PyValue) -> String {
    match v {
        PyValue::Bytes(b) => format!("{} ", String::from_utf8_lossy(b)),
        PyValue::Unicode(s) => format!("{s} "),
        PyValue::Int(n) => format!("{n} "),
        PyValue::Real(n) => format!("{n} "),
        PyValue::Bool(b) => format!("{b} "),
        PyValue::Tuple(l) | PyValue::List(l) => l.iter().map(flat).collect(),
        PyValue::Dict(d) => d.iter().map(|(a, b)| flat(a) + &flat(b)).collect(),
        PyValue::Instance { state, .. } => flat(state),
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            flat(header)
                + &list.iter().map(flat).collect::<String>()
                + &dict
                    .iter()
                    .map(|(a, b)| flat(a) + &flat(b))
                    .collect::<String>()
        }
        _ => String::new(),
    }
}

fn text(sent: &[hubfx::Sent]) -> Vec<(String, Option<Vec<i64>>)> {
    sent.iter()
        .map(|(_, a, to)| (a.iter().map(flat).collect::<String>(), to.clone()))
        .collect()
}

/// The raised `UserError`'s key.
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

fn engage(reg: &mut Registry, who: i64, module: i64, t: i64) -> Result<PyValue, ServiceError> {
    crate::modules::activate(reg, Some(who), module, "microJumpDrive", None, 1, t)
}

fn position(reg: &Registry, who: i64) -> [f64; 3] {
    reg.roster().get(who).expect("in space").motion.position
}

fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

fn signature(reg: &Registry, who: i64) -> f64 {
    crate::dogma::hull(reg, ship(who))
        .expect("a sheet")
        .signature_radius
}

/// The graphicInfo of each jump flash, with who it went to.
fn flashes(sent: &[hubfx::Sent]) -> Vec<(Vec<i64>, Vec<f64>)> {
    let mut out = Vec::new();
    for (name, args, to) in sent {
        if *name != "DoDestinyUpdate"
            || !args.iter().map(flat).collect::<String>().contains(FX_JUMP)
        {
            continue;
        }
        let PyValue::List(entries) = &args[0] else {
            continue;
        };
        for e in entries {
            let PyValue::Tuple(e) = e else { continue };
            let PyValue::Tuple(call) = &e[1] else {
                continue;
            };
            let PyValue::Tuple(fx) = &call[1] else {
                continue;
            };
            let PyValue::Tuple(info) = fx.last().expect("arguments") else {
                panic!("graphicInfo is a tuple");
            };
            let v = info
                .iter()
                .map(|c| match c {
                    PyValue::Real(r) => *r,
                    other => panic!("a real: {other:?}"),
                })
                .collect();
            out.push((to.clone().unwrap_or_default(), v));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The jump
// ---------------------------------------------------------------------------

#[test]
fn the_drive_jumps_a_hundred_km_along_the_heading_and_keeps_its_velocity() {
    let mut reg = world();
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    // The spool blooms the signature, and only while it runs.
    assert_eq!(signature(&reg, ALICE), 100.0 * (1.0 + BLOOM / 100.0));
    let _ = pushes(&mut reg);
    crate::modules::tick(&mut reg, T0 + SPOOL - 1);
    assert!(close(position(&reg, ALICE), [0.0; 3], 5.0), "not yet");
    crate::modules::tick(&mut reg, T0 + SPOOL);
    let p = position(&reg, ALICE);
    assert!(
        close(p, [DISTANCE, 0.0, 0.0], 5.0),
        "a hundred km down +x: {p:?}"
    );
    let v = reg.roster().get(ALICE).expect("in space").motion.velocity;
    assert!(close(v, [100.0, 0.0, 0.0], 1.0), "velocity kept: {v:?}");
    assert_eq!(signature(&reg, ALICE), 100.0, "the bloom ends");
    let sent = text(&pushes(&mut reg));
    let moved: Vec<_> = sent
        .iter()
        .filter(|(t, _)| t.contains("SetBallPosition"))
        .collect();
    assert_eq!(moved.len(), 1, "one batch moves the ball: {sent:?}");
    assert!(
        !moved[0].0.contains("SetBallVelocity"),
        "no velocity in it: {}",
        moved[0].0
    );
    assert!(
        sent.iter().all(|(t, _)| !t.contains("SetBallVelocity")),
        "the velocity is not touched"
    );
}

#[test]
fn each_viewer_gets_the_flash_relative_to_his_own_ego_ball() {
    let mut reg = world();
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    let _ = pushes(&mut reg);
    crate::modules::tick(&mut reg, T0 + SPOOL);
    let all = flashes(&pushes(&mut reg));
    let to_bob = all
        .iter()
        .find(|(to, _)| to == &vec![BOB])
        .expect("Bob is sent the flash");
    // Alice left from about the origin; Bob is at 50 km.
    assert!(
        (to_bob.1[0] + 50_000.0).abs() < 5.0 && to_bob.1[1].abs() < 5.0 && to_bob.1[2].abs() < 5.0,
        "origin minus Bob's ego: {:?}",
        to_bob.1
    );
    let to_alice = all
        .iter()
        .find(|(to, _)| to == &vec![ALICE])
        .expect("Alice is sent the flash");
    assert!(
        (to_alice.1[0] + DISTANCE).abs() < 5.0,
        "origin minus Alice's own (new) position: {:?}",
        to_alice.1
    );
    assert_ne!(to_bob.1, to_alice.1, "one shared position would be wrong");
}

#[test]
fn a_scram_at_the_end_of_the_spool_blocks_the_jump() {
    let mut reg = world();
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    let _ = pushes(&mut reg);
    // Bob locks Alice and scrams her, six seconds in.
    assert!(reg.locks_mut().add(ship(BOB), ship(ALICE)));
    reg.modules_mut().insert_running(Running {
        character_id: BOB,
        ship_id: ship(BOB),
        module_id: BOB_SCRAM,
        type_id: SCRAMBLER,
        effect: Effect {
            effect_id: SCRAMBLE,
            name: "warpScrambleForEntity",
            category: 2,
            guid: "effects.WarpScramble",
            is_offensive: true,
            duration_attribute: 73,
            warp_safe: None,
        },
        target_id: Some(ship(ALICE)),
        started: T0 + 6_000 * MSEC,
        period: 5_000 * MSEC,
        repeat: 1000,
        cycles: 1,
        stop_at: None,
    });
    crate::modules::tick(&mut reg, T0 + SPOOL);
    assert!(close(position(&reg, ALICE), [0.0; 3], 5.0), "no jump");
    let sent = text(&pushes(&mut reg));
    assert!(
        sent.iter()
            .any(|(t, to)| t.contains("ModuleIsBlocked") && to.as_deref() == Some(&[ALICE][..])),
        "the stop says why: {sent:?}"
    );
    assert!(flashes(&hubfx::pending(&mut reg)).is_empty());
}

#[test]
fn nothing_warps_jumps_or_docks_while_the_drive_spools() {
    let mut reg = world();
    assert!(spooling(&reg, ALICE).is_none());
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    let r = spooling(&reg, ALICE).expect("spooling");
    assert_eq!(r.user_error, SPOOLING);
    assert!(
        crate::movement_guard::check(crate::movement_guard::WARP_GUARDS, &reg, ALICE).is_some()
    );
    assert!(
        crate::movement_guard::check(crate::movement_guard::JUMP_GUARDS, &reg, ALICE).is_some()
    );
    assert_eq!(key(&refuse_dock(&reg, Some(ALICE)).unwrap_err()), SPOOLING);
    // Bob is free.
    assert!(spooling(&reg, BOB).is_none());
    crate::modules::tick(&mut reg, T0 + SPOOL);
    assert!(spooling(&reg, ALICE).is_none(), "free again");
    assert!(refuse_dock(&reg, Some(ALICE)).is_ok());
}

#[test]
fn the_drive_cannot_be_activated_again_until_its_delay_is_over() {
    let mut reg = world();
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    crate::modules::tick(&mut reg, T0 + SPOOL);
    let e = engage(&mut reg, ALICE, ALICE_MJD, T0 + SPOOL + 100_000 * MSEC).expect_err("too soon");
    assert_eq!(key(&e), "ModuleReactivationDelayed2");
    engage(&mut reg, ALICE, ALICE_MJD, T0 + SPOOL + 200_000 * MSEC).expect("after the delay");
}

#[test]
fn an_immobile_ship_cannot_activate_it() {
    let mut reg = world();
    let e = engage(&mut reg, CAROL, CAROL_MJD, T0).expect_err("immobile");
    assert_eq!(key(&e), "CantActivateMJDBecauseOfZeroVelocity");
    assert!(spooling(&reg, CAROL).is_none());
}

#[test]
fn a_stationary_ship_jumps_toward_its_goto_point() {
    let mut reg = world();
    {
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion.velocity = [0.0; 3];
        p.motion.mode = Mode::Goto([0.0, 1.0e9, 0.0]);
    }
    engage(&mut reg, ALICE, ALICE_MJD, T0).expect("activates");
    crate::modules::tick(&mut reg, T0 + SPOOL);
    let p = position(&reg, ALICE);
    assert!(p[1] > DISTANCE - 1_000.0, "up the goto line: {p:?}");
}

#[test]
fn facing_prefers_the_velocity_then_the_goto_point_then_z() {
    assert_eq!(
        facing([0.0; 3], [0.0, 50.0, 0.0], Some([1.0, 0.0, 0.0])),
        [0.0, 1.0, 0.0]
    );
    assert_eq!(
        facing([0.0; 3], [0.5, 0.0, 0.0], Some([0.0, 0.0, 9.0])),
        [0.0, 0.0, 1.0]
    );
    assert_eq!(facing([0.0; 3], [0.0; 3], None), [0.0, 0.0, 1.0]);
    assert_eq!(
        facing([1.0, 1.0, 1.0], [0.0; 3], Some([1.0, 1.0, 5.0])),
        [0.0, 0.0, 1.0]
    );
}

// ---------------------------------------------------------------------------
// Mass and agility
// ---------------------------------------------------------------------------

fn mwd(reg: &mut Registry, activate: bool, t: i64) {
    if activate {
        crate::modules::activate(
            reg,
            Some(ALICE),
            ALICE_MWD,
            "moduleBonusMicrowarpdrive",
            None,
            1000,
            t,
        )
        .expect("the MWD runs");
    } else {
        assert!(crate::modules::stop(reg, ALICE_MWD, t, None));
    }
}

#[test]
fn a_microwarpdrive_tells_the_park_its_new_mass_and_the_stop_tells_it_back() {
    let mut reg = world();
    let hull = crate::dogma::hull(&reg, ship(ALICE)).expect("a sheet");
    let _ = pushes(&mut reg);
    mwd(&mut reg, true, T0);
    let on = crate::dogma::hull(&reg, ship(ALICE)).expect("a sheet");
    assert_eq!(on.mass, hull.mass + MASS_ADDITION);
    let sent = text(&pushes(&mut reg));
    let batch = sent
        .iter()
        .find(|(t, to)| t.contains("SetMaxSpeed") && to.as_ref().is_some_and(|v| v.contains(&BOB)))
        .expect("the pilot and the observer are told");
    assert!(batch.0.contains("SetBallMass"), "{}", batch.0);
    assert!(batch.0.contains(&format!("{}", hull.mass + MASS_ADDITION)));
    assert!(
        !batch.0.contains("SetBallAgility") || on.agility != hull.agility,
        "agility only if it moved"
    );
    // The stop puts the hull's mass back.
    mwd(&mut reg, false, T0 + 1);
    let sent = text(&pushes(&mut reg));
    let back = sent
        .iter()
        .find(|(t, _)| t.contains("SetBallMass"))
        .expect("the stop is told");
    assert!(back.0.contains(&format!("{}", hull.mass)), "{}", back.0);
    assert!(!back.0.contains(&format!("{}", hull.mass + MASS_ADDITION)));
}

#[test]
fn a_change_of_speed_alone_sends_no_mass() {
    // The web path (`ewar`, `npc_ewar`) calls `SetMaxSpeed` itself; this
    // is the module path: with `before` equal in mass and agility, only the
    // speed goes.
    let mut reg = world();
    let h = crate::dogma::hull(&reg, ship(ALICE)).expect("a sheet");
    let _ = pushes(&mut reg);
    let mut before = h;
    before.max_velocity = h.max_velocity - 10.0;
    crate::modules::motion_changed_for_test(&mut reg, ALICE, ship(ALICE), Some(before));
    let sent = text(&pushes(&mut reg));
    let batch = sent
        .iter()
        .find(|(t, _)| t.contains("SetMaxSpeed"))
        .expect("the speed moved");
    assert!(!batch.0.contains("SetBallMass"), "{}", batch.0);
    assert!(!batch.0.contains("SetBallAgility"), "{}", batch.0);
    // With nothing to compare against, all three go.
    crate::modules::motion_changed_for_test(&mut reg, ALICE, ship(ALICE), None);
    let sent = text(&pushes(&mut reg));
    let batch = sent
        .iter()
        .find(|(t, _)| t.contains("SetMaxSpeed"))
        .expect("everything is sent");
    assert!(batch.0.contains("SetBallMass") && batch.0.contains("SetBallAgility"));
}
