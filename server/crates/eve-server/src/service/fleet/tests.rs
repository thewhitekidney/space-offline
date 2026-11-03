//! The fleet service's acceptance suite: what the client sends through
//! `fleetObjectHandler`, `fleetProxy` and `fleetMgr`, and what it gets back.

use super::*;
use crate::model::{Character, SandboxConfig};

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
/// Invented ship types: none is the client's.
const SHIP_TYPE: i64 = 990_001;
const OTHER_SHIP: i64 = 990_011;
const SPARE_SHIP: i64 = 990_012;

/// A registry with two characters, which is what this subsystem is for.
fn registry() -> Registry {
    let mut config = SandboxConfig::default();
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: 100_000_001,
        ..Character::default()
    };
    let bob = Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: 100_000_002,
        ..Character::default()
    };
    config.characters = vec![alice, bob];
    Registry::new(config)
}

fn call(
    reg: &mut Registry,
    actor: Option<i64>,
    method: &str,
    args: Vec<PyValue>,
    bind: Option<PyValue>,
) -> Result<PyValue, ServiceError> {
    let bind = bind.unwrap_or(PyValue::None);
    let req = Request {
        service: "fleetObjectHandler",
        method,
        args: &args,
        kwargs: &[],
        bind_params: Some(&bind),
        actor,
    };
    dispatch(reg, &req)
}

/// Alice creates a fleet and `Init`s it, the way `fleetSvc.CreateFleet`
/// does: `CreateFleet`, then `Init`, then `GetFleetID`, on an object
/// minted with no bind params.
fn alices_fleet(reg: &mut Registry) -> i64 {
    call(reg, Some(ALICE), "CreateFleet", vec![], None).expect("create");
    call(
        reg,
        Some(ALICE),
        "Init",
        vec![PyValue::Int(SHIP_TYPE)],
        None,
    )
    .expect("init");
    let PyValue::Int(id) = call(reg, Some(ALICE), "GetFleetID", vec![], None).expect("id") else {
        panic!("GetFleetID must answer an integer");
    };
    id
}

// -------------------------------------------------- the no-actor branch

/// **Every arm's no-character branch, in one test.** A connection that
/// has not run `SelectCharacterID` has `actor == None`, and that is a
/// real state rather than a cue to pick a character -- the mistake
/// `Request::actor`'s `Option` exists to make unrepresentable.
///
/// `FleetNotInFleet` and not a silent `None`: a character who is not
/// playing is in no fleet, and that is a dialog this build ships.
#[test]
fn every_method_answers_the_no_actor_branch_without_inventing_a_character() {
    let mut reg = registry();
    // Alice has a fleet, so an arm that read "the first character" would
    // find one and answer about it.
    let id = alices_fleet(&mut reg);
    let bind = PyValue::Int(id);
    for method in [
        "CreateFleet",
        "Init",
        "GetInitState",
        "GetFleetID",
        "GetWings",
        "GetMotd",
        "GetFleetMaxSize",
        "GetFleetComposition",
        "GetJoinRequests",
        "RejectJoinRequest",
        "Invite",
        "AcceptInvite",
        "RejectInvite",
        "LeaveFleet",
        "KickMember",
        "MakeLeader",
        "DisbandFleet",
        "MoveMember",
        "MassMoveMembers",
        "CreateWing",
        "DeleteWing",
        "ChangeWingName",
        "CreateSquad",
        "DeleteSquad",
        "ChangeSquadName",
        "SetMotdEx",
        "SetOptions",
        "SetFleetMaxSize",
        "SetAutoJoinSquadID",
        "SetTakesFleetWarp",
        "SetAcceptsRegroupValue",
        "SetAcceptsConduitJumpsValue",
        "Reconnect",
        "UpdateMemberInfo",
        "SendBroadcast",
    ] {
        let got = call(&mut reg, None, method, vec![], Some(bind.clone()));
        assert!(
            matches!(got, Err(ServiceError::Raise(_))),
            "{method} with no actor answered {got:?} instead of raising"
        );
    }
    // That list is `METHODS`, so this is also the check that the two
    // cannot drift: a method added to `METHODS` without being added above
    // is one whose no-actor branch nobody wrote.
    assert_eq!(
        METHODS.len(),
        35,
        "a method was added to METHODS; add it to the no-actor sweep too"
    );
    // A method we do **not** implement must still not reach a character:
    // it is `NoSuchMethod` whether or not anyone is playing, because the
    // method name is settled before the actor is looked at.
    for method in ["LoadFleetSetup"] {
        for actor in [None, Some(ALICE)] {
            assert!(
                matches!(
                    call(&mut reg, actor, method, vec![], Some(bind.clone())),
                    Err(ServiceError::NoSuchMethod { .. })
                ),
                "{method} must be refused by name, not by fleet state"
            );
        }
    }
    // And the fleet is untouched: nothing above mutated anything.
    assert_eq!(
        reg.fleets().fleet(id).expect("fleet").members.len(),
        1,
        "a no-actor call changed the world"
    );
}

