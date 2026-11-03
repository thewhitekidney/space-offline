//! NW-9a tests. **Every id and number is invented**: types 990,195,xxx,
//! systems 30,195,xxx, a 20 km Lance 1 km wide. No client value is asserted;
//! the attribute ids are the module's own named constants.

use super::*;
use crate::celestial::Universe;
use crate::space_state::{Motion, Presence};

const SYSTEM: i64 = 30_195_001;
pub(crate) const ALICE: i64 = 90_195_001;
const BOB: i64 = 90_195_002;
const CAROL: i64 = 90_195_003;
const DAVE: i64 = 90_195_004;
const CORP: i64 = 98_195_001;
const LANCE: i64 = 990_195_001;
const FUEL: i64 = 990_195_002;
const HULL: i64 = 990_195_003;
const IMMUNE_HULL: i64 = 990_195_004;
const LEGACY: i64 = 990_195_005;
const DISRUPTIVE: i64 = 990_195_006;
const FRIGATE: i64 = 990_195_007;
const REAPER: i64 = 990_195_008;
const BOSONIC: i64 = 990_195_009;
/// Alice is a long way from the origin, so an aim read as a world point fails.
const OFF: f64 = 1.0e9;
const LEGACY_EFFECT: i64 = LEGACY_EFFECTS[0];
pub(crate) const ALICE_LANCE: i64 = 100_195_101;
const FUEL_ITEM: i64 = 100_195_102;
const CARGO: i64 = 5;
const HI_SLOT_0: i64 = 27;

/// Invented: warning 4 s, damage 3 s in 1 s ticks, hold 30 s / 100 s, reuse 60 s.
const WARN_MS: i64 = 4_000;
const NEED: f64 = 10.0;
const BURN: i64 = 50;

