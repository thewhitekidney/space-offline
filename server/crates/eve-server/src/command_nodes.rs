//! **Command nodes and the node fight** (SOC-D3b; lane note
//! `docs/lanes/sov-d3b.md`, spec R-SOV §5.2, §6.5, §9 D3b).
//!
//! D3a ends an attack on a held hub with a reinforced hub and, when its time
//! comes, a campaign in `nodefight` (`sov_campaign`). This module is what
//! happens in the fight: command nodes appear, either side captures them
//! with an Entosis Link, every captured node moves the campaign's bar, and
//! the bar ends the campaign.
//!
//! # The nodes
//!
//! A node is a ball of the Entosis Command Node type ([`TYPE_COMMAND_NODE`],
//! group 1316 **[V]**) owned by SCC ([`sov::NEUTRAL_HUB_OWNER`]) **[A]**. It
//! is not an item: it has an id from [`NODE_ID_BASE`], one row of kind
//! [`KIND`], and lives only as long as its campaign.
//!
//! ```text
//! cnode <nodeID>  -> systemID hubSystemID hubID x y z spawnedAt
//! cnode seq       -> next id offset
//! ```
//!
//! - **Count and place** (R-SOV §6.5 **[V2]**): [`INITIAL_NODES`] (four) the
//!   moment the campaign starts, spread over the hub's system and the other
//!   systems of its constellation ([`choose_system`]: the least loaded
//!   system first, the hub's own on a tie), at most [`MAX_PER_SYSTEM`] in a
//!   system and [`MAX_PER_CONSTELLATION`] in the constellation, then **one
//!   more every [`WAVE_INTERVAL`] game minutes, +-50 %** while the fight
//!   lasts. Where in a system is [`node_position`]: a deterministic scatter
//!   around the star **[A]**.
//! - **On the wire** (R-SOV §5.2 **[V]**): the slim carries `typeID` 34832,
//!   `ownerID` SCC, `campaign_sourceInfo = (sourceSolarSystemID, eventTypeID,
//!   sourceItemID, defenderAllianceID)` and `entosis_score` (the entosis
//!   engine's tuple, `None` until a link has scored). The ball is global in
//!   its system, like the hub **[A]** (the type is not `isAlwaysGlobal` in
//!   the client's data, but a node nobody can see cannot be fought).
//!
//! # Capturing a node
//!
//! The same Entosis Link, the same engine: [`entosis::target_of`] joins the
//! nodes, so a node can be locked, ranged, fuelled and linked like a hub, and
//! its score is the `entosis` row of the node's id. What differs is the
//! table ([`decide_node`], R-SOV §6.4): a node is *two-directional* **[V]**
//! (`EntosisCommandNode.twoDirectionalCapture`), so the defender may score on
//! it too:
//!
//! | links in scoring phase | the bar |
//! |---|---|
//! | attackers only (any alliance but the defender: team `-D`) | up for `-D`, or down if `D` holds it |
//! | the defender only (team `D`) | up for `D`, or down if `-D` holds it |
//! | both | blocked |
//! | nobody | falls at the node's regen rate (0.04 **[V-data]**) |
//!
//! A node's capture takes 235 s **[V-data]** (EvEJS has 240; ours wins).
//!
//! # The campaign bar
//!
//! `Campaign::scores` is `{D: 0.6, -D: 0.4}` at the start **[V2]**. A node
//! captured for a side moves [`NODE_STEP`] (0.07 **[V2]**) from the other
//! side to it ([`moved`]), the node despawns, and the bar is pushed
//! (`OnSolarSystemSovStructuresUpdated` with `{hubID: [0]}`, the client's
//! `STRUCTURE_SCORE_UPDATED`). The total stays 1.0 **[A]** (conservation).
//! An attacking alliance that captures a node gets a key of its own **[A]**
//! (the client draws one segment per key and names the positive ones); the
//! `-D` segment is the attackers' share before anyone has captured, and the
//! first the defenders take back.
//!
//! **The result** ([`verdict`]): the defenders' share at 1.0 is a defence,
//! at 0.0 the attackers have won **[I]**. [`sov_campaign::resolve`] ends it:
//! attackers destroy the hub and the claim lapses; defenders keep the claim
//! and the hub returns to `claimed`. A fight nobody decides ends after
//! [`FIGHT_LIMIT`] game hours **[A]** for whoever holds more of the bar
//! (the defenders on a tie), so a forgotten campaign cannot hold a system
//! for ever.
//!
//! # Notifications
//!
//! Start: `SovCommandNodeEventStarted` (161), D3a's `begin`, to both sides'
//! managers. Progress: the bar push for every capture, and
//! `EntosisCaptureStarted` (147, structure type: the node) to the
//! defender's managers when a node first comes under attack **[A]**. End:
//! `SovStructureDestroyed` (162) to **both** sides and `SovAllClaimLostMsg`
//! (41) to the defender when the hub falls (D3a's `resolve`, plus
//! [`on_resolve`] for the attackers); a successful defence has no client
//! formatter and so no mail, only the structure push.
//!
//! # Game time (lane CLOCK)
//!
//! Everything is on the world clock: the capture rate carries the scale
//! (`GameClock::rate`), the wave interval and the fight limit are game spans
//! (`GameClock::span`). Nothing here reads the wall clock for a decision.
//!
//! # Seams in D3a's files (the coordinator diff)
//!
//! `sov_campaign::on_begin` calls [`on_begin`], `sov_campaign::tick` calls
//! [`tick`], `sov_campaign::resolve` calls [`on_resolve`],
//! `entosis::target_of` joins [`target_of`], `entosis::check_target` asks
//! [`gate`], `entosis::forget` calls [`despawn_hub`], and the engine's
//! `upkeep` and `stop_links` are `pub(crate)`.

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::entosis::{self, Decision, Done, Score, Spec, Step, Target};
use crate::game_clock::GameClock;
use crate::notifications::{self as notif, Value, data};
use crate::service::Registry;
use crate::sov::{self, HubState};
use crate::sov_campaign::{self, Campaign, Outcome};
use crate::sov_hub::{HubAdded, HubSlimChange};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "cnode";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `typeEntosisCommandNode` **[V]** (R-SOV §2): type 34832, group 1316.
pub const TYPE_COMMAND_NODE: i64 = 34_832;
pub const GROUP_COMMAND_NODE: i64 = 1_316;

