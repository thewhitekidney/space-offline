//! A mock client drives the whole handshake over a real TCP socket.
//!
//! This is the test that matters most in this crate. The unit tests check the
//! handshake state machine against payloads constructed in memory; this one
//! puts the framing layer, the marshal codec, the capture log and the
//! connection loop in the path as well, so a mistake in any of them shows up
//! here rather than on the first real client connection — which is expensive
//! to arrange and the one chance to take a clean capture.
//!
//! It is not a substitute for the real client. It proves the pieces agree with
//! each other, not that they agree with CCP's code. The handshake payloads are
//! built to match `docs/reference/login-path.md` §4, and if that document is wrong this
//! test is wrong in the same direction.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{ConnEnd, dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

const CHALLENGE_LEN: usize = 64;

/// Reads and writes GPS payloads over a socket, doing the framing itself.
struct MockClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl MockClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect");
        // A hung read must fail the test, not wedge the suite.
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        MockClient {
            stream,
            buf: Vec::new(),
        }
    }

    fn send(&mut self, v: &PyValue) {
        let mut framed = Vec::new();
        write_frame(&dumps(v), &mut framed).expect("frame");
        self.stream.write_all(&framed).expect("write");
    }

    /// Send raw bytes as one frame, bypassing the marshal layer.
    fn send_raw_payload(&mut self, payload: &[u8]) {
        let mut framed = Vec::new();
        write_frame(payload, &mut framed).expect("frame");
        self.stream.write_all(&framed).expect("write");
    }

    /// The next frame the test cares about.
    ///
    /// **A call *from* the server is consumed here and not returned**, which
    /// is what a real client does with one: `SessionCall` runs the method and
    /// writes a response, and nothing surfaces to the code under test. The
    /// only such call this server makes is
    /// `machoNet.InitClientGlobalConfig` (`global_config`), and before this
    /// skip existed it shifted the frame sequence under **38** fixtures at
    /// once -- see `docs/port-tracker.md` §41.7 for the measurements that led
    /// here.
    ///
    /// Narrowed to that one service deliberately. An unexpected inbound call
    /// is news and fails here rather than being swallowed by a mock, and the
    /// frame's *content* is covered by `global_config`'s own unit tests
    /// rather than by its absence from these.
    fn recv(&mut self) -> PyValue {
        loop {
            let frame = self.recv_raw();
            if let PyValue::Instance { class, .. } = &frame
                && String::from_utf8_lossy(class).rsplit('.').next() == Some("CallReq")
            {
                let packet = eve_proto::MachoPacket::from_value(&frame).expect("a macho packet");
                assert_eq!(
                    packet.destination.service(),
                    Some("machoNet"),
                    "the only call this server makes is to the client's \
                     machoNet; anything else arriving here is news"
                );
                continue;
            }
            return frame;
        }
    }

    /// Every frame, including the server's own calls. For the one test that
    /// is about those.
    fn recv_raw(&mut self) -> PyValue {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((frame, consumed)) = try_split_frame(&self.buf).expect("split") {
                let payload = frame.payload.to_vec();
                self.buf.drain(..consumed);
                return loads(&payload).expect("payload parses");
            }
            let n = self.stream.read(&mut chunk).expect("read");
            assert_ne!(n, 0, "server closed while a payload was expected");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Returns true if the server closed without sending anything more.
    fn expect_closed(&mut self) -> bool {
        let mut chunk = [0u8; 64];
        matches!(self.stream.read(&mut chunk), Ok(0))
    }
}

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
    // Wire order is value-then-key.
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
}

fn dict_get<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| *k == bstr(key))
            .map(|(value, _)| value),
        _ => None,
    }
}

/// `str(crc_hqx(marshal.Save((x,)), 0))`, as the client computes it.
fn crypto_hash(args: &[PyValue]) -> PyValue {
    PyValue::Bytes(eve_server::crypto_hash(args).into_bytes())
}

fn start(
    mode: Mode,
    capture_dir: Option<std::path::PathBuf>,
) -> (SocketAddr, std::thread::JoinHandle<ConnEnd>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = mode;
    config.capture_dir = capture_dir;
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || server.run_once().expect("serve"));
    (addr, handle)
}

/// A server that serves **two** connections in sequence, on one thread.
///
/// `start` calls `run_once`, which is the right shape for every other test
/// here. This one exists because the only honest test of a shared world needs
/// a second connection to the *same* server.
fn start_twice() -> (SocketAddr, std::thread::JoinHandle<Vec<ConnEnd>>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        vec![
            server.run_once().expect("serve first"),
            server.run_once().expect("serve second"),
        ]
    });
    (addr, handle)
}

fn h2() -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(i64::from(eve_marshal::build::reference().line.secret)),
        PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
        PyValue::Int(0),
        PyValue::Real(eve_marshal::build::reference().line.version),
        PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
        bstr(eve_marshal::build::reference().line.codename_at_region),
    ])
}

fn request_dict(challenge: &[u8]) -> PyValue {
    let _ = challenge;
    dict(vec![
        (
            "macho_version",
            PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
        ),
        (
            "boot_version",
            PyValue::Real(eve_marshal::build::reference().line.version),
        ),
        (
            "boot_build",
            PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
        ),
        (
            "boot_codename",
            bstr(eve_marshal::build::reference().line.codename),
        ),
        ("boot_region", bstr("ccp")),
        ("user_name", bstr("EVE-SSO-CONNECTION")),
        ("user_password", PyValue::None),
        ("user_password_hash", PyValue::Bytes(vec![0u8; 20])),
        ("user_languageid", bstr("EN")),
        ("user_affiliateid", PyValue::Int(0)),
        ("user_sso_token", bstr("token")),
    ])
}

/// Run the client side of H1..H10. Returns the server challenge from H8 and
/// the H10 result dict.
fn full_handshake(client: &mut MockClient) -> (Vec<u8>, PyValue) {
    // H1
    let PyValue::Tuple(h1) = client.recv() else {
        panic!("H1 is not a tuple")
    };
    assert_eq!(h1.len(), 7, "H1 must be a 7-tuple");
    assert_eq!(
        h1[0],
        PyValue::Int(i64::from(eve_marshal::build::reference().line.secret))
    );
    assert_eq!(
        h1[1],
        PyValue::Int(i64::from(eve_marshal::build::reference().line.macho))
    );
    assert_eq!(
        h1[3],
        PyValue::Real(eve_marshal::build::reference().line.version),
        "boot_version must be a real"
    );
    assert_eq!(
        h1[5],
        bstr(eve_marshal::build::reference().line.codename_at_region)
    );

    client.send(&h2());
    client.send(&PyValue::Tuple(vec![
        PyValue::None,
        bstr("VK"),
        bstr("name-hash"),
    ]));
    client.send(&PyValue::Tuple(vec![
        bstr("placebo"),
        PyValue::Dict(vec![]),
    ]));

    // H5
    assert_eq!(client.recv(), bstr("OK CC"));

    // H6
    let client_challenge = vec![0x5Au8; CHALLENGE_LEN];
    client.send(&PyValue::Tuple(vec![
        PyValue::Bytes(client_challenge.clone()),
        request_dict(&client_challenge),
    ]));

    // H7
    assert_eq!(client.recv(), PyValue::Int(2), "passwordVersion must be 2");

    // H8
    let PyValue::Tuple(h8) = client.recv() else {
        panic!("H8 is not a tuple")
    };
    assert_eq!(h8.len(), 4);
    let server_challenge = match &h8[0] {
        PyValue::Bytes(b) => b.clone(),
        other => panic!("H8[0] should be the server challenge, got {other:?}"),
    };
    assert_eq!(server_challenge.len(), CHALLENGE_LEN);
    assert_eq!(h8[2], PyValue::Dict(vec![]), "eval locals must be empty");

    // The client's own first check: the response hash must match its challenge.
    assert_eq!(
        dict_get(&h8[3], "challenge_responsehash"),
        Some(&crypto_hash(&[PyValue::Bytes(client_challenge)])),
        "a mismatch here is 'Server response signature incorrect'"
    );
    // Its second check: a queue position of 2 or more aborts the login.
    match dict_get(&h8[3], "user_logonqueueposition") {
        Some(PyValue::Int(n)) => assert!(*n < 2, "queue position {n} aborts the client"),
        None => {}
        other => panic!("unexpected queue position {other:?}"),
    }

    // signedFunc must be a 2-tuple whose element 0 is a marshalled code
    // object — the client hands it to marshal.loads.
    let PyValue::Tuple(signed) = &h8[1] else {
        panic!("signedFunc is not a tuple: {:?}", h8[1])
    };
    assert_eq!(signed.len(), 2);
    match &signed[0] {
        PyValue::Bytes(b) => assert_eq!(b[0], b'c', "not a marshalled code object"),
        other => panic!("signedFunc[0] should be a string, got {other:?}"),
    }

    // H9
    client.send(&PyValue::Tuple(vec![
        crypto_hash(&[PyValue::Bytes(server_challenge.clone())]),
        bstr(""),
        PyValue::Dict(vec![]),
    ]));

    // H10
    let h10 = client.recv();
    (server_challenge, h10)
}

#[test]
fn a_client_completes_the_handshake_over_tcp() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);

    let (_, h10) = full_handshake(&mut client);

    let session_init = dict_get(&h10, "session_init").expect("session_init is required");
    assert!(
        dict_get(session_init, "userid").is_some(),
        "the client indexes session_init['userid'] directly"
    );
    assert!(dict_get(&h10, "proxy_nodeid").is_some());
    assert!(dict_get(&h10, "access_token").is_some());
    assert!(dict_get(&h10, "computer_hash").is_some());

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    assert!(matches!(handle.join().expect("join"), ConnEnd::PeerClosed));
}

#[test]
fn an_unreadable_packet_after_authentication_does_not_kill_the_connection() {
    // This is the real post-handshake situation: the client sends a
    // MachoPacket, which is a Python instance wrapped in an object type code
    // we cannot decode. The server must log it and stay up, because the point
    // of the first connection is to capture what it sends.
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);

    // A payload whose first tag is 0x17, the old-style-instance wrapper.
    client.send_raw_payload(&[0x7E, 0, 0, 0, 0, 0x17, 0x00]);
    // And a second one, to prove the connection is still being read.
    client.send_raw_payload(&[0x7E, 0, 0, 0, 0, 0x22, 0x00]);

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    assert!(
        matches!(handle.join().expect("join"), ConnEnd::PeerClosed),
        "the connection should have survived both unreadable packets"
    );
}

#[test]
fn the_status_probe_gets_one_answer_and_a_disconnect() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    client.recv(); // H1
    client.send(&h2());
    client.send(&PyValue::Tuple(vec![PyValue::None, bstr("QC")]));
    assert_eq!(client.recv(), PyValue::Int(0));
    assert!(client.expect_closed(), "the server should close after QC");
    assert!(matches!(
        handle.join().expect("join"),
        ConnEnd::DispatchClosed
    ));
}

#[test]
fn a_bad_version_ends_the_connection_with_a_named_reason() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    client.recv(); // H1
    let PyValue::Tuple(mut t) = h2() else {
        unreachable!()
    };
    t[1] = PyValue::Int(495); // wrong machoVersion
    client.send(&PyValue::Tuple(t));

    match handle.join().expect("join") {
        ConnEnd::Dispatch(why) => assert!(
            why.contains("macho_version"),
            "the reason should name the field, got: {why}"
        ),
        other => panic!("expected a dispatch refusal, got {other}"),
    }
}

#[test]
fn capture_mode_answers_nothing_and_still_reads() {
    let (addr, handle) = start(Mode::Capture, None);
    let mut client = MockClient::connect(addr);
    client.send(&PyValue::Int(1));
    client.send(&PyValue::Int(2));
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    assert!(matches!(handle.join().expect("join"), ConnEnd::PeerClosed));
}

/// `CARGO_TARGET_TMPDIR/captures/<tag>-<pid>-<n>`, fresh and empty.
///
/// A stale directory of the same name can only come from a recycled pid, so
/// clearing it races nobody.
fn capture_dir(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("captures")
        .join(format!("{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn every_frame_in_both_directions_lands_in_the_capture() {
    // Under target/ rather than /tmp, and deliberately left behind: it is a
    // real capture of a complete handshake, and `evecap` wants something to
    // read. Cheaper than arranging a client every time the reader changes.
    // CARGO_TARGET_TMPDIR, not the package directory: under tools/dev.sh the
    // repo is mounted read-only and only target/ is writable.
    //
    // One directory per run (V4 S3, IP18): the pid and a counter keep two
    // `cargo test` runs sharing this target dir from wiping each other's
    // capture between write and `read_dir`. This file cannot use the crate's
    // `testkit::scratch_dir` (it is `#[cfg(test)]`, invisible here), and
    // that guard would remove the capture on drop anyway.
    let dir = capture_dir("every-frame");
    let (addr, handle) = start(Mode::Gps, Some(dir.clone()));
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");

    let files: Vec<_> = std::fs::read_dir(&dir)
        .expect("capture dir")
        .map(|e| e.expect("entry").path())
        .collect();
    assert_eq!(files.len(), 1, "one capture per connection");
    let records = eve_server::capture::read_all(&files[0]).expect("capture parses");

    // 5 server payloads (H1, H5, H7, H8, H10) + 5 client payloads
    // (H2, H3, H4, H6, H9) + the SessionInitialStateNotification that
    // follows H10 in the same flush.
    assert_eq!(records.len(), 11, "records: {records:?}");
    let from_client = records
        .iter()
        .filter(|r| r.direction == eve_server::capture::Direction::ClientToServer)
        .count();
    assert_eq!(from_client, 5);
    // The last frame is the session, and it is an 0x17 instance.
    let last = records.last().expect("a last record");
    assert_eq!(
        last.direction,
        eve_server::capture::Direction::ServerToClient
    );
    assert_eq!(last.bytes[5], 0x17, "the session packet is an instance");
    // The first record must be ours: the handshake is server-initiated.
    assert_eq!(
        records[0].direction,
        eve_server::capture::Direction::ServerToClient
    );
    // And the bytes must be the real payload, not a summary of it.
    assert_eq!(
        records[0].bytes[0], 0x7E,
        "a captured payload starts with its marshal header"
    );
}

#[test]
fn a_frame_declaring_an_impossible_length_is_refused() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    client.recv(); // H1
    // 0x0FFFFFFF bytes declared, four sent. Above MAX_FRAME, so the splitter
    // must refuse rather than buffer forever.
    client
        .stream
        .write_all(&[0xFF, 0xFF, 0xFF, 0x0F])
        .expect("write");
    match handle.join().expect("join") {
        ConnEnd::Framing(_) => {}
        other => panic!("expected a framing error, got {other}"),
    }
}

#[test]
fn a_payload_split_across_two_writes_is_still_one_frame() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    client.recv(); // H1

    let mut framed = Vec::new();
    write_frame(&dumps(&h2()), &mut framed).expect("frame");
    let (a, b) = framed.split_at(3);
    client.stream.write_all(a).expect("write head");
    std::thread::sleep(Duration::from_millis(20));
    client.stream.write_all(b).expect("write tail");

    // If the split had broken framing we would get a Framing or Dispatch end;
    // reaching the next phase proves the splitter reassembled it.
    client.send(&PyValue::Tuple(vec![PyValue::None, bstr("QC")]));
    assert_eq!(client.recv(), PyValue::Int(0));
    assert!(matches!(
        handle.join().expect("join"),
        ConnEnd::DispatchClosed
    ));
}

#[test]
fn two_payloads_in_one_write_are_both_handled() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    client.recv(); // H1

    let mut framed = Vec::new();
    write_frame(&dumps(&h2()), &mut framed).expect("frame");
    write_frame(
        &dumps(&PyValue::Tuple(vec![PyValue::None, bstr("QC")])),
        &mut framed,
    )
    .expect("frame");
    client.stream.write_all(&framed).expect("write");

    assert_eq!(client.recv(), PyValue::Int(0));
    assert!(matches!(
        handle.join().expect("join"),
        ConnEnd::DispatchClosed
    ));
}

// --- the session and the first service call -------------------------------

/// The character every single-character fixture offers.
/// `SandboxConfig::default()`'s one `Character`.
const FIXTURE_CHARACTER: i64 = 90_000_001;

/// The fixture character's ship, which is also its `ship_id` and the item id
/// of the capsule row `Registry::new` seeds. The Destiny ego ball is keyed on
/// it: `gameui` stores `session.shipid` as `wannaBeEgo` and the camera
/// attaches to the ball with that id.
const FIXTURE_SHIP_ID: i64 = 100_000_001;

/// Select a character, and read back both payloads it answers with.
///
/// **Required before any call that acts as a character.** Since Round 0.3
/// the server records `session.character_id` here and nowhere else, and
/// every service arm reads it as `Request::actor`; before this call the
/// actor is `None` and an arm answers its no-character branch. That is the
/// point of the design -- there is no "the first character" to fall back on
/// -- so a test that wants a wallet, a hangar or a mission has to say whose.
///
/// The order it reads in is contractual and is asserted here rather than
/// commented: the `SessionChangeNotification` comes **first**, then the
/// `CallRsp`. See `selecting_a_character_changes_the_session` for what is in
/// the change itself.
fn select_character(client: &mut MockClient, character_id: i64, call_id: i64) {
    client.send(&call_req_args(
        "charUnboundMgr",
        "SelectCharacterID",
        call_id,
        vec![
            PyValue::Int(character_id),
            PyValue::None,
            PyValue::Bool(false),
        ],
    ));
    let notification = client.recv();
    let PyValue::Instance { class, .. } = &notification else {
        panic!("expected a packet, got {notification:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionChangeNotification",
        "the session change must arrive before the answer"
    );
    let answer = expect_call_rsp(&client.recv());
    assert_eq!(answer, PyValue::None, "SelectCharacterID answers None");
}

/// Read the `SessionInitialStateNotification` the server sends after H10 and
/// return its `initialstate` map.
fn read_session(client: &mut MockClient) -> PyValue {
    let packet = client.recv();
    let PyValue::Instance { class, state } = &packet else {
        panic!("the session packet must be a 0x17 instance, got {packet:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionInitialStateNotification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    assert_eq!(t.len(), 14, "a packet state is a 14-tuple");
    assert_eq!(t[0], PyValue::Int(18), "command 18");
    // body = (sid, sessionType, initialstate)
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple: {:?}", t[4])
    };
    assert_eq!(body.len(), 3);
    assert_eq!(body[1], PyValue::Int(5), "sessionType must be 5");
    body[2].clone()
}

#[test]
fn the_session_carries_a_role_mask_with_the_gm_bits_set() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);

    let state = read_session(&mut client);
    let PyValue::Int(role) = *dict_get(&state, "role").expect("role is required") else {
        panic!("role must be an integer the client can mask against")
    };

    // The whole point of the project: ROLE_GML and ROLE_GMH must be set.
    // `ROLE_ANY & ~ROLE_SERVICE`, which two of our own documents once
    // recommended, would fail both of these.
    assert_ne!(role & (1 << 54), 0, "ROLE_GML missing");
    assert_ne!(role & (1 << 53), 0, "ROLE_GMH missing");
    assert_ne!(role & (1 << 62), 0, "ROLE_LOGIN missing");
    assert_ne!(role & (1 << 61), 0, "ROLE_PLAYER missing");
    // And the one that poisons the session.
    assert_eq!(role & (1 << 60), 0, "ROLE_SERVICE must never be set");
    // charid absent is what routes the client to character select.
    assert!(dict_get(&state, "charid").is_none());

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// Build a `CallReq` the way `ServiceCallGPCS` does.
fn call_req(service: &str, method: &str, call_id: i64) -> PyValue {
    call_req_args(service, method, call_id, vec![])
}

fn call_req_args(service: &str, method: &str, call_id: i64, args: Vec<PyValue>) -> PyValue {
    let payload = eve_proto::CallPayload::Service {
        method: method.to_owned(),
        args,
        kwargs: vec![],
    };
    let packet = eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallReq,
        // The client's own address, carrying the callID it will match on.
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(call_id),
            service: None,
        },
        // No node known, so the ANY address with the service name.
        eve_proto::MachoAddress::Any {
            service: Some(service.to_owned()),
            call_id: Some(call_id),
        },
        vec![payload.to_payload()],
    );
    packet.to_value().expect("a CallReq value")
}

/// The reply's payload, after checking it is a `CallRsp`.
/// Read until a `CallRsp` arrives, skipping asynchronous notifications.
///
/// **Needed because the wire does not promise otherwise.** A push posted for
/// this connection by *another* connection's turn is sent by the ticker, so
/// it can land between a request and its answer. Two-player tests used to
/// read the next frame and assert it was the response, which worked only
/// because nothing was ever pushed to a session that had not acted -- the
/// moment `OnCharNowInStation` existed, the same tests became a race on
/// `push::TICK`.
///
/// Returns the notifications it skipped, so a caller can still assert on
/// them rather than losing them.
fn call_rsp_after_pushes(client: &mut MockClient) -> (PyValue, Vec<String>) {
    let mut frames = Vec::new();
    call_rsp_after_pushes_with(client, &mut frames)
}

