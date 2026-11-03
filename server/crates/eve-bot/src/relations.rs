//! **The four-level relations model** (R-REL-1, R-RELATIONS §2): personal
//! (`P`), corp (`C`), alliance (`A`) and coalition (`K`) views, one fact
//! stream, a fairness classifier, betrayal, blocs and the difficulty
//! [`Temper`]. **Everything here is pure**: no socket, no clock (`now` and
//! `day` are arguments). Observation (R-REL-2) builds [`Fact`]s and calls
//! [`deltas`]; effects (R-REL-3) read [`r_eff`], [`c_eff`], [`a_eff_k`],
//! [`blocs`] and [`next_stance5`](crate::society::next_stance5).
//!
//! - **Stored**: `P` in [`Mind::regard`](crate::mind::Mind) (decaying,
//!   [`PERSONAL_KEEP`] at most), `C` in the hall's [`Relations`], `A`'s own
//!   facts (wars, sov, treaties, coalition acts) in a second [`Relations`]
//!   (`alliance_own`). `K` is derived ([`blocs`]).
//! - **Spill up at write**: a personal fact adds [`U_PC`] x its delta to the
//!   corp's view. **Compose down at read**: [`r_eff`] adds the corp, the
//!   alliance and the bloc views at [`D_CP`], [`D_AP`], [`D_KP`].
//! - **[`Temper`]** scales the constants; [`Temper::NORMAL`] reproduces
//!   today's.
//!
//! Every number is **[A]** (R-RELATIONS §8), each with its falsifier at its
//! constant; no test holds one (tests name the consts).

use crate::career::Band;
use crate::mind::Regard;
use crate::society::{
    self, ALLY_AT, ALLY_DAYS, ENEMY_AT, Event, Group, HALF_LIFE_DAYS, K_ALLY_FIGHT, K_CONTEST,
    K_KILL, K_NEIGHBOUR, K_SIEGE, K_TRADE, K_TRADE_CAP, R_EPSILON, R_MAX, Relations, Stances,
    group_of, is_player_corp, value_factor,
};
use eve_server::group_contacts::Affiliation;
use std::collections::{BTreeMap, BTreeSet};

// ---- constants (all [A]; falsifiers in R-RELATIONS §2.2-2.5) ---------------------

/// A personal fact's spill to the corp **[A]**. Falsified by one ganked bot
/// turning its corp red alone (too high).
pub const U_PC: f64 = 0.25;
/// Down-composition weights: corp, alliance, coalition into a pilot's view
/// **[A]**; corp, alliance, coalition into a corp's; coalition into an
/// alliance's.
pub const D_CP: f64 = 0.5;
pub const D_AP: f64 = 0.3;
pub const D_KP: f64 = 0.15;
pub const D_AC: f64 = 0.6;
pub const D_KC: f64 = 0.25;
pub const D_KA: f64 = 0.4;
/// Half-life of a personal regard, days at loyalty 0.5 (x `0.5 + loyalty`)
/// **[A]**. Falsified by grudges outliving a war by weeks.
pub const PERSONAL_HALF_LIFE_DAYS: f64 = 14.0;
/// Half-life of an alliance's own facts **[A]**. Falsified by a treaty
/// fading between play evenings.
pub const ALLIANCE_HALF_LIFE_DAYS: f64 = 21.0;
/// Regards kept per bot **[A]** (state bound).
pub const PERSONAL_KEEP: usize = 24;

/// Fairness classifier **[A]** (falsified by fair duels making more than
/// 20 % of red pairs).
pub const FAIR_LO: f64 = 0.5;
pub const FAIR_HI: f64 = 2.0;
pub const FAIR_GANG: usize = 3;
pub const GANK_RHO: f64 = 3.0;
pub const GANK_GANG: usize = 5;
pub const FAIR_MULT: f64 = 0.5;
pub const GANK_MULT: f64 = 1.5;
pub const POD_MULT: f64 = 1.25;
pub const HIGH_MULT: f64 = 1.5;
pub const AGGRESSOR_MULT: f64 = 0.5;

/// Betrayal **[A]** (falsified by an accidental fleet-fire locking a
/// friend out for a month).
pub const K_BETRAY_MULT: f64 = 2.5;
pub const K_BETRAY_K: f64 = 2.0;
pub const BETRAY_MEMORY: f64 = 3.0;
pub const BETRAY_MEMORY_DAYS: f64 = 30.0;
pub const BETRAY_PENALTY: f64 = 2.0;
/// A loss under this many ISK is no betrayal.
pub const BETRAY_MIN_ISK: f64 = 1_000_000.0;
/// Bloc witnesses' share of a loss **[A]**.
pub const K_WITNESS: f64 = 0.3;

/// Fact weights **[A]** (R-RELATIONS §2.3).
pub const K_RESPECT: f64 = 0.3;
pub const K_HELP: f64 = 1.5;
pub const K_RESCUE: f64 = 2.0;
pub const K_FLEET: f64 = 0.2;
pub const K_FLEET_CAP: f64 = 2.0;
pub const K_THEFT: f64 = 2.0;
pub const K_THEFT_SUSPECT: f64 = 1.5;
pub const K_SCAM: f64 = 3.0;
pub const K_SIEGE_KILL: f64 = 1.5;
pub const K_WAR_ON: f64 = 6.0;
pub const K_PEACE: f64 = 3.0;
pub const K_WAR_ALLY: f64 = 3.0;
pub const K_TALK: f64 = 0.5;
pub const K_ACCEPT: f64 = 1.5;
pub const K_GIFT: f64 = 1e-7;
pub const K_GIFT_CAP: f64 = 1.5;
pub const K_JOIN: f64 = 1.0;
pub const K_ENEMY_OF_ENEMY: f64 = 0.5;

/// Coalition **[A]**: the lag at coordination 0, game-hours; the share of
/// a bloc's weight that must agree at coordination 0.5.
pub const COALITION_LAG_H: f64 = 12.0;
pub const QUORUM: f64 = 0.5;

// ---- Temper (section 5) -----------------------------------------------------------

/// **The difficulty knobs**, all [A]. [`Temper::NORMAL`] reproduces today's
/// constants, by test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temper {
    /// x every negative weight (kill, theft, scam, siege, betrayal).
    pub grudge_rate: f64,
    /// x every half-life.
    pub memory: f64,
    /// Shifts the Wary and Hostile thresholds toward 0 (hunts earlier).
    pub hunt_bias: f64,
    /// 0..1: bloc lag, [`D_KP`], quorum.
    pub coordination: f64,
    /// R0 toward an unlisted human.
    pub human_prior: f64,
    /// Widens the fair band and drops the gank multiplier.
    pub gank_tolerance: f64,
    /// Game-days between two stance changes of a pair.
    pub flip_min_days: f64,
    /// How long a betrayal is remembered, days (infinite: always).
    pub betray_memory_days: f64,
}

