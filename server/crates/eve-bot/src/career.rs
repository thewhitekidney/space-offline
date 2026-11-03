//! **Careers** (R-SOCIETY §2.2, SOC-A1 = R-ECON E11's career half): the
//! table of things a bot can make a living at, and the utility a bot's
//! [`crate::mind::Traits`] and the market give each one.
//!
//! - **A career is a row** ([`Career`]): its name, the [`RoleSpec`] that
//!   plays it, whether the server and the bot support it today
//!   ([`Support`]), its trait affinities, its security band and an ISK/h
//!   prior. A new activity from stage B is a new row here and nothing else.
//! - **A career the server or the bot cannot do yet is in the table** with
//!   [`Support::Missing`] and the reason. Its utility is 0, so it is never
//!   chosen; the reason is what the `[soc]` line and the lane note show.
//! - **The utility** ([`utility`]):
//!   `U = feasible x max(0, affinity)^(1 - greed) x (income / income_ref)^greed x corp_need`,
//!   with `affinity = base + sum_t w_t x (trait_t - 0.5)`.
//!
//! Everything here is pure. Every weight, prior and band is **[A]** policy:
//! falsified by a `[soc]` career mix the owner says does not look like
//! players (one career eats the population, or a served one is never
//! chosen at neutral prices).

use crate::mind::{TRAIT_COUNT, Traits};
use crate::roles::{self, RoleSpec};
use std::collections::{BTreeMap, BTreeSet};

/// Whether a career can be played today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// The server answers the activity and a bot role plays it.
    Served,
    /// Something is missing; the text says what, and which stage adds it.
    Missing(&'static str),
}

/// The security band a career lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    High,
    Low,
    Null,
    Wormhole,
}

/// One career. Rows compare by `name`, which [`CAREERS`] keeps unique.
#[derive(Debug, Clone, Copy)]
pub struct Career {
    /// Its name in logs and in the state file. Never renamed: a stored
    /// mind names its career by it.
    pub name: &'static str,
    /// The role that plays it. `None` until a stage adds one.
    pub role: Option<&'static RoleSpec>,
    pub support: Support,
    /// `base_c` of the affinity.
    pub base: f64,
    /// `w_ct`, in [`crate::mind::Trait`] order.
    pub affinity: [f64; TRAIT_COUNT],
    pub band: Band,
    /// ISK/h before the bot has its own ledger for the career, at neutral
    /// prices; scaled by [`Signals::price_index`].
    pub prior_isk_h: f64,
}

impl Career {
    /// Served, and a role plays it.
    pub fn served(&self) -> bool {
        self.support == Support::Served && self.role.is_some()
    }

    /// `affinity(c, b)`.
    pub fn affinity(&self, traits: &Traits) -> f64 {
        self.base
            + self
                .affinity
                .iter()
                .zip(traits.0.iter())
                .map(|(w, t)| w * (t - 0.5))
                .sum::<f64>()
    }
}

/// What the world says about careers at one decision: all inputs the bot
/// read, none of them the mind's own. `eve-bots` sets `blocked`; each
/// docked decision fills `price_index` and `corp_need` from the market
/// ([`crate::signal::signals`], SOC-SIG).
#[derive(Debug, Clone, Default)]
pub struct Signals {
    /// `P_mkt / P_ref` of a career's output ([`crate::pricing::p_mkt`]),
    /// by career name. Absent means 1.
    pub price_index: BTreeMap<String, f64>,
    /// Careers this bot cannot do now: no skill within the horizon, no
    /// hull and no ISK for one. By career name.
    pub blocked: BTreeSet<String>,
    /// The corp planner's demand multiplier (R-SOCIETY §3.4), by career
    /// name. Absent means 1.
    pub corp_need: BTreeMap<String, f64>,
}

impl Signals {
    fn index(&self, c: &Career) -> f64 {
        self.price_index.get(c.name).copied().unwrap_or(1.0)
    }

    fn need(&self, c: &Career) -> f64 {
        self.corp_need.get(c.name).copied().unwrap_or(1.0)
    }

