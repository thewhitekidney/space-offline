//! **The game clock** (lane `clock`): `--time-scale N` runs the slow game
//! timers N times faster, so a 24-hour economic day takes 24/N hours, while
//! physics, module cycles, warp and the network stay on real time.
//!
//! Lane note: `docs/lanes/clock.md`, which lists every timer that reads
//! this module and every one that deliberately does not.
//!
//! ## The choice: compressed spans, real timestamps
//!
//! There is **one** clock in this server, the wall clock
//! (`pyutil::filetime_now`), and the client sets its own clock from it
//! (`connection.SynchronizeClock`, see `filetime_now`'s note). Every
//! timestamp we store or send stays a real wall FILETIME. What the scale
//! changes is the **length of a slow span** at the moment a deadline is
//! made from it: a 10-hour job installed at scale 10 ends one wall hour
//! after its start, and its `endDate` says so. So what the client shows and
//! what the server does agree by construction: the client counts down to
//! the same wall time the server will act at.
//!
//! The alternative, a `game_now()` that runs ahead of the wall clock, was
//! rejected: the client's clock is the wall clock, so every game-time stamp
//! sent to it would have to be converted back (to the same compressed span
//! this module computes), and a persisted game-time stamp would need the
//! scale it was written under to mean anything after a restart.
//!
//! ## Restarts at another scale
//!
//! A deadline already made is a wall FILETIME and keeps its time: a job
//! installed at scale 10 still ends when it said it would after a restart
//! at scale 1. A span that is **derived** at read time from a stored start
//! (a skill's training end, a courier's due date, a site's respawn, a market
//! order's expiry, a dynamic bounty's recovery) is re-derived at the new
//! scale; each such site says so in the lane note. Nothing about the scale
//! is persisted, and nothing in the store format changed.
//!
//! ## The day
//!
//! At scale 1 the day turns over at 11:00 UTC exactly as before
//! (`crate::seed::day_of`). At scale N > 1 there is no calendar: a rollover
//! is owed every [`GameClock::day`] wall ticks after the last one
//! ([`GameClock::days_owed`]), and a rollover step that wanted "the start of
//! the new day" gets the rollover's own time ([`day_start`]).

use crate::damage::BlueTime;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::time::Duration;

/// The largest `--time-scale`: a game day in 24 wall minutes. Bounded so a
/// typo cannot make every slow timer fire on every tick.
pub const MAX_SCALE: u32 = 60;

/// The largest `--training-scale`: a level in seconds for a dev run.
pub const MAX_TRAINING_SCALE: u32 = 10_000;

/// The environment variable `eve-server` reads when `--training-scale` is
/// not given.
pub const TRAINING_ENV: &str = "EVE_TRAINING_SCALE";

/// The environment variable `eve-server` reads when `--time-scale` is not
/// given.
pub const ENV: &str = "EVE_TIME_SCALE";

/// One blue FILETIME day, the period of the rollover at scale 1.
const DAY: BlueTime = crate::seed::DAY;

/// How many times faster than the wall clock the slow game timers run.
/// `Copy`; read it with [`of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameClock {
    scale: u32,
    /// `--training-scale` (lane `bot-ctl`): skill training only, on top of
    /// `scale`. 1 in every normal run.
    training: u32,
}

impl Default for GameClock {
    fn default() -> Self {
        GameClock::REAL
    }
}

impl GameClock {
    /// Scale 1: every span is its own length. What every test and every run
    /// without `--time-scale` gets.
    pub const REAL: GameClock = GameClock {
        scale: 1,
        training: 1,
    };

    /// A clock at `scale`, refused outside `1..=MAX_SCALE`.
    pub fn new(scale: u32) -> Result<GameClock, String> {
        if (1..=MAX_SCALE).contains(&scale) {
            Ok(GameClock { scale, training: 1 })
        } else {
            Err(format!("time scale {scale} is outside 1..={MAX_SCALE}"))
        }
    }

