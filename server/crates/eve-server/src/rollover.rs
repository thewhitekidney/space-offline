//! **The daily rollover** (W13 packet CP25): the day turns over at 11:00 UTC
//! and nobody is disconnected.
//!
//! Spec: `docs/content/plan.md` §4.4 (ruling 7) and
//! `docs/content/C7-liveness.md` §4.2, §7.4. Lane note: `docs/lanes/cp25.md`.
//!
//! - **The trigger** ([`owed`]) is the first world tick at or after an
//!   11:00 UTC boundary ([`crate::seed::day_of`]) that the world has not yet
//!   rolled over. The world record's `last_rollover` says which boundary that
//!   was, so a server that was down for N days rolls over **once**, with
//!   `days = N` ([`Day::days`]), and a tick that finds nothing owed does
//!   nothing: running twice for one day is a no-op.
//! - **The work is [`crate::rollover_steps::STEPS`]**, in order. A
//!   [`Run::Once`] step runs the tick the rollover begins (delta-wide work:
//!   regrowth, pruning). A [`Run::PerSystem`] step runs once per live (warm or
//!   hot) system, at most [`SLICE`] systems a tick, so a busy universe never
//!   holds the world lock for one long tick. Cold systems are not in the list:
//!   they answer through their generator and their deltas.
//! - **Advancing `day` and the `[rollover]` line are this module's**, not
//!   rows: `day` must move before any step reads it, and the line can only be
//!   written when the last slice is done.
//! - **`/rollover now`** ([`force`], `service/gm_live.rs`) rolls over at
//!   once, one day, whatever the clock says.
//!
//! **[A] a forced rollover adds a day; it does not borrow the next one.**
//! The trigger compares the calendar day of `now` with that of
//! `last_rollover`, not with `world.day`, so after a `/rollover now` the next
//! 11:00 UTC still rolls over and `day` stays one ahead of the calendar for
//! good. C7 §4.2 words the trigger as `day_of(now) > world.day`, which agrees
//! whenever no GM forced a day. Falsified by an operator who expects the
//! next real rollover to be skipped after a forced one.

use crate::damage::BlueTime;
use crate::liveness::Liveness;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// At most this many live systems get their per-system steps in one tick
/// **[A]** (C7 §4.2: 64 systems a tick, so 1,000 live systems take 16 ticks,
/// 4 s at 250 ms). Falsified by a rollover tick that shows in the `[world]`
/// stage timings as a stall a pilot notices; lower it then.
pub const SLICE: usize = 64;

/// What a step is told about the rollover it is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    /// The new day, already written to the world record.
    pub day: u32,
    /// How many days this rollover stands for: 1 normally, N for the
    /// catch-up after a server was down for N days. Never 0.
    pub days: u32,
    /// When the rollover began (blue FILETIME).
    pub now: BlueTime,
}

/// The work of one step. Each returns a count for the `[rollover]` line
/// (rocks regrown, sites expired, ...).
#[derive(Clone, Copy)]
pub enum Run {
    /// Once, the tick the rollover begins.
    Once(fn(&mut Registry, &Day) -> usize),
    /// Once per live system, [`SLICE`] systems a tick.
    /// (The per-system re-materialisation row, C7 section 4.2 step 3,
    /// `docs/lanes/cp25.md`; no core row yet. A module may use it.)
    PerSystem(fn(&mut Registry, &Day, i64) -> usize),
}

/// One row of [`crate::rollover_steps::STEPS`]: a name for the log line, and
/// the work.
#[derive(Clone, Copy)]
pub struct Step {
    pub name: &'static str,
    pub run: Run,
}

/// The world slot: the rollover in progress, if any, and the last line.
#[derive(Debug, Default)]
pub struct Rollover {
    pass: Option<Pass>,
    last: Option<String>,
}

/// A rollover that has begun and still has systems to slice through.
#[derive(Debug)]
struct Pass {
    day: Day,
    queue: VecDeque<i64>,
    systems: usize,
    /// Per step, in table order: the counts so far.
    counts: Vec<(&'static str, usize)>,
    deltas_before: usize,
    took: Duration,
    ticks: u32,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "rollover",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Rollover::default())
}

impl Rollover {
    /// The day a rollover in progress is turning over to, if one is.
    pub fn in_progress(&self) -> Option<Day> {
        self.pass.as_ref().map(|p| p.day)
    }

    /// Systems still waiting for their per-system steps.
    pub fn queued(&self) -> usize {
        self.pass.as_ref().map_or(0, |p| p.queue.len())
    }

