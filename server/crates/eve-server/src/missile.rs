//! **Missiles in flight** (R-MISSILE §2, packet M3): the server's own copy
//! of every missile between its launch and its end.
//!
//! The client flies the missile it is shown (`AddBalls2` + `LaunchMissile`,
//! [`crate::destiny::MissileLaunched`]); this module flies a copy at the
//! world clock's rate, decides the hit, lands the damage through
//! [`crate::combat::missile_volley`], and removes the ball: a plain
//! `RemoveBalls` after a hit, so the warhead finishes its explosion, and
//! `RemoveBall(id, True)` ([`crate::destiny::Kill::fizzle`]) when the fuse
//! ran out, so it vanishes in flight (R-MISSILE §1.5 **[V]**).
//!
//! # The flight (§2.1)
//!
//! - `speed` is the charge's `maxVelocity` (37), times the entity's 645 for
//!   an NPC; the fuse is `explosionDelay` (281, ms), times 646 for an NPC.
//! - **The launch tick** flies straight along the launch line, as Carbon's
//!   `EvolveMissile` does for its first 800 ms **[V]**; after that a pure
//!   pursuit at `speed`. `[A]` pursuit is close enough to Carbon's lead-point
//!   steering. Falsifier: server impact and client explosion more than 1 s
//!   apart against an orbiting target.
//! - **The hit**: the step's closest approach to the target is within
//!   `charge radius + target radius` (`MissileFollow`'s range **[V]**), so a
//!   fast missile cannot tunnel through a small target.
//! - **Lost**: the target left the roster (died, docked, jumped), its
//!   system, or entered an activated warp. Carbon sends the missile into the
//!   sunset (`StopAllFollowers` **[V]**); the copy stops, deals nothing, and
//!   fizzles at the end of its fuse. Cloaking is not modelled here: the
//!   roster has no cloak.
//! - **The fuse**: at `launch + fuse` without a hit, the missile fizzles.
//!   A launch beyond reach is allowed and fizzles.
//! - The skills, rigs, implants and modules that change the flight, the
//!   explosion and the damage are applied by [`crate::missile_skills`]
//!   (K-MSKILL): a player's [`Charge`] is the sheet's, not the type's.
//! - `[A]` the launcher dying or warping does not stop the missile (Carbon's
//!   master would explode it). Falsifier: a retail log of a missile hitting
//!   after its launcher died -- or of one that never did.
//!
//! # The damage (§2.2) **[D]**
//!
//! CCP's published formula, not in the client:
//! `min(1, S/E, ((S/E)·(Ve/Vt))^drf)` with `S` the target's signature
//! (552), `Vt` its speed now, `E` the charge's `aoeCloudSize` (654), `Ve`
//! its `aoeVelocity` (653) and `drf` its `aoeDamageReductionFactor`
//! (1353). See [`damage_factor`]. `[A]` 1354 is unused (the pre-2010 form);
//! falsified by a retail log figure that disagrees.
//!
//! # Ids
//!
//! [`ID_BASE`]`..`[`ID_END`], not persisted, the counter wraps; clear of
//! every other range this server mints (the test lists them). A restart
//! loses every flight, as `npc_missile`'s did.

use crate::combat::MissileVolley;
use crate::damage::{BlueTime, Damage};
use crate::destiny::{BallsRemoved, Kill, MissileBall, MissileLaunched, MissileSlim};
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use std::time::Duration;

/// Blue ticks per second.
const BLUE_PER_S: f64 = 10_000_000.0;

/// The first missile id, and one past the last (R-MISSILE §2.3).
pub const ID_BASE: i64 = 9_500_000_000_000;
pub const ID_END: i64 = 9_600_000_000_000;

/// `effects.MissileDeployment`: the guid of the charge's default effect 9
/// (`missileLaunching`) and of the entities' 569 **[V-data]**.
pub const GUID: &str = "effects.MissileDeployment";

/// `[A]` how long after a hit the plain `RemoveBalls` goes: one Destiny
/// stamp, so the warhead's explosion is not cut short (R-MISSILE §1.5).
/// Falsifier: the explosion is visibly clipped in the client.
pub const HIT_REMOVAL_DELAY: Duration = Duration::from_secs(1);

/// The signature radius a target with none gets **[V-data]** (the attribute's
/// own default).
pub const DEFAULT_SIGNATURE: f64 = 100.0;

/// The dogma ids this module reads **[V]**.
pub mod attr {
    pub const MAX_VELOCITY: i64 = 37;
    pub const AGILITY: i64 = 70;
    pub const EXPLOSION_DELAY: i64 = 281;
    pub const AOE_VELOCITY: i64 = 653;
    pub const AOE_CLOUD_SIZE: i64 = 654;
    pub const AOE_DAMAGE_REDUCTION_FACTOR: i64 = 1353;
}

/// A charge's explosion: what the damage factor reads from it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Aoe {
    /// `aoeCloudSize` (654), metres. Zero or less: no reduction.
    pub cloud: f64,
    /// `aoeVelocity` (653), m/s.
    pub velocity: f64,
    /// `aoeDamageReductionFactor` (1353).
    pub drf: f64,
}

/// One charge type, in the server's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Charge {
    pub type_id: i64,
    /// Its four damages (114/116/117/118), unmodified.
    pub damage: Damage,
    /// Metres per second: 37 times the velocity multiplier.
    pub speed: f64,
    /// Seconds: 281 / 1000 times the flight-time multiplier.
    pub fuse_s: f64,
    pub aoe: Aoe,
    /// The type's own radius, mass and agility (70), for the ball.
    pub radius: f64,
    pub mass: f64,
    pub agility: f64,
}

