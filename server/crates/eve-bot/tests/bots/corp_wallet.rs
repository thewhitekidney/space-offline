//! CORP-W over a real socket: a member gives ISK to the corporation and the
//! CEO takes some out to a member; a payout is taxed into the Master Wallet;
//! `CreateAlliance` is paid from the corporation's wallet and not the CEO's.
//!
//! The calls are the client's own (`docs/lanes/corp-w.md` §2): the Wallet
//! window's `account` service (`GiveCash(toID, amount, reason, toAccountKey=)`,
//! `GiveCashFromCorpAccount(toID, amount, fromAccountKey, toAccountKey=,
//! reason=)`, `GetCashBalance(1, accountKey=)`, `GetWalletDivisionsInfo`,
//! `GetTransactions(accountKey, year, month, True)`) and `corpRegistry`
//! through its moniker. Names, tickers and ids are invented. The same
//! limitation as every test here: both ends marshal with `eve-marshal`.

use super::common::{config, gm, login, pilot, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use eve_server::server::{Config, Server};
use eve_server::service::Registry;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

const ALICE: i64 = 90_000_501;
const BOB: i64 = 90_000_502;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

/// A keyword argument in wire order: `(value, key)`.
fn kw(name: &str, value: PyValue) -> (PyValue, PyValue) {
    (value, PyValue::Bytes(name.as_bytes().to_vec()))
}

/// The server fixture, with the world it runs, so a test can reach the
/// registry the way the world clock would.
fn serve_world(config: Config) -> (SocketAddr, Arc<RwLock<Registry>>) {
    let server = Server::bind(config).expect("bind");
    let addr = server.local_addr().expect("local addr");
    let world = server.world();
    std::thread::spawn(move || {
        let _ = server.run();
    });
    (addr, world)
}

/// `corpRegistry`, bound on the caller's corp id as `GetCorpRegistry` does.
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

/// The `applicationID` of the first application in a `GetMyApplications`
/// answer: `{corpID: [KeyVal, ...]}`.
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

/// Alice invites Bob; Bob accepts (status 2) and joins.
fn join(alice: &mut Conn, bob: &mut Conn, corp: i64) {
    let a = corp_call(alice, "InsertInvitation", vec![PyValue::Int(BOB)]);
    assert_eq!(a.refusal(), None, "InsertInvitation refused");
    let mine = corp_call(bob, "GetMyApplications", vec![]);
    let id = mine
        .value
        .as_ref()
        .and_then(first_application_id)
        .expect("Bob's invitation");
    let a = corp_call(
        bob,
        "UpdateApplicationOffer",
        vec![
            PyValue::Int(id),
            PyValue::Int(BOB),
            PyValue::Int(corp),
            uni(""),
            PyValue::Int(2),
        ],
    );
    assert_eq!(a.refusal(), None, "accepting refused");
    assert_eq!(bob.session().get_i64("corpid"), Some(corp));
}

/// `account.GiveCash(corp, isk, reason, toAccountKey=key)`: a deposit, as
/// `transferMoneyWnd.Confirm` sends it.
pub fn deposit(c: &mut Conn, corp: i64, isk: f64, key: i64) -> eve_bot::CallOutcome {
    c.call_kw(
        "account",
        "GiveCash",
        vec![PyValue::Int(corp), PyValue::Real(isk), uni("test")],
        vec![kw("toAccountKey", PyValue::Int(key))],
    )
    .expect("GiveCash is answered")
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

fn corp_balance(c: &mut Conn, key: i64) -> eve_bot::CallOutcome {
    c.call_kw(
        "account",
        "GetCashBalance",
        vec![PyValue::Int(1)],
        vec![kw("accountKey", PyValue::Int(key))],
    )
    .expect("GetCashBalance(1)")
}

/// The ISK in a `GetCashBalance` push for `corp`'s division `key_name`.
fn push_balance(pushes: &[(String, Vec<PyValue>)], key_name: &str, corp: i64) -> Option<f64> {
    pushes.iter().rev().find_map(|(name, args)| {
        if name != "OnAccountChange" {
            return None;
        }
        match args.as_slice() {
            [PyValue::Bytes(k), PyValue::Int(owner), PyValue::Real(b)]
                if k == key_name.as_bytes() && *owner == corp =>
            {
                Some(*b)
            }
            _ => None,
        }
    })
}

#[test]
fn a_member_gives_isk_to_the_corp_and_the_ceo_withdraws_to_a_member() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_501),
        pilot(BOB, "Bob", 100_000_502),
    ];
    let (addr, _) = serve_world(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let corp = found_corp(&mut alice, "Wallet Invented", "WALL");
    join(&mut alice, &mut bob, corp);
    alice.keep_pushes();
    gm(&mut bob, "/giveisk 1000000");
    let bob_before = own_balance(&mut bob);

    // Bob deposits into the second division.
    let a = deposit(&mut bob, corp, 250_000.0, 1001);
    assert_eq!(a.refusal(), None, "GiveCash refused");
    assert_eq!(
        a.value(),
        Some(PyValue::Bool(true)),
        "the Give ISK tab truth-tests it"
    );
    assert_eq!(own_balance(&mut bob), bob_before - 250_000.0);

    // The CEO reads it; a plain member may not.
    assert_eq!(real(corp_balance(&mut alice, 1001)), 250_000.0);
    assert_eq!(real(corp_balance(&mut alice, 1000)), 0.0);
    assert_eq!(
        corp_balance(&mut bob, 1001).refusal().as_deref(),
        Some("CrpAccessDenied")
    );
    let divisions = alice
        .call("account", "GetWalletDivisionsInfo", vec![])
        .expect("GetWalletDivisionsInfo");
    let PyValue::List(rows) = divisions.value().expect("a list") else {
        panic!("a list of divisions")
    };
    assert_eq!(rows.len(), 7, "the CEO reads every division");
    let none = bob
        .call("account", "GetWalletDivisionsInfo", vec![])
        .expect("GetWalletDivisionsInfo");
    assert_eq!(none.value(), Some(PyValue::List(vec![])));

    // Alice hears of the balance without asking: `OnAccountChange('cash2',
    // corpID, balance)`, which `WalletSvc.OnAccountChange` takes for a
    // corporation division.
    let heard = until(Duration::from_secs(10), || {
        let _ = alice.pump(Duration::from_millis(200));
        let pushes = alice.take_pushes();
        push_balance(&pushes, "cash2", corp) == Some(250_000.0)
    });
    assert!(heard, "Alice never heard OnAccountChange for the division");

    // The journal is a CRowset for the CEO, a refusal for the member.
    let journal = |c: &mut Conn, key: i64| {
        c.call(
            "account",
            "GetTransactions",
            vec![
                PyValue::Int(key),
                PyValue::None,
                PyValue::None,
                PyValue::Bool(true),
            ],
        )
        .expect("GetTransactions")
    };
    assert_eq!(journal(&mut alice, 1001).refusal(), None);
    assert_eq!(
        journal(&mut bob, 1001).refusal().as_deref(),
        Some("CrpAccessDenied")
    );

    // A member without the take role cannot withdraw; the CEO can.
    let take = |c: &mut Conn, isk: f64| {
        c.call_kw(
            "account",
            "GiveCashFromCorpAccount",
            vec![PyValue::Int(BOB), PyValue::Real(isk), PyValue::Int(1001)],
            vec![kw("reason", uni("pay"))],
        )
        .expect("GiveCashFromCorpAccount")
    };
    assert_eq!(
        take(&mut bob, 1.0).refusal().as_deref(),
        Some("CrpAccessDenied")
    );
    let bob_mid = own_balance(&mut bob);
    let a = take(&mut alice, 100_000.0);
    assert_eq!(a.refusal(), None, "the CEO's withdrawal refused");
    assert_eq!(own_balance(&mut bob), bob_mid + 100_000.0);
    assert_eq!(real(corp_balance(&mut alice, 1001)), 150_000.0);

    // More than the division holds names the division.
    let a = take(&mut alice, 9_000_000.0);
    assert_eq!(a.refusal().as_deref(), Some("NotEnoughMoneyCorp"));
    assert_eq!(real(corp_balance(&mut alice, 1001)), 150_000.0);
}

