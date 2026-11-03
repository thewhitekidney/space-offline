//! R-REL-4 unit tests. Invented ids; no `[A]` value is an expected value
//! (the assertions are about signs, tiers and counts, with the constants
//! named).

use super::*;
use crate::career::Band;
use crate::corpmind::{CorpMind, Member, Seen};
use crate::effects;
use crate::mind::{Regard, TRAIT_COUNT, Traits};
use crate::relations::{Kill, Temper};
use crate::society::{Event, Profile, Satisfaction};
use std::collections::BTreeMap;

const DAY: u64 = 240;
const C1: i64 = 98_000_401;
const C2: i64 = 98_000_402;
const ANN: i64 = 90_000_401;
const CAS: i64 = 90_000_402;
const FRIEND: i64 = 90_000_411;
const STRANGER: i64 = 90_000_412;
const HELPER: i64 = 90_000_413;
const NPC: i64 = 1_000_001;

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

fn corp(id: i64, ceo: i64) -> CorpMind {
    CorpMind {
        id,
        ceo: Some(ceo),
        profile: Profile::of("pirate", Band::Low),
        founded: 0,
        alliance: None,
        name: String::new(),
        applied_alliance: None,
        salt: 0,
    }
}

fn hall() -> HallState {
    let mut st = HallState::new(DAY);
    st.members.insert(ANN, member(C1));
    st.members.insert(CAS, member(C2));
    st.corps.insert(C1, corp(C1, ANN));
    st.corps.insert(C2, corp(C2, CAS));
    st
}

fn human(id: i64) -> Affiliation {
    Affiliation {
        character_id: id,
        corporation_id: NPC,
        alliance_id: None,
    }
}

fn bot(id: i64, corp: i64) -> Affiliation {
    Affiliation {
        character_id: id,
        corporation_id: corp,
        alliance_id: None,
    }
}

fn r(st: &HallState, regard: &[Regard], who: i64, now: u64) -> f64 {
    st.r_eff_of(C1, regard, 0.5, &human(who), now)
}

/// A loss of a bot to `who` at `gang` attackers worth the reference value.
fn loss(who: i64, gang: usize) -> Fact {
    Fact {
        kind: FactKind::Loss,
        actor: human(who),
        target: bot(ANN, C1),
        isk: society::KILL_VALUE_REF,
        ctx: Ctx {
            kill: Kill {
                gang,
                ..Ctx::default().kill
            },
            ..Ctx::default()
        },
        at: 0,
    }
}

fn help(who: i64) -> Fact {
    Fact {
        kind: FactKind::Help,
        actor: human(who),
        target: bot(ANN, C1),
        isk: society::KILL_VALUE_REF,
        ctx: Ctx::default(),
        at: 0,
    }
}

/// Apply a fact as the observer does: the bot's own regard and the hall.
fn apply(st: &mut HallState, regard: &mut Vec<Regard>, f: &Fact, now: u64) {
    let ds = relations::deltas(f, &st.temper);
    let half = relations::personal_half_days(0.5, &st.temper);
    for d in ds
        .iter()
        .filter(|d| d.level == relations::Level::Personal && d.holder == ANN)
    {
        relations::regard_add(regard, d.subject, d.d, now, DAY, half);
    }
    st.apply_deltas(&ds);
}

