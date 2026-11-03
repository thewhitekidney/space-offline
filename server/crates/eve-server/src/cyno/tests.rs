//! The cynosural field's tests. **Every id and number here is invented**:
//! the types are 990,640,xxx, the systems 30,640,xxx, the cycle and the
//! ozone are round numbers. The client's constants appear only by name
//! (the effect, the three attributes the data fixes, the three field types).

use super::*;
use crate::sov_hub::fixture as hubfx;
use crate::space_state::{Motion, Presence};

const T0: i64 = 133_000_000_000_000_000;
/// Invented: a cycle of one minute.
const CYCLE_MS: i64 = 60_000;
/// That cycle in blue ticks.
const CYCLE: i64 = CYCLE_MS * crate::modules::MSEC;
/// Invented: ozone a normal / covert / industrial cycle costs.
const FUEL_NORMAL: i64 = 500;
const FUEL_COVERT: i64 = 50;
const FUEL_INDUSTRIAL: i64 = 800;
/// Invented: the speed limit the rows carry, and a spawn radius.
const LIMIT: f64 = 500.0;
const SPAWN: f64 = 5_000.0;
/// Invented: plenty of ozone.
const PLENTY: i64 = 5_000;

const SYSTEM: i64 = 30_640_001;
const HIGH: i64 = 30_640_002;
const WORMHOLE: i64 = 31_640_001;

const ALICE: i64 = 90_640_001;
const BOB: i64 = 90_640_002;
const CAROL: i64 = 90_640_003;
const CORP: i64 = 98_640_001;

const CYNO: i64 = 990_640_001;
const COVERT: i64 = 990_640_002;
const INDUSTRIAL: i64 = 990_640_003;
const OZONE: i64 = 990_640_010;
const INHIBITOR: i64 = 990_640_020;
const GROUP_CYNO: i64 = 990_640;
const GROUP_OZONE: i64 = 990_641;
const FRIGATE: i64 = 990_000_001;

const ALICE_CYNO: i64 = 100_640_101;
const ALICE_CYNO_2: i64 = 100_640_102;
const BOB_COVERT: i64 = 100_640_103;
const CAROL_INDUSTRIAL: i64 = 100_640_104;
const ALICE_CARGO: i64 = 100_640_201;
const ALICE_BAY: i64 = 100_640_202;
const BOB_CARGO: i64 = 100_640_203;
const CAROL_CARGO: i64 = 100_640_204;

/// `flagHiSlot0` **[V]**.
const HI_SLOT_0: i64 = 27;

