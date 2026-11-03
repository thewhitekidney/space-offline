//! **Per-system tick budgets** (TICK-1, `docs/lanes/tick-1.md`): a system
//! nobody is in costs the clock almost nothing, and what it owes the world
//! is paid exactly when somebody looks.
//!
//! # Classes
//!
//! [`classify`] reads [`crate::liveness::Liveness`] (refreshed first in the
//! tick) and sorts a system into one of three [`Class`]es:
//!
//! - **Hot**: a character's ship (human or bot) is in space there
//!   ([`Reasons::PILOT`]). Every stage runs for it on every tick.
//! - **Warm**: live, but nobody is flying there: NPCs or drones only, an
//!   online pilot docked, a jump landing, a hot gate neighbour, or cooling
//!   after any of those ([`COOL_AFTER`]). Nobody can see it, so a stage may
//!   skip its work **only where the skip is provably the identity** (the
//!   rule per stage is in [`RULES`]).
//! - **Cold**: no entry. Stages touch it on events (a pilot arriving, a
//!   hub's contest, the rollover) and never on the clock.
//!
//! # The rule every stage follows
//!
//! A budget never changes an outcome, it only declines to compute one that
//! is already known. Three shapes of that, all in [`RULES`]:
//!
//! 1. **[`Rule::Every`]**: the stage already costs nothing for an idle
//!    system (it is keyed on the liveness list, on a queue, or on its own
//!    `due` time), so it is not touched.
//! 2. **[`Rule::IdleIsIdentity`]**: the stage skips the units (hubs, ships,
//!    NPCs, sites) for which it would compute "no change", decided by a
//!    cheap predicate that is the *exact* precondition of the no-op. The
//!    next tick that the predicate fails runs the stage's own code on the
//!    same stored state, so nothing needs to catch up: time-based state
//!    (timers, cycles, fuel, scores) is stored as timestamps and read at the
//!    tick's `now`, never integrated tick by tick.
//! 3. **[`Rule::OwnClock`]**: the stage keeps its own `due` timestamp and
//!    compares it to the tick's blue time; a late run does the work once for
//!    all the periods it missed (the same code a stalled tick already ran).
//!
//! Stages whose state would drift if skipped (module cycles, missiles in
//! flight, drone and NPC fire) are `Every`, and are cheap while there is
//! nothing in flight, because they iterate in-flight things, not systems.

use crate::liveness::{Liveness, Reasons};

/// A system's class for the tick budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Cold,
    Warm,
    Hot,
}

/// The class of `system` in this tick's liveness index.
pub fn classify(live: &Liveness, system: i64) -> Class {
    match live.get(system) {
        None => Class::Cold,
        Some(s) if s.reasons.contains(Reasons::PILOT) => Class::Hot,
        Some(_) => Class::Warm,
    }
}

/// [`classify`] over the world.
pub fn class_of(w: &crate::service::Registry, system: i64) -> Class {
    classify(w.ext::<Liveness>(), system)
}

/// How many beats a warm system waits between looks.
///
/// **[A]** five: a sov window edge is hours apart, so a look every five
/// seconds is already far finer than the thing it watches. Falsified by an
/// alliance member seeing a window edge more than ten seconds after it.
pub const WARM_EVERY: u64 = 5;

/// How many beats a cold system waits between looks.
///
/// **[A]** thirty (half a minute at a one-second beat), for the same reason.
/// Falsified by a window edge seen more than a minute late.
pub const COLD_EVERY: u64 = 30;

/// **Whether a system of `class` is looked at on beat number `beat`**: hot
/// on every beat, warm one in [`WARM_EVERY`], cold one in [`COLD_EVERY`],
/// staggered by `key` (a system id) so the looks of many systems spread over
/// the beats instead of landing together. A stage that uses this must read
/// stored timestamps at `now`, never integrate per beat, so a late look
/// finds exactly what an early one would have (see the module docs).
pub fn due(class: Class, beat: u64, key: i64) -> bool {
    let every = match class {
        Class::Hot => return true,
        Class::Warm => WARM_EVERY,
        Class::Cold => COLD_EVERY,
    };
    beat.wrapping_add(key.unsigned_abs()).is_multiple_of(every)
}

/// How a stage meets the budget (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Rule {
    /// Already free for an idle system; untouched.
    Every,
    /// Skips the units whose result is the identity; the predicate is the
    /// stage's own, named in the note.
    IdleIsIdentity,
    /// Runs on its own `due` timestamp, once for any missed periods.
    OwnClock,
}

/// One stage's rule and the sentence that justifies it.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct StageRule {
    pub stage: &'static str,
    pub rule: Rule,
    pub note: &'static str,
}