#[test]
fn a_taxed_bounty_pays_the_corp_and_the_pilot_the_rest() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_501),
        pilot(BOB, "Bob", 100_000_502),
    ];
    let (addr, world) = serve_world(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let corp = found_corp(&mut alice, "Taxed Invented", "TAXD");
    join(&mut alice, &mut bob, corp);

    // The CEO sets 25% (the form sends fractions); a member cannot.
    let form = |rate: f64| {
        vec![
            uni("about"),
            uni(""),
            PyValue::Real(rate),
            PyValue::Int(1),
            PyValue::Real(0.0),
        ]
    };
    // A refusal on a moniker's first call is the bind's own failure.
    let corp_id = bob.session().get_i64("corpid").expect("a corp");
    let refused = bob
        .moniker_call(
            "corpRegistry",
            PyValue::Int(corp_id),
            "UpdateCorporation",
            form(0.9),
            vec![],
        )
        .expect_err("a plain member may not edit the corporation");
    assert!(
        format!("{refused}").contains("CrpAccessDenied"),
        "refused with the client's key: {refused}"
    );
    let a = corp_call(&mut alice, "UpdateCorporation", form(0.25));
    assert_eq!(a.refusal(), None, "UpdateCorporation refused");

    // Bob has a bounty payout due: one row, as `npc::bounty` keeps it, and
    // the world pays it as the clock would.
    let bob_before = own_balance(&mut bob);
    {
        let mut w = world.write().expect("the world lock");
        w.deltas_mut().put(
            eve_server::npc::bounty::PENDING_KIND,
            vec![BOB.to_string(), "30000001".to_owned()],
            vec!["1000000".to_owned(), "1".to_owned(), "599011:3".to_owned()],
        );
        let now = eve_server::pyutil::filetime_now();
        assert_eq!(eve_server::npc::bounty::flush(&mut w, now), 1);
        let stamped = w
            .journal()
            .entries()
            .iter()
            .rev()
            .find(|e| e.wallet() == BOB && e.entry_type_id == 85)
            .expect("Bob's bounty row")
            .clone();
        assert_eq!(stamped.amount, 750_000.0, "the row is the net");
        assert!(
            stamped
                .description
                .starts_with(&format!("{{92: [{corp}, 250000.00]")),
            "the tax is stamped for the client's derived line: {}",
            stamped.description
        );
    }
    assert_eq!(own_balance(&mut bob), bob_before + 750_000.0);
    assert_eq!(
        real(corp_balance(&mut alice, 1000)),
        250_000.0,
        "the tax is in the Master Wallet"
    );
}

