//! **Site squads and lifecycle**: the NPCs of an instantiated site, their
//! triggers and in-pocket respawn, the site's completion, and its despawn.
//!
//! W15 packet CP28b (`docs/content/plan.md` §3.3, §6.2; spec
//! `docs/content/C3-population.md` §4.5 and §6, `docs/content/
//! C6-npc-content.md` R5, plan conflict 6, `docs/re/dungeon-model.md` §5).
//! Lane note: `docs/lanes/cp28b.md`.
//!
//! `site.rs` (CP28a) turns a reached site's entry room into balls. This
//! module watches its instances ([`tick`], a stage after `site`):
//!
//! - **Squads (R5).** On a new instance, `npc_gen::dungeon_squads` gives one
//!   squad per spawn entity of the dungeon; the ones of the placed room
//!   become [`Group`]s at their object, each member on a ring of the
//!   object's radius plus `orbitRange` and within `jitterRadius` of it
//!   **[V fields]**. Every NPC is stamped through `site::tag` with its
//!   `dunObjectID` (the spawn's object) and `entityGroupID` (the entity).
//! - **Triggers (R5's rule, [A]).** An entity without `triggerSpawn` spawns
//!   on arrival. A `triggerSpawn` one waits for the death of the room's last
//!   ungated squad ([`trigger_due`]); the other half of R5's rule, the next
//!   room's gate, does not exist yet (only the entry room is placed).
//! - **In-pocket respawn.** A squad whose ships are all gone comes back
//!   after `U(min, max)` seconds of its [`window`] **[V fields]**, unless
//!   it is `entitySpawnOnce` **[V]**, which never respawns.
//! - **Completion (C3 §6, [A]).** [`is_complete`]: G1, every squad gone
//!   after at least one kill (NPC deaths arrive through the `kill_hooks::NPC`
//!   row [`npc_killed`]); G2/G3, the placed rocks down to
//!   [`ORE_DONE_FRACTION`] (a gas site's clouds count as rocks, SOC-B2). Respawns stop at completion, and
//!   `dungeon_tracking::complete` (CP36) tells the pilots inside at once,
//!   long before the despawn's `site::vacate`.
//! - **Despawn (conflict 6).** A pocket nobody has been within
//!   `site::ARRIVAL_RANGE` of for [`SITE_VACANT`] leaves: its NPCs, then
//!   `site::vacate` (one `RemoveBalls`). A completed one then marks its
//!   population slot through `population::complete`, so the slot refills
//!   after its row's `respawn_s`.
//!
//! **Why the slot is marked at despawn, not at completion** [A]:
//! `site::tick` vacates an instance as soon as its site stops being live,
//! so marking it at completion would remove the pocket while the pilot who
//! cleared it is still inside, which conflict 6's grace exists to prevent.
//! The scanner keeps listing a completed site until it despawns. Falsified
//! by a client that expects a cleared anomaly to leave the overlay at once.
//!
//! **Not persisted.** NPCs are not (as belts' are not); a restart
//! re-instantiates a live site on the next arrival with fresh squads.

use crate::damage::BlueTime;
use crate::dungeon_data::{self, Dungeon, Object, Spawn};
use crate::kill_hooks::KillEvent;
use crate::npc::{self, NpcType};
use crate::service::Registry;
use crate::site::{self, Instance, Sites};
use crate::world_ext::{Boxed, SlotSpec};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const BLUE_PER_SECOND: BlueTime = 10_000_000;

/// The registration row; the coordinator adds it to `world_ext::SLOTS`.
/// Builds empty and reads no deltas (nothing here is persisted).
pub const SLOT: SlotSpec = SlotSpec {
    name: "site_spawn",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(SiteSpawns::default())
}

/// **Conflict 6**: how long a site instance stays with nobody within
/// `site::ARRIVAL_RANGE` before it despawns, completed or abandoned.
/// **[A]**: CCP's `threatTimeoutSeconds`, borrowed; falsified by a pilot who
/// warps out to repair and finds the pocket gone, or a cleared pocket that
/// lingers far too long.
pub const SITE_VACANT: BlueTime = 900 * BLUE_PER_SECOND;

/// How often [`tick`] runs, as `site`'s own check. **[A]**; falsified by a
/// respawn or a completion that is visibly late.
pub const CHECK_EVERY: BlueTime = site::CHECK_EVERY;

/// **G2/G3** (C3 §6): an ore or ice site is done when its rocks are down to
/// this fraction of what was placed. **[A]**: the inverse of the client's
/// "largely untouched" 0.9 (`mining_util.pyc:104`); falsified by a site
/// that vanishes while it still looks well stocked.
pub const ORE_DONE_FRACTION: f64 = 0.1;

/// A spawn's orbit when the row has none. **[A]**; falsified by a squad
/// that stands visibly too far from or too close to its structure.
pub const DEFAULT_ORBIT: f64 = 1_500.0;

/// Where a squad is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Ungated, not spawned yet: it spawns on the next step.
    Ready,
    /// `triggerSpawn`: waits for [`trigger_due`].
    Gated,
    /// In space.
    Live,
    /// All gone; back at this time.
    Respawn(BlueTime),
    /// All gone for good (`entitySpawnOnce`, no window, no ships, or the
    /// site is complete).
    Spent,
}

/// One spawn entity's squad.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The room it stands in (Y-COMBAT: a site with gates has several).
    pub room: i64,
    /// The row's index in the dungeon's `spawns`.
    pub spawn: usize,
    /// The catalogue's `objectID` it stands at (the NPCs' `dunObjectID`).
    pub object: i64,
    /// The entity id (the NPCs' `entityGroupID`).
    pub entity: i64,
    /// Type ids, lead first (R5).
    pub members: Vec<i64>,
    /// The object's position in space, and its radius.
    pub centre: [f64; 3],
    pub radius: f64,
    pub orbit: f64,
    pub jitter: f64,
    pub gated: bool,
    pub once: bool,
    /// Respawn delay `[min, max]`, seconds.
    pub window: Option<(i64, i64)>,
    pub phase: Phase,
    /// The NPC ids in space now.
    pub npcs: Vec<i64>,
    /// How many times it has spawned.
    pub rounds: u64,
}

/// One instance's squads and lifecycle.
#[derive(Debug, Clone, PartialEq)]
pub struct Pocket {
    pub site_id: i64,
    pub system: i64,
    /// The instance's sequence number: a re-instantiation is a new pocket.
    pub seq: u64,
    pub anchor: [f64; 3],
    /// The anchors of the rooms placed so far, the entry room's first
    /// (Y-COMBAT, `combat_site`).
    pub anchors: Vec<[f64; 3]>,
    pub groups: Vec<Group>,
    /// Kills of this pocket's NPCs ([`npc_killed`]).
    pub kills: u32,
    /// The placed rocks' ids.
    pub rocks: Vec<i64>,
    pub completed: Option<BlueTime>,
    pub vacant_since: Option<BlueTime>,
}

/// The world slot.
#[derive(Debug, Default)]
pub struct SiteSpawns {
    pockets: BTreeMap<i64, Pocket>,
    /// NPC id to its site.
    by_npc: BTreeMap<i64, i64>,
    /// Loaded NPC types `npc::find` does not know, derived once.
    kinds: BTreeMap<i64, &'static NpcType>,
    due: BlueTime,
}

impl SiteSpawns {
    #[cfg(test)]
    pub fn get(&self, site_id: i64) -> Option<&Pocket> {
        self.pockets.get(&site_id)
    }

    /// The site an NPC was spawned for.
    #[cfg(test)]
    pub fn site_of(&self, npc_id: i64) -> Option<i64> {
        self.by_npc.get(&npc_id).copied()
    }
}

/// **The respawn window** of a spawn row: the entity's
/// `[entityMinRespawnDelay, entityMaxRespawnDelay]`, else its object's
/// `[objectMin…, objectMax…]` **[V fields]**, in seconds, ordered. One bound
/// alone is both. `None` when neither has one, or the upper bound is not
/// positive (`[A]`: a zero window would respawn every check).
pub fn window(spawn: &Spawn, object: Option<&Object>) -> Option<(i64, i64)> {
    let pair = |a: Option<i64>, b: Option<i64>| {
        match (a, b) {
            (None, None) => None,
            (Some(x), None) | (None, Some(x)) => Some((x, x)),
            (Some(x), Some(y)) => Some((x.min(y), x.max(y))),
        }
        .filter(|(_, hi)| *hi > 0)
        .map(|(lo, hi)| (lo.max(0), hi))
    };
    pair(spawn.min_respawn, spawn.max_respawn)
        .or_else(|| object.and_then(|o| pair(o.min_respawn, o.max_respawn)))
}

/// When a squad emptied at `now` comes back: `now + U(min, max)` seconds,
/// drawn from `roll` in `[0, 1)`.
pub fn respawn_at(window: (i64, i64), now: BlueTime, roll: f64) -> BlueTime {
    now + npc::pick(roll, window.0, window.1) * BLUE_PER_SECOND
}

/// **R5's trigger rule** (the half that exists, [A]): the gated squads come
/// when the room's last ungated squad is gone (every ungated one is
/// respawning or spent). A room with no ungated squad has nothing to wait
/// for, so its gated ones come on arrival. Falsified by a site whose second
/// wave the client shows on some other event.
pub fn trigger_due(groups: &[Group]) -> bool {
    groups.iter().any(|g| g.phase == Phase::Gated)
        && groups
            .iter()
            .filter(|g| !g.gated)
            .all(|g| matches!(g.phase, Phase::Respawn(_) | Phase::Spent))
}

/// **C3 §6's completion test** [A]. A site with squads needs G1: at least one
/// kill, and no squad in space or still to come (a pending respawn does not
/// count). A site with rocks needs G2/G3: at most [`ORE_DONE_FRACTION`] of
/// them left -- and a gas site's clouds are rocks (`Law::cloud`,
/// SOC-B2), so it completes the same way once its clouds are harvested.
/// A site with neither (data, relic) never completes here.
pub fn is_complete(groups: &[Group], kills: u32, rocks_placed: usize, rocks_left: usize) -> bool {
    let squads = groups.iter().any(|g| !g.members.is_empty());
    let g1 = kills > 0
        && groups
            .iter()
            .all(|g| matches!(g.phase, Phase::Respawn(_) | Phase::Spent));
    let g2 = rocks_left as f64 <= ORE_DONE_FRACTION * rocks_placed as f64;
    (squads || rocks_placed > 0) && (!squads || g1) && (rocks_placed == 0 || g2)
}

/// **Y-WH: a Sleeper site with rocks is done when its rocks are.** A
/// reservoir's Sleepers come late (`delay_arrival`) and a huffer may empty
/// the clouds before any is killed, which `is_complete`'s G1 would never
/// call done (the site would sit empty in the scanner until it expired).
/// [A]: harvesting the site out completes it, as for a site with no
/// squads. Falsified by an emptied reservoir that stays on the overlay.
pub fn harvested_out(w: &Registry, p: &Pocket, rocks_left: usize) -> bool {
    let sleeper = || {
        let site = w.ext::<Sites>().get(p.site_id)?;
        let data = w.content().try_get::<crate::npc_data::Table>()?;
        Some(data.sleeper_dungeons.contains(&site.dungeon_id))
    };
    !p.rocks.is_empty()
        && rocks_left as f64 <= ORE_DONE_FRACTION * p.rocks.len() as f64
        && sleeper().unwrap_or(false)
}

