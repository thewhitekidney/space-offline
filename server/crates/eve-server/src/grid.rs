//! **The grid: which balls belong in a pilot's park** (GRID-B, phase 1 of
//! `docs/lanes/grid-research.md`).
//!
//! A pilot undocked in Tanoo and the overview listed belt rocks 1.2 AU away
//! (`docs/port-tracker.md` §74.2). There was no grid: every ball in the
//! system went to every pilot in it, in the state and in every `AddBalls2`
//! after it. And the client does no filtering of its own -- the overview
//! loops over `ballpark.balls` with a preset filter and **no distance test**
//! (`overviewWindow.pyc:1270-1272`), the brackets the same
//! (`bracketMgr.pyc:339-392`) **[V]**. Off grid means not in the park.
//!
//! # The rule (research §4)
//!
//! - **Global** balls -- the star, planets, moons, belts (the belt's *ball*,
//!   not its rocks), stargates, stations and customs offices -- are in every
//!   park in the system, always. They are what [`destiny::scene`] builds
//!   that [`local_balls`] does not list.
//! - **Local** balls -- ships, capsules, NPCs, drones, rocks, wrecks and site
//!   objects -- are in a park while their centre is within [`GRID_ENTER`] of
//!   the viewer's own ship, and stay until it passes [`GRID_LEAVE`].
//! - The viewer's own ship is never taken out of its own park.
//!
//! # The mechanism (research §3.1)
//!
//! 1. **[`Parks`] models each client's park**: the ball ids the server has
//!    sent it and not taken back. It is written in one place, [`filter`],
//!    which `Registry::take_pending` and `Registry::take_due` run every push
//!    through as it leaves -- so the model follows the same order as the
//!    wire.
//! 2. **Every Destiny push says what it does to a park**
//!    ([`crate::notify::Push::grid`], a [`GridRole`]), and the filter keeps a
//!    viewer only where that makes sense: a push naming a ball goes to the
//!    clients holding it, an `AddBalls2` to the clients whose grid the ball
//!    is on. A batch about several balls is cut per viewer
//!    ([`crate::notify::Push::narrowed`]).
//! 3. **[`tick`] reconciles on the clock**: every [`RECONCILE_EVERY`] ticks,
//!    each modelled pilot is sent `RemoveBalls` for what left their grid and
//!    `AddBalls2` for what entered it. That one stage handles a warp's
//!    departure and arrival, a ship flying past, a `/tp`, and anything a
//!    spawn path missed.
//!
//! **An unmodelled pilot is unfiltered.** The model begins at a pilot's
//! `SetState` and ends at dock or jump ([`forget`]); in between, a client
//! whose state was never drained -- most of the test suite, which builds a
//! roster by hand -- gets what it always got. That is the safe direction:
//! the failure is a ball too many, which is the behaviour before this
//! module, never a ball the client needed and was not sent.
//!
//! One exception: a pilot in `Leg::Arriving` (session change sent, bind not
//! yet) is skipped whole, so no stale push becomes the new park's first entry.
//!
//! # What this does not change
//!
//! The ball flags. Every ball still carries `isGlobal` ([`FLAG_IS_GLOBAL`]);
//! clearing it on local balls is phase 2 and waits on a client run, because
//! the native park's bubble paths are untested on this client (research R2).
//! Nor d-scan or warp-to-member, which read the roster, not the park
//! (research §0.4).

use crate::damage::BlueTime;
use crate::notify::{Addressed, Push};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// `GRIDSIZE` (`eve/common/lib/appConst.pyc:69`) **[V]**: the edge of CCP's
/// grid box. **[I]** in metres -- its one consumer compares it, times 1000,
/// with a surface distance in metres (`spaceObject.py:519`), which read as
/// metres is about 52 AU and read as kilometres is absurd; and 7,864,320 is
/// 7.5 x 2^20, the shape of a box partition's size (research §1.4).
pub const GRIDSIZE: f64 = 7_864_320.0;

/// **A local ball enters a park inside this many metres of the viewer's ship**,
/// centre to centre. **[A]** `GRIDSIZE / 2`: the sphere inscribed in CCP's
/// box. Over every system in the extract it merges no two belts, and puts no
/// belt's rocks on a station's or a gate's grid (research §1.4 table).
/// Falsified by a play test where the grid feels wrong; 250 km is the
/// fallback and also passes the data (research R1).
pub const GRID_ENTER: f64 = GRIDSIZE / 2.0;

/// **A held local ball leaves a park past this many metres.** **[A]** 1.1 x
/// [`GRID_ENTER`]: the hysteresis that stops a ball at the edge flickering
/// in and out of the overview once a second.
pub const GRID_LEAVE: f64 = GRID_ENTER * 1.1;

/// **The reconcile runs on one tick in this many** of `push::TICK` (250 ms),
/// i.e. once a second. **[A]** A warp crosses [`GRID_LEAVE`] about one to two
/// seconds after the engine starts (research §5), so a second is the
/// coarsest step that still empties the old grid before the tunnel ends.
/// Falsified by a soak whose `grid` stage dominates the tick, or by a pilot
/// who lands before their destination's balls appear.
pub const RECONCILE_EVERY: u32 = 4;