// -------------------------------------------- the two kinds of binding

/// The creator's object is minted **before** the client knows the fleet
/// id, so its bind params are `None` and the arm has to resolve the fleet
/// from the actor. The invitee's comes from
/// `Moniker('fleetObjectHandler', fleetID)` and carries a bare integer.
#[test]
fn a_fleet_resolves_from_the_actor_or_from_a_bare_integer_bind() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    // No bind params: the creator's own object.
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetFleetID", vec![], None).expect("id"),
        PyValue::Int(id)
    );
    // A bare integer: `eveMoniker.GetFleet`'s shape.
    assert_eq!(
        call(
            &mut reg,
            Some(ALICE),
            "GetFleetID",
            vec![],
            Some(PyValue::Int(id))
        )
        .expect("id"),
        PyValue::Int(id)
    );
    // Bob is in no fleet and binds none, so there is nothing to answer
    // about -- and the answer must not be Alice's fleet.
    assert!(matches!(
        call(&mut reg, Some(BOB), "GetFleetID", vec![], None),
        Err(ServiceError::Raise(_))
    ));
}

/// `CreateFleet` is the one method whose bind params are meaningless, so
/// it must not go through the resolver -- a second `CreateFleet` from a
/// character already in a fleet is refused on its own terms.
#[test]
fn create_fleet_does_not_need_a_fleet_to_resolve() {
    let mut reg = registry();
    assert!(matches!(
        call(&mut reg, Some(BOB), "CreateFleet", vec![], None),
        Ok(PyValue::Int(_))
    ));
    // Bob has a record but has not `Init`ed, so a second create is still
    // allowed -- `create` tests membership, not the record.
    call(
        &mut reg,
        Some(BOB),
        "Init",
        vec![PyValue::Int(SHIP_TYPE)],
        None,
    )
    .expect("init");
    assert!(matches!(
        call(&mut reg, Some(BOB), "CreateFleet", vec![], None),
        Err(ServiceError::Raise(_)),
    ));
}

// -------------------------------------------------------- the read arms

/// `initState.fleetID` is read on the very next line
/// (`fleetSvc.InitFleet:306`), so `None` here is an `AttributeError`
/// inside the handler that three separate paths call.
#[test]
fn get_init_state_answers_an_object_with_the_five_attributes() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    let state = call(&mut reg, Some(ALICE), "GetInitState", vec![], None).expect("state");
    let PyValue::Instance { state, .. } = &state else {
        panic!("GetInitState must be an old-style instance: {state:?}");
    };
    for key in ["fleetID", "members", "wings", "options", "motd"] {
        assert!(
            crate::pyutil::dict_get(state, key.as_bytes()).is_some(),
            "InitFleet reads initState.{key}"
        );
    }
    assert_eq!(
        crate::pyutil::dict_get(state, b"fleetID"),
        Some(&PyValue::Int(id))
    );
}

/// `GetMotd` is cached behind `if self.motd is None:` with the result
/// stored back, so `None` makes the client re-ask forever. An empty
/// string is the answer that ends it.
#[test]
fn the_motd_is_a_string_and_never_none() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetMotd", vec![], None).expect("motd"),
        PyValue::Unicode(String::new())
    );
    call(
        &mut reg,
        Some(ALICE),
        "SetMotdEx",
        vec![PyValue::Unicode("form up".to_owned())],
        None,
    )
    .expect("set");
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetMotd", vec![], None).expect("motd"),
        PyValue::Unicode("form up".to_owned())
    );
}

