//! **The liveness index**: which solar systems are cold, warm or hot (W11
//! packet CP6, `docs/content/plan.md` §4.1; C7 §2, packet L3). Hot: a ball
//! in space (a character's, or an NPC's or drone's), ticked. Warm: an online
//! pilot docked there, a jump in flight landing there, or a hot gate
//! neighbour [A]. Cold: none of those for [`COOL_AFTER`], and **no entry**.
//!
//! **Liveness pulls; nothing pushes to it.** [`refresh`] is a world-clock
//! stage reading the roster, the session table, `jump::Jumps` and the NPCs;
//! nothing calls in, so the index cannot drift from what it reads.
//! Hot drops to warm on the first tick with no hot reason (leftover NPCs are
//! a hot reason, so `belts::VACANT_DESPAWN` is already in it); warm drops to
//! cold [`COOL_AFTER`] after the last tick that saw any reason.
//!
//! **Not read yet**, as nothing writes them: mission and route destinations
//! (C4) and system-keyed deltas (`site`, `dbs`, `wh`); their packets add a
//! reason. Materialising on an edge is L4.
//!
//! **Degradation** (W14 CP40, plan §4.6; C7 §5.3, packet L8). The ticker
//! hands every report's world-lock hold to [`note_report`]. After
//! [`OVERRUN_REPORTS`] reports in a row over [`LOCK_BUDGET`] the world is
//! *degraded*: a hot system with **no human** in it is [`due`] only every
//! [`DEGRADED_EVERY`]th tick, and a system with a human is due on every
//! tick, always. [`RECOVER_REPORTS`] reports in a row within budget end it.
//! A human is an online character whose `Character::is_bot` is false; the
//! marker comes from the `--character NAME,bot` spec (`cli.rs`),
//! never from a name (ruling 2), and it never leaves the server (ruling 5):
//! nothing in a select row, a slim item or an overview reads it.

use crate::content::Content;
use crate::damage::BlueTime;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// FILETIME ticks per second (100 ns units) [V: the blue clock].
const BLUE_PER_SECOND: BlueTime = 10_000_000;

/// How long a warm system stays warm after its last reason ends.
///
/// **[A]** C7 §2.1: ten minutes. Falsified by a regenerate storm in `[live]`
/// (too short) or warm counts that never fall on a quiet server (too long).
pub const COOL_AFTER: BlueTime = 10 * 60 * BLUE_PER_SECOND;

/// The world lock's hold budget per tick, over which a report counts as an
/// overrun.
///
/// **[A]** C7 §5.1 and §5.3: 25 ms. Falsified by humans feeling lag in
/// `[live]` while no report overruns (too high), or by `DEGRADED` lines on a
/// server nobody calls slow (too low).
pub const LOCK_BUDGET: Duration = Duration::from_millis(25);

/// How many reports in a row must overrun [`LOCK_BUDGET`] before the world
/// degrades.
///
/// **[A]** C7 §5.3: three, so one slow save or one materialisation burst
/// does not slow anyone. Falsified by a sustained overrun that humans feel
/// before `DEGRADED` prints.
pub const OVERRUN_REPORTS: u32 = 3;

/// How many reports in a row must stay within [`LOCK_BUDGET`] before a
/// degraded world recovers.
///
/// **[A]** not in C7: the same three as [`OVERRUN_REPORTS`], so the halved
/// load must prove itself before it is lifted. Falsified by `DEGRADED` and
/// `recovered` lines alternating every few reports (too short).
pub const RECOVER_REPORTS: u32 = 3;

/// While degraded, a hot system with no human ticks once in this many ticks.
///
/// **[A]** C7 §5.3: every second tick, and NPC weapons absorb the catch-up
/// through `npc::MAX_SHOTS_PER_TICK` [V]. Falsified by bots losing fights
/// they should win under load (too sparse).
pub const DEGRADED_EVERY: u64 = 2;

/// **Graduated shedding** (PA-3): once degraded, every this many overrun
/// reports in a row (counting from the first overrun) the shed deepens one
/// step: [`DEGRADED_EVERY`] (2) at [`OVERRUN_REPORTS`], 4 at twice this, 8 at
/// three times. One report within budget resets the count.
///
/// **[A]** equal to [`OVERRUN_REPORTS`]: a step must prove itself over the
/// same three reports (about 30 s) the first one did. Falsified by a deep
/// shed (8) printed on a world that one more report would have recovered
/// (too eager), or by an overrun that lasts minutes at level 2 (too slow).
pub const SHED_STEP_REPORTS: u32 = 3;