/// `isGlobal` in the ball record's flag byte (offset `0x2a`): the client's
/// own writer reads `0x1d` for `FLAGS_FREE_BALL` with this bit cleared,
/// whether by `AddBall`'s sixth argument or by the `isGlobal` attribute
/// (`findings/destiny-ref/11-arg-a6_i.bin`, `28-attr-isGlobal.bin`) **[V]**.
/// Named for phase 2, which clears it on local balls; nothing sets it apart
/// yet.
pub const FLAG_IS_GLOBAL: u8 = 0x02;

/// **What one push does to a client's park** -- the declaration the drain
/// filter acts on ([`crate::notify::Push::grid`]).
#[derive(Debug, Clone, PartialEq)]
pub enum GridRole {
    /// Not a park event: the audience is left alone.
    None,
    /// An entry about these balls -- an action, an effect, a damage or slim
    /// change. Goes only to clients that hold them; a client holding some
    /// gets a batch cut down to those.
    Names(Vec<i64>),
    /// `AddBalls2` of these balls, where they are. Goes to clients whose grid
    /// they are on and who do not hold them already, cut per client.
    Adds(Vec<(i64, [f64; 3])>),
    /// `RemoveBalls` of these. Goes to clients that hold any of them.
    Removes(Vec<i64>),
    /// `CloakBall` of these (CLOAK-1). Goes to clients that hold any of them,
    /// like [`GridRole::Removes`], and takes them out of the park -- except a
    /// viewer's **own** ship, which the client keeps (michelle removes a
    /// cloaked ball only when it is not the ego).
    Hides(Vec<i64>),
    /// `SetState`: the client's park becomes exactly these.
    Resets(Vec<i64>),
    /// One batch that takes `gone` out and puts `adds` in where it was: a
    /// kill ([`crate::destiny::Kill`]). Goes, whole, to clients that hold
    /// `gone`.
    Swaps { gone: i64, adds: Vec<i64> },
    /// One batch that puts `adds` into parks that already hold every ball
    /// of `needs`: a missile's add-and-launch
    /// ([`crate::destiny::MissileLaunched`]), whose `LaunchMissile` names
    /// the launcher and the target. Goes, whole, only to clients holding
    /// all of `needs` -- never cut, because the add without its launch is
    /// a ball parked in space, and the launch without its add names no
    /// ball. On send, `adds` joins those parks, so the removal that ends
    /// it routes through [`GridRole::Removes`] to exactly them.
    ///
    /// The spawned balls are never in [`local_balls`], so the reconcile
    /// neither adds nor removes them: `[A]` a pilot who arrives mid-flight
    /// sees none of the missiles already flying (R-MISSILE §2.3). Falsified
    /// by a play test where that reads as a bug rather than a detail.
    Spawns { needs: Vec<i64>, adds: Vec<i64> },
}

/// **The server's model of every client's park**: what each character has
/// been sent and not been told to remove. A world slot ([`SLOT`]).
#[derive(Debug, Default)]
pub struct Parks {
    held: BTreeMap<i64, BTreeSet<i64>>,
    /// World-clock ticks seen, for [`RECONCILE_EVERY`].
    ticks: u32,
    /// Ships the last reconcile saw in warp (BOT-REDOCK): the ones whose
    /// landing is re-sent to the parks that held them.
    warping: BTreeSet<i64>,
}

impl Parks {
    /// What this character's park holds, or `None` when it is not modelled
    /// (see the module header: unmodelled is unfiltered).
    pub fn of(&self, character_id: i64) -> Option<&BTreeSet<i64>> {
        self.held.get(&character_id)
    }

    /// Every modelled character.
    pub fn viewers(&self) -> impl Iterator<Item = i64> + '_ {
        self.held.keys().copied()
    }
}

/// The world slot. **Coordinator-only** registration (`world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec { name: "grid", make };

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(Parks::default())
}

/// Stop modelling a character's park: they docked, or went through a gate.
/// The client tears its park down itself (`Park.Release`, `michelle:1196`)
/// **[V]**, and the next one starts at a `SetState`.
pub fn forget(w: &mut Registry, character_id: i64) {
    w.ext_mut::<Parks>().held.remove(&character_id);
}

/// Whether a local ball at `ball` belongs on the grid of a ship at `ego`:
/// inside [`GRID_ENTER`], or inside [`GRID_LEAVE`] when it is already held.
pub fn on_grid(ego: [f64; 3], ball: [f64; 3], held: bool) -> bool {
    let d = crate::space_state::distance(ego, ball);
    d <= GRID_ENTER || (held && d <= GRID_LEAVE)
}

