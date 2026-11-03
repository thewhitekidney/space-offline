//! **A bot's mind** (R-SOCIETY §2, SOC-A1): its traits, its career and how
//! it changes career, its goals, its income ledger, and the `[soc]` lines
//! that audit every choice (SOC-A5).
//!
//! A mind is the bot's, not the server's: it never goes on the wire, and
//! the server does not know it. It is persisted bot-side by
//! [`crate::state`].
//!
//! - **Traits** ([`Traits::draw`]) are seven numbers in `[0, 1]`, drawn
//!   once from the character's name and the society seed, each the mean of
//!   three uniform draws, so bell-shaped **[A]** (falsified by the owner
//!   saying the bots feel alike). They never change.
//! - **Career choice** ([`Mind::decide`]) runs at a docked decision point,
//!   never mid-trip; the caller (SOC-A2) decides when that is. The best
//!   other career replaces the current one only when, on
//!   [`SWITCH_CONFIRMATIONS`] evaluations in a row, it beats it by more
//!   than [`HYSTERESIS`], after [`MIN_TENURE`] in the current one, and only
//!   if it was not left within [`COOLDOWN`]. A current career that became
//!   infeasible is left at once.
//! - **Goals** ([`Mind::rescore_goals`]): scored `need x trait weight x
//!   feasibility`; ties keep [`Goal`]'s kind order, then the caller's.
//!
//! Pure: time is a Unix second the caller passes; the dice are
//! [`crate::brain::xorshift`], seeded from [`crate::brain::name_hash`].

use crate::brain::{name_hash, unit, xorshift};
use crate::career::{self, Career, Signals};
use std::collections::BTreeMap;

/// How many traits there are.
pub const TRAIT_COUNT: usize = 7;

/// One trait, `0 ... 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trait {
    /// timid ... reckless
    Risk,
    /// content ... greedy: the weight of ISK/h against affinity
    Greed,
    /// loner ... joiner
    Sociability,
    /// fickle ... loyal
    Loyalty,
    /// pacifist ... warlike
    Aggression,
    /// homebody ... explorer
    Curiosity,
    /// casual ... grinder
    Diligence,
}

/// Every trait, in index order.
pub const TRAITS: [Trait; TRAIT_COUNT] = [
    Trait::Risk,
    Trait::Greed,
    Trait::Sociability,
    Trait::Loyalty,
    Trait::Aggression,
    Trait::Curiosity,
    Trait::Diligence,
];

impl Trait {
    pub fn name(self) -> &'static str {
        match self {
            Trait::Risk => "risk",
            Trait::Greed => "greed",
            Trait::Sociability => "sociability",
            Trait::Loyalty => "loyalty",
            Trait::Aggression => "aggression",
            Trait::Curiosity => "curiosity",
            Trait::Diligence => "diligence",
        }
    }
}

/// A trait vector, indexed by `Trait as usize`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Traits(pub [f64; TRAIT_COUNT]);

/// Uniform draws averaged per trait **[A]**: falsified with the bell shape.
pub const TRAIT_DRAWS: usize = 3;

/// splitmix64's finaliser: spreads a seed so nearby seeds give unrelated
/// dice.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Traits {
    /// The traits of character `name` under society seed `seed`: a pure
    /// function of the two.
    pub fn draw(name: &str, seed: u64) -> Traits {
        let mut rng = mix(name_hash(name) ^ mix(seed)) | 1;
        let mut t = [0.0; TRAIT_COUNT];
        for v in &mut t {
            let sum: f64 = (0..TRAIT_DRAWS).map(|_| unit(xorshift(&mut rng))).sum();
            *v = sum / TRAIT_DRAWS as f64;
        }
        Traits(t)
    }

    pub fn get(&self, t: Trait) -> f64 {
        self.0[t as usize]
    }
}

// ---- the switching rule's constants (all [A]) ------------------------------------

