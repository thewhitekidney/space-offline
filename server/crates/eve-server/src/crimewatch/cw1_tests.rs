//! CW-1's tests: the crime verdicts applied, the criminal timer, limited
//! engagements, assistance, the safety setting and its refusals, the crime
//! seam, kill rights and duels. Invented ids and rows only.

use super::*;
use crate::kill_hooks::KillEvent;
use std::time::Instant;

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const CAROL: i64 = 90_000_003;
const SHIP: [i64; 3] = [100_000_001, 100_000_002, 100_000_003];
/// An invented hull: anything but the capsule.
const HULL: i64 = 990_000_001;
/// The system the fights happen in; its security is each test's.
const SYS: i64 = 30_000_003;
const T0: BlueTime = 133_000_000_000_000_000;
const HIGH: f64 = 0.75;
const LOW: f64 = 0.25;

/// An invented universe row for `SYS` at `security` (CP3b's format).
fn universe(security: f64) -> crate::celestial::Universe {
    crate::celestial::Universe::parse(
        &[format!(
            "{SYS}\t19000001\t29000001\t\t\t{security}\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tx"
        )],
        &[],
        &[],
    )
    .expect("the invented row parses")
}

/// Alice, Bob and Carol in space in `SYS` (security `security`), in hulls,
/// Alice with ISK.
fn world(security: f64) -> Registry {
    let pilot = |i: usize, id: i64| crate::model::Character {
        character_id: id,
        name: format!("pilot {id}"),
        ship_id: SHIP[i],
        ship_type_id: HULL,
        balance: 1_000_000.0,
        ..crate::model::Character::default()
    };
    let mut content = crate::content::Content::fixture();
    content.insert("universe", universe(security));
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![pilot(0, ALICE), pilot(1, BOB), pilot(2, CAROL)],
        ..crate::model::SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    for (i, id) in [ALICE, BOB, CAROL].into_iter().enumerate() {
        reg.character_mut(id).expect("a pilot").solar_system_id = Some(SYS);
        reg.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                id,
                SHIP[i],
                SYS,
                crate::space_state::Motion::arrived([i as f64 * 5_000.0, 0.0, 0.0], [0.0; 3]),
            ));
    }
    let _ = reg.take_pending();
    let _ = CRIMES.with(|c| std::mem::take(&mut *c.borrow_mut()));
    reg
}

type Out = Vec<(Vec<i64>, &'static str, Vec<PyValue>)>;

fn due(reg: &mut Registry) -> Out {
    reg.take_due(Instant::now() + Duration::from_secs(1))
        .into_iter()
        .map(|p| {
            let (name, args) = p.to_push();
            (p.audience().unwrap_or_default(), name, args)
        })
        .collect()
}

/// The args of every `event` sent to `to`.
fn sent(out: &Out, to: i64, event: &str) -> Vec<Vec<PyValue>> {
    out.iter()
        .filter(|(a, n, _)| *n == event && a.contains(&to))
        .map(|(_, _, args)| args.clone())
        .collect()
}

fn ints(v: &[i64]) -> PyValue {
    PyValue::List(v.iter().copied().map(PyValue::Int).collect())
}

fn raised(err: &ServiceError) -> String {
    let ServiceError::Raise(PyValue::ObjectEx { header, .. }) = err else {
        panic!("not a raised exception: {err:?}")
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        panic!("a header tuple")
    };
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("UserError's args")
    };
    String::from_utf8_lossy(crate::pyutil::as_bytes(&args[0]).expect("a key")).into_owned()
}

fn crimes() -> Vec<CrimeEvent> {
    CRIMES.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn board_capsule(reg: &mut Registry, id: i64) {
    reg.character_mut(id).expect("a pilot").ship_type_id = crate::damage::CAPSULE_TYPE_ID;
}

fn kill(reg: &mut Registry, victim: i64, killer: i64, ship_type: i64, when: BlueTime) {
    let ev = KillEvent {
        victim_character: victim,
        victim_ship: 1,
        ship_type,
        killer: Some(killer),
        system: SYS,
        npc: None,
        when,
        park: vec![],
    };
    on_kill(reg, &ev);
}

// ---------------------------------------------------------------------------
// Verdicts applied
// ---------------------------------------------------------------------------

/// **Shooting a plain pilot in high sec makes a criminal** (283272's
/// "criminal"-level offense; `DoesConcordReact`) [V]: the shooter's HUD
/// counts the criminal timer (`TimerCriminal` 303, fifteen minutes), every
/// pilot in the system is told in `newCriminals`, `GetClientStates` lists
/// the criminal, and the crime is offered to the seam once.
#[test]
fn an_attack_in_high_sec_makes_a_criminal_everyone_sees() {
    let mut reg = world(HIGH);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnCriminalTimerUpdate"),
        vec![vec![
            PyValue::Int(state::TIMER_CRIMINAL),
            PyValue::Int(T0 + CRIMINAL_TIMEOUT)
        ]]
    );
    let flags = sent(&out, CAROL, "OnSystemCriminalFlagUpdates");
    assert_eq!(flags, vec![vec![ints(&[]), ints(&[]), ints(&[ALICE])]]);
    assert!(is_criminal(&reg, ALICE, T0));
    let PyValue::Tuple(parts) = client_states(&reg, Some(BOB), T0) else {
        unreachable!()
    };
    assert_eq!(parts[2], PyValue::Tuple(vec![ints(&[ALICE]), ints(&[])]));
    let seen = crimes();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        (
            seen[0].offender,
            seen[0].victim,
            seen[0].kind,
            seen[0].act,
            seen[0].space
        ),
        (
            ALICE,
            Some(BOB),
            CrimeKind::Criminal,
            ActKind::Attack,
            Space::High
        )
    );
}