/// **Every local ball in a system, with where it is**: the ones the grid
/// rule applies to. Ships, capsules, NPCs and drones from the roster; rocks;
/// wrecks; site objects. Positions are as the roster last integrated them --
/// call `Registry::advance_system` first for "now".
///
/// Whatever [`crate::destiny::scene`] builds that is not in here is global.
pub fn local_balls(w: &Registry, system: i64) -> Vec<(i64, [f64; 3])> {
    let mut out: Vec<(i64, [f64; 3])> = w
        .roster()
        .in_system(system)
        .iter()
        .map(|p| (p.ship_id, p.motion.position))
        .collect();
    out.extend(w.rocks().in_system(system).map(|r| (r.id, r.position)));
    out.extend(
        crate::combat::wrecks_in(w, system)
            .iter()
            .map(|(b, _)| (b.ball_id, b.position)),
    );
    out.extend(
        crate::site::balls_in(w, system)
            .iter()
            .map(|(b, _)| (b.ball_id, b.position)),
    );
    // CAP-D: a lit cynosural field is a local ball.
    out.extend(
        crate::cyno::balls_in(w, system)
            .iter()
            .map(|(b, _)| (b.ball_id, b.position)),
    );
    // ESS-2: the ESS is a local ball (its beacon is global).
    out.extend(
        crate::ess::balls_in(w, system)
            .iter()
            .map(|(b, _)| (b.ball_id, b.position)),
    );
    // BUBBLES-SB: a warp disrupt probe is a local ball.
    out.extend(
        crate::bubble::balls_in(w, system)
            .iter()
            .map(|(b, _)| (b.ball_id, b.position)),
    );
    out
}

/// **The local balls a new state for `ship` at `ego` should hold**: every
/// local ball within [`GRID_ENTER`], and the ship. `SetState` uses the enter
/// radius (research §4).
///
/// The caller keeps every ball *not* in [`local_balls`] -- the global
/// furniture -- and every id this returns.
pub fn grid_of(w: &Registry, system: i64, ship: i64, ego: [f64; 3]) -> GridSet {
    let local = local_balls(w, system);
    GridSet {
        keep: local
            .iter()
            .filter(|(id, at)| {
                *id == ship
                    || (on_grid(ego, *at, false) && !crate::cloak::hidden_from(w, *id, ship))
            })
            .map(|(id, _)| *id)
            .collect(),
        local: local.into_iter().map(|(id, _)| id).collect(),
    }
}

/// What [`grid_of`] answers: which ids are local, and which of those stay.
pub struct GridSet {
    local: BTreeSet<i64>,
    keep: BTreeSet<i64>,
}

impl GridSet {
    /// Whether a ball of the scene stays: global, or local and kept.
    pub fn contains(&self, id: &i64) -> bool {
        !self.local.contains(id) || self.keep.contains(id)
    }
}

/// How one viewer takes one push.
enum Cut {
    /// The whole push, as it is.
    All,
    /// Nothing.
    Nothing,
    /// The push cut down to these balls.
    Only(Vec<i64>),
}

/// Which ids of `ids` `keep` admits: counted first, so a viewer who takes all
/// or nothing builds no `Vec` (PA-7).
fn cut(ids: &[i64], keep: impl Fn(i64) -> bool) -> Cut {
    let n = ids.iter().filter(|id| keep(**id)).count();
    if n == 0 {
        Cut::Nothing
    } else if n == ids.len() {
        Cut::All
    } else {
        Cut::Only(ids.iter().copied().filter(|id| keep(*id)).collect())
    }
}

/// What one viewer does with one push.
enum Decision {
    /// Takes it whole.
    Whole,
    /// Takes none of it.
    Skip,
    /// Takes this cut copy of its own.
    Part(Box<dyn Push>),
}

/// The role and audience of a push the filter acts on; `None` when it passes
/// through untouched (no role, or no audience).
fn plan(push: &dyn Push) -> Option<(GridRole, Vec<i64>)> {
    let role = push.grid();
    if role == GridRole::None {
        // The forcing function for a new Destiny push (`Push::grid`): one
        // that forgot its role would reach every pilot in the system.
        debug_assert!(
            push.to_push().0 != "DoDestinyUpdate",
            "a DoDestinyUpdate push with no grid role: {push:?}"
        );
        return None;
    }
    let audience = push.audience()?;
    Some((role, audience))
}

/// The ids an `Adds` push places, once per push (empty for any other role).
fn placed_ids(role: &GridRole) -> Vec<i64> {
    match role {
        GridRole::Adds(placed) => placed.iter().map(|(id, _)| *id).collect(),
        _ => Vec::new(),
    }
}

