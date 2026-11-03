//! **What a bot earns** (PROG-PVE): the realised income a career choice reads,
//! and a pure model of what a fighter's tier pays.
//!
//! # Learned income
//!
//! [`crate::mind::Mind`] already keeps an income ledger per career, an EWMA of
//! ISK per hour over the wallet's change ([`crate::mind::Mind::record`]), and
//! [`crate::mind::Mind::income_of`] replaces a career's prior with it after
//! [`crate::mind::LEDGER_MIN_HOURS`]. Two things made that ledger lie for a
//! bot that progresses, and this module fixes both without touching
//! `career.rs` or `mind.rs`:
//!
//! 1. **Investments are not losses.** A hull, a gun set or a book is ISK
//!    spent on a better income, but the wallet's change counted it as a
//!    negative day: the bot that had just upgraded read as the worst earner
//!    and was pushed out of the career it had upgraded for. [`Tracker`]
//!    takes the wallet at each decision and the ISK the bot laid out on
//!    assets since ([`Tracker::invest`]), and books the change *less* what
//!    was invested.
//! 2. **A better ship earns more at once.** When the class a bot flies
//!    changes, the ledger it built in the old ship is rescaled by the model's
//!    ratio ([`retier`]) instead of taking half a day to catch up.
//!
//! The ledger's active time is the connection's own: the driver drops the
//! tracker on a reconnect, so the hours a bot was offline never count.
//!
//! # The tier prior
//!
//! [`tier_index`] multiplies the prior of the fighting careers by how much
//! more the model says the bot's tier pays than a frigate at level 1, capped
//! at [`TIER_CAP`]. It goes through [`crate::career::Signals::price_index`],
//! which `career::prior_income` already multiplies in, so `career.rs` is
//! unchanged and the seeded mix (frigate tier, factor 1) is too.
//!
//! # The model
//!
//! Pure, from numbers that are **[A]** here and named: a site or a mission
//! pocket of difficulty `d` pays a bounty of [`BOUNTY_SHARE`] of
//! `npc_gen::DIFFICULTY_BUDGET[d-1]`; a kill mission pays on top
//! `k_isk(level)` times the level's ISK base (the median pocket bounty, so
//! [`BOUNTY_SHARE`] x the budget again), a time bonus of `k_bonus` of that
//! inside its window, and loyalty points worth [`LP_REALISED`] of
//! `mission_rewards::ISK_PER_LP` each. The time a pocket takes is
//! [`BASE_MINUTES`] for a frigate, divided by the class's [`DPS`] factor, plus
//! [`OVERHEAD_MINUTES`] of warp, loot and turn-in. Falsified by the realised
//! ledger of a bot at the tier reading far from [`mission_isk_h`] once it has
//! [`crate::mind::LEDGER_MIN_HOURS`].

use crate::career::Signals;
use crate::hull_choice::{CLASSES, Class};
use crate::mind::Ledger;
use std::collections::BTreeMap;

// ---- the realised ledger -----------------------------------------------------

/// **One connection's wallet book**: the wallet and the Unix second of the
/// last decision, and the ISK laid out on assets since.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tracker {
    last: Option<(f64, u64)>,
    invested: f64,
    class: Option<Class>,
}

impl Tracker {
    /// A reconnect: the offline hours never count.
    pub fn reset(&mut self) {
        self.last = None;
        self.invested = 0.0;
    }

    /// ISK laid out on assets (a hull, guns, books), not an income loss.
    pub fn invest(&mut self, isk: f64) {
        if isk.is_finite() && isk > 0.0 {
            self.invested += isk;
        }
    }

    /// **The decision's booking**: `(earned, hours)` since the last decision,
    /// the wallet's change plus what was invested, or `None` the first time
    /// (and with no wallet reading). Starts the next interval at `now`.
    pub fn step(&mut self, wallet: Option<f64>, now: u64) -> Option<(f64, f64)> {
        let booked = match (wallet, self.last) {
            (Some(w), Some((w0, t0))) => Some((
                w - w0 + self.invested,
                now.saturating_sub(t0) as f64 / 3600.0,
            )),
            _ => None,
        };
        self.last = wallet.map(|w| (w, now));
        self.invested = 0.0;
        booked
    }

    /// The class the bot flew at the last decision, set to `now`; the one
    /// before it when it changed.
    pub fn moved(&mut self, now: Class) -> Option<Class> {
        let before = self.class.replace(now);
        before.filter(|b| *b != now)
    }
}

