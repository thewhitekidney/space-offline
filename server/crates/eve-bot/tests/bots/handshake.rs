//! The bot logs into a real server over a real socket.
//!
//! This is milestone 1 of `docs/archive/plan-bots.md` asserting itself. It proves the
//! two halves agree with *each other*, which is all it can prove: both sides
//! marshal with `eve-marshal`, so a shared encoding mistake would pass here
//! and be caught only by `tools/client-ci.sh` against the real client. The
//! crate header says so at more length; it is worth saying twice, because a
//! green test here is easy to over-read.

use super::common::{config, gm, pilot, serve};
use eve_server::server::Server;
use std::net::SocketAddr;
use std::time::Duration;

/// Into space and out past the 150 km minimum warp distance (`/tp`, the
/// GM's own setup): the server refuses a warp that lands inside it.
fn enter_far(c: &mut eve_bot::Conn) -> eve_bot::WorldState {
    c.enter_space(30_000_001).expect("read the world");
    gm(c, "/tp 300");
    let out = c.request_state().expect("a fresh state after the tp");
    out.notifications
        .iter()
        .filter_map(eve_bot::notification)
        .filter(|(event, _)| event == "DoDestinyUpdate")
        .find_map(|(_, args)| eve_bot::read_set_state(&args))
        .expect("a SetState with the ship far out")
}

fn start() -> (SocketAddr, std::thread::JoinHandle<()>) {
    let server = Server::bind(config()).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        // One connection is all this test makes. The end state is the
        // server's business; the bot's success is asserted below.
        let _ = server.run_once();
    });
    (addr, handle)
}

#[test]
fn the_bot_completes_the_handshake_against_a_real_server() {
    let (addr, server) = start();
    let mut conn = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    let session = conn.handshake().expect("the handshake completes");

    // The server challenge is the one thing H8 must carry, and the post
    // handshake sequence refers back to it.
    assert_eq!(
        session.server_challenge.len(),
        64,
        "the challenge is 64 bytes in both directions"
    );
    assert!(
        !session.server_challenge.iter().all(|b| *b == 0),
        "an all-zero challenge means the server never filled it in"
    );

    // H10 exists and is not nothing. Deliberately not interpreted further:
    // milestone 1 is "log in and hold the session", and reading a meaning into
    // this frame before a scenario needs one would be inventing a claim.
    assert_ne!(
        session.h10,
        eve_marshal::PyValue::None,
        "H10 should carry the session"
    );

    drop(conn);
    let _ = server.join();
}

#[test]
fn a_server_that_says_nothing_is_an_error_and_not_a_hang() {
    // A bot is unattended by definition, so the failure mode that matters is
    // the one where nobody is watching: a listener that accepts and then never
    // speaks. The handshake is server-initiated, so this blocks in the very
    // first `recv` -- which is exactly where a missing read timeout would wedge
    // a bot for ever.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let silent = std::thread::spawn(move || {
        let _accepted = listener.accept();
        // Hold the connection open, saying nothing, until the test drops it.
        std::thread::sleep(Duration::from_secs(2));
    });

    let mut conn = eve_bot::Conn::connect(addr, Duration::from_millis(250))
        .expect("connecting still succeeds");
    let err = conn.handshake().expect_err("a silent server must fail");
    match err {
        eve_bot::Error::Io(e) => {
            // WouldBlock or TimedOut depending on the platform; both mean the
            // timeout fired, which is the behaviour under test.
            assert!(
                matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ),
                "expected a timeout, got {:?}",
                e.kind()
            );
        }
        other => panic!("expected an io timeout, got {other}"),
    }
    let _ = silent.join();
}

#[test]
fn the_bot_becomes_a_character_and_the_session_changes() {
    // This is what makes a bot a *player*: a handshake gets a session, which
    // nobody else can see. A character is what another player's station lobby
    // or ballpark can hold.
    //
    // The assertion is the session change rather than the return value,
    // because `SelectCharacterID`'s return value is discarded by the real
    // client -- its effect IS the session change (`dispatch.rs`
    // `handle_select_character`). A bot that checked the result would pass
    // against a server that answered politely and changed nothing.
    let (addr, server) = start();
    let mut conn = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    conn.handshake().expect("the handshake completes");

    let outcome = conn
        .select_character(90_000_001)
        .expect("character 90000001 is the sandbox's first");
    assert!(
        outcome.session_changed,
        "select_character only returns Ok when the session changed"
    );

    drop(conn);
    let _ = server.join();
}

#[test]
fn selecting_a_character_that_does_not_exist_is_reported_not_assumed() {
    // The server answers this call even when it fails, on purpose: the real
    // client stays on the character screen, which is visible, rather than
    // freezing on the click with nothing in either log. So the bot must read
    // the absence of a session change as the failure -- there is no error
    // reply to notice.
    let (addr, server) = start();
    let mut conn = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    conn.handshake().expect("the handshake completes");

    let err = conn
        .select_character(1)
        .expect_err("character 1 does not exist");
    match err {
        eve_bot::Error::Protocol(why) => assert!(
            why.contains("session change"),
            "the message should name what was missing, got {why:?}"
        ),
        other => panic!("expected a protocol error, got {other}"),
    }

    drop(conn);
    let _ = server.join();
}

#[test]
fn the_bot_undocks_binds_the_ballpark_and_the_world_arrives() {
    // Milestone 2's first half: the bot receives DoDestinyUpdate. Decoding the
    // blob into a Park is separate; what this pins is the *chain*, which is
    // the part with the surprising step in it.
    //
    // The surprising step: binding the ballpark is what makes the world
    // arrive, not undocking. The server holds the bootstrap until
    // `beyonce.MachoBindObject(ssid, None)` asks for it, because the real
    // client has no ballpark for about three seconds after the undock and
    // `michelle.DoDestinyUpdate` silently returns while that is true. A bot
    // that undocks and waits waits for ever.
    let (addr, server) = start();
    let mut conn = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    conn.handshake().expect("the handshake completes");
    conn.select_character(90_000_001)
        .expect("become a character");
    conn.undock().expect("leave the station");

    let outcome = conn.bind_ballpark(30_000_001).expect("bind the ballpark");
    let events: Vec<String> = outcome
        .notifications
        .iter()
        .filter_map(|f| eve_bot::notification(f).map(|(event, _)| event))
        .collect();
    assert!(
        events.iter().any(|e| e == "DoDestinyUpdate"),
        "the bootstrap state should arrive behind the bind reply; saw {events:?}"
    );

    drop(conn);
    let _ = server.join();
}

#[test]
fn the_bot_knows_where_it_is() {
    // Milestone 2, whole: not "a push arrived" but "the bot can say what is
    // in the scene and which ball is its own". That is the difference between
    // a connection and an actor.
    let (addr, server) = start();
    let mut conn = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    conn.handshake().expect("the handshake completes");
    conn.select_character(90_000_001)
        .expect("become a character");
    conn.undock().expect("leave the station");

    let world = conn.enter_space(30_000_001).expect("read the world");

    assert_eq!(
        world.ego, 100_000_001,
        "the ego is the ship item id, which is what gameui stores as wannaBeEgo"
    );
    assert_eq!(
        world.stamp as u32, world.park.current_time,
        "the action stamp and the blob's clock must be the same number, or the \
         park starts out of step with the actions that follow it"
    );
    assert!(
        !world.park.balls.is_empty(),
        "an empty park is the undocked-into-nowhere defect"
    );

    // The ego ball has to actually be there. `Park.SetState` assigns the ego
    // without checking, so a state can name a ball it never created -- which
    // looks fine until the camera attaches to nothing.
    let ego = world
        .ego_ball()
        .expect("the ego ball must be in the park it is the ego of");
    assert_eq!(ego.ball_id, world.ego);
    assert!(
        ego.radius > 0.0,
        "a zero radius is a ball nothing can be outside of"
    );

    drop(conn);
    let _ = server.join();
}

