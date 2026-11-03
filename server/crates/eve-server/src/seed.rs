//! The world seed, deterministic streams and the game day (W11 packet CP4,
//! `docs/content/plan.md` §4.3, `docs/content/C7-liveness.md` §4.1).
//!
//! **The rule for every generator** (plan §3.1): its output is a pure
//! function of `(world_seed, system, slot key, generation)` plus loaded
//! tables. It draws only from a [`Stream`] built by [`stream`] or [`mix`],
//! and never from the wall clock, `HashMap` order or the order systems were
//! touched.
//!
//! The mixer is `splitmix64`, with the same constants and the same step as
//! the belt layout's private `asteroid::Mix` (`asteroid.rs:450-463`) **[V]**,
//! so `Stream::new(belt ^ 0xA57E_801D)` lays out a belt exactly as it does
//! today and asteroid layouts can migrate here without moving.
//!
//! `warp_in::PyRandom` is not content and stays as it is: it reproduces the
//! client's own MT19937.

/// The splitmix64 increment (the golden ratio in 64 bits).
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// splitmix64's output function.
fn finalize(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A deterministic stream of draws: splitmix64 from a 64-bit state.
///
/// `Stream::new(x)` yields the same sequence as `asteroid::Mix(x)` [V].
///
/// ```
/// use eve_server::seed::Stream;
/// let mut a = Stream::new(7);
/// let mut b = Stream::new(7);
/// assert_eq!(a.next(), b.next());
/// let u = a.unit();
/// assert!((0.0..1.0).contains(&u));
/// assert!((10..20).contains(&a.range(10, 20)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream(u64);

impl Stream {
    /// A stream whose state starts at `state`.
    pub const fn new(state: u64) -> Self {
        Stream(state)
    }

    /// The next 64 uniform bits.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN);
        finalize(self.0)
    }

    /// Uniform in `[0, 1)`: the top 53 bits, so `1.0` is never returned.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[lo, hi)`; `lo` when the range is empty.
    ///
    /// Multiply-shift (Lemire) without the rejection step: the bias is at
    /// most `(hi - lo) / 2^64`, far below anything a generator can see.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo;
        lo + ((u128::from(self.next()) * u128::from(span)) >> 64) as u64
    }

    /// True with probability `p` (clamped to `[0, 1]`).
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// splitmix64 folded over `parts`: each part is XORed into the state, which
/// then takes one splitmix step. The result is the next output after the last
/// part.
///
/// With no parts, `mix(s, &[])` is the first draw of `Stream::new(s)`, and so
/// of `asteroid::Mix(s)` [V].
///
/// ```
/// use eve_server::seed::{mix, Stream};
/// assert_eq!(mix(42, &[]), Stream::new(42).next());
/// assert_ne!(mix(42, &[1, 2]), mix(42, &[2, 1]));
/// ```
pub fn mix(seed: u64, parts: &[u64]) -> u64 {
    let mut s = Stream::new(seed);
    for &p in parts {
        s = Stream::new(s.next() ^ p);
    }
    s.next()
}

/// Which generator draws, so two generators never share a stream.
///
/// The discriminants are part of the universe: **never renumber or reuse
/// one**, only append. A change moves every world, which the golden test
/// below makes visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u64)]
pub enum Kind {
    /// Population slots: `(system, key, k, generation)` (plan §3.3).
    Site = 1,
    /// Signature ids and scan results.
    Sig = 2,
    /// Belt rocks and belt rats.
    Belt = 3,
    /// NPC market orders (C5).
    NpcOrder = 4,
    /// Agent mission offers; these take `day`.
    Offer = 5,
    /// NPC spawns and squad fills.
    Spawn = 6,
    /// Loot drops.
    Loot = 7,
    /// NPC choices (`npc::roll`).
    Npc = 8,
    /// Probe result noise (`scan::smear`).
    Scan = 9,
    /// Wormhole connections.
    Wormhole = 10,
}

/// The stream a generator of `kind` draws from for `parts`, e.g.
/// `stream(world.seed, Kind::Site, &[system, key, k, generation])`.
///
/// ```
/// use eve_server::seed::{stream, Kind};
/// let a = stream(5, Kind::Site, &[30_000_001, 3]).next();
/// assert_eq!(a, stream(5, Kind::Site, &[30_000_001, 3]).next());
/// assert_ne!(a, stream(5, Kind::Belt, &[30_000_001, 3]).next());
/// ```
pub fn stream(seed: u64, kind: Kind, parts: &[u64]) -> Stream {
    let mut s = Stream::new(seed ^ (kind as u64).wrapping_mul(GOLDEN));
    for &p in parts {
        s = Stream::new(s.next() ^ p);
    }
    Stream::new(s.next())
}

