//! The owner's words to the wing (FC-3): what a command does to a bot's
//! memory and who answers. Invented ids; the asserts are acts and states,
//! never an [A] constant.

use super::tests::{ball, docked, fc_at, in_space, wing};
use super::*;
use crate::brain::{ActKind, decide};
use crate::chat::RoomKind;
use crate::chatter::Line;

const FC: i64 = 7;
const MATE: i64 = 8;
const S1: i64 = 1;
const S3: i64 = 3;
const HERE: i64 = 61;

fn said(o: &mut Observation, seq: u64, from: i64, text: &str) {
    o.talk.heard.push(Line {
        seq,
        room: RoomKind::Fleet,
        from,
        text: text.to_owned(),
        at: o.now,
        human: true,
    });
}

fn says(acts: &[Act]) -> Vec<String> {
    acts.iter()
        .filter_map(|a| match a {
            Act::Say(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_line_from_the_fc_is_a_command_and_one_from_anyone_else_is_not() {
    let mut o = docked(fc_at(S1, Some(HERE)));
    said(&mut o, 1, MATE, "weapons free");
    let (m, _) = decide(&wing(Some(FC)), &o);
    assert_eq!(m.wing.stance, Stance::Defend, "a mate's words are talk");
    said(&mut o, 2, FC, "weapons free");
    let (m, _) = decide(&wing(Some(FC)), &o);
    assert_eq!(m.wing.stance, Stance::Free);
    assert_eq!(m.wing.chat_seq, 2);
    // Talk that is not a command changes nothing.
    let mut o = docked(fc_at(S1, Some(HERE)));
    said(&mut o, 1, FC, "weapons free would be rude here");
    let (m, _) = decide(&wing(Some(FC)), &o);
    assert_eq!(m.wing.stance, Stance::Defend);
}

#[test]
fn a_line_is_read_once() {
    let mut o = docked(fc_at(S1, Some(HERE)));
    said(&mut o, 1, FC, "weapons free");
    let (mut m, _) = decide(&wing(Some(FC)), &o);
    // The FC said "defend" through /bots; the old chat line stays in the
    // observation for a minute but is not read again.
    queue(&mut m, WingCmd::Stance(WingMode::Defend));
    let (m, _) = decide(&m, &o);
    assert_eq!(m.wing.stance, Stance::Defend);
}

#[test]
fn only_the_lead_answers_and_once_per_command() {
    let mut o = docked(fc_at(S1, Some(HERE)));
    said(&mut o, 1, FC, "hold");
    // Not the lead: silent.
    let (_, acts) = decide(&wing(Some(FC)), &o);
    assert!(says(&acts).is_empty(), "{acts:?}");
    // The lead: one copy.
    o.wing.lead = true;
    let (m, acts) = decide(&wing(Some(FC)), &o);
    assert_eq!(says(&acts), vec!["copy: holding".to_owned()], "{acts:?}");
    // The next turn, same observation: no second copy.
    let (_, acts) = decide(&m, &o);
    assert!(says(&acts).is_empty(), "{acts:?}");
}

#[test]
fn hold_stops_the_moves_and_follow_resumes() {
    let mut m = wing(Some(FC));
    let o = in_space(S1, fc_at(S3, None), None);
    queue(&mut m, WingCmd::Hold);
    let (m, acts) = decide(&m, &o);
    assert!(
        !acts.iter().any(|a| matches!(a, Act::JumpGate { .. })),
        "{acts:?}"
    );
    assert!(m.order.is_some_and(|x| x.heard == Heard::Hold));
    // Follow: the order is gone, and the wing goes where the FC is.
    let mut m = m;
    queue(&mut m, WingCmd::Follow);
    let (m, acts) = decide(&m, &o);
    assert!(m.order.is_none());
    assert_eq!(acts, vec![Act::JumpGate { gate: 101 }]);
}

#[test]
fn the_stance_defaults_to_defend_and_follow_resets_it() {
    let mut m = wing(Some(FC));
    assert_eq!(m.wing.stance, Stance::Defend);
    let o = docked(fc_at(S1, Some(HERE)));
    for (cmd, want) in [
        (WingCmd::Stance(WingMode::Free), Stance::Free),
        (WingCmd::Stance(WingMode::Hold), Stance::Hold),
        (WingCmd::Stance(WingMode::Defend), Stance::Defend),
    ] {
        queue(&mut m, cmd);
        m = decide(&m, &o).0;
        assert_eq!(m.wing.stance, want);
    }
    queue(&mut m, WingCmd::Stance(WingMode::Free));
    queue(&mut m, WingCmd::Follow);
    m = decide(&m, &o).0;
    assert_eq!(m.wing.stance, Stance::Defend);
}

#[test]
fn dock_and_go_home_keep_a_docked_bot_in_until_follow() {
    let o = docked(fc_at(S1, None));
    let undocks =
        |m: &Memory, o: &Observation| decide(m, o).1.iter().any(|a| a.kind() == ActKind::Undock);
    let mut m = wing(Some(FC));
    assert!(undocks(&m, &o), "the FC is out: follow him");
    queue(&mut m, WingCmd::GoHome);
    let (m, _) = decide(&m, &o);
    assert_eq!(m.wing.duty, Some(Duty::Home));
    assert!(!undocks(&m, &o));
    let mut m = m;
    queue(&mut m, WingCmd::Follow);
    let (m, _) = decide(&m, &o);
    assert_eq!(m.wing.duty, None);
    assert!(undocks(&m, &o));
    let mut m = m;
    queue(&mut m, WingCmd::Dock);
    let (m, _) = decide(&m, &o);
    assert!(!undocks(&m, &o));
}

#[test]
fn in_space_go_home_and_rearm_head_for_the_station() {
    for cmd in [WingCmd::GoHome, WingCmd::Rearm] {
        let mut m = wing(Some(FC));
        queue(&mut m, cmd.clone());
        let o = in_space(S1, fc_at(S3, None), Some(5_000.0));
        let (m, acts) = decide(&m, &o);
        assert!(m.homing.is_some(), "{cmd:?}");
        assert!(
            !acts.iter().any(|a| matches!(a, Act::JumpGate { .. })),
            "{cmd:?}: {acts:?}"
        );
    }
    // Dock with the FC docked: the rally docks him where the FC is.
    let mut m = wing(Some(FC));
    queue(&mut m, WingCmd::Dock);
    let o = in_space(S1, fc_at(S1, Some(HERE)), None);
    let (m, acts) = decide(&m, &o);
    assert!(m.homing.is_none());
    assert_eq!(acts, vec![Act::Wing(WingAct::GoDock { station: HERE })]);
}

#[test]
fn a_primary_waits_for_its_name_on_grid() {
    let mut m = wing(Some(FC));
    queue(&mut m, WingCmd::Primary("invented rat".to_owned()));
    let mut o = in_space(S1, fc_at(S1, None), Some(1_000.0));
    let (m2, _) = decide(&m, &o);
    assert!(m2.wing.primary.is_some(), "no such name yet");
    assert!(m2.target.is_none());
    o.park.as_mut().expect("a park").hostiles = vec![brain::Hostile {
        ball: ball(90, 3_000.0),
        type_id: 1,
        name: "Invented Rat Captain".to_owned(),
        shooting: None,
    }];
    let (m3, _) = decide(&m2, &o);
    assert_eq!(m3.target.as_ref().map(|t| t.0), Some(90));
    assert!(m3.wing.primary.is_none());
    // Unseen too long: dropped.
    o.park.as_mut().expect("a park").hostiles.clear();
    let mut late = m2.clone();
    o.now += PRIMARY_FOR + Duration::from_secs(1);
    late = decide(&late, &o).0;
    assert!(late.wing.primary.is_none());
}

#[test]
fn the_sov_words_are_answered_not_yet() {
    let mut o = docked(fc_at(S1, Some(HERE)));
    o.wing.lead = true;
    let mut m = wing(Some(FC));
    for cmd in [
        WingCmd::Entosis(eve_server::wing_words::LinkAim::Hub),
        WingCmd::StopEntosis,
        WingCmd::DropHub,
        WingCmd::FuelHub,
    ] {
        queue(&mut m, cmd);
        let (m2, acts) = decide(&m, &o);
        let r = says(&acts);
        assert!(r.len() == 1 && r[0].starts_with("not yet"), "{acts:?}");
        m = m2;
    }
    assert_eq!(m.wing.duty, None);
}

#[test]
fn status_is_one_line_from_the_lead() {
    let mut o = docked(fc_at(S1, None));
    o.wing.lead = true;
    o.wing.tally = Tally {
        grid: 3,
        docked: 1,
        away: 2,
    };
    let mut m = wing(Some(FC));
    queue(&mut m, WingCmd::Status);
    let (_, acts) = decide(&m, &o);
    let r = says(&acts);
    assert_eq!(r.len(), 1, "{acts:?}");
    assert!(
        r[0].contains("3 on grid") && r[0].contains("1 docked"),
        "{r:?}"
    );
}

#[test]
fn the_queue_is_bounded_and_join_re_arms_the_poll() {
    let mut m = wing(Some(FC));
    for _ in 0..CMD_QUEUE + 5 {
        queue(&mut m, WingCmd::Status);
    }
    assert_eq!(m.wing.cmds.len(), CMD_QUEUE);
    // No fleet: a join is asked at once, and again after `/bots join`.
    let mut o = docked(None);
    o.fleet = None;
    let mut m = wing(Some(FC));
    assert_eq!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
    assert_ne!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
    queue(&mut m, WingCmd::Join);
    let _ = commands(&mut m, &o);
    assert_eq!(super::docked(&mut m, &o), vec![Act::Wing(WingAct::Join)]);
}

#[test]
fn a_bot_that_is_not_a_wing_member_ignores_the_words() {
    let mut m = wing(Some(FC));
    m.policy.wing = None;
    let mut o = docked(fc_at(S1, Some(HERE)));
    said(&mut o, 1, FC, "weapons free");
    let (m, _) = decide(&m, &o);
    assert_eq!(m.wing.stance, Stance::Defend);
}