/// The same, keeping each skipped notification's **payload** as well as its
/// name.
///
/// Needed from the moment two players share a system: every assertion about a
/// second player is about what a push *said*, and the name-only form can only
/// say that something arrived. Returning the names too keeps every existing
/// caller unchanged.
fn call_rsp_after_pushes_with(
    client: &mut MockClient,
    frames: &mut Vec<(String, Vec<PyValue>)>,
) -> (PyValue, Vec<String>) {
    let mut skipped = Vec::new();
    for _ in 0..8 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("expected a packet, got {frame:?}")
        };
        let name = String::from_utf8_lossy(class).into_owned();
        // **The final segment, compared exactly.** This read
        // `name.ends_with("Notification")`, which is also true of
        // `SessionChangeNotification` -- so the first caller to drive a call
        // that changes the session fed a session change to
        // `expect_notification` and died inside the helper rather than at an
        // assertion. The undock is that call.
        match name.rsplit('.').next() {
            Some("Notification") => {
                let (event, args) = expect_notification(&frame);
                skipped.push(event.clone());
                frames.push((event, args));
                continue;
            }
            // Skipped and not collected: a session change is not a push, it
            // is the other half of the call that caused it, and
            // `expect_session_change` is what reads one.
            Some("SessionChangeNotification") => continue,
            _ => return (expect_call_rsp(&frame), skipped),
        }
    }
    panic!("no CallRsp after 8 frames; skipped {skipped:?}")
}

/// The object id out of a `MachoBindObject` reply.
///
/// A bind answers `(boundObject, retval)`, and the bound object is a
/// `Callback` whose child is a whole second marshal stream holding
/// `(objectID, refID)` -- which is what `ObjectCall.ParseObjectID` loads.
fn bound_object_id(reply: &PyValue) -> String {
    let PyValue::Tuple(pair) = reply else {
        panic!("a bind answers (boundObject, retval), got {reply:?}")
    };
    let PyValue::Callback(child) = &pair[0] else {
        panic!(
            "the bound object is a callback reference, got {:?}",
            pair[0]
        )
    };
    let PyValue::Bytes(inner) = child.as_ref() else {
        panic!("a reference's child is a nested stream")
    };
    let PyValue::Tuple(parts) = eve_server::loads(inner).expect("the reference loads") else {
        panic!("a reference is (objectID, refID)")
    };
    let PyValue::Bytes(object_id) = &parts[0] else {
        panic!("the objectID is a string")
    };
    String::from_utf8_lossy(object_id).into_owned()
}

/// A call on an already-bound object, which is how every `Cmd*` arrives.
///
/// The destination is the **node**, with no service name: the object id in the
/// payload is the only routing information in the frame.
fn bound_call(object_id: &str, method: &str, call_id: i64, args: Vec<PyValue>) -> PyValue {
    let payload = eve_proto::CallPayload::BoundObject {
        object_id: object_id.to_owned(),
        method: method.to_owned(),
        args,
        kwargs: vec![],
    };
    eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallReq,
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(call_id),
            service: None,
        },
        eve_proto::MachoAddress::Node {
            node_id: Some(1),
            service: None,
            call_id: None,
        },
        vec![payload.to_payload()],
    )
    .to_value()
    .expect("a bound CallReq")
}

/// Undock, and hand back the `beyonce` object id and the bootstrap's stamp.
///
/// The sequence a real client performs, in the order the capture shows it:
/// `ship.Undock` fused onto a bind, then `beyonce.GetFormations`, then
/// `MachoResolveObject` and `MachoBindObject` on the solar system -- and the
/// state arrives after that last answer, in the same flush.
fn undock_and_bind_beyonce(client: &mut MockClient) -> (String, i64) {
    let mut park = std::collections::BTreeSet::new();
    undock_and_bind_beyonce_into(client, &mut park)
}

/// The same, folding the bootstrap's balls into `park`.
///
/// Two-client tests assert on the **set of balls a player has**, so the
/// bootstrap has to go through the same decoder every later push does --
/// otherwise the starting point is asserted one way and the changes another,
/// and only one of them is the client's own reading.
fn undock_and_bind_beyonce_into(
    client: &mut MockClient,
    park: &mut std::collections::BTreeSet<i64>,
) -> (String, i64) {
    let (id, args) = undock_and_bind_beyonce_args(client, park);
    (id, first_stamp(&args))
}

/// The same, handing back the bootstrap's own arguments.
fn undock_and_bind_beyonce_args(
    client: &mut MockClient,
    park: &mut std::collections::BTreeSet<i64>,
) -> (String, Vec<PyValue>) {
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        80,
        vec![
            PyValue::Tuple(vec![PyValue::Int(60_012_526), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Undock"),
                PyValue::Tuple(vec![]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (_undock, _) = call_rsp_after_pushes(client);
    client.send(&call_req_args("beyonce", "GetFormations", 81, vec![]));
    let (_formations, _) = call_rsp_after_pushes(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        82,
        vec![PyValue::Int(30_000_001)],
    ));
    let _node = expect_call_rsp(&client.recv());
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        83,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let bound = expect_call_rsp(&client.recv());
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate", "the bootstrap follows the bind");
    fold_balls(&event, &args, park);
    (bound_object_id(&bound), args)
}

/// The stamp of a `DoDestinyUpdate` batch, and the only entry's `(name, args)`.
fn destiny_entry(args: &[PyValue]) -> (i64, String, Vec<PyValue>) {
    assert_eq!(args.len(), 2, "state and waitForBubble, and no third");
    assert_eq!(args[1], PyValue::Int(0), "waitForBubble must be false");
    let PyValue::List(entries) = &args[0] else {
        panic!("the batch is a sequence: {args:?}")
    };
    assert_eq!(entries.len(), 1, "one action per command: {entries:?}");
    let PyValue::Tuple(entry) = &entries[0] else {
        panic!("an entry is (stamp, (name, args))")
    };
    let PyValue::Int(stamp) = entry[0] else {
        panic!("a stamp is an int")
    };
    let PyValue::Tuple(call) = &entry[1] else {
        panic!("(name, args)")
    };
    let PyValue::Bytes(name) = &call[0] else {
        panic!("the name is a Python 2 str")
    };
    let PyValue::Tuple(call_args) = &call[1] else {
        panic!("args is one tuple")
    };
    (
        stamp,
        String::from_utf8_lossy(name).into_owned(),
        call_args.clone(),
    )
}

fn first_stamp(args: &[PyValue]) -> i64 {
    let PyValue::List(entries) = &args[0] else {
        panic!("the batch is a sequence")
    };
    let PyValue::Tuple(entry) = &entries[0] else {
        panic!("an entry is a tuple")
    };
    let PyValue::Int(stamp) = entry[0] else {
        panic!("a stamp is an int")
    };
    stamp
}

/// A throttle drag from a standing start is **two** commands, and both have to
/// come back as actions.
///
/// Four properties, none of them facts about our own code:
///
/// 1. **`Cmd*` arrives as a blocking `CallReq` on a bound object** and must be
///    answered, because we mint `N=` object ids and the prefix is what sets
///    `blocking` (`docs/reference/bound-objects.md` §1.6) `[V]`. Sending it as a
///    `Notification` instead would be dropped by `handle_notification` and a
///    test that only checked the push would still pass -- while the real
///    client hung until `machoTimeout`.
/// 2. **Answering is not enough.** Nothing on the client applies a command
///    locally: `michelle.GetRemotePark()` returns the bound object itself and
///    `CmdSetSpeedFraction` does not occur in `michelle.pyc` at all `[V]`. So
///    each answer must be followed by a `DoDestinyUpdate`.
/// 3. **Both commands land.** `activeShipController.SetSpeed`'s
///    `mode == DSTBALL_STOP` branch at `:222` falls through with no `else`, so
///    a standing start sends `CmdGotoDirection` *then*
///    `CmdSetSpeedFraction` `[V]`. Driving only the fraction -- which the
///    round-2 spec's "either/or" reading would suggest -- leaves the ship in
///    STOP for ever, and the mode is what the client's next command branches
///    on.
/// 4. **The stamps never go backwards**, including across the bootstrap: a
///    batch below the last `SetState`'s stamp is discarded with
///    `Michelle discarded a state that was too old` before it reaches the
///    dispatcher (`michelle.py:1443-1448`) `[V]`. Asserting a particular
///    stamp, or `stamp == now`, would fail exactly when the clamp was doing
///    its job.
#[test]
fn the_throttle_pair_is_answered_and_echoed_as_two_actions() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);
    let (beyonce, bootstrap_stamp) = undock_and_bind_beyonce(&mut client);

    // The heading first, as the HUD sends it.
    client.send(&bound_call(
        &beyonce,
        "CmdGotoDirection",
        84,
        vec![PyValue::Real(1.0), PyValue::Real(0.0), PyValue::Real(0.0)],
    ));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(
        answer,
        PyValue::None,
        "every Cmd* caller discards the return value, but the call blocks \
         until it arrives"
    );
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate");
    let (heading_stamp, name, call_args) = destiny_entry(&args);
    assert_eq!(name, "GotoDirection");
    assert_eq!(
        call_args,
        vec![
            PyValue::Int(FIXTURE_SHIP_ID),
            PyValue::Real(1.0),
            PyValue::Real(0.0),
            PyValue::Real(0.0),
        ],
        "the ball id goes in front of the direction the client sent"
    );

    // Then the throttle.
    client.send(&bound_call(
        &beyonce,
        "CmdSetSpeedFraction",
        85,
        vec![PyValue::Real(0.5)],
    ));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(answer, PyValue::None);
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate");
    let (throttle_stamp, name, call_args) = destiny_entry(&args);
    assert_eq!(name, "SetSpeedFraction");
    assert_eq!(
        call_args,
        vec![PyValue::Int(FIXTURE_SHIP_ID), PyValue::Real(0.5)],
        "two arguments: the ball and the fraction"
    );

    assert!(
        heading_stamp >= bootstrap_stamp && throttle_stamp >= heading_stamp,
        "stamps must never decrease: bootstrap {bootstrap_stamp}, \
         heading {heading_stamp}, throttle {throttle_stamp}"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **CAPTURE-UNDOCK: the undock's velocity as the client's destiny reads it.**
///
/// The owner reported himself at "15k m/s" while undocking. The ego ball in
/// the bootstrap is decoded here with the same `Park::decode` that `evecap`
/// uses on a capture: STOP mode, at most the 700 m/s shove
/// (`docking::ship_undock_velocity`), a finite positive mass and agility and
/// maximum velocity (an integration that blows up needs one of them wrong),
/// and a velocity that falls off as the ship coasts.
#[test]
fn the_undock_velocity_is_at_most_700_and_falls_off() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);
    let mut balls = std::collections::BTreeSet::new();
    let (beyonce, args) = undock_and_bind_beyonce_args(&mut client, &mut balls);

    let ego_of = |args: &[PyValue]| -> eve_server::destiny::Ball {
        let (_stamp, name, call_args) = destiny_entry(args);
        assert_eq!(name, "SetState");
        let PyValue::Instance { state: bag, .. } = &call_args[0] else {
            panic!("the bag is a KeyVal instance")
        };
        let PyValue::Dict(entries) = &**bag else {
            panic!("a KeyVal's state is an attribute dict")
        };
        let get = |key: &[u8]| {
            entries
                .iter()
                .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == key))
                .map(|(v, _)| v.clone())
                .expect("bag key")
        };
        let PyValue::Bytes(blob) = get(b"state") else {
            panic!("bag.state is a byte string")
        };
        let PyValue::Int(ego) = get(b"ego") else {
            panic!("bag.ego is an id")
        };
        eve_server::destiny::Park::decode(&blob)
            .expect("our own blob decodes")
            .balls
            .into_iter()
            .find(|b| b.ball_id == ego)
            .expect("the ego ball is in the blob")
    };
    let speed = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();

    let first = ego_of(&args);
    assert_eq!(first.mode, eve_server::destiny::Mode::Stop);
    let p = first.physics.expect("an ego ball is free");
    let v0 = speed(p.velocity);
    assert!(
        v0 <= 700.0 + 1e-6,
        "the undock shove is capped at 700: {v0}"
    );
    assert!(v0.is_finite());
    for (what, x) in [
        ("mass", first.mass),
        ("agility", f64::from(p.agility)),
        ("max velocity", f64::from(p.max_velocity)),
        ("radius", f64::from(first.radius)),
    ] {
        assert!(x.is_finite() && x > 0.0, "{what} must be positive: {x}");
    }

    // Coasting: a later state carries less speed.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    client.send(&bound_call(&beyonce, "UpdateStateRequest", 85, vec![]));
    let (_answer, _) = call_rsp_after_pushes(&mut client);
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate");
    let v1 = speed(ego_of(&args).physics.expect("free").velocity);
    assert!(v1 < v0, "a STOP ball coasts to rest: {v0} then {v1}");
    drop(client);
    let _ = handle.join();
}

