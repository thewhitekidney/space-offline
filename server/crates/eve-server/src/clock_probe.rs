//! Counters for the clock's wait on the world lock (lane fight-stall-fix,
//! step 1; `docs/lanes/fight-stall.md`). Process statics, read once a
//! report window by the ticker and reset: the numbers that say **who** the
//! clock waited for when a 200v200 fight skipped ticks.
//!
//! Nothing here changes behaviour. Every note is a relaxed atomic or a leaf
//! mutex held for one assignment; a counter that is never read costs a few
//! nanoseconds a lock.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn ns(d: Duration) -> u64 {
    d.as_nanos().min(u64::MAX as u128) as u64
}

// ---- I-1: the longest wait of one clock slice ------------------------------

/// `wait_ns << 8 | slice`, so one `fetch_max` keeps the slice of the
/// longest wait. A wait of 2^56 ns is two years.
static SLICE_WAIT: AtomicU64 = AtomicU64::new(0);

/// A clock slice `slice` waited `wait` for the world lock.
pub(crate) fn note_slice_wait(slice: usize, wait: Duration) {
    SLICE_WAIT.fetch_max((ns(wait) << 8) | (slice as u64 & 0xff), Relaxed);
}

// ---- I-2: the clock thread's own CPU and run-queue time --------------------

static CLOCK_CPU_MAX: AtomicU64 = AtomicU64::new(0);
static CLOCK_RUNQ_MAX: AtomicU64 = AtomicU64::new(0);
static CLOCK_SCHED_SEEN: AtomicU64 = AtomicU64::new(0);

/// One slice's CPU time and run-queue wait, from `sched_stat` (only with
/// `--turn-sched`).
pub(crate) fn note_clock_sched(cpu: Duration, runq: Duration) {
    CLOCK_SCHED_SEEN.fetch_add(1, Relaxed);
    CLOCK_CPU_MAX.fetch_max(ns(cpu), Relaxed);
    CLOCK_RUNQ_MAX.fetch_max(ns(runq), Relaxed);
}

/// The ` clock cpu max X ms runq max Y ms` tail of the stages line, reset.
/// `sched=off` when no slice was sampled.
pub(crate) fn take_clock_sched_suffix() -> String {
    let seen = CLOCK_SCHED_SEEN.swap(0, Relaxed);
    let cpu = CLOCK_CPU_MAX.swap(0, Relaxed);
    let runq = CLOCK_RUNQ_MAX.swap(0, Relaxed);
    if seen == 0 {
        return "; clock sched=off".to_owned();
    }
    format!(
        "; clock cpu max {:.2} ms runq max {:.2} ms",
        ms(Duration::from_nanos(cpu)),
        ms(Duration::from_nanos(runq))
    )
}

// ---- the gate (step 3) -----------------------------------------------------

static GATE_RAISES: AtomicU64 = AtomicU64::new(0);
static GATE_WAITS: AtomicU64 = AtomicU64::new(0);
static GATE_CAPPED: AtomicU64 = AtomicU64::new(0);
static GATE_WAIT_MAX: AtomicU64 = AtomicU64::new(0);

/// The clock raised the gate (it had to wait for the lock).
pub(crate) fn note_gate_raise() {
    GATE_RAISES.fetch_add(1, Relaxed);
}

/// A payload turn held back at a raised gate for `waited`; `capped` when it
/// gave up at the cap rather than being released.
pub(crate) fn note_gate_wait(waited: Duration, capped: bool) {
    GATE_WAITS.fetch_add(1, Relaxed);
    GATE_WAIT_MAX.fetch_max(ns(waited), Relaxed);
    if capped {
        GATE_CAPPED.fetch_add(1, Relaxed);
    }
}

// ---- I-5: threads waiting for the world lock -------------------------------

static WAITERS: AtomicUsize = AtomicUsize::new(0);
static WAITERS_MAX: AtomicUsize = AtomicUsize::new(0);

/// Alive from just before a `write()` or `read()` on the world lock to just
/// after it is granted: the count of threads queued for it.
pub(crate) struct Waiter;

impl Waiter {
    pub(crate) fn enter() -> Waiter {
        let now = WAITERS.fetch_add(1, Relaxed) + 1;
        WAITERS_MAX.fetch_max(now, Relaxed);
        Waiter
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        WAITERS.fetch_sub(1, Relaxed);
    }
}

// ---- I-6: lock sites the turn stats do not time ----------------------------

#[derive(Default)]
struct Other {
    held: Duration,
    held_site: &'static str,
    wait: Duration,
    wait_site: &'static str,
}

static OTHER: Mutex<Other> = Mutex::new(Other {
    held: Duration::ZERO,
    held_site: "-",
    wait: Duration::ZERO,
    wait_site: "-",
});

