//! The fleet state machine's tests: the acceptance suite.

use super::*;

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const CAROL: i64 = 90_000_003;
const NODE: i64 = 1;
/// An invented ship type, station and system: none is the client's.
const SHIP_TYPE: i64 = 990_001;
const STATION: i64 = 990_002;
const SYSTEM: i64 = 990_003;

/// A fleet with Alice as boss, and the fleet id.
fn with_alice() -> (Fleets, i64) {
    let mut f = Fleets::new(NODE);
    let id = f.create(ALICE).expect("create");
    f.init(ALICE, id, Some(SHIP_TYPE), Some(STATION), Some(SYSTEM))
        .expect("init");
    (f, id)
}

/// Alice's fleet with Bob in it, invited and accepted.
fn with_alice_and_bob() -> (Fleets, i64) {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    f.accept_invite(BOB, id, Some(SHIP_TYPE), Some(STATION), Some(SYSTEM))
        .expect("accept");
    (f, id)
}

fn events(e: &Effects) -> Vec<&'static str> {
    e.pushes.iter().map(|p| p.event.to_push().0).collect()
}

/// An invented beacon: ids are not the client's.
const BEACON: Beacon = Beacon {
    solar_system_id: 990_779,
    beacon_id: 9_800_000_000_001,
    type_id: 990_777,
};

/// A pilot in no fleet has nobody to tell, and the field is still
/// remembered; joining a fleet then announces it (CAP-D).
#[test]
fn a_beacon_lit_outside_a_fleet_is_announced_when_its_pilot_joins_one() {
    let (mut f, id) = with_alice();
    assert!(f.light_beacon(BOB, BEACON).is_empty(), "no fleet, no push");
    assert_eq!(f.beacon_of(BOB), Some(BEACON));
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    let (_, effects) = f
        .accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    let told: Vec<&Push> = effects
        .pushes
        .iter()
        .filter(|p| p.event.to_push().0 == "OnFleetJumpBeaconModuleChange")
        .collect();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].to, vec![ALICE], "the members already there");
    assert_eq!(f.beacons_visible_to(ALICE), vec![(BOB, BEACON)]);
}

/// The joiner learns what the members already hold, and the one who
/// leaves is taken off the others' lists (the leaver's own client clears).
#[test]
fn beacons_are_replayed_to_a_joiner_and_withdrawn_on_leaving() {
    let (mut f, id) = with_alice();
    let pushes = f.light_beacon(ALICE, BEACON);
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].to, vec![ALICE]);
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    let (_, joined) = f
        .accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    let replay = joined
        .pushes
        .iter()
        .find(|p| p.event.to_push().0 == "OnFleetJumpBeaconModuleChange")
        .expect("a replay");
    assert_eq!(replay.to, vec![BOB], "to the joiner alone");
    let (name, args) = replay.event.to_push();
    assert_eq!(name, "OnFleetJumpBeaconModuleChange");
    assert_eq!(args[0], PyValue::Int(ALICE));
    assert_eq!(args[4], PyValue::Bool(true));
    // Alice leaves; Bob is told her beacon is off, she is not.
    let left = f.leave(ALICE, id).expect("leaves");
    let off = left
        .pushes
        .iter()
        .find(|p| p.event.to_push().0 == "OnFleetJumpBeaconModuleChange")
        .expect("a withdrawal");
    assert_eq!(off.to, vec![BOB]);
    assert_eq!(off.event.to_push().1[4], PyValue::Bool(false));
    // Doused: the pilot's own field going out tells nobody now.
    assert!(f.douse_beacon(ALICE).is_empty());
    assert_eq!(f.beacon_of(ALICE), None);
}

/// An invented portal: ids are not the client's.
const BRIDGE: Bridge = Bridge {
    ship_id: 990_778,
    solar_system_id: 990_780,
    beacon_id: 9_800_000_000_002,
};

/// Portals are replayed to a joiner and withdrawn on leaving, as beacons
/// are (CAP-G); closing one tells the fleet once.
#[test]
fn bridges_are_replayed_to_a_joiner_and_withdrawn_on_leaving() {
    let (mut f, id) = with_alice();
    let opened = f.open_bridge(ALICE, BRIDGE);
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].to, vec![ALICE]);
    let (name, args) = opened[0].event.to_push();
    assert_eq!(name, "OnBridgeModeChange");
    assert_eq!(args[0], PyValue::Int(BRIDGE.ship_id));
    assert_eq!(args[1], PyValue::Int(BRIDGE.solar_system_id));
    assert_eq!(args[2], PyValue::Int(BRIDGE.beacon_id));
    assert_eq!(args[3], PyValue::Bool(true));
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    let (_, joined) = f
        .accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    let replay = joined
        .pushes
        .iter()
        .find(|p| p.event.to_push().0 == "OnBridgeModeChange")
        .expect("a replay");
    assert_eq!(replay.to, vec![BOB], "to the joiner alone");
    assert_eq!(f.bridges_visible_to(BOB), vec![(ALICE, BRIDGE)]);
    let left = f.leave(ALICE, id).expect("leaves");
    let off = left
        .pushes
        .iter()
        .find(|p| p.event.to_push().0 == "OnBridgeModeChange")
        .expect("a withdrawal");
    assert_eq!(off.to, vec![BOB]);
    assert_eq!(off.event.to_push().1[3], PyValue::Bool(false));
}

/// Closing a portal tells the fleet once; closing it again tells nobody.
#[test]
fn closing_a_bridge_tells_the_fleet_once() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    f.accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    f.open_bridge(ALICE, BRIDGE);
    let closed = f.close_bridge(ALICE);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].to.len(), 2, "Alice and Bob");
    assert_eq!(closed[0].event.to_push().1[3], PyValue::Bool(false));
    assert!(f.close_bridge(ALICE).is_empty());
    assert_eq!(f.bridge_of(ALICE), None);
}

/// One attribute off a `KeyVal`.
///
/// `pyutil::dict_get` only looks inside a `PyValue::Dict`, and a `KeyVal`
/// is a `PyValue::Instance` whose `state` is that dict -- which is
/// exactly the indirection the client itself performs
/// (`instance.__dict__.update(state)`), so unwrapping it here is reading
/// the payload the way the client does.
fn attr<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    crate::pyutil::dict_get(state, key.as_bytes())
}

/// Every attribute name on a `KeyVal`, sorted.
fn attrs(v: &PyValue) -> Vec<String> {
    let PyValue::Instance { state, .. } = v else {
        panic!("not an old-style instance: {v:?}");
    };
    let PyValue::Dict(entries) = state.as_ref() else {
        panic!("a KeyVal's state must be a dict");
    };
    let mut keys: Vec<String> = entries
        .iter()
        .filter_map(|(_, k)| crate::pyutil::as_bytes(k))
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    keys.sort();
    keys
}

// ------------------------------------------------- the membership machine

#[test]
fn create_mints_an_empty_fleet_with_one_wing_and_one_squad() {
    let mut f = Fleets::new(NODE);
    let id = f.create(ALICE).expect("create");
    let fleet = f.fleet(id).expect("exists");
    // Empty, because `Init` seats the creator one line later in the
    // client. See `Fleets::create`.
    assert!(fleet.members.is_empty());
    assert_eq!(fleet.wings.len(), 1);
    assert_eq!(fleet.wings.values().next().expect("wing").squads.len(), 1);
    // Not a member yet, so not resolvable as "Alice's fleet"...
    assert!(f.fleet_of(ALICE).is_none());
    // ...but her bound object still has to resolve to it.
    assert_eq!(f.resolve_for(ALICE), Some(id));
}

