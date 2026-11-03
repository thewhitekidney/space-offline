//! SOC-D1 over a real socket: a CEO founds a corp and an alliance, `/sov set`
//! gives a claimable system to the alliance, and the client's own reads
//! answer the claim: `stationSvc.GetAllianceSystems` (the map),
//! `sovMgr.GetSystemSovereigntyInfo` (Show Info, the claim cache) and the
//! alliance moniker's `GetAllianceSovereigntyStructuresInfo` (Claimed
//! Systems, Sov Hubs), whose TCU and hub rows carry the same hub id. The
//! claim also arrives unasked as `ProcessSovStatusChanged`.
//!
//! The system is whichever one the fixture universe offers as claimable;
//! names, tickers and other ids are invented.

use super::common::{config, gm, login, pilot, serve, until};
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use std::time::Duration;

const ALICE: i64 = 90_000_401;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
}

fn corp_call(c: &mut Conn, method: &str, args: Vec<PyValue>) -> eve_bot::Answer {
    let corp = c
        .session()
        .get_i64("corpid")
        .expect("a corp in the session");
    c.moniker_call("corpRegistry", PyValue::Int(corp), method, args, vec![])
        .unwrap_or_else(|e| panic!("corpRegistry.{method}: {e}"))
}

#[test]
fn sov_set_is_answered_by_the_map_the_claim_cache_and_the_alliance_rows() {
    let system = {
        let u = eve_server::celestial::Universe::fixture();
        u.systems()
            .map(|s| s.id)
            .find(|id| eve_server::sov::claimable(&u, *id))
            .expect("the fixture universe has a claimable system")
    };
    let mut config = config();
    config.sandbox.characters = vec![pilot(ALICE, "Alice", 100_000_401)];
    let addr = serve(config);
    let mut alice = login(addr, "Alice");

    gm(&mut alice, "/giveisk 2000000000");
    let a = corp_call(
        &mut alice,
        "AddCorporation",
        vec![uni("Sov Invented"), uni("SOVI"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    let corp = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("corpid"))
        .expect("a new corp");
    // The fee comes out of the corporation's Master Wallet (CORP-W).
    let a = super::corp_wallet::deposit(&mut alice, corp, 1_000_000_000.0, 1000);
    assert_eq!(a.refusal(), None, "the deposit refused");
    let a = corp_call(
        &mut alice,
        "CreateAlliance",
        vec![uni("Invented Holdings"), uni("IHLD"), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    let alliance = a
        .outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance");

    // Nothing is claimed yet.
    let none = alice
        .call(
            "sovMgr",
            "GetSystemSovereigntyInfo",
            vec![PyValue::Int(system)],
        )
        .expect("GetSystemSovereigntyInfo");
    assert_eq!(none.value(), Some(PyValue::None));

    // The GM claim.
    let set = alice
        .call(
            "slash",
            "SlashCmd",
            vec![PyValue::Bytes(
                format!("/sov set {system} {alliance}").into_bytes(),
            )],
        )
        .expect("the slash is answered");
    let mut pushed = format!("{:?}", set.notifications).contains("ProcessSovStatusChanged");
    let arrived = until(Duration::from_secs(10), || {
        if !pushed {
            for e in alice.pump(Duration::from_millis(200)).unwrap_or_default() {
                if let Event::Push(name, _) = e
                    && name == "ProcessSovStatusChanged"
                {
                    pushed = true;
                }
            }
        }
        pushed
    });
    assert!(arrived, "ProcessSovStatusChanged never arrived");

    // The map's colours.
    let systems = alice
        .call("stationSvc", "GetAllianceSystems", vec![])
        .expect("GetAllianceSystems")
        .value()
        .expect("a value");
    let text = format!("{systems:?}");
    assert!(
        text.contains(&system.to_string()) && text.contains(&alliance.to_string()),
        "the claim is on the map: {text}"
    );

    // Show Info and the claim cache: a SovClaimInfo naming the alliance and
    // the owner corp.
    let info = alice
        .call(
            "sovMgr",
            "GetSystemSovereigntyInfo",
            vec![PyValue::Int(system)],
        )
        .expect("GetSystemSovereigntyInfo")
        .value()
        .expect("a value");
    let text = format!("{info:?}");
    let PyValue::ObjectEx { header, .. } = &info else {
        panic!("a new-style object: {text}")
    };
    let PyValue::Tuple(parts) = &**header else {
        panic!("{text}")
    };
    assert_eq!(
        parts[0],
        PyValue::Tuple(vec![PyValue::Global(
            eve_server::sov::SOV_CLAIM_INFO.as_bytes().to_vec()
        )]),
        "a SovClaimInfo"
    );
    assert!(text.contains(&alliance.to_string()), "{text}");
    assert!(text.contains(&corp.to_string()), "{text}");

    // The alliance window: one TCU row and one hub row, the same hub id.
    let bind = PyValue::Tuple(vec![PyValue::Int(alliance), PyValue::Int(1)]);
    let rows = alice
        .moniker_call(
            "allianceRegistry",
            bind,
            "GetAllianceSovereigntyStructuresInfo",
            vec![],
            vec![],
        )
        .expect("GetAllianceSovereigntyStructuresInfo")
        .value
        .expect("a value");
    let PyValue::Tuple(parts) = &rows else {
        panic!("a 3-tuple: {rows:?}")
    };
    assert_eq!(parts.len(), 3);
    let (tcu, hubs) = (format!("{:?}", parts[0]), format!("{:?}", parts[1]));
    let hub_id = eve_server::sov::stand_in_hub_id(system).to_string();
    assert!(
        tcu.contains(&hub_id),
        "the TCU row carries the hub id: {tcu}"
    );
    assert!(
        hubs.contains(&hub_id),
        "the hub row carries the hub id: {hubs}"
    );
}
