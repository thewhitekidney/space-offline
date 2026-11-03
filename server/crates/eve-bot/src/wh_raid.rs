//! **The J-space raiders** (T3-SALVAGE, `docs/lanes/t3-salvage.md`): the last
//! input of a Tengu kit, Ancient Salvage, from the wrecks of Sleepers.
//!
//! Y-WH put wormhole space in the world: Sleeper combat anomalies whose wrecks
//! yield Ancient Salvage, and guarded relic and data sites. Nobody took any of
//! it. This module is the bot that does, built on what exists:
//!
//! - The **raider** ([`ROLE`], career `wormhole_raider`) is the site runner
//!   ([`crate::site_runner`]) with a Salvager I fitted (X-SALV): the same
//!   fight, loot and salvage rules, in a hull the combat ladder
//!   (`progression`, PROG-PVE2) has grown past a frigate. It dives only when
//!   its hull can run what is on the other side ([`want_dive`]).
//! - **By the difficulty cap.** A hull's cap is [`site_runner::difficulty_cap`]
//!   (destroyers 3, cruisers 4, battleships 5); Sleeper anomalies start at
//!   difficulty 3 (Perimeter [V-data]), so a frigate never dives, and a hull
//!   is sent through a hole only to a class whose sites it can run
//!   ([`max_class`], [`class_difficulty`]).
//! - **The trip** is the brain's, turn by turn, not a blocking run: in
//!   k-space the raider is a site runner until it may dive; then
//!   [`Op::In`] ([`crate::wh_dive::dive_in_where`]: scan a wormhole, read the
//!   class off its slim, jump). In J-space [`inside`] runs the site runner's
//!   rules (fight the squads, loot, salvage each wreck), and, when the
//!   Agency has no combat anomaly left, [`Op::Sweep`]s: scan down a relic or
//!   data signature, warp, fight its guards, and when the grid has been quiet
//!   for the settle, post the site as **guard-cleared** on the [`Board`].
//!   Then [`Op::Out`] (the K162 nearest the arrival,
//!   [`crate::wh_dive::come_out`]), home, sell.
//! - **Relic and data diving** is the hacker's, after the raiders
//!   ([`hack_dive_due`], [`hack_dive`]): relic and data guards stand on
//!   warp-in, so an unescorted frigate dies; the hacker goes in only through
//!   a hole whose far system holds a guard-cleared site, and works only those
//!   ([`crate::hacker::run_dive`]).
//! - **The loot** is sold the way every ratter's is (`escort::sell_loot`):
//!   salvage is loot, listed or sold, and the builders buy it from the book
//!   for the T3 kit (`t3::bill` names the Ancient Salvage a kit lacks;
//!   `t3::kit_note` prints it). Nothing is handed to a builder directly.
//!
//! # What a bot cannot do yet
//!
//! It cannot flee a fight in J-space (leaving takes a scan and a warp), so
//! it stands and fights ([`inside`]); it does not budget wormhole mass; and
//! the guard-cleared board is per process.
//!
//! Every rule and number here is **[A] policy**, each with its falsifier.

