//! BOT-CORP over a real socket: a CEO bot sets the corporation's tax from its
//! traits, a member earns a bounty and the tax lands in the Master Wallet,
//! and the CEO pays the member from it; builders, traders and couriers are
//! granted the hangar division, a builder delivers product to it and a
//! trader sells it and remits to the Master Wallet.
//!
//! The calls are the client's own (`docs/lanes/bot-corp.md` §2). Names and
//! ids are invented; every number comes from a named const of
//! `eve_bot::corp_ops` (the `[A]` ones) or `eve_server`. The same limitation
//! as every test here: both ends marshal with `eve-marshal`.

use super::common::{config, dump, gm, login, pilot, until};
use eve_bot::Conn;
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::corp_ops as ops;
use eve_bot::corpmind::{Hall, Member};
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait, Traits};
use eve_bot::society::Satisfaction;
use eve_marshal::PyValue;
use eve_server::content::Content;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;
use eve_server::server::{Config, Server};
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ANN: i64 = 90_000_701;
const BOB: i64 = 90_000_702;
const CY: i64 = 90_000_703;
const BUYER: i64 = 90_000_704;
const PRODUCT: i64 = 990_700_001;
const PRODUCT_GROUP: i64 = 99_083;
const PRODUCT_REF: f64 = 100.0;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, PyValue::Bytes(name.as_bytes().to_vec()))
}

fn serve_world(config: Config) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

