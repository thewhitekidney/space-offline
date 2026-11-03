//! **Miners defend themselves** (MINER-DEFENSE): a barge, an exhumer or an
//! Orca carries combat drones beside its mining drones; when a rat or a
//! hostile shoots the ship (or one of its drones) the drones go out and
//! engage the attacker, come home when it is dead or before the ship warps,
//! and the ship warps off to dock when the tank is failing, coming back on
//! the next trip.
//!
//! **It wraps, it does not fork** (the way [`crate::drone_use`] does for the
//! fighters): [`mine_in_space`] and [`foreman_in_space`] run the role's own
//! rules and add to what they decided; [`docked`] adds one act before the
//! undock, [`DroneAct::Guard`] ([`stock`]).
//!
//! - **Docked, once a visit.** [`loadout`] reads the hull's drone bay and
//!   bandwidth from the bundle (`eve_server::drones::bay_in`: the served
//!   dogma row does not carry them), less what the mining drones use, and
//!   picks Hammerhead I (medium) when the bay takes [`MIN_MEDIUM`] of them,
//!   else Hobgoblin I (light), at most [`MAX_DEFENCE`]. They are bought in
//!   the closed economy (with Drones, and Light or Medium Drone Operation) and
//!   moved into the bay; an open server's bot is given them (GM). A hull
//!   with no bay (a Merlin), or no bundle to read it from, buys nothing.
//! - **Aggression** ([`attackers`]): an NPC whose gun is on the ship or on
//!   one of its drones (`Hostile::shooting`, from `OnSpecialFX`).
//! - **A fight** ([`judge`], [`Verdict::Fight`]): the drones are launched and
//!   sent at the attacker nearest the ship (kept while it lives), once per
//!   target and again every [`crate::drone_use::TOP_UP_EVERY`] when some are
//!   missing. The miner keeps mining. Nothing attacking and drones out: they
//!   are recalled.
//! - **Flight** ([`Verdict::Flee`]): armour under [`FLEE_ARMOUR`] or hull
//!   under [`FLEE_HULL`]: the drones are recalled and the ship goes home the
//!   way [`Homing::Repair`] goes (the laser stops, then the warp and the
//!   dock); undocked again after the dwell, it is back at a belt.
//! - **Before any warp, dock or reconnect** the combat drones are recalled.
//!
//! Policy [`crate::brain::Policy::combat_drones`] gates all of it (off at
//! [`crate::brain::Policy::fast`]). Every number is **[A]** policy.

use crate::brain::{Act, Homing, Hostile, Memory, Observation, ParkView};
use crate::drone_use::{self, DroneAct};
use crate::{Conn, Error, Result};
use std::time::Instant;

/// Hammerhead I **[V-data]**: the medium drone the defence buys (10 m3,
/// bandwidth 10).
pub const MEDIUM_DRONE: i64 = 2_183;
/// Medium Drone Operation **[V-data]** (33699, needs Drones II).
pub const SKILL_MEDIUM_DRONES: i64 = 33_699;
/// Light Drone Operation **[V-data]**.
pub const SKILL_LIGHT_DRONES: i64 = 24_241;
/// The Drones skill **[V-data]**.
pub const SKILL_DRONES: i64 = 3_436;
/// A medium drone's volume and bandwidth **[V-data]**.
pub const MEDIUM_SIZE: f64 = 10.0;
/// A light drone's bandwidth **[V-data]** (its volume is
/// [`drone_use::DRONE_VOLUME`]); a mining drone's too **[A]**.
pub const LIGHT_BANDWIDTH: f64 = 5.0;
/// The most drones a miner keeps out in its defence. **[A]** five (Drones V);
/// falsified by a Hulk that loses its mining time to a long fight.
pub const MAX_DEFENCE: i64 = 5;
/// The fewest medium drones worth the medium drone's skill. **[A]**
pub const MIN_MEDIUM: i64 = 2;
/// Armour fraction under which the miner leaves. **[A]** half the plates:
/// a Hulk's tank is a few thousand hit points and a rat does hundreds a
/// volley. Falsified by miners lost with armour over this, or leaving at the
/// first volley.
pub const FLEE_ARMOUR: f64 = 0.5;
/// Hull fraction under which the miner leaves. **[A]**
pub const FLEE_HULL: f64 = 0.9;

