//! **What the world is doing, in greppable text** -- W12 packet CP15
//! (`docs/content/plan.md` §6.2, CA13; C7 §5.4 and §7; V3 R3 and R5).
//!
//! Three outputs, all rendered under the world lock the clock already holds
//! (`push::spawn_world_ticker`) and written after it is released:
//!
//! - the **`[turns]` line**: payload turns per window, how long each held
//!   the world lock and how long it waited for it, the saves, and the
//!   slowest `service.method`. Recorded by `dispatch.rs` into this module's
//!   world slot ([`TurnStats`]) **under the lock that turn already holds**,
//!   so there is no new global mutex (V3 R3) and no per-thread plumbing.
//! - the **`[live]` line** (C7 §7.1): the day, the tiers, pilots, NPCs,
//!   rocks and deltas.
//! - the **status file** (`--status FILE`, `docs/content/schema.md` §5):
//!   one TSV row per warm or hot system, rewritten with tmp + rename and no
//!   fsync, because it is disposable.
//!
//! **The `[turns]` format** is `key=value` pairs, one line, and
//! `tools/soak.sh` reads `hold_max=<ms>` from it (`docs/lanes/ip7.md`):
//!
//! ```text
//! [turns] secs=10.0 turns=120 turns_per_s=12.0 hold_mean=0.8ms hold_max=12.5ms wait_mean=0.1ms wait_max=3.0ms saves=4 saves_per_s=0.4 save_max=10.5ms slowest=char.GetFoo slowest_ms=12.5ms labels=beyonce.MachoBindObject:40:55.0ms,marketProxy.GetOrders:12:3.2ms
//! ```
//!
//! `labels=` (E1, R-ECON §1.1 B1) is `name:count:hold max` for every
//! watched call seen in the window ([`WATCHED`]), in name order, or `-`.
//! It never contains `hold_max=`, so soak.sh's greedy `sed` still finds the
//! turn's own field.
//!
//! **The `[turn-tail]` and `[turn-slow]` lines** (TT-A) say where a slow
//! payload turn's hold went: the arm, the push drain (`take`, `fanout`), the
//! pushes found waiting at acquire (`left`), the time spent in the log
//! channel (`log`, [`crate::logline`]) and, with `--turn-sched`, the thread's
//! CPU, run-queue wait and the rest (`blocked`). `[turns]` is unchanged,
//! byte for byte. `worst` is printed every window, whatever its hold;
//! [`SLOW_TURN`] and over are kept, at most [`SLOW_KEEP`].
//!
//! ```text
//! [turn-tail] secs=10.0 turns_left=3 left_max=41 queued_max=120 take_max=1.2ms fanout_max=3.4ms frames_max=380 posted=5123 bytes=2.1MB log_ms=4.1 log_max=0.6ms log_lines=2416 clock_log_ms=0.2 clock_log_max=0.1ms clock_log_lines=30 clock_left=0 sched=off slow=2 slow_over=0
//! [turn-slow] worst label=bound.GetInventoryFromId hold=231.0ms wait=0.1ms arm=0.4ms take=0.0ms fanout=0.0ms log=0.0ms/1 left=0 queued=0 pushes=0 frames=1+0 bytes=180 sc=0 cpu=0.5ms runq=229.9ms blocked=0.6ms
//! ```
//!
//! **The `[save]` line** (E1, E1b) splits every save the window measured
//! into the clone under the lock (`snapshot`), the render under the lock
//! (`render`) and the write (`write`: tmp, fsync, rename), with the rendered
//! size, so the debug and release cost per KB is measured rather than
//! derived from two runs (R-ECON §1.1). It is the only `[save]` line: since
//! CP52 the write happens on the `world-saver` thread, off the lock, so the
//! clock records the caller's half with each save ([`record_save_cost`]) and
//! the saver's writes once a window ([`record_save_writes`]):
//!
//! ```text
//! [save] secs=10.0 saves=2 split=2 bytes_mean=70123 bytes_max=70200 snapshot_mean=0.4ms snapshot_max=0.5ms render_mean=1.2ms render_max=1.5ms writes=2 write_mean=10.1ms write_max=11.0ms coalesced=0 gen=14 render_us_per_kb=17.5 write_us_per_kb=147.5
//! ```
//!
//! `saves` counts every save handed to the store; `split` the ones that came
//! with a [`SaveCost`] (a failed save has none); the snapshot and render
//! means are over `split`. `writes` counts the texts that reached the disk
//! in the window, `coalesced` the texts the saver skipped because a newer
//! one was queued, and `gen` the newest generation written (0: none). The
//! write mean and `write_us_per_kb` are over `writes` and the bytes they
//! wrote. `[turns]`' `save_max` is the time a save held the lock: snapshot
//! plus render, plus the write only for a store that writes on the caller.
//!
//! **Not yet measured**, because nothing models them: sites (CP17's
//! tracker), a bot marker (CP40: every pilot counts as a human until then),
//! the materialise time (`gen max`, C7 L4) and the static rock count behind
//! `rocks_depleted_pct`. Each is written as zero, never guessed.

use crate::content::Content;
use crate::damage::BlueTime;
use crate::liveness::{Liveness, Reasons, Tier};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::time::Duration;

/// The world slot holding this window's [`TurnStats`]. Registered in
/// `world_ext::SLOTS` by the coordinator.
pub const SLOT: SlotSpec = SlotSpec {
    name: "live_status",
    make,
};

fn make(_: &Content) -> Boxed {
    Box::new(TurnSlot::default())
}

/// The world slot's type: this window's [`TurnStats`] behind a **leaf mutex**
/// (PA-12a), so a payload turn served under `world.read()` can record itself
/// through `&Registry`. Lock order: world (read or write), then this, then
/// nothing. The `&mut Registry` recorders go through `get_mut` and never lock.
#[derive(Debug, Default)]
pub struct TurnSlot(std::sync::Mutex<TurnStats>);

impl TurnSlot {
    fn stats(&mut self) -> &mut TurnStats {
        self.0.get_mut().unwrap_or_else(|e| e.into_inner())
    }
}

/// The status file's header, byte for byte `docs/content/schema.md` §5.
pub const STATUS_HEADER: &str = "#system\ttier\thumans\tbots\tnpcs\tsites_live\tsites_cleared\trocks\trocks_depleted_pct\tlast_touch_utc\treasons";

/// FILETIME ticks (100 ns) per second and per day [V: the blue clock].
const BLUE_PER_SECOND: i64 = 10_000_000;
const BLUE_PER_DAY: i64 = 86_400 * BLUE_PER_SECOND;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mean(total: Duration, n: u64) -> Duration {
    match u32::try_from(n) {
        Ok(n) if n > 0 => total / n,
        _ => Duration::ZERO,
    }
}

/// The calls whose holds `[turns]` reports one by one (E1, R-ECON §1.1 B1):
/// the economy's services and the bind behind the burst waits. An entry
/// ending in `.` is a service prefix; any other is one exact label.
/// `market` with no dot covers `marketProxy` and any later market service.
pub const WATCHED: &[&str] = &["market", "industryManager.", "beyonce.MachoBindObject"];

fn watched(label: &str) -> bool {
    WATCHED.iter().any(|w| {
        if w.ends_with('.') || !w.contains('.') {
            label.starts_with(w)
        } else {
            label == *w
        }
    })
}

/// One world save's half on the caller's thread, under the world lock
/// (E1). `snapshot` is timed by the caller around `Registry::to_snapshot`;
/// `render`, `write` and `bytes` by the store (`Store::save_timed`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveCost {
    /// The world cloned into a `Snapshot`, under the lock.
    pub snapshot: Duration,
    /// The snapshot rendered to the world file's text.
    pub render: Duration,
    /// The text written on the caller's thread: tmp, fsync, rename,
    /// directory fsync. Zero when a saver writes it off the lock (CP52):
    /// that write arrives later as [`SaveWrites`].
    pub write: Duration,
    /// The rendered size in bytes.
    pub bytes: u64,
}

impl SaveCost {
    /// The time the save held the caller, as `[turns]`' `save_max` counts
    /// it.
    pub fn total(&self) -> Duration {
        self.snapshot + self.render + self.write
    }
}

/// The writes a saver finished off the world lock since it was last asked
/// (E1b, CP52's `world-saver`). Accumulated by that thread; taken by the
/// clock once a report window with `Store::take_writes`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveWrites {
    /// Texts written (a failed write is logged and not counted).
    pub writes: u64,
    /// Their rendered size, summed.
    pub bytes: u64,
    pub total: Duration,
    pub max: Duration,
    /// Texts skipped because a newer one was already queued.
    pub coalesced: u64,
    /// The newest generation written; 0 if none.
    pub generation: u64,
}