impl Temper {
    pub const EASY: Temper = Temper {
        grudge_rate: 0.6,
        memory: 0.5,
        hunt_bias: -1.0,
        coordination: 0.2,
        human_prior: 3.0,
        gank_tolerance: 1.5,
        flip_min_days: 2.0,
        betray_memory_days: 7.0,
    };
    pub const NORMAL: Temper = Temper {
        grudge_rate: 1.0,
        memory: 1.0,
        hunt_bias: 0.0,
        coordination: 0.5,
        human_prior: 0.0,
        gank_tolerance: 1.0,
        flip_min_days: society::FLIP_MIN_DAYS,
        betray_memory_days: BETRAY_MEMORY_DAYS,
    };
    pub const HARD: Temper = Temper {
        grudge_rate: 1.4,
        memory: 1.5,
        hunt_bias: 1.0,
        coordination: 0.8,
        human_prior: -2.0,
        gank_tolerance: 0.7,
        flip_min_days: 0.5,
        betray_memory_days: 60.0,
    };
    pub const BRUTAL: Temper = Temper {
        grudge_rate: 2.0,
        memory: 3.0,
        hunt_bias: 2.0,
        coordination: 1.0,
        human_prior: -5.0,
        gank_tolerance: 0.5,
        flip_min_days: 0.25,
        betray_memory_days: f64::INFINITY,
    };

    /// The built-in knobs of a difficulty level (DOC-8): 1 easy .. 4
    /// brutal; 0 and 2 are normal (level 0 freezes the doctrine layer
    /// elsewhere).
    pub fn at(level: u8) -> Temper {
        match level {
            1 => Temper::EASY,
            3 => Temper::HARD,
            4.. => Temper::BRUTAL,
            _ => Temper::NORMAL,
        }
    }

    /// `easy`, `normal`, `hard` or `brutal` (the `--difficulty` values).
    pub fn by_name(name: &str) -> Option<Temper> {
        match name {
            "easy" => Some(Temper::EASY),
            "normal" => Some(Temper::NORMAL),
            "hard" => Some(Temper::HARD),
            "brutal" => Some(Temper::BRUTAL),
            _ => None,
        }
    }

    /// A half-life in days: `base x (0.5 + loyalty) x memory`.
    pub fn half_days(&self, base: f64, loyalty: f64) -> f64 {
        base * (0.5 + loyalty.clamp(0.0, 1.0)) * self.memory
    }

    /// [`D_KP`] at this coordination (the normal 0.5 gives [`D_KP`]).
    pub fn d_kp(&self) -> f64 {
        D_KP * self.coordination / Temper::NORMAL.coordination
    }

    /// The share of a bloc's weight that must agree to adopt an enemy:
    /// [`QUORUM`] at the normal coordination, lower the more coordinated.
    pub fn quorum(&self) -> f64 {
        (QUORUM * (1.5 - self.coordination)).clamp(0.1, 1.0)
    }

    /// Game-hours a bloc member waits before it adopts the bloc's enemy
    /// list: `(1 - coordination) x COALITION_LAG_H`.
    pub fn lag_hours(&self) -> f64 {
        (1.0 - self.coordination.clamp(0.0, 1.0)) * COALITION_LAG_H
    }

    /// `Fair` band `[lo, hi]` and the gank ratio, widened by the tolerance.
    pub fn fair_band(&self) -> (f64, f64) {
        (FAIR_LO / self.gank_tolerance, FAIR_HI * self.gank_tolerance)
    }
    pub fn gank_rho(&self) -> f64 {
        GANK_RHO * self.gank_tolerance
    }
    pub fn gank_mult(&self) -> f64 {
        1.0 + (GANK_MULT - 1.0) / self.gank_tolerance
    }
    /// Betrayal's multiplier, scaled by the grudge rate.
    pub fn betray_mult(&self) -> f64 {
        1.0 + (K_BETRAY_MULT - 1.0) * self.grudge_rate
    }
}

/// Has a bloc member waited long enough (`hours` since the bloc's enemy
/// appeared) to adopt it? Pure.
pub fn lag_elapsed(hours: f64, t: &Temper) -> bool {
    hours >= t.lag_hours()
}

// ---- the fairness classifier (section 2.4) ----------------------------------------

/// What a loss reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fairness {
    Fair,
    Plain,
    Gank,
}

/// What a killmail says about a loss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kill {
    /// Attackers on the report.
    pub gang: usize,
    /// Sum of the attackers' [`hunt::power`](crate::hunt::power) and the
    /// victim's.
    pub attacker_power: f64,
    pub victim_power: f64,
    /// The victim was a capsule.
    pub pod: bool,
    pub band: Band,
    /// The victim was the aggressor or flagged at the time.
    pub victim_aggressor: bool,
}

/// **Fair, plain or gank.** A pod has no power, so it is a gank only by the
/// gang size; the pod's own weight is [`POD_MULT`].
pub fn classify(k: &Kill, t: &Temper) -> Fairness {
    let gang = k.gang.max(1);
    if k.pod {
        return if gang >= GANK_GANG {
            Fairness::Gank
        } else {
            Fairness::Plain
        };
    }
    let rho = if k.victim_power > 0.0 {
        k.attacker_power / k.victim_power
    } else {
        f64::INFINITY
    };
    let (lo, hi) = t.fair_band();
    if rho >= t.gank_rho() || gang >= GANK_GANG {
        Fairness::Gank
    } else if (lo..=hi).contains(&rho) && gang <= FAIR_GANG {
        Fairness::Fair
    } else {
        Fairness::Plain
    }
}

/// The multiplier a loss's context puts on [`K_KILL`].
pub fn kill_mult(k: &Kill, t: &Temper) -> f64 {
    let mut m = match classify(k, t) {
        Fairness::Fair => FAIR_MULT,
        Fairness::Gank => t.gank_mult(),
        Fairness::Plain => 1.0,
    };
    if k.pod {
        m *= POD_MULT;
    }
    if k.band == Band::High {
        m *= HIGH_MULT;
    }
    if k.victim_aggressor {
        m *= AGGRESSOR_MULT;
    }
    m
}

/// **Is it a betrayal?** The killer was allied to the victim when it
/// happened, the loss was worth [`BETRAY_MIN_ISK`] and the victim was no
/// aggressor.
pub fn is_betrayal(allied_at_time: bool, isk: f64, victim_aggressor: bool) -> bool {
    allied_at_time && isk >= BETRAY_MIN_ISK && !victim_aggressor
}

// ---- facts and deltas (section 2.3) -----------------------------------------------

/// The 17 facts (F1..F17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactKind {
    /// F1: the target lost a ship to the actor.
    Loss,
    /// F2: the actor killed the target's ship (the actor holds the respect).
    Victory,
    /// F3: the actor assisted the target in a fight.
    Help,
    /// F4: the actor rescued the target.
    Rescue,
    /// F5: both flew in one fleet for `hours`.
    Fleet,
    /// F6: the actor looted the target's can.
    Theft,
    /// F7: the actor's contract was not honoured.
    Scam,
    /// F8: a contract or trade completed (both).
    Trade,
    /// F9: the actor attacked the target's sov.
    Siege,
    /// F10: the actor declared war on the target.
    WarOn,
    /// F11: war ended or treaty (both).
    Peace,
    /// F12: the actor joined the target's war.
    WarAlly,
    /// F13: a loss where the killer was allied to the victim.
    Betrayal,
    /// F14: mail exchanged (both).
    Talk,
    /// F15: the actor gifted the target.
    Gift,
    /// F16: the actor was accepted into the target's corp.
    Join,
    /// F17: the actor killed the target's enemy.
    EnemyOfEnemy,
}

