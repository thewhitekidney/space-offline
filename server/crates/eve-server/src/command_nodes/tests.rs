//! Tests for the command nodes and the node fight: the pure rules (the
//! decision table, the score, the bar, the placement) at scale 1 and at
//! scale N, and the registry paths (spawning, the wire, capture by a link,
//! the result both ways, the fight limit, the GM levers).
//!
//! **Invented ids and numbers throughout.** The client's effect id and the
//! node and hub type ids appear by name; every other number is the
//! fixture's own, and no `[A]` value is an expected value (they are named
//! constants of the module under test).

use super::*;
use crate::entosis::{ATTR_FUEL_QUANTITY, ATTR_FUEL_TYPE, EFFECT_ENTOSIS_LINK, NEUTRAL_TEAM};
use crate::pyutil::filetime_now;
use crate::sov_hub::fixture as hubfx;
use crate::sov_hub::fixture::{ALICE, BOB, CAROL, DAVE, SYSTEM};

const SEC: i64 = 10_000_000;

// ---------------------------------------------------------------------------
// The pure rules
// ---------------------------------------------------------------------------

const A: i64 = 99_640_001;
const B: i64 = 99_640_002;
const D: i64 = 99_640_003;

fn set(v: &[i64]) -> BTreeSet<i64> {
    v.iter().copied().collect()
}

fn scale(n: u32) -> GameClock {
    GameClock::new(n).expect("a valid scale")
}

fn fresh_bar() -> Vec<(i64, f64)> {
    vec![
        (D, sov_campaign::DEFENDER_START_SCORE),
        (-D, 1.0 - sov_campaign::DEFENDER_START_SCORE),
    ]
}

fn total(v: &[(i64, f64)]) -> f64 {
    v.iter().map(|(_, s)| s).sum()
}

#[test]
fn the_node_table_lets_either_side_score_and_blocks_both() {
    let n = Spec::NODE;
    // Attackers (any alliance but D) on an empty node score for -D, and two
    // attacking alliances are one team.
    let up = decide_node(n, D, 0.0, None, &set(&[A]));
    assert_eq!(
        (up.base, up.controlling, up.scoring, up.blocked),
        (1.0, Some(-D), Some(-D), false)
    );
    let two = decide_node(n, D, 0.0, None, &set(&[A, B]));
    assert_eq!((two.base, two.blocked), (1.0, false));
    // The defender alone scores for D: a node is two-directional.
    let def = decide_node(n, D, 0.0, None, &set(&[D]));
    assert_eq!(
        (def.base, def.controlling, def.scoring, def.blocked),
        (1.0, Some(D), Some(D), false)
    );
    // Each takes the other's progress down first.
    let down = decide_node(n, D, 0.4, Some(-D), &set(&[D]));
    assert_eq!(
        (down.base, down.controlling, down.scoring),
        (-1.0, Some(-D), Some(D))
    );
    let down = decide_node(n, D, 0.4, Some(D), &set(&[A]));
    assert_eq!(
        (down.base, down.controlling, down.scoring),
        (-1.0, Some(D), Some(-D))
    );
    // Both: a stalemate, the bar frozen where it stood.
    let both = decide_node(n, D, 0.4, Some(-D), &set(&[A, D]));
    assert_eq!(
        (both.base, both.blocked, both.controlling),
        (0.0, true, Some(-D))
    );
    // Nobody: the node's own regen, and at 0 rest.
    let none = decide_node(n, D, 0.4, Some(-D), &set(&[]));
    assert_eq!(
        (none.base, none.blocked),
        (-crate::entosis::NODE_REGEN_RATE, false)
    );
    let rest = decide_node(n, D, 0.0, Some(-D), &set(&[]));
    assert_eq!((rest.base, rest.controlling), (0.0, None));
    // A bar at 0 belongs to nobody, whoever held it: the other side starts up.
    let fresh = decide_node(n, D, 0.0, Some(D), &set(&[A]));
    assert_eq!((fresh.base, fresh.controlling), (1.0, Some(-D)));
}

/// The client's formula, `entosisController._CalculateCurrentScore`.
fn client_score(s0: f64, t0: i64, rate: f64, now: i64, capture: f64) -> f64 {
    (s0 + (now - t0) as f64 / capture / SEC as f64 * rate).clamp(0.0, 1.0)
}

#[test]
fn a_node_takes_235_seconds_over_the_scale_and_the_client_agrees() {
    for n in [1u32, 6, 60] {
        let clock = scale(n);
        let t0 = 134_000_000 * SEC;
        let out = node_step(None, t0, Spec::NODE, D, &set(&[A]), clock);
        assert!(out.changed && out.done.is_none());
        let s = out.score;
        assert_eq!(s.rate, f64::from(n), "the scale rides the rate");
        for secs in [0i64, 1, 9, 40] {
            let now = t0 + secs * SEC;
            assert!(
                (s.at(now, Spec::NODE.capture_seconds)
                    - client_score(0.0, t0, s.rate, now, crate::entosis::NODE_CAPTURE_SECONDS))
                .abs()
                    < 1e-12
            );
        }
        let done = s.completes_at(Spec::NODE.capture_seconds).expect("rising");
        let wall = (done - t0) as f64 / SEC as f64;
        assert!(
            (wall - crate::entosis::NODE_CAPTURE_SECONDS / f64::from(n)).abs() < 1e-6,
            "{n}: {wall}"
        );
        let before = node_step(Some(s), done - SEC / 100, Spec::NODE, D, &set(&[A]), clock);
        assert!(before.done.is_none() && !before.changed);
        let at = node_step(Some(s), done, Spec::NODE, D, &set(&[A]), clock);
        assert_eq!(at.done, Some(Done::Full { team: Some(-D) }));
        assert_eq!(at.score, Score::idle(done));
    }
}

#[test]
fn a_node_step_pushes_only_edges_and_regenerates_at_its_own_rate() {
    let clock = scale(1);
    let t0 = 134_000_000 * SEC;
    // Nothing scoring and nothing stored is not an edge.
    let nothing = node_step(None, t0, Spec::NODE, D, &set(&[]), clock);
    assert!(!nothing.changed && nothing.done.is_none());
    let up = node_step(None, t0, Spec::NODE, D, &set(&[A]), clock).score;
    for later in [1, 10, 100] {
        let again = node_step(Some(up), t0 + later * SEC, Spec::NODE, D, &set(&[A]), clock);
        assert!(!again.changed);
        assert_eq!(again.score, up, "the base is not moved");
    }
    // The attacker leaves after 47 s: the bar falls at the node's regen rate.
    let t1 = t0 + 47 * SEC;
    let held = 47.0 / crate::entosis::NODE_CAPTURE_SECONDS;
    let fall = node_step(Some(up), t1, Spec::NODE, D, &set(&[]), clock);
    assert!(fall.changed);
    assert!((fall.score.s0 - held).abs() < 1e-9);
    assert_eq!(fall.score.rate, -crate::entosis::NODE_REGEN_RATE);
    assert_eq!(fall.score.controlling, Some(-D), "toward the last owner");
    // It reaches 0 after held / regen capture-times, and then rests.
    let gone = fall
        .score
        .completes_at(Spec::NODE.capture_seconds)
        .expect("falling");
    let rest = node_step(
        Some(fall.score),
        gone + SEC,
        Spec::NODE,
        D,
        &set(&[]),
        clock,
    );
    assert!(rest.changed, "the idle tuple is pushed");
    assert_eq!((rest.score.rate, rest.score.controlling), (0.0, None));
    // A block freezes the bar and is not a completion however long it lasts.
    let block = node_step(Some(up), t1, Spec::NODE, D, &set(&[A, D]), clock);
    assert!(block.score.blocked && block.score.rate == 0.0);
    assert_eq!(block.score.completes_at(Spec::NODE.capture_seconds), None);
    let later = node_step(
        Some(block.score),
        t1 + 10_000 * SEC,
        Spec::NODE,
        D,
        &set(&[A, D]),
        clock,
    );
    assert!(!later.changed && later.done.is_none());
}

