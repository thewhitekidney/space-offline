//! **The leader's daily doctrine choice** (DOC-4, R-DOCTRINE 4, 5, 10, 11).
//!
//! Pure: no socket, no clock, no random (every die is a hash of the group, the
//! day and a seed), no `OnceLock`. The hall calls [`daily`] once per game day
//! per leader (see `corpmind::HallState::doctrine_daily`); everything the
//! score reads comes in through [`Inputs`], so a test builds a world from
//! invented profiles.
//!
//! ```text
//! S(d) =  W_CAN    * can_fly           fighters who can fly d now or within TRAIN_DAYS_MAX
//!      +  W_SUPPLY * supply            what the market at the staging region holds of d's parts
//!      -  W_COST   * ln(1 + cost/budget)
//!      +  W_COUNTER* counter           the matrix against the enemy remembered (0 when none)
//!      +  W_TASTE_eff * taste          the Fancy, which yields to losses
//!      +  W_INERT  * [d == current]    sunk stock
//!      -  W_REARM  * ln(1 + rearm/budget)
//! ```
//!
//! The argmax is a *candidate*. It replaces the current doctrine only after
//! [`LeadKnobs::confirmations`] consecutive daily ticks beating it by
//! [`SWITCH_GAIN`] of the score range, with the current one at least
//! [`LeadKnobs::min_days`] old and the last switch [`LeadKnobs::cooldown_days`]
//! ago. A switch changes `current` and nothing else: members move over by
//! replace-on-loss (DOC-5), and the old doctrine stays legal until
//! [`DoctrineBook::retiring`] runs out.
//!
//! **What DOC-5 reads**: [`DoctrineBook::current`] (the key), the retiring
//! key and its deadline ([`DoctrineBook::standing`]), and
//! `HallState::doctrine_of(corp)`. **What DOC-5 must report to the hall**:
//! `corpmind::Readiness` per bot (skill levels, the ship it holds, its
//! system, its next window); until it does, a bot's skills count as
//! [`P_UNREPORTED`].

use crate::brain::{name_hash, unit, xorshift};
use crate::doctrine::{Doctrine, Family, Layer, Range, Stance};
use crate::doctrine_exchange::{Features, Matrix, versus};
use crate::doctrine_sense::{Lens, Memory, SenseKnobs, Util, apply_utility, infer, reference_ewar};
use crate::hull_choice::Class;
use crate::mind::{Trait, Traits};
use crate::society::Group;
use std::collections::BTreeMap;

// ---- named constants (all [A]; R-DOCTRINE 11) -----------------------------------------------

/// Weights of the score **[A]**. Falsified by a doctrine unflyable by most
/// members that the leader nonetheless picks (raise `W_CAN`).
pub const W_CAN: f64 = 1.5;
pub const W_SUPPLY: f64 = 1.0;
pub const W_COST: f64 = 0.8;
/// The taste weight at founding; it yields to losses ([`LOSS_LEARN`]).
pub const W_TASTE: f64 = 1.0;
pub const W_INERT: f64 = 0.6;
pub const W_REARM: f64 = 0.5;
/// A candidate must beat the current doctrine by this share of the score
/// range **[A]** (mirrors `mind::HYSTERESIS`). Falsified by a leader that
/// flips every day (too low) or one that never changes in a week of losing
/// (too high).
pub const SWITCH_GAIN: f64 = 0.25;
/// Days of training a leader counts as "soon" **[A]**.
pub const TRAIN_DAYS_MAX: f64 = 5.0;
/// How fast taste yields to losses: `W_TASTE * exp(-LOSS_LEARN * ratio)`
/// **[A]**.
pub const LOSS_LEARN: f64 = 1.2;
/// A fighter whose skills the hall does not know counts for this much
/// (a human, or a bot that has not reported yet) **[A]**. Falsified by a
/// human-led corp in a bot alliance whose members never could fly the
/// choice.
pub const P_HUMAN_SKILL: f64 = 0.5;
pub const P_UNREPORTED: f64 = 0.5;
/// What a leader assumes of a part's supply when the market is not read
/// (DOC-6 reads it) **[A]**.
pub const SUPPLY_UNKNOWN: f64 = 0.5;
/// The share of (corp wallet + members' median wallet x members) a leader
/// would put in a fleet **[A]**.
pub const BUDGET_SHARE: f64 = 0.5;
/// The wallet a leader assumes when none is reported **[A]**.
pub const BUDGET_DEFAULT: f64 = 2_000_000_000.0;
/// The share of a fleet expected lost per day, for the stock term **[A]**.
pub const LOSS_SHARE_DAY: f64 = 0.15;
/// Doctrine fleets are bounded by this share of the group's fighters
/// (R-DOCTRINE 11 U3) **[A]**.
pub const FIGHTER_SHARE: f64 = 0.8;
/// The half-life of the loss ledger, days **[A]**.
pub const LOSS_HALF_LIFE_DAYS: f64 = 3.0;
/// The most a loss ratio counts for **[A]** (`LOSS_BAIL` territory).
pub const LOSS_RATIO_CAP: f64 = 3.0;
/// The leader's misjudgement (`COUNTER_PICK`) holds for this many days, so a
/// confirmation streak is not broken by a die thrown every day **[A]**.
pub const PICK_EPOCH_DAYS: f64 = 7.0;
/// How many switches the book keeps.
pub const HISTORY_KEEP: usize = 16;
/// While a size step is open, the class one up scores this much more
/// **[A]**; the step stays open [`ESCALATE_DAYS`].
pub const W_ESCALATE: f64 = 0.8;
pub const ESCALATE_DAYS: f64 = 2.0;
/// The score unit one counter point is worth after `tanh` **[A]**.
pub const DAY_S: u64 = 86_400;

// ---- difficulty (R-DOCTRINE 10: the knobs that touch the choice) ------------------------------

