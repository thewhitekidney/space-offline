//! The activation rules' tests (module-rules): the warp shutdown, the
//! `maxGroupActive` limit, `NotEnoughEnergy` and the reactivation delay.
//! **Every id and number here is invented**: types 990,700,xxx, effects
//! 990,00x, a 10 s cycle, a 30 s delay.

use super::*;
use crate::destiny::Mode;
use crate::sov_hub::fixture as hubfx;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
const SEC: i64 = 1_000 * MSEC;
const ALICE: i64 = 90_700_001;
const SHIP: i64 = 100_700_001;
const SYSTEM: i64 = 30_700_001;
const FRIGATE: i64 = 990_000_001;
const HI_SLOT_0: i64 = 27;

const GROUP_PROP: i64 = 990_700;
const GROUP_SAFE: i64 = 990_701;
const GROUP_DELAYED: i64 = 990_702;
const GROUP_HUNGRY: i64 = 990_703;
const PROP_A: i64 = 990_700_001;
const PROP_B: i64 = 990_700_002;
const SAFE: i64 = 990_700_003;
const DELAYED: i64 = 990_700_004;
const HUNGRY: i64 = 990_700_005;
const M_PROP_A: i64 = 100_700_101;
const M_PROP_B: i64 = 100_700_102;
const M_SAFE: i64 = 100_700_103;
const M_DELAYED: i64 = 100_700_104;
const M_HUNGRY: i64 = 100_700_105;
const M_SAFE_2: i64 = 100_700_106;
/// A hull that limits `GROUP_SAFE` to one active module (909 / 910).
const CAPPED: i64 = 990_700_010;

const E_BURN: i64 = 990_001;
const E_REP: i64 = 990_002;
const E_COOL: i64 = 990_003;
const E_DRAIN: i64 = 990_004;

/// The invented content. `with_column` false writes the effect rows the way
/// an old extract did, without `isWarpSafe`.
fn content(with_column: bool) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let module = |id: i64, group: i64, effect: i64, attrs: &str, name: &str| {
        format!(
            "type\t{id}\t{group}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{effect}\t12,16,{effect}\t\
             73=10000.0;{attrs}\t{name}\n"
        )
    };
    let eff = |id: i64, name: &str, safe: &str| {
        let col = if with_column {
            format!("\t{safe}")
        } else {
            String::new()
        };
        format!("effect\t{id}\t1\t0\t73\t0\t0\t0\t0\t{name}\t{col}\n")
    };
    let capped = format!(
        "type\t{CAPPED}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t482=260.0;909={GROUP_SAFE}.0;910=1.0\tInvented Capped Hull\n"
    );
    let rows = [
        crate::typedata::FIXTURE,
        &module(PROP_A, GROUP_PROP, E_BURN, "763=1.0", "Invented Burner A"),
        &module(PROP_B, GROUP_PROP, E_BURN, "763=1.0", "Invented Burner B"),
        &module(SAFE, GROUP_SAFE, E_REP, "", "Invented Repairer"),
        &module(
            DELAYED,
            GROUP_DELAYED,
            E_COOL,
            "669=30000.0",
            "Invented Cooler",
        ),
        &module(HUNGRY, GROUP_HUNGRY, E_DRAIN, "6=50.0", "Invented Drainer"),
        &eff(E_BURN, "fixtureBurn", "0"),
        &eff(E_REP, "fixtureRep", "1"),
        &eff(E_COOL, "fixtureCool", "0"),
        &eff(E_DRAIN, "fixtureDrain", "1"),
        &capped,
    ]
    .concat();
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn world(with_column: bool) -> Registry {
    let content = std::sync::Arc::new(content(with_column));
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: SHIP,
            ship_type_id: FRIGATE,
            solar_system_id: Some(SYSTEM),
            ..crate::model::Character::default()
        }],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = FRIGATE;
        i.group_id = 25;
    }
    for (n, (id, ty)) in [
        (M_PROP_A, PROP_A),
        (M_PROP_B, PROP_B),
        (M_SAFE, SAFE),
        (M_DELAYED, DELAYED),
        (M_HUNGRY, HUNGRY),
        (M_SAFE_2, SAFE),
    ]
    .into_iter()
    .enumerate()
    {
        let t = crate::item::find_in(&content, ty).expect("a test type");
        items.push(crate::item::Item {
            item_id: id,
            type_id: ty,
            owner_id: ALICE,
            location_id: SHIP,
            flag_id: HI_SLOT_0 + n as i64,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_701_000,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let _ = hubfx::pending(&mut reg);
    let _ = hubfx::due(&mut reg);
    reg
}

fn go(reg: &mut Registry, module: i64, effect: &str, t: i64) -> Result<PyValue, ServiceError> {
    activate(reg, Some(ALICE), module, effect, None, 1000, t)
}

/// `(key, dict)` of a raised `UserError`.
fn raised(e: &ServiceError) -> (String, Vec<(PyValue, PyValue)>) {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("not a raised UserError: {e:?}");
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("odd header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("odd args")
    };
    let (PyValue::Bytes(key), PyValue::Dict(d)) = (&args[0], &args[1]) else {
        panic!("odd key")
    };
    (String::from_utf8_lossy(key).into_owned(), d.clone())
}

fn arg<'a>(d: &'a [(PyValue, PyValue)], name: &str) -> &'a PyValue {
    crate::pyutil::pairs_get(d, name.as_bytes()).unwrap_or_else(|| panic!("no arg {name}: {d:?}"))
}