#[test]
fn a_presence_clock_credits_caps_and_restarts() {
    let hour = DAY / 24;
    let (c, h) = tick(None, 1_000, DAY);
    assert_eq!(h, 0.0, "the first tick credits nothing");
    let (c, h) = tick(Some(c), 1_000 + hour / 2, DAY);
    assert!((h - 0.5).abs() < 0.01, "half an hour: {h}");
    // A long gap restarts the clock with no credit.
    let far = c.last + (PRESENCE_GAP_HOURS as u64 + 2) * hour;
    let (c2, h) = tick(Some(c), far, DAY);
    assert_eq!(h, 0.0);
    assert_eq!(c2.last, far);
    // A game-day credits at most the cap, a new day starts afresh.
    let mut c = Clock {
        last: 0,
        day: 0,
        today: 0.0,
    };
    let mut total = 0.0;
    let mut t = 0;
    while t < DAY - hour {
        t += hour / 2;
        let (n, h) = tick(Some(c), t, DAY);
        c = n;
        total += h;
    }
    assert!(total <= FLEET_DAY_CAP_HOURS + 1e-9, "{total}");
    let spent = Clock {
        last: DAY - 1,
        day: 0,
        today: FLEET_DAY_CAP_HOURS,
    };
    assert_eq!(tick(Some(spent), DAY - 1, DAY).1, 0.0, "the cap holds");
    let (_, h) = tick(Some(spent), DAY + 1, DAY);
    assert!(h > 0.0, "a new day credits again");
}

#[test]
fn the_tier_table_agrees_with_the_effects_that_read_r() {
    use effects::Gate;
    assert!(unlocks(Stance5::Hostile).refused);
    assert!(matches!(
        effects::counterparty_gate(society::ENEMY_AT),
        Gate::Refuse
    ));
    assert!(unlocks(Stance5::Wary).premium);
    assert!(matches!(
        effects::counterparty_gate(society::WARY_AT),
        Gate::Premium(_)
    ));
    assert!(unlocks(Stance5::Allied).discount);
    assert!(matches!(
        effects::counterparty_gate(society::ALLY_AT),
        Gate::Discount(_)
    ));
    for t in [Stance5::Hostile, Stance5::Wary] {
        assert!(unlocks(t).hunted && !unlocks(t).invites && !unlocks(t).backup);
    }
    for t in [Stance5::Friendly, Stance5::Allied] {
        assert!(!unlocks(t).hunted && unlocks(t).invites && unlocks(t).backup);
    }
    let n = unlocks(Stance5::Neutral);
    assert!(!n.hunted && !n.invites && !n.discount && !n.premium && !n.refused);
}

#[test]
fn a_friends_file_sets_priors_at_start() {
    // The file names the pilot; the bot's regard is set to its standing and
    // wins over what the bot had learned.
    let pairs = crate::friends::load(&format!("{FRIEND}\t4\n"), |_| None).unwrap();
    let mut regard = vec![Regard {
        who: FRIEND,
        score: -6.0,
        at: 0,
    }];
    relations::seed_friends(&mut regard, &pairs, 10);
    assert_eq!(regard[0].score, 4.0, "the file wins");
    let mut st = hall();
    for (id, s) in &pairs {
        st.friends.insert(*id, *s);
    }
    st.see_human(&human(FRIEND), 10);
    assert!(r(&st, &[], FRIEND, 10) >= 4.0);
    assert!(r(&st, &regard, FRIEND, 10) >= 4.0);
    // And the corps tell the friend so, once.
    let ch = st.tier_changes(C1, &regard, 0.5, 10, 5);
    assert_eq!(ch.len(), 1);
    assert_eq!(
        (ch[0].from, ch[0].to),
        (Stance5::Neutral, Stance5::Friendly)
    );
    assert!(st.tier_changes(C1, &regard, 0.5, 11, 5).is_empty());
}

#[test]
fn a_prior_decays_but_fleet_hours_renew_it() {
    let st = hall();
    let half = relations::personal_half_days(0.5, &st.temper);
    let mut regard = Vec::new();
    relations::seed_friends(&mut regard, &[(FRIEND, 4.0)], 0);
    let at_start = relations::regard_decayed(&regard[0], 0, DAY, half);
    let later = (half * DAY as f64) as u64;
    let faded = relations::regard_decayed(&regard[0], later, DAY, half);
    assert!(faded < at_start * 0.6, "the prior decays: {faded}");
    // A fleet hour renews it.
    let f = Fact {
        kind: FactKind::Fleet,
        actor: human(FRIEND),
        target: bot(ANN, C1),
        isk: 0.0,
        ctx: Ctx {
            hours: 4.0,
            ..Ctx::default()
        },
        at: later,
    };
    for d in relations::deltas(&f, &st.temper)
        .iter()
        .filter(|d| d.level == relations::Level::Personal && d.holder == ANN)
    {
        relations::regard_add(&mut regard, d.subject, d.d, later, DAY, half);
    }
    let renewed = relations::regard_decayed(&regard[0], later, DAY, half);
    assert!(renewed > faded, "{renewed} > {faded}");
}