/// `GetFleetComposition`'s callers `iterate x8`, so it has to be a list.
/// `GetJoinRequests`'s window does `.itervalues()`, so it has to be a
/// dict. `None` for either is a `TypeError` in the client's own build.
#[test]
fn the_container_answers_are_the_right_container_kind() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    let comp =
        call(&mut reg, Some(ALICE), "GetFleetComposition", vec![], None).expect("composition");
    let PyValue::List(rows) = &comp else {
        panic!("GetFleetComposition must be a list: {comp:?}");
    };
    assert_eq!(rows.len(), 1, "one row per member");
    assert!(matches!(
        call(&mut reg, Some(ALICE), "GetJoinRequests", vec![], None).expect("requests"),
        PyValue::Dict(_)
    ));
}

/// `SetFleetMaxSize` feeds `GetFleetMaxSize`'s answer into `uix.QtyPopup`
/// as the current value, and that prompt offers 1 to `MAX_MEMBERS_IN_FLEET`
/// (`fleetSvc.py:2196-2201`), so it is an integer in that range.
#[test]
fn the_max_size_is_an_integer_and_is_clamped() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetFleetMaxSize", vec![], None).expect("size"),
        PyValue::Int(256)
    );
    call(
        &mut reg,
        Some(ALICE),
        "SetFleetMaxSize",
        vec![PyValue::Int(1_000_000)],
        None,
    )
    .expect("set");
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetFleetMaxSize", vec![], None).expect("size"),
        PyValue::Int(256),
        "clamped to MAX_MEMBERS_IN_FLEET"
    );
    call(
        &mut reg,
        Some(ALICE),
        "SetFleetMaxSize",
        vec![PyValue::Int(0)],
        None,
    )
    .expect("set");
    assert_eq!(
        call(&mut reg, Some(ALICE), "GetFleetMaxSize", vec![], None).expect("size"),
        PyValue::Int(1),
        "clamped up to the prompt's floor of 1"
    );
}

// ------------------------------------------------- the two-player path

/// **The thing this lane exists for**, end to end through the dispatcher:
/// Alice forms a fleet, invites Bob, Bob accepts, and the server has two
/// people in one fleet with the pushes addressed to the right characters.
#[test]
fn two_characters_end_up_in_one_fleet() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);

    // Alice invites Bob. Four arguments, the last three unspecified.
    let invited = call(
        &mut reg,
        Some(ALICE),
        "Invite",
        vec![
            PyValue::Int(BOB),
            PyValue::None,
            PyValue::None,
            PyValue::None,
        ],
        None,
    )
    .expect("invite");
    assert_eq!(invited, PyValue::Bool(true));
    assert!(reg.fleets().invite_of(BOB).is_some());

    // Bob's client binds `Moniker('fleetObjectHandler', fleetID)` from
    // the notification's first argument and calls `AcceptInvite`.
    let accepted = call(
        &mut reg,
        Some(BOB),
        "AcceptInvite",
        vec![PyValue::Int(SHIP_TYPE)],
        Some(PyValue::Int(id)),
    )
    .expect("accept");
    assert_eq!(accepted, PyValue::Bool(true));

    let fleet = reg.fleets().fleet(id).expect("fleet");
    assert_eq!(fleet.members.len(), 2);
    assert!(fleet.members[&ALICE].is_boss());
    assert!(!fleet.members[&BOB].is_boss());
    // And Bob's own bound object now answers about the same fleet.
    assert_eq!(
        call(&mut reg, Some(BOB), "GetFleetID", vec![], None).expect("id"),
        PyValue::Int(id)
    );
}

/// A member cannot invite into a fleet they are not in, and cannot kick
/// from one either -- the authority checks are in the state machine and
/// this is the arm honouring them rather than bypassing them.
#[test]
fn a_stranger_cannot_act_on_a_fleet_they_bound_by_id() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    let bind = Some(PyValue::Int(id));
    for (method, args) in [
        ("Invite", vec![PyValue::Int(ALICE)]),
        ("KickMember", vec![PyValue::Int(ALICE)]),
        ("DisbandFleet", vec![]),
        ("CreateWing", vec![]),
        ("SetMotdEx", vec![PyValue::Unicode("mine now".to_owned())]),
    ] {
        let got = call(&mut reg, Some(BOB), method, args, bind.clone());
        assert!(
            matches!(got, Err(ServiceError::Raise(_))),
            "{method} let a stranger act: {got:?}"
        );
    }
    // Including `Init`, which is the one that would seat them.
    assert!(matches!(
        call(
            &mut reg,
            Some(BOB),
            "Init",
            vec![PyValue::Int(SHIP_TYPE)],
            bind
        ),
        Err(ServiceError::Raise(_))
    ));
    assert_eq!(reg.fleets().fleet(id).expect("fleet").members.len(), 1);
}

