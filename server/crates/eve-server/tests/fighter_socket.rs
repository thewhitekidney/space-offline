//! **A squadron is launched, ordered to attack an NPC, does damage and is
//! recalled**, over a real socket (FT-E; lane note `docs/lanes/ft-e.md`).
//!
//! Alice flies an invented carrier hull with a fighter stack in her fighter
//! bay. She undocks, loads a tube, launches it, and sends what the client's
//! squadron window sends: `fighterMgr.CmdActivateAbilitySlots(fighterIDs,
//! abilitySlotID, targetID)` (`fightersSvc._ActivateAbilitySlots` [V]); the
//! clock runs the cycle, the combat log line and the ability pushes come
//! back, the NPC takes the damage; she deactivates and recalls
//! (`RecallFightersToTubes`) and the tube is `READY` again.
//!
//! **Every id here is invented** (types 990,59x,xxx); the client's constants
//! appear only by name. The NPC is the server's own fixture rat; no lock is
//! made, as the server does not ask for one (a drone's `engage` does not
//! either).

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
/// `flagFighterBay` and the first `flagFighterTube`.
const FLAG_BAY: i64 = 158;
const FIRST_TUBE: i64 = 159;

/// Invented: the carrier hull and the fighter.
const CARRIER: i64 = 990_591_001;
const FIGHTER: i64 = 990_591_010;
const GROUP_CARRIER: i64 = 990_591;
const GROUP_FIGHTER: i64 = 990_592;
const ABILITY: i64 = 990_791;
/// What the fixture fighter does: a volley of `EM x MULT` per fighter every
/// `CYCLE_MS`, and a short refuel so the test is quick.
const SQUADRON: i64 = 6;
const EM: f64 = 1.0;
const MULT: f64 = 1.0;
const CYCLE_MS: i64 = 500;
const REFUEL_MS: i64 = 500;
const RANGE: f64 = 20_000.0;

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

