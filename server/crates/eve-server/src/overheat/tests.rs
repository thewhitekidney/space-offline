//! K-OVERHEAT tests: invented rows only. Attribute and effect ids are the
//! client's own protocol vocabulary [V]; every number the rolls and the heat
//! use comes from this file's own rows or from a named const.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::modules::Running;

const ALICE: i64 = 90_000_501;
const SHIP: i64 = 990_501_001;
/// Invented types.
const HULL: i64 = 990_501_101;
const GUN: i64 = 990_501_102;
const PLAIN: i64 = 990_501_103;
const MINER: i64 = 990_501_104;
/// The invented overload effect (category 5): the damage multiplier grows by
/// the module's own `overloadDamageModifier`.
const EFFECT: i64 = 990_501_201;
/// The invented activation effect of the gun.
const ACTIVE: i64 = 990_501_202;
/// Fitted modules: two guns, a plain module and a miner in the high rack.
const G1: i64 = 990_501_301;
const G2: i64 = 990_501_302;
const P1: i64 = 990_501_303;
const M1: i64 = 990_501_304;
const HI: i64 = 27;
/// The invented numbers of the rows.
const HEAT_CAP: f64 = 100.0;
const HEAT_RATE: f64 = 0.5;
const HEAT_DAMAGE: f64 = 4.0;
const MODULE_HP: f64 = 10.0;
const DISSIPATION: f64 = 0.1;
const ATTENUATION: f64 = 0.5;
const OVERLOAD_DAMAGE_PCT: f64 = 20.0;
const GUN_DAMAGE: f64 = 2.0;
const PERIOD_MS: i64 = 4_000;

/// SYS-4: three invented effect beacons, one per heat number they move, each
/// in a system of its own (the content's system-effects table is replaced
/// below). Each multiplies one thing on the ship or on its group-53 modules.
const BEACON_DAMAGE: i64 = 990_501_401;
const BEACON_RATE: i64 = 990_501_402;
const BEACON_CAPACITY: i64 = 990_501_403;
const FX_DAMAGE: i64 = 990_501_411;
const FX_RATE: i64 = 990_501_412;
const FX_CAPACITY: i64 = 990_501_413;
const SYS_DAMAGE: i64 = 990_501_501;
const SYS_RATE: i64 = 990_501_502;
const SYS_CAPACITY: i64 = 990_501_503;
/// The invented Thermodynamics effects: the level into the skill's own bonus
/// (a pre-mul on itself) and the bonus onto the ship's modules (a post-percent
/// on their heat damage), the shape the client's skill has.
const FX_THERMO_LEVEL: i64 = 990_501_421;
const FX_THERMO_BONUS: i64 = 990_501_422;
/// The skill's bonus per level, percent: the fixture row's own number.
const THERMO_PER_LEVEL: f64 = -5.0;
const BEACON_DAMAGE_X: f64 = 1.5;
const BEACON_RATE_X: f64 = 0.5;
const BEACON_CAPACITY_X: f64 = 0.5;

fn rows(required_thermo: f64) -> String {
    format!(
        "{}\
         attribute\t{a}\t1.0\theatGenerationMultiplier\n\
         attribute\t64\t1.0\tdamageMultiplier\n\
         attribute\t1267\t6.0\tmoduleRepairRate\n\
         type\t{HULL}\t25\t6\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t1178={HEAT_CAP};1199={HEAT_CAP};1200={HEAT_CAP};1179={DISSIPATION};1196={DISSIPATION};1198={DISSIPATION};1259={ATTENUATION};1261={ATTENUATION};1262={ATTENUATION}\tFixture Hull\n\
         type\t{GUN}\t53\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t{ACTIVE}\t{ACTIVE},{EFFECT}\t9={MODULE_HP};64={GUN_DAMAGE};73={PERIOD_MS}.0;1180={HEAT_RATE};1210={OVERLOAD_DAMAGE_PCT};1211={HEAT_DAMAGE};1212={required_thermo}\tFixture Gun\n\
         type\t{PLAIN}\t53\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t\t9={MODULE_HP}\tFixture Plain\n\
         type\t{MINER}\t54\t7\t1\t1\t1.0\t0.0\t0.0\t1.0\t0\t{EFFECT}\t9={MODULE_HP};77=10.0;1180={HEAT_RATE};1211={HEAT_DAMAGE}\tFixture Miner\n\
         type\t{TYPE_NANITE_PASTE}\t916\t8\t1\t10\t0.01\t0.0\t0.0\t1.0\t0\t\t\tNanite Repair Paste\n\
         type\t28164\t255\t16\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t{FX_THERMO_LEVEL},{FX_THERMO_BONUS}\t280=0.0;1229={THERMO_PER_LEVEL}\tThermodynamics\n\
         type\t{BEACON_DAMAGE}\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{FX_DAMAGE}\t99401={BEACON_DAMAGE_X}\tFixture Damage Beacon\n\
         type\t{BEACON_RATE}\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{FX_RATE}\t99401={BEACON_RATE_X}\tFixture Rate Beacon\n\
         type\t{BEACON_CAPACITY}\t920\t2\t1\t1\t0.0\t0.0\t0.0\t0.0\t0\t{FX_CAPACITY}\t99401={BEACON_CAPACITY_X}\tFixture Capacity Beacon\n\
         effect\t{FX_THERMO_LEVEL}\t0\t0\t0\t0\t0\t0\t0\tfixtureThermoLevel\t\n\
         effect\t{FX_THERMO_BONUS}\t0\t0\t0\t0\t0\t0\t0\tfixtureThermoBonus\t\n\
         modifier\t{FX_THERMO_LEVEL}\titemID\tItemModifier\t1229\t280\t0\t\t\n\
         modifier\t{FX_THERMO_BONUS}\tshipID\tLocationModifier\t{ATTR_HEAT_DAMAGE}\t1229\t6\t\t\n\
         effect\t{FX_DAMAGE}\t7\t0\t0\t0\t0\t0\t0\tfixtureBeaconDamage\t\n\
         effect\t{FX_RATE}\t7\t0\t0\t0\t0\t0\t0\tfixtureBeaconRate\t\n\
         effect\t{FX_CAPACITY}\t7\t0\t0\t0\t0\t0\t0\tfixtureBeaconCapacity\t\n\
         modifier\t{FX_DAMAGE}\tshipID\tLocationGroupModifier\t{ATTR_HEAT_DAMAGE}\t99401\t4\t\t53\n\
         modifier\t{FX_RATE}\tshipID\tLocationGroupModifier\t{ATTR_HEAT_RATE}\t99401\t4\t\t53\n\
         modifier\t{FX_CAPACITY}\tshipID\tItemModifier\t1178\t99401\t4\t\t\n\
         effect\t{ACTIVE}\t1\t0\t73\t0\t0\t0\t0\tfixtureActive\teffects.Fixture\n\
         effect\t{EFFECT}\t5\t0\t0\t0\t0\t0\t0\tfixtureOverload\t\n\
         modifier\t{EFFECT}\titemID\tItemModifier\t64\t1210\t6\t\t\n",
        crate::typedata::FIXTURE,
        a = ATTR_GENERATION,
    )
}

