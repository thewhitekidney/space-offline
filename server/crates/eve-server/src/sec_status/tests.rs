//! SEC-1 unit tests: the flags, the penalty rule, the history, the slim and
//! the dock refusal. **Invented ids and values throughout**; an `[A]`
//! constant is named, never written as a number.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::npc::bounty::{BOUNTY_PERIOD, security_status, set_security_status};
use std::time::Instant;

const ALICE: i64 = 90_000_501;
const BOB: i64 = 90_000_502;
const CAROL: i64 = 90_000_503;
const ALICE_SHIP: i64 = 100_000_501;
const BOB_SHIP: i64 = 100_000_502;
const CAROL_SHIP: i64 = 100_000_503;
/// The universe fixture's invented high-sec and low-sec systems.
const HIGH: i64 = 39_000_001;
const LOW: i64 = 39_000_002;
const T0: BlueTime = 134_000_000_000_000_000;
const SEC: i64 = FILETIME_SECOND;
/// An invented ship type for the victim row.
const VICTIM_TYPE: i64 = 990_500_001;

fn pilot(id: i64, name: &str, ship: i64) -> Character {
    Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ..Character::default()
    }
}

/// Alice, Bob and Carol, the listed ones undocked into the system with their
/// parks bound and every push drained.
fn world(in_space: &[i64]) -> Registry {
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", ALICE_SHIP),
            pilot(BOB, "Bob", BOB_SHIP),
            pilot(CAROL, "Carol", CAROL_SHIP),
        ],
        ..SandboxConfig::default()
    });
    for who in in_space {
        crate::destiny::on_ballpark_bind(&mut reg, Some(*who));
    }
    let _ = reg.take_pending();
    reg
}

/// Everything on the clock, as `(audience, name, args)`.
fn due(reg: &mut Registry) -> Vec<(Vec<i64>, &'static str, Vec<PyValue>)> {
    reg.take_due(Instant::now() + Duration::from_secs(600))
        .into_iter()
        .map(|p| {
            let (name, args) = p.to_push();
            (p.audience().unwrap_or_default(), name, args)
        })
        .collect()
}

