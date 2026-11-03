//! Tests for [`Registry`] itself: the world object, its clock, its durability
//! and the slash console.
//!
//! **Why these are here and not distributed into `service/*.rs`.** They were
//! `service.rs`'s own `mod tests`, 4,274 lines of a 8,942-line file, and
//! `docs/archive/plan-subsystems.md` P0.1 planned to split them across the sixteen
//! service modules the way every other test in this crate lives beside the
//! code it exercises.
//!
//! Most of them cannot go there, and the reason is worth recording: they call
//! `Registry` methods directly -- `r.owned_item_rows(Some(ALICE))`,
//! `r.hangar_item_rows(..)`, `advance_character` -- rather than dispatching a
//! service. They are tests of the world, not of a service surface, so filing
//! them under `service/inventory.rs` would put them next to code they do not
//! exercise.
//!
//! So the test module follows the god object, and `Registry`'s breadth is a
//! deliberate decision rather than an accident (§4a of that plan: one lock,
//! acquired once per payload, is what closes the session-ordering hazard by
//! construction). Moving the file out of `service.rs` is the part of P0.1
//! that was real: it halves the hottest file in the crate and stops a lane
//! editing dispatch logic from colliding with a lane adding a test.
//!
//! **Where a new test belongs.** If it dispatches a service, put it in that
//! service's own module. Only a test of `Registry`, the world clock, the
//! snapshot or the slash console belongs here.

use super::*;

fn registry() -> Registry {
    Registry::new(SandboxConfig::default())
}

/// **A panic anywhere under the world lock must not silently cut every
/// later player off from everyone else.**
///
/// `RwLock`'s poison flag never clears. `server::serve_one` used to
/// `match world.write()` and skip its session-table work on `Err`, while
/// `dispatch` and the world ticker recovered -- so one panic made the
/// server permanently unable to register or deregister a connection,
/// while answering that connection's own calls perfectly. Since
/// `of_character` is how any notification reaches somebody who is not the
/// caller, the symptom is a player alone in a populated system.
///
/// The test poisons the lock the only way it can be poisoned -- a panic
/// while holding it -- and then asserts the world is still usable.
/// Asserting `is_poisoned()` as well is deliberate: it pins that we are
/// recovering from a *genuinely* poisoned lock rather than passing
/// because the panic never landed.
#[test]
fn the_world_survives_a_panic_under_its_own_lock() {
    let world = std::sync::Arc::new(std::sync::RwLock::new(registry()));
    let poisoner = std::sync::Arc::clone(&world);
    let panicked = std::thread::spawn(move || {
        let _guard = poisoner.write().expect("not poisoned yet");
        panic!("a dispatch arm hit an unwrap");
    })
    .join();
    assert!(panicked.is_err(), "the thread must actually have panicked");
    assert!(
        world.is_poisoned(),
        "and the lock must actually be poisoned, or this proves nothing"
    );

    // The two things `serve_one` does around a connection's life. Before
    // the fix these were skipped for the rest of the process.
    let mut w = world_write(&world);
    w.session_opened(crate::session_table::SessionEntry {
        client_id: 42,
        session_id: 7,
        user_id: 1,
        character_id: None,
    });
    assert!(
        w.sessions().entries().iter().any(|e| e.client_id == 42),
        "a connection opened after a poisoning must still be addressable"
    );
    w.session_closed(42);
    assert!(
        !w.sessions().entries().iter().any(|e| e.client_id == 42),
        "and must still be removable, or the lobby keeps a ghost"
    );
}

/// **A scheduled Destiny batch is stamped when it leaves, not when it is
/// built — which is what made the clock possible at all.**
///
/// `docs/port-tracker.md` §30.4 recorded this as blocked on "a stamp
/// allocator that can mint a future stamp without moving the floor",
/// because a batch built now and sent in sixty seconds would be stamped
/// sixty seconds in the past, and a Destiny stamp must be monotonic
/// non-decreasing for ever: a backwards one is either dropped or sends
/// `SynchroniseToSimulationTime` rewinding. Minting the *future* stamp
/// instead records that future value as the floor, which then drags every
/// push in between up to it.
///
/// Both horns come from one assumption — that the stamp is chosen when
/// the batch is built. It is not. [`Registry::take_due`] binds it at
/// release with the same [`Registry::next_destiny_stamp`] an immediate
/// push uses, so no allocator is needed and the floor moves exactly as it
/// always did.
///
/// The assertion is the ordering property rather than a number: a push
/// that goes out *between* the schedule and its due time must not be
/// stamped after the scheduled one.
#[test]
fn a_scheduled_push_is_stamped_when_it_leaves_not_when_it_is_built() {
    let mut reg = registry();
    let built = crate::destiny::ActionUpdate::new(
        vec![1_000_001],
        // The caller's placeholder, which must never reach the wire.
        0,
        vec![crate::destiny::Action::Stop { ball: 100_000_001 }],
    )
    .expect("a batch");
    reg.schedule(Duration::ZERO, built);
    assert_eq!(reg.scheduled_count(), 1);
    assert!(
        reg.take_pending().is_empty(),
        "the two queues are separate: a scheduled push must not appear in \
         the payload turn's drain, because `on_ballpark_bind` depends on \
         the order of that one"
    );

    // Something the server sends in the meantime.
    let meanwhile = reg.next_destiny_stamp();

    let due = reg.take_due(Instant::now());
    assert_eq!(due.len(), 1, "it was due");
    assert_eq!(reg.scheduled_count(), 0, "and the queue drained");
    let stamp = destiny_stamp(due[0].as_ref());
    assert_ne!(stamp, 0, "the placeholder must have been replaced");
    assert!(
        stamp >= meanwhile,
        "a batch released after an immediate push must not be stamped \
         before it: scheduled batch {stamp}, meanwhile {meanwhile}"
    );
}

/// **An item change is addressed to the item's owner, not to the caller.**
///
/// `apply_items` left its pushes unaddressed, which the dispatcher reads as
/// "the acting session" -- so a market fill made in Alice's call would have
/// sent Bob's hangar rows to Alice, and a death on the world tick would have
/// sent them to whoever spoke next (found by the P2.2c lane).
#[test]
fn an_item_change_goes_to_the_items_owner() {
    let mut r = two_characters();
    let content = crate::content::Content::fixture();
    let (ids, _) = r
        .store
        .spawn_in(&content, "tritanium", 10, BOB, 60_000_001)
        .expect("spawn");
    let _ = r.take_pending();
    r.apply_items(&[crate::item::Edit::Consume {
        item_id: ids[0],
        quantity: 3,
    }])
    .expect("consume");
    let pushed: Vec<(&'static str, Option<Vec<i64>>)> = r
        .take_pending()
        .into_iter()
        .map(|(_, n)| (n.to_push().0, n.audience()))
        .collect();
    assert!(!pushed.is_empty());
    for (event, audience) in pushed {
        assert_eq!(audience, Some(vec![BOB]), "{event} must go to Bob");
    }
}

/// **A pilot who closes the client leaves the fleet when the reconnect
/// window ends, and the rest are told by the clock.**
///
/// `Fleets::character_disconnected` had four unit tests and no caller
/// (`docs/port-tracker.md` §60.3), so every test passed while a quitter stayed
/// a member for ever. Asserted through `session_closed`, for the reason the
/// test below gives: what is under test is that the disconnect path reaches
/// it at all, and which queue it reaches for. A second session of the same
/// character keeps them in -- the logoff stamp's rule.
#[test]
fn a_quitter_leaves_the_fleet_and_the_rest_hear_it_by_the_clock() {
    let mut r = two_characters();
    for (client_id, who) in [
        (10_000_000_001, ALICE),
        (10_000_000_002, ALICE),
        (20_000_000_001, BOB),
    ] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
    }
    let fleets = r.fleets_mut();
    let id = fleets.create(ALICE).expect("create");
    fleets.init(ALICE, id, Some(670), None, None).expect("init");
    fleets
        .invite(ALICE, id, BOB, None, None, None)
        .expect("invite");
    fleets
        .accept_invite(BOB, id, Some(670), None, None)
        .expect("accept");
    let _ = r.take_pending();
    let _ = r.take_due(Instant::now());

    r.session_closed(10_000_000_001);
    assert!(
        r.fleets().fleet_of(ALICE).is_some(),
        "Alice still has a client open, so she is still in the fleet"
    );
    assert!(r.take_due(Instant::now()).is_empty());

    r.session_closed(10_000_000_002);
    assert!(
        r.fleets().fleet_of(ALICE).is_some(),
        "the seat is held for the reconnect window (F1)"
    );
    assert!(
        r.take_due(Instant::now())
            .iter()
            .all(|n| n.to_push().0 != "OnFleetLeave"),
        "and nobody is told yet"
    );
    crate::fleet::tick(
        &mut r,
        Instant::now(),
        crate::pyutil::filetime_now() + crate::fleet::RECONNECT_TIMEOUT,
    );
    assert!(r.fleets().fleet_of(ALICE).is_none());
    assert!(r.fleets().fleet_of(BOB).is_some(), "Bob's fleet goes on");
    let due: Vec<(String, Vec<i64>)> = r
        .take_due(Instant::now())
        .iter()
        .map(|n| (n.to_push().0.to_string(), n.audience().unwrap_or_default()))
        .collect();
    assert!(
        due.contains(&("OnFleetLeave".to_string(), vec![BOB])),
        "Bob is the one left to tell, and nothing he calls has to happen \
         first; due={due:?}"
    );
    assert!(
        r.take_pending().is_empty(),
        "a disconnect has no turn to ride"
    );
}

/// **A quitter's ball is removed by the clock, not by somebody's turn.**
///
/// The sibling of `a_departure_is_announced_by_the_clock_not_by_a_turn`,
/// and the thing `docs/lanes/two-in-space.md` §1.3 called "the one
/// architectural ask in this lane": a stale guest row costs a wrong name
/// in a panel, but a stale *ball* is a ship hanging in space that the
/// observer can select, approach and shoot -- and the observer is, by
/// construction, the player who is doing nothing, so "it goes out on
/// their next call" could be never.
///
/// Written because the claim had no test and the comment above the code
/// still said the frame was best-effort. Asserted through
/// `session_closed` rather than by building the push directly: what is
/// under test is which queue the disconnect path reaches for.
///
/// **Since the log-off hold, the removal is not at the disconnect** but when
/// the hold ends (`crate::crimewatch::logged_off`): the ship stays a ball in
/// Bob's park until then, and the world clock removes it -- so the clock is
/// run to the hold's own end, whatever that is, and the question is the
/// same one.
#[test]
fn a_quitters_ball_is_removed_by_the_clock_not_by_a_turn() {
    let mut r = two_characters();
    for (client_id, who) in [(10_000_000_001, ALICE), (20_000_000_001, BOB)] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
        // Into space, which is what puts them in each other's park.
        crate::destiny::on_ballpark_bind(&mut r, Some(who));
    }
    assert!(
        r.roster().get(ALICE).is_some() && r.roster().get(BOB).is_some(),
        "both have to be in space or there is no ball to remove"
    );
    // Undocking happens inside a turn and is entitled to `pending`.
    let _ = r.take_pending();
    let _ = r.take_due(Instant::now());

    // Alice quits. Bob is sitting in space calling nothing, which is the
    // whole point.
    r.session_closed(10_000_000_001);
    assert!(
        r.roster().get(ALICE).is_some(),
        "the ship stays in space while it is held"
    );
    let until = r
        .combat()
        .crimewatch()
        .held_until(ALICE)
        .expect("held after a disconnect in space");
    let _ = r.take_due(Instant::now());
    crate::crimewatch::tick(&mut r, until);
    assert!(r.roster().get(ALICE).is_none(), "and leaves when it ends");

    // **The action name is `PyValue::Bytes`, so it has to be matched as
    // bytes.** `format!("{:?}", args).contains("RemoveBalls")` is the
    // shape this started as and it can never match: `Bytes` debug-prints
    // as a list of numbers, so the filter silently selected nothing and
    // the test failed with `due=[] pending=[]` against a server that was
    // doing the right thing.
    fn mentions(v: &PyValue, want: &[u8]) -> bool {
        match v {
            PyValue::Bytes(b) => b.as_slice() == want,
            PyValue::List(items) | PyValue::Tuple(items) => items.iter().any(|i| mentions(i, want)),
            _ => false,
        }
    }
    let removals = |batch: Vec<Box<dyn crate::notify::Push>>| -> Vec<Vec<i64>> {
        batch
            .iter()
            .filter(|n| {
                let (event, args) = n.to_push();
                event == "DoDestinyUpdate" && args.iter().any(|a| mentions(a, b"RemoveBalls"))
            })
            .map(|n| n.audience().unwrap_or_default())
            .collect()
    };
    let due = removals(r.take_due(Instant::now()));
    let pending: Vec<Vec<i64>> = removals(
        r.take_pending()
            .into_iter()
            .map(|(_, n)| n)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        due,
        vec![vec![BOB]],
        "the removal has to be on the clock queue, addressed to the \
         observer who is still in space; due={due:?} pending={pending:?}"
    );
    assert!(
        pending.is_empty(),
        "and not on the turn queue, which a disconnect never drains: \
         pending={pending:?}"
    );
}

/// The clock leaves alone what is not due yet.
///
/// Trivial, and worth a test because the failure is not: an arrival batch
/// released early sends `SetBallPosition` and `Stop` to a ship still in
/// warp, which aborts the warp wherever the ship happens to be. That is
/// the one failure direction `destiny::warp_flight_seconds` is biased
/// against, and it would be undone by a queue that drained eagerly.
#[test]
fn the_clock_does_not_release_a_batch_before_its_time() {
    let mut reg = registry();
    let later = crate::destiny::ActionUpdate::new(
        vec![1_000_001],
        0,
        vec![crate::destiny::Action::Stop { ball: 100_000_001 }],
    )
    .expect("a batch");
    reg.schedule(Duration::from_secs(600), later);
    let now = Instant::now();
    assert!(reg.take_due(now).is_empty(), "ten minutes out");
    assert_eq!(reg.scheduled_count(), 1, "and it is still there");
    assert!(
        reg.take_due(now + Duration::from_secs(599)).is_empty(),
        "still not due one second short"
    );
    assert_eq!(reg.take_due(now + Duration::from_secs(601)).len(), 1);
}

/// The stamp of a `DoDestinyUpdate`'s first entry.
fn destiny_stamp(push: &dyn crate::notify::Push) -> u32 {
    let (name, args) = push.to_push();
    assert_eq!(name, "DoDestinyUpdate", "not a Destiny batch");
    let PyValue::List(entries) = &args[0] else {
        panic!("the batch is a sequence: {args:?}")
    };
    let PyValue::Tuple(first) = &entries[0] else {
        panic!("an entry is a tuple")
    };
    match first[0] {
        PyValue::Int(s) => u32::try_from(s).expect("a stamp is a u32"),
        ref other => panic!("a stamp is an Int: {other:?}"),
    }
}

/// **`/tp` puts the ship far enough out that the station is a legal warp
/// target.**
///
/// The property is the *distance*, because that is the whole purpose:
/// `appConst.minWarpDistance` is 150 km and the client offers no "Warp
/// To" entry below it, so a teleport that lands short makes warp
/// untestable while looking like it worked.
///
/// Also asserted: everyone in the system is told, because a ship that
/// teleports on one screen and not the others is a desync we caused
/// deliberately.
#[test]
fn tp_puts_the_ship_past_the_clients_minimum_warp_distance() {
    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;
    const MIN_WARP: f64 = 150_000.0;
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: 100_000_001,
        ..Character::default()
    };
    let bob = Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: 100_000_002,
        ..Character::default()
    };
    let mut reg = Registry::new(SandboxConfig {
        characters: vec![alice, bob],
        ..SandboxConfig::default()
    });
    crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
    crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
    let _ = reg.take_pending();

    let answer = reg.slash(Some(ALICE), "/tp 200").expect("answered");
    assert!(
        said(&answer).contains("200"),
        "the answer says where you went: {}",
        said(&answer)
    );
    let from_station = crate::space_state::distance(
        [0.0; 3],
        reg.roster().get(ALICE).expect("in space").motion.position,
    );
    assert!(
        from_station > MIN_WARP,
        "{from_station} m is inside the client's 150 km warp minimum, so \
         the station still offers no Warp To entry"
    );

    // Bob is told, and the batch names Alice's ball.
    let addressed: Vec<Vec<i64>> = reg
        .take_pending()
        .iter()
        .map(|(_, p)| p.audience().unwrap_or_default())
        .collect();
    assert!(
        addressed.iter().any(|a| a.contains(&BOB)),
        "Bob never saw Alice move: {addressed:?}"
    );
}

/// `/tp` refuses what it cannot do, rather than doing something else.
#[test]
fn tp_refuses_a_docked_pilot_and_a_nonsense_distance() {
    let mut reg = registry();
    let actor = actor(&reg);
    // Docked: nobody is in the roster yet.
    let docked = reg.slash(actor, "/tp 200").expect("answered");
    assert!(said(&docked).contains("not in space"), "{}", said(&docked));
    assert!(said(&reg.slash(actor, "/tp banana").expect("answered")).contains("not a number"));
    assert!(said(&reg.slash(actor, "/tp -5").expect("answered")).contains("pick between"));
    // **And a distance inside the station model**, which `/tp 0` was
    // until self-review: the origin is the middle of a 15.7 km station,
    // which is the state `destiny::berth` exists to avoid.
    for inside in ["/tp 0", "/tp 5", "/tp 15"] {
        assert!(
            said(&reg.slash(actor, inside).expect("answered")).contains("pick between"),
            "{inside} put the ship inside the station"
        );
    }
    assert!(reg.take_pending().is_empty(), "and pushed nothing");
}

/// A slash command's answer as the player reads it.
///
/// `bstr` is `PyValue::Bytes`, which `Debug` prints as a list of numbers,
/// so an assertion on the text has to decode it first -- the version that
/// did not failed with a hundred-element byte array as its message, which
/// is the opposite of a useful test failure.
fn said(v: &PyValue) -> String {
    String::from_utf8_lossy(crate::pyutil::as_bytes(v).unwrap_or_default()).into_owned()
}

/// **No ship this server can fly is served a zero in any of the three    /// **No ship this server can fly is served a zero in any of the three
/// targeting attributes.**
///
/// The property, and the reason it is a property rather than a table
/// check: `GetAttribute` falls back to the attribute *definition's*
/// `defaultValue`, and all three default to `0`, so a missing or zero
/// value never raises -- it silently takes targeting away, with a zero
/// 76 hiding the right-click "Lock target" entry and no error anywhere
/// to explain it. A test that asserted specific numbers would pass while
/// a later edit zeroed a row.
#[test]
fn no_flyable_hull_is_served_a_zero_targeting_attribute() {
    let content = crate::content::Content::fixture();
    let mut checked = 0;
    for h in crate::destiny::hull::FIXTURE {
        let served = ship_attributes(&content, h.type_id);
        let PyValue::Dict(entries) = served else {
            panic!("attributes are a dict")
        };
        for id in [76, 192, 564] {
            let (value, _) = entries
                .iter()
                .find(|(_, k)| *k == PyValue::Int(id))
                .unwrap_or_else(|| panic!("type {} has no attribute {id}", h.type_id));
            let PyValue::Real(v) = value else {
                panic!("attribute {id} must be a Real: the client floor-divides ints")
            };
            assert!(
                *v > 0.0,
                "type {} is served {id} = {v}, which disables targeting silently",
                h.type_id
            );
            checked += 1;
        }
    }
    assert_eq!(checked, crate::destiny::hull::FIXTURE.len() * 3);
}

/// The capsule keeps CCP's real zeros in the table and is *served*
/// something it can fly with.
///
/// Both halves matter. The table is our record of the shipped data and a
/// pod really cannot lock in EVE `[V]`; what we serve is a sandbox
/// decision, taken because every character here starts in a pod and
/// nothing answers a ship change, so the honest zeros would ship
/// targeting no human could reach. Keeping them in different places is
/// what lets the divergence be deleted the day ship changing works.
#[test]
fn the_capsule_keeps_its_real_zeros_and_is_served_a_floor() {
    const CAPSULE: i64 = 670;
    let content = crate::content::Content::fixture();
    let table =
        crate::destiny::hull::find_in(&content, CAPSULE).expect("the capsule is in the table");
    assert_eq!(
        (table.max_target_range, table.max_locked_targets),
        (0.0, 0.0),
        "the capsule's typedogma is zeros (the invented fixture keeps them)"
    );

    let PyValue::Dict(entries) = ship_attributes(&content, CAPSULE) else {
        panic!("attributes are a dict")
    };
    let served = |id: i64| match entries.iter().find(|(_, k)| *k == PyValue::Int(id)) {
        Some((PyValue::Real(v), _)) => *v,
        other => panic!("attribute {id} is {other:?}"),
    };
    assert!(served(76) > 0.0 && served(192) >= 1.0 && served(564) > 0.0);
}

/// The one character in a single-character fixture, as an actor.    /// The one character in a single-character fixture, as an actor.
///
/// Taken from the fixture rather than from inside the registry: finding
/// the actor by reading "the first character" is the thing Round 0.3
/// removed, and a test that did it would hide its return.
fn actor(reg: &Registry) -> Option<i64> {
    Some(reg.config().characters[0].character_id)
}

/// The same value as a constant, for the `Request` literals that are
/// built where no `&Registry` is in hand. Tied to the fixture by
/// [`the_fixture_character_is_the_actor_these_tests_pass`] so the two
/// cannot drift apart silently.
const ACTOR: Option<i64> = Some(90_000_001);

#[test]
fn the_fixture_character_is_the_actor_these_tests_pass() {
    assert_eq!(ACTOR, actor(&registry()));
}