/// The extra context a fact may carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ctx {
    pub kill: Kill,
    /// The killer was allied to the victim at the time (F13).
    pub allied_at_time: bool,
    /// The actor was flagged suspect when seen (F6).
    pub suspect: bool,
    /// A structure was shot, not just a link (F9).
    pub structure_shot: bool,
    /// Hours flown together (F5).
    pub hours: f64,
    /// The reply answered a blue offer in time (F14).
    pub accepted_offer: bool,
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx {
            kill: Kill {
                gang: 1,
                attacker_power: 1.0,
                victim_power: 1.0,
                pod: false,
                band: Band::Low,
                victim_aggressor: false,
            },
            allied_at_time: false,
            suspect: false,
            structure_shot: false,
            hours: 0.0,
            accepted_offer: false,
        }
    }
}

/// One thing that happened, between two pilots. `at` is the game second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fact {
    pub kind: FactKind,
    pub actor: Affiliation,
    pub target: Affiliation,
    pub isk: f64,
    pub ctx: Ctx,
    pub at: u64,
}

/// Where a delta is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// `Mind.regard` of the holder character.
    Personal,
    /// `Hall.relations` (the holder a bot corp).
    Corp,
    /// `Hall.alliance_own` (the holder an alliance).
    Alliance,
}

/// One change to one stored view: `holder`'s view of `subject` moves by `d`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Delta {
    pub level: Level,
    pub holder: i64,
    pub subject: i64,
    pub d: f64,
}

fn personal(holder: &Affiliation, subject: &Affiliation, d: f64, out: &mut Vec<Delta>) {
    if holder.character_id != subject.character_id {
        out.push(Delta {
            level: Level::Personal,
            holder: holder.character_id,
            subject: subject.character_id,
            d,
        });
    }
}

/// The corp's spill of a personal fact ([`U_PC`]).
fn corp_spill(holder: &Affiliation, subject: &Affiliation, d: f64, out: &mut Vec<Delta>) {
    if is_player_corp(holder.corporation_id) {
        out.push(Delta {
            level: Level::Corp,
            holder: holder.corporation_id,
            subject: group_of(subject),
            d: U_PC * d,
        });
    }
}

/// The group-level holder of an alliance fact: the alliance, else the corp.
fn group_level(holder: &Affiliation, subject: &Affiliation, d: f64, out: &mut Vec<Delta>) {
    match holder.alliance_id {
        Some(al) => out.push(Delta {
            level: Level::Alliance,
            holder: al,
            subject: group_of(subject),
            d,
        }),
        None if is_player_corp(holder.corporation_id) => out.push(Delta {
            level: Level::Corp,
            holder: holder.corporation_id,
            subject: group_of(subject),
            d,
        }),
        None => {}
    }
}

/// **A fact's deltas** (pure): personal at full weight, the corp at
/// [`U_PC`], the alliance's own facts at their own level.
pub fn deltas(f: &Fact, t: &Temper) -> Vec<Delta> {
    let mut out = Vec::new();
    let rate = t.grudge_rate;
    let vf = value_factor(f.isk);
    let (a, b) = (&f.actor, &f.target);
    // `holder` sees `subject` move by `d`, at the personal level and spilled.
    let both = |out: &mut Vec<Delta>, d: f64| {
        personal(b, a, d, out);
        corp_spill(b, a, d, out);
    };
    match f.kind {
        FactKind::Loss | FactKind::Betrayal => {
            let betrayal = f.kind == FactKind::Betrayal
                || is_betrayal(f.ctx.allied_at_time, f.isk, f.ctx.kill.victim_aggressor);
            let mut m = kill_mult(&f.ctx.kill, t);
            if betrayal {
                m *= t.betray_mult();
            }
            both(&mut out, -K_KILL * vf * m * rate);
        }
        FactKind::Victory => {
            if classify(&f.ctx.kill, t) == Fairness::Fair {
                personal(a, b, K_RESPECT, &mut out);
            }
        }
        FactKind::Help => both(&mut out, K_HELP * vf),
        FactKind::Rescue => both(&mut out, K_RESCUE),
        FactKind::Fleet => {
            let d = (K_FLEET * f.ctx.hours.max(0.0)).min(K_FLEET_CAP);
            both(&mut out, d);
            personal(a, b, d, &mut out);
            corp_spill(a, b, d, &mut out);
        }
        FactKind::Theft => {
            let w = if f.ctx.suspect { K_THEFT_SUSPECT } else { 1.0 };
            both(&mut out, -K_THEFT * w * rate);
        }
        FactKind::Scam => both(&mut out, -K_SCAM * vf * rate),
        FactKind::Trade => {
            let d = (K_TRADE * f.isk.max(0.0)).min(K_TRADE_CAP);
            both(&mut out, d);
            personal(a, b, d, &mut out);
            corp_spill(a, b, d, &mut out);
        }
        FactKind::Siege => {
            let w = if f.ctx.structure_shot {
                K_SIEGE_KILL
            } else {
                K_SIEGE
            };
            group_level(b, a, -w * rate, &mut out);
        }
        FactKind::WarOn => group_level(b, a, -K_WAR_ON * rate, &mut out),
        FactKind::Peace => {
            group_level(b, a, K_PEACE, &mut out);
            group_level(a, b, K_PEACE, &mut out);
        }
        FactKind::WarAlly => {
            group_level(b, a, K_WAR_ALLY, &mut out);
            group_level(a, b, K_WAR_ALLY, &mut out);
        }
        FactKind::Talk => {
            let d = if f.ctx.accepted_offer {
                K_TALK + K_ACCEPT
            } else {
                K_TALK
            };
            both(&mut out, d);
            personal(a, b, d, &mut out);
        }
        FactKind::Gift => both(&mut out, (K_GIFT * f.isk.max(0.0)).min(K_GIFT_CAP)),
        FactKind::Join => both(&mut out, K_JOIN),
        FactKind::EnemyOfEnemy => {
            if is_player_corp(b.corporation_id) {
                out.push(Delta {
                    level: Level::Corp,
                    holder: b.corporation_id,
                    subject: group_of(a),
                    d: K_ENEMY_OF_ENEMY,
                });
            }
        }
    }
    out
}