/// A lock taken outside the clock and the payload turns: `site` waited
/// `wait` and held `hold`. Keeps the window's worst of each, by site.
pub(crate) fn note_other(site: &'static str, wait: Duration, hold: Duration) {
    let mut o = OTHER.lock().unwrap_or_else(|e| e.into_inner());
    if hold >= o.held {
        o.held = hold;
        o.held_site = site;
    }
    if wait >= o.wait {
        o.wait = wait;
        o.wait_site = site;
    }
}

// ---- I-3 / I-4: the sweep, and the scheduled-queue scan --------------------

static CANCEL_SCANS: AtomicU64 = AtomicU64::new(0);
static CANCEL_SCANNED: AtomicU64 = AtomicU64::new(0);
static CANCEL_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// This thread's running `(scans, scanned, ns)` of `cancel_scheduled`,
    /// so a turn or a sweep reads its own share as a delta, whatever the
    /// other threads do.
    static CANCEL_TL: std::cell::Cell<(u64, u64, u64)> = const { std::cell::Cell::new((0, 0, 0)) };
}

/// One `Registry::cancel_scheduled` walked `scanned` entries in `took`.
pub(crate) fn note_cancel(scanned: usize, took: Duration) {
    let n = ns(took);
    CANCEL_SCANS.fetch_add(1, Relaxed);
    CANCEL_SCANNED.fetch_add(scanned as u64, Relaxed);
    CANCEL_NS.fetch_add(n, Relaxed);
    CANCEL_TL.with(|c| {
        let (a, b, t) = c.get();
        c.set((a + 1, b + scanned as u64, t.saturating_add(n)));
    });
}

/// This thread's `cancel_scheduled` time so far; two reads around a turn
/// give the turn's `cancel_us`.
pub(crate) fn cancel_ns_here() -> u64 {
    CANCEL_TL.with(|c| c.get().2)
}

/// This thread's `(scans, entries scanned, ns)` so far, for a delta.
fn cancel_totals() -> (u64, u64, u64) {
    CANCEL_TL.with(|c| c.get())
}

/// What one `targeting::sweep_out_of_range` did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepStats {
    /// Locks dropped.
    pub dropped: usize,
    /// Lock pairs walked.
    pub pairs: usize,
    /// Entries in the scheduled queue when the sweep began.
    pub scheduled_at_start: usize,
    /// `cancel_scheduled` calls the sweep made.
    pub cancel_scans: u64,
    /// Entries those calls walked.
    pub cancel_scanned: u64,
    /// Time inside them.
    pub cancel_ns: u64,
}

impl SweepStats {
    /// Fill the cancel fields from the totals read at the sweep's start.
    pub(crate) fn finish(&mut self, start: CancelMark) {
        let (scans, scanned, took) = cancel_totals();
        self.cancel_scans = scans.saturating_sub(start.0);
        self.cancel_scanned = scanned.saturating_sub(start.1);
        self.cancel_ns = took.saturating_sub(start.2);
    }
}

/// The cancel totals when a sweep began; see [`SweepStats::finish`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct CancelMark(u64, u64, u64);

pub(crate) fn cancel_mark() -> CancelMark {
    let (a, b, c) = cancel_totals();
    CancelMark(a, b, c)
}

#[derive(Default)]
struct SweepMax {
    ticks: u64,
    dropped: usize,
    pairs: usize,
    scheduled: usize,
    scans: u64,
    scanned: u64,
    cancel_ns: u64,
    sweep_ns: u64,
}

static SWEEP: Mutex<SweepMax> = Mutex::new(SweepMax {
    ticks: 0,
    dropped: 0,
    pairs: 0,
    scheduled: 0,
    scans: 0,
    scanned: 0,
    cancel_ns: 0,
    sweep_ns: 0,
});

/// The clock's sweep, once a tick: keeps the window's maxima of each field
/// (not of one tick, so a column can come from different ticks).
pub(crate) fn note_sweep(s: &SweepStats, took: Duration) {
    let mut m = SWEEP.lock().unwrap_or_else(|e| e.into_inner());
    m.ticks += 1;
    m.dropped = m.dropped.max(s.dropped);
    m.pairs = m.pairs.max(s.pairs);
    m.scheduled = m.scheduled.max(s.scheduled_at_start);
    m.scans = m.scans.max(s.cancel_scans);
    m.scanned = m.scanned.max(s.cancel_scanned);
    m.cancel_ns = m.cancel_ns.max(s.cancel_ns);
    m.sweep_ns = m.sweep_ns.max(ns(took));
}