#[test]
fn a_listed_friend_is_never_hunted_after_one_fair_loss() {
    let mut st = hall();
    st.friends.insert(FRIEND, 3.0);
    let mut regard = Vec::new();
    // The friend kills a bot in a fair fight, and the bot's corp hears it.
    apply(&mut st, &mut regard, &loss(FRIEND, 1), 5);
    st.record(&Event::Kill {
        victim: C1,
        killer: society::group_of(&human(FRIEND)),
        isk: society::KILL_VALUE_REF,
    });
    assert!(r(&st, &regard, FRIEND, 5) >= 3.0, "the file is the floor");
    assert!(!st.hostile_to(bot(ANN, C1), human(FRIEND)));
    // An unlisted pilot in the same fight is not spared.
    let mut other = Vec::new();
    apply(&mut st, &mut other, &loss(STRANGER, 1), 5);
    assert!(r(&st, &other, STRANGER, 5) < 0.0);
    assert!(st.hostile_to(bot(ANN, C1), human(STRANGER)));
}

#[test]
fn a_gank_of_a_bot_makes_a_human_wary_within_two_events() {
    let mut st = hall();
    st.see_human(&human(STRANGER), 0);
    let mut regard = Vec::new();
    let mut events = 0;
    while r(&st, &regard, STRANGER, 1) > society::WARY_AT && events < 2 {
        apply(
            &mut st,
            &mut regard,
            &loss(STRANGER, relations::GANK_GANG),
            1,
        );
        events += 1;
    }
    assert!(
        r(&st, &regard, STRANGER, 1) <= society::WARY_AT,
        "{events} events"
    );
    let ch = st.tier_changes(C1, &regard, 0.5, 1, 5);
    assert_eq!(ch.len(), 1);
    assert!(ch[0].to <= Stance5::Wary, "{:?}", ch[0]);
    assert!(unlocks(ch[0].to).hunted);
    // A gank weighs more than a fair loss.
    let mut a = Vec::new();
    let mut fair = hall();
    apply(&mut fair, &mut a, &loss(STRANGER, 1), 1);
    assert!(r(&fair, &a, STRANGER, 1) > r(&st, &regard, STRANGER, 1));
}

#[test]
fn an_unlisted_human_starts_at_the_difficulty_prior() {
    for t in [Temper::EASY, Temper::NORMAL, Temper::HARD, Temper::BRUTAL] {
        let mut st = hall();
        st.temper = t;
        assert_eq!(r(&st, &[], STRANGER, 0), t.human_prior);
    }
    // A listed friend is at least the file's standing, whatever the prior.
    let mut st = hall();
    st.temper = Temper::BRUTAL;
    st.friends.insert(FRIEND, 3.0);
    assert!(r(&st, &[], FRIEND, 0) >= 3.0);
}

#[test]
fn a_human_who_helps_rises_a_tier_and_is_then_invited() {
    let mut st = hall();
    st.see_human(&human(HELPER), 0);
    let mut regard = Vec::new();
    assert!(st.tier_changes(C1, &regard, 0.5, 0, 5).is_empty());
    let mut helps = 0;
    while r(&st, &regard, HELPER, 1) < effects::INVITE_AT && helps < 20 {
        apply(&mut st, &mut regard, &help(HELPER), 1);
        helps += 1;
    }
    assert!(helps >= 2, "one fight is not a friendship");
    let ch = st.tier_changes(C1, &regard, 0.5, 1, 5);
    assert_eq!(ch.len(), 1);
    assert!(ch[0].to >= Stance5::Friendly, "{:?}", ch[0]);
    assert!(unlocks(ch[0].to).invites);
    // The invitation also wants the shared fleet hours.
    assert!(st.invitations(C1, 1).is_empty());
    st.note_fleet_hours(HELPER, effects::INVITE_FLEET_HOURS);
    assert_eq!(st.invitations(C1, 1), vec![HELPER]);
    assert!(effects::may_invite(
        r(&st, &regard, HELPER, 1),
        effects::INVITE_FLEET_HOURS,
        false
    ));
}

