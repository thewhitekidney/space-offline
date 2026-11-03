//! The wing's choice of target as tables. Invented ids; the asserts are
//! which ship is chosen and which act names it, never a range or a delay
//! (those are [A]).

use super::*;
use crate::brain::{GunInfo, ParkView, Policy, ShipInfo};
use crate::threat::Foe;
use crate::wing::Seat;
use crate::wing_hall::WingHall;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SHIP: i64 = 71;
const FC_SHIP: i64 = 70;
const RAT: i64 = 80;
const RAT_B: i64 = 81;
const PILOT: i64 = 82;
const PILOT_B: i64 = 83;
const C_A: i64 = 5;
const C_B: i64 = 6;

fn ball(id: i64, x: f64) -> Ball {
    Ball {
        id,
        position: [x, 0.0, 0.0],
        radius: 20.0,
    }
}

fn npc(id: i64, x: f64) -> Contact {
    Contact {
        ball: ball(id, x),
        char_id: None,
        hostile: true,
        war: false,
        linking: false,
    }
}

fn pilot(id: i64, ch: i64, x: f64) -> Contact {
    Contact {
        ball: ball(id, x),
        char_id: Some(ch),
        hostile: true,
        war: false,
        linking: false,
    }
}

fn tags(list: &[(i64, &str)]) -> BTreeMap<i64, String> {
    list.iter().map(|(i, t)| (*i, (*t).to_owned())).collect()
}

fn pic<'a>(
    stance: Stance,
    tags: &'a BTreeMap<i64, String>,
    contacts: &'a [Contact],
    foes: &'a [i64],
) -> Picture<'a> {
    Picture {
        stance,
        tags,
        contacts,
        fc: Some(ball(FC_SHIP, 0.0)),
        foes,
        our_linking: false,
        held: false,
        security: Some(0.5),
    }
}

fn why(p: &Picture) -> Option<(i64, Why)> {
    pick_target(p).map(|x| (x.ball, x.why))
}

#[test]
fn a_tag_beats_an_aggressor_and_the_menu_order_decides_between_tags() {
    let t = tags(&[(RAT, "5"), (RAT_B, "A")]);
    let cs = [
        npc(RAT, 9_000.0),
        npc(RAT_B, 1_000.0),
        pilot(PILOT, C_A, 500.0),
    ];
    let p = pic(Stance::Defend, &t, &cs, &[C_A]);
    // Digits come before letters in the menu, however near the other is.
    assert_eq!(why(&p), Some((RAT, Why::Tag)));
    let t = tags(&[(RAT_B, "A"), (RAT, "Z")]);
    assert_eq!(
        why(&pic(Stance::Defend, &t, &cs, &[C_A])),
        Some((RAT_B, Why::Tag))
    );
    // The menu order, end to end; a label the menu never offers is last.
    assert!(rank("0") < rank("9") && rank("9") < rank("A") && rank("J") < rank("X"));
    assert!(rank("Z") < rank("?"));
}

#[test]
fn a_tag_is_followed_in_every_stance_but_never_on_an_ally() {
    let t = tags(&[(RAT, "1"), (PILOT, "2")]);
    let mut ally = pilot(PILOT, C_A, 100.0);
    ally.hostile = false;
    let cs = [npc(RAT, 5_000.0), ally];
    for s in [Stance::Defend, Stance::Hold, Stance::Free] {
        assert_eq!(why(&pic(s, &t, &cs, &[])), Some((RAT, Why::Tag)), "{s:?}");
    }
    let t = tags(&[(PILOT, "1")]);
    assert_eq!(pick_target(&pic(Stance::Free, &t, &cs, &[])), None);
}

#[test]
fn a_contest_beats_an_aggressor_and_hold_does_not_contest() {
    let t = BTreeMap::new();
    let mut linker = pilot(PILOT_B, C_B, 8_000.0);
    linker.linking = true;
    let cs = [pilot(PILOT, C_A, 500.0), linker];
    // Our linkers link: the stranger's link is the target.
    let mut p = pic(Stance::Defend, &t, &cs, &[C_A]);
    p.our_linking = true;
    assert_eq!(why(&p), Some((PILOT_B, Why::Contest)));
    // On our own sov, with no link of ours, the same.
    p.our_linking = false;
    p.held = true;
    assert_eq!(why(&p), Some((PILOT_B, Why::Contest)));
    // Nobody links and we hold nothing: the aggressor.
    p.held = false;
    assert_eq!(why(&p), Some((PILOT, Why::Defend)));
    // Hold contests nothing.
    let mut p = pic(Stance::Hold, &t, &cs, &[C_A]);
    p.our_linking = true;
    assert_eq!(pick_target(&p), None);
}