#[test]
fn init_seats_the_creator_as_commander_and_boss() {
    let (f, id) = with_alice();
    let m = &f.fleet(id).expect("fleet").members[&ALICE];
    assert_eq!(m.role, consts::ROLE_LEADER);
    assert!(m.is_boss(), "job & JOB_CREATOR is what GetRankName reads");
    assert_eq!(m.wing_id, consts::NONE_ID, "the commander is in no wing");
    assert_eq!(m.squad_id, consts::NONE_ID);
    assert_eq!(f.fleet_of(ALICE).map(|x| x.fleet_id), Some(id));
}

/// `Init` is how a fleet is formed, not a way into one. A pilot who binds
/// a stranger's fleet id must not be able to seat themselves.
#[test]
fn init_refuses_a_character_who_did_not_create_the_fleet() {
    let (mut f, id) = with_alice();
    let err = f.init(BOB, id, None, None, None).expect_err("must refuse");
    assert_eq!(err, FleetError::Generic);
    assert_eq!(f.fleet(id).expect("fleet").members.len(), 1);
}

/// The client calls `InitFleet` again on reconnect, so a second `Init`
/// from the member who is already in it must be a no-op, not an error.
#[test]
fn init_twice_is_idempotent() {
    let (mut f, id) = with_alice();
    let again = f.init(ALICE, id, None, None, None).expect("idempotent");
    assert!(again.pushes.is_empty() && again.sessions.is_empty());
    assert_eq!(f.fleet(id).expect("fleet").members.len(), 1);
}

#[test]
fn a_character_cannot_create_a_second_fleet() {
    let (mut f, _id) = with_alice();
    assert_eq!(f.create(ALICE).expect_err("refused"), FleetError::Generic);
}

#[test]
fn invite_then_accept_seats_the_invitee_in_the_first_squad() {
    let (f, id) = with_alice_and_bob();
    let fleet = f.fleet(id).expect("fleet");
    let bob = &fleet.members[&BOB];
    assert_eq!(bob.role, consts::ROLE_MEMBER);
    assert!(!bob.is_boss());
    let (w, s) = fleet.first_wing_and_squad();
    assert_eq!(Some(bob.wing_id), w);
    assert_eq!(Some(bob.squad_id), s);
    // The invite is consumed.
    assert!(f.invite_of(BOB).is_none());
}

/// **The push that cannot reach its target through the acting
/// connection.** The invitee is by definition not in the fleet, so the
/// audience is a character with no part in the call that produced it.
#[test]
fn an_invite_is_addressed_to_the_invitee_alone() {
    let (mut f, id) = with_alice();
    let (ok, e) = f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    assert!(ok);
    assert_eq!(e.pushes.len(), 1);
    assert_eq!(e.pushes[0].to, vec![BOB]);
    assert_eq!(events(&e), vec!["OnFleetInvite"]);
}

/// `OnFleetJoin`'s audience is everyone **except** the joiner: their own
/// client takes the `member.charID == session.charid` branch and re-reads
/// `GetInitState` instead (`fleetSvc.py:1359-1361`).
#[test]
fn joining_tells_the_existing_members_and_not_the_joiner() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    let (_, e) = f
        .accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    assert_eq!(events(&e), vec!["OnFleetJoin"]);
    assert_eq!(e.pushes[0].to, vec![ALICE], "not Bob, who re-reads instead");
    // And Bob's session has to be told, because `AcceptInvite` runs
    // inside a selective session change that is waiting for it.
    let bob = e
        .sessions
        .iter()
        .find(|s| s.character_id == BOB)
        .expect("Bob's session change");
    assert_eq!(bob.fleet_id, Some(id));
    assert_eq!(bob.fleet_role, Some(consts::ROLE_MEMBER));
}

#[test]
fn a_pilot_already_in_a_fleet_cannot_be_invited() {
    let (mut f, id) = with_alice_and_bob();
    let err = f
        .invite(ALICE, id, BOB, None, None, None)
        .expect_err("already in");
    assert_eq!(err, FleetError::Generic);
}

/// A wing commander may invite into his own wing, or with no seat named,
/// but not into another wing's squad.
#[test]
fn a_wing_commander_invites_only_into_his_own_wing() {
    let (mut f, id) = with_alice_and_bob();
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    f.move_member(
        ALICE,
        id,
        BOB,
        Some(wing),
        None,
        Some(consts::ROLE_WING_CMDR),
    )
    .expect("Bob commands the wing");
    let (other, _) = f.create_wing(ALICE, id).expect("second wing");
    let (far, _) = f.create_squad(ALICE, id, other).expect("its squad");
    assert_eq!(
        f.invite(
            BOB,
            id,
            CAROL,
            Some(other),
            Some(far),
            Some(consts::ROLE_MEMBER)
        )
        .expect_err("not his wing"),
        FleetError::NotCommanderOrBoss
    );
    assert!(f.invite_of(CAROL).is_none());
    f.invite(BOB, id, CAROL, Some(wing), None, Some(consts::ROLE_MEMBER))
        .expect("his own wing");
    f.invite(BOB, id, CAROL, None, None, None)
        .expect("no seat named");
}

/// A squad commander may not invite a pilot into a commander's seat.
#[test]
fn a_squad_commander_cannot_invite_a_commander() {
    let (mut f, id) = with_alice_and_bob();
    f.move_member(ALICE, id, BOB, None, None, Some(consts::ROLE_SQUAD_CMDR))
        .expect("Bob commands his squad");
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    assert_eq!(
        f.invite(
            BOB,
            id,
            CAROL,
            Some(wing),
            None,
            Some(consts::ROLE_WING_CMDR)
        )
        .expect_err("a wing seat"),
        FleetError::NotCommanderOrBoss
    );
    let squad = f.fleet(id).expect("fleet").members[&BOB].squad_id;
    f.invite(BOB, id, CAROL, None, Some(squad), Some(consts::ROLE_MEMBER))
        .expect("into his own squad");
}

/// The invite prompt is **not modal** (`OnFleetInvite:1344` passes
/// `modal = False`), so a player can form their own fleet and then click
/// Yes. Accepting then must not seat them twice.
#[test]
fn accepting_after_joining_another_fleet_is_refused_and_drops_the_invite() {
    let (mut f, alice_fleet) = with_alice();
    f.invite(ALICE, alice_fleet, BOB, None, None, None)
        .expect("invite");
    let bobs = f.create(BOB).expect("Bob makes his own");
    f.init(BOB, bobs, None, None, None).expect("init");
    let err = f
        .accept_invite(BOB, alice_fleet, None, None, None)
        .expect_err("refused");
    assert_eq!(err, FleetError::Generic);
    assert!(f.invite_of(BOB).is_none(), "the stale invite is dropped");
    assert_eq!(f.fleet_of(BOB).map(|x| x.fleet_id), Some(bobs));
}

#[test]
fn accepting_an_invite_for_the_wrong_fleet_is_refused() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    assert_eq!(
        f.accept_invite(BOB, id + 1, None, None, None)
            .expect_err("wrong fleet"),
        FleetError::NotFound
    );
}