// ---------------------------------------------------- structure and opts

#[test]
fn creating_a_wing_and_a_squad_answers_their_ids() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    let PyValue::Int(wing) = call(&mut reg, Some(ALICE), "CreateWing", vec![], None).expect("wing")
    else {
        panic!("CreateWing's answer is stored as wingID with consumer `truth`");
    };
    let PyValue::Int(squad) = call(
        &mut reg,
        Some(ALICE),
        "CreateSquad",
        vec![PyValue::Int(wing)],
        None,
    )
    .expect("squad") else {
        panic!("CreateSquad must answer the new id");
    };
    assert_ne!(wing, squad);
    // And the tree the client re-reads has both.
    let wings = call(&mut reg, Some(ALICE), "GetWings", vec![], None).expect("wings");
    let PyValue::Dict(entries) = &wings else {
        panic!("GetWings must be a dict");
    };
    assert_eq!(entries.len(), 2, "the default wing plus the new one");
}

/// `SetOptions`'s answer is **read** (consumer `pass`, two `if x not in
/// y` caller guards), and an absent key means "leave it" rather than
/// "set it false".
#[test]
fn set_options_echoes_the_options_and_leaves_absent_keys_alone() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    let answer = call(
        &mut reg,
        Some(ALICE),
        "SetOptions",
        vec![crate::pyutil::keyval(vec![(
            "isFreeMove",
            PyValue::Bool(true),
        )])],
        None,
    )
    .expect("options");
    let PyValue::Instance { state, .. } = &answer else {
        panic!("the options answer is read: {answer:?}");
    };
    assert_eq!(
        crate::pyutil::dict_get(state, b"isFreeMove"),
        Some(&PyValue::Bool(true))
    );
    // `autoJoinSquadID` was absent, so it is still unset rather than
    // cleared-and-reported.
    assert_eq!(
        crate::pyutil::dict_get(state, b"autoJoinSquadID"),
        Some(&PyValue::None)
    );
    assert!(
        reg.fleets()
            .fleet(_fleet_id(&reg))
            .expect("fleet")
            .options
            .is_free_move
    );
}

/// The only fleet in the registry, for a test that does not want to carry
/// the id around.
fn _fleet_id(reg: &Registry) -> i64 {
    reg.fleets()
        .fleet_of(ALICE)
        .map(|f| f.fleet_id)
        .expect("Alice is in a fleet")
}

/// The three opt-out setters write the three keys
/// `evefleet/members.pyc`'s own constructor builds, and a change is
/// reported as `OnFleetMemberChanged` because `memberOptOuts` is one of
/// its five old/new fields.
#[test]
fn the_three_opt_out_setters_each_move_their_own_flag() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    for (method, read) in [
        (
            "SetTakesFleetWarp",
            (|o: crate::fleet::MemberOptOuts| o.accepts_fleet_warp)
                as fn(crate::fleet::MemberOptOuts) -> bool,
        ),
        ("SetAcceptsRegroupValue", |o| o.accepts_fleet_regroups),
        ("SetAcceptsConduitJumpsValue", |o| o.accepts_conduit_jumps),
    ] {
        call(
            &mut reg,
            Some(ALICE),
            method,
            vec![PyValue::Bool(false)],
            Some(PyValue::Int(id)),
        )
        .unwrap_or_else(|e| panic!("{method}: {e}"));
        let opts = reg.fleets().fleet(id).expect("fleet").members[&ALICE].opt_outs;
        assert!(!read(opts), "{method} did not clear its own flag");
        // And nothing else moved.
        call(
            &mut reg,
            Some(ALICE),
            method,
            vec![PyValue::Bool(true)],
            Some(PyValue::Int(id)),
        )
        .expect("restore");
        assert_eq!(
            reg.fleets().fleet(id).expect("fleet").members[&ALICE].opt_outs,
            crate::fleet::MemberOptOuts::default(),
            "{method} left another flag changed"
        );
    }
}