/// The `error` of every stop `OnGodmaShipEffect` for `module` the clock sent.
fn stop_errors(reg: &mut Registry, module: i64) -> Vec<PyValue> {
    hubfx::due(reg)
        .into_iter()
        .filter(|(n, a, _)| {
            *n == "OnGodmaShipEffect" && a[0] == PyValue::Int(module) && a[3] == PyValue::Int(0)
        })
        .map(|(_, a, _)| a[9].clone())
        .collect()
}

/// Put Alice's ship in an active warp (the stamp is long past).
fn warp(reg: &mut Registry) {
    reg.roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .mode = Mode::Warp {
        goto: [1.0e9, 0.0, 0.0],
        effect_stamp: 1,
        total_distance: 1.0e9,
        stop_distance: 0.0,
        warp_factor: 4_000,
    };
}

fn error_key(v: &PyValue) -> String {
    let PyValue::Tuple(t) = v else {
        panic!("an error is (key, dict): {v:?}")
    };
    let PyValue::Bytes(k) = &t[0] else {
        panic!("key")
    };
    String::from_utf8_lossy(k).into_owned()
}

#[test]
fn a_warp_safe_effect_runs_on_and_an_unsafe_one_is_shut_off() {
    let mut reg = world(true);
    go(&mut reg, M_PROP_A, "fixtureBurn", T0).expect("burn");
    go(&mut reg, M_SAFE, "fixtureRep", T0).expect("rep");
    let _ = hubfx::pending(&mut reg);
    // Not warping yet: a tick stops nothing.
    tick(&mut reg, T0 + SEC);
    assert!(reg.modules().running(M_PROP_A).is_some());
    warp(&mut reg);
    tick(&mut reg, T0 + 2 * SEC);
    assert!(reg.modules().running(M_PROP_A).is_none(), "unsafe: stopped");
    assert!(reg.modules().running(M_SAFE).is_some(), "safe: still runs");
    let errors = stop_errors(&mut reg, M_PROP_A);
    assert_eq!(errors.len(), 1);
    assert_eq!(error_key(&errors[0]), "EffectDeactivationWarping");
}

#[test]
fn an_extract_without_the_column_stops_nothing() {
    let mut reg = world(false);
    go(&mut reg, M_PROP_A, "fixtureBurn", T0).expect("burn");
    warp(&mut reg);
    tick(&mut reg, T0 + SEC);
    assert!(reg.modules().running(M_PROP_A).is_some());
}

#[test]
fn a_second_module_of_a_limited_group_is_crowded_out_until_the_first_stops() {
    let mut reg = world(true);
    go(&mut reg, M_PROP_A, "fixtureBurn", T0).expect("the first");
    let e = go(&mut reg, M_PROP_B, "fixtureBurn", T0 + SEC).expect_err("the second");
    let (key, d) = raised(&e);
    assert_eq!(key, "EffectCrowdedOut");
    assert_eq!(arg(&d, "module"), &PyValue::Int(PROP_B));
    assert_eq!(arg(&d, "count"), &PyValue::Int(1));
    // Stopping still counts as active until the cycle ends.
    deactivate(&mut reg, Some(ALICE), M_PROP_A, "fixtureBurn", T0 + 2 * SEC).expect("stop");
    let e = go(&mut reg, M_PROP_B, "fixtureBurn", T0 + 3 * SEC).expect_err("still stopping");
    assert_eq!(raised(&e).0, "EffectCrowdedOut");
    // After the cycle boundary the first is gone and the second starts.
    go(&mut reg, M_PROP_B, "fixtureBurn", T0 + 11 * SEC).expect("the second");
    assert!(reg.modules().running(M_PROP_B).is_some());
}

