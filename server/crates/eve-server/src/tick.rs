//! **The world clock's stages** -- W10.5 seam S6 (`docs/content/plan.md`
//! §6.1).
//!
//! `push::spawn_world_ticker` used to call each subsystem's tick by hand,
//! one line per subsystem, under the world lock. Those calls are now the
//! rows of [`STAGES`], and the ticker calls [`run_all`] once. A new
//! subsystem that needs the clock exports a `fn(&mut Registry, Instant,
//! BlueTime)` and the coordinator adds its row here; `push.rs` does not
//! change.
//!
//! **The order is the dependency order, and it is today's order** (the
//! hand-written calls this replaced, verbatim):
//!
//! 0. `liveness` -- which systems are cold, warm and hot (W11 CP6), first
//!    so every later stage can read this tick's hot list; and right after
//!    it `asteroid`, which writes the last tick's bites as `rock` deltas,
//!    drops the rocks of systems that went cold and lays out the ones that
//!    went warm (CP14');
//! 1. `modules` -- capacitor drains, stops, and the `Cycle`s damage consumes;
//! 2. `combat` -- every cycle just started, as volleys, kills and wrecks;
//! 3. `mining` -- every cycle that just *ended*, as ore;
//! 4. `npc` -- see, lock, move and shoot; after combat, so a ship killed
//!    this tick is already out of the roster (`docs/lanes/npc.md` §5 S4);
//!    then `missile`, every missile in flight (player launches come from
//!    `combat`, NPC ones from `npc`), before `grid`;
//! 5. `jump` -- stargate session changes that are due;
//! 6. `skill_training` -- every level whose end time has passed.
//!
//! All of them run before the ticker's `take_due`, so what they schedule
//! leaves in the same tick. `targeting::sweep_out_of_range` is **not** a
//! stage: it returns a count the ticker keeps, so it stays an explicit call
//! before [`run_all`].

use crate::damage::BlueTime;
use crate::service::Registry;
use std::time::{Duration, Instant};

/// One row of the world clock: a name (for timings) and the work.
///
/// `run` gets the tick's monotonic instant (for the queues keyed on
/// [`Instant`], `jump`'s) and its blue time (for the ones keyed on the
/// game clock, everything else).
#[derive(Clone, Copy)]
pub struct Stage {
    pub name: &'static str,
    pub run: fn(&mut Registry, Instant, BlueTime),
    /// **Cadence** (PA-3): the stage runs on the ticks where
    /// `beat % every == phase` (`beat` is [`crate::liveness::beat`]). `1` is
    /// every tick. Above `1` only where `tick_budget::CADENCE_OK` says the
    /// stage tolerates it: its work is keyed on stored timestamps, so a late
    /// run does what an early one would have. `phase` staggers the cadenced
    /// stages so they do not all land on one tick.
    pub every: u8,
    pub phase: u8,
}

impl Stage {
    /// Whether the stage runs on beat number `beat`.
    pub fn due(&self, beat: u64) -> bool {
        self.every <= 1 || beat % u64::from(self.every) == u64::from(self.phase)
    }
}

