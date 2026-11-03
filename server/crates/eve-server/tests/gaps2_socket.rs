//! **A pilot renames a ship and keeps notes and contact labels**, over a real
//! socket (GAPS-2; lane note `docs/lanes/gaps-2.md`).
//!
//! The rename is `invbroker.SetLabel(itemID, name)` on the inventory manager
//! (`menuFunctions.SetName:402`, a bound call with the call fused onto the
//! bind, answer dropped), after which the client reads the name from
//! `config.GetMultiLocationsEx` (`cfg.evelocations`) and the server has told
//! it `OnCfgDataChanged('evelocations', row)` (`cfg.py:210-232`). The notes
//! are `charMgr.SetNote` / `GetNote` (`panelNotes.py:40,67`) and the notepad's
//! `AddOwnerNote` / `GetOwnerNoteLabels`; the labels are `charMgr.CreateLabel`
//! / `GetLabels` / `AssignLabels` (`addressbookService.py:1195-1328`).
//!
//! **Every id is invented.** The harness helpers are copies of
//! `tests/split_socket.rs`'.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const CHALLENGE_LEN: usize = 64;

const ALICE: i64 = 90_000_001;
const ALICE_SHIP: i64 = 100_000_001;
/// `groupStation` **[V]**.
const GROUP_STATION: i64 = 15;
const BOB: i64 = 90_000_002;

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

fn start() -> (MockClient, i64) {
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
    let station = world
        .read()
        .expect("the world")
        .character(ALICE)
        .and_then(|c| c.station_id)
        .expect("Alice starts docked");
    (client, station)
}

fn plain(
    client: &mut MockClient,
    id: i64,
    service: &str,
    method: &str,
    args: Vec<PyValue>,
) -> PyValue {
    client.send(&call_req_args(service, method, id, args));
    call_rsp_collecting(client).0
}

