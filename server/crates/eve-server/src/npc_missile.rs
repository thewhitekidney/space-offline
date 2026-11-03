//! **NPC missiles** (W17 CP45, C6 N5): what an entity's launcher throws,
//! how long the missile flies, and the landing, which is CP44's
//! [`crate::combat::missile_volley`] -- the same aggression, bars, rings,
//! combat log and death as a gun's volley.
//!
//! # The numbers [V]
//!
//! Every attribute id is CCP's, read by name from the client's
//! `dogmaattributes` (`out/typedata.tsv`'s `attribute` rows, build 3619879):
//!
//! - on the **entity**: `entityMissileTypeID` (507), `missileDamageMultiplier`
//!   (212, default 1.0), `missileLaunchDuration` (506, default 20,000 ms),
//!   `missileEntityVelocityMultiplier` (645, default 1.0) and
//!   `missileEntityFlightTimeMultiplier` (646, default 1.0);
//! - on the **missile type**: `emDamage`/`explosiveDamage`/`kineticDamage`/
//!   `thermalDamage` (114/116/117/118), `maxVelocity` (37) and
//!   `explosionDelay` (281, ms).
//!
//! The entity's launching effect is `missileLaunchingForEntity` (569), guid
//! `effects.MissileDeployment`, duration attribute 506 (the `effect` row of
//! the same extract) **[V]**.
//!
//! # The rules
//!
//! - **The volley** is the missile type's four damages times the entity's
//!   212 (plan row CP45 **[V]**).
//! - **The flight** **[I]** from the attribute names (C6 R2's
//!   `velocity_mul`/`flight_mul`): the missile flies at `37 × 645` m/s for
//!   at most `281 × 646` ms, so its reach is their product.
//!
//! # The flight and the landing (M3)
//!
//! [`launch`] hands the volley to [`crate::missile::launch`], which shows
//! the client a real missile ball (`launchModules == [0]`, the entity's
//! `modules[0]` **[V]**), sends the launch `OnSpecialFX` on the entity's own
//! ball, flies the server's copy, and lands it through
//! [`crate::combat::missile_volley`] when the copy touches the target --
//! scaled by the explosion factor. A target that warps, docks, dies or
//! leaves takes nothing, and the missile fizzles at the end of its fuse.

use crate::damage::{BlueTime, Damage};
use crate::service::Registry;

/// Blue ticks per millisecond.
const BLUE_PER_MS: f64 = 10_000.0;

/// The dogma ids this module reads **[V]** (the module doc names each).
pub mod attr {
    pub const MAX_VELOCITY: i64 = 37;
    pub const EXPLOSION_DELAY: i64 = 281;
    pub const VELOCITY_MULTIPLIER: i64 = 645;
    pub const FLIGHT_TIME_MULTIPLIER: i64 = 646;
}

/// An entity's launcher, in the server's units: everything a launch needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Launcher {
    /// Attribute 507: the missile type, the combat log's weapon.
    pub missile_type_id: i64,
    /// One volley: the missile type's damage times attribute 212.
    pub volley: Damage,
    /// Attribute 506, the launch cycle, in blue ticks, never zero.
    pub period: BlueTime,
    /// `37 × 645`, metres per second, above zero.
    pub speed: f64,
    /// `281 × 646`, the longest flight, in seconds.
    pub flight_s: f64,
}

impl Launcher {
    /// The launcher of an entity whose profile has `m` (`npc_data::Missile`,
    /// attribute 507 > 0), reading the flight multipliers from the entity
    /// `type_id` and the rest from the missile type in `td`. `None` when the
    /// missile cannot fly or does no damage, or its cycle is zero.
    pub fn of(
        td: &crate::typedata::Table,
        type_id: i64,
        m: &crate::npc_data::Missile,
    ) -> Option<Launcher> {
        let missile = m.type_id;
        let a = |id| td.attribute(missile, id);
        let damage = Damage {
            em: a(crate::damage::attr::EM_DAMAGE),
            explosive: a(crate::damage::attr::EXPLOSIVE_DAMAGE),
            kinetic: a(crate::damage::attr::KINETIC_DAMAGE),
            thermal: a(crate::damage::attr::THERMAL_DAMAGE),
        };
        let speed = a(attr::MAX_VELOCITY) * td.attribute(type_id, attr::VELOCITY_MULTIPLIER);
        let flight_s =
            a(attr::EXPLOSION_DELAY) / 1000.0 * td.attribute(type_id, attr::FLIGHT_TIME_MULTIPLIER);
        let period = (m.period_ms.max(0.0) * BLUE_PER_MS) as BlueTime;
        let volley = scaled(damage, m.multiplier);
        (td.get(missile).is_some()
            && speed > 0.0
            && flight_s > 0.0
            && period > 0
            && volley.total() > 0.0)
            .then_some(Launcher {
                missile_type_id: missile,
                volley,
                period,
                speed,
                flight_s,
            })
    }