fn crime(act: Act, at: BlueTime) -> Crime {
    Crime {
        offender: ALICE,
        victim: Some(BOB),
        act,
        system: HIGH,
        victim_type: VICTIM_TYPE,
        at,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

// ---------------------------------------------------------------------------
// The flags
// ---------------------------------------------------------------------------

#[test]
fn outlaw_is_the_rounded_status_at_or_below_the_cut() {
    assert!(is_outlaw(OUTLAW_AT));
    assert!(is_outlaw(OUTLAW_AT - 3.0));
    assert!(is_outlaw(MIN_STATUS));
    // `round(x, 1)` is what the client tests, so a hair above the cut that
    // rounds onto it is an outlaw, and one that rounds off it is not.
    assert!(is_outlaw(OUTLAW_AT + 0.04));
    assert!(!is_outlaw(OUTLAW_AT + 0.06));
    assert!(!is_outlaw(0.0));
}

#[test]
fn dangerous_is_between_the_cut_and_just_below_zero() {
    assert!(!is_dangerous(0.0));
    assert!(!is_dangerous(DANGEROUS_BELOW), "-0.1 > -0.1 is false");
    assert!(is_dangerous(DANGEROUS_BELOW - 0.01));
    assert!(is_dangerous(OUTLAW_AT), "the cut itself is still orange");
    assert!(!is_dangerous(OUTLAW_AT - 0.01));
    assert!(!is_dangerous(5.0));
}

#[test]
fn flags_change_only_where_the_overview_would_notice() {
    assert_eq!(flags(-1.0), flags(-4.0), "orange both sides of -2.5");
    assert_ne!(flags(0.0), flags(-1.0), "clean to orange");
    assert_ne!(flags(-4.0), flags(-6.0), "orange to red");
    assert_eq!(flags(-6.0), flags(-9.0));
    assert_eq!(flags(2.0), flags(9.0));
}

#[test]
fn the_attack_bands_are_the_clients() {
    assert_eq!(band_attacked_in(-1.99), None);
    assert_eq!(band_attacked_in(0.0), None);
    assert_eq!(band_attacked_in(10.0), None);
    // Each threshold is inclusive, and its neighbour above is the next band.
    for (i, (at, sec)) in ATTACK_BANDS.iter().enumerate() {
        assert_eq!(band_attacked_in(*at), Some(*sec), "at {at}");
        if let Some((_, above)) = ATTACK_BANDS.get(i + 1) {
            assert_eq!(
                band_attacked_in(*at + 0.01),
                Some(*above),
                "just above {at}"
            );
        }
    }
    assert_eq!(band_attacked_in(MIN_STATUS), Some(0.5));
    // The lower the status, the wider the net.
    let mut last = f64::INFINITY;
    for s in [-2.0, -2.6, -3.1, -3.6, -4.1, -4.6] {
        let sec = band_attacked_in(s).expect("a band");
        assert!(sec < last, "{s}");
        last = sec;
    }
}

#[test]
fn police_attack_a_pilot_in_systems_at_or_above_the_band() {
    // At -3.0 the band is 0.8 and up.
    assert!(police_attack_here(-3.0, 1.0));
    assert!(police_attack_here(-3.0, 0.8));
    assert!(
        police_attack_here(-3.0, 0.7999),
        "the client rounds to tenths"
    );
    assert!(!police_attack_here(-3.0, 0.7));
    assert!(!police_attack_here(-3.0, 0.0));
    assert!(!police_attack_here(-1.0, 1.0), "above -2.0 nobody comes");
    assert!(police_attack_here(-4.6, 0.5));
    assert!(!police_attack_here(-4.6, 0.4));
}

#[test]
fn a_status_change_that_enters_the_net_is_a_crossing() {
    // At -3.0 the net starts at 0.8: a 0.9 system is inside it.
    assert!(crossed_into_police_net(-1.0, -3.0, 0.9));
    assert!(!crossed_into_police_net(-3.0, -3.1, 0.9), "already inside");
    assert!(!crossed_into_police_net(-3.0, -1.0, 0.9), "leaving it");
    assert!(
        !crossed_into_police_net(-1.0, -3.0, 0.5),
        "a 0.5 system is below the 0.8 net"
    );
    // A deeper fall widens the net to a system that was outside it.
    assert!(crossed_into_police_net(-3.0, -4.6, 0.5));
}

#[test]
fn police_are_called_on_an_in_space_pilot_in_the_net_of_his_system() {
    // The universe fixture's high-sec system has no owner faction; give the
    // pilot a status that the net covers there.
    let mut reg = world(&[ALICE]);
    let here = reg.roster().get(ALICE).expect("in space").solar_system_id;
    // The sandbox home is not in the fixture universe, so nobody is called.
    assert_eq!(police_call(&reg, ALICE), None);

    let mut c = crate::content::Content::fixture();
    let u = crate::celestial::Universe::parse(
        &[format!(
            "{here}\t19000001\t29000001\t\t500001\t0.9\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented empire"
        )],
        &["19000001\t\t\t\t\tInvented region".to_owned()],
        &["29000001\t19000001\t\t\t\tInvented constellation".to_owned()],
    )
    .expect("invented rows");
    c.insert("universe", u);
    reg = reg.with_content(std::sync::Arc::new(c));
    assert_eq!(police_call(&reg, ALICE), None, "a clean pilot");

    set_security_status(&mut reg, ALICE, -1.9);
    assert_eq!(police_call(&reg, ALICE), None, "above the net");
    set_security_status(&mut reg, ALICE, -2.6);
    let got = police_call(&reg, ALICE).expect("inside the 0.9 net");
    assert_eq!(got.character, ALICE);
    assert_eq!(got.system, here);
    assert_eq!(got.security, 0.9);
    assert_eq!(got.faction, Some(500_001));
    // Docked, nobody comes.
    let docked = world(&[]);
    assert_eq!(police_call(&docked, ALICE), None);
}

// ---------------------------------------------------------------------------
// The penalty rule
// ---------------------------------------------------------------------------

#[test]
fn a_ship_aggression_costs_the_base_fraction_of_the_distance_to_minus_ten() {
    // Two clean pilots in a 1.0 system: no victim factor, no scaling.
    let m = fraction(Act::ShipAggression, 0.0, 0.0, 1.0);
    assert!(near(m, SHIP_AGGRESSION_M));
    let p = penalty(Act::ShipAggression, 0.0, 0.0, 1.0);
    assert!(near(p, -(10.0 * SHIP_AGGRESSION_M)));
    assert!(near(after_loss(0.0, m), p));
}

#[test]
fn the_loss_shrinks_toward_minus_ten() {
    let at_zero = penalty(Act::ShipAggression, -4.0, 0.0, 1.0);
    let at_minus_four = penalty(Act::ShipAggression, -4.0, -4.0, 1.0);
    assert!(at_minus_four < 0.0);
    assert!(
        at_minus_four.abs() < at_zero.abs(),
        "a pilot at -4 loses fewer points than one at 0"
    );
    // And exactly: the points are the remaining distance times the fraction.
    let m4 = fraction(Act::ShipAggression, -4.0, -4.0, 1.0);
    assert!(near(at_minus_four, -(6.0 * m4)));
    // Nothing is left to lose at the floor.
    assert_eq!(penalty(Act::ShipAggression, 0.0, MIN_STATUS, 1.0), 0.0);
}

#[test]
fn the_fraction_scales_with_security_and_the_victims_standing() {
    let at = |victim, offender, sec| fraction(Act::ShipAggression, victim, offender, sec);
    // Quieter space costs less.
    assert!(at(0.0, 0.0, 0.5) < at(0.0, 0.0, 1.0));
    assert!(near(at(0.0, 0.0, 0.5) * 2.0, at(0.0, 0.0, 1.0)));
    // A victim better than the offender costs more, a worse one less.
    assert!(at(5.0, 0.0, 1.0) > at(0.0, 0.0, 1.0));
    assert!(at(-5.0, 0.0, 1.0) < at(0.0, 0.0, 1.0));
    assert!(near(at(5.0, 0.0, 1.0), SHIP_AGGRESSION_M * 1.05));
    // The security is floored, so a null-sec-adjacent system is not free.
    assert!(near(at(0.0, 0.0, 0.0), SHIP_AGGRESSION_M * MIN_SECURITY));
    assert_eq!(at(0.0, 0.0, -1.0), at(0.0, 0.0, 0.0));
}

#[test]
fn a_capsule_costs_more_than_a_ship_and_a_facility_is_flat() {
    let ship = fraction(Act::ShipAggression, 0.0, 0.0, 1.0);
    let pod = fraction(Act::CapsuleAggression, 0.0, 0.0, 1.0);
    assert!(near(pod / ship, CAPSULE_AGGRESSION_M / SHIP_AGGRESSION_M));
    for act in [Act::AttackConcord, Act::AttackFacility] {
        assert_eq!(fraction(act, 3.0, -2.0, 1.0), FACILITY_M);
        assert_eq!(fraction(act, -9.0, 4.0, 0.2), FACILITY_M);
    }
    assert_eq!(fraction(Act::KillConcord, 0.0, 0.0, 1.0), POLICE_KILL_M);
    assert_eq!(
        fraction(Act::Assistance, 0.0, 0.0, 1.0),
        fraction(Act::ShipAggression, 0.0, 0.0, 1.0),
        "assistance is priced as a ship aggression"
    );
}

#[test]
fn a_fraction_never_leaves_zero_to_one() {
    for act in [
        Act::ShipAggression,
        Act::CapsuleAggression,
        Act::Assistance,
        Act::AttackConcord,
        Act::AttackFacility,
        Act::KillConcord,
    ] {
        for (v, o, s) in [(10.0, -10.0, 1.0), (-10.0, 10.0, 1.0), (0.0, 0.0, 1e9)] {
            let m = fraction(act, v, o, s);
            assert!((0.0..=1.0).contains(&m), "{act:?} {v} {o} {s}: {m}");
        }
    }
    // A loss never crosses the floor, a gain never the ceiling.
    assert_eq!(after_loss(-9.99, 1.0), MIN_STATUS);
    assert_eq!(after_loss(MIN_STATUS, 0.5), MIN_STATUS);
    assert_eq!(after_gain(9.99, 1.0), MAX_STATUS);
    assert!(near(after_gain(0.0, 0.1), 1.0));
}

#[test]
fn each_act_has_the_clients_message_key() {
    assert_eq!(
        Act::ShipAggression.message(),
        "SecurityStatusAggressionChange"
    );
    assert_eq!(
        Act::CapsuleAggression.message(),
        "SecurityStatusAggressionChange"
    );
    assert_eq!(Act::Assistance.message(), "SecurityStatusAssistanceChange");
    assert_eq!(
        Act::AttackConcord.message(),
        "SecurityStatusPoliceAggressionChange"
    );
    assert_eq!(
        Act::AttackFacility.message(),
        "SecurityStatusPoliceAggressionChange"
    );
    assert_eq!(Act::KillConcord.message(), "SecurityStatusPoliceKillChange");
}

#[test]
fn the_histories_change_is_the_signed_fraction_of_the_distance_covered() {
    // A loss: the fraction of the distance to -10, negative.
    let m = fraction(Act::ShipAggression, 0.0, -2.0, 1.0);
    let after = after_loss(-2.0, m);
    assert!(near(modification(-2.0, after).expect("a change"), -m));
    // A gain: the fraction of the distance to +10, positive.
    let gain = modification(-2.0, -1.5).expect("a change");
    assert!(near(gain, 0.5 / 12.0));
    // No span, no fraction.
    assert_eq!(modification(MIN_STATUS, MIN_STATUS), None);
    assert_eq!(modification(MAX_STATUS, MAX_STATUS), None);
}

// ---------------------------------------------------------------------------
// One crime
// ---------------------------------------------------------------------------

#[test]
fn a_crime_lowers_the_status_tells_the_pilot_and_writes_a_row() {
    let mut reg = world(&[]);
    let sec = system_security(&reg, HIGH).expect("an invented high-sec system");
    let want = penalty(Act::ShipAggression, 0.0, 0.0, sec);
    assert!(want < 0.0);

    let got = on_crime(&mut reg, &crime(Act::ShipAggression, T0));
    assert_eq!(got, Some(want));
    assert!(near(security_status(&reg, ALICE), want));
    assert_eq!(security_status(&reg, BOB), 0.0, "the victim loses nothing");

    // The history row: event 379, the victim, his ship type and the system.
    let rows = transactions(&reg, ALICE);
    assert_eq!(rows.len(), 1);
    let t = &rows[0];
    assert_eq!(t.event, EVENT_ILLEGAL_AGGRESSION);
    assert_eq!(t.date, T0);
    assert_eq!(t.other_owner, BOB);
    assert_eq!(t.other_type, VICTIM_TYPE);
    assert_eq!(t.location, HIGH);
    assert!(near(t.new_value, want));
    let m = fraction(Act::ShipAggression, 0.0, 0.0, sec);
    assert!(near(t.modification.expect("a change"), -m));

    // The status push, then the client's message with the change in points.
    let pushes = due(&mut reg);
    let names: Vec<&str> = pushes.iter().map(|(_, n, _)| *n).collect();
    assert!(names.contains(&"OnSecurityStatusUpdate"), "{names:?}");
    let (to, _, args) = pushes
        .iter()
        .find(|(_, n, _)| *n == "OnRemoteMessage")
        .expect("the message");
    assert_eq!(to, &vec![ALICE]);
    assert_eq!(
        args[0],
        PyValue::Bytes(b"SecurityStatusAggressionChange".to_vec())
    );
    assert_eq!(
        crate::pyutil::dict_get(&args[1], b"penalty"),
        Some(&PyValue::Real(want))
    );
    assert!(
        reg.take_pending().is_empty(),
        "nothing goes to the caller's session"
    );
}

#[test]
fn a_capsule_is_priced_as_a_capsule_and_concord_flat() {
    let mut reg = world(&[]);
    let sec = system_security(&reg, HIGH).expect("a system");
    let pod = on_crime(&mut reg, &crime(Act::CapsuleAggression, T0)).expect("a loss");
    assert!(near(pod, penalty(Act::CapsuleAggression, 0.0, 0.0, sec)));

    let mut reg = world(&[]);
    let c = Crime {
        victim: None,
        ..crime(Act::AttackConcord, T0)
    };
    let hit = on_crime(&mut reg, &c).expect("a loss");
    assert!(near(hit, -(10.0 * FACILITY_M)));
    let t = &transactions(&reg, ALICE)[0];
    assert_eq!(t.other_owner, OWNER_CONCORD, "the panel names CONCORD");
    let (_, _, args) = due(&mut reg)
        .into_iter()
        .find(|(_, n, _)| *n == "OnRemoteMessage")
        .expect("a message");
    assert_eq!(
        args[0],
        PyValue::Bytes(b"SecurityStatusPoliceAggressionChange".to_vec())
    );
}

#[test]
fn a_repeat_against_the_same_victim_inside_the_window_is_swallowed() {
    let mut reg = world(&[]);
    assert!(on_crime(&mut reg, &crime(Act::ShipAggression, T0)).is_some());
    let after_first = security_status(&reg, ALICE);

    let inside = T0 + SUPPRESS.as_secs() as i64 * SEC - 1;
    assert_eq!(
        on_crime(&mut reg, &crime(Act::ShipAggression, inside)),
        None
    );
    assert_eq!(security_status(&reg, ALICE), after_first);
    assert_eq!(transactions(&reg, ALICE).len(), 1, "no second row");

    // Another victim is another act.
    let other = Crime {
        victim: Some(CAROL),
        ..crime(Act::ShipAggression, inside)
    };
    assert!(on_crime(&mut reg, &other).is_some());

    // And the window ends.
    let outside = T0 + SUPPRESS.as_secs() as i64 * SEC;
    assert!(on_crime(&mut reg, &crime(Act::ShipAggression, outside)).is_some());
    assert_eq!(transactions(&reg, ALICE).len(), 3);
}

#[test]
fn a_pilot_with_nothing_to_lose_and_a_stranger_cost_nothing() {
    let mut reg = world(&[]);
    set_security_status(&mut reg, ALICE, MIN_STATUS);
    let _ = due(&mut reg);
    assert_eq!(on_crime(&mut reg, &crime(Act::ShipAggression, T0)), None);
    assert!(transactions(&reg, ALICE).is_empty());
    assert!(due(&mut reg).is_empty(), "no message for no change");

    let stranger = Crime {
        offender: 1,
        ..crime(Act::ShipAggression, T0)
    };
    assert_eq!(on_crime(&mut reg, &stranger), None);
}

#[test]
fn an_unlisted_system_is_priced_at_the_fallback_security() {
    let mut reg = world(&[]);
    let unlisted = Crime {
        system: 1,
        ..crime(Act::ShipAggression, T0)
    };
    assert_eq!(system_security(&reg, 1), None);
    let got = on_crime(&mut reg, &unlisted).expect("a loss");
    assert!(near(
        got,
        penalty(Act::ShipAggression, 0.0, 0.0, FALLBACK_SECURITY)
    ));
}

#[test]
fn the_victims_standing_enters_the_price_of_a_real_crime() {
    let mut reg = world(&[]);
    set_security_status(&mut reg, BOB, MIN_STATUS / 2.0);
    set_security_status(&mut reg, CAROL, MAX_STATUS / 2.0);
    let _ = due(&mut reg);
    let sec = system_security(&reg, HIGH).expect("a system");
    let outlaw = on_crime(&mut reg, &crime(Act::ShipAggression, T0)).expect("a loss");
    assert!(near(
        outlaw,
        penalty(Act::ShipAggression, MIN_STATUS / 2.0, 0.0, sec)
    ));
    // Carol is the same act on the same footing, one window later.
    let mut reg2 = world(&[]);
    set_security_status(&mut reg2, CAROL, MAX_STATUS / 2.0);
    let saint = on_crime(
        &mut reg2,
        &Crime {
            victim: Some(CAROL),
            ..crime(Act::ShipAggression, T0)
        },
    )
    .expect("a loss");
    assert!(near(
        saint,
        penalty(Act::ShipAggression, MAX_STATUS / 2.0, 0.0, sec)
    ));
    assert!(outlaw.abs() < saint.abs(), "a saint costs more to shoot");
}

// ---------------------------------------------------------------------------
// The history
// ---------------------------------------------------------------------------

fn gm_row(n: i64) -> Tx {
    Tx {
        event: EVENT_GM_MODIFICATION,
        date: T0 + n,
        modification: None,
        new_value: n as f64,
        other_owner: 0,
        other_type: 0,
        location: 0,
        start: None,
    }
}

#[test]
fn rows_round_trip_and_come_newest_first() {
    let mut reg = world(&[]);
    for n in 0..3 {
        record(&mut reg, ALICE, gm_row(n));
    }
    let full = Tx {
        event: EVENT_ILLEGAL_AGGRESSION,
        date: T0 + 9,
        modification: Some(-0.0125),
        new_value: -3.25,
        other_owner: BOB,
        other_type: VICTIM_TYPE,
        location: HIGH,
        start: Some(-3.0),
    };
    record(&mut reg, ALICE, full.clone());
    let got = transactions(&reg, ALICE);
    assert_eq!(got.len(), 4);
    assert_eq!(got[0], full, "newest first, every field kept");
    assert_eq!(
        got[1..].iter().map(|t| t.new_value).collect::<Vec<_>>(),
        vec![2.0, 1.0, 0.0]
    );
    assert!(
        transactions(&reg, BOB).is_empty(),
        "histories are per pilot"
    );
}

#[test]
fn a_history_keeps_only_the_newest_rows() {
    let mut reg = world(&[]);
    let total = TX_KEEP as i64 + 7;
    for n in 0..total {
        record(&mut reg, ALICE, gm_row(n));
    }
    record(&mut reg, BOB, gm_row(0));
    let got = transactions(&reg, ALICE);
    assert_eq!(got.len(), TX_KEEP);
    assert_eq!(got[0].new_value, (total - 1) as f64);
    assert_eq!(
        got.last().map(|t| t.new_value),
        Some((total - TX_KEEP as i64) as f64),
        "the oldest rows went"
    );
    assert_eq!(transactions(&reg, BOB).len(), 1, "another pilot's are safe");
}

#[test]
fn a_pilot_whose_id_prefixes_anothers_does_not_share_a_history() {
    // `90000501` and `900005010`: a key prefix must not bleed across pilots.
    let mut reg = world(&[]);
    record(&mut reg, ALICE, gm_row(1));
    record(&mut reg, ALICE * 10, gm_row(2));
    assert_eq!(transactions(&reg, ALICE).len(), 1);
    assert_eq!(transactions(&reg, ALICE * 10).len(), 1);
}

#[test]
fn rat_gains_fold_into_one_row_per_bounty_period_and_owner() {
    let mut reg = world(&[]);
    let owner = 1_000_501;
    let period = BOUNTY_PERIOD.as_secs() as i64 * SEC;
    record_kill_gain(&mut reg, ALICE, -2.0, -1.9, owner, HIGH, T0);
    record_kill_gain(&mut reg, ALICE, -1.9, -1.8, owner, HIGH, T0 + period - 1);
    let rows = transactions(&reg, ALICE);
    assert_eq!(rows.len(), 1, "two kills, one row");
    let r = &rows[0];
    assert_eq!(r.event, EVENT_KILL_PIRATE_NPC);
    assert_eq!(r.date, T0, "it keeps its first date");
    assert_eq!(r.other_owner, owner);
    assert_eq!(r.new_value, -1.8, "and takes the newest value");
    assert!(near(
        r.modification.expect("a change"),
        modification(-2.0, -1.8).expect("a change")
    ));

    // A third kill after the period starts a row; another owner's does too.
    record_kill_gain(&mut reg, ALICE, -1.8, -1.7, owner, HIGH, T0 + period);
    assert_eq!(transactions(&reg, ALICE).len(), 2);
    record_kill_gain(
        &mut reg,
        ALICE,
        -1.7,
        -1.6,
        owner + 1,
        HIGH,
        T0 + period + 1,
    );
    assert_eq!(transactions(&reg, ALICE).len(), 3);
    // A different kind of row in between also breaks the fold.
    record(&mut reg, ALICE, gm_row(0));
    record_kill_gain(
        &mut reg,
        ALICE,
        -1.6,
        -1.5,
        owner + 1,
        HIGH,
        T0 + period + 2,
    );
    assert_eq!(transactions(&reg, ALICE).len(), 5);
}

#[test]
fn the_history_is_a_list_of_keyvals_the_panel_reads_by_attribute() {
    let mut reg = world(&[]);
    on_crime(&mut reg, &crime(Act::ShipAggression, T0));
    let PyValue::List(rows) = transactions_value(&reg, ALICE) else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    let PyValue::Instance { class, state } = &rows[0] else {
        panic!("a KeyVal: {:?}", rows[0])
    };
    assert_eq!(class.as_slice(), crate::pyutil::KEYVAL_CLASS.as_bytes());
    for name in [
        "eventTypeID",
        "eventDate",
        "modification",
        "newValue",
        "otherOwnerID",
        "otherTypeID",
        "locationID",
    ] {
        assert!(
            crate::pyutil::dict_get(state, name.as_bytes()).is_some(),
            "securityStatusPanel reads .{name}"
        );
    }
    assert_eq!(
        crate::pyutil::dict_get(state, b"eventTypeID"),
        Some(&PyValue::Int(EVENT_ILLEGAL_AGGRESSION))
    );
    assert!(matches!(
        crate::pyutil::dict_get(state, b"newValue"),
        Some(PyValue::Real(_))
    ));
    // A pilot with none gets an empty list, not None.
    assert_eq!(transactions_value(&reg, BOB), PyValue::List(vec![]));

    // A row with no change prints nothing: `modification` is `None`.
    record(&mut reg, BOB, gm_row(0));
    let PyValue::List(rows) = transactions_value(&reg, BOB) else {
        panic!("a list")
    };
    let PyValue::Instance { state, .. } = &rows[0] else {
        panic!("a KeyVal")
    };
    assert_eq!(
        crate::pyutil::dict_get(state, b"modification"),
        Some(&PyValue::None)
    );
}

#[test]
fn a_damaged_row_is_skipped_not_fatal() {
    let mut reg = world(&[]);
    record(&mut reg, ALICE, gm_row(1));
    reg.deltas_mut().put(
        TX_KIND,
        tx_key(ALICE, 99),
        vec!["not".to_owned(), "a".to_owned(), "row".to_owned()],
    );
    assert_eq!(transactions(&reg, ALICE).len(), 1);
}

// ---------------------------------------------------------------------------
// The slim
// ---------------------------------------------------------------------------

#[test]
fn a_pilots_ship_slim_carries_the_status_and_an_unknown_ball_does_not() {
    let mut reg = world(&[ALICE, BOB]);
    set_security_status(&mut reg, ALICE, -3.25);
    let ship = reg.roster().get(ALICE).expect("in space").ship_id;
    let other = reg.roster().get(BOB).expect("in space").ship_id;
    let got = slim_extras(&reg, &[ship, other, 12345]);
    assert_eq!(got.len(), 2, "an unknown id has nothing");
    assert_eq!(
        got[0],
        (ship, vec![("securityStatus", PyValue::Real(-3.25))])
    );
    assert_eq!(
        got[1],
        (other, vec![("securityStatus", PyValue::Real(0.0))]),
        "a clean record is the float 0.0, not absent"
    );
    // The seam really carries it.
    let via = crate::slim_extra::for_slims(&reg, &[ship]);
    assert!(
        via.iter()
            .any(|(id, kv)| *id == ship && kv.iter().any(|(k, _)| *k == "securityStatus")),
        "SOURCES has the row: {via:?}"
    );
}

#[test]
fn a_docked_pilot_has_no_ball_so_no_extras() {
    let reg = world(&[]);
    assert!(slim_extras(&reg, &[ALICE_SHIP]).is_empty());
}

/// The slim changes among the pushes on the clock, as `(audience, itemID,
/// securityStatus)`.
fn slim_changes(reg: &mut Registry) -> Vec<(Vec<i64>, i64, Option<PyValue>)> {
    due(reg)
        .into_iter()
        .filter(|(_, n, _)| *n == "DoDestinyUpdate")
        .flat_map(|(to, _, args)| {
            let PyValue::List(entries) = &args[0] else {
                panic!("a batch")
            };
            entries
                .iter()
                .filter_map(|e| {
                    let PyValue::Tuple(e) = e else { return None };
                    let PyValue::Tuple(call) = &e[1] else {
                        return None;
                    };
                    if call[0] != PyValue::Bytes(b"OnSlimItemChange".to_vec()) {
                        return None;
                    }
                    let PyValue::Tuple(a) = &call[1] else {
                        return None;
                    };
                    let PyValue::Int(id) = a[0] else { return None };
                    let PyValue::Instance { state, .. } = &a[1] else {
                        panic!("the slim in its instance form")
                    };
                    Some((
                        to.clone(),
                        id,
                        crate::pyutil::dict_get(state, b"securityStatus").cloned(),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn crossing_a_flag_boundary_refreshes_the_slim_for_everyone_in_the_system() {
    let mut reg = world(&[ALICE, BOB]);
    let ship = reg.roster().get(ALICE).expect("in space").ship_id;

    // Clean to orange: the -0.1 crossing.
    set_security_status(&mut reg, ALICE, -0.5);
    let got = slim_changes(&mut reg);
    assert!(!got.is_empty(), "the overview must re-colour");
    for (_, id, status) in &got {
        assert_eq!(*id, ship);
        assert_eq!(*status, Some(PyValue::Real(-0.5)));
    }
    let heard: std::collections::BTreeSet<i64> =
        got.iter().flat_map(|(to, _, _)| to.clone()).collect();
    assert_eq!(
        heard,
        [ALICE, BOB].into_iter().collect(),
        "the pilot and the one beside him"
    );

    // Within a band nothing is sent.
    set_security_status(&mut reg, ALICE, -3.0);
    assert!(slim_changes(&mut reg).is_empty());

    // Orange to red: the -5.0 crossing.
    set_security_status(&mut reg, ALICE, -5.5);
    let got = slim_changes(&mut reg);
    assert!(!got.is_empty());
    assert!(
        got.iter().all(|(_, _, s)| *s == Some(PyValue::Real(-5.5))),
        "{got:?}"
    );

    // And back.
    set_security_status(&mut reg, ALICE, 0.0);
    assert!(!slim_changes(&mut reg).is_empty());
}

#[test]
fn a_pilot_docked_when_the_flag_flips_sends_nothing() {
    let mut reg = world(&[BOB]);
    set_security_status(&mut reg, ALICE, -6.0);
    assert!(slim_changes(&mut reg).is_empty());
    assert_eq!(security_status(&reg, ALICE), -6.0);
}

#[test]
fn the_slim_change_keeps_the_pilots_other_slim_attributes() {
    let mut reg = world(&[ALICE]);
    let ship = reg.roster().get(ALICE).expect("in space").ship_id;
    set_security_status(&mut reg, ALICE, -6.0);
    let p = due(&mut reg)
        .into_iter()
        .find(|(_, n, _)| *n == "DoDestinyUpdate")
        .expect("a slim change");
    let PyValue::List(entries) = &p.2[0] else {
        panic!("a batch")
    };
    let PyValue::Tuple(e) = &entries[0] else {
        panic!("an entry")
    };
    let PyValue::Tuple(call) = &e[1] else {
        panic!("a call")
    };
    let PyValue::Tuple(a) = &call[1] else {
        panic!("args")
    };
    let PyValue::Instance { state, .. } = &a[1] else {
        panic!("a slim")
    };
    for (key, want) in [
        ("itemID", PyValue::Int(ship)),
        ("ownerID", PyValue::Int(ALICE)),
        ("charID", PyValue::Int(ALICE)),
    ] {
        assert_eq!(
            crate::pyutil::dict_get(state, key.as_bytes()),
            Some(&want),
            "{key}"
        );
    }
    assert!(crate::pyutil::dict_get(state, b"typeID").is_some());
    assert!(crate::pyutil::dict_get(state, b"corpID").is_some());
}

// ---------------------------------------------------------------------------
// The CP50 gain writes a 378 row
// ---------------------------------------------------------------------------

#[test]
fn a_rat_kill_writes_a_378_row_and_the_next_one_folds_into_it() {
    use crate::kill_hooks::KillEvent;

    // An invented NPC type with the two attributes the credit reads
    // (252 and 563), in a world that has the type table.
    const RAT_TYPE: i64 = 990_500_100;
    let rows = format!(
        "{}type\t{RAT_TYPE}\t99703\t11\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t252=0.25;563=5.0\tFixture Pirate\n",
        crate::typedata::FIXTURE
    );
    let table = crate::typedata::Table::parse(&rows).expect("rows parse");
    let mut content = crate::content::Content::fixture();
    content.insert("typedata", crate::typedata::Installed::build(table));
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![pilot(ALICE, "Alice", ALICE_SHIP)],
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content));
    set_security_status(&mut reg, ALICE, -2.0);
    let _ = due(&mut reg);

    let ev = |when| KillEvent {
        victim_character: crate::npc::MIN_NPC_ID,
        victim_ship: crate::npc::MIN_NPC_ID,
        ship_type: RAT_TYPE,
        killer: Some(ALICE),
        system: LOW,
        npc: Some(&crate::npc::FIXTURE_RAT),
        when,
        park: Vec::new(),
    };
    crate::npc::bounty::on_kill(&mut reg, &ev(T0));
    let first = security_status(&reg, ALICE);
    assert!(first > -2.0, "the kill raised the status");
    let rows = transactions(&reg, ALICE);
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.event, EVENT_KILL_PIRATE_NPC);
    assert_eq!(r.other_owner, crate::npc::FIXTURE_RAT.owner_corp_id);
    assert_eq!(r.location, LOW);
    assert_eq!(r.new_value, first);
    assert!(near(
        r.modification.expect("a change"),
        modification(-2.0, first).expect("a change")
    ));

    crate::npc::bounty::on_kill(&mut reg, &ev(T0 + SEC));
    let rows = transactions(&reg, ALICE);
    assert_eq!(rows.len(), 1, "folded");
    assert_eq!(rows[0].new_value, security_status(&reg, ALICE));
    assert!(near(
        rows[0].modification.expect("a change"),
        modification(-2.0, rows[0].new_value).expect("a change")
    ));
}

// ---------------------------------------------------------------------------
// The outlaw at a station door
// ---------------------------------------------------------------------------

const STATION_HIGH: i64 = 60_500_001;
const STATION_LOW: i64 = 60_500_002;

/// A world whose catalogue and station table place two invented stations,
/// one in each of the universe fixture's high-sec and low-sec systems.
fn station_world(ship_type: i64) -> Registry {
    let catalogue = crate::celestial::Catalogue::parse(&format!(
        "#kind\tid\ttype_id\tradius\tx\ty\tz\tsystem\tdest\tdest_system\n\
         #system\t{HIGH}\t29000001\t19000001\n\
         station\t{STATION_HIGH}\t1529\t0.0\t1.0\t2.0\t3.0\t{HIGH}\t0\t0\n\
         station\t{STATION_LOW}\t1529\t0.0\t4.0\t5.0\t6.0\t{LOW}\t0\t0\n"
    ));
    let mut stations = crate::stations::Table::default();
    for (id, system) in [(STATION_HIGH, HIGH), (STATION_LOW, LOW)] {
        stations.insert(
            id,
            crate::stations::Station {
                system_id: system,
                region_id: 19_000_001,
                owner_id: 1_999_501,
                operation_id: 99,
            },
        );
    }
    let mut content = crate::content::Content::fixture();
    content.insert("stations", stations);
    let mut c = pilot(ALICE, "Alice", ALICE_SHIP);
    c.ship_type_id = ship_type;
    Registry::new(SandboxConfig {
        celestials: catalogue,
        characters: vec![c],
        ..SandboxConfig::default()
    })
    .with_content(std::sync::Arc::new(content))
}

fn raised_key(err: &ServiceError) -> String {
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

/// An invented ship type, not the capsule.
const A_SHIP_TYPE: i64 = 990_000_001;

#[test]
fn an_outlaws_ship_is_turned_away_from_high_sec_but_not_the_capsule() {
    let mut reg = station_world(A_SHIP_TYPE);
    set_security_status(&mut reg, ALICE, OUTLAW_AT);
    let err = refuse_dock(&reg, Some(ALICE), Some(STATION_HIGH)).expect_err("refused");
    assert_eq!(raised_key(&err), DENIED_CRIMINAL);

    // Low-sec, an unplaced station, and no actor or no station are all open.
    assert!(refuse_dock(&reg, Some(ALICE), Some(STATION_LOW)).is_ok());
    assert!(refuse_dock(&reg, Some(ALICE), Some(1)).is_ok());
    assert!(refuse_dock(&reg, None, Some(STATION_HIGH)).is_ok());
    assert!(refuse_dock(&reg, Some(ALICE), None).is_ok());

    // One step above the cut is an ordinary criminal, not an outlaw.
    set_security_status(&mut reg, ALICE, OUTLAW_AT + 0.5);
    assert!(refuse_dock(&reg, Some(ALICE), Some(STATION_HIGH)).is_ok());

    // The capsule is let in.
    let mut pod = station_world(crate::damage::CAPSULE_TYPE_ID);
    set_security_status(&mut pod, ALICE, MIN_STATUS);
    assert!(refuse_dock(&pod, Some(ALICE), Some(STATION_HIGH)).is_ok());
}

#[test]
fn the_dock_call_itself_turns_an_outlaws_ship_away() {
    // Through the `CmdDock` arm, not just the rule: the refusal comes before
    // any docking is attempted.
    let dock = |reg: &mut Registry, station: i64| {
        reg.dispatch(&crate::service::Request {
            service: "beyonce",
            method: "CmdDock",
            args: &[PyValue::Int(station), PyValue::Int(ALICE_SHIP)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
    };
    let mut reg = station_world(A_SHIP_TYPE);
    set_security_status(&mut reg, ALICE, OUTLAW_AT);
    let err = dock(&mut reg, STATION_HIGH).expect_err("refused");
    assert_eq!(raised_key(&err), DENIED_CRIMINAL);
    // Not an outlaw: the same call is not refused for that reason.
    set_security_status(&mut reg, ALICE, OUTLAW_AT + 1.0);
    if let Err(e) = dock(&mut reg, STATION_HIGH) {
        assert_ne!(raised_key(&e), DENIED_CRIMINAL);
    }
}

// ---------------------------------------------------------------------------
// The GM console
// ---------------------------------------------------------------------------

#[test]
fn secstatus_sets_a_status_and_writes_a_gm_row() {
    let mut reg = world(&[]);
    let says = |r: Result<PyValue, ServiceError>| match r {
        Ok(PyValue::Bytes(b)) => String::from_utf8(b).expect("text"),
        other => panic!("a text answer: {other:?}"),
    };
    assert!(says(slash(&mut reg, Some(ALICE), &["-4.5"])).contains("-4.5000"));
    assert_eq!(security_status(&reg, ALICE), -4.5);
    let rows = transactions(&reg, ALICE);
    assert_eq!(rows[0].event, EVENT_GM_MODIFICATION);
    assert_eq!(rows[0].modification, None);
    assert_eq!(rows[0].new_value, -4.5);

    // Somebody else, by id.
    slash(&mut reg, Some(ALICE), &[&BOB.to_string(), "2"]).expect("answers");
    assert_eq!(security_status(&reg, BOB), 2.0);
    assert_eq!(security_status(&reg, ALICE), -4.5);

    // Bad input changes nothing.
    for words in [
        &[][..],
        &["abc"][..],
        &["11"][..],
        &["-10.5"][..],
        &["NaN"][..],
        &["1", "2", "3"][..],
        &["999", "1"][..],
    ] {
        let before = security_status(&reg, ALICE);
        slash(&mut reg, Some(ALICE), words).expect("answers");
        assert_eq!(security_status(&reg, ALICE), before, "{words:?}");
    }
    assert!(says(slash(&mut reg, None, &["1"])).contains("no such"));
}

#[test]
fn the_delta_kind_is_claimed() {
    assert!(crate::delta_kinds::is_registered(TX_KIND));
}