fn u(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn attr(v: &PyValue, name: &str) -> PyValue {
    let PyValue::Instance { state, .. } = v else {
        panic!("a KeyVal: {v:?}")
    };
    eve_server::pyutil::dict_get(state, name.as_bytes())
        .unwrap_or_else(|| panic!("no .{name}"))
        .clone()
}

#[test]
fn a_pilot_renames_a_ship_and_the_client_reads_it_back() {
    let (mut client, station) = start();
    // The moniker call, fused onto the bind, as `SetName` makes it.
    client.send(&call_req_args(
        "invbroker",
        "MachoBindObject",
        100,
        vec![
            PyValue::Tuple(vec![PyValue::Int(station), PyValue::Int(GROUP_STATION)]),
            PyValue::Tuple(vec![
                bstr("SetLabel"),
                PyValue::Tuple(vec![PyValue::Int(ALICE_SHIP), u("Moby Dick")]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (answer, pushed) = call_rsp_collecting(&mut client);
    let PyValue::Tuple(pair) = &answer else {
        panic!("a bind answers (boundObject, retval): {answer:?}")
    };
    assert_eq!(pair[1], PyValue::None, "SetLabel answers nothing");
    // `OnCfgDataChanged('evelocations', row)` -- the notification may
    // arrive before or after the answer.
    let mut notes = pushed;
    if !notes.iter().any(|(n, _)| n == "OnCfgDataChanged") {
        for _ in 0..8 {
            let frame = client.recv();
            if let PyValue::Instance { class, .. } = &frame
                && String::from_utf8_lossy(class).ends_with("Notification")
            {
                notes.push(expect_notification(&frame));
                break;
            }
        }
    }
    let (_, args) = notes
        .iter()
        .find(|(n, _)| n == "OnCfgDataChanged")
        .expect("the client is told of the new name");
    assert_eq!(args[0], u("evelocations"));
    let PyValue::List(row) = &args[1] else {
        panic!("a row list: {:?}", args[1])
    };
    assert_eq!(row[0], PyValue::Int(ALICE_SHIP));
    assert_eq!(row[1], u("Moby Dick"));

    // And `cfg.evelocations.Prime([ship])` finds it.
    let got = plain(
        &mut client,
        101,
        "config",
        "GetMultiLocationsEx",
        vec![PyValue::List(vec![PyValue::Int(ALICE_SHIP)])],
    );
    assert!(
        format!("{got:?}").contains("Moby Dick"),
        "GetMultiLocationsEx names the ship: {got:?}"
    );
}

#[test]
fn a_pilot_keeps_notes_and_labels_a_contact() {
    let (mut client, _) = start();
    // The note on a character, then read back.
    let r = plain(
        &mut client,
        110,
        "charMgr",
        "SetNote",
        vec![PyValue::Int(BOB), u("pays late")],
    );
    assert_eq!(r, PyValue::None);
    let r = plain(
        &mut client,
        111,
        "charMgr",
        "GetNote",
        vec![PyValue::Int(BOB)],
    );
    assert_eq!(r, u("pays late"));
    // The notepad.
    let id = plain(
        &mut client,
        112,
        "charMgr",
        "AddOwnerNote",
        vec![u("N:Plans"), u("<br>")],
    );
    let PyValue::Int(id) = id else {
        panic!("an id")
    };
    let all = plain(&mut client, 113, "charMgr", "GetOwnerNoteLabels", vec![]);
    let PyValue::List(rows) = all else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(attr(&rows[0], "label"), u("N:Plans"));
    assert_eq!(attr(&rows[0], "noteID"), PyValue::Int(id));
    // A contact label: made, listed in the dict the address book reads, and
    // assigned to nobody in an empty book (no error).
    let label = plain(
        &mut client,
        114,
        "charMgr",
        "CreateLabel",
        vec![u("Friends"), PyValue::Int(3)],
    );
    let PyValue::Int(label) = label else {
        panic!("a label id")
    };
    let labels = plain(&mut client, 115, "charMgr", "GetLabels", vec![]);
    let PyValue::Dict(d) = labels else {
        panic!("a dict")
    };
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].1, PyValue::Int(label));
    assert_eq!(attr(&d[0].0, "name"), u("Friends"));
    let r = plain(
        &mut client,
        116,
        "charMgr",
        "AssignLabels",
        vec![PyValue::List(vec![PyValue::Int(BOB)]), PyValue::Int(label)],
    );
    assert_eq!(r, PyValue::None);
}

/// Undock, as the client does after the session change: the `ship` bind with
/// `Undock` fused on, then the ballpark's bind, which seats the pilot in
/// space.
fn undock(client: &mut MockClient, first_call_id: i64) {
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        first_call_id,
        vec![
            PyValue::Tuple(vec![PyValue::Int(60_012_526), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Undock"),
                PyValue::Tuple(vec![]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        first_call_id + 1,
        vec![PyValue::Int(30_000_001)],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        first_call_id + 2,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let _ = call_rsp_collecting(client);
}

#[test]
fn a_pilot_jettisons_cargo_into_a_can_in_space() {
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
    undock(&mut client, 100);

    // Cargo in the hold: a stackable of the fixture's item table.
    let stack = {
        let mut w = world.write().expect("the world");
        w.apply_items(&[eve_server::item::Edit::Create {
            type_id: 34,
            quantity: 250,
            to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, 5),
        }])
        .expect("the stack is made");
        w.store()
            .items()
            .iter()
            .find(|i| i.type_id == 34 && i.owner_id == ALICE)
            .expect("the stack")
            .item_id
    };

    // `GetShipAccess().Jettison(itemIDs)` -> `(jettisonedToCanIDs, [])`.
    client.send(&call_req_args(
        "ship",
        "MachoBindObject",
        200,
        vec![
            PyValue::Tuple(vec![PyValue::Int(30_000_001), PyValue::Int(5)]),
            PyValue::Tuple(vec![
                bstr("Jettison"),
                PyValue::Tuple(vec![PyValue::List(vec![PyValue::Int(stack)])]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let (answer, _) = call_rsp_collecting(&mut client);
    let PyValue::Tuple(pair) = &answer else {
        panic!("a bind answers (boundObject, retval): {answer:?}")
    };
    assert_eq!(
        pair[1],
        PyValue::Tuple(vec![
            PyValue::List(vec![PyValue::Int(stack)]),
            PyValue::List(vec![])
        ]),
        "(jettisonedToCanIDs, jettisonItemList)"
    );
    let w = world.read().expect("the world");
    let item = w.item(stack).expect("the stack is kept");
    let can = w.item(item.location_id).expect("it lies in a can");
    assert_eq!(
        (can.group_id, can.owner_id),
        (12, ALICE),
        "the can is Alice's cargo container"
    );
    assert!(
        w.combat().wreck(can.item_id).is_some(),
        "and is a thing in space"
    );
}
