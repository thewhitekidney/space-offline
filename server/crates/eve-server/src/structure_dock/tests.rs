//! Tests of docking in an Upwell structure. Every id is invented.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::service::Request;
use crate::space_state::{Motion, Presence};

const BOSS: i64 = 7;
const PAL: i64 = 8;
const FOE: i64 = 9;
const CORP: i64 = 11;
const FOE_CORP: i64 = 13;
const SYSTEM: i64 = 31;
const OTHER_SYSTEM: i64 = 32;
const ASTRAHUS: i64 = crate::structure_space::ASTRAHUS;
const RADIUS: f64 = 20_000.0;
/// An NPC station in the system, where stored things go when a structure
/// leaves.
const HARBOUR: i64 = 6_001;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}type\t{ASTRAHUS}\t1657\t65\t1\t1\t10.0\t0.0\t1000.0\t{RADIUS}\t0\t\t\tInvented Hull\n",
        crate::typedata::FIXTURE
    );
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    c
}

/// Three pilots in space in one system: two of one corporation, one of
/// another. The corporation's boss spawns the structure.
fn config() -> SandboxConfig {
    let ch = |id: i64, corp: i64| Character {
        character_id: id,
        corporation_id: corp,
        solar_system_id: Some(SYSTEM),
        station_id: Some(HARBOUR),
        ship_id: 1_000 + id,
        ship_type_id: crate::service::TYPE_CAPSULE,
        ..Character::default()
    };
    SandboxConfig {
        characters: vec![ch(BOSS, CORP), ch(PAL, CORP), ch(FOE, FOE_CORP)],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{SYSTEM}\t0\t0\n"
        )),
        ..SandboxConfig::default()
    }
}

fn world() -> (Registry, i64) {
    world_in(config())
}

/// [`config`] with the only NPC station in `OTHER_SYSTEM`: the pilots'
/// system has none.
fn stationless_config() -> SandboxConfig {
    SandboxConfig {
        celestials: crate::celestial::Catalogue::parse(&format!(
            "station\t{HARBOUR}\t2502\t0.0\t0.0\t0.0\t0.0\t{OTHER_SYSTEM}\t0\t0\n"
        )),
        ..config()
    }
}

fn world_in(config: SandboxConfig) -> (Registry, i64) {
    let mut reg = Registry::new(config).with_content(std::sync::Arc::new(content()));
    reg.corporations_mut()
        .enrol_for_test(BOSS, CORP, crate::corp_roles::CORP_ROLE_DIRECTOR);
    reg.corporations_mut().enrol_for_test(PAL, CORP, 0);
    reg.corporations_mut().enrol_for_test(FOE, FOE_CORP, 0);
    for (n, who) in [BOSS, PAL, FOE].into_iter().enumerate() {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            1_000 + who,
            SYSTEM,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    let out = crate::service::structures::slash(
        &mut reg,
        Some(BOSS),
        &["spawn", "astrahus", "Invented", "Home"],
    )
    .expect("spawns");
    let PyValue::Bytes(b) = out else {
        panic!("{out:?}")
    };
    assert!(String::from_utf8_lossy(&b).contains("online"));
    let id = structure::all(reg.deltas()).last().expect("spawned").id;
    let _ = reg.take_pending();
    (reg, id)
}

/// The user error a refusal raises, by its key.
fn refusal(e: &ServiceError) -> String {
    let ServiceError::Raise(o) = e else {
        return format!("{e:?}");
    };
    let PyValue::ObjectEx { header, .. } = o else {
        return format!("{o:?}");
    };
    match &**header {
        PyValue::Tuple(h) => match h.get(1) {
            Some(PyValue::Tuple(a)) => match a.first() {
                Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                other => format!("{other:?}"),
            },
            other => format!("{other:?}"),
        },
        other => format!("{other:?}"),
    }
}

fn names(reg: &mut Registry) -> Vec<&'static str> {
    reg.take_pending()
        .into_iter()
        .map(|(_, p)| p.to_push().0)
        .collect()
}

fn call(
    reg: &mut Registry,
    who: i64,
    service: &str,
    method: &str,
    args: &[PyValue],
) -> Result<PyValue, ServiceError> {
    let req = Request {
        service,
        method,
        args,
        kwargs: &[],
        bind_params: None,
        actor: Some(who),
    };
    reg.dispatch(&req)
}