/// **The leader's knobs**: exactly the reaction and counter-accuracy groups
/// of the difficulty table. [`Self::NORMAL`] is the spec's normal column.
/// DOC-8's `Difficulty` hands one out per level (`at`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadKnobs {
    /// Tenure before a doctrine switch, days.
    pub min_days: f64,
    /// Days between switches.
    pub cooldown_days: f64,
    /// Daily ticks the gain must hold.
    pub confirmations: u32,
    /// Days the old doctrine may persist.
    pub force_days: f64,
    /// Weight of the counter score against taste.
    pub w_counter: f64,
    /// Chance the leader takes the argmax (else the second or third best).
    pub counter_pick: f64,
    /// Minimax weight.
    pub counter_risk: f64,
    /// Days of expected losses staged.
    pub stock_days: f64,
    /// REVIEW-FIX2: the share of the fit's cost SRP pays
    /// ([`crate::doctrine_stock::SRP_SHARE`] at every level).
    pub srp_share: f64,
    /// REVIEW-FIX2: the share of the Master Wallet above the reserve the
    /// stock may use ([`crate::doctrine_stock::STOCK_WALLET_SHARE`]).
    pub stock_wallet: f64,
    /// Level 0: one doctrine, never switched.
    pub frozen: bool,
    /// BP-COUNTER: how much of the utility seen on killmails (smartbombs,
    /// drives, disruptors, jammers) the leader believes the enemy fields,
    /// `[0, 1]`. `0` reads none, as before.
    pub counter_read: f64,
}

impl LeadKnobs {
    pub const EASY: LeadKnobs = LeadKnobs {
        min_days: 7.0,
        cooldown_days: 5.0,
        confirmations: 5,
        force_days: 14.0,
        w_counter: 0.5,
        counter_pick: 0.30,
        counter_risk: 0.0,
        stock_days: 1.0,
        srp_share: 0.9,
        stock_wallet: 0.15,
        frozen: false,
        counter_read: 0.3,
    };
    pub const NORMAL: LeadKnobs = LeadKnobs {
        min_days: 3.0,
        cooldown_days: 2.0,
        confirmations: 3,
        force_days: 7.0,
        w_counter: 1.0,
        counter_pick: 0.60,
        counter_risk: 0.3,
        stock_days: 2.0,
        srp_share: crate::doctrine_stock::SRP_SHARE,
        stock_wallet: crate::doctrine_stock::STOCK_WALLET_SHARE,
        frozen: false,
        counter_read: 0.6,
    };
    pub const HARD: LeadKnobs = LeadKnobs {
        min_days: 2.0,
        cooldown_days: 1.0,
        confirmations: 2,
        force_days: 4.0,
        w_counter: 1.6,
        counter_pick: 0.85,
        counter_risk: 0.6,
        stock_days: 4.0,
        srp_share: 0.5,
        stock_wallet: 0.4,
        frozen: false,
        counter_read: 0.85,
    };
    pub const BRUTAL: LeadKnobs = LeadKnobs {
        min_days: 1.0,
        cooldown_days: 0.5,
        confirmations: 1,
        force_days: 2.0,
        w_counter: 2.2,
        counter_pick: 1.0,
        counter_risk: 1.0,
        stock_days: 7.0,
        srp_share: 0.3,
        stock_wallet: 0.5,
        frozen: false,
        counter_read: 1.0,
    };

    /// Level 1 easy .. 4 brutal; 2 is normal; 0 is normal and frozen.
    pub fn at(level: u8) -> LeadKnobs {
        match level {
            0 => LeadKnobs {
                frozen: true,
                counter_read: 0.0,
                ..Self::NORMAL
            },
            1 => Self::EASY,
            3 => Self::HARD,
            4.. => Self::BRUTAL,
            _ => Self::NORMAL,
        }
    }
}

impl Default for LeadKnobs {
    fn default() -> Self {
        Self::NORMAL
    }
}

// ---- taste ---------------------------------------------------------------------------------

/// **A leader's fancy** (R-DOCTRINE 4.4): the doctrine it would fly if it
/// did not have to win. Drawn once from the leader's name and the society
/// seed; it never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fancy {
    pub layer: Layer,
    pub range: Range,
    pub family: Family,
    pub class: Class,
}

const FANCY_FAMILIES: [Family; 4] = [
    Family::Projectile,
    Family::Hybrid,
    Family::Laser,
    Family::Missile,
];
const FANCY_CLASSES: [Class; 5] = [
    Class::Frigate,
    Class::Destroyer,
    Class::Cruiser,
    Class::Battlecruiser,
    Class::Battleship,
];

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A die in `0..1` from a hash of the parts.
fn die(parts: &[u64]) -> f64 {
    let mut h = 0x243f_6a88_85a3_08d3_u64;
    for p in parts {
        h = mix(h ^ mix(*p));
    }
    let mut s = h | 1;
    unit(xorshift(&mut s))
}

impl Fancy {
    /// The fancy of `leader` under society seed `seed`: a pure function.
    pub fn draw(leader: &str, seed: u64) -> Fancy {
        let base = [name_hash(leader), seed];
        let pick =
            |salt: u64, n: usize| ((die(&[base[0], base[1], salt]) * n as f64) as usize).min(n - 1);
        Fancy {
            layer: [Layer::Shield, Layer::Armour][pick(1, 2)],
            range: Range::ALL[pick(2, 3)],
            family: FANCY_FAMILIES[pick(3, FANCY_FAMILIES.len())],
            class: FANCY_CLASSES[pick(4, FANCY_CLASSES.len())],
        }
    }

    /// The inverse of [`Fancy::line`].
    pub fn parse(text: &str) -> Option<Fancy> {
        let parts: Vec<&str> = text.split('/').collect();
        if parts.len() != 4 {
            return None;
        }
        Some(Fancy {
            layer: [Layer::Shield, Layer::Armour, Layer::Hull]
                .into_iter()
                .find(|l| l.name() == parts[0])?,
            range: Range::ALL.into_iter().find(|r| r.name() == parts[1])?,
            family: FANCY_FAMILIES.into_iter().find(|f| f.name() == parts[2])?,
            class: FANCY_CLASSES
                .into_iter()
                .find(|c| crate::doctrine::class_name(*c) == parts[3])?,
        })
    }

    pub fn line(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.layer.name(),
            self.range.name(),
            self.family.name(),
            crate::doctrine::class_name(self.class)
        )
    }
}

/// **Taste** (R-DOCTRINE 4.4): how well `d` fits the leader's fancy and
/// character. `cost_share` is the doctrine's cost per ship over the dearest
/// in the catalog (`0..=1`).
///
/// ```text
/// (1.0 [layer] + 0.7 [range] + 0.3 [family] + 0.3 [class]) x (0.5 + loyalty)
///   + aggression [brawl] + (1 - aggression) [kite]
///   - greed x cost_share + diligence x logi share
/// ```
pub fn taste(d: &Doctrine, f: &Fancy, t: &Traits, cost_share: f64) -> f64 {
    let hit = |b: bool| if b { 1.0 } else { 0.0 };
    let loyal = 0.5 + t.get(Trait::Loyalty);
    let aggression = t.get(Trait::Aggression);
    let own = 1.0 * hit(d.layer == f.layer)
        + 0.7 * hit(d.range == f.range)
        + 0.3 * hit(d.family == f.family)
        + 0.3 * hit(d.class == f.class);
    let stance = match d.stance {
        Stance::Brawl => aggression,
        Stance::Kite => 1.0 - aggression,
        Stance::Skirmish => 0.0,
    };
    own * loyal + stance - t.get(Trait::Greed) * cost_share.clamp(0.0, 1.0)
        + t.get(Trait::Diligence) * d.share(crate::doctrine::Role::Logi)
}

