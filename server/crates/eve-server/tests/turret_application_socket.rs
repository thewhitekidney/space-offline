//! **Turret application over a real socket** (DOC-0 K-TURRET; lane note
//! `docs/lanes/doc-0.md`): Alice's gun fires at a rat inside its range and at
//! one beyond its hard cut. The first volley's combat-log line carries a
//! `hitQuality` above 0 and damage; the second's carries `hitQuality` 0 and no
//! damage -- what the client's `uilog` turns into "hits" and "misses" lines.
//!
//! Every id is the fixture's or invented; the gun is the fixture autocannon
//! (optimal 900 m, falloff 4000 m, no signature resolution, so the roll is
//! range only).

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
const SYSTEM: i64 = 30_000_001;
/// The fixture autocannon and its ammo (`typedata::FIXTURE`): optimal
/// 900 m, falloff 4000 m, so the hard cut is at 12.9 km.
const GUN: i64 = 990_000_002;
const AMMO: i64 = 990_000_005;
/// `flagHiSlot0`.
const HI_SLOT_0: i64 = 27;
const NEAR: f64 = 500.0;
const BEYOND_THE_CUT: f64 = 60_000.0;

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
fn undock(client: &mut MockClient, first_call_id: i64) -> String {
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
        "GetFormations",
        first_call_id + 1,
        vec![],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        first_call_id + 2,
        vec![PyValue::Int(30_000_001)],
    ));
    let _ = call_rsp_collecting(client);
    client.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        first_call_id + 3,
        vec![PyValue::Int(30_000_001), PyValue::None],
    ));
    let (bound, _) = call_rsp_collecting(client);
    bound_object_id(&bound)
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

/// Read frames, keeping every notification, until one satisfies `want`.
fn pump_until(
    client: &mut MockClient,
    seen: &mut Vec<(String, Vec<PyValue>)>,
    what: &str,
    want: impl Fn(&str, &[PyValue]) -> bool,
) -> Vec<PyValue> {
    for _ in 0..400 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("a packet, got {frame:?}")
        };
        if String::from_utf8_lossy(class).rsplit('.').next() != Some("Notification") {
            continue;
        }
        let (event, args) = expect_notification(&frame);
        let hit = want(&event, &args);
        seen.push((event, args.clone()));
        if hit {
            return args;
        }
    }
    panic!(
        "no {what} in 400 frames; saw {:?}",
        seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
}

fn dict_get<'a>(d: &'a [(PyValue, PyValue)], key: &str) -> Option<&'a PyValue> {
    d.iter().find(|(_, k)| *k == bstr(key)).map(|(v, _)| v)
}

/// Alice's gun fires one volley at a rat `distance` away; the hit quality
/// and damage of her own combat-log line.
fn one_volley_at(distance: f64) -> (i64, f64) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        ..eve_server::model::Character::default()
    }];
    config.content = Some(std::sync::Arc::new(eve_server::content::Content::fixture()));
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
    let _beyonce = undock(&mut client, 100);
    {
        let mut w = world.write().expect("the world");
        assert!(w.combat().turret_application(), "on by default");
        w.apply_items(&[
            eve_server::item::Edit::Create {
                type_id: GUN,
                quantity: 1,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_0),
            },
            eve_server::item::Edit::Create {
                type_id: AMMO,
                quantity: 100,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_0),
            },
        ])
        .expect("the gun and its rounds");
        let gun = w
            .store()
            .items()
            .iter()
            .find(|i| i.type_id == GUN && i.owner_id == ALICE)
            .expect("the gun")
            .item_id;
        let at = w.roster().get(ALICE).expect("in space").motion.position;
        let kind = eve_server::npc::find(990_310_001).expect("the fixture rat");
        let rat = eve_server::npc::spawn(&mut w, kind, SYSTEM, [at[0] + distance, at[1], at[2]])
            .expect("a rat");
        w.locks_mut().add(ALICE_SHIP, rat);
        w.modules_mut().set_online(gun, true);
        let now = eve_server::pyutil::filetime_now();
        eve_server::modules::activate(
            &mut w,
            Some(ALICE),
            gun,
            "projectileFired",
            Some(rat),
            1,
            now,
        )
        .expect("the gun activates");
    }
    let mut seen = Vec::new();
    let line = pump_until(&mut client, &mut seen, "Alice's own line", |e, a| {
        e == "OnDamageMessage"
            && matches!(&a[0], PyValue::Dict(d) if dict_get(d, "weapon").is_some() || dict_get(d, "target").is_some())
    });
    let PyValue::Dict(d) = &line[0] else {
        panic!("a dict")
    };
    let Some(PyValue::Int(q)) = dict_get(d, "hitQuality") else {
        panic!("hitQuality: {d:?}")
    };
    let Some(PyValue::Real(dmg)) = dict_get(d, "damage") else {
        panic!("damage: {d:?}")
    };
    let out = (*q, *dmg);
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    out
}

#[test]
fn a_gun_hits_inside_its_range_and_misses_beyond_the_cut() {
    let (q, damage) = one_volley_at(NEAR);
    assert!(
        q > 0 && damage > 0.0,
        "inside optimal: quality {q}, damage {damage}"
    );
    let (q, damage) = one_volley_at(BEYOND_THE_CUT);
    assert_eq!(q, 0, "beyond the hard cut the volley misses");
    assert_eq!(damage, 0.0);
}
