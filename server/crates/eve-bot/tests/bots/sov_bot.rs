//! SOC-D5 over a real socket: a bot pilot of an alliance, through
//! [`eve_bot::sov_bot::turn`] alone (no GM line after the setup), reads the
//! sov map as the client does, plans a claim on a neutral hub, fits the link
//! it owns, loads strontium, flies to the hub and runs the entosis until the
//! hub is its alliance's; and a second pilot hauls fuel blocks to the hub of
//! its alliance with the client's own `/boostsov` line and sets the
//! alliance's prime hour.
//!
//! The server runs at `--time-scale 60`. **Invented furniture only**: the
//! system is made null-sec here; the star, the fuel, the link and the blocks
//! are invented; the hub's type id, the effect id and the client's group ids
//! appear by name.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::Conn;
use eve_bot::brain::{Memory, Policy};
use eve_bot::sov_bot::{self, Ctx, Duty, Kind, SovHall};
use eve_bot::travel::Map;
use eve_marshal::PyValue;
use eve_server::entosis::{ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, EFFECT_ENTOSIS_LINK};
use eve_server::game_clock::GameClock;
use eve_server::sov_hub;
use std::sync::Arc;
use std::time::Duration;

const ALICE: i64 = 90_000_651;
const BOB: i64 = 90_000_652;
const LINK_TYPE: i64 = 990_651_001;
const FUEL_TYPE: i64 = 990_651_002;
const BLOCK_TYPE: i64 = 990_651_003;
const LINK_GROUP: i64 = 990_651;
const EFFECT_NAME: &str = "fixtureEntosis";
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: three high and three mid slots.
const FRIGATE: i64 = 990_000_001;
/// The invented link's range (m) and cycle (ms).
const LINK_RANGE: f64 = 20_000.0;
const LINK_MS: i64 = 2_000;
const SCALE: u32 = 60;
/// Blocks in the hauler's hangar.
const BLOCKS: i64 = 300;

fn uni(s: &str) -> PyValue {
    PyValue::Unicode(s.to_owned())
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
         type\t{BLOCK_TYPE}\t{}\t4\t1\t1\t0.01\t0.0\t1.0\t1.0\t0\t\t\tInvented Block\n\
         effect\t{EFFECT_ENTOSIS_LINK}\t2\t0\t73\t6\t54\t0\t0\t{EFFECT_NAME}\teffects.EntosisBeam\n",
        eve_server::typedata::FIXTURE,
        eve_server::sov::TYPE_INFRASTRUCTURE_HUB,
        sov_hub::GROUP_INFRASTRUCTURE_HUB,
        sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        LINK_GROUP + 1,
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

/// A pilot's memory with the sov role and the process's hall.
fn memory(name: &str, hall: &Arc<SovHall>) -> Memory {
    let mut policy = Policy::human();
    policy.role = &sov_bot::ROLE;
    policy.sov = Some(Arc::clone(hall));
    Memory::new(name, policy)
}

fn serve_world() -> std::net::SocketAddr {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_651),
        pilot(BOB, "Bob", 100_000_652),
    ];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&star_row());
    config.content = Some(Arc::new(content()));
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    serve(config)
}

/// The map a bot routes over: the one null system and the home station.
fn map_of(c: &Conn) -> Map {
    let station = c.session().station_id().expect("docked at home");
    Map::new()
        .with_security(SYSTEM, -0.4)
        .with_station(station, SYSTEM)
}

#[test]
fn a_bot_pilot_plans_fits_flies_and_entoses_a_neutral_hub_into_its_alliances_claim() {
    let addr = serve_world();
    let mut alice = login(addr, "Alice");
    let a_alliance = found_alliance(
        &mut alice,
        ("Sov Bots Invented", "SBOT"),
        ("Sov Bots Pact", "SBPT"),
    );
    // The setup is a person's: a frigate, a link and strontium in the
    // hangar (the bot buys or owns them; here they are owned), and a
    // neutral hub in the system (a GM claim and release leaves one).
    gm(&mut alice, &format!("/createitem {FRIGATE}"));
    let ship = hangar_item(&mut alice, FRIGATE);
    alice.board(ship).expect("boards");
    gm(&mut alice, &format!("/createitem {LINK_TYPE}"));
    gm(&mut alice, &format!("/createitem {FUEL_TYPE} 400"));
    let out = slash(&mut alice, &format!("/sov set {SYSTEM} {a_alliance}"));
    assert!(out.contains("claimed"), "{out}");
    let out = slash(&mut alice, &format!("/sov clear {SYSTEM}"));
    assert!(!out.is_empty());
    assert_eq!(claim_info(&mut alice), PyValue::None, "neutral again");

    let hall = Arc::new(SovHall::new());
    let mut mem = memory("Alice", &hall);
    let map = map_of(&alice);
    let ctx = Ctx {
        map: Some(&map),
        content: Some(&content()),
        hall: &hall,
        relation: None,
        society: None,
    };
    let note = sov_bot::turn(&mut alice, &mut mem, &ctx).expect("a sov turn");

    let info = claim_info(&mut alice);
    let text = format!("{info:?}");
    assert!(
        text.contains(&a_alliance.to_string()),
        "the bot's alliance holds the hub: {text}\nnote: {note}\nlog: {:?}",
        hall.log(a_alliance)
    );
    let log = hall.log(a_alliance);
    assert!(
        log.iter()
            .any(|l| l.contains(&format!("{:?}", Kind::Claim))),
        "the plan was a claim: {log:?}"
    );
    assert!(note.contains("sov entosis"), "{note}");
    // Held now: the next plan has nothing to claim, and the hub is ours to fuel.
    let hubs = sov_bot::observe(&mut alice, &map, SYSTEM, eve_server::pyutil::filetime_now())
        .expect("the view");
    assert_eq!(hubs.len(), 1, "{hubs:?}");
    assert_eq!(hubs[0].holder, Some(a_alliance));
}