/// Where node ids live: between the missile band (`missile::ID_END`) and
/// the NPC order band, clear of every other range. A node is never an item.
pub const NODE_ID_BASE: i64 = 9_600_000_000_000;
pub const NODE_ID_END: i64 = 9_700_000_000_000;

/// **[V2]** the nodes a campaign starts with (R-SOV §6.5).
pub const INITIAL_NODES: usize = 4;
/// **[V2]** the most nodes in one system, and in one constellation.
pub const MAX_PER_SYSTEM: usize = 5;
pub const MAX_PER_CONSTELLATION: usize = 20;
/// **[V2]** one more node every half hour, plus or minus half of it. A
/// game span: `--time-scale` shortens it.
pub const WAVE_INTERVAL: i64 = 30 * (crate::seed::HOUR / 60);

/// **[A]** a fight nobody decides ends this many game hours after it began,
/// for whoever holds more of the bar (the defenders on a tie). Falsified by
/// an owner who wants fights to run until a side wins outright, or by the
/// play round finding 48 hours too short for an alliance to gather.
pub const FIGHT_LIMIT: i64 = 48 * crate::seed::HOUR;

/// The campaign bar's step per captured node (`NODE_STEP` in
/// `sov_campaign`, **[V2]**).
pub const NODE_STEP: f64 = sov_campaign::NODE_STEP;

/// **[A]** a node's radius in metres when the type table lacks the type
/// (the client's own value is not in the bundle). Falsified by a node
/// bracket whose range readout disagrees with the Entosis Link's.
pub const NODE_RADIUS: f64 = 3_000.0;

/// **[A]** the scatter around the star: a node is this far from the star
/// at the least and at the most (metres, about 1 and 5 AU), and this high
/// or low at the most. Falsified by a human who finds nodes at the sun or
/// beyond the outermost planet (retail puts them at random deep-space
/// points).
pub const NODE_MIN_DISTANCE: f64 = 1.5e11;
pub const NODE_MAX_DISTANCE: f64 = 7.5e11;
pub const NODE_MAX_ALTITUDE: f64 = 5.0e10;

/// Within this of 0 or 1 a share is complete (float dust).
const EPSILON: f64 = 1e-9;

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// One command node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub id: i64,
    /// The system the node is in.
    pub system_id: i64,
    /// The campaign it belongs to: the hub, and the hub's system.
    pub hub_id: i64,
    pub hub_system: i64,
    pub position: [f64; 3],
    pub spawned: BlueTime,
}

fn parse_row(key: &[String], v: &[String]) -> Option<Node> {
    let num = |i: usize| v.get(i)?.parse::<f64>().ok();
    Some(Node {
        id: key.first()?.parse().ok()?,
        system_id: v.first()?.parse().ok()?,
        hub_system: v.get(1)?.parse().ok()?,
        hub_id: v.get(2)?.parse().ok()?,
        position: [num(3)?, num(4)?, num(5)?],
        spawned: v.get(6)?.parse().ok()?,
    })
}

/// The node `id`, if one lives. The id is the row's key.
pub fn node(d: &Deltas, id: i64) -> Option<Node> {
    let key = vec![id.to_string()];
    d.of(KIND).get(&key).and_then(|v| parse_row(&key, v))
}

/// Every node, in id order. The id counter's row is not a node.
pub fn all(d: &Deltas) -> Vec<Node> {
    d.of(KIND)
        .iter()
        .filter_map(|(k, v)| parse_row(k, v))
        .collect()
}

/// The nodes of one campaign (`hub_id`).
pub fn nodes_of(d: &Deltas, hub_id: i64) -> Vec<Node> {
    all(d).into_iter().filter(|n| n.hub_id == hub_id).collect()
}

fn put(d: &mut Deltas, n: &Node) {
    d.put(
        KIND,
        vec![n.id.to_string()],
        vec![
            n.system_id.to_string(),
            n.hub_system.to_string(),
            n.hub_id.to_string(),
            n.position[0].to_string(),
            n.position[1].to_string(),
            n.position[2].to_string(),
            n.spawned.to_string(),
        ],
    );
}

/// The next node id. The counter is a row of its own, so an id is never
/// handed out twice, across restarts too (a cached client would otherwise
/// hold a stale object under a reused id).
fn mint(d: &mut Deltas) -> i64 {
    let key = vec!["seq".to_owned()];
    let next = d
        .of(KIND)
        .get(&key)
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    d.put(KIND, key, vec![(next + 1).to_string()]);
    NODE_ID_BASE + next % (NODE_ID_END - NODE_ID_BASE)
}

/// What the campaign keeps of the fight in [`Campaign::rest`] **[D3b's
/// fields]**: when the next node is due, and how many have spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fight {
    pub next_wave: BlueTime,
    pub spawned: u32,
}