/// Two characters in one station, so each can see the other's ship in space.
///
/// `run()` rather than `run_once()`: two bots are two connections.
fn start_two() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(90_000_001, "Alice", 100_000_001),
        pilot(90_000_002, "Bob", 100_000_002),
    ];
    serve(config)
}

/// Three characters, because some observables need a bystander.
///
/// **A bot can have the third observer a human cannot.** human-checks §7
/// step 5 says of the follow test: *"a third observer is not available so just
/// watch from Bob's client"* -- and watching from Bob's client is exactly what
/// cannot answer it, because Bob is the ship being followed and his own ego
/// ball is in every state he is sent. A bystander who binds last reads the
/// server's record of **both** of them.
///
/// Carol has no second station to be at, so she undocks from the same one.
fn start_three() -> SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(90_000_001, "Alice", 100_000_001),
        pilot(90_000_002, "Bob", 100_000_002),
        pilot(90_000_003, "Carol", 100_000_003),
    ];
    serve(config)
}

#[test]
fn two_bots_in_one_system_each_see_the_other_s_ship() {
    // **Milestone 4, and the point of the whole crate.** This is the shape of
    // human-checks section 7 steps 1, 2 and 14 -- "two ships, plainly apart",
    // "Alice is where she actually is" -- with no Wine, no display and no
    // person watching. `lobby_guests` proved two real clients can do it; two
    // bots cost a test.
    //
    // The assertion is deliberately the *second* bot's view of the first.
    // Asserting that a bot sees its own ship proves nothing about the server's
    // multi-actor state: our own ego ball is in every state we are sent. The
    // defect this shape catches is the one where a joiner is told about
    // itself and nothing else.
    let addr = start_two();

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let alice_world = alice
        .enter_space(30_000_001)
        .expect("alice reads the world");
    assert_eq!(alice_world.ego, 100_000_001);

    // Bob arrives afterwards, which is the interesting order: his first state
    // is built at bind time and is the only chance to be right about Alice.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");

    assert_eq!(bob_world.ego, 100_000_002, "bob's ego is bob's ship");
    let ids: Vec<i64> = bob_world.park.balls.iter().map(|b| b.ball_id).collect();
    assert!(
        ids.contains(&100_000_001),
        "bob's park should contain alice's ship 100000001; it holds {ids:?}"
    );
    assert!(
        ids.contains(&100_000_002),
        "and bob's own, 100000002; it holds {ids:?}"
    );

    // Two ships, plainly apart -- the observable of step 1. Sharing a position
    // is the defect where both ships are put at the station mouth.
    let alice_ball = bob_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == 100_000_001)
        .expect("just asserted present");
    let bob_ball = bob_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == 100_000_002)
        .expect("just asserted present");
    let apart = ((alice_ball.position[0] - bob_ball.position[0]).powi(2)
        + (alice_ball.position[1] - bob_ball.position[1]).powi(2)
        + (alice_ball.position[2] - bob_ball.position[2]).powi(2))
    .sqrt();
    // Printed, not just asserted: "more than a metre" is a weak claim and the
    // actual figure is the interesting part. human-checks expects about 750 m.
    eprintln!("two_bots: alice and bob are {apart:.1} m apart in bob's park");
    assert!(
        apart > 1.0,
        "two undocked ships must not share a position; {apart} m apart"
    );

    drop(bob);
    drop(alice);
}

#[test]
fn the_bot_can_warp_and_the_server_sends_the_whole_batch() {
    // **Milestone 3, and a headless regression test for both defects the
    // player reported.** A bot issues movement through the BOUND ballpark --
    // addressed to the node with no service name, the object id in the payload
    // being the only routing information in the frame -- and then reads what
    // the server pushed back.
    let (addr, server) = start();
    let mut bot = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("the bot connects");
    bot.handshake().expect("handshake");
    bot.select_character(90_000_001)
        .expect("become a character");
    bot.undock().expect("undock");
    let world = enter_far(&mut bot);

    bot.set_speed_fraction(1.0)
        .expect("the throttle is a bound call and must be accepted");

    // Anything but our own ship. In this sandbox that is the station, which is
    // a legitimate warp target and all a bare in-process server has: the
    // celestials come from a TSV the real runs load.
    let target = world
        .park
        .balls
        .iter()
        .map(|b| b.ball_id)
        .find(|id| *id != world.ego)
        .expect("something to warp at");

    let warp = bot.warp_to_item(target, 0.0).expect("the warp is accepted");
    let mut actions = Vec::new();
    let mut collect = |frames: &[eve_marshal::PyValue]| {
        for frame in frames {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "DoDestinyUpdate"
            {
                actions.extend(eve_bot::read_actions(&args));
            }
        }
    };
    collect(&warp.notifications);
    let later = bot
        .collect_pushes(Duration::from_millis(1500))
        .unwrap_or_default();
    collect(&later);

    let names: Vec<&str> = actions.iter().map(|(_, n)| n.as_str()).collect();
    eprintln!("warp: server pushed {names:?}");

    assert!(
        names.contains(&"WarpTo"),
        "the warp command must produce a WarpTo action; got {names:?}"
    );
    // **The aligning-forever defect, asserted headlessly.** A WarpTo with no
    // SetSpeedFraction beside it is the bug the player hit: the sub-warp goto
    // integrator scales maxVelocity by the throttle, so a shut throttle
    // commands zero speed, neither align gate can ever pass, and the ship
    // aligns until the 180-tick watchdog forces the warp
    // (docs/port-tracker.md section 34).
    assert!(
        names.contains(&"SetSpeedFraction"),
        "a warp must carry the throttle or the ship aligns for three minutes; got {names:?}"
    );
    // And the tunnel, which was the player's second report.
    assert!(
        names.contains(&"OnSpecialFX"),
        "the warp tunnel effect should be in the batch; got {names:?}"
    );

    drop(bot);
    let _ = server.join();
}

