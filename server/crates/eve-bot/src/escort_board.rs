//! **The escort request board** (L7b, issue #111): how a foreman asks for an
//! escort and an escort takes the job, with no GM call and no help from the
//! server. Lane note `docs/lanes/l7b.md`.
//!
//! A player foreman in a dangerous belt would say so in Local and a pilot
//! with a combat ship would come. The bots do the same through a board held
//! in the `eve-bots` process (per process, as [`crate::ice`]'s notes are):
//!
//! - **A foreman posts** ([`Board::post`]) from the belt when it has at
//!   least [`MIN_MINERS`] miners on the grid and the system is under
//!   [`SAFE_ABOVE`] security, and refreshes the post every turn; it
//!   withdraws ([`Board::withdraw`]) when it docks or the miners leave. A
//!   post nobody refreshed for [`TTL`] is gone (a foreman that crashed).
//! - **An escort takes** ([`Board::take`]): docked, outside a fleet, with no
//!   job, it picks the nearest open post within its risk's reach
//!   ([`crate::geode_haul::reach`]) and flies there. A post holds one taker.
//! - **The foreman invites its taker** whatever it flies
//!   ([`crate::foreman::invitees_with`]); in the fleet the escort follows
//!   the boss by its own rules ([`crate::escort`]). Before that, in the
//!   foreman's system and with no boss, it warps to the foreman's belt
//!   ([`Job::belt`]) instead of the nearest rock.
//! - **The job ends** when the escort is in a fleet, the post is gone or
//!   another's, or the escort has not arrived in [`PATIENCE`].
//!
//! Everything here is **[A]** policy; the falsifier for the numbers is the
//! soak's: a foreman in lowsec with miners should have an escort on the grid
//! within a few minutes, and no escort should sit in a job for an hour.

use crate::travel::Map;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Miners on the grid before a foreman asks **[A]**: a lone miner is not a
/// fleet worth guarding.
pub const MIN_MINERS: usize = 2;
/// A system at or above this security is not asked about **[A]**: high sec
/// has CONCORD. Falsifier: foremen lost in 0.5 systems.
pub const SAFE_ABOVE: f64 = 0.5;
/// How long a post lives without a refresh **[A]**.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// How long an escort keeps a job it has not reached **[A]**.
pub const PATIENCE: Duration = Duration::from_secs(30 * 60);

/// One foreman's ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub system: i64,
    /// The belt the foreman is at, if it has one.
    pub belt: Option<i64>,
    /// The last refresh.
    pub at: Instant,
    /// The escort that took it.
    pub taker: Option<i64>,
}

/// What an escort holds: the foreman it serves, and since when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Job {
    pub foreman: i64,
    pub system: i64,
    pub since: Instant,
}

/// The asks, by foreman (character id).
#[derive(Debug, Default)]
pub struct Board {
    asks: BTreeMap<i64, Request>,
}

/// **Does a foreman ask?** (pure) At a belt, with enough miners, in a
/// system where no CONCORD answers. `security` is `None` when the map does
/// not know the system: then no ask.
pub fn wants_escort(at_belt: bool, miners: usize, security: Option<f64>) -> bool {
    at_belt && miners >= MIN_MINERS && security.is_some_and(|s| s < SAFE_ABOVE)
}

impl Board {
    /// Post (or refresh) `foreman`'s ask. A refresh keeps the taker.
    pub fn post(&mut self, foreman: i64, system: i64, belt: Option<i64>, now: Instant) {
        let taker = self.asks.get(&foreman).and_then(|r| r.taker);
        self.asks.insert(
            foreman,
            Request {
                system,
                belt,
                at: now,
                taker,
            },
        );
    }

    /// Take `foreman`'s ask down.
    pub fn withdraw(&mut self, foreman: i64) {
        self.asks.remove(&foreman);
    }

    /// Drop the asks nobody refreshed for [`TTL`].
    pub fn expire(&mut self, now: Instant) {
        self.asks
            .retain(|_, r| now.saturating_duration_since(r.at) < TTL);
    }