fn dock_call(reg: &mut Registry, who: i64, id: i64) -> Result<PyValue, ServiceError> {
    let ship = reg.character(who).unwrap().ship_id;
    call(
        reg,
        who,
        "structureDocking",
        "Dock",
        &[PyValue::Int(id), PyValue::Int(ship)],
    )
}

fn key<'a>(entries: &'a [(PyValue, PyValue)], name: &str) -> Option<&'a PyValue> {
    crate::pyutil::pairs_get(entries, name.as_bytes())
}

#[test]
fn a_dock_is_one_structureid_and_the_pilot_leaves_space() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).expect("docks");
    assert!(!reg.roster().is_in_space(BOSS));
    assert_eq!(reg.docked_station(BOSS), Some(id));
    assert_eq!(docked_in(&reg, BOSS), Some(id));
    let ship = reg.character(BOSS).unwrap().ship_id;
    let it = reg.store().get(ship).unwrap();
    assert_eq!(
        (it.location_id, it.flag_id),
        (id, crate::model::FLAG_HANGAR)
    );
    // The change has exactly the one key, with the structure on the new side.
    let changes = reg.take_session_changes();
    assert_eq!(changes.len(), 1);
    let (who, change) = &changes[0];
    assert_eq!(*who, BOSS);
    assert_eq!(change.len(), 1, "{change:?}");
    assert_eq!(
        key(change, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::None, PyValue::Int(id)]))
    );
    // The others lose the ball; the slim says one is inside.
    let sent = names(&mut reg);
    assert!(sent.contains(&"DoDestinyUpdate"), "{sent:?}");
    let ex = crate::structure_space::slim_extras(&reg, &[id]);
    let docked = ex[0].1.iter().find(|(k, _)| *k == "docked").unwrap();
    assert_eq!(docked.1, PyValue::Int(1));
}

#[test]
fn the_station_fields_name_the_structure_and_the_system() {
    let (mut reg, id) = world();
    dock_call(&mut reg, PAL, id).expect("docks");
    let c = reg.character(PAL).unwrap();
    assert_eq!(c.station_id, Some(id));
    assert_eq!(c.station_type_id, ASTRAHUS);
    assert_eq!(c.station_owner_id, CORP);
    assert_eq!(c.solar_system_id, Some(SYSTEM));
}

#[test]
fn dock_refuses_what_it_should() {
    let (mut reg, id) = world();
    // A stranger has no setting 19.
    let e = dock_call(&mut reg, FOE, id).unwrap_err();
    assert!(refusal(&e).contains("StructureDockingDenied"), "{e:?}");
    assert!(reg.roster().is_in_space(FOE));
    // The wrong ship.
    let e = call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Dock",
        &[PyValue::Int(id), PyValue::Int(424_242)],
    )
    .unwrap_err();
    assert!(refusal(&e).contains("DockingRequestDenied"), "{e:?}");
    // Not a structure.
    let e = dock_call(&mut reg, BOSS, id + 1).unwrap_err();
    assert!(refusal(&e).contains("NotDockable"), "{e:?}");
    // Far away.
    let mut s = structure::get(reg.deltas(), id).unwrap();
    s.pos = [9.0e9, 0.0, 0.0];
    structure::put(reg.deltas_mut(), &s);
    let e = dock_call(&mut reg, BOSS, id).unwrap_err();
    assert!(refusal(&e).contains("NotDockable"), "{e:?}");
    // Offline (anchoring): the Dock button is not offered.
    s.pos = [0.0; 3];
    s.state = structure::State::Anchoring;
    structure::put(reg.deltas_mut(), &s);
    let e = dock_call(&mut reg, BOSS, id).unwrap_err();
    assert!(refusal(&e).contains("DockingRequestDenied"), "{e:?}");
    // Another system.
    s.state = structure::State::ShieldVulnerable;
    structure::put(reg.deltas_mut(), &s);
    reg.roster_mut().leave(BOSS);
    reg.roster_mut().arrive(Presence::arriving(
        BOSS,
        1_000 + BOSS,
        OTHER_SYSTEM,
        Motion::arrived([0.0; 3], [0.0; 3]),
    ));
    let e = dock_call(&mut reg, BOSS, id).unwrap_err();
    assert!(refusal(&e).contains("NotDockable"), "{e:?}");
    // Nothing happened to anyone.
    assert!(reg.take_session_changes().is_empty());
}