/// `RejectInvite` is called with **0 or 1** arguments by the client
/// (`fleetSvc.py:1123` sends `True`, `:1141` sends nothing), so both
/// have to be accepted.
#[test]
fn reject_invite_takes_zero_or_one_arguments() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    let bind = Some(PyValue::Int(id));
    call(
        &mut reg,
        Some(ALICE),
        "Invite",
        vec![PyValue::Int(BOB)],
        None,
    )
    .expect("invite");
    assert_eq!(
        call(&mut reg, Some(BOB), "RejectInvite", vec![], bind.clone()).expect("reject"),
        PyValue::Bool(true)
    );
    call(
        &mut reg,
        Some(ALICE),
        "Invite",
        vec![PyValue::Int(BOB)],
        None,
    )
    .expect("invite");
    assert_eq!(
        call(
            &mut reg,
            Some(BOB),
            "RejectInvite",
            vec![PyValue::Bool(true)],
            bind
        )
        .expect("reject"),
        PyValue::Bool(true)
    );
    assert!(reg.fleets().invite_of(BOB).is_none());
}

/// `MassMoveMembers`'s first argument is a list, and the answer is the list
/// of pilots who moved: the client passes it to `OnManyFleetMembersChanging`,
/// which iterates it (`fleetSvc.py:518-520`, `ui/shared/fleet/fleet.py:146-149`).
#[test]
fn mass_move_takes_a_list_and_reports_partial_success() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    call(
        &mut reg,
        Some(ALICE),
        "Invite",
        vec![PyValue::Int(BOB)],
        None,
    )
    .expect("invite");
    call(
        &mut reg,
        Some(BOB),
        "AcceptInvite",
        vec![PyValue::Int(SHIP_TYPE)],
        Some(PyValue::Int(id)),
    )
    .expect("accept");
    let PyValue::Int(wing) = call(&mut reg, Some(ALICE), "CreateWing", vec![], None).expect("wing")
    else {
        panic!("wing id");
    };
    let PyValue::Int(squad) = call(
        &mut reg,
        Some(ALICE),
        "CreateSquad",
        vec![PyValue::Int(wing)],
        None,
    )
    .expect("squad") else {
        panic!("squad id");
    };
    // Bob plus a character who is not in the fleet: one moves, one is
    // skipped, and only the one who moved is named.
    let moved = call(
        &mut reg,
        Some(ALICE),
        "MassMoveMembers",
        vec![
            PyValue::List(vec![PyValue::Int(BOB), PyValue::Int(42)]),
            PyValue::Int(wing),
            PyValue::Int(squad),
            PyValue::None,
        ],
        None,
    )
    .expect("mass move");
    assert_eq!(moved, PyValue::List(vec![PyValue::Int(BOB)]));
    assert_eq!(
        reg.fleets().fleet(id).expect("fleet").members[&BOB].squad_id,
        squad
    );
}

/// A method we deliberately do not answer falls through to the
/// `UNANSWERED` diagnostic, which names it in the log -- the signal the
/// next lane drives from. It must not be a silent `None`.
#[test]
fn a_method_this_lane_left_out_is_refused_by_name() {
    let mut reg = registry();
    let _ = alices_fleet(&mut reg);
    let method = "LoadFleetSetup";
    let got = call(&mut reg, Some(ALICE), method, vec![], None);
    match got {
        Err(ServiceError::NoSuchMethod { method: m, .. }) => assert_eq!(m, method),
        other => panic!("{method} answered {other:?} instead of naming itself"),
    }
}

/// The Fleet Finder's list, and `{}` is the answer rather than a stub.
///
/// `fleetSvc.GetAvailableFleetAds` does
/// `for fleetID, advert in ...iteritems()` unguarded, so `None` was an
/// `AttributeError` -- which is what a real client produced. Nobody here
/// has advertised a fleet, so an empty dict is true.
///
/// **Answered for a caller with no character too.** The module's
/// `FleetNotInFleet` guard is about methods on a fleet object; the finder
/// is how somebody with no fleet finds one.
#[test]
fn available_fleet_ads_is_an_empty_dict_even_for_nobody() {
    let mut reg = registry();
    for actor in [None, Some(90_000_001)] {
        let answer = dispatch(
            &mut reg,
            &Request {
                service: "fleetProxy",
                method: "GetAvailableFleetAds",
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            },
        )
        .expect("the finder answers");
        assert_eq!(
            answer,
            PyValue::Dict(vec![]),
            "iteritems() needs a dict, actor={actor:?}"
        );
    }
}