#[test]
fn rejecting_names_the_inviter_and_consumes_the_invite() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    assert_eq!(f.reject_invite(BOB, id), Some(ALICE));
    assert!(f.invite_of(BOB).is_none());
    assert_eq!(f.reject_invite(BOB, id), None, "and only once");
}

/// `OnFleetLeave` goes to **everyone including the leaver**:
/// `_OnFleetLeave:1392-1393` branches on `charID == session.charid` and
/// calls `self.Clear()` there, so it is what empties the leaver's own
/// fleet window.
#[test]
fn leaving_tells_the_whole_fleet_including_the_leaver() {
    let (mut f, id) = with_alice_and_bob();
    let e = f.leave(BOB, id).expect("leave");
    assert_eq!(events(&e), vec!["OnFleetLeave"]);
    let mut to = e.pushes[0].to.clone();
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB]);
    assert!(f.fleet_of(BOB).is_none());
    assert_eq!(f.fleet(id).expect("still there").members.len(), 1);
}

/// A fleet of one that loses its member is disbanded, not left empty --
/// an empty fleet would keep answering `GetInitState`.
#[test]
fn the_last_member_leaving_disbands_the_fleet() {
    let (mut f, id) = with_alice();
    let e = f.leave(ALICE, id).expect("leave");
    assert_eq!(events(&e), vec!["OnFleetDisbanded"]);
    assert!(f.fleet(id).is_none());
    assert!(f.is_empty());
    assert!(f.fleet_of(ALICE).is_none());
}

/// `OnFleetDisbanded(charIDs)` carries **every** member, the actor
/// included: the handler loops and the actor's own branch is the one that
/// calls `Clear()`.
#[test]
fn disbanding_lists_every_member_including_the_boss() {
    let (mut f, id) = with_alice_and_bob();
    let e = f.disband(ALICE, id).expect("disband");
    assert_eq!(e.pushes.len(), 1);
    let FleetEvent::Disbanded { char_ids } = &e.pushes[0].event else {
        panic!("expected OnFleetDisbanded, got {:?}", e.pushes[0].event);
    };
    let mut ids = char_ids.clone();
    ids.sort_unstable();
    assert_eq!(ids, vec![ALICE, BOB]);
    // And every one of them is told.
    let mut to = e.pushes[0].to.clone();
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB]);
    // Both sessions are cleared.
    assert_eq!(e.sessions.len(), 2);
    assert!(e.sessions.iter().all(|s| s.fleet_id.is_none()));
}

#[test]
fn only_the_boss_may_disband() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.disband(BOB, id).expect_err("not boss"),
        FleetError::NotCreator
    );
    assert!(f.fleet(id).is_some());
}

#[test]
fn disbanding_drops_pending_invites_into_that_fleet() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    f.disband(ALICE, id).expect("disband");
    assert!(f.invite_of(BOB).is_none());
}

/// The boss may still remove **themselves**, which is leaving by another
/// name and is what the client's own menu offers.
#[test]
fn the_boss_may_kick_themselves() {
    let (mut f, id) = with_alice_and_bob();
    let (ok, _) = f.kick(ALICE, id, ALICE).expect("boss removes themselves");
    assert!(ok);
    assert!(f.fleet_of(ALICE).is_none());
    assert!(f.fleet(id).expect("fleet").members[&BOB].is_boss());
}

/// **`job` and `role` are different axes, and `MakeLeader` moves only
/// `job`.**
///
/// This test exists because getting it wrong looks like a bug in either
/// direction. `fleetbroadcastexports.GetRankName:245-257` **[V]** reads
/// `member.job & fleetJobCreator` *first* and only then switches on
/// `member.role`, so "the boss" and "the fleet commander" are genuinely
/// separate: a squad member can be the boss, and the pilot sitting in the
/// commander's seat can be nobody's boss. That is EVE's model, not an
/// artefact of ours -- the client has four distinct `*Boss` rank labels
/// precisely so it can render the combinations.
///
/// So after `MakeLeader(BOB)`: Bob has the boss bit and Alice keeps
/// `ROLE_LEADER`, which still makes her a commander for authority
/// purposes.
#[test]
fn make_leader_moves_the_boss_bit_and_leaves_the_commander_seat_alone() {
    let (mut f, id) = with_alice_and_bob();
    f.make_leader(ALICE, id, BOB).expect("Bob is boss");
    let fleet = f.fleet(id).expect("fleet");
    assert!(fleet.members[&BOB].is_boss());
    assert_eq!(fleet.members[&BOB].role, consts::ROLE_MEMBER);
    assert!(!fleet.members[&ALICE].is_boss());
    assert_eq!(
        fleet.members[&ALICE].role,
        consts::ROLE_LEADER,
        "the seat did not move, only the boss bit"
    );
    // Alice is therefore still a commander, and is refused on the
    // specific ground that Bob is the boss -- not on authority.
    assert_eq!(
        f.kick(ALICE, id, BOB).expect_err("cannot kick the boss"),
        FleetError::CantKickBoss
    );
}

/// A pilot who is neither boss nor commander cannot kick at all, which is
/// the *other* refusal and must not be confused with `CantKickBoss`.
#[test]
fn a_plain_member_cannot_kick_anyone() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.fleet(id).expect("fleet").members[&BOB].role,
        consts::ROLE_MEMBER
    );
    assert_eq!(
        f.kick(BOB, id, ALICE).expect_err("no authority"),
        FleetError::NotCommanderOrBoss
    );
}

#[test]
fn kicking_a_non_member_is_a_no_op_not_an_error() {
    let (mut f, id) = with_alice_and_bob();
    let (changed, e) = f.kick(ALICE, id, CAROL).expect("no-op");
    assert!(!changed);
    assert!(e.pushes.is_empty());
}

/// Their `kickMember` checks membership and nothing else, so the boss can
/// be kicked out of their own fleet. Ours refuses with the dialog this
/// build actually ships.
#[test]
fn kicking_the_boss_as_someone_else_is_refused() {
    let (mut f, id) = with_alice_and_bob();
    f.move_member(ALICE, id, BOB, None, None, Some(consts::ROLE_SQUAD_CMDR))
        .expect("Bob commands his squad");
    assert_eq!(
        f.kick(BOB, id, ALICE).expect_err("cannot kick the boss"),
        FleetError::CantKickBoss
    );
}

/// The boss leaving a fleet that still has members hands the job on
/// deterministically, and that is reported as a member change -- which is
/// what writes "is now the fleet boss" into the client's member history.
#[test]
fn the_boss_leaving_hands_the_boss_bit_to_the_one_left() {
    let (mut f, id) = with_alice_and_bob();
    let e = f.leave(ALICE, id).expect("leave");
    assert_eq!(events(&e), vec!["OnFleetLeave", "OnFleetMemberChanged"]);
    assert!(f.fleet(id).expect("fleet").members[&BOB].is_boss());
    let FleetEvent::MemberChanged {
        char_id, old, new, ..
    } = &e.pushes[1].event
    else {
        panic!("expected a member change");
    };
    assert_eq!(*char_id, BOB);
    assert_eq!(old.job & consts::JOB_CREATOR, 0);
    assert_ne!(new.job & consts::JOB_CREATOR, 0);
}

#[test]
fn make_leader_moves_the_boss_bit_and_reports_both_members() {
    let (mut f, id) = with_alice_and_bob();
    let (ok, e) = f.make_leader(ALICE, id, BOB).expect("make leader");
    assert!(ok);
    assert_eq!(
        events(&e),
        vec!["OnFleetMemberChanged", "OnFleetMemberChanged"],
        "one notification per character, because it describes one"
    );
    let fleet = f.fleet(id).expect("fleet");
    assert!(fleet.members[&BOB].is_boss());
    assert!(!fleet.members[&ALICE].is_boss());
}