    /// `feasible(c, b)`: served and not blocked for this bot.
    pub fn feasible(&self, c: &Career) -> bool {
        c.served() && !self.blocked.contains(c.name)
    }
}

/// The ISK/h a career is worth at neutral prices for the utility's ratio
/// (`income_ref`) **[A]**: falsified with the priors. BOT-TUNE: every prior
/// and this reference are 1/20 of what they were, one factor, so the
/// ratios -- hence the seeded mix and every utility -- are unchanged while
/// the miner's prior ([`MINER_PRIOR_ISK_H`]) is the closed economy's.
pub const INCOME_REF: f64 = 25_000.0;

/// **The miner's prior, ISK/h** (BOT-TUNE) **[A]**, from the closed soak
/// GB-FIX read (tree `700ac48`, `docs/lanes/gb-fix.md`): a miner's trip
/// sold 1,088 Tritanium and Pyerite for about 3,300 ISK (1.57 and 6.28 ISK
/// a unit, the soak's `mkttrade` rows), and the 3-hour log's ten miners
/// that sold at all booked 51,943 ISK in all, 5.2 k each, about 1.7 k an
/// hour of wall clock. A trip every ~7 minutes of play is the ceiling
/// (3,300 x 60/7 = 28 k/h); 20,000 sits between the ceiling and what the
/// soak saw, and is 1/20 of the 400,000 it replaces. Falsified by a closed
/// soak's `[econ]` ledger for miners reading far from 20 k/h once the
/// ledger has [`crate::mind::LEDGER_MIN_HOURS`].
pub const MINER_PRIOR_ISK_H: f64 = 20_000.0;

/// **The reactor's prior, ISK/h** (MN-G) **[A]**: one slot, one run every
/// three hours, a margin of about a tenth of the ~100 k ISK of inputs a
/// formula eats (`econ/prices.tsv`, R-MOON §1.1), less the job's fee: about
/// 3 k ISK/h. Falsified by a closed soak's `[econ]` ledger for reactors
/// reading far from it once the ledger has [`crate::mind::LEDGER_MIN_HOURS`].
pub const REACTOR_PRIOR_ISK_H: f64 = 3_000.0;

/// **The capital pair's prior, ISK/h** (CAP-L) **[A]**: a dreadnought's or
/// carrier's pay is the fights it joins, which no bot picks yet; the prior is
/// the miner's 20 k a bot earns for hours in a hull that costs a thousand
/// times a Merlin. Falsified by a pair's `[econ]` ledger.
pub const CAPITAL_PRIOR_ISK_H: f64 = 20_000.0;

/// **The freighter hauler's prior, ISK/h** (CAP-L) **[A]**: one bulk trip of
/// about a million ISK profit a game day, under 50 k an hour. Falsified by a
/// hauler's ledger.
pub const FREIGHT_PRIOR_ISK_H: f64 = 40_000.0;

/// The careers that need a hull no bot is given (CAP-L, nothing from
/// nowhere): blocked for everybody, and opened per bot by
/// [`crate::capital::lift_blocks`].
pub const CAPITAL_CAREERS: [&str; 3] = ["capital_pilot", "cyno_alt", "freighter_hauler"];

/// Block [`CAPITAL_CAREERS`] in `s`: what `eve-bots` runs with.
pub fn block_capital(s: &mut Signals) {
    for name in CAPITAL_CAREERS {
        s.blocked.insert(name.to_owned());
    }
    // PROG-MINE: the foreman needs a command hull nobody is given
    // ([`crate::foreman::lift_block`] opens it).
    s.blocked.insert(crate::foreman::CAREER.to_owned());
}

/// The prior income of `c` at today's prices.
pub fn prior_income(c: &Career, s: &Signals) -> f64 {
    c.prior_isk_h * s.index(c)
}

