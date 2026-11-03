//! The parallel kit for the tick (PA-6): order-preserving, panic-contained
//! helpers over `std::thread::scope`. No pool, no unsafe, no dependencies.
//!
//! Rules every caller can rely on:
//! * the result order is the input order, whatever the thread count;
//! * below `min_work` items, or with one thread, the work runs on the caller's
//!   thread and no thread is spawned (the serial oracle: `--tick-threads 1`);
//! * a panic in `f` (serial or in a worker) comes back as `Err(ParPanic)`. The
//!   caller decides what to do; nothing is poisoned and the world is untouched
//!   as long as `f` only reads (which is the contract: plan in parallel,
//!   apply serially).
//!
//! `min_work` counts items, not microseconds: pick it from the `spawn_cost`
//! bench (`tools/dev.sh cargo test --release -p eve-server par::tests::spawn_cost
//! -- --ignored --nocapture`) so that `min_work * cost_per_item >> spawn cost`.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// 0 = not set: use the default.
static THREADS: AtomicUsize = AtomicUsize::new(0);
/// Worker threads spawned by this process, for diagnostics.
static SPAWNED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Workers spawned by calls made from this thread (exact under parallel tests).
    static SPAWNED_HERE: Cell<u64> = const { Cell::new(0) };
}

/// A worker (or the serial body) panicked. The payload message is kept when it
/// was a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParPanic(pub String);

impl std::fmt::Display for ParPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "parallel worker panicked: {}", self.0)
    }
}

impl std::error::Error for ParPanic {}

/// The default thread count: half the hardware threads (the bots and the
/// writers need the rest), at least 1.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get() / 2)
        .max(1)
}

/// Set the process-wide thread count (`--tick-threads N`). 0 restores the
/// default; 1 means serial.
pub fn set_threads(n: usize) {
    THREADS.store(n, Ordering::Relaxed);
}

/// The thread count the helpers use now.
pub fn threads() -> usize {
    match THREADS.load(Ordering::Relaxed) {
        0 => default_threads(),
        n => n,
    }
}

/// Worker threads this process has spawned so far.
pub fn spawned_total() -> u64 {
    SPAWNED.load(Ordering::Relaxed)
}

/// Worker threads spawned by calls from the current thread so far.
pub fn spawned_here() -> u64 {
    SPAWNED_HERE.with(Cell::get)
}

fn message(p: Box<dyn std::any::Any + Send>) -> ParPanic {
    let m = match p.downcast::<String>() {
        Ok(s) => *s,
        Err(p) => p
            .downcast::<&'static str>()
            .map_or_else(|_| "non-string panic".to_string(), |s| (*s).to_string()),
    };
    ParPanic(m)
}

/// Split `items` into at most `threads` contiguous chunks (sizes differ by at
/// most one, earlier chunks larger) and run `f` on each, one result per chunk,
/// in chunk order. Serial (one chunk: the whole slice; none when empty) below
/// `min_work` items or with one thread. The chunk count differs between serial
/// and parallel runs, so `f` must not depend on where a chunk starts.
pub fn par_chunks<T, R, F>(items: &[T], min_work: usize, f: F) -> Result<Vec<R>, ParPanic>
where
    T: Sync,
    R: Send,
    F: Fn(&[T]) -> R + Sync,
{
    par_chunks_with(threads(), items, min_work, f)
}

/// `par_chunks` with an explicit thread count (tests, benches).
pub fn par_chunks_with<T, R, F>(
    threads: usize,
    items: &[T],
    min_work: usize,
    f: F,
) -> Result<Vec<R>, ParPanic>
where
    T: Sync,
    R: Send,
    F: Fn(&[T]) -> R + Sync,
{
    let n = items.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let parts = threads.min(n);
    if parts <= 1 || n < min_work {
        return catch_unwind(AssertUnwindSafe(|| f(items)))
            .map(|r| vec![r])
            .map_err(message);
    }
    let (base, extra) = (n / parts, n % parts);
    let f = &f;
    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(parts - 1);
        let mut rest = items;
        let mut first = &items[..0];
        for i in 0..parts {
            let (chunk, tail) = rest.split_at(base + usize::from(i < extra));
            rest = tail;
            if i == 0 {
                first = chunk; // the caller's thread works the first chunk
            } else {
                handles.push(s.spawn(move || f(chunk)));
            }
        }
        let spawned = handles.len() as u64;
        SPAWNED.fetch_add(spawned, Ordering::Relaxed);
        SPAWNED_HERE.with(|c| c.set(c.get() + spawned));
        let head = catch_unwind(AssertUnwindSafe(|| f(first))).map_err(message);
        let mut out = Vec::with_capacity(parts);
        out.push(head);
        // Join every worker (the scope would anyway); the first failure wins.
        for h in handles {
            out.push(h.join().map_err(message));
        }
        out.into_iter().collect::<Result<Vec<R>, ParPanic>>()
    })
}

/// `items.iter().map(f).collect()` over several threads, same order. Serial
/// below `min_work` items or with one thread.
pub fn par_map<T, R, F>(items: &[T], min_work: usize, f: F) -> Result<Vec<R>, ParPanic>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    par_map_with(threads(), items, min_work, f)
}

