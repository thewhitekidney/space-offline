//! SOC-D4 over a real socket: in a claimed system, a laser cycle and an NPC
//! kill raise the industrial and military indices that
//! `devIndexManager.GetDevelopmentIndicesForSystem` and
//! `GetAllDevelopmentIndices` answer, the strategic index is there in
//! seconds, and the alliance's only system is its capital.
//!
//! **Invented rows only** (the no-client-data rule): the home system is an
//! invented null-sec row, so `/sov set` may claim it; the mining hull and
//! laser are `mining_hold.rs`'s invented ones, the gun is
//! `typedata::FIXTURE`'s autocannon with `killmail.rs`'s invented round, the
//! rat is the no-bundle `npc::FIXTURE_RAT`. Ids and names are invented.
//! **No `[A]` value is an expected value**: points only have to rise above
//! zero, never to a count the accrual constants chose.

use super::common::{ROCK, SYSTEM, VELDSPAR, config, gm, login, pilot, rock, serve, until};
use eve_bot::{Conn, Event, LockState};
use eve_marshal::PyValue;
use eve_server::celestial::Universe;
use eve_server::content::Content;
use eve_server::sov_index::{
    ATTR_DEV_INDEX_INDUSTRIAL, ATTR_DEV_INDEX_MILITARY, ATTR_DEV_INDEX_SOVEREIGNTY,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_461;
const BOB: i64 = 90_000_462;
/// Invented: the home system's region and constellation.
const REGION: i64 = 19_046_001;
const CONSTELLATION: i64 = 29_046_001;
/// An invented hull with a mining hold and an invented laser
/// (`mining_hold.rs`'s rows).
const DIGGER_HULL: i64 = 990_946_001;
const DIGGER_LASER: i64 = 990_946_002;
const LASER_CYCLE_MS: u64 = 5_000;
/// `typedata::FIXTURE`'s frigate and its autocannon, and an invented round
/// that kills a fixture rat in one volley.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
const ROUND: i64 = 990_946_003;
const RAT: i64 = eve_server::npc::FIXTURE_RAT.type_id;

/// The home system as an invented null-sec row with no faction: claimable.
fn universe() -> Universe {
    let system = format!(
        "{SYSTEM}\t{REGION}\t{CONSTELLATION}\t\t\t-0.5\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tFixture null home"
    );
    Universe::parse(
        &[system],
        &[format!("{REGION}\t\t\t\t\tFixture null region")],
        &[format!(
            "{CONSTELLATION}\t{REGION}\t\t\t\tFixture null constellation"
        )],
    )
    .expect("the invented rows parse")
}

fn content() -> Arc<Content> {
    let hull = format!(
        "type\t{DIGGER_HULL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
         9=310.0;11=45.0;12=4.0;13=3.0;14=3.0;37=355.0;48=125.0;55=150000.0;70=3.1;\
         76=21000.0;101=1.0;102=2.0;192=5.0;263=420.0;265=440.0;479=600000.0;\
         482=260.0;552=40.0;564=640.0;600=5.0;1137=3.0;\
         {}=1000.0\tFixture Digger Hull\n",
        eve_server::mining_hold::ATTR_GENERAL_MINING_HOLD_CAPACITY
    );
    let effect = "effect\t67\t2\t0\t73\t6\t54\t0\t0\tminingLaser\teffects.Mining\n";
    let laser = format!(
        "type\t{DIGGER_LASER}\t54\t7\t1\t1\t5.0\t0.0\t0.0\t25.0\t67\t12,16,67\t\
         6=1.0;30=1.0;50=1.0;54=15000.0;73={LASER_CYCLE_MS}.0;77=10.0\tFixture Digger Laser\n"
    );
    let round = format!(
        "type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n"
    );
    let text = format!(
        "{}{effect}{hull}{laser}{round}",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&text).expect("the test rows parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("universe", universe());
    Arc::new(c)
}

fn start() -> (SocketAddr, Arc<Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_461),
        pilot(BOB, "Bob", 100_000_462),
    ];
    config.sandbox.rocks =
        eve_server::asteroid::Field::from_rocks([rock(0, 100_000, [0.0, 0.0, 120_000.0])]);
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

/// A KeyVal's attribute (its state dict is value first).
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    state
        .dict_as_kv()?
        .into_iter()
        .find_map(|(k, val)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()).then_some(val))
}