impl Fight {
    /// The fight a campaign's `rest` holds; `None` for a campaign nodes
    /// have not been made for.
    pub fn of(k: &Campaign) -> Option<Fight> {
        Some(Fight {
            next_wave: k.rest.first()?.parse().ok()?,
            spawned: k.rest.get(1)?.parse().ok()?,
        })
    }

    fn write(self) -> Vec<String> {
        vec![self.next_wave.to_string(), self.spawned.to_string()]
    }
}

// ---------------------------------------------------------------------------
// Pure rules: placement, the table, the bar
// ---------------------------------------------------------------------------

/// splitmix64: a deterministic stream, so a node's place is a function of
/// its campaign and id (a test can predict it, a restart does not move it).
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `0.0..1.0` from a hash.
fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// **Where a node goes** in a system whose star is at `star`: an angle, a
/// distance in `[NODE_MIN_DISTANCE, NODE_MAX_DISTANCE]` and a height within
/// `NODE_MAX_ALTITUDE`, all from `(hub_id, id)` **[A]**.
pub fn node_position(star: [f64; 3], hub_id: i64, id: i64) -> [f64; 3] {
    let h = mix(hub_id as u64 ^ mix(id as u64));
    let angle = unit(h) * std::f64::consts::TAU;
    let distance = NODE_MIN_DISTANCE + unit(mix(h)) * (NODE_MAX_DISTANCE - NODE_MIN_DISTANCE);
    let height = (unit(mix(mix(h))) - 0.5) * 2.0 * NODE_MAX_ALTITUDE;
    [
        star[0] + angle.cos() * distance,
        star[1] + height,
        star[2] + angle.sin() * distance,
    ]
}

/// **The system the next node goes to**: of `candidates` (the hub's system
/// first, then the rest of its constellation in id order), the one with the
/// fewest nodes, the earliest on a tie; none once every system holds
/// [`MAX_PER_SYSTEM`] or the constellation holds [`MAX_PER_CONSTELLATION`].
/// `counts` is the nodes standing, by system (every campaign's).
pub fn choose_system(candidates: &[i64], counts: &BTreeMap<i64, usize>) -> Option<i64> {
    let held = |s: &i64| counts.get(s).copied().unwrap_or(0);
    if candidates.iter().map(held).sum::<usize>() >= MAX_PER_CONSTELLATION {
        return None;
    }
    candidates
        .iter()
        .copied()
        .filter(|s| held(s) < MAX_PER_SYSTEM)
        .min_by_key(|s| held(s))
}

/// **The decision table of a node** (R-SOV §6.4). `d` is the defending
/// alliance: its links score for team `d`, every other alliance's for `-d`.
/// `s` is the bar now and `controlling` the team that holds it.
pub fn decide_node(
    spec: Spec,
    d: i64,
    s: f64,
    controlling: Option<i64>,
    scorers: &BTreeSet<i64>,
) -> Decision {
    let controlling = if s <= EPSILON { None } else { controlling };
    let regen = if s > EPSILON { -spec.regen } else { 0.0 };
    let attackers = scorers.iter().any(|a| *a != d);
    let defending = scorers.contains(&d);
    // A side scoring pushes the bar up when it holds it (or it is empty) and
    // takes the other side's down first otherwise.
    let toward = |side: i64| match controlling {
        Some(c) if c != side => Decision {
            controlling: Some(c),
            scoring: Some(side),
            base: -1.0,
            blocked: false,
        },
        _ => Decision {
            controlling: Some(side),
            scoring: Some(side),
            base: 1.0,
            blocked: false,
        },
    };
    match (attackers, defending) {
        (true, true) => Decision {
            controlling,
            scoring: controlling,
            base: 0.0,
            blocked: true,
        },
        (true, false) => toward(-d),
        (false, true) => toward(d),
        (false, false) => Decision {
            controlling,
            scoring: None,
            base: regen,
            blocked: false,
        },
    }
}

/// The same signature `entosis::step` compares: what must be pushed when it
/// changes (the extrapolation's base does not count).
fn signature(s: &Score) -> (Option<i64>, Option<i64>, u64, bool) {
    (s.controlling, s.scoring, s.rate.to_bits(), s.blocked)
}

/// **One step of a node's score at `now`** (the node twin of
/// `entosis::step`): carry the stored tuple, notice a completion, ask
/// [`decide_node`], make a new tuple only on an edge.
pub fn node_step(
    prev: Option<Score>,
    now: BlueTime,
    spec: Spec,
    d: i64,
    scorers: &BTreeSet<i64>,
    clock: GameClock,
) -> Step {
    let p = prev.unwrap_or_else(|| Score::idle(now));
    let s = p.at(now, spec.capture_seconds);
    if p.rate > 0.0 && s >= 1.0 - EPSILON && !p.blocked {
        return Step {
            score: Score::idle(now),
            changed: true,
            done: Some(Done::Full {
                team: p.controlling,
            }),
        };
    }
    let dec = decide_node(spec, d, s, p.controlling, scorers);
    let next = Score {
        controlling: dec.controlling,
        scoring: dec.scoring,
        s0: s,
        t0: now,
        rate: clock.rate(dec.base),
        blocked: dec.blocked,
    };
    let unchanged = prev.is_some() && signature(&next) == signature(&p);
    let first_and_idle = prev.is_none() && signature(&next) == signature(&Score::idle(now));
    if unchanged {
        return Step {
            score: p,
            changed: false,
            done: None,
        };
    }
    if first_and_idle {
        return Step {
            score: Score::idle(now),
            changed: false,
            done: None,
        };
    }
    Step {
        score: next,
        changed: true,
        done: None,
    }
}