// --- two characters ---------------------------------------------------
//
// Everything below exists because a one-character world cannot tell a
// correct answer from `characters.first()`. These are the tests that
// would have caught the whole class of bug Round 0.3 removed, and the
// only thing that makes them possible is a second row in the roster.

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;

/// Two characters, distinct in **every id that is a key somewhere**.
///
/// `ship_id` is not cosmetic: `Registry::new` inserts a capsule at that
/// id and `Store::insert` is a `push`, so two characters sharing one
/// would put two items with one id in the store. `Registry::new` now
/// asserts it; this fixture is what the assert is for.
fn two_characters() -> Registry {
    let alice = Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: 100_000_001,
        ..Character::default()
    };
    let bob = Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: 100_000_002,
        // Same station on purpose: the station is what makes them
        // visible to each other.
        ..Character::default()
    };
    Registry::new(SandboxConfig {
        characters: vec![alice, bob],
        ..SandboxConfig::default()
    })
}

/// **The single most valuable test in this round.**
///
/// `owned_item_rows` read `characters.first()`, so it answered Alice's
/// items for every caller. With two players that is not a missing
/// feature, it is Bob's inventory window showing Alice's ship.
#[test]
fn owned_item_rows_are_the_actors_not_the_firsts() {
    let r = two_characters();
    let ids = |rows: Vec<PyValue>| -> Vec<i64> {
        rows.iter()
            .filter_map(|row| match row {
                PyValue::DbRow(b) => match b.values().first() {
                    Some(DbValue::Int(id)) => Some(*id),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    };
    let alice = ids(r.owned_item_rows(Some(ALICE)));
    let bob = ids(r.owned_item_rows(Some(BOB)));
    assert!(alice.contains(&100_000_001), "Alice owns her own capsule");
    assert!(bob.contains(&100_000_002), "Bob owns his own capsule");
    assert!(
        !bob.contains(&100_000_001),
        "Bob must not be shown Alice's capsule: {bob:?}"
    );
    assert!(!alice.contains(&100_000_002));
}

#[test]
fn hangar_item_rows_are_per_character() {
    let r = two_characters();
    let n = |actor: i64| r.hangar_item_rows(Some(actor)).len();
    assert_eq!(n(ALICE), 1, "one capsule each, not two");
    assert_eq!(n(BOB), 1);
}

/// `/giveisk` credited `characters.first_mut()`, so a grant aimed at Bob
/// landed on Alice and Bob's wallet never moved.
#[test]
fn the_balance_and_the_grant_are_the_actors() {
    let mut r = two_characters();
    let balance = |r: &mut Registry, actor: i64| match r.dispatch(&Request {
        service: "account",
        method: "GetCashBalance",
        args: &[PyValue::Int(0)],
        kwargs: &[],
        bind_params: None,
        actor: Some(actor),
    }) {
        Ok(PyValue::Real(v)) => v,
        other => panic!("GetCashBalance answered {other:?}"),
    };
    r.slash(Some(BOB), "/giveisk 1000000").expect("answers");
    assert_eq!(balance(&mut r, BOB), 1_000_000.0);
    assert_eq!(
        balance(&mut r, ALICE),
        0.0,
        "a grant to Bob must not appear in Alice's wallet"
    );
}

/// The wallet journal was one table with no owner filter, so
/// `GetTransactions` handed each player everyone's history.
#[test]
fn the_journal_rows_are_the_actors() {
    let mut r = two_characters();
    r.slash(Some(BOB), "/giveisk 1000000").expect("answers");
    let now = crate::pyutil::filetime_now();
    assert_eq!(r.journal().select(now, None, None, BOB).len(), 1);
    assert_eq!(
        r.journal().select(now, None, None, ALICE).len(),
        0,
        "Alice must not read Bob's wallet history"
    );
}

/// The bio was one `String` for the whole server: whoever saved last
/// owned it, and the other player's character sheet showed it as theirs.
#[test]
fn the_bio_is_per_character() {
    let mut r = two_characters();
    let set = |r: &mut Registry, actor: i64, text: &str| {
        r.dispatch(&Request {
            service: "charMgr",
            method: "SetCharacterDescription",
            args: &[PyValue::Unicode(text.to_owned())],
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        })
        .expect("must not raise");
    };
    set(&mut r, ALICE, "I fly Rifters");
    set(&mut r, BOB, "I fly nothing");
    assert_eq!(
        r.character_description(ALICE).as_deref(),
        Some("I fly Rifters")
    );
    assert_eq!(
        r.character_description(BOB).as_deref(),
        Some("I fly nothing")
    );
}

/// **The hazard `sessionIdentity.js` names, as a test.**
///
/// An id that is not a character must answer nothing -- never Alice.
/// This is the test that fails if anyone reintroduces a default, and the
/// reason `Request::actor` is an `Option` rather than an `i64`.
#[test]
fn an_unknown_actor_gets_nothing_and_never_the_first_character() {
    let mut r = two_characters();
    assert!(r.character(999).is_none());
    assert!(r.undock_change(Some(999)).is_none());
    assert_eq!(r.skill_dict(Some(999)), PyValue::Dict(vec![]));
    assert!(r.owned_item_rows(Some(999)).is_empty());
    assert!(r.hangar_item_rows(Some(999)).is_empty());
    assert!(r.character_description(999).is_none());

    // And the same for no actor at all, which is every call made before
    // `SelectCharacterID`.
    assert!(r.undock_change(None).is_none());
    assert_eq!(r.skill_dict(None), PyValue::Dict(vec![]));
    assert!(r.owned_item_rows(None).is_empty());
    assert!(r.hangar_item_rows(None).is_empty());
}

/// **`GetPublicInfo` answers about the id it was asked about.**
///
/// It used to answer about the actor and discard the argument, which was
/// indistinguishable from correct with one character and a visible lie
/// with two: the information window on Bob would show Alice.
#[test]
fn get_public_info_answers_about_the_id_it_was_asked_for() {
    let mut r = two_characters();
    let ask = |r: &mut Registry, actor: i64, about: i64| {
        let got = r
            .dispatch(&Request {
                service: "charMgr",
                method: "GetPublicInfo",
                args: &[PyValue::Int(about)],
                kwargs: &[],
                bind_params: None,
                actor: Some(actor),
            })
            .expect("GetPublicInfo answers");
        let PyValue::Instance { state, .. } = got else {
            panic!("a KeyVal instance, got {got:?}")
        };
        crate::pyutil::dict_get(&state, b"characterID").cloned()
    };
    // Alice asking about Bob gets Bob, which is the whole fix.
    assert_eq!(ask(&mut r, ALICE, BOB), Some(PyValue::Int(BOB)));
    assert_eq!(ask(&mut r, BOB, ALICE), Some(PyValue::Int(ALICE)));
    assert_eq!(ask(&mut r, ALICE, ALICE), Some(PyValue::Int(ALICE)));
}

/// A character we have never heard of is still answered, and the answer
/// is still truthy.
///
/// `None` would be an `AttributeError` at
/// `crimewatchSvc.GetSlimItemDataForCharID`, which reads
/// `corporationID`, `warFactionID` and `allianceID` off the result with
/// no guard and no `try` above them. So the unknown branch is required
/// by the client, not defensive padding -- and it must carry those three
/// keys.
#[test]
fn an_unknown_character_is_answered_truthily_and_not_with_none() {
    let mut r = two_characters();
    let got = r
        .dispatch(&Request {
            service: "charMgr",
            method: "GetPublicInfo",
            args: &[PyValue::Int(424_242)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        })
        .expect("GetPublicInfo answers");
    let PyValue::Instance { state, .. } = got else {
        panic!("None here is an AttributeError on the client: {got:?}")
    };
    for key in [b"corporationID".as_slice(), b"allianceID", b"warFactionID"] {
        assert!(
            crate::pyutil::dict_get(&state, key).is_some(),
            "crimewatch reads {} with no guard",
            String::from_utf8_lossy(key)
        );
    }
    // Echoed, not silently turned into the actor.
    assert_eq!(
        crate::pyutil::dict_get(&state, b"characterID"),
        Some(&PyValue::Int(424_242))
    );
    // And an NPC corp, so nothing takes a player-corp branch about
    // somebody who does not exist.
    assert_eq!(
        crate::pyutil::dict_get(&state, b"corporationID"),
        Some(&PyValue::Int(crate::model::NPC_CORPORATION_ID))
    );
}

/// **The select screen's order is the logoff date, descending.**
///
/// `charselData.py:65` is `chars.sort(key=lambda x: x.logoffDate,
/// reverse=True)` **[V]**, so this field decides which character the
/// player sees first. Nothing wrote it until now, so every character
/// tied at 0 and the screen fell back to roster order.
#[test]
fn logging_off_stamps_the_character_and_orders_the_select_screen() {
    let mut r = two_characters();
    let open = |r: &mut Registry, client_id: i64, who: i64| {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
    };
    let logoff = |r: &mut Registry, id: i64| r.character(id).map(|c| c.logoff_date);

    assert_eq!(logoff(&mut r, ALICE), Some(0), "nothing has logged off yet");

    open(&mut r, 10_000_000_001, ALICE);
    open(&mut r, 20_000_000_001, BOB);
    r.session_closed(10_000_000_001);
    let alice_off = logoff(&mut r, ALICE).expect("Alice exists");
    assert!(alice_off > 0, "Alice logged off and was not stamped");
    assert_eq!(logoff(&mut r, BOB), Some(0), "Bob is still connected");

    r.session_closed(20_000_000_001);
    let bob_off = logoff(&mut r, BOB).expect("Bob exists");
    assert!(
        bob_off > alice_off,
        "Bob logged off last so he must sort first: {bob_off} vs {alice_off}"
    );
}

/// **A departure must leave on the clock, not on somebody's next turn.**
///
/// `pending` is drained by `dispatch.rs` after a payload, and a
/// disconnect is not a payload (`server.rs:402`) — so a departure queued
/// there waits for *any* connection to call something, which may be
/// never. The client cannot rescue it either: `StationSvc.GetGuests`
/// caches the list per station behind `guestListReceived`
/// (`station/base.py:135`), so reopening the lobby makes no call at all.
/// The frame is the only thing that can correct the panel.
///
/// This test fails if the announcement goes back on `pending`.
#[test]
fn a_departure_is_announced_by_the_clock_not_by_a_turn() {
    let mut r = two_characters();
    for (client_id, who) in [(10_000_000_001, ALICE), (20_000_000_001, BOB)] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
    }
    // Arrivals happen inside a turn, so they are entitled to `pending`.
    // Clear both queues to isolate what the disconnect itself produces.
    let _ = r.take_pending();
    let _ = r.take_due(Instant::now());

    // Alice quits. Bob is still standing in the station.
    r.session_closed(10_000_000_001);

    let due: Vec<&'static str> = r
        .take_due(Instant::now())
        .iter()
        .map(|n| n.to_push().0)
        .collect();
    let pending: Vec<&'static str> = r
        .take_pending()
        .iter()
        .map(|(_, n)| n.to_push().0)
        .collect();

    assert!(
        due.contains(&"OnCharNoLongerInStation"),
        "the departure has to be on the clock queue, which the world \
         ticker drains with no turn involved; due={due:?} pending={pending:?}"
    );
    assert!(
        !pending.contains(&"OnCharNoLongerInStation"),
        "the turn queue cannot deliver this one: a disconnect is not a \
         payload, and the client never re-asks for the guest list"
    );
}

/// **A pilot who drops in space was never a station guest**, so the lobby
/// is not told they left it. `Character::station_id` stays set on undock;
/// `Registry::docked_station` is what knows (`docs/lanes/docked.md`). The
/// docked half is `a_departure_is_announced_by_the_clock_not_by_a_turn`.
#[test]
fn a_pilot_who_disconnects_in_space_is_not_announced_leaving_the_station() {
    let mut r = two_characters();
    for (client_id, who) in [(10_000_000_001, ALICE), (20_000_000_001, BOB)] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
    }
    crate::destiny::on_ballpark_bind(&mut r, Some(ALICE));
    assert_eq!(r.docked_station(ALICE), None);
    let station = r.character(BOB).and_then(|c| c.station_id).expect("Bob's");
    assert!(
        r.characters_at_station(station)
            .iter()
            .all(|c| c.character_id != ALICE),
        "in space is not in the lobby"
    );
    let _ = r.take_pending();
    let _ = r.take_due(Instant::now());

    r.session_closed(10_000_000_001);

    let due: Vec<&'static str> = r
        .take_due(Instant::now())
        .iter()
        .map(|n| n.to_push().0)
        .collect();
    let pending: Vec<&'static str> = r
        .take_pending()
        .iter()
        .map(|(_, n)| n.to_push().0)
        .collect();
    assert!(
        !due.contains(&"OnCharNoLongerInStation") && !pending.contains(&"OnCharNoLongerInStation"),
        "due={due:?} pending={pending:?}"
    );
}

/// **Closing one of a character's two sessions must not stamp a logoff**,
/// because they are still playing on the other one.
///
/// `logoff_date` is not inert: `GetCharacterSelectionData` orders the
/// select screen by it, newest first. So a character still in space would
/// be shown as the most recently played the moment their second window
/// closed. The stamp belongs inside the `is_online` check, and it was
/// outside it.
#[test]
fn one_of_two_sessions_closing_does_not_stamp_a_logoff() {
    let mut r = two_characters();
    // Alice is logged in twice; whether that SHOULD be allowed is a
    // separate question (see `docs/lanes/session-and-binding.md`), but it
    // is possible today and must not corrupt her logoff date.
    for client_id in [10_000_000_001, 20_000_000_001] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, ALICE);
    }

    r.session_closed(10_000_000_001);
    assert_eq!(
        r.character(ALICE).map(|c| c.logoff_date),
        Some(0),
        "Alice still has a live session, so she has not logged off"
    );
    assert!(
        r.sessions.is_online(ALICE),
        "the other session must still count her as online"
    );

    r.session_closed(20_000_000_001);
    assert!(
        r.character(ALICE).expect("Alice").logoff_date > 0,
        "the last session closing is the logoff, and it must be stamped"
    );
}

/// **Every `rolesAt*` must stay 0, and the reason is a client deadlock.**
///
/// A non-zero `rolesAtAll` sends the client down `invCache.py:162`, which
/// makes a blocking `officeManager` call *inside the session-change
/// handler chain* — and the character-select click never returns. We are
/// safe today because the value is zero, **not** because the key is
/// absent, which is a distinction no test was holding: anyone adding
/// corporation roles would set these four in the obvious place and freeze
/// the select screen with nothing in the server log.
///
/// Evidence: `docs/lanes/session-and-binding.md`, breakage 3.
#[test]
fn every_rolesat_stays_zero_or_the_select_click_hangs() {
    let r = two_characters();
    let change = r
        .character_change(ALICE)
        .expect("Alice has a session change");
    let roles: Vec<(&str, &PyValue)> = change
        .iter()
        // **`(value, key)`, in that order** — `PyValue::Dict` stores
        // entries in wire order, where the dict handler reads the value
        // first and the key second. Reading these as `(key, value)`
        // silently matches nothing, which is how this test first passed
        // zero roles and claimed the keys were absent.
        .filter_map(|(v, k)| {
            let name: &str = match k {
                PyValue::Unicode(n) => n.as_str(),
                // Python 2 `str` on the wire is bytes, not text.
                PyValue::Bytes(b) => std::str::from_utf8(b).ok()?,
                _ => return None,
            };
            name.starts_with("rolesAt").then_some((name, v))
        })
        .collect();
    assert_eq!(
        roles.len(),
        4,
        "expected rolesAtAll/HQ/Base/Other; got {:?}",
        roles.iter().map(|(k, _)| *k).collect::<Vec<_>>()
    );
    for (name, v) in roles {
        let inner = match v {
            // `pair(..)` wraps each value as (None, new).
            PyValue::Tuple(t) if t.len() == 2 => &t[1],
            other => other,
        };
        assert_eq!(
            inner,
            &PyValue::Int(0),
            "{name} must be 0: a non-zero value hangs the character-select \
             click in invCache.py:162 and logs nothing server-side"
        );
    }
}

/// Two logoffs inside one FILETIME tick must not tie.
///
/// `list.sort` is stable, so a tie falls back to roster order and the
/// screen shows a stale "most recent" with nothing wrong anywhere to
/// find. The stamp is therefore one tick past the highest, not just the
/// clock.
#[test]
fn two_logoffs_in_one_tick_still_order() {
    let mut r = two_characters();
    for (client_id, who) in [(10_000_000_001, ALICE), (20_000_000_001, BOB)] {
        r.session_opened(crate::session_table::SessionEntry {
            client_id,
            session_id: client_id,
            user_id: 1,
            character_id: None,
        });
        r.session_selected_character(client_id, who);
    }
    // Back to back, with no sleep: whatever the clock resolution is,
    // these two land in the same tick far more often than not.
    r.session_closed(10_000_000_001);
    r.session_closed(20_000_000_001);
    let a = r.character(ALICE).expect("Alice").logoff_date;
    let b = r.character(BOB).expect("Bob").logoff_date;
    assert_ne!(a, b, "a tie here is a silently stale select screen");
    assert!(b > a);
}

/// A session that never selected a character stamps nobody.
#[test]
fn closing_a_session_with_no_character_stamps_nothing() {
    let mut r = two_characters();
    r.session_opened(crate::session_table::SessionEntry {
        client_id: 10_000_000_001,
        session_id: 1,
        user_id: 1,
        character_id: None,
    });
    r.session_closed(10_000_000_001);
    assert_eq!(r.character(ALICE).map(|c| c.logoff_date), Some(0));
    assert_eq!(r.character(BOB).map(|c| c.logoff_date), Some(0));
}

/// The stamp survives a restart, and it does so **without a schema
/// bump**: `logoff` is a new record type, and the parser skips records
/// it does not know, which is exactly what that leniency is for. A world
/// written before this has no `logoff` lines and loads unchanged.
#[test]
fn the_logoff_date_round_trips_through_a_snapshot() {
    let mut r = two_characters();
    r.session_opened(crate::session_table::SessionEntry {
        client_id: 10_000_000_001,
        session_id: 1,
        user_id: 1,
        character_id: None,
    });
    r.session_selected_character(10_000_000_001, BOB);
    r.session_closed(10_000_000_001);
    let stamped = r.character(BOB).expect("Bob").logoff_date;
    assert!(stamped > 0);

    let snap = r.to_snapshot();
    let mut fresh = two_characters();
    fresh.apply_snapshot(snap);
    assert_eq!(fresh.character(BOB).map(|c| c.logoff_date), Some(stamped));
    assert_eq!(
        fresh.character(ALICE).map(|c| c.logoff_date),
        Some(0),
        "a character the snapshot says nothing about keeps what it had"
    );
}