/// The deepest shed: a bot-only system ticks one in this many ticks.
pub const SHED_MAX_EVERY: u64 = 8;

/// The world slot (`crate::world_ext`). Registered in `world_ext::SLOTS` by
/// the coordinator.
pub const SLOT: SlotSpec = SlotSpec {
    name: "liveness",
    make,
};

fn make(_: &Content) -> Boxed {
    Box::new(Liveness::default())
}

/// A system's tier. Ordered: cold < warm < hot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Cold,
    Warm,
    Hot,
}

/// Why a system is live on this tick: a bit set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reasons(u8);

impl Reasons {
    /// Hot: a character's ship (human or bot) is in space here.
    pub const PILOT: Reasons = Reasons(1);
    /// Hot: some other ball is here, an NPC or a drone, so its timers run.
    pub const BALLS: Reasons = Reasons(1 << 1);
    /// Warm: an online character is docked here.
    pub const DOCKED: Reasons = Reasons(1 << 2);
    /// Warm: a jump in flight lands here.
    pub const JUMP: Reasons = Reasons(1 << 3);
    /// Warm **[A]** (C7 §2.1): a hot system one gate away, so a jump lands
    /// warm. Falsified if materialising is cheap enough inline (L6 timings).
    pub const NEIGHBOUR: Reasons = Reasons(1 << 4);

    const HOT: Reasons = Reasons(Self::PILOT.0 | Self::BALLS.0);

    pub fn contains(self, other: Reasons) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Reasons) {
        self.0 |= other.0;
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether any of these makes a system hot.
    pub fn is_hot(self) -> bool {
        self.0 & Self::HOT.0 != 0
    }

    /// The tier these reasons alone give.
    pub fn tier(self) -> Tier {
        if self.is_hot() {
            Tier::Hot
        } else if self.is_empty() {
            Tier::Cold
        } else {
            Tier::Warm
        }
    }
}

/// One live (warm or hot) system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemLive {
    pub tier: Tier,
    /// This tick's reasons; empty while a warm system cools.
    pub reasons: Reasons,
    /// The last tick that saw any reason. Cold at this plus [`COOL_AFTER`].
    pub last_reason: BlueTime,
    /// This tick's online human characters here: in space, docked, or on a
    /// jump landing here (so a system is never skipped on the tick a human
    /// arrives). Zero while it cools.
    pub humans: u32,
    /// This tick's online bot characters here, counted the same way.
    pub bots: u32,
}

/// Who is in a system on this tick: [`SystemLive::humans`] and `bots`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pilots {
    pub humans: u32,
    pub bots: u32,
}

/// What [`Degrade::observe`] changed, for the one log line an edge prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Degraded,
    Recovered,
    /// Still degraded, and the overrun went on: the shed is now this deep
    /// (a bot-only system ticks one in `n`).
    Deeper(u64),
}

/// The overrun hysteresis, pure: [`OVERRUN_REPORTS`] in a row degrade,
/// [`RECOVER_REPORTS`] in a row recover.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Degrade {
    degraded: bool,
    /// Reports in a row on the other side of the budget from `degraded`.
    streak: u32,
    /// Overrun reports in a row, whatever the state (the shed level).
    overruns: u32,
}

impl Degrade {
    pub fn is_degraded(self) -> bool {
        self.degraded
    }

    /// **The shed level**: 1 while healthy, then 2, 4 and 8 as consecutive
    /// overrun reports pile up ([`SHED_STEP_REPORTS`] a step, the first
    /// step being the degrade itself at [`OVERRUN_REPORTS`]). A bot-only
    /// system is due one tick in this many.
    pub fn every(self) -> u64 {
        if !self.degraded {
            return 1;
        }
        let extra = self.overruns.saturating_sub(OVERRUN_REPORTS) / SHED_STEP_REPORTS;
        (DEGRADED_EVERY << extra.min(8)).min(SHED_MAX_EVERY)
    }

