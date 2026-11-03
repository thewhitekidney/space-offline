//! The jump drive's tests. **Every id and number here is invented**: the types
//! are 990,650,xxx, the systems 30,650,xxx, the distances and the drive's
//! numbers round-ish values chosen so no product sits on an integer edge. The
//! client's constants appear only by name (the effects, the attribute numbers,
//! the three field types, the light year).

use super::*;
use crate::celestial::LIGHTYEAR;
use crate::space_state::{Motion, Presence};
use std::time::{Duration, Instant};

const T0: i64 = 133_000_000_000_000_000;
/// Invented: a one-minute cyno cycle, and 500 ozone a cycle.
const CYCLE_MS: i64 = 60_000;
const OZONE_PER_CYCLE: i64 = 500;
/// Invented: the field's landing radius, and the cyno's speed limit.
const SPAWN: f64 = 5_000.0;
const LIMIT: f64 = 500.0;

const ORIGIN: i64 = 30_650_001;
const DEST: i64 = 30_650_002;
const HIGH: i64 = 30_650_003;
const FAR: i64 = 30_650_004;
const CORP: i64 = 98_650_001;

/// Invented distances from ORIGIN, in light years (never a whole number:
/// `long(ly * perLY)` must not sit on an edge).
const DEST_LY: f64 = 3.2504;
const HIGH_LY: f64 = 2.0;
const FAR_LY: f64 = 9.0504;

const ALICE: i64 = 90_650_001;
const BOB: i64 = 90_650_002;
const CAROL: i64 = 90_650_003;

const CYNO: i64 = 990_650_001;
const OZONE: i64 = 990_650_002;
const ISOTOPE: i64 = 990_650_003;
const JUMPER: i64 = 990_650_004;
const PICKY: i64 = 990_650_005;
const FUEL_SKILL: i64 = 990_650_006;
const RANGE_SKILL: i64 = 990_650_007;
const GROUP_CYNO: i64 = 990_650;
const GROUP_OZONE: i64 = 990_651;
const GROUP_ISOTOPE: i64 = 990_652;
const GROUP_JUMPER: i64 = 990_653;
const FRIGATE: i64 = 990_000_001;
/// Invented typelist ids: the field types a hull admits.
const LIST_PLAIN: i64 = 990_660;
const LIST_COVERT_ONLY: i64 = 990_661;

/// Invented drive: range, fuel per light year, fatigue multiplier.
const RANGE: f64 = 5.0;
const PER_LY: f64 = 1_000.0;
const MULTIPLIER: f64 = 0.5;

const ALICE_CYNO: i64 = 100_650_101;
const ALICE_OZONE: i64 = 100_650_201;
const BOB_BAY: i64 = 100_650_202;
const BOB_CARGO: i64 = 100_650_203;
const CAROL_CARGO: i64 = 100_650_204;
/// Invented stacks: the bay's and the cargo's.
const BAY: i64 = 2_000;
const CARGO: i64 = 5_000;