#[test]
fn a_pilot_who_shoots_us_is_shot_and_a_bystander_is_not() {
    let t = BTreeMap::new();
    let cs = [pilot(PILOT, C_A, 500.0), pilot(PILOT_B, C_B, 100.0)];
    assert_eq!(
        why(&pic(Stance::Defend, &t, &cs, &[C_A])),
        Some((PILOT, Why::Defend))
    );
    assert_eq!(pick_target(&pic(Stance::Defend, &t, &cs, &[])), None);
    // Nearest to the FC first when two shoot.
    assert_eq!(
        why(&pic(Stance::Free, &t, &cs, &[C_A, C_B])),
        Some((PILOT_B, Why::Defend))
    );
}

#[test]
fn free_ignores_hi_and_low_unless_the_target_is_flagged_for_us() {
    let t = BTreeMap::new();
    let mut cs = [pilot(PILOT, C_A, 500.0)];
    let mut p = pic(Stance::Free, &t, &cs, &[]);
    assert_eq!(pick_target(&p), None, "high sec");
    p.security = Some(-0.3);
    assert_eq!(why(&p), Some((PILOT, Why::Free)), "null");
    p.security = None;
    assert_eq!(pick_target(&p), None, "unknown security is not lawless");
    cs[0].war = true;
    let mut p = pic(Stance::Free, &t, &cs, &[]);
    p.security = Some(0.9);
    assert_eq!(why(&p), Some((PILOT, Why::Free)), "a war target");
    // Defend never takes the free shot.
    p.stance = Stance::Defend;
    assert_eq!(pick_target(&p), None);
}

#[test]
fn an_npc_on_the_fc_is_last_and_left_to_the_escort() {
    let t = BTreeMap::new();
    let cs = [npc(RAT, 1_000.0)];
    assert_eq!(pick_target(&pic(Stance::Free, &t, &cs, &[])), None);
}

#[test]
fn tags_are_read_off_the_wire_as_the_server_sends_them() {
    // `{label: itemID}` in wire order `(value, key)`, as `state_value` builds it.
    let wire = |pairs: Vec<(&str, i64)>| {
        PyValue::Dict(vec![(
            PyValue::Dict(
                pairs
                    .into_iter()
                    .map(|(l, i)| (PyValue::Unicode(l.to_owned()), PyValue::Int(i)))
                    .collect(),
            ),
            PyValue::Unicode("targetTags".to_owned()),
        )])
    };
    assert_eq!(
        parse_tags(&wire(vec![("3", RAT), ("A", PILOT)])),
        Some(tags(&[(RAT, "3"), (PILOT, "A")]))
    );
    assert_eq!(parse_tags(&wire(vec![])), Some(BTreeMap::new()));
    assert_eq!(parse_tags(&PyValue::None), None);
}

// ---- on the grid ------------------------------------------------------------------------------

fn wing(stance: Stance) -> Memory {
    let mut m = Memory::new(
        "Wing1",
        Policy {
            role: &crate::wing::ROLE,
            wing: Some(Seat::new(Arc::new(WingHall::new()), Some(7))),
            ..Policy::fast()
        },
    );
    m.wing.stance = stance;
    m
}