/// A desync is recoverable, and the recovery carries where the ship *is*.
///
/// `beyonce.UpdateStateRequest` is reachable only from `Park.RequestReset` <-
/// `RealFlushState`'s failed synchronise, which has already set
/// `validState = False` -- after which every batch we push is dropped with
/// `Events ignored` until a `SetState` arrives (`michelle.py:1076`, `:1260`,
/// `:1527`) `[V]`. So the answer alone cannot recover the session; the push is
/// the reply that matters.
///
/// The second half is the one that would have been easy to get wrong: a
/// recovery built from the undock constants would clear `validState` *and*
/// teleport the ship back to the station mouth with its throttle at zero. The
/// tempting wrong version asserts the recovery is byte-identical to the
/// bootstrap, which is precisely the bug.
#[test]
fn a_state_request_is_answered_with_a_fresh_state_carrying_the_current_motion() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);
    let (beyonce, _) = undock_and_bind_beyonce(&mut client);

    client.send(&bound_call(
        &beyonce,
        "CmdSetSpeedFraction",
        84,
        vec![PyValue::Real(0.75)],
    ));
    let (_answer, _) = call_rsp_after_pushes(&mut client);
    let _echo = expect_notification(&client.recv());

    client.send(&bound_call(&beyonce, "UpdateStateRequest", 85, vec![]));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(
        answer,
        PyValue::None,
        "the retval is discarded -- `self.remoteBallpark.UpdateStateRequest()` \
         is a bare statement"
    );
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate");
    let (_stamp, name, call_args) = destiny_entry(&args);
    assert_eq!(
        name, "SetState",
        "only a SetState clears validState after a RequestReset"
    );

    // And it carries the throttle the player set, not the undock's zero.
    let PyValue::Instance { state: bag, .. } = &call_args[0] else {
        panic!("the bag is a KeyVal instance")
    };
    let PyValue::Dict(entries) = &**bag else {
        panic!("a KeyVal's state is an attribute dict")
    };
    let PyValue::Bytes(blob) = entries
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == b"state"))
        .map(|(v, _)| v.clone())
        .expect("bag.state")
    else {
        panic!("bag.state is a byte string")
    };
    let park = eve_server::destiny::Park::decode(&blob).expect("our own blob decodes");
    let ego = park
        .balls
        .iter()
        .find(|b| b.ball_id == FIXTURE_SHIP_ID)
        .expect("the ego ball");
    let physics = ego.physics.expect("an ego ball is free");
    assert!(
        (physics.speed_fraction - 0.75).abs() < 1e-6,
        "the recovery must not reset the throttle the player set: {}",
        physics.speed_fraction
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// One character, two windows: an addressed push reaches **both**.
///
/// A character may be logged in twice -- `session_table::of_character` returns
/// a `Vec` for exactly that reason -- and `deliver_elsewhere` has always
/// reached every session of its target. The acting character was the
/// exception: `notify` addresses *this* connection by construction, so the
/// window that acted got the frame and the other got nothing, while the
/// identical push reached both windows of anybody else.
///
/// Undocking is where that stops being cosmetic. The second window would sit
/// in space with no ballpark state at all: no ship, no station, no scene.
///
/// Pins **"both connections receive it"**, which is the property. The
/// tempting wrong version asserts the acting window got its `SetState` --
/// true throughout the defect -- or drives two *different* characters, which
/// exercises `deliver_elsewhere`'s own loop and never touches the branch that
/// was wrong.
#[test]
fn an_addressed_push_reaches_both_windows_of_one_character() {
    let addr = start_two_players();

    let mut one = MockClient::connect(addr);
    full_handshake(&mut one);
    read_session(&mut one);
    select_character(&mut one, ALICE, 70);

    // The same character again, from another window.
    let mut two = MockClient::connect(addr);
    full_handshake(&mut two);
    read_session(&mut two);
    select_character(&mut two, ALICE, 80);

    // Window one undocks. The state is addressed to the character, not to a
    // connection.
    let (_beyonce, _stamp) = undock_and_bind_beyonce(&mut one);

    // Window two asked for nothing and must still be told. The frame is
    // **posted, not sent**: `deliver_elsewhere` enqueues it and the ticker
    // writes it within one `push::TICK`, so it can land after the answer to
    // any call window two happens to make. Poll rather than read once --
    // the same shape `a_player_logging_off_is_removed_from_the_other_players_lobby`
    // uses, and for the same reason. Reading once passes or fails on a race
    // with a 250 ms timer, which is the worst kind of green.
    // **The wait is not padding.** `conn::serve` writes a reply and the
    // pushes that ride with it; it never drains the wire's posted outbox.
    // Only `push::spawn_ticker` does, every `push::TICK`. So a frame posted
    // for a connection that is merely *reading* waits for the next tick, and
    // a poll loop with no sleep in it finishes inside a millisecond and
    // proves nothing. Twenty tries at 100 ms is two seconds of headroom on a
    // 250 ms timer, and succeeds on the third or fourth on an idle machine.
    let mut saw = false;
    for call_id in 90..110 {
        two.send(&call_req("machoNet", "GetTime", call_id));
        let (_time, pushed) = call_rsp_after_pushes(&mut two);
        if pushed.iter().any(|e| e == "DoDestinyUpdate") {
            saw = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        saw,
        "the other window of the same character must get the ballpark too, \
         or it sits in space with no ship"
    );

    one.stream.shutdown(Shutdown::Both).expect("shutdown");
    two.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// Docking is a loop, not a trip home: undock, dock, undock again.
///
/// Three properties, and the third is the one that makes this increment worth
/// having:
///
/// 1. **`CmdDock` is answered**, because it is a blocking call on a bound
///    object like every other `Cmd*`. Left unanswered, the client's dock
///    blocks until `machoTimeout` with a 10-second session-change lockout
///    already armed.
/// 2. **The session change clears `solarsystemid` and restores `stationid`,
///    and carries nothing else.** Clearing the system is what tears the
///    ballpark down -- `michelle.UpdateBallpark:216` is gated on it `[V]` --
///    and `shipid` in particular must stay out: with `solarsystemid` already
///    `None`, `invCache.ProcessSessionChange:182` would take a branch that
///    deletes an inventory and skips the invalidation the dock needs `[V]`.
/// 3. **The second undock gets its own bootstrap**, at the mouth. `Park.Release`
///    destroys the park and unbinds the object (`michelle.py:1196`, `:1213`),
///    so a real client re-resolves and re-binds `beyonce` -- and our hook has
///    to fire again. The tempting wrong version stops after asserting the
///    dock's session change, which is also true of a server that can never
///    undock twice.
#[test]
fn docking_closes_the_loop_and_the_second_undock_starts_at_the_mouth() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);
    let (beyonce, first_stamp_out) = undock_and_bind_beyonce(&mut client);

    // Fly a little, so that "the second undock starts at the mouth" is a
    // claim about the mouth and not about a ship that never moved.
    client.send(&bound_call(
        &beyonce,
        "CmdSetSpeedFraction",
        84,
        vec![PyValue::Real(1.0)],
    ));
    let (_answer, _) = call_rsp_after_pushes(&mut client);
    let _echo = expect_notification(&client.recv());

    // Dock. The session change comes in front of the answer, which is the
    // order `with_session_change` already uses for the undock.
    client.send(&bound_call(
        &beyonce,
        "CmdDock",
        85,
        vec![PyValue::Int(60_012_526), PyValue::Int(FIXTURE_SHIP_ID)],
    ));
    let change = loop {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("expected a packet, got {frame:?}")
        };
        if String::from_utf8_lossy(class).ends_with("SessionChangeNotification") {
            break expect_session_change(&frame).1;
        }
    };
    assert_eq!(
        change_of(&change, "solarsystemid"),
        &PyValue::None,
        "docked means the system is absent, and clearing it is what tears \
         the ballpark down"
    );
    assert_eq!(
        change_of(&change, "stationid"),
        &PyValue::Int(60_012_526),
        "and the station comes back"
    );
    let PyValue::Dict(entries) = &change else {
        panic!("a session change is a dict, got {change:?}")
    };
    assert_eq!(
        entries.len(),
        2,
        "two keys: locationid and corprole are derived by the client, and \
         shipid must not be sent at all: {entries:?}"
    );
    assert_eq!(
        expect_call_rsp(&client.recv()),
        PyValue::None,
        "CmdDock is answered -- PerformSessionChange discards the value, but \
         the call blocks on it"
    );

    // And out again. A real client builds a new `Park` and binds a new
    // object, so the whole resolve/bind pair runs a second time.
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        86,
        vec![
            PyValue::Tuple(vec![PyValue::Int(60_012_526), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Undock"),
                PyValue::Tuple(vec![]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (_undock, _) = call_rsp_after_pushes(&mut client);
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        87,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let _bound = expect_call_rsp(&client.recv());
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(
        event, "DoDestinyUpdate",
        "the second undock must get its own state, or the player is in a \
         hangar with no way out"
    );
    let (second_stamp, name, call_args) = destiny_entry(&args);
    assert_eq!(name, "SetState");
    assert!(
        second_stamp >= first_stamp_out,
        "stamps never decrease, across a dock as well: \
         {second_stamp} < {first_stamp_out}"
    );

    // At the mouth, with the throttle back at zero: the previous flight is
    // forgotten when a ship arrives in space, or docking and undocking would
    // put it back where it docked.
    let PyValue::Instance { state: bag, .. } = &call_args[0] else {
        panic!("the bag is a KeyVal instance")
    };
    let PyValue::Dict(bag_entries) = &**bag else {
        panic!("a KeyVal's state is an attribute dict")
    };
    let PyValue::Bytes(blob) = bag_entries
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == b"state"))
        .map(|(v, _)| v.clone())
        .expect("bag.state")
    else {
        panic!("bag.state is a byte string")
    };
    let park = eve_server::destiny::Park::decode(&blob).expect("our own blob decodes");
    let ego = park
        .balls
        .iter()
        .find(|b| b.ball_id == FIXTURE_SHIP_ID)
        .expect("the ego ball");
    assert_eq!(
        ego.mode,
        eve_server::destiny::Mode::Stop,
        "a ship that has just undocked is parked, whatever it was doing last \
         time it was in space"
    );
    assert_eq!(
        ego.physics.expect("free").speed_fraction,
        0.0,
        "and its throttle is back at zero"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

const ALICE_SHIP: i64 = 100_000_001;
const BOB_SHIP: i64 = 100_000_002;

/// Every ball id a `DoDestinyUpdate` batch adds or removes, folded the way
/// `michelle` folds them.
///
/// **A decoder, not a shape assertion.** The lesson of the empty shield bar
/// (`docs/port-tracker.md` §23.3) is that asserting the tuples we emit passes
/// happily while the client reads something else, so these tests assert the
/// *set of balls the player ends up with*. `SetState` replaces, `AddBalls2`
/// appends, `RemoveBalls` subtracts -- which is exactly what the client does
/// with them.
fn fold_balls(event: &str, args: &[PyValue], park: &mut std::collections::BTreeSet<i64>) {
    assert_eq!(event, "DoDestinyUpdate");
    let PyValue::List(entries) = &args[0] else {
        panic!("the batch is a sequence: {args:?}")
    };
    for entry in entries {
        let PyValue::Tuple(entry) = entry else {
            panic!("an entry is (stamp, (name, args))")
        };
        let PyValue::Tuple(call) = &entry[1] else {
            panic!("(name, args)")
        };
        let PyValue::Bytes(name) = &call[0] else {
            panic!("the name is a python 2 str")
        };
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args is one tuple")
        };
        match name.as_slice() {
            b"SetState" => {
                park.clear();
                let PyValue::Instance { state: bag, .. } = &call_args[0] else {
                    panic!("the bag is a KeyVal")
                };
                let PyValue::Dict(attrs) = &**bag else {
                    panic!("a KeyVal's state is an attribute dict")
                };
                let PyValue::Bytes(blob) = attrs
                    .iter()
                    .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == b"state"))
                    .map(|(v, _)| v.clone())
                    .expect("bag.state")
                else {
                    panic!("bag.state is a byte string")
                };
                for b in eve_server::destiny::Park::decode(&blob)
                    .expect("decodes")
                    .balls
                {
                    park.insert(b.ball_id);
                }
            }
            b"AddBalls2" => {
                // `((state, extraBallData),)` -- one argument, a 2-tuple.
                let PyValue::Tuple(pair) = &call_args[0] else {
                    panic!("AddBalls2 takes one argument and it is a 2-tuple")
                };
                assert_eq!(pair.len(), 2, "(state, extraBallData)");
                let PyValue::Bytes(blob) = &pair[0] else {
                    panic!("the appended state is a byte string")
                };
                for b in eve_server::destiny::Park::decode(blob)
                    .expect("decodes")
                    .balls
                {
                    park.insert(b.ball_id);
                }
            }
            b"RemoveBalls" => {
                assert_eq!(
                    call_args.len(),
                    1,
                    "RemoveBalls takes EXACTLY one argument: the dispatcher \
                     appends its own exploders dict as the second, so a \
                     helpful second argument of ours lands in isRelease"
                );
                let PyValue::List(ids) = &call_args[0] else {
                    panic!("RemoveBalls takes a list")
                };
                for id in ids {
                    let PyValue::Int(id) = id else {
                        panic!("a ball id is an int")
                    };
                    park.remove(id);
                }
            }
            other => panic!("unexpected destiny call {}", String::from_utf8_lossy(other)),
        }
    }
}

/// Read `tries` rounds of a cheap call, folding every Destiny push that
/// arrives into `park`, and stop early once `done` is satisfied.
///
/// The sleep is load-bearing: a frame addressed to a character who is not
/// calling is **posted**, and only `push::spawn_ticker` drains the outbox, so
/// a poll loop with no delay measures its own speed. See
/// `docs/port-tracker.md` §25.2.
fn drain_destiny(
    client: &mut MockClient,
    first_call_id: i64,
    park: &mut std::collections::BTreeSet<i64>,
    done: impl Fn(&std::collections::BTreeSet<i64>) -> bool,
) -> Vec<String> {
    let mut seen = Vec::new();
    for call_id in first_call_id..first_call_id + 20 {
        client.send(&call_req("machoNet", "GetTime", call_id));
        let mut frames = Vec::new();
        let (_rsp, names) = call_rsp_after_pushes_with(client, &mut frames);
        for (event, args) in &frames {
            if event == "DoDestinyUpdate" {
                fold_balls(event, args, park);
            }
        }
        seen.extend(names);
        if done(park) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    seen
}

/// Two players, one system, each with the other's ship in their ballpark.
///
/// **The increment, as a property.** Three things, and the second is the one
/// that cannot be faked:
///
/// 1. Bob's own bootstrap already contains Alice, because she was there
///    first.
/// 2. **Alice learns about Bob without asking.** She makes no call that could
///    carry the news; the frame is posted to her socket and written by the
///    ticker while she sits idle. That is what makes this co-op rather than
///    two single-player servers.
/// 3. Bob's arrival reaches Alice as an **append**, not a reset. A `SetState`
///    would work too -- and would rebuild every model and bracket on her
///    screen and teleport her own ship to the position we last stored. So the
///    test counts her `SetState`s and requires exactly the one she started
///    with.
///
/// The tempting wrong version asserts that *a* `DoDestinyUpdate` reached
/// Alice. That passes for a `SetState`, which is the mistake this is guarding
/// against, and it passes for a batch naming the wrong ball.
#[test]
fn two_players_in_one_system_each_have_the_others_ship() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    guest_arrival(&mut a, BOB);

    // Alice undocks into an empty system.
    let mut alice_park = std::collections::BTreeSet::new();
    let (_beyonce_a, _stamp) = undock_and_bind_beyonce_into(&mut a, &mut alice_park);
    assert!(
        alice_park.contains(&ALICE_SHIP),
        "her own ship, or the camera has nothing to attach to: {alice_park:?}"
    );
    assert!(
        !alice_park.contains(&BOB_SHIP),
        "Bob is still docked: {alice_park:?}"
    );

    // Bob undocks. His bootstrap must already know about Alice.
    let mut bob_park = std::collections::BTreeSet::new();
    let (_beyonce_b, _stamp_b) = undock_and_bind_beyonce_into(&mut b, &mut bob_park);
    assert!(
        bob_park.contains(&ALICE_SHIP) && bob_park.contains(&BOB_SHIP),
        "the pilot who undocks second sees the one already there: {bob_park:?}"
    );

    // And Alice is told, having asked for nothing.
    let events = drain_destiny(&mut a, 200, &mut alice_park, |p| p.contains(&BOB_SHIP));
    assert!(
        alice_park.contains(&BOB_SHIP),
        "Alice must learn about Bob without asking: {alice_park:?} {events:?}"
    );
    assert_eq!(
        events.iter().filter(|e| *e == "DoDestinyUpdate").count(),
        1,
        "one push for one arrival: {events:?}"
    );

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// When a pilot docks, their ship leaves everyone else's park.
///
/// The other half of the loop, and the one that leaves a ghost if it is
/// missed: a ball nobody removes stays selectable and targetable for ever.
///
/// Pins the **set** -- Bob's ball gone, Alice's and the station still there.
/// The tempting wrong version asserts a `RemoveBalls` arrived, which is true
/// of a batch naming the wrong ball, or asserts the park is empty, which is
/// true of the `SetState` reset this is specifically avoiding.
#[test]
fn docking_takes_your_ship_out_of_everyone_elses_park() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    guest_arrival(&mut a, BOB);

    let mut alice_park = std::collections::BTreeSet::new();
    undock_and_bind_beyonce_into(&mut a, &mut alice_park);
    let mut bob_park = std::collections::BTreeSet::new();
    let (beyonce_b, _) = undock_and_bind_beyonce_into(&mut b, &mut bob_park);
    drain_destiny(&mut a, 200, &mut alice_park, |p| p.contains(&BOB_SHIP));
    assert!(alice_park.contains(&BOB_SHIP), "Bob arrived first");
    let station = *alice_park
        .iter()
        .find(|id| **id != ALICE_SHIP && **id != BOB_SHIP)
        .expect("the station is a ball too");

    // Bob docks.
    b.send(&bound_call(
        &beyonce_b,
        "CmdDock",
        90,
        vec![PyValue::Int(60_012_526), PyValue::Int(BOB_SHIP)],
    ));
    let (_answer, _) = call_rsp_after_pushes(&mut b);

    drain_destiny(&mut a, 300, &mut alice_park, |p| !p.contains(&BOB_SHIP));
    assert!(
        !alice_park.contains(&BOB_SHIP),
        "a docked ship must leave the observer's park, or it is a ghost \
         nobody can remove: {alice_park:?}"
    );
    assert!(
        alice_park.contains(&ALICE_SHIP) && alice_park.contains(&station),
        "and nothing else may go with it: {alice_park:?}"
    );

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// A player who logs off stops being a guest, and the one still docked is
/// told.
///
/// The other half of the stale-lobby bug. It is **queued rather than sent**:
/// a disconnect has no dispatcher turn of its own, so the frame goes out on
/// the next turn of any connection -- here, Bob's own next call. That is why
/// this test makes a call it does not care about before reading.
#[test]
fn a_player_logging_off_is_removed_from_the_other_players_lobby() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    guest_arrival(&mut a, BOB);

    // Alice goes away.
    a.stream.shutdown(Shutdown::Both).expect("shutdown");

    // Bob asks about anything at all; the queued departure rides out with it.
    b.stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut seen = None;
    for call_id in 90..96 {
        b.send(&call_req("machoNet", "GetTime", call_id));
        let (_rsp, pushes) = call_rsp_after_pushes(&mut b);
        if let Some(e) = pushes.iter().find(|e| *e == "OnCharNoLongerInStation") {
            seen = Some(e.clone());
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    assert_eq!(
        seen.as_deref(),
        Some("OnCharNoLongerInStation"),
        "the session still docked must be told that the other one left"
    );

    b.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// Drain the `OnCharNowInStation` a newly arrived character owes everyone
/// already docked, and assert it is the right one.
///
/// **Deterministic, not tolerant.** It is the only frame the waiting session
/// is owed at that moment, so a blocking read is safe -- and draining it here
/// keeps the rest of a two-player test reading a clean stream instead of
/// racing `push::TICK`.
///
/// Asserted rather than discarded because this is the notification whose
/// absence made a guest list go stale: the player already docked when
/// somebody arrived was never told, so their lobby showed a station they were
/// alone in. Found with two real clients, which is the only way it could have
/// been -- a test that connects in one order and asks afterwards exercises
/// precisely the order that already worked.
fn guest_arrival(waiting: &mut MockClient, who: i64) {
    waiting
        .stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let (event, args) = expect_notification(&waiting.recv());
    assert_eq!(
        event, "OnCharNowInStation",
        "an already-docked session must be told who arrived; got {event} {args:?}"
    );
    // One argument, and `UNPACK_SEQUENCE 4` on it.
    assert_eq!(args.len(), 1, "one argument, a sequence");
    let PyValue::List(row) = &args[0] else {
        panic!("the row must be a sequence: {:?}", args[0])
    };
    assert_eq!(row.len(), 4, "UNPACK_SEQUENCE 4 raises on any other arity");
    assert_eq!(row[0], PyValue::Int(who), "characterID is element 0");
}

fn expect_call_rsp(v: &PyValue) -> PyValue {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.CallRsp"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    assert_eq!(t[0], PyValue::Int(7), "command 7 = CallRsp");
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    // The payload is a pickle, not the value: the client feeds it straight to
    // blue.marshal.Load. Decoding it here is the test's half of that contract,
    // so a regression to sending the live value fails loudly right here.
    let PyValue::Bytes(pickle) = &body[0] else {
        panic!(
            "a CallRsp payload must be a marshalled string, got {:?}",
            body[0]
        )
    };
    loads(pickle).expect("the response pickle must be a complete marshal stream")
}

#[test]
fn a_service_call_gets_a_call_rsp() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    // #1 in docs/reference/login-path.md §8: the first real service call the client
    // makes after connecting.
    client.send(&call_req("machoNet", "GetServiceInfo", 1));
    assert_eq!(
        expect_call_rsp(&client.recv()),
        PyValue::Dict(vec![]),
        "an empty serviceInfo dict is the correct answer for one node"
    );

    // #4: the clock sync, which must be a real value rather than a stub.
    client.send(&call_req("machoNet", "GetTime", 2));
    let PyValue::Int(filetime) = expect_call_rsp(&client.recv()) else {
        panic!("GetTime must return an integer")
    };
    // Windows FILETIME for 2020-01-01 and 2100-01-01, as a sanity window.
    assert!(
        (132_223_104_000_000_000..=157_472_928_000_000_000).contains(&filetime),
        "FILETIME {filetime} is not a plausible current time"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn the_character_selection_call_returns_the_four_tuple_the_client_unpacks() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    client.send(&call_req("charUnboundMgr", "GetCharacterSelectionData", 3));
    let result = expect_call_rsp(&client.recv());

    // charselData unpacks the answer into user details, training details,
    // character details and wars, and reads element 0 of the first.
    let PyValue::Tuple(four) = &result else {
        panic!("must be a 4-tuple: {result:?}")
    };
    assert_eq!(four.len(), 4);

    let PyValue::List(user_details) = &four[0] else {
        panic!("userDetails must be indexable")
    };
    assert_eq!(user_details.len(), 1, "only [0] is ever read");
    // Attributes the select screen reads off userDetails[0].
    let PyValue::Instance { class, state } = &user_details[0] else {
        panic!("userDetails[0] must be an instance")
    };
    assert_eq!(String::from_utf8_lossy(class), "utillib.KeyVal");
    for attr in ["userName", "characterSlots", "maxCharacterSlots"] {
        assert!(dict_get(state, attr).is_some(), "missing {attr}");
    }

    // trainingDetails is initialised to (None, None) before the call.
    assert_eq!(four[1], PyValue::Tuple(vec![PyValue::None, PyValue::None]));

    let PyValue::List(chars) = &four[2] else {
        panic!("characterDetails must be iterable")
    };
    assert_eq!(chars.len(), 1);
    let PyValue::Instance { state, .. } = &chars[0] else {
        panic!("a character row must be an instance")
    };
    // Every attribute read off a row across charselData.py,
    // characterSlots.py and characterSelection.py.
    for attr in [
        "characterID",
        "logoffDate",
        "corporationID",
        "allianceID",
        "stationID",
        "solarSystemID",
        "balance",
        "skillPoints",
        "unreadMailCount",
        "shipTypeID",
        "paperdollState",
        "finishedSkills",
    ] {
        assert!(dict_get(state, attr).is_some(), "row is missing {attr}");
    }
    // paperdollState must not divert the client into character creation.
    assert_eq!(dict_get(state, "paperdollState"), Some(&PyValue::Int(0)));

    assert_eq!(four[3], PyValue::List(vec![]), "no wars");

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// `slash.SlashCmd("/")` must come back as an **ErrorResponse**, not a CallRsp.
///
/// This is the one place the project's "always answer with a value" default
/// is wrong. `FetchRemoteCommandList` binds its local only inside the
/// `except UserError` block and reads it unconditionally afterwards, so a
/// normal return is an `UnboundLocalError` that breaks every slash command --
/// including the purely client-side ones, because `_SlashCmd` calls it before
/// matching anything.
#[test]
fn the_slash_command_list_is_raised_not_returned() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        70,
        // The client always sends a trailing space, even with no arguments.
        vec![PyValue::Bytes(b"/ ".to_vec())],
    ));
    let reply = client.recv();
    let PyValue::Instance { class, state } = &reply else {
        panic!("not an instance: {reply:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.ErrorResponse",
        "a CallRsp here is an UnboundLocalError on the far side"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    // ['originalCommand', 'code', 'payload']; code 2 is
    // MACHONETERR_WRAPPEDEXCEPTION, the only one the client re-raises rather
    // than turning into a routing error of its own.
    assert_eq!(body[1], PyValue::Int(2));

    // The payload is a 1-tuple holding a nested marshal stream. With more
    // than one element the client treats the rest as a server stack trace and
    // builds a cross-the-wire report we have nothing to put in.
    let PyValue::Tuple(payload) = &body[2] else {
        panic!("payload must be a tuple, got {:?}", body[2])
    };
    assert_eq!(payload.len(), 1);
    let PyValue::Bytes(stream) = &payload[0] else {
        panic!("payload[0] must be a marshalled exception")
    };

    // It must rebuild as eveexceptions.UserError('SlashError', {...}) --
    // tag 0x22, which CALLS the class, because UserError derives from
    // Exception and so is new-style: PyInstance_NewRaw (tag 0x17, what our
    // other instances use) does not apply to it.
    let exception = loads(stream).expect("the exception is a complete stream");
    let PyValue::ObjectEx { header, .. } = &exception else {
        panic!("must be an ObjectEx, got {exception:?}")
    };
    let PyValue::Tuple(parts) = header.as_ref() else {
        panic!("header must be (callable, args)")
    };
    assert_eq!(
        parts[0],
        PyValue::Global(b"eveexceptions.UserError".to_vec()),
        "and this name must be in the client's globalsWhitelist"
    );
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("args must be a tuple")
    };
    assert_eq!(args[0], PyValue::Bytes(b"SlashError".to_vec()));

    // The reason must contain ': ' once, with a sortable Python list literal
    // after it -- the client does eval(msg.split(': ')[1]).
    let PyValue::Dict(d) = &args[1] else {
        panic!("second arg must be the dict UserError stores as .dict")
    };
    let reason = d
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == b"reason"))
        .map(|(v, _)| v)
        .expect("a 'reason' key");
    let PyValue::Bytes(text) = reason else {
        panic!("reason must be a string")
    };
    let text = String::from_utf8_lossy(text);
    assert_eq!(
        text.matches(": ").count(),
        1,
        "split(': ')[1] takes one split"
    );
    let listed = text.split(": ").nth(1).expect("a payload after the colon");
    assert!(
        listed.starts_with('[') && listed.ends_with(']'),
        "must eval to a list"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// The standings wave, answered end to end in the shapes its callers demand.
///
/// `Standing.Run` -> `__RefreshStandings` issues `GetNPCNPCStandings` and then
/// `GetCharStandings` back to back on one tasklet, so these arrive together.
/// `GetCorpStandings` is **not** issued for our character -- the
/// `uthread.parallel` branch runs only when `not IsNPC(session.corpid)`, and
/// `IsNPC` is `9999 < id < 90000000`, which our NPC corp 1000001 satisfies --
/// but we answer it anyway.
///
/// The per-column shape of the rowset is asserted in `service.rs`'s own
/// tests, which have the helpers for picking an instance apart; what this one
/// proves is that all five are reachable over a real connection.
#[test]
fn the_standings_wave_is_answered_in_the_shapes_their_callers_demand() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    // `.Filter(...)` is called on this twice, unconditionally, as the next two
    // statements -- so it must be an object, not any of the four empties.
    client.send(&call_req("standingMgr", "GetNPCNPCStandings", 60));
    let standings = expect_call_rsp(&client.recv());
    let PyValue::Instance { class, .. } = &standings else {
        panic!("must be a Rowset instance with methods, got {standings:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "eve.common.script.sys.rowset.Rowset"
    );

    // A dict, which this caller uniquely *proves* rather than implies:
    // the client checks whether the standings are a dict and, if not,
    // rebuilds them as an index by `fromID`
    client.send(&call_req("standingMgr", "GetCharStandings", 61));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::Dict(vec![]));
    client.send(&call_req("standingMgr", "GetCorpStandings", 62));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::Dict(vec![]));

    // These two must be `list`-like, not rowsets: `standingsGraph` does
    // `self.transactions.reverse()`, which no `Rowset` has.
    client.send(&call_req("standingMgr", "GetStandingTransactions", 63));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::List(vec![]));
    client.send(&call_req("standingMgr", "GetStandingCompositions", 64));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::List(vec![]));

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// A call we cannot answer properly is still *answered*.
///
/// This used to assert the opposite -- that an unknown service produced no
/// reply at all -- on the reasoning that the connection surviving was the
/// thing that mattered. It is not. Silence on this protocol is a hang:
/// `RemoteServiceCallWithoutTheStars` blocks its tasklet with no recovery,
/// and the client logs nothing because nothing threw. Every missing method
/// therefore cost one human round trip: launch, hang, read the log,
/// implement one method, launch again.
///
/// Answering `None` is strictly better, by the corollary in
/// `docs/guides/debugging-the-client.md` section 1. The client either tolerates it
/// and keeps going, exposing the *next* gap in the same run, or it throws and
/// names the shape it wanted in its own log. Both beat a frozen screen.
#[test]
fn a_call_we_cannot_answer_is_answered_with_none_rather_than_silence() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    // Neither the service nor the method exists.
    client.send(&call_req("thereIsNoSuchService", "Nope", 9));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::None);

    // A real service we implement, asked for a method we do not.
    client.send(&call_req("charMgr", "NoSuchMethodHere", 10));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::None);

    // And the connection is still good for a call we *can* answer, so the
    // fallback is not a disconnect in disguise.
    client.send(&call_req("machoNet", "GetServiceInfo", 11));
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::Dict(vec![]));

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    assert!(matches!(handle.join().expect("join"), ConnEnd::PeerClosed));
}