pub(crate) fn ship(who: i64) -> i64 {
    100_195_000 + (who - ALICE) + 1
}

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let row = format!(
        "{SYSTEM}\t19000001\t29000001\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture {SYSTEM}"
    );
    c.insert(
        "universe",
        Universe::parse(
            &[row],
            &["19000001\t\t\t\t\tFixture region".to_owned()],
            &["29000001\t19000001\t\t\t\tFixture constellation".to_owned()],
        )
        .expect("the invented rows parse"),
    );
    let rows = format!(
        "{}type\t{LANCE}\t588\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t0\t12,16\t\
         {ATTR_CAP_NEED}={NEED};{ATTR_RANGE}=20000.0;{ATTR_CYCLE}=60000.0;114=1000.0;116=0.0;117=0.0;118=0.0;\
         713={FUEL}.0;714={BURN}.0;{ATTR_WARNING}={WARN_MS}.0;{ATTR_RADIUS}=1000.0;\
         {ATTR_DAMAGE_DURATION}=3000.0;{ATTR_DAMAGE_CYCLE}=1000.0;{ATTR_POINT_TARGETED}=1.0;\
         {ATTR_NO_JUMP}=100000.0;{ATTR_IMMOBILE}=30000.0;{ATTR_SHAPE}=1.0;{ATTR_FIXED_RANGE}=1.0\tInvented Lance\n\
         type\t{LEGACY}\t588\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{LEGACY_EFFECT}\t12,16,{LEGACY_EFFECT}\t\
         73=60000.0;114=5000.0;{ATTR_DAMAGE_DELAY}=2000.0;{ATTR_NO_JUMP}=100000.0;{ATTR_IMMOBILE}=30000.0\tInvented Legacy\n\
         type\t{DISRUPTIVE}\t588\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t0\t12,16\t\
         {ATTR_CAP_NEED}={NEED};{ATTR_RANGE}=20000.0;{ATTR_CYCLE}=60000.0;114=1000.0;116=0.0;117=0.0;118=0.0;\
         713={FUEL}.0;714={BURN}.0;{ATTR_WARNING}={WARN_MS}.0;{ATTR_RADIUS}=1000.0;\
         {ATTR_DAMAGE_DURATION}=3000.0;{ATTR_DAMAGE_CYCLE}=1000.0;{ATTR_POINT_TARGETED}=1.0;\
         {ATTR_NO_JUMP}=100000.0;{ATTR_IMMOBILE}=30000.0;{ATTR_FIXED_RANGE}=1.0;{ATTR_SHAPE}=1.0;{ATTR_DEBUFF_DURATION}=45000.0;{ATTR_NEEDS_SIEGE}=1.0\tInvented Disruptive Lance\n\
         type\t{BOSONIC}\t588\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t0\t12,16\t\
         {ATTR_CAP_NEED}={NEED};{ATTR_RANGE}=20000.0;{ATTR_CYCLE}=60000.0;114=1000.0;116=0.0;117=0.0;118=0.0;\
         713={FUEL}.0;714={BURN}.0;{ATTR_WARNING}={WARN_MS}.0;{ATTR_RADIUS}=1000.0;\
         {ATTR_DAMAGE_DURATION}=3000.0;{ATTR_DAMAGE_CYCLE}=1000.0;{ATTR_POINT_TARGETED}=1.0;\
         {ATTR_NO_JUMP}=100000.0;{ATTR_IMMOBILE}=30000.0;{ATTR_FIXED_RANGE}=1.0;{ATTR_SHAPE}=3.0\tInvented Bosonic\n\
         type\t{REAPER}\t588\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t0\t12,16\t\
         {ATTR_CAP_NEED}={NEED};{ATTR_RANGE}=20000.0;{ATTR_CYCLE}=60000.0;114=1000.0;116=0.0;117=0.0;118=0.0;\
         713={FUEL}.0;714={BURN}.0;{ATTR_WARNING}={WARN_MS}.0;{ATTR_RADIUS}=1000.0;\
         {ATTR_DAMAGE_DURATION}=3000.0;{ATTR_DAMAGE_CYCLE}=1000.0;{ATTR_POINT_TARGETED}=1.0;\
         {ATTR_NO_JUMP}=100000.0;{ATTR_IMMOBILE}=30000.0;{ATTR_SHAPE}=2.0\tInvented Reaper\n\
         type\t{FRIGATE}\t25\t6\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t\tInvented Frigate\n\
         type\t{FUEL}\t18\t4\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tInvented Fuel\n\
         type\t{HULL}\t659\t6\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t482=1000.0;55=300000.0\tInvented Titan\n\
         type\t{IMMUNE_HULL}\t659\t6\t1\t1\t0.0\t0.0\t0.0\t1.0\t0\t\t{ATTR_IMMUNE}=1.0\tInvented Immune Titan\n",
        crate::typedata::FIXTURE,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

fn place(reg: &mut Registry, who: i64, at: [f64; 3]) {
    let _ = reg.roster_mut().leave(who);
    reg.roster_mut().arrive(Presence::arriving(
        who,
        ship(who),
        SYSTEM,
        Motion::arrived(at, [0.0; 3]),
    ));
}

/// Alice (at x = 1e9) with a Lance and fuel; Bob 10 km down the axis (+x), Carol 10 km
/// along it but 5 km off, Dave an immune ship 12 km down it.
pub(crate) fn world() -> Registry {
    world_with(LANCE, HULL)
}

fn world_with(lance: i64, carol_hull: i64) -> Registry {
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
            pilot(DAVE, "Dave"),
        ],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for i in items.iter_mut() {
        if i.item_id == ship(DAVE) {
            i.type_id = IMMUNE_HULL;
            i.group_id = 659;
        } else if i.item_id == ship(CAROL) {
            i.type_id = carol_hull;
            i.group_id = if carol_hull == FRIGATE { 25 } else { 659 };
        } else if [ALICE, BOB].iter().any(|p| i.item_id == ship(*p)) {
            i.type_id = HULL;
            i.group_id = 659;
        }
    }
    let t = crate::item::find_in(&content, lance).expect("a test type");
    items.push(crate::item::Item {
        item_id: ALICE_LANCE,
        type_id: lance,
        owner_id: ALICE,
        location_id: ship(ALICE),
        flag_id: HI_SLOT_0,
        quantity: 1,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton: true,
    });
    let f = crate::item::find_in(&content, FUEL).expect("a test type");
    items.push(crate::item::Item {
        item_id: FUEL_ITEM,
        type_id: FUEL,
        owner_id: ALICE,
        location_id: ship(ALICE),
        flag_id: CARGO,
        quantity: BURN * 3,
        group_id: f.group_id,
        category_id: f.category_id,
        singleton: false,
    });
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_196_000,
        ..crate::store::Snapshot::default()
    });
    for p in [ALICE, BOB] {
        reg.character_mut(p).expect("a pilot").ship_type_id = HULL;
    }
    reg.character_mut(CAROL).expect("a pilot").ship_type_id = carol_hull;
    reg.character_mut(DAVE).expect("a pilot").ship_type_id = IMMUNE_HULL;
    place(&mut reg, ALICE, [OFF, 0.0, 0.0]);
    place(&mut reg, BOB, [OFF + 10_000.0, 0.0, 0.0]);
    place(&mut reg, CAROL, [OFF + 10_000.0, 5_000.0, 0.0]);
    place(&mut reg, DAVE, [OFF + 12_000.0, 0.0, 0.0]);
    let now = crate::pyutil::filetime_now();
    reg.modules_mut().set_capacitor(ship(ALICE), 1000.0, now);
    let _ = reg.take_pending();
    reg
}