/// `fleetMgr.RegisterForDamageUpdates` is answered and registers nothing.
///
/// The client discards the value and its one session-change caller sits
/// inside a per-listener `except Exception`, so the shape cannot be got
/// wrong -- the reason to answer it at all is that the `UNANSWERED`
/// diagnostic would otherwise fire on every entry into space for a call
/// that needs nothing.
#[test]
fn registering_for_damage_updates_is_answered_and_does_nothing() {
    let mut reg = registry();
    let before = reg.fleets().len();
    let answer = dispatch(
        &mut reg,
        &Request {
            service: "fleetMgr",
            method: "RegisterForDamageUpdates",
            args: &[PyValue::Int(1)],
            kwargs: &[],
            bind_params: None,
            actor: Some(90_000_001),
        },
    )
    .expect("answered rather than logged");
    assert_eq!(answer, PyValue::None);
    assert_eq!(before, reg.fleets().len(), "and nothing happened");
}

// ------------------------------------------------------------- FC-0

fn field<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Instance { state, .. } => crate::pyutil::dict_get(state, key.as_bytes()),
        other => crate::pyutil::dict_get(other, key.as_bytes()),
    }
}

fn composition_row(reg: &mut Registry, who: i64) -> PyValue {
    let PyValue::List(rows) =
        call(reg, Some(ALICE), "GetFleetComposition", vec![], None).expect("composition")
    else {
        panic!("a list");
    };
    rows.into_iter()
        .find(|r| field(r, "characterID") == Some(&PyValue::Int(who)))
        .expect("a row for the member")
}

fn opt(v: Option<i64>) -> PyValue {
    v.map_or(PyValue::None, PyValue::Int)
}

fn where_is(reg: &mut Registry, who: i64) -> (PyValue, PyValue) {
    let row = composition_row(reg, who);
    let get = |k: &str| field(&row, k).cloned().expect("field");
    (get("solarSystemID"), get("stationID"))
}

/// A member who undocks, jumps (the roster entry moves) and docks is read
/// live each time, whatever the `Member` row recorded at accept.
#[test]
fn composition_follows_undock_jump_and_dock() {
    const STATION: i64 = 5_001;
    const HOME: i64 = 7_001;
    const NEXT: i64 = 7_002;
    let mut reg = registry();
    alices_fleet(&mut reg);
    let fleet = reg.fleets().fleet_of(ALICE).expect("fleet").fleet_id;
    reg.fleets_mut()
        .invite(ALICE, fleet, BOB, None, None, None)
        .expect("invited");
    reg.fleets_mut()
        .accept_invite(BOB, fleet, None, None, None)
        .expect("seated");
    {
        let c = reg.character_mut(BOB).expect("bob");
        c.station_id = Some(STATION);
        c.solar_system_id = Some(HOME);
        c.ship_type_id = OTHER_SHIP;
    }
    assert_eq!(
        where_is(&mut reg, BOB),
        (opt(Some(HOME)), opt(Some(STATION))),
        "docked"
    );
    let arrive = |reg: &mut Registry, system: i64| {
        reg.roster_mut().leave(BOB);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                BOB,
                100_000_002,
                system,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
    };
    arrive(&mut reg, HOME);
    assert_eq!(
        where_is(&mut reg, BOB),
        (opt(Some(HOME)), PyValue::None),
        "undocked: system, no station"
    );
    arrive(&mut reg, NEXT);
    assert_eq!(
        where_is(&mut reg, BOB),
        (opt(Some(NEXT)), PyValue::None),
        "jumped"
    );
    reg.roster_mut().leave(BOB);
    assert_eq!(
        where_is(&mut reg, BOB),
        (opt(Some(HOME)), opt(Some(STATION))),
        "docked again: the character record answers"
    );
    let row = composition_row(&mut reg, BOB);
    assert_eq!(
        field(&row, "shipTypeID"),
        Some(&PyValue::Int(OTHER_SHIP)),
        "the ship type is the character's"
    );
}