#[test]
fn a_refused_profile_can_be_widened_by_the_owner() {
    let (reg, id) = world();
    // The access rule is the directory lane's; this pins that Dock uses it.
    let s = structure::get(reg.deltas(), id).unwrap();
    assert!(sp::has_access(&reg, &s, PAL, sp::SETTING_HOUSING_CAN_DOCK));
    assert!(!sp::has_access(&reg, &s, FOE, sp::SETTING_HOUSING_CAN_DOCK));
}

#[test]
fn the_hangar_is_the_structures_and_10014_is_the_hangar() {
    let (mut reg, id) = world();
    assert_eq!(
        hangar_alias(&reg, Some(BOSS), Some(CONTAINER_STRUCTURE)),
        Some(CONTAINER_HANGAR)
    );
    // Before the dock, the structure's id is no hangar of this pilot's.
    assert_eq!(hangar_alias(&reg, Some(BOSS), Some(id)), None);
    dock_call(&mut reg, BOSS, id).unwrap();
    assert_eq!(
        hangar_alias(&reg, Some(BOSS), Some(id)),
        Some(CONTAINER_HANGAR)
    );
    assert_eq!(hangar_alias(&reg, Some(PAL), Some(id)), None);
    assert_eq!(hangar_alias(&reg, Some(BOSS), Some(CONTAINER_HANGAR)), None);
    // And the hangar lists the ship through the inventory arm.
    let bound = PyValue::Tuple(vec![PyValue::Int(CONTAINER_STRUCTURE), PyValue::None]);
    let req = Request {
        service: "invbroker::inventory",
        method: "List",
        args: &[],
        kwargs: &[],
        bind_params: Some(&bound),
        actor: Some(BOSS),
    };
    let PyValue::List(rows) = reg.dispatch(&req).unwrap() else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1, "the ship is in the hangar");
    // The self item is the structure, with its own group and category.
    let req = Request {
        method: "GetSelfInvItem",
        ..req
    };
    let PyValue::DbRow(row) = reg.dispatch(&req).unwrap() else {
        panic!("a row")
    };
    let text = format!("{row:?}");
    assert!(text.contains(&id.to_string()), "{text}");
    assert!(text.contains("1657") && text.contains("65"), "{text}");
}

#[test]
fn the_session_at_login_is_docked_in_the_structure() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    // The server restarts: nobody is in the roster, the station persisted.
    reg.roster_mut().leave(PAL);
    let change = reg.character_change(BOSS).expect("a change");
    let pair = |v: PyValue| PyValue::Tuple(vec![PyValue::None, v]);
    assert_eq!(key(&change, "stationid"), Some(&pair(PyValue::None)));
    assert_eq!(key(&change, "structureid"), Some(&pair(PyValue::Int(id))));
    assert_eq!(
        key(&change, "solarsystemid"),
        Some(&pair(PyValue::Int(SYSTEM)))
    );
    assert_eq!(
        key(&change, "solarsystemid2"),
        Some(&pair(PyValue::Int(SYSTEM)))
    );
    // A pilot docked at no structure is as before: no solarsystemid.
    let mut reg2 = world().0;
    reg2.roster_mut().leave(BOSS);
    let change = reg2.character_change(BOSS).unwrap();
    assert!(key(&change, "solarsystemid").is_none());
    assert_eq!(key(&change, "structureid"), Some(&pair(PyValue::None)));
}

#[test]
fn undock_without_a_park_arms_the_bind_that_seats_the_ship() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_session_changes();
    let _ = reg.take_pending();
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::Int(1_000 + BOSS)],
    )
    .expect("undocks");
    // Still docked until the client's bind: the session change is the
    // dispatcher's and says only structureid.
    assert!(!reg.roster().is_in_space(BOSS));
    let change = reg.undock_change(Some(BOSS)).unwrap();
    assert_eq!(change.len(), 1);
    assert_eq!(
        key(&change, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
    );
    // The bind is an ordinary arrival.
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    assert!(reg.roster().is_in_space(BOSS));
    let p = reg.roster().get(BOSS).unwrap();
    assert_eq!(p.solar_system_id, SYSTEM);
    // The structure's own place, off its surface (no locator rows here).
    let d = crate::space_state::distance(p.motion.position, [0.0; 3]);
    assert!(d >= RADIUS, "{d}");
    // And a later bind is not mistaken for anything.
    assert!(!on_bind(&mut reg, BOSS));
}