fn content(required_thermo: f64) -> std::sync::Arc<crate::content::Content> {
    let mut content = crate::content::Content::fixture();
    let table = crate::typedata::Table::parse(&rows(required_thermo)).expect("the rows parse");
    content.insert("typedata", crate::typedata::Installed::build(table));
    let beacons = [
        format!("{SYS_DAMAGE}\t{BEACON_DAMAGE}\t\t\t0.0\t0.0\t0.0"),
        format!("{SYS_RATE}\t{BEACON_RATE}\t\t\t0.0\t0.0\t0.0"),
        format!("{SYS_CAPACITY}\t{BEACON_CAPACITY}\t\t\t0.0\t0.0\t0.0"),
    ];
    content.insert(
        crate::system_effect::TABLE.name,
        crate::system_effect::Effects::parse(&beacons).expect("the beacon rows parse"),
    );
    std::sync::Arc::new(content)
}

/// Alice flying the invented hull in space, two guns, a plain module and a
/// miner fitted in the high rack.
fn world_with(required_thermo: f64) -> Registry {
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: SHIP,
        ship_type_id: HULL,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![alice],
        ..SandboxConfig::default()
    })
    .with_content(content(required_thermo));
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == SHIP) {
        i.type_id = HULL;
        i.group_id = 25;
    }
    for (id, type_id, group, flag) in [
        (G1, GUN, 53, HI),
        (G2, GUN, 53, HI + 1),
        (P1, PLAIN, 53, HI + 2),
        (M1, MINER, 54, HI + 4),
    ] {
        items.push(crate::item::Item {
            item_id: id,
            type_id,
            owner_id: ALICE,
            location_id: SHIP,
            flag_id: flag,
            quantity: 1,
            group_id: group,
            category_id: 7,
            singleton: true,
        });
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: M1 + 100,
        ..crate::store::Snapshot::default()
    });
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    let _ = reg.take_pending();
    let _ = reg.take_due(far());
    reg
}

fn world() -> Registry {
    world_with(0.0)
}

fn now() -> i64 {
    crate::pyutil::filetime_now()
}

fn far() -> std::time::Instant {
    std::time::Instant::now() + Duration::from_secs(3_600)
}