/// The aim as the client sends it: an offset from the firer.
pub(crate) const AIM: [f64; 3] = [15_000.0, 0.0, 0.0];

pub(crate) fn hurt(reg: &Registry, who: i64) -> bool {
    reg.combat().ship(ship(who)).is_some()
}

fn fuel_left(reg: &Registry) -> i64 {
    reg.item(FUEL_ITEM).map_or(0, |i| i.quantity)
}

fn run_to(reg: &mut Registry, t: BlueTime) {
    tick(reg, Instant::now(), t);
}

#[test]
fn a_lance_needs_its_fuel_and_cap() {
    let now = crate::pyutil::filetime_now();
    // No cap: refused, nothing burned, no hold, nothing booked.
    let mut reg = world();
    reg.modules_mut().set_capacitor(ship(ALICE), 1.0, now);
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).is_err());
    assert_eq!(fuel_left(&reg), BURN * 3);
    assert!(!immobile_at(&reg, ship(ALICE), now));
    // No fuel: refused, capacitor kept.
    let mut reg = world();
    reg.apply_items(&[crate::item::Edit::Destroy { item_id: FUEL_ITEM }])
        .expect("the fuel goes");
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).is_err());
    assert!(!immobile_at(&reg, ship(ALICE), now));
    // Both: fires, burns one cycle's fuel, and may not fire again at once.
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    assert_eq!(fuel_left(&reg), BURN * 2);
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now + 1).is_err());
    assert_eq!(fuel_left(&reg), BURN * 2);
}

#[test]
fn the_warning_delays_damage() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS - 1) * MSEC);
    assert!(!hurt(&reg, BOB), "nothing during the warning");
    // [A] when exactly the first tick lands; by warning + duration it has.
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(
        hurt(&reg, BOB),
        "hurt once warning and duration have passed"
    );
}

#[test]
fn the_beam_ticks_for_its_duration_and_then_stops() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS + 10_000) * MSEC);
    assert!(
        reg.ext::<State>().beams.is_empty(),
        "three ticks, then done"
    );
}

#[test]
fn a_ship_outside_the_cylinder_takes_nothing() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(hurt(&reg, BOB), "on the axis");
    assert!(!hurt(&reg, CAROL), "5 km off a 1 km cylinder");
    assert!(!hurt(&reg, ALICE), "never the firer");
    // The shape itself: behind, beyond the end, and at the edge.
    let o = [OFF, 0.0, 0.0];
    let x = [1.0, 0.0, 0.0];
    let at = |dx: f64, dy: f64| [OFF + dx, dy, 0.0];
    assert!(!in_cylinder(o, x, 20_000.0, 1_000.0, at(-1.0, 0.0)));
    assert!(!in_cylinder(o, x, 20_000.0, 1_000.0, at(20_001.0, 0.0)));
    assert!(in_cylinder(o, x, 20_000.0, 1_000.0, at(20_000.0, 1_000.0)));
    assert!(!in_cylinder(o, x, 20_000.0, 1_000.0, at(5.0, 1_001.0)));
    // The length: fixed range, or the aim past the hull capped by range.
    assert_eq!(
        cylinder_length(5_000.0, 100.0, Some(20_000.0), true),
        20_000.0
    );
    assert_eq!(
        cylinder_length(5_000.0, 100.0, Some(20_000.0), false),
        4_900.0
    );
    assert_eq!(
        cylinder_length(50_000.0, 100.0, Some(20_000.0), false),
        20_000.0
    );
}