const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_650_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let hull = |id: i64, list: i64, name: &str| {
        format!(
            "type\t{id}\t{GROUP_JUMPER}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
             9=310.0;37=355.0;482=260.0;{ATTR_CAN_JUMP}=1.0;{ATTR_FUEL_TYPE}={ISOTOPE}.0;\
             {ATTR_RANGE}={RANGE};{ATTR_FUEL_AMOUNT}={PER_LY};{ATTR_BEACON_TYPELIST}={list}.0;\
             1971={MULTIPLIER}\t{name}\n"
        )
    };
    let rows = format!(
        "{}attribute\t280\t0.0\tskillLevel\n\
         attribute\t870\t0.0\tjumpDriveRangeBonus\n\
         attribute\t1296\t0.0\tconsumptionQuantityBonusPercentage\n\
         type\t{OZONE}\t{GROUP_OZONE}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n\
         type\t{ISOTOPE}\t{GROUP_ISOTOPE}\t4\t1\t1\t0.15\t0.0\t0.0\t1.0\t0\t\t\tInvented Isotope\n\
         type\t{CYNO}\t{GROUP_CYNO}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{ef}\t12,16,{ef}\t\
         20=-100.0;30=1.0;50=1.0;73={CYCLE_MS}.0;{fuel_t}={OZONE}.0;{fuel_q}={OZONE_PER_CYCLE}.0;\
         {mga}=1.0;852=100.0;1163=0.0;1245=1.0;{lim}={LIMIT};{spawn}={SPAWN}\tInvented Cyno Generator\n\
         type\t{FUEL_SKILL}\t275\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t3527,3532\t1296=-10.0\tInvented Fuel Skill\n\
         type\t{RANGE_SKILL}\t275\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t1581,1582\t870=20.0\tInvented Range Skill\n\
         {}{}\
         attribute\t20\t0.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t852\t0.0\tsiegeModeWarpStatus\n\
         attribute\t1163\t1.0\tcanCloak\n\
         effect\t{ef}\t1\t0\t73\t0\t0\t0\t0\t{name}\t\n\
         effect\t3527\t0\t0\t0\t0\t0\t0\t0\tskillConsumptionQuantityBonusPercentageSkillLevel\t\n\
         effect\t3532\t0\t0\t0\t0\t0\t0\t0\tskillJumpDriveConsumptionAmountBonusPercentage\t\n\
         effect\t1581\t0\t0\t0\t0\t0\t0\t0\tjumpDriveSkillsRangeBonus\t\n\
         effect\t1582\t0\t0\t0\t0\t0\t0\t0\tjumpDriveSkillsRangeBonusSkillLevel\t\n\
         modifier\t3527\titemID\tItemModifier\t1296\t280\t0\t\t\n\
         modifier\t3532\tshipID\tItemModifier\t868\t1296\t6\t\t\n\
         modifier\t1582\titemID\tItemModifier\t870\t280\t0\t\t\n\
         modifier\t1581\tshipID\tItemModifier\t867\t870\t6\t\t\n\
         modifier\t{ef}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{ef}\tshipID\tItemModifier\t37\t20\t6\t\t\n\
         modifier\t{ef}\tshipID\tItemModifier\t1163\t1163\t7\t\t\n",
        crate::typedata::FIXTURE,
        hull(JUMPER, LIST_PLAIN, "Invented Jumper"),
        hull(PICKY, LIST_COVERT_ONLY, "Invented Picky Jumper"),
        ef = cyno::EFFECT_CYNOSURAL_GENERATION,
        name = cyno::EFFECT_NAME,
        fuel_t = cyno::ATTR_FUEL_TYPE,
        fuel_q = cyno::ATTR_FUEL_QUANTITY,
        mga = cyno::ATTR_MAX_GROUP_ACTIVE,
        lim = cyno::ATTR_MAX_VELOCITY_ACTIVATION_LIMIT,
        spawn = cyno::ATTR_SPAWN_RADIUS,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, sec: &str, name: &str| {
        format!("{id}\t10650001\t20650001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = crate::celestial::Universe::parse(
        &[
            sys(ORIGIN, "-0.45", "Invented Origin"),
            sys(DEST, "-0.45", "Invented Dest"),
            sys(HIGH, "0.9", "Invented High"),
            sys(FAR, "-0.45", "Invented Far"),
        ],
        &["10650001\t\t\t\t\tInvented Region".to_owned()],
        &["20650001\t10650001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    let at = |id: i64, ly: f64| format!("s\t{id}\t{}\t0.0\t0.0", ly * LIGHTYEAR);
    let centers = Centers::parse(&[
        at(ORIGIN, 0.0),
        at(DEST, DEST_LY),
        at(HIGH, HIGH_LY),
        at(FAR, FAR_LY),
    ])
    .expect("the test centres parse");
    c.insert("centers", centers);
    let mut t = crate::dungeon_data::Table::fixture();
    t.typelists.insert(
        LIST_PLAIN,
        crate::dungeon_data::TypeList {
            inc_types: vec![cyno::TYPE_CYNOSURAL_FIELD],
            ..Default::default()
        },
    );
    t.typelists.insert(
        LIST_COVERT_ONLY,
        crate::dungeon_data::TypeList {
            inc_types: vec![cyno::TYPE_COVERT_CYNOSURAL_FIELD],
            ..Default::default()
        },
    );
    c.insert("dungeon_data", t);
    c
}

fn item(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    owner: i64,
    flag_id: i64,
    quantity: i64,
    singleton: bool,
) -> Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: ship(owner),
        flag_id,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
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

fn call_fleet(
    reg: &mut Registry,
    actor: i64,
    method: &str,
    args: Vec<PyValue>,
    bind: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let bind = bind.map_or(PyValue::None, PyValue::Int);
    reg.dispatch(&Request {
        service: "fleetObjectHandler",
        method,
        args: &args,
        kwargs: &[],
        bind_params: Some(&bind),
        actor: Some(actor),
    })
}

/// Alice (a frigate with a cyno module and ozone) in DEST; Bob (a jumper with
/// isotopes in the bay and the cargo) and Carol (a jumper, no fleet) in
/// ORIGIN. Alice and Bob in one fleet. Every push drained.
fn world() -> Registry {
    let pilot = |id: i64, name: &str, system: i64| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship(id),
        solar_system_id: Some(system),
        corporation_id: CORP,
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", DEST),
            pilot(BOB, "Bob", ORIGIN),
            pilot(CAROL, "Carol", ORIGIN),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(ALICE)) {
        i.type_id = FRIGATE;
        i.group_id = 25;
    }
    for who in [BOB, CAROL] {
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = JUMPER;
            i.group_id = GROUP_JUMPER;
        }
    }
    items.extend([
        item(&content, ALICE_CYNO, CYNO, ALICE, HI_SLOT_0, 1, true),
        item(
            &content,
            ALICE_OZONE,
            OZONE,
            ALICE,
            cyno::FLAG_CARGO,
            10 * OZONE_PER_CYCLE,
            false,
        ),
        item(
            &content,
            BOB_BAY,
            ISOTOPE,
            BOB,
            cyno::FLAG_FUEL_BAY,
            BAY,
            false,
        ),
        item(
            &content,
            BOB_CARGO,
            ISOTOPE,
            BOB,
            cyno::FLAG_CARGO,
            CARGO,
            false,
        ),
        item(
            &content,
            CAROL_CARGO,
            ISOTOPE,
            CAROL,
            cyno::FLAG_CARGO,
            CARGO,
            false,
        ),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_651_000,
        ..crate::store::Snapshot::default()
    });
    for (client, who) in [(1, ALICE), (2, BOB), (3, CAROL)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
    }
    for (who, ty) in [(ALICE, FRIGATE), (BOB, JUMPER), (CAROL, JUMPER)] {
        reg.character_mut(who).expect("a pilot").ship_type_id = ty;
    }
    place(&mut reg, ALICE, DEST, [1.0e7, 2.0e6, -3.0e6]);
    place(&mut reg, BOB, ORIGIN, [0.0, 0.0, 0.0]);
    place(&mut reg, CAROL, ORIGIN, [0.0, 0.0, 0.0]);
    call_fleet(&mut reg, ALICE, "CreateFleet", vec![], None).expect("create");
    call_fleet(&mut reg, ALICE, "Init", vec![PyValue::Int(670)], None).expect("init");
    let PyValue::Int(id) = call_fleet(&mut reg, ALICE, "GetFleetID", vec![], None).expect("id")
    else {
        panic!("an integer");
    };
    call_fleet(&mut reg, ALICE, "Invite", vec![PyValue::Int(BOB)], Some(id)).expect("invite");
    call_fleet(
        &mut reg,
        BOB,
        "AcceptInvite",
        vec![PyValue::Int(670)],
        Some(id),
    )
    .expect("accept");
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(60));
    reg
}