    /// The last finished rollover's `[rollover]` line.
    pub fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
}

/// How many 11:00 UTC boundaries have passed since the last rollover, at
/// `now`: 0 when none is due. At a `--time-scale` N above 1, how many
/// compressed days (`crate::game_clock::GameClock::days_owed`, lane
/// `clock`) have.
pub fn owed(w: &Registry, now: BlueTime) -> u32 {
    let world = w.world();
    crate::game_clock::of(w).days_owed(world.birth, world.last_rollover, now)
}

/// Every delta row: what the line's "deltas pruned" is the drop of.
fn delta_rows(w: &Registry) -> usize {
    w.deltas().kinds().map(|(_, rows)| rows.len()).sum()
}

/// Begin a rollover of `days` days at `now`: advance `day`, run every
/// [`Run::Once`] step, and queue the live systems for the per-system ones.
/// A rollover still in progress is finished first, so steps never see two
/// days at once.
fn begin(w: &mut Registry, steps: &[Step], now: BlueTime, days: u32) {
    if w.ext::<Rollover>().pass.is_some() {
        while slice(w, steps, usize::MAX).is_none() {}
    }
    let started = Instant::now();
    let deltas_before = delta_rows(w);
    // `deltas_mut` marks the world dirty: the new day is durable.
    let mut world = *w.world();
    world.day = world.day.saturating_add(days);
    world.last_rollover = now;
    w.deltas_mut().set_world(world);
    let day = Day {
        day: world.day,
        days,
        now,
    };
    let mut counts = Vec::with_capacity(steps.len());
    for s in steps {
        let n = match s.run {
            Run::Once(f) => f(w, &day),
            Run::PerSystem(_) => 0,
        };
        counts.push((s.name, n));
    }
    let live: VecDeque<i64> = w.ext::<Liveness>().iter().map(|(id, _)| id).collect();
    let per_system = steps.iter().any(|s| matches!(s.run, Run::PerSystem(_)));
    let systems = live.len();
    w.ext_mut::<Rollover>().pass = Some(Pass {
        day,
        queue: if per_system { live } else { VecDeque::new() },
        systems,
        counts,
        deltas_before,
        took: started.elapsed(),
        ticks: 0,
    });
}

/// Run the per-system steps for at most `max` queued systems. When the queue
/// is empty afterwards, the rollover is done: its `[rollover]` line is
/// returned (and kept for `/rollover`). `None` while systems remain, and
/// when no rollover is in progress (then with nothing done).
fn slice(w: &mut Registry, steps: &[Step], max: usize) -> Option<String> {
    let mut pass = w.ext_mut::<Rollover>().pass.take()?;
    let started = Instant::now();
    let take = pass.queue.len().min(max);
    for system in pass.queue.drain(..take).collect::<Vec<_>>() {
        for (i, s) in steps.iter().enumerate() {
            if let Run::PerSystem(f) = s.run {
                let n = f(w, &pass.day, system);
                if let Some(c) = pass.counts.get_mut(i) {
                    c.1 += n;
                }
            }
        }
    }
    pass.took += started.elapsed();
    pass.ticks += 1;
    let slot = w.ext_mut::<Rollover>();
    if !pass.queue.is_empty() {
        slot.pass = Some(pass);
        return None;
    }
    let pruned = pass.deltas_before.saturating_sub(delta_rows(w));
    let line = line(&pass, pruned);
    w.ext_mut::<Rollover>().last = Some(line.clone());
    Some(line)
}