    /// `--time-scale`'s argument: a whole number in `1..=MAX_SCALE`.
    pub fn parse(text: &str) -> Result<GameClock, String> {
        let n: u32 = text
            .trim()
            .parse()
            .map_err(|_| format!("time scale {text:?} is not a whole number"))?;
        GameClock::new(n)
    }

    /// `eve-server`'s clock: `--time-scale`'s value when given, else the
    /// [`ENV`] variable's when set and not empty, else [`GameClock::REAL`].
    /// A bad value in either is an error, never a silent scale 1.
    pub fn from_flag_or_env(flag: Option<&str>, env: Option<&str>) -> Result<GameClock, String> {
        match (flag, env) {
            (Some(f), _) => GameClock::parse(f).map_err(|e| format!("--time-scale: {e}")),
            (None, Some(e)) if !e.trim().is_empty() => {
                GameClock::parse(e).map_err(|err| format!("{ENV}: {err}"))
            }
            _ => Ok(GameClock::REAL),
        }
    }

    /// The same clock with skill training `training` times faster still
    /// (`--training-scale`, lane `bot-ctl`), refused outside
    /// `1..=MAX_TRAINING_SCALE`. **Combines by multiplication** with the
    /// time scale: training runs `scale x training` times the wall clock.
    /// Nothing but skill training reads it ([`Self::training_pace`]).
    pub fn with_training(self, training: u32) -> Result<GameClock, String> {
        if (1..=MAX_TRAINING_SCALE).contains(&training) {
            Ok(GameClock { training, ..self })
        } else {
            Err(format!(
                "training scale {training} is outside 1..={MAX_TRAINING_SCALE}"
            ))
        }
    }

    /// `--training-scale`'s argument (or [`TRAINING_ENV`]'s), `1` when
    /// neither is given; a bad value is an error, never a silent 1.
    pub fn training_from_flag_or_env(flag: Option<&str>, env: Option<&str>) -> Result<u32, String> {
        let (text, from) = match (flag, env) {
            (Some(f), _) => (f, "--training-scale".to_owned()),
            (None, Some(e)) if !e.trim().is_empty() => (e, TRAINING_ENV.to_owned()),
            _ => return Ok(1),
        };
        let n: u32 = text
            .trim()
            .parse()
            .map_err(|_| format!("{from}: {text:?} is not a whole number"))?;
        GameClock::REAL
            .with_training(n)
            .map(|c| c.training)
            .map_err(|e| format!("{from}: {e}"))
    }

    /// The training factor alone.
    pub const fn training(self) -> u32 {
        self.training
    }

    /// How many times faster than the wall clock a skill trains:
    /// `scale x training`.
    pub fn training_pace(self) -> f64 {
        f64::from(self.scale) * f64::from(self.training)
    }

    pub const fn scale(self) -> u32 {
        self.scale
    }

    /// Scale 1, where every converted site takes its old path unchanged.
    pub const fn is_real(self) -> bool {
        self.scale == 1
    }

    /// The wall length of a game span of `game` (any unit): `game / N`. A
    /// positive span never compresses to nothing (it stays at least 1), so
    /// a deadline made from it is still after its start; zero and negative
    /// spans are returned as they are.
    pub fn span(self, game: i64) -> i64 {
        if self.scale == 1 || game <= 0 {
            return game;
        }
        (game / i64::from(self.scale)).max(1)
    }

    /// The game time that passes in a wall span of `wall`: `wall × N`. The
    /// inverse of [`Self::span`], for a timer derived from elapsed time (a
    /// recovery, an extractor's decay curve).
    pub fn elapsed(self, wall: i64) -> i64 {
        wall.saturating_mul(i64::from(self.scale))
    }

    /// A rate per game unit, as a rate per wall unit: `rate × N`.
    pub fn rate(self, per_game: f64) -> f64 {
        per_game * f64::from(self.scale)
    }

    /// [`Self::span`] for a `Duration` (a scheduled push's delay).
    pub fn duration(self, game: Duration) -> Duration {
        if self.scale == 1 {
            return game;
        }
        (game / self.scale).max(Duration::from_nanos(100).min(game))
    }

