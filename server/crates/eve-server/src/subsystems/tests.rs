//! T3-D: subsystems and the slots they give. **Invented ids and numbers**
//! throughout: only the attribute and effect ids are the client's (they are
//! the ids the code under test reads).

use super::*;
use crate::content::Content;
use crate::fitting::Refusal;
use crate::service::Registry;

pub(crate) const ACTOR: i64 = 90_000_001;
pub(crate) const HULL: i64 = 990_960_001;
pub(crate) const OTHER_HULL: i64 = 990_960_002;
/// The four subsystems of [`HULL`], in flag order 125..128, and a second one
/// for flag 125.
pub(crate) const SUBS: [i64; 4] = [990_960_011, 990_960_012, 990_960_013, 990_960_014];
pub(crate) const CORE_TWO: i64 = 990_960_015;
/// A subsystem for [`OTHER_HULL`], flag 125.
pub(crate) const OTHER_SUB: i64 = 990_960_021;
/// An invented high-power module and a launcher-like one (hardpoint).
pub(crate) const MODULE: i64 = 990_960_031;
pub(crate) const SUBSYSTEM_GROUP: i64 = 990_958;
/// An invented passive effect with a real modifier row: ship agility
/// post-percent from the subsystem's attribute 1523.
const AGILITY_EFFECT: i64 = 990_960_901;
const AGILITY_BONUS_ATTR: i64 = 1523;
const AGILITY: i64 = 70;
/// Each of the two bonus subsystems' invented `agility` bonus, percent.
const BONUS_PERCENT: f64 = -10.0;

/// What each of [`SUBS`] adds `[hi, med, low, turrets, launchers]`.
pub(crate) const GIVES: [[i64; 5]; 4] = [
    [0, 1, 1, 0, 0],
    [1, 0, 2, 0, 0],
    [4, 0, 0, 3, 1],
    [1, 2, 0, 0, 0],
];

fn sub_row(id: i64, flag: i64, fits: i64, gives: [i64; 5], extra: &str) -> String {
    format!(
        "type\t{id}\t{SUBSYSTEM_GROUP}\t32\t1\t1\t40.0\t0.0\t1200000.0\t1.0\t0\t3772,3774,3773{extra}\t\
         1366={flag}.0;1380={fits}.0;1374={}.0;1375={}.0;1376={}.0;1368={}.0;1369={}.0;{AGILITY_BONUS_ATTR}={BONUS_PERCENT}\t\
         Fixture Subsystem {id}\n",
        gives[0], gives[1], gives[2], gives[3], gives[4]
    )
}

pub(crate) fn content() -> Content {
    content_with(false)
}