/// One index's KeyVal from `GetDevelopmentIndicesForSystem`'s dict.
fn index(dict: &PyValue, attribute: i64) -> Option<&PyValue> {
    let PyValue::Dict(pairs) = dict else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| *k == PyValue::Int(attribute))
        .map(|(v, _)| v)
}

fn points(c: &mut Conn, attribute: i64) -> Option<i64> {
    let v = c
        .call(
            "devIndexManager",
            "GetDevelopmentIndicesForSystem",
            vec![PyValue::Int(SYSTEM)],
        )
        .expect("GetDevelopmentIndicesForSystem")
        .value()
        .expect("a value");
    match attr(index(&v, attribute)?, "points")? {
        PyValue::Int(n) => Some(*n),
        _ => None,
    }
}

/// Docked: create `type_id` and board it.
fn board(c: &mut Conn, type_id: i64) -> i64 {
    gm(c, &format!("/createitem {type_id}"));
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .expect("the hull is in the hangar");
    c.board(hull.item_id).expect("boards");
    hull.item_id
}

/// A dict's value by `str` key, for a slim.
fn slim_key<'a>(dict: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let dict = match dict {
        PyValue::Instance { state, .. } => state.as_ref(),
        other => other,
    };
    let PyValue::Dict(pairs) = dict else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| *k == PyValue::Bytes(name.as_bytes().to_vec()))
        .map(|(v, _)| v)
}

/// The rat ball an `AddBalls2` in this frame adds, if it adds one.
fn rat_added(frame: &PyValue) -> Option<i64> {
    let (event, args) = eve_bot::notification(frame)?;
    if event != "DoDestinyUpdate" {
        return None;
    }
    eve_bot::read_action_list(&args)
        .into_iter()
        .filter(|(_, name, _)| name == "AddBalls2")
        .find_map(|(_, _, operands)| {
            let PyValue::Tuple(state) = operands.first()? else {
                return None;
            };
            let PyValue::List(extra) = state.get(1)? else {
                return None;
            };
            extra.iter().find_map(|e| {
                let slim = match e {
                    PyValue::Tuple(pair) => pair.first()?,
                    other => other,
                };
                (slim_key(slim, "typeID") == Some(&PyValue::Int(RAT)))
                    .then(|| match slim_key(slim, "itemID") {
                        Some(PyValue::Int(id)) => Some(*id),
                        _ => None,
                    })
                    .flatten()
            })
        })
}