/// Alice lights her cyno; returns the field's ball id.
fn light(reg: &mut Registry) -> i64 {
    crate::modules::activate(
        reg,
        Some(ALICE),
        ALICE_CYNO,
        cyno::EFFECT_NAME,
        None,
        1000,
        T0,
    )
    .expect("the cyno lights");
    let _ = reg.take_pending();
    reg.fleets()
        .beacon_of(ALICE)
        .expect("a lit beacon")
        .beacon_id
}

fn lit_world() -> (Registry, i64) {
    let mut reg = world();
    let beacon = light(&mut reg);
    (reg, beacon)
}

fn call(
    reg: &mut Registry,
    who: i64,
    owner: i64,
    beacon: i64,
    system: i64,
) -> Result<PyValue, ServiceError> {
    jump_to(
        reg,
        &Request {
            service: "beyonce",
            method: "CmdJumpToFleetModuleBeacon",
            args: &[
                PyValue::Int(owner),
                PyValue::Int(beacon),
                PyValue::Int(system),
            ],
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        },
        crate::pyutil::filetime_now(),
    )
}

fn bob_jumps(reg: &mut Registry, beacon: i64) -> Result<PyValue, ServiceError> {
    call(reg, BOB, ALICE, beacon, DEST)
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

fn refused(r: Result<PyValue, ServiceError>) -> (String, Vec<(PyValue, PyValue)>) {
    raised(&r.expect_err("the jump must be refused"))
}

fn arg<'a>(d: &'a [(PyValue, PyValue)], name: &str) -> &'a PyValue {
    crate::pyutil::pairs_get(d, name.as_bytes()).unwrap_or_else(|| panic!("no arg {name}: {d:?}"))
}

