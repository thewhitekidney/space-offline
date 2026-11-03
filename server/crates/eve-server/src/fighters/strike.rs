//! **Heavy fighters' bombs and kamikaze** (FT-G, `docs/lanes/ft-gh.md`; spec
//! R-DEEP §2.3).
//!
//! This build has both: ability 7 *Launch bomb* (effect 6485, point-targeted,
//! three charges or more, banned in high and low security space; 16 types)
//! and ability 38 *True Sacrifice* (effect 6554, item-targeted; two types)
//! [V-data, `fighters/abilities.tsv`, `slots.tsv`]. Neither effect has a
//! modifier, and the client has no code that runs them: what it does have is
//! the **NPC behaviour tree** of the kamikaze (`behaviors/actions/combat.py`
//! `KamikazeAttack`), which is the one read source of the mechanics below.
//!
//! # Kamikaze [V `behaviors/actions/combat.py:83-193`]
//!
//! The squadron approaches its target with a distance of "minus a great
//! deal" (it flies *into* it, `ApproachObject(approachRange=-999999.0)`);
//! within `fighterAbilityKamikazeRange` (2330) of the target's surface it
//! becomes a missile (`MissileFollow`, `SetBallMassive(False)`); when the
//! surface distance reaches zero or it collides with the target it waits
//! **one second** (`explosionDelaySeconds = 1.0`) and detonates. The damage
//! is `squadronSize * cloudFactor` times the four damages, the cloud being
//! `fighterAbilityKamikazeSignatureRadius` (2329) (`_ApplyDamageToTarget`,
//! `GetCloudFactor`; `dogma/effects/util.CalcCloudFactorModifier`: a cloud of
//! 1 m or less is no reduction) and the squadron is exploded
//! (`ExplodeEx`). A target that goes away makes it stop
//! (`Abort`).
//!
//! # Bomb
//!
//! The ability's type attribute 2324 (`fighterAbilityLaunchBombType`) names a
//! bomb charge type (group "bomb", category charge [V-data]); its duration
//! attribute 2349 is the cycle. Charges, the rearm time and the security
//! bans are the generic slot rules (`ability.rs`).
//!
//! # `[A]`, and why each could not be found
//!
//! Everything the client does not hold, which is the *server's* behaviour
//! (`StartFighterAbilityDogmaEffect` is not in the client):
//!
//! 1. **A player's kamikaze runs the NPC tree's steps** (approach, one
//!    second at contact, detonate). The effect (6554) is offensive with a
//!    duration (2401) and no modifier; what the server does on activation is
//!    not readable. Falsifier: a player-activated squadron that does not fly
//!    into its target, or that detonates at once.
//! 2. **The fourth damage term is `fighterAbilityKamikazeDamageTherm` (2326).**
//!    The client's tree reads the *thermal damage* attribute (118, which no
//!    fighter carries) for it, which reads as a slip: it would make CCP's own
//!    NPC kamikaze deal no thermal damage. Falsifier: a retail kamikaze's
//!    combat log with no thermal part.
//! 3. **The cloud factor** is `min(1, signature / cloud)` (the missile
//!    explosion factor with an infinite explosion velocity,
//!    `missile::damage_factor`): `GetCloudFactor` is server code.
//!    Falsifier: damage against a target smaller than the cloud.
//! 4. **The target's resistance** is applied as
//!    `util.CalcResistanceModifier` does for an effect: the squadron's
//!    attribute 2432 names a target attribute (2433 on the shipped types)
//!    whose value multiplies the damage; 1 when it names none [V
//!    `dogma/effects/util.py:73-77`]. That the kamikaze uses it is not read.
//!    Falsifier: a ship with that attribute below 1 takes full damage.
//! 5. **Bomb flight**: the bomb flies at its type's `maxVelocity` (37) from
//!    the squadron toward the chosen point, never further than `maxVelocity`
//!    times its `explosionDelay` (281) (the point is shortened to the bomb's
//!    reach), and detonates when it gets there. No ball is shown for the
//!    flight (the missile model is homing, `missile.rs`). Falsifier: a bomb
//!    seen in flight in the client, or one that explodes at a different time
//!    or place.
//! 6. **Bomb blast**: every ship, drone and squadron within the bomb's
//!    `explosionRange` (107; `empFieldRange` 99 where that is absent) of the
//!    point takes `damage x squadronSize` through the missile explosion
//!    factor (signature 552 against `aoeCloudSize` 654, `aoeVelocity` 653,
//!    `aoeDamageReductionFactor` 1353), except what the **owner** owns. The
//!    owner's own ship is spared, other pilots' ships (a fleet-mate's
//!    included) are not. Falsifier: a bomb damaging its own squadron, or
//!    sparing a fleet mate.
//! 7. **A squadron's skills do not reach a bomb**: the bomb is a charge of
//!    the squadron's owner and its damage is its type's (a bomb launcher's
//!    ignores the pilot's missile damage multiplier, `missile_skills`).
//!    Falsifier: a skilled pilot's bomb deals more than a fresh one's.

