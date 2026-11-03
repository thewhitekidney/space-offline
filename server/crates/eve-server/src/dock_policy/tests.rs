//! CAP-K tests. Every id is invented: hulls 990_000_101.., systems of
//! `Universe::fixture` (39_000_001 high, 39_000_002 low, 39_000_003 null).
//! The group numbers are the client's `docking.GROUP_MAPPING` keys, which
//! the rule is about, not extracted rows.

use super::*;
use crate::docking::UndockCategory::*;
use crate::model::SandboxConfig;
use crate::service::Request;

const HIGH: Option<SecurityClass> = Some(SecurityClass::High);
const LOW: Option<SecurityClass> = Some(SecurityClass::Low);
const NULL: Option<SecurityClass> = Some(SecurityClass::Zero);

#[test]
fn small_and_large_hulls_dock_everywhere() {
    for class in [HIGH, LOW, NULL, None] {
        for cat in [Default, Small, Large] {
            assert!(may_dock(cat, 25, class, true));
            assert!(may_dock(cat, 25, class, false));
        }
    }
}

#[test]
fn a_capital_docks_only_in_null_with_the_switch_on() {
    assert!(may_dock(Capital, 485, NULL, true));
    assert!(!may_dock(Capital, 485, NULL, false));
    assert!(!may_dock(Capital, 485, HIGH, true));
    assert!(!may_dock(Capital, 485, LOW, true));
    assert!(!may_dock(Capital, 485, None, true));
}

#[test]
fn a_supercapital_never_docks() {
    for class in [HIGH, LOW, NULL, None] {
        assert!(!may_dock(Supercapital, 30, class, true));
    }
}

#[test]
fn the_capital_industrial_group_docks_anywhere() {
    for class in [HIGH, LOW, NULL, None] {
        assert!(may_dock(Capital, CAPITAL_INDUSTRIAL_GROUP, class, false));
    }
}

#[test]
fn the_refusal_is_the_clients_own_reason_tuple() {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = too_large() else {
        panic!("not a raise")
    };
    let PyValue::Tuple(parts) = *header else {
        panic!("header")
    };
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("args")
    };
    assert_eq!(crate::pyutil::as_bytes(&args[0]), Some(DENIED.as_bytes()));
    let PyValue::Dict(d) = &args[1] else {
        panic!("dict")
    };
    let reason = crate::pyutil::pairs_get(d, b"reason").expect("reason");
    assert_eq!(
        reason,
        &PyValue::Tuple(vec![
            PyValue::Int(UE_LOC),
            PyValue::Bytes(TOO_LARGE_LABEL.as_bytes().to_vec())
        ])
    );
}

#[test]
fn the_slash_flips_and_reports_the_switch() {
    let mut reg = Registry::new(SandboxConfig::default());
    assert!(reg.ext::<Policy>().capitals_in_null);
    slash(&mut reg, None, &["off"]).expect("off");
    assert!(!reg.ext::<Policy>().capitals_in_null);
    slash(&mut reg, None, &[]).expect("report");
    assert!(!reg.ext::<Policy>().capitals_in_null);
    slash(&mut reg, None, &["on"]).expect("on");
    assert!(reg.ext::<Policy>().capitals_in_null);
    slash(&mut reg, None, &["maybe"]).expect("usage");
    assert!(reg.ext::<Policy>().capitals_in_null);
}

// ---- over the dispatch seam, as `CmdDock` arrives ----------------------

const PILOT: i64 = 1_000_501;
const SHIP: i64 = 100_000_501;
const STATION: i64 = 69_000_501;
const DREAD: i64 = 990_000_101;
const FREIGHTER: i64 = 990_000_102;
const RORQUAL: i64 = 990_000_103;
const TITAN: i64 = 990_000_104;
const HIGH_SYSTEM: i64 = 39_000_001;
const LOW_SYSTEM: i64 = 39_000_002;
const NULL_SYSTEM: i64 = 39_000_003;

fn content() -> std::sync::Arc<crate::content::Content> {
    let hull = |id: i64, group: i64, name: &str| {
        format!(
            "type\t{id}\t{group}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t9=310.0;37=355.0\t{name}\n"
        )
    };
    let text = format!(
        "{}{}{}{}{}",
        crate::typedata::FIXTURE,
        hull(DREAD, 485, "Fixture Dread"),
        hull(FREIGHTER, 513, "Fixture Freighter"),
        hull(RORQUAL, CAPITAL_INDUSTRIAL_GROUP, "Fixture Rorqual"),
        hull(TITAN, 30, "Fixture Titan"),
    );
    let mut c = crate::content::Content::fixture();
    c.insert(
        crate::typedata::TABLE.name,
        crate::typedata::Installed::build(crate::typedata::Table::parse(&text).expect("parses")),
    );
    std::sync::Arc::new(c)
}

fn world(hull: i64, system: i64) -> Registry {
    let pilot = crate::model::Character {
        character_id: PILOT,
        name: "Pilot".to_owned(),
        ship_id: SHIP,
        ship_type_id: hull,
        station_id: Some(STATION),
        solar_system_id: Some(system),
        ..crate::model::Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![pilot],
        ..SandboxConfig::default()
    })
    .with_content(content());
    crate::destiny::on_ballpark_bind(&mut reg, Some(PILOT));
    let _ = reg.take_pending();
    reg
}

