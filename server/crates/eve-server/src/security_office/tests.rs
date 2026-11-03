//! SEC-1c unit tests: the tag algorithm against the client's own (computed
//! by hand from invented starts), the office's access rule and the one
//! call. **Invented ids**: the service tests run on an invented band table
//! with the client's own ranges; the real tag ids are only ever read through
//! [`BANDS`].

use super::*;
use crate::model::{Character, SandboxConfig};
use crate::npc::bounty::{security_status, set_security_status};

const ALICE: i64 = 90_000_601;
const BOB: i64 = 90_000_602;
const ALICE_SHIP: i64 = 100_000_601;
const BOB_SHIP: i64 = 100_000_602;
/// The universe fixture's invented systems: high-sec, low-sec, zero-sec.
const HIGH: i64 = 39_000_001;
const LOW: i64 = 39_000_002;
const NULL: i64 = 39_000_003;
const OTHER_STATION: i64 = 60_600_002;

/// Invented tag types with the client's four ranges, lowest first.
const TRAINER: i64 = 990_600_001;
const RECRUITER: i64 = 990_600_002;
const TRANSPORTER: i64 = 990_600_003;
const NEGOTIATOR: i64 = 990_600_004;
const TEST_BANDS: [Band; 4] = [
    Band {
        type_id: TRAINER,
        min: -10.0,
        max: -8.0,
    },
    Band {
        type_id: RECRUITER,
        min: -8.0,
        max: -5.0,
    },
    Band {
        type_id: TRANSPORTER,
        min: -5.0,
        max: -2.0,
    },
    Band {
        type_id: NEGOTIATOR,
        min: -2.0,
        max: 0.0,
    },
];
/// Invented per-tag fees (attribute 1904), one per type.
const FEES: [(i64, f64); 4] = [
    (TRAINER, 1_000.0),
    (RECRUITER, 2_000.0),
    (TRANSPORTER, 3_000.0),
    (NEGOTIATOR, 4_000.0),
];

fn fee(type_id: i64) -> f64 {
    FEES.iter()
        .find(|(t, _)| *t == type_id)
        .map(|(_, f)| *f)
        .expect("a tag type")
}

fn content() -> std::sync::Arc<crate::content::Content> {
    let mut rows = format!(
        "{}attribute\t{ATTR_PROCESSING_FEE}\t0.0\tsecurityProcessingFee\n",
        crate::typedata::FIXTURE
    );
    for (type_id, fee) in FEES {
        rows.push_str(&format!(
            "type\t{type_id}\t99702\t17\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t{ATTR_PROCESSING_FEE}={fee}\tFixture Tag {type_id}\n"
        ));
    }
    let table = crate::typedata::Table::parse(&rows).expect("the invented rows parse");
    let mut content = crate::content::Content::fixture();
    content.insert("typedata", crate::typedata::Installed::build(table));
    std::sync::Arc::new(content)
}

/// Alice docked at a station of `owner` in `system`, with `balance` ISK.
fn world(owner: i64, system: i64, balance: f64) -> Registry {
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        balance,
        station_owner_id: owner,
        solar_system_id: Some(system),
        ..Character::default()
    };
    let bob = Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: BOB_SHIP,
        ..Character::default()
    };
    Registry::new(SandboxConfig {
        characters: vec![alice, bob],
        ..SandboxConfig::default()
    })
    .with_content(content())
}

fn station(reg: &Registry) -> i64 {
    reg.character(ALICE)
        .and_then(|c| c.station_id)
        .expect("a station")
}

/// `quantity` tags of `type_id` into `owner`'s hangar at `at`.
fn give(reg: &mut Registry, type_id: i64, quantity: i64, owner: i64, at: i64) {
    reg.spawn_items(&type_id.to_string(), quantity, owner, at)
        .expect("the invented tag spawns");
}

fn held(reg: &Registry, type_id: i64) -> i64 {
    tags_in_hangar(reg, ALICE, station(reg), &TEST_BANDS)
        .get(&type_id)
        .copied()
        .unwrap_or(0)
}