#[test]
fn a_captured_node_moves_the_step_and_the_total_stays_one() {
    // The first attacking capture: the step leaves D for the alliance's key.
    let one = moved(&fresh_bar(), D, Winner::Attackers(Some(A)), NODE_STEP);
    assert_eq!(one[0], (D, sov_campaign::DEFENDER_START_SCORE - NODE_STEP));
    assert_eq!(one[1], (-D, 1.0 - sov_campaign::DEFENDER_START_SCORE));
    assert_eq!(one[2], (A, NODE_STEP));
    assert!((total(&one) - 1.0).abs() < 1e-12);
    // A second alliance gets a key of its own; the first keeps its share.
    let two = moved(&one, D, Winner::Attackers(Some(B)), NODE_STEP);
    assert_eq!(two.len(), 4);
    assert!(two.contains(&(A, NODE_STEP)) && two.contains(&(B, NODE_STEP)));
    // No known alliance: the -D share grows.
    let anon = moved(&fresh_bar(), D, Winner::Attackers(None), NODE_STEP);
    assert_eq!(anon.len(), 2);
    assert!((anon[1].1 - (1.0 - sov_campaign::DEFENDER_START_SCORE + NODE_STEP)).abs() < 1e-12);
    // Defenders take from -D first, then from the alliances' keys.
    let back = moved(&two, D, Winner::Defenders, NODE_STEP);
    assert!((back[0].1 - (two[0].1 + NODE_STEP)).abs() < 1e-12);
    assert!((back[1].1 - (two[1].1 - NODE_STEP)).abs() < 1e-12);
    let mut v = vec![(D, 0.9), (-D, 0.0), (A, 0.1)];
    v = moved(&v, D, Winner::Defenders, NODE_STEP);
    assert!(
        v.contains(&(A, 0.1 - NODE_STEP))
            || v.iter()
                .any(|(k, s)| *k == A && (*s - (0.1 - NODE_STEP)).abs() < 1e-12)
    );
    assert!((total(&v) - 1.0).abs() < 1e-12);
    // The step is clamped to what the other side has, and a spent key goes.
    let last = moved(
        &[(D, 0.98), (-D, 0.0), (A, 0.02)],
        D,
        Winner::Defenders,
        NODE_STEP,
    );
    assert!(last.iter().all(|(k, _)| *k != A), "{last:?}");
    assert!((last[0].1 - 1.0).abs() < 1e-12);
    let short = moved(
        &[(D, 0.03), (-D, 0.97)],
        D,
        Winner::Attackers(Some(A)),
        NODE_STEP,
    );
    assert_eq!(short[0], (D, 0.0));
    assert!((total(&short) - 1.0).abs() < 1e-12);
}

#[test]
fn the_threshold_decides_the_result_and_the_limit_decides_a_stalemate() {
    assert_eq!(verdict(&fresh_bar(), D), None);
    assert_eq!(
        verdict(&[(D, 1.0), (-D, 0.0)], D),
        Some(Outcome::DefendersWin)
    );
    assert_eq!(
        verdict(&[(D, 0.0), (-D, 0.6), (A, 0.4)], D),
        Some(Outcome::AttackersWin)
    );
    // Walking the bar a node at a time ends exactly when a side runs out.
    let mut v = fresh_bar();
    let mut nodes = 0;
    while verdict(&v, D).is_none() {
        v = moved(&v, D, Winner::Attackers(Some(A)), NODE_STEP);
        nodes += 1;
        assert!(nodes < 50);
    }
    assert_eq!(verdict(&v, D), Some(Outcome::AttackersWin));
    let want = (sov_campaign::DEFENDER_START_SCORE / NODE_STEP).ceil() as i32;
    assert_eq!(nodes, want);
    let mut v = fresh_bar();
    let mut nodes = 0;
    while verdict(&v, D).is_none() {
        v = moved(&v, D, Winner::Defenders, NODE_STEP);
        nodes += 1;
        assert!(nodes < 50);
    }
    assert_eq!(verdict(&v, D), Some(Outcome::DefendersWin));
    let want = ((1.0 - sov_campaign::DEFENDER_START_SCORE) / NODE_STEP).ceil() as i32;
    assert_eq!(nodes, want);
    // At the limit the larger share wins; the defenders on a tie.
    assert_eq!(at_limit(&fresh_bar(), D), Outcome::DefendersWin);
    assert_eq!(
        at_limit(&[(D, 0.3), (-D, 0.4), (A, 0.3)], D),
        Outcome::AttackersWin
    );
    assert_eq!(at_limit(&[(D, 0.5), (-D, 0.5)], D), Outcome::DefendersWin);
}

#[test]
fn nodes_spread_over_the_constellation_and_stop_at_the_caps() {
    let cands = [10, 20, 30];
    let mut counts = BTreeMap::new();
    let mut order = Vec::new();
    for _ in 0..INITIAL_NODES {
        let s = choose_system(&cands, &counts).expect("room");
        *counts.entry(s).or_insert(0) += 1;
        order.push(s);
    }
    // The hub's system first, then the others, then round again.
    assert_eq!(order, vec![10, 20, 30, 10]);
    // A single system holds MAX_PER_SYSTEM and then refuses.
    let mut counts = BTreeMap::new();
    for _ in 0..MAX_PER_SYSTEM {
        let s = choose_system(&[10], &counts).expect("room");
        *counts.entry(s).or_insert(0) += 1;
    }
    assert_eq!(choose_system(&[10], &counts), None);
    // Another campaign's nodes count too; the constellation caps the sum.
    let many: Vec<i64> = (0..8).collect();
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    let mut made = 0;
    while let Some(s) = choose_system(&many, &counts) {
        *counts.entry(s).or_insert(0) += 1;
        made += 1;
        assert!(made <= MAX_PER_CONSTELLATION);
    }
    assert_eq!(made, MAX_PER_CONSTELLATION);
    assert!(counts.values().all(|c| *c <= MAX_PER_SYSTEM));
    // A system full of another fight's nodes is skipped.
    let full: BTreeMap<i64, usize> = [(10, MAX_PER_SYSTEM)].into_iter().collect();
    assert_eq!(choose_system(&[10, 20], &full), Some(20));
}

#[test]
fn a_nodes_place_is_a_function_of_its_campaign_and_id_within_bounds() {
    let star = [3.0e9, -2.0e8, 7.5e9];
    let mut seen = BTreeSet::new();
    for id in NODE_ID_BASE..NODE_ID_BASE + 50 {
        let p = node_position(star, 77, id);
        assert_eq!(p, node_position(star, 77, id), "deterministic");
        let dx = p[0] - star[0];
        let dz = p[2] - star[2];
        let flat = (dx * dx + dz * dz).sqrt();
        assert!(
            (NODE_MIN_DISTANCE - 1.0..=NODE_MAX_DISTANCE + 1.0).contains(&flat),
            "{flat}"
        );
        assert!((p[1] - star[1]).abs() <= NODE_MAX_ALTITUDE + 1.0);
        seen.insert(p.map(|v| v.to_bits()));
    }
    assert_eq!(seen.len(), 50, "every node its own place");
    assert_ne!(
        node_position(star, 77, NODE_ID_BASE),
        node_position(star, 78, NODE_ID_BASE)
    );
}

