//! Sovereignty's development indices, the activity defense multiplier (ADM)
//! and the alliance capital (SOC-D4; lane note `docs/lanes/sov-d4.md`, spec
//! R-SOV §6.6).
//!
//! **Three indices per claimed system**, as the client draws them:
//!
//! - **military** (attribute [`ATTR_DEV_INDEX_MILITARY`]): points from NPCs
//!   killed in the system ([`on_npc_kill`], a `kill_hooks::NPC` row);
//! - **industrial** ([`ATTR_DEV_INDEX_INDUSTRIAL`]): points from ore, ice and
//!   gas cut there ([`on_mined`], one line in `mining::cut`);
//! - **strategic** ([`ATTR_DEV_INDEX_SOVEREIGNTY`]): the claim's age, read
//!   from the `sov` row's claim time; nothing is stored for it.
//!
//! Military and industrial points fall by [`DECAY_PER_DAY`] at every
//! rollover ([`on_rollover`]), which also decides each index's
//! "increasing" arrow and tells the pilots in each claimed system to
//! re-read (`OnSolarSystemDevIndexChanged`).
//!
//! **ADM** is the client's own formula ([`adm`], `entosis.occupancyCalculator`
//! **[V]**): the three levels' bonuses, plus [`CAPITAL_BONUS`] in the
//! alliance's capital, clamped to `1..=6`. The vulnerability window D3a
//! opens is [`vulnerability_window`]`(adm)` long.
//!
//! **The capital**: `allianceRegistry.SetCapitalSystem` names a held system,
//! which becomes the capital [`CAPITAL_CHANGE_DELAY`] later (a game span,
//! scaled by `--time-scale`); `CancelCapitalSystemTransition` drops the
//! pending one. An alliance holding exactly one system has it as capital at
//! once, and a capital the alliance no longer holds is no capital
//! ([`resolve_capital`]). A change pushes `OnCapitalSystemChanged`.
//!
//! **The world's deltas are the only copy.** Two kinds:
//!
//! ```text
//! sovidx  <systemID>   -> military industrial militaryToday industrialToday militaryUp industrialUp
//! sovcap  <allianceID> -> currentCapital newCapital validAfter
//! ```
//!
//! `-` is an absent value; the two `Up` fields are `1` or `0`. A `sovidx`
//! row whose points are all spent is dropped. A `sovcap` row holds the
//! capital **last announced**: reads resolve it at "now" ([`capital`]), and
//! [`settle_capital`] writes the resolution back and pushes the change.

use crate::damage::BlueTime;
use crate::delta::Deltas;
use crate::game_clock::GameClock;
use crate::pyutil::keyval;
use crate::service::{Registry, ServiceError};
use crate::sov::{self, Claim, SovPush};
use eve_marshal::PyValue;
use std::time::Instant;

/// The delta kinds this module owns (`delta_kinds::KINDS`).
pub const KIND: &str = "sovidx";
pub const KIND_CAPITAL: &str = "sovcap";

/// `const.attributeDevIndexMilitary`, `...Industrial` and
/// `...Sovereignty` **[V]** (R-SOV §2): the keys of
/// `GetDevelopmentIndicesForSystem`'s dict.
pub const ATTR_DEV_INDEX_MILITARY: i64 = 1_583;
pub const ATTR_DEV_INDEX_INDUSTRIAL: i64 = 1_584;
pub const ATTR_DEV_INDEX_SOVEREIGNTY: i64 = 1_615;

/// FILETIME ticks per second, hour and day.
const SECOND: i64 = 10_000_000;
const HOUR: i64 = crate::seed::HOUR;
const DAY: i64 = crate::seed::DAY;

/// `devIndexUtil.devIndexDecayRate` and `devIndexDecayTime` **[V]**. Their
/// product is the width of one military or industrial level
/// ([`LEVEL_WIDTH`]): `GetDevIndexLevels` stacks six bands of it, 0 to 5.
pub const DEV_INDEX_DECAY_RATE: i64 = 100_000;
pub const DEV_INDEX_DECAY_TIME: i64 = 96;
/// Points per military or industrial level **[V]** (`devIndexUtil`).
pub const LEVEL_WIDTH: i64 = DEV_INDEX_DECAY_RATE * DEV_INDEX_DECAY_TIME;
/// The top level of every index **[V]** (`totalLevels = 5`; `GetIndexLevel`
/// answers 5 past the last band).
pub const MAX_LEVEL: usize = 5;
/// **[I]** the most points an index holds: the end of the client's last
/// band (`GetDevIndexLevels`' level-5 `maxLevel`), past which
/// `sovSvc.GetIndexLevel` answers level 5 with no remainder. A cap so a busy
/// week cannot bank months of decay. Falsified by a retail index reading
/// above it.
pub const TOP_POINTS: i64 = (MAX_LEVEL as i64 + 1) * LEVEL_WIDTH;
/// `devIndexUtil.timeIndexLevels` **[V]**: the claim days that reach the
/// strategic levels 1 to 5.
pub const STRATEGIC_LEVEL_DAYS: [i64; MAX_LEVEL] = [7, 21, 35, 65, 100];
/// `sovSvc.CLAIM_DAYS_TO_SECONDS` **[V]**: `GetAllDevelopmentIndices`'
/// `claimedFor` is in days and the client multiplies by this.
pub const CLAIM_DAYS_TO_SECONDS: i64 = 86_400;

/// `entosis.occupancyCalculator` **[V]**: the ADM's parts. Index `L` of each
/// table is level `L`'s bonus (level 0 has none: the client's
/// `.get(level, 0.0)`).
pub const BASE_BONUS: f64 = 1.0;
pub const CAPITAL_BONUS: f64 = 2.0;
pub const MIN_OCCUPANCY_BONUS: f64 = 1.0;
pub const MAX_OCCUPANCY_BONUS: f64 = 6.0;
pub const MILITARY_INDEX_TO_BONUS: [f64; MAX_LEVEL + 1] = [0.0, 0.6, 1.2, 1.7, 2.1, 2.5];
pub const INDUSTRIAL_INDEX_TO_BONUS: [f64; MAX_LEVEL + 1] = [0.0, 0.6, 1.2, 1.7, 2.1, 2.5];
pub const STRATEGIC_INDEX_TO_BONUS: [f64; MAX_LEVEL + 1] = [0.0, 0.4, 0.6, 0.8, 0.9, 1.0];

/// **[A]** military points per NPC killed in a claimed system (R-SOV §6.6):
/// level 1 after 160 kills, level 5 after 800 with no decay. Falsified by a
/// 7-day society soak in which no ratted system reaches level 2, or every
/// one reaches 5.
pub const MILITARY_POINTS_PER_NPC: i64 = 60_000;
/// **[A]** industrial points per cubic metre of ore, ice or gas cut in a
/// claimed system (R-SOV §6.6). Falsified as [`MILITARY_POINTS_PER_NPC`].
pub const INDUSTRIAL_POINTS_PER_M3: f64 = 1_000.0;
/// **[I]** what an index loses per game hour, from `devIndexDecayRate`: the
/// client's level width is 96 hours of it, so an idle level fades in four
/// days. Falsified by a retail index that fades faster or slower.
pub const DECAY_PER_HOUR: i64 = DEV_INDEX_DECAY_RATE;
/// [`DECAY_PER_HOUR`] over one game day: what each rollover takes.
pub const DECAY_PER_DAY: i64 = 24 * DECAY_PER_HOUR;

/// **[V2]** how long a new capital waits (EvEJS's
/// `CAPITAL_SYSTEM_CHANGE_DELAY_MS`, a fact; the client has no constant).
/// A game span: the caller scales it with the world clock. Falsified by a
/// retail capital that moves sooner or later.
pub const CAPITAL_CHANGE_DELAY: i64 = 7 * DAY;