/// Every string and number in a value, joined.
fn flat(v: &[PyValue]) -> String {
    fn one(v: &PyValue) -> String {
        match v {
            PyValue::Bytes(b) => format!("{} ", String::from_utf8_lossy(b)),
            PyValue::Int(n) => format!("{n} "),
            PyValue::Tuple(l) | PyValue::List(l) => l.iter().map(one).collect(),
            _ => String::new(),
        }
    }
    v.iter().map(one).collect()
}

fn isotope(reg: &Registry, who: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == ISOTOPE && i.owner_id == who)
        .map(|i| i.quantity)
        .sum()
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

fn with_item(reg: &mut Registry, it: Item) {
    let mut items = reg.store().items().to_vec();
    items.push(it);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_652_000,
        ..crate::store::Snapshot::default()
    });
}

/// A beacon Alice's fleet holds in a system the field table does not have:
/// for the refusals that come before the field is read.
fn manual_beacon(reg: &mut Registry, system: i64) -> i64 {
    let id = 9_800_000_999_999;
    let _ = reg.fleets_mut().douse_beacon(ALICE);
    let _ = reg.fleets_mut().light_beacon(
        ALICE,
        Beacon {
            solar_system_id: system,
            beacon_id: id,
            type_id: cyno::TYPE_CYNOSURAL_FIELD,
        },
    );
    let _ = reg.take_pending();
    id
}

#[test]
fn jump_needs_a_jump_drive() {
    let (mut reg, beacon) = lit_world();
    // Alice's frigate has no `canJump`; she is in DEST, so give her a fleet
    // mate's beacon to ask for: Bob lights nothing, so use the manual one.
    place(&mut reg, ALICE, ORIGIN, [0.0; 3]);
    let _ = beacon;
    let (key, _) = refused(call(&mut reg, ALICE, BOB, 1, DEST));
    assert_eq!(key, "JumpNoDrive");
}

#[test]
fn jump_to_a_stranger_fleets_beacon_is_refused() {
    let (mut reg, beacon) = lit_world();
    // Carol is in no fleet at all.
    let (key, _) = refused(call(&mut reg, CAROL, ALICE, beacon, DEST));
    assert_eq!(key, "NoActiveBeacon");
    // And Bob, in the fleet, names the wrong beacon, owner or system.
    for (owner, ball, system) in [
        (ALICE, beacon + 1, DEST),
        (BOB, beacon, DEST),
        (ALICE, beacon, FAR),
    ] {
        let (key, _) = refused(call(&mut reg, BOB, owner, ball, system));
        assert_eq!(key, "NoActiveBeacon", "{owner} {ball} {system}");
    }
    assert!(reg.roster().get(BOB).is_some(), "nothing moved");
    assert_eq!(isotope(&reg, BOB), BAY + CARGO, "nothing was paid");
}

