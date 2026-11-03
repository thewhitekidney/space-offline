//! CAP-F unit tests. **Invented ids**; the client's constants ([V]) are
//! named, never typed as numbers, so the tests state the formula's shape.

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::service::Request;
use std::time::Instant;

const ALICE: i64 = 90_000_901;
const BOB: i64 = 90_000_902;
const NOW: BlueTime = 134_000_000_000_000_000;
const HOUR_S: f64 = 3_600.0;

fn world() -> Registry {
    let pilot = |id: i64, name: &str, ship: i64| Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice", 100_000_901),
            pilot(BOB, "Bob", 100_000_902),
        ],
        ..SandboxConfig::default()
    });
    let _ = reg.take_pending();
    reg
}

fn secs(t: BlueTime) -> f64 {
    t as f64 / SEC as f64
}

fn get_timers(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    reg.dispatch(&Request {
        service: "jumpTimers",
        method: "GetTimers",
        args: &[],
        kwargs: &[],
        bind_params: None,
        actor,
    })
    .expect("answered")
}

#[test]
fn fatigue_after_matches_the_client_formula() {
    // No fatigue: the floor, `k * FATIGUE_SECONDS_PER_DISTANCE`.
    let (ly, m) = (5.0, 1.0);
    let k = ADDITIONAL_DISTANCE + ly * m;
    assert_eq!(fatigue_after(0.0, ly, m), k * FATIGUE_SECONDS_PER_DISTANCE);
    // Fatigue above the floor multiplies by k.
    let f = HOUR_S / 2.0;
    assert_eq!(fatigue_after(f, ly, m), f * k);
}

#[test]
fn activation_is_the_larger_of_distance_and_a_tenth_of_fatigue() {
    let m = 1.0;
    // Distance dominates: k * ACTIVATION_SECONDS_PER_DISTANCE.
    let k = ADDITIONAL_DISTANCE + 4.0 * m;
    assert_eq!(
        activation_after(0.0, 4.0, m),
        k * ACTIVATION_SECONDS_PER_DISTANCE
    );
    // Fatigue dominates: remaining * FATIGUE_RATIO.
    let f = 10.0 * HOUR_S / 2.0;
    assert_eq!(activation_after(f, 0.5, m), (f * FATIGUE_RATIO).trunc());
    // Never below the one-unit minimum.
    assert_eq!(
        activation_after(0.0, 0.0, m),
        ADDITIONAL_DISTANCE * ACTIVATION_SECONDS_PER_DISTANCE
    );
}

#[test]
fn fatigue_is_capped() {
    assert_eq!(
        fatigue_after(FATIGUE_MAX_SECONDS, 3.0, 1.0),
        FATIGUE_MAX_SECONDS
    );
    // The client's clamp puts the floor last: a very long jump from a
    // clean slate is above the cap (`mathext.clamp`), as its tooltip says.
    let ly = 2.0 * FATIGUE_MAX_SECONDS / FATIGUE_SECONDS_PER_DISTANCE;
    let k = ADDITIONAL_DISTANCE + ly;
    assert_eq!(
        fatigue_after(0.0, ly, 1.0),
        (k * FATIGUE_SECONDS_PER_DISTANCE).trunc()
    );
    assert!(fatigue_after(0.0, ly, 1.0) > FATIGUE_MAX_SECONDS);
}

#[test]
fn ship_multiplier_scales_both() {
    let ly = 10.0;
    let (full, tenth) = (1.0, 0.1);
    assert!(fatigue_after(0.0, ly, tenth) < fatigue_after(0.0, ly, full));
    assert!(activation_after(0.0, ly, tenth) < activation_after(0.0, ly, full));
    let k = ADDITIONAL_DISTANCE + ly * tenth;
    assert_eq!(
        fatigue_after(0.0, ly, tenth),
        (k * FATIGUE_SECONDS_PER_DISTANCE).trunc()
    );
    // A multiplier below zero is not a negative distance.
    assert_eq!(fatigue_after(0.0, ly, -3.0), fatigue_after(0.0, ly, 0.0));
}

#[test]
fn fatigue_decays_in_real_time_and_the_next_jump_grows_from_what_is_left() {
    let clock = GameClock::REAL;
    let first = after_jump(&Timers::default(), NOW, 10.0, 1.0, clock);
    assert_eq!(first.last_updated, NOW);
    let start = secs(first.fatigue_until - NOW);
    // One hour later the remainder is an hour less.
    let later = NOW + HOUR_S as i64 * SEC;
    assert_eq!(first.fatigue_seconds(later, clock), start - HOUR_S);
    // The second jump multiplies that remainder.
    let second = after_jump(&first, later, 2.0, 1.0, clock);
    let k = ADDITIONAL_DISTANCE + 2.0;
    assert_eq!(secs(second.fatigue_until - later), (start - HOUR_S) * k);
    assert_eq!(second.last_updated, later);
    // Long after, the fatigue is gone and a jump starts clean.
    let much_later = first.fatigue_until + 1;
    let third = after_jump(&first, much_later, 2.0, 1.0, clock);
    assert_eq!(
        secs(third.fatigue_until - much_later),
        k * FATIGUE_SECONDS_PER_DISTANCE
    );
}