/// **[V2]** the vulnerability window at ADM 1 (EvEJS's model, the 2018
/// retail rule; the client has no constant). D3a opens a window of
/// [`vulnerability_window`]`(adm)`. **The owner may change this** (R-SOV
/// §12 U-SOV-3): falsified by an owner who wants a fixed window.
pub const VULNERABILITY_BASE: i64 = 18 * HOUR;
/// **[V2]** the shortest window, whatever the ADM.
pub const VULNERABILITY_MIN: i64 = HOUR;

// ---------------------------------------------------------------------------
// Levels and ADM (pure)
// ---------------------------------------------------------------------------

/// A military or industrial level from its points, as
/// `sovSvc.GetIndexLevel` reads them **[V]**: one level per
/// [`LEVEL_WIDTH`], 5 from five widths on.
pub fn level(points: i64) -> usize {
    usize::try_from(points.max(0) / LEVEL_WIDTH)
        .unwrap_or(MAX_LEVEL)
        .min(MAX_LEVEL)
}

/// The strategic level of a claim `seconds` old: the highest of
/// [`STRATEGIC_LEVEL_DAYS`] it has reached **[V]** (`GetIndexLevel` over
/// `GetTimeIndexLevels`).
pub fn strategic_level(seconds: i64) -> usize {
    STRATEGIC_LEVEL_DAYS
        .iter()
        .take_while(|days| seconds >= **days * CLAIM_DAYS_TO_SECONDS)
        .count()
}

/// The ADM: `1 + military + industrial + strategic (+ 2 in the capital)`,
/// clamped to `1..=6` **[V]** (`occupancyCalculator.GetOccupancyMultiplier`
/// returns the inverse; every reader takes `1 /` it back, and the
/// dashboard's `defenseMultiplier` is this number).
pub fn adm(military: usize, industrial: usize, strategic: usize, capital: bool) -> f64 {
    let bonus = |table: &[f64; MAX_LEVEL + 1], l: usize| table.get(l).copied().unwrap_or(0.0);
    let total = BASE_BONUS
        + bonus(&MILITARY_INDEX_TO_BONUS, military)
        + bonus(&INDUSTRIAL_INDEX_TO_BONUS, industrial)
        + bonus(&STRATEGIC_INDEX_TO_BONUS, strategic)
        + if capital { CAPITAL_BONUS } else { 0.0 };
    total.clamp(MIN_OCCUPANCY_BONUS, MAX_OCCUPANCY_BONUS)
}

/// The vulnerability window for a hub at `adm`: [`VULNERABILITY_BASE`] /
/// ADM, never under [`VULNERABILITY_MIN`]. A game span in FILETIME ticks,
/// for D3a.
pub fn vulnerability_window(adm: f64) -> i64 {
    let adm = if adm.is_finite() {
        adm.max(MIN_OCCUPANCY_BONUS)
    } else {
        MIN_OCCUPANCY_BONUS
    };
    ((VULNERABILITY_BASE as f64 / adm) as i64).max(VULNERABILITY_MIN)
}

/// How long a claim stamped at `claim_time` has been held at `now`, in game
/// seconds (the world clock scales the wall span: lane CLOCK's rule for a
/// span derived from a stored start).
pub fn claimed_seconds(claim_time: Option<i64>, now: i64, clock: GameClock) -> i64 {
    claim_time.map_or(0, |t| clock.elapsed(now.saturating_sub(t).max(0)) / SECOND)
}

// ---------------------------------------------------------------------------
// The index ledger
// ---------------------------------------------------------------------------

/// One system's military and industrial points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Indices {
    pub military: i64,
    pub industrial: i64,
    /// Points gained since the last rollover.
    pub military_today: i64,
    pub industrial_today: i64,
    /// The last rollover's verdict: the day's gain beat its decay.
    pub military_up: bool,
    pub industrial_up: bool,
}

/// Which stored index a gain goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Military,
    Industrial,
}

fn key(id: i64) -> Vec<String> {
    vec![id.to_string()]
}

fn num(v: Option<&String>) -> i64 {
    v.and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// `system_id`'s points; zeros when it has no row. A field that does not
/// parse reads as 0: one index lost, not the world.
pub fn indices(d: &Deltas, system_id: i64) -> Indices {
    d.of(KIND)
        .get(&key(system_id))
        .map(|v| Indices {
            military: num(v.first()),
            industrial: num(v.get(1)),
            military_today: num(v.get(2)),
            industrial_today: num(v.get(3)),
            military_up: v.get(4).is_some_and(|s| s == "1"),
            industrial_up: v.get(5).is_some_and(|s| s == "1"),
        })
        .unwrap_or_default()
}

fn put_indices(d: &mut Deltas, system_id: i64, x: &Indices) {
    if *x == Indices::default() {
        d.del(KIND, &key(system_id));
        return;
    }
    let flag = |b: bool| if b { "1" } else { "0" }.to_owned();
    d.put(
        KIND,
        key(system_id),
        vec![
            x.military.to_string(),
            x.industrial.to_string(),
            x.military_today.to_string(),
            x.industrial_today.to_string(),
            flag(x.military_up),
            flag(x.industrial_up),
        ],
    );
}

/// Add `points` to one index of `system_id`, capped at [`TOP_POINTS`].
/// Only a **claimed** system gains (R-SOV §6.6). Returns the index's level
/// before and after, or `None` when nothing was added.
pub fn add_points(
    d: &mut Deltas,
    system_id: i64,
    which: Index,
    points: i64,
) -> Option<(usize, usize)> {
    if points <= 0 || sov::holder(d, system_id).is_none() {
        return None;
    }
    let mut x = indices(d, system_id);
    let (total, today) = match which {
        Index::Military => (&mut x.military, &mut x.military_today),
        Index::Industrial => (&mut x.industrial, &mut x.industrial_today),
    };
    let before = level(*total);
    let added = points.min(TOP_POINTS - (*total).min(TOP_POINTS));
    *total += added;
    *today = today.saturating_add(points);
    let after = level(*total);
    put_indices(d, system_id, &x);
    Some((before, after))
}

/// One rollover's decay on `x`: each index loses up to [`DECAY_PER_DAY`]
/// and is "increasing" when the day's gain beat what it lost (an empty
/// index that gained nothing is not). The day's gains start again.
pub fn decay_one_day(x: &Indices) -> Indices {
    let lost = |points: i64| points.clamp(0, DECAY_PER_DAY);
    let (ml, il) = (lost(x.military), lost(x.industrial));
    Indices {
        military: x.military - ml,
        industrial: x.industrial - il,
        military_today: 0,
        industrial_today: 0,
        military_up: x.military_today > ml,
        industrial_up: x.industrial_today > il,
    }
}

// ---------------------------------------------------------------------------
// The capital
// ---------------------------------------------------------------------------

/// An alliance's capital (`allianceRegistry.GetCapitalSystemInfo`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capital {
    pub current: Option<i64>,
    /// A pending capital and when it takes over.
    pub new: Option<i64>,
    pub valid_after: Option<i64>,
}

fn opt(v: Option<&String>) -> Option<i64> {
    v.and_then(|s| s.parse().ok())
}

fn opt_field(v: Option<i64>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| v.to_string())
}

/// The stored `sovcap` row: the capital last announced.
pub fn stored_capital(d: &Deltas, alliance_id: i64) -> Capital {
    d.of(KIND_CAPITAL)
        .get(&key(alliance_id))
        .map(|v| Capital {
            current: opt(v.first()),
            new: opt(v.get(1)),
            valid_after: opt(v.get(2)),
        })
        .unwrap_or_default()
}

fn put_capital(d: &mut Deltas, alliance_id: i64, c: &Capital) {
    if *c == Capital::default() {
        d.del(KIND_CAPITAL, &key(alliance_id));
    } else {
        d.put(
            KIND_CAPITAL,
            key(alliance_id),
            vec![
                opt_field(c.current),
                opt_field(c.new),
                opt_field(c.valid_after),
            ],
        );
    }
}

