//! Pure tests of the observer. Every id and row is invented; every
//! tunable is named, never written as a number.

use super::*;
use crate::career::{CAREERS, Signals};
use crate::corpmind::{CorpMind, HallState, Member};
use crate::mind::{TRAIT_COUNT, Traits};
use crate::society::{Relations, Satisfaction};
use eve_server::pyutil::keyval;
use std::sync::Arc;

const ME: i64 = 90_000_001;
const ME_CORP: i64 = 98_000_001;
const FOE: i64 = 90_000_002;
const FOE_CORP: i64 = 98_000_002;
const MATE: i64 = 90_000_003;
const MATE_CORP: i64 = 98_000_003;
const THIRD: i64 = 90_000_004;
const THIRD_CORP: i64 = 98_000_004;
const SYSTEM: i64 = 30_000_001;
const FRIGATE: i64 = 999_001;
const POD: i64 = 999_002;
const NOW: u64 = 1_000;
const ISK: f64 = 5_000_000.0;

fn aff(c: i64, corp: i64) -> Affiliation {
    Affiliation {
        character_id: c,
        corporation_id: corp,
        alliance_id: None,
    }
}

fn hit(c: i64, corp: i64, damage: i64) -> Hit {
    Hit {
        who: aff(c, corp),
        ship: Some(FRIGATE),
        damage,
    }
}

fn seen(id: i64, victim: Affiliation, hits: Vec<Hit>) -> Seen {
    Seen {
        id,
        system: SYSTEM,
        victim,
        victim_ship: Some(FRIGATE),
        isk: ISK,
        hits,
        final_is_player: true,
    }
}

fn row_of(h: &Hit, final_blow: bool) -> PyValue {
    keyval(vec![
        ("characterID", PyValue::Int(h.who.character_id)),
        ("corporationID", PyValue::Int(h.who.corporation_id)),
        ("allianceID", PyValue::None),
        ("shipTypeID", PyValue::Int(h.ship.unwrap_or(0))),
        ("damageDone", PyValue::Int(h.damage)),
        ("finalBlow", PyValue::Int(i64::from(final_blow))),
    ])
}

fn member(corp: i64) -> Member {
    Member {
        name: "Invented".to_owned(),
        traits: Traits([0.5; TRAIT_COUNT]),
        career: "pirate".to_owned(),
        band: Band::Low,
        corp,
        alliance: None,
        last_turn: None,
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

fn hall(corps: &[(i64, i64)]) -> Hall {
    let hall = Hall::new(60);
    {
        let mut st = hall.lock();
        for &(corp, ceo) in corps {
            st.members.insert(ceo, member(corp));
            st.corps.insert(
                corp,
                CorpMind {
                    id: corp,
                    ceo: Some(ceo),
                    profile: society::Profile::of("pirate", Band::Low),
                    founded: 0,
                    alliance: None,
                    name: String::new(),
                    applied_alliance: None,
                    salt: 0,
                },
            );
        }
    }
    hall
}

fn mind() -> Mutex<Mind> {
    Mutex::new(Mind::seeded(
        "Invented Pilot",
        7,
        CAREERS,
        &Signals::default(),
        0,
    ))
}

fn regard(m: &Mutex<Mind>, who: i64) -> f64 {
    m.lock()
        .unwrap()
        .regard
        .iter()
        .find(|r| r.who == who)
        .map_or(0.0, |r| r.score)
}

// ---- reading rows ---------------------------------------------------------------------

#[test]
fn a_row_reads_every_attacker_with_the_final_blow_first() {
    let (a, b) = (hit(FOE, FOE_CORP, 10), hit(MATE, MATE_CORP, 90));
    let row = keyval(vec![
        ("killID", PyValue::Int(5)),
        ("solarSystemID", PyValue::Int(SYSTEM)),
        ("victimCharacterID", PyValue::Int(ME)),
        ("victimCorporationID", PyValue::Int(ME_CORP)),
        ("victimAllianceID", PyValue::None),
        ("victimShipTypeID", PyValue::Int(FRIGATE)),
        ("iskLost", PyValue::Real(ISK)),
        // Listed with the final blow second to prove the sort.
        (
            "attackers",
            PyValue::List(vec![row_of(&b, false), row_of(&a, true)]),
        ),
    ]);
    let got = seen_of(&[row]);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].hits, vec![a, b]);
    assert!(got[0].final_is_player);
    assert_eq!(got[0].victim, aff(ME, ME_CORP));
}

