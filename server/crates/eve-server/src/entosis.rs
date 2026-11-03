//! **The entosis engine**: the link module (effect 6063), the score a hub
//! shows, the capture of a neutral hub, the attack on a held one and the
//! windows it may happen in (SOC-D3a; lane note `docs/lanes/sov-d3a.md`,
//! spec R-SOV §5.3, §6.3, §6.4, §9 D3a).
//!
//! # What a link does
//!
//! An Entosis Link is a target module (`effectEntosisLink`, 6063, guid
//! `effects.EntosisBeam`) **[V-data]**. Its first cycle is **warm-up**
//! **[A]**: scoring starts when that cycle ends. From then on the link is
//! *in scoring phase* for as long as the module runs. Each cycle burns the
//! fuel the module names (`consumptionType` 713 / `consumptionQuantity`
//! 714) from the ship's cargo; a cycle with no fuel stops the module. A
//! link also stops when the ship drifts out of range, and while it runs the
//! ship cannot warp, jump or dock ([`linking`], [`refuse_dock`]). Only
//! one link runs per ship.
//!
//! The links on a target do not move a number each; they pick a **rate**
//! for one number, the target's score `s` in `0..=1` ([`decide`]):
//!
//! | target | only attackers | only the owner | both | nobody |
//! |---|---|---|---|---|
//! | neutral hub | one alliance: +1; two or more: blocked | | | regen, down |
//! | held hub | team -D (everyone but the owner D): +1 | -1 | blocked | regen, down |
//!
//! `+1` is a full capture in `captureTimeSeconds` (595 s for a hub
//! **[V-data]**); regen is `defensiveRegenRate` (0.2) **[V-data]**. At
//! `s = 1` a neutral hub is **claimed** ([`claim_neutral`], the same
//! function `/sov capture` calls) and a held hub is **reinforced**
//! ([`reinforce`], with a [`crate::sov_campaign::Campaign`] scheduled).
//!
//! # The score on the wire [V]
//!
//! The hub's slim carries `entosis_score = (controllingTeam, scoringTeam,
//! (s0, t0, rate, isBlocked), defenderTeam)` (`entosisController`). The
//! client extrapolates `s = s0 + (now - t0) / captureTime / SEC * rate`,
//! clamped, and draws the countdown itself, so **the server pushes only
//! on edges** (a link starts or stops scoring, a block begins or ends, a
//! completion) and never per tick ([`step`]). Two details of the client
//! decide what is sent:
//!
//! - it **ignores a `None` score** (`__UpdateEntosisScoreFromSlimItem`
//!   tests `is not None`), so a finished capture is cleared by an *idle*
//!   tuple (rate 0), never by `None`; the row therefore stays, idle, once a
//!   hub has been scored on;
//! - a negative rate with a score above 0 keeps its countdown running, so
//!   a regress that reached 0 is pushed as an idle tuple too.
//!
//! Team ids: positive is an alliance, `-A` is "everyone except alliance
//! A", `1` ([`NEUTRAL_TEAM`]) is the neutral defender **[V]**
//! (`entosis.ui.IsSameCaptureTeam`).
//!
//! # Game time (lane CLOCK)
//!
//! One clock, the wall clock. A game span is compressed when a deadline is
//! made from it (`GameClock::span`), and the **rate** the client extrapolates
//! with is multiplied by the scale (`GameClock::rate`), so a 595-second
//! capture takes `595 / N` wall seconds at `--time-scale N` and the
//! client's own countdown agrees. Module cycles stay on real time.
//!
//! # Windows
//!
//! A held hub can be attacked only inside its alliance's **vulnerability
//! window** ([`window_around`]): `18 h / ADM` (D4's
//! `vulnerability_window`), centred on the middle of the prime hour, UTC,
//! repeating every game day (`GameClock::day`). A capture already in
//! progress when the window ends continues, and either side may link, until
//! the score is back to 0 (the client's `VULNERABLE_OVERTIME`) **[I]**. A
//! neutral hub has no window. An `offline` hub (out of fuel, D2) is open
//! the whole game day **[A]**, R-SOV §6.7.
//!
//! # State
//!
//! ```text
//! entosis <targetID> -> systemID controlling scoring s0 t0 rate blocked
//! ```
//!
//! `-` is absent; `rate` is the wall rate (already multiplied by the
//! scale), so a world restarted at another scale keeps its tuple until the
//! next edge. The defender is never stored: it is read from the claim. The
//! links are never stored either: they are read each tick from the running
//! modules (`modules().running_against`). Only the fuel already paid
//! ([`Entosis::paid`]) and the last window phase pushed live in the world
//! slot ([`SLOT`]).
//!
//! # What D3b takes over
//!
//! The campaign's command nodes and the node fight are D3b's
//! (`sov_campaign`). Here: [`Spec::NODE`] and the [`decide`] table for a
//! node are written so the same engine scores a node, and [`target_of`] is
//! where a node target joins the hubs.

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::game_clock::GameClock;
use crate::modules::{Running, find_in as module_in, type_arg, user_error};
use crate::notifications::{self as notif, Value, data};
use crate::service::{Registry, ServiceError};
use crate::sov::{self, Claim, HubState};
use crate::sov_hub;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

/// The delta kind this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "entosis";

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `effectEntosisLink` **[V-data]** (R-SOV §2): category 2 (target), guid
/// `effects.EntosisBeam`, duration attribute 73, range 54, discharge 6.
pub const EFFECT_ENTOSIS_LINK: i64 = 6_063;
/// `maxRange` **[V]**.
pub const ATTR_MAX_RANGE: i64 = 54;
/// `consumptionType` and `consumptionQuantity` **[V-data]**: the fuel type
/// a link burns and how much a cycle (Strontium Clathrates, 1).
pub const ATTR_FUEL_TYPE: i64 = 713;
pub const ATTR_FUEL_QUANTITY: i64 = 714;

/// `entosisSovereigntyHub.captureTimeSeconds` and `defensiveRegenRate`
/// **[V-data]**; the server must use the client's numbers or the timers
/// lie (R-SOV §5.3).
pub const HUB_CAPTURE_SECONDS: f64 = 595.0;
pub const HUB_REGEN_RATE: f64 = 0.2;
/// `entosisCommandNode`'s **[V-data]** (EvEJS has 240 / 0.4: ours wins).
/// For D3b.
pub const NODE_CAPTURE_SECONDS: f64 = 235.0;
pub const NODE_REGEN_RATE: f64 = 0.04;

/// `invConst.ownerSystem` **[V]**: the neutral team.
pub const NEUTRAL_TEAM: i64 = 1;
/// `entosisConst.EVENT_TYPE_IHUB_DEFENSE` **[V]**: the campaign type of a
/// hub (R-SOV §2).
pub const EVENT_TYPE_HUB: i64 = 2;
/// `entosisConst.STRUCTURE_SCORE_UPDATED` / `STRUCTURES_UPDATED` **[V]**.
pub const STRUCTURE_SCORE_UPDATED: i64 = 0;
pub const STRUCTURES_UPDATED: i64 = 1;

/// The hub type's signature radius **[V-data]** (R-SOV §1.1), which the
/// client's lock-time formula reads.
pub const SIGNATURE_RADIUS: f64 = 4_000.0;

/// **[A]** how far past the link's `maxRange` a ship may drift before the
/// link stops, metres. Falsified by a link that stops while the client's
/// range readout still shows inside range.
pub const RANGE_SLACK: f64 = 500.0;

/// **[A]** the wall time between two scans of the windows (a vulnerability
/// edge is pushed at most this late), FILETIME ticks. Falsified by a
/// dashboard that shows the wrong status for longer than a second.
pub const WINDOW_SCAN: i64 = 10_000_000;