#[test]
fn the_next_node_is_due_in_half_an_hour_plus_or_minus_half_at_any_scale() {
    for n in [1u32, 6, 60] {
        let clock = scale(n);
        let base = clock.span(WAVE_INTERVAL);
        let mut distinct = BTreeSet::new();
        for k in 0..40u32 {
            let due = next_wave(1_000, 5 + i64::from(k), k, clock) - 1_000;
            assert!(
                due >= base / 2 && due < base / 2 * 3 + 1,
                "scale {n}: {due} outside [{}, {})",
                base / 2,
                base / 2 * 3
            );
            distinct.insert(due);
        }
        assert!(
            distinct.len() > 20,
            "the jitter jitters: {}",
            distinct.len()
        );
    }
    assert_eq!(next_wave(5, 9, 3, scale(60)), next_wave(5, 9, 3, scale(60)));
}

#[test]
fn node_ids_are_minted_once_in_their_band() {
    let mut d = Deltas::default();
    let a = mint(&mut d);
    let b = mint(&mut d);
    assert_eq!((a, b), (NODE_ID_BASE, NODE_ID_BASE + 1));
    assert!(all(&d).is_empty(), "the counter is not a node");
    // The band is clear of the other id ranges.
    let clear = |lo: i64, hi: i64| hi <= NODE_ID_BASE || lo >= NODE_ID_END;
    assert!(clear(crate::missile::ID_BASE, crate::missile::ID_END));
    assert!(clear(
        crate::scan::PROBE_ID_BASE,
        crate::scan::PROBE_ID_BASE + 100_000_000_000
    ));
    assert!(clear(crate::econ_exchange::NPC_ORDER_BASE, i64::MAX));
    assert!(
        clear(0, 9_000_000_000),
        "items, npcs, sites, customs offices"
    );
    const { assert!(NODE_ID_END <= 9_000_000_000_000_000_000, "minFakeItem") };
}

// ---------------------------------------------------------------------------
// The registry paths
// ---------------------------------------------------------------------------

const LINK_TYPE: i64 = 990_640_001;
const STRONTIUM: i64 = 990_640_002;
const LINK_GROUP: i64 = 990_640;
const LINK_ALICE: i64 = 100_640_001;
const LINK_DAVE: i64 = 100_640_002;
const FUEL_ALICE: i64 = 100_640_003;
const FUEL_DAVE: i64 = 100_640_004;
const LINK_BOB: i64 = 100_640_005;
const FUEL_BOB: i64 = 100_640_006;
const LINK_CAROL: i64 = 100_640_007;
const FUEL_CAROL: i64 = 100_640_008;
const EFFECT_NAME: &str = "fixtureEntosis";
const LINK_RANGE: f64 = 20_000.0;
const LINK_MS: i64 = 4_000;
const HI_SLOT_0: i64 = 27;
const FUEL_PLENTY: i64 = 5_000;
/// The constellation's other systems and a wormhole.
const SECOND: i64 = 30_640_002;
const THIRD: i64 = 30_640_003;
const WORMHOLE: i64 = 30_640_004;
const WORMHOLE_CLASS: i64 = 990_640_900;

fn content() -> crate::content::Content {
    let mut c = crate::content::Content::fixture();
    let rows = format!(
        "{}type\t{}\t{}\t{}\t1\t1\t10.0\t0.0\t1000.0\t20000.0\t0\t\t\tInvented Hub Row\n\
         type\t{STRONTIUM}\t{}\t4\t1\t40\t5.0\t0.0\t1.0\t1.0\t0\t\t\tInvented Strontium\n\
         type\t{LINK_TYPE}\t{LINK_GROUP}\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{EFFECT_ENTOSIS_LINK}\t12,16,{EFFECT_ENTOSIS_LINK}\t6=0.0;30=1.0;50=5.0;54={LINK_RANGE};73={LINK_MS}.0;{ATTR_FUEL_TYPE}={STRONTIUM}.0;{ATTR_FUEL_QUANTITY}=1.0\tInvented Link\n\
         type\t{TYPE_COMMAND_NODE}\t{GROUP_COMMAND_NODE}\t2\t1\t1\t1.0\t0.0\t0.0\t{NODE_TEST_RADIUS}\t0\t\t\tInvented Node Row\n\
         effect\t{EFFECT_ENTOSIS_LINK}\t2\t0\t73\t6\t54\t0\t0\t{EFFECT_NAME}\teffects.EntosisBeam\n",
        crate::typedata::FIXTURE,
        sov::TYPE_INFRASTRUCTURE_HUB,
        crate::sov_hub::GROUP_INFRASTRUCTURE_HUB,
        crate::sov_hub::CATEGORY_SOVEREIGNTY_STRUCTURE,
        LINK_GROUP + 1,
    );
    let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
    c.insert("typedata", crate::typedata::Installed::build(table));
    let sys = |id: i64, sec: &str, wh: &str, name: &str| {
        format!("{id}\t10640001\t20640001\t\t\t{sec}\t\t0\t0\t0\t0\t0\t0\t{wh}\t0\t\t\t{name}")
    };
    let u = crate::celestial::Universe::parse(
        &[
            sys(SYSTEM, "-0.45", "", "Invented Null"),
            sys(SECOND, "-0.5", "", "Invented Second"),
            sys(THIRD, "-0.55", "", "Invented Third"),
            sys(
                WORMHOLE,
                "-1.0",
                &WORMHOLE_CLASS.to_string(),
                "Invented Hole",
            ),
        ],
        &["10640001\t\t\t\t\tInvented Region".to_owned()],
        &["20640001\t10640001\t\t\t\tInvented Constellation".to_owned()],
    )
    .expect("the test universe parses");
    c.insert("universe", u);
    c
}

/// The invented node row's radius, a named number the tests read back.
const NODE_TEST_RADIUS: f64 = 1_500.0;

fn item(
    c: &crate::content::Content,
    item_id: i64,
    type_id: i64,
    owner: i64,
    flag_id: i64,
    quantity: i64,
    singleton: bool,
) -> crate::item::Item {
    let t = crate::item::find_in(c, type_id).expect("a test type");
    crate::item::Item {
        item_id,
        type_id,
        owner_id: owner,
        location_id: hubfx::ship(owner),
        flag_id,
        quantity,
        group_id: t.group_id,
        category_id: t.category_id,
        singleton,
    }
}

struct World {
    reg: Registry,
    /// Alice's (the defender) and Dave's (the attacker) alliances.
    a: i64,
    d: i64,
    hub: i64,
    t0: i64,
}