#[test]
fn the_view_reads_a_rivals_hub_and_the_planner_attacks_it_only_when_it_is_red() {
    let addr = serve_world();
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");
    let a_alliance = found_alliance(
        &mut alice,
        ("View A Invented", "VWAI"),
        ("View A Pact", "VWAP"),
    );
    let b_alliance = found_alliance(
        &mut bob,
        ("View B Invented", "VWBI"),
        ("View B Pact", "VWBP"),
    );
    let out = slash(&mut alice, &format!("/sov set {SYSTEM} {b_alliance}"));
    assert!(out.contains("claimed"), "{out}");
    let map = map_of(&alice);
    let now = eve_server::pyutil::filetime_now();
    let hubs = sov_bot::observe(&mut alice, &map, SYSTEM, now).expect("the view");
    assert_eq!(hubs.len(), 1, "{hubs:?}");
    let h = &hubs[0];
    assert_eq!(h.holder, Some(b_alliance));
    assert!(
        matches!(
            h.phase,
            sov_bot::Phase::Open { .. } | sov_bot::Phase::Shut { .. }
        ),
        "a held hub shows its window: {:?}",
        h.phase
    );
    let jumps = |_: i64| Some(0);
    let red = |a: i64| if a == b_alliance { -1.0 } else { 0.0 };
    let blue = |_: i64| 0.0;
    let facts = |relation: &dyn Fn(i64) -> f64, linkers| {
        sov_bot::plan(&sov_bot::Facts {
            me: a_alliance,
            hubs: &hubs,
            jumps: &jumps,
            relation,
            bloc_enemy: &|_| false,
            bloc_mate: &|_| false,
            now,
            linkers,
            previous: None,
        })
    };
    assert_eq!(
        facts(&blue, 6),
        None,
        "a stranger who is not red is left alone"
    );
    let attack = facts(&red, 6);
    // The window may be shut for hours: the schedule waits for it.
    match h.phase {
        sov_bot::Phase::Open { .. } => {
            assert_eq!(attack.map(|t| t.kind), Some(Kind::Attack));
        }
        sov_bot::Phase::Shut { opens } => {
            assert_eq!(
                attack.is_some(),
                opens - now <= sov_bot::LEAD_SECS * sov_bot::TICKS
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn a_bot_hauls_fuel_blocks_to_its_alliances_hub_and_sets_the_prime_hour() {
    let addr = serve_world();
    let mut bob = login(addr, "Bob");
    let b_alliance = found_alliance(
        &mut bob,
        ("Fuel Bots Invented", "FBOT"),
        ("Fuel Bots Pact", "FBPT"),
    );
    gm(&mut bob, &format!("/createitem {FRIGATE}"));
    let ship = hangar_item(&mut bob, FRIGATE);
    bob.board(ship).expect("boards");
    gm(&mut bob, &format!("/createitem {BLOCK_TYPE} {BLOCKS}"));
    let out = slash(&mut bob, &format!("/sov set {SYSTEM} {b_alliance}"));
    assert!(out.contains("claimed"), "{out}");
    let before = slash(&mut bob, &format!("/sovfuel status {SYSTEM}"));
    assert!(
        before.contains(" 0 fuel blocks"),
        "an empty bay first: {before}"
    );

    let hall = Arc::new(SovHall::new());
    let mut mem = memory("Bob", &hall);
    let map = map_of(&bob);
    let ctx = Ctx {
        map: Some(&map),
        content: Some(&content()),
        hall: &hall,
        relation: None,
        society: None,
    };
    let note = sov_bot::turn(&mut bob, &mut mem, &ctx).expect("a sov turn");
    let who = bob.session().char_id().expect("a character");
    assert!(
        matches!(hall.duty(b_alliance, who), Duty::Haul(_)),
        "a pilot with no link hauls: {note}"
    );
    let after = until(Duration::from_secs(20), || {
        slash(&mut bob, &format!("/sovfuel status {SYSTEM}"))
            .contains(&format!(" {BLOCKS} fuel blocks"))
    });
    assert!(
        after,
        "the blocks reached the hub: {}\nnote: {note}",
        slash(&mut bob, &format!("/sovfuel status {SYSTEM}"))
    );

    // The prime hour: the CEO of the executor corp asked for the hash hour.
    let info = bob
        .moniker_call(
            "allianceRegistry",
            PyValue::Int(b_alliance),
            "GetPrimeTimeInfo",
            vec![],
            vec![],
        )
        .expect("answered")
        .value
        .expect("a value");
    let pending = attr(&info, "newPrimeHour").cloned();
    let current = attr(&info, "currentPrimeHour").cloned();
    let want = PyValue::Int(sov_bot::prime_hour_for(b_alliance));
    assert!(
        pending.as_ref() == Some(&want) || current.as_ref() == Some(&want),
        "the prime hour: {info:?}\nnote: {note}"
    );
}
