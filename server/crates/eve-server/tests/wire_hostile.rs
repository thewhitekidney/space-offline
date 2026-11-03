//! Failing and hostile peers against real loopback listeners: the
//! connection lifecycle (`docs/review/V2-wire.md` F3, F4;
//! `docs/review/V6-security-ops.md` F9; `docs/review/plan-review.md` M6).
//!
//! Every cap here is injected through `Listener::cap`, never the default
//! `LOCAL_CAP` or `REMOTE_CAP`, which are assumptions.

use eve_proto::write_frame;
use eve_server::dispatch::DispatchError;
use eve_server::server::{
    ACCEPT_BACKOFF, Config, DispatchFactory, Listener, Mode, Server, accept_loop,
};
use eve_server::{Dispatch, Reply};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().expect("literal")
}

/// Wait for `cond`, or fail after a generous deadline: accepts and session
/// bookkeeping happen on the server's threads.
fn eventually(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Bind and accept on every listener, forever, on background threads.
fn serve_forever(config: Config) -> Arc<Server> {
    let server = Arc::new(Server::bind(config).expect("bind"));
    let runner = Arc::clone(&server);
    std::thread::spawn(move || runner.run());
    server
}

fn connect(addr: SocketAddr) -> TcpStream {
    TcpStream::connect(addr).expect("connect")
}

/// The server closed this stream: a read sees end of file or a reset, and
/// does so well before any idle timeout.
fn assert_closed(mut s: TcpStream, what: &str) {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Ok(0) => {}
        Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
        other => panic!("{what}: expected a close, got {other:?}"),
    }
}

/// The server is holding this stream open: a short read times out.
fn assert_open(s: &mut TcpStream, what: &str) {
    s.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut b = [0u8; 1];
    match s.read(&mut b) {
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) => {}
        other => panic!("{what}: expected an open, silent stream, got {other:?}"),
    }
}

/// A dispatch whose every payload panics, as an unaudited arm might.
struct Panics;

impl Dispatch for Panics {
    fn on_payload(&mut self, _payload: &[u8]) -> Result<Reply, DispatchError> {
        panic!("wire_hostile: a dispatch arm panicked, deliberately");
    }

    fn state(&self) -> String {
        "panics".to_owned()
    }
}

/// F3: the session is taken out of the world when its thread unwinds. It
/// used to stay, a ghost other players saw online.
#[test]
fn a_panicking_dispatch_leaves_no_session() {
    let mut config = Config::new(loopback());
    config.test_dispatch = Some(DispatchFactory(Arc::new(|| Box::new(Panics))));
    let server = serve_forever(config);
    let world = server.world();
    let sessions = || {
        world
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .sessions()
            .len()
    };

    let mut s = connect(server.local_addr().unwrap());
    eventually("the session to open", || sessions() == 1);
    let mut framed = Vec::new();
    write_frame(b"anything", &mut framed).expect("frame");
    s.write_all(&framed).expect("write");

    eventually("the panicking connection's session to close", || {
        sessions() == 0
    });
    eventually("its cap slot to be released", || server.live() == [0]);
    assert_closed(s, "the panicked connection");
}

/// F4: no accept error ends the loop. Out of descriptors (`EMFILE`,
/// `ENFILE`) also backs off rather than spinning.
#[test]
fn accept_errors_do_not_end_the_loop() {
    const EMFILE: i32 = 24; // [V] Linux asm-generic/errno-base.h
    const ENFILE: i32 = 23;
    let mut script: VecDeque<io::Result<u32>> = VecDeque::from([
        Err(io::Error::from_raw_os_error(EMFILE)),
        Err(io::Error::from(io::ErrorKind::ConnectionAborted)),
        Ok(1),
        Err(io::Error::from_raw_os_error(ENFILE)),
        Err(io::Error::from(io::ErrorKind::Interrupted)),
        Ok(2),
    ]);
    let mut served = Vec::new();
    let start = Instant::now();
    accept_loop("wire_hostile", || script.pop_front(), |n| served.push(n));
    assert_eq!(served, [1, 2], "every connection after an error is served");
    assert!(
        start.elapsed() >= ACCEPT_BACKOFF,
        "running out of descriptors must back off, not spin"
    );
}