#[test]
fn make_leader_on_the_current_boss_changes_nothing() {
    let (mut f, id) = with_alice_and_bob();
    let (ok, e) = f.make_leader(ALICE, id, ALICE).expect("no-op");
    assert!(ok);
    assert!(e.pushes.is_empty());
}

// ---------------------------------------------------- roles and placement

#[test]
fn a_second_fleet_commander_is_refused() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.move_member(ALICE, id, BOB, None, None, Some(consts::ROLE_LEADER))
            .expect_err("one commander"),
        FleetError::Generic
    );
}

#[test]
fn a_second_wing_commander_in_one_wing_is_refused() {
    let (mut f, id) = with_alice_and_bob();
    f.invite(ALICE, id, CAROL, None, None, None)
        .expect("invite");
    f.accept_invite(CAROL, id, None, None, None)
        .expect("accept");
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    f.move_member(
        ALICE,
        id,
        BOB,
        Some(wing),
        None,
        Some(consts::ROLE_WING_CMDR),
    )
    .expect("Bob commands the wing");
    assert_eq!(
        f.move_member(
            ALICE,
            id,
            CAROL,
            Some(wing),
            None,
            Some(consts::ROLE_WING_CMDR)
        )
        .expect_err("one per wing"),
        FleetError::Generic
    );
}

#[test]
fn a_wing_commander_has_a_wing_and_no_squad() {
    let (mut f, id) = with_alice_and_bob();
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    f.move_member(
        ALICE,
        id,
        BOB,
        Some(wing),
        None,
        Some(consts::ROLE_WING_CMDR),
    )
    .expect("promote");
    let bob = &f.fleet(id).expect("fleet").members[&BOB];
    assert_eq!(bob.wing_id, wing);
    assert_eq!(bob.squad_id, consts::NONE_ID);
}

#[test]
fn moving_a_member_nowhere_reports_nothing() {
    let (mut f, id) = with_alice_and_bob();
    let (changed, e) = f
        .move_member(ALICE, id, BOB, None, None, None)
        .expect("move");
    assert!(!changed, "nothing moved");
    assert!(
        e.pushes.is_empty(),
        "a push here writes a history line for a change that did not happen"
    );
}

#[test]
fn a_plain_member_cannot_move_anyone() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.move_member(BOB, id, BOB, None, None, Some(consts::ROLE_SQUAD_CMDR))
            .expect_err("not a commander"),
        FleetError::NotCommanderOrBoss
    );
}

/// `isFreeMove` is the option that lets a member reseat themselves, which
/// is the only thing it is for.
#[test]
fn free_move_lets_a_member_move_themselves() {
    let (mut f, id) = with_alice_and_bob();
    let wing = f.create_wing(ALICE, id).expect("wing").0;
    let squad = f.create_squad(ALICE, id, wing).expect("squad").0;
    assert_eq!(
        f.move_member(BOB, id, BOB, Some(wing), Some(squad), None)
            .expect_err("not yet"),
        FleetError::NotCommanderOrBoss
    );
    f.set_options(ALICE, id, Some(true), None).expect("options");
    let (changed, _) = f
        .move_member(BOB, id, BOB, Some(wing), Some(squad), None)
        .expect("free move");
    assert!(changed);
    assert_eq!(f.fleet(id).expect("fleet").members[&BOB].squad_id, squad);
}

#[test]
fn a_squad_that_does_not_exist_is_refused() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.move_member(ALICE, id, BOB, None, Some(999_999), None)
            .expect_err("no such squad"),
        FleetError::NoPositionFound
    );
}

/// A negative id that is not the `-1` sentinel is a client bug, not
/// "unspecified".
#[test]
fn a_negative_position_that_is_not_the_sentinel_is_refused() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.move_member(ALICE, id, BOB, Some(-7), None, None)
            .expect_err("bad id"),
        FleetError::NoPositionFound
    );
    // ...and -1 *is* "unspecified", so this one is accepted.
    f.move_member(ALICE, id, BOB, Some(consts::NONE_ID), None, None)
        .expect("the sentinel means unspecified");
}

// ------------------------------------------------------- wings and squads

#[test]
fn creating_a_wing_returns_its_id_and_tells_the_fleet() {
    let (mut f, id) = with_alice_and_bob();
    let (wing, e) = f.create_wing(ALICE, id).expect("wing");
    assert_eq!(events(&e), vec!["OnFleetWingAdded"]);
    let mut to = e.pushes[0].to.clone();
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB]);
    assert!(f.fleet(id).expect("fleet").wings.contains_key(&wing));
}

/// The wing menu, which holds "create squad" and "rename wing", is offered
/// to the boss, the fleet commander and wing commanders only
/// (`ui/shared/fleet/fleet.py:916-917`); a squad commander has no such entry.
#[test]
fn only_the_boss_may_create_a_wing_but_a_wing_commander_may_create_a_squad() {
    let (mut f, id) = with_alice_and_bob();
    f.invite(ALICE, id, CAROL, None, None, None)
        .expect("invite");
    f.accept_invite(CAROL, id, None, None, None)
        .expect("accept");
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    f.move_member(
        ALICE,
        id,
        BOB,
        Some(wing),
        None,
        Some(consts::ROLE_WING_CMDR),
    )
    .expect("Bob commands the wing");
    f.move_member(ALICE, id, CAROL, None, None, Some(consts::ROLE_SQUAD_CMDR))
        .expect("Carol commands her squad");
    assert_eq!(
        f.create_wing(BOB, id).expect_err("boss only"),
        FleetError::NotCreator
    );
    f.create_squad(BOB, id, wing)
        .expect("a wing commander organises his wing");
    f.change_wing_name(BOB, id, wing, "Bobs")
        .expect("and names it");
    assert_eq!(
        f.create_squad(CAROL, id, wing)
            .expect_err("a squad commander has no wing menu"),
        FleetError::NotCommanderOrBoss
    );
    assert_eq!(
        f.change_wing_name(CAROL, id, wing, "Carols")
            .expect_err("nor renames the wing"),
        FleetError::NotCommanderOrBoss
    );
    let (other, _) = f.create_wing(ALICE, id).expect("second wing");
    assert_eq!(
        f.create_squad(BOB, id, other).expect_err("not his wing"),
        FleetError::NotCommanderOrBoss
    );
}

#[test]
fn a_wing_with_members_in_it_cannot_be_deleted() {
    let (mut f, id) = with_alice_and_bob();
    let wing = f.fleet(id).expect("fleet").members[&BOB].wing_id;
    assert_eq!(
        f.delete_wing(ALICE, id, wing).expect_err("not empty"),
        FleetError::CannotDeleteNonEmptyWing
    );
}

#[test]
fn a_squad_with_members_in_it_cannot_be_deleted() {
    let (mut f, id) = with_alice_and_bob();
    let squad = f.fleet(id).expect("fleet").members[&BOB].squad_id;
    assert_eq!(
        f.delete_squad(ALICE, id, squad).expect_err("not empty"),
        FleetError::CannotDeleteNonEmptySquad
    );
}