#[test]
fn a_hull_limit_on_a_group_caps_it_though_the_module_has_none() {
    // On a plain hull two repairers run together.
    let mut reg = world(true);
    go(&mut reg, M_SAFE, "fixtureRep", T0).expect("the first");
    go(&mut reg, M_SAFE_2, "fixtureRep", T0).expect("the second");
    // On a hull that names the group (909) with a limit of 1 (910), they
    // do not.
    let mut reg = world(true);
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = CAPPED;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_701_000,
        ..crate::store::Snapshot::default()
    });
    go(&mut reg, M_SAFE, "fixtureRep", T0).expect("the first");
    let e = go(&mut reg, M_SAFE_2, "fixtureRep", T0).expect_err("the second");
    let (key, d) = raised(&e);
    assert_eq!(key, "EffectCrowdedOut");
    assert_eq!(arg(&d, "module"), &PyValue::Int(SAFE));
}

#[test]
fn too_little_capacitor_refuses_the_activation_and_names_the_numbers() {
    let mut reg = world(true);
    reg.modules_mut().capacitor.insert(
        SHIP,
        Charge {
            value: 10.0,
            at: T0,
        },
    );
    let e = go(&mut reg, M_HUNGRY, "fixtureDrain", T0).expect_err("refused");
    let (key, d) = raised(&e);
    assert_eq!(key, "NotEnoughEnergy");
    assert_eq!(arg(&d, "need"), &PyValue::Real(50.0));
    assert_eq!(arg(&d, "got"), &PyValue::Real(10.0));
    assert_eq!(
        arg(&d, "effectname"),
        &crate::pyutil::bstr("Invented Drainer")
    );
    assert!(reg.modules().running(M_HUNGRY).is_none());
}

#[test]
fn a_drained_capacitor_stops_the_module_with_the_key() {
    let mut reg = world(true);
    go(&mut reg, M_HUNGRY, "fixtureDrain", T0).expect("starts on a full cap");
    let _ = hubfx::pending(&mut reg);
    reg.modules_mut().capacitor.insert(
        SHIP,
        Charge {
            value: 0.0,
            at: T0 + 10 * SEC,
        },
    );
    // The next cycle starts at +10 s and cannot be paid.
    tick(&mut reg, T0 + 10 * SEC);
    assert!(reg.modules().running(M_HUNGRY).is_none());
    let errors = stop_errors(&mut reg, M_HUNGRY);
    assert_eq!(errors.len(), 1);
    assert_eq!(error_key(&errors[0]), "NotEnoughEnergy");
}

#[test]
fn a_stopped_module_waits_out_its_reactivation_delay() {
    let mut reg = world(true);
    go(&mut reg, M_DELAYED, "fixtureCool", T0).expect("starts");
    // The stop is the cycle's end, 10 s in; the click came at 4 s.
    deactivate(
        &mut reg,
        Some(ALICE),
        M_DELAYED,
        "fixtureCool",
        T0 + 4 * SEC,
    )
    .expect("stop");
    tick(&mut reg, T0 + 10 * SEC);
    assert!(reg.modules().running(M_DELAYED).is_none());
    assert_eq!(reg.modules().last_stop(M_DELAYED), Some(T0 + 10 * SEC));
    let e = go(&mut reg, M_DELAYED, "fixtureCool", T0 + 20 * SEC).expect_err("too soon");
    let (key, d) = raised(&e);
    assert_eq!(key, "ModuleReactivationDelayed2");
    assert_eq!(arg(&d, "timeSinceLastStop"), &PyValue::Int(10_000));
    assert_eq!(arg(&d, "itemID"), &PyValue::Int(M_DELAYED));
    assert_eq!(arg(&d, "delay"), &PyValue::Int(20 * SEC));
    go(&mut reg, M_DELAYED, "fixtureCool", T0 + 40 * SEC).expect("30 s have passed");
}

#[test]
fn a_type_without_a_delay_restarts_at_once() {
    let mut reg = world(true);
    go(&mut reg, M_SAFE, "fixtureRep", T0).expect("starts");
    assert!(stop(&mut reg, M_SAFE, T0 + SEC, None));
    go(&mut reg, M_SAFE, "fixtureRep", T0 + SEC).expect("again");
}

#[test]
fn forgetting_a_module_clears_its_delay() {
    let mut reg = world(true);
    go(&mut reg, M_DELAYED, "fixtureCool", T0).expect("starts");
    assert!(stop(&mut reg, M_DELAYED, T0 + SEC, None));
    assert!(reg.modules().last_stop(M_DELAYED).is_some());
    reg.modules_mut().forget(M_DELAYED);
    assert!(reg.modules().last_stop(M_DELAYED).is_none());
}
