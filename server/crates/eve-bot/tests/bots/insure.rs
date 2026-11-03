//! X-INSURE over a real socket: a pilot's hull is insured by the bot's own
//! undock check, the hull is shot dead by another pilot, and the payout
//! arrives in the wallet.
//!
//! The ship is lost to a pilot, not an NPC: the harness has no NPC that
//! kills a fixture frigate in one volley, and `service::insurance`'s unit
//! test `an_npc_kill_pays_the_owner` covers the NPC killer and the journal
//! row (ref 19, the hull's type as reference). Here the over-the-socket half
//! is the thing: the premium leaves the wallet at the undock, the contract
//! is readable, and `fraction x full` comes back at the loss.
//!
//! Invented rows only: `typedata::FIXTURE`'s frigate and autocannon, an
//! invented round and an invented hull price.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::insure::{self, PACKAGES, premium};
use eve_marshal::PyValue;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_121;
const BOB: i64 = 90_000_122;
const FRIGATE: i64 = 990_000_001;
const AUTOCANNON: i64 = 990_000_002;
const ROUND: i64 = 990_630_002;
/// An invented `P_ref` for the frigate.
const FRIGATE_REF: f64 = 200_000.0;
/// Rich enough for any package within the premium share.
const WALLET: f64 = 10_000_000.0;

fn content() -> Arc<eve_server::content::Content> {
    let rows = format!(
        "{}type\t{ROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Insure Round\n",
        eve_server::typedata::FIXTURE
    );
    let mut content = eve_server::content::Content::fixture();
    let table = eve_server::typedata::Table::parse(&rows).expect("the type rows parse");
    content.insert("typedata", eve_server::typedata::Installed::build(table));
    let mut prices = eve_server::econ_prices::Table::fixture();
    prices.prices.extend(
        eve_server::econ_prices::Table::parse(&[format!(
            "{FRIGATE}\t{FRIGATE_REF}\tbase\t{FRIGATE_REF}\t6\t99010\t\t1"
        )])
        .expect("parses")
        .prices,
    );
    content.insert("econ_prices", prices);
    Arc::new(content)
}

fn start() -> (SocketAddr, Arc<eve_server::content::Content>) {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_121),
        eve_server::model::Character {
            balance: WALLET,
            ..pilot(BOB, "Bob", 100_000_122)
        },
    ];
    let content = content();
    config.content = Some(content.clone());
    (serve(config), content)
}

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

fn contract_of(c: &mut Conn, ship: i64) -> Option<PyValue> {
    match c
        .call(
            "insuranceSvc",
            "GetContractForShip",
            vec![PyValue::Int(ship)],
        )
        .expect("answered")
        .value()
    {
        Some(PyValue::None) | None => None,
        other => other,
    }
}

/// **Insured at the undock, paid at the loss.**
#[test]
fn a_bots_insured_hull_dies_and_the_payout_arrives() {
    let (addr, content) = start();
    let level = PACKAGES[3];
    let mut bob = login(addr, "Bob");
    let bob_ship = board_frigate(&mut bob);
    let start_wallet = bob.cash_balance().expect("a wallet");
    assert!(contract_of(&mut bob, bob_ship).is_none());

    let note = insure::cover_with(&mut bob, level, |t| t == FRIGATE).expect("covers");
    assert!(note.contains("insured"), "{note}");
    assert!(contract_of(&mut bob, bob_ship).is_some(), "a contract runs");
    let quoted = match bob
        .call(
            "insuranceSvc",
            "GetInsurancePrice",
            vec![PyValue::Int(FRIGATE)],
        )
        .expect("answered")
        .value()
    {
        Some(PyValue::Real(r)) => r,
        other => panic!("a price: {other:?}"),
    };
    assert!(quoted > 0.0 && quoted <= FRIGATE_REF);
    let paid = premium(quoted, level);
    let after_premium = bob.cash_balance().expect("a wallet");
    assert_eq!(
        after_premium,
        start_wallet - paid,
        "the premium left the wallet"
    );
    // Covered now: a second check buys nothing.
    let again = insure::cover_with(&mut bob, level, |t| t == FRIGATE).expect("covers");
    assert!(again.is_empty(), "{again}");
    assert_eq!(bob.cash_balance().expect("a wallet"), after_premium);

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
        .fire(gun.item_id, AUTOCANNON, Some(&content), bob_ship)
        .expect("fires");

    let expected = after_premium + level * quoted;
    let mut dead = false;
    let paid_out = until(Duration::from_secs(30), || {
        let _ = bob.collect_pushes(Duration::from_millis(100));
        dead = dead || bob.session().ship_id() != Some(bob_ship);
        dead && bob.cash_balance().is_ok_and(|w| w == expected)
    });
    assert!(dead, "Bob's ship dies");
    assert!(
        paid_out,
        "the payout of fraction x full arrives: wallet {:?}, expected {expected}",
        bob.cash_balance()
    );
    assert!(
        contract_of(&mut bob, bob_ship).is_none(),
        "the contract ended with the ship"
    );
}
