//! CORP-O over a real socket: a director rents an office and pays from the
//! corporation's wallet; a member with division 2's roles puts an item in
//! it, and a member without them is refused; a CEO bot rents an office at
//! its home station on its own.
//!
//! The calls are the client's own (`docs/lanes/corp-o.md` §2):
//! `officeManager` bound on the station (`GetPriceQuote(corpID)`,
//! `RentOffice(cost)`) and `GetMyCorporationsOffices` unbound, then
//! `GetInventoryFromId(officeID, 0)` and `Add(itemID, sourceID, qty=, flag=)`
//! on the office's inventory (`invCacheContainer.Add`). Names and ids are
//! invented; amounts come from the named consts. The same limitation as
//! every test here: both ends marshal with `eve-marshal`.

use super::common::{VELDSPAR, config, dump, gm, login, pilot, until};
use eve_bot::Conn;
use eve_bot::career::{Band, Career, INCOME_REF, Signals, Support};
use eve_bot::corpmind::Hall;
use eve_bot::driver::{self, BotConfig, Pace, Society};
use eve_bot::mind::{Mind, TRAIT_COUNT, Trait};
use eve_marshal::PyValue;
use eve_server::corp_office as co;
use eve_server::corp_wallet as cw;
use eve_server::server::{Config, Server};
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_601;
const BOB: i64 = 90_000_602;
const CAROL: i64 = 90_000_603;
/// `Character::default()`'s station, where every fixture pilot is docked.
const HOME: i64 = 60_012_526;
const DIVISION_2: i64 = co::FLAG_SAG[1];

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

/// `account.GiveCash(corp, isk, reason, toAccountKey=1000)`: into the Master
/// Wallet, as the Give ISK tab sends it.
fn deposit(c: &mut Conn, corp: i64, isk: f64) {
    let o = c
        .call_kw(
            "account",
            "GiveCash",
            vec![PyValue::Int(corp), PyValue::Real(isk), uni("rent")],
            vec![kw("toAccountKey", PyValue::Int(cw::MASTER_KEY))],
        )
        .expect("GiveCash is answered");
    assert_eq!(o.refusal(), None, "the deposit refused");
}

/// An office row's `officeID` from `GetMyCorporationsOffices`' answer
/// (`Row(header, line)`).
fn office_ids(v: &PyValue) -> Vec<i64> {
    let PyValue::List(rows) = v else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|r| {
            let PyValue::ObjectEx { header, .. } = r else {
                return None;
            };
            let PyValue::Tuple(h) = header.as_ref() else {
                return None;
            };
            let PyValue::Tuple(args) = h.get(1)? else {
                return None;
            };
            let PyValue::List(line) = args.get(1)? else {
                return None;
            };
            match line.first()? {
                PyValue::Int(id) => Some(*id),
                _ => None,
            }
        })
        .collect()
}

fn my_offices(c: &mut Conn) -> Vec<i64> {
    let o = c
        .call("officeManager", "GetMyCorporationsOffices", vec![])
        .expect("GetMyCorporationsOffices");
    office_ids(&o.value().expect("a list"))
}

fn real(v: Option<PyValue>) -> f64 {
    match v {
        Some(PyValue::Real(r)) => r,
        other => panic!("a float, got {other:?}"),
    }
}