/// **The pure squads of a placed room** (R5): `npc_gen::dungeon_squads` for
/// the dungeon, keeping the spawns of `room`. `placed` answers a catalogue
/// object's position and radius in space; an object it does not know stands
/// at `anchor + pos`, else at the anchor.
pub fn groups_of(
    d: &Dungeon,
    squads: &[crate::npc_gen::SlotSquad],
    room: i64,
    anchor: [f64; 3],
    placed: &dyn Fn(i64) -> Option<([f64; 3], f64)>,
) -> Vec<Group> {
    squads
        .iter()
        .filter(|s| s.room == room)
        .filter_map(|s| {
            // PV-8: a Triglavian dungeon with no spawn rows is given one
            // squad at its entry object (`pochven::squads`), whose `spawn`
            // names no row: it stands for a row of defaults.
            let stand_in = crate::dungeon_data::Spawn {
                room: s.room,
                object: s.object,
                ..crate::dungeon_data::Spawn::default()
            };
            let sp = d
                .spawns
                .get(s.spawn)
                .or_else(|| d.spawns.is_empty().then_some(&stand_in))?;
            let object = d.objects.iter().find(|o| o.id == sp.object);
            let (centre, radius) = placed(sp.object).unwrap_or_else(|| {
                let p = object.map_or([0.0; 3], |o| o.pos);
                let r = object.and_then(|o| o.radius).unwrap_or(0.0);
                ([anchor[0] + p[0], anchor[1] + p[1], anchor[2] + p[2]], r)
            });
            let phase = match (s.squad.members.is_empty(), sp.trigger_spawn) {
                (true, _) => Phase::Spent,
                (false, true) => Phase::Gated,
                (false, false) => Phase::Ready,
            };
            Some(Group {
                room,
                spawn: s.spawn,
                object: sp.object,
                entity: sp.entity,
                members: s.squad.members.clone(),
                centre,
                radius: radius.max(0.0),
                orbit: sp
                    .orbit_range
                    .filter(|r| *r >= 0.0)
                    .unwrap_or(DEFAULT_ORBIT),
                jitter: sp.jitter_radius.filter(|r| *r >= 0.0).unwrap_or(0.0),
                gated: sp.trigger_spawn,
                once: sp.spawn_once,
                window: window(sp, object),
                phase,
                npcs: Vec::new(),
                rounds: 0,
            })
        })
        .collect()
}

/// Member `i` of `n`: on the ring of `radius + orbit` round the centre, and
/// moved by at most `jitter`. `r(k)` is a roll in `[0, 1)`.
pub fn member_at(g: &Group, i: usize, n: usize, r: &dyn Fn(u64) -> f64) -> [f64; 3] {
    use std::f64::consts::TAU;
    let ring = g.radius + g.orbit;
    let a = TAU * (r(0) + i as f64 / n.max(1) as f64);
    let j = g.jitter * r(1);
    let (theta, phi) = (TAU * r(2), (2.0 * r(3) - 1.0).clamp(-1.0, 1.0).acos());
    [
        g.centre[0] + ring * a.cos() + j * phi.sin() * theta.cos(),
        g.centre[1] + j * phi.cos(),
        g.centre[2] + ring * a.sin() + j * phi.sin() * theta.sin(),
    ]
}

/// **The kill hook** (`kill_hooks::NPC` row): a pocket's NPC died, which
/// counts toward G1. The roster has already lost it; [`tick`] notices the
/// squad's state.
pub fn npc_killed(w: &mut Registry, ev: &KillEvent) {
    let s = w.ext_mut::<SiteSpawns>();
    let Some(site) = s.by_npc.get(&ev.victim_ship).copied() else {
        return;
    };
    if let Some(p) = s.pockets.get_mut(&site) {
        p.kills += 1;
    }
}

/// **The stage** (after `site`): every [`CHECK_EVERY`], new instances get
/// their squads, instances gone take their NPCs with them, and every pocket
/// steps: respawns, triggers, completion and despawn.
pub fn tick(w: &mut Registry, _now: Instant, ft: BlueTime) {
    if ft < w.ext::<SiteSpawns>().due {
        return;
    }
    w.ext_mut::<SiteSpawns>().due = ft + CHECK_EVERY;
    let instances: BTreeMap<i64, Instance> = w
        .ext::<Sites>()
        .iter()
        .map(|i| (i.site_id, i.clone()))
        .collect();
    let stale: Vec<i64> = w
        .ext::<SiteSpawns>()
        .pockets
        .values()
        .filter(|p| instances.get(&p.site_id).is_none_or(|i| i.seq != p.seq))
        .map(|p| p.site_id)
        .collect();
    for id in stale {
        drop_pocket(w, id);
    }
    for inst in instances.values() {
        if w.ext::<SiteSpawns>().pockets.contains_key(&inst.site_id) {
            continue;
        }
        let mut p = open(w, inst);
        delay_arrival(w, &mut p, ft);
        w.ext_mut::<SiteSpawns>().pockets.insert(p.site_id, p);
    }
    let ids: Vec<i64> = w.ext::<SiteSpawns>().pockets.keys().copied().collect();
    // **Each system is carried to now once per scan, not once per pocket**
    // (PERF-STAGES): every pocket's vacancy check reads the pilots' places,
    // and a belt system with eight pockets advanced its whole roster eight
    // times. The scan is one instant, as `Registry::advance_system` wants.
    let systems: std::collections::BTreeSet<i64> = w
        .ext::<SiteSpawns>()
        .pockets
        .values()
        .map(|p| p.system)
        .collect();
    for system in systems {
        w.advance_system(system);
    }
    for id in ids {
        step(w, id, ft);
    }
}

/// **[A] How long a Sleeper squad whose spawn row says `delay_spawn` waits
/// after the site is opened**, seconds: the rows' own `entityMinRespawn`
/// (900) [V-data], which EvEJS reads as the 15 to 20 minutes before the
/// Sleepers of a gas reservoir arrive. Only J-space sites
/// (`npc_data::Table::sleeper_dungeons`) are delayed: the flag is on pirate
/// dungeons too, and whether they wait is not Y-WH's to change. Falsified
/// by a huffer shot on arrival at a Reservoir, or one never troubled.
pub const DELAY_SPAWN: i64 = 900;

/// **Y-WH: the arrival delay.** Every ready squad of a Sleeper site whose
/// spawn row has `delay_spawn` comes [`DELAY_SPAWN`] seconds after the
/// pocket opens, not at once: a reservoir is empty when the huffer warps
/// in.
pub fn delay_arrival(w: &Registry, p: &mut Pocket, ft: BlueTime) {
    let Some(site) = w.ext::<Sites>().get(p.site_id) else {
        return;
    };
    let (Some(t), Some(data)) = (
        w.content().try_get::<dungeon_data::Table>(),
        w.content().try_get::<crate::npc_data::Table>(),
    ) else {
        return;
    };
    let Some(d) = t.dungeons.get(&site.dungeon_id) else {
        return;
    };
    if !data.sleeper_dungeons.contains(&d.id) {
        return;
    }
    for g in &mut p.groups {
        if g.phase == Phase::Ready && d.spawns.get(g.spawn).is_some_and(|s| s.delay_spawn) {
            g.phase = Phase::Respawn(ft + DELAY_SPAWN * BLUE_PER_SECOND);
        }
    }
}

/// The stream parts of a site's squad draw: its id, except for a mission
/// site, which draws the dungeon's one fixed fill (`&[]`), exactly what
/// `mission_rewards` prices (`spawn_bounty_sums`).
pub fn squad_parts(site_id: i64) -> Vec<u64> {
    if crate::mission::is_mission_site(site_id) {
        Vec::new()
    } else {
        vec![site_id as u64]
    }
}

/// A new instance's pocket: its room's R5 squads, none spawned yet.
pub fn open(w: &Registry, inst: &Instance) -> Pocket {
    let groups = (|| {
        let room = inst.room_id?;
        let d = w
            .content()
            .try_get::<dungeon_data::Table>()?
            .dungeons
            .get(&inst.dungeon_id)?;
        let data = w.content().try_get::<crate::npc_data::Table>()?;
        let squads =
            crate::npc_gen::dungeon_squads(d, data, w.world().seed, &squad_parts(inst.site_id));
        let placed = |object: i64| {
            inst.objects
                .iter()
                .find(|p| p.dun_object_id == object)
                .map(|p| (p.position, p.radius))
        };
        Some(groups_of(d, &squads, room, inst.anchor, &placed))
    })()
    .unwrap_or_default();
    Pocket {
        site_id: inst.site_id,
        system: inst.system,
        seq: inst.seq,
        anchor: inst.anchor,
        anchors: vec![inst.anchor],
        groups,
        kills: 0,
        rocks: inst
            .objects
            .iter()
            .filter(|p| p.rock.is_some())
            .map(|p| p.item_id)
            .collect(),
        completed: None,
        vacant_since: None,
    }
}