/// `W_TASTE * exp(-LOSS_LEARN * loss_ratio)`: the weight of taste after
/// recent losses. A leader who keeps losing gets pragmatic.
pub fn taste_weight(loss_ratio: f64) -> f64 {
    W_TASTE * (-LOSS_LEARN * loss_ratio.clamp(0.0, LOSS_RATIO_CAP)).exp()
}

// ---- the loss ledger -------------------------------------------------------------------------

/// ISK lost and ISK killed by the group, decaying with [`LOSS_HALF_LIFE_DAYS`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LossLedger {
    pub lost: f64,
    pub killed: f64,
    pub at_s: u64,
}

impl LossLedger {
    fn decay_to(&mut self, now_s: u64, day_s: u64) {
        if now_s > self.at_s {
            let days = (now_s - self.at_s) as f64 / day_s.max(1) as f64;
            let k = 0.5_f64.powf(days / LOSS_HALF_LIFE_DAYS);
            self.lost *= k;
            self.killed *= k;
        }
        self.at_s = now_s;
    }

    /// Add what a battle cost and what it took.
    pub fn note(&mut self, lost: f64, killed: f64, now_s: u64, day_s: u64) {
        self.decay_to(now_s, day_s);
        self.lost += lost.max(0.0);
        self.killed += killed.max(0.0);
    }

    /// `lost / killed`, `0` with nothing lost, [`LOSS_RATIO_CAP`] at most.
    pub fn ratio(&self, now_s: u64, day_s: u64) -> f64 {
        let mut l = *self;
        l.decay_to(now_s, day_s);
        if l.lost <= 1.0 {
            return 0.0;
        }
        (l.lost / l.killed.max(l.lost / LOSS_RATIO_CAP)).min(LOSS_RATIO_CAP)
    }
}

// ---- the book --------------------------------------------------------------------------------

/// **One leader group's doctrine state** (R-DOCTRINE 4.1). Persisted by the
/// hall: `current`, `since`, `last_switch`; the rest is rebuilt (the fancy
/// from the name, the memory from the next battles).
#[derive(Debug, Clone, PartialEq)]
pub struct DoctrineBook {
    pub current: Option<String>,
    /// Game seconds since `current` was chosen.
    pub since: u64,
    pub last_switch: Option<u64>,
    pub fancy: Fancy,
    /// The candidate and its consecutive confirmations: `key -> (n, score)`.
    pub cand: BTreeMap<String, (u32, f64)>,
    /// `(when, from, to)`, newest last, [`HISTORY_KEEP`].
    pub switches: Vec<(u64, String, String)>,
    /// The doctrine just left and when it stops being legal.
    pub retiring: Option<(String, u64)>,
    /// What the group lost and killed lately.
    pub losses: LossLedger,
    /// The enemy as remembered (not persisted).
    pub memory: Memory,
    /// A size step is open until this time (the theatre asked for it).
    pub escalate_until: Option<u64>,
}

impl DoctrineBook {
    pub fn new(fancy: Fancy) -> DoctrineBook {
        DoctrineBook {
            current: None,
            since: 0,
            last_switch: None,
            fancy,
            cand: BTreeMap::new(),
            switches: Vec::new(),
            retiring: None,
            losses: LossLedger::default(),
            memory: Memory::default(),
            escalate_until: None,
        }
    }

    /// The doctrines a member may fly at `now_s`: the current one, and the
    /// one just left until its deadline. DOC-5 reads this.
    pub fn standing(&self, now_s: u64) -> Vec<(&str, Standing)> {
        let mut v = Vec::new();
        if let Some(c) = &self.current {
            v.push((c.as_str(), Standing::Current));
        }
        if let Some((k, until)) = &self.retiring
            && *until > now_s
        {
            v.push((k.as_str(), Standing::Retiring { until: *until }));
        }
        v
    }

    /// Open a size step (a theatre's `loss_ratio` crossed `ESCALATE_ER`).
    pub fn escalate(&mut self, now_s: u64, day_s: u64) {
        self.escalate_until = Some(now_s + (ESCALATE_DAYS * day_s as f64) as u64);
    }
}

/// How a doctrine stands for the members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Current,
    /// Left behind: still legal for owned ships until `until`.
    Retiring {
        until: u64,
    },
}

// ---- the inputs ------------------------------------------------------------------------------

/// A fighter-capable member as the leader knows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pilot {
    /// `skill -> level` as the bot last reported it; `None`: not reported.
    pub levels: Option<BTreeMap<i64, i64>>,
    /// Not a bot: its skills are unknown to a bot leader.
    pub human: bool,
}

/// Days a pilot needs to train some `(skill, level)` pairs.
pub type TrainDays = dyn Fn(&Pilot, &[(i64, i64)]) -> f64;

/// Everything [`daily`] reads besides the book.
pub struct Inputs<'a> {
    pub now_s: u64,
    pub day_s: u64,
    pub seed: u64,
    pub group: Group,
    pub catalog: &'a [Doctrine],
    pub matrix: &'a Matrix,
    pub lens: &'a Lens,
    pub sense: SenseKnobs,
    pub pilots: &'a [Pilot],
    /// Days a pilot needs to train `lacking` `(skill, level)` pairs.
    pub train_days: &'a TrainDays,
    /// How much of the doctrine's parts the market holds (`0..=1`).
    pub supply: &'a dyn Fn(&Doctrine) -> f64,
    /// The ISK value of old-doctrine stock held for `key`.
    pub rearm: &'a dyn Fn(&str) -> f64,
    pub budget: f64,
    pub traits: Traits,
}

/// One doctrine's score, term by term (logged as `[doc] score`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Score {
    pub can: f64,
    pub supply: f64,
    pub cost: f64,
    pub counter: f64,
    pub taste: f64,
    pub inert: f64,
    pub rearm: f64,
    pub escalate: f64,
    pub total: f64,
}

fn missing(levels: &BTreeMap<i64, i64>, need: &[(i64, i64)]) -> Vec<(i64, i64)> {
    need.iter()
        .copied()
        .filter(|(s, l)| levels.get(s).copied().unwrap_or(0) < *l)
        .collect()
}