impl SaveWrites {
    /// Count one finished write of `bytes`, `skipped` older texts dropped
    /// for it.
    pub fn observe(&mut self, bytes: u64, took: Duration, skipped: u64, generation: u64) {
        self.writes += 1;
        self.bytes += bytes;
        self.total += took;
        self.max = self.max.max(took);
        self.coalesced += skipped;
        self.generation = self.generation.max(generation);
    }

    /// Fold `other` (a later batch) into this one.
    pub fn merge(&mut self, other: &SaveWrites) {
        self.writes += other.writes;
        self.bytes += other.bytes;
        self.total += other.total;
        self.max = self.max.max(other.max);
        self.coalesced += other.coalesced;
        self.generation = self.generation.max(other.generation);
    }
}

/// One window's split saves. Counters only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveStats {
    /// Saves recorded with a [`SaveCost`].
    pub split: u64,
    pub bytes_total: u64,
    pub bytes_max: u64,
    pub snapshot_total: Duration,
    pub snapshot_max: Duration,
    pub render_total: Duration,
    pub render_max: Duration,
    /// Every write in the window: on the caller's thread (a [`SaveCost`]
    /// with a write) or off it ([`SaveWrites`]).
    pub written: SaveWrites,
}

impl SaveStats {
    fn observe(&mut self, c: &SaveCost) {
        self.split += 1;
        self.bytes_total += c.bytes;
        self.bytes_max = self.bytes_max.max(c.bytes);
        self.snapshot_total += c.snapshot;
        self.snapshot_max = self.snapshot_max.max(c.snapshot);
        self.render_total += c.render;
        self.render_max = self.render_max.max(c.render);
        // A store that writes on the caller reports its write here; a
        // saver reports zero and its write later, through `written`.
        if c.write > Duration::ZERO {
            self.written.observe(c.bytes, c.write, 0, 0);
        }
    }
}

/// One watched label's turns in a window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LabelHold {
    pub count: u64,
    pub max: Duration,
}

/// Microseconds per KB of `bytes`, 0 when nothing was written.
fn us_per_kb(total: Duration, bytes: u64) -> f64 {
    if bytes == 0 {
        0.0
    } else {
        total.as_secs_f64() * 1e6 / (bytes as f64 / 1024.0)
    }
}

/// One payload turn, as `dispatch.rs` saw it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Turn<'a> {
    /// From asking for the world write lock to getting it.
    pub wait: Duration,
    /// From getting the lock to the end of the turn's work, its save
    /// included: the time every other thread was shut out.
    pub hold: Duration,
    /// The save's time (snapshot plus store write), if the turn saved and
    /// did not record it split through [`record_save_cost`].
    pub save: Option<Duration>,
    /// `service.method`, when the payload was a call.
    pub label: Option<&'a str>,
    /// What the turn did after its arm (TT-A); `None` where not traced.
    pub tail: Option<&'a TurnTail>,
    /// Served under `world.read()` (PA-12a). Its wait and hold overlap other
    /// turns', so it is counted in `ro_turns` and `ro_hold_total` and **not**
    /// in `turns`/`hold_*`/`wait_*`, which stay the exclusive occupancy.
    pub read: bool,
    /// A read-eligible call that fell back to the write path (queued output,
    /// a refused arm, a poisoned lock); this is the write turn that served it.
    pub fallback: bool,
}

/// One window's payload turns and saves. Counters only, as `TickHealth`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnStats {
    pub turns: u64,
    pub hold_total: Duration,
    pub hold_max: Duration,
    pub wait_total: Duration,
    pub wait_max: Duration,
    /// Every save in the window: the turns' and the clock's.
    pub saves: u64,
    pub save_max: Duration,
    /// The call that held the lock longest, and for how long.
    pub slowest: Option<(String, Duration)>,
    /// The saves that came split (E1).
    pub save: SaveStats,
    /// Per [`WATCHED`] label: its turns and its longest hold (E1).
    pub labels: BTreeMap<String, LabelHold>,
    /// The turn tails (TT-A): `[turn-tail]` and `[turn-slow]`.
    pub tail: TailStats,
    /// Turns served under `world.read()` and their summed holds (PA-12a).
    pub ro_turns: u64,
    pub ro_hold_total: Duration,
    /// Read-eligible calls served by the write path instead.
    pub ro_fallbacks: u64,
}

impl TurnStats {
    /// Count one turn. The label is copied only when it is the new slowest.
    pub fn observe(&mut self, t: &Turn<'_>) {
        if t.read {
            self.ro_turns += 1;
            self.ro_hold_total += t.hold;
            return;
        }
        self.ro_fallbacks += u64::from(t.fallback);
        self.turns += 1;
        self.hold_total += t.hold;
        self.hold_max = self.hold_max.max(t.hold);
        self.wait_total += t.wait;
        self.wait_max = self.wait_max.max(t.wait);
        if let Some(save) = t.save {
            self.observe_save(save);
        }
        if let Some(label) = t.label
            && self.slowest.as_ref().is_none_or(|(_, d)| t.hold > *d)
        {
            self.slowest = Some((label.to_owned(), t.hold));
        }
        if let Some(tail) = t.tail {
            self.tail.observe(t, tail);
        }
        if let Some(label) = t.label
            && watched(label)
        {
            // Copied once per label per window, not once per turn.
            let held = match self.labels.get_mut(label) {
                Some(h) => h,
                None => self.labels.entry(label.to_owned()).or_default(),
            };
            held.count += 1;
            held.max = held.max.max(t.hold);
        }
    }

    /// Count one save, a turn's or the clock's.
    pub fn observe_save(&mut self, took: Duration) {
        self.saves += 1;
        self.save_max = self.save_max.max(took);
    }

    /// Count one save with its split: a save in `saves` and `save_max` (by
    /// its total) and in the `[save]` line.
    pub fn observe_save_cost(&mut self, cost: &SaveCost) {
        self.observe_save(cost.total());
        self.save.observe(cost);
    }

    /// Count the writes a saver finished off the lock (E1b).
    pub fn observe_save_writes(&mut self, writes: &SaveWrites) {
        self.save.written.merge(writes);
    }

    /// The `[save]` line for a window `window` long; see the module header.
    pub fn save_report(&self, window: Duration) -> String {
        let s = &self.save;
        let w = &s.written;
        let bytes_mean = s.bytes_total.checked_div(s.split).unwrap_or(0);
        format!(
            "[save] secs={:.1} saves={} split={} bytes_mean={bytes_mean} bytes_max={} \
snapshot_mean={:.1}ms snapshot_max={:.1}ms render_mean={:.1}ms render_max={:.1}ms \
writes={} write_mean={:.1}ms write_max={:.1}ms coalesced={} gen={} \
render_us_per_kb={:.1} write_us_per_kb={:.1}",
            window.as_secs_f64(),
            self.saves,
            s.split,
            s.bytes_max,
            ms(mean(s.snapshot_total, s.split)),
            ms(s.snapshot_max),
            ms(mean(s.render_total, s.split)),
            ms(s.render_max),
            w.writes,
            ms(mean(w.total, w.writes)),
            ms(w.max),
            w.coalesced,
            w.generation,
            us_per_kb(s.render_total, s.bytes_total),
            us_per_kb(w.total, w.bytes),
        )
    }

    /// `labels=`' value: `name:count:max` per watched label, or `-`.
    fn labels_field(&self) -> String {
        if self.labels.is_empty() {
            return "-".to_owned();
        }
        let mut out = String::new();
        for (name, h) in &self.labels {
            if !out.is_empty() {
                out.push(',');
            }
            let _ = write!(out, "{name}:{}:{:.1}ms", h.count, ms(h.max));
        }
        out
    }