/// **The packet's socket test.** Alice founds a corp and an alliance and
/// claims the (invented null-sec) home; she mines one laser cycle there
/// and Bob shoots a rat dead there. The indices the client reads rise.
#[test]
fn kills_and_mining_in_a_claimed_system_raise_the_indices_dev_index_manager_returns() {
    let (addr, content) = start();
    let mut alice = login(addr, "Alice");

    // The alliance and its claim.
    gm(&mut alice, "/giveisk 2000000000");
    let a = corp_call(
        &mut alice,
        "AddCorporation",
        vec![uni("Index Invented"), uni("IDXI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    // CORP-W: the alliance fee comes from the corp's Master Wallet.
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(&mut alice, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Invented Indices"), uni("IIDX"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    let before = alice
        .call(
            "devIndexManager",
            "GetDevelopmentIndicesForSystem",
            vec![PyValue::Int(SYSTEM)],
        )
        .expect("answered")
        .value();
    assert_eq!(before, Some(PyValue::Dict(vec![])), "unclaimed: no indices");
    gm(&mut alice, &format!("/sov set {SYSTEM} {alliance}"));
    assert!(
        points(&mut alice, ATTR_DEV_INDEX_SOVEREIGNTY).is_some(),
        "a claimed system carries the strategic index"
    );
    assert_eq!(points(&mut alice, ATTR_DEV_INDEX_MILITARY), Some(0));
    assert_eq!(points(&mut alice, ATTR_DEV_INDEX_INDUSTRIAL), Some(0));

    // The alliance's only system is its capital at once.
    let bind = PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]);
    let cap = alice
        .moniker_call(
            "allianceRegistry",
            bind.clone(),
            "GetCapitalSystemInfo",
            vec![],
            vec![],
        )
        .expect("GetCapitalSystemInfo")
        .value
        .expect("a value");
    assert_eq!(
        attr(&cap, "currentCapitalSystem"),
        Some(&PyValue::Int(SYSTEM))
    );
    // A system the alliance does not hold cannot be the capital.
    let refused = alice
        .moniker_call(
            "allianceRegistry",
            bind,
            "SetCapitalSystem",
            vec![PyValue::Int(SYSTEM + 1)],
            vec![],
        )
        .expect("answered");
    assert!(refused.refusal().is_some(), "an unheld capital is refused");
    let kept = alice
        .moniker_call(
            "allianceRegistry",
            PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]),
            "SetCapitalSystem",
            vec![PyValue::Int(SYSTEM)],
            vec![],
        )
        .expect("answered");
    assert_eq!(kept.refusal(), None, "the CEO may name a held capital");

    // Industrial: one laser cycle on the home rock.
    gm(&mut alice, &format!("/createitem {DIGGER_LASER}"));
    let hull = board(&mut alice, DIGGER_HULL);
    let laser = alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == DIGGER_LASER)
        .expect("the laser")
        .item_id;
    let laser = alice.fit_auto(&[laser]).expect("fits")[0].item_id;
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");
    alice.follow(ROCK, 1_000.0).expect("follows");
    alice.request_state().expect("a fresh state");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut state = alice.lock_target(ROCK).expect("AddTarget is answered");
    while state == LockState::Pending && Instant::now() < deadline {
        let pushes = alice.pump(Duration::from_millis(500)).expect("reads");
        if pushes.iter().any(|e| {
            matches!(e, Event::Push(n, a)
                if n == "OnTarget"
                    && a.first() == Some(&PyValue::Bytes(b"add".to_vec()))
                    && a.get(1) == Some(&PyValue::Int(ROCK)))
        }) {
            state = LockState::Locked;
        }
    }
    assert_eq!(state, LockState::Locked);
    alice.mine(laser, ROCK).expect("the laser starts");
    let mined = until(
        Duration::from_millis(LASER_CYCLE_MS) + Duration::from_secs(15),
        || {
            let _ = alice.collect_pushes(Duration::from_millis(100));
            points(&mut alice, ATTR_DEV_INDEX_INDUSTRIAL).is_some_and(|p| p > 0)
        },
    );
    assert!(mined, "a laser cycle raises the industrial index");
    let ore: i64 = alice
        .ship_items(hull, None)
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == VELDSPAR)
        .map(|r| r.quantity)
        .sum();
    assert!(ore > 0, "the cycle cut ore");

    // Military: Bob kills a rat in one volley.
    let mut bob = login(addr, "Bob");
    board(&mut bob, FRIGATE);
    gm(&mut bob, &format!("/createitem {AUTOCANNON}"));
    gm(&mut bob, &format!("/createitem {ROUND} 10"));
    let station = bob.session().station_id().expect("docked");
    let spare: Vec<i64> = bob
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == AUTOCANNON)
        .map(|r| r.item_id)
        .collect();
    let gun = bob
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !bob.is_online(gun.item_id).expect("asks") {
        bob.set_online(gun.item_id).expect("onlines");
    }
    let rounds: Vec<i64> = bob
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == ROUND)
        .map(|r| r.item_id)
        .collect();
    bob.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");
    let mut frames = bob
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(
                format!("/entity deploy 1 {RAT}").into_bytes(),
            )],
        )
        .expect("the slash is answered")
        .notifications;
    frames.extend(
        bob.collect_pushes(Duration::from_millis(500))
            .unwrap_or_default(),
    );
    let rat = frames
        .iter()
        .find_map(rat_added)
        .expect("the deploy adds the rat's ball");
    bob.lock_target(rat).expect("locks the rat");
    bob.fire(gun.item_id, AUTOCANNON, Some(&content), rat)
        .expect("fires");
    let killed = until(Duration::from_secs(30), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        points(&mut bob, ATTR_DEV_INDEX_MILITARY).is_some_and(|p| p > 0)
    });
    assert!(killed, "the rat's death raises the military index");

    // The galaxy-wide read lists the system with both indices.
    let all = bob
        .call("devIndexManager", "GetAllDevelopmentIndices", vec![])
        .expect("GetAllDevelopmentIndices")
        .value()
        .expect("a value");
    let PyValue::List(rows) = &all else {
        panic!("a list: {all:?}")
    };
    let row = rows
        .iter()
        .find(|r| attr(r, "solarSystemID") == Some(&PyValue::Int(SYSTEM)))
        .expect("the claimed system is listed");
    for key in ["militaryPoints", "industrialPoints"] {
        assert!(
            matches!(attr(row, key), Some(PyValue::Int(n)) if *n > 0),
            "{key}: {row:?}"
        );
    }
    assert!(matches!(attr(row, "claimedFor"), Some(PyValue::Int(_))));
}