/// One pocket, one check.
fn step(w: &mut Registry, site_id: i64, ft: BlueTime) {
    let Some(mut p) = w.ext_mut::<SiteSpawns>().pockets.remove(&site_id) else {
        return;
    };
    // Who is gone: a squad with nobody left respawns, or is spent.
    for (gi, g) in p.groups.iter_mut().enumerate() {
        let before = g.npcs.len();
        g.npcs.retain(|id| w.roster().get(*id).is_some());
        if before > 0 && g.npcs.is_empty() && g.phase == Phase::Live {
            // [A] A mission's pocket never respawns a squad: the reward prices
            // each squad once. Falsified by TQ mission rats that come back.
            let respawns = !crate::mission::is_mission_site(site_id);
            g.phase = match (g.once, g.window, p.completed) {
                (false, Some(win), None) if respawns => {
                    let salt = salt(gi, g.rounds, 15, 0);
                    Phase::Respawn(respawn_at(win, ft, npc::roll(site_id, salt)))
                }
                _ => Phase::Spent,
            };
        }
    }
    let alive: Vec<i64> = p.groups.iter().flat_map(|g| g.npcs.clone()).collect();
    w.ext_mut::<SiteSpawns>()
        .by_npc
        .retain(|id, s| *s != site_id || alive.contains(id));
    let mut spawned = Vec::new();
    if p.completed.is_none() {
        // Y-COMBAT: rooms placed behind a gate since the last look get their
        // squads and rocks.
        for room in crate::combat_site::take_fresh(w, site_id) {
            add_room(w, &mut p, room);
        }
        let due = |g: &Group| match g.phase {
            Phase::Ready => true,
            Phase::Respawn(at) => ft >= at,
            _ => false,
        };
        for gi in 0..p.groups.len() {
            if due(&p.groups[gi]) {
                // A squad that comes back (its respawn window) arrives in
                // warp; the first population (`Ready`) is there at once.
                let warp = matches!(p.groups[gi].phase, Phase::Respawn(_));
                spawned.extend(spawn_group(w, &mut p, gi, warp));
            }
        }
        // R5's trigger is per room: a room's gated squads wait for *its*
        // last ungated squad (Y-COMBAT).
        let rooms: std::collections::BTreeSet<i64> = p.groups.iter().map(|g| g.room).collect();
        for room in rooms {
            let idx: Vec<usize> = (0..p.groups.len())
                .filter(|i| p.groups[*i].room == room)
                .collect();
            let there: Vec<Group> = idx.iter().map(|i| p.groups[*i].clone()).collect();
            if trigger_due(&there) {
                for gi in idx {
                    if p.groups[gi].phase == Phase::Gated {
                        // The trigger fires with pilots in the room: a
                        // reinforcement warps in.
                        spawned.extend(spawn_group(w, &mut p, gi, true));
                    }
                }
            }
        }
        let left = p
            .rocks
            .iter()
            .filter(|id| w.rocks().get(**id).is_some())
            .count();
        // HK2: a data or relic site completes when its cans are hacked or
        // destroyed (`site::complete_with_cans`).
        let (cans, cans_left) = site::hackables_left(w, site_id);
        let other_goals = !p.rocks.is_empty() || p.groups.iter().any(|g| !g.members.is_empty());
        // Y-COMBAT: a site whose gates have not all been used is not done,
        // whatever its first rooms hold.
        let base = (is_complete(&p.groups, p.kills, p.rocks.len(), left)
            || harvested_out(w, &p, left))
            && crate::combat_site::rooms_unopened(w, site_id) == 0;
        // An empty pocket (no squad, no rock, no can) has nothing to clear:
        // it never counts as cleared and never pays LP. In build 3707965 the
        // FW complexes (archetypes 33-36) carry no static spawns **[V per
        // expac-D-data 2 and 3]**, so most of them open empty; where their
        // rats come from is **[A]** (node graphs or reactive spawns; falsifier:
        // a complex in a 3707965 client run that has rats this server never
        // spawns). `is_complete` already says false for it; this keeps the
        // rule explicit and out of reach of a later change to that test.
        let empty = !other_goals && cans == 0;
        // A battlefield is won on its scoreboard (`fw_scoreboard::win`), never
        // by clearing its pocket (#8).
        if !empty
            && !crate::fw_scoreboard::is_battlefield(w, site_id)
            && site::complete_with_cans(base, other_goals, cans, cans_left)
        {
            p.completed = Some(ft);
            // CP36: the pilots inside hear `OnDungeonCompleted` now, while
            // the instance is still there (it must precede `site::vacate`).
            crate::dungeon_tracking::complete(w, site_id);
            if crate::mission::is_mission_site(site_id) {
                // A mission's own pocket: the agent pays, so no cache, no
                // escalation roll and no militia LP; the mission is told so
                // its dialogue shows Complete.
                crate::mission::site_completed(w, site_id);
            } else {
                // Y-COMBAT: the last room's cache, and the escalation roll.
                leave_cache(w, &p, ft);
                crate::combat_site::on_complete(w, site_id, ft);
                // FU-FWLP: an FW complex pays its pilots' militia LP now,
                // while they are still on the grid -- unless its capture
                // point already took the complex and paid (FW-7A).
                if !crate::fw_capture::is_captured(w, site_id) {
                    crate::fw_lp::on_complete(w, site_id, p.system, p.anchor, ft);
                }
            }
            for g in &mut p.groups {
                if matches!(g.phase, Phase::Respawn(_)) {
                    g.phase = Phase::Spent;
                }
            }
            crate::outln!("[site_spawn] site {site_id} in {} is complete", p.system);
        }
    }
    announce(w, p.system, &spawned, ft);
    let near = crate::dungeon::audience(w, p.system).into_iter().any(|c| {
        w.roster().get(c).is_some_and(|pr| {
            p.anchors.iter().any(|a| {
                crate::space_state::distance(pr.motion.position, *a) <= site::ARRIVAL_RANGE
            })
        })
    });
    // **An AIR tutorial site is `crate::air_npe`'s to vacate**, as `site`'s
    // own sweep already leaves it: its tasks keep the pilot 130-270 km from
    // the anchor (scene 2.1's rocks and wreck), far past ARRIVAL_RANGE, and
    // this despawn tore the site down under a pilot mid-task.
    if near || crate::air_npe::is_npe_site(site_id) {
        p.vacant_since = None;
    } else if !crate::mission::is_mission_site(site_id)
        && ft.saturating_sub(*p.vacant_since.get_or_insert(ft)) >= SITE_VACANT
    {
        // A mission's pocket never leaves for being vacant: it stands until
        // the mission is turned in, quit-and-expired or declined
        // (`mission::drop_pocket` calls [`discard`]).
        despawn(w, p, ft);
        return;
    }
    w.ext_mut::<SiteSpawns>().pockets.insert(site_id, p);
}

/// **A room behind a gate gets its squads** (Y-COMBAT): the dungeon's R5
/// squads again (they are a function of the seed, the site and the dungeon,
/// so these are the ones `open` drew), the placed room's groups, its rocks.
fn add_room(w: &Registry, p: &mut Pocket, r: crate::combat_site::OpenRoom) {
    let Some(inst) = w.ext::<Sites>().get(p.site_id) else {
        return;
    };
    let groups = (|| {
        let d = w
            .content()
            .try_get::<dungeon_data::Table>()?
            .dungeons
            .get(&inst.dungeon_id)?;
        let data = w.content().try_get::<crate::npc_data::Table>()?;
        let squads =
            crate::npc_gen::dungeon_squads(d, data, w.world().seed, &squad_parts(p.site_id));
        let placed = |object: i64| {
            inst.objects
                .iter()
                .find(|q| q.dun_object_id == object)
                .map(|q| (q.position, q.radius))
        };
        Some(groups_of(d, &squads, r.room, r.anchor, &placed))
    })()
    .unwrap_or_default();
    p.groups.extend(groups);
    p.anchors.push(r.anchor);
    for q in inst.objects.iter().filter(|q| q.rock.is_some()) {
        if !p.rocks.contains(&q.item_id) {
            p.rocks.push(q.item_id);
        }
    }
}

/// **The cache of a finished site** (Y-COMBAT): owned by the corporation of
/// the last room's lead NPC, a wreck of its hull's wreck type. A site with
/// no squad (ore, gas, data) leaves none.
fn leave_cache(w: &mut Registry, p: &Pocket, ft: BlueTime) {
    let lead = p
        .groups
        .iter()
        .rev()
        .find_map(|g| g.members.first().copied());
    let Some(kind) = lead.and_then(|t| kind_of(w, t)) else {
        return;
    };
    let Some(wreck) = kind.wreck_type_id else {
        return;
    };
    crate::combat_site::drop_cache(w, p.site_id, kind.owner_corp_id, wreck, ft);
}

/// A deterministic roll's salt: group, round, member and draw.
fn salt(group: usize, round: u64, member: u64, k: u64) -> u64 {
    ((group as u64) << 32) | ((round & 0xffff) << 16) | ((member & 0xfff) << 4) | (k & 0xf)
}

/// Put group `gi` in space, tag its NPCs, and answer their ids (told by the
/// caller in one batch). A member whose ship is unknown is skipped; a squad
/// with none in space is spent.
fn spawn_group(w: &mut Registry, p: &mut Pocket, gi: usize, warp: bool) -> Vec<i64> {
    let g = p.groups[gi].clone();
    let (site_id, system) = (p.site_id, p.system);
    let n = g.members.len();
    let mut ids = Vec::new();
    for (i, type_id) in g.members.iter().enumerate() {
        let Some(kind) = kind_of(w, *type_id) else {
            continue;
        };
        let r = |k: u64| npc::roll(site_id, salt(gi, g.rounds, i as u64, k));
        let at = member_at(&g, i, n, &r);
        // `[A]` the bearing is the member's own position from the pocket's
        // origin; any bearing is a fact-free choice the client shows alike.
        let made = if warp {
            npc::spawn_warping_in(w, kind, system, at, at)
        } else {
            npc::spawn(w, kind, system, at)
        };
        let Some(id) = made else {
            break;
        };
        site::tag(
            w,
            id,
            site::Tag {
                site_id,
                dun_object_id: g.object,
                entity_group: Some(g.entity),
            },
        );
        w.ext_mut::<SiteSpawns>().by_npc.insert(id, site_id);
        ids.push(id);
    }
    let g = &mut p.groups[gi];
    g.rounds += 1;
    g.phase = if ids.is_empty() {
        Phase::Spent
    } else {
        Phase::Live
    };
    g.npcs = ids.clone();
    ids
}

/// The ship a member names: the hand row or the derived table
/// (`npc::find`), else the loaded rows over the world's typedata
/// (`npc::derive_loaded`), derived once and kept.
fn kind_of(w: &mut Registry, type_id: i64) -> Option<&'static NpcType> {
    if let Some(k) = npc::find(type_id) {
        return Some(k);
    }
    if let Some(k) = w.ext::<SiteSpawns>().kinds.get(&type_id) {
        return Some(*k);
    }
    let made = {
        let data = w.content().try_get::<crate::npc_data::Table>()?;
        let td = w.content().try_get::<crate::typedata::Installed>()?.table();
        npc::derive_loaded(td.get(type_id)?, td, data)?
    };
    let k: &'static NpcType = Box::leak(Box::new(made));
    w.ext_mut::<SiteSpawns>().kinds.insert(type_id, k);
    Some(k)
}

/// One `AddBalls2` for the NPCs, on the clock, with their slim extras (S9).
fn announce(w: &mut Registry, system: i64, ids: &[i64], ft: BlueTime) {
    if ids.is_empty() {
        return;
    }
    let to = crate::dungeon::audience(w, system);
    if to.is_empty() {
        return;
    }
    let mut balls = Vec::new();
    let mut extra = Vec::new();
    for id in ids {
        let (Some(n), Some(pr)) = (w.npcs().get(*id), w.roster().get(*id)) else {
            continue;
        };
        balls.push(npc::ball(w.content(), n, &pr.motion));
        extra.push((
            npc::slim(n),
            Some(crate::combat::damage_state(w, *id, ft).to_pyvalue()),
        ));
    }
    let extras = crate::slim_extra::for_slims(w, ids);
    let stamp = w.next_destiny_stamp();
    let park = crate::destiny::Park {
        current_time: stamp,
        balls,
    };
    if let Some(added) = crate::destiny::BallsAdded::new(to, stamp, park, extra) {
        w.schedule(Duration::ZERO, added.with_extras(extras));
    }
}

/// NPCs leave space (not killed): out of every lock, the roster and the NPC
/// table, and one `RemoveBalls` to the system's characters.
fn remove_npcs(w: &mut Registry, system: i64, ids: &[i64]) {
    let ids: Vec<i64> = ids
        .iter()
        .copied()
        .filter(|id| w.roster().get(*id).is_some())
        .collect();
    if ids.is_empty() {
        return;
    }
    for id in &ids {
        crate::targeting::left_space(
            w,
            *id,
            crate::targeting::Reason::Unlocked,
            crate::targeting::Delivery::OnTheClock,
        );
        w.roster_mut().leave(*id);
        w.npcs_mut().remove(*id);
    }
    let to = crate::dungeon::audience(w, system);
    let stamp = w.next_destiny_stamp();
    if let Some(gone) = crate::destiny::BallsRemoved::new(to, stamp, ids) {
        w.schedule(Duration::ZERO, gone);
    }
}

fn forget(w: &mut Registry, site_id: i64) -> Vec<i64> {
    let s = w.ext_mut::<SiteSpawns>();
    let ids: Vec<i64> = s
        .by_npc
        .iter()
        .filter(|(_, s)| **s == site_id)
        .map(|(id, _)| *id)
        .collect();
    s.by_npc.retain(|_, s| *s != site_id);
    ids
}