fn ship(who: i64) -> i64 {
    100_640_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let module = |id: i64, extra: &str, fuel: i64, name: &str| {
        format!(
            "type\t{id}\t{GROUP_CYNO}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{EFFECT_CYNOSURAL_GENERATION}\t12,16,{EFFECT_CYNOSURAL_GENERATION}\t\
             20=-100.0;30=1.0;50=1.0;73={CYCLE_MS}.0;{ATTR_FUEL_TYPE}={OZONE}.0;{ATTR_FUEL_QUANTITY}={fuel}.0;\
             {ATTR_MAX_GROUP_ACTIVE}=1.0;852=100.0;1163=0.0;1245=1.0;{ATTR_MAX_VELOCITY_ACTIVATION_LIMIT}={LIMIT};2354=1.0;\
             {ATTR_SPAWN_RADIUS}={SPAWN}{extra}\t{name}\n"
        )
    };
    let rows = format!(
        "{}attribute\t20\t0.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t852\t0.0\tsiegeModeWarpStatus\n\
         attribute\t1163\t1.0\tcanCloak\n\
         type\t{OZONE}\t{GROUP_OZONE}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n\
         {}{}{}\
         type\t{INHIBITOR}\t{GROUP_INHIBITOR}\t22\t1\t1\t10.0\t50.0\t0.0\t1.0\t0\t\t\tInvented Inhibitor\n\
         effect\t{EFFECT_CYNOSURAL_GENERATION}\t1\t0\t73\t0\t0\t0\t0\t{EFFECT_NAME}\t\n\
         modifier\t{EFFECT_CYNOSURAL_GENERATION}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{EFFECT_CYNOSURAL_GENERATION}\tshipID\tItemModifier\t37\t20\t6\t\t\n\
         modifier\t{EFFECT_CYNOSURAL_GENERATION}\tshipID\tItemModifier\t1163\t1163\t7\t\t\n",
        crate::typedata::FIXTURE,
        module(CYNO, "", FUEL_NORMAL, "Invented Cyno Generator"),
        module(
            COVERT,
            &format!(";{ATTR_IS_COVERT}=1.0"),
            FUEL_COVERT,
            "Invented Covert Cyno Generator"
        ),
        module(
            INDUSTRIAL,
            &format!(";{ATTR_IS_INDUSTRIAL_CYNO}=1.0"),
            FUEL_INDUSTRIAL,
            "Invented Industrial Cyno Generator"
        ),
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, sec: &str, name: &str| {
        format!("{id}\t10640001\t20640001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = crate::celestial::Universe::parse(
        &[
            sys(SYSTEM, "-0.45", "Invented Null"),
            sys(HIGH, "0.9", "Invented High"),
            sys(WORMHOLE, "-0.99", "Invented Wormhole"),
        ],
        &["10640001\t\t\t\t\tInvented Region".to_owned()],
        &["20640001\t10640001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
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

struct World {
    reg: Registry,
}

/// Alice (a frigate, two plain cyno modules, ozone in her cargo and fuel
/// bay), Bob (a covert module) and Carol (an industrial one), all in space
/// in the null-sec system, at rest. Alice and Bob in one fleet; Carol alone.
/// Every push drained.
fn world() -> World {
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
    // Alice flies a frigate: the dogma sheet needs a hull the table holds.
    for i in items.iter_mut().filter(|i| i.item_id == ship(ALICE)) {
        i.type_id = FRIGATE;
        i.group_id = 25;
    }
    items.extend([
        item(&content, ALICE_CYNO, CYNO, ALICE, HI_SLOT_0, 1, true),
        item(&content, ALICE_CYNO_2, CYNO, ALICE, HI_SLOT_0 + 1, 1, true),
        item(&content, BOB_COVERT, COVERT, BOB, HI_SLOT_0, 1, true),
        item(
            &content,
            CAROL_INDUSTRIAL,
            INDUSTRIAL,
            CAROL,
            HI_SLOT_0,
            1,
            true,
        ),
        item(
            &content,
            ALICE_CARGO,
            OZONE,
            ALICE,
            FLAG_CARGO,
            PLENTY,
            false,
        ),
        item(
            &content,
            ALICE_BAY,
            OZONE,
            ALICE,
            FLAG_FUEL_BAY,
            PLENTY,
            false,
        ),
        item(&content, BOB_CARGO, OZONE, BOB, FLAG_CARGO, PLENTY, false),
        item(
            &content,
            CAROL_CARGO,
            OZONE,
            CAROL,
            FLAG_CARGO,
            PLENTY,
            false,
        ),
    ]);
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_641_000,
        ..crate::store::Snapshot::default()
    });
    for who in [ALICE, BOB, CAROL] {
        reg.character_mut(who).expect("a pilot").ship_type_id =
            if who == ALICE { FRIGATE } else { 0 };
    }
    let spot = [1.0e7, 2.0e6, -3.0e6];
    for who in [ALICE, BOB, CAROL] {
        place(&mut reg, who, SYSTEM, spot);
    }
    let mut w = World { reg };
    let id = w.form_fleet();
    assert_eq!(w.reg.fleets().fleet_of(BOB).map(|f| f.fleet_id), Some(id));
    let _ = hubfx::pending(&mut w.reg);
    let _ = hubfx::due(&mut w.reg);
    w
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

fn call_fleet(
    reg: &mut Registry,
    actor: i64,
    method: &str,
    args: Vec<PyValue>,
    bind: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let bind = bind.map_or(PyValue::None, PyValue::Int);
    reg.dispatch(&crate::service::Request {
        service: "fleetObjectHandler",
        method,
        args: &args,
        kwargs: &[],
        bind_params: Some(&bind),
        actor: Some(actor),
    })
}

impl World {
    /// Alice creates a fleet and invites Bob, who accepts, the way the
    /// client's own calls go.
    fn form_fleet(&mut self) -> i64 {
        let r = &mut self.reg;
        call_fleet(r, ALICE, "CreateFleet", vec![], None).expect("create");
        call_fleet(r, ALICE, "Init", vec![PyValue::Int(670)], None).expect("init");
        let PyValue::Int(id) = call_fleet(r, ALICE, "GetFleetID", vec![], None).expect("id") else {
            panic!("an integer");
        };
        call_fleet(r, ALICE, "Invite", vec![PyValue::Int(BOB)], Some(id)).expect("invite");
        call_fleet(r, BOB, "AcceptInvite", vec![PyValue::Int(670)], Some(id)).expect("accept");
        id
    }

    fn light(&mut self, who: i64, module: i64, t: i64) -> Result<PyValue, ServiceError> {
        crate::modules::activate(&mut self.reg, Some(who), module, EFFECT_NAME, None, 1000, t)
    }

    fn light_once(&mut self, who: i64, module: i64, t: i64) -> Result<PyValue, ServiceError> {
        crate::modules::activate(&mut self.reg, Some(who), module, EFFECT_NAME, None, 1, t)
    }

    fn ozone(&self, who: i64, flag: i64) -> i64 {
        self.reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == OZONE && i.owner_id == who && i.flag_id == flag)
            .map(|i| i.quantity)
            .sum()
    }

    fn pushes(&mut self) -> Vec<hubfx::Sent> {
        let mut v = hubfx::pending(&mut self.reg);
        v.extend(hubfx::due(&mut self.reg));
        v
    }

    fn field_of(&self, who: i64) -> Field {
        field_of_ship(&self.reg, ship(who)).expect("a lit field")
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn cyno_needs_a_fleet() {
    let mut w = world();
    let e = w.light(CAROL, CAROL_INDUSTRIAL, T0).expect_err("no fleet");
    let (key, d) = raised(&e);
    assert_eq!(key, "CynoMustBeInFleet");
    assert!(d.is_empty(), "the key takes no argument: {d:?}");
    assert!(fields(&w.reg).next().is_none());
    assert_eq!(
        w.ozone(CAROL, FLAG_CARGO),
        PLENTY,
        "a refusal costs no ozone"
    );
}

#[test]
fn cyno_refused_above_the_velocity_limit() {
    let mut w = world();
    w.reg
        .roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .velocity = [LIMIT + 100.0, 0.0, 0.0];
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("too fast");
    let (key, d) = raised(&e);
    assert_eq!(key, "VelocityTooHighToActivate");
    assert_eq!(arg(&d, "type"), &PyValue::Int(CYNO));
    let PyValue::Real(limit) = arg(&d, "velocityLimit") else {
        panic!("a number");
    };
    assert_eq!(*limit, LIMIT);
    let PyValue::Real(speed) = arg(&d, "shipVelocity") else {
        panic!("a number");
    };
    assert!(*speed > LIMIT, "the ship's own speed is named: {speed}");
    // At rest it lights.
    w.reg
        .roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .velocity = [0.0; 3];
    w.light(ALICE, ALICE_CYNO, T0).expect("at rest");
}

#[test]
fn cyno_refused_in_high_sec_and_in_wormhole_space() {
    let mut w = world();
    place(&mut w.reg, ALICE, HIGH, [0.0; 3]);
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("high-sec");
    assert_eq!(raised(&e).0, "CantInHighSecSpace");
    place(&mut w.reg, ALICE, WORMHOLE, [0.0; 3]);
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("wormhole");
    assert_eq!(raised(&e).0, "JumpBeaconInWormholeSpace");
    place(&mut w.reg, ALICE, SYSTEM, [0.0; 3]);
    w.light(ALICE, ALICE_CYNO, T0).expect("null-sec");
}

#[test]
fn cyno_refused_in_warp() {
    let mut w = world();
    w.reg
        .roster_mut()
        .get_mut(ALICE)
        .expect("in space")
        .motion
        .mode = Mode::Warp {
        goto: [1.0e12, 0.0, 0.0],
        effect_stamp: crate::destiny::tick() as i32,
        total_distance: 1.0e12,
        stop_distance: 0.0,
        warp_factor: 4_000,
    };
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("in warp");
    assert_eq!(raised(&e).0, "DeniedActivateInWarp");
}

#[test]
fn a_second_cyno_is_crowded_out() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("the first");
    let e = w.light(ALICE, ALICE_CYNO_2, T0).expect_err("the second");
    let (key, d) = raised(&e);
    assert_eq!(key, "EffectCrowdedOut");
    assert_eq!(arg(&d, "module"), &PyValue::Int(CYNO));
    assert_eq!(arg(&d, "count"), &PyValue::Int(1));
    assert_eq!(fields(&w.reg).count(), 1);
}

#[test]
fn cyno_refused_without_a_full_stack_of_ozone() {
    let mut w = world();
    // Two stacks, each a unit short of a cycle: the text says "in a single
    // stack", so the sum does not pay.
    let half = FUEL_NORMAL - 1;
    let items: Vec<Item> = w
        .reg
        .store()
        .items()
        .iter()
        .filter(|i| !(i.type_id == OZONE && i.owner_id == ALICE))
        .cloned()
        .chain([
            item(
                &content(),
                ALICE_CARGO,
                OZONE,
                ALICE,
                FLAG_CARGO,
                half,
                false,
            ),
            item(
                &content(),
                ALICE_BAY,
                OZONE,
                ALICE,
                FLAG_FUEL_BAY,
                half,
                false,
            ),
        ])
        .collect();
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_641_000,
        ..crate::store::Snapshot::default()
    });
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("no stack");
    let (key, d) = raised(&e);
    assert_eq!(key, "ModuleRequiresFuel");
    assert_eq!(flat(arg(&d, "fuel")).trim(), "Invented Ozone");
    assert_eq!(flat(arg(&d, "fueled")).trim(), "Invented Cyno Generator");
    assert_eq!(
        w.ozone(ALICE, FLAG_CARGO) + w.ozone(ALICE, FLAG_FUEL_BAY),
        2 * half
    );
}

// ---------------------------------------------------------------------------
// The fuel
// ---------------------------------------------------------------------------

#[test]
fn cyno_consumes_ozone_per_cycle_from_the_bay_first() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    assert_eq!(
        w.ozone(ALICE, FLAG_FUEL_BAY),
        PLENTY - FUEL_NORMAL,
        "the first cycle is paid at the click, from the bay"
    );
    assert_eq!(w.ozone(ALICE, FLAG_CARGO), PLENTY, "the cargo is untouched");
    // The next cycle starts a period later and is paid then.
    crate::modules::tick(&mut w.reg, T0 + CYCLE - 1);
    assert_eq!(w.ozone(ALICE, FLAG_FUEL_BAY), PLENTY - FUEL_NORMAL);
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    assert_eq!(w.ozone(ALICE, FLAG_FUEL_BAY), PLENTY - 2 * FUEL_NORMAL);
    assert!(
        field_of_ship(&w.reg, ship(ALICE)).is_some(),
        "the field outlives its first cycle"
    );
}

#[test]
fn the_cargo_pays_when_the_bay_is_short() {
    let mut w = world();
    let items: Vec<Item> = w
        .reg
        .store()
        .items()
        .iter()
        .filter(|i| i.item_id != ALICE_BAY)
        .cloned()
        .collect();
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_641_000,
        ..crate::store::Snapshot::default()
    });
    w.light(ALICE, ALICE_CYNO, T0).expect("lit from the cargo");
    assert_eq!(w.ozone(ALICE, FLAG_CARGO), PLENTY - FUEL_NORMAL);
}