/// Who a captured node is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    Defenders,
    /// An attacking alliance, `None` when none is known (the `-D` key).
    Attackers(Option<i64>),
}

/// **The bar after a node was captured for `winner`**: `step` moves from the
/// other side to it, the total stays what it was. Defenders take from the
/// `-D` share first, then from the alliances' keys in id order; an attacking
/// alliance takes from the defender and gets a key of its own. The defender's
/// and the `-D` keys stay even at zero; any other key at zero goes.
pub fn moved(scores: &[(i64, f64)], d: i64, winner: Winner, step: f64) -> Vec<(i64, f64)> {
    let mut m: BTreeMap<i64, f64> = scores.iter().copied().collect();
    m.entry(d).or_insert(0.0);
    m.entry(-d).or_insert(0.0);
    match winner {
        Winner::Defenders => {
            let mut left = step;
            let mut keys: Vec<i64> = vec![-d];
            keys.extend(m.keys().copied().filter(|k| *k != d && *k != -d));
            for k in keys {
                let take = left.min(m[&k]);
                *m.entry(k).or_insert(0.0) -= take;
                left -= take;
            }
            *m.entry(d).or_insert(0.0) += step - left;
        }
        Winner::Attackers(a) => {
            let take = step.min(m[&d]);
            *m.entry(d).or_insert(0.0) -= take;
            let key = a.filter(|a| *a > 0 && *a != d).unwrap_or(-d);
            *m.entry(key).or_insert(0.0) += take;
        }
    }
    let mut out = vec![(d, clean(m[&d])), (-d, clean(m[&-d]))];
    out.extend(
        m.iter()
            .filter(|(k, v)| **k != d && **k != -d && **v > EPSILON)
            .map(|(k, v)| (*k, *v)),
    );
    out
}

fn clean(v: f64) -> f64 {
    if v.abs() < EPSILON { 0.0 } else { v }
}

/// The defender's share of a bar.
pub fn defender_share(scores: &[(i64, f64)], d: i64) -> f64 {
    scores
        .iter()
        .find(|(t, _)| *t == d)
        .map_or(0.0, |(_, s)| *s)
}

/// **The result at the threshold**: the defenders' share at 1.0 is a defence,
/// at 0.0 the attackers have won; `None` while the bar is between.
pub fn verdict(scores: &[(i64, f64)], d: i64) -> Option<Outcome> {
    let share = defender_share(scores, d);
    if share >= 1.0 - EPSILON {
        Some(Outcome::DefendersWin)
    } else if share <= EPSILON {
        Some(Outcome::AttackersWin)
    } else {
        None
    }
}

/// **The result when time runs out** ([`FIGHT_LIMIT`]): whoever holds more
/// of the bar; the defenders on a tie.
pub fn at_limit(scores: &[(i64, f64)], d: i64) -> Outcome {
    if defender_share(scores, d) >= 0.5 - EPSILON {
        Outcome::DefendersWin
    } else {
        Outcome::AttackersWin
    }
}

/// When the next node is due, `after` the one just made: [`WAVE_INTERVAL`]
/// game minutes times a factor in `[0.5, 1.5)` from `(hub_id, n)`, on the
/// clock.
pub fn next_wave(after: BlueTime, hub_id: i64, n: u32, clock: GameClock) -> BlueTime {
    let factor = 0.5 + unit(mix(hub_id as u64 ^ mix(u64::from(n) + 77)));
    after + (clock.span(WAVE_INTERVAL) as f64 * factor) as i64
}

// ---------------------------------------------------------------------------
// The ball and the slim
// ---------------------------------------------------------------------------

/// The node type's radius: the type table's, else [`NODE_RADIUS`].
pub fn node_radius(content: &crate::content::Content) -> f64 {
    content
        .try_get::<crate::typedata::Installed>()
        .and_then(|t| t.table().get(TYPE_COMMAND_NODE))
        .map(|r| r.radius)
        .filter(|r| *r > 0.0)
        .unwrap_or(NODE_RADIUS)
}

/// A node's ball: it never moves, owned by SCC.
pub fn ball(reg: &Registry, n: &Node) -> Ball {
    Ball {
        ball_id: n.id,
        mode: Mode::Stop,
        radius: node_radius(reg.content()) as f32,
        position: n.position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(sov::NEUTRAL_HUB_OWNER).unwrap_or(-1),
        alliance_id: -1,
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// A node's slim: SCC's, no pilot.
pub fn slim(n: &Node) -> Slim {
    Slim {
        item_id: n.id,
        type_id: TYPE_COMMAND_NODE,
        owner_id: sov::NEUTRAL_HUB_OWNER,
        char_id: None,
        corp_id: Some(sov::NEUTRAL_HUB_OWNER),
        response_thresholds: None,
    }
}

/// The defender of the campaign a node belongs to.
fn defender_of(d: &Deltas, n: &Node) -> Option<i64> {
    sov_campaign::campaign(d, n.hub_system)
        .filter(|k| k.hub_id == n.hub_id)
        .map(|k| k.defender)
}

/// `campaign_sourceInfo` **[V]** (`infoContentBase.ConstructHeaderForEntoisItems`):
/// `(sourceSolarSystemID, eventTypeID, sourceItemID, defenderAllianceID)`.
pub fn source_info(d: &Deltas, n: &Node) -> PyValue {
    PyValue::Tuple(vec![
        PyValue::Int(n.hub_system),
        PyValue::Int(entosis::EVENT_TYPE_HUB),
        PyValue::Int(n.hub_id),
        defender_of(d, n).map_or(PyValue::None, PyValue::Int),
    ])
}

/// A node's `entosis_score`: `None` until a link has scored on it, then the
/// stored tuple with the campaign's defender as the defender team.
pub fn slim_score(d: &Deltas, n: &Node) -> PyValue {
    match (entosis::score_row(d, n.id), defender_of(d, n)) {
        (Some((_, s)), Some(def)) => entosis::score_value(&s, def),
        _ => PyValue::None,
    }
}

fn extras_of(d: &Deltas, n: &Node) -> Vec<(&'static str, PyValue)> {
    vec![
        ("campaign_sourceInfo", source_info(d, n)),
        ("entosis_score", slim_score(d, n)),
    ]
}

/// The `slim_extra::Source` row (coordinator: `slim_extra::SOURCES`).
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    ids.iter()
        .filter_map(|id| node(w.deltas(), *id))
        .map(|n| (n.id, extras_of(w.deltas(), &n)))
        .collect()
}

/// A system's nodes, balls and slims, for `destiny::scene` (coordinator: a
/// loop beside the hub's). Global in the system, like the hub.
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .filter(|n| n.system_id == system)
        .map(|n| (ball(w, n), slim(n)))
        .collect()
}

