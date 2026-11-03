//! The wing's rules as tables, each with a minimal observation. Invented
//! ids; the asserts are acts and ids, never a range or a delay (those are
//! [A]).

use super::*;
use crate::ItemRow;
use crate::brain::{
    AcceptFleet, ActKind, Ball, FleetView, GunInfo, Invite, ParkView, Policy, ShipInfo, decide,
};
use crate::escort::{AMMO, GUN, RIFTER, SPARE_ROUNDS};
use crate::travel::Map;
use std::collections::BTreeMap;

const S1: i64 = 1;
const S2: i64 = 2;
const S3: i64 = 3;
const ISLAND: i64 = 9;
const HERE: i64 = 61;
const HERE_B: i64 = 62;
const FAR: i64 = 63;
const FC: i64 = 7;
const MATE: i64 = 8;
const FC_SHIP: i64 = 70;
const SHIP: i64 = 71;
const FLEET: i64 = 5;
const OTHER_FLEET: i64 = 6;

fn map() -> Map {
    Map::new()
        .with_gate(S1, 101, S2, 102)
        .with_gate(S2, 102, S1, 101)
        .with_gate(S2, 103, S3, 104)
        .with_gate(S3, 104, S2, 103)
        .with_station(HERE, S1)
        .with_station(HERE_B, S1)
        .with_station(FAR, S3)
}

pub(super) fn ball(id: i64, x: f64) -> Ball {
    Ball {
        id,
        position: [x, 0.0, 0.0],
        radius: 50.0,
    }
}

pub(super) fn wing(fc: Option<i64>) -> Memory {
    Memory::new(
        "Wing1",
        Policy {
            role: &ROLE,
            wing: Some(Seat::new(Arc::new(WingHall::new()), fc)),
            travel: Some(Arc::new(map())),
            accept_fleet: fc.map_or(AcceptFleet::Any, AcceptFleet::Fc),
            ..Policy::fast()
        },
    )
}

fn gun(item_id: i64) -> GunInfo {
    GunInfo {
        item_id,
        type_id: GUN,
        flag_id: 27,
        online: true,
        active: false,
        target: None,
        rounds: Some(200),
        capacity: 200,
        range: 3_000.0,
        reload: Duration::ZERO,
        kind: crate::brain::GunKind::Weapon,
        reach: 0.0,
    }
}

/// Docked in `HERE` in a Rifter with full guns and a stocked hold, in a
/// fleet, the FC at `fc`.
pub(super) fn docked(fc: Option<Fc>) -> Observation {
    let mut o = Observation::empty(Instant::now());
    o.place = Place::Docked { station: HERE };
    o.ship = Some(ShipInfo {
        ship_id: SHIP,
        type_id: Some(RIFTER),
    });
    o.guns = vec![gun(1), gun(2)];
    o.cargo = vec![ItemRow {
        item_id: 3,
        type_id: AMMO,
        owner_id: MATE,
        location_id: SHIP,
        flag_id: crate::FLAG_CARGO,
        quantity: SPARE_ROUNDS,
        group_id: 0,
        category_id: crate::burst::CATEGORY_CHARGE,
        singleton: false,
    }];
    o.home = Some(HERE);
    o.lock_range = 20_000.0;
    o.fleet = Some(FleetView {
        fleet_id: FLEET,
        boss: Some(FC),
    });
    o.wing.fc = fc;
    o
}

pub(super) fn fc_at(system: i64, station: Option<i64>) -> Option<Fc> {
    Some(Fc {
        char_id: FC,
        system,
        station,
    })
}

/// In space in `system` with the FC's ship on the grid at `boss_x`, if any.
pub(super) fn in_space(system: i64, fc: Option<Fc>, boss_x: Option<f64>) -> Observation {
    let mut o = docked(fc);
    o.place = Place::InSpace {
        system,
        bound: true,
    };
    o.park = Some(ParkView {
        ego: ball(SHIP, 0.0),
        ego_warping: false,
        rocks: Vec::new(),
        belts: Vec::new(),
        station: None,
        boss: boss_x.map(|x| ball(FC_SHIP, x)),
        taken: BTreeMap::new(),
        hostiles: Vec::new(),
        wrecks: Vec::new(),
        ego_tether: boss_x.map(|_| FC_SHIP),
    });
    o
}

fn kinds(acts: &[Act]) -> Vec<ActKind> {
    acts.iter().map(Act::kind).collect()
}

#[test]
fn docked_the_undock_waits_for_an_fc_to_follow() {
    // The kit is ready, so the escort's turn undocks; the wing's gate decides.
    let mut m = wing(Some(FC));
    let undocks = |m: &mut Memory, o: &Observation| docked_acts(m, o).contains(&ActKind::Undock);
    // He is docked in this very station: stay.
    assert!(!undocks(&mut m, &docked(fc_at(S1, Some(HERE)))));
    // He is in space here, docked in another station, or in another system.
    assert!(undocks(&mut m, &docked(fc_at(S1, None))));
    assert!(undocks(&mut m, &docked(fc_at(S1, Some(HERE_B)))));
    assert!(undocks(&mut m, &docked(fc_at(S3, Some(FAR)))));
    // Nothing known of him: wait, do not wander off.
    assert!(!undocks(&mut m, &docked(None)));
    // No map to route by: the escort's own rules.
    m.policy.travel = None;
    assert!(undocks(&mut m, &docked(None)));
}