/// Four pilots, each with a link and fuel; the hub launched by the GM and
/// held by Alice's alliance. Every push drained.
fn world() -> World {
    let pilot = |id: i64, name: &str| crate::model::Character {
        character_id: id,
        name: name.to_owned(),
        ship_id: hubfx::ship(id),
        solar_system_id: Some(SYSTEM),
        ..crate::model::Character::default()
    };
    let content = std::sync::Arc::new(content());
    let mut reg = Registry::new(crate::model::SandboxConfig {
        characters: vec![
            pilot(ALICE, "Alice"),
            pilot(BOB, "Bob"),
            pilot(CAROL, "Carol"),
            pilot(DAVE, "Dave"),
        ],
        celestials: crate::celestial::Catalogue::parse(&format!(
            "star\t{}\t990620010\t{}\t{}\t{}\t{}\t{SYSTEM}\t0\t0\n",
            hubfx::STAR,
            hubfx::STAR_RADIUS,
            hubfx::STAR_AT[0],
            hubfx::STAR_AT[1],
            hubfx::STAR_AT[2]
        )),
        ..crate::model::SandboxConfig::default()
    })
    .with_content(content.clone());
    let mut items = reg.store().items().to_vec();
    for (link, fuel, who) in [
        (LINK_ALICE, FUEL_ALICE, ALICE),
        (LINK_DAVE, FUEL_DAVE, DAVE),
        (LINK_BOB, FUEL_BOB, BOB),
        (LINK_CAROL, FUEL_CAROL, CAROL),
    ] {
        items.push(item(&content, link, LINK_TYPE, who, HI_SLOT_0, 1, true));
        items.push(item(
            &content,
            fuel,
            STRONTIUM,
            who,
            crate::sov_hub::FLAG_CARGO,
            FUEL_PLENTY,
            false,
        ));
    }
    reg.apply_snapshot(crate::store::Snapshot {
        items,
        next_item_id: 100_641_000,
        ..crate::store::Snapshot::default()
    });
    crate::service::sov_hub::test_corp(&mut reg, hubfx::CORP, ALICE, &[(ALICE, 0), (BOB, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, hubfx::LONE, CAROL, &[(CAROL, 0)]);
    crate::service::sov_hub::test_corp(&mut reg, hubfx::RIVAL, DAVE, &[(DAVE, 0)]);
    let mut st = crate::alliance::Alliances::load(reg.deltas());
    let a = st
        .create(hubfx::CORP, ALICE, "Invented Sov Holders", "ISH", "", "", 1)
        .expect("an alliance");
    let d = st
        .create(hubfx::RIVAL, DAVE, "Invented Rivals", "IRV", "", "", 1)
        .expect("a second alliance");
    st.save(reg.deltas_mut());
    for (who, corp, alliance) in [
        (ALICE, hubfx::CORP, Some(a)),
        (BOB, hubfx::CORP, Some(a)),
        (CAROL, hubfx::LONE, None),
        (DAVE, hubfx::RIVAL, Some(d)),
    ] {
        let c = reg.character_mut(who).expect("a pilot");
        c.corporation_id = corp;
        c.alliance_id = alliance;
    }
    let spot = hubfx::launch_spot(&reg);
    for (n, who) in [ALICE, BOB, CAROL, DAVE].into_iter().enumerate() {
        hubfx::place_at(&mut reg, who, spot);
        let n = n as i64 + 1;
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: n,
            session_id: n,
            user_id: n,
            character_id: Some(who),
        });
    }
    let hub = crate::sov_hub::ensure_hub(&mut reg, SYSTEM).expect("the GM hub");
    let exec = hubfx::owner(&reg, a);
    crate::sov_hub::capture(&mut reg, SYSTEM, exec).expect("held by Alice's alliance");
    let _ = reg.take_pending();
    let _ = hubfx::due(&mut reg);
    World {
        reg,
        a,
        d,
        hub,
        t0: filetime_now(),
    }
}

impl World {
    fn clock(&mut self, n: u32) {
        *self.reg.ext_mut::<GameClock>() = scale(n);
    }

    fn advance(&mut self, t: i64) {
        crate::modules::tick(&mut self.reg, t);
        entosis::tick(&mut self.reg, std::time::Instant::now(), t);
    }

    /// Reinforce at once and let the first tick begin the campaign: the
    /// node fight, its first nodes standing. Pushes drained.
    fn fight(&mut self) -> Campaign {
        entosis::reinforce_at(&mut self.reg, SYSTEM, vec![self.d], true, self.t0)
            .expect("reinforced");
        self.advance(self.t0 + SEC);
        let k = self.campaign().expect("a campaign");
        assert!(k.started, "the fight began");
        let _ = self.pushes();
        k
    }

    fn campaign(&self) -> Option<Campaign> {
        sov_campaign::campaign(self.reg.deltas(), SYSTEM)
    }

    fn claim(&self) -> sov::Claim {
        sov::claim(self.reg.deltas(), SYSTEM).expect("a hub row")
    }

    fn nodes(&self) -> Vec<Node> {
        nodes_of(self.reg.deltas(), self.hub)
    }

    /// A node in the hub's system (the first node always is).
    fn near_node(&self) -> Node {
        self.nodes()
            .into_iter()
            .find(|n| n.system_id == SYSTEM)
            .expect("a node in the hub's system")
    }

    fn pushes(&mut self) -> Vec<hubfx::Sent> {
        let mut v = hubfx::pending(&mut self.reg);
        v.extend(hubfx::due(&mut self.reg));
        v
    }

    /// Put `who` beside `n`, in range of a link.
    fn go_to(&mut self, who: i64, n: &Node) {
        let at = hubfx::off(
            n.position,
            [0.0, 0.0, node_radius(self.reg.content()) + 500.0],
        );
        hubfx::place_at(&mut self.reg, who, at);
    }

    fn link(
        &mut self,
        who: i64,
        n: &Node,
        t: i64,
    ) -> Result<PyValue, crate::service::ServiceError> {
        let module = match who {
            ALICE => LINK_ALICE,
            DAVE => LINK_DAVE,
            BOB => LINK_BOB,
            _ => LINK_CAROL,
        };
        self.go_to(who, n);
        self.reg.locks_mut().add(hubfx::ship(who), n.id);
        crate::modules::activate(
            &mut self.reg,
            Some(who),
            module,
            EFFECT_NAME,
            Some(n.id),
            1000,
            t,
        )
    }

    fn period(&self) -> i64 {
        LINK_MS * crate::modules::MSEC
    }

    /// Step the world from `from` to `until` in `by` ticks; the time reached.
    fn run(&mut self, from: i64, until: i64, by: i64) -> i64 {
        let mut t = from;
        while t < until {
            t += by;
            self.advance(t);
        }
        t
    }

    fn score_of(&self, n: &Node) -> Option<Score> {
        entosis::score_row(self.reg.deltas(), n.id).map(|(_, s)| s)
    }

    /// The bar, as `(team, score)` pairs.
    fn bar(&self) -> Vec<(i64, f64)> {
        self.campaign().expect("a campaign").scores
    }
}

fn named<'a>(sent: &'a [hubfx::Sent], name: &str) -> Vec<&'a hubfx::Sent> {
    sent.iter().filter(|(n, _, _)| *n == name).collect()
}

fn flat(v: &PyValue) -> String {
    match v {
        PyValue::Bytes(b) => format!("{} ", String::from_utf8_lossy(b)),
        PyValue::Unicode(s) => format!("{s} "),
        PyValue::Int(n) => format!("{n} "),
        PyValue::Tuple(l) | PyValue::List(l) => l.iter().map(flat).collect(),
        PyValue::Dict(d) => d.iter().map(|(a, b)| flat(a) + &flat(b)).collect(),
        PyValue::Instance { state, .. } => flat(state),
        PyValue::ObjectEx {
            header, list, dict, ..
        } => {
            flat(header)
                + &list.iter().map(flat).collect::<String>()
                + &dict
                    .iter()
                    .map(|(a, b)| flat(a) + &flat(b))
                    .collect::<String>()
        }
        _ => String::new(),
    }
}

fn sent_text(s: &hubfx::Sent) -> String {
    s.1.iter().map(flat).collect()
}

fn notice_types(sent: &[hubfx::Sent]) -> Vec<i64> {
    named(sent, "OnNotificationReceived")
        .iter()
        .filter_map(|(_, a, _)| match a.get(1) {
            Some(PyValue::Int(t)) => Some(*t),
            _ => None,
        })
        .collect()
}

fn refusal_text(e: &crate::service::ServiceError) -> String {
    match e {
        crate::service::ServiceError::Raise(v) => flat(v),
        other => format!("{other:?}"),
    }
}

