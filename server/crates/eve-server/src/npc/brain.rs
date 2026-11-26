//! An NPC's brain and its hands: [`decide`], the whole policy, pure, and
//! [`apply`], which carries one intent out through the subsystems that
//! already exist.
//!
//! Moved verbatim out of `npc.rs` by W10.5 seam S4 (`docs/content/plan.md`
//! §6.1). `crate::npc` re-exports every public item, so no caller changed;
//! the executor that calls both (`npc::tick`) stayed there.
//!
//! **CP23 (W13), turret only:** the policy reads each type's own behaviour
//! attributes through `npc_data::Table::profile` (C6 R8, N4), as a
//! [`Behaviour`] in the [`View`]: the first-shot window (475/476), target
//! switching (1648/1416), signature preference (1650), the leash (665) and
//! the self-repairer (shield 637/636/639, armour 631/630/638). What a type
//! does with each number is C6's **[I]**; a type with no profile -- no
//! bundle, or a hand row such as the Angel Hijacker -- has
//! [`Behaviour::default`], which is the pre-CP23 brain exactly. What the
//! brain remembers between ticks is its [`Mind`].
//!
//! **CP45 (W17), missiles:** a type with a launcher (attribute 507 > 0,
//! C6 N5) carries a [`crate::npc_missile::Launcher`] in its [`Behaviour`],
//! and [`decide`] adds one [`Intent::Missile`] per launch cycle while it is
//! engaged, its first shot due and the target inside the missile's reach.
//! `npc_missile` owns the flight and the landing.
//!
//! **CP49 (W18), tackle:** a type whose profile has `needs ∋ SCRAM`
//! (effect 563) or `needs ∋ WEB` (attribute 512, effect 575) carries a
//! [`crate::npc_ewar::Effect`] for each in its [`Behaviour`], and [`decide`]
//! adds one [`Intent::Scram`] / [`Intent::Web`] per cycle (505 / 513) that
//! passes its chance roll (504 / 512), on the same terms as the launcher:
//! engaged, the first shot due, the target inside the effect's range.
//! `npc_ewar` holds each cycle, answers the warp guard and the speed factor,
//! and tells the target's client.

use super::{
    AGGRESSION_RANGE, BLUE_PER_MS, MAX_SHOTS_PER_TICK, Npc, Phase, audience, orbit_latch, roll,
};
use crate::damage::{BlueTime, Shot};
use crate::destiny::Action;
use crate::service::Registry;
use std::time::Duration;

// ---------------------------------------------------------------------------
// The brain
// ---------------------------------------------------------------------------

/// How close to its spawn point a leashed NPC must come before it stops and
/// looks round again, in metres.
///
/// `[A]`, falsified by a capture of a leashed rat stopping elsewhere: C6 R8
/// says "return to spawn" and names no radius. Until it is back it ignores
/// everything, so a pilot who dragged it out cannot drag it straight back.
pub const HOME_RADIUS: f64 = 5_000.0;

/// The least leash, in metres from home, for a type that has one.
///
/// Attribute 665 is **not** a distance from home: the data has frigates
/// with 3,600 against an attack range of 5,400, which as a leash is shorter
/// than the arrival radius [`HOME_RADIUS`] -- the owner's capture shows such
/// a rat cycling Follow, Return, Stop every 750 ms and never chasing at all
/// (NPC-MOTION). EvEJS bounds the chase by a profile leash of 60,000 for its
/// dungeon rats and 250,000 for its miners' guards
/// (`dungeonUniverseSiteService.js:10233`, `miningNpcOperations.js:2091`;
/// hints only, EvEJS is often wrong), measured from the home position
/// (`npcBehaviorLoop.js:4857-4866`). So a type's leash is the greater of its
/// 665 and this.
///
/// `[A]` that 60 km feels right; falsified by a rat that is kited past it
/// and "teleports" home too early, or one that follows a pilot half way
/// across the belt field.
pub const LEASH_FLOOR: f64 = 60_000.0;

/// The furthest a rat keeps chasing a target whose lock it has lost to range,
/// centre to centre, in metres. The sweep drops a lock past the NPC's
/// targeting range (about 20 km), and without this the chase ended there:
/// the owner's "when I gain some range they stop outright chasing".
/// EvEJS notices at 50 km and lets the lock lag behind the approach
/// (`npcBehaviorLoop.js:157-172`, `NPC_MIN_ACQUISITION_RANGE_METERS`; facts
/// only; a hint), so we chase that far.
///
/// `[A]` 50 km; falsified by a rat that follows a pilot who has warped off
/// in the same system, or one that gives up while the pilot is still in the
/// belt.
pub const PURSUIT_RANGE: f64 = 50_000.0;

/// A lost lock is taken again once the target is inside this fraction of the
/// NPC's targeting range, so the sweep does not drop it on the next tick and
/// the victim's client is not told "locked" and "lost" in turn. `[A]`.
pub const RELOCK_FRACTION: f64 = 0.9;

/// How far from home an idle rat roams, in metres. `[A]`: EvEJS has no idle
/// roam (`npcBehaviorLoop.js`: an idle NPC with a home flies to it and
/// stops, `syncNpcReturnHome` `:4683-4725`; only `idleAnchorOrbit` circles an
/// anchor entity, `:4764-4803`), so the owner's "slowly roam around" is our
/// own number. Kept under [`HOME_RADIUS`] so a roaming rat is always "home".
/// Falsified by a belt that looks crowded or empty after a minute.
pub const ROAM_RADIUS: f64 = 3_000.0;

/// An idle rat picks a new roam point every this many milliseconds, drawn
/// per pick in `[min, max]`. `[A]`: at the type's cruise speed (about
/// 275 m/s for a frigate) a leg of 3 km takes about 11 s, so a rat spends
/// roughly half its time moving.
pub const ROAM_PERIOD_MS: (f64, f64) = (20_000.0, 45_000.0);

const ROAM_SALT: u64 = 0x80A3;

/// What a type fights like, from its own attributes (C6 R8): the brain's
/// half of `npc_data::Profile`, in the brain's units.
///
/// [`Behaviour::default`] is "no profile": no first-shot delay, one target
/// until it is gone, nearest first, no leash, no repairer.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Behaviour {
    /// Attributes 475/476: the first shot comes this long after the lock
    /// completes, drawn per lock in `[min, max]`, in blue ticks.
    pub attack_delay: (BlueTime, BlueTime),
    /// Attribute 665: a leash in metres from the spawn point, read through
    /// [`Behaviour::leash`]; 0 is no leash.
    pub chase_max: f64,
    /// Attributes 1648 (= 1.0) and 1416: re-pick the target this often, in
    /// blue ticks. `None` keeps one target until it is gone.
    pub switch_every: Option<BlueTime>,
    /// Attribute 1651: the chance a due switch is skipped (K-RATAI).
    pub switch_skip_chance: f64,
    /// Attribute 1650 (= 1.0): score targets by signature against its own.
    pub prefer_signature: bool,
    pub self_rep: Option<Rep>,
    /// Attribute 507 and its missile type (CP45): the launcher, if the type
    /// has one the server can fly.
    pub missile: Option<crate::npc_missile::Launcher>,
    /// Effect 563 (CP49): the scrambler, if the type's `needs ∋ SCRAM`.
    pub scram: Option<crate::npc_ewar::Effect>,
    /// Effect 575 (CP49): the web, if the type's `needs ∋ WEB`.
    pub web: Option<crate::npc_ewar::Effect>,
    /// Effect 6691 or 6756 (NEUT-NOS): the energy neutralizer.
    pub neut: Option<crate::npc_ewar::Effect>,
    /// Effect 3852, 3855, 6741 or 6742 (K-RATAI): the remote repairer.
    pub assist: Option<Assist>,
    /// A tracking disruptor on the entity (#137): the factor its optimal range
    /// has been cut to, applied to the type's attack range. `None` is none
    /// on it: the range is the type's own.
    pub gun_reach: Option<f64>,
    /// Attributes 2733 and 2734 (PV-5): a disintegrator's per-cycle climb and its
    /// cap. `None` is no ramp: 2733 is 0 on every other type [V-data].
    pub spool: Option<(f64, f64)>,
}

/// A remote repairer, in the brain's units (K-RATAI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assist {
    /// The repairing effect's typedata id ([`crate::npc_data::RemoteRep::effect_id`]),
    /// resolved with the rest of the behaviour from the type's profile
    /// ([`Behaviour::of`]); the beam's guid is looked up from it.
    pub effect_id: i64,
    pub armor: bool,
    pub amount: f64,
    /// The cycle in blue ticks, never zero.
    pub period: BlueTime,
    pub chance: f64,
    /// An ally is repaired while its layer's fraction is below this.
    pub threshold: f64,
    pub range: f64,
    pub max_targets: usize,
}

/// A self-repairer, in the brain's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rep {
    /// Armour (631/630/638) rather than shield (637/636/639).
    pub armor: bool,
    /// Hit points per cycle.
    pub amount: f64,
    /// The cycle, in blue ticks, never zero.
    pub period: BlueTime,
    /// The chance a cycle does nothing.
    pub delay_chance: f64,
}

/// Milliseconds as blue ticks, never negative.
fn ticks(ms: f64) -> BlueTime {
    (ms.max(0.0) * BLUE_PER_MS as f64) as BlueTime
}

impl Behaviour {
    /// The leash in metres from home: 0 for none, else the type's 665 or
    /// [`LEASH_FLOOR`], whichever is greater.
    pub fn leash(&self) -> f64 {
        if self.chase_max > 0.0 {
            self.chase_max.max(LEASH_FLOOR)
        } else {
            0.0
        }
    }

    /// A profile's numbers as the brain uses them. A window given high-low
    /// is read low-high; a switch timer or repair cycle of zero is none.
    pub fn from_profile(p: &crate::npc_data::Profile) -> Behaviour {
        let (a, b) = p.attack_delay_ms;
        Behaviour {
            attack_delay: (ticks(a.min(b)), ticks(a.max(b))),
            chase_max: p.chase_max.max(0.0),
            switch_every: (p.target_switch && p.switch_timer_ms > 0.0)
                .then(|| ticks(p.switch_timer_ms)),
            switch_skip_chance: p.switch_skip_chance.clamp(0.0, 1.0),
            prefer_signature: p.prefer_signature,
            assist: p
                .remote_rep
                .filter(|r| r.amount > 0.0 && ticks(r.period_ms) > 0 && r.range > 0.0)
                .map(|r| Assist {
                    effect_id: r.effect_id,
                    armor: r.armor,
                    amount: r.amount,
                    period: ticks(r.period_ms),
                    chance: r.chance,
                    threshold: r.threshold,
                    range: r.range,
                    max_targets: r.max_targets.max(1),
                }),
            self_rep: p
                .self_rep
                .filter(|r| r.amount > 0.0 && ticks(r.period_ms) > 0)
                .map(|r| Rep {
                    armor: r.armor,
                    amount: r.amount,
                    period: ticks(r.period_ms),
                    delay_chance: r.delay_chance,
                }),
            // Need the type's own rows: [`Behaviour::of`].
            missile: None,
            scram: None,
            web: None,
            neut: None,
            gun_reach: None,
            spool: None,
        }
    }

    /// The behaviour of `type_id` in this world: its profile from the
    /// loaded `npc_data` table and the world's typedata (`w.content()`), else
    /// [`Behaviour::default`].
    pub fn of(w: &Registry, type_id: i64) -> Behaviour {
        let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
            return Behaviour::default();
        };
        // The spool is a property of the type's own typedata (PV-5, #228), so a
        // beam type with no `npc_data` profile ramps too.
        let spool = spool_of(td.table(), type_id);
        let Some(table) = w.content().try_get::<crate::npc_data::Table>() else {
            return Behaviour {
                spool,
                ..Behaviour::default()
            };
        };
        table.profile(td.table(), type_id).map_or_else(
            || Behaviour {
                spool,
                ..Behaviour::default()
            },
            |p| Behaviour {
                missile: p
                    .missile
                    .and_then(|m| crate::npc_missile::Launcher::of(td.table(), type_id, &m)),
                scram: crate::npc_ewar::Effect::scram(&p, td.table()),
                web: crate::npc_ewar::Effect::web(&p, td.table()),
                neut: crate::npc_ewar::Effect::neut(&p, td.table()),
                spool,
                ..Behaviour::from_profile(&p)
            },
        )
    }
}

/// A type's spool numbers `(per cycle, cap)` from its own typedata, `None` for a
/// type with no per-cycle value.
pub fn spool_of(table: &crate::typedata::Table, type_id: i64) -> Option<(f64, f64)> {
    use crate::pochven::spool::{ATTR_MAX, ATTR_PER_CYCLE};
    let p = table.attribute(type_id, ATTR_PER_CYCLE);
    (p > 0.0).then(|| (p, table.attribute(type_id, ATTR_MAX)))
}

/// What an NPC's brain remembers between ticks, beside the fight itself.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Mind {
    /// The disintegrator's ramp (PV-5): reset by a target change or a stop.
    pub spool: crate::pochven::spool::Spool,
    /// Where it spawned: the leash's anchor. `None` is no leash.
    pub home: Option<[f64; 3]>,
    /// Leashed and on its way home: it looks at nothing until it arrives.
    pub returning: bool,
    /// When it next re-picks its target.
    pub retarget_at: Option<BlueTime>,
    /// When its repairer next cycles.
    pub next_rep_at: Option<BlueTime>,
    /// When its launcher next launches (CP45); `None` until the first launch
    /// on a lock.
    pub next_missile_at: Option<BlueTime>,
    /// When its scrambler next cycles (CP49); `None` until the first cycle
    /// on a lock.
    pub next_scram_at: Option<BlueTime>,
    /// When its web next cycles (CP49), as the scrambler.
    pub next_web_at: Option<BlueTime>,
    /// When its neutralizer next cycles (NEUT-NOS), as the scrambler.
    pub next_neut_at: Option<BlueTime>,
    /// When its remote repairer may next cycle (K-RATAI); `None` is ready.
    pub next_assist_at: Option<BlueTime>,
    /// When an idle rat next picks a roam point; `None` until its first
    /// idle look.
    pub next_wander_at: Option<BlueTime>,
    /// Who has it locked, and since when: the newest aggressor is who a
    /// switching type turns to ([`newest_aggressor`], K-RATAI **[A]**).
    pub aggressors: [Option<(i64, BlueTime)>; MAX_AGGRESSORS],
    /// The repair beams it has running (PV-6, #228): one start per ally, kept
    /// until the repairs stop, then ended ([`Intent::AssistEnd`]).
    pub assist_beams: [Option<AssistBeam>; MAX_ASSIST_BEAMS],
}

/// How many remote-repair beams one NPC keeps running; a repairer with more
/// targets than this repairs the rest without drawing a beam.
///
/// `[A]` **4 is enough.** Falsifier: a repairing NPC hull whose max locked
/// targets exceed 3 (none today, `[I]` from the type data).
pub const MAX_ASSIST_BEAMS: usize = 4;

/// How long past its cycle a repair beam is kept before it is ended, in blue
/// ticks (one second): a cycle that has not renewed it by then did not run. [A]
/// falsifier: a beam that ends between two cycles of a repairer that goes on
/// repairing the same ally.
const ASSIST_BEAM_SLACK: BlueTime = 1_000 * BLUE_PER_MS;

/// One running remote-repair beam, with what its stop and a late arrival's
/// replay need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistBeam {
    pub ally: i64,
    /// Past this the beam is ended unless a cycle renewed it.
    pub until: BlueTime,
    pub guid: &'static str,
    pub period_ms: i64,
}

/// How many aggressors a brain remembers.
pub const MAX_AGGRESSORS: usize = 4;

impl Mind {
    /// A fresh mind for an NPC spawned at `home`.
    pub fn at(home: [f64; 3]) -> Mind {
        Mind {
            home: Some(home),
            ..Mind::default()
        }
    }
}

/// A ship type's signature radius, by its hull; 0 for an unknown type,
/// which [`decide`]'s signature score treats as "no preference".
pub fn signature_of(content: &crate::content::Content, type_id: i64) -> f64 {
    crate::destiny::hull::find_in(content, type_id).map_or(0.0, |h| h.signature_radius)
}

/// A player ship an NPC can see, or a drone that provoked it
/// ([`drone_in_view`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub ship_id: i64,
    pub position: [f64; 3],
    pub is_capsule: bool,
    /// This ship has the NPC locked.
    pub locking_me: bool,
    /// How long the NPC would take to lock it, in blue ticks
    /// (`targeting::acquisition_delay`).
    pub lock_delay: BlueTime,
    /// Its signature radius ([`signature_of`]), for a type that prefers by
    /// signature.
    pub signature: f64,
    /// NPC-THREAT: this ship has acted on the NPC (damage, ewar, or a repair
    /// of one that did) within [`crate::npc_threat::AGGRESSION_MEMORY`].
    pub aggressor: bool,
    /// Its threat to this NPC: the damage it dealt it plus the healing it
    /// gave over the last [`crate::npc_threat::THREAT_WINDOW`]; 0 for a
    /// ship that is not an aggressor.
    pub threat: f64,
}

/// Another NPC in the system, as a remote repairer sees it (K-RATAI).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ally {
    pub id: i64,
    pub position: [f64; 3],
    /// Shield fraction, `None` for a hull with no shield.
    pub shield: Option<f64>,
    pub armour: f64,
}

/// Everything [`decide`] may know.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub me: [f64; 3],
    pub candidates: Vec<Candidate>,
    /// The other NPCs in its system that have taken damage; empty unless
    /// its type has a remote repairer ([`Behaviour::assist`]).
    pub allies: Vec<Ally>,
    /// The NPC's lock on its current target is still in the lock graph --
    /// the range sweep, a dock or a death has not taken it.
    pub lock_held: bool,
    /// How far its lock reaches (the sweep's range, with damps), in metres;
    /// 0 is unknown, and a lost lock is then never retaken at range.
    pub lock_range: f64,
    pub now: BlueTime,
    /// Its type's own numbers ([`Behaviour::of`]).
    pub behaviour: Behaviour,
}