#[test]
fn covert_and_industrial_fields_use_their_own_types_and_ozone() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("plain");
    w.light(BOB, BOB_COVERT, T0).expect("covert");
    assert_eq!(w.field_of(ALICE).type_id, TYPE_CYNOSURAL_FIELD);
    assert_eq!(w.field_of(BOB).type_id, TYPE_COVERT_CYNOSURAL_FIELD);
    assert_eq!(w.ozone(BOB, FLAG_CARGO), PLENTY - FUEL_COVERT);
    // Carol joins the fleet, then lights the industrial one.
    let id = w.reg.fleets().fleet_of(ALICE).expect("a fleet").fleet_id;
    call_fleet(
        &mut w.reg,
        ALICE,
        "Invite",
        vec![PyValue::Int(CAROL)],
        Some(id),
    )
    .expect("invite");
    call_fleet(
        &mut w.reg,
        CAROL,
        "AcceptInvite",
        vec![PyValue::Int(670)],
        Some(id),
    )
    .expect("accept");
    w.light(CAROL, CAROL_INDUSTRIAL, T0).expect("industrial");
    assert_eq!(w.field_of(CAROL).type_id, TYPE_INDUSTRIAL_CYNOSURAL_FIELD);
    assert_eq!(w.ozone(CAROL, FLAG_CARGO), PLENTY - FUEL_INDUSTRIAL);
    // The module's own spawn radius rides on the field, for a jump to land
    // within.
    assert_eq!(w.field_of(CAROL).spawn_radius, SPAWN);
}