#[test]
fn campaign_start_spawns_the_initial_nodes_within_system_and_constellation_caps() {
    for n in [1u32, 60] {
        let mut w = world();
        w.clock(n);
        entosis::reinforce_at(&mut w.reg, SYSTEM, vec![w.d], true, w.t0).expect("reinforced");
        // Reinforced, not begun: no nodes yet.
        assert!(w.nodes().is_empty(), "scale {n}");
        w.advance(w.t0 + SEC);
        assert_eq!(w.claim().state, HubState::NodeFight);
        let nodes = w.nodes();
        assert_eq!(nodes.len(), INITIAL_NODES, "scale {n}");
        // Spread over the constellation's systems -- never the wormhole --
        // and no system over its cap.
        let mut per: BTreeMap<i64, usize> = BTreeMap::new();
        for node in &nodes {
            *per.entry(node.system_id).or_insert(0) += 1;
            assert_eq!(node.hub_id, w.hub);
            assert_eq!(node.hub_system, SYSTEM);
            assert!(node.id >= NODE_ID_BASE && node.id < NODE_ID_END);
            assert_ne!(node.system_id, WORMHOLE);
            assert!(
                [SYSTEM, SECOND, THIRD].contains(&node.system_id),
                "{}",
                node.system_id
            );
        }
        assert!(per.values().all(|c| *c <= MAX_PER_SYSTEM));
        assert_eq!(per.len(), 3, "one in each, the hub's system twice: {per:?}");
        assert_eq!(per[&SYSTEM], 2);
        assert!(nodes.iter().map(|n| n.id).collect::<BTreeSet<_>>().len() == INITIAL_NODES);
        // The fight's clock is stored in the campaign, on the game clock.
        let k = w.campaign().expect("kept");
        let fight = Fight::of(&k).expect("a fight clock");
        assert_eq!(fight.spawned as usize, INITIAL_NODES);
        let base = scale(n).span(WAVE_INTERVAL);
        assert!(fight.next_wave > k.start_time + base / 2 - 1);
        assert!(fight.next_wave <= k.start_time + base / 2 * 3);
        // The pilots in the hub's system were told: a global add per node.
        let sent = {
            // The tick's own pushes left on the clock before `fight` drained;
            // run it again on a fresh world to read them.
            let mut w2 = world();
            w2.clock(n);
            entosis::reinforce_at(&mut w2.reg, SYSTEM, vec![w2.d], true, w2.t0).expect("again");
            let _ = w2.pushes();
            w2.advance(w2.t0 + SEC);
            w2.pushes()
        };
        let adds = named(&sent, "DoDestinyUpdate")
            .iter()
            .filter(|s| sent_text(s).contains("AddBalls2"))
            .count();
        assert!(adds >= 2, "node adds for the hub's system: {adds}");
    }
}

#[test]
fn a_nodes_systems_are_the_constellations_without_its_wormholes_and_the_hub_first() {
    let w = world();
    assert_eq!(systems_for(&w.reg, SYSTEM), vec![SYSTEM, SECOND, THIRD]);
    // From another system of the constellation: that one first.
    assert_eq!(systems_for(&w.reg, THIRD), vec![THIRD, SYSTEM, SECOND]);
    // A system the universe does not know has only itself.
    assert_eq!(systems_for(&w.reg, 30_999_999), vec![30_999_999]);
}

#[test]
fn node_slim_carries_the_four_tuple_source_info() {
    let mut w = world();
    let k = w.fight();
    let node = w.near_node();
    let extras = slim_extras(&w.reg, &[node.id, 123]);
    assert_eq!(extras.len(), 1, "only the node, not the stranger");
    let (id, pairs) = &extras[0];
    assert_eq!(*id, node.id);
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("{name} in {pairs:?}"))
    };
    assert_eq!(
        get("campaign_sourceInfo"),
        PyValue::Tuple(vec![
            PyValue::Int(SYSTEM),
            PyValue::Int(entosis::EVENT_TYPE_HUB),
            PyValue::Int(w.hub),
            PyValue::Int(k.defender),
        ])
    );
    // The client ignores a None score but reads the key: present, None.
    assert_eq!(get("entosis_score"), PyValue::None);
    // The slim: the node type, SCC's, no pilot.
    let s = slim(&node);
    assert_eq!(
        (s.item_id, s.type_id, s.owner_id, s.char_id),
        (node.id, TYPE_COMMAND_NODE, sov::NEUTRAL_HUB_OWNER, None)
    );
    // The ball: global in its system, never moving, the type table's radius.
    let balls = balls_in(&w.reg, SYSTEM);
    assert_eq!(
        balls.iter().filter(|(b, _)| b.ball_id == node.id).count(),
        1
    );
    let b = ball(&w.reg, &node);
    assert_eq!(b.radius, NODE_TEST_RADIUS as f32);
    assert_eq!(b.position, node.position);
    assert_eq!(b.mode, Mode::Stop);
    // Where a pilot warps and a link aims.
    let t = target(&w.reg, node.id).expect("a target");
    assert_eq!(t.position, node.position);
    assert!(target(&w.reg, 123).is_none());
    let lock = entosis::target_of(&w.reg, node.id).expect("an entosis target");
    assert_eq!((lock.system_id, lock.spec), (SYSTEM, Spec::NODE));
    assert!(entosis::is_lockable(&w.reg, node.id, SYSTEM));
    assert!(!entosis::is_lockable(&w.reg, node.id, hubfx::HIGH));
    // The hub is still a hub.
    assert_eq!(
        entosis::target_of(&w.reg, w.hub).expect("hub").spec,
        Spec::HUB
    );
    // The sources list carries the node's extras.
    let all = crate::slim_extra::for_slims(&w.reg, &[node.id]);
    assert!(all.iter().any(|(i, p)| *i == node.id && p.len() == 2));
}

#[test]
fn a_link_on_a_node_needs_a_running_fight_and_an_alliance() {
    let mut w = world();
    // No fight: no nodes. A node of a campaign that has not begun is refused.
    entosis::reinforce_at(&mut w.reg, SYSTEM, vec![w.d], false, w.t0).expect("reinforced");
    let ghost = Node {
        id: NODE_ID_BASE + 500,
        system_id: SYSTEM,
        hub_system: SYSTEM,
        hub_id: w.hub,
        position: hubfx::off(
            crate::sov_hub::position_in(&w.reg, SYSTEM),
            [0.0, 5_000.0, 0.0],
        ),
        spawned: w.t0,
    };
    put(w.reg.deltas_mut(), &ghost);
    let e = w.link(DAVE, &ghost, w.t0).expect_err("no fight");
    assert!(refusal_text(&e).contains("no running campaign"), "{e:?}");
    // Begin the fight for real.
    let mut w = world();
    w.fight();
    let node = w.near_node();
    // Carol has no alliance.
    let e = w.link(CAROL, &node, w.t0).expect_err("no alliance");
    assert!(refusal_text(&e).contains("alliance"), "{e:?}");
    // Out of range: refused with the range readout.
    hubfx::place_at(
        &mut w.reg,
        DAVE,
        hubfx::off(node.position, [0.0, 0.0, 10.0 * LINK_RANGE]),
    );
    w.reg.locks_mut().add(hubfx::ship(DAVE), node.id);
    let e = crate::modules::activate(
        &mut w.reg,
        Some(DAVE),
        LINK_DAVE,
        EFFECT_NAME,
        Some(node.id),
        1000,
        w.t0,
    )
    .expect_err("too far");
    assert!(refusal_text(&e).contains("TargetTooFar"), "{e:?}");
    // In range: both the attacker and the defender may link a node.
    w.link(DAVE, &node, w.t0).expect("the attacker");
    w.link(ALICE, &node, w.t0).expect("the defender");
    // And while a link runs on a node the ship cannot warp, jump or dock.
    let warp = crate::movement_guard::check(&crate::module::plan().warp_guards, &w.reg, DAVE);
    assert!(warp.is_some(), "warp refused");
    assert!(
        entosis::refuse_dock(&w.reg, Some(DAVE)).is_err(),
        "dock refused"
    );
    assert!(linking_a_node(&w.reg, hubfx::ship(DAVE)));
    assert!(!linking_a_node(&w.reg, hubfx::ship(BOB)));
}

fn step_for(n: u32) -> i64 {
    if n == 1 { 5 * SEC } else { SEC / 2 }
}