/// [`content`], and with `locked` the last subsystem needs Gunnery 3, which
/// the starter sheet lacks (SKILLGATE).
pub(crate) fn content_with(locked: bool) -> Content {
    let mut text = String::from(crate::typedata::FIXTURE);
    text += "effect\t3772\t0\t0\t0\t0\t0\t0\t0\tsubSystem\t\n";
    text += "effect\t3773\t0\t0\t0\t0\t0\t0\t0\thardPointModifierEffect\t\n";
    text += "effect\t3774\t0\t0\t0\t0\t0\t0\t0\tslotModifier\t\n";
    text += &format!("effect\t{AGILITY_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tinventedAgility\t\n");
    text += &format!(
        "modifier\t{AGILITY_EFFECT}\tshipID\tItemModifier\t{AGILITY}\t{AGILITY_BONUS_ATTR}\t6\t\t\n"
    );
    text += &format!("attribute\t{AGILITY}\t1.0\tagility\n");
    // Penalised unless the source is a subsystem (or skill, ship ...).
    text += &format!("attrmeta\t{AGILITY}\t0\t0\t0\t0\t5\n");
    for h in [HULL, OTHER_HULL] {
        text += &format!(
            "type\t{h}\t{GROUP_STRATEGIC_CRUISER}\t6\t1\t1\t100000.0\t0.0\t12000000.0\t200.0\t0\t\t\
             9=2000.0;11=1000.0;12=0.0;13=0.0;14=0.0;48=500.0;70=100.0;101=0.0;102=0.0;1137=3.0\t\
             Fixture Modular Hull {h}\n"
        );
    }
    for (n, id) in SUBS.iter().enumerate() {
        let extra = if n < 2 {
            format!(",{AGILITY_EFFECT}")
        } else {
            String::new()
        };
        let mut row = sub_row(*id, FIRST_FLAG + n as i64, HULL, GIVES[n], &extra);
        if locked && n == 3 {
            row = row.replacen("\t1366=", "\t182=3300.0;277=3.0;1366=", 1);
        }
        text += &row;
    }
    text += &sub_row(CORE_TWO, FIRST_FLAG, HULL, [0, 0, 0, 0, 0], "");
    text += &sub_row(OTHER_SUB, FIRST_FLAG, OTHER_HULL, [1, 0, 0, 0, 0], "");
    text += &format!(
        "type\t{MODULE}\t55\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t12\t50=1.0\tFixture Module\n"
    );
    let table = crate::typedata::Table::parse(&text).expect("the invented rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

/// Alice, docked, with a packaged-and-assembled [`HULL`] as her active ship,
/// every type in [`SUBS`], [`CORE_TWO`], [`OTHER_SUB`] and `modules` modules
/// in the hangar. Returns the registry, the station and the hull's id.
pub(crate) fn world(modules: i64) -> (Registry, i64, i64) {
    world_with(modules, false)
}

pub(crate) fn world_with(modules: i64, locked: bool) -> (Registry, i64, i64) {
    let mut reg = Registry::new(crate::model::SandboxConfig::default())
        .with_content(std::sync::Arc::new(content_with(locked)));
    let station = reg
        .character(ACTOR)
        .and_then(|c| c.station_id)
        .expect("the fixture character is docked");
    let at = crate::item::Slot::new(ACTOR, station, crate::model::FLAG_HANGAR);
    let create = |type_id, quantity| Edit::Create {
        type_id,
        quantity,
        to: at,
    };
    let mut plan = vec![create(HULL, 1), create(OTHER_HULL, 1)];
    plan.extend(SUBS.iter().map(|t| create(*t, 1)));
    plan.push(create(CORE_TWO, 1));
    plan.push(create(OTHER_SUB, 1));
    if modules > 0 {
        plan.push(create(MODULE, modules));
    }
    reg.apply_items(&plan).expect("the invented items apply");
    let hull = id_of(&reg, HULL);
    let c = reg.character_mut(ACTOR).expect("Alice");
    c.ship_id = hull;
    c.ship_type_id = HULL;
    (reg, station, hull)
}

pub(crate) fn id_of(reg: &Registry, type_id: i64) -> i64 {
    reg.store()
        .items()
        .iter()
        .find(|i| i.type_id == type_id)
        .map(|i| i.item_id)
        .unwrap_or_else(|| panic!("no item of type {type_id}"))
}

/// Fit `type_id` the way `fit_one` does: plan, apply.
pub(crate) fn fit(reg: &mut Registry, hull: i64, type_id: i64) -> Result<Fit, Refusal> {
    let item = id_of(reg, type_id);
    let fit = crate::fitting::plan_fit_in(
        reg.content(),
        reg.store(),
        reg.character(ACTOR),
        hull,
        item,
        crate::fitting::FLAG_AUTOFIT,
    )?;
    reg.apply_items(&fit.plan).expect("a checked plan applies");
    Ok(fit)
}

fn fit_all(reg: &mut Registry, hull: i64) {
    for t in SUBS {
        fit(reg, hull, t).expect("a subsystem of the hull fits");
    }
}

fn sums() -> [i64; 5] {
    let mut s = [0; 5];
    for g in GIVES {
        for (i, v) in g.iter().enumerate() {
            s[i] += v;
        }
    }
    s
}

#[test]
fn a_bare_strategic_cruiser_has_no_slots() {
    let (mut reg, _, hull) = world(1);
    let h = hull_fit(reg.content(), reg.store(), hull, None).expect("the hull has a row");
    assert_eq!([h.hi, h.med, h.low, h.turrets, h.launchers], [0; 5]);
    assert_eq!(fitted_count(reg.store(), hull), 0);
    // Not even a module that would fit a normal hull: there is no slot.
    let got = fit(&mut reg, hull, MODULE);
    assert_eq!(got, Err(Refusal::NoSlot));
}

#[test]
fn four_subsystems_give_the_hull_their_slots_and_hardpoints() {
    let (mut reg, _, hull) = world(1);
    fit_all(&mut reg, hull);
    // Each went to its own flag, assembled.
    for (n, t) in SUBS.iter().enumerate() {
        let s = reg.store().get(id_of(&reg, *t)).expect("the subsystem");
        assert_eq!((s.location_id, s.flag_id), (hull, FIRST_FLAG + n as i64));
        assert!(s.singleton);
    }
    let want = sums();
    let h = hull_fit(reg.content(), reg.store(), hull, None).expect("the hull");
    assert_eq!([h.hi, h.med, h.low, h.turrets, h.launchers], want);
    // The sheet says the same: the two sides agree.
    let sheet = crate::dogma::sheet(&reg, hull).expect("a sheet");
    use crate::dogma::attr;
    let got = [
        sheet.ship(attr::HI_SLOTS),
        sheet.ship(attr::MED_SLOTS),
        sheet.ship(attr::LOW_SLOTS),
        sheet.ship(attr::TURRET_SLOTS),
        sheet.ship(attr::LAUNCHER_SLOTS),
    ];
    assert_eq!(got.map(|v| v as i64), want);
    // And the hull now takes a module in the first high slot.
    let fit = fit(&mut reg, hull, MODULE).expect("a high slot exists now");
    assert_eq!(fit.flag_id, crate::fitting::Rack::High.first_flag());
}

#[test]
fn slot_modifier_effect_has_no_modifier_rows_so_it_is_special_cased() {
    let c = content();
    let table = crate::typedata::installed_in(&c).table();
    assert!(table.modifiers(3774).is_empty(), "slotModifier has none");
    assert!(
        table.modifiers(3773).is_empty(),
        "hardPointModifier has none"
    );
    assert!(table.effect(3774).is_some() && table.effect(3773).is_some());
    // Without the special case the sheet could not know: with it, it does.
    let (mut reg, _, hull) = world(0);
    let before = crate::dogma::sheet(&reg, hull).expect("a sheet");
    assert_eq!(before.ship(crate::dogma::attr::HI_SLOTS), 0.0);
    fit_all(&mut reg, hull);
    let after = crate::dogma::sheet(&reg, hull).expect("a sheet");
    assert_eq!(
        after.ship(crate::dogma::attr::HI_SLOTS) as i64,
        sums()[0],
        "the cached sheet follows the fit"
    );
}

#[test]
fn a_subsystem_of_the_wrong_hull_is_refused() {
    let (mut reg, _, hull) = world(0);
    let got = fit(&mut reg, hull, OTHER_SUB);
    assert_eq!(
        got,
        Err(Refusal::SubSystemNotShip {
            type_id: OTHER_SUB,
            valid_ship_type_id: OTHER_HULL,
            ship_type_id: HULL,
        })
    );
    assert_eq!(
        reg.store().get(id_of(&reg, OTHER_SUB)).map(|i| i.flag_id),
        Some(4)
    );
    // The client's key and its three type arguments.
    let e = got.unwrap_err().to_error();
    let text = format!("{e:?}");
    for id in [OTHER_SUB, OTHER_HULL, HULL] {
        assert!(text.contains(&id.to_string()), "{id} in {text}");
    }
    let key = b"CannotFitSubSystemNotShip"
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(text.contains(&key), "the client's key in {text}");
}

#[test]
fn a_second_subsystem_in_a_flag_is_refused() {
    let (mut reg, _, hull) = world(0);
    fit(&mut reg, hull, SUBS[0]).expect("the first");
    assert_eq!(fit(&mut reg, hull, CORE_TWO), Err(Refusal::Occupied));
    // And a subsystem named into a flag that is not its own.
    let def = id_of(&reg, SUBS[1]);
    let into_core = crate::fitting::plan_fit_in(
        reg.content(),
        reg.store(),
        reg.character(ACTOR),
        hull,
        def,
        FIRST_FLAG,
    );
    assert_eq!(
        into_core,
        Err(Refusal::NoSubSystemSlot { type_id: SUBS[1] })
    );
}

/// The pilot's flight hull is the fitted one: the agility the dogma sheet
/// gives (two -10 % bonuses on 100), not the type table's.
#[test]
fn the_flight_hull_is_the_fitted_one() {
    let (mut reg, _, hull) = world(0);
    fit(&mut reg, hull, SUBS[0]).expect("the first bonus subsystem");
    fit(&mut reg, hull, SUBS[1]).expect("the second");
    crate::destiny::on_ballpark_bind(&mut reg, Some(ACTOR));
    let sheet = crate::dogma::sheet(&reg, hull).expect("a sheet");
    let flown = reg.flight_hull(ACTOR).expect("a hull");
    assert!(
        (f64::from(flown.agility) - 81.0).abs() < 1e-3,
        "{}",
        flown.agility
    );
    assert_eq!(
        f64::from(flown.agility),
        f64::from(sheet.ship(AGILITY) as f32)
    );
}

#[test]
fn subsystems_are_not_stacking_penalised() {
    let (mut reg, _, hull) = world(0);
    let agility = |reg: &Registry| {
        crate::dogma::sheet(reg, hull)
            .expect("a sheet")
            .ship(AGILITY)
    };
    assert_eq!(agility(&reg), 100.0);
    fit(&mut reg, hull, SUBS[0]).expect("the first bonus subsystem");
    fit(&mut reg, hull, SUBS[1]).expect("the second");
    // Two -10 % bonuses on a non-stackable attribute: the plain product, not
    // the penalised one (the second would have counted for about 87 %).
    let factor = 1.0 + BONUS_PERCENT / 100.0;
    let want = 100.0 * factor * factor;
    assert!(
        (agility(&reg) - want).abs() < 1e-9,
        "{} vs {want}",
        agility(&reg)
    );
}

#[test]
fn unfitting_in_space_is_refused() {
    let (mut reg, _, hull) = world(0);
    fit_all(&mut reg, hull);
    let sub = id_of(&reg, SUBS[2]);
    crate::destiny::on_ballpark_bind(&mut reg, Some(ACTOR));
    let c = reg.character(ACTOR).cloned();
    let got = crate::fitting::plan_unfit(&reg, c.as_ref(), sub, None);
    assert_eq!(got, Err(Refusal::NotDocked));
}

#[test]
fn a_subsystem_that_slots_depend_on_stays_while_modules_need_them() {
    let (mut reg, _, hull) = world(1);
    fit_all(&mut reg, hull);
    fit(&mut reg, hull, MODULE).expect("a module in a high slot");
    let c = reg.character(ACTOR).cloned();
    // Subsystem 2 gives 4 of the 6 high slots; with it out two remain, and
    // one module is fitted: allowed. Subsystem 0..: still fine. Fill the
    // hull's high slots to the brim and the same unfit is refused.
    let offence = id_of(&reg, SUBS[2]);
    assert!(crate::fitting::plan_unfit(&reg, c.as_ref(), offence, None).is_ok());
    let total = hull_fit(reg.content(), reg.store(), hull, None)
        .expect("hull")
        .hi;
    let mut more = total - 1;
    while more > 0 {
        let item = Edit::Create {
            type_id: MODULE,
            quantity: 1,
            to: crate::item::Slot::new(
                ACTOR,
                hull,
                crate::fitting::Rack::High.first_flag() + total - more,
            ),
        };
        reg.apply_items(&[item]).expect("a module in a slot");
        more -= 1;
    }
    let got = crate::fitting::plan_unfit(&reg, c.as_ref(), offence, None);
    assert!(matches!(got, Err(Refusal::Unsupported(_))), "{got:?}");
}

#[test]
fn undock_is_allowed_only_with_four() {
    let (mut reg, _, hull) = world(0);
    assert!(!may_undock(reg.store(), hull));
    for t in &SUBS[..3] {
        fit(&mut reg, hull, *t).expect("fits");
        assert!(!may_undock(reg.store(), hull));
    }
    fit(&mut reg, hull, SUBS[3]).expect("fits");
    assert!(may_undock(reg.store(), hull));
}