use crate::brain::{self, Act, ActKind, GIVE_UP_AFTER, Homing, Memory, Observation};
use crate::explore_roam as roam;
use crate::probe;
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::site_runner;
use crate::wh_dive::{self, Leads};
use crate::{Conn, Error, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---- the numbers ------------------------------------------------------------

/// The lowest difficulty of a Sleeper combat anomaly: Perimeter 3
/// **[V-data]** (`y-wh.md`: the client's own difficulty field agrees).
pub const MIN_DIFFICULTY: i64 = 3;
/// **[A]** The difficulty a class's sites run at, C1..C6: Perimeter sites
/// (3) in C1-C2, Frontier (4) in C3-C4, Core (5) in C5-C6, as the
/// EvEJS facts group them. Falsified by a raider lost in a class this says
/// it can run, or one that finds no anomaly it fits in a class this says it
/// can.
pub const CLASS_DIFFICULTY: [i64; 6] = [3, 3, 4, 4, 5, 5];
/// **[A]** A sweep of guarded relic and data sites wants a hull this many
/// difficulty steps over the lowest anomaly's: their guards are many
/// squads at once. Falsified by sweepers lost at warp-in (too low) or
/// guards never cleared by a hull that could (too high).
pub const SWEEP_MIN_CAP: i64 = 4;
/// **[A]** The longest a raider stays in one dive: the wormhole's own
/// lifetime is hours, and the hold (wrecks, salvage) fills first. Falsified
/// by raiders that come home with a third of a hold (too short) or whose
/// hole collapsed behind them (too long).
pub const RAID_CEILING: Duration = Duration::from_secs(2 * 3_600);
/// **[A]** Sites swept per dive. Falsified by hackers finding no cleared
/// site (too few) or sweepers that never leave (too many).
pub const MAX_SWEEPS: usize = 2;
/// **[A]** How long after a dive before the next: the bot docks, sells and
/// runs k-space anomalies in between. Falsified by hulls that spend the day
/// in J-space (too short) or never go (too long).
pub const DIVE_REST: Duration = Duration::from_secs(20 * 60);
/// **[A]** How long after a dive that found no way in before the next try.
/// Falsified by raiders that never retry a closed hole or retry every turn.
pub const DIVE_RETRY: Duration = Duration::from_secs(30 * 60);
/// **[A]** How long a guard-cleared site stays cleared: the population row's
/// minimum respawn for relic and data signatures (7,200 s,
/// `content/population.tsv`, `wh_relic_sig`). Falsified by hackers meeting
/// fresh guards.
pub const CLEARED_TTL: Duration = Duration::from_secs(7_200);
/// **[A]** How long a grid is given to show its guards after the bot lands
/// on it: the site stage places a room and spawns its squads on its own
/// tick, a few seconds behind the warp (`SITE_SETTLE` is the quiet the site
/// runner asks for, and it has been seen to pass before the Sleepers came).
/// A grid still empty after this is a site with nothing on it. Falsified by
/// raiders that leave an anomaly unfought (too short) or sit on empty
/// grids (too long).
pub const GRID_PATIENCE: Duration = Duration::from_secs(45);
/// **[A]** How long a dive waits, at most, for the system's scan overlay to
/// show a signature (a system's signatures reach the tracker on its first
/// sync, seconds after the undock). Falsified by raiders that give up a
/// dive on a freshly started server, or that wait out a system with none.
pub const SIGNATURE_PATIENCE: Duration = Duration::from_secs(90);
/// Ancient Salvage's group **[V-data]** (`t3-supply.md`: group 966).
pub const GROUP_ANCIENT_SALVAGE: i64 = 966;

/// **The raider's kit** (closed economy): the ratter's, and the scanner's
/// launcher and probes beside it, bought when lacking. **[A]**: a Core
/// launcher and one probe load. Falsified by raiders that cannot scan a
/// hole (more) or hold probes they never launch (fewer).
pub const RAID_KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: crate::salvage::RATTER_KIT.hull,
    modules: crate::salvage::RATTER_KIT.modules,
    ammo: crate::salvage::RATTER_KIT.ammo,
    skills: crate::salvage::RATTER_KIT.skills,
    more: &[
        (crate::salvage::SALVAGER_I, 1),
        (probe::CORE_LAUNCHER, 1),
        (probe::CORE_PROBE, probe::SETUP_PROBES),
    ],
};

// ---- the class rule ---------------------------------------------------------

/// The difficulty a class's sites run at ([`CLASS_DIFFICULTY`]); `None`
/// outside C1-C6.
pub fn class_difficulty(class: i64) -> Option<i64> {
    let i = usize::try_from(class.checked_sub(1)?).ok()?;
    CLASS_DIFFICULTY.get(i).copied()
}

/// **The highest class a hull of difficulty cap `cap` is sent to**: the
/// highest whose sites it runs. `None` when even C1 is too hard (a frigate).
pub fn max_class(cap: i64) -> Option<i64> {
    (1..=6)
        .rev()
        .find(|&c| class_difficulty(c).is_some_and(|d| d <= cap))
}

/// Whether a hull of cap `cap` may dive at all.
pub fn capable(cap: Option<i64>) -> bool {
    cap.is_some_and(|c| max_class(c).is_some())
}

// ---- the board --------------------------------------------------------------

/// Where a server's cleared sites are: `(server, J-space system)`.
type Pitch = (SocketAddr, i64);

/// The sites of one J-space system whose guards were killed.
#[derive(Debug, Clone, PartialEq)]
pub struct Cleared {
    /// The k-space system the raider left from: the hole's home.
    pub from: i64,
    /// Target id (`ABC-123`) and when it was cleared.
    pub targets: Vec<(String, Instant)>,
}

