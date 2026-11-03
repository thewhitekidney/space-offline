//! SOC-D3b over a real socket: a held hub is **reinforced**, its campaign
//! starts (the GM line, at `--time-scale 60`), **command nodes appear** in
//! the pilots' park with the slim keys the client reads, an attacking
//! alliance's pilot flies to the nodes one after another, runs an Entosis
//! Link on each until it is captured, the campaign bar moves a step per
//! node and is pushed, and when the defenders' share is spent the **hub is
//! destroyed and the claim lapses**, with the managers told.
//!
//! **Invented furniture only**: the home system is made null-sec here, the
//! star, the fuel and the link are invented; the hub's type id, the node's,
//! the link's effect id and the client's group ids appear by name.

use super::common::{SYSTEM, config, gm, login, pilot, serve, until};
use eve_bot::{Conn, Event};
use eve_marshal::PyValue;
use eve_server::command_nodes::{INITIAL_NODES, NODE_ID_BASE, NODE_ID_END};
use eve_server::entosis::{ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, EFFECT_ENTOSIS_LINK};
use eve_server::game_clock::GameClock;
use eve_server::sov_hub;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ALICE: i64 = 90_000_641;
const BOB: i64 = 90_000_642;
const LINK_TYPE: i64 = 990_641_001;
const FUEL_TYPE: i64 = 990_641_002;
const LINK_GROUP: i64 = 990_641;
const EFFECT_NAME: &str = "fixtureEntosis";
const HUB_RADIUS: f64 = 10_000.0;
const HUB_X: f64 = 400_000.0;
/// `typedata::FIXTURE`'s frigate: three high and three mid slots.
const FRIGATE: i64 = 990_000_001;
/// The invented link's range (m) and cycle (ms).
const LINK_RANGE: f64 = 20_000.0;
const LINK_MS: i64 = 1_000;
const SCALE: u32 = 60;

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
        "star\t40641001\t990641010\t{radius}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
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

/// A frigate with a fitted link and `fuel` units of strontium in its hold.
/// Docked, ready to undock; the link's module id.
fn outfit(c: &mut Conn, fuel: i64) -> i64 {
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
    module
}

/// Pump a connection and count the structure pushes it carried. **Not
/// load-bearing**: a push that arrives while a call waits for its answer is
/// folded into that call's outcome, so this undercounts; the assertions
/// that matter read the park and the stores.
#[derive(Default)]
struct Seen {
    /// `OnSolarSystemSovStructuresUpdated` carrying a `changes` dict.
    bar_pushes: usize,
}

impl Seen {
    fn pump(&mut self, c: &mut Conn, within: Duration) {
        for e in c.pump(within).unwrap_or_default() {
            if let Event::Push(name, args) = e
                && name == "OnSolarSystemSovStructuresUpdated"
                && matches!(args.get(2), Some(PyValue::Dict(d)) if !d.is_empty())
            {
                self.bar_pushes += 1;
            }
        }
    }
}

/// The command nodes in `c`'s park, `(ball id, type id, owner)` as the
/// slim told it, nearest the ego first.
fn park_nodes(c: &mut Conn) -> Vec<(i64, Option<i64>, Option<i64>)> {
    let w = c.world_now();
    let Some(me) = w.ego_ball().map(|e| e.motion.position) else {
        return Vec::new();
    };
    let mut v: Vec<(f64, i64, Option<i64>, Option<i64>)> = w
        .balls()
        .filter(|b| (NODE_ID_BASE..NODE_ID_END).contains(&b.ball_id))
        .map(|b| {
            (
                eve_server::space_state::distance(me, b.motion.position),
                b.ball_id,
                b.slim.and_then(|s| s.type_id),
                b.slim.and_then(|s| s.owner_id),
            )
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, id, t, o)| (id, t, o)).collect()
}

/// The notification types in `c`'s notification centre.
fn notice_types(c: &mut Conn) -> Vec<i64> {
    let rows = c
        .call_kw(
            "notificationMgr",
            "GetAllNotifications",
            vec![],
            vec![(PyValue::Int(0), PyValue::Bytes(b"fromID".to_vec()))],
        )
        .expect("answered")
        .value();
    let Some(PyValue::List(rows)) = rows else {
        panic!("a list: {rows:?}")
    };
    rows.iter()
        .filter_map(|r| match attr(r, "typeID") {
            Some(PyValue::Int(t)) => Some(*t),
            _ => None,
        })
        .collect()
}

/// Lock `node` and start the link, retrying until the ship is in range (a
/// refusal before arrival is `TargetTooFar`). The last refusal on a timeout.
fn link_up(
    c: &mut Conn,
    seen: &mut Seen,
    module: i64,
    node: i64,
    within: Duration,
) -> Result<(), String> {
    let mut last = String::from("never tried");
    let ok = until(within, || {
        seen.pump(c, Duration::from_millis(200));
        let _ = c.lock_target(node);
        match c.activate(module, (EFFECT_NAME, EFFECT_ENTOSIS_LINK), Some(node), 1000) {
            Ok(_) => true,
            Err(e) => {
                last = e.to_string();
                false
            }
        }
    });
    if ok { Ok(()) } else { Err(last) }
}