fn on_grid_obs(hostiles: &[(i64, f64)]) -> Observation {
    let mut o = Observation::empty(Instant::now());
    o.place = Place::InSpace {
        system: 30,
        bound: true,
    };
    o.ship = Some(ShipInfo {
        ship_id: SHIP,
        type_id: Some(crate::escort::RIFTER),
    });
    o.lock_range = 40_000.0;
    o.guns = vec![GunInfo {
        item_id: 3,
        type_id: crate::escort::GUN,
        flag_id: 27,
        online: true,
        active: false,
        target: None,
        rounds: Some(100),
        capacity: 100,
        range: 3_000.0,
        reload: Duration::ZERO,
        kind: crate::brain::GunKind::Weapon,
        reach: 0.0,
    }];
    o.fleet = Some(crate::brain::FleetView {
        fleet_id: 1,
        boss: Some(7),
    });
    o.park = Some(ParkView {
        ego: ball(SHIP, 0.0),
        ego_warping: false,
        rocks: Vec::new(),
        belts: Vec::new(),
        station: None,
        boss: Some(ball(FC_SHIP, 1_000.0)),
        taken: BTreeMap::new(),
        hostiles: hostiles
            .iter()
            .map(|(id, x)| Hostile {
                ball: ball(*id, *x),
                type_id: 1,
                name: "a rat".to_owned(),
                shooting: None,
            })
            .collect(),
        wrecks: Vec::new(),
        ego_tether: None,
    });
    o
}

/// The ships the acts go for: orbited or locked.
fn locks(acts: &[Act]) -> Vec<i64> {
    acts.iter()
        .filter_map(|a| match a {
            Act::Lock(id) | Act::Orbit { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tagged_rat_is_locked_though_another_is_nearer() {
    let mut o = on_grid_obs(&[(RAT, 2_000.0), (RAT_B, 9_000.0)]);
    o.wing.tags = tags(&[(RAT_B, "1")]);
    let mut m = wing(Stance::Defend);
    // The wing may think for a moment first; give it a few turns.
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.extend(locks(&on_grid(&mut m, &o)));
        o.now += Duration::from_secs(3);
    }
    assert!(seen.contains(&RAT_B), "locked {seen:?}");
    assert!(!seen.contains(&RAT));
}

#[test]
fn a_hostile_pilot_who_shoots_the_wing_is_locked_from_the_hunt_scene() {
    let mut o = on_grid_obs(&[]);
    o.hunt.pilots = vec![crate::hunt::Pilot {
        ball: ball(PILOT, 3_000.0),
        char_id: C_A,
        type_id: Some(1),
        who: eve_server::group_contacts::Affiliation {
            character_id: C_A,
            corporation_id: 0,
            alliance_id: None,
        },
        hostile: true,
        relation: 0.0,
        war: false,
    }];
    o.threat.foes = vec![Foe {
        char_id: C_A,
        cause: Cause::Mate,
    }];
    let mut m = wing(Stance::Defend);
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.extend(locks(&on_grid(&mut m, &o)));
        o.now += Duration::from_secs(3);
    }
    assert!(seen.contains(&PILOT), "locked {seen:?}");
    // Held: the same pilot is not on its own account.
    let mut m = wing(Stance::Hold);
    for _ in 0..4 {
        assert!(locks(&on_grid(&mut m, &o)).is_empty());
        o.now += Duration::from_secs(3);
    }
}

#[test]
fn hold_silences_the_escorts_own_fight_but_keeps_the_place() {
    let o = on_grid_obs(&[(RAT, 2_000.0)]);
    let mut m = wing(Stance::Hold);
    let mut m_free = wing(Stance::Defend);
    let mut open = false;
    let mut o2 = o.clone();
    for _ in 0..4 {
        assert!(locks(&on_grid(&mut m, &o2)).is_empty());
        open |= !locks(&on_grid(&mut m_free, &o2)).is_empty();
        o2.now += Duration::from_secs(3);
    }
    assert!(open, "the default stance does take the rat");
}

#[test]
fn the_default_stance_defends_and_a_stance_is_set_by_assignment() {
    let mut m = wing(Stance::default());
    assert_eq!(m.wing.stance, Stance::Defend);
    m.wing.stance = Stance::Free;
    assert_eq!(m.wing.stance.word(), "free");
}

#[test]
fn winglogi_is_a_wing_row_and_a_logi() {
    assert!(is_wing(&crate::wing::ROLE) && is_wing(&LOGI_ROLE));
    assert!(!is_wing(&crate::roles::ESCORT));
    assert!(crate::logi::is_logi(&LOGI_ROLE));
    assert!(std::ptr::eq(
        crate::roles::parse("winglogi").expect("a row"),
        &LOGI_ROLE
    ));
    assert!(!LOGI_ROLE.civilian);
}