/// Where the node `ball_id` is, for `Registry::ball_target` (coordinator:
/// one line): approach, orbit, warp to.
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    let n = node(w.deltas(), ball_id)?;
    Some(crate::model::Target {
        position: n.position,
        radius: node_radius(w.content()),
        warp_in: None,
    })
}

/// The entosis target a node is: `entosis::target_of` joins this.
pub fn target_of(w: &Registry, id: i64) -> Option<Target> {
    if !(NODE_ID_BASE..NODE_ID_END).contains(&id) {
        return None;
    }
    let n = node(w.deltas(), id)?;
    Some(Target {
        id,
        system_id: n.system_id,
        position: n.position,
        radius: node_radius(w.content()),
        spec: Spec::NODE,
    })
}

/// The characters in space in `system`: who sees a ball there.
fn in_space(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// `AddBalls2` of a node to every park in its system (it is global).
///
/// **Every node push is scheduled, not pushed**: the tick runs inside
/// `sov_hub::on_the_clock`, which drains `pending` through the grid filter
/// (writing the park model) and schedules what it got, so a push sent that
/// way is filtered twice -- harmless for an add, fatal for a `RemoveBalls`
/// (the second pass finds the ball no longer held). Scheduled directly, a
/// push is filtered once, as it leaves, and the add, the slim change and the
/// removal of one node leave in the order they were made.
fn announce_added(reg: &mut Registry, n: &Node) {
    let to = in_space(reg, n.system_id);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![ball(reg, n)],
        },
        vec![(slim(n), None)],
    )
    .map(|a| a.with_extras(slim_extras(reg, &[n.id])));
    if let Some(a) = added {
        reg.schedule(std::time::Duration::ZERO, HubAdded(a));
    }
}

/// `OnSlimItemChange` for a node: its score moved.
fn push_slim(reg: &mut Registry, n: &Node) {
    let to = in_space(reg, n.system_id);
    if to.is_empty() {
        return;
    }
    let stamp = reg.next_destiny_stamp();
    let push = HubSlimChange {
        to,
        stamp,
        slim: slim(n),
        extras: extras_of(reg.deltas(), n),
    };
    reg.schedule(std::time::Duration::ZERO, push);
}

/// `{hubID: [STRUCTURE_SCORE_UPDATED]}`: the `changes` of a bar push **[V]**.
pub fn score_changed(hub_id: i64) -> PyValue {
    PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Int(entosis::STRUCTURE_SCORE_UPDATED)]),
        PyValue::Int(hub_id),
    )])
}

/// The alliances a campaign's pushes go to: the defender and every attacker.
fn sides(k: &Campaign, also: Option<i64>) -> Vec<i64> {
    let mut v = vec![k.defender];
    v.extend(k.attackers.iter().copied());
    v.extend(also);
    v.sort_unstable();
    v.dedup();
    v
}

// ---------------------------------------------------------------------------
// Spawning and despawning
// ---------------------------------------------------------------------------

/// The systems a campaign's nodes may go to: the hub's, then the rest of its
/// constellation (not wormholes) in id order. **[A]** a hub whose system the
/// universe lacks has only its own.
pub fn systems_for(reg: &Registry, hub_system: i64) -> Vec<i64> {
    let mut v = vec![hub_system];
    let Some(u) = reg.content().try_get::<crate::celestial::Universe>() else {
        return v;
    };
    let Some(home) = u.system(hub_system) else {
        return v;
    };
    v.extend(
        u.systems()
            .filter(|s| {
                s.constellation_id == home.constellation_id
                    && s.id != hub_system
                    && s.wormhole_class_id.is_none()
            })
            .map(|s| s.id),
    );
    v
}

/// The nodes standing, by system.
fn counts(d: &Deltas) -> BTreeMap<i64, usize> {
    let mut m = BTreeMap::new();
    for n in all(d) {
        *m.entry(n.system_id).or_insert(0) += 1;
    }
    m
}

/// **Make one node** for the campaign `k` (a started one): choose the
/// system, mint the id, place it, store it, tell the pilots there. `None`
/// when every system is full.
pub fn spawn_one(reg: &mut Registry, k: &Campaign, now: BlueTime) -> Option<Node> {
    let candidates = systems_for(reg, k.system_id);
    let system = choose_system(&candidates, &counts(reg.deltas()))?;
    let id = mint(reg.deltas_mut());
    let star = reg
        .celestials()
        .star_of(system)
        .map_or([0.0; 3], |s| s.position);
    let n = Node {
        id,
        system_id: system,
        hub_system: k.system_id,
        hub_id: k.hub_id,
        position: node_position(star, k.hub_id, id),
        spawned: now,
    };
    put(reg.deltas_mut(), &n);
    announce_added(reg, &n);
    crate::errln!(
        "[sov] command node {} appears in {system} for the campaign on hub {}",
        n.id,
        k.hub_id
    );
    Some(n)
}