/// `U(c, b)` for a bot with `traits` whose income estimate for `c` is
/// `income` ISK/h.
pub fn utility(c: &Career, traits: &Traits, income: f64, s: &Signals) -> f64 {
    if !s.feasible(c) {
        return 0.0;
    }
    let aff = c.affinity(traits);
    let ratio = income / INCOME_REF;
    if aff <= 0.0 || ratio <= 0.0 || !ratio.is_finite() {
        return 0.0;
    }
    let greed = traits.get(crate::mind::Trait::Greed);
    aff.powf(1.0 - greed) * ratio.powf(greed) * s.need(c).max(0.0)
}

/// The career named `name` in `table`.
pub fn find<'a>(table: &'a [Career], name: &str) -> Option<&'a Career> {
    table.iter().find(|c| c.name == name)
}

/// **The mix a society seeds at neutral signals** (SOC-SIG) **[A]**, as
/// shares of the bots, without `--agent` (the mission runner blocked):
/// EVE-like high sec, where most pilots mine or rat, fewer run sites, and
/// a few builders turn many miners' minerals into hulls (one builder's
/// standing bids want more than one miner yields). With `--agent` the
/// mission runner takes about a tenth, mostly from the miners (both are
/// timid). Falsified by the owner's view of a 50-bot `[soc]` mix, or by
/// a market that never clears its minerals (too few builders) or never
/// has hulls (too few miners).
pub const NEUTRAL_MIX: &[(&str, f64)] = &[
    ("miner", 0.40),
    ("ratter", 0.26),
    ("site_runner", 0.18),
    ("builder", 0.16),
];
/// How far a seeded share may sit from [`NEUTRAL_MIX`] (absolute) **[A]**:
/// the fit's own slack.
pub const NEUTRAL_MIX_SLACK: f64 = 0.05;
/// No served career takes more than this share at neutral signals
/// (SOC-A1's falsifier, "one career eats the population") **[A]**.
pub const MIX_MAX_SHARE: f64 = 0.5;

const fn row(
    name: &'static str,
    role: Option<&'static RoleSpec>,
    support: Support,
    base: f64,
    affinity: [f64; TRAIT_COUNT],
    band: Band,
    prior_isk_h: f64,
) -> Career {
    Career {
        name,
        role,
        support,
        base,
        affinity,
        band,
        prior_isk_h,
    }
}

use Band::{High, Low, Null, Wormhole};
use Support::{Missing, Served};