/// What an NPC wants done. The executor in [`tick`] applies each.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intent {
    /// Close on a ship at full speed.
    Follow(i64),
    /// Circle a ship at cruise speed.
    Orbit(i64),
    Stop,
    Lock(i64),
    Unlock(i64),
    /// Start the gun's visual on a ship.
    FireStart(i64),
    /// Stop the gun's visual on a ship.
    FireStop(i64),
    /// One volley at `target`, landing at `at`.
    Shoot {
        target: i64,
        at: BlueTime,
        /// The spool's multiplier for this cycle (1.0 for a type with none).
        mult: f64,
    },
    /// Leashed: fly home at full speed.
    Return([f64; 3]),
    /// Idle: drift to a point near home at cruise speed.
    Roam([f64; 3]),
    /// One cycle of its own repairer.
    SelfRep {
        armor: bool,
        amount: f64,
    },
    /// One missile launched at `target` on the cycle boundary `at`; it lands
    /// after its flight (`npc_missile`, CP45).
    Missile {
        target: i64,
        at: BlueTime,
    },
    /// One cycle of its scrambler on `target` from the boundary `at`
    /// (`npc_ewar`, CP49).
    Scram {
        target: i64,
        at: BlueTime,
    },
    /// One cycle of its web on `target` from the boundary `at` (CP49).
    Web {
        target: i64,
        at: BlueTime,
    },
    /// One cycle of its energy neutralizer on `target` from the boundary
    /// `at` (NEUT-NOS).
    Neut {
        target: i64,
        at: BlueTime,
    },
    /// One cycle of its remote repairer on another NPC (K-RATAI).
    Assist {
        ally: i64,
        armor: bool,
        amount: f64,
        /// The repairing effect's id and the cycle, from the behaviour's
        /// [`Assist`]: the beam needs no profile lookup.
        effect_id: i64,
        period: BlueTime,
    },
    /// The repair beam to `ally` ends (PV-6, #228): the repairs stopped, went
    /// to someone else, or a cycle did not run.
    AssistEnd {
        ally: i64,
        guid: &'static str,
        period_ms: i64,
    },
}

/// **The whole policy**, pure: one NPC, what it can see, and what it does
/// next (`docs/lanes/npc.md` §4.2, C6 R8).
///
/// - **Idle**: engage the best non-capsule player ship inside
///   [`AGGRESSION_RANGE`], or one that has us locked and is inside our own
///   lock range (retaliation) -- Lock, then move. Best is nearest, or, for
///   a type that prefers by signature, the least distance over
///   [`signature_fit`] **[I]**.
/// - **Engaged**, movement: the follow/orbit latch ([`orbit_latch`], EvEJS's
///   numbers): follow beyond `orbit + max(5 km, 50%)`, orbit inside `orbit +
///   max(1 km, 20%)`, and between the two keep doing what it was doing.
/// - **Pursuit**: a lock lost to range while the target is still in view
///   (and inside [`PURSUIT_RANGE`] and the leash) does not end the fight:
///   the gun stops, the NPC follows, and it locks again inside
///   [`RELOCK_FRACTION`] of its lock range.
/// - **Idle roam**: with nothing to do and a home, a rat drifts to a point
///   inside [`ROAM_RADIUS`] of it every [`ROAM_PERIOD_MS`], in the home's
///   plane, at cruise speed.
/// - **Engaged**, weapons: once the lock is ready **and the type's
///   first-shot delay after it has run** (475/476, drawn per lock), and the
///   target is inside the attack range, start the gun; out of range, stop
///   it; while it runs, one volley per period, each landing on its own
///   period boundary.
/// - **Retarget** (1648/1416): at each switch timer, re-pick as Idle does,
///   the current target always eligible; a different pick drops the old
///   lock and gun and locks the new one.
/// - **Cache** (group 2009 with no damage): none of the below; it stays where
///   it is. Any other type, armed or not, is unaffected.
/// - **Leash** (665, floored at [`LEASH_FLOOR`]): engaged and further than [`Behaviour::leash`] from its spawn
///   point, stop the gun, drop the lock and fly home; it looks at nothing
///   until it is within [`HOME_RADIUS`] of home, then stops.
/// - **Disengage** when the target has left the view, became a capsule, or
///   the lock is gone: stop the gun, drop the lock, stop the engines, idle.
///   It looks again next tick.
/// - **Self-repair**, whatever else it does: every cycle of its repairer,
///   with probability `1 - delay_chance`, one [`Intent::SelfRep`] of the
///   type's amount.
/// - **Missiles** (CP45), for a type with a launcher: once the first shot is
///   due and the target is inside the missile's reach, one
///   [`Intent::Missile`] per launch cycle (506), the first at once. A type
///   with a launcher and no hull damage starts no gun.
/// - **Tackle** (CP49), for a type with a scrambler or a web: on the same
///   terms, one [`Intent::Scram`] / [`Intent::Web`] per cycle, the first at
///   once, each only if the cycle's roll is under the type's chance
///   **[I]**; a stalled clock is owed nothing (one cycle, on the latest
///   boundary). It starts before the gun, so a launcher-only tackler tackles.
/// - **Neut** (NEUT-NOS), for a type with an energy neutralizer: one
///   [`Intent::Neut`] per cycle on the same terms. Every repair and ewar
///   intent is paid from the NPC's own capacitor when it is applied
///   ([`crate::npc_ewar::pay`]): a dry rat does not repair or tackle.
///
/// `[A]`, each falsified only by "it does not feel like a rat": capsules are
/// ignored (CCP's guard tree excludes `groupCapsule`,
/// `docs/re/npc-behaviors.md` §6.3 [V] for CCP); retaliation is limited to
/// the NPC's own lock range, beyond which the sweep would drop the lock and
/// the two would flap; every shot hits; one target at a time
/// (`maxAttackTargets` 1 [V-data]); a repair cycle missed by a stalled
/// clock is not owed.
pub fn decide(npc: &Npc, view: &View) -> (Npc, Vec<Intent>) {
    let mut n = *npc;
    let mut out = Vec::new();
    // **An abyssal cache or node** (group 2009, no damage) has nothing to
    // engage with: it never locks, fires or moves.
    if unarmed(&n, &view.behaviour) {
        return (n, out);
    }
    // The law keeps exactly its CONCORD-1a brain: none of K-RATAI's
    // behaviours reach a ship of the law (`a_ship_of_the_law_ignores_...`).
    let rat = n.law.is_none();
    if rat {
        note_aggressors(&mut n, view);
    }
    if n.mind.returning {
        if n.mind
            .home
            .is_none_or(|h| crate::space_state::distance(view.me, h) <= HOME_RADIUS)
        {
            n.mind.returning = false;
            out.push(Intent::Stop);
        }
    } else if let Some(t) = n.target {
        let still = view
            .candidates
            .iter()
            .find(|c| c.ship_id == t && !c.is_capsule);
        match still {
            Some(c) => match leashed(&n, view) {
                Some(home) => {
                    let_go(&mut n, t, view.lock_held, &mut out);
                    n.mind.returning = true;
                    out.push(Intent::Return(home));
                }
                None if view.lock_held => {
                    let c = retarget(&mut n, c, view, &mut out);
                    engaged(&mut n, &c, view, &mut out);
                }
                // The lock went (range) but the target is still in reach of
                // a chase: keep on it.
                None if crate::space_state::distance(view.me, c.position) <= PURSUIT_RANGE => {
                    pursue(&mut n, c, view, &mut out);
                }
                None => {
                    let_go(&mut n, t, false, &mut out);
                    out.push(Intent::Stop);
                }
            },
            None => {
                let_go(&mut n, t, view.lock_held, &mut out);
                out.push(Intent::Stop);
            }
        }
    } else if let Some(c) = best(&n, view, None) {
        acquire(&mut n, &c, view, &mut out);
        engaged(&mut n, &c, view, &mut out);
    } else if rat {
        roam(&mut n, view, &mut out);
    }
    self_rep(&mut n, view, &mut out);
    if rat {
        assist(&mut n, view, &mut out);
    }
    (n, out)
}

/// Whether an NPC is an abyssal cache or node (group 2009) with no hull damage:
/// scoped to the cache group on purpose. `Behaviour::of` gives every type not in
/// `npc/types.tsv` the default profile, so a test on the profile would freeze
/// unlisted missile-only, remote-repair or ECM types and unarmed haulers.
fn unarmed(n: &Npc, _b: &Behaviour) -> bool {
    n.kind.group_id == crate::abyss::loot::GROUP_CACHE
        && n.kind.damage.total() <= 0.0
        && n.kind.volley().total() <= 0.0
}

/// Remember who has this NPC locked and since when: a new locker is
/// stamped `now`, one that no longer locks is forgotten, and a full memory
/// drops its oldest.
fn note_aggressors(n: &mut Npc, view: &View) {
    let locking = |id: i64| {
        view.candidates
            .iter()
            .any(|c| c.ship_id == id && c.locking_me)
    };
    for slot in n.mind.aggressors.iter_mut() {
        if slot.is_some_and(|(id, _)| !locking(id)) {
            *slot = None;
        }
    }
    for c in view.candidates.iter().filter(|c| c.locking_me) {
        if n.mind
            .aggressors
            .iter()
            .flatten()
            .any(|(id, _)| *id == c.ship_id)
        {
            continue;
        }
        let free = n.mind.aggressors.iter().position(Option::is_none);
        let at = free.unwrap_or_else(|| {
            n.mind
                .aggressors
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.map_or(BlueTime::MAX, |(_, since)| since))
                .map_or(0, |(i, _)| i)
        });
        n.mind.aggressors[at] = Some((c.ship_id, view.now));
    }
}

/// **The newest aggressor** that is a fair target (not a capsule, inside
/// the NPC's lock range): the locker that began locking last, the nearer on
/// a tie. `None` when nobody has it locked.
///
/// **[A]** that a target-switching rat turns to its newest aggressor. The
/// data says only that the type switches (1648) and how often (1416);
/// CCP's default strategy is `'threat'` (`docs/re/npc-behaviors.md` §2.7)
/// and its scoring is unread. Falsified by a capture of a switching rat
/// that stays on its first attacker, or turns to the nearest instead.
fn newest_aggressor(n: &Npc, view: &View) -> Option<Candidate> {
    view.candidates
        .iter()
        .filter(|c| !c.is_capsule)
        .filter(|c| {
            crate::space_state::distance(view.me, c.position) <= n.kind.hull.max_target_range
        })
        .filter_map(|c| {
            let since = n
                .mind
                .aggressors
                .iter()
                .flatten()
                .find(|(id, _)| *id == c.ship_id)?
                .1;
            Some((since, *c))
        })
        .max_by(|(a, ca), (b, cb)| {
            let d = |c: &Candidate| crate::space_state::distance(view.me, c.position);
            a.cmp(b).then(d(cb).total_cmp(&d(ca)))
        })
        .map(|(_, c)| c)
}

/// The remote repairer's half of [`decide`] (K-RATAI): when its cycle is
/// ready, repair up to `max_targets` allies in `range` whose layer is below
/// `threshold`, the most hurt first, if the cycle's roll is under `chance`.
/// A cycle with nobody to repair is not spent: it goes the moment someone
/// drops below the threshold. **[I]** the threshold's meaning from its name
/// (`npcRemoteArmorRepairThreshold`).
fn assist(n: &mut Npc, view: &View, out: &mut Vec<Intent>) {
    let Some(a) = view.behaviour.assist else {
        return;
    };
    // A beam no cycle has renewed is over, whatever else this tick does.
    end_beams(n, out, |b| view.now > b.until);
    if n.mind.next_assist_at.is_some_and(|due| view.now < due) {
        return;
    }
    let layer = |l: &Ally| if a.armor { Some(l.armour) } else { l.shield };
    let mut hurt: Vec<(f64, i64)> = view
        .allies
        .iter()
        .filter(|l| l.id != n.id)
        .filter(|l| crate::space_state::distance(view.me, l.position) <= a.range)
        .filter_map(|l| layer(l).filter(|f| *f < a.threshold).map(|f| (f, l.id)))
        .collect();
    if hurt.is_empty() {
        // Nobody left to repair: the beams end with the cycle that found it.
        end_beams(n, out, |_| true);
        return;
    }
    hurt.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    n.mind.next_assist_at = Some(view.now.saturating_add(a.period));
    if roll(n.id, view.now as u64 ^ ASSIST_SALT) >= a.chance {
        end_beams(n, out, |_| true);
        return;
    }
    let chosen: Vec<i64> = hurt.into_iter().take(a.max_targets).map(|h| h.1).collect();
    // A beam to someone no longer repaired ends now, not a cycle later.
    end_beams(n, out, |b| !chosen.contains(&b.ally));
    for ally in chosen {
        out.push(Intent::Assist {
            ally,
            armor: a.armor,
            amount: a.amount,
            effect_id: a.effect_id,
            period: a.period,
        });
    }
}

/// End (clear, and say so) every running repair beam `which` selects.
fn end_beams(n: &mut Npc, out: &mut Vec<Intent>, which: impl Fn(&AssistBeam) -> bool) {
    for slot in n.mind.assist_beams.iter_mut() {
        if let Some(b) = slot.filter(|b| which(b)) {
            out.push(Intent::AssistEnd {
                ally: b.ally,
                guid: b.guid,
                period_ms: b.period_ms,
            });
            *slot = None;
        }
    }
}

/// What a remote-repair cycle's chance roll is salted with.
const ASSIST_SALT: u64 = 0x4A55;
/// What a due switch's skip roll is salted with.
const SWITCH_SALT: u64 = 0x5317;

/// How well a target's signature suits a hunter's, in `(0, 1]`: the
/// smaller over the larger, so a frigate rat prefers frigates and a
/// battleship rat battleships. `[A]` the shape of C6 R8's
/// `f(sig, own class)`, which C6 marks **[I]**; falsified by a capture of a
/// signature-preferring rat passing a nearer like-sized ship for a further
/// unlike one. An unknown signature (0) is no preference.
pub fn signature_fit(theirs: f64, mine: f64) -> f64 {
    if theirs <= 0.0 || mine <= 0.0 {
        1.0
    } else {
        theirs.min(mine) / theirs.max(mine)
    }
}

/// The best target in view: not a capsule; inside [`AGGRESSION_RANGE`], or
/// locking us inside our lock range, or `keep` (the current target). The
/// least distance (over [`signature_fit`] for a type that prefers by
/// signature); the first in view wins a tie.
fn best(n: &Npc, view: &View, keep: Option<i64>) -> Option<Candidate> {
    if let Some(c) = top_threat(n, view) {
        return Some(c);
    }
    let range_to = |c: &Candidate| crate::space_state::distance(view.me, c.position);
    let mine = n.kind.hull.signature_radius;
    let cost = |c: &Candidate| {
        let d = range_to(c);
        if view.behaviour.prefer_signature {
            d / signature_fit(c.signature, mine)
        } else {
            d
        }
    };
    view.candidates
        .iter()
        .filter(|c| !c.is_capsule)
        .filter(|c| {
            let d = range_to(c);
            keep == Some(c.ship_id)
                || d <= AGGRESSION_RANGE
                || (c.locking_me && d <= n.kind.hull.max_target_range)
                // A ship of the law was sent for this pilot: anywhere in
                // its lock range, not only inside the aggression radius (a
                // gate checkpoint answers from its post).
                || (n.law.is_some() && d <= n.kind.hull.max_target_range)
        })
        .min_by(|a, b| cost(a).total_cmp(&cost(b)))
        .copied()
}

/// **The aggressor that threatens most** (NPC-THREAT): of the ships that
/// have acted on this NPC, not a capsule, inside [`PURSUIT_RANGE`] and (with
/// a leash) inside the leash of home, the highest threat; the nearer on a
/// tie, the first in view after that. `None` for the law, which keeps its
/// CONCORD-1a brain, and when nobody has acted on it.
///
/// **[A]** that a rat turns on whoever hurts it most over the last 10 s,
/// from anywhere it can chase to, ahead of a nearer ship that is only
/// within its notice radius (CCP: the highest of damage dealt plus healing,
/// `docs/re` g7). Falsified by a capture of a rat that stays on a
/// near non-aggressor while a distant sniper shoots it.
fn top_threat(n: &Npc, view: &View) -> Option<Candidate> {
    if n.law.is_some() {
        return None;
    }
    let d = |c: &Candidate| crate::space_state::distance(view.me, c.position);
    let leash = view.behaviour.leash();
    view.candidates
        .iter()
        .filter(|c| c.aggressor && !c.is_capsule)
        .filter(|c| d(c) <= PURSUIT_RANGE)
        .filter(|c| match n.mind.home {
            Some(home) if leash > 0.0 => crate::space_state::distance(c.position, home) <= leash,
            _ => true,
        })
        .copied()
        .reduce(|a, b| {
            let (da, db) = (d(&a), d(&b));
            if b.threat > a.threat || (b.threat == a.threat && db < da) {
                b
            } else {
                a
            }
        })
}

/// Lock `c` and make it the target; the switch timer starts now.
fn acquire(n: &mut Npc, c: &Candidate, view: &View, out: &mut Vec<Intent>) {
    n.target = Some(c.ship_id);
    // The law locks at once (owner, 2026-10-27: CONCORD lands and the kill
    // follows, no lock time to survive).
    let delay = if n.law.is_some() { 0 } else { c.lock_delay };
    n.lock_ready_at = Some(view.now.saturating_add(delay));
    n.mind.retarget_at = view
        .behaviour
        .switch_every
        .map(|e| view.now.saturating_add(e));
    n.mind.next_missile_at = None;
    n.mind.next_scram_at = None;
    n.mind.next_web_at = None;
    n.mind.next_neut_at = None;
    out.push(Intent::Lock(c.ship_id));
}

/// Stop the gun on `t`, drop the lock if it is held, and idle.
fn let_go(n: &mut Npc, t: i64, lock_held: bool, out: &mut Vec<Intent>) {
    if n.firing {
        out.push(Intent::FireStop(t));
    }
    n.mind.spool.stop();
    if lock_held {
        out.push(Intent::Unlock(t));
    }
    n.phase = Phase::Idle;
    n.target = None;
    n.lock_ready_at = None;
    n.firing = false;
    n.mind.retarget_at = None;
    n.mind.next_missile_at = None;
    n.mind.next_scram_at = None;
    n.mind.next_web_at = None;
    n.mind.next_neut_at = None;
}