#[test]
fn the_bot_times_its_own_arrival_against_the_server_s_own_model() {
    // **The assertion only a bot can make.** A client scenario can see that a
    // ship arrived; it cannot easily time the arrival against the number the
    // server computed, because it has no access to our model and its own
    // clock readings go through the client's scheduler. A bot has both: it
    // knows when it sent the warp, and it can call
    // `destiny::warp_flight_seconds` itself.
    //
    // This is also the first end-to-end test over a real socket of the
    // SCHEDULED push queue -- the newest machinery in the server. Everything
    // else that exercises it does so in-process.
    let (addr, server) = start();
    let mut bot = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("the bot connects");
    bot.handshake().expect("handshake");
    bot.select_character(90_000_001)
        .expect("become a character");
    bot.undock().expect("undock");
    let world = enter_far(&mut bot);

    let ego = *world.ego_ball().expect("our own ball");
    let target = world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id != world.ego)
        .copied()
        .expect("something to warp at");
    // **The distance to the STANDOFF, not to the centre.** The first version of
    // this measured ego-to-target and got a model of 11.4 s against an
    // observed 8.7 s, which looks exactly like the server scheduling an
    // arrival earlier than its own flight time -- a real defect, if it were
    // true. It was not: the server warps to `min_range.max(radius +
    // WARP_STANDOFF)` short of the target and computes the flight over THAT
    // distance, and a station has a large radius. Recomputing the model with
    // the raw distance overestimates the flight and makes a correct arrival
    // look early.
    //
    // Same mistake as section 34.8 in the port tracker, in the other
    // direction: compute what the server computes, or the disagreement is
    // yours.
    let standoff = (target.radius as f64) + eve_server::space_state::WARP_STANDOFF;
    let arrival = eve_server::destiny::warp_arrival(ego.position, target.position, standoff);
    let distance = ((ego.position[0] - arrival[0]).powi(2)
        + (ego.position[1] - arrival[1]).powi(2)
        + (ego.position[2] - arrival[2]).powi(2))
    .sqrt();

    // The server's own arithmetic, from the same functions and tables the
    // server uses (a server with no `--content` holds `Content::fixture`).
    let content = eve_server::content::Content::fixture();
    let hull =
        eve_server::destiny::hull::find_in(&content, 670).expect("the capsule is in the table");
    let expected = eve_server::destiny::warp_flight_seconds(
        distance,
        eve_server::destiny::warp_factor_in(&content, 670),
        eve_server::destiny::hull::align_seconds(hull),
    );

    bot.set_speed_fraction(1.0).expect("throttle");
    let sent = std::time::Instant::now();
    bot.warp_to_item(target.ball_id, 0.0).expect("warp");

    // Wait for the pilot's settle batch. The pilot's own client flies and
    // stops its warp, so it is sent only the massive demotion at the
    // predicted landing (CAPTURE-UNDOCK); observers get the
    // SetBallPosition, SetBallVelocity, Stop batch.
    let mut arrived_after = None;
    let mut names = Vec::new();
    let deadline = Duration::from_secs_f64(expected + 10.0);
    while sent.elapsed() < deadline {
        let frames = bot
            .collect_pushes(Duration::from_millis(500))
            .unwrap_or_default();
        for frame in &frames {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "DoDestinyUpdate"
            {
                for (_, name) in eve_bot::read_actions(&args) {
                    names.push(name.clone());
                    if name == "SetBallMassive" && arrived_after.is_none() {
                        arrived_after = Some(sent.elapsed());
                    }
                }
            }
        }
        if arrived_after.is_some() {
            break;
        }
    }

    eprintln!(
        "arrival: standoff {standoff:.0} m, travel {distance:.0} m, model {expected:.1} s, \
         observed {:?}, actions {names:?}",
        arrived_after
    );

    let arrived_after = arrived_after.expect(
        "the scheduled arrival batch never fired -- a warp that is commanded and never \
         settled leaves the ship coasting past its target for ever",
    );

    // Loose on purpose: this is a real socket on a machine that is also
    // building containers, and the point is that the schedule fires at
    // roughly the computed time rather than at some other time entirely. A
    // tight bound here would be a flaky test, and a flaky test is worse than
    // a loose one.
    assert!(
        arrived_after.as_secs_f64() >= expected - 2.0,
        "arrived in {:.1} s but the model says {expected:.1} s -- early means the \
         schedule is not using the flight time at all",
        arrived_after.as_secs_f64()
    );
    // Measured residual on a real socket: +0.63 s (model 8.1, observed 8.73),
    // which is the 250 ms clock granularity plus the push's trip out. Five
    // seconds of headroom on top of that is loose enough for a host that is
    // also building containers and tight enough to catch a clock that has
    // stopped honouring its schedule.
    assert!(
        arrived_after.as_secs_f64() <= expected + 5.0,
        "arrived in {:.1} s against a model of {expected:.1} s -- that much late \
         means the clock is not keeping its schedule",
        arrived_after.as_secs_f64()
    );

    drop(bot);
    let _ = server.join();
}

/// **A pilot who binds mid-warp sees a ship in warp, not a ship already
/// parked.**
///
/// This test is the same experiment it always was, with the verdict inverted,
/// and that inversion is the point of `docs/port-tracker.md` §59.
///
/// It used to be called
/// `a_pilot_who_binds_mid_warp_sees_the_warping_ship_already_parked` and it
/// pinned §30.3's `[A]`: `Motion::after(WarpTo)` teleported the server's
/// record to the arrival point at command time, so a third party binding
/// mid-flight was handed a ship that had already finished. The old doc ended
/// with an instruction, and this is it being followed:
///
/// > **If this fails, read it as §30.3 having been fixed**, not as the test
/// > breaking: it would mean the server's record now travels, and the doc and
/// > this test both need rewriting. A characterisation test, and it says so.
///
/// It failed, on the first run after the flip, and it was the **only** test in
/// 1,066 that did. A characterisation test that names its own falsifier is
/// worth more than one that merely passes: this one told the next person what
/// its own failure meant, a month before it happened.
///
/// **What is asserted now.** Alice warps; Bob binds while she is in flight and
/// his bootstrap is built from the server's record. That record must put her
/// *between* the two ends -- not at the departure point, which would mean the
/// warp never started, and not at the arrival, which was the old defect. The
/// bracket travels.
#[test]
fn a_pilot_who_binds_mid_warp_sees_a_ship_in_flight() {
    let addr = start_two();

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let before = enter_far(&mut alice);
    let departed = before.ego_ball().copied().expect("alice's own ball");
    let station = before
        .park
        .balls
        .iter()
        .find(|b| b.ball_id != before.ego)
        .copied()
        .expect("the station is in the park");

    // Where the server will put her: the same arithmetic it uses.
    let standoff = (station.radius as f64) + eve_server::space_state::WARP_STANDOFF;
    let arrival = eve_server::destiny::warp_arrival(departed.position, station.position, standoff);

    alice.set_speed_fraction(1.0).expect("throttle");
    alice
        .warp_to_item(station.ball_id, 0.0)
        .expect("alice warps at the station");

    // Bob arrives while she is in flight. His bootstrap is built at bind time
    // and is the only chance to be right about Alice -- the §29 lesson.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");

    let seen = bob_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == departed.ball_id)
        .copied()
        .expect("bob should see alice's ship at all");

    let gap = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let trip = gap(departed.position, arrival);
    let from_arrival = gap(seen.position, arrival);
    eprintln!(
        "mid-warp: bob sees alice {from_arrival:.0} m from her arrival point, \
         {:.0} m from where she left, on a {trip:.0} m trip; mode {:?}",
        gap(seen.position, departed.position),
        seen.mode,
    );

    // **Not parked at the destination**, which is the defect this test was
    // written to characterise and now exists to prevent coming back. The
    // tolerance is a metre of float noise, not a budget.
    assert!(
        from_arrival > 1.0,
        "alice must not be AT her arrival point the moment she is told to warp: \
         {from_arrival:.3} m away on a {trip:.0} m trip. That is §30.3's \
         teleport, and §59 removed it",
    );

    // **And she is on the line, not somewhere else.** A ship that travels but
    // leaves the path would pass the assertion above while being wrong in a
    // way nobody would notice until a client drew it.
    assert!(
        from_arrival <= trip + 1.0,
        "alice is {from_arrival:.0} m from arrival on a {trip:.0} m trip, so she is \
         not between the two ends at all",
    );

    drop(bob);
    drop(alice);
}