/// The capital at `now`, given the stored row and the systems the alliance
/// holds:
///
/// - a pending capital no longer held is dropped; one whose time has come
///   is the current capital;
/// - a current capital no longer held is no capital (EvEJS drops it the
///   same way **[V2]**);
/// - **[V2]** an alliance holding exactly one system has it as capital at
///   once, with nothing pending (EvEJS's "retail" rule, a fact). Falsified
///   by a retail alliance whose only system waits seven days.
pub fn resolve_capital(stored: Capital, held: &[i64], now: i64) -> Capital {
    let mut c = stored;
    if c.new.is_some_and(|s| !held.contains(&s)) {
        c.new = None;
        c.valid_after = None;
    }
    if let (Some(new), Some(at)) = (c.new, c.valid_after)
        && now >= at
    {
        c.current = Some(new);
        c.new = None;
        c.valid_after = None;
    }
    if c.new.is_none() {
        c.valid_after = None;
    }
    if c.current.is_some_and(|s| !held.contains(&s)) {
        c.current = None;
    }
    if let [only] = held {
        c.current = Some(*only);
        c.new = None;
        c.valid_after = None;
    }
    if c.new.is_some() && c.new == c.current {
        c.new = None;
        c.valid_after = None;
    }
    c
}

fn held_systems(d: &Deltas, alliance_id: i64) -> Vec<i64> {
    sov::claims_of(d, alliance_id)
        .iter()
        .map(|c| c.system_id)
        .collect()
}

/// `alliance_id`'s capital at `now` ([`resolve_capital`]).
pub fn capital(d: &Deltas, alliance_id: i64, now: i64) -> Capital {
    resolve_capital(
        stored_capital(d, alliance_id),
        &held_systems(d, alliance_id),
        now,
    )
}

/// Why a capital change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalRefusal {
    /// The alliance does not hold that system.
    NotHeld,
}

/// `SetCapitalSystem(system)`: `system` becomes the capital `delay` after
/// `now` (the caller passes [`CAPITAL_CHANGE_DELAY`] scaled by the world
/// clock). A second choice replaces a pending one and restarts the wait,
/// as the prime hour does; choosing the current capital cancels a pending
/// change. Writes the resolved row; announcing is [`settle_capital`]'s.
pub fn set_capital(
    d: &mut Deltas,
    alliance_id: i64,
    system_id: i64,
    now: i64,
    delay: i64,
) -> Result<Capital, CapitalRefusal> {
    let held = held_systems(d, alliance_id);
    if !held.contains(&system_id) {
        return Err(CapitalRefusal::NotHeld);
    }
    let mut c = resolve_capital(stored_capital(d, alliance_id), &held, now);
    if c.current == Some(system_id) {
        c.new = None;
        c.valid_after = None;
    } else {
        c.new = Some(system_id);
        c.valid_after = Some(now.saturating_add(delay));
    }
    put_capital(d, alliance_id, &c);
    Ok(c)
}

/// `CancelCapitalSystemTransition()`: drop the pending capital.
pub fn cancel_capital(d: &mut Deltas, alliance_id: i64, now: i64) -> Capital {
    let held = held_systems(d, alliance_id);
    let mut c = resolve_capital(stored_capital(d, alliance_id), &held, now);
    c.new = None;
    c.valid_after = None;
    put_capital(d, alliance_id, &c);
    c
}

/// Whether `system_id` is its holder's capital at `now`.
pub fn is_capital(d: &Deltas, system_id: i64, now: i64) -> bool {
    sov::holder(d, system_id).is_some_and(|a| capital(d, a, now).current == Some(system_id))
}

// ---------------------------------------------------------------------------
// One system's bonus, for the rows
// ---------------------------------------------------------------------------

/// What a claim's rows carry from this module: `isCapital` and the ADM
/// (`defenseMultiplier`, the occupancy levels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bonus {
    pub is_capital: bool,
    pub adm: f64,
}

impl Bonus {
    /// A neutral hub's: no capital, the lowest ADM.
    pub const NONE: Bonus = Bonus {
        is_capital: false,
        adm: MIN_OCCUPANCY_BONUS,
    };
}

/// The three levels of a claimed system at `now`: military, industrial,
/// strategic.
pub fn levels(d: &Deltas, c: &Claim, now: i64, clock: GameClock) -> (usize, usize, usize) {
    let x = indices(d, c.system_id);
    (
        level(x.military),
        level(x.industrial),
        strategic_level(claimed_seconds(c.claim_time, now, clock)),
    )
}

/// `c`'s [`Bonus`] at `now`; [`Bonus::NONE`] for a hub nobody holds.
pub fn bonus(d: &Deltas, c: &Claim, now: i64, clock: GameClock) -> Bonus {
    let Some(alliance) = c.holder() else {
        return Bonus::NONE;
    };
    let is_capital = capital(d, alliance, now).current == Some(c.system_id);
    let (m, i, s) = levels(d, c, now, clock);
    Bonus {
        is_capital,
        adm: adm(m, i, s, is_capital),
    }
}

/// The ADM of `system_id` at `now`: 1 when nobody holds it. For D3a's
/// window.
pub fn system_adm(d: &Deltas, system_id: i64, now: i64, clock: GameClock) -> f64 {
    sov::claim(d, system_id).map_or(MIN_OCCUPANCY_BONUS, |c| bonus(d, &c, now, clock).adm)
}

// ---------------------------------------------------------------------------
// The client's answers (R-SOV §3)
// ---------------------------------------------------------------------------

fn index_kv(points: i64, increasing: bool) -> PyValue {
    keyval(vec![
        ("points", PyValue::Int(points)),
        ("increasing", PyValue::Bool(increasing)),
    ])
}

/// `devIndexManager.GetDevelopmentIndicesForSystem(system)`: `{1583:
/// KeyVal(points, increasing), 1584: ..., 1615: KeyVal(points = claim
/// seconds, increasing = True)}` for a claimed system, `{}` otherwise.
/// **1615 must be there**: `sovSvc.GetLevelInfoForIndex` overwrites its
/// own claim-time reading with `devIndex.points`, or 0 when absent **[V]**.
pub fn dev_indices_at(d: &Deltas, system_id: i64, now: i64, clock: GameClock) -> PyValue {
    let Some(c) = sov::claim(d, system_id).filter(Claim::is_claimed) else {
        return PyValue::Dict(vec![]);
    };
    let x = indices(d, system_id);
    PyValue::Dict(vec![
        (
            index_kv(x.military, x.military_up),
            PyValue::Int(ATTR_DEV_INDEX_MILITARY),
        ),
        (
            index_kv(x.industrial, x.industrial_up),
            PyValue::Int(ATTR_DEV_INDEX_INDUSTRIAL),
        ),
        (
            index_kv(claimed_seconds(c.claim_time, now, clock), true),
            PyValue::Int(ATTR_DEV_INDEX_SOVEREIGNTY),
        ),
    ])
}

/// `devIndexManager.GetAllDevelopmentIndices()`: one `KeyVal(solarSystemID,
/// militaryPoints, industrialPoints, claimedFor)` per claimed system,
/// `claimedFor` in **whole days** (the client multiplies by 86 400 **[V]**).
pub fn all_dev_indices_at(d: &Deltas, now: i64, clock: GameClock) -> PyValue {
    PyValue::List(
        sov::all(d)
            .iter()
            .filter(|c| c.is_claimed())
            .map(|c| {
                let x = indices(d, c.system_id);
                keyval(vec![
                    ("solarSystemID", PyValue::Int(c.system_id)),
                    ("militaryPoints", PyValue::Int(x.military)),
                    ("industrialPoints", PyValue::Int(x.industrial)),
                    (
                        "claimedFor",
                        PyValue::Int(
                            claimed_seconds(c.claim_time, now, clock) / CLAIM_DAYS_TO_SECONDS,
                        ),
                    ),
                ])
            })
            .collect(),
    )
}