/// **The rule, for one viewer and one push**: what the viewer takes and what
/// its park then gains or loses. `held` is the viewer's park as left by the
/// pushes before this one (`None`: unmodelled), `placed` is
/// [`placed_ids`]. The one copy of the rule, called by [`filter`] and by the
/// per-viewer groups of [`filter_all`].
fn decide(
    r: &Registry,
    role: &GridRole,
    push: &dyn Push,
    placed: &[i64],
    held: Option<&BTreeSet<i64>>,
    v: i64,
) -> (Decision, Option<Write>) {
    // A state is where a model begins, so it is taken before the
    // unmodelled test below.
    if let GridRole::Resets(ids) = role {
        return (Decision::Whole, Some(Write::reset(v, ids.clone())));
    }
    // A pilot whose session change has gone and whose bind has not come
    // (`Leg::Arriving`) takes no push: the new park's first entry must be its
    // SetState, and a stale one ahead of it makes the client ignore the state
    // for good. Unmodelled is not enough to let it through here.
    if !r.jumps().is_empty() && crate::jump::awaiting_bind(r, v) {
        return (Decision::Skip, None);
    }
    let Some(held) = held else {
        return (Decision::Whole, None);
    };
    let write = |removed: Vec<i64>, added: Vec<i64>| Write {
        viewer: v,
        removed,
        added,
        reset: None,
    };
    let (cut, ids, adds): (Cut, &[i64], bool) = match role {
        GridRole::None | GridRole::Resets(_) => unreachable!("handled above"),
        GridRole::Swaps { gone, adds } => {
            return if held.contains(gone) {
                (Decision::Whole, Some(write(vec![*gone], adds.clone())))
            } else {
                (Decision::Skip, None)
            };
        }
        GridRole::Spawns { needs, adds } => {
            return if needs.iter().all(|b| held.contains(b)) {
                (Decision::Whole, Some(write(Vec::new(), adds.clone())))
            } else {
                (Decision::Skip, None)
            };
        }
        GridRole::Names(ids) | GridRole::Removes(ids) | GridRole::Hides(ids) => {
            (cut(ids, |b| held.contains(&b)), ids, false)
        }
        GridRole::Adds(placed_at) => {
            // Inside the enter radius of the viewer's ship as the roster
            // last placed it -- at most a reconcile old -- and not held
            // already: the reconcile and a spawn path may both add a
            // ball in one tick, and the client must not get it twice.
            let me = r.roster().get(v).map(|p| (p.ship_id, p.motion.position));
            // A cloaked ball is no one's but its pilot's (CLOAK-1).
            let on = |id: i64, at: [f64; 3]| {
                me.is_none_or(|(ship, ego)| {
                    id == ship
                        || (on_grid(ego, at, false) && !crate::cloak::hidden_from(r, id, ship))
                })
            };
            // A set, not a `Vec`: a warm belt's batch is thousands of
            // balls, and `cut` asks once per ball (lane perf-fight).
            let wanted: BTreeSet<i64> = placed_at
                .iter()
                .filter(|(id, at)| !held.contains(id) && on(*id, *at))
                .map(|(id, _)| *id)
                .collect();
            (cut(placed, |b| wanted.contains(&b)), placed, true)
        }
    };
    // `Names` writes nothing; `Removes` takes the delivered ids out of the
    // park, `Adds` puts them in.
    let record = |delivered: Vec<i64>| match role {
        GridRole::Removes(_) => Some(write(delivered, Vec::new())),
        GridRole::Hides(_) => {
            let own = r.roster().get(v).map(|p| p.ship_id);
            let gone: Vec<i64> = delivered.into_iter().filter(|b| Some(*b) != own).collect();
            (!gone.is_empty()).then(|| write(gone, Vec::new()))
        }
        _ if adds => Some(write(Vec::new(), delivered)),
        _ => None,
    };
    let writes = matches!(role, GridRole::Removes(_) | GridRole::Hides(_)) || adds;
    match cut {
        Cut::All => (
            Decision::Whole,
            if writes { record(ids.to_vec()) } else { None },
        ),
        Cut::Nothing => (Decision::Skip, None),
        Cut::Only(some) => {
            let keep: BTreeSet<i64> = some.iter().copied().collect();
            match push.narrowed(vec![v], &|b| keep.contains(&b)) {
                Some(p) => (Decision::Part(p), record(some)),
                None => (Decision::Skip, None),
            }
        }
    }
}

/// Change one park by one write: the same code for the serial filter and the
/// groups.
fn apply(held: &mut BTreeMap<i64, BTreeSet<i64>>, write: Write) {
    let park = held.entry(write.viewer).or_default();
    if let Some(ids) = write.reset {
        *park = ids.into_iter().collect();
    }
    for id in write.removed {
        park.remove(&id);
    }
    park.extend(write.added);
}

/// **The drain filter**: one push in, the pushes to send out, with
/// [`Parks`] brought up to date as if they had been sent.
///
/// Run by `Registry::take_pending` and `Registry::take_due` on every push, in
/// order, so the model and the wire agree. A push whose audience is
/// unchanged comes back as itself; one whose audience shrank comes back
/// wrapped in [`Addressed`]; a viewer who takes part of a multi-ball batch
/// gets a cut copy of their own ([`Push::narrowed`]).
pub fn filter(w: &mut Registry, push: Box<dyn Push>) -> Vec<Box<dyn Push>> {
    match plan(push.as_ref()) {
        None => vec![push],
        Some((role, audience)) => filter_planned(w, push, &role, &audience),
    }
}

fn filter_planned(
    w: &mut Registry,
    push: Box<dyn Push>,
    role: &GridRole,
    audience: &[i64],
) -> Vec<Box<dyn Push>> {
    let placed = placed_ids(role);
    let mut whole = Vec::new();
    let mut parts: Vec<Box<dyn Push>> = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    {
        // One slot lookup per push, not one per viewer (PA-7).
        let parks = w.ext::<Parks>();
        let r: &Registry = w;
        for v in audience.iter().copied() {
            let (d, write) = decide(r, role, push.as_ref(), &placed, parks.of(v), v);
            match d {
                Decision::Whole => whole.push(v),
                Decision::Part(p) => parts.push(p),
                Decision::Skip => {}
            }
            writes.extend(write);
        }
    }
    if !writes.is_empty() {
        let held = &mut w.ext_mut::<Parks>().held;
        for write in writes {
            apply(held, write);
        }
    }
    let mut out: Vec<Box<dyn Push>> = Vec::new();
    if whole.len() == audience.len() {
        out.push(push);
    } else if !whole.is_empty() {
        out.push(Box::new(Addressed {
            to: Some(whole),
            inner: push,
        }));
    }
    out.extend(parts);
    out
}