    /// The `[turns]` line for a window `window` long. See the module header
    /// for the shape; `hold_max=<ms>` is `tools/soak.sh`'s contract.
    pub fn report(&self, window: Duration) -> String {
        let secs = window.as_secs_f64();
        let rate = |n: u64| if secs > 0.0 { n as f64 / secs } else { 0.0 };
        let (slowest, slowest_ms) = match &self.slowest {
            Some((name, d)) => (name.as_str(), ms(*d)),
            None => ("-", 0.0),
        };
        format!(
            "[turns] secs={secs:.1} turns={} turns_per_s={:.1} hold_mean={:.1}ms \
hold_max={:.1}ms wait_mean={:.1}ms wait_max={:.1}ms saves={} saves_per_s={:.1} \
save_max={:.1}ms slowest={slowest} slowest_ms={slowest_ms:.1}ms labels={} \
ro_turns={} ro_hold_mean={:.2}ms ro_fallbacks={}",
            self.turns,
            rate(self.turns),
            ms(mean(self.hold_total, self.turns)),
            ms(self.hold_max),
            ms(mean(self.wait_total, self.turns)),
            ms(self.wait_max),
            self.saves,
            rate(self.saves),
            ms(self.save_max),
            self.labels_field(),
            self.ro_turns,
            ms(mean(self.ro_hold_total, self.ro_turns)),
            self.ro_fallbacks,
        )
    }
}

/// What a payload turn did after its arm: the session-change drain, the
/// push drain and the fan-out (TT-A). Built by `dispatch.rs` on the stack,
/// lent to [`Turn`], and folded into [`TailStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TurnTail {
    /// `pending_len()` right after the lock was acquired: pushes queued
    /// outside any payload turn.
    pub left: u32,
    /// `pending_len()` after the arm, before the drain.
    pub queued: u32,
    /// `handle_inbound`: the arm, its session-change drain and its log lines.
    pub arm: Duration,
    /// `Registry::take_pending`, `grid::filter` included.
    pub take: Duration,
    /// The rest of `pending_pushes`: `to_push`, the pickle, the
    /// per-recipient marshal and the posts.
    pub fanout: Duration,
    /// Pushes left after the grid filter.
    pub pushes: u32,
    /// Frames into this turn's own reply.
    pub frames_inline: u32,
    /// `Writers::post` calls (pushes and session changes to other
    /// connections).
    pub frames_posted: u32,
    /// Bytes of all those frames.
    pub bytes: u64,
    /// Session changes this turn built, its own and posted.
    pub sc: u32,
    /// Logging time on this thread during the hold. Inside `arm`, `take`
    /// and `fanout`, not added to them.
    pub log: Duration,
    pub log_lines: u32,
    /// Only with `--turn-sched`.
    pub sched: Option<Sched>,
    /// Time in `Registry::cancel_scheduled` (the scheduled-queue scan)
    /// during the hold; inside `arm`.
    pub cancel: Duration,
}

/// A turn's thread accounting; `blocked` is `hold - cpu - runq`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sched {
    pub cpu: Duration,
    pub runq: Duration,
}

/// A turn that held the lock for [`SLOW_TURN`] or more, or the window's
/// longest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SlowTurn {
    pub label: String,
    pub wait: Duration,
    pub hold: Duration,
    pub tail: TurnTail,
}

/// [A] 20 ms. Falsifier: more than [`SLOW_KEEP`] every window at idle, or
/// none in a run whose `[turns]` `hold_max` is over 50 ms.
pub const SLOW_TURN: Duration = Duration::from_millis(20);
/// Slow turns kept per window; the rest are counted in `slow_over`.
pub const SLOW_KEEP: usize = 8;

/// One window's turn tails, for the `[turn-tail]` and `[turn-slow]` lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TailStats {
    /// Turns that found pushes waiting when they got the lock.
    pub turns_left: u64,
    pub left_max: u32,
    pub queued_max: u32,
    pub take_max: Duration,
    pub fanout_max: Duration,
    pub frames_max: u32,
    pub posted: u64,
    pub bytes: u64,
    pub log_total: Duration,
    pub log_max: Duration,
    pub log_lines: u64,
    pub sched_seen: bool,
    pub cancel_max: Duration,
    pub cancel_total: Duration,
    pub runq_max: Duration,
    pub blocked_max: Duration,
    /// The clock's own logging, summed per tick.
    pub clock_log_total: Duration,
    pub clock_log_max: Duration,
    pub clock_log_lines: u64,
    /// Pushes the clock left in `pending`.
    pub clock_left: u64,
    /// The window's longest hold, always kept.
    pub worst: Option<SlowTurn>,
    /// Holds of [`SLOW_TURN`] or more, at most [`SLOW_KEEP`].
    pub slow: Vec<SlowTurn>,
    pub slow_over: u32,
}

impl TailStats {
    fn observe(&mut self, t: &Turn<'_>, tail: &TurnTail) {
        if tail.left > 0 {
            self.turns_left += 1;
        }
        self.left_max = self.left_max.max(tail.left);
        self.queued_max = self.queued_max.max(tail.queued);
        self.take_max = self.take_max.max(tail.take);
        self.fanout_max = self.fanout_max.max(tail.fanout);
        self.frames_max = self
            .frames_max
            .max(tail.frames_inline.saturating_add(tail.frames_posted));
        self.posted += u64::from(tail.frames_posted);
        self.bytes += tail.bytes;
        self.log_total += tail.log;
        self.log_max = self.log_max.max(tail.log);
        self.log_lines += u64::from(tail.log_lines);
        self.cancel_max = self.cancel_max.max(tail.cancel);
        self.cancel_total += tail.cancel;
        if let Some(s) = tail.sched {
            self.sched_seen = true;
            self.runq_max = self.runq_max.max(s.runq);
            self.blocked_max = self.blocked_max.max(blocked(t.hold, &s));
        }
        let slow = t.hold >= SLOW_TURN && self.slow.len() < SLOW_KEEP;
        let worst = self.worst.as_ref().is_none_or(|w| t.hold > w.hold);
        if t.hold >= SLOW_TURN && !slow {
            self.slow_over += 1;
        }
        if slow || worst {
            let turn = SlowTurn {
                label: t.label.unwrap_or("-").to_owned(),
                wait: t.wait,
                hold: t.hold,
                tail: *tail,
            };
            if worst {
                self.worst = Some(turn.clone());
            }
            if slow {
                self.slow.push(turn);
            }
        }
    }
}

/// `hold - cpu - runq`, saturating at zero.
fn blocked(hold: Duration, s: &Sched) -> Duration {
    hold.saturating_sub(s.cpu).saturating_sub(s.runq)
}

impl SlowTurn {
    /// One `[turn-slow]` line; `head` is `worst` or empty.
    fn line(&self, head: &str) -> String {
        let t = &self.tail;
        let sched = match &t.sched {
            Some(s) => format!(
                "cpu={:.1}ms runq={:.1}ms blocked={:.1}ms",
                ms(s.cpu),
                ms(s.runq),
                ms(blocked(self.hold, s))
            ),
            None => "cpu=- runq=- blocked=-".to_owned(),
        };
        format!(
            "[turn-slow] {head}label={} hold={:.1}ms wait={:.1}ms arm={:.1}ms take={:.1}ms \
fanout={:.1}ms log={:.1}ms/{} left={} queued={} pushes={} frames={}+{} bytes={} sc={} cancel_us={:.0} {sched}",
            self.label,
            ms(self.hold),
            ms(self.wait),
            ms(t.arm),
            ms(t.take),
            ms(t.fanout),
            ms(t.log),
            t.log_lines,
            t.left,
            t.queued,
            t.pushes,
            t.frames_inline,
            t.frames_posted,
            t.bytes,
            t.sc,
            t.cancel.as_secs_f64() * 1e6,
        )
    }
}

impl TurnStats {
    /// The `[turn-tail]` line for a window `window` long; see the module
    /// header. `[turns]` is not touched by any of this.
    pub fn tail_report(&self, window: Duration) -> String {
        let t = &self.tail;
        let sched = if t.sched_seen {
            format!(
                "sched=on runq_max={:.1}ms blocked_max={:.1}ms",
                ms(t.runq_max),
                ms(t.blocked_max)
            )
        } else {
            "sched=off".to_owned()
        };
        format!(
            "[turn-tail] secs={:.1} turns_left={} left_max={} queued_max={} take_max={:.1}ms \
fanout_max={:.1}ms frames_max={} posted={} bytes={:.1}MB log_ms={:.1} log_max={:.1}ms \
log_lines={} clock_log_ms={:.1} clock_log_max={:.1}ms clock_log_lines={} clock_left={} {sched} \
slow={} slow_over={} cancel_ms={:.1} cancel_max={:.1}ms",
            window.as_secs_f64(),
            t.turns_left,
            t.left_max,
            t.queued_max,
            ms(t.take_max),
            ms(t.fanout_max),
            t.frames_max,
            t.posted,
            t.bytes as f64 / (1024.0 * 1024.0),
            ms(t.log_total),
            ms(t.log_max),
            t.log_lines,
            ms(t.clock_log_total),
            ms(t.clock_log_max),
            t.clock_log_lines,
            t.clock_left,
            t.slow.len(),
            t.slow_over,
            ms(t.cancel_total),
            ms(t.cancel_max),
        )
    }