/// Skills are keyed by character id already, so this one was *almost*
/// right -- it just found the id by taking the first character.
#[test]
fn the_skills_are_the_actors() {
    let mut r = two_characters();
    r.slash(Some(BOB), "/giveskill 3302 4").expect("answers");
    let count = |actor: i64| match r.skill_dict(Some(actor)) {
        PyValue::Dict(e) => e.len(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        count(BOB),
        count(ALICE) + 1,
        "Bob's grant must not land in Alice's skill map"
    );
}

/// **Two players no longer share one mission.**
///
/// The cargo spawn was already keyed on the actor, so what Alice
/// accepting used to do was spawn *her* cargo and then tell Bob's agent
/// window the mission was accepted -- offering him a Complete button for
/// work he had not done.
#[test]
fn the_mission_is_per_character() {
    let mut r = two_characters();
    let accept = |r: &mut Registry, who: i64| {
        r.agent_do_action(Some(who), Some(crate::mission::ACTION_ACCEPT));
        let _ = r.take_pending();
    };
    let accepted =
        |r: &Registry, who: i64| r.mission_state(Some(who)) == crate::mission::State::Accepted;

    assert!(!accepted(&r, ALICE));
    assert!(!accepted(&r, BOB));

    accept(&mut r, ALICE);
    assert!(accepted(&r, ALICE), "Alice accepted it");
    assert!(
        !accepted(&r, BOB),
        "Bob was offered a Complete button for work he had not done"
    );

    // And the cargo, which was already per-actor, agrees with the state
    // rather than being the only thing that was right.
    let cargo = |r: &Registry, who: i64| -> i64 {
        r.store()
            .items()
            .iter()
            .filter(|i| i.type_id == crate::mission::CARGO_TYPE_ID && i.owner_id == who)
            .map(|i| i.quantity)
            .sum()
    };
    assert_eq!(cargo(&r, ALICE), crate::mission::CARGO_QUANTITY);
    assert_eq!(cargo(&r, BOB), 0);
}

/// An unknown or absent actor is `Offered` and stores nothing.
///
/// Refusing would be worse than answering: `_GetConversation` has no
/// branch for a refusal and the player would get a blank agent window.
/// So the conversation runs from the default state and the transition is
/// discarded.
#[test]
fn an_actorless_agent_action_answers_and_stores_nothing() {
    let mut r = two_characters();
    let answer = r.agent_do_action(None, Some(crate::mission::ACTION_ACCEPT));
    assert_ne!(answer, PyValue::None, "a blank agent window is worse");
    assert!(!accepted_anywhere(&r), "nothing may have been stored");
    // And an actor who is not a character of ours is the same case.
    r.agent_do_action(Some(424_242), Some(crate::mission::ACTION_ACCEPT));
    assert!(
        r.mission_state(Some(ALICE)) != crate::mission::State::Accepted,
        "a stranger's action must not land on Alice"
    );
}

fn accepted_anywhere(r: &Registry) -> bool {
    [ALICE, BOB]
        .iter()
        .any(|id| r.mission_state(Some(*id)) == crate::mission::State::Accepted)
}

/// Two characters sharing a `ship_id` is caught where the fixture is
/// built, not in a window that shows the wrong hull.
#[test]
#[should_panic(expected = "share ship_id")]
fn two_characters_may_not_share_a_ship_id() {
    let _ = Registry::new(SandboxConfig {
        characters: vec![
            Character {
                character_id: ALICE,
                ..Character::default()
            },
            Character {
                character_id: BOB,
                ..Character::default()
            },
        ],
        ..SandboxConfig::default()
    });
}

/// No service may be claimed by two subsystem modules.
///
/// `DOMAINS` is searched in order and the first match wins, so a service
/// in two `SERVICES` lists would be answered by one module and silently
/// not by the other -- and the one that loses is decided by the order of
/// a table nobody reads. The split that created these modules put the
/// fittings arm in `store` instead of `inventory` on its first attempt,
/// so this is not hypothetical.
#[test]
fn no_service_is_claimed_by_two_modules() {
    let mut seen: Vec<&str> = Vec::new();
    for (services, _) in DOMAINS {
        for s in *services {
            assert!(
                !seen.contains(s),
                "{s} is claimed by two subsystem modules; the first in \
                 DOMAINS would win and the other would never run"
            );
            seen.push(s);
        }
    }
}

/// Every service a module advertises must actually be dispatched.
///
/// [`Registry::services`] is derived from `DOMAINS` now, so the old
/// list-versus-table drift cannot happen -- but a module can still name a
/// service in its own `SERVICES` and have no arm for it, which is the bug
/// that mattered: a refused call is a hung client. This probes every
/// advertised name with a method that cannot exist and insists the answer
/// is `NoSuchMethod` (we reached the service) and not `NoSuchService`.
#[test]
fn every_advertised_service_is_actually_dispatched() {
    for name in Registry::services() {
        let req = Request {
            service: name,
            method: "__no_such_method_exists__",
            args: &[],
            kwargs: &[],

            bind_params: None,
            actor: ACTOR,
        };
        match registry().dispatch(&req) {
            Err(ServiceError::NoSuchMethod { .. }) => {}
            Err(ServiceError::NoSuchService(s)) => {
                // A service can also be answered one layer up, in
                // `dispatch`, when its call returns an object rather than
                // data -- `skillMgr2.GetMySkillHandler` is the case. That
                // is still a service we answer for, so it belongs in
                // SERVICES; it just has no arm here.
                assert!(
                    Registry::OBJECT_RETURNING
                        .iter()
                        .any(|(svc, _, _)| *svc == s),
                    "{s} is advertised in SERVICES but nothing dispatches it"
                );
            }
            other => panic!("{name}: unexpected answer to a nonexistent method: {other:?}"),
        }
    }
}

/// **The assets window needs both halves, and the second one is silent
/// when it is wrong.**
///
/// `ListIncludingContainers` sends the items; `ListStations` decides which
/// of them are shown. `assetsWindow.Search_thread` groups the items by
/// station and then drops every group whose station is missing from
/// `ListStations().Index('stationID')` -- so an empty station rowset makes
/// a perfectly good item list invisible, with no traceback anywhere. That
/// is a worse failure than the `TypeError` the item call used to raise,
/// because nothing names it; it would read as "the fix did not work".
#[test]
fn the_station_rowset_carries_the_station_the_items_are_in() {
    let reg = registry();
    let rows = reg.owned_item_rows(actor(&reg));
    assert!(
        !rows.is_empty(),
        "the character owns at least the capsule they are sitting in"
    );

    let answer = station_assets_rowset(&reg, actor(&reg));
    let PyValue::ObjectEx { list, .. } = &answer else {
        panic!("a CRowset marshals as an 0x22 object: {answer:?}")
    };
    let _ = list;
    // Reach the rows the way the client does: the CRowset's own payload.
    let crowset = {
        let mut cr = eve_rowset::CRowset::new(
            DbRowDescriptor::new(
                [
                    ("stationID", DbType::I8),
                    ("typeID", DbType::I4),
                    ("solarSystemID", DbType::I4),
                    ("itemCount", DbType::I4),
                ]
                .into_iter()
                .map(|(name, db_type)| Column {
                    name: name.as_bytes().to_vec(),
                    db_type,
                })
                .collect(),
            )
            .expect("the station columns are a valid layout"),
        );
        let c = &reg.config().characters[0];
        cr.push_values(vec![
            DbValue::Int(c.station_id.expect("docked")),
            DbValue::Int(c.station_type_id),
            DbValue::Int(c.solar_system_id.expect("in a system")),
            DbValue::Int(rows.len() as i64),
        ])
        .expect("row matches descriptor");
        cr
    };
    assert_eq!(
        answer,
        crowset.to_pyvalue(),
        "the station row must name the character's own station and count \
         the items we actually sent, or the window drops every one of them"
    );
}

/// The portrait batch must answer with a container the client can call
/// `.iteritems()` on. `None` would be an `AttributeError` in the
/// `photo::ProcessScenePortraits` worker, and a `UserError` would be read
/// as rate limiting and retried every 5 seconds forever.
#[test]
fn the_portrait_batch_answers_a_dict_even_when_it_is_empty() {
    let req = Request {
        service: "paperDollServer",
        method: "GetPaperDollDataBatch",
        args: &[PyValue::List(vec![PyValue::Int(90_000_001)])],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    assert!(
        matches!(got, PyValue::Dict(_)),
        "the client calls .iteritems() on this with no guard, got {got:?}"
    );
}

/// Every attribute the client reads off a character row, recovered by
/// decompiling the only two modules that touch one:
/// `loginCharacterselection/charselData.py` and
/// `charSelection/characterSlots.py`. `factionID` is read through
/// `getattr(.., None)` and so is the one optional name; every other entry
/// here is a bare attribute access that raises `AttributeError` if we
/// leave it out.
const ROW_ATTRIBUTES: &[&str] = &[
    "characterID",
    "characterName",
    "logoffDate",
    "corporationID",
    "allianceID",
    "stationID",
    "solarSystemID",
    "factionID",
    "balance",
    "balanceChange",
    "skillPoints",
    "unreadMailCount",
    "shipTypeID",
    "paperdollState",
    "finishedSkills",
    "skillTypeID",
    "toLevel",
    "trainingStartTime",
    "trainingEndTime",
    "queueEndTime",
    "finishSP",
    "trainedSP",
    "deletePrepareDateTime",
    "lockTypeID",
];

/// The character row must carry every attribute the client reads.
///
/// This is the test that was missing when the select screen first drew.
/// The screen appeared, and then froze: the row was short of
/// `skillTypeID`, which threw inside `LoadSlot` and left the slot
/// half-built, and short of `deletePrepareDateTime`, which threw from the
/// **mouse-over** handler and so threw again on every hover until the
/// client's own recursion guard tripped.
///
/// Neither failure reached our logs at all. Every call was answered, so
/// there was no `UNANSWERED` line, and an `AttributeError` on the far side
/// of a reply we already sent is invisible from here -- it only showed up
/// in the client's own traceback log. A missing field is much easier to
/// assert than to diagnose, so assert it.
///
/// Two names we used to send -- `petitionMessage` and
/// `deleteCharacterTime` -- appear nowhere in the client at all. They were
/// invented, and `deleteCharacterTime` was close enough to the real
/// `deletePrepareDateTime` to look right in a review.
/// The attribute names on a `KeyVal`, which is how every row the select
/// screen reads is carried.
fn keyval_attributes(v: &PyValue) -> Vec<String> {
    let PyValue::Instance { state, .. } = v else {
        panic!("expected a KeyVal instance, got {v:?}")
    };
    // A KeyVal's state is its attribute dict, stored value-then-key.
    let PyValue::Dict(entries) = state.as_ref() else {
        panic!("KeyVal state must be a dict, got {state:?}")
    };
    entries
        .iter()
        .map(|(_value, key)| match key {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            PyValue::Unicode(s) => s.clone(),
            other => panic!("a KeyVal attribute name must be a string, got {other:?}"),
        })
        .collect()
}

#[test]
fn the_character_row_carries_every_attribute_the_client_reads() {
    let present = keyval_attributes(&Character::default().to_row());
    let missing: Vec<&str> = ROW_ATTRIBUTES
        .iter()
        .copied()
        .filter(|want| !present.iter().any(|p| p == want))
        .collect();
    assert!(
        missing.is_empty(),
        "the client reads these off the row and will raise AttributeError: {missing:?}"
    );
}

/// Nothing in the row should be a name the client never reads.
///
/// The other half of the bug above: an invented field is not harmless,
/// because it reads as evidence that we checked. If this fails, either
/// the name is wrong or `ROW_ATTRIBUTES` needs the real one adding.
#[test]
fn the_character_row_invents_no_attributes() {
    for name in keyval_attributes(&Character::default().to_row()) {
        assert!(
            ROW_ATTRIBUTES.contains(&name.as_str()),
            "{name} is read nowhere in the client: misspelled, or should not be sent"
        );
    }
}

/// The same check for the user row. `charselData.CharacterSelectionData`
/// reads exactly these five off `userDetails[0]`, each through a bare
/// attribute access.
#[test]
fn the_user_row_matches_what_the_client_reads() {
    let selection = registry().character_selection_data();
    let PyValue::Tuple(parts) = &selection else {
        panic!("GetCharacterSelectionData returns a 4-tuple, got {selection:?}")
    };
    let PyValue::List(users) = &parts[0] else {
        panic!("the user details are a list, got {:?}", parts[0])
    };
    let present = keyval_attributes(&users[0]);
    for want in [
        "userName",
        "characterSlots",
        "maxCharacterSlots",
        "creationDate",
        "subscriptionEndTime",
    ] {
        assert!(
            present.iter().any(|p| p == want),
            "the client reads userDetails.{want}; row has {present:?}"
        );
    }
}

/// Any service at all can be resolved, because every bindable service
/// exports `MachoResolveObject` and we are a single node. Answering it
/// for services we otherwise know nothing about is deliberate: it trades
/// a hang whose only evidence is a service name for one that names the
/// method, via the fused call in the `MachoBindObject` that follows.
#[test]
fn any_service_resolves_to_our_node() {
    for service in [
        "populationCap",
        "dogmaIM",
        "invbroker",
        "a_service_we_never_heard_of",
    ] {
        let req = Request {
            service,
            method: "MachoResolveObject",
            args: &[PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(1)])],
            kwargs: &[],

            bind_params: None,
            actor: ACTOR,
        };
        assert_eq!(
            registry().dispatch(&req).expect("resolved"),
            PyValue::Int(SandboxConfig::default().node_id),
            "{service} must resolve to the node that will own the object"
        );
    }
}