/// How much better the best other career must be, as a fraction of the
/// current one's utility (`HYSTERESIS`) **[A]**. Falsified by `[soc]`
/// showing more than [`FLAP_MAX`] switches per bot-day (too low) or no
/// switch in a week while prices move 2x (too high).
pub const HYSTERESIS: f64 = 0.25;
/// Consecutive evaluations the rival must win (`SWITCH_CONFIRMATIONS`)
/// **[A]**: falsified as [`HYSTERESIS`].
pub const SWITCH_CONFIRMATIONS: u32 = 3;
/// Seconds in the current career before any switch (`MIN_TENURE`), 4 h
/// **[A]**: falsified as [`HYSTERESIS`].
pub const MIN_TENURE: u64 = 4 * 3600;
/// Seconds after leaving a career before it can be re-entered
/// (`COOLDOWN`), 24 h **[A]**: falsified by bots bouncing between two
/// careers day on day.
pub const COOLDOWN: u64 = 24 * 3600;
/// Switches kept in the history (and the state file) **[A]**.
pub const HISTORY_KEEP: usize = 16;
/// Ledger hours on a career before its own income replaces the prior
/// **[A]**: falsified by a bad first hour pushing a bot out of a career
/// that pays (too low), or priors that linger for days (too high).
pub const LEDGER_MIN_HOURS: f64 = 1.0;
/// Ledger hours at which a career's own income fully replaces the prior
/// (FE-CAREER) **[A]**. Between [`LEDGER_MIN_HOURS`] and this the estimate
/// blends the two linearly ([`ledger_weight`]), so one lucky or unlucky first
/// trip neither makes nor breaks a career and the fixed prior is only the
/// cold start. Falsified by `[soc]` showing bots stuck on a career whose
/// ledger is far below its prior for days (too slow), or the same bot
/// flapping on its first hours' noise (too fast).
pub const FE_LEDGER_FULL_HOURS: f64 = 4.0;
/// The income ledger's half-life in hours (EWMA) **[A]**: falsified by a
/// ledger that lags price moves by days (too long) or swings with one
/// trip (too short).
pub const INCOME_HALF_LIFE_H: f64 = 12.0;
/// Most switches per bot-day before `[soc]` calls it flapping (`FLAP_MAX`)
/// **[A]**: R-SOCIETY §2.2's "about 1 switch per bot-day".
pub const FLAP_MAX: f64 = 1.0;
/// A day, seconds.
pub const DAY: u64 = 86_400;

/// **How much of a career's estimate is its own ledger** (FE-CAREER, pure):
/// 0 below [`LEDGER_MIN_HOURS`], 1 from [`FE_LEDGER_FULL_HOURS`], linear
/// between.
pub fn ledger_weight(hours: f64) -> f64 {
    if hours.is_nan() || hours < LEDGER_MIN_HOURS {
        return 0.0;
    }
    ((hours - LEDGER_MIN_HOURS) / (FE_LEDGER_FULL_HOURS - LEDGER_MIN_HOURS)).clamp(0.0, 1.0)
}

/// One career change.
#[derive(Debug, Clone, PartialEq)]
pub struct Switch {
    pub from: String,
    pub to: String,
    pub at: u64,
}

/// One career's income ledger: an EWMA of ISK/h over the hours played.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ledger {
    pub isk_h: f64,
    pub hours: f64,
}

/// A personal regard (R-REL-1): this bot's view of one pilot (or, for a
/// row read from an old state file, of one group), `-R_MAX ..= R_MAX`,
/// negative a grudge. `score` is as of `at` (game seconds) and decays when
/// read ([`crate::relations::regard_decayed`]). Replaces the never-decaying
/// `Grudge`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Regard {
    pub who: i64,
    pub score: f64,
    pub at: u64,
}

/// A stint in a corp (stage C fills it). `left` is `None` while in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stint {
    pub corp: i64,
    pub joined: u64,
    pub left: Option<u64>,
}

/// A goal (R-SOCIETY §2.3). The variant order is the tie order.
#[derive(Debug, Clone, PartialEq)]
pub enum Goal {
    /// Own the next hull of the career's ladder (a type id).
    Rung(i64),
    /// A skill (type id) to a level.
    Skill(i64, u8),
    /// A wallet of this many ISK.
    Wallet(i64),
    FoundCorp,
    /// Join a corp matching a profile name.
    JoinCorp(String),
    /// Claim a system (stage D).
    Claim(i64),
    /// Avenge a group (stage C).
    Avenge(i64),
}

impl Goal {
    /// The kind's tie rank.
    pub fn rank(&self) -> u8 {
        match self {
            Goal::Rung(_) => 0,
            Goal::Skill(..) => 1,
            Goal::Wallet(_) => 2,
            Goal::FoundCorp => 3,
            Goal::JoinCorp(_) => 4,
            Goal::Claim(_) => 5,
            Goal::Avenge(_) => 6,
        }
    }

    /// The trait a goal kind weighs by **[A]**.
    pub fn trait_of(&self) -> Trait {
        match self {
            Goal::Rung(_) | Goal::Wallet(_) => Trait::Greed,
            Goal::Skill(..) => Trait::Diligence,
            Goal::FoundCorp | Goal::JoinCorp(_) => Trait::Sociability,
            Goal::Claim(_) | Goal::Avenge(_) => Trait::Aggression,
        }
    }
}

/// A goal on offer at a decision: how much the bot needs it (`0 ... 1`)
/// and whether it can pursue it now. The caller works both out.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalOption {
    pub goal: Goal,
    pub need: f64,
    pub feasible: bool,
}