/// **The fraction of the fighters who can fly `d`** now or within
/// [`TRAIN_DAYS_MAX`] (the damage dealers' fit; support roles are a
/// minority and are filled from whoever can). An unreported pilot counts
/// [`P_UNREPORTED`], a human [`P_HUMAN_SKILL`].
pub fn can_fly(pilots: &[Pilot], d: &Doctrine, train_days: &TrainDays) -> f64 {
    let Some(line) = d.line() else { return 0.0 };
    if pilots.is_empty() {
        return 0.0;
    }
    let sum: f64 = pilots
        .iter()
        .map(|p| match (&p.levels, p.human) {
            (_, true) => P_HUMAN_SKILL,
            (None, _) => P_UNREPORTED,
            (Some(l), _) => {
                let lack = missing(l, &line.skills);
                if lack.is_empty() || train_days(p, &lack) <= TRAIN_DAYS_MAX {
                    1.0
                } else {
                    0.0
                }
            }
        })
        .sum();
    sum / pilots.len() as f64
}

/// The doctrine fleet the group can field: the catalog's target bounded by
/// [`FIGHTER_SHARE`] of its fighters (U3).
pub fn fleet_n(d: &Doctrine, fighters: usize) -> f64 {
    let cap = ((fighters as f64 * FIGHTER_SHARE).floor() as usize).max(1);
    d.fleet.target.min(cap).max(1) as f64
}

// ---- the counter score -----------------------------------------------------------------------

/// The enemy as the leader weighs it: per remembered group its likely
/// doctrines `(catalog index, share)` (best first, at most two used), its
/// weight among the enemies and its confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyMix {
    pub group: Group,
    pub mix: Vec<(usize, f64)>,
    pub weight: f64,
    pub conf: f64,
    /// BP-COUNTER: the utility the group's killmails showed, the most of each
    /// on one ship over its hulls.
    pub util: BTreeMap<Util, u32>,
    /// The inferred doctrines of `mix` as the group has been seen to fly them
    /// (`catalog index -> doctrine with the utility applied`); empty when the
    /// leader reads none, or when it changes nothing.
    pub adjusted: BTreeMap<usize, Doctrine>,
}

/// **What the leader believes the enemy fields**: the memory's groups,
/// each inferred against the catalog (R-DOCTRINE 5). Empty with no memory.
pub fn enemy_mix(book: &DoctrineBook, inp: &Inputs<'_>) -> Vec<EnemyMix> {
    enemy_mix_with(book, inp, 0.0)
}

/// [`enemy_mix`] with the utility the memory holds applied to the inferred
/// doctrines at `read` (BP-COUNTER, [`LeadKnobs::counter_read`]).
pub fn enemy_mix_with(book: &DoctrineBook, inp: &Inputs<'_>, read: f64) -> Vec<EnemyMix> {
    let reference = if read > 0.0 {
        reference_ewar(inp.catalog)
    } else {
        Default::default()
    };
    let mut out = Vec::new();
    for m in book.memory.groups.values() {
        let (top, conf) = infer(m, inp.now_s, &inp.sense, inp.lens, inp.catalog);
        let mix: Vec<(usize, f64)> = top
            .iter()
            .filter_map(|(k, s)| {
                inp.catalog
                    .iter()
                    .position(|d| &d.key == k)
                    .map(|i| (i, *s))
            })
            .collect();
        if mix.is_empty() || conf <= 0.0 {
            continue;
        }
        let strength: f64 = m
            .seen
            .values()
            .map(|s| s.strength(inp.now_s, &inp.sense))
            .sum();
        let mut util: BTreeMap<Util, u32> = BTreeMap::new();
        for s in m.seen.values() {
            for (u, n) in &s.utility {
                let e = util.entry(*u).or_insert(0);
                *e = (*e).max(*n);
            }
        }
        let mut adjusted = BTreeMap::new();
        if read > 0.0 && !util.is_empty() {
            for (j, _) in &mix {
                let d = apply_utility(&inp.catalog[*j], &util, &reference, read);
                if d != inp.catalog[*j] {
                    adjusted.insert(*j, d);
                }
            }
        }
        out.push(EnemyMix {
            group: m.group,
            mix,
            weight: strength,
            conf,
            util,
            adjusted,
        });
    }
    let total: f64 = out.iter().map(|e| e.weight).sum();
    if total > 0.0 {
        for e in &mut out {
            e.weight /= total;
        }
    }
    out
}

/// **The counter score of catalog entry `i`** (R-DOCTRINE 4.3): per enemy
/// group, the blend of the share-weighted mean and the worst case over its
/// two likeliest compositions, `tanh` of the matrix's `ER`, shrunk by the
/// group's confidence. `0` when no enemy is known.
pub fn counter(i: usize, enemies: &[EnemyMix], matrix: &Matrix, risk: f64) -> f64 {
    counter_with(i, enemies, matrix, risk, None)
}

/// [`counter`], where a component the leader has seen carry utility is scored
/// by the exchange against that clone, live (BP-COUNTER), instead of the
/// static matrix. `live` is the catalog and the features the matrix was made
/// with. The matrix itself is never changed.
pub fn counter_with(
    i: usize,
    enemies: &[EnemyMix],
    matrix: &Matrix,
    risk: f64,
    live: Option<(&[Doctrine], Features)>,
) -> f64 {
    let mut total = 0.0;
    for e in enemies {
        let share: f64 = e.mix.iter().map(|m| m.1).sum::<f64>().max(1e-9);
        let er = |j: usize| {
            if let (Some(adj), Some((cat, feat))) = (e.adjusted.get(&j), live)
                && let Some(mine) = cat.get(i)
            {
                return versus(mine, adj, feat).er.tanh();
            }
            matrix
                .er
                .get(i)
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(0.0)
                .tanh()
        };
        let mean = e.mix.iter().map(|(j, s)| er(*j) * s).sum::<f64>() / share;
        let worst = e
            .mix
            .iter()
            .take(2)
            .map(|(j, _)| er(*j))
            .fold(f64::INFINITY, f64::min);
        let worst = if worst.is_finite() { worst } else { mean };
        total += e.weight * e.conf * ((1.0 - risk) * mean + risk * worst);
    }
    total
}

// ---- scoring and choosing --------------------------------------------------------------------

fn escalating(book: &DoctrineBook, now_s: u64) -> bool {
    book.escalate_until.is_some_and(|u| u > now_s)
}

