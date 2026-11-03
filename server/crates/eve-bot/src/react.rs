//! **Reaction jitter** (REACT-JITTER, S6): a person takes a moment to notice
//! and to click; a bot that locks, accepts and warps off in 0 ms reads as a
//! script. The first reaction to a *new* event gets a think delay; the
//! events that follow it, and the actions that continue it, do not.
//!
//! Pure, like the rest of the brain: [`State`] holds the pending and spent
//! events, [`think`] is a function of the bot's seed, its Diligence and its
//! own dice, and the driver only ever sees a `Wait` it already handles (its
//! wait is a socket pump with a ceiling, never a busy loop).
//!
//! The classes ([`Class`]) and what keys them:
//! - `Invite`: a fleet invite, by fleet id ([`crate::brain`] rule 2);
//! - `Hostile`: an NPC the escort engages, by ball id ([`crate::escort`]);
//! - `Human`: a human arriving within courtesy range, by ball id;
//! - `Threat`: a player locking or shooting us, by character id
//!   ([`crate::threat`]);
//! - `Order`: a fleet boss's order (Warp To, Hold, Travel To ...), by its
//!   number in the order the bot heard them ([`crate::fleet_orders`]);
//! - `Target`: a lock on a new target (a rock, a hostile, the boss's
//!   primary), by ball id.
//!
//! What is **not** delayed: fleet follow and warp-with-the-fleet (a fleet
//! that lags its boss looks worse than one that is a touch too prompt),
//! and anything the bot repeats for an event it has already reacted to.
//!
//! **Why 0.4-2.5 s [A].** A pilot has to see the overview row change
//! (a glance, 0.25-0.5 s), decide, and move the mouse and click (0.3-0.8 s);
//! one who is busy elsewhere takes a second or two longer. A simple
//! visual reaction is ~0.25 s and a choice among a few things ~0.5 s, so
//! 0.4 s is the fastest a person who is watching could be, and 2.5 s the
//! slowest one who is glancing in between other work. EvEJS has no such
//! number, only a chat delay. Falsifier: an owner who sees bots that feel
//! sluggish (lower `HI`) or still robotic (raise `LO`/the spread).
//! Under fire the delay is cut to the low end ([`LO`]): self-preservation is
//! the one reaction a person does not dawdle over.

use crate::brain::{unit, xorshift};
use std::time::{Duration, Instant};

/// The fastest think delay **[A]**.
pub const LO: Duration = Duration::from_millis(400);
/// The slowest **[A]**.
pub const HI: Duration = Duration::from_millis(2500);
/// How much per-event noise adds or takes, as a share of the span **[A]**.
const NOISE: f64 = 0.3;
/// A reaction not seen again for this long is forgotten: the same event
/// returning later is new **[A]**.
pub const FORGET_AFTER: Duration = Duration::from_secs(60);
/// The most events remembered.
const CAP: usize = 24;

/// A kind of event the first reaction to which is delayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Invite,
    Hostile,
    Human,
    Threat,
    Target,
    /// A fleet boss's order, by its sequence number
    /// ([`crate::fleet_orders`]).
    Order,
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    class: Class,
    key: i64,
    due: Instant,
    last: Instant,
}

/// The events the bot has begun to react to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    seen: Vec<Entry>,
}

impl State {
    /// `None`: go ahead. `Some(left)`: still thinking, for `left` more.
    /// `delay` is the delay to give an event seen for the first time
    /// (see [`think`]); it is not asked for again.
    pub fn gate(
        &mut self,
        class: Class,
        key: i64,
        now: Instant,
        delay: impl FnOnce() -> Duration,
    ) -> Option<Duration> {
        self.seen
            .retain(|e| now.saturating_duration_since(e.last) < FORGET_AFTER);
        if let Some(e) = self
            .seen
            .iter_mut()
            .find(|e| e.class == class && e.key == key)
        {
            e.last = now;
            return Some(e.due.saturating_duration_since(now)).filter(|d| !d.is_zero());
        }
        let d = delay();
        if self.seen.len() >= CAP {
            self.seen.remove(0);
        }
        self.seen.push(Entry {
            class,
            key,
            due: now + d,
            last: now,
        });
        Some(d).filter(|d| !d.is_zero())
    }
}