impl Charge {
    /// `charge_type` read from `td`, its speed times `velocity_mul` and its
    /// fuse times `flight_mul` (1.0 for a player; an entity's 645 and 646).
    /// `None` when the type is missing or cannot fly. Zero damage is the
    /// caller's to refuse.
    pub fn of(
        td: &crate::typedata::Table,
        charge_type: i64,
        velocity_mul: f64,
        flight_mul: f64,
    ) -> Option<Charge> {
        let row = td.get(charge_type)?;
        let a = |id| td.attribute(charge_type, id);
        let speed = a(attr::MAX_VELOCITY) * velocity_mul;
        let fuse_s = a(attr::EXPLOSION_DELAY) / 1000.0 * flight_mul;
        (speed > 0.0 && fuse_s > 0.0).then(|| Charge {
            type_id: charge_type,
            damage: Damage {
                em: a(crate::damage::attr::EM_DAMAGE),
                explosive: a(crate::damage::attr::EXPLOSIVE_DAMAGE),
                kinetic: a(crate::damage::attr::KINETIC_DAMAGE),
                thermal: a(crate::damage::attr::THERMAL_DAMAGE),
            },
            speed,
            fuse_s,
            aoe: Aoe {
                cloud: a(attr::AOE_CLOUD_SIZE),
                velocity: a(attr::AOE_VELOCITY),
                drf: a(attr::AOE_DAMAGE_REDUCTION_FACTOR),
            },
            radius: row.radius,
            mass: row.mass,
            agility: a(attr::AGILITY),
        })
    }

    /// How far it flies before the fuse runs out, in metres.
    #[cfg(test)]
    pub fn reach(&self) -> f64 {
        self.speed * self.fuse_s
    }
}

/// The `OnSpecialFX` a launch sends on the launcher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaunchFx {
    /// The launcher module; an NPC's own id.
    pub module_id: i64,
    /// The launcher's type; an NPC's own type.
    pub module_type_id: i64,
    pub duration_ms: i64,
    pub repeat: i64,
}

/// Everything one launch needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The roster key that launched: a character, an NPC's id, a drone.
    pub shooter: i64,
    /// The launching ship's ball (the NPC itself for an NPC).
    pub launcher_ship: i64,
    /// The combat log's weapon item: the launcher module (the NPC's id).
    pub launcher_item_id: i64,
    /// The slim's `launchModules`: `[module]`, or
    /// `[crate::destiny::NPC_LAUNCH_MODULE]` for an NPC.
    pub launch_modules: Vec<i64>,
    pub charge: Charge,
    /// The volley as it lands before the explosion factor: the charge's
    /// damage as the shooter's dogma modified it.
    pub volley: Damage,
    /// The target ship's ball.
    pub target: i64,
    /// `Some`: this module sends the launch `OnSpecialFX`; `None`: the
    /// caller does.
    pub fx: Option<LaunchFx>,
}

/// Why a launch flew nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The launching ship is not in space.
    NoLauncher,
    /// The target is not a ship in space.
    NoTarget,
    /// The target is in another system.
    OtherSystem,
    /// The launcher is its own target (`MissileFollow` would cancel).
    SameBall,
}

/// One missile in the air.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Flight {
    id: i64,
    shooter: i64,
    launcher_ship: i64,
    launcher_item_id: i64,
    /// Launched by a grouped bank: one missile for the whole bank.
    banked: bool,
    charge_type: i64,
    target: i64,
    volley: Damage,
    aoe: Aoe,
    speed: f64,
    /// `charge radius + target radius`: `MissileFollow`'s range **[V]**.
    hit_dist: f64,
    solar_system_id: i64,
    pos: [f64; 3],
    /// The launch line, flown on the launch tick; `None` once it has been.
    straight: Option<[f64; 3]>,
    /// Where the copy's clock stands, blue time.
    last: BlueTime,
    expires_at: BlueTime,
    /// The target left: no damage, fizzle at the fuse.
    lost: bool,
}

/// Every missile in the air, and the id counter. Not persisted.
#[derive(Debug, Default)]
pub struct Flights {
    flying: Vec<Flight>,
    next: i64,
}

impl Flights {
    /// How many missiles are in the air.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.flying.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flying.is_empty()
    }

    /// The ids in the air, in launch order.
    #[cfg(test)]
    pub fn ids(&self) -> Vec<i64> {
        self.flying.iter().map(|f| f.id).collect()
    }

    /// What the first missile in the air flies with: its speed, its volley,
    /// its explosion and the fuse it was launched with, seconds.
    #[cfg(test)]
    pub fn first(&self) -> Option<(f64, Damage, Aoe, f64)> {
        self.flying.first().map(|f| {
            (
                f.speed,
                f.volley,
                f.aoe,
                (f.expires_at - f.last) as f64 / BLUE_PER_S,
            )
        })
    }

    /// [`Flights::first`] for the missile launched last.
    #[cfg(test)]
    pub fn last_probe(&self) -> Option<(f64, Damage, Aoe, f64)> {
        self.flying.last().map(|f| {
            (
                f.speed,
                f.volley,
                f.aoe,
                (f.expires_at - f.last) as f64 / BLUE_PER_S,
            )
        })
    }

    /// The next free id in the range; wraps, and skips one still flying.
    fn mint(&mut self) -> i64 {
        let span = ID_END - ID_BASE;
        loop {
            let id = ID_BASE + self.next.rem_euclid(span);
            self.next = (self.next + 1).rem_euclid(span);
            if !self.flying.iter().any(|f| f.id == id) {
                return id;
            }
        }
    }
}

/// The [`Flights`] world slot (`world_ext::SLOTS`). Its `make` reads no
/// deltas.
pub const SLOT: SlotSpec = SlotSpec {
    name: "missile",
    make: |_| Box::new(Flights::default()) as Boxed,
};