/// **The event adapter**: today's [`Event`]s as corp-level deltas. At
/// [`Temper::NORMAL`] they equal [`Relations::apply`], by test.
pub fn event_deltas(e: &Event, t: &Temper) -> Vec<Delta> {
    let c = |holder: Group, subject: Group, d: f64| Delta {
        level: Level::Corp,
        holder,
        subject,
        d,
    };
    match *e {
        Event::Kill {
            victim,
            killer,
            isk,
        } => vec![c(
            victim,
            killer,
            -K_KILL * value_factor(isk) * t.grudge_rate,
        )],
        Event::Siege { defender, attacker } => {
            vec![c(defender, attacker, -K_SIEGE * t.grudge_rate)]
        }
        Event::Trade { a, b, isk } => {
            let d = (K_TRADE * isk.max(0.0)).min(K_TRADE_CAP);
            vec![c(a, b, d), c(b, a, d)]
        }
        Event::AllyFight { a, b } => vec![c(a, b, K_ALLY_FIGHT), c(b, a, K_ALLY_FIGHT)],
        Event::Contest { a, b, days } => {
            let d = -K_CONTEST * days.max(0.0);
            vec![c(a, b, d), c(b, a, d)]
        }
        Event::Neighbour { a, b, days } => {
            let d = K_NEIGHBOUR * days.max(0.0);
            vec![c(a, b, d), c(b, a, d)]
        }
        // DOC-4: the same arithmetic as `Relations::apply` (the harder
        // tempers scale only what hurts).
        Event::Reinforced {
            helper,
            helped,
            isk_saved,
        } => {
            let d = crate::society::K_REINFORCE * value_factor(isk_saved);
            vec![c(helper, helped, d), c(helped, helper, d)]
        }
        Event::Abandoned { asker, ally } => {
            vec![c(asker, ally, -crate::society::K_ABANDON * t.grudge_rate)]
        }
        Event::Betrayed { betrayer, victim } => vec![c(
            victim,
            betrayer,
            -crate::society::K_BETRAY * t.grudge_rate,
        )],
    }
}

// ---- personal regard (replaces Mind.grudges) --------------------------------------

/// A regard's value at `now`, decayed with half-life `half_days`.
pub fn regard_decayed(r: &Regard, now: u64, day: u64, half_days: f64) -> f64 {
    let days = now.saturating_sub(r.at) as f64 / day.max(1) as f64;
    r.score * 0.5f64.powf(days / half_days.max(f64::MIN_POSITIVE))
}

/// A bot's half-life for its regards.
pub fn personal_half_days(loyalty: f64, t: &Temper) -> f64 {
    t.half_days(PERSONAL_HALF_LIFE_DAYS, loyalty)
}

/// `who`'s regard at `now` (0 when none).
pub fn regard_of(list: &[Regard], who: i64, now: u64, day: u64, half_days: f64) -> f64 {
    list.iter()
        .find(|r| r.who == who)
        .map_or(0.0, |r| regard_decayed(r, now, day, half_days))
}

/// **Move `who`'s regard by `d`** at `now`: every entry decays to `now`
/// first, those under [`R_EPSILON`] are forgotten, and past
/// [`PERSONAL_KEEP`] the weakest is evicted.
pub fn regard_add(list: &mut Vec<Regard>, who: i64, d: f64, now: u64, day: u64, half_days: f64) {
    if !d.is_finite() {
        return;
    }
    for r in list.iter_mut() {
        r.score = regard_decayed(r, now, day, half_days);
        r.at = now;
    }
    match list.iter_mut().find(|r| r.who == who) {
        Some(r) => r.score = (r.score + d).clamp(-R_MAX, R_MAX),
        None => list.push(Regard {
            who,
            score: d.clamp(-R_MAX, R_MAX),
            at: now,
        }),
    }
    list.retain(|r| r.score.abs() >= R_EPSILON);
    while list.len() > PERSONAL_KEEP {
        let weakest = list
            .iter()
            .enumerate()
            .min_by(|(_, x), (_, y)| {
                x.score
                    .abs()
                    .total_cmp(&y.score.abs())
                    .then(x.at.cmp(&y.at))
                    .then(x.who.cmp(&y.who))
            })
            .map(|(i, _)| i);
        match weakest {
            Some(i) => {
                list.remove(i);
            }
            None => break,
        }
    }
}

/// **The grudges a bot holds** (what [`society::join_score`] reads): the
/// negative regards as positive magnitudes, `who -> 0 ..= R_MAX`.
pub fn grudges(list: &[Regard], now: u64, day: u64, half_days: f64) -> BTreeMap<i64, f64> {
    list.iter()
        .map(|r| (r.who, -regard_decayed(r, now, day, half_days)))
        .filter(|(_, g)| *g > 0.0)
        .collect()
}

/// **The `--friends` seam**: set a personal regard at the file's standing
/// for each friend (the file wins over what the bot learned). The standing
/// and the regard share the contact scale `-10 ..= 10`.
pub fn seed_friends(list: &mut Vec<Regard>, friends: &[(i64, f64)], now: u64) {
    for &(who, standing) in friends {
        let score = standing.clamp(-R_MAX, R_MAX);
        match list.iter_mut().find(|r| r.who == who) {
            Some(r) => {
                r.score = score;
                r.at = now;
            }
            None => list.push(Regard {
                who,
                score,
                at: now,
            }),
        }
    }
    // The friends are the strongest by the owner's word: evict others first.
    while list.len() > PERSONAL_KEEP {
        let weakest = list
            .iter()
            .enumerate()
            .filter(|(_, r)| !friends.iter().any(|(f, _)| *f == r.who))
            .min_by(|(_, x), (_, y)| x.score.abs().total_cmp(&y.score.abs()))
            .map(|(i, _)| i);
        match weakest {
            Some(i) => {
                list.remove(i);
            }
            None => break,
        }
    }
}

// ---- storage for tests and the observer: one place that applies deltas ------------

/// **The stored levels together**: every bot's regards, the corp matrix and
/// the alliances' own facts. The hall and the minds hold these in the real
/// bots; the observer and the tests use this to apply [`Delta`]s.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Web {
    pub regard: BTreeMap<i64, Vec<Regard>>,
    pub corp: Relations,
    pub alliance_own: Relations,
}

impl Web {
    /// Apply one delta at `now`. `loyalty` is the holder's.
    pub fn apply(&mut self, d: &Delta, now: u64, day: u64, loyalty: f64, t: &Temper) {
        match d.level {
            Level::Personal => regard_add(
                self.regard.entry(d.holder).or_default(),
                d.subject,
                d.d,
                now,
                day,
                personal_half_days(loyalty, t),
            ),
            Level::Corp => self.corp.add(d.holder, d.subject, d.d),
            Level::Alliance => self.alliance_own.add(d.holder, d.subject, d.d),
        }
    }

    /// Apply a fact.
    pub fn observe(&mut self, f: &Fact, day: u64, loyalty: f64, t: &Temper) {
        for d in deltas(f, t) {
            self.apply(&d, f.at, day, loyalty, t);
        }
    }
}

// ---- blocs (section 2.6) ----------------------------------------------------------

/// A coalition: a connected component of mutually blue groups.
#[derive(Debug, Clone, PartialEq)]
pub struct Bloc {
    /// Member groups with their weight (bot members).
    pub members: BTreeMap<Group, f64>,
    pub weight: f64,
    /// The heaviest member; ties to the lower id.
    pub lead: Group,
}

impl Bloc {
    pub fn contains(&self, g: Group) -> bool {
        self.members.contains_key(&g)
    }
}