/// Home, when the NPC is further than its type's `chase_max` from it.
fn leashed(n: &Npc, view: &View) -> Option<[f64; 3]> {
    let max = view.behaviour.leash();
    let home = n.mind.home?;
    (max > 0.0 && crate::space_state::distance(view.me, home) > max).then_some(home)
}

/// At the switch timer, re-pick; answer the target to fight this tick.
fn retarget(n: &mut Npc, current: &Candidate, view: &View, out: &mut Vec<Intent>) -> Candidate {
    let Some(every) = view.behaviour.switch_every else {
        return *current;
    };
    let due = *n
        .mind
        .retarget_at
        .get_or_insert(view.now.saturating_add(every));
    if view.now < due {
        return *current;
    }
    n.mind.retarget_at = Some(view.now.saturating_add(every));
    // Attribute 1651: a due switch is skipped with this chance. A ship of
    // the law never gets here with a type that has one ([`decide`] gates it).
    if n.law.is_none() && roll(n.id, due as u64 ^ SWITCH_SALT) < view.behaviour.switch_skip_chance {
        return *current;
    }
    // NPC-THREAT: the highest threat first; then (no ledger entry) the
    // newest locker, as before.
    let newest = if n.law.is_none() {
        top_threat(n, view).or_else(|| newest_aggressor(n, view))
    } else {
        None
    };
    let pick = newest
        .or_else(|| best(n, view, Some(current.ship_id)))
        .unwrap_or(*current);
    if pick.ship_id == current.ship_id {
        return *current;
    }
    let_go(n, current.ship_id, true, out);
    acquire(n, &pick, view, out);
    pick
}

/// When the first shot after a lock that completes at `ready` may land:
/// `ready` plus the type's delay, drawn once per lock from the NPC's id
/// and `ready`, so every tick draws the same.
fn first_shot_at(n: &Npc, b: &Behaviour, ready: BlueTime) -> BlueTime {
    let (lo, hi) = b.attack_delay;
    let r = roll(n.id, ready as u64);
    ready
        .saturating_add(lo)
        .saturating_add(((hi - lo) as f64 * r) as BlueTime)
}

/// One cycle of the repairer when it is due.
fn self_rep(n: &mut Npc, view: &View, out: &mut Vec<Intent>) {
    let Some(rep) = view.behaviour.self_rep else {
        return;
    };
    let due = *n
        .mind
        .next_rep_at
        .get_or_insert(view.now.saturating_add(rep.period));
    if view.now < due {
        return;
    }
    if roll(n.id, due as u64) >= rep.delay_chance {
        out.push(Intent::SelfRep {
            armor: rep.armor,
            amount: rep.amount,
        });
    }
    let cycles = (view.now - due) / rep.period + 1;
    n.mind.next_rep_at = Some(due.saturating_add(cycles * rep.period));
}

/// Movement for a fight: the follow/orbit latch ([`orbit_latch`]). An order
/// goes out only when the phase changes, so a steady fight is silent.
fn steer(n: &mut Npc, t: i64, gap: f64, gun_reach: Option<f64>, out: &mut Vec<Intent>) {
    // A disruptor shortens the gun's reach (#137): orbit inside it, or the rat
    // circles out of range and never fires.
    let range = match gun_reach {
        Some(f) => n.kind.orbit_range.min(n.kind.attack_range * f * 0.9),
        None => n.kind.orbit_range,
    };
    let want = orbit_latch(n.phase, gap, range);
    if want != n.phase {
        out.push(match want {
            Phase::Approach => Intent::Follow(t),
            Phase::Orbit => Intent::Orbit(t),
            Phase::Idle => Intent::Stop,
        });
        n.phase = want;
    }
}

/// The target is still there but the lock is gone (the sweep took it at the
/// NPC's targeting range): close on it, stop the gun, and lock again once it
/// is comfortably inside the lock range.
fn pursue(n: &mut Npc, c: &Candidate, view: &View, out: &mut Vec<Intent>) {
    let t = c.ship_id;
    let d = crate::space_state::distance(view.me, c.position);
    if n.firing {
        out.push(Intent::FireStop(t));
        n.firing = false;
    }
    n.mind.spool.stop();
    n.lock_ready_at = None;
    n.mind.next_missile_at = None;
    n.mind.next_scram_at = None;
    n.mind.next_web_at = None;
    n.mind.next_neut_at = None;
    let reach = if view.lock_range > 0.0 {
        view.lock_range * RELOCK_FRACTION
    } else {
        AGGRESSION_RANGE
    };
    if d <= reach {
        let delay = if n.law.is_some() { 0 } else { c.lock_delay };
        n.lock_ready_at = Some(view.now.saturating_add(delay));
        out.push(Intent::Lock(t));
    }
    // Out of lock range it is following, whatever the latch would say about
    // a gap that is only a little over the settle distance.
    if d > reach && n.phase != Phase::Approach {
        out.push(Intent::Follow(t));
        n.phase = Phase::Approach;
    } else {
        steer(n, t, d, view.behaviour.gun_reach, out);
    }
}

/// An idle rat with a home drifts to a point inside [`ROAM_RADIUS`] of it
/// now and then, at its cruise speed. The point is drawn from the NPC's id
/// and the due time, so every run draws the same; it stays in the home's
/// plane (a rat that bobs up and down looks wrong, owner 2026-10-28).
fn roam(n: &mut Npc, view: &View, out: &mut Vec<Intent>) {
    let Some(home) = n.mind.home else { return };
    if n.law.is_some() || n.kind.cruise_fraction <= 0.0 {
        return;
    }
    let next = |due: BlueTime| {
        let (lo, hi) = ROAM_PERIOD_MS;
        let r = roll(n.id, due as u64 ^ ROAM_SALT ^ 1);
        view.now.saturating_add(ticks(lo + (hi - lo) * r))
    };
    let due = match n.mind.next_wander_at {
        Some(due) => due,
        None => {
            let first = next(view.now);
            n.mind.next_wander_at = Some(first);
            return;
        }
    };
    if view.now < due {
        return;
    }
    let angle = std::f64::consts::TAU * roll(n.id, due as u64 ^ ROAM_SALT ^ 2);
    let radius = ROAM_RADIUS * roll(n.id, due as u64 ^ ROAM_SALT ^ 3).sqrt();
    n.mind.next_wander_at = Some(next(due));
    out.push(Intent::Roam([
        home[0] + radius * angle.cos(),
        home[1],
        home[2] + radius * angle.sin(),
    ]));
}

/// The engaged half of [`decide`]: movement, then weapons.
fn engaged(n: &mut Npc, c: &Candidate, view: &View, out: &mut Vec<Intent>) {
    let t = c.ship_id;
    let k = n.kind;
    let d = crate::space_state::distance(view.me, c.position);
    steer(n, t, d, view.behaviour.gun_reach, out);
    let ready = n
        .lock_ready_at
        .is_some_and(|r| view.now >= first_shot_at(n, &view.behaviour, r));
    if let Some(l) = view.behaviour.missile {
        launches(n, t, &l, ready && d <= l.reach(), view.now, out);
    }
    if let Some(e) = view.behaviour.scram {
        let next = n.mind.next_scram_at;
        n.mind.next_scram_at = tackles(n.id, t, &e, ready && d <= e.range, view.now, next, out);
    }
    if let Some(e) = view.behaviour.web {
        let next = n.mind.next_web_at;
        n.mind.next_web_at = tackles(n.id, t, &e, ready && d <= e.range, view.now, next, out);
    }
    if let Some(e) = view.behaviour.neut {
        let next = n.mind.next_neut_at;
        n.mind.next_neut_at = tackles(n.id, t, &e, ready && d <= e.range, view.now, next, out);
    }
    // A launcher-only type has no gun to start (CP45); every other type
    // fires exactly as before.
    if view.behaviour.missile.is_some() && k.volley().total() <= 0.0 {
        return;
    }
    // A tracking disruptor on it shortens the reach (#137).
    let reach = k.attack_range * view.behaviour.gun_reach.unwrap_or(1.0);
    if !n.firing && ready && d <= reach {
        out.push(Intent::FireStart(t));
        n.firing = true;
        // Never earlier than owed: stop-start flapping cannot raise the volley rate.
        n.next_shot_at = n.next_shot_at.max(view.now);
    } else if n.firing && d > reach {
        out.push(Intent::FireStop(t));
        n.firing = false;
        n.mind.spool.stop();
    }
    if n.firing {
        let period = k.weapon_period().max(1);
        let mut shots = 0;
        while view.now >= n.next_shot_at && shots < MAX_SHOTS_PER_TICK {
            // A new target resets the ramp inside `cycle` (PV-5) [I].
            let mult = match view.behaviour.spool {
                Some((p, m)) => n.mind.spool.cycle(t, p, m),
                None => 1.0,
            };
            out.push(Intent::Shoot {
                target: t,
                at: n.next_shot_at,
                mult,
            });
            n.next_shot_at = n.next_shot_at.saturating_add(period);
            shots += 1;
        }
        if view.now >= n.next_shot_at {
            // Further behind than the cap: those volleys are not owed.
            let behind = view.now - n.next_shot_at;
            n.next_shot_at += (behind / period + 1) * period;
        }
    }
}

/// The launcher's half of [`engaged`] (CP45): while `live`, one
/// [`Intent::Missile`] per cycle, the first at once, a stalled clock owed
/// at most [`MAX_SHOTS_PER_TICK`]. While not live nothing is owed: a cycle
/// that came due waits for the target, and goes the moment it is live again.
fn launches(
    n: &mut Npc,
    t: i64,
    l: &crate::npc_missile::Launcher,
    live: bool,
    now: BlueTime,
    out: &mut Vec<Intent>,
) {
    if !live {
        n.mind.next_missile_at = n.mind.next_missile_at.map(|at| at.max(now));
        return;
    }
    let period = l.period.max(1);
    let mut next = n.mind.next_missile_at.unwrap_or(now);
    let mut shots = 0;
    while now >= next && shots < MAX_SHOTS_PER_TICK {
        out.push(Intent::Missile {
            target: t,
            at: next,
        });
        next = next.saturating_add(period);
        shots += 1;
    }
    if now >= next {
        // Further behind than the cap: those launches are not owed.
        next += ((now - next) / period + 1) * period;
    }
    n.mind.next_missile_at = Some(next);
}

/// The tackle half of [`engaged`] (CP49): while `live`, one cycle of `e`
/// on `t` per period, the first at once, if the cycle's roll is under
/// `e.chance`; answers when the next is due. A stalled clock is owed only
/// the latest boundary: a scram does not stack on itself. While not live
/// nothing is owed, as [`launches`].
fn tackles(
    id: i64,
    t: i64,
    e: &crate::npc_ewar::Effect,
    live: bool,
    now: BlueTime,
    next: Option<BlueTime>,
    out: &mut Vec<Intent>,
) -> Option<BlueTime> {
    if !live {
        return next.map(|at| at.max(now));
    }
    let period = e.period.max(1);
    let due = next.unwrap_or(now);
    if now < due {
        return Some(due);
    }
    let at = due + (now - due) / period * period;
    if roll(id, (at as u64) ^ e.kind.salt()) < e.chance {
        out.push(match e.kind {
            crate::npc_ewar::Kind::Scram => Intent::Scram { target: t, at },
            crate::npc_ewar::Kind::Web => Intent::Web { target: t, at },
            crate::npc_ewar::Kind::Neut => Intent::Neut { target: t, at },
        });
    }
    Some(at.saturating_add(period))
}

/// The reverse, clamped at zero.
fn duration(ticks: BlueTime) -> Duration {
    Duration::from_millis(u64::try_from(ticks / BLUE_PER_MS).unwrap_or(0))
}

/// The character flying a ship, if a character is.
fn victim_character(w: &Registry, ship: i64) -> Option<i64> {
    w.roster()
        .by_ship(ship)
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
}

/// Apply one intent. `npc` is the record **before** [`decide`], which the
/// unlock needs: a lock dropped before it completed must not be undone on
/// the victim's screen before it was done.
pub(super) fn apply(
    w: &mut Registry,
    npc: &Npc,
    intent: Intent,
    now: BlueTime,
    actions: &mut Vec<Action>,
) {
    let id = npc.id;
    let k = npc.kind;
    // The movement orders, as the wire carries them and as the server's own
    // record takes them (see `reaim` for why those differ for an approach).
    let (wire, record): (Vec<Action>, Vec<Action>) = match intent {
        Intent::Follow(t) => {
            let speed = Action::SetSpeedFraction {
                ball: id,
                fraction: 1.0,
            };
            // A besieged structure is no roster ship: its surface point.
            let at = w
                .roster()
                .by_ship(t)
                .map(|p| p.motion.position)
                .or_else(|| {
                    let me = w.roster().get(id)?.motion.position;
                    crate::structure_combat::surface_point(w, me, t)
                });
            (
                vec![
                    Action::FollowBall {
                        ball: id,
                        target: t,
                        range: k.orbit_range,
                    },
                    speed,
                ],
                at.map_or_else(Vec::new, |point| {
                    vec![Action::GotoPoint { ball: id, point }, speed]
                }),
            )
        }
        Intent::Orbit(t) => {
            let actions = vec![
                Action::Orbit {
                    ball: id,
                    target: t,
                    range: k.orbit_range,
                },
                Action::SetSpeedFraction {
                    ball: id,
                    fraction: k.cruise_fraction,
                },
            ];
            (actions.clone(), actions)
        }
        Intent::Stop => {
            let actions = vec![Action::Stop { ball: id }];
            (actions.clone(), actions)
        }
        Intent::Lock(t) => {
            lock(w, id, t);
            return;
        }
        Intent::Unlock(t) => {
            unlock(w, npc, t, now);
            return;
        }
        Intent::FireStart(t) => {
            fx(w, npc, t, true);
            return;
        }
        Intent::FireStop(t) => {
            fx(w, npc, t, false);
            return;
        }
        Intent::Shoot { target, at, mult } => {
            shoot(w, npc, target, at, now, mult);
            return;
        }
        Intent::Return(point) => {
            let actions = vec![
                Action::GotoPoint { ball: id, point },
                Action::SetSpeedFraction {
                    ball: id,
                    fraction: 1.0,
                },
            ];
            (actions.clone(), actions)
        }
        Intent::Roam(point) => {
            let actions = vec![
                Action::GotoPoint { ball: id, point },
                Action::SetSpeedFraction {
                    ball: id,
                    fraction: k.cruise_fraction,
                },
            ];
            (actions.clone(), actions)
        }
        Intent::SelfRep { armor, amount } => {
            let effects: &[i64] = if armor {
                &crate::npc_data::dogma::CAP_ARMOR_REP
            } else {
                &crate::npc_data::dogma::CAP_SHIELD_REP
            };
            // A rat whose capacitor is dry does not repair (NEUT-NOS).
            if crate::npc_ewar::pay(w, npc, effects, now) {
                crate::combat::self_repair(w, id, armor, amount, now);
            }
            return;
        }
        Intent::Missile { target, at } => {
            if let Some(l) = Behaviour::of(w, k.type_id).missile {
                crate::npc_missile::launch(w, npc, &l, target, at);
            }
            return;
        }
        Intent::Scram { target, at } => {
            // **[A]** a structure is immune to NPC ewar; falsified by a TQ
            // structure that is scrammed, webbed or neuted by a rat.
            if w.roster().by_ship(target).is_none() && crate::structure_combat::is_target(w, target)
            {
                return;
            }
            if let Some(e) = super::behaviour_of(w, npc).scram
                && crate::npc_ewar::pay(w, npc, &crate::npc_data::dogma::CAP_SCRAM, at)
            {
                crate::npc_ewar::cycle(w, npc, &e, target, at);
            }
            return;
        }
        Intent::Web { target, at } => {
            // **[A]** a structure is immune to NPC ewar; falsified by a TQ
            // structure that is scrammed, webbed or neuted by a rat.
            if w.roster().by_ship(target).is_none() && crate::structure_combat::is_target(w, target)
            {
                return;
            }
            if let Some(e) = super::behaviour_of(w, npc).web
                && crate::npc_ewar::pay(w, npc, &crate::npc_data::dogma::CAP_WEB, at)
            {
                crate::npc_ewar::cycle(w, npc, &e, target, at);
            }
            return;
        }
        Intent::Neut { target, at } => {
            // **[A]** a structure is immune to NPC ewar; falsified by a TQ
            // structure that is scrammed, webbed or neuted by a rat.
            if w.roster().by_ship(target).is_none() && crate::structure_combat::is_target(w, target)
            {
                return;
            }
            if let Some(e) = Behaviour::of(w, k.type_id).neut
                && crate::npc_ewar::pay(w, npc, &crate::npc_data::dogma::CAP_NEUT, at)
            {
                crate::npc_ewar::cycle(w, npc, &e, target, at);
            }
            return;
        }
        Intent::Assist {
            ally,
            armor,
            amount,
            effect_id,
            period,
        } => {
            let effects: &[i64] = if armor {
                &crate::npc_data::dogma::CAP_ASSIST_ARMOR
            } else {
                &crate::npc_data::dogma::CAP_ASSIST_SHIELD
            };
            if crate::npc_ewar::pay(w, npc, effects, now) {
                crate::combat::self_repair(w, ally, armor, amount, now);
                assist_fx(w, npc, effect_id, period, ally, now);
            }
            return;
        }
        Intent::AssistEnd {
            ally,
            guid,
            period_ms,
        } => {
            let to = audience(w, npc.solar_system_id);
            if !to.is_empty() {
                w.schedule(
                    Duration::ZERO,
                    assist_beam(to, npc, ally, guid, period_ms, false),
                );
            }
            return;
        }
    };
    if let Some(p) = w.roster_mut().get_mut(id) {
        for a in &record {
            p.motion.after(*a);
        }
    }
    actions.extend(wire);
}