    /// How far the missile flies before it runs out, in metres.
    pub fn reach(&self) -> f64 {
        self.speed * self.flight_s
    }

    /// How long a volley launched at a target `distance` metres away flies,
    /// in blue ticks; `None` beyond [`Self::reach`].
    #[cfg(test)]
    pub fn flight(&self, distance: f64) -> Option<BlueTime> {
        (distance <= self.reach())
            .then(|| (distance.max(0.0) / self.speed * 1000.0 * BLUE_PER_MS) as BlueTime)
    }
}

/// Each of the four times `m`.
fn scaled(d: Damage, m: f64) -> Damage {
    Damage {
        em: d.em * m,
        explosive: d.explosive * m,
        kinetic: d.kinetic * m,
        thermal: d.thermal * m,
    }
}

/// **One launch** of `launcher` by the entity `npc` at `target`, its cycle
/// boundary `at`: a [`crate::missile::launch`] with the entity's numbers --
/// the FX on its own ball, the missile ball from `launchModules == [0]`, and
/// the server's copy in the air. The charge's radius, mass, agility and
/// explosion come from the content's typedata; with none loaded the missile
/// is a point that does full damage. A target out of reach is launched at
/// anyway and fizzles (R-MISSILE §2.1); the brain launches only inside reach.
pub fn launch(
    w: &mut Registry,
    npc: &crate::npc::Npc,
    launcher: &Launcher,
    target: i64,
    at: BlueTime,
) {
    let mut charge = w
        .content()
        .try_get::<crate::typedata::Installed>()
        .and_then(|td| crate::missile::Charge::of(td.table(), launcher.missile_type_id, 1.0, 1.0))
        .unwrap_or(crate::missile::Charge {
            type_id: launcher.missile_type_id,
            damage: launcher.volley,
            speed: launcher.speed,
            fuse_s: launcher.flight_s,
            aoe: crate::missile::Aoe::default(),
            radius: 0.0,
            mass: 1.0,
            agility: 1.0,
        });
    // The entity's own multipliers are already in the launcher.
    charge.speed = launcher.speed;
    charge.fuse_s = launcher.flight_s;
    // A guidance disruptor on the entity (EWAR-2B).
    let charge = crate::missile_skills::disrupted(charge, crate::ewar::missile_factors(w, npc.id));
    let _ = crate::missile::launch(
        w,
        crate::missile::Launch {
            shooter: npc.id,
            launcher_ship: npc.id,
            launcher_item_id: npc.id,
            launch_modules: vec![crate::destiny::NPC_LAUNCH_MODULE],
            charge,
            volley: launcher.volley,
            target,
            fx: Some(crate::missile::LaunchFx {
                // The entity's own ball, as its gun's FX (`npc::brain::fx`).
                module_id: npc.id,
                module_type_id: npc.kind.type_id,
                duration_ms: launcher.period / BLUE_PER_MS as BlueTime,
                // `[A]` one launch per FX, as the cycle's own repeat.
                repeat: 1,
            }),
        },
        at,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destiny;
    use crate::npc::{FIXTURE_RAT, Npc};
    use crate::npc_data::Missile;
    use std::time::{Duration, Instant};

    const ALICE: i64 = 90_045_001;
    const ALICE_SHIP: i64 = 100_045_001;
    const RIFTER: i64 = 587;
    const SYSTEM: i64 = 30_000_001;
    /// Invented ids: an entity and its missile.
    const ENTITY: i64 = 990_450_001;
    const MISSILE: i64 = 990_450_002;

    /// Invented typedata: the entity's multipliers, the missile's damage,
    /// speed and fuse. **Not the client's numbers.**
    const TYPEDATA: &str = "\
attribute\t645\t1.0\tmissileEntityVelocityMultiplier
attribute\t646\t1.0\tmissileEntityFlightTimeMultiplier
type\t990450001\t99111\t11\t0\t1\t1.0\t0.0\t1.0\t1.0\t569\t569\t212=2.5;506=4000.0;507=990450002.0;645=2.0;646=1.5\tFixture Launcher
type\t990450002\t99112\t8\t0\t1\t1.0\t0.0\t1.0\t1.0\t0\t\t114=3.0;116=5.0;117=7.0;118=11.0;37=1000.0;281=4000.0\tFixture Missile
";

    fn td() -> crate::typedata::Table {
        crate::typedata::Table::parse(TYPEDATA).expect("the invented typedata parses")
    }

    fn missile(td: &crate::typedata::Table) -> Missile {
        Missile {
            type_id: td.attribute(ENTITY, crate::npc_data::dogma::MISSILE_TYPE) as i64,
            multiplier: td.attribute(ENTITY, crate::npc_data::dogma::MISSILE_MULTIPLIER),
            period_ms: td.attribute(ENTITY, crate::npc_data::dogma::MISSILE_PERIOD),
        }
    }

    fn launcher() -> Launcher {
        let td = td();
        Launcher::of(&td, ENTITY, &missile(&td)).expect("a launcher")
    }

    /// **Damage is the missile type's times attribute 212** [V, plan CP45],
    /// each of the four; the cycle is attribute 506.
    #[test]
    fn the_volley_is_the_missile_types_damage_times_212() {
        let td = td();
        let m = missile(&td);
        let l = Launcher::of(&td, ENTITY, &m).expect("a launcher");
        let own = |id| td.attribute(MISSILE, id);
        let k = td.attribute(ENTITY, 212);
        assert_eq!(
            l.volley,
            Damage {
                em: own(114) * k,
                explosive: own(116) * k,
                kinetic: own(117) * k,
                thermal: own(118) * k,
            }
        );
        assert_eq!(l.missile_type_id, MISSILE);
        assert_eq!(
            l.period,
            (td.attribute(ENTITY, 506) * BLUE_PER_MS) as BlueTime
        );
    }

    /// **The flight**: speed is the missile's 37 times the entity's 645,
    /// reach that times 281 × 646; a volley inside reach flies
    /// `distance / speed`, one beyond it never lands.
    #[test]
    fn the_flight_time_is_distance_over_speed_inside_reach() {
        let td = td();
        let l = launcher();
        let speed = td.attribute(MISSILE, 37) * td.attribute(ENTITY, 645);
        let fuse_s = td.attribute(MISSILE, 281) / 1000.0 * td.attribute(ENTITY, 646);
        assert_eq!(l.speed, speed);
        assert_eq!(l.reach(), speed * fuse_s);
        let d = l.reach() / 2.0;
        assert_eq!(
            l.flight(d),
            Some((d / speed * 1000.0 * BLUE_PER_MS) as BlueTime)
        );
        assert_eq!(l.flight(0.0), Some(0));
        assert_eq!(l.flight(l.reach() + 1.0), None);
    }

    /// A missile that cannot fly, or a type the table lacks, is no launcher.
    #[test]
    fn a_missile_that_cannot_fly_is_no_launcher() {
        let td = td();
        let mut m = missile(&td);
        m.type_id = MISSILE + 1;
        assert!(Launcher::of(&td, ENTITY, &m).is_none());
        let mut m = missile(&td);
        m.period_ms = 0.0;
        assert!(Launcher::of(&td, ENTITY, &m).is_none());
    }

    /// Alice undocked in a Rifter, her park bound, and one entity
    /// `offset` metres from her.
    fn world(offset: f64) -> (Registry, Npc) {
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
        destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        // Held still, so the missile's copy chases a target that stays put.
        let p = reg.roster_mut().get_mut(ALICE).expect("in space");
        p.motion.velocity = [0.0; 3];
        p.motion.mode = destiny::Mode::Stop;
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
        let npc = *reg.npcs().get(id).expect("spawned");
        // The grid's reconcile puts the entity in Alice's park, as the clock
        // would: the launch FX goes only to parks that hold its ball.
        crate::grid::reconcile_in(&mut reg, SYSTEM);
        let _ = reg.take_due(Instant::now() + Duration::from_secs(600));
        (reg, npc)
    }

    fn alice_bars(reg: &Registry) -> Option<crate::damage::ShipDamage> {
        reg.combat().ship(ALICE_SHIP).copied()
    }

    /// **The volley lands when the missile arrives** (plan CP45, now
    /// R-MISSILE M3): launched at `t0` at a still target `d` away, nothing
    /// has happened to Alice one clock step before the copy touches her
    /// (`ceil((d − hit_dist) / speed / dt)` steps); at it, her record is
    /// exactly what CP44's `missile_volley` leaves on a twin world given the
    /// same volley -- with no typedata loaded the charge has no explosion
    /// cloud, so the factor is 1.
    #[test]
    fn the_volley_lands_on_arrival_through_missile_volley() {
        let l = launcher();
        let offset = l.reach() / 2.0;
        let (mut reg, npc) = world(offset);
        let t0 = crate::pyutil::filetime_now();
        launch(&mut reg, &npc, &l, ALICE_SHIP, t0);
        assert_eq!(reg.ext::<crate::missile::Flights>().len(), 1);
        let hit_dist = crate::missile::target_facts(&reg, ALICE).0;
        let dt: BlueTime = 2_500_000;
        let n = ((offset - hit_dist) / l.speed / (dt as f64 / 1.0e7)).ceil() as i64;
        assert!(n > 1, "it flies");
        for k in 1..n {
            crate::missile::tick(&mut reg, Instant::now(), t0 + k * dt);
        }
        assert_eq!(alice_bars(&reg), None, "nothing before it arrives");
        assert_eq!(reg.ext::<crate::missile::Flights>().len(), 1);

        let lands = t0 + n * dt;
        crate::missile::tick(&mut reg, Instant::now(), lands);
        assert!(reg.ext::<crate::missile::Flights>().is_empty());
        let hit = alice_bars(&reg).expect("Alice was hit");

        let (mut twin, twin_npc) = world(offset);
        assert!(crate::combat::missile_volley(
            &mut twin,
            crate::combat::MissileVolley {
                shooter: twin_npc.id,
                attacker_ship_id: twin_npc.id,
                launcher_item_id: twin_npc.id,
                missile_type_id: l.missile_type_id,
                target_ship_id: ALICE_SHIP,
                damage: l.volley,
                at: lands,
            },
            lands,
        ));
        assert_eq!(Some(hit), alice_bars(&twin));
    }

    /// EWAR-2B: a guidance disruptor on the entity shortens its missile's
    /// speed and flight.
    #[test]
    fn a_guidance_disruptor_slows_an_entitys_missile() {
        let l = launcher();
        let (mut reg, npc) = world(l.reach() / 2.0);
        launch(
            &mut reg,
            &npc,
            &l,
            ALICE_SHIP,
            crate::pyutil::filetime_now(),
        );
        let (speed, _, _, fuse) = reg
            .ext::<crate::missile::Flights>()
            .first()
            .expect("in the air");
        assert!((speed - l.speed).abs() < 1e-9);
        let (mut reg, npc) = world(l.reach() / 2.0);
        reg.ext_mut::<crate::ewar::Ewars>().factors_for_test(
            npc.id,
            crate::ewar::Factors {
                missile_speed: 0.5,
                missile_fuse: 0.5,
                ..crate::ewar::Factors::default()
            },
        );
        launch(
            &mut reg,
            &npc,
            &l,
            ALICE_SHIP,
            crate::pyutil::filetime_now(),
        );
        let (slow, _, _, short) = reg
            .ext::<crate::missile::Flights>()
            .first()
            .expect("in the air");
        assert!((slow - l.speed * 0.5).abs() < 1e-9);
        assert!((short - fuse * 0.5).abs() < 1e-6, "{short} {fuse}");
    }

    /// **The launch is seen**: one offensive `OnSpecialFX` with the
    /// entity's missile guid on its own ball, aimed at Alice.
    #[test]
    fn a_launch_sends_the_missile_fx() {
        let l = launcher();
        let (mut reg, npc) = world(l.reach() / 2.0);
        launch(
            &mut reg,
            &npc,
            &l,
            ALICE_SHIP,
            crate::pyutil::filetime_now(),
        );
        let fx: Vec<_> = reg
            .take_due(Instant::now() + Duration::from_secs(600))
            .into_iter()
            .map(|p| p.to_push())
            .filter(|(name, _)| *name == "OnSpecialFX")
            .collect();
        assert_eq!(fx.len(), 1);
        let args = &fx[0].1;
        assert_eq!(args[0], eve_marshal::PyValue::Int(npc.id));
        assert_eq!(args[3], eve_marshal::PyValue::Int(ALICE_SHIP));
        assert_eq!(
            args[5],
            eve_marshal::PyValue::Bytes(crate::missile::GUID.as_bytes().to_vec())
        );
        assert_eq!(args[6], eve_marshal::PyValue::Bool(true));
    }

    /// **Out of reach is a miss**: a target beyond reach at launch is
    /// launched at and the missile fizzles at its fuse; one that leaves the
    /// reach before the missile arrives takes nothing either.
    #[test]
    fn a_target_out_of_reach_takes_nothing() {
        let l = launcher();
        let fuse = (l.flight_s * 1.0e7) as BlueTime;
        let t0 = crate::pyutil::filetime_now();

        let (mut reg, npc) = world(l.reach() * 2.0);
        launch(&mut reg, &npc, &l, ALICE_SHIP, t0);
        assert_eq!(reg.ext::<crate::missile::Flights>().len(), 1);
        crate::missile::tick(&mut reg, Instant::now(), t0 + fuse);
        assert!(reg.ext::<crate::missile::Flights>().is_empty());
        assert_eq!(alice_bars(&reg), None);

        let (mut reg, npc) = world(l.reach() / 2.0);
        launch(&mut reg, &npc, &l, ALICE_SHIP, t0);
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.position[0] -= l.reach() * 3.0;
        }
        crate::missile::tick(&mut reg, Instant::now(), t0 + fuse);
        assert!(reg.ext::<crate::missile::Flights>().is_empty());
        assert_eq!(alice_bars(&reg), None);
    }
}