#[test]
fn a_row_without_an_attackers_list_reads_as_its_final_blow() {
    let row = keyval(vec![
        ("killID", PyValue::Int(5)),
        ("victimCharacterID", PyValue::Int(ME)),
        ("victimCorporationID", PyValue::Int(ME_CORP)),
        ("finalCharacterID", PyValue::Int(FOE)),
        ("finalCorporationID", PyValue::Int(FOE_CORP)),
        ("finalDamageDone", PyValue::Int(7)),
    ]);
    let got = seen_of(&[row]);
    assert_eq!(got[0].hits.len(), 1);
    assert_eq!(got[0].hits[0].who, aff(FOE, FOE_CORP));
}

#[test]
fn an_npc_only_row_and_a_suicide_make_no_seen() {
    let row = keyval(vec![
        ("killID", PyValue::Int(5)),
        ("victimCharacterID", PyValue::Int(ME)),
        ("victimCorporationID", PyValue::Int(ME_CORP)),
        (
            "attackers",
            PyValue::List(vec![keyval(vec![
                ("characterID", PyValue::None),
                ("finalBlow", PyValue::Int(1)),
            ])]),
        ),
    ]);
    assert!(seen_of(&[row]).is_empty());
}

// ---- the facts ------------------------------------------------------------------------

fn nobody_allied(_: &Affiliation, _: &Affiliation) -> bool {
    false
}

#[test]
fn a_two_attacker_kill_records_an_assist_for_both_sides() {
    let s = seen(
        1,
        aff(ME, ME_CORP),
        vec![
            hit(FOE, FOE_CORP, 60),
            hit(MATE, MATE_CORP, 40),
            hit(THIRD, THIRD_CORP, 1),
        ],
    );
    let facts = kill_facts(&s, None, &nobody_allied, NOW);
    let who: Vec<i64> = facts.iter().map(|f| f.hit.who.character_id).collect();
    // The stray shot (under ASSIST_SHARE) is no party to it.
    assert_eq!(who, vec![FOE, MATE]);
    assert!(facts[0].final_blow && !facts[1].final_blow);
    // Both the victim's loss and each attacker's victory are facts.
    assert!(
        facts
            .iter()
            .all(|f| f.loss.kind == FactKind::Loss && f.victory.kind == FactKind::Victory)
    );
}

#[test]
fn the_class_and_security_of_a_kill_come_from_the_ground() {
    let ground = Ground::new()
        .with_class(FRIGATE, Class::Small)
        .with_class(POD, Class::Capsule)
        .with_security(SYSTEM, 0.9);
    let mut s = seen(1, aff(ME, ME_CORP), vec![hit(FOE, FOE_CORP, 1)]);
    let k = kill_of(&s, Some(&ground));
    assert!(!k.pod);
    assert_eq!(k.band, Band::High);
    assert_eq!(k.gang, 1);
    s.victim_ship = Some(POD);
    assert!(kill_of(&s, Some(&ground)).pod);
    // No ground: low sec, nothing a pod.
    let k = kill_of(&s, None);
    assert_eq!((k.band, k.pod), (Band::Low, false));
}

// ---- the matrix, through the model ----------------------------------------------------

/// The old `HallState::record`, as it was before R-REL-2.
fn old_record(st: &mut HallState, e: &Event) {
    let mut scratch = Relations::default();
    scratch.apply(e);
    for (&(v, t), &d) in &scratch.r {
        let t = if society::is_player_corp(t) {
            st.group_of_corp(t)
        } else {
            t
        };
        let viewers: Vec<i64> = st.bot_corps().into_iter().filter(|c| *c == v).collect();
        for viewer in viewers {
            st.relations.add(viewer, t, d);
        }
    }
}