/// Into the lock graph now; the victim learns when the lock **completes**,
/// as `targeting::lock` tells them (`targeting.rs`, "The victim learns when
/// the lock completes").
fn lock(w: &mut Registry, id: i64, target: i64) {
    // An ECM cycle on the entity refuses it every lock until it ends
    // (K-EWAR).
    if crate::ewar::jammed(w, id) {
        return;
    }
    // A ship in warp cannot be locked (WARP-LOCK #191).
    if crate::targeting::in_active_warp(w, target) {
        return;
    }
    if !w.locks_mut().add(id, target) {
        return;
    }
    let delay = crate::targeting::acquisition_delay(w, id, target);
    if let Some(victim) = victim_character(w, target) {
        w.schedule(
            delay,
            crate::targeting::TargetChange {
                to: vec![victim],
                what: crate::targeting::What::OtherAdd,
                ship: Some(id),
                reason: None,
            },
        );
    }
}

/// Out of the lock graph, and `'otherlost'` to the victim -- **no earlier
/// than the `'otheradd'` it undoes**, which may still be on the clock queue.
fn unlock(w: &mut Registry, npc: &Npc, target: i64, now: BlueTime) {
    if !w.locks_mut().remove(npc.id, target) {
        return;
    }
    let pending = npc.lock_ready_at.map_or(0, |r| r.saturating_sub(now));
    if let Some(victim) = victim_character(w, target) {
        w.schedule(
            duration(pending.max(0)),
            crate::targeting::TargetChange {
                to: vec![victim],
                what: crate::targeting::What::OtherLost,
                ship: Some(npc.id),
                reason: Some(crate::targeting::Reason::Unlocked),
            },
        );
    }
}

/// The gun's visual, to everyone in the system.
///
/// **`module_id` is the NPC's own ball id -- [V], and the one trap here.**
/// `EntityShip.FitHardpoints` fits the entity's turret from its
/// `gfxTurretID` and keeps it at `self.modules[self.id]`
/// (`entityShip.py:47`, `:136-138`); `StandardWeapon.PopulateTurrets` looks
/// it up as `shipBall.modules.get(self.moduleID)` (`turrets.py:96`). Any
/// other id is an empty turret list and no tracer, in silence.
/// `moduleTypeID` is only logged by `fxSequencer.OnSpecialFX:500-534` [V],
/// so the hull's type there is harmless. `is_offensive` is what makes the
/// victim's client set `threatAttackingMe` (`tactical.py:434`) [V].
fn fx(w: &mut Registry, npc: &Npc, target: i64, start: bool) {
    let to = audience(w, npc.solar_system_id);
    if to.is_empty() {
        return;
    }
    let spool = Behaviour::of(w, npc.kind.type_id).spool;
    w.schedule(Duration::ZERO, gun_fx(to, npc, target, start, spool));
}

/// The `OnSpecialFX` an NPC's gun sends.
///
/// A `TriglavianBeam` gun gets the [`crate::modules::TriglavianFx`] form; its
/// `graphicInfo` carries the type's spool pair, or the dogma defaults
/// `(0.0, 0.5)` for a type with none [V-data].
pub(super) fn gun_fx(
    to: Vec<i64>,
    npc: &Npc,
    target: i64,
    start: bool,
    spool: Option<(f64, f64)>,
) -> crate::modules::GunFx {
    let fx = crate::modules::SpecialFx {
        to,
        ship_id: npc.id,
        module_id: npc.id,
        type_id: npc.kind.type_id,
        target_id: Some(target),
        guid: npc.kind.weapon_guid,
        is_offensive: true,
        start,
        duration_ms: npc.kind.weapon_period_ms,
        // What the client's own `dogmaIM.Activate` asks for a gun.
        repeat: 1000,
    };
    if fx.guid == crate::pochven::spool::BEAM_GUID {
        let (per_cycle, max) = spool.unwrap_or((0.0, 0.5));
        crate::modules::GunFx::Beam(crate::modules::TriglavianFx { fx, per_cycle, max })
    } else {
        crate::modules::GunFx::Plain(fx)
    }
}

/// The repair beam of an assist cycle (PV-6, #228), to everyone in the system.
///
/// **One start per ally, not per cycle.** The client keeps a running beam
/// by (guid, ship, module, target) and `Activation.AddTrigger` drops a start
/// whose `moduleID` it already holds (`fxSequencer.py:95-105` [V]), so a start
/// every cycle would not extend it: the activation would expire two durations
/// after the first and the beam would blink. So the first cycle on an ally
/// sends a start with the gun's long `repeat`, the cycles after it only renew
/// the beam's `until` in the NPC's [`Mind`], and [`Intent::AssistEnd`] sends the
/// stop when the repairs end, switch ally, or a cycle does not run. A dead or
/// departed NPC's ball removal ends its activations on the client
/// (`DoBallRemove` -> `RemoveAllBallActivations`, `fxSequencer.py:521-522` [V]).
///
/// Resolved from the stored effect id ([`Assist::effect_id`]) and its typedata
/// guid; the cycle in milliseconds is the stored period. A type with no such
/// guid sends nothing.
fn assist_fx(
    w: &mut Registry,
    npc: &Npc,
    effect_id: i64,
    period: BlueTime,
    ally: i64,
    now: BlueTime,
) {
    let Some(td) = w.content().try_get::<crate::typedata::Installed>() else {
        return;
    };
    let Some(guid) = td.table().effect(effect_id).and_then(|e| rep_guid(&e.guid)) else {
        return;
    };
    // Whole ms, as the old `period_ms as i64`.
    let period_ms = period / BLUE_PER_MS;
    let until = now.saturating_add(period).saturating_add(ASSIST_BEAM_SLACK);
    let Some(live) = w.npcs_mut().get_mut(npc.id) else {
        return;
    };
    let beams = &mut live.mind.assist_beams;
    if let Some(b) = beams.iter_mut().flatten().find(|b| b.ally == ally) {
        b.until = until;
        return;
    }
    let Some(slot) = beams.iter_mut().find(|s| s.is_none()) else {
        return;
    };
    *slot = Some(AssistBeam {
        ally,
        until,
        guid,
        period_ms,
    });
    let to = audience(w, npc.solar_system_id);
    if !to.is_empty() {
        w.schedule(
            Duration::ZERO,
            assist_beam(to, npc, ally, guid, period_ms, true),
        );
    }
}

/// The running repair beams of `npc` as a late arrival's replay (`to` empty,
/// [`crate::modules::active_fx`]): the start each was sent with.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn running_assist_fx(npc: &Npc, now: BlueTime) -> Vec<crate::modules::SpecialFx> {
    npc.mind
        .assist_beams
        .iter()
        .flatten()
        .filter(|b| now <= b.until)
        .map(|b| assist_beam(Vec::new(), npc, b.ally, b.guid, b.period_ms, true))
        .collect()
}

/// The `OnSpecialFX` of a remote-repair beam: from the repairer to its ally.
///
/// - **`ship_id` = `module_id` = the NPC's ball.** The client keys the beam by ship,
///   module, target and guid (`fxSequencer.py:526-539` [V]) and needs both balls in
///   the park (`:407-413`, `:417-421` [V]).
/// - **`type_id` 0.** `EveStretch3._SetRemoteModuleType` has two branches: with a
///   truthy `moduleTypeID` it reads that *module* type's group, and a hull is in
///   none of the repair groups; with 0 it takes the guid's place in
///   `[RemoteArmourRepair, ShieldTransfer]` (`EveStretch3.py:67-87` [V]). That branch
///   has no `+ 1`, so the look is `0` for the armour guid (the same value as "no
///   module") and `1` for the shield guid, which the module branch numbers as armour
///   [V code path]. The armour guid drawing the armour look is [I]; the shield guid
///   drawing the shield look is [A], falsified by a shield repairer that shows the
///   armour-coloured beam. Kept at 0 because a typed path would need a module type
///   the NPC does not have.
/// - **`is_offensive` false**: an ally is helped, not attacked.
/// - **`repeat` 1000 [A]**, as the gun's (`gun_fx`) and the e-war beams'
///   (`npc_ewar::fx`): the start runs until the stop. The client purges the
///   effect after about 1001 cycles (about 100 min at 6 s), so a beam renewed
///   for longer than that blinks off once, as the guns and e-war do.
///   Falsifier: a beam that ends by itself within a few cycles.
pub(super) fn assist_beam(
    to: Vec<i64>,
    npc: &Npc,
    ally: i64,
    guid: &'static str,
    period_ms: i64,
    start: bool,
) -> crate::modules::SpecialFx {
    crate::modules::SpecialFx {
        to,
        ship_id: npc.id,
        module_id: npc.id,
        type_id: 0,
        target_id: Some(ally),
        guid,
        is_offensive: false,
        start,
        duration_ms: period_ms,
        repeat: 1000,
    }
}

/// A repairing effect's guid as the client resolves it: the part before the first
/// comma ([`crate::pochven::spool::first_guid`]), interned because an
/// `OnSpecialFX` holds a `&'static str`. `None` for an effect with no guid.
fn rep_guid(guid: &str) -> Option<&'static str> {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let g = crate::pochven::spool::first_guid(guid);
    if g.is_empty() {
        return None;
    }
    let mut seen = SEEN.get_or_init(Default::default).lock().ok()?;
    if let Some(known) = seen.iter().find(|k| **k == g) {
        return Some(*known);
    }
    let leaked: &'static str = Box::leak(g.to_owned().into_boxed_str());
    seen.push(leaked);
    Some(leaked)
}

