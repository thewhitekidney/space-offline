//! Tests of the generic heavy-module rules. **Every id and number is
//! invented**, the cycle length and the burn included: they equal no real
//! type's data, and no expected value comes from the client. What is named
//! is a formula (a -100 speed factor holds a ship still, a skill level
//! scales a quantity) or an effect's own shape. No test asserts at which
//! instant of a cycle the fuel leaves: only what a run of cycles burns.

use super::*;
use crate::sov_hub::fixture as hubfx;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
/// Invented cycle length in milliseconds, and in blue ticks below.
const CYCLE_MS: i64 = 123_000;
const CYCLE: i64 = CYCLE_MS * crate::modules::MSEC;
/// Invented: fuel a cycle takes.
const BURN: i64 = 240;
const PLENTY: i64 = 4_900;

const SYSTEM: i64 = 30_650_001;
const ALICE: i64 = 90_650_001;
const BOB: i64 = 90_650_002;
const CORP: i64 = 98_650_001;

/// A hull with 1973 at 0 (as the capitals carry it), and the frigate's rest.
const HULL: i64 = 990_650_001;
const SIEGE: i64 = 990_650_011;
const BASTION: i64 = 990_650_012;
const BURST: i64 = 990_650_013;
const SKILL: i64 = 990_650_030;
const FUEL: i64 = 990_650_020;
const GROUP_MODULE: i64 = 990_650;
const GROUP_FUEL: i64 = 990_651;

const SIEGE_EFFECT: &str = "moduleBonusSiegeModule";
const BASTION_EFFECT: &str = "moduleBonusBastionModule";
const BURST_EFFECT: &str = "inventedBurstModule";
const BURST_EFFECT_ID: i64 = 990_650_401;
const SKILL_EFFECT_A: i64 = 990_650_402;
const SKILL_EFFECT_B: i64 = 990_650_403;

const ALICE_SIEGE: i64 = 100_650_101;
const ALICE_BASTION: i64 = 100_650_102;
const ALICE_BURST: i64 = 100_650_103;
const ALICE_CARGO: i64 = 100_650_201;
const ALICE_BAY: i64 = 100_650_202;

const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_650_000 + (who - ALICE) + 1
}