    /// The window's `[turn-slow]` lines: the worst turn, then every kept
    /// slow one. Empty when no turn carried a tail.
    pub fn slow_lines(&self) -> Vec<String> {
        let t = &self.tail;
        let mut out = Vec::new();
        if let Some(w) = &t.worst {
            out.push(w.line("worst "));
        }
        out.extend(t.slow.iter().map(|s| s.line("")));
        out
    }
}

/// Record the clock's logging for one tick and the pushes it left queued.
/// `dispatch.rs` has the payload half; this is `push::spawn_world_ticker`'s.
pub fn record_clock_log(w: &mut Registry, stats: crate::logline::LogStats, clock_left: u32) {
    let t = &mut w.ext_mut::<TurnSlot>().stats().tail;
    let took = Duration::from_nanos(stats.ns);
    t.clock_log_total += took;
    t.clock_log_max = t.clock_log_max.max(took);
    t.clock_log_lines += u64::from(stats.lines);
    t.clock_left += u64::from(clock_left);
}

/// Record one payload turn. `dispatch.rs` calls this under the world lock
/// the turn holds (read or write), as its last act before releasing it. The
/// slot's own leaf mutex is what makes `&Registry` enough.
pub fn record_turn(w: &Registry, turn: Turn<'_>) {
    w.ext::<TurnSlot>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(&turn);
}

