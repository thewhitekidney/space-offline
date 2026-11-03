//! SEC-1 over a real socket: a pilot with a low security status shows the
//! overview's Outlaw and Dangerous flags in his ship's slim, the flags
//! follow his status while he flies, and a trip to a low-sec CONCORD
//! station's security office turns tags into status.
//!
//! The calls are the client's own: `crimewatch.GetMySecurityStatus`,
//! `GetSecurityStatusTransactions` (the character sheet's security tab) and
//! `ApplyTagsForSecStatus` (the security office window's Submit); the
//! observer reads the slim out of the `SetState` / `AddBalls2` /
//! `OnSlimItemChange` frames of its own park. The same limitation as every
//! test here: both ends marshal with `eve-marshal`, so this proves
//! behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): the home system is an
//! invented low-sec one, the fees are invented, the status values are
//! picked to sit well inside their bands, and the tag types are read from
//! `security_office::BANDS` by name. The expected plan is `requirements`'s
//! by name too.

use super::common::{SYSTEM, config, gm, login, pilot, serve};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::celestial::Universe;
use eve_server::sec_status::{self, is_dangerous, is_outlaw};
use eve_server::security_office::{ATTR_PROCESSING_FEE, BANDS, OWNERS_WITH_SERVICE, requirements};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_901;
const BOB: i64 = 90_000_902;
const REGION: i64 = 19_090_001;
const CONSTELLATION: i64 = 29_090_001;
/// Bob's starting status: an outlaw, and one that needs three Recruiter tags
/// and one Transporter tag to reach [`RAISED`].
const OUTLAW: f64 = -6.5;
/// A status between the flags.
const ORANGE: f64 = -3.0;
const RAISED: f64 = -4.5;
/// Invented per-tag fees, one per band, lowest band first.
const FEES: [f64; 4] = [1_000.0, 2_000.0, 3_000.0, 4_000.0];

fn universe() -> Universe {
    Universe::parse(
        &[format!(
            "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t0.3\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture low"
        )],
        &[format!("{REGION}\t\t\t\t\tFixture region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture constellation"
        )],
    )
    .expect("the invented rows parse")
}

fn content() -> Arc<eve_server::content::Content> {
    let mut rows = format!(
        "{}attribute\t{ATTR_PROCESSING_FEE}\t0.0\tsecurityProcessingFee\n",
        eve_server::typedata::FIXTURE
    );
    for (band, fee) in BANDS.iter().zip(FEES) {
        rows.push_str(&format!(
            "type\t{}\t99702\t17\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t{ATTR_PROCESSING_FEE}={fee}\tFixture Tag {}\n",
            band.type_id, band.type_id
        ));
    }
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    let mut content = eve_server::content::Content::fixture();
    content.insert("universe", universe());
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> SocketAddr {
    let mut config = config();
    let at_concord = |id, name: &str, ship| {
        let mut c = pilot(id, name, ship);
        c.station_owner_id = OWNERS_WITH_SERVICE[0];
        c.balance = 1.0e7;
        c
    };
    config.sandbox.characters = vec![
        at_concord(ALICE, "Alice", 100_000_901),
        at_concord(BOB, "Bob", 100_000_902),
    ];
    config.content = Some(content());
    serve(config)
}

/// Read `c`'s pushes until `done` says so or `within` runs out.
fn watch(c: &mut Conn, within: Duration, mut done: impl FnMut(&Conn) -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done(c) {
            return true;
        }
        let _ = c.pump(Duration::from_millis(250));
    }
    done(c)
}

/// What Bob's ball says about his status in `observer`'s park, once it is
/// there.
fn bobs_slim_status(observer: &Conn) -> Option<f64> {
    observer
        .world()
        .balls()
        .find(|b| b.slim.is_some_and(|s| s.char_id == Some(BOB)))
        .and_then(|b| b.slim.and_then(|s| s.security_status))
}

fn my_status(c: &mut Conn) -> f64 {
    match c
        .call("crimewatch", "GetMySecurityStatus", vec![])
        .expect("answered")
        .value()
    {
        Some(PyValue::Real(r)) => r,
        other => panic!("a float status: {other:?}"),
    }
}

/// The character sheet's security tab: the `eventTypeID` of every row,
/// newest first.
fn history(c: &mut Conn) -> Vec<i64> {
    let Some(PyValue::List(rows)) = c
        .call("crimewatch", "GetSecurityStatusTransactions", vec![])
        .expect("answered")
        .value()
    else {
        panic!("a list of rows")
    };
    rows.iter()
        .map(|row| {
            let PyValue::Instance { state, .. } = row else {
                panic!("a KeyVal row: {row:?}")
            };
            match eve_server::pyutil::dict_get(state, b"eventTypeID") {
                Some(PyValue::Int(n)) => *n,
                other => panic!("an event id: {other:?}"),
            }
        })
        .collect()
}