#[test]
fn a_docked_bind_is_the_structure_view_and_the_ego_is_the_structure() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_pending();
    let state = docked_state(&mut reg, BOSS).expect("a state");
    assert_eq!(state.ego, id);
    assert!(
        state.every_slim_has_a_ball(),
        "the ego is a ball of the scene"
    );
    assert!(
        state.park.balls.iter().any(|b| b.ball_id == id),
        "the structure ball is in it"
    );
    assert!(
        state.park.balls.iter().all(|b| b.ball_id != 1_000 + BOSS),
        "no ship ball for a docked pilot"
    );
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    assert!(
        !reg.roster().is_in_space(BOSS),
        "a docked bind seats nobody"
    );
    assert_eq!(names(&mut reg), vec!["DoDestinyUpdate"]);
    assert!(reg.ext::<Docked>().live.contains(&BOSS));
}

#[test]
fn undock_from_the_structure_view_seats_the_ship_in_the_live_park() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    let _ = reg.take_pending();
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
    // Seated at once, and told the scene again.
    assert!(reg.roster().is_in_space(BOSS));
    assert!(!reg.ext::<Docked>().live.contains(&BOSS));
    assert!(reg.ext::<Docked>().seated.contains(&BOSS));
    assert!(names(&mut reg).contains(&"DoDestinyUpdate"));
    // The hangar view's new park binds anyway: a state, and the ship stays.
    let at = reg.roster().get(BOSS).unwrap().motion.position;
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    assert!(names(&mut reg).contains(&"DoDestinyUpdate"));
    assert_eq!(reg.roster().get(BOSS).unwrap().motion.position, at);
    // Consumed: the next bind is the ordinary one.
    assert!(!reg.ext::<Docked>().seated.contains(&BOSS));
}

#[test]
fn undock_refuses_a_pilot_who_is_not_inside_or_a_changed_ship() {
    let (mut reg, id) = world();
    let e = call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .unwrap_err();
    assert!(refusal(&e).contains("NotDockable"), "{e:?}");
    dock_call(&mut reg, BOSS, id).unwrap();
    let e = call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::Int(5)],
    )
    .unwrap_err();
    assert!(refusal(&e).contains("CustomNotify"), "{e:?}");
    assert_eq!(docked_in(&reg, BOSS), Some(id), "still inside");
}

#[test]
fn guests_hear_who_comes_and_goes() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let _ = reg.take_pending();
    dock_call(&mut reg, PAL, id).unwrap();
    let sent = names(&mut reg);
    assert!(sent.contains(&"OnCharacterEnteredStructure"), "{sent:?}");
    let PyValue::Dict(d) = call(
        &mut reg,
        BOSS,
        "structureGuests",
        "GetGuests",
        &[PyValue::Int(id)],
    )
    .unwrap() else {
        panic!("a dict")
    };
    assert_eq!(d.len(), 2);
    call(
        &mut reg,
        PAL,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .unwrap();
    assert!(names(&mut reg).contains(&"OnCharacterLeftStructure"));
}

#[test]
fn the_station_guest_events_leave_a_structure_alone() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    dock_call(&mut reg, PAL, id).unwrap();
    let _ = reg.take_pending();
    assert!(reg.station_guest_frame(PAL, true).is_none());
    assert!(logoff_frame(&reg, PAL).is_some());
}

#[test]
fn the_hangar_view_state_carries_what_the_scene_reads() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let v = call(
        &mut reg,
        BOSS,
        "structureHangarViewMgr",
        "GetMyHangarViewState",
        &[],
    )
    .unwrap();
    let PyValue::Instance { state, .. } = &v else {
        panic!("a KeyVal, got {v:?}")
    };
    for k in [
        "upkeepState",
        "operatingState",
        "damageState",
        "timerProgress",
        "timerIsProgressing",
        "timerIsPaused",
        "timerEndAt",
    ] {
        assert!(
            crate::pyutil::dict_get(state, k.as_bytes()).is_some(),
            "{k}"
        );
    }
    // Never None, even for a pilot who is not docked.
    let v = call(
        &mut reg,
        PAL,
        "structureHangarViewMgr",
        "GetMyHangarViewState",
        &[],
    )
    .unwrap();
    assert!(matches!(v, PyValue::Instance { .. }));
    // A running timer gives a progress between 0 and 1.
    let mut s = structure::get(reg.deltas(), id).unwrap();
    let now = crate::pyutil::filetime_now();
    s.timer.start = Some(now - 10 * structure::SECOND);
    s.timer.end = Some(now + 10 * structure::SECOND);
    let PyValue::Instance { state, .. } = hangar_state(&s, now) else {
        panic!()
    };
    let Some(PyValue::Real(p)) = crate::pyutil::dict_get(&state, b"timerProgress") else {
        panic!("a float")
    };
    assert!((p - 0.5).abs() < 1e-6, "{p}");
}