/// **The blocs** (derived): groups joined by edges where both stances are
/// blue and have been for [`ALLY_DAYS`]. `weights` is each group's bot
/// member count (a group without a weight is not in a bloc). Components of
/// two or more only.
pub fn blocs(stances: &Stances, weights: &BTreeMap<Group, f64>, now: u64, day: u64) -> Vec<Bloc> {
    let need = (ALLY_DAYS * day as f64) as u64;
    let old = |a: Group, b: Group| {
        stances.get(a, b) == society::Stance::Blue
            && stances
                .since(a, b)
                .is_some_and(|s| now.saturating_sub(s) >= need)
    };
    let mut parent: BTreeMap<Group, Group> = weights.keys().map(|g| (*g, *g)).collect();
    fn find(p: &mut BTreeMap<Group, Group>, x: Group) -> Group {
        let mut r = x;
        while p[&r] != r {
            r = p[&r];
        }
        let mut c = x;
        while p[&c] != r {
            let n = p[&c];
            p.insert(c, r);
            c = n;
        }
        r
    }
    let gs: Vec<Group> = weights.keys().copied().collect();
    for (i, &a) in gs.iter().enumerate() {
        for &b in &gs[i + 1..] {
            if old(a, b) && old(b, a) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent.insert(ra.max(rb), ra.min(rb));
                }
            }
        }
    }
    let mut comps: BTreeMap<Group, BTreeMap<Group, f64>> = BTreeMap::new();
    for &g in &gs {
        let r = find(&mut parent, g);
        comps.entry(r).or_default().insert(g, weights[&g]);
    }
    comps
        .into_values()
        .filter(|m| m.len() >= 2)
        .map(|members| {
            let lead = members
                .iter()
                .max_by(|(ga, wa), (gb, wb)| wa.total_cmp(wb).then(gb.cmp(ga)))
                .map(|(g, _)| *g)
                .unwrap_or_default();
            Bloc {
                weight: members.values().sum(),
                members,
                lead,
            }
        })
        .collect()
}

// ---- composing the views ----------------------------------------------------------

/// The stored layers a read composes.
#[derive(Debug, Clone, Copy)]
pub struct Layers<'a> {
    pub corp: &'a Relations,
    pub alliance_own: &'a Relations,
    /// Alliance -> its bot member corps.
    pub members: &'a BTreeMap<Group, Vec<Group>>,
    pub blocs: &'a [Bloc],
}

/// The viewer: a bot and where it belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewer {
    pub corp: Group,
    pub alliance: Option<Group>,
}

impl Layers<'_> {
    /// The effective alliance view: its member corps' mean plus its own.
    pub fn a_eff(&self, alliance: Group, t: Group) -> f64 {
        let mean = match self.members.get(&alliance) {
            Some(m) if !m.is_empty() => {
                m.iter().map(|c| self.corp.get(*c, t)).sum::<f64>() / m.len() as f64
            }
            _ => 0.0,
        };
        (mean + self.alliance_own.get(alliance, t)).clamp(-R_MAX, R_MAX)
    }

    /// A member's own view for the bloc mean: an alliance's effective view,
    /// a corp's matrix row.
    fn member_view(&self, m: Group, t: Group) -> f64 {
        if society::PLAYER_ALLIANCES.contains(&m) {
            self.a_eff(m, t)
        } else {
            self.corp.get(m, t)
        }
    }

    /// `K[bloc][t]`: the weight-mean of the members' views.
    pub fn k(&self, bloc: &Bloc, t: Group) -> f64 {
        if bloc.weight <= 0.0 {
            return 0.0;
        }
        bloc.members
            .iter()
            .map(|(m, w)| w * self.member_view(*m, t))
            .sum::<f64>()
            / bloc.weight
    }

    /// The bloc a group belongs to.
    pub fn bloc_of(&self, g: Group) -> Option<&Bloc> {
        self.blocs.iter().find(|b| b.contains(g))
    }

    fn k_for(&self, v: &Viewer, t: Group) -> f64 {
        v.alliance
            .and_then(|a| self.bloc_of(a))
            .or_else(|| self.bloc_of(v.corp))
            .map_or(0.0, |b| self.k(b, t))
    }
}

/// **A pilot's effective view of another** (section 2.2), bounded by
/// `R_MAX`: `P + D_CP C + D_AP A_eff + D_KP K`. `regard` is the viewer's
/// stored list; `human` makes an unlisted pilot start at
/// [`Temper::human_prior`].
#[allow(clippy::too_many_arguments)] // the read's whole context, flat for the callers
pub fn r_eff(
    l: &Layers,
    v: &Viewer,
    regard: &[Regard],
    loyalty: f64,
    target: &Affiliation,
    human: bool,
    now: u64,
    day: u64,
    t: &Temper,
) -> f64 {
    let g = group_of(target);
    let p = match regard.iter().find(|r| r.who == target.character_id) {
        Some(r) => regard_decayed(r, now, day, personal_half_days(loyalty, t)),
        None if human => t.human_prior,
        None => 0.0,
    };
    let a = v.alliance.map_or(0.0, |al| l.a_eff(al, g));
    (p + D_CP * l.corp.get(v.corp, g) + D_AP * a + t.d_kp() * l.k_for(v, g)).clamp(-R_MAX, R_MAX)
}

/// **The corp-level view the CEO sets contacts from**:
/// `C + D_AC A_own + D_KC K`.
pub fn c_eff(l: &Layers, v: &Viewer, t: Group) -> f64 {
    let own = v.alliance.map_or(0.0, |a| l.alliance_own.get(a, t));
    (l.corp.get(v.corp, t) + D_AC * own + D_KC * l.k_for(v, t)).clamp(-R_MAX, R_MAX)
}

/// **The alliance-level view**: `A_eff + D_KA K`.
pub fn a_eff_k(l: &Layers, alliance: Group, t: Group) -> f64 {
    let k = l.bloc_of(alliance).map_or(0.0, |b| l.k(b, t));
    (l.a_eff(alliance, t) + D_KA * k).clamp(-R_MAX, R_MAX)
}

/// **A bloc's enemies** among `targets`: `K <= ENEMY_AT`, and at least the
/// quorum of its weight sees the target at `<= ENEMY_AT + 1`.
pub fn enemies(l: &Layers, bloc: &Bloc, targets: &[Group], t: &Temper) -> Vec<Group> {
    targets
        .iter()
        .copied()
        .filter(|&x| !bloc.contains(x))
        .filter(|&x| l.k(bloc, x) <= ENEMY_AT)
        .filter(|&x| {
            let share: f64 = bloc
                .members
                .iter()
                .filter(|(m, _)| l.member_view(**m, x) <= ENEMY_AT + 1.0)
                .map(|(_, w)| *w)
                .sum();
            bloc.weight > 0.0 && share / bloc.weight >= t.quorum()
        })
        .collect()
}

