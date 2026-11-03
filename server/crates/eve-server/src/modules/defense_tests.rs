//! LIVE-RESISTS' tests: the fitting window's twelve resonances reach godma
//! (`fitstate::defense`), a hardener's start and end push them
//! (`defense_changed`), and a changed resist reaches the combat record.
//! **Every id and number here is invented**: types 990,720,xxx, a hull with
//! shield EM 0.75, armour EM 0.5, structure EM 0.6 in its own row, a hardener
//! of -20 % and a passive module of -30 %.

use super::*;
use crate::damage::{Profile, ShipDamage};
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
const SEC: i64 = 1_000 * MSEC;
const ALICE: i64 = 90_720_001;
const SHIP: i64 = 100_720_001;
const SYSTEM: i64 = 30_720_001;
const HULL: i64 = 990_720_001;
const HARDENER: i64 = 990_720_002;
const PASSIVE: i64 = 990_720_003;
const PLAIN: i64 = 990_720_004;
const M_HARDENER: i64 = 100_720_101;
const M_PASSIVE: i64 = 100_720_102;
const M_PLAIN: i64 = 100_720_103;
const BOOSTER: i64 = 990_720_005;
const M_BOOSTER: i64 = 100_720_104;
const E_BOOSTER: i64 = 990_723;
const E_HARDENER: i64 = 990_720;
const E_PASSIVE: i64 = 990_721;
const E_PLAIN: i64 = 990_722;
const HI_SLOT_0: i64 = 27;
const MED_SLOT_0: i64 = 19;
const LO_SLOT_0: i64 = 11;
const SHIELD_EM: i64 = 271;
const ARMOR_EM: i64 = 267;
const HULL_EM: i64 = 113;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}\
         type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;37=355.0;76=20000.0;263=420.0;265=440.0;271=0.75;267=0.5;113=0.6;283=25.0;264=420.0\tInvented Hull\n\
         type\t{HARDENER}\t77\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_HARDENER}\t13,16,{E_HARDENER}\t\
         73=5000.0;6=0.0;271=-20.0\tInvented Hardener\n\
         type\t{PASSIVE}\t78\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t11,16,{E_PASSIVE}\t\
         267=-30.0;9000=50.0\tInvented Passive\n\
         type\t{PLAIN}\t79\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_PLAIN}\t12,16,{E_PLAIN}\t\
         73=5000.0;6=0.0\tInvented Plain\n\
         type\t{BOOSTER}\t80\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_BOOSTER}\t13,16,{E_BOOSTER}\t\
         73=5000.0;6=0.0;309=30.0\tInvented Booster\n\
         effect\t{E_HARDENER}\t1\t0\t73\t0\t0\t0\t0\tinventedHardener\t1\n\
         effect\t{E_PASSIVE}\t4\t0\t0\t0\t0\t0\t0\tinventedPassive\t\n\
         effect\t{E_PLAIN}\t1\t0\t73\t0\t0\t0\t0\tinventedPlain\t1\n\
         effect\t{E_BOOSTER}\t1\t0\t73\t0\t0\t0\t0\tinventedBooster\t1\n\
         modifier\t{E_BOOSTER}\tshipID\tItemModifier\t76\t309\t6\t\t\n\
         modifier\t{E_HARDENER}\tshipID\tItemModifier\t271\t271\t6\t\t\n\
         modifier\t{E_PASSIVE}\tshipID\tItemModifier\t267\t267\t6\t\t\n\
         modifier\t{E_PASSIVE}\tshipID\tItemModifier\t38\t9000\t6\t\t\n",
        crate::typedata::FIXTURE
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