/// `par_map` with an explicit thread count (tests, benches).
pub fn par_map_with<T, R, F>(
    threads: usize,
    items: &[T],
    min_work: usize,
    f: F,
) -> Result<Vec<R>, ParPanic>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let parts = par_chunks_with(threads, items, min_work, |c| {
        c.iter().map(&f).collect::<Vec<R>>()
    })?;
    let mut out = Vec::with_capacity(items.len());
    for p in parts {
        out.extend(p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny LCG (Knuth MMIX constants); no dependencies.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn work(x: &u64) -> u64 {
        x.wrapping_mul(2654435761).rotate_left(7) ^ 0x5a5a
    }

    #[test]
    fn par_map_equals_serial_for_random_inputs() {
        let mut rng = Lcg(7);
        for _ in 0..300 {
            let len = rng.below(200) as usize;
            let items: Vec<u64> = (0..len).map(|_| rng.next()).collect();
            let threads = 1 + rng.below(9) as usize;
            let min_work = rng.below(60) as usize;
            let want: Vec<u64> = items.iter().map(work).collect();
            let got = par_map_with(threads, &items, min_work, work).unwrap();
            assert_eq!(got, want, "len {len} threads {threads} min {min_work}");
        }
    }

    #[test]
    fn par_chunks_concatenate_to_input_in_order() {
        let mut rng = Lcg(11);
        for _ in 0..300 {
            let len = rng.below(150) as usize;
            let items: Vec<u64> = (0..len).map(|_| rng.next()).collect();
            let threads = 1 + rng.below(9) as usize;
            let min_work = rng.below(40) as usize;
            let chunks = par_chunks_with(threads, &items, min_work, |c| c.to_vec()).unwrap();
            assert!(chunks.len() <= threads.max(1));
            assert!(chunks.iter().all(|c| !c.is_empty()));
            let flat: Vec<u64> = chunks.concat();
            assert_eq!(flat, items);
            let sums = par_chunks_with(threads, &items, min_work, |c| {
                c.iter().fold(0u64, |a, x| a.wrapping_add(*x))
            })
            .unwrap();
            let total = sums.iter().fold(0u64, |a, x| a.wrapping_add(*x));
            assert_eq!(total, items.iter().fold(0u64, |a, x| a.wrapping_add(*x)));
        }
    }

    #[test]
    fn worker_panic_is_err_not_abort() {
        let items: Vec<u64> = (0..64).collect();
        // The panic lands in a spawned worker (late item) ...
        let r = par_map_with(4, &items, 1, |x| {
            assert!(*x != 60, "boom {x}");
            *x
        });
        assert!(
            matches!(&r, Err(ParPanic(m)) if m.contains("boom 60")),
            "{r:?}"
        );
        // ... and in the caller's own chunk (first item) ...
        let r = par_map_with(4, &items, 1, |x| {
            assert!(*x != 0, "first");
            *x
        });
        assert!(r.is_err());
        // ... and on the serial path.
        let r = par_map_with(1, &items, 1, |x| {
            assert!(*x != 5, "serial");
            *x
        });
        assert!(r.is_err());
        // The kit still works afterwards.
        assert_eq!(par_map_with(4, &items, 1, |x| *x).unwrap(), items);
    }

    #[test]
    fn below_min_work_spawns_nothing() {
        let items: Vec<u64> = (0..10).collect();
        let before = spawned_here();
        let r = par_map_with(8, &items, 11, |x| *x + 1).unwrap();
        assert_eq!(r.len(), 10);
        assert_eq!(spawned_here(), before, "serial below threshold");
        let _ = par_map_with(1, &items, 1, |x| *x).unwrap();
        assert_eq!(spawned_here(), before, "one thread is serial");
        let _ = par_map_with(4, &items, 10, |x| *x).unwrap();
        assert_eq!(
            spawned_here(),
            before + 3,
            "at threshold: 3 workers + caller"
        );
        assert!(spawned_total() >= 3);
    }

    #[test]
    fn global_thread_count() {
        // Only this test touches the global.
        set_threads(3);
        assert_eq!(threads(), 3);
        set_threads(0);
        assert_eq!(threads(), default_threads());
        assert!(default_threads() >= 1);
    }

    /// Measure what a parallel call costs over a serial one, to choose
    /// `min_work`. Run: `cargo test --release -p eve-server par::tests::spawn_cost
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark"]
    fn spawn_cost() {
        let items: Vec<u64> = (0..1024).collect();
        let iters = 2000u32;
        for threads in [2usize, 4, 8, 12] {
            // Empty work: the whole time is the spawn/join overhead.
            let t = std::time::Instant::now();
            for _ in 0..iters {
                let r = par_chunks_with(threads, &items, 1, |c| c.len()).unwrap();
                std::hint::black_box(r);
            }
            let par_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(iters);
            let t = std::time::Instant::now();
            for _ in 0..iters {
                let r = par_chunks_with(1, &items, 1, |c| c.len()).unwrap();
                std::hint::black_box(r);
            }
            let ser_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(iters);
            crate::outln!(
                "threads {threads:>2}: {par_us:7.1} us/call parallel, {ser_us:5.2} us serial, \
                 {:6.1} us per spawned worker",
                (par_us - ser_us) / (threads - 1) as f64
            );
        }
    }
}