#[test]
fn deleting_a_wing_clears_an_auto_join_squad_inside_it() {
    let (mut f, id) = with_alice();
    let wing = f.create_wing(ALICE, id).expect("wing").0;
    let squad = f.create_squad(ALICE, id, wing).expect("squad").0;
    f.set_options(ALICE, id, None, Some(Some(squad)))
        .expect("auto-join");
    assert_eq!(
        f.fleet(id).expect("fleet").options.auto_join_squad_id,
        Some(squad)
    );
    let e = f.delete_wing(ALICE, id, wing).expect("delete");
    assert_eq!(
        f.fleet(id).expect("fleet").options.auto_join_squad_id,
        None,
        "a dangling default seat would be a placement nothing can resolve"
    );
    assert!(
        events(&e).contains(&"OnFleetOptionsChanged"),
        "the client sends its options back whole, so it must hear of the clear"
    );
}

#[test]
fn deleting_the_auto_join_squad_tells_the_fleet_its_options_changed() {
    let (mut f, id) = with_alice();
    let wing = f.create_wing(ALICE, id).expect("wing").0;
    let squad = f.create_squad(ALICE, id, wing).expect("squad").0;
    f.set_options(ALICE, id, None, Some(Some(squad)))
        .expect("auto-join");
    let e = f.delete_squad(ALICE, id, squad).expect("delete");
    assert_eq!(
        events(&e),
        vec!["OnFleetSquadDeleted", "OnFleetOptionsChanged"]
    );
    let other = f.create_squad(ALICE, id, wing).expect("squad").0;
    let e = f.delete_squad(ALICE, id, other).expect("delete");
    assert_eq!(
        events(&e),
        vec!["OnFleetSquadDeleted"],
        "a squad that was not the default leaves the options alone"
    );
}

/// A real change to the options is told to every member as
/// `OnFleetOptionsChanged(old, new)`, the only way the client learns its
/// options after `InitFleet` (`fleetSvc.py:1254-1265`).
#[test]
fn an_options_change_is_told_to_the_fleet_with_old_and_new() {
    let (mut f, id) = with_alice_and_bob();
    let (now, e) = f.set_options(ALICE, id, Some(true), None).expect("options");
    assert!(now.is_free_move);
    assert_eq!(events(&e), vec!["OnFleetOptionsChanged"]);
    let mut to = e.pushes[0].to.clone();
    to.sort_unstable();
    assert_eq!(to, vec![ALICE, BOB]);
    assert_eq!(
        e.pushes[0].event,
        FleetEvent::OptionsChanged {
            old: Options::default(),
            new: now,
        }
    );
}

#[test]
fn an_auto_join_squad_that_does_not_exist_is_refused() {
    let (mut f, id) = with_alice();
    assert_eq!(
        f.set_options(ALICE, id, None, Some(Some(424_242)))
            .expect_err("no such squad"),
        FleetError::NoPositionFound
    );
}

#[test]
fn the_wing_and_squad_limits_are_the_clients_own() {
    let (mut f, id) = with_alice();
    // One wing already exists, so 24 more reaches the cap of 25.
    for _ in 1..consts::MAX_WINGS_IN_FLEET {
        f.create_wing(ALICE, id).expect("wing");
    }
    assert_eq!(
        f.create_wing(ALICE, id).expect_err("full"),
        FleetError::TooManyWings(25),
        "and it carries the `num` loc arg the dialog takes"
    );
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    // That wing has one squad from `create`.
    for _ in 1..consts::MAX_SQUADS_IN_WING {
        f.create_squad(ALICE, id, wing).expect("squad");
    }
    assert_eq!(
        f.create_squad(ALICE, id, wing).expect_err("full"),
        FleetError::TooManySquads(25)
    );
}

/// Ten **characters**, not ten bytes: truncating UTF-8 at a byte index
/// can split a code point and produce a name the marshal cannot encode.
#[test]
fn names_are_truncated_to_ten_characters() {
    let (mut f, id) = with_alice();
    let wing = *f
        .fleet(id)
        .expect("fleet")
        .wings
        .keys()
        .next()
        .expect("wing");
    f.change_wing_name(ALICE, id, wing, "Alpha Wing Of Doom")
        .expect("rename");
    assert_eq!(f.fleet(id).expect("fleet").wings[&wing].name, "Alpha Wing");
    f.change_wing_name(ALICE, id, wing, "ööööööööööööö")
        .expect("rename");
    let name = &f.fleet(id).expect("fleet").wings[&wing].name;
    assert_eq!(name.chars().count(), 10);
    assert_eq!(name.len(), 20, "and it is still valid UTF-8");
}

/// **No wing or squad id repeats across fleets.** `DeleteSquad(squadID)`
/// names no wing, so one id must find one squad on the node. See
/// `Fleets::next_group_id`.
#[test]
fn no_two_wings_or_squads_anywhere_share_an_id() {
    let mut f = Fleets::new(NODE);
    let mut seen: Vec<i64> = Vec::new();
    for (n, who) in [ALICE, BOB, CAROL].iter().enumerate() {
        let id = f.create(*who).expect("create");
        f.init(*who, id, None, None, None).expect("init");
        for _ in 0..4 {
            let (wing, _) = f.create_wing(*who, id).expect("wing");
            for _ in 0..4 {
                let (squad, _) = f.create_squad(*who, id, wing).expect("squad");
                seen.push(squad);
            }
            seen.push(wing);
        }
        seen.push(id);
        assert_eq!(f.len(), n + 1);
    }
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "a wing and a squad in different fleets collided: {seen:?}"
    );
}

// ------------------------------------------------------------ the payload