    /// One report's worst world-lock hold. Over [`LOCK_BUDGET`] is an
    /// overrun; exactly at it is not.
    pub fn observe(&mut self, held_max: Duration) -> Option<Edge> {
        let over = held_max > LOCK_BUDGET;
        let before = self.every();
        self.overruns = if over {
            self.overruns.saturating_add(1)
        } else {
            0
        };
        let deeper = self.degraded && over && self.every() > before;
        if over != self.degraded {
            self.streak += 1;
        } else {
            self.streak = 0;
        }
        let need = if self.degraded {
            RECOVER_REPORTS
        } else {
            OVERRUN_REPORTS
        };
        if self.streak < need {
            return deeper.then(|| Edge::Deeper(self.every()));
        }
        self.streak = 0;
        self.degraded = !self.degraded;
        Some(if self.degraded {
            Edge::Degraded
        } else {
            Edge::Recovered
        })
    }
}

/// The index: every warm or hot system. A cold one has no entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Liveness {
    systems: BTreeMap<i64, SystemLive>,
    /// Each hot system's gate neighbours, read from the celestial catalogue
    /// the first time it is hot. The catalogue does not change while the
    /// server runs, so a cached list cannot go stale.
    neighbours: BTreeMap<i64, Vec<i64>>,
    /// The overrun state the ticker feeds through [`note_report`].
    degrade: Degrade,
    /// [`refresh`] calls so far: the parity [`Liveness::due`] reads.
    ticks: u64,
}

impl Liveness {
    /// A system's tier; cold when it has no entry.
    pub fn tier(&self, system: i64) -> Tier {
        self.systems.get(&system).map_or(Tier::Cold, |s| s.tier)
    }

    /// A live system's entry; `None` is cold.
    pub fn get(&self, system: i64) -> Option<&SystemLive> {
        self.systems.get(&system)
    }

    /// The hot systems, in id order: the list a per-system tick walks.
    pub fn hot(&self) -> impl Iterator<Item = i64> + '_ {
        self.systems
            .iter()
            .filter(|(_, s)| s.tier == Tier::Hot)
            .map(|(id, _)| *id)
    }

    /// Every live (warm or hot) system, in id order: the status file's rows
    /// (CP15, `live_status::status_tsv`).
    pub fn iter(&self) -> impl Iterator<Item = (i64, &SystemLive)> + '_ {
        self.systems.iter().map(|(id, s)| (*id, s))
    }

    /// How many systems are live (warm or hot).
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Whether the world is degraded (C7 §5.3).
    #[cfg(test)]
    pub fn is_degraded(&self) -> bool {
        self.degrade.is_degraded()
    }

    /// **Whether a per-system tick should run for `system` on this tick.**
    ///
    /// Always, unless the world is degraded. Then a system with a human in
    /// it is still due on every tick, and any other system (bots and NPCs
    /// only, or nobody indexed) once in [`DEGRADED_EVERY`] ticks.
    pub fn due(&self, system: i64) -> bool {
        if !self.degrade.is_degraded() {
            return true;
        }
        if self.systems.get(&system).is_some_and(|s| s.humans > 0) {
            return true;
        }
        self.ticks.is_multiple_of(self.degrade.every().max(1))
    }

    /// **The hysteresis, pure**: this tick's reasons, observed at `now`.
    ///
    /// A system with reasons takes the tier they give and restarts its
    /// cooling. One without drops to warm, and is removed (cold) once
    /// [`COOL_AFTER`] has passed since the last tick that saw a reason.
    #[cfg(test)]
    pub fn apply(&mut self, reasons: &BTreeMap<i64, Reasons>, now: BlueTime) {
        self.apply_with(reasons, &BTreeMap::new(), now);
    }

    /// [`Liveness::apply`], with this tick's [`Pilots`] per system.
    pub fn apply_with(
        &mut self,
        reasons: &BTreeMap<i64, Reasons>,
        pilots: &BTreeMap<i64, Pilots>,
        now: BlueTime,
    ) {
        for (&system, &r) in reasons {
            if r.is_empty() {
                continue;
            }
            let p = pilots.get(&system).copied().unwrap_or_default();
            self.systems.insert(
                system,
                SystemLive {
                    tier: r.tier(),
                    reasons: r,
                    last_reason: now,
                    humans: p.humans,
                    bots: p.bots,
                },
            );
        }
        self.systems.retain(|system, live| {
            if reasons.get(system).is_some_and(|r| !r.is_empty()) {
                return true;
            }
            live.reasons = Reasons::default();
            live.tier = Tier::Warm;
            live.humans = 0;
            live.bots = 0;
            now.saturating_sub(live.last_reason) < COOL_AFTER
        });
    }
}