#[test]
fn a_fired_titan_is_held() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    assert!(!immobile_at(&reg, ship(ALICE), now));
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    assert!(immobile_at(&reg, ship(ALICE), now));
    // [A] whether the hold starts at the click or at the warning's end: free
    // after click + warning + 2428 either way.
    assert!(!immobile_at(
        &reg,
        ship(ALICE),
        now + (WARN_MS + 30_000) * MSEC
    ));
    // No docking while held.
    assert!(dock_refusal(&reg, Some(ALICE)).is_err());
    assert!(dock_refusal(&reg, Some(BOB)).is_ok());
    // The guard row refuses a warp now, and is a member of the table.
    assert!(immobile_guard(&reg, ALICE).is_some());
    assert!(
        crate::movement_guard::WARP_GUARDS
            .contains(&(immobile_guard as crate::movement_guard::Guard))
    );
    assert!(immobile_guard(&reg, BOB).is_none(), "only the firer");
}

#[test]
fn no_jump_or_cloak_after() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    assert!(check_cloak(&reg, ship(ALICE), now).is_ok());
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    assert!(jump_guard(&reg, ALICE).is_some());
    assert!(
        crate::movement_guard::JUMP_GUARDS.contains(&(jump_guard as crate::movement_guard::Guard))
    );
    assert!(check_cloak(&reg, ship(ALICE), now).is_err());
    assert!(
        check_cloak(&reg, ship(ALICE), now + (WARN_MS + 100_000) * MSEC).is_ok(),
        "free after click + warning + 2427"
    );
    assert!(jump_guard(&reg, BOB).is_none());
    // The hold outlasts the immobility, and the clock forgets it.
    assert!(!immobile_at(&reg, ship(ALICE), now + 50_000 * MSEC));
    assert!(no_jump_at(&reg, ship(ALICE), now + 50_000 * MSEC));
    run_to(&mut reg, now + 200_000 * MSEC);
    assert!(reg.ext::<State>().holds.is_empty());
}

#[test]
fn the_immune_are_refused() {
    let now = crate::pyutil::filetime_now();
    // The Lance spares an immune ship in its cylinder.
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + WARN_MS * MSEC);
    assert!(hurt(&reg, BOB));
    assert!(!hurt(&reg, DAVE), "immune");
    // The legacy one is refused at the click, by key; a plain titan passes.
    let reg = world();
    let r = legacy_refusal(&reg, LEGACY, LEGACY_EFFECT, Some(ship(DAVE))).expect("refused");
    assert_eq!(r.user_error, "CantActivateDoomsDayOnImmuneShips");
    assert!(legacy_refusal(&reg, LEGACY, LEGACY_EFFECT, Some(ship(BOB))).is_none());
    let r = legacy_refusal(&reg, LEGACY, LEGACY_EFFECT, Some(999)).expect("refused");
    assert_eq!(r.user_error, "CantActivateDoomsDayOnNonShip");
    assert!(
        legacy_refusal(&reg, LEGACY, 1, Some(ship(DAVE))).is_none(),
        "other effects"
    );
}

#[test]
fn a_legacy_cycle_strikes_after_its_delay_and_holds_the_firer() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    let c = crate::modules::Cycle {
        character_id: ALICE,
        ship_id: ship(ALICE),
        module_id: ALICE_LANCE,
        type_id: LEGACY,
        effect_id: LEGACY_EFFECT,
        target_id: Some(ship(BOB)),
        at: now,
        index: 0,
        charge_type_id: None,
    };
    on_legacy_cycle(&mut reg, &c, now);
    assert!(immobile_at(&reg, ship(ALICE), now));
    run_to(&mut reg, now);
    assert!(!hurt(&reg, BOB), "not at the click");
    // [A] the delay counts from the cycle start; well past it, it has landed.
    run_to(&mut reg, now + 10_000 * MSEC);
    assert!(hurt(&reg, BOB));
    assert!(!hurt(&reg, CAROL));
}