/// The notification types sov sends (`notificationconst`) **[V]**; the data
/// keys are the formatters' (`notifications/common/formatters/sov*`,
/// `notificationUtil.formatters`).
pub mod kind {
    pub const ALLIANCE_CLAIM_LOST: i64 = 41;
    pub const ALLIANCE_CLAIM_ACQUIRED: i64 = 43;
    pub const ENTOSIS_CAPTURE_STARTED: i64 = 147;
    pub const STRUCTURE_REINFORCED: i64 = 160;
    pub const COMMAND_NODE_EVENT_STARTED: i64 = 161;
    pub const STRUCTURE_DESTROYED: i64 = 162;
}

/// What the engine needs to know about a kind of target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// `captureTimeSeconds`.
    pub capture_seconds: f64,
    /// `defensiveRegenRate`: how fast an unattended score falls, in
    /// capture-times per capture-time.
    pub regen: f64,
}

impl Spec {
    pub const HUB: Spec = Spec {
        capture_seconds: HUB_CAPTURE_SECONDS,
        regen: HUB_REGEN_RATE,
    };
    /// A command node (D3b).
    pub const NODE: Spec = Spec {
        capture_seconds: NODE_CAPTURE_SECONDS,
        regen: NODE_REGEN_RATE,
    };
}

const TICKS_PER_SECOND: f64 = 10_000_000.0;
/// Within this of 1.0 (or 0.0) a score is complete: float dust from an
/// extrapolation.
const EPSILON: f64 = 1e-9;

// ---------------------------------------------------------------------------
// The score
// ---------------------------------------------------------------------------

/// One target's score as last decided: what `entosis_score` carries and
/// what the server extrapolates from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    /// The team that holds the progress (`None` at 0).
    pub controlling: Option<i64>,
    /// The team whose links are moving it now.
    pub scoring: Option<i64>,
    /// The score at `t0`.
    pub s0: f64,
    /// FILETIME the tuple was made.
    pub t0: BlueTime,
    /// Capture-times per wall second, signed, **scale included**.
    pub rate: f64,
    pub blocked: bool,
}

impl Score {
    /// Nothing happening, as of `t0`.
    pub fn idle(t0: BlueTime) -> Score {
        Score {
            controlling: None,
            scoring: None,
            s0: 0.0,
            t0,
            rate: 0.0,
            blocked: false,
        }
    }

    /// The score at `now`: the client's formula, clamped.
    pub fn at(&self, now: BlueTime, capture_seconds: f64) -> f64 {
        let seconds = (now - self.t0).max(0) as f64 / TICKS_PER_SECOND;
        (self.s0 + seconds / capture_seconds * self.rate).clamp(0.0, 1.0)
    }

    /// When the score reaches the end it is heading for (1.0 for a rising
    /// rate, 0.0 for a falling one); `None` when it is not moving.
    pub fn completes_at(&self, capture_seconds: f64) -> Option<BlueTime> {
        if self.rate == 0.0 || self.blocked {
            return None;
        }
        let target = if self.rate > 0.0 { 1.0 } else { 0.0 };
        let seconds = (target - self.s0) / self.rate * capture_seconds;
        Some(self.t0 + (seconds.max(0.0) * TICKS_PER_SECOND).ceil() as i64)
    }

    /// What decides whether a new tuple must be pushed: everything but the
    /// extrapolation's base.
    fn signature(&self) -> (Option<i64>, Option<i64>, u64, bool) {
        (
            self.controlling,
            self.scoring,
            self.rate.to_bits(),
            self.blocked,
        )
    }
}

/// Who defends the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Defender {
    /// A neutral hub (team 1).
    Neutral,
    /// The alliance that holds it.
    Owned(i64),
}

impl Defender {
    pub fn team(self) -> i64 {
        match self {
            Defender::Neutral => NEUTRAL_TEAM,
            Defender::Owned(a) => a,
        }
    }
}

/// What the links on a target make of its score (R-SOV §6.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    pub controlling: Option<i64>,
    pub scoring: Option<i64>,
    /// `+1`, `-1`, `-regen` or 0, before the clock's scale.
    pub base: f64,
    pub blocked: bool,
}

/// **The decision table.** `scorers` are the alliances with a link in
/// scoring phase; `s` is the score now and `controlling` who holds it.
pub fn decide(
    spec: Spec,
    defender: Defender,
    s: f64,
    controlling: Option<i64>,
    scorers: &BTreeSet<i64>,
) -> Decision {
    let controlling = if s <= EPSILON { None } else { controlling };
    let regen = if s > EPSILON { -spec.regen } else { 0.0 };
    let rest = |controlling: Option<i64>| Decision {
        controlling,
        scoring: None,
        base: regen,
        blocked: false,
    };
    let blocked = |controlling: Option<i64>| Decision {
        controlling,
        scoring: controlling,
        base: 0.0,
        blocked: true,
    };
    match defender {
        Defender::Neutral => match scorers.len() {
            0 => rest(controlling),
            1 => {
                let a = *scorers.iter().next().unwrap_or(&0);
                match controlling {
                    // Whoever scores on an empty or its own bar pushes it up.
                    None => Decision {
                        controlling: Some(a),
                        scoring: Some(a),
                        base: 1.0,
                        blocked: false,
                    },
                    Some(c) if c == a => Decision {
                        controlling: Some(a),
                        scoring: Some(a),
                        base: 1.0,
                        blocked: false,
                    },
                    // Another alliance holds progress: this one first
                    // takes it down.
                    Some(c) => Decision {
                        controlling: Some(c),
                        scoring: Some(a),
                        base: -1.0,
                        blocked: false,
                    },
                }
            }
            _ => blocked(controlling),
        },
        Defender::Owned(d) => {
            let attackers = scorers.iter().any(|a| *a != d);
            let defending = scorers.contains(&d);
            let team = -d;
            match (attackers, defending) {
                (true, true) => blocked(Some(team)),
                (true, false) => Decision {
                    controlling: Some(team),
                    scoring: Some(team),
                    base: 1.0,
                    blocked: false,
                },
                (false, true) if s > EPSILON => Decision {
                    controlling: Some(team),
                    scoring: Some(d),
                    base: -1.0,
                    blocked: false,
                },
                (false, true) => rest(None),
                (false, false) => rest(controlling),
            }
        }
    }
}

/// How a score ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    /// It reached 1.0 under `team` (`None` if the table named none).
    Full { team: Option<i64> },
}

/// What one [`step`] made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    /// The score to keep.
    pub score: Score,
    /// Whether a new tuple must be pushed (an edge).
    pub changed: bool,
    pub done: Option<Done>,
}