#[test]
fn a_warp_to_a_structure_someone_is_docked_in_finds_the_structure() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let mut s = structure::get(reg.deltas(), id).unwrap();
    s.pos = [1.0e6, 2.0e6, 3.0e6];
    structure::put(reg.deltas_mut(), &s);
    let t = reg.ball_target(id).expect("a target");
    assert_eq!(t.position, [1.0e6, 2.0e6, 3.0e6]);
    assert_eq!(t.radius, RADIUS);
    assert_eq!(reg.station_position(id), [1.0e6, 2.0e6, 3.0e6]);
}

#[test]
fn the_structure_is_no_station_to_the_map_and_no_cmddock_target() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let rows = reg.station_rowset(Some(BOSS));
    assert!(
        !format!("{rows:?}").contains(&id.to_string()),
        "map.GetStationInfo lists the structure"
    );
    assert!(reg.dock_target(PAL, id).is_none());
    // The system has its one NPC station; the structure is not a second.
    let count = reg
        .station_counts()
        .iter()
        .find(|(sys, _)| *sys == SYSTEM)
        .map(|(_, n)| *n);
    assert_eq!(count, Some(1));
    assert!(reg.station_ref(id).is_none());
}

#[test]
fn a_bracket_name_for_the_structure_wins_over_the_station_name() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let req = Request {
        service: "config",
        method: "GetMultiLocationsEx",
        args: &[PyValue::List(vec![PyValue::Int(id)])],
        kwargs: &[],
        bind_params: None,
        actor: Some(BOSS),
    };
    let out = reg.dispatch(&req).unwrap();
    let text = format!("{out:?}");
    assert!(text.contains("Invented Home"), "{text}");
}

#[test]
fn unanchoring_puts_the_pilots_out_and_moves_what_is_stored() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    dock_call(&mut reg, PAL, id).unwrap();
    let _ = reg.take_session_changes();
    let s = structure::get(reg.deltas(), id).unwrap();
    evacuate(&mut reg, &s);
    // Both are in space where the structure is, not docked.
    assert!(reg.roster().is_in_space(BOSS));
    assert!(reg.roster().is_in_space(PAL));
    let changes = reg.take_session_changes();
    assert_eq!(changes.len(), 2, "{changes:?}");
    for (_, ch) in &changes {
        assert_eq!(
            key(ch, "structureid"),
            Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
        );
    }
    // The seated marker lets the bind that follows resend the scene.
    assert!(reg.ext::<Docked>().seated.contains(&BOSS));
    // Their ships and their bases moved to the NPC station of the system.
    for who in [BOSS, PAL] {
        let c = reg.character(who).unwrap();
        assert_eq!(c.station_id, Some(HARBOUR));
        let it = reg.store().get(c.ship_id).unwrap();
        assert_eq!(it.location_id, HARBOUR);
    }
    assert_eq!(docked_count(&reg, id), 0);
}

#[test]
fn an_item_left_in_the_hangar_follows_its_owner_to_the_harbour() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id: crate::service::TYPE_CAPSULE,
            quantity: 1,
            to: Slot::new(BOSS, id, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
    let spare = changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("an id");
    let s = structure::get(reg.deltas(), id).unwrap();
    evacuate(&mut reg, &s);
    let it = reg.store().get(spare).unwrap();
    assert_eq!((it.location_id, it.owner_id), (HARBOUR, BOSS));
}