    /// The wall length of one game day.
    pub fn day(self) -> BlueTime {
        self.span(DAY)
    }

    /// **The rollover's trigger**: how many days are owed at `now` for a
    /// world born at `birth` whose last rollover was at `last`. At scale 1
    /// the 11:00 UTC boundaries crossed (`crate::seed::day_of`, unchanged);
    /// at scale N the whole [`Self::day`]s since `last` **[A]**: no calendar,
    /// so the first compressed day ends one compressed day after the last
    /// rollover, wherever in the UTC day that falls, and downtime counts at
    /// the scale the server comes back with. Falsified by an owner who wants
    /// compressed days aligned to 11:00 UTC.
    pub fn days_owed(self, birth: BlueTime, last: BlueTime, now: BlueTime) -> u32 {
        use crate::seed::day_of;
        if self.is_real() {
            return day_of(birth, now).saturating_sub(day_of(birth, last));
        }
        let days = now.saturating_sub(last).max(0) / self.day();
        days.clamp(0, i64::from(u32::MAX)) as u32
    }

    /// Where day `day` of a world born at `birth` began, for a rollover step
    /// that runs at `rolled_at` (`World::last_rollover`, which the rollover
    /// writes before any step runs). Scale 1: the calendar's
    /// `crate::seed::day_start`, unchanged. Scale N: `rolled_at`, the only
    /// start a compressed day has.
    pub fn day_start(self, birth: BlueTime, day: u32, rolled_at: BlueTime) -> BlueTime {
        if self.is_real() {
            crate::seed::day_start(birth, day)
        } else {
            rolled_at
        }
    }

    /// When the day after the one holding `now` begins: the `/rollover`
    /// answer and the status file's "next rollover". Scale 1: the next 11:00
    /// UTC. Scale N: one compressed day after `last`.
    pub fn next_rollover(self, birth: BlueTime, last: BlueTime, now: BlueTime) -> BlueTime {
        use crate::seed::{day_of, day_start};
        if self.is_real() {
            return day_start(birth, day_of(birth, now).saturating_add(1));
        }
        let owed = i64::from(self.days_owed(birth, last, now));
        last.saturating_add((owed + 1).saturating_mul(self.day()))
    }
}

/// The world slot: the clock this server runs at.
pub const SLOT: SlotSpec = SlotSpec {
    name: "game_clock",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(GameClock::REAL)
}

/// The clock of this world.
pub fn of(reg: &Registry) -> GameClock {
    *reg.ext::<GameClock>()
}

/// Set the world's clock (`eve-server --time-scale`, at startup, after the
/// world is loaded) and tell the two books that index a derived deadline:
/// the market's expiry index and the colonies' pace.
pub fn set(reg: &mut Registry, clock: GameClock) {
    *reg.ext_mut::<GameClock>() = clock;
    reg.planet.set_pace(clock);
    reg.set_market_pace(clock);
}

/// [`GameClock::day_start`] on the registry's world: where day `day` began,
/// for a rollover step.
pub fn day_start(reg: &Registry, day: u32) -> BlueTime {
    let w = reg.world();
    of(reg).day_start(w.birth, day, w.last_rollover)
}

