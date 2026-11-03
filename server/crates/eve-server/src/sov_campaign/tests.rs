//! Tests for the campaign row and its scheduling. The reinforcement and the
//! node-fight hand-off are tested with the engine (`entosis/tests.rs`).

use super::*;

const SEC: i64 = 10_000_000;
const HOUR: i64 = crate::seed::HOUR;
const DAY: i64 = crate::seed::DAY;
const SYSTEM: i64 = 30_631_001;
const HUB: i64 = 100_631_001;
const DEFENDER: i64 = 99_631_001;
const ATTACKER: i64 = 99_631_002;

fn campaign_row() -> Campaign {
    Campaign {
        system_id: SYSTEM,
        hub_id: HUB,
        event_type: crate::entosis::EVENT_TYPE_HUB,
        defender: DEFENDER,
        attackers: vec![ATTACKER, ATTACKER + 1],
        start_time: 134_000_000 * SEC,
        started: false,
        created: 133_000_000 * SEC,
        scores: vec![(DEFENDER, 0.6), (-DEFENDER, 0.4)],
        rest: vec!["d3b".to_owned(), "1,2,3".to_owned()],
    }
}

fn claim() -> Claim {
    Claim {
        system_id: SYSTEM,
        alliance_id: Some(DEFENDER),
        corporation_id: 98_631_001,
        hub_id: HUB,
        state: HubState::Claimed,
        claim_time: Some(130_000_000 * SEC),
        state_until: None,
        rest: Vec::new(),
    }
}

#[test]
fn a_campaign_round_trips_through_its_row_and_keeps_d3bs_fields() {
    let mut d = Deltas::default();
    let k = campaign_row();
    put(&mut d, &k);
    assert_eq!(campaign(&d, SYSTEM), Some(k.clone()));
    assert_eq!(all(&d), vec![k.clone()]);
    // No attackers and no scores are stored as `-`.
    let bare = Campaign {
        attackers: Vec::new(),
        scores: Vec::new(),
        rest: Vec::new(),
        ..k
    };
    put(&mut d, &bare);
    assert_eq!(campaign(&d, SYSTEM), Some(bare));
    drop_campaign(&mut d, SYSTEM);
    assert_eq!(campaign(&d, SYSTEM), None);
    // A row that does not parse reads as absent, not as a panic.
    d.put(KIND, vec![SYSTEM.to_string()], vec!["x".to_owned()]);
    assert_eq!(campaign(&d, SYSTEM), None);
}

#[test]
fn the_state_value_is_the_four_tuple_the_client_unpacks() {
    let k = campaign_row();
    let PyValue::Tuple(v) = k.state_value() else {
        panic!("a tuple")
    };
    assert_eq!(v[0], PyValue::Int(crate::entosis::EVENT_TYPE_HUB));
    assert_eq!(v[1], PyValue::Int(DEFENDER));
    assert_eq!(v[2], PyValue::Int(k.start_time));
    let PyValue::Dict(scores) = &v[3] else {
        panic!("a dict")
    };
    // {team: score}: pairs are (value, key).
    assert!(scores.contains(&(PyValue::Real(0.6), PyValue::Int(DEFENDER))));
    assert!(scores.contains(&(PyValue::Real(0.4), PyValue::Int(-DEFENDER))));
}

#[test]
fn a_campaign_starts_in_a_prime_window_at_least_the_delay_later() {
    let d = Deltas::default();
    let now = 150_000 * DAY + 5 * HOUR;
    for n in [1u32, 2, 24] {
        let clock = GameClock::new(n).expect("a scale");
        let k = schedule(&d, &claim(), vec![ATTACKER], now, clock, false);
        let delay = clock.span(REINFORCE_DELAY);
        assert!(k.start_time >= now + delay, "scale {n}");
        assert!(k.start_time < now + delay + clock.day(), "scale {n}");
        assert_eq!(k.created, now);
        assert_eq!(k.defender, DEFENDER);
        assert_eq!(k.attackers, vec![ATTACKER]);
        assert_eq!(k.event_type, crate::entosis::EVENT_TYPE_HUB);
        assert!(!k.started);
        // The start is the start of a window: the one the helper names.
        let adm = crate::sov_index::system_adm(&d, SYSTEM, now, clock);
        let want = crate::entosis::window_start_at_or_after(
            sov::DEFAULT_PRIME_HOUR,
            crate::sov_index::vulnerability_window(adm),
            now + delay,
            clock,
        );
        assert_eq!(k.start_time, want, "scale {n}");
    }
    // At scale 1 the delay is the 48 hours of R-SOV.
    assert_eq!(REINFORCE_DELAY, 48 * HOUR);
}

#[test]
fn the_gm_start_is_now_and_the_bar_starts_at_the_defenders_share() {
    let d = Deltas::default();
    let now = 150_000 * DAY;
    let k = schedule(&d, &claim(), Vec::new(), now, GameClock::REAL, true);
    assert_eq!(k.start_time, now);
    assert_eq!(
        k.scores,
        vec![
            (DEFENDER, DEFENDER_START_SCORE),
            (-DEFENDER, 1.0 - DEFENDER_START_SCORE)
        ]
    );
    let total: f64 = k.scores.iter().map(|(_, s)| s).sum();
    assert!((total - 1.0).abs() < 1e-12, "the bar sums to one");
}

#[test]
fn a_stale_campaign_is_dropped_when_it_begins() {
    // A campaign for a hub the ledger no longer holds begins nothing.
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    put(reg.deltas_mut(), &campaign_row());
    begin(&mut reg, &campaign_row());
    assert!(campaign(reg.deltas(), SYSTEM).is_none());
    assert!(reg.take_pending().is_empty());
}
