//! SOC-C6 over a real socket: one pilot shoots another's ship dead, and
//! both character sheets' combat logs (`charMgr.GetRecentShipKillsAndLosses`)
//! list the kill report; the kill report link (`warStatisticMgr.
//! GetKillMail`) opens it.
//!
//! The same limitation as every test in this crate: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.
//!
//! **Invented rows only** (the no-client-data rule): `typedata::FIXTURE`'s
//! frigate and autocannon, and an invented round that kills a fixture
//! frigate in one volley. Alice and Bob are plain connections; nothing here
//! is an `[A]` value.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_101;
const BOB: i64 = 90_000_102;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// An invented round for the fixture autocannon (charge group 83, size 1),
/// kinetic (the fixture frigate's shield is immune to EM) and heavy enough
/// to kill a fixture frigate with one volley.
const ROUND: i64 = 990_630_001;
/// REL-FIX: a second invented round, one unit of kinetic damage, so a
/// first shooter hurts the ship without killing it.
const WEAK_ROUND: i64 = 990_630_002;
const CAROL: i64 = 90_000_103;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n\
         type\t{WEAK_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1.0;128=1.0\tFixture Weak Round\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    Arc::new(content)
}

/// An invented player corporation id (outside the NPC range).
const PLAYER_CORP: i64 = 98_000_551;

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    // REVIEW-FIX: the corp lists answer a player corporation only, so the
    // three share an invented one (the default is an NPC corporation).
    let in_corp = |mut p: eve_server::model::Character| {
        p.corporation_id = PLAYER_CORP;
        p
    };
    config.sandbox.characters = vec![
        in_corp(pilot(ALICE, "Alice", 100_000_101)),
        in_corp(pilot(BOB, "Bob", 100_000_102)),
        in_corp(pilot(CAROL, "Carol", 100_000_103)),
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

/// The caller's combat log, as `(killID, victim, final blow)`.
fn combat_log(c: &mut Conn) -> Vec<(PyValue, PyValue, PyValue)> {
    let answer = c
        .call(
            "charMgr",
            "GetRecentShipKillsAndLosses",
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = answer else {
        panic!("a list: {answer:?}")
    };
    rows.iter()
        .map(|r| {
            let get = |n: &str| attr(r, n).cloned().unwrap_or_else(|| panic!("no {n}"));
            (
                get("killID"),
                get("victimCharacterID"),
                get("finalCharacterID"),
            )
        })
        .collect()
}

/// Docked: board a fixture frigate, fit and load the autocannon with
/// `round`, undock into space. Answers the fitted gun.
fn arm(c: &mut Conn, round: i64) -> eve_bot::ItemRow {
    board_frigate(c);
    gm(c, &format!("/createitem {AUTOCANNON}"));
    gm(c, &format!("/createitem {round} 10"));
    let station = c.session().station_id().expect("docked");
    let spare: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == AUTOCANNON)
        .map(|r| r.item_id)
        .collect();
    let gun = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == AUTOCANNON)
        .expect("the autocannon is fitted");
    if !c.is_online(gun.item_id).expect("asks") {
        c.set_online(gun.item_id).expect("onlines");
    }
    let rounds: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == round)
        .map(|r| r.item_id)
        .collect();
    c.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, round, station)
        .expect("loads");
    c.undock().expect("Alice undocks");
    c.enter_space(SYSTEM).expect("Alice's state");
    gun
}

/// Bob undocks in a fixture frigate; Alice undocks in one with the
/// autocannon and the invented round, locks Bob's ship and fires until it
/// is gone. Answers both connections and the dead ship's id.
fn fight() -> (Conn, Conn, i64) {
    fight_with(false)
}

/// [`fight`]; with `first_hit` Carol hurts Bob's ship with the weak round
/// first, so Alice's kill has a second attacker on the report.
fn fight_with(first_hit: bool) -> (Conn, Conn, i64) {
    let (addr, content) = start();

    let mut bob = login(addr, "Bob");
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");

    let _carol = first_hit.then(|| {
        let mut c = login(addr, "Carol");
        let gun = arm(&mut c, WEAK_ROUND);
        c.lock_target(bob_ship).expect("Carol locks");
        c.fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
            .expect("Carol fires");
        // One cycle later her volley is booked and Alice's will kill.
        std::thread::sleep(Duration::from_secs(5));
        c
    });
    let mut alice = login(addr, "Alice");
    let gun = arm(&mut alice, ROUND);

    alice.lock_target(bob_ship).expect("locks Bob's ship");
    alice
        .fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
        .expect("fires");

    // 90 s: the run takes about 6 s alone (HANG-1: enter_space had been
    // waiting out Carol's whole magazine, 50 to 270 s).
    let dead = until(Duration::from_secs(90), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        bob.session().ship_id() != Some(bob_ship)
    });
    assert!(dead, "Bob's ship dies");
    (alice, bob, bob_ship)
}