#[test]
fn binding_the_ballpark_twice_puts_the_ship_back_at_the_station_mouth() {
    // **The contract, pinned, and the sharp edge a bot can walk into.**
    //
    // `destiny::on_ballpark_bind` rewrites the actor's roster entry from
    // `undock_motion` on every bind, on purpose: "a bind means a ship has just
    // arrived in space, and the mouth is where it starts". That is sound for
    // the real client, which binds once per undock because
    // `michelle.Park.Release` destroys the park when it docks. It is an
    // *assumption* for anything else, and a bot is the anything else.
    //
    // This is how it was found: `/tp 1000` answers "moved you 1000 km out"
    // and does move the roster, and then a second `enter_space` quietly put
    // the ship back at the mouth -- so BOTH the ship's own view and another
    // pilot's view showed the teleport undone. Not a server defect for the
    // client; a contract worth a test and a guard.
    let addr = start_two();

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let first = alice
        .enter_space(30_000_001)
        .expect("alice reads the world");
    let at_undock = first.ego_ball().copied().expect("alice's own ball");

    alice
        .call(
            "slash",
            "SlashCmd",
            vec![eve_marshal::PyValue::Bytes(b"/tp 1000".to_vec())],
        )
        .expect("the teleport is accepted");

    // The guard: an accidental second bind is refused, naming the reason.
    let refused = alice
        .bind_ballpark(30_000_001)
        .expect_err("a second bind must not be silent");
    match refused {
        eve_bot::Error::Protocol(why) => assert!(
            why.contains("station") && why.contains("rebind_ballpark"),
            "the refusal should name the consequence and the way round it, got {why:?}"
        ),
        other => panic!("expected a protocol refusal, got {other}"),
    }

    // And deliberately, which is the behaviour being characterised.
    alice
        .rebind_ballpark(30_000_001)
        .expect("an explicit re-bind is allowed");
    let after = alice
        .collect_pushes(Duration::from_millis(1500))
        .unwrap_or_default();
    let mut reset_to = None;
    for frame in &after {
        if let Some((event, args)) = eve_bot::notification(frame)
            && event == "DoDestinyUpdate"
            && let Some(world) = eve_bot::read_set_state(&args)
            && let Some(ball) = world.ego_ball()
        {
            reset_to = Some(ball.position);
        }
    }

    let gap = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };

    // Whatever the re-bind said, the roster is what another pilot reads, and
    // that is the half that matters: Bob should see Alice back at the mouth,
    // not 1000 km out.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");
    let alice_as_bob_sees = bob_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == at_undock.ball_id)
        .copied()
        .expect("bob sees alice's ship");
    let from_mouth = gap(alice_as_bob_sees.position, at_undock.position);

    eprintln!(
        "rebind: after /tp 1000 and an explicit re-bind, bob sees alice {from_mouth:.0} m \
         from the undock point (re-bind reported {reset_to:?})"
    );

    assert!(
        from_mouth < 1_000.0,
        "the re-bind should have reset alice to the mouth; bob sees her \
         {from_mouth:.0} m away, which would mean on_ballpark_bind no longer \
         rewrites the roster and this test is the thing to update"
    );

    drop(bob);
    drop(alice);
}

#[test]
fn orbit_follow_and_align_reach_the_server_and_come_back_as_actions() {
    // **Human-checks §7 steps 4 and 5, server half.** Those steps ask a person
    // to orbit and follow and watch; what a bot can settle is whether the
    // commands are accepted and broadcast at all, which is the half that has
    // been `[A]` since it was written. The visual half -- does the ship
    // actually circle -- stays with the human batch, because a mode change is
    // not a trajectory.
    let (addr, server) = start();
    let mut bot = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("the bot connects");
    bot.handshake().expect("handshake");
    bot.select_character(90_000_001)
        .expect("become a character");
    bot.undock().expect("undock");
    let world = bot.enter_space(30_000_001).expect("read the world");
    let target = world
        .park
        .balls
        .iter()
        .map(|b| b.ball_id)
        .find(|id| *id != world.ego)
        .expect("something to orbit");

    let mut seen = Vec::new();
    let mut run = |label: &str, outcome: eve_bot::CallOutcome, bot: &mut eve_bot::Conn| {
        let mut names = Vec::new();
        let mut frames = outcome.notifications;
        frames.extend(
            bot.collect_pushes(Duration::from_millis(800))
                .unwrap_or_default(),
        );
        for frame in &frames {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "DoDestinyUpdate"
            {
                for (_, name) in eve_bot::read_actions(&args) {
                    names.push(name);
                }
            }
        }
        eprintln!("{label}: {names:?}");
        seen.push((label.to_owned(), names));
    };

    let o = bot.orbit(target, 5_000.0).expect("orbit is accepted");
    run("orbit", o, &mut bot);
    let f = bot.follow(target, 2_000.0).expect("follow is accepted");
    run("follow", f, &mut bot);
    let a = bot
        .goto_direction([0.0, 0.0, -1.0])
        .expect("align is accepted");
    run("align", a, &mut bot);

    for (label, wanted) in [
        ("orbit", "Orbit"),
        ("follow", "FollowBall"),
        ("align", "GotoDirection"),
    ] {
        let names = &seen.iter().find(|(l, _)| l == label).expect("ran").1;
        assert!(
            names.iter().any(|n| n == wanted),
            "{label} should produce a {wanted} action; got {names:?}"
        );
    }

    drop(bot);
    let _ = server.join();
}

