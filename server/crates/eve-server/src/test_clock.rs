//! A frozen wall clock for one test thread (PA-8 golden tests).
//!
//! Thread-local on purpose: `par` workers are fresh threads and read the real
//! clock, so **a plan that reads the clock makes a golden test fail**, which
//! is the point (a plan is a pure function of the world, `docs/plans`).
//! The three wall-clock readers (`space_state::now_seconds`,
//! `destiny::tick`, `pyutil::filetime_now`) consult it in test builds only.

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

thread_local! {
    static FROZEN: Cell<Option<Duration>> = const { Cell::new(None) };
}

/// Freeze this thread's clock at `since_epoch`.
pub fn freeze(since_epoch: Duration) {
    FROZEN.with(|f| f.set(Some(since_epoch)));
}

/// Move a frozen clock on by `d` (nothing if it is not frozen).
pub fn step(d: Duration) {
    FROZEN.with(|f| f.set(f.get().map(|t| t + d)));
}

/// Back to the real clock.
pub fn thaw() {
    FROZEN.with(|f| f.set(None));
}

/// The frozen time, or `None` when this thread's clock runs.
pub fn now() -> Option<SystemTime> {
    FROZEN.with(Cell::get).map(|d| UNIX_EPOCH + d)
}

/// The frozen time or the real one: what the three readers use.
pub fn system_now() -> SystemTime {
    now().unwrap_or_else(SystemTime::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_clock_reads_the_same_twice_and_steps() {
        freeze(Duration::from_secs(1_800_000_000));
        let a = (
            crate::space_state::now_seconds(),
            crate::destiny::tick(),
            crate::pyutil::filetime_now(),
        );
        let b = (
            crate::space_state::now_seconds(),
            crate::destiny::tick(),
            crate::pyutil::filetime_now(),
        );
        assert_eq!(a, b);
        step(Duration::from_secs(1));
        assert_eq!(crate::destiny::tick(), a.1 + 1);
        assert_eq!(crate::space_state::now_seconds(), a.0 + 1.0);
        assert_eq!(crate::pyutil::filetime_now(), a.2 + 10_000_000);
        thaw();
        assert!(now().is_none());
    }
}