type Sent = (&'static str, Vec<PyValue>, Option<Vec<i64>>);

/// Everything pushed since the last call: answers and the clock's.
fn sent(reg: &mut Registry) -> Vec<Sent> {
    let mut out: Vec<Sent> = reg
        .take_pending()
        .into_iter()
        .map(|(_, p)| {
            let (n, a) = p.to_push();
            (n, a, p.audience())
        })
        .collect();
    out.extend(reg.take_due(far()).into_iter().map(|p| {
        let (n, a) = p.to_push();
        (n, a, p.audience())
    }));
    out
}

fn run(reg: &mut Registry, module: i64, cycles: i64, at: i64) {
    reg.modules_mut().insert_running(Running {
        character_id: ALICE,
        ship_id: SHIP,
        module_id: module,
        type_id: GUN,
        effect: crate::modules::Effect {
            effect_id: ACTIVE,
            name: "fixtureActive",
            category: 1,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        },
        target_id: None,
        started: at,
        period: PERIOD_MS * MSEC,
        repeat: 1000,
        cycles,
        stop_at: None,
    });
}

fn tick_at(reg: &mut Registry, t: i64) {
    tick(reg, std::time::Instant::now(), t);
}

/// The values of every heat attribute change sent, `(attribute, new)`.
fn heat_values(sent: &[Sent]) -> Vec<(i64, f64)> {
    changes(sent)
        .into_iter()
        .filter(|(_, a, ..)| ATTR_HEAT.contains(a))
        .map(|(_, a, n, _)| (a, n))
        .collect()
}

/// Every change of every `OnModuleAttributeChanges`, `(item, attribute, new,
/// old)`.
fn changes(sent: &[Sent]) -> Vec<(i64, i64, f64, f64)> {
    let mut out = Vec::new();
    for (n, a, _) in sent {
        if *n != "OnModuleAttributeChanges" {
            continue;
        }
        let PyValue::List(list) = &a[0] else {
            panic!("a list")
        };
        for c in list {
            let PyValue::Tuple(t) = c else {
                panic!("a tuple")
            };
            assert_eq!(t.len(), 8, "eight-tuples: cdl:391 unpacks eight");
            let (PyValue::Int(i), PyValue::Int(at), PyValue::Real(nv), PyValue::Real(ov)) =
                (&t[2], &t[3], &t[5], &t[6])
            else {
                panic!("shapes: {t:?}")
            };
            out.push((*i, *at, *nv, *ov));
        }
    }
    out
}

/// `(module, effect, start)` of every `OnGodmaShipEffect`.
fn effects(sent: &[Sent]) -> Vec<(i64, i64, bool)> {
    sent.iter()
        .filter(|(n, ..)| *n == "OnGodmaShipEffect")
        .map(|(_, a, _)| match (&a[0], &a[1], &a[3]) {
            (PyValue::Int(m), PyValue::Int(e), PyValue::Int(s)) => (*m, *e, *s == 1),
            other => panic!("shape {other:?}"),
        })
        .collect()
}

/// `(heatID, moduleID, added)` of every `OnHeatAdded` / `OnHeatRemoved`.
fn sources(sent: &[Sent]) -> Vec<(i64, i64, bool)> {
    sent.iter()
        .filter(|(n, ..)| *n == "OnHeatAdded" || *n == "OnHeatRemoved")
        .map(|(n, a, _)| match (&a[0], &a[1]) {
            (PyValue::Int(h), PyValue::Int(m)) => (*h, *m, *n == "OnHeatAdded"),
            other => panic!("shape {other:?}"),
        })
        .collect()
}

/// An error as text: its `Debug`, with every byte string spelled out (the
/// keys and argument names travel as bytes).
fn text(e: &ServiceError) -> String {
    let d = format!("{e:?}");
    let mut out = String::new();
    let mut rest = d.as_str();
    while let Some(at) = rest.find("Bytes([") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "Bytes([".len()..];
        let end = after.find("])").expect("a closed byte list");
        let bytes: Vec<u8> = after[..end]
            .split(", ")
            .filter_map(|n| n.parse().ok())
            .collect();
        out.push_str(&String::from_utf8_lossy(&bytes));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn refused(r: Result<PyValue, ServiceError>) -> String {
    text(&r.expect_err("refused"))
}

// -- the client's formula --------------------------------------------------

/// `CalculateHeat` [V], by hand: heating from nought for `t` seconds at `inc`
/// with generation `g` is `cap * (1 - exp(-t * inc * g))`.
#[test]
fn heat_climbs_towards_the_capacity_as_the_client_computes_it() {
    let h = calculate_heat(0.0, 10_000.0, 0.5, 0.1, 2.0, 100.0);
    assert!((h - 100.0 * (1.0 - (-10.0f64).exp())).abs() < 1e-9, "{h}");
    // From a warm start the old heat decays inside the same exponential.
    let h = calculate_heat(50.0, 1_000.0, 0.5, 0.1, 1.0, 100.0);
    let e = (-0.5f64).exp();
    assert!((h - (100.0 - 100.0 * e + 50.0 * e)).abs() < 1e-9);
}

#[test]
fn heat_decays_by_the_dissipation_and_reaches_nought() {
    let h = calculate_heat(80.0, 5_000.0, 0.0, 0.1, 1.0, 100.0);
    assert!((h - 80.0 * (-0.5f64).exp()).abs() < 1e-9);
    // Under half a unit rounds to nought, as the client's `round(h, 0) <= 0`.
    assert_eq!(calculate_heat(0.4, 1.0, 0.0, 0.1, 1.0, 100.0), 0.0);
    assert!(calculate_heat(0.6, 1.0, 0.0, 0.0, 1.0, 100.0) > 0.0);
    // An incoming below the client's epsilon is no heating.
    assert!(calculate_heat(10.0, 1_000.0, 1e-9, 0.1, 1.0, 100.0) < 10.0);
}

#[test]
fn a_hit_falls_off_by_slot_distance() {
    assert_eq!(hit_chance(100.0, 100.0, 0.5, 0), 1.0);
    assert_eq!(hit_chance(100.0, 100.0, 0.5, 2), 0.25);
    assert_eq!(hit_chance(50.0, 100.0, 0.5, 1), 0.25);
    assert_eq!(hit_chance(0.0, 100.0, 0.5, 0), 0.0);
    assert_eq!(hit_chance(10.0, 0.0, 0.5, 0), 0.0);
}

// -- the calls -------------------------------------------------------------

#[test]
fn overload_lights_the_effect_and_returns_the_module() {
    let mut reg = world();
    let t = now();
    let r = overload(&mut reg, Some(ALICE), G1, EFFECT, t).expect("overloads");
    assert_eq!(r, PyValue::Int(G1));
    assert!(is_overloaded(&reg, SHIP, G1));
    let s = sent(&mut reg);
    assert_eq!(effects(&s), [(G1, EFFECT, true)]);
    // Told to the pilot alone, in the eleven-argument shape of the client.
    let e = s.iter().find(|(n, ..)| *n == "OnGodmaShipEffect").unwrap();
    assert_eq!(e.2, Some(vec![ALICE]));
    assert_eq!(e.1.len(), 11);
    // An effect id of 0 names the module's own.
    stop_overload(&mut reg, Some(ALICE), G1, EFFECT, t + 1).expect("stops");
    overload(&mut reg, Some(ALICE), G1, 0, t + 2).expect("by default");
    assert!(is_overloaded(&reg, SHIP, G1));
}

#[test]
fn stopping_tells_the_stop_and_a_second_stop_tells_nothing() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, EFFECT, t).unwrap();
    let _ = sent(&mut reg);
    assert_eq!(
        stop_overload(&mut reg, Some(ALICE), G1, EFFECT, t + 1).unwrap(),
        PyValue::Int(G1)
    );
    assert!(!is_overloaded(&reg, SHIP, G1));
    assert_eq!(effects(&sent(&mut reg)), [(G1, EFFECT, false)]);
    stop_overload(&mut reg, Some(ALICE), G1, EFFECT, t + 2).unwrap();
    assert!(effects(&sent(&mut reg)).is_empty());
}