/// The invented world: the fixture's tables with a carrier hull (three
/// tubes, two light, a bay), a light fighter with one attack ability, the
/// effect row that ability points at, and the fighter tables.
fn content() -> eve_server::content::Content {
    let atk = eve_server::fighters_data::effect::ATTACK;
    let rows = format!(
        "{}type\t{CARRIER}\t{GROUP_CARRIER}\t6\t1\t1\t50000.0\t400.0\t10000000.0\t200.0\t0\t\t\
         37=200.0;70=1.0;76=100000.0;2216=3.0;2217=2.0;2218=1.0;2055=100.0\tInvented Carrier\n\
         type\t{FIGHTER}\t{GROUP_FIGHTER}\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t\
         37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2223=2000.0;\
         2426={REFUEL_MS}.0;2226={MULT};2227={EM};2233={CYCLE_MS}.0;2236={RANGE}\tInvented Fighter\n\
         effect\t{atk}\t2\t1\t2233\t0\t2236\t2237\t0\tinventedFighterAttack\teffects.InventedFighterAttack\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let fighters = eve_server::fighters_data::Fighters::parse(
        &[format!(
            "{ABILITY}\titemTargeted\t0\t0\t{atk}\t990631\t\tInvented Attack"
        )],
        &[format!("{FIGHTER}\t0\t{ABILITY}\t\t\t")],
    )
    .expect("the fighter tables parse");
    c.insert("fighters_data", fighters);
    c
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

/// `OnFighterTubeTaskStatus(tube, status, ..)` naming `status`.
fn tube_status(status: &'static str) -> impl Fn(&str, &[PyValue]) -> bool {
    move |event, args| {
        event == "OnFighterTubeTaskStatus"
            && args.first() == Some(&PyValue::Int(FIRST_TUBE))
            && args.get(1) == Some(&bstr(status))
    }
}

/// One call on a service, answered. Notifications that arrive first are kept.
fn call(
    client: &mut MockClient,
    seen: &mut Vec<(String, Vec<PyValue>)>,
    method: &str,
    call_id: i64,
    args: Vec<PyValue>,
) -> PyValue {
    client.send(&call_req_args("fighterMgr", method, call_id, args));
    let (answer, frames) = call_rsp_collecting(client);
    seen.extend(frames);
    answer
}

fn ids(v: &[i64]) -> PyValue {
    PyValue::List(v.iter().map(|i| PyValue::Int(*i)).collect())
}

#[test]
fn a_squadron_launches_attacks_an_npc_and_is_recalled() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        ..eve_server::model::Character::default()
    }];
    config.content = Some(std::sync::Arc::new(content()));
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

    // The hull is a carrier and the bay holds a stack.
    let bay_stack = {
        let mut w = world.write().expect("the world");
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = CARRIER;
            i.group_id = GROUP_CARRIER;
        }
        w.apply_snapshot(snap);
        w.character_mut(ALICE).expect("Alice").ship_type_id = CARRIER;
        w.apply_items(&[eve_server::item::Edit::Create {
            type_id: FIGHTER,
            quantity: SQUADRON,
            to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, FLAG_BAY),
        }])
        .expect("the stack is made");
        w.store()
            .items()
            .iter()
            .find(|i| i.type_id == FIGHTER && i.owner_id == ALICE)
            .expect("the stack")
            .item_id
    };
    let _beyonce = undock(&mut client, 100);

    let mut seen = Vec::new();
    // Load, wait for READY, launch, wait for INSPACE.
    let answer = call(
        &mut client,
        &mut seen,
        "LoadFightersToTube",
        110,
        vec![PyValue::Int(bay_stack), PyValue::Int(FIRST_TUBE)],
    );
    assert_eq!(answer, PyValue::None);
    pump_until(&mut client, &mut seen, "READY", tube_status("READY"));
    seen.clear();
    let answer = call(
        &mut client,
        &mut seen,
        "LaunchFightersFromTubes",
        111,
        vec![ids(&[FIRST_TUBE])],
    );
    assert_eq!(
        answer,
        PyValue::Dict(vec![(PyValue::None, PyValue::Int(FIRST_TUBE))]),
        "{{tubeFlagID: None}}"
    );
    let added = if let Some((_, a)) = seen.iter().find(|(e, _)| e == "OnFighterAddedToController") {
        a.clone()
    } else {
        pump_until(&mut client, &mut seen, "the controller event", |e, _| {
            e == "OnFighterAddedToController"
        })
    };
    let PyValue::Int(squadron) = added[0] else {
        panic!("the squadron's item id: {added:?}")
    };
    assert_eq!(added[3], PyValue::Int(SQUADRON), "its size");
    pump_until(&mut client, &mut seen, "INSPACE", tube_status("INSPACE"));

    // A rat three kilometres from the squadron.
    let rat = {
        let mut w = world.write().expect("the world");
        let at = w
            .roster()
            .get(squadron)
            .expect("the squadron flies")
            .motion
            .position;
        let kind = eve_server::npc::find(990_310_001).expect("the fixture rat");
        eve_server::npc::spawn(&mut w, kind, SYSTEM, [at[0] + 3_000.0, at[1], at[2]])
            .expect("a rat")
    };
    seen.clear();

    // **The call the squadron window makes**: the squadron, the slot, the
    // target. Answered `{fighterID: None}`.
    let answer = call(
        &mut client,
        &mut seen,
        "CmdActivateAbilitySlots",
        112,
        vec![ids(&[squadron]), PyValue::Int(0), PyValue::Int(rat)],
    );
    assert_eq!(
        answer,
        PyValue::Dict(vec![(PyValue::None, PyValue::Int(squadron))])
    );
    let on = pump_until(&mut client, &mut seen, "the activation", |e, _| {
        e == "OnFighterAbilitySlotActivated"
    });
    assert_eq!(on[0], PyValue::Int(squadron));
    assert_eq!(on[1], PyValue::Int(0));
    assert_eq!(on[4], PyValue::Int(CYCLE_MS), "durationMs");
    // The clock runs the cycle: the combat-log line arrives.
    pump_until(&mut client, &mut seen, "the combat-log line", |e, _| {
        e == "OnDamageMessage"
    });
    {
        let w = world.read().expect("the world");
        let hurt = w
            .combat()
            .ship(rat)
            .and_then(|r| r.bars(eve_server::pyutil::filetime_now()).shield)
            .is_some_and(|s| s.fraction < 1.0);
        assert!(hurt || w.roster().get(rat).is_none(), "the rat took damage");
    }

    // Stop it: the slot is told, at the cycle's end.
    let answer = call(
        &mut client,
        &mut seen,
        "CmdDeactivateAbilitySlots",
        113,
        vec![ids(&[squadron]), PyValue::Int(0)],
    );
    assert_eq!(
        answer,
        PyValue::Dict(vec![(PyValue::None, PyValue::Int(squadron))])
    );
    let off = pump_until(&mut client, &mut seen, "the deactivation", |e, _| {
        e == "OnFighterAbilitySlotDeactivated"
    });
    assert_eq!(off[..2], [PyValue::Int(squadron), PyValue::Int(0)]);

    // Recall: `{fighterID: None}`; the clock lands it, refuels it, and the
    // tube is READY again.
    seen.clear();
    let answer = call(
        &mut client,
        &mut seen,
        "RecallFightersToTubes",
        114,
        vec![ids(&[squadron])],
    );
    assert_eq!(
        answer,
        PyValue::Dict(vec![(PyValue::None, PyValue::Int(squadron))])
    );
    pump_until(
        &mut client,
        &mut seen,
        "the tube READY",
        tube_status("READY"),
    );
    let w = world.read().expect("the world");
    assert!(w.roster().get(squadron).is_none(), "the ball is gone");
    let home = w.store().get(squadron).expect("the squadron's item");
    assert_eq!(
        (home.location_id, home.flag_id),
        (ALICE_SHIP, FIRST_TUBE),
        "it is back in its tube"
    );
    drop(w);
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
}

