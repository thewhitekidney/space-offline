//! The portal and conduit tests. **Every id and number here is invented**: the
//! types are 990,660,xxx, the systems 30,660,xxx, the masses and rates round-ish
//! values chosen so no product sits on an integer edge. The client's constants
//! appear only by name (the effects, the attribute numbers, the field types).

use super::*;
use crate::celestial::LIGHTYEAR;
use crate::fleet::Beacon;
use crate::item::Item;
use crate::jump_drive::{ATTR_CAN_JUMP, ATTR_FUEL_AMOUNT, ATTR_FUEL_TYPE, ATTR_RANGE};
use crate::space_state::{Motion, Presence};
use std::time::{Duration, Instant};

const CORP: i64 = 98_660_001;

const ORIGIN: i64 = 30_660_001;
const DEST: i64 = 30_660_002;
const HIGH: i64 = 30_660_003;
const FAR: i64 = 30_660_004;

/// Invented distances from ORIGIN, in light years (never a whole number).
const DEST_LY: f64 = 3.2504;
const HIGH_LY: f64 = 2.0;
const FAR_LY: f64 = 9.0504;

const ALICE: i64 = 90_660_001;
const BOB: i64 = 90_660_002;
const CAROL: i64 = 90_660_003;
const DAN: i64 = 90_660_004;
const ERIN: i64 = 90_660_005;

const CYNO: i64 = 990_660_001;
const OZONE: i64 = 990_660_002;
const ISOTOPE: i64 = 990_660_003;
const STRONTIUM: i64 = 990_660_004;
const PORTAL_MODULE: i64 = 990_660_005;
const BRIDGER: i64 = 990_660_006;
const PASSENGER: i64 = 990_660_007;
const BARRED: i64 = 990_660_008;
const PORTAL_SKILL: i64 = 990_660_009;
const FRIGATE: i64 = 990_660_010;
const DRIVER: i64 = 990_660_011;
const GROUP_CYNO: i64 = 990_660;
const GROUP_ITEM: i64 = 990_661;
const GROUP_SHIP: i64 = 990_662;
const GROUP_PORTAL: i64 = 990_663;

/// Invented: the cyno's cycle and ozone, the field's landing radius and speed
/// limit.
const CYNO_CYCLE_MS: i64 = 600_000;
const OZONE_PER_CYCLE: i64 = 500;
const SPAWN: f64 = 5_000.0;
const LIMIT: f64 = 500.0;
/// Invented: the portal's run, the strontium it burns, its capacitor.
const PORTAL_MS: i64 = 60_000;
const STRONTIUM_PER_RUN: i64 = 500;
const PORTAL_CAP: f64 = 50.0;
/// Invented drive: range, fuel per light year, fatigue multiplier, the
/// portal-side multiplier (1972) and the portal's mass factor.
const RANGE: f64 = 5.0;
const PER_LY: f64 = 1_000.0;
const MULTIPLIER: f64 = 0.5;
const THROUGH: f64 = 0.4;
const MASS_FACTOR: f64 = 1.5e-7;
/// Invented conduit: riders and fuel per light year.
const CONDUIT_RIDERS: f64 = 2.0;
const CONDUIT_PER_LY: f64 = 800.0;
/// Invented masses, kilograms.
const PASSENGER_MASS: f64 = 4_000_000.0;
const BRIDGER_MASS: f64 = 9_000_000.0;

const ALICE_CYNO: i64 = 100_660_101;
const ALICE_OZONE: i64 = 100_660_201;
const BOB_PORTAL: i64 = 100_660_102;
const BOB_BAY: i64 = 100_660_202;
const BOB_CARGO: i64 = 100_660_203;
const BOB_STRONTIUM: i64 = 100_660_204;
const CAROL_CARGO: i64 = 100_660_205;
/// Invented stacks.
const BAY: i64 = 2_000;
const CARGO: i64 = 5_000;
const STRONTIUM_STACK: i64 = 1_200;