/// Every stage, in the order the clock runs them. **Coordinator-only**
/// after S6: a packet exports its stage function and names the row in its
/// report.
pub const STAGES: &[Stage] = &[
    Stage {
        name: "liveness",
        every: 1,
        phase: 0,
        run: crate::liveness::refresh,
    },
    Stage {
        name: "asteroid",
        every: 1,
        phase: 0,
        run: crate::asteroid::sync,
    },
    // MN-C: after `asteroid` and `liveness`: the moon belts of every live
    // system are laid out, bitten rocks written as rows, expired belts replaced.
    Stage {
        name: "moon_belt",
        every: 1,
        phase: 0,
        run: crate::moon_belt::tick,
    },
    // MN-D: survey probes whose time is up report to the Moon Analysis window.
    Stage {
        name: "moon_scan",
        every: 1,
        phase: 0,
        run: crate::moon_scan::tick,
    },
    Stage {
        name: "modules",
        every: 1,
        phase: 0,
        // The count of cycle boundaries is dropped, as the ticker did.
        run: |w, _, ft| {
            crate::modules::tick(w, ft);
        },
    },
    // Right after `modules`, whose stops it reads: what a tackled pilot's
    // client is told, a scrammed MWD stopped, a tackler's aggression (W16
    // CP57).
    // UNDOCK-INVULN: the cancels of the grants that ran out or whose ship
    // left space.
    Stage {
        name: "invuln",
        every: 1,
        phase: 0,
        run: crate::invuln::tick,
    },
    // CLOAK-1: proximity decloak, a cloak whose module is gone, and the
    // forgetting of a cloak whose ship left space. After `modules`, whose
    // stops it reads.
    Stage {
        name: "cloak",
        every: 1,
        phase: 0,
        run: crate::cloak::tick,
    },
    // NW-9a: the Lance's damage ticks and the legacy doomsday's strike.
    Stage {
        name: "superweapon",
        every: 1,
        phase: 0,
        run: crate::superweapon::tick,
    },
    // SYS-3: a pilot who arrived in, or left, a system with an effect beacon
    // is told (Start/Stop and the attribute pushes). After the roster writes
    // of the jump and the bind.
    Stage {
        name: "system_effects",
        every: 1,
        phase: 0,
        run: crate::system_effect::tick,
    },
    Stage {
        name: "tackle",
        every: 1,
        phase: 0,
        run: crate::tackle::tick,
    },
    // After `tackle` (and `modules`): the web, ECM, dampener and painter
    // cycles, the target's client told, the jam rolled (K-EWAR).
    Stage {
        name: "ewar",
        every: 1,
        phase: 0,
        run: crate::ewar::tick,
    },
    // After `modules` (the cycles it started) and before `combat`, so a
    // rep lands before the next volley of the same tick (K-LOGI).
    Stage {
        name: "remote_rep",
        every: 1,
        phase: 0,
        run: crate::remote_rep::tick,
    },
    // After `modules` (the cycles it started): rack heat, the rolls of the
    // overloaded cycles, repairs (K-OVERHEAT).
    Stage {
        name: "overheat",
        every: 1,
        phase: 0,
        run: crate::overheat::tick,
    },
    // A weapon bank whose member left the fitting is dissolved (K-OVERHEAT).
    Stage {
        name: "weapon_bank",
        every: 1,
        phase: 0,
        run: crate::weapon_bank::tick,
    },
    // After `modules` (the burst cycles it started) and before `combat`,
    // so a boost lands before the next volley of the same tick (K-BOOST).
    Stage {
        name: "dbuff",
        every: 1,
        phase: 0,
        run: crate::dbuff::tick,
    },
    Stage {
        name: "combat",
        every: 1,
        phase: 0,
        run: |w, _, ft| crate::combat::tick(w, ft),
    },
    Stage {
        name: "mining",
        every: 1,
        phase: 0,
        run: |w, _, ft| crate::mining::tick(w, ft),
    },
    // After `mining`, which hands hacking its completed analyzer cycles:
    // a game whose hacker warped, left or drifted out of range is lost
    // (R-HACK HK2).
    Stage {
        name: "hacking",
        every: 1,
        phase: 0,
        run: crate::hacking::tick,
    },
    Stage {
        name: "npc",
        every: 1,
        phase: 0,
        run: |w, _, ft| crate::npc::tick(w, ft),
    },
    // Right after `npc` (NPC launches) and after `combat` (player
    // launches, M4): every missile in flight flown up to now, the hits
    // landed through `combat::missile_volley` and the ended ones removed
    // (R-MISSILE M3; W17 CP45's `npc_missile` stage before it).
    Stage {
        name: "missile",
        every: 1,
        phase: 0,
        run: crate::missile::tick,
    },
    // After `npc`, whose scram and web cycles it holds: what an NPC-tackled
    // pilot's client is told, and the webbed ship's top speed (W18 CP49).
    Stage {
        name: "npc_ewar",
        every: 1,
        phase: 0,
        run: crate::npc_ewar::tick,
    },
    // After every scram source (`tackle`, `npc_ewar`, fighters): a warp
    // still aligning under a scram is stopped.
    Stage {
        name: "warp_abort",
        every: 1,
        phase: 0,
        run: |w, _, _| crate::movement_guard::abort_scrammed_warps(w),
    },
    // After `npc`, whose locks it reads, and `npc_ewar`, whose scram and web
    // holds it renews (CONCORD-1a).
    Stage {
        name: "concord",
        every: 1,
        phase: 0,
        run: crate::concord::tick,
    },
    Stage {
        name: "jump",
        every: 1,
        phase: 0,
        run: |w, now, _| crate::jump::tick(w, now),
    },
    // SESSION-FIX2: a safe logoff whose 30 s are up leaves space and logs
    // the pilot off.
    Stage {
        name: "safe_logoff",
        every: 1,
        phase: 0,
        run: crate::safe_logoff::tick,
    },
    // GAP-SELFDESTRUCT: a sequence whose 2 minutes are up blows the ship.
    Stage {
        name: "self_destruct",
        every: 1,
        phase: 0,
        run: crate::self_destruct::tick,
    },
    Stage {
        name: "skill_training",
        every: 1,
        phase: 0,
        run: |w, _, ft| crate::service::skill_training_tick(w, ft),
    },
    // BOOST: boosters whose time has come; one comparison while none is due.
    Stage {
        name: "boosters",
        every: 1,
        phase: 0,
        run: crate::boosters::tick,
    },
    // E6: every bounty payout whose period is up, as one ref-85 row each.
    Stage {
        name: "bounty_payout",
        every: 4,
        phase: 3,
        run: crate::npc::bounty::tick,
    },
    // X-GAPS: a contract that ended is announced (notification 54) when it
    // happens. Reads only the `insurance` rows, so its place is free.
    Stage {
        name: "insurance",
        every: 1,
        phase: 0,
        run: crate::service::insurance::tick,
    },
    // After `liveness` and before `signal_tracker`, so a site that appears
    // this tick goes out in this tick's tracker update (W13 CP21).
    Stage {
        name: "population",
        every: 2,
        phase: 1,
        run: crate::population::tick,
    },
    // Before `wormhole`, so a hole that collapsed (spent or expired) is
    // re-read by CP54a in the same tick (W17 CP54b).
    Stage {
        name: "wormhole_jump",
        every: 1,
        phase: 0,
        run: crate::wormhole_jump::tick,
    },
    // #88: a Needlejack group whose spool ended leaves.
    Stage {
        name: "random_jump",
        every: 1,
        phase: 0,
        run: crate::random_jump::tick,
    },
    // After `population` and before `site` and `signal_tracker`: wormhole
    // signatures go out in this tick's tracker update, and a pilot who
    // reached a hole gets its ball through the site path (W16 CP54a).
    Stage {
        name: "wormhole",
        every: 1,
        phase: 0,
        run: crate::wormhole::tick,
    },
    // #91: the Abyss probe's pockets: a pilot who arrived is told, a room the
    // site stage dropped comes back, a pocket out of time ends. Before
    // `site`, so a room made here is in this tick's pass.
    Stage {
        name: "abyss",
        every: 1,
        phase: 0,
        run: crate::abyss::tick,
    },
    // After `site_spawn` of the tick before (it queues the escalations) and
    // before `site`: an escalation is opened and a pilot who reached one gets
    // its room through the site path (Y-ESCAL).
    Stage {
        name: "escalation",
        every: 1,
        phase: 0,
        run: crate::escalation::tick,
    },
    // After `population` (the live sites) and `asteroid` (the field is
    // laid out): a pilot who reached a site gets its room (W14 CP28a).
    Stage {
        name: "site",
        every: 1,
        phase: 0,
        run: crate::site::tick,
    },
    // After `site` (the instances): a pilot who landed in one, or warped
    // out of one, is told (W15 CP36).
    Stage {
        name: "dungeon_tracking",
        every: 1,
        phase: 0,
        run: crate::dungeon_tracking::tick,
    },
    // After `dungeon_tracking` (who is inside an instance): squads,
    // triggers, respawn, completion and despawn (W15 CP28b).
    Stage {
        name: "site_spawn",
        every: 1,
        phase: 0,
        run: crate::site_spawn::tick,
    },
    // W17 CP53b: pulls completed FW complexes out of `population`'s slots.
    Stage {
        name: "facwar",
        every: 4,
        phase: 0,
        run: crate::facwar::tick,
    },
    // After `jump`, so a pilot who left a system this tick is pruned from
    // its tracker before its deltas go out (W12 CP17).
    Stage {
        name: "signal_tracker",
        every: 1,
        phase: 0,
        run: crate::signal_tracker::tick,
    },
    // SOC-D3a: the entosis links' fuel, every hub's score and completions,
    // the window edges and the campaigns whose time came. After `modules`
    // (the links' cycles) and before `sov_capital` and `rollover`.
    Stage {
        name: "entosis",
        every: 1,
        phase: 0,
        run: crate::entosis::tick,
    },
    // SOC-D4: a pending alliance capital whose time has come moves and
    // pushes `OnCapitalSystemChanged`. Reads only the few `sovcap` rows.
    Stage {
        name: "sov_capital",
        every: 4,
        phase: 2,
        run: crate::sov_index::tick,
    },
    // C8: wars that became fightable, allies that joined, offers, invites and
    // bills that fell due. After `skill_training`, before `rollover`; it reads
    // only its own rows, so its place is free.
    Stage {
        name: "war",
        every: 4,
        phase: 1,
        run: crate::war::tick,
    },
    // U3: every Upwell structure's state machine (anchoring, onlining,
    // decommissioning) is stepped from its stored timestamps and the slim
    // changes go out. Reads only the `structure` rows.
    Stage {
        name: "structure_space",
        every: 4,
        phase: 3,
        run: crate::structure_space::tick,
    },
    // F1: a dropped pilot's fleet place runs out `RECONNECT_TIMEOUT` after
    // the drop. Reads only the fleets with a gone member.
    Stage {
        name: "fleet_reconnect",
        every: 1,
        phase: 0,
        run: crate::fleet::tick,
    },
    // E1: every `SWEEP_EVERY_H` game hours, a slice of the parked bots'
    // offline careers is settled. After the rocks are captured and the
    // bounty payouts paid, before the rollover reads the journal.
    Stage {
        name: "econ_agents",
        every: 1,
        phase: 0,
        run: crate::econ_agents::tick,
    },
    // Last, so a rollover sees this tick's tiers and the rocks `asteroid`
    // captured; slices of `rollover::SLICE` live systems a tick (W13 CP25).
    Stage {
        name: "rollover",
        every: 1,
        phase: 0,
        run: crate::rollover::tick,
    },
    // The chat directory (PA-11): its own 1 s clock, so no cadence here.
    // Before `grid`, which keeps the last slice to itself and the drain;
    // chat only needs sessions, which no stage after this one changes.
    Stage {
        name: "chat_dir",
        every: 1,
        phase: 0,
        run: crate::chat_dir::tick,
    },
    // Last of all, so every ball this tick spawned, moved or removed is in
    // the world before the grids are measured, and what it schedules leaves
    // in this tick's `take_due` (GRID-B, `crate::grid`).
    Stage {
        name: "grid",
        every: 1,
        phase: 0,
        run: crate::grid::tick,
    },
];