// ---- the model ---------------------------------------------------------------

/// R5's dungeon budget by difficulty 1-5 (`npc_gen::DIFFICULTY_BUDGET`).
pub const BUDGET: [f64; 5] = eve_server::npc_gen::DIFFICULTY_BUDGET;
/// What a pocket's squads really pay, as a share of its budget **[A]**: the
/// budget is the ceiling every squad sum stays under
/// (`mission_rewards::isk_base`). Falsified by a bot's realised bounty per
/// pocket far off this share.
pub const BOUNTY_SHARE: f64 = 0.5;
/// `k_isk` of the security scale by level 1-5, `k_bonus`, `k_lp` and the
/// bonus window (minutes) **[A]**, copied from `content/agents/rewards.tsv`
/// (a test keeps them equal).
pub const K_ISK: [f64; 5] = [1.0, 1.5, 2.0, 3.0, 4.0];
pub const K_BONUS: f64 = 0.5;
pub const K_LP: f64 = 0.5;
pub const BONUS_MINUTES: [f64; 5] = [60.0, 120.0, 180.0, 240.0, 300.0];
/// The ISK one LP stands for: `mission_rewards::ISK_PER_LP` (a private
/// module of the server, so mirrored here **[A]**).
pub const ISK_PER_LP: f64 = 1_000.0;
/// The share of [`ISK_PER_LP`] a bot realises spending LP at the store
/// **[A]**: offers are priced in ISK as well as LP. Falsified by LP spent at
/// a store whose offers return far from this share of their LP cost.
pub const LP_REALISED: f64 = 0.5;
/// Minutes a frigate takes to clear a pocket of difficulty 1-5 **[A]**.
pub const BASE_MINUTES: [f64; 5] = [10.0, 14.0, 20.0, 28.0, 40.0];
/// Damage output of a class's fit, a frigate's as 1 **[A]**: two small guns,
/// three, three medium, four medium, four large (`fit_plan`), each with the
/// hull's own bonus and slots. Falsified by a bot whose kill times do not
/// fall with its class.
pub const DPS: [f64; CLASSES] = [1.0, 1.5, 2.6, 4.0, 6.0];
/// Minutes of warp, loot, travel and turn-in around a pocket **[A]**.
pub const OVERHEAD_MINUTES: f64 = 8.0;
/// The biggest factor [`tier_index`] gives a career's prior **[A]**: the
/// model's top tier pays a hundred times a frigate's L1, which would make
/// every bot with a battleship a fighter before its ledger says so.
pub const TIER_CAP: f64 = 3.0;

/// The hardest difficulty a class runs (`site_runner::difficulty_cap`).
pub fn cap_of(class: Class) -> i64 {
    match class {
        Class::Frigate => 2,
        Class::Destroyer => 3,
        Class::Cruiser | Class::Battlecruiser => 4,
        Class::Battleship => 5,
    }
}

/// The agent level a class runs (`missioner::HULL_LEVELS`): frigates 1,
/// destroyers 2, cruisers 3, battlecruisers and battleships 4.
pub fn level_of(class: Class) -> i64 {
    match class {
        Class::Frigate => 1,
        Class::Destroyer => 2,
        Class::Cruiser => 3,
        Class::Battlecruiser | Class::Battleship => 4,
    }
}

/// What one run of a pocket pays and takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    /// The rats' bounties.
    pub bounty: f64,
    /// The agent's reward (0 for an anomaly).
    pub reward: f64,
    /// The time bonus inside its window.
    pub bonus: f64,
    /// Loyalty points, as ISK at [`LP_REALISED`].
    pub lp: f64,
    pub minutes: f64,
}

impl Run {
    pub fn total(&self) -> f64 {
        self.bounty + self.reward + self.bonus + self.lp
    }

    /// ISK per hour.
    pub fn isk_h(&self) -> f64 {
        self.total() / (self.minutes / 60.0)
    }
}

fn at(table: &[f64], d: i64) -> f64 {
    table[(d.clamp(1, 5) - 1) as usize]
}

/// **The minutes** a class takes to clear a pocket of difficulty `d`.
pub fn pocket_minutes(class: Class, d: i64) -> f64 {
    at(&BASE_MINUTES, d) / DPS[class as usize]
}