#[test]
fn a_pilot_with_no_fleet_has_no_beacon() {
    let (mut reg, beacon) = lit_world();
    call_fleet(&mut reg, BOB, "LeaveFleet", vec![], None).expect("leave");
    let _ = reg.take_pending();
    let (key, _) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "NoActiveBeacon");
}

#[test]
fn the_pilots_own_running_cyno_blocks_the_jump() {
    let (mut reg, beacon) = lit_world();
    // Bob is made to run a cyno of his own: his ship carries a module.
    with_item(
        &mut reg,
        item(&content(), 100_650_102, CYNO, BOB, HI_SLOT_0, 1, true),
    );
    crate::modules::activate(
        &mut reg,
        Some(BOB),
        100_650_102,
        cyno::EFFECT_NAME,
        None,
        1000,
        T0,
    )
    .ok();
    // His cyno needs ozone he does not have: if it refused, the test is
    // meaningless, so give him some and try again.
    if cyno::running_module_on(&reg, ship(BOB)).is_none() {
        with_item(
            &mut reg,
            item(
                &content(),
                100_650_205,
                OZONE,
                BOB,
                cyno::FLAG_CARGO,
                5_000,
                false,
            ),
        );
        crate::modules::activate(
            &mut reg,
            Some(BOB),
            100_650_102,
            cyno::EFFECT_NAME,
            None,
            1000,
            T0,
        )
        .expect("his cyno lights");
    }
    let (key, d) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "CantJumpWhileModuleActive");
    assert_eq!(
        arg(&d, "moduleType"),
        &PyValue::Int(CYNO),
        "named by the module's type"
    );
}

#[test]
fn out_of_range_raises_jumptoofar_with_dist_and_range() {
    let mut reg = world();
    let ball = manual_beacon(&mut reg, FAR);
    let (key, d) = refused(call(&mut reg, BOB, ALICE, ball, FAR));
    assert_eq!(key, "JumpTooFar");
    let PyValue::Real(dist) = arg(&d, "dist") else {
        panic!("a real")
    };
    assert!((dist - FAR_LY).abs() < 1e-9, "{dist}");
    assert_eq!(arg(&d, "range"), &PyValue::Real(RANGE));
}

#[test]
fn jump_drive_calibration_lengthens_the_range() {
    let mut reg = world();
    let ball = manual_beacon(&mut reg, FAR);
    // Level 5 of a +20 %/level skill: 5 x 2 = 10 ly, over FAR_LY.
    train(&mut reg, BOB, &[(RANGE_SKILL, 5)]);
    let (key, _) = refused(call(&mut reg, BOB, ALICE, ball, FAR));
    assert_ne!(key, "JumpTooFar", "the range now reaches");
    // Level 2: 5 x 1.4 = 7 ly, short of FAR_LY.
    train(&mut reg, BOB, &[(RANGE_SKILL, 2)]);
    let (key, d) = refused(call(&mut reg, BOB, ALICE, ball, FAR));
    assert_eq!(key, "JumpTooFar");
    let PyValue::Real(range) = arg(&d, "range") else {
        panic!("a real")
    };
    assert!((range - 7.0).abs() < 1e-9, "{range}");
}

#[test]
fn jump_into_high_sec_is_refused() {
    let mut reg = world();
    let ball = manual_beacon(&mut reg, HIGH);
    let (key, d) = refused(call(&mut reg, BOB, ALICE, ball, HIGH));
    assert_eq!(key, "CantEnterHighSec");
    assert_eq!(arg(&d, "typeID"), &type_arg(JUMPER));
    assert_eq!(isotope(&reg, BOB), BAY + CARGO);
}