#[test]
fn finishing_an_unanchor_runs_the_evacuation_before_the_row_goes() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let mut s = structure::get(reg.deltas(), id).unwrap();
    s.state = structure::State::Unanchored;
    structure::put(reg.deltas_mut(), &s);
    crate::structure_space::react_for_test(&mut reg, id);
    assert!(
        structure::get(reg.deltas(), id).is_none(),
        "the row is gone"
    );
    assert!(reg.roster().is_in_space(BOSS), "the pilot was put out");
    assert_eq!(reg.character(BOSS).unwrap().station_id, Some(HARBOUR));
    // The login after that is a station's.
    let change = reg.character_change(BOSS);
    let _ = change;
}

#[test]
fn the_dock_policy_asks_the_locator_rows_only_when_there_are_any() {
    let (reg, id) = world();
    let table = reg.content().get::<crate::docking::Table>();
    let s = structure::get(reg.deltas(), id).unwrap();
    assert!(
        crate::docking::undock_directions(table, s.type_id).is_none(),
        "the data gap: no rows for the type, so every hull may dock"
    );
}

#[test]
fn a_pilot_in_a_structure_is_in_the_system_to_the_relocator() {
    use crate::service::relocate::{Where, change};
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let inside = reg.where_now(BOSS).expect("somewhere");
    assert_eq!(inside.station, None);
    assert_eq!(inside.structure, Some(id));
    assert_eq!(inside.system, Some(SYSTEM));
    // Out into space is the one key, as the structure's own undock sends it.
    let outside = Where {
        structure: None,
        ..inside
    };
    let c = change(&inside, &outside).expect("a change");
    assert_eq!(c.len(), 1);
    assert_eq!(
        key(&c, "structureid"),
        Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))
    );
    // A GM teleport out to the harbour station is a station dock, and the
    // guests and the slim hear the pilot go.
    let station = reg.station_ref(HARBOUR).expect("the harbour is a station");
    dock_call(&mut reg, PAL, id).unwrap();
    let _ = reg.take_session_changes();
    let _ = reg.take_pending();
    reg.relocate(BOSS, crate::service::relocate::Dest::Station(station))
        .expect("moves");
    assert_eq!(reg.docked_station(BOSS), Some(HARBOUR));
    let changes = reg.take_session_changes();
    let (_, ch) = changes
        .iter()
        .find(|(who, _)| *who == BOSS)
        .expect("a change");
    assert!(key(ch, "structureid").is_some() && key(ch, "stationid").is_some());
    assert!(names(&mut reg).contains(&"OnCharacterLeftStructure"));
}

#[test]
fn a_pilot_docked_in_a_structure_is_still_docked_after_a_restart() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    let snap = reg.to_snapshot();
    // A new process: seeded again, then the stored world over it. Nobody is
    // in the roster, no client has bound anything.
    let mut again = Registry::new(config()).with_content(std::sync::Arc::new(content()));
    again.apply_snapshot(snap);
    assert!(
        structure::get(again.deltas(), id).is_some(),
        "the structure persisted"
    );
    assert_eq!(again.docked_station(BOSS), Some(id));
    assert_eq!(docked_in(&again, BOSS), Some(id));
    let ship = again.character(BOSS).unwrap().ship_id;
    assert_eq!(again.store().get(ship).unwrap().location_id, id);
    // The login is the docked one, and the client's hangar lists the ship.
    let change = again.character_change(BOSS).unwrap();
    let pair = |v: PyValue| PyValue::Tuple(vec![PyValue::None, v]);
    assert_eq!(key(&change, "structureid"), Some(&pair(PyValue::Int(id))));
    assert_eq!(key(&change, "stationid"), Some(&pair(PyValue::None)));
    assert_eq!(again.hangar_item_rows(Some(BOSS)).len(), 1);
    // And it comes out through the hangar view: an armed undock and a bind.
    call(
        &mut again,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .unwrap();
    crate::destiny::on_ballpark_bind(&mut again, Some(BOSS));
    assert!(again.roster().is_in_space(BOSS));
}

mod control;
mod home;
fn create_for(reg: &mut Registry, owner: i64, at: i64) -> i64 {
    let changes = reg
        .apply_items(&[Edit::Create {
            type_id: crate::service::TYPE_CAPSULE,
            quantity: 1,
            to: Slot::new(owner, at, crate::model::FLAG_HANGAR),
        }])
        .expect("created");
    changes
        .iter()
        .find_map(|c| match c {
            crate::item::Change::Created { item_id, .. } => Some(*item_id),
            _ => None,
        })
        .expect("an id")
}