#[test]
fn recording_an_event_through_the_model_equals_the_old_matrix() {
    let events = [
        Event::Kill {
            victim: ME_CORP,
            killer: FOE_CORP,
            isk: ISK,
        },
        Event::Kill {
            victim: ME_CORP,
            killer: FOE_CORP,
            isk: 0.0,
        },
        Event::Siege {
            defender: ME_CORP,
            attacker: FOE_CORP,
        },
        Event::Trade {
            a: ME_CORP,
            b: FOE_CORP,
            isk: 3.0e9,
        },
        Event::AllyFight {
            a: ME_CORP,
            b: FOE_CORP,
        },
        Event::Contest {
            a: ME_CORP,
            b: FOE_CORP,
            days: 1.0e6,
        },
        Event::Neighbour {
            a: ME_CORP,
            b: FOE_CORP,
            days: 2.5,
        },
        // Same group on both sides: a no-op in both.
        Event::Kill {
            victim: ME_CORP,
            killer: ME_CORP,
            isk: ISK,
        },
    ];
    let new = hall(&[(ME_CORP, ME), (FOE_CORP, FOE)]);
    let mut old = new.snapshot();
    for e in &events {
        new.record(e);
        old_record(&mut old, e);
    }
    assert_eq!(new.snapshot().relations, old.relations);
    assert!(!old.relations.r.is_empty());
}

// ---- folding a row --------------------------------------------------------------------

#[test]
fn a_kill_by_a_pilot_raises_a_grudge_against_him_and_the_assister_corp_goes_red() {
    let h = hall(&[(ME_CORP, ME), (MATE_CORP, MATE)]);
    let m = mind();
    let s = seen(
        9,
        aff(ME, ME_CORP),
        vec![hit(FOE, FOE_CORP, 60), hit(MATE, MATE_CORP, 40)],
    );
    let got = fold_row(&h, &m, ME, &s, NOW);
    assert!(got.personal >= 2, "{got:?}");
    assert!(regard(&m, FOE) < 0.0, "a grudge against the final blow");
    assert!(regard(&m, MATE) < 0.0, "and against the assister");
    // The assister's corp is red toward the victim's corp view, at the spill.
    let r = h.lock().relations.get(ME_CORP, MATE_CORP);
    assert!(
        r < 0.0 && r > -society::K_KILL * society::VALUE_FACTOR_MAX,
        "{r}"
    );
    // The final blow's corp level is the event's, not this fold's.
    assert_eq!(h.lock().relations.get(ME_CORP, FOE_CORP), 0.0);
}

#[test]
fn a_report_is_counted_once_across_bots() {
    let h = hall(&[(ME_CORP, ME), (MATE_CORP, MATE)]);
    let (m1, m2) = (mind(), mind());
    let s = seen(
        9,
        aff(ME, ME_CORP),
        vec![hit(FOE, FOE_CORP, 60), hit(MATE, MATE_CORP, 40)],
    );
    let first = fold_row(&h, &m1, ME, &s, NOW);
    assert!(first.personal > 0);
    let snap = h.snapshot().relations;
    // The same bot again: nothing.
    assert_eq!(fold_row(&h, &m1, ME, &s, NOW + 1), Folded::default());
    // The assister reads the same report: its own regard moves, the
    // hall-wide part does not repeat.
    let other = fold_row(&h, &m2, MATE, &s, NOW + 2);
    assert_eq!(other.hall, 0);
    assert_eq!(h.snapshot().relations, snap);
}

#[test]
fn a_fair_victory_is_respect_and_a_gank_victory_is_none() {
    let ground = Arc::new(Ground::new().with_class(FRIGATE, Class::Small));
    let h = hall(&[(ME_CORP, ME)]).with_ground(ground);
    let m = mind();
    // One frigate against one frigate: fair.
    let fair = seen(1, aff(FOE, FOE_CORP), vec![hit(ME, ME_CORP, 1)]);
    fold_row(&h, &m, ME, &fair, NOW);
    assert_eq!(regard(&m, FOE), relations::K_RESPECT);
    // Five frigates on one: a gank, no respect.
    let m2 = mind();
    let gang: Vec<Hit> = (0..relations::GANK_GANG as i64)
        .map(|i| hit(ME + 100 + i, ME_CORP, 1))
        .chain(std::iter::once(hit(ME, ME_CORP, 1)))
        .collect();
    let gank = seen(2, aff(FOE, FOE_CORP), gang);
    fold_row(&h, &m2, ME, &gank, NOW);
    assert_eq!(regard(&m2, FOE), 0.0);
}