#[test]
fn a_non_capital_target_is_refused_for_a_legacy_doomsday() {
    let reg = world_with(LANCE, FRIGATE);
    let r = legacy_refusal(&reg, LEGACY, LEGACY_EFFECT, Some(ship(CAROL))).expect("refused");
    assert_eq!(r.user_error, "CannotActivateSuperWeaponOnNonCapitalShip");
}

/// A siege-family module of Alice's, running (invented module id).
fn run_siege(reg: &mut Registry, now: BlueTime, stop_at: Option<BlueTime>) {
    reg.modules_mut().insert_running(crate::modules::Running {
        character_id: ALICE,
        ship_id: ship(ALICE),
        module_id: 100_195_199,
        type_id: LANCE,
        effect: crate::modules::Effect {
            effect_id: crate::module_fuel::EFFECT_SIEGE,
            name: "invented",
            category: 1,
            guid: "",
            is_offensive: false,
            duration_attribute: 73,
            warp_safe: None,
        },
        target_id: None,
        started: now,
        period: 60_000 * MSEC,
        repeat: 1,
        cycles: 1,
        stop_at,
    });
}

#[test]
fn a_disruptive_lance_needs_an_active_siege_module() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(DISRUPTIVE, HULL);
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).is_err());
    assert_eq!(fuel_left(&reg), BURN * 3, "nothing burned");
    assert!(!immobile_at(&reg, ship(ALICE), now));
    // A siege that is ending is not active.
    run_siege(&mut reg, now, Some(now - 1));
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).is_err());
    let mut reg = world_with(DISRUPTIVE, HULL);
    run_siege(&mut reg, now, None);
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    assert_eq!(fuel_left(&reg), BURN * 2);
    assert!(immobile_at(&reg, ship(ALICE), now));
}

#[test]
fn a_disruptive_lance_debuffs_what_it_hits_for_the_data_duration() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(DISRUPTIVE, HULL);
    run_siege(&mut reg, now, None);
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(hurt(&reg, BOB));
    let t = now + (WARN_MS + 10_000) * MSEC;
    assert!(disrupted_at(&reg, ship(BOB), t));
    assert!(!disrupted_at(&reg, ship(CAROL), t), "outside the cylinder");
    assert!(!disrupted_at(&reg, ship(ALICE), t), "not the firer");
    // The window is 5412 long, counted from the last tick that hit (the
    // third, at WARN_MS + 2000): both edges of that window.
    let last = now + (WARN_MS + 2_000) * MSEC;
    assert!(disrupted_at(&reg, ship(BOB), last + 45_000 * MSEC - 1));
    assert!(!disrupted_at(&reg, ship(BOB), last + 45_000 * MSEC));
    // The refusals: no cloak, no dock for a victim.
    assert!(check_cloak(&reg, ship(BOB), t).is_err());
    assert!(!immobile_at(&reg, ship(BOB), t), "its own hold is not set");
    // A plain Lance debuffs nobody.
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(!disrupted_at(&reg, ship(BOB), t));
}

#[test]
fn the_bosonic_cone_hits_inside_the_cone_only() {
    let apex = [0.0; 3];
    let axis = [1.0, 0.0, 0.0];
    let cos = std::f64::consts::FRAC_1_SQRT_2; // an invented 45 degrees
    assert!(in_cone(apex, axis, 10_000.0, cos, [5_000.0, 4_000.0, 0.0]));
    assert!(!in_cone(apex, axis, 10_000.0, cos, [5_000.0, 6_000.0, 0.0]));
    assert!(!in_cone(apex, axis, 10_000.0, cos, [11_000.0, 0.0, 0.0]));
    assert!(!in_cone(apex, axis, 10_000.0, cos, [-1_000.0, 0.0, 0.0]));
}

#[test]
fn a_bosonic_fires_its_cone_after_the_warning_and_holds_the_firer() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(BOSONIC, HULL);
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    assert_eq!(fuel_left(&reg), BURN * 2);
    assert!(immobile_at(&reg, ship(ALICE), now));
    run_to(&mut reg, now + (WARN_MS - 1) * MSEC);
    assert!(!hurt(&reg, BOB), "nothing during the warning");
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(hurt(&reg, BOB), "10 km down the axis");
    assert!(!disrupted_at(
        &reg,
        ship(BOB),
        now + (WARN_MS + 4_000) * MSEC
    ));
}

