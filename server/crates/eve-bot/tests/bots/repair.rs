//! GAP-REPAIR-BOT over a real socket: docking no longer mends a hull, so a
//! bot docked with damage pays the station's repair shop, from its own
//! wallet, with journal ref 15.
//!
//! The damage is put in the world file the server starts from (a stash row,
//! as a dock leaves it) on the pilot's capsule, because the harness has no
//! way to hurt a ship and keep it alive. Invented hull price only.

use super::common::{config, login, pilot, serve};
use eve_marshal::PyValue;
use eve_server::Store;
use eve_server::repair::{COST_MULTIPLIER, Hurt};
use std::sync::Arc;

const BOB: i64 = 90_000_131;
const CAPSULE: i64 = 100_000_131;
const CAPSULE_TYPE: i64 = 670;
/// An invented `P_ref` for the capsule.
const CAPSULE_REF: f64 = 400_000.0;
const WALLET: f64 = 5_000_000.0;

/// How many quote rows the shop gives for the capsule.
fn quote_rows(c: &mut eve_bot::Conn, station: i64) -> usize {
    let params = PyValue::Tuple(vec![
        PyValue::Int(station),
        PyValue::Int(station),
        PyValue::Int(eve_bot::GROUP_STATION),
    ]);
    let answer = c
        .moniker_call(
            "repairSvc",
            params,
            "GetRepairQuotes",
            vec![PyValue::List(vec![PyValue::Int(CAPSULE)])],
            vec![],
        )
        .expect("answered");
    match answer.value {
        Some(PyValue::Dict(d)) => match d.first() {
            Some((PyValue::List(rows), _)) => rows.len(),
            other => panic!("rows: {other:?}"),
        },
        other => panic!("a dict: {other:?}"),
    }
}

#[test]
fn a_damaged_bot_pays_the_repair_shop_at_the_dock() {
    let mut config = config();
    config.sandbox.characters = vec![eve_server::model::Character {
        balance: WALLET,
        ship_type_id: CAPSULE_TYPE,
        ..pilot(BOB, "Bob", CAPSULE)
    }];
    let mut content = eve_server::content::Content::fixture();
    content.insert(
        "econ_prices",
        eve_server::econ_prices::Table::parse(&[format!(
            "{CAPSULE_TYPE}\t{CAPSULE_REF}\tbase\t{CAPSULE_REF}\t6\t99010\t\t1"
        )])
        .expect("parses"),
    );
    let content = Arc::new(content);
    config.content = Some(content.clone());

    // The world the server starts from: the capsule hurt, as a dock left it.
    let mut reg = eve_server::service::Registry::new(config.sandbox.clone()).with_content(content);
    eve_server::repair::put(
        &mut reg,
        CAPSULE,
        Hurt {
            armor: 30.0,
            structure: 20.0,
        },
    );
    let dir = std::env::temp_dir().join(format!("eve-repair-bot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let station = reg
        .character(BOB)
        .and_then(|c| c.station_id)
        .expect("docked");
    let lines = eve_server::repair::lines_for(&reg, BOB, station, CAPSULE);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        (lines[0].unit_cost * lines[0].max_health - COST_MULTIPLIER * CAPSULE_REF).abs() < 1e-6,
        "a full repair costs the multiplier of the price"
    );
    let due = lines[0].cost();
    assert!(due > 0.0);
    let path = dir.join("world.dat");
    eve_server::FileStore::new(&path)
        .save(&reg.to_snapshot())
        .expect("saves the world");
    config.world_file = Some(path);

    let addr = serve(config);
    let mut bob = login(addr, "Bob");
    let station = bob.session().station_id().expect("docked");
    assert_eq!(quote_rows(&mut bob, station), 1, "one hurt hull");
    let wallet = bob.cash_balance().expect("a wallet");

    let note = bob.repair_ship().expect("the repair shop answers");
    assert!(note.contains("repaired"), "{note}");
    assert_eq!(
        bob.cash_balance().expect("a wallet"),
        wallet - due,
        "exactly the quote left the wallet"
    );
    assert_eq!(quote_rows(&mut bob, station), 0, "whole now");
    assert!(
        bob.repair_ship().expect("answers").is_empty(),
        "a whole ship costs nothing and says nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-Act role path (a courier's or a traveller's `undock_repaired`)
/// repairs too, with one note, and the undock still happens.
#[test]
fn a_role_undock_repairs_first() {
    let mut config = config();
    config.sandbox.characters = vec![eve_server::model::Character {
        balance: WALLET,
        ship_type_id: CAPSULE_TYPE,
        ..pilot(BOB, "Bob", CAPSULE)
    }];
    let mut content = eve_server::content::Content::fixture();
    content.insert(
        "econ_prices",
        eve_server::econ_prices::Table::parse(&[format!(
            "{CAPSULE_TYPE}\t{CAPSULE_REF}\tbase\t{CAPSULE_REF}\t6\t99010\t\t1"
        )])
        .expect("parses"),
    );
    let content = Arc::new(content);
    config.content = Some(content.clone());
    let mut reg = eve_server::service::Registry::new(config.sandbox.clone()).with_content(content);
    eve_server::repair::put(
        &mut reg,
        CAPSULE,
        Hurt {
            armor: 30.0,
            structure: 20.0,
        },
    );
    let dir = std::env::temp_dir().join(format!("eve-repair-role-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let path = dir.join("world.dat");
    eve_server::FileStore::new(&path)
        .save(&reg.to_snapshot())
        .expect("saves the world");
    config.world_file = Some(path);

    let addr = serve(config);
    let mut bob = login(addr, "Bob");
    let station = bob.session().station_id().expect("docked");
    assert_eq!(quote_rows(&mut bob, station), 1, "one hurt hull");
    let wallet = bob.cash_balance().expect("a wallet");
    let (outcome, note) = bob.repair_then_undock().expect("undocks");
    assert!(outcome.session_changed, "undocked");
    assert!(note.contains("repaired"), "{note}");
    assert!(bob.cash_balance().expect("a wallet") < wallet, "it paid");
    let _ = std::fs::remove_dir_all(&dir);
}