/// **The career table** **[A]** (every number). Trait order: risk, greed,
/// sociability, loyalty, aggression, curiosity, diligence. The served
/// rows are the roles that exist (`roles::ROLES`); the rest wait for the
/// stage their reason names. Coordinator-only once SOC-A2 reads it.
///
/// **The row shape is stable** (SOC-SIG): a stage-B lane serves a row by
/// changing its `role` and `support` only, then fits its `base` so the
/// neutral-signal test still passes (add the career to [`NEUTRAL_MIX`]).
///
/// **The served rows are fitted** (SOC-SIG) so that, at neutral signals
/// and without `--agent`, a society seeds about [`NEUTRAL_MIX`]; the
/// trait weights are twice SOC-A1's, so careers differ enough between
/// bots that a price move shifts part of the population, not all of it.
/// The unserved rows' weights were doubled with them; their bases are
/// SOC-A1's and are fitted when a lane serves them.
pub static CAREERS: &[Career] = &[
    row(
        "miner",
        Some(&roles::MINE),
        Served,
        0.89,
        [-0.8, 0.0, 0.2, 0.2, -0.8, -0.4, 0.8],
        High,
        MINER_PRIOR_ISK_H,
    ),
    row(
        "ratter",
        Some(&roles::ESCORT),
        Served,
        0.58,
        [0.8, 0.2, 0.0, 0.0, 1.0, 0.0, 0.2],
        High,
        30_000.0,
    ),
    row(
        "site_runner",
        Some(&crate::site_runner::ROLE),
        Served,
        0.31,
        [0.6, 0.4, -0.2, 0.0, 0.6, 0.8, 0.0],
        High,
        40_000.0,
    ),
    row(
        "mission_runner",
        Some(&crate::missioner::ROLE),
        Served,
        0.72,
        [-0.4, 0.0, 0.4, 1.2, 0.0, 0.2, 0.0],
        High,
        20_000.0,
    ),
    row(
        "builder",
        Some(&crate::builder::ROLE),
        Served,
        0.33,
        [-0.6, 0.8, 0.2, 0.0, -0.8, 0.0, 1.0],
        High,
        35_000.0,
    ),
    row(
        "reactor",
        // MN-G: buys a formula, runs reactions at a station and sells the
        // products to the builders. A builder's traits (patient, careful,
        // greedy for a margin); its prior is a run's margin over the
        // inputs, ~10% of ~100 k ISK of inputs every three hours, which is
        // far under the miner's (a formula costs 10-15 M ISK and pays back
        // slowly). The base is high because the prior is low (the miner's is
        // 7x it): 1.8 seeds a few of 50 bots at neutral signals out of the
        // builders and miners; 1.5 seeds under 1 %, 1.9 takes the pi
        // farmers under 2 %, 2.5 14 % (the base is steep there).
        Some(&crate::reactor::ROLE),
        Served,
        1.8,
        [-0.4, 0.6, 0.0, 0.0, -0.8, 0.0, 1.0],
        High,
        REACTOR_PRIOR_ISK_H,
    ),
    row(
        "capital_pilot",
        // CAP-L: a dreadnought or carrier flown beside a cyno alt. Blocked
        // for every bot ([`block_capital`]) until it owns the hull (or has
        // the ISK for one) and a partner ([`crate::capital::lift_blocks`]);
        // the traits are a careful, social, patient pilot's.
        Some(&crate::capital::ROLE),
        Served,
        0.0,
        [-0.2, 0.2, 0.8, 0.8, 0.2, 0.0, 0.6],
        Null,
        CAPITAL_PRIOR_ISK_H,
    ),
    row(
        "mining_foreman",
        // PROG-MINE: an Orca (or Porpoise, Rorqual) flown beside a mining
        // fleet, boosting it and compressing its ore. Blocked for every bot
        // ([`block_capital`]) until it owns the hull
        // ([`crate::foreman::lift_block`]): nothing from nowhere. It earns
        // no ISK of its own (the fleet's ledgers show the boost), so its
        // prior is the miner's, and the traits are a sociable, loyal,
        // patient pilot's.
        Some(&crate::foreman::ROLE),
        Served,
        0.0,
        [-0.2, 0.0, 1.0, 0.6, -0.2, 0.0, 0.6],
        High,
        MINER_PRIOR_ISK_H,
    ),
    row(
        "cyno_alt",
        // CAP-L: the pilot's alt. The same gate; it earns nothing by itself,
        // its prior is the pair's share.
        Some(&crate::capital::ALT_ROLE),
        Served,
        0.0,
        [0.0, -0.2, 0.8, 1.0, 0.0, 0.4, 0.6],
        Null,
        CAPITAL_PRIOR_ISK_H / 2.0,
    ),
    row(
        "freighter_hauler",
        // CAP-L (X-FREIGHT): a freighter hauling bulk by gate. Blocked until
        // it owns the hull or has the ISK ([`crate::capital::lift_blocks`]).
        Some(&crate::hauler::ROLE),
        Served,
        0.0,
        [-0.4, 1.0, 0.0, 0.0, -0.6, 0.0, 0.8],
        High,
        FREIGHT_PRIOR_ISK_H,
    ),
    row(
        "pirate",
        // SOC-C5: the hunter plays it; `eve-bots` blocks it unless
        // `--society-pvp`. Fitted so hunters are ~2.5 of 50 at neutral
        // signals, out of the ratters: 0.45 took 1.2, 0.6 took 6.6, 0.75
        // took 14 (`hunt::tests::the_pirate_career_is_a_few_percent_when_unblocked`).
        Some(&crate::hunt::ROLE),
        Served,
        0.5,
        [1.2, 0.4, -0.4, -0.4, 1.6, 0.2, 0.0],
        Low,
        25_000.0,
    ),
    row(
        "ice_miner",
        Some(&crate::ice::ROLE),
        Served,
        // SOC-B1: fitted so ice miners (~2.3 of 50 at neutral) come out of
        // the miners (17.9 -> 15.9), whose traits they share; the base is
        // steep there (0.74: 0.75 of 50, 0.80: 7).
        0.76,
        [-0.6, 0.0, 0.2, 0.2, -0.8, -0.2, 1.0],
        High,
        22_500.0,
    ),
    row(
        "gas_huffer",
        Some(&crate::huffer::ROLE),
        Served,
        // GB-FIX: fitted so huffers are ~2 of 50 at neutral signals, out of
        // the miners and ratters. SOC-B2 left SOC-A1's 0.35, which seeded
        // 0.1 % of bots (none in the stage-B gate soak): 0.5 seeds 3.7 %,
        // 0.6 12.6 %, 0.7 26 % (the base is steep there).
        0.5,
        [0.2, 0.2, 0.0, 0.0, -0.4, 0.8, 0.4],
        Low,
        30_000.0,
    ),
    row(
        "explorer",
        // HK4: the hacker scans data and relic sites down (SOC-B3's
        // probes), hacks their cans and sells the loot. SOC-B3's
        // probe::ROLE stays `--loop explore`. Fitted so explorers are ~2
        // of 50 at neutral signals, mostly out of the site runners (its
        // 900 k ISK/h prior is the table's highest): 0.4 took 20 of 50,
        // 0.05 took 3.5.
        Some(&crate::hacker::ROLE),
        Served,
        -0.05,
        [0.2, 0.2, -0.6, 0.0, -0.2, 1.6, 0.0],
        Low,
        45_000.0,
    ),
    row(
        "wormhole_diver",
        // Y-WH: a Venture and a scoop flown through a wormhole for
        // Fullerite. Fitted so divers are ~1-3 of 50 at neutral signals,
        // out of the explorers and site runners whose curious, bold traits
        // they share: 0.2 took 36 % of the bots, 0.0 17 %, -0.2 6 %, -0.4
        // 1.6 % (the base is steep there).
        Some(&crate::wh_dive::ROLE),
        Served,
        -0.3,
        [1.4, 0.4, 0.0, 0.0, 0.4, 1.4, 0.0],
        Wormhole,
        60_000.0,
    ),
    row(
        "wormhole_raider",
        // T3-SALVAGE: a site runner in a hull that may dive, clearing Sleeper
        // anomalies in J-space and salvaging their wrecks for Ancient
        // Salvage. Aggressive and diligent where the diver is curious; fitted
        // to a few of 50 beside the diver (-0.2 took 4 % and the diver fell
        // under its floor, -0.3 gave 1.8 %; the base is steep here).
        Some(&crate::wh_raid::ROLE),
        Served,
        -0.27,
        [0.8, 0.6, 0.0, 0.0, 1.2, 0.8, 0.4],
        Wormhole,
        60_000.0,
    ),
    row(
        "pi_farmer",
        Some(&crate::pi::ROLE),
        Served,
        // SOC-B6: fitted so farmers are ~2 of 50 at neutral signals, out
        // of the miners and ratters (docs/lanes/soc-b6.md); 0.4 took none.
        1.05,
        [-0.8, 0.2, -0.2, 0.2, -0.6, 0.0, 1.2],
        High,
        12_500.0,
    ),
    row(
        "trader",
        Some(&crate::trader::ROLE),
        Served,
        // merge-b: fitted so traders (~2.4 of 50 at neutral) leave the
        // builders about 7 of 50; 0.4 took 9 traders and left 3 builders.
        0.30,
        [-0.2, 1.2, 0.0, 0.0, -0.6, 0.4, 0.6],
        High,
        35_000.0,
    ),
    row(
        "fw_plexer",
        Some(&crate::plexer::ROLE),
        Served,
        0.3,
        [0.8, 0.0, 0.6, 0.8, 1.0, 0.0, 0.2],
        Low,
        35_000.0,
    ),
    row(
        "soldier",
        None,
        Missing("no relations or wars yet (stage C)"),
        0.2,
        [1.0, 0.0, 1.0, 1.0, 1.4, 0.0, 0.0],
        Null,
        20_000.0,
    ),
    row(
        "sov_grunt",
        None,
        Missing("no sovereignty yet (stage D)"),
        0.2,
        [0.8, 0.0, 1.2, 1.2, 0.8, 0.0, 0.4],
        Null,
        20_000.0,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::Traits;

    #[test]
    fn names_are_unique_and_every_served_row_has_a_role_in_roles() {
        for (i, c) in CAREERS.iter().enumerate() {
            assert!(CAREERS[..i].iter().all(|d| d.name != c.name), "{}", c.name);
            assert!(std::ptr::eq(find(CAREERS, c.name).unwrap(), c));
            if let Some(r) = c.role {
                assert!(
                    roles::parse(r.name).is_some(),
                    "{} plays no ROLES row",
                    c.name
                );
            }
            if c.support == Support::Served {
                assert!(c.role.is_some(), "{} is served with no role", c.name);
            }
            if let Support::Missing(why) = c.support {
                assert!(!why.is_empty(), "{} gives no reason", c.name);
            }
        }
        assert!(CAREERS.iter().any(Career::served));
    }

    /// An infeasible career is never chosen: its utility is 0 whatever the
    /// bot is like and whatever it pays.
    #[test]
    fn an_unserved_or_blocked_career_has_no_utility() {
        let s = Signals::default();
        for seed in 0..200 {
            let t = Traits::draw("Invented Pilot", seed);
            for c in CAREERS.iter().filter(|c| !c.served()) {
                assert_eq!(utility(c, &t, 1e12, &s), 0.0, "{}", c.name);
            }
        }
        let c = CAREERS.iter().find(|c| c.served()).unwrap();
        let mut blocked = Signals::default();
        blocked.blocked.insert(c.name.to_owned());
        let t = Traits([0.5; TRAIT_COUNT]);
        assert_eq!(utility(c, &t, 1e12, &blocked), 0.0);
    }

    /// The seeded mix of `bots` names over `seeds` society seeds, as
    /// shares, at signals `s`.
    pub(crate) fn seeded_mix(s: &Signals, bots: usize, seeds: u64) -> BTreeMap<String, f64> {
        let mut mix: BTreeMap<String, f64> = BTreeMap::new();
        for seed in 0..seeds {
            for i in 0..bots {
                let m =
                    crate::mind::Mind::seeded(&format!("Invented Pilot {i}"), seed, CAREERS, s, 0);
                *mix.entry(m.career).or_default() += 1.0;
            }
        }
        let n = (bots as u64 * seeds) as f64;
        mix.values_mut().for_each(|v| *v /= n);
        mix
    }

    /// **BOT-TUNE: the miner's prior is a closed miner's pay.** Measured in
    /// the closed soak (GB-FIX): 3.3 k ISK a trip, a trip every ~7 minutes
    /// at best (28 k/h, the ceiling), 1.7 k/h as the log booked it. The
    /// prior sits between them and far under the 400 k it was; and the
    /// rescale of every prior kept the miner's ratio to the reference.
    #[test]
    fn the_miner_prior_sits_between_what_the_closed_soak_measured_and_its_ceiling() {
        let miner = find(CAREERS, "miner").expect("a miner row");
        assert_eq!(miner.prior_isk_h, MINER_PRIOR_ISK_H);
        let ceiling = 3_300.0 * 60.0 / 7.0;
        let observed = 51_943.0 / 10.0 / 3.05;
        let prior = miner.prior_isk_h;
        assert!(prior > observed && prior < ceiling);
        assert!(prior <= 400_000.0 / 10.0);
        // The ratio to the reference, which the seeding reads, is the old
        // 400 k to 500 k.
        assert!((MINER_PRIOR_ISK_H / INCOME_REF - 0.8).abs() < 1e-9);
    }

    /// At neutral signals a 50-bot society seeds about [`NEUTRAL_MIX`]
    /// (averaged over seeds); a served career outside it is blocked here,
    /// so a stage-B registration does not move this test, only the next.
    #[test]
    fn at_neutral_signals_a_society_seeds_the_target_mix() {
        let mut s = Signals::default();
        s.blocked.insert("mission_runner".to_owned());
        for c in CAREERS.iter().filter(|c| c.served()) {
            if !NEUTRAL_MIX.iter().any(|(n, _)| *n == c.name) {
                s.blocked.insert(c.name.to_owned());
            }
        }
        let mix = seeded_mix(&s, 50, 20);
        for &(name, want) in NEUTRAL_MIX {
            let got = mix.get(name).copied().unwrap_or(0.0);
            assert!(
                (got - want).abs() <= NEUTRAL_MIX_SLACK,
                "{name}: {got:.3} against {want:.3} ({mix:?})"
            );
        }
    }

    /// No served career eats the population at neutral signals, with or
    /// without the mission runner.
    #[test]
    fn no_served_career_eats_the_population() {
        let mut no_agent = Signals::default();
        no_agent.blocked.insert("mission_runner".to_owned());
        for s in [Signals::default(), no_agent] {
            let mix = seeded_mix(&s, 50, 20);
            for (name, share) in &mix {
                assert!(*share <= MIX_MAX_SHARE, "{name}: {share:.3} ({mix:?})");
            }
        }
    }

    /// The utility grows with income for a greedy bot, and corp need
    /// multiplies it.
    #[test]
    fn utility_grows_with_income_and_corp_need() {
        let c = CAREERS.iter().find(|c| c.served()).unwrap();
        let mut t = Traits([0.5; TRAIT_COUNT]);
        t.0[crate::mind::Trait::Greed as usize] = 0.9;
        let s = Signals::default();
        let lo = utility(c, &t, INCOME_REF, &s);
        let hi = utility(c, &t, 2.0 * INCOME_REF, &s);
        assert!(lo > 0.0 && hi > lo);
        let mut need = Signals::default();
        need.corp_need.insert(c.name.to_owned(), 2.0);
        assert!((utility(c, &t, INCOME_REF, &need) - 2.0 * lo).abs() < 1e-9);
        let mut cheap = Signals::default();
        cheap.price_index.insert(c.name.to_owned(), 0.5);
        assert!((prior_income(c, &cheap) - 0.5 * prior_income(c, &s)).abs() < 1e-6);
    }

    /// The signals `eve-bots` runs with, no `--society-pvp`: the pirate is
    /// blocked, everything else competes.
    fn real_signals() -> Signals {
        let mut s = Signals::default();
        s.blocked.insert("pirate".to_owned());
        block_capital(&mut s);
        s
    }

    /// The share a served career with no row in [`NEUTRAL_MIX`] seeds at
    /// least, at neutral signals (GB-FIX) **[A]**: about one bot in 50.
    /// Falsified by an owner who wants more gas huffers, or none.
    pub(crate) const STAGE_B_MIN_SHARE: f64 = 0.02;
    /// ... and at most.
    pub(crate) const STAGE_B_MAX_SHARE: f64 = 0.08;

    /// GB-FIX: the stage-B gate soak seeded 50 bots with no gas huffer and
    /// no PI farmer. The huffer's base was never fitted (SOC-B2 left
    /// SOC-A1's 0.35, which scores 0.1 % of bots at neutral signals); the
    /// farmer is fitted at ~4 % (one soak in nine has none by luck). Both
    /// are a few of fifty at the signals `eve-bots` runs with.
    #[test]
    fn gas_huffers_and_pi_farmers_are_a_few_of_fifty_at_neutral_signals() {
        let mix = seeded_mix(&real_signals(), 50, 40);
        for name in ["gas_huffer", "pi_farmer"] {
            let share = mix.get(name).copied().unwrap_or(0.0);
            assert!(
                (STAGE_B_MIN_SHARE..=STAGE_B_MAX_SHARE).contains(&share),
                "{name}: {share:.4} ({mix:?})"
            );
        }
    }

    /// MN-G: the reactor is a served career and a few of 50 at neutral
    /// signals (out of the builders, whose traits it shares).
    #[test]
    fn reactors_are_a_few_of_fifty_at_neutral_signals() {
        let reactor = find(CAREERS, "reactor").expect("a reactor row");
        assert!(reactor.served());
        assert_eq!(reactor.prior_isk_h, REACTOR_PRIOR_ISK_H);
        let mix = seeded_mix(&real_signals(), 50, 40);
        let share = mix.get("reactor").copied().unwrap_or(0.0);
        assert!(
            (STAGE_B_MIN_SHARE..=STAGE_B_MAX_SHARE).contains(&share),
            "reactor: {share:.4} ({mix:?})"
        );
    }

    /// ... and they answer their own prices: a gas market at 1.5 x its
    /// reference draws more huffers than at 1, and a PI market at 0.5 x
    /// draws fewer farmers.
    #[test]
    fn a_dearer_gas_market_draws_more_huffers_and_a_cheap_pi_market_fewer_farmers() {
        let at = |name: &str, index: f64| {
            let mut s = real_signals();
            s.price_index.insert(name.to_owned(), index);
            seeded_mix(&s, 50, 20).get(name).copied().unwrap_or(0.0)
        };
        assert!(at("gas_huffer", 1.5) > at("gas_huffer", 1.0));
        assert!(at("pi_farmer", 0.5) < at("pi_farmer", 1.0));
    }

    /// PROG-MINE: the foreman is a served career nobody is seeded into (it
    /// needs a hull nobody is given) until its gate opens.
    #[test]
    fn the_foreman_career_is_blocked_until_a_command_hull_is_owned() {
        let c = find(CAREERS, crate::foreman::CAREER).expect("a row");
        assert!(c.served());
        let t = crate::mind::Traits([0.9; TRAIT_COUNT]);
        let mut s = real_signals();
        assert_eq!(utility(c, &t, 1e9, &s), 0.0, "blocked");
        let hull = crate::ItemRow {
            item_id: 5,
            type_id: crate::mine_ladder::ORCA,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: 941,
            category_id: 6,
            singleton: true,
        };
        crate::foreman::lift_block(&mut s, &[], Some(crate::mine_ladder::HULK));
        assert_eq!(
            utility(c, &t, 1e9, &s),
            0.0,
            "an exhumer is no command ship"
        );
        crate::foreman::lift_block(&mut s, &[hull], None);
        assert!(
            utility(c, &t, 1e9, &s) > 0.0,
            "an Orca in the hangar opens it"
        );
        let m = seeded_mix(&real_signals(), 50, 20);
        assert!(!m.contains_key(crate::foreman::CAREER));
    }

    /// T3-SALVAGE: the wormhole raider is a served career with the raid role,
    /// and a few of 50 at neutral signals without taking the diver's share.
    #[test]
    fn raiders_are_a_few_of_fifty_at_neutral_signals() {
        let raider = find(CAREERS, "wormhole_raider").expect("a raider row");
        assert!(raider.served());
        assert_eq!(raider.role.map(|r| r.name()), Some("raid"));
        let mix = seeded_mix(&real_signals(), 50, 40);
        let share = mix.get("wormhole_raider").copied().unwrap_or(0.0);
        assert!(
            (STAGE_B_MIN_SHARE..=STAGE_B_MAX_SHARE).contains(&share),
            "wormhole_raider: {share:.4} ({mix:?})"
        );
    }

    /// Y-WH: the wormhole diver is a served career with the dive role, and
    /// a few of 50 at neutral signals (out of the explorers and site
    /// runners, whose bold, curious traits it shares).
    #[test]
    fn divers_are_a_few_of_fifty_at_neutral_signals() {
        let diver = find(CAREERS, "wormhole_diver").expect("a diver row");
        assert!(diver.served());
        assert_eq!(diver.role.map(|r| r.name()), Some("dive"));
        let mix = seeded_mix(&real_signals(), 50, 40);
        let share = mix.get("wormhole_diver").copied().unwrap_or(0.0);
        assert!(
            (STAGE_B_MIN_SHARE..=STAGE_B_MAX_SHARE).contains(&share),
            "wormhole_diver: {share:.4} ({mix:?})"
        );
    }
}