// ---------------------------------------------------------------------------
// FT-H: skills over the socket
// ---------------------------------------------------------------------------

/// Invented again: a Fighters-like skill that every fighter of the type needs
/// and whose bonus is the real shape (`OwnerRequiredSkillModifier`, level
/// times its bonus), and a fighter of its own.
const SKILL: i64 = 990_593_100;
const SKILLED_FIGHTER: i64 = 990_593_010;
const SKILLED_ABILITY: i64 = 990_793;
/// The skill's bonus to the fighter's damage multiplier, percent per level.
const BONUS_PCT: f64 = 5.0;

fn skilled_content() -> eve_server::content::Content {
    let atk = eve_server::fighters_data::effect::ATTACK;
    let rows = format!(
        "{}type\t{CARRIER}\t{GROUP_CARRIER}\t6\t1\t1\t50000.0\t400.0\t10000000.0\t200.0\t0\t\t\
         37=200.0;70=1.0;76=100000.0;2216=3.0;2217=2.0;2218=1.0;2055=100.0\tInvented Carrier\n\
         attribute\t280\t0.0\tskillLevel\n\
         attribute\t292\t0.0\tdamageMultiplierBonus\n\
         attribute\t2226\t0.0\tfighterAbilityAttackMissileDamageMultiplier\n\
         effect\t990593200\t0\t0\t0\t0\t0\t0\t0\tinventedLevelTimesBonus\t\n\
         effect\t990593201\t0\t0\t0\t0\t0\t0\t0\tinventedSkillBonusFighters\t\n\
         modifier\t990593200\titemID\tItemModifier\t292\t280\t0\t\t\n\
         modifier\t990593201\tcharID\tOwnerRequiredSkillModifier\t2226\t292\t6\t{SKILL}\t\n\
         type\t{SKILL}\t273\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t990593200,990593201\t292={BONUS_PCT}\tInvented Fighters Skill\n\
         type\t{SKILLED_FIGHTER}\t{GROUP_FIGHTER}\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t\
         37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;182={SKILL}.0;277=1.0;2212=1.0;2215={SQUADRON}.0;\
         2223=2000.0;2426={REFUEL_MS}.0;2226={MULT};2227={EM};2233={CYCLE_MS}.0;2236={RANGE}\tInvented Skilled Fighter\n\
         effect\t{atk}\t2\t1\t2233\t0\t2236\t2237\t0\tinventedFighterAttack\teffects.InventedFighterAttack\n",
        eve_server::typedata::FIXTURE
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the test rows parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let fighters = eve_server::fighters_data::Fighters::parse(
        &[format!(
            "{SKILLED_ABILITY}\titemTargeted\t0\t0\t{atk}\t990631\t\tInvented Attack"
        )],
        &[format!("{SKILLED_FIGHTER}\t0\t{SKILLED_ABILITY}\t\t\t")],
    )
    .expect("the fighter tables parse");
    c.insert("fighters_data", fighters);
    c
}

/// What a skilled-fighter launch came to.
#[derive(Debug, PartialEq)]
enum Flight {
    /// The launch answered this `UserError` key for the tube.
    Refused(String),
    /// The squadron flew; this is the `damage` of the first line of the
    /// owner's combat log against a rat.
    Damage(f64),
}

/// The key of a `UserError` marshalled as a value in a reply dict.
fn error_key(v: &PyValue) -> String {
    let PyValue::ObjectEx { header, .. } = v else {
        panic!("a UserError object, got {v:?}")
    };
    let PyValue::Tuple(h) = &**header else {
        panic!("the header is a tuple")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("the header holds the error's args")
    };
    let PyValue::Bytes(key) = &args[0] else {
        panic!("the key is a string")
    };
    String::from_utf8_lossy(key).into_owned()
}

type World = std::sync::Arc<std::sync::RwLock<eve_server::service::Registry>>;

/// A server, Alice connected and in a carrier with `fighter` stacked in her
/// bay, trained in the skill to `level` (`0`: not at all); not yet undocked.
/// The client, the world and the stack's item id.
fn boot(
    content: eve_server::content::Content,
    fighter: i64,
    level: i64,
) -> (MockClient, World, i64) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("literal"));
    config.mode = Mode::Gps;
    config.sandbox.characters = vec![eve_server::model::Character {
        character_id: ALICE,
        name: "Alice".to_owned(),
        ship_id: ALICE_SHIP,
        ..eve_server::model::Character::default()
    }];
    config.content = Some(std::sync::Arc::new(content));
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
    let bay_stack = {
        let mut w = world.write().expect("the world");
        let mut snap = w.to_snapshot();
        for i in snap.items.iter_mut().filter(|i| i.item_id == ALICE_SHIP) {
            i.type_id = CARRIER;
            i.group_id = GROUP_CARRIER;
        }
        snap.skills.retain(|(c, _)| *c != ALICE);
        if level > 0 {
            let mut skills = eve_server::skill::Skills::new();
            skills.grant_type(
                &eve_server::skill::SkillType {
                    type_id: SKILL,
                    name: "Invented Fighters Skill",
                    group_id: 273,
                    rank: 1,
                    primary: 166,
                    secondary: 167,
                },
                level,
            );
            snap.skills
                .push((ALICE, skills.entries().cloned().collect()));
        }
        w.apply_snapshot(snap);
        w.character_mut(ALICE).expect("Alice").ship_type_id = CARRIER;
        w.apply_items(&[eve_server::item::Edit::Create {
            type_id: fighter,
            quantity: SQUADRON,
            to: eve_server::item::Slot::new(ALICE, ALICE_SHIP, FLAG_BAY),
        }])
        .expect("the stack is made");
        w.store()
            .items()
            .iter()
            .find(|i| i.type_id == fighter && i.owner_id == ALICE)
            .expect("the stack")
            .item_id
    };
    (client, world, bay_stack)
}

