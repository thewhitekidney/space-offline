//! Log in, become a character, and hold the session open. Milestone 1 of
//! `docs/archive/plan-bots.md`.
//!
//! Usage: `eve-bot --connect IP:PORT [--character-id ID] [--undock]
//!         [--solar-system ID] [--hold SECONDS]`
//!
//! Prints what it did and exits non-zero if the handshake failed, so a harness
//! can read it like any other check.

use std::net::SocketAddr;
use std::time::Duration;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let mut connect = String::from("127.0.0.1:26000");
    let mut hold = Duration::from_secs(0);
    // The sandbox's first character. There is no "list the characters" call on
    // our server, so this has to be supplied rather than discovered.
    let mut character_id: i64 = 90_000_001;
    // Off by default: a bot that only logs in is milestone 1 and still useful.
    let mut undock = false;
    let mut solar_system_id: i64 = 30_000_001;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--connect" => match args.next() {
                Some(v) => connect = v,
                None => return fail("--connect needs IP:PORT"),
            },
            "--hold" => match args.next().and_then(|v| v.parse::<u64>().ok()) {
                Some(secs) => hold = Duration::from_secs(secs),
                None => return fail("--hold needs a whole number of seconds"),
            },
            "--character-id" => match args.next().and_then(|v| v.parse::<i64>().ok()) {
                Some(id) => character_id = id,
                None => return fail("--character-id needs a whole number"),
            },
            "--undock" => undock = true,
            "--solar-system" => match args.next().and_then(|v| v.parse::<i64>().ok()) {
                Some(id) => solar_system_id = id,
                None => return fail("--solar-system needs a whole number"),
            },
            other => return fail(&format!("unknown argument {other:?}")),
        }
    }

    // **An IP literal, never a name.** `to_socket_addrs` would hand a host
    // name to the system resolver, and a bot must reach nothing but the
    // server's address (`docs/guides/jail.md`, `docs/lanes/bots.md` §6). Parsing
    // is the whole of it: no lookup can happen on this path.
    let Ok(addr) = connect.parse::<SocketAddr>() else {
        return fail(&format!(
            "--connect wants an IP:PORT literal, got {connect:?} (names are not resolved)"
        ));
    };

    // Ten seconds: long enough for a loaded host, short enough that an
    // unattended bot reports rather than hangs.
    let mut conn = match eve_bot::Conn::connect(addr, Duration::from_secs(10)) {
        Ok(c) => c,
        Err(e) => return fail(&format!("connect to {addr}: {e}")),
    };
    println!("eve-bot: connected to {addr}");

    let session = match conn.handshake() {
        Ok(s) => s,
        Err(e) => return fail(&format!("handshake: {e}")),
    };
    println!(
        "eve-bot: handshake complete, server challenge {} bytes",
        session.server_challenge.len()
    );

    match conn.select_character(character_id) {
        Ok(outcome) => println!(
            "eve-bot: now character {character_id} ({} notification(s) alongside)",
            outcome.notifications.len()
        ),
        Err(e) => return fail(&format!("select character {character_id}: {e}")),
    }

    if undock {
        match conn.undock() {
            Ok(_) => println!("eve-bot: undocked"),
            Err(e) => return fail(&format!("undock: {e}")),
        }
        // The bind is what makes the world arrive; the undock alone does not.
        match conn.enter_space(solar_system_id) {
            Ok(world) => {
                println!(
                    "eve-bot: in {solar_system_id}, stamp {}, {} ball(s), ego {}",
                    world.stamp,
                    world.park.balls.len(),
                    world.ego
                );
                match world.ego_ball() {
                    Some(ball) => println!(
                        "eve-bot: ego ball at {:.1}, {:.1}, {:.1} radius {:.1} m",
                        ball.position[0], ball.position[1], ball.position[2], ball.radius
                    ),
                    // Not fatal, but worth saying loudly: a state that names an
                    // ego it never created is a camera attached to nothing.
                    None => println!(
                        "eve-bot: WARNING ego {} is not a ball in the park",
                        world.ego
                    ),
                }
            }
            Err(e) => return fail(&format!("enter space {solar_system_id}: {e}")),
        }
    }

    if !hold.is_zero() {
        // Holding the socket open is the point of milestone 1: it is what
        // makes the bot a session the server can see and address.
        println!("eve-bot: holding the session for {}s", hold.as_secs());
        std::thread::sleep(hold);
    }
    println!("eve-bot: done");
    std::process::ExitCode::SUCCESS
}

fn fail(why: &str) -> std::process::ExitCode {
    eprintln!("eve-bot: {why}");
    std::process::ExitCode::FAILURE
}