const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_660_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let hull = |id: i64, mass: f64, extra: &str, name: &str| {
        format!(
            "type\t{id}\t{GROUP_SHIP}\t6\t1\t1\t20000.0\t130.0\t{mass}\t33.0\t0\t5000\t\
             9=310.0;37=355.0;482=260.0;{extra}\t{name}\n"
        )
    };
    let drive = format!(
        "{ATTR_CAN_JUMP}=1.0;{ATTR_FUEL_TYPE}={ISOTOPE}.0;{ATTR_RANGE}={RANGE};\
         {ATTR_FUEL_AMOUNT}={PER_LY};1971={MULTIPLIER};{ATTR_THROUGH_FATIGUE}={THROUGH}"
    );
    let conduit = format!(
        "{drive};{ATTR_CONDUIT_REQUIRED}=5682.0;{ATTR_CONDUIT_COUNT}={CONDUIT_RIDERS};\
         {ATTR_CONDUIT_FUEL_AMOUNT}={CONDUIT_PER_LY}"
    );
    let rows = format!(
        "{}attribute\t280\t0.0\tskillLevel\n\
         attribute\t1775\t0.0\tconsumptionQuantityBonusPercent\n\
         attribute\t1971\t1.0\tjumpFatigueMultiplier\n\
         attribute\t{ATTR_THROUGH_FATIGUE}\t1.0\tjumpThroughFatigueMultiplier\n\
         attribute\t3319\t1.0\tisTitanJumpPortalPassenger\n\
         attribute\t5682\t1.0\tisCarrierJumpConduitPassenger\n\
         attribute\t20\t0.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t852\t0.0\tsiegeModeWarpStatus\n\
         attribute\t1163\t1.0\tcanCloak\n\
         type\t{OZONE}\t{GROUP_ITEM}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n\
         type\t{ISOTOPE}\t{GROUP_ITEM}\t4\t1\t1\t0.15\t0.0\t0.0\t1.0\t0\t\t\tInvented Isotope\n\
         type\t{STRONTIUM}\t{GROUP_ITEM}\t4\t1\t1\t0.2\t0.0\t0.0\t1.0\t0\t\t\tInvented Strontium\n\
         type\t{CYNO}\t{GROUP_CYNO}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{cy}\t12,16,{cy}\t\
         20=-100.0;30=1.0;50=1.0;73={CYNO_CYCLE_MS}.0;{fuel_t}={OZONE}.0;{fuel_q}={OZONE_PER_CYCLE}.0;\
         {mga}=1.0;852=100.0;1163=0.0;1245=1.0;{lim}={LIMIT};{spawn}={SPAWN}\tInvented Cyno Generator\n\
         type\t{PORTAL_MODULE}\t{GROUP_PORTAL}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{pe}\t12,16,{pe}\t\
         6={PORTAL_CAP};20=-100.0;30=1.0;50=1.0;{fuel_t}={STRONTIUM}.0;{fuel_q}={STRONTIUM_PER_RUN}.0;\
         182={PORTAL_SKILL}.0;852=100.0;{ATTR_MASS_FACTOR}={MASS_FACTOR};1002={PORTAL_MS}.0;\
         {ATTR_PASSENGER_REQUIRED}=3319.0\tInvented Portal Generator\n\
         type\t{PORTAL_SKILL}\t275\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t4813,4814\t1775=-10.0\tInvented Portal Skill\n\
         {}{}{}{}{}\
         effect\t{cy}\t1\t0\t73\t0\t0\t0\t0\t{cn}\t\n\
         effect\t{pe}\t1\t0\t1002\t6\t0\t0\t0\t{pn}\teffects.JumpPortal\n\
         effect\t4813\t0\t0\t0\t0\t0\t0\t0\tskillConsumptionQuantityBonusPercentSkillLevel\t\n\
         effect\t4814\t0\t0\t0\t0\t0\t0\t0\tjumpPortalConsumptionBonusPercentSkill\t\n\
         modifier\t4813\titemID\tItemModifier\t1775\t280\t0\t\t\n\
         modifier\t4814\tshipID\tLocationRequiredSkillModifier\t714\t1775\t6\t{PORTAL_SKILL}\t\n\
         modifier\t{cy}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{cy}\tshipID\tItemModifier\t37\t20\t6\t\t\n\
         modifier\t{cy}\tshipID\tItemModifier\t1163\t1163\t7\t\t\n\
         modifier\t{pe}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{pe}\tshipID\tItemModifier\t37\t20\t6\t\t\n",
        crate::typedata::FIXTURE,
        hull(BRIDGER, BRIDGER_MASS, &conduit, "Invented Bridger"),
        hull(DRIVER, BRIDGER_MASS, &drive, "Invented Driver"),
        hull(PASSENGER, PASSENGER_MASS, "", "Invented Passenger"),
        hull(
            BARRED,
            PASSENGER_MASS,
            "3319=0.0;5682=0.0",
            "Invented Barred Hull"
        ),
        hull(FRIGATE, 1_000_000.0, "", "Invented Frigate"),
        cy = cyno::EFFECT_CYNOSURAL_GENERATION,
        cn = cyno::EFFECT_NAME,
        pe = EFFECT_JUMP_PORTAL,
        pn = "jumpPortalGeneration",
        fuel_t = cyno::ATTR_FUEL_TYPE,
        fuel_q = cyno::ATTR_FUEL_QUANTITY,
        mga = cyno::ATTR_MAX_GROUP_ACTIVE,
        lim = cyno::ATTR_MAX_VELOCITY_ACTIVATION_LIMIT,
        spawn = cyno::ATTR_SPAWN_RADIUS,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, sec: &str, name: &str| {
        format!("{id}\t10660001\t20660001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = crate::celestial::Universe::parse(
        &[
            sys(ORIGIN, "-0.45", "Invented Origin"),
            sys(DEST, "-0.45", "Invented Dest"),
            sys(HIGH, "0.9", "Invented High"),
            sys(FAR, "-0.45", "Invented Far"),
        ],
        &["10660001\t\t\t\t\tInvented Region".to_owned()],
        &["20660001\t10660001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    let at = |id: i64, ly: f64| format!("s\t{id}\t{}\t0.0\t0.0", ly * LIGHTYEAR);
    let centers = crate::celestial::Centers::parse(&[
        at(ORIGIN, 0.0),
        at(DEST, DEST_LY),
        at(HIGH, HIGH_LY),
        at(FAR, FAR_LY),
    ])
    .expect("the test centres parse");
    c.insert("centers", centers);
    c.insert("dungeon_data", crate::dungeon_data::Table::fixture());
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

fn fleet_id(reg: &mut Registry) -> i64 {
    let PyValue::Int(id) = call_fleet(reg, ALICE, "GetFleetID", vec![], None).expect("id") else {
        panic!("an integer");
    };
    id
}

/// Alice (a frigate with a cyno and ozone) in DEST. Bob (the bridger: portal
/// generator, isotopes in the bay and the cargo, strontium) in ORIGIN at the
/// origin; Carol (a passenger hull, isotopes of her own), Dan (a barred hull)
/// and Erin (a passenger hull, in no fleet) in ORIGIN a kilometre or two away.
/// Alice, Bob, Carol and Dan in one fleet. Every push drained.
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
            pilot(DAN, "Dan", ORIGIN),
            pilot(ERIN, "Erin", ORIGIN),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let hulls = [
        (ALICE, FRIGATE),
        (BOB, BRIDGER),
        (CAROL, PASSENGER),
        (DAN, BARRED),
        (ERIN, PASSENGER),
    ];
    let mut items = reg.store().items().to_vec();
    for (who, ty) in hulls {
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = ty;
            i.group_id = GROUP_SHIP;
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
        item(&content, BOB_PORTAL, PORTAL_MODULE, BOB, HI_SLOT_0, 1, true),
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
            BOB_STRONTIUM,
            STRONTIUM,
            BOB,
            cyno::FLAG_FUEL_BAY,
            STRONTIUM_STACK,
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
        next_item_id: 100_661_000,
        ..crate::store::Snapshot::default()
    });
    for (client, who) in [(1, ALICE), (2, BOB), (3, CAROL), (4, DAN), (5, ERIN)] {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: client,
            session_id: client,
            user_id: client,
            character_id: Some(who),
        });
    }
    for (who, ty) in hulls {
        reg.character_mut(who).expect("a pilot").ship_type_id = ty;
    }
    place(&mut reg, ALICE, DEST, [1.0e7, 2.0e6, -3.0e6]);
    place(&mut reg, BOB, ORIGIN, [0.0, 0.0, 0.0]);
    place(&mut reg, CAROL, ORIGIN, [1_000.0, 0.0, 0.0]);
    place(&mut reg, DAN, ORIGIN, [0.0, 2_000.0, 0.0]);
    place(&mut reg, ERIN, ORIGIN, [0.0, 0.0, 1_000.0]);
    call_fleet(&mut reg, ALICE, "CreateFleet", vec![], None).expect("create");
    call_fleet(&mut reg, ALICE, "Init", vec![PyValue::Int(670)], None).expect("init");
    let id = fleet_id(&mut reg);
    for who in [BOB, CAROL, DAN] {
        join(&mut reg, id, who);
    }
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(60));
    reg
}