/// **The guard-cleared sites** (module header): raiders post, hackers read.
/// Per process, like [`site_runner::Board`]: the bots' own etiquette.
#[derive(Debug)]
pub struct Board {
    inner: Mutex<Option<BTreeMap<Pitch, Cleared>>>,
}

impl Board {
    pub const fn new() -> Board {
        Board {
            inner: Mutex::new(None),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<Pitch, Cleared>) -> R) -> R {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(BTreeMap::new))
    }

    /// A raider cleared `target` in J-space `system`, entered from `from`.
    pub fn post(&self, server: SocketAddr, system: i64, from: i64, target: &str, now: Instant) {
        self.with(|m| {
            let c = m.entry((server, system)).or_insert_with(|| Cleared {
                from,
                targets: Vec::new(),
            });
            c.from = from;
            c.targets.retain(|(t, _)| t != target);
            c.targets.push((target.to_owned(), now));
        });
    }

    /// Is `target` in `system` cleared, and not yet respawned?
    pub fn is_cleared(&self, server: SocketAddr, system: i64, target: &str, now: Instant) -> bool {
        self.with(|m| {
            m.get(&(server, system)).is_some_and(|c| {
                c.targets
                    .iter()
                    .any(|(t, at)| t == target && now.saturating_duration_since(*at) < CLEARED_TTL)
            })
        })
    }

    /// Whether some J-space system entered from k-space `from` holds a
    /// cleared site.
    pub fn due_from(&self, server: SocketAddr, from: i64, now: Instant) -> bool {
        self.with(|m| {
            m.iter().any(|((s, _), c)| {
                *s == server
                    && c.from == from
                    && c.targets
                        .iter()
                        .any(|(_, at)| now.saturating_duration_since(*at) < CLEARED_TTL)
            })
        })
    }

    /// The J-space systems entered from `from` with a cleared site.
    pub fn systems_from(&self, server: SocketAddr, from: i64, now: Instant) -> Vec<i64> {
        self.with(|m| {
            m.iter()
                .filter(|((s, _), c)| {
                    *s == server
                        && c.from == from
                        && c.targets
                            .iter()
                            .any(|(_, at)| now.saturating_duration_since(*at) < CLEARED_TTL)
                })
                .map(|((_, sys), _)| *sys)
                .collect()
        })
    }

    /// A hacker worked `target`: it is not cleared for a second one.
    pub fn retire(&self, server: SocketAddr, system: i64, target: &str) {
        self.with(|m| {
            if let Some(c) = m.get_mut(&(server, system)) {
                c.targets.retain(|(t, _)| t != target);
            }
        });
    }
}

impl Default for Board {
    fn default() -> Board {
        Board::new()
    }
}

/// The board every raider and hacker of this process shares.
pub static BOARD: Board = Board::new();

// ---- what the raider remembers ------------------------------------------------

/// Which half of a dive the raider is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// The combat anomalies, by the Agency.
    #[default]
    Combat,
    /// The relic and data sites' guards, by the scanner.
    Sweep,
}

/// [`Memory::raid`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// Where the dive came in, for the way out.
    pub entered: Option<wh_dive::Entered>,
    /// The J-space system landed in.
    pub system: Option<i64>,
    /// When the bot first looked around in J-space.
    pub inside_since: Option<Instant>,
    pub phase: Phase,
    /// The signature being swept, while its grid is fought.
    pub sweeping: Option<String>,
    /// Signatures swept (or tried) this dive.
    pub swept: Vec<String>,
    /// Swept signatures whose grid went quiet: posted on the way out.
    pub cleared: Vec<String>,
    /// When the bot last set out for a site (an anomaly or a sweep), while
    /// its grid has not been given [`GRID_PATIENCE`] yet.
    pub landed: Option<Instant>,
    /// A Sleeper was on that grid.
    pub fought: bool,
    /// Nothing left to do inside: leave.
    pub done: bool,
    /// No dive before this.
    pub retry_after: Option<Instant>,
    /// The probe launcher was seen to this dock.
    pub outfitted: bool,
    /// The last dive's line for the act log.
    pub note: Option<String>,
}

impl State {
    /// A fresh dive.
    fn begin(&mut self, e: wh_dive::Entered, system: i64) {
        *self = State {
            entered: Some(e),
            system: Some(system),
            retry_after: self.retry_after,
            ..State::default()
        };
    }