#[test]
fn a_betrayal_weighs_more_than_the_same_loss_by_a_stranger() {
    let loss = |befriend: bool| {
        let h = hall(&[(ME_CORP, ME)]);
        if befriend {
            h.befriend(FOE, 3.0);
        }
        let m = mind();
        let s = seen(1, aff(ME, ME_CORP), vec![hit(FOE, FOE_CORP, 1)]);
        fold_row(&h, &m, ME, &s, NOW);
        regard(&m, FOE)
    };
    let (stranger, friend) = (loss(false), loss(true));
    assert!(friend < stranger && stranger < 0.0, "{friend} {stranger}");
}

#[test]
fn a_mate_on_my_kill_earns_help_and_a_friend_victim_does_not() {
    let h = hall(&[(ME_CORP, ME), (MATE_CORP, MATE)]);
    let m = mind();
    let s = seen(
        3,
        aff(FOE, FOE_CORP),
        vec![hit(ME, ME_CORP, 50), hit(MATE, MATE_CORP, 50)],
    );
    fold_row(&h, &m, ME, &s, NOW);
    assert!(regard(&m, MATE) > 0.0, "help");
    // The victim is a friend of mine: no help for shooting him.
    let h2 = hall(&[(ME_CORP, ME)]);
    h2.befriend(FOE, 3.0);
    let m2 = mind();
    fold_row(&h2, &m2, ME, &s, NOW);
    assert_eq!(regard(&m2, MATE), 0.0);
}

#[test]
fn the_pass_puts_an_assist_into_the_matrix_as_the_event() {
    // I only assisted: neither victim nor final blow, so nobody else
    // would put the final blow into the matrix.
    let h = hall(&[(THIRD_CORP, THIRD)]);
    let m = mind();
    let s = seen(
        4,
        aff(THIRD, THIRD_CORP),
        vec![hit(FOE, FOE_CORP, 70), hit(ME, ME_CORP, 30)],
    );
    pass(&h, &m, ME, std::slice::from_ref(&s), NOW);
    assert!(h.lock().relations.get(THIRD_CORP, FOE_CORP) < 0.0);
    // Once, however many times it is read.
    let snap = h.snapshot().relations;
    pass(&h, &m, ME, &[s], NOW + 1);
    assert_eq!(h.snapshot().relations, snap);
}

// ---- contracts, wars, mail ------------------------------------------------------------

fn contract(status: i64, issuer: i64, acceptor: i64) -> PyValue {
    keyval(vec![
        ("contractID", PyValue::Int(77)),
        ("issuerID", PyValue::Int(issuer)),
        ("acceptorID", PyValue::Int(acceptor)),
        ("status", PyValue::Int(status)),
        ("price", PyValue::Real(ISK)),
        ("reward", PyValue::Real(0.0)),
    ])
}

#[test]
fn a_completed_contract_records_f8_with_the_counterparty() {
    let me = aff(ME, ME_CORP);
    let other = |c: i64| aff(c, FOE_CORP);
    let (id, f) = contract_fact(&contract(STATUS_FINISHED, ME, FOE), &me, other, NOW).unwrap();
    assert_eq!(
        (id, f.kind, f.actor, f.target.character_id),
        (77, FactKind::Trade, me, FOE)
    );
    // As the acceptor, the issuer is the counterparty.
    let (_, f) = contract_fact(&contract(STATUS_FINISHED, FOE, ME), &me, other, NOW).unwrap();
    assert_eq!((f.kind, f.target.character_id), (FactKind::Trade, FOE));
    // Nobody accepted, or it was rejected: no fact.
    assert!(contract_fact(&contract(STATUS_FINISHED, ME, 0), &me, other, NOW).is_none());
    assert!(contract_fact(&contract(6, ME, FOE), &me, other, NOW).is_none());
}