fn call(reg: &mut Registry, target: f64, current: f64) -> Result<PyValue, ServiceError> {
    apply_with(
        reg,
        &Request {
            service: "crimewatch",
            method: "ApplyTagsForSecStatus",
            args: &[PyValue::Real(target), PyValue::Real(current)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        },
        &TEST_BANDS,
    )
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

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// What a refused call must leave alone: the status, the wallet and the
/// tags.
fn snapshot(reg: &Registry) -> (f64, f64, Vec<i64>) {
    (
        security_status(reg, ALICE),
        reg.character(ALICE).map_or(0.0, |c| c.balance),
        FEES.iter().map(|(t, _)| held(reg, *t)).collect(),
    )
}

// ---------------------------------------------------------------------------
// The client's algorithm
// ---------------------------------------------------------------------------

#[test]
fn the_real_bands_are_the_clients_four_half_open_ranges() {
    assert_eq!(BANDS.len(), 4);
    let ranges: Vec<(f64, f64)> = BANDS.iter().map(|b| (b.min, b.max)).collect();
    assert_eq!(
        ranges,
        vec![(-10.0, -8.0), (-8.0, -5.0), (-5.0, -2.0), (-2.0, 0.0)]
    );
    let mut ids: Vec<i64> = BANDS.iter().map(|b| b.type_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "one type per band");
    assert_eq!(GAIN_PER_TAG, 0.5);
}

#[test]
fn a_climb_across_all_four_bands_takes_the_hand_computed_tags() {
    // From -9.3: Trainer ceil(1.3/0.5) = 3 (to -7.8); Recruiter
    // ceil(2.8/0.5) = 6 (to -4.8); Transporter ceil(2.8/0.5) = 6 (to -1.8);
    // Negotiator ceil(1.8/0.5) = 4 (to 0.2, past the request of 0.0).
    let plan = requirements(&BANDS, -9.3, 0.0).expect("a plan");
    assert_eq!(
        plan,
        vec![
            (BANDS[0].type_id, 3),
            (BANDS[1].type_id, 6),
            (BANDS[2].type_id, 6),
            (BANDS[3].type_id, 4),
        ]
    );
    assert_eq!(tag_count(&plan), 19);
}

#[test]
fn a_request_inside_a_band_stops_at_it() {
    // From -3.2 to -2.0: ceil(1.2/0.5) = 3 Transporters, nothing above.
    assert_eq!(
        requirements(&BANDS, -3.2, -2.0).expect("a plan"),
        vec![(BANDS[2].type_id, 3)]
    );
    // From -5.3 to -1.0: 1 Recruiter, then 6 Transporters, then 2
    // Negotiators (to -0.8).
    assert_eq!(
        requirements(&BANDS, -5.3, -1.0).expect("a plan"),
        vec![
            (BANDS[1].type_id, 1),
            (BANDS[2].type_id, 6),
            (BANDS[3].type_id, 2),
        ]
    );
}

#[test]
fn exact_halves_and_the_half_open_edges() {
    // Exactly two tags from the bottom of a band to two halves up.
    assert_eq!(
        requirements(&BANDS, -5.0, -4.0).expect("a plan"),
        vec![(BANDS[2].type_id, 2)]
    );
    // Starting at a band's lower edge is in that band, not the one below.
    assert_eq!(
        requirements(&BANDS, -8.0, -7.5).expect("a plan"),
        vec![(BANDS[1].type_id, 1)]
    );
    // One half short of the top edge is one tag, and it ends exactly there.
    assert_eq!(
        requirements(&BANDS, -2.5, -2.0).expect("a plan"),
        vec![(BANDS[2].type_id, 1)]
    );
    // A tag that overshoots the top of the last band is still one tag.
    assert_eq!(
        requirements(&BANDS, -0.3, 0.0).expect("a plan"),
        vec![(BANDS[3].type_id, 1)]
    );
}

#[test]
fn the_client_raises_where_the_plan_is_impossible() {
    // Outside (-10, 10].
    assert_eq!(
        requirements(&BANDS, -9.0, -10.0),
        Err(PlanError::OutOfRange)
    );
    assert_eq!(requirements(&BANDS, -9.0, 10.5), Err(PlanError::OutOfRange));
    assert_eq!(
        requirements(&BANDS, -9.0, -10.5),
        Err(PlanError::OutOfRange)
    );
    // Not above the start.
    assert_eq!(requirements(&BANDS, -3.0, -3.0), Err(PlanError::NotHigher));
    assert_eq!(requirements(&BANDS, -3.0, -4.0), Err(PlanError::NotHigher));
    // No tag lifts a status that is already 0 or more, nor past it.
    assert_eq!(requirements(&BANDS, 0.0, 1.0), Err(PlanError::NoTags));
    assert_eq!(requirements(&BANDS, -0.3, 1.0), Err(PlanError::NoTags));
}

#[test]
fn every_start_and_target_on_the_half_grid_is_a_plan_that_reaches_the_target() {
    let mut start = -9.5;
    while start < -0.4 {
        let mut target = start + 0.5;
        while target <= 0.0 + 1e-9 {
            let plan = requirements(&BANDS, start, target).expect("a plan");
            let gained = f64::from(tag_count(&plan)) * GAIN_PER_TAG;
            assert!(
                start + gained >= target - 1e-9,
                "{start} -> {target}: {plan:?}"
            );
            // And never more than one tag too many.
            assert!(
                start + gained < target + GAIN_PER_TAG + 1e-9,
                "{start} -> {target}: {plan:?}"
            );
            target += 0.5;
        }
        start += 0.5;
    }
}

// ---------------------------------------------------------------------------
// Who may use the office
// ---------------------------------------------------------------------------

#[test]
fn the_office_is_a_low_sec_service_of_concord_and_the_ded() {
    use crate::celestial::SecurityClass as C;
    for owner in OWNERS_WITH_SERVICE {
        assert!(office_open(owner, Some(C::Low)));
        assert!(!office_open(owner, Some(C::High)));
        assert!(!office_open(owner, Some(C::Safe)));
        assert!(!office_open(owner, Some(C::Zero)));
        assert!(!office_open(owner, None));
    }
    assert!(!office_open(1_999_601, Some(C::Low)), "another owner");
    assert_eq!(OWNERS_WITH_SERVICE.len(), 2);
}

// ---------------------------------------------------------------------------
// The call
// ---------------------------------------------------------------------------

#[test]
fn turning_in_tags_consumes_them_charges_the_fee_and_raises_the_status() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -3.2);
    give(&mut reg, TRANSPORTER, 5, ALICE, st);
    let _ = reg.take_pending();
    let _ = reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(600));

    assert_eq!(call(&mut reg, -2.0, -3.2).expect("accepted"), PyValue::None);

    // 3 tags, not 5: ceil(1.2 / 0.5).
    assert_eq!(held(&reg, TRANSPORTER), 2);
    assert!(near(security_status(&reg, ALICE), -2.0));
    let owed = 3.0 * fee(TRANSPORTER);
    let c = reg.character(ALICE).expect("alice");
    assert!(near(c.balance, 1.0e6 - owed));

    // One journal row: reference 117, to CONCORD, the fee as a debit.
    let row = reg
        .journal()
        .entries()
        .iter()
        .find(|e| e.entry_type_id == REF_PROCESSING_FEE)
        .expect("a ref-117 row");
    assert_eq!(row.owner_id_1, ALICE);
    assert_eq!(row.owner_id_2, sec_status::OWNER_CONCORD);
    assert!(near(row.amount, -owed));
    assert!(near(row.balance, 1.0e6 - owed));

    // The history row, event 385.
    let rows = sec_status::transactions(&reg, ALICE);
    assert_eq!(rows[0].event, sec_status::EVENT_HAND_IN_TAGS);
    assert!(near(rows[0].new_value, -2.0));
    assert_eq!(rows[0].location, st);
    assert!(near(
        rows[0].modification.expect("a change"),
        sec_status::modification(-3.2, -2.0).expect("a change")
    ));

    // The window refreshes from these three pushes.
    let now_pushes: Vec<&'static str> = reg
        .take_pending()
        .iter()
        .map(|(_, p)| p.to_push().0)
        .collect();
    assert!(now_pushes.contains(&"OnAccountChange"), "{now_pushes:?}");
    assert!(now_pushes.contains(&"OnItemChange"), "{now_pushes:?}");
    let on_clock: Vec<(Vec<i64>, f64)> = reg
        .take_due(std::time::Instant::now() + std::time::Duration::from_secs(600))
        .iter()
        .filter(|p| p.to_push().0 == "OnSecurityStatusUpdate")
        .map(|p| {
            let PyValue::Real(s) = p.to_push().1[0] else {
                panic!("a float")
            };
            (p.audience().unwrap_or_default(), s)
        })
        .collect();
    assert_eq!(on_clock, vec![(vec![ALICE], -2.0)]);
}