use super::ability::{Active, Cycle};
use super::*;
use crate::combat;
use crate::damage::{Damage, Shot};
use crate::fighters_data::effect;
use crate::missile::{self, Aoe};

/// The type attribute naming the bomb charge [V-data
/// `fighterAbilityLaunchBombType`].
pub const BOMB_TYPE: i64 = 2324;
/// `fighterAbilityKamikaze*` [V-data]: damage EM, thermal, kinetic,
/// explosive, the cloud (signature radius) and the resistance id. The range
/// attribute (2330) is the effect's own range, which turns a squadron into a
/// missile in the client's tree [V]; here a squadron flies at its own speed all
/// the way.
pub const KAMIKAZE_DAMAGE: [i64; 4] = [2325, 2326, 2327, 2328];
pub const KAMIKAZE_CLOUD: i64 = 2329;
pub const KAMIKAZE_RESISTANCE: i64 = 2432;
/// A bomb type's own attributes [V-data]: `maxVelocity`, `explosionDelay`
/// (ms), `explosionRange`, `empFieldRange`, the damages (EM, thermal,
/// kinetic, explosive) and the application three.
pub const BOMB_SPEED: i64 = 37;
pub const BOMB_FUSE: i64 = 281;
pub const BOMB_RANGE: i64 = 107;
pub const BOMB_EMP_RANGE: i64 = 99;
pub const BOMB_DAMAGE: [i64; 4] = [114, 118, 117, 116];
/// `explosionDelaySeconds` of the kamikaze task [V `combat.py:84`].
pub const KAMIKAZE_DELAY_MS: i64 = 1_000;

/// Whether this build runs the effect.
pub fn runs(effect_id: i64) -> bool {
    matches!(effect_id, effect::BOMB | effect::KAMIKAZE)
}

/// A bomb in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct Bomb {
    pub owner: i64,
    pub source: i64,
    pub system: i64,
    pub type_id: i64,
    pub at: [f64; 3],
    pub due: BlueTime,
    /// The squadron's size when it launched.
    pub size: i64,
}

/// A kamikaze run: the squadron is flying into `target`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    pub target: i64,
    /// When the surface distance first reached zero.
    pub contact: Option<BlueTime>,
}

/// Every bomb in flight and every kamikaze run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strikes {
    bombs: Vec<Bomb>,
    runs: BTreeMap<i64, Run>,
}

impl Strikes {
    #[cfg(test)]
    pub fn bombs(&self) -> &[Bomb] {
        &self.bombs
    }

    #[cfg(test)]
    pub fn run_of(&self, id: i64) -> Option<&Run> {
        self.runs.get(&id)
    }
}

/// A squadron is gone or let go: its kamikaze run is off. (A bomb it threw
/// still flies.)
pub(super) fn forget(w: &mut Registry, id: i64) {
    state_mut(w).strikes.runs.remove(&id);
}

/// A kamikaze is called off: the run ends and the squadron stops where it is
/// (`KamikazeAttack.Abort`: `StopEx`, the target tracking off [V]).
pub(super) fn abort(w: &mut Registry, id: i64) {
    if state_mut(w).strikes.runs.remove(&id).is_none() {
        return;
    }
    let Some(s) = state(w).squadron(id).copied() else {
        return;
    };
    if let Some(r) = state_mut(w).rt.get_mut(&id) {
        r.order = super::ability::Order::Stop;
    }
    let stop = Action::Stop { ball: id };
    if let Some(p) = w.roster_mut().get_mut(id) {
        p.motion.after(stop);
    }
    send_actions(w, Via::Answer, s.solar_system_id, vec![stop]);
}