/// **Betrayal's spill**: every other member of the victim's bloc takes
/// [`K_BETRAY_K`] against the killer's group, at the alliance level (the
/// corp level for a corp member).
pub fn betrayal_spill(
    l: &Layers,
    victim: &Affiliation,
    killer: &Affiliation,
    t: &Temper,
) -> Vec<Delta> {
    let own = victim.alliance_id.unwrap_or(victim.corporation_id);
    let Some(b) = l.bloc_of(own) else {
        return Vec::new();
    };
    b.members
        .keys()
        .filter(|m| **m != own)
        .map(|m| Delta {
            level: if society::PLAYER_ALLIANCES.contains(m) {
                Level::Alliance
            } else {
                Level::Corp
            },
            holder: *m,
            subject: group_of(killer),
            d: -K_BETRAY_K * t.grudge_rate,
        })
        .collect()
}

/// The half-life multiplier a pair's betrayal puts on its memory: [`BETRAY_MEMORY`]
/// within `betray_memory_days` of `betrayed_at`, else 1.
pub fn betrayal_memory(betrayed_at: Option<u64>, now: u64, day: u64, t: &Temper) -> f64 {
    match betrayed_at {
        Some(b) if (now.saturating_sub(b) as f64) < t.betray_memory_days * day as f64 => {
            BETRAY_MEMORY
        }
        _ => 1.0,
    }
}

/// The relation a betrayed pair needs to be blue again.
pub fn reblue_at(betrayed_at: Option<u64>, now: u64, day: u64, t: &Temper) -> f64 {
    if betrayal_memory(betrayed_at, now, day, t) > 1.0 {
        ALLY_AT + BETRAY_PENALTY
    } else {
        ALLY_AT
    }
}

/// Alliance half-life for the `alliance_own` matrix's decay.
pub fn alliance_half_days(loyalty: f64, t: &Temper) -> f64 {
    t.half_days(ALLIANCE_HALF_LIFE_DAYS, loyalty)
}

/// The corp matrix's half-life under a temper (today's at normal).
pub fn corp_half_days(loyalty: f64, t: &Temper) -> f64 {
    t.half_days(HALF_LIFE_DAYS, loyalty)
}