#[test]
fn a_lock_is_deferred_and_the_server_times_it() {
    // **Human-checks §7 step 8, settled on our side.**
    //
    // I twice got this wrong before the test ran. First I planned to time a
    // lock against "the server's formula" -- then found a comment in
    // `service.rs` saying "the server never times a lock" and concluded a bot
    // could not finish the step. That comment is about the two attribute
    // columns in the hull table, not about the lock path: `targeting::lock`
    // computes `acquisition_delay` and, when it exceeds one `push::TICK`,
    // answers **`(1, [])`** -- "not yet" -- and pushes `OnTarget` when the
    // time is up. `(0, [])` is the instant answer that makes `_LockTarget`
    // draw the bracket immediately.
    //
    // So the server does time it, and this is the measurement. What it
    // deliberately does NOT do is predict the duration: the fallback scan
    // resolution is `pub(crate)` and copying the number here would duplicate
    // a server constant that could then drift silently. The doc on
    // `acquisition_delay` says a capsule locking a capsule lands "around five
    // seconds"; the figure is printed so a reader can check that against the
    // run rather than against my arithmetic.
    //
    // A lock also needs another ship -- "lockable means another ship in this
    // solar system" -- so the station, which the first version used, is
    // refused with an error reply rather than a `CallRsp`.
    let addr = start_two();

    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bobs_ship = bob
        .enter_space(30_000_001)
        .expect("bob reads the world")
        .ego;

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    alice
        .enter_space(30_000_001)
        .expect("alice reads the world");

    assert_eq!(
        eve_bot::reply_value(&alice.targets().expect("GetTargets").result),
        Some(eve_marshal::PyValue::List(vec![])),
        "alice must start with nothing locked or nothing below proves anything"
    );

    let asked = std::time::Instant::now();
    let locked = alice.lock(bobs_ship).expect("AddTarget is answered");
    let flag = match eve_bot::reply_value(&locked.result) {
        Some(eve_marshal::PyValue::Tuple(pair)) => match pair.first() {
            Some(eve_marshal::PyValue::Int(n)) => *n,
            other => panic!("the flag must be an int, got {other:?}"),
        },
        other => panic!("AddTarget must answer a 2-tuple that unpacks, got {other:?}"),
    };
    assert_eq!(
        flag, 1,
        "a capsule locking a capsule takes seconds, so the server should defer \
         with (1, []) rather than answer (0, []) and have the client draw a \
         bracket before its own progress bar has run"
    );

    // **The server-side lock is recorded immediately even though the client's
    // is not**, which is what makes `GetTargets` right during the acquisition
    // and is why `CancelAddTarget` has work to do.
    let during = eve_bot::reply_value(&alice.targets().expect("GetTargets again").result);
    let listed = match &during {
        Some(eve_marshal::PyValue::List(items)) => items.iter().any(|v| match v {
            eve_marshal::PyValue::Int(n) => *n == bobs_ship,
            eve_marshal::PyValue::Tuple(t) | eve_marshal::PyValue::List(t) => {
                t.contains(&eve_marshal::PyValue::Int(bobs_ship))
            }
            _ => false,
        }),
        _ => false,
    };
    assert!(
        listed,
        "GetTargets should list the target during acquisition, not only after; got {during:?}"
    );

    // Now wait for the push that tells the client the lock completed.
    let mut on_target_after = None;
    while asked.elapsed() < Duration::from_secs(25) {
        let frames = alice
            .collect_pushes(Duration::from_millis(500))
            .unwrap_or_default();
        for frame in &frames {
            if let Some((event, _)) = eve_bot::notification(frame)
                && event == "OnTarget"
                && on_target_after.is_none()
            {
                on_target_after = Some(asked.elapsed());
            }
        }
        if on_target_after.is_some() {
            break;
        }
    }

    let took = on_target_after.expect(
        "no OnTarget push after a deferred lock -- the client's progress bar would \
         fill and then hang for ever, which is the whole reason the push exists",
    );
    eprintln!(
        "lock: a capsule locked a capsule; OnTarget arrived after {:.2} s \
         (the acquisition_delay doc says around five)",
        took.as_secs_f64()
    );
    assert!(
        took >= eve_bot::TICK_FLOOR,
        "a deferred lock cannot complete faster than a tick, or (1, []) was the \
         wrong answer; took {:.2} s",
        took.as_secs_f64()
    );
    assert!(
        took < Duration::from_secs(20),
        "a capsule on a capsule should be seconds, not {:.1} s -- 180 s is the \
         formula's cap and would mean the attributes went missing",
        took.as_secs_f64()
    );

    drop(alice);
    drop(bob);
}

#[test]
fn a_lock_is_dropped_when_its_target_leaves_range() {
    // **The gap this closes.** `Reason::OutOfRange` existed since locking was
    // written and nothing ever sent it: a lock survived any distance, so a
    // ship that warped away stayed locked on the locker's screen for ever.
    // The missing piece was somebody to notice, which is one of the three
    // things `Registry::schedule`'s own doc says the second queue exists for.
    //
    // Alice locks Bob, Bob is teleported past her targeting range, and the
    // clock should drop the lock and tell both of them -- with nobody calling
    // anything, which is the part only the clock can do.
    let addr = start_two();

    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bobs_ship = bob
        .enter_space(30_000_001)
        .expect("bob reads the world")
        .ego;

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(30)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    alice
        .enter_space(30_000_001)
        .expect("alice reads the world");

    alice.lock(bobs_ship).expect("the lock is accepted");
    // Wait for it to complete, so what follows is a dropped lock and not an
    // abandoned acquisition.
    let asked = std::time::Instant::now();
    let mut acquired = false;
    while asked.elapsed() < Duration::from_secs(25) && !acquired {
        for frame in &alice
            .collect_pushes(Duration::from_millis(500))
            .unwrap_or_default()
        {
            if let Some((event, _)) = eve_bot::notification(frame)
                && event == "OnTarget"
            {
                acquired = true;
            }
        }
    }
    assert!(acquired, "the lock never completed, so this tests nothing");

    // Bob leaves. A pod's targeting range is tens of kilometres; 1000 km is
    // unambiguously outside it.
    bob.call(
        "slash",
        "SlashCmd",
        vec![eve_marshal::PyValue::Bytes(b"/tp 1000".to_vec())],
    )
    .expect("the teleport is accepted");

    // Now nobody calls anything. The clock has to notice on its own.
    let left = std::time::Instant::now();
    let mut lost = None;
    while left.elapsed() < Duration::from_secs(15) && lost.is_none() {
        for frame in &alice
            .collect_pushes(Duration::from_millis(500))
            .unwrap_or_default()
        {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "OnTarget"
            {
                // **Decode the byte strings rather than Debug-formatting
                // them.** `format!("{args:?}")` on a `PyValue::Bytes` prints
                // the byte numbers, so matching it against "lost" can never
                // succeed -- which is what made this look like a missing push
                // when the push had arrived.
                let words: Vec<String> = args
                    .iter()
                    .filter_map(|v| match v {
                        eve_marshal::PyValue::Bytes(b) => {
                            Some(String::from_utf8_lossy(b).into_owned())
                        }
                        _ => None,
                    })
                    .collect();
                if words.iter().any(|w| w == "lost") {
                    lost = Some((left.elapsed(), words.join(", ")));
                }
            }
        }
    }

    let (after, payload) = lost.expect(
        "no OnTarget('lost') after the target left range -- the lock survives any \
         distance, which is the defect sweep_out_of_range exists to close",
    );
    eprintln!(
        "range: lock dropped {:.1} s after the target left, payload {}",
        after.as_secs_f64(),
        &payload[..payload.len().min(160)]
    );
    assert!(
        payload.contains("OutOfRange"),
        "the reason should be OutOfRange so the client can say why; got {payload:?}"
    );

    drop(alice);
    drop(bob);
}

