//! SOC-D3a over a real socket: a pilot of an alliance launches a hub, flies
//! to it, locks it and runs an **Entosis Link** on it for the capture time,
//! and the claim appears; a pilot of another alliance is refused outside the
//! held hub's vulnerability window, accepted inside it, and takes the hub
//! to **reinforced** with a campaign.
//!
//! The server runs at `--time-scale 60` so the 595-second capture takes
//! about ten seconds. **Invented furniture only**: the home system is made
//! null-sec here, the star, the fuel and the link are invented; the hub's
//! type id, the link's effect id and the client's group ids appear by name.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use eve_server::entosis::{ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, EFFECT_ENTOSIS_LINK};
use eve_server::game_clock::GameClock;
use eve_server::sov_hub;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_631;
const BOB: i64 = 90_000_632;
const LINK_TYPE: i64 = 990_631_001;
const FUEL_TYPE: i64 = 990_631_002;
const LINK_GROUP: i64 = 990_631;
const EFFECT_NAME: &str = "fixtureEntosis";
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: three high and three mid slots.
const FRIGATE: i64 = 990_000_001;
/// The invented link's range (m) and cycle (ms).
const LINK_RANGE: f64 = 20_000.0;
const LINK_MS: i64 = 2_000;
const SCALE: u32 = 60;

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

/// An attribute of a `KeyVal` row.
fn attr<'a>(row: &'a PyValue, name: &str) -> Option<&'a PyValue> {
    let PyValue::Instance { state, .. } = row else {
        return None;
    };
    let PyValue::Dict(pairs) = &**state else {
        return None;
    };
    pairs
        .iter()
        .find(|(_, k)| matches!(k, PyValue::Bytes(b) if b == name.as_bytes()))
        .map(|(v, _)| v)
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
/// hub row, a fuel row and an invented Entosis Link.
fn content() -> eve_server::content::Content {
    let mut c = eve_server::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t1.0\t0.0\t1000.0\t{HUB_RADIUS}\t0\t\t\tInvented Hub Row\n\
         type\t{FUEL_TYPE}\t{}\t4\t1\t40\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Strontium\n\
         type\t{LINK_TYPE}\t{LINK_GROUP}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_ENTOSIS_LINK}\t12,16,{EFFECT_ENTOSIS_LINK}\t6=0.0;30=1.0;50=5.0;54={LINK_RANGE};73={LINK_MS}.0;{ATTR_FUEL_TYPE}={FUEL_TYPE}.0;{ATTR_FUEL_QUANTITY}=1.0\tInvented Link\n\
         effect\t{EFFECT_ENTOSIS_LINK}\t2\t0\t73\t6\t54\t0\t0\t{EFFECT_NAME}\teffects.EntosisBeam\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        LINK_GROUP + 1,
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
        "star\t40631001\t990631010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
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