/// Groups appearing in `weights`' keys that are not in any bloc (a small
/// helper for the reports).
pub fn unblocked(weights: &BTreeMap<Group, f64>, blocs: &[Bloc]) -> BTreeSet<Group> {
    weights
        .keys()
        .copied()
        .filter(|g| !blocs.iter().any(|b| b.contains(*g)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use society::{Stance, Stances};

    const DAY: u64 = 86_400;
    const T: Temper = Temper::NORMAL;

    fn aff(c: i64, corp: i64, al: Option<i64>) -> Affiliation {
        Affiliation {
            character_id: c,
            corporation_id: corp,
            alliance_id: al,
        }
    }
    // Invented ids: bot 1 in corp 98_000_001 / alliance 99_000_001; rival 2
    // in corp 98_000_002; bot 3 in corp 98_000_003 / the same alliance.
    const CORP1: i64 = 98_000_001;
    const CORP2: i64 = 98_000_002;
    const CORP3: i64 = 98_000_003;
    const AL1: i64 = 99_000_001;
    const AL2: i64 = 99_000_002;
    fn bot1() -> Affiliation {
        aff(1, CORP1, Some(AL1))
    }
    fn bot3() -> Affiliation {
        aff(3, CORP3, Some(AL1))
    }
    fn rival() -> Affiliation {
        aff(2, CORP2, None)
    }

    fn kill(gang: usize, att: f64, vic: f64, pod: bool, band: Band) -> Kill {
        Kill {
            gang,
            attacker_power: att,
            victim_power: vic,
            pod,
            band,
            victim_aggressor: false,
        }
    }

    fn loss(k: Kill) -> Fact {
        Fact {
            kind: FactKind::Loss,
            actor: rival(),
            target: bot1(),
            isk: KILL_REF,
            ctx: Ctx {
                kill: k,
                ..Ctx::default()
            },
            at: 0,
        }
    }
    const KILL_REF: f64 = society::KILL_VALUE_REF;

    fn personal_d(f: &Fact, t: &Temper) -> f64 {
        deltas(f, t)
            .iter()
            .find(|d| d.level == Level::Personal)
            .map_or(0.0, |d| d.d)
    }

    #[test]
    fn a_gank_weighs_more_than_a_fair_duel() {
        let fair = loss(kill(1, 3.0, 3.0, false, Band::Low));
        let gank = loss(kill(1, 3.0 * GANK_RHO, 3.0, false, Band::Low));
        let gang = loss(kill(GANK_GANG, 5.0, 5.0, false, Band::Low));
        assert_eq!(classify(&fair.ctx.kill, &T), Fairness::Fair);
        assert_eq!(classify(&gank.ctx.kill, &T), Fairness::Gank);
        assert_eq!(classify(&gang.ctx.kill, &T), Fairness::Gank);
        assert!(personal_d(&gank, &T) < personal_d(&fair, &T));
        assert!(personal_d(&gang, &T) < personal_d(&fair, &T));
        assert_eq!(personal_d(&fair, &T), -K_KILL * FAIR_MULT);
    }

    #[test]
    fn a_pod_weighs_more_than_a_frigate() {
        // The same lone attacker and no ratio to speak of: only the hull.
        let ship = loss(kill(1, 2.0, 1.0 * 2.0 * 1.2, false, Band::Low));
        let mut pod_k = kill(1, 1.0, 0.0, true, Band::Low);
        pod_k.victim_power = 0.0;
        let pod = loss(pod_k);
        assert_eq!(classify(&ship.ctx.kill, &T), Fairness::Fair);
        assert_eq!(classify(&pod.ctx.kill, &T), Fairness::Plain);
        assert!(personal_d(&pod, &T) < personal_d(&ship, &T));
        assert_eq!(personal_d(&pod, &T), -K_KILL * POD_MULT);
    }

    #[test]
    fn a_high_sec_gank_weighs_more_than_a_low_sec_one() {
        let low = loss(kill(GANK_GANG, 9.0, 3.0, false, Band::Low));
        let high = loss(kill(GANK_GANG, 9.0, 3.0, false, Band::High));
        assert_eq!(personal_d(&high, &T), personal_d(&low, &T) * HIGH_MULT,);
        assert_eq!(personal_d(&low, &T), -K_KILL * GANK_MULT);
    }

    #[test]
    fn an_aggressor_loss_is_discounted() {
        let plain = loss(kill(2, 6.0, 3.0, false, Band::Low));
        let mut k = plain.ctx.kill;
        k.victim_aggressor = true;
        let mut aggr = plain;
        aggr.ctx.kill = k;
        assert_eq!(
            personal_d(&aggr, &T),
            personal_d(&plain, &T) * AGGRESSOR_MULT
        );
    }

    fn layers_with<'a>(
        corp: &'a Relations,
        own: &'a Relations,
        members: &'a BTreeMap<Group, Vec<Group>>,
        blocs: &'a [Bloc],
    ) -> Layers<'a> {
        Layers {
            corp,
            alliance_own: own,
            members,
            blocs,
        }
    }

    fn two_alliance_bloc() -> (Stances, BTreeMap<Group, f64>) {
        let mut s = Stances::default();
        s.settle(AL1, AL2, ALLY_AT, 0, DAY);
        s.settle(AL2, AL1, ALLY_AT, 0, DAY);
        let w = BTreeMap::from([(AL1, 3.0), (AL2, 2.0)]);
        (s, w)
    }

    #[test]
    fn a_betrayal_flips_stance_at_once_and_spills_to_the_bloc() {
        let (mut s, w) = two_alliance_bloc();
        let now = (ALLY_DAYS * DAY as f64) as u64 + 10;
        let bl = blocs(&s, &w, now, DAY);
        assert_eq!(bl.len(), 1);
        // AL1's bot is shot by a pilot of AL2 that AL1 had blue.
        let killer = aff(7, 98_000_007, Some(AL2));
        let victim = bot1();
        let betrayal = Fact {
            kind: FactKind::Loss,
            actor: killer,
            target: victim,
            isk: society::KILL_VALUE_REF,
            ctx: Ctx {
                allied_at_time: true,
                kill: kill(1, 3.0, 3.0, false, Band::Low),
                ..Ctx::default()
            },
            at: now,
        };
        let plain = Fact {
            ctx: Ctx {
                allied_at_time: false,
                ..betrayal.ctx
            },
            ..betrayal
        };
        let d_b = personal_d(&betrayal, &T);
        assert_eq!(d_b, personal_d(&plain, &T) * T.betray_mult());
        // The stance flips now, inside the flip window, and the pair is marked.
        assert_eq!(s.get(AL1, AL2), Stance::Blue);
        s.settle_forced(AL1, AL2, Stance::Red, now + 1);
        s.mark_betrayed(AL1, AL2, now + 1);
        assert_eq!(s.get(AL1, AL2), Stance::Red);
        assert_eq!(s.betrayed_at(AL1, AL2), Some(now + 1));
        // Re-blue needs more while it is remembered.
        assert!(reblue_at(s.betrayed_at(AL1, AL2), now + 2, DAY, &T) > ALLY_AT);
        assert_eq!(reblue_at(None, now, DAY, &T), ALLY_AT);
        // The bloc: the other member of the victim's bloc takes the spill.
        let (c, o, m) = (Relations::default(), Relations::default(), BTreeMap::new());
        let l = layers_with(&c, &o, &m, &bl);
        let sp = betrayal_spill(&l, &victim, &killer, &T);
        // The victim's own alliance is excluded; AL2 (the killer's) takes it too.
        assert_eq!(sp.len(), 1);
        assert_eq!(sp[0].holder, AL2);
        assert_eq!(sp[0].subject, group_of(&killer));
        assert_eq!(sp[0].d, -K_BETRAY_K * T.grudge_rate);
        assert_eq!(sp[0].level, Level::Alliance);
    }

    #[test]
    fn a_personal_fact_spills_up_to_the_corp_by_u_pc() {
        let f = loss(kill(1, 3.0, 3.0, false, Band::Low));
        let mut web = Web::default();
        web.observe(&f, DAY, 0.5, &T);
        let p = regard_of(&web.regard[&1], 2, f.at, DAY, personal_half_days(0.5, &T));
        let c = web.corp.get(CORP1, group_of(&rival()));
        assert!(p < 0.0);
        assert!((c - U_PC * p).abs() < 1e-12, "{c} vs {p}");
        assert!(web.alliance_own.r.is_empty(), "nothing is written to A");
    }

    #[test]
    fn an_alliance_war_colours_every_member_without_a_write() {
        let war = Fact {
            kind: FactKind::WarOn,
            actor: rival(),
            target: bot1(),
            isk: 0.0,
            ctx: Ctx::default(),
            at: 0,
        };
        let mut web = Web::default();
        web.observe(&war, DAY, 0.5, &T);
        assert!(web.regard.is_empty() && web.corp.r.is_empty());
        let members = BTreeMap::from([(AL1, vec![CORP1, CORP3])]);
        let l = layers_with(&web.corp, &web.alliance_own, &members, &[]);
        for bot in [bot1(), bot3()] {
            let v = Viewer {
                corp: bot.corporation_id,
                alliance: bot.alliance_id,
            };
            let r = r_eff(&l, &v, &[], 0.5, &rival(), false, 0, DAY, &T);
            assert!((r + D_AP * K_WAR_ON * T.grudge_rate).abs() < 1e-12, "{r}");
        }
    }

    #[test]
    fn r_eff_composes_all_four_levels_and_is_bounded() {
        let x = rival();
        let g = group_of(&x);
        let mut corp = Relations::default();
        let mut own = Relations::default();
        corp.add(CORP1, g, -2.0);
        corp.add(CORP3, g, -4.0);
        corp.add(CORP2 + 1000, g, -6.0); // another bloc member (a corp)
        own.add(AL1, g, -1.0);
        let members = BTreeMap::from([(AL1, vec![CORP1, CORP3])]);
        let bloc = Bloc {
            members: BTreeMap::from([(AL1, 2.0), (CORP2 + 1000, 2.0)]),
            weight: 4.0,
            lead: AL1,
        };
        let blocs_v = [bloc];
        let l = layers_with(&corp, &own, &members, &blocs_v);
        let v = Viewer {
            corp: CORP1,
            alliance: Some(AL1),
        };
        let regard = [Regard {
            who: x.character_id,
            score: -1.0,
            at: 0,
        }];
        let a_eff = (-2.0 - 4.0) / 2.0 - 1.0;
        let k = (2.0 * a_eff + 2.0 * -6.0) / 4.0;
        let want = -1.0 + D_CP * -2.0 + D_AP * a_eff + T.d_kp() * k;
        let got = r_eff(&l, &v, &regard, 0.5, &x, false, 0, DAY, &T);
        assert!((got - want).abs() < 1e-12, "{got} vs {want}");
        // Bounded.
        corp.add(CORP1, g, -R_MAX);
        let regard = [Regard {
            who: x.character_id,
            score: -R_MAX,
            at: 0,
        }];
        let l = layers_with(&corp, &own, &members, &blocs_v);
        let got = r_eff(&l, &v, &regard, 0.5, &x, false, 0, DAY, &T);
        assert!((-R_MAX..=R_MAX).contains(&got));
        assert_eq!(got, -R_MAX);
        // A human with no regard starts at the temper's prior.
        let hp = r_eff(
            &l,
            &v,
            &[],
            0.5,
            &aff(9, 9, None),
            true,
            0,
            DAY,
            &Temper::EASY,
        );
        assert_eq!(hp, Temper::EASY.human_prior);
        // The corp- and alliance-level views compose the bloc too.
        assert!(c_eff(&l, &v, g) < corp.get(CORP1, g) + 1e-9);
        assert!(a_eff_k(&l, AL1, g) <= l.a_eff(AL1, g));
    }

    #[test]
    fn personal_decay_is_monotone_and_loyalty_slows_it() {
        let r = Regard {
            who: 2,
            score: -8.0,
            at: 0,
        };
        let half = personal_half_days(0.5, &T);
        let mut last = r.score.abs();
        for d in 1..40u64 {
            let v = regard_decayed(&r, d * DAY, DAY, half).abs();
            assert!(v < last);
            last = v;
        }
        // One half-life halves it.
        let h = regard_decayed(&r, (half * DAY as f64) as u64, DAY, half);
        assert!((h - r.score / 2.0).abs() < 1e-3);
        let loyal = personal_half_days(1.0, &T);
        let fickle = personal_half_days(0.0, &T);
        let at = 10 * DAY;
        assert!(
            regard_decayed(&r, at, DAY, loyal).abs() > regard_decayed(&r, at, DAY, fickle).abs()
        );
    }

    #[test]
    fn regard_keeps_the_24_strongest() {
        let mut list = Vec::new();
        let half = personal_half_days(0.5, &T);
        for i in 1..=30i64 {
            regard_add(&mut list, 1000 + i, -(i as f64) / 4.0, 0, DAY, half);
        }
        assert_eq!(list.len(), PERSONAL_KEEP);
        assert!(list.iter().all(|r| r.who > 1000 + 6));
        assert!(!list.iter().any(|r| r.who == 1001));
        // Grudges read as positive magnitudes.
        let g = grudges(&list, 0, DAY, half);
        assert!(g.values().all(|v| *v > 0.0));
    }

    #[test]
    fn blocs_are_mutual_blue_components() {
        let (mut s, mut w) = two_alliance_bloc();
        let later = (ALLY_DAYS * DAY as f64) as u64;
        // Mutual blue, but not yet ALLY_DAYS old.
        assert!(blocs(&s, &w, later - 1, DAY).is_empty());
        let b = blocs(&s, &w, later, DAY);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].lead, AL1, "the heavier leads");
        assert_eq!(b[0].weight, 5.0);
        // A one-way blue is no edge.
        s.settle(AL2, 99_000_003, ALLY_AT, 0, DAY);
        w.insert(99_000_003, 9.0);
        let b = blocs(&s, &w, later, DAY);
        assert_eq!(b.len(), 1);
        assert!(!b[0].contains(99_000_003));
        // A chain A-B-C is one component.
        s.settle(99_000_003, AL2, ALLY_AT, 0, DAY);
        let b = blocs(&s, &w, later, DAY);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].members.len(), 3);
        assert_eq!(b[0].lead, 99_000_003);
        assert_eq!(unblocked(&w, &b).len(), 0);
    }

    #[test]
    fn a_bloc_lags_by_one_minus_coordination() {
        assert_eq!(T.lag_hours(), (1.0 - T.coordination) * COALITION_LAG_H);
        assert_eq!(Temper::BRUTAL.lag_hours(), 0.0);
        assert!(Temper::EASY.lag_hours() > T.lag_hours());
        assert!(!lag_elapsed(T.lag_hours() - 0.01, &T));
        assert!(lag_elapsed(T.lag_hours(), &T));
        // The quorum: the bloc's enemy needs enough of its weight.
        let g = 55;
        let mut corp = Relations::default();
        corp.add(CORP1, g, -8.0);
        corp.add(CORP3, g, 0.0);
        let own = Relations::default();
        let members = BTreeMap::new();
        let bloc = Bloc {
            members: BTreeMap::from([(CORP1, 3.0), (CORP3, 1.0)]),
            weight: 4.0,
            lead: CORP1,
        };
        let bl = [bloc.clone()];
        let l = layers_with(&corp, &own, &members, &bl);
        assert_eq!(enemies(&l, &bloc, &[g], &T), vec![g]);
        corp.add(CORP1, g, 8.0);
        let l = layers_with(&corp, &own, &members, &bl);
        assert!(enemies(&l, &bloc, &[g], &T).is_empty());
    }

    #[test]
    fn normal_temper_equals_the_old_constants() {
        let n = Temper::NORMAL;
        assert_eq!(n.grudge_rate, 1.0);
        assert_eq!(n.memory, 1.0);
        assert_eq!(n.hunt_bias, 0.0);
        assert_eq!(n.flip_min_days, society::FLIP_MIN_DAYS);
        assert_eq!(n.d_kp(), D_KP);
        assert_eq!(n.quorum(), QUORUM);
        assert_eq!(n.gank_mult(), GANK_MULT);
        assert_eq!(n.betray_mult(), K_BETRAY_MULT);
        assert_eq!(n.fair_band(), (FAIR_LO, FAIR_HI));
        for loyalty in [0.0, 0.3, 1.0] {
            assert_eq!(
                corp_half_days(loyalty, &n),
                HALF_LIFE_DAYS * (0.5 + loyalty)
            );
        }
        // Every old event's deltas equal Relations::apply.
        let events = [
            Event::Kill {
                victim: 1,
                killer: 2,
                isk: 25_000_000.0,
            },
            Event::Siege {
                defender: 1,
                attacker: 2,
            },
            Event::Trade {
                a: 1,
                b: 2,
                isk: 5e6,
            },
            Event::AllyFight { a: 1, b: 2 },
            Event::Contest {
                a: 1,
                b: 2,
                days: 2.0,
            },
            Event::Neighbour {
                a: 1,
                b: 2,
                days: 1.5,
            },
        ];
        for e in &events {
            let mut old = Relations::default();
            old.apply(e);
            let mut new = Relations::default();
            for d in event_deltas(e, &n) {
                new.add(d.holder, d.subject, d.d);
            }
            assert_eq!(old, new, "{e:?}");
        }
        assert_eq!(Temper::by_name("normal"), Some(n));
        assert_eq!(Temper::by_name("nope"), None);
    }

    #[test]
    fn the_harder_tempers_remember_and_condemn_more() {
        let f = loss(kill(1, 3.0, 3.0, false, Band::Low));
        assert!(personal_d(&f, &Temper::BRUTAL) < personal_d(&f, &T));
        assert!(personal_d(&f, &Temper::EASY) > personal_d(&f, &T));
        assert!(personal_half_days(0.5, &Temper::BRUTAL) > personal_half_days(0.5, &T));
        assert_eq!(Temper::BRUTAL.betray_memory_days, f64::INFINITY);
        assert_eq!(
            betrayal_memory(Some(0), 1000 * DAY, DAY, &Temper::BRUTAL),
            BETRAY_MEMORY
        );
    }

    #[test]
    fn friends_seed_personal_regard_and_the_file_wins() {
        let half = personal_half_days(0.5, &T);
        let mut list = Vec::new();
        regard_add(&mut list, 42, -6.0, 0, DAY, half);
        seed_friends(&mut list, &[(42, 3.0), (43, 5.0)], 100);
        assert_eq!(regard_of(&list, 42, 100, DAY, half), 3.0);
        assert_eq!(regard_of(&list, 43, 100, DAY, half), 5.0);
        // A friend survives a full list.
        let mut full = Vec::new();
        for i in 1..=PERSONAL_KEEP as i64 {
            regard_add(&mut full, 500 + i, -1.0, 0, DAY, half);
        }
        seed_friends(&mut full, &[(42, 3.0)], 0);
        assert_eq!(full.len(), PERSONAL_KEEP);
        assert!(full.iter().any(|r| r.who == 42));
    }
}