/// **Score the whole catalog** (R-DOCTRINE 4.2). One [`Score`] per entry.
pub fn scores(book: &DoctrineBook, inp: &Inputs<'_>, k: &LeadKnobs) -> Vec<Score> {
    let enemies = enemy_mix_with(book, inp, k.counter_read);
    let risk = (k.counter_risk * (0.5 + inp.traits.get(Trait::Risk))).clamp(0.0, 1.0);
    let w_taste = taste_weight(book.losses.ratio(inp.now_s, inp.day_s));
    let dearest = inp
        .catalog
        .iter()
        .map(Doctrine::cost_per_ship)
        .fold(0.0, f64::max)
        .max(1e-9);
    let cur_idx = book
        .current
        .as_ref()
        .and_then(|c| inp.catalog.iter().position(|d| &d.key == c));
    let cur_class = cur_idx.map(|i| inp.catalog[i].class as usize);
    let budget = inp.budget.max(1.0);
    inp.catalog
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let n = fleet_n(d, inp.pilots.len());
            let can = W_CAN * can_fly(inp.pilots, d, inp.train_days);
            let supply = W_SUPPLY * (inp.supply)(d).clamp(0.0, 1.0);
            let fleet_cost = d.cost_per_ship() * n * (1.0 + k.stock_days * LOSS_SHARE_DAY);
            let cost = -W_COST * (1.0 + fleet_cost / budget).ln();
            let counter = k.w_counter
                * counter_with(
                    i,
                    &enemies,
                    inp.matrix,
                    risk,
                    Some((inp.catalog, Features::default())),
                );
            let taste = w_taste * taste(d, &book.fancy, &inp.traits, d.cost_per_ship() / dearest);
            let is_cur = book.current.as_deref() == Some(d.key.as_str());
            let inert = if is_cur { W_INERT } else { 0.0 };
            let rearm = match &book.current {
                Some(c) if !is_cur => -W_REARM * (1.0 + 0.5 * (inp.rearm)(c) / budget).ln(),
                _ => 0.0,
            };
            let escalate = match cur_class {
                Some(c) if escalating(book, inp.now_s) && d.class as usize == c + 1 => W_ESCALATE,
                _ => 0.0,
            };
            Score {
                can,
                supply,
                cost,
                counter,
                taste,
                inert,
                rearm,
                escalate,
                total: can + supply + cost + counter + taste + inert + rearm + escalate,
            }
        })
        .collect()
}

/// What the daily tick did.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Nothing to choose from.
    Empty,
    /// The first choice.
    First {
        key: String,
    },
    /// Kept the current doctrine.
    Hold,
    /// Level 0: the first doctrine stays for good.
    Frozen,
    /// A better doctrine is being confirmed: `n` of `of` daily ticks.
    Candidate {
        key: String,
        n: u32,
        of: u32,
    },
    /// Confirmed, but the tenure or cooldown is not over.
    Waiting {
        key: String,
    },
    Switch {
        from: String,
        to: String,
    },
}