/// The first post-handshake frame the first real client ever sent us, byte for
/// byte, from capture `1792404377-0002.evecap`.
///
/// It is zlib-compressed (`78 01`): the client's native marshal layer
/// compresses whole streams once they grow, which is why the server could
/// reach "Authenticated" and then go deaf, leaving the client on
/// *Authenticating* forever. A synthetic `call_req` cannot catch that, because
/// we build those uncompressed — so the real bytes are the test.
const REAL_CLIENT_GET_SERVICE_INFO: &[u8] = &[
    0x78, 0x01, 0xab, 0x63, 0x00, 0x02, 0x71, 0x61, 0xfd, 0xe4, 0xc4, 0xa2, 0xa4, 0xfc, 0x3c, 0xbd,
    0xe4, 0xfc, 0xdc, 0x5c, 0x20, 0x55, 0x9c, 0x5c, 0x94, 0x59, 0x50, 0xa2, 0x97, 0x97, 0x5a, 0xa2,
    0x97, 0x9b, 0x98, 0x9c, 0x91, 0xef, 0x97, 0x5a, 0x12, 0x90, 0x98, 0x9c, 0x0d, 0xe4, 0x3a, 0x27,
    0xe6, 0xe4, 0x04, 0xa5, 0x16, 0x8a, 0xf0, 0xb1, 0xb1, 0x89, 0x0b, 0x9b, 0x12, 0xd4, 0xe5, 0x98,
    0x92, 0x52, 0x94, 0x5a, 0x5c, 0xac, 0xe7, 0x0b, 0x32, 0x05, 0xca, 0x11, 0x61, 0x61, 0x63, 0xe2,
    0xe0, 0x64, 0x24, 0x5f, 0x3b, 0x27, 0xa7, 0x30, 0x07, 0xcc, 0x59, 0x8c, 0x9c, 0xaa, 0x3a, 0x1c,
    0xda, 0x72, 0x75, 0x20, 0x6f, 0x88, 0xb0, 0x70, 0x0a, 0xf3, 0xb9, 0xa7, 0x96, 0x04, 0xa7, 0x16,
    0x95, 0x65, 0x26, 0xa7, 0x7a, 0xe6, 0xa5, 0xe5, 0xab, 0x88, 0x31, 0x72, 0x0a, 0xb6, 0x33, 0x32,
    0x0a, 0xab, 0x98, 0x24, 0x1a, 0xa7, 0x25, 0x9a, 0xa5, 0xa5, 0xe8, 0xa6, 0x24, 0xa6, 0x9a, 0xe9,
    0x9a, 0x18, 0x26, 0x99, 0xea, 0x5a, 0x5a, 0x98, 0x1a, 0xeb, 0x26, 0x27, 0x1b, 0xa5, 0xa5, 0xa6,
    0x9a, 0x27, 0x27, 0x5a, 0x24, 0xa6, 0x30, 0x82, 0x01, 0x00, 0xa8, 0xe4, 0x52, 0xb7,
];

