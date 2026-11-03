//! **A bridging ship opens a portal and a fleet member jumps through it**,
//! over three real sockets (CAP-G; lane note `docs/lanes/cap-g.md`).
//!
//! Alice, Bob and Carol form a fleet. Alice is put in another system and
//! lights a cyno there. Bob, who flies a hull with a Jump Portal Generator,
//! sends what the client's "Bridge to" menu sends, `beyonce.CmdBridgeToFleetModuleBeacon(
//! charID, beaconID, solarsystemID)` (`cynoTravelSvc.py:51`, no session
//! change); the fleet is told `OnBridgeModeChange`. Carol, in Bob's system and
//! within range, sends `beyonce.CmdJumpThroughFleet(otherCharID, otherShipID,
//! beaconID, solarsystemID)` through `PerformSessionChange('jump', ...)`
//! (`cynoTravelSvc.py:78`) with the ids her `activeBridge` holds. What this
//! proves that the unit tests cannot is the wire half: the push to both
//! passengers' clients, the departure FX and timers, the session change, the
//! bind that lands Carol at the field, **the fuel paid out of Bob's hold**, and
//! Carol's fatigue.
//!
//! **Every id here is invented** (types 990,670,xxx, systems 30,670,xxx); the
//! client's constants appear only by name.

use eve_marshal::PyValue;
use eve_proto::{try_split_frame, write_frame};
use eve_server::server::{Config, Mode, Server};
use eve_server::{dumps, loads};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::Duration;

const CHALLENGE_LEN: usize = 64;

const ALICE: i64 = 90_000_001;
const BOB: i64 = 90_000_002;
const CAROL: i64 = 90_000_003;
const ALICE_SHIP: i64 = 100_000_001;
const BOB_SHIP: i64 = 100_000_002;
const CAROL_SHIP: i64 = 100_000_003;

/// Invented: the module, ozone and the module's numbers.
const CYNO: i64 = 990_650_001;
const OZONE: i64 = 990_650_002;
const GROUP_CYNO: i64 = 990_650;
const GROUP_OZONE: i64 = 990_651;
const CYCLE_MS: i64 = 600_000;
const FUEL: i64 = 500;
/// `flagHiSlot0` and `flagCargo` **[V]**.
const HI_SLOT_0: i64 = 27;
const CARGO: i64 = 5;

/// The client's names, which the server must agree on.
const EFFECT_ID: i64 = 2_857;
const EFFECT_NAME: &str = "cynosuralGeneration";
const TYPE_CYNOSURAL_FIELD: i64 = 21_094;

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

fn dict_get<'a>(v: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match v {
        PyValue::Dict(entries) => entries
            .iter()
            .find(|(_, k)| *k == bstr(key))
            .map(|(value, _)| value),
        _ => None,
    }
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

fn change_of<'a>(change: &'a PyValue, key: &str) -> &'a PyValue {
    let entry = dict_get(change, key).unwrap_or_else(|| panic!("{key} missing from {change:?}"));
    let PyValue::Tuple(pair) = entry else {
        panic!("{key} must be an (old, new) pair")
    };
    &pair[1]
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