const fn r(stage: &'static str, rule: Rule, note: &'static str) -> StageRule {
    StageRule { stage, rule, note }
}

/// **The rule of every stage in `tick::STAGES`.** `tick::tests` fails when a
/// stage has no row, so a new stage must say how it meets the budget.
#[cfg_attr(not(test), allow(dead_code))]
pub const RULES: &[StageRule] = &[
    r(
        "liveness",
        Rule::Every,
        "reads the roster and sessions; the index itself",
    ),
    r(
        "asteroid",
        Rule::Every,
        "walks live systems only; cold ones are dropped, warm laid out once",
    ),
    r(
        "moon_belt",
        Rule::Every,
        "per live system, returns at once while its window is laid out",
    ),
    r(
        "moon_scan",
        Rule::Every,
        "probes in flight only; each carries its own expiry",
    ),
    r(
        "modules",
        Rule::Every,
        "running cycles only; a boundary is a timestamp read at now",
    ),
    r(
        "tackle",
        Rule::IdleIsIdentity,
        "a ship no player tackle effect aims at, and not told before, computes nothing",
    ),
    r("ewar", Rule::Every, "running ewar cycles only"),
    r("remote_rep", Rule::Every, "running rep cycles only"),
    r("overheat", Rule::Every, "heated racks only"),
    r("weapon_bank", Rule::Every, "banks only"),
    r("dbuff", Rule::Every, "running bursts only"),
    r("combat", Rule::Every, "cycles just started only"),
    r("mining", Rule::Every, "cycles just ended only"),
    r("hacking", Rule::Every, "armed and linked games only"),
    r(
        "npc",
        Rule::IdleIsIdentity,
        "an NPC alone with NPCs, with no target, lock memory, due repair or assist, decides nothing",
    ),
    r(
        "missile",
        Rule::Every,
        "missiles in flight only; flown to now from their launch time",
    ),
    r("npc_ewar", Rule::Every, "held ewar only"),
    r(
        "warp_abort",
        Rule::Every,
        "a roster walk; the guards run only for a warp still aligning",
    ),
    r(
        "concord",
        Rule::Every,
        "responses and checkpoints in play only",
    ),
    r("jump", Rule::Every, "jumps in flight only"),
    r("safe_logoff", Rule::Every, "countdowns in flight only"),
    r("self_destruct", Rule::Every, "sequences in flight only"),
    r(
        "invuln",
        Rule::Every,
        "protected ships only (undock, gate arrival)",
    ),
    r(
        "system_effects",
        Rule::Every,
        "returns at once while no beacon table is loaded and nobody was told; with a bundle every tick walks the occupied systems, a map lookup each, and reads a roster entry only for a character in a beacon system",
    ),
    r(
        "cloak",
        Rule::Every,
        "cloaked ships only; the table is empty (a fast return) while nobody is",
    ),
    r(
        "superweapon",
        Rule::Every,
        "doomsday state only; the tables are empty (a fast return) until a titan fires",
    ),
    r(
        "skill_training",
        Rule::Every,
        "queues that train; a level is a timestamp compared with now",
    ),
    r(
        "bounty_payout",
        Rule::Every,
        "pending rows, each with its due time",
    ),
    r(
        "insurance",
        Rule::OwnClock,
        "prunes on a fixed second of the clock",
    ),
    r(
        "population",
        Rule::OwnClock,
        "per live system, each with its own due time",
    ),
    r(
        "wormhole_jump",
        Rule::Every,
        "wormhole jumps in flight only",
    ),
    r(
        "random_jump",
        Rule::Every,
        "Needlejack groups in their spool only",
    ),
    r(
        "wormhole",
        Rule::IdleIsIdentity,
        "an arrivals check in a system with no pilot in space and no wormhole instance does nothing",
    ),
    r("abyss", Rule::Every, "Abyss pockets in use only"),
    r("escalation", Rule::Every, "open escalations only"),
    r(
        "site",
        Rule::IdleIsIdentity,
        "an arrivals check in a system with no pilot in space and no site instance does nothing",
    ),
    r(
        "dungeon_tracking",
        Rule::Every,
        "instances with a pilot inside only",
    ),
    r("site_spawn", Rule::OwnClock, "one scan a CHECK_EVERY"),
    r("facwar", Rule::OwnClock, "one scan a SCAN_EVERY"),
    r(
        "signal_tracker",
        Rule::Every,
        "systems with a tracker entry only; an entry with nothing queued sends nothing",
    ),
    r(
        "entosis",
        Rule::IdleIsIdentity,
        "a hub with no running link and no stored score computes nothing; the window scan looks at warm and cold systems on their beat (a phase is stored window plus now)",
    ),
    r(
        "boosters",
        Rule::OwnClock,
        "the soonest expiry is kept in memory and compared with the wall clock; rows are read only when it has passed",
    ),
    r("sov_capital", Rule::Every, "the few pending capital rows"),
    r(
        "fleet_reconnect",
        Rule::Every,
        "only the fleets with a gone member, read from their stored deadlines",
    ),
    r(
        "structure_space",
        Rule::Every,
        "structures with a stored timer or decommissioning time due, read from their rows; no row, no work",
    ),
    r(
        "war",
        Rule::Every,
        "wars with a start, end or ally due, read from their stored times",
    ),
    r(
        "econ_agents",
        Rule::OwnClock,
        "one sweep every SWEEP_EVERY_H game hours, sliced",
    ),
    r(
        "rollover",
        Rule::OwnClock,
        "owed days from the day counter, sliced over live systems",
    ),
    r(
        "chat_dir",
        Rule::OwnClock,
        "a full build every RECONCILE_EVERY (1 s), nothing between",
    ),
    r("grid", Rule::Every, "systems with a viewer only"),
];