/// **human-checks §7 step 14, and its `[A]` is wrong.**
///
/// That step says: *"Alice warps a long way, Bob watches from his overview.
/// Expected to be wrong, in a specific way: Alice's bracket will **jump to
/// the destination the moment she starts**, rather than travelling. The
/// server has no way to schedule the arrival message, so it records the ship
/// as already arrived."*
///
/// Two things in that prediction stopped being true, and they are different.
/// The server *can* schedule now (§36), so there is an arrival batch. But the
/// bracket was never going to jump for an observer who is **already in
/// space**, and the reason has nothing to do with scheduling: Alice's warp
/// goes out as a Destiny `WarpTo` *action*, addressed to everyone in the
/// system, and Bob's own client runs the same warp simulation on her ball
/// that hers does. §30.3's teleport is in the server's *record*, which only
/// reaches somebody who binds a ballpark mid-flight -- the case
/// `a_pilot_who_binds_mid_warp_sees_the_warping_ship_already_parked`
/// measures.
///
/// So the assertion here is the one that distinguishes those: Bob is told
/// `WarpTo` for Alice's ball and is **not** told a new position for it.
/// `SetBallPosition` would be the teleport the step predicted, and it is the
/// thing `/tp` uses precisely because it moves a ball without any state
/// change (§service::space's note on it).
///
/// What is left for a human is the overview *drawing* it, which no bot can
/// answer.
#[test]
fn a_watching_bot_is_told_the_warp_as_an_action_not_as_a_teleport() {
    let addr = start_two();

    // Bob first and left idle, which is the half that matters: he must learn
    // about Alice's warp without calling anything himself. A bot that polled
    // would be indistinguishable from the real client, which polls constantly
    // and hides exactly this class of defect.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");
    assert_eq!(bob_world.ego, 100_000_002);

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let alice_world = enter_far(&mut alice);
    // Bob hears the tp's own traffic first; the warp is what is measured.
    let _ = bob.collect_pushes(Duration::from_secs(1));

    // Something that is not either ship: the station or a celestial. Warping
    // at Bob would make "did Bob hear about it" ambiguous, since a target is
    // told about its own lock and follow traffic too.
    let target = alice_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id != 100_000_001 && b.ball_id != 100_000_002)
        .copied()
        .expect("a station or a celestial to warp at");

    alice
        .warp_to_item(target.ball_id, 0.0)
        .expect("alice warps");

    // Everything Bob was handed, with no call of his own in between.
    let heard = bob
        .collect_pushes(Duration::from_secs(3))
        .unwrap_or_default();
    let mut actions: Vec<(i64, String)> = Vec::new();
    for frame in &heard {
        if let Some((event, args)) = eve_bot::notification(frame)
            && event == "DoDestinyUpdate"
        {
            actions.extend(eve_bot::read_actions(&args));
        }
    }
    let names: Vec<&str> = actions.iter().map(|(_, n)| n.as_str()).collect();
    eprintln!("watching bot heard: {names:?}");

    assert!(
        names.contains(&"WarpTo"),
        "an observer already in space must be told the warp as an action, so \
         that their own client flies the ball; got {names:?}"
    );
    // **The negative, and what makes it more than a tautology.** Three action
    // names come back through this same `read_actions` path in this same
    // frame, so the detector is demonstrably live: a `SetBallPosition` in that
    // list would be returned by the code that returned `WarpTo`. What is NOT
    // done here is firing a real `SetBallPosition` to watch this assertion go
    // red -- the verb that sends one is the `/tp` console command and
    // `eve-bot` has no console. Said plainly rather than dressed up as a
    // falsification that was not run.
    assert!(
        !names.contains(&"SetBallPosition"),
        "a SetBallPosition for the warping ship IS the jumping bracket step 14 \
         predicted -- the ball would be moved with no state change and the \
         overview would redraw it at the destination; got {names:?}"
    );
    // The throttle and the tunnel travel to the observer too, and the tunnel
    // is the one that matters for somebody else's ship: `Warping.Prepare`
    // turns `effects.Warping` into `effects.WarpOut` when the effect's ship is
    // not the viewer's, so this frame is Bob's departure flash.
    assert!(
        names.contains(&"SetSpeedFraction"),
        "the observer needs the throttle too, or their copy of the simulation \
         aligns forever while hers warps; got {names:?}"
    );

    drop(alice);
    drop(bob);
}

/// **human-checks §7 step 4's real observable, which is the shell and not the
/// angle.**
///
/// That step says to look for *"Alice's bracket **at the station**, about an
/// orbit radius from it"*, that being on the **wrong side** of it is expected
/// and fine, and that *"a real failure here is Alice kilometres **away** from
/// the station, which is what the old straight-line integration did -- 23.8 km
/// after a minute"*.
///
/// So the assertion is a distance to the station and nothing about direction.
/// `Motion::tethered_to`'s own doc draws the same line: it keeps the angle the
/// ship last had and fixes the distance, which bounds the error at `range`
/// instead of at speed times elapsed time. Asserting a side would be asserting
/// the part the server is never told.
///
/// **No waiting is needed, and that is worth explaining.** The old defect grew
/// with elapsed time, so the obvious test is "orbit for a minute and measure".
/// But the tether is applied on *every* integration, so one advance with any
/// `dt > 0` is enough: the un-tethered answer here is the ship still at its
/// **undock distance** from the station, about 16 km, and the tethered answer
/// is the orbit range. Those differ by kilometres from the first tick, so a
/// test that waits a minute would only make the same point more slowly.
///
/// The measurement is taken from **Bob's** park rather than Alice's, for the
/// reason `two_bots_in_one_system_each_see_the_other_s_ship` gives: a bot's
/// own ego ball is in every state it is sent, so asserting on it proves
/// nothing about the server's record of somebody else.
#[test]
fn an_orbiting_ship_reads_as_on_the_shell_not_at_its_undock_distance() {
    /// Far enough from the undock distance that the two answers cannot be
    /// confused, and a range a player might actually type.
    const RANGE: f64 = 2_500.0;

    let gap = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };

    let addr = start_two();

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let alice_world = alice
        .enter_space(30_000_001)
        .expect("alice reads the world");

    // **The station, by id range.** Station ids are 60000000+ and nothing else
    // in this park is; the celestials are 40000000+ and the ships 100000000+.
    // Picked this way rather than by radius because a planet's radius is
    // larger than a station's and "the biggest ball" would be a trap.
    let station = alice_world
        .park
        .balls
        .iter()
        .find(|b| (60_000_000..70_000_000).contains(&b.ball_id))
        .copied()
        .expect("the station ball should be in the park");
    let ego = *alice_world.ego_ball().expect("alice's own ball");
    let undocked_at = gap(ego.position, station.position);
    assert!(
        undocked_at > RANGE * 2.0,
        "the premise of this test is that the undock distance and the orbit \
         range are far apart; undocked at {undocked_at:.1} m, range {RANGE} m"
    );

    alice
        .orbit(station.ball_id, RANGE)
        .expect("alice orbits the station");

    // Bob binds afterwards, so his bootstrap is built from the server's record
    // of Alice -- integrated, and tethered, at bind time.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");

    let alice_ball = bob_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == 100_000_001)
        .copied()
        .expect("alice's ship should be in bob's park");
    let on_shell = gap(alice_ball.position, station.position);
    // **The commanded range is the gap between the hulls**, so the shell this
    // asserts is `RANGE` plus both radii -- and the radii come off the wire,
    // out of the very park blob the client reads, rather than from a constant
    // we could get wrong in the same direction twice. This used to compare
    // `on_shell` against `RANGE` directly and passed for as long as the bug
    // described in `docs/port-tracker.md` §54.4 existed: a ship orbiting the
    // station at 2.5 km was being placed 13 km inside it.
    let hulls = f64::from(alice_ball.radius) + f64::from(station.radius);
    let clear = on_shell - hulls;
    eprintln!(
        "orbit shell: alice undocked {undocked_at:.1} m from the station, \
         orbiting at {RANGE} m, and reads {on_shell:.1} m from its centre in \
         bob's park -- {clear:.1} m of space past {hulls:.1} m of hulls"
    );

    // One metre of slack on a 2.5 km shell: `tethered_to` sets the distance
    // exactly, so this is float noise and not a tolerance for drift.
    assert!(
        (clear - RANGE).abs() < 1.0,
        "an orbiting ship must read as on its own shell: {clear:.1} m of space \
         between the hulls against a commanded {RANGE} m, at {on_shell:.1} m \
         from the station's centre. The old straight-line integration left it \
         at its undock distance and then sailed off; {undocked_at:.1} m is \
         what that looked like here"
    );

    drop(bob);
    drop(alice);
}