    /// The dive is over, out in k-space.
    fn finish(&mut self, now: Instant) {
        let retry = Some(now + DIVE_REST);
        *self = State {
            retry_after: retry,
            ..State::default()
        };
    }
}

/// One step of a dive, as an [`Act`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Docked: fit the probe launcher and load the probes ([`probe::outfit`]).
    Outfit,
    /// Scan a wormhole into a class the hull runs, and jump.
    In,
    /// Scan down a relic or data signature and warp to it.
    Sweep,
    /// Scan down the way out and take it.
    Out,
}

impl Op {
    pub fn describe(self) -> &'static str {
        match self {
            Op::Outfit => "raid: fit the probe launcher, load the probes",
            Op::In => "raid: scan a wormhole into a class this hull runs, jump",
            Op::Sweep => "raid: scan down a relic or data site, warp to its guards",
            Op::Out => "raid: scan down the way out, jump",
        }
    }
}

// ---- the role ---------------------------------------------------------------

/// Runs the anomalies of k-space, and J-space's when the hull can.
pub static ROLE: RoleSpec = RoleSpec {
    name: "raid",
    greeting: "o7 -- clearing Sleepers and salvaging",
    civilian: false,
    docked,
    in_space,
    banner,
    setup,
};

/// Docked: the site runner's rules, and, before its undock, the probe
/// launcher fitted once a dock when the hull may dive.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let mut acts = (site_runner::ROLE.docked)(m, o);
    if m.raid.outfitted || !capable(cap_of(m, o)) {
        return acts;
    }
    if let Some(at) = acts.iter().position(|a| matches!(a, Act::Undock)) {
        m.raid.outfitted = true;
        acts.insert(at, Act::Raid(Op::Outfit));
    }
    acts
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, kills {}, reconnects {}",
        s.name, s.phase, s.kills, s.reconnects
    )
}

fn setup(lacks: Lacks) -> Vec<String> {
    (site_runner::ROLE.setup)(lacks)
}

/// The difficulty cap the raider plays at: the hull's
/// ([`site_runner::difficulty_cap`]), or the policy's floor when that is
/// higher ([`brain::Policy::raid_cap`], a test harness's).
pub fn cap(floor: Option<i64>, hull: Option<i64>) -> Option<i64> {
    match (floor, hull) {
        (Some(f), Some(h)) => Some(f.max(h)),
        (f, h) => f.or(h),
    }
}

/// [`cap`] of the ship the observation holds.
pub fn cap_of(m: &Memory, o: &Observation) -> Option<i64> {
    cap(
        m.policy.raid_cap,
        o.ship
            .and_then(|s| s.type_id)
            .and_then(site_runner::difficulty_cap),
    )
}

/// **May the raider dive now?** In k-space, armed, with a salvager fitted,
/// not on its way home, a hull that may go, and rested.
pub fn want_dive(m: &Memory, o: &Observation) -> bool {
    !o.j_space
        && o.park.is_some()
        && m.homing.is_none()
        && !o.hold_full
        && !o.guns.is_empty()
        && o.salvager.is_some()
        && capable(cap_of(m, o))
        && o.failed(ActKind::Raid) < GIVE_UP_AFTER
        && m.raid.retry_after.is_none_or(|t| o.now >= t)
}

/// Whether the bot sweeps guarded sites at all (the hull's cap).
pub fn may_sweep(m: &Memory, o: &Observation) -> bool {
    cap_of(m, o).is_some_and(|c| c >= SWEEP_MIN_CAP)
}

/// Rule 5: what to do in space.
fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(_) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    m.raid.outfitted = false;
    if !o.j_space {
        if m.raid.entered.is_some() || m.raid.inside_since.is_some() {
            // Out (or carried out by a death and a respawn): the dive is over.
            m.raid.finish(o.now);
        }
        if o.failed(ActKind::Raid) > 0 && m.raid.retry_after.is_none_or(|t| o.now >= t) {
            m.raid.retry_after = Some(o.now + DIVE_RETRY);
        }
        if want_dive(m, o) {
            return vec![Act::Raid(Op::In)];
        }
        return (site_runner::ROLE.in_space)(m, o);
    }
    inside(m, o)
}

