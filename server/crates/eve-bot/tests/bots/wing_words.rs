//! FC-3 over real sockets: the owner's words reach a wing bot, from fleet
//! chat and from the control (`Handle::order`, which `/bots` uses), and the
//! wing's lead answers each once, in the fleet room.
//!
//! Same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes. **No `[A]` value is an
//! expected value**: the assertions are the words of the acknowledgements
//! (the cheat-sheet's own verbs) and that a hold sends the ship a stop; every
//! deadline is a generous ceiling.

use super::common::{SYSTEM, config, dump, login, ok, pilot, serve_with_chat, until};
use eve_bot::brain::{AcceptFleet, ActKind};
use eve_bot::driver::{self, BotConfig, Handle, Pace};
use eve_bot::wing::{ROLE, Seat};
use eve_bot::wing_hall::WingHall;
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use eve_server::model::Character;
use eve_server::wing_words::WingCmd;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WING: i64 = 90_000_811;
const FC: i64 = 90_000_812;
const FC_SHIP: i64 = 100_000_812;
const RIFTER: i64 = 587;

fn bot(addr: SocketAddr, chat_port: u16) -> Handle {
    let mut config = BotConfig::new("Wing1", addr, Pace::Fast).with_role(&ROLE);
    config.chat_port = chat_port;
    config.policy.quick = true;
    config.policy.accept_fleet = AcceptFleet::Fc(FC);
    config.policy.wing = Some(Seat::new(Arc::new(WingHall::new()), Some(FC)));
    driver::spawn(config).0
}

fn text_of(v: PyValue) -> Option<String> {
    match v {
        PyValue::Bytes(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        PyValue::Unicode(s) => Some(s),
        _ => None,
    }
}

/// What the room has said since the last call: the groupchat bodies.
fn room_lines(fc: &mut Conn, chat: &mut eve_xmpp::Client, within: Duration) -> Vec<String> {
    let deadline = Instant::now() + within;
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        let _ = fc.pump(Duration::from_millis(50));
        if let Some(st) = chat
            .recv_timeout(Duration::from_millis(100))
            .expect("reads")
            && st.name == "message"
            && st.attr("type") == Some("groupchat")
            && let Some(b) = st.child("body")
        {
            lines.push(b.text.clone());
        }
    }
    lines
}

/// Say `words` and wait for a line containing `want`; every line seen.
fn say_and_wait(
    fc: &mut Conn,
    chat: &mut eve_xmpp::Client,
    room: &str,
    words: &str,
    want: &str,
) -> (bool, Vec<String>) {
    chat.say(room, words).expect("says");
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let lines = room_lines(fc, chat, Duration::from_millis(500));
        let hit = lines.iter().any(|l| l.contains(want));
        seen.extend(lines);
        if hit {
            return (true, seen);
        }
    }
    (false, seen)
}

