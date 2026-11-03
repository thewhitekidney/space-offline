//! **A pilot splits a stack in the station hangar**, over a real socket
//! (X-GAPS; lane note `docs/lanes/srv-gaps.md`).
//!
//! The client's "Split Stack" on a hangar row is
//! `invCacheSvc.GetInventoryMgr().SplitStack(stationID, itemID, qty,
//! ownerID)` (`invItemFunctions.py:410`): a moniker call on `invbroker`,
//! bound on `(stationID, groupStation)` with the call fused onto the bind,
//! answer dropped. It is followed by
//! `GetInventory(containerGlobal).InvalidateStationItemsCache(stationID)`
//! on `charMgr` (`:299`), which is also made here, and by the reload of the
//! hangar, whose listing must show both stacks.
//!
//! **Every id is invented** except the stack's type, which is a row of the
//! test fixture's tables. The harness helpers are copies of
//! `tests/fighter_socket.rs`', trimmed to what this test uses.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

const CHALLENGE_LEN: usize = 64;

const ALICE: i64 = 90_000_001;
const ALICE_SHIP: i64 = 100_000_001;
/// `flagHangar` and `groupStation` **[V]**.
const FLAG_HANGAR: i64 = 4;
const GROUP_STATION: i64 = 15;
/// A stackable type of the test fixture's tables (the wallet tests' one).
const TYPE_ID: i64 = 34;
/// The stack, and the part split off it.
const STACK: i64 = 1_000;
const PART: i64 = 300;

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

/// `(event, args)` of a `Notification`.
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
    let PyValue::Instance { state: dest, .. } = &t[2] else {
        panic!("destination is not an address instance")
    };
    let PyValue::Tuple(dest) = &**dest else {
        panic!("the address state is not a tuple")
    };
    let PyValue::Bytes(event) = &dest[1] else {
        panic!("broadcastID must be a byte string")
    };
    let PyValue::Tuple(body) = &t[4] else {
        panic!("body is not a tuple")
    };
    let PyValue::Tuple(outer) = &body[0] else {
        panic!("the payload is a tuple")
    };
    let PyValue::Bytes(pickle) = &outer[1] else {
        panic!("the inner payload must be a byte string")
    };
    let PyValue::Tuple(after_object) = loads(pickle).expect("the inner pickle") else {
        panic!("the pickle is a tuple")
    };
    let PyValue::Tuple(after_service) = &after_object[1] else {
        panic!("payload[1] is a tuple")
    };
    let PyValue::Tuple(args) = &after_service[1] else {
        panic!("args is a tuple")
    };
    (String::from_utf8_lossy(event).into_owned(), args.clone())
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

/// Read until a `CallRsp`, keeping every notification skipped on the way.
fn call_rsp_collecting(client: &mut MockClient) -> (PyValue, Vec<(String, Vec<PyValue>)>) {
    let mut frames = Vec::new();
    for _ in 0..16 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("expected a packet, got {frame:?}")
        };
        match String::from_utf8_lossy(class).rsplit('.').next() {
            Some("Notification") => frames.push(expect_notification(&frame)),
            Some("SessionChangeNotification") => continue,
            _ => return (expect_call_rsp(&frame), frames),
        }
    }
    panic!("no CallRsp after 16 frames; skipped {frames:?}")
}

/// The ids of the item rows in a hangar listing and their quantities.
fn stacks_of(list: &PyValue, type_id: i64) -> Vec<i64> {
    let PyValue::List(rows) = list else {
        panic!("a hangar listing is a list: {list:?}")
    };
    let mut out = Vec::new();
    for r in rows {
        let PyValue::DbRow(row) = r else {
            panic!("item rows are DBRows: {r:?}")
        };
        let v = row.values();
        // `item_columns`: itemID, typeID, ownerID, locationID, flagID, quantity.
        if v[1] == eve_marshal::DbValue::Int(type_id)
            && let eve_marshal::DbValue::Int(q) = v[5]
        {
            out.push(q);
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn a_pilot_splits_a_hangar_stack() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
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

    // A stack of the fixture's first stackable type, in Alice's hangar.
    let (station, stack) = {
        let mut w = world.write().expect("the world");
        let station = w
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("Alice starts docked");
        w.apply_items(&[eve_server::item::Edit::Create {
            type_id: TYPE_ID,
            quantity: STACK,
            to: eve_server::item::Slot::new(ALICE, station, FLAG_HANGAR),
        }])
        .expect("the stack is made");
        let id = w
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == TYPE_ID && i.owner_id == ALICE)
            .expect("the stack")
            .item_id;
        (station, id)
    };

    // The moniker call, fused onto the bind.
    client.send(&call_req_args(
        "invbroker",
        "MachoBindObject",
        100,
        vec![
            PyValue::Tuple(vec![PyValue::Int(station), PyValue::Int(GROUP_STATION)]),
            PyValue::Tuple(vec![
                bstr("SplitStack"),
                PyValue::Tuple(vec![
                    PyValue::Int(station),
                    PyValue::Int(stack),
                    PyValue::Int(PART),
                    PyValue::Int(ALICE),
                ]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (answer, _) = call_rsp_collecting(&mut client);
    let PyValue::Tuple(pair) = &answer else {
        panic!("a bind answers (boundObject, retval): {answer:?}")
    };
    assert_eq!(pair[1], PyValue::None, "SplitStack answers nothing");

    // The client's cache invalidation after it.
    client.send(&call_req_args(
        "charMgr",
        "MachoBindObject",
        101,
        vec![
            PyValue::Tuple(vec![PyValue::Int(ALICE), PyValue::Int(10_002)]),
            PyValue::Tuple(vec![
                bstr("InvalidateStationItemsCache"),
                PyValue::Tuple(vec![PyValue::Int(station)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (answer, _) = call_rsp_collecting(&mut client);
    let PyValue::Tuple(pair) = &answer else {
        panic!("a bind answers (boundObject, retval): {answer:?}")
    };
    assert_eq!(pair[1], PyValue::None);

    // What the hangar lists now, over the wire: two stacks, same total.
    client.send(&call_req_args(
        "charMgr",
        "MachoBindObject",
        102,
        vec![
            PyValue::Tuple(vec![PyValue::Int(ALICE), PyValue::Int(10_002)]),
            PyValue::Tuple(vec![
                bstr("ListStationItems"),
                PyValue::Tuple(vec![PyValue::Int(station)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (answer, _) = call_rsp_collecting(&mut client);
    let PyValue::Tuple(pair) = &answer else {
        panic!("a bind answers (boundObject, retval): {answer:?}")
    };
    assert_eq!(
        stacks_of(&pair[1], TYPE_ID),
        vec![PART, STACK - PART],
        "the split stack and what is left"
    );
    {
        let w = world.read().expect("the world");
        assert_eq!(
            w.store().get(stack).expect("the original").quantity,
            STACK - PART
        );
    }
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
}