/// The balls of every `AddBalls2` in a `DoDestinyUpdate` batch.
fn added_balls(args: &[PyValue]) -> Vec<i64> {
    let PyValue::List(entries) = &args[0] else {
        panic!("the batch is a sequence: {args:?}")
    };
    let mut out = Vec::new();
    for entry in entries {
        let PyValue::Tuple(entry) = entry else {
            panic!("an entry is (stamp, (name, args))")
        };
        let PyValue::Tuple(call) = &entry[1] else {
            panic!("(name, args)")
        };
        if call[0] == bstr("SetState") {
            let PyValue::Tuple(call_args) = &call[1] else {
                panic!("args is one tuple")
            };
            let PyValue::Instance { state: bag, .. } = &call_args[0] else {
                panic!("the bag is a KeyVal")
            };
            let blob = dict_get(bag, "state").expect("bag.state");
            let PyValue::Bytes(blob) = blob else {
                panic!("bag.state is a byte string")
            };
            out.extend(
                eve_server::destiny::Park::decode(blob)
                    .expect("decodes")
                    .balls
                    .iter()
                    .map(|b| b.ball_id),
            );
            continue;
        }
        if call[0] != bstr("AddBalls2") {
            continue;
        }
        let PyValue::Tuple(call_args) = &call[1] else {
            panic!("args is one tuple")
        };
        let PyValue::Tuple(pair) = &call_args[0] else {
            panic!("AddBalls2 takes (state, extraBallData)")
        };
        let PyValue::Bytes(blob) = &pair[0] else {
            panic!("the appended state is a byte string")
        };
        out.extend(
            eve_server::destiny::Park::decode(blob)
                .expect("decodes")
                .balls
                .iter()
                .map(|b| b.ball_id),
        );
    }
    out
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

/// A call on an already-bound object, which is how every `Cmd*` arrives.
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

/// Invented: the bridging hull, a passenger hull, the portal module, the
/// strontium it burns, the isotope, and the other system.
const ISOTOPE: i64 = 990_670_003;
const BRIDGER: i64 = 990_670_004;
const PASSENGER: i64 = 990_670_005;
const PORTAL_MODULE: i64 = 990_670_006;
const STRONTIUM: i64 = 990_670_007;
const GROUP_ISOTOPE: i64 = 990_672;
const GROUP_HULL: i64 = 990_673;
const GROUP_PORTAL: i64 = 990_674;
const LIST_PLAIN: i64 = 990_680;
const HOME: i64 = 30_000_001;
const DEST: i64 = 30_670_002;
/// The client's attribute and effect ids, by name.
const EFFECT_PORTAL: i64 = 2_152;
const EFFECT_PORTAL_NAME: &str = "jumpPortalGeneration";
/// Invented drive and distance (never an integer product).
const RANGE: f64 = 5.0;
const PER_LY: f64 = 1_000.0;
const DEST_LY: f64 = 3.2504;
const MASS_FACTOR: f64 = 1.5e-7;
const PASSENGER_MASS: f64 = 4_000_000.0;
const BRIDGER_MASS: f64 = 9_000_000.0;
const PORTAL_MS: i64 = 60_000;
const STRONTIUM_PER_RUN: i64 = 500;
const BAY: i64 = 2_000;
const HOLD: i64 = 5_000;
const STRONTIUM_STACK: i64 = 1_200;
/// What Carol's jump costs: 3.2504 ly x 4,000,000 kg x 1.5e-7 x 1,000 a light
/// year, `long()` of it (1950.24).
const FUEL_NEEDED: i64 = 1_950;
const SPAWN: f64 = 5_000.0;
const FUEL_BAY: i64 = 133;

fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{OZONE}\t{GROUP_OZONE}\t4\t1\t1\t0.4\t0.0\t0.0\t1.0\t0\t\t\tInvented Ozone\n\
         type\t{ISOTOPE}\t{GROUP_ISOTOPE}\t4\t1\t1\t0.15\t0.0\t0.0\t1.0\t0\t\t\tInvented Isotope\n\
         type\t{STRONTIUM}\t{GROUP_ISOTOPE}\t4\t1\t1\t0.2\t0.0\t0.0\t1.0\t0\t\t\tInvented Strontium\n\
         type\t{BRIDGER}\t{GROUP_HULL}\t6\t1\t1\t20000.0\t130.0\t{BRIDGER_MASS}\t33.0\t0\t5000\t\
         9=310.0;37=355.0;482=260.0;861=1.0;866={ISOTOPE}.0;867={RANGE};868={PER_LY};3317={LIST_PLAIN}.0\t\
         Invented Bridger\n\
         type\t{PASSENGER}\t{GROUP_HULL}\t6\t1\t1\t20000.0\t130.0\t{PASSENGER_MASS}\t33.0\t0\t5000\t\
         9=310.0;37=355.0;482=260.0\tInvented Passenger\n\
         attribute\t3319\t1.0\tisTitanJumpPortalPassenger\n\
         attribute\t20\t0.0\tspeedFactor\n\
         attribute\t104\t0.0\twarpScrambleStatus\n\
         attribute\t852\t0.0\tsiegeModeWarpStatus\n\
         type\t{CYNO}\t{GROUP_CYNO}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{EFFECT_ID}\t12,16,{EFFECT_ID}\t\
         30=1.0;50=1.0;73={CYCLE_MS}.0;713={OZONE}.0;714={FUEL}.0;763=1.0;1245=1.0;1334=500.0;2455={SPAWN}\t\
         Invented Cyno Generator\n\
         type\t{PORTAL_MODULE}\t{GROUP_PORTAL}\t7\t1\t1\t50.0\t0.0\t0.0\t1.0\t{EFFECT_PORTAL}\t12,16,{EFFECT_PORTAL}\t\
         6=50.0;20=-100.0;30=1.0;50=1.0;713={STRONTIUM}.0;714={STRONTIUM_PER_RUN}.0;852=100.0;\
         1001={MASS_FACTOR};1002={PORTAL_MS}.0;3318=3319.0\tInvented Portal Generator\n\
         effect\t{EFFECT_ID}\t1\t0\t73\t0\t0\t0\t0\t{EFFECT_NAME}\t\n\
         effect\t{EFFECT_PORTAL}\t1\t0\t1002\t6\t0\t0\t0\t{EFFECT_PORTAL_NAME}\teffects.JumpPortal\n\
         modifier\t{EFFECT_PORTAL}\tshipID\tItemModifier\t104\t852\t2\t\t\n\
         modifier\t{EFFECT_PORTAL}\tshipID\tItemModifier\t37\t20\t6\t\t\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let sys = |id: i64, name: &str| {
        format!("{id}\t10670001\t20670001\t\t\t-0.45\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\t{name}")
    };
    let u = eve_server::celestial::Universe::parse(
        &[sys(HOME, "Invented Home"), sys(DEST, "Invented Dest")],
        &["10670001\t\t\t\t\tInvented Region".to_owned()],
        &["20670001\t10670001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    let centers = eve_server::celestial::Centers::parse(&[
        format!("s\t{HOME}\t0.0\t0.0\t0.0"),
        format!(
            "s\t{DEST}\t{}\t0.0\t0.0",
            DEST_LY * eve_server::celestial::LIGHTYEAR
        ),
    ])
    .expect("the test centres parse");
    c.insert("centers", centers);
    let mut lists = eve_server::dungeon_data::Table::fixture();
    lists.typelists.insert(
        LIST_PLAIN,
        eve_server::dungeon_data::TypeList {
            inc_types: vec![TYPE_CYNOSURAL_FIELD],
            ..Default::default()
        },
    );
    c.insert("dungeon_data", lists);
    c
}