/// What the hull's bay holds for the defence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loadout {
    /// The drone type to carry.
    pub drone: i64,
    /// The skill that flies it.
    pub skill: i64,
    /// How many.
    pub count: i64,
}

/// **The loadout** (pure): of a bay of `capacity` m3 and `bandwidth` Mbit/s,
/// with `reserved` of each already taken (the mining drones, 5 m3 and 5
/// Mbit/s each), Hammerhead I when at least [`MIN_MEDIUM`] fit, else
/// Hobgoblin I; at most [`MAX_DEFENCE`]. `None`: not even one light drone.
pub fn loadout(capacity: f64, bandwidth: f64, reserved: f64) -> Option<Loadout> {
    let room = (capacity - reserved).max(0.0);
    let band = (bandwidth - reserved).max(0.0);
    let fit = |size: f64, bw: f64| ((room / size).min(band / bw) + 1e-9).floor() as i64;
    let medium = fit(MEDIUM_SIZE, MEDIUM_SIZE).min(MAX_DEFENCE);
    if medium >= MIN_MEDIUM {
        return Some(Loadout {
            drone: MEDIUM_DRONE,
            skill: SKILL_MEDIUM_DRONES,
            count: medium,
        });
    }
    let light = fit(drone_use::DRONE_VOLUME, LIGHT_BANDWIDTH).min(MAX_DEFENCE);
    (light >= 1).then_some(Loadout {
        drone: drone_use::DRONE_I,
        skill: SKILL_LIGHT_DRONES,
        count: light,
    })
}

/// How many of `want` to buy and how many to move into the bay, given
/// `in_bay` and `in_hangar` already.
pub fn plan_counts(want: i64, in_bay: i64, in_hangar: i64) -> drone_use::StockPlan {
    let buy = (want - in_bay - in_hangar).max(0);
    let moves = (want - in_bay).max(0).min(in_hangar + buy);
    drone_use::StockPlan { buy, moves }
}

/// The NPCs shooting the ship or one of its drones.
pub fn attackers<'a>(m: &Memory, park: &'a ParkView) -> Vec<&'a Hostile> {
    park.hostiles
        .iter()
        .filter(|h| {
            h.shooting.is_some_and(|t| {
                t == park.ego.id || m.combat.out.contains(&t) || m.drones.contains(&t)
            })
        })
        .collect()
}

/// What the miner does about its situation this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing attacks it.
    Calm,
    /// Something does: the drones go at this ball.
    Fight(i64),
    /// The tank is failing: leave.
    Flee,
}

/// Whether the ship's bars say go.
pub fn failing(health: Option<crate::brain::Health>) -> bool {
    health.is_some_and(|h| h.armour < FLEE_ARMOUR || h.hull < FLEE_HULL)
}

/// **The verdict** (pure).
pub fn judge(m: &Memory, o: &Observation, park: &ParkView) -> Verdict {
    if failing(o.health) || m.homing == Some(Homing::Repair) {
        return Verdict::Flee;
    }
    let hit = attackers(m, park);
    // The target stays while it lives, shooting or not.
    if let Some(t) = m
        .combat
        .engaged
        .filter(|t| park.hostiles.iter().any(|h| h.ball.id == *t))
        && !hit.is_empty()
    {
        return Verdict::Fight(t);
    }
    // BOT-SCRAM: a scrambler on grid is the one to kill, shooting or not.
    if o.scram.scrambled
        && let Some(s) = o.scram.sources.iter().copied().find(|s| {
            !m.scram.as_ref().is_some_and(|e| e.fought.contains(s))
                && park.hostiles.iter().any(|h| h.ball.id == *s)
        })
    {
        return Verdict::Fight(s);
    }
    match hit.into_iter().min_by(|a, b| {
        park.ego
            .surface_to(&a.ball)
            .total_cmp(&park.ego.surface_to(&b.ball))
            .then(a.ball.id.cmp(&b.ball.id))
    }) {
        Some(h) => Verdict::Fight(h.ball.id),
        None => Verdict::Calm,
    }
}