/// Audience entries in one drain from which [`filter_all`] decides per viewer
/// group on the kit. **[A]**: ~45 ns per entry serial after the fast path, so
/// 25k entries is ~1.1 ms against ~0.1 ms of spawns; falsified by
/// `grid_bench` showing the kit slower near it.
const FILTER_PAR_MIN: usize = 25_000;
/// Most viewer groups (and so workers) one drain uses.
const FILTER_GROUPS_MAX: usize = 64;

/// One viewer's departure from "takes the whole push", at audience entry `j`
/// of push `i`. `Whole` is the default and is not recorded.
struct Exception {
    i: u32,
    j: u32,
    d: Cut1,
}

enum Cut1 {
    Skip,
    Part(Box<dyn Push>),
}

/// A push the drain filter acts on, with its role and audience taken once.
struct Item {
    push: Box<dyn Push>,
    /// `None`: passes through untouched.
    plan: Option<(GridRole, Vec<i64>)>,
    placed: Vec<i64>,
}

/// `pushes.flat_map(|p| filter(w, p))`: the same output and the same
/// [`Parks`], with the per-viewer decisions made in parallel when the drain
/// is big (PA-7).
pub fn filter_all(w: &mut Registry, pushes: Vec<Box<dyn Push>>) -> Vec<Box<dyn Push>> {
    filter_all_with(w, pushes, crate::par::threads(), FILTER_PAR_MIN)
}

/// **Why per viewer is exact**: the decision for push `i` and viewer `v`
/// reads the push, `v`'s park as pushes `< i` left it (writes are applied
/// after the whole audience, so a viewer listed twice decides both times
/// against the pre-push state) and, for `Adds`, the roster, which nothing
/// changes during a drain. Its write touches only `v`'s park. So replaying
/// the pushes in order for each viewer on its own, in any partition of the
/// viewers, gives the serial answer. Output is assembled in push order.
pub(crate) fn filter_all_with(
    w: &mut Registry,
    pushes: Vec<Box<dyn Push>>,
    threads: usize,
    min: usize,
) -> Vec<Box<dyn Push>> {
    let items: Vec<Item> = pushes
        .into_iter()
        .map(|push| {
            let plan = plan(push.as_ref());
            let placed = plan.as_ref().map_or_else(Vec::new, |(r, _)| placed_ids(r));
            Item { push, plan, placed }
        })
        .collect();
    let entries: usize = items
        .iter()
        .map(|it| it.plan.as_ref().map_or(0, |(_, a)| a.len()))
        .sum();
    if threads <= 1 || entries < min || entries == 0 {
        // The exact old path.
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            match it.plan {
                None => out.push(it.push),
                Some((role, aud)) => out.extend(filter_planned(w, it.push, &role, &aud)),
            }
        }
        return out;
    }
    let g = threads.min(FILTER_GROUPS_MAX);
    let group = |v: i64| v.rem_euclid(g as i64) as usize;
    // Move the parks out, no clone: one map per group, behind an uncontended
    // `Mutex` only so a worker can hold `&mut` through the kit's `&T`.
    let held = std::mem::take(&mut w.ext_mut::<Parks>().held);
    let mut split: Vec<BTreeMap<i64, BTreeSet<i64>>> = vec![BTreeMap::new(); g];
    for (v, park) in held {
        split[group(v)].insert(v, park);
    }
    let groups: Vec<std::sync::Mutex<BTreeMap<i64, BTreeSet<i64>>>> =
        split.into_iter().map(std::sync::Mutex::new).collect();
    let ids: Vec<usize> = (0..g).collect();
    let decided = {
        let r: &Registry = w;
        crate::par::par_map_with(threads, &ids, 2, |gi| {
            let mut park = groups[*gi].lock().unwrap_or_else(|e| e.into_inner());
            decide_group(r, &items, *gi, g, &mut park)
        })
    };
    // Put the parks back, whatever happened to the workers.
    let mut merged = BTreeMap::new();
    for m in groups {
        merged.append(&mut m.into_inner().unwrap_or_else(|e| e.into_inner()));
    }
    w.ext_mut::<Parks>().held = merged;
    let decided = match decided {
        Ok(d) => d,
        // The model was mutated by the workers that finished: serial
        // fallback would decide twice. Raise it again, as the serial filter
        // does on the clock thread.
        Err(e) => panic!("grid filter worker panicked: {}", e.0),
    };
    let mut ex: Vec<Exception> = decided.into_iter().flatten().collect();
    ex.sort_unstable_by_key(|e| (e.i, e.j));
    let mut ex = ex.into_iter().peekable();
    let mut out: Vec<Box<dyn Push>> = Vec::with_capacity(items.len());
    for (i, it) in items.into_iter().enumerate() {
        let Some((_, aud)) = it.plan else {
            out.push(it.push);
            continue;
        };
        let mut skipped: Vec<u32> = Vec::new();
        let mut parts: Vec<Box<dyn Push>> = Vec::new();
        while let Some(e) = ex.next_if(|e| e.i as usize == i) {
            skipped.push(e.j);
            if let Cut1::Part(p) = e.d {
                parts.push(p);
            }
        }
        if skipped.is_empty() {
            out.push(it.push);
            continue;
        }
        let whole: Vec<i64> = {
            let mut s = skipped.iter().peekable();
            aud.iter()
                .enumerate()
                .filter(|(j, _)| s.next_if(|k| **k as usize == *j).is_none())
                .map(|(_, v)| *v)
                .collect()
        };
        if !whole.is_empty() {
            out.push(Box::new(Addressed {
                to: Some(whole),
                inner: it.push,
            }));
        }
        out.extend(parts);
    }
    out
}