#[test]
fn a_cycle_the_ozone_cannot_pay_stops_the_module_and_takes_the_field_down() {
    let mut w = world();
    // Exactly one cycle's ozone, in one stack.
    let items: Vec<Item> = w
        .reg
        .store()
        .items()
        .iter()
        .filter(|i| !(i.type_id == OZONE && i.owner_id == ALICE))
        .cloned()
        .chain([item(
            &content(),
            ALICE_CARGO,
            OZONE,
            ALICE,
            FLAG_CARGO,
            FUEL_NORMAL,
            false,
        )])
        .collect();
    w.reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_641_000,
        ..crate::store::Snapshot::default()
    });
    w.light(ALICE, ALICE_CYNO, T0).expect("one cycle's worth");
    let _ = w.pushes();
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    let sent = w.pushes();
    assert!(w.reg.modules().running(ALICE_CYNO).is_none(), "stopped");
    let stops = named(&sent, "OnGodmaShipEffect");
    assert!(
        stops.iter().any(|s| s
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains("ModuleRequiresFuel")),
        "the stop carries the key: {sent:?}"
    );
    assert!(fields(&w.reg).next().is_none(), "the field is gone");
}

// ---------------------------------------------------------------------------
// The beacon
// ---------------------------------------------------------------------------

#[test]
fn cyno_pushes_the_beacon_to_every_member() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    let f = w.field_of(ALICE);
    let sent = w.pushes();
    let beacons = named(&sent, "OnFleetJumpBeaconModuleChange");
    assert_eq!(beacons.len(), 1, "{sent:?}");
    let (_, args, to) = beacons[0];
    assert_eq!(
        args,
        &vec![
            PyValue::Int(ALICE),
            PyValue::Int(SYSTEM),
            PyValue::Int(f.ball_id),
            PyValue::Int(TYPE_CYNOSURAL_FIELD),
            PyValue::Bool(true),
        ],
        "(charID, solarsystemID, beaconID, typeID, active), the client's order"
    );
    let mut to = to.clone().expect("addressed");
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB], "the fleet, not the stranger");
    assert_eq!(
        w.reg.fleets().beacon_of(ALICE),
        Some(Beacon {
            solar_system_id: SYSTEM,
            beacon_id: f.ball_id,
            type_id: TYPE_CYNOSURAL_FIELD
        })
    );
}