/// **human-checks §7 step 5, answered by the observer a human does not have.**
///
/// That step says: *"Alice follows Bob, Bob flies, a third observer is not
/// available so just watch from Bob's client. Look for: Alice behind Bob at
/// roughly the follow range. Wrong: Alice drifting away from Bob over time."*
///
/// Watching from Bob's client is what cannot answer it: Bob is the followed
/// ship, and a bot's -- or a client's -- own ego ball is in every state it is
/// sent, so it proves nothing about the server's record of somebody else.
/// Carol binds last and reads both ships out of one bootstrap.
///
/// The assertion is the **separation**, not the side, for the same reason as
/// the orbit test: `Motion::tethered_to` keeps the angle the ship last had and
/// fixes the distance, so a side would be asserting the thing the server is
/// never told. "Behind Bob" is the part that stays `[A]` and human.
///
/// `Follow` and `Orbit` are different `Mode` variants reaching the same
/// tether, which is why this is worth a test of its own rather than being
/// assumed from the orbit one: a bug that read `Follow`'s range from the wrong
/// field would pass there and fail here.
#[test]
fn a_following_ship_reads_at_its_follow_range_from_the_ship_it_follows() {
    const RANGE: f64 = 1_200.0;

    let gap = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };

    let addr = start_three();

    // Bob first: he has to be in space before anybody can be told to follow
    // his ball.
    let mut bob = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("bob connects");
    bob.handshake().expect("bob handshake");
    bob.select_character(90_000_002).expect("bob is Bob");
    bob.undock().expect("bob undocks");
    let bob_world = bob.enter_space(30_000_001).expect("bob reads the world");
    assert_eq!(bob_world.ego, 100_000_002);

    let mut alice = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("alice connects");
    alice.handshake().expect("alice handshake");
    alice.select_character(90_000_001).expect("alice is Alice");
    alice.undock().expect("alice undocks");
    let alice_world = alice
        .enter_space(30_000_001)
        .expect("alice reads the world");
    let alice_start = *alice_world.ego_ball().expect("alice's own ball");
    let bob_ball_then = alice_world
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == 100_000_002)
        .copied()
        .expect("bob's ship in alice's park");
    let apart_before = gap(alice_start.position, bob_ball_then.position);

    alice
        .follow(100_000_002, RANGE)
        .expect("alice follows bob's ship");

    // Carol: the bystander, binding last, reading the server's record of both.
    let mut carol = eve_bot::Conn::connect(addr, Duration::from_secs(10)).expect("carol connects");
    carol.handshake().expect("carol handshake");
    carol.select_character(90_000_003).expect("carol is Carol");
    carol.undock().expect("carol undocks");
    let seen = carol
        .enter_space(30_000_001)
        .expect("carol reads the world");
    assert_eq!(seen.ego, 100_000_003, "carol's ego is carol's ship");

    let find = |id: i64| {
        seen.park
            .balls
            .iter()
            .find(|b| b.ball_id == id)
            .copied()
            .unwrap_or_else(|| panic!("ball {id} should be in carol's park"))
    };
    let a = find(100_000_001);
    let b = find(100_000_002);
    let apart = gap(a.position, b.position);
    // Hull gap, not centre separation -- see the orbit test above and
    // `docs/port-tracker.md` §54.4. Two frigates are small enough that this
    // correction is tens of metres rather than the orbit test's thirteen
    // kilometres, which is exactly why nobody noticed it here.
    let hulls = f64::from(a.radius) + f64::from(b.radius);
    let clear = apart - hulls;
    eprintln!(
        "follow range: alice and bob were {apart_before:.1} m apart at undock, \
         alice follows at {RANGE} m, and carol reads {apart:.1} m between \
         centres -- {clear:.1} m of space past {hulls:.1} m of hulls"
    );

    assert!(
        (clear - RANGE).abs() < 1.0,
        "a following ship must read at its follow range from the ship it \
         follows: {clear:.1} m of space between the hulls against a commanded \
         {RANGE} m, at {apart:.1} m between centres (they were \
         {apart_before:.1} m apart before the command). Drifting away over \
         time is what step 5 calls the wrong answer, and an untethered \
         straight-line cruise is how it happens"
    );

    drop(carol);
    drop(alice);
    drop(bob);
}