#[test]
fn a_hull_whose_harmonics_do_not_match_is_refused() {
    let (mut reg, beacon) = lit_world();
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut().filter(|i| i.item_id == ship(BOB)) {
        i.type_id = PICKY;
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_652_000,
        ..crate::store::Snapshot::default()
    });
    reg.character_mut(BOB).expect("Bob").ship_type_id = PICKY;
    let (key, _) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "JumpDestinationHarmonicInvalid");
}

#[test]
fn jump_while_the_activation_timer_runs_is_refused() {
    let (mut reg, beacon) = lit_world();
    crate::jump_fatigue::on_jump(&mut reg, BOB, 1.0);
    let _ = reg.take_pending();
    let (key, d) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "JumpActivationTimer");
    let PyValue::Int(left) = arg(&d, "remaining") else {
        panic!("blue ticks")
    };
    assert!(*left > 0);
    assert_eq!(isotope(&reg, BOB), BAY + CARGO, "nothing was paid");
}

#[test]
fn the_fuel_cost_is_the_clients_formula() {
    // `max(long(ly * perLY), 1)`.
    assert_eq!(fuel_required(DEST_LY, PER_LY), 3_250);
    assert_eq!(fuel_required(0.0, PER_LY), 1, "a zero distance costs one");
    assert_eq!(fuel_required(0.0004, PER_LY), 1, "and a fraction costs one");
    assert_eq!(fuel_required(1.9999, 1.0), 1, "long() truncates");
}

#[test]
fn fuel_comes_from_the_fuel_bay_then_cargo() {
    let (mut reg, beacon) = lit_world();
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    // 3,250 from a bay of 2,000 and a cargo of 5,000: the bay is emptied
    // (its stack destroyed) and the cargo pays the rest.
    let bay: i64 = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id == ISOTOPE && i.flag_id == cyno::FLAG_FUEL_BAY)
        .map(|i| i.quantity)
        .sum();
    let cargo: i64 = reg
        .store()
        .items()
        .iter()
        .filter(|i| i.type_id == ISOTOPE && i.flag_id == cyno::FLAG_CARGO && i.owner_id == BOB)
        .map(|i| i.quantity)
        .sum();
    assert_eq!(bay, 0);
    assert_eq!(cargo, BAY + CARGO - 3_250);
    assert_eq!(isotope(&reg, CAROL), CARGO, "nobody else's fuel moved");
}

#[test]
fn the_plan_takes_whole_stacks_then_part_of_one() {
    let stack = |id: i64, q: i64| Item {
        item_id: id,
        type_id: ISOTOPE,
        owner_id: BOB,
        location_id: ship(BOB),
        flag_id: cyno::FLAG_CARGO,
        quantity: q,
        group_id: GROUP_ISOTOPE,
        category_id: 4,
        singleton: false,
    };
    let stacks = [stack(1, 10), stack(2, 10), stack(3, 10)];
    assert_eq!(
        fuel_plan(&stacks, 15),
        Some(vec![
            Edit::Destroy { item_id: 1 },
            Edit::Consume {
                item_id: 2,
                quantity: 5
            }
        ])
    );
    assert_eq!(
        fuel_plan(&stacks, 10),
        Some(vec![Edit::Destroy { item_id: 1 }])
    );
    assert_eq!(fuel_plan(&stacks, 31), None, "short");
    assert_eq!(fuel_plan(&[], 1), None, "none at all");
}

#[test]
fn short_of_fuel_raises_jumpnotenoughcharge() {
    let (mut reg, beacon) = lit_world();
    // Take all but 100 of Bob's isotopes.
    let take = BAY + CARGO - 100;
    reg.apply_items(&[
        Edit::Destroy { item_id: BOB_BAY },
        Edit::Consume {
            item_id: BOB_CARGO,
            quantity: take - BAY,
        },
    ])
    .expect("the stacks");
    assert_eq!(isotope(&reg, BOB), 100);
    let (key, d) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "JumpNotEnoughCharge");
    assert_eq!(arg(&d, "typeName"), &PyValue::Int(ISOTOPE));
    assert_eq!(arg(&d, "available"), &PyValue::Int(100));
    assert_eq!(arg(&d, "required"), &PyValue::Int(3_250));
    assert_eq!(isotope(&reg, BOB), 100, "nothing was taken");
    assert!(reg.roster().get(BOB).is_some(), "and nobody left");
}