/// The campaign line of `/sov nodes`: `(fighting, bar, node ids)`, or `None`
/// when the system has no campaign.
type Fight = (bool, Vec<(i64, f64)>, Vec<i64>);

fn fight_of(c: &mut Conn) -> Option<Fight> {
    let text = slash(c, &format!("/sov nodes {SYSTEM}"));
    if text.contains("no campaign") {
        return None;
    }
    let fighting = text.contains("(fighting)");
    let bar = text
        .split("bar [")
        .nth(1)
        .and_then(|r| r.split(']').next())
        .map(|b| {
            b.split_whitespace()
                .filter_map(|p| {
                    let (t, s) = p.rsplit_once(':')?;
                    Some((t.parse().ok()?, s.parse().ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let nodes = text
        .split("nodes: ")
        .nth(1)
        .map(|r| {
            r.split(", ")
                .filter_map(|n| n.split_whitespace().next()?.parse::<i64>().ok())
                .filter(|id| (NODE_ID_BASE..NODE_ID_END).contains(id))
                .collect()
        })
        .unwrap_or_default();
    Some((fighting, bar, nodes))
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

fn hub_row(c: &mut Conn) -> Option<PyValue> {
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
        PyValue::List(mut l) if !l.is_empty() => Some(l.remove(0)),
        _ => None,
    }
}

#[test]
fn an_attacking_pilot_entoses_command_nodes_until_the_defenders_hub_falls() {
    let mut config = config();
    config.sandbox.characters = vec![
        pilot(ALICE, "Alice", 100_000_641),
        pilot(BOB, "Bob", 100_000_642),
    ];
    config.sandbox.celestials = eve_server::celestial::Catalogue::parse(&star_row());
    config.content = Some(Arc::new(content()));
    config.time_scale = GameClock::new(SCALE).expect("a scale");
    let addr = serve(config);
    let mut alice = login(addr, "Alice");
    let mut bob = login(addr, "Bob");

    let a_alliance = found_alliance(
        &mut alice,
        ("Defender Invented", "DEFI"),
        ("Defender Pact", "DEFP"),
    );
    let b_alliance = found_alliance(
        &mut bob,
        ("Attacker Invented", "ATKI"),
        ("Attacker Pact", "ATKP"),
    );
    assert_ne!(a_alliance, b_alliance);

    // Bob: a frigate with a link and fuel for every cycle of every capture.
    let b_link = outfit(&mut bob, 400);
    bob.undock().expect("undocks");
    bob.enter_space(SYSTEM).expect("a state");
    let mut seen = Seen::default();
    let t0 = Instant::now();

    // The GM gives Alice's alliance the system and reinforces the hub at
    // once; the campaign begins on the next tick of the world clock.
    let out = slash(&mut alice, &format!("/sov set {SYSTEM} {a_alliance}"));
    assert!(!out.starts_with("usage"), "{out}");
    let out = slash(&mut alice, &format!("/sov reinforce {SYSTEM} now"));
    assert!(out.starts_with("reinforced"), "{out}");
    let mut fight = None;
    let began = until(Duration::from_secs(30), || {
        seen.pump(&mut bob, Duration::from_millis(250));
        fight = fight_of(&mut alice).filter(|(on, _, nodes)| *on && nodes.len() >= INITIAL_NODES);
        fight.is_some()
    });
    assert!(began, "the campaign never began or spawned its nodes");
    let (_, bar, nodes) = fight.expect("a fight");
    assert!(nodes.len() >= INITIAL_NODES, "{nodes:?}");
    assert_eq!(bar.len(), 2, "{bar:?}");
    assert!(
        bar.iter()
            .any(|(t, s)| *t == a_alliance && (*s - 0.6).abs() < 1e-6),
        "the defender holds 0.6: {bar:?}"
    );
    // Bob, in space, has every node in his park, typed and owned as the
    // client reads it; the dashboard row carries the campaign.
    seen.pump(&mut bob, Duration::from_millis(500));
    let parked = park_nodes(&mut bob);
    for id in &nodes {
        assert!(
            parked.iter().any(|(p, _, _)| p == id),
            "node {id} is not in Bob's park: {parked:?}"
        );
    }
    assert!(
        parked.iter().all(
            |(_, t, o)| *t == Some(eve_server::command_nodes::TYPE_COMMAND_NODE)
                && *o == Some(eve_server::sov::NEUTRAL_HUB_OWNER)
        ),
        "{parked:?}"
    );
    let row = hub_row(&mut bob).expect("the hub row");
    assert!(
        matches!(attr(&row, "campaignState"), Some(PyValue::Tuple(c)) if c[0] == PyValue::Int(2)
            && c[1] == PyValue::Int(a_alliance)),
        "{row:?}"
    );
    assert_ne!(
        claim_info(&mut bob),
        PyValue::None,
        "Alice's alliance holds the system while the fight is on"
    );
    eprintln!("[node_fight] fight on after {:?}", t0.elapsed());

    // The GM lever: six nodes are taken by hand (the real captures below take
    // half a minute of flying each), which leaves the defenders 0.18.
    const BY_HAND: usize = 6;
    let mut taken = 0;
    while taken < BY_HAND {
        let (_, _, nodes) = fight_of(&mut alice).expect("the fight is on");
        let Some(id) = nodes.first().copied() else {
            let out = slash(&mut alice, &format!("/sov nodes spawn {SYSTEM}"));
            assert!(out.starts_with("node "), "{out}");
            continue;
        };
        let out = slash(&mut alice, &format!("/sov nodes capture {id} attackers"));
        assert!(out.contains("captured for the attackers"), "{out}");
        taken += 1;
    }
    let (_, bar, _) = fight_of(&mut alice).expect("the fight is on");
    let share = bar
        .iter()
        .find(|(t, _)| *t == a_alliance)
        .map_or(f64::NAN, |(_, s)| *s);
    let step = eve_server::command_nodes::NODE_STEP;
    let left = (share / step).ceil() as usize;
    assert!(
        (share - (0.6 - BY_HAND as f64 * step)).abs() < 1e-6 && left >= 2,
        "the defenders hold {share} of the bar: {bar:?}"
    );

    // The fight: fly to the nearest node, link it, wait for it to be
    // captured, repeat until the defenders' share is spent and the campaign
    // is gone.
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut captured = 0usize;
    let mut last_bar = bar;
    let mut current: Option<i64> = None;
    let mut ended = false;
    while Instant::now() < deadline {
        seen.pump(&mut bob, Duration::from_millis(250));
        let Some((_, bar_now, nodes)) = fight_of(&mut alice) else {
            ended = true;
            break;
        };
        if let Some(id) = current.filter(|id| !nodes.contains(id)) {
            // Captured: the link stopped with the node, and the ball left.
            captured += 1;
            current = None;
            eprintln!(
                "[node_fight] node {captured} captured after {:?}",
                t0.elapsed()
            );
            let gone = until(Duration::from_secs(10), || {
                seen.pump(&mut bob, Duration::from_millis(250));
                !park_nodes(&mut bob).iter().any(|(p, _, _)| *p == id)
            });
            assert!(gone, "node {id} was captured but is still in Bob's park");
            assert!(
                bar_now.iter().any(|(t, s)| *t == b_alliance && *s > 0.0),
                "Bob's alliance has a share of the bar: {bar_now:?}"
            );
        }
        last_bar = bar_now;
        if current.is_none() {
            let Some((next, _, _)) = park_nodes(&mut bob)
                .into_iter()
                .find(|(id, _, _)| nodes.contains(id))
            else {
                continue;
            };
            bob.warp_to_item(next, 0.0).expect("warps");
            link_up(&mut bob, &mut seen, b_link, next, Duration::from_secs(90))
                .unwrap_or_else(|e| panic!("the link on node {next} never started: {e}"));
            current = Some(next);
        }
    }
    assert!(
        ended,
        "the fight never ended (bar {last_bar:?}, {captured} captured)"
    );
    assert!(
        captured + 1 >= left,
        "{captured} nodes entosed by link, {left} needed"
    );

    // The result: the hub is destroyed and the claim lapsed; no node is left
    // in the park.
    assert_eq!(claim_info(&mut bob), PyValue::None, "the claim lapsed");
    let info = slash(&mut alice, &format!("/sov info {SYSTEM}"));
    assert!(info.contains("no hub"), "the hub is gone: {info}");
    for _ in 0..4 {
        seen.pump(&mut bob, Duration::from_millis(250));
    }
    assert!(park_nodes(&mut bob).is_empty(), "nodes outlived the fight");
    // The managers on both sides were told the hub fell, and the defender
    // that the claim was lost.
    let hers = notice_types(&mut alice);
    let his = notice_types(&mut bob);
    assert!(
        hers.contains(&eve_server::entosis::kind::STRUCTURE_DESTROYED),
        "Alice: {hers:?}"
    );
    assert!(
        hers.contains(&eve_server::entosis::kind::ALLIANCE_CLAIM_LOST),
        "Alice: {hers:?}"
    );
    assert!(
        hers.contains(&eve_server::entosis::kind::COMMAND_NODE_EVENT_STARTED),
        "Alice: {hers:?}"
    );
    assert!(
        hers.contains(&eve_server::entosis::kind::ENTOSIS_CAPTURE_STARTED),
        "Alice: {hers:?}"
    );
    assert!(
        his.contains(&eve_server::entosis::kind::STRUCTURE_DESTROYED),
        "Bob: {his:?}"
    );
    // A GM-started campaign names no attackers at the start, so Bob (who
    // became one by capturing) hears only that the hub fell.
    eprintln!(
        "[node_fight] done after {:?}, {} bar pushes seen",
        t0.elapsed(),
        seen.bar_pushes
    );
}