/// `allianceRegistry.GetCapitalSystemInfo()`: `currentCapitalSystem`,
/// `newCapitalSystem`, `newCapitalSystemValidAfter` (`all_ui_systems` reads
/// all three; `sovSvc.GetMyCapitalSystem` the first) **[V]**. The time is 0
/// when nothing is pending (EvEJS sends a zero FILETIME, a fact; the client
/// reads it only under `if newCapitalSystem`).
pub fn capital_info(c: &Capital) -> PyValue {
    let id = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    keyval(vec![
        ("currentCapitalSystem", id(c.current)),
        ("newCapitalSystem", id(c.new)),
        (
            "newCapitalSystemValidAfter",
            PyValue::Int(c.valid_after.unwrap_or(0)),
        ),
    ])
}

/// The world's "now" and clock, for a service arm.
fn now_of(reg: &Registry) -> (i64, GameClock) {
    (crate::pyutil::filetime_now(), crate::game_clock::of(reg))
}

/// [`dev_indices_at`] on the registry, now.
pub fn dev_indices(reg: &Registry, system_id: i64) -> PyValue {
    let (now, clock) = now_of(reg);
    dev_indices_at(reg.deltas(), system_id, now, clock)
}

/// [`all_dev_indices_at`] on the registry, now.
pub fn all_dev_indices(reg: &Registry) -> PyValue {
    let (now, clock) = now_of(reg);
    all_dev_indices_at(reg.deltas(), now, clock)
}

/// `GetCapitalSystemInfo` for `alliance` (none: no capital).
pub fn capital_system_info(reg: &Registry, alliance: Option<i64>) -> PyValue {
    let now = crate::pyutil::filetime_now();
    capital_info(&alliance.map_or_else(Capital::default, |a| capital(reg.deltas(), a, now)))
}

/// `SetCapitalSystem(solarSystemID)` and `CancelCapitalSystemTransition()`
/// on the alliance moniker. `allowed` is the caller's verdict that the
/// actor is a director (or the CEO) of `alliance`'s executor corp, the
/// rule the window's `_IsDirectorInExecCorp` gates the buttons on **[V]**
/// (the server's own check is EvEJS's, a fact **[V2]**). `None` for any
/// other method.
pub fn capital_call(
    reg: &mut Registry,
    method: &str,
    args: &[PyValue],
    alliance: Option<i64>,
    allowed: bool,
) -> Option<Result<PyValue, ServiceError>> {
    if method != "SetCapitalSystem" && method != "CancelCapitalSystemTransition" {
        return None;
    }
    let refuse = |why: &str| {
        Some(Err(ServiceError::Raise(eve_proto::user_error(
            "CustomNotify",
            crate::pyutil::pypairs(vec![("notify", PyValue::Unicode(why.to_owned()))]),
        ))))
    };
    let Some(alliance) = alliance else {
        return refuse("You are not in an alliance.");
    };
    if !allowed {
        return Some(Err(ServiceError::Raise(eve_proto::user_error(
            "CrpAccessDenied",
            crate::pyutil::pypairs(vec![(
                "reason",
                PyValue::Unicode(
                    "Only a director of the executor corporation can change the capital."
                        .to_owned(),
                ),
            )]),
        ))));
    }
    let now = crate::pyutil::filetime_now();
    if method == "CancelCapitalSystemTransition" {
        cancel_capital(reg.deltas_mut(), alliance, now);
        settle_capital(reg, alliance, now);
        return Some(Ok(PyValue::None));
    }
    let Some(system) = (match args.first() {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }) else {
        return Some(Err(ServiceError::BadArguments {
            service: "allianceRegistry".to_owned(),
            method: method.to_owned(),
            why: "a solar system id".to_owned(),
        }));
    };
    let delay = crate::game_clock::span(reg, CAPITAL_CHANGE_DELAY);
    match set_capital(reg.deltas_mut(), alliance, system, now, delay) {
        Ok(_) => {
            settle_capital(reg, alliance, now);
            Some(Ok(PyValue::None))
        }
        Err(CapitalRefusal::NotHeld) => {
            refuse("Your alliance does not hold sovereignty in that system.")
        }
    }
}

// ---------------------------------------------------------------------------
// Pushes
// ---------------------------------------------------------------------------

fn push(reg: &mut Registry, to: Vec<i64>, event: &'static str, args: Vec<PyValue>) {
    if !to.is_empty() {
        reg.push(SovPush { to, event, args });
    }
}

/// Online characters in `system_id`.
fn online_in(reg: &Registry, system_id: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|id| reg.session_system(*id) == Some(system_id))
        .collect()
}

/// Online members of `alliance_id`.
fn online_members(reg: &Registry, alliance_id: i64) -> Vec<i64> {
    reg.sessions()
        .online_characters()
        .into_iter()
        .filter(|id| reg.character(*id).and_then(|c| c.alliance_id) == Some(alliance_id))
        .collect()
}

/// `OnSolarSystemDevIndexChanged(solarSystemID)` to the pilots in the
/// system: the only ones it does anything for (`sovSvc` drops its memo and
/// redraws the location panel when the system is `solarsystemid2` **[V]**).
pub fn push_dev_index_changed(reg: &mut Registry, system_id: i64) {
    let to = online_in(reg, system_id);
    push(
        reg,
        to,
        "OnSolarSystemDevIndexChanged",
        vec![PyValue::Int(system_id)],
    );
}