#[test]
fn jump_fuel_conservation_cuts_the_bill() {
    let (mut reg, beacon) = lit_world();
    // Level 3 of a -10 %/level skill: the hull's 1,000 a light year is 700,
    // and 3.2504 ly of it is 2,275.28, truncated.
    train(&mut reg, BOB, &[(FUEL_SKILL, 3)]);
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    assert_eq!(isotope(&reg, BOB), BAY + CARGO - 2_275);
}

#[test]
fn a_jump_sets_the_fatigue_timers_and_tells_the_pilot() {
    let (mut reg, beacon) = lit_world();
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    let sent = crate::sov_hub::fixture::due(&mut reg);
    assert!(
        sent.iter()
            .any(|(n, _, to)| *n == "OnJumpTimersUpdated" && to.as_deref() == Some(&[BOB][..])),
        "the pilot is told: {sent:?}"
    );
    let now = crate::pyutil::filetime_now();
    let left = crate::jump_fatigue::activation_remaining(&reg, BOB, now).expect("a timer runs");
    // The model, by hand: `(1 + ly * multiplier) * 60` seconds at least.
    let want = ((1.0 + DEST_LY * MULTIPLIER) * 60.0) as i64 * crate::industry::FILETIME_SECOND;
    assert!(
        (left - want).abs() < 5 * crate::industry::FILETIME_SECOND,
        "{left} against {want}"
    );
    let (key, _) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "ShipJumping", "and he is in the jump");
}

#[test]
fn the_departure_fx_is_the_jump_drives_with_no_target() {
    let (mut reg, beacon) = lit_world();
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    let sent = crate::sov_hub::fixture::pending(&mut reg);
    let fx = sent
        .iter()
        .find(|(n, a, _)| *n == "DoDestinyUpdate" && flat(a).contains(FX_JUMP_DRIVE_OUT))
        .expect("the FX is pushed");
    let PyValue::List(entries) = &fx.1[0] else {
        panic!("entries")
    };
    let PyValue::Tuple(entry) = &entries[0] else {
        panic!("an entry")
    };
    let PyValue::Tuple(named) = &entry[1] else {
        panic!("(name, args)")
    };
    assert_eq!(named[0], crate::pyutil::bstr("OnSpecialFX"));
    let PyValue::Tuple(ops) = &named[1] else {
        panic!("args")
    };
    assert_eq!(ops.len(), 14, "all fourteen positionals");
    assert_eq!(ops[0], PyValue::Int(ship(BOB)));
    assert_eq!(ops[3], PyValue::None, "no target ball in this park");
    assert_eq!(ops[13], PyValue::Tuple(vec![PyValue::Int(DEST)]));
    assert_eq!(fx.2.as_deref().map(|a| a[0]), Some(BOB), "the jumper first");
}