/// **One step of a target's score at `now`.**
///
/// Carries the stored tuple up to `now` with its own rate, so nothing jumps;
/// notices a completion; then asks [`decide`] about the links and, only if
/// the answer differs in team, rate or block, makes a new tuple based at
/// `now`. An unchanged answer leaves the stored tuple (and so the client's
/// extrapolation) untouched.
pub fn step(
    prev: Option<Score>,
    now: BlueTime,
    spec: Spec,
    defender: Defender,
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
    let d = decide(spec, defender, s, p.controlling, scorers);
    let rate = clock.rate(d.base);
    let next = Score {
        controlling: d.controlling,
        scoring: d.scoring,
        s0: s,
        t0: now,
        rate,
        blocked: d.blocked,
    };
    if prev.is_some() && next.signature() == p.signature() {
        return Step {
            score: p,
            changed: false,
            done: None,
        };
    }
    // A first sighting with nothing scoring is not an edge.
    if prev.is_none() && next.signature() == Score::idle(now).signature() {
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

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// A span of FILETIME.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: BlueTime,
    pub end: BlueTime,
}

/// **The vulnerability window around `now`**: `(current, next)`, where
/// `current` is the window that started most recently at or before `now`
/// (it may already have ended) and `next` the one after it.
///
/// Centred on the middle of `prime_hour` (UTC), `length_game` long (a game
/// span, compressed by `clock`), repeating every `clock.day()`. At scale 1
/// that is the UTC day: FILETIME's epoch is midnight and a day is
/// `seed::DAY`.
pub fn window_around(
    prime_hour: i64,
    length_game: i64,
    now: BlueTime,
    clock: GameClock,
) -> (Window, Window) {
    let period = clock.day();
    let len = clock.span(length_game).clamp(1, period - 1);
    let centre = clock.span(prime_hour.rem_euclid(24) * crate::seed::HOUR + crate::seed::HOUR / 2);
    let start_off = centre - len / 2;
    let k = (now - start_off).div_euclid(period);
    let start = k * period + start_off;
    let current = Window {
        start,
        end: start + len,
    };
    let next = Window {
        start: current.start + period,
        end: current.end + period,
    };
    (current, next)
}

/// The first window start at or after `t`: where a campaign scheduled for
/// "no earlier than `t`" begins.
pub fn window_start_at_or_after(
    prime_hour: i64,
    length_game: i64,
    t: BlueTime,
    clock: GameClock,
) -> BlueTime {
    let (current, next) = window_around(prime_hour, length_game, t, clock);
    if current.start >= t {
        current.start
    } else {
        next.start
    }
}

/// The whole game day around `now`: an offline hub's window **[A]**.
fn whole_day(now: BlueTime, clock: GameClock) -> Window {
    let period = clock.day();
    let k = now.div_euclid(period);
    Window {
        start: k * period,
        end: (k + 1) * period,
    }
}

/// Where a hub stands against attack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No window: a neutral hub.
    Open,
    /// Inside the window.
    Vulnerable(Window),
    /// The window has ended and a capture goes on.
    Overtime(Window),
    /// Outside the window; the next one.
    Invulnerable(Window),
    /// Reinforced until the campaign starts.
    Reinforced(BlueTime),
    /// The campaign's node fight is on.
    NodeFight,
}

impl Phase {
    /// A key that changes exactly when the client's status would.
    fn key(self) -> (u8, i64) {
        match self {
            Phase::Open => (0, 0),
            Phase::Vulnerable(w) => (1, w.start),
            Phase::Overtime(w) => (2, w.start),
            Phase::Invulnerable(w) => (3, w.start),
            Phase::Reinforced(t) => (4, t),
            Phase::NodeFight => (5, 0),
        }
    }

    /// Whether a fresh link may start scoring.
    pub fn attackable(self) -> bool {
        matches!(self, Phase::Open | Phase::Vulnerable(_))
    }
}

/// `c`'s phase at `now`.
pub fn phase(d: &Deltas, c: &Claim, now: BlueTime, clock: GameClock) -> Phase {
    match c.state {
        HubState::Neutral => Phase::Open,
        HubState::Reinforced => Phase::Reinforced(
            crate::sov_campaign::campaign(d, c.system_id)
                .map(|k| k.start_time)
                .or(c.state_until)
                .unwrap_or(0),
        ),
        HubState::NodeFight => Phase::NodeFight,
        HubState::Offline => Phase::Vulnerable(whole_day(now, clock)),
        HubState::Claimed => {
            let Some(holder) = c.alliance_id else {
                return Phase::Open;
            };
            let prime = sov::prime_hour(d, holder, now)
                .current
                .unwrap_or(sov::DEFAULT_PRIME_HOUR);
            let adm = crate::sov_index::system_adm(d, c.system_id, now, clock);
            let (current, next) = window_around(
                prime,
                crate::sov_index::vulnerability_window(adm),
                now,
                clock,
            );
            if now <= current.end {
                Phase::Vulnerable(current)
            } else if capture_active(d, c.hub_id, now, clock) {
                Phase::Overtime(current)
            } else {
                Phase::Invulnerable(next)
            }
        }
    }
}