/// Alice in a carrier with a stack of [`SKILLED_FIGHTER`] in her bay and
/// the Fighters-like skill trained to `level` (`0`: not at all); she loads
/// and launches it, and, when it flies, attacks a rat once.
fn skilled_flight(level: i64) -> Flight {
    let (mut client, world, bay_stack) = boot(skilled_content(), SKILLED_FIGHTER, level);
    let _beyonce = undock(&mut client, 100);
    let mut seen = Vec::new();
    // A pilot may carry what he cannot fly: the load is allowed.
    let answer = call(
        &mut client,
        &mut seen,
        "LoadFightersToTube",
        110,
        vec![PyValue::Int(bay_stack), PyValue::Int(FIRST_TUBE)],
    );
    assert_eq!(answer, PyValue::None);
    pump_until(&mut client, &mut seen, "READY", tube_status("READY"));
    seen.clear();
    let answer = call(
        &mut client,
        &mut seen,
        "LaunchFightersFromTubes",
        111,
        vec![ids(&[FIRST_TUBE])],
    );
    let PyValue::Dict(entries) = answer else {
        panic!("a dict answers a launch")
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1, PyValue::Int(FIRST_TUBE));
    if entries[0].0 != PyValue::None {
        return Flight::Refused(error_key(&entries[0].0));
    }
    let added = if let Some((_, a)) = seen.iter().find(|(e, _)| e == "OnFighterAddedToController") {
        a.clone()
    } else {
        pump_until(&mut client, &mut seen, "the controller event", |e, _| {
            e == "OnFighterAddedToController"
        })
    };
    let PyValue::Int(squadron) = added[0] else {
        panic!("the squadron's item id: {added:?}")
    };
    pump_until(&mut client, &mut seen, "INSPACE", tube_status("INSPACE"));
    let rat = {
        let mut w = world.write().expect("the world");
        let at = w.roster().get(squadron).expect("it flies").motion.position;
        let kind = eve_server::npc::find(990_310_001).expect("the fixture rat");
        eve_server::npc::spawn(&mut w, kind, SYSTEM, [at[0] + 3_000.0, at[1], at[2]])
            .expect("a rat")
    };
    seen.clear();
    let answer = call(
        &mut client,
        &mut seen,
        "CmdActivateAbilitySlots",
        112,
        vec![ids(&[squadron]), PyValue::Int(0), PyValue::Int(rat)],
    );
    assert_eq!(
        answer,
        PyValue::Dict(vec![(PyValue::None, PyValue::Int(squadron))])
    );
    let line = pump_until(&mut client, &mut seen, "the combat-log line", |e, _| {
        e == "OnDamageMessage"
    });
    let PyValue::Dict(d) = &line[0] else {
        panic!("the message is a dict")
    };
    let damage = d
        .iter()
        .find(|(_, k)| *k == bstr("damage"))
        .map(|(v, _)| v.clone())
        .expect("the log line names its damage");
    let PyValue::Real(damage) = damage else {
        panic!("the damage is a real: {damage:?}")
    };
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
    Flight::Damage(damage)
}