#[test]
fn a_long_climb_takes_tags_from_several_stacks_and_several_types() {
    let mut reg = world(OWNERS_WITH_SERVICE[1], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -5.3);
    give(&mut reg, RECRUITER, 1, ALICE, st);
    give(&mut reg, TRANSPORTER, 4, ALICE, st);
    give(&mut reg, TRANSPORTER, 4, ALICE, st);
    give(&mut reg, NEGOTIATOR, 3, ALICE, st);

    call(&mut reg, -1.0, -5.3).expect("accepted");

    // 1 Recruiter, 6 Transporters (of two stacks of four), 2 Negotiators.
    assert_eq!(held(&reg, RECRUITER), 0);
    assert_eq!(held(&reg, TRANSPORTER), 2);
    assert_eq!(held(&reg, NEGOTIATOR), 1);
    let owed = fee(RECRUITER) + 6.0 * fee(TRANSPORTER) + 2.0 * fee(NEGOTIATOR);
    assert!(near(
        reg.character(ALICE).expect("alice").balance,
        1.0e6 - owed
    ));
    // The tags overshot (to -0.8) but the pilot asked for -1.0.
    assert!(near(security_status(&reg, ALICE), -1.0));
}

#[test]
fn a_target_inside_the_last_tags_reach_is_the_target_exactly() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -0.3);
    give(&mut reg, NEGOTIATOR, 2, ALICE, st);
    call(&mut reg, 0.0, -0.3).expect("accepted");
    assert_eq!(held(&reg, NEGOTIATOR), 1, "one tag, to 0.0");
    assert_eq!(security_status(&reg, ALICE), 0.0);
}

