//! C8 over a real socket: one corp declares war on another, the war is
//! pending and then hostile, both sides are pushed `OnWarChanged`, the
//! notices arrive, and `war::at_war` -- the question crimewatch and CONCORD
//! ask -- flips with it. The second test ends a war by surrender.
//!
//! The calls are the client's own (`docs/lanes/c8-a.md` §2): `warRegistry`
//! through its `(ownerID, 1)` moniker, `corpRegistry` through its corp
//! moniker, `structureDirectory` unbound. Names and ids are invented.
//!
//! **The spool-up cannot be waited out here.** `--time-scale` stops at 60,
//! which still makes a 24-hour spool-up 24 minutes; so the war runs at scale
//! 60 (the test reads the scaled 24 minutes off the wire) and the GM call
//! `GMSetDeclareTime` -- the client's own menu item -- moves the declaration
//! back to start it, as the owner would in a soak. The same limitation as
//! every test here: both ends marshal with `eve-marshal`.

use super::common::{config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::game_clock::GameClock;
use eve_server::server::Server;
use eve_server::service::Registry;
use eve_server::war;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_811;
const BOB: i64 = 90_000_812;
/// The scale the war runs at: the biggest the server allows.
const SCALE: u32 = 60;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// A server at `SCALE`, and a handle on its world to ask `at_war` of.
fn start() -> (SocketAddr, Arc<RwLock<Registry>>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_811),
        pilot(BOB, "Bob", 100_000_812),
    ];
    config.time_scale = GameClock::new(SCALE).expect("a legal scale");
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

/// One attribute of a `utillib.KeyVal` (its state dict is value first).
fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = v else {
        return None;
    };
    state
        .dict_as_kv()?
        .into_iter()
        .find_map(|(k, val)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()).then_some(val))
}

fn int(v: Option<&PyValue>) -> i64 {
    match v {
        Some(PyValue::Int(n)) => *n,
        other => panic!("not an int: {other:?}"),
    }
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

/// `warRegistry`, bound on `(ownerID, 1)` as `eveMoniker.GetWar` does.
fn war_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let owner = c.session().get_i64("corpid").expect("a corp");
    let bind = PyValue::Tuple(vec![PyValue::Int(owner), PyValue::Int(1)]);
    c.moniker_call("warRegistry", bind, method, args, vec![])
        .unwrap_or_else(|e| panic!("warRegistry.{method}: {e}"))
}

