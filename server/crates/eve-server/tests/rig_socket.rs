//! **Rigs over a real socket** (X-RIG; lane note `docs/lanes/x-rig.md`): a
//! rig within the hull's calibration is fitted by the call the client's
//! fitting window makes (`invbroker` `Add` on the ship's inventory with the
//! rig-slot flag, `fittingSlotController`), a rig that would pass the
//! calibration, or is of the wrong size, is refused with the client's own
//! keys, and unfitting one (`DestroyFitting`, what the window sends after the
//! `RigUnFittingInfo` prompt) destroys it.
//!
//! Every id and number is invented: the hull and the rigs are rows added to
//! the fixture's typedata.

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
const STATION: i64 = 60_012_526;
const HULL: i64 = 990_810_001;
const GROUP_HULL: i64 = 990_810;
const GROUP_RIG: i64 = 990_811;
const RIG_A: i64 = 990_810_011;
const RIG_B: i64 = 990_810_012;
const RIG_BIG: i64 = 990_810_013;
const RIG_MEDIUM: i64 = 990_810_014;
const CALIBRATION: f64 = 100.0;
const COST_A: f64 = 60.0;
const COST_B: f64 = 40.0;
const COST_BIG: f64 = 50.0;
const FLAG_HANGAR: i64 = 4;
const FIRST_RIG_FLAG: i64 = 92;

fn content() -> std::sync::Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{HULL}\t{GROUP_HULL}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
         1137=3.0;1132={CALIBRATION};1547=1.0\tInvented Hull\n\
         type\t{RIG_A}\t{GROUP_RIG}\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
         1153={COST_A};1547=1.0\tInvented Rig A\n\
         type\t{RIG_B}\t{GROUP_RIG}\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
         1153={COST_B};1547=1.0\tInvented Rig B\n\
         type\t{RIG_BIG}\t{GROUP_RIG}\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
         1153={COST_BIG};1547=1.0\tInvented Rig Big\n\
         type\t{RIG_MEDIUM}\t{GROUP_RIG}\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
         1153=10.0;1547=2.0\tInvented Rig Medium\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    std::sync::Arc::new(c)
}

/// The next frame that is neither a notification nor a session change.
fn next_answer(client: &mut MockClient) -> PyValue {
    for _ in 0..64 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("a packet, got {frame:?}")
        };
        match String::from_utf8_lossy(class).rsplit('.').next() {
            Some("Notification" | "SessionChangeNotification") => continue,
            _ => return frame,
        }
    }
    panic!("no answer in 64 frames")
}

/// The id of the object a bind answers with.
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
    let PyValue::Tuple(parts) = loads(inner).expect("the reference loads") else {
        panic!("a reference is (objectID, refID)")
    };
    let PyValue::Bytes(object_id) = &parts[0] else {
        panic!("the objectID is a string")
    };
    String::from_utf8_lossy(object_id).into_owned()
}