/// **In low sec a ship is a suspect act and a capsule a criminal one**
/// [V]; the flag only ever rises, from suspect (304) to criminal (303),
/// and the system hears the rise.
#[test]
fn low_sec_suspect_then_a_pod_shot_makes_a_criminal() {
    let mut reg = world(LOW);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    assert!(is_suspect(&reg, ALICE, T0));
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, CAROL, "OnSystemCriminalFlagUpdates"),
        vec![vec![ints(&[]), ints(&[ALICE]), ints(&[])]]
    );
    board_capsule(&mut reg, BOB);
    let t1 = T0 + 30 * SEC;
    aggression(&mut reg, ALICE, BOB, t1, t1);
    assert!(is_criminal(&reg, ALICE, t1));
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnCriminalTimerUpdate"),
        vec![vec![
            PyValue::Int(state::TIMER_CRIMINAL),
            PyValue::Int(t1 + CRIMINAL_TIMEOUT)
        ]]
    );
    assert_eq!(
        sent(&out, CAROL, "OnSystemCriminalFlagUpdates"),
        vec![vec![ints(&[]), ints(&[]), ints(&[ALICE])]]
    );
    // A later suspect act does not lower it.
    reg.character_mut(BOB).expect("Bob").ship_type_id = HULL;
    let t2 = t1 + SEC;
    aggression(&mut reg, ALICE, CAROL, t2, t2);
    assert!(is_criminal(&reg, ALICE, t2));
}

/// **The criminal flag runs out**: after fifteen minutes the HUD's timer is
/// idle (300) and the system hears the pilot in `newIdles`; the record is
/// forgotten.
#[test]
fn a_criminal_flag_clears_after_its_timer() {
    let mut reg = world(HIGH);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    let _ = due(&mut reg);
    tick(&mut reg, T0 + 16 * MIN);
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnCriminalTimerUpdate"),
        vec![vec![PyValue::Int(state::CRIMINAL_IDLE), PyValue::None]]
    );
    assert_eq!(
        sent(&out, CAROL, "OnSystemCriminalFlagUpdates"),
        vec![vec![ints(&[ALICE]), ints(&[]), ints(&[])]]
    );
    assert!(flag_of(&reg, ALICE, T0 + 16 * MIN).is_none());
    assert!(reg.combat().crimewatch().records.is_empty());
}

/// Zero sec has no crimes and no engagements, whatever is shot [V].
#[test]
fn zero_sec_has_no_crime() {
    let mut reg = world(-0.4);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    assert_eq!(flag_of(&reg, ALICE, T0), None);
    assert!(crimes().is_empty());
    assert!(reg.combat().crimewatch().engagements.is_empty());
    assert_eq!(space_of(&reg, SYS), Space::Zero);
}