/// Whether an act takes the ship away from where its drones are.
fn leaves(a: &Act) -> bool {
    matches!(
        a,
        Act::WarpToItem { .. } | Act::WarpToMember { .. } | Act::Dock(_) | Act::Reconnect
    )
}

/// **The steering** (pure): the drones around `acts`.
pub fn steer(m: &Memory, now: Instant, warping: bool, v: Verdict, acts: Vec<Act>) -> Vec<Act> {
    let c = &m.combat;
    let out_now = !c.out.is_empty();
    let mut lead = Vec::new();
    // BOT-SCRAM: scrambled, no warp will happen, so a leaving act does not
    // call the drones home.
    let leaving = m.scram.is_none() && acts.iter().any(leaves);
    match v {
        Verdict::Fight(t) if !warping && !leaving => {
            let missing = (c.out.len() as i64) < c.want_out() && !c.dry;
            let due = c
                .launched_at
                .is_none_or(|l| now.saturating_duration_since(l) >= drone_use::TOP_UP_EVERY);
            let new_target = c.engaged != Some(t);
            if new_target && !(c.dry && c.out.is_empty()) || missing && due {
                lead.push(Act::Drone(DroneAct::Engage(t)));
            }
        }
        _ => {
            if out_now {
                lead.push(Act::Drone(DroneAct::Recall));
            }
        }
    }
    lead.extend(acts);
    lead
}

/// Whether this bot defends itself: the policy, and a ship that can have
/// drones at all is found out when the bay is stocked.
fn on(m: &Memory) -> bool {
    m.policy.combat_drones
}

/// The shared in-space step: `core` runs the role's rules, `flee` says what
/// leaving means for this role.
fn guarded(
    m: &mut Memory,
    o: &Observation,
    core: fn(&mut Memory, &Observation) -> Vec<Act>,
    flee: fn(&mut Memory, &Observation, &ParkView) -> Option<Vec<Act>>,
) -> Vec<Act> {
    // BOT-SCRAM: first, before the role's rules and the flight.
    if let Some(acts) = crate::scram::decide(m, o) {
        return acts;
    }
    let Some(park) = o.park.as_ref().filter(|_| on(m)) else {
        return core(m, o);
    };
    m.combat.stocked = false;
    let v = judge(m, o, park);
    let warping = park.ego_warping;
    if v == Verdict::Flee {
        if m.homing.is_none() {
            m.homing = Some(Homing::Repair);
        }
        if m.homing == Some(Homing::Repair) {
            m.home_why = Some((
                Homing::Repair,
                match o.health {
                    Some(h) => format!(
                        "repair: armour {:.2} (flees under {FLEE_ARMOUR}), hull {:.2} (under {FLEE_HULL})",
                        h.armour, h.hull
                    ),
                    None => "repair: no tank reading".to_owned(),
                },
            ));
        }
        if let Some(acts) = flee(m, o, park) {
            return steer(m, o.now, warping, v, acts);
        }
    }
    let acts = core(m, o);
    steer(m, o.now, warping, v, acts)
}

/// **The miner's** in-space rules ([`crate::brain::in_space`]), defended.
/// Fleeing is the miner's own way home: its rules see [`Homing::Repair`].
pub fn mine_in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    guarded(m, o, crate::brain::in_space, |_, _, _| None)
}