/// **The explosion factor** **[D]**: `min(1, S/E, ((S/E)·(Ve/Vt))^drf)`.
/// A still target makes the third term infinite; a charge with no cloud
/// (E ≤ 0) does full damage.
pub fn damage_factor(signature: f64, target_speed: f64, aoe: Aoe) -> f64 {
    if aoe.cloud <= 0.0 {
        return 1.0;
    }
    let r = signature.max(0.0) / aoe.cloud;
    let third = if target_speed <= 0.0 {
        f64::INFINITY
    } else {
        (r * aoe.velocity.max(0.0) / target_speed).powf(aoe.drf)
    };
    1.0_f64.min(r).min(third).max(0.0)
}

/// The signature radius of the ship `ship_id` flown by roster entry
/// `roster_key`, **as the dogma engine has it** (a microwarpdrive's bloom, a
/// rig's, a skill's) when the ship has a sheet; the hull table's otherwise.
fn target_signature(w: &Registry, roster_key: i64, ship_id: i64) -> f64 {
    crate::dogma::hull(w, ship_id)
        .map(|h| h.signature_radius)
        .filter(|s| *s > 0.0)
        .unwrap_or_else(|| {
            target_facts(w, roster_key).1 * crate::ewar::signature_factor(w, ship_id)
        })
}

/// A target's radius and signature, from its pilot's hull.
pub(crate) fn target_facts(w: &Registry, roster_key: i64) -> (f64, f64) {
    let Some(p) = w.pilot(roster_key) else {
        return (0.0, DEFAULT_SIGNATURE);
    };
    let h = crate::destiny::hull::of_in(w.content(), p.ship_type_id);
    let sig = if h.signature_radius > 0.0 {
        h.signature_radius
    } else {
        DEFAULT_SIGNATURE
    };
    (f64::from(h.radius), sig)
}

/// **One launch**: an id, the `MissileLaunched` batch (and the FX when
/// `l.fx` is set) on the clock queue, and the copy in the air from `at`.
/// Returns the missile's ball id.
///
/// The batch goes to every character in the system; the grid narrows it to
/// the parks holding both ends (`GridRole::Spawns`). Nobody watching still
/// flies the copy, so a fight between bots lands its damage.
pub fn launch(w: &mut Registry, l: Launch, at: BlueTime) -> Result<i64, Refused> {
    if l.launcher_ship == l.target {
        return Err(Refused::SameBall);
    }
    let launcher = *w
        .roster()
        .by_ship(l.launcher_ship)
        .ok_or(Refused::NoLauncher)?;
    let victim = *w.roster().by_ship(l.target).ok_or(Refused::NoTarget)?;
    if victim.solar_system_id != launcher.solar_system_id {
        return Err(Refused::OtherSystem);
    }
    let system = launcher.solar_system_id;
    let from = launcher.motion.position;
    let toward = victim.motion.position;
    let (target_radius, _) = target_facts(w, victim.character_id);
    let id = w.ext_mut::<Flights>().mint();

    let to = audience(w, system);
    if !to.is_empty() {
        let pilot = w.pilot(l.shooter);
        let owner_id = pilot.map_or(l.shooter, |p| p.owner_id);
        let ball = MissileBall {
            id,
            owner: l.launcher_ship,
            target: l.target,
            from,
            toward,
            radius: l.charge.radius,
            target_radius,
            mass: l.charge.mass,
            max_velocity: l.charge.speed,
            agility: l.charge.agility,
            corporation_id: pilot.map_or(0, |p| p.corporation_id as i32),
            alliance_id: pilot.and_then(|p| p.alliance_id).map_or(-1, |a| a as i32),
        };
        let slim = MissileSlim {
            item_id: id,
            type_id: l.charge.type_id,
            owner_id,
            source_ship_id: l.launcher_ship,
            launch_modules: l.launch_modules.clone(),
        };
        let stamp = w.next_destiny_stamp();
        if let Some(p) = MissileLaunched::new(to.clone(), stamp, ball, slim) {
            w.schedule(Duration::ZERO, p);
        }
        if let Some(fx) = l.fx {
            w.schedule(
                Duration::ZERO,
                crate::modules::SpecialFx {
                    to,
                    ship_id: l.launcher_ship,
                    module_id: fx.module_id,
                    type_id: fx.module_type_id,
                    target_id: Some(l.target),
                    guid: GUID,
                    is_offensive: true,
                    start: true,
                    duration_ms: fx.duration_ms,
                    repeat: fx.repeat,
                },
            );
        }
    }

    let fuse = (l.charge.fuse_s * BLUE_PER_S) as BlueTime;
    w.ext_mut::<Flights>().flying.push(Flight {
        id,
        shooter: l.shooter,
        launcher_ship: l.launcher_ship,
        launcher_item_id: l.launcher_item_id,
        banked: l.launch_modules.len() > 1,
        charge_type: l.charge.type_id,
        target: l.target,
        volley: l.volley,
        aoe: l.charge.aoe,
        speed: l.charge.speed,
        hit_dist: l.charge.radius + target_radius,
        solar_system_id: system,
        pos: from,
        straight: Some(crate::space_state::unit(sub(toward, from)).unwrap_or([0.0; 3])),
        last: at,
        expires_at: at.saturating_add(fuse),
        lost: false,
    });
    Ok(id)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add_scaled(a: [f64; 3], d: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k]
}