/// **Shooting a flagged pilot is no crime and opens a limited engagement**
/// [A rule, V pushes]: both are told `OnCrimewatchEngagementCreated(other,
/// -1)`, the flagged one may shoot back without a crime, the engagement
/// starts timing out ten seconds after the last volley
/// (`...StartTimeout(other, end)`), goes back to ongoing on a new volley
/// (`...StopTimeout`), and ends five minutes after the last
/// (`...Ended`). `GetClientStates` carries it as `{other: timeout}`.
#[test]
fn shooting_a_suspect_engages_and_the_suspect_may_shoot_back() {
    let mut reg = world(LOW);
    theft_from(&mut reg, BOB, Some(CAROL), SYS, T0);
    assert!(is_suspect(&reg, BOB, T0));
    let _ = due(&mut reg);
    let _ = crimes();

    let t1 = T0 + MIN;
    aggression(&mut reg, ALICE, BOB, t1, t1);
    assert_eq!(flag_of(&reg, ALICE, t1), None, "a suspect is free to shoot");
    let out = due(&mut reg);
    for (to, other) in [(ALICE, BOB), (BOB, ALICE)] {
        assert_eq!(
            sent(&out, to, "OnCrimewatchEngagementCreated"),
            vec![vec![PyValue::Int(other), PyValue::Int(ENGAGEMENT_ONGOING)]]
        );
    }
    let PyValue::Tuple(parts) = client_states(&reg, Some(ALICE), t1) else {
        unreachable!()
    };
    assert_eq!(
        parts[1],
        PyValue::Dict(vec![(PyValue::Int(ENGAGEMENT_ONGOING), PyValue::Int(BOB))])
    );

    // Bob shoots back: no crime, the engagement keeps going.
    let t2 = t1 + 5 * SEC;
    aggression(&mut reg, BOB, ALICE, t2, t2);
    assert!(crimes().is_empty(), "returning fire inside an engagement");
    assert!(is_suspect(&reg, BOB, t2), "and still only a suspect");

    // Quiet: the timeout starts.
    let t3 = t2 + ACTIVE_GRACE;
    tick(&mut reg, t3);
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnCrimewatchEngagementStartTimeout"),
        vec![vec![
            PyValue::Int(BOB),
            PyValue::Int(t2 + ENGAGEMENT_DURATION)
        ]]
    );
    // A new volley: ongoing again.
    aggression(&mut reg, ALICE, BOB, t3 + SEC, t3 + SEC);
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, BOB, "OnCrimewatchEngagementStopTimeout"),
        vec![vec![PyValue::Int(ALICE)]]
    );
    // And over five minutes after the last.
    tick(&mut reg, t3 + SEC + ENGAGEMENT_DURATION);
    let out = due(&mut reg);
    for (to, other) in [(ALICE, BOB), (BOB, ALICE)] {
        assert_eq!(
            sent(&out, to, "OnCrimewatchEngagementEnded"),
            vec![vec![PyValue::Int(other)]]
        );
    }
    assert!(
        !reg.combat()
            .crimewatch()
            .engaged(ALICE, BOB, t3 + SEC + ENGAGEMENT_DURATION)
    );
}

/// **A death ends the dead pilot's engagements**, and both are told [A].
#[test]
fn a_death_ends_an_engagement() {
    let mut reg = world(LOW);
    engage(&mut reg, ALICE, BOB, T0, T0);
    let _ = due(&mut reg);
    kill(&mut reg, BOB, ALICE, HULL, T0 + SEC);
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnCrimewatchEngagementEnded"),
        vec![vec![PyValue::Int(BOB)]]
    );
    assert!(!reg.combat().crimewatch().engaged(ALICE, BOB, T0 + SEC));
}

/// **Assisting a flagged pilot inherits the flag** [V rule], and the target's
/// running timers in their `Inherited` states (103, 203) [V states, A rule].
#[test]
fn assisting_a_criminal_makes_a_criminal_with_inherited_timers() {
    let mut reg = world(HIGH);
    // Bob shoots Alice: a criminal in high sec, with weapons and pvp timers.
    aggression(&mut reg, BOB, ALICE, T0, T0);
    let _ = due(&mut reg);
    let _ = crimes();
    assistance(&mut reg, CAROL, SHIP[1], T0 + SEC, T0 + SEC);
    assert!(is_criminal(&reg, CAROL, T0 + SEC), "a criminal's helper");
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, CAROL, "OnWeaponsTimerUpdate"),
        vec![vec![PyValue::Int(state::WEAPONS_INHERITED), PyValue::None]]
    );
    assert_eq!(
        sent(&out, CAROL, "OnPvpTimerUpdate"),
        vec![vec![PyValue::Int(state::PVP_INHERITED), PyValue::None]]
    );
    let seen = crimes();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].act, seen[0].victim), (ActKind::Assist, Some(BOB)));
    // Assisting a clean pilot is nothing.
    let mut reg = world(HIGH);
    assistance(&mut reg, CAROL, SHIP[0], T0, T0);
    assert_eq!(flag_of(&reg, CAROL, T0), None);
}

/// **Theft** is a suspect act, and none at all from an owner one may
/// attack freely (`HaveLootRight` asks `CanAttackFreely`) [V].
#[test]
fn theft_from_a_flagged_owner_is_no_theft() {
    let mut reg = world(HIGH);
    theft_from(&mut reg, ALICE, Some(BOB), SYS, T0);
    assert!(is_suspect(&reg, ALICE, T0));
    // Carol robs Alice, now a suspect: no crime.
    theft_from(&mut reg, CAROL, Some(ALICE), SYS, T0 + SEC);
    assert_eq!(flag_of(&reg, CAROL, T0 + SEC), None);
}