/// Read frames from `client` until one is the notification `want`; its
/// arguments. Anything else is kept in `seen`.
fn until_notification(
    client: &mut MockClient,
    want: &str,
    seen: &mut Vec<(String, Vec<PyValue>)>,
) -> Vec<PyValue> {
    for _ in 0..60 {
        let frame = client.recv();
        let PyValue::Instance { class, .. } = &frame else {
            panic!("a packet")
        };
        if String::from_utf8_lossy(class).rsplit('.').next() == Some("Notification") {
            let (event, args) = expect_notification(&frame);
            if event == want {
                return args;
            }
            seen.push((event, args));
        }
    }
    panic!("no {want}; saw {seen:?}");
}

fn join_fleet(a: &mut MockClient, who: &mut MockClient, who_id: i64, fleet_id: i64, call: i64) {
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Invite",
        call,
        vec![PyValue::Int(who_id)],
    ));
    let _ = call_rsp_collecting(a);
    let mut seen = Vec::new();
    let _ = until_notification(who, "OnFleetInvite", &mut seen);
    who.send(&call_req_args(
        "fleetObjectHandler",
        "MachoBindObject",
        call + 100,
        vec![
            PyValue::Int(fleet_id),
            PyValue::Tuple(vec![
                bstr("AcceptInvite"),
                PyValue::Tuple(vec![PyValue::Int(670)]),
                PyValue::Dict(vec![]),
            ]),
        ],
    ));
    let _ = call_rsp_collecting(who);
}