    /// The ask of `foreman`.
    pub fn get(&self, foreman: i64) -> Option<&Request> {
        self.asks.get(&foreman)
    }

    /// The escort that took `foreman`'s ask.
    pub fn taker_of(&self, foreman: i64) -> Option<i64> {
        self.asks.get(&foreman).and_then(|r| r.taker)
    }

    /// **Take the nearest open ask** within `reach` jumps of `from`, at
    /// security `floor`, for `escort`; the system the ask is in counts as
    /// distance 0. Ties go to the lower foreman id. `None`: nothing open in
    /// reach.
    pub fn take(
        &mut self,
        escort: i64,
        map: &Map,
        from: i64,
        reach: u32,
        floor: f64,
        now: Instant,
    ) -> Option<Job> {
        self.expire(now);
        let (_, foreman) = self
            .asks
            .iter()
            .filter(|(f, r)| r.taker.is_none() && **f != escort)
            .filter_map(|(f, r)| {
                let jumps = if r.system == from {
                    0
                } else {
                    map.jumps(from, r.system, floor)?
                };
                (jumps <= reach).then_some((jumps, *f))
            })
            .min()?;
        let r = self.asks.get_mut(&foreman)?;
        r.taker = Some(escort);
        Some(Job {
            foreman,
            system: r.system,
            since: now,
        })
    }

    /// **Does `job` stand?** The ask is still there and still `escort`'s,
    /// and the escort has not been at it longer than [`PATIENCE`].
    pub fn holds(&self, escort: i64, job: &Job, now: Instant) -> bool {
        self.asks
            .get(&job.foreman)
            .is_some_and(|r| r.taker == Some(escort))
            && now.saturating_duration_since(job.since) < PATIENCE
    }

    /// Give the ask of `job` back (the escort is done with it, or in a fleet).
    pub fn release(&mut self, escort: i64, job: &Job) {
        if let Some(r) = self.asks.get_mut(&job.foreman)
            && r.taker == Some(escort)
        {
            r.taker = None;
        }
    }
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// Run `f` on the process's board.
pub fn with<T>(f: impl FnOnce(&mut Board) -> T) -> Option<T> {
    let mut g = BOARD.lock().ok()?;
    Some(f(g.get_or_insert_with(Board::default)))
}

/// What an escort remembers: its job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mind {
    pub job: Option<Job>,
}

impl Mind {
    /// The belt of the foreman served, for the escort with no boss yet.
    pub fn belt(&self) -> Option<i64> {
        let job = self.job?;
        with(|b| b.get(job.foreman).and_then(|r| r.belt)).flatten()
    }

    /// The system to fly to while the job stands.
    pub fn system(&self) -> Option<i64> {
        self.job.map(|j| j.system)
    }

    /// **Keep or drop the job** for this turn: dropped when the escort is in
    /// a fleet (it has what it came for) or the board no longer holds it.
    pub fn tend(&mut self, escort: i64, in_fleet: bool, now: Instant) {
        let Some(job) = self.job else { return };
        let stands = with(|b| {
            if in_fleet || !b.holds(escort, &job, now) {
                b.release(escort, &job);
                false
            } else {
                true
            }
        })
        .unwrap_or(false);
        if !stands {
            self.job = None;
        }
    }