/// **The seam**: [`ON_CRIME`] is empty in CW-1 and every crime passes
/// through [`fire_crime`]; a local table runs each hook once per crime, in
/// order, with the event.
#[test]
fn every_crime_is_offered_to_the_crime_hooks() {
    thread_local! {
        static GOT: std::cell::RefCell<Vec<(i64, CrimeKind)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn hook(_: &mut Registry, ev: &CrimeEvent) {
        GOT.with(|g| g.borrow_mut().push((ev.offender, ev.kind)));
    }
    // `ON_CRIME` itself holds the subscribers the coordinator adds.
    let mut reg = world(HIGH);
    let ev = CrimeEvent {
        offender: ALICE,
        victim: Some(BOB),
        kind: CrimeKind::Criminal,
        act: ActKind::Attack,
        solar_system_id: SYS,
        space: Space::High,
        at: T0,
    };
    fire_crime(&[hook, hook], &mut reg, &ev);
    assert_eq!(
        GOT.with(|g| g.borrow().clone()),
        vec![(ALICE, CrimeKind::Criminal), (ALICE, CrimeKind::Criminal)]
    );
}

// ---------------------------------------------------------------------------
// The safety setting
// ---------------------------------------------------------------------------

/// **Green by default, stored, and answered back**: `GetClientStates`'
/// fourth element is the setting (`_UpdateSafetyLevel`) [V], `2` until the
/// pilot sets another, which survives in its delta row.
#[test]
fn the_safety_setting_is_stored_and_answered() {
    let mut reg = world(HIGH);
    let level = |reg: &Registry| {
        let PyValue::Tuple(parts) = client_states(reg, Some(ALICE), T0) else {
            unreachable!()
        };
        parts[3].clone()
    };
    assert_eq!(level(&reg), PyValue::Int(2));
    set_safety(&mut reg, ALICE, Safety::None);
    assert_eq!(level(&reg), PyValue::Int(0));
    assert_eq!(
        reg.deltas().of(SAFETY_KIND).get(&vec![ALICE.to_string()]),
        Some(&vec!["0".to_owned()])
    );
    set_safety(&mut reg, ALICE, Safety::Full);
    assert!(
        reg.deltas().of(SAFETY_KIND).is_empty(),
        "green needs no row"
    );
    assert_eq!(safety(&reg, ALICE), Safety::Full);
}

/// **The server refuses what the safety forbids**, in the client's keys
/// (283544/283545) [V]: green refuses a suspect act and a criminal one,
/// yellow only the criminal one, red nothing; and nothing is refused for an
/// act that is no crime.
#[test]
fn safety_refuses_attacks_by_the_crime_they_would_be() {
    for (security, crime_key) in [
        (HIGH, "ModuleActivationDeniedSafetyPreventsCriminal"),
        (LOW, "ModuleActivationDeniedSafetyPreventsSuspect"),
    ] {
        let mut reg = world(security);
        let r = attack_refusal(&reg, ALICE, Some(HULL), SHIP[1], T0).expect("green refuses");
        assert_eq!(r.key, crime_key);
        assert_eq!(r.args[0].0, "moduleName");
        assert_eq!(r.args[1].0, "targetName");
        set_safety(&mut reg, ALICE, Safety::Partial);
        assert_eq!(
            attack_refusal(&reg, ALICE, Some(HULL), SHIP[1], T0).is_some(),
            security == HIGH,
            "yellow allows a suspect act only"
        );
        set_safety(&mut reg, ALICE, Safety::None);
        assert!(attack_refusal(&reg, ALICE, Some(HULL), SHIP[1], T0).is_none());
    }
    // No crime, no refusal: a suspect target at green.
    let mut reg = world(HIGH);
    suspect(&mut reg, BOB, T0);
    assert!(attack_refusal(&reg, ALICE, Some(HULL), SHIP[1], T0).is_none());
    // Nor in zero sec.
    let reg = world(-0.4);
    assert!(attack_refusal(&reg, ALICE, Some(HULL), SHIP[1], T0).is_none());
}

/// The assistance gate: a suspect target at green is refused, an engaged
/// one with the engagement key (517301) [V key, A rule].
#[test]
fn safety_refuses_assistance_to_the_flagged() {
    let mut reg = world(HIGH);
    suspect(&mut reg, BOB, T0);
    let r = assist_refusal(&reg, ALICE, None, SHIP[1], T0).expect("refused");
    assert_eq!(r.key, "ModuleActivationDeniedSafetyPreventsSuspect");
    let mut reg = world(HIGH);
    engage(&mut reg, BOB, CAROL, T0, T0);
    let r = assist_refusal(&reg, ALICE, None, SHIP[1], T0).expect("refused");
    assert_eq!(
        r.key,
        "ModuleActivationDeniedSafetyPreventsLimitedEngagement"
    );
    assert!(assist_refusal(&reg, ALICE, None, SHIP[2], T0).is_some());
    let reg = world(HIGH);
    assert!(assist_refusal(&reg, ALICE, None, SHIP[1], T0).is_none());
}

/// **`SetSafetyLevel` over the service**: the bound `crimewatch` object
/// stores it and the next `GetClientStates` answers it; a level outside
/// `0..=2` is bad arguments.
#[test]
fn set_safety_level_is_served() {
    let mut reg = world(HIGH);
    let call = |reg: &mut Registry, method: &str, args: &[PyValue]| {
        reg.dispatch(&crate::service::Request {
            service: "crimewatch",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    };
    assert_eq!(
        call(&mut reg, "SetSafetyLevel", &[PyValue::Int(1)]).expect("answered"),
        PyValue::None
    );
    assert_eq!(safety(&reg, ALICE), Safety::Partial);
    let PyValue::Tuple(parts) = call(&mut reg, "GetClientStates", &[]).expect("answered") else {
        panic!("a tuple")
    };
    assert_eq!(parts[3], PyValue::Int(1));
    assert!(matches!(
        call(&mut reg, "SetSafetyLevel", &[PyValue::Int(7)]),
        Err(ServiceError::BadArguments { .. })
    ));
}

// ---------------------------------------------------------------------------
// Kill rights
// ---------------------------------------------------------------------------

/// **A criminal kill earns the victim a kill right** (285051) [V]: Alice
/// shoots Bob criminally in high sec and kills him; Bob holds a right on
/// Alice for [`killright::DURATION`], both are told `OnKillRightCreated`,
/// both see it in `GetMyKillRights`. A second kill earns no second right
/// [A].
#[test]
fn a_criminal_kill_earns_a_kill_right() {
    let mut reg = world(HIGH);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    let _ = due(&mut reg);
    kill(&mut reg, BOB, ALICE, HULL, T0 + SEC);
    let rights = killright::all(&reg);
    assert_eq!(rights.len(), 1);
    let kr = rights[0];
    assert_eq!((kr.from, kr.to), (BOB, ALICE));
    assert_eq!(kr.expiry, T0 + SEC + killright::DURATION);
    assert_eq!((kr.price, kr.restricted_to), (None, None));
    let out = due(&mut reg);
    for to in [ALICE, BOB] {
        assert_eq!(
            sent(&out, to, "OnKillRightCreated"),
            vec![vec![
                PyValue::Int(kr.id),
                PyValue::Int(BOB),
                PyValue::Int(ALICE),
                PyValue::Int(kr.expiry)
            ]]
        );
        let PyValue::List(rows) = killright::get_my(&mut reg, Some(to), T0 + 2 * SEC) else {
            panic!("a list")
        };
        assert_eq!(rows, vec![kr.to_value()]);
    }
    let PyValue::List(rows) = killright::get_my(&mut reg, Some(CAROL), T0) else {
        panic!("a list")
    };
    assert!(rows.is_empty(), "nobody else's");
    kill(
        &mut reg,
        BOB,
        ALICE,
        crate::damage::CAPSULE_TYPE_ID,
        T0 + 2 * SEC,
    );
    assert_eq!(killright::all(&reg).len(), 1, "one per pair");
}

/// A kill with no criminal act behind it earns nothing: a suspect's death,
/// a zero-sec kill, a kill long after the crime.
#[test]
fn a_lawful_kill_earns_nothing() {
    let mut reg = world(HIGH);
    suspect(&mut reg, BOB, T0);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    kill(&mut reg, BOB, ALICE, HULL, T0 + SEC);
    assert!(killright::all(&reg).is_empty(), "Bob was a suspect");

    let mut reg = world(-0.4);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    kill(&mut reg, BOB, ALICE, HULL, T0 + SEC);
    assert!(killright::all(&reg).is_empty(), "zero sec");

    let mut reg = world(HIGH);
    aggression(&mut reg, ALICE, BOB, T0, T0);
    tick(&mut reg, T0 + CRIMINAL_TIMEOUT);
    kill(&mut reg, BOB, ALICE, HULL, T0 + CRIMINAL_TIMEOUT + SEC);
    assert!(killright::all(&reg).is_empty(), "the act is too old");
}

/// A kill right's row round-trips through its delta row, an expired one is
/// pruned on the next read, and the allocator never reuses an id.
#[test]
fn a_kill_right_round_trips_and_expires() {
    let mut reg = world(HIGH);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    assert_eq!(killright::get(&reg, kr.id), Some(kr));
    let PyValue::List(_) =
        killright::sell(&mut reg, Some(BOB), kr.id, 1_500.0, Some(CAROL), T0).expect("sold")
    else {
        panic!("a list")
    };
    let sold = killright::get(&reg, kr.id).expect("still there");
    assert_eq!(
        (sold.price, sold.restricted_to),
        (Some(1_500.0), Some(CAROL))
    );
    let PyValue::List(rows) = killright::get_my(&mut reg, Some(BOB), kr.expiry) else {
        panic!("a list")
    };
    assert!(rows.is_empty(), "expired and pruned");
    assert!(killright::get(&reg, kr.id).is_none());
    let next = killright::grant(&mut reg, BOB, ALICE, kr.expiry).expect("a new one");
    assert!(next.id > kr.id);
}

/// **Selling**: only the holder may (`NoValidKillRight` otherwise) [V key];
/// the sale is visible to whom it is open (`GetKillRightsOnCharacters`),
/// the target is told (`OnKillRightForYouSold`), and a cancel takes it off.
#[test]
fn a_kill_right_is_sold_to_whom_it_is_open() {
    let mut reg = world(HIGH);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    let _ = due(&mut reg);
    let err = killright::sell(&mut reg, Some(CAROL), kr.id, 10.0, None, T0).expect_err("not hers");
    assert_eq!(raised(&err), "NoValidKillRight");
    killright::sell(&mut reg, Some(BOB), kr.id, 10.0, Some(BOB), T0).expect("sold");
    let out = due(&mut reg);
    assert_eq!(
        sent(&out, ALICE, "OnKillRightForYouSold"),
        vec![vec![PyValue::Int(kr.id)]]
    );
    let PyValue::List(rows) = killright::on_characters(&mut reg, Some(CAROL), &[ALICE], T0) else {
        panic!("a list")
    };
    assert!(rows.is_empty(), "restricted to Bob");
    killright::sell(&mut reg, Some(BOB), kr.id, 10.0, None, T0).expect("sold to all");
    let PyValue::List(rows) = killright::on_characters(&mut reg, Some(CAROL), &[ALICE], T0) else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1, "open to all");
    killright::cancel(&mut reg, Some(BOB), kr.id, T0);
    let PyValue::List(rows) = killright::on_characters(&mut reg, Some(CAROL), &[ALICE], T0) else {
        panic!("a list")
    };
    assert!(rows.is_empty(), "cancelled");
}

/// **Activation**: the holder's own right makes the target a suspect for
/// fifteen minutes ("Activate suspect flag on {charName} for 15 minutes")
/// [V text], and the target is told (`OnKillRightActivated`); refused in
/// zero sec (`CantActivateKillRightInNullSec`) [V key], by anyone but the
/// holder, and once expired (`KillRightExpired`).
#[test]
fn activating_a_kill_right_makes_the_target_a_suspect() {
    let mut reg = world(HIGH);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    let _ = due(&mut reg);
    let err = killright::activate(&mut reg, Some(CAROL), kr.id, T0).expect_err("not hers");
    assert_eq!(raised(&err), "NoValidKillRight");
    killright::activate(&mut reg, Some(BOB), kr.id, T0).expect("activated");
    assert!(is_suspect(&reg, ALICE, T0));
    assert_eq!(
        reg.combat()
            .crimewatch()
            .activation(ALICE, T0)
            .map(|a| a.activator),
        Some(BOB)
    );
    let out = due(&mut reg);
    assert_eq!(sent(&out, ALICE, "OnKillRightActivated"), vec![vec![]]);
    let err = killright::activate(&mut reg, Some(BOB), kr.id, kr.expiry).expect_err("expired");
    assert_eq!(raised(&err), "KillRightExpired");

    let mut reg = world(-0.4);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    let err = killright::activate(&mut reg, Some(BOB), kr.id, T0).expect_err("null");
    assert_eq!(raised(&err), "CantActivateKillRightInNullSec");
}

/// **Buying**: the price the buyer saw (`KillRightPriceNotCorrect`), the
/// buyer's money (`NotEnoughMoney`), a sale open to them
/// (`KillRightNotForSale`) [V keys]; a purchase pays the holder and
/// activates for the buyer.
#[test]
fn buying_a_kill_right_pays_the_holder_and_activates() {
    let mut reg = world(HIGH);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    let err = killright::buy(&mut reg, Some(CAROL), kr.id, 0.0, T0).expect_err("not for sale");
    assert_eq!(raised(&err), "KillRightNotForSale");
    killright::sell(&mut reg, Some(BOB), kr.id, 5_000.0, None, T0).expect("sold");
    let err = killright::buy(&mut reg, Some(CAROL), kr.id, 4_000.0, T0).expect_err("price");
    assert_eq!(raised(&err), "KillRightPriceNotCorrect");
    reg.character_mut(CAROL).expect("Carol").balance = 100.0;
    let err = killright::buy(&mut reg, Some(CAROL), kr.id, 5_000.0, T0).expect_err("poor");
    assert_eq!(raised(&err), "NotEnoughMoney");
    reg.character_mut(CAROL).expect("Carol").balance = 1_000_000.0;
    let bob_before = reg.character(BOB).expect("Bob").balance;
    killright::buy(&mut reg, Some(CAROL), kr.id, 5_000.0, T0).expect("bought");
    assert_eq!(reg.character(CAROL).expect("Carol").balance, 995_000.0);
    assert_eq!(
        reg.character(BOB).expect("Bob").balance,
        bob_before + 5_000.0
    );
    assert!(is_suspect(&reg, ALICE, T0));
    assert!(
        killright::get(&reg, kr.id).is_some(),
        "a price per activation"
    );
}

/// **A kill under an activation uses the right up** (285055) [V]: the
/// target's ship dies, the right is gone, and its holder, activator and
/// target are told `OnKillRightUsed(killRightID, toID)`. A capsule kill
/// does not use it [A].
#[test]
fn a_kill_under_an_activation_uses_the_right() {
    let mut reg = world(HIGH);
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    killright::sell(&mut reg, Some(BOB), kr.id, 0.0, None, T0).expect("sold");
    killright::buy(&mut reg, Some(CAROL), kr.id, 0.0, T0).expect("bought");
    let _ = due(&mut reg);
    kill(
        &mut reg,
        ALICE,
        CAROL,
        crate::damage::CAPSULE_TYPE_ID,
        T0 + SEC,
    );
    assert!(killright::get(&reg, kr.id).is_some(), "a pod kill keeps it");
    kill(&mut reg, ALICE, CAROL, HULL, T0 + 2 * SEC);
    assert!(killright::get(&reg, kr.id).is_none());
    let out = due(&mut reg);
    for to in [ALICE, BOB, CAROL] {
        assert_eq!(
            sent(&out, to, "OnKillRightUsed"),
            vec![vec![PyValue::Int(kr.id), PyValue::Int(ALICE)]],
            "{to}"
        );
    }
    assert!(
        killright::all(&reg).is_empty(),
        "Carol's lawful kill earns Alice nothing"
    );
}

/// **The services route**: `bountyProxy.GetMyKillRights` and
/// `killRightMgr.ActivateKillRight` reach the kill rights through
/// `Registry::dispatch`.
#[test]
fn the_kill_right_services_are_served() {
    let mut reg = world(HIGH);
    let kr =
        killright::grant(&mut reg, BOB, ALICE, crate::pyutil::filetime_now()).expect("granted");
    let call = |reg: &mut Registry, service: &str, method: &str, args: &[PyValue]| {
        reg.dispatch(&crate::service::Request {
            service,
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(BOB),
        })
    };
    let PyValue::List(rows) =
        call(&mut reg, "bountyProxy", "GetMyKillRights", &[]).expect("answered")
    else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        call(
            &mut reg,
            "killRightMgr",
            "ActivateKillRight",
            &[PyValue::Int(kr.id)]
        )
        .expect("activated"),
        PyValue::None
    );
    let PyValue::Tuple(parts) = call(
        &mut reg,
        "bountyProxy",
        "GetBountiesAndKillRights",
        &[
            PyValue::List(vec![]),
            PyValue::List(vec![PyValue::Int(ALICE)]),
        ],
    )
    .expect("answered") else {
        panic!("a tuple")
    };
    assert_eq!(parts[1], PyValue::List(vec![]), "not for sale");
}

// ---------------------------------------------------------------------------
// Duels
// ---------------------------------------------------------------------------

/// **A duel**: the challenge reaches the challenged pilot as
/// `OnDuelChallenge(from, corp, alliance, expiry)` [V], expiring in
/// `duelOfferExpiryTimeout`; an acceptance engages the two; a second offer
/// to someone else while one waits is `TooManyOutgoing` [V key]; an
/// acceptance of nothing is `DuelChallengeAcceptFail` [V key]; a decline
/// tells the challenger.
#[test]
fn a_duel_is_offered_accepted_and_engages() {
    let mut reg = world(HIGH);
    let online = |reg: &mut Registry, id: i64| {
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: id,
            session_id: id,
            user_id: id,
            character_id: Some(id),
        });
    };
    online(&mut reg, BOB);
    online(&mut reg, CAROL);
    online(&mut reg, ALICE);
    duel::start(&mut reg, Some(ALICE), BOB, T0).expect("offered");
    let out = due(&mut reg);
    let offer = sent(&out, BOB, "OnDuelChallenge");
    assert_eq!(offer.len(), 1);
    assert_eq!(offer[0][0], PyValue::Int(ALICE));
    assert_eq!(offer[0][3], PyValue::Int(T0 + duel::OFFER_TIMEOUT));
    let err = duel::start(&mut reg, Some(ALICE), CAROL, T0).expect_err("one at a time");
    assert_eq!(raised(&err), "DuelChallengeFailTooManyOutgoingRequests");
    let err = duel::respond(&mut reg, Some(CAROL), ALICE, None, true, T0).expect_err("no offer");
    assert_eq!(raised(&err), "DuelChallengeAcceptFail");
    duel::respond(
        &mut reg,
        Some(BOB),
        ALICE,
        Some(T0 + duel::OFFER_TIMEOUT),
        true,
        T0 + SEC,
    )
    .expect("accepted");
    assert!(reg.combat().crimewatch().engaged(ALICE, BOB, T0 + SEC));
    // Inside the duel, shooting is no crime.
    aggression(&mut reg, ALICE, BOB, T0 + 2 * SEC, T0 + 2 * SEC);
    assert_eq!(flag_of(&reg, ALICE, T0 + 2 * SEC), None);

    duel::start(&mut reg, Some(CAROL), ALICE, T0).expect("offered");
    let _ = due(&mut reg);
    duel::respond(&mut reg, Some(ALICE), CAROL, None, false, T0 + SEC).expect("declined");
    let out = due(&mut reg);
    assert_eq!(sent(&out, CAROL, "OnRemoteMessage").len(), 1);
}