/// **An anomaly** of difficulty `d` run by `class` (pure): the bounty only.
pub fn site(class: Class, d: i64) -> Run {
    Run {
        bounty: BOUNTY_SHARE * at(&BUDGET, d),
        reward: 0.0,
        bonus: 0.0,
        lp: 0.0,
        minutes: pocket_minutes(class, d) + OVERHEAD_MINUTES,
    }
}

/// **A kill mission** of agent level `level` run by `class` (pure): the
/// pocket's bounty (difficulty = level **[A]**), the reward, the bonus when
/// the run fits the window, and the LP.
pub fn mission(class: Class, level: i64) -> Run {
    let minutes = pocket_minutes(class, level) + OVERHEAD_MINUTES;
    let base = BOUNTY_SHARE * at(&BUDGET, level);
    let reward = at(&K_ISK, level) * base;
    let bonus = if minutes <= at(&BONUS_MINUTES, level) {
        K_BONUS * reward
    } else {
        0.0
    };
    // The server pays `k_lp x reward / R` points; a point is worth `R` x the
    // share the store realises.
    let points = K_LP * reward / ISK_PER_LP;
    let lp = points * ISK_PER_LP * LP_REALISED;
    Run {
        bounty: base,
        reward,
        bonus,
        lp,
        minutes,
    }
}

/// ISK per hour of a mission runner at `class`, taking the level the class
/// runs and the standings allow (`level` is the cap the standings give, 1-5).
pub fn mission_isk_h(class: Class, level: i64) -> f64 {
    mission(class, level.min(level_of(class)).max(1)).isk_h()
}

/// ISK per hour of a site runner at `class`: the hardest anomaly it runs.
pub fn site_isk_h(class: Class) -> f64 {
    site(class, cap_of(class)).isk_h()
}

/// ISK per hour of a belt ratter at `class`: difficulty 1 pockets at the
/// class's speed **[A]** (a belt's rats do not scale with the ship).
pub fn ratter_isk_h(class: Class) -> f64 {
    site(class, 1).isk_h()
}

/// The careers the model prices, by name (`career::CAREERS`).
pub const FIGHTING: [&str; 3] = ["ratter", "site_runner", "mission_runner"];

/// The model's income of a fighting `career` at `class` (the mission runner
/// at the level the class runs), or `None` for any other.
pub fn model_isk_h(career: &str, class: Class) -> Option<f64> {
    match career {
        "ratter" => Some(ratter_isk_h(class)),
        "site_runner" => Some(site_isk_h(class)),
        "mission_runner" => Some(mission_isk_h(class, level_of(class))),
        _ => None,
    }
}

/// **How much more a class pays than a frigate** in a fighting career,
/// capped at [`TIER_CAP`] and never under 1.
pub fn tier_factor(career: &str, class: Class) -> f64 {
    match (
        model_isk_h(career, class),
        model_isk_h(career, Class::Frigate),
    ) {
        (Some(now), Some(base)) if base > 0.0 => (now / base).clamp(1.0, TIER_CAP),
        _ => 1.0,
    }
}

/// **The tier on the prior** (pure): `signals` with each fighting career's
/// `price_index` multiplied by [`tier_factor`] of `class`.
pub fn tier_index(mut signals: Signals, class: Class) -> Signals {
    for career in FIGHTING {
        let f = tier_factor(career, class);
        if f != 1.0 {
            *signals.price_index.entry(career.to_owned()).or_insert(1.0) *= f;
        }
    }
    signals
}

/// **A class change on the ledger** (pure): each fighting career's learned
/// rate times the model's ratio of the new class to the old one, so the
/// income a bot reads follows its new ship at once. A career with no ledger
/// is left alone.
pub fn retier(income: &mut BTreeMap<String, Ledger>, from: Class, to: Class) {
    for career in FIGHTING {
        let (Some(a), Some(b), Some(l)) = (
            model_isk_h(career, from),
            model_isk_h(career, to),
            income.get_mut(career),
        ) else {
            continue;
        };
        if a > 0.0 && b > 0.0 {
            l.isk_h *= b / a;
        }
    }
}