/// A call on an already-bound object: the destination is the node.
fn bound_call(
    object_id: &str,
    method: &str,
    call_id: i64,
    args: Vec<PyValue>,
    kwargs: Vec<(PyValue, PyValue)>,
) -> PyValue {
    let payload = eve_proto::CallPayload::BoundObject {
        object_id: object_id.to_owned(),
        method: method.to_owned(),
        args,
        kwargs,
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

/// `invCache.GetInventoryFromId(shipID, 1)` on the station's `invbroker`
/// (`bindParams = (stationID, 15)`): the answer's second element is the
/// ship's inventory, itself a bound object.
fn bind_ship_inventory(client: &mut MockClient, call_id: i64) -> String {
    client.send(&call_req_args(
        "invbroker",
        "MachoBindObject",
        call_id,
        vec![
            PyValue::Tuple(vec![PyValue::Int(STATION), PyValue::Int(15)]),
            PyValue::Tuple(vec![
                bstr("GetInventoryFromId"),
                PyValue::Tuple(vec![PyValue::Int(ALICE_SHIP), PyValue::Int(1)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let frame = next_answer(client);
    let PyValue::Tuple(pair) = expect_call_rsp(&frame) else {
        panic!("a bind answers (boundObject, retval)")
    };
    bound_object_id(&PyValue::Tuple(vec![pair[1].clone(), PyValue::None]))
}

/// One call on the ship's inventory: `Ok` for a reply, or the key of the
/// `UserError` an `ErrorResponse` carries.
fn ship_call(
    client: &mut MockClient,
    inventory: &str,
    call_id: i64,
    method: &str,
    args: Vec<PyValue>,
    kwargs: Vec<(&str, PyValue)>,
) -> Result<(), String> {
    let kwargs = kwargs.into_iter().map(|(k, v)| (v, bstr(k))).collect();
    client.send(&bound_call(inventory, method, call_id, args, kwargs));
    let frame = next_answer(client);
    let PyValue::Instance { class, state } = &frame else {
        unreachable!()
    };
    if String::from_utf8_lossy(class).ends_with("CallRsp") {
        return Ok(());
    }
    // An ErrorResponse: the key of the UserError it carries.
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
    Err(String::from_utf8_lossy(key).into_owned())
}

fn fit(
    client: &mut MockClient,
    inventory: &str,
    call_id: i64,
    item: i64,
    flag: i64,
) -> Result<(), String> {
    ship_call(
        client,
        inventory,
        call_id,
        "Add",
        vec![PyValue::Int(item), PyValue::Int(STATION)],
        vec![("qty", PyValue::Int(1)), ("flag", PyValue::Int(flag))],
    )
}

type World = std::sync::Arc<std::sync::RwLock<eve_server::service::Registry>>;

fn grant(world: &World, type_id: i64) -> i64 {
    let mut w = world.write().expect("the world");
    w.apply_items(&[eve_server::item::Edit::Create {
        type_id,
        quantity: 1,
        to: eve_server::item::Slot::new(ALICE, STATION, FLAG_HANGAR),
    }])
    .expect("created");
    w.store()
        .items()
        .iter()
        .filter(|i| i.type_id == type_id && i.owner_id == ALICE)
        .map(|i| i.item_id)
        .max()
        .expect("the new item")
}

/// The type fitted at `flag` on Alice's ship.
fn fitted(world: &World, flag: i64) -> Option<i64> {
    let w = world.read().expect("the world");
    w.store()
        .contents_of(ALICE_SHIP)
        .find(|i| i.flag_id == flag)
        .map(|i| i.type_id)
}

#[test]
fn a_rig_fits_within_calibration_is_refused_over_it_and_is_destroyed_on_unfit() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        ..eve_server::model::Character::default()
    }];
    config.content = Some(content());
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

    // Alice flies the invented hull, docked.
    {
        let mut w = world.write().expect("the world");
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = HULL;
            i.group_id = GROUP_HULL;
        }
        w.apply_snapshot(snap);
        w.character_mut(ALICE).expect("Alice").ship_type_id = HULL;
    }
    let (a, b, big, medium) = (
        grant(&world, RIG_A),
        grant(&world, RIG_B),
        grant(&world, RIG_BIG),
        grant(&world, RIG_MEDIUM),
    );

    let inv = bind_ship_inventory(&mut client, 71);

    // Within calibration: the first rig, then the second to the brim.
    assert_eq!(fit(&mut client, &inv, 71, a, FIRST_RIG_FLAG), Ok(()));
    assert_eq!(fitted(&world, FIRST_RIG_FLAG), Some(RIG_A));
    assert_eq!(fit(&mut client, &inv, 72, b, FIRST_RIG_FLAG + 1), Ok(()));
    assert_eq!(fitted(&world, FIRST_RIG_FLAG + 1), Some(RIG_B));
    assert_eq!(COST_A + COST_B, CALIBRATION, "exactly at capacity");

    // Over calibration, with a slot free: refused, nothing moves.
    assert_eq!(
        fit(&mut client, &inv, 73, big, FIRST_RIG_FLAG + 2),
        Err("NotEnoughUpgradeCapacity".to_owned())
    );
    assert_eq!(fitted(&world, FIRST_RIG_FLAG + 2), None);
    // The wrong size.
    assert_eq!(
        fit(&mut client, &inv, 74, medium, FIRST_RIG_FLAG + 2),
        Err("CannotFitRigWrongSize".to_owned())
    );

    // Unfit: the window's DestroyFitting. The rig is gone, not in the
    // hangar, and its calibration is free again.
    assert_eq!(
        ship_call(
            &mut client,
            &inv,
            75,
            "DestroyFitting",
            vec![PyValue::Int(a)],
            vec![]
        ),
        Ok(())
    );
    {
        let w = world.read().expect("the world");
        assert!(w.store().get(a).is_none(), "destroyed, not returned");
        assert!(
            !w.store()
                .items()
                .iter()
                .any(|i| i.type_id == RIG_A && i.owner_id == ALICE),
            "no rig of that type is left"
        );
    }
    assert_eq!(fitted(&world, FIRST_RIG_FLAG), None);
    assert_eq!(fit(&mut client, &inv, 76, big, FIRST_RIG_FLAG), Ok(()));
    assert_eq!(fitted(&world, FIRST_RIG_FLAG), Some(RIG_BIG));
}