#[test]
fn create_alliance_is_paid_from_the_corp_wallet_not_the_ceos() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_501)];
    let (addr, _) = serve_world(config);
    let mut alice = login(addr, "Alice");
    let corp = found_corp(&mut alice, "Fee Invented", "FEEI");
    let create = |c: &mut Conn| {
        corp_call(
            c,
            "CreateAlliance",
            vec![uni("Fee Pact"), uni("FEEP"), uni(""), uni("")],
        )
    };

    // Alice is rich and the corporation is not: refused, naming the division.
    // (A moniker's first call is fused with its bind, whose refusal surfaces
    // as an error; this one holds the moniker first.)
    assert_eq!(
        corp_call(&mut alice, "GetCorporation", vec![]).refusal(),
        None
    );
    let before = own_balance(&mut alice);
    assert!(before > 1_000_000_000.0);
    let a = create(&mut alice);
    assert_eq!(a.refusal().as_deref(), Some("NotEnoughMoneyCorp"));
    assert_eq!(own_balance(&mut alice), before, "nothing was taken");

    // The CEO funds the Master Wallet, and now the corporation pays.
    let a = deposit(&mut alice, corp, 1_000_000_000.0, 1000);
    assert_eq!(a.refusal(), None, "the deposit refused");
    let after_deposit = own_balance(&mut alice);
    assert_eq!(after_deposit, before - 1_000_000_000.0);
    let a = create(&mut alice);
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    assert!(
        a.outcome
            .session_change
            .as_ref()
            .and_then(|s| s.new_i64("allianceid"))
            .is_some(),
        "the founder's session gains allianceid"
    );
    assert_eq!(
        own_balance(&mut alice),
        after_deposit,
        "the CEO's own wallet paid nothing for the alliance"
    );
    assert_eq!(real(corp_balance(&mut alice, 1000)), 0.0);
}

#[test]
fn the_active_division_and_the_division_names_reach_the_session_and_the_row() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_501)];
    let (addr, _) = serve_world(config);
    let mut alice = login(addr, "Alice");
    let _ = found_corp(&mut alice, "Names Invented", "NAME");
    assert_eq!(alice.session().get_i64("corpAccountKey"), Some(1000));

    // `SetAccountKey` is `PerformSessionChange('corp.setaccountkey', ...)`:
    // the effect is the `corpAccountKey` change.
    let a = corp_call(&mut alice, "SetAccountKey", vec![PyValue::Int(1003)]);
    assert_eq!(a.refusal(), None, "SetAccountKey refused");
    assert_eq!(
        a.outcome
            .session_change
            .as_ref()
            .and_then(|s| s.new_i64("corpAccountKey")),
        Some(1003)
    );
    assert_eq!(alice.session().get_i64("corpAccountKey"), Some(1003));

    // The CEO names a wallet division (fourteen positionals); a bad name is
    // the client's own key.
    let names = |wallet4: &str| {
        let mut v: Vec<PyValue> = (0..14).map(|_| uni("")).collect();
        v[10] = uni(wallet4);
        v
    };
    let a = corp_call(&mut alice, "UpdateDivisionNames", names(&"x".repeat(60)));
    assert_eq!(
        a.refusal().as_deref(),
        Some("WalletCorpDivNameInvalidMaxLength")
    );
    let a = corp_call(&mut alice, "UpdateDivisionNames", names("Ammo fund"));
    assert_eq!(a.refusal(), None, "UpdateDivisionNames refused");
    let row = corp_call(&mut alice, "GetCorporation", vec![]).value;
    assert!(
        format!("{row:?}").contains("Ammo fund"),
        "the corporation row carries the new wallet division name"
    );
}
