//! LOCAL-TANK's tests: the cap booster and the ancillary repairers that run
//! empty. **Every id and number here is invented**: types 990,710,xxx, a
//! 5 s cycle, a 4-round `chargeRate`, a cap charge of 150.

use super::*;
use crate::damage::{Damage, Profile, ShipDamage};
use crate::space_state::{Motion, Presence};
use crate::tackle::Live;

const T0: i64 = 133_000_000_000_000_000;
const SEC: i64 = 1_000 * MSEC;
const ALICE: i64 = 90_710_001;
const SHIP: i64 = 100_710_001;
const SYSTEM: i64 = 30_710_001;
const FRIGATE: i64 = 990_000_001;
const LO_SLOT_0: i64 = 11;
const HI_SLOT_0: i64 = 27;

const GROUP_PASTE: i64 = 990_710;
const GROUP_AAR: i64 = 990_711;
const GROUP_BOOSTER: i64 = 990_712;
const GROUP_CAP_CHARGE: i64 = 990_713;
const AAR: i64 = 990_710_001;
const PASTE: i64 = 990_710_002;
const BOOSTER: i64 = 990_710_003;
const CAP_CHARGE: i64 = 990_710_004;
const M_AAR: i64 = 100_710_101;
const M_BOOSTER: i64 = 100_710_102;
const SCRIPTED: i64 = 990_710_005;
const SCRIPT: i64 = 990_710_006;
const GROUP_SCRIPTED: i64 = 990_715;
/// A script group the module may run without (909).
const GROUP_SCRIPT: i64 = 909;
const E_SCRIPTED: i64 = 990_710;
const M_SCRIPTED: i64 = 100_710_103;
const HI_SLOT_1: i64 = 28;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    // type id, group, category, volume, capacity, default effect, attrs, name
    let ty = |id: i64, group: i64, cat: i64, vol: f64, cap: f64, effect: i64, attrs: &str| {
        // A module is slotted by its rack effect (11 low, 12 high).
        let list = match (effect, cat) {
            (0, _) => String::new(),
            (5275, _) => format!("11,{effect}"),
            _ => format!("12,{effect}"),
        };
        format!(
            "type\t{id}\t{group}\t{cat}\t1\t1\t{vol}\t{cap}\t0.0\t1.0\t{effect}\t{list}\t{attrs}\tInvented {id}\n"
        )
    };
    let eff = |id: i64, name: &str| format!("effect\t{id}\t1\t0\t73\t0\t0\t0\t0\t{name}\t1\n");
    let rows = [
        crate::typedata::FIXTURE,
        // An ancillary armor repairer: 100 armour, x3 with paste, 4 rounds
        // a cycle, 8 s reload, room for 40 rounds of 0.0025.
        &ty(
            AAR,
            GROUP_AAR,
            7,
            5.0,
            0.1,
            5275,
            &format!(
                "73=5000.0;6=4.0;84=100.0;1886=3.0;56=4.0;1795=8000.0;128=1.0;604={GROUP_PASTE}.0"
            ),
        ),
        &ty(PASTE, GROUP_PASTE, 8, 0.0025, 0.0, 0, "128=1.0"),
        // A cap booster: no need of its own, takes one charge a cycle.
        &ty(
            BOOSTER,
            GROUP_BOOSTER,
            7,
            5.0,
            0.5,
            48,
            &format!("73=5000.0;6=0.0;56=1.0;1795=10000.0;128=1.0;604={GROUP_CAP_CHARGE}.0"),
        ),
        &ty(
            CAP_CHARGE,
            GROUP_CAP_CHARGE,
            8,
            0.1,
            0.0,
            0,
            "128=1.0;67=150.0",
        ),
        // A script-capable module: no chargeRate, a script group it may run
        // without.
        &ty(
            SCRIPTED,
            GROUP_SCRIPTED,
            7,
            5.0,
            0.5,
            E_SCRIPTED,
            &format!("73=5000.0;6=0.0;604={GROUP_SCRIPT}.0"),
        ),
        &ty(SCRIPT, GROUP_SCRIPT, 8, 0.1, 0.0, 0, "128=1.0"),
        &eff(E_SCRIPTED, "fixtureScripted"),
        &eff(5275, "fueledArmorRepair"),
        &eff(48, "powerBooster"),
    ]
    .concat();
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn world() -> Registry {
    let content = std::sync::Arc::new(content());
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
    for (id, ty, flag) in [
        (M_AAR, AAR, LO_SLOT_0),
        (M_BOOSTER, BOOSTER, HI_SLOT_0),
        (M_SCRIPTED, SCRIPTED, HI_SLOT_1),
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
        next_item_id: 100_711_000,
        ..crate::store::Snapshot::default()
    });
    let _ = reg.roster_mut().leave(ALICE);
    reg.roster_mut().arrive(Presence::arriving(
        ALICE,
        SHIP,
        SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    // Alice's armour is hurt: 600 of 1000 gone.
    let profile = Profile {
        shield_capacity: 100.0,
        shield_recharge_rate: 1.0e12,
        armor_hp: 1000.0,
        hp: 500.0,
        shield_resonance: Damage::UNRESISTED,
        armor_resonance: Damage::UNRESISTED,
        hull_resonance: Damage::UNRESISTED,
    };
    let mut record = ShipDamage::full(profile, T0);
    record.apply(
        Damage {
            kinetic: 700.0,
            ..Damage::ZERO
        },
        T0,
    );
    reg.combat_mut().set_ship(SHIP, record);
    let _ = reg.take_pending();
    let _ = reg.take_due(std::time::Instant::now() + Duration::from_secs(3_600));
    reg
}

/// Put `n` units of `type_id` in the module's slot (`flag`) or the cargo.
fn put(reg: &mut Registry, type_id: i64, flag: i64, n: i64) -> i64 {
    let changes = reg
        .apply_items(&[crate::item::Edit::Create {
            type_id,
            quantity: n,
            to: crate::item::Slot::new(ALICE, SHIP, flag),
        }])
        .expect("created");
    let _ = reg.take_pending();
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("a stack")
}

fn armour_damage(reg: &Registry, at: i64) -> f64 {
    reg.combat()
        .ship(SHIP)
        .expect("a record")
        .rings(at)
        .armor_damage
}

fn rounds(reg: &Registry, flag: i64) -> Option<(i64, i64)> {
    loaded(reg.store(), SHIP, flag).map(|i| (i.type_id, i.quantity))
}

fn go(reg: &mut Registry, module: i64, effect: &str, t: i64) {
    activate(reg, Some(ALICE), module, effect, None, 1000, t).expect("activates");
}

/// **The AAR takes `min(rate, loaded)`, repairs `base x (1 + (mult - 1) x
/// taken / rate)` and keeps running empty.** 100 base, x3, rate 4, two
/// rounds loaded: the first cycle repairs 100 x (1 + 2 x 2/4) = 200, the
/// empty one the base 100, and the module never stops.
#[test]
fn an_ancillary_armor_repairer_runs_empty_at_the_base_amount() {
    let mut reg = world();
    put(&mut reg, PASTE, LO_SLOT_0, 2);
    go(&mut reg, M_AAR, "fueledArmorRepair", T0);
    assert_eq!(rounds(&reg, LO_SLOT_0), None, "both rounds taken");
    let f = reg.modules().fired_at(M_AAR, 0).expect("recorded");
    assert_eq!((f.charge_type_id, f.taken, f.rate), (Some(PASTE), 2, 4));
    crate::remote_rep::reconcile(&mut reg, T0, &Live);
    assert_eq!(armour_damage(&reg, T0), 400.0, "600 - 200");

    tick(&mut reg, T0 + 5 * SEC);
    assert!(
        reg.modules().running(M_AAR).is_some(),
        "an empty one cycles"
    );
    let f = reg.modules().fired_at(M_AAR, 1).expect("recorded");
    assert_eq!((f.charge_type_id, f.taken), (None, 0));
    crate::remote_rep::reconcile(&mut reg, T0 + 5 * SEC, &Live);
    assert_eq!(armour_damage(&reg, T0 + 5 * SEC), 300.0, "400 - 100");

    tick(&mut reg, T0 + 10 * SEC);
    assert!(reg.modules().running(M_AAR).is_some());
    crate::remote_rep::reconcile(&mut reg, T0 + 10 * SEC, &Live);
    assert_eq!(armour_damage(&reg, T0 + 10 * SEC), 200.0, "300 - 100");
}

/// A full load repairs the full multiplied amount: 100 x 3.
#[test]
fn a_fed_ancillary_armor_repairer_repairs_the_full_multiplier() {
    let mut reg = world();
    put(&mut reg, PASTE, LO_SLOT_0, 8);
    go(&mut reg, M_AAR, "fueledArmorRepair", T0);
    crate::remote_rep::reconcile(&mut reg, T0, &Live);
    assert_eq!(armour_damage(&reg, T0), 300.0, "600 - 300");
    assert_eq!(rounds(&reg, LO_SLOT_0), Some((PASTE, 4)));
}

/// It activates with nothing loaded; a booster, which is not ancillary, is
/// refused `NoCharges`.
#[test]
fn an_ancillary_repairer_activates_with_nothing_loaded_and_a_booster_does_not() {
    let mut reg = world();
    go(&mut reg, M_AAR, "fueledArmorRepair", T0);
    assert!(reg.modules().running(M_AAR).is_some());
    let e = activate(
        &mut reg,
        Some(ALICE),
        M_BOOSTER,
        "powerBooster",
        None,
        1000,
        T0,
    )
    .expect_err("a booster with no charge");
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = e else {
        panic!("a raise");
    };
    assert!(
        format!("{header:?}").contains(&format!("{:?}", b"NoCharges".to_vec())),
        "{header:?}"
    );
}

/// **Switched off empty with paste in the hold, it reloads**: the stop lands
/// at the cycle boundary, the paste goes in from the cargo and the reload
/// timer starts.
#[test]
fn an_empty_ancillary_repairer_reloads_when_switched_off() {
    let mut reg = world();
    let stack = put(&mut reg, PASTE, crate::fitting::FLAG_CARGO, 100);
    go(&mut reg, M_AAR, "fueledArmorRepair", T0);
    deactivate(&mut reg, Some(ALICE), M_AAR, "fueledArmorRepair", T0 + SEC).expect("honoured");
    assert_eq!(rounds(&reg, LO_SLOT_0), None, "not while it runs");
    tick(&mut reg, T0 + 5 * SEC);
    assert!(reg.modules().running(M_AAR).is_none());
    let full = rounds_capacity_in(reg.content(), AAR, PASTE);
    assert!(full > 0 && full < 100, "{full}");
    assert_eq!(rounds(&reg, LO_SLOT_0), Some((PASTE, full)));
    assert_eq!(reg.item(stack).map(|i| i.quantity), Some(100 - full));
    assert!(reg.modules().reloading_until(M_AAR, T0 + 5 * SEC).is_some());
    let sent: Vec<_> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    assert!(sent.contains(&"OnChargeBeingLoadedToModule"), "{sent:?}");
}

/// Nothing in the hold, nothing to reload; and a loaded one is left alone.
#[test]
fn switching_off_without_paste_in_the_hold_reloads_nothing() {
    let mut reg = world();
    go(&mut reg, M_AAR, "fueledArmorRepair", T0);
    deactivate(&mut reg, Some(ALICE), M_AAR, "fueledArmorRepair", T0 + SEC).expect("honoured");
    tick(&mut reg, T0 + 5 * SEC);
    assert!(reg.modules().running(M_AAR).is_none());
    assert_eq!(rounds(&reg, LO_SLOT_0), None);
    assert!(reg.modules().reloading_until(M_AAR, T0 + 5 * SEC).is_none());
}

/// **A cap booster gives the loaded charge's attribute 67 at the cycle
/// start** and uses one charge: 100 + 150 = 250 at the activation instant.
#[test]
fn a_cap_booster_gives_its_charges_capacitor_bonus_at_activation() {
    let mut reg = world();
    put(&mut reg, CAP_CHARGE, HI_SLOT_0, 5);
    let ship_type = reg.character(ALICE).map_or(0, |c| c.ship_type_id);
    let (_, capacity) = capacitor_at(&reg, SHIP, ship_type, T0);
    assert!(capacity >= 250.0, "the invented hull holds {capacity}");
    reg.modules_mut().capacitor.insert(
        SHIP,
        Charge {
            value: 100.0,
            at: T0,
        },
    );
    go(&mut reg, M_BOOSTER, "powerBooster", T0);
    assert_eq!(rounds(&reg, HI_SLOT_0), Some((CAP_CHARGE, 4)), "one used");
    let f = reg.modules().fired_at(M_BOOSTER, 0).expect("recorded");
    assert_eq!(f.charge_type_id, Some(CAP_CHARGE));
    crate::remote_rep::reconcile(&mut reg, T0, &Live);
    assert!((capacitor_at(&reg, SHIP, ship_type, T0).0 - 250.0).abs() < 1e-6);
    // Reconciling again lands nothing new.
    crate::remote_rep::reconcile(&mut reg, T0, &Live);
    assert!((capacitor_at(&reg, SHIP, ship_type, T0).0 - 250.0).abs() < 1e-6);
}

/// A booster that has run out of charges stops at the next boundary (it is
/// not ancillary), having given each charge once.
#[test]
fn a_cap_booster_stops_when_its_charges_run_out() {
    let mut reg = world();
    put(&mut reg, CAP_CHARGE, HI_SLOT_0, 1);
    reg.modules_mut().capacitor.insert(
        SHIP,
        Charge {
            value: 10.0,
            at: T0,
        },
    );
    go(&mut reg, M_BOOSTER, "powerBooster", T0);
    tick(&mut reg, T0 + 5 * SEC);
    assert!(reg.modules().running(M_BOOSTER).is_none());
}

/// **A module that takes an optional script runs with none loaded**, and
/// cycles on without stopping; a loaded script is never used up (no
/// `chargeRate`). A gun with a required charge group still answers
/// `NoCharges` (the booster test above).
#[test]
fn a_script_module_cycles_empty_and_never_uses_up_a_loaded_script() {
    let mut reg = world();
    go(&mut reg, M_SCRIPTED, "fixtureScripted", T0);
    for n in 1..=3 {
        tick(&mut reg, T0 + n * 5 * SEC);
        assert!(reg.modules().running(M_SCRIPTED).is_some(), "cycle {n}");
    }

    let mut reg = world();
    put(&mut reg, SCRIPT, HI_SLOT_1, 1);
    go(&mut reg, M_SCRIPTED, "fixtureScripted", T0);
    for n in 1..=3 {
        tick(&mut reg, T0 + n * 5 * SEC);
        assert!(reg.modules().running(M_SCRIPTED).is_some(), "cycle {n}");
    }
    assert_eq!(rounds(&reg, HI_SLOT_1), Some((SCRIPT, 1)), "not consumed");
}