/// **The arity of every payload this module can emit**, against the
/// argcounts read off the client's own handlers this session.
///
/// This is the test that replaces a human watching: a notification with
/// the wrong arity raises `TypeError` inside `BroadcastStuff.NotifyUp`,
/// where `ExceptionWrapper.NotifyUp` swallows it -- no traceback on the
/// client, nothing in our log, and a fleet window that is quietly wrong.
#[test]
fn every_payload_has_the_arity_the_client_declares() {
    let member = Member {
        char_id: ALICE,
        wing_id: 1,
        squad_id: 2,
        role: consts::ROLE_MEMBER,
        job: consts::JOB_NONE,
        skills: [0, 0, 0],
        timestamp: 1,
        station_id: None,
        ship_type_id: None,
        solar_system_id: None,
        opt_outs: MemberOptOuts::default(),
    };
    let slot = member.slot();
    // (event, arity, where the arity comes from)
    let cases: Vec<(FleetEvent, usize, &str)> = vec![
        (
            FleetEvent::Join {
                member: member.clone(),
            },
            1,
            "fleetSvc.py:1358 argcount 2",
        ),
        (
            FleetEvent::Leave { char_id: ALICE },
            1,
            "fleetSvc.py:1385 argcount 2",
        ),
        (
            FleetEvent::MemberChanged {
                char_id: ALICE,
                fleet_id: 7,
                old: slot,
                new: slot,
                is_only_member: false,
            },
            // **13, not 12.** `OnFleetMemberChanged` is argcount 14;
            // the client-emitted `_Local` twin takes twelve and must not
            // be copied.
            13,
            "fleetSvc.py:1421 argcount 14",
        ),
        (FleetEvent::WingAdded { wing_id: 1 }, 1, "fleetSvc.py:1477"),
        (FleetEvent::WingDeleted { wing_id: 1 }, 1, "TSV"),
        (
            FleetEvent::SquadAdded {
                wing_id: 1,
                squad_id: 2,
            },
            2,
            "fleetSvc.py:1487",
        ),
        (
            FleetEvent::SquadDeleted { squad_id: 2 },
            1,
            "TSV; asymmetric",
        ),
        (
            FleetEvent::WingNameChanged {
                wing_id: 1,
                name: "w".to_owned(),
            },
            2,
            "TSV",
        ),
        (
            FleetEvent::SquadNameChanged {
                squad_id: 2,
                name: "s".to_owned(),
            },
            2,
            "TSV",
        ),
        (
            FleetEvent::Invite {
                fleet_id: 7,
                inviter_id: ALICE,
                msg_name: INVITE_DIALOG.to_owned(),
                auto_accept: false,
            },
            4,
            "fleetSvc.py:1326 argcount 5",
        ),
        (
            FleetEvent::MotdChanged {
                motd: "hi".to_owned(),
                reload_motd: true,
            },
            2,
            "fleetSvc.py:2229",
        ),
        (
            FleetEvent::OptionsChanged {
                old: Options::default(),
                new: Options::default(),
            },
            2,
            "fleetSvc.py:1254",
        ),
        (
            FleetEvent::Disbanded {
                char_ids: vec![ALICE],
            },
            1,
            "fleetSvc.py:1417 argcount 2",
        ),
        (
            FleetEvent::BeaconChange {
                char_id: ALICE,
                solar_system_id: 2,
                beacon_id: 3,
                type_id: 4,
                active: true,
            },
            5,
            "fleetSvc.py:258 argcount 6",
        ),
        (
            FleetEvent::BridgeChange {
                ship_id: 1,
                solar_system_id: 2,
                beacon_id: 3,
                active: true,
            },
            4,
            "fleetSvc.py:196 argcount 5",
        ),
    ];
    for (event, arity, why) in cases {
        let (name, args) = event.to_push();
        assert_eq!(args.len(), arity, "{name} takes {arity} ({why})");
        assert!(
            name.starts_with("On"),
            "{name}: ScatterEventWithoutTheStars logs two errors and a \
             traceback for anything that does not start with On, and the \
             Do*/Process* prefixes are handled differently"
        );
    }
}

/// `OnFleetDisbanded`'s one argument is **iterated**, so it is a list
/// even for a fleet of one. A bare integer raises inside `NotifyUp`.
#[test]
fn disbanded_sends_a_list_even_for_one_character() {
    let (name, args) = FleetEvent::Disbanded {
        char_ids: vec![ALICE],
    }
    .to_push();
    assert_eq!(name, "OnFleetDisbanded");
    assert!(matches!(&args[0], PyValue::List(ids) if ids.len() == 1));
}

/// `msgDict.get('autoAccept', False)` is the handler's first line, so it
/// must be a **mapping** -- and a marshal dict's entries are
/// `(value, key)`, which is the inversion `pypairs` writes down once.
#[test]
fn the_invite_message_dict_is_a_real_dict_keyed_the_right_way_round() {
    let (_, args) = FleetEvent::Invite {
        fleet_id: 7,
        inviter_id: ALICE,
        msg_name: INVITE_DIALOG.to_owned(),
        auto_accept: false,
    }
    .to_push();
    // The third argument is the FSD dialog name, as a Python 2 `str`.
    assert_eq!(args[2], crate::pyutil::bstr("AskJoinFleet"));
    let PyValue::Dict(entries) = &args[3] else {
        panic!("msgDict must be a dict, because :1343 subscript-assigns into it");
    };
    // `(value, key)`: the *second* element is the key.
    let keys: Vec<&PyValue> = entries.iter().map(|(_, k)| k).collect();
    assert!(
        keys.contains(&&crate::pyutil::bstr("autoAccept")),
        "got {entries:?} -- if the pair is the obvious way round this is \
         the dict {{'autoAccept': False}} inverted, and `.get` misses"
    );
    // And the second argument is the **inviter's character id**, which
    // the client hands to `evelink.character_link`.
    assert_eq!(args[1], PyValue::Int(ALICE));
}

/// The five attributes `fleetSvc.InitFleet:306-310` reads, and only
/// those: no `isLootLogging` (absent from this build entirely) and no
/// top-level `squads` (never read).
#[test]
fn the_init_state_carries_exactly_the_five_attributes_the_client_reads() {
    let (f, id) = with_alice_and_bob();
    let state = f.fleet(id).expect("fleet").init_state();
    assert_eq!(
        attrs(&state),
        vec!["fleetID", "members", "motd", "options", "wings"],
        "InitFleet reads exactly these five"
    );
}

/// `cfg.eveowners.Prime(self.members.keys())` -- so `members` must be a
/// mapping whose **keys are character ids**. With the marshal dict's
/// `(value, key)` pairs written the obvious way round, the keys would be
/// the member `KeyVal`s and the primer would try to resolve those as
/// owner ids.
#[test]
fn members_is_keyed_by_character_id() {
    let (f, id) = with_alice_and_bob();
    let state = f.fleet(id).expect("fleet").init_state();
    let members = attr(&state, "members").expect("members");
    let PyValue::Dict(entries) = members else {
        panic!("members must be a dict: {members:?}");
    };
    let mut keys: Vec<i64> = entries
        .iter()
        .map(|(_, k)| match k {
            PyValue::Int(n) => *n,
            other => panic!("a member key is not an id: {other:?}"),
        })
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec![ALICE, BOB]);
}

/// `wing.squads.keys()` / `.iteritems()` / `.get(squadID)` -- a mapping,
/// keyed by squad id, nested inside its wing.
#[test]
fn wings_hold_their_squads_as_an_id_keyed_mapping() {
    let (f, id) = with_alice();
    let wings = f.fleet(id).expect("fleet").wings_pyvalue();
    let PyValue::Dict(wing_entries) = &wings else {
        panic!("wings must be a dict");
    };
    assert_eq!(wing_entries.len(), 1);
    let (wing, wing_key) = &wing_entries[0];
    assert!(matches!(wing_key, PyValue::Int(_)), "keyed by wing id");
    let squads = attr(wing, "squads").expect("squads");
    let PyValue::Dict(squad_entries) = squads else {
        panic!("wing.squads must be a dict");
    };
    assert_eq!(squad_entries.len(), 1);
    assert!(matches!(squad_entries[0].1, PyValue::Int(_)));
    // And both carry a name, which `GetFleetHierarchy` reads off each.
    assert!(attr(wing, "name").is_some());
    assert!(attr(&squad_entries[0].0, "name").is_some());
}

/// The eleven keys the client reads off a member, and `clientID` is not
/// among them -- a sweep of every `LOAD_ATTR` in the fleet UI counts zero
/// reads of it.
#[test]
fn a_member_carries_the_keys_the_client_reads_and_not_client_id() {
    let (f, id) = with_alice();
    let member = f.fleet(id).expect("fleet").members[&ALICE].to_pyvalue();
    for key in [
        "charID",
        "wingID",
        "squadID",
        "role",
        "job",
        "skills",
        "timestamp",
        "stationID",
        "shipTypeID",
        "solarSystemID",
        "memberOptOuts",
    ] {
        assert!(attr(&member, key).is_some(), "a member must carry {key}");
    }
    assert!(
        attr(&member, "clientID").is_none(),
        "read by no fleet code in this build, and wrong for a character \
         with two sessions"
    );
    // The three opt-outs, from `evefleet/members.pyc`'s own constructor,
    // and **exactly** those three.
    let opt = attr(&member, "memberOptOuts").expect("optOuts");
    assert_eq!(
        attrs(opt),
        vec![
            "acceptsConduitJumps",
            "acceptsFleetRegroups",
            "acceptsFleetWarp"
        ]
    );
}