/// Write `alliance_id`'s capital as resolved at `now` and, when the current
/// capital moved, push `OnCapitalSystemChanged((alliance, old, new))` --
/// **one tuple argument**, which the location panel unpacks into three
/// **[V]** -- to the alliance's online members and the pilots in both
/// systems, and the two systems' structure rows (their `isCapital`
/// changed). Returns whether the capital moved.
pub fn settle_capital(reg: &mut Registry, alliance_id: i64, now: i64) -> bool {
    let stored = stored_capital(reg.deltas(), alliance_id);
    let resolved = capital(reg.deltas(), alliance_id, now);
    if stored != resolved {
        put_capital(reg.deltas_mut(), alliance_id, &resolved);
    }
    if stored.current == resolved.current {
        return false;
    }
    let mut to = online_members(reg, alliance_id);
    for s in stored.current.into_iter().chain(resolved.current) {
        for id in online_in(reg, s) {
            if !to.contains(&id) {
                to.push(id);
            }
        }
    }
    let id = |v: Option<i64>| v.map_or(PyValue::None, PyValue::Int);
    push(
        reg,
        to,
        "OnCapitalSystemChanged",
        vec![PyValue::Tuple(vec![
            PyValue::Int(alliance_id),
            id(stored.current),
            id(resolved.current),
        ])],
    );
    for s in stored.current.into_iter().chain(resolved.current) {
        sov::push_structures(reg, s, &[alliance_id], None);
    }
    true
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

/// Add `points` to `which` in `system_id` and tell the system when a level
/// moved.
fn gain(reg: &mut Registry, system_id: i64, which: Index, points: i64) {
    if let Some((before, after)) = add_points(reg.deltas_mut(), system_id, which, points)
        && before != after
    {
        push_dev_index_changed(reg, system_id);
    }
}

/// **`kill_hooks::NPC` row**: an NPC killed in a claimed system adds
/// [`MILITARY_POINTS_PER_NPC`] to its military index. Only a kill with a
/// killer counts [A: a GM removal is not ratting].
pub fn on_npc_kill(reg: &mut Registry, ev: &crate::kill_hooks::KillEvent) {
    if ev.npc.is_some() && ev.killer.is_some() {
        gain(reg, ev.system, Index::Military, MILITARY_POINTS_PER_NPC);
    }
}

/// **The mining hook** (`mining::cut`): `m3` cubic metres of ore, ice or
/// gas cut in `system_id` add [`INDUSTRIAL_POINTS_PER_M3`] each to its
/// industrial index when the system is claimed.
pub fn on_mined(reg: &mut Registry, system_id: i64, m3: f64) {
    if !(m3.is_finite() && m3 > 0.0) {
        return;
    }
    let points = (m3 * INDUSTRIAL_POINTS_PER_M3).round();
    if points >= 1.0 {
        gain(
            reg,
            system_id,
            Index::Industrial,
            points.min(TOP_POINTS as f64) as i64,
        );
    }
}

/// **The claim hook** (`sov::announce`): after a holder change, settle both
/// alliances' capitals (the one-system rule, a lost capital) and tell the
/// system its indices changed (they appear or vanish with the claim).
pub fn on_claim_change(reg: &mut Registry, change: &sov::Change) {
    let now = crate::pyutil::filetime_now();
    for a in change.old.into_iter().chain(change.new) {
        settle_capital(reg, a, now);
    }
    push_dev_index_changed(reg, change.system_id);
}

/// **The rollover step** (`rollover_steps::STEPS`, after the economy's):
/// one game day of decay on every stored index ([`decay_one_day`]), and
/// `OnSolarSystemDevIndexChanged` to every claimed system (its strategic
/// age moved a day too). Its count is the index rows decayed.
///
/// A catch-up rollover after N days down decays **one** day here: the days
/// nobody was on are packet E2's.
pub fn on_rollover(reg: &mut Registry, _day: &crate::rollover::Day) -> usize {
    let rows: Vec<(i64, Indices)> = reg
        .deltas()
        .of(KIND)
        .keys()
        .filter_map(|k| k.first()?.parse::<i64>().ok())
        .map(|s| (s, indices(reg.deltas(), s)))
        .collect();
    for (system, x) in &rows {
        put_indices(reg.deltas_mut(), *system, &decay_one_day(x));
    }
    let claimed: Vec<i64> = sov::all(reg.deltas())
        .iter()
        .filter(|c| c.is_claimed())
        .map(|c| c.system_id)
        .collect();
    for system in claimed {
        push_dev_index_changed(reg, system);
    }
    rows.len()
}

/// **The tick stage** (`tick::STAGES`): every alliance whose pending
/// capital's time has come is settled, so `OnCapitalSystemChanged` goes
/// out when the capital moves rather than at the next read. Reads only the
/// `sovcap` rows, which are few.
pub fn tick(reg: &mut Registry, _: Instant, now: BlueTime) {
    let due: Vec<i64> = reg
        .deltas()
        .of(KIND_CAPITAL)
        .iter()
        .filter(|(_, v)| opt(v.get(2)).is_some_and(|at| now >= at))
        .filter_map(|(k, _)| k.first()?.parse().ok())
        .collect();
    for alliance in due {
        settle_capital(reg, alliance, now);
    }
}

// ---------------------------------------------------------------------------
// /sov index, /sov capital
// ---------------------------------------------------------------------------

/// The `/sov` words this module answers (`service::sov::slash` hands them
/// over).
pub const SLASH_WORDS: &[&str] = &["index", "capital"];

const USAGE: &str =
    "usage: /sov index [systemID|here] [military industrial] | /sov capital <systemID|here> [now]";

fn parse_system(reg: &Registry, actor: Option<i64>, word: &str) -> Result<i64, String> {
    if word == "here" {
        return actor
            .and_then(|id| reg.session_system(id))
            .ok_or_else(|| "you are not in a system".to_owned());
    }
    word.parse::<i64>()
        .map_err(|_| format!("'{word}' is not a system id"))
}

fn index_text(reg: &Registry, system_id: i64) -> String {
    let (now, clock) = now_of(reg);
    let d = reg.deltas();
    let Some(c) = sov::claim(d, system_id).filter(Claim::is_claimed) else {
        return format!("system {system_id}: not claimed, no indices");
    };
    let x = indices(d, system_id);
    let (m, i, s) = levels(d, &c, now, clock);
    let b = bonus(d, &c, now, clock);
    format!(
        "system {system_id}: military {} (level {m}{}), industrial {} (level {i}{}), strategic {} days (level {s}); capital {}; ADM {:.1}; window {} min",
        x.military,
        if x.military_up { ", rising" } else { "" },
        x.industrial,
        if x.industrial_up { ", rising" } else { "" },
        claimed_seconds(c.claim_time, now, clock) / CLAIM_DAYS_TO_SECONDS,
        b.is_capital,
        b.adm,
        vulnerability_window(b.adm) / (60 * SECOND),
    )
}

/// `/sov index [system] [military industrial]` reads a claimed system's
/// indices and ADM, or sets its two stored indices (points, capped at
/// [`TOP_POINTS`]); `/sov capital <system> [now]` makes a held system its
/// holder's pending capital, or the capital at once with `now`. GM tools
/// for tests and the human checks.
pub fn slash(reg: &mut Registry, actor: Option<i64>, args: &[&str]) -> String {
    match args {
        ["index"] => match parse_system(reg, actor, "here") {
            Ok(s) => index_text(reg, s),
            Err(e) => format!("{USAGE} -- {e}"),
        },
        ["index", system] => match parse_system(reg, actor, system) {
            Ok(s) => index_text(reg, s),
            Err(e) => format!("{USAGE} -- {e}"),
        },
        ["index", system, military, industrial] => {
            let s = match parse_system(reg, actor, system) {
                Ok(s) => s,
                Err(e) => return format!("{USAGE} -- {e}"),
            };
            let (Ok(m), Ok(i)) = (military.parse::<i64>(), industrial.parse::<i64>()) else {
                return format!("{USAGE} -- the points are whole numbers");
            };
            if sov::holder(reg.deltas(), s).is_none() {
                return format!("system {s} is not claimed");
            }
            let mut x = indices(reg.deltas(), s);
            x.military = m.clamp(0, TOP_POINTS);
            x.industrial = i.clamp(0, TOP_POINTS);
            put_indices(reg.deltas_mut(), s, &x);
            push_dev_index_changed(reg, s);
            index_text(reg, s)
        }
        ["capital", system] | ["capital", system, "now"] => {
            let s = match parse_system(reg, actor, system) {
                Ok(s) => s,
                Err(e) => return format!("{USAGE} -- {e}"),
            };
            let Some(a) = sov::holder(reg.deltas(), s) else {
                return format!("system {s} is not claimed");
            };
            let now = crate::pyutil::filetime_now();
            let delay = if args.len() == 3 {
                0
            } else {
                crate::game_clock::span(reg, CAPITAL_CHANGE_DELAY)
            };
            if set_capital(reg.deltas_mut(), a, s, now, delay).is_err() {
                return format!("system {s} is not held by alliance {a}");
            }
            settle_capital(reg, a, now);
            let c = capital(reg.deltas(), a, now);
            format!(
                "alliance {a}: capital {:?}, pending {:?} valid after {:?}",
                c.current, c.new, c.valid_after
            )
        }
        _ => USAGE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyutil::dict_get;

    // Invented ids: nothing here is a client value.
    const SYSTEM: i64 = 30_900_101;
    const OTHER_SYSTEM: i64 = 30_900_102;
    const THIRD_SYSTEM: i64 = 30_900_103;
    const ALLIANCE: i64 = 99_100_101;
    const CORP: i64 = 98_100_101;
    const NOW: i64 = 134_200_000_000_000_000;
    const PILOT_HERE: i64 = 90_100_101;
    const PILOT_AWAY: i64 = 90_100_102;
    const MEMBER: i64 = 90_100_103;

    fn claim(d: &mut Deltas, system: i64, at: i64) {
        sov::set_claim(d, system, Some((ALLIANCE, CORP)), at).expect("a change");
    }

    /// A KeyVal's attribute.
    fn attr<'a>(v: &'a PyValue, name: &str) -> Option<&'a PyValue> {
        match v {
            PyValue::Instance { state, .. } => dict_get(state, name.as_bytes()),
            _ => None,
        }
    }

    fn int_key(dict: &PyValue, k: i64) -> Option<&PyValue> {
        let PyValue::Dict(pairs) = dict else {
            return None;
        };
        pairs
            .iter()
            .find(|(_, key)| *key == PyValue::Int(k))
            .map(|(v, _)| v)
    }

    /// Three pilots online: one in SYSTEM, one elsewhere, one an alliance
    /// member elsewhere.
    fn world() -> Registry {
        let pilot = |id: i64, system: i64, alliance: Option<i64>| crate::model::Character {
            character_id: id,
            name: format!("pilot {id}"),
            ship_id: id + 1_000,
            solar_system_id: Some(system),
            alliance_id: alliance,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(PILOT_HERE, SYSTEM, None),
                pilot(PILOT_AWAY, THIRD_SYSTEM + 7, None),
                pilot(MEMBER, THIRD_SYSTEM + 7, Some(ALLIANCE)),
            ],
            ..crate::model::SandboxConfig::default()
        });
        for (n, who) in [(1, PILOT_HERE), (2, PILOT_AWAY), (3, MEMBER)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(who),
            });
        }
        let _ = reg.take_pending();
        reg
    }

    /// Every push queued, as `(event, args, sorted audience)`.
    fn pushes(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>, Vec<i64>)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| {
                let (e, a) = p.to_push();
                let mut to = p.audience().unwrap_or_default();
                to.sort_unstable();
                (e, a, to)
            })
            .collect()
    }

    #[test]
    fn level_bands_match_the_client_width() {
        assert_eq!(LEVEL_WIDTH, DEV_INDEX_DECAY_RATE * DEV_INDEX_DECAY_TIME);
        assert_eq!(level(-5), 0);
        assert_eq!(level(0), 0);
        assert_eq!(level(LEVEL_WIDTH - 1), 0);
        for l in 1..=MAX_LEVEL {
            let at = l as i64 * LEVEL_WIDTH;
            assert_eq!(level(at), l, "level {l} starts at {at}");
            assert_eq!(level(at - 1), l - 1);
        }
        assert_eq!(
            level(TOP_POINTS),
            MAX_LEVEL,
            "the cap is still the top level"
        );
        assert_eq!(level(i64::MAX), MAX_LEVEL);
        assert_eq!(TOP_POINTS, (MAX_LEVEL as i64 + 1) * LEVEL_WIDTH);
    }

    #[test]
    fn strategic_level_from_claim_days_uses_the_client_thresholds() {
        assert_eq!(strategic_level(0), 0);
        for (i, days) in STRATEGIC_LEVEL_DAYS.iter().enumerate() {
            let at = days * CLAIM_DAYS_TO_SECONDS;
            assert_eq!(strategic_level(at), i + 1, "{days} days");
            assert_eq!(strategic_level(at - 1), i, "a second short of {days} days");
        }
        assert_eq!(strategic_level(i64::MAX), MAX_LEVEL);
        // Claim age in game seconds: a scaled clock makes the same wall
        // hour N game hours.
        let hour_later = NOW + HOUR;
        assert_eq!(
            claimed_seconds(Some(NOW), hour_later, GameClock::REAL),
            3_600
        );
        let fast = GameClock::new(24).unwrap();
        assert_eq!(claimed_seconds(Some(NOW), hour_later, fast), 24 * 3_600);
        assert_eq!(claimed_seconds(None, hour_later, fast), 0);
        assert_eq!(
            claimed_seconds(Some(hour_later), NOW, fast),
            0,
            "never negative"
        );
    }

    #[test]
    fn adm_matches_the_client_formula_and_clamps() {
        assert_eq!(adm(0, 0, 0, false), BASE_BONUS);
        assert_eq!(adm(0, 0, 0, true), BASE_BONUS + CAPITAL_BONUS);
        for l in 0..=MAX_LEVEL {
            let want = BASE_BONUS
                + MILITARY_INDEX_TO_BONUS[l]
                + INDUSTRIAL_INDEX_TO_BONUS[l]
                + STRATEGIC_INDEX_TO_BONUS[l];
            assert!(
                (adm(l, l, l, false) - want.clamp(MIN_OCCUPANCY_BONUS, MAX_OCCUPANCY_BONUS)).abs()
                    < 1e-9,
                "level {l}"
            );
        }
        let one = adm(1, 2, 3, false);
        let expect = BASE_BONUS
            + MILITARY_INDEX_TO_BONUS[1]
            + INDUSTRIAL_INDEX_TO_BONUS[2]
            + STRATEGIC_INDEX_TO_BONUS[3];
        assert!((one - expect).abs() < 1e-9);
        assert_eq!(
            adm(MAX_LEVEL, MAX_LEVEL, MAX_LEVEL, true),
            MAX_OCCUPANCY_BONUS,
            "clamped at the top"
        );
        assert_eq!(adm(99, 99, 99, false), adm(0, 0, 0, false), "no such level");
        // The window shrinks with the ADM, never under its floor.
        assert_eq!(vulnerability_window(1.0), VULNERABILITY_BASE);
        assert_eq!(vulnerability_window(2.0), VULNERABILITY_BASE / 2);
        assert_eq!(
            vulnerability_window(0.0),
            VULNERABILITY_BASE,
            "ADM is at least 1"
        );
        assert_eq!(vulnerability_window(f64::NAN), VULNERABILITY_BASE);
        assert_eq!(vulnerability_window(1.0e9), VULNERABILITY_MIN);
    }

    #[test]
    fn dev_indices_for_a_claimed_system_include_the_strategic_attribute_in_seconds() {
        let mut d = Deltas::default();
        assert_eq!(
            dev_indices_at(&d, SYSTEM, NOW, GameClock::REAL),
            PyValue::Dict(vec![]),
            "unclaimed: empty"
        );
        claim(&mut d, SYSTEM, NOW);
        add_points(&mut d, SYSTEM, Index::Military, 3 * LEVEL_WIDTH).unwrap();
        let later = NOW + 2 * DAY + 5 * SECOND;
        let v = dev_indices_at(&d, SYSTEM, later, GameClock::REAL);
        let mil = int_key(&v, ATTR_DEV_INDEX_MILITARY).expect("military");
        assert_eq!(attr(mil, "points"), Some(&PyValue::Int(3 * LEVEL_WIDTH)));
        assert_eq!(attr(mil, "increasing"), Some(&PyValue::Bool(false)));
        let ind = int_key(&v, ATTR_DEV_INDEX_INDUSTRIAL).expect("industrial");
        assert_eq!(attr(ind, "points"), Some(&PyValue::Int(0)));
        let strat = int_key(&v, ATTR_DEV_INDEX_SOVEREIGNTY).expect("strategic");
        assert_eq!(
            attr(strat, "points"),
            Some(&PyValue::Int(2 * CLAIM_DAYS_TO_SECONDS + 5))
        );
        assert_eq!(attr(strat, "increasing"), Some(&PyValue::Bool(true)));
        // The claim lapses: the system answers empty again.
        sov::set_claim(&mut d, SYSTEM, None, later).unwrap();
        assert_eq!(
            dev_indices_at(&d, SYSTEM, later, GameClock::REAL),
            PyValue::Dict(vec![])
        );
    }

    #[test]
    fn all_indices_report_claimed_for_in_days() {
        let mut d = Deltas::default();
        claim(&mut d, SYSTEM, NOW);
        claim(&mut d, OTHER_SYSTEM, NOW + DAY);
        add_points(&mut d, OTHER_SYSTEM, Index::Industrial, 7).unwrap();
        let now = NOW + 3 * DAY + HOUR;
        let PyValue::List(rows) = all_dev_indices_at(&d, now, GameClock::REAL) else {
            panic!()
        };
        assert_eq!(rows.len(), 2);
        let row = |s: i64| {
            rows.iter()
                .find(|r| attr(r, "solarSystemID") == Some(&PyValue::Int(s)))
                .unwrap_or_else(|| panic!("no row for {s}"))
        };
        assert_eq!(attr(row(SYSTEM), "claimedFor"), Some(&PyValue::Int(3)));
        assert_eq!(
            attr(row(OTHER_SYSTEM), "claimedFor"),
            Some(&PyValue::Int(2))
        );
        assert_eq!(
            attr(row(OTHER_SYSTEM), "industrialPoints"),
            Some(&PyValue::Int(7))
        );
        assert_eq!(
            attr(row(OTHER_SYSTEM), "militaryPoints"),
            Some(&PyValue::Int(0))
        );
        // Days on a scaled clock are game days.
        let PyValue::List(rows) = all_dev_indices_at(&d, NOW + HOUR, GameClock::new(24).unwrap())
        else {
            panic!()
        };
        let one = rows
            .iter()
            .find(|r| attr(r, "solarSystemID") == Some(&PyValue::Int(SYSTEM)))
            .unwrap();
        assert_eq!(attr(one, "claimedFor"), Some(&PyValue::Int(1)));
    }

    #[test]
    fn npc_kill_and_mining_in_a_claimed_system_add_points() {
        let mut reg = world();
        let kill = |system: i64, killer: Option<i64>| crate::kill_hooks::KillEvent {
            victim_character: 7_000_000_001,
            victim_ship: 7_000_000_001,
            ship_type: crate::npc::FIXTURE_RAT.type_id,
            killer,
            system,
            npc: Some(&crate::npc::FIXTURE_RAT),
            when: NOW,
            park: Vec::new(),
        };
        // Unclaimed: nothing.
        on_npc_kill(&mut reg, &kill(SYSTEM, Some(PILOT_HERE)));
        on_mined(&mut reg, SYSTEM, 10.0);
        assert_eq!(indices(reg.deltas(), SYSTEM), Indices::default());
        assert!(reg.deltas().of(KIND).is_empty());

        claim(reg.deltas_mut(), SYSTEM, NOW);
        let _ = reg.take_pending();
        on_npc_kill(&mut reg, &kill(SYSTEM, Some(PILOT_HERE)));
        on_npc_kill(&mut reg, &kill(SYSTEM, None));
        on_npc_kill(&mut reg, &kill(OTHER_SYSTEM, Some(PILOT_HERE)));
        let x = indices(reg.deltas(), SYSTEM);
        assert_eq!(
            x.military, MILITARY_POINTS_PER_NPC,
            "one kill with a killer"
        );
        assert_eq!(x.military_today, MILITARY_POINTS_PER_NPC);
        on_mined(&mut reg, SYSTEM, 2.5);
        on_mined(&mut reg, SYSTEM, f64::NAN);
        on_mined(&mut reg, SYSTEM, -1.0);
        let x = indices(reg.deltas(), SYSTEM);
        assert_eq!(
            x.industrial,
            (2.5 * INDUSTRIAL_POINTS_PER_M3).round() as i64
        );
        assert_eq!(indices(reg.deltas(), OTHER_SYSTEM), Indices::default());

        // A level crossed tells the system; points stop at the top.
        let _ = reg.take_pending();
        let m3 = LEVEL_WIDTH as f64 / INDUSTRIAL_POINTS_PER_M3;
        on_mined(&mut reg, SYSTEM, m3);
        let p = pushes(&mut reg);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].0, "OnSolarSystemDevIndexChanged");
        assert_eq!(p[0].1, vec![PyValue::Int(SYSTEM)]);
        assert_eq!(p[0].2, vec![PILOT_HERE]);
        on_mined(&mut reg, SYSTEM, m3 * 100.0);
        assert_eq!(indices(reg.deltas(), SYSTEM).industrial, TOP_POINTS);

        // The rows registered for both hooks.
        assert!(
            crate::kill_hooks::NPC
                .iter()
                .any(|r| r.name == "sov_military"),
            "kill_hooks::NPC has the military row"
        );
    }

    #[test]
    fn rollover_decays_decides_the_arrow_and_tells_claimed_systems() {
        let x = Indices {
            military: DECAY_PER_DAY * 3,
            industrial: DECAY_PER_DAY / 2,
            military_today: DECAY_PER_DAY + 1,
            industrial_today: DECAY_PER_DAY / 4,
            military_up: false,
            industrial_up: true,
        };
        let y = decay_one_day(&x);
        assert_eq!(y.military, DECAY_PER_DAY * 2);
        assert_eq!(y.industrial, 0, "never below zero");
        assert!(y.military_up, "gained more than it lost");
        assert!(!y.industrial_up, "lost more than it gained");
        assert_eq!((y.military_today, y.industrial_today), (0, 0));
        assert!(
            !decay_one_day(&Indices::default()).military_up,
            "an empty index that gained nothing is not rising"
        );

        let mut reg = world();
        claim(reg.deltas_mut(), SYSTEM, NOW);
        add_points(reg.deltas_mut(), SYSTEM, Index::Military, DECAY_PER_DAY / 2).unwrap();
        let _ = reg.take_pending();
        let day = crate::rollover::Day {
            day: 2,
            days: 1,
            now: NOW + DAY,
        };
        assert_eq!(on_rollover(&mut reg, &day), 1);
        assert!(
            reg.deltas().of(KIND).is_empty(),
            "a spent index row is dropped"
        );
        let p = pushes(&mut reg);
        assert!(
            p.iter()
                .any(|(e, a, to)| *e == "OnSolarSystemDevIndexChanged"
                    && a == &vec![PyValue::Int(SYSTEM)]
                    && to == &vec![PILOT_HERE]),
            "{p:?}"
        );
        assert!(
            crate::rollover_steps::STEPS
                .iter()
                .any(|s| s.name == "sov_index"),
            "the rollover runs this step"
        );
    }

    #[test]
    fn capital_change_waits_and_pushes_the_three_tuple() {
        let mut reg = world();
        claim(reg.deltas_mut(), SYSTEM, NOW);
        // One system: it is the capital at once.
        assert_eq!(capital(reg.deltas(), ALLIANCE, NOW).current, Some(SYSTEM));
        claim(reg.deltas_mut(), OTHER_SYSTEM, NOW);
        // Two systems and nothing settled: no capital yet.
        assert_eq!(capital(reg.deltas(), ALLIANCE, NOW).current, None);
        assert_eq!(
            set_capital(reg.deltas_mut(), ALLIANCE, THIRD_SYSTEM, NOW, 1),
            Err(CapitalRefusal::NotHeld)
        );
        let c = set_capital(
            reg.deltas_mut(),
            ALLIANCE,
            OTHER_SYSTEM,
            NOW,
            CAPITAL_CHANGE_DELAY,
        )
        .unwrap();
        assert_eq!(c.new, Some(OTHER_SYSTEM));
        assert_eq!(c.valid_after, Some(NOW + CAPITAL_CHANGE_DELAY));
        let info = capital_info(&capital(reg.deltas(), ALLIANCE, NOW));
        assert_eq!(attr(&info, "currentCapitalSystem"), Some(&PyValue::None));
        assert_eq!(
            attr(&info, "newCapitalSystem"),
            Some(&PyValue::Int(OTHER_SYSTEM))
        );
        assert_eq!(
            attr(&info, "newCapitalSystemValidAfter"),
            Some(&PyValue::Int(NOW + CAPITAL_CHANGE_DELAY))
        );
        // Before the delay: nothing moves, nothing is pushed.
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), NOW + CAPITAL_CHANGE_DELAY - 1);
        assert!(pushes(&mut reg).is_empty());
        assert!(!is_capital(
            reg.deltas(),
            OTHER_SYSTEM,
            NOW + CAPITAL_CHANGE_DELAY - 1
        ));
        // At the delay the tick moves it and pushes one tuple.
        let at = NOW + CAPITAL_CHANGE_DELAY;
        tick(&mut reg, Instant::now(), at);
        let p = pushes(&mut reg);
        let moved = p
            .iter()
            .find(|x| x.0 == "OnCapitalSystemChanged")
            .unwrap_or_else(|| panic!("no capital push: {p:?}"));
        assert_eq!(
            moved.1,
            vec![PyValue::Tuple(vec![
                PyValue::Int(ALLIANCE),
                PyValue::None,
                PyValue::Int(OTHER_SYSTEM),
            ])],
            "one argument: the three-tuple"
        );
        assert_eq!(moved.2, vec![MEMBER], "the alliance's members");
        assert!(
            p.iter().any(|x| x.0 == "OnSolarSystemSovStructuresUpdated"),
            "the capital's rows changed"
        );
        assert_eq!(
            stored_capital(reg.deltas(), ALLIANCE),
            Capital {
                current: Some(OTHER_SYSTEM),
                new: None,
                valid_after: None
            },
            "settled into the row"
        );
        assert!(is_capital(reg.deltas(), OTHER_SYSTEM, at));
        tick(&mut reg, Instant::now(), at + 1);
        assert!(pushes(&mut reg).is_empty(), "settled once");

        // A move back waits again; a cancel drops it.
        set_capital(reg.deltas_mut(), ALLIANCE, SYSTEM, at, CAPITAL_CHANGE_DELAY).unwrap();
        let c = cancel_capital(reg.deltas_mut(), ALLIANCE, at);
        assert_eq!((c.current, c.new), (Some(OTHER_SYSTEM), None));

        // Losing the capital's claim drops it, and the push says so; the
        // one system left is the capital at once.
        let change = sov::set_claim(reg.deltas_mut(), OTHER_SYSTEM, None, at).unwrap();
        let _ = reg.take_pending();
        on_claim_change(&mut reg, &change);
        let p = pushes(&mut reg);
        let moved = p
            .iter()
            .find(|x| x.0 == "OnCapitalSystemChanged")
            .unwrap_or_else(|| panic!("no capital push: {p:?}"));
        assert_eq!(
            moved.1,
            vec![PyValue::Tuple(vec![
                PyValue::Int(ALLIANCE),
                PyValue::Int(OTHER_SYSTEM),
                PyValue::Int(SYSTEM),
            ])]
        );
        assert!(
            crate::module::plan()
                .stages
                .iter()
                .any(|s| s.name == "sov_capital"),
            "the clock settles due capitals"
        );
    }

    #[test]
    fn resolve_capital_drops_what_is_not_held_and_applies_what_is_due() {
        let stored = Capital {
            current: Some(SYSTEM),
            new: Some(OTHER_SYSTEM),
            valid_after: Some(NOW),
        };
        let held = [SYSTEM, OTHER_SYSTEM, THIRD_SYSTEM];
        assert_eq!(
            resolve_capital(stored, &held, NOW - 1),
            stored,
            "not due yet"
        );
        assert_eq!(
            resolve_capital(stored, &held, NOW).current,
            Some(OTHER_SYSTEM)
        );
        let lost_new = resolve_capital(stored, &[SYSTEM, THIRD_SYSTEM], NOW);
        assert_eq!(
            lost_new,
            Capital {
                current: Some(SYSTEM),
                new: None,
                valid_after: None
            }
        );
        let lost_both = resolve_capital(stored, &[THIRD_SYSTEM, 1], NOW - 1);
        assert_eq!(lost_both, Capital::default());
        assert_eq!(
            resolve_capital(Capital::default(), &[THIRD_SYSTEM], NOW).current,
            Some(THIRD_SYSTEM),
            "a lone system is the capital"
        );
        assert_eq!(resolve_capital(stored, &[], NOW), Capital::default());
    }

    #[test]
    fn structure_and_alliance_rows_carry_the_capital_and_the_adm() {
        let mut d = Deltas::default();
        claim(&mut d, SYSTEM, NOW);
        add_points(&mut d, SYSTEM, Index::Military, 2 * LEVEL_WIDTH).unwrap();
        let rows = sov::structure_rows_at(&d, SYSTEM, NOW, GameClock::REAL);
        let PyValue::List(rows) = rows else { panic!() };
        let want = adm(2, 0, 0, true);
        assert_eq!(attr(&rows[0], "isCapital"), Some(&PyValue::Bool(true)));
        assert_eq!(
            attr(&rows[0], "defenseMultiplier"),
            Some(&PyValue::Real(want))
        );
        let PyValue::Tuple(t) = sov::alliance_structures_at(&d, ALLIANCE, NOW, GameClock::REAL)
        else {
            panic!()
        };
        for list in &t[..2] {
            let PyValue::List(r) = list else { panic!() };
            assert_eq!(
                attr(&r[0], "campaignOccupancyLevel"),
                Some(&PyValue::Real(want))
            );
            assert_eq!(
                attr(&r[0], "vulnerabilityOccupancyLevel"),
                Some(&PyValue::Real(want))
            );
        }
        assert_eq!(system_adm(&d, SYSTEM, NOW, GameClock::REAL), want);
        assert_eq!(
            system_adm(&d, OTHER_SYSTEM, NOW, GameClock::REAL),
            MIN_OCCUPANCY_BONUS
        );
    }

    #[test]
    fn sov_index_slash_reads_and_sets_and_capital_now_skips_the_wait() {
        let mut reg = world();
        claim(reg.deltas_mut(), SYSTEM, NOW);
        claim(reg.deltas_mut(), OTHER_SYSTEM, NOW);
        let out = slash(
            &mut reg,
            None,
            &["index", &SYSTEM.to_string(), &LEVEL_WIDTH.to_string(), "-4"],
        );
        assert!(out.contains("level 1"), "{out}");
        let x = indices(reg.deltas(), SYSTEM);
        assert_eq!((x.military, x.industrial), (LEVEL_WIDTH, 0));
        assert!(slash(&mut reg, Some(PILOT_HERE), &["index"]).contains("military"));
        let out = slash(
            &mut reg,
            None,
            &["capital", &OTHER_SYSTEM.to_string(), "now"],
        );
        assert!(out.contains(&format!("Some({OTHER_SYSTEM})")), "{out}");
        assert!(is_capital(
            reg.deltas(),
            OTHER_SYSTEM,
            crate::pyutil::filetime_now()
        ));
        assert!(slash(&mut reg, None, &["capital", "1"]).contains("not claimed"));
        assert!(slash(&mut reg, None, &["frob"]).starts_with("usage"));
    }

    #[test]
    fn dev_index_push_reaches_pilots_in_space_not_the_docked_system() {
        use crate::space_state::{Motion, Presence};
        let mut reg = world();
        let arrive = |reg: &mut Registry, who: i64, at: i64| {
            reg.roster_mut().arrive(Presence::arriving(
                who,
                who + 1_000,
                at,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        };
        // Docked elsewhere, in space in SYSTEM: reached.
        arrive(&mut reg, PILOT_AWAY, SYSTEM);
        // Docked in SYSTEM, in space elsewhere: not reached.
        arrive(&mut reg, PILOT_HERE, OTHER_SYSTEM);
        push_dev_index_changed(&mut reg, SYSTEM);
        let all = pushes(&mut reg);
        let (_, _, to) = all
            .iter()
            .find(|p| p.0 == "OnSolarSystemDevIndexChanged")
            .expect("pushed");
        assert_eq!(to, &vec![PILOT_AWAY]);
    }
}
