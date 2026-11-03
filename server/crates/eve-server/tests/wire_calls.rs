//! Dispatch guards over real loopback connections (`docs/review/V2-wire.md`
//! F5, F6): every parsed `CallReq` is answered, and a peer that never logs
//! in is dropped on the short pre-auth leash.
//!
//! The pre-auth timeout here is injected through
//! `GpsSession::with_preauth_timeout`, never the `PREAUTH_TIMEOUT` default,
//! which is an assumption.

use eve_marshal::{MachoNetMsg, PyValue};
use eve_proto::{CallPayload, MachoAddress, MachoPacket, try_split_frame, write_frame};
use eve_server::dispatch::GpsSession;
use eve_server::handshake::Handshake;
use eve_server::model::SandboxConfig;
use eve_server::server::{Config, DispatchFactory, Mode, Server};
use eve_server::service::Registry;
use eve_server::session::Session;
use eve_server::{ConnEnd, Dispatch, dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const CHALLENGE_LEN: usize = 64;

/// `cluster.MACHONETERR_WRAPPEDEXCEPTION` [V] (`dispatch.rs`).
const WRAPPED_EXCEPTION: i64 = 2;

struct MockClient {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl MockClient {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect");
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

    /// The next frame, skipping a call the server makes to the client's
    /// `machoNet` (the one call this server makes; see `loopback.rs`).
    fn recv(&mut self) -> PyValue {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((frame, consumed)) = try_split_frame(&self.buf).expect("split") {
                let payload = frame.payload.to_vec();
                self.buf.drain(..consumed);
                let v = loads(&payload).expect("payload parses");
                if let Ok(p) = MachoPacket::from_value(&v)
                    && p.command == MachoNetMsg::CallReq
                {
                    continue;
                }
                return v;
            }
            let n = self.stream.read(&mut chunk).expect("read");
            assert_ne!(n, 0, "server closed while a payload was expected");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
    // Wire order is value-then-key.
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
}

fn h8_challenge(h8: &PyValue) -> Vec<u8> {
    let PyValue::Tuple(t) = h8 else {
        panic!("H8 is not a tuple")
    };
    match &t[0] {
        PyValue::Bytes(b) => b.clone(),
        other => panic!("H8[0] should be the server challenge, got {other:?}"),
    }
}

/// The client side of H1..H10 and the session that follows; the checks on
/// each step live in `loopback.rs`.
fn log_in(client: &mut MockClient) {
    client.recv(); // H1
    client.send(&PyValue::Tuple(vec![
        PyValue::Int(i64::from(eve_marshal::build::reference().line.secret)),
        PyValue::Int(i64::from(eve_marshal::build::reference().line.macho)),
        PyValue::Int(0),
        PyValue::Real(eve_marshal::build::reference().line.version),
        PyValue::Int(i64::from(eve_marshal::build::REFERENCE)),
        bstr(eve_marshal::build::reference().line.codename_at_region),
    ]));
    client.send(&PyValue::Tuple(vec![
        PyValue::None,
        bstr("VK"),
        bstr("name-hash"),
    ]));
    client.send(&PyValue::Tuple(vec![
        bstr("placebo"),
        PyValue::Dict(vec![]),
    ]));
    assert_eq!(client.recv(), bstr("OK CC"));
    client.send(&PyValue::Tuple(vec![
        PyValue::Bytes(vec![0x5A; CHALLENGE_LEN]),
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
        ]),
    ]));
    assert_eq!(client.recv(), PyValue::Int(2), "H7");
    let challenge = h8_challenge(&client.recv());
    client.send(&PyValue::Tuple(vec![
        PyValue::Bytes(eve_server::crypto_hash(&[PyValue::Bytes(challenge)]).into_bytes()),
        bstr(""),
        PyValue::Dict(vec![]),
    ]));
    client.recv(); // H10
    client.recv(); // the session's initial state
}

/// A `CallReq` from the client with `call_id`, carrying `body` verbatim.
fn call_req_with_body(call_id: i64, body: Vec<PyValue>) -> PyValue {
    MachoPacket::new(
        MachoNetMsg::CallReq,
        MachoAddress::Client {
            client_id: 1,
            call_id: Some(call_id),
            service: None,
        },
        MachoAddress::Any {
            service: Some("machoNet".to_owned()),
            call_id: Some(call_id),
        },
        body,
    )
    .to_value()
    .expect("a CallReq value")
}

/// The reply must be a code-2 `ErrorResponse` for a `CallReq`, addressed to
/// the caller's call id.
fn assert_error_response(v: &PyValue, call_id: i64, what: &str) {
    let p = MachoPacket::from_value(v).expect("a packet");
    assert_eq!(p.command, MachoNetMsg::ErrorResponse, "{what}: {v:?}");
    assert_eq!(p.destination.call_id(), Some(call_id), "{what}: call id");
    assert_eq!(
        p.body.first(),
        Some(&PyValue::Int(i64::from(MachoNetMsg::CallReq as u8))),
        "{what}: originalCommand"
    );
    assert_eq!(
        p.body.get(1),
        Some(&PyValue::Int(WRAPPED_EXCEPTION)),
        "{what}: code"
    );
}

#[test]
fn a_garbage_callreq_gets_an_error_response_with_its_call_id() {
    let server = Server::bind(Config::new("127.0.0.1:0".parse().expect("literal"))).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || server.run_once().expect("serve"));
    let mut client = MockClient::connect(addr);
    log_in(&mut client);

    // A header that parses around an inner pickle that does not.
    client.send(&call_req_with_body(
        41,
        vec![PyValue::Tuple(vec![
            PyValue::Int(0),
            PyValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        ])],
    ));
    assert_error_response(&client.recv(), 41, "garbage inner pickle");

    // No payload at all: this one used to end the connection.
    client.send(&call_req_with_body(42, vec![]));
    assert_error_response(&client.recv(), 42, "empty body");

    // The connection is still served.
    let get_time = CallPayload::Service {
        method: "GetTime".to_owned(),
        args: vec![],
        kwargs: vec![],
    };
    client.send(&call_req_with_body(43, vec![get_time.to_payload()]));
    let rsp = MachoPacket::from_value(&client.recv()).expect("a packet");
    assert_eq!(rsp.command, MachoNetMsg::CallRsp);
    assert_eq!(rsp.destination.call_id(), Some(43));

    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    assert!(matches!(handle.join().expect("join"), ConnEnd::PeerClosed));
}

#[test]
fn a_silent_peer_is_dropped_before_the_preauth_deadline() {
    let leash = Duration::from_millis(300);
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.test_dispatch = Some(DispatchFactory(Arc::new(move || {
        let world = Arc::new(RwLock::new(Registry::new(SandboxConfig::default())));
        Box::new(
            GpsSession::new(
                Handshake::new(Default::default(), [7u8; CHALLENGE_LEN]),
                Session::default(),
                world,
            )
            .with_preauth_timeout(leash),
        ) as Box<dyn Dispatch>
    })));
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || server.run_once().expect("serve"));

    let started = Instant::now();
    let mut client = MockClient::connect(addr);
    client.recv(); // H1, then nothing.
    let end = handle.join().expect("join");
    let took = started.elapsed();

    assert!(matches!(end, ConnEnd::Idle), "ended {end:?}");
    assert!(took >= leash, "dropped after {took:?}, before the leash");
    assert!(
        took < leash + Duration::from_secs(5),
        "dropped after {took:?}: the pre-auth leash did not apply"
    );
}