/// **In J-space** (module header).
fn inside(m: &mut Memory, o: &Observation) -> Vec<Act> {
    let Some(park) = &o.park else {
        return vec![Act::Wait(brain::SHORT_WAIT)];
    };
    let started = *m.raid.inside_since.get_or_insert(o.now);
    // Stand and fight: a fight in J-space cannot be left (it takes a scan
    // and a warp), so the retreat reasons wait for a quiet grid.
    let fighting = !park.hostiles.is_empty();
    if fighting {
        m.homing = None;
        m.raid.fought = true;
    }
    let acts = (site_runner::ROLE.in_space)(m, o);
    if fighting {
        m.homing = None;
        return acts;
    }
    let late = o.now.saturating_duration_since(started) >= RAID_CEILING;
    if m.homing.is_some() || m.raid.done || late {
        return leave(m, o, acts);
    }
    if !acts.iter().any(|a| matches!(a, Act::Sites)) {
        return acts;
    }
    // The site runner calls the grid clear and asks for the next site. A
    // grid the bot has only just set out for is not clear until its guards
    // have had [`GRID_PATIENCE`] to come ([`waiting_for_guards`]).
    let mut out: Vec<Act> = acts
        .into_iter()
        .filter(|a| matches!(a, Act::Say(_)))
        .collect();
    let sweeping = m.raid.phase == Phase::Sweep && m.raid.sweeping.is_some();
    let combat_went_through =
        m.raid.phase == Phase::Combat && m.raid.landed.is_some() && o.failed(ActKind::Sites) == 0;
    if (sweeping || combat_went_through) && waiting_for_guards(m, o.now) {
        out.push(Act::Wait(brain::SHORT_WAIT));
        return out;
    }
    match m.raid.phase {
        Phase::Combat if o.failed(ActKind::Sites) == 0 => {
            m.raid.landed = Some(o.now);
            m.raid.fought = false;
            out.push(Act::Sites);
        }
        Phase::Combat => {
            // No combat anomaly left (or none this hull fits).
            m.raid.phase = Phase::Sweep;
            if may_sweep(m, o) {
                m.raid.landed = Some(o.now);
                m.raid.fought = false;
                out.push(Act::Raid(Op::Sweep));
            } else {
                m.raid.done = true;
                out.push(Act::Raid(Op::Out));
            }
        }
        Phase::Sweep => {
            // A sweep's grid went quiet: its guards are dead.
            if let Some(t) = m.raid.sweeping.take() {
                m.raid.cleared.push(t);
            }
            if m.raid.swept.len() >= MAX_SWEEPS || o.failed(ActKind::Raid) > 0 {
                m.raid.done = true;
                out.push(Act::Raid(Op::Out));
            } else {
                m.raid.landed = Some(o.now);
                m.raid.fought = false;
                out.push(Act::Raid(Op::Sweep));
            }
        }
    }
    out
}

/// Whether the grid the bot set out for is still inside its
/// [`GRID_PATIENCE`] with no guard seen.
fn waiting_for_guards(m: &Memory, now: Instant) -> bool {
    !m.raid.fought
        && m.raid
            .landed
            .is_some_and(|t| now.saturating_duration_since(t) < GRID_PATIENCE)
}

/// Out: the way out is scanned for, or, after [`GIVE_UP_AFTER`] failures to
/// find one, the bot says it lacks one and goes on with its fight.
fn leave(m: &mut Memory, o: &Observation, fight: Vec<Act>) -> Vec<Act> {
    m.raid.done = true;
    if m.homing.is_none() {
        m.homing = Some(Homing::HoldFull);
    }
    if o.failed(ActKind::Raid) >= GIVE_UP_AFTER {
        let mut v: Vec<Act> = fight
            .into_iter()
            .filter(|a| matches!(a, Act::Say(_)))
            .collect();
        v.push(Act::Missing("a way out of J-space"));
        v.push(Act::Wait(brain::MISSING_WAIT));
        return v;
    }
    vec![Act::Raid(Op::Out)]
}

// ---- on the wire ------------------------------------------------------------

fn server_of(c: &mut Conn) -> Result<SocketAddr> {
    Ok(c.socket()?.peer_addr()?)
}

/// **One raid step** over the wire (the driver's `Act::Raid` arm).
pub fn act(c: &mut Conn, m: &mut Memory, op: Op) -> Result<()> {
    match op {
        // No launcher, no free slot or no probes: the refusal is the log's
        // word for it, and the dive that follows is not tried.
        Op::Outfit => probe::outfit(c).map(|_| ()),
        Op::In => dive_in(c, m),
        Op::Sweep => sweep(c, m),
        Op::Out => come_out(c, m),
    }
}