/// The floor under a trait's weight, so a trait at 0 halves a goal rather
/// than erasing it **[A]**.
pub const GOAL_WEIGHT_FLOOR: f64 = 0.5;

/// `need x trait weight x feasibility`.
pub fn goal_score(traits: &Traits, o: &GoalOption) -> f64 {
    if !o.feasible || !o.need.is_finite() {
        return 0.0;
    }
    o.need.max(0.0) * (GOAL_WEIGHT_FLOOR + traits.get(o.goal.trait_of()))
}

/// The wallet a bot aims for before anything bigger (`Goal::Wallet`)
/// **[A]**: ten frigates' worth at a few hundred thousand ISK each.
/// Falsified by `[soc] goals` lines where every bot has reached it in a
/// day (too low) or none in a week (too high).
pub const WALLET_GOAL: i64 = 10_000_000;

/// **The goals on offer at a docked decision** (SOC-SIG; R-SOCIETY §2.3,
/// the part this stage can see):
/// - `Rung(hull)`: the career's hull (`hull`, its kit's) while the bot
///   flies and holds none; feasible when the wallet covers its `P_ref`
///   (`hull_price`; unknown means not feasible yet).
/// - `Wallet(`[`WALLET_GOAL`]`)`: need is the share still missing.
///
/// Skills, corps, claims and grudges wait for the stages that observe
/// them.
pub fn career_goals(
    hull: Option<i64>,
    holds_hull: bool,
    wallet: Option<f64>,
    hull_price: Option<f64>,
) -> Vec<GoalOption> {
    let mut out = Vec::new();
    if let Some(h) = hull {
        out.push(GoalOption {
            goal: Goal::Rung(h),
            need: if holds_hull { 0.0 } else { 1.0 },
            feasible: matches!((wallet, hull_price), (Some(w), Some(p)) if w >= p),
        });
    }
    if let Some(w) = wallet {
        out.push(GoalOption {
            goal: Goal::Wallet(WALLET_GOAL),
            need: (1.0 - w / WALLET_GOAL as f64).clamp(0.0, 1.0),
            feasible: true,
        });
    }
    out
}

/// What one career decision did.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The current career stays: nothing beats it by enough.
    Stay,
    /// A rival beats it by enough, `wins` times in a row so far.
    Wait { to: String, wins: u32 },
    /// A rival beats it by enough, but [`MIN_TENURE`] has not passed.
    Tenure { to: String },
    /// The career changed.
    Switch { from: String, to: String },
}

/// A decision and the scores behind it, every career in table order.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub scores: Vec<(&'static str, f64)>,
    pub decision: Decision,
}

/// A bot's mind.
#[derive(Debug, Clone, PartialEq)]
pub struct Mind {
    pub name: String,
    /// The trait seed: the society seed the traits were drawn under.
    pub seed: u64,
    pub traits: Traits,
    pub career: String,
    /// When the current career began, Unix seconds.
    pub since: u64,
    /// The rival winning so far, and its consecutive wins.
    pub pending: Option<(String, u32)>,
    /// The last [`HISTORY_KEEP`] switches, oldest first.
    pub history: Vec<Switch>,
    /// The goal stack; the first is the active goal.
    pub goals: Vec<Goal>,
    /// Income by career name.
    pub income: BTreeMap<String, Ledger>,
    /// Personal regards, at most [`crate::relations::PERSONAL_KEEP`].
    pub regard: Vec<Regard>,
    pub corps: Vec<Stint>,
    /// The bot's home station (SOC-SIG): the first it docked at, kept
    /// across runs so a bot whose last run died mid-trip still goes home.
    pub home: Option<i64>,
}

impl Mind {
    /// A fresh mind: traits from `(name, seed)`, the career with the
    /// highest utility (no hysteresis on the first choice), or the
    /// table's first served career when nothing scores.
    pub fn seeded(name: &str, seed: u64, table: &[Career], s: &Signals, now: u64) -> Mind {
        let mut m = Mind {
            name: name.to_owned(),
            seed,
            traits: Traits::draw(name, seed),
            career: String::new(),
            since: now,
            pending: None,
            history: Vec::new(),
            goals: Vec::new(),
            income: BTreeMap::new(),
            regard: Vec::new(),
            corps: Vec::new(),
            home: None,
        };
        let scores = m.scores(table, s);
        let best = best_of(&scores, |_| true)
            .map(|(n, _)| n)
            .or_else(|| table.iter().find(|c| c.served()).map(|c| c.name))
            .or_else(|| table.first().map(|c| c.name))
            .unwrap_or_default();
        m.career = best.to_owned();
        m
    }