/// A member's `timestamp` has to unmarshal as a Python `long`, which our
/// encoder decides **by magnitude**. Anything under 2^31 is formatted by
/// `FormatDateTime`'s else branch: a blank field, no exception, nothing
/// in our log.
#[test]
fn a_members_timestamp_is_a_real_filetime() {
    let (f, id) = with_alice();
    let ts = f.fleet(id).expect("fleet").members[&ALICE].timestamp;
    assert!(
        ts > i64::from(i32::MAX),
        "{ts} is small enough to arrive as a Python int, which \
         FormatDateTime renders as a blank"
    );
}

// ------------------------------------------------------------- disconnect

const T0: i64 = 1_000_000_000_000;

#[test]
fn a_disconnect_keeps_the_place_and_tells_nobody_yet() {
    let (mut f, id) = with_alice_and_bob();
    let e = f.character_disconnected(BOB, T0);
    assert_eq!(e, Effects::default());
    assert!(f.fleet_of(BOB).is_some(), "the seat is held");
    assert!(f.has_gone());
    assert_eq!(f.fleet(id).unwrap().audience(), vec![ALICE]);
    // Nothing is due inside the window.
    assert_eq!(f.expire(T0 + RECONNECT_TIMEOUT - 1), Effects::default());
    assert!(f.fleet_of(BOB).is_some());
}

#[test]
fn a_disconnect_removes_the_member_at_the_deadline_and_tells_the_rest() {
    let (mut f, _id) = with_alice_and_bob();
    f.character_disconnected(BOB, T0);
    let e = f.expire(T0 + RECONNECT_TIMEOUT);
    assert_eq!(events(&e), vec!["OnFleetLeave"]);
    assert_eq!(
        e.pushes[0].to,
        vec![ALICE],
        "the departing character has no socket to write to"
    );
    assert!(
        e.sessions.iter().all(|s| s.character_id != BOB),
        "a session change on a closing socket is a race, not a courtesy"
    );
    assert!(f.fleet_of(BOB).is_none());
    assert!(!f.has_gone());
}

#[test]
fn the_last_member_disconnecting_disbands_the_fleet_at_the_deadline() {
    let (mut f, id) = with_alice();
    f.character_disconnected(ALICE, T0);
    assert!(f.fleet(id).is_some());
    let e = f.expire(T0 + RECONNECT_TIMEOUT);
    // Nobody live is left to tell; the record is gone all the same.
    assert!(e.pushes.is_empty());
    assert!(f.fleet(id).is_none());
    assert!(f.fleet_of(ALICE).is_none());
}

#[test]
fn a_gone_boss_is_replaced_at_the_deadline() {
    let (mut f, _id) = with_alice_and_bob();
    f.character_disconnected(ALICE, T0);
    let e = f.expire(T0 + RECONNECT_TIMEOUT);
    assert_eq!(events(&e), vec!["OnFleetLeave", "OnFleetMemberChanged"]);
    assert!(f.fleet_of(BOB).unwrap().members[&BOB].is_boss());
}

#[test]
fn a_reconnect_inside_the_window_keeps_the_seat_and_resends_the_session() {
    let (mut f, id) = with_alice_and_bob();
    f.character_disconnected(BOB, T0);
    let e = f.reconnect(BOB, id).expect("a member");
    assert_eq!(e.sessions.len(), 1);
    assert_eq!(e.sessions[0].character_id, BOB);
    assert_eq!(e.sessions[0].fleet_id, Some(id));
    assert!(!f.has_gone());
    // The deadline passes with nobody left to remove.
    assert_eq!(f.expire(T0 + RECONNECT_TIMEOUT), Effects::default());
    assert!(f.fleet_of(BOB).is_some());
    assert_eq!(f.fleet(id).unwrap().audience(), vec![ALICE, BOB]);
}

#[test]
fn a_reconnect_replays_the_fields_and_tags_to_the_returning_pilot() {
    let (mut f, id) = with_alice_and_bob();
    f.light_beacon(ALICE, BEACON);
    f.tag(ALICE, 7_001, Some("1")).expect("tag");
    f.character_disconnected(BOB, T0);
    let e = f.reconnect(BOB, id).expect("a member");
    assert_eq!(
        events(&e),
        vec!["OnFleetJumpBeaconModuleChange", "OnFleetStateChange"]
    );
    assert!(e.pushes.iter().all(|p| p.to == vec![BOB]));
}

#[test]
fn a_reconnect_by_a_stranger_or_to_another_fleet_is_refused() {
    let (mut f, id) = with_alice();
    assert!(f.reconnect(BOB, id).is_none());
    assert!(f.reconnect(ALICE, id + 1).is_none());
}

#[test]
fn nothing_is_pushed_to_a_gone_member() {
    let (mut f, id) = with_alice_and_bob();
    f.character_disconnected(BOB, T0);
    let e = f.set_motd(ALICE, id, "form up").expect("motd");
    assert_eq!(e.pushes[0].to, vec![ALICE]);
}

#[test]
fn the_window_expires_a_gone_member_once_a_boss_is_alone() {
    // Both gone: the first expiry removes one, the last disbands.
    let (mut f, id) = with_alice_and_bob();
    f.character_disconnected(ALICE, T0);
    f.character_disconnected(BOB, T0 + 10);
    f.expire(T0 + RECONNECT_TIMEOUT);
    assert!(f.fleet(id).is_some(), "Bob's window is still open");
    f.expire(T0 + 10 + RECONNECT_TIMEOUT);
    assert!(f.fleet(id).is_none());
}

#[test]
fn a_disconnect_drops_a_pending_invite() {
    let (mut f, id) = with_alice();
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    f.character_disconnected(BOB, T0);
    assert!(f.invite_of(BOB).is_none());
}

#[test]
fn a_disconnect_by_someone_in_no_fleet_does_nothing() {
    let mut f = Fleets::new(NODE);
    assert_eq!(f.character_disconnected(ALICE, T0), Effects::default());
}

// --------------------------------------------------------------- the motd

#[test]
fn setting_the_motd_tells_the_fleet_to_reload_it() {
    let (mut f, id) = with_alice_and_bob();
    let e = f.set_motd(ALICE, id, "form up").expect("motd");
    assert_eq!(events(&e), vec!["OnFleetMotdChanged"]);
    let FleetEvent::MotdChanged { motd, reload_motd } = &e.pushes[0].event else {
        panic!("expected a motd change");
    };
    assert_eq!(motd, "form up");
    assert!(*reload_motd, "the text really did change");
    assert_eq!(f.fleet(id).expect("fleet").motd, "form up");
}

#[test]
fn only_the_boss_may_set_the_motd() {
    let (mut f, id) = with_alice_and_bob();
    assert_eq!(
        f.set_motd(BOB, id, "nope").expect_err("boss only"),
        FleetError::NotCreator
    );
}

// ------------------------------------------------------------- the errors