/// A copy of this window's numbers, leaving them in place (`/live`).
pub fn peek_turns(w: &Registry) -> TurnStats {
    w.ext::<TurnSlot>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Record a save the clock made (`push::spawn_world_ticker`).
pub fn record_save(w: &mut Registry, took: Duration) {
    w.ext_mut::<TurnSlot>().stats().observe_save(took);
}

/// Record a save's caller half with its split (E1): the clock's (since
/// CP52 a payload turn does not save).
pub fn record_save_cost(w: &mut Registry, cost: SaveCost) {
    w.ext_mut::<TurnSlot>().stats().observe_save_cost(&cost);
}

/// Record the writes a saver finished off the lock (E1b): the clock takes
/// them from the store once a report window, under the lock it holds.
pub fn record_save_writes(w: &mut Registry, writes: SaveWrites) {
    w.ext_mut::<TurnSlot>().stats().observe_save_writes(&writes);
}

/// This window's numbers, leaving an empty window behind.
pub fn take_turns(w: &mut Registry) -> TurnStats {
    std::mem::take(w.ext_mut::<TurnSlot>().stats())
}

/// `YYYY-MM-DDTHH:MM:SSZ` for a blue FILETIME.
pub fn iso_utc(ft: BlueTime) -> String {
    let (y, m, d) = crate::ledger::civil_from_filetime(ft);
    let secs = ft.rem_euclid(BLUE_PER_DAY) / BLUE_PER_SECOND;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The reasons as `list<str>`, in bit order.
fn reason_names(r: Reasons) -> String {
    [
        (Reasons::PILOT, "inspace"),
        (Reasons::BALLS, "balls"),
        (Reasons::DOCKED, "docked"),
        (Reasons::JUMP, "jump"),
        (Reasons::NEIGHBOUR, "neighbour"),
    ]
    .iter()
    .filter(|(bit, _)| r.contains(*bit))
    .map(|(_, name)| *name)
    .collect::<Vec<_>>()
    .join(",")
}

/// Per system: (pilots in space, pilots docked online). Read the way
/// `liveness::refresh` reads them, so a row agrees with its tier.
fn pilots_by_system(w: &Registry) -> BTreeMap<i64, (i64, i64)> {
    let mut out: BTreeMap<i64, (i64, i64)> = BTreeMap::new();
    let roster = w.roster();
    for (system, ids) in roster.systems() {
        let n = ids.iter().filter(|id| w.character(**id).is_some()).count();
        if n > 0 {
            out.entry(system).or_default().0 += n as i64;
        }
    }
    for c in w.sessions().online_characters() {
        if roster.is_in_space(c) || w.jumps().contains(c) {
            continue;
        }
        if let Some(system) = w.character(c).and_then(|c| c.solar_system_id) {
            out.entry(system).or_default().1 += 1;
        }
    }
    out
}

/// The status file's body: [`STATUS_HEADER`] and one row per warm or hot
/// system, in id order. Rendered under the lock; written after it.
pub fn status_tsv(w: &Registry) -> String {
    let mut npcs: BTreeMap<i64, i64> = BTreeMap::new();
    for npc in w.npcs().iter() {
        *npcs.entry(npc.solar_system_id).or_default() += 1;
    }
    let mut out = String::from(STATUS_HEADER);
    out.push('\n');
    for (system, live) in w.ext::<Liveness>().iter() {
        let tier = match live.tier {
            Tier::Hot => "hot",
            Tier::Warm | Tier::Cold => "warm",
        };
        let rocks = w.rocks().in_system(system).count();
        // humans, bots: liveness counts them by `Character::is_bot` (CP40).
        // sites_live, sites_cleared, rocks_depleted_pct: not modelled yet.
        let _ = writeln!(
            out,
            "{system}\t{tier}\t{}\t{}\t{}\t0\t0\t{rocks}\t0.0\t{}\t{}",
            live.humans,
            live.bots,
            npcs.get(&system).copied().unwrap_or(0),
            iso_utc(live.last_reason),
            reason_names(live.reasons),
        );
    }
    out
}

/// Write the status file: a sibling tmp file, then a rename, so a reader
/// never sees half a file. No fsync: the file is rewritten every report.
pub fn write_status(path: &Path, body: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// Writes the status file each window and reports a failure once: a
/// read-only directory would otherwise log the same error every 10 s.
/// A later success re-arms the warning.
#[derive(Default)]
pub struct StatusWriter {
    warned: bool,
}

impl StatusWriter {
    /// Write the file; `Some(error)` only for the first failure in a row.
    pub fn write(&mut self, path: &Path, body: &str) -> Option<io::Error> {
        match write_status(path, body) {
            Ok(()) => {
                self.warned = false;
                None
            }
            Err(e) if !self.warned => {
                self.warned = true;
                Some(e)
            }
            Err(_) => None,
        }
    }
}

/// `XhYm` for a span of blue ticks; negative spans read as `0h0m`.
fn hours_minutes(ticks: i64) -> String {
    let mins = ticks.max(0) / (60 * BLUE_PER_SECOND);
    format!("{}h{}m", mins / 60, mins % 60)
}

/// The `[live]` line (C7 §7.1), from the world at blue time `now` and this
/// window's turn numbers.
pub fn live_line(w: &Registry, now: BlueTime, turns: &TurnStats) -> String {
    let mut line = String::from("[live]");
    match w.deltas().world() {
        Some(world) => {
            // Lane clock: one compressed day after the last rollover at
            // a `--time-scale` above 1; the next 11:00 UTC at scale 1.
            let next =
                crate::game_clock::of(w).next_rollover(world.birth, world.last_rollover, now);
            let _ = write!(
                line,
                " day {} next-rollover {}",
                world.day,
                hours_minutes(next - now)
            );
        }
        None => line.push_str(" day - next-rollover -"),
    }
    let live = w.ext::<Liveness>();
    let hot = live.hot().count();
    let pilots = pilots_by_system(w);
    let in_space: i64 = pilots.values().map(|p| p.0).sum();
    let docked: i64 = pilots.values().map(|p| p.1).sum();
    let _ = write!(
        line,
        " | systems hot {hot} warm {} | pilots {} (in space {in_space}) | npcs {} rocks {} | deltas",
        live.len() - hot,
        in_space + docked,
        w.npcs().len(),
        w.rocks().len(),
    );
    let mut any = false;
    for (kind, rows) in w.deltas().kinds() {
        let _ = write!(line, " {kind} {}", rows.len());
        any = true;
    }
    if !any {
        line.push_str(" none");
    }
    let _ = write!(
        line,
        " | lock-wait max {:.1} ms | save max {:.1} ms",
        ms(turns.wait_max),
        ms(turns.save_max)
    );
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// The header the operator's `column -t` and CP25's `/live` read is
    /// schema.md §5's, copied from that document (schema §1.1: the test
    /// that pins a header copies the line).
    #[test]
    fn the_status_header_equals_schema_md() {
        assert_eq!(
            STATUS_HEADER,
            "#system\ttier\thumans\tbots\tnpcs\tsites_live\tsites_cleared\trocks\trocks_depleted_pct\tlast_touch_utc\treasons"
        );
        let w = Registry::new(crate::model::SandboxConfig::default());
        let body = status_tsv(&w);
        assert_eq!(
            body.lines().next(),
            Some(STATUS_HEADER),
            "line 1 is the header"
        );
    }

    /// Every row has the header's column count, and a live system appears.
    #[test]
    fn a_live_system_is_one_row_with_every_column() {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        // Undocked, so the default character's system is hot.
        let who = w.config().characters[0].character_id;
        let system = w
            .character(who)
            .and_then(|c| c.solar_system_id)
            .expect("a system");
        let ship = w.character(who).map(|c| c.ship_id).expect("a ship");
        w.roster_mut()
            .arrive(crate::space_state::Presence::arriving(
                who,
                ship,
                system,
                crate::space_state::Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        crate::liveness::refresh(&mut w, std::time::Instant::now(), 1_000 * BLUE_PER_DAY);
        let body = status_tsv(&w);
        let cols = STATUS_HEADER.split('\t').count();
        let rows: Vec<&str> = body.lines().skip(1).collect();
        assert!(!rows.is_empty(), "a hot system has a row: {body}");
        for row in &rows {
            assert_eq!(row.split('\t').count(), cols, "{row}");
        }
        let row = rows
            .iter()
            .find(|r| r.starts_with(&format!("{system}\t")))
            .expect("the pilot's system");
        let f: Vec<&str> = row.split('\t').collect();
        assert_eq!(f[1], "hot");
        assert_eq!(f[2], "1", "one pilot");
        assert!(f[10].split(',').any(|r| r == "inspace"), "{row}");
    }

    /// Wait and hold are separate numbers, and each is reported as itself.
    #[test]
    fn wait_and_hold_are_reported_separately() {
        let mut t = TurnStats::default();
        t.observe(&Turn {
            wait: d(7),
            hold: d(2),
            save: None,
            label: Some("a.Slow"),
            tail: None,
            ..Default::default()
        });
        t.observe(&Turn {
            wait: d(1),
            hold: d(4),
            save: Some(d(3)),
            label: Some("b.Slower"),
            tail: None,
            ..Default::default()
        });
        assert_eq!((t.wait_max, t.hold_max), (d(7), d(4)));
        let line = t.report(Duration::from_secs(2));
        for needle in [
            "wait_max=7.0ms",
            "hold_max=4.0ms",
            "wait_mean=4.0ms",
            "hold_mean=3.0ms",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        assert_eq!(t.slowest, Some(("b.Slower".to_owned(), d(4))));
    }

    /// The `[turns]` line has every named field, `tools/soak.sh`'s
    /// `hold_max=<ms>` among them, on one line with the prefix.
    #[test]
    fn the_turns_line_has_its_named_fields() {
        let mut t = TurnStats::default();
        t.observe(&Turn {
            wait: d(1),
            hold: d(5),
            save: Some(d(2)),
            label: Some("svc.Call"),
            tail: None,
            ..Default::default()
        });
        t.observe_save(d(9));
        let line = t.report(Duration::from_secs(1));
        assert!(line.starts_with("[turns] "), "{line}");
        assert!(!line.contains('\n'));
        for needle in [
            "secs=1.0",
            "turns=1",
            "turns_per_s=1.0",
            "hold_mean=5.0ms",
            "hold_max=5.0ms",
            "wait_mean=1.0ms",
            "wait_max=1.0ms",
            "saves=2",
            "saves_per_s=2.0",
            "save_max=9.0ms",
            "slowest=svc.Call",
            "slowest_ms=5.0ms",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        // soak.sh: `sed -n 's/.*hold_max=\([0-9.]*\).*/\1/p'`.
        let after = line.split("hold_max=").nth(1).expect("hold_max=");
        let num: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        assert_eq!(num, "5.0");
        // An empty window still prints, so a quiet server reads as quiet.
        let quiet = TurnStats::default().report(Duration::from_secs(10));
        assert!(
            quiet.contains("turns=0") && quiet.contains("slowest=-"),
            "{quiet}"
        );
    }

    /// E1: a split save reports its bytes, snapshot, render and write
    /// apart; `saves` still counts an unsplit save and `save_max` is the
    /// whole of a split one.
    #[test]
    fn a_save_reports_bytes_render_and_write_apart() {
        let mut t = TurnStats::default();
        t.observe_save_cost(&SaveCost {
            snapshot: d(1),
            render: d(2),
            write: d(10),
            bytes: 2048,
        });
        t.observe_save_cost(&SaveCost {
            snapshot: d(3),
            render: d(4),
            write: d(20),
            bytes: 4096,
        });
        t.observe_save(d(5));
        let line = t.save_report(Duration::from_secs(10));
        assert!(line.starts_with("[save] "), "{line}");
        assert!(!line.contains('\n'));
        for needle in [
            "secs=10.0",
            "saves=3",
            "split=2",
            "bytes_mean=3072",
            "bytes_max=4096",
            "snapshot_mean=2.0ms",
            "snapshot_max=3.0ms",
            "render_mean=3.0ms",
            "render_max=4.0ms",
            "writes=2",
            "write_mean=15.0ms",
            "coalesced=0",
            "gen=0",
            "write_max=20.0ms",
            // 6 ms over 6 KB, 30 ms over 6 KB.
            "render_us_per_kb=1000.0",
            "write_us_per_kb=5000.0",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        let turns = t.report(Duration::from_secs(10));
        assert!(turns.contains("save_max=27.0ms"), "{turns}");
        // A window with no split save still prints, all zero.
        let quiet = TurnStats::default().save_report(Duration::from_secs(10));
        for needle in [
            "saves=0",
            "split=0",
            "bytes_mean=0",
            "writes=0",
            "render_us_per_kb=0.0",
        ] {
            assert!(quiet.contains(needle), "{needle:?} missing from {quiet:?}");
        }
    }

    /// E1b: a saver's save (CP52) reports its caller half with the save
    /// and its write later, off the lock. `save_max` is the lock's share
    /// only; the write's mean and per-KB are over the writes that landed,
    /// not over the saves handed over (two coalesced into one here).
    #[test]
    fn a_savers_write_arrives_apart_and_stays_out_of_save_max() {
        let mut t = TurnStats::default();
        for render in [2, 4] {
            t.observe_save_cost(&SaveCost {
                snapshot: d(1),
                render: d(render),
                write: Duration::ZERO,
                bytes: 2048,
            });
        }
        let mut written = SaveWrites::default();
        written.observe(2048, d(40), 1, 2);
        t.observe_save_writes(&written);
        let line = t.save_report(Duration::from_secs(10));
        for needle in [
            "saves=2",
            "split=2",
            "render_max=4.0ms",
            "writes=1",
            "write_mean=40.0ms",
            "write_max=40.0ms",
            "coalesced=1",
            "gen=2",
            // 40 ms over the 2 KB that were written.
            "write_us_per_kb=20000.0",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        let turns = t.report(Duration::from_secs(10));
        assert!(turns.contains("save_max=5.0ms"), "{turns}");
    }

    /// E1: the watched calls get their own hold max and count; others do
    /// not, and soak.sh's greedy `hold_max=` sed still reads the turn's.
    #[test]
    fn watched_calls_get_their_own_hold_max() {
        let mut t = TurnStats::default();
        for (label, hold) in [
            ("beyonce.MachoBindObject", 9),
            ("beyonce.MachoBindObject", 30),
            ("marketProxy.GetOrders", 2),
            ("industryManager.InstallJob", 4),
            ("beyonce.CmdWarpToStuff", 50),
            ("char.GetFoo", 70),
        ] {
            t.observe(&Turn {
                hold: d(hold),
                label: Some(label),
                ..Turn::default()
            });
        }
        t.observe(&Turn {
            hold: d(1),
            ..Turn::default()
        });
        assert_eq!(
            t.labels.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "beyonce.MachoBindObject",
                "industryManager.InstallJob",
                "marketProxy.GetOrders"
            ]
        );
        assert_eq!(
            t.labels["beyonce.MachoBindObject"],
            LabelHold {
                count: 2,
                max: d(30)
            }
        );
        let line = t.report(Duration::from_secs(1));
        assert!(
            line.contains(
                " labels=beyonce.MachoBindObject:2:30.0ms,industryManager.InstallJob:1:4.0ms,marketProxy.GetOrders:1:2.0ms"
            ),
            "{line}"
        );
        // `sed -n 's/.*hold_max=\([0-9.]*\).*/\1/p'`: greedy, so the last one.
        let after = line.rsplit("hold_max=").next().expect("hold_max=");
        let num: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        assert_eq!(num, "70.0");
        assert!(
            TurnStats::default()
                .report(Duration::from_secs(1))
                .contains(" labels=- ro_turns=0 ")
        );
    }

    /// Recorded through the world slot, and taken once.
    #[test]
    fn a_recorded_turn_is_taken_once() {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        record_turn(
            &w,
            Turn {
                hold: d(1),
                ..Turn::default()
            },
        );
        record_save(&mut w, d(1));
        record_save_cost(
            &mut w,
            SaveCost {
                bytes: 10,
                ..SaveCost::default()
            },
        );
        record_save_writes(
            &mut w,
            SaveWrites {
                writes: 1,
                bytes: 10,
                ..SaveWrites::default()
            },
        );
        let got = take_turns(&mut w);
        assert_eq!(
            (
                got.turns,
                got.saves,
                got.save.split,
                got.save.written.writes
            ),
            (1, 2, 1, 1)
        );
        assert_eq!(take_turns(&mut w), TurnStats::default());
    }

    /// The `[live]` line carries C7 §7.1's sections.
    #[test]
    fn the_live_line_names_its_sections() {
        let w = Registry::new(crate::model::SandboxConfig::default());
        let line = live_line(&w, 1_000 * BLUE_PER_DAY, &TurnStats::default());
        assert!(line.starts_with("[live] day "), "{line}");
        for needle in [
            "next-rollover ",
            "| systems hot ",
            " warm ",
            "| pilots ",
            "(in space ",
            "| npcs ",
            " rocks ",
            "| deltas",
            "| lock-wait max ",
            " ms | save max ",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        assert!(!line.contains('\n'));
    }

    /// The status file is replaced whole, and its directory is made.
    #[test]
    fn the_status_file_is_replaced_whole() {
        let dir = std::env::temp_dir().join(format!(
            "eve-status-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir.join("live").join("status.tsv");
        write_status(&path, "one\n").expect("first write");
        write_status(&path, "two\n").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two\n");
        assert!(!dir.join("live").join("status.tsv.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failing status file is reported once, not every window.
    #[test]
    fn a_failing_status_file_warns_once() {
        let dir = std::env::temp_dir().join(format!("eve-status-warn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        // A regular file where the directory should be: cannot be created.
        let block = dir.join("block");
        std::fs::write(&block, "x").expect("block");
        let path = block.join("status.tsv");
        let mut w = StatusWriter::default();
        assert!(w.write(&path, "a").is_some());
        assert!(w.write(&path, "a").is_none());
        assert!(w.write(&path, "a").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `iso_utc` against the day the ledger's own test pins: 2000-02-29
    /// is day 11,016 after 1970-01-01, and 1601 to 1970 is 134,774 days.
    #[test]
    fn iso_utc_formats_a_filetime() {
        let day = (11_016 + 134_774) * BLUE_PER_DAY;
        let t = day + (3_600 + 2 * 60 + 3) * BLUE_PER_SECOND;
        assert_eq!(iso_utc(t), "2000-02-29T01:02:03Z");
    }

    fn tail_turn(hold: u64, tail: &TurnTail) -> (Duration, Duration, TurnTail) {
        (d(hold), d(1), *tail)
    }

    fn observe_tail(t: &mut TurnStats, label: &str, hold: u64, tail: TurnTail) {
        let (hold, wait, tail) = tail_turn(hold, &tail);
        t.observe(&Turn {
            wait,
            hold,
            save: None,
            label: Some(label),
            tail: Some(&tail),
            ..Default::default()
        });
    }

    /// Every field of `[turn-tail]` and `[turn-slow]` is named, with its unit.
    #[test]
    fn tail_report_names_every_field_and_unit() {
        let mut t = TurnStats::default();
        observe_tail(
            &mut t,
            "bound.GetInventoryFromId",
            231,
            TurnTail {
                left: 3,
                queued: 5,
                arm: d(1),
                take: d(2),
                fanout: d(3),
                pushes: 4,
                frames_inline: 1,
                frames_posted: 2,
                bytes: 3 * 1024 * 1024,
                sc: 6,
                log: d(7),
                log_lines: 8,
                sched: Some(Sched {
                    cpu: d(10),
                    runq: d(200),
                }),
                cancel: d(9),
            },
        );
        let line = t.tail_report(Duration::from_secs(10));
        assert!(line.starts_with("[turn-tail] "), "{line}");
        assert!(!line.contains('\n'));
        for needle in [
            "secs=10.0",
            "turns_left=1",
            "left_max=3",
            "queued_max=5",
            "take_max=2.0ms",
            "fanout_max=3.0ms",
            "frames_max=3",
            "posted=2",
            "bytes=3.0MB",
            "log_ms=7.0",
            "log_max=7.0ms",
            "log_lines=8",
            "clock_log_ms=0.0",
            "clock_log_max=0.0ms",
            "clock_left=0",
            "sched=on",
            "runq_max=200.0ms",
            "blocked_max=21.0ms",
            "slow=1",
            "slow_over=0",
            "cancel_max=9.0ms",
        ] {
            assert!(line.contains(needle), "{needle:?} missing from {line:?}");
        }
        let lines = t.slow_lines();
        assert_eq!(lines.len(), 2, "the worst, then the kept slow one");
        assert!(lines[0].starts_with("[turn-slow] worst label=bound.GetInventoryFromId "));
        assert!(lines[1].starts_with("[turn-slow] label=bound.GetInventoryFromId "));
        for needle in [
            "hold=231.0ms",
            "wait=1.0ms",
            "arm=1.0ms",
            "take=2.0ms",
            "fanout=3.0ms",
            "log=7.0ms/8",
            "left=3",
            "queued=5",
            "pushes=4",
            "frames=1+2",
            "bytes=3145728",
            "sc=6",
            "cancel_us=9000",
            "cpu=10.0ms",
            "runq=200.0ms",
            "blocked=21.0ms",
        ] {
            assert!(
                lines[0].contains(needle),
                "{needle:?} missing from {:?}",
                lines[0]
            );
        }
        // Without --turn-sched the three read `-`.
        let mut off = TurnStats::default();
        observe_tail(&mut off, "a.B", 30, TurnTail::default());
        assert!(
            off.tail_report(Duration::from_secs(1))
                .contains("sched=off ")
        );
        assert!(off.slow_lines()[0].ends_with("cpu=- runq=- blocked=-"));
    }

    /// The window's longest turn is kept even below the threshold, and the
    /// slow list is capped.
    #[test]
    fn worst_is_kept_below_the_threshold_and_slow_is_capped() {
        let mut t = TurnStats::default();
        observe_tail(&mut t, "quick.Call", 3, TurnTail::default());
        assert!(t.tail.slow.is_empty());
        assert_eq!(t.tail.worst.as_ref().map(|w| w.hold), Some(d(3)));
        for i in 0..20 {
            observe_tail(
                &mut t,
                "slow.Call",
                25 + u64::from(i == 13),
                TurnTail::default(),
            );
        }
        assert_eq!(t.tail.slow.len(), SLOW_KEEP);
        assert_eq!(t.tail.slow_over, 12);
        let worst = t.tail.worst.as_ref().expect("worst");
        assert_eq!((worst.label.as_str(), worst.hold), ("slow.Call", d(26)));
    }

    #[test]
    fn turns_left_counts_only_nonzero_left() {
        let mut t = TurnStats::default();
        for left in [0, 2, 0, 41] {
            observe_tail(
                &mut t,
                "a.B",
                1,
                TurnTail {
                    left,
                    ..TurnTail::default()
                },
            );
        }
        assert_eq!((t.tail.turns_left, t.tail.left_max), (2, 41));
    }

    /// A turn without a tail changes only `[turns]`'s own numbers, and the
    /// tail fields never reach the `[turns]` line.
    #[test]
    fn the_tail_does_not_touch_the_turns_line() {
        let mut a = TurnStats::default();
        let mut b = TurnStats::default();
        let turn = |tail| Turn {
            wait: d(1),
            hold: d(30),
            save: None,
            label: Some("x.Y"),
            tail,
            ..Default::default()
        };
        a.observe(&turn(None));
        let full = TurnTail {
            left: 9,
            log: d(5),
            ..TurnTail::default()
        };
        b.observe(&turn(Some(&full)));
        assert_eq!(
            a.report(Duration::from_secs(1)),
            b.report(Duration::from_secs(1))
        );
        assert!(!a.report(Duration::from_secs(1)).contains("turn-tail"));
    }

    #[test]
    fn clock_logging_is_summed_and_maxed() {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        let st = |ns, lines| crate::logline::LogStats {
            ns,
            lines,
            dropped: 0,
        };
        record_clock_log(&mut w, st(2_000_000, 3), 1);
        record_clock_log(&mut w, st(5_000_000, 4), 0);
        let t = take_turns(&mut w);
        assert_eq!(t.tail.clock_log_total, d(7));
        assert_eq!(t.tail.clock_log_max, d(5));
        assert_eq!((t.tail.clock_log_lines, t.tail.clock_left), (7, 1));
    }
}

#[cfg(test)]
mod read_turn_tests {
    use super::*;
    use crate::model::SandboxConfig;

    #[test]
    fn record_turn_works_through_a_shared_registry_from_two_threads() {
        let mut w = Registry::new(SandboxConfig::default());
        take_turns(&mut w);
        let world = std::sync::RwLock::new(w);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    for _ in 0..500 {
                        let w = world.read().expect("unpoisoned");
                        record_turn(
                            &w,
                            Turn {
                                hold: Duration::from_micros(10),
                                read: true,
                                ..Turn::default()
                            },
                        );
                    }
                });
            }
        });
        let mut w = world.write().expect("unpoisoned");
        record_turn(&w, Turn::default());
        record_turn(
            &w,
            Turn {
                fallback: true,
                ..Turn::default()
            },
        );
        let t = take_turns(&mut w);
        assert_eq!(
            (t.ro_turns, t.ro_hold_total),
            (1000, Duration::from_millis(10))
        );
        // Reads stay out of the exclusive occupancy numbers.
        assert_eq!((t.turns, t.ro_fallbacks), (2, 1));
    }

    #[test]
    fn the_turns_line_keeps_its_keys_and_appends_the_read_ones() {
        let line = TurnStats::default().report(Duration::from_secs(1));
        let keys: Vec<&str> = line
            .split(' ')
            .filter_map(|f| f.split('=').next().filter(|_| f.contains('=')))
            .collect();
        assert_eq!(
            keys,
            [
                "secs",
                "turns",
                "turns_per_s",
                "hold_mean",
                "hold_max",
                "wait_mean",
                "wait_max",
                "saves",
                "saves_per_s",
                "save_max",
                "slowest",
                "slowest_ms",
                "labels",
                "ro_turns",
                "ro_hold_mean",
                "ro_fallbacks"
            ]
        );
    }
}

/// **Performance profile counters** (lane pa0-prof, `perf-arch` PA-0):
/// measurement only, no behaviour. Process statics, because the writers run
/// off the world lock and the clock reports from outside it: relaxed atomics
/// for the sections, one leaf mutex (taken under the world lock by a payload
/// turn, never the other way round) for the per-method table. Printed as
/// extra `[world] tick ...` lines beside the tick-health line by
/// [`prof::report_lines`], which also resets them. Parsed by
/// `tools/bench_ramp.py` and `tools/soak_summary.py`.
pub mod prof {
    use super::ms;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::time::Duration;

    /// A timed section: how many times, total and longest, in nanoseconds
    /// (`Bytes`: `total` is bytes).
    #[derive(Clone, Copy)]
    pub enum Sec {
        /// `take_due` less `grid::filter`: the scan, the stamp, the restamp.
        DrainTake,
        /// `grid::filter` over the due pushes.
        DrainFilter,
        /// Audience to client-id resolution (`SessionTable::of_character`).
        DrainAudience,
        /// `session_changes_due`.
        SessionChanges,
        /// The clock's save, under the lock: `to_snapshot` and the hand-off.
        Save,
        /// The clock turn's whole hold of the world lock.
        ClockHeld,
        /// A payload turn's hold.
        PayloadHeld,
        /// A payload turn's wait for the lock.
        PayloadWait,
        /// One writer shard's pass over its wires (count = shard passes; max = the slowest).
        WriterPass,
        /// `compress_payload`, wherever it ran: a writer shard, or the encode phase (PA-9); only calls that deflated.
        Deflate,
        /// One `write_all` on the ticker: the longest is the blocking write.
        WriteAll,
        /// Frames the ticker wrote (`count`) and their wire bytes (`total`).
        Bytes,
        /// A payload turn served under `world.read()`: its hold. Overlaps
        /// other turns, so it is **not** part of any occupancy percentage.
        PayloadReadHeld,
        /// Its wait for the read lock.
        PayloadReadWait,
    }
    const N: usize = 14;

    /// How a payload turn was served (PA-12a).
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum TurnKind {
        /// Under the world write lock; the call is not read-eligible.
        Write,
        /// Under `world.read()`.
        Read,
        /// Read-eligible, but the write path served it.
        Fallback,
    }

    struct Cell {
        count: AtomicU64,
        total: AtomicU64,
        max: AtomicU64,
    }
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: Cell = Cell {
        count: AtomicU64::new(0),
        total: AtomicU64::new(0),
        max: AtomicU64::new(0),
    };
    static CELLS: [Cell; N] = [ZERO; N];

    pub fn add(sec: Sec, d: Duration) {
        add_n(sec, d.as_nanos().min(u64::MAX as u128) as u64);
    }

    pub fn add_n(sec: Sec, n: u64) {
        let c = &CELLS[sec as usize];
        c.count.fetch_add(1, Relaxed);
        c.total.fetch_add(n, Relaxed);
        c.max.fetch_max(n, Relaxed);
    }

    /// `(count, total, max)`, reset.
    fn take(sec: Sec) -> (u64, u64, u64) {
        let c = &CELLS[sec as usize];
        (
            c.count.swap(0, Relaxed),
            c.total.swap(0, Relaxed),
            c.max.swap(0, Relaxed),
        )
    }

    fn dur(ns: u64) -> Duration {
        Duration::from_nanos(ns)
    }

    /// One method's payload turns this window.
    #[derive(Default, Clone)]
    struct Method {
        count: u64,
        hold: Duration,
        max: Duration,
        /// Of `count`, the turns served under `world.read()` and their hold.
        read: u64,
        read_hold: Duration,
        /// Of `count`, the read-eligible calls the write path served.
        fallbacks: u64,
    }
    static METHODS: Mutex<BTreeMap<String, Method>> = Mutex::new(BTreeMap::new());

    /// **A guess** that a call changes nothing, from its name alone: the
    /// `Get`/`List`/`Is`/`Has`/`Find`/`Search`/`Fetch`/`Query`/`Check`/
    /// `Browse`/`Peek`/`Read` prefixes. A guess because "read-only in
    /// effect" needs the arm read (PA-12 does that); a `Get` that stamps a
    /// last-seen time or lazily creates a row is counted read-only here.
    /// Falsifier: PA-12's per-arm audit.
    pub fn read_only_guess(label: &str) -> bool {
        let method = label.rsplit('.').next().unwrap_or(label);
        [
            "Get", "List", "Is", "Has", "Find", "Search", "Fetch", "Query", "Check", "Browse",
            "Peek", "Read",
        ]
        .iter()
        .any(|p| {
            method
                .strip_prefix(p)
                .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_lowercase()))
        })
    }

    /// One payload turn (`dispatch.rs`, with the world lock held).
    pub fn payload_turn(label: Option<&str>, wait: Duration, hold: Duration, kind: TurnKind) {
        if kind == TurnKind::Read {
            add(Sec::PayloadReadWait, wait);
            add(Sec::PayloadReadHeld, hold);
        } else {
            add(Sec::PayloadWait, wait);
            add(Sec::PayloadHeld, hold);
        }
        let label = label.unwrap_or("(no call)");
        if let Ok(mut m) = METHODS.lock() {
            let e = match m.get_mut(label) {
                Some(e) => e,
                None => m.entry(label.to_owned()).or_default(),
            };
            e.count += 1;
            e.hold += hold;
            e.max = e.max.max(hold);
            match kind {
                TurnKind::Read => {
                    e.read += 1;
                    e.read_hold += hold;
                }
                TurnKind::Fallback => e.fallbacks += 1,
                TurnKind::Write => {}
            }
        }
    }

    fn top(rows: &[(&String, &Method)], n: usize) -> String {
        if rows.is_empty() {
            return "-".to_owned();
        }
        rows.iter()
            .take(n)
            .map(|(name, m)| {
                format!(
                    "{name}:{}:{:.1}ms:{:.1}ms:{}",
                    m.count,
                    ms(m.hold),
                    ms(m.max),
                    // `ro`: served under the read lock at least once; else the
                    // name guess, marked `?`.
                    if m.read > 0 {
                        "ro"
                    } else if read_only_guess(name) {
                        "ro?"
                    } else {
                        "rw"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The four profile lines for a window of `ticks` clock ticks lasting
    /// `window`; resets the counters.
    pub fn report_lines(ticks: u64, window: Duration) -> Vec<String> {
        let secs = window.as_secs_f64().max(1e-9);
        let t = ticks.max(1) as f64;
        let per_tick = |s: Sec| {
            let (n, total, max) = take(s);
            (n, ms(dur(total)) / t, ms(dur(max)), ms(dur(total)))
        };
        let sec_txt = |name: &str, s: Sec| {
            let (n, mean, max, _) = per_tick(s);
            format!("{name} n {n} mean {mean:.3} ms max {max:.3} ms")
        };
        let sections = format!(
            "[world] tick sections: secs={secs:.1}, {}, {}, {}, {}, {}",
            sec_txt("take_due", Sec::DrainTake),
            sec_txt("filter", Sec::DrainFilter),
            sec_txt("audience", Sec::DrainAudience),
            sec_txt("session_changes", Sec::SessionChanges),
            sec_txt("save", Sec::Save),
        );
        let (_, _, _, clock_ms) = per_tick(Sec::ClockHeld);
        let (pn, _, pmax, payload_ms) = per_tick(Sec::PayloadHeld);
        let (_, _, wmax, wait_ms) = per_tick(Sec::PayloadWait);
        let (rn, _, rmax, read_ms) = per_tick(Sec::PayloadReadHeld);
        let (_, _, rwmax, read_wait_ms) = per_tick(Sec::PayloadReadWait);
        let pct = |ms_total: f64| ms_total / (secs * 1000.0) * 100.0;
        let lock = format!(
            "[world] tick lock: secs={secs:.1}, clock held {clock_ms:.1} ms occupancy {:.1}%, \
payload held {payload_ms:.1} ms occupancy {:.1}% turns {pn} hold max {pmax:.2} ms, \
payload wait mean {:.2} ms max {wmax:.2} ms, total occupancy {:.1}%, \
read turns {rn} held {read_ms:.1} ms (shared, not in occupancy) hold max {rmax:.2} ms \
wait mean {:.2} ms max {rwmax:.2} ms{}",
            pct(clock_ms),
            pct(payload_ms),
            if pn > 0 { wait_ms / pn as f64 } else { 0.0 },
            pct(clock_ms + payload_ms),
            if rn > 0 {
                read_wait_ms / rn as f64
            } else {
                0.0
            },
            crate::clock_probe::take_lock_suffix(),
        );
        let methods = std::mem::take(&mut *METHODS.lock().unwrap_or_else(|e| e.into_inner()));
        let turns: u64 = methods.values().map(|m| m.count).sum();
        let hold: Duration = methods.values().map(|m| m.hold).sum();
        let (ro_turns, ro_hold) = methods
            .iter()
            .filter(|(k, _)| read_only_guess(k))
            .fold((0u64, Duration::ZERO), |a, (_, m)| {
                (a.0 + m.count, a.1 + m.hold)
            });
        let (served, served_hold, fallbacks) =
            methods.values().fold((0u64, Duration::ZERO, 0u64), |a, m| {
                (a.0 + m.read, a.1 + m.read_hold, a.2 + m.fallbacks)
            });
        let share = |a: f64, b: f64| if b > 0.0 { a / b * 100.0 } else { 0.0 };
        let mut by_hold: Vec<_> = methods.iter().collect();
        by_hold.sort_by_key(|(_, m)| std::cmp::Reverse(m.hold));
        let top_hold = top(&by_hold, 10);
        let mut by_count: Vec<_> = methods.iter().collect();
        by_count.sort_by_key(|(_, m)| std::cmp::Reverse(m.count));
        let top_count = top(&by_count, 10);
        let methods_line = format!(
            "[world] tick methods: secs={secs:.1}, turns {turns} methods {} hold {:.1} ms, \
ro-guess turns {:.1}% hold {:.1}%, top_hold {top_hold}, top_count {top_count}, \
ro-served turns {:.1}% hold {:.1}%, fallbacks {fallbacks}",
            methods.len(),
            ms(hold),
            share(ro_turns as f64, turns as f64),
            share(ms(ro_hold), ms(hold)),
            share(served as f64, turns as f64),
            share(ms(served_hold), ms(hold)),
        );
        let (frames, bytes, _) = take(Sec::Bytes);
        let (passes, pass_total, pass_max) = take(Sec::WriterPass);
        let (dn, dtotal, dmax) = take(Sec::Deflate);
        let (wn, wtotal, wmax) = take(Sec::WriteAll);
        let writers = format!(
            "[world] tick writers: secs={secs:.1}, frames {frames} ({:.0}/s) bytes {bytes} \
({:.1} kB/s), passes {passes} pass mean {:.2} ms max {:.2} ms, \
deflate n {dn} total {:.1} ms max {:.2} ms, writes {wn} write total {:.1} ms \
longest write {:.2} ms",
            frames as f64 / secs,
            bytes as f64 / 1000.0 / secs,
            if passes > 0 {
                ms(dur(pass_total)) / passes as f64
            } else {
                0.0
            },
            ms(dur(pass_max)),
            ms(dur(dtotal)),
            ms(dur(dmax)),
            ms(dur(wtotal)),
            ms(dur(wmax)),
        );
        vec![sections, lock, methods_line, writers]
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The counters are process statics: tests that report them take turns.
        static PROF_TESTS: Mutex<()> = Mutex::new(());

        #[test]
        fn the_method_row_says_where_a_call_was_served() {
            let _serial = PROF_TESTS.lock().unwrap_or_else(|e| e.into_inner());
            let d = Duration::from_millis(1);
            payload_turn(Some("invented.GetRead"), d, d, TurnKind::Read);
            payload_turn(Some("invented.GetFell"), d, d, TurnKind::Fallback);
            payload_turn(Some("invented.Mutate"), d, d, TurnKind::Write);
            let lines = report_lines(10, Duration::from_secs(1));
            let m = &lines[2];
            assert!(m.contains("invented.GetRead:1:1.0ms:1.0ms:ro"), "{m}");
            assert!(m.contains("invented.GetFell:1:1.0ms:1.0ms:ro?"), "{m}");
            assert!(m.contains("invented.Mutate:1:1.0ms:1.0ms:rw"), "{m}");
            assert!(m.contains("ro-served turns"), "{m}");
            // Process statics: other tests' read turns may land in the same
            // window, so at least ours is counted (it was a flake at "1").
            let reads = lines[1]
                .split("read turns ")
                .nth(1)
                .and_then(|r| r.split(' ').next())
                .and_then(|n| n.parse::<u64>().ok());
            assert!(reads.is_some_and(|n| n >= 1), "{}", lines[1]);
        }

        #[test]
        fn read_only_guess_reads_the_method_prefix() {
            assert!(read_only_guess("charMgr.GetPublicInfo"));
            assert!(read_only_guess("svc.ListThings"));
            assert!(!read_only_guess("svc.Getaway"));
            assert!(!read_only_guess("ship.Board"));
            assert!(!read_only_guess("(no call)"));
        }

        #[test]
        fn lines_report_and_reset() {
            let _serial = PROF_TESTS.lock().unwrap_or_else(|e| e.into_inner());
            payload_turn(
                Some("invented.GetThing"),
                Duration::from_millis(1),
                Duration::from_millis(2),
                TurnKind::Write,
            );
            payload_turn(
                Some("invented.GetThing"),
                Duration::from_millis(1),
                Duration::from_millis(2),
                TurnKind::Read,
            );
            add(Sec::DrainTake, Duration::from_millis(3));
            let lines = report_lines(40, Duration::from_secs(10));
            assert_eq!(lines.len(), 4);
            assert!(lines[0].starts_with("[world] tick sections:"));
            assert!(lines[1].starts_with("[world] tick lock:"));
            assert!(lines[2].starts_with("[world] tick methods:"));
            assert!(lines[3].starts_with("[world] tick writers:"));
        }
    }
}
