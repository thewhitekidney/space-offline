//! NOTIF-1 over a real socket: one pilot shoots another's ship dead, and
//! the victim's notification store (`notificationMgr`) holds a kill report
//! notification that was pushed to him, that he can list, read and delete;
//! the killer holds the final blow's.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): `typedata::FIXTURE`'s
//! frigate and autocannon and an invented round, as `killmail.rs` uses
//! them. The notification types and groups are the server's named
//! constants; nothing here is an `[A]` value.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::notifications::{group, kind};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_111;
const BOB: i64 = 90_000_112;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// An invented kinetic round heavy enough to kill a fixture frigate with
/// one volley (charge group 83, size 1).
const ROUND: i64 = 990_630_002;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Notifier Round\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_111),
        pilot(BOB, "Bob", 100_000_112),
    ];
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

/// Docked: create a fixture frigate and board it.
fn board_frigate(c: &mut Conn) -> i64 {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = c
        .hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE)
        .expect("the frigate is in the hangar");
    c.board(hull.item_id).expect("boards the frigate");
    hull.item_id
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

/// One key of a marshal dict (stored value first).
fn key<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Dict(pairs) = v else {
        return None;
    };
    pairs
        .iter()
        .find_map(|(val, k)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()).then_some(val))
}

fn ask(c: &mut Conn, method: &str, args: Vec<PyValue>) -> Option<PyValue> {
    c.call("notificationMgr", method, args)
        .expect("answered")
        .value()
}

/// A list answer's rows.
fn rows(v: Option<PyValue>) -> Vec<PyValue> {
    let Some(PyValue::List(rows)) = v else {
        panic!("a list: {v:?}")
    };
    rows
}

/// `GetAllNotifications(fromID=0)`, the notification centre's call, with
/// its argument as the kwarg the client sends.
fn all(c: &mut Conn) -> Vec<PyValue> {
    rows(
        c.call_kw(
            "notificationMgr",
            "GetAllNotifications",
            vec![],
            vec![(PyValue::Int(0), PyValue::Bytes(b"fromID".to_vec()))],
        )
        .expect("answered")
        .value(),
    )
}

/// Alice kills Bob's frigate with one volley (`killmail.rs`'s fight).
fn fight(addr: SocketAddr, content: &eve_server::content::Content) -> (Conn, Conn, Vec<PyValue>) {
    let mut bob = login(addr, "Bob");
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");

    let mut alice = login(addr, "Alice");
    board_frigate(&mut alice);
    gm(&mut alice, &format!("/createitem {AUTOCANNON}"));
    gm(&mut alice, &format!("/createitem {ROUND} 10"));
    let station = alice.session().station_id().expect("docked");
    let spare: Vec<i64> = alice
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == AUTOCANNON)
        .map(|r| r.item_id)
        .collect();
    let gun = alice
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !alice.is_online(gun.item_id).expect("asks") {
        alice.set_online(gun.item_id).expect("onlines");
    }
    let rounds: Vec<i64> = alice
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == ROUND)
        .map(|r| r.item_id)
        .collect();
    alice
        .load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, ROUND, station)
        .expect("loads");
    alice.undock().expect("Alice undocks");
    alice.enter_space(SYSTEM).expect("Alice's state");

    alice.lock_target(bob_ship).expect("locks Bob's ship");
    alice
        .fire(gun.item_id, AUTOCANNON, Some(content), bob_ship)
        .expect("fires");

    // Everything Bob is pushed while his ship dies and a moment after: the
    // notification goes out on the clock, not with any answer.
    let mut pushes = Vec::new();
    let mut dead = false;
    let told = until(Duration::from_secs(30), || {
        pushes.extend(
            bob.collect_pushes(Duration::from_millis(100))
                .unwrap_or_default(),
        );
        dead = dead || bob.session().ship_id() != Some(bob_ship);
        dead && pushes
            .iter()
            .any(|p| eve_bot::notification(p).is_some_and(|(e, _)| e == "OnNotificationReceived"))
    });
    assert!(dead, "Bob's ship dies");
    assert!(told, "Bob is pushed OnNotificationReceived");
    (alice, bob, pushes)
}