#[test]
fn what_cannot_be_overloaded_is_refused() {
    let mut reg = world();
    let t = now();
    // No overload effect on the type.
    assert!(refused(overload(&mut reg, Some(ALICE), P1, 0, t)).contains("CustomNotify"));
    // A mining module has no heat system.
    assert!(refused(overload(&mut reg, Some(ALICE), M1, 0, t)).contains("CustomNotify"));
    // Another effect id.
    assert!(overload(&mut reg, Some(ALICE), G1, EFFECT + 1, t).is_err());
    // Offline.
    reg.modules_mut().set_online(G1, false);
    assert!(refused(overload(&mut reg, Some(ALICE), G1, 0, t)).contains("ModuleNotOnline"));
    reg.modules_mut().set_online(G1, true);
    // Not on this ship.
    assert!(overload(&mut reg, Some(ALICE), 12345, 0, t).is_err());
    assert!(!is_overloaded(&reg, SHIP, G1));
    assert!(effects(&sent(&mut reg)).is_empty());
}

fn train(reg: &mut Registry, level: i64) {
    reg.skills.insert(
        ALICE,
        crate::skill::Skills::restore(vec![crate::skill::SkillEntry {
            type_id: TYPE_THERMODYNAMICS,
            level,
            points: 0,
            rank: 1,
        }]),
    );
}

#[test]
fn the_thermodynamics_skill_gates_the_toggle_with_the_clients_key() {
    let mut reg = world_with(3.0);
    let t = now();
    let e = refused(overload(&mut reg, Some(ALICE), G1, 0, t));
    assert!(e.contains("DontHaveThermoDynamicsSkill"), "{e}");
    assert!(e.contains("skillLevel"), "{e}");
    train(&mut reg, 2);
    assert!(overload(&mut reg, Some(ALICE), G1, 0, t).is_err());
    train(&mut reg, 3);
    assert!(overload(&mut reg, Some(ALICE), G1, 0, t).is_ok());
}

#[test]
fn a_rack_call_changes_and_tells_exactly_the_modules_it_returns() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G2, 0, t).unwrap();
    let _ = sent(&mut reg);
    // The rack: G1 and G2 have the effect; the plain module and the miner do
    // not. G2 is on already, so only G1 changes and only G1 is told.
    let r = overload_rack(&mut reg, Some(ALICE), P1, t + 1).expect("the rack");
    assert_eq!(r, PyValue::List(vec![PyValue::Int(G1)]));
    assert_eq!(effects(&sent(&mut reg)), [(G1, EFFECT, true)]);
    assert!(is_overloaded(&reg, SHIP, G1) && is_overloaded(&reg, SHIP, G2));
    // Stopping returns and tells the two that were on.
    let r = stop_overload_rack(&mut reg, Some(ALICE), G1, t + 2).expect("stops");
    assert_eq!(r, PyValue::List(vec![PyValue::Int(G1), PyValue::Int(G2)]));
    assert_eq!(
        effects(&sent(&mut reg)),
        [(G1, EFFECT, false), (G2, EFFECT, false)]
    );
    // Nothing left to stop: nothing returned for the client to flash.
    let r = stop_overload_rack(&mut reg, Some(ALICE), G1, t + 3).expect("again");
    assert_eq!(r, PyValue::List(vec![]));
    assert!(effects(&sent(&mut reg)).is_empty());
}

#[test]
fn an_overloaded_module_has_the_overload_bonus_in_the_engine() {
    let mut reg = world();
    let t = now();
    let base = crate::dogma::sheet(&reg, SHIP)
        .expect("a sheet")
        .module(G1, 64);
    assert_eq!(base, Some(GUN_DAMAGE));
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    let hot = crate::dogma::sheet(&reg, SHIP)
        .expect("a sheet")
        .module(G1, 64)
        .expect("the attribute");
    assert!(
        (hot - GUN_DAMAGE * (1.0 + OVERLOAD_DAMAGE_PCT / 100.0)).abs() < 1e-9,
        "{hot}"
    );
    // And back.
    stop_overload(&mut reg, Some(ALICE), G1, 0, t + 1).unwrap();
    assert_eq!(
        crate::dogma::sheet(&reg, SHIP).unwrap().module(G1, 64),
        Some(GUN_DAMAGE)
    );
}