/// A slice 20 km long opening +-5 km at its far end, as an `ArcPath` draws
/// it: two offsets from the firer. Bob (10 km down +x) is inside, Carol
/// (10 km down, 5 km off) is outside at that depth, Dave (12 km) is immune.
const P0: [f64; 3] = [20_000.0, 5_000.0, 0.0];
const P1: [f64; 3] = [20_000.0, -5_000.0, 0.0];

fn pushed(reg: &mut Registry) -> Vec<(String, Vec<PyValue>)> {
    reg.take_pending()
        .into_iter()
        .map(|(_, p)| {
            let (n, a) = p.to_push();
            (n.to_owned(), a)
        })
        .collect()
}

/// A KeyVal's attribute (the marshal dict is `(value, key)` pairs).
fn attr_of(info: &PyValue, name: &str) -> Option<PyValue> {
    let PyValue::Instance { state, .. } = info else {
        return None;
    };
    let PyValue::Dict(d) = &**state else {
        return None;
    };
    d.iter()
        .find(|(_, k)| *k == crate::pyutil::bstr(name))
        .map(|(v, _)| v.clone())
}

#[test]
fn the_slice_geometry_is_a_flat_triangle() {
    let apex = [OFF, 0.0, 0.0];
    let at = |dx: f64, dy: f64, dz: f64| [OFF + dx, dy, dz];
    // Inside: near the middle of the wedge, and near an edge.
    assert!(in_slice(apex, P0, P1, 1_000.0, at(10_000.0, 0.0, 0.0)));
    assert!(in_slice(apex, P0, P1, 1_000.0, at(10_000.0, 2_400.0, 0.0)));
    // Outside: 5 km off at 10 km depth (the wedge is +-2.5 km there), behind
    // the apex, beyond the far side, and above the slab.
    assert!(!in_slice(apex, P0, P1, 1_000.0, at(10_000.0, 5_000.0, 0.0)));
    assert!(!in_slice(apex, P0, P1, 1_000.0, at(-1.0, 0.0, 0.0)));
    assert!(!in_slice(apex, P0, P1, 1_000.0, at(21_000.0, 0.0, 0.0)));
    assert!(!in_slice(apex, P0, P1, 1_000.0, at(10_000.0, 0.0, 1_500.0)));
    assert!(in_slice(apex, P0, P1, 1_000.0, at(10_000.0, 0.0, 900.0)));
    // Two points on one line from the firer make no slice.
    assert!(!in_slice(
        apex,
        P0,
        [P0[0] * 2.0, P0[1] * 2.0, 0.0],
        1e6,
        apex
    ));
}

#[test]
fn a_reaper_hits_what_is_in_the_slice_after_its_warning() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(REAPER, HULL);
    fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now).expect("it fires");
    run_to(&mut reg, now + (WARN_MS - 1) * MSEC);
    assert!(!hurt(&reg, BOB), "nothing during the warning");
    run_to(&mut reg, now + (WARN_MS + 3_000) * MSEC);
    assert!(hurt(&reg, BOB), "in the slice");
    assert!(!hurt(&reg, CAROL), "5 km off at 10 km depth");
    assert!(!hurt(&reg, ALICE), "never the firer");
}

#[test]
fn a_reaper_pays_fuel_and_cap_and_holds_the_firer() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(REAPER, HULL);
    reg.modules_mut().set_capacitor(ship(ALICE), 1.0, now);
    assert!(fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now).is_err());
    assert_eq!(fuel_left(&reg), BURN * 3);
    assert!(!immobile_at(&reg, ship(ALICE), now));
    let mut reg = world_with(REAPER, HULL);
    fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now).expect("it fires");
    assert_eq!(fuel_left(&reg), BURN * 2);
    assert!(immobile_at(&reg, ship(ALICE), now));
    assert!(no_jump_at(&reg, ship(ALICE), now));
    assert!(fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now + 1).is_err());
}