/// The `[world] sweep:` line for the window, reset.
pub(crate) fn take_sweep_line() -> String {
    let m = std::mem::take(&mut *SWEEP.lock().unwrap_or_else(|e| e.into_inner()));
    format!(
        "[world] sweep: ticks {} max sweep {:.2} ms, dropped {} pairs {} scheduled_at_start {}, \
cancel scans {} scanned {} cancel {:.2} ms",
        m.ticks,
        ms(Duration::from_nanos(m.sweep_ns)),
        m.dropped,
        m.pairs,
        m.scheduled,
        m.scans,
        m.scanned,
        ms(Duration::from_nanos(m.cancel_ns)),
    )
}

/// The window totals of the scan outside the sweep too: `cancel` over every
/// caller, payload turns included. Reset with the line above.
pub(crate) fn take_cancel_total() -> String {
    let (scans, scanned, took) = (
        CANCEL_SCANS.swap(0, Relaxed),
        CANCEL_SCANNED.swap(0, Relaxed),
        CANCEL_NS.swap(0, Relaxed),
    );
    format!(
        "cancel total scans {scans} scanned {scanned} {:.2} ms",
        ms(Duration::from_nanos(took))
    )
}

// ---- the tail of `[world] tick lock` ---------------------------------------

/// What the lock line adds: the longest clock slice wait and its slice, the
/// waiter high-water mark, the worst outside lock, and the scan totals.
/// Resets everything it reports.
pub(crate) fn take_lock_suffix() -> String {
    let packed = SLICE_WAIT.swap(0, Relaxed);
    let waiters = WAITERS_MAX.swap(WAITERS.load(Relaxed), Relaxed);
    let o = std::mem::take(&mut *OTHER.lock().unwrap_or_else(|e| e.into_inner()));
    let (hs, ws) = if o.held_site.is_empty() {
        ("-", "-")
    } else {
        (o.held_site, o.wait_site)
    };
    format!(
        ", clock slice wait max {:.1} ms (slice {}), waiters max {waiters}, \
other held max {:.2} ms ({hs}) wait max {:.2} ms ({ws}), \
gate raised {} held back {} turns max {:.1} ms capped {}, {}",
        ms(Duration::from_nanos(packed >> 8)),
        packed & 0xff,
        ms(o.held),
        ms(o.wait),
        GATE_RAISES.swap(0, Relaxed),
        GATE_WAITS.swap(0, Relaxed),
        ms(Duration::from_nanos(GATE_WAIT_MAX.swap(0, Relaxed))),
        GATE_CAPPED.swap(0, Relaxed),
        take_cancel_total(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The statics are process-wide: tests that read them take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn the_slice_wait_keeps_the_longest_and_its_slice() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_lock_suffix();
        // Seconds, so no real tick another test runs can outdo them.
        note_slice_wait(2, Duration::from_millis(4000));
        note_slice_wait(5, Duration::from_millis(9000));
        note_slice_wait(6, Duration::from_millis(1000));
        let line = take_lock_suffix();
        assert!(
            line.contains("clock slice wait max 9000.0 ms (slice 5)"),
            "{line}"
        );
    }

    #[test]
    fn waiters_count_up_and_down_and_report_the_high_water() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_lock_suffix();
        let a = Waiter::enter();
        let b = Waiter::enter();
        let c = Waiter::enter();
        drop((a, b));
        // Other tests' sessions queue for their own worlds, so "at least".
        let line = take_lock_suffix();
        let max: usize = line
            .split("waiters max ")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .and_then(|n| n.parse().ok())
            .expect(&line);
        assert!(max >= 3, "{line}");
        drop(c);
    }

    #[test]
    fn the_worst_outside_lock_is_named_by_site() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_lock_suffix();
        note_other(
            "invented.a",
            Duration::from_millis(1),
            Duration::from_millis(9000),
        );
        note_other(
            "invented.b",
            Duration::from_millis(7000),
            Duration::from_millis(2),
        );
        let line = take_lock_suffix();
        assert!(
            line.contains("other held max 9000.00 ms (invented.a)"),
            "{line}"
        );
        assert!(line.contains("wait max 7000.00 ms (invented.b)"), "{line}");
    }

    #[test]
    fn a_cancel_scan_is_counted_and_the_sweep_reads_its_delta() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mark = cancel_mark();
        let tl0 = cancel_ns_here();
        note_cancel(10, Duration::from_micros(5));
        note_cancel(4, Duration::from_micros(2));
        let mut s = SweepStats::default();
        s.finish(mark);
        assert_eq!((s.cancel_scans, s.cancel_scanned), (2, 14));
        assert_eq!(s.cancel_ns, 7_000);
        assert_eq!(cancel_ns_here() - tl0, 7_000);
        note_sweep(&s, Duration::from_millis(3));
        let line = take_sweep_line();
        assert!(line.starts_with("[world] sweep: ticks 1"), "{line}");
        assert!(line.contains("cancel scans 2 scanned 14"), "{line}");
    }
}