/// [`GameClock::span`] on the registry's clock.
pub fn span(reg: &Registry, game: i64) -> i64 {
    of(reg).span(game)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = crate::seed::HOUR;

    #[test]
    fn scale_one_changes_nothing() {
        let c = GameClock::REAL;
        for x in [-5, 0, 1, 7, HOUR, DAY, 90 * DAY] {
            assert_eq!(c.span(x), x);
            assert_eq!(c.elapsed(x), x);
        }
        assert_eq!(c.rate(2.5), 2.5);
        assert_eq!(
            c.duration(Duration::from_secs(86_400)),
            Duration::from_secs(86_400)
        );
        assert_eq!(c.day(), DAY);
        assert_eq!(GameClock::default(), GameClock::REAL);
    }

    #[test]
    fn scale_n_compresses_spans_n_times_and_never_to_nothing() {
        let c = GameClock::new(8).unwrap();
        assert_eq!(c.span(8 * HOUR), HOUR);
        assert_eq!(c.span(DAY), 3 * HOUR);
        assert_eq!(c.span(3), 1, "a positive span stays positive");
        assert_eq!(c.span(0), 0);
        assert_eq!(c.span(-16), -16);
        assert_eq!(c.elapsed(HOUR), 8 * HOUR);
        assert_eq!(c.rate(1.5), 12.0);
        assert_eq!(c.duration(Duration::from_secs(80)), Duration::from_secs(10));
        assert_eq!(c.day(), 3 * HOUR);
    }

    #[test]
    fn the_scale_is_bounded_and_parsed() {
        assert!(GameClock::new(0).is_err());
        assert!(GameClock::new(MAX_SCALE + 1).is_err());
        assert_eq!(GameClock::new(MAX_SCALE).unwrap().scale(), MAX_SCALE);
        assert_eq!(GameClock::parse(" 12 ").unwrap().scale(), 12);
        assert!(GameClock::parse("1.5").is_err());
        assert!(GameClock::parse("-3").is_err());
        assert!(GameClock::parse("").is_err());
        let both = GameClock::from_flag_or_env(Some("3"), Some("9"));
        assert_eq!(both.map(GameClock::scale), Ok(3), "the flag wins");
        assert_eq!(
            GameClock::from_flag_or_env(None, Some("9")).map(GameClock::scale),
            Ok(9)
        );
        assert_eq!(
            GameClock::from_flag_or_env(None, Some(" ")),
            Ok(GameClock::REAL)
        );
        assert_eq!(GameClock::from_flag_or_env(None, None), Ok(GameClock::REAL));
        assert!(GameClock::from_flag_or_env(None, Some("0")).is_err());
        assert!(GameClock::from_flag_or_env(Some("x"), None).is_err());
    }

    /// Scale 1 keeps the calendar; scale N owes one day per compressed day
    /// since the last rollover, and catches up whole days.
    #[test]
    fn days_owed_is_the_calendar_at_one_and_compressed_days_at_n() {
        let birth = 20_000 * DAY + 9 * HOUR;
        let eleven = 20_000 * DAY + 11 * HOUR;
        let real = GameClock::REAL;
        assert_eq!(real.days_owed(birth, birth, eleven - 1), 0);
        assert_eq!(real.days_owed(birth, birth, eleven), 1);

        let c = GameClock::new(6).unwrap();
        let last = birth + 17;
        assert_eq!(c.days_owed(birth, last, last + c.day() - 1), 0);
        assert_eq!(c.days_owed(birth, last, last + c.day()), 1);
        assert_eq!(c.days_owed(birth, last, last + 5 * c.day() + 3), 5);
        assert_eq!(
            c.days_owed(birth, last, last - 1),
            0,
            "a clock run back owes nothing"
        );
        assert_eq!(c.next_rollover(birth, last, last + 1), last + c.day());
        assert_eq!(real.next_rollover(birth, birth, birth), eleven);
    }

    #[test]
    fn day_start_is_the_calendar_at_one_and_the_rollover_at_n() {
        let birth = 20_000 * DAY + 9 * HOUR;
        let rolled = birth + 5 * HOUR;
        assert_eq!(
            GameClock::REAL.day_start(birth, 1, rolled),
            crate::seed::day_start(birth, 1)
        );
        assert_eq!(
            GameClock::new(24).unwrap().day_start(birth, 7, rolled),
            rolled
        );
    }

    #[test]
    fn a_registry_runs_at_scale_one_until_set() {
        let mut reg = crate::testkit::registry();
        assert_eq!(of(&reg), GameClock::REAL);
        set(&mut reg, GameClock::new(4).unwrap());
        assert_eq!(of(&reg).scale(), 4);
        assert_eq!(span(&reg, DAY), 6 * HOUR);
    }
}