#[test]
fn an_accepted_courier_that_failed_is_a_scam_fact_against_the_acceptor() {
    let me = aff(ME, ME_CORP);
    let (_, f) = contract_fact(
        &contract(STATUS_FAILED, ME, FOE),
        &me,
        |c| aff(c, FOE_CORP),
        NOW,
    )
    .unwrap();
    assert_eq!(
        (f.kind, f.actor.character_id, f.target.character_id),
        (FactKind::Scam, FOE, ME)
    );
    // A courier I failed myself is nobody's fact here.
    assert!(
        contract_fact(
            &contract(STATUS_FAILED, FOE, ME),
            &me,
            |c| aff(c, FOE_CORP),
            NOW
        )
        .is_none()
    );
}

#[test]
fn a_trade_improves_regard_and_a_scam_sours_it() {
    let t = Temper::NORMAL;
    let me = aff(ME, ME_CORP);
    let m = mind();
    let (_, trade) = contract_fact(
        &contract(STATUS_FINISHED, ME, FOE),
        &me,
        |c| aff(c, FOE_CORP),
        NOW,
    )
    .unwrap();
    apply_personal(&m, ME, &relations::deltas(&trade, &t), NOW, 60, &t);
    assert!(regard(&m, FOE) > 0.0);
    let (_, scam) = contract_fact(
        &contract(STATUS_FAILED, ME, MATE),
        &me,
        |c| aff(c, MATE_CORP),
        NOW,
    )
    .unwrap();
    apply_personal(&m, ME, &relations::deltas(&scam, &t), NOW, 60, &t);
    assert!(regard(&m, MATE) < 0.0);
}

fn war(id: i64, by: i64, against: i64, finished: bool, allies: &[i64]) -> PyValue {
    keyval(vec![
        ("warID", PyValue::Int(id)),
        ("declaredByID", PyValue::Int(by)),
        ("againstID", PyValue::Int(against)),
        (
            "timeFinished",
            if finished {
                PyValue::Int(5)
            } else {
                PyValue::None
            },
        ),
        (
            "allies",
            PyValue::Dict(
                allies
                    .iter()
                    .map(|a| (keyval(vec![("allyID", PyValue::Int(*a))]), PyValue::Int(*a)))
                    .collect(),
            ),
        ),
    ])
}

#[test]
fn a_war_on_my_group_is_f10_an_end_is_f11_and_an_ally_f12() {
    let mine = [ME_CORP];
    let kinds = |w: &PyValue| -> Vec<FactKind> {
        war_facts(w, &mine, NOW)
            .into_iter()
            .map(|(_, f)| f.kind)
            .collect()
    };
    assert_eq!(
        kinds(&war(1, FOE_CORP, ME_CORP, false, &[])),
        vec![FactKind::WarOn]
    );
    // I declared it: not a war on me.
    assert!(kinds(&war(1, ME_CORP, FOE_CORP, false, &[])).is_empty());
    assert_eq!(
        kinds(&war(1, FOE_CORP, ME_CORP, true, &[MATE_CORP])),
        vec![FactKind::WarOn, FactKind::Peace, FactKind::WarAlly]
    );
    // A war that is not mine says nothing.
    assert!(kinds(&war(1, FOE_CORP, THIRD_CORP, true, &[])).is_empty());
}

#[test]
fn a_war_on_me_lowers_the_alliance_own_view_of_the_declarer_once() {
    let h = hall(&[(ME_CORP, ME)]);
    let al = 99_000_001;
    let w = war(2, FOE_CORP, al, false, &[]);
    let mine = [ME_CORP, al];
    for _ in 0..2 {
        for ((who, id), f) in war_facts(&w, &mine, NOW) {
            let mut st = h.lock();
            if st.first(KIND_WAR, who, id) {
                let ds = relations::deltas(&f, &st.temper);
                st.apply_deltas(&ds);
            }
        }
    }
    assert_eq!(
        h.lock().alliance_own.get(al, FOE_CORP),
        -relations::K_WAR_ON
    );
}