/// A fresh world seed, drawn once at world birth and then persisted (CP5).
///
/// std-only entropy: the process's randomly keyed `RandomState` hasher over
/// the wall clock and the process id. Not cryptographic, and it need not be.
pub fn draw_world_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.write_u128(now);
    h.write_u32(std::process::id());
    finalize(h.finish())
}

/// One blue FILETIME hour: 100 ns ticks.
pub const HOUR: i64 = 3_600 * 10_000_000;
/// One blue FILETIME day.
pub const DAY: i64 = 24 * HOUR;
/// When the day turns over, as an offset into the UTC day: 11:00 **[V]**.
///
/// The client's default `DowntimeStarts` is `time(11, 0)`
/// (`gametime/downtime.pyc:14-17`); the server reads it as UTC and the jail
/// sets `TZ=UTC` (ruling 7, plan §4.4).
pub const ROLLOVER: i64 = 11 * HOUR;

/// The index of the game day holding `t`: days start at 11:00 UTC. A blue
/// FILETIME counts from 1601-01-01 00:00 UTC with no leap seconds, so a UTC
/// day is exactly [`DAY`] ticks and midnight is a multiple of it.
fn day_index(t: i64) -> i64 {
    (t - ROLLOVER).div_euclid(DAY)
}

/// The game day at `now` for a world born at `birth` (both blue FILETIMEs):
/// the number of 11:00 UTC boundaries in `(birth, now]`.
///
/// Day 0 runs from birth to the first 11:00 UTC after it; `11:00:00`
/// exactly belongs to the new day, as in the client, which steps back a day
/// only when today's 11:00 "is still in the future" (`downtime.pyc:31-37`)
/// **[V]**. A `now` before `birth` is day 0.
pub fn day_of(birth: i64, now: i64) -> u32 {
    let d = day_index(now) - day_index(birth);
    d.clamp(0, i64::from(u32::MAX)) as u32
}