#[test]
fn only_a_low_sec_concord_or_ded_station_has_the_office() {
    let cases = [
        ("high-sec", OWNERS_WITH_SERVICE[0], HIGH),
        ("zero-sec", OWNERS_WITH_SERVICE[0], NULL),
        ("an unlisted system", OWNERS_WITH_SERVICE[0], 1),
        ("another owner", 1_999_601, LOW),
    ];
    for (what, owner, system) in cases {
        let mut reg = world(owner, system, 1.0e6);
        let st = station(&reg);
        set_security_status(&mut reg, ALICE, -3.2);
        give(&mut reg, TRANSPORTER, 5, ALICE, st);
        let before = snapshot(&reg);
        let err = call(&mut reg, -2.0, -3.2).expect_err(what);
        assert_eq!(raised_key(&err), "CustomNotify", "{what}");
        assert_eq!(snapshot(&reg), before, "{what}: nothing is taken");
        assert!(sec_status::transactions(&reg, ALICE).is_empty(), "{what}");
    }
}

#[test]
fn a_pilot_in_space_cannot_use_it() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -3.2);
    give(&mut reg, TRANSPORTER, 5, ALICE, st);
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    let before = snapshot(&reg);
    let err = call(&mut reg, -2.0, -3.2).expect_err("refused");
    assert_eq!(raised_key(&err), "CustomNotify");
    assert_eq!(snapshot(&reg), before);
}