/// **Which stages may run less than every tick** (PA-3), and the most
/// ticks apart they may be (`tick::Stage::every`). A stage is listed only
/// when its rule is [`Rule::OwnClock`], or [`Rule::Every`] with its work
/// keyed on stored timestamps and rows (the note says so): a run that is
/// `every - 1` ticks (750 ms at 4) late finds what an early one would have
/// and does it once. Never [`Rule::IdleIsIdentity`] (those skip per unit, not
/// per tick) and never a stage that is sliced per tick (`econ_agents`,
/// `rollover`: a slower cadence is a slower sweep) or that keys on a fixed
/// second of the clock (`insurance`: a cadence could step over the second).
#[cfg_attr(not(test), allow(dead_code))]
pub const CADENCE_OK: &[(&str, u8)] = &[
    ("war", 4),
    ("structure_space", 4),
    ("sov_capital", 4),
    ("bounty_payout", 4),
    ("facwar", 4),
    ("population", 4),
];

/// The rule for `stage`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn rule_of(stage: &str) -> Option<&'static StageRule> {
    RULES.iter().find(|r| r.stage == stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::BlueTime;
    use std::collections::BTreeMap;

    const A: i64 = 30_000_001;
    const B: i64 = 30_000_002;
    const C: i64 = 30_000_003;
    const D: i64 = 30_000_004;
    const E: i64 = 30_000_005;
    const T0: BlueTime = 1_000 * crate::liveness::COOL_AFTER;

    fn apply(live: &mut Liveness, rows: &[(i64, Reasons)], now: BlueTime) {
        let m: BTreeMap<i64, Reasons> = rows.iter().copied().collect();
        live.apply(&m, now);
    }

    /// **The classifier**: a pilot in space is hot; NPCs alone, a docked
    /// pilot, a landing jump and a neighbour are warm; nothing is cold.
    #[test]
    fn classes_follow_the_reasons() {
        let mut live = Liveness::default();
        let both = {
            let mut r = Reasons::PILOT;
            r.insert(Reasons::BALLS);
            r
        };
        apply(
            &mut live,
            &[
                (A, both),
                (B, Reasons::BALLS),
                (C, Reasons::DOCKED),
                (D, Reasons::NEIGHBOUR),
            ],
            T0,
        );
        assert_eq!(classify(&live, A), Class::Hot);
        assert_eq!(classify(&live, B), Class::Warm, "NPCs alone: nobody to see");
        assert_eq!(classify(&live, C), Class::Warm);
        assert_eq!(classify(&live, D), Class::Warm);
        assert_eq!(classify(&live, E), Class::Cold);
        assert!(Class::Cold < Class::Warm && Class::Warm < Class::Hot);
    }

    /// A system whose pilot left cools: warm for [`COOL_AFTER`], then cold.
    #[test]
    fn a_system_cools_hot_to_warm_to_cold() {
        let mut live = Liveness::default();
        apply(&mut live, &[(A, Reasons::PILOT)], T0);
        assert_eq!(classify(&live, A), Class::Hot);
        apply(&mut live, &[], T0 + 1);
        assert_eq!(classify(&live, A), Class::Warm);
        apply(&mut live, &[], T0 + crate::liveness::COOL_AFTER);
        assert_eq!(classify(&live, A), Class::Cold);
    }

    /// **The beat**: hot is due on every beat; over one cold period every
    /// system is looked at exactly once when cold and `COLD_EVERY /
    /// WARM_EVERY` times when warm, and the looks of neighbouring ids do not
    /// coincide.
    #[test]
    fn the_beat_by_class() {
        for key in [A, B, C, 7] {
            assert!((0..100).all(|b| due(Class::Hot, b, key)));
            let cold = (0..COLD_EVERY)
                .filter(|b| due(Class::Cold, *b, key))
                .count();
            assert_eq!(cold, 1);
            let warm = (0..COLD_EVERY)
                .filter(|b| due(Class::Warm, *b, key))
                .count();
            assert_eq!(warm as u64, COLD_EVERY / WARM_EVERY);
        }
        let first = |key| (0..COLD_EVERY).find(|b| due(Class::Cold, *b, key));
        assert_ne!(first(A), first(B), "neighbouring systems are staggered");
        // A negative id cannot panic or skip a class.
        assert!((0..COLD_EVERY).any(|b| due(Class::Cold, b, -3)));
    }

    /// **A cadenced stage is one whose rule tolerates it.** Table-driven over
    /// `tick::STAGES`: every stage with `every > 1` is in [`CADENCE_OK`], has
    /// a row in [`RULES`] that is `OwnClock` or `Every` (never
    /// `IdleIsIdentity`), keeps `every` within its limit and `phase` below
    /// `every`; every `CADENCE_OK` row names a stage; and the cadenced stages
    /// are staggered over at least two phases.
    #[test]
    fn a_cadenced_stage_has_a_rule_that_tolerates_it() {
        for s in &crate::module::plan().stages {
            if s.every <= 1 {
                assert_eq!(s.phase, 0, "{}: phase without cadence", s.name);
                continue;
            }
            let rule = rule_of(s.name).unwrap_or_else(|| panic!("{} has no rule", s.name));
            assert!(
                matches!(rule.rule, Rule::OwnClock | Rule::Every),
                "{}: {:?} does not tolerate a cadence",
                s.name,
                rule.rule
            );
            let (_, limit) = CADENCE_OK
                .iter()
                .find(|(n, _)| *n == s.name)
                .unwrap_or_else(|| panic!("{} is cadenced but not in CADENCE_OK", s.name));
            assert!(s.every <= *limit, "{}: every {} > {limit}", s.name, s.every);
            assert!(s.phase < s.every, "{}: phase out of range", s.name);
        }
        for (name, limit) in CADENCE_OK {
            assert!(*limit > 1);
            assert!(
                crate::module::plan().stages.iter().any(|s| s.name == *name),
                "CADENCE_OK names unknown stage {name}"
            );
            assert!(
                matches!(
                    rule_of(name).map(|r| r.rule),
                    Some(Rule::OwnClock | Rule::Every)
                ),
                "{name}"
            );
        }
        let mut phases: Vec<u8> = crate::module::plan()
            .stages
            .iter()
            .filter(|s| s.every > 1)
            .map(|s| s.phase)
            .collect();
        phases.sort_unstable();
        phases.dedup();
        assert!(phases.len() >= 2, "cadenced stages are staggered");
    }

    /// A cadenced stage runs exactly once per `every` beats, and an
    /// uncadenced one on every beat.
    #[test]
    fn the_cadence_runs_a_stage_once_per_period() {
        for s in &crate::module::plan().stages {
            let n = (0..240u64).filter(|b| s.due(*b)).count() as u64;
            assert_eq!(n, 240 / u64::from(s.every.max(1)), "{}", s.name);
        }
    }

    /// Every stage of the clock has a rule, and no rule names a stage that
    /// is not in the table.
    #[test]
    fn every_stage_states_its_rule() {
        for s in &crate::module::plan().stages {
            assert!(rule_of(s.name).is_some(), "stage {} has no rule", s.name);
        }
        for row in RULES {
            assert!(
                crate::module::plan()
                    .stages
                    .iter()
                    .any(|s| s.name == row.stage),
                "rule for unknown stage {}",
                row.stage
            );
        }
        assert!(RULES.iter().all(|r| !r.note.is_empty()));
        let mut skipping: Vec<&str> = RULES
            .iter()
            .filter(|r| r.rule == Rule::IdleIsIdentity)
            .map(|r| r.stage)
            .collect();
        skipping.sort_unstable();
        assert_eq!(
            skipping,
            ["entosis", "npc", "site", "tackle", "wormhole"],
            "the stages that skip idle units"
        );
        let mut names: Vec<&str> = RULES.iter().map(|r| r.stage).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), RULES.len(), "one row per stage");
    }
}