#[test]
fn a_late_joiner_is_told_the_active_beacons_and_the_members_the_joiners() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    let a = w.field_of(ALICE);
    let _ = w.pushes();
    let id = w.reg.fleets().fleet_of(ALICE).expect("a fleet").fleet_id;
    // Carol lights nothing yet; she joins and is told Alice's field.
    call_fleet(
        &mut w.reg,
        ALICE,
        "Invite",
        vec![PyValue::Int(CAROL)],
        Some(id),
    )
    .expect("invite");
    let _ = w.pushes();
    call_fleet(
        &mut w.reg,
        CAROL,
        "AcceptInvite",
        vec![PyValue::Int(670)],
        Some(id),
    )
    .expect("accept");
    let sent = w.pushes();
    let beacons = named(&sent, "OnFleetJumpBeaconModuleChange");
    assert_eq!(beacons.len(), 1, "{sent:?}");
    assert_eq!(beacons[0].2, Some(vec![CAROL]), "told to the joiner alone");
    assert_eq!(beacons[0].1[0], PyValue::Int(ALICE));
    assert_eq!(beacons[0].1[2], PyValue::Int(a.ball_id));
    assert_eq!(beacons[0].1[4], PyValue::Bool(true));
    // And the other way: a pilot who lights while outside any fleet, then
    // joins one, is announced to its members.
    let mut w = world();
    let id = w.reg.fleets().fleet_of(ALICE).expect("a fleet").fleet_id;
    call_fleet(&mut w.reg, CAROL, "CreateFleet", vec![], None).expect("her own fleet");
    call_fleet(&mut w.reg, CAROL, "Init", vec![PyValue::Int(670)], None).expect("init");
    w.light(CAROL, CAROL_INDUSTRIAL, T0)
        .expect("lit in her own fleet");
    let c = w.field_of(CAROL);
    call_fleet(&mut w.reg, CAROL, "LeaveFleet", vec![], None).expect("leaves");
    let _ = w.pushes();
    call_fleet(
        &mut w.reg,
        ALICE,
        "Invite",
        vec![PyValue::Int(CAROL)],
        Some(id),
    )
    .expect("invite");
    let _ = w.pushes();
    call_fleet(
        &mut w.reg,
        CAROL,
        "AcceptInvite",
        vec![PyValue::Int(670)],
        Some(id),
    )
    .expect("accept");
    let sent = w.pushes();
    let told: Vec<&hubfx::Sent> = named(&sent, "OnFleetJumpBeaconModuleChange")
        .into_iter()
        .filter(|s| s.1[0] == PyValue::Int(CAROL))
        .collect();
    assert_eq!(told.len(), 1, "{sent:?}");
    let mut to = told[0].2.clone().expect("addressed");
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB]);
    assert_eq!(told[0].1[2], PyValue::Int(c.ball_id));
}

#[test]
fn a_member_who_leaves_takes_their_beacon_off_the_others_lists() {
    let mut w = world();
    w.light(BOB, BOB_COVERT, T0).expect("lit");
    let _ = w.pushes();
    call_fleet(&mut w.reg, BOB, "LeaveFleet", vec![], None).expect("leaves");
    let sent = w.pushes();
    let off: Vec<&hubfx::Sent> = named(&sent, "OnFleetJumpBeaconModuleChange");
    assert_eq!(off.len(), 1, "{sent:?}");
    assert_eq!(off[0].1[0], PyValue::Int(BOB));
    assert_eq!(off[0].1[4], PyValue::Bool(false));
    assert_eq!(off[0].2, Some(vec![ALICE]), "the leaver clears itself");
    // The field is still lit and still Bob's.
    assert!(field_of_ship(&w.reg, ship(BOB)).is_some());
}

#[test]
fn the_beacon_count_is_the_callers_fleets_by_system() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    w.light(BOB, BOB_COVERT, T0).expect("lit");
    let PyValue::Dict(d) = beacon_count(&w.reg, Some(ALICE)) else {
        panic!("a dict: the client iterates .iteritems()");
    };
    assert_eq!(d, vec![(PyValue::Int(2), PyValue::Int(SYSTEM))]);
    assert_eq!(beacon_count(&w.reg, Some(CAROL)), PyValue::Dict(vec![]));
    assert_eq!(beacon_count(&w.reg, None), PyValue::Dict(vec![]));
}

// ---------------------------------------------------------------------------
// The ball
// ---------------------------------------------------------------------------