fn docked_acts(m: &mut Memory, o: &Observation) -> Vec<ActKind> {
    kinds(&docked_with(m, o))
}

fn docked_with(m: &mut Memory, o: &Observation) -> Vec<Act> {
    // Joined already: no join act in the way.
    super::docked(m, o)
}

#[test]
fn a_bot_that_is_not_a_wing_member_is_the_escort() {
    let mut m = wing(Some(FC));
    m.policy.wing = None;
    let o = docked(fc_at(S1, Some(HERE)));
    assert_eq!(kinds(&super::docked(&mut m, &o)), vec![ActKind::Undock]);
}

#[test]
fn in_space_the_wing_goes_where_the_fc_is() {
    let mut m = wing(Some(FC));
    // Another system: the first gate of the route.
    let acts = super::in_space(&mut m, &in_space(S1, fc_at(S3, None), None));
    assert_eq!(acts, vec![Act::JumpGate { gate: 101 }]);
    // He docked in this system: go and dock there.
    let acts = super::in_space(&mut m, &in_space(S1, fc_at(S1, Some(HERE_B)), None));
    assert_eq!(acts, vec![Act::Wing(WingAct::GoDock { station: HERE_B })]);
    // In space here, not on this grid: warp to him.
    let acts = super::in_space(&mut m, &in_space(S1, fc_at(S1, None), None));
    assert_eq!(
        acts,
        vec![Act::WarpToMember {
            char_id: FC,
            range: crate::escort::WARP_STANDOFF
        }]
    );
    // A warp the server refused is not repeated at once.
    let mut o = in_space(S1, fc_at(S1, None), None);
    o.boss_unreachable_until = Some(o.now + Duration::from_secs(30));
    assert!(
        !super::in_space(&mut m, &o)
            .iter()
            .any(|a| matches!(a, Act::WarpToMember { .. }))
    );
}

#[test]
fn on_his_grid_the_escorts_turn_follows_and_fights() {
    let mut m = wing(Some(FC));
    let mut o = in_space(S1, fc_at(S1, None), Some(20_000.0));
    o.park.as_mut().expect("a park").ego_tether = None;
    let acts = super::in_space(&mut m, &o);
    assert_eq!(kinds(&acts), vec![ActKind::Follow], "{acts:?}");
    assert!(matches!(acts[0], Act::Follow { id, .. } if id == FC_SHIP));
    // A rat on him is the escort's fight, not a wing act.
    let mut o = in_space(S1, fc_at(S1, None), Some(1_000.0));
    o.park.as_mut().expect("a park").hostiles = vec![brain::Hostile {
        ball: ball(90, 3_000.0),
        type_id: 1,
        name: "Rat".to_owned(),
        shooting: None,
    }];
    let acts = super::in_space(&mut m, &o);
    assert!(
        !acts
            .iter()
            .any(|a| matches!(a, Act::JumpGate { .. } | Act::Wing(_))),
        "{acts:?}"
    );
}

#[test]
fn no_route_is_said_once_and_then_the_wing_waits() {
    let mut m = wing(Some(FC));
    let o = in_space(S1, fc_at(ISLAND, None), None);
    let first = super::in_space(&mut m, &o);
    assert_eq!(kinds(&first), vec![ActKind::Missing, ActKind::Wait]);
    let again = super::in_space(&mut m, &o);
    assert_eq!(kinds(&again), vec![ActKind::Wait]);
}

#[test]
fn a_bot_with_no_fleet_applies_to_the_fc_every_poll() {
    let mut m = wing(Some(FC));
    let mut o = docked(None);
    o.fleet = None;
    let t0 = o.now;
    assert_eq!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
    // Not again inside the poll.
    assert_ne!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
    o.now = t0 + JOIN_POLL + Duration::from_secs(1);
    assert_eq!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
    // In a fleet, or with an invite waiting, it does not look.
    let mut o2 = docked(None);
    o2.now = o.now + JOIN_POLL * 2;
    assert_ne!(super::docked(&mut m, &o2), vec![Act::Wing(WingAct::Join)]);
    // No FC named (the inviter leads): nothing to look for.
    let mut free = wing(None);
    let mut o3 = docked(None);
    o3.fleet = None;
    assert_ne!(
        super::docked(&mut free, &o3),
        vec![Act::Wing(WingAct::Join)]
    );
}

#[test]
fn an_invite_is_taken_from_the_fc_or_into_the_fleet_applied_to() {
    let invite = |from: i64, fleet: i64| Invite {
        fleet_id: fleet,
        inviter_id: from,
        inviter_name: None,
    };
    let mut o = docked(None);
    o.fleet = None;
    // The FC's own invite.
    o.invite = Some(invite(FC, FLEET));
    let (_, acts) = decide(&wing(Some(FC)), &o);
    assert_eq!(acts, vec![Act::AcceptInvite { fleet: FLEET }]);
    // A stranger's, not.
    o.invite = Some(invite(MATE, OTHER_FLEET));
    let (m, acts) = decide(&wing(Some(FC)), &o);
    assert!(!acts.iter().any(|a| matches!(a, Act::AcceptInvite { .. })));
    assert!(m.declined.contains(&OTHER_FLEET));
    // Into the fleet it applied to, from whoever the server says (the boss).
    let mut applied = wing(Some(FC));
    applied.wing.applied = Some(OTHER_FLEET);
    let (_, acts) = decide(&applied, &o);
    assert_eq!(acts, vec![Act::AcceptInvite { fleet: OTHER_FLEET }]);
}