#[test]
fn a_status_that_fell_since_the_window_opened_needs_more_tags() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    give(&mut reg, TRANSPORTER, 9, ALICE, st);
    // The window planned from -3.2 (3 tags); the pilot is at -3.8 (4 tags).
    set_security_status(&mut reg, ALICE, -3.8);
    let before = snapshot(&reg);
    let err = call(&mut reg, -2.0, -3.2).expect_err("refused");
    assert_eq!(raised_key(&err), "MoreTagsNeedDueToLowerSec");
    assert_eq!(snapshot(&reg), before, "nothing was taken");

    // A status that rose since is fine: the plan is no longer than shown.
    set_security_status(&mut reg, ALICE, -3.2);
    call(&mut reg, -2.0, -3.8).expect("accepted");
    assert_eq!(held(&reg, TRANSPORTER), 6, "3 tags, the plan from -3.2");
    assert!(near(security_status(&reg, ALICE), -2.0));
}

#[test]
fn a_fall_that_changes_nothing_in_the_plan_is_not_an_error() {
    // -3.1 and -3.4 both need ceil(x / 0.5) = 3 tags to reach -2.0 ... no:
    // 1.1 / 0.5 = 2.2 -> 3, 1.4 / 0.5 = 2.8 -> 3.
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    give(&mut reg, TRANSPORTER, 3, ALICE, st);
    set_security_status(&mut reg, ALICE, -3.4);
    call(&mut reg, -2.0, -3.1).expect("the same three tags");
    assert_eq!(held(&reg, TRANSPORTER), 0);
}

#[test]
fn the_tags_come_only_from_this_stations_hangar_and_this_pilots() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -3.2);
    // Plenty, but in another station, and plenty in this one that are Bob's.
    give(&mut reg, TRANSPORTER, 9, ALICE, OTHER_STATION);
    give(&mut reg, TRANSPORTER, 9, BOB, st);
    // And not enough of her own here.
    give(&mut reg, TRANSPORTER, 2, ALICE, st);
    let before = snapshot(&reg);
    let err = call(&mut reg, -2.0, -3.2).expect_err("short");
    assert_eq!(raised_key(&err), "CustomNotify");
    assert_eq!(snapshot(&reg), before, "nothing is consumed on a refusal");
    let elsewhere = tags_in_hangar(&reg, ALICE, OTHER_STATION, &TEST_BANDS);
    assert_eq!(elsewhere.get(&TRANSPORTER), Some(&9), "left alone");
    let bobs = tags_in_hangar(&reg, BOB, st, &TEST_BANDS);
    assert_eq!(bobs.get(&TRANSPORTER), Some(&9));

    // The third tag arrives and it goes through, still sparing both piles.
    give(&mut reg, TRANSPORTER, 1, ALICE, st);
    call(&mut reg, -2.0, -3.2).expect("accepted");
    assert_eq!(held(&reg, TRANSPORTER), 0);
    assert_eq!(
        tags_in_hangar(&reg, ALICE, OTHER_STATION, &TEST_BANDS).get(&TRANSPORTER),
        Some(&9)
    );
    assert_eq!(
        tags_in_hangar(&reg, BOB, st, &TEST_BANDS).get(&TRANSPORTER),
        Some(&9)
    );
}