#[test]
fn a_module_that_is_primed_hot_shows_its_overload_effect_and_damage() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    reg.ext_mut::<Heat>().set_damage(G1, 1.5);
    let c = reg.character(ALICE).unwrap().clone();
    let i = reg.item(G1).unwrap().clone();
    let row = format!("{:?}", crate::modules::module_info(&reg, &c, &i));
    assert!(
        row.contains("Real(1.5)"),
        "the damage is a served attribute"
    );
    assert!(
        row.contains(&format!("Int({EFFECT})")),
        "the effect is active"
    );
    let lines = overload_lines(&reg, &c, G1, GUN);
    assert_eq!(lines.len(), 1);
    assert!(overload_lines(&reg, &c, G2, GUN).is_empty());
}

// -- heat ---------------------------------------------------------------------

/// The first words: the value, then the source added; heat then climbs only
/// while the module cycles; leaving it removes the source.
#[test]
fn a_cycling_overloaded_module_heats_its_rack_and_the_client_is_told_the_source() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    run(&mut reg, G1, 1, t);
    let _ = sent(&mut reg);
    tick_at(&mut reg, t + 1);
    let s = sent(&mut reg);
    assert_eq!(heat_values(&s), [(ATTR_HEAT[0], 0.0)]);
    assert_eq!(sources(&s), [(ATTR_HEAT[0], G1, true)], "the high rack");
    // Ten seconds on, the rack is where the client's own formula puts it.
    let later = t + 10_000 * MSEC;
    tick_at(&mut reg, later);
    let want = calculate_heat(0.0, 10_000.0, HEAT_RATE, DISSIPATION, 1.0, HEAT_CAP);
    let got = reg.ext::<Heat>().rack_value(SHIP, Rack::High);
    assert!((got - want).abs() < 0.01, "{got} vs {want}");
    assert_eq!(reg.ext::<Heat>().rack_value(SHIP, Rack::Medium), 0.0);
    // The module stops cycling: the value is told, then the source goes.
    reg.modules_mut().forget(G1);
    let _ = sent(&mut reg);
    tick_at(&mut reg, later + MSEC);
    let s = sent(&mut reg);
    assert_eq!(sources(&s), [(ATTR_HEAT[0], G1, false)]);
    let told = heat_values(&s);
    assert_eq!(told.len(), 1);
    assert!((told[0].1 - got).abs() < 1.0);
    // Cooling: the value decays to nought.
    tick_at(&mut reg, later + 600_000 * MSEC);
    assert_eq!(reg.ext::<Heat>().rack_value(SHIP, Rack::High), 0.0);
}

#[test]
fn an_overloaded_module_that_is_not_cycling_heats_nothing() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    let _ = sent(&mut reg);
    tick_at(&mut reg, t + 20_000 * MSEC);
    let s = sent(&mut reg);
    assert!(
        sources(&s).is_empty() && heat_values(&s).is_empty(),
        "{s:?}"
    );
    assert_eq!(reg.ext::<Heat>().rack_value(SHIP, Rack::High), 0.0);
}

#[test]
fn two_modules_heat_a_rack_by_their_summed_rates() {
    let mut reg = world();
    let t = now();
    for m in [G1, G2] {
        overload(&mut reg, Some(ALICE), m, 0, t).unwrap();
        run(&mut reg, m, 1, t);
    }
    tick_at(&mut reg, t + 1);
    let s = sent(&mut reg);
    assert_eq!(
        sources(&s),
        [(ATTR_HEAT[0], G1, true), (ATTR_HEAT[0], G2, true)]
    );
    tick_at(&mut reg, t + 2_000 * MSEC);
    let got = reg.ext::<Heat>().rack_value(SHIP, Rack::High);
    let want = calculate_heat(0.0, 2_000.0, 2.0 * HEAT_RATE, DISSIPATION, 1.0, HEAT_CAP);
    assert!((got - want).abs() < 0.01, "{got} vs {want}");
}

/// A warm rack is told again every [`SYNC`] to cancel the client's drift.
#[test]
fn a_warm_rack_is_told_again_now_and_then() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    run(&mut reg, G1, 1, t);
    tick_at(&mut reg, t + 1);
    let _ = sent(&mut reg);
    let sync = SYNC.as_nanos() as i64 / 100;
    tick_at(&mut reg, t + sync / 2);
    assert!(heat_values(&sent(&mut reg)).is_empty(), "not yet");
    tick_at(&mut reg, t + sync + 2);
    let s = sent(&mut reg);
    assert_eq!(heat_values(&s).len(), 1);
    assert!(sources(&s).is_empty(), "the set did not change");
}

/// Docking, jumping or dying ends the heat and the overload.
#[test]
fn leaving_space_clears_heat_and_overload() {
    let mut reg = world();
    let t = now();
    overload(&mut reg, Some(ALICE), G1, 0, t).unwrap();
    run(&mut reg, G1, 1, t);
    tick_at(&mut reg, t + 1);
    assert!(is_overloaded(&reg, SHIP, G1));
    let _ = reg.roster_mut().leave(ALICE);
    tick_at(&mut reg, t + 5_000 * MSEC);
    assert!(!is_overloaded(&reg, SHIP, G1));
    assert_eq!(reg.ext::<Heat>().rack_value(SHIP, Rack::High), 0.0);
}