fn found_corp(c: &mut Conn, name: &str, ticker: &str) -> i64 {
    gm(c, "/giveisk 2000000000");
    let a = corp_call(
        c,
        "AddCorporation",
        vec![uni(name), uni(ticker), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    a.outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp")
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

/// `who` is invited and accepts.
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

/// The corporation row's `taxRate`, as a member's client reads it
/// (`Row(header, line)`).
fn tax_rate_of(c: &mut Conn) -> f64 {
    let a = corp_call(c, "GetCorporation", vec![]);
    let Some(PyValue::ObjectEx { header, .. }) = a.value else {
        panic!("a row")
    };
    let PyValue::Tuple(h) = header.as_ref() else {
        panic!("a header")
    };
    let PyValue::Tuple(args) = &h[1] else {
        panic!("a row's args")
    };
    let (PyValue::List(names), PyValue::List(line)) = (&args[0], &args[1]) else {
        panic!("names and line")
    };
    let i = names
        .iter()
        .position(|n| matches!(n, PyValue::Bytes(b) if b == b"taxRate"))
        .expect("a taxRate column");
    match line[i] {
        PyValue::Real(r) => r,
        ref other => panic!("a float, got {other:?}"),
    }
}

fn traits() -> Traits {
    let mut t = Traits([0.5; TRAIT_COUNT]);
    t.0[Trait::Loyalty as usize] = 1.0;
    t
}

fn member(name: &str, career: &str, corp: i64, last_turn: u64) -> Member {
    Member {
        name: name.to_owned(),
        traits: traits(),
        career: career.to_owned(),
        band: Band::High,
        corp,
        alliance: None,
        last_turn: Some(last_turn),
        satisfaction: Satisfaction::default(),
        applied: None,
        salt: 0,
    }
}

static TABLE: &[Career] = &[Career {
    name: "miner",
    role: Some(&eve_bot::roles::MINE),
    support: Support::Served,
    base: 1.0,
    affinity: [0.0; TRAIT_COUNT],
    band: Band::High,
    prior_isk_h: INCOME_REF,
}];

#[test]
fn a_ceo_bot_sets_the_tax_a_member_earns_a_bounty_and_the_ceo_pays_the_member() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_701),
        pilot(BOB, "Bob", 100_000_702),
    ];
    let (addr, world) = serve_world(config);
    let mut bob = login(addr, "Bob");
    let corp = {
        let mut c = login(addr, "Ann");
        let corp = found_corp(&mut c, "Bot Corp Invented", "BOTC");
        join(&mut c, &mut bob, BOB, corp);
        corp
    };
    let now = driver::unix_now();
    let mut mind = Mind::seeded("Ann", 1, TABLE, &Signals::default(), now);
    mind.career = "miner".to_owned();
    mind.since = now;
    mind.home = Some(60_012_526);
    mind.traits = traits();
    let hall = Arc::new(Hall::new(2).with_every(0));
    {
        // Bob is a bot of the hall, seen lately; the test keeps him fresh.
        hall.lock()
            .members
            .insert(BOB, member("Bob", "miner", corp, now));
    }
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(&hall));
    let mut bot_config = BotConfig::new("Ann", addr, Pace::Fast).with_society(soc);
    bot_config.chat = false;
    bot_config.policy.gm_setup = false;
    let (handle, thread) = driver::spawn(bot_config);

    // The CEO asks the rate its traits give.
    let want = ops::tax_rate(&traits());
    let set = until(Duration::from_secs(60), || {
        let t = driver::unix_now();
        if let Some(m) = hall.lock().members.get_mut(&BOB) {
            m.last_turn = Some(t);
        }
        (tax_rate_of(&mut bob) - want).abs() < 1e-9
    });
    if !set {
        handle.stop();
        let _ = thread.join();
        panic!(
            "no tax set:\nacts {:?}\n{}",
            hall.snapshot().acts,
            dump(&handle)
        );
    }

    // Bob has a bounty payout due; the world pays it as the clock would.
    let gross = 100.0 * cw::MIN_TAXED_PAYOUT;
    let bob_before = own_balance(&mut bob);
    {
        let mut w = world.write().expect("the world lock");
        w.deltas_mut().put(
            eve_server::npc::bounty::PENDING_KIND,
            vec![BOB.to_string(), "30000001".to_owned()],
            vec![format!("{gross:.0}"), "1".to_owned(), "599011:3".to_owned()],
        );
        let at = eve_server::pyutil::filetime_now();
        assert_eq!(eve_server::npc::bounty::flush(&mut w, at), 1);
    }
    let tax = cw::tax_for(want, gross);
    let net = gross - tax;
    let pay = ops::pay_pool(tax, tax, ops::pay_reserve());
    assert!(pay >= ops::PAY_MIN, "the fixture is big enough to pay");
    // The CEO pays the member the pool (only Bob is a member to pay).
    let paid = until(Duration::from_secs(60), || {
        let t = driver::unix_now();
        if let Some(m) = hall.lock().members.get_mut(&BOB) {
            m.last_turn = Some(t);
        }
        own_balance(&mut bob) > bob_before + net + 1.0
    });
    handle.stop();
    let _ = thread.join();
    assert!(
        paid,
        "no pay:\nacts {:?}\n{}",
        hall.snapshot().acts,
        dump(&handle)
    );
    let bob_after = own_balance(&mut bob);
    assert!(
        (bob_after - (bob_before + net + pay)).abs() < 1.0,
        "Bob holds the net bounty and the pay: {bob_after} vs {}",
        bob_before + net + pay
    );
    assert!(
        (master(&mut bob_as_ceo(addr)) - (tax - pay)).abs() < 1.0,
        "the Master Wallet holds the tax less the pay"
    );
    assert!(
        hall.snapshot()
            .acts
            .iter()
            .any(|a| a.contains("set corp") && a.contains("tax")),
        "{:?}",
        hall.snapshot().acts
    );
}

/// Ann, as a plain connection, to read the Master Wallet (the bot has
/// stopped).
fn bob_as_ceo(addr: SocketAddr) -> Conn {
    login(addr, "Ann")
}