/// **Take a node away**: its links stop, its locks are dropped, its rows go,
/// the ball leaves every park in its system.
pub fn despawn(reg: &mut Registry, n: &Node, why: &str, now: BlueTime) {
    entosis::stop_links(reg, n.id, now, why);
    let lockers: Vec<i64> = reg
        .locks()
        .targeters_of(n.id)
        .into_iter()
        .filter_map(|ship| reg.roster().by_ship(ship).map(|p| p.character_id))
        .collect();
    for who in lockers {
        crate::targeting::unlock(reg, who, n.id, crate::targeting::Reason::Exploding);
    }
    let key = [n.id.to_string()];
    reg.deltas_mut().del(KIND, &key);
    entosis::reset(reg.deltas_mut(), n.id);
    // Scheduled, not pushed: see `announce_added`.
    let to = in_space(reg, n.system_id);
    let stamp = reg.next_destiny_stamp();
    if let Some(rm) = crate::destiny::BallsRemoved::new(to, stamp, vec![n.id]) {
        reg.schedule(std::time::Duration::ZERO, rm);
    }
}

/// Take every node of a hub's campaign away (the campaign ended, or the
/// hub went). Coordinator: one line in `entosis::forget`.
pub fn despawn_hub(reg: &mut Registry, hub_id: i64) {
    let now = crate::pyutil::filetime_now();
    for n in nodes_of(reg.deltas(), hub_id) {
        despawn(reg, &n, "The campaign is over.", now);
    }
}

// ---------------------------------------------------------------------------
// The seams D3a left
// ---------------------------------------------------------------------------

/// **The campaign started** (`sov_campaign::on_begin`): the first
/// [`INITIAL_NODES`] nodes, and the clock of the next. The fight is anchored
/// on the campaign's own start time, so a server that was down through it
/// still counts the fight from when it was due.
pub fn on_begin(reg: &mut Registry, k: &Campaign) {
    let clock = crate::game_clock::of(reg);
    let now = k.start_time;
    let mut spawned = 0;
    for _ in 0..INITIAL_NODES {
        if spawn_one(reg, k, now).is_some() {
            spawned += 1;
        }
    }
    let fight = Fight {
        next_wave: next_wave(now, k.hub_id, spawned, clock),
        spawned,
    };
    store_fight(reg, k, fight);
}

fn store_fight(reg: &mut Registry, k: &Campaign, f: Fight) {
    // Re-read: the caller's copy may predate a score change.
    let mut cur = sov_campaign::campaign(reg.deltas(), k.system_id).unwrap_or_else(|| k.clone());
    cur.rest = f.write();
    sov_campaign::put(reg.deltas_mut(), &cur);
}

/// **The campaign ended** (`sov_campaign::resolve`, after the row went): the
/// nodes go, and when the hub fell the attackers hear it too (`SovStructureDestroyed`,
/// 162; the defender's notices are D3a's).
pub fn on_resolve(reg: &mut Registry, k: &Campaign, outcome: Outcome) {
    despawn_hub(reg, k.hub_id);
    if outcome != Outcome::AttackersWin {
        return;
    }
    let mut to: Vec<i64> = k
        .attackers
        .iter()
        .flat_map(|a| entosis::alliance_managers(reg, *a, None))
        .collect();
    to.sort_unstable();
    to.dedup();
    for who in to {
        notif::notify(
            reg,
            who,
            entosis::kind::STRUCTURE_DESTROYED,
            data([
                ("structureTypeID", Value::Int(sov::TYPE_INFRASTRUCTURE_HUB)),
                ("solarSystemID", Value::Int(k.system_id)),
            ]),
        );
    }
}

/// What the activation check asks about a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Not a command node: the hub rules apply.
    NotANode,
    /// A node of a running fight: anyone with an alliance may link it.
    Allowed,
    /// A node without a fight to belong to.
    Refused(String),
}

/// **`entosis::check_target`'s node branch.** A node is linkable while its
/// campaign is in `nodefight`; either side may link it.
pub fn gate(w: &Registry, t: &Target) -> Gate {
    let Some(n) = node(w.deltas(), t.id) else {
        return Gate::NotANode;
    };
    let live = sov_campaign::campaign(w.deltas(), n.hub_system)
        .filter(|k| k.hub_id == n.hub_id && k.started)
        .is_some()
        && sov::claim(w.deltas(), n.hub_system)
            .is_some_and(|c| c.hub_id == n.hub_id && c.state == HubState::NodeFight);
    if live {
        Gate::Allowed
    } else {
        Gate::Refused("This command node belongs to no running campaign.".to_owned())
    }
}

/// Whether any link runs against a node: the movement guards ask.
pub fn linking_a_node(w: &Registry, ship_id: i64) -> bool {
    if w.deltas().of(KIND).is_empty() {
        return false;
    }
    all(w.deltas()).iter().any(|n| {
        entosis::links_on(w, n.id)
            .iter()
            .any(|l| l.run.ship_id == ship_id)
    })
}

/// The module ids of every link on a node: the engine keeps their fuel
/// accounts (`entosis::tick` retains only the living).
pub fn link_modules(w: &Registry) -> Vec<i64> {
    if w.deltas().of(KIND).is_empty() {
        return Vec::new();
    }
    all(w.deltas())
        .iter()
        .flat_map(|n| entosis::links_on(w, n.id))
        .map(|l| l.run.module_id)
        .collect()
}