/// **A kill is in both pilots' combat logs.** Bob undocks in a fixture
/// frigate; Alice undocks in one with the autocannon and the invented
/// round, locks Bob's ship and fires. When the ship is gone, Bob's log (his
/// loss) and Alice's (her kill) both hold one report naming Bob as the
/// victim and Alice as the final blow, and the report link opens it.
/// **REL-FIX: the kill report lists every attacker.** Carol's weak volley
/// lands first, Alice's kills: the row's `attackers` holds the final blow
/// first and Carol second, and `finalCharacterID` is still Alice.
#[test]
fn the_kill_report_lists_every_attacker() {
    let (mut alice, _bob, _) = fight_with(true);
    let rows = alice
        .call(
            "charMgr",
            "GetRecentShipKillsAndLosses",
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = rows else {
        panic!("a list")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        attr(&rows[0], "finalCharacterID"),
        Some(&PyValue::Int(ALICE))
    );
    let Some(PyValue::List(att)) = attr(&rows[0], "attackers") else {
        panic!("attackers: {:?}", attr(&rows[0], "attackers"))
    };
    let who: Vec<_> = att
        .iter()
        .map(|a| attr(a, "characterID").cloned().expect("characterID"))
        .collect();
    assert_eq!(
        who,
        vec![PyValue::Int(ALICE), PyValue::Int(CAROL)],
        "{att:?}"
    );
}

#[test]
fn a_kill_shows_in_the_victims_and_the_killers_combat_logs() {
    let (mut alice, mut bob, _) = fight();
    let lost = combat_log(&mut bob);
    assert_eq!(lost.len(), 1, "Bob's log: {lost:?}");
    assert_eq!(lost[0].1, PyValue::Int(BOB));
    assert_eq!(lost[0].2, PyValue::Int(ALICE));
    let killed = combat_log(&mut alice);
    assert_eq!(killed, lost, "Alice's log holds the same report");

    // SEC-FIX 6: the link carries the client's own hash of the report
    // (`GetKillReportHashValue`), computed here from the row's fields; a
    // wrong one opens nothing.
    let rows = alice
        .call(
            "charMgr",
            "GetRecentShipKillsAndLosses",
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = rows else {
        panic!("a list")
    };
    let int = |n: &str| match attr(&rows[0], n) {
        Some(PyValue::Int(i)) => *i,
        other => panic!("{n}: {other:?}"),
    };
    let hash = eve_server::notifications::kill_hash_of(
        int("victimCharacterID"),
        Some(int("finalCharacterID")),
        int("victimShipTypeID"),
        int("killTime"),
    );
    let refused = alice
        .call(
            "warStatisticMgr",
            "GetKillMail",
            vec![lost[0].0.clone(), PyValue::Bytes(b"hash".to_vec())],
        )
        .expect("answered")
        .value();
    assert_eq!(refused, Some(PyValue::None), "a wrong hash opens nothing");
    let one = alice
        .call(
            "warStatisticMgr",
            "GetKillMail",
            vec![lost[0].0.clone(), PyValue::Bytes(hash.into_bytes())],
        )
        .expect("answered")
        .value()
        .expect("a report");
    assert_eq!(attr(&one, "killID"), Some(&lost[0].0));
    assert_eq!(attr(&one, "victimShipTypeID"), Some(&PyValue::Int(FRIGATE)));
    assert_eq!(
        attr(&one, "finalWeaponTypeID"),
        Some(&PyValue::Int(AUTOCANNON))
    );
}

/// A page of a corporation's list: `(killID, victim, final blow)` rows.
fn corp_list(c: &mut Conn, method: &str) -> Vec<(PyValue, PyValue, PyValue)> {
    let answer = c
        .call(
            "corpRegistry",
            method,
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = answer else {
        panic!("a list: {answer:?}")
    };
    rows.iter()
        .map(|r| {
            let get = |n: &str| attr(r, n).cloned().unwrap_or_else(|| panic!("no {n}"));
            (
                get("killID"),
                get("victimCharacterID"),
                get("finalCharacterID"),
            )
        })
        .collect()
}

/// **C6B: the kill is in the corporation's list and tells both sides.**
/// Alice and Bob are in one corporation (the default), so after Alice kills
/// Bob's frigate both see the report in the corporation window's Kills
/// (`corpRegistry.GetRecentKills`) and Losses (`GetRecentLosses`) tabs, the
/// same report their own combat logs hold; and each was told by a stored
/// notification (the victim's and the final blow's kill report types), whose
/// id and hash open the report.
#[test]
fn a_kill_is_in_the_corp_kill_list_and_notifies_both_sides() {
    use eve_server::notifications::kind;
    let (mut alice, mut bob, _) = fight();
    let own = combat_log(&mut bob);
    assert_eq!(own.len(), 1);
    for who in [&mut alice, &mut bob] {
        assert_eq!(corp_list(who, "GetRecentKills"), own, "corp kills");
        assert_eq!(corp_list(who, "GetRecentLosses"), own, "corp losses");
    }

    let unprocessed = |c: &mut Conn| -> Vec<PyValue> {
        let Some(PyValue::List(rows)) = c
            .call("notificationMgr", "GetUnprocessed", vec![])
            .expect("answered")
            .value()
        else {
            panic!("a list")
        };
        rows
    };
    let type_of = |r: &PyValue| match attr(r, "typeID") {
        Some(PyValue::Int(t)) => *t,
        other => panic!("typeID: {other:?}"),
    };
    let victim_told = unprocessed(&mut bob);
    let killer_told = unprocessed(&mut alice);
    assert_eq!(
        victim_told.iter().map(&type_of).collect::<Vec<_>>(),
        vec![kind::KILL_REPORT_VICTIM]
    );
    assert_eq!(
        killer_told.iter().map(&type_of).collect::<Vec<_>>(),
        vec![kind::KILL_REPORT_FINAL_BLOW]
    );
}