/// A corp and an alliance founded by `c`; the alliance id.
fn found_alliance(c: &mut Conn, corp: (&str, &str), alliance: (&str, &str)) -> i64 {
    gm(c, "/giveisk 2000000000");
    let a = corp_call(
        c,
        "AddCorporation",
        vec![uni(corp.0), uni(corp.1), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "AddCorporation refused");
    // CORP-W: the alliance fee comes from the corp's Master Wallet.
    let corp_id = c.session().get_i64("corpid").expect("a corp");
    let d = crate::corp_wallet::deposit(c, corp_id, 1_500_000_000.0, 1000);
    assert_eq!(d.refusal(), None, "the deposit was refused");
    let a = corp_call(
        c,
        "CreateAlliance",
        vec![uni(alliance.0), uni(alliance.1), uni(""), uni("")],
    );
    assert_eq!(a.refusal(), None, "CreateAlliance refused");
    a.outcome
        .session_change
        .as_ref()
        .and_then(|s| s.new_i64("allianceid"))
        .expect("an alliance")
}

/// A frigate with a fitted link and `fuel` units of strontium in its hold,
/// and `extra` hub-sized items loaded. Docked, ready to undock.
fn outfit(c: &mut Conn, fuel: i64, hub: bool) -> (i64, Option<i64>) {
    gm(c, &format!("/createitem {FRIGATE}"));
    let ship = hangar_item(c, FRIGATE);
    c.board(ship).expect("boards");
    gm(c, &format!("/createitem {LINK_TYPE}"));
    let link = hangar_item(c, LINK_TYPE);
    let fitted = c.fit_auto(&[link]).expect("the link fits");
    let module = fitted.first().expect("a fitted link").item_id;
    c.set_online(module).expect("the link is online");
    gm(c, &format!("/createitem {FUEL_TYPE} {fuel}"));
    let stack = hangar_item(c, FUEL_TYPE);
    c.load_cargo(stack, None).expect("the fuel loads");
    let mut hub_stack = None;
    if hub {
        gm(
            c,
            &format!("/createitem {}", eve_server::sov::TYPE_INFRASTRUCTURE_HUB),
        );
        let h = hangar_item(c, eve_server::sov::TYPE_INFRASTRUCTURE_HUB);
        c.load_cargo(h, None).expect("the hub loads");
        hub_stack = Some(h);
    }
    (module, hub_stack)
}

/// Lock the hub and start the link, retrying until the ship is in range
/// (a refusal before arrival is `TargetTooFar`). The last refusal on a
/// timeout.
fn link_up(c: &mut Conn, module: i64, hub: i64, within: Duration) -> Result<(), String> {
    let mut last = String::from("never tried");
    let ok = until(within, || {
        let _ = c.pump(Duration::from_millis(200));
        let _ = c.lock_target(hub);
        match c.activate(module, (EFFECT_NAME, EFFECT_ENTOSIS_LINK), Some(hub), 1000) {
            Ok(_) => true,
            Err(e) => {
                last = e.to_string();
                false
            }
        }
    });
    if ok { Ok(()) } else { Err(last) }
}

/// Set `alliance`'s prime hour (at once, by the GM line) to an hour that
/// puts the hub inside (`inside`) or outside its vulnerability window now.
/// At `--time-scale` N the window is not aligned with the wall hour, so the
/// hours are classified by asking the server's own status line. The hour
/// chosen is the middle of the longest run of matching hours, so the window
/// does not open or close under the test within the seconds it needs (the
/// first matching hour can sit on the window's edge under load).
fn set_window(c: &mut Conn, alliance: i64, inside: bool) {
    let mut matching = [false; 24];
    for (hour, slot) in matching.iter_mut().enumerate() {
        let out = slash(c, &format!("/sov prime {alliance} {hour}"));
        assert!(out.contains("prime hour"), "{out}");
        let status = slash(c, "/sov entosis here");
        let open = status.contains("): vulnerable ");
        let shut = status.contains("): invulnerable ");
        *slot = (inside && open) || (!inside && shut);
    }
    // The longest circular run of matching hours: (start, length).
    let mut best: Option<(usize, usize)> = None;
    for start in 0..24 {
        if !matching[start] || matching[(start + 23) % 24] {
            continue;
        }
        let len = (0..24).take_while(|k| matching[(start + k) % 24]).count();
        if best.is_none_or(|(_, l)| len > l) {
            best = Some((start, len));
        }
    }
    let Some((start, len)) = best else {
        panic!(
            "no prime hour puts the hub {} its window",
            if inside { "inside" } else { "outside" }
        );
    };
    let hour = (start + (len - 1) / 2) % 24;
    let out = slash(c, &format!("/sov prime {alliance} {hour}"));
    assert!(out.contains("prime hour"), "{out}");
}

fn claim_info(c: &mut Conn) -> PyValue {
    c.call(
        "sovMgr",
        "GetSystemSovereigntyInfo",
        vec![PyValue::Int(SYSTEM)],
    )
    .expect("answered")
    .value()
    .expect("a value")
}

fn hub_row(c: &mut Conn) -> PyValue {
    let rows = c
        .call(
            "sovMgr",
            "GetSovStructuresInfoForSolarSystem",
            vec![PyValue::Int(SYSTEM)],
        )
        .expect("answered")
        .value()
        .expect("a value");
    match rows {
        PyValue::List(mut l) if !l.is_empty() => l.remove(0),
        other => panic!("a hub row: {other:?}"),
    }
}

#[test]
fn an_alliance_pilot_entoses_a_neutral_hub_into_a_claim_and_a_rival_reinforces_it() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_631),
        pilot(BOB, "Bob", 100_000_632),
    ];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&star_row());
    config.content = Some(Arc::new(content()));
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");

    let a_alliance = found_alliance(
        &mut alice,
        ("Entosis Invented", "ENTI"),
        ("Entosis Pact", "ENTP"),
    );
    let b_alliance = found_alliance(&mut bob, ("Rival Invented", "RIVI"), ("Rival Pact", "RIVP"));
    assert_ne!(a_alliance, b_alliance);

    // Alice: a frigate with a link, fuel for every cycle of the capture, and
    // a hub. Bob the same, without the hub.
    let (a_link, hub_stack) = outfit(&mut alice, 400, true);
    let hub_stack = hub_stack.expect("a hub");
    let (b_link, _) = outfit(&mut bob, 400, false);
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
    assert_eq!(claim_info(&mut alice), PyValue::None, "neutral: no claim");

    // To the hub, and the link: the ship is out of range until it arrives.
    alice.warp_to_item(hub_id, 0.0).expect("warps");
    link_up(&mut alice, a_link, hub_id, Duration::from_secs(90))
        .unwrap_or_else(|e| panic!("the link never started: {e}"));

    // The capture: the warm-up cycle, then 595 / 60 seconds. The hub's slim
    // changes on the edge, with the score the client extrapolates.
    let mut score_pushed = false;
    let claimed = until(Duration::from_secs(90), || {
        for e in alice.pump(Duration::from_millis(250)).unwrap_or_default() {
            if let Event::Push(n, a) = e
                && n == "DoDestinyUpdate"
                && a.iter().any(|x| holds(x, b"entosis_score"))
            {
                score_pushed = true;
            }
        }
        claim_info(&mut alice) != PyValue::None
    });
    assert!(claimed, "the hub was never claimed");
    assert!(score_pushed, "no entosis_score slim change was pushed");
    let info = claim_info(&mut alice);
    let text = format!("{info:?}");
    assert!(text.contains(&hub_id.to_string()), "the real hub: {text}");
    assert!(
        text.contains(&a_alliance.to_string()),
        "Alice's alliance: {text}"
    );
    let row = hub_row(&mut alice);
    assert_eq!(
        attr(&row, "allianceID"),
        Some(&PyValue::Int(a_alliance)),
        "{row:?}"
    );
    // A held hub shows its window and no campaign.
    assert_eq!(attr(&row, "campaignState"), Some(&PyValue::None));
    assert!(
        matches!(attr(&row, "vulnerabilityState"), Some(PyValue::Tuple(w)) if w.len() == 2),
        "a window: {:?}",
        attr(&row, "vulnerabilityState")
    );

    // Bob outside the window: refused. Alice's alliance's prime hour is set
    // twelve hours from now (UTC), so the hub is invulnerable.
    set_window(&mut bob, a_alliance, false);
    bob.undock().expect("undocks");
    bob.enter_space(SYSTEM).expect("a state");
    bob.warp_to_item(hub_id, 0.0).expect("warps");
    // Arrived and refused: the refusal changes from TargetTooFar to the
    // server's own notice.
    let mut refusal = String::new();
    let refused = until(Duration::from_secs(90), || {
        let _ = bob.pump(Duration::from_millis(200));
        let _ = bob.lock_target(hub_id);
        match bob.activate(
            b_link,
            (EFFECT_NAME, EFFECT_ENTOSIS_LINK),
            Some(hub_id),
            1000,
        ) {
            Ok(_) => panic!("a link started outside the window"),
            Err(e) => {
                refusal = e.to_string();
                refusal.contains("CustomNotify")
            }
        }
    });
    assert!(refused, "never refused by the window: {refusal}");
    let info = slash(&mut bob, "/sov entosis here");
    assert!(info.contains("invulnerable"), "{info}");

    // Inside the window it starts, and a completed capture reinforces.
    set_window(&mut bob, a_alliance, true);
    link_up(&mut bob, b_link, hub_id, Duration::from_secs(60))
        .unwrap_or_else(|e| panic!("Bob's link never started: {e}"));
    let reinforced = until(Duration::from_secs(90), || {
        let _ = bob.pump(Duration::from_millis(250));
        matches!(
            attr(&hub_row(&mut bob), "campaignState"),
            Some(PyValue::Tuple(_))
        )
    });
    assert!(reinforced, "the hub never reinforced");
    let row = hub_row(&mut bob);
    let Some(PyValue::Tuple(campaign)) = attr(&row, "campaignState") else {
        panic!("a campaign: {row:?}")
    };
    assert_eq!(campaign[0], PyValue::Int(2), "an IHub campaign");
    assert_eq!(campaign[1], PyValue::Int(a_alliance), "the defender");
    assert!(matches!(&campaign[3], PyValue::Dict(d) if d.len() == 2));
    // Still Alice's: reinforced, not lost.
    assert_eq!(attr(&row, "allianceID"), Some(&PyValue::Int(a_alliance)));
    let info = slash(&mut alice, "/sov entosis here");
    assert!(info.contains("reinforced"), "{info}");
    let _ = b_alliance;
}