/// Whether the open pocket of `site_id` holds nothing to clear: no squad and
/// no rock. A pocket not opened yet is not known to be empty.
pub fn is_empty_pocket(w: &Registry, site_id: i64) -> bool {
    w.ext::<SiteSpawns>()
        .pockets
        .get(&site_id)
        .is_some_and(|p| p.groups.is_empty() && p.rocks.is_empty())
}

/// Whether the pocket of `site_id` has been cleared (its completion already
/// paid the militia LP): FW-7A's capture stage leaves such a site alone.
pub fn is_completed(w: &Registry, site_id: i64) -> bool {
    w.ext::<SiteSpawns>()
        .pockets
        .get(&site_id)
        .is_some_and(|p| p.completed.is_some())
}

/// The site an NPC was spawned for, while it is in space.
pub fn site_of_npc(w: &Registry, npc_id: i64) -> Option<i64> {
    w.ext::<SiteSpawns>().by_npc.get(&npc_id).copied()
}

/// The NPCs of `site_id` in space now.
#[cfg(test)]
pub fn npcs_of(w: &Registry, site_id: i64) -> Vec<i64> {
    w.ext::<SiteSpawns>()
        .by_npc
        .iter()
        .filter(|(_, s)| **s == site_id)
        .map(|(id, _)| *id)
        .collect()
}

/// **Take a site away on its owner's say-so** (a mission turned in, expired
/// or cleared by a GM): its pocket, its NPCs and its instance go, with one
/// `RemoveBalls` each. Never marks a population slot: only a site `population`
/// listed has one. `false` when nothing of it was there.
pub fn discard(w: &mut Registry, site_id: i64) -> bool {
    let had = w.ext_mut::<SiteSpawns>().pockets.remove(&site_id);
    let ids = forget(w, site_id);
    let system = had
        .as_ref()
        .map(|p| p.system)
        .or_else(|| w.ext::<Sites>().get(site_id).map(|i| i.system));
    if let Some(system) = system {
        remove_npcs(w, system, &ids);
    }
    site::vacate(w, site_id) || had.is_some()
}

/// An instance `site` dropped (its site stopped being live, or its system
/// went cold): its NPCs go too.
fn drop_pocket(w: &mut Registry, site_id: i64) {
    let Some(p) = w.ext_mut::<SiteSpawns>().pockets.remove(&site_id) else {
        return;
    };
    let ids = forget(w, site_id);
    remove_npcs(w, p.system, &ids);
}

