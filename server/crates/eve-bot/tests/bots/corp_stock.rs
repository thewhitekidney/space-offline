//! DOC-6 over a real socket: **the corporation keeps the doctrine's stock,
//! pays for it from its own wallet, pays ship replacement on a verified loss
//! and mails its doctrine.**
//!
//! Ann is the CEO of a bot corporation, Bob its trader (the procurement
//! member), Cy a ratter. The hall's doctrine for the corp is one fit on the
//! fixture frigate. Steps:
//!
//! 1. A `useCorp = True` bid by a member with no accountant or trader role is
//!    refused and nothing moves (CORP-BUY; DOC-6's U2 found the server
//!    ignoring the box, which it no longer does).
//! 2. The CEO's staging pass notes the open theatre, mails the doctrine to the
//!    corp and lets Bob bid from the corporation's wallet: she grants him
//!    the accountant and trader roles, and funds no float.
//! 3. Bob's pass puts the doctrine's parts in the division and bids for the
//!    shortfall with `useCorp`, inside the stock budget of the Master Wallet.
//! 4. Cy has lost his ship: he draws the kit out of the division.
//! 5. Dan shoots Cy's frigate dead in the ordered system (a real kill): the
//!    loss is paid from the Master Wallet once, by the share of the fit capped
//!    at the killmail's own `iskLost`.
//!
//! **Invented rows and ids only** (`typedata::FIXTURE`, 990_xxx_xxx).

