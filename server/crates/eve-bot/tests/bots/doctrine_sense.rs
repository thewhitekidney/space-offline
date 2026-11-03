//! **DOC-3 U1 over a real socket: does the victim's `OnDamageMessage` carry
//! the damage-type split?** (R-DOCTRINE section 11, U1.) It does now: the
//! victim's line holds `damageAttributes` (`{114/116/117/118: amount}`) and
//! the `weapon` type, both optional keys of the client's own reader [V]
//! (`uilog.py` `_ReadIconsFromMessageDict`, `AddCombatMessageFromDict`).
//! Before this lane the server sent neither, so a bot could not tell what
//! was hitting it.
//!
//! Alice shoots Bob's fixture frigate with an invented round that is
//! all kinetic; Bob (a plain connection) keeps his pushes, reads the combat
//! log line with `doctrine_sense::parse_damage` and counts it in a
//! `DamageTally`. Invented rows only; no `[A]` value is asserted.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::doctrine_sense::{DamageTally, TYPE_NAMES, parse_damage};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_201;
const BOB: i64 = 90_000_202;
/// `typedata::FIXTURE`'s frigate and its autocannon.
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
/// An invented round for the fixture autocannon: one unit of kinetic damage,
/// so a single volley hurts and does not kill.
const WEAK_ROUND: i64 = 990_630_002;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{WEAK_ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1.0;128=1.0\tFixture Weak Round\n",
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
        pilot(ALICE, "Alice", 100_000_201),
        pilot(BOB, "Bob", 100_000_202),
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

#[test]
fn damage_message_carries_types_socket() {
    let (addr, content) = start();
    let mut bob = login(addr, "Bob");
    bob.keep_pushes();
    let bob_ship = board_frigate(&mut bob);
    bob.undock().expect("Bob undocks");
    bob.enter_space(SYSTEM).expect("Bob's state");

    let mut alice = login(addr, "Alice");
    let gun = arm(&mut alice, WEAK_ROUND);
    alice.lock_target(bob_ship).expect("locks Bob's ship");
    alice
        .fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
        .expect("fires");

    let mut lines: Vec<Vec<PyValue>> = Vec::new();
    let got = until(Duration::from_secs(60), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        for (name, args) in bob.take_pushes() {
            if name == "OnDamageMessage" {
                lines.push(args);
            }
        }
        lines
            .iter()
            .any(|a| parse_damage(a).is_some_and(|h| h.incoming && h.quality > 0))
    });
    assert!(got, "Bob is hit and reads the combat log line");

    let hit = lines
        .iter()
        .filter_map(|a| parse_damage(a))
        .find(|h| h.incoming && h.quality > 0)
        .expect("a landed hit");
    let split = hit
        .by_type
        .expect("the victim's line carries damageAttributes");
    assert!(split[2] > 0.0, "kinetic: {split:?}");
    assert_eq!(
        (split[0], split[1], split[3]),
        (0.0, 0.0, 0.0),
        "nothing else: {split:?}"
    );
    assert_eq!(hit.weapon, Some(AUTOCANNON), "the weapon type is named");
    assert!(hit.attacker.is_some(), "and the attacker's ball");

    let mut tally = DamageTally::default();
    tally.record(&hit);
    assert_eq!(
        tally.dominant().map(|i| TYPE_NAMES[i]),
        Some("kinetic"),
        "{tally:?}"
    );
}