#[test]
fn a_director_rents_an_office_a_member_with_the_role_fills_division_two_and_one_without_is_refused()
{
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_601),
        pilot(BOB, "Bob", 100_000_602),
        pilot(CAROL, "Carol", 100_000_603),
    ];
    let (addr, world) = serve_world(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let mut carol = login(addr, "Carol");
    let corp = found_corp(&mut alice, "Office Invented", "OFFI");
    join(&mut alice, &mut bob, BOB, corp);
    join(&mut alice, &mut carol, CAROL, corp);
    alice.keep_pushes();
    let station = alice.session().get_i64("stationid").expect("docked");
    assert_eq!(station, HOME);

    // The corporation pays: no ISK in the wallet, no office.
    let quote = alice
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "GetPriceQuote",
            vec![PyValue::Int(corp)],
            vec![],
        )
        .expect("the CEO is quoted");
    assert_eq!(quote.refusal(), None);
    let price = real(quote.value);
    assert_eq!(price, co::OFFICE_RENT_ISK);
    let poor = alice
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "RentOffice",
            vec![PyValue::Real(price)],
            vec![],
        )
        .expect("answered");
    assert_eq!(poor.refusal().as_deref(), Some("NotEnoughMoneyCorp"));
    assert!(my_offices(&mut alice).is_empty());

    deposit(&mut alice, corp, price * 3.0);
    let rented = alice
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "RentOffice",
            vec![PyValue::Real(price)],
            vec![],
        )
        .expect("answered");
    assert_eq!(rented.refusal(), None, "RentOffice refused");
    let heard = until(Duration::from_secs(10), || {
        let _ = alice.pump(Duration::from_millis(200));
        alice
            .take_pushes()
            .iter()
            .any(|(name, _)| name == "OnOfficeRentalChange")
    });
    assert!(heard, "Alice never heard OnOfficeRentalChange");
    let offices = my_offices(&mut alice);
    assert_eq!(offices.len(), 1, "one office");
    let office = offices[0];
    {
        let w = world.read().expect("the world lock");
        assert_eq!(
            cw::to_isk(cw::balance(w.deltas(), corp, cw::MASTER_KEY)),
            price * 2.0,
            "the rent came out of the corporation's wallet"
        );
        assert!(co::get(w.deltas(), office).is_some_and(|o| o.live()));
    }
    // A second rental here is refused with the client's own key.
    let twice = alice
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "GetPriceQuote",
            vec![PyValue::Int(corp)],
            vec![],
        )
        .expect("answered");
    assert_eq!(
        twice.refusal().as_deref(),
        Some("RentingYouHaveAnOfficeHere")
    );
    // The Offices panel lists the corporation to anyone docked here.
    let listed = bob
        .moniker_call(
            "officeManager",
            PyValue::Int(station),
            "GetCorporationsWithOffices",
            vec![],
            vec![],
        )
        .expect("answered");
    assert_eq!(listed.value, Some(PyValue::List(vec![PyValue::Int(corp)])));

    // Bob gets division 2's take and query roles from the CEO.
    let mut args = vec![
        PyValue::Int(BOB),
        PyValue::None,
        PyValue::None,
        PyValue::None,
    ];
    args.push(PyValue::Int(
        co::ROLE_HANGAR_TAKE[1] | co::ROLE_HANGAR_QUERY[1],
    ));
    args.extend(std::iter::repeat_n(PyValue::Int(0), 7));
    args.extend([PyValue::None, PyValue::None, PyValue::None]);
    let a = corp_call(&mut alice, "UpdateMember", args);
    assert_eq!(a.refusal(), None, "UpdateMember refused");

    // Both have ore in their hangars.
    for c in [&mut bob, &mut carol] {
        gm(c, &format!("/createitem {VELDSPAR} 100"));
    }
    let ore = |c: &mut Conn| {
        c.hangar_items()
            .expect("the hangar")
            .into_iter()
            .find(|r| r.type_id == VELDSPAR)
            .expect("ore in the hangar")
    };
    let (bobs, carols) = (ore(&mut bob), ore(&mut carol));
    let drag = |c: &mut Conn, item: i64| {
        let inv = c.open_container(office).expect("the office's inventory");
        let outcome = c
            .call_on(
                &inv,
                "Add",
                vec![PyValue::Int(item), PyValue::Int(station)],
                vec![
                    kw("qty", PyValue::None),
                    kw("flag", PyValue::Int(DIVISION_2)),
                ],
            )
            .expect("Add is answered");
        (inv, outcome)
    };

    // Carol holds no role: refused, and her ore stays.
    let (_, refused) = drag(&mut carol, carols.item_id);
    assert_eq!(refused.refusal().as_deref(), Some("CrpAccessDenied"));
    assert_eq!(ore(&mut carol).quantity, 100);

    // Bob holds both: his ore is the corporation's, in division 2.
    let (inv, done) = drag(&mut bob, bobs.item_id);
    assert_eq!(done.refusal(), None, "Bob's Add refused");
    let inside = bob.list(&inv, Some(DIVISION_2)).expect("the division");
    assert_eq!(inside.len(), 1);
    assert_eq!(
        (inside[0].owner_id, inside[0].location_id, inside[0].flag_id),
        (corp, office, DIVISION_2)
    );
    assert_eq!(inside[0].quantity, 100);
    assert!(
        bob.hangar_items()
            .expect("the hangar")
            .iter()
            .all(|r| r.type_id != VELDSPAR),
        "the ore left Bob's hangar"
    );
    // Carol may not look, and Bob may not look into division 3.
    let cinv = carol.open_container(office).expect("inventory");
    assert!(carol.list(&cinv, None).expect("list").is_empty());
    assert!(
        bob.list(&inv, Some(co::FLAG_SAG[2]))
            .expect("list")
            .is_empty()
    );
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
fn a_ceo_bot_rents_an_office_at_its_home_station_when_the_corp_can_pay() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Ann", 100_000_611)];
    let (addr, world) = serve_world(config);
    // Ann founds the corp as a plain connection (the bots' founding is
    // CO-FOUND's test), and the corporation holds two periods of rent.
    let corp = {
        let mut c = login(addr, "Ann");
        found_corp(&mut c, "Bot Office Invented", "BOFF")
    };
    {
        let mut w = world.write().expect("the world lock");
        let cents = cw::to_cents(co::OFFICE_RENT_ISK * eve_bot::corpmind::OFFICE_RESERVE_PERIODS);
        cw::adjust(w.deltas_mut(), corp, cw::MASTER_KEY, cents).expect("funded");
    }
    let now = driver::unix_now();
    let mut mind = Mind::seeded("Ann", 1, TABLE, &Signals::default(), now);
    mind.career = "miner".to_owned();
    mind.since = now;
    mind.home = Some(HOME);
    for t in &mut mind.traits.0 {
        *t = 0.5;
    }
    mind.traits.0[Trait::Loyalty as usize] = 1.0;
    let hall = Arc::new(Hall::new(2).with_every(0));
    let mut soc = Society::new(mind, TABLE, Signals::default());
    soc.decide_every = Duration::from_secs(3_600);
    soc.hall = Some(Arc::clone(&hall));
    let mut bot_config = BotConfig::new("Ann", addr, Pace::Fast).with_society(soc);
    bot_config.chat = false;
    bot_config.policy.gm_setup = false;
    let (handle, thread) = driver::spawn(bot_config);
    let rented = until(Duration::from_secs(60), || {
        let w = world.read().expect("the world lock");
        !co::live_of(w.deltas(), corp).is_empty()
    });
    handle.stop();
    let _ = thread.join();
    assert!(
        rented,
        "no office:\nacts {:?}\n{}",
        hall.snapshot().acts,
        dump(&handle)
    );
    let w = world.read().expect("the world lock");
    let o = co::live_of(w.deltas(), corp);
    assert_eq!(o[0].station, HOME);
    assert_eq!(
        cw::to_isk(cw::balance(w.deltas(), corp, cw::MASTER_KEY)),
        co::OFFICE_RENT_ISK * (eve_bot::corpmind::OFFICE_RESERVE_PERIODS - 1.0)
    );
    drop(w);
    assert!(
        hall.snapshot()
            .acts
            .iter()
            .any(|a| a.contains("rented an office")),
        "{:?}",
        hall.snapshot().acts
    );
}