#[test]
fn a_reply_within_the_window_counts_as_an_accept() {
    let header = |title: &str, sender: i64| {
        keyval(vec![
            ("messageID", PyValue::Int(11)),
            ("senderID", PyValue::Int(sender)),
            ("title", PyValue::Unicode(title.to_owned())),
        ])
    };
    let reply = crate::mail::heard(&header("Re: Ours: blue standing", FOE), ME).unwrap();
    assert!(reply.reply_to_offer && reply.sender == FOE);
    let plain = crate::mail::heard(&header("hello", FOE), ME).unwrap();
    assert!(!plain.reply_to_offer);
    // My own sent copy is no talk.
    assert!(crate::mail::heard(&header("hello", ME), ME).is_none());
    // Talk is worth more when it answers an offer.
    let t = Temper::NORMAL;
    let talk = |accepted| Fact {
        kind: FactKind::Talk,
        actor: aff(FOE, FOE_CORP),
        target: aff(ME, ME_CORP),
        isk: 0.0,
        ctx: Ctx {
            accepted_offer: accepted,
            ..Ctx::default()
        },
        at: NOW,
    };
    let gain = |f: &Fact| {
        relations::deltas(f, &t)
            .iter()
            .find(|d| d.level == Level::Personal && d.holder == ME)
            .map(|d| d.d)
            .unwrap()
    };
    assert!(gain(&talk(true)) > gain(&talk(false)));
}

// ---- the state file, the fleet --------------------------------------------------------

#[test]
fn the_alliance_own_store_and_the_observed_keys_survive_a_state_file() {
    let h = hall(&[(ME_CORP, ME)]);
    {
        let mut st = h.lock();
        st.alliance_own.add(99_000_001, FOE_CORP, -2.5);
        assert!(st.first(KIND_WAR, 0, 8));
        assert!(!st.first(KIND_WAR, 0, 8));
    }
    let text = h.lock().render();
    let back = HallState::parse(&text, 60).expect("parses");
    assert_eq!(back.alliance_own, h.lock().alliance_own);
    assert_eq!(back.observed, h.lock().observed);
    // An old file (no such rows) still loads.
    let old = hall(&[(ME_CORP, ME)]).lock().render();
    assert!(HallState::parse(&old, 60).is_ok());
}

#[test]
fn shared_fleet_time_is_in_game_hours() {
    let joined = std::collections::BTreeMap::from([(MATE, 100u64), (FOE, 460u64)]);
    // A game-day of 240 seconds: ten seconds an hour.
    let got = crate::fleet::shared_hours(&joined, 500, 240);
    assert_eq!(got, vec![(FOE, 4.0), (MATE, 40.0)]);
}

#[test]
fn review_fix_one_talk_per_sender_per_game_day() {
    let h = hall(&[(ME_CORP, ME)]);
    let day = h.lock().day;
    assert_eq!(talk_gate(&h, ME, FOE, false, NOW), Some(false));
    // The same sender, the same day, again: nothing.
    assert_eq!(talk_gate(&h, ME, FOE, false, NOW), None);
    // Another sender is a fresh talk; the next day the first one counts again.
    assert!(talk_gate(&h, ME, MATE, false, NOW).is_some());
    assert!(talk_gate(&h, ME, FOE, false, NOW + day).is_some());
}

#[test]
fn review_fix_the_accept_bonus_needs_an_offer_this_bot_sent() {
    let h = hall(&[(ME_CORP, ME)]);
    // A "Re: blue standing" from a pilot nobody offered anything to.
    assert_eq!(talk_gate(&h, ME, FOE, true, NOW), Some(false));
    h.lock().offers.insert((ME, MATE));
    assert_eq!(talk_gate(&h, ME, MATE, true, NOW), Some(true));
    // An offer is per bot: another bot's reply does not count.
    assert_eq!(talk_gate(&h, THIRD, MATE, true, NOW), Some(false));
}