/// Run the stages of `stages[range]` in order, and say how long each took.
/// A stage that is not due on this beat reports a zero timing, so the
/// stages line still names every row.
///
/// The ticker passes [`STAGES`]; a test passes its own table.
pub fn run_range(
    stages: &[Stage],
    range: std::ops::Range<usize>,
    w: &mut Registry,
    now: Instant,
    ft: BlueTime,
) -> Vec<(&'static str, Duration)> {
    stages[range]
        .iter()
        .map(|s| {
            // Read per stage, not per range: `liveness` (stage 0) is the
            // only writer, so the beat is the same for every stage after it
            // however the table is cut into slices.
            if !s.due(crate::liveness::beat(w)) {
                return (s.name, Duration::ZERO);
            }
            let start = Instant::now();
            (s.run)(w, now, ft);
            (s.name, start.elapsed())
        })
        .collect()
}

/// Run every stage of `stages` in order, and say how long each took.
#[cfg(test)]
pub fn run_stages(
    stages: &[Stage],
    w: &mut Registry,
    now: Instant,
    ft: BlueTime,
) -> Vec<(&'static str, Duration)> {
    run_range(stages, 0..stages.len(), w, now, ft)
}

/// Run [`STAGES`]. The timings are unused until CP15 (tick observability,
/// plan §4.6).
#[cfg(test)]
pub fn run_all(w: &mut Registry, now: Instant, ft: BlueTime) -> Vec<(&'static str, Duration)> {
    run_stages(&crate::module::plan().stages, w, now, ft)
}

