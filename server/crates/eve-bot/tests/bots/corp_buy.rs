//! CORP-BUY over a real socket: **a buy placed with `useCorp` spends the
//! corporation's wallet, appears in the corporation's order list and fills
//! into the corporation's deliveries hangar; a member without the role is
//! refused** (docs/lanes/corp-buy.md).
//!
//! Ann is the CEO, Bob the corporation's trader (granted the accountant and
//! trader roles through `UpdateMember`, as the CEO's bot does), Cy a member
//! with no role, Dan a seller outside the corporation. Invented fixture ids
//! only (`typedata::FIXTURE`).

use super::common::{config, gm, login, pilot};
use eve_bot::Conn;
use eve_bot::builder::SellLine;
use eve_marshal::PyValue;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;
use eve_server::server::Server;
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

const ANN: i64 = 90_000_821;
const BOB: i64 = 90_000_822;
const CY: i64 = 90_000_823;
const DAN: i64 = 90_000_824;
/// An invented round from the fixture type table.
const ROUND: i64 = 990_000_005;
/// The accountant and trader bits the client checks (`appConst`).
const BUY_ROLES: i64 = cw::ROLE_ACCOUNTANT | cw::ROLE_TRADER;
const MASTER: f64 = 5_000_000.0;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c.session().get_i64("corpid").expect("a corp");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

fn first_application_id(v: &PyValue) -> Option<i64> {
    let PyValue::Dict(entries) = v else {
        return None;
    };
    let (PyValue::List(apps), _) = entries.first()? else {
        return None;
    };
    let PyValue::Instance { state, .. } = apps.first()? else {
        return None;
    };
    let PyValue::Dict(attrs) = state.as_ref() else {
        return None;
    };
    attrs.iter().find_map(|(value, key)| match (key, value) {
        (PyValue::Bytes(k), PyValue::Int(id)) if k == b"applicationID" => Some(*id),
        _ => None,
    })
}

fn join(ceo: &mut Conn, who: &mut Conn, id: i64, corp: i64) {
    let a = corp_call(ceo, "InsertInvitation", vec![PyValue::Int(id)]);
    assert_eq!(a.refusal(), None, "InsertInvitation refused");
    let mine = corp_call(who, "GetMyApplications", vec![]);
    let app = mine
        .value
        .as_ref()
        .and_then(first_application_id)
        .expect("the invitation");
    let a = corp_call(
        who,
        "UpdateApplicationOffer",
        vec![
            PyValue::Int(app),
            PyValue::Int(id),
            PyValue::Int(corp),
            uni(""),
            PyValue::Int(2),
        ],
    );
    assert_eq!(a.refusal(), None, "accepting refused");
}

fn own_balance(c: &mut Conn) -> f64 {
    match c
        .call("account", "GetCashBalance", vec![PyValue::Int(0)])
        .expect("GetCashBalance(0)")
        .value()
    {
        Some(PyValue::Real(r)) => r,
        other => panic!("a float, got {other:?}"),
    }
}

/// `PlaceBuyOrder` as the buy window sends it, with the box as given.
fn place(c: &mut Conn, station: i64, price: f64, qty: i64, use_corp: bool) -> eve_bot::CallOutcome {
    c.call(
        "marketProxy",
        "PlaceBuyOrder",
        vec![
            PyValue::Int(station),
            PyValue::Int(ROUND),
            PyValue::Real(price),
            PyValue::Int(qty),
            PyValue::Int(eve_bot::builder::RANGE_STATION),
            PyValue::Int(1),
            PyValue::Int(1),
            PyValue::Bool(use_corp),
            PyValue::Real(0.0),
        ],
    )
    .expect("answered")
}