    /// Docked, free and outside a fleet: take the nearest open ask.
    pub fn look(
        &mut self,
        escort: i64,
        map: &Map,
        from: i64,
        reach: u32,
        floor: f64,
        now: Instant,
    ) -> Option<Job> {
        if self.job.is_some() {
            return None;
        }
        let job = with(|b| b.take(escort, map, from, reach, floor, now)).flatten()?;
        self.job = Some(job);
        Some(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Map {
        // 1 - 2 - 3 - 4 in a line.
        let mut m = Map::new();
        for (a, b) in [(1, 2), (2, 3), (3, 4)] {
            m = m.with_gate(a, a * 10 + b, b, b * 10 + a);
            m = m.with_gate(b, b * 10 + a, a, a * 10 + b);
        }
        m
    }

    #[test]
    fn a_foreman_asks_only_in_a_dangerous_system_with_miners() {
        assert!(wants_escort(true, MIN_MINERS, Some(0.3)));
        assert!(!wants_escort(true, MIN_MINERS - 1, Some(0.3)));
        assert!(!wants_escort(false, MIN_MINERS, Some(0.3)));
        assert!(!wants_escort(true, MIN_MINERS, Some(0.9)));
        assert!(!wants_escort(true, MIN_MINERS, None));
    }

    #[test]
    fn a_refresh_keeps_the_taker_and_a_stale_post_dies() {
        let t0 = Instant::now();
        let mut b = Board::default();
        b.post(10, 3, Some(77), t0);
        assert!(
            b.take(20, &map(), 3, 0, crate::travel::ANY_SECURITY, t0)
                .is_some()
        );
        b.post(10, 3, Some(78), t0 + Duration::from_secs(60));
        assert_eq!(b.taker_of(10), Some(20));
        assert_eq!(b.get(10).and_then(|r| r.belt), Some(78));
        b.expire(t0 + Duration::from_secs(60) + TTL);
        assert!(b.get(10).is_none());
    }

    #[test]
    fn the_nearest_open_ask_in_reach_is_taken_and_only_once() {
        let t0 = Instant::now();
        let mut b = Board::default();
        b.post(10, 4, None, t0);
        b.post(11, 2, None, t0);
        let job = b
            .take(20, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
            .expect("a job");
        assert_eq!((job.foreman, job.system), (11, 2));
        let next = b
            .take(21, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
            .expect("the other");
        assert_eq!(next.foreman, 10);
        assert!(
            b.take(22, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
                .is_none(),
            "all taken"
        );
    }

    #[test]
    fn an_ask_out_of_reach_stays_open() {
        let t0 = Instant::now();
        let mut b = Board::default();
        b.post(10, 4, None, t0);
        assert!(
            b.take(20, &map(), 1, 2, crate::travel::ANY_SECURITY, t0)
                .is_none()
        );
        assert_eq!(b.taker_of(10), None);
    }

    #[test]
    fn a_foreman_does_not_escort_itself() {
        let t0 = Instant::now();
        let mut b = Board::default();
        b.post(10, 2, None, t0);
        assert!(
            b.take(10, &map(), 2, 3, crate::travel::ANY_SECURITY, t0)
                .is_none()
        );
    }

    #[test]
    fn a_job_ends_when_withdrawn_stolen_or_old_and_release_frees_the_ask() {
        let t0 = Instant::now();
        let mut b = Board::default();
        b.post(10, 2, None, t0);
        let job = b
            .take(20, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
            .expect("a job");
        assert!(b.holds(20, &job, t0));
        assert!(!b.holds(21, &job, t0), "another escort's");
        assert!(!b.holds(20, &job, t0 + PATIENCE));
        b.release(20, &job);
        assert_eq!(b.taker_of(10), None);
        let again = b
            .take(21, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
            .expect("free again");
        assert_eq!(again.foreman, 10);
        b.withdraw(10);
        assert!(!b.holds(21, &again, t0));
    }

    #[test]
    fn the_mind_takes_once_and_drops_the_job_in_a_fleet() {
        let t0 = Instant::now();
        // Process-wide board: ids no other test uses.
        let (foreman, escort) = (9_001, 9_002);
        with(|b| b.post(foreman, 2, Some(55), t0));
        let mut mind = Mind::default();
        let job = mind
            .look(escort, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
            .expect("a job");
        assert_eq!(job.foreman, foreman);
        assert!(
            mind.look(escort, &map(), 1, 3, crate::travel::ANY_SECURITY, t0)
                .is_none()
        );
        assert_eq!((mind.system(), mind.belt()), (Some(2), Some(55)));
        mind.tend(escort, false, t0);
        assert!(mind.job.is_some());
        mind.tend(escort, true, t0);
        assert!(mind.job.is_none());
        assert_eq!(with(|b| b.taker_of(foreman)).flatten(), None);
        with(|b| b.withdraw(foreman));
    }
}