/// **The lock slices** (PA-3): the first stage of every slice after the
/// first. The clock takes the world write lock once per slice
/// ([`crate::push::clock_slices`]), so a payload turn can run between two.
/// The last slice (from `grid`) also holds the shutdown countdown, the
/// drain, the session changes and the save: reconcile -> filter -> stamp is
/// one unit, because a payload turn's `SetState` between a reconcile and the
/// filter of what it scheduled is the Parks race `grid.rs` warns about.
///
/// # What crosses a slice boundary (the scratch-state note)
///
/// A stage's hand-over to a later stage in the same tick is either a field
/// that persists in the world (safe across a payload turn: that turn is
/// what a tick boundary already admits) or a value the *clock* carries.
/// Group by group:
///
/// 1. **prep** (`sweep`, `liveness` .. `moon_scan`): the sweep's drop count
///    is carried in `ClockTurn`; the liveness index is a world slot, read by
///    every later stage. A pilot who undocks between slices is hot from the
///    next tick, exactly as one who undocks just after a tick.
/// 2. **combat** (`modules` .. `hacking`): `modules::tick` records the
///    cycles it started in `Modules`, and `combat::tick` drains them with
///    `take_cycles` (`mining` and `hacking` read the ended ones). That is
///    why the whole chain is one slice: nothing may take or add a cycle in
///    between. `tackle`, `ewar`, `remote_rep`, `overheat`, `dbuff` read the
///    stops and cycles `modules` left; also inside.
/// 3. **ai** (`npc` .. `concord`): `npc` reads the roster `combat` left
///    (a kill is already out of it), its launches are in the missile table
///    `missile` flies, `npc_ewar` reads the holds `npc` made, and
///    `warp_abort` the scrams of every source. No carried value; a payload
///    turn between slices 2 and 3 sees a consistent post-combat world.
/// 4. **movement and queues** (`jump` .. `insurance`): each stage reads its
///    own rows and timestamps.
/// 5. **sites** (`population` .. `signal_tracker`): `population` keeps the
///    sites in world slots, `site`/`site_spawn` read them, the tracker
///    queue (world state) is flushed by `signal_tracker` after `jump`'s
///    prune, which is in an earlier slice, so the order holds.
/// 6. **society** (`entosis` .. `rollover`): own clocks and slices.
/// 7. **fan-out** (`grid` + shutdown + drain + changes + save).
///
/// `now` and the blue time `ft` are fixed for the whole tick, so a slice
/// that runs after a payload turn still computes against the tick's time.
pub const SLICE_STARTS: &[&str] = &["modules", "npc", "jump", "population", "entosis", "grid"];

