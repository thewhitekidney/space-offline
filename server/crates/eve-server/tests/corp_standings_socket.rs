//! **A player corp's standings tab shows the members' average**, over a real
//! socket (CORP-S; lane note `docs/lanes/corp-s.md`).
//!
//! Alice and Bob are in an invented player corp, joined long ago; Alice has
//! one standing from an invented NPC owner and Bob another from the same
//! owner. The world is written by a first run, the corp and the standings
//! are appended to the file as the world file's own records, and a second
//! server reads it. Alice's client then asks what `Standing.Run` asks for a
//! player corp (`GetCorpStandings`, `GetStandingCompositions`) and gets the
//! mean (`UI/Standings/CompositionWindow/NPCToPlayerCorp` [V]). A pilot who
//! joined yesterday is not in it.
//!
//! Every id is invented.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

const CHALLENGE_LEN: usize = 64;

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

    /// The next frame, skipping the server's own `machoNet` calls.
    fn recv(&mut self) -> PyValue {
        loop {
            let frame = self.recv_raw();
            if let PyValue::Instance { class, .. } = &frame
                && String::from_utf8_lossy(class).rsplit('.').next() == Some("CallReq")
            {
                continue;
            }
            return frame;
        }
    }

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
}

fn bstr(s: &str) -> PyValue {
    PyValue::Bytes(s.as_bytes().to_vec())
}

fn dict(kv: Vec<(&str, PyValue)>) -> PyValue {
    PyValue::Dict(kv.into_iter().map(|(k, v)| (v, bstr(k))).collect())
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

fn request_dict() -> PyValue {
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

fn crypto_hash(args: &[PyValue]) -> PyValue {
    PyValue::Bytes(eve_server::crypto_hash(args).into_bytes())
}

/// The client side of H1..H10.
fn full_handshake(client: &mut MockClient) {
    let PyValue::Tuple(h1) = client.recv() else {
        panic!("H1 is not a tuple")
    };
    assert_eq!(h1.len(), 7);
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
    assert_eq!(client.recv(), bstr("OK CC"));
    let client_challenge = vec![0x5Au8; CHALLENGE_LEN];
    client.send(&PyValue::Tuple(vec![
        PyValue::Bytes(client_challenge),
        request_dict(),
    ]));
    assert_eq!(client.recv(), PyValue::Int(2));
    let PyValue::Tuple(h8) = client.recv() else {
        panic!("H8 is not a tuple")
    };
    let PyValue::Bytes(server_challenge) = &h8[0] else {
        panic!("H8[0] is the server challenge")
    };
    client.send(&PyValue::Tuple(vec![
        crypto_hash(&[PyValue::Bytes(server_challenge.clone())]),
        bstr(""),
        PyValue::Dict(vec![]),
    ]));
    let _h10 = client.recv();
}

/// The `SessionInitialStateNotification` after H10.
fn read_session(client: &mut MockClient) {
    let PyValue::Instance { class, .. } = client.recv() else {
        panic!("the session packet is an instance")
    };
    assert_eq!(
        String::from_utf8_lossy(&class),
        "carbon.common.script.net.machoNetPacket.SessionInitialStateNotification"
    );
}

fn call_req_args(service: &str, method: &str, call_id: i64, args: Vec<PyValue>) -> PyValue {
    let payload = eve_proto::CallPayload::Service {
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
        eve_proto::MachoAddress::Any {
            service: Some(service.to_owned()),
            call_id: Some(call_id),
        },
        vec![payload.to_payload()],
    )
    .to_value()
    .expect("a CallReq value")
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
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Bytes(pickle) = &body[0] else {
        panic!("a CallRsp payload is a marshalled string")
    };
    loads(pickle).expect("the response pickle loads")
}

fn expect_session_change(v: &PyValue) -> PyValue {
    let PyValue::Instance { class, state } = v else {
        panic!("not an instance: {v:?}")
    };
    assert_eq!(
        String::from_utf8_lossy(class),
        "carbon.common.script.net.machoNetPacket.SessionChangeNotification"
    );
    let PyValue::Tuple(t) = &**state else {
        panic!("state is not a tuple")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(wrapped) = &body[1] else {
        panic!("the change is (0, change)")
    };
    wrapped[1].clone()
}

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
    let _ = expect_session_change(&client.recv());
    let _ = expect_call_rsp(&client.recv());
}

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const CAROL: i64 = 90_000_003;
const CORP: i64 = 98_000_555;
const NPC_OWNER: i64 = 1_000_555;
/// FILETIME of the join: long ago for Alice and Bob, "yesterday" for Carol
/// is made from the real clock below.
const LONG_AGO: i64 = 130_000_000_000_000_000;
const ALICE_STANDING: f64 = 6.0;
const BOB_STANDING: f64 = 2.0;
const CAROL_STANDING: f64 = -10.0;

fn pilot(id: i64, name: &str) -> eve_server::model::Character {
    eve_server::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: id + 10_000_000,
        ..eve_server::model::Character::default()
    }
}

fn start(world: &std::path::Path) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice"),
        pilot(BOB, "Bob"),
        pilot(CAROL, "Carol"),
    ];
    config.world_file = Some(world.to_path_buf());
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let _ = server.run_once().expect("serve");
    });
    (addr, handle)
}