fn found_corp(c: &mut Conn, name: &str, ticker: &str) -> i64 {
    gm(c, "/giveisk 2000000000");
    let a = corp_call(
        c,
        "AddCorporation",
        vec![uni(name), uni(ticker), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp");
    // CORP-W: a war is paid from the corp's Master Wallet.
    let d = crate::corp_wallet::deposit(c, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    corp
}

/// The wars `c`'s corp is in, `GetWars`' dict as `(warID, war)` pairs.
fn wars(c: &mut Conn, owner: i64) -> Vec<(i64, PyValue)> {
    let a = war_call(c, "GetWars", vec![PyValue::Int(owner)]);
    assert_eq!(a.refusal(), None);
    let v = a.value.expect("a dict");
    v.dict_as_kv()
        .expect("GetWars answers a dict: the client calls .itervalues()")
        .into_iter()
        .map(|(k, w)| (int(Some(k)), w.clone()))
        .collect()
}

/// Everything pushed to `c` within a moment, as `(event, args)`.
fn pushes(c: &mut Conn) -> Vec<(String, Vec<PyValue>)> {
    c.collect_pushes(Duration::from_millis(300))
        .unwrap_or_default()
        .iter()
        .filter_map(eve_bot::notification)
        .collect()
}

#[test]
fn a_corp_declares_the_war_spools_up_and_both_sides_hear_it() {
    let (addr, world) = start();
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let a = found_corp(&mut alice, "Alpha Invented", "ALPH");
    let b = found_corp(&mut bob, "Bravo Invented", "BRAV");
    let _ = pushes(&mut bob);

    // The wizard's war HQ, then the declaration.
    let list = alice
        .call(
            "structureDirectory",
            "GetValidWarHQs",
            vec![PyValue::Int(a)],
        )
        .expect("the HQ list")
        .value()
        .expect("a value");
    let PyValue::List(hqs) = list else {
        panic!("a list")
    };
    assert_eq!(hqs.len(), 1, "one war HQ stand-in");
    assert_eq!(int(attr(&hqs[0], "typeID")), war::TYPE_WAR_HQ);
    let hq = int(attr(&hqs[0], "structureID"));
    let r = corp_call(
        &mut alice,
        "DeclareWarAgainst",
        vec![PyValue::Int(b), PyValue::Int(hq)],
    );
    assert_eq!(r.refusal(), None, "DeclareWarAgainst refused");

    // Pending: the wire carries the scaled 24-hour spool-up, and nobody is at
    // war yet.
    let ours = wars(&mut alice, a);
    assert_eq!(ours.len(), 1);
    let (war_id, w) = &ours[0];
    assert_eq!(int(attr(w, "declaredByID")), a);
    assert_eq!(int(attr(w, "againstID")), b);
    assert_eq!(int(attr(w, "warHQ")), hq);
    let (declared, started) = (int(attr(w, "timeDeclared")), int(attr(w, "timeStarted")));
    assert_eq!(started - declared, war::WAR_SPOOLUP / i64::from(SCALE));
    assert_eq!(attr(w, "timeFinished"), Some(&PyValue::None));
    assert_eq!(wars(&mut bob, b).len(), 1, "the defender sees it too");
    assert!(!war::at_war(&world.read().expect("world"), a, b));

    // Bob is pushed the war without asking, and is told by notification.
    let mut told = Vec::new();
    let heard = until(Duration::from_secs(10), || {
        told.extend(pushes(&mut bob));
        told.iter().any(|(e, _)| e == "OnWarChanged")
            && told.iter().any(|(e, args)| {
                e == "OnNotificationReceived"
                    && args.get(1)
                        == Some(&PyValue::Int(eve_server::war_notice::kind::WAR_DECLARED))
            })
    });
    assert!(
        heard,
        "Bob is pushed OnWarChanged and the declaration: {told:?}"
    );
    let (_, args) = told
        .iter()
        .find(|(e, _)| e == "OnWarChanged")
        .expect("the push");
    assert_eq!(args.len(), 3, "war, ownerIDs, change");
    // A Python set: the client does `ownerIDs & {corpid, allianceid}`.
    let PyValue::ObjectEx { header, .. } = &args[1] else {
        panic!("ownerIDs is a set: {:?}", args[1]);
    };
    let PyValue::Tuple(parts) = &**header else {
        panic!("a set call: {header:?}");
    };
    assert_eq!(parts[0], PyValue::Global(b"__builtin__.set".to_vec()));
    assert_eq!(
        parts[1],
        PyValue::Tuple(vec![PyValue::List(vec![PyValue::Int(a), PyValue::Int(b)])])
    );
    assert_eq!(int(attr(&args[0], "warID")), *war_id);

    // Alice moves the declaration back (the client's GM menu item): the war
    // is hostile, both are pushed again, and `at_war` says so both ways.
    let two_spoolups = 2 * war::WAR_SPOOLUP;
    let past = eve_server::pyutil::filetime_now() - two_spoolups;
    let r = war_call(
        &mut alice,
        "GMSetDeclareTime",
        vec![PyValue::Int(*war_id), PyValue::Int(past)],
    );
    assert_eq!(r.refusal(), None, "GMSetDeclareTime refused");
    {
        let reg = world.read().expect("world");
        assert!(war::at_war(&reg, a, b));
        assert!(war::at_war(&reg, b, a));
    }
    let again = until(Duration::from_secs(10), || {
        pushes(&mut bob).iter().any(|(e, _)| e == "OnWarChanged")
    });
    assert!(again, "Bob is pushed the change");
    let w = &wars(&mut bob, b)[0].1;
    assert!(int(attr(w, "timeStarted")) < eve_server::pyutil::filetime_now());
}

#[test]
fn a_surrender_over_the_socket_ends_the_war_and_forbids_a_new_one() {
    let (addr, world) = start();
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let a = found_corp(&mut alice, "Alpha Invented", "ALPH");
    let b = found_corp(&mut bob, "Bravo Invented", "BRAV");
    let hq = {
        let PyValue::List(h) = alice
            .call(
                "structureDirectory",
                "GetValidWarHQs",
                vec![PyValue::Int(a)],
            )
            .expect("hq")
            .value()
            .expect("value")
        else {
            panic!("list")
        };
        int(attr(&h[0], "structureID"))
    };
    let r = corp_call(
        &mut alice,
        "DeclareWarAgainst",
        vec![PyValue::Int(b), PyValue::Int(hq)],
    );
    assert_eq!(r.refusal(), None);
    let war_id = wars(&mut alice, a)[0].0;
    // The war starts (the GM menu's "Set Declare Time").
    let past = eve_server::pyutil::filetime_now() - 2 * war::WAR_SPOOLUP;
    let r = war_call(
        &mut alice,
        "GMSetDeclareTime",
        vec![PyValue::Int(war_id), PyValue::Int(past)],
    );
    assert_eq!(r.refusal(), None);
    assert!(war::at_war(&world.read().expect("world"), a, b));
    // Declaring again is refused with the client's key.
    let r = corp_call(
        &mut alice,
        "DeclareWarAgainst",
        vec![PyValue::Int(b), PyValue::Int(hq)],
    );
    assert_eq!(r.refusal().as_deref(), Some("AlreadyAtWarWithThem"));

    // Bob offers to surrender for ISK; Alice sees the offer and accepts.
    let r = war_call(
        &mut bob,
        "CreateSurrenderNegotiation",
        vec![
            PyValue::Int(war_id),
            PyValue::Real(1_000_000.0),
            uni("we give up"),
        ],
    );
    assert_eq!(r.refusal(), None, "CreateSurrenderNegotiation refused");
    let offers = war_call(&mut alice, "GetNegotiations", vec![])
        .value
        .expect("a list");
    let PyValue::List(offers) = offers else {
        panic!("list")
    };
    assert_eq!(offers.len(), 1);
    assert_eq!(int(attr(&offers[0], "warNegotiationTypeID")), 2);
    assert_eq!(int(attr(&offers[0], "ownerID1")), b);
    let neg = int(attr(&offers[0], "warNegotiationID"));
    let r = war_call(&mut alice, "AcceptSurrender", vec![PyValue::Int(neg)]);
    assert_eq!(r.refusal(), None, "AcceptSurrender refused");

    // The war is retracted into its cooldown, with the surrender as the
    // reason, and the treaty lists for both.
    let w = &wars(&mut alice, a)[0].1;
    assert_eq!(int(attr(w, "reasonEnded")), war::ended::SURRENDER);
    assert!(attr(w, "retracted").is_some_and(|v| *v != PyValue::None));
    assert!(attr(w, "timeFinished").is_some_and(|v| *v != PyValue::None));
    let t = bob
        .call("peaceTreatyMgr", "GetPeaceTreatiesForSession", vec![])
        .expect("treaties")
        .value()
        .expect("value");
    let PyValue::Tuple(sides) = t else {
        panic!("a 2-tuple")
    };
    assert_eq!(sides.len(), 2);
    for side in &sides {
        let PyValue::List(l) = side else {
            panic!("list")
        };
        assert_eq!(l.len(), 1);
        assert_eq!(int(attr(&l[0], "otherOwnerID")), a);
    }
    // Still war during the cooldown (the client's own rule).
    assert!(war::at_war(&world.read().expect("world"), a, b));
}