// ---------------------------------------------------------------------------
// The tick
// ---------------------------------------------------------------------------

/// **The fight's half of the stage** (called by `sov_campaign::tick`, which
/// `entosis::tick` calls): drop what lost its campaign, make the nodes that
/// are due, settle every node's score, end a fight that ran out.
pub fn tick(reg: &mut Registry, now: BlueTime) {
    let started: Vec<Campaign> = sov_campaign::all(reg.deltas())
        .into_iter()
        .filter(|k| k.started)
        .collect();
    let nodes = all(reg.deltas());
    if nodes.is_empty() && started.is_empty() {
        return;
    }
    let clock = crate::game_clock::of(reg);
    // A node whose campaign is gone (a GM moved the claim, the hub lapsed).
    for n in &nodes {
        if !fight_is_on(reg, n) {
            despawn(reg, n, "The campaign is over.", now);
        }
    }
    // A fight nobody decided ends before it makes another node.
    for k in &started {
        let Some(cur) = sov_campaign::campaign(reg.deltas(), k.system_id) else {
            continue;
        };
        if cur.hub_id == k.hub_id && now >= cur.start_time + clock.span(FIGHT_LIMIT) {
            let outcome = at_limit(&cur.scores, cur.defender);
            crate::errln!(
                "[sov] the campaign on hub {} ran out of time: {outcome:?}",
                cur.hub_id
            );
            if let Err(e) = sov_campaign::resolve(reg, cur.system_id, outcome) {
                crate::errln!("[sov] campaign in {} not resolved: {e}", cur.system_id);
            }
        }
    }
    // The nodes that are due.
    for k in &started {
        wave(reg, k, now, clock);
    }
    // Every standing node's score.
    for n in all(reg.deltas()) {
        settle_node(reg, &n, now, clock);
    }
}

/// Whether the campaign a node belongs to is still in its fight.
fn fight_is_on(reg: &Registry, n: &Node) -> bool {
    sov_campaign::campaign(reg.deltas(), n.hub_system)
        .is_some_and(|k| k.hub_id == n.hub_id && k.started)
        && sov::claim(reg.deltas(), n.hub_system)
            .is_some_and(|c| c.hub_id == n.hub_id && c.state == HubState::NodeFight)
}

/// Make the next node of `k` if it is due (or the first, for a campaign
/// that began before nodes existed).
fn wave(reg: &mut Registry, k: &Campaign, now: BlueTime, clock: GameClock) {
    let Some(cur) = sov_campaign::campaign(reg.deltas(), k.system_id) else {
        return;
    };
    if cur.hub_id != k.hub_id {
        return;
    }
    let Some(fight) = Fight::of(&cur) else {
        // A campaign from before D3b: start its fight now.
        on_begin(reg, &cur);
        return;
    };
    if now < fight.next_wave {
        return;
    }
    let spawned = fight.spawned + u32::from(spawn_one(reg, &cur, now).is_some());
    store_fight(
        reg,
        &cur,
        Fight {
            next_wave: next_wave(now, cur.hub_id, spawned, clock),
            spawned,
        },
    );
}

/// Settle one node: its links, its score, and what the score completed.
fn settle_node(reg: &mut Registry, n: &Node, now: BlueTime, clock: GameClock) {
    // An earlier node's capture may have ended the campaign.
    if node(reg.deltas(), n.id).is_none() {
        return;
    }
    let Some(k) = sov_campaign::campaign(reg.deltas(), n.hub_system)
        .filter(|k| k.hub_id == n.hub_id && k.started)
    else {
        return;
    };
    let Some(t) = target_of(reg, n.id) else {
        return;
    };
    let stored = entosis::score_row(reg.deltas(), n.id).map(|(_, s)| s);
    let links = entosis::links_on(reg, n.id);
    if links.is_empty() && stored.is_none() {
        return;
    }
    let links = entosis::upkeep(reg, links, &t, now);
    let scoring = entosis::scorers(&links, now);
    let out = node_step(stored, now, t.spec, k.defender, &scoring, clock);
    match out.done {
        Some(Done::Full { team }) => {
            captured(reg, n, &k, team, &scoring, now);
        }
        None if out.changed => {
            let first_attack = out.score.rate > 0.0
                && !out.score.blocked
                && out.score.scoring == Some(-k.defender)
                && stored.is_none_or(|p| p.controlling.is_none());
            entosis::put_score(reg.deltas_mut(), n.id, n.system_id, &out.score);
            push_slim(reg, n);
            if first_attack {
                notify_attacked(reg, n, &k);
            }
        }
        None => {}
    }
}

/// `EntosisCaptureStarted` (147) with the node as the structure, to the
/// defender's managers **[A]**.
fn notify_attacked(reg: &mut Registry, n: &Node, k: &Campaign) {
    for to in entosis::alliance_managers(reg, k.defender, None) {
        notif::notify(
            reg,
            to,
            entosis::kind::ENTOSIS_CAPTURE_STARTED,
            data([
                ("structureTypeID", Value::Int(TYPE_COMMAND_NODE)),
                ("solarSystemID", Value::Int(n.system_id)),
            ]),
        );
    }
}