    /// The ISK/h estimate for `c`: the bot's own ledger once it has
    /// [`LEDGER_MIN_HOURS`] there, else the career's prior at today's
    /// prices.
    pub fn income_of(&self, c: &Career, s: &Signals) -> f64 {
        let prior = career::prior_income(c, s);
        match self.income.get(c.name) {
            Some(l) if l.hours >= LEDGER_MIN_HOURS && l.isk_h.is_finite() => {
                let w = ledger_weight(l.hours);
                w * l.isk_h + (1.0 - w) * prior
            }
            _ => prior,
        }
    }

    /// Book `isk` earned over `hours` in career `name`.
    pub fn record(&mut self, name: &str, isk: f64, hours: f64) {
        if !(hours > 0.0 && hours.is_finite() && isk.is_finite()) {
            return;
        }
        let rate = isk / hours;
        let l = self.income.entry(name.to_owned()).or_insert(Ledger {
            isk_h: rate,
            hours: 0.0,
        });
        let alpha = 1.0 - 0.5_f64.powf(hours / INCOME_HALF_LIFE_H);
        l.isk_h += alpha * (rate - l.isk_h);
        l.hours += hours;
    }

    /// `U` of every career in `table`, in table order.
    pub fn scores(&self, table: &[Career], s: &Signals) -> Vec<(&'static str, f64)> {
        table
            .iter()
            .map(|c| {
                let u = career::utility(c, &self.traits, self.income_of(c, s), s);
                (c.name, u)
            })
            .collect()
    }

    /// Was career `name` left within [`COOLDOWN`] of `now`?
    pub fn cooling(&self, name: &str, now: u64) -> bool {
        self.history
            .iter()
            .any(|w| w.from == name && now.saturating_sub(w.at) < COOLDOWN)
    }

    /// Switches in the day before `now`.
    pub fn switches_in_day(&self, now: u64) -> usize {
        self.history
            .iter()
            .filter(|w| now.saturating_sub(w.at) < DAY)
            .count()
    }

    /// One career decision, at a docked decision point.
    pub fn decide(&mut self, table: &[Career], s: &Signals, now: u64) -> Evaluation {
        let scores = self.scores(table, s);
        let current = scores
            .iter()
            .find(|(n, _)| *n == self.career)
            .map(|&(_, u)| u)
            .unwrap_or(0.0);
        let best = best_of(&scores, |n| n != self.career && !self.cooling(n, now));
        let decision = match best {
            None => {
                self.pending = None;
                Decision::Stay
            }
            Some((to, _)) if current <= 0.0 => self.switch(to, now),
            Some((_, u)) if u <= current * (1.0 + HYSTERESIS) => {
                self.pending = None;
                Decision::Stay
            }
            Some((to, _)) if now.saturating_sub(self.since) < MIN_TENURE => {
                self.pending = None;
                Decision::Tenure { to: to.to_owned() }
            }
            Some((to, _)) => {
                let wins = match &self.pending {
                    Some((p, w)) if p == to => w + 1,
                    _ => 1,
                };
                if wins >= SWITCH_CONFIRMATIONS {
                    self.switch(to, now)
                } else {
                    self.pending = Some((to.to_owned(), wins));
                    Decision::Wait {
                        to: to.to_owned(),
                        wins,
                    }
                }
            }
        };
        Evaluation { scores, decision }
    }

    fn switch(&mut self, to: &str, now: u64) -> Decision {
        let from = std::mem::replace(&mut self.career, to.to_owned());
        self.history.push(Switch {
            from: from.clone(),
            to: to.to_owned(),
            at: now,
        });
        if self.history.len() > HISTORY_KEEP {
            let extra = self.history.len() - HISTORY_KEEP;
            self.history.drain(..extra);
        }
        self.since = now;
        self.pending = None;
        Decision::Switch {
            from,
            to: to.to_owned(),
        }
    }