fn minus(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn times(d: Damage, k: f64) -> Damage {
    Damage {
        em: d.em * k,
        thermal: d.thermal * k,
        kinetic: d.kinetic * k,
        explosive: d.explosive * k,
    }
}

fn damage_of(attr: impl Fn(i64) -> f64, ids: [i64; 4]) -> Damage {
    Damage {
        em: attr(ids[0]),
        thermal: attr(ids[1]),
        kinetic: attr(ids[2]),
        explosive: attr(ids[3]),
    }
}

/// The signature a victim's ball has now, as `ability::cycle` reads it.
fn signature_of(w: &Registry, v: &Presence) -> f64 {
    crate::dogma::hull(w, v.ship_id)
        .map(|h| h.signature_radius)
        .filter(|sig| *sig > 0.0)
        .unwrap_or_else(|| {
            missile::target_facts(w, v.character_id).1 * crate::ewar::signature_factor(w, v.ship_id)
        })
}

/// One cycle of a strike ability, at `a.next`.
pub(super) fn cycle(w: &mut Registry, s: &Squadron, a: &Active, now: BlueTime) -> Cycle {
    match a.effect {
        effect::BOMB => launch_bomb(w, s, a),
        effect::KAMIKAZE => kamikaze(w, s, a, now),
        _ => Cycle::Ran,
    }
}

// ---------------------------------------------------------------------------
// Bombs
// ---------------------------------------------------------------------------

/// **Launch**: the bomb's flight is worked out now (module header, `[A]` 5).
fn launch_bomb(w: &mut Registry, s: &Squadron, a: &Active) -> Cycle {
    let Some(point) = a.point else {
        return Cycle::Ran;
    };
    let td = typedata::installed_in(w.content()).table();
    let bomb = skills::attr_of(w, s.id, s.type_id, BOMB_TYPE) as i64;
    if bomb <= 0 || td.get(bomb).is_none() {
        return Cycle::Ran;
    }
    let speed = td.attribute(bomb, BOMB_SPEED).max(1.0);
    let reach = speed * td.attribute(bomb, BOMB_FUSE).max(0.0) / 1000.0;
    w.advance_system(s.solar_system_id);
    let Some(me) = w.roster().get(s.id).copied() else {
        return Cycle::Ran;
    };
    let from = me.motion.position;
    let way = minus(point, from);
    let distance = length(way);
    let travel = if reach > 0.0 {
        distance.min(reach)
    } else {
        distance
    };
    let at = if distance > 0.0 {
        [
            from[0] + way[0] / distance * travel,
            from[1] + way[1] / distance * travel,
            from[2] + way[2] / distance * travel,
        ]
    } else {
        from
    };
    let flight = (travel / speed * 10_000_000.0) as BlueTime;
    state_mut(w).strikes.bombs.push(Bomb {
        owner: s.owner_id,
        source: s.id,
        system: s.solar_system_id,
        type_id: bomb,
        at,
        due: a.next + flight,
        size: s.size,
    });
    Cycle::Ran
}

/// A bomb goes off: everything in its field, but the owner's, takes it.
fn detonate(w: &mut Registry, b: &Bomb, now: BlueTime) {
    w.advance_system(b.system);
    let td = typedata::installed_in(w.content()).table();
    let a = |attribute: i64| td.attribute(b.type_id, attribute);
    let radius = if a(BOMB_RANGE) > 0.0 {
        a(BOMB_RANGE)
    } else {
        a(BOMB_EMP_RANGE)
    };
    let aoe = Aoe {
        cloud: a(missile::attr::AOE_CLOUD_SIZE),
        velocity: a(missile::attr::AOE_VELOCITY),
        drf: a(missile::attr::AOE_DAMAGE_REDUCTION_FACTOR),
    };
    let volley = times(damage_of(a, BOMB_DAMAGE), b.size.max(0) as f64);
    let ship = w.character(b.owner).map_or(0, |c| c.ship_id);
    let victims: Vec<Presence> = w
        .roster()
        .in_system(b.system)
        .into_iter()
        .filter(|v| {
            w.pilot(v.character_id)
                .is_some_and(|p| p.owner_id != b.owner)
        })
        .filter(|v| crate::space_state::distance(v.motion.position, b.at) <= radius)
        .collect();
    for victim in victims {
        let speed = length(victim.motion.velocity);
        let k = missile::damage_factor(signature_of(w, &victim), speed, aoe);
        let damage = times(volley, k);
        if damage.total() <= 0.0 {
            continue;
        }
        let shot = Shot {
            at: b.due,
            attacker_ship_id: ship,
            weapon_item_id: b.source,
            weapon_type_id: b.type_id,
            target_ship_id: victim.ship_id,
            damage,
            hit_quality: combat::HIT_QUALITY,
        };
        combat::fire(w, shot, b.owner, victim, now);
    }
}

// ---------------------------------------------------------------------------
// Kamikaze
// ---------------------------------------------------------------------------

/// The ability's first cycle sets the squadron flying into its target; the
/// later ones only see that the target is still there.
fn kamikaze(w: &mut Registry, s: &Squadron, a: &Active, _now: BlueTime) -> Cycle {
    let Some(target) = a.target else {
        return Cycle::Ran;
    };
    if super::ability::target_in(w, s.solar_system_id, target).is_none() {
        return Cycle::TargetGone;
    }
    if state(w).strikes.runs.contains_key(&s.id) {
        return Cycle::Ran;
    }
    state_mut(w).strikes.runs.insert(
        s.id,
        Run {
            target,
            contact: None,
        },
    );
    // Into it: a follow with no stand-off; recorded as a goto the tick
    // re-aims, as a movement order is (`ability::movement`).
    w.advance_system(s.solar_system_id);
    let Some(at) = w.ball_position(target) else {
        return Cycle::TargetGone;
    };
    let speed = Action::SetSpeedFraction {
        ball: s.id,
        fraction: super::ability::SPEED_FRACTION,
    };
    if let Some(r) = state_mut(w).rt.get_mut(&s.id) {
        r.order = super::ability::Order::Follow { target, range: 0.0 };
    }
    if let Some(p) = w.roster_mut().get_mut(s.id) {
        p.motion.after(Action::GotoPoint {
            ball: s.id,
            point: at,
        });
        p.motion.after(speed);
    }
    send_actions(
        w,
        Via::Clock,
        s.solar_system_id,
        vec![
            Action::FollowBall {
                ball: s.id,
                target,
                range: 0.0,
            },
            speed,
        ],
    );
    Cycle::Ran
}

/// A kamikaze goes off: the squadron's damage on its target, the squadron
/// exploded.
fn strike(w: &mut Registry, s: &Squadron, victim: Presence, now: BlueTime) {
    let a = |attribute: i64| skills::attr_of(w, s.id, s.type_id, attribute);
    let mut damage = times(damage_of(a, KAMIKAZE_DAMAGE), s.size.max(0) as f64);
    // `CalcCloudFactorModifier`: a cloud of 1 m or less is no reduction.
    let cloud = a(KAMIKAZE_CLOUD);
    if cloud > 1.0 {
        damage = times(
            damage,
            missile::damage_factor(
                signature_of(w, &victim),
                0.0,
                Aoe {
                    cloud,
                    velocity: 0.0,
                    drf: 1.0,
                },
            ),
        );
    }
    // `CalcResistanceModifier`: the attribute the squadron's 2432 names, on
    // the target; 1 when it names none.
    let id = a(KAMIKAZE_RESISTANCE);
    if id > 0.0
        && let Some(r) = super::effects::target_attr(w, victim.ship_id, id as i64)
    {
        damage = times(damage, r.max(0.0));
    }
    if damage.total() > 0.0 {
        let shot = Shot {
            at: now,
            attacker_ship_id: s.id,
            weapon_item_id: s.id,
            weapon_type_id: s.type_id,
            target_ship_id: victim.ship_id,
            damage,
            hit_quality: combat::HIT_QUALITY,
        };
        combat::fire(w, shot, s.owner_id, victim, now);
    }
    let park = audience(w, s.solar_system_id);
    super::ability::explode(w, s, park);
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// Once a world tick: the bombs that are due go off; each kamikaze run
/// closes, waits its second at contact and detonates.
pub(super) fn run(w: &mut Registry, now: BlueTime) {
    if state(w).strikes.bombs.is_empty() && state(w).strikes.runs.is_empty() {
        return;
    }
    let (due, later): (Vec<Bomb>, Vec<Bomb>) = std::mem::take(&mut state_mut(w).strikes.bombs)
        .into_iter()
        .partition(|b| b.due <= now);
    state_mut(w).strikes.bombs = later;
    for b in &due {
        detonate(w, b, now);
    }
    let ids: Vec<i64> = state(w).strikes.runs.keys().copied().collect();
    for id in ids {
        let (Some(s), Some(r)) = (
            state(w).squadron(id).copied(),
            state(w).strikes.runs.get(&id).copied(),
        ) else {
            forget(w, id);
            continue;
        };
        w.advance_system(s.solar_system_id);
        let Some(victim) = super::ability::target_in(w, s.solar_system_id, r.target) else {
            forget(w, id);
            continue;
        };
        let Some(me) = w.roster().get(id).copied() else {
            forget(w, id);
            continue;
        };
        let my_radius = base_hull(w, id).map_or(0.0, |h| f64::from(h.radius));
        let their_radius =
            crate::dogma::hull(w, victim.ship_id).map_or(0.0, |h| f64::from(h.radius));
        let surface = crate::space_state::distance(me.motion.position, victim.motion.position)
            - my_radius
            - their_radius;
        let contact = match r.contact {
            Some(t) => t,
            None if surface <= 0.0 => {
                if let Some(run) = state_mut(w).strikes.runs.get_mut(&id) {
                    run.contact = Some(now);
                }
                now
            }
            None => continue,
        };
        if now.saturating_sub(contact) >= ms(KAMIKAZE_DELAY_MS) {
            strike(w, &s, victim, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fighters::ability::tests as at;
    use crate::fighters::fixture::*;
    use crate::fighters::tests as ft;
    use crate::fighters::tests::{ALICE, ALICE_SHIP, BOB, SYSTEM};

    fn point(p: [f64; 3]) -> PyValue {
        PyValue::Tuple(p.iter().map(|v| PyValue::Real(*v)).collect())
    }

    fn at_of(reg: &Registry, id: i64) -> [f64; 3] {
        reg.roster().get(id).expect("in space").motion.position
    }

    fn soon() -> BlueTime {
        crate::pyutil::filetime_now() + ms(100)
    }

    fn the_slot(reg: &Registry, sq: i64) -> ability::AbilitySlot {
        state(reg).rt.get(&sq).expect("flying").slots[2].clone()
    }

    // -- the bomb ---------------------------------------------------------------

    #[test]
    fn a_bomb_detonates_where_it_was_sent_and_hurts_what_is_in_its_field() {
        let (mut reg, ids, _) = at::fly(&[(BOMBER, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 3_000.0);
        at::velocity_zero(&mut reg, rat);
        let target = at_of(&reg, rat);
        assert_eq!(
            at::activate_call(&mut reg, &ids, 2, &[point(target)]),
            at::ok_for(&ids)
        );
        assert_eq!(
            the_slot(&reg, sq).charges,
            Some(BOMB_CHARGES - 1),
            "a charge is spent"
        );
        let now = soon();
        tick(&mut reg, now);
        let bombs = state(&reg).strikes.bombs().to_vec();
        assert_eq!(bombs.len(), 1, "one bomb in flight");
        let b = &bombs[0];
        assert_eq!(
            (b.owner, b.source, b.type_id, b.size),
            (ALICE, sq, BOMB, SQUADRON)
        );
        assert!(
            crate::space_state::distance(b.at, target) < 50.0,
            "it goes where it was sent"
        );
        // Not yet: it flies at the bomb's speed.
        tick(&mut reg, now + ms(1_000));
        assert!(reg.combat().ship(rat).is_none(), "still in flight");
        let boom = b.due + ms(1);
        tick(&mut reg, boom);
        assert!(state(&reg).strikes.bombs().is_empty(), "it went off");
        let lost = at::shield_lost(&reg, rat, boom);
        // The bomb's damage times the squadron, through the explosion factor
        // (the bomb's cloud of 10 m, a rat at rest).
        let signature = signature_of(&reg, &reg.roster().get(rat).copied().expect("the rat"));
        let factor = (signature / 10.0).min(1.0);
        let want = BOMB_EM * SQUADRON as f64 * factor;
        assert!(
            (lost - want).abs() < 0.05 * want,
            "{lost} of the rat's shield, {want} wanted (factor {factor})"
        );
    }

    #[test]
    fn a_bomb_never_flies_past_its_reach_and_misses_what_is_far() {
        let (mut reg, ids, _) = at::fly(&[(BOMBER, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 3_000.0);
        at::velocity_zero(&mut reg, rat);
        let from = at_of(&reg, sq);
        // Twenty kilometres the other way: further than the bomb's reach.
        let far = [from[0] - 20_000.0, from[1], from[2]];
        assert_eq!(
            at::activate_call(&mut reg, &ids, 2, &[point(far)]),
            at::ok_for(&ids)
        );
        let now = soon();
        tick(&mut reg, now);
        let b = state(&reg).strikes.bombs()[0].clone();
        let reach = BOMB_MPS * BOMB_FUSE_MS / 1000.0;
        let went = crate::space_state::distance(b.at, from);
        assert!(
            (went - reach).abs() < 50.0,
            "{went} against a reach of {reach}"
        );
        let flight = b.due - now;
        assert!(
            (flight - ms(BOMB_FUSE_MS as i64)).abs() < ms(500),
            "a flight of the fuse's length"
        );
        tick(&mut reg, b.due + ms(1));
        assert!(
            reg.combat().ship(rat).is_none(),
            "the rat is further than the field's radius from where it went off"
        );
    }

    #[test]
    fn a_bomb_spares_what_the_owner_owns_and_hurts_the_rest() {
        let (mut reg, ids, _) = at::fly(&[(BOMBER, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 1_000.0);
        at::velocity_zero(&mut reg, rat);
        // Dropped on its own squadron: the owner's ship and squadron are in
        // the field (the carrier orbits within it) and the rat is too.
        let here = at_of(&reg, sq);
        assert_eq!(
            at::activate_call(&mut reg, &ids, 2, &[point(here)]),
            at::ok_for(&ids)
        );
        // Dropped where it is, the bomb is there at once.
        let now = soon();
        tick(&mut reg, now);
        assert!(state(&reg).strikes.bombs().is_empty(), "no flight, no wait");
        assert!(reg.combat().ship(rat).is_some(), "a stranger takes it");
        assert!(
            reg.combat().ship(ALICE_SHIP).is_none(),
            "not the owner's ship"
        );
        assert!(reg.combat().ship(sq).is_none(), "not the owner's squadron");
    }

    #[test]
    fn a_bomb_has_charges_and_a_rearm_time() {
        let (mut reg, ids, _) = at::fly(&[(BOMBER, SQUADRON)]);
        let sq = ids[0];
        let here = at_of(&reg, sq);
        let mut last = None;
        for n in 0..BOMB_CHARGES {
            if let Some(r) = state_mut(&mut reg).rt.get_mut(&sq) {
                r.last_ability = None;
                r.slots[2].blocked_until = None;
            }
            assert_eq!(
                at::activate_call(&mut reg, &ids, 2, &[point(here)]),
                at::ok_for(&ids),
                "bomb {n}"
            );
            last = Some(the_slot(&reg, sq));
            // Right after, the button is grey for the rearm time.
            if let Some(r) = state_mut(&mut reg).rt.get_mut(&sq) {
                r.last_ability = None;
                r.slots[2].active = None;
            }
            let a = at::activate_call(&mut reg, &ids, 2, &[point(here)]);
            if n + 1 < BOMB_CHARGES {
                assert_eq!(at::refusal_for(&a, sq), "FighterAbilityIsBlocked");
            }
        }
        let slot = last.expect("a slot");
        assert_eq!(slot.charges, Some(0));
        let until = slot.blocked_until.expect("rearming");
        let active = slot.active.expect("the last one is running");
        assert_eq!(
            until,
            active.started + active.duration + ms((BOMB_REARM_S * 1000.0) as i64)
        );
        // No charge, no bomb, whatever the clock says.
        if let Some(r) = state_mut(&mut reg).rt.get_mut(&sq) {
            r.last_ability = None;
            r.slots[2].blocked_until = None;
            r.slots[2].active = None;
        }
        let a = at::activate_call(&mut reg, &ids, 2, &[point(here)]);
        assert_eq!(at::refusal_for(&a, sq), "FighterAbilityIsBlocked");
    }

    #[test]
    fn a_bomb_needs_a_point() {
        let (mut reg, ids, _) = at::fly(&[(BOMBER, SQUADRON)]);
        let a = at::activate_call(&mut reg, &ids, 2, &[]);
        assert_eq!(at::refusal_for(&a, ids[0]), "CustomNotify");
        assert_eq!(
            the_slot(&reg, ids[0]).charges,
            Some(BOMB_CHARGES),
            "none spent"
        );
    }

    // -- the kamikaze -------------------------------------------------------------

    /// The volley a kamikaze deals, before the target's resistance and the
    /// cloud.
    fn raw() -> f64 {
        4.0 * KAMIKAZE_EACH * SQUADRON as f64
    }

    #[test]
    fn a_kamikaze_flies_into_its_target_waits_a_second_and_detonates() {
        let (mut reg, ids, _) = at::fly(&[(KAMIKAZE, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 2_000.0);
        at::velocity_zero(&mut reg, rat);
        assert_eq!(
            at::activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]),
            at::ok_for(&ids)
        );
        let now = soon();
        tick(&mut reg, now);
        assert_eq!(
            state(&reg).strikes.run_of(sq).map(|r| r.target),
            Some(rat),
            "it is on its way"
        );
        assert!(matches!(
            state(&reg).rt.get(&sq).map(|r| r.order),
            Some(ability::Order::Follow { target, range }) if target == rat && range == 0.0
        ));
        tick(&mut reg, now + ms(100));
        assert!(
            state(&reg)
                .strikes
                .run_of(sq)
                .is_some_and(|r| r.contact.is_none())
        );
        // On the target: contact, and a second to wait.
        let on = at_of(&reg, rat);
        if let Some(p) = reg.roster_mut().get_mut(sq) {
            p.motion.position = on;
            p.motion.velocity = [0.0; 3];
        }
        let contact = now + ms(200);
        tick(&mut reg, contact);
        assert_eq!(
            state(&reg).strikes.run_of(sq).and_then(|r| r.contact),
            Some(contact)
        );
        tick(&mut reg, contact + ms(KAMIKAZE_DELAY_MS - 100));
        assert!(state(&reg).squadron(sq).is_some(), "not before the second");
        assert!(reg.combat().ship(rat).is_none());
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        let boom = contact + ms(KAMIKAZE_DELAY_MS);
        tick(&mut reg, boom);
        // The squadron is gone, item and ball.
        assert!(state(&reg).squadron(sq).is_none());
        assert!(reg.roster().get(sq).is_none());
        assert!(reg.store().get(sq).is_none());
        assert!(state(&reg).strikes.run_of(sq).is_none());
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        assert_eq!(
            ft::named(&sent, "OnFighterRemovedFromController").len(),
            1,
            "the pilot is told"
        );
        // The target took the squadron's damage through the cloud.
        let signature = {
            let v = reg.roster().get(rat).copied().expect("the rat");
            signature_of(&reg, &v)
        };
        let factor = (signature / CLOUD_M).min(1.0);
        let lost = at::shield_lost(&reg, rat, boom);
        let want = raw() * factor;
        assert!(
            (lost - want).abs() < 0.05 * want,
            "{lost} against {want} (cloud factor {factor})"
        );
    }

    /// **`CalcResistanceModifier`**: the attribute the squadron's 2432 names,
    /// read on the target. A hull without it is untouched; one that carries it
    /// at 1 takes it all.
    #[test]
    fn a_kamikaze_only_hurts_what_carries_its_resistance_attribute() {
        let damaged = |hull: i64| {
            let (mut reg, ids, _) = at::fly_with_bob_in(&[(KAMIKAZE, SQUADRON)], hull);
            let sq = ids[0];
            at::velocity_zero(&mut reg, BOB);
            at::activate_call(
                &mut reg,
                &ids,
                2,
                &[PyValue::Int(crate::fighters::tests::BOB_SHIP)],
            );
            let now = soon();
            tick(&mut reg, now);
            let on = at_of(&reg, BOB);
            if let Some(p) = reg.roster_mut().get_mut(sq) {
                p.motion.position = on;
                p.motion.velocity = [0.0; 3];
            }
            let _ = ft::pending(&mut reg);
            let _ = ft::due(&mut reg);
            tick(&mut reg, now + ms(200));
            tick(&mut reg, now + ms(200 + KAMIKAZE_DELAY_MS));
            let mut sent = ft::pending(&mut reg);
            sent.extend(ft::due(&mut reg));
            assert!(
                state(&reg).squadron(sq).is_none(),
                "it detonated either way"
            );
            ft::named(&sent, "OnDamageMessage")
                .iter()
                .any(|(to, _, _)| to.as_ref().is_some_and(|t| t.contains(&BOB)))
        };
        assert!(
            damaged(CAPITAL),
            "the hull that carries it takes the damage"
        );
        assert!(
            !damaged(at::JAM_TARGET),
            "a hull that does not carry it is not touched"
        );
    }

    #[test]
    fn a_kamikaze_stops_when_its_target_goes() {
        let (mut reg, ids, _) = at::fly(&[(KAMIKAZE, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 2_000.0);
        at::activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        let now = soon();
        tick(&mut reg, now);
        assert!(state(&reg).strikes.run_of(sq).is_some());
        reg.roster_mut().leave(rat);
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        tick(&mut reg, now + ms(1_000));
        assert!(state(&reg).strikes.run_of(sq).is_none(), "the run is off");
        tick(&mut reg, now + ms(11_000));
        assert!(state(&reg).squadron(sq).is_some(), "the squadron lives");
        let sent = ft::due(&mut reg);
        assert_eq!(
            ft::named(&sent, "OnFighterAbilitySlotDeactivated").len(),
            1,
            "the ability ends"
        );
        assert!(reg.store().get(sq).is_some());
    }

    #[test]
    fn deactivating_a_kamikaze_calls_it_off_at_once() {
        let (mut reg, ids, _) = at::fly(&[(KAMIKAZE, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 2_000.0);
        at::activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        let now = soon();
        tick(&mut reg, now);
        assert!(state(&reg).strikes.run_of(sq).is_some());
        let _ = ft::pending(&mut reg);
        assert_eq!(at::deactivate_call(&mut reg, &ids, 2), at::ok_for(&ids));
        assert!(
            state(&reg).strikes.run_of(sq).is_none(),
            "off, not at the cycle's end"
        );
        assert!(matches!(
            state(&reg).rt.get(&sq).map(|r| r.order),
            Some(ability::Order::Stop)
        ));
        // Even on top of the target it does not go off.
        let on = at_of(&reg, rat);
        if let Some(p) = reg.roster_mut().get_mut(sq) {
            p.motion.position = on;
        }
        tick(&mut reg, now + ms(2_000));
        assert!(state(&reg).squadron(sq).is_some(), "it lives");
    }

    #[test]
    fn letting_go_of_a_kamikaze_ends_its_run() {
        let (mut reg, ids, _) = at::fly(&[(KAMIKAZE, SQUADRON)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 2_000.0);
        at::activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        tick(&mut reg, soon());
        assert!(state(&reg).strikes.run_of(sq).is_some());
        ft::call(&mut reg, "CmdAbandonFighter", &[PyValue::Int(sq)]).expect("abandoned");
        assert!(state(&reg).strikes.run_of(sq).is_none());
        let _ = (ALICE, SYSTEM);
    }
}