/// The ranked indices, best first, ties broken by a hash of (group, day,
/// index) so the order is stable within a day.
fn ranked(s: &[Score], group: Group, day: u64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..s.len()).collect();
    idx.sort_by(|&a, &b| {
        s[b].total
            .partial_cmp(&s[a].total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                die(&[group as u64, day, a as u64])
                    .partial_cmp(&die(&[group as u64, day, b as u64]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    idx
}

/// **The daily tick** (R-DOCTRINE 4.5). Scores the catalog, takes the
/// argmax (or, by `COUNTER_PICK`, the second or third when an enemy is
/// known), and changes `book` as the module header says.
pub fn daily(book: &mut DoctrineBook, inp: &Inputs<'_>, k: &LeadKnobs) -> Decision {
    if inp.catalog.is_empty() {
        return Decision::Empty;
    }
    let s = scores(book, inp, k);
    let day = inp.now_s / inp.day_s.max(1);
    let order = ranked(&s, inp.group, day);
    let known = s.iter().any(|x| x.counter != 0.0);
    // The leader's misjudgement, fixed for a stretch of days.
    let epoch = (inp.now_s as f64 / (PICK_EPOCH_DAYS * inp.day_s.max(1) as f64)) as u64;
    let rank = if !known || die(&[inp.group as u64, epoch, inp.seed]) < k.counter_pick {
        0
    } else {
        let r = 1 + (die(&[inp.group as u64, epoch, inp.seed, 7]) * 2.0) as usize;
        r.min(order.len() - 1)
    };
    let pick = order[rank];
    let key = inp.catalog[pick].key.clone();

    let Some(cur) = book.current.clone() else {
        book.current = Some(key.clone());
        book.since = inp.now_s;
        book.cand.clear();
        return Decision::First { key };
    };
    if k.frozen {
        return Decision::Frozen;
    }
    let Some(cur_i) = inp.catalog.iter().position(|d| d.key == cur) else {
        // The current doctrine left the catalog (its parts are no longer
        // makeable): take the best at once.
        let key = inp.catalog[order[0]].key.clone();
        book.switches.push((inp.now_s, cur.clone(), key.clone()));
        trim(&mut book.switches);
        book.retiring = Some((
            cur.clone(),
            inp.now_s + (k.force_days * inp.day_s as f64) as u64,
        ));
        book.current = Some(key.clone());
        book.since = inp.now_s;
        book.last_switch = Some(inp.now_s);
        book.cand.clear();
        return Decision::Switch { from: cur, to: key };
    };
    let hi = s.iter().map(|x| x.total).fold(f64::NEG_INFINITY, f64::max);
    let lo = s.iter().map(|x| x.total).fold(f64::INFINITY, f64::min);
    let gain = s[pick].total - s[cur_i].total;
    if pick == cur_i || gain <= SWITCH_GAIN * (hi - lo) {
        book.cand.clear();
        return Decision::Hold;
    }
    let n = book.cand.get(&key).map_or(0, |c| c.0) + 1;
    book.cand.clear();
    book.cand.insert(key.clone(), (n, s[pick].total));
    if n < k.confirmations {
        return Decision::Candidate {
            key,
            n,
            of: k.confirmations,
        };
    }
    let tenure = inp.now_s.saturating_sub(book.since) as f64 >= k.min_days * inp.day_s as f64;
    let cooled = book
        .last_switch
        .is_none_or(|t| inp.now_s.saturating_sub(t) as f64 >= k.cooldown_days * inp.day_s as f64);
    if !tenure || !cooled {
        return Decision::Waiting { key };
    }
    book.switches.push((inp.now_s, cur.clone(), key.clone()));
    trim(&mut book.switches);
    book.retiring = Some((
        cur.clone(),
        inp.now_s + (k.force_days * inp.day_s as f64) as u64,
    ));
    book.current = Some(key.clone());
    book.since = inp.now_s;
    book.last_switch = Some(inp.now_s);
    book.cand.clear();
    Decision::Switch { from: cur, to: key }
}

fn trim(v: &mut Vec<(u64, String, String)>) {
    while v.len() > HISTORY_KEEP {
        v.remove(0);
    }
}

/// **The `[doc] lead` line**: the leader, the choice and its terms.
pub fn line(book: &DoctrineBook, inp: &Inputs<'_>, k: &LeadKnobs, d: &Decision) -> String {
    let s = scores(book, inp, k);
    let cur = book
        .current
        .as_ref()
        .and_then(|c| inp.catalog.iter().position(|x| &x.key == c))
        .map(|i| s[i]);
    let terms = cur.map_or(String::new(), |x| {
        format!(
            " can={:.2} sup={:.2} cost={:.2} ctr={:.2} taste={:.2} total={:.2}",
            x.can, x.supply, x.cost, x.counter, x.taste, x.total
        )
    });
    format!(
        "[doc] lead group={} current={} fancy={} decision={:?}{terms}",
        inp.group,
        book.current.as_deref().unwrap_or("-"),
        book.fancy.line(),
        d
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctrine::testkit::*;
    use crate::doctrine_exchange::{Features, matrix};
    use crate::doctrine_sense::EnemyMemory;

    const DAY: u64 = 1_000;
    const GROUP: Group = 98_100_001;

    fn traits(loyalty: f64, aggression: f64, greed: f64) -> Traits {
        let mut t = [0.5; crate::mind::TRAIT_COUNT];
        t[Trait::Loyalty as usize] = loyalty;
        t[Trait::Aggression as usize] = aggression;
        t[Trait::Greed as usize] = greed;
        t[Trait::Diligence as usize] = 0.0;
        t[Trait::Risk as usize] = 0.5;
        Traits(t)
    }

    /// The invented catalog with labels set so taste can tell them apart.
    fn labelled() -> Vec<Doctrine> {
        let mut c = invented_catalog();
        let labels = [
            (Layer::Shield, Range::Long, Family::Hybrid, Class::Cruiser),
            (
                Layer::Armour,
                Range::Short,
                Family::Projectile,
                Class::Cruiser,
            ),
            (Layer::Shield, Range::Short, Family::Hybrid, Class::Cruiser),
            (
                Layer::Shield,
                Range::Medium,
                Family::Laser,
                Class::Battleship,
            ),
            (
                Layer::Armour,
                Range::Medium,
                Family::Missile,
                Class::Frigate,
            ),
            (
                Layer::Armour,
                Range::Short,
                Family::Projectile,
                Class::Destroyer,
            ),
        ];
        for (d, (l, r, f, k)) in c.iter_mut().zip(labels) {
            d.layer = l;
            d.range = r;
            d.family = f;
            d.class = k;
            d.stance = Stance::Skirmish;
        }
        // Equal money, so cost does not decide.
        for d in &mut c {
            for r in &mut d.roles {
                r.2.cost = 100.0;
            }
            d.fleet.target = 10;
        }
        c
    }

    struct World {
        cat: Vec<Doctrine>,
        m: Matrix,
        lens: Lens,
        pilots: Vec<Pilot>,
    }

    fn world() -> World {
        let cat = labelled();
        let m = matrix(&cat, Features::default());
        World {
            cat,
            m,
            lens: Lens::default(),
            pilots: (0..12)
                .map(|_| Pilot {
                    levels: Some(BTreeMap::new()),
                    human: false,
                })
                .collect(),
        }
    }

    fn inputs<'a>(w: &'a World, now_s: u64, t: Traits) -> Inputs<'a> {
        Inputs {
            now_s,
            day_s: DAY,
            seed: 11,
            group: GROUP,
            catalog: &w.cat,
            matrix: &w.m,
            lens: &w.lens,
            sense: SenseKnobs::NORMAL,
            pilots: &w.pilots,
            train_days: &|_, _| 0.0,
            supply: &|_| 1.0,
            rearm: &|_| 0.0,
            budget: 1.0e9,
            traits: t,
        }
    }

    fn book_with_fancy(f: Fancy) -> DoctrineBook {
        DoctrineBook::new(f)
    }

    fn fancy_of(d: &Doctrine) -> Fancy {
        Fancy {
            layer: d.layer,
            range: d.range,
            family: d.family,
            class: d.class,
        }
    }

    #[test]
    fn a_leader_picks_its_fancy_at_first() {
        let w = world();
        // The shield / long / hybrid cruiser is index 0; the lone battleship
        // laser fleet is index 3. Each fancy picks its own, with no enemy known.
        for want in [0usize, 2, 3, 4] {
            let mut b = book_with_fancy(fancy_of(&w.cat[want]));
            let d = daily(
                &mut b,
                &inputs(&w, 5 * DAY, traits(0.5, 0.5, 0.0)),
                &LeadKnobs::NORMAL,
            );
            assert_eq!(
                d,
                Decision::First {
                    key: w.cat[want].key.clone()
                },
                "fancy {}",
                b.fancy.line()
            );
        }
    }

    /// A world that has met the armour projectile cruiser fleet (catalog
    /// index 1): the lens knows its hull, gun and plate.
    fn met_the_brick(w: &mut World, now_s: u64) -> Memory {
        use crate::doctrine_fit::Kind;
        use crate::doctrine_sense::Seen;
        w.lens.hull_class.insert(77, Class::Cruiser);
        w.lens
            .module_kind
            .insert(88, Kind::Gun(Family::Projectile, 2));
        w.lens.module_kind.insert(89, Kind::Plate);
        let mut e = EnemyMemory::new(98_500_001);
        e.conf = 0.9;
        e.last_battle_s = now_s;
        e.seen.insert(
            77,
            Seen {
                n_max: 10,
                last_s: now_s,
                weapons: [88].into(),
                tanks: [(Layer::Armour, 3)].into(),
                utility: BTreeMap::new(),
            },
        );
        let mut mem = Memory::default();
        mem.groups.insert(e.group, e);
        mem
    }

    #[test]
    fn losses_make_taste_yield() {
        // The weight is monotone down in the loss ratio ...
        assert_eq!(taste_weight(0.0), W_TASTE);
        assert!(taste_weight(1.0) < taste_weight(0.5));
        assert!(taste_weight(LOSS_RATIO_CAP) < 0.1 * W_TASTE);
        // ... and a leader that fancies a doctrine the enemy beats flies it
        // while it wins, and takes a counter once it keeps losing.
        let mut w = world();
        let now = 20 * DAY;
        let mem = met_the_brick(&mut w, now);
        let t = traits(0.5, 0.5, 0.0);
        let liked = 2;
        let fancy = fancy_of(&w.cat[liked]);
        let k = LeadKnobs {
            w_counter: 0.3,
            counter_pick: 1.0,
            ..LeadKnobs::NORMAL
        };
        let mut calm = book_with_fancy(fancy);
        calm.memory = mem.clone();
        let inp = inputs(&w, now, t);
        assert_eq!(
            enemy_mix(&calm, &inp).len(),
            1,
            "the enemy is placed in the catalog"
        );
        let d = daily(&mut calm, &inp, &k);
        assert!(matches!(d, Decision::First { .. }));
        assert_eq!(
            calm.current.as_deref(),
            Some(w.cat[liked].key.as_str()),
            "it flies its fancy"
        );

        let mut losing = book_with_fancy(fancy);
        losing.memory = mem;
        losing.losses.note(3.0e9, 1.0e9, now, DAY);
        daily(&mut losing, &inputs(&w, now, t), &k);
        let pick = losing.current.clone().unwrap();
        assert_ne!(pick, w.cat[liked].key, "it gave up its fancy");
        let i = w.cat.iter().position(|d| d.key == pick).unwrap();
        assert!(
            w.m.er[i][1] > w.m.er[liked][1],
            "and took a better counter to the brick"
        );
    }

    /// BP-COUNTER: a smartbomb on a killmail moves the counter score of the
    /// doctrines that meet the brick, only for a leader that reads it, and the
    /// static matrix is not touched.
    #[test]
    fn a_seen_bomber_changes_the_counter_score() {
        use crate::doctrine_sense::Util;
        let mut w = world();
        let now = 20 * DAY;
        let mut mem = met_the_brick(&mut w, now);
        // The catalog's own bomb is the reference the seen one is given.
        for (_, _, f) in &mut w.cat[3].roles {
            f.profile.ewar.bomb = [400.0; 4];
            f.profile.ewar.bomb_range = 5_000.0;
        }
        let before = w.m.clone();
        let inp = inputs(&w, now, traits(0.5, 0.5, 0.0));
        let mut book = book_with_fancy(fancy_of(&w.cat[0]));
        book.memory = mem.clone();
        let blind = scores(&book, &inp, &LeadKnobs::NORMAL);
        for e in mem.groups.values_mut() {
            for s in e.seen.values_mut() {
                s.utility.insert(Util::Bomb, 2);
            }
        }
        book.memory = mem;
        let reads = |read: f64| {
            let k = LeadKnobs {
                counter_read: read,
                ..LeadKnobs::NORMAL
            };
            scores(&book, &inp, &k)
        };
        let off: Vec<f64> = reads(0.0).iter().map(|s| s.counter).collect();
        assert_eq!(
            off,
            blind.iter().map(|s| s.counter).collect::<Vec<_>>(),
            "a leader that reads none scores as before"
        );
        let on: Vec<f64> = reads(1.0).iter().map(|s| s.counter).collect();
        assert_ne!(off, on, "a seen bomber moves the score");
        assert_eq!(w.m, before, "the static matrix stays as it was");
    }

    #[test]
    fn unknown_enemy_means_no_counter() {
        let w = world();
        let b = book_with_fancy(fancy_of(&w.cat[0]));
        let inp = inputs(&w, 5 * DAY, traits(0.5, 0.5, 0.0));
        assert!(enemy_mix(&b, &inp).is_empty());
        assert!(
            scores(&b, &inp, &LeadKnobs::NORMAL)
                .iter()
                .all(|s| s.counter == 0.0)
        );
        for i in 0..w.cat.len() {
            assert_eq!(counter(i, &[], &w.m, 1.0), 0.0);
        }
        // A remembered group the catalog cannot place is no enemy either.
        let mut b = b;
        b.memory.groups.insert(5, EnemyMemory::new(5));
        assert!(enemy_mix(&b, &inp).is_empty());
    }

    /// Drive a book through daily ticks where the score favours `to` over
    /// the current doctrine, by making the current one unflyable.
    fn tick(b: &mut DoctrineBook, w: &World, day: u64, k: &LeadKnobs, t: Traits) -> Decision {
        daily(b, &inputs(w, day * DAY, t), k)
    }

    /// Inputs where the doctrine `bad` cannot be flown by anyone and its
    /// parts are off the market.
    fn without<'a>(w: &'a World, day: u64, t: Traits, bad: &'a str) -> Inputs<'a> {
        let mut inp = inputs(w, day * DAY, t);
        inp.train_days = &|_, _| 99.0;
        inp.supply = Box::leak(Box::new(
            move |d: &Doctrine| if d.key == bad { 0.0 } else { 1.0 },
        ));
        inp
    }

    #[test]
    fn a_switch_needs_confirmations_and_tenure() {
        let mut w = world();
        let k = LeadKnobs {
            min_days: 6.0,
            ..LeadKnobs::NORMAL
        };
        let t = traits(0.5, 0.5, 0.0);
        let mut b = book_with_fancy(fancy_of(&w.cat[0]));
        assert!(matches!(tick(&mut b, &w, 1, &k, t), Decision::First { .. }));
        let first = b.current.clone().unwrap();
        // The first doctrine becomes plainly worse: nobody can fly it and its
        // parts are gone. A switch still needs its confirmations ...
        let first_i = w.cat.iter().position(|d| d.key == first).unwrap();
        w.cat[first_i].roles[0].2.skills = vec![(42, 5)];
        let run = |b: &mut DoctrineBook, day: u64| daily(b, &without(&w, day, t, &first), &k);
        let d2 = run(&mut b, 2);
        assert!(matches!(d2, Decision::Candidate { n: 1, .. }), "{d2:?}");
        assert!(matches!(run(&mut b, 3), Decision::Candidate { n: 2, .. }));
        // ... the third confirmation arrives inside the tenure: wait ...
        assert!(matches!(run(&mut b, 4), Decision::Waiting { .. }), "tenure");
        assert_eq!(b.current.as_deref(), Some(first.as_str()));
        // ... and the switch lands once the tenure is over.
        let mut got = None;
        for at in 5..12 {
            if let Decision::Switch { from, to } = run(&mut b, at) {
                got = Some((from, to, at));
                break;
            }
        }
        let (from, to, day) = got.expect("the switch lands");
        assert_eq!(from, first);
        assert_ne!(to, first);
        assert!(
            day >= 7,
            "the tenure (6 days from day 1) is respected: day {day}"
        );
        // The old doctrine stays legal for the force days.
        let st = b.standing(day * DAY);
        assert_eq!(st.len(), 2);
        assert!(matches!(st[1].1, Standing::Retiring { .. }));
        assert_eq!(b.standing((day + 8) * DAY).len(), 1);
    }

    #[test]
    fn no_flapping_in_four_days() {
        let mut w = world();
        let k = LeadKnobs::NORMAL;
        let t = traits(0.5, 0.5, 0.0);
        let mut b = book_with_fancy(fancy_of(&w.cat[0]));
        tick(&mut b, &w, 1, &k, t);
        let a = b.current.clone().unwrap();
        let other = w
            .cat
            .iter()
            .find(|d| d.key != a && d.layer != w.cat[0].layer)
            .unwrap()
            .key
            .clone();
        // Every other day the current doctrine is unflyable and unsupplied;
        // on the others it is fine. The argmax moves every day.
        w.cat.iter_mut().find(|d| d.key == a).unwrap().roles[0]
            .2
            .skills = vec![(42, 5)];
        let (mut switches, mut candidates) = (0, 0);
        for day in 2..=5 {
            let bad: &str = if day % 2 == 0 { &a } else { &other };
            match daily(&mut b, &without(&w, day, t, bad), &k) {
                Decision::Switch { .. } => switches += 1,
                Decision::Candidate { .. } => candidates += 1,
                _ => {}
            }
        }
        assert!(candidates >= 1, "the argmax did move");
        assert_eq!(switches, 0, "confirmations never reached");
        assert_eq!(b.current.as_deref(), Some(a.as_str()));
    }

    #[test]
    fn level_zero_never_switches() {
        let mut w = world();
        let k = LeadKnobs::at(0);
        let t = traits(0.5, 0.5, 0.0);
        let mut b = book_with_fancy(fancy_of(&w.cat[0]));
        tick(&mut b, &w, 1, &k, t);
        let first = b.current.clone().unwrap();
        let i = w.cat.iter().position(|d| d.key == first).unwrap();
        w.cat[i].roles[0].2.skills = vec![(42, 5)];
        for day in 2..30 {
            let train = |_: &Pilot, _: &[(i64, i64)]| 99.0;
            let mut inp = inputs(&w, day * DAY, t);
            inp.train_days = &train;
            assert_eq!(daily(&mut b, &inp, &k), Decision::Frozen);
        }
        assert_eq!(b.current.as_deref(), Some(first.as_str()));
    }

    #[test]
    fn the_knobs_are_monotone_in_level() {
        let l = [
            LeadKnobs::EASY,
            LeadKnobs::NORMAL,
            LeadKnobs::HARD,
            LeadKnobs::BRUTAL,
        ];
        for w in l.windows(2) {
            assert!(w[0].min_days > w[1].min_days);
            assert!(w[0].cooldown_days > w[1].cooldown_days);
            assert!(w[0].confirmations > w[1].confirmations);
            assert!(w[0].force_days > w[1].force_days);
            assert!(w[0].w_counter < w[1].w_counter);
            assert!(w[0].counter_pick < w[1].counter_pick);
            assert!(w[0].counter_risk < w[1].counter_risk);
            assert!(w[0].stock_days < w[1].stock_days);
        }
        assert_eq!(LeadKnobs::at(2), LeadKnobs::NORMAL);
        assert_eq!(LeadKnobs::at(1), LeadKnobs::EASY);
        assert_eq!(LeadKnobs::at(9), LeadKnobs::BRUTAL);
        assert!(LeadKnobs::at(0).frozen);
    }

    #[test]
    fn a_fancy_survives_its_line() {
        for i in 0..30 {
            let f = Fancy::draw(&format!("pilot{i}"), 9);
            assert_eq!(Fancy::parse(&f.line()), Some(f));
        }
        assert_eq!(Fancy::parse("nonsense"), None);
    }

    #[test]
    fn the_fancy_is_a_pure_function_of_name_and_seed() {
        assert_eq!(Fancy::draw("Ada", 3), Fancy::draw("Ada", 3));
        let all: std::collections::BTreeSet<String> = (0..40)
            .map(|i| Fancy::draw(&format!("leader{i}"), 3).line())
            .collect();
        assert!(all.len() > 8, "leaders differ: {}", all.len());
    }

    #[test]
    fn unflyable_and_dear_doctrines_score_lower() {
        let w = world();
        let b = book_with_fancy(fancy_of(&w.cat[0]));
        let t = traits(0.5, 0.5, 0.0);
        let mut inp = inputs(&w, 5 * DAY, t);
        let base = scores(&b, &inp, &LeadKnobs::NORMAL);
        let train = |_: &Pilot, _: &[(i64, i64)]| 99.0;
        let mut w2 = world();
        w2.cat[2].roles[0].2.skills = vec![(42, 5)];
        let mut inp2 = inputs(&w2, 5 * DAY, t);
        inp2.train_days = &train;
        let low = scores(&b, &inp2, &LeadKnobs::NORMAL);
        assert!(low[2].can < base[2].can);
        // A human is a half-guess; supply scales its term.
        inp.supply = &|_| 0.0;
        let none = scores(&b, &inp, &LeadKnobs::NORMAL);
        assert!(none[0].supply < base[0].supply);
        // Cost: a dearer fleet scores lower.
        let mut w3 = world();
        for r in &mut w3.cat[1].roles {
            r.2.cost = 1.0e9;
        }
        let dear = scores(&b, &inputs(&w3, 5 * DAY, t), &LeadKnobs::NORMAL);
        assert!(dear[1].cost < base[1].cost);
    }

    #[test]
    fn the_loss_ledger_decays_and_caps() {
        let mut l = LossLedger::default();
        assert_eq!(l.ratio(0, DAY), 0.0);
        l.note(300.0, 100.0, 0, DAY);
        assert!((l.ratio(0, DAY) - 3.0).abs() < 1e-9);
        l.note(1000.0, 0.0, 0, DAY);
        assert_eq!(l.ratio(0, DAY), LOSS_RATIO_CAP);
        let mut m = LossLedger::default();
        m.note(100.0, 100.0, 0, DAY);
        assert!(m.ratio(0, DAY) > 0.9);
        m.note(0.0, 0.0, 30 * DAY, DAY);
        assert_eq!(m.ratio(30 * DAY, DAY), 0.0, "old losses are forgotten");
    }
}