#[test]
fn the_game_clock_compresses_spans_and_stretches_the_remainder() {
    let clock = GameClock::new(4).unwrap();
    let t = after_jump(&Timers::default(), NOW, 5.0, 1.0, clock);
    let k = ADDITIONAL_DISTANCE + 5.0;
    let game = k * FATIGUE_SECONDS_PER_DISTANCE;
    assert_eq!(secs(t.fatigue_until - NOW), game / 4.0);
    assert_eq!(t.fatigue_seconds(NOW, clock), game);
}

#[test]
fn timers_are_zero_when_expired() {
    let t = after_jump(&Timers::default(), NOW, 1.0, 1.0, GameClock::REAL);
    assert_eq!(t.at(NOW), t);
    let over = t.activation_until.max(t.fatigue_until);
    assert_eq!(t.at(over), Timers::default());
    assert_eq!(Timers::default().at(NOW), Timers::default());
}

#[test]
fn on_jump_stores_the_row_and_tells_the_pilot() {
    let mut reg = world();
    let t = on_jump_at(&mut reg, ALICE, 5.0, 1.0, NOW);
    assert_eq!(stored(&reg, ALICE), t, "the row reads back as written");
    assert_eq!(stored(&reg, BOB), Timers::default());
    let row = reg.deltas().of(KIND).get(&vec![ALICE.to_string()]).cloned();
    assert_eq!(
        row,
        Some(vec![
            t.activation_until.to_string(),
            t.fatigue_until.to_string(),
            t.last_updated.to_string()
        ])
    );
    let pushes: Vec<_> = reg
        .take_due(Instant::now() + Duration::from_secs(600))
        .into_iter()
        .map(|p| (p.audience(), p.to_push()))
        .collect();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].0, Some(vec![ALICE]));
    assert_eq!(pushes[0].1.0, "OnJumpTimersUpdated");
    // Three values: the client caches the arguments and splats them next to
    // `doAlert=False`, so a fourth would crash the HUD's rebuild.
    assert_eq!(pushes[0].1.1, t.wire());
    assert_eq!(pushes[0].1.1.len(), 3);
}

#[test]
fn the_real_clock_entry_point_sets_a_future_timer() {
    let mut reg = world();
    let t = on_jump(&mut reg, ALICE, 3.0);
    let now = crate::pyutil::filetime_now();
    assert!(t.activation_until > now - 60 * SEC && t.fatigue_until > t.activation_until);
    assert!(activation_remaining(&reg, ALICE, t.last_updated).is_some());
    assert!(activation_remaining(&reg, ALICE, t.activation_until).is_none());
}

#[test]
fn get_timers_answers_a_three_tuple_of_ints() {
    let mut reg = world();
    // Nobody jumped, or nobody is acting: zeros, never None.
    for actor in [Some(ALICE), None] {
        assert_eq!(
            get_timers(&mut reg, actor),
            PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0), PyValue::Int(0)])
        );
    }
    // A jump a moment ago: the stored timers, in order, for Alice only.
    let now = crate::pyutil::filetime_now();
    let t = on_jump_at(&mut reg, ALICE, 5.0, 1.0, now);
    assert_eq!(get_timers(&mut reg, Some(ALICE)), PyValue::Tuple(t.wire()));
    let PyValue::Tuple(parts) = get_timers(&mut reg, Some(ALICE)) else {
        panic!("a tuple")
    };
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|p| matches!(p, PyValue::Int(n) if *n > 0)));
    assert_eq!(
        get_timers(&mut reg, Some(BOB)),
        PyValue::Tuple(vec![PyValue::Int(0), PyValue::Int(0), PyValue::Int(0)])
    );
}

#[test]
fn an_over_row_is_dropped_on_read() {
    let mut reg = world();
    on_jump_at(&mut reg, ALICE, 1.0, 1.0, NOW - 100 * HOUR_S as i64 * SEC);
    assert!(!reg.deltas().of(KIND).is_empty(), "stored as written");
    let got = timers(&mut reg, ALICE, NOW);
    assert_eq!(got, Timers::default());
    assert!(reg.deltas().of(KIND).is_empty(), "pruned");
}

#[test]
fn the_gm_command_clears_and_pushes_zeros() {
    let mut reg = world();
    on_jump_at(&mut reg, ALICE, 5.0, 1.0, crate::pyutil::filetime_now());
    let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
    slash(&mut reg, Some(ALICE), &["0"]).expect("ran");
    assert_eq!(stored(&reg, ALICE), Timers::default());
    assert!(reg.deltas().of(KIND).is_empty());
    let pushes = reg.take_due(Instant::now() + Duration::from_secs(600));
    assert_eq!(pushes.len(), 1);
    let (name, args) = pushes[0].to_push();
    assert_eq!(name, "OnJumpTimersUpdated");
    assert_eq!(
        args,
        vec![PyValue::Int(0), PyValue::Int(0), PyValue::Int(0)]
    );
    // Anything but 0 is a usage line, and changes nothing.
    on_jump_at(&mut reg, ALICE, 5.0, 1.0, crate::pyutil::filetime_now());
    slash(&mut reg, Some(ALICE), &["60"]).expect("ran");
    assert_ne!(stored(&reg, ALICE), Timers::default());
}