/// **human-checks §7 step 7, and it found a defect the unit tests could not.**
///
/// That step says to align to the station and look for the ship turning toward
/// it and **flying past** it rather than stopping at it, with the marked `[A]`
/// being that the throttle jumps to full. None of it could happen: `CmdAlignTo`
/// did **nothing at all** for a real client, silently, and had for the whole
/// life of the feature.
///
/// The cause is one slot. A marshal dict is stored in wire order and each pair
/// is `(value, key)` -- `A = ReadObject(); B = ReadObject();
/// PyDict_SetItem(dict, B, A)`, confirmed from `0x1802dd1d0` and written on
/// `eve_marshal::PyValue::Dict` since the parser was built -- and
/// `space_state::kwarg` matched the name against the *value*. So
/// `CmdAlignTo(dstID=60012526)` arrived as `[(Int(60012526), Bytes("dstID"))]`,
/// matched nothing, and `align` returned early. Measured over this socket
/// before the fix: wire order produced `[]`, and the `(key, value)` order the
/// unit tests hand-build produced `["GotoDirection", "SetSpeedFraction"]`.
///
/// **Every unit test of a keyword command passed throughout**, because each
/// built its kwargs in the order the function wanted rather than the order the
/// wire uses. That is why this test is in `eve-bot` and not beside them: it
/// goes through the real encoder, the real socket and the real parser, so it
/// cannot agree with the server about a shape they are both wrong about.
/// `docs/port-tracker.md` §39.
///
/// What it asserts, which is step 7's own list minus the part only eyes can
/// do:
///
/// - the command produces actions at all;
/// - a `GotoDirection` and **not** a `GotoPoint` -- "flies past it, not up to
///   it and stops" is the whole difference between align and approach, and in
///   this server it is the difference between those two action names;
/// - the heading actually points at the thing, by dot product;
/// - the throttle operand is exactly `1.0`, which turns step 7's `[A]` from a
///   decision into a measurement.
#[test]
fn an_align_sent_the_way_the_client_sends_it_reaches_the_server() {
    let (addr, _server) = start();
    let mut bot = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("the bot connects");
    bot.handshake().expect("handshake");
    bot.select_character(90_000_001).expect("a character");
    bot.undock().expect("undock");
    let world = bot.enter_space(30_000_001).expect("read the world");
    let ego = *world.ego_ball().expect("our own ball");
    let station = world
        .park
        .balls
        .iter()
        .find(|b| (60_000_000..70_000_000).contains(&b.ball_id))
        .copied()
        .expect("the station ball");

    let out = bot
        .align_to(station.ball_id)
        .expect("the align is answered");

    let mut actions: Vec<(i64, String, Vec<eve_marshal::PyValue>)> = Vec::new();
    let mut collect = |frames: &[eve_marshal::PyValue]| {
        for frame in frames {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "DoDestinyUpdate"
            {
                actions.extend(eve_bot::read_action_list(&args));
            }
        }
    };
    collect(&out.notifications);
    let later = bot
        .collect_pushes(Duration::from_millis(1500))
        .unwrap_or_default();
    collect(&later);

    let names: Vec<&str> = actions.iter().map(|(_, n, _)| n.as_str()).collect();
    eprintln!("align: server pushed {names:?}");
    assert!(
        !actions.is_empty(),
        "a CmdAlignTo sent the way the client sends it must do something. \
         Nothing here is the defect this test was written for: the kwarg was \
         read out of the wrong slot and the command returned early"
    );

    // Align, not approach.
    assert!(
        names.contains(&"GotoDirection"),
        "an align is a heading, so it must come back as GotoDirection; \
         got {names:?}"
    );
    assert!(
        !names.contains(&"GotoPoint"),
        "a GotoPoint would stop the ship AT the station, which is the \
         approach command and not this one; got {names:?}"
    );

    // And the heading points at it. Operands are `(ball, x, y, z)`.
    let (_, _, goto) = actions
        .iter()
        .find(|(_, n, _)| n == "GotoDirection")
        .expect("just asserted present");
    let heading = [
        eve_bot::real_at(goto, 1).expect("x"),
        eve_bot::real_at(goto, 2).expect("y"),
        eve_bot::real_at(goto, 3).expect("z"),
    ];
    let to_station = [
        station.position[0] - ego.position[0],
        station.position[1] - ego.position[1],
        station.position[2] - ego.position[2],
    ];
    let norm = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let dot =
        (heading[0] * to_station[0] + heading[1] * to_station[1] + heading[2] * to_station[2])
            / (norm(heading) * norm(to_station));
    eprintln!("align: heading-to-station dot product {dot:.6}");
    assert!(
        dot > 0.999_999,
        "the heading must point at the station: dot product {dot}, heading \
         {heading:?}, station at {:?} from a ship at {:?}",
        station.position,
        ego.position
    );

    // **Step 7's `[A]`, measured.** "We decided an align implies full speed"
    // is now a number rather than a decision, and a human can disagree with
    // the number instead of guessing what we did.
    let (_, _, throttle) = actions
        .iter()
        .find(|(_, n, _)| n == "SetSpeedFraction")
        .expect("an align must carry the throttle, or the ship never accelerates");
    let fraction = eve_bot::real_at(throttle, 1).expect("the fraction operand");
    eprintln!("align: throttle set to {fraction}");
    assert!(
        (fraction - 1.0).abs() < 1e-9,
        "step 7 says the throttle jumps to full on an align and that this is \
         our decision; it is {fraction}. If this ever becomes 'leave it where \
         the player put it', that step and this assertion change together"
    );

    // **Step 7's last line -- "aligning to a planet does nothing at all" --
    // is probably stale, and this test cannot say so.** `CmdAlignTo` resolves
    // its destination through `Registry::ball_position`, which answers for
    // every ball in the park, and a real run's park holds the system's 13
    // warpable celestials (`warp_blob` listed them). So a planet should be as
    // alignable as the station.
    //
    // Not asserted, because **this fixture's park has no celestials in it**:
    // the first version of this block looked for a ball in the 40000000 range
    // and panicked with "a celestial should be in the park". Asserting it from
    // here would have meant either a conditional check that passes by being
    // skipped, or a claim about a park this test does not have. What settles
    // it is a client scenario -- `warp_blob` already has the celestial list in
    // hand -- or a fixture that seeds them.

    drop(bot);
}

/// **The second keyword argument in the server, end to end.**
///
/// §39 fixed `kwarg` reading the wrong slot of a marshal dict pair, and the
/// sweep found exactly two consumers: `CmdAlignTo`'s `dstID` and
/// `CmdWarpToStuff`'s `minRange`. The first is covered by
/// `an_align_sent_the_way_the_client_sends_it_reaches_the_server`. This is the
/// second, and writing it turned up a third mistake of the same family: this
/// crate's own `warp_to_item` sent `minRange` as a **third positional**, which
/// the server never reads.
///
/// **Both bugs hid behind the same number.** Every caller asked for `0.0`, and
/// `0.0` is exactly what `min_range.max(radius + WARP_STANDOFF)` produces when
/// the request is ignored -- so a dropped argument and an honoured one agreed.
/// A request *larger* than the standoff floor is the only one that can tell
/// them apart, which is what this asks for.
///
/// `read_action_list` gives the operands, and `WarpTo`'s are
/// `(ball, x, y, z, minRange, warpFactor)` (`destiny::Action::args`), so the
/// assertion is on the number the server actually put in the action rather
/// than on the action's name.
#[test]
fn a_warps_min_range_keyword_reaches_the_server_and_wins_over_the_floor() {
    /// Comfortably past any station's radius plus `WARP_STANDOFF`, so
    /// `max` has to pick this and not the floor.
    const WITHIN: f64 = 100_000.0;

    let (addr, _server) = start();
    let mut bot = eve_bot::Conn::connect(addr, Duration::from_secs(20)).expect("the bot connects");
    bot.handshake().expect("handshake");
    bot.select_character(90_000_001).expect("a character");
    bot.undock().expect("undock");
    let world = enter_far(&mut bot);
    let station = world
        .park
        .balls
        .iter()
        .find(|b| (60_000_000..70_000_000).contains(&b.ball_id))
        .copied()
        .expect("the station ball");
    let floor = f64::from(station.radius) + eve_server::space_state::WARP_STANDOFF;
    assert!(
        WITHIN > floor,
        "the premise: the requested range has to beat the floor or this test \
         cannot tell an honoured keyword from an ignored one. Requested \
         {WITHIN} m against a floor of {floor:.1} m"
    );

    let out = bot
        .warp_to_item(station.ball_id, WITHIN)
        .expect("the warp is answered");
    let mut actions: Vec<(i64, String, Vec<eve_marshal::PyValue>)> = Vec::new();
    // The reply's own frames, then a window for the rest of the batch -- the
    // same two-step `the_bot_can_warp_and_the_server_sends_the_whole_batch`
    // uses. Reading only the reply found no `WarpTo` and looked like the
    // warp had failed.
    let mut collect = |frames: &[eve_marshal::PyValue]| {
        for frame in frames {
            if let Some((event, args)) = eve_bot::notification(frame)
                && event == "DoDestinyUpdate"
            {
                actions.extend(eve_bot::read_action_list(&args));
            }
        }
    };
    collect(&out.notifications);
    let later = bot
        .collect_pushes(Duration::from_millis(1500))
        .unwrap_or_default();
    collect(&later);
    let names: Vec<&str> = actions.iter().map(|(_, n, _)| n.as_str()).collect();
    let (_, _, warp) = actions
        .iter()
        .find(|(_, n, _)| n == "WarpTo")
        .unwrap_or_else(|| panic!("the warp must produce a WarpTo action; got {names:?}"));
    let sent = eve_bot::real_at(warp, 4).expect("the minRange operand");
    eprintln!("warp minRange: asked {WITHIN}, floor {floor:.1}, action carries {sent}");

    assert!(
        (sent - WITHIN).abs() < 1.0,
        "the client's minRange must win over the standoff floor: asked \
         {WITHIN} m, floor {floor:.1} m, action carries {sent} m. Getting the \
         floor back means the keyword was not read -- either the §39 slot bug \
         or a minRange sent as a positional, and this test was written after \
         finding both"
    );

    drop(bot);
}