use super::common::{config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_bot::corp_ops as ops;
use eve_bot::corpmind::{CorpMind, Hall, Member};
use eve_bot::doctrine::{
    Doctrine, Family, FleetSize, Layer, Profile, Range, Role, RoleFit, Stance,
};
use eve_bot::doctrine_hall::{DoctrineWorld, Readiness};
use eve_bot::doctrine_lead::{DoctrineBook, Fancy};
use eve_bot::doctrine_stock as stock;
use eve_bot::doctrine_theatre::{Ship, State, Theatre};
use eve_bot::driver;
use eve_bot::hull_choice::Class;
use eve_marshal::PyValue;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;
use eve_server::damage::{Damage, Profile as Tank};
use eve_server::server::Server;
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ANN: i64 = 90_000_811;
const BOB: i64 = 90_000_812;
const CY: i64 = 90_000_813;
const FRIGATE: i64 = 990_000_001;
const GUN: i64 = 990_000_002;
const ROUND: i64 = 990_000_005;
const BASE: f64 = 10_000.0;
const GROUP: i64 = 99_291;
const KEY: &str = "invented/stock";
const DAY: u64 = 1_000;
/// The fit's cost: the pay's basis.
const FIT_COST: f64 = 2_000_000.0;
const ROUNDS: u32 = 10;
/// The theatre is the sandbox's home system, where the fight is.
const THEATRE: i64 = super::common::SYSTEM;
const DAN: i64 = 90_000_814;
/// An invented round heavy enough to kill the fixture frigate in a volley.
const KROUND: i64 = 990_630_001;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn serve_world(config: eve_server::server::Config) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn content() -> eve_server::content::Content {
    let rows = format!(
        "{}type\t{KROUND}\t83\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t117=1000000.0;128=1.0\tFixture Killer Round\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&rows).expect("rows");
    let row = |t: i64, cat: i64| format!("{t}\t{BASE}\tbase\t{BASE}\t{cat}\t{GROUP}\t\t1");
    let prices =
        eve_server::econ_prices::Table::parse(&[row(FRIGATE, 6), row(GUN, 7), row(ROUND, 8)])
            .expect("the prices parse");
    let mut c = eve_server::content::Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c
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

fn join(alice: &mut Conn, who: &mut Conn, id: i64, corp: i64) {
    let a = corp_call(alice, "InsertInvitation", vec![PyValue::Int(id)]);
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

fn real(o: eve_bot::CallOutcome) -> f64 {
    match o.value() {
        Some(PyValue::Real(r)) => r,
        other => panic!("a float, got {other:?} ({:?})", o.refusal()),
    }
}

fn own_balance(c: &mut Conn) -> f64 {
    real(
        c.call("account", "GetCashBalance", vec![PyValue::Int(0)])
            .expect("GetCashBalance(0)"),
    )
}

fn master(c: &mut Conn) -> f64 {
    real(
        c.call_kw(
            "account",
            "GetCashBalance",
            vec![PyValue::Int(1)],
            vec![kw("accountKey", PyValue::Int(cw::MASTER_KEY))],
        )
        .expect("GetCashBalance(1)"),
    )
}

fn units(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar")
        .iter()
        .filter(|r| r.type_id == type_id && !r.singleton)
        .map(|r| r.quantity)
        .sum()
}

fn fit() -> RoleFit {
    let l = Damage {
        em: 1.0,
        explosive: 1.0,
        kinetic: 1.0,
        thermal: 1.0,
    };
    RoleFit {
        hull: FRIGATE,
        modules: vec![(GUN, 1)],
        charges: vec![(ROUND, ROUNDS)],
        skills: vec![],
        profile: Profile::blank(Tank {
            shield_capacity: 1.0,
            shield_recharge_rate: 0.0,
            armor_hp: 1.0,
            hp: 1.0,
            shield_resonance: l,
            armor_resonance: l,
            hull_resonance: l,
        }),
        cost: FIT_COST,
    }
}

fn member(name: &str, career: &str, corp: i64, last_turn: u64) -> Member {
    Member {
        name: name.to_owned(),
        traits: eve_bot::mind::Traits([0.5; eve_bot::mind::TRAIT_COUNT]),
        career: career.to_owned(),
        band: eve_bot::career::Band::High,
        corp,
        alliance: None,
        last_turn: Some(last_turn),
        satisfaction: Default::default(),
        applied: None,
        salt: 0,
    }
}

/// A hall whose corp flies the fit in `KEY` and has an open theatre.
fn hall(corp: i64, now: u64) -> Hall {
    let doctrine = Doctrine {
        key: KEY.to_owned(),
        class: Class::Frigate,
        family: Family::Projectile,
        layer: Layer::Armour,
        active: false,
        range: Range::Short,
        stance: Stance::Brawl,
        roles: vec![(Role::Dps, 1.0, fit())],
        fleet: FleetSize {
            min: 1,
            target: 5,
            max_on_grid: 30,
        },
    };
    let world = DoctrineWorld::new(vec![doctrine], Default::default(), Default::default());
    let hall = Hall::new(DAY)
        .with_every(0)
        .with_doctrine(Arc::new(world), 2, 7);
    {
        let mut st = hall.lock();
        // REVIEW-FIX2: the difficulty rows, not the consts, rule SRP and the
        // stock budget (a half of each, so a caller on the const fails).
        st.knobs.lead.srp_share = stock::SRP_SHARE / 2.0;
        st.knobs.lead.stock_wallet = stock::STOCK_WALLET_SHARE / 2.0;
        st.members.insert(ANN, member("Ann", "miner", corp, now));
        st.members.insert(BOB, member("Bob", "trader", corp, now));
        st.members.insert(CY, member("Cy", "ratter", corp, now));
        st.corps.insert(
            corp,
            CorpMind {
                id: corp,
                ceo: Some(ANN),
                profile: eve_bot::society::Profile::of("ratter", eve_bot::career::Band::High),
                founded: 0,
                alliance: None,
                name: "Stock Invented".to_owned(),
                applied_alliance: None,
                salt: 0,
            },
        );
        let mut book = DoctrineBook::new(Fancy::draw("Ann", 7));
        book.current = Some(KEY.to_owned());
        st.dbooks.insert(corp, book);
        let mut t = Theatre::new(THEATRE, now);
        t.state = State::Hold;
        st.theatres.insert(corp, vec![t]);
        st.readiness.insert(
            CY,
            Readiness {
                ship: Some(Ship::Market),
                ..Readiness::default()
            },
        );
    }
    hall
}

/// Repackage Bob's assembled hulls, as the market and a division want them.
fn repackage_hulls(c: &mut Conn) {
    let station = c.session().station_id().expect("docked");
    let hulls: Vec<PyValue> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == FRIGATE && r.singleton)
        .map(|r| PyValue::Tuple(vec![PyValue::Int(r.item_id), PyValue::Int(station)]))
        .collect();
    if hulls.is_empty() {
        return;
    }
    let batch = PyValue::Dict(vec![(PyValue::List(hulls), PyValue::Int(station))]);
    let done = c
        .call("repackagingSvc", "RepackageItems", vec![batch])
        .expect("answered");
    assert_eq!(done.refusal(), None, "repackaged");
}

/// Dan fits the fixture autocannon with the killer round and undocks.
fn arm(c: &mut Conn) -> eve_bot::ItemRow {
    gm(c, &format!("/createitem {FRIGATE}"));
    let hull = c
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE && r.singleton)
        .expect("the frigate");
    c.board(hull.item_id).expect("boards");
    gm(c, &format!("/createitem {GUN}"));
    gm(c, &format!("/createitem {KROUND} 10"));
    let station = c.session().station_id().expect("docked");
    let spare: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == GUN)
        .map(|r| r.item_id)
        .collect();
    let gun = c
        .fit_auto(&spare)
        .expect("fits")
        .into_iter()
        .find(|r| r.type_id == GUN)
        .expect("the gun is fitted");
    if !c.is_online(gun.item_id).expect("asks") {
        c.set_online(gun.item_id).expect("onlines");
    }
    let rounds: Vec<i64> = c
        .hangar_items()
        .expect("lists")
        .iter()
        .filter(|r| r.type_id == KROUND)
        .map(|r| r.item_id)
        .collect();
    c.load_ammo(&[(gun.item_id, gun.flag_id)], &rounds, KROUND, station)
        .expect("loads");
    c.undock().expect("undocks");
    c.enter_space(THEATRE).expect("state");
    gun
}

/// Cy undocks in a frigate, Dan shoots it dead. Answers the dead ship's id.
fn kill_cys_frigate(
    addr: SocketAddr,
    cy: &mut Conn,
    content: &Arc<eve_server::content::Content>,
) -> i64 {
    gm(cy, &format!("/createitem {FRIGATE}"));
    let hull = cy
        .hangar_items()
        .expect("lists")
        .into_iter()
        .find(|r| r.type_id == FRIGATE && r.singleton)
        .expect("the frigate");
    cy.board(hull.item_id).expect("boards");
    cy.undock().expect("undocks");
    cy.enter_space(THEATRE).expect("state");
    let mut dan = login(addr, "Dan");
    let gun = arm(&mut dan);
    dan.lock_target(hull.item_id).expect("locks");
    dan.fire(gun.item_id, GUN, Some(content), hull.item_id)
        .expect("fires");
    let dead = until(Duration::from_secs(90), || {
        let _ = cy.collect_pushes(Duration::from_millis(100));
        cy.session().ship_id() != Some(hull.item_id)
    });
    assert!(dead, "Cy's ship dies");
    hull.item_id
}

#[test]
fn corp_stock_socket() {
    let mut config = config();
    let rich = |id: i64, name: &str, ship: i64| {
        let mut p = pilot(id, name, ship);
        p.balance = 5_000_000.0;
        p
    };
    config.sandbox.characters = vec![
        rich(ANN, "Ann", 100_000_811),
        rich(BOB, "Bob", 100_000_812),
        rich(CY, "Cy", 100_000_813),
        rich(DAN, "Dan", 100_000_814),
    ];
    let content = Arc::new(content());
    config.content = Some(content.clone());
    let (addr, world) = serve_world(config);
    let mut ann = login(addr, "Ann");
    let mut bob = login(addr, "Bob");
    let mut cy = login(addr, "Cy");
    gm(&mut ann, "/giveisk 2000000000");
    let a = corp_call(
        &mut ann,
        "AddCorporation",
        vec![uni("Stock Invented"), uni("STKI"), uni(""), uni("")],
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

    // The Master Wallet holds the rent for an office and a stock's worth.
    let station = ann.session().get_i64("stationid").expect("docked");
    const MASTER: f64 = 50_000_000.0;
    {
        let mut w = world.write().expect("the world lock");
        cw::adjust(
            w.deltas_mut(),
            corp,
            cw::MASTER_KEY,
            cw::to_cents(co::OFFICE_RENT_ISK * 3.0 + MASTER),
        )
        .expect("funded");
    }
    let quote = ann
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "GetPriceQuote",
            vec![PyValue::Int(corp)],
            vec![],
        )
        .expect("quoted");
    let price = match quote.value {
        Some(PyValue::Real(p)) => p,
        other => panic!("a price, got {other:?}"),
    };
    let rented = ann
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "RentOffice",
            vec![PyValue::Real(price)],
            vec![],
        )
        .expect("answered");
    assert_eq!(rented.refusal(), None, "RentOffice refused");

    let now = driver::unix_now();
    let hall = hall(corp, now);
    // Roles first: the trader tends the division, the ratter draws from it.
    let lines = ops::run_ceo(&mut ann, &hall, ANN, corp, now).expect("the pass");
    assert!(
        lines.iter().any(|l| l.contains("roles of")),
        "no grants: {lines:?}"
    );

    // 1. A `useCorp` bid without the role is refused (CORP-BUY).
    let master_before = master(&mut ann);
    let bob_before = own_balance(&mut bob);
    let placed = bob
        .call(
            "marketProxy",
            "PlaceBuyOrder",
            vec![
                PyValue::Int(station),
                PyValue::Int(ROUND),
                PyValue::Real(1.0),
                PyValue::Int(10),
                PyValue::Int(eve_bot::builder::RANGE_STATION),
                PyValue::Int(1),
                PyValue::Int(1),
                PyValue::Bool(true),
                PyValue::Real(0.0),
            ],
        )
        .expect("answered");
    assert_eq!(placed.refusal().as_deref(), Some("CrpAccessDenied"));
    assert_eq!(own_balance(&mut bob), bob_before, "the pilot pays nothing");
    assert_eq!(master(&mut ann), master_before, "nor does the corp wallet");

    // 2. The CEO stages, announces and lets Bob bid from the wallet.
    let lines = stock::run_stage(&mut ann, &hall, ANN, corp, now).expect("stages");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("doctrine") && l.ends_with("ok")),
        "no announcement: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("procurement roles") && l.ends_with("ok")),
        "no grant: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("fund")),
        "no float is funded: {lines:?}"
    );
    assert!(hall.snapshot().stock.direct.contains(&BOB));
    assert_eq!(
        master(&mut ann),
        master_before,
        "nothing left the wallet yet"
    );
    assert_eq!(
        hall.snapshot().stock.staging.get(&corp),
        Some(&THEATRE),
        "the open theatre is the staging"
    );
    let heard = eve_bot::mail::sync_inbox(&mut bob, BOB).expect("syncs");
    assert!(
        heard.iter().any(|h| h.sender == ANN),
        "the doctrine mail reached a member: {heard:?}"
    );
    // The announcement is once, not every pass.
    let again = stock::run_stage(&mut ann, &hall, ANN, corp, now + 1).expect("stages");
    assert!(!again.iter().any(|l| l.contains("mail corp")), "{again:?}");

    // 3. Bob has parts; he stocks the division and bids with `useCorp`.
    gm(&mut bob, &format!("/createitem {FRIGATE} 1"));
    repackage_hulls(&mut bob);
    gm(&mut bob, &format!("/createitem {GUN} 1"));
    gm(&mut bob, &format!("/createitem {ROUND} 5"));
    let bob_before = own_balance(&mut bob);
    let lines = stock::run_procure(&mut bob, &hall, BOB, corp, now).expect("procures");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("stock") && l.ends_with("ok")),
        "no deposit: {lines:?}"
    );
    assert_eq!(units(&mut bob, GUN), 0, "the gun went to the division");
    let held = hall.snapshot().stock.held[&corp].clone();
    assert_eq!(held.get(&GUN), Some(&1));
    assert_eq!(held.get(&ROUND), Some(&5));
    let resting: f64 = bob
        .corp_open_orders()
        .expect("the corporation's orders")
        .iter()
        .filter(|o| o.order.bid && o.order.price > 1.5)
        .map(|o| o.order.price * o.order.vol_remaining as f64)
        .sum();
    assert!(resting > 0.0, "the shortfall is bid for");
    assert!(
        resting
            <= stock::stock_budget(
                master_before,
                ops::pay_reserve(),
                stock::STOCK_WALLET_SHARE / 2.0
            ) + 1.0,
        "inside the stock budget: {resting}"
    );
    assert!(
        master(&mut ann) <= master_before - resting + 1.0,
        "the escrow came out of the Master Wallet"
    );
    assert_eq!(own_balance(&mut bob), bob_before, "not out of Bob's wallet");
    assert!(
        bob.my_open_orders()
            .expect("orders")
            .iter()
            .all(|o| o.order.price < 1.5),
        "none of the bids is Bob's own"
    );

    // 4. Cy has no ship: he draws his kit out of the division.
    let lines = stock::run_draw(&mut cy, &hall, CY, corp).expect("draws");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("draw") && l.ends_with("ok")),
        "no draw: {lines:?}"
    );
    assert_eq!(units(&mut cy, GUN), 1, "the gun is in Cy's hangar");
    assert_eq!(units(&mut cy, ROUND), 5);

    // Later he flies a frigate into the theatre's system and Dan kills it:
    // a real killmail.
    let cy_ship = kill_cys_frigate(addr, &mut cy, &content);
    let _ = cy_ship;
    // 5. SRP: the loss was in the ordered system, on the doctrine's hull.
    let isk_lost = match cy
        .call(
            "charMgr",
            "GetRecentShipKillsAndLosses",
            vec![PyValue::Int(25), PyValue::None],
        )
        .expect("answered")
        .value()
    {
        Some(PyValue::List(rows)) => eve_bot::observe::real(&rows[0], "iskLost").expect("a value"),
        other => panic!("a list, got {other:?}"),
    };
    let cy_before = own_balance(&mut cy);
    let master_before = master(&mut ann);
    // REVIEW-FIX: Cy drew part of his kit free from the division (step 4),
    // and SRP nets that share out of what it pays.
    let share = hall
        .snapshot()
        .stock
        .kit_share
        .get(&CY)
        .copied()
        .expect("the draw is recorded");
    assert!(share > 0.0 && share < 1.0, "part of the kit: {share}");
    let due = (stock::SRP_SHARE / 2.0 * FIT_COST * (1.0 - share)).min(isk_lost);
    let lines = stock::run_srp(&mut ann, &hall, ANN, corp, now).expect("pays");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("srp") && l.ends_with("ok")),
        "no pay: {lines:?}"
    );
    assert!(
        (own_balance(&mut cy) - (cy_before + due)).abs() < 1.0,
        "Cy is paid the share of the fit, capped by the killmail: {due}"
    );
    assert!(
        (master(&mut ann) - (master_before - due)).abs() < 1.0,
        "from the Master Wallet"
    );
    assert_eq!(hall.snapshot().stock.paid.len(), 1);
    let again = stock::run_srp(&mut ann, &hall, ANN, corp, now + 1).expect("pays");
    assert!(
        !again.iter().any(|l| l.ends_with("ok")),
        "the same kill is not paid twice: {again:?}"
    );
    assert!((own_balance(&mut cy) - (cy_before + due)).abs() < 1.0);
}