/// The stage ranges of the slices of `stages`, cut at [`SLICE_STARTS`]:
/// the first starts at 0, the last ends at `stages.len()`.
pub fn slice_ranges(stages: &[Stage]) -> Vec<std::ops::Range<usize>> {
    let mut cuts = vec![0];
    for start in SLICE_STARTS {
        if let Some(at) = stages.iter().position(|s| s.name == *start) {
            cuts.push(at);
        }
    }
    cuts.push(stages.len());
    cuts.windows(2).map(|c| c[0]..c[1]).collect()
}

#[cfg(test)]
pub(crate) mod stage_bench;

#[cfg(test)]
mod tests {
    use super::*;

    fn position(name: &str) -> usize {
        STAGES
            .iter()
            .position(|s| s.name == name)
            .unwrap_or_else(|| panic!("no stage named {name}"))
    }

    /// **The order is the dependency order.** Modules before combat, because
    /// combat consumes the cycles `modules::tick` records
    /// (`modules_mut().take_cycles()`); combat before npc, because a ship
    /// combat killed this tick must already be out of the roster when the
    /// NPCs look for targets; and every stage before the ticker's
    /// `take_due`, which holds because [`run_all`] runs the whole table and
    /// only then returns -- one timing per stage, in table order -- and
    /// `push.rs` calls `take_due` after it. Names are unique, so a timing
    /// names one stage.
    #[test]
    fn stage_order_is_the_dependency_order() {
        assert!(
            position("liveness") < position("asteroid"),
            "CP14' reads tiers"
        );
        assert!(position("asteroid") < position("mining"));
        assert!(position("modules") < position("combat"));
        assert!(position("combat") < position("npc"));
        assert!(position("npc") < position("missile"), "NPC launches fly");
        assert!(position("missile") < position("grid"));
        let mut names: Vec<&str> = STAGES.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), STAGES.len(), "stage names are unique");

        let mut w = Registry::new(crate::model::SandboxConfig::default());
        let ran = run_all(&mut w, Instant::now(), crate::pyutil::filetime_now());
        let ran: Vec<&str> = ran.into_iter().map(|(name, _)| name).collect();
        let table: Vec<&str> = STAGES.iter().map(|s| s.name).collect();
        assert_eq!(ran, table, "run_all runs every stage, in table order");
    }

    /// **The slices partition the table**: contiguous, in order, covering
    /// every stage once; every cut names a stage; the `modules`..`hacking`
    /// chain that shares `take_cycles` is one slice, and `grid` opens the
    /// last one (the slice that also drains).
    #[test]
    fn slices_partition_the_table_and_keep_their_chains_whole() {
        let ranges = slice_ranges(STAGES);
        assert_eq!(ranges.len(), SLICE_STARTS.len() + 1);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges.last().unwrap().end, STAGES.len());
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "contiguous");
        }
        assert!(ranges.iter().all(|r| !r.is_empty()), "no empty slice");
        for name in SLICE_STARTS {
            assert!(
                STAGES.iter().any(|s| s.name == *name),
                "cut at unknown {name}"
            );
        }
        let slice_of = |name: &str| {
            ranges
                .iter()
                .position(|r| r.contains(&position(name)))
                .unwrap()
        };
        for chain in [
            ["modules", "combat", "mining", "hacking"].as_slice(),
            [
                "tackle",
                "ewar",
                "remote_rep",
                "overheat",
                "dbuff",
                "combat",
            ]
            .as_slice(),
            ["npc", "missile", "npc_ewar", "warp_abort", "concord"].as_slice(),
            [
                "population",
                "wormhole",
                "site",
                "site_spawn",
                "signal_tracker",
            ]
            .as_slice(),
        ] {
            let first = slice_of(chain[0]);
            assert!(
                chain.iter().all(|n| slice_of(n) == first),
                "{chain:?} is split"
            );
        }
        assert_eq!(ranges.last().unwrap().start, position("grid"));
        assert_eq!(
            ranges.last().unwrap().len(),
            1,
            "grid alone before the drain"
        );
    }

    /// **TICK-1's bench**: many systems of persistent state, few pilots.
    /// Run in release with
    /// `tools/dev.sh cargo test --release --locked -p eve-server tick_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tick_bench() {
        let n_char = 300;
        let n_sys = 400;
        let characters = (0..n_char)
            .map(|i| crate::model::Character {
                character_id: 2_000_000 + i,
                name: format!("Off{i}"),
                ship_id: 200_000_000 + i,
                ..crate::model::Character::default()
            })
            .collect();
        let mut w = Registry::new(crate::model::SandboxConfig {
            characters,
            ..crate::model::SandboxConfig::default()
        });
        let now = crate::pyutil::filetime_now();
        for i in 0..n_char {
            let q = w.skill_queues.entry(2_000_000 + i);
            q.entries = vec![(3300, 5)];
            q.start = Some(now);
        }
        for i in 0..n_sys {
            let sys = 31_000_000 + i;
            crate::sov::set_claim(
                w.deltas_mut(),
                sys,
                Some((99_000_000 + i, 98_000_000 + i)),
                now,
            );
            crate::sov_hub::put(
                w.deltas_mut(),
                &crate::sov_hub::Hub {
                    system_id: sys,
                    hub_id: 700_000_000 + i,
                    position: [0.0; 3],
                    launcher_id: None,
                    launch_time: now,
                    offline_days: 0,
                },
            );
        }
        for i in 0..100 {
            for k in 0..5 {
                crate::npc::spawn(
                    &mut w,
                    &crate::npc::FIXTURE_RAT,
                    32_000_000 + i,
                    [k as f64 * 1000.0, 0.0, 0.0],
                );
            }
        }
        let mut sum: std::collections::BTreeMap<&'static str, (Duration, Duration)> =
            Default::default();
        let warmup = 40;
        let ticks = 400;
        let mut ft = now;
        let mut totals: Vec<Duration> = Vec::new();
        for n in 0..warmup + ticks {
            ft += 2_500_000;
            let ran = run_all(&mut w, Instant::now(), ft);
            if n < warmup {
                continue;
            }
            totals.push(ran.iter().map(|r| r.1).sum());
            for (name, d) in ran {
                let e = sum.entry(name).or_default();
                e.0 += d;
                e.1 = e.1.max(d);
            }
        }
        let mean: Duration = totals.iter().sum::<Duration>() / ticks;
        let max = totals.iter().max().copied().unwrap_or_default();
        println!("tick_bench tick work: mean {mean:?} max {max:?}");
        let mut rows: Vec<_> = sum.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
        for (name, (t, m)) in rows.iter().take(8) {
            println!("  {name:16} mean {:?} max {:?}", *t / ticks, m);
        }
    }
}