#[test]
fn field_is_a_ball_and_a_slim_in_the_pilots_park() {
    let mut w = world();
    let at = w.reg.roster().get(ALICE).expect("in space").motion.position;
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    let f = w.field_of(ALICE);
    assert!((FIELD_ID_BASE..FIELD_ID_END).contains(&f.ball_id));
    let (balls, slims): (Vec<Ball>, Vec<Slim>) = balls_in(&w.reg, SYSTEM).into_iter().unzip();
    assert_eq!(balls.len(), 1);
    assert_eq!(balls[0].ball_id, f.ball_id);
    assert_eq!(balls[0].position, at, "at the ship, where it was lit");
    assert_eq!(balls[0].radius, FIELD_RADIUS as f32);
    assert_eq!(balls[0].mode, Mode::Stop);
    assert_eq!(balls[0].mass, 0.0);
    assert_eq!(
        (slims[0].item_id, slims[0].type_id, slims[0].owner_id),
        (f.ball_id, TYPE_CYNOSURAL_FIELD, ALICE)
    );
    assert_eq!(slims[0].corp_id, Some(CORP));
    assert_eq!(slims[0].char_id, None);
    assert!(balls_in(&w.reg, HIGH).is_empty(), "only in its own system");
    // A local ball: the grid lists it, and the scene a late arrival is sent.
    assert!(
        crate::grid::local_balls(&w.reg, SYSTEM)
            .iter()
            .any(|(id, _)| *id == f.ball_id)
    );
    let scene = crate::destiny::scene(&w.reg, SYSTEM, ship(BOB), None, 1);
    assert!(scene.park.balls.iter().any(|b| b.ball_id == f.ball_id));
    assert!(scene.slims.iter().any(|s| s.item_id == f.ball_id));
    // Approach and orbit resolve it.
    let t = target(&w.reg, f.ball_id).expect("a target");
    assert_eq!((t.position, t.radius), (at, FIELD_RADIUS));
    assert!(w.reg.ball_target(f.ball_id).is_some());
    // The pilots in the system are sent one AddBalls2 naming it.
    let sent = w.pushes();
    let adds: Vec<&hubfx::Sent> = named(&sent, "DoDestinyUpdate")
        .into_iter()
        .filter(|s| {
            s.1.iter()
                .map(flat)
                .collect::<String>()
                .contains("AddBalls2")
        })
        .collect();
    assert_eq!(adds.len(), 1, "{sent:?}");
    assert!(
        adds[0]
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains(&f.ball_id.to_string())
    );
    let mut to = adds[0].2.clone().expect("addressed");
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB, CAROL]);
}

#[test]
fn cyno_ends_with_the_cycle_and_removes_the_ball() {
    let mut w = world();
    // A one-shot: the client's `repeat` of 1 is one cycle.
    w.light_once(ALICE, ALICE_CYNO, T0).expect("lit");
    let f = w.field_of(ALICE);
    let _ = w.pushes();
    crate::modules::tick(&mut w.reg, T0 + CYCLE - 1);
    assert!(field_of_ship(&w.reg, ship(ALICE)).is_some(), "still lit");
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    assert!(fields(&w.reg).next().is_none(), "the field is gone");
    assert!(balls_in(&w.reg, SYSTEM).is_empty());
    assert!(w.reg.fleets().beacon_of(ALICE).is_none());
    let sent = w.pushes();
    let removed: Vec<&hubfx::Sent> = named(&sent, "DoDestinyUpdate")
        .into_iter()
        .filter(|s| {
            s.1.iter()
                .map(flat)
                .collect::<String>()
                .contains("RemoveBalls")
        })
        .collect();
    assert_eq!(removed.len(), 1, "{sent:?}");
    assert!(
        removed[0]
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains(&f.ball_id.to_string())
    );
    let off = named(&sent, "OnFleetJumpBeaconModuleChange");
    assert_eq!(off.len(), 1, "{sent:?}");
    assert_eq!(off[0].1[4], PyValue::Bool(false));
    assert_eq!(
        off[0].2.clone().map(|mut t| {
            t.sort_unstable();
            t
        }),
        Some(vec![ALICE, BOB])
    );
}

#[test]
fn a_pilot_who_leaves_space_takes_the_field_down() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    let _ = w.reg.roster_mut().leave(ALICE);
    crate::modules::tick(&mut w.reg, T0 + 10);
    assert!(fields(&w.reg).next().is_none());
    assert!(w.reg.fleets().beacon_of(ALICE).is_none());
}

#[test]
fn turning_the_module_off_takes_the_field_down() {
    let mut w = world();
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    crate::modules::set_module_online(&mut w.reg, Some(ALICE), ALICE_CYNO, false, T0 + 5)
        .expect("offline");
    crate::modules::tick(&mut w.reg, T0 + 10);
    assert!(fields(&w.reg).next().is_none());
}

// ---------------------------------------------------------------------------
// Immobility (data, not code)
// ---------------------------------------------------------------------------

#[test]
fn a_lit_cyno_holds_the_ship_still_and_unwarpable_until_it_goes_out() {
    let mut w = world();
    let before = crate::dogma::hull(&w.reg, ship(ALICE)).expect("a sheet");
    assert!(before.max_velocity > 0.0);
    assert!(crate::tackle::scrammed(&w.reg, ALICE).is_none());
    w.light_once(ALICE, ALICE_CYNO, T0).expect("lit");
    let held = crate::dogma::hull(&w.reg, ship(ALICE)).expect("a sheet");
    assert_eq!(held.max_velocity, 0.0, "speedFactor -100 post-percent");
    assert!(
        crate::tackle::scrammed(&w.reg, ALICE).is_some(),
        "warpScrambleStatus + siegeModeWarpStatus > 0: no warp"
    );
    // The park is told the new top speed.
    let sent = w.pushes();
    assert!(
        named(&sent, "DoDestinyUpdate").iter().any(|s| s
            .1
            .iter()
            .map(flat)
            .collect::<String>()
            .contains("SetMaxSpeed")),
        "{sent:?}"
    );
    crate::modules::tick(&mut w.reg, T0 + CYCLE);
    assert_eq!(
        crate::dogma::hull(&w.reg, ship(ALICE))
            .expect("a sheet")
            .max_velocity,
        before.max_velocity
    );
    assert!(crate::tackle::scrammed(&w.reg, ALICE).is_none());
}