#[test]
fn a_pilot_who_cannot_pay_the_fee_keeps_his_tags() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 100.0);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -3.2);
    give(&mut reg, TRANSPORTER, 5, ALICE, st);
    let before = snapshot(&reg);
    let err = call(&mut reg, -2.0, -3.2).expect_err("poor");
    assert_eq!(raised_key(&err), "NotEnoughMoney");
    assert_eq!(snapshot(&reg), before);
    assert!(reg.journal().entries().is_empty(), "no row");

    // Exactly enough is enough.
    reg.character_mut(ALICE).expect("alice").balance = 3.0 * fee(TRANSPORTER);
    call(&mut reg, -2.0, -3.2).expect("accepted");
    assert!(near(reg.character(ALICE).expect("alice").balance, 0.0));
}

#[test]
fn asking_for_no_rise_does_nothing_and_an_impossible_rise_is_refused() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -3.2);
    give(&mut reg, TRANSPORTER, 5, ALICE, st);
    let before = snapshot(&reg);

    // At or below the current status: nothing to buy, nothing happens.
    assert_eq!(call(&mut reg, -3.2, -3.2).expect("a no-op"), PyValue::None);
    assert_eq!(call(&mut reg, -4.0, -3.2).expect("a no-op"), PyValue::None);
    assert_eq!(snapshot(&reg), before);
    assert!(sec_status::transactions(&reg, ALICE).is_empty());

    // Out of range, and past what any tag reaches.
    for target in [11.0, 0.5, 10.0] {
        let err = call(&mut reg, target, -3.2).expect_err("impossible");
        assert_eq!(raised_key(&err), "CustomNotify", "{target}");
    }
    assert_eq!(snapshot(&reg), before);
}

#[test]
fn malformed_arguments_are_refused_not_panicked_on() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    for args in [
        vec![],
        vec![PyValue::Real(-2.0)],
        vec![PyValue::None, PyValue::Real(-3.0)],
        vec![PyValue::Real(f64::NAN), PyValue::Real(-3.0)],
        vec![
            PyValue::Unicode("x".to_owned()),
            PyValue::Unicode("y".to_owned()),
        ],
    ] {
        let err = apply_with(
            &mut reg,
            &Request {
                service: "crimewatch",
                method: "ApplyTagsForSecStatus",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
            &TEST_BANDS,
        )
        .expect_err("refused");
        assert_eq!(raised_key(&err), "CustomNotify", "{args:?}");
    }
    // No actor: nothing to do.
    assert_eq!(
        apply_with(
            &mut reg,
            &Request {
                service: "crimewatch",
                method: "ApplyTagsForSecStatus",
                args: &[PyValue::Real(-2.0), PyValue::Real(-3.0)],
                kwargs: &[],
                bind_params: None,
                actor: None,
            },
            &TEST_BANDS,
        )
        .expect("a no-op"),
        PyValue::None
    );
}

#[test]
fn integers_are_accepted_for_the_two_statuses() {
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    let st = station(&reg);
    set_security_status(&mut reg, ALICE, -5.0);
    give(&mut reg, TRANSPORTER, 6, ALICE, st);
    apply_with(
        &mut reg,
        &Request {
            service: "crimewatch",
            method: "ApplyTagsForSecStatus",
            args: &[PyValue::Int(-2), PyValue::Int(-5)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        },
        &TEST_BANDS,
    )
    .expect("accepted");
    assert_eq!(security_status(&reg, ALICE), -2.0);
    assert_eq!(held(&reg, TRANSPORTER), 0);
}

#[test]
fn the_real_entry_point_uses_the_real_bands() {
    // No tags of the client's real types exist in this world, so the call is
    // refused for want of them -- which shows it planned with `BANDS`.
    let mut reg = world(OWNERS_WITH_SERVICE[0], LOW, 1.0e6);
    set_security_status(&mut reg, ALICE, -3.2);
    let st = station(&reg);
    give(&mut reg, TRANSPORTER, 5, ALICE, st);
    let err = apply(
        &mut reg,
        &Request {
            service: "crimewatch",
            method: "ApplyTagsForSecStatus",
            args: &[PyValue::Real(-2.0), PyValue::Real(-3.2)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        },
    )
    .expect_err("the invented tags are not the client's");
    assert_eq!(raised_key(&err), "CustomNotify");
}