/// **The kill report notification, end to end.** Bob was pushed
/// `OnNotificationReceived(id, 140, sender, created, data)`; the same
/// notification is in his `GetAllNotifications`, `GetUnprocessed` and the
/// war group, and its `killMailID` opens the kill report. Marking it read
/// moves it out of the unread list and sets `processed`; deleting it empties
/// the store. Alice holds the final blow's notification naming Bob, and
/// Bob's delete leaves hers alone.
#[test]
fn a_kill_report_notification_is_listed_read_and_deleted() {
    let (addr, content) = start();
    let (mut alice, mut bob, pushes) = fight(addr, &content);

    let (_, args) = pushes
        .iter()
        .filter_map(eve_bot::notification)
        .find(|(e, _)| e == "OnNotificationReceived")
        .expect("pushed");
    assert_eq!(args.len(), 5, "{args:?}");
    assert_eq!(args[1], PyValue::Int(kind::KILL_REPORT_VICTIM));
    let id = args[0].clone();
    let kill_id = key(&args[4], "killMailID").cloned().expect("killMailID");
    assert!(key(&args[4], "killMailHash").is_some());

    let listed = all(&mut bob);
    assert_eq!(listed.len(), 1, "Bob's: {listed:?}");
    let n = &listed[0];
    assert_eq!(attr(n, "notificationID"), Some(&id));
    assert_eq!(
        attr(n, "typeID"),
        Some(&PyValue::Int(kind::KILL_REPORT_VICTIM))
    );
    assert_eq!(attr(n, "receiverID"), Some(&PyValue::Int(BOB)));
    assert_eq!(attr(n, "processed"), Some(&PyValue::Bool(false)));
    assert_eq!(
        attr(n, "data").and_then(|d| key(d, "killMailID")),
        Some(&kill_id)
    );
    assert_eq!(rows(ask(&mut bob, "GetUnprocessed", vec![])).len(), 1);
    assert_eq!(
        rows(ask(
            &mut bob,
            "GetByGroupID",
            vec![PyValue::Int(group::WAR)]
        ))
        .len(),
        1
    );

    // The notification centre opens the report from the notification's own
    // two values.
    let hash = attr(n, "data")
        .and_then(|d| key(d, "killMailHash"))
        .cloned()
        .expect("a hash");
    let report = bob
        .call(
            "warStatisticMgr",
            "GetKillMail",
            vec![kill_id.clone(), hash],
        )
        .expect("answered")
        .value()
        .expect("a report");
    assert_eq!(attr(&report, "killID"), Some(&kill_id));
    assert_eq!(attr(&report, "victimCharacterID"), Some(&PyValue::Int(BOB)));

    // The killer's: the final blow, naming the victim.
    let hers = all(&mut alice);
    assert_eq!(hers.len(), 1, "Alice's: {hers:?}");
    assert_eq!(
        attr(&hers[0], "typeID"),
        Some(&PyValue::Int(kind::KILL_REPORT_FINAL_BLOW))
    );
    assert_eq!(
        attr(&hers[0], "data").and_then(|d| key(d, "victimID")),
        Some(&PyValue::Int(BOB))
    );

    // Read.
    ask(
        &mut bob,
        "MarkAsProcessed",
        vec![PyValue::List(vec![id.clone()])],
    );
    assert!(rows(ask(&mut bob, "GetUnprocessed", vec![])).is_empty());
    assert_eq!(
        attr(&all(&mut bob)[0], "processed"),
        Some(&PyValue::Bool(true))
    );

    // Delete: Bob's goes, Alice's stays.
    ask(
        &mut bob,
        "DeleteNotifications",
        vec![PyValue::List(vec![id])],
    );
    assert!(all(&mut bob).is_empty());
    assert!(
        rows(ask(
            &mut bob,
            "GetByGroupID",
            vec![PyValue::Int(group::WAR)]
        ))
        .is_empty()
    );
    assert_eq!(all(&mut alice).len(), 1);
}