fn capture_ticks(n: u32) -> i64 {
    (crate::entosis::NODE_CAPTURE_SECONDS / f64::from(n) * SEC as f64) as i64
}

#[test]
fn node_capture_moves_the_step_and_pushes_score_updated() {
    for n in [1u32, 60] {
        let mut w = world();
        w.clock(n);
        let k = w.fight();
        let node = w.near_node();
        let before = w.nodes().len();
        w.link(DAVE, &node, w.t0 + SEC).expect("the attacker");
        let p = w.period();
        let by = step_for(n);
        let scoring_from = w.t0 + SEC + p;
        // Not captured a little before the capture time is up.
        let mut t = w.run(w.t0 + SEC, scoring_from + capture_ticks(n) - 2 * by, by);
        assert!(node_alive(&w, &node), "not yet at scale {n}");
        assert_eq!(w.bar(), k.scores, "the bar is where it was");
        // The node's slim changed on the edge with the score the client reads.
        let sent = w.pushes();
        let slim_pushes = named(&sent, "DoDestinyUpdate")
            .iter()
            .filter(|s| sent_text(s).contains("entosis_score"))
            .count();
        assert!(slim_pushes >= 1, "scale {n}: {sent:?}");
        let s = w.score_of(&node).expect("scoring");
        assert!(s.rate > 0.0 && s.scoring == Some(-w.a) && s.controlling == Some(-w.a));
        // The defender's managers were told the node is under attack.
        assert!(
            notice_types(&sent).contains(&entosis::kind::ENTOSIS_CAPTURE_STARTED),
            "{:?}",
            notice_types(&sent)
        );
        t = w.run(t, scoring_from + capture_ticks(n) + 3 * by, by);
        let _ = t;
        // Captured: the node is gone, the bar moved one step to Dave's alliance.
        assert!(!node_alive(&w, &node), "captured at scale {n}");
        assert_eq!(w.nodes().len(), before - 1);
        let bar = w.bar();
        let share = |team: i64| bar.iter().find(|(k, _)| *k == team).map(|(_, s)| *s);
        assert_eq!(
            share(w.a),
            Some(sov_campaign::DEFENDER_START_SCORE - NODE_STEP)
        );
        assert_eq!(share(w.d), Some(NODE_STEP), "the capturing alliance's key");
        assert_eq!(share(-w.a), Some(1.0 - sov_campaign::DEFENDER_START_SCORE));
        assert!((total(&bar) - 1.0).abs() < 1e-9);
        assert!(w.campaign().expect("kept").attackers.contains(&w.d));
        // The link stopped, the lock was dropped, the score row is gone.
        assert!(w.reg.modules().running(LINK_DAVE).is_none());
        assert!(
            !w.reg
                .locks()
                .targets_of(hubfx::ship(DAVE))
                .contains(&node.id)
        );
        assert!(w.score_of(&node).is_none());
        // The pushes: the bar with STRUCTURE_SCORE_UPDATED, the ball removed.
        let sent = w.pushes();
        let structs = named(&sent, "OnSolarSystemSovStructuresUpdated");
        assert!(
            structs.iter().any(|s| {
                matches!(s.1.get(2), Some(PyValue::Dict(d))
                    if d.iter().any(|(v, k)| *k == PyValue::Int(w.hub)
                        && *v == PyValue::List(vec![PyValue::Int(entosis::STRUCTURE_SCORE_UPDATED)])))
            }),
            "scale {n}: {structs:?}"
        );
        assert!(
            named(&sent, "DoDestinyUpdate")
                .iter()
                .any(|s| sent_text(s).contains("RemoveBalls")
                    && sent_text(s).contains(&node.id.to_string())),
            "the node's ball is removed: {sent:?}"
        );
        // The dashboard reads the new bar.
        let rows = sov::alliance_structures_at(w.reg.deltas(), w.a, filetime_now(), scale(n));
        let PyValue::Tuple(parts) = rows else {
            panic!()
        };
        let PyValue::List(scores) = &parts[2] else {
            panic!()
        };
        assert_eq!(scores.len(), 3, "D, -D and Dave's key");
        assert!(w.reg.modules().running(LINK_DAVE).is_none());
    }
}

fn node_alive(w: &World, n: &Node) -> bool {
    node(w.reg.deltas(), n.id).is_some()
}

#[test]
fn the_defender_can_capture_a_node_and_both_sides_block_each_other() {
    let mut w = world();
    w.clock(60);
    w.fight();
    let node = w.near_node();
    let by = step_for(60);
    // Both link: a stalemate, nothing moves however long it lasts.
    w.link(DAVE, &node, w.t0 + SEC).expect("attacker");
    w.link(ALICE, &node, w.t0 + SEC).expect("defender");
    let t = w.run(w.t0 + SEC, w.t0 + SEC + w.period() + 2 * by, by);
    let s = w.score_of(&node).expect("a row");
    assert!(s.blocked && s.rate == 0.0, "{s:?}");
    let t = w.run(t, t + 3 * capture_ticks(60), by);
    assert!(node_alive(&w, &node));
    // Dave stops: Alice resumes from where it stood and captures.
    crate::modules::stop(&mut w.reg, LINK_DAVE, t, None);
    let t = w.run(t, t + 2 * capture_ticks(60) + 4 * by, by);
    let _ = t;
    assert!(!node_alive(&w, &node), "the defender captured it");
    let bar = w.bar();
    // Defenders take from -D first: the attackers' pool shrinks by a step.
    assert_eq!(bar[0].0, w.a);
    assert!(
        (bar[0].1 - (sov_campaign::DEFENDER_START_SCORE + NODE_STEP)).abs() < 1e-9,
        "{bar:?}"
    );
    assert!((bar[1].1 - (1.0 - sov_campaign::DEFENDER_START_SCORE - NODE_STEP)).abs() < 1e-9);
    assert!(
        w.campaign().expect("kept").attackers == vec![w.d],
        "no new attacker"
    );
}

#[test]
fn an_abandoned_node_regenerates_and_goes_quiet() {
    let mut w = world();
    w.clock(60);
    w.fight();
    let node = w.near_node();
    let by = step_for(60);
    w.link(DAVE, &node, w.t0 + SEC).expect("attacker");
    let t = w.run(
        w.t0 + SEC,
        w.t0 + SEC + w.period() + capture_ticks(60) / 2,
        by,
    );
    assert!(w.score_of(&node).expect("scoring").rate > 0.0);
    crate::modules::stop(&mut w.reg, LINK_DAVE, t, None);
    // It falls at the node's regen rate and ends on an idle tuple.
    let t = w.run(t, t + 2 * by, by);
    let s = w.score_of(&node).expect("kept");
    assert!(s.rate < 0.0, "{s:?}");
    let gone = s.completes_at(Spec::NODE.capture_seconds).expect("falling");
    let _ = w.run(t, gone + 2 * by, by);
    let s = w.score_of(&node).expect("an idle row");
    assert_eq!((s.rate, s.controlling), (0.0, None));
    assert!(node_alive(&w, &node), "an abandoned node stays");
    assert_eq!(w.bar(), start_bar(&w), "and the bar did not move");
}

fn bar_of(k: &Campaign) -> Vec<(i64, f64)> {
    k.scores.clone()
}

fn start_bar(w: &World) -> Vec<(i64, f64)> {
    vec![
        (w.a, sov_campaign::DEFENDER_START_SCORE),
        (-w.a, 1.0 - sov_campaign::DEFENDER_START_SCORE),
    ]
}