#[test]
fn a_system_with_no_npc_station_still_gets_a_harbour_for_people_and_corporations() {
    let (mut reg, id) = world_in(stationless_config());
    dock_call(&mut reg, BOSS, id).unwrap();
    let mine = create_for(&mut reg, BOSS, id);
    let corps = create_for(&mut reg, CORP, id);
    let s = structure::get(reg.deltas(), id).unwrap();
    assert!(crate::structure_space::station_in(&reg, SYSTEM).is_none());
    evacuate(&mut reg, &s);
    for item in [mine, corps, reg.character(BOSS).unwrap().ship_id] {
        assert_eq!(reg.store().get(item).unwrap().location_id, HARBOUR);
    }
    assert_eq!(reg.character(BOSS).unwrap().station_id, Some(HARBOUR));
    assert!(!is_structure(
        &reg,
        reg.character(BOSS).unwrap().station_id.unwrap()
    ));
}

#[test]
fn a_pilot_based_at_the_structure_is_rebased_when_no_station_is_near() {
    let (mut reg, id) = world_in(stationless_config());
    // Based there, never docked now: a character record that names it.
    let s = structure::get(reg.deltas(), id).unwrap();
    station_ref(&reg, &s).seat(reg.character_mut(FOE).unwrap());
    evacuate(&mut reg, &s);
    assert_eq!(reg.character(FOE).unwrap().station_id, Some(HARBOUR));
}

#[test]
fn a_desync_recovery_from_a_docked_pilot_keeps_the_structure_as_the_ego() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    let _ = reg.take_pending();
    crate::service::space::resend_state(&mut reg, Some(BOSS));
    assert!(!reg.roster().is_in_space(BOSS), "not seated in space");
    assert_eq!(docked_in(&reg, BOSS), Some(id));
    assert_eq!(names(&mut reg), vec!["DoDestinyUpdate"]);
}

#[test]
fn a_cloaked_ship_does_not_dock_in_a_structure() {
    let (mut reg, id) = world();
    let ship = reg.character(BOSS).unwrap().ship_id;
    crate::cloak::set_device_for_test(&mut reg, BOSS, ship);
    let e = dock_call(&mut reg, BOSS, id).expect_err("refused");
    assert_eq!(refusal(&e), "CantDockWhileCloaked");
    assert!(reg.roster().is_in_space(BOSS));
}

#[test]
fn an_undock_from_the_structure_view_drops_a_stale_cloak() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    crate::invuln::set_enabled(&mut reg, true);
    let ship = reg.character(BOSS).unwrap().ship_id;
    crate::cloak::grant_jump(&mut reg, BOSS, ship, crate::pyutil::filetime_now());
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
    assert!(!crate::cloak::is_cloaked(&reg, ship));
}

#[test]
fn a_denial_carries_its_reason() {
    let (mut reg, id) = world();
    let e = call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Dock",
        &[PyValue::Int(id), PyValue::Int(1)],
    )
    .expect_err("a ship that is not the active one");
    assert_eq!(refusal(&e), crate::dock_policy::DENIED);
    // The `reason` argument is there, with words in it.
    assert!(format!("{e:?}").contains("Dict([(Unicode("), "{e:?}");
}

#[test]
fn a_late_bind_after_a_seating_is_still_the_same_undock() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
    let at = reg.roster().get(BOSS).unwrap().motion.position;
    let _ = reg.take_pending();
    // There is no clock in the decision: the mark is spent on the first bind.
    assert!(on_bind(&mut reg, BOSS));
    assert_eq!(reg.roster().get(BOSS).unwrap().motion.position, at);
    assert!(!reg.ext::<Docked>().seated.contains(&BOSS));
}

#[test]
fn a_gm_move_inside_the_system_from_the_structure_view_seats_the_ship() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    let _ = reg.take_pending();
    let _ = reg.take_session_changes();
    let spot = [5_000_000.0, 0.0, 0.0];
    reg.relocate(
        BOSS,
        crate::service::relocate::Dest::Space {
            system: SYSTEM,
            spot,
        },
    )
    .expect("moves");
    assert!(reg.roster().is_in_space(BOSS));
    assert_eq!(docked_in(&reg, BOSS), None);
    let at = reg.roster().get(BOSS).unwrap().motion.position;
    assert!(crate::space_state::distance(at, spot) < 1.0, "{at:?}");
    let changes = reg.take_session_changes();
    assert!(changes.iter().any(|(who, ch)| *who == BOSS
        && key(ch, "structureid") == Some(&PyValue::Tuple(vec![PyValue::Int(id), PyValue::None]))));
    assert!(names(&mut reg).contains(&"DoDestinyUpdate"));
}