#[test]
fn a_reaper_refuses_a_lance_call_and_a_bad_slice() {
    let now = crate::pyutil::filetime_now();
    // A Lance is not a slice, and a Reaper is not a cylinder.
    let mut reg = world();
    assert!(fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now).is_err());
    let mut reg = world_with(REAPER, HULL);
    assert!(fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).is_err());
    // Both points on one line; and an arc far wider than the range allows.
    assert!(
        fire_slash(
            &mut reg,
            Some(ALICE),
            ALICE_LANCE,
            P0,
            [P0[0] * 2.0, P0[1] * 2.0, 0.0],
            now
        )
        .is_err()
    );
    assert!(
        fire_slash(
            &mut reg,
            Some(ALICE),
            ALICE_LANCE,
            [90_000.0, 1.0, 0.0],
            [90_000.0, -1.0, 0.0],
            now
        )
        .is_err()
    );
    assert_eq!(fuel_left(&reg), BURN * 3);
}

#[test]
fn the_fx_carries_the_fields_the_client_reads() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world_with(REAPER, HULL);
    fire_slash(&mut reg, Some(ALICE), ALICE_LANCE, P0, P1, now).expect("it fires");
    assert!(
        !pushed(&mut reg).iter().any(|(n, _)| n == "OnSpecialFX"),
        "the beam is drawn when the warning ends"
    );
    run_to(&mut reg, now + WARN_MS * MSEC);
    let fx: Vec<_> = pushed(&mut reg)
        .into_iter()
        .filter(|(n, _)| n == "OnSpecialFX")
        .collect();
    assert_eq!(fx.len(), 1, "one start, to the firer");
    let args = &fx[0].1;
    assert_eq!(args.len(), 14, "all fourteen positionals");
    assert_eq!(args[0], PyValue::Int(ship(ALICE)));
    assert_eq!(args[1], PyValue::Int(ALICE_LANCE));
    assert_eq!(args[7], PyValue::Int(1), "start");
    let point = |p: [f64; 3]| PyValue::Tuple(p.iter().map(|x| PyValue::Real(*x)).collect());
    assert_eq!(attr_of(&args[13], "startTargetOffset"), Some(point(P0)));
    assert_eq!(attr_of(&args[13], "endTargetOffset"), Some(point(P1)));
    assert_eq!(attr_of(&args[13], "duration"), Some(PyValue::Int(3_000)));
    // The stop follows the last tick.
    run_to(&mut reg, now + (WARN_MS + 10_000) * MSEC);
    let stop: Vec<_> = pushed(&mut reg)
        .into_iter()
        .filter(|(n, _)| n == "OnSpecialFX")
        .collect();
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0].1[7], PyValue::Int(0), "stop");
}

#[test]
fn a_lance_fx_carries_its_aim_offset() {
    let now = crate::pyutil::filetime_now();
    let mut reg = world();
    fire_lance(&mut reg, Some(ALICE), ALICE_LANCE, AIM, now).expect("it fires");
    run_to(&mut reg, now + WARN_MS * MSEC);
    let fx: Vec<_> = pushed(&mut reg)
        .into_iter()
        .filter(|(n, _)| n == "OnSpecialFX")
        .collect();
    assert_eq!(fx.len(), 1);
    let aim = PyValue::Tuple(AIM.iter().map(|x| PyValue::Real(*x)).collect());
    assert_eq!(attr_of(&fx[0].1[13], "targetOffset"), Some(aim));
    assert_eq!(attr_of(&fx[0].1[13], "duration"), Some(PyValue::Int(3_000)));
}

/// The doomsday FX guid follows the module's one damage type; the bare
/// guid the client has no row for is never sent (fci_doomsday).
#[test]
fn the_beam_guid_follows_the_damage_type() {
    let d = |em, kinetic, thermal, explosive| Damage {
        em,
        explosive,
        kinetic,
        thermal,
    };
    assert_eq!(race_index(&d(7.0, 0.0, 0.0, 0.0)), 0);
    assert_eq!(race_index(&d(0.0, 7.0, 0.0, 0.0)), 1);
    assert_eq!(race_index(&d(0.0, 0.0, 7.0, 0.0)), 2);
    assert_eq!(race_index(&d(0.0, 0.0, 0.0, 7.0)), 3);
    for g in GUIDS_BEAM.iter().chain(GUIDS_SLASH.iter()) {
        assert!(
            g != &"effects.SWDamageBeam" && g != &"effects.SWSlashBeam",
            "{g}"
        );
    }
    assert!(GUIDS_SLASH[race_index(&d(0.0, 0.0, 0.0, 7.0))].ends_with("Minmatar"));
}