#[test]
fn a_tier_is_told_once_and_the_cap_defers_the_rest() {
    let mut st = hall();
    let mut regard = Vec::new();
    for id in [HELPER, STRANGER, FRIEND] {
        st.see_human(&human(id), 0);
    }
    // A first meeting at neutral says nothing.
    assert!(st.tier_changes(C1, &regard, 0.5, 0, 5).is_empty());
    assert_eq!(st.tiers.len(), 3, "but is remembered");
    for id in [HELPER, STRANGER, FRIEND] {
        for _ in 0..4 {
            apply(&mut st, &mut regard, &help(id), 1);
        }
    }
    let first = st.tier_changes(C1, &regard, 0.5, 1, 2);
    assert_eq!(first.len(), 2, "capped");
    let second = st.tier_changes(C1, &regard, 0.5, 2, 2);
    assert_eq!(second.len(), 1, "the rest waits");
    assert!(st.tier_changes(C1, &regard, 0.5, 3, 2).is_empty());
}

#[test]
fn a_tier_does_not_flap_inside_its_band() {
    let mut st = hall();
    st.see_human(&human(HELPER), 0);
    st.tier_changes(C1, &[], 0.5, 0, 5);
    st.tiers.insert((C1, HELPER), (Stance5::Friendly, 0));
    // Just under the entry bar but over the leave point: still friendly.
    let mut regard = vec![Regard {
        who: HELPER,
        score: (society::FRIEND_AT + society::FRIEND_LEAVE) / 2.0,
        at: 0,
    }];
    assert!(st.tier_changes(C1, &regard, 0.5, 0, 5).is_empty());
    regard[0].score = 0.0;
    let ch = st.tier_changes(C1, &regard, 0.5, 0, 5);
    assert_eq!(
        (ch[0].from, ch[0].to),
        (Stance5::Friendly, Stance5::Neutral)
    );
}

#[test]
fn fleet_presence_is_credited_once_hall_wide_and_a_hostile_earns_nothing() {
    let hall = Hall::new(DAY).with_every(0);
    {
        let mut st = hall.lock();
        *st = {
            let mut s = self::hall();
            s.members.insert(CAS + 100, member(C1));
            s
        };
    }
    let (m1, m2) = (
        Mutex::new(Mind::seeded(
            "One",
            1,
            crate::career::CAREERS,
            &crate::career::Signals::default(),
            0,
        )),
        Mutex::new(Mind::seeded(
            "Two",
            2,
            crate::career::CAREERS,
            &crate::career::Signals::default(),
            0,
        )),
    );
    let (b1, b2) = (bot(ANN, C1), bot(CAS + 100, C1));
    let mates = [human(HELPER), human(STRANGER)];
    // A grudge: the stranger is one this bot would not credit.
    m1.lock().unwrap().regard.push(Regard {
        who: STRANGER,
        score: -8.0,
        at: 0,
    });
    let hour = DAY / 24;
    for t in 0..8u64 {
        let now = 1_000 + t * hour / 4;
        presence(&hall, &m1, &b1, &mates, now);
        presence(&hall, &m2, &b2, &mates, now);
    }
    let st = hall.lock();
    let wide = st.fleet_hours.get(&HELPER).copied().unwrap_or(0.0);
    assert!(wide > 1.0, "credited: {wide}");
    assert!(wide < 2.0 + 1e-9, "once, not once per bot: {wide}");
    assert!(st.humans.contains_key(&HELPER), "and remembered");
    drop(st);
    let own = |m: &Mutex<Mind>, who| {
        m.lock()
            .unwrap()
            .regard
            .iter()
            .find(|r| r.who == who)
            .map_or(0.0, |r| r.score)
    };
    assert!(
        own(&m1, HELPER) > 0.0 && own(&m2, HELPER) > 0.0,
        "each bot's own regard"
    );
    assert!(own(&m1, STRANGER) < 0.0, "the grudge stands");
    assert!(
        !hall.lock().fleet_clock.contains_key(&(ANN, STRANGER)),
        "a grudge earns nothing"
    );
    assert!(
        hall.lock()
            .fleet_hours
            .get(&STRANGER)
            .copied()
            .unwrap_or(0.0)
            > 0.0
    );
}