/// F9: past the cap a stream is closed at once, not left in the backlog;
/// and a slot given back is a slot the next client gets.
#[test]
fn connections_over_the_cap_are_closed_not_queued() {
    const CAP: usize = 2;
    let mut config = Config::new(loopback());
    config.mode = Mode::Capture;
    config.listeners[0].cap = Some(CAP);
    let server = serve_forever(config);
    let addr = server.local_addr().unwrap();

    let mut held: Vec<TcpStream> = (0..CAP).map(|_| connect(addr)).collect();
    eventually("the cap to fill", || server.live() == [CAP]);
    assert_closed(connect(addr), "a connection over the cap");
    assert_eq!(server.live(), [CAP], "a refusal takes no slot");
    for s in &mut held {
        assert_open(s, "a connection under the cap");
    }

    drop(held.pop());
    eventually("the closed connection's slot", || {
        server.live() == [CAP - 1]
    });
    let mut again = connect(addr);
    eventually("the freed slot to be taken", || server.live() == [CAP]);
    assert_open(&mut again, "a connection in a freed slot");
}

/// M6: caps are per listener, so a bot fleet filling its own listener never
/// locks the friends out of theirs. (The friends' listener is a second
/// loopback one here: `ceiling=player` is refused until accounts exist.)
#[test]
fn a_full_bot_listener_does_not_refuse_the_friends_listener() {
    const BOTS: usize = 2;
    const FRIENDS: usize = 1;
    let mut config = Config::new(loopback());
    config.mode = Mode::Capture;
    config.listeners = vec![
        Listener {
            cap: Some(BOTS),
            ..Listener::new(loopback())
        },
        Listener {
            cap: Some(FRIENDS),
            ..Listener::new(loopback())
        },
    ];
    let server = serve_forever(config);
    let addrs = server.local_addrs().unwrap();

    let _bots: Vec<TcpStream> = (0..BOTS).map(|_| connect(addrs[0])).collect();
    eventually("the bot listener to fill", || server.live() == [BOTS, 0]);
    assert_closed(connect(addrs[0]), "one bot too many");

    let mut friend = connect(addrs[1]);
    eventually("the friend to be accepted", || {
        server.live() == [BOTS, FRIENDS]
    });
    assert_open(&mut friend, "the friend beside a full bot listener");
}

/// Read until the server closes the stream (it may greet first), failing
/// after a deadline far below any idle timeout.
fn closes_soon(mut s: TcpStream, what: &str) {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut b = [0u8; 4096];
    loop {
        match s.read(&mut b) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => return,
            Err(e) => panic!("{what}: the server kept the stream open ({e})"),
        }
    }
}

/// SEC-FIX 11: a frame declared over the pre-login ceiling is refused from
/// its header; the server does not wait for (buffer) its body.
#[test]
fn a_pre_login_frame_over_the_ceiling_is_refused_at_its_header() {
    let server = serve_forever(Config::new(loopback()));
    let mut s = connect(server.local_addr().unwrap());
    let declared = (eve_server::PREAUTH_MAX_FRAME as u32) + 1;
    // The header and a few body bytes, nothing more: a server that waits for
    // the rest never closes.
    let mut bytes = declared.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 16]);
    s.write_all(&bytes).expect("write");
    closes_soon(s, "an oversize pre-login frame");
}

/// SEC-FIX 11: the zlib flag is refused from the header, at any size.
#[test]
fn a_zlib_frame_is_refused_at_its_header_not_after_its_body() {
    let server = serve_forever(Config::new(loopback()));
    let mut s = connect(server.local_addr().unwrap());
    // Bit 29 of word0 (`docs/reference/framing.md`).
    const FLAG_ZLIB: u32 = 0x2000_0000;
    let word0 = FLAG_ZLIB | 5_000_000;
    let mut bytes = word0.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0u8; 16]);
    s.write_all(&bytes).expect("write");
    closes_soon(s, "a zlib frame");
}