/// This tick's reasons, read from the world. Neighbours are not here: they
/// need the cache, so [`refresh`] adds them.
fn observe(w: &Registry) -> (BTreeMap<i64, Reasons>, BTreeMap<i64, Pilots>) {
    let mut out: BTreeMap<i64, Reasons> = BTreeMap::new();
    let mut pilots: BTreeMap<i64, Pilots> = BTreeMap::new();
    let mut add = |system: i64, r: Reasons| out.entry(system).or_default().insert(r);
    // A pilot counts once, where the reason puts them. The marker is the
    // character's flag, never its name (ruling 2).
    let mut count = |system: i64, c: &crate::model::Character| {
        let p = pilots.entry(system).or_default();
        if c.is_bot {
            p.bots += 1;
        } else {
            p.humans += 1;
        }
    };
    let roster = w.roster();
    for (system, ids) in roster.systems() {
        for &id in ids {
            let r = match w.character(id) {
                Some(c) => {
                    count(system, c);
                    Reasons::PILOT
                }
                None => Reasons::BALLS,
            };
            add(system, r);
        }
    }
    for npc in w.npcs().iter() {
        add(npc.solar_system_id, Reasons::BALLS);
    }
    for (c, system) in w.jumps().destinations() {
        add(system, Reasons::JUMP);
        if let Some(c) = w.character(c) {
            count(system, c);
        }
    }
    // Docked: online, not in space, not mid-jump (a jumper's system is the
    // one being left; the destination is warm through `JUMP`).
    for c in w.sessions().online_characters() {
        if roster.is_in_space(c) || w.jumps().contains(c) {
            continue;
        }
        if let Some(c) = w.character(c)
            && let Some(system) = c.solar_system_id
        {
            add(system, Reasons::DOCKED);
            count(system, c);
        }
    }
    (out, pilots)
}

