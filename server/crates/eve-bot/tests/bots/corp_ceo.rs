//! SOC-C3 over a real socket: a CEO hands the seat to a member, whose
//! session gains roles without asking; and a lone CEO closes the
//! corp and is back in the NPC corp.
//!
//! The calls are the client's own (`docs/lanes/soc-c3.md`): `corpRegistry`
//! through its moniker on the corp id, `ResignFromCEO(None)` for the list
//! of successors, then `ResignFromCEO(pick)`. Names, tickers and ids are
//! invented. The same limitation as every test here: both ends marshal with
//! `eve-marshal`.

use super::common::{config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::time::Duration;

const ALICE: i64 = 90_000_311;
const BOB: i64 = 90_000_312;
const CAROL: i64 = 90_000_313;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
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
    gm(c, "/giveisk 2000000");
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

fn ints(v: &Option<PyValue>) -> Vec<i64> {
    match v {
        Some(PyValue::List(items)) => items
            .iter()
            .filter_map(|x| match x {
                PyValue::Int(n) => Some(*n),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn a_ceo_resigns_to_a_member_whose_session_then_holds_the_ceo_roles() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_311),
        pilot(BOB, "Bob", 100_000_312),
    ];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");

    let corp = found_corp(&mut alice, "Succession Invented", "SUCC");

    // Alice invites Bob; Bob accepts (status 2) and joins.
    let a = corp_call(&mut alice, "InsertInvitation", vec![PyValue::Int(BOB)]);
    assert_eq!(a.refusal(), None, "InsertInvitation refused");
    let mine = corp_call(&mut bob, "GetMyApplications", vec![]);
    let id = mine
        .value
        .as_ref()
        .and_then(first_application_id)
        .expect("Bob's invitation");
    let a = corp_call(
        &mut bob,
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
    // A joiner holds no roles. (Which roles a new CEO gains is an [A] in
    // `service/corp.rs`, `CEO_ROLES`; this test only asks that some arrive.)
    assert_eq!(bob.session().get_i64("rolesAtAll").unwrap_or(0), 0);

    // The first call lists the successors; the second names one.
    let listed = corp_call(&mut alice, "ResignFromCEO", vec![PyValue::None]);
    assert_eq!(listed.refusal(), None, "ResignFromCEO(None) refused");
    assert_eq!(ints(&listed.value), vec![BOB]);
    let a = corp_call(&mut alice, "ResignFromCEO", vec![PyValue::Int(BOB)]);
    assert_eq!(a.refusal(), None, "ResignFromCEO(Bob) refused");

    // Bob did not call anything: his roles arrive by session change.
    let promoted = until(Duration::from_secs(10), || {
        let _ = bob.pump(Duration::from_millis(200));
        bob.session().get_i64("rolesAtAll").unwrap_or(0) != 0
    });
    assert!(promoted, "Bob's session never gained the CEO's roles");

    // And Bob is the CEO now: the succession question is his to ask, and
    // Alice is his candidate.
    let theirs = corp_call(&mut bob, "GetNumberOfPotentialCEOs", vec![]);
    assert_eq!(ints(&theirs.value), vec![ALICE]);
    let not_hers = corp_call(&mut alice, "ResignFromCEO", vec![PyValue::None]);
    assert_eq!(not_hers.refusal().as_deref(), Some("OnlyCEOCanResign"));
}

#[test]
fn a_lone_ceo_resigning_closes_the_corp() {
    let mut config = config();
    config.sandbox.characters = vec![pilot(CAROL, "Carol", 100_000_313)];
    let addr = serve(config);
    let mut carol = login(addr, "Carol");
    let npc_corp = carol.session().get_i64("corpid").expect("an NPC corp");
    let corp = found_corp(&mut carol, "Solitude Invented", "SOLO");
    assert_ne!(corp, npc_corp);

    let a = corp_call(&mut carol, "ResignFromCEO", vec![PyValue::None]);
    assert_eq!(a.refusal(), None, "ResignFromCEO(None) refused");
    assert_eq!(a.value, Some(PyValue::None), "a close answers None");
    let back = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"));
    assert_eq!(back, Some(npc_corp), "Carol is back in her NPC corp");
    assert_eq!(carol.session().get_i64("rolesAtAll"), Some(0));

    // The corp is closed: nobody may apply to it.
    let a = corp_call(
        &mut carol,
        "InsertApplication",
        vec![PyValue::Int(corp), uni("again?")],
    );
    assert!(a.refusal().is_some(), "a closed corp takes no applications");
}

/// CORP-T: a CEO whose roles predate the corp wallets (director and
/// personnel manager only) logs in again and the login change carries the
/// wallet roles, so the Corporation wallet tab opens. The old state is made
/// the way a player could: the CEO edits their own roles down.
#[test]
fn an_old_style_ceo_gains_the_wallet_roles_on_login() {
    const DIRECTOR: i64 = 1;
    const PERSONNEL_MANAGER: i64 = 128;
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_311)];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    found_corp(&mut alice, "Old Guard Invented", "OLDG");
    let full = alice.session().get_i64("rolesAtAll").unwrap_or(0);
    assert_ne!(full & eve_server::corp_wallet::CEO_WALLET_ROLES, 0);

    // `UpdateMember(charID, title, division, squadron, roles, ...)`.
    let mut args = vec![
        PyValue::Int(ALICE),
        PyValue::None,
        PyValue::None,
        PyValue::None,
    ];
    args.push(PyValue::Int(DIRECTOR | PERSONNEL_MANAGER));
    args.extend(std::iter::repeat_n(PyValue::Int(0), 7));
    args.extend([PyValue::None, PyValue::None, PyValue::None]);
    let a = corp_call(&mut alice, "UpdateMember", args);
    assert_eq!(a.refusal(), None, "UpdateMember refused");
    let aged = until(Duration::from_secs(10), || {
        let _ = alice.pump(Duration::from_millis(200));
        alice.session().get_i64("rolesAtAll") == Some(DIRECTOR | PERSONNEL_MANAGER)
    });
    assert!(aged, "the CEO's roles never dropped to the old set");
    drop(alice);

    // Signing in again heals them before the login change is built.
    let mut again = login(addr, "Alice");
    assert_eq!(again.session().get_i64("rolesAtAll"), Some(full));

    // And the title tab opens: sixteen rows, one edit, one echo.
    let titles = corp_call(&mut again, "GetTitles", vec![]);
    let PyValue::Dict(rows) = titles.value.expect("an answer") else {
        panic!("GetTitles is a mapping")
    };
    assert_eq!(rows.len(), 16);
    let named = corp_call(
        &mut again,
        "UpdateTitle",
        std::iter::once(PyValue::Int(2))
            .chain([uni("Invented Title")])
            .chain(std::iter::repeat_n(PyValue::Int(0), 8))
            .collect(),
    );
    assert_eq!(named.refusal(), None, "UpdateTitle refused");
}