fn dive_in(c: &mut Conn, m: &mut Memory) -> Result<()> {
    let hull = c
        .world_now()
        .ego_ball()
        .and_then(|b| b.type_id())
        .and_then(site_runner::difficulty_cap);
    let cap = cap(m.policy.raid_cap, hull)
        .ok_or_else(|| Error::NotSent("raid: a hull with no difficulty cap".to_owned()))?;
    let max = max_class(cap)
        .ok_or_else(|| Error::NotSent("raid: no class this hull can run".to_owned()))?;
    await_signatures(c);
    let Some(e) = wh_dive::dive_in_where(c, Leads::JSpaceTo(max), &|_, _| true)? else {
        return Err(Error::Refused(format!(
            "raid: no hole into a class up to C{max}"
        )));
    };
    let system = c.session().solar_system_id().unwrap_or_default();
    m.raid.begin(e, system);
    Ok(())
}

/// Pump until the system's scan overlay holds something, or
/// [`SIGNATURE_PATIENCE`] has passed. Best effort: a bot with no scanner,
/// or no overlay, goes on to find out for itself.
fn await_signatures(c: &mut Conn) {
    let Ok(manager) = c.system_scan_manager() else {
        return;
    };
    let deadline = Instant::now() + SIGNATURE_PATIENCE;
    while Instant::now() < deadline {
        match c.sensor_overlay(&manager) {
            Ok(o) if !o.is_empty() => return,
            Ok(_) => {}
            Err(_) => return,
        }
        if c.pump(Duration::from_millis(500)).is_err() {
            return;
        }
    }
}

fn sweep(c: &mut Conn, m: &mut Memory) -> Result<()> {
    let mut scanner = match probe::Scanner::open(c) {
        Ok(s) => s,
        Err(Error::NotSent(_)) => {
            return Err(Error::Refused("raid: no probes to sweep with".to_owned()));
        }
        Err(e) => return Err(e),
    };
    let swept = m.raid.swept.clone();
    let sig = probe::find_with(c, &mut scanner, |t, k| {
        !swept.iter().any(|x| x == t) && roam::job_for(k, true) == roam::Job::Work
    })?;
    let Some(sig) = sig else {
        return Err(Error::Refused(
            "raid: no relic or data signature left".to_owned(),
        ));
    };
    m.raid.swept.push(sig.target_id.clone());
    probe::warp_to(c, &sig)?;
    m.raid.sweeping = Some(sig.target_id);
    Ok(())
}

fn come_out(c: &mut Conn, m: &mut Memory) -> Result<()> {
    let arrival = m.raid.entered.map(|e| e.arrival);
    // The cleared sites are posted on the way out, with the hole's home: a
    // hacker leaving from the same system finds them.
    let system = m.raid.system.or_else(|| c.session().solar_system_id());
    let from = m.raid.entered.map(|e| e.from);
    let cleared = std::mem::take(&mut m.raid.cleared);
    if !wh_dive::come_out(c, arrival)? {
        m.raid.cleared = cleared;
        return Err(Error::Refused(
            "raid: no hole out of J-space was found".to_owned(),
        ));
    }
    let server = server_of(c)?;
    let now = Instant::now();
    if let (Some(system), Some(from)) = (system, from) {
        for t in &cleared {
            BOARD.post(server, system, from, t, now);
        }
    }
    let salvage: i64 = c
        .cargo()
        .map(|rows| {
            rows.iter()
                .filter(|r| r.group_id == GROUP_ANCIENT_SALVAGE)
                .map(|r| r.quantity)
                .sum()
        })
        .unwrap_or(0);
    m.raid.note = Some(format!(
        "[raid] out of {system:?}: {} guard sites cleared, {salvage} Ancient Salvage in the hold",
        cleared.len()
    ));
    m.raid.finish(now);
    if m.homing.is_none() {
        m.homing = Some(Homing::HoldFull);
    }
    Ok(())
}

// ---- the hacker's half ------------------------------------------------------

/// Whether the hacker docked here should dive: a raider cleared a site
/// behind a hole from this system ([`Board::due_from`]).
pub fn hack_dive_due(c: &mut Conn) -> bool {
    let (Some(system), Ok(server)) = (c.session().solar_system_id2(), server_of(c)) else {
        return false;
    };
    BOARD.due_from(server, system, Instant::now())
}