// -- heat damage ---------------------------------------------------------------

/// G1 overloaded and cycling, its rack run up to the capacity; the time then.
fn at_full_heat(reg: &mut Registry, t: i64) -> i64 {
    overload(reg, Some(ALICE), G1, 0, t).unwrap();
    run(reg, G1, 1, t);
    tick_at(reg, t + 1);
    let later = t + 200_000 * MSEC;
    tick_at(reg, later);
    assert!((reg.ext::<Heat>().rack_value(SHIP, Rack::High) - HEAT_CAP).abs() < 1.0);
    let _ = sent(reg);
    later
}

/// One more cycle of G1 started: the cycle that ended is rolled.
fn one_more_cycle(reg: &mut Registry, t: i64, at: i64) {
    run(reg, G1, 2, t);
    tick_at(reg, at + 1);
}

/// Heat at the top: a cycle that ended rolls a certain hit on the cycling
/// module itself (slot distance 0) and the hit is the module's `heatDamage`.
#[test]
fn a_cycle_at_full_heat_hits_the_cycling_module_for_its_heat_damage() {
    let mut reg = world();
    let t = now();
    let at = at_full_heat(&mut reg, t);
    one_more_cycle(&mut reg, t, at);
    let s = sent(&mut reg);
    let hit: Vec<_> = changes(&s)
        .into_iter()
        .filter(|(i, a, ..)| *i == G1 && *a == ATTR_DAMAGE)
        .collect();
    assert_eq!(hit.len(), 1, "{s:?}");
    assert_eq!(hit[0].2, HEAT_DAMAGE);
    assert_eq!(hit[0].3, 0.0);
    assert_eq!(module_damage(&reg, G1), HEAT_DAMAGE);
    assert!(!burnt_out(&reg, G1), "{HEAT_DAMAGE} of {MODULE_HP}");
    // Nothing new has cycled: no second roll.
    tick_at(&mut reg, at + 1_000);
    assert!(
        changes(&sent(&mut reg))
            .iter()
            .all(|(i, a, ..)| !(*i == G1 && *a == ATTR_DAMAGE))
    );
    // A mining module is never hit, whatever the distance rolls.
    assert_eq!(module_damage(&reg, M1), 0.0);
}

#[test]
fn the_thermodynamics_skill_trims_the_hit() {
    let mut reg = world();
    train(&mut reg, 2);
    let t = now();
    let at = at_full_heat(&mut reg, t);
    one_more_cycle(&mut reg, t, at);
    assert!((module_damage(&reg, G1) - HEAT_DAMAGE * 0.9).abs() < 1e-9);
}

/// One hit at level L is the type row's heat damage times (1 + p L / 100),
/// once: the sheet carries the skill, so a second manual factor would show as
/// base x (1 + p L / 100)^2 (0.81 at level 2). p and the base are the
/// fixture's own numbers.
#[test]
fn thermodynamics_is_applied_once_to_a_hit() {
    for level in [1, 2, 5] {
        let mut reg = world();
        train(&mut reg, level);
        let t = now();
        let at = at_full_heat(&mut reg, t);
        one_more_cycle(&mut reg, t, at);
        let once = HEAT_DAMAGE * (1.0 + THERMO_PER_LEVEL * level as f64 / 100.0);
        let twice = once * (1.0 + THERMO_PER_LEVEL * level as f64 / 100.0);
        let got = module_damage(&reg, G1);
        assert!((got - once).abs() < 1e-9, "level {level}: {got} vs {once}");
        assert!((got - twice).abs() > 1e-3, "level {level}: counted twice");
    }
}

/// A module whose damage reaches its hit points stops, ends its overload,
/// goes offline and cannot be put online until repaired.
#[test]
fn a_burnt_out_module_goes_offline_and_stays_there() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, MODULE_HP - 1.0);
    let at = at_full_heat(&mut reg, t);
    // The rack is hot, but a burnt module heats nothing: run the cycle.
    one_more_cycle(&mut reg, t, at);
    let s = sent(&mut reg);
    assert!(burnt_out(&reg, G1));
    assert_eq!(
        module_damage(&reg, G1),
        MODULE_HP,
        "never past the hit points"
    );
    assert!(!reg.modules().is_online(G1));
    assert!(reg.modules().running(G1).is_none());
    assert!(!is_overloaded(&reg, SHIP, G1));
    // The pilot is told the damage, the overload's end and the module offline.
    let hit = changes(&s)
        .into_iter()
        .find(|(i, a, ..)| *i == G1 && *a == ATTR_DAMAGE)
        .expect("the damage is told");
    assert_eq!((hit.2, hit.3), (MODULE_HP, MODULE_HP - 1.0));
    let e = effects(&s);
    assert!(
        e.contains(&(G1, EFFECT, false)),
        "the overload ended: {e:?}"
    );
    assert!(
        e.contains(&(G1, crate::modules::EFFECT_ONLINE, false)),
        "offline: {e:?}"
    );
    // The client's dogma goes offline on an isOnline change to 0.
    let off = changes(&s)
        .into_iter()
        .find(|(i, a, ..)| *i == G1 && *a == crate::modules::ATTR_IS_ONLINE)
        .expect("isOnline is told");
    assert_eq!((off.2, off.3), (0.0, 1.0));
    // Online again is refused with the client's key.
    let e = text(&refuse_online(&reg, G1).expect_err("burnt"));
    assert!(e.contains("ModuleTooDamagedToBeOnlined"), "{e}");
    assert!(crate::modules::set_module_online(&mut reg, Some(ALICE), G1, true, now()).is_err());
    assert!(!reg.modules().is_online(G1));
    assert!(overload(&mut reg, Some(ALICE), G1, 0, now()).is_err());
    // A burnt module no longer heats the rack: the source is taken off (the
    // client has still been heating it until told), then the rack cools.
    tick_at(&mut reg, t + 400_000 * MSEC);
    tick_at(&mut reg, t + 800_000 * MSEC);
    assert_eq!(reg.ext::<Heat>().rack_value(SHIP, Rack::High), 0.0);
}