    /// Re-score the goal stack from what is on offer: the stack becomes the
    /// options with a positive score, best first (ties: [`Goal::rank`],
    /// then the order given). Returns the stack with its scores.
    pub fn rescore_goals(&mut self, options: &[GoalOption]) -> Vec<(Goal, f64)> {
        let mut scored: Vec<(Goal, f64)> = options
            .iter()
            .map(|o| (o.goal.clone(), goal_score(&self.traits, o)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by_key(|(g, _)| g.rank());
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        self.goals = scored.iter().map(|(g, _)| g.clone()).collect();
        scored
    }

    /// The active goal.
    pub fn goal(&self) -> Option<&Goal> {
        self.goals.first()
    }
}

/// The highest positive score among names `keep` admits; ties go to the
/// first in table order.
fn best_of(
    scores: &[(&'static str, f64)],
    keep: impl Fn(&str) -> bool,
) -> Option<(&'static str, f64)> {
    let mut best: Option<(&'static str, f64)> = None;
    for &(n, u) in scores {
        if u > 0.0 && keep(n) && best.is_none_or(|(_, b)| u > b) {
            best = Some((n, u));
        }
    }
    best
}

// ---- the [soc] lines (SOC-A5) -------------------------------------------------------

/// The `[soc]` line for one career decision, with every positive score.
pub fn decision_line(m: &Mind, e: &Evaluation) -> String {
    let what = match &e.decision {
        Decision::Stay => "stay".to_owned(),
        Decision::Wait { to, wins } => format!("wait {to} {wins}/{SWITCH_CONFIRMATIONS}"),
        Decision::Tenure { to } => format!("tenure {to}"),
        Decision::Switch { from, to } => format!("switch {from} -> {to}"),
    };
    let scores: Vec<String> = e
        .scores
        .iter()
        .filter(|(_, u)| *u > 0.0)
        .map(|(n, u)| format!("{n}={u:.3}"))
        .collect();
    format!(
        "[soc] {} career {} decision {} scores {}",
        m.name,
        m.career,
        what,
        scores.join(",")
    )
}

/// The `[soc]` line for a goal re-score.
pub fn goal_line(m: &Mind, scored: &[(Goal, f64)]) -> String {
    let goals: Vec<String> = scored
        .iter()
        .map(|(g, s)| format!("{g:?}={s:.3}"))
        .collect();
    format!("[soc] {} goals {}", m.name, goals.join(","))
}

/// The population's `[soc]` summary: bots, sessions online, the career
/// mix, switches per bot-day over the last day against [`FLAP_MAX`].
pub fn summary_line(minds: &[&Mind], online: usize, now: u64) -> String {
    let mut mix: BTreeMap<&str, usize> = BTreeMap::new();
    for m in minds {
        *mix.entry(m.career.as_str()).or_default() += 1;
    }
    let mix: Vec<String> = mix.iter().map(|(c, n)| format!("{c}={n}")).collect();
    let switches: usize = minds.iter().map(|m| m.switches_in_day(now)).sum();
    let per_day = if minds.is_empty() {
        0.0
    } else {
        switches as f64 / minds.len() as f64
    };
    format!(
        "[soc] bots {} online {} mix {} switches_per_bot_day {per_day:.2} flap_max {FLAP_MAX:.2}{}",
        minds.len(),
        online,
        mix.join(","),
        if per_day > FLAP_MAX { " FLAPPING" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::{Band, Support};
    use crate::roles;

    const NEUTRAL: [f64; TRAIT_COUNT] = [0.0; TRAIT_COUNT];

    /// Invented careers: equal priors, affinities on one trait each.
    fn fixture() -> Vec<Career> {
        let mk = |name, aff: [f64; TRAIT_COUNT]| Career {
            name,
            role: Some(&roles::MINE),
            support: Support::Served,
            base: 0.5,
            affinity: aff,
            band: Band::High,
            prior_isk_h: 1000.0,
        };
        let mut explorer = NEUTRAL;
        explorer[Trait::Curiosity as usize] = 2.0;
        explorer[Trait::Risk as usize] = 0.2;
        let mut miner = NEUTRAL;
        miner[Trait::Risk as usize] = -0.5;
        miner[Trait::Curiosity as usize] = -0.5;
        vec![mk("t_miner", miner), mk("t_explorer", explorer)]
    }

    fn traits(set: &[(Trait, f64)]) -> Traits {
        let mut t = Traits([0.5; TRAIT_COUNT]);
        for &(k, v) in set {
            t.0[k as usize] = v;
        }
        t
    }

    fn mind_with(t: Traits, career: &str, since: u64) -> Mind {
        let mut m = Mind::seeded("Invented Pilot", 7, &fixture(), &Signals::default(), since);
        m.traits = t;
        m.career = career.to_owned();
        m.since = since;
        m
    }

    #[test]
    fn traits_are_a_pure_function_of_name_and_seed_and_stay_in_range() {
        let a = Traits::draw("Invented Pilot", 42);
        assert_eq!(a, Traits::draw("Invented Pilot", 42));
        assert_ne!(a, Traits::draw("Invented Pilot", 43));
        assert_ne!(a, Traits::draw("Other Pilot", 42));
        for seed in 0..500 {
            for v in Traits::draw("Invented Pilot", seed).0 {
                assert!((0.0..1.0).contains(&v));
            }
        }
    }

    #[test]
    fn two_bots_with_the_same_seed_and_name_are_identical() {
        let t = crate::career::CAREERS;
        let s = Signals::default();
        assert_eq!(
            Mind::seeded("Invented Pilot", 9, t, &s, 100),
            Mind::seeded("Invented Pilot", 9, t, &s, 100)
        );
    }

    #[test]
    fn a_curious_timid_bot_picks_the_explorer_over_the_miner_at_equal_income() {
        let table = fixture();
        let m = mind_with(
            traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]),
            "",
            0,
        );
        let scores = m.scores(&table, &Signals::default());
        assert_eq!(best_of(&scores, |_| true).unwrap().0, "t_explorer");
        let mut fresh = Mind::seeded("x", 1, &table, &Signals::default(), 0);
        fresh.traits = m.traits;
        fresh.career.clear();
        let e = fresh.decide(&table, &Signals::default(), 0);
        assert!(matches!(e.decision, Decision::Switch { ref to, .. } if to == "t_explorer"));
    }

    /// A rival just over the threshold switches only on the
    /// `SWITCH_CONFIRMATIONS`th consecutive win; a lost evaluation resets
    /// the count.
    #[test]
    fn a_rival_over_the_threshold_needs_consecutive_confirmations() {
        let table = fixture();
        let t = traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]);
        let mut m = mind_with(t, "t_miner", 0);
        let now = MIN_TENURE;
        let s = Signals::default();
        let u = m.scores(&table, &s);
        assert!(
            u[1].1 > u[0].1 * (1.0 + HYSTERESIS),
            "fixture: the rival wins"
        );
        for k in 1..SWITCH_CONFIRMATIONS {
            let e = m.decide(&table, &s, now + u64::from(k));
            assert_eq!(
                e.decision,
                Decision::Wait {
                    to: "t_explorer".into(),
                    wins: k
                }
            );
        }
        // One evaluation where the miner pays enough resets the count.
        let mut rich = Signals::default();
        rich.price_index.insert("t_miner".into(), 1e6);
        assert_eq!(m.decide(&table, &rich, now + 50).decision, Decision::Stay);
        for k in 1..SWITCH_CONFIRMATIONS {
            assert!(matches!(
                m.decide(&table, &s, now + 100 + u64::from(k)).decision,
                Decision::Wait { wins, .. } if wins == k
            ));
        }
        let e = m.decide(&table, &s, now + 200);
        assert!(matches!(e.decision, Decision::Switch { .. }));
        assert_eq!(m.career, "t_explorer");
        assert_eq!(m.since, now + 200);
        assert_eq!(m.history.len(), 1);
    }

    #[test]
    fn a_rival_under_the_threshold_never_switches() {
        let table = fixture();
        let mut m = mind_with(Traits([0.5; TRAIT_COUNT]), "t_miner", 0);
        let s = Signals::default();
        // Neutral traits: both affinities are the base, so the rival only ties.
        for k in 0..10 * u64::from(SWITCH_CONFIRMATIONS) {
            assert_eq!(
                m.decide(&table, &s, MIN_TENURE + k).decision,
                Decision::Stay
            );
        }
    }

    #[test]
    fn min_tenure_blocks_a_switch() {
        let table = fixture();
        let t = traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]);
        let mut m = mind_with(t, "t_miner", 1000);
        for k in 0..10 * u64::from(SWITCH_CONFIRMATIONS) {
            let d = m.decide(&table, &Signals::default(), 1000 + k).decision;
            assert!(matches!(d, Decision::Tenure { .. }), "{d:?}");
        }
        assert_eq!(m.career, "t_miner");
    }

    #[test]
    fn a_career_left_inside_the_cooldown_is_not_re_entered() {
        let table = fixture();
        let t = traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]);
        let mut m = mind_with(t, "t_miner", 0);
        m.history.push(Switch {
            from: "t_explorer".into(),
            to: "t_miner".into(),
            at: 0,
        });
        let s = Signals::default();
        for k in 0..10 {
            assert_eq!(
                m.decide(&table, &s, MIN_TENURE + k).decision,
                Decision::Stay
            );
        }
        assert!(!m.cooling("t_explorer", COOLDOWN));
    }

    /// An infeasible career is never chosen, and a current career that
    /// becomes infeasible is left at once.
    #[test]
    fn an_infeasible_career_is_never_chosen_and_is_left_at_once() {
        let table = fixture();
        let t = traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]);
        let mut blocked = Signals::default();
        blocked.blocked.insert("t_explorer".into());
        let mut m = mind_with(t, "t_miner", 0);
        for k in 0..10 {
            assert_eq!(
                m.decide(&table, &blocked, MIN_TENURE + k).decision,
                Decision::Stay
            );
        }
        let fresh = Mind::seeded("Invented Pilot", 3, &table, &blocked, 0);
        assert_eq!(fresh.career, "t_miner");
        let mut stuck = Signals::default();
        stuck.blocked.insert("t_miner".into());
        let mut m = mind_with(t, "t_miner", 10);
        assert!(matches!(
            m.decide(&table, &stuck, 11).decision,
            Decision::Switch { .. }
        ));
        // The real table: no fresh bot starts in an unserved career.
        for seed in 0..300 {
            let m = Mind::seeded(
                "Invented Pilot",
                seed,
                crate::career::CAREERS,
                &Signals::default(),
                0,
            );
            assert!(
                crate::career::find(crate::career::CAREERS, &m.career)
                    .unwrap()
                    .served()
            );
        }
    }

    #[test]
    fn the_ledger_replaces_the_prior_after_enough_hours() {
        let table = fixture();
        let mut m = mind_with(Traits([0.5; TRAIT_COUNT]), "t_miner", 0);
        let c = &table[0];
        let s = Signals::default();
        assert_eq!(m.income_of(c, &s), c.prior_isk_h);
        m.record(
            "t_miner",
            3.0 * c.prior_isk_h * LEDGER_MIN_HOURS,
            LEDGER_MIN_HOURS,
        );
        // FE-CAREER: at the minimum hours the prior still stands ...
        assert!((m.income_of(c, &s) - c.prior_isk_h).abs() < 1e-6);
        // ... and a full tenure's worth of ledger replaces it.
        let more = FE_LEDGER_FULL_HOURS - LEDGER_MIN_HOURS;
        m.record("t_miner", 3.0 * c.prior_isk_h * more, more);
        assert!((m.income_of(c, &s) - 3.0 * c.prior_isk_h).abs() < 1e-6);
        m.record("t_miner", 0.0, INCOME_HALF_LIFE_H);
        assert!((m.income_of(c, &s) - 1.5 * c.prior_isk_h).abs() < 1e-6);
    }

    #[test]
    fn the_ledger_blends_in_and_the_prior_is_only_a_cold_start() {
        assert_eq!(ledger_weight(0.0), 0.0);
        assert_eq!(ledger_weight(LEDGER_MIN_HOURS), 0.0);
        assert_eq!(ledger_weight(FE_LEDGER_FULL_HOURS), 1.0);
        assert_eq!(ledger_weight(FE_LEDGER_FULL_HOURS * 10.0), 1.0);
        let mid = ledger_weight((LEDGER_MIN_HOURS + FE_LEDGER_FULL_HOURS) / 2.0);
        assert!((mid - 0.5).abs() < 1e-9, "linear between: {mid}");
        assert_eq!(ledger_weight(f64::NAN), 0.0, "no NaN poisons an estimate");
        // Monotone.
        let mut last = 0.0;
        for i in 0..100 {
            let w = ledger_weight(i as f64 * 0.1);
            assert!(w >= last);
            last = w;
        }
    }

    #[test]
    fn ledger_overrides_prior_faster_when_priced() {
        // A price spike lifts the prior of a career the bot has never played,
        // but not the estimate of one it has a full ledger for: what it
        // really earned stands against what the market says it might.
        let table = fixture();
        let c = &table[0];
        let mut s = Signals::default();
        s.price_index.insert(c.name.to_owned(), 3.0);
        let mut fresh = mind_with(Traits([0.5; TRAIT_COUNT]), "t_explorer", 0);
        assert!((fresh.income_of(c, &s) - 3.0 * c.prior_isk_h).abs() < 1e-6);
        fresh.record(
            c.name,
            c.prior_isk_h * FE_LEDGER_FULL_HOURS,
            FE_LEDGER_FULL_HOURS,
        );
        assert!((fresh.income_of(c, &s) - c.prior_isk_h).abs() < 1e-6);
    }

    #[test]
    fn hysteresis_prevents_flapping() {
        // Two careers whose prices wobble +-10% on alternate hours for two
        // days: well inside HYSTERESIS, so the bot never leaves.
        let table = fixture();
        let mut m = mind_with(Traits([0.5; TRAIT_COUNT]), "t_miner", 0);
        let mut moves = 0;
        for h in 0..48u64 {
            let mut s = Signals::default();
            let wobble = if h % 2 == 0 { 1.1 } else { 0.9 };
            s.price_index.insert("t_explorer".to_owned(), wobble);
            s.price_index.insert("t_miner".to_owned(), 2.0 - wobble);
            let now = MIN_TENURE + h * 3600;
            if matches!(m.decide(&table, &s, now).decision, Decision::Switch { .. }) {
                moves += 1;
            }
        }
        assert_eq!(moves, 0);
        assert!(m.switches_in_day(MIN_TENURE + 48 * 3600) as f64 <= FLAP_MAX);
    }

    #[test]
    fn price_move_changes_career_mix_within_a_day() {
        // 60 bots at neutral signals, then one career's price doubles: some
        // of them (not all) move to it within a day, none more than FLAP_MAX
        // times, and the career they leave is not emptied.
        let table = crate::career::CAREERS;
        let neutral = Signals::default();
        let mut minds: Vec<Mind> = (0..60)
            .map(|i| Mind::seeded(&format!("Invented {i}"), 3, table, &neutral, 0))
            .collect();
        let target = "builder";
        let count = |ms: &[Mind]| ms.iter().filter(|m| m.career == target).count();
        let before = count(&minds);
        let mut s = neutral.clone();
        s.price_index.insert(target.to_owned(), 2.0);
        for h in 0..24u64 {
            for m in &mut minds {
                m.decide(table, &s, MIN_TENURE + h * 3600);
            }
        }
        let after = count(&minds);
        assert!(
            after > before,
            "the dear career gained: {before} -> {after}"
        );
        assert!(after < minds.len(), "but did not eat the society");
        assert!(
            minds
                .iter()
                .all(|m| m.switches_in_day(MIN_TENURE + 24 * 3600) as f64 <= FLAP_MAX)
        );
    }

    #[test]
    fn goals_score_need_times_trait_and_infeasible_ones_drop() {
        let mut m = mind_with(
            traits(&[(Trait::Greed, 1.0), (Trait::Diligence, 0.0)]),
            "t_miner",
            0,
        );
        let opts = [
            GoalOption {
                goal: Goal::Skill(1, 2),
                need: 1.0,
                feasible: true,
            },
            GoalOption {
                goal: Goal::Wallet(10),
                need: 1.0,
                feasible: true,
            },
            GoalOption {
                goal: Goal::FoundCorp,
                need: 1.0,
                feasible: false,
            },
            GoalOption {
                goal: Goal::Rung(5),
                need: 1.0,
                feasible: true,
            },
        ];
        let s = m.rescore_goals(&opts);
        // Greed beats diligence; Rung and Wallet tie on greed, Rung first by rank.
        assert_eq!(
            m.goals,
            vec![Goal::Rung(5), Goal::Wallet(10), Goal::Skill(1, 2)]
        );
        assert_eq!(m.goal(), Some(&Goal::Rung(5)));
        assert!(s[0].1 > s[2].1);
    }

    #[test]
    fn career_goals_offer_the_missing_hull_and_the_wallet() {
        let hull = 9_001;
        let opts = career_goals(Some(hull), false, Some(500.0), Some(400.0));
        assert_eq!(opts[0].goal, Goal::Rung(hull));
        assert!(opts[0].feasible && opts[0].need > 0.0);
        let wallet = &opts[1];
        assert_eq!(wallet.goal, Goal::Wallet(WALLET_GOAL));
        assert!(wallet.need > 0.0 && wallet.need < 1.0);
        // Held, unaffordable or unpriced: no rung to chase.
        let mut m = mind_with(Traits([0.5; TRAIT_COUNT]), "t_miner", 0);
        for o in [
            career_goals(Some(hull), true, Some(500.0), Some(400.0)),
            career_goals(Some(hull), false, Some(300.0), Some(400.0)),
            career_goals(Some(hull), false, Some(500.0), None),
        ] {
            let scored = m.rescore_goals(&o);
            assert!(scored.iter().all(|(g, _)| !matches!(g, Goal::Rung(_))));
        }
        // A rich bot with no hull to want has no goal; no wallet, no goals.
        let rich = career_goals(None, false, Some(2.0 * WALLET_GOAL as f64), None);
        assert!(m.rescore_goals(&rich).is_empty());
        assert!(career_goals(None, false, None, None).is_empty());
    }

    #[test]
    fn the_soc_lines_name_every_field() {
        let table = fixture();
        let t = traits(&[(Trait::Curiosity, 0.95), (Trait::Risk, 0.1)]);
        let mut m = mind_with(t, "t_miner", 0);
        let e = m.decide(&table, &Signals::default(), MIN_TENURE);
        let line = decision_line(&m, &e);
        for f in [
            "[soc]",
            "Invented Pilot",
            "career t_miner",
            "decision wait t_explorer",
            "scores t_miner=",
        ] {
            assert!(line.contains(f), "{line} lacks {f}");
        }
        let other = mind_with(t, "t_explorer", 0);
        let mut moved = m.clone();
        moved.history.push(Switch {
            from: "a".into(),
            to: "b".into(),
            at: 10,
        });
        let line = summary_line(&[&m, &other, &moved], 2, 20);
        for f in [
            "[soc]",
            "bots 3",
            "online 2",
            "mix t_explorer=1,t_miner=2",
            "switches_per_bot_day 0.33",
            "flap_max",
        ] {
            assert!(line.contains(f), "{line} lacks {f}");
        }
        assert!(goal_line(&m, &[(Goal::FoundCorp, 0.5)]).contains("goals FoundCorp=0.500"));
    }
}