fn content() -> Content {
    let types = format!(
        "{}type\t{PRODUCT}\t{PRODUCT_GROUP}\t8\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tFixture Product\n",
        eve_server::typedata::FIXTURE
    );
    let td = eve_server::typedata::Table::parse(&types).expect("the test rows parse");
    let prices = eve_server::econ_prices::Table::parse(&[format!(
        "{PRODUCT}\t{PRODUCT_REF}\trecipe\t{PRODUCT_REF}\t8\t{PRODUCT_GROUP}\t1\t1"
    )])
    .expect("the prices parse");
    let mut c = Content::fixture();
    c.insert("typedata", eve_server::typedata::Installed::build(td));
    c.insert("econ_prices", prices);
    c
}

fn units(c: &mut Conn, type_id: i64) -> i64 {
    c.hangar_items()
        .expect("the hangar")
        .iter()
        .filter(|r| r.type_id == type_id)
        .map(|r| r.quantity)
        .sum()
}

#[test]
fn builders_are_granted_the_division_a_builder_delivers_and_a_trader_sells_and_remits() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ANN, "Ann", 100_000_701),
        pilot(BOB, "Bob", 100_000_702),
        pilot(CY, "Cy", 100_000_703),
        pilot(BUYER, "Buyer", 100_000_704),
    ];
    config.content = Some(Arc::new(content()));
    let (addr, world) = serve_world(config);
    let mut ann = login(addr, "Ann");
    let mut bob = login(addr, "Bob");
    let mut cy = login(addr, "Cy");
    let corp = found_corp(&mut ann, "Stock Invented", "STOK");
    join(&mut ann, &mut bob, BOB, corp);
    join(&mut ann, &mut cy, CY, corp);
    // The corporation holds the rent for two periods and rents at home.
    let station = ann.session().get_i64("stationid").expect("docked");
    {
        let mut w = world.write().expect("the world lock");
        cw::adjust(
            w.deltas_mut(),
            corp,
            cw::MASTER_KEY,
            cw::to_cents(co::OFFICE_RENT_ISK * 3.0),
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
    let hall = Hall::new(2).with_every(0);
    {
        let mut st = hall.lock();
        st.members.insert(ANN, member("Ann", "miner", corp, now));
        st.members.insert(BOB, member("Bob", "builder", corp, now));
        st.members.insert(CY, member("Cy", "trader", corp, now));
    }
    // The CEO's pass grants the division to the builder and the trader.
    let lines = ops::run_ceo(&mut ann, &hall, ANN, corp, now).expect("the pass");
    assert!(
        lines.iter().any(|l| l.contains("roles of")),
        "no grants: {lines:?}"
    );

    // Bob has product; he delivers a share to the division.
    gm(&mut bob, &format!("/createitem {PRODUCT} 100"));
    let lines = ops::run_member(&mut bob, &hall, BOB, corp, "builder").expect("delivers");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("deliver") && l.ends_with("ok")),
        "{lines:?}"
    );
    let delivered = (100.0 * ops::DELIVER_SHARE).floor() as i64;
    assert_eq!(units(&mut bob, PRODUCT), 100 - delivered);

    // A buyer bids for it; Cy takes it out, sells it and remits.
    let mut buyer = login(addr, "Buyer");
    gm(&mut buyer, "/giveisk 100000000");
    let bid = buyer
        .buy(station, PRODUCT, PRODUCT_REF, delivered, 1, 0.0)
        .expect("the bid is placed");
    let _ = bid;
    let master_before = master(&mut ann);
    let cy_before = own_balance(&mut cy);
    let lines = ops::run_member(&mut cy, &hall, CY, corp, "trader").expect("sells");
    assert!(
        lines.iter().any(|l| l.contains("remitted")),
        "no sale: {lines:?}"
    );
    let earned = (own_balance(&mut cy) - cy_before) + (master(&mut ann) - master_before);
    assert!(earned > 0.0, "the stock sold for something");
    assert!(
        master(&mut ann) > master_before,
        "the Master Wallet got its share"
    );
    assert_eq!(units(&mut cy, PRODUCT), 0, "nothing is left over");
}
