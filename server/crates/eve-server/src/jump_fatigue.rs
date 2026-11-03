//! **Jump fatigue and the jump timers** (CAP-F; `docs/lanes/cap-f.md`,
//! `out/research/R-CAP.md` §1.6 and §3.5).
//!
//! Two timers per pilot, both absolute wall FILETIMEs: the **reactivation**
//! (activation) timer, during which the jump drive refuses, and the **jump
//! fatigue** timer, which decays one-for-one with time and grows with every
//! jump. The pure model is [`fatigue_after`], [`activation_after`] and
//! [`after_jump`]; [`on_jump`] is the entry point the jump drive (CAP-E)
//! calls once a jump has happened; [`timers`] and [`activation_remaining`]
//! are the reads.
//!
//! ## The formulas, read from the client **[V]**
//!
//! The constants are `jumpFatigue/const.py` (build 3619879's decompile).
//! The two formulas are the client's own prediction of "the next jump":
//!
//! - fatigue: `markerTooltipUtil._GetJumpFatigueAfterJump(distance)`
//!   (`eve/client/script/ui/shared/mapView/markers/markerTooltipUtil.py`):
//!   `k = ADDITIONAL_DISTANCE + distance * shipMultiplier`,
//!   `new = clamp(remaining * k, k * FATIGUE_SECONDS_PER_DISTANCE,
//!   FATIGUE_MAX_SECONDS)`, `int(new * SEC)`; `mathext.clamp(v, lo, hi)` is
//!   `max(lo, min(v, hi))`, so **the lower bound wins when `k * 600` is above
//!   the cap** (a jump of more than 29 light years at multiplier 1): the
//!   client's own arithmetic, kept here so the HUD hint and the server agree.
//! - reactivation: `markerTooltipUtil._ReactivationTimerAfterJump` with
//!   `crimewatchHints.GetJumpActivationTimesForDistance`:
//!   `max(max(1 * 60, remaining * FATIGUE_RATIO), k * 60)` seconds, `int`
//!   truncated, times `SEC`.
//! - the ship multiplier is attribute 1971 (`jumpFatigueMultiplier`),
//!   default 1.0 in the extract **[V]**.
//! - both timers reach the client as `OnJumpTimersUpdated(jumpActivation,
//!   jumpFatigue, lastUpdated, doAlert)` and `jumpTimers.GetTimers`, and
//!   the HUD draws a timer only while its value is in the future, with
//!   `maxDuration = expiry - lastUpdated` (`crimewatchTimers.py`).
//!
//! ## What the client does not say, **[A]**
//!
//! That the server applies the same formulas as the tooltip. The client
//! contains no server code, so the tooltip is the only statement of the rule
//! there is. Falsified by a HUD "next jump" hint that disagrees with the
//! timer the server then sets (H-CAP-2).
//!
//! ## The game clock
//!
//! Every stored value is a wall FILETIME. The model works in **game
//! seconds**: the remaining fatigue is the wall remainder times the scale
//! ([`GameClock::elapsed`]) and a span made by a jump is compressed
//! ([`GameClock::span`]) before it becomes a deadline. At scale 1 nothing
//! changes. At scale N the HUD's own "next jump" hint, which knows no
//! scale, reads N times too long; a soak-only difference.

use crate::damage::BlueTime;
use crate::game_clock::GameClock;
use crate::industry::FILETIME_SECOND as SEC;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;
use std::time::Duration;

/// The delta kind this module owns (`delta_kinds::KINDS`): `char ->
/// activation_until fatigue_until last_updated`, wall FILETIMEs, only while
/// one of the two is in the future.
pub const KIND: &str = "jumpfat";

/// `jumpFatigue/const.py ADDITIONAL_DISTANCE` **[V]**.
pub const ADDITIONAL_DISTANCE: f64 = 1.0;
/// `jumpFatigue/const.py ACTIVATION_SECONDS_PER_DISTANCE` **[V]**.
pub const ACTIVATION_SECONDS_PER_DISTANCE: f64 = 60.0;
/// `jumpFatigue/const.py FATIGUE_SECONDS_PER_DISTANCE` **[V]**.
pub const FATIGUE_SECONDS_PER_DISTANCE: f64 = 600.0;
/// `jumpFatigue/const.py FATIGUE_MAX_SECONDS` **[V]**: 5 hours (retail's
/// maximum is 30 days; this client's file says 18000).
pub const FATIGUE_MAX_SECONDS: f64 = 18_000.0;
/// `jumpFatigue/const.py FATIGUE_RATIO` **[V]**: activation seconds per
/// fatigue second, `60 / 600`.
pub const FATIGUE_RATIO: f64 = ACTIVATION_SECONDS_PER_DISTANCE / FATIGUE_SECONDS_PER_DISTANCE;
/// Dogma attribute `jumpFatigueMultiplier` **[V]** (`typedata.tsv`).
pub const ATTR_FATIGUE_MULTIPLIER: i64 = 1971;
/// Its `defaultValue` in the extract **[V]**: what a hull that does not set
/// it uses (also the fallback for a hull the table lacks).
pub const DEFAULT_MULTIPLIER: f64 = 1.0;