#[test]
fn extra_node_spawns_after_the_interval() {
    for n in [1u32, 60] {
        let mut w = world();
        w.clock(n);
        let k = w.fight();
        let fight = Fight::of(&w.campaign().expect("kept")).expect("a clock");
        assert_eq!(w.nodes().len(), INITIAL_NODES);
        // Just before it is due: nothing.
        w.advance(fight.next_wave - SEC / 100);
        assert_eq!(w.nodes().len(), INITIAL_NODES, "scale {n}");
        // Due: one more, and the clock moves on by a jittered interval.
        w.advance(fight.next_wave + SEC / 100);
        assert_eq!(w.nodes().len(), INITIAL_NODES + 1, "scale {n}");
        let after = Fight::of(&w.campaign().expect("kept")).expect("a clock");
        assert_eq!(after.spawned as usize, INITIAL_NODES + 1);
        let base = scale(n).span(WAVE_INTERVAL);
        let gap = after.next_wave - (fight.next_wave + SEC / 100);
        assert!(
            gap >= base / 2 - 1 && gap <= base / 2 * 3 + 1,
            "{gap} vs {base}"
        );
        // The new node is announced to the pilots in its system.
        let sent = w.pushes();
        let adds = named(&sent, "DoDestinyUpdate")
            .iter()
            .filter(|s| sent_text(s).contains("AddBalls2"))
            .count();
        assert!(
            adds >= 1
                || w.nodes()
                    .iter()
                    .all(|n| n.system_id != SYSTEM || n.spawned < fight.next_wave)
        );
        let _ = bar_of(&k);
    }
}

#[test]
fn waves_stop_at_the_caps_but_the_clock_goes_on() {
    let mut w = world();
    let k = w.fight();
    // Fill the constellation by hand: the cap is the sum over its systems.
    while w.nodes().len() < MAX_PER_CONSTELLATION {
        if spawn_one(&mut w.reg, &k, w.t0).is_none() {
            break;
        }
    }
    let per_system_cap = 3 * MAX_PER_SYSTEM;
    assert_eq!(w.nodes().len(), per_system_cap.min(MAX_PER_CONSTELLATION));
    assert!(spawn_one(&mut w.reg, &k, w.t0).is_none(), "full");
    let fight = Fight::of(&w.campaign().expect("kept")).expect("a clock");
    w.advance(fight.next_wave + SEC);
    assert_eq!(w.nodes().len(), per_system_cap.min(MAX_PER_CONSTELLATION));
    let after = Fight::of(&w.campaign().expect("kept")).expect("a clock");
    assert!(after.next_wave > fight.next_wave, "the clock advanced");
}

/// Drive `n` captures for `winner` through the same function a link's
/// completion calls.
fn capture_n(w: &mut World, n: usize, winner: Winner) {
    for _ in 0..n {
        let Some(k) = w.campaign() else { return };
        apply_capture(&mut w.reg, &k, winner);
    }
}

#[test]
fn attacker_win_destroys_the_hub_and_lapses_the_claim() {
    let mut w = world();
    let k = w.fight();
    let hub_item = w.hub;
    let _ = w.pushes();
    // Eight captures leave the defenders a sliver.
    let need = (sov_campaign::DEFENDER_START_SCORE / NODE_STEP).ceil() as usize;
    let d = w.d;
    capture_n(&mut w, need - 1, Winner::Attackers(Some(d)));
    assert_eq!(w.claim().state, HubState::NodeFight, "not yet");
    assert!(w.campaign().is_some());
    let share = defender_share(&w.bar(), w.a);
    assert!(share > 0.0 && share < NODE_STEP, "{share}");
    // The last one ends it: the hub is gone, the claim lapsed, the nodes too.
    capture_n(&mut w, 1, Winner::Attackers(Some(d)));
    assert!(
        sov::claim(w.reg.deltas(), SYSTEM).is_none(),
        "the claim lapsed"
    );
    assert!(
        crate::sov_hub::hub(w.reg.deltas(), SYSTEM).is_none(),
        "the hub is gone"
    );
    assert!(
        w.reg.store().get(hub_item).is_none(),
        "the hub item is destroyed"
    );
    assert!(w.campaign().is_none());
    assert!(
        all(w.reg.deltas()).is_empty(),
        "no node outlives its campaign"
    );
    assert!(entosis::score_row(w.reg.deltas(), hub_item).is_none());
    // The loss is on the sov ledger, as an unowned system.
    assert_eq!(
        sov::recent_activity(w.reg.deltas(), filetime_now() + SEC)
            .last()
            .map(|a| a.owner),
        Some(None)
    );
    // The system is open: a new hub can be launched there.
    assert!(sov::claim(w.reg.deltas(), SYSTEM).is_none());
    // Both sides heard: the defender 162 and 41, the attackers 162.
    let sent = w.pushes();
    let all_notes = notice_types(&sent);
    assert!(
        all_notes.contains(&entosis::kind::STRUCTURE_DESTROYED),
        "{all_notes:?}"
    );
    assert!(
        all_notes.contains(&entosis::kind::ALLIANCE_CLAIM_LOST),
        "{all_notes:?}"
    );
    let to_attackers = named(&sent, "OnNotificationReceived")
        .iter()
        .filter(|(_, a, aud)| {
            a.get(1) == Some(&PyValue::Int(entosis::kind::STRUCTURE_DESTROYED))
                && aud.as_ref().is_some_and(|v| v.contains(&DAVE))
        })
        .count();
    assert_eq!(to_attackers, 1, "Dave, the rival CEO, heard it fell");
    assert!(
        named(&sent, "ProcessSovStatusChanged")
            .iter()
            .any(|s| s.1.get(1) == Some(&PyValue::None))
    );
    let _ = k;
}

#[test]
fn defender_win_restores_the_claim_with_a_new_window() {
    let mut w = world();
    w.fight();
    let hub = w.hub;
    let need = ((1.0 - sov_campaign::DEFENDER_START_SCORE) / NODE_STEP).ceil() as usize;
    capture_n(&mut w, need - 1, Winner::Defenders);
    assert_eq!(w.claim().state, HubState::NodeFight);
    let sent = w.pushes();
    let _ = sent;
    capture_n(&mut w, 1, Winner::Defenders);
    // The campaign is cleared, the hub claimed again and still Alice's.
    let c = w.claim();
    assert_eq!(c.state, HubState::Claimed);
    assert_eq!(c.holder(), Some(w.a));
    assert_eq!(c.hub_id, hub);
    assert!(w.campaign().is_none());
    assert!(all(w.reg.deltas()).is_empty(), "the nodes are gone");
    assert!(crate::sov_hub::hub(w.reg.deltas(), SYSTEM).is_some());
    // A fresh window: the structure row shows the next one, no campaign.
    let PyValue::List(rows) = sov::structure_rows_at(
        w.reg.deltas(),
        SYSTEM,
        w.t0 + 2 * SEC,
        crate::game_clock::of(&w.reg),
    ) else {
        panic!()
    };
    let PyValue::Instance { state, .. } = &rows[0] else {
        panic!()
    };
    assert_eq!(
        crate::pyutil::dict_get(state, b"campaignState"),
        Some(&PyValue::None)
    );
    assert!(matches!(
        crate::pyutil::dict_get(state, b"vulnerabilityState"),
        Some(PyValue::Tuple(w)) if w.len() == 2
    ));
    // The hub's slim is idle, not None.
    assert!(matches!(
        entosis::slim_score(w.reg.deltas(), hub),
        PyValue::Tuple(_)
    ));
    // The attackers can come again: the hub is attackable in its window.
    let end = w.t0 + 3 * SEC;
    w.advance(end);
    assert_eq!(w.claim().state, HubState::Claimed);
}