#[test]
fn jump_lands_inside_the_spawn_radius() {
    let (mut reg, beacon) = lit_world();
    let field = cyno::field_by_ball(&reg, beacon).expect("the field");
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    let _ = reg.take_pending();
    let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    let _ = reg.take_due(late);
    crate::jump::tick(&mut reg, late);
    let changes = reg.take_session_changes();
    assert_eq!(changes.len(), 1, "one session change");
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
    let p = reg.roster().get(BOB).copied().expect("Bob is in space");
    assert_eq!(p.solar_system_id, DEST, "in the other system");
    let d = (0..3)
        .map(|i| (p.motion.position[i] - field.position[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(d <= SPAWN, "{d} m from the field, inside {SPAWN}");
    assert_eq!(p.motion.speed(), 0.0, "at rest");
}

#[test]
fn a_jump_leaves_the_old_system_roster_and_joins_the_new_one() {
    let (mut reg, beacon) = lit_world();
    assert!(reg.roster().others_in_system(ORIGIN, BOB).contains(&CAROL));
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    assert!(reg.roster().get(BOB).is_none(), "out of space at the click");
    assert!(reg.jumps().contains(BOB), "in the tunnel");
    assert!(
        !reg.roster().others_in_system(ORIGIN, CAROL).contains(&BOB),
        "gone from Carol's system"
    );
    let _ = reg.take_pending();
    let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    let _ = reg.take_due(late);
    crate::jump::tick(&mut reg, late);
    let _ = reg.take_session_changes();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
    assert!(
        reg.roster().others_in_system(DEST, ALICE).contains(&BOB),
        "and in Alice's"
    );
    assert!(!reg.jumps().contains(BOB), "the leg is over");
}

#[test]
fn the_landing_spot_is_inside_the_ball_for_every_seed() {
    let field = Field {
        ball_id: 9_800_000_000_001,
        character_id: ALICE,
        ship_id: ship(ALICE),
        module_id: ALICE_CYNO,
        module_type_id: CYNO,
        type_id: cyno::TYPE_CYNOSURAL_FIELD,
        system_id: DEST,
        position: [4.0e9, -2.0e9, 7.0e9],
        spawn_radius: SPAWN,
        corporation_id: CORP,
        alliance_id: None,
    };
    let mut spread = [0.0f64; 3];
    for seed in 0..500u64 {
        let p = landing_spot(&field, seed);
        let d: f64 = (0..3)
            .map(|i| (p[i] - field.position[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(d <= SPAWN + 1e-6, "seed {seed}: {d}");
        for i in 0..3 {
            spread[i] = spread[i].max((p[i] - field.position[i]).abs());
        }
    }
    assert!(
        spread.iter().all(|s| *s > SPAWN / 2.0),
        "it varies: {spread:?}"
    );
    let none = Field {
        spawn_radius: 0.0,
        ..field
    };
    assert_eq!(
        landing_spot(&none, 1),
        none.position,
        "no radius, no spread"
    );
}

#[test]
fn a_lit_field_that_went_out_is_no_beacon() {
    let (mut reg, beacon) = lit_world();
    // The field goes with its module; the fleet's list went with it.
    assert!(cyno::remove_field(&mut reg, ALICE_CYNO));
    let _ = reg.take_pending();
    let (key, _) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "NoActiveBeacon");
}

#[test]
fn the_same_system_is_not_a_destination() {
    let (mut reg, beacon) = lit_world();
    place(&mut reg, BOB, DEST, [0.0; 3]);
    let (key, _) = refused(bob_jumps(&mut reg, beacon));
    assert_eq!(key, "JumpDestinationInvalid");
}

/// S8: the jumper's own ball goes 2 s before the session changes, and an
/// overview reading the ego in that window raises on every row. In the same
/// stamp the jumper is told to drop everything its park holds; the others
/// lose the ship alone.
#[test]
fn the_jumper_drops_the_whole_park_with_its_own_ball_and_the_others_only_the_ship() {
    let (mut reg, beacon) = lit_world();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
    crate::destiny::on_ballpark_bind(&mut reg, Some(CAROL));
    let _ = reg.take_pending();
    bob_jumps(&mut reg, beacon).expect("a legal jump");
    let _ = reg.take_pending();
    let sent = crate::sov_hub::fixture::due(&mut reg);
    let removals = |to: i64| -> Vec<String> {
        sent.iter()
            .filter(|(n, a, who)| {
                *n == "DoDestinyUpdate"
                    && flat(a).contains("RemoveBalls")
                    && who.as_deref().is_some_and(|w| w.contains(&to))
            })
            .map(|(_, a, _)| flat(a))
            .collect()
    };
    let bob = removals(BOB);
    assert!(
        bob.iter()
            .any(|s| s.contains(&ship(BOB).to_string()) && s.contains(&ship(CAROL).to_string())),
        "the jumper drops its ball and the park it held: {bob:?}"
    );
    let carol = removals(CAROL);
    assert!(
        carol.iter().all(|s| !s.contains(&ship(CAROL).to_string())),
        "and Carol is told of the ship alone: {carol:?}"
    );
}