/// The distance from `p` to the segment `a..b`.
fn segment_distance(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    let ab = sub(b, a);
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if len2 > 0.0 {
        let ap = sub(p, a);
        ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    crate::space_state::distance(add_scaled(a, ab, t), p)
}

/// What one step of one flight came to.
enum Outcome {
    Flying,
    Hit { signature: f64, speed: f64 },
    Fizzle,
}

/// **The world clock's stage**: fly every copy up to `now`, land the hits
/// through [`crate::combat::missile_volley`] scaled by [`damage_factor`],
/// and remove every missile that ended.
pub fn tick(w: &mut Registry, _: std::time::Instant, now: BlueTime) {
    if w.ext::<Flights>().is_empty() {
        return;
    }
    // Every target's position, brought up to now once per system.
    let mut systems: Vec<i64> = w
        .ext::<Flights>()
        .flying
        .iter()
        .map(|f| f.solar_system_id)
        .collect();
    systems.sort_unstable();
    systems.dedup();
    for s in systems {
        w.advance_system(s);
    }

    let flights = std::mem::take(&mut w.ext_mut::<Flights>().flying);
    let mut keep = Vec::with_capacity(flights.len());
    let mut ended = Vec::new();
    for mut f in flights {
        match step(w, &mut f, now) {
            Outcome::Flying => keep.push(f),
            o => ended.push((f, o)),
        }
    }
    // Launches made while landing (a kill hook) are kept too.
    let slot = w.ext_mut::<Flights>();
    keep.append(&mut slot.flying);
    slot.flying = keep;

    for (f, o) in ended {
        let to = audience(w, f.solar_system_id);
        match o {
            Outcome::Hit { signature, speed } => {
                let k = damage_factor(signature, speed, f.aoe);
                crate::combat::missile_volley_banked(
                    w,
                    MissileVolley {
                        shooter: f.shooter,
                        attacker_ship_id: f.launcher_ship,
                        launcher_item_id: f.launcher_item_id,
                        missile_type_id: f.charge_type,
                        target_ship_id: f.target,
                        damage: scaled(f.volley, k),
                        at: now,
                    },
                    f.banked,
                    now,
                );
                if let Some(p) = BallsRemoved::new(to, 0, vec![f.id]) {
                    w.schedule(HIT_REMOVAL_DELAY, p);
                }
            }
            Outcome::Fizzle => {
                // The pilot is told it missed, as a turret's miss is.
                crate::combat::missile_miss_banked(
                    w,
                    MissileVolley {
                        shooter: f.shooter,
                        attacker_ship_id: f.launcher_ship,
                        launcher_item_id: f.launcher_item_id,
                        missile_type_id: f.charge_type,
                        target_ship_id: f.target,
                        damage: f.volley,
                        at: now,
                    },
                    f.banked,
                    now,
                );
                if let Some(p) = Kill::fizzle(to, 0, f.id) {
                    w.schedule(Duration::ZERO, p);
                }
            }
            Outcome::Flying => {}
        }
    }
}

/// **Warping, as a missile sees it**: the warp has *activated*. The roster
/// stamps a commanded warp with its activation tick in the future (the
/// align time is predicted, `service::space::warp_to_point`), so
/// [`crate::dungeon_tracking::in_warp`] is true for the whole align -- a
/// 30 s crawl for an Orca, in which the ball is still in the open, slow,
/// and in the client's park. Treating that as a warp made every missile at
/// a ship that had just been told to warp "lost" (the owner's 20 at an
/// undocked Orca). Only a stamp that has come due outruns a missile.
fn warp_activated(m: &crate::space_state::Motion) -> bool {
    m.warp_active()
}

/// Carry `f` from its clock to `now` (never past its fuse).
fn step(w: &Registry, f: &mut Flight, now: BlueTime) -> Outcome {
    let until = now.min(f.expires_at);
    if !f.lost {
        let victim = w
            .roster()
            .by_ship(f.target)
            .filter(|v| v.solar_system_id == f.solar_system_id && !warp_activated(&v.motion))
            .copied();
        match victim {
            None => f.lost = true,
            Some(v) if until > f.last => {
                let dt = (until - f.last) as f64 / BLUE_PER_S;
                let goal = v.motion.position;
                let from = f.pos;
                let travel = f.speed * dt;
                f.pos = match f.straight.take() {
                    Some(dir) => add_scaled(from, dir, travel),
                    None => {
                        let gap = crate::space_state::distance(from, goal);
                        match crate::space_state::unit(sub(goal, from)) {
                            Some(dir) => add_scaled(from, dir, travel.min(gap)),
                            None => from,
                        }
                    }
                };
                f.last = until;
                if segment_distance(from, f.pos, goal) <= f.hit_dist {
                    return Outcome::Hit {
                        signature: target_signature(w, v.character_id, v.ship_id),
                        speed: v.motion.speed(),
                    };
                }
            }
            Some(v) => {
                // No time passed: a launcher sitting on its target still hits.
                if crate::space_state::distance(f.pos, v.motion.position) <= f.hit_dist {
                    return Outcome::Hit {
                        signature: target_signature(w, v.character_id, v.ship_id),
                        speed: v.motion.speed(),
                    };
                }
            }
        }
    }
    f.last = f.last.max(until);
    if now >= f.expires_at {
        Outcome::Fizzle
    } else {
        Outcome::Flying
    }
}

/// Each of the four times `k`.
pub(crate) fn scaled(d: Damage, k: f64) -> Damage {
    Damage {
        em: d.em * k,
        explosive: d.explosive * k,
        kinetic: d.kinetic * k,
        thermal: d.thermal * k,
    }
}

/// Every character in space in `system`: who is told. The grid narrows it.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::npc::FIXTURE_RAT;
    use eve_marshal::PyValue;
    use std::time::Instant;

    const ALICE: i64 = 90_045_001;
    const ALICE_SHIP: i64 = 100_045_001;
    const RIFTER: i64 = 587;
    const SYSTEM: i64 = 30_000_001;
    /// Invented: a charge type and its numbers. **Not the client's.**
    const CHARGE: i64 = 990_460_002;
    const SPEED: f64 = 1_000.0;
    const FUSE_S: f64 = 10.0;
    const RADIUS: f64 = 10.0;
    /// The clock's step, as the ticker runs it.
    const DT: BlueTime = 2_500_000;

    fn charge(aoe: Aoe) -> Charge {
        Charge {
            type_id: CHARGE,
            damage: Damage {
                em: 3.0,
                explosive: 5.0,
                kinetic: 7.0,
                thermal: 11.0,
            },
            speed: SPEED,
            fuse_s: FUSE_S,
            aoe,
            radius: RADIUS,
            mass: 500.0,
            agility: 0.001,
        }
    }

    /// Alice undocked in a Rifter, held still, her park bound, and one
    /// entity `offset` metres from her along x, in her park.
    fn world(offset: f64) -> (Registry, i64) {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ship_type_id: RIFTER,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion.velocity = [0.0; 3];
        p.motion.mode = crate::destiny::Mode::Stop;
        p.motion.speed_fraction = 0.0;
        p.since = crate::space_state::now_seconds();
        p.ticked = None;
        let at = p.motion.position;
        let id = crate::npc::spawn(
            &mut reg,
            &FIXTURE_RAT,
            SYSTEM,
            [at[0] + offset, at[1], at[2]],
        )
        .expect("an id");
        crate::grid::reconcile_in(&mut reg, SYSTEM);
        let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
        (reg, id)
    }

    fn shot(npc: i64, aoe: Aoe) -> Launch {
        let c = charge(aoe);
        Launch {
            shooter: npc,
            launcher_ship: npc,
            launcher_item_id: npc,
            launch_modules: vec![crate::destiny::NPC_LAUNCH_MODULE],
            charge: c,
            volley: c.damage,
            target: ALICE_SHIP,
            fx: Some(LaunchFx {
                module_id: npc,
                module_type_id: FIXTURE_RAT.type_id,
                duration_ms: 4_000,
                repeat: 1,
            }),
        }
    }

    fn bars(reg: &Registry) -> Option<crate::damage::ShipDamage> {
        reg.combat().ship(ALICE_SHIP).copied()
    }

    /// Every Destiny entry that left the clock, as `(name, args)`, and the
    /// other pushes' names.
    fn sent(reg: &mut Registry) -> Vec<(String, Vec<PyValue>)> {
        let mut out = Vec::new();
        for p in reg.take_due(Instant::now() + Duration::from_secs(600)) {
            let (name, args) = p.to_push();
            if name != "DoDestinyUpdate" {
                out.push((name.to_owned(), args));
                continue;
            }
            let PyValue::List(es) = &args[0] else {
                panic!("a batch is a list")
            };
            for e in es {
                let PyValue::Tuple(e) = e else {
                    panic!("an entry")
                };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!("(name, args)")
                };
                let (PyValue::Bytes(n), PyValue::Tuple(a)) = (&call[0], &call[1]) else {
                    panic!("(str, tuple)")
                };
                out.push((String::from_utf8_lossy(n).into_owned(), a.clone()));
            }
        }
        out
    }

    fn names(s: &[(String, Vec<PyValue>)]) -> Vec<&str> {
        s.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// Run the stage at `t0 + n·DT` for n in `from..=to`.
    fn run(reg: &mut Registry, t0: BlueTime, from: i64, to: i64) {
        for n in from..=to {
            tick(reg, Instant::now(), t0 + n * DT);
        }
    }

    fn hit_dist(reg: &Registry) -> f64 {
        RADIUS + target_facts(reg, ALICE).0
    }

    /// **The launch** is the ball and `LaunchMissile` in one batch, then the
    /// FX with the charge's guid; the copy is in the air with an id in the
    /// missile range.
    #[test]
    fn a_launch_adds_the_ball_launches_it_and_sends_the_fx() {
        let (mut reg, npc) = world(5_000.0);
        let id = launch(
            &mut reg,
            shot(npc, Aoe::default()),
            crate::pyutil::filetime_now(),
        )
        .expect("launched");
        assert!((ID_BASE..ID_END).contains(&id));
        assert_eq!(reg.ext::<Flights>().ids(), [id]);
        let out = sent(&mut reg);
        assert_eq!(names(&out), ["AddBalls2", "LaunchMissile", "OnSpecialFX"]);
        assert_eq!(out[1].1[0], PyValue::Int(id));
        assert_eq!(out[1].1[1], PyValue::Int(ALICE_SHIP));
        assert_eq!(out[1].1[2], PyValue::Int(npc));
        assert_eq!(out[2].1[5], PyValue::Bytes(GUID.as_bytes().to_vec()));
        assert_eq!(
            launch(
                &mut reg,
                Launch {
                    target: npc,
                    ..shot(npc, Aoe::default())
                },
                0
            ),
            Err(Refused::SameBall)
        );
        assert_eq!(
            launch(
                &mut reg,
                Launch {
                    target: ALICE_SHIP + 1,
                    ..shot(npc, Aoe::default())
                },
                0
            ),
            Err(Refused::NoTarget)
        );
    }

    /// **Hit timing, and no early hit**: a still target `d` away is hit on
    /// tick `ceil((d − hit_dist) / speed / dt)`, not one before; the hit
    /// lands `missile_volley`'s damage, the same as a twin world given the
    /// volley times the factor, and removes the ball with a plain
    /// `RemoveBalls` (the warhead finishes).
    #[test]
    fn a_still_target_is_hit_on_the_predicted_tick_with_the_scaled_volley() {
        let d = 5_000.0;
        let (mut reg, npc) = world(d);
        let (_, sig) = target_facts(&reg, ALICE);
        // A cloud four times the signature: the factor is S/E.
        let aoe = Aoe {
            cloud: sig * 4.0,
            velocity: 100.0,
            drf: 0.5,
        };
        let t0 = crate::pyutil::filetime_now();
        let id = launch(&mut reg, shot(npc, aoe), t0).expect("launched");
        let _ = sent(&mut reg);
        let dt_s = DT as f64 / BLUE_PER_S;
        let n = ((d - hit_dist(&reg)) / SPEED / dt_s).ceil() as i64;
        assert!(n > 1, "the fixture flies more than one tick");

        run(&mut reg, t0, 1, n - 1);
        assert_eq!(bars(&reg), None, "no damage one tick before contact");
        assert_eq!(reg.ext::<Flights>().len(), 1);
        assert!(sent(&mut reg).is_empty());

        run(&mut reg, t0, n, n);
        assert!(reg.ext::<Flights>().is_empty());
        let hit = bars(&reg).expect("Alice was hit");
        let out = sent(&mut reg);
        let removals: Vec<_> = out
            .iter()
            .filter(|(n, _)| n.starts_with("RemoveBall"))
            .collect();
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].0, "RemoveBalls");
        assert_eq!(removals[0].1, [PyValue::List(vec![PyValue::Int(id)])]);

        let (mut twin, twin_npc) = world(d);
        let k = damage_factor(sig, 0.0, aoe);
        assert_eq!(k, sig / aoe.cloud);
        assert!(crate::combat::missile_volley(
            &mut twin,
            MissileVolley {
                shooter: twin_npc,
                attacker_ship_id: twin_npc,
                launcher_item_id: twin_npc,
                missile_type_id: CHARGE,
                target_ship_id: ALICE_SHIP,
                damage: scaled(charge(aoe).damage, k),
                at: t0 + n * DT,
            },
            t0 + n * DT,
        ));
        assert_eq!(Some(hit), bars(&twin));
    }

    /// **A fleeing target** faster than the missile is never hit; at the
    /// fuse the missile goes with `RemoveBall(id, True)` and Alice takes
    /// nothing. The fuse is `FUSE_S` from launch.
    #[test]
    fn a_fleeing_target_is_never_hit_and_the_missile_fizzles_at_its_fuse() {
        let (mut reg, npc) = world(5_000.0);
        let t0 = crate::pyutil::filetime_now();
        let id = launch(&mut reg, shot(npc, Aoe::default()), t0).expect("launched");
        let _ = sent(&mut reg);
        let fuse_ticks = (FUSE_S * BLUE_PER_S) as i64 / DT;
        for n in 1..fuse_ticks {
            if let Some(p) = reg.roster_mut().get_mut(ALICE) {
                // Away from the entity (which sits at +x), twice its speed.
                p.motion.position[0] -= SPEED * 2.0 * DT as f64 / BLUE_PER_S;
            }
            run(&mut reg, t0, n, n);
        }
        assert_eq!(
            reg.ext::<Flights>().len(),
            1,
            "still flying before the fuse"
        );
        assert!(sent(&mut reg).is_empty());
        run(&mut reg, t0, fuse_ticks, fuse_ticks);
        assert!(reg.ext::<Flights>().is_empty());
        // Nothing was taken off, but the pilot is told: the record, if the
        // miss made one, is full, and the message is a hitQuality-0 line.
        if let Some(r) = bars(&reg) {
            let b = r.bars(t0 + fuse_ticks * DT);
            assert_eq!((b.armour, b.hull), (1.0, 1.0));
        }
        let out = sent(&mut reg);
        let msgs: Vec<_> = out.iter().filter(|(n, _)| n == "OnDamageMessage").collect();
        assert_eq!(msgs.len(), 1, "one miss line to the target: {out:?}");
        let PyValue::Dict(m) = &msgs[0].1[0] else {
            panic!("a dict")
        };
        assert!(
            m.iter().any(|(v, k)| *k == PyValue::Bytes(b"hitQuality".to_vec())
                && *v == PyValue::Int(0)),
            "{m:?}"
        );
        let removals: Vec<_> = out
            .iter()
            .filter(|(n, _)| n.starts_with("RemoveBall"))
            .collect();
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].0, "RemoveBall");
        assert_eq!(removals[0].1[0], PyValue::Int(id));
        assert_eq!(removals[0].1[1], PyValue::Bool(true), "terminal: a fizzle");
    }

    /// **A target told to warp is still hittable while it aligns**: the
    /// roster holds its activation stamp in the future, and that is not yet
    /// a warp (the owner's 20 missiles at an undocked Orca all fizzled).
    #[test]
    fn a_target_aligning_for_warp_is_hit() {
        let d = 5_000.0;
        let (mut reg, npc) = world(d);
        let here = reg.roster().get(ALICE).expect("in space").motion.position;
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.mode = crate::destiny::Mode::Warp {
                goto: [here[0] - 1.0e9, here[1], here[2]],
                // Activates in an hour; the ship is still aligning.
                effect_stamp: crate::destiny::tick() as i32 + 3_600,
                total_distance: 1.0e9,
                stop_distance: 0.0,
                warp_factor: 4_000,
            };
        }
        let t0 = crate::pyutil::filetime_now();
        launch(&mut reg, shot(npc, Aoe::default()), t0).expect("launched");
        let _ = sent(&mut reg);
        run(&mut reg, t0, 1, 24);
        assert!(reg.ext::<Flights>().is_empty());
        assert!(bars(&reg).is_some(), "it took the hit");
    }

    /// **The target warps mid-flight**: it takes nothing, even after the
    /// warp ends next to the missile, and the missile is removed at its fuse.
    #[test]
    fn a_target_that_warps_mid_flight_takes_nothing() {
        let d = 8_000.0;
        let (mut reg, npc) = world(d);
        let t0 = crate::pyutil::filetime_now();
        launch(&mut reg, shot(npc, Aoe::default()), t0).expect("launched");
        let _ = sent(&mut reg);
        run(&mut reg, t0, 1, 2);
        let here = reg.roster().get(ALICE).expect("in space").motion.position;
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.mode = crate::destiny::Mode::Warp {
                goto: [here[0] - 1.0e9, here[1], here[2]],
                // Activated now: an hour of warp ahead of it.
                effect_stamp: crate::destiny::tick() as i32,
                total_distance: 1.0e9,
                stop_distance: 0.0,
                warp_factor: 4_000,
            };
        }
        run(&mut reg, t0, 3, 3);
        assert_eq!(reg.ext::<Flights>().len(), 1, "lost, still in the air");
        // Back out of warp, right where the missile was heading.
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.mode = crate::destiny::Mode::Stop;
            p.motion.velocity = [0.0; 3];
            p.motion.position = here;
            p.since = crate::space_state::now_seconds();
            p.ticked = None;
        }
        let fuse_ticks = (FUSE_S * BLUE_PER_S) as i64 / DT;
        run(&mut reg, t0, 4, fuse_ticks - 1);
        assert_eq!(reg.ext::<Flights>().len(), 1);
        run(&mut reg, t0, fuse_ticks, fuse_ticks);
        assert!(reg.ext::<Flights>().is_empty());
        assert_eq!(bars(&reg), None, "a lost missile deals nothing");
        let out = sent(&mut reg);
        assert!(names(&out).contains(&"RemoveBall"));
        assert!(!names(&out).contains(&"RemoveBalls"));
    }

    /// **The factor's three branches** [D]: a big slow target takes it all,
    /// a small one `S/E`, a fast one the power term; no cloud is no
    /// reduction.
    #[test]
    fn the_explosion_factor_has_three_branches() {
        let aoe = Aoe {
            cloud: 40.0,
            velocity: 100.0,
            drf: 0.5,
        };
        assert_eq!(damage_factor(400.0, 0.0, aoe), 1.0);
        assert_eq!(damage_factor(400.0, 50.0, aoe), 1.0);
        assert_eq!(damage_factor(10.0, 0.0, aoe), 10.0 / 40.0);
        let fast = damage_factor(40.0, 1_000.0, aoe);
        assert_eq!(fast, (1.0_f64 * 100.0 / 1_000.0).powf(0.5));
        assert!(fast < 1.0);
        assert_eq!(damage_factor(1.0, 1.0e6, Aoe::default()), 1.0);
    }

    /// **`Charge::of`** reads the flight and the explosion off the type,
    /// with the multipliers; a type that cannot fly is none.
    #[test]
    fn a_charge_reads_its_flight_and_explosion() {
        let td = crate::typedata::Table::parse(
            "type\t990460002\t99112\t8\t0\t1\t1.0\t0.0\t700.0\t30.0\t0\t\t37=1000.0;70=0.002;114=3.0;281=4000.0;653=90.0;654=50.0;1353=0.6\tFixture Missile\n\
type\t990460003\t99112\t8\t0\t1\t1.0\t0.0\t700.0\t30.0\t0\t\t114=3.0\tFixture Brick\n",
        )
        .expect("the invented typedata parses");
        let c = Charge::of(&td, CHARGE, 2.0, 1.5).expect("it flies");
        assert_eq!(c.speed, 1_000.0 * 2.0);
        assert_eq!(c.fuse_s, 4.0 * 1.5);
        assert_eq!(c.reach(), c.speed * c.fuse_s);
        assert_eq!(
            c.aoe,
            Aoe {
                cloud: 50.0,
                velocity: 90.0,
                drf: 0.6
            }
        );
        assert_eq!((c.radius, c.mass, c.agility), (30.0, 700.0, 0.002));
        assert_eq!(c.damage.em, 3.0);
        assert!(Charge::of(&td, CHARGE + 1, 1.0, 1.0).is_none());
        assert!(Charge::of(&td, CHARGE + 2, 1.0, 1.0).is_none());
    }

    /// **Skills change the range and the damage of a real flight** (K-MSKILL).
    /// The same launcher and missile, two pilots: the untrained pilot's
    /// missile runs out of fuse short of a target 5 km away and fizzles; the
    /// trained pilot's flies farther (Missile Bombardment and Projection on
    /// the sheet) and its explosion is tighter (Guided Missile Precision),
    /// so it hits, for the sheet's damage times the explosion factor.
    #[test]
    fn a_trained_pilots_missile_reaches_and_hits_harder_than_an_untrained_ones() {
        use crate::dogma::{Inputs, ModuleInput, State, evaluate};
        // Invented ids; the attribute ids are protocol. The skills are the
        // client's shapes: a per-level bonus attribute, pre-multiplied by
        // the level, post-percent on the charges requiring the skill.
        const ROW: &str = "\
attribute\t37\t0.0\tmaxVelocity
attribute\t1353\t1.0\taoeDamageReductionFactor
attrmeta\t37\t0\t1\t0\t0\t4
attrmeta\t281\t0\t1\t0\t0\t4
attrmeta\t654\t0\t0\t0\t0\t4
type\t990471001\t25\t6\t1\t1\t1000.0\t100.0\t1000000.0\t30.0\t0\t\t9=300.0\tFlight Hull
type\t990471010\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9301\t557=10.0\tFlight Bombardment
type\t990471011\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9302\t547=10.0\tFlight Projection
type\t990471012\t256\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t9303\t848=-5.0\tFlight Precision
type\t990471020\t510\t7\t1\t1\t10.0\t0.9\t0.0\t1.0\t101\t12,101\t51=4000.0;182=990471010.0\tFlight Launcher
type\t990471021\t385\t8\t1\t100\t0.03\t0.0\t1000.0\t10.0\t9\t9\t37=1000.0;117=100.0;281=3000.0;653=100.0;654=400.0;1353=0.5;182=990471010.0;183=990471011.0;184=990471012.0\tFlight Missile
effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t
effect\t9301\t0\t0\t0\t0\t0\t0\t0\tbombardment\t
effect\t9302\t0\t0\t0\t0\t0\t0\t0\tprojection\t
effect\t9303\t0\t0\t0\t0\t0\t0\t0\tprecision\t
modifier\t9301\titemID\tItemModifier\t557\t280\t0\t\t
modifier\t9301\tcharID\tOwnerRequiredSkillModifier\t281\t557\t6\t990471010\t
modifier\t9302\titemID\tItemModifier\t547\t280\t0\t\t
modifier\t9302\tcharID\tOwnerRequiredSkillModifier\t37\t547\t6\t990471011\t
modifier\t9303\titemID\tItemModifier\t848\t280\t0\t\t
modifier\t9303\tcharID\tOwnerRequiredSkillModifier\t654\t848\t6\t990471012\t
";
        let table = crate::typedata::Table::parse(ROW).expect("parses");
        let numbers = |skills: Vec<(i64, i64)>| {
            let sheet = evaluate(
                &table,
                &Inputs {
                    ship_type: 990_471_001,
                    skills,
                    modules: vec![ModuleInput {
                        item_id: 1,
                        type_id: 990_471_020,
                        state: State::Active,
                        charge_type: Some(990_471_021),
                    }],
                    implants: vec![],
                    ..Inputs::default()
                },
            )
            .expect("the hull is in the table");
            let base = Charge::of(&table, 990_471_021, 1.0, 1.0).expect("flies");
            crate::missile_skills::stats_from(&sheet, 1, base, 510)
        };
        let untrained = numbers(vec![]);
        let trained = numbers(vec![(990_471_010, 5), (990_471_011, 3), (990_471_012, 5)]);
        // The sheet moved the numbers the flight reads.
        assert_eq!((untrained.speed, untrained.flight_s), (1_000.0, 3.0));
        assert!((trained.speed - 1_000.0 * 1.3).abs() < 1e-6);
        assert!((trained.flight_s - 3.0 * 1.5).abs() < 1e-6);
        assert!((trained.aoe.cloud - 400.0 * 0.75).abs() < 1e-6);
        assert!(trained.range > 5_000.0 && untrained.range < 4_000.0);

        let d = 5_000.0;
        let launch_with = |s: &crate::missile_skills::Stats| {
            let (mut reg, npc) = world(d);
            let t0 = crate::pyutil::filetime_now();
            let l = Launch {
                charge: s.charge,
                volley: s.volley,
                ..shot(npc, s.charge.aoe)
            };
            launch(&mut reg, l, t0).expect("launched");
            let _ = sent(&mut reg);
            // Fly it to its fuse and a tick beyond; note the tick it ended.
            let end = (s.charge.fuse_s * BLUE_PER_S) as i64 / DT + 2;
            let mut ended = None;
            for n in 1..=end {
                run(&mut reg, t0, n, n);
                if ended.is_none() && reg.ext::<Flights>().is_empty() {
                    ended = Some(t0 + n * DT);
                }
            }
            (reg, ended.expect("the missile ended"))
        };
        let (short, _) = launch_with(&untrained);
        assert!(short.ext::<Flights>().is_empty(), "the fuse ended it");
        assert_eq!(bars(&short), None, "out of fuel short of the target");

        let (far, landed_at) = launch_with(&trained);
        let hit = bars(&far).expect("the trained pilot's missile arrived");
        // The damage is the sheet's volley times the explosion factor with
        // the trained cloud, against Alice's signature and a still target.
        let (_, sig) = target_facts(&far, ALICE);
        let k = damage_factor(sig, 0.0, trained.aoe);
        let (mut twin, twin_npc) = world(d);
        assert!(crate::combat::missile_volley(
            &mut twin,
            MissileVolley {
                shooter: twin_npc,
                attacker_ship_id: twin_npc,
                launcher_item_id: twin_npc,
                missile_type_id: 990_471_021,
                target_ship_id: ALICE_SHIP,
                damage: scaled(trained.volley, k),
                at: landed_at,
            },
            landed_at,
        ));
        assert_eq!(
            Some(hit),
            bars(&twin),
            "the sheet's volley times the factor"
        );
        assert!(k < 1.0 && k > 0.0, "the explosion factor mattered: {k}");
        // And the untrained cloud would have taken less.
        assert!(damage_factor(sig, 0.0, untrained.aoe) < k);
    }

    /// **The id range** clashes with no other id this server mints, and the
    /// counter wraps inside it, skipping an id still in the air.
    #[test]
    fn missile_ids_sit_clear_of_every_other_range_and_wrap() {
        let ours = ID_BASE..ID_END;
        let clear = |lo: i64, hi: i64| hi <= ours.start || lo >= ours.end;
        // Probes count up from their base; a hundred billion is beyond any run.
        assert!(clear(
            crate::scan::PROBE_ID_BASE,
            crate::scan::PROBE_ID_BASE + 100_000_000_000
        ));
        assert!(clear(crate::econ_exchange::NPC_ORDER_BASE, i64::MAX));
        assert!(clear(crate::npc::MIN_NPC_ID, crate::npc::END_NPC_ID));
        assert!(clear(
            crate::planet::FIRST_PIN_ID,
            crate::planet::FIRST_PIN_ID + 100_000_000_000
        ));
        assert!(clear(0, 2_147_483_648), "item, character and type ids");
        assert!(ours.end <= 9_000_000_000_000_000_000, "minFakeItem");

        let mut f = Flights {
            next: ID_END - ID_BASE - 1,
            ..Flights::default()
        };
        assert_eq!(f.mint(), ID_END - 1);
        assert_eq!(f.mint(), ID_BASE);
        f.next = 0;
        f.flying.push(Flight {
            id: ID_BASE,
            shooter: 1,
            launcher_ship: 1,
            launcher_item_id: 1,
            banked: false,
            charge_type: CHARGE,
            target: 2,
            volley: Damage::ZERO,
            aoe: Aoe::default(),
            speed: SPEED,
            hit_dist: 0.0,
            solar_system_id: SYSTEM,
            pos: [0.0; 3],
            straight: None,
            last: 0,
            expires_at: 0,
            lost: false,
        });
        assert_eq!(f.mint(), ID_BASE + 1, "an id in the air is skipped");
    }
}