/// The blue FILETIME at which day `day` of a world born at `birth` starts
/// (`birth` itself for day 0). The rollover (CP25) uses it for catch-up.
pub fn day_start(birth: i64, day: u32) -> i64 {
    if day == 0 {
        return birth;
    }
    (day_index(birth) + i64::from(day)) * DAY + ROLLOVER
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Midnight UTC, 1970-01-01, as a blue FILETIME [V: the Win32 epoch
    /// offset, 11,644,473,600 s].
    const UNIX_EPOCH: i64 = 116_444_736_000_000_000;
    const SECOND: i64 = 10_000_000;

    #[test]
    fn mix_is_stable() {
        // Produced by this function when CP4 landed. A change here moves
        // every generated universe: bump `gen_version` and say why.
        let got = [
            mix(0, &[]),
            mix(0x5EED, &[]),
            mix(0x5EED, &[1]),
            mix(0x5EED, &[1, 2, 3]),
            mix(u64::MAX, &[u64::MAX, 0]),
            stream(0x5EED, Kind::Site, &[30_000_001, 7, 0, 0]).next(),
        ];
        assert_eq!(got, GOLDEN_VECTOR, "{got:#x?}");
    }

    const GOLDEN_VECTOR: [u64; 6] = [
        // splitmix64's published first output from state 0 [V].
        0xe220_a839_7b1d_cdaf,
        0x09f1_fd9d_03f0_a9b4,
        0x6f8d_8fc9_a414_4514,
        0x5b6b_75e6_2130_5151,
        0xf14a_3108_3112_3e0a,
        0x4662_23e1_29a3_23bf,
    ];

    #[test]
    fn mix_equals_asteroid_mix_on_the_same_input() {
        for seed in [0u64, 1, 0x5EED, 30_000_001 ^ 0xA57E_801D, u64::MAX] {
            let mut ours = Stream::new(seed);
            let mut theirs = crate::asteroid::Mix(seed);
            assert_eq!(mix(seed, &[]), crate::asteroid::Mix(seed).next());
            for _ in 0..64 {
                assert_eq!(ours.next(), theirs.next());
            }
            for _ in 0..64 {
                assert_eq!(ours.unit().to_bits(), theirs.unit().to_bits());
            }
        }
    }

    #[test]
    fn stream_stays_in_range() {
        let mut s = Stream::new(0x5EED);
        let spans = [
            (0, 1),
            (0, 2),
            (5, 6),
            (10, 17),
            (0, u64::MAX),
            (u64::MAX - 3, u64::MAX),
            (1 << 63, u64::MAX),
        ];
        for _ in 0..10_000 {
            let u = s.unit();
            assert!((0.0..1.0).contains(&u), "{u}");
            for &(lo, hi) in &spans {
                let r = s.range(lo, hi);
                assert!(lo <= r && r < hi, "{r} not in [{lo}, {hi})");
            }
        }
        // An empty range answers its floor.
        assert_eq!(s.range(9, 9), 9);
        assert_eq!(s.range(9, 3), 9);
        // The largest draw still maps below 1 and below hi.
        let top = (u64::MAX >> 11) as f64 / (1u64 << 53) as f64;
        assert!(top < 1.0);
    }

    #[test]
    fn range_reaches_both_ends() {
        let mut s = Stream::new(1);
        let mut seen = [false; 4];
        for _ in 0..1_000 {
            seen[(s.range(3, 7) - 3) as usize] = true;
        }
        assert_eq!(seen, [true; 4]);
    }

    #[test]
    fn kinds_and_parts_separate_streams() {
        let a = stream(1, Kind::Site, &[2]).next();
        assert_ne!(a, stream(1, Kind::Sig, &[2]).next());
        assert_ne!(a, stream(2, Kind::Site, &[2]).next());
        assert_ne!(a, stream(1, Kind::Site, &[3]).next());
        assert_ne!(a, stream(1, Kind::Site, &[2, 0]).next());
        assert_eq!(a, stream(1, Kind::Site, &[2]).next());
    }

    #[test]
    fn day_turns_at_11_utc() {
        // Born 09:30 UTC on some day N [the client's 11:00 default, V].
        let midnight = UNIX_EPOCH + 20_000 * DAY;
        let birth = midnight + 9 * HOUR + 30 * 60 * SECOND;
        let at = |h: i64, m: i64, s: i64| midnight + (h * 3_600 + m * 60 + s) * SECOND;
        assert_eq!(day_of(birth, birth), 0);
        assert_eq!(day_of(birth, at(10, 59, 59)), 0);
        assert_eq!(day_of(birth, at(11, 0, 0)), 1);
        assert_eq!(day_of(birth, at(11, 0, 0) - 1), 0);
        assert_eq!(day_of(birth, at(10, 59, 59) + DAY), 1);
        assert_eq!(day_of(birth, at(11, 0, 0) + DAY), 2);
        assert_eq!(day_of(birth, at(11, 0, 0) + 9 * DAY), 10);
        // Before birth is day 0, never negative.
        assert_eq!(day_of(birth, birth - 5 * DAY), 0);
        assert_eq!(day_start(birth, 0), birth);
        assert_eq!(day_start(birth, 1), at(11, 0, 0));
        assert_eq!(day_start(birth, 3), at(11, 0, 0) + 2 * DAY);
    }

    #[test]
    fn day_of_a_world_born_after_11_or_at_11() {
        let midnight = UNIX_EPOCH + 20_000 * DAY;
        let eleven = midnight + ROLLOVER;
        // Born 15:00: the next turn is tomorrow's 11:00.
        let late = midnight + 15 * HOUR;
        assert_eq!(day_of(late, eleven + DAY - SECOND), 0);
        assert_eq!(day_of(late, eleven + DAY), 1);
        // Born exactly at 11:00: that boundary is not after birth.
        assert_eq!(day_of(eleven, eleven), 0);
        assert_eq!(day_of(eleven, eleven + DAY - SECOND), 0);
        assert_eq!(day_of(eleven, eleven + DAY), 1);
    }

    #[test]
    fn the_rollover_hour_is_utc_aligned() {
        // 1970-01-01 11:00:00 UTC is a boundary; one second before is not.
        let b = UNIX_EPOCH + ROLLOVER;
        assert_eq!(day_of(b - 1, b), 1);
        assert_eq!(day_of(b - SECOND, b - 1), 0);
    }

    #[test]
    fn fresh_seeds_differ() {
        assert_ne!(draw_world_seed(), draw_world_seed());
    }
}