/// **[`SITE_VACANT`] passed**: the NPCs leave, `site::vacate` sends the
/// room's one `RemoveBalls`, and a completed site's population slot is
/// marked done (it refills after its row's `respawn_s`). An abandoned one
/// stays live and is re-instantiated on the next arrival.
fn despawn(w: &mut Registry, p: Pocket, ft: BlueTime) {
    let ids = forget(w, p.site_id);
    remove_npcs(w, p.system, &ids);
    site::vacate(w, p.site_id);
    if p.completed.is_some() {
        crate::population::complete(w, p.system, p.site_id, ft);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;
    use crate::dungeon_data::{Object, Room};
    use crate::model::SandboxConfig;
    use crate::npc_data::{Bounty, Needs, NpcTypeRow, Role, Tier};
    use crate::population::{self, Site};
    use crate::space_state::{Motion, Presence};
    use eve_marshal::PyValue;
    use std::sync::Arc;

    const SYSTEM: i64 = 30_990_001;
    const ALICE: i64 = 90_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const SITE_ID: i64 = population::SITE_BASE + 4343;
    const ANCHOR: [f64; 3] = [1.0e11, 2.0e9, -3.0e10];
    /// Invented ids.
    const DUNGEON: i64 = 999_301;
    const FACTION: i64 = 599_301;
    const STRUCTURE: i64 = 990_450;

    /// The one ship a test can spawn with no typedata: the hand row,
    /// named, never numbered.
    fn hand() -> i64 {
        npc::ANGEL_HIJACKER.type_id
    }

    /// `npc_data` with the hand type as `FACTION`'s only (asteroid) frigate,
    /// with an invented bounty.
    fn npc_table() -> crate::npc_data::Table {
        let mut t = crate::npc_data::Table::default();
        t.types.insert(
            hand(),
            NpcTypeRow {
                type_id: hand(),
                group_id: 99_600,
                faction_id: FACTION,
                context: crate::npc_gen::ASTEROID.to_owned(),
                class: "Frigate".to_owned(),
                tier: Tier::Base,
                alias_of: None,
                needs: Needs::TURRET,
                role: Role::Dps,
            },
        );
        t.bounty.insert(
            hand(),
            Bounty {
                bounty: 1.0,
                ..Bounty::default()
            },
        );
        t
    }

    fn spawn_row(object: i64, entity: i64) -> Spawn {
        Spawn {
            room: 1,
            object,
            entity,
            ..Spawn::default()
        }
    }

    /// Room 1: a beacon (1) and a structure (2); one ungated spawn at 1
    /// with a window, one gated at 2, one spawn-once at 1; room 2 has a
    /// spawn that is never placed.
    fn dungeon() -> Dungeon {
        let obj = |room, id, x| Object {
            room,
            id,
            type_id: STRUCTURE,
            pos: [x, 0.0, 0.0],
            radius: Some(2_000.0),
            ..Object::default()
        };
        Dungeon {
            id: DUNGEON,
            archetype: 24,
            faction: Some(FACTION),
            difficulty: Some(2),
            entry_object: Some(1),
            rooms: vec![
                Room {
                    id: 1,
                    ..Room::default()
                },
                Room {
                    id: 2,
                    ..Room::default()
                },
            ],
            objects: vec![obj(1, 1, 0.0), obj(1, 2, 20_000.0), obj(2, 3, 0.0)],
            spawns: vec![
                Spawn {
                    min_respawn: Some(60),
                    max_respawn: Some(120),
                    orbit_range: Some(1_500.0),
                    jitter_radius: Some(500.0),
                    ..spawn_row(1, 71)
                },
                Spawn {
                    trigger_spawn: true,
                    ..spawn_row(2, 72)
                },
                Spawn {
                    spawn_once: true,
                    min_respawn: Some(60),
                    max_respawn: Some(60),
                    ..spawn_row(1, 73)
                },
                Spawn {
                    room: 2,
                    ..spawn_row(3, 74)
                },
            ],
            ..Dungeon::default()
        }
    }

    fn world() -> Registry {
        let mut table = dungeon_data::Table::fixture();
        table.dungeons.insert(DUNGEON, dungeon());
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        c.insert("npc_data", npc_table());
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        reg
    }

    fn site() -> Site {
        Site {
            system: SYSTEM,
            row: 0,
            k: 0,
            slot: 0,
            generation: 0,
            site_id: SITE_ID,
            dungeon_id: DUNGEON,
            archetype: 24,
            entry: dungeon_data::Entry::Anomaly,
            anchor: ANCHOR,
            target_id: "ABC-123".into(),
            state: population::State::Live,
            at: 0,
        }
    }

    fn drain(reg: &mut Registry) -> Vec<(&'static str, Vec<PyValue>)> {
        let _ = reg.take_pending();
        reg.take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| p.to_push())
            .collect()
    }

    /// The entry names of every `DoDestinyUpdate` among `pushes`.
    fn entry_names(pushes: &[(&'static str, Vec<PyValue>)]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (_, args) in pushes {
            let Some(PyValue::List(es)) = args.first() else {
                continue;
            };
            for e in es {
                let PyValue::Tuple(e) = e else { continue };
                let Some(PyValue::Tuple(call)) = e.get(1) else {
                    continue;
                };
                if let Some(PyValue::Bytes(b)) = call.first() {
                    out.push(b.to_vec());
                }
            }
        }
        out
    }

    fn kill(reg: &mut Registry, id: i64, when: BlueTime) {
        reg.roster_mut().leave(id);
        let ev = KillEvent {
            victim_character: id,
            victim_ship: id,
            ship_type: hand(),
            killer: Some(ALICE),
            system: SYSTEM,
            npc: Some(&npc::ANGEL_HIJACKER),
            when,
            park: Vec::new(),
        };
        npc_killed(reg, &ev);
        reg.npcs_mut().remove(id);
    }

    fn pocket(reg: &Registry) -> Pocket {
        reg.ext::<SiteSpawns>()
            .get(SITE_ID)
            .cloned()
            .expect("a pocket")
    }

    /// Instantiated and stepped once at `t0`.
    fn started() -> (Registry, BlueTime) {
        let mut reg = world();
        assert!(site::instantiate(&mut reg, &site()));
        let t0 = 1_000 * BLUE_PER_SECOND;
        tick(&mut reg, Instant::now(), t0);
        (reg, t0)
    }

    #[test]
    fn the_window_is_the_entitys_else_the_objects() {
        let o = Object {
            min_respawn: Some(300),
            max_respawn: Some(200),
            ..Object::default()
        };
        let mut s = Spawn {
            min_respawn: Some(90),
            max_respawn: Some(30),
            ..Spawn::default()
        };
        assert_eq!(window(&s, Some(&o)), Some((30, 90)));
        s.min_respawn = None;
        assert_eq!(window(&s, Some(&o)), Some((30, 30)));
        s.max_respawn = None;
        assert_eq!(window(&s, Some(&o)), Some((200, 300)));
        assert_eq!(window(&s, None), None);
        s.max_respawn = Some(0);
        assert_eq!(window(&s, None), None);
    }

    #[test]
    fn a_respawn_is_inside_the_window() {
        let (lo, hi) = (60, 120);
        for k in 0..2_000u64 {
            let at = respawn_at((lo, hi), 0, npc::roll(SITE_ID, k));
            assert!((lo * BLUE_PER_SECOND..=hi * BLUE_PER_SECOND).contains(&at));
        }
        assert_eq!(respawn_at((lo, hi), 5, 0.0), 5 + lo * BLUE_PER_SECOND);
    }

    /// An FW complex with no static spawns (3707965) opens as an empty
    /// pocket: it never counts as cleared, however long it is left, and it
    /// pays nobody. Invented ids and rows.
    #[test]
    fn an_empty_fw_complex_pocket_is_never_cleared_and_pays_no_lp() {
        let mut d = dungeon();
        d.archetype = 33;
        d.spawns.clear();
        let mut table = dungeon_data::Table::fixture();
        table.dungeons.insert(DUNGEON, d);
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        c.insert("npc_data", npc_table());
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        let mut inst = site();
        inst.archetype = 33;
        assert!(site::instantiate(&mut reg, &inst));
        let before = reg.character(ALICE).map(|c| c.balance);
        let t0 = 1_000 * BLUE_PER_SECOND;
        for k in 0..6 {
            tick(&mut reg, Instant::now(), t0 + k * 10 * CHECK_EVERY);
        }
        let p = pocket(&reg);
        assert!(p.groups.is_empty() && p.rocks.is_empty(), "an empty pocket");
        assert_eq!(p.completed, None);
        assert!(!is_completed(&reg, SITE_ID));
        assert_eq!(reg.character(ALICE).map(|c| c.balance), before);
    }

    /// #8: a battlefield (archetype 68) is won on its scoreboard, never by
    /// clearing its pocket: with the same one squad killed, the control
    /// (a combat site) completes and the battlefield does not.
    #[test]
    fn a_cleared_battlefield_pocket_is_not_completed() {
        let cleared = |archetype: i64| {
            let mut d = dungeon();
            d.archetype = archetype;
            d.spawns = vec![Spawn {
                spawn_once: true,
                min_respawn: Some(60),
                max_respawn: Some(60),
                ..spawn_row(1, 73)
            }];
            let mut table = dungeon_data::Table::fixture();
            table.dungeons.insert(DUNGEON, d);
            let mut c = Content::fixture();
            c.insert("dungeon_data", table);
            c.insert("npc_data", npc_table());
            let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
            reg.roster_mut().arrive(Presence::arriving(
                ALICE,
                ALICE_SHIP,
                SYSTEM,
                Motion::arrived(ANCHOR, [0.0; 3]),
            ));
            drain(&mut reg);
            let mut inst = site();
            inst.archetype = archetype;
            assert!(site::instantiate(&mut reg, &inst));
            let t0 = 1_000 * BLUE_PER_SECOND;
            tick(&mut reg, Instant::now(), t0);
            let ids: Vec<i64> = pocket(&reg)
                .groups
                .iter()
                .flat_map(|g| g.npcs.clone())
                .collect();
            assert!(!ids.is_empty(), "the squad came");
            for id in ids {
                kill(&mut reg, id, t0);
            }
            tick(&mut reg, Instant::now(), t0 + CHECK_EVERY);
            is_completed(&reg, SITE_ID)
        };
        assert!(cleared(24), "the control: a cleared combat site completes");
        assert!(!cleared(crate::fw_scoreboard::BATTLEFIELD));
    }

    /// PV-8: the one squad a spawnless Triglavian dungeon gets is placed at
    /// its entry object, and a squad naming a spawn row that is not there in
    /// a dungeon that has rows is still dropped.
    #[test]
    fn a_spawnless_dungeons_squad_stands_at_its_entry_object() {
        let mut d = dungeon();
        d.spawns.clear();
        let squads = |spawn| {
            vec![crate::npc_gen::SlotSquad {
                room: 1,
                object: 1,
                spawn,
                squad: crate::npc_gen::Squad {
                    faction_id: None,
                    members: vec![990_711, 990_712],
                    bounty: 0.0,
                },
            }]
        };
        let placed = |o: i64| (o == 1).then_some(([5.0, 6.0, 7.0], 100.0));
        let g = groups_of(&d, &squads(0), 1, [0.0; 3], &placed);
        assert_eq!(g.len(), 1);
        assert_eq!(
            (g[0].object, g[0].centre, g[0].phase),
            (1, [5.0, 6.0, 7.0], Phase::Ready)
        );
        assert_eq!(g[0].members, [990_711, 990_712]);
        assert!(groups_of(&d, &squads(0), 2, [0.0; 3], &placed).is_empty());
        let full = dungeon();
        assert!(!full.spawns.is_empty());
        assert!(groups_of(&full, &squads(usize::MAX), 1, [0.0; 3], &placed).is_empty());
    }

    #[test]
    fn the_squads_are_the_dungeons_r5_squads_of_the_placed_room() {
        let (reg, _) = started();
        let p = pocket(&reg);
        let want = crate::npc_gen::dungeon_squads(
            &dungeon(),
            &npc_table(),
            reg.world().seed,
            &[SITE_ID as u64],
        );
        assert_eq!(p.groups.len(), 3, "room 2's spawn is not placed");
        for g in &p.groups {
            assert_eq!(g.members, want[g.spawn].squad.members);
            assert!(!g.members.is_empty());
        }
        assert!(p.groups.iter().all(|g| g.object != 3));
    }

    #[test]
    fn ungated_squads_spawn_on_arrival_in_one_addballs2_and_gated_ones_wait() {
        let mut reg = world();
        assert!(site::instantiate(&mut reg, &site()));
        drain(&mut reg);
        tick(&mut reg, Instant::now(), BLUE_PER_SECOND);
        let p = pocket(&reg);
        for g in &p.groups {
            if g.gated {
                assert_eq!(g.phase, Phase::Gated);
                assert!(g.npcs.is_empty());
            } else {
                assert_eq!(g.phase, Phase::Live);
                assert_eq!(g.npcs.len(), g.members.len());
            }
        }
        let names = entry_names(&drain(&mut reg));
        assert_eq!(names, vec![b"AddBalls2".to_vec()]);
    }

    #[test]
    fn members_stand_on_the_orbit_ring_within_the_jitter() {
        let (reg, _) = started();
        let p = pocket(&reg);
        let g = p.groups.iter().find(|g| g.spawn == 0).expect("spawn 0");
        for id in &g.npcs {
            let at = reg.roster().get(*id).expect("in space").motion.position;
            let d = crate::space_state::distance(at, g.centre);
            assert!((d - (g.radius + g.orbit)).abs() <= g.jitter + 1e-6, "{d}");
        }
    }

    #[test]
    fn npcs_are_tagged_with_their_object_and_entity() {
        let (reg, _) = started();
        let p = pocket(&reg);
        for g in &p.groups {
            for id in &g.npcs {
                assert_eq!(reg.ext::<SiteSpawns>().site_of(*id), Some(SITE_ID));
                let ex = site::slim_extras(&reg, &[*id]);
                assert_eq!(
                    ex[0].1,
                    vec![
                        (site::KEY_DUN_OBJECT, PyValue::Int(g.object)),
                        (site::KEY_ENTITY_GROUP, PyValue::Int(g.entity)),
                    ]
                );
            }
        }
    }

    #[test]
    fn the_trigger_rule() {
        let g = |gated, phase| Group {
            room: 1,
            spawn: 0,
            object: 1,
            entity: 1,
            members: vec![1],
            centre: [0.0; 3],
            radius: 0.0,
            orbit: 0.0,
            jitter: 0.0,
            gated,
            once: false,
            window: None,
            phase,
            npcs: Vec::new(),
            rounds: 0,
        };
        assert!(!trigger_due(&[
            g(false, Phase::Live),
            g(true, Phase::Gated)
        ]));
        assert!(trigger_due(&[
            g(false, Phase::Respawn(9)),
            g(false, Phase::Spent),
            g(true, Phase::Gated)
        ]));
        assert!(trigger_due(&[g(true, Phase::Gated)]), "nothing to wait for");
        assert!(!trigger_due(&[
            g(false, Phase::Spent),
            g(true, Phase::Live)
        ]));
    }

    #[test]
    fn gated_squads_come_when_the_last_ungated_squad_dies() {
        let (mut reg, t0) = started();
        let ungated: Vec<i64> = pocket(&reg)
            .groups
            .iter()
            .filter(|g| !g.gated)
            .flat_map(|g| g.npcs.clone())
            .collect();
        let (last, rest) = ungated.split_last().expect("ungated ships");
        for id in rest {
            kill(&mut reg, *id, t0);
        }
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY);
        assert!(pocket(&reg).groups.iter().any(|g| g.phase == Phase::Gated));
        kill(&mut reg, *last, t0);
        tick(&mut reg, Instant::now(), t0 + 2 * CHECK_EVERY);
        let p = pocket(&reg);
        let gated = p.groups.iter().find(|g| g.gated).expect("gated");
        assert_eq!(gated.phase, Phase::Live);
        assert!(!gated.npcs.is_empty());
        assert!(p.completed.is_none());
    }

    #[test]
    fn spawn_once_never_respawns_and_the_rest_respawn_inside_their_window() {
        let (mut reg, t0) = started();
        let p = pocket(&reg);
        // Keep the gated squad waiting: kill only the spawn-once squad and
        // the windowed one's ships, then step past the window.
        let once = p.groups.iter().find(|g| g.once).expect("once").clone();
        let win = p.groups.iter().find(|g| g.spawn == 0).expect("w").clone();
        for id in &once.npcs {
            kill(&mut reg, *id, t0);
        }
        let t1 = t0 + CHECK_EVERY;
        tick(&mut reg, Instant::now(), t1);
        let after = pocket(&reg);
        let o = after.groups.iter().find(|g| g.once).expect("once");
        assert_eq!(o.phase, Phase::Spent);
        for id in &win.npcs {
            kill(&mut reg, *id, t1);
        }
        let t2 = t1 + CHECK_EVERY;
        tick(&mut reg, Instant::now(), t2);
        let (lo, hi) = win.window.expect("a window");
        let w = pocket(&reg).groups[0].clone();
        let Phase::Respawn(at) = w.phase else {
            panic!("respawning, got {:?}", w.phase)
        };
        assert!((t2 + lo * BLUE_PER_SECOND..=t2 + hi * BLUE_PER_SECOND).contains(&at));
        // The gated squad came (every ungated squad is gone); it holds the
        // site open, so the window runs out and the squad comes back.
        tick(&mut reg, Instant::now(), at);
        let p = pocket(&reg);
        assert_eq!(p.groups[0].phase, Phase::Live);
        assert_eq!(p.groups[0].rounds, 2);
        assert_eq!(
            p.groups.iter().find(|g| g.once).expect("once").phase,
            Phase::Spent
        );
        tick(&mut reg, Instant::now(), at + 100_000 * BLUE_PER_SECOND);
        let p = pocket(&reg);
        assert_eq!(p.groups.iter().find(|g| g.once).expect("once").rounds, 1);
    }

    #[test]
    fn completion_needs_a_kill_and_no_squad_left() {
        let mut g = vec![Group {
            room: 1,
            spawn: 0,
            object: 1,
            entity: 1,
            members: vec![1],
            centre: [0.0; 3],
            radius: 0.0,
            orbit: 0.0,
            jitter: 0.0,
            gated: false,
            once: false,
            window: None,
            phase: Phase::Live,
            npcs: vec![5],
            rounds: 1,
        }];
        assert!(!is_complete(&g, 1, 0, 0));
        g[0].phase = Phase::Respawn(7);
        assert!(!is_complete(&g, 0, 0, 0), "no kill");
        assert!(is_complete(&g, 1, 0, 0));
        g[0].phase = Phase::Gated;
        assert!(!is_complete(&g, 1, 0, 0));
        assert!(!is_complete(&[], 3, 0, 0), "nothing to complete");
    }

    #[test]
    fn an_ore_site_completes_at_the_fraction() {
        let n = 50;
        let at = (ORE_DONE_FRACTION * n as f64).floor() as usize;
        assert!(is_complete(&[], 0, n, at));
        assert!(!is_complete(&[], 0, n, at + 1));
    }

    #[test]
    fn a_dropped_instance_takes_its_npcs() {
        let (mut reg, t0) = started();
        let ids: Vec<i64> = pocket(&reg)
            .groups
            .iter()
            .flat_map(|g| g.npcs.clone())
            .collect();
        assert!(!ids.is_empty());
        drain(&mut reg);
        site::vacate(&mut reg, SITE_ID);
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY);
        assert!(reg.ext::<SiteSpawns>().get(SITE_ID).is_none());
        assert!(
            ids.iter()
                .all(|id| reg.roster().get(*id).is_none() && reg.npcs().get(*id).is_none())
        );
    }

    #[test]
    fn an_abandoned_pocket_despawns_after_site_vacant() {
        let (mut reg, t0) = started();
        reg.roster_mut().get_mut(ALICE).expect("alice").motion =
            Motion::arrived([0.0; 3], [0.0; 3]);
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY);
        assert!(reg.ext::<SiteSpawns>().get(SITE_ID).is_some());
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY + SITE_VACANT);
        assert!(reg.ext::<SiteSpawns>().get(SITE_ID).is_none());
        assert!(reg.ext::<Sites>().get(SITE_ID).is_none());
        assert!(reg.ext::<SiteSpawns>().by_npc.is_empty());
    }

    /// An AIR tutorial site left for [`SITE_VACANT`] stays: `air_npe` vacates
    /// it, and its tasks keep the pilot far from the anchor.
    #[test]
    fn an_air_tutorial_site_is_not_despawned_as_vacant() {
        let npe = crate::air_npe::NPE_SITE_BASE + 1;
        let mut reg = world();
        assert!(site::instantiate(
            &mut reg,
            &Site {
                site_id: npe,
                ..site()
            }
        ));
        let t0 = 1_000 * BLUE_PER_SECOND;
        tick(&mut reg, Instant::now(), t0);
        reg.roster_mut().get_mut(ALICE).expect("alice").motion =
            Motion::arrived([0.0; 3], [0.0; 3]);
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY);
        tick(&mut reg, Instant::now(), t0 + CHECK_EVERY + SITE_VACANT);
        tick(
            &mut reg,
            Instant::now(),
            t0 + 2 * CHECK_EVERY + 2 * SITE_VACANT,
        );
        assert!(reg.ext::<Sites>().get(npe).is_some(), "still there");
    }

    /// **End to end**: a population site is reached and placed, its squads
    /// are killed, it completes; once nobody has been there for
    /// [`SITE_VACANT`] it leaves in one `RemoveBalls` and its slot is done.
    #[test]
    fn completion_then_site_vacant_gives_one_remove_balls_and_marks_the_slot() {
        use crate::celestial::{Catalogue, Universe};
        let systems: Vec<i64> = Universe::fixture().systems().map(|s| s.id).collect();
        let mut table = dungeon_data::Table::fixture();
        for d in table.dungeons.values_mut() {
            d.faction = Some(FACTION);
            d.entry_object = Some(1);
            d.objects = vec![Object {
                room: 1,
                id: 1,
                type_id: STRUCTURE,
                ..Object::default()
            }];
            d.room_rocks = Vec::new();
            d.spawns = vec![spawn_row(1, 81)];
        }
        // Every fixture region's pirate is the test faction, so the
        // faction rows draw these dungeons.
        let mut npcs = npc_table();
        for s in Universe::fixture().systems() {
            npcs.regions.insert(
                s.region_id,
                vec![crate::npc_data::RegionPirate {
                    faction_id: FACTION,
                    weight: 1.0,
                    rule: crate::npc_data::Rule::Override,
                }],
            );
        }
        let mut c = Content::fixture();
        c.insert("universe", Universe::fixture());
        c.insert("dungeon_data", table);
        c.insert("npc_data", npcs);
        let mut text = String::new();
        for (n, s) in systems.iter().enumerate() {
            let x = 1.0e11 * (n as f64 + 1.0);
            text += &format!(
                "planet\t{}\t990011\t6000000.0\t{x}\t0.0\t0.0\t{s}\n",
                40_000_000 + n
            );
        }
        let mut reg = Registry::new(SandboxConfig {
            celestials: Catalogue::parse(&text),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        reg.deltas_mut().set_world(crate::delta::World {
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        let now = reg.world().birth;
        let Some(target) = systems
            .iter()
            .flat_map(|s| population::sites(&reg, *s, now).sites)
            .find(|s| s.state == population::State::Live)
        else {
            panic!("a fixture system has a live site")
        };
        let system = target.system;
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            system,
            Motion::arrived(target.anchor, [0.0; 3]),
        ));
        drain(&mut reg);
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        site::tick(&mut reg, Instant::now(), now);
        tick(&mut reg, Instant::now(), now);
        let ids: Vec<i64> = reg
            .ext::<SiteSpawns>()
            .get(target.site_id)
            .expect("a pocket")
            .groups
            .iter()
            .flat_map(|g| g.npcs.clone())
            .collect();
        assert!(!ids.is_empty());
        for id in &ids {
            kill(&mut reg, *id, now);
        }
        let t1 = now + CHECK_EVERY;
        tick(&mut reg, Instant::now(), t1);
        let p = reg
            .ext::<SiteSpawns>()
            .get(target.site_id)
            .cloned()
            .expect("p");
        assert_eq!(p.completed, Some(t1));
        // Alice leaves the grid but stays in the system.
        reg.roster_mut().get_mut(ALICE).expect("alice").motion =
            Motion::arrived([0.0; 3], [0.0; 3]);
        let t2 = t1 + CHECK_EVERY;
        tick(&mut reg, Instant::now(), t2);
        drain(&mut reg);
        tick(&mut reg, Instant::now(), t2 + SITE_VACANT - CHECK_EVERY);
        assert!(reg.ext::<Sites>().get(target.site_id).is_some(), "grace");
        tick(&mut reg, Instant::now(), t2 + SITE_VACANT);
        assert!(reg.ext::<Sites>().get(target.site_id).is_none());
        let removes = entry_names(&drain(&mut reg))
            .into_iter()
            .filter(|n| n == b"RemoveBalls")
            .count();
        assert_eq!(removes, 1);
        let after = population::sites(&reg, system, t2 + SITE_VACANT);
        let slot = after
            .sites
            .iter()
            .find(|s| (s.row, s.k) == (target.row, target.k))
            .expect("the slot");
        assert_eq!(slot.state, population::State::Done);
    }

    // -----------------------------------------------------------------
    // Y-COMBAT: rooms behind acceleration gates
    // -----------------------------------------------------------------

    use crate::combat_site::{self, Shut};
    use crate::dungeon_data::{Gate, TypeList};

    /// Invented: a dungeon of three rooms, the entry room with the highest
    /// id (as in the bundle), and the hull groups the gates test.
    const ENTRY: i64 = 30;
    const MIDDLE: i64 = 10;
    const LAST: i64 = 11;
    const GATE_ENTRY: i64 = 2;
    const GATE_MIDDLE: i64 = 4;
    const LIST: i64 = 999_321;
    const SMALL: i64 = 990_601;
    const SMALL_GROUP: i64 = 990_025;
    const HUGE: i64 = 990_602;
    const HUGE_GROUP: i64 = 990_026;
    /// A hull of a blacklisted group (the client's own constant).
    const CAPITAL: i64 = 990_603;
    /// Cache pieces: invented weapons of the dungeon faction.
    const PIECES: [i64; 4] = [990_611, 990_612, 990_613, 990_614];

    fn gate_dungeon() -> Dungeon {
        let obj = |room, id, x| Object {
            room,
            id,
            type_id: STRUCTURE,
            pos: [x, 0.0, 0.0],
            radius: Some(500.0),
            ..Object::default()
        };
        let spawn = |room, object, entity, gated| Spawn {
            room,
            object,
            entity,
            trigger_spawn: gated,
            spawn_once: true,
            ..Spawn::default()
        };
        Dungeon {
            id: DUNGEON,
            archetype: 24,
            faction: Some(FACTION),
            difficulty: Some(combat_site::FACTION_FROM),
            entry_object: Some(1),
            rooms: [ENTRY, MIDDLE, LAST]
                .into_iter()
                .map(|id| Room {
                    id,
                    ..Room::default()
                })
                .collect(),
            objects: vec![
                obj(ENTRY, 1, 0.0),
                obj(ENTRY, GATE_ENTRY, 5_000.0),
                obj(MIDDLE, 3, 0.0),
                obj(MIDDLE, GATE_MIDDLE, 8_000.0),
                obj(LAST, 5, 0.0),
            ],
            gates: vec![
                Gate {
                    from_object: GATE_ENTRY,
                    allowed_ships_list: Some(LIST),
                    ..Gate::default()
                },
                Gate {
                    from_object: GATE_MIDDLE,
                    ..Gate::default()
                },
            ],
            spawns: vec![
                spawn(MIDDLE, 3, 71, false),
                spawn(MIDDLE, 3, 72, true),
                spawn(LAST, 5, 73, false),
            ],
            ..Dungeon::default()
        }
    }

    /// Alice in a small hull, in a world whose store knows the hulls and the
    /// cache pieces and whose faction has a weapon pool.
    fn gate_world() -> Registry {
        let rows = format!(
            "type\t{SMALL}\t{SMALL_GROUP}\t6\t1\t1\t1000.0\t500.0\t1000000.0\t30.0\t0\t\t\tFixture Small\n\
             type\t{HUGE}\t{HUGE_GROUP}\t6\t1\t1\t1000.0\t500.0\t1000000.0\t30.0\t0\t\t\tFixture Huge\n\
             type\t{CAPITAL}\t{}\t6\t1\t1\t1000.0\t500.0\t1000000.0\t30.0\t0\t\t\tFixture Capital\n\
             type\t{}\t990333\t7\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Gun A\n\
             type\t{}\t990333\t7\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Gun B\n\
             type\t{}\t990333\t7\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Gun C\n\
             type\t{}\t990333\t7\t1\t1\t0.1\t0.0\t0.0\t1.0\t0\t\t\tFixture Gun D\n",
            combat_site::BLACKLISTED_GROUPS[1],
            PIECES[0],
            PIECES[1],
            PIECES[2],
            PIECES[3],
        );
        let table = crate::typedata::Table::parse(&format!("{}{rows}", crate::typedata::FIXTURE))
            .expect("the rows parse");
        let mut dungeons = dungeon_data::Table::fixture();
        dungeons.dungeons.insert(DUNGEON, gate_dungeon());
        dungeons.typelists.insert(
            LIST,
            TypeList {
                inc_groups: vec![SMALL_GROUP],
                ..TypeList::default()
            },
        );
        let mut npcs = npc_table();
        let size = combat_site::cache_size(combat_site::FACTION_FROM);
        for (k, t) in PIECES.iter().enumerate() {
            npcs.loot.push(crate::npc_data::Loot {
                faction_id: FACTION,
                size,
                kind: "weapon".to_owned(),
                tier: Tier::Base,
                type_id: *t,
                meta_level: Some(k as i64),
            });
        }
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c.insert("dungeon_data", dungeons);
        c.insert("npc_data", npcs);
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.character_mut(ALICE).expect("Alice exists").ship_type_id = SMALL;
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        reg
    }

    fn gate_site() -> Site {
        Site {
            dungeon_id: DUNGEON,
            ..site()
        }
    }

    /// The instance's item id of the catalogue object `object`.
    fn item_of(reg: &Registry, object: i64) -> i64 {
        reg.ext::<Sites>()
            .iter()
            .flat_map(|i| i.objects.iter())
            .find(|p| p.dun_object_id == object)
            .map(|p| p.item_id)
            .expect("placed")
    }

    /// Alice, 1 km off the surface of catalogue object `object`.
    fn beside(reg: &mut Registry, object: i64) {
        let p = site::object(reg, item_of(reg, object)).expect("placed").0;
        reg.roster_mut().get_mut(ALICE).expect("alice").motion = Motion::arrived(
            [
                p.position[0] + p.radius + 1_000.0,
                p.position[1],
                p.position[2],
            ],
            [0.0; 3],
        );
    }

    fn live_npcs(reg: &Registry) -> Vec<i64> {
        pocket(reg)
            .groups
            .iter()
            .flat_map(|g| g.npcs.clone())
            .collect()
    }

    /// The key of a refusal.
    fn shut_key(s: Shut) -> String {
        match s.into_error() {
            crate::service::ServiceError::Raise(PyValue::ObjectEx { header, .. }) => {
                match *header {
                    PyValue::Tuple(t) => match t.get(1) {
                        Some(PyValue::Tuple(a)) => match a.first() {
                            Some(PyValue::Bytes(b)) => String::from_utf8_lossy(b).into_owned(),
                            other => panic!("{other:?}"),
                        },
                        other => panic!("{other:?}"),
                    },
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    /// **The row's test**: the entry room holds a beacon and a gate and no
    /// squad; the gate places the next room a grid away and answers where to
    /// warp; its first wave comes, its gated wave when that dies; the last
    /// room likewise; the site completes only when every room has been
    /// cleared, and its cache lies in the last room.
    #[test]
    fn gates_lead_through_the_rooms_and_the_site_completes_in_the_last() {
        let mut reg = gate_world();
        assert!(site::instantiate(&mut reg, &gate_site()));
        let mut t = BLUE_PER_SECOND;
        tick(&mut reg, Instant::now(), t);
        assert!(
            pocket(&reg).groups.is_empty(),
            "the entry room has no squad"
        );
        assert_eq!(combat_site::rooms_unopened(&mut reg, SITE_ID), 2);
        let entry_objects = reg.ext::<Sites>().get(SITE_ID).expect("live").objects.len();

        // The first gate: the middle room is placed a grid away.
        beside(&mut reg, GATE_ENTRY);
        let gate = item_of(&reg, GATE_ENTRY);
        let through =
            combat_site::activate(&mut reg, ALICE, gate).expect("a small hull is admitted");
        assert_eq!(through.aim, combat_site::room_anchor(ANCHOR, 1));
        assert_eq!(combat_site::rooms_unopened(&mut reg, SITE_ID), 1);
        let inst = reg.ext::<Sites>().get(SITE_ID).expect("live").clone();
        assert_eq!(
            inst.objects.len(),
            entry_objects + 2,
            "the middle room's two objects"
        );
        let ids: std::collections::BTreeSet<i64> = inst.objects.iter().map(|p| p.item_id).collect();
        assert_eq!(
            ids.len(),
            inst.objects.len(),
            "every id of the instance is its own"
        );
        let room_two = inst
            .objects
            .iter()
            .find(|p| p.dun_object_id == 3)
            .expect("placed");
        assert_eq!(room_two.position, through.aim, "at its anchor");
        assert!(
            crate::space_state::distance(room_two.position, inst.anchor) > crate::grid::GRID_LEAVE
        );

        // Its squads: the ungated one at once, the gated one when it dies.
        t += CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        let p = pocket(&reg);
        assert_eq!(p.groups.len(), 2);
        assert!(p.groups.iter().all(|g| g.room == MIDDLE));
        let first: Vec<i64> = p
            .groups
            .iter()
            .filter(|g| !g.gated)
            .flat_map(|g| g.npcs.clone())
            .collect();
        assert!(
            !first.is_empty()
                && p.groups
                    .iter()
                    .filter(|g| g.gated)
                    .all(|g| g.npcs.is_empty())
        );
        for id in first {
            kill(&mut reg, id, t);
        }
        t += CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        let second = live_npcs(&reg);
        assert!(!second.is_empty(), "the gated wave came");
        for id in second {
            kill(&mut reg, id, t);
        }
        t += CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        assert_eq!(
            pocket(&reg).completed,
            None,
            "a room is still behind the second gate"
        );

        // The second gate; the last room's squad; completion; the cache.
        beside(&mut reg, GATE_MIDDLE);
        let gate = item_of(&reg, GATE_MIDDLE);
        let through =
            combat_site::activate(&mut reg, ALICE, gate).expect("the second gate has no list");
        assert_eq!(through.aim, combat_site::room_anchor(ANCHOR, 2));
        assert_eq!(combat_site::rooms_unopened(&mut reg, SITE_ID), 0);
        t += CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        assert_eq!(pocket(&reg).completed, None);
        let last = live_npcs(&reg);
        assert!(!last.is_empty());
        for id in last {
            kill(&mut reg, id, t);
        }
        t += CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        assert_eq!(pocket(&reg).completed, Some(t), "every room cleared");
        assert!(
            is_completed(&reg, SITE_ID),
            "the capture stage must skip it"
        );

        let caches: Vec<_> = reg.combat().wrecks().copied().collect();
        assert_eq!(caches.len(), 1, "one cache");
        let c = caches[0];
        assert_eq!(c.solar_system_id, SYSTEM);
        assert!(
            crate::space_state::distance(c.position, combat_site::room_anchor(ANCHOR, 2))
                < 2.0 * combat_site::CACHE_OFFSET[0]
        );
        let inside: Vec<i64> = reg
            .store()
            .contents_of(c.wreck_id)
            .map(|i| i.type_id)
            .collect();
        assert_eq!(
            inside.iter().filter(|t| PIECES.contains(t)).count() as i64,
            combat_site::FACTION_FROM - combat_site::PIECES_BASE,
            "one piece fewer than the difficulty"
        );
        assert!(
            crate::combat::has_loot_right(&reg, ALICE, &c),
            "an NPC corporation owns it: anyone may take it"
        );
    }

    /// The gate's refusals are the client's own: a ship outside the list,
    /// a blacklisted group at a gate with no list, a gate too far.
    #[test]
    fn a_gate_refuses_by_the_clients_rule_and_by_distance() {
        let mut reg = gate_world();
        assert!(site::instantiate(&mut reg, &gate_site()));
        tick(&mut reg, Instant::now(), BLUE_PER_SECOND);
        let gate = item_of(&reg, GATE_ENTRY);
        // Too far: the anchor is 5 km off the gate.
        let far = combat_site::activate(&mut reg, ALICE, gate).expect_err("too far");
        assert_eq!(far, Shut::OutOfRange);
        beside(&mut reg, GATE_ENTRY);
        reg.character_mut(ALICE).expect("alice").ship_type_id = HUGE;
        let listed = combat_site::activate(&mut reg, ALICE, gate).expect_err("not listed");
        assert!(matches!(
            listed,
            Shut::NotListed {
                ship_type: HUGE,
                ..
            }
        ));
        assert_eq!(shut_key(listed), "DunShipCannotWarp");
        reg.character_mut(ALICE).expect("alice").ship_type_id = CAPITAL;
        let capital = combat_site::activate(&mut reg, ALICE, gate).expect_err("not listed");
        assert_eq!(
            shut_key(capital),
            "DunShipCannotWarp",
            "a list, however large, rules"
        );
        // The second room's gate has no list: only the blacklist applies.
        reg.character_mut(ALICE).expect("alice").ship_type_id = SMALL;
        beside(&mut reg, GATE_ENTRY);
        combat_site::activate(&mut reg, ALICE, gate).expect("admitted");
        beside(&mut reg, GATE_MIDDLE);
        let second = item_of(&reg, GATE_MIDDLE);
        reg.character_mut(ALICE).expect("alice").ship_type_id = CAPITAL;
        let blocked = combat_site::activate(&mut reg, ALICE, second).expect_err("blacklisted");
        assert_eq!(shut_key(blocked), "DunBlacklistCannotWarp");
        reg.character_mut(ALICE).expect("alice").ship_type_id = HUGE;
        combat_site::activate(&mut reg, ALICE, second).expect("a huge hull is no capital");
        // Not a gate at all.
        let beacon = item_of(&reg, 1);
        assert_eq!(
            combat_site::activate(&mut reg, ALICE, beacon).expect_err("a beacon"),
            Shut::NotAGate
        );
    }

    /// Going back through a gate is not placing the room twice.
    #[test]
    fn a_room_is_placed_once_however_often_its_gate_is_used() {
        let mut reg = gate_world();
        assert!(site::instantiate(&mut reg, &gate_site()));
        tick(&mut reg, Instant::now(), BLUE_PER_SECOND);
        beside(&mut reg, GATE_ENTRY);
        let gate = item_of(&reg, GATE_ENTRY);
        combat_site::activate(&mut reg, ALICE, gate).expect("first");
        let n = reg.ext::<Sites>().get(SITE_ID).expect("live").objects.len();
        beside(&mut reg, GATE_ENTRY);
        combat_site::activate(&mut reg, ALICE, gate).expect("again");
        assert_eq!(
            reg.ext::<Sites>().get(SITE_ID).expect("live").objects.len(),
            n
        );
        assert_eq!(combat_site::open_rooms(&mut reg, SITE_ID).len(), 2);
        tick(&mut reg, Instant::now(), 2 * BLUE_PER_SECOND);
        assert_eq!(pocket(&reg).groups.len(), 2, "its squads once");
    }

    /// Every room goes with the site, and a pilot who is only in the last
    /// room keeps it from despawning.
    #[test]
    fn a_pocket_stays_while_a_pilot_is_in_any_room_and_takes_every_room_when_it_goes() {
        let mut reg = gate_world();
        assert!(site::instantiate(&mut reg, &gate_site()));
        tick(&mut reg, Instant::now(), BLUE_PER_SECOND);
        beside(&mut reg, GATE_ENTRY);
        let gate = item_of(&reg, GATE_ENTRY);
        combat_site::activate(&mut reg, ALICE, gate).expect("through");
        let mut t = 2 * BLUE_PER_SECOND;
        tick(&mut reg, Instant::now(), t);
        // Alice stands in the middle room, a grid from the entry anchor.
        let at = combat_site::room_anchor(ANCHOR, 1);
        reg.roster_mut().get_mut(ALICE).expect("alice").motion = Motion::arrived(at, [0.0; 3]);
        t += SITE_VACANT + CHECK_EVERY;
        tick(&mut reg, Instant::now(), t);
        assert!(
            reg.ext::<Sites>().get(SITE_ID).is_some(),
            "she is in room two"
        );
        // She leaves; after the grace the whole instance goes.
        reg.roster_mut().get_mut(ALICE).expect("alice").motion =
            Motion::arrived([0.0; 3], [0.0; 3]);
        tick(&mut reg, Instant::now(), t + CHECK_EVERY);
        tick(&mut reg, Instant::now(), t + CHECK_EVERY + SITE_VACANT);
        assert!(reg.ext::<Sites>().get(SITE_ID).is_none());
        assert!(live_npcs_gone(&reg));
    }

    fn live_npcs_gone(reg: &Registry) -> bool {
        reg.ext::<SiteSpawns>().get(SITE_ID).is_none() && reg.npcs().is_empty()
    }

    // ---- Y-WH: Sleeper sites ---------------------------------------------

    /// The combat archetype, as the server names it.
    const COMBAT_ARCHETYPE_FOR_TEST: i64 = crate::population::COMBAT_ARCHETYPE;

    /// [`world`] with every spawn row of the dungeon `delay_spawn`, and,
    /// with `sleepers`, the dungeon a J-space one whose squads are the hand
    /// ship as an invented Sleeper.
    fn delayed_world(sleepers: bool) -> Registry {
        let mut table = dungeon_data::Table::fixture();
        let mut d = dungeon();
        // As the data: no row is both delayed and gated.
        for sp in &mut d.spawns {
            sp.delay_spawn = !sp.trigger_spawn;
        }
        table.dungeons.insert(DUNGEON, d);
        let mut npcs = npc_table();
        if sleepers {
            let row = crate::wormhole_sites::SleeperRow {
                type_id: hand(),
                group_id: 99_600,
                hull: crate::wormhole_sites::Hull::Frigate,
                rank: crate::wormhole_sites::Rank::Emergent,
                needs: Needs::TURRET,
                role: Role::Dps,
                wreck_type_id: None,
            };
            // The hand ship is the only type a test can fly: it is the
            // Sleeper, not a pirate.
            npcs.types.clear();
            npcs = crate::wormhole_sites::with_sleepers(
                npcs,
                &[row],
                std::collections::BTreeSet::from([DUNGEON]),
            );
        }
        let mut c = Content::fixture();
        c.insert("dungeon_data", table);
        c.insert("npc_data", npcs);
        let mut reg = Registry::new(SandboxConfig::default()).with_content(Arc::new(c));
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            SYSTEM,
            Motion::arrived(ANCHOR, [0.0; 3]),
        ));
        drain(&mut reg);
        reg
    }

    /// A Sleeper squad whose spawn row says `delay_spawn` arrives
    /// [`DELAY_SPAWN`] seconds after the pocket opens; a pirate dungeon with
    /// the same flag is not delayed (the rule is Y-WH's, J-space only).
    #[test]
    fn a_sleeper_squad_with_delay_spawn_arrives_late() {
        let t0 = 1_000 * BLUE_PER_SECOND;
        let mut reg = delayed_world(true);
        assert!(site::instantiate(&mut reg, &site()));
        tick(&mut reg, Instant::now(), t0);
        let p = pocket(&reg);
        let ready: Vec<&Group> = p.groups.iter().filter(|g| !g.gated).collect();
        assert!(!ready.is_empty());
        for g in &ready {
            assert_eq!(
                g.phase,
                Phase::Respawn(t0 + DELAY_SPAWN * BLUE_PER_SECOND),
                "spawn {}",
                g.spawn
            );
            assert!(g.npcs.is_empty());
        }
        tick(
            &mut reg,
            Instant::now(),
            t0 + (DELAY_SPAWN - 1) * BLUE_PER_SECOND,
        );
        assert!(
            pocket(&reg)
                .groups
                .iter()
                .filter(|g| !g.gated)
                .all(|g| g.npcs.is_empty()),
            "still waiting"
        );
        tick(
            &mut reg,
            Instant::now(),
            t0 + (DELAY_SPAWN + 1) * BLUE_PER_SECOND,
        );
        for g in pocket(&reg).groups.iter().filter(|g| !g.gated) {
            assert_eq!(g.phase, Phase::Live, "spawn {}", g.spawn);
            assert_eq!(g.npcs.len(), g.members.len());
        }
        let mut pirate = delayed_world(false);
        assert!(site::instantiate(&mut pirate, &site()));
        tick(&mut pirate, Instant::now(), t0);
        assert!(
            pocket(&pirate)
                .groups
                .iter()
                .filter(|g| !g.gated)
                .all(|g| g.phase == Phase::Live),
            "a pirate dungeon is not delayed"
        );
    }

    /// A Sleeper site with rocks is complete when its rocks are down to the
    /// fraction even though no Sleeper was killed; a pirate site is not.
    #[test]
    fn a_harvested_out_reservoir_is_complete_without_a_kill() {
        let mut sleeper = delayed_world(true);
        assert!(site::instantiate(&mut sleeper, &site()));
        tick(&mut sleeper, Instant::now(), 1_000 * BLUE_PER_SECOND);
        let mut p = pocket(&sleeper);
        p.rocks = (1..=20).collect();
        let at = (ORE_DONE_FRACTION * 20.0) as usize;
        assert!(harvested_out(&sleeper, &p, at));
        assert!(!harvested_out(&sleeper, &p, at + 1));
        p.rocks.clear();
        assert!(!harvested_out(&sleeper, &p, 0), "no rocks, no gas site");
        let mut pirate = delayed_world(false);
        assert!(site::instantiate(&mut pirate, &site()));
        tick(&mut pirate, Instant::now(), 1_000 * BLUE_PER_SECOND);
        let mut q = pocket(&pirate);
        q.rocks = (1..=20).collect();
        assert!(!harvested_out(&pirate, &q, 0));
    }

    /// **Y-WH end to end**: a J-space system's population holds a gas
    /// reservoir and a Sleeper anomaly (both dungeons carry the faction
    /// that k-space excludes, and the reservoir the excluded gas row's
    /// shape); a pilot who arrives finds the anomaly's Sleepers at once and
    /// the reservoir's only after [`DELAY_SPAWN`]; and a reservoir whose
    /// clouds are harvested out completes with no Sleeper killed.
    #[test]
    fn a_wormhole_system_holds_a_reservoir_and_an_anomaly_and_the_sleepers_come_late() {
        use crate::celestial::{Band, Catalogue, Universe};
        use crate::dungeon_data::{Entry, Resource, ResourceKind, RoomRock};
        use crate::wormhole_sites::{Hull, Rank, Sites as WhSites, SleeperRow, with_sleepers};
        const JSYS: i64 = 31_990_001;
        const GAS_D: i64 = 990_501;
        const COMBAT_D: i64 = 990_504;
        const CLOUD: i64 = 990_302;
        let universe = Universe::parse(
            &[format!(
                "{JSYS}\t19990001\t29000001\t\t\t-1.0\t\t0\t0\t0\t0\t0\t0\t1\t0\t\t\tWH"
            )],
            &["19990001\t\t\t\t\tR".to_owned()],
            &["29000001\t19990001\t\t\t\tC".to_owned()],
        )
        .expect("an invented universe");
        let obj = |id, type_id, x, radius| Object {
            room: 1,
            id,
            type_id,
            pos: [x, 0.0, 0.0],
            radius: Some(radius),
            ..Object::default()
        };
        let base = |id, archetype, entry: Entry| Dungeon {
            id,
            archetype,
            faction: Some(crate::wormhole_sites::FACTION),
            difficulty: Some(3),
            entry_type: Some(entry.type_id()),
            entry_object: Some(1),
            rooms: vec![Room {
                id: 1,
                ..Room::default()
            }],
            ..Dungeon::default()
        };
        let gas = Dungeon {
            objects: vec![
                obj(1, STRUCTURE, 0.0, 100.0),
                obj(2, CLOUD, 4_000.0, 2_000.0),
                obj(3, CLOUD, -6_000.0, 2_000.0),
            ],
            resources: vec![Resource {
                kind: ResourceKind::Gas,
                type_ids: vec![CLOUD],
                ore_band: None,
            }],
            room_rocks: vec![RoomRock {
                room: 1,
                type_id: CLOUD,
                count: 2,
            }],
            spawns: vec![Spawn {
                delay_spawn: true,
                ..spawn_row(1, 81)
            }],
            ..base(GAS_D, 30, Entry::Signature)
        };
        let combat = Dungeon {
            objects: vec![obj(1, STRUCTURE, 0.0, 100.0)],
            spawns: vec![spawn_row(1, 82)],
            ..base(COMBAT_D, COMBAT_ARCHETYPE_FOR_TEST, Entry::Anomaly)
        };
        let mut table = dungeon_data::Table::fixture();
        for d in [gas, combat] {
            table.dungeons.insert(d.id, d);
        }
        let sleeper = SleeperRow {
            type_id: hand(),
            group_id: 99_600,
            hull: Hull::Frigate,
            rank: Rank::Emergent,
            needs: Needs::TURRET,
            role: Role::Dps,
            wreck_type_id: None,
        };
        let mut npcs = npc_table();
        npcs.types.clear();
        let npcs = with_sleepers(
            npcs,
            &[sleeper],
            std::collections::BTreeSet::from([GAS_D, COMBAT_D]),
        );
        let row = |key: &str, archetype: i64, entry| population::Row {
            key: key.to_owned(),
            band: Band::Wormhole,
            archetypes: vec![archetype],
            entry,
            count: 1.0,
            difficulties: population::Diffs::Star,
            respawn_s: 600,
            ttl_s: 172_800,
            pochven: false,
        };
        let pop = population::Table::new(vec![
            row("wh_gas", 30, Entry::Signature),
            row("wh_combat", COMBAT_ARCHETYPE_FOR_TEST, Entry::Anomaly),
        ])
        .expect("two rows");
        let mut c = Content::fixture();
        c.insert("universe", universe);
        c.insert("dungeon_data", table);
        c.insert("npc_data", npcs);
        c.insert("population", pop);
        c.insert("wormhole_sites", WhSites::fixture());
        let mut reg = Registry::new(SandboxConfig {
            celestials: Catalogue::parse(&format!(
                "planet\t40000001\t990011\t6000000.0\t1.0e11\t0.0\t0.0\t{JSYS}\n"
            )),
            ..SandboxConfig::default()
        })
        .with_content(Arc::new(c));
        reg.deltas_mut().set_world(crate::delta::World {
            gen_version: population::GEN_VERSION,
            ..crate::delta::World::FIXTURE
        });
        let now = reg.world().birth;
        let sites = population::sites(&reg, JSYS, now).sites;
        let reservoir = sites
            .iter()
            .find(|s| s.dungeon_id == GAS_D)
            .expect("a reservoir");
        let anomaly = sites
            .iter()
            .find(|s| s.dungeon_id == COMBAT_D)
            .expect("a Sleeper anomaly");
        assert_eq!(
            (reservoir.archetype, anomaly.archetype),
            (30, COMBAT_ARCHETYPE_FOR_TEST)
        );
        let npcs_of = |reg: &Registry, id: i64| -> usize {
            reg.ext::<SiteSpawns>()
                .get(id)
                .map_or(0, |p| p.groups.iter().map(|g| g.npcs.len()).sum())
        };
        // The anomaly first: its Sleepers are there on arrival.
        reg.roster_mut().arrive(Presence::arriving(
            ALICE,
            ALICE_SHIP,
            JSYS,
            Motion::arrived(anomaly.anchor, [0.0; 3]),
        ));
        crate::liveness::refresh(&mut reg, Instant::now(), now);
        site::tick(&mut reg, Instant::now(), now);
        tick(&mut reg, Instant::now(), now);
        assert!(
            npcs_of(&reg, anomaly.site_id) > 0,
            "the anomaly's Sleepers are there at once"
        );
        // Then the reservoir: empty on arrival.
        let arrive = now + site::CHECK_EVERY;
        reg.roster_mut().get_mut(ALICE).expect("alice").motion =
            Motion::arrived(reservoir.anchor, [0.0; 3]);
        site::tick(&mut reg, Instant::now(), arrive);
        tick(&mut reg, Instant::now(), arrive);
        assert_eq!(
            npcs_of(&reg, reservoir.site_id),
            0,
            "the reservoir is empty on arrival"
        );
        let late = arrive + (DELAY_SPAWN + 1) * BLUE_PER_SECOND;
        tick(&mut reg, Instant::now(), late);
        assert!(
            npcs_of(&reg, reservoir.site_id) > 0,
            "the Sleepers have come"
        );
        // Harvest the clouds out: complete, with no kill.
        let ids: Vec<i64> = reg
            .ext::<Sites>()
            .get(reservoir.site_id)
            .expect("an instance")
            .objects
            .iter()
            .filter(|p| p.rock.is_some())
            .map(|p| p.item_id)
            .collect();
        assert_eq!(ids.len(), 2);
        for id in ids {
            let q = reg.rocks().get(id).expect("a cloud").quantity;
            reg.rocks_mut().mine(id, q);
        }
        tick(&mut reg, Instant::now(), late + 2 * CHECK_EVERY);
        let p = reg
            .ext::<SiteSpawns>()
            .get(reservoir.site_id)
            .cloned()
            .expect("the pocket");
        assert_eq!(p.kills, 0);
        assert!(p.completed.is_some(), "harvested out");
    }
}