/// A system's gate neighbours: the destination system of every gate in it.
fn neighbours_of(catalogue: &crate::celestial::Catalogue, system: i64) -> Vec<i64> {
    let mut out: Vec<i64> = catalogue
        .served_in(system)
        .filter(|c| c.kind == crate::celestial::Kind::Gate)
        .filter(|c| catalogue.system_of(c.id) == Some(system))
        .filter_map(|c| catalogue.jump_of(c.id))
        .map(|(_, to)| to)
        .filter(|to| *to != system)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// **The world-clock stage** (`tick::STAGES`): observe, add the hot
/// systems' neighbours, apply the hysteresis at the tick's blue time.
pub fn refresh(w: &mut Registry, _now: Instant, ft: BlueTime) {
    let (mut reasons, pilots) = observe(w);
    let hot: Vec<i64> = reasons
        .iter()
        .filter(|(_, r)| r.is_hot())
        .map(|(s, _)| *s)
        .collect();
    let missing: Vec<(i64, Vec<i64>)> = {
        let cached = &w.ext::<Liveness>().neighbours;
        hot.iter()
            .filter(|s| !cached.contains_key(s))
            .map(|&s| (s, neighbours_of(w.celestials(), s)))
            .collect()
    };
    let live = w.ext_mut::<Liveness>();
    live.neighbours.extend(missing);
    for s in &hot {
        for &n in live.neighbours.get(s).into_iter().flatten() {
            reasons.entry(n).or_default().insert(Reasons::NEIGHBOUR);
        }
    }
    live.ticks = live.ticks.wrapping_add(1);
    live.apply_with(&reasons, &pilots, ft);
}

/// **The beat**: how many ticks the clock has run ([`refresh`] calls), the
/// number a cadenced stage's `beat % every == phase` reads
/// (`tick::Stage`). Wraps; a fresh world is 0.
pub fn beat(w: &Registry) -> u64 {
    w.ext::<Liveness>().ticks
}

/// **Whether `system`'s per-system work runs on this tick**: the read a
/// per-system stage makes before ticking a system ([`Liveness::due`]).
pub fn due(w: &Registry, system: i64) -> bool {
    w.ext::<Liveness>().due(system)
}

/// **The ticker's report hook**: one report window's worst world-lock hold,
/// and the stage that took longest in it. Prints the `[live] DEGRADED` line
/// on entering (naming that stage, C7 §5.3) and a `[live] recovered` line on
/// leaving.
pub fn note_report(w: &mut Registry, held_max: Duration, slowest_stage: Option<&str>) {
    let live = w.ext_mut::<Liveness>();
    match live.degrade.observe(held_max) {
        Some(Edge::Degraded) => println!(
            "[live] DEGRADED: world lock held max {:.1} ms over {} ms for {OVERRUN_REPORTS} \
             reports (slowest stage {}); systems with no human tick 1 in {DEGRADED_EVERY}",
            held_max.as_secs_f64() * 1000.0,
            LOCK_BUDGET.as_millis(),
            slowest_stage.unwrap_or("-"),
        ),
        Some(Edge::Deeper(n)) => println!(
            "[live] DEGRADED deeper: still over {} ms (held max {:.1} ms); systems with no \
             human tick 1 in {n}",
            LOCK_BUDGET.as_millis(),
            held_max.as_secs_f64() * 1000.0,
        ),
        Some(Edge::Recovered) => println!(
            "[live] recovered: world lock held within {} ms for {RECOVER_REPORTS} reports",
            LOCK_BUDGET.as_millis(),
        ),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Request;
    use crate::space_state::{Motion, Presence};
    use eve_marshal::PyValue;

    const ALICE: i64 = 1_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    /// Invented ids (no client data). `A` is `Character::default()`'s system.
    const A: i64 = 30_000_001;
    const B: i64 = 30_000_003;
    /// Linked to nothing: never anyone's neighbour.
    const C: i64 = 30_000_009;
    const GATE_A: i64 = 51;
    const GATE_B: i64 = 52;
    /// A blue-clock origin; any value works.
    const T0: BlueTime = 1_000 * COOL_AFTER;

    /// A and B joined by one gate pair, ten-column extract format, synthetic
    /// coordinates.
    fn catalogue() -> crate::celestial::Catalogue {
        crate::celestial::Catalogue::parse(&format!(
            "#system\t{A}\t10\t100\n\
             #system\t{B}\t10\t100\n\
             gate\t{GATE_A}\t29624\t2473.0\t5000000000.0\t0.0\t0.0\t{A}\t{GATE_B}\t{B}\n\
             gate\t{GATE_B}\t16\t3532.0\t0.0\t-5000000000.0\t1000.0\t{B}\t{GATE_A}\t{A}\n"
        ))
    }

    fn world(online: bool) -> Registry {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            celestials: catalogue(),
            ..crate::model::SandboxConfig::default()
        });
        if online {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: 1,
                session_id: 1,
                user_id: 1,
                character_id: Some(ALICE),
            });
        }
        reg
    }

    fn in_space(reg: &mut Registry, system: i64) {
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    }

    fn at(reg: &mut Registry, t: BlueTime) -> &Liveness {
        refresh(reg, Instant::now(), t);
        reg.ext::<Liveness>()
    }

    const BOB: i64 = 1_000_002;
    const BOB_SHIP: i64 = 100_000_002;
    /// Over the budget by any margin: an overrun by definition.
    const OVER: Duration = LOCK_BUDGET.saturating_add(Duration::from_millis(1));

    /// Alice (a human) and Bob (a bot), both online. Bob's name is a plain
    /// name and Alice's says "bot": the marker is the flag alone (ruling 2).
    fn two_pilots() -> Registry {
        let pilot = |id, name: &str, ship, is_bot| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            is_bot,
            ..crate::model::Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![
                pilot(ALICE, "Alice Bot", ALICE_SHIP, false),
                pilot(BOB, "Bob", BOB_SHIP, true),
            ],
            celestials: catalogue(),
            ..crate::model::SandboxConfig::default()
        });
        for (n, id) in [(1, ALICE), (2, BOB)] {
            reg.session_opened(crate::session_table::SessionEntry {
                client_id: n,
                session_id: n,
                user_id: n,
                character_id: Some(id),
            });
        }
        reg
    }

    fn undock(reg: &mut Registry, who: i64, ship: i64, system: i64) {
        reg.roster_mut().arrive(Presence::arriving(
            who,
            ship,
            system,
            Motion::arrived([0.0; 3], [0.0; 3]),
        ));
    }

    fn degrade(reg: &mut Registry) {
        for _ in 0..OVERRUN_REPORTS {
            note_report(reg, OVER, Some("npc"));
        }
        assert!(reg.ext::<Liveness>().is_degraded());
    }

    /// How many of the next `n` ticks each system is due on.
    fn due_counts(reg: &mut Registry, systems: &[i64], n: u64) -> Vec<u64> {
        let mut out = vec![0; systems.len()];
        for t in 0..n {
            refresh(reg, Instant::now(), T0 + t as BlueTime);
            for (i, &s) in systems.iter().enumerate() {
                out[i] += u64::from(due(reg, s));
            }
        }
        out
    }

    /// **The overrun hysteresis.** [`OVERRUN_REPORTS`] overruns in a row
    /// degrade, and one report within budget resets the count; exactly the
    /// budget is not an overrun. Degraded, [`RECOVER_REPORTS`] reports
    /// within budget in a row recover, and one overrun resets that count.
    #[test]
    fn degradation_needs_overruns_in_a_row_and_recovers_the_same_way() {
        let mut d = Degrade::default();
        for _ in 1..OVERRUN_REPORTS {
            assert_eq!(d.observe(OVER), None);
        }
        assert_eq!(d.observe(LOCK_BUDGET), None, "at budget is within it");
        for _ in 1..OVERRUN_REPORTS {
            assert_eq!(d.observe(OVER), None);
        }
        assert!(!d.is_degraded(), "the streak restarted");
        assert_eq!(d.observe(OVER), Some(Edge::Degraded));
        assert!(d.is_degraded());

        for _ in 1..RECOVER_REPORTS {
            assert_eq!(d.observe(Duration::ZERO), None);
        }
        assert_eq!(d.observe(OVER), None);
        for _ in 1..RECOVER_REPORTS {
            assert_eq!(d.observe(Duration::ZERO), None);
        }
        assert!(d.is_degraded(), "the streak restarted");
        assert_eq!(d.observe(Duration::ZERO), Some(Edge::Recovered));
        assert!(!d.is_degraded());
    }

    /// **Graduated shedding** (PA-3): consecutive overrun reports deepen the
    /// shed 2 -> 4 -> 8 (`SHED_STEP_REPORTS` a step) and stop at 8; a report
    /// within budget resets the count (the shed is back to 2 while still
    /// degraded, and recovery needs its own streak).
    #[test]
    fn consecutive_overruns_shed_at_2_4_and_8() {
        let mut d = Degrade::default();
        assert_eq!(d.every(), 1);
        let mut seen = Vec::new();
        for _ in 0..(OVERRUN_REPORTS + 4 * SHED_STEP_REPORTS) {
            let edge = d.observe(OVER);
            seen.push((d.every(), edge));
        }
        let at = |n: u32| seen[(n - 1) as usize];
        assert_eq!(at(OVERRUN_REPORTS - 1), (1, None));
        assert_eq!(at(OVERRUN_REPORTS), (2, Some(Edge::Degraded)));
        assert_eq!(at(OVERRUN_REPORTS + SHED_STEP_REPORTS - 1), (2, None));
        assert_eq!(
            at(OVERRUN_REPORTS + SHED_STEP_REPORTS),
            (4, Some(Edge::Deeper(4)))
        );
        assert_eq!(
            at(OVERRUN_REPORTS + 2 * SHED_STEP_REPORTS),
            (8, Some(Edge::Deeper(8)))
        );
        let last = seen.last().unwrap();
        assert_eq!(*last, (8, None), "8 is the floor");
        // One report within budget resets the climb, not the degrade.
        assert_eq!(d.observe(Duration::ZERO), None);
        assert!(d.is_degraded());
        assert_eq!(d.every(), 2);
    }

    /// While shed to 8, a bot-only system is due one tick in eight (and a
    /// human system on every one).
    #[test]
    fn a_deep_shed_makes_a_bot_only_system_due_one_tick_in_eight() {
        let mut reg = two_pilots();
        undock(&mut reg, BOB, BOB_SHIP, C);
        degrade(&mut reg);
        for _ in 0..(2 * SHED_STEP_REPORTS) {
            note_report(&mut reg, OVER, None);
        }
        assert_eq!(reg.ext::<Liveness>().degrade.every(), SHED_MAX_EVERY);
        let counts = due_counts(&mut reg, &[C], SHED_MAX_EVERY * 4);
        assert_eq!(counts, vec![4]);
    }

    /// **Under a budget overrun, a bot-only hot system is ticked strictly
    /// less often than a human system** (C7 §5.3). Alice (human) is in space
    /// in A, Bob (bot) in C, and NPCs alone hold B. Before the overrun every
    /// system is due on every tick. Degraded, A still is; C and B are due on
    /// strictly fewer ticks, yet never starved: at least once in every
    /// [`DEGRADED_EVERY`] ticks. Recovered, every system is due again.
    #[test]
    fn under_a_budget_overrun_a_bot_only_system_ticks_less_often_than_a_human_one() {
        let mut reg = two_pilots();
        undock(&mut reg, ALICE, ALICE_SHIP, A);
        undock(&mut reg, BOB, BOB_SHIP, C);
        reg.npcs_mut().insert(crate::npc::Npc::new(
            9_000_000_001,
            &crate::npc::ANGEL_HIJACKER,
            B,
        ));
        let n = DEGRADED_EVERY * 4;
        assert_eq!(due_counts(&mut reg, &[A, B, C], n), vec![n; 3]);
        let live = reg.ext::<Liveness>();
        assert_eq!(
            (live.get(A).unwrap().humans, live.get(A).unwrap().bots),
            (1, 0)
        );
        assert_eq!(
            (live.get(C).unwrap().humans, live.get(C).unwrap().bots),
            (0, 1)
        );
        assert_eq!(live.get(B).unwrap().humans, 0);

        degrade(&mut reg);
        let counts = due_counts(&mut reg, &[A, B, C], n);
        assert_eq!(counts[0], n, "the human system is never skipped");
        for bot_only in &counts[1..] {
            assert!(*bot_only < counts[0], "{counts:?}");
            assert!(*bot_only >= n / DEGRADED_EVERY, "never starved: {counts:?}");
        }

        for _ in 0..RECOVER_REPORTS {
            note_report(&mut reg, Duration::ZERO, None);
        }
        assert_eq!(due_counts(&mut reg, &[A, B, C], n), vec![n; 3]);
    }

    /// **A system with a human is never skipped**, however the human is
    /// there: docked beside a bot in space, or on a jump into a system only
    /// a bot holds. When the human is gone, the system is a bot-only one and
    /// degrades like any other.
    #[test]
    fn a_system_with_a_human_is_never_skipped() {
        let mut reg = two_pilots();
        undock(&mut reg, BOB, BOB_SHIP, A);
        degrade(&mut reg);
        let n = DEGRADED_EVERY * 4;
        assert_eq!(reg.character(ALICE).unwrap().solar_system_id, Some(A));
        assert_eq!(due_counts(&mut reg, &[A], n), vec![n], "Alice docked in A");
        assert_eq!(reg.ext::<Liveness>().tier(A), Tier::Hot);

        // Bob moves to B; Alice undocks in A and jumps to B.
        reg.roster_mut().leave(BOB);
        undock(&mut reg, BOB, BOB_SHIP, B);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let args = [
            PyValue::Int(GATE_A),
            PyValue::Int(GATE_B),
            PyValue::Int(ALICE_SHIP),
        ];
        crate::jump::cmd_stargate_jump(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdStargateJump",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("the jump starts");
        assert_eq!(due_counts(&mut reg, &[B], n), vec![n], "Alice departing");
        crate::jump::tick(&mut reg, Instant::now() + crate::jump::JUMP_DELAY);
        assert_eq!(due_counts(&mut reg, &[B], n), vec![n], "Alice arriving");

        // Alice lands nowhere and logs off: B is Bob's alone.
        let _ = crate::jump::take_arrival(&mut reg, ALICE);
        reg.session_closed(1);
        assert!(!reg.jumps().contains(ALICE) && !reg.roster().is_in_space(ALICE));
        assert!(due_counts(&mut reg, &[B], n)[0] < n);
    }

    /// **Promote and demote with hysteresis, in `COOL_AFTER` units.** A pilot
    /// in space makes A hot and its gate neighbour B warm. Hot holds for as
    /// long as the pilot does, however long. When the pilot goes, A is warm
    /// at once, stays warm until one tick short of `COOL_AFTER`, and is cold
    /// (no entry) at `COOL_AFTER`. A pilot returning mid-cooling takes A
    /// straight back to hot without passing cold, and the cooling restarts
    /// from the new departure.
    #[test]
    fn promote_and_demote_with_hysteresis_in_cool_after_units() {
        let mut reg = world(false);
        assert!(at(&mut reg, T0).is_empty());

        in_space(&mut reg, A);
        let live = at(&mut reg, T0);
        assert_eq!(live.tier(A), Tier::Hot);
        assert!(live.get(A).unwrap().reasons.contains(Reasons::PILOT));
        assert_eq!(live.tier(B), Tier::Warm);
        assert_eq!(live.get(B).unwrap().reasons, Reasons::NEIGHBOUR);
        assert_eq!(live.tier(C), Tier::Cold);
        let t1 = T0 + 5 * COOL_AFTER;
        assert_eq!(at(&mut reg, t1).tier(A), Tier::Hot, "hot never cools");

        reg.roster_mut().leave(ALICE);
        assert_eq!(at(&mut reg, t1 + 1).tier(A), Tier::Warm);
        assert_eq!(at(&mut reg, t1 + COOL_AFTER - 1).tier(A), Tier::Warm);
        assert_eq!(at(&mut reg, t1 + COOL_AFTER - 1).tier(B), Tier::Warm);

        // Back before cooling ends: hot again, never cold in between.
        let t2 = t1 + COOL_AFTER - 1;
        in_space(&mut reg, A);
        assert_eq!(at(&mut reg, t2).tier(A), Tier::Hot);
        reg.roster_mut().leave(ALICE);
        assert_eq!(at(&mut reg, t2 + COOL_AFTER - 1).tier(A), Tier::Warm);
        let live = at(&mut reg, t2 + COOL_AFTER);
        assert_eq!(live.tier(A), Tier::Cold);
        assert_eq!(live.tier(B), Tier::Cold);
        assert!(live.get(A).is_none() && live.is_empty());
    }

    /// **A jump in flight warms its destination**, before the arrival: on
    /// the departing leg and on the arriving one. Not through the neighbour
    /// rule: the jumper has left the roster, so nothing is hot, and the
    /// origin is not "docked" either.
    #[test]
    fn a_jump_in_flight_warms_its_destination() {
        let mut reg = world(true);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        assert!(reg.roster().is_in_space(ALICE));
        let args = [
            PyValue::Int(GATE_A),
            PyValue::Int(GATE_B),
            PyValue::Int(ALICE_SHIP),
        ];
        crate::jump::cmd_stargate_jump(
            &mut reg,
            &Request {
                service: "beyonce",
                method: "CmdStargateJump",
                args: &args,
                kwargs: &[],
                bind_params: None,
                actor: Some(ALICE),
            },
        )
        .expect("the jump starts");

        let live = at(&mut reg, T0);
        assert_eq!(live.tier(B), Tier::Warm);
        assert_eq!(live.get(B).unwrap().reasons, Reasons::JUMP);
        assert!(live.hot().next().is_none(), "the jumper is in no system");
        assert!(
            live.get(A)
                .is_none_or(|a| !a.reasons.contains(Reasons::DOCKED))
        );

        crate::jump::tick(&mut reg, Instant::now() + crate::jump::JUMP_DELAY);
        let live = at(&mut reg, T0 + 1);
        assert_eq!(live.get(B).unwrap().reasons, Reasons::JUMP, "arriving leg");
    }

    /// **A cold system has no entry**; the rest of the reasons. An empty
    /// world indexes nothing. NPCs alone keep a system hot, with nobody in
    /// space. An online pilot who is docked keeps their system warm for as
    /// long as they stay, past any `COOL_AFTER`; logging off starts the
    /// cooling.
    #[test]
    fn a_cold_system_has_no_entry_and_npcs_and_docked_pilots_count() {
        let mut reg = world(false);
        let live = at(&mut reg, T0);
        assert!(live.is_empty());
        assert_eq!(live.tier(A), Tier::Cold);

        reg.npcs_mut().insert(crate::npc::Npc::new(
            9_000_000_001,
            &crate::npc::ANGEL_HIJACKER,
            C,
        ));
        let live = at(&mut reg, T0);
        assert_eq!(live.tier(C), Tier::Hot);
        assert_eq!(live.get(C).unwrap().reasons, Reasons::BALLS);
        assert_eq!(live.len(), 1, "C has no gates, so no warm neighbour");

        let mut reg = world(true);
        assert_eq!(at(&mut reg, T0).get(A).unwrap().reasons, Reasons::DOCKED);
        let live = at(&mut reg, T0 + 3 * COOL_AFTER);
        assert_eq!(live.tier(A), Tier::Warm);
        assert_eq!(live.hot().count(), 0);
        reg.session_closed(1);
        assert_eq!(at(&mut reg, T0 + 4 * COOL_AFTER - 1).tier(A), Tier::Warm);
        assert!(at(&mut reg, T0 + 4 * COOL_AFTER).is_empty());
    }
}