/// `[rollover] day N (+D): S systems, P deltas pruned, step n ..., T ms over
/// K ticks` (C7 §4.2 step 7, with one count per step instead of fixed
/// fields, so a new step needs no edit here).
fn line(pass: &Pass, pruned: usize) -> String {
    let steps = pass
        .counts
        .iter()
        .map(|(name, n)| format!("{name} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[rollover] day {} (+{}): {} systems, {pruned} deltas pruned, steps: {}, {:.1} ms over {} ticks",
        pass.day.day,
        pass.day.days,
        pass.systems,
        if steps.is_empty() { "none" } else { &steps },
        pass.took.as_secs_f64() * 1000.0,
        pass.ticks,
    )
}

/// One tick of the rollover against `steps`: begin one if a boundary has
/// passed, then run one slice. Returns the `[rollover]` line on the tick it
/// finishes.
pub fn tick_with(w: &mut Registry, steps: &[Step], now: BlueTime) -> Option<String> {
    let days = owed(w, now);
    if days > 0 {
        begin(w, steps, now, days);
    }
    slice(w, steps, SLICE)
}

/// **The stage** (`tick::STAGES`): [`tick_with`] over the real
/// [`crate::rollover_steps::STEPS`], printing the line when it finishes.
pub fn tick(w: &mut Registry, _now: Instant, ft: BlueTime) {
    if let Some(line) = tick_with(w, &crate::module::plan().rollover, ft) {
        crate::outln!("{line}");
    }
}

/// **`/rollover now`**: a one-day rollover at `now`, whatever the clock
/// says, and its first slice. Returns the line if that slice finished it.
pub fn force_with(w: &mut Registry, steps: &[Step], now: BlueTime) -> Option<String> {
    begin(w, steps, now, 1);
    slice(w, steps, SLICE)
}

/// [`force_with`] over the real steps, printing the line when it finishes.
pub fn force(w: &mut Registry, now: BlueTime) -> Option<String> {
    let line = force_with(w, &crate::module::plan().rollover, now);
    if let Some(l) = &line {
        crate::outln!("{l}");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::World;
    use crate::liveness::Reasons;
    use crate::seed::{DAY, HOUR, day_start};
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The fake steps record into statics, so the tests that use them run
    // one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());
    static ONCE_CALLS: Mutex<Vec<Day>> = Mutex::new(Vec::new());
    static SYSTEM_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn once(_: &mut Registry, d: &Day) -> usize {
        ONCE_CALLS.lock().unwrap().push(*d);
        7
    }

    fn per_system(_: &mut Registry, _: &Day, _: i64) -> usize {
        SYSTEM_CALLS.fetch_add(1, Ordering::SeqCst);
        1
    }

    const FAKE: &[Step] = &[
        Step {
            name: "fake_once",
            run: Run::Once(once),
        },
        Step {
            name: "fake_system",
            run: Run::PerSystem(per_system),
        },
    ];

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        ONCE_CALLS.lock().unwrap().clear();
        SYSTEM_CALLS.store(0, Ordering::SeqCst);
        g
    }

    /// An invented world born at `World::FIXTURE.birth` (midnight UTC), on
    /// day 0 with no rollover yet.
    fn reg() -> Registry {
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        w.deltas_mut().set_world(World::FIXTURE);
        w
    }

    /// Make `n` invented systems live (warm), observed at `now`.
    fn warm(w: &mut Registry, n: i64, now: BlueTime) {
        let reasons: BTreeMap<i64, Reasons> =
            (0..n).map(|i| (30_900_000 + i, Reasons::DOCKED)).collect();
        w.ext_mut::<Liveness>().apply(&reasons, now);
    }

    fn birth() -> BlueTime {
        World::FIXTURE.birth
    }

    #[test]
    fn the_trigger_fires_on_the_first_tick_past_eleven_utc() {
        let _g = reset();
        let mut w = reg();
        let eleven = day_start(birth(), 1);
        assert_eq!((eleven - birth()).rem_euclid(DAY), 11 * HOUR);
        assert_eq!(tick_with(&mut w, FAKE, eleven - 1), None);
        assert!(ONCE_CALLS.lock().unwrap().is_empty(), "not before 11:00");
        assert_eq!(w.world().day, 0);
        let line = tick_with(&mut w, FAKE, eleven).expect("no live systems: done at once");
        assert_eq!(w.world().day, 1);
        assert_eq!(w.world().last_rollover, eleven);
        let calls = ONCE_CALLS.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![Day {
                day: 1,
                days: 1,
                now: eleven
            }]
        );
        assert!(line.starts_with("[rollover] day 1 (+1):"), "{line}");
        assert!(line.contains("fake_once 7"), "{line}");
    }

    /// Lane clock: at scale N a day is owed every `DAY / N` after the last
    /// rollover, with no calendar, so N days roll over in one wall day.
    #[test]
    fn at_scale_n_a_day_rolls_over_every_day_over_n() {
        let _g = reset();
        let mut w = reg();
        let clock = crate::game_clock::GameClock::new(8).unwrap();
        crate::game_clock::set(&mut w, clock);
        let t0 = birth();
        let mut rolled = 0;
        let mut t = t0;
        while t <= t0 + DAY {
            if tick_with(&mut w, FAKE, t).is_some() {
                rolled += 1;
            }
            t += 15 * 60 * 10_000_000;
        }
        assert_eq!(rolled, 8, "eight compressed days in one wall day");
        assert_eq!(w.world().day, 8);
        assert!(ONCE_CALLS.lock().unwrap().iter().all(|d| d.days == 1));
        assert_eq!(clock.day() * 8, DAY);
    }

    #[test]
    fn running_twice_for_one_day_is_idempotent() {
        let _g = reset();
        let mut w = reg();
        let eleven = day_start(birth(), 1);
        tick_with(&mut w, FAKE, eleven);
        let after = *w.world();
        assert_eq!(tick_with(&mut w, FAKE, eleven), None);
        assert_eq!(tick_with(&mut w, FAKE, eleven + HOUR), None);
        assert_eq!(*w.world(), after);
        assert_eq!(ONCE_CALLS.lock().unwrap().len(), 1);
    }

    #[test]
    fn catch_up_runs_once_for_an_n_day_gap() {
        let _g = reset();
        let mut w = reg();
        let n = 5;
        let later = day_start(birth(), n) + 3 * HOUR;
        tick_with(&mut w, FAKE, later);
        tick_with(&mut w, FAKE, later + HOUR);
        let calls = ONCE_CALLS.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].days, n);
        assert_eq!(calls[0].day, n);
        assert_eq!(w.world().day, n);
    }

    #[test]
    fn slices_touch_at_most_slice_systems_per_tick() {
        let _g = reset();
        let mut w = reg();
        let eleven = day_start(birth(), 1);
        let live = 2 * SLICE as i64 + 3;
        warm(&mut w, live, eleven - 1);
        let mut ticks = 0;
        let mut seen = 0;
        let mut now = eleven;
        let line = loop {
            let out = tick_with(&mut w, FAKE, now);
            ticks += 1;
            let done = SYSTEM_CALLS.load(Ordering::SeqCst);
            assert!(done - seen <= SLICE, "{} systems in one tick", done - seen);
            seen = done;
            if let Some(line) = out {
                break line;
            }
            assert_eq!(w.ext::<Rollover>().in_progress().map(|d| d.day), Some(1));
            now += 1;
        };
        assert_eq!(seen, live as usize, "every live system once");
        assert_eq!(ticks, (live as usize).div_ceil(SLICE));
        assert!(line.contains(&format!("{live} systems")), "{line}");
        assert!(line.contains(&format!("fake_system {live}")), "{line}");
        assert!(line.contains(&format!("over {ticks} ticks")), "{line}");
        assert_eq!(ONCE_CALLS.lock().unwrap().len(), 1);
        assert_eq!(w.ext::<Rollover>().last(), Some(line.as_str()));
    }

    #[test]
    fn a_forced_rollover_adds_a_day_and_the_next_eleven_still_rolls() {
        let _g = reset();
        let mut w = reg();
        let morning = birth() + 2 * HOUR;
        assert!(force_with(&mut w, FAKE, morning).is_some());
        assert_eq!(w.world().day, 1);
        assert_eq!(
            tick_with(&mut w, FAKE, morning + HOUR),
            None,
            "nothing owed"
        );
        assert_eq!(ONCE_CALLS.lock().unwrap().len(), 1);
        tick_with(&mut w, FAKE, day_start(birth(), 1));
        assert_eq!(w.world().day, 2);
        assert_eq!(ONCE_CALLS.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_new_rollover_finishes_the_one_in_progress_first() {
        let _g = reset();
        let mut w = reg();
        let eleven = day_start(birth(), 1);
        let live = SLICE as i64 + 1;
        warm(&mut w, live, eleven - 1);
        assert_eq!(tick_with(&mut w, FAKE, eleven), None);
        assert_eq!(w.ext::<Rollover>().queued(), 1);
        force_with(&mut w, FAKE, eleven + 1);
        assert_eq!(w.world().day, 2);
        // Day 1's last system, then day 2's first slice.
        assert_eq!(SYSTEM_CALLS.load(Ordering::SeqCst), live as usize + SLICE);
    }

    #[test]
    fn the_rollover_marks_the_world_dirty() {
        let _g = reset();
        let mut w = reg();
        w.take_dirty();
        tick_with(&mut w, FAKE, day_start(birth(), 1));
        assert!(w.take_dirty());
    }

    #[test]
    fn the_real_steps_run_on_a_world_with_no_content() {
        let mut w = reg();
        let line = tick_with(&mut w, crate::rollover_steps::STEPS, day_start(birth(), 1))
            .expect("no live systems");
        for s in crate::rollover_steps::STEPS {
            assert!(line.contains(s.name), "{line}");
        }
    }
}