/// Alice in space in an invented hull with the three modules fitted and
/// nothing running.
fn world() -> Registry {
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![crate::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: SHIP,
            ship_type_id: HULL,
            solar_system_id: Some(SYSTEM),
            ..crate::model::Character::default()
        }],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = HULL;
        i.group_id = 25;
    }
    for (id, ty, flag) in [
        (M_HARDENER, HARDENER, MED_SLOT_0),
        (M_PASSIVE, PASSIVE, LO_SLOT_0),
        (M_PLAIN, PLAIN, HI_SLOT_0),
        (M_BOOSTER, BOOSTER, MED_SLOT_0 + 1),
    ] {
        let t = crate::item::find_in(&content, ty).expect("a test type");
        items.push(crate::item::Item {
            item_id: id,
            type_id: ty,
            owner_id: ALICE,
            location_id: SHIP,
            flag_id: flag,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_721_000,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let _ = reg.take_pending();
    let _ = reg.take_due(std::time::Instant::now() + Duration::from_secs(3_600));
    reg
}

/// Every resonance change pushed so far, now or on the clock, as
/// `(attribute, new, old)`.
fn resonance_changes(reg: &mut Registry) -> Vec<(i64, f64, f64)> {
    let mut pushes: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
    pushes.extend(reg.take_due(std::time::Instant::now() + Duration::from_secs(3_600)));
    pushes
        .into_iter()
        .map(|p| p.to_push())
        .filter(|(n, _)| *n == "OnModuleAttributeChange")
        .filter_map(|(_, a)| match (&a[2], &a[4], &a[5]) {
            (PyValue::Int(id), PyValue::Real(new), PyValue::Real(old))
                if crate::damage::attr::is_resonance(*id) =>
            {
                Some((*id, *new, *old))
            }
            _ => None,
        })
        .collect()
}

fn served(reg: &Registry, id: i64) -> Option<f64> {
    let c = reg.character(ALICE).expect("pilot").clone();
    let PyValue::Instance { state, .. } = reg.capsule_dogma_row(&c) else {
        panic!("a KeyVal");
    };
    let PyValue::Dict(fields) = *state else {
        panic!("a dict state");
    };
    let attrs = fields.into_iter().find_map(|(v, k)| match k {
        PyValue::Bytes(b) if b == b"attributes" => Some(v),
        _ => None,
    });
    let Some(PyValue::Dict(e)) = attrs else {
        panic!("an attributes dict");
    };
    e.iter().find_map(|(v, k)| match (k, v) {
        (PyValue::Int(k), PyValue::Real(v)) if *k == id => Some(*v),
        _ => None,
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// The ship's row carries all twelve resonances: the hull's own where its
/// row has them, the attribute's default (1.0) where it does not.
#[test]
fn the_ships_row_carries_the_twelve_resonances() {
    let reg = world();
    assert!(close(served(&reg, SHIELD_EM).expect("served"), 0.75));
    // The fitted passive module is online: 0.5 x (1 - 0.30).
    assert!(close(served(&reg, ARMOR_EM).expect("served"), 0.35));
    assert!(close(served(&reg, HULL_EM).expect("served"), 0.6));
    for id in [272, 273, 274, 268, 269, 270, 111, 109, 110] {
        assert!(close(served(&reg, id).expect("served"), 1.0), "{id}");
    }
}

/// Onlining a passive resist module pushes the armour resonance once;
/// asking again says nothing.
#[test]
fn onlining_a_resist_module_pushes_the_new_resonance() {
    let mut reg = world();
    // Fitted modules start online: take it offline, then bring it back.
    crate::fitstate::around(&mut reg, Some(ALICE), |reg| {
        set_online_kept(reg, M_PASSIVE, false);
    });
    let _ = resonance_changes(&mut reg);
    crate::fitstate::around(&mut reg, Some(ALICE), |reg| {
        set_online_kept(reg, M_PASSIVE, true);
    });
    let got = resonance_changes(&mut reg);
    assert_eq!(got.len(), 1, "{got:?}");
    let (id, new, old) = got[0];
    assert_eq!(id, ARMOR_EM);
    assert!(close(old, 0.5) && close(new, 0.35), "{got:?}");
    crate::fitstate::around(&mut reg, Some(ALICE), |_| ());
    assert!(resonance_changes(&mut reg).is_empty());
}

/// A hardener's start pushes its layer's resonance at once; its end, at the
/// cycle boundary on the clock, pushes it back. A module with no resonance
/// modifier pushes none.
#[test]
fn a_hardener_pushes_its_resonance_on_and_off() {
    let mut reg = world();
    activate(
        &mut reg,
        Some(ALICE),
        M_PLAIN,
        "inventedPlain",
        None,
        1000,
        T0,
    )
    .expect("plain");
    assert!(resonance_changes(&mut reg).is_empty(), "no resonance moved");

    activate(
        &mut reg,
        Some(ALICE),
        M_HARDENER,
        "inventedHardener",
        None,
        1000,
        T0,
    )
    .expect("hardener");
    let on = resonance_changes(&mut reg);
    assert_eq!(on.len(), 1, "{on:?}");
    assert_eq!(on[0].0, SHIELD_EM);
    assert!(close(on[0].2, 0.75) && close(on[0].1, 0.6), "{on:?}");

    deactivate(
        &mut reg,
        Some(ALICE),
        M_HARDENER,
        "inventedHardener",
        T0 + SEC,
    )
    .expect("asked");
    assert!(resonance_changes(&mut reg).is_empty(), "still running");
    tick(&mut reg, T0 + 6 * SEC);
    assert!(reg.modules().running(M_HARDENER).is_none());
    let off = resonance_changes(&mut reg);
    assert_eq!(off.len(), 1, "{off:?}");
    assert_eq!(off[0].0, SHIELD_EM);
    assert!(close(off[0].2, 0.6) && close(off[0].1, 0.75), "{off:?}");
}

/// The combat record is read at the first hit; a hardener switched on or off
/// after it must reach the record.
#[test]
fn a_hardener_changes_the_combat_record_after_the_first_hit() {
    let mut reg = world();
    let first = crate::dogma::profile(&reg, SHIP).expect("a sheet");
    assert!(close(first.shield_resonance.em, 0.75));
    // The ship was hit once: a record exists with the profile of that moment.
    let record = ShipDamage::full(first, T0);
    reg.combat_mut().set_ship(SHIP, record);

    activate(
        &mut reg,
        Some(ALICE),
        M_HARDENER,
        "inventedHardener",
        None,
        1000,
        T0,
    )
    .expect("hardener");
    let p: Profile = *reg.combat().ship(SHIP).expect("a record").profile();
    assert!(
        close(p.shield_resonance.em, 0.6),
        "{:?}",
        p.shield_resonance
    );

    deactivate(
        &mut reg,
        Some(ALICE),
        M_HARDENER,
        "inventedHardener",
        T0 + SEC,
    )
    .expect("asked");
    tick(&mut reg, T0 + 6 * SEC);
    let p: Profile = *reg.combat().ship(SHIP).expect("a record").profile();
    assert!(
        close(p.shield_resonance.em, 0.75),
        "{:?}",
        p.shield_resonance
    );
}

// SHIP-SHEET: the sheet's ship attributes reach godma (capacity, drone bay),
// follow a fit, and never include a charged attribute.

const CAPACITY: i64 = 38;
const DRONE_BAY: i64 = 283;

/// Every attribute change pushed so far for `id`, as `(new, old)`.
fn changes_of(reg: &mut Registry, id: i64) -> Vec<(f64, f64)> {
    let mut pushes: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
    pushes.extend(reg.take_due(std::time::Instant::now() + Duration::from_secs(3_600)));
    pushes
        .into_iter()
        .map(|p| p.to_push())
        .filter(|(n, _)| *n == "OnModuleAttributeChange")
        .filter_map(|(_, a)| match (&a[2], &a[4], &a[5]) {
            (PyValue::Int(i), PyValue::Real(new), PyValue::Real(old)) if *i == id => {
                Some((*new, *old))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_prime_carries_capacity_and_drone_bay_from_the_sheet() {
    let reg = world();
    // The hull's 130, raised by the online passive module's +50 %.
    assert!(
        close(served(&reg, CAPACITY).expect("served"), 195.0),
        "{:?}",
        served(&reg, CAPACITY)
    );
    assert!(close(served(&reg, DRONE_BAY).expect("served"), 25.0));
}

#[test]
fn an_expander_going_online_pushes_the_new_capacity_once() {
    let mut reg = world();
    crate::fitstate::around(&mut reg, Some(ALICE), |reg| {
        set_online_kept(reg, M_PASSIVE, false);
    });
    let off = changes_of(&mut reg, CAPACITY);
    assert_eq!(off.len(), 1, "{off:?}");
    assert!(close(off[0].0, 130.0) && close(off[0].1, 195.0), "{off:?}");
    crate::fitstate::around(&mut reg, Some(ALICE), |reg| {
        set_online_kept(reg, M_PASSIVE, true);
    });
    let on = changes_of(&mut reg, CAPACITY);
    assert_eq!(on.len(), 1, "{on:?}");
    assert!(close(on[0].0, 195.0) && close(on[0].1, 130.0), "{on:?}");
    // Nothing moved, nothing said.
    crate::fitstate::around(&mut reg, Some(ALICE), |_| ());
    assert!(changes_of(&mut reg, CAPACITY).is_empty());
}

#[test]
fn no_charged_attribute_comes_from_the_sheet_path() {
    let reg = world();
    for id in [264, 18, 3, 266, 37] {
        assert!(
            !crate::fitstate::SHEET_ATTRIBUTES.contains(&id),
            "{id} is owned elsewhere"
        );
        assert!(
            !crate::fitstate::sheet_attributes(&reg, SHIP)
                .iter()
                .any(|(i, _)| *i == id),
            "{id}"
        );
    }
}

/// A sensor booster with no script runs its own effect: a ship modifier on
/// the lock range (76) from the module's bonus, which godma must be told at
/// the click and told back at the cycle boundary (fci_script_empty).
#[test]
fn a_bare_booster_pushes_the_lock_range_on_and_off() {
    const LOCK_RANGE: i64 = 76;
    let mut reg = world();
    let _ = changes_of(&mut reg, LOCK_RANGE);
    activate(
        &mut reg,
        Some(ALICE),
        M_BOOSTER,
        "inventedBooster",
        None,
        1000,
        T0,
    )
    .expect("booster");
    let on = changes_of(&mut reg, LOCK_RANGE);
    assert_eq!(on.len(), 1, "{on:?}");
    assert!(close(on[0].1, 20000.0) && close(on[0].0, 26000.0), "{on:?}");
    assert!(resonance_changes(&mut reg).is_empty(), "no resonance moved");

    deactivate(
        &mut reg,
        Some(ALICE),
        M_BOOSTER,
        "inventedBooster",
        T0 + SEC,
    )
    .expect("asked");
    assert!(changes_of(&mut reg, LOCK_RANGE).is_empty(), "still running");
    tick(&mut reg, T0 + 6 * SEC);
    let off = changes_of(&mut reg, LOCK_RANGE);
    assert_eq!(off.len(), 1, "{off:?}");
    assert!(
        close(off[0].1, 26000.0) && close(off[0].0, 20000.0),
        "{off:?}"
    );
}