fn join(reg: &mut Registry, id: i64, who: i64) {
    call_fleet(reg, ALICE, "Invite", vec![PyValue::Int(who)], Some(id)).expect("invite");
    call_fleet(reg, who, "AcceptInvite", vec![PyValue::Int(670)], Some(id)).expect("accept");
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
        crate::pyutil::filetime_now(),
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

/// A world with Bob's portal open; the beacon.
fn open_world() -> (Registry, i64) {
    let (mut reg, beacon) = lit_world();
    bridge(&mut reg, BOB, beacon).expect("the portal opens");
    let _ = reg.take_pending();
    let _ = reg.take_due(Instant::now() + Duration::from_secs(1));
    (reg, beacon)
}

fn req<'a>(method: &'a str, args: &'a [PyValue], who: i64) -> Request<'a> {
    Request {
        service: "beyonce",
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    }
}

fn bridge(reg: &mut Registry, who: i64, beacon: i64) -> Result<PyValue, ServiceError> {
    let args = [
        PyValue::Int(ALICE),
        PyValue::Int(beacon),
        PyValue::Int(DEST),
    ];
    cmd_bridge_to_fleet_module_beacon(reg, &req("CmdBridgeToFleetModuleBeacon", &args, who))
}

fn through(reg: &mut Registry, who: i64, beacon: i64) -> Result<PyValue, ServiceError> {
    let args = [
        PyValue::Int(BOB),
        PyValue::Int(ship(BOB)),
        PyValue::Int(beacon),
        PyValue::Int(DEST),
    ];
    cmd_jump_through_fleet(reg, &req("CmdJumpThroughFleet", &args, who))
}

fn group(reg: &mut Registry, who: i64, beacon: i64) -> Result<PyValue, ServiceError> {
    let args = [
        PyValue::Int(ALICE),
        PyValue::Int(beacon),
        PyValue::Int(DEST),
    ];
    crate::jump_drive::cmd_group_jump_to_fleet_module_beacon(
        reg,
        &req("CmdGroupJumpToFleetModuleBeacon", &args, who),
    )
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
    raised(&r.expect_err("the call must be refused"))
}

fn arg<'a>(d: &'a [(PyValue, PyValue)], name: &str) -> &'a PyValue {
    crate::pyutil::pairs_get(d, name.as_bytes()).unwrap_or_else(|| panic!("no arg {name}: {d:?}"))
}

fn held(reg: &Registry, who: i64, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .filter(|i| i.type_id == type_id && i.owner_id == who)
        .map(|i| i.quantity)
        .sum()
}

fn edit_items(reg: &mut Registry, f: impl FnOnce(&mut Vec<Item>)) {
    let mut items = reg.store().items().to_vec();
    f(&mut items);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_662_000,
        ..crate::store::Snapshot::default()
    });
}

