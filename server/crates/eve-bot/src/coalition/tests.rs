//! R-REL-5 pure tests. Every id and row is invented; every tunable is
//! named, never written as a number.

use super::*;
use crate::corpmind::{CorpMind, HallState, Member};
use crate::mind::{TRAIT_COUNT, Traits};
use crate::society::{ALLY_AT, ALLY_DAYS, BLUE_STANDING, ENEMY_AT, Event, Satisfaction};

const DAY: u64 = 60;
const A1: i64 = 99_000_001;
const A2: i64 = 99_000_002;
const A3: i64 = 99_000_003;
const C1: i64 = 98_000_001;
const C2: i64 = 98_000_002;
const C3: i64 = 98_000_003;
const U1: i64 = 90_000_001;
const U2: i64 = 90_000_002;
const U3: i64 = 90_000_003;
/// A rival corp nobody runs.
const X: i64 = 98_000_900;
const XU: i64 = 90_000_900;

fn member(corp: i64, alliance: Option<i64>) -> Member {
    Member {
        name: "Invented".to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "pirate".to_owned(),
        band: crate::career::Band::Low,
        corp,
        alliance,
        last_turn: None,
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

/// A hall of bot corps `(corp, ceo, alliance, extra members)`.
fn world(spec: &[(i64, i64, Option<i64>, usize)]) -> HallState {
    let mut st = HallState::new(DAY);
    for &(corp, ceo, al, extra) in spec {
        st.members.insert(ceo, member(corp, al));
        for i in 0..extra {
            st.members
                .insert(ceo * 100 + i as i64 + 1, member(corp, al));
        }
        st.corps.insert(
            corp,
            CorpMind {
                id: corp,
                ceo: Some(ceo),
                profile: crate::society::Profile::of("pirate", crate::career::Band::Low),
                founded: 0,
                alliance: al,
                name: String::new(),
                applied_alliance: None,
                salt: 0,
            },
        );
        if let Some(a) = al {
            st.alliances.entry(a).or_default();
        }
    }
    st
}

/// Mutually blue since `since` (the coarse stance the blocs read).
fn blue(st: &mut HallState, a: i64, b: i64, since: u64) {
    st.stances.settle(a, b, ALLY_AT, since, DAY);
    st.stances.settle(b, a, ALLY_AT, since, DAY);
}

fn later() -> u64 {
    (ALLY_DAYS * DAY as f64) as u64 + 10
}

fn aff(c: i64, corp: i64, al: Option<i64>) -> Affiliation {
    Affiliation {
        character_id: c,
        corporation_id: corp,
        alliance_id: al,
    }
}

/// Game-hours as bot seconds.
fn hours_to_secs(h: f64) -> u64 {
    (h / 24.0 * DAY as f64).ceil() as u64
}

// ---- coalitions form ------------------------------------------------------------------

#[test]
fn two_friendly_alliances_court_into_a_coalition_the_contacts_show() {
    let mut st = world(&[(C1, U1, Some(A1), 0), (C2, U2, Some(A2), 0)]);
    st.relations.add(C1, A2, FRIEND_AT + 0.2);
    st.relations.add(C2, A1, FRIEND_AT + 0.2);
    let mut formed = None;
    for d in 0..40u64 {
        let now = d * DAY;
        st.tick(now);
        if formed.is_none() && !st.blocs(now).is_empty() {
            formed = Some(d);
        }
    }
    let now = 39 * DAY;
    let blocs = st.blocs(now);
    assert_eq!(blocs.len(), 1, "formed on day {formed:?}");
    assert!(blocs[0].contains(A1) && blocs[0].contains(A2));
    // What the executors' contact sync would publish: mutual +10.
    assert_eq!(st.desired_contacts(A1).get(&A2), Some(&BLUE_STANDING));
    assert_eq!(st.desired_contacts(A2).get(&A1), Some(&BLUE_STANDING));
}

#[test]
fn a_blue_set_on_a_group_is_answered_when_it_does_not_hold_it_against_it() {
    let mut st = world(&[(C1, U1, Some(A1), 0), (C2, U2, Some(A2), 0)]);
    // A1's executor has set A2 blue (the hall's book); A2's own view is only
    // fair, under the friendly bar.
    st.books.insert((A1, A2), BLUE_STANDING);
    st.relations.add(C2, A1, ACCEPT_BLUE_AT + 0.1);
    const { assert!(ACCEPT_BLUE_AT + 0.1 < FRIEND_AT) };
    st.relations.at = Some(0);
    let before = st.owner_view(A2, A1, 0);
    let lines = st.coalition_tick(0);
    let after = st.owner_view(A2, A1, 0);
    assert!(after > before, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("accepts")), "{lines:?}");
    // Once a day: the same day again changes nothing.
    assert!(st.coalition_tick(1).iter().all(|l| !l.contains("accepts")));
    assert_eq!(st.owner_view(A2, A1, 0), after);
    // A hostile one does not answer.
    let mut st = world(&[(C1, U1, Some(A1), 0), (C2, U2, Some(A2), 0)]);
    st.books.insert((A1, A2), BLUE_STANDING);
    st.relations.add(C2, A1, ENEMY_AT);
    st.relations.at = Some(0);
    assert!(st.coalition_tick(0).iter().all(|l| !l.contains("accepts")));
}

// ---- coalitions break -----------------------------------------------------------------

#[test]
fn a_betrayal_flips_the_victims_stance_at_once_and_the_bloc_takes_the_spill() {
    let mut st = world(&[
        (C1, U1, Some(A1), 2),
        (C2, U2, Some(A2), 1),
        (C3, U3, Some(A3), 0),
    ]);
    for (a, b) in [(A1, A2), (A1, A3), (A2, A3)] {
        blue(&mut st, a, b, 0);
    }
    st.relations.at = Some(later());
    // A pilot of A3 shoots a pilot of A2.
    let victim = aff(U2, C2, Some(A2));
    let killer = aff(U3 + 7, C3, Some(A3));
    assert_eq!(st.stances.get(A2, A3), Stance::Blue);
    let lines = st.betray(&victim, &killer, later());
    assert!(!lines.is_empty());
    assert_eq!(
        st.stances.get(A2, A3),
        Stance::Red,
        "at once, no flip window"
    );
    assert_eq!(st.stances.get5(A2, A3), Stance5::Hostile);
    assert_eq!(st.stances.betrayed_at(A2, A3), Some(later()));
    // The other member of the victim's bloc takes the spill against the
    // killer's group.
    assert!(st.alliance_own.get(A1, A3) < 0.0);
    // And a non-bot victim is not a betrayal of ours.
    assert!(st.betray(&aff(5, X, None), &killer, later()).is_empty());
}

#[test]
fn a_blue_partner_is_expelled_after_it_shoots_a_member() {
    // A1 leads (the heaviest); A3 is the lightest.
    let mut st = world(&[
        (C1, U1, Some(A1), 3),
        (C2, U2, Some(A2), 1),
        (C3, U3, Some(A3), 0),
    ]);
    for (a, b) in [(A1, A2), (A1, A3), (A2, A3)] {
        blue(&mut st, a, b, 0);
    }
    st.relations.at = Some(later());
    st.relations.add(C1, A3, ALLY_AT + 1.0);
    let bloc = st.blocs(later());
    assert_eq!(bloc.len(), 1);
    assert_eq!(bloc[0].lead, A1);
    let before = st.relations.get(C1, A3);
    // A3 shoots A2: A2 turns on A3 at once.
    st.betray(&aff(U2, C2, Some(A2)), &aff(U3, C3, Some(A3)), later());
    let lines = st.coalition_tick(later());
    assert!(
        lines
            .iter()
            .any(|l| l.contains("expels") && l.contains(&A3.to_string())),
        "{lines:?}"
    );
    assert_eq!(
        st.stances.get(A1, A3),
        Stance::Neutral,
        "the lead clears its blue"
    );
    assert!(st.relations.get(C1, A3) < before);
    // The contact sync would take the blue off A3.
    let book = BTreeMap::from([(A3, BLUE_STANDING)]);
    st.managed.insert((A1, A3));
    assert!(st.contact_diff(A1, &book, 0).contains(&(A3, None)));
    // The lead itself is never expelled by its members' quarrel with it.
    assert!(st.coal.expelled.contains_key(&(A1, A3)));
    assert!(!st.coal.expelled.contains_key(&(A1, A2)));
}

// ---- the enemy list, the lag, the quorum ----------------------------------------------

/// A bloc of A1 and A2 whose members both rate X at `v`.
fn bloc_against_x(v: f64) -> HallState {
    let mut st = world(&[(C1, U1, Some(A1), 1), (C2, U2, Some(A2), 0)]);
    blue(&mut st, A1, A2, 0);
    st.relations.at = Some(later());
    st.relations.add(C1, X, v);
    st.relations.add(C2, X, v);
    st
}

#[test]
fn a_lagging_member_follows_after_the_lag() {
    let mut st = bloc_against_x(ENEMY_AT - 2.0);
    let t0 = later();
    let lines = st.coalition_tick(t0);
    assert!(lines.iter().any(|l| l.contains("enemy")), "{lines:?}");
    let stranger = aff(XU, X, None);
    let view = |st: &HallState, now| st.r_eff_of(C1, &[], 0.5, &stranger, now);
    let named = view(&st, t0);
    assert!(!st.bloc_enemy(A1, X), "named a moment ago: not yet adopted");
    assert!(st.lagging(A1, X, t0));
    // Past the lag the coalition's word reaches the member.
    let lag = hours_to_secs(st.temper.lag_hours());
    st.relations.at = Some(t0 + lag);
    st.coalition_tick(t0 + lag);
    assert!(st.bloc_enemy(A1, X));
    assert!(!st.lagging(A1, X, t0 + lag));
    let adopted = view(&st, t0 + lag);
    assert!(adopted < named, "{adopted} < {named}");
    // The enemy list is dropped when the bloc stops agreeing.
    st.relations.r.clear();
    st.coalition_tick(t0 + lag + 1);
    assert!(!st.bloc_enemy(A1, X));
}

#[test]
fn a_bloc_declares_when_quorum_is_reached() {
    // A1 (weight 2) and A2 (weight 1). Only A2 hates X: under the quorum.
    let mut st = world(&[(C1, U1, Some(A1), 1), (C2, U2, Some(A2), 0)]);
    blue(&mut st, A1, A2, 0);
    st.relations.at = Some(later());
    st.relations.add(C2, X, WAR_AT - 2.0);
    st.relations.add(C1, X, -1.0);
    let q = |st: &HallState, owner| {
        st.war_candidates(owner, later())
            .into_iter()
            .find(|c| c.group == X)
            .map(|c| c.quorum_ok)
    };
    assert_eq!(
        q(&st, A1),
        Some(false),
        "a third of the weight is not a quorum"
    );
    // The heavy member agrees: the lead declares for the bloc, the follower
    // never does.
    st.relations.add(C1, X, WAR_AT - 1.0);
    assert_eq!(q(&st, A1), Some(true));
    assert_eq!(q(&st, A2), Some(false), "only the lead issues the act");
}

// ---- peace moves both views -----------------------------------------------------------

#[test]
fn peace_moves_both_views() {
    let mut st = world(&[(C1, U1, Some(A1), 0), (C2, U2, Some(A2), 0)]);
    st.relations.at = Some(0);
    let rows = |finished: bool| {
        let v = eve_server::pyutil::keyval(vec![
            ("warID", PyValue::Int(5)),
            ("declaredByID", PyValue::Int(A1)),
            ("againstID", PyValue::Int(A2)),
            (
                "timeFinished",
                if finished {
                    PyValue::Int(1)
                } else {
                    PyValue::None
                },
            ),
        ]);
        crate::observe::war_facts(&v, &[A1, C1], 0)
    };
    assert!(
        rows(false)
            .iter()
            .all(|(_, f)| f.kind != relations::FactKind::Peace)
    );
    let before = (st.alliance_own.get(A1, A2), st.alliance_own.get(A2, A1));
    for (_, f) in rows(true) {
        let ds = relations::deltas(&f, &st.temper);
        st.apply_deltas(&ds);
    }
    assert!(
        st.alliance_own.get(A1, A2) > before.0,
        "the aggressor's view"
    );
    assert!(
        st.alliance_own.get(A2, A1) > before.1,
        "the defender's view"
    );
}

// ---- the wire rows, mirrored ----------------------------------------------------------

fn server_war(
    by: i64,
    against: i64,
    started: i64,
    finished: Option<i64>,
    ally: Option<(i64, i64, i64)>,
) -> eve_server::war::War {
    let mut w = eve_server::war::War::new(7, by, against, started - 100, 100, None);
    w.time_finished = finished;
    if let Some((id, s, f)) = ally {
        w.allies.insert(
            id,
            eve_server::war::Ally {
                ally_id: id,
                time_started: s,
                time_finished: f,
                told: false,
            },
        );
    }
    w
}

#[test]
fn a_wire_row_reads_and_war_between_is_the_servers() {
    let (by, against, ally, other) = (98_000_101, 98_000_102, 98_000_103, 98_000_104);
    let cases = [
        server_war(by, against, 1_000, None, None),
        server_war(by, against, 1_000, Some(2_000), None),
        server_war(by, against, 1_000, None, Some((ally, 1_500, 3_000))),
    ];
    let sides: [&[i64]; 5] = [&[by], &[against], &[ally], &[other], &[by, other]];
    for w in &cases {
        let value = eve_server::war::war_value(w, 1_200);
        let row = WarRow::from_value(&value, 5).expect("a row");
        assert_eq!((row.id, row.by, row.against), (7, by, against));
        assert_eq!(row.finished, w.time_finished);
        assert_eq!(row.allies.len(), w.allies.len());
        for now in [900, 1_000, 1_001, 1_499, 1_501, 1_999, 2_001, 3_001] {
            for a in sides {
                for b in sides {
                    assert_eq!(
                        war_between([&row], a, b, now),
                        eve_server::war::war_between([w], a, b, now),
                        "{a:?} v {b:?} at {now}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_hall_answers_at_war_by_each_sides_corp_and_alliance() {
    let mut st = world(&[(C1, U1, Some(A1), 0)]);
    let ft = 1_000_000;
    st.note_war(WarRow {
        id: 1,
        by: A1,
        against: X,
        started: ft - 10,
        finished: None,
        mutual: false,
        open: false,
        allies: vec![],
        seen: 0,
    });
    let me = aff(U1, C1, Some(A1));
    let foe = aff(XU, X, None);
    assert!(st.at_war(&me, &foe, ft));
    assert!(st.at_war(&foe, &me, ft), "either way round");
    assert!(!st.at_war(&me, &aff(5, 98_000_950, None), ft));
    assert!(!st.at_war(&me, &foe, ft - 20), "still spooling up");
    assert!(st.war_on(&me, ft));
    // The first sighting's time is kept when the row is read again.
    st.note_war(WarRow {
        seen: 99,
        ..st.coal.wars[&1].clone()
    });
    assert_eq!(st.coal.wars[&1].seen, 0);
}

// ---- the war pass (pure) ----------------------------------------------------------------

const OWNER: i64 = A1;
const FT: i64 = 5_000_000;
const NOW: u64 = 100 * DAY;

fn cand(group: i64, view: f64) -> Cand {
    Cand {
        group,
        view,
        bot: false,
        back: 0.0,
        quorum_ok: true,
        friend: false,
    }
}

fn row(id: i64, by: i64, against: i64) -> WarRow {
    WarRow {
        id,
        by,
        against,
        started: FT - 1_000,
        finished: None,
        mutual: false,
        open: false,
        allies: vec![],
        seen: NOW - (WAR_MIN_DAYS * DAY as f64) as u64 - 1,
    }
}

/// The context with every gate open; each test closes the one it tests.
struct Rig {
    wars: Vec<WarRow>,
    open: Vec<WarRow>,
    negs: Vec<Neg>,
    invites: Vec<i64>,
    cands: Vec<Cand>,
    views: BTreeMap<i64, f64>,
    blues: BTreeSet<i64>,
    lost: BTreeMap<i64, f64>,
    killed: BTreeMap<i64, f64>,
    offered: BTreeMap<(i64, i64), u64>,
    enabled: bool,
    humans: bool,
    aggression: f64,
    wallet: f64,
    autopay: bool,
    mates: bool,
    lag_ok: bool,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            wars: vec![],
            open: vec![],
            negs: vec![],
            invites: vec![],
            cands: vec![],
            views: BTreeMap::new(),
            blues: BTreeSet::new(),
            lost: BTreeMap::new(),
            killed: BTreeMap::new(),
            offered: BTreeMap::new(),
            enabled: true,
            humans: true,
            aggression: 1.0,
            wallet: eve_server::war::COST_OF_WAR * WAR_RESERVE * 2.0,
            autopay: true,
            mates: false,
            lag_ok: true,
        }
    }

    fn acts(&self) -> Vec<WarAct> {
        let view = |g: i64| self.views.get(&g).copied().unwrap_or(0.0);
        let blue = |g: i64| self.blues.contains(&g);
        let lost = |g: i64| self.lost.get(&g).copied().unwrap_or(0.0);
        let killed = |g: i64| self.killed.get(&g).copied().unwrap_or(0.0);
        let lag = |_: &WarRow| self.lag_ok;
        war_acts(&WarCtx {
            owner: OWNER,
            ft: FT,
            now: NOW,
            day: DAY,
            aggression: self.aggression,
            wallet: self.wallet,
            enabled: self.enabled,
            humans_ok: self.humans,
            wars: &self.wars,
            open_wars: &self.open,
            negs: &self.negs,
            invites: &self.invites,
            cands: &self.cands,
            view: &view,
            blue: &blue,
            lost: &lost,
            destroyed: &killed,
            autopay: self.autopay,
            has_mates: self.mates,
            lag_ok: &lag,
            offered: &self.offered,
        })
    }
}

#[test]
fn a_declaration_needs_the_switch_the_aggression_the_wallet_and_a_bad_enough_view() {
    let mut r = Rig::new();
    r.cands = vec![cand(X, WAR_AT)];
    assert_eq!(r.acts(), vec![WarAct::Declare(X)]);
    r.enabled = false;
    assert!(r.acts().is_empty(), "no switch, no war");
    r.enabled = true;
    r.aggression = WAR_AGGRESSION - 0.1;
    assert!(r.acts().is_empty(), "a meek CEO");
    r.aggression = 1.0;
    r.wallet = eve_server::war::COST_OF_WAR * WAR_RESERVE - 1.0;
    assert!(r.acts().is_empty(), "under the reserve");
    r.wallet = eve_server::war::COST_OF_WAR * WAR_RESERVE;
    r.cands = vec![cand(X, WAR_AT + 0.5)];
    assert!(r.acts().is_empty(), "not bad enough");
    // The worst view first, one a pass.
    r.cands = vec![
        cand(X, WAR_AT - 1.0),
        cand(X + 1, WAR_AT - 3.0),
        cand(X + 2, WAR_AT),
    ];
    assert_eq!(r.acts(), vec![WarAct::Declare(X + 1)]);
}

#[test]
fn a_declaration_respects_the_cap_friends_humans_and_the_quorum() {
    let mut r = Rig::new();
    r.cands = vec![cand(X, WAR_AT - 1.0)];
    // At the cap.
    r.wars = (0..BOT_MAX_WARS as i64)
        .map(|i| row(i + 1, OWNER, 98_000_500 + i))
        .collect();
    assert!(r.acts().iter().all(|a| !matches!(a, WarAct::Declare(_))));
    r.wars.clear();
    // A group that holds a friend, one without the quorum.
    r.cands[0].friend = true;
    assert!(r.acts().is_empty());
    r.cands[0].friend = false;
    r.cands[0].quorum_ok = false;
    assert!(r.acts().is_empty());
    r.cands[0].quorum_ok = true;
    // A group of humans needs `--hostile` too.
    r.humans = false;
    assert!(r.acts().is_empty());
    r.cands[0].bot = true;
    assert_eq!(r.acts(), vec![WarAct::Declare(X)], "a bot group does not");
    // A war already running with it: not again.
    r.wars = vec![row(1, X, OWNER)];
    assert!(
        r.acts()
            .iter()
            .all(|a| !matches!(a, WarAct::Declare(_) | WarAct::Mutual(_)))
    );
}

#[test]
fn two_bot_groups_that_hate_each_other_use_a_mutual_invite_without_the_fee() {
    let mut r = Rig::new();
    let mut c = cand(X, WAR_AT - 1.0);
    c.bot = true;
    c.back = ENEMY_AT;
    r.cands = vec![c];
    r.wallet = 0.0;
    assert_eq!(
        r.acts(),
        vec![WarAct::Mutual(X)],
        "no fee, so no wallet needed"
    );
    // The other side is not hostile: a declaration, which needs the fee.
    r.cands[0].back = 0.0;
    assert!(r.acts().is_empty());
}

#[test]
fn a_mutual_invite_is_accepted_by_a_red_aggressive_group_and_declined_by_a_friend() {
    let mut r = Rig::new();
    r.invites = vec![X, X + 1];
    r.views.insert(X, ENEMY_AT - 1.0);
    r.views.insert(X + 1, 1.0);
    r.cands = vec![{
        let mut c = cand(X, 0.0);
        c.bot = true;
        c
    }];
    let acts = r.acts();
    assert!(
        acts.contains(&WarAct::Answer {
            from: X,
            accept: true
        }),
        "{acts:?}"
    );
    assert!(
        acts.contains(&WarAct::Answer {
            from: X + 1,
            accept: false
        }),
        "{acts:?}"
    );
    r.enabled = false;
    assert!(!r.acts().contains(&WarAct::Answer {
        from: X,
        accept: true
    }));
}

#[test]
fn an_ally_is_offered_help_in_a_war_by_a_blue_group() {
    let defender = 98_000_700;
    let aggressor = 98_000_701;
    let mut r = Rig::new();
    let mut w = row(9, aggressor, defender);
    w.open = true;
    r.open = vec![w];
    r.views.insert(aggressor, ENEMY_AT - 1.0);
    // Not blue to the defender: no help.
    assert!(r.acts().is_empty());
    r.blues.insert(defender);
    assert_eq!(r.acts(), vec![WarAct::OfferAlly { war: 9, defender }]);
    // The lag has not passed for a bloc follower, or the CEO is meek, or it
    // already offered today.
    r.lag_ok = false;
    assert!(r.acts().is_empty());
    r.lag_ok = true;
    r.aggression = ALLY_AGGRESSION - 0.1;
    assert!(r.acts().is_empty());
    r.aggression = 1.0;
    r.offered.insert((OWNER, 9), NOW);
    assert!(r.acts().is_empty());
    // A war that is closed to allies is not a call for help.
    r.offered.clear();
    r.open[0].open = false;
    assert!(r.acts().is_empty());
}

#[test]
fn a_defender_opens_its_war_to_allies_and_takes_a_blue_ones_offer() {
    let aggressor = 98_000_701;
    let mut r = Rig::new();
    r.wars = vec![row(9, aggressor, OWNER)];
    assert!(r.acts().is_empty(), "nobody to ask");
    r.mates = true;
    assert_eq!(r.acts(), vec![WarAct::OpenAllies(9)]);
    r.wars[0].open = true;
    let friend = 98_000_702;
    r.negs = vec![Neg {
        id: 4,
        war: 9,
        kind: NEG_ALLY,
        from: friend,
        to: OWNER,
        state: NEG_NEW,
        isk: 0.0,
    }];
    assert!(r.acts().is_empty(), "a stranger's offer waits");
    r.blues.insert(friend);
    assert_eq!(r.acts(), vec![WarAct::AcceptAlly(4)]);
}

#[test]
fn the_losing_side_offers_surrender_and_the_other_accepts_when_healed() {
    let other = 98_000_701;
    let mut r = Rig::new();
    r.wars = vec![row(9, OWNER, other)];
    r.lost.insert(other, 3_000.0);
    r.killed.insert(other, 3_000.0 / SURRENDER_LOSS - 1.0);
    let acts = r.acts();
    assert!(
        acts.iter()
            .any(|a| matches!(a, WarAct::OfferSurrender { war: 9, to, .. } if *to == other)),
        "{acts:?}"
    );
    // Winning, or an offer already made today, or a young war: nothing.
    r.killed.insert(other, 3_000.0 / SURRENDER_LOSS + 1.0);
    assert!(
        r.acts()
            .iter()
            .all(|a| !matches!(a, WarAct::OfferSurrender { .. }))
    );
    r.killed.insert(other, 0.0);
    r.offered.insert((OWNER, 9), NOW);
    assert!(
        r.acts()
            .iter()
            .all(|a| !matches!(a, WarAct::OfferSurrender { .. }))
    );
    r.offered.clear();
    r.wars[0].seen = NOW;
    assert!(
        r.acts()
            .iter()
            .all(|a| !matches!(a, WarAct::OfferSurrender { .. }))
    );
    // The receiving side: accepts at a healed view or when it is losing too,
    // not while it is winning and still angry.
    let mut r = Rig::new();
    r.negs = vec![Neg {
        id: 6,
        war: 9,
        kind: NEG_SURRENDER,
        from: other,
        to: OWNER,
        state: NEG_NEW,
        isk: 1.0,
    }];
    r.views.insert(other, PEACE_AT - 1.0);
    r.killed.insert(other, 1_000.0);
    assert!(r.acts().is_empty());
    r.views.insert(other, PEACE_AT);
    assert_eq!(
        r.acts(),
        vec![WarAct::AcceptSurrender {
            neg: 6,
            from: other
        }]
    );
    r.views.insert(other, PEACE_AT - 1.0);
    r.lost.insert(other, 5_000.0);
    assert_eq!(
        r.acts(),
        vec![WarAct::AcceptSurrender {
            neg: 6,
            from: other
        }]
    );
}

#[test]
fn the_bill_lapses_once_the_relation_has_recovered_and_is_paid_again_when_it_sours() {
    let other = 98_000_701;
    let mut r = Rig::new();
    r.wars = vec![row(9, OWNER, other)];
    r.views.insert(other, WAR_AT);
    assert!(
        r.acts().is_empty(),
        "still angry: the bill is paid, as it is"
    );
    r.views.insert(other, WAR_AT + PEACE_BAND + 0.5);
    assert_eq!(r.acts(), vec![WarAct::AutoPay(false)]);
    r.autopay = false;
    assert!(r.acts().is_empty());
    r.views.insert(other, WAR_AT);
    assert_eq!(r.acts(), vec![WarAct::AutoPay(true)]);
    // A young war is paid whatever the view says.
    r.autopay = true;
    r.views.insert(other, 0.0);
    r.wars[0].seen = NOW;
    assert!(r.acts().is_empty());
}

#[test]
fn losses_are_kept_by_group_and_bounded() {
    let mut s = State::default();
    s.lose(A1, A2, 10.0);
    s.lose(A1, A2, 5.0);
    s.lose(A2, A1, 1.0);
    s.lose(A1, A1, 9.0);
    s.lose(A1, A3, -3.0);
    s.lose(A1, A3, f64::NAN);
    assert_eq!(s.ledger.get(&(A1, A2)), Some(&15.0));
    assert_eq!(s.ledger.len(), 2);
    for i in 0..(LEDGER_KEEP as i64 + 10) {
        s.lose(1_000 + i, 5, 1.0);
    }
    assert!(s.ledger.len() <= LEDGER_KEEP);
    // The hall books a kill into it.
    let mut st = world(&[(C1, U1, Some(A1), 0)]);
    st.record(&Event::Kill {
        victim: A1,
        killer: X,
        isk: 7.0,
    });
    assert_eq!(st.war_ledger(A1, X), (7.0, 0.0));
}

#[test]
fn the_war_pass_is_due_when_the_switch_is_on_or_a_war_runs() {
    let mut st = world(&[(C1, U1, Some(A1), 0)]);
    assert!(!st.war_pass_due(A1, 0), "no switch, no war");
    st.coal.enabled = true;
    assert!(st.war_pass_due(A1, 0));
    st.coal.war_tried.insert(A1, 0);
    assert!(!st.war_pass_due(A1, 1), "within the cadence");
    assert!(st.war_pass_due(A1, (WAR_EVERY_DAYS * DAY as f64) as u64));
    st.coal.enabled = false;
    st.note_war(WarRow {
        started: eve_server::pyutil::filetime_now() - 10,
        ..row(1, X, A1)
    });
    assert!(
        st.war_pass_due(A1, (WAR_EVERY_DAYS * DAY as f64) as u64),
        "a war runs"
    );
}