fn content(cap_need: f64) -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let fuelled = format!("713={FUEL}.0;714={BURN}.0;");
    let module =
        |id: i64, group: i64, effect: i64, name: &str, fuel: &str, extra: &str, label: &str| {
            format!(
                "type\t{id}\t{group}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{effect}\t12,16,{effect}\t\
             6={cap_need};20=-100.0;30=1.0;50=1.0;73={CYCLE_MS}.0;182={SKILL}.0;{fuel}763=1.0;\
             852=100.0;906=1.0;1471=7.0;{extra}{name}\t{label}\n"
            )
        };
    let rows = format!(
        "{}attribute\t4\t0.0\tmass\n\
         attribute\t6\t0.0\tcapacitorNeed\n\
         attribute\t20\t0.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t280\t0.0\tskillLevel\n\
         attribute\t1775\t0.0\tinventedQuantityBonus\n\
         attribute\t852\t0.0\tsiegeModeWarpStatus\n\
         attribute\t1471\t1.0\tsiegeMassMultiplier\n\
         attribute\t1973\t-1000.0\tgateScrambleStatus\n\
         attribute\t1974\t1.0\tgateScrambleStrength\n\
         attribute\t2354\t0.0\tdisallowDocking\n\
         type\t{HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=1000000.0;70=3.1;76=21000.0;\
         101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;271=0.0;272=0.5;273=0.6;274=0.8;\
         479=600000.0;482=260.0;564=640.0;600=5.0;1137=3.0;1281=1.0;1973=0.0\tInvented Hull\n\
         type\t{FUEL}\t{GROUP_FUEL}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Fuel\n\
         type\t{SKILL}\t275\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t{SKILL_EFFECT_A},{SKILL_EFFECT_B}\t1775=-10.0\tInvented Skill\n\
         {}{}{}\
         effect\t{SKILL_EFFECT_A}\t0\t0\t0\t0\t0\t0\t0\tinventedSkillLevel\t\n\
         effect\t{SKILL_EFFECT_B}\t0\t0\t0\t0\t0\t0\t0\tinventedSkillBonus\t\n\
         modifier\t{SKILL_EFFECT_A}\titemID\tItemModifier\t1775\t280\t0\t\t\n\
         modifier\t{SKILL_EFFECT_B}\tshipID\tLocationRequiredSkillModifier\t714\t1775\t6\t{SKILL}\t\n\
         effect\t{BURST_EFFECT_ID}\t1\t0\t73\t0\t0\t0\t0\t{BURST_EFFECT}\t\n\
         effect\t{EFFECT_SIEGE}\t1\t0\t73\t0\t0\t0\t0\t{SIEGE_EFFECT}\t\n\
         effect\t{EFFECT_BASTION}\t1\t0\t73\t0\t0\t0\t0\t{BASTION_EFFECT}\t\n\
         {}{}",
        crate::typedata::FIXTURE,
        module(
            SIEGE,
            GROUP_MODULE,
            EFFECT_SIEGE,
            "",
            &fuelled,
            "2354=1.0",
            "Invented Siege Module"
        ),
        module(
            BASTION,
            GROUP_MODULE + 2,
            EFFECT_BASTION,
            "",
            "",
            "2354=1.0",
            "Invented Bastion Module"
        ),
        module(
            BURST,
            GROUP_MODULE + 3,
            BURST_EFFECT_ID,
            "",
            "",
            "2354=1.0",
            "Invented Burst Module"
        ),
        modifiers(EFFECT_SIEGE),
        modifiers(EFFECT_BASTION),
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = format!(
        "{SYSTEM}\t10650001\t20650001\t\t\t-0.45\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented Null"
    );
    let u = crate::celestial::Universe::parse(
        &[sys],
        &["10650001\t\t\t\t\tInvented Region".to_owned()],
        &["20650001\t10650001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    c
}

/// The modifiers a siege-like effect carries on its ship, as the data has
/// them (speed, scramble status, docking, gate status, mass).
fn modifiers(effect: i64) -> String {
    format!(
        "modifier\t{effect}\tshipID\tItemModifier\t37\t20\t6\t\t\n\
         modifier\t{effect}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{effect}\tshipID\tItemModifier\t2354\t2354\t2\t\t\n\
         modifier\t{effect}\tshipID\tItemModifier\t1973\t1974\t2\t\t\n\
         modifier\t{effect}\tshipID\tItemModifier\t4\t1471\t4\t\t\n"
    )
}

fn item(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    flag_id: i64,
    quantity: i64,
    singleton: bool,
) -> Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    Item {
        item_id,
        type_id,
        owner_id: ALICE,
        location_id: ship(ALICE),
        flag_id,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
    }
}

struct World {
    reg: Registry,
}

/// Alice in space in the invented hull with a siege and a bastion module,
/// fuel in her cargo and her fuel bay; Bob alone beside her. Pushes drained.
fn world() -> World {
    world_with(0.0)
}

/// As [`world`], the siege and the others needing `cap_need` capacitor a cycle.
fn world_with(cap_need: f64) -> World {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(SYSTEM),
        corporation_id: CORP,
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content(cap_need));
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![pilot(ALICE, "Alice"), pilot(BOB, "Bob")],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(ALICE)) {
        i.type_id = HULL;
        i.group_id = 25;
    }
    items.extend([
        item(&content, ALICE_SIEGE, SIEGE, HI_SLOT_0, 1, true),
        item(&content, ALICE_BASTION, BASTION, HI_SLOT_0 + 1, 1, true),
        item(&content, ALICE_BURST, BURST, HI_SLOT_0 + 2, 1, true),
        item(
            &content,
            ALICE_CARGO,
            FUEL,
            crate::cyno::FLAG_CARGO,
            PLENTY,
            false,
        ),
        item(
            &content,
            ALICE_BAY,
            FUEL,
            crate::cyno::FLAG_FUEL_BAY,
            PLENTY,
            false,
        ),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_651_000,
        ..crate::store::Snapshot::default()
    });
    reg.character_mut(ALICE).expect("a pilot").ship_type_id = HULL;
    for who in [ALICE, BOB] {
        let _ = reg.roster_mut().leave(who);
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship(who),
            SYSTEM,
            Motion::arrived([1.0e7, 2.0e6, -3.0e6], [0.0; 3]),
        ));
    }
    let _ = hubfx::pending(&mut reg);
    let _ = hubfx::due(&mut reg);
    World { reg }
}