fn dock(reg: &mut Registry) -> Result<PyValue, ServiceError> {
    reg.dispatch(&Request {
        service: "beyonce",
        method: "CmdDock",
        args: &[PyValue::Int(STATION), PyValue::Int(SHIP)],
        kwargs: &[],
        bind_params: None,
        actor: Some(PILOT),
    })
}

fn denied_too_large(r: &Result<PyValue, ServiceError>) -> bool {
    let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
        return false;
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        return false;
    };
    let PyValue::Tuple(args) = &parts[1] else {
        return false;
    };
    crate::pyutil::as_bytes(&args[0]) == Some(DENIED.as_bytes())
}

#[test]
fn a_dreadnought_is_refused_in_high_sec_and_stays_in_space() {
    let mut reg = world(DREAD, HIGH_SYSTEM);
    let r = dock(&mut reg);
    assert!(denied_too_large(&r), "{r:?}");
    assert!(reg.roster().is_in_space(PILOT), "still in space");
}

#[test]
fn a_dreadnought_is_refused_in_low_sec() {
    let mut reg = world(DREAD, LOW_SYSTEM);
    assert!(denied_too_large(&dock(&mut reg)));
}

#[test]
fn a_dreadnought_docks_in_null_sec_with_the_switch_on_and_not_off() {
    let mut reg = world(DREAD, NULL_SYSTEM);
    slash(&mut reg, None, &["off"]).expect("off");
    assert!(denied_too_large(&dock(&mut reg)));
    slash(&mut reg, None, &["on"]).expect("on");
    assert!(matches!(dock(&mut reg), Ok(PyValue::None)));
}

#[test]
fn a_freighter_and_a_rorqual_dock_in_high_sec() {
    for hull in [FREIGHTER, RORQUAL] {
        let mut reg = world(hull, HIGH_SYSTEM);
        assert!(matches!(dock(&mut reg), Ok(PyValue::None)), "hull {hull}");
    }
}

#[test]
fn a_titan_is_refused_even_in_null_sec() {
    let mut reg = world(TITAN, NULL_SYSTEM);
    assert!(denied_too_large(&dock(&mut reg)));
}

// ---- the range ----------------------------------------------------------

/// A station the catalogue places, in the pilot's system.
const PLACED: &str = "# solarSystemID\t39000001\n\
#kind\tid\ttype_id\tradius\tx\ty\tz\n\
station\t69000501\t990000201\t0.0\t1000000.0\t0.0\t0.0\n";

/// A pilot in space `gap` metres off the station's surface.
fn world_at(gap: f64) -> Registry {
    let pilot = crate::model::Character {
        character_id: PILOT,
        name: "Pilot".to_owned(),
        ship_id: SHIP,
        ship_type_id: FREIGHTER,
        station_id: Some(STATION),
        solar_system_id: Some(HIGH_SYSTEM),
        ..crate::model::Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![pilot],
        celestials: crate::celestial::Catalogue::parse(PLACED),
        ..SandboxConfig::default()
    })
    .with_content(content());
    crate::destiny::on_ballpark_bind(&mut reg, Some(PILOT));
    let _ = reg.take_pending();
    let station = reg.ball_target(STATION).expect("a placed station");
    let ship = reg.ball_target(SHIP).expect("the ship in space");
    let x = station.position[0] - station.radius - ship.radius - gap;
    let p = reg.roster_mut().get_mut(PILOT).expect("in space");
    p.motion.position = [x, 0.0, 0.0];
    reg
}

fn not_dockable(r: &Result<PyValue, ServiceError>) -> bool {
    let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
        return false;
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        return false;
    };
    let PyValue::Tuple(args) = &parts[1] else {
        return false;
    };
    crate::pyutil::as_bytes(&args[0]) == Some(crate::service::space::NOT_DOCKABLE.as_bytes())
}

#[test]
fn the_surface_distance_is_centre_to_centre_less_both_radii() {
    let reg = world_at(1_234.0);
    let d = station_surface_distance(&reg, PILOT, STATION).expect("a distance");
    assert!((d - 1_234.0).abs() < 1e-6, "{d}");
}

#[test]
fn inside_the_client_s_range_a_pilot_docks() {
    let mut reg = world_at(DOCK_RANGE - 100.0);
    assert!(matches!(dock(&mut reg), Ok(PyValue::None)));
}

#[test]
fn the_lag_allowance_is_still_a_dock() {
    let mut reg = world_at(DOCK_RANGE + DOCK_RANGE_SLACK - 1.0);
    assert!(matches!(dock(&mut reg), Ok(PyValue::None)));
}

#[test]
fn from_across_the_system_a_dock_is_refused_and_the_ship_stays_out() {
    for gap in [DOCK_RANGE + DOCK_RANGE_SLACK + 1.0, 50_000.0, 900_000.0] {
        let mut reg = world_at(gap);
        let r = dock(&mut reg);
        assert!(not_dockable(&r), "gap {gap}: {r:?}");
        assert!(reg.roster().is_in_space(PILOT), "gap {gap}: still in space");
    }
}

#[test]
fn a_station_with_no_position_has_no_distance_and_no_refusal() {
    // The fixture world places no station: the origin would be a lie.
    let mut reg = world(FREIGHTER, HIGH_SYSTEM);
    assert_eq!(station_surface_distance(&reg, PILOT, STATION), None);
    assert!(matches!(dock(&mut reg), Ok(PyValue::None)));
}