fn rows(v: &PyValue) -> Vec<(i64, f64)> {
    let items: Vec<&PyValue> = match v {
        PyValue::Dict(d) => d.iter().map(|(value, _)| value).collect(),
        PyValue::List(l) => l.iter().collect(),
        other => panic!("a dict or a list, got {other:?}"),
    };
    items
        .into_iter()
        .map(|r| {
            let PyValue::DbRow(row) = r else {
                panic!("a DBRow, got {r:?}")
            };
            let (eve_marshal::DbValue::Int(a), eve_marshal::DbValue::Real(b)) =
                (&row.values()[0], &row.values()[1])
            else {
                panic!("(id, standing)")
            };
            (*a, *b)
        })
        .collect()
}

#[test]
fn the_corp_standings_call_returns_the_members_mean() {
    let dir = std::env::temp_dir().join(format!("eve-corps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let world = dir.join("world.txt");

    // First run: any mutation makes the server write the world file.
    {
        let (addr, handle) = start(&world);
        let mut c = MockClient::connect(addr);
        full_handshake(&mut c);
        read_session(&mut c);
        select_character(&mut c, ALICE, 10);
        c.send(&call_req_args(
            "slash",
            "SlashCmd",
            11,
            vec![PyValue::Bytes(b"/giveisk 1 ".to_vec())],
        ));
        let _ = expect_call_rsp(&c.recv());
        c.stream.shutdown(Shutdown::Both).expect("shutdown");
        handle.join().expect("join");
    }
    assert!(world.exists(), "the world file was written");

    // The corp, three memberships and the standings, as records.
    let yesterday = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        ((now + 11_644_473_600) * 10_000_000) as i64 - 24 * 3_600 * 10_000_000
    };
    let mut text = std::fs::read_to_string(&world).expect("read");
    text.push_str(&format!(
        "corp\t{CORP}\t{ALICE}\t{ALICE}\t0\t0.1\t0.1\t1\t0\t-1\t-1\t-1\t-1\t-1\t-1\tInvented Corp\tINVC\t\t\n\
         corpmember\t{ALICE}\t{CORP}\t0\t{LONG_AGO}\n\
         corpmember\t{BOB}\t{CORP}\t0\t{LONG_AGO}\n\
         corpmember\t{CAROL}\t{CORP}\t0\t{yesterday}\n\
         corpnext\t{CORP}\t0\n\
         standing\t{ALICE}\t{NPC_OWNER}\t{ALICE_STANDING}\n\
         standing\t{BOB}\t{NPC_OWNER}\t{BOB_STANDING}\n\
         standing\t{CAROL}\t{NPC_OWNER}\t{CAROL_STANDING}\n"
    ));
    std::fs::write(&world, text).expect("write");

    // Second run: Alice asks as the client's Standing service does.
    let (addr, handle) = start(&world);
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    select_character(&mut c, ALICE, 20);

    c.send(&call_req_args(
        "standingMgr",
        "GetCorpStandings",
        21,
        vec![],
    ));
    let answer = expect_call_rsp(&c.recv());
    // Carol joined yesterday and is not counted: (6 + 2) / 2, not (6 + 2 - 10) / 3.
    assert_eq!(
        rows(&answer),
        vec![(NPC_OWNER, (ALICE_STANDING + BOB_STANDING) / 2.0)]
    );

    c.send(&call_req_args(
        "standingMgr",
        "GetStandingCompositions",
        22,
        vec![PyValue::Int(NPC_OWNER), PyValue::Int(CORP)],
    ));
    let answer = expect_call_rsp(&c.recv());
    assert_eq!(
        rows(&answer),
        vec![(ALICE, ALICE_STANDING), (BOB, BOB_STANDING)]
    );

    // Her own standings are untouched by the corp's.
    c.send(&call_req_args(
        "standingMgr",
        "GetCharStandings",
        23,
        vec![],
    ));
    let answer = expect_call_rsp(&c.recv());
    assert_eq!(rows(&answer), vec![(NPC_OWNER, ALICE_STANDING)]);

    c.stream.shutdown(Shutdown::Both).expect("shutdown");
    handle.join().expect("join");
    let _ = std::fs::remove_dir_all(&dir);
}