#[test]
fn a_ship_with_a_lit_cyno_does_not_dock() {
    let mut w = world();
    assert!(refuse_dock(&w.reg, Some(ALICE)).is_ok());
    w.light(ALICE, ALICE_CYNO, T0).expect("lit");
    let e = refuse_dock(&w.reg, Some(ALICE)).expect_err("busy");
    let (key, d) = raised(&e);
    assert_eq!(key, "CantDockWhileModuleActive");
    assert_eq!(arg(&d, "moduleType"), &PyValue::Int(CYNO));
    assert!(refuse_dock(&w.reg, Some(BOB)).is_ok(), "a different ship");
    assert!(refuse_dock(&w.reg, None).is_ok());
    assert_eq!(running_module_on(&w.reg, ship(ALICE)), Some(CYNO));
}

// ---------------------------------------------------------------------------
// Ids and the pure parts
// ---------------------------------------------------------------------------

#[test]
fn field_ids_clear_of_every_other_range() {
    let ours = FIELD_ID_BASE..FIELD_ID_END;
    let clear = |lo: i64, hi: i64| hi <= ours.start || lo >= ours.end;
    assert!(clear(
        crate::scan::PROBE_ID_BASE,
        crate::scan::PROBE_ID_BASE + 100_000_000_000
    ));
    assert!(clear(crate::missile::ID_BASE, crate::missile::ID_END));
    assert!(clear(crate::econ_exchange::NPC_ORDER_BASE, i64::MAX));
    assert!(clear(crate::npc::MIN_NPC_ID, crate::npc::END_NPC_ID));
    assert!(clear(
        crate::planet::FIRST_PIN_ID,
        crate::planet::FIRST_PIN_ID + 100_000_000_000
    ));
    assert!(clear(crate::site::INSTANCE_BASE, crate::site::INSTANCE_END));
    assert!(clear(0, 2_147_483_648), "item, character and type ids");
}

#[test]
fn ids_wrap_inside_the_range_and_skip_a_lit_field() {
    let mut c = Cynos::default();
    let first = c.alloc();
    assert_eq!(first, FIELD_ID_BASE);
    c.fields.insert(
        1,
        Field {
            ball_id: FIELD_ID_BASE + 1,
            character_id: 0,
            ship_id: 0,
            module_id: 1,
            module_type_id: 0,
            type_id: 0,
            system_id: 0,
            position: [0.0; 3],
            spawn_radius: 0.0,
            corporation_id: 0,
            alliance_id: None,
        },
    );
    assert_eq!(c.alloc(), FIELD_ID_BASE + 2, "the lit one is skipped");
    c.next = FIELD_ID_END - FIELD_ID_BASE;
    assert_eq!(c.alloc(), FIELD_ID_BASE, "wrapped");
}

#[test]
fn the_effect_is_the_one_the_hooks_test() {
    assert!(is_cyno(EFFECT_CYNOSURAL_GENERATION));
    assert!(!is_cyno(crate::dogma::effect::MICROWARPDRIVE));
    assert_eq!(EFFECT_NAME, "cynosuralGeneration");
}

// ---------------------------------------------------------------------------
// The cyno inhibitor (NW-4)
// ---------------------------------------------------------------------------

/// A fresh inhibitor stack in Carol's cargo. The stack's item id.
fn inhibitor_stack(w: &mut World) -> i64 {
    let changes = w
        .reg
        .apply_items(&[crate::item::Edit::Create {
            type_id: INHIBITOR,
            quantity: 1,
            to: crate::item::Slot::new(CAROL, ship(CAROL), FLAG_CARGO),
        }])
        .expect("a known type");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("created")
}

/// Carol launches an inhibitor from her cargo. The unit's id.
fn launch_inhibitor(w: &mut World) -> i64 {
    let stack = inhibitor_stack(w);
    crate::anchor::launch_one(&mut w.reg, CAROL, stack).expect("launched")
}

/// Run the unit clock far enough for any activation time.
fn activate_inhibitor(w: &mut World) {
    let later = crate::pyutil::filetime_now() + 100_000 * 10_000_000;
    crate::anchor::tick(&mut w.reg, later);
}