impl World {
    fn engage(&mut self, module: i64, effect: &str, t: i64) -> Result<PyValue, ServiceError> {
        crate::modules::activate(&mut self.reg, Some(ALICE), module, effect, None, 1000, t)
    }

    fn fuel(&self, flag: i64) -> i64 {
        self.reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == FUEL && i.owner_id == ALICE && i.flag_id == flag)
            .map(|i| i.quantity)
            .sum()
    }

    fn pushes(&mut self) -> Vec<hubfx::Sent> {
        let mut v = hubfx::pending(&mut self.reg);
        v.extend(hubfx::due(&mut self.reg));
        v
    }
}

fn train(reg: &mut Registry, who: i64, skills: &[(i64, i64)]) {
    let entries = skills
        .iter()
        .map(|&(type_id, level)| crate::skill::SkillEntry {
            type_id,
            level,
            points: 0,
            rank: 1,
        })
        .collect();
    *reg.skills_mut(who).expect("a pilot has skills") = crate::skill::Skills::restore(entries);
}

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

fn named<'a>(sent: &'a [hubfx::Sent], name: &str) -> Vec<&'a hubfx::Sent> {
    sent.iter().filter(|(n, _, _)| *n == name).collect()
}

#[test]
fn a_siege_burns_its_fuel_every_cycle() {
    let mut w = world();
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    let one = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    crate::modules::tick(&mut w.reg, T0 + 2 * CYCLE);
    let two = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    crate::modules::tick(&mut w.reg, T0 + 3 * CYCLE);
    let three = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    assert_eq!(one - two, BURN, "a cycle burns its quantity, from the bay");
    assert_eq!(two - three, BURN);
    assert!(one < PLENTY, "and it has been burning from the start");
    assert_eq!(w.fuel(crate::cyno::FLAG_CARGO), PLENTY, "the bay first");
    assert!(w.reg.modules().running(ALICE_SIEGE).is_some());
}

#[test]
fn a_skill_changes_the_quantity_a_cycle_burns() {
    let mut w = world();
    train(&mut w.reg, ALICE, &[(SKILL, 3)]);
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    let one = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    crate::modules::tick(&mut w.reg, T0 + 2 * CYCLE);
    let two = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    // Three levels at -10 percent each, on the module's live quantity.
    let want = (BURN as f64 * 0.7).round() as i64;
    assert_eq!(one - two, want, "the modified 714, not the type's");
    assert_ne!(want, BURN);
}

#[test]
fn a_cycle_stopped_for_energy_burns_no_fuel() {
    // The siege needs more capacitor than the ship has regained by the end of
    // the first cycle (the hull's recharge is slow).
    let mut w = world_with(200.0);
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let _ = w.pushes();
    let held = w.fuel(crate::cyno::FLAG_FUEL_BAY);
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    let sent = w.pushes();
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none(), "stopped");
    assert!(
        named(&sent, "OnGodmaShipEffect").iter().any(|s| s
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains("NotEnoughEnergy")),
        "the stop carries the key: {sent:?}"
    );
    assert_eq!(
        w.fuel(crate::cyno::FLAG_FUEL_BAY),
        held,
        "the cycle that never started burned nothing"
    );
}

#[test]
fn no_fuel_refuses_the_click_and_stops_the_module_with_the_key() {
    let mut w = world();
    // No fuel at all: the click is refused.
    let keep: Vec<Item> = w
        .reg
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id != FUEL)
        .cloned()
        .collect();
    w.reg.apply_snapshot(crate::store::Snapshot {
        items: keep.clone(),
        next_item_id: 100_651_000,
        ..crate::store::Snapshot::default()
    });
    let e = w
        .engage(ALICE_SIEGE, SIEGE_EFFECT, T0)
        .expect_err("no fuel");
    assert_eq!(raised(&e).0, "ModuleRequiresFuel");
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none());
    // One cycle's worth and no more: the module runs out within a few
    // cycles, and the stop carries the key.
    let mut items = keep;
    items.push(item(
        &content(0.0),
        ALICE_CARGO,
        FUEL,
        crate::cyno::FLAG_CARGO,
        BURN,
        false,
    ));
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_651_000,
        ..crate::store::Snapshot::default()
    });
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("one cycle");
    let _ = w.pushes();
    for n in 1..=3 {
        crate::modules::tick(&mut w.reg, T0 + n * CYCLE);
    }
    let sent = w.pushes();
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none(), "stopped");
    assert!(
        named(&sent, "OnGodmaShipEffect").iter().any(|s| s
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains("ModuleRequiresFuel")),
        "the stop carries the key: {sent:?}"
    );
    assert_eq!(w.fuel(crate::cyno::FLAG_CARGO), 0, "all of it burned");
}