#[test]
fn corp_buy_socket() {
    let mut config = config();
    let rich = |id: i64, name: &str, ship: i64| {
        let mut p = pilot(id, name, ship);
        p.balance = 5_000_000.0;
        p
    };
    config.sandbox.characters = vec![
        rich(ANN, "Ann", 100_000_821),
        rich(BOB, "Bob", 100_000_822),
        rich(CY, "Cy", 100_000_823),
        rich(DAN, "Dan", 100_000_824),
    ];
    config.content = Some(Arc::new(eve_server::content::Content::fixture()));
    let server = Server::bind(config).expect("bind");
    let addr: SocketAddr = server.local_addr().expect("local addr");
    let world: Arc<RwLock<Registry>> = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    let mut ann = login(addr, "Ann");
    let mut bob = login(addr, "Bob");
    let mut cy = login(addr, "Cy");
    let mut dan = login(addr, "Dan");
    gm(&mut ann, "/giveisk 2000000000");
    let a = corp_call(
        &mut ann,
        "AddCorporation",
        vec![uni("Buy Invented"), uni("BUYI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session");
    join(&mut ann, &mut bob, BOB, corp);
    join(&mut ann, &mut cy, CY, corp);
    let station = ann.session().get_i64("stationid").expect("docked");
    {
        let mut w = world.write().expect("the world lock");
        cw::adjust(w.deltas_mut(), corp, cw::MASTER_KEY, cw::to_cents(MASTER)).expect("funded");
    }
    let master = |w: &Arc<RwLock<Registry>>| {
        let w = w.read().expect("the world lock");
        cw::to_isk(cw::balance(w.deltas(), corp, cw::MASTER_KEY))
    };

    // The CEO grants Bob the trader's roles, the way her bot does.
    let mut args = vec![
        PyValue::Int(BOB),
        PyValue::None,
        PyValue::None,
        PyValue::None,
        PyValue::Int(BUY_ROLES),
    ];
    args.extend(std::iter::repeat_n(PyValue::None, 10));
    let a = corp_call(&mut ann, "UpdateMember", args);
    assert_eq!(a.refusal(), None, "the grant refused");

    // 1. Cy holds no role: the box is refused, nothing moves, no order rests.
    let (cy_before, master_before) = (own_balance(&mut cy), master(&world));
    let refused = place(&mut cy, station, 10.0, 5, true);
    assert_eq!(refused.refusal().as_deref(), Some("CrpAccessDenied"));
    assert_eq!(own_balance(&mut cy), cy_before);
    assert_eq!(master(&world), master_before);
    assert!(cy.my_open_orders().expect("orders").is_empty());

    // 2. Bob's useCorp bid escrows from the Master Wallet, not from Bob.
    let bob_before = own_balance(&mut bob);
    let placed = place(&mut bob, station, 10.0, 5, true);
    assert_eq!(placed.refusal(), None, "the corp bid is accepted");
    assert_eq!(own_balance(&mut bob), bob_before, "Bob paid nothing");
    let held = master(&world);
    assert!(
        held <= master_before - 50.0,
        "the escrow left the division: {held} vs {master_before}"
    );
    // The orders are the corporation's, not Bob's own.
    assert!(bob.my_open_orders().expect("orders").is_empty());
    let theirs = bob.corp_open_orders().expect("corp orders");
    assert_eq!(theirs.len(), 1, "the Corporation Orders list: {theirs:?}");
    assert!(theirs[0].order.bid && theirs[0].order.type_id == ROUND);
    // The CEO's tab shows it too; Cy, with no role, sees nothing.
    assert_eq!(ann.corp_open_orders().expect("corp orders").len(), 1);
    assert!(cy.corp_open_orders().expect("corp orders").is_empty());

    // 3. Dan sells into it: the goods go to the corporation's deliveries.
    gm(&mut dan, &format!("/createitem {ROUND} 5"));
    let stack = dan
        .hangar_items()
        .expect("the hangar")
        .into_iter()
        .find(|r| r.type_id == ROUND)
        .expect("Dan holds the round");
    dan.sell(
        &[SellLine {
            station_id: station,
            type_id: ROUND,
            item_id: stack.item_id,
            price: 10.0,
            quantity: 5,
        }],
        0,
        0.0,
    )
    .expect("the sale fills");
    {
        let w = world.read().expect("the world lock");
        let delivered: i64 = w
            .store()
            .at(eve_server::item::Slot::new(
                corp,
                station,
                co::FLAG_CORP_DELIVERIES,
            ))
            .filter(|i| i.type_id == ROUND)
            .map(|i| i.quantity)
            .sum();
        assert_eq!(delivered, 5, "the corporation's deliveries hangar");
        let refs: Vec<i64> = cw::all_rows(w.deltas(), corp)
            .iter()
            .map(|r| r.ref_type)
            .collect();
        assert!(
            refs.contains(&42),
            "the escrow is in the corp journal: {refs:?}"
        );
    }
    assert!(
        bob.hangar_items()
            .expect("the hangar")
            .iter()
            .all(|r| r.type_id != ROUND),
        "not Bob's own hangar"
    );
    assert!(
        ann.corp_open_orders().expect("corp orders").is_empty(),
        "a filled order leaves the list"
    );
}