#[test]
fn the_ends_of_a_fight_are_decided_on_the_clock_at_any_scale() {
    for n in [1u32, 60] {
        // Nobody comes: the defenders hold more of the bar and keep the hub.
        let mut w = world();
        w.clock(n);
        let k = w.fight();
        let limit = k.start_time + scale(n).span(FIGHT_LIMIT);
        w.advance(limit - SEC);
        assert_eq!(w.claim().state, HubState::NodeFight, "scale {n}: not yet");
        w.advance(limit + SEC);
        assert_eq!(w.claim().state, HubState::Claimed, "scale {n}");
        assert!(w.campaign().is_none());
        assert!(all(w.reg.deltas()).is_empty());
        // The attackers hold more of it: they win by default.
        let mut w = world();
        w.clock(n);
        let mut k = w.fight();
        k.scores = vec![(w.a, 0.3), (-w.a, 0.4), (w.d, 0.3)];
        sov_campaign::put(w.reg.deltas_mut(), &k);
        w.advance(k.start_time + scale(n).span(FIGHT_LIMIT) + SEC);
        assert!(
            sov::claim(w.reg.deltas(), SYSTEM).is_none(),
            "scale {n}: lapsed"
        );
        assert!(all(w.reg.deltas()).is_empty());
    }
}

#[test]
fn the_gm_resolve_still_ends_a_fight_and_takes_the_nodes_with_it() {
    let mut w = world();
    w.fight();
    assert_eq!(w.nodes().len(), INITIAL_NODES);
    let node = w.near_node();
    w.link(DAVE, &node, w.t0 + SEC).expect("a link");
    sov_campaign::resolve(&mut w.reg, SYSTEM, Outcome::DefendersWin).expect("resolved");
    assert_eq!(w.claim().state, HubState::Claimed);
    assert!(all(w.reg.deltas()).is_empty());
    assert!(
        w.reg.modules().running(LINK_DAVE).is_none(),
        "its link stopped"
    );
    assert!(
        !w.reg
            .locks()
            .targets_of(hubfx::ship(DAVE))
            .contains(&node.id),
        "its lock dropped"
    );
    let sent = w.pushes();
    assert!(
        named(&sent, "DoDestinyUpdate")
            .iter()
            .any(|s| sent_text(s).contains("RemoveBalls")),
        "the balls leave"
    );
}

#[test]
fn a_lapsed_hub_or_a_moved_claim_takes_its_nodes_away() {
    // The hub lapses (fuel, GM): D2's lapse -> entosis::forget.
    let mut w = world();
    w.fight();
    let h = crate::sov_hub::hub(w.reg.deltas(), SYSTEM).expect("a hub");
    crate::sov_hub::lapse(&mut w.reg, &h, w.t0 + 10 * SEC);
    assert!(all(w.reg.deltas()).is_empty());
    // A GM moves the claim to the rival: the campaign goes (D3a) and the
    // next tick takes the stale nodes.
    let mut w = world();
    w.fight();
    let exec = hubfx::owner(&w.reg, w.d);
    sov::set_claim(w.reg.deltas_mut(), SYSTEM, Some(exec), w.t0).expect("changed");
    assert!(w.campaign().is_none());
    assert_eq!(
        w.nodes().len(),
        INITIAL_NODES,
        "still standing until the tick"
    );
    w.advance(w.t0 + 2 * SEC);
    assert!(all(w.reg.deltas()).is_empty());
}

#[test]
fn nodes_scores_and_the_fight_clock_survive_the_world_file() {
    let mut w = world();
    w.clock(60);
    let k = w.fight();
    let node = w.near_node();
    w.link(DAVE, &node, w.t0 + SEC).expect("a link");
    let t = w.run(w.t0 + SEC, w.t0 + SEC + w.period() + SEC / 2, step_for(60));
    let _ = t;
    let stored = w.score_of(&node).expect("scoring");
    // The rows are plain strings: through put_verbatim, as a save and a load.
    let mut copy = Deltas::default();
    for (kind, rows) in w.reg.deltas().kinds() {
        for (key, v) in rows {
            copy.put_verbatim(kind, key.clone(), v.clone());
        }
    }
    assert_eq!(nodes_of(&copy, w.hub).len(), w.nodes().len());
    assert_eq!(node_row_of(&copy, node.id), Some(node));
    assert_eq!(entosis::score_row(&copy, node.id), Some((SYSTEM, stored)));
    let kept = sov_campaign::campaign(&copy, SYSTEM).expect("the campaign");
    assert_eq!(Fight::of(&kept), Fight::of(&k));
    // The id counter survives too: a restarted world never reuses an id.
    let next = mint(&mut copy);
    assert!(w.nodes().iter().all(|n| n.id < next), "{next}");
    // And the slim a restarted world serves says what it said.
    assert_eq!(slim_score(&copy, &node), slim_score(w.reg.deltas(), &node));
}

fn node_row_of(d: &Deltas, id: i64) -> Option<Node> {
    node(d, id)
}

#[test]
fn a_campaign_from_before_the_nodes_started_gets_its_fight_on_the_next_tick() {
    let mut w = world();
    let k = w.fight();
    // As D3a left it: begun, no nodes, no clock.
    despawn_hub(&mut w.reg, w.hub);
    let mut bare = k.clone();
    bare.rest = Vec::new();
    sov_campaign::put(w.reg.deltas_mut(), &bare);
    assert!(w.nodes().is_empty());
    w.advance(w.t0 + 2 * SEC);
    assert_eq!(w.nodes().len(), INITIAL_NODES);
    assert!(Fight::of(&w.campaign().expect("kept")).is_some());
}

#[test]
fn the_gm_levers_report_spawn_and_capture() {
    let mut w = world();
    w.fight();
    let words = |line: &str| {
        line.split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let say = |w: &mut World, line: &str| {
        let owned = words(line);
        let args: Vec<&str> = owned.iter().map(String::as_str).collect();
        slash(&mut w.reg, Some(ALICE), &args)
    };
    let out = say(&mut w, "here");
    assert!(
        out.contains("fighting") && out.contains("4 nodes") && out.contains(&w.hub.to_string()),
        "{out}"
    );
    let out = say(&mut w, "spawn here");
    assert!(out.starts_with("node "), "{out}");
    assert_eq!(w.nodes().len(), INITIAL_NODES + 1);
    let first = w.near_node();
    let out = say(&mut w, &format!("capture {} attackers", first.id));
    assert!(out.contains("captured for the attackers"), "{out}");
    assert!(!node_alive(&w, &first));
    assert!(defender_share(&w.bar(), w.a) < sov_campaign::DEFENDER_START_SCORE);
    let second = w.near_node();
    let out = say(&mut w, &format!("capture {} defenders", second.id));
    assert!(out.contains("for the defenders"), "{out}");
    assert!(say(&mut w, "capture 1 nobody").starts_with("usage"));
    assert!(say(&mut w, "capture 5 defenders").contains("no node"));
    // The word is routed through the engine's slash.
    let out = entosis::slash(&mut w.reg, Some(ALICE), &["nodes", "here"]);
    assert!(out.contains("campaign on hub"), "{out}");
    assert!(entosis::SLASH_WORDS.contains(&"nodes"));
    // After the fight there is nothing to report.
    sov_campaign::resolve(&mut w.reg, SYSTEM, Outcome::DefendersWin).expect("resolved");
    assert!(say(&mut w, "here").contains("no campaign"));
    assert!(say(&mut w, "spawn here").contains("no campaign"));
}

#[test]
fn the_kind_and_the_sources_are_registered() {
    assert!(crate::delta_kinds::is_registered(KIND));
    assert!(
        crate::slim_extra::SOURCES
            .iter()
            .any(|s| std::ptr::fn_addr_eq(*s, slim_extras as crate::slim_extra::Source))
    );
    let _ = NEUTRAL_TEAM;
    // A world with no campaign: the tick costs nothing and pushes nothing.
    let mut reg = Registry::new(crate::model::SandboxConfig::default());
    tick(&mut reg, filetime_now());
    assert!(reg.take_pending().is_empty());
    assert!(balls_in(&reg, SYSTEM).is_empty());
    assert!(slim_extras(&reg, &[NODE_ID_BASE]).is_empty());
}