fn count_in_hangar(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

#[test]
fn a_low_status_shows_in_the_slim_follows_it_in_flight_and_tags_raise_it() {
    let addr = start();
    let plan = requirements(&BANDS, OUTLAW, RAISED).expect("a plan");
    // Three Recruiters and a Transporter, by the client's own algorithm.
    assert_eq!(plan, vec![(BANDS[1].type_id, 3), (BANDS[2].type_id, 1)]);

    // Bob's setup, docked: a GM sets the status, grants ISK and the tags.
    let mut bob = login(addr, "Bob");
    gm(&mut bob, &format!("/secstatus {OUTLAW}"));
    for (tag, n) in &plan {
        gm(&mut bob, &format!("/createitem {tag} {n}"));
    }
    assert_eq!(my_status(&mut bob), OUTLAW);
    assert_eq!(
        history(&mut bob),
        vec![sec_status::EVENT_GM_MODIFICATION],
        "the security tab lists the change"
    );
    for (tag, n) in &plan {
        assert_eq!(count_in_hangar(&mut bob, *tag), i64::from(*n));
    }
    let station = bob.session().station_id().expect("docked");

    // Both undock. Alice's park has Bob's ball, and its slim says Outlaw.
    let mut alice = login(addr, "Alice");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");
    assert!(
        watch(&mut alice, Duration::from_secs(30), |c| bobs_slim_status(c)
            .is_some()),
        "Bob's slim reaches Alice with a securityStatus"
    );
    let seen = bobs_slim_status(&alice).expect("a status");
    assert_eq!(seen, OUTLAW);
    assert!(is_outlaw(seen), "the red skull");

    // In flight: orange. The slim is refreshed by OnSlimItemChange, and the
    // overview's Dangerous flag is what it reads.
    gm(&mut bob, &format!("/secstatus {ORANGE}"));
    assert!(
        watch(&mut alice, Duration::from_secs(30), |c| bobs_slim_status(c)
            == Some(ORANGE)),
        "the slim follows the status across the -5.0 line"
    );
    assert!(is_dangerous(ORANGE) && !is_outlaw(ORANGE));
    // And back to an outlaw, for the office.
    gm(&mut bob, &format!("/secstatus {OUTLAW}"));
    assert!(
        watch(&mut alice, Duration::from_secs(30), |c| bobs_slim_status(c)
            == Some(OUTLAW)),
        "and back"
    );

    // Docked at a low-sec CONCORD station: the security office's one call.
    bob.dock(station).expect("Bob docks");
    let wealth_before = bob.cash_balance().expect("the wallet");
    assert_eq!(count_in_hangar(&mut bob, BANDS[1].type_id), 3);
    let done = bob
        .call(
            "crimewatch",
            "ApplyTagsForSecStatus",
            vec![PyValue::Real(RAISED), PyValue::Real(OUTLAW)],
        )
        .expect("answered");
    assert_eq!(done.refusal(), None, "the office accepts the tags");
    assert_eq!(my_status(&mut bob), RAISED);
    for (tag, _) in &plan {
        assert_eq!(
            count_in_hangar(&mut bob, *tag),
            0,
            "the tags were turned in"
        );
    }
    assert_eq!(
        history(&mut bob)[..2],
        [
            sec_status::EVENT_HAND_IN_TAGS,
            sec_status::EVENT_GM_MODIFICATION
        ],
        "the hand-in is the newest row"
    );
    // The fee left the wallet: one tag-processing fee per tag, in the fees
    // the type table gives.
    let owed: f64 = plan
        .iter()
        .map(|(tag, n)| {
            let at = BANDS
                .iter()
                .position(|b| b.type_id == *tag)
                .expect("a band");
            FEES[at] * f64::from(*n)
        })
        .sum();
    let wealth_after = bob.cash_balance().expect("the wallet");
    assert!(
        (wealth_before - wealth_after - owed).abs() < 1e-6,
        "{wealth_before} -> {wealth_after}, owed {owed}"
    );

    // Undocked again, the new ball carries the new status.
    bob.undock().expect("Bob undocks again");
    bob.enter_space(SYSTEM).expect("Bob's state");
    assert!(
        watch(&mut alice, Duration::from_secs(30), |c| bobs_slim_status(c)
            == Some(RAISED)),
        "Alice sees the raised status on Bob's new ball"
    );
    assert!(!is_outlaw(RAISED) && is_dangerous(RAISED));

    // A second hand-in with nothing in the hangar is refused and changes
    // nothing: no tags, no deal.
    bob.dock(station).expect("Bob docks again");
    let refused = bob
        .call(
            "crimewatch",
            "ApplyTagsForSecStatus",
            vec![PyValue::Real(-3.0), PyValue::Real(RAISED)],
        )
        .expect("answered");
    assert!(refused.refusal().is_some(), "no tags, no deal");
    assert_eq!(my_status(&mut bob), RAISED);
}