/// Every error key this module can raise exists in **this** build's
/// dialog corpus, and the three that take a `num` argument send one.
#[test]
fn every_error_names_a_dialog_this_build_ships() {
    // Keys confirmed against `findings/user-errors.tsv`, where each reads
    // `fsd+loc` or better in the `source` column.
    for e in [
        FleetError::NotFound,
        FleetError::NotInFleet,
        FleetError::NotCreator,
        FleetError::NotCommanderOrBoss,
        FleetError::NoPositionFound,
        FleetError::CannotDeleteNonEmptyWing,
        FleetError::CannotDeleteNonEmptySquad,
        FleetError::CantKickBoss,
        FleetError::Generic,
    ] {
        assert!(e.key().starts_with("Fleet"), "{:?}", e.key());
        assert!(e.args().is_empty(), "{:?} takes no loc args", e.key());
    }
    for e in [
        FleetError::TooManyMembers(256),
        FleetError::TooManyWings(25),
        FleetError::TooManySquads(25),
    ] {
        // `num`: the dialog text substitutes `{num}`, so without it the
        // player would see the placeholder.
        let args = e.args();
        assert_eq!(args.len(), 1, "{} takes a num", e.key());
        assert_eq!(args[0].1, crate::pyutil::bstr("num"));
    }
}

#[test]
fn an_error_marshals_as_a_user_error() {
    let v = FleetError::NotInFleet.to_user_error();
    assert!(
        matches!(&v, PyValue::ObjectEx { .. }),
        "a UserError is an ObjectEx: {v:?}"
    );
}

// ------------------------------------------------ nothing is persisted

/// A standing claim worth a test, because it is the kind of thing a later
/// round "fixes" by adding fleets to the snapshot. A fleet is a session
/// artefact: `docs/reference/world-state.md` §2 lists presence as state a restart
/// must rebuild rather than restore, and a stored fleet is a fleet of
/// people who are not there.
#[test]
fn fleets_hold_nothing_a_restart_should_restore() {
    let (f, _id) = with_alice_and_bob();
    // The only honest assertion available: a fresh `Fleets` is empty, so
    // a restart starts with no fleets no matter what the old one held.
    assert!(!f.is_empty());
    assert!(Fleets::new(NODE).is_empty());
}

#[test]
fn the_stage_removes_an_expired_member_through_the_registry() {
    use crate::service::Registry;
    let mut w = Registry::new(crate::model::SandboxConfig::default());
    let id = w.fleets_mut().create(ALICE).expect("create");
    w.fleets_mut()
        .init(ALICE, id, Some(SHIP_TYPE), Some(STATION), Some(SYSTEM))
        .expect("init");
    w.fleets_mut()
        .invite(ALICE, id, BOB, None, None, None)
        .expect("invite");
    w.fleets_mut()
        .accept_invite(BOB, id, Some(SHIP_TYPE), Some(STATION), Some(SYSTEM))
        .expect("accept");
    w.fleets_mut().character_disconnected(BOB, T0);
    tick(&mut w, std::time::Instant::now(), T0 + 1);
    assert!(w.fleets().fleet_of(BOB).is_some());
    tick(&mut w, std::time::Instant::now(), T0 + RECONNECT_TIMEOUT);
    assert!(w.fleets().fleet_of(BOB).is_none());
    assert!(w.fleets().fleet_of(ALICE).is_some());
}

const ROCK: i64 = 7_001;
const WRECK: i64 = 7_002;

#[test]
fn a_commander_tags_and_every_member_is_told() {
    let (mut f, _) = with_alice_and_bob();
    let e = f.tag(ALICE, ROCK, Some("1")).expect("tag");
    assert_eq!(events(&e), vec!["OnFleetStateChange"]);
    let mut to = e.pushes[0].to.clone();
    to.sort();
    assert_eq!(to, vec![ALICE, BOB]);
    assert_eq!(f.fleet_of(BOB).unwrap().target_tags[&ROCK], "1");
    // The same letter on another item moves.
    f.tag(ALICE, WRECK, Some("1")).expect("tag");
    let tags = &f.fleet_of(ALICE).unwrap().target_tags;
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[&WRECK], "1");
    assert_eq!(
        f.tag(ALICE, ROCK, Some("K")).unwrap_err(),
        FleetError::Generic
    );
}

#[test]
fn a_member_cannot_tag() {
    let (mut f, _) = with_alice_and_bob();
    assert_eq!(
        f.tag(BOB, ROCK, Some("1")).unwrap_err(),
        FleetError::NotCommanderOrBoss
    );
    assert_eq!(
        f.tag(CAROL, ROCK, Some("1")).unwrap_err(),
        FleetError::NotInFleet
    );
}

#[test]
fn untag_with_none_removes_it() {
    let (mut f, _) = with_alice_and_bob();
    f.tag(ALICE, ROCK, Some("X")).expect("tag");
    let e = f.tag(ALICE, ROCK, None).expect("untag");
    assert_eq!(events(&e), vec!["OnFleetStateChange"]);
    assert!(f.fleet_of(ALICE).unwrap().target_tags.is_empty());
}

#[test]
fn the_state_keyval_has_target_tags_keyed_by_item() {
    let (mut f, _) = with_alice();
    f.tag(ALICE, ROCK, Some("Z")).expect("tag");
    let v = f.state_of(ALICE).expect("state");
    let PyValue::Instance { .. } = &v else {
        panic!("not an object: {v:?}")
    };
    assert_eq!(attrs(&v), vec!["targetTags".to_string()]);
    let (name, args) = FleetEvent::State {
        tags: f.fleet_of(ALICE).unwrap().target_tags.clone(),
    }
    .to_push();
    assert_eq!(name, "OnFleetStateChange");
    assert_eq!(args.len(), 1);
    let d = format!("{:?}", args[0]);
    assert!(
        d.contains("Int(7001)") && d.contains("Unicode(\"Z\")"),
        "{d}"
    );
}

#[test]
fn get_fleet_state_is_none_without_a_fleet() {
    let f = Fleets::new(NODE);
    assert!(f.state_of(CAROL).is_none());
}

#[test]
fn a_dead_item_loses_its_tag_and_a_joiner_is_told_the_tags() {
    let (mut f, id) = with_alice();
    f.tag(ALICE, ROCK, Some("2")).expect("tag");
    f.invite(ALICE, id, BOB, None, None, None).expect("invite");
    let (_, joined) = f
        .accept_invite(BOB, id, Some(SHIP_TYPE), None, None)
        .expect("accept");
    let st = joined
        .pushes
        .iter()
        .find(|p| p.event.to_push().0 == "OnFleetStateChange")
        .expect("replay");
    assert_eq!(st.to, vec![BOB]);
    let e = f.untag_item(ROCK);
    assert_eq!(events(&e), vec!["OnFleetStateChange"]);
    assert!(f.untag_item(ROCK).pushes.is_empty());
}

#[test]
fn a_loot_event_is_keyed_by_char_and_type() {
    let (f, _) = with_alice_and_bob();
    let pushes = f.loot_event(ALICE, 555, 3, |m| m == BOB);
    assert_eq!(pushes.len(), 1);
    let (name, args) = pushes[0].event.to_push();
    assert_eq!(name, "OnFleetLootEvent");
    assert_eq!(
        args[0],
        PyValue::Dict(vec![(
            PyValue::Int(3),
            PyValue::Tuple(vec![PyValue::Int(ALICE), PyValue::Int(555)])
        )])
    );
    assert_eq!(pushes[0].to.len(), 2);
    assert!(f.loot_event(CAROL, 555, 1, |_| true).is_empty());
}