/// **The foreman's** in-space rules, defended. The foreman has no way home
/// of its own: fleeing is [`crate::brain::go_home`], and it stays home-bound
/// until docked ([`foreman_docked`]).
pub fn foreman_in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    guarded(m, o, crate::foreman::in_space_core, |m, o, park| {
        let _ = m.homing; // set by the caller
        Some(crate::brain::go_home(m, o, park))
    })
}

/// **Docked**: `core`'s acts, and, once a visit just before the undock, the
/// defence drones stocked.
pub fn docked(
    m: &mut Memory,
    o: &Observation,
    core: fn(&mut Memory, &Observation) -> Vec<Act>,
) -> Vec<Act> {
    if !on(m) {
        return core(m, o);
    }
    m.combat.out.clear();
    m.combat.engaged = None;
    m.combat.dry = false;
    let acts = core(m, o);
    if !m.combat.stocked && acts.contains(&Act::Undock) {
        m.combat.stocked = true;
        let mut out = vec![Act::Drone(DroneAct::Guard)];
        out.extend(acts);
        return out;
    }
    acts
}

/// The miner's docked rules, with the defence stocked.
pub fn mine_docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    docked(m, o, crate::brain::docked)
}

/// The foreman's docked rules, with the defence stocked; a flight is over.
pub fn foreman_docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    docked(m, o, crate::foreman::docked_core)
}

// ---- the arm (I/O) ----------------------------------------------------------