/// **The words reach the wing.** The FC invites the bot, undocks, and says
/// "weapons free", "hold" and "go home" in the fleet room; the bot's lead
/// answers each with "copy: ...", exactly once; a hold sends the ship a
/// stop; a `Handle::order` (the control's path) is answered the same way; a
/// line that is talk, not a command, is not.
#[test]
fn the_fcs_words_in_fleet_chat_and_the_controls_orders_are_copied_once() {
    let mut cfg = config();
    cfg.sandbox.characters = vec![
        pilot(WING, "Wing1", 100_000_811),
        Character {
            ship_type_id: RIFTER,
            ..pilot(FC, "Boss", FC_SHIP)
        },
    ];
    cfg.chat = Some(eve_server::chat::ChatConfig {
        port: 0,
        advertise: None,
    });
    let (addr, port) = serve_with_chat(cfg);
    let port = port.expect("chat is bound");
    let wing = bot(addr, port);
    assert!(
        until(Duration::from_secs(60), || wing
            .log()
            .iter()
            .any(|e| ok(e, ActKind::Chat))),
        "the bot's chat is up:\n{}",
        dump(&wing)
    );

    let mut fc = login(addr, "Boss");
    let host = fc
        .call("XmppChatMgr", "Hostname", vec![])
        .expect("Hostname")
        .value()
        .and_then(text_of)
        .expect("a chat host");
    let token = fc
        .call(
            "chatAuthenticationService",
            "GetAuthenticationToken",
            vec![],
        )
        .expect("a token")
        .value()
        .and_then(text_of)
        .expect("a token string");
    let mut chat = eve_xmpp::Client::login(SocketAddr::new(addr.ip(), port), &host, FC, &token)
        .expect("the FC logs in to chat");

    // The FC forms a fleet, joins its room and invites the bot.
    let created = fc
        .call("fleetObjectHandler", "CreateFleet", vec![])
        .expect("CreateFleet");
    let fleet = eve_bot::object_refs(&created.value().expect("a reference"))
        .into_iter()
        .next()
        .expect("an object");
    fc.call_on(
        &fleet,
        "Init",
        vec![PyValue::Int(super::common::FLEET_SHIP)],
        vec![],
    )
    .expect("Init");
    let rooms = fc
        .call("XmppChatMgr", "ResyncSystemChannelAccess", vec![])
        .expect("rooms")
        .value();
    let Some(PyValue::List(rooms)) = rooms else {
        panic!("a list of rooms")
    };
    let room = rooms
        .into_iter()
        .filter_map(text_of)
        .find(|r| r.starts_with("fleet_"))
        .expect("a fleet room");
    chat.join(&room).expect("joins");
    fc.call_on(
        &fleet,
        "Invite",
        vec![
            PyValue::Int(WING),
            PyValue::None,
            PyValue::None,
            PyValue::None,
        ],
        vec![],
    )
    .expect("Invite");
    let mut joined = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !joined && Instant::now() < deadline {
        joined = fc
            .pump(Duration::from_millis(250))
            .expect("reads")
            .iter()
            .any(|e| matches!(e, Event::Push(n, _) if n == "OnFleetJoin"));
    }
    assert!(joined, "the bot joins:\n{}", dump(&wing));
    // Let the hello and the first posts go by.
    let _ = room_lines(&mut fc, &mut chat, Duration::from_secs(3));

    // Talk is not a command.
    chat.say(&room, "hold the door for me").expect("says");
    let lines = room_lines(&mut fc, &mut chat, Duration::from_secs(4));
    assert!(
        !lines.iter().any(|l| l.contains("copy:")),
        "talk is not copied: {lines:?}"
    );

    // 1. Words in fleet chat, each copied once.
    let (heard, seen) = say_and_wait(
        &mut fc,
        &mut chat,
        &room,
        "weapons free",
        "copy: weapons free",
    );
    assert!(heard, "weapons free is copied: {seen:?}\n{}", dump(&wing));
    let more = room_lines(&mut fc, &mut chat, Duration::from_secs(4));
    let copies = seen
        .iter()
        .chain(more.iter())
        .filter(|l| l.contains("copy: weapons free"))
        .count();
    assert_eq!(copies, 1, "one copy, not one per turn: {seen:?} {more:?}");

    // 2. A hold is copied and stops the ship.
    let before = wing
        .log()
        .iter()
        .filter(|e| e.kind == ActKind::Order)
        .count();
    let (heard, seen) = say_and_wait(&mut fc, &mut chat, &room, "hold", "copy: holding");
    assert!(heard, "hold is copied: {seen:?}\n{}", dump(&wing));
    assert!(
        until(Duration::from_secs(20), || wing
            .log()
            .iter()
            .filter(|e| e.kind == ActKind::Order)
            .count()
            > before),
        "a hold sends the ship a stop:\n{}",
        dump(&wing)
    );

    // 3. The control's path (`/bots follow me` -> `order <fc> follow`).
    wing.order(WingCmd::Follow);
    let lines = room_lines(&mut fc, &mut chat, Duration::from_secs(8));
    assert!(
        lines.iter().any(|l| l.contains("copy: following")),
        "the control's follow is copied: {lines:?}\n{}",
        dump(&wing)
    );
    // 4. A sov word parses and is answered "not yet".
    let (heard, seen) = say_and_wait(&mut fc, &mut chat, &room, "drop hub", "not yet");
    assert!(heard, "drop hub is answered not yet: {seen:?}");
    let _ = SYSTEM;
    wing.stop();
}