/// `GetAllInfo` must carry a genuinely primed character.
///
/// All six attributes have to exist because `Prime` reads each of them,
/// and `charInfo` cannot be falsy: skipping `PrimeLocation` makes the
/// unconditional `GetItem(session.charid)` at the end of `Prime` return
/// None, and `getattr(None, 'memory')` raises inside the blocking
/// session-change handler.
#[test]
fn get_all_info_primes_the_character() {
    let req = Request {
        service: "dogmaIM",
        method: "GetAllInfo",
        args: &[PyValue::Bool(true), PyValue::None, PyValue::None],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let present = keyval_attributes(&got);
    for want in [
        "structureInfo",
        "shipInfo",
        "charInfo",
        "shipModifiedCharAttribs",
        "systemWideEffectsOnShip",
        "shipState",
    ] {
        assert!(present.iter().any(|p| p == want), "Prime reads .{want}");
    }

    // charInfo = (contents, charBrain)
    let PyValue::Instance { state, .. } = &got else {
        panic!("allInfo is a KeyVal")
    };
    let PyValue::Dict(entries) = state.as_ref() else {
        panic!("KeyVal state is a dict")
    };
    let char_info = entries
        .iter()
        .find(|(_v, k)| matches!(k, PyValue::Bytes(b) if b == b"charInfo"))
        .map(|(v, _k)| v)
        .expect("charInfo present");
    let PyValue::Tuple(parts) = char_info else {
        panic!("charInfo must be (contents, charBrain), got {char_info:?}")
    };
    assert_eq!(parts.len(), 2, "(contents, charBrain)");

    // The brain must be a truthy 4-tuple: `if charBrain:` guards
    // `initialBrain = (-1,) + charBrain[1:]`.
    let PyValue::Tuple(brain) = &parts[1] else {
        panic!("charBrain must be a tuple, got {:?}", parts[1])
    };
    assert_eq!(brain.len(), 4, "(version, char, ship, structure) effects");

    // The contents must hold the character, keyed by its id, and its
    // invItem must be a real DBRow.
    let PyValue::Dict(contents) = &parts[0] else {
        panic!("charInfo[0] must be a dict, got {:?}", parts[0])
    };
    assert_eq!(contents.len(), 1, "only the character belongs in here");
    let (row, key) = &contents[0];
    assert_eq!(*key, PyValue::Int(Character::default().character_id));

    let row_attrs = keyval_attributes(row);
    for want in [
        "itemID",
        "invItem",
        "attributes",
        "activeEffects",
        "time",
        "wallclockTime",
    ] {
        assert!(
            row_attrs.iter().any(|a| a == want),
            "a KeyVal raises AttributeError on a missing .{want}"
        );
    }

    let PyValue::Instance { state, .. } = row else {
        panic!("the info row is a KeyVal")
    };
    let PyValue::Dict(fields) = state.as_ref() else {
        panic!("KeyVal state is a dict")
    };
    let inv_item = fields
        .iter()
        .find(|(_v, k)| matches!(k, PyValue::Bytes(b) if b == b"invItem"))
        .map(|(v, _k)| v)
        .expect("invItem present");
    let PyValue::DbRow(dbrow) = inv_item else {
        panic!(
            "invItem must be a real blue.DBRow -- ItemWrapper.__getattr__                  tests `attribute in dbrow.__columns__`, which a KeyVal has not"
        )
    };

    // The two values that decide whether the row survives UpdateItem.
    let columns = dbrow.descriptor().columns();
    let index = |name: &str| {
        columns
            .iter()
            .position(|c| c.name == name.as_bytes())
            .unwrap_or_else(|| panic!("no {name} column"))
    };
    assert_eq!(
        dbrow.values()[index("groupID")],
        DbValue::Int(GROUP_CHARACTER),
        "the only reason a category-1 item passes UpdateItem's filter"
    );
    assert_ne!(
        dbrow.values()[index("singleton")],
        DbValue::Int(0),
        "a falsy singleton makes skip3 return before any attribute is stored"
    );
}

/// The aggression settings must be an instance of a **whitelisted**
/// class carrying the two attributes its `__init__` assigns.
///
/// A class name the client's `globalsWhitelist` does not know is a
/// `HACKER WARNING! object %s.%s is not in whitelist`, which is a far
/// less obvious failure than a missing attribute, so the whitelist is
/// asserted here rather than taken on trust.
#[test]
fn the_corp_aggression_settings_are_a_whitelisted_instance() {
    let req = Request {
        service: "corpRegistry",
        method: "GetAggressionSettings",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Instance { class, .. } = &got else {
        panic!("must be an old-style instance, got {got:?}")
    };
    assert!(
        eve_rowset::is_whitelisted(class),
        "{} is not in the client's globalsWhitelist",
        String::from_utf8_lossy(class)
    );
    let present = keyval_attributes(&got);
    for want in ["_enableAfter", "_disableAfter"] {
        assert!(
            present.iter().any(|a| a == want),
            "AggressionSettings.__init__ assigns {want}; got {present:?}"
        );
    }
    // ALWAYS_DISABLED is `AggressionSettings(None, 0)`, which is what the
    // client's own CreateDefaultForCorp picks for an NPC corp.
    let PyValue::Instance { state, .. } = &got else {
        unreachable!()
    };
    let PyValue::Dict(fields) = state.as_ref() else {
        panic!("state is a dict")
    };
    let disable_after = fields
        .iter()
        .find(|(_v, k)| matches!(k, PyValue::Bytes(b) if b == b"_disableAfter"))
        .map(|(v, _k)| v)
        .expect("_disableAfter present");
    assert_eq!(*disable_after, PyValue::Int(0), "ALWAYS_DISABLED");
}

/// The station item bits must be exactly four ints, and the `itemID`
/// must be the station the character is in.
///
/// `Row.__init__` raises unless the length matches its four-name header,
/// and `GetStationItem` re-issues this call every time
/// `_stationItem.itemID != session.stationid` — so a wrong id is not one
/// wrong value, it is a synchronous call storm driven by every hangar
/// operation.
#[test]
fn the_station_item_bits_are_four_ints_naming_our_station() {
    let req = Request {
        service: "stationSvc",
        method: "GetStationItemBits",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Tuple(bits) = &got else {
        panic!("must be a sequence of four, got {got:?}")
    };
    assert_eq!(bits.len(), 4, "Row() raises on any other length");
    for b in bits {
        assert!(matches!(b, PyValue::Int(_)), "all four are ints: {b:?}");
    }
    // (ownerID, itemID, operationID, stationTypeID)
    let expected = Character::default();
    assert_eq!(
        bits[1],
        PyValue::Int(expected.station_id.expect("docked")),
        "itemID must equal session.stationid or the client re-asks forever"
    );
    assert_eq!(bits[0], PyValue::Int(expected.station_owner_id));
    assert_eq!(bits[2], PyValue::Int(expected.station_operation_id));
    assert_eq!(bits[3], PyValue::Int(expected.station_type_id));
}

/// `GetMyEngagements` proves the second element is a dict.
///
/// This was a `[]` for several runs. The arity matched, the unpack in
/// `ProcessSessionChange` succeeded, the session change completed -- and
/// the failure surfaced later and elsewhere, on the timers HUD's tasklet:
///
///     AttributeError: 'list' object has no attribute 'copy'
///     crimewatchSvc.py(342) GetMyEngagements
///
/// which is `return self.myEngagements.copy()`. Python 2 lists have no
/// `.copy()`; dicts do, and `__init__` says `self.myEngagements = {}`.
#[test]
fn the_crimewatch_engagements_are_a_dict_not_a_list() {
    let got = registry()
        .dispatch(&Request {
            service: "crimewatch",
            method: "GetClientStates",
            args: &[],
            kwargs: &[],

            bind_params: None,
            actor: ACTOR,
        })
        .expect("answered");
    let PyValue::Tuple(parts) = &got else {
        panic!("destructured to three levels on arrival, got {got:?}")
    };
    // myCombatTimers, myEngagements, flaggedCharacters, safetyLevel
    assert_eq!(parts.len(), 4);
    assert_eq!(
        parts[1],
        PyValue::Dict(vec![]),
        "a list here is an AttributeError on .copy(), one tasklet later"
    );
    // The first is five (state, expiry) pairs, each unpacked by name.
    let PyValue::Tuple(timers) = &parts[0] else {
        panic!("combat timers must be a tuple")
    };
    assert_eq!(timers.len(), 5);
    assert!(
        timers
            .iter()
            .all(|t| matches!(t, PyValue::Tuple(p) if p.len() == 2))
    );
}

/// The capsule: one item row, reachable three different ways.
///
/// The same row has to satisfy the hangar listing (where
/// `GetShipItemFromHangar` matches `itemID` and `categoryID`), the dogma
/// prime (where `UpdateItem` drops anything whose `singleton` is falsy),
/// and the ship's own inventory's `GetSelfInvItem`.
#[test]
fn the_capsule_is_a_ship_in_the_hangar() {
    let mut r = registry();
    let hangar = PyValue::Tuple(vec![PyValue::Int(10_004), PyValue::None]);
    let listed = r
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "List",
            args: &[],
            kwargs: &[],
            bind_params: Some(&hangar),
            actor: ACTOR,
        })
        .expect("answered");
    let PyValue::List(rows) = &listed else {
        panic!("List must be a sequence of DBRows, got {listed:?}")
    };
    assert_eq!(rows.len(), 1, "the capsule, and nothing else yet");
    let PyValue::DbRow(row) = &rows[0] else {
        panic!("rows must be blue.DBRow")
    };
    let col = |name: &str| {
        let i = row
            .descriptor()
            .columns()
            .iter()
            .position(|c| c.name == name.as_bytes())
            .unwrap_or_else(|| panic!("no {name} column"));
        row.values()[i].clone()
    };
    // Matched by GetShipItemFromHangar, and what partitions the Ship
    // Hangar from the Item Hangar.
    assert_eq!(col("categoryID"), DbValue::Int(6), "categoryShip");
    assert_eq!(col("groupID"), DbValue::Int(29), "groupCapsule");
    assert_eq!(col("typeID"), DbValue::Int(670), "typeCapsule");
    // The active ship is listed in the ordinary hangar; flagPilot is
    // applied client-side to the character, not to this row.
    assert_eq!(col("flagID"), DbValue::Int(4), "flagHangar");
    // UpdateItem drops a falsy singleton at skip3, silently.
    assert_eq!(col("singleton"), DbValue::Int(1));
    // Tested by ProcessInventoryChange and every IsItemHere.
    assert_eq!(col("locationID"), DbValue::Int(60_012_526));
    assert_eq!(col("ownerID"), DbValue::Int(90_000_001));
    // >= minPlayerItem, or the client routes the id through its own
    // local sqlite fetcher instead of asking us.
    let DbValue::Int(item_id) = col("itemID") else {
        panic!("itemID must be an int")
    };
    assert!(item_id >= 100_000_000, "below minPlayerItem: {item_id}");

    // The ship's OWN inventory must answer with the capsule, not the
    // station -- `_SetItemData` asks for every container, magic key or
    // not, and a wrong answer builds the pod's dogma item as a station.
    let ship = PyValue::Tuple(vec![PyValue::Int(item_id), PyValue::Int(0)]);
    let self_item = r
        .dispatch(&Request {
            service: "invbroker::inventory",
            method: "GetSelfInvItem",
            args: &[],
            kwargs: &[],
            bind_params: Some(&ship),
            actor: ACTOR,
        })
        .expect("answered");
    let PyValue::DbRow(row) = &self_item else {
        panic!("must be a DBRow")
    };
    assert_eq!(row.values()[0], DbValue::Int(item_id));
    assert_eq!(
        row.values()[1],
        DbValue::Int(670),
        "the pod, not the station"
    );
}

/// **Every mutator we refuse must refuse with a key the client can
/// render, and must not refuse with a key that means something specific.**
///
/// Two different mistakes are possible and this test catches both. A key
/// that does not exist in `dialogs.static` gives the player a *fatal*
/// "could not find message with key" modal -- worse than the `None` these
/// arms replaced, and we shipped exactly that once
/// (`OfferNotAvailable`). And a key that *does* exist but means something
/// precise is a lie: `ConBidTooLow` tells the player their bid was too
/// low, `PermissionDenied` sends them hunting for a password,
/// `NotEnoughCargoSpace` says their hold is full. None of those is why we
/// are refusing.
#[test]
fn every_refusal_uses_a_generic_key_that_the_client_can_render() {
    // The keys we are allowed to emit, and the reason each is here.
    // Anything else wants checking against `findings/user-errors.tsv`
    // first -- that file is the list of the 4,223 keys that resolve.
    const ALLOWED: &[&str] = &[
        "CustomNotify",         // generic, renders one log line
        "CustomInfo",           // generic, renders a modal
        "FolderAccessDenied",   // the client evicts and refreshes cleanly
        "OfferNotAvailableNow", // real, and the only specific one we send
        "SlashError",           // our own console's error channel
        "IndustryBlueprintNotFound",
        "IndustryFacilityAccessDenied",
        "IndustryJobAccessDenied",
        // A contract id that does not exist or is not the caller's to see.
        // The client raises this very key itself for the same case
        // (contracts.py `AcceptContract`, `DeleteContract`), so it is the
        // generic answer, not a specific claim.
        "ConContractNotFound",
    ];
    // Keys that exist but carry a specific meaning we must never claim.
    const FORBIDDEN: &[&str] = &[
        "ConBidTooLow",
        "PermissionDenied",
        "NotEnoughCargoSpace",
        "EffectAlreadyActive2",
    ];

    let mut r = registry();
    let mut seen = std::collections::BTreeSet::new();
    for service in Registry::services() {
        // One method name that cannot exist, to find the arm list, plus
        // the real mutators we know refuse.
        for method in [
            "CreateContract",
            "AcceptContract",
            "DeleteContract",
            "PlaceBid",
            "CompleteContract",
            "FinishAuction",
            "StackAll",
            "UnloadAmmo",
            "SetModuleOnline",
            "AddFolder",
            "UpdateKnownFolderState",
            "RepackageItems",
            "BuyOffer",
        ] {
            let got = r.dispatch(&Request {
                service,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            });
            let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = got else {
                continue;
            };
            let PyValue::Tuple(parts) = *header else {
                panic!("{service}.{method}: the exception header is (class, args)")
            };
            assert_eq!(
                parts[0],
                PyValue::Global(b"eveexceptions.UserError".to_vec()),
                "{service}.{method} must raise eveexceptions.UserError, and \
                 not the class's own __guid__, which trips HACKER WARNING!"
            );
            let PyValue::Tuple(args) = &parts[1] else {
                panic!("{service}.{method}: UserError.__init__ takes (msg, dict)")
            };
            let PyValue::Bytes(key) = &args[0] else {
                panic!("{service}.{method}: the key is a str")
            };
            let key = String::from_utf8_lossy(key).into_owned();
            assert!(
                !FORBIDDEN.contains(&key.as_str()),
                "{service}.{method} raises {key}, which tells the player \
                 something specific that is not why we are refusing"
            );
            assert!(
                ALLOWED.contains(&key.as_str()),
                "{service}.{method} raises {key}, which is not in this \
                 test's allow-list -- check it against \
                 findings/user-errors.tsv and add it with a reason"
            );
            seen.insert(key);
        }
    }
    assert!(
        seen.len() >= 2,
        "the sweep found almost nothing, so it is probably not reaching \
         the arms any more: {seen:?}"
    );
}

/// **The GM skill commands, which make the skill system testable without
/// a market.**
///
/// Each one has to do two things, and the second is the one that is easy
/// to leave out: change the map, and tell the client to re-read it. The
/// client's skill caches are reset only by `OnSkillForcedRefresh`, so a
/// grant without the push is a grant nobody can see.
#[test]
fn the_gm_skill_commands_change_the_map_and_push_the_refresh() {
    let mut r = registry();
    let count = |r: &Registry| {
        let PyValue::Dict(entries) = r.skill_dict(actor(r)) else {
            panic!("the skill dict must be a dict")
        };
        entries.len()
    };
    let starter = count(&r);
    assert!(starter > 0, "the starter set seeds something to look at");

    // 3302 is Small Projectile Turret, read out of the client's own
    // `types.fsdbinary` -- not in the starter set.
    let line = match r.slash(actor(&r), "/giveskill 3302 4").expect("answers") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(line.contains("Small Projectile Turret"), "{line}");
    assert!(line.contains('4'), "the level is worth echoing: {line}");
    assert_eq!(count(&r), starter + 1);
    // The **event the client will see**, not the type we queued: an
    // assertion on the variant would pass just as well if `to_push`
    // named the wrong method, which is the half that reaches the client.
    assert_eq!(
        r.take_pending()
            .iter()
            .map(|(_, n)| n.to_push())
            .collect::<Vec<_>>(),
        vec![("OnSkillForcedRefresh", vec![])],
        "a grant the client never re-reads is a grant nobody can see"
    );

    // By name, case-insensitively, which is what a person types.
    r.slash(actor(&r), "/giveskill gunnery 5").expect("answers");
    assert_eq!(
        count(&r),
        starter + 1,
        "Gunnery is already in the starter set"
    );

    assert!(r.slash(actor(&r), "/removeskill Gunnery").is_ok());
    assert_eq!(count(&r), starter, "removing one that was there");

    // And removing one that was not is a message, not a silent success.
    let line = match r.slash(actor(&r), "/removeskill Gunnery").expect("answers") {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(line.contains("not trained"), "{line}");

    // The reset restores exactly the starter set, whatever happened since.
    r.slash(actor(&r), "/givestarterskills").expect("answers");
    assert_eq!(count(&r), starter);
}

/// An unknown skill lists what is known rather than failing silently.
#[test]
fn a_bad_giveskill_names_the_skills_it_does_know() {
    let mut r = registry();
    let line = match r
        .slash(actor(&r), "/giveskill Thermodynamics")
        .expect("answers")
    {
        PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(line.contains("Thermodynamics"), "{line}");
    assert!(
        line.contains("Gunnery"),
        "it must list what it does know: {line}"
    );
    assert!(
        r.take_pending().is_empty(),
        "nothing changed, so nothing to push"
    );
}

/// **`/giveisk` now leaves a trace, which is tracker #1.**
///
/// The balance has been live since the push path landed; what was missing
/// was any record of how it got there. This asserts the whole chain --
/// the grant writes a journal entry, `GetTransactions` reads it back
/// through the four arguments it used to ignore, and the values arrive in
/// the units the client divides by.
#[test]
fn giveisk_writes_a_journal_entry_the_wallet_can_read_back() {
    let transactions = |r: &mut Registry, args: Vec<PyValue>| {
        let out = r
            .dispatch(&Request {
                service: "account",
                method: "GetTransactions",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            })
            .expect("GetTransactions answers");
        // A `CRowset`, which is forced: every row is handed to
        // `utillib.KeyVal(row)`, and that raises `TypeError` on anything
        // that is not a dict, a `blue.DBRow` or another KeyVal.
        //
        // It marshals as a `0x22` ObjectEx in call form, **not** a `0x17`
        // instance -- the class is reconstructed by calling it with the
        // descriptor and the rows, rather than by restoring state. The
        // first draft of this test asserted `Instance` and the failure
        // said so.
        let PyValue::ObjectEx { header, .. } = out else {
            panic!("GetTransactions must be a CRowset ObjectEx, got {out:?}")
        };
        let PyValue::Tuple(parts) = *header else {
            panic!("the ObjectEx header must be (class, args)")
        };
        assert_eq!(
            parts[0],
            PyValue::Global(b"carbon.common.script.sys.crowset.CRowset".to_vec()),
            "the journal is a CRowset, and only a whitelisted class name \
             survives the client's loader"
        );
        parts[1].clone()
    };

    let mut r = registry();
    // `(accountKey, year, month, isCorp)`, as the window sends it on open.
    let recent = vec![
        PyValue::Int(1000),
        PyValue::None,
        PyValue::None,
        PyValue::None,
    ];

    let before = transactions(&mut r, recent.clone());
    let rows_in = |state: &PyValue| -> usize {
        // The rows live in the instance's state; count them without
        // depending on the exact nesting, which `eve-rowset` owns.
        fn count(v: &PyValue) -> usize {
            match v {
                PyValue::DbRow(_) => 1,
                PyValue::List(items) | PyValue::Tuple(items) => items.iter().map(count).sum(),
                PyValue::Dict(kv) => kv.iter().map(|(a, b)| count(a) + count(b)).sum(),
                _ => 0,
            }
        }
        count(state)
    };
    assert_eq!(rows_in(&before), 0, "an empty wallet has no history");

    r.slash(actor(&r), "/giveisk 1000000")
        .expect("giveisk answers");

    let after = transactions(&mut r, recent);
    assert_eq!(rows_in(&after), 1, "one grant, one journal line");

    // And the entry itself, in ISK rather than in the wire's units.
    let entries = r.journal().entries();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.entry_type_id, crate::ledger::REF_GM_CASH_TRANSFER);
    assert_eq!(e.amount, 1_000_000.0);
    assert_eq!(
        e.balance, 1_000_000.0,
        "the balance *after* the grant, so the journal reads as a running total"
    );
    assert_eq!(e.owner_id_2, 90_000_001, "credited to the character");
    assert!(
        e.date > 0,
        "a zero date raises ValueError inside strftime and takes down the \
         whole Journal tab, uncaught"
    );
}

/// A month the player picks is answered with that month, not with
/// everything.
///
/// The cheap implementation -- ignore the range and return every row --
/// would pass every test that only ever asks for "recent", and would show
/// a grant made today under any month in the combo.
#[test]
fn a_month_the_player_picks_is_answered_honestly() {
    let mut r = registry();
    r.slash(actor(&r), "/giveisk 50").expect("giveisk answers");
    let ask = |r: &mut Registry, y: i64, m: i64| {
        r.dispatch(&Request {
            service: "account",
            method: "GetTransactions",
            args: &[
                PyValue::Int(1000),
                PyValue::Int(y),
                PyValue::Int(m),
                PyValue::None,
            ],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("answers")
    };
    // 2003 is before this grant by two decades, so it must be empty
    // whatever the month.
    let then = ask(&mut r, 2003, 5);
    let now_ft = crate::pyutil::filetime_now();
    let (y, m, _) = crate::ledger::civil_from_filetime(now_ft);
    let this_month = ask(&mut r, y, m);
    assert_ne!(
        format!("{then:?}"),
        format!("{this_month:?}"),
        "a month with the grant in it must not look like one without it"
    );
}

/// **The hangar listing grows, which is the whole point of the store.**
///
/// `invbroker.List` on the station hangar used to synthesise the one row
/// it knew about -- the capsule -- from `Character::ship_id`. So an item
/// could exist in every other sense and still not be in the hangar,
/// because the hangar was not reading anything. This asserts the two ends
/// against each other: `/createitem` writes, `List` reads, and the count
/// moves.
///
/// It also pins the capsule as the *first* occupant rather than a special
/// case, because that was the real change -- the pod is an ordinary item
/// now, and if it ever stops being listed this is what notices.
#[test]
fn createitem_shows_up_in_the_hangar_listing() {
    let hangar = |r: &mut Registry| {
        let out = r
            .dispatch(&Request {
                // The *bound object's* synthetic name, not `invbroker`:
                // `List` is a method on the inventory that
                // `GetInventory` handed out, and it has no plain-call
                // form. Getting this wrong is a `NoSuchMethod`, which is
                // how the test found out.
                service: "invbroker::inventory",
                method: "List",
                args: &[],
                kwargs: &[],
                // Which inventory, from the bind params the minting call
                // carried. `container_of` reads the first element, and
                // `CONTAINER_HANGAR` is 10004 -- a container id, not the
                // station's.
                bind_params: Some(&PyValue::Tuple(vec![PyValue::Int(CONTAINER_HANGAR)])),
                actor: ACTOR,
            })
            .expect("List answers");
        let PyValue::List(rows) = out else {
            panic!("List must be a list of rows, got {out:?}")
        };
        rows.len()
    };

    let mut r = registry();
    assert_eq!(hangar(&mut r), 1, "the capsule, and nothing else yet");

    let out = r
        .slash(actor(&r), "/createitem Rifter 2")
        .expect("createitem answers");
    let PyValue::Bytes(b) = out else {
        panic!("the console stringifies the result")
    };
    let line = String::from_utf8_lossy(&b);
    assert!(line.contains("Rifter"), "{line}");

    assert_eq!(
        hangar(&mut r),
        3,
        "the capsule plus two Rifters -- a singleton type is N objects, \
         not one row with a quantity"
    );

    // And the client is told to drop its cache, in the same breath. The
    // item being there is not the same as the player being able to see
    // it: `invCacheContainer.listed` has no TTL.
    // **Three pushes, in this order**, and the order is the contract:
    // one `OnItemChange` per item created, so each reaches the client's
    // cache, and *then* the invalidation that drops the container. The
    // reverse order would race the re-ask against the adds.
    let pending = r.take_pending();
    assert_eq!(
        pending.len(),
        3,
        "two Rifters plus one invalidation: {pending:?}"
    );
    assert!(
        pending[0].1.to_push().0 == "OnItemChange" && pending[1].1.to_push().0 == "OnItemChange",
        "the per-item pushes come first: {pending:?}"
    );
    assert_eq!(
        pending[2].1.to_push(),
        (
            "OnInvalidateCacheForLocations",
            vec![PyValue::List(vec![PyValue::Int(60012526)])]
        ),
        "and the invalidation names the station as a list"
    );
    for (after, _) in &pending {
        assert_eq!(
            *after,
            Duration::ZERO,
            "all of them ride with the answer; a delayed one would let \
             the player look before it arrived"
        );
    }
}

/// The client's console sends `/createitem "Name With Spaces" 2` with the
/// quotes still in the words; they are dropped (FCI-FIX2).
#[test]
fn createitem_accepts_a_quoted_name_and_a_quantity() {
    let mut r = registry();
    let out = r
        .slash(actor(&r), "/createitem \"Rifter\" 2")
        .expect("createitem answers");
    let PyValue::Bytes(b) = out else {
        panic!("the console stringifies the result")
    };
    let line = String::from_utf8_lossy(&b);
    assert!(line.contains("created 2 x Rifter"), "{line}");
}

/// An unknown type is refused with a usable message rather than granted
/// as something else.
///
/// The list is two dozen types, so "no such type" has to say what *is*
/// available or the player's only recourse is to read the source.
#[test]
fn a_bad_createitem_explains_itself_and_grants_nothing() {
    let mut r = registry();
    // Each of the three fails differently and each message has to be
    // actionable on its own, so they are asserted apart rather than
    // through one disjunction -- a loose `contains` here passed a message
    // that still named a command this server does not have.
    let say = |r: &mut Registry, line: &str| {
        let out = r.slash(actor(r), line).expect("must answer, not raise");
        let PyValue::Bytes(b) = out else {
            panic!("{line}: must be a string")
        };
        String::from_utf8_lossy(&b).into_owned()
    };

    // An unknown type points at the command that lists them rather than
    // printing two dozen names inside an error.
    let text = say(&mut r, "/createitem Nyx");
    assert!(text.contains("Nyx"), "{text}");
    assert!(text.contains("/createitem"), "{text}");

    // A bad quantity names the range it wanted.
    let text = say(&mut r, "/createitem Rifter 0");
    assert!(text.contains("quantity"), "{text}");
    assert!(text.contains("1000"), "{text}");

    // The bare command is the usage line, and it is the only place the
    // full list belongs.
    let text = say(&mut r, "/createitem");
    assert!(text.contains("usage"), "{text}");
    assert!(text.contains("Rifter"), "{text}");
    assert!(text.contains("Tritanium"), "{text}");
    assert!(
        r.take_pending().is_empty(),
        "a refused createitem must not push an invalidation"
    );
    assert_eq!(
        r.store().items().len(),
        1,
        "a refused createitem grants nothing; only the capsule remains"
    );
}

/// `shipState` is unpacked with `UNPACK_SEQUENCE 4` before anything is
/// tested, and `None` additionally fires a nested `board` session change
/// into `ship.Board`, which we do not implement.
#[test]
fn the_ship_state_is_a_four_tuple_and_ship_info_is_primed() {
    let mut r = registry();
    let all = r
        .dispatch(&Request {
            service: "dogmaIM",
            method: "GetAllInfo",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("answered");
    let PyValue::Instance { state, .. } = &all else {
        panic!("GetAllInfo must be a KeyVal")
    };

    let ship_state = crate::pyutil::dict_get(state, b"shipState").expect("shipState");
    let PyValue::Tuple(parts) = ship_state else {
        panic!("shipState must be a 4-tuple, got {ship_state:?}")
    };
    assert_eq!(parts.len(), 4);
    // instanceCache, instanceFlagQuantityCache: the client's own
    // ClearInstanceCache sets both to {}.
    assert_eq!(parts[0], PyValue::Dict(vec![]));
    assert_eq!(parts[1], PyValue::Dict(vec![]));
    // wbData: SetWeaponBanks opens `if data is None: return`.
    assert_eq!(parts[2], PyValue::None);

    // shipInfo must prime the pod, or godma.IsItemReady never becomes
    // true and the fitting window raises TypeNotFoundException.
    let ship_info = crate::pyutil::dict_get(state, b"shipInfo").expect("shipInfo");
    let PyValue::Dict(entries) = ship_info else {
        panic!("shipInfo is a {{itemKey: row}} dict, got {ship_info:?}")
    };
    assert_eq!(entries.len(), 1, "one entry for an empty pod");
}

/// Every attribute we send is an integer id against a **float** value.
///
/// Pins the two things that fail far from here and without an error.
/// `GetDisplayAttributes` subscripts `attributesByName[name]` with no
/// guard, so a key that is not an attribute id is a `KeyError` in the
/// info window; and `activeShipController` has no `CO_FUTURE_DIVISION`,
/// so an integer value makes `GetArmorHPPortion` floor-divide -- which
/// reads correct at full health and draws an **empty** ring at 68%
/// armour. The tempting wrong version asserts every value is `> 0.0`:
/// `damage` and `armorDamage` are legitimately zero, and it would not
/// catch an `Int`.
#[test]
fn every_ship_attribute_is_an_id_against_a_float() {
    let PyValue::Dict(entries) = ship_attributes(&crate::content::Content::fixture(), 670) else {
        panic!("attributes is a dict")
    };
    for (value, key) in &entries {
        assert!(
            matches!(key, PyValue::Int(_)),
            "the client calls get_attribute_name(key): {key:?}"
        );
        assert!(
            matches!(value, PyValue::Real(_)),
            "an integer here floor-divides in the client and empties the \
             gauge the moment the ship is damaged: {key:?} -> {value:?}"
        );
    }
}

/// A ship's mass, top speed and agility are served: omitted, the client
/// reads the attribute definition's default (0) and the propulsion module
/// tooltip divides by the mass (`ZeroDivisionError`, 2026-10-28 logs).
#[test]
fn every_hull_serves_a_nonzero_mass() {
    let content = crate::content::Content::fixture();
    for hull in crate::destiny::hull::FIXTURE {
        let PyValue::Dict(e) = ship_attributes(&content, hull.type_id) else {
            panic!("attributes is a dict")
        };
        for id in [4, 37, 70] {
            let v = e
                .iter()
                .find(|(_, k)| *k == PyValue::Int(id))
                .map(|(v, _)| v.clone())
                .unwrap_or_else(|| panic!("attribute {id} missing for {}", hull.type_id));
            assert!(
                matches!(v, PyValue::Real(x) if x > 0.0),
                "attribute {id} of {}: {v:?}",
                hull.type_id
            );
        }
    }
}

/// Every hull draws three full rings, computed the way the client
/// computes them.
///
/// Pins **the ratios the HUD reads**, for the whole table rather than one
/// hull: `shieldCharge/shieldCapacity`, `(armorHP - armorDamage)/armorHP`
/// and `(hp - damage)/hp`. The tempting wrong version asserts the dict's
/// length or its exact key list, which pins our emission rather than the
/// property -- and would have passed just as happily with the integer
/// values that break in combat.
#[test]
fn every_hull_reads_as_three_full_rings() {
    let at = |entries: &[(PyValue, PyValue)], id: i64| -> f64 {
        entries
            .iter()
            .find_map(|(v, k)| match (k, v) {
                (PyValue::Int(i), PyValue::Real(f)) if *i == id => Some(*f),
                _ => None,
            })
            .unwrap_or_else(|| panic!("attribute {id} is missing"))
    };
    let content = crate::content::Content::fixture();
    for hull in crate::destiny::hull::FIXTURE {
        let PyValue::Dict(e) = ship_attributes(&content, hull.type_id) else {
            panic!("attributes is a dict")
        };
        let shield = at(&e, 264) / at(&e, 263);
        let armour = (at(&e, 265) - at(&e, 266)) / at(&e, 265);
        let structure = (at(&e, 9) - at(&e, 3)) / at(&e, 9);
        assert_eq!(
            (shield, armour, structure),
            (1.0, 1.0, 1.0),
            "type {} undocks damaged",
            hull.type_id
        );
    }
}

/// A charged attribute never arrives without the two numbers that build
/// it.
///
/// `godma.UpdateAttribute` creates the charged attribute only when both
/// the tau and the cap are truthy, and a half-built one is what
/// `ApplyAttributeChange` later subscripts into a `KeyError`. Pins that
/// client-side invariant rather than our layout. The tempting wrong
/// version asserts `shieldCharge == shieldCapacity`, which pins today's
/// full-health seed and would fail the first time we model damage --
/// exactly backwards.
#[test]
fn a_charged_attribute_brings_its_rate_and_its_cap() {
    let content = crate::content::Content::fixture();
    for hull in crate::destiny::hull::FIXTURE {
        let PyValue::Dict(e) = ship_attributes(&content, hull.type_id) else {
            panic!("attributes is a dict")
        };
        let has = |id: i64| {
            e.iter().any(
                |(v, k)| matches!((k, v), (PyValue::Int(i), PyValue::Real(f)) if *i == id && *f != 0.0),
            )
        };
        for (state, tau, cap) in [(264, 479, 263), (18, 55, 482)] {
            if has(state) {
                assert!(has(tau) && has(cap), "type {}: {state}", hull.type_id);
            }
        }
    }
}

/// Docking is the undock change with both pairs reversed, and nothing
/// else.
///
/// Pins the **mirror property** rather than the two literals: written as
/// `assert_eq!(change_of("stationid"), Int(60012526))` it would pass just
/// as well with the old and new values swapped, which is the one mistake
/// that matters here -- `IsUndockingSessionChange` and
/// `UpdateBallpark` both read the pair's *sides*, not its contents.
#[test]
fn docking_is_the_undock_change_with_both_pairs_reversed() {
    let mut r = registry();
    let who = actor(&r);
    let out = r.undock_change(who).expect("a docked character can undock");
    let back = r.dock_change(who).expect("and can dock again");
    assert_eq!(out.len(), 2);
    assert_eq!(back.len(), 2, "two keys and no more: {back:?}");

    let pair = |change: &[(PyValue, PyValue)], name: &[u8]| -> Vec<PyValue> {
        let value = change
            .iter()
            .find_map(|(value, key)| match key {
                PyValue::Bytes(k) if k.as_slice() == name => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} is missing", String::from_utf8_lossy(name)));
        let PyValue::Tuple(pair) = value else {
            panic!("a change is an (old, new) pair")
        };
        pair.clone()
    };
    for key in [b"stationid".as_slice(), b"solarsystemid"] {
        let (there, and_back) = (pair(&out, key), pair(&back, key));
        assert_eq!(
            and_back,
            vec![there[1].clone(), there[0].clone()],
            "{} must be the undock pair reversed",
            String::from_utf8_lossy(key)
        );
    }
    // The derived keys, and the one that takes a destructive branch.
    for absent in [b"locationid".as_slice(), b"corprole", b"shipid"] {
        assert!(
            !back
                .iter()
                .any(|(_, k)| matches!(k, PyValue::Bytes(b) if b.as_slice() == absent)),
            "{} must not be sent",
            String::from_utf8_lossy(absent)
        );
    }
}

/// Undocking: the one session change where an **extra key is the bug**.
#[test]
fn the_undock_change_carries_exactly_two_attributes() {
    let mut r = registry();
    let change = r
        .undock_change(actor(&r))
        .expect("a docked character can undock");

    let by_key = |name: &[u8]| {
        change.iter().find_map(|(value, key)| match key {
            PyValue::Bytes(k) if k.as_slice() == name => Some(value),
            _ => None,
        })
    };

    // Exactly two. `locationid` and `corprole` are DERIVED by the client,
    // and sending either would overwrite its own calculation.
    assert_eq!(change.len(), 2, "got {change:?}");
    for absent in [
        b"locationid".as_slice(),
        b"corprole",
        b"shipid",
        b"solarsystemid2",
    ] {
        assert!(
            by_key(absent).is_none(),
            "{} must not be sent",
            String::from_utf8_lossy(absent)
        );
    }

    // `IsUndockingSessionChange` needs a truthy OLD stationid and a
    // truthy NEW solarsystemid, and `RecalculateDependantAttributesWithChanges`
    // needs the new stationid to be None or `locationid` stays on the
    // station -- which would also stop crimewatch's wave firing at all.
    let PyValue::Tuple(station) = by_key(b"stationid").expect("stationid") else {
        panic!("stationid must be a 2-tuple")
    };
    assert!(
        matches!(station[0], PyValue::Int(id) if id != 0),
        "old must be truthy"
    );
    assert_eq!(station[1], PyValue::None, "new stationid must be None");

    let PyValue::Tuple(system) = by_key(b"solarsystemid").expect("solarsystemid") else {
        panic!("solarsystemid must be a 2-tuple")
    };
    assert_eq!(system[0], PyValue::None);
    assert!(
        matches!(system[1], PyValue::Int(id) if id != 0),
        "new must be truthy"
    );
}

/// `/giveisk` moves the number `account.GetCashBalance` serves.
///
/// The point of the test is the *coupling*: a grant that does not show up
/// in the call the wallet actually makes is not a grant. Asserting the
/// field alone would pass with the two wired to different places.
#[test]
fn giveisk_changes_what_the_wallet_asks_for() {
    let mut r = registry();
    let balance = |r: &mut Registry| {
        r.dispatch(&Request {
            service: "account",
            method: "GetCashBalance",
            // The personal wallet, not the corporation's. The first
            // argument is a corp flag, not an id.
            args: &[PyValue::Int(0)],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("GetCashBalance answers")
    };
    let slash = |r: &mut Registry, line: &str| {
        let out = r
            .dispatch(&Request {
                service: "slash",
                method: "SlashCmd",
                args: &[bstr(line)],
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            })
            .expect("SlashCmd answers");
        let PyValue::Bytes(b) = out else {
            panic!("the console stringifies the result; it must be a string")
        };
        String::from_utf8_lossy(&b).into_owned()
    };

    assert_eq!(balance(&mut r), PyValue::Real(0.0));
    slash(&mut r, "/giveisk 1000000");
    assert_eq!(balance(&mut r), PyValue::Real(1_000_000.0));
    // Grants accumulate rather than replace.
    slash(&mut r, "/giveisk 500000");
    assert_eq!(balance(&mut r), PyValue::Real(1_500_000.0));

    // The `/giveisk <character> <amount>` form, which is what the real
    // command takes. There is one character, so the name is accepted and
    // ignored; the amount is the last word either way.
    slash(&mut r, "/giveisk Capsuleer 500000");
    assert_eq!(balance(&mut r), PyValue::Real(2_000_000.0));

    // Thousands separators and a trailing unit, both of which a player
    // typing a large number produces.
    slash(&mut r, "/giveisk 1,000,000isk");
    assert_eq!(balance(&mut r), PyValue::Real(3_000_000.0));
}

/// **`/giveisk` now tells the client, and that is the first thing this
/// server ever says unprompted.**
///
/// The grant used to be invisible by every route: `WalletSvc.Run` calls
/// `GetCashBalance(0)` once and caches it in `_wealth`, so reopening the
/// wallet does not re-ask, and a reconnect builds a fresh `Registry`, so
/// the ISK was gone before anything could have shown it. The answer is a
/// notification, and this test is that it is queued -- exactly one, with
/// the **character** id (`WalletSvc.OnAccountChange` tests `ownerID ==
/// session.charid`) and the balance **after** the grant.
#[test]
fn giveisk_asks_for_one_account_change_carrying_the_new_balance() {
    let mut r = registry();
    let charid = r.config().characters[0].character_id;
    assert!(
        r.take_pending().is_empty(),
        "nothing pending before a grant"
    );

    let _ = r
        .dispatch(&Request {
            service: "slash",
            method: "SlashCmd",
            args: &[bstr("/giveisk 1000000")],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("SlashCmd answers");

    let pending = r.take_pending();
    assert_eq!(pending.len(), 1, "one grant, one notification");
    let (after, n) = &pending[0];
    assert!(
        after.is_zero(),
        "the client must learn in the same flush as the answer, so the \
         console line and the wallet cannot disagree even briefly"
    );
    assert_eq!(
        n.to_push(),
        (
            "OnAccountChange",
            vec![
                PyValue::Bytes(b"cash".to_vec()),
                PyValue::Int(charid),
                PyValue::Real(1_000_000.0),
            ]
        ),
        "the account key is the name 'cash'; an integer there updates \
         nothing and says nothing"
    );

    // Drained, not read: sending it twice would blink the neocom twice and
    // compute a second difference of zero.
    assert!(r.take_pending().is_empty(), "take_pending must drain");
}

/// `/pushtest` queues a notification for the **ticker** to write.
///
/// The only difference from `/giveisk` is which thread writes the frame
/// and when, so this test is about the delay and nothing else: a non-zero
/// `Duration` is what sends the frame down `Wire::post` ->
/// `Wire::flush_due` instead of out with the reply. The balance must be
/// unchanged, so that a wallet which reacts proves the path rather than
/// the arithmetic.
#[test]
fn pushtest_queues_a_delayed_notification_without_moving_any_isk() {
    let mut r = registry();
    let before = r.config().characters[0].balance;
    let charid = r.config().characters[0].character_id;

    let _ = r
        .dispatch(&Request {
            service: "slash",
            method: "SlashCmd",
            args: &[bstr("/pushtest 5")],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("SlashCmd answers");

    let pending = r.take_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].0,
        Duration::from_secs(5),
        "a zero delay would go out in the reply's own flush and prove nothing"
    );
    assert_eq!(
        pending[0].1.to_push(),
        (
            "OnAccountChange",
            vec![
                PyValue::Bytes(b"cash".to_vec()),
                PyValue::Int(charid),
                PyValue::Real(before),
            ]
        )
    );
    assert_eq!(
        r.config().characters[0].balance,
        before,
        "/pushtest must not move ISK"
    );
}

/// The console line must not promise a reopen any more.
///
/// It said "reopen the wallet to see it", which was advice that could not
/// work. A stale apology in a string the player reads is worse than no
/// message, so pin its absence.
#[test]
fn the_giveisk_line_no_longer_tells_the_player_to_reopen_the_wallet() {
    let mut r = registry();
    let out = r
        .dispatch(&Request {
            service: "slash",
            method: "SlashCmd",
            args: &[bstr("/giveisk 1")],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("SlashCmd answers");
    let PyValue::Bytes(b) = out else {
        panic!("the console stringifies the result")
    };
    let line = String::from_utf8_lossy(&b).to_lowercase();
    assert!(
        !line.contains("reopen"),
        "the line still asks the player to reopen something: {line}"
    );
}

/// A bad argument must come back as a sentence, not as a raise.
///
/// `_SlashCmd` has no handler around the call, so raising for a typo
/// would surface as a client-side error dialog rather than a console
/// line. The one call that *must* raise is `/`, and it is the only one.
#[test]
fn a_bad_giveisk_explains_itself_instead_of_raising() {
    let mut r = registry();
    for line in ["/giveisk", "/giveisk plenty", "/giveisk nan,nan"] {
        let out = r.dispatch(&Request {
            service: "slash",
            method: "SlashCmd",
            args: &[bstr(line)],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        });
        assert!(
            matches!(out, Ok(PyValue::Bytes(_))),
            "{line} must answer with a string, got {out:?}"
        );
    }
    // And it credited nothing along the way.
    let b = r
        .dispatch(&Request {
            service: "account",
            method: "GetCashBalance",
            args: &[PyValue::Int(0)],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("answers");
    assert_eq!(b, PyValue::Real(0.0));
}

/// `/` is the one slash call whose correct answer is a raise, and the
/// command list it carries must stay `eval`-able.
///
/// `FetchRemoteCommandList` does `eval(msg.split(': ')[1])` on the reason
/// string, so `': '` must appear exactly once before the payload and the
/// payload must be a Python list literal. Adding a command is the easiest
/// way to break that by accident.
#[test]
fn the_command_list_is_still_evaluable() {
    let mut r = registry();
    let err = r
        .dispatch(&Request {
            service: "slash",
            method: "SlashCmd",
            args: &[bstr("/")],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect_err("/ must raise");
    let ServiceError::Raise(v) = err else {
        panic!("/ must raise a UserError, got {err:?}")
    };

    // Walk the exception for its byte strings. Reading it through
    // `{:?}` does not work -- `PyValue::Bytes` renders as a numeric
    // array -- and the reason text is what the client parses, so the
    // test has to look at the same thing the client does.
    fn texts(v: &PyValue, out: &mut Vec<String>) {
        match v {
            PyValue::Bytes(b) => out.push(String::from_utf8_lossy(b).into_owned()),
            PyValue::Unicode(u) => out.push(u.clone()),
            PyValue::Tuple(items) | PyValue::List(items) => {
                for i in items {
                    texts(i, out);
                }
            }
            PyValue::Dict(entries) => {
                for (k, val) in entries {
                    texts(k, out);
                    texts(val, out);
                }
            }
            // `user_error` builds an `ObjectEx`; the reason text is
            // inside its header.
            PyValue::ObjectEx {
                header, list, dict, ..
            } => {
                texts(header, out);
                for i in list {
                    texts(i, out);
                }
                for (k, val) in dict {
                    texts(k, out);
                    texts(val, out);
                }
            }
            PyValue::Instance { state, .. } => texts(state, out),
            _ => {}
        }
    }
    let mut found = Vec::new();
    texts(&v, &mut found);

    let reason = found
        .iter()
        .find(|t| t.starts_with("Available commands: "))
        .unwrap_or_else(|| panic!("no reason text in the exception: {found:?}"));

    // `FetchRemoteCommandList` does `eval(msg.split(': ')[1])`, so `': '`
    // must appear exactly once in the whole string -- a second one and
    // the client evals the wrong half.
    assert_eq!(
        reason.matches(": ").count(),
        1,
        "`: ` must appear exactly once in {reason}"
    );
    let payload = reason.split_once(": ").expect("just counted one").1;
    assert!(
        payload.starts_with('[') && payload.ends_with(']'),
        "must eval to a list literal, got {payload}"
    );
    assert!(
        payload.contains("giveisk"),
        "a command that works should be offered for completion: {payload}"
    );
}

/// The agent rowset carries the thirteen epic-arc starter agents, and
/// every row is exactly as long as the header.
///
/// Two separate failure modes, both of which a later edit would
/// reintroduce without noticing. Emptying `lines` puts the Standings tab
/// back to throwing on open; adding or removing a header column without
/// touching the rows makes `Row.__init__` raise `ValueError` on the
/// client, where we cannot see it.
#[test]
fn every_epic_arc_agent_has_a_row_of_the_right_width() {
    let rowset = agent_rowset(None, None, None);
    let PyValue::Instance { state, .. } = &rowset else {
        panic!("the agent rowset must be a Rowset instance: {rowset:?}")
    };
    let header = crate::pyutil::dict_get(state, b"header").expect("a header");
    let lines = crate::pyutil::dict_get(state, b"lines").expect("lines");
    let (PyValue::List(header), PyValue::List(lines)) = (header, lines) else {
        panic!("header and lines are both lists")
    };

    assert_eq!(
        lines.len(),
        13,
        "an empty rowset is an AttributeError on `agent.factionID` in \
         standingsUIUtil, which costs the whole Standings tab on open"
    );
    // With a station, the real agent is appended and nothing else changes.
    let with_station = agent_rowset(None, Some(60_012_526), Some(30_000_001));
    let PyValue::Instance { state, .. } = &with_station else {
        panic!("still a Rowset")
    };
    let PyValue::List(station_lines) = crate::pyutil::dict_get(state, b"lines").expect("lines")
    else {
        panic!("lines is a list")
    };
    assert_eq!(
        station_lines.len(),
        14,
        "the thirteen epic-arc ids plus the one real agent in our station"
    );
    let PyValue::List(agent) = station_lines.last().expect("the real agent") else {
        panic!("a row is a list")
    };
    assert_eq!(agent.len(), header.len(), "ten cells, in header order");
    assert_eq!(agent[0], PyValue::Int(STATION_AGENT_ID));
    // `level` 1 and `agentTypeID` 2 together are what make the agent
    // usable with no standings at all: `eveCfg.CanUseAgent` returns 1
    // immediately for a level-1 non-research agent. Asserted because
    // changing either one silently makes the agent unreachable rather
    // than visibly broken.
    // All three read out of `npccharacters`, not chosen -- but asserted
    // anyway, because `CanUseAgent` short-circuits only for level 1 with
    // `agentType` 2, and the other two agents at this station are level 1
    // type 10 and level 3 type 8. Picking either of those would make the
    // agent unreachable rather than visibly broken.
    assert_eq!(
        agent[1],
        PyValue::Int(STATION_AGENT_TYPE),
        "agentTypeID 2: basic, not research"
    );
    assert_eq!(
        agent[2],
        PyValue::Int(STATION_AGENT_DIVISION),
        "division 24, Security"
    );
    assert_eq!(
        agent[5],
        PyValue::Int(STATION_AGENT_LEVEL),
        "level 1 is what bypasses the standing check"
    );
    assert_eq!(agent[6], PyValue::Int(60_012_526), "stationID: ours");
    assert_eq!(
        agent[7],
        PyValue::Int(30_000_001),
        "solarsystemID, all lowercase in this header"
    );
    for row in lines {
        let PyValue::List(cells) = row else {
            panic!("each line is a list of cells: {row:?}")
        };
        assert_eq!(
            cells.len(),
            header.len(),
            "Row.__init__ raises ValueError unless len(line) == len(header)"
        );
    }
    // The ids are what the client looks up, so they are the one thing in
    // the row that cannot be None.
    assert_eq!(lines.len(), EPIC_ARC_STARTER_AGENTS.len());
}

/// What the client asks for in space, and the one answer that must agree
/// with the session.
#[test]
fn the_client_can_settle_into_space() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // Two elements by UNPACK_SEQUENCE, and the first must be *our* solar
    // system. A mismatch is not an error -- it silently doubles the call
    // and leaves the client's cache at its UNKNOWN sentinel forever --
    // which is why this asserts the id rather than just the arity.
    let snap = call("insurgencySolarsystem", "GetLocalCampaignClientSnapshot");
    assert_eq!(
        snap,
        PyValue::Tuple(vec![PyValue::Int(30_000_001), PyValue::None]),
        "the first element must equal session.solarsystemid2"
    );

    // The return is discarded by `sensorSuiteService.OnEnterSpace`, so
    // `None` is the answer and not a stub.
    assert_eq!(call("scanMgr", "SignalTrackerRegister"), PyValue::None);

    // Pre-empted, not fixed: nothing reaches these until we push a state
    // with balls in it. The guard is `is not None` and the assignment
    // completes, so a `None` would be stored and then iterated every
    // 500 ms by `activeitem.UpdateAll` while anything is selected.
    assert_eq!(
        call("bountyProxy", "GetMyKillRights"),
        PyValue::List(vec![])
    );
    assert_eq!(
        call("bountyProxy", "GetBountiesAndKillRights"),
        PyValue::Tuple(vec![PyValue::List(vec![]), PyValue::List(vec![])])
    );
}

/// The map's two answers, which are lines 111 and 116 of one client
/// function and therefore have to exist together.
#[test]
fn the_map_filter_can_build_its_data() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // Iterated at line 112, so a list.
    assert_eq!(
        call("stationSvc", "GetAllianceSystems"),
        PyValue::List(vec![])
    );
    // `.itervalues()` at line 117, so a dict -- a list would survive the
    // outer loop by accident and still be the wrong type. Answering only
    // the one above moves the same failure four lines down, which is why
    // both are asserted in one test rather than two. CP18: a dict of dicts
    // with every warzone key, never `{}` (R-FW §0 item 2).
    let states = call("fwWarzoneSolarsystem", "GetAllWarzonesOccupationStates");
    let kv = states.dict_as_kv().expect("a dict");
    assert!(!kv.is_empty(), "{{}} is a KeyError at mapData:59");
    assert!(kv.iter().all(|(_, v)| matches!(v, PyValue::Dict(_))));

    // Search: two methods, two different empties. `Query` is the one
    // where `None` throws, because `_SortResults` iterates it outside the
    // `if resultsByType:` guard.
    assert_eq!(call("search", "QuickQuery"), PyValue::List(vec![]));
    assert_eq!(call("search", "Query"), PyValue::Dict(vec![]));
}

/// The bio survives a round trip, which is the only part of it the player
/// can see.
#[test]
fn the_bio_is_still_there_when_the_window_is_reopened() {
    let mut r = registry();
    let set = |r: &mut Registry, text: &str| {
        r.dispatch(&Request {
            service: "charMgr",
            method: "SetCharacterDescription",
            args: &[PyValue::Unicode(text.to_owned())],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("the setter answers")
    };
    let get = |r: &mut Registry| {
        r.dispatch(&Request {
            service: "charMgr",
            method: "GetCharacterDescription",
            args: &[PyValue::Int(90_000_001)],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("the getter answers")
    };

    // A string, not a row and not a 1-tuple: it goes straight to
    // `edit.SetValue`.
    assert_eq!(get(&mut r), PyValue::Unicode(String::new()));
    assert_eq!(set(&mut r, "hello"), PyValue::None);
    assert_eq!(get(&mut r), PyValue::Unicode("hello".to_owned()));
}

/// Anything the client formats as a time has to arrive as a Python
/// `long`, and the only lever we have on that is magnitude.
#[test]
fn times_the_client_formats_are_long_sized() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // Our encoder picks the integer tag by magnitude, so a value below
    // 2^31 arrives as an `int` and `FormatDateTime`'s `isinstance(value,
    // long)` sends it to a branch that logs and returns None -- a blank
    // field with no exception anywhere. Assert the magnitude, because
    // that is the actual requirement.
    let omega = call("subscriptionMgr", "GetSubscriptionTime");
    // `PyValue::Long` pins the long tag (0x2F) whatever the magnitude; the
    // magnitude assertion stays as a guard on the value itself.
    let PyValue::Long(t) = omega else {
        panic!("a FILETIME Long, got {omega:?}")
    };
    assert!(
        t > i64::from(i32::MAX),
        "{t} is int-sized; a real FILETIME is far past 2^31"
    );
    let mut w = eve_marshal::Writer::new();
    eve_marshal::save(&PyValue::Long(t), &mut w);
    assert_eq!(w.into_bytes()[..2], [0x2F, 8], "marshals with the long tag");

    // Same requirement for `nextTimedRespec`: a FILETIME magnitude. Whether it
    // is past is the remap's business (`skill_queue` tests).
    let respec = call("skillMgr2::skillHandler", "GetRespecInfo");
    let Some(PyValue::Int(next) | PyValue::Long(next)) =
        crate::pyutil::dict_get(&respec, b"nextTimedRespec")
    else {
        panic!("nextTimedRespec is a FILETIME integer")
    };
    assert!(*next > i64::from(i32::MAX));
}

/// The contracts window's second layer: a live lock with no traceback,
/// and the envelope the list panel unpacks.
#[test]
fn the_contracts_window_can_fetch_and_count() {
    fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        let PyValue::Instance { state, .. } = v else {
            return None;
        };
        crate::pyutil::dict_get(state, name.as_bytes())
    }
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // The guard is `is None`, so any non-None answer breaks the loop --
    // which means all the risk is in the field names and none of it is in
    // the values. Four names, each read by arithmetic that happens
    // *outside* the wizard's swallowing `try`.
    let counts = call("contractProxy", "NumOutstandingContracts");
    assert_ne!(
        counts,
        PyValue::None,
        "a None here is stored by a cache guarded on `is None`, which \
         poisons it permanently and re-issues on every read"
    );
    for name in [
        "myCharTotal",
        "nonCorpForMyChar",
        "myCorpTotal",
        "nonCorpForMyCorp",
    ] {
        assert_eq!(
            attr(&counts, name),
            Some(&PyValue::Int(0)),
            "{name} must be a real number: the wizard does \
             `maxNumContracts - currentNum` on it outside its try block"
        );
    }

    // The envelope, for all three list methods. `contracts` must be a
    // **list** and not a dict: the client subscripts it `[0]` and `[-1]`
    // during pagination, which a dict survives in three other places and
    // fails there.
    for method in [
        "GetContractListForOwner",
        "GetMyExpiredContractList",
        "GetMyBids",
    ] {
        let page = call("contractProxy", method);
        assert_eq!(
            attr(&page, "contracts"),
            Some(&PyValue::List(vec![])),
            "{method}: contracts is indexed [0] and [-1], so not a dict"
        );
        assert_eq!(
            attr(&page, "items"),
            Some(&PyValue::Dict(vec![])),
            "{method}: items is `.get(contractID, [])`, so a dict"
        );
    }
}

/// The three windows a player opened that threw, and the one answer
/// behind three of the tracebacks.
#[test]
fn the_character_sheet_contracts_and_map_can_open() {
    /// One attribute off a `KeyVal`.
    ///
    /// A `KeyVal` is an old-style instance whose state *is* its attribute
    /// dict, so reading one means unwrapping the instance first --
    /// `dict_get` on the `KeyVal` itself silently finds nothing, which
    /// would make every assertion below pass vacuously.
    fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        let PyValue::Instance { state, .. } = v else {
            return None;
        };
        crate::pyutil::dict_get(state, name.as_bytes())
    }

    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // `GetCloneState` is read for exactly three attributes, and a KeyVal
    // short of any one fails one line later with the same traceback. The
    // assert is on the attribute names rather than on the whole value,
    // because the values are free and the names are not.
    let state = call("jumpCloneSvc", "GetCloneState");
    for name in ["clones", "implants", "timeLastJump"] {
        assert!(
            attr(&state, name).is_some(),
            "GetCloneState must carry {name}; missing one poisons the \
             whole character sheet, not just the clones tab"
        );
    }
    // `clones` must be falsy but **not** `None`: `GetCloneState` caches
    // on `if self.jumpClones is None`, so a `None` here raises before the
    // assignment, the cache stays unset, and the next read re-issues the
    // call. That is the live lock, not merely a traceback.
    assert_ne!(
        attr(&state, "clones"),
        Some(&PyValue::None),
        "a None here is a live lock, not a missing value"
    );

    // The map. A 3-tuple, and the arity is the whole requirement:
    // `jumpBridgesGates, hasAccessTo, hasNoAccessTo = ...`. Element 2 is
    // never read by anything in the client tree -- it exists so the
    // unpack succeeds.
    let bridges = call("structureDirectory", "GetJumpBridgesWithMyAccess");
    let PyValue::Tuple(parts) = &bridges else {
        panic!("the client unpacks this; it must be a sequence: {bridges:?}")
    };
    assert_eq!(parts.len(), 3, "UNPACK_SEQUENCE 3, no more and no fewer");

    // Contracts. Ten fields, three of which are read arithmetically --
    // `int - None` raises -- and one of which costs a modal dialog at the
    // player if it is None rather than an empty list.
    let page = call("contractProxy", "CollectMyPageInfo");
    for name in [
        "numOutstandingContracts",
        "numOutstandingContractsNonCorp",
        "numOutstandingContractsForCorp",
        "outstandingContracts",
        "numRequiresAttention",
        "numRequiresAttentionCorp",
        "numBiddingOn",
        "numBiddingOnCorp",
        "numInProgress",
        "numInProgressCorp",
    ] {
        assert!(
            attr(&page, name).is_some(),
            "CollectMyPageInfo must carry {name}"
        );
    }
    assert_eq!(
        attr(&page, "outstandingContracts"),
        Some(&PyValue::List(vec![])),
        "None here is not a crash but a modal dialog on every open"
    );
}

/// The mail service's two early calls, neither of which has a window
/// behind it.
///
/// `mailingListsSvc.Initialize` stores `GetJoinedLists()` unexamined, so
/// the shape is not checked until the player opens the mail window or
/// sends to a list -- six client call sites later, in a different file.
/// The type is therefore asserted here rather than left to a playtest.
#[test]
fn the_mail_service_can_start_before_any_window_opens() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // A real dict, because the client does `in`, subscript, insert and
    // delete on it. `List` would satisfy `in` and nothing else, which is
    // the trap: it would pass a smoke test and fail on first use.
    assert_eq!(
        call("mailingListsMgr", "GetJoinedLists"),
        PyValue::Dict(vec![])
    );

    // The return is discarded at the only call site; `None` is the
    // answer. Asserted so that implementing persistence later does not
    // quietly start returning something the client would cache.
    assert_eq!(call("charMgr", "SaveCharacterSetting"), PyValue::None);
    assert_eq!(call("charMgr", "DeleteCharacterSetting"), PyValue::None);
}

/// The three answers the space view will not open without.
#[test]
fn the_space_view_can_start_its_dependencies() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // Iterated by `godma.RefreshTargets`, which `targetMgr.Run` calls
    // SYNCHRONOUSLY once we are in space -- and `targetMgr` is one of the
    // 29 services `_OpenView` starts outside any try block. A raise here
    // means the space view never opens at all.
    assert_eq!(call("dogmaIM", "GetTargets"), PyValue::List(vec![]));
    assert_eq!(call("dogmaIM", "GetTargeters"), PyValue::List(vec![]));

    // Handed straight to native `destiny.Ballpark.LoadFormations`. The
    // only answer in this file whose consumer is in blue.dll rather than
    // in Python, and therefore the only one where an empty **tuple** and
    // an empty **list** are not interchangeable. Answering the list cost
    // a run: `TypeError: must be tuple, not list` inside `AddBallpark`,
    // which the player experienced as a black screen after undocking.
    // The assert is on the variant, so changing it back fails here.
    assert_eq!(call("beyonce", "GetFormations"), PyValue::Tuple(vec![]));

    // Discarded by its caller; the session change is the real answer.
    assert_eq!(call("ship", "Undock"), PyValue::None);
}

/// The skill wave, and the pair that must never ship apart.
#[test]
fn the_character_sheet_can_total_its_skill_points() {
    let mut r = registry();
    let mut call = |method: &str| {
        r.dispatch(&Request {
            service: "skillMgr2::skillHandler",
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("skillMgr2::skillHandler.{method}: {e}"))
    };

    // `GetTotalSkillPointsForCharacter` is
    //     sum(... GetSkillsIncludingLapsed().iteritems()) + GetFreeSkillPoints()
    // over two separate `is None` caches. Answering only the first moves
    // the traceback one line, to `int + NoneType` -- and by then the
    // character sheet has installed a 2-second AutoTimer, which turns the
    // still-empty second cache into a live lock. They go together.
    // **This used to assert an empty dict, and that was the point of the
    // test when the pair's only job was not to raise.** It now carries
    // the starter set, so the assertion moved from "empty" to "non-empty
    // and the same shape from both reads" -- which is what the pair being
    // inseparable actually means.
    let all = call("GetAllSkills");
    let PyValue::Dict(entries) = &all else {
        panic!("GetAllSkills must be a dict keyed by type id")
    };
    assert!(
        !entries.is_empty(),
        "a character sheet showing no skills is indistinguishable from \
         the broken one the owner reported"
    );
    // `GetSkills` and `GetAllSkills` differ only once lapsed skills
    // exist, which needs clone grades. Identical today, and asserted so
    // that the day they diverge is a deliberate change.
    assert_eq!(call("GetSkills"), all);
    assert_eq!(call("GetFreeSkillPoints"), PyValue::Int(0));

    // All five keys, subscripted directly -- `{}` is a KeyError -- and
    // non-zero, because every training estimate divides by them.
    let PyValue::Dict(attrs) = call("GetAttributes") else {
        panic!("GetAttributes must be a dict")
    };
    assert_eq!(attrs.len(), 5);
    for (value, key) in &attrs {
        assert!(
            matches!(key, PyValue::Int(id)
                if (ATTR_CHARISMA..=ATTR_WILLPOWER).contains(id)),
            "unexpected attribute key {key:?}"
        );
        assert!(
            matches!(value, PyValue::Real(v) if *v > 0.0),
            "attribute values must be non-zero: training estimates divide by them"
        );
    }

    // Both keys present, because the Attributes panel subscripts them
    // by name. `{}` is a KeyError and `None` is a TypeError.
    let respec = call("GetRespecInfo");
    let PyValue::Dict(entries) = &respec else {
        panic!("GetRespecInfo must be a dict, got {respec:?}")
    };
    let keys: Vec<&[u8]> = entries
        .iter()
        .filter_map(|(_value, key)| match key {
            PyValue::Bytes(k) => Some(k.as_slice()),
            _ => None,
        })
        .collect();
    assert!(keys.contains(&b"nextTimedRespec".as_slice()));
    assert!(keys.contains(&b"freeRespecs".as_slice()));
}

/// The answers that must not be `None` because `None` would **loop**.
///
/// A guard test for a failure mode with a symptom unlike any other one in
/// this file: not a traceback, not silence, but dropped frames.
#[test]
fn the_answers_that_would_live_lock_the_client() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // The caller is
    //     if self._cache is None:
    //         self._cache = {row[0]: row[1] for row in remote()}
    // so None throws inside the comprehension, the assignment never
    // happens, and the `is None` guard sends the next read back out to
    // us. Opening the wallet made that a tight loop.
    for method in [
        "GetAllMyCharacterWalletLPBalances",
        "GetAllMyCorporationWalletLPBalances",
    ] {
        assert_eq!(
            call("LPSvc", method),
            PyValue::List(vec![]),
            "LPSvc.{method} must be an iterable of (issuerCorpID, balance) pairs; \
             None throws in the comprehension AND leaves the cache empty, which loops"
        );
    }
}

/// The market ticker, whose argument is a `set` and therefore not a
/// container on the wire.
#[test]
fn the_price_history_set_argument_is_unwrapped_and_every_id_answered() {
    // The shape `evecap` rendered: tag 0x22, `__builtin__.set`, with the
    // ids one level down inside the argument tuple. Read `args[0]` as a
    // sequence and you find nothing at all.
    let wrapped = PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::Call,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Global(b"__builtin__.set".to_vec()),
            PyValue::Tuple(vec![PyValue::List(vec![
                PyValue::Int(16_272),
                PyValue::Int(33_681),
                PyValue::Int(34),
            ])]),
        ])),
        list: Vec::new(),
        dict: Vec::new(),
    };
    assert_eq!(type_ids_of(Some(&wrapped)), vec![16_272, 33_681, 34]);

    let mut r = registry();
    let answer = r
        .dispatch(&Request {
            service: "marketProxy",
            method: "GetHistoryForManyTypeIDs",
            args: &[wrapped],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("answered");
    let PyValue::Dict(entries) = &answer else {
        panic!("must be a dict, got {answer:?}")
    };
    // One entry per id ASKED FOR, not per id we have data for. A missing
    // key falls through to `GetPriceHistory(typeID)`, which is two
    // further remote calls each -- so a partial answer to the ticker's
    // 31 types costs 62 extra round trips rather than leaving a gap.
    assert_eq!(entries.len(), 3);
    // Each value is `(old, new)`, and `new.header` is handed to
    // `dbutil.RowList`, which rejects anything that is not a real
    // DBRowDescriptor. That is what rules out an old-style Rowset here.
    let (value, _key) = &entries[0];
    let PyValue::Tuple(pair) = value else {
        panic!("each entry must be an (old, new) pair, got {value:?}")
    };
    assert_eq!(pair.len(), 2);
}

/// The character sheet, which failed on a call we were *answering*.
///
/// `charMgr.GetPublicInfo` never appeared on the unanswered list -- we
/// replied `None` to it, deliberately, because the only caller we knew
/// about discarded the result. The character sheet is the second caller,
/// and it threads the value through five frames without re-testing it.
#[test]
fn the_character_sheet_can_read_its_own_birth_date() {
    let mut r = registry();
    let info = r
        .dispatch(&Request {
            service: "charMgr",
            method: "GetPublicInfo",
            args: &[PyValue::Int(90_000_001)],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .expect("answered");

    let PyValue::Instance { state, .. } = &info else {
        panic!("GetPublicInfo must be a KeyVal, got {info:?}")
    };

    // The attribute the window actually died on.
    let born = crate::pyutil::dict_get(state, b"createDateTime").expect("createDateTime");
    let PyValue::Int(ticks) = born else {
        panic!("createDateTime must be an integer, got {born:?}")
    };
    // It must exceed 2^31, or our encoder emits INT32, the client
    // unmarshals a Python 2 `int` rather than a `long`, and
    // `FormatDateTime`'s `isinstance(value, long)` test takes the `else`
    // branch -- logging a traceback and returning None. A blank birth
    // date with no exception, which is a worse failure than the one we
    // just fixed.
    assert!(
        *ticks > i64::from(i32::MAX),
        "createDateTime must be a real FILETIME so it marshals as a long, got {ticks}"
    );

    for attr in [
        b"characterID".as_slice(),
        b"corporationID",
        b"raceID",
        b"bloodlineID",
        b"gender",
    ] {
        assert!(
            crate::pyutil::dict_get(state, attr).is_some(),
            "GetPublicInfo is missing {}",
            String::from_utf8_lossy(attr)
        );
    }

    // `schoolID` is read unguarded by the home-station panel, and
    // `characterdata.schools.get_school` is a bare dict subscript over
    // `schools.fsdbinary`, so the id must be one of that table's 23 keys in
    // build 3619879 [V] -- an invented id is a KeyError, not a wrong label.
    let school = match crate::pyutil::dict_get(state, b"schoolID") {
        Some(PyValue::Int(id)) => *id,
        other => panic!("schoolID must be an int: {other:?}"),
    };
    assert!(
        (11..=25).contains(&school) || (31..=38).contains(&school),
        "schoolID {school} is not a key of schools.fsdbinary"
    );
}

/// Two answers that `None` got wrong without ever throwing.
///
/// Both were on the unanswered list for runs at a time with nothing in
/// any log to show for it, and both are worth a guard test because the
/// symptom of a regression is silence.
#[test]
fn the_two_answers_that_fail_without_throwing() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // A float, and specifically not an integer and not None.
    //
    // `None <= -4.5` is True in Python 2, so answering None did not
    // fail -- it told the client this character is a -4.5 outlaw.
    assert_eq!(
        call("crimewatch", "GetMySecurityStatus"),
        PyValue::Real(0.0),
        "must be a float: it is formatted with decimalPlaces=1 and \
         written into a dogma attribute"
    );

    // The store's answer must be TRUTHY. Every falsy value latches
    // `VgsClient.available = False` and kills the store for the session.
    for method in ["get_offers", "get_products", "get_categories"] {
        let answer = call("storeManager", method);
        let PyValue::List(items) = &answer else {
            panic!("storeManager.{method} must be a list, got {answer:?}")
        };
        assert!(
            !items.is_empty(),
            "storeManager.{method} must be non-empty: `if not offers` latches \
             self.available = False and every later store call then raises \
             StoreUnavailableError without touching the wire"
        );
    }

    // ... and `get_offers` must then filter itself to nothing, so that a
    // truthy answer does not become an invented product.
    let PyValue::List(offers) = call("storeManager", "get_offers") else {
        panic!("a list")
    };
    let products = crate::pyutil::dict_get(&offers[0], b"products").expect("products key");
    assert_eq!(
        products,
        &PyValue::List(vec![]),
        "the one offer must carry no products, so `len(json['products'])` \
         drops it from the comprehension"
    );
}

/// The fitting window's own wave, and the one call whose argument
/// arrives as a keyword.
///
/// Every shape here was read off a traceback the client mailed us rather
/// than guessed: `.itervalues()` on the fittings, `len()` on the same
/// value one frame later, and `for n in notifications` with `fromID` in
/// the kwargs.
#[test]
fn the_fitting_window_wave_is_answered() {
    let mut r = registry();
    let mut call = |service: &str, method: &str, kwargs: &[(PyValue, PyValue)]| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs,
            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // A dict, because the client MUTATES what we send -- it does
    // `del fittings[id]` and `fittings[id] = fitting` on the very object
    // stored by PrimeFittings. No rowset family can take either.
    for (service, method) in [
        ("charFittingMgr", "GetFittings"),
        ("corpFittingMgr", "GetFittings"),
        ("corpFittingMgr", "GetCommunityFittings"),
    ] {
        assert_eq!(
            call(service, method, &[]),
            PyValue::Dict(vec![]),
            "{service}.{method} must be a dict: `.itervalues()` and `len()` are both called on it"
        );
    }

    // A list. `[]` is not merely tolerated, it caches: the client's test
    // is `if self.allNotifications is None`, so None would re-fetch on
    // every read while [] settles.
    //
    // The argument is a KWARG. Our UNANSWERED line called this
    // `GetAllNotifications/0` and the zero was accurate -- the positional
    // count really is zero.
    let from_id = [(PyValue::Bytes(b"fromID".to_vec()), PyValue::Int(0))];
    assert_eq!(
        call("notificationMgr", "GetAllNotifications", &from_id),
        PyValue::List(vec![])
    );

    // Fire-and-forget: the caller discards it, so None is the answer and
    // not a placeholder.
    assert_eq!(
        call(
            "monolithTaskTrackerMgr",
            "UpdateClientAchievmentsAndCounters",
            &[]
        ),
        PyValue::None
    );

    // A 3-tuple, unpacked with no guard in front of it, whose THIRD
    // element has `.iteritems()` called on it. `([], [], [])` would pass
    // the unpack and then fail one line later -- the §4 third-row trap
    // with an extra step.
    let fighters = call("fighterMgr", "GetFightersForShip", &[]);
    let PyValue::Tuple(parts) = &fighters else {
        panic!("must be a 3-tuple, got {fighters:?}")
    };
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], PyValue::List(vec![]));
    assert_eq!(parts[1], PyValue::List(vec![]));
    assert_eq!(
        parts[2],
        PyValue::Dict(vec![]),
        "abilityStates has .iteritems() called on it"
    );

    // A dict, keyed by packed slot index. Empty keeps the cosmetics path
    // off the protobuf license gateway entirely.
    assert_eq!(
        call("shipCosmeticsMgr", "GetEnabledCosmetics", &[]),
        PyValue::Dict(vec![])
    );
}

/// The `OnClientReady` wave, answered in the shapes its callers demand.
///
/// Three of these are the "empty is not one answer" table in miniature:
/// a list that must be a list (`.extend` is called on it), a dict that
/// must be a dict (`.iteritems()`), and a 3-tuple that is splatted into
/// a 3-parameter handler before anything is tested.
#[test]
fn the_on_client_ready_wave_is_answered() {
    let mut r = registry();
    let mut call = |service: &str, method: &str| {
        r.dispatch(&Request {
            service,
            method,
            args: &[],
            kwargs: &[],

            bind_params: None,
            actor: ACTOR,
        })
        .unwrap_or_else(|e| panic!("{service}.{method}: {e}"))
    };

    // Iterated only; a plain list, not a rowset.
    assert_eq!(
        call("skillMgr2::skillHandler", "GetSkillHistory"),
        PyValue::List(vec![])
    );
    // `.iteritems()` AND `.itervalues()` -- a list is an AttributeError
    // here just as None is.
    assert_eq!(
        call("skillMgr2::skillHandler", "GetBoosters"),
        PyValue::Dict(vec![])
    );
    // Discarded by its only caller; None is the honest answer.
    assert_eq!(
        call("skillMgr2::skillHandler", "CheckAndSendNotifications"),
        PyValue::None
    );

    // Splatted into OnJumpTimersUpdated(jumpActivation, jumpFatigue,
    // lastUpdated). Zeros rather than Nones, because one consumer does
    // `max(0, jumpFatigueExpiryTime - now)` with no guard and Python 2
    // cannot subtract an int from None.
    let timers = call("jumpTimers", "GetTimers");
    let PyValue::Tuple(parts) = &timers else {
        panic!("must be a sequence to splat, got {timers:?}")
    };
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|p| *p == PyValue::Int(0)));

    // Both of these are accessed by attribute name, so an object --
    // never a dict, list or tuple.
    for (service, method, attrs) in [
        (
            "contractProxy",
            "GetLoginInfo",
            ["assignedToMe", "needsAttention", "inProgress"],
        ),
        ("mailMgr", "SyncMail", ["newMail", "oldMail", "mailStatus"]),
    ] {
        let got = call(service, method);
        let have = keyval_attributes(&got);
        for attr in attrs {
            assert!(
                have.iter().any(|a| a == attr),
                "{service}.{method} is missing {attr}"
            );
        }
    }

    // `incoming = mailbox.newMail` then `incoming.extend(...)`, so this
    // one specifically has to be a list, and `oldMail` is None so that
    // stays true even if `newMail` ever becomes a rowset.
    let mailbox = call("mailMgr", "SyncMail");
    let PyValue::Instance { state, .. } = &mailbox else {
        panic!("mailbox must be an instance")
    };
    assert_eq!(
        crate::pyutil::dict_get(state, b"newMail"),
        Some(&PyValue::List(vec![])),
        "newMail has .extend called on it"
    );
    assert_eq!(
        crate::pyutil::dict_get(state, b"oldMail"),
        Some(&PyValue::None)
    );
}

/// The two rowset families are not interchangeable, in *both* directions.
///
/// `agentMgr.GetAgents` must be a `Rowset` because `CRowset` has no
/// `Clone()`. `onlineStatus.GetInitialState` must be a `CRowset` because
/// the client later calls `blue.DBRow(self.onlineStatus.header, ...)`,
/// and `blue.DBRow` needs a real `DBRowDescriptor` -- which a `Rowset`'s
/// plain list-of-names header is not.
#[test]
fn the_online_status_is_a_crowset_because_its_header_becomes_a_dbrow() {
    let got = registry()
        .dispatch(&Request {
            service: "onlineStatus",
            method: "GetInitialState",
            args: &[],
            kwargs: &[],

            bind_params: None,
            actor: ACTOR,
        })
        .expect("answered");
    // A CRowset serialises as an ObjectEx, never as an old-style instance.
    assert!(
        !matches!(got, PyValue::Instance { .. }),
        "a Rowset instance here would break blue.DBRow(header, ...)"
    );
    // `.Index('contactID')` resolves its column up front, and `.online`
    // is read off every row, so both names must be declared.
    let mut names: Vec<Vec<u8>> = Vec::new();
    collect_bytes(&got, &mut names);
    for column in ["contactID", "online"] {
        assert!(
            names.iter().any(|n| n == column.as_bytes()),
            "header is missing {column}"
        );
    }
}

/// The client's own error handler and two of its lookups crashed on `None`
/// (2026-10-28 client logs): `map.GetFacWarZoneInfo` is read as
/// `.systemUpgradeLevel.get(id)`, `alert.SendClientStackTraceAlert` is stored
/// and then unpacked as `(errorID, logMode)`, and the gatekeeper tests
/// `cohortID in <answer>`.
#[test]
fn the_calls_the_client_dereferences_are_not_none() {
    let call = |service: &str, method: &str, args: &[PyValue]| {
        registry()
            .dispatch(&Request {
                service,
                method,
                args,
                kwargs: &[],
                bind_params: None,
                actor: ACTOR,
            })
            .expect("answered")
    };

    let zone = call("map", "GetFacWarZoneInfo", &[PyValue::Int(7_001)]);
    let attrs = keyval_attributes(&zone);
    assert!(attrs.iter().any(|a| a == "systemUpgradeLevel"));
    let PyValue::Instance { state, .. } = &zone else {
        panic!("a KeyVal")
    };
    let PyValue::Dict(entries) = state.as_ref() else {
        panic!("a dict state")
    };
    let level = entries
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == b"systemUpgradeLevel"))
        .map(|(v, _)| v)
        .expect("the attribute");
    assert_eq!(level, &PyValue::Dict(vec![]));

    let stack_id = PyValue::Tuple(vec![
        PyValue::Int(-1_234),
        PyValue::Bytes(b"trace".to_vec()),
    ]);
    let got = call(
        "alert",
        "SendClientStackTraceAlert",
        &[
            stack_id,
            PyValue::Bytes(b"text".to_vec()),
            PyValue::Bytes(b"Error".to_vec()),
        ],
    );
    let PyValue::Tuple(pair) = got else {
        panic!("unpacked as (errorID, logMode)")
    };
    assert_eq!(pair.len(), 2);
    // `errorID is None` sends the client's tasklet into a 30 x 5 s wait.
    assert_eq!(pair[0], PyValue::Int(1_234));
    // `LOGGING_BATCHED`.
    assert_eq!(pair[1], PyValue::Bytes(b"B".to_vec()));

    // `changes, flooded = GetLogModeForError(...)`: a pair, not an error.
    assert_eq!(
        call("alert", "GetLogModeForError", &[PyValue::Dict(vec![])]),
        PyValue::Tuple(vec![PyValue::Dict(vec![]), PyValue::List(vec![])])
    );
    assert_eq!(
        call("alert", "BeanDelivery", &[PyValue::Dict(vec![])]),
        PyValue::None
    );
    assert_eq!(
        call("charUnboundMgr", "GetCohortsForUser", &[]),
        PyValue::List(vec![])
    );
    assert_eq!(
        call("charMgr", "GetCohortsForCharacter", &[]),
        PyValue::List(vec![])
    );
}

/// The warzone state must echo back *our own* solar system id.
///
/// `fwWarzoneSvc.GetLocalOccupationState` compares the first element
/// against `session.solarsystemid2`; on a mismatch it calls us a second
/// time and then `raise RuntimeError`. So `(None, None)` and `(0, None)`
/// are worse than useless -- they double the traffic and throw anyway --
/// and only the matching id makes the call stop repeating.
#[test]
fn the_warzone_state_echoes_our_own_solar_system_id() {
    let req = Request {
        service: "fwWarzoneSolarsystem",
        method: "GetLocalOccupationState",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Tuple(parts) = &got else {
        panic!("unpacked as a 2-tuple before any test, got {got:?}")
    };
    assert_eq!(parts.len(), 2);
    // The same value the session change carries as `solarsystemid2`.
    assert_eq!(parts[0], PyValue::Int(30_000_001));
    // `IsWarzoneSolarSystem` only does `bool(...)`, so None is the
    // honest "this system is not in a warzone".
    assert_eq!(parts[1], PyValue::None);
}

/// Three calls whose emptiness is a *decision*, not a placeholder.
///
/// `seasonManager` is polymorphic -- `None`, a list, or one season -- and
/// `None` is the author's own "no season running", tested first. An empty
/// list is falsy but `isinstance(x, list)` is true, which would push the
/// client down the season-selection branch instead. The other two are
/// unpacked or subscripted before any test, so they cannot be `None`.
#[test]
fn the_shapes_whose_emptiness_is_a_decision() {
    let call = |service: &str, method: &str| {
        registry()
            .dispatch(&Request {
                service,
                method,
                args: &[],
                kwargs: &[],

                bind_params: None,
                actor: ACTOR,
            })
            .expect("answered")
    };

    assert_eq!(
        call("seasonManager", "get_season_data_for_character"),
        PyValue::None,
        "an empty list would be read as a season *selection*"
    );

    // `output, dbsIsActive = ...`: a float and a bool, active for an actor
    // with a system (CP38, `econ_dbs::answer`).
    let PyValue::Tuple(dbs) = call("dynamicBountyMgr", "GetOutputForClientSolarSystem") else {
        panic!("a tuple")
    };
    assert!(matches!(dbs[..], [PyValue::Real(_), PyValue::Bool(true)]));

    // **The one call a real client made that we did not answer**, counted
    // in every client run until 2026-10-24. `None` is what
    // `GetESSDataForCurrentSystem` returns itself when the ESS feature is
    // flagged offline, so every caller already copes with it. §41.10 has
    // why the alternative -- setting that flag through
    // `machoNet.InitClientGlobalConfig` -- is not available: the client
    // answers "method InitClientGlobalConfig is not exported".
    assert_eq!(call("essMgr", "GetDataForClientSolarSystem"), PyValue::None);

    // `folders, bookmarks, subfolders = ...`, then `len(bookmarks)`.
    let bookmarks = call("accessGroupBookmarkMgr", "GetMyActiveBookmarks");
    let PyValue::Tuple(parts) = &bookmarks else {
        panic!("unpacked as a 3-tuple, got {bookmarks:?}")
    };
    assert_eq!(parts.len(), 3);
    assert!(
        parts
            .iter()
            .all(|p| matches!(p, PyValue::List(l) if l.is_empty()))
    );

    // Empty `folders` makes the client create a default one with
    // `AddFolder`, which is now stored rather than refused; the folder
    // lifecycle is tested in `service/world.rs`.
}

/// `GetSelfInvItem` must be a real `DBRow` naming the station.
///
/// `invCacheContainer._SetItemData` does `self._item.itemID` and
/// `self._item.typeID` with no guard, so `None` is an `AttributeError`
/// that the `itemID` property re-raises as a `RuntimeError` -- which is
/// what aborted `LoadView` for the whole hangar and left a complete
/// docked UI sitting over a black scene.
#[test]
fn the_hangar_inventorys_self_item_is_the_station() {
    let req = Request {
        service: "invbroker::inventory",
        method: "GetSelfInvItem",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::DbRow(row) = &got else {
        panic!("must be a DBRow with .itemID and .typeID, got {got:?}")
    };

    let column = |name: &str| {
        let i = row
            .descriptor()
            .columns()
            .iter()
            .position(|c| c.name == name.as_bytes())
            .unwrap_or_else(|| panic!("no {name} column"));
        row.values()[i].clone()
    };
    // The container is the station: `invCache.GetInventory` routes
    // `containerHangar` to a moniker bound on the station.
    assert_eq!(column("itemID"), DbValue::Int(60_012_526));
    assert_eq!(column("typeID"), DbValue::Int(2502));
    // A station sits in its solar system, not in itself.
    assert_eq!(column("locationID"), DbValue::Int(30_000_001));
    // Verified against inventorycommon/const.pyc, not remembered.
    assert_eq!(column("groupID"), DbValue::Int(15));
    assert_eq!(column("categoryID"), DbValue::Int(3));
}

/// `GetNPCNPCStandings` is the strictest empty-value caller we have met.
///
/// `Standing.__RefreshStandings` calls `.Filter('toID')` and then
/// `.Filter('fromID')` on the result as its very next two statements,
/// with no truthiness test and no `isinstance` escape. All four kinds of
/// empty -- `None`, `{}`, `[]`, and the `(header, data)` prime tuple --
/// are an `AttributeError` here. And `Rowset.Filter` goes on to do
/// `header.index(idName)`, so both key columns have to be *named* even
/// though there are no rows to key.
#[test]
fn the_npc_standings_are_a_rowset_naming_every_column_its_callers_touch() {
    let req = Request {
        service: "standingMgr",
        method: "GetNPCNPCStandings",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Instance { class, .. } = &got else {
        panic!("must be an object with a .Filter method, got {got:?}")
    };
    let class = String::from_utf8_lossy(class).into_owned();
    assert_eq!(class, "eve.common.script.sys.rowset.Rowset");
    assert!(
        eve_rowset::is_whitelisted(class.as_bytes()),
        "{class} is not in the client's globalsWhitelist"
    );

    let attrs = keyval_attributes(&got);
    assert!(attrs.iter().any(|a| a == "header"));
    assert!(attrs.iter().any(|a| a == "lines"));
    // As for the agent rowset: `RowClass` is a class attribute with a
    // default, and sending one would override it.
    assert!(
        !attrs.iter().any(|a| a == "RowClass"),
        "RowClass is a class attribute; sending one would override it"
    );

    let mut names: Vec<Vec<u8>> = Vec::new();
    collect_bytes(&got, &mut names);
    // `fromID` and `toID` are the two Filter keys. `standing` is read as
    // `standing.standing` in `Standing.GetStanding`, through the Row's
    // `__getattr__`, which resolves names against this same header.
    for column in ["fromID", "toID", "standing"] {
        assert!(
            names.iter().any(|n| n == column.as_bytes()),
            "header is missing {column}"
        );
    }
}

/// `GetAgents` must be a `Rowset`, not a `dbutil.CRowset`.
///
/// The caller's first act is `GetAgents().Clone()`, and `CRowset` has no
/// `Clone` — only `Rowset`, `IndexRowset` and `FilterRowset` do. This is
/// one of the few places the two rowset families are not
/// interchangeable, and getting it wrong is an `AttributeError` rather
/// than a shape mismatch, so it is worth pinning.
#[test]
fn the_agent_rowset_is_a_rowset_instance_not_a_crowset() {
    let req = Request {
        service: "agentMgr",
        method: "GetAgents",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Instance { class, .. } = &got else {
        panic!("must be a Rowset instance with methods, got {got:?}")
    };
    let class = String::from_utf8_lossy(class).into_owned();
    assert_eq!(
        class, "eve.common.script.sys.rowset.Rowset",
        "a CRowset has no Clone(), which is the first thing the caller calls"
    );
    assert!(
        eve_rowset::is_whitelisted(class.as_bytes()),
        "{class} is not in the client's globalsWhitelist"
    );
    // `RowClass` has a class-level default and must not be sent, or
    // `Clone()` would carry our value instead of the real row class.
    let attrs = keyval_attributes(&got);
    assert!(attrs.iter().any(|a| a == "header"));
    assert!(attrs.iter().any(|a| a == "lines"));
    assert!(
        !attrs.iter().any(|a| a == "RowClass"),
        "RowClass is a class attribute; sending one would override it"
    );
    // Index and Filter resolve their column up front, so every column
    // they name has to be declared even with no rows.
    let mut names: Vec<Vec<u8>> = Vec::new();
    collect_bytes(&got, &mut names);
    for column in [
        "agentID",
        "stationID",
        "solarsystemID",
        "corporationID",
        "factionID",
        "agentTypeID",
    ] {
        assert!(
            names.iter().any(|n| n == column.as_bytes()),
            "Index/Filter names {column}: {:?}",
            names
                .iter()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect::<Vec<_>>()
        );
    }
}

/// The journal is a two-element sequence of lists.
///
/// The client does `[0]`, `[:1][0]` and `[n].extend(...)` for `n` in
/// `range(2)`. One element survives the first two and breaks the third.
#[test]
fn the_agent_journal_has_both_of_its_lists() {
    let req = Request {
        service: "agentMgr",
        method: "GetMyJournalDetails",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::List(parts) = &got else {
        panic!("must be a sliceable sequence, got {got:?}")
    };
    assert_eq!(
        parts.len(),
        2,
        "[missions, offers]; .extend is called on both"
    );
    for p in parts {
        assert!(matches!(p, PyValue::List(_)), "both are lists: {p:?}");
    }
}

/// The station rowset's columns, spelled exactly.
///
/// `solarSystemID` has a capital S here, while the agent rowset spells
/// the same idea `solarsystemID`. Both are read by name, so the
/// inconsistency is the client's and we have to match it per rowset.
#[test]
fn the_station_info_rowset_names_its_four_columns() {
    let req = Request {
        service: "map",
        method: "GetStationInfo",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let mut names: Vec<Vec<u8>> = Vec::new();
    collect_bytes(&got, &mut names);
    for column in ["stationID", "solarSystemID", "ownerID", "operationID"] {
        assert!(
            names.iter().any(|n| n == column.as_bytes()),
            "{column} is read by name off this rowset"
        );
    }
}

/// The modified-security answer must be a real rowset carrying its
/// columns, because the caller does `.Index('solarSystemID')` on it with
/// no guard. An empty list or dict would be an `AttributeError`, and the
/// `(header, data)` tuple the owner primes use has no `.Index` either.
#[test]
fn the_modified_security_systems_answer_is_a_rowset_with_its_columns_named() {
    let req = Request {
        service: "map",
        method: "GetSecurityModifiedSystems",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::ObjectEx { header, .. } = &got else {
        panic!("GetSecurityModifiedSystems must be a CRowset object, got {got:?}")
    };
    // A CRowset's arguments live in the header tuple --
    // `(Global(class), (descriptor, rows))` -- not in the list-append
    // stream, so both the column names and the rows are reached through
    // the header.
    let PyValue::Tuple(head) = header.as_ref() else {
        panic!("CRowset header must be (class, args), got {header:?}")
    };
    let PyValue::Tuple(args) = &head[1] else {
        panic!("CRowset args must be a tuple, got {:?}", head[1])
    };
    assert_eq!(args.len(), 2, "CRowset(descriptor, rows)");
    assert_eq!(
        args[1],
        PyValue::List(vec![]),
        "no system in this sandbox has a modified security level"
    );

    // Column names are byte strings, so look for them as bytes rather
    // than in a debug rendering, which prints them as numbers.
    let mut names: Vec<Vec<u8>> = Vec::new();
    collect_bytes(&args[0], &mut names);
    for column in ["solarSystemID", "security", "modifiedSecurity"] {
        assert!(
            names.iter().any(|n| n == column.as_bytes()),
            "{column} must be declared even with no rows: {:?}",
            names
                .iter()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .collect::<Vec<_>>()
        );
    }
}

/// Every byte string anywhere in a value, for asserting on column names.
fn collect_bytes(v: &PyValue, out: &mut Vec<Vec<u8>>) {
    match v {
        PyValue::Bytes(b) => out.push(b.clone()),
        PyValue::Tuple(items) | PyValue::List(items) => {
            for i in items {
                collect_bytes(i, out);
            }
        }
        // An instance's attribute names and values both live in its
        // state dict, one level down.
        PyValue::Instance { state, .. } => collect_bytes(state, out),
        PyValue::Dict(entries) => {
            for (v, k) in entries {
                collect_bytes(k, out);
                collect_bytes(v, out);
            }
        }
        // The descriptor is itself an ObjectEx, so the column names are
        // one level deeper than the rowset's own arguments.
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            collect_bytes(header, out);
            for i in list {
                collect_bytes(i, out);
            }
            for (k, v) in dict {
                collect_bytes(k, out);
                collect_bytes(v, out);
            }
        }
        _ => {}
    }
}

/// The seasonal campaign's "nothing is running" answer must still be
/// four elements long, because its caller unpacks before it tests.
/// See the comment on the arm: `UNPACK_SEQUENCE 4` precedes the
/// `campaign_id is not None` check, so `None` would raise `TypeError`
/// inside the tasklet and hang the client just as a missing answer does.
#[test]
fn the_seasonal_campaign_answers_a_four_tuple_when_there_is_no_campaign() {
    let req = Request {
        service: "seasonalLoginCampaignManager",
        method: "get_active_campaign",
        args: &[],
        kwargs: &[],

        bind_params: None,
        actor: ACTOR,
    };
    let got = registry().dispatch(&req).expect("answered");
    let PyValue::Tuple(items) = &got else {
        panic!("get_active_campaign must return a tuple, got {got:?}")
    };
    assert_eq!(items.len(), 4, "the client does UNPACK_SEQUENCE 4 on this");
    assert_eq!(
        items[0],
        PyValue::None,
        "a None campaign id is what makes the client skip the whole campaign"
    );
}

/// A job outlives the process, which is what the `jobs` field's own
/// comment said it did not.
///
/// **The install goes through the arm, not through `Jobs::commit`**, so
/// what is persisted is a job the server really built -- cost, dates,
/// material plan and all -- rather than a struct a test invented. It then
/// goes out through `to_snapshot`, through the real `FileStore` on a real
/// file, back through `apply_snapshot` into a `Registry` that has never
/// seen it, and the assertion is on the whole job.
///
/// This is a layer below `loopback::the_world_survives_the_server_being_dropped`
/// on purpose: that test proves the `Server` calls these two methods
/// around the file, and this one proves the job survives them. Repeating
/// the wire plumbing here would need the blueprint's item id back out of
/// a `KeyVal`, which tests the test and not the world.
#[test]
fn an_industry_job_survives_the_world_file() {
    let dir = std::env::temp_dir().join(format!(
        "eve-jobs-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("a directory to write in");
    let path = dir.join("world.txt");

    let mut reg = Registry::new(SandboxConfig::default());
    let actor = 90_000_001;
    let station = reg
        .character(actor)
        .and_then(|c| c.station_id)
        .expect("docked");
    if let Some(c) = reg.character_mut(actor) {
        c.balance = 1_000_000_000.0;
    }
    reg.skills
        .get_mut(&actor)
        .expect("a skill sheet")
        .grant_in(&reg.content, crate::industry::SKILL_INDUSTRY, 1)
        .expect("Industry is in the content skill table");
    reg.apply_items(&[
        crate::item::Edit::Create {
            type_id: 21_858,
            quantity: 1,
            to: crate::item::Slot::new(actor, station, crate::industry::FLAG_HANGAR),
        },
        crate::item::Edit::Create {
            type_id: 34,
            quantity: 100,
            to: crate::item::Slot::new(actor, station, crate::industry::FLAG_HANGAR),
        },
    ])
    .expect("the fixture's items");
    let blueprint_id = reg
        .store()
        .items()
        .iter()
        .find(|i| i.type_id == 21_858)
        .expect("the blueprint")
        .item_id;
    let location = |item_id: i64| PyValue::ObjectEx {
        form: eve_marshal::ObjectForm::New,
        header: Box::new(PyValue::Tuple(vec![
            PyValue::Tuple(vec![PyValue::Global(b"industry.job.Location".to_vec())]),
            pydict(vec![
                ("itemID", PyValue::Int(item_id)),
                ("flagID", PyValue::Int(crate::industry::FLAG_HANGAR)),
            ]),
        ])),
        list: vec![],
        dict: vec![],
    };
    let request = pydict(vec![
        ("activityID", PyValue::Int(crate::industry::MANUFACTURING)),
        ("blueprintID", PyValue::Int(blueprint_id)),
        ("runs", PyValue::Int(1)),
        ("inputLocation", location(station)),
        ("outputLocation", location(station)),
    ]);
    let answer = reg
        .dispatch(&Request {
            service: "industryManager",
            method: "InstallJob",
            args: &[request],
            kwargs: &[],
            bind_params: None,
            actor: Some(actor),
        })
        .expect("the install");
    let PyValue::Int(job_id) = answer else {
        panic!("InstallJob answers the job id: {answer:?}")
    };
    let before = *reg.jobs().get(job_id).expect("the job we installed");
    let next_before = reg.jobs().next_id();

    let store = crate::store::FileStore::new(&path);
    crate::store::Store::save(&store, &reg.to_snapshot()).expect("write the world");
    let snap = crate::store::Store::load(&store)
        .expect("read the world")
        .expect("a world was written");

    let mut fresh = Registry::new(SandboxConfig::default());
    assert!(
        fresh.jobs().is_empty(),
        "a fresh registry must start with no jobs, or this proves nothing"
    );
    fresh.apply_snapshot(snap);
    assert_eq!(
        fresh.jobs().get(job_id),
        Some(&before),
        "every one of the job's 28 fields has to come back"
    );
    // At or above, not equal: a load leaps every allocator (`store::settle`,
    // C7 §3.4).
    assert!(
        fresh.jobs().next_id() >= next_before,
        "and the allocator, or the next job reuses this one's id and              lands on the client's cached row for it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// W10.5 seam S2 moved eight services out of `service/space.rs`. Each must
/// still route, and to its new module: its name sits in exactly one
/// `DOMAINS` row, that row's list is the new module's `SERVICES`, `space`
/// no longer claims it, and both the new module's own `dispatch` and the
/// registry's reach an arm for it (`NoSuchMethod` for a method that cannot
/// exist, where a name with no arm would answer `NoSuchService`).
#[test]
fn every_moved_service_still_routes() {
    let moved: [(&str, &[&str], DomainDispatch); 8] = [
        ("scanMgr", sites::SERVICES, sites::dispatch),
        ("dungeonInstanceCacheMgr", sites::SERVICES, sites::dispatch),
        ("dungeonExplorationMgr", sites::SERVICES, sites::dispatch),
        ("fwWarzoneSolarsystem", facwar::SERVICES, facwar::dispatch),
        // Moved on again by W16 CP53a, to the enlistment module.
        (
            "fwCharacterEnlistmentMgr",
            facwar_mgr::SERVICES,
            facwar_mgr::dispatch,
        ),
        ("dynamicBountyMgr", bounty::SERVICES, bounty::dispatch),
        ("bountyProxy", bounty::SERVICES, bounty::dispatch),
        ("taleMgr", tales::SERVICES, tales::dispatch),
    ];
    let mut r = registry();
    for (name, services, module_dispatch) in moved {
        let rows: Vec<&[&str]> = DOMAINS
            .iter()
            .map(|(s, _)| *s)
            .filter(|s| s.contains(&name))
            .collect();
        assert_eq!(rows.len(), 1, "{name} must sit in exactly one DOMAINS row");
        assert_eq!(rows[0], services, "{name} routes to the wrong module");
        assert!(
            !space::SERVICES.contains(&name),
            "space still claims {name}"
        );
        let req = Request {
            service: name,
            method: "__no_such_method_exists__",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: None,
        };
        assert!(
            matches!(
                module_dispatch(&mut r, &req),
                Err(ServiceError::NoSuchMethod { .. })
            ),
            "{name} has no arm in its new module"
        );
        assert!(
            matches!(r.dispatch(&req), Err(ServiceError::NoSuchMethod { .. })),
            "{name} does not reach an arm through the registry"
        );
    }
}

/// Every registry has a world record, so a generator never handles `None`
/// (review CA6): a new one runs on the fixture without being dirty, a
/// snapshot with no record keeps it, and a loaded record replaces it. The
/// taint flag starts clear, is sticky, and survives a snapshot (review M5).
#[test]
fn a_new_registry_has_a_world() {
    let mut r = registry();
    assert_eq!(r.world(), &crate::delta::World::FIXTURE);
    assert_eq!(r.deltas().world(), Some(&crate::delta::World::FIXTURE));
    assert!(!r.take_dirty(), "the fixture is not a change to save");

    r.apply_snapshot(crate::store::Snapshot::default());
    assert_eq!(r.world(), &crate::delta::World::FIXTURE);

    let loaded = crate::delta::World {
        seed: crate::delta::World::FIXTURE.seed ^ 1,
        ..crate::delta::World::FIXTURE
    };
    let mut snap = crate::store::Snapshot::default();
    snap.deltas.set_world(loaded);
    r.apply_snapshot(snap);
    assert_eq!(r.world(), &loaded);

    assert!(!r.tainted());
    r.taint();
    r.apply_snapshot(crate::store::Snapshot::default());
    assert!(r.tainted(), "only a restart clears the taint");
    assert_eq!(r.world(), &loaded);
}

/// The item row's shape, pinned by the client's own hard-coded indices
/// (`const.ix*`, `docs/reference/dbrow.md` Gap 4). Invented ids.
mod item_row_shape {
    use super::*;
    use eve_marshal::DbRowError;

    const IX_ITEM_ID: usize = 0;
    const IX_FLAG: usize = 4;
    const IX_QUANTITY: usize = 5;
    const IX_CUSTOM_INFO: usize = 8;
    const IX_STACK_SIZE: usize = 9;
    const IX_SINGLETON: usize = 10;

    fn row(values: Vec<DbValue>) -> Result<DbRow, DbRowError> {
        DbRow::new(item_row_descriptor(), values)
    }

    fn eleven() -> Vec<DbValue> {
        vec![
            DbValue::Int(7_000_000_001),
            DbValue::Int(9001),
            DbValue::Int(8_000_001),
            DbValue::Int(7_000_000_002),
            DbValue::Int(27),
            DbValue::Int(20),
            DbValue::Int(9002),
            DbValue::Int(9003),
            DbValue::Null,
            DbValue::Int(20),
            DbValue::Int(0),
        ]
    }

    #[test]
    fn names_sit_at_the_clients_hard_coded_indices() {
        let d = item_row_descriptor();
        for (name, ix) in [
            ("itemID", IX_ITEM_ID),
            ("flagID", IX_FLAG),
            ("quantity", IX_QUANTITY),
            ("customInfo", IX_CUSTOM_INFO),
            ("stacksize", IX_STACK_SIZE),
            ("singleton", IX_SINGLETON),
        ] {
            assert_eq!(d.index_of(name.as_bytes()), Some(ix), "{name}");
        }
    }

    #[test]
    fn an_eleven_value_row_packs_and_reads_back_singleton() {
        let r = row(eleven()).expect("eleven values fit the descriptor");
        assert_eq!(r.values()[IX_SINGLETON], DbValue::Int(0));
        assert_eq!(r.values()[IX_STACK_SIZE], DbValue::Int(20));
        assert!(!r.pack().is_empty());
    }

    /// The cause of the client's "sequence is too short" warnings: godma
    /// builds nine-value rows against `itemrd`/`sublocrd` (`godma.py:506,
    /// 1900, 1910, 2245, 2656, 3061` [V]). Ours has eleven physical columns,
    /// so such a row is short by two. Delete this test with the deviation.
    #[test]
    fn a_nine_value_row_is_short_of_our_descriptor() {
        let mut v = eleven();
        v.truncate(9);
        assert!(matches!(
            row(v),
            Err(DbRowError::ColumnCountMismatch { want: 11, got: 9 })
        ));
    }

    #[test]
    fn every_item_builder_shares_one_descriptor() {
        let reg = Registry::new(SandboxConfig::default());
        let a = reg.item_row_value(&crate::item::Item {
            item_id: 7_000_000_001,
            type_id: 9001,
            owner_id: 8_000_001,
            location_id: 7_000_000_002,
            flag_id: 27,
            quantity: 20,
            group_id: 9002,
            category_id: 9003,
            singleton: false,
        });
        let PyValue::DbRow(a) = a else { panic!("row") };
        assert_eq!(a.descriptor(), &item_row_descriptor());
        assert_eq!(a.values().len(), 11);
    }

    fn pa1_chars(n: i64) -> Registry {
        Registry::new(SandboxConfig {
            characters: (0..n)
                .map(|i| crate::model::Character {
                    character_id: 2_000_000 + i,
                    name: format!("Pilot{i}"),
                    ship_id: 3_000_000 + i,
                    ..crate::model::Character::default()
                })
                .collect(),
            ..SandboxConfig::default()
        })
    }

    /// PA-1: `Registry::character` through the index equals the scan for
    /// hits, misses, a later `seed_character`, `character_mut`, and a Vec
    /// somebody pushed to directly (falls back to the scan). `character`
    /// also asserts this itself in a debug build.
    #[test]
    fn character_index_matches_the_scan_through_every_mutation() {
        let mut reg = pa1_chars(20);
        for i in 0..20 {
            assert_eq!(
                reg.character(2_000_000 + i).map(|c| c.ship_id),
                Some(3_000_000 + i)
            );
        }
        assert!(reg.character(1).is_none());
        reg.seed_character(crate::model::Character {
            character_id: 2_000_100,
            name: "Late".into(),
            ship_id: 3_000_100,
            ..crate::model::Character::default()
        });
        assert_eq!(reg.character(2_000_100).map(|c| c.ship_id), Some(3_000_100));
        reg.character_mut(2_000_005).expect("mut").name = "Renamed".into();
        assert_eq!(
            reg.character(2_000_005).map(|c| c.name.as_str()),
            Some("Renamed")
        );
        assert!(reg.character_mut(2).is_none());
        // a direct push (a test fixture) is still found, by the fallback
        reg.config.characters.push(crate::model::Character {
            character_id: 2_000_200,
            name: "Direct".into(),
            ship_id: 3_000_200,
            ..crate::model::Character::default()
        });
        assert_eq!(reg.character(2_000_200).map(|c| c.ship_id), Some(3_000_200));
        assert_eq!(reg.character(2_000_003).map(|c| c.ship_id), Some(3_000_003));
        assert!(reg.character_mut(2_000_200).is_some());
    }

    /// PA-1 benchmark: 500 characters, one lookup each, indexed vs scan.
    /// `tools/dev.sh cargo test --release --locked -p eve-server character_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn character_bench_500() {
        let reg = pa1_chars(500);
        let rounds = 400;
        let t0 = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..rounds {
            for i in 0..500 {
                n += reg.char_at(2_000_000 + i).unwrap_or(0);
            }
        }
        let indexed = t0.elapsed();
        let t0 = std::time::Instant::now();
        let mut m = 0;
        for _ in 0..rounds {
            for i in 0..500 {
                m += reg
                    .config
                    .characters
                    .iter()
                    .position(|c| c.character_id == 2_000_000 + i)
                    .unwrap_or(0);
            }
        }
        let scan = t0.elapsed();
        assert_eq!(n, m);
        crate::errln!(
            "Registry::character x{}: indexed {:?}, scan {:?} ({:.0}x)",
            rounds * 500,
            indexed,
            scan,
            scan.as_secs_f64() / indexed.as_secs_f64().max(1e-9)
        );
    }
}

/// **`character` answers from the id index, and from a scan when the list
/// was changed behind it** (PERF-STAGES): seeding keeps the index, a test
/// that pushes to the list directly is still found.
#[test]
fn character_lookup_uses_the_index_and_survives_a_direct_push() {
    let mut r = registry();
    let base = r.config.characters.len();
    for i in 0..5 {
        r.seed_character(Character {
            character_id: 77_000 + i,
            ship_id: 770_000 + i,
            name: format!("Indexed{i}"),
            ..Character::default()
        });
    }
    for i in 0..5 {
        assert_eq!(
            r.character(77_000 + i).map(|c| c.name.clone()),
            Some(format!("Indexed{i}"))
        );
    }
    assert!(r.character(1).is_none() || base > 0);
    assert!(r.character(76_999).is_none());
    r.config.characters.push(Character {
        character_id: 88_000,
        ship_id: 880_000,
        name: "Pushed".to_owned(),
        ..Character::default()
    });
    assert_eq!(
        r.character(88_000).map(|c| c.name.as_str()),
        Some("Pushed"),
        "a list changed behind the index is scanned"
    );
    assert_eq!(
        r.character(77_003).map(|c| c.name.as_str()),
        Some("Indexed3")
    );
    r.seed_character(Character {
        character_id: 99_000,
        ship_id: 990_000,
        name: "Later".to_owned(),
        ..Character::default()
    });
    assert_eq!(r.character(99_000).map(|c| c.name.as_str()), Some("Later"));
    assert_eq!(r.character(88_000).map(|c| c.name.as_str()), Some("Pushed"));
}

/// PA-12a: every `READS` entry is answered by its read arm, and the answer is
/// the write arm's, error variants included. The registry-level half of the
/// dispatch differential (the byte-level half is in `dispatch.rs`).
#[test]
fn every_read_method_is_answered_by_its_read_arm() {
    let mut r = registry();
    let actor: Option<i64> = None;
    let caller = Caller::Session {
        user_id: 1,
        role: crate::roles::SANDBOX_DOCKED,
    };
    let mut seen = 0;
    for (methods, _) in READS {
        for (service, method) in methods.iter() {
            let req = Request {
                service,
                method,
                args: &[],
                kwargs: &[],
                bind_params: None,
                actor,
            };
            let ro = r
                .dispatch_ro(caller, &req)
                .unwrap_or_else(|| panic!("{service}.{method} has no read arm"))
                .map(RoValue::into_owned);
            let rw = r.dispatch_as(caller, &req);
            assert_eq!(format!("{ro:?}"), format!("{rw:?}"), "{service}.{method}");
            assert!(Registry::read_eligible(service, method));
            seen += 1;
        }
    }
    assert_eq!(seen, 11);
    // Not in the table: no read arm, and not eligible.
    let req = Request {
        service: "config",
        method: "GetOldStationData",
        args: &[],
        kwargs: &[],
        bind_params: None,
        actor,
    };
    assert!(r.dispatch_ro(caller, &req).is_none());
    assert!(!Registry::read_eligible("config", "GetOldStationData"));
}

/// A read arm answers for the services it names only: the method name alone
/// is not enough.
#[test]
fn a_read_arm_declines_another_services_method() {
    let r = registry();
    let req = Request {
        service: "otherSvc",
        method: "GetAverageMarketPrices",
        args: &[],
        kwargs: &[],
        bind_params: None,
        actor: None,
    };
    assert!(r.dispatch_ro(Caller::Internal, &req).is_none());
}

/// GAP-QUICKFIX B2: `config.GetStationSolarSystemsByOwner` is iterated
/// unguarded by Show Info, so it is a list (never None), one KeyVal per
/// system the owner holds a station in.
#[test]
fn station_solar_systems_by_owner_is_a_list_of_systems() {
    let mut stations = crate::stations::Table::default();
    for (id, system, owner) in [
        (60_001, 30_001, 1_001),
        (60_002, 30_001, 1_001),
        (60_003, 30_002, 1_001),
        (60_004, 30_003, 1_002),
    ] {
        stations.insert(
            id,
            crate::stations::Station {
                system_id: system,
                region_id: 1,
                owner_id: owner,
                operation_id: 1,
            },
        );
    }
    let mut content = crate::content::Content::fixture();
    content.insert("stations", stations);
    let mut reg = registry().with_content(std::sync::Arc::new(content));
    let mut ask = |owner: i64| {
        let args = [PyValue::Int(owner)];
        let req = Request {
            service: "config",
            method: "GetStationSolarSystemsByOwner",
            args: &args,
            kwargs: &[],
            bind_params: None,
            actor: ACTOR,
        };
        let PyValue::List(rows) = reg.dispatch(&req).expect("answered") else {
            panic!("must be a list")
        };
        rows
    };
    assert_eq!(ask(1_001).len(), 2);
    assert_eq!(ask(1_002).len(), 1);
    assert!(ask(9).is_empty());
}

/// GAP-QUICKFIX B3 and B5: the edit-member dialog's private info and the
/// first-flight camera question.
#[test]
fn private_info_and_has_ever_warped_answer_the_clients_shape() {
    let mut reg = registry();
    let me = ACTOR.expect("a pilot");
    let args = [PyValue::Int(me)];
    let req = Request {
        service: "charMgr",
        method: "GetPrivateInfo",
        args: &args,
        kwargs: &[],
        bind_params: None,
        actor: ACTOR,
    };
    let got = reg.dispatch(&req).expect("answered");
    let have = keyval_attributes(&got);
    for want in ["characterID", "gender", "raceID", "createDateTime"] {
        assert!(have.iter().any(|a| a == want), "{want} in {have:?}");
    }
    let c = reg.character(me).expect("pilot").clone();
    let PyValue::Instance { state, .. } = &got else {
        panic!("a keyval")
    };
    let get = |k: &str| crate::pyutil::dict_get(state, k.as_bytes()).cloned();
    assert_eq!(get("characterID"), Some(PyValue::Int(me)));
    assert_eq!(get("gender"), Some(PyValue::Int(c.gender_id)));
    assert_eq!(get("raceID"), Some(PyValue::Int(c.race_id)));
    assert_eq!(
        get("createDateTime"),
        Some(PyValue::Int(c.create_date_time))
    );
    // An unknown id is refused, not answered with a half-empty record.
    let args = [PyValue::Int(7)];
    let req = Request { args: &args, ..req };
    assert!(reg.dispatch(&req).is_err());

    let req = Request {
        service: "monolithTaskTrackerMgr",
        method: "HasEverWarped",
        args: &[],
        kwargs: &[],
        bind_params: None,
        actor: ACTOR,
    };
    assert_eq!(reg.dispatch(&req).expect("answered"), PyValue::Bool(true));
}

#[test]
fn every_global_in_the_item_descriptor_is_whitelisted() {
    // The client raises HACKER WARNING on any global outside its whitelist.
    fn walk(v: &PyValue, out: &mut Vec<Vec<u8>>) {
        match v {
            PyValue::Global(g) => out.push(g.clone()),
            PyValue::Tuple(xs) | PyValue::List(xs) => xs.iter().for_each(|x| walk(x, out)),
            PyValue::ObjectEx { header, .. } => walk(header, out),
            _ => {}
        }
    }
    let mut globals = Vec::new();
    walk(&item_descriptor(), &mut globals);
    // The descriptor class and the two virtual-column functions.
    assert_eq!(globals.len(), 3);
    for g in globals {
        assert!(
            eve_rowset::whitelist::is_whitelisted(&g),
            "not whitelisted: {}",
            String::from_utf8_lossy(&g)
        );
    }
}

#[test]
fn the_item_descriptor_still_parses_as_its_nine_physical_columns() {
    let d = DbRowDescriptor::from_pyvalue(&item_descriptor()).expect("parses");
    let names: Vec<_> = d
        .columns()
        .iter()
        .map(|c| String::from_utf8_lossy(&c.name).into_owned())
        .collect();
    assert_eq!(names.len(), 9);
    assert_eq!(names[0], "itemID");
    assert_eq!(names[8], "customInfo");
    // Rows stay on the eleven-column descriptor.
    assert_eq!(item_row_descriptor().columns().len(), 11);
}

/// **Lane tick-schedule (#136).** The clock takes one `now` at the top of a
/// tick and drains with it. A push a stage schedules to leave "in this tick"
/// (`after` of zero) must be due at that `now`; stamped with a clock read
/// taken later it would wait for the next tick, a quarter second on.
#[test]
fn a_zero_delay_push_scheduled_inside_a_tick_is_due_at_that_ticks_now() {
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    let now = std::time::Instant::now();
    // Make sure the real clock has moved past `now` without sleeping on the
    // wall clock: spin until it reads later.
    while std::time::Instant::now() <= now {}
    reg.set_tick_now(Some(now));
    reg.schedule(
        std::time::Duration::ZERO,
        crate::notify::AccountChange {
            owner_id: 1,
            balance: 0.0,
        },
    );
    reg.set_tick_now(None);
    assert_eq!(reg.take_due(now).len(), 1, "left in the tick it was made");
}

/// Outside a tick (a payload turn) `schedule` still reads the real clock, so
/// a delay is counted from the moment of the call.
#[test]
fn a_push_scheduled_outside_a_tick_counts_from_the_real_clock() {
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    let before = std::time::Instant::now();
    reg.schedule(
        std::time::Duration::from_secs(5),
        crate::notify::AccountChange {
            owner_id: 1,
            balance: 0.0,
        },
    );
    assert!(
        reg.take_due(before + std::time::Duration::from_secs(4))
            .is_empty()
    );
    assert_eq!(
        reg.take_due(before + std::time::Duration::from_secs(6))
            .len(),
        1
    );
}

/// A delay scheduled inside a tick counts from the tick's `now`.
#[test]
fn a_delayed_push_inside_a_tick_counts_from_the_call() {
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    let now = std::time::Instant::now();
    while std::time::Instant::now() == now {}
    reg.set_tick_now(Some(now));
    reg.schedule(
        std::time::Duration::from_secs(2),
        crate::notify::AccountChange {
            owner_id: 1,
            balance: 0.0,
        },
    );
    reg.set_tick_now(None);
    assert!(
        reg.take_due(now + std::time::Duration::from_secs(2))
            .is_empty()
    );
    assert_eq!(
        reg.take_due(std::time::Instant::now() + std::time::Duration::from_secs(2))
            .len(),
        1
    );
}