/// **Docked: stock the defence** ([`DroneAct::Guard`]). Soft: no bay, no
/// bundle, a bay without room and a market without drones are notes.
pub fn stock(c: &mut Conn, m: &mut Memory, ctx: drone_use::Ctx<'_>) -> Result<drone_use::Done> {
    let mut done = drone_use::Done {
        docked_dirty: true,
        ..drone_use::Done::default()
    };
    let (Some(ship), Some(station)) = (c.session().ship_id(), c.session().station_id()) else {
        return Err(Error::NotSent("defend: stock: not docked in a ship".into()));
    };
    let hull = c
        .hangar_items()?
        .iter()
        .find(|r| r.item_id == ship)
        .map(|r| r.type_id);
    let bay = match (ctx.content, hull) {
        (Some(content), Some(h)) => eve_server::drones::bay_in(content, h),
        _ => None,
    };
    let Some(bay) = bay else {
        done.note = Some(
            if ctx.content.is_none() {
                "defence drones: no bundle to read the bay from"
            } else {
                "defence drones: no drone bay"
            }
            .to_owned(),
        );
        return Ok(done);
    };
    let in_bay = c.ship_items(ship, Some(drone_use::FLAG_DRONE_BAY))?;
    let mining: i64 = in_bay
        .iter()
        .filter(|r| crate::ore::is_mining_drone(r.type_id))
        .map(|r| r.quantity.max(1))
        .sum();
    let reserved = mining as f64 * drone_use::DRONE_VOLUME;
    let Some(load) = loadout(bay.capacity, bay.bandwidth, reserved) else {
        done.note = Some("defence drones: the bay has no room beside the mining drones".to_owned());
        return Ok(done);
    };
    m.combat.want = load.count;
    let of_type = |rows: &[crate::ItemRow]| -> i64 {
        rows.iter()
            .filter(|r| r.type_id == load.drone)
            .map(|r| r.quantity.max(1))
            .sum()
    };
    let hangar = c.hangar_items()?;
    let plan = plan_counts(load.count, of_type(&in_bay), of_type(&hangar));
    if plan.buy > 0 {
        if ctx.closed {
            let mut order = crate::acquire::Order::default();
            order
                .want_skill(SKILL_DRONES, load.count.min(drone_use::WANT_DRONES))
                .want_skill(load.skill, 1)
                .want_module(load.drone, load.count);
            let got = crate::acquire::buy_order(c, &order, ctx.trips)?;
            done.spent = (got.before - got.after).max(0.0);
            done.note = Some(format!("defence drones: {}", got.summary()));
        } else if !m.combat.gm_given {
            m.combat.gm_given = true;
            for line in [
                format!("/giveskill {SKILL_DRONES} {}", load.count.min(5)),
                format!("/giveskill {} 1", load.skill),
                format!("/createitem {} {}", load.drone, plan.buy),
            ] {
                let outcome = crate::gm::slash(c, &line)?;
                done.slashes.push((line, outcome.refusal()));
            }
        }
    }
    let drone = load.drone;
    if let Some(note) = drone_use::move_to_bay(c, ship, station, plan.moves, &|t| t == drone)? {
        done.note = Some(note);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, Health, Policy};
    use std::collections::BTreeMap;
    use std::time::Duration;

    const SHIP: i64 = 8_200_000_001;
    const RAT: i64 = 8_200_000_010;
    const RAT_2: i64 = 8_200_000_011;
    const OUR_DRONE: i64 = 8_200_000_901;

    fn ball(id: i64, x: f64) -> Ball {
        Ball {
            id,
            position: [x, 0.0, 0.0],
            radius: 50.0,
        }
    }

    fn rat(id: i64, x: f64, shooting: Option<i64>) -> Hostile {
        Hostile {
            ball: ball(id, x),
            type_id: 8_200_099,
            name: "Invented Rat".to_owned(),
            shooting,
        }
    }

    fn memory(on: bool) -> Memory {
        let p = Policy {
            role: &crate::roles::MINE,
            combat_drones: on,
            ..Policy::fast()
        };
        Memory::new("defend-test", p)
    }

    fn scene(hostiles: Vec<Hostile>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.park = Some(ParkView {
            ego: ball(SHIP, 0.0),
            ego_warping: false,
            rocks: Vec::new(),
            belts: Vec::new(),
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles,
            wrecks: Vec::new(),
            ego_tether: None,
        });
        o
    }

    fn health(armour: f64, hull: f64) -> Option<Health> {
        Some(Health {
            shield: 0.0,
            armour,
            hull,
        })
    }

    fn wait() -> Vec<Act> {
        vec![Act::Wait(Duration::from_secs(1))]
    }

    fn verdict(m: &Memory, o: &Observation) -> Verdict {
        judge(m, o, o.park.as_ref().expect("park"))
    }

    #[test]
    fn a_bay_beside_mining_drones_picks_medium_or_light_by_room() {
        // Room for plenty of both: medium, capped.
        let l = loadout(50.0, 50.0, 5.0).expect("a loadout");
        assert_eq!((l.drone, l.skill), (MEDIUM_DRONE, SKILL_MEDIUM_DRONES));
        assert_eq!(l.count, 4);
        let l = loadout(500.0, 500.0, 0.0).expect("a loadout");
        assert_eq!(l.count, MAX_DEFENCE);
        // Bandwidth, not volume, limits it.
        let l = loadout(50.0, 25.0, 5.0).expect("a loadout");
        assert_eq!((l.drone, l.count), (MEDIUM_DRONE, 2));
        // Room for one medium only: lights instead.
        let l = loadout(15.0, 15.0, 0.0).expect("a loadout");
        assert_eq!((l.drone, l.skill), (drone_use::DRONE_I, SKILL_LIGHT_DRONES));
        assert_eq!(l.count, 3);
        // A rookie ship's 5 m3 with a mining drone in it: nothing.
        assert_eq!(loadout(5.0, 5.0, 5.0), None);
        assert_eq!(loadout(0.0, 0.0, 0.0), None);
    }

    #[test]
    fn the_plan_buys_what_is_short_and_moves_what_the_bay_lacks() {
        let p = plan_counts(4, 1, 1);
        assert_eq!((p.buy, p.moves), (2, 3));
        let p = plan_counts(4, 4, 0);
        assert_eq!((p.buy, p.moves), (0, 0));
        let p = plan_counts(3, 0, 5);
        assert_eq!((p.buy, p.moves), (0, 3));
    }

    #[test]
    fn only_an_npc_shooting_the_ship_or_its_drones_is_an_attacker() {
        let mut m = memory(true);
        m.combat.out = vec![OUR_DRONE];
        let o = scene(vec![
            rat(RAT, 3_000.0, Some(SHIP)),
            rat(RAT_2, 5_000.0, Some(OUR_DRONE)),
            rat(RAT_2 + 1, 6_000.0, None),
            rat(RAT_2 + 2, 7_000.0, Some(1)),
        ]);
        let park = o.park.as_ref().expect("park");
        let ids: Vec<i64> = attackers(&m, park).iter().map(|h| h.ball.id).collect();
        assert_eq!(ids, vec![RAT, RAT_2]);
    }

    #[test]
    fn aggression_picks_the_nearest_attacker_and_keeps_it_while_it_lives() {
        let mut m = memory(true);
        let o = scene(vec![
            rat(RAT, 9_000.0, Some(SHIP)),
            rat(RAT_2, 3_000.0, Some(SHIP)),
        ]);
        assert_eq!(verdict(&m, &o), Verdict::Fight(RAT_2));
        // Engaged on the far one: it stays the target while it is alive.
        m.combat.engaged = Some(RAT);
        assert_eq!(verdict(&m, &o), Verdict::Fight(RAT));
        // Nothing shooting: calm.
        let calm = scene(vec![rat(RAT, 9_000.0, None)]);
        assert_eq!(verdict(&m, &calm), Verdict::Calm);
        assert_eq!(verdict(&m, &scene(Vec::new())), Verdict::Calm);
    }

    #[test]
    fn a_fight_launches_and_engages_once_and_the_miner_keeps_mining() {
        let mut m = memory(true);
        let now = Instant::now();
        let acts = steer(&m, now, false, Verdict::Fight(RAT), wait());
        assert_eq!(
            acts,
            [vec![Act::Drone(DroneAct::Engage(RAT))], wait()].concat()
        );
        // Sent: no more orders for the same target.
        m.combat.out = vec![1, 2, 3];
        m.combat.want = 3;
        m.combat.engaged = Some(RAT);
        m.combat.launched_at = Some(now);
        assert_eq!(steer(&m, now, false, Verdict::Fight(RAT), wait()), wait());
        // Another target is a new order.
        let next = steer(&m, now, false, Verdict::Fight(RAT_2), wait());
        assert_eq!(next[0], Act::Drone(DroneAct::Engage(RAT_2)));
        // A lost drone is replaced, but only at the pace.
        m.combat.out = vec![1];
        assert_eq!(steer(&m, now, false, Verdict::Fight(RAT), wait()), wait());
        let later = now + drone_use::TOP_UP_EVERY;
        let again = steer(&m, later, false, Verdict::Fight(RAT), wait());
        assert_eq!(again[0], Act::Drone(DroneAct::Engage(RAT)));
        // An empty bay is not asked again.
        m.combat.out.clear();
        m.combat.dry = true;
        assert_eq!(steer(&m, later, false, Verdict::Fight(RAT), wait()), wait());
    }

    #[test]
    fn drones_are_recalled_when_calm_and_before_any_warp() {
        let mut m = memory(true);
        let now = Instant::now();
        assert_eq!(steer(&m, now, false, Verdict::Calm, wait()), wait());
        m.combat.out = vec![1, 2];
        let calm = steer(&m, now, false, Verdict::Calm, wait());
        assert_eq!(calm[0], Act::Drone(DroneAct::Recall));
        // A fight and a warp in one turn: recall, never engage.
        let warp = vec![Act::WarpToItem { id: 9, range: 0.0 }];
        let acts = steer(&m, now, false, Verdict::Fight(RAT), warp.clone());
        assert_eq!(acts, [vec![Act::Drone(DroneAct::Recall)], warp].concat());
        // No launching while the ship is in warp.
        let acts = steer(&memory(true), now, true, Verdict::Fight(RAT), wait());
        assert_eq!(acts, wait());
        // Fleeing recalls too.
        let acts = steer(&m, now, false, Verdict::Flee, wait());
        assert_eq!(acts[0], Act::Drone(DroneAct::Recall));
    }

    #[test]
    fn a_failing_tank_flees_and_a_sound_one_fights() {
        let mut m = memory(true);
        let mut o = scene(vec![rat(RAT, 3_000.0, Some(SHIP))]);
        o.health = health(FLEE_ARMOUR + 0.1, 1.0);
        assert_eq!(verdict(&m, &o), Verdict::Fight(RAT));
        o.health = health(FLEE_ARMOUR - 0.1, 1.0);
        assert_eq!(verdict(&m, &o), Verdict::Flee);
        o.health = health(1.0, FLEE_HULL - 0.1);
        assert_eq!(verdict(&m, &o), Verdict::Flee);
        // Once leaving, it keeps leaving.
        o.health = None;
        m.homing = Some(Homing::Repair);
        assert_eq!(verdict(&m, &o), Verdict::Flee);
    }

    #[test]
    fn the_wrapper_sets_the_flight_off_recalls_first_and_does_nothing_when_off() {
        fn core(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Wait(Duration::from_secs(1))]
        }
        let mut m = memory(true);
        m.combat.out = vec![OUR_DRONE];
        let mut o = scene(vec![rat(RAT, 3_000.0, Some(SHIP))]);
        o.health = health(0.1, 1.0);
        let acts = guarded(&mut m, &o, core, |_, _, _| None);
        assert_eq!(m.homing, Some(Homing::Repair));
        assert_eq!(acts[0], Act::Drone(DroneAct::Recall));
        // Policy off: the core's acts, untouched, homing untouched.
        let mut off = memory(false);
        let acts = guarded(&mut off, &o, core, |_, _, _| None);
        assert_eq!(acts, wait());
        assert_eq!(off.homing, None);
    }

    /// BOT-CHURN: a fresh ship (no tank reading yet) or a whole one never
    /// flees; one that does flee says why, with the bars, for the dock's
    /// act-log note.
    #[test]
    fn a_fresh_or_whole_ship_stays_and_a_fleeing_one_names_its_bars() {
        fn core(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Wait(Duration::from_secs(1))]
        }
        for health_now in [None, health(1.0, 1.0)] {
            let mut m = memory(true);
            let mut o = scene(vec![]);
            o.health = health_now;
            let acts = guarded(&mut m, &o, core, |_, _, _| None);
            assert_eq!(acts, wait());
            assert_eq!(m.homing, None);
        }
        let mut m = memory(true);
        let mut o = scene(vec![]);
        o.health = health(FLEE_ARMOUR - 0.1, 1.0);
        guarded(&mut m, &o, core, |_, _, _| None);
        let why = m.dock_reason();
        assert!(why.starts_with("repair: armour 0."), "{why}");
    }

    #[test]
    fn a_hull_with_no_bay_or_no_room_buys_nothing_and_docking_stocks_once() {
        fn core(_: &mut Memory, _: &Observation) -> Vec<Act> {
            vec![Act::Undock]
        }
        let mut m = memory(true);
        m.combat.out = vec![1];
        let o = Observation::empty(Instant::now());
        let acts = docked(&mut m, &o, core);
        assert_eq!(acts, vec![Act::Drone(DroneAct::Guard), Act::Undock]);
        assert!(m.combat.out.is_empty());
        // Once a visit.
        assert_eq!(docked(&mut m, &o, core), vec![Act::Undock]);
        // Off: nothing added.
        let mut off = memory(false);
        assert_eq!(docked(&mut off, &o, core), vec![Act::Undock]);
        // A Merlin's no bay is no loadout.
        assert_eq!(loadout(0.0, 0.0, 0.0), None);
    }
}