#[test]
fn the_humans_standing_survives_a_restart() {
    let mut st = hall();
    st.humans.insert(
        HELPER,
        Seen {
            corp: NPC,
            alliance: Some(99_000_401),
            at: 77,
        },
    );
    st.fleet_hours.insert(HELPER, 2.5);
    st.tiers.insert((C1, HELPER), (Stance5::Friendly, 80));
    st.relations.add(C1, HELPER, 3.0);
    let back = HallState::parse(&st.render(), DAY).expect("parses");
    assert_eq!(back.humans, st.humans);
    assert_eq!(back.fleet_hours, st.fleet_hours);
    assert_eq!(back.tiers, st.tiers);
    assert_eq!(back.relations.get(C1, HELPER), 3.0);
    // The file's standing wins again on the new run.
    let mut back = back;
    back.friends.insert(HELPER, 5.0);
    assert!(r(&back, &[], HELPER, 80) >= 5.0);
}

#[test]
fn the_report_names_every_corp_and_its_tier() {
    let mut st = hall();
    st.see_human(&human(HELPER), 0);
    st.relations.add(C1, HELPER, society::ALLY_AT * 2.0);
    st.relations.add(C2, HELPER, society::ENEMY_AT * 2.0);
    let lines = st.standing_report(HELPER, 0);
    assert_eq!(lines.len(), 2);
    let by: BTreeMap<_, _> = lines
        .iter()
        .map(|l| (l.contains(&C1.to_string()), l.clone()))
        .collect();
    assert!(by[&true].contains("allied"), "{lines:?}");
    assert!(by[&false].contains("hostile"), "{lines:?}");
}

#[test]
fn each_tier_has_its_own_mail_and_only_the_allied_one_names_the_staging() {
    let mut titles = std::collections::BTreeSet::new();
    for t in [
        Stance5::Hostile,
        Stance5::Wary,
        Stance5::Neutral,
        Stance5::Friendly,
        Stance5::Allied,
    ] {
        let (title, body) = crate::mail::tier_text("Invented Corp", t, Some(30_000_999));
        assert!(title.contains(t.name()), "{title}");
        assert!(body.len() <= crate::mail::MAX_TEXT, "{} chars", body.len());
        assert_eq!(body.contains("30000999"), t == Stance5::Allied, "{t:?}");
        titles.insert(title);
    }
    assert_eq!(titles.len(), 5);
}

#[test]
fn review_fix_grid_presence_is_not_fleet_membership() {
    let aff = |c: i64| Affiliation {
        character_id: c,
        corporation_id: 0,
        alliance_id: None,
    };
    let joined: std::collections::BTreeSet<i64> = [FRIEND].into_iter().collect();
    let on_grid = vec![aff(FRIEND), aff(STRANGER), aff(HELPER)];
    // The stranger parked on the grid is out; the joined pilot and the boss are in.
    let got: Vec<i64> = fleet_mates(on_grid.clone(), &joined, Some(HELPER))
        .iter()
        .map(|a| a.character_id)
        .collect();
    assert_eq!(got, vec![FRIEND, HELPER]);
    assert!(fleet_mates(on_grid, &Default::default(), None).is_empty());
}