/// **The `[soc]` progress line** (PROG-PVE2, pure): where the bot stands on
/// the ladder and what it really earns, so a soak shows progression over
/// time. `isk_h` and `over` are the realised ledger of the bot's current
/// career ([`Mind::income`](crate::mind::Mind)) and the hours it rests on;
/// `model` the pure model's figure for the career at the bot's class
/// ([`model_isk_h`], `-` for a career it does not price); `ledgers` every
/// career's realised rate, `isk/h:hours`.
pub fn progress_line(
    name: &str,
    career: &str,
    rung: &crate::progression::Rung,
    wallet: Option<f64>,
    income: &BTreeMap<String, Ledger>,
) -> String {
    let own = income.get(career);
    let realised = own.map_or_else(|| "-".to_owned(), |l| format!("{:.0}", l.isk_h));
    let hours = own.map_or(0.0, |l| l.hours);
    let model =
        model_isk_h(career, rung.class).map_or_else(|| "-".to_owned(), |m| format!("{m:.0}"));
    let ledgers: Vec<String> = income
        .iter()
        .map(|(c, l)| format!("{c}={:.0}:{:.1}h", l.isk_h, l.hours))
        .collect();
    format!(
        "[soc] {name} progress class {:?} race {:?} weapon {:?} career {career} isk_h {realised} over {hours:.1}h model {model} wallet {} ledgers {}",
        rung.class,
        rung.race,
        rung.weapon,
        wallet.map_or_else(|| "-".to_owned(), |w| format!("{w:.0}")),
        if ledgers.is_empty() {
            "-".to_owned()
        } else {
            ledgers.join(",")
        },
    )
}

/// **The population's realised income** (PROG-PVE2, pure): per career the
/// mean ISK/h over the bots whose ledger there rests on at least
/// [`crate::mind::LEDGER_MIN_HOURS`], as `income career=mean/n,...` (`income
/// -` when no ledger is old enough yet). It rides on the periodic `[soc]`
/// summary, so a soak shows the societies' income by career over time.
pub fn income_fields(minds: &[&crate::mind::Mind]) -> String {
    let mut by: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
    for m in minds {
        for (career, l) in &m.income {
            if l.hours >= crate::mind::LEDGER_MIN_HOURS && l.isk_h.is_finite() {
                let e = by.entry(career.as_str()).or_default();
                e.0 += l.isk_h;
                e.1 += 1;
            }
        }
    }
    if by.is_empty() {
        return "income -".to_owned();
    }
    let rows: Vec<String> = by
        .iter()
        .map(|(c, (sum, n))| format!("{c}={:.0}/{n}", sum / *n as f64))
        .collect();
    format!("income {}", rows.join(","))
}