/// **A node was captured** by `team` (`D` for the defenders, `-D` for the
/// attackers; `scoring` are the alliances whose links were scoring): the
/// node goes, the bar moves and is pushed, and a side at the threshold ends
/// the campaign.
fn captured(
    reg: &mut Registry,
    n: &Node,
    k: &Campaign,
    team: Option<i64>,
    scoring: &BTreeSet<i64>,
    now: BlueTime,
) {
    let winner = if team == Some(k.defender) {
        Winner::Defenders
    } else {
        Winner::Attackers(
            scoring
                .iter()
                .copied()
                .find(|a| *a != k.defender)
                .or_else(|| k.attackers.first().copied()),
        )
    };
    despawn(reg, n, "The command node is captured.", now);
    apply_capture(reg, k, winner);
}

/// Move the bar of `k` for a node captured for `winner`, push it, and end
/// the campaign at the threshold. The GM's `/sov nodes capture` calls this
/// too.
pub fn apply_capture(reg: &mut Registry, k: &Campaign, winner: Winner) {
    let Some(mut cur) =
        sov_campaign::campaign(reg.deltas(), k.system_id).filter(|c| c.hub_id == k.hub_id)
    else {
        return;
    };
    cur.scores = moved(&cur.scores, cur.defender, winner, NODE_STEP);
    if let Winner::Attackers(Some(a)) = winner
        && !cur.attackers.contains(&a)
    {
        cur.attackers.push(a);
    }
    sov_campaign::put(reg.deltas_mut(), &cur);
    sov::push_structures(
        reg,
        cur.system_id,
        &sides(&cur, None),
        Some(score_changed(cur.hub_id)),
    );
    crate::errln!(
        "[sov] a node of hub {} was taken by the {winner:?}: the bar is {:?}",
        cur.hub_id,
        cur.scores
    );
    if let Some(outcome) = verdict(&cur.scores, cur.defender)
        && let Err(e) = sov_campaign::resolve(reg, cur.system_id, outcome)
    {
        crate::errln!("[sov] campaign in {} not resolved: {e}", cur.system_id);
    }
}

// ---------------------------------------------------------------------------
// /sov nodes
// ---------------------------------------------------------------------------

/// The usage line for `/sov nodes` (`entosis::USAGE` and the service's).
pub const USAGE: &str = "/sov nodes [systemID|here] | /sov nodes spawn [systemID|here] | /sov nodes capture <nodeID> attackers|defenders";

fn parse_system(reg: &Registry, actor: Option<i64>, word: Option<&&str>) -> Result<i64, String> {
    match word {
        None | Some(&"here") => actor
            .and_then(|id| reg.session_system(id))
            .ok_or_else(|| "you are not in a system".to_owned()),
        Some(w) => w
            .parse::<i64>()
            .map_err(|_| format!("'{w}' is not a system id")),
    }
}

/// The campaign that has `system` as its hub's system, or a node in it.
fn campaign_for(reg: &Registry, system: i64) -> Option<Campaign> {
    sov_campaign::campaign(reg.deltas(), system).or_else(|| {
        all(reg.deltas())
            .into_iter()
            .find(|n| n.system_id == system)
            .and_then(|n| sov_campaign::campaign(reg.deltas(), n.hub_system))
    })
}

/// The status line of `system`'s fight: the bar and the nodes.
pub fn status_text(reg: &Registry, system: i64) -> String {
    let Some(k) = campaign_for(reg, system) else {
        return format!("system {system}: no campaign");
    };
    let now = crate::pyutil::filetime_now();
    let bar: Vec<String> = k
        .scores
        .iter()
        .map(|(t, s)| format!("{t}:{s:.3}"))
        .collect();
    let nodes: Vec<String> = nodes_of(reg.deltas(), k.hub_id)
        .iter()
        .map(|n| {
            let s = entosis::score_row(reg.deltas(), n.id)
                .map_or(0.0, |(_, s)| s.at(now, Spec::NODE.capture_seconds));
            format!("{} in {} ({s:.2})", n.id, n.system_id)
        })
        .collect();
    format!(
        "campaign on hub {} in {} ({}): bar [{}]; {} nodes: {}",
        k.hub_id,
        k.system_id,
        if k.started { "fighting" } else { "not started" },
        bar.join(" "),
        nodes.len(),
        nodes.join(", ")
    )
}

/// `/sov nodes ...` (`entosis::slash` hands over).
pub fn slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let usage = || format!("usage: {USAGE}");
    match args {
        [] | [_] => match parse_system(reg, actor, args.first()) {
            Ok(s) => status_text(reg, s),
            Err(e) => format!("{} -- {e}", usage()),
        },
        ["spawn", rest @ ..] => match parse_system(reg, actor, rest.first()) {
            Ok(system) => {
                let Some(k) = campaign_for(reg, system).filter(|k| k.started) else {
                    return format!("system {system}: no campaign in its node fight");
                };
                match spawn_one(reg, &k, crate::pyutil::filetime_now()) {
                    Some(n) => format!("node {} in {}", n.id, n.system_id),
                    None => "every system of the constellation is full".to_owned(),
                }
            }
            Err(e) => format!("{} -- {e}", usage()),
        },
        ["capture", id, who] => {
            let (Ok(id), true) = (id.parse::<i64>(), matches!(*who, "attackers" | "defenders"))
            else {
                return usage();
            };
            let Some(n) = node(reg.deltas(), id) else {
                return format!("no node {id}");
            };
            let Some(k) = sov_campaign::campaign(reg.deltas(), n.hub_system)
                .filter(|k| k.hub_id == n.hub_id && k.started)
            else {
                return format!("node {id} has no running campaign");
            };
            let winner = if *who == "defenders" {
                Winner::Defenders
            } else {
                Winner::Attackers(k.attackers.first().copied())
            };
            let now = crate::pyutil::filetime_now();
            despawn(reg, &n, "The command node is captured.", now);
            apply_capture(reg, &k, winner);
            format!("node {id} captured for the {who}")
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests;