/// A pilot's two timers, as the client takes them. All wall FILETIMEs; a
/// value of 0, or one in the past, is "no timer".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timers {
    pub activation_until: BlueTime,
    pub fatigue_until: BlueTime,
    pub last_updated: BlueTime,
}

impl Timers {
    /// Both timers over at `now`.
    pub fn is_expired(&self, now: BlueTime) -> bool {
        self.activation_until <= now && self.fatigue_until <= now
    }

    /// The fatigue left at `now`, in **game** seconds.
    pub fn fatigue_seconds(&self, now: BlueTime, clock: GameClock) -> f64 {
        let wall = (self.fatigue_until - now).max(0);
        clock.elapsed(wall) as f64 / SEC as f64
    }

    /// What `jumpTimers.GetTimers` answers: the timers as they stand at
    /// `now`, **zeros when over** (never `None`: `markerTooltipUtil` does
    /// arithmetic on each [V], see the `jumpTimers` arm).
    pub fn at(&self, now: BlueTime) -> Timers {
        if self.is_expired(now) {
            return Timers::default();
        }
        *self
    }

    /// The three values in `GetTimers`' order.
    pub fn wire(&self) -> Vec<PyValue> {
        vec![
            PyValue::Int(self.activation_until),
            PyValue::Int(self.fatigue_until),
            PyValue::Int(self.last_updated),
        ]
    }
}

/// `k`: the jump's weight, `ADDITIONAL_DISTANCE + distance * multiplier`.
fn weight(ly: f64, multiplier: f64) -> f64 {
    ADDITIONAL_DISTANCE + ly.max(0.0) * multiplier.max(0.0)
}

/// The fatigue, in game seconds, after a jump of `ly` light years from
/// `remaining` game seconds of fatigue, at ship multiplier `multiplier`.
/// `_GetJumpFatigueAfterJump`, including `mathext.clamp`'s argument order
/// (the lower bound wins over the cap).
pub fn fatigue_after(remaining: f64, ly: f64, multiplier: f64) -> f64 {
    let k = weight(ly, multiplier);
    let low = k * FATIGUE_SECONDS_PER_DISTANCE;
    low.max((remaining.max(0.0) * k).min(FATIGUE_MAX_SECONDS))
        .trunc()
}

/// The reactivation timer, in game seconds, for the same jump:
/// `max(max(60, remaining * FATIGUE_RATIO), k * 60)`, whole seconds.
pub fn activation_after(remaining: f64, ly: f64, multiplier: f64) -> f64 {
    let k = weight(ly, multiplier);
    let minimum = ADDITIONAL_DISTANCE * ACTIVATION_SECONDS_PER_DISTANCE;
    let from_fatigue = (remaining.max(0.0) * FATIGUE_RATIO).max(minimum);
    from_fatigue
        .max(k * ACTIVATION_SECONDS_PER_DISTANCE)
        .trunc()
}

/// The wall length of `game_seconds` of game time, in FILETIME ticks.
fn wall_ticks(game_seconds: f64, clock: GameClock) -> i64 {
    clock.span((game_seconds * SEC as f64) as i64)
}