fn retype(reg: &mut Registry, who: i64, ty: i64) {
    edit_items(reg, |items| {
        for i in items.iter_mut().filter(|i| i.item_id == ship(who)) {
            i.type_id = ty;
        }
    });
    reg.character_mut(who).expect("a pilot").ship_type_id = ty;
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

/// The pushes to `who` in `sent`, by name.
fn told(sent: &[crate::sov_hub::fixture::Sent], name: &str, who: i64) -> Vec<Vec<PyValue>> {
    sent.iter()
        .filter(|(n, _, to)| *n == name && to.as_ref().is_some_and(|t| t.contains(&who)))
        .map(|(_, a, _)| a.clone())
        .collect()
}

/// What Carol's jump costs, by hand from the invented numbers.
fn carol_fuel() -> i64 {
    portal_fuel_required(DEST_LY, PASSENGER_MASS, MASS_FACTOR, PER_LY)
}

fn land(reg: &mut Registry) {
    let _ = reg.take_pending();
    let late = Instant::now() + crate::jump::JUMP_DELAY + Duration::from_millis(50);
    let _ = reg.take_due(late);
    crate::jump::tick(reg, late);
    let _ = reg.take_session_changes();
}

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

#[test]
fn bridging_needs_a_drive() {
    let (mut reg, beacon) = lit_world();
    let (key, _) = refused(bridge(&mut reg, CAROL, beacon));
    assert_eq!(key, "JumpNoDrive", "a passenger hull has no `canJump`");
}

#[test]
fn bridging_needs_a_portal_generator() {
    let (mut reg, beacon) = lit_world();
    edit_items(&mut reg, |items| items.retain(|i| i.item_id != BOB_PORTAL));
    let (key, _) = refused(bridge(&mut reg, BOB, beacon));
    assert_eq!(key, "JumpNoPortal");
}

#[test]
fn a_beacon_the_fleet_does_not_hold_is_refused() {
    let (mut reg, beacon) = lit_world();
    let (key, _) = refused(bridge(&mut reg, BOB, beacon + 1));
    assert_eq!(key, "NoActiveBeacon");
    call_fleet(&mut reg, BOB, "LeaveFleet", vec![], None).expect("leave");
    let _ = reg.take_pending();
    let (key, _) = refused(bridge(&mut reg, BOB, beacon));
    assert_eq!(key, "NoActiveBeacon", "out of the fleet");
    assert!(reg.ext::<Portals>().open.is_empty());
    assert_eq!(held(&reg, BOB, STRONTIUM), STRONTIUM_STACK, "nothing paid");
}

#[test]
fn out_of_range_raises_jumptoofar_with_dist_and_range() {
    let (mut reg, beacon) = lit_world();
    let _ = reg.fleets_mut().douse_beacon(ALICE);
    let _ = reg.fleets_mut().light_beacon(
        ALICE,
        Beacon {
            solar_system_id: FAR,
            beacon_id: beacon,
            type_id: cyno::TYPE_CYNOSURAL_FIELD,
        },
    );
    let args = [PyValue::Int(ALICE), PyValue::Int(beacon), PyValue::Int(FAR)];
    let (key, d) = refused(cmd_bridge_to_fleet_module_beacon(
        &mut reg,
        &req("CmdBridgeToFleetModuleBeacon", &args, BOB),
    ));
    assert_eq!(key, "JumpTooFar");
    assert_eq!(arg(&d, "range"), &PyValue::Real(RANGE));
    let PyValue::Real(dist) = arg(&d, "dist") else {
        panic!("a real");
    };
    assert!((dist - FAR_LY).abs() < 1e-6, "{dist}");
}

#[test]
fn the_destination_must_not_be_high_sec() {
    let (mut reg, beacon) = lit_world();
    let _ = reg.fleets_mut().douse_beacon(ALICE);
    let _ = reg.fleets_mut().light_beacon(
        ALICE,
        Beacon {
            solar_system_id: HIGH,
            beacon_id: beacon,
            type_id: cyno::TYPE_CYNOSURAL_FIELD,
        },
    );
    let args = [
        PyValue::Int(ALICE),
        PyValue::Int(beacon),
        PyValue::Int(HIGH),
    ];
    let (key, _) = refused(cmd_bridge_to_fleet_module_beacon(
        &mut reg,
        &req("CmdBridgeToFleetModuleBeacon", &args, BOB),
    ));
    assert_eq!(key, "CantEnterHighSec");
}

#[test]
fn bridging_opens_the_portal_and_tells_the_whole_fleet() {
    let (mut reg, beacon) = lit_world();
    bridge(&mut reg, BOB, beacon).expect("the portal opens");
    let sent = crate::sov_hub::fixture::pending(&mut reg);
    for who in [ALICE, BOB, CAROL, DAN] {
        let changes = told(&sent, "OnBridgeModeChange", who);
        assert_eq!(changes.len(), 1, "{who} is told once: {sent:?}");
        assert_eq!(
            changes[0],
            vec![
                PyValue::Int(ship(BOB)),
                PyValue::Int(DEST),
                PyValue::Int(beacon),
                PyValue::Bool(true)
            ]
        );
    }
    assert!(
        told(&sent, "OnBridgeModeChange", ERIN).is_empty(),
        "not the stranger"
    );
    assert_eq!(
        reg.fleets().bridges_visible_to(CAROL),
        vec![(
            BOB,
            Bridge {
                ship_id: ship(BOB),
                solar_system_id: DEST,
                beacon_id: beacon
            }
        )]
    );
    assert!(
        reg.modules().running(BOB_PORTAL).is_some(),
        "the module cycles"
    );
    assert_eq!(
        held(&reg, BOB, STRONTIUM),
        STRONTIUM_STACK - STRONTIUM_PER_RUN,
        "the module's own fuel"
    );
    assert_eq!(held(&reg, BOB, ISOTOPE), BAY + CARGO, "no isotopes yet");
    let p = portal_of_ship(&reg, ship(BOB)).expect("a portal");
    assert_eq!((p.dest_system, p.system_id), (DEST, ORIGIN));
}

#[test]
fn jump_portal_generation_cuts_the_strontium() {
    let (mut reg, beacon) = lit_world();
    train(&mut reg, BOB, &[(PORTAL_SKILL, 3)]);
    bridge(&mut reg, BOB, beacon).expect("the portal opens");
    let want = (STRONTIUM_PER_RUN as f64 * 0.7).round() as i64;
    assert_eq!(
        held(&reg, BOB, STRONTIUM),
        STRONTIUM_STACK - want,
        "three levels of the skill: the dogma sheet's modified 714"
    );
}

#[test]
fn short_of_strontium_is_modulerequiresfuel() {
    let (mut reg, beacon) = lit_world();
    edit_items(&mut reg, |items| {
        items.retain(|i| i.item_id != BOB_STRONTIUM)
    });
    let (key, d) = refused(bridge(&mut reg, BOB, beacon));
    assert_eq!(key, "ModuleRequiresFuel");
    assert_eq!(arg(&d, "fuel"), &crate::pyutil::bstr("Invented Strontium"));
    assert!(reg.modules().running(BOB_PORTAL).is_none());
}

#[test]
fn a_ship_holds_one_portal_at_a_time() {
    let (mut reg, beacon) = open_world();
    let (key, _) = refused(bridge(&mut reg, BOB, beacon));
    assert_eq!(key, "EffectAlreadyActive2");
}

#[test]
fn a_click_on_the_module_is_refused() {
    let (mut reg, _) = lit_world();
    let r = crate::modules::activate(
        &mut reg,
        Some(BOB),
        BOB_PORTAL,
        "jumpPortalGeneration",
        None,
        1,
        crate::pyutil::filetime_now(),
    );
    let (key, _) = refused(r);
    assert_eq!(key, "CustomNotify", "the module needs a destination");
    assert!(reg.modules().running(BOB_PORTAL).is_none());
}

#[test]
fn the_bridging_ship_is_held_still_while_the_portal_is_open() {
    let (reg, _) = open_world();
    let sheet = crate::dogma::sheet(&reg, ship(BOB)).expect("a sheet");
    assert_eq!(
        sheet.ship(37),
        0.0,
        "`speedFactor` -100 post-percent on `maxVelocity`: the effect's own modifier"
    );
}

#[test]
fn the_portal_closes_when_the_cycle_ends() {
    let (mut reg, _) = open_world();
    let later = crate::pyutil::filetime_now()
        + (PORTAL_MS + 2_000) * crate::industry::FILETIME_SECOND / 1000;
    crate::modules::tick(&mut reg, later);
    assert!(portal_of_ship(&reg, ship(BOB)).is_none(), "forgotten");
    let sent = crate::sov_hub::fixture::due(&mut reg);
    let closed = told(&sent, "OnBridgeModeChange", CAROL);
    assert_eq!(closed.len(), 1, "{sent:?}");
    assert_eq!(closed[0][3], PyValue::Bool(false));
    assert!(reg.fleets().bridge_of(BOB).is_none());
}

#[test]
fn a_portal_whose_field_went_out_closes_and_the_generator_stops() {
    let (mut reg, _) = open_world();
    assert!(cyno::remove_field(&mut reg, ALICE_CYNO));
    tick(&mut reg);
    assert!(portal_of_ship(&reg, ship(BOB)).is_none());
    assert!(reg.modules().running(BOB_PORTAL).is_none(), "stopped");
    let sent = crate::sov_hub::fixture::due(&mut reg);
    assert_eq!(
        told(&sent, "OnBridgeModeChange", CAROL).len(),
        1,
        "{sent:?}"
    );
}

#[test]
fn a_portal_closes_when_its_pilot_leaves_the_fleet() {
    let (mut reg, _) = open_world();
    call_fleet(&mut reg, BOB, "LeaveFleet", vec![], None).expect("leave");
    tick(&mut reg);
    assert!(portal_of_ship(&reg, ship(BOB)).is_none());
}

// ---------------------------------------------------------------------------
// Jumping through
// ---------------------------------------------------------------------------

#[test]
fn jumping_through_pays_the_bridging_ship_by_the_passengers_mass() {
    let (mut reg, beacon) = open_world();
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    let fuel = carol_fuel();
    assert!(fuel > 1, "a real bill: {fuel}");
    assert_eq!(
        held(&reg, BOB, ISOTOPE),
        BAY + CARGO - fuel,
        "paid by the bridging ship"
    );
    assert_eq!(held(&reg, CAROL, ISOTOPE), CARGO, "not by the passenger");
    // By hand, as the lane note writes it: ly x mass x factor x per-LY.
    assert_eq!(
        fuel,
        (DEST_LY * PASSENGER_MASS * MASS_FACTOR * PER_LY) as i64
    );
}

#[test]
fn the_portal_fuel_is_a_product_with_a_floor_of_one() {
    assert_eq!(portal_fuel_required(2.0, 3.0e6, 1.0e-7, 1_000.0), 600);
    assert_eq!(portal_fuel_required(0.0, 3.0e6, 1.0e-7, 1_000.0), 1);
    assert_eq!(portal_fuel_required(1.0, 1.0, 1.0e-9, 1_000.0), 1);
    let heavier = portal_fuel_required(2.0, 6.0e6, 1.0e-7, 1_000.0);
    assert_eq!(heavier, 2 * 600, "twice the mass, twice the fuel");
}

#[test]
fn a_heavier_passenger_costs_more() {
    let (mut reg, beacon) = open_world();
    // A drive hull sets no 3319 row, so the attribute's default (1) lets it
    // through; it is heavier than Carol's own.
    retype(&mut reg, CAROL, DRIVER);
    let before = held(&reg, BOB, ISOTOPE);
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    let paid = before - held(&reg, BOB, ISOTOPE);
    assert_eq!(
        paid,
        portal_fuel_required(DEST_LY, BRIDGER_MASS, MASS_FACTOR, PER_LY)
    );
    assert!(paid > carol_fuel());
}

#[test]
fn short_of_fuel_raises_jumpnotenoughcharge2_with_the_bridgers_stock() {
    let (mut reg, beacon) = open_world();
    edit_items(&mut reg, |items| {
        for i in items.iter_mut().filter(|i| i.item_id == BOB_BAY) {
            i.quantity = 10;
        }
        items.retain(|i| i.item_id != BOB_CARGO);
    });
    let (key, d) = refused(through(&mut reg, CAROL, beacon));
    assert_eq!(key, "JumpNotEnoughCharge2", "the servicing ship's dialog");
    assert_eq!(arg(&d, "available"), &PyValue::Int(10));
    assert_eq!(arg(&d, "required"), &PyValue::Int(carol_fuel()));
    assert_eq!(arg(&d, "typeName"), &PyValue::Int(ISOTOPE));
    assert!(reg.roster().get(CAROL).is_some(), "nothing moved");
    assert_eq!(held(&reg, CAROL, ISOTOPE), CARGO);
}

#[test]
fn a_hull_that_carries_no_passenger_attribute_is_refused() {
    let (mut reg, beacon) = open_world();
    let (key, _) = refused(through(&mut reg, DAN, beacon));
    assert_eq!(key, "JumpHarmonicsDoNotMatch");
    assert!(reg.roster().get(DAN).is_some());
    assert_eq!(held(&reg, BOB, ISOTOPE), BAY + CARGO, "nothing paid");
}

#[test]
fn a_stranger_cannot_use_the_portal() {
    let (mut reg, beacon) = open_world();
    let (key, _) = refused(through(&mut reg, ERIN, beacon));
    assert_eq!(key, "JumpNoBridge", "Erin is in no fleet");
    assert!(reg.roster().get(ERIN).is_some());
}

#[test]
fn a_ship_with_no_portal_open_raises_jumpnobridge() {
    let (mut reg, beacon) = lit_world();
    let (key, _) = refused(through(&mut reg, CAROL, beacon));
    assert_eq!(key, "JumpNoBridge", "No portal active on this ship");
}

#[test]
fn stale_ids_raise_jumpnobridge() {
    let (mut reg, beacon) = open_world();
    for args in [
        [BOB, ship(BOB), beacon + 1, DEST],
        [BOB, ship(BOB) + 500, beacon, DEST],
        [BOB, ship(BOB), beacon, FAR],
        [DAN, ship(BOB), beacon, DEST],
    ] {
        let a: Vec<PyValue> = args.iter().map(|v| PyValue::Int(*v)).collect();
        let (key, _) = refused(cmd_jump_through_fleet(
            &mut reg,
            &req("CmdJumpThroughFleet", &a, CAROL),
        ));
        assert_eq!(key, "JumpNoBridge", "{args:?}");
    }
}

#[test]
fn a_passenger_too_far_from_the_bridging_ship_is_refused() {
    let (mut reg, beacon) = open_world();
    place(&mut reg, CAROL, ORIGIN, [2.0e5, 0.0, 0.0]);
    let (key, d) = refused(through(&mut reg, CAROL, beacon));
    assert_eq!(key, "NotCloseEnoughToJump");
    assert_eq!(
        arg(&d, "maxdist"),
        &PyValue::Int(crate::jump::MAX_JUMP_DISTANCE as i64)
    );
    assert_eq!(held(&reg, BOB, ISOTOPE), BAY + CARGO);
}

#[test]
fn a_passenger_in_another_system_is_refused() {
    let (mut reg, beacon) = open_world();
    place(&mut reg, CAROL, FAR, [0.0; 3]);
    let (key, _) = refused(through(&mut reg, CAROL, beacon));
    assert_eq!(key, "JumpNoBridge");
}

#[test]
fn the_passengers_own_activation_timer_refuses_the_jump() {
    let (mut reg, beacon) = open_world();
    crate::jump_fatigue::on_jump(&mut reg, CAROL, 3.0);
    let (key, d) = refused(through(&mut reg, CAROL, beacon));
    assert_eq!(key, "JumpActivationTimer");
    let PyValue::Int(left) = arg(&d, "remaining") else {
        panic!("an integer");
    };
    assert!(*left > 0);
}

#[test]
fn jumping_through_sets_the_passengers_fatigue_with_both_multipliers() {
    let (mut reg, beacon) = open_world();
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    let sent = crate::sov_hub::fixture::due(&mut reg);
    assert!(
        !told(&sent, "OnJumpTimersUpdated", CAROL).is_empty(),
        "the passenger is told: {sent:?}"
    );
    let now = crate::pyutil::filetime_now();
    let left = crate::jump_fatigue::activation_remaining(&reg, CAROL, now).expect("a timer runs");
    // Carol's hull sets no multiplier of its own (1, the default); the
    // bridging hull's 1972 is THROUGH: `(1 + ly x 1 x THROUGH) x 60` s.
    let want = ((1.0 + DEST_LY * 1.0 * THROUGH) * 60.0) as i64 * crate::industry::FILETIME_SECOND;
    assert!(
        (left - want).abs() < 5 * crate::industry::FILETIME_SECOND,
        "{left} against {want}"
    );
    // The bridging pilot is not fatigued.
    assert!(crate::jump_fatigue::activation_remaining(&reg, BOB, now).is_none());
}

#[test]
fn the_passengers_own_multiplier_multiplies_in() {
    let (mut reg, beacon) = open_world();
    retype(&mut reg, CAROL, DRIVER);
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    let now = crate::pyutil::filetime_now();
    let left = crate::jump_fatigue::activation_remaining(&reg, CAROL, now).expect("a timer");
    let want =
        ((1.0 + DEST_LY * MULTIPLIER * THROUGH) * 60.0) as i64 * crate::industry::FILETIME_SECOND;
    assert!(
        (left - want).abs() < 5 * crate::industry::FILETIME_SECOND,
        "{left} against {want}"
    );
}

#[test]
fn the_passenger_leaves_the_roster_and_lands_in_the_field() {
    let (mut reg, beacon) = open_world();
    let field = cyno::field_by_ball(&reg, beacon).expect("the field");
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    assert!(
        reg.roster().get(CAROL).is_none(),
        "out of space at the click"
    );
    assert!(reg.jumps().contains(CAROL), "in the tunnel");
    assert!(
        reg.roster().get(BOB).is_some(),
        "the bridging ship stays where it is"
    );
    land(&mut reg);
    crate::destiny::on_ballpark_bind(&mut reg, Some(CAROL));
    let p = reg.roster().get(CAROL).copied().expect("Carol is in space");
    assert_eq!(p.solar_system_id, DEST);
    let d = (0..3)
        .map(|i| (p.motion.position[i] - field.position[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(d <= SPAWN, "{d} m from the field, inside {SPAWN}");
    assert_eq!(p.motion.speed(), 0.0, "at rest");
}

#[test]
fn the_portal_stays_open_for_the_next_passenger() {
    let (mut reg, beacon) = open_world();
    through(&mut reg, CAROL, beacon).expect("the first");
    retype(&mut reg, DAN, PASSENGER);
    through(&mut reg, DAN, beacon).expect("the second");
    assert_eq!(
        held(&reg, BOB, ISOTOPE),
        BAY + CARGO - 2 * carol_fuel(),
        "each pays its own mass"
    );
    assert!(portal_of_ship(&reg, ship(BOB)).is_some());
}

#[test]
fn the_departure_fx_goes_to_the_passenger_and_the_bridgers_neighbours() {
    let (mut reg, beacon) = open_world();
    through(&mut reg, CAROL, beacon).expect("a legal jump through");
    let sent = crate::sov_hub::fixture::pending(&mut reg);
    let fx = sent
        .iter()
        .find(|(n, _, _)| *n == "DoDestinyUpdate")
        .expect("the FX is pushed");
    let to = fx.2.as_ref().expect("an audience");
    assert!(to.contains(&CAROL) && to.contains(&BOB), "{to:?}");
}

// ---------------------------------------------------------------------------
// The conduit
// ---------------------------------------------------------------------------

#[test]
fn a_conduit_jumps_with_the_fleet_members_nearby() {
    let (mut reg, beacon) = lit_world();
    retype(&mut reg, DAN, PASSENGER);
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(reg.jumps().contains(BOB), "the anchor");
    assert!(reg.jumps().contains(CAROL), "a passenger");
    assert!(reg.jumps().contains(DAN), "a passenger");
    assert!(
        reg.roster().get(ERIN).is_some(),
        "a stranger is left behind"
    );
    let sent = crate::sov_hub::fixture::pending(&mut reg);
    let anchor = told(&sent, "OnGroupJumpAnchorActivated", BOB);
    assert_eq!(anchor, vec![vec![PyValue::Int(DEST), PyValue::Int(2)]]);
    for who in [CAROL, DAN] {
        assert_eq!(
            told(&sent, "OnGroupJumpedPassenger", who),
            vec![vec![PyValue::Int(BOB), PyValue::Int(DEST)]],
            "{who}"
        );
    }
    assert!(told(&sent, "OnGroupJumpedPassenger", BOB).is_empty());
}

#[test]
fn the_conduit_burns_the_anchors_fuel_at_its_own_rate_and_nothing_of_the_riders() {
    let (mut reg, beacon) = lit_world();
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    let want = (DEST_LY * CONDUIT_PER_LY) as i64;
    assert_eq!(held(&reg, BOB, ISOTOPE), BAY + CARGO - want);
    assert_eq!(held(&reg, CAROL, ISOTOPE), CARGO);
}

#[test]
fn conduit_passengers_get_their_own_hulls_fatigue() {
    let (mut reg, beacon) = lit_world();
    retype(&mut reg, CAROL, DRIVER);
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    let now = crate::pyutil::filetime_now();
    let left = crate::jump_fatigue::activation_remaining(&reg, CAROL, now).expect("a timer");
    let want = ((1.0 + DEST_LY * MULTIPLIER) * 60.0) as i64 * crate::industry::FILETIME_SECOND;
    assert!(
        (left - want).abs() < 5 * crate::industry::FILETIME_SECOND,
        "{left} against {want}"
    );
    assert!(
        crate::jump_fatigue::activation_remaining(&reg, BOB, now).is_some(),
        "and the anchor's own"
    );
}

#[test]
fn a_member_who_opted_out_stays() {
    let (mut reg, beacon) = lit_world();
    let id = fleet_id(&mut reg);
    call_fleet(
        &mut reg,
        CAROL,
        "SetAcceptsConduitJumpsValue",
        vec![PyValue::Bool(false)],
        Some(id),
    )
    .expect("opt out");
    let _ = reg.take_pending();
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(reg.jumps().contains(BOB));
    assert!(!reg.jumps().contains(CAROL), "Carol opted out");
    assert!(reg.roster().get(CAROL).is_some());
    let sent = crate::sov_hub::fixture::pending(&mut reg);
    assert_eq!(
        told(&sent, "OnGroupJumpAnchorActivated", BOB),
        vec![vec![PyValue::Int(DEST), PyValue::Int(0)]],
        "no passengers"
    );
}

#[test]
fn a_hull_the_conduit_cannot_carry_stays() {
    let (mut reg, beacon) = lit_world();
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(reg.jumps().contains(CAROL));
    assert!(
        !reg.jumps().contains(DAN),
        "Dan's hull sets the conduit attribute to 0"
    );
}

#[test]
fn the_conduit_takes_at_most_the_hulls_passenger_count_nearest_first() {
    let (mut reg, beacon) = lit_world();
    retype(&mut reg, DAN, PASSENGER);
    // Three candidates for two seats: Erin joins the fleet, as near as Carol;
    // Dan is two kilometres off and is left.
    let id = fleet_id(&mut reg);
    join(&mut reg, id, ERIN);
    let _ = reg.take_pending();
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(reg.jumps().contains(CAROL));
    assert!(reg.jumps().contains(ERIN));
    assert!(!reg.jumps().contains(DAN), "the farthest is left");
}

#[test]
fn a_member_out_of_conduit_range_stays() {
    let (mut reg, beacon) = lit_world();
    place(&mut reg, CAROL, ORIGIN, [CONDUIT_RANGE_M + 1.0e5, 0.0, 0.0]);
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(!reg.jumps().contains(CAROL));
    assert!(reg.roster().get(CAROL).is_some());
}

#[test]
fn a_member_on_an_activation_timer_stays() {
    let (mut reg, beacon) = lit_world();
    crate::jump_fatigue::on_jump(&mut reg, CAROL, 3.0);
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    assert!(!reg.jumps().contains(CAROL));
}

#[test]
fn a_drive_with_no_conduit_cannot_anchor() {
    let (mut reg, beacon) = lit_world();
    retype(&mut reg, BOB, DRIVER);
    let (key, _) = refused(group(&mut reg, BOB, beacon));
    assert_eq!(key, "CustomNotify", "no 3321 on the hull");
    assert!(reg.roster().get(BOB).is_some());
    assert_eq!(held(&reg, BOB, ISOTOPE), BAY + CARGO, "nothing paid");
}

#[test]
fn a_solo_jump_takes_nobody_along() {
    let (mut reg, beacon) = lit_world();
    let args = [
        PyValue::Int(ALICE),
        PyValue::Int(beacon),
        PyValue::Int(DEST),
    ];
    crate::jump_drive::cmd_jump_to_fleet_module_beacon(
        &mut reg,
        &req("CmdJumpToFleetModuleBeacon", &args, BOB),
    )
    .expect("a solo jump");
    assert!(reg.jumps().contains(BOB));
    assert!(!reg.jumps().contains(CAROL));
    let want = (DEST_LY * PER_LY) as i64;
    assert_eq!(
        held(&reg, BOB, ISOTOPE),
        BAY + CARGO - want,
        "the drive's own rate"
    );
}

#[test]
fn every_rider_lands_in_the_field_in_a_spot_of_its_own() {
    let (mut reg, beacon) = lit_world();
    retype(&mut reg, DAN, PASSENGER);
    let field = cyno::field_by_ball(&reg, beacon).expect("the field");
    group(&mut reg, BOB, beacon).expect("a legal group jump");
    land(&mut reg);
    let mut spots = Vec::new();
    for who in [BOB, CAROL, DAN] {
        crate::destiny::on_ballpark_bind(&mut reg, Some(who));
        let p = reg.roster().get(who).copied().expect("in space");
        assert_eq!(p.solar_system_id, DEST);
        let d = (0..3)
            .map(|i| (p.motion.position[i] - field.position[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(d <= SPAWN, "{who}: {d}");
        spots.push(p.motion.position);
    }
    assert!(
        spots[0] != spots[1] && spots[1] != spots[2],
        "no two share a spot"
    );
}