/// **One hacking dive** ([`crate::hacker::run_dive`]): in through a hole
/// whose far system holds a cleared site, only the cleared sites hacked,
/// out again.
pub fn hack_dive(c: &mut Conn, name: &str, seed: u64) -> Result<crate::hacker::Trip> {
    let server = server_of(c)?;
    let home = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("hack dive: docked with no system".to_owned()))?;
    let now = Instant::now();
    let want = BOARD.systems_from(server, home, now);
    let enter = move |c: &mut Conn| -> Result<Option<[f64; 3]>> {
        let e = wh_dive::dive_in_where(c, Leads::JSpace, &|landed, _| want.contains(&landed))?;
        Ok(e.map(|e| e.arrival))
    };
    let exit = |c: &mut Conn, arrival: Option<[f64; 3]>| wh_dive::come_out(c, arrival);
    let allowed =
        move |system: i64, target: &str| BOARD.is_cleared(server, system, target, Instant::now());
    let dive = crate::hacker::Dive {
        enter: &enter,
        exit: &exit,
        allowed: &allowed,
    };
    let trip = crate::hacker::run_dive(c, name, seed, &dive);
    // A worked site is not cleared for a second hacker.
    if let Ok(t) = &trip {
        for target in &t.sites {
            for sys in BOARD.systems_from(server, home, Instant::now()) {
                BOARD.retire(server, sys, target);
            }
        }
    }
    trip
}

// ---- the model: how long to a first Tengu kit --------------------------------