// -- repair -------------------------------------------------------------------

fn paste(reg: &mut Registry, n: i64) {
    reg.apply_items(&[crate::item::Edit::Create {
        type_id: TYPE_NANITE_PASTE,
        quantity: n,
        to: crate::item::Slot::new(ALICE, SHIP, FLAG_CARGO),
    }])
    .expect("paste in the hold");
    let _ = sent(reg);
}

fn paste_left(reg: &Registry) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == TYPE_NANITE_PASTE)
        .map(|i| i.quantity)
        .sum()
}

#[test]
fn a_repair_needs_the_module_offline_and_paste_and_costs_it() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, 5.0);
    // Online: the client's key.
    assert!(
        refused(initiate_repair(&mut reg, Some(ALICE), G1, t))
            .contains("CanNotRepairWhileModuleIsOnline")
    );
    reg.modules_mut().set_online(G1, false);
    // No paste: the client's key, naming the paste.
    let e = refused(initiate_repair(&mut reg, Some(ALICE), G1, t));
    assert!(
        e.contains("NotEnoughRepairMaterialToFinishAllRepairs"),
        "{e}"
    );
    paste(&mut reg, 50);
    // Nothing damaged is not a repair.
    assert_eq!(
        initiate_repair(&mut reg, Some(ALICE), G2, t).unwrap(),
        PyValue::Bool(false)
    );
    assert_eq!(
        initiate_repair(&mut reg, Some(ALICE), G1, t).unwrap(),
        PyValue::Bool(true)
    );
    let cost = paste_needed(5.0, MODULE_HP);
    assert!(cost > 0);
    assert_eq!(paste_left(&reg), 50 - cost);
    // Once begun it is not begun twice (and not charged twice).
    assert_eq!(
        initiate_repair(&mut reg, Some(ALICE), G1, t).unwrap(),
        PyValue::Bool(false)
    );
    assert_eq!(paste_left(&reg), 50 - cost);
}

#[test]
fn a_repair_takes_damage_over_rate_minutes_and_clears_the_damage() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, 6.0);
    reg.modules_mut().set_online(G1, false);
    paste(&mut reg, 50);
    initiate_repair(&mut reg, Some(ALICE), G1, t).unwrap();
    // The rate is the table's 6 hit points a minute: 6 damage is a minute.
    tick_at(&mut reg, t + 30_000 * MSEC);
    assert_eq!(module_damage(&reg, G1), 6.0, "still being repaired");
    tick_at(&mut reg, t + 60_001 * MSEC);
    assert_eq!(module_damage(&reg, G1), 0.0);
    let c = changes(&sent(&mut reg));
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].0, c[0].1, c[0].2, c[0].3),
        (G1, ATTR_DAMAGE, 0.0, 6.0)
    );
    assert!(refuse_online(&reg, G1).is_ok());
}

#[test]
fn stopping_a_repair_keeps_what_was_done() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, 6.0);
    reg.modules_mut().set_online(G1, false);
    paste(&mut reg, 50);
    initiate_repair(&mut reg, Some(ALICE), G1, t).unwrap();
    let _ = sent(&mut reg);
    assert_eq!(
        stop_repair(&mut reg, Some(ALICE), G1, t + 30_000 * MSEC).unwrap(),
        PyValue::None
    );
    assert!((module_damage(&reg, G1) - 3.0).abs() < 1e-6);
    let c = changes(&sent(&mut reg));
    assert_eq!(c.len(), 1);
    assert!((c[0].2 - 3.0).abs() < 1e-6 && c[0].3 == 6.0);
    // A stop with nothing running is quiet.
    assert!(stop_repair(&mut reg, Some(ALICE), G1, t + 31_000 * MSEC).is_ok());
    assert!(changes(&sent(&mut reg)).is_empty());
}

#[test]
fn many_repairs_sort_into_the_clients_three_lists() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, 5.0);
    reg.ext_mut::<Heat>().set_damage(G2, 5.0);
    reg.modules_mut().set_online(G1, false);
    reg.modules_mut().set_online(G2, false);
    // Paste for one only.
    paste(&mut reg, paste_needed(5.0, MODULE_HP));
    let r = initiate_repair_many(&mut reg, Some(ALICE), &[G1, G2, P1], t).unwrap();
    let PyValue::Tuple(lists) = r else {
        panic!("a tuple")
    };
    assert_eq!(lists[0], PyValue::List(vec![PyValue::Int(G1)]), "begun");
    assert_eq!(lists[1], PyValue::List(vec![PyValue::Int(G2)]), "no paste");
    assert_eq!(
        lists[2],
        PyValue::List(vec![PyValue::Int(P1)]),
        "nothing to do"
    );
}