/// The timers after a jump of `ly` light years at `now`, from `prev`.
/// Pure: the clock is a parameter, nothing is stored.
pub fn after_jump(
    prev: &Timers,
    now: BlueTime,
    ly: f64,
    multiplier: f64,
    clock: GameClock,
) -> Timers {
    let remaining = prev.fatigue_seconds(now, clock);
    Timers {
        activation_until: now.saturating_add(wall_ticks(
            activation_after(remaining, ly, multiplier),
            clock,
        )),
        fatigue_until: now
            .saturating_add(wall_ticks(fatigue_after(remaining, ly, multiplier), clock)),
        last_updated: now,
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// `character`'s stored timers, as stored (possibly over).
pub fn stored(reg: &Registry, character: i64) -> Timers {
    let row = reg.deltas().of(KIND).get(&vec![character.to_string()]);
    let Some(v) = row else {
        return Timers::default();
    };
    let n = |i: usize| v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
    Timers {
        activation_until: n(0),
        fatigue_until: n(1),
        last_updated: n(2),
    }
}

fn store(reg: &mut Registry, character: i64, t: &Timers, now: BlueTime) {
    let key = vec![character.to_string()];
    if t.is_expired(now) {
        if reg.deltas().of(KIND).contains_key(&key) {
            reg.deltas_mut().del(KIND, &key);
        }
        return;
    }
    reg.deltas_mut().put(
        KIND,
        key,
        vec![
            t.activation_until.to_string(),
            t.fatigue_until.to_string(),
            t.last_updated.to_string(),
        ],
    );
}

/// `jumpTimers.GetTimers` for `character` at `now`: the timers, zeros when
/// over. An over row is dropped on the way.
pub fn timers(reg: &mut Registry, character: i64, now: BlueTime) -> Timers {
    let t = stored(reg, character);
    if t.is_expired(now) {
        store(reg, character, &t, now);
    }
    t.at(now)
}

/// How long the activation timer still runs, in wall FILETIME ticks, or
/// `None` when the drive may be used. For CAP-E's `JumpActivationTimer`
/// refusal.
pub fn activation_remaining(reg: &Registry, character: i64, now: BlueTime) -> Option<i64> {
    let left = stored(reg, character).activation_until - now;
    (left > 0).then_some(left)
}

/// The jump-fatigue multiplier of `character`'s ship (attribute 1971; the
/// extract's default when the hull has none or is unknown).
pub fn ship_multiplier(reg: &Registry, character: i64) -> f64 {
    let Some(c) = reg.character(character) else {
        return DEFAULT_MULTIPLIER;
    };
    crate::typedata::attribute_in(reg.content(), c.ship_type_id, ATTR_FATIGUE_MULTIPLIER)
        .unwrap_or(DEFAULT_MULTIPLIER)
}

/// **A jump of `ly` light years has happened** to `character`: set both
/// timers from their fatigue at `now`, store them on the game clock, and
/// tell the pilot. Returns the new timers. CAP-E calls this once per
/// completed jump (and CAP-G for each passenger of a bridge).
pub fn on_jump(reg: &mut Registry, character: i64, ly: f64) -> Timers {
    let m = ship_multiplier(reg, character);
    on_jump_at(reg, character, ly, m, crate::pyutil::filetime_now())
}

/// [`on_jump`] with the multiplier and the time given.
pub fn on_jump_at(
    reg: &mut Registry,
    character: i64,
    ly: f64,
    multiplier: f64,
    now: BlueTime,
) -> Timers {
    let clock = crate::game_clock::of(reg);
    let next = after_jump(&stored(reg, character), now, ly, multiplier, clock);
    store(reg, character, &next, now);
    push(reg, character, &next, true);
    next
}

/// Clear both timers and tell the pilot (`/jumpfatigue 0`).
pub fn clear(reg: &mut Registry, character: i64) {
    let now = crate::pyutil::filetime_now();
    store(reg, character, &Timers::default(), now);
    push(reg, character, &Timers::default(), false);
}

fn push(reg: &mut Registry, character: i64, t: &Timers, alert: bool) {
    reg.schedule(
        Duration::ZERO,
        OnJumpTimersUpdated {
            character,
            timers: *t,
            alert,
        },
    );
}

/// `OnJumpTimersUpdated(jumpActivation, jumpFatigue, lastUpdated, doAlert)`
/// **[V]** (`crimewatchTimers.py:449`; `crimewatchSvc.OnJumpTimersUpdated`
/// stores the arguments as `GetJumpTimers`' cache). One pilot.
#[derive(Debug, Clone, PartialEq)]
pub struct OnJumpTimersUpdated {
    pub character: i64,
    pub timers: Timers,
    /// Not sent (see `to_push`): the wire has no room for `doAlert`.
    pub alert: bool,
}

impl Push for OnJumpTimersUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        // **Three values, never four.** `crimewatchSvc.OnJumpTimersUpdated(*args)`
        // caches the notification's arguments as `GetJumpTimers`' answer, and
        // `TimerContainer.OnCombatTimersUpdated` (run when the HUD is built,
        // e.g. after a jump's session change) splats that cache into
        // `OnJumpTimersUpdated(doAlert=False, *cache)`: a fourth value fills
        // `doAlert` positionally and the call dies with "got multiple values
        // for keyword argument 'doAlert'" (`crimewatchTimers.py:349`, fci_jump_drive
        // traceback). The timers' `doAlert` therefore stays at its default.
        ("OnJumpTimersUpdated", self.timers.wire())
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// `/jumpfatigue 0 [characterID]`: clear a pilot's timers. Only `0` is
/// understood.
pub fn slash(
    reg: &mut Registry,
    actor: Option<i64>,
    words: &[&str],
) -> Result<PyValue, ServiceError> {
    let say = |s: &str| Ok(crate::pyutil::bstr(s));
    let who = match words {
        ["0"] => actor,
        ["0", c] => c.parse::<i64>().ok(),
        _ => return say("usage: /jumpfatigue 0 [characterID]"),
    };
    let Some(who) = who.filter(|id| reg.character(*id).is_some()) else {
        return say("no such character");
    };
    clear(reg, who);
    say("jump timers cleared")
}

#[cfg(test)]
mod tests;
