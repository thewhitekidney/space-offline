//! SOC-C1 over a real socket: one CEO founds a corp and an alliance, a
//! second CEO's corp applies, the first accepts, and the second pilot's
//! session gains `allianceid` without asking for it.
//!
//! The calls are the client's own (`docs/lanes/soc-c1.md` §2): the corp
//! registry through its moniker on the corp id, the alliance registry through
//! its moniker on `(allianceID, 1)`. Names, tickers and ids are invented. The
//! same limitation as every test here: both ends marshal with `eve-marshal`.

use super::common::{config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_marshal::PyValue;
use std::time::Duration;

const ALICE: i64 = 90_000_301;
const BOB: i64 = 90_000_302;

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

/// Found a corp: the create form's first two positionals are enough.
fn found_corp(c: &mut Conn, name: &str, ticker: &str) -> i64 {
    gm(c, "/giveisk 2000000000");
    let a = corp_call(
        c,
        "AddCorporation",
        vec![uni(name), uni(ticker), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("the founding moves the session to the new corp");
    assert!(corp > 98_000_000);
    corp
}

#[test]
fn a_ceo_founds_an_alliance_and_a_second_corp_joins_it() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_301),
        pilot(BOB, "Bob", 100_000_302),
    ];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");

    let alpha = found_corp(&mut alice, "Alpha Invented", "ALPH");
    let bravo = found_corp(&mut bob, "Bravo Invented", "BRAV");

    // The fee is the corporation's to pay (CORP-W): Alice funds the Master
    // Wallet first, as a CEO does from the Wallet window.
    let a = super::corp_wallet::deposit(&mut alice, alpha, 1_000_000_000.0, 1000);
    assert_eq!(a.refusal(), None, "the deposit refused");

    // Alice founds the alliance: the answer carries `allianceid`.
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Invented Pact"), uni("INVP"), uni("about"), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("the founder's session gains allianceid");
    assert!((99_000_000..=99_999_999).contains(&alliance));
    assert_eq!(alice.session().get_i64("allianceid"), Some(alliance));

    // Bob's corp applies.
    let a = corp_call(
        &mut bob,
        "ApplyToJoinAlliance",
        vec![PyValue::Int(alliance), uni("let us in")],
    );
    assert_eq!(a.refusal(), None, "ApplyToJoinAlliance refused");

    // Alice sees it on the alliance moniker and accepts (2 =
    // allianceApplicationAccepted).
    let bind = PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]);
    let apps = alice
        .moniker_call(
            "allianceRegistry",
            bind.clone(),
            "GetApplications",
            vec![],
            vec![],
        )
        .expect("GetApplications");
    assert!(
        format!("{:?}", apps.value).contains(&bravo.to_string()),
        "Bravo's application is listed: {:?}",
        apps.value
    );
    let a = alice
        .moniker_call(
            "allianceRegistry",
            bind.clone(),
            "UpdateApplication",
            vec![PyValue::Int(bravo), uni("welcome"), PyValue::Int(2)],
            vec![],
        )
        .expect("UpdateApplication");
    assert_eq!(a.refusal(), None, "UpdateApplication refused");

    // Bob did not call anything: the change arrives on its own.
    let joined = until(Duration::from_secs(10), || {
        let _ = bob.pump(Duration::from_millis(200));
        bob.session().get_i64("allianceid") == Some(alliance)
    });
    assert!(joined, "Bob's session never gained allianceid {alliance}");

    // Both corps are members, and the alliance has a name for the client.
    let members = alice
        .moniker_call("allianceRegistry", bind, "GetMembers", vec![], vec![])
        .expect("GetMembers");
    assert!(format!("{:?}", members.value).contains(&bravo.to_string()));
    let owners = alice
        .call(
            "config",
            "GetMultiOwnersEx",
            vec![PyValue::List(vec![PyValue::Int(alliance)])],
        )
        .expect("GetMultiOwnersEx");
    let text = format!("{:?}", owners.result);
    let as_str = format!("{:?}", PyValue::Bytes(b"Invented Pact".to_vec()));
    let as_unicode = format!("{:?}", uni("Invented Pact"));
    let bytes: Vec<String> = b"Invented Pact".iter().map(u8::to_string).collect();
    assert!(
        text.contains(&as_str) || text.contains(&as_unicode) || text.contains(&bytes.join(", ")),
        "the alliance is named: {text}"
    );
}