/// **The notification centre** (NOTIF-1) hears each kill-right event with
/// the client's type and data keys [V]: the holder "earned" (117,
/// `charID` the criminal) and "used" (118, `charID` the target); the target
/// the sale (116 open, 115 restricted with `toEntityID`; `price`) and its
/// withdrawal (120, 119).
#[test]
fn kill_right_events_reach_the_notification_centre() {
    use crate::notifications::{Value, of};
    let mut reg = world(HIGH);
    let kinds = |reg: &Registry, who: i64| -> Vec<(i64, Vec<(String, Value)>)> {
        let mut n = of(reg.deltas(), who);
        n.sort_by_key(|x| x.id);
        n.into_iter().map(|x| (x.type_id, x.data)).collect()
    };
    let kr = killright::grant(&mut reg, BOB, ALICE, T0).expect("granted");
    assert_eq!(
        kinds(&reg, BOB),
        vec![(117, vec![("charID".to_owned(), Value::Int(ALICE))])]
    );
    killright::sell(&mut reg, Some(BOB), kr.id, 50.0, None, T0).expect("sold");
    killright::cancel(&mut reg, Some(BOB), kr.id, T0);
    killright::sell(&mut reg, Some(BOB), kr.id, 60.0, Some(CAROL), T0).expect("sold");
    killright::cancel(&mut reg, Some(BOB), kr.id, T0);
    killright::cancel(&mut reg, Some(BOB), kr.id, T0);
    let to_alice = kinds(&reg, ALICE);
    assert_eq!(
        to_alice.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        vec![116, 120, 115, 119],
        "a cancel of nothing for sale says nothing"
    );
    assert_eq!(
        to_alice[2].1,
        vec![
            ("charID".to_owned(), Value::Int(BOB)),
            ("price".to_owned(), Value::Real(60.0)),
            ("toEntityID".to_owned(), Value::Int(CAROL)),
        ]
    );
    killright::activate(&mut reg, Some(BOB), kr.id, T0).expect("activated");
    kill(&mut reg, ALICE, BOB, HULL, T0 + SEC);
    assert_eq!(
        kinds(&reg, BOB).last(),
        Some(&(118, vec![("charID".to_owned(), Value::Int(ALICE))]))
    );
}