#[test]
fn the_real_clients_first_call_is_answered() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    client.send_raw_payload(REAL_CLIENT_GET_SERVICE_INFO);
    assert_eq!(
        expect_call_rsp(&client.recv()),
        PyValue::Dict(vec![]),
        "the real client's GetServiceInfo must come back as an empty dict"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// The `PingReq` the first real client sent immediately after its first
/// service call, byte for byte, from capture `1792405830-0002.evecap` frame 13.
///
/// It went unanswered twice over: the server could not even decode it (it
/// carries Windows FILETIMEs under tag `0x2F`, which was unimplemented), and
/// `PingReq` had no handler. Its `times` list holds one `client::start` triple.
const REAL_CLIENT_PING_REQ: &[u8] = &[
    0x78, 0x01, 0xab, 0x63, 0x00, 0x02, 0x71, 0x61, 0xfd, 0xe4, 0xc4, 0xa2, 0xa4, 0xfc, 0x3c, 0xbd,
    0xe4, 0xfc, 0xdc, 0x5c, 0x20, 0x55, 0x9c, 0x5c, 0x94, 0x59, 0x50, 0xa2, 0x97, 0x97, 0x5a, 0xa2,
    0x97, 0x9b, 0x98, 0x9c, 0x91, 0xef, 0x97, 0x5a, 0x12, 0x90, 0x98, 0x9c, 0x0d, 0xe4, 0x06, 0x64,
    0xe6, 0xa5, 0x07, 0xa5, 0x16, 0x8a, 0xf0, 0xb1, 0x89, 0x88, 0x0b, 0x9b, 0x12, 0xd4, 0xe5, 0x98,
    0x92, 0x52, 0x94, 0x5a, 0x5c, 0xac, 0xe7, 0x0b, 0x32, 0x05, 0xca, 0x11, 0x61, 0x61, 0x63, 0xe2,
    0x60, 0x63, 0x62, 0x24, 0x5b, 0x3f, 0x33, 0x1b, 0x07, 0x23, 0x23, 0xa7, 0xaa, 0xba, 0x08, 0xb3,
    0x3e, 0xc7, 0x91, 0xd7, 0x01, 0x77, 0xb7, 0xc4, 0xdf, 0x65, 0xd4, 0xe7, 0x08, 0xbe, 0x99, 0x00,
    0x66, 0xe9, 0xf1, 0x26, 0xe7, 0x64, 0xa6, 0xe6, 0x95, 0x58, 0x59, 0x15, 0x97, 0x24, 0x16, 0x95,
    0x30, 0x32, 0x0a, 0xab, 0xa4, 0xa6, 0x98, 0x98, 0x59, 0x98, 0x99, 0x24, 0xeb, 0x9a, 0xa6, 0x1a,
    0x26, 0xe9, 0x9a, 0x24, 0x99, 0x25, 0xeb, 0x5a, 0x98, 0xa7, 0x25, 0xea, 0x26, 0x5a, 0x1a, 0x19,
    0x24, 0x1a, 0xa6, 0x99, 0x1a, 0x98, 0x9b, 0x1b, 0x30, 0x82, 0x01, 0x00, 0x97, 0xd6, 0x55, 0xfe,
];

#[test]
fn the_real_clients_ping_is_answered_with_its_own_times_plus_ours() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    client.send_raw_payload(REAL_CLIENT_PING_REQ);
    let packet = client.recv();

    let PyValue::Instance { class, state } = &packet else {
        panic!("not an instance: {packet:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.PingRsp"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    assert_eq!(t[0], PyValue::Int(21), "command 21 = PingRsp");

    // params for PingRsp are ['times'], so body[0] is the list.
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::List(times) = &body[0] else {
        panic!("times must be a list, got {:?}", body[0])
    };

    // A ping is annotated, not acknowledged: the client's own entry must still
    // be there, with the hop chain appended after it.
    assert_eq!(
        times.len(),
        1 + 7,
        "client's entry plus the proxy/server hops"
    );
    let PyValue::Tuple(theirs) = &times[0] else {
        panic!("entry 0 is not a tuple")
    };
    assert_eq!(
        theirs[2],
        PyValue::Unicode("client::start".to_owned()),
        "the client's own label must survive"
    );
    // Its FILETIMEs decoded through tag 0x2F; this is the capture's own second.
    let PyValue::Int(ticks) = theirs[0] else {
        panic!("a ping time is not an integer")
    };
    assert_eq!(ticks / 10_000_000 - 11_644_473_600, 1_792_405_830);

    // The order machoNet.Ping requires, unconditionally:
    // it takes the position of `proxy::writing` and then of
    // `proxy::handle_message` searching from there. (`[V]`)
    // Answering with a single 'proxy::turnaround' made the client raise
    // ValueError: 'proxy::writing' is not in list.
    let labels: Vec<String> = times
        .iter()
        .map(|t| match t {
            PyValue::Tuple(f) => match &f[2] {
                PyValue::Unicode(s) => s.clone(),
                other => panic!("a ping label must be unicode, got {other:?}"),
            },
            other => panic!("a ping entry must be a 3-tuple, got {other:?}"),
        })
        .collect();
    assert_eq!(labels[0], "client::start", "the client's entry stays first");
    let server1 = labels
        .iter()
        .position(|l| l == "proxy::writing")
        .expect("machoNet.Ping indexes 'proxy::writing' and will raise without it");
    let server2 = labels[server1..]
        .iter()
        .position(|l| l == "proxy::handle_message")
        .expect("'proxy::handle_message' must follow 'proxy::writing'");
    assert!(server2 > 0, "the two labels must be distinct entries");
    for t in &times[1..] {
        let PyValue::Tuple(f) = t else { panic!() };
        assert_eq!(f.len(), 3, "every ping entry is (start, now, label)");
    }

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn the_item_descriptor_is_a_real_dbrowdescriptor() {
    // invCache.GetItemHeader blocks on this call, and godma.sublocrd then
    // does `for key, dbtype, size in list(self.itemrd)`. An unanswered call
    // here hung the session's ApplyInitialState and left the client on a
    // black screen, so this checks both that it is answered and that what
    // comes back is something the client can iterate.
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    client.send(&call_req("invbroker", "GetItemDescriptor", 1));
    let result = expect_call_rsp(&client.recv());

    let PyValue::ObjectEx { header, .. } = &result else {
        panic!("not an ObjectEx: {result:?}")
    };
    // header = (global 'blue.DBRowDescriptor', (columns,))
    let PyValue::Tuple(h) = &**header else {
        panic!("header is not a tuple")
    };
    let PyValue::Global(class) = &h[0] else {
        panic!("header[0] is not a global: {:?}", h[0])
    };
    assert_eq!(String::from_utf8_lossy(class), "blue.DBRowDescriptor");
    let PyValue::Tuple(outer) = &h[1] else {
        panic!("descriptor args is not a tuple")
    };
    let PyValue::Tuple(columns) = &outer[0] else {
        panic!("descriptor columns is not a tuple")
    };
    let names: Vec<String> = columns
        .iter()
        .map(|c| match c {
            PyValue::Tuple(p) => match &p[0] {
                PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                other => panic!("column name is not a string: {other:?}"),
            },
            other => panic!("a column is not a pair: {other:?}"),
        })
        .collect();
    // The order is CCP's, and **this assertion previously encoded a wrong
    // one** -- it is the second time this descriptor has been wrong and the
    // second time a test agreed with it.
    //
    // The error was a missing `customInfo` at index 8, which shifted
    // `stacksize` to 8 and `singleton` to 9. The client's row indices are
    // hard-coded and `ixSingleton` is **10**, so `undockingSvc.ShipPicker`
    // and `station.base.SelectShipDlg` -- both of which subscript a hangar
    // row at `const.ixSingleton` after the `categoryShip` test the capsule
    // passes -- were reading past the end of every row we sent.
    //
    // **The cause is worth more than the fix: the indices are split across
    // two files.** `const.pyc` holds `ixItemID=0 .. ixQuantity=5`, which is
    // what the earlier pass found and cited right here; `ixGroupID=6`
    // through `ixSingleton=10` are in `inventorycommon/const.pyc`, which it
    // never opened. Stopping at the first file that answers the question is
    // how you get six correct indices and a confident comment. All eleven
    // are now read from both files together.
    //
    // It is asserted here rather than just checked for `itemID` because the
    // client takes the descriptor from us and builds rows against it, so a
    // wrong order is self-consistent right up until something reads a row
    // positionally -- and then it is an IndexError with no traceback of
    // ours.
    // CCP's shape: nine physical columns; stacksize and singleton are
    // virtual and travel as the header's state (below).
    assert_eq!(
        names,
        vec![
            "itemID",
            "typeID",
            "ownerID",
            "locationID",
            "flagID",
            "quantity",
            "groupID",
            "categoryID",
            // `ixCustomInfo` = 8.
            "customInfo",
        ],
        "the physical column order is confirmed; see service::item_columns"
    );
    // The third header element: the virtual columns, stacksize first so they
    // land at ixStackSize 9 and ixSingleton 10.
    let PyValue::List(state) = &h[2] else {
        panic!("header[2] is not the virtual-column list: {:?}", h[2])
    };
    let virt: Vec<(String, String)> = state
        .iter()
        .map(|e| match e {
            PyValue::Tuple(p) => match (&p[0], &p[1]) {
                (PyValue::Bytes(n), PyValue::Global(g)) => (
                    String::from_utf8_lossy(n).into_owned(),
                    String::from_utf8_lossy(g).into_owned(),
                ),
                other => panic!("a virtual column is not (Bytes, Global): {other:?}"),
            },
            other => panic!("a virtual column is not a pair: {other:?}"),
        })
        .collect();
    assert_eq!(
        virt,
        vec![
            (
                "stacksize".to_string(),
                "eve.common.script.sys.eveCfg.StackSize".to_string()
            ),
            (
                "singleton".to_string(),
                "eve.common.script.sys.eveCfg.Singleton".to_string()
            ),
        ]
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn the_owner_and_location_primes_are_answered_in_the_tuple_form() {
    // cfg.Recordset._Prime does `fetch = conf.<dbMultiFetcher>(keysToGet)`
    // and then `if type(fetch) is TupleType: self.header, data = fetch`, so
    // the reply must be an actual 2-tuple. Leaving these unanswered hung the
    // client right after GetCharacterSelectionData -- a black screen one call
    // short of the character select.
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);

    // The default sandbox character: id 90000001 in corporation 1000001,
    // docked at station 60000004 in system 30000001.
    let ids = |v: Vec<i64>| PyValue::List(v.into_iter().map(PyValue::Int).collect());

    for (method, arg, want_first) in [
        (
            "GetMultiOwnersEx",
            ids(vec![90_000_001, 1_000_001]),
            "ownerID",
        ),
        (
            "GetMultiCorpTickerNamesEx",
            ids(vec![1_000_001]),
            "corporationID",
        ),
        (
            "GetMultiLocationsEx",
            ids(vec![60_700_004, 30_000_001]),
            "locationID",
        ),
    ] {
        client.send(&call_req_args("config", method, 1, vec![arg]));
        let result = expect_call_rsp(&client.recv());
        let PyValue::Tuple(pair) = &result else {
            panic!("{method} must return a (header, data) tuple, got {result:?}")
        };
        assert_eq!(pair.len(), 2, "{method}: (header, data)");
        let PyValue::List(header) = &pair[0] else {
            panic!("{method}: header is not a list")
        };
        assert_eq!(
            header[0],
            bstr(want_first),
            "{method}: the client pre-sets this column order on itself"
        );
        let PyValue::List(rows) = &pair[1] else {
            panic!("{method}: data is not a list")
        };
        assert!(!rows.is_empty(), "{method}: the ids we know must come back");
    }

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// The bind protocol, end to end: resolve the node, then bind-and-call.
///
/// This is the round trip the client makes the moment a character is clicked
/// (`characterSelection.TryEnterGame`), and it is the first thing on this
/// server that answers with an *object* rather than a value. Both halves are
/// asserted against what the client's own code does with them:
///
/// - `MachoResolveObject` must return a node id. `Moniker.__ResolveImpl`
///   treats `None` as 'Resolution failure: ResolveObject returned None'.
/// - `MachoBindObject(bindParams, call)` must return the 2-tuple
///   `(boundObject, retval)`, with the fused first call already executed --
///   `Moniker.Bind` unpacks exactly two values.
/// - the reference must survive `ObjectCall.ParseObjectID`, which loads the
///   nested stream and splits the id on a single colon.
#[test]
fn a_bind_resolves_a_node_then_answers_with_an_object_and_the_fused_call() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    let _session = client.recv();

    // Step 1: which node owns this object? bindParams = (charID, groupCharacter).
    let bind_params = PyValue::Tuple(vec![PyValue::Int(90_000_001), PyValue::Int(1)]);
    client.send(&call_req_args(
        "populationCap",
        "MachoResolveObject",
        1,
        vec![bind_params.clone()],
    ));
    let node = expect_call_rsp(&client.recv());
    let PyValue::Int(node_id) = node else {
        panic!("MachoResolveObject must return a node id, got {node:?}")
    };

    // Step 2: bind, with the first call fused in.
    client.send(&call_req_args(
        "populationCap",
        "MachoBindObject",
        2,
        vec![
            bind_params,
            PyValue::Tuple(vec![
                bstr("GetCharacterLoadSlot"),
                PyValue::Tuple(vec![PyValue::Int(90_000_001)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let result = expect_call_rsp(&client.recv());
    let PyValue::Tuple(pair) = &result else {
        panic!("MachoBindObject must return (boundObject, retval), got {result:?}")
    };
    assert_eq!(pair.len(), 2, "Moniker.Bind unpacks exactly two values");

    // The retval is the fused call's answer: no alternative load locations,
    // which is what keeps the client out of MoveCharacterToNewSystem.
    assert_eq!(
        pair[1],
        PyValue::Dict(vec![]),
        "GetCharacterLoadSlot must be answered in the same round trip"
    );

    // The bound object is a 0x19 callback carrying a nested marshal stream.
    let PyValue::Callback(child) = &pair[0] else {
        panic!(
            "a bound object travels as a Load-callback object, got {:?}",
            pair[0]
        )
    };
    let PyValue::Bytes(inner) = child.as_ref() else {
        panic!("the callback child is a byte string, got {child:?}")
    };
    let reference = eve_server::loads(inner).expect("the nested stream loads");
    let PyValue::Tuple(parts) = &reference else {
        panic!("a reference is (objectID, refID), got {reference:?}")
    };
    assert_eq!(parts.len(), 2, "(objectID, refID)");
    let PyValue::Bytes(object_id) = &parts[0] else {
        panic!("objectID is a string, got {:?}", parts[0])
    };
    let object_id = String::from_utf8_lossy(object_id).into_owned();

    // ParseObjectID does `nid, oid = objectID.split(':')`, so exactly one
    // colon, and it must name the node we just resolved to -- the client
    // addresses every later call on this object there.
    assert_eq!(
        object_id.matches(':').count(),
        1,
        "ParseObjectID splits on one colon: {object_id}"
    );
    assert_eq!(
        object_id.split(':').next(),
        Some(format!("N={node_id}").as_str()),
        "the object must belong to the node we resolved to: {object_id}"
    );
    // And it must not look like the client's own identifier, which is the
    // literal 'C=0'; ParseObjectID raises RuntimeError when they match.
    assert_ne!(object_id.split(':').next(), Some("C=0"));

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// Selecting a character sends a session change, not just an answer.
///
/// `PerformSessionChange` throws the return value away; what puts the client
/// in the game is the `SessionChangeNotification`. So the test is about the
/// notification, and about four properties of it that each fail *silently*
/// if they are wrong:
///
/// - `charid` and `stationid` must be present: `gameui.OnSessionChanged`
///   routes on the live session, and `_HandleSessionChangeInStation` then acts
///   only `if 'stationid' in change`.
/// - `locationid` and `corprole` must be **absent**: the client derives both,
///   and sending them fights its own bookkeeping.
/// - every value is a `(old, new)` pair.
/// - `solarsystemid` stays absent while docked; `solarsystemid2` carries it.
#[test]
fn selecting_a_character_changes_the_session() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    let _session_init = client.recv();

    client.send(&call_req_args(
        "charUnboundMgr",
        "SelectCharacterID",
        1,
        vec![
            PyValue::Int(90_000_001),
            PyValue::None,
            PyValue::Bool(false),
        ],
    ));

    // The notification comes first, then the answer.
    let notification = client.recv();
    let PyValue::Instance { class, state } = &notification else {
        panic!("expected a packet, got {notification:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionChangeNotification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("packet state is not a tuple")
    };
    assert_eq!(
        t[0],
        PyValue::Int(16),
        "command 16 = SessionChangeNotification"
    );

    // body = [sid, (0, change), nodesOfInterest]
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(change_pair) = &body[1] else {
        panic!("change must be (0, dict), got {:?}", body[1])
    };
    assert_eq!(
        change_pair[0],
        PyValue::Int(0),
        "the real server sends 0 here"
    );
    let PyValue::Dict(entries) = &change_pair[1] else {
        panic!("change[1] must be a dict, got {:?}", change_pair[1])
    };

    let names: Vec<String> = entries
        .iter()
        .map(|(_v, k)| match k {
            PyValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            other => panic!("attribute name must be a string, got {other:?}"),
        })
        .collect();

    for want in ["charid", "stationid", "corpid", "solarsystemid2"] {
        assert!(
            names.iter().any(|n| n == want),
            "change must carry {want}: {names:?}"
        );
    }
    for forbidden in ["locationid", "corprole", "solarsystemid"] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "{forbidden} is derived by the client and must not be sent: {names:?}"
        );
    }
    for (value, key) in entries {
        let PyValue::Tuple(pair) = value else {
            panic!("{key:?} must be an (old, new) pair, got {value:?}")
        };
        assert_eq!(pair.len(), 2, "{key:?} must be an (old, new) pair");
    }

    // And then the discarded answer.
    assert_eq!(expect_call_rsp(&client.recv()), PyValue::None);

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// A call on an object we handed out, which carries no service name at all.
///
/// This is the second half of the bind protocol and it caught us out on a
/// real client: we handed `warRegistry` a reference, the client bound with no
/// fused call, and then called `GetWars` **on the object**. That packet is
/// addressed to the node and names the object only by its id string, so
/// without a map from id back to service it cannot be dispatched — it showed
/// up in the capture as a call to service `?`.
#[test]
fn a_call_on_a_handed_out_object_is_routed_back_to_its_service() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    let _session_init = client.recv();

    // Bind with no fused call, exactly as the client does for warRegistry.
    client.send(&call_req_args(
        "warRegistry",
        "MachoBindObject",
        1,
        vec![
            PyValue::Tuple(vec![PyValue::Int(1_000_001), PyValue::Int(1)]),
            PyValue::None,
        ],
    ));
    let bound = expect_call_rsp(&client.recv());
    let PyValue::Tuple(pair) = &bound else {
        panic!("expected (boundObject, retval), got {bound:?}")
    };
    assert_eq!(pair[1], PyValue::None, "no fused call means no retval");

    // Dig the objectID out of the reference the way the client would.
    let PyValue::Callback(child) = &pair[0] else {
        panic!("expected a callback reference, got {:?}", pair[0])
    };
    let PyValue::Bytes(inner) = child.as_ref() else {
        panic!("reference child must be bytes")
    };
    let reference = eve_server::loads(inner).expect("reference loads");
    let PyValue::Tuple(parts) = &reference else {
        panic!("reference must be a tuple")
    };
    let PyValue::Bytes(object_id) = &parts[0] else {
        panic!("objectID must be a string")
    };
    let object_id = String::from_utf8_lossy(object_id).into_owned();

    // Now call a method on it. Note the destination: the NODE, with no
    // service name -- the object id in the payload is the only routing
    // information there is.
    let payload = eve_proto::CallPayload::BoundObject {
        object_id,
        method: "GetWars".to_owned(),
        args: vec![PyValue::Int(1_000_001)],
        kwargs: vec![],
    };
    let packet = eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallReq,
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(2),
            service: None,
        },
        eve_proto::MachoAddress::Node {
            node_id: Some(1),
            service: None,
            call_id: None,
        },
        vec![payload.to_payload()],
    );
    client.send(&packet.to_value().expect("a bound CallReq"));

    assert_eq!(
        expect_call_rsp(&client.recv()),
        PyValue::Dict(vec![]),
        "GetWars is iterated with .itervalues(); an empty dict means no wars"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// A plain call whose return value is a live object, not data.
///
/// `skillsvc.GetSkillHandler` assigns the result and then calls some
/// twenty-five methods on it, so no data value can stand in: `None` or an
/// empty container both raise `AttributeError` inside
/// `SkillQueueService.Run`, and `StartServiceRun` then stores that exception
/// and re-raises it on every later `sm.GetService('skillqueue')` — poisoning
/// the service for the rest of the session.
///
/// So the reply is a bare reference (not the `(object, retval)` pair a bind
/// answers with), and a method call on it must route to the synthetic service
/// its methods live under.
#[test]
fn a_call_that_returns_an_object_hands_out_a_usable_reference() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    let _session_init = client.recv();

    client.send(&call_req_args("skillMgr2", "GetMySkillHandler", 1, vec![]));
    let handler = expect_call_rsp(&client.recv());

    // A bare reference, not a pair: `self.skillHandler = ...GetMySkillHandler()`.
    let PyValue::Callback(child) = &handler else {
        panic!("GetMySkillHandler must return an object reference, got {handler:?}")
    };
    let PyValue::Bytes(inner) = child.as_ref() else {
        panic!("reference child must be bytes")
    };
    let PyValue::Tuple(parts) = eve_server::loads(inner).expect("reference loads") else {
        panic!("reference must be (objectID, refID)")
    };
    let PyValue::Bytes(object_id) = &parts[0] else {
        panic!("objectID must be a string")
    };
    let object_id = String::from_utf8_lossy(object_id).into_owned();

    // The first thing the client does with it, from `PrimeSkillQueue`.
    let payload = eve_proto::CallPayload::BoundObject {
        object_id,
        method: "GetSkillQueueAndFreePoints".to_owned(),
        args: vec![],
        kwargs: vec![],
    };
    let packet = eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallReq,
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(2),
            service: None,
        },
        eve_proto::MachoAddress::Node {
            node_id: Some(1),
            service: None,
            call_id: None,
        },
        vec![payload.to_payload()],
    );
    client.send(&packet.to_value().expect("a bound CallReq"));

    let result = expect_call_rsp(&client.recv());
    let PyValue::Tuple(pair) = &result else {
        panic!("destructured on arrival as (skillQueue, freeSkillPoints), got {result:?}")
    };
    assert_eq!(pair.len(), 2, "(skillQueue, freeSkillPoints)");
    assert_eq!(
        pair[0],
        PyValue::List(vec![]),
        "an empty queue trains nothing"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// A `Notification` the server sent us, unpacked down the client's own GPCS
/// chain to `(broadcastID, args)`.
///
/// Mirrors what `ObjectCall.NotifyUp` -> `ServiceCall.NotifyUp` ->
/// `BroadcastStuff.NotifyUp` do in the client, in that order. Doing it here
/// rather than asserting on bytes is what makes this test say "the client
/// would have dispatched this event with these arguments" rather than "the
/// frame looked right".
/// An item change goes out as `OnItemsChanged([row], change, location)`, as
/// a live server sends it (`eve_server::notify::ItemChange::to_wire`): the
/// arguments with the one row taken out of its list, so the assertions below
/// read `(row, change, location)` as the client's per-row `OnItemChange` does.
fn one_row(mut args: Vec<PyValue>) -> Vec<PyValue> {
    let PyValue::List(rows) = &args[0] else {
        panic!("OnItemsChanged's first argument is a list: {:?}", args[0])
    };
    assert_eq!(rows.len(), 1, "one row per push");
    args[0] = rows[0].clone();
    args
}

fn expect_notification(v: &PyValue) -> (String, Vec<PyValue>) {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.Notification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    assert_eq!(t[0], PyValue::Int(12), "command 12 = Notification");

    // The destination is a `machoNetAddress.MachoAddress` instance whose
    // state is the broadcast 4-tuple `(4, broadcastID, narrowcast, idtype)` --
    // an instance and not a bare tuple, because `__getstate__`/`__setstate__`
    // are what the client reconstructs it with.
    let PyValue::Instance {
        class: dest_class,
        state: dest,
    } = &t[2]
    else {
        panic!("destination is not an address instance: {:?}", t[2])
    };
    assert_eq!(
        String::from_utf8_lossy(dest_class),
        "carbon.common.script.net.machoNetAddress.MachoAddress"
    );
    let PyValue::Tuple(dest) = &**dest else {
        panic!("the address state is not a tuple")
    };
    assert_eq!(dest[0], PyValue::Int(4), "address type 4 = BROADCAST");
    let PyValue::Bytes(event) = &dest[1] else {
        panic!("broadcastID must be a byte string")
    };
    assert_eq!(
        dest[3],
        PyValue::Bytes(b"*clientID".to_vec()),
        "the star is added by ScattercastWithoutTheStars, so it is on the wire"
    );

    // `oob` absent: a sequence number here parks the client's tasklet forever.
    assert_eq!(t[5], PyValue::None, "oob must be absent on a notification");

    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(outer) = &body[0] else {
        panic!("the payload is a tuple")
    };
    assert_eq!(
        outer[0],
        PyValue::Int(0),
        "0 takes ObjectCall's else branch"
    );
    let PyValue::Bytes(pickle) = &outer[1] else {
        panic!("the inner payload must be a byte string, not a substream")
    };
    let PyValue::Tuple(after_object) = loads(pickle).expect("the inner pickle") else {
        panic!("the pickle is a tuple")
    };
    assert_eq!(
        after_object[0],
        PyValue::Int(0),
        "ServiceCall's else branch"
    );
    let PyValue::Tuple(after_service) = &after_object[1] else {
        panic!("payload[1] is a tuple")
    };
    assert_eq!(
        after_service[0],
        PyValue::Int(1),
        "1 is what makes BroadcastStuff dispatch"
    );
    let PyValue::Tuple(args) = &after_service[1] else {
        panic!("args is a tuple")
    };
    (String::from_utf8_lossy(event).into_owned(), args.clone())
}

/// `/giveisk` answers the console **and** tells the client, in that order, in
/// one flush.
///
/// This is the first thing this server ever says that the client did not ask
/// for, end to end over a real socket. It also closes a hole that was not
/// visible from either side alone: the grant used to be unobservable by every
/// route, because `WalletSvc.Run` caches `GetCashBalance(0)` once, so
/// reopening the wallet does not re-ask, and -- at the time this was written
/// -- a reconnect built a fresh `Registry`. That last half is no longer true:
/// see `a_grant_survives_the_connection_that_made_it` below, which exists
/// because it was.
#[test]
fn giveisk_answers_the_console_and_then_pushes_the_new_balance() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/giveisk 1000000 ".to_vec())],
    ));

    // The answer first: the console line is the reply to the call.
    let answer = expect_call_rsp(&client.recv());
    let PyValue::Bytes(line) = &answer else {
        panic!("the console stringifies the result, got {answer:?}")
    };
    let line = String::from_utf8_lossy(line);
    assert!(line.contains("1000000"), "{line}");

    // Then the push, in the same flush, which the client did not ask for.
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "OnAccountChange");
    assert_eq!(
        args.len(),
        3,
        "OnAccountChange(accountKey, ownerID, balance)"
    );
    assert_eq!(
        args[0],
        PyValue::Bytes(b"cash".to_vec()),
        "the string 'cash', not appConst.accountingKeyCash -- WalletSvc \
         compares it to a string and an integer updates nothing"
    );
    // The character id, not the user id: `ownerID == session.charid`.
    assert_eq!(args[1], PyValue::Int(90000001));
    assert_eq!(args[2], PyValue::Real(1_000_000.0));

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **`/createitem` puts a ship in the hangar, and tells the client to re-ask.**
///
/// Two things in one flush, and the second is the one that was missing: the
/// item existing is not the same as the client being able to see it.
/// `invCacheContainer.listed` has no TTL, so without the push the player can
/// close and reopen the inventory all evening and the window emits no call at
/// all.
///
/// The push argument is a **list** even for one location, because the handler
/// is `for locID in locationIDs:` -- and a `TypeError` raised in there is
/// swallowed by `ExceptionWrapper.NotifyUp`, so getting it wrong would leave
/// no traceback on either side and an inventory that just never refreshes.
#[test]
fn createitem_puts_a_ship_in_the_hangar_and_asks_the_client_to_re_ask() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/createitem Rifter 2 ".to_vec())],
    ));

    let answer = expect_call_rsp(&client.recv());
    let PyValue::Bytes(line) = &answer else {
        panic!("the console stringifies the result, got {answer:?}")
    };
    let line = String::from_utf8_lossy(line);
    assert!(line.contains("Rifter"), "{line}");
    assert!(line.contains('2'), "{line}");

    // **`OnItemChange` comes first now**, one per item created, and the
    // invalidation follows. The order is not arbitrary: the item has to exist
    // in the client's cache before the container is dropped, or the re-ask
    // races the add.
    let (event, _) = expect_notification(&client.recv());
    assert_eq!(
        event, "OnItemsChanged",
        "per-item push precedes the invalidation"
    );
    let (event, _) = expect_notification(&client.recv());
    assert_eq!(event, "OnItemsChanged", "two Rifters, two pushes");

    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "OnInvalidateCacheForLocations");
    assert_eq!(args.len(), 1, "one argument: the location id list");
    assert_eq!(
        args[0],
        PyValue::List(vec![PyValue::Int(60012526)]),
        "a list, not a bare id -- the handler iterates it"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **`/createitem` pushes `OnItemChange`, and the row is a real packed
/// `DBRow`.**
///
/// This is the push that makes a mutator visible in a window that is *already
/// open*, so it is the one that matters for 618 call sites' worth of mutators
/// that discard their return value. Three things are asserted because each is
/// a different kind of failure:
///
/// - the row is a `DbRow`, not a `KeyVal` -- two handlers do
///   `blue.DBRow(item)`, which type-checks in native code;
/// - the `change` dict is `{3: 0}` and not `{0: 3}`, because marshal entries
///   are `(value, key)` and the inverted form would be a quiet instruction to
///   rewrite the item's own id;
/// - `location` is `None`.
#[test]
fn createitem_pushes_a_real_dbrow_and_an_integer_keyed_change() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/createitem EMP S 500 ".to_vec())],
    ));
    expect_call_rsp(&client.recv());

    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "OnItemsChanged");
    assert_eq!(args.len(), 3, "OnItemsChanged([item], change, location)");
    let args = one_row(args);

    assert!(
        matches!(args[0], PyValue::DbRow(_)),
        "the item must be a packed DBRow -- `blue.DBRow(item)` is applied to \
         it in two handlers: {:?}",
        args[0]
    );

    assert_eq!(
        args[1],
        PyValue::Dict(vec![(PyValue::Int(0), PyValue::Int(3))]),
        "change is {{ixLocationID: 0}}; the wire pair is (value, key), so the \
         value 0 comes first and the key 3 second"
    );

    assert_eq!(
        args[2],
        PyValue::None,
        "location is ignored by every handler"
    );

    // The cache invalidation still follows, because it covers the window that
    // is *not* open: `invCacheContainer.listed` has no TTL.
    let (event, _) = expect_notification(&client.recv());
    assert_eq!(event, "OnInvalidateCacheForLocations");

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **A real EVE mission, offered and accepted over a real socket.**
///
/// Mission 4832, "Slave Shipment", from agent 3010110 at our own station. The
/// point of doing it here rather than only in a unit test is that every other
/// claim about this conversation has been *read* out of the client and not
/// run -- so this at least proves the frames leave, in the right order, with
/// the right arities.
///
/// What it cannot prove is what the player sees: the message ids are CCP's
/// and the client resolves them against its own corpus, which no test of ours
/// can reach.
#[test]
fn a_real_mission_is_offered_accepted_and_pushed() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    // Opening the conversation: the offer, with two buttons.
    client.send(&call_req_args("agentMgr", "DoAction", 80, vec![]));
    let PyValue::Tuple(outer) = expect_call_rsp(&client.recv()) else {
        panic!("DoAction is a 2-tuple")
    };
    let PyValue::Tuple(pair) = &outer[0] else {
        panic!("(agentSays, actions)")
    };
    let PyValue::Tuple(says) = &pair[0] else {
        panic!("agentSaysMsg is (messageID, contentID)")
    };
    assert_eq!(
        says[0],
        PyValue::Int(134_918),
        "the offer line is CCP's message 134918, not a greeting of ours"
    );
    assert_eq!(says[1], PyValue::Int(4832), "contentID beside it");
    let PyValue::List(actions) = &pair[1] else {
        panic!("availableActions is a list")
    };
    assert_eq!(actions.len(), 2, "Accept and Decline");

    // Accept: the acceptance line, the cargo pushes, and the journal push.
    client.send(&call_req_args(
        "agentMgr",
        "DoAction",
        81,
        vec![PyValue::Int(2)],
    ));
    let PyValue::Tuple(outer) = expect_call_rsp(&client.recv()) else {
        panic!("DoAction is a 2-tuple")
    };
    let PyValue::Tuple(pair) = &outer[0] else {
        panic!("(agentSays, actions)")
    };
    let PyValue::Tuple(says) = &pair[0] else {
        panic!("agentSaysMsg")
    };
    assert_eq!(
        says[0],
        PyValue::Int(134_919),
        "\"Well said, citizen. Good luck.\" -- CCP's 134919"
    );

    // The pushes, in order: the cargo item, the cache drop, then the journal.
    // `OnAgentMissionChange` last is not arbitrary -- the journal re-reads on
    // it, so anything it should see has to already be true.
    let mut events = Vec::new();
    for _ in 0..3 {
        let (event, args) = expect_notification(&client.recv());
        if event == "OnAgentMissionChange" {
            assert_eq!(args.len(), 2, "OnAgentMissionChange(change, agentID)");
            assert_eq!(
                args[0],
                PyValue::Bytes(b"accepted".to_vec()),
                "the first argument is a string, compared against literals"
            );
            assert_eq!(args[1], PyValue::Int(3_010_110));
        }
        events.push(event);
    }
    assert!(
        events.contains(&"OnItemsChanged".to_string()),
        "the briefing tells the player to load cargo, so it must exist: {events:?}"
    );
    assert_eq!(
        events.last().map(String::as_str),
        Some("OnAgentMissionChange"),
        "the journal push goes last, after what it will read is true: {events:?}"
    );

    // And the objective now describes a courier run.
    client.send(&call_req_args(
        "agentMgr",
        "GetMissionObjectiveInfo",
        82,
        vec![],
    ));
    let info = expect_call_rsp(&client.recv());
    let objectives =
        eve_server::pyutil::dict_get(&info, b"objectives").expect("objectives is mandatory");
    let PyValue::List(objectives) = objectives else {
        panic!("objectives is a list")
    };
    assert_eq!(objectives.len(), 1, "one courier leg");

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **The world outlives the process, which is the clause that was still
/// false.**
///
/// The owner's test for the skill work is "buy a skill, see it trained,
/// relaunch, still trained", and only the last word failed: `tools/up.sh`
/// restarts the server, so every grant died with it.
///
/// This is deliberately harsher than a round-trip of the file. It grants ISK,
/// a ship and a skill over one connection, **drops the entire `Server`** --
/// listener, world, ticker, every `Arc` -- and builds a second one from
/// nothing but the path on disk. Anything surviving that got there through
/// the file.
#[test]
fn the_world_survives_the_server_being_dropped() {
    let dir = std::env::temp_dir().join(format!(
        "eve-persist-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let world = dir.join("world.txt");
    let _ = std::fs::remove_dir_all(&dir);

    let start = |world: &std::path::Path| {
        let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
        config.mode = Mode::Gps;
        config.world_file = Some(world.to_path_buf());
        let server = Server::bind(config).expect("bind");
        let addr = server.local_addr().expect("addr");
        let handle = std::thread::spawn(move || server.run_once().expect("serve"));
        (addr, handle)
    };
    let slash = |client: &mut MockClient, call_id: i64, line: &str| {
        client.send(&call_req_args(
            "slash",
            "SlashCmd",
            call_id,
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        ));
        expect_call_rsp(&client.recv())
    };

    // --- first process: change things ---
    {
        let (addr, handle) = start(&world);
        let mut c = MockClient::connect(addr);
        full_handshake(&mut c);
        read_session(&mut c);
        select_character(&mut c, FIXTURE_CHARACTER, 79);
        slash(&mut c, 80, "/giveisk 4200000 ");
        c.recv(); // the OnAccountChange push rides along
        slash(&mut c, 81, "/createitem Rifter 1 ");
        // Two pushes now: OnItemChange for the window that is open, then
        // OnInvalidateCacheForLocations for one opened later.
        c.recv();
        c.recv();
        slash(&mut c, 82, "/giveskill 3302 4 ");
        c.recv(); // the scheduled OnServerSkillsChanged (a grant sends no forced refresh)
        c.stream.shutdown(Shutdown::Both).expect("shutdown");
        handle.join().expect("join");
    }
    assert!(
        world.exists(),
        "a mutation must have written the world file"
    );

    // --- second process: nothing in memory, only the file ---
    let (addr, handle) = start(&world);
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    // **The second process has to say who it is too.** The balance is the
    // actor's, so a reconnect that never selects a character reads 0.0 --
    // which is the right answer to the wrong question, and would have made
    // this test look like a persistence failure.
    select_character(&mut c, FIXTURE_CHARACTER, 89);

    c.send(&call_req_args(
        "account",
        "GetCashBalance",
        90,
        vec![PyValue::Int(0)],
    ));
    assert_eq!(
        expect_call_rsp(&c.recv()),
        PyValue::Real(4_200_000.0),
        "the ISK must come back from the file, not from the Default seed"
    );

    // The Rifter, in the hangar, alongside the capsule the seed rebuilds.
    c.send(&call_req_args(
        "charMgr",
        "ListIncludingContainers",
        91,
        vec![],
    ));
    let PyValue::List(rows) = expect_call_rsp(&c.recv()) else {
        panic!("ListIncludingContainers is a list of item rows")
    };
    assert_eq!(
        rows.len(),
        2,
        "the seeded capsule plus the Rifter that was granted before the \
         restart"
    );

    // And the skill, which is the owner's own acceptance test.
    c.send(&call_req_args("skillMgr2", "GetMySkillHandler", 92, vec![]));
    c.recv();
    let mut found = false;
    c.send(&call_req_args(
        "skillMgr2::skillHandler",
        "GetSkills",
        93,
        vec![],
    ));
    if let PyValue::Dict(entries) = expect_call_rsp(&c.recv()) {
        found = entries
            .iter()
            .any(|(_, key)| matches!(key, PyValue::Int(3302)));
    }
    assert!(
        found,
        "the granted skill 3302 must still be trained after a restart -- this \
         is the \"relaunch, still trained\" clause"
    );

    c.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The world outlives the connection that changed it.**
///
/// This is the honest test of the shared world, and it is behavioural rather
/// than structural on purpose: asserting that two `GpsSession`s hold the same
/// `Arc` would pass the moment the type changed, whether or not anything the
/// player does survives. So it grants ISK over one connection, drops that
/// connection entirely, and asks a *second* one for the balance.
///
/// Before the world was shared this failed by construction: `serve_one` built
/// each connection its own `Registry` from `config.sandbox.clone()`, so two
/// clients were two universes and a reconnect discarded everything. The
/// comment on the test above still said so.
///
/// What it does **not** prove is persistence across a restart -- the world
/// still dies with the process. That needs the `Store` seam
/// (`docs/reference/world-state.md` §5 step 2), and this test is the one that will
/// catch it when the two are confused.
#[test]
fn a_grant_survives_the_connection_that_made_it() {
    let (addr, handle) = start_twice();

    // First connection: make the grant, then leave.
    let mut first = MockClient::connect(addr);
    full_handshake(&mut first);
    read_session(&mut first);
    select_character(&mut first, FIXTURE_CHARACTER, 79);
    first.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/giveisk 1000000 ".to_vec())],
    ));
    expect_call_rsp(&first.recv());
    // The push rides in the same flush; drain it so the socket is empty.
    first.recv();
    first.stream.shutdown(Shutdown::Both).expect("shutdown");

    // Second connection: a different session, a different `GpsSession`, and
    // the same world.
    let mut second = MockClient::connect(addr);
    full_handshake(&mut second);
    read_session(&mut second);
    // The *same character* on a new connection, which is what makes this a
    // test about the world outliving a session rather than about identity:
    // the balance is the actor's, and a connection with no actor reads 0.0.
    select_character(&mut second, FIXTURE_CHARACTER, 80);
    second.send(&call_req_args(
        "account",
        "GetCashBalance",
        81,
        vec![PyValue::Int(0)],
    ));
    let balance = expect_call_rsp(&second.recv());
    assert_eq!(
        balance,
        PyValue::Real(1_000_000.0),
        "the grant must survive the connection that made it; a per-connection \
         Registry would answer the default balance here"
    );
    second.stream.shutdown(Shutdown::Both).expect("shutdown");

    for end in handle.join().expect("join") {
        assert!(matches!(end, ConnEnd::PeerClosed), "{end}");
    }
}

/// **`/pushtest` proves the thread, which is the part `/giveisk` does not.**
///
/// The console answers immediately; the notification arrives a second later,
/// written by the push ticker, with the client having sent nothing at all in
/// between. That silence is the whole claim: nothing on the client fetches
/// Destiny state, and the client is entitled to say nothing for minutes at a
/// time, so a server that can only answer can never make a ship move.
#[test]
fn pushtest_arrives_later_with_the_client_having_said_nothing() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        81,
        vec![PyValue::Bytes(b"/pushtest 1 ".to_vec())],
    ));
    let answer = expect_call_rsp(&client.recv());
    let PyValue::Bytes(line) = &answer else {
        panic!("the console stringifies the result, got {answer:?}")
    };
    assert!(
        String::from_utf8_lossy(line).contains("queued"),
        "the console should say it queued something"
    );

    // Not one byte is sent between here and the frame below. `recv` blocks on
    // the socket, so if the ticker never writes, this test times out rather
    // than passing vacuously.
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "OnAccountChange");
    assert_eq!(args.len(), 3);

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

