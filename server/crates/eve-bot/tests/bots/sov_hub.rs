//! SOC-D2 over a real socket: a CEO who founded an alliance launches a
//! Sovereignty Hub from her cargo ("Launch for Corporation", `ship.Drop`),
//! the hub is a ball in her park, `/sov capture` makes the claim the way
//! D3a's entosis will, the claim names the **real** hub id everywhere D1
//! answers it, `/boostsov` (the players' line, through the ungated
//! `publicQaToolsServer`) fuels it, the burn eats the fuel, and an empty
//! bay takes the hub offline.
//!
//! **Invented furniture only**: the home system is made null-sec here, the
//! star, the fuel block and every number on the hub's type row are
//! invented; the hub's type id and the client's group ids appear by name.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use eve_server::sov_hub;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_621;
const FUEL_TYPE: i64 = 990_621_001;
const HUB_RADIUS: f64 = 10_000.0;
/// Where the hub will be: inside launch range of the undock.
const HUB_X: f64 = 400_000.0;
const MERLIN: i64 = 603;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// Whether `v` holds the byte string `needle` anywhere.
fn holds(v: &PyValue, needle: &[u8]) -> bool {
    match v {
        PyValue::Bytes(b) => b.windows(needle.len()).any(|w| w == needle),
        PyValue::List(l) | PyValue::Tuple(l) => l.iter().any(|x| holds(x, needle)),
        PyValue::Dict(d) => d.iter().any(|(a, b)| holds(a, needle) || holds(b, needle)),
        PyValue::Instance { state, .. } => holds(state, needle),
        _ => false,
    }
}

fn said(v: Option<PyValue>) -> String {
    match v {
        Some(PyValue::Bytes(b)) => String::from_utf8_lossy(&b).into_owned(),
        other => format!("{other:?}"),
    }
}

fn slash(c: &mut Conn, line: &str) -> String {
    let out = c
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(line.as_bytes().to_vec())],
        )
        .expect("the slash is answered");
    said(out.value())
}

/// The fixture's tables, with the home system null-sec and no faction, a
/// hub row and a fuel block.
fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t1.0\t0.0\t1000.0\t{HUB_RADIUS}\t0\t\t\tInvented Hub Row\n\
         type\t{FUEL_TYPE}\t{}\t4\t1\t40\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Fuel Block\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        sov_hub::GROUP_FUEL_BLOCK,
    );
    let table = eve_server::typedata::Table::parse(&rows).expect("the rows parse");
    c.insert("typedata", eve_server::typedata::Installed::build(table));
    let u = eve_server::celestial::Universe::parse(
        &[format!(
            "{SYSTEM}\t10000001\t20000001\t\t\t-0.4\t\t0\t0\t0\t0\t0\t0\t\t0\t\t\tInvented Null Home"
        )],
        &["10000001\t\t\t\t\tInvented Region".to_owned()],
        &["20000001\t10000001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the universe parses");
    c.insert("universe", u);
    c
}