/// **The skills over the wire**: a pilot without the skill is refused the
/// launch with the client's own key, and a skilled pilot's squadron deals
/// more damage than a less skilled one's, per level.
#[test]
fn a_pilot_without_the_skill_is_refused_and_a_skilled_squadron_hits_harder() {
    assert_eq!(
        skilled_flight(0),
        Flight::Refused("FighterHasSkillPrerequisites".to_owned()),
        "no skill, no launch"
    );
    let (Flight::Damage(low), Flight::Damage(high)) = (skilled_flight(1), skilled_flight(5)) else {
        panic!("a trained pilot's squadron flies")
    };
    let volley = EM * MULT * SQUADRON as f64;
    let bonus = |level: f64| 1.0 + BONUS_PCT * level / 100.0;
    assert!(
        (low - volley * bonus(1.0)).abs() < 1e-6,
        "level 1: {low} against {}",
        volley * bonus(1.0)
    );
    assert!(
        (high - volley * bonus(5.0)).abs() < 1e-6,
        "level 5: {high} against {}",
        volley * bonus(5.0)
    );
    assert!(high > low);
}

/// **A squadron is abandoned and scooped back**, over the socket
/// (`fighterMgr.CmdAbandonFighter(fighterID)` and
/// `CmdScoopAbandonedFighterFromSpace(fighterID, toFlagID)`, both discarded by
/// the client): the controller event the client's tube handler empties the
/// tube on, a refusal of a scoop of a controlled squadron as a raised
/// `UserError`, and the squadron back in the fighter bay.
#[test]
fn an_abandoned_squadron_is_scooped_back_into_the_bay() {
    let (mut client, world, bay_stack) = boot(content(), FIGHTER, 0);
    let _beyonce = undock(&mut client, 100);
    let mut seen = Vec::new();
    call(
        &mut client,
        &mut seen,
        "LoadFightersToTube",
        110,
        vec![PyValue::Int(bay_stack), PyValue::Int(FIRST_TUBE)],
    );
    pump_until(&mut client, &mut seen, "READY", tube_status("READY"));
    seen.clear();
    call(
        &mut client,
        &mut seen,
        "LaunchFightersFromTubes",
        111,
        vec![ids(&[FIRST_TUBE])],
    );
    let added = if let Some((_, a)) = seen.iter().find(|(e, _)| e == "OnFighterAddedToController") {
        a.clone()
    } else {
        pump_until(&mut client, &mut seen, "the controller event", |e, _| {
            e == "OnFighterAddedToController"
        })
    };
    let PyValue::Int(squadron) = added[0] else {
        panic!("the squadron's item id: {added:?}")
    };
    pump_until(&mut client, &mut seen, "INSPACE", tube_status("INSPACE"));
    seen.clear();

    // A controlled squadron cannot be scooped: a raised UserError, which the
    // client's caller guards (`try/except UserError`).
    client.send(&call_req_args(
        "fighterMgr",
        "CmdScoopAbandonedFighterFromSpace",
        112,
        vec![PyValue::Int(squadron), PyValue::Int(FLAG_BAY)],
    ));
    let frame = client.recv();
    let PyValue::Instance { class, .. } = &frame else {
        panic!("a packet, got {frame:?}")
    };
    assert!(
        String::from_utf8_lossy(class).ends_with("ErrorResponse"),
        "a refusal is an error response, got {class:?}"
    );

    // Abandon it: the answer is discarded (`None`) and the controller event
    // comes, naming the tube.
    let answer = call(
        &mut client,
        &mut seen,
        "CmdAbandonFighter",
        113,
        vec![PyValue::Int(squadron)],
    );
    assert_eq!(answer, PyValue::None);
    let removed = if let Some((_, a)) = seen
        .iter()
        .find(|(e, _)| e == "OnFighterRemovedFromController")
    {
        a.clone()
    } else {
        pump_until(&mut client, &mut seen, "the controller leaving", |e, _| {
            e == "OnFighterRemovedFromController"
        })
    };
    assert_eq!(
        removed,
        vec![PyValue::Int(squadron), PyValue::Int(FIRST_TUBE)]
    );
    {
        let w = world.read().expect("the world");
        assert!(w.roster().get(squadron).is_some(), "the ball stays");
        let item = w.store().get(squadron).expect("the item stays");
        assert_eq!(item.location_id, SYSTEM, "still in space");
    }

    // Scoop it into the fighter bay.
    let answer = call(
        &mut client,
        &mut seen,
        "CmdScoopAbandonedFighterFromSpace",
        114,
        vec![PyValue::Int(squadron), PyValue::Int(FLAG_BAY)],
    );
    assert_eq!(answer, PyValue::None);
    let w = world.read().expect("the world");
    assert!(w.roster().get(squadron).is_none(), "the ball is gone");
    let item = w.store().get(squadron).expect("the squadron's item");
    assert_eq!(
        (item.location_id, item.flag_id, item.owner_id, item.quantity),
        (ALICE_SHIP, FLAG_BAY, ALICE, SQUADRON)
    );
    drop(w);
    client.stream.shutdown(Shutdown::Both).expect("shutdown");
}