/// splitmix64's finaliser, so names whose hashes are close get unrelated
/// delays.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// **A bot's think delay for one new event**: `LO + span * f`, where `f`
/// is half the bot's own constant slowness (from its name hash `seed`) and
/// half its want of Diligence (`diligence` in `0 ... 1`: a grinder is
/// alert and quick, a casual pilot slow), nudged by `+-NOISE / 2` of the
/// span from `rng`, and kept in `[LO, HI]`. `under_fire` cuts it to `LO`
/// at most.
pub fn think(seed: u64, diligence: f64, rng: &mut u64, under_fire: bool) -> Duration {
    let own = unit(mix(seed));
    let f = 0.5 * own + 0.5 * (1.0 - diligence.clamp(0.0, 1.0));
    let noise = (unit(xorshift(rng)) - 0.5) * NOISE;
    let span = HI - LO;
    let d = LO + span.mul_f64((f + noise).clamp(0.0, 1.0));
    if under_fire { d.min(LO) } else { d }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::name_hash;

    fn at(s: u64) -> Instant {
        // One fixed origin per test run keeps the arithmetic exact.
        static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *ORIGIN.get_or_init(Instant::now) + Duration::from_secs(s)
    }

    fn delay_of(name: &str, dil: f64) -> Duration {
        let mut rng = name_hash(name) | 1;
        think(name_hash(name), dil, &mut rng, false)
    }

    #[test]
    fn the_first_reaction_waits_within_the_range_and_the_second_does_not() {
        let mut s = State::default();
        let mut rng = name_hash("Vela Orsk") | 1;
        let seed = name_hash("Vela Orsk");
        let left = s
            .gate(Class::Target, 7, at(0), || {
                think(seed, 0.5, &mut rng, false)
            })
            .expect("the first look is delayed");
        assert!(left >= LO && left <= HI, "{left:?}");
        // Halfway through, it is still thinking, with less to go.
        let mid = s
            .gate(Class::Target, 7, at(0) + left / 2, || unreachable!())
            .expect("still thinking");
        assert!(mid < left);
        // Done: goes ahead, and keeps going ahead.
        assert_eq!(
            s.gate(Class::Target, 7, at(0) + left, || unreachable!()),
            None
        );
        assert_eq!(s.gate(Class::Target, 7, at(5), || unreachable!()), None);
    }

    #[test]
    fn a_different_event_or_class_is_new_and_a_forgotten_one_is_new_again() {
        let mut s = State::default();
        let go = |s: &mut State, c, k, t| s.gate(c, k, t, || Duration::from_secs(1));
        assert!(go(&mut s, Class::Hostile, 1, at(0)).is_some());
        assert_eq!(go(&mut s, Class::Hostile, 1, at(2)), None);
        assert!(go(&mut s, Class::Hostile, 2, at(2)).is_some());
        assert!(go(&mut s, Class::Invite, 1, at(2)).is_some());
        // Unseen for a minute, the first is a new event once more.
        assert!(go(&mut s, Class::Hostile, 1, at(2 + 61)).is_some());
    }

    #[test]
    fn delays_differ_across_bots_and_stay_in_range() {
        let names: Vec<String> = (0..40).map(|i| format!("Pilot{i:02}")).collect();
        let ds: Vec<Duration> = names.iter().map(|n| delay_of(n, 0.5)).collect();
        assert!(ds.iter().all(|d| *d >= LO && *d <= HI));
        let mut sorted = ds.clone();
        sorted.sort();
        sorted.dedup();
        assert!(sorted.len() >= 30, "only {} distinct delays", sorted.len());
        assert!(sorted[sorted.len() - 1] - sorted[0] > Duration::from_millis(800));
    }

    #[test]
    fn a_diligent_pilot_is_quicker_and_one_under_fire_takes_the_low_end() {
        let slow: Duration = (0..40)
            .map(|i| delay_of(&format!("Pilot{i:02}"), 0.0))
            .sum();
        let quick: Duration = (0..40)
            .map(|i| delay_of(&format!("Pilot{i:02}"), 1.0))
            .sum();
        assert!(quick < slow, "{quick:?} vs {slow:?}");
        for i in 0..40 {
            let n = format!("Pilot{i:02}");
            let mut rng = name_hash(&n) | 1;
            assert!(think(name_hash(&n), 0.0, &mut rng, true) <= LO);
        }
    }

    #[test]
    fn the_same_name_and_seed_give_the_same_delay() {
        assert_eq!(delay_of("Kael Dunne", 0.3), delay_of("Kael Dunne", 0.3));
    }
}
