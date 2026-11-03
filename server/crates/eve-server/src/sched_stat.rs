//! Per-thread scheduler accounting, for the turn trace (TT-A, part a).
//!
//! `/proc/thread-self/schedstat` holds three numbers: nanoseconds on a CPU,
//! nanoseconds waiting on a run queue, and timeslices. Two samples around a
//! payload turn split its hold into CPU, run-queue wait (the holder was
//! preempted) and the rest (blocked: a lock, an allocator, I/O).
//!
//! Off unless `--turn-sched` sets [`set_enabled`]; off, [`sample`] is one
//! relaxed load. A missing file (not Linux, or `CONFIG_SCHEDSTATS` off) or a
//! parse failure gives `None` too, and the report says `sched=off`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// `--turn-sched`.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether [`set_enabled`] was called with true.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// One reading: nanoseconds on CPU and on a run queue so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedSample {
    pub cpu_ns: u64,
    pub runq_ns: u64,
}

impl SchedSample {
    /// What happened between `earlier` and `self`.
    pub fn since(self, earlier: SchedSample) -> (Duration, Duration) {
        (
            Duration::from_nanos(self.cpu_ns.saturating_sub(earlier.cpu_ns)),
            Duration::from_nanos(self.runq_ns.saturating_sub(earlier.runq_ns)),
        )
    }
}

/// `"123 456 7\n"` -> `(123, 456)`.
pub fn parse(text: &str) -> Option<(u64, u64)> {
    let mut it = text.split_ascii_whitespace();
    let cpu = it.next()?.parse().ok()?;
    let runq = it.next()?.parse().ok()?;
    it.next()?.parse::<u64>().ok()?;
    Some((cpu, runq))
}

/// The calling thread's counters, or `None`; see the module header.
pub fn sample() -> Option<SchedSample> {
    if !enabled() {
        return None;
    }
    read()
}

#[cfg(target_os = "linux")]
fn read() -> Option<SchedSample> {
    use std::cell::RefCell;
    use std::os::unix::fs::FileExt;
    thread_local! {
        // Opened once per thread; `/proc/thread-self` is the opener's own.
        static FILE: RefCell<Option<Option<std::fs::File>>> = const { RefCell::new(None) };
    }
    FILE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let file = slot
            .get_or_insert_with(|| std::fs::File::open("/proc/thread-self/schedstat").ok())
            .as_ref()?;
        let mut buf = [0u8; 64];
        let n = file.read_at(&mut buf, 0).ok()?;
        let (cpu_ns, runq_ns) = parse(std::str::from_utf8(&buf[..n]).ok()?)?;
        Some(SchedSample { cpu_ns, runq_ns })
    })
}

#[cfg(not(target_os = "linux"))]
fn read() -> Option<SchedSample> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_numbers() {
        assert_eq!(parse("123 456 7\n"), Some((123, 456)));
    }

    #[test]
    fn garbage_gives_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("abc def ghi"), None);
        assert_eq!(parse("1 2"), None);
    }

    #[test]
    fn disabled_gives_none() {
        if !enabled() {
            assert_eq!(sample(), None);
        }
    }

    #[test]
    fn since_saturates() {
        let a = SchedSample {
            cpu_ns: 10,
            runq_ns: 5,
        };
        let b = SchedSample {
            cpu_ns: 30,
            runq_ns: 4,
        };
        assert_eq!(b.since(a), (Duration::from_nanos(20), Duration::ZERO));
    }

    #[test]
    #[ignore]
    fn sched_pair() {
        set_enabled(true);
        let t = std::time::Instant::now();
        let mut ok = 0u32;
        for _ in 0..100_000 {
            let a = sample();
            let b = sample();
            if a.is_some() && b.is_some() {
                ok += 1;
            }
        }
        println!("sched pair: {:?}/pair, ok={ok}", t.elapsed() / 100_000);
        set_enabled(false);
    }
}