#[test]
fn an_inhibitor_refuses_a_cyno_in_range() {
    let mut w = world();
    launch_inhibitor(&mut w);
    activate_inhibitor(&mut w);
    let e = w.light(ALICE, ALICE_CYNO, T0).expect_err("inhibited");
    let (key, d) = raised(&e);
    assert_eq!(key, "CynosuralGenerationInhibited");
    assert_eq!(
        flat(arg(&d, "type")),
        format!("{} {} ", crate::modules::UE_TYPEID, INHIBITOR)
    );
    assert!(matches!(arg(&d, "range"), PyValue::Real(r) if *r > 0.0));
    assert!(fields(&w.reg).next().is_none(), "no field was made");
    assert_eq!(w.ozone(ALICE, FLAG_CARGO), PLENTY, "no ozone was burned");
    // Out of its range the same module lights.
    let range = inhibited_by(&w.reg, SYSTEM, [1.0e7, 2.0e6, -3.0e6])
        .expect("in range")
        .0;
    place(
        &mut w.reg,
        ALICE,
        SYSTEM,
        [1.0e7 + range * 3.0, 2.0e6, -3.0e6],
    );
    w.light(ALICE, ALICE_CYNO, T0).expect("far from it");
}

#[test]
fn not_a_covert_one() {
    let mut w = world();
    launch_inhibitor(&mut w);
    activate_inhibitor(&mut w);
    w.light(BOB, BOB_COVERT, T0)
        .expect("a covert cyno is not affected");
    assert_eq!(w.field_of(BOB).type_id, TYPE_COVERT_CYNOSURAL_FIELD);
}

#[test]
fn not_before_it_activates() {
    let mut w = world();
    let unit = launch_inhibitor(&mut w);
    assert!(
        matches!(
            crate::anchor::state_of(&w.reg, unit),
            Some(crate::anchor::State::Anchoring(_))
        ),
        "launched, not yet active"
    );
    w.light(ALICE, ALICE_CYNO, T0)
        .expect("nothing is refused yet");
    // The field goes out with its cycle; then the unit activates.
    crate::modules::stop(&mut w.reg, ALICE_CYNO, T0, None);
    activate_inhibitor(&mut w);
    assert_eq!(
        crate::anchor::state_of(&w.reg, unit),
        Some(crate::anchor::State::Anchored)
    );
    let e = w
        .light(ALICE, ALICE_CYNO, T0)
        .expect_err("now it is active");
    assert_eq!(raised(&e).0, "CynosuralGenerationInhibited");
}

#[test]
fn an_inhibitor_cannot_be_picked_up() {
    let mut w = world();
    let unit = launch_inhibitor(&mut w);
    let e = crate::anchor::scoop_unit(&mut w.reg, CAROL, unit, FLAG_CARGO).expect_err("refused");
    let ServiceError::Raise(v) = &e else {
        panic!("not raised: {e:?}")
    };
    assert!(
        flat(v).contains("cannot be retrieved once deployed"),
        "{e:?}"
    );
    assert!(
        crate::anchor::state_of(&w.reg, unit).is_some(),
        "still there"
    );
}

#[test]
fn an_inhibitor_keeps_its_rows_distance_from_its_own_kind() {
    let mut w = world();
    launch_inhibitor(&mut w);
    let second = inhibitor_stack(&mut w);
    let e = crate::anchor::launch_one(&mut w.reg, CAROL, second).expect_err("too close");
    assert!(e.contains("close to another one"), "{e}");
    // Clear of the row's spacing (invented 40 km) it launches.
    place(&mut w.reg, CAROL, SYSTEM, [1.0e7 + 60_000.0, 2.0e6, -3.0e6]);
    crate::anchor::launch_one(&mut w.reg, CAROL, second).expect("far enough");
}

#[test]
fn an_inhibitor_keeps_away_from_an_upwell_structure() {
    let mut w = world();
    let env = crate::structure::Env::new(crate::pyutil::filetime_now());
    let s = crate::structure::Structure::anchored(
        990_640_900,
        990_640_901,
        CORP,
        SYSTEM,
        [1.0e7 + 5_000.0, 2.0e6, -3.0e6],
        0.0,
        "Invented Keepstar",
        0,
        &env,
    );
    crate::structure::put(w.reg.deltas_mut(), &s);
    let stack = inhibitor_stack(&mut w);
    let e = crate::anchor::launch_one(&mut w.reg, CAROL, stack).expect_err("too close");
    assert!(e.contains("gate, station or structure"), "{e}");
    // Beyond the row's distance (invented 15 km) it launches.
    place(&mut w.reg, CAROL, SYSTEM, [1.0e7 + 30_000.0, 2.0e6, -3.0e6]);
    crate::anchor::launch_one(&mut w.reg, CAROL, stack).expect("clear of it");
}

#[test]
fn an_inhibitor_is_refused_above_the_rows_security_ceiling() {
    let mut w = world();
    // HIGH is 0.9, the row's invented ceiling is 0.3.
    place(&mut w.reg, CAROL, HIGH, [0.0; 3]);
    let stack = inhibitor_stack(&mut w);
    let e = crate::anchor::launch_one(&mut w.reg, CAROL, stack).expect_err("too secure");
    assert!(e.contains("system this secure"), "{e}");
    place(&mut w.reg, CAROL, SYSTEM, [1.0e7, 2.0e6, -3.0e6]);
    crate::anchor::launch_one(&mut w.reg, CAROL, stack).expect("null sec is fine");
}