/// **The report** (pure): one line per tier with what it earns, for the lane
/// note and `[soc]` debugging.
pub fn report() -> Vec<String> {
    let mut out = Vec::new();
    for class in Class::ALL {
        let level = level_of(class);
        let m = mission(class, level);
        out.push(format!(
            "{class:?} L{level}: mission {:.0} ISK/h (bounty {:.0} + reward {:.0} + bonus {:.0} + LP {:.0} per {:.1} min), site d{} {:.0} ISK/h, belt {:.0} ISK/h",
            m.isk_h(),
            m.bounty,
            m.reward,
            m.bonus,
            m.lp,
            m.minutes,
            cap_of(class),
            site_isk_h(class),
            ratter_isk_h(class),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_investment_is_not_a_loss_and_a_reconnect_starts_over() {
        let mut t = Tracker::default();
        assert_eq!(
            t.step(Some(1_000.0), 0),
            None,
            "the first reading books nothing"
        );
        // Two hours later: 500 richer, 300 of which went into a hull and
        // came out of the wallet before: the wallet is 200 up, income 500.
        t.invest(300.0);
        let (earned, hours) = t.step(Some(1_200.0), 7_200).unwrap();
        assert!((earned - 500.0).abs() < 1e-9 && (hours - 2.0).abs() < 1e-9);
        // Nothing invested the next time.
        let (earned, _) = t.step(Some(1_100.0), 10_800).unwrap();
        assert!((earned + 100.0).abs() < 1e-9);
        // A reconnect drops the interval and what was invested in it.
        t.invest(50.0);
        t.reset();
        assert_eq!(t.step(Some(5.0), 20_000), None);
        let (earned, _) = t.step(Some(5.0), 23_600).unwrap();
        assert_eq!(earned, 0.0);
        // No wallet reading, no booking, and the interval is lost.
        assert_eq!(t.step(None, 30_000), None);
        assert_eq!(t.step(Some(1.0), 31_000), None);
        t.invest(f64::NAN);
        t.invest(-5.0);
        assert_eq!(t.invested, 0.0);
    }

    #[test]
    fn a_class_change_is_reported_once() {
        let mut t = Tracker::default();
        assert_eq!(t.moved(Class::Frigate), None);
        assert_eq!(t.moved(Class::Frigate), None);
        assert_eq!(t.moved(Class::Cruiser), Some(Class::Frigate));
        assert_eq!(t.moved(Class::Cruiser), None);
    }

    #[test]
    fn income_rises_with_class_and_level() {
        for career in FIGHTING {
            let rates: Vec<f64> = Class::ALL
                .iter()
                .map(|c| model_isk_h(career, *c).unwrap())
                .collect();
            assert!(rates.windows(2).all(|w| w[0] < w[1]), "{career}: {rates:?}");
        }
        // A mission pays more per hour at a higher level in the same ship,
        // when the ship is bigger than the level asks.
        assert!(mission(Class::Battleship, 4).isk_h() > mission(Class::Battleship, 1).isk_h());
        // The level a standing allows caps the mission; the class caps it too.
        assert_eq!(
            mission_isk_h(Class::Frigate, 5),
            mission_isk_h(Class::Frigate, 1)
        );
        assert_eq!(
            mission_isk_h(Class::Battleship, 2),
            mission(Class::Battleship, 2).isk_h()
        );
    }

    #[test]
    fn a_level_four_battleship_out_earns_a_level_one_frigate_by_a_wide_margin() {
        let top = mission_isk_h(Class::Battleship, 4);
        let bottom = mission_isk_h(Class::Frigate, 1);
        assert!(top > 20.0 * bottom, "{top} vs {bottom}");
        // The pieces: the reward is k_isk x the ISK base, the bonus a share.
        let m = mission(Class::Battleship, 4);
        assert!((m.reward - K_ISK[3] * m.bounty).abs() < 1e-6);
        assert!((m.bonus - K_BONUS * m.reward).abs() < 1e-6);
        assert!(m.total() > m.reward + m.bounty);
    }

    #[test]
    fn the_bonus_is_paid_exactly_when_the_run_fits_its_window() {
        for class in Class::ALL {
            for level in 1..=5 {
                let m = mission(class, level);
                let fits = m.minutes <= BONUS_MINUTES[(level - 1) as usize];
                assert_eq!(m.bonus > 0.0, fits, "{class:?} L{level}");
            }
        }
        // The windows are long next to a pocket, so today it always is.
        assert!(mission(Class::Frigate, 5).bonus > 0.0);
    }

    #[test]
    fn the_tier_factor_is_one_for_a_frigate_and_capped_for_the_top() {
        for career in FIGHTING {
            assert_eq!(tier_factor(career, Class::Frigate), 1.0);
            for c in Class::ALL {
                let f = tier_factor(career, c);
                assert!((1.0..=TIER_CAP).contains(&f), "{career} {c:?} {f}");
            }
        }
        assert_eq!(tier_factor("miner", Class::Battleship), 1.0);
        assert_eq!(tier_factor("mission_runner", Class::Battleship), TIER_CAP);
    }

    #[test]
    fn the_tier_scales_only_the_fighting_priors_and_leaves_the_frigate_mix_alone() {
        let mut s = Signals::default();
        s.price_index.insert("ratter".to_owned(), 2.0);
        let frigate = tier_index(s.clone(), Class::Frigate);
        assert_eq!(
            frigate.price_index, s.price_index,
            "the seeded mix is unchanged"
        );
        let big = tier_index(s, Class::Battleship);
        assert!(big.price_index["ratter"] > 2.0);
        assert!(big.price_index["site_runner"] > 1.0);
        assert!(!big.price_index.contains_key("miner"));
    }

    #[test]
    fn the_ledger_follows_a_new_ship_at_once() {
        let mut income = BTreeMap::new();
        income.insert(
            "mission_runner".to_owned(),
            Ledger {
                isk_h: 1_000.0,
                hours: 5.0,
            },
        );
        income.insert(
            "miner".to_owned(),
            Ledger {
                isk_h: 7.0,
                hours: 5.0,
            },
        );
        retier(&mut income, Class::Frigate, Class::Cruiser);
        assert!(income["mission_runner"].isk_h > 1_000.0);
        assert_eq!(income["miner"].isk_h, 7.0);
        assert_eq!(income["mission_runner"].hours, 5.0, "the hours stay");
        // Back down again: the same factor undoes it.
        retier(&mut income, Class::Cruiser, Class::Frigate);
        assert!((income["mission_runner"].isk_h - 1_000.0).abs() < 1e-6);
    }

    #[test]
    fn the_constants_match_the_shipped_reward_table() {
        let text = include_str!("../../../../content/agents/rewards.tsv");
        let mut seen = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 || f[0] != "security" {
                continue;
            }
            let level: usize = f[1].parse().unwrap();
            assert_eq!(
                f[2].parse::<f64>().unwrap(),
                K_ISK[level - 1],
                "k_isk L{level}"
            );
            assert_eq!(f[3].parse::<f64>().unwrap(), K_BONUS);
            assert_eq!(f[4].parse::<f64>().unwrap(), K_LP);
            assert_eq!(f[5].parse::<f64>().unwrap(), BONUS_MINUTES[level - 1]);
            seen += 1;
        }
        assert_eq!(seen, 5);
    }

    #[test]
    fn the_class_caps_follow_the_site_runner_and_the_mission_runner() {
        for c in Class::ALL {
            let hull = crate::hull_choice::HULLS
                .iter()
                .find(|h| h.class == c)
                .unwrap()
                .type_id;
            assert_eq!(crate::site_runner::difficulty_cap(hull), Some(cap_of(c)));
        }
        let rows = report();
        assert_eq!(rows.len(), CLASSES);
    }

    #[test]
    fn the_progress_line_names_the_rung_the_realised_rate_and_the_model() {
        use crate::progression::{Race, Rung, weapon_of};
        let rung = Rung {
            race: Race::Amarr,
            class: Class::Cruiser,
            weapon: weapon_of(Race::Amarr, 0),
        };
        let mut income = BTreeMap::new();
        income.insert(
            "ratter".to_owned(),
            Ledger {
                isk_h: 123_456.4,
                hours: 7.25,
            },
        );
        income.insert(
            "miner".to_owned(),
            Ledger {
                isk_h: 900.0,
                hours: 2.0,
            },
        );
        let line = progress_line(
            "Invented Pilot",
            "ratter",
            &rung,
            Some(5_000_000.4),
            &income,
        );
        assert!(
            line.starts_with("[soc] Invented Pilot progress class Cruiser race Amarr weapon Laser")
        );
        assert!(
            line.contains("career ratter isk_h 123456 over 7.2h"),
            "{line}"
        );
        assert!(line.contains(&format!("model {:.0}", ratter_isk_h(Class::Cruiser))));
        assert!(line.contains("wallet 5000000"));
        assert!(line.contains("miner=900:2.0h") && line.contains("ratter=123456:7.2h"));
        // A career with no ledger and no model prints dashes, not zeros.
        let none = progress_line("Invented Pilot", "builder", &rung, None, &BTreeMap::new());
        assert!(
            none.contains("isk_h - over 0.0h model - wallet - ledgers -"),
            "{none}"
        );
    }

    #[test]
    fn the_population_line_averages_only_ledgers_old_enough_to_count() {
        let mut a = crate::mind::Mind::seeded(
            "Invented A",
            1,
            crate::career::CAREERS,
            &Signals::default(),
            0,
        );
        let mut b = a.clone();
        b.name = "Invented B".to_owned();
        a.income.insert(
            "ratter".to_owned(),
            Ledger {
                isk_h: 1_000.0,
                hours: crate::mind::LEDGER_MIN_HOURS,
            },
        );
        b.income.insert(
            "ratter".to_owned(),
            Ledger {
                isk_h: 3_000.0,
                hours: crate::mind::LEDGER_MIN_HOURS + 1.0,
            },
        );
        // Too young to count.
        b.income.insert(
            "miner".to_owned(),
            Ledger {
                isk_h: 9_999.0,
                hours: crate::mind::LEDGER_MIN_HOURS / 2.0,
            },
        );
        assert_eq!(income_fields(&[&a, &b]), "income ratter=2000/2");
        assert_eq!(income_fields(&[]), "income -");
    }

    /// The lane note's table: `cargo test -p eve-bot --lib earnings -- --ignored
    /// --nocapture print_the_report`.
    #[test]
    #[ignore = "prints the model's table for the lane note"]
    fn print_the_report() {
        for line in report() {
            println!("{line}");
        }
        for career in FIGHTING {
            for c in Class::ALL {
                println!("{career} {c:?}: factor {:.2}", tier_factor(career, c));
            }
        }
    }
}