#[test]
fn a_bastion_burns_nothing() {
    let mut w = world();
    w.engage(ALICE_BASTION, BASTION_EFFECT, T0)
        .expect("engaged");
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    crate::modules::tick(&mut w.reg, T0 + 2 * CYCLE);
    assert_eq!(w.fuel(crate::cyno::FLAG_FUEL_BAY), PLENTY);
    assert_eq!(w.fuel(crate::cyno::FLAG_CARGO), PLENTY);
    assert!(
        w.reg.modules().running(ALICE_BASTION).is_some(),
        "it keeps cycling without fuel"
    );
}

#[test]
fn early_deactivation_says_the_time_left() {
    let mut w = world();
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let click = T0 + 100 * crate::modules::MSEC * 1000;
    let e = crate::modules::deactivate(&mut w.reg, Some(ALICE), ALICE_SIEGE, SIEGE_EFFECT, click)
        .expect_err("not at will");
    let (key, d) = raised(&e);
    assert_eq!(key, "EffectStillActive");
    // Blue ticks: what is left of the 5-minute cycle, 100 s in.
    assert_eq!(
        flat(arg(&d, "timeLeft")),
        format!("{} ", CYCLE - 100 * 10_000_000)
    );
    assert_eq!(arg(&d, "moduleTypeID"), &PyValue::Int(SIEGE));
    // The stop is booked for the boundary all the same, and a second click
    // is told the same.
    assert_eq!(
        w.reg.modules().running(ALICE_SIEGE).and_then(|r| r.stop_at),
        Some(T0 + CYCLE)
    );
    let again = crate::modules::deactivate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        SIEGE_EFFECT,
        click + 10_000_000,
    )
    .expect_err("still not");
    assert_eq!(raised(&again).0, "EffectStillActive");
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none(), "it went");
}

#[test]
fn a_sieged_ship_does_not_dock() {
    let mut w = world();
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let e = refuse_dock(&w.reg, Some(ALICE)).expect_err("sieged");
    let (key, d) = raised(&e);
    assert_eq!(key, "CantDockWhileModuleActive");
    assert_eq!(arg(&d, "moduleType"), &PyValue::Int(SIEGE));
    assert!(refuse_dock(&w.reg, Some(BOB)).is_ok(), "another ship");
    assert!(refuse_dock(&w.reg, None).is_ok());
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    // Deactivated and gone: the ship may dock again.
    let _ = crate::modules::deactivate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        SIEGE_EFFECT,
        T0 + CYCLE,
    );
    crate::modules::tick(&mut w.reg, T0 + 2 * CYCLE);
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
}

#[test]
fn a_module_that_only_carries_2354_does_not_bar_the_dock() {
    // The ship's own 2354 is what counts, as the effect raises it: a module
    // whose effect does not touch the ship leaves the pilot free to dock.
    let mut w = world();
    w.engage(ALICE_BURST, BURST_EFFECT, T0).expect("engaged");
    assert!(w.reg.modules().running(ALICE_BURST).is_some());
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
}

