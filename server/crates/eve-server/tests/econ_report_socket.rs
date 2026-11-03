//! **/econ over a real socket** (X-ECONREPORT; lane note `docs/lanes/x-econreport.md`):
//! a GM grants ISK and asks `/econ`, `/econ <region>`, `/econ report` and
//! `/econ tsv`, and the replies carry the grant as a source in the right
//! region, the career and tier tables and the TSV header.
//!
//! Every id is invented.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
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
const ALICE_SHIP: i64 = 100_000_001;
const BOT: i64 = 90_000_002;
const BOT_SHIP: i64 = 100_000_002;
const REGION: i64 = 10_000_901;
const BOT_REGION: i64 = 10_000_902;

fn slash(client: &mut MockClient, call_id: i64, line: &str) -> String {
    client.send(&call_req_args(
        "slash",
        "SlashCmd",
        call_id,
        vec![PyValue::Bytes(line.as_bytes().to_vec())],
    ));
    loop {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("a packet, got {frame:?}")
        };
        if String::from_utf8_lossy(class).ends_with("CallRsp") {
            return match expect_call_rsp(&frame) {
                PyValue::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                other => panic!("a slash reply is text, got {other:?}"),
            };
        }
    }
}

#[test]
fn econ_reports_isk_by_class_per_region_and_bot_rates() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![
        eve_server::model::Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: ALICE_SHIP,
            region_id: REGION,
            ..eve_server::model::Character::default()
        },
        eve_server::model::Character {
            character_id: BOT,
            name: "Botty".to_owned(),
            ship_id: BOT_SHIP,
            region_id: BOT_REGION,
            is_bot: true,
            ..eve_server::model::Character::default()
        },
    ];
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, ALICE, 70);

    slash(&mut client, 69, "/giveisk 1000 ");
    // The bot earned a bounty a moment ago (journal row: the system pays it).
    {
        let mut w = world.write().expect("the world");
        let now = eve_server::pyutil::filetime_now();
        let mut snap = w.to_snapshot();
        let mut row = snap.journal.last().expect("the grant's row").clone();
        row.transaction_id = snap.next_transaction_id;
        row.date = now;
        row.entry_type_id = 85;
        row.owner_id_1 = 1;
        row.owner_id_2 = BOT;
        row.amount = 4_000.0;
        row.balance = 4_000.0;
        snap.journal.push(row);
        snap.next_transaction_id += 1;
        w.apply_snapshot(snap);
    }
    let all = slash(&mut client, 71, "/econ report");
    assert!(
        all.starts_with("[econ] report total over the last 24 game hours"),
        "{all}"
    );
    assert!(all.contains("bounties 4000 (1)"), "{all}");
    assert!(
        all.contains("ISK/h by career: pve 1 bots 1h net 4000/h"),
        "{all}"
    );
    assert!(all.contains("ISK/h by ship tier: "), "{all}");

    let mine = slash(&mut client, 72, &format!("/econ {REGION}"));
    assert!(
        mine.starts_with(&format!("[econ] report region {REGION}")),
        "{mine}"
    );
    assert!(!mine.contains("bounties"), "{mine}");
    let theirs = slash(&mut client, 73, &format!("/econ report {BOT_REGION} 6"));
    assert!(theirs.contains("last 6 game hours"), "{theirs}");
    assert!(theirs.contains("bounties 4000 (1)"), "{theirs}");

    let tsv = slash(&mut client, 74, "/econ tsv");
    let mut lines = tsv.lines();
    assert_eq!(
        lines.next(),
        Some("day\tscope\tkind\tkey\tn\thours\tisk\tisk2\tper_h")
    );
    assert!(
        lines.any(|l| l.starts_with(&format!(
            "1\t{BOT_REGION}\tcareer\tpve\t1\t1\t4000\t4000\t4000"
        ))),
        "{tsv}"
    );

    let bad = slash(&mut client, 75, "/econ report banana");
    assert!(bad.starts_with("usage: /econ"), "{bad}");
}