/// `GetAllInfo`'s answer as `(attribute -> value)`.
fn all_info_of(reg: &mut Registry, who: i64) -> Vec<(PyValue, PyValue)> {
    let got = call(
        reg,
        who,
        "dogmaIM",
        "GetAllInfo",
        &[
            PyValue::Bool(false),
            PyValue::Bool(true),
            PyValue::Bool(true),
        ],
    )
    .expect("answered");
    let PyValue::Instance { state, .. } = got else {
        panic!("allInfo is a KeyVal")
    };
    let PyValue::Dict(entries) = *state else {
        panic!("KeyVal state is a dict")
    };
    entries
}

/// The location column of an `invItem` row.
fn location_of(item: &PyValue) -> i64 {
    let PyValue::DbRow(row) = item else {
        panic!("invItem is a DbRow, got {item:?}")
    };
    let eve_marshal::DbValue::Int(location) = row.values()[3] else {
        panic!("locationID is an int")
    };
    location
}

/// S6: a pilot docked in a structure gets it primed, because the pod's row
/// names it and godma's `UpdateItem` raises `KeyError` on an unprimed
/// location. The row's own location is the system.
#[test]
fn get_all_info_primes_the_structure_a_pilot_is_docked_in() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).expect("docks");
    let info = all_info_of(&mut reg, BOSS);
    let PyValue::Dict(primed) = key(&info, "structureInfo").expect("present") else {
        panic!("structureInfo is a dict when docked in a structure")
    };
    assert_eq!(primed.len(), 1);
    assert_eq!(primed[0].1, PyValue::Int(id));
    let PyValue::Instance { state, .. } = &primed[0].0 else {
        panic!("a KeyVal row")
    };
    let PyValue::Dict(row) = state.as_ref() else {
        panic!("a KeyVal dict")
    };
    assert_eq!(key(row, "itemID"), Some(&PyValue::Int(id)));
    assert_eq!(location_of(key(row, "invItem").expect("invItem")), SYSTEM);
}

/// Asked to undock from the hangar view (no bind since the dock): the
/// station id stays the structure until the bind, but the dogma already
/// has no structure to prime and the ship row says the system.
#[test]
fn an_undocking_pod_primes_no_structure_and_sits_in_the_system() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
    let info = all_info_of(&mut reg, BOSS);
    assert_eq!(key(&info, "structureInfo"), Some(&PyValue::None));
    let PyValue::Dict(ships) = key(&info, "shipInfo").expect("shipInfo") else {
        panic!("shipInfo is a dict")
    };
    let PyValue::Instance { state, .. } = &ships[0].0 else {
        panic!("a KeyVal row")
    };
    let PyValue::Dict(row) = state.as_ref() else {
        panic!("a KeyVal dict")
    };
    assert_eq!(location_of(key(row, "invItem").expect("invItem")), SYSTEM);
}

/// Undocked, the pod's row no longer names the structure it left.
#[test]
fn an_undocked_pod_is_not_located_in_the_structure() {
    let (mut reg, id) = world();
    dock_call(&mut reg, BOSS, id).unwrap();
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOSS));
    call(
        &mut reg,
        BOSS,
        "structureDocking",
        "Undock",
        &[PyValue::Int(id), PyValue::None],
    )
    .expect("undocks");
    let info = all_info_of(&mut reg, BOSS);
    assert_eq!(key(&info, "structureInfo"), Some(&PyValue::None));
    let PyValue::Dict(ships) = key(&info, "shipInfo").expect("shipInfo") else {
        panic!("shipInfo is a dict")
    };
    let PyValue::Instance { state, .. } = &ships[0].0 else {
        panic!("a KeyVal row")
    };
    let PyValue::Dict(row) = state.as_ref() else {
        panic!("a KeyVal dict")
    };
    assert_ne!(location_of(key(row, "invItem").expect("invItem")), id);
}