/// **SEC-1 answers the seam** (`ON_CRIME`'s first row): a criminal attack
/// costs the offender security status and writes an "illegal aggression"
/// history row (379); a suspect attack and a criminal theft cost nothing
/// (`sec_status::on_crime_event`, `docs/lanes/sec-1.md` §2).
#[test]
fn a_criminal_act_costs_security_status_through_the_seam() {
    let mut reg = world(HIGH);
    let ev = |kind, act| CrimeEvent {
        offender: ALICE,
        victim: Some(BOB),
        kind,
        act,
        solar_system_id: SYS,
        space: Space::High,
        at: T0,
    };
    let status = |reg: &Registry| crate::npc::bounty::security_status(reg, ALICE);
    fire_crime(ON_CRIME, &mut reg, &ev(CrimeKind::Suspect, ActKind::Attack));
    fire_crime(ON_CRIME, &mut reg, &ev(CrimeKind::Criminal, ActKind::Theft));
    assert_eq!(status(&reg), 0.0);
    assert!(crate::sec_status::transactions(&reg, ALICE).is_empty());
    fire_crime(
        ON_CRIME,
        &mut reg,
        &ev(CrimeKind::Criminal, ActKind::Attack),
    );
    assert!(status(&reg) < 0.0, "a criminal attack costs status");
    let tx = crate::sec_status::transactions(&reg, ALICE);
    assert_eq!(tx.len(), 1);
    assert_eq!(tx[0].event, crate::sec_status::EVENT_ILLEGAL_AGGRESSION);
    assert_eq!(tx[0].other_owner, BOB);
}