#[test]
fn the_park_hears_the_speed_and_mass() {
    let mut w = world();
    let before = crate::dogma::hull(&w.reg, ship(ALICE)).expect("a sheet");
    assert!(before.max_velocity > 0.0);
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let held = crate::dogma::hull(&w.reg, ship(ALICE)).expect("a sheet");
    assert_eq!(held.max_velocity, 0.0, "speedFactor -100 post-percent");
    assert!(held.mass > before.mass * 5.0, "the mass multiplier");
    let said: String = w
        .pushes()
        .iter()
        .filter(|s| s.0 == "DoDestinyUpdate")
        .flat_map(|s| s.1.iter().map(flat))
        .collect();
    assert!(said.contains("SetMaxSpeed"), "{said}");
    assert!(said.contains("SetBallMass"), "{said}");
    // And told again when it ends.
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    let _ = crate::modules::deactivate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        SIEGE_EFFECT,
        T0 + CYCLE,
    );
    let _ = w.pushes();
    crate::modules::tick(&mut w.reg, T0 + 2 * CYCLE);
    let said: String = w
        .pushes()
        .iter()
        .filter(|s| s.0 == "DoDestinyUpdate")
        .flat_map(|s| s.1.iter().map(flat))
        .collect();
    assert!(said.contains("SetMaxSpeed"), "{said}");
}

#[test]
fn a_siege_ending_under_another_still_motion_module_tells_the_park() {
    // Two modules hold the ship; the siege ends and the mass changes while
    // the other still runs.
    let mut w = world();
    w.engage(ALICE_BASTION, BASTION_EFFECT, T0)
        .expect("bastion");
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("siege");
    let _ = w.pushes();
    let click = T0 + CYCLE / 2;
    let _ = crate::modules::deactivate(&mut w.reg, Some(ALICE), ALICE_SIEGE, SIEGE_EFFECT, click);
    let _ = w.pushes();
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none(), "it went");
    assert!(w.reg.modules().running(ALICE_BASTION).is_some());
    let said: String = w
        .pushes()
        .iter()
        .filter(|s| s.0 == "DoDestinyUpdate")
        .flat_map(|s| s.1.iter().map(flat))
        .collect();
    assert!(said.contains("SetBallMass"), "{said}");
}

#[test]
fn a_stop_booked_for_an_earlier_run_is_dropped() {
    let mut w = world();
    train(&mut w.reg, ALICE, &[(SKILL, 1)]);
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let _ = crate::modules::deactivate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        SIEGE_EFFECT,
        T0 + CYCLE / 2,
    );
    let stale = |p: &dyn crate::notify::Push| {
        p.cancel_tag()
            == Some(crate::notify::CancelTag::ModuleStop {
                module: ALICE_SIEGE,
            })
    };
    assert!(w.reg.has_scheduled(stale), "the click booked its stop");
    // The run ends some other way, and the pilot starts it again.
    crate::modules::deactivate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        "online",
        T0 + CYCLE / 2 + 1,
    )
    .expect("offline");
    assert!(w.reg.modules().running(ALICE_SIEGE).is_none());
    crate::modules::activate(
        &mut w.reg,
        Some(ALICE),
        ALICE_SIEGE,
        "online",
        None,
        1,
        T0 + CYCLE / 2 + 2,
    )
    .expect("online");
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0 + CYCLE / 2 + 3)
        .expect("engaged again");
    assert!(
        !w.reg.has_scheduled(stale),
        "the old run's stop no longer waits to tell the client this run has stopped"
    );
}

#[test]
fn a_sieged_capital_refuses_warp_gate_and_jump() {
    let mut w = world();
    let warp = &crate::movement_guard::WARP_GUARDS;
    let gate = &crate::movement_guard::JUMP_GUARDS;
    assert!(crate::movement_guard::check(warp, &w.reg, ALICE).is_none());
    assert!(crate::movement_guard::check(gate, &w.reg, ALICE).is_none());
    w.engage(ALICE_SIEGE, SIEGE_EFFECT, T0).expect("engaged");
    let r = crate::movement_guard::check(warp, &w.reg, ALICE).expect("no warp");
    assert_eq!(r.user_error, "WarpScrambled");
    let r = crate::movement_guard::check(gate, &w.reg, ALICE).expect("no gate");
    assert_eq!(r.user_error, "GateScrambled");
}

#[test]
fn the_siege_family_is_the_five_effects() {
    for e in [
        EFFECT_SIEGE,
        EFFECT_TRIAGE,
        EFFECT_BASTION,
        EFFECT_INDUSTRIAL_CORE,
        EFFECT_COMPACT_CORE,
    ] {
        assert!(is_siege_like(e));
    }
    assert!(!is_siege_like(crate::cyno::EFFECT_CYNOSURAL_GENERATION));
}