#[test]
fn a_bridging_ship_opens_a_portal_and_a_fleet_member_jumps_through_it() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    let pilot = |id: i64, name: &str, ship: i64| eve_server::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: ship,
        ..eve_server::model::Character::default()
    };
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", ALICE_SHIP),
        pilot(BOB, "Bob", BOB_SHIP),
        pilot(CAROL, "Carol", CAROL_SHIP),
    ];
    config.content = Some(std::sync::Arc::new(content()));
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let mut a = MockClient::connect(addr);
    full_handshake(&mut a);
    read_session(&mut a);
    select_character(&mut a, ALICE, 70);
    let mut b = MockClient::connect(addr);
    full_handshake(&mut b);
    read_session(&mut b);
    select_character(&mut b, BOB, 80);
    let mut c = MockClient::connect(addr);
    full_handshake(&mut c);
    read_session(&mut c);
    select_character(&mut c, CAROL, 90);
    let (event, _) = expect_notification(&a.recv());
    assert_eq!(event, "OnCharNowInStation");

    // Items: Alice's module and ozone; Bob's hull, portal generator,
    // strontium and isotopes (a bay and a hold); Carol's hull.
    let module_id = {
        let mut w = world.write().expect("the world");
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut() {
            if i.item_id == BOB_SHIP {
                i.type_id = BRIDGER;
                i.group_id = GROUP_HULL;
            }
            if i.item_id == CAROL_SHIP {
                i.type_id = PASSENGER;
                i.group_id = GROUP_HULL;
            }
        }
        w.apply_snapshot(snap);
        w.character_mut(BOB).expect("Bob").ship_type_id = BRIDGER;
        w.character_mut(CAROL).expect("Carol").ship_type_id = PASSENGER;
        w.apply_items(&[
            eve_server::item::Edit::Create {
                type_id: CYNO,
                quantity: 1,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, HI_SLOT_0),
            },
            eve_server::item::Edit::Create {
                type_id: OZONE,
                quantity: 5 * FUEL,
                to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, CARGO),
            },
            eve_server::item::Edit::Create {
                type_id: PORTAL_MODULE,
                quantity: 1,
                to: eve_server::item::Slot::new(BOB, BOB_SHIP, HI_SLOT_0),
            },
            eve_server::item::Edit::Create {
                type_id: STRONTIUM,
                quantity: STRONTIUM_STACK,
                to: eve_server::item::Slot::new(BOB, BOB_SHIP, FUEL_BAY),
            },
            eve_server::item::Edit::Create {
                type_id: ISOTOPE,
                quantity: BAY,
                to: eve_server::item::Slot::new(BOB, BOB_SHIP, FUEL_BAY),
            },
            eve_server::item::Edit::Create {
                type_id: ISOTOPE,
                quantity: HOLD,
                to: eve_server::item::Slot::new(BOB, BOB_SHIP, CARGO),
            },
        ])
        .expect("the items are made");
        w.store()
            .items()
            .iter()
            .find(|i| i.type_id == CYNO && i.owner_id == ALICE)
            .expect("her module")
            .item_id
    };

    // The fleet, in the client's own order (`fleetSvc.py:395-397`).
    a.send(&call_req_args(
        "fleetObjectHandler",
        "CreateFleet",
        71,
        vec![],
    ));
    let _ = call_rsp_collecting(&mut a);
    a.send(&call_req_args(
        "fleetObjectHandler",
        "Init",
        72,
        vec![PyValue::Int(670)],
    ));
    let seated = {
        let mut seated = None;
        for _ in 0..6 {
            let frame = a.recv();
            let PyValue::Instance { class, .. } = &frame else {
                panic!("a packet")
            };
            if String::from_utf8_lossy(class).ends_with("SessionChangeNotification") {
                seated = Some(expect_session_change(&frame));
                break;
            }
        }
        seated.expect("Alice's seat")
    };
    let PyValue::Int(fleet_id) = *change_of(&seated, "fleetid") else {
        panic!("the boss's session names the fleet")
    };
    let _ = call_rsp_collecting(&mut a);
    join_fleet(&mut a, &mut b, BOB, fleet_id, 73);
    join_fleet(&mut a, &mut c, CAROL, fleet_id, 74);

    // Everyone undocks in the home system; Bob and Carol stand a kilometre
    // apart; Alice is somewhere else, at rest, and lights her cyno there.
    let _ = undock(&mut a, 100);
    let beyonce_b = undock(&mut b, 110);
    let beyonce_c = undock(&mut c, 120);
    {
        let mut w = world.write().expect("the world");
        for (who, ship, system, at) in [
            (ALICE, ALICE_SHIP, DEST, [1.0e7, 2.0e6, -3.0e6]),
            (BOB, BOB_SHIP, HOME, [0.0, 0.0, 0.0]),
            (CAROL, CAROL_SHIP, HOME, [1_000.0, 0.0, 0.0]),
        ] {
            let _ = w.roster_mut().leave(who);
            w.roster_mut()
                .arrive(eve_server::space_state::Presence::arriving(
                    who,
                    ship,
                    system,
                    eve_server::space_state::Motion::arrived(at, [0.0; 3]),
                ));
        }
    }
    a.send(&call_req_args(
        "dogmaIM",
        "Activate",
        130,
        vec![
            PyValue::Int(module_id),
            bstr(EFFECT_NAME),
            PyValue::None,
            PyValue::Int(1000),
        ],
    ));
    let _ = call_rsp_collecting(&mut a);
    let mut seen = Vec::new();
    let args = until_notification(&mut b, "OnFleetJumpBeaconModuleChange", &mut seen);
    assert_eq!(args[1], PyValue::Int(DEST), "the beacon's system");
    let PyValue::Int(beacon_id) = args[2] else {
        panic!("the beacon is a ball id")
    };

    // **The call the client's "Bridge to" menu makes**: no session change,
    // answered `None`; Bob and Carol are both told the portal is open.
    b.send(&bound_call(
        &beyonce_b,
        "CmdBridgeToFleetModuleBeacon",
        140,
        vec![
            PyValue::Int(ALICE),
            PyValue::Int(beacon_id),
            PyValue::Int(DEST),
        ],
    ));
    let (reply, frames) = call_rsp_collecting(&mut b);
    assert_eq!(reply, PyValue::None, "answered None");
    let mut told_b: Vec<(String, Vec<PyValue>)> = frames;
    let bridge_b = told_b
        .iter()
        .find(|(e, _)| e == "OnBridgeModeChange")
        .map(|(_, a)| a.clone())
        .unwrap_or_else(|| until_notification(&mut b, "OnBridgeModeChange", &mut told_b));
    let bridge_c = until_notification(&mut c, "OnBridgeModeChange", &mut seen);
    let want = vec![
        PyValue::Int(BOB_SHIP),
        PyValue::Int(DEST),
        PyValue::Int(beacon_id),
        PyValue::Bool(true),
    ];
    assert_eq!(bridge_b, want, "the bridging pilot");
    assert_eq!(bridge_c, want, "the passenger");
    {
        let w = world.read().expect("the world");
        assert!(
            w.modules()
                .running(module_of(&w, BOB, PORTAL_MODULE))
                .is_some(),
            "the generator cycles"
        );
        assert_eq!(
            strontium(&w),
            STRONTIUM_STACK - STRONTIUM_PER_RUN,
            "the module's own fuel was paid"
        );
    }

    // **The call the client's jump-through entry makes**, with the ids
    // `fleet.GetActiveBridgeForShip` holds, inside `PerformSessionChange`.
    c.send(&bound_call(
        &beyonce_c,
        "CmdJumpThroughFleet",
        150,
        vec![
            PyValue::Int(BOB),
            PyValue::Int(BOB_SHIP),
            PyValue::Int(beacon_id),
            PyValue::Int(DEST),
        ],
    ));
    let mut seen: Vec<(String, Vec<PyValue>)> = Vec::new();
    let mut answered = false;
    let change = 'wait: {
        for _ in 0..40 {
            let frame = c.recv();
            let PyValue::Instance { class, .. } = &frame else {
                panic!("a packet")
            };
            match String::from_utf8_lossy(class).rsplit('.').next() {
                Some("Notification") => seen.push(expect_notification(&frame)),
                Some("SessionChangeNotification") => break 'wait expect_session_change(&frame),
                _ => {
                    assert_eq!(expect_call_rsp(&frame), PyValue::None, "answered None");
                    answered = true;
                }
            }
        }
        panic!("no session change after the jump; saw {seen:?}");
    };
    assert!(answered, "PerformSessionChange blocks on the answer");
    let fx = seen
        .iter()
        .find(|(e, args)| {
            e == "DoDestinyUpdate"
                && format!("{args:?}").contains(&format!("{:?}", b"effects.JumpDriveOut".to_vec()))
        })
        .expect("the departure FX reaches the passenger");
    assert_eq!(fx.1.len(), 2);
    assert!(
        seen.iter().any(|(e, _)| e == "OnJumpTimersUpdated"),
        "the passenger's timers: {:?}",
        seen.iter().map(|(e, _)| e).collect::<Vec<_>>()
    );
    let PyValue::Tuple(pair) = dict_get(&change, "solarsystemid").expect("the system moves") else {
        panic!("a pair")
    };
    assert_eq!(pair, &vec![PyValue::Int(HOME), PyValue::Int(DEST)]);

    // The client rebinds `beyonce` in the new system; the bind lands Carol.
    c.send(&call_req_args(
        "beyonce",
        "MachoResolveObject",
        151,
        vec![PyValue::Int(DEST)],
    ));
    let _ = call_rsp_collecting(&mut c);
    c.send(&call_req_args(
        "beyonce",
        "MachoBindObject",
        152,
        vec![PyValue::Int(DEST), PyValue::None],
    ));
    let (_bound, frames) = call_rsp_collecting(&mut c);
    let mut added = Vec::new();
    for (event, args) in frames.iter() {
        if event == "DoDestinyUpdate" {
            added.extend(added_balls(args));
        }
    }
    for _ in 0..8 {
        if added.contains(&beacon_id) {
            break;
        }
        let (event, args) = expect_notification(&c.recv());
        if event == "DoDestinyUpdate" {
            added.extend(added_balls(&args));
        }
    }
    assert!(
        added.contains(&beacon_id) && added.contains(&CAROL_SHIP),
        "Carol's new park holds the field and herself: {added:?}"
    );

    // And the server agrees: where she is, who paid, what is set.
    let w = world.read().expect("the world");
    let me = w.roster().get(CAROL).copied().expect("Carol is in space");
    assert_eq!(me.solar_system_id, DEST);
    let field = eve_server::cyno::field_by_ball(&w, beacon_id).expect("the field");
    let d: f64 = (0..3)
        .map(|i| (me.motion.position[i] - field.position[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(d <= SPAWN, "{d} m from the field, inside {SPAWN}");
    assert!(
        w.roster()
            .get(BOB)
            .is_some_and(|p| p.solar_system_id == HOME),
        "the bridging ship stays"
    );
    let isotopes = |who: i64| -> i64 {
        w.store()
            .items()
            .iter()
            .filter(|i| i.type_id == ISOTOPE && i.owner_id == who)
            .map(|i| i.quantity)
            .sum()
    };
    assert_eq!(
        isotopes(BOB),
        BAY + HOLD - FUEL_NEEDED,
        "**the bridging ship** paid by the passenger's mass"
    );
    assert_eq!(isotopes(CAROL), 0, "the passenger paid nothing");
    let now = eve_server::pyutil::filetime_now();
    assert!(
        eve_server::jump_fatigue::activation_remaining(&w, CAROL, now).is_some(),
        "the passenger's fatigue is set"
    );
    assert!(
        eve_server::jump_fatigue::activation_remaining(&w, BOB, now).is_none(),
        "the bridging pilot's is not"
    );
    assert!(
        eve_server::jump_portal::portal_of_ship(&w, BOB_SHIP).is_some(),
        "the portal stays open for the next"
    );
    drop(w);

    a.stream.shutdown(Shutdown::Both).expect("shutdown");
    b.stream.shutdown(Shutdown::Both).expect("shutdown");
    c.stream.shutdown(Shutdown::Both).expect("shutdown");
}

fn module_of(w: &eve_server::service::Registry, who: i64, type_id: i64) -> i64 {
    w.store()
        .items()
        .iter()
        .find(|i| i.type_id == type_id && i.owner_id == who)
        .expect("the module")
        .item_id
}

fn strontium(w: &eve_server::service::Registry) -> i64 {
    w.store()
        .items()
        .iter()
        .filter(|i| i.type_id == STRONTIUM && i.owner_id == BOB)
        .map(|i| i.quantity)
        .sum()
}