/// One volley through P2.2b's own path, so the victim's bars, rings and
/// combat log move exactly as they do in PvP.
fn shoot(w: &mut Registry, npc: &Npc, target: i64, at: BlueTime, now: BlueTime, mult: f64) {
    // An Upwell structure under siege (`/entity siege`): the volley goes
    // through the structure's own path, which takes the bars, tells the
    // owner and tallies the killmail. **[A]** full volleys, no tracking:
    // falsified by a TQ structure that is missed by a rat; the gun's
    // disruptors do not apply to a fixed target.
    if w.roster().by_ship(target).is_none() && crate::structure_combat::is_target(w, target) {
        let volley = crate::drone_skills::scaled(npc.kind.volley(), mult);
        crate::structure_combat::hit(
            w,
            crate::structure_combat::Volley {
                shooter: npc.id,
                attacker_ship: npc.id,
                weapon_type: npc.kind.type_id,
                target,
                damage: volley,
                hit_quality: crate::combat::HIT_QUALITY,
                banked: false,
                at,
            },
            now,
        );
        return;
    }
    let Some(victim) = w.roster().by_ship(target).copied() else {
        return;
    };
    if victim.solar_system_id != npc.solar_system_id {
        return;
    }
    // The turret formula on the rat's own gun (`combat::entity_chance`),
    // with a tracking disruptor's factors when one holds it (#137): a rat
    // misses and grazes as a player's turret does [V-TQ].
    let verdict = w
        .roster()
        .get(npc.id)
        .and_then(|me| crate::combat::entity_gun_verdict(w, me, npc.kind.type_id, &victim, at));
    let volley = crate::drone_skills::scaled(npc.kind.volley(), mult);
    let (damage, hit_quality) = match verdict {
        Some(v) => (crate::drone_skills::scaled(volley, v.multiplier), v.quality),
        None => (volley, crate::combat::HIT_QUALITY),
    };
    let shot = Shot {
        at,
        attacker_ship_id: npc.id,
        weapon_item_id: npc.id,
        weapon_type_id: npc.kind.type_id,
        target_ship_id: target,
        damage,
        hit_quality,
    };
    crate::combat::fire(w, shot, npc.id, victim, now);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc::ANGEL_HIJACKER;
    use crate::npc::tests::{ALICE_SHIP, BOB_SHIP, idle};

    fn candidate(ship_id: i64, at: [f64; 3]) -> Candidate {
        Candidate {
            ship_id,
            position: at,
            is_capsule: false,
            locking_me: false,
            lock_delay: 0,
            signature: 0.0,
            aggressor: false,
            threat: 0.0,
        }
    }

    fn view(candidates: Vec<Candidate>, lock_held: bool, now: BlueTime) -> View {
        View {
            me: [0.0; 3],
            candidates,
            allies: Vec::new(),
            lock_held,
            lock_range: 0.0,
            now,
            behaviour: Behaviour::default(),
        }
    }

    /// An NPC already fighting `ALICE_SHIP`, its lock ready, gun running.
    fn firing_at_alice(next_shot_at: BlueTime) -> Npc {
        Npc {
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(0),
            firing: true,
            next_shot_at,
            ..idle()
        }
    }

    // ---- §8.1 test 5: `decide`, pure ---------------------------------------

    #[test]
    fn an_idle_npc_with_nothing_to_see_does_nothing() {
        let (n, out) = decide(&idle(), &view(vec![], false, 1));
        assert_eq!(n, idle());
        assert!(out.is_empty());
    }

    #[test]
    fn an_idle_npc_engages_a_ship_inside_its_aggression_range() {
        let c = Candidate {
            lock_delay: 7,
            ..candidate(ALICE_SHIP, [AGGRESSION_RANGE / 2.0, 0.0, 0.0])
        };
        let (n, out) = decide(&idle(), &view(vec![c], false, 100));
        assert_eq!(
            out,
            vec![Intent::Lock(ALICE_SHIP), Intent::Follow(ALICE_SHIP)]
        );
        assert_eq!(n.target, Some(ALICE_SHIP));
        assert_eq!(n.lock_ready_at, Some(107));
        assert_eq!(n.phase, Phase::Approach);
        assert!(!n.firing, "not before the lock is ready");
    }

    /// The cache guard is for caches only: a zero-damage NPC of any other
    /// group (an unlisted hauler, a missile-only or remote-rep type that has
    /// the default profile) still acquires a target and flies.
    #[test]
    fn a_zero_damage_npc_that_is_not_a_cache_still_acquires_and_moves() {
        let mut k = crate::npc::FIXTURE_RAT;
        k.damage = crate::damage::Damage::default();
        let kind: &'static crate::npc::NpcType = Box::leak(Box::new(k));
        assert_ne!(kind.group_id, crate::abyss::loot::GROUP_CACHE);
        let npc = Npc { kind, ..idle() };
        let c = candidate(ALICE_SHIP, [AGGRESSION_RANGE / 2.0, 0.0, 0.0]);
        let (n, out) = decide(&npc, &view(vec![c], false, 100));
        assert_eq!(n.target, Some(ALICE_SHIP));
        assert!(out.contains(&Intent::Lock(ALICE_SHIP)));
        assert!(out.contains(&Intent::Follow(ALICE_SHIP)), "it moves");
    }

    #[test]
    fn a_ship_beyond_aggression_range_is_left_alone() {
        let c = candidate(ALICE_SHIP, [AGGRESSION_RANGE * 1.01, 0.0, 0.0]);
        let (_, out) = decide(&idle(), &view(vec![c], false, 1));
        assert!(out.is_empty());
    }

    #[test]
    fn a_capsule_is_not_a_target() {
        let c = Candidate {
            is_capsule: true,
            locking_me: true,
            ..candidate(ALICE_SHIP, [AGGRESSION_RANGE / 2.0, 0.0, 0.0])
        };
        let (n, out) = decide(&idle(), &view(vec![c], false, 1));
        assert!(out.is_empty());
        assert_eq!(n.target, None);
    }

    /// Retaliation reaches past the aggression range, but not past the
    /// NPC's own lock range, where the sweep would drop the lock.
    #[test]
    fn a_ship_that_locks_the_npc_is_engaged_from_further_out() {
        let k = &ANGEL_HIJACKER;
        let between = (AGGRESSION_RANGE + k.hull.max_target_range) / 2.0;
        let c = Candidate {
            locking_me: true,
            ..candidate(ALICE_SHIP, [between, 0.0, 0.0])
        };
        let (n, out) = decide(&idle(), &view(vec![c], false, 1));
        assert_eq!(n.target, Some(ALICE_SHIP));
        assert_eq!(out[0], Intent::Lock(ALICE_SHIP));
        let beyond = Candidate {
            position: [k.hull.max_target_range * 1.01, 0.0, 0.0],
            ..c
        };
        let (_, out) = decide(&idle(), &view(vec![beyond], false, 1));
        assert!(out.is_empty());
    }

    #[test]
    fn the_nearest_ship_is_the_one_engaged() {
        let far = candidate(BOB_SHIP, [AGGRESSION_RANGE * 0.9, 0.0, 0.0]);
        let near = candidate(ALICE_SHIP, [AGGRESSION_RANGE * 0.5, 0.0, 0.0]);
        let (n, _) = decide(&idle(), &view(vec![far, near], false, 1));
        assert_eq!(n.target, Some(ALICE_SHIP));
    }

    #[test]
    fn a_lock_that_is_gone_stops_the_gun_and_is_taken_again_while_the_target_is_in_view() {
        let npc = firing_at_alice(10);
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (n, out) = decide(&npc, &view(vec![c], false, 5));
        assert_eq!(
            out,
            vec![Intent::FireStop(ALICE_SHIP), Intent::Lock(ALICE_SHIP)]
        );
        assert_eq!((n.target, n.firing), (Some(ALICE_SHIP), false));
        // Its target gone from view, the engagement ends.
        let (n, out) = decide(&npc, &view(vec![], false, 5));
        assert_eq!(out, vec![Intent::FireStop(ALICE_SHIP), Intent::Stop]);
        assert_eq!((n.phase, n.target, n.firing), (Phase::Idle, None, false));
    }

    #[test]
    fn a_target_that_became_a_capsule_is_let_go() {
        let npc = firing_at_alice(10);
        let c = Candidate {
            is_capsule: true,
            ..candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0])
        };
        let (_, out) = decide(&npc, &view(vec![c], true, 5));
        assert_eq!(
            out,
            vec![
                Intent::FireStop(ALICE_SHIP),
                Intent::Unlock(ALICE_SHIP),
                Intent::Stop
            ]
        );
    }

    fn spooling_view(c: Candidate, now: BlueTime) -> View {
        let mut v = view(vec![c], true, now);
        v.behaviour.spool = Some((0.25, 1.0));
        v
    }

    fn mults(out: &[Intent]) -> Vec<f64> {
        out.iter()
            .filter_map(|i| match i {
                Intent::Shoot { mult, .. } => Some(*mult),
                _ => None,
            })
            .collect()
    }

    /// PV-5: the spool pair is read from the type's own attributes (invented
    /// rows here), and a type with a zero per-cycle value has no ramp.
    #[test]
    fn the_behaviour_of_a_spooling_type_carries_its_per_cycle_and_max() {
        let table = crate::typedata::Table::parse(
            "attribute\t2733\t0.0\tper\nattribute\t2734\t0.5\tmax\n\
type\t990305001\t99111\t11\t0\t1\t10.0\t0.0\t1.0\t1.0\t0\t\t2733=0.125;2734=2.0\tSpooler\n\
type\t990305002\t99111\t11\t0\t1\t10.0\t0.0\t1.0\t1.0\t0\t\t\tPlain\n",
        )
        .expect("the fixture parses");
        assert_eq!(spool_of(&table, 990_305_001), Some((0.125, 2.0)));
        assert_eq!(spool_of(&table, 990_305_002), None, "default 0 is no ramp");
        assert_eq!(Behaviour::default().spool, None);
    }

    /// PV-5: a type with spool numbers but no `npc_data` profile still ramps; a
    /// type with neither does not.
    #[test]
    fn a_spooling_type_without_a_profile_still_carries_its_spool() {
        let table = crate::typedata::Table::parse(
            "attribute\t2733\t0.0\tper\nattribute\t2734\t0.5\tmax\n\
type\t990305001\t99111\t11\t0\t1\t10.0\t0.0\t1.0\t1.0\t0\t\t2733=0.125;2734=2.0\tSpooler\n\
type\t990305002\t99111\t11\t0\t1\t10.0\t0.0\t1.0\t1.0\t0\t\t\tPlain\n",
        )
        .expect("the fixture parses");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let reg = crate::npc::tests::world(&[crate::npc::tests::ALICE])
            .with_content(std::sync::Arc::new(content));
        assert_eq!(
            Behaviour::of(&reg, 990_305_001).spool,
            Some((0.125, 2.0)),
            "the type's own numbers"
        );
        assert_eq!(Behaviour::of(&reg, 990_305_002).spool, None);
    }

    /// PV-5: each cycle on one target hits harder, up to the cap.
    #[test]
    fn a_spooling_npc_shoots_harder_each_cycle_on_one_target() {
        let p = ANGEL_HIJACKER.weapon_period();
        let t = 1_000_000;
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (n, out) = decide(&firing_at_alice(t), &spooling_view(c, t + 2 * p));
        let first = mults(&out);
        assert_eq!(first.len(), 3);
        assert!(
            first.windows(2).all(|w| (w[1] - w[0] - 0.25).abs() < 1e-12),
            "each cycle steps by per_cycle"
        );
        let (_, out) = decide(&n, &spooling_view(c, t + 6 * p));
        let m = mults(&out);
        assert!(
            (m[0] - first[2] - 0.25).abs() < 1e-12,
            "the ramp carries over a tick"
        );
        assert!(m.iter().all(|x| *x <= 2.0), "capped at 1 + max");
        assert_eq!(*m.last().unwrap(), 2.0);
    }

    /// PV-5: a type with no ramp always shoots at 1.0.
    #[test]
    fn a_type_without_a_ramp_shoots_at_one() {
        let p = ANGEL_HIJACKER.weapon_period();
        let t = 1_000_000;
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (_, out) = decide(&firing_at_alice(t), &view(vec![c], true, t + 2 * p));
        assert_eq!(mults(&out), vec![1.0, 1.0, 1.0]);
    }

    /// PV-5: a ramp built on another target starts again at the new one.
    #[test]
    fn switching_target_resets_the_multiplier() {
        let t = 1_000_000;
        let mut npc = firing_at_alice(t);
        npc.mind.spool = crate::pochven::spool::Spool {
            target: Some(BOB_SHIP),
            current: 0.75,
        };
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (n, out) = decide(&npc, &spooling_view(c, t));
        let fresh = crate::pochven::spool::Spool::default().cycle(ALICE_SHIP, 0.25, 1.0);
        assert_eq!(mults(&out), vec![fresh]);
        assert_eq!(n.mind.spool.target, Some(ALICE_SHIP));
    }

    /// One volley per period, each on its own boundary.
    #[test]
    fn volleys_land_on_their_period_boundaries() {
        let p = ANGEL_HIJACKER.weapon_period();
        let t = 1_000_000;
        let npc = firing_at_alice(t);
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (n, out) = decide(&npc, &view(vec![c], true, t + 2 * p));
        let shots: Vec<_> = out
            .iter()
            .filter_map(|i| match i {
                Intent::Shoot { target, at, .. } => Some((*target, *at)),
                _ => None,
            })
            .collect();
        assert_eq!(
            shots,
            vec![
                (ALICE_SHIP, t),
                (ALICE_SHIP, t + p),
                (ALICE_SHIP, t + 2 * p)
            ]
        );
        assert_eq!(n.next_shot_at, t + 3 * p);
    }

    #[test]
    fn a_stalled_clock_is_caught_up_a_little_and_not_all_at_once() {
        let p = ANGEL_HIJACKER.weapon_period();
        let npc = firing_at_alice(0);
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let now = 100 * p;
        let (n, out) = decide(&npc, &view(vec![c], true, now));
        let shots = out
            .iter()
            .filter(|i| matches!(i, Intent::Shoot { .. }))
            .count();
        assert_eq!(shots, MAX_SHOTS_PER_TICK as usize);
        assert!(n.next_shot_at > now);
        assert_eq!(n.next_shot_at % p, 0, "still on the period grid");
    }

    #[test]
    fn nothing_fires_before_the_lock_is_ready() {
        let npc = Npc {
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(50),
            ..idle()
        };
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let (n, out) = decide(&npc, &view(vec![c], true, 49));
        assert!(out.is_empty(), "{out:?}");
        let (n2, out) = decide(&n, &view(vec![c], true, 50));
        assert_eq!(
            out,
            vec![
                Intent::FireStart(ALICE_SHIP),
                Intent::Shoot {
                    target: ALICE_SHIP,
                    at: 50,
                    mult: 1.0,
                }
            ]
        );
        assert!(n2.firing);
    }

    /// #137: a tracking disruptor on the rat cuts the range it opens fire at;
    /// without one it opens fire where it always did.
    #[test]
    fn a_disrupted_rat_opens_fire_closer_in() {
        let npc = Npc {
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(0),
            ..idle()
        };
        let x = ANGEL_HIJACKER.attack_range * 0.8;
        let c = candidate(ALICE_SHIP, [x, 0.0, 0.0]);
        let fires = |reach: Option<f64>| {
            let mut v = view(vec![c], true, 50);
            v.behaviour.gun_reach = reach;
            let (_, out) = decide(&npc, &v);
            out.contains(&Intent::FireStart(ALICE_SHIP))
        };
        assert!(fires(None));
        assert!(fires(Some(1.0)));
        // Only the direction is pinned, not the size of the cut [A].
        assert!(!fires(Some(0.1)), "a strong disruptor shortens the reach");
        assert!(fires(Some(10.0)), "a tracking computer lengthens it");
    }

    /// #137: a rat whose reach was cut closes in to orbit inside it, rather
    /// than circling outside the gun's range.
    #[test]
    fn a_disrupted_rat_orbits_inside_its_cut_reach() {
        let npc = Npc {
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(0),
            ..idle()
        };
        // The edge where an undisrupted rat still holds its orbit.
        let c = candidate(ALICE_SHIP, [latch_edges(&ANGEL_HIJACKER).1, 0.0, 0.0]);
        let follows = |reach: Option<f64>| {
            let mut v = view(vec![c], true, 50);
            v.behaviour.gun_reach = reach;
            decide(&npc, &v).1.contains(&Intent::Follow(ALICE_SHIP))
        };
        assert!(follows(Some(0.1)), "out of the cut reach: close in");
        assert!(!follows(None), "undisrupted, the orbit is as it was");
    }

    /// Where the latch hands over, for `k`'s orbit range.
    fn latch_edges(k: &crate::npc::NpcType) -> (f64, f64) {
        let o = k.orbit_range;
        (
            o + crate::npc::ORBIT_SETTLE_MIN.max(o * crate::npc::ORBIT_SETTLE_FRACTION),
            o + crate::npc::ORBIT_REENTER_MIN.max(o * crate::npc::ORBIT_REENTER_FRACTION),
        )
    }

    #[test]
    fn approach_turns_to_orbit_inside_the_settle_gap_and_back_outside_the_reenter_gap() {
        let k = &ANGEL_HIJACKER;
        let (settle, reenter) = latch_edges(k);
        let at = |x: f64| view(vec![candidate(ALICE_SHIP, [x, 0.0, 0.0])], true, 1);
        let approaching = Npc {
            phase: Phase::Approach,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(i64::MAX),
            ..idle()
        };
        // Between the two edges a follower keeps following.
        let mid = (settle + reenter) / 2.0;
        let (n, out) = decide(&approaching, &at(mid));
        assert!(out.is_empty());
        assert_eq!(n.phase, Phase::Approach);
        // Inside the settle gap it orbits, once.
        let (n, out) = decide(&approaching, &at(settle));
        assert_eq!(out, vec![Intent::Orbit(ALICE_SHIP)]);
        // And an orbiter between the edges keeps orbiting; past the re-enter
        // gap it follows again.
        let (n, out) = decide(&n, &at(mid));
        assert!(out.is_empty());
        let (_, out) = decide(&n, &at(reenter * 1.01));
        assert_eq!(out, vec![Intent::Follow(ALICE_SHIP)]);
    }

    /// **No churn across the boundary**: a target that wobbles around either
    /// edge of the latch, every tick for a minute, draws at most the one
    /// order per real crossing (owner 2026-10-28: erratic up-and-down
    /// movement while fighting).
    #[test]
    fn the_follow_orbit_latch_does_not_churn_at_its_edges() {
        let k = &ANGEL_HIJACKER;
        let (settle, reenter) = latch_edges(k);
        for (edge, allowed) in [(settle, 0), (reenter, 1)] {
            let mut n = Npc {
                phase: Phase::Orbit,
                target: Some(ALICE_SHIP),
                lock_ready_at: Some(i64::MAX),
                ..idle()
            };
            let mut orders = 0;
            for tick in 0..240 {
                let x = edge + if tick % 2 == 0 { 150.0 } else { -150.0 };
                let v = view(vec![candidate(ALICE_SHIP, [x, 0.0, 0.0])], true, tick);
                let (next, out) = decide(&n, &v);
                orders += out
                    .iter()
                    .filter(|i| matches!(i, Intent::Follow(_) | Intent::Orbit(_) | Intent::Stop))
                    .count();
                n = next;
            }
            // At the settle edge an orbiter never leaves orbit; at the
            // re-enter edge it follows once when it first crosses, then
            // keeps following (its gap stays above the settle edge).
            assert_eq!(
                orders, allowed,
                "a target wobbling at {edge} m: {orders} orders"
            );
        }
        // A follower wobbling at the settle edge hands over once, and stays.
        let mut n = Npc {
            phase: Phase::Approach,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(i64::MAX),
            ..idle()
        };
        let mut orders = 0;
        for tick in 0..240 {
            let x = settle + if tick % 2 == 0 { 150.0 } else { -150.0 };
            let v = view(vec![candidate(ALICE_SHIP, [x, 0.0, 0.0])], true, tick);
            let (next, out) = decide(&n, &v);
            orders += out.len();
            n = next;
        }
        assert_eq!(orders, 1, "one hand-over, then quiet");
        assert_eq!(n.phase, Phase::Orbit);
    }

    /// **A fleeing target is chased until the leash**: its lock gone to range
    /// (the sweep took it), it is still followed, not let go; a lock is
    /// retaken once it is inside the lock range; past [`PURSUIT_RANGE`] the
    /// NPC gives up; past the leash it flies home.
    #[test]
    fn a_target_that_opens_range_is_chased_until_pursuit_range_or_the_leash() {
        let lock_range = 25_000.0;
        let far = |x: f64, home_gap: f64, lock_held: bool| View {
            me: [home_gap, 0.0, 0.0],
            lock_range,
            ..view(
                vec![candidate(ALICE_SHIP, [home_gap + x, 0.0, 0.0])],
                lock_held,
                1,
            )
        };
        let fighting = Npc {
            mind: Mind::at([0.0; 3]),
            ..firing_at_alice(i64::MAX)
        };
        // Beyond lock range: no unlock, no stop; the gun stops and it follows.
        let (n, out) = decide(&fighting, &far(30_000.0, 0.0, false));
        assert_eq!(n.target, Some(ALICE_SHIP), "{out:?}");
        assert!(
            !out.iter()
                .any(|i| matches!(i, Intent::Unlock(_) | Intent::Stop))
        );
        assert!(out.contains(&Intent::FireStop(ALICE_SHIP)));
        assert!(out.contains(&Intent::Follow(ALICE_SHIP)));
        assert!(!out.contains(&Intent::Lock(ALICE_SHIP)), "not yet in reach");
        assert!(!n.firing);
        // Still opening, nothing new to say.
        let (n, out) = decide(&n, &far(31_000.0, 100.0, false));
        assert!(out.is_empty(), "{out:?}");
        // Closed to inside the lock range: it locks again.
        let (n, out) = decide(&n, &far(lock_range * 0.5, 200.0, false));
        assert!(out.contains(&Intent::Lock(ALICE_SHIP)), "{out:?}");
        assert!(n.lock_ready_at.is_some());
        // Past the pursuit range: it gives up.
        let (n, out) = decide(&fighting, &far(PURSUIT_RANGE * 1.1, 0.0, false));
        assert_eq!(out, vec![Intent::FireStop(ALICE_SHIP), Intent::Stop]);
        assert!(n.target.is_none());
        // Past the leash from home, it flies home even with the lock gone.
        let b = fixture_behaviour();
        let v = View {
            behaviour: b,
            ..far(30_000.0, b.leash() * 1.01, false)
        };
        let (n, out) = decide(&fighting, &v);
        assert!(out.contains(&Intent::Return([0.0; 3])), "{out:?}");
        assert!(n.mind.returning);
    }

    /// A type's 665 below [`HOME_RADIUS`] is no leash at that length: the
    /// capture's 3,600 m frigate cycled Follow/Return/Stop at 750 ms.
    #[test]
    fn a_small_665_leashes_no_closer_than_the_floor() {
        let small = Behaviour {
            chase_max: 3_600.0,
            ..Behaviour::default()
        };
        assert_eq!(small.leash(), LEASH_FLOOR);
        let huge = Behaviour {
            chase_max: LEASH_FLOOR * 2.0,
            ..Behaviour::default()
        };
        assert_eq!(huge.leash(), LEASH_FLOOR * 2.0);
        assert_eq!(Behaviour::default().leash(), 0.0, "0 is no leash");
        // A rat 10 km from home fighting a target keeps fighting.
        let npc = Npc {
            mind: Mind::at([0.0; 3]),
            ..firing_at_alice(i64::MAX)
        };
        let v = View {
            me: [10_000.0, 0.0, 0.0],
            behaviour: small,
            ..view(vec![candidate(ALICE_SHIP, [14_000.0, 0.0, 0.0])], true, 1)
        };
        let (n, out) = decide(&npc, &v);
        assert!(
            !out.iter().any(|i| matches!(i, Intent::Return(_))),
            "{out:?}"
        );
        assert!(!n.mind.returning);
    }

    /// **Idle rats roam near home**: over ten minutes an idle rat with a
    /// home picks several points, every one inside [`ROAM_RADIUS`] of home
    /// and in the home's plane; none is picked at once, and a rat with no
    /// home (or of the law) never moves.
    #[test]
    fn idle_rats_roam_within_a_radius_of_home() {
        let home = [1_000.0, 50.0, -2_000.0];
        let mut n = Npc {
            mind: Mind::at(home),
            ..idle()
        };
        let second = 1_000 * BLUE_PER_MS;
        let mut points = Vec::new();
        for t in 0..600 {
            let v = View {
                me: home,
                ..view(vec![], false, t * second)
            };
            let (next, out) = decide(&n, &v);
            if t == 0 {
                assert!(out.is_empty(), "no roam on the first look: {out:?}");
            }
            for i in out {
                match i {
                    Intent::Roam(p) => points.push(p),
                    other => panic!("an idle rat only roams: {other:?}"),
                }
            }
            n = next;
        }
        assert!(points.len() >= 8, "{} roams in ten minutes", points.len());
        assert!(
            points.len() <= 30,
            "{} roams in ten minutes is a rat that never settles",
            points.len()
        );
        for p in &points {
            let flat = ((p[0] - home[0]).powi(2) + (p[2] - home[2]).powi(2)).sqrt();
            assert!(flat <= ROAM_RADIUS + 1e-6, "{flat} m from home");
            assert_eq!(p[1], home[1], "no vertical wander");
        }
        let distinct = points.iter().filter(|p| **p != points[0]).count();
        assert!(distinct > 0, "the points differ");
        // No home, no roam.
        let mut homeless = idle();
        homeless.mind.home = None;
        for t in 0..600 {
            let (next, out) = decide(&homeless, &view(vec![], false, t * second));
            assert!(out.is_empty());
            homeless = next;
        }
    }

    // ---- CP23: the type's own behaviour (C6 R8, N4) ----------------------

    /// Invented typedata for `npc_data::Table::fixture()`'s turret frigate
    /// 990300001: first-shot window 475/476, leash 665, switch 1648/1416,
    /// signature 1650, shield repairer 637/636/639 (as `npc_data`'s own
    /// test row).
    const TYPEDATA: &str = "\
attribute\t114\t0.0\temDamage
type\t990300001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t10\t114=2.0;475=1000.0;476=3000.0;481=99.0;636=5000.0;637=20.0;639=0.25;665=40000.0;1416=15000.0;1648=1.0;1650=1.0\tFixture A
";

    fn profile() -> crate::npc_data::Profile {
        let td = crate::typedata::Table::parse(TYPEDATA).expect("the invented typedata parses");
        crate::npc_data::Table::fixture()
            .profile(&td, 990_300_001)
            .expect("a profile")
    }

    fn fixture_behaviour() -> Behaviour {
        Behaviour::from_profile(&profile())
    }

    fn with(b: Behaviour, v: View) -> View {
        View { behaviour: b, ..v }
    }

    /// Every number comes from the type's own attribute, in blue ticks.
    #[test]
    fn a_profile_becomes_its_behaviour() {
        let p = profile();
        let b = Behaviour::from_profile(&p);
        let ms = BLUE_PER_MS;
        assert_eq!(
            b.attack_delay,
            (
                p.attack_delay_ms.0 as BlueTime * ms,
                p.attack_delay_ms.1 as BlueTime * ms
            )
        );
        assert_eq!(b.chase_max, p.chase_max);
        assert_eq!(b.switch_every, Some(p.switch_timer_ms as BlueTime * ms));
        assert!(b.prefer_signature);
        let (r, pr) = (b.self_rep.expect("a repairer"), p.self_rep.expect("rep"));
        assert_eq!(
            (r.armor, r.amount, r.period, r.delay_chance),
            (
                pr.armor,
                pr.amount,
                pr.period_ms as BlueTime * ms,
                pr.delay_chance
            )
        );
        // The remote repairer's numbers, effect id included, come from the profile.
        let rr = crate::npc_data::RemoteRep {
            effect_id: 990_003,
            armor: true,
            amount: 11.0,
            period_ms: 4_000.0,
            chance: 0.5,
            threshold: 0.6,
            range: 9_000.0,
            max_targets: 2,
        };
        let with_rr = crate::npc_data::Profile {
            remote_rep: Some(rr),
            ..p
        };
        let a = Behaviour::from_profile(&with_rr)
            .assist
            .expect("a remote repairer");
        assert_eq!(
            a,
            Assist {
                effect_id: 990_003,
                armor: true,
                amount: 11.0,
                period: 4_000 * ms,
                chance: 0.5,
                threshold: 0.6,
                range: 9_000.0,
                max_targets: 2,
            }
        );
        let off = crate::npc_data::Profile {
            target_switch: false,
            self_rep: None,
            ..p
        };
        let b = Behaviour::from_profile(&off);
        assert_eq!((b.switch_every, b.self_rep), (None, None));
    }

    /// **No `Shoot` before the type's own delay attribute**: nothing fires
    /// in the first `attack_delay.0` after the lock completes, and the gun
    /// has started by `attack_delay.1`.
    #[test]
    fn no_shot_before_the_types_own_first_shot_delay() {
        let b = fixture_behaviour();
        let ready = 1_000_000;
        let npc = Npc {
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(ready),
            ..idle()
        };
        let c = candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let step = (b.attack_delay.0 / 10).max(1);
        let mut now = ready;
        while now < ready + b.attack_delay.0 {
            let (_, out) = decide(&npc, &with(b, view(vec![c], true, now)));
            assert!(out.is_empty(), "at +{}: {out:?}", now - ready);
            now += step;
        }
        let mut n = npc;
        let mut first = None;
        let mut now = ready;
        while now <= ready + b.attack_delay.1 && first.is_none() {
            let (next, out) = decide(&n, &with(b, view(vec![c], true, now)));
            first = out.iter().find_map(|i| match i {
                Intent::Shoot { at, .. } => Some(*at),
                _ => None,
            });
            n = next;
            now += step;
        }
        let at = first.expect("the gun starts inside the window");
        assert!(at >= ready + b.attack_delay.0 && at <= ready + b.attack_delay.1);
        let drawn = first_shot_at(&npc, &b, ready);
        assert!(
            at >= drawn && at - step < drawn,
            "the first tick past the draw"
        );
    }

    /// **Retarget at the type's timer**: a better ship is ignored until the
    /// switch timer, then the old lock and gun are dropped and the new
    /// ship locked; a type that does not switch keeps its target.
    #[test]
    fn a_switching_type_retargets_at_its_own_timer() {
        let b = fixture_behaviour();
        let every = b.switch_every.expect("the fixture switches");
        let b = Behaviour {
            prefer_signature: false,
            ..b
        };
        let t0 = 1_000_000;
        let (n, _) = decide(
            &idle(),
            &with(
                b,
                view(vec![candidate(ALICE_SHIP, [5_000.0, 0.0, 0.0])], false, t0),
            ),
        );
        assert_eq!(n.mind.retarget_at, Some(t0 + every));
        let n = Npc {
            firing: true,
            phase: Phase::Orbit,
            ..n
        };
        let alice = candidate(ALICE_SHIP, [5_000.0, 0.0, 0.0]);
        let bob = candidate(BOB_SHIP, [1_000.0, 0.0, 0.0]);
        let (before, out) = decide(&n, &with(b, view(vec![alice, bob], true, t0 + every - 1)));
        assert_eq!(before.target, Some(ALICE_SHIP));
        assert!(!out.contains(&Intent::Lock(BOB_SHIP)));
        let (after, out) = decide(&n, &with(b, view(vec![alice, bob], true, t0 + every)));
        assert_eq!(after.target, Some(BOB_SHIP));
        assert_eq!(
            out[..3],
            [
                Intent::FireStop(ALICE_SHIP),
                Intent::Unlock(ALICE_SHIP),
                Intent::Lock(BOB_SHIP)
            ]
        );
        assert_eq!(after.mind.retarget_at, Some(t0 + 2 * every));
        let still = Behaviour {
            switch_every: None,
            ..b
        };
        let (kept, _) = decide(&n, &with(still, view(vec![alice, bob], true, t0 + every)));
        assert_eq!(kept.target, Some(ALICE_SHIP));
    }

    /// A type that prefers by signature takes the like-sized ship over a
    /// nearer unlike one ([`signature_fit`]); without it, the nearest.
    #[test]
    fn a_signature_preferring_type_scores_by_signature() {
        let b = fixture_behaviour();
        let mine = idle().kind.hull.signature_radius;
        let big = Candidate {
            signature: mine * 10.0,
            ..candidate(ALICE_SHIP, [1_000.0, 0.0, 0.0])
        };
        let like = Candidate {
            signature: mine,
            ..candidate(BOB_SHIP, [2_000.0, 0.0, 0.0])
        };
        let (n, _) = decide(&idle(), &with(b, view(vec![big, like], false, 1)));
        assert_eq!(n.target, Some(BOB_SHIP));
        let plain = Behaviour {
            prefer_signature: false,
            ..b
        };
        let (n, _) = decide(&idle(), &with(plain, view(vec![big, like], false, 1)));
        assert_eq!(n.target, Some(ALICE_SHIP));
        assert_eq!(signature_fit(0.0, mine), 1.0, "unknown is no preference");
    }

    /// **The leash returns the NPC past `chase_max`**: dragged further than
    /// the type's 665 from home, it drops the fight and flies home, sees
    /// nothing on the way, and stops inside [`HOME_RADIUS`].
    #[test]
    fn the_leash_returns_the_npc_past_chase_max() {
        let b = fixture_behaviour();
        let home = [0.0; 3];
        let npc = Npc {
            mind: Mind::at(home),
            ..firing_at_alice(i64::MAX)
        };
        let at = |x: f64, now| View {
            me: [x, 0.0, 0.0],
            ..with(
                b,
                view(
                    vec![candidate(ALICE_SHIP, [x + 1_000.0, 0.0, 0.0])],
                    true,
                    now,
                ),
            )
        };
        let (inside, out) = decide(&npc, &at(b.leash() * 0.99, 1));
        assert_eq!(inside.target, Some(ALICE_SHIP));
        assert!(!out.iter().any(|i| matches!(i, Intent::Return(_))));
        let (n, out) = decide(&npc, &at(b.leash() * 1.01, 1));
        assert_eq!(
            out,
            vec![
                Intent::FireStop(ALICE_SHIP),
                Intent::Unlock(ALICE_SHIP),
                Intent::Return(home)
            ]
        );
        assert!(n.mind.returning && n.target.is_none() && !n.firing);
        // On the way: the ship in aggression range is ignored.
        let (n, out) = decide(&n, &at(HOME_RADIUS * 2.0, 2));
        assert!(out.is_empty() && n.mind.returning, "{out:?}");
        let (n, out) = decide(&n, &at(HOME_RADIUS, 3));
        assert_eq!(out, vec![Intent::Stop]);
        assert!(!n.mind.returning);
        // No leash without a home, or for a type with `chase_max` 0.
        let (n, _) = decide(&firing_at_alice(i64::MAX), &at(b.leash() * 2.0, 1));
        assert_eq!(n.target, Some(ALICE_SHIP));
    }

    /// **Self-repair heals per the type's period**: with no delay chance,
    /// one cycle of the type's amount per period and none before the
    /// first; with certain delay, none.
    #[test]
    fn self_repair_heals_per_the_types_period() {
        let b = fixture_behaviour();
        let rep = b.self_rep.expect("a repairer");
        let sure = Behaviour {
            self_rep: Some(Rep {
                delay_chance: 0.0,
                ..rep
            }),
            ..b
        };
        let reps = |b: Behaviour| {
            let mut n = idle();
            let mut got = Vec::new();
            let mut now = 0;
            while now <= 5 * rep.period {
                let (next, out) = decide(&n, &with(b, view(vec![], false, now)));
                got.extend(out.into_iter().map(|i| (now, i)));
                n = next;
                now += rep.period / 4;
            }
            got
        };
        let got = reps(sure);
        assert_eq!(
            got,
            (1..=5)
                .map(|k| (
                    k * rep.period,
                    Intent::SelfRep {
                        armor: rep.armor,
                        amount: rep.amount
                    }
                ))
                .collect::<Vec<_>>()
        );
        let never = Behaviour {
            self_rep: Some(Rep {
                delay_chance: 1.0,
                ..rep
            }),
            ..b
        };
        assert!(reps(never).is_empty());
    }

    /// The executor: a cycle puts hit points back on the NPC's record and
    /// tells the system's pilots its bars.
    #[test]
    fn a_self_repair_cycle_heals_the_npcs_record() {
        use crate::npc::tests::{ALICE, due, hijacker_near, ship_of, world};
        let mut reg = world(&[ALICE]);
        let npc = hijacker_near(&mut reg, ALICE, 5_000.0);
        let alice_ship = ship_of(&reg, ALICE);
        let now = crate::pyutil::filetime_now();
        let victim = *reg.roster().get(npc).expect("in space");
        let shot = Shot {
            at: now,
            attacker_ship_id: alice_ship,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: npc,
            damage: crate::combat::volley_in(reg.content(), 3640).expect("a civilian gun"),
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(&mut reg, shot, ALICE, victim, now);
        let hit = reg.combat().ship(npc).expect("a record").shield_at(now);
        let _ = due(&mut reg);
        let me = *reg.npcs().get(npc).expect("live");
        let mut actions = Vec::new();
        let rep = Intent::SelfRep {
            armor: false,
            amount: 1.0,
        };
        apply(&mut reg, &me, rep, now, &mut actions);
        let healed = reg.combat().ship(npc).expect("a record").shield_at(now);
        assert!(healed > hit, "{hit} -> {healed}");
        let out = due(&mut reg);
        assert!(
            out.iter()
                .any(|(to, name, _)| *name == "DoDestinyUpdate" && to == &vec![ALICE])
        );
    }

    /// NEUT-NOS: a repair cycle costs the effect's `dischargeAttributeID`
    /// from the NPC's own capacitor; a rat with a dry one does not repair.
    /// Invented rows: a made-up discharge attribute (990001) on effect 2192.
    #[test]
    fn a_dry_npc_does_not_repair() {
        use crate::npc::tests::{ALICE, hijacker_near, ship_of, world};
        const ROWS: &str = "\
effect\t2192\t1\t0\t636\t990001\t0\t0\t0\tfixtureRep\t\t0
type\t990310001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t2192\t482=40.0;55=400000.0;990001=30.0;263=40.0;265=45.0;9=50.0;271=1.0;272=1.0;273=1.0;274=1.0;267=1.0;268=1.0;269=1.0;270=1.0;109=1.0;110=1.0;111=1.0;113=1.0\tFixture Raider
";
        let table = crate::typedata::Table::parse(ROWS).expect("the invented rows parse");
        let mut content = crate::content::Content::fixture();
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut reg = world(&[ALICE]).with_content(std::sync::Arc::new(content));
        let npc = hijacker_near(&mut reg, ALICE, 5_000.0);
        let alice_ship = ship_of(&reg, ALICE);
        let now = crate::pyutil::filetime_now();
        let victim = *reg.roster().get(npc).expect("in space");
        let shot = Shot {
            at: now,
            attacker_ship_id: alice_ship,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: npc,
            damage: crate::combat::volley_in(reg.content(), 3640).expect("a civilian gun"),
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(&mut reg, shot, ALICE, victim, now);
        let hit = reg.combat().ship(npc).expect("a record").shield_at(now);
        let me = *reg.npcs().get(npc).expect("live");
        let rep = Intent::SelfRep {
            armor: false,
            amount: 1.0,
        };
        let mut actions = Vec::new();
        // Full: it mends, and pays.
        apply(&mut reg, &me, rep, now, &mut actions);
        let mended = reg.combat().ship(npc).expect("a record").shield_at(now);
        assert!(mended > hit, "{hit} -> {mended}");
        let (left, _) = crate::modules::capacitor_at(&reg, npc, me.kind.type_id, now);
        assert!((left - 10.0).abs() < 1e-6, "{left}");
        // Short of the next cycle's need: nothing more mended, nothing paid.
        apply(&mut reg, &me, rep, now, &mut actions);
        assert_eq!(
            reg.combat().ship(npc).expect("a record").shield_at(now),
            mended
        );
        let (left, _) = crate::modules::capacitor_at(&reg, npc, me.kind.type_id, now);
        assert!((left - 10.0).abs() < 1e-6, "{left}");
    }

    // ---- PV-6 (#228): the remote repairer's beam ----------------------------

    /// Invented rows, no client value: a remote armour repairer (6741, the id
    /// the behaviour table keys on) whose cycle costs 30 of the 40 units of
    /// capacitor the type holds, and whose guid cell is `{guid}`. The cycle
    /// (2491), range (2492) and amount (84) are the attributes the profile reads.
    fn repairer_rows(guid: &str, cost: f64) -> String {
        format!(
            "\
effect\t6741\t1\t0\t51\t990001\t0\t0\t0\tfixtureRemoteRep\t{guid}\t0
effect\t990002\t1\t0\t51\t990001\t0\t0\t0\tfixtureStoredRep\teffects.FixtureRepStored\t0
type\t990300003\t99112\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t10\t6741\t482=40.0;55=400000.0;990001={cost};2491=6000.0;2492=18000.0;84=55.0\tFixture Repairer
"
        )
    }

    type Pushed = Vec<(Vec<i64>, Vec<eve_marshal::PyValue>)>;

    /// A world with one pilot, an invented repairer (`cost` of its 40 units of
    /// capacitor a cycle) and an invented ally; returns `(world, repairer, ally)`.
    fn repairer_world(guid: &str, cost: f64) -> (Registry, i64, i64) {
        use crate::npc::tests::{ALICE, due, hijacker_near, hijacker_near_kind, world};
        let table =
            crate::typedata::Table::parse(&repairer_rows(guid, cost)).expect("invented rows");
        let mut content = crate::content::Content::fixture();
        content.insert("npc_data", crate::npc_data::Table::fixture());
        content.insert("typedata", crate::typedata::Installed::build(table));
        let mut reg = world(&[ALICE]).with_content(std::sync::Arc::new(content));
        let kind: &'static crate::npc::NpcType = Box::leak(Box::new(crate::npc::NpcType {
            type_id: 990_300_003,
            ..crate::npc::FIXTURE_RAT
        }));
        let repairer = hijacker_near_kind(&mut reg, ALICE, 5_000.0, kind);
        let ally = hijacker_near(&mut reg, ALICE, 6_000.0);
        let _ = due(&mut reg);
        (reg, repairer, ally)
    }

    /// One assist cycle of `repairer` on `ally` at `now`.
    fn cycle_at(reg: &mut Registry, repairer: i64, ally: i64, now: BlueTime) {
        cycle_with(reg, repairer, ally, 6741, now); // the fixture rows' effect
    }

    /// [`cycle_at`] with the effect id the intent carries.
    fn cycle_with(reg: &mut Registry, repairer: i64, ally: i64, effect_id: i64, now: BlueTime) {
        let me = *reg.npcs().get(repairer).expect("live");
        let assist = Intent::Assist {
            ally,
            armor: true,
            amount: 5.0,
            effect_id,
            period: 6_000 * BLUE_PER_MS,
        };
        apply(reg, &me, assist, now, &mut Vec::new());
    }

    /// Every `OnSpecialFX` pushed since the last look.
    fn fx_pushed(reg: &mut Registry) -> Pushed {
        crate::npc::tests::due(reg)
            .into_iter()
            .filter(|(_, name, _)| *name == "OnSpecialFX")
            .map(|(to, _, args)| (to, args))
            .collect()
    }

    /// Run `cycles` assist cycles at one instant (cost 30 of 40) and return
    /// `(repairer, ally, every OnSpecialFX pushed, with its audience)`.
    fn assist_cycles(guid: &str, cycles: usize) -> (i64, i64, Pushed) {
        let (mut reg, repairer, ally) = repairer_world(guid, 30.0);
        let now = crate::pyutil::filetime_now();
        for _ in 0..cycles {
            cycle_at(&mut reg, repairer, ally, now);
        }
        (repairer, ally, fx_pushed(&mut reg))
    }

    #[test]
    fn an_assist_cycle_sends_a_beam_from_the_repairer_to_the_ally() {
        use eve_marshal::PyValue::{Bool, Int};
        let (repairer, ally, fx) = assist_cycles("effects.FixtureRepBeam", 1);
        assert_eq!(fx.len(), 1, "one beam for one cycle: {fx:?}");
        let (to, a) = &fx[0];
        assert_eq!(to, &vec![crate::npc::tests::ALICE], "the system's pilots");
        assert_eq!(
            (&a[0], &a[1]),
            (&Int(repairer), &Int(repairer)),
            "ship, module"
        );
        assert_eq!(a[3], Int(ally), "the target");
        assert_eq!(a[6], Bool(false), "not offensive");
        assert_eq!(
            (a[7].clone(), a[10].clone()),
            (Int(1), Int(1000)),
            "a start that runs until the stop"
        );
        assert_eq!(a[9], Int(6_000), "the cycle in ms");
    }

    #[test]
    fn the_beam_names_the_effects_own_guid() {
        use eve_marshal::PyValue::Bytes;
        let (_, _, fx) = assist_cycles("effects.FixtureRepSolo", 1);
        assert_eq!(fx[0].1[5], Bytes(b"effects.FixtureRepSolo".to_vec()));
        // A comma list sends the part before the first comma (§0.3.2).
        let (_, _, fx) = assist_cycles("effects.FixtureRepFirst,effects.FixtureRepSecond", 1);
        assert_eq!(fx[0].1[5], Bytes(b"effects.FixtureRepFirst".to_vec()));
        // An effect with no guid draws nothing.
        let (_, _, fx) = assist_cycles("", 1);
        assert!(fx.is_empty(), "{fx:?}");
    }

    /// The cycle uses the effect id the intent carries and does not read the
    /// profile: the type's profile says 6741 (guid `FixtureRepBeam`), the intent
    /// says 990002 (guid `FixtureRepStored`), and the beam names the intent's.
    #[test]
    fn the_cycle_uses_the_stored_effect_id_not_the_profile() {
        use eve_marshal::PyValue::Bytes;
        let (mut reg, repairer, ally) = repairer_world("effects.FixtureRepBeam", 5.0);
        let now = crate::pyutil::filetime_now();
        cycle_with(&mut reg, repairer, ally, 990_002, now);
        let fx = fx_pushed(&mut reg);
        assert_eq!(fx.len(), 1, "{fx:?}");
        assert_eq!(fx[0].1[5], Bytes(b"effects.FixtureRepStored".to_vec()));
    }

    #[test]
    fn no_beam_when_the_capacitor_cannot_pay() {
        // 40 held, 30 a cycle: the first cycle pays and shows, the second cannot.
        let (_, _, fx) = assist_cycles("effects.FixtureRepBeam", 2);
        assert_eq!(fx.len(), 1, "only the paid cycle draws: {fx:?}");
    }

    /// **One start for many cycles** (review r1 #1): the client drops a second
    /// start with the same module, so only the first cycle on an ally sends one.
    #[test]
    fn consecutive_cycles_on_one_ally_send_one_start() {
        use eve_marshal::PyValue::Int;
        let (mut reg, repairer, ally) = repairer_world("effects.FixtureRepBeam", 5.0);
        let t = crate::pyutil::filetime_now();
        for i in 0..3 {
            cycle_at(&mut reg, repairer, ally, t + i * 6_000 * BLUE_PER_MS);
        }
        let fx = fx_pushed(&mut reg);
        assert_eq!(fx.len(), 1, "three cycles, one start: {fx:?}");
        assert_eq!(fx[0].1[7], Int(1));
        // The later cycles moved the end of the beam, not its start.
        let beam = reg.npcs().get(repairer).expect("live").mind.assist_beams[0].expect("beam");
        assert!(beam.until > t + 12_000 * BLUE_PER_MS, "{}", beam.until - t);
    }

    fn mind_with_beam(ally: i64, until: BlueTime) -> Npc {
        let mut n = idle();
        n.mind.assist_beams[0] = Some(AssistBeam {
            ally,
            until,
            guid: "effects.FixtureRepBeam",
            period_ms: 6_000,
        });
        n
    }

    fn ends(out: &[Intent]) -> Vec<i64> {
        out.iter()
            .filter_map(|i| match i {
                Intent::AssistEnd { ally, .. } => Some(*ally),
                _ => None,
            })
            .collect()
    }

    /// **A stop when the repairs move to another ally**, none while they stay.
    #[test]
    fn the_beam_ends_when_the_repairs_switch_ally() {
        let b = assist_behaviour(true, 1, 1.0);
        let far = 10 * ASSIST_PERIOD;
        let n = mind_with_beam(1, far);
        // Same ally still hurt: it keeps its beam.
        let wards = vec![ally(1, 1_000.0, None, 0.1), ally(2, 1_000.0, None, 0.5)];
        let (after, out) = decide(&n, &assist_view(b, wards, 1));
        assert!(ends(&out).is_empty(), "{out:?}");
        assert_eq!(assists(&out), vec![1]);
        assert!(after.mind.assist_beams[0].is_some());
        // Ally 1 healed, 2 hurt: 1's beam ends, 2 is repaired.
        let wards = vec![ally(1, 1_000.0, None, 0.9), ally(2, 1_000.0, None, 0.1)];
        let (after, out) = decide(&n, &assist_view(b, wards, 1));
        assert_eq!(ends(&out), vec![1]);
        assert_eq!(assists(&out), vec![2]);
        assert!(after.mind.assist_beams[0].is_none());
    }

    /// **A stop when the assist ends**: nobody hurt at the next cycle, a cycle
    /// that did not run (chance), or a beam nothing renewed.
    #[test]
    fn the_beam_ends_when_the_assist_ends() {
        let b = assist_behaviour(true, 1, 1.0);
        let n = mind_with_beam(1, 10 * ASSIST_PERIOD);
        let healed = vec![ally(1, 1_000.0, None, 0.9)];
        let (after, out) = decide(&n, &assist_view(b, healed.clone(), 1));
        assert_eq!(ends(&out), vec![1]);
        assert!(after.mind.assist_beams[0].is_none());
        // A zero chance: the cycle does not run, the beam ends.
        let never = assist_behaviour(true, 1, 0.0);
        let hurt = vec![ally(1, 1_000.0, None, 0.1)];
        let (_, out) = decide(&n, &assist_view(never, hurt, 1));
        assert_eq!(ends(&out), vec![1]);
        // Between cycles, within its time: nothing. Past it: ended.
        let mut waiting = mind_with_beam(1, 100);
        waiting.mind.next_assist_at = Some(1_000);
        let (_, out) = decide(&waiting, &assist_view(b, healed.clone(), 100));
        assert!(ends(&out).is_empty(), "{out:?}");
        let (after, out) = decide(&waiting, &assist_view(b, healed, 101));
        assert_eq!(ends(&out), vec![1]);
        assert!(after.mind.assist_beams[0].is_none());
    }

    /// The end is a stop `OnSpecialFX` on the same ship, module, target and guid.
    #[test]
    fn an_assist_end_sends_the_stop() {
        use eve_marshal::PyValue::{Bytes, Int};
        let (mut reg, repairer, ally) = repairer_world("effects.FixtureRepBeam", 5.0);
        let me = *reg.npcs().get(repairer).expect("live");
        let end = Intent::AssistEnd {
            ally,
            guid: "effects.FixtureRepBeam",
            period_ms: 6_000,
        };
        apply(&mut reg, &me, end, 0, &mut Vec::new());
        let fx = fx_pushed(&mut reg);
        assert_eq!(fx.len(), 1, "{fx:?}");
        let a = &fx[0].1;
        assert_eq!(
            (&a[0], &a[1], &a[3]),
            (&Int(repairer), &Int(repairer), &Int(ally))
        );
        assert_eq!(a[5], Bytes(b"effects.FixtureRepBeam".to_vec()));
        assert_eq!(a[7], Int(0), "a stop");
    }

    /// A late arrival is replayed the running beams, and not an expired one.
    #[test]
    fn a_running_beam_is_replayed_to_a_late_arrival() {
        let (mut reg, repairer, ally) = repairer_world("effects.FixtureRepBeam", 5.0);
        let t = crate::pyutil::filetime_now();
        cycle_at(&mut reg, repairer, ally, t);
        let me = *reg.npcs().get(repairer).expect("live");
        let replay = running_assist_fx(&me, t + BLUE_PER_MS);
        assert_eq!(replay.len(), 1);
        assert!(replay[0].start && replay[0].to.is_empty());
        assert_eq!(replay[0].target_id, Some(ally));
        assert!(running_assist_fx(&me, t + 60_000 * BLUE_PER_MS).is_empty());
    }

    /// **Through the NPC loop** (pv6dbg, #228): the world clock's own `npc::tick`
    /// (view, `allies_of`, decide, apply) draws the beam only once the ally's
    /// *armour* is hurt. A volley the shield soaks leaves the armour full, and an
    /// armour repairer has nothing to repair: in the client run that saw no beam
    /// (ci-pv6b) the Liminal's shield was at 0.565 and its armour at 1.000 when the
    /// pilot died. Invented hit points; no client value.
    #[test]
    fn the_loop_beams_on_armour_damage_not_on_shield_damage() {
        use crate::damage::{Damage, Profile, ShipDamage};
        use eve_marshal::PyValue::{Bytes, Int};
        let (mut reg, repairer, ally) = repairer_world("effects.FixtureRepBeam", 5.0);
        let one = Damage {
            em: 1.0,
            explosive: 1.0,
            kinetic: 1.0,
            thermal: 1.0,
        };
        let profile = Profile {
            shield_capacity: 100.0,
            shield_recharge_rate: 1.0e9,
            armor_hp: 100.0,
            hp: 100.0,
            shield_resonance: one,
            armor_resonance: one,
            hull_resonance: one,
        };
        let em = |em: f64| Damage {
            em,
            ..Damage::default()
        };
        let beams = |reg: &mut Registry| {
            fx_pushed(reg)
                .into_iter()
                .filter(|(_, a)| a[5] == Bytes(b"effects.FixtureRepBeam".to_vec()))
                .filter(|(_, a)| a[0] == Int(repairer))
                .collect::<Vec<_>>()
        };
        let t = crate::pyutil::filetime_now();
        // The shield takes the volley: no beam, however many cycles pass.
        let mut shot = ShipDamage::full(profile, t);
        shot.apply(em(60.0), t);
        assert_eq!(shot.bars(t).armour, 1.0, "the shield soaked it");
        reg.combat_mut().set_ship(ally, shot);
        for i in 0..4 {
            crate::npc::tick(&mut reg, t + i * 3_000 * BLUE_PER_MS);
        }
        assert!(
            beams(&mut reg).is_empty(),
            "shield damage alone draws no beam"
        );
        // Through the shield into the armour: the next cycle draws one start.
        let t2 = t + 12_000 * BLUE_PER_MS;
        let mut shot = ShipDamage::full(profile, t2);
        shot.apply(em(130.0), t2);
        assert!(shot.bars(t2).armour < 1.0, "the armour is hurt");
        reg.combat_mut().set_ship(ally, shot);
        for i in 0..3 {
            crate::npc::tick(&mut reg, t2 + i * 3_000 * BLUE_PER_MS);
        }
        // One start; the cycle's repair (55) heals the 30 lost in one go, so the
        // next cycle finds nobody hurt and sends the stop.
        let got = beams(&mut reg);
        let flags: Vec<_> = got.iter().map(|(_, a)| a[7].clone()).collect();
        assert_eq!(flags, vec![Int(1), Int(0)], "a start, then a stop: {got:?}");
        assert_eq!(got[0].1[3], Int(ally), "aimed at the ally");
    }

    // ---- CP45 (W17): missiles ----------------------------------------------

    /// An invented launcher: 4 s cycle, 10 km reach. **Not a client row.**
    fn launcher() -> crate::npc_missile::Launcher {
        crate::npc_missile::Launcher {
            missile_type_id: 990_450_002,
            volley: crate::damage::Damage {
                kinetic: 10.0,
                ..crate::damage::Damage::ZERO
            },
            period: 4_000 * BLUE_PER_MS,
            speed: 1_000.0,
            flight_s: 10.0,
        }
    }

    /// An NPC fighting `ALICE_SHIP`, its lock ready, nothing launched yet.
    fn engaged_at_alice(kind: &'static crate::npc::NpcType) -> Npc {
        Npc {
            kind,
            phase: Phase::Orbit,
            target: Some(ALICE_SHIP),
            lock_ready_at: Some(0),
            ..idle()
        }
    }

    /// PV-5 (#227): a type with no 247 but a gun range (54) opens fire inside
    /// that range and holds outside it; a type with neither never fires.
    #[test]
    fn a_gun_range_without_247_is_the_reach_the_brain_fires_at() {
        let fires = |id: i64, d: f64| {
            let kind: &'static crate::npc::NpcType =
                Box::leak(Box::new(crate::npc::tests::reach_type(id)));
            let v = view(vec![candidate(ALICE_SHIP, [d, 0.0, 0.0])], true, 50);
            let (_, out) = decide(&engaged_at_alice(kind), &v);
            out.contains(&Intent::FireStart(ALICE_SHIP))
        };
        assert!(fires(990_200_021, 7000.0), "inside 54: fires");
        assert!(!fires(990_200_021, 9000.0), "outside 54: holds");
        assert!(!fires(990_200_022, 7000.0), "247 (5000) still rules");
        assert!(fires(990_200_022, 4000.0));
        assert!(!fires(990_200_023, 1.0), "neither: reach 0, as before");
    }

    fn missile_view(at: f64, now: BlueTime) -> View {
        View {
            behaviour: Behaviour {
                missile: Some(launcher()),
                ..Behaviour::default()
            },
            ..view(vec![candidate(ALICE_SHIP, [at, 0.0, 0.0])], true, now)
        }
    }

    fn missiles(out: &[Intent]) -> Vec<Intent> {
        out.iter()
            .copied()
            .filter(|i| matches!(i, Intent::Missile { .. }))
            .collect()
    }

    /// **One launch per cycle inside reach**, the first at once, each on its
    /// own boundary.
    #[test]
    fn a_launcher_launches_once_per_cycle_inside_reach() {
        let l = launcher();
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let t0 = 1_000;
        let (n, out) = decide(&npc, &missile_view(l.reach() / 2.0, t0));
        assert_eq!(
            missiles(&out),
            vec![Intent::Missile {
                target: ALICE_SHIP,
                at: t0
            }]
        );
        assert_eq!(n.mind.next_missile_at, Some(t0 + l.period));
        let (n, out) = decide(&n, &missile_view(l.reach() / 2.0, t0 + l.period - 1));
        assert!(missiles(&out).is_empty(), "not before the cycle");
        let (_, out) = decide(&n, &missile_view(l.reach() / 2.0, t0 + l.period));
        assert_eq!(
            missiles(&out),
            vec![Intent::Missile {
                target: ALICE_SHIP,
                at: t0 + l.period
            }]
        );
    }

    /// Beyond reach, before the lock or with no launcher: no launch.
    #[test]
    fn no_launch_beyond_reach_before_the_lock_or_without_a_launcher() {
        let l = launcher();
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let (n, out) = decide(&npc, &missile_view(l.reach() * 1.01, 10));
        assert!(missiles(&out).is_empty());
        assert_eq!(n.mind.next_missile_at, None);
        let locking = Npc {
            lock_ready_at: Some(20),
            ..npc
        };
        let (_, out) = decide(&locking, &missile_view(l.reach() / 2.0, 10));
        assert!(missiles(&out).is_empty());
        let plain = View {
            behaviour: Behaviour::default(),
            ..missile_view(l.reach() / 2.0, 10)
        };
        let (n, out) = decide(&npc, &plain);
        assert!(missiles(&out).is_empty());
        assert_eq!(n.mind.next_missile_at, None);
    }

    /// A launcher-only type -- no hull damage -- starts no gun; a type with
    /// both does both.
    #[test]
    fn a_launcher_only_type_starts_no_gun() {
        static LAUNCHER_ONLY: crate::npc::NpcType = crate::npc::NpcType {
            damage: crate::damage::Damage::ZERO,
            ..crate::npc::FIXTURE_RAT
        };
        let near = 1_000.0;
        let (n, out) = decide(&engaged_at_alice(&LAUNCHER_ONLY), &missile_view(near, 10));
        assert_eq!(missiles(&out).len(), 1);
        assert!(!n.firing);
        assert!(
            !out.iter()
                .any(|i| matches!(i, Intent::FireStart(_) | Intent::Shoot { .. }))
        );
        let (n, out) = decide(
            &engaged_at_alice(&crate::npc::FIXTURE_RAT),
            &missile_view(near, 10),
        );
        assert_eq!(missiles(&out).len(), 1);
        assert!(n.firing && out.contains(&Intent::FireStart(ALICE_SHIP)));
    }

    /// Letting a target go forgets the launch cycle: the next lock launches
    /// at its own first shot.
    #[test]
    fn letting_go_forgets_the_launch_cycle() {
        let l = launcher();
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let (n, _) = decide(&npc, &missile_view(l.reach() / 2.0, 10));
        assert!(n.mind.next_missile_at.is_some());
        let gone = View {
            candidates: vec![],
            ..missile_view(l.reach() / 2.0, 20)
        };
        let (n, _) = decide(&n, &gone);
        assert_eq!(n.mind.next_missile_at, None);
    }

    // ---- CP49 (W18): tackle ------------------------------------------------

    /// An invented scrambler and web: 4 s cycles, 10 km, always on.
    /// **Not a client row.**
    fn tackle(kind: crate::npc_ewar::Kind, chance: f64) -> crate::npc_ewar::Effect {
        crate::npc_ewar::Effect {
            kind,
            range: 10_000.0,
            strength: match kind {
                crate::npc_ewar::Kind::Scram => 1.0,
                crate::npc_ewar::Kind::Web => -50.0,
                crate::npc_ewar::Kind::Neut => 30.0,
            },
            period: 4_000 * BLUE_PER_MS,
            chance,
        }
    }

    fn tackle_view(at: f64, now: BlueTime, chance: f64) -> View {
        View {
            behaviour: Behaviour {
                scram: Some(tackle(crate::npc_ewar::Kind::Scram, chance)),
                web: Some(tackle(crate::npc_ewar::Kind::Web, chance)),
                ..Behaviour::default()
            },
            ..view(vec![candidate(ALICE_SHIP, [at, 0.0, 0.0])], true, now)
        }
    }

    fn tackling(out: &[Intent]) -> Vec<Intent> {
        out.iter()
            .copied()
            .filter(|i| matches!(i, Intent::Scram { .. } | Intent::Web { .. }))
            .collect()
    }

    /// **One scram and one web per cycle inside range**, the first at once,
    /// each on its own boundary; a stalled clock gets only the latest.
    #[test]
    fn a_tackler_cycles_its_scram_and_web_inside_range() {
        let e = tackle(crate::npc_ewar::Kind::Scram, 1.0);
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let t0 = 1_000;
        let (n, out) = decide(&npc, &tackle_view(e.range / 2.0, t0, 1.0));
        assert_eq!(
            tackling(&out),
            vec![
                Intent::Scram {
                    target: ALICE_SHIP,
                    at: t0
                },
                Intent::Web {
                    target: ALICE_SHIP,
                    at: t0
                },
            ]
        );
        assert_eq!(n.mind.next_scram_at, Some(t0 + e.period));
        let (n, out) = decide(&n, &tackle_view(e.range / 2.0, t0 + e.period - 1, 1.0));
        assert!(tackling(&out).is_empty(), "not before the cycle");
        let late = t0 + 3 * e.period + 7;
        let (n, out) = decide(&n, &tackle_view(e.range / 2.0, late, 1.0));
        assert_eq!(tackling(&out).len(), 2, "one each, not three owed");
        assert!(out.contains(&Intent::Scram {
            target: ALICE_SHIP,
            at: t0 + 3 * e.period
        }));
        assert_eq!(n.mind.next_scram_at, Some(t0 + 4 * e.period));
    }

    /// NEUT-NOS: a neutralizer cycles like the others, on its own clock.
    #[test]
    fn a_neutralizer_cycles_inside_range() {
        let e = tackle(crate::npc_ewar::Kind::Neut, 1.0);
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let view = |at: f64, now: BlueTime| View {
            behaviour: Behaviour {
                neut: Some(e),
                ..Behaviour::default()
            },
            ..view(vec![candidate(ALICE_SHIP, [at, 0.0, 0.0])], true, now)
        };
        let t0 = 1_000;
        let (n, out) = decide(&npc, &view(e.range / 2.0, t0));
        assert!(out.contains(&Intent::Neut {
            target: ALICE_SHIP,
            at: t0
        }));
        assert_eq!(n.mind.next_neut_at, Some(t0 + e.period));
        let (n, out) = decide(&n, &view(e.range / 2.0, t0 + e.period - 1));
        assert!(!out.iter().any(|i| matches!(i, Intent::Neut { .. })));
        let (_, out) = decide(&n, &view(e.range * 1.01, t0 + e.period));
        assert!(!out.iter().any(|i| matches!(i, Intent::Neut { .. })));
    }

    /// Beyond range, before the first shot, at a chance of zero, or without
    /// the capability: no cycle. Letting go forgets the cycle.
    #[test]
    fn no_tackle_beyond_range_before_the_lock_or_by_chance() {
        let e = tackle(crate::npc_ewar::Kind::Scram, 1.0);
        let npc = engaged_at_alice(&crate::npc::FIXTURE_RAT);
        let (n, out) = decide(&npc, &tackle_view(e.range * 1.01, 10, 1.0));
        assert!(tackling(&out).is_empty());
        assert_eq!(n.mind.next_scram_at, None);
        let locking = Npc {
            lock_ready_at: Some(20),
            ..npc
        };
        let (_, out) = decide(&locking, &tackle_view(e.range / 2.0, 10, 1.0));
        assert!(tackling(&out).is_empty());
        let (n, out) = decide(&npc, &tackle_view(e.range / 2.0, 10, 0.0));
        assert!(tackling(&out).is_empty(), "a zero chance never runs");
        assert!(n.mind.next_scram_at.is_some(), "but the cycle still turns");
        let plain = View {
            behaviour: Behaviour::default(),
            ..tackle_view(e.range / 2.0, 10, 1.0)
        };
        let (_, out) = decide(&npc, &plain);
        assert!(tackling(&out).is_empty());

        let (n, _) = decide(&npc, &tackle_view(e.range / 2.0, 10, 1.0));
        assert!(n.mind.next_scram_at.is_some() && n.mind.next_web_at.is_some());
        let gone = View {
            candidates: vec![],
            ..tackle_view(e.range / 2.0, 20, 1.0)
        };
        let (n, _) = decide(&n, &gone);
        assert_eq!((n.mind.next_scram_at, n.mind.next_web_at), (None, None));
    }

    // ---- K-RATAI: switching, remote repair, the law ----------------------

    /// A switching behaviour with named, invented numbers (no `[A]` value:
    /// the skip chance and the cycle are the test's own).
    const EVERY: BlueTime = 600_000;
    const ASSIST_PERIOD: BlueTime = 100_000;

    fn switcher() -> Behaviour {
        Behaviour {
            switch_every: Some(EVERY),
            ..Behaviour::default()
        }
    }

    fn locker(ship_id: i64, at: [f64; 3]) -> Candidate {
        Candidate {
            locking_me: true,
            ..candidate(ship_id, at)
        }
    }

    /// Alice locks first and is nearer; Bob locks later and is further.
    /// Returns the NPC after both have been seen, engaged on Alice.
    fn alice_then_bob(b: Behaviour, base: Npc) -> (Npc, Candidate, Candidate) {
        let alice = locker(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let bob = locker(BOB_SHIP, [5_000.0, 0.0, 0.0]);
        let t0 = 1_000_000;
        let (n, _) = decide(&base, &with(b, view(vec![alice], false, t0)));
        assert_eq!(n.target, Some(ALICE_SHIP));
        let (n, _) = decide(&n, &with(b, view(vec![alice, bob], true, t0 + 10)));
        (n, alice, bob)
    }

    /// **A switching rat turns to its newest aggressor** at its timer, not
    /// to the nearest ship; before the timer it keeps its target.
    #[test]
    fn a_switching_rat_turns_to_its_newest_aggressor() {
        let b = switcher();
        let (n, alice, bob) = alice_then_bob(b, idle());
        assert_eq!(n.mind.aggressors.iter().flatten().count(), 2);
        let due = n.mind.retarget_at.expect("a timer");
        let (kept, _) = decide(&n, &with(b, view(vec![alice, bob], true, due - 1)));
        assert_eq!(kept.target, Some(ALICE_SHIP));
        let (after, out) = decide(&n, &with(b, view(vec![alice, bob], true, due)));
        assert_eq!(after.target, Some(BOB_SHIP));
        assert!(out.contains(&Intent::Unlock(ALICE_SHIP)));
        assert!(out.contains(&Intent::Lock(BOB_SHIP)));
    }

    /// An aggressor that stops locking is forgotten, so the newest one
    /// still locking wins; with nobody locking the nearest does, as before.
    #[test]
    fn a_rat_forgets_an_aggressor_that_stops_locking() {
        let b = switcher();
        let (n, alice, bob) = alice_then_bob(b, idle());
        let due = n.mind.retarget_at.expect("a timer");
        let bob_quiet = Candidate {
            locking_me: false,
            ..bob
        };
        let (after, _) = decide(&n, &with(b, view(vec![alice, bob_quiet], true, due)));
        assert_eq!(after.target, Some(ALICE_SHIP));
        assert_eq!(after.mind.aggressors.iter().flatten().count(), 1);
    }

    const CAROL_SHIP: i64 = 100_000_003;

    fn hostile(ship_id: i64, at: [f64; 3], threat: f64) -> Candidate {
        Candidate {
            aggressor: true,
            threat,
            ..candidate(ship_id, at)
        }
    }

    /// NPC-THREAT, the sniper: an idle rat acquires a ship that shot it from
    /// 40 km (past the notice radius, not locking it) and follows it; the
    /// same ship, had it not shot, is ignored.
    #[test]
    fn an_idle_rat_answers_a_sniper_beyond_its_notice_radius() {
        let at = [40_000.0, 0.0, 0.0];
        let (n, out) = decide(
            &idle(),
            &view(vec![hostile(ALICE_SHIP, at, 50.0)], false, 1),
        );
        assert_eq!(n.target, Some(ALICE_SHIP));
        assert!(out.contains(&Intent::Lock(ALICE_SHIP)));
        assert!(out.contains(&Intent::Follow(ALICE_SHIP)));
        let (n, _) = decide(&idle(), &view(vec![candidate(ALICE_SHIP, at)], false, 1));
        assert_eq!(n.target, None, "a bystander at 40 km is not noticed");
        let far = [PURSUIT_RANGE * 1.1, 0.0, 0.0];
        let (n, _) = decide(
            &idle(),
            &view(vec![hostile(ALICE_SHIP, far, 50.0)], false, 1),
        );
        assert_eq!(n.target, None, "past the chase range it is out of reach");
    }

    /// An aggressor with threat 0 (a jam, a miss) still outranks a nearer
    /// ship that did nothing.
    #[test]
    fn an_aggressor_outranks_a_nearer_bystander() {
        let near = candidate(BOB_SHIP, [3_000.0, 0.0, 0.0]);
        let jammer = hostile(ALICE_SHIP, [15_000.0, 0.0, 0.0], 0.0);
        let (n, _) = decide(&idle(), &view(vec![near, jammer], false, 1));
        assert_eq!(n.target, Some(ALICE_SHIP));
    }

    /// NPC-THREAT: at the timer a rat leaves the nearer, weaker aggressor
    /// for the one that hurts it most, even from 30 km.
    #[test]
    fn threat_beats_proximity_at_the_timer() {
        let b = switcher();
        let bob = hostile(BOB_SHIP, [5_000.0, 0.0, 0.0], 100.0);
        let alice = hostile(ALICE_SHIP, [30_000.0, 0.0, 0.0], 1_000.0);
        let t0 = 1_000_000;
        let (n, _) = decide(&idle(), &with(b, view(vec![bob], false, t0)));
        assert_eq!(n.target, Some(BOB_SHIP));
        let due = n.mind.retarget_at.expect("a timer");
        let (kept, _) = decide(&n, &with(b, view(vec![bob, alice], true, due - 1)));
        assert_eq!(kept.target, Some(BOB_SHIP));
        let (after, out) = decide(&n, &with(b, view(vec![bob, alice], true, due)));
        assert_eq!(after.target, Some(ALICE_SHIP));
        assert!(out.contains(&Intent::Unlock(BOB_SHIP)));
        assert!(out.contains(&Intent::Lock(ALICE_SHIP)));
    }

    /// The logi: its heals made it an aggressor with the top threat, so the
    /// rat leaves the shooter for the repairer at the timer.
    #[test]
    fn the_logi_becomes_the_top_threat() {
        let b = switcher();
        let alice = hostile(ALICE_SHIP, [8_000.0, 0.0, 0.0], 100.0);
        let carol = hostile(CAROL_SHIP, [9_000.0, 0.0, 0.0], 300.0);
        let t0 = 1_000_000;
        let (n, _) = decide(&idle(), &with(b, view(vec![alice], false, t0)));
        let due = n.mind.retarget_at.expect("a timer");
        let (after, _) = decide(&n, &with(b, view(vec![alice, carol], true, due)));
        assert_eq!(after.target, Some(CAROL_SHIP));
    }

    /// The law keeps its brain: a threat on a candidate changes nothing for it.
    #[test]
    fn the_law_ignores_the_ledger() {
        let law = Npc {
            law: Some(crate::npc::Law { offender: 1 }),
            ..idle()
        };
        let at = [AGGRESSION_RANGE / 2.0, 0.0, 0.0];
        let with_ledger = decide(&law, &view(vec![hostile(ALICE_SHIP, at, 5.0)], false, 1));
        let without = decide(&law, &view(vec![candidate(ALICE_SHIP, at)], false, 1));
        assert_eq!(with_ledger, without);
    }

    /// Attribute 1651: a due switch is skipped with the type's chance; a
    /// chance of 1 never switches, and the timer still turns.
    #[test]
    fn a_skipped_switch_keeps_the_target_and_renews_the_timer() {
        let b = Behaviour {
            switch_skip_chance: 1.0,
            ..switcher()
        };
        let (n, alice, bob) = alice_then_bob(b, idle());
        let due = n.mind.retarget_at.expect("a timer");
        let (after, out) = decide(&n, &with(b, view(vec![alice, bob], true, due)));
        assert_eq!(after.target, Some(ALICE_SHIP));
        assert!(!out.contains(&Intent::Lock(BOB_SHIP)));
        assert_eq!(after.mind.retarget_at, Some(due + EVERY));
    }

    fn assist_behaviour(armor: bool, max_targets: usize, chance: f64) -> Behaviour {
        Behaviour {
            assist: Some(Assist {
                effect_id: 990_002,
                armor,
                amount: 7.0,
                period: ASSIST_PERIOD,
                chance,
                threshold: 0.5,
                range: 10_000.0,
                max_targets,
            }),
            ..Behaviour::default()
        }
    }

    fn ally(id: i64, x: f64, shield: Option<f64>, armour: f64) -> Ally {
        Ally {
            id,
            position: [x, 0.0, 0.0],
            shield,
            armour,
        }
    }

    fn assists(out: &[Intent]) -> Vec<i64> {
        out.iter()
            .filter_map(|i| match i {
                Intent::Assist { ally, .. } => Some(*ally),
                _ => None,
            })
            .collect()
    }

    fn assist_view(b: Behaviour, allies: Vec<Ally>, now: BlueTime) -> View {
        View {
            allies,
            ..with(b, view(vec![], false, now))
        }
    }

    /// **A remote repairer repairs the hurt, in range, below its
    /// threshold**, most hurt first, up to its target count, one layer.
    #[test]
    fn a_remote_repairer_repairs_the_hurt_in_range() {
        let b = assist_behaviour(true, 2, 1.0);
        let wards = vec![
            ally(1, 1_000.0, Some(1.0), 0.4),
            ally(2, 2_000.0, Some(1.0), 0.1),
            ally(3, 3_000.0, Some(1.0), 0.3),
            ally(4, 4_000.0, Some(1.0), 0.9),
            ally(5, 50_000.0, Some(1.0), 0.0),
        ];
        let (_, out) = decide(&idle(), &assist_view(b, wards.clone(), 1));
        assert_eq!(assists(&out), vec![2, 3], "most hurt first, two of them");
        assert!(out.contains(&Intent::Assist {
            ally: 2,
            armor: true,
            amount: 7.0,
            effect_id: 990_002,
            period: ASSIST_PERIOD,
        }));
        // A shield repairer looks at the shield, and a hull with none is
        // never repaired by it.
        let shield = assist_behaviour(false, 3, 1.0);
        let wards = vec![
            ally(1, 1_000.0, Some(0.2), 1.0),
            ally(2, 1_000.0, None, 0.0),
            ally(3, 1_000.0, Some(0.5), 1.0),
        ];
        let (_, out) = decide(&idle(), &assist_view(shield, wards, 1));
        assert_eq!(assists(&out), vec![1], "0.5 is not below the threshold");
    }

    /// **The cycle**: a repair spends it for one period; with nobody hurt
    /// it is not spent and goes at once when someone is; a zero chance
    /// repairs nothing but still takes the period.
    #[test]
    fn a_remote_repairer_cycles_per_its_period() {
        let b = assist_behaviour(true, 1, 1.0);
        let hurt = vec![ally(1, 1_000.0, None, 0.1)];
        let (n, none) = decide(&idle(), &assist_view(b, vec![], 5));
        assert!(none.is_empty() && n.mind.next_assist_at.is_none());
        let (n, out) = decide(&n, &assist_view(b, hurt.clone(), 5));
        assert_eq!(assists(&out), vec![1]);
        assert_eq!(n.mind.next_assist_at, Some(5 + ASSIST_PERIOD));
        let (n, out) = decide(&n, &assist_view(b, hurt.clone(), 5 + ASSIST_PERIOD - 1));
        assert!(out.is_empty());
        let (_, out) = decide(&n, &assist_view(b, hurt.clone(), 5 + ASSIST_PERIOD));
        assert_eq!(assists(&out), vec![1]);
        let never = assist_behaviour(true, 1, 0.0);
        let (n, out) = decide(&idle(), &assist_view(never, hurt, 5));
        assert!(out.is_empty());
        assert_eq!(n.mind.next_assist_at, Some(5 + ASSIST_PERIOD));
        // Itself is never its own ward.
        let me = idle();
        let (_, out) = decide(&me, &assist_view(b, vec![ally(me.id, 0.0, None, 0.0)], 5));
        assert!(out.is_empty());
    }

    /// **CONCORD stays as it was** (K-RATAI's keep-out rule): a ship of the
    /// law, given a behaviour with a switch skip, a remote repairer and two
    /// aggressors, decides exactly as it does with the same behaviour
    /// stripped of every K-RATAI field, and remembers no aggressor. The
    /// same scene makes a rat switch, so the test sees the difference.
    #[test]
    fn a_ship_of_the_law_is_untouched_by_the_rat_behaviours() {
        let new = Behaviour {
            switch_skip_chance: 0.0,
            ..assist_behaviour(true, 3, 1.0)
        };
        let new = Behaviour {
            switch_every: Some(EVERY),
            ..new
        };
        let legacy = Behaviour {
            assist: None,
            switch_skip_chance: 0.0,
            ..new
        };
        let law = Npc {
            law: Some(crate::npc::Law { offender: 1 }),
            ..idle()
        };
        let wards = vec![ally(9, 500.0, Some(0.1), 0.1)];
        let alice = locker(ALICE_SHIP, [1_000.0, 0.0, 0.0]);
        let bob = locker(BOB_SHIP, [5_000.0, 0.0, 0.0]);
        let run = |b: Behaviour, base: Npc| {
            let mut n = base;
            let mut log = Vec::new();
            let steps = [
                (vec![alice], 1_000_000),
                (vec![alice, bob], 1_980_010),
                (vec![alice, bob], 1_000_000 + EVERY),
                (vec![alice, bob], 1_000_000 + 2 * EVERY),
            ];
            for (cands, now) in steps {
                let v = View {
                    allies: wards.clone(),
                    ..with(b, view(cands, true, now))
                };
                let (next, out) = decide(&n, &v);
                log.push((next, out));
                n = next;
            }
            log
        };
        assert_eq!(run(new, law), run(legacy, law), "the law ignores K-RATAI");
        for (n, out) in run(new, law) {
            assert!(n.mind.aggressors.iter().all(Option::is_none));
            assert!(assists(&out).is_empty());
        }
        let rat = run(new, idle());
        assert!(
            rat.iter().any(|(n, _)| n.target == Some(BOB_SHIP)),
            "the same scene turns a rat to the newer aggressor"
        );
        assert!(rat.iter().any(|(_, out)| !assists(out).is_empty()));
    }
}