/// The window the client is shown for a phase (`None` for no window).
fn shown(p: Phase) -> Option<Window> {
    match p {
        Phase::Vulnerable(w) | Phase::Overtime(w) | Phase::Invulnerable(w) => Some(w),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

fn opt(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

fn parse_opt(s: Option<&String>) -> Option<Option<i64>> {
    match s.map(String::as_str) {
        None => None,
        Some("-") => Some(None),
        Some(s) => s.parse().ok().map(Some),
    }
}

fn parse_row(v: &[String]) -> Option<(i64, Score)> {
    Some((
        v.first()?.parse().ok()?,
        Score {
            controlling: parse_opt(v.get(1))?,
            scoring: parse_opt(v.get(2))?,
            s0: v.get(3)?.parse().ok()?,
            t0: v.get(4)?.parse().ok()?,
            rate: v.get(5)?.parse().ok()?,
            blocked: v.get(6)? == "1",
        },
    ))
}

/// A target's stored score and its system.
pub fn score_row(d: &Deltas, target: i64) -> Option<(i64, Score)> {
    d.of(KIND)
        .get(&vec![target.to_string()])
        .and_then(|v| parse_row(v))
}

/// Store a target's score.
pub fn put_score(d: &mut Deltas, target: i64, system: i64, s: &Score) {
    d.put(
        KIND,
        vec![target.to_string()],
        vec![
            system.to_string(),
            opt(s.controlling),
            opt(s.scoring),
            s.s0.to_string(),
            s.t0.to_string(),
            s.rate.to_string(),
            if s.blocked { "1" } else { "0" }.to_owned(),
        ],
    );
}

/// Forget a target's score (its hub is gone, or changed hands in a way
/// that voids it). The client is told by the push that follows.
pub fn reset(d: &mut Deltas, target: i64) {
    d.del(KIND, &[target.to_string()]);
}

/// Whether a capture is under way on `hub_id` at `now`: a score above 0.
pub fn capture_active(d: &Deltas, hub_id: i64, now: BlueTime, clock: GameClock) -> bool {
    let _ = clock;
    score_row(d, hub_id).is_some_and(|(_, s)| s.at(now, Spec::HUB.capture_seconds) > EPSILON)
}

/// The defender of `hub_id` in `system`.
fn defender_of(d: &Deltas, system: i64, hub_id: i64) -> Defender {
    match sov::claim(d, system)
        .filter(|c| c.hub_id == hub_id)
        .and_then(|c| c.holder())
    {
        Some(a) => Defender::Owned(a),
        None => Defender::Neutral,
    }
}

fn team(t: Option<i64>) -> PyValue {
    t.map_or(PyValue::None, PyValue::Int)
}

/// `entosis_score` for a tuple and a defender (R-SOV §5.3 **[V]**).
pub fn score_value(s: &Score, defender_team: i64) -> PyValue {
    PyValue::Tuple(vec![
        team(s.controlling),
        team(s.scoring),
        PyValue::Tuple(vec![
            PyValue::Real(s.s0),
            PyValue::Int(s.t0),
            PyValue::Real(s.rate),
            PyValue::Bool(s.blocked),
        ]),
        PyValue::Int(defender_team),
    ])
}

/// **The hub slim's `entosis_score`** (the coordinator line in
/// `sov_hub::extras_of`): `None` for a hub nobody has scored on, else the
/// stored tuple, whose defender is read from the claim as it stands.
pub fn slim_score(d: &Deltas, hub_id: i64) -> PyValue {
    match score_row(d, hub_id) {
        Some((system, s)) => score_value(&s, defender_of(d, system, hub_id).team()),
        None => PyValue::None,
    }
}

/// What a structure row shows beside the ADM: `(campaignState,
/// vulnerabilityState)` **[V]** (`sovDashboard`, R-SOV §3).
///
/// - `campaignState` is `(eventType, defenderAllianceID, startTime,
///   {team: score})` while the hub is reinforced or in its node fight;
/// - `vulnerabilityState` is `(start, end)` of the window (the current
///   one, or the next while invulnerable); an overtime capture keeps the
///   end in the past.
pub fn row_states(d: &Deltas, c: &Claim, now: BlueTime, clock: GameClock) -> (PyValue, PyValue) {
    let campaign = crate::sov_campaign::campaign(d, c.system_id)
        .filter(|_| matches!(c.state, HubState::Reinforced | HubState::NodeFight))
        .map_or(PyValue::None, |k| k.state_value());
    let window = shown(phase(d, c, now, clock)).map_or(PyValue::None, |w| {
        PyValue::Tuple(vec![PyValue::Int(w.start), PyValue::Int(w.end)])
    });
    (campaign, window)
}

/// The times an alliance row shows (`campaignStartTime`, `campaignEventType`,
/// `vulnerableStartTime`, `vulnerableEndTime`): absent is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllianceTimes {
    pub campaign_start: Option<i64>,
    pub campaign_type: Option<i64>,
    pub vulnerable: Option<Window>,
}

/// [`AllianceTimes`] of `c` at `now`.
pub fn alliance_times(d: &Deltas, c: &Claim, now: BlueTime, clock: GameClock) -> AllianceTimes {
    let camp = crate::sov_campaign::campaign(d, c.system_id)
        .filter(|_| matches!(c.state, HubState::Reinforced | HubState::NodeFight));
    AllianceTimes {
        campaign_start: camp.as_ref().map(|k| k.start_time),
        campaign_type: camp.as_ref().map(|k| k.event_type),
        vulnerable: shown(phase(d, c, now, clock)),
    }
}

/// The third element of `GetAllianceSovereigntyStructuresInfo`:
/// `KeyVal(sourceItemID, teamID, score)` per team of each campaign of
/// `claims` **[V]**.
pub fn campaign_scores(d: &Deltas, claims: &[Claim]) -> PyValue {
    PyValue::List(
        claims
            .iter()
            .filter(|c| matches!(c.state, HubState::Reinforced | HubState::NodeFight))
            .filter_map(|c| crate::sov_campaign::campaign(d, c.system_id))
            .flat_map(|k| {
                k.scores
                    .iter()
                    .map(|(team, score)| {
                        crate::pyutil::keyval(vec![
                            ("sourceItemID", PyValue::Int(k.hub_id)),
                            ("teamID", PyValue::Int(*team)),
                            ("score", PyValue::Real(*score)),
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The world slot
// ---------------------------------------------------------------------------

/// What the tick remembers between ticks. Nothing here is state of the
/// game: the fuel already paid is re-derived as "every started cycle" after
/// a restart (the modules do not survive one), and the last phase pushed
/// only decides whether a push is owed.
#[derive(Debug, Default)]
pub struct Entosis {
    /// Module -> cycles already paid for.
    paid: BTreeMap<i64, i64>,
    /// System -> the phase key last seen, for the edge pushes.
    phases: BTreeMap<i64, (u8, i64)>,
    /// When the windows were last scanned.
    scanned: BlueTime,
    /// Scans so far: the beat [`crate::tick_budget::due`] paces the warm and
    /// cold systems by.
    scans: u64,
}

fn make_slot(_: &crate::content::Content) -> Boxed {
    Box::new(Entosis::default())
}

/// The world slot (coordinator: `world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "entosis",
    make: make_slot,
};

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

/// A thing a link may be aimed at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub id: i64,
    pub system_id: i64,
    pub position: [f64; 3],
    pub radius: f64,
    pub spec: Spec,
}

/// The entosis target `id` is: a launched hub, or a command node of a
/// running campaign (SOC-D3b, `command_nodes`).
pub fn target_of(w: &Registry, id: i64) -> Option<Target> {
    let Some(h) = sov_hub::by_id(w.deltas(), id) else {
        return crate::command_nodes::target_of(w, id);
    };
    Some(Target {
        id,
        system_id: h.system_id,
        position: h.position,
        radius: sov_hub::hub_radius(w.content()),
        spec: Spec::HUB,
    })
}

/// Whether `targeting::lock` may lock `id` from `system` (coordinator: one
/// line there, beside the hackable cans).
pub fn is_lockable(w: &Registry, id: i64, system: i64) -> bool {
    target_of(w, id).is_some_and(|t| t.system_id == system)
}

/// Where a locked target is, for the range sweep (coordinator: one branch in
/// `targeting::sweep_out_of_range`).
pub fn lock_position(w: &Registry, id: i64) -> Option<(i64, [f64; 3])> {
    target_of(w, id).map(|t| (t.system_id, t.position))
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// A running Entosis Link, read from the running modules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub run: Running,
    /// The pilot's alliance; `None` is a pilot who left theirs mid-cycle.
    pub alliance: Option<i64>,
}

impl Link {
    /// Whether the warm-up cycle is over **[A]**.
    pub fn scoring(&self, now: BlueTime) -> bool {
        now >= self.run.started + self.run.period
    }
}

/// Every link running against `target`.
pub fn links_on(w: &Registry, target: i64) -> Vec<Link> {
    w.modules()
        .running_against(target)
        .into_iter()
        .filter(|r| r.effect.effect_id == EFFECT_ENTOSIS_LINK)
        .map(|run| Link {
            run,
            alliance: w.character(run.character_id).and_then(|c| c.alliance_id),
        })
        .collect()
}

/// The alliances with a link in scoring phase.
pub fn scorers(links: &[Link], now: BlueTime) -> BTreeSet<i64> {
    links
        .iter()
        .filter(|l| l.scoring(now))
        .filter_map(|l| l.alliance)
        .collect()
}

/// Whether any link runs on `ship_id`, against any hub or command node.
fn ship_is_linking(w: &Registry, ship_id: i64) -> bool {
    sov_hub::all(w.deltas()).iter().any(|h| {
        links_on(w, h.hub_id)
            .iter()
            .any(|l| l.run.ship_id == ship_id)
    }) || crate::command_nodes::linking_a_node(w, ship_id)
}

fn custom_notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// The `error` a stop shows: `(key, args)`.
fn stop_error(text: &str) -> PyValue {
    PyValue::Tuple(vec![
        crate::pyutil::bstr("CustomNotify"),
        PyValue::Dict(crate::pyutil::pypairs(vec![(
            "notify",
            crate::pyutil::bstr(text),
        )])),
    ])
}

// ---------------------------------------------------------------------------
// Movement
// ---------------------------------------------------------------------------

/// **The warp and jump guard** (coordinator: a row in
/// `movement_guard::WARP_GUARDS` and `JUMP_GUARDS`): a ship with a link
/// running cannot warp or jump **[A]** (`siegeModeWarpStatus` 852 is the
/// data's hint **[V-data]**; the retail rule is the same).
pub fn linking(w: &Registry, character_id: i64) -> Option<crate::movement_guard::Refusal> {
    let ship = w.character(character_id)?.ship_id;
    ship_is_linking(w, ship).then(|| crate::movement_guard::Refusal {
        user_error: "CustomNotify",
        args: crate::pyutil::pypairs(vec![(
            "notify",
            crate::pyutil::bstr("You cannot move while your Entosis Link is active."),
        )]),
    })
}

/// **The dock guard** (coordinator: one line in `service/space.rs`'s
/// `CmdDock` arm, beside `crimewatch::refuse_dock`) **[A]**.
pub fn refuse_dock(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    match actor.and_then(|a| linking(w, a)) {
        Some(r) => Err(r.into_error()),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// The value of an attribute of a link: the dogma sheet's modified value,
/// else the type's own.
fn module_value(w: &Registry, ship_id: i64, module_id: i64, type_id: i64, a: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a)
        .filter(|v| *v != 0.0)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, a))
        .or_else(|| module_in(w.content(), type_id).and_then(|t| t.attribute(a)))
}

/// The surface gap from a pilot's ship to a target, the ship carried to now.
fn gap(w: &mut Registry, character: i64, t: &Target) -> Option<f64> {
    w.advance_character(character);
    let here = w.roster().get(character).copied()?;
    let ship_type = w.character(character).map_or(0, |c| c.ship_type_id);
    let hull = crate::destiny::hull::of_in(w.content(), ship_type);
    Some(
        crate::space_state::distance(here.motion.position, t.position)
            - t.radius
            - f64::from(hull.radius),
    )
}

/// A link's fuel: `(type, per cycle)`, or `None` for a module that names
/// none (then it burns none).
fn fuel_of(w: &Registry, module_type: i64) -> Option<(i64, i64)> {
    let ty = crate::typedata::attribute_in(w.content(), module_type, ATTR_FUEL_TYPE)
        .map(|v| v.round() as i64)
        .filter(|t| *t > 0)?;
    let qty = crate::typedata::attribute_in(w.content(), module_type, ATTR_FUEL_QUANTITY)
        .map(|v| v.round() as i64)
        .filter(|q| *q > 0)
        .unwrap_or(1);
    Some((ty, qty))
}

/// The ship's fuel stacks of `fuel_type`: the cargo hold and the fleet
/// hangar **[A]**.
fn fuel_stacks(w: &Registry, ship_id: i64, fuel_type: i64) -> Vec<crate::item::Item> {
    let mut v: Vec<crate::item::Item> = w
        .store()
        .items()
        .iter()
        .filter(|i| {
            i.location_id == ship_id
                && i.type_id == fuel_type
                && sov_hub::LAUNCH_FLAGS.contains(&i.flag_id)
        })
        .cloned()
        .collect();
    v.sort_by_key(|i| (i.quantity, i.item_id));
    v
}

/// What `modules::activate` asks before an Entosis Link starts (the
/// `if effect.effect_id == EFFECT_ENTOSIS_LINK` row, coordinator).
///
/// The client checks only that a target is locked **[V]**, so these are
/// ours, each refusal **[A]** in its key:
///
/// - not an entosis target in this system -> `DeniedActivateTargetModuleDisallowed`;
/// - not locked -> `DeniedActivateTargetNotPresent`;
/// - no alliance, in warp, or a second link on the ship -> a `CustomNotify`;
/// - beyond the link's modified `maxRange` (surface gap) -> `TargetTooFar`;
/// - no fuel for a cycle -> a `CustomNotify`;
/// - the target's own rules ([`check_target`]): reinforced, in a node fight,
///   outside its window, or a defender with nothing to defend.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module: &crate::modules::ModuleType,
    module_id: i64,
    target_id: Option<i64>,
    now: BlueTime,
) -> Result<(), ServiceError> {
    let disallowed = || {
        user_error(
            "DeniedActivateTargetModuleDisallowed",
            vec![("moduleTypeID", type_arg(module.type_id))],
        )
    };
    let here = w.roster().get(character_id).copied();
    let Some(t) = target_id.and_then(|id| target_of(w, id)) else {
        return Err(disallowed());
    };
    if here.is_none_or(|h| h.solar_system_id != t.system_id) {
        return Err(disallowed());
    }
    if !w.locks().targets_of(ship_id).contains(&t.id) {
        return Err(user_error("DeniedActivateTargetNotPresent", vec![]));
    }
    let Some(alliance) = w.character(character_id).and_then(|c| c.alliance_id) else {
        return Err(custom_notify(
            "You must be in a corporation that belongs to an alliance to use an Entosis Link.",
        ));
    };
    if here.is_some_and(|h| h.motion.warp_active()) {
        return Err(custom_notify(
            "You cannot activate an Entosis Link in warp.",
        ));
    }
    if ship_is_linking(w, ship_id) {
        return Err(custom_notify(
            "Only one Entosis Link can run on a ship at a time.",
        ));
    }
    let range = module_value(w, ship_id, module_id, module.type_id, ATTR_MAX_RANGE).unwrap_or(0.0);
    let gap = gap(w, character_id, &t).unwrap_or(f64::INFINITY);
    if gap > range {
        return Err(user_error(
            "TargetTooFar",
            crate::module_range::too_far_pairs(module.type_id, gap, "Sovereignty Hub"),
        ));
    }
    if let Some((fuel, qty)) = fuel_of(w, module.type_id)
        && fuel_stacks(w, ship_id, fuel)
            .iter()
            .map(|i| i.quantity)
            .sum::<i64>()
            < qty
    {
        return Err(custom_notify(
            "The Entosis Link has no fuel: it burns Strontium Clathrates every cycle.",
        ));
    }
    check_target(w, &t, alliance, now)
}

/// The target's rules for a link of `alliance` at `now`.
pub fn check_target(
    w: &Registry,
    t: &Target,
    alliance: i64,
    now: BlueTime,
) -> Result<(), ServiceError> {
    // A command node (SOC-D3b) has its own rules: its campaign's.
    match crate::command_nodes::gate(w, t) {
        crate::command_nodes::Gate::NotANode => {}
        crate::command_nodes::Gate::Allowed => return Ok(()),
        crate::command_nodes::Gate::Refused(text) => return Err(custom_notify(&text)),
    }
    let clock = crate::game_clock::of(w);
    let Some(c) = sov::claim(w.deltas(), t.system_id).filter(|c| c.hub_id == t.id) else {
        return Err(custom_notify("That structure cannot be entosised."));
    };
    let live = capture_active(w.deltas(), t.id, now, clock);
    match phase(w.deltas(), &c, now, clock) {
        Phase::Reinforced(until) => {
            return Err(custom_notify(&format!(
                "This structure is reinforced until {}: its command nodes appear then.",
                crate::live_status::iso_utc(until)
            )));
        }
        Phase::NodeFight => {
            return Err(custom_notify(
                "This structure is in its node fight: capture its command nodes.",
            ));
        }
        Phase::Invulnerable(next) => {
            return Err(custom_notify(&format!(
                "This structure is invulnerable. Its vulnerability window opens at {}.",
                crate::live_status::iso_utc(next.start)
            )));
        }
        Phase::Open | Phase::Vulnerable(_) | Phase::Overtime(_) => {}
    }
    if c.holder() == Some(alliance) && !live {
        return Err(custom_notify(
            "Your alliance holds this structure and nobody is attacking it.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The tick
// ---------------------------------------------------------------------------

/// Stop every link on `hub_id` with `text`.
pub(crate) fn stop_links(w: &mut Registry, hub_id: i64, now: BlueTime, text: &str) {
    for l in links_on(w, hub_id) {
        crate::modules::stop(w, l.run.module_id, now, Some(stop_error(text)));
        w.ext_mut::<Entosis>().paid.remove(&l.run.module_id);
    }
}

/// Pay the fuel for every cycle started since the last tick, and stop what
/// cannot go on: no fuel, or out of range. Returns the links left.
pub(crate) fn upkeep(w: &mut Registry, links: Vec<Link>, t: &Target, now: BlueTime) -> Vec<Link> {
    let mut kept = Vec::new();
    for l in links {
        let r = l.run;
        let paid = w
            .ext::<Entosis>()
            .paid
            .get(&r.module_id)
            .copied()
            .unwrap_or(0);
        let owed = r.cycles - paid;
        if owed > 0 {
            let burned = fuel_of(w, r.type_id).is_none_or(|(fuel, qty)| {
                burn_fuel(w, r.character_id, r.ship_id, fuel, qty.saturating_mul(owed))
            });
            if !burned {
                crate::modules::stop(
                    w,
                    r.module_id,
                    now,
                    Some(stop_error("The Entosis Link ran out of fuel.")),
                );
                w.ext_mut::<Entosis>().paid.remove(&r.module_id);
                continue;
            }
            w.ext_mut::<Entosis>().paid.insert(r.module_id, r.cycles);
        }
        let range = module_value(w, r.ship_id, r.module_id, r.type_id, ATTR_MAX_RANGE)
            .unwrap_or(0.0)
            + RANGE_SLACK;
        if gap(w, r.character_id, t).is_some_and(|g| g > range) {
            crate::modules::stop(
                w,
                r.module_id,
                now,
                Some(stop_error(
                    "The Entosis Link lost its target: out of range.",
                )),
            );
            w.ext_mut::<Entosis>().paid.remove(&r.module_id);
            continue;
        }
        kept.push(l);
    }
    kept
}

/// Take `n` of `fuel_type` out of a ship's hold: all of it, or, when the
/// hold carries fewer, what there is. `true` only if all `n` were there.
fn burn_fuel(w: &mut Registry, character: i64, ship_id: i64, fuel_type: i64, n: i64) -> bool {
    let stacks: Vec<crate::item::Item> = fuel_stacks(w, ship_id, fuel_type)
        .into_iter()
        .filter(|i| i.owner_id == character)
        .collect();
    let have = stacks.iter().map(|i| i.quantity).sum::<i64>();
    let paid = have >= n;
    let mut left = n.min(have);
    let mut edits = Vec::new();
    for s in &stacks {
        if left <= 0 {
            break;
        }
        let take = left.min(s.quantity);
        edits.push(if take == s.quantity {
            crate::item::Edit::Destroy { item_id: s.item_id }
        } else {
            crate::item::Edit::Consume {
                item_id: s.item_id,
                quantity: take,
            }
        });
        left -= take;
    }
    if edits.is_empty() {
        return paid;
    }
    match w.apply_items(&edits) {
        Ok(_) => paid,
        Err(e) => {
            crate::errln!("[entosis] fuel for {character} not burned: {e}");
            false
        }
    }
}

/// **The world clock's stage** (coordinator: `tick::STAGES`, after
/// `signal_tracker`, before `sov_capital`): pay the links' fuel, settle every
/// hub's score, push the window edges, begin due campaigns. Everything it
/// pushes leaves on the clock.
pub fn tick(w: &mut Registry, _now: Instant, ft: BlueTime) {
    if w.deltas().of(sov_hub::KIND).is_empty() {
        return;
    }
    sov_hub::on_the_clock(w, |w| {
        // TICK-1: only a hub with a running link or a stored score has
        // anything to settle (`settle_hub` returns at once for the rest),
        // so those are the only ones read. Same hubs, same order.
        for h in hubs_with_work(w) {
            settle_hub(w, &h, ft);
        }
        scan_windows(w, ft);
        crate::sov_campaign::tick(w, ft);
        // A module that stopped no longer owes or is owed.
        if !w.ext::<Entosis>().paid.is_empty() {
            let alive: BTreeSet<i64> = hub_links(w)
                .into_iter()
                .map(|(_, run)| run.module_id)
                .chain(crate::command_nodes::link_modules(w))
                .collect();
            w.ext_mut::<Entosis>()
                .paid
                .retain(|module, _| alive.contains(module));
        }
    });
}

/// Every running Entosis Link aimed at a sovereignty hub, as `(hub, run)`:
/// one pass over the running modules, not one per hub.
fn hub_links(w: &Registry) -> Vec<(sov_hub::Hub, Running)> {
    w.modules()
        .running_all()
        .into_iter()
        .filter(|r| r.effect.effect_id == EFFECT_ENTOSIS_LINK)
        .filter_map(|r| Some((sov_hub::by_id(w.deltas(), r.target_id?)?, r)))
        .collect()
}

/// **The hubs [`settle_hub`] can do something for**: those with a link
/// running against them or a stored score, in the order `sov_hub::all` lists
/// them (by system id as text). A hub in neither set returns from
/// `settle_hub` before it reads anything else, so leaving it out changes
/// nothing, and a hub gains a score row or a link only through the calls
/// that make one, after which it is in the set on the next tick.
fn hubs_with_work(w: &Registry) -> Vec<sov_hub::Hub> {
    let d = w.deltas();
    let mut out: BTreeMap<Vec<String>, sov_hub::Hub> = BTreeMap::new();
    for (key, row) in d.of(KIND) {
        let (Some(target), Some(system)) = (
            key.first().and_then(|t| t.parse::<i64>().ok()),
            row.first().and_then(|s| s.parse::<i64>().ok()),
        ) else {
            continue;
        };
        if let Some(h) = sov_hub::hub(d, system).filter(|h| h.hub_id == target) {
            out.insert(vec![h.system_id.to_string()], h);
        }
    }
    for (h, _) in hub_links(w) {
        out.insert(vec![h.system_id.to_string()], h);
    }
    out.into_values().collect()
}

/// Settle one hub: its links, its score, and what the score completed.
fn settle_hub(w: &mut Registry, h: &sov_hub::Hub, now: BlueTime) {
    let clock = crate::game_clock::of(w);
    let Some(c) = sov::claim(w.deltas(), h.system_id).filter(|c| c.hub_id == h.hub_id) else {
        return;
    };
    let stored = score_row(w.deltas(), h.hub_id).map(|(_, s)| s);
    let links = links_on(w, h.hub_id);
    if links.is_empty() && stored.is_none() {
        return;
    }
    let ph = phase(w.deltas(), &c, now, clock);
    if matches!(ph, Phase::Reinforced(_) | Phase::NodeFight) {
        // A link left running through a reinforcement.
        if !links.is_empty() {
            stop_links(w, h.hub_id, now, "This structure is reinforced.");
        }
        return;
    }
    let Some(t) = target_of(w, h.hub_id) else {
        return;
    };
    let mut links = upkeep(w, links, &t, now);
    let s_now = stored.map_or(0.0, |s| s.at(now, t.spec.capture_seconds));
    if !ph.attackable() && !matches!(ph, Phase::Overtime(_)) && s_now <= EPSILON {
        // The window closed on a link that never scored.
        if !links.is_empty() {
            stop_links(
                w,
                h.hub_id,
                now,
                "The vulnerability window closed before this link scored.",
            );
            links.clear();
        }
    }
    let scoring = scorers(&links, now);
    let defender = match c.holder() {
        Some(a) => Defender::Owned(a),
        None => Defender::Neutral,
    };
    let out = step(stored, now, t.spec, defender, &scoring, clock);
    match out.done {
        Some(Done::Full { team }) => {
            // Reset first: the completion's pushes carry the idle tuple.
            put_score(w.deltas_mut(), h.hub_id, h.system_id, &out.score);
            complete(w, h, &c, team, &scoring, now);
        }
        None if out.changed => {
            if defender != Defender::Neutral
                && stored.is_none_or(|p| p.controlling.is_none())
                && out.score.controlling.is_some()
                && out.score.base_is_attack()
            {
                notify_capture_started(w, &c);
            }
            put_score(w.deltas_mut(), h.hub_id, h.system_id, &out.score);
            sov_hub::claim_changed(w, h.system_id);
        }
        None => {}
    }
}

impl Score {
    /// Whether the tuple is an attack on a held hub moving up.
    fn base_is_attack(&self) -> bool {
        self.rate > 0.0 && !self.blocked
    }
}

/// A score reached 1.0.
fn complete(
    w: &mut Registry,
    h: &sov_hub::Hub,
    c: &Claim,
    team: Option<i64>,
    scoring: &BTreeSet<i64>,
    now: BlueTime,
) {
    match c.holder() {
        None => {
            // A neutral hub: the alliance whose team held the bar claims it.
            let Some(alliance) = team.filter(|t| *t > 0) else {
                sov_hub::claim_changed(w, h.system_id);
                return;
            };
            let exec = crate::alliance::Alliances::load(w.deltas())
                .get(alliance)
                .and_then(|a| a.executor_corp_id);
            let Some(exec) = exec else {
                crate::errln!("[entosis] alliance {alliance} has no executor corp: no claim");
                sov_hub::claim_changed(w, h.system_id);
                return;
            };
            stop_links(w, h.hub_id, now, "The hub is claimed.");
            if let Err(e) = claim_neutral_at(w, h.system_id, (alliance, exec), now) {
                crate::errln!("[entosis] claim of {} failed: {e}", h.system_id);
            }
        }
        Some(holder) => {
            let attackers: Vec<i64> = scoring.iter().copied().filter(|a| *a != holder).collect();
            if let Err(e) = reinforce_at(w, h.system_id, attackers, false, now) {
                crate::errln!("[entosis] reinforcing {} failed: {e}", h.system_id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Completions
// ---------------------------------------------------------------------------

/// Who hears about a hub: a corporation's CEO and every director or
/// personnel manager **[A]** (the same handlers the corp notifications
/// reach, `notifications::corp_events::corp_managers`). Falsified by a
/// retail sov notice reaching ordinary members.
fn managers(w: &Registry, corp: i64) -> Vec<i64> {
    crate::service::notifications::corp_events::corp_managers(w, corp)
}

/// The managers of an alliance's executor corp and of `also`.
pub(crate) fn alliance_managers(w: &Registry, alliance: i64, also: Option<i64>) -> Vec<i64> {
    let mut corps: Vec<i64> = crate::alliance::Alliances::load(w.deltas())
        .get(alliance)
        .and_then(|a| a.executor_corp_id)
        .into_iter()
        .collect();
    corps.extend(also);
    corps.sort_unstable();
    corps.dedup();
    corps.into_iter().flat_map(|c| managers(w, c)).collect()
}

/// `EntosisCaptureStarted` (147) `{structureTypeID, solarSystemID}` to the
/// holder's managers **[V]**.
fn notify_capture_started(w: &mut Registry, c: &Claim) {
    let Some(holder) = c.holder() else { return };
    for to in alliance_managers(w, holder, Some(c.corporation_id)) {
        notif::notify(
            w,
            to,
            kind::ENTOSIS_CAPTURE_STARTED,
            data([
                ("structureTypeID", Value::Int(sov::TYPE_INFRASTRUCTURE_HUB)),
                ("solarSystemID", Value::Int(c.system_id)),
            ]),
        );
    }
}

/// **A neutral hub claimed by `owner`** (`(allianceID, executorCorpID)`):
/// the one function the entosis completion and `/sov capture` both call
/// (R-SOV §6.2 step 4). The alliance's executor corp holds it,
/// `claim_time = now`, D1's three pushes, a `sovact` row, the hub's slim
/// (with an idle score) and `SovAllClaimAquiredMsg` to the new holder.
/// Refused for a system with no hub, or whose hub already has a holder
/// (an attack on a held hub reinforces it).
pub fn claim_neutral(
    reg: &mut Registry,
    system: i64,
    owner: (i64, i64),
) -> Result<sov::Change, String> {
    claim_neutral_at(reg, system, owner, crate::pyutil::filetime_now())
}

/// [`claim_neutral`] at `now` (the tick's time, which the claim is stamped
/// with).
pub fn claim_neutral_at(
    reg: &mut Registry,
    system: i64,
    owner: (i64, i64),
    now: BlueTime,
) -> Result<sov::Change, String> {
    let h = sov_hub::hub(reg.deltas(), system)
        .ok_or_else(|| format!("system {system} has no launched Sovereignty Hub"))?;
    let c = sov::claim(reg.deltas(), system)
        .ok_or_else(|| format!("system {system} has no hub row"))?;
    if let Some(holder) = c.holder() {
        return Err(format!(
            "hub {} is held by alliance {holder}; /sov set moves a held claim",
            h.hub_id
        ));
    }
    let change = sov::set_claim(reg.deltas_mut(), system, Some(owner), now)
        .ok_or_else(|| "nothing changed".to_owned())?;
    // The bar goes back to idle, and the clients are told so.
    put_score(reg.deltas_mut(), h.hub_id, system, &Score::idle(now));
    sov::announce(reg, &change);
    sov_hub::claim_changed(reg, system);
    for to in managers(reg, owner.1) {
        notif::notify_from(
            reg,
            to,
            kind::ALLIANCE_CLAIM_ACQUIRED,
            owner.1,
            data([
                ("corpID", Value::Int(owner.1)),
                ("allianceID", Value::Int(owner.0)),
                ("solarSystemID", Value::Int(system)),
            ]),
        );
    }
    crate::errln!(
        "[sov] hub {} in {system} captured by alliance {} (corp {})",
        h.hub_id,
        owner.0,
        owner.1
    );
    Ok(change)
}

/// **A held hub's attack completed**: the hub goes `reinforced` and a
/// campaign is scheduled (`sov_campaign::schedule`); the nodes and the fight
/// are D3b's. `immediately` (the GM line) starts the campaign now instead
/// of at the next window. The defender's managers are told
/// (`SovStructureReinforced`, 160).
pub fn reinforce(
    reg: &mut Registry,
    system: i64,
    attackers: Vec<i64>,
    immediately: bool,
) -> Result<crate::sov_campaign::Campaign, String> {
    reinforce_at(
        reg,
        system,
        attackers,
        immediately,
        crate::pyutil::filetime_now(),
    )
}

/// [`reinforce`] at `now`.
pub fn reinforce_at(
    reg: &mut Registry,
    system: i64,
    attackers: Vec<i64>,
    immediately: bool,
    now: BlueTime,
) -> Result<crate::sov_campaign::Campaign, String> {
    let h = sov_hub::hub(reg.deltas(), system)
        .ok_or_else(|| format!("system {system} has no launched Sovereignty Hub"))?;
    let c = sov::claim(reg.deltas(), system)
        .filter(|c| c.hub_id == h.hub_id)
        .ok_or_else(|| format!("system {system} has no hub row"))?;
    let holder = c
        .holder()
        .ok_or_else(|| format!("hub {} has no holder to reinforce against", h.hub_id))?;
    if matches!(c.state, HubState::Reinforced | HubState::NodeFight) {
        return Err(format!("hub {} is already reinforced", h.hub_id));
    }
    let clock = crate::game_clock::of(reg);
    let camp = crate::sov_campaign::schedule(reg.deltas(), &c, attackers, now, clock, immediately);
    crate::sov_campaign::put(reg.deltas_mut(), &camp);
    sov::put(
        reg.deltas_mut(),
        &Claim {
            state: HubState::Reinforced,
            state_until: Some(camp.start_time),
            ..c.clone()
        },
    );
    put_score(reg.deltas_mut(), h.hub_id, system, &Score::idle(now));
    stop_links(reg, h.hub_id, now, "This structure is reinforced.");
    sov_hub::claim_changed(reg, system);
    let mut alliances = vec![holder];
    alliances.extend(camp.attackers.iter().copied());
    sov::push_structures(reg, system, &alliances, Some(changed(h.hub_id)));
    for to in alliance_managers(reg, holder, Some(c.corporation_id)) {
        notif::notify(
            reg,
            to,
            kind::STRUCTURE_REINFORCED,
            data([
                ("campaignEventType", Value::Int(camp.event_type)),
                ("solarSystemID", Value::Int(system)),
                ("decloakTime", Value::Int(camp.start_time)),
            ]),
        );
    }
    crate::errln!(
        "[sov] hub {} in {system} reinforced; campaign starts {}",
        h.hub_id,
        crate::live_status::iso_utc(camp.start_time)
    );
    Ok(camp)
}

/// `{hubID: [STRUCTURES_UPDATED]}`: the `changes` of a structure push.
pub fn changed(hub_id: i64) -> PyValue {
    PyValue::Dict(vec![(
        PyValue::List(vec![PyValue::Int(STRUCTURES_UPDATED)]),
        PyValue::Int(hub_id),
    )])
}

/// A hub is gone (lapsed, destroyed): stop its links and forget its score
/// and campaign. Coordinator: one line in `sov_hub::lapse`.
pub fn forget(reg: &mut Registry, system: i64, hub_id: i64) {
    let now = crate::pyutil::filetime_now();
    stop_links(reg, hub_id, now, "The structure is gone.");
    reset(reg.deltas_mut(), hub_id);
    crate::sov_campaign::drop_campaign(reg.deltas_mut(), system);
    // SOC-D3b: its command nodes go with it.
    crate::command_nodes::despawn_hub(reg, hub_id);
}

// ---------------------------------------------------------------------------
// Windows, pushed
// ---------------------------------------------------------------------------

/// Push `OnSolarSystemSovStructuresUpdated` when a hub's phase changed
/// since the last scan: a window opening, closing, going into overtime.
/// The first sight of a hub (a restart) pushes nothing.
fn scan_windows(w: &mut Registry, now: BlueTime) {
    if now - w.ext::<Entosis>().scanned < WINDOW_SCAN {
        return;
    }
    let scans = {
        let e = w.ext_mut::<Entosis>();
        e.scanned = now;
        e.scans = e.scans.wrapping_add(1);
        e.scans
    };
    let clock = crate::game_clock::of(w);
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    for c in sov::all(w.deltas()) {
        if !c.is_claimed() {
            continue;
        }
        seen.insert(c.system_id);
        // TICK-1: a system nobody is flying in is looked at on its class's
        // beat (hot: every scan). A phase is a function of the stored
        // window and `now`, so a later look finds the same phase and the
        // edge, if any, is pushed then; a first sight is never skipped.
        let known = w.ext::<Entosis>().phases.contains_key(&c.system_id);
        let class = crate::tick_budget::class_of(w, c.system_id);
        if known && !crate::tick_budget::due(class, scans, c.system_id) {
            continue;
        }
        let key = phase(w.deltas(), &c, now, clock).key();
        let before = w.ext_mut::<Entosis>().phases.insert(c.system_id, key);
        if before.is_some_and(|b| b != key) {
            edges.push(c);
        }
    }
    w.ext_mut::<Entosis>()
        .phases
        .retain(|system, _| seen.contains(system));
    for c in edges {
        let alliances: Vec<i64> = c.alliance_id.into_iter().collect();
        sov::push_structures(w, c.system_id, &alliances, Some(changed(c.hub_id)));
    }
}

// ---------------------------------------------------------------------------
// /sov entosis | window | reinforce | resolve
// ---------------------------------------------------------------------------

/// The `/sov` words this module answers (`service/sov.rs` hands them over).
pub const SLASH_WORDS: &[&str] = &[
    "entosis",
    "window",
    "reinforce",
    "resolve",
    "prime",
    "nodes",
];

/// The usage lines for `service/sov.rs`'s `USAGE`.
pub const USAGE: &str = "/sov entosis [systemID|here] | /sov window [systemID|here] | /sov reinforce <systemID|here> [now] | /sov resolve <systemID|here> attackers|defenders | /sov prime <allianceID|shortName> <hour> | /sov nodes [systemID|here] | /sov nodes spawn [systemID|here] | /sov nodes capture <nodeID> attackers|defenders";

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

fn describe_phase(p: Phase) -> String {
    let iso = crate::live_status::iso_utc;
    match p {
        Phase::Open => "no window (neutral hub)".to_owned(),
        Phase::Vulnerable(w) => format!("vulnerable {} to {}", iso(w.start), iso(w.end)),
        Phase::Overtime(w) => format!("overtime (window ended {})", iso(w.end)),
        Phase::Invulnerable(w) => format!("invulnerable until {}", iso(w.start)),
        Phase::Reinforced(t) => format!("reinforced until {}", iso(t)),
        Phase::NodeFight => "node fight".to_owned(),
    }
}

/// The status line of `system`'s hub: phase, score, links.
pub fn status_text(reg: &Registry, system: i64) -> String {
    let now = crate::pyutil::filetime_now();
    let clock = crate::game_clock::of(reg);
    let Some(c) = sov::claim(reg.deltas(), system) else {
        return format!("system {system}: no hub");
    };
    let score = match score_row(reg.deltas(), c.hub_id) {
        Some((_, s)) => format!(
            "score {:.3} (controlling {:?}, scoring {:?}, rate {:+.4}/s{})",
            s.at(now, Spec::HUB.capture_seconds),
            s.controlling,
            s.scoring,
            s.rate,
            if s.blocked { ", BLOCKED" } else { "" }
        ),
        None => "never scored".to_owned(),
    };
    let links = links_on(reg, c.hub_id);
    let scoring = links.iter().filter(|l| l.scoring(now)).count();
    format!(
        "hub {} in {system} ({}): {}; {score}; {} links ({scoring} scoring)",
        c.hub_id,
        c.state.as_str(),
        describe_phase(phase(reg.deltas(), &c, now, clock)),
        links.len()
    )
}

/// `/sov entosis`, `/sov window`, `/sov reinforce`, `/sov resolve`.
pub fn slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    let usage = || format!("usage: {USAGE}");
    match args {
        ["entosis" | "window"] | ["entosis" | "window", _] => {
            match parse_system(reg, actor, args.get(1)) {
                Ok(system) => status_text(reg, system),
                Err(e) => format!("{} -- {e}", usage()),
            }
        }
        ["reinforce", system, rest @ ..] => {
            let now = rest.first() == Some(&"now");
            match parse_system(reg, actor, Some(system)) {
                Ok(system) => match reinforce(reg, system, Vec::new(), now) {
                    Ok(k) => format!(
                        "reinforced: campaign starts {}",
                        crate::live_status::iso_utc(k.start_time)
                    ),
                    Err(e) => e,
                },
                Err(e) => format!("{} -- {e}", usage()),
            }
        }
        // Set an alliance's prime hour at once (the window dialog waits 48
        // game hours): for tests and human checks.
        ["prime", who, hour] => {
            let found = {
                let st = crate::alliance::Alliances::load(reg.deltas());
                match who.parse::<i64>() {
                    Ok(id) => st.get(id).map(|a| a.alliance_id),
                    Err(_) => st
                        .all()
                        .find(|a| !a.deleted && a.short_name.eq_ignore_ascii_case(who))
                        .map(|a| a.alliance_id),
                }
            };
            match (found, hour.parse::<i64>()) {
                (Some(a), Ok(hour)) => {
                    let now = crate::pyutil::filetime_now();
                    match sov::set_prime_hour(reg.deltas_mut(), a, hour, now, 0) {
                        Ok(_) => format!("alliance {a}: prime hour {hour} from now"),
                        Err(why) => format!("{why} (0 to 23)"),
                    }
                }
                (None, _) => format!("no alliance '{who}'"),
                (_, Err(_)) => usage(),
            }
        }
        // SOC-D3b: the node fight's status and its GM levers.
        ["nodes", rest @ ..] => crate::command_nodes::slash(reg, actor, rest),
        ["resolve", system, who] => {
            let outcome = match *who {
                "attackers" => crate::sov_campaign::Outcome::AttackersWin,
                "defenders" => crate::sov_campaign::Outcome::DefendersWin,
                _ => return usage(),
            };
            match parse_system(reg, actor, Some(system)) {
                Ok(system) => match crate::sov_campaign::resolve(reg, system, outcome) {
                    Ok(()) => format!("campaign in {system} resolved for the {who}"),
                    Err(e) => e,
                },
                Err(e) => format!("{} -- {e}", usage()),
            }
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests;