/// Group `gi`'s viewers' decisions for every push, in push order, with their
/// parks (`park`) updated as the serial filter would. Returns the exceptions
/// to "takes the whole push", sorted by `(i, j)`.
fn decide_group(
    r: &Registry,
    items: &[Item],
    gi: usize,
    g: usize,
    park: &mut BTreeMap<i64, BTreeSet<i64>>,
) -> Vec<Exception> {
    let mut out = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let Some((role, aud)) = &it.plan else {
            continue;
        };
        for (j, v) in aud.iter().copied().enumerate() {
            if v.rem_euclid(g as i64) as usize != gi {
                continue;
            }
            let (d, write) = decide(r, role, it.push.as_ref(), &it.placed, park.get(&v), v);
            match d {
                Decision::Whole => {}
                Decision::Skip => out.push(Exception {
                    i: i as u32,
                    j: j as u32,
                    d: Cut1::Skip,
                }),
                Decision::Part(p) => out.push(Exception {
                    i: i as u32,
                    j: j as u32,
                    d: Cut1::Part(p),
                }),
            }
            writes.extend(write);
        }
        // After the whole audience: a viewer listed twice decides both times
        // against the park as the previous push left it.
        for write in writes.drain(..) {
            apply(park, write);
        }
    }
    out
}

/// One viewer's park, changed by one push that reached it: written after the
/// reads, so the filter holds no borrow of [`Parks`] while it decides.
struct Write {
    viewer: i64,
    removed: Vec<i64>,
    added: Vec<i64>,
    /// `SetState`: the park becomes exactly this, before the other two.
    reset: Option<Vec<i64>>,
}

impl Write {
    fn reset(viewer: i64, ids: Vec<i64>) -> Self {
        Write {
            viewer,
            removed: Vec::new(),
            added: Vec::new(),
            reset: Some(ids),
        }
    }
}

/// **The world-clock stage** (`tick::STAGES`, last, so what it schedules
/// leaves in this tick's `take_due`): every [`RECONCILE_EVERY`] ticks,
/// reconcile every system a modelled pilot is in, and stop modelling anyone
/// no longer in space.
pub fn tick(w: &mut Registry, _now: Instant, _ft: BlueTime) {
    let parks = w.ext_mut::<Parks>();
    parks.ticks = parks.ticks.wrapping_add(1);
    if !parks.ticks.is_multiple_of(RECONCILE_EVERY) {
        return;
    }
    let viewers: Vec<i64> = parks.viewers().collect();
    let mut systems = BTreeSet::new();
    for v in viewers {
        match w.roster().get(v) {
            Some(p) => {
                systems.insert(p.solar_system_id);
            }
            None => forget(w, v),
        }
    }
    for s in systems {
        reconcile_in(w, s);
    }
}

/// **Bring every modelled park in one system up to its grid**: for each
/// pilot, one `RemoveBalls` of the local balls now past [`GRID_LEAVE`] (one
/// positional argument, never a destruction effect --
/// `docs/lanes/addballs.md` §3.2 and §3.4) and one `AddBalls2` of those now
/// inside [`GRID_ENTER`], scheduled with no delay so they leave on the next
/// `take_due` and pass through [`filter`] like any push.
///
/// Called by [`tick`], and by `/tp` for the pilot it moved. The positions
/// are brought up to now first: the roster is only true as of each ship's
/// last command (`Registry::advance_system`), and a warp is a command a
/// minute long.
///
/// **A ball the model holds that is not a local ball here is left alone**:
/// a global one, or a stale id that no longer exists anywhere -- removing
/// the second is the job of whatever made it go, and the client skips an
/// unknown id silently anyway (`michelle:1831`) **[V]**.
pub fn reconcile_in(w: &mut Registry, system: i64) {
    reconcile_in_with(w, system, crate::par::threads(), ReconcileMins::DEFAULT);
}

/// Where the reconcile goes parallel (PA-7). **[A]**, from the measured unit
/// costs and PA-6's spawn cost (~0.12 ms at 8 threads); falsified by
/// `grid_bench` showing the kit slower than serial near a threshold.
#[derive(Clone, Copy)]
pub(crate) struct ReconcileMins {
    /// Viewers x local balls from which the per-viewer diff runs on the kit
    /// (`diff_park` is ~7 ns per pair).
    pub diff_pairs: usize,
    /// Gaining viewers from which `added` runs on the kit.
    pub added: usize,
}

impl ReconcileMins {
    pub(crate) const DEFAULT: Self = Self {
        diff_pairs: 150_000,
        added: 32,
    };
}

