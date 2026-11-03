//! **Bot-process cost counters** (`eve-bots --stats SECS`): where the CPU of
//! many bots goes. Process-wide atomics, summed over every bot thread; a
//! timed section costs two `Instant::now()` and four relaxed atomic adds, so
//! they are always on. Allocation counts would need a `GlobalAlloc`, which is
//! `unsafe`; RSS, threads and CPU come from `/proc/self` instead.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// One timed section: calls and total nanoseconds.
pub struct Section {
    calls: AtomicU64,
    ns: AtomicU64,
}

impl Section {
    const fn new() -> Section {
        Section {
            calls: AtomicU64::new(0),
            ns: AtomicU64::new(0),
        }
    }

    /// Time `f` into this section.
    pub fn time<T>(&self, f: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let out = f();
        self.calls.fetch_add(1, Relaxed);
        self.ns.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
        out
    }

    /// Add a section timed by hand.
    pub fn add(&self, d: std::time::Duration) {
        self.calls.fetch_add(1, Relaxed);
        self.ns.fetch_add(d.as_nanos() as u64, Relaxed);
    }

    fn take(&self) -> (u64, u64) {
        (self.calls.swap(0, Relaxed), self.ns.swap(0, Relaxed))
    }
}

/// Bytes of frame payload read, and frames.
pub static FRAME_BYTES: AtomicU64 = AtomicU64::new(0);
/// `loads` of one payload.
pub static DECODE: Section = Section::new();
/// `observe`: session changes and the destiny apply.
pub static OBSERVE: Section = Section::new();
/// Of which `DoDestinyUpdate`s applied to the park.
pub static DESTINY: Section = Section::new();
/// `brain::decide`.
pub static DECIDE: Section = Section::new();
/// `Driver::refresh` (park views, observation snapshot).
pub static REFRESH: Section = Section::new();
/// Act-log records (formatting, printing, pushing).
pub static LOG: Section = Section::new();
/// Pushes folded in by `absorb_all`.
pub static ABSORB: Section = Section::new();

/// `/proc/self/stat` utime+stime in clock ticks (100 Hz on Linux).
fn cpu_ticks() -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/stat") else {
        return 0;
    };
    // After the `)` that ends the command name; fields 14 and 15 overall.
    let rest = s.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let get = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    get(11) + get(12)
}

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// Reports a window's worth of counters as one line.
pub struct Reporter {
    last: Instant,
    ticks: u64,
}

impl Default for Reporter {
    fn default() -> Reporter {
        Reporter {
            last: Instant::now(),
            ticks: cpu_ticks(),
        }
    }
}

impl Reporter {
    /// The `[stats]` line for the window since the last call.
    pub fn line(&mut self, bots: usize) -> String {
        let secs = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();
        let ticks = cpu_ticks();
        let cpu = (ticks.saturating_sub(self.ticks)) as f64 / 100.0 / secs * 100.0;
        self.ticks = ticks;
        let bytes = FRAME_BYTES.swap(0, Relaxed);
        let sec = |name: &str, s: &Section| {
            let (n, ns) = s.take();
            // ms of CPU per second of wall time, and the mean call in us.
            format!(
                "{name} {:.1}ms/s n={n} {:.0}us",
                ns as f64 / 1e6 / secs,
                if n == 0 {
                    0.0
                } else {
                    ns as f64 / 1e3 / n as f64
                }
            )
        };
        let rss = status_kb("VmRSS:");
        format!(
            "[stats] bots={bots} cpu={cpu:.1}% rss={}MB ({}kB/bot) threads={} in={}kB/s | {} | {} | {} | {} | {} | {} | {}",
            rss / 1024,
            rss.checked_div(bots.max(1) as u64).unwrap_or(0),
            status_kb("Threads:"),
            (bytes as f64 / 1024.0 / secs) as u64,
            sec("decode", &DECODE),
            sec("observe", &OBSERVE),
            sec("destiny", &DESTINY),
            sec("refresh", &REFRESH),
            sec("decide", &DECIDE),
            sec("absorb", &ABSORB),
            sec("log", &LOG),
        )
    }
}