/// A star placed so the hub lands at `(HUB_X, 0, 0)`.
fn star_row() -> String {
    let radius = 3.3e8;
    let at = sov_hub::hub_position([0.0; 3], radius);
    format!(
        "star\t40621001\t990621010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
        HUB_X - at[0],
        -at[1],
        -at[2]
    )
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

fn hangar_item(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar lists")
        .into_iter()
        .find(|r| r.type_id == type_id)
        .unwrap_or_else(|| panic!("a {type_id} in the hangar"))
        .item_id
}

#[test]
fn a_hub_is_launched_captured_fuelled_and_goes_offline_when_the_fuel_is_gone() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_621)];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&star_row());
    config.content = Some(Arc::new(content()));
    let addr = serve(config);
    let mut alice = login(addr, "Alice");

    // A corp and an alliance, Alice the CEO of both.
    gm(&mut alice, "/giveisk 2000000000");
    let a = corp_call(
        &mut alice,
        "AddCorporation",
        vec![uni("Hub Invented"), uni("HUBI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    // CORP-W: the alliance fee comes from the corp's Master Wallet.
    let corp = alice.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(&mut alice, corp, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Invented Hub Pact"), uni("IHPC"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    // A ship, a hub and a day's fuel and one block more in its hold.
    let blocks = sov_hub::FUEL_PER_DAY + 1;
    gm(&mut alice, "/createitem merlin");
    let merlin = hangar_item(&mut alice, MERLIN);
    alice.board(merlin).expect("boards");
    gm(
        &mut alice,
        &format!("/createitem {}", eve_server::sov::TYPE_INFRASTRUCTURE_HUB),
    );
    let mut left = blocks;
    while left > 0 {
        let n = left.min(1_000);
        gm(&mut alice, &format!("/createitem {FUEL_TYPE} {n}"));
        left -= n;
    }
    let hub_stack = hangar_item(&mut alice, eve_server::sov::TYPE_INFRASTRUCTURE_HUB);
    alice.load_cargo(hub_stack, None).expect("the hub loads");
    let fuel: Vec<i64> = alice
        .hangar_items()
        .expect("lists")
        .into_iter()
        .filter(|r| r.type_id == FUEL_TYPE)
        .map(|r| r.item_id)
        .collect();
    for id in fuel {
        alice.load_cargo(id, None).expect("the fuel loads");
    }

    // Into space, within launch range of where the hub goes.
    alice.undock().expect("undocks");
    alice.enter_space(SYSTEM).expect("a state");

    // Launch for Corporation.
    let params = alice.session().location_moniker().expect("in a system");
    let items = PyValue::List(vec![PyValue::Tuple(vec![
        PyValue::Int(hub_stack),
        PyValue::Int(1),
    ])]);
    let answer = alice
        .moniker_call(
            "ship",
            params,
            "Drop",
            vec![items, PyValue::None, PyValue::Bool(false)],
            vec![],
        )
        .expect("Drop is answered");
    let value = answer.value.clone().expect("a value");
    let PyValue::Dict(pairs) = &value else {
        panic!("a dict: {value:?}")
    };
    let hub_id = match pairs.first() {
        Some((PyValue::List(ids), PyValue::Int(src))) if *src == hub_stack => match ids.first() {
            Some(PyValue::Int(id)) => *id,
            other => panic!("refused: {other:?}"),
        },
        other => panic!("keyed by the source item: {other:?}"),
    };
    // The ball, pushed to the park right after the answer.
    let mut added = answer.outcome.notifications.iter().any(|f| {
        eve_bot::notification(f).is_some_and(|(n, a)| {
            n == "DoDestinyUpdate" && a.iter().any(|x| holds(x, b"AddBalls2"))
        })
    });
    let arrived = until(Duration::from_secs(10), || {
        if !added {
            for e in alice.pump(Duration::from_millis(200)).unwrap_or_default() {
                if let Event::Push(n, a) = e
                    && n == "DoDestinyUpdate"
                    && a.iter().any(|x| holds(x, b"AddBalls2"))
                {
                    added = true;
                }
            }
        }
        added
    });
    assert!(arrived, "the hub's AddBalls2 never arrived");
    // A fresh state has the hub where the spec puts it.
    let fresh = alice.request_state().expect("a state");
    let park = fresh
        .notifications
        .iter()
        .filter_map(eve_bot::notification)
        .filter(|(n, _)| n == "DoDestinyUpdate")
        .find_map(|(_, a)| eve_bot::read_set_state(&a))
        .expect("a SetState");
    let ball = park
        .park
        .balls
        .iter()
        .find(|b| b.ball_id == hub_id)
        .expect("the hub is in the park");
    assert!(
        (ball.position[0] - HUB_X).abs() < 1.0,
        "{:?}",
        ball.position
    );

    // Neutral until captured: no claim.
    let none = alice
        .call(
            "sovMgr",
            "GetSystemSovereigntyInfo",
            vec![PyValue::Int(SYSTEM)],
        )
        .expect("answered");
    assert_eq!(none.value(), Some(PyValue::None));

    // The GM capture: the claim names the real hub.
    let out = slash(&mut alice, &format!("/sov capture here {alliance}"));
    assert!(out.contains("captured"), "{out}");
    let info = alice
        .call(
            "sovMgr",
            "GetSystemSovereigntyInfo",
            vec![PyValue::Int(SYSTEM)],
        )
        .expect("answered")
        .value()
        .expect("a value");
    let text = format!("{info:?}");
    assert!(text.contains(&hub_id.to_string()), "{text}");
    assert_ne!(hub_id, eve_server::sov::stand_in_hub_id(SYSTEM));
    let bind = PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]);
    let rows = alice
        .moniker_call(
            "allianceRegistry",
            bind,
            "GetAllianceSovereigntyStructuresInfo",
            vec![],
            vec![],
        )
        .expect("answered")
        .value
        .expect("a value");
    let PyValue::Tuple(parts) = &rows else {
        panic!("{rows:?}")
    };
    for (i, what) in [(0, "TCU"), (1, "hub")] {
        assert!(
            format!("{:?}", parts[i]).contains(&hub_id.to_string()),
            "the {what} row carries the real hub id: {:?}",
            parts[i]
        );
    }

    // Warp to the hub and fuel it the players' way: `/boostsov` is
    // refused in warp and out of range, and moves the blocks on arrival.
    alice.warp_to_item(hub_id, 0.0).expect("warps");
    let want = format!("{blocks} fuel blocks");
    let mut status = String::new();
    let fuelled = until(Duration::from_secs(90), || {
        let _ = alice.pump(Duration::from_millis(200));
        alice
            .call(
                "publicQaToolsServer",
                "SlashCmd",
                vec![PyValue::Bytes(b"/boostsov all".to_vec())],
            )
            .expect("answered");
        status = slash(&mut alice, "/sovfuel");
        status.contains(&want)
    });
    assert!(fuelled && status.contains("(claimed)"), "{status}");

    // The burn: a day is paid, the next is not and the hub goes offline,
    // its claim still held.
    let out = slash(&mut alice, "/sovfuel burn 1");
    assert!(out.contains("1 fuel blocks"), "{out}");
    let out = slash(&mut alice, "/sovfuel burn 1");
    assert!(
        out.contains("(offline)") && out.contains("0 fuel blocks"),
        "{out}"
    );
    let info = slash(&mut alice, "/sov info here");
    assert!(
        info.contains("offline") && info.contains(&alliance.to_string()),
        "{info}"
    );
}