/// [`reconcile_in`] with the thread count and thresholds explicit (tests and
/// benches). One thread is the serial path, exactly.
pub(crate) fn reconcile_in_with(
    w: &mut Registry,
    system: i64,
    threads: usize,
    mins: ReconcileMins,
) {
    let viewers: Vec<i64> = w
        .ext::<Parks>()
        .viewers()
        .filter(|v| {
            w.roster()
                .get(*v)
                .is_some_and(|p| p.solar_system_id == system)
        })
        .collect();
    if viewers.is_empty() {
        return;
    }
    w.advance_system(system);
    let local = local_balls(w, system);
    // **A ship in warp follows the ordinary grid rule** (LOCK-WARP, which
    // reverts BOT-REDOCK's hold). The roster position of a warper is its true
    // place on the warp curve (`Motion::advanced_at` flies
    // `destiny::warp_trajectory`, the client's own profile), so an observer
    // keeps it while it is still on his grid -- he sees it accelerate away
    // for a few seconds -- and it is removed when that position passes
    // GRID_LEAVE. The hold kept it for the whole warp: a ship bound for
    // another belt stayed on the overview 50 AU out, the GRID-VIS defect.
    // What the client does is only fly its copy to the arrival tick
    // (`docs/re/warp.md` 1.1); it never removes a ball for distance, so
    // the removal is the server's. **[A]** that TQ removes at the grid edge
    // and not at the start of the warp; falsified by a warper that blinks
    // out of view at the moment the tunnel opens.
    let warping: BTreeSet<i64> = w
        .roster()
        .in_system(system)
        .iter()
        .filter(|p| {
            matches!(
                p.motion.mode,
                crate::destiny::Mode::Warp { effect_stamp, .. } if effect_stamp >= 0
            )
        })
        .map(|p| p.ship_id)
        .collect();
    // **A landing is a fresh add**: a ship that was in warp at the last
    // reconcile and is not now leaves and re-enters the parks that held it
    // (or just leaves, when it landed off their grid), so every observer
    // gets it as an `AddBalls2` in Stop mode, as before the hold -- not only
    // the late settle batch, which a client that does not fly its own copy
    // of the ball (the bots' world) waits on. **Not for a ball the viewer orbits,
    // follows or has locked**: its client would end that order on the
    // removal and the server would keep it.
    let local_ids: BTreeSet<i64> = local.iter().map(|(id, _)| *id).collect();
    let landed: BTreeSet<i64> = w
        .ext::<Parks>()
        .warping
        .iter()
        .copied()
        .filter(|id| local_ids.contains(id) && !warping.contains(id))
        .collect();
    {
        let parks = w.ext_mut::<Parks>();
        parks.warping.retain(|id| !local_ids.contains(id));
        parks.warping.extend(warping.iter().copied());
    }
    let by_id = id_order(&local);
    // CLOAK-1: the cloaked ships; no viewer but the owner holds one.
    let hidden = crate::cloak::hidden_ids(w);
    // **Plan, then apply** (PA-7): each viewer's `(gone, new)` reads only the
    // roster, its own park and the shared `local`, and `schedule` writes
    // neither, so the plans may be made on the kit and applied in viewer
    // order, which is the order the serial loop scheduled in.
    let plans: Vec<Option<(Vec<i64>, Vec<i64>)>> = {
        let r: &Registry = w;
        let one = |v: &i64| {
            let me = r.roster().get(*v).copied()?;
            let held = r.ext::<Parks>().of(*v)?;
            // What the viewer's own client holds an order on: an orbit or
            // follow target, and its locks. The client drops these when the
            // ball is removed, and nothing re-sends them, so a landing must
            // not remove-and-add such a ball (LANDING-ORBIT, #112).
            let mut keep: Vec<i64> = r.locks().targets_of(me.ship_id);
            keep.extend(me.motion.tether().map(|(t, _)| t));
            Some(diff_park(
                me.ship_id,
                me.motion.position,
                &local,
                by_id.as_deref(),
                held,
                &landed,
                &keep,
                &hidden,
            ))
        };
        let min = if viewers.len() * local.len() >= mins.diff_pairs {
            2
        } else {
            usize::MAX
        };
        // Workers only read, so a panic leaves nothing to undo; the serial
        // map raises it again on this thread, as before PA-7.
        crate::par::par_map_with(threads, &viewers, min, one)
            .unwrap_or_else(|_| viewers.iter().map(&one).collect())
    };
    let mut adds: Vec<(i64, Vec<i64>)> = Vec::new();
    for (v, plan) in viewers.iter().zip(plans) {
        let Some((gone, new)) = plan else { continue };
        // Stamp 0: `take_due` stamps a scheduled batch as it leaves.
        if let Some(rm) = crate::destiny::BallsRemoved::new(vec![*v], 0, gone) {
            w.schedule(Duration::ZERO, rm);
        }
        if !new.is_empty() {
            adds.push((*v, new));
        }
    }
    if adds.is_empty() {
        return;
    }
    // **One scene for the system**, built only when somebody gains a ball:
    // the slims, gauges and modules a `SetState` would carry.
    let scene = crate::destiny::scene(w, system, 0, None, 0);
    let index = scene.add_index();
    // **The effects already running, by either end** (FX-REPLAY): a viewer
    // who gains a ship learns its guns, reps and beams in the same batch,
    // after the `AddBalls2`. Read from the scene, so the plan needs no more
    // than `&Registry`.
    let mut fx_at: std::collections::BTreeMap<i64, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, f) in scene.fx.iter().enumerate() {
        fx_at.entry(f.ship_id).or_default().push(i);
        if let Some(t) = f.target_id {
            fx_at.entry(t).or_default().push(i);
        }
    }
    let built: Vec<Option<crate::destiny::BallsAdded>> = {
        let r: &Registry = w;
        let one = |(v, ids): &(i64, Vec<i64>)| {
            let added = scene.added_indexed(&index, vec![*v], 0, ids)?;
            if fx_at.is_empty() {
                return Some(added);
            }
            let held = r.ext::<Parks>().of(*v);
            let ego = r.roster().get(*v).map(|p| p.ship_id);
            // What the viewer's park holds once the batch lands.
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            let sees = |id: i64| {
                Some(id) == ego
                    || held.is_some_and(|h| h.contains(&id))
                    || sorted.binary_search(&id).is_ok()
            };
            let mut at: Vec<usize> = ids
                .iter()
                .filter_map(|id| fx_at.get(id))
                .flatten()
                .copied()
                .collect();
            at.sort_unstable();
            at.dedup();
            let fx = at
                .into_iter()
                .map(|i| &scene.fx[i])
                .filter(|f| sees(f.ship_id) && f.target_id.is_none_or(&sees))
                .map(|f| crate::modules::SpecialFx {
                    to: vec![*v],
                    ..f.clone()
                })
                .collect();
            Some(added.with_fx(fx))
        };
        let min = if adds.len() >= mins.added {
            2
        } else {
            usize::MAX
        };
        crate::par::par_map_with(threads, &adds, min, one)
            .unwrap_or_else(|_| adds.iter().map(&one).collect())
    };
    for added in built.into_iter().flatten() {
        w.schedule(Duration::ZERO, added);
    }
}