// --- two players ----------------------------------------------------------

/// A server that accepts **concurrently**, thread per connection, with two
/// characters in its roster.
///
/// `start_twice` serves two connections in sequence, which is the right shape
/// for testing a shared world and the wrong one for testing two sessions: the
/// first is gone before the second arrives, so no `GetGuests` could ever see
/// both. This uses `Server::run`, which is already thread-per-connection.
///
/// The thread is never joined. `run` loops on `accept` forever and there is
/// no shutdown for it; the process ends when the test binary does.
fn start_two_players() -> SocketAddr {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![alice(), bob()];
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let _ = server.run();
    });
    addr
}

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;

/// Two characters, distinct in every id that is a key somewhere and **in the
/// same station**, which is what makes them visible to each other.
///
/// `ship_id` is not cosmetic: `Registry::new` inserts a capsule at that id
/// and `Store::insert` is a `push`, so two characters sharing one would put
/// two items with one id in the store. `Registry::new` asserts it.
fn alice() -> eve_server::model::Character {
    eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: 100_000_001,
        ..eve_server::model::Character::default()
    }
}

fn bob() -> eve_server::model::Character {
    eve_server::model::Character {
        character_id: BOB,
        name: "Bob".to_owned(),
        ship_id: 100_000_002,
        ..eve_server::model::Character::default()
    }
}

/// **The observable of Round 0.3**, and the cheapest thing two players can
/// see each other through.
///
/// Two real sockets, two real handshakes, two characters selected, and a
/// station lobby that lists both. Everything below the wire was already
/// unit-tested; what this adds is that the two sessions are genuinely
/// concurrent and that the server kept them apart -- which is the one thing a
/// `Registry` test cannot show, because it has no sessions in it.
#[test]
fn two_sessions_see_each_other_in_the_station_lobby() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    // Alice alone first: one guest, herself. A lobby that listed nobody
    // while she is standing in it would look broken rather than empty, and
    // it is also the assertion that fails if `characters_at_station` ever
    // starts excluding the caller.
    assert_eq!(guests(&mut a, 71), vec![ALICE]);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);

    // And now both, from either side.
    let from_alice = guests(&mut a, 72);
    let from_bob = guests(&mut b, 81);
    assert_eq!(from_alice.len(), 2, "two guests: {from_alice:?}");
    assert!(from_alice.contains(&ALICE) && from_alice.contains(&BOB));
    assert_eq!(
        from_alice, from_bob,
        "the lobby is the station's, not the caller's"
    );

    // Bob leaves. The session table is unregistered on `serve_one`'s exit
    // path, so Alice stops seeing him -- which is the half of this that a
    // register-only implementation passes and then leaks forever.
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
    // The unregister happens on the server's connection thread after the
    // read returns, so poll rather than assume it has already run.
    let mut after = guests(&mut a, 73);
    for attempt in 1..=50 {
        if after == vec![ALICE] {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        after = guests(&mut a, 73 + attempt);
    }
    assert_eq!(after, vec![ALICE], "a disconnected guest must not linger");

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// `station.GetGuests`, as the list of character ids it carries.
///
/// Checks the arity of every row on the way through, because a wrong arity is
/// a `ValueError` in the client's own unpack -- caught and logged, guests tab
/// silently empty -- and so is exactly the failure a test has to catch
/// instead.
fn guests(client: &mut MockClient, call_id: i64) -> Vec<i64> {
    client.send(&call_req_args("station", "GetGuests", call_id, vec![]));
    let answer = expect_call_rsp(&client.recv());
    let PyValue::List(rows) = &answer else {
        panic!("GetGuests must answer a list, got {answer:?}")
    };
    rows.iter()
        .map(|row| {
            let (PyValue::List(f) | PyValue::Tuple(f)) = row else {
                panic!("a guest row must be a sequence, got {row:?}")
            };
            assert_eq!(
                f.len(),
                4,
                "unpacked as (charID, corpID, allianceID, warFactionID): {row:?}"
            );
            match f[0] {
                PyValue::Int(id) => id,
                ref other => panic!("charID must be an integer, got {other:?}"),
            }
        })
        .collect()
}

/// A character selection answers with the session change **first**, then the
/// `CallRsp`. The order is contractual and nothing else asserts it.
///
/// `PerformSessionChange` throws the return value away, so what puts the
/// client in the game is the notification. If they ever swap, the client gets
/// an answer to a call it has not been told the session for.
#[test]
fn a_character_selection_answers_with_a_session_change_first() {
    let addr = start_two_players();
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    // `select_character` asserts the order itself; this test is the one that
    // names the property, so that the assertion has somewhere to point when
    // it fails.
    select_character(&mut c, BOB, 70);
    assert_eq!(guests(&mut c, 71), vec![BOB]);
    c.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// **Before a character is chosen, a call must never act as someone else.**
///
/// A call made before any character is selected must answer its no-character
/// branch -- never the first character in the roster. This is the one that
/// fails if anyone reintroduces a default, and it has to be on the wire
/// rather than against a `Registry`, because the thing being tested is that
/// `session.character_id` starts as `None`.
#[test]
fn a_call_before_character_selection_does_not_answer_for_a_character() {
    let addr = start_two_players();
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);

    // No `select_character` here. That is the test.
    assert!(
        guests(&mut c, 70).is_empty(),
        "a session with no character is not standing in a station"
    );

    // **The assertion is "not Alice", not "None".** This used to expect
    // `PyValue::None`, which was the implementation rather than the property:
    // `crimewatchSvc.GetSlimItemDataForCharID` reads `pubInfo.corporationID`,
    // `.warFactionID` and `.allianceID` unguarded, so `None` is an
    // `AttributeError` on the client and cannot be the right answer to
    // anything. What must hold is that a session which is nobody is not told
    // about a real character.
    c.send(&call_req_args("charMgr", "GetPublicInfo", 71, vec![]));
    let answer = expect_call_rsp(&c.recv());
    let PyValue::Instance { state, .. } = &answer else {
        panic!("GetPublicInfo answers a KeyVal instance, got {answer:?}")
    };
    for id in [ALICE, BOB] {
        assert_ne!(
            dict_get(state, "characterID"),
            Some(&PyValue::Int(id)),
            "a session with no character was told about {id}"
        );
    }
    // And the three fields crimewatch reads unguarded are all present, which
    // is the whole reason this answer is a KeyVal and not nothing.
    for key in ["corporationID", "allianceID", "warFactionID"] {
        assert!(
            dict_get(state, key).is_some(),
            "crimewatch reads {key} with no guard; it must exist"
        );
    }

    c.send(&call_req_args(
        "account",
        "GetCashBalance",
        72,
        vec![PyValue::Int(0)],
    ));
    assert_eq!(
        expect_call_rsp(&c.recv()),
        PyValue::Real(0.0),
        "the balance of nobody is 0.0, not Alice's"
    );

    c.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// **The push ordering guarantee, pinned.**
///
/// `conn::serve` clones `Reply::payloads` and then *appends* every
/// `Duration::ZERO` push to the same `send_all`, under one `Wire` lock. So a
/// zero-delay push is written after every payload of that reply, in the order
/// the arm queued them, and no other connection's frame can land between.
/// Nothing asserted that before this test; `giveisk_answers_the_console_...`
/// relies on it and would have failed with a confusing message if it changed.
///
/// It is load-bearing because some client handlers **silently drop** a change
/// for an item that is not yet in a cache which the in-flight call's own
/// response fills: `clientDogmaLocation.OnModuleAttributeChanges` skips any
/// item where `IsItemLoaded` is false -- in one branch with no log at all --
/// and godma's `ApplyAttributeChange` returns after a single `LogInfo`. A
/// push that overtakes its response is a change that never happened and left
/// nothing to find.
///
/// `/createitem` is the case worth testing rather than `/giveisk`, because it
/// queues **two** pushes: the order among them is a second property, and
/// `OnInvalidateCacheForLocations` arriving before the `OnItemChange` it is
/// meant to follow is exactly the shape of bug this pins.
#[test]
fn a_zero_delay_push_is_sent_after_the_payloads_in_one_flush() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/createitem Rifter 1 ".to_vec())],
    ));

    // Frame 1 must be the answer. Read as a `CallRsp` specifically: a
    // notification here would be a `ValueError` in the client's own
    // `BroadcastStuff` dispatch, not a mis-ordered but harmless frame.
    let answer = expect_call_rsp(&client.recv());
    assert!(
        matches!(answer, PyValue::Bytes(_)),
        "the console line is the answer to the call, got {answer:?}"
    );

    // Then the pushes, in the order the arm queued them.
    let (first, _) = expect_notification(&client.recv());
    let (second, _) = expect_notification(&client.recv());
    assert_eq!(
        (first.as_str(), second.as_str()),
        ("OnItemsChanged", "OnInvalidateCacheForLocations"),
        "the item has to exist for the window before the window is told to \
         re-read it"
    );

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **A notification reaches a character who is not the caller.**
///
/// This is the one property that `docs/lanes/fleet.md` §4 said was missing,
/// and the reason it is a loopback test rather than a unit test is that it is
/// a claim about *two sockets*: Alice calls `Invite`, and the frame has to
/// come out of **Bob's** connection. No `Registry` test can show that, because
/// a registry has no sockets in it.
///
/// Before this worked, `GpsSession::notify` addressed every frame to
/// `self.session.client_id` and `pending_pushes` drained one global queue on
/// whichever session was mid-payload — so a push could only ever reach its
/// own caller. Fine for `OnAccountChange`, and exactly wrong for a fleet
/// invite, whose entire point is that somebody else finds out.
/// `docs/reference/push-path.md:786-788` predicted it: "the day a second player exists
/// the narrowcast is the field a bug would hide in."
#[test]
fn a_fleet_invite_arrives_on_the_other_players_socket() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    guest_arrival(&mut a, BOB);

    // Alice forms a fleet. `CreateFleet` is object-returning, so the answer
    // is a reference — what matters here is that the arm ran at all, which is
    // the dispatch-before-mint change.
    // Alice forms a fleet, in the client's own order: `CreateFleet` returns
    // the object, `Init` makes the creator its first member
    // (`fleetSvc.py:395-397` **[V]**). `CreateFleet` alone mints an *empty*
    // fleet, so inviting before `Init` is correctly refused.
    a.send(&call_req_args(
        "fleetObjectHandler",
        "CreateFleet",
        71,
        vec![],
    ));
    // Tolerant of a push in front: Bob selecting a character posted
    // `OnCharNowInStation` to Alice, and the ticker decides when it lands.
    let (_reference, _) = call_rsp_after_pushes(&mut a);
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Init",
        72,
        vec![PyValue::Int(670)],
    ));
    // `Init` seats the creator, and a seat is a session change -- which comes
    // before the answer. `forming_a_fleet_puts_the_fleet_id_on_the_bosss_session`
    // is where that is the subject rather than noise.
    let _ = expect_session_change(&a.recv());
    let _ = expect_call_rsp(&a.recv());

    // ...and invites Bob.
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Invite",
        73,
        vec![PyValue::Int(BOB)],
    ));
    let _ = expect_call_rsp(&a.recv());

    // **The assertion.** Bob said nothing and is owed a frame. It arrives
    // through `Wire::post` and the ticker rather than in a reply, because
    // nothing may write to a socket while the world lock is held, so give it
    // more than one `push::TICK` (250ms).
    b.stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let (event, args) = expect_notification(&b.recv());
    assert_eq!(
        event, "OnFleetInvite",
        "Bob must be told he was invited; got {event} {args:?}"
    );

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// Pull `(sid, change)` out of a `SessionChangeNotification`.
///
/// The body is `(sid, (0, change), nodesOfInterest)`. CCP's own sender builds
/// the same 2-tuple -- `SessionChangeGPCS.SessionChanged` passes
/// `change=(0, change)` **[V]** -- so the leading `0` is part of the contract
/// and not padding of ours.
fn expect_session_change(v: &PyValue) -> (i64, PyValue) {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionChangeNotification",
        "expected a session change"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple: {:?}", t[4])
    };
    let PyValue::Int(sid) = body[0] else {
        panic!("a session change must name a sid, got {:?}", body[0])
    };
    let PyValue::Tuple(wrapped) = &body[1] else {
        panic!(
            "the change must be the (0, change) 2-tuple, got {:?}",
            body[1]
        )
    };
    assert_eq!(wrapped.len(), 2, "(0, change)");
    assert_eq!(wrapped[0], PyValue::Int(0), "the leading 0 is theirs");
    (sid, wrapped[1].clone())
}

/// The new value out of one `{key: (old, new)}` entry.
fn change_of<'a>(change: &'a PyValue, key: &str) -> &'a PyValue {
    let entry = dict_get(change, key).unwrap_or_else(|| panic!("{key} missing from {change:?}"));
    let PyValue::Tuple(pair) = entry else {
        panic!("{key} must be an (old, new) pair, got {entry:?}")
    };
    assert_eq!(pair.len(), 2, "{key} is a 2-tuple");
    &pair[1]
}