/// `Init` from space names the system the pilot is in, not the one his
/// station is in, and no station (`docs/lanes/stale-system.md`).
#[test]
fn init_in_space_names_the_session_system() {
    const HOME: i64 = 7_001;
    const NEXT: i64 = 7_002;
    let mut reg = registry();
    {
        let c = reg.character_mut(ALICE).expect("alice");
        c.station_id = Some(5_001);
        c.solar_system_id = Some(HOME);
    }
    reg.roster_mut()
        .arrive(crate::space_state::Presence::arriving(
            ALICE,
            100_000_001,
            NEXT,
            crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    alices_fleet(&mut reg);
    let m = &reg.fleets().fleet_of(ALICE).expect("fleet").members[&ALICE];
    assert_eq!((m.solar_system_id, m.station_id), (Some(NEXT), None));
}

/// With no roster entry the character record answers; with no location on
/// the record either, the `Member` fields do.
#[test]
fn composition_falls_back_to_the_character_then_the_member() {
    let mut reg = registry();
    alices_fleet(&mut reg);
    let fleet = reg.fleets().fleet_of(ALICE).expect("fleet").fleet_id;
    reg.fleets_mut()
        .invite(ALICE, fleet, BOB, None, None, None)
        .expect("invited");
    reg.fleets_mut()
        .accept_invite(BOB, fleet, None, None, None)
        .expect("seated");
    {
        let c = reg.character_mut(BOB).expect("bob");
        c.station_id = None;
        c.solar_system_id = None;
        c.ship_type_id = 0;
    }
    let m = reg.fleets().fleet(fleet).expect("fleet").members[&BOB].clone();
    let mut m = m;
    m.solar_system_id = Some(7_100);
    m.station_id = Some(5_100);
    m.ship_type_id = Some(SPARE_SHIP);
    assert_eq!(
        live_location(&reg, &m),
        (Some(7_100), Some(5_100), Some(SPARE_SHIP)),
        "the Member fields"
    );
    reg.character_mut(BOB).expect("bob").solar_system_id = Some(7_200);
    assert_eq!(
        live_location(&reg, &m),
        (Some(7_200), None, Some(SPARE_SHIP)),
        "the character record"
    );
}

/// The client takes new options only from `OnFleetOptionsChanged(oldOptions,
/// options)` (`fleetSvc.py:1254-1265`), so `SetOptions` and
/// `SetAutoJoinSquadID` tell the fleet, with the options before and after.
#[test]
fn changing_the_options_tells_the_fleet_old_and_new() {
    let mut reg = registry();
    let id = alices_fleet(&mut reg);
    let _ = reg.take_pending();
    call(
        &mut reg,
        Some(ALICE),
        "SetOptions",
        vec![crate::pyutil::keyval(vec![(
            "isFreeMove",
            PyValue::Bool(true),
        )])],
        None,
    )
    .expect("options");
    let told: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
    let changes: Vec<_> = told
        .iter()
        .filter(|p| p.to_push().0 == "OnFleetOptionsChanged")
        .collect();
    assert_eq!(changes.len(), 1, "one notification for one change");
    assert_eq!(changes[0].audience(), Some(vec![ALICE]));
    let (_, args) = changes[0].to_push();
    assert_eq!(args.len(), 2, "(oldOptions, options)");
    assert_eq!(field(&args[0], "isFreeMove"), Some(&PyValue::Bool(false)));
    assert_eq!(field(&args[1], "isFreeMove"), Some(&PyValue::Bool(true)));
    // The default squad goes the same way.
    let squad = reg
        .fleets()
        .fleet(id)
        .expect("fleet")
        .wings
        .values()
        .next()
        .expect("wing")
        .squads
        .keys()
        .next()
        .copied()
        .expect("squad");
    call(
        &mut reg,
        Some(ALICE),
        "SetAutoJoinSquadID",
        vec![PyValue::Int(squad)],
        None,
    )
    .expect("auto-join");
    let told: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
    let change = told
        .iter()
        .find(|p| p.to_push().0 == "OnFleetOptionsChanged")
        .expect("the default squad change is told");
    let (_, args) = change.to_push();
    assert_eq!(field(&args[0], "autoJoinSquadID"), Some(&PyValue::None));
    assert_eq!(
        field(&args[1], "autoJoinSquadID"),
        Some(&PyValue::Int(squad))
    );
    assert_eq!(
        reg.fleets()
            .fleet(id)
            .expect("fleet")
            .options
            .auto_join_squad_id,
        Some(squad)
    );
}