/// **What one viewer's park gains and loses**, as ball ids in `local` order:
/// the grid rule ([`on_grid`]) applied to every local ball, with the park
/// `held`.
///
/// The obvious loop ([`diff_scan`]) looks every local ball up in `held`, a
/// set lookup per (viewer, ball) pair. `by_id` is `local`'s indices sorted
/// by ball id (one sort per reconcile, shared by every viewer), and `held`
/// is a sorted set, so the two are walked together: a distance per ball and
/// no lookup, 6.6 ms to well under that at 500 pilots in one system
/// (PERF-HOLDS). The answer is [`diff_scan`]'s, which the tests hold it
/// to. `by_id` is `None` when `local` repeats an id, which no index is true
/// to; the scan answers then.
#[allow(clippy::too_many_arguments)]
fn diff_park(
    ego: i64,
    at: [f64; 3],
    local: &[(i64, [f64; 3])],
    by_id: Option<&[usize]>,
    held: &BTreeSet<i64>,
    landed: &BTreeSet<i64>,
    keep: &[i64],
    hidden: &BTreeSet<i64>,
) -> (Vec<i64>, Vec<i64>) {
    let Some(by_id) = by_id else {
        return diff_scan(ego, at, local, held, landed, keep, hidden);
    };
    let (mut gone, mut new): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let mut park = held.iter().peekable();
    for &i in by_id {
        let (id, ball) = local[i];
        // Held ids that are not local here are left alone.
        while park.next_if(|h| **h < id).is_some() {}
        let has = park.next_if(|h| **h == id).is_some();
        if id == ego {
            continue;
        }
        let d = crate::space_state::distance(at, ball);
        let on = !hidden.contains(&id) && (d <= GRID_ENTER || (has && d <= GRID_LEAVE));
        match (has, on) {
            (true, false) => gone.push(i),
            (true, true) if landed.contains(&id) && !keep.contains(&id) => {
                gone.push(i);
                new.push(i);
            }
            (false, true) => new.push(i),
            _ => {}
        }
    }
    gone.sort_unstable();
    new.sort_unstable();
    (
        gone.into_iter().map(|i| local[i].0).collect(),
        new.into_iter().map(|i| local[i].0).collect(),
    )
}

/// `local`'s indices in ball-id order, or `None` when an id repeats.
fn id_order(local: &[(i64, [f64; 3])]) -> Option<Vec<usize>> {
    let mut order: Vec<usize> = (0..local.len()).collect();
    order.sort_unstable_by_key(|&i| local[i].0);
    order
        .windows(2)
        .all(|w| local[w[0]].0 != local[w[1]].0)
        .then_some(order)
}

/// [`diff_park`] by the plain loop over every local ball: the rule as it is
/// stated, and the oracle for the fast one.
fn diff_scan(
    ego: i64,
    at: [f64; 3],
    local: &[(i64, [f64; 3])],
    held: &BTreeSet<i64>,
    landed: &BTreeSet<i64>,
    keep: &[i64],
    hidden: &BTreeSet<i64>,
) -> (Vec<i64>, Vec<i64>) {
    let (mut gone, mut new) = (Vec::new(), Vec::new());
    for &(id, ball) in local {
        // The ego is never taken out of its own park, and is in it from
        // the state on.
        if id == ego {
            continue;
        }
        let has = held.contains(&id);
        match (has, on_grid(at, ball, has) && !hidden.contains(&id)) {
            (true, false) => gone.push(id),
            (true, true) if landed.contains(&id) && !keep.contains(&id) => {
                gone.push(id);
                new.push(id);
            }
            (false, true) => new.push(id),
            _ => {}
        }
    }
    (gone, new)
}

#[cfg(test)]
pub(crate) mod tests;