/// **FE-MEASURE / FE-GUARD-A** (lane note `docs/lanes/fe-measure.md`): the
/// price tape, the audit and the tape rows of the TSV answer over the wire,
/// and the insurance quote follows a falling market index but never a
/// rising one past `P_ref`. Invented ids and prices.
#[test]
fn the_tape_the_audit_and_the_insurance_cap_answer_over_the_wire() {
    const HULL: i64 = 990_501;
    const HULL_REF: f64 = 1_000.0;
    let mut content = eve_server::content::Content::fixture();
    content.insert(
        "econ_prices",
        eve_server::econ_prices::Table::parse(&[format!(
            "{HULL}\t{HULL_REF}\trecipe\t500.0\t6\t99501\t1\t1"
        )])
        .expect("parses"),
    );
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.content = Some(std::sync::Arc::new(content));
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        region_id: REGION,
        ..eve_server::model::Character::default()
    }];
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let mut client = MockClient::connect(addr);
    full_handshake(&mut client);
    read_session(&mut client);
    select_character(&mut client, ALICE, 70);

    let quote = |client: &mut MockClient, call_id: i64| {
        client.send(&call_req_args(
            "insuranceSvc",
            "GetInsurancePrice",
            call_id,
            vec![PyValue::Int(HULL)],
        ));
        loop {
            let frame = client.recv();
            let PyValue::Instance { class, .. } = &frame else {
                panic!("a packet, got {frame:?}")
            };
            if String::from_utf8_lossy(class).ends_with("CallRsp") {
                return match expect_call_rsp(&frame) {
                    PyValue::Real(r) => r,
                    other => panic!("a price is a float, got {other:?}"),
                };
            }
        }
    };
    let put_index = |avg: f64| {
        let mut w = world.write().expect("the world");
        w.deltas_mut().put(
            eve_server::econ_prices::KIND,
            vec![HULL.to_string()],
            vec![avg.to_string(), "1".to_owned()],
        );
    };
    assert_eq!(quote(&mut client, 80), 0.8 * HULL_REF, "untraded: P_ref");
    put_index(0.7 * HULL_REF);
    assert!(
        (quote(&mut client, 81) - 0.8 * 0.7 * HULL_REF).abs() < 1e-6,
        "a falling market"
    );
    put_index(1.6 * HULL_REF);
    assert_eq!(quote(&mut client, 82), 0.8 * HULL_REF, "never past P_ref");

    // The index on its floor: the audit names it.
    put_index(eve_server::econ_prices::CLAMP.0 * HULL_REF);
    let audit = slash(&mut client, 83, "/econ audit");
    assert!(
        audit.starts_with("[econ] audit over the last 24 game hours: findings"),
        "{audit}"
    );
    assert!(
        audit.contains(&format!("pinned: type {HULL}'s index sits on its floor")),
        "{audit}"
    );
    let tape = slash(&mut client, 84, "/econ price-tape");
    assert!(
        tape.starts_with("[econ] price tape total over the last 24 game hours"),
        "{tape}"
    );
    assert!(tape.contains("level: closed-loop index"), "{tape}");
    let region = slash(&mut client, 85, &format!("/econ price-tape {REGION} 2"));
    assert!(
        region.contains(&format!("region {REGION} over the last 48 game hours")),
        "{region}"
    );
    let tsv = slash(&mut client, 86, "/econ tsv");
    assert!(
        tsv.lines().any(|l| l.contains("\taudit\tpinned\t1\t")),
        "{tsv}"
    );
    assert!(tsv.lines().any(|l| l.contains("\tlevel\tall\t")), "{tsv}");
    let bad = slash(&mut client, 87, "/econ price-tape banana");
    assert!(bad.starts_with("usage: /econ"), "{bad}");
    let bad = slash(&mut client, 88, "/econ audit -3");
    assert!(bad.starts_with("usage: /econ"), "{bad}");
}