/// The boss's own session learns the fleet it is the boss of.
///
/// `Init` and not `CreateFleet`: the latter mints an **empty** fleet and
/// seats nobody, which is the client's own two-step at `fleetSvc.py:395-397`.
/// It is also the arm that used to drop its `Effects` on the floor, so this
/// test is the one that would have caught that.
#[test]
fn forming_a_fleet_puts_the_fleet_id_on_the_bosss_session() {
    let addr = start_two_players();
    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    a.send(&call_req_args(
        "fleetObjectHandler",
        "CreateFleet",
        71,
        vec![],
    ));
    // **The answer is the reference alone**, not `(reference, retval)` --
    // that shape belongs to a bind. So the fleet id does not reach the client
    // here at all, which is exactly why the session change below is the thing
    // the client has to learn it from (`fleetSvc.py:399` then calls
    // `GetFleetID` on the object, and this test cross-checks the two).
    let _reference = expect_call_rsp(&a.recv());

    a.send(&call_req_args(
        "fleetObjectHandler",
        "Init",
        72,
        vec![PyValue::Int(670)],
    ));

    // **Before the answer**, for the same reason `SelectCharacterID`'s change
    // is: the two are handled by different tasklets on the client.
    let (sid, change) = expect_session_change(&a.recv());
    let PyValue::Int(fleet_id) = *change_of(&change, "fleetid") else {
        panic!("the boss's session must name the fleet it just formed")
    };
    assert!(
        matches!(change_of(&change, "fleetrole"), PyValue::Int(_)),
        "a seated member has a role"
    );
    // **`None`, not `-1`.** The member is in no wing, and every consumer of
    // these two keys tests truthiness -- `-1` is truthy.
    assert_eq!(*change_of(&change, "wingid"), PyValue::None);
    assert_eq!(*change_of(&change, "squadid"), PyValue::None);
    let _ = expect_call_rsp(&a.recv());

    // And it is the same fleet the object answers for.
    a.send(&call_req_args(
        "fleetObjectHandler",
        "GetFleetID",
        73,
        vec![],
    ));
    assert_eq!(
        expect_call_rsp(&a.recv()),
        PyValue::Int(fleet_id),
        "the session and the object must agree on which fleet this is"
    );

    // The sid is this connection's own, which is trivially true here and is
    // the thing the next test checks when it is not.
    assert!(sid > 0, "a session change must name a real sid");

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// A kick changes a session belonging to somebody who called nothing.
///
/// The cross-connection half of the fleet session keys, and the only one that
/// can get the **sid** wrong: Bob's frame is built from the session table's
/// record of Bob, not from the caller's session, so the two frames Bob
/// receives here -- one from his own `AcceptInvite`, one posted by Alice's
/// `KickMember` -- must agree on it. Reusing Alice's sid would send Bob a
/// change naming Alice's session, on Bob's socket.
#[test]
fn a_kick_reaches_the_kicked_players_session_and_names_that_session() {
    let addr = start_two_players();

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);

    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    b.stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    guest_arrival(&mut a, BOB);

    // Alice forms a fleet and invites Bob.
    a.send(&call_req_args(
        "fleetObjectHandler",
        "CreateFleet",
        71,
        vec![],
    ));
    // Tolerant of a push in front: Bob selecting a character posted
    // `OnCharNowInStation` to Alice, and the ticker decides when it lands.
    let (_reference, _) = call_rsp_after_pushes(&mut a);
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Init",
        72,
        vec![PyValue::Int(670)],
    ));
    // Alice's own seat, and where the fleet id comes from: `CreateFleet`
    // answers a reference and nothing else.
    let (_, seated) = expect_session_change(&a.recv());
    let PyValue::Int(fleet_id) = *change_of(&seated, "fleetid") else {
        panic!("the boss's session must name the fleet")
    };
    let _ = expect_call_rsp(&a.recv());
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Invite",
        73,
        vec![PyValue::Int(BOB)],
    ));
    let _ = expect_call_rsp(&a.recv());

    // Bob is told, and accepts the way the client does: `Moniker(
    // 'fleetObjectHandler', fleetID)` is a bind, with the first call fused
    // on. A plain call could not work -- `fleet_id()` would have nothing to
    // resolve, because Bob is in no fleet yet, which is the whole point.
    let (event, _) = expect_notification(&b.recv());
    assert_eq!(event, "OnFleetInvite");
    b.send(&call_req_args(
        "fleetObjectHandler",
        "MachoBindObject",
        81,
        vec![
            PyValue::Int(fleet_id),
            PyValue::Tuple(vec![
                bstr("AcceptInvite"),
                PyValue::Tuple(vec![PyValue::Int(670)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (bob_sid, joined) = expect_session_change(&b.recv());
    assert_eq!(
        *change_of(&joined, "fleetid"),
        PyValue::Int(fleet_id),
        "the joiner's own session must name the fleet"
    );
    let _ = expect_call_rsp(&b.recv());

    // Alice kicks him. Bob calls nothing from here on.
    a.send(&call_req_args(
        "fleetObjectHandler",
        "KickMember",
        74,
        vec![PyValue::Int(BOB)],
    ));
    let _ = expect_call_rsp(&a.recv());

    // **The assertion.** Bob is owed a session change he did not ask for. It
    // arrives through `Wire::post` and the ticker, like any other frame for
    // somebody who is not the caller, so it may be preceded by the fleet
    // notifications the same kick produced -- the order between the two is
    // not what this test is about.
    let mut found = None;
    for _ in 0..4 {
        let frame = b.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("expected a packet, got {frame:?}")
        };
        if String::from_utf8_lossy(class).ends_with("SessionChangeNotification") {
            found = Some(expect_session_change(&frame));
            break;
        }
    }
    let (kicked_sid, cleared) = found.expect("the kicked player's session must be told");
    assert_eq!(
        *change_of(&cleared, "fleetid"),
        PyValue::None,
        "a kicked pilot is in no fleet"
    );
    assert_eq!(
        kicked_sid, bob_sid,
        "the change must name Bob's session, not the session of whoever kicked him"
    );

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
}

/// **The minimum viable undock, over a real socket.**
///
/// One ship, visible, stationary. Every assertion below is a rule the
/// *client* enforces, read out of `michelle.pyc` and written up in
/// `docs/lanes/round2-spec.md` §2 -- not a fact about our own code. In
/// particular the last one, which is the easiest to write wrongly: the
/// natural version ("a notification eventually arrives") passes whether or
/// not the push was early enough to be thrown away.
#[test]
fn an_undock_pushes_one_setstate_naming_the_ship_as_ego() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    // The real sequence: `ship.Undock` arrives fused onto a bind.
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        80,
        vec![
            PyValue::Tuple(vec![PyValue::Int(60_012_526), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Undock"),
                PyValue::Tuple(vec![]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (undock, before_undock) = call_rsp_after_pushes(&mut client);
    let PyValue::Tuple(pair) = &undock else {
        panic!("a bind answers (boundObject, retval), got {undock:?}")
    };
    assert_eq!(pair.len(), 2);
    assert!(
        !before_undock.iter().any(|e| e == "DoDestinyUpdate"),
        "nothing before the undock's own answer: {before_undock:?}"
    );

    // **Assertion 7, and the reason this test drives the whole sequence
    // rather than just the bind.** `michelle.DoDestinyUpdate` returns
    // silently while `__bp is None`, and the ballpark does not exist until
    // seconds after the undock -- so a state pushed with the undock reply is
    // discarded without a trace. `Registry::pending` is shared and drains on
    // the next turn of *any* connection, so if the undock had queued one it
    // would arrive ahead of this unrelated call's answer.
    client.send(&call_req_args("beyonce", "GetFormations", 81, vec![]));
    let (_formations, after_undock) = call_rsp_after_pushes(&mut client);
    assert!(
        !after_undock.iter().any(|e| e == "DoDestinyUpdate"),
        "the state must not be queued by the undock, where the client would \
         throw it away: {after_undock:?}"
    );

    // `beyonce.MachoResolveObject(solarSystemID)` then
    // `MachoBindObject(solarSystemID, None)`. By the time the bind arrives,
    // `AddBallpark` has run and `Park.history` is empty.
    client.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        82,
        vec![PyValue::Int(30_000_001)],
    ));
    let node = expect_call_rsp(&client.recv());
    assert!(
        matches!(node, PyValue::Int(_)),
        "MachoResolveObject answers a node id, got {node:?}"
    );
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        83,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let bound = expect_call_rsp(&client.recv());
    assert!(
        matches!(&bound, PyValue::Tuple(p) if p.len() == 2),
        "the bind answers (boundObject, retval), got {bound:?}"
    );

    // And now the state, in the same flush, after the answer.
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "DoDestinyUpdate");
    assert_eq!(
        args.len(),
        2,
        "a truthy third argument scatters OnMultiEvent instead"
    );

    // §2.1: a waiting entry at the head of `history` is never flushed by
    // `DoPreTick`, and only a later batch at the identical stamp clears it.
    assert_eq!(
        args[1],
        PyValue::Int(0),
        "waitForBubble must be false on the bootstrap"
    );

    // §2.2 and §2.4: `RealFlushState` honours `SetState` only as `state[0]`,
    // and more than one distinct stamp in a batch calls `OnFatalDesync`.
    let PyValue::List(actions) = &args[0] else {
        panic!("the batch is a sequence, got {:?}", args[0])
    };
    assert_eq!(
        actions.len(),
        1,
        "exactly one action, and it is the SetState"
    );
    let PyValue::Tuple(action) = &actions[0] else {
        panic!("an action is (stamp, (name, args))")
    };
    let PyValue::Int(stamp) = action[0] else {
        panic!("the stamp is an int")
    };
    let PyValue::Tuple(call) = &action[1] else {
        panic!("(name, args)")
    };
    assert_eq!(call[0], bstr("SetState"));
    let PyValue::Tuple(call_args) = &call[1] else {
        panic!("args is a tuple")
    };
    assert_eq!(call_args.len(), 1, "SetState takes one argument, the bag");

    // The bag. `Park.SetState` reads each of these by name.
    let PyValue::Instance { class, state: bag } = &call_args[0] else {
        panic!("the bag is a KeyVal instance, got {:?}", call_args[0])
    };
    assert_eq!(String::from_utf8_lossy(class), "utillib.KeyVal");
    let attr = |name: &[u8]| -> PyValue {
        let PyValue::Dict(entries) = &**bag else {
            panic!("a KeyVal's state is an attribute dict")
        };
        entries
            .iter()
            .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name))
            .map(|(v, _)| v.clone())
            .unwrap_or_else(|| panic!("bag.{} is missing", String::from_utf8_lossy(name)))
    };

    // The ego is the ship's **item** id, which is what `gameui` stored as
    // `wannaBeEgo` from `session.shipid`; a mismatch is a camera that never
    // attaches.
    let ego = attr(b"ego");
    assert_eq!(ego, PyValue::Int(FIXTURE_SHIP_ID));

    // `slims` must name a ball the blob created, or `SetState` raises
    // `RuntimeError('BallNotInPark')` and loses the whole state.
    let PyValue::List(slims) = attr(b"slims") else {
        panic!("slims is iterated")
    };
    assert_eq!(
        slims.len(),
        2,
        "the ship and the station -- the ballpark is the whole scene, and a \
         park with only a ship is a ship in an empty universe"
    );
    let PyValue::Instance {
        class: slim_class,
        state: slim,
    } = &slims[0]
    else {
        panic!("a slim item is an instance")
    };
    assert_eq!(String::from_utf8_lossy(slim_class), "foo.SlimItem");
    let PyValue::Dict(slim_attrs) = &**slim else {
        panic!("the slim item's state is an attribute dict")
    };
    let slim_attr = |name: &[u8]| -> PyValue {
        slim_attrs
            .iter()
            .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name))
            .map(|(v, _)| v.clone())
            .unwrap_or_else(|| panic!("slim.{} is missing", String::from_utf8_lossy(name)))
    };
    assert_eq!(slim_attr(b"itemID"), ego, "the slim item is the ego ball");
    assert_eq!(slim_attr(b"typeID"), PyValue::Int(670), "the capsule");
    assert_ne!(
        slim_attr(b"charID"),
        PyValue::None,
        "ProcessBallAdd primes the pilot's name only when charID is not None"
    );

    // The three `SetState` iterates before testing anything: an empty
    // container is fine, `None` raises.
    assert!(matches!(attr(b"droneState"), PyValue::List(_)));
    assert!(matches!(attr(b"damageState"), PyValue::Dict(_)));
    assert!(matches!(attr(b"effectStates"), PyValue::List(_)));

    // §2.3: the action's stamp and the blob's own clock must be the same
    // number, because the stamp decides when the client flushes the batch and
    // the header decides what the ballpark's clock becomes when it does.
    let PyValue::Bytes(blob) = attr(b"state") else {
        panic!("bag.state is a byte string")
    };
    let park = eve_server::destiny::Park::decode(&blob).expect("our own blob must decode");
    assert_eq!(
        i64::from(park.current_time),
        stamp,
        "the blob's header clock is the action's stamp"
    );
    assert_eq!(park.balls.len(), 2, "the ship and the station it left");
    let ego = park
        .balls
        .iter()
        .find(|b| b.ball_id == FIXTURE_SHIP_ID)
        .expect("the ego ball is in the park, or SetState raises BallNotInPark");
    assert_eq!(
        ego.mode,
        eve_server::destiny::Mode::Stop,
        "a ship that has just undocked is parked"
    );
    let station = park
        .balls
        .iter()
        .find(|b| b.ball_id != FIXTURE_SHIP_ID)
        .expect("the station ball");
    // Radial distance, not one axis: the real mouth of station 2502 is at
    // `(17, 10096, -12607)`, so an `x`-only check would read 17 m.
    let out = ego.position.iter().map(|a| a * a).sum::<f64>().sqrt();
    assert!(
        out > f64::from(station.radius),
        "the ship has to be outside the station model, not inside it: {out} m"
    );
    let physics = ego
        .physics
        .expect("an ego ball must be free, or it cannot be flown");
    // And it leaves at CCP's own undock speed rather than sitting still:
    // 1.75x the hull's top speed, capped at 700 m/s, which the STOP mode
    // then decelerates to rest.
    let speed = physics.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
    assert!(
        speed > 0.0,
        "a ship is shoved out of the mouth, not parked in it"
    );
    // The capsule's own numbers, off the wire: with no bundle, the invented
    // `destiny::hull::CAPSULE` row (CP58b), not the native placeholders.
    let pod = eve_server::destiny::hull::CAPSULE;
    assert_eq!(ego.mass, pod.mass, "the capsule's mass");
    assert_eq!(ego.radius, pod.radius, "the capsule's radius");
    assert_eq!(physics.max_velocity, pod.max_velocity);
    // The health the first real undock did not have. A ball with no entry in
    // `damageState` gets **no gauges** -- `GetDamageState` answers `None` and
    // `SetDamageState` hides the bar -- so an empty dict showed zero across
    // shield, armour and hull on a ship that had taken no damage.
    let PyValue::Dict(damage) = attr(b"damageState") else {
        panic!("damageState has .iteritems() called on it")
    };
    assert_eq!(damage.len(), 1, "the ship has health, the station has none");
    assert_eq!(damage[0].1, PyValue::Int(FIXTURE_SHIP_ID));
    let PyValue::Tuple(state) = &damage[0].0 else {
        panic!("a per-ball state is a sequence: {:?}", damage[0].0)
    };
    // **What the client will draw, not what we happened to pack.**
    // `CalculateCurrentDamageStateValues` takes `shieldValues[:2]` as
    // `(health, tau)`, so the health is the shield slot's **first** element
    // and `tau = 0` makes it pass through untouched. This read 17 m of the
    // wrong slot once already: we put the health at index 2, the client read
    // our zero at index 0, and the player got an empty red shield bar on an
    // undamaged ship. Asserting the slot's *length* -- which is what this
    // test used to do -- passed throughout that.
    let PyValue::Tuple(shield) = &state[0] else {
        panic!(
            "the shield slot must be a sequence or the gauge is HIDDEN, which \
             is what a bare float produces: {:?}",
            state[0]
        )
    };
    assert!(shield.len() >= 2, "`[:2]` needs two: {shield:?}");
    assert_eq!(
        shield[0],
        PyValue::Real(1.0),
        "the shield health is the FIRST element, and full"
    );
    assert_eq!(
        shield[1],
        PyValue::Real(0.0),
        "tau = 0 means no regeneration curve, so the health passes through"
    );
    assert_eq!(state[1], PyValue::Real(1.0), "armour, full");
    assert_eq!(state[2], PyValue::Real(1.0), "structure, full");

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// **A `CALL_RSP` from the client must not break the connection**, which is
/// the prerequisite for this server ever being the caller.
///
/// Nothing here has ever sent a `CALL_REQ`, so nothing has ever received the
/// answer to one. §41 needs that direction: `InitClientGlobalConfig` is a
/// plain method on the client's `machoNet`, not a notify event, so it has to
/// be a call -- and `machoNet.__clientallowedcommands__` includes
/// `MACHONETMSG_TYPE_CALL_REQ`, so the client is built to accept one `[V]`.
///
/// Before the arm this test guards, an inbound `CallRsp` fell through to
/// `unhandled command CallRsp` and was counted as unanswered. That would have
/// put an `UNANSWERED` line on **every login** in exchange for removing the
/// one §41 is about, which is a bad trade and exactly the self-inflicted log
/// noise `note_unanswered`'s own doc warns against.
///
/// **This test passes with the new arm removed, and that is worth stating
/// rather than discovering later.** It was written to guard the arm and it
/// does not: `note_unanswered` followed by `Reply::nothing()` was already
/// graceful, so the connection always survived an unsolicited `CallRsp`. What
/// the arm changes is the *log line*, and `unanswered` is a dispatcher field
/// with no accessor -- invisible from a socket.
///
/// So what this pins is the property the login path actually depends on: an
/// unsolicited `CallRsp` does not break the session, verified by the next
/// ordinary call still answering correctly. That was true before and must stay
/// true, which is a regression guard worth having and not the same claim as
/// "the arm works".
///
/// **The arm's own effect is only observable once this server sends a
/// `CALL_REQ`**, because until then no `CallRsp` ever arrives. Its verification
/// belongs to that step: a bot that receives our call and answers it exercises
/// this path end to end, and the server's end-of-connection
/// `unanswered=[...]` line is where the absence shows. Noted here so nobody
/// reads this test as the arm's coverage.
#[test]
fn a_call_response_from_the_client_is_accepted_and_the_connection_survives() {
    let (addr, _server) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    // The initial session state arrives unprompted after the handshake;
    // reading it first is what every other test here does, under one name or
    // another. Leaving it queued made this test's first `expect_call_rsp` see
    // a `SessionInitialStateNotification` and look like the CallRsp had
    // broken the session.
    let _ = read_session(&mut client);

    // The shape the client sends when it answers a call of ours: a CallRsp
    // addressed back at the node, carrying the call id we would have sent.
    let rsp = eve_proto::MachoPacket::new(
        eve_marshal::MachoNetMsg::CallRsp,
        eve_proto::MachoAddress::Client {
            client_id: 1,
            call_id: Some(4242),
            service: None,
        },
        eve_proto::MachoAddress::Node {
            node_id: Some(1),
            service: None,
            call_id: Some(4242),
        },
        vec![PyValue::None],
    );
    client.send(&rsp.to_value().expect("a CallRsp value"));

    // And the connection is still good for ordinary business. `GetTargets` is
    // the cheapest call that returns something checkable.
    client.send(&call_req("dogmaIM", "GetTargets", 9001));
    // `call_rsp_after_pushes` already unwraps the CallRsp, so this is the
    // answer itself and not the packet around it.
    let (answer, _pushes) = call_rsp_after_pushes(&mut client);
    assert_eq!(
        answer,
        PyValue::List(vec![]),
        "after an unsolicited CallRsp the session must still answer calls; a \
         broken connection here is the login failure this arm exists to avoid"
    );
}

// --- Lane shutdown: the clean stop ------------------------------------------

/// A scratch directory for one shutdown test.
fn shutdown_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("eve-shutdown-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

/// A GPS server keeping `world`, stopping on `stop_file`.
fn stoppable(world: &std::path::Path, stop_file: &std::path::Path) -> Server {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.world_file = Some(world.to_path_buf());
    config.stop_file = Some(stop_file.to_path_buf());
    Server::bind(config).expect("bind")
}

/// Everything the server sends until it closes, raw. Panics on a hang.
fn read_to_close(c: &mut MockClient) -> Vec<u8> {
    let mut all = std::mem::take(&mut c.buf);
    let mut chunk = [0u8; 4096];
    loop {
        match c.stream.read(&mut chunk) {
            Ok(0) => return all,
            Ok(n) => all.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return all,
            Err(e) => panic!("the server must close the session, not hang: {e}"),
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The fixture character's balance on a fresh connection to `addr`.
fn balance_after_restart(addr: SocketAddr) -> PyValue {
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    select_character(&mut c, FIXTURE_CHARACTER, 50);
    c.send(&call_req_args(
        "account",
        "GetCashBalance",
        51,
        vec![PyValue::Int(0)],
    ));
    let balance = expect_call_rsp(&c.recv());
    c.stream.shutdown(Shutdown::Both).expect("shutdown");
    balance
}

/// `/shutdown 1 <message>` tells the client with its own cluster shutdown
/// notice, then the countdown fires once: the server stops accepting, hangs
/// the session up, writes the world through the saver and frees the
/// `--world` lock -- so a second server binds the same file and finds the
/// ISK granted just before.
#[test]
fn a_shutdown_countdown_warns_hangs_up_and_leaves_the_world_on_disk() {
    let dir = shutdown_dir("countdown");
    let world = dir.join("world.txt");
    let server = stoppable(&world, &dir.join("world.txt.stop"));
    let addr = server.local_addr().expect("addr");
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        server.run().expect("run");
        server.hang_up(eve_server::shutdown::SESSION_DRAIN);
        let why = server.stop_reason();
        drop(server);
        let _ = done.send(why);
    });

    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    select_character(&mut c, FIXTURE_CHARACTER, 79);
    let slash = |c: &mut MockClient, id: i64, line: &str| {
        c.send(&call_req_args(
            "slash",
            "SlashCmd",
            id,
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        ));
        expect_call_rsp(&c.recv())
    };
    slash(&mut c, 80, "/giveisk 4200000");
    c.recv(); // OnAccountChange
    let answer = slash(&mut c, 81, "/shutdown 1 back in five");
    let PyValue::Bytes(answer) = answer else {
        panic!("a console answer is a string")
    };
    assert!(
        String::from_utf8_lossy(&answer).starts_with("shutdown in 1 s"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let rest = read_to_close(&mut c);
    assert!(contains(&rest, b"OnRemoteMessage"), "the GM's words go out");
    assert!(
        contains(&rest, b"OnClusterShutdownInitiated"),
        "the client's own shutdown notice goes out before the close"
    );
    let why = finished
        .recv_timeout(Duration::from_secs(20))
        .expect("the server stops by itself");
    assert!(
        why.is_some_and(|w| w.contains("/shutdown") && w.contains("back in five")),
        "stopped by the countdown"
    );

    // The lock is free and the ISK is in the file.
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.world_file = Some(world.clone());
    let again = Server::bind(config).expect("the --world lock was released");
    let addr = again.local_addr().expect("addr");
    let handle = std::thread::spawn(move || again.run_once().expect("serve"));
    assert_eq!(balance_after_restart(addr), PyValue::Real(4_200_000.0));
    handle.join().expect("join");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stop file takes the same path: the clock sees it, `run` returns, a
/// connection made after that is never served (no handshake byte), the live
/// session is hung up, and the dropped server writes the world and frees
/// the lock.
#[test]
fn the_stop_file_stops_the_server_and_no_new_login_is_served() {
    let dir = shutdown_dir("stopfile");
    let world = dir.join("world.txt");
    let stop_file = dir.join("world.txt.stop");
    std::fs::write(&stop_file, b"").expect("stale stop file");
    let server = std::sync::Arc::new(stoppable(&world, &stop_file));
    assert!(
        !stop_file.exists(),
        "a stale stop file is removed at startup"
    );
    let addr = server.local_addr().expect("addr");
    let (done, returned) = std::sync::mpsc::channel();
    let runner = std::sync::Arc::clone(&server);
    std::thread::spawn(move || {
        runner.run().expect("run");
        let _ = done.send(());
    });

    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    select_character(&mut c, FIXTURE_CHARACTER, 79);

    std::fs::write(&stop_file, b"").expect("touch");
    returned
        .recv_timeout(Duration::from_secs(10))
        .expect("the stop file ends run()");
    assert!(
        server
            .stop_reason()
            .is_some_and(|r| r.contains("stop file")),
        "stopped by the file"
    );

    // Stopped: a new connection gets nothing, not even the handshake.
    let accepted = server.accepted();
    let mut late = TcpStream::connect(addr).expect("the port is still bound");
    late.set_read_timeout(Some(Duration::from_millis(500)))
        .expect("timeout");
    let mut byte = [0u8; 1];
    assert!(
        !matches!(late.read(&mut byte), Ok(n) if n > 0),
        "no login is served after the stop"
    );
    assert_eq!(server.accepted(), accepted, "nothing new was accepted");

    assert_eq!(server.hang_up(eve_server::shutdown::SESSION_DRAIN), 0);
    read_to_close(&mut c);
    let server = std::sync::Arc::into_inner(server).expect("the run thread has ended");
    server.stop().clear_file();
    drop(server);
    assert!(!stop_file.exists());
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.world_file = Some(world);
    Server::bind(config).expect("the --world lock was released");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The binary exits 0 on its stop file** (what `tools/soak.sh` now
/// expects instead of SIGTERM's 143), and leaves its world file written.
#[test]
fn the_binary_exits_zero_on_its_stop_file_with_the_world_written() {
    let dir = shutdown_dir("binary");
    let world = dir.join("world.txt");
    let stop_file = eve_server::shutdown::default_stop_file(&world);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_eve-server"))
        .args(["--bind", "127.0.0.1:0", "--world"])
        .arg(&world)
        .arg("--status")
        .arg(dir.join("status.tsv"))
        .current_dir(&dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn eve-server");
    // Wait for it to listen, reading its stdout on a thread.
    let stdout = child.stdout.take().expect("stdout");
    let (lines, listening) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let _ = lines.send(line);
        }
    });
    let mut log = Vec::new();
    while !log.iter().any(|l: &String| l.contains("listening on")) {
        log.push(
            listening
                .recv_timeout(Duration::from_secs(30))
                .expect("eve-server says it listens"),
        );
    }
    std::fs::write(&stop_file, b"").expect("touch");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(s) = child.try_wait().expect("wait") {
            break s;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("eve-server did not stop on its stop file");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    while let Ok(line) = listening.recv_timeout(Duration::from_millis(200)) {
        log.push(line);
    }
    assert!(status.success(), "exit 0, not {status:?}: {log:#?}");
    assert!(
        log.iter().any(|l| l.contains("[shutdown] world written")),
        "{log:#?}"
    );
    assert!(world.exists(), "the world file is on disk");
    assert!(!stop_file.exists(), "the stop file is consumed");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`fighterMgr` over a real socket** (FT-B/FT-C): the shapes the client
/// unpacks, through the marshal both ways.
///
/// `GetFightersForShip` is a 3-tuple whose third element is a dict
/// (`shipFighterState._UpdateStateForShip`). `LaunchFightersFromTubes` and
/// `RecallFightersToTubes` answer `{key: None | UserError}` and the client
/// calls `error.args` on each value (`fightersSvc.py:104-134`), so a
/// refusal has to be a `UserError` **object inside a dict**, which is the
/// `[A]` of `fighters::Refusal::to_value`: this shows it encodes and decodes
/// as `eveexceptions.UserError(key, dict)`. The fixture character is docked
/// in a capsule, so every launch is refused; load, launch and recall with
/// real squadrons are in `fighters::tests` over `Registry::dispatch`, since
/// the socket harness has no carrier hull to board.
#[test]
fn fighter_mgr_answers_in_the_shapes_the_client_unpacks() {
    let (addr, handle) = start(Mode::Gps, None);
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req("fighterMgr", "GetFightersForShip", 80));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    let PyValue::Tuple(parts) = &answer else {
        panic!("a 3-tuple, got {answer:?}")
    };
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], PyValue::List(vec![]));
    assert_eq!(parts[1], PyValue::List(vec![]));
    assert_eq!(parts[2], PyValue::Dict(vec![]));

    for (call_id, method, key) in [
        (81, "LaunchFightersFromTubes", 159),
        (82, "RecallFightersToTubes", 1_000_000),
    ] {
        client.send(&call_req_args(
            "fighterMgr",
            method,
            call_id,
            vec![PyValue::List(vec![PyValue::Int(key)])],
        ));
        let (answer, _) = call_rsp_after_pushes(&mut client);
        let PyValue::Dict(pairs) = &answer else {
            panic!("{method}: a dict keyed by tube or fighter, got {answer:?}")
        };
        assert_eq!(pairs.len(), 1, "{method}");
        // Wire order: value, then key.
        assert_eq!(pairs[0].1, PyValue::Int(key), "{method}");
        let PyValue::ObjectEx { header, .. } = &pairs[0].0 else {
            panic!(
                "{method}: the refusal is a UserError object: {:?}",
                pairs[0].0
            )
        };
        let PyValue::Tuple(h) = &**header else {
            panic!("{method}: header")
        };
        assert_eq!(
            h[0],
            PyValue::Global(b"eveexceptions.UserError".to_vec()),
            "{method}"
        );
    }

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

// ---------------------------------------------------------------------------
// IMPL: implants over the socket
// ---------------------------------------------------------------------------

/// Invented ids (none of CCP's): a slot-1 memory implant worth +3, and a
/// second slot-1 implant. The rows are in the shape of the client's own: the
/// type's `implantness` (331) and `memoryBonus` (177), and a passive effect
/// whose modifier is a `charID` `ItemModifier` adding 177 to `memory` (166).
const SOCKET_MEMORY_IMPLANT: i64 = 990_481_011;
const SOCKET_OTHER_IMPLANT: i64 = 990_481_012;
const SOCKET_MEMORY_EFFECT: i64 = 990_481_901;

fn implant_content() -> std::sync::Arc<eve_server::content::Content> {
    let mut rows = String::new();
    for (id, bonus, name) in [
        (SOCKET_MEMORY_IMPLANT, 3.0, "Socket Memory Implant"),
        (SOCKET_OTHER_IMPLANT, 1.0, "Socket Other Implant"),
    ] {
        rows += &format!(
            "type\t{id}\t300\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t{SOCKET_MEMORY_EFFECT}\t\
             331=1.0;177={bonus}\t{name}\n"
        );
    }
    rows += &format!("effect\t{SOCKET_MEMORY_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tsocketMemory\t\n");
    rows += &format!("modifier\t{SOCKET_MEMORY_EFFECT}\tcharID\tItemModifier\t166\t177\t2\t\t\n");
    let table =
        eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
            .expect("the invented rows parse");
    let mut content = eve_server::content::Content::fixture();
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    std::sync::Arc::new(content)
}

const SOCKET_BOOSTER: i64 = 990_491_011;
const SOCKET_BOOSTER_EFFECT: i64 = 990_491_901;

/// A group-303 booster: slot 1 (`boosterness` 1087), ten minutes (`boosterDuration`
/// 330), +3 memory, and the effect rows with the 13th column (no chance), as
/// a regenerated bundle writes them.
fn booster_content() -> std::sync::Arc<eve_server::content::Content> {
    let mut rows = format!(
        "type\t{SOCKET_BOOSTER}\t303\t20\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t{SOCKET_BOOSTER_EFFECT}\t\
         1087=1.0;330=600000.0;177=3.0\tSocket Booster\n"
    );
    rows +=
        &format!("effect\t{SOCKET_BOOSTER_EFFECT}\t0\t0\t0\t0\t0\t0\t0\tsocketBooster\t\t1\t0\n");
    rows += &format!("modifier\t{SOCKET_BOOSTER_EFFECT}\tcharID\tItemModifier\t166\t177\t2\t\t\n");
    let table =
        eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
            .expect("the invented rows parse");
    let mut content = eve_server::content::Content::fixture();
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    std::sync::Arc::new(content)
}

/// **The booster slice, over a real socket**: grant two, take one through the
/// call the client's menu makes (`UseBooster`), read `GetBoosters` (a dict
/// keyed by type with a future `expiryTime`) and `GetAttributes` (memory
/// 20 -> 23), and be refused the second for the slot.
#[test]
fn a_booster_is_taken_over_the_socket_and_its_bonus_shows() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.content = Some(booster_content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || server.run_once().expect("serve"));
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        80,
        vec![PyValue::Bytes(b"/createitem Socket Booster 2 ".to_vec())],
    ));
    expect_call_rsp(&client.recv());
    let (event, args) = expect_notification(&client.recv());
    assert_eq!(event, "OnItemsChanged");
    let args = one_row(args);
    let PyValue::DbRow(row) = &args[0] else {
        panic!("a packed item row, got {:?}", args[0])
    };
    let eve_marshal::DbValue::Int(item_id) = row.values()[0] else {
        panic!("column 0 is itemID")
    };
    let (event, _) = expect_notification(&client.recv());
    assert_eq!(event, "OnInvalidateCacheForLocations");

    client.send(&call_req_args("skillMgr2", "GetMySkillHandler", 82, vec![]));
    client.recv();
    let boosters = |client: &mut MockClient, call_id: i64| -> Vec<(PyValue, PyValue)> {
        client.send(&call_req_args(
            "skillMgr2::skillHandler",
            "GetBoosters",
            call_id,
            vec![],
        ));
        let PyValue::Dict(d) = call_rsp_after_pushes(client).0 else {
            panic!("GetBoosters is a dict keyed by type")
        };
        d
    };
    let memory = |client: &mut MockClient, call_id: i64| -> f64 {
        client.send(&call_req_args(
            "skillMgr2::skillHandler",
            "GetAttributes",
            call_id,
            vec![],
        ));
        let PyValue::Dict(d) = call_rsp_after_pushes(client).0 else {
            panic!("GetAttributes is a dict")
        };
        let PyValue::Real(memory) = d
            .iter()
            .find(|(_, k)| *k == PyValue::Int(166))
            .map(|(v, _)| v.clone())
            .expect("memory is in it")
        else {
            panic!("a real")
        };
        memory
    };
    assert_eq!(memory(&mut client, 83), 20.0, "no booster: the start");
    assert!(boosters(&mut client, 84).is_empty());

    client.send(&call_req_args(
        "dogmaIM",
        "UseBooster",
        85,
        vec![PyValue::Int(item_id), PyValue::Int(1)],
    ));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(answer, PyValue::None);
    let mut events = Vec::new();
    while !events.contains(&"OnServerBoostersChanged".to_owned()) && events.len() < 8 {
        events.push(expect_notification(&client.recv()).0);
    }
    let held = boosters(&mut client, 86);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].1, PyValue::Int(SOCKET_BOOSTER), "keyed by type");
    let PyValue::Instance { state, .. } = &held[0].0 else {
        panic!("a KeyVal")
    };
    let get = |name: &str| {
        let PyValue::Dict(d) = &**state else {
            panic!("a KeyVal's state is a dict")
        };
        d.iter()
            .find(|(_, k)| *k == PyValue::Bytes(name.as_bytes().to_vec()))
            .map(|(v, _)| v.clone())
    };
    assert_eq!(get("boosterSlot"), Some(PyValue::Int(1)));
    assert_eq!(get("boosterDuration"), Some(PyValue::Int(600_000)));
    assert_eq!(get("sideEffectIDs"), Some(PyValue::List(vec![])));
    assert!(matches!(get("expiryTime"), Some(PyValue::Int(t)) if t > 0));
    assert_eq!(memory(&mut client, 87), 23.0, "memory 20 + the booster's 3");

    client.send(&call_req_args(
        "dogmaIM",
        "UseBooster",
        88,
        vec![PyValue::Int(item_id), PyValue::Int(1)],
    ));
    let (key, args) = error_response_key(&client.recv());
    assert_eq!(key, "OnlyOneBoosterActive");
    assert_eq!(args.len(), 2, "typeName and typeName2: {args:?}");

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}

/// The `(key, dict)` of the `UserError` an `ErrorResponse` carries.
fn error_response_key(frame: &PyValue) -> (String, Vec<(PyValue, PyValue)>) {
    let PyValue::Instance { class, state } = frame else {
        panic!("not an instance: {frame:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.ErrorResponse",
        "a refusal is an ErrorResponse, not a CallRsp"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(payload) = &body[2] else {
        panic!("payload must be a tuple")
    };
    let PyValue::Bytes(stream) = &payload[0] else {
        panic!("a marshalled exception")
    };
    let PyValue::ObjectEx { header, .. } = loads(stream).expect("a complete stream") else {
        panic!("an ObjectEx")
    };
    let PyValue::Tuple(parts) = *header else {
        panic!("(callable, args)")
    };
    let PyValue::Tuple(args) = &parts[1] else {
        panic!("args")
    };
    let PyValue::Bytes(key) = &args[0] else {
        panic!("the message key")
    };
    let PyValue::Dict(d) = &args[1] else {
        panic!("the dict")
    };
    (String::from_utf8_lossy(key).into_owned(), d.clone())
}

/// **The implant slice, over a real socket**: grant an implant, plug it in
/// through the call the client's menu makes, and read what the client reads --
/// `GetImplants` (keyed by slot), `GetAttributes` (memory 20 -> 23: the
/// answer carries the implant, because the attribute window subtracts it) --
/// then be refused a second one for the slot, and unplug it.
///
/// **Not here**: a pod death and its killmail. The harness has no combat over
/// the socket; that half is `combat::tests::plugged_implants_die_with_the_pod_...`
/// at registry level.
#[test]
fn an_attribute_implant_is_plugged_over_the_socket_and_its_bonus_shows() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.content = Some(implant_content());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || server.run_once().expect("serve"));
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, FIXTURE_CHARACTER, 79);

    // Two implants in the hangar; each arrives as an `OnItemChange` whose row
    // starts with the new item's id.
    let grant = |client: &mut MockClient, name: &str, call_id: i64| -> i64 {
        client.send(&call_req_args(
            "slash",
            "SlashCmd",
            call_id,
            vec![PyValue::Bytes(
                format!("/createitem {name} 1 ").into_bytes(),
            )],
        ));
        expect_call_rsp(&client.recv());
        let (event, args) = expect_notification(&client.recv());
        assert_eq!(event, "OnItemsChanged");
        let args = one_row(args);
        let PyValue::DbRow(row) = &args[0] else {
            panic!("a packed item row, got {:?}", args[0])
        };
        let eve_marshal::DbValue::Int(item_id) = row.values()[0] else {
            panic!("column 0 is itemID")
        };
        let (event, _) = expect_notification(&client.recv());
        assert_eq!(event, "OnInvalidateCacheForLocations");
        item_id
    };
    let first = grant(&mut client, "Socket Memory Implant", 80);
    let second = grant(&mut client, "Socket Other Implant", 81);

    client.send(&call_req_args("skillMgr2", "GetMySkillHandler", 82, vec![]));
    client.recv();
    let attributes = |client: &mut MockClient, call_id: i64| -> f64 {
        client.send(&call_req_args(
            "skillMgr2::skillHandler",
            "GetAttributes",
            call_id,
            vec![],
        ));
        let PyValue::Dict(d) = call_rsp_after_pushes(client).0 else {
            panic!("GetAttributes is a dict")
        };
        // `(value, key)`: memory is attribute 166.
        let PyValue::Real(memory) = d
            .iter()
            .find(|(_, k)| *k == PyValue::Int(166))
            .map(|(v, _)| v.clone())
            .expect("memory is in it")
        else {
            panic!("a real")
        };
        memory
    };
    let implants = |client: &mut MockClient, call_id: i64| -> Vec<(PyValue, PyValue)> {
        client.send(&call_req_args(
            "skillMgr2::skillHandler",
            "GetImplants",
            call_id,
            vec![],
        ));
        let PyValue::Dict(d) = call_rsp_after_pushes(client).0 else {
            panic!("GetImplants is a dict keyed by slot")
        };
        d
    };
    assert_eq!(attributes(&mut client, 83), 20.0, "no implant: the start");
    assert!(implants(&mut client, 84).is_empty());

    // Plug the first in: the call the menu makes, then the push that makes
    // the client re-read.
    client.send(&call_req_args(
        "dogmaIM",
        "InjectImplant",
        85,
        vec![PyValue::Int(first)],
    ));
    // The pushes follow the answer, in one flush (`a_zero_delay_push_is_sent_
    // after_the_payloads`): read until the one that matters, bounded.
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(answer, PyValue::None);
    let mut events = Vec::new();
    while !events.contains(&"OnServerImplantsChanged".to_owned()) && events.len() < 8 {
        events.push(expect_notification(&client.recv()).0);
    }
    assert!(events.contains(&"OnItemsChanged".to_owned()), "{events:?}");
    assert!(
        events.contains(&"OnServerImplantsChanged".to_owned()),
        "{events:?}"
    );
    let head = implants(&mut client, 86);
    assert_eq!(head.len(), 1);
    assert_eq!(
        head[0].1,
        PyValue::Int(1),
        "keyed by the type's implantness"
    );
    let PyValue::Instance { state, .. } = &head[0].0 else {
        panic!("a KeyVal")
    };
    let get = |name: &str| {
        let PyValue::Dict(d) = &**state else {
            panic!("a KeyVal's state is a dict")
        };
        d.iter()
            .find(|(_, k)| *k == PyValue::Bytes(name.as_bytes().to_vec()))
            .map(|(v, _)| v.clone())
    };
    assert_eq!(get("typeID"), Some(PyValue::Int(SOCKET_MEMORY_IMPLANT)));
    assert_eq!(get("itemID"), Some(PyValue::Int(first)));
    assert_eq!(
        attributes(&mut client, 87),
        23.0,
        "memory 20 + the implant's 3"
    );

    // One per slot: the second is refused by name, and nothing changes.
    client.send(&call_req_args(
        "dogmaIM",
        "InjectImplant",
        88,
        vec![PyValue::Int(second)],
    ));
    let (key, args) = error_response_key(&client.recv());
    assert_eq!(key, "OnlyOneImplantActive");
    assert_eq!(args.len(), 1, "typeName and nothing else: {args:?}");
    assert_eq!(attributes(&mut client, 89), 23.0);

    // Unplug it: the bonus goes with it.
    client.send(&call_req_args(
        "dogmaIM",
        "DestroyImplant",
        90,
        vec![PyValue::Int(first)],
    ));
    let (answer, _) = call_rsp_after_pushes(&mut client);
    assert_eq!(answer, PyValue::None);
    assert!(implants(&mut client, 91).is_empty());
    assert_eq!(attributes(&mut client, 92), 20.0);

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
}