/// **The pure model** of Ancient Salvage from Sleeper wrecks (module header,
/// `docs/lanes/t3-salvage.md` "Honest measurement"). Every field is **[A]**
/// except where noted; the defaults are the lane note's.
pub mod model {
    /// What a dive costs and yields.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Params {
        /// Seconds in, out and home (`y-wh.md`'s dive: 420 + 420 + 300).
        pub overhead_s: f64,
        /// Seconds to scan a way in and a way out.
        pub scan_s: f64,
        /// Seconds from one site to the next (warp and settle).
        pub site_gap_s: f64,
        /// Seconds to kill one Sleeper ship.
        pub kill_s: f64,
        /// Sleeper ships (wrecks) in one combat anomaly (`y-wh.md`: ~3.4 +
        /// 6.2 + 7.9 by hull class, a Perimeter-to-Core mix).
        pub wrecks_per_anomaly: f64,
        /// Seconds to approach, loot and lock one wreck.
        pub wreck_overhead_s: f64,
        /// Salvager cycle, seconds.
        pub cycle_s: f64,
        /// A cycle's success chance (Salvager I, Salvaging III: 58 %,
        /// `y-wh.md`).
        pub chance: f64,
        /// Attempts on one wreck before it is let go (`salvage::MAX_TRIES`).
        pub tries: u32,
        /// Mean Ancient Salvage units in a salvaged wreck (`y-wh.md`: 69 an
        /// anomaly at 58 % over ~17.5 wrecks, so ~3.9 per wreck-cycle;
        /// divided by the chance, ~6.8 a salvaged wreck).
        pub units_per_wreck: f64,
        /// The dive's ceiling, seconds ([`super::RAID_CEILING`]).
        pub ceiling_s: f64,
        /// Seconds on a dock a trip: unload, sell, refit.
        pub dock_s: f64,
        /// Anomalies a J-space system holds (population row: 3).
        pub anomalies_per_system: f64,
        /// Mean seconds before a cleared anomaly respawns (the row's
        /// 3,600..259,200 s, uniform).
        pub respawn_s: f64,
        /// Distinct J-space systems the raiders reach (holes are static per
        /// home system: one bot's homes reach few).
        pub systems: f64,
        /// Ancient Salvage types, and the units of each a kit needs
        /// (`t3-supply.md`: 2,357 over 19 types).
        pub types: u32,
        pub need_each: i64,
    }

    impl Default for Params {
        fn default() -> Params {
            Params {
                overhead_s: 420.0 + 420.0 + 300.0,
                scan_s: 600.0,
                site_gap_s: 90.0,
                kill_s: 120.0,
                wrecks_per_anomaly: 17.5,
                wreck_overhead_s: 25.0,
                cycle_s: 5.0,
                chance: 0.58,
                tries: crate::salvage::MAX_TRIES,
                units_per_wreck: 6.8,
                ceiling_s: 2.0 * 3_600.0,
                dock_s: 600.0,
                anomalies_per_system: 3.0,
                respawn_s: (3_600.0 + 259_200.0) / 2.0,
                systems: 6.0,
                types: 19,
                need_each: 124,
            }
        }
    }

    /// Cycles one wreck costs, and the chance it is salvaged at all.
    pub fn wreck_cycles(chance: f64, tries: u32) -> (f64, f64) {
        let miss = 1.0 - chance;
        let ok = 1.0 - miss.powi(tries as i32);
        // Expected cycles of a geometric run cut at `tries`.
        let cycles = if chance <= 0.0 {
            f64::from(tries)
        } else {
            ok / chance
        };
        (cycles, ok)
    }

    /// Seconds one anomaly takes: warp, kills, loot, salvage.
    pub fn anomaly_s(p: &Params) -> f64 {
        let (cycles, _) = wreck_cycles(p.chance, p.tries);
        p.site_gap_s + p.wrecks_per_anomaly * (p.kill_s + p.wreck_overhead_s + cycles * p.cycle_s)
    }

    /// Ancient Salvage one anomaly gives.
    pub fn anomaly_units(p: &Params) -> f64 {
        let (_, ok) = wreck_cycles(p.chance, p.tries);
        p.wrecks_per_anomaly * ok * p.units_per_wreck
    }

    /// Anomalies one dive clears: what the ceiling allows after the
    /// overhead, and at least one (the ceiling is only looked at between
    /// anomalies, `inside`).
    pub fn anomalies_per_dive(p: &Params) -> f64 {
        let free = p.ceiling_s - p.overhead_s - p.scan_s;
        (free / anomaly_s(p)).floor().max(1.0)
    }

    /// Ancient Salvage one raider brings a game day when anomalies are
    /// plenty: dives a day times a dive's yield.
    pub fn per_bot_day(p: &Params) -> f64 {
        let k = anomalies_per_dive(p);
        let dive_s = p.overhead_s + p.scan_s + k * anomaly_s(p) + p.dock_s;
        86_400.0 / dive_s * k * anomaly_units(p)
    }

    /// Anomalies a day the J-space systems the raiders reach can give:
    /// each system's anomalies come back one respawn after being cleared.
    pub fn supply_anomalies_per_day(p: &Params) -> f64 {
        p.systems * p.anomalies_per_system * 86_400.0 / p.respawn_s
    }

    /// Ancient Salvage a day from `n` raiders: the time limit, or the
    /// supply of anomalies, whichever binds.
    pub fn units_per_day(p: &Params, n: u32) -> f64 {
        let by_time = per_bot_day(p) * f64::from(n);
        let by_supply = supply_anomalies_per_day(p) * anomaly_units(p);
        by_time.min(by_supply)
    }

    /// A tiny deterministic generator (SplitMix64): the model needs no
    /// dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
    }

    /// **The binding-type factor**: units drawn (types uniform, quantity
    /// 1..=3 as `wormhole_sites::SALVAGE_QTY`) until every one of `types`
    /// has `need_each`, over `types * need_each`. The mean says done long
    /// before the scarcest type does; this is by how much.
    pub fn binding_factor(types: u32, need_each: i64, trials: u32) -> f64 {
        let mut rng = Rng(0x0773_35a1);
        let mut total = 0u64;
        for _ in 0..trials {
            let mut have = vec![0i64; types as usize];
            let mut drawn = 0u64;
            while have.iter().any(|h| *h < need_each) {
                let t = (rng.next() % u64::from(types)) as usize;
                let q = (rng.next() % 3 + 1) as i64;
                have[t] += q;
                drawn += q as u64;
            }
            total += drawn;
        }
        total as f64 / f64::from(trials) / (f64::from(types) * need_each as f64)
    }

    /// **Game days to the first kit's Ancient Salvage** with `n` raiders.
    pub fn days_to_kit(p: &Params, n: u32) -> f64 {
        let need =
            f64::from(p.types) * p.need_each as f64 * binding_factor(p.types, p.need_each, 200);
        let per_day = units_per_day(p, n);
        if per_day <= 0.0 {
            return f64::INFINITY;
        }
        need / per_day
    }
}

#[cfg(test)]
mod tests;