#[test]
fn no_repair_while_a_module_is_overloaded_and_no_overload_while_repairing() {
    let mut reg = world();
    let t = now();
    reg.ext_mut::<Heat>().set_damage(G1, 5.0);
    reg.modules_mut().set_online(G1, false);
    paste(&mut reg, 50);
    overload(&mut reg, Some(ALICE), G2, 0, t).unwrap();
    let e = refused(initiate_repair(&mut reg, Some(ALICE), G1, t));
    assert!(e.contains("CanNotRepairWhileModulesOverloaded"), "{e}");
    stop_overload(&mut reg, Some(ALICE), G2, 0, t).unwrap();
    initiate_repair(&mut reg, Some(ALICE), G1, t).unwrap();
    let e = refused(overload(&mut reg, Some(ALICE), G2, 0, t));
    assert!(
        e.contains("CanNotOverloadModuleOnShipThatHasModuleRepair"),
        "{e}"
    );
}

// -- the shapes -----------------------------------------------------------------

#[test]
fn the_heat_pushes_have_the_clients_shapes() {
    let (n, a) = HeatSource {
        to: ALICE,
        heat_id: 1175,
        module_id: 9,
        added: true,
    }
    .to_push();
    assert_eq!(
        (n, a),
        ("OnHeatAdded", vec![PyValue::Int(1175), PyValue::Int(9)])
    );
    let (n, _) = HeatSource {
        to: ALICE,
        heat_id: 1175,
        module_id: 9,
        added: false,
    }
    .to_push();
    assert_eq!(n, "OnHeatRemoved");
    let p = Changes {
        to: ALICE,
        time: 77,
        changes: vec![Change {
            item_id: 5,
            attribute_id: 3,
            new_value: 1.0,
            old_value: 0.0,
        }],
    };
    let (n, a) = p.to_push();
    assert_eq!(n, "OnModuleAttributeChanges");
    assert_eq!(
        a,
        vec![PyValue::List(vec![PyValue::Tuple(vec![
            crate::pyutil::bstr("OnModuleAttributeChange"),
            PyValue::Int(ALICE),
            PyValue::Int(5),
            PyValue::Int(3),
            PyValue::Int(77),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::None,
        ])])]
    );
    assert_eq!(p.audience(), Some(vec![ALICE]));
}

// -- SYS-4: the sheet reaches the heat ------------------------------------------

/// Alice's ship, flown in `system` (a beacon's, or none).
fn fly_in(reg: &mut Registry, system: i64) {
    reg.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            ALICE,
            SHIP,
            system,
            crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
        ));
}

/// The rack's heat `secs` seconds after G1 starts overloaded and cycling.
fn heat_after(reg: &mut Registry, secs: i64) -> f64 {
    let t = now();
    overload(reg, Some(ALICE), G1, 0, t).unwrap();
    run(reg, G1, 1, t);
    tick_at(reg, t + 1);
    tick_at(reg, t + secs * 1_000 * MSEC);
    reg.ext::<Heat>().rack_value(SHIP, Rack::High)
}

/// A beacon that multiplies `heatDamage` of the ship's modules changes what a
/// hit does: the roll reads the module's modified sheet, not its type row.
#[test]
fn a_beacon_that_moves_the_heat_damage_changes_the_hit() {
    let mut reg = world();
    fly_in(&mut reg, SYS_DAMAGE);
    let t = now();
    let at = at_full_heat(&mut reg, t);
    one_more_cycle(&mut reg, t, at);
    assert_eq!(module_damage(&reg, G1), HEAT_DAMAGE * BEACON_DAMAGE_X);
    assert_ne!(module_damage(&reg, G1), HEAT_DAMAGE, "the type row alone");
}

/// The same beacon one system over, on a ship the beacon does not reach: the
/// control, so the check above can only pass by reading the sheet.
#[test]
fn without_the_beacon_the_hit_is_the_type_rows() {
    let mut reg = world();
    fly_in(&mut reg, 990_501_999);
    let t = now();
    let at = at_full_heat(&mut reg, t);
    one_more_cycle(&mut reg, t, at);
    assert_eq!(module_damage(&reg, G1), HEAT_DAMAGE);
}

/// A beacon that halves `heatRate` slows the climb of the rack's heat.
#[test]
fn a_beacon_that_moves_the_heat_rate_changes_the_climb() {
    let mut plain = world();
    let mut hot = world();
    fly_in(&mut hot, SYS_RATE);
    let (a, b) = (heat_after(&mut plain, 1), heat_after(&mut hot, 1));
    assert!(a > 0.0 && b > 0.0, "{a} {b}");
    assert!(b < a * 0.75, "half the rate, a lot less heat: {a} {b}");
}

/// A beacon that halves the ship's heat capacity halves the rack's numbers.
#[test]
fn a_beacon_that_moves_the_heat_capacity_changes_the_rack() {
    let mut reg = world();
    let plain = rack_numbers(crate::dogma::sheet(&reg, SHIP).as_deref(), Rack::High);
    assert_eq!(plain.capacity, HEAT_CAP);
    fly_in(&mut reg, SYS_CAPACITY);
    let hot = rack_numbers(crate::dogma::sheet(&reg, SHIP).as_deref(), Rack::High);
    assert_eq!(hot.capacity, HEAT_CAP * BEACON_CAPACITY_X);
    assert_eq!(hot.dissipation, plain.dissipation, "nothing else moved");
}
