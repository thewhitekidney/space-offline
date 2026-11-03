//! A squadron in space: its movement orders, its abilities and their damage
//! (FT-E, `docs/lanes/ft-e.md`; spec R-DEEP §2.3-§2.4, §2.7).
//!
//! **Movement** (`fighterMgr.ExecuteMovementCommandOnFighters(fighterIDs,
//! command, *args)`, `fightersSvc._ExecuteMovementCommandOnFighters` [V]): the
//! four commands are `'ORBIT'(targetID, range)`, `'FOLLOW'(targetID, range)`,
//! `'STOP'` and `'GOTO_POINT'(point)`, a plain `(x, y, z)` of global
//! coordinates (`positionalControl._ConvertPositionToGlobalSpace` [V]). "Return
//! and orbit" is `ORBIT` on the carrier at 1,500 m
//! (`fightersSvc.CmdReturnAndOrbit`, `DEFAULT_CONTROLLER_ORBIT_DISTANCE` [V]);
//! "approach" is `FOLLOW` at `const.approachRange` (`movementFunctions.Approach`
//! [V]).
//!
//! **Abilities** (`CmdActivateAbilitySlots(fighterIDs, abilitySlotID, *args)`
//! and `CmdDeactivateAbilitySlots`, both answering `{fighterID: None |
//! UserError}` [V `fightersSvc.py:178-196`]). The args are `()` for an
//! untargeted ability, `(targetID,)` for an item-targeted one and `(point,)`
//! for a point-targeted one (the three `Activate...` wrappers [V]). **None of
//! the thirteen ability effects has a modifier** [V-data], so the numbers are
//! read from the type's attributes (R-DEEP §2.3) and the effect row's
//! duration, range and falloff attribute ids; this file is the arithmetic.
//!
//! **What an activation is.** `effectRepeatCount` is the client's own rule
//! (`fighters.GetEffectRepeatCountForAbility` [V]): an ability with charges or
//! a cooldown runs **one** cycle per activation, any other repeats until
//! deactivated. The pilot's UI draws the cycle from the `startTime` and
//! `durationMs` of `OnFighterAbilitySlotActivated` and keeps looping the ramp
//! until `OnFighterAbilitySlotDeactivated` (`abilityIcon._StartCycleAnimationThread`
//! [V]); a cooldown `(start, end)` in the same event is what greys the button
//! afterwards (`abilityIcon.OnAbilityStatusUpdated` [V]).
//!
//! **Damage** is one volley per cycle, at the cycle's start: the type's four
//! damage attributes times its multiplier times the **current squadron size**,
//! through the missile explosion factor, as the owner's shot through
//! `combat::fire`, so aggression, the combat log, bounties and killmails are
//! the ones a module's volley has. A squadron's **health** is one fighter's
//! shield, armour and hull, a record like any ship's; when it is spent one
//! fighter is lost, the survivors' size is pushed, and the next fighter is
//! whole. The pilot's health gauge reads exactly this: the ball's shield
//! fraction colours the last fighter of the row
//! (`squadronCont.UpdateDamage`, `fightersHealthGaugeCont.GetDashColors` [V]).

use super::effects;
use super::skills;
use super::strike;
use super::*;
use crate::combat;
use crate::crimewatch;
use crate::damage::{Damage, Shot};
use crate::fighters_data::{Fighters, TargetMode, effect};
use crate::missile;

// ---------------------------------------------------------------------------
// The assumptions, by name
// ---------------------------------------------------------------------------

/// One movement command per fighter per this many ms. **[A]** the client has
/// the key (`FighterMovementCommandThrottled`) and no number. Falsifier: a
/// squadron drag-ordered by the UI is refused as throttled.
pub const MOVE_THROTTLE_MS: i64 = 1_000;
/// One ability command per fighter per this many ms **[A]**, as above
/// (`FighterAbilityCommandThrottled`).
pub const ABILITY_THROTTLE_MS: i64 = 1_000;
/// A fighter further than this share of the carrier's `maxTargetRange` (76)
/// refuses commands with `CannotControlFighterTooDistant` **[A]**: the key
/// has no raise site in the client and R-DEEP §2.6 names the lock range as
/// the likeliest bound. A hull with no lock range has no limit. Falsifier:
/// the client lets a pilot command a squadron further out with the stock
/// server.
pub const CONTROL_RANGE_FRACTION: f64 = 1.0;
/// Volleys one tick may owe a squadron (a stalled clock must not fire a
/// minute of shots at once), as `drones::MAX_SHOTS_PER_TICK`.
pub const MAX_CYCLES_PER_TICK: u32 = 8;
/// Squadrons fly every order at full throttle **[A]**, as a launch does
/// ([`LAUNCH_SPEED_FRACTION`]); the client sends no throttle with a movement
/// command.
pub const SPEED_FRACTION: f64 = 1.0;
/// The damage reduction exponent's constant: the missile formula's
/// `log(drf) / log(drs)` **[A]**, because the fighter attributes are named
/// like a missile's (`ReductionFactor`, `ReductionSensitivity` [V
/// `abilityAttributes.infoAttributesByDogmaEffect`]) and R-DEEP §2.3 says to
/// apply them as a missile's. Falsifier: the combat log's number against a
/// fast target.
pub const NO_SENSITIVITY: f64 = 1.0;

// ---------------------------------------------------------------------------
// The attributes of each ability effect [V-data, R-DEEP §2.3]
// ---------------------------------------------------------------------------

/// Where an effect's duration, range and falloff live on the fighter's type
/// (the effect row names them too; this is the fallback when it does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectAttrs {
    pub duration: i64,
    pub range: i64,
    pub falloff: i64,
}

/// The attribute ids of every ability effect, by the effect's id; `0` for
/// "none". `None` for an effect no fighter ability uses.
pub fn effect_attrs(effect_id: i64) -> Option<EffectAttrs> {
    let a = |duration, range, falloff| {
        Some(EffectAttrs {
            duration,
            range,
            falloff,
        })
    };
    match effect_id {
        effect::ATTACK => a(2233, 2236, 2237),
        effect::MISSILES => a(2182, 2149, 0),
        effect::WEB => a(2183, 2186, 2187),
        effect::WARP_DISRUPT => a(2203, 2204, 0),
        effect::NEUT => a(2208, 2209, 2210),
        effect::ECM => a(2220, 2221, 2222),
        effect::MICROWARPDRIVE => a(2157, 0, 0),
        effect::AFTERBURNER => a(2158, 0, 0),
        effect::MICRO_JUMP_DRIVE => a(2155, 0, 0),
        effect::EVASION => a(2123, 0, 0),
        effect::BOMB => a(2349, 0, 0),
        effect::TACKLE => a(2238, 2239, 0),
        effect::KAMIKAZE => a(2401, 2330, 0),
        _ => None,
    }
}

/// The attributes of a damage-dealing ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weapon {
    /// EM, thermal, kinetic, explosive.
    pub damage: [i64; 4],
    pub multiplier: i64,
    pub explosion_radius: i64,
    pub explosion_velocity: i64,
    pub reduction_factor: i64,
    pub reduction_sensitivity: i64,
}

/// `fighterAbilityAttackMissile*` (the cannons and the "attack missile").
pub const ATTACK_WEAPON: Weapon = Weapon {
    damage: [2227, 2228, 2229, 2230],
    multiplier: 2226,
    explosion_radius: 2234,
    explosion_velocity: 2235,
    reduction_factor: 2231,
    reduction_sensitivity: 2232,
};

/// `fighterAbilityMissiles*` (micro swarms, salvos, torpedoes).
pub const MISSILE_WEAPON: Weapon = Weapon {
    damage: [2131, 2132, 2133, 2134],
    multiplier: 2130,
    explosion_radius: 2125,
    explosion_velocity: 2126,
    reduction_factor: 2127,
    reduction_sensitivity: 2128,
};

/// The weapon an effect deals damage with.
pub fn weapon_of(effect_id: i64) -> Option<&'static Weapon> {
    match effect_id {
        effect::ATTACK => Some(&ATTACK_WEAPON),
        effect::MISSILES => Some(&MISSILE_WEAPON),
        _ => None,
    }
}

/// Whether this build runs the effect (the others are refused at
/// activation, in words).
pub fn supported(effect_id: i64) -> bool {
    weapon_of(effect_id).is_some() || effects::runs(effect_id) || strike::runs(effect_id)
}

// ---------------------------------------------------------------------------
// The live record
// ---------------------------------------------------------------------------

/// A squadron's standing movement order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Order {
    Orbit { target: i64, range: f64 },
    Follow { target: i64, range: f64 },
    Goto([f64; 3]),
    Stop,
}

/// An ability that is running.
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub effect: i64,
    pub target: Option<i64>,
    pub point: Option<[f64; 3]>,
    pub started: BlueTime,
    /// When the next cycle begins (and its volley lands).
    pub next: BlueTime,
    pub duration: BlueTime,
    pub cycles: u32,
    /// `CmdDeactivateAbilitySlots` arrived: end at the cycle's end.
    pub stopping: bool,
    /// One cycle per activation (`GetEffectRepeatCountForAbility` [V]).
    pub one_shot: bool,
    pub guid: String,
    pub offensive: bool,
    /// Range plus falloff, m; `0` states none (no limit).
    pub reach: f64,
}

/// One ability slot's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AbilitySlot {
    pub active: Option<Active>,
    /// Rounds left; `None` for an ability with no charges.
    pub charges: Option<i64>,
    /// Blocked until: the cooldown or the rearm time after the last cycle.
    pub blocked_until: Option<BlueTime>,
}

/// What a flying squadron is doing.
#[derive(Debug, Clone, PartialEq)]
pub struct Runtime {
    /// The type's attributes as this pilot's skills and this carrier's bonuses
    /// make them, fixed at launch (FT-H, [`skills::numbers`]).
    pub numbers: skills::Numbers,
    pub order: Order,
    pub slots: [AbilitySlot; 3],
    pub last_move: Option<BlueTime>,
    pub last_ability: Option<BlueTime>,
}

impl Runtime {
    /// A fresh launch: orbiting its carrier, every ability ready and every
    /// charge full (the charges refill in the tube, with the squadron's
    /// refuelling **[A]**; the client has no event that refills one in space).
    pub(super) fn launched(w: &Registry, type_id: i64, ship: i64) -> Runtime {
        let range = fighter_type(w.content(), type_id)
            .map(|f| f.orbit_range)
            .filter(|r| *r > 0.0)
            .unwrap_or(DEFAULT_ORBIT);
        let mut slots: [AbilitySlot; 3] = Default::default();
        for (slot, charges) in charges_of(w, type_id) {
            if let Some(s) = slots.get_mut(slot as usize) {
                s.charges = Some(charges);
            }
        }
        Runtime {
            numbers: skills::numbers(w, ship, type_id),
            order: Order::Orbit {
                target: ship,
                range,
            },
            slots,
            last_move: None,
            last_ability: None,
        }
    }
}

fn rt(w: &Registry, id: i64) -> Option<&Runtime> {
    state(w).rt.get(&id)
}

fn rt_mut(w: &mut Registry, id: i64) -> Option<&mut Runtime> {
    state_mut(w).rt.get_mut(&id)
}

/// The ability running in `slot` of squadron `id`, if any.
pub(super) fn active_in(w: &Registry, id: i64, slot: usize) -> Option<Active> {
    rt(w, id)
        .and_then(|r| r.slots.get(slot))
        .and_then(|sl| sl.active.clone())
}

/// A squadron that is no longer flying for anybody (scooped): its runtime,
/// its holds and any strike run go.
pub(super) fn drop_runtime(w: &mut Registry, id: i64) {
    state_mut(w).rt.remove(&id);
    strike::forget(w, id);
    effects::source_gone(w, id);
}

// ---------------------------------------------------------------------------
// What an ability is: the tables, joined
// ---------------------------------------------------------------------------

/// One ability of one fighter type: `abilities.tsv`, `slots.tsv`, the effect
/// row and the type's own numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    pub slot: i64,
    pub ability: i64,
    pub effect: i64,
    pub mode: TargetMode,
    pub low_sec_banned: bool,
    pub high_sec_banned: bool,
    pub cooldown_ms: Option<i64>,
    pub charges: Option<i64>,
    pub rearm_ms: Option<i64>,
    pub duration_ms: i64,
    pub range: f64,
    pub falloff: f64,
    pub guid: String,
    pub offensive: bool,
}

impl Info {
    /// One cycle per activation: charges or a cooldown (`effectRepeatCount`
    /// 1 [V `fighters.GetEffectRepeatCountForAbility`]).
    pub fn one_shot(&self) -> bool {
        self.charges.is_some() || self.cooldown_ms.is_some()
    }

    /// How long the button stays grey after the cycle: the cooldown, or the
    /// rearm time between charges **[A]** (the client reads no rearm time;
    /// R-DEEP §2.3 says only "rearm"). Falsifier: a bomb's button is ready
    /// sooner or later than the type's `rearmTimeSeconds` after use.
    pub fn after_cycle_ms(&self) -> Option<i64> {
        self.cooldown_ms.or(self.rearm_ms)
    }
}

fn secs_to_ms(s: f64) -> i64 {
    (s * 1000.0).round() as i64
}

/// The ability in `slot` of `type_id`, or the reason it cannot be used, on
/// the type's own numbers.
#[cfg(test)]
pub fn info(content: &Content, type_id: i64, slot: i64) -> Result<Info, Refusal> {
    let td = typedata::installed_in(content).table();
    info_with(content, type_id, slot, |a| td.attribute(type_id, a))
}

/// [`info`] on a flying squadron's numbers: its duration, range and falloff
/// with the pilot's skills and the carrier's bonuses in them (FT-H).
pub fn info_of(w: &Registry, s: &Squadron, slot: i64) -> Result<Info, Refusal> {
    info_with(w.content(), s.type_id, slot, |a| {
        skills::attr_of(w, s.id, s.type_id, a)
    })
}

fn info_with(
    content: &Content,
    type_id: i64,
    slot: i64,
    value: impl Fn(i64) -> f64,
) -> Result<Info, Refusal> {
    let table = content
        .try_get::<Fighters>()
        .ok_or_else(|| Refusal::text("This server has no fighter ability data."))?;
    let row = table
        .slot(type_id, slot)
        .ok_or_else(|| Refusal::text("That fighter has no ability in that slot."))?;
    let ability = table
        .ability(row.ability_id)
        .ok_or_else(|| Refusal::text("This server has no data for that ability."))?;
    let td = typedata::installed_in(content).table();
    let fx = td
        .effect(ability.effect_id)
        .ok_or_else(|| Refusal::text("This server has no data for that ability."))?;
    let known = effect_attrs(ability.effect_id);
    let pick = |from_row: i64, from_table: Option<i64>| {
        if from_row != 0 {
            from_row
        } else {
            from_table.unwrap_or(0)
        }
    };
    let duration_attr = pick(fx.duration_attribute, known.map(|k| k.duration));
    let range_attr = pick(fx.range_attribute, known.map(|k| k.range));
    let falloff_attr = pick(fx.falloff_attribute, known.map(|k| k.falloff));
    let attr = |a: i64| if a == 0 { 0.0 } else { value(a) };
    Ok(Info {
        slot,
        ability: row.ability_id,
        effect: ability.effect_id,
        mode: ability.target_mode,
        low_sec_banned: ability.low_sec_banned,
        high_sec_banned: ability.high_sec_banned,
        cooldown_ms: row.cooldown_s.map(secs_to_ms),
        charges: row.charges,
        rearm_ms: row.rearm_s.map(secs_to_ms),
        duration_ms: attr(duration_attr).round() as i64,
        range: attr(range_attr),
        falloff: attr(falloff_attr),
        guid: fx.guid.clone(),
        offensive: fx.is_offensive,
    })
}

// ---------------------------------------------------------------------------
// The arithmetic
// ---------------------------------------------------------------------------

/// The missile formula's exponent from a fighter's two attributes:
/// `log(factor) / log(sensitivity)` when both are usable, else the factor as
/// given (a type with no sensitivity states the exponent itself) **[A]**.
pub fn reduction_exponent(factor: f64, sensitivity: f64) -> f64 {
    if factor > 0.0 && sensitivity > NO_SENSITIVITY {
        factor.ln() / sensitivity.ln()
    } else {
        factor
    }
}

/// One volley, before the target is considered: the weapon's four damages
/// times its multiplier times the squadron's size. `attr` is the type's
/// attribute lookup (0 for what it lacks).
pub fn volley(attr: impl Fn(i64) -> f64, weapon: &Weapon, size: i64) -> Damage {
    let k = attr(weapon.multiplier) * size.max(0) as f64;
    let d = |i: usize| attr(weapon.damage[i]) * k;
    Damage {
        em: d(0),
        thermal: d(1),
        kinetic: d(2),
        explosive: d(3),
    }
}

/// How much of a volley a target takes: the missile explosion factor over the
/// weapon's cloud and velocity and the target's signature and speed.
pub fn application(attr: impl Fn(i64) -> f64, weapon: &Weapon, signature: f64, speed: f64) -> f64 {
    missile::damage_factor(
        signature,
        speed,
        missile::Aoe {
            cloud: attr(weapon.explosion_radius),
            velocity: attr(weapon.explosion_velocity),
            drf: reduction_exponent(
                attr(weapon.reduction_factor),
                attr(weapon.reduction_sensitivity),
            ),
        },
    )
}

fn scale(d: Damage, k: f64) -> Damage {
    Damage {
        em: d.em * k,
        explosive: d.explosive * k,
        kinetic: d.kinetic * k,
        thermal: d.thermal * k,
    }
}

fn speed_of(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// A squadron's health as a ship's: **one fighter's** shield, armour and hull
/// (with its pilot's and its carrier's bonuses, [`base_hull`]) with the type's
/// resonances, times the evasive manoeuvres' while they run. For
/// `dogma::profile`'s fighter arm (coordinator).
pub fn profile_of(w: &Registry, id: i64) -> Option<crate::damage::Profile> {
    let h = base_hull(w, id)?;
    let mut p = crate::damage::Profile::from_hull_in(w.content(), &h);
    if let Some(k) = effects::evasion_resonance(w, id) {
        let times = |d: Damage| Damage {
            em: d.em * k.em,
            thermal: d.thermal * k.thermal,
            kinetic: d.kinetic * k.kinetic,
            explosive: d.explosive * k.explosive,
        };
        p.shield_resonance = times(p.shield_resonance);
        p.armor_resonance = times(p.armor_resonance);
        p.hull_resonance = times(p.hull_resonance);
    }
    Some(p)
}

// ---------------------------------------------------------------------------
// FX
// ---------------------------------------------------------------------------

/// `OnSpecialFX` for an ability: [`crate::modules::SpecialFx`] with the guid
/// the effect row carries (the fighter effects' guids are not `'static`).
/// **[A]** `moduleID` is the squadron's own item id, as a drone's gun is its
/// drone's (`drones::gun_fx` [V `Drone.py:109`]); that the client draws a
/// fighter's ability from this push is not read. Falsifier: no beam or
/// missile trail when an ability is active.
#[derive(Debug, Clone, PartialEq)]
pub struct AbilityFx {
    pub to: Vec<i64>,
    pub ship_id: i64,
    pub type_id: i64,
    pub target_id: Option<i64>,
    pub guid: String,
    pub offensive: bool,
    pub start: bool,
    pub duration_ms: i64,
    pub repeat: i64,
}

impl Push for AbilityFx {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let flag = PyValue::Int(i64::from(self.start));
        (
            "OnSpecialFX",
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Int(self.ship_id),
                PyValue::Int(self.type_id),
                self.target_id.map_or(PyValue::None, PyValue::Int),
                PyValue::None,
                PyValue::Bytes(self.guid.as_bytes().to_vec()),
                PyValue::Bool(self.offensive),
                flag.clone(),
                flag,
                PyValue::Int(self.duration_ms),
                PyValue::Int(self.repeat),
                PyValue::None,
                PyValue::Int(0),
                PyValue::None,
            ],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship_id])
    }
}

fn fx(w: &mut Registry, via: Via, s: &Squadron, a: &Active, start: bool) {
    if a.guid.is_empty() {
        return;
    }
    let to = audience(w, s.solar_system_id);
    if to.is_empty() {
        return;
    }
    send(
        w,
        via,
        AbilityFx {
            to,
            ship_id: s.id,
            type_id: s.type_id,
            target_id: a.target,
            guid: a.guid.clone(),
            offensive: a.offensive,
            start,
            duration_ms: a.duration / BLUE_PER_MS,
            // `GetEffectRepeatCountForAbility` [V]: 1, or 1000.
            repeat: if a.one_shot { 1 } else { 1000 },
        },
    );
}

// ---------------------------------------------------------------------------
// Movement
// ---------------------------------------------------------------------------

/// Why a movement command is not carried out.
#[derive(Debug, Clone, PartialEq)]
pub enum MoveError {
    /// A `UserError` for the pilot.
    Refused(Refusal),
    /// The arguments are not the client's shape.
    Args(&'static str),
}

fn number(v: &PyValue) -> Option<f64> {
    match v {
        PyValue::Int(n) => Some(*n as f64),
        PyValue::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

/// An `(x, y, z)` of finite numbers.
fn point_arg(v: Option<&PyValue>) -> Option<[f64; 3]> {
    match v? {
        PyValue::Tuple(p) | PyValue::List(p) if p.len() == 3 => {
            Some([number(&p[0])?, number(&p[1])?, number(&p[2])?])
        }
        _ => None,
    }
}

/// The largest orbit or follow range accepted, metres: a number far beyond any
/// system is a client bug, not an order.
pub const MAX_RANGE: f64 = 1.0e12;

/// What the carrier's lock range allows a command to reach, if anything does.
fn control_range(w: &Registry, ship: i64) -> Option<f64> {
    crate::dogma::hull(w, ship)
        .map(|h| h.max_target_range * CONTROL_RANGE_FRACTION)
        .filter(|r| *r > 0.0)
}

/// `(owner, ship, [(fighterID, the squadron or its refusal)])`.
type Controlled = (i64, i64, Vec<(i64, Result<Squadron, Refusal>)>);

/// The squadrons of `ids` that this pilot's ship controls, in space and
/// `INSPACE`, with the refusal for each that is not.
fn controlled(w: &Registry, actor: Option<i64>, ids: &[i64]) -> Result<Controlled, Refusal> {
    let (owner, ship, _) = pilot_ship(w, actor)?;
    let here = w
        .roster()
        .get(owner)
        .copied()
        .filter(|p| p.ship_id == ship)
        .map(|p| p.motion.position);
    let limit = control_range(w, ship);
    let out = ids
        .iter()
        .map(|&id| {
            let r = (|| {
                let s = state(w)
                    .squadron(id)
                    .copied()
                    .filter(|s| s.ship_id == ship)
                    .ok_or_else(|| Refusal::key("CannotControlFighterNotControlledByYourShip"))?;
                if state(w).task(ship, s.tube).map(|t| t.status) != Some(Status::InSpace) {
                    return Err(Refusal::key("FighterTubeIsBusy"));
                }
                if let (Some(limit), Some(me), Some(at)) =
                    (limit, here, position_in(w, id, s.solar_system_id))
                    && crate::space_state::distance(me, at) > limit
                {
                    return Err(Refusal::key("CannotControlFighterTooDistant"));
                }
                Ok(s)
            })();
            (id, r)
        })
        .collect();
    Ok((owner, ship, out))
}

/// `fighterMgr.ExecuteMovementCommandOnFighters(fighterIDs, command, *args)`
/// [V `fightersSvc.py:82`]; the client discards the answer, so a refusal is
/// raised (the first one, after the squadrons that can obey have).
///
/// A squadron that is not `INSPACE` (launching, recalling, landing) is skipped
/// without a word: the client's own bulk commands (`RecallAllFightersAndOrbit`
/// names every fighter in space) include them.
pub fn movement(
    w: &mut Registry,
    actor: Option<i64>,
    ids: &[i64],
    command: &str,
    args: &[PyValue],
) -> Result<(), MoveError> {
    let wire_target = |args: &[PyValue]| -> Result<(i64, f64), MoveError> {
        let (Some(PyValue::Int(target)), Some(range)) = (args.first(), args.get(1)) else {
            return Err(MoveError::Args("(targetID, range)"));
        };
        let range = number(range).ok_or(MoveError::Args("(targetID, range)"))?;
        Ok((*target, range.clamp(0.0, MAX_RANGE)))
    };
    enum Cmd {
        Orbit(i64, f64),
        Follow(i64, f64),
        Stop,
        Goto([f64; 3]),
    }
    let cmd = match command {
        "ORBIT" => {
            let (t, r) = wire_target(args)?;
            Cmd::Orbit(t, r)
        }
        "FOLLOW" => {
            let (t, r) = wire_target(args)?;
            Cmd::Follow(t, r)
        }
        "STOP" => Cmd::Stop,
        "GOTO_POINT" => Cmd::Goto(point_arg(args.first()).ok_or(MoveError::Args("(point,)"))?),
        _ => return Err(MoveError::Args("a known movement command")),
    };
    let (owner, _ship, list) = controlled(w, actor, ids).map_err(MoveError::Refused)?;
    w.advance_character(owner);
    let now = crate::pyutil::filetime_now();
    let mut first_refusal: Option<Refusal> = None;
    let mut actions: Vec<Action> = Vec::new();
    let mut system = 0;
    for (id, r) in list {
        let s = match r {
            Ok(s) => s,
            Err(r) if r.key == "FighterTubeIsBusy" => continue,
            Err(r) => {
                first_refusal.get_or_insert(r);
                continue;
            }
        };
        if rt(w, id).is_some_and(|r| {
            r.last_move
                .is_some_and(|t| now.saturating_sub(t) < ms(MOVE_THROTTLE_MS))
        }) {
            first_refusal.get_or_insert(Refusal::key("FighterMovementCommandThrottled"));
            continue;
        }
        let (wire, record, order): (Vec<Action>, Vec<Action>, Order) = match cmd {
            Cmd::Orbit(target, range) => {
                if w.ball_position(target).is_none() {
                    first_refusal.get_or_insert(Refusal::text("That target is not in space."));
                    continue;
                }
                let a = vec![
                    Action::Orbit {
                        ball: id,
                        target,
                        range,
                    },
                    Action::SetSpeedFraction {
                        ball: id,
                        fraction: SPEED_FRACTION,
                    },
                ];
                (a.clone(), a, Order::Orbit { target, range })
            }
            Cmd::Follow(target, range) => {
                let Some(at) = w.ball_position(target) else {
                    first_refusal.get_or_insert(Refusal::text("That target is not in space."));
                    continue;
                };
                let speed = Action::SetSpeedFraction {
                    ball: id,
                    fraction: SPEED_FRACTION,
                };
                // Recorded as a goto at the target, re-aimed every tick: the
                // drone rule (`drones::reaim`), because a recorded `Follow`
                // is placed at its range from the target on the next read.
                (
                    vec![
                        Action::FollowBall {
                            ball: id,
                            target,
                            range,
                        },
                        speed,
                    ],
                    vec![
                        Action::GotoPoint {
                            ball: id,
                            point: at,
                        },
                        speed,
                    ],
                    Order::Follow { target, range },
                )
            }
            Cmd::Stop => {
                let a = vec![Action::Stop { ball: id }];
                (a.clone(), a, Order::Stop)
            }
            Cmd::Goto(point) => {
                let a = vec![
                    Action::GotoPoint { ball: id, point },
                    Action::SetSpeedFraction {
                        ball: id,
                        fraction: SPEED_FRACTION,
                    },
                ];
                (a.clone(), a, Order::Goto(point))
            }
        };
        if let Some(p) = w.roster_mut().get_mut(id) {
            for a in &record {
                p.motion.after(*a);
            }
        }
        if let Some(r) = rt_mut(w, id) {
            r.order = order;
            r.last_move = Some(now);
        }
        system = s.solar_system_id;
        actions.extend(wire);
    }
    if !actions.is_empty() {
        send_actions(w, Via::Answer, system, actions);
    }
    match first_refusal {
        Some(r) => Err(MoveError::Refused(r)),
        None => Ok(()),
    }
}

/// The follow order's re-aim, once a tick.
fn reaim(w: &mut Registry, id: i64) {
    let Some(Order::Follow { target, .. }) = rt(w, id).map(|r| r.order) else {
        return;
    };
    let Some(at) = w.ball_position(target) else {
        return;
    };
    if let Some(p) = w.roster_mut().get_mut(id) {
        p.motion.mode = destiny::Mode::Goto(at);
    }
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// A `{fighterID: None | UserError}` answer.
fn answer(entries: Vec<(i64, Option<Refusal>)>) -> PyValue {
    reply(entries)
}

fn blocked() -> Refusal {
    Refusal::key("FighterAbilityIsBlocked")
}

/// A target ship's ball, in the squadron's system.
pub(super) fn target_in(
    w: &Registry,
    system: i64,
    target: i64,
) -> Option<crate::space_state::Presence> {
    w.roster()
        .by_ship(target)
        .copied()
        .filter(|p| p.solar_system_id == system)
}

/// `fighterMgr.CmdActivateAbilitySlots(fighterIDs, abilitySlotID, *args)`
/// [V `fightersSvc.py:178`]: `{fighterID: None | UserError}`.
///
/// Per squadron, in order: it is this ship's and `INSPACE`; in control range;
/// not throttled; the slot has an ability; (already running: the activation
/// is told again and answered `None`, so the client's pending ramp ends); the
/// security-space bans (`disallowInLowSec` and `disallowInHighSec` of the
/// ability, answered `FighterAbilityIsBlocked` **[A]**: the key's text is not
/// readable here); not on cooldown or out of charges; the target (a ball in
/// the system, not the carrier or a squadron of it; an attack the pilot's
/// safety forbids is refused as a module's is); the effect is one this build
/// runs.
pub fn activate(
    w: &mut Registry,
    actor: Option<i64>,
    ids: &[i64],
    slot: i64,
    args: &[PyValue],
) -> PyValue {
    let (owner, ship, list) = match controlled(w, actor, ids) {
        Ok(v) => v,
        Err(r) => return answer(ids.iter().map(|i| (*i, Some(r.clone()))).collect()),
    };
    w.advance_character(owner);
    let now = crate::pyutil::filetime_now();
    let mut out = Vec::new();
    for (id, r) in list {
        let result = r.and_then(|s| activate_one(w, owner, ship, s, slot, args, now));
        out.push((id, result.err()));
    }
    answer(out)
}

fn activate_one(
    w: &mut Registry,
    owner: i64,
    ship: i64,
    s: Squadron,
    slot: i64,
    args: &[PyValue],
    now: BlueTime,
) -> Result<(), Refusal> {
    let id = s.id;
    if !(0..3).contains(&slot) {
        return Err(Refusal::text("That is not an ability slot."));
    }
    if rt(w, id).is_some_and(|r| {
        r.last_ability
            .is_some_and(|t| now.saturating_sub(t) < ms(ABILITY_THROTTLE_MS))
    }) {
        return Err(Refusal::key("FighterAbilityCommandThrottled"));
    }
    let info = info_of(w, &s, slot)?;
    if !supported(info.effect) {
        return Err(Refusal::text(
            "This server does not run that fighter ability yet.",
        ));
    }
    let ix = slot as usize;
    if let Some(a) = rt(w, id).and_then(|r| r.slots[ix].active.clone()) {
        // Already running: tell the pilot again.
        let snapshot = rt(w, id).cloned();
        activated_event(w, Via::Answer, &s, &info, &a, snapshot.as_ref(), now);
        return Ok(());
    }
    match crimewatch::space_of(w, s.solar_system_id) {
        crimewatch::Space::Low if info.low_sec_banned => return Err(blocked()),
        crimewatch::Space::High if info.high_sec_banned => return Err(blocked()),
        _ => {}
    }
    let slot_state = rt(w, id).map(|r| r.slots[ix].clone()).unwrap_or_default();
    if slot_state.blocked_until.is_some_and(|t| t > now) || slot_state.charges == Some(0) {
        return Err(blocked());
    }
    let (target, point) = match info.mode {
        TargetMode::Untargeted => (None, None),
        TargetMode::PointTargeted => (
            None,
            Some(
                point_arg(args.first())
                    .ok_or_else(|| Refusal::text("That ability needs a point."))?,
            ),
        ),
        TargetMode::ItemTargeted => {
            let Some(PyValue::Int(target)) = args.first() else {
                return Err(Refusal::text("That ability needs a target."));
            };
            if *target == ship || state(w).squadron(*target).is_some() {
                return Err(Refusal::text("A squadron cannot target its own ship."));
            }
            let victim = target_in(w, s.solar_system_id, *target)
                .ok_or_else(|| Refusal::text("That target is not in this system."))?;
            if effects::is_target_ewar(info.effect)
                && let Some(me) = w.roster().get(id).map(|p| p.motion.position)
                && effects::weight(
                    &info,
                    crate::space_state::distance(me, victim.motion.position),
                ) <= 0.0
            {
                return Err(Refusal::key("FighterAbilityEWarFailsDueToRange"));
            }
            if info.offensive
                && let Some(r) =
                    crimewatch::attack_refusal(w, owner, Some(s.type_id), victim.ship_id, now)
            {
                return Err(Refusal {
                    key: r.key,
                    args: r.args,
                });
            }
            (Some(*target), None)
        }
    };
    if info.duration_ms <= 0 {
        return Err(Refusal::text("This server has no timing for that ability."));
    }
    let duration = ms(info.duration_ms);
    let active = Active {
        effect: info.effect,
        target,
        point,
        started: now,
        next: now,
        duration,
        cycles: 0,
        stopping: false,
        one_shot: info.one_shot(),
        guid: info.guid.clone(),
        offensive: info.offensive,
        reach: info.range + info.falloff,
    };
    // A round that cannot reach is not spent: a limited ability refuses a
    // target out of reach **[A]** (the client has no key for it; an ability
    // that repeats just waits for the target to close).
    if let (Some(t), true) = (target, info.charges.is_some() && active.reach > 0.0)
        && let (Some(me), Some(at)) = (
            w.roster().get(id).map(|p| p.motion.position),
            position_in(w, t, s.solar_system_id),
        )
        && crate::space_state::distance(me, at) > active.reach
    {
        return Err(Refusal::text("That target is out of reach."));
    }
    let Some(r) = rt_mut(w, id) else {
        return Err(Refusal::key("FighterTubeIsBusy"));
    };
    r.last_ability = Some(now);
    let sl = &mut r.slots[ix];
    if let Some(c) = sl.charges.as_mut() {
        *c -= 1;
    }
    sl.blocked_until = info
        .after_cycle_ms()
        .map(|after| now + duration + ms(after));
    let keep = active.clone();
    effects::around_speed(w, Via::Answer, &s, |w| {
        if let Some(sl) = rt_mut(w, id).map(|r| &mut r.slots[ix]) {
            sl.active = Some(keep);
        }
    });
    let snapshot = rt(w, id).cloned();
    activated_event(w, Via::Answer, &s, &info, &active, snapshot.as_ref(), now);
    fx(w, Via::Answer, &s, &active, true);
    Ok(())
}

/// `OnFighterAbilitySlotActivated` for an active ability.
fn activated_event(
    w: &mut Registry,
    via: Via,
    s: &Squadron,
    info: &Info,
    a: &Active,
    r: Option<&Runtime>,
    _now: BlueTime,
) {
    let charges = r.and_then(|r| r.slots[info.slot as usize].charges);
    let cooldown = info
        .after_cycle_ms()
        .map(|after| (a.started + a.duration, a.started + a.duration + ms(after)));
    tell(
        w,
        via,
        s.owner_id,
        Kind::Activated {
            fighter: s.id,
            slot: info.slot,
            charges,
            start: a.started,
            duration_ms: a.duration / BLUE_PER_MS,
            cooldown,
        },
    );
}

/// `fighterMgr.CmdDeactivateAbilitySlots(fighterIDs, abilitySlotID)`
/// [V `fightersSvc.py:190`]: `{fighterID: None | UserError}`. A running
/// ability ends with its cycle (`OnFighterAbilitySlotDeactivated`, from the
/// clock); one that is not running is answered `None`.
pub fn deactivate(w: &mut Registry, actor: Option<i64>, ids: &[i64], slot: i64) -> PyValue {
    let (_, _, list) = match controlled(w, actor, ids) {
        Ok(v) => v,
        Err(r) => return answer(ids.iter().map(|i| (*i, Some(r.clone()))).collect()),
    };
    let mut out = Vec::new();
    for (id, r) in list {
        let result = r.and_then(|_| {
            if !(0..3).contains(&slot) {
                return Err(Refusal::text("That is not an ability slot."));
            }
            let kamikaze = rt_mut(w, id)
                .and_then(|r| r.slots[slot as usize].active.as_mut())
                .map(|a| {
                    a.stopping = true;
                    a.effect == effect::KAMIKAZE
                })
                .unwrap_or(false);
            // A kamikaze is called off at once, not at its cycle's end
            // (`KamikazeAttack.Abort` [V]).
            if kamikaze {
                strike::abort(w, id);
            }
            Ok(())
        });
        out.push((id, result.err()));
    }
    answer(out)
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// How one cycle went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cycle {
    /// It ran (or could not reach, which is a cycle all the same).
    Ran,
    /// The target left: the ability ends.
    TargetGone,
}

/// Every flying squadron's abilities and orders, once a world tick: the
/// follow re-aim, the cycles that are due, the abilities that end.
pub(super) fn run(w: &mut Registry, now: BlueTime) {
    let ids: Vec<i64> = state(w).rt.keys().copied().collect();
    let mut advanced: Vec<i64> = Vec::new();
    for id in ids {
        let Some(s) = state(w).squadron(id).copied() else {
            continue;
        };
        let busy = rt(w, id).is_some_and(|r| {
            r.slots.iter().any(|sl| sl.active.is_some()) || matches!(r.order, Order::Follow { .. })
        });
        if !busy {
            continue;
        }
        if !advanced.contains(&s.solar_system_id) {
            w.advance_system(s.solar_system_id);
            advanced.push(s.solar_system_id);
        }
        reaim(w, id);
        // A squadron on its way home runs nothing.
        let flying = state(w).task(s.ship_id, s.tube).map(|t| t.status) == Some(Status::InSpace);
        for slot in 0..3 {
            if rt(w, id).is_some_and(|r| r.slots[slot].active.is_some()) {
                run_slot(w, id, slot, flying, now);
            }
        }
    }
    effects::reconcile(w, now);
}

fn run_slot(w: &mut Registry, id: i64, slot: usize, flying: bool, now: BlueTime) {
    for _ in 0..MAX_CYCLES_PER_TICK {
        // Re-read each round: a volley can change the squadron.
        let Some(s) = state(w).squadron(id).copied() else {
            return;
        };
        let Some(a) = rt(w, id).and_then(|r| r.slots[slot].active.clone()) else {
            return;
        };
        if !flying {
            end(w, &s, slot, &a, false);
            return;
        }
        if a.next > now {
            return;
        }
        if a.cycles >= 1 && (a.one_shot || a.stopping) {
            end(w, &s, slot, &a, true);
            return;
        }
        let outcome = cycle(w, &s, &a, slot, now);
        if outcome == Cycle::TargetGone {
            end(w, &s, slot, &a, false);
            return;
        }
        if let Some(act) = rt_mut(w, id).and_then(|r| r.slots[slot].active.as_mut()) {
            act.cycles += 1;
            act.next += act.duration;
        }
    }
}

/// An ability ends: the FX off, the slot free, the pilot told. `completed`:
/// it ran its cycle out (a micro jump drive jumps then, and only then).
pub(super) fn end(w: &mut Registry, s: &Squadron, slot: usize, a: &Active, completed: bool) {
    effects::around_speed(w, Via::Clock, s, |w| {
        if let Some(r) = rt_mut(w, s.id) {
            r.slots[slot].active = None;
        }
    });
    if completed && a.effect == effect::MICRO_JUMP_DRIVE {
        effects::jump(w, s, a);
    }
    fx(w, Via::Clock, s, a, false);
    tell(
        w,
        Via::Clock,
        s.owner_id,
        Kind::Deactivated {
            fighter: s.id,
            slot: slot as i64,
            reason: None,
        },
    );
}

/// One cycle's effect at `a.next`.
fn cycle(w: &mut Registry, s: &Squadron, a: &Active, slot: usize, now: BlueTime) -> Cycle {
    let Some(weapon) = weapon_of(a.effect) else {
        return if effects::is_target_ewar(a.effect) {
            effects::cycle(w, s, a, slot, now)
        } else if strike::runs(a.effect) {
            strike::cycle(w, s, a, now)
        } else {
            Cycle::Ran
        };
    };
    let Some(target) = a.target else {
        return Cycle::Ran;
    };
    let Some(victim) = target_in(w, s.solar_system_id, target) else {
        return Cycle::TargetGone;
    };
    let Some(me) = w.roster().get(s.id).copied() else {
        return Cycle::Ran;
    };
    let attr = |id: i64| {
        if id == 0 {
            0.0
        } else {
            skills::attr_of(w, s.id, s.type_id, id)
        }
    };
    // Reach: the effect's range and falloff attributes; no range stated, no
    // limit. **[A]** full damage out to range + falloff, none beyond:
    // R-DEEP §2.3 names the attributes and not the curve. Falsifier: the
    // combat log's damage at the edge of falloff.
    let at = crate::space_state::distance(me.motion.position, victim.motion.position);
    if a.reach > 0.0 && at > a.reach {
        return Cycle::Ran;
    }
    let signature = crate::dogma::hull(w, victim.ship_id)
        .map(|h| h.signature_radius)
        .filter(|sig| *sig > 0.0)
        .unwrap_or_else(|| {
            missile::target_facts(w, victim.character_id).1
                * crate::ewar::signature_factor(w, victim.ship_id)
        });
    let damage = scale(
        volley(attr, weapon, s.size),
        application(attr, weapon, signature, speed_of(victim.motion.velocity)),
    );
    if damage.total() <= 0.0 {
        return Cycle::Ran;
    }
    let shot = Shot {
        at: a.next,
        attacker_ship_id: s.id,
        weapon_item_id: s.id,
        weapon_type_id: s.type_id,
        target_ship_id: victim.ship_id,
        damage,
        hit_quality: combat::HIT_QUALITY,
    };
    combat::fire(w, shot, s.owner_id, victim, now);
    Cycle::Ran
}

// ---------------------------------------------------------------------------
// Losses
// ---------------------------------------------------------------------------

/// **A squadron's health ran out** (`kill_hooks::PLAYER`, coordinator): one
/// fighter dies. With others left the squadron is smaller (the item's
/// quantity, `OnInSpaceSquadronSizeChanged`, the slim's
/// `fighter.squadronSize`) and the next fighter is whole; the last one takes
/// the squadron out of space like a drone's death (the explosion, the locks,
/// the controller event, the item). Nothing for a ball that is not a
/// squadron.
pub fn destroyed(w: &mut Registry, id: i64, park: Vec<i64>) {
    let Some(mut s) = state(w).squadron(id).copied() else {
        return;
    };
    let now = crate::pyutil::filetime_now();
    if s.size > 1 {
        s.size -= 1;
        state_mut(w).live.insert(id, s);
        // The next fighter is whole.
        if let Some(p) = profile_of(w, id)
            && let Some(rec) = w.combat_mut().ship_mut(id)
        {
            *rec = crate::damage::ShipDamage::full(p, now);
        }
        if let Some((_, held)) = apply_held(
            w,
            &[Edit::Consume {
                item_id: id,
                quantity: 1,
            }],
        ) {
            release(w, Via::Clock, s.owner_id, held);
        }
        // An abandoned squadron is no longer the pilot's to the client.
        if !state(w).is_abandoned(id) {
            tell(
                w,
                Via::Clock,
                s.owner_id,
                Kind::SizeChanged {
                    fighter: id,
                    size: s.size,
                },
            );
        }
        let to = audience(w, s.solar_system_id);
        let stamp = w.next_destiny_stamp();
        if !to.is_empty() {
            w.schedule(
                Duration::ZERO,
                crate::hacking::CanSlimChange {
                    to,
                    stamp,
                    slim: slim(w, &s),
                    extras: vec![(SQUADRON_SIZE_KEY, PyValue::Int(s.size))],
                },
            );
        }
        let bars = combat::damage_state(w, id, now);
        let stamp = w.next_destiny_stamp();
        if let Some(p) = destiny::DamageStateChange::new(park, stamp, id, bars) {
            w.schedule(Duration::ZERO, p);
        }
        return;
    }
    // The last fighter.
    explode(w, &s, park);
}

/// **A squadron leaves space for good**: the explosion, the abilities' FX off,
/// the locks, the roster and the tables, the controller event (unless nobody
/// controls it) and the item destroyed. The last fighter's death and a
/// kamikaze's detonation (`strike.rs`) end here; an abandoned squadron that
/// despawns does too.
pub(super) fn explode(w: &mut Registry, s: &Squadron, park: Vec<i64>) {
    let id = s.id;
    for slot in 0..3 {
        if let Some(a) = rt(w, id).and_then(|r| r.slots[slot].active.clone()) {
            fx(w, Via::Clock, s, &a, false);
        }
    }
    let stamp = w.next_destiny_stamp();
    if let Some(k) = destiny::Kill::new(park, stamp, id, None, None) {
        w.schedule(Duration::ZERO, k);
    }
    crate::targeting::left_space(
        w,
        id,
        crate::targeting::Reason::Exploding,
        crate::targeting::Delivery::OnTheClock,
    );
    w.roster_mut().leave(id);
    let abandoned = state(w).is_abandoned(id);
    state_mut(w).live.remove(&id);
    state_mut(w).rt.remove(&id);
    state_mut(w).abandoned.remove(&id);
    strike::forget(w, id);
    effects::source_gone(w, id);
    if !abandoned {
        // The pilot's tube is empty: `OnFighterRemovedFromController` sets it
        // so unless it is landing or refuelling [V
        // `shipFighterState.py:118-126`], which a squadron shot to pieces is
        // not. An abandoned squadron has no tube and the client has forgotten
        // it (its handler `pop`s, and would raise on a squadron it does not
        // hold).
        state_mut(w).tasks.remove(&(s.ship_id, s.tube));
        tell(
            w,
            Via::Clock,
            s.owner_id,
            Kind::Removed {
                fighter: id,
                tube: s.tube,
            },
        );
    }
    match apply_held(w, &[Edit::Destroy { item_id: id }]) {
        Some((_, held)) => release(w, Via::Clock, s.owner_id, held),
        None => {
            crate::outln!("[fighters] the item of destroyed squadron {id} could not be removed")
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::fighters::tests as ft;
    use crate::fighters::tests::{ALICE, ALICE_SHIP, BAY, CARRIER, SYSTEM};
    use crate::pyutil::bstr;

    // Invented ids (the 99059xxxx band); **not the client's**. The effect ids
    // are the code's own constants ([`effect`]), the one thing the rows must
    // agree with.
    pub(in crate::fighters) const ATTACKER: i64 = 990_590_020;
    pub(in crate::fighters) const SALVOER: i64 = 990_590_021;
    pub(in crate::fighters) const PLAIN: i64 = 990_590_022;
    pub(in crate::fighters) const BANNED: i64 = 990_590_023;
    const AB_ATTACK: i64 = 990_701;
    const AB_SALVO: i64 = 990_702;
    const AB_PLAIN: i64 = 990_703;
    const AB_POINT: i64 = 990_704;
    const AB_BANNED: i64 = 990_705;
    /// The invented numbers of the fixture types.
    const DURATION_MS: i64 = 2_000;
    const SALVO_MS: i64 = 1_000;
    const RANGE: f64 = 10_000.0;
    const FALLOFF: f64 = 5_000.0;
    const EM: f64 = 1.0;
    const THERMAL: f64 = 2.0;
    const MULT: f64 = 1.5;
    const SALVO_EM: f64 = 5.0;
    const SALVO_MULT: f64 = 3.0;
    const COOLDOWN_S: f64 = 30.0;
    const REARM_S: f64 = 6.0;
    const CHARGES: i64 = 2;
    const SQUADRON: i64 = 6;
    pub(in crate::fighters) const MOVER: i64 = 990_590_024;
    pub(in crate::fighters) const ABER: i64 = 990_590_025;
    pub(in crate::fighters) const EWARER: i64 = 990_590_026;
    pub(in crate::fighters) const TACKLER: i64 = 990_590_027;
    /// Bob's hull: a sensor strength of 4 and a capacitor.
    pub(in crate::fighters) const JAM_TARGET: i64 = 990_590_030;
    const AB_MWD: i64 = 990_706;
    const AB_MJD: i64 = 990_707;
    const AB_EVASION: i64 = 990_708;
    const AB_AB: i64 = 990_709;
    const AB_WEB: i64 = 990_710;
    const AB_DISRUPT: i64 = 990_711;
    const AB_NEUT: i64 = 990_712;
    const AB_TACKLE: i64 = 990_713;
    const AB_ECM: i64 = 990_714;
    const AB_BOMB: i64 = 990_715;
    /// The fixture's numbers for the non-damage abilities.
    pub(in crate::fighters) const JUMP_M: f64 = 8_000.0;
    pub(in crate::fighters) const WEB_PCT: f64 = 60.0;
    pub(in crate::fighters) const WEB_RANGE: f64 = 5_000.0;
    pub(in crate::fighters) const WEB_FALLOFF: f64 = 2_000.0;
    pub(in crate::fighters) const TACKLE_PCT: f64 = 50.0;
    pub(in crate::fighters) const POINTS: f64 = 2.0;
    pub(in crate::fighters) const NEUT: f64 = 100.0;
    pub(in crate::fighters) const CAPACITOR: f64 = 500.0;

    pub(in crate::fighters) fn rows() -> String {
        let (atk, mis) = (effect::ATTACK, effect::MISSILES);
        let (wb, wd, nt, ec, tk) = (
            effect::WEB,
            effect::WARP_DISRUPT,
            effect::NEUT,
            effect::ECM,
            effect::TACKLE,
        );
        let (mw, ab, mj, ev, bm) = (
            effect::MICROWARPDRIVE,
            effect::AFTERBURNER,
            effect::MICRO_JUMP_DRIVE,
            effect::EVASION,
            effect::BOMB,
        );
        format!(
            "\
type\t{ATTACKER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2223=2000.0;2426=4000.0;2226={MULT};2227={EM};2228={THERMAL};2233={DURATION_MS}.0;2236={RANGE};2237={FALLOFF}\tFixture Attacker
type\t{SALVOER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;265=50.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2226={MULT};2227={EM};2233={DURATION_MS}.0;2236={RANGE};2130={SALVO_MULT};2131={SALVO_EM};2182={SALVO_MS}.0;2149={RANGE}\tFixture Salvoer
type\t{PLAIN}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;2212=1.0;2215={SQUADRON}.0;2226={MULT};2227={EM};2233={DURATION_MS}.0;2236={RANGE}\tFixture Plain
type\t{BANNED}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;2212=1.0;2215={SQUADRON}.0;2226={MULT};2227={EM};2233={DURATION_MS}.0;2236={RANGE}\tFixture Banned
effect\t{atk}\t2\t1\t2233\t0\t2236\t2237\t0\tfixtureFighterAttack\teffects.FixtureFighterAttack
effect\t{mis}\t2\t1\t2182\t0\t2149\t0\t0\tfixtureFighterSalvo\teffects.FixtureFighterSalvo
type\t{MOVER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2152=300.0;2153=400.0;2157=2000.0;2154={JUMP_M};2155=1000.0;2224=-50.0;2225=-30.0;2123=1000.0\tFixture Mover
type\t{ABER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2151=100.0;2158=1500.0\tFixture Afterburner
type\t{EWARER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2184={WEB_PCT};2183=1500.0;2186={WEB_RANGE};2187={WEB_FALLOFF};2205={POINTS};2203=1500.0;2204=8000.0;2211={NEUT};2208=1500.0;2209=5000.0;2210=2000.0\tFixture Ewar
type\t{TACKLER}\t99591\t87\t1\t1\t5.0\t0.0\t1000.0\t12.0\t0\t\t37=3000.0;9=50.0;263=100.0;552=30.0;2212=1.0;2215={SQUADRON}.0;2242={TACKLE_PCT};2425={POINTS};2238=1500.0;2239=6000.0;2246=4.0;2247=4.0;2248=4.0;2249=4.0;2220=1500.0;2221=7000.0;2222=3000.0\tFixture Tackler
type\t{JAM_TARGET}\t99594\t6\t1\t1\t20000.0\t100.0\t1000000.0\t40.0\t0\t\t37=1000.0;55=1000000.0;482={CAPACITOR};208=4.0;552=100.0\tFixture Jam Target
effect\t{wb}\t2\t1\t2183\t0\t2186\t2187\t0\tfixtureWeb\teffects.FixtureWeb
effect\t{wd}\t2\t1\t2203\t0\t2204\t0\t0\tfixtureDisrupt\teffects.FixtureDisrupt
effect\t{nt}\t2\t1\t2208\t0\t2209\t2210\t0\tfixtureNeut\teffects.FixtureNeut
effect\t{ec}\t2\t1\t2220\t0\t2221\t2222\t0\tfixtureEcm\teffects.FixtureEcm
effect\t{tk}\t2\t1\t2238\t0\t2239\t0\t0\tfixtureTackle\teffects.FixtureTackle
effect\t{mw}\t1\t0\t2157\t0\t0\t0\t0\tfixtureMwd\teffects.FixtureMwd
effect\t{ab}\t1\t0\t2158\t0\t0\t0\t0\tfixtureAb\teffects.FixtureAb
effect\t{mj}\t1\t0\t2155\t0\t0\t0\t0\tfixtureMjd\teffects.FixtureMjd
effect\t{ev}\t1\t0\t2123\t0\t0\t0\t0\tfixtureEvasion\teffects.FixtureEvasion
effect\t{bm}\t2\t1\t2349\t0\t0\t0\t0\tfixtureBomb\teffects.FixtureBomb
"
        )
    }

    pub(in crate::fighters) fn table() -> Fighters {
        let (atk, mis) = (effect::ATTACK, effect::MISSILES);
        let abilities = vec![
            format!("{AB_ATTACK}\titemTargeted\t0\t0\t{atk}\t990631\t\tAttack"),
            format!("{AB_SALVO}\titemTargeted\t0\t0\t{mis}\t990632\t\tSalvo"),
            format!("{AB_PLAIN}\tuntargeted\t0\t0\t{atk}\t990633\t\tPlain"),
            format!("{AB_POINT}\tpointTargeted\t0\t0\t{atk}\t990634\t\tPointed"),
            format!("{AB_BANNED}\titemTargeted\t1\t1\t{atk}\t990635\t\tBanned"),
            format!(
                "{AB_MWD}\tuntargeted\t0\t0\t{}\t990636\t\tMwd",
                effect::MICROWARPDRIVE
            ),
            format!(
                "{AB_MJD}\tpointTargeted\t0\t0\t{}\t990637\t\tMjd",
                effect::MICRO_JUMP_DRIVE
            ),
            format!(
                "{AB_EVASION}\tuntargeted\t0\t0\t{}\t990638\t\tEvasion",
                effect::EVASION
            ),
            format!(
                "{AB_AB}\tuntargeted\t0\t0\t{}\t990639\t\tAfterburner",
                effect::AFTERBURNER
            ),
            format!(
                "{AB_WEB}\titemTargeted\t0\t0\t{}\t990640\t\tWeb",
                effect::WEB
            ),
            format!(
                "{AB_DISRUPT}\titemTargeted\t0\t0\t{}\t990641\t\tDisrupt",
                effect::WARP_DISRUPT
            ),
            format!(
                "{AB_NEUT}\titemTargeted\t0\t0\t{}\t990642\t\tNeut",
                effect::NEUT
            ),
            format!(
                "{AB_TACKLE}\titemTargeted\t0\t0\t{}\t990643\t\tTackle",
                effect::TACKLE
            ),
            format!(
                "{AB_ECM}\titemTargeted\t0\t0\t{}\t990644\t\tEcm",
                effect::ECM
            ),
            format!(
                "{AB_BOMB}\tuntargeted\t0\t0\t{}\t990645\t\tBomb",
                effect::BOMB
            ),
        ];
        let slots = vec![
            format!("{ATTACKER}\t0\t{AB_ATTACK}\t\t\t"),
            format!("{ATTACKER}\t1\t{AB_ATTACK}\t{COOLDOWN_S}\t\t"),
            format!("{ATTACKER}\t2\t{AB_POINT}\t\t\t"),
            format!("{SALVOER}\t0\t{AB_ATTACK}\t\t\t"),
            format!("{SALVOER}\t2\t{AB_SALVO}\t\t{CHARGES}\t{REARM_S}"),
            format!("{PLAIN}\t0\t{AB_PLAIN}\t\t\t"),
            format!("{BANNED}\t0\t{AB_BANNED}\t\t\t"),
            format!("{PLAIN}\t1\t{AB_BOMB}\t\t\t"),
            format!("{MOVER}\t0\t{AB_MWD}\t30\t\t"),
            format!("{MOVER}\t1\t{AB_MJD}\t60\t\t"),
            format!("{MOVER}\t2\t{AB_EVASION}\t20\t\t"),
            format!("{ABER}\t0\t{AB_AB}\t30\t\t"),
            format!("{EWARER}\t0\t{AB_WEB}\t\t\t"),
            format!("{EWARER}\t1\t{AB_DISRUPT}\t\t\t"),
            format!("{EWARER}\t2\t{AB_NEUT}\t\t\t"),
            format!("{TACKLER}\t0\t{AB_TACKLE}\t\t\t"),
            format!("{TACKLER}\t1\t{AB_ECM}\t\t\t"),
        ];
        let mut abilities = abilities;
        let mut slots = slots;
        abilities.extend(super::super::fixture::abilities());
        slots.extend(super::super::fixture::slots());
        Fighters::parse(&abilities, &slots).expect("the fixture tables parse")
    }

    // -- helpers ------------------------------------------------------------

    /// Alice undocked in the carrier with `bay` as (type, count) stacks, each
    /// loaded into its own tube and launched, `INSPACE`; the squadron ids and
    /// the clock's start.
    pub(in crate::fighters) fn fly(bay: &[(i64, i64)]) -> (Registry, Vec<i64>, BlueTime) {
        launch_all(ft::world(CARRIER, bay), bay)
    }

    /// [`fly`], with Bob in [`JAM_TARGET`] undocked beside the first squadron,
    /// at rest, 3 km along x.
    pub(in crate::fighters) fn fly_with_bob(bay: &[(i64, i64)]) -> (Registry, Vec<i64>, BlueTime) {
        fly_with_bob_in(bay, JAM_TARGET)
    }

    /// [`fly_with_bob`] with Bob in `hull`.
    pub(in crate::fighters) fn fly_with_bob_in(
        bay: &[(i64, i64)],
        hull: i64,
    ) -> (Registry, Vec<i64>, BlueTime) {
        let reg = ft::world_with_bob(CARRIER, bay, Some(hull));
        let (mut reg, ids, t0) = launch_all(reg, bay);
        destiny::on_ballpark_bind(&mut reg, Some(ft::BOB));
        let at = reg.roster().get(ids[0]).expect("flying").motion.position;
        let bob = reg.roster_mut().get_mut(ft::BOB).expect("Bob in space");
        bob.motion.position = [at[0] + 3_000.0, at[1], at[2]];
        bob.motion.velocity = [0.0; 3];
        bob.motion.mode = destiny::Mode::Stop;
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        (reg, ids, t0)
    }

    pub(in crate::fighters) fn launch_all(
        mut reg: Registry,
        bay: &[(i64, i64)],
    ) -> (Registry, Vec<i64>, BlueTime) {
        ft::undock(&mut reg);
        let mut tubes = Vec::new();
        for n in 0..bay.len() {
            let tube = FIRST_TUBE + n as i64;
            ft::load_to(&mut reg, BAY + n as i64, tube).expect("loaded");
            tubes.push(tube);
        }
        ft::settle(&mut reg);
        let ids: Vec<i64> = tubes
            .iter()
            .map(|t| tube_item(&reg, ALICE_SHIP, *t).expect("loaded").0)
            .collect();
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        let answer =
            ft::call(&mut reg, "LaunchFightersFromTubes", &[ft::ints(&tubes)]).expect("answered");
        let PyValue::Dict(d) = answer else {
            panic!("a dict")
        };
        assert!(d.iter().all(|(v, _)| *v == PyValue::None), "{d:?}");
        let t0 = crate::pyutil::filetime_now();
        tick(&mut reg, t0 + ms(LAUNCH_MS + 1_000));
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        (reg, ids, t0)
    }

    /// One fixture rat `offset` metres beyond a squadron, along x.
    pub(in crate::fighters) fn rat_beyond(reg: &mut Registry, sq: i64, offset: f64) -> i64 {
        let at = reg.roster().get(sq).expect("in space").motion.position;
        crate::npc::spawn(
            reg,
            &crate::npc::ANGEL_HIJACKER,
            SYSTEM,
            [at[0] + offset, at[1], at[2]],
        )
        .expect("an NPC")
    }

    pub(in crate::fighters) fn move_call(
        reg: &mut Registry,
        ids: &[i64],
        command: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let mut all = vec![ft::ints(ids), bstr(command)];
        all.extend(args.iter().cloned());
        ft::call(reg, "ExecuteMovementCommandOnFighters", &all)
    }

    pub(in crate::fighters) fn activate_call(
        reg: &mut Registry,
        ids: &[i64],
        slot: i64,
        args: &[PyValue],
    ) -> PyValue {
        let mut all = vec![ft::ints(ids), PyValue::Int(slot)];
        all.extend(args.iter().cloned());
        ft::call(reg, "CmdActivateAbilitySlots", &all).expect("answered")
    }

    pub(in crate::fighters) fn deactivate_call(
        reg: &mut Registry,
        ids: &[i64],
        slot: i64,
    ) -> PyValue {
        ft::call(
            reg,
            "CmdDeactivateAbilitySlots",
            &[ft::ints(ids), PyValue::Int(slot)],
        )
        .expect("answered")
    }

    /// `{fighterID: None}`.
    pub(in crate::fighters) fn ok_for(ids: &[i64]) -> PyValue {
        PyValue::Dict(
            ids.iter()
                .map(|i| (PyValue::None, PyValue::Int(*i)))
                .collect(),
        )
    }

    /// The key of the refusal answered for `id`.
    pub(in crate::fighters) fn refusal_for(answer: &PyValue, id: i64) -> String {
        let PyValue::Dict(d) = answer else {
            panic!("a dict: {answer:?}")
        };
        let (v, _) = d
            .iter()
            .find(|(_, k)| *k == PyValue::Int(id))
            .unwrap_or_else(|| panic!("no entry for {id}: {d:?}"));
        ft::key_of_value(v)
    }

    /// Shield lost, hit points (the fixture rat's 40 m3... capacity), as a
    /// fraction of nothing: the plain number of points.
    pub(in crate::fighters) fn shield_lost(reg: &Registry, ship: i64, now: BlueTime) -> f64 {
        let cap = crate::npc::ANGEL_HIJACKER.hull.shield_capacity;
        reg.combat()
            .ship(ship)
            .and_then(|r| r.bars(now).shield)
            .map_or(0.0, |b| (1.0 - b.fraction) * cap)
    }

    pub(in crate::fighters) fn velocity_zero(reg: &mut Registry, id: i64) {
        if let Some(p) = reg.roster_mut().get_mut(id) {
            p.motion.mode = destiny::Mode::Stop;
            p.motion.velocity = [0.0; 3];
        }
    }

    // -- the arithmetic -----------------------------------------------------

    #[test]
    fn volley_is_the_attributes_times_the_multiplier_times_the_size() {
        let attr = |a: i64| match a {
            2226 => MULT,
            2227 => EM,
            2228 => THERMAL,
            _ => 0.0,
        };
        let v = volley(attr, &ATTACK_WEAPON, 4);
        assert_eq!(v.em, EM * MULT * 4.0);
        assert_eq!(v.thermal, THERMAL * MULT * 4.0);
        assert_eq!(v.kinetic + v.explosive, 0.0);
        assert_eq!(
            volley(attr, &ATTACK_WEAPON, 0).total(),
            0.0,
            "an empty squadron"
        );
    }

    #[test]
    fn the_explosion_factor_reduces_a_small_fast_target() {
        const CLOUD: f64 = 100.0;
        const VELOCITY: f64 = 50.0;
        const FACTOR: f64 = 3.0;
        const SENSITIVITY: f64 = 5.5;
        let attr = |a: i64| match a {
            2234 => CLOUD,
            2235 => VELOCITY,
            2231 => FACTOR,
            2232 => SENSITIVITY,
            _ => 0.0,
        };
        let big_slow = application(attr, &ATTACK_WEAPON, 400.0, 10.0);
        assert_eq!(big_slow, 1.0, "a target bigger than the cloud takes it all");
        let small = application(attr, &ATTACK_WEAPON, 25.0, 0.0);
        assert!(
            (small - 0.25).abs() < 1e-12,
            "signature over cloud: {small}"
        );
        let fast = application(attr, &ATTACK_WEAPON, 50.0, 500.0);
        let exponent = reduction_exponent(FACTOR, SENSITIVITY);
        assert!(((0.5_f64 * VELOCITY / 500.0).powf(exponent) - fast).abs() < 1e-12);
        assert!(fast < 0.5);
        // No cloud, no reduction.
        assert_eq!(application(|_| 0.0, &ATTACK_WEAPON, 1.0, 1.0e6), 1.0);
    }

    #[test]
    fn the_reduction_exponent_uses_the_sensitivity_when_there_is_one() {
        assert!((reduction_exponent(3.0, 5.5) - 3.0_f64.ln() / 5.5_f64.ln()).abs() < 1e-12);
        assert_eq!(
            reduction_exponent(0.7, 0.0),
            0.7,
            "no sensitivity: as stated"
        );
        assert_eq!(reduction_exponent(0.0, 5.5), 0.0);
    }

    #[test]
    fn the_ability_table_joins_the_effect_row_and_the_type() {
        let c = ft::content();
        let i = info(&c, SALVOER, 2).expect("the salvo");
        assert_eq!(i.effect, effect::MISSILES);
        assert_eq!(i.charges, Some(CHARGES));
        assert_eq!(i.rearm_ms, Some(secs_to_ms(REARM_S)));
        assert_eq!(i.duration_ms, SALVO_MS);
        assert_eq!(i.range, RANGE);
        assert_eq!(i.guid, "effects.FixtureFighterSalvo");
        assert!(i.one_shot(), "charges: one cycle per activation [V]");
        assert_eq!(i.after_cycle_ms(), Some(secs_to_ms(REARM_S)));
        let a = info(&c, ATTACKER, 1).expect("the cooled attack");
        assert!(a.one_shot() && a.cooldown_ms == Some(secs_to_ms(COOLDOWN_S)));
        let plain = info(&c, ATTACKER, 0).expect("the attack");
        assert!(!plain.one_shot(), "no charges, no cooldown: repeats");
        assert_eq!(plain.falloff, FALLOFF);
        assert!(info(&c, PLAIN, 2).is_err(), "an empty slot");
    }

    // -- movement -----------------------------------------------------------

    #[test]
    fn orbit_command_sets_destiny_orbit_at_range() {
        const AT: f64 = 3_500.0;
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 8_000.0);
        let _ = ft::pending(&mut reg);
        let r = move_call(
            &mut reg,
            &ids,
            "ORBIT",
            &[PyValue::Int(rat), PyValue::Real(AT)],
        );
        assert_eq!(r.expect("accepted"), PyValue::None);
        let p = reg.roster().get(ids[0]).copied().expect("in space");
        assert_eq!(
            p.motion.mode,
            destiny::Mode::Orbit {
                ball_id: rat,
                range: AT as f32
            }
        );
        assert_eq!(
            rt(&reg, ids[0]).map(|r| r.order),
            Some(Order::Orbit {
                target: rat,
                range: AT
            })
        );
        let sent = ft::pending(&mut reg);
        assert_eq!(
            ft::named(&sent, "DoDestinyUpdate").len(),
            1,
            "one batch with the orbit: {sent:?}"
        );
    }

    #[test]
    fn return_and_orbit_is_an_orbit_on_the_carrier() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 8_000.0);
        move_call(
            &mut reg,
            &ids,
            "ORBIT",
            &[PyValue::Int(rat), PyValue::Int(500)],
        )
        .expect("away");
        if let Some(r) = rt_mut(&mut reg, ids[0]) {
            r.last_move = None;
        }
        move_call(
            &mut reg,
            &ids,
            "ORBIT",
            &[PyValue::Int(ALICE_SHIP), PyValue::Int(DEFAULT_ORBIT as i64)],
        )
        .expect("home");
        assert_eq!(
            rt(&reg, ids[0]).map(|r| r.order),
            Some(Order::Orbit {
                target: ALICE_SHIP,
                range: DEFAULT_ORBIT
            })
        );
    }

    #[test]
    fn follow_is_recorded_as_a_goto_the_tick_re_aims() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 8_000.0);
        move_call(
            &mut reg,
            &ids,
            "FOLLOW",
            &[PyValue::Int(rat), PyValue::Int(300)],
        )
        .expect("ok");
        assert_eq!(
            rt(&reg, ids[0]).map(|r| r.order),
            Some(Order::Follow {
                target: rat,
                range: 300.0
            })
        );
        // The target moves; the next tick re-aims the record.
        let moved = [1.0e5, 2.0e4, -3.0e4];
        reg.roster_mut().get_mut(rat).expect("rat").motion.position = moved;
        tick(&mut reg, t0 + ms(30_000));
        let p = reg.roster().get(ids[0]).copied().expect("in space");
        assert!(
            matches!(p.motion.mode, destiny::Mode::Goto(_)),
            "{:?}",
            p.motion.mode
        );
    }

    #[test]
    fn stop_and_goto_point() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        move_call(&mut reg, &ids, "STOP", &[]).expect("stopped");
        assert_eq!(
            reg.roster().get(ids[0]).map(|p| p.motion.mode),
            Some(destiny::Mode::Stop)
        );
        assert_eq!(rt(&reg, ids[0]).map(|r| r.order), Some(Order::Stop));
        if let Some(r) = rt_mut(&mut reg, ids[0]) {
            r.last_move = None;
        }
        let point = PyValue::Tuple(vec![
            PyValue::Real(1_000.5),
            PyValue::Int(-20),
            PyValue::Real(3.0e4),
        ]);
        move_call(&mut reg, &ids, "GOTO_POINT", &[point]).expect("going");
        assert_eq!(
            reg.roster().get(ids[0]).map(|p| p.motion.mode),
            Some(destiny::Mode::Goto([1_000.5, -20.0, 3.0e4]))
        );
    }

    #[test]
    fn a_movement_command_with_the_wrong_arguments_is_refused() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        for (command, args) in [
            ("SPIN", vec![]),
            ("ORBIT", vec![PyValue::Int(1)]),
            ("ORBIT", vec![PyValue::Bytes(vec![]), PyValue::Int(1)]),
            (
                "GOTO_POINT",
                vec![PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2)])],
            ),
            (
                "GOTO_POINT",
                vec![PyValue::Tuple(vec![
                    PyValue::Real(f64::NAN),
                    PyValue::Int(0),
                    PyValue::Int(0),
                ])],
            ),
        ] {
            let e = move_call(&mut reg, &ids, command, &args).expect_err(command);
            assert!(
                matches!(e, ServiceError::BadArguments { .. }),
                "{command}: {e:?}"
            );
        }
        // And a target that is nowhere.
        let e = move_call(
            &mut reg,
            &ids,
            "ORBIT",
            &[PyValue::Int(990_000_999_999), PyValue::Int(500)],
        )
        .expect_err("no such ball");
        assert_eq!(ft::key_of(&e), "CustomNotify");
    }

    #[test]
    fn command_beyond_control_range_is_refused() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        velocity_zero(&mut reg, ids[0]);
        // The carrier's lock range is 20 km in the fixture.
        reg.roster_mut()
            .get_mut(ids[0])
            .expect("sq")
            .motion
            .position = [9.0e5, 0.0, 0.0];
        let e = move_call(&mut reg, &ids, "STOP", &[]).expect_err("too far");
        assert_eq!(ft::key_of(&e), "CannotControlFighterTooDistant");
        let a = activate_call(&mut reg, &ids, 0, &[PyValue::Int(ALICE_SHIP)]);
        assert_eq!(refusal_for(&a, ids[0]), "CannotControlFighterTooDistant");
    }

    #[test]
    fn a_squadron_that_is_not_the_pilots_is_refused() {
        let (mut reg, _, _) = fly(&[(ATTACKER, SQUADRON)]);
        let e = move_call(&mut reg, &[12_345], "STOP", &[]).expect_err("a stranger");
        assert_eq!(
            ft::key_of(&e),
            "CannotControlFighterNotControlledByYourShip"
        );
        let a = activate_call(&mut reg, &[12_345], 0, &[]);
        assert_eq!(
            refusal_for(&a, 12_345),
            "CannotControlFighterNotControlledByYourShip"
        );
        let d = deactivate_call(&mut reg, &[12_345], 0);
        assert_eq!(
            refusal_for(&d, 12_345),
            "CannotControlFighterNotControlledByYourShip"
        );
    }

    #[test]
    fn a_second_command_inside_the_throttle_is_refused() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        move_call(&mut reg, &ids, "STOP", &[]).expect("first");
        let e = move_call(&mut reg, &ids, "STOP", &[]).expect_err("second");
        assert_eq!(ft::key_of(&e), "FighterMovementCommandThrottled");
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        let first = activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        assert_eq!(first, ok_for(&ids));
        let second = activate_call(
            &mut reg,
            &ids,
            2,
            &[PyValue::Tuple(vec![
                PyValue::Int(0),
                PyValue::Int(0),
                PyValue::Int(0),
            ])],
        );
        assert_eq!(
            refusal_for(&second, ids[0]),
            "FighterAbilityCommandThrottled"
        );
    }

    #[test]
    fn a_squadron_on_its_way_home_takes_no_orders() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        ft::call(&mut reg, "RecallFightersToTubes", &[ft::ints(&ids)]).expect("recalled");
        // The bulk commands the client sends name it all the same: skipped.
        move_call(&mut reg, &ids, "STOP", &[]).expect("skipped without a word");
        let a = activate_call(&mut reg, &ids, 0, &[PyValue::Int(ALICE_SHIP)]);
        assert_eq!(refusal_for(&a, ids[0]), "FighterTubeIsBusy");
    }

    // -- abilities ----------------------------------------------------------

    #[test]
    fn ability_target_mode_is_enforced() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON), (PLAIN, SQUADRON)]);
        let (attacker, plain) = (ids[0], ids[1]);
        let rat = rat_beyond(&mut reg, attacker, 4_000.0);
        // Item-targeted: no target, the carrier, a squadron, a ball nowhere.
        for args in [
            vec![],
            vec![PyValue::Int(ALICE_SHIP)],
            vec![PyValue::Int(plain)],
            vec![PyValue::Int(990_000_999_999)],
            vec![PyValue::Bytes(vec![1])],
        ] {
            let a = activate_call(&mut reg, &[attacker], 0, &args);
            assert_eq!(refusal_for(&a, attacker), "CustomNotify", "{args:?}");
            if let Some(r) = rt_mut(&mut reg, attacker) {
                r.last_ability = None;
            }
        }
        assert!(rt(&reg, attacker).is_some_and(|r| r.slots[0].active.is_none()));
        // Point-targeted wants a point.
        let a = activate_call(&mut reg, &[attacker], 2, &[PyValue::Int(rat)]);
        assert_eq!(refusal_for(&a, attacker), "CustomNotify");
        if let Some(r) = rt_mut(&mut reg, attacker) {
            r.last_ability = None;
        }
        let point = PyValue::Tuple(vec![PyValue::Int(1), PyValue::Int(2), PyValue::Int(3)]);
        assert_eq!(
            activate_call(&mut reg, &[attacker], 2, &[point]),
            ok_for(&[attacker])
        );
        // Untargeted needs nothing.
        assert_eq!(activate_call(&mut reg, &[plain], 0, &[]), ok_for(&[plain]));
        // The real thing.
        if let Some(r) = rt_mut(&mut reg, attacker) {
            r.last_ability = None;
        }
        assert_eq!(
            activate_call(&mut reg, &[attacker], 0, &[PyValue::Int(rat)]),
            ok_for(&[attacker])
        );
    }

    #[test]
    fn an_empty_slot_and_a_bad_slot_are_refused() {
        let (mut reg, ids, _) = fly(&[(PLAIN, SQUADRON)]);
        for slot in [1, 2, 3, -1, 7] {
            let a = activate_call(&mut reg, &ids, slot, &[]);
            assert_eq!(refusal_for(&a, ids[0]), "CustomNotify", "slot {slot}");
        }
    }

    #[test]
    fn activation_tells_the_pilot_and_the_ball_in_the_clients_shapes() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        let _ = ft::pending(&mut reg);
        let t = crate::pyutil::filetime_now();
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let sent = ft::pending(&mut reg);
        let on = ft::named(&sent, "OnFighterAbilitySlotActivated");
        assert_eq!(on.len(), 1, "{sent:?}");
        let a = &on[0].2;
        assert_eq!(on[0].0, Some(vec![ALICE]));
        assert_eq!(a[0], PyValue::Int(ids[0]));
        assert_eq!(a[1], PyValue::Int(0));
        assert_eq!(a[2], PyValue::None, "no charges on a repeating attack");
        let PyValue::Int(start) = a[3] else {
            panic!("a start time")
        };
        assert!((start - t).abs() < ms(5_000));
        assert_eq!(a[4], PyValue::Int(DURATION_MS), "durationMs");
        assert_eq!(a[5], PyValue::None, "no cooldown on a repeating attack");
        // The visual: the effect row's guid, offensive, repeating, the
        // squadron as the module.
        let fx = ft::named(&sent, "OnSpecialFX");
        assert_eq!(fx.len(), 1, "{sent:?}");
        assert_eq!(fx[0].2[0], PyValue::Int(ids[0]));
        assert_eq!(fx[0].2[1], PyValue::Int(ids[0]));
        assert_eq!(fx[0].2[3], PyValue::Int(rat));
        assert_eq!(fx[0].2[5], bstr("effects.FixtureFighterAttack"));
        assert_eq!(fx[0].2[6], PyValue::Bool(true));
        assert_eq!(fx[0].2[10], PyValue::Int(1000), "repeats until stopped");
    }

    #[test]
    fn attack_damage_scales_with_squadron_size() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON), (ATTACKER, SQUADRON / 2)]);
        let big_rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        let small_rat = rat_beyond(&mut reg, ids[1], -4_000.0);
        let _ = ft::pending(&mut reg);
        activate_call(&mut reg, &[ids[0]], 0, &[PyValue::Int(big_rat)]);
        activate_call(&mut reg, &[ids[1]], 0, &[PyValue::Int(small_rat)]);
        let _ = t0;
        let now = crate::pyutil::filetime_now() + ms(100);
        tick(&mut reg, now);
        let per_fighter = (EM + THERMAL) * MULT;
        let big = shield_lost(&reg, big_rat, now);
        let small = shield_lost(&reg, small_rat, now);
        // Shield regenerates a little between the volley and the read.
        let want_big = per_fighter * SQUADRON as f64;
        let want_small = per_fighter * (SQUADRON / 2) as f64;
        assert!(
            (big - want_big).abs() < 0.05 * want_big,
            "{big} vs {want_big}"
        );
        assert!(
            (small - want_small).abs() < 0.05 * want_small,
            "{small} vs {want_small}"
        );
        assert!(big > small * 1.9, "twice the fighters, twice the damage");
    }

    #[test]
    fn the_volley_is_the_owners_shot_through_combat_and_crimewatch() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        let _ = ft::pending(&mut reg);
        tick(&mut reg, t0 + ms(10_000));
        let sent = ft::due(&mut reg);
        let log = ft::named(&sent, "OnDamageMessage");
        assert!(!log.is_empty(), "the owner's combat-log line: {sent:?}");
        assert_eq!(log[0].0, Some(vec![ALICE]));
        let PyValue::Dict(d) = &log[0].2[0] else {
            panic!("a dict")
        };
        let weapon = d
            .iter()
            .find(|(_, k)| *k == bstr("weapon"))
            .map(|(v, _)| v.clone());
        assert_eq!(
            weapon,
            Some(PyValue::Int(ATTACKER)),
            "the squadron's type is the weapon"
        );
        assert!(
            reg.combat().ship(rat).is_some(),
            "the rat has a health record: the shot went through `combat::fire`"
        );
    }

    #[test]
    fn a_repeating_attack_fires_every_cycle_until_deactivated() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        let _ = ft::pending(&mut reg);
        let now = t0 + ms(10_000);
        // Ten seconds of clock owe the first volley at once and one more per
        // cycle after: (10 s / 2 s) volleys at most, bounded per tick.
        tick(&mut reg, now);
        let first = ft::due(&mut reg);
        let volleys = ft::named(&first, "OnDamageMessage").len();
        assert!(volleys >= 2, "a stalled clock owes several: {volleys}");
        assert!(volleys as u32 <= MAX_CYCLES_PER_TICK);
        // Deactivate: it ends at the cycle's end, with the FX off and the
        // slot told.
        let d = deactivate_call(&mut reg, &ids, 0);
        assert_eq!(d, ok_for(&ids));
        tick(&mut reg, now + ms(2 * DURATION_MS));
        let sent = ft::due(&mut reg);
        let off = ft::named(&sent, "OnFighterAbilitySlotDeactivated");
        assert_eq!(off.len(), 1, "{sent:?}");
        assert_eq!(
            off[0].2,
            vec![PyValue::Int(ids[0]), PyValue::Int(0), PyValue::None]
        );
        let fx = ft::named(&sent, "OnSpecialFX");
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].2[7], PyValue::Int(0), "the FX stops");
        assert!(rt(&reg, ids[0]).is_some_and(|r| r.slots[0].active.is_none()));
        // And it no longer fires.
        let _ = ft::due(&mut reg);
        tick(&mut reg, now + ms(10 * DURATION_MS));
        assert!(ft::named(&ft::due(&mut reg), "OnDamageMessage").is_empty());
    }

    #[test]
    fn salvo_spends_a_charge_and_rearms() {
        let (mut reg, ids, t0) = fly(&[(SALVOER, SQUADRON)]);
        let sq = ids[0];
        let rat = rat_beyond(&mut reg, sq, 4_000.0);
        let _ = ft::pending(&mut reg);
        // The first salvo: one charge gone, told with the cooldown that the
        // rearm time makes.
        assert_eq!(
            activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let sent = ft::pending(&mut reg);
        let a = &ft::named(&sent, "OnFighterAbilitySlotActivated")[0].2;
        assert_eq!(a[2], PyValue::Int(CHARGES - 1), "remaining charges");
        assert_eq!(a[4], PyValue::Int(SALVO_MS));
        let (PyValue::Int(start), PyValue::Tuple(cd)) = (&a[3], &a[5]) else {
            panic!("a start and a cooldown pair: {a:?}")
        };
        assert_eq!(
            cd,
            &vec![
                PyValue::Int(start + ms(SALVO_MS)),
                PyValue::Int(start + ms(SALVO_MS) + ms(secs_to_ms(REARM_S)))
            ],
            "the button is grey from the end of the cycle for the rearm time"
        );
        // One cycle: the volley lands, then the ability ends by itself.
        // Inside the cycle: the volley has landed and the ability still runs.
        tick(&mut reg, crate::pyutil::filetime_now() + ms(SALVO_MS / 2));
        let sent = ft::due(&mut reg);
        assert_eq!(ft::named(&sent, "OnDamageMessage").len(), 1, "{sent:?}");
        assert!(ft::named(&sent, "OnFighterAbilitySlotDeactivated").is_empty());
        let PyValue::Dict(msg) = &ft::named(&sent, "OnDamageMessage")[0].2[0] else {
            panic!("a dict")
        };
        let dealt = msg
            .iter()
            .find(|(_, k)| *k == bstr("damage"))
            .map(|(v, _)| v.clone());
        assert_eq!(
            dealt,
            Some(PyValue::Real(SALVO_EM * SALVO_MULT * SQUADRON as f64)),
            "a salvo: the type's damage times its multiplier times the squadron"
        );
        // At the cycle's end it ends by itself, with the FX off.
        tick(&mut reg, t0 + ms(7_000));
        let sent = ft::due(&mut reg);
        assert_eq!(
            ft::named(&sent, "OnDamageMessage").len(),
            0,
            "one cycle only"
        );
        assert_eq!(
            ft::named(&sent, "OnFighterAbilitySlotDeactivated").len(),
            1,
            "{sent:?}"
        );
        assert_eq!(ft::named(&sent, "OnSpecialFX").len(), 1, "the FX stops");
        // Inside the rearm: refused. After it: the second charge.
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
        }
        let a = activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        assert_eq!(refusal_for(&a, sq), "FighterAbilityIsBlocked");
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
            r.slots[2].blocked_until = None;
        }
        assert_eq!(
            activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let sent = ft::pending(&mut reg);
        let a = &ft::named(&sent, "OnFighterAbilitySlotActivated")[0].2;
        assert_eq!(a[2], PyValue::Int(0), "the last charge");
        // Out of charges: refused however long it waits.
        tick(&mut reg, t0 + ms(20_000));
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
            r.slots[2].blocked_until = None;
        }
        let a = activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        assert_eq!(refusal_for(&a, sq), "FighterAbilityIsBlocked");
    }

    #[test]
    fn a_squadron_launched_again_has_its_charges_back() {
        let (mut reg, ids, _) = fly(&[(SALVOER, SQUADRON)]);
        if let Some(r) = rt_mut(&mut reg, ids[0]) {
            r.slots[2].charges = Some(0);
        }
        let fresh = Runtime::launched(&reg, SALVOER, ALICE_SHIP);
        assert_eq!(fresh.slots[2].charges, Some(CHARGES));
        assert_eq!(fresh.slots[0].charges, None, "an attack has no charges");
    }

    #[test]
    fn cooldown_blocks_reactivation_and_is_in_state() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let sq = ids[0];
        let rat = rat_beyond(&mut reg, sq, 4_000.0);
        let _ = ft::pending(&mut reg);
        assert_eq!(
            activate_call(&mut reg, &ids, 1, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let sent = ft::pending(&mut reg);
        let a = &ft::named(&sent, "OnFighterAbilitySlotActivated")[0].2;
        assert_eq!(a[1], PyValue::Int(1));
        assert_eq!(a[2], PyValue::None, "a cooldown is not a charge");
        let PyValue::Tuple(cd) = &a[5] else {
            panic!("the cooldown pair: {a:?}")
        };
        let (PyValue::Int(from), PyValue::Int(to)) = (&cd[0], &cd[1]) else {
            panic!("two times")
        };
        assert_eq!(to - from, ms(secs_to_ms(COOLDOWN_S)));
        // One cycle, then over.
        tick(&mut reg, t0 + ms(10_000));
        tick(&mut reg, t0 + ms(13_000));
        let sent = ft::due(&mut reg);
        assert_eq!(ft::named(&sent, "OnDamageMessage").len(), 1, "{sent:?}");
        assert_eq!(ft::named(&sent, "OnFighterAbilitySlotDeactivated").len(), 1);
        // The cooldown still runs: refused.
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
        }
        let a = activate_call(&mut reg, &ids, 1, &[PyValue::Int(rat)]);
        assert_eq!(refusal_for(&a, sq), "FighterAbilityIsBlocked");
        // The squadron's `GetFightersForShip` row still reads as before.
        let PyValue::Tuple(parts) = ft::call(&mut reg, "GetFightersForShip", &[]).expect("rows")
        else {
            panic!("a 3-tuple")
        };
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn the_security_bans_are_refused() {
        let (mut reg, ids, _) = fly(&[(BANNED, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        let space = crimewatch::space_of(&reg, SYSTEM);
        let a = activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        match space {
            crimewatch::Space::Zero => assert_eq!(a, ok_for(&ids), "null sec allows it"),
            _ => assert_eq!(refusal_for(&a, ids[0]), "FighterAbilityIsBlocked"),
        }
    }

    #[test]
    fn a_target_out_of_reach_takes_no_damage_and_a_salvo_is_not_spent() {
        let (mut reg, ids, t0) = fly(&[(SALVOER, SQUADRON)]);
        let sq = ids[0];
        velocity_zero(&mut reg, sq);
        let rat = rat_beyond(&mut reg, sq, RANGE + FALLOFF + 5_000.0);
        // A salvo (limited) will not be thrown at what it cannot reach.
        let a = activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        assert_eq!(refusal_for(&a, sq), "CustomNotify");
        assert_eq!(
            rt(&reg, sq).and_then(|r| r.slots[2].charges),
            Some(CHARGES),
            "no round was spent"
        );
        // A repeating attack waits for the target to close: active, no damage.
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
        }
        assert_eq!(
            activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]),
            ok_for(&ids)
        );
        let _ = ft::pending(&mut reg);
        tick(&mut reg, t0 + ms(10_000));
        assert!(ft::named(&ft::due(&mut reg), "OnDamageMessage").is_empty());
        assert!(rt(&reg, sq).is_some_and(|r| r.slots[0].active.is_some()));
    }

    #[test]
    fn the_target_leaving_ends_the_ability() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        reg.roster_mut().leave(rat);
        let _ = ft::pending(&mut reg);
        tick(&mut reg, t0 + ms(10_000));
        let sent = ft::due(&mut reg);
        assert!(ft::named(&sent, "OnDamageMessage").is_empty());
        let off = ft::named(&sent, "OnFighterAbilitySlotDeactivated");
        assert_eq!(off.len(), 1, "{sent:?}");
        assert_eq!(off[0].2[2], PyValue::None, "no failure reason");
        assert!(rt(&reg, ids[0]).is_some_and(|r| r.slots[0].active.is_none()));
    }

    #[test]
    fn a_squadron_killing_a_rat_is_the_owners_kill() {
        let (mut reg, ids, t0) = fly(&[(SALVOER, SQUADRON)]);
        let sq = ids[0];
        let rat = rat_beyond(&mut reg, sq, 4_000.0);
        let hp = crate::npc::ANGEL_HIJACKER.hull.shield_capacity
            + crate::npc::ANGEL_HIJACKER.hull.armor_hp
            + crate::npc::ANGEL_HIJACKER.hull.hp;
        // Two fixture salvos do 2 x 5 x 3 x 6 = 180 > the rat's 135, one
        // does not.
        let salvo = SALVO_EM * SALVO_MULT * SQUADRON as f64;
        assert!(salvo < hp && 2.0 * salvo > hp);
        activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        let _ = ft::pending(&mut reg);
        tick(&mut reg, t0 + ms(10_000));
        assert!(reg.roster().get(rat).is_some(), "one salvo leaves it alive");
        if let Some(r) = rt_mut(&mut reg, sq) {
            r.last_ability = None;
            r.slots[2].blocked_until = None;
        }
        activate_call(&mut reg, &ids, 2, &[PyValue::Int(rat)]);
        tick(&mut reg, t0 + ms(20_000));
        assert!(reg.roster().get(rat).is_none(), "the rat is dead");
        let sent = ft::due(&mut reg);
        assert!(
            sent.iter()
                .any(|(a, n, _)| *n == "OnDamageMessage" && a == &Some(vec![ALICE])),
            "Alice saw her squadron's kill"
        );
        assert!(reg.roster().get(sq).is_some(), "and the squadron is fine");
    }

    #[test]
    fn abilities_stop_when_the_squadron_recalls() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let rat = rat_beyond(&mut reg, ids[0], 4_000.0);
        activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]);
        ft::call(&mut reg, "RecallFightersToTubes", &[ft::ints(&ids)]).expect("recalled");
        let _ = ft::pending(&mut reg);
        tick(&mut reg, t0 + ms(10_000));
        let sent = ft::due(&mut reg);
        assert!(ft::named(&sent, "OnDamageMessage").is_empty(), "{sent:?}");
        assert_eq!(
            ft::named(&sent, "OnFighterAbilitySlotDeactivated").len(),
            1,
            "{sent:?}"
        );
    }

    // -- losses -------------------------------------------------------------

    /// A volley at the squadron from an NPC, as `combat::fire` takes it.
    pub(in crate::fighters) fn shoot_at(
        reg: &mut Registry,
        sq: i64,
        from: i64,
        em: f64,
        now: BlueTime,
    ) {
        let victim = reg
            .roster()
            .by_ship(sq)
            .copied()
            .expect("the squadron's ball");
        combat::fire(
            reg,
            Shot {
                at: now,
                attacker_ship_id: from,
                weapon_item_id: from,
                weapon_type_id: crate::npc::ANGEL_HIJACKER.type_id,
                target_ship_id: sq,
                damage: Damage { em, ..Damage::ZERO },
                hit_quality: combat::HIT_QUALITY,
            },
            from,
            victim,
            now,
        );
    }

    /// One fighter's whole health in the fixture: shield 100, armour 50, hull 50.
    const FIGHTER_HP: f64 = 200.0;

    #[test]
    fn a_squadron_has_one_fighters_health_per_fighter() {
        let (reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        let p = profile_of(&reg, ids[0]).expect("a profile");
        assert_eq!(
            p.shield_capacity + p.armor_hp + p.hp,
            FIGHTER_HP,
            "shield + armour + hull of one fighter"
        );
        assert_eq!(
            crate::dogma::profile(&reg, ids[0]).map(|p| p.hp),
            Some(p.hp),
            "dogma::profile reads it through the fighter arm"
        );
    }

    #[test]
    fn a_dead_fighter_decrements_size_and_updates_slim() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, SQUADRON)]);
        let sq = ids[0];
        let rat = rat_beyond(&mut reg, sq, 4_000.0);
        let _ = ft::pending(&mut reg);
        // Half a fighter: the squadron is still whole, the bars moved.
        shoot_at(&mut reg, sq, rat, FIGHTER_HP / 2.0, t0 + ms(2_000));
        assert_eq!(state(&reg).squadron(sq).map(|s| s.size), Some(SQUADRON));
        let _ = ft::due(&mut reg);
        // One fighter's worth and a little over: one fighter lost.
        shoot_at(&mut reg, sq, rat, FIGHTER_HP, t0 + ms(2_100));
        assert_eq!(
            state(&reg).squadron(sq).map(|s| s.size),
            Some(SQUADRON - 1),
            "the squadron lost one"
        );
        assert_eq!(
            reg.store().get(sq).map(|i| i.quantity),
            Some(SQUADRON - 1),
            "the item is the squadron"
        );
        assert!(reg.roster().get(sq).is_some(), "the others fly on");
        let sent = ft::due(&mut reg);
        let size = ft::named(&sent, "OnInSpaceSquadronSizeChanged");
        assert_eq!(size.len(), 1, "{sent:?}");
        assert_eq!(size[0].0, Some(vec![ALICE]));
        assert_eq!(
            size[0].2,
            vec![PyValue::Int(sq), PyValue::Int(SQUADRON - 1)]
        );
        // The slim, in the instance form, carries the dotted key.
        let slim_change = sent
            .iter()
            .find(|(_, n, a)| {
                *n == "DoDestinyUpdate"
                    && format!("{a:?}").contains(&format!("{:?}", bstr("OnSlimItemChange")))
            })
            .expect("OnSlimItemChange");
        let text = format!("{:?}", slim_change.2);
        assert!(
            text.contains(&format!("{:?}", SQUADRON_SIZE_KEY.as_bytes())),
            "{text}"
        );
        // The next fighter is whole: the record is full again.
        let bars = reg.combat().ship(sq).map(|r| r.bars(t0 + ms(2_200)));
        assert!(
            bars.is_none_or(|b| b.shield.is_none_or(|s| s.fraction > 0.99)),
            "{bars:?}"
        );
    }

    #[test]
    fn the_last_fighter_takes_the_squadron_out_of_space() {
        let (mut reg, ids, t0) = fly(&[(ATTACKER, 1)]);
        let sq = ids[0];
        let rat = rat_beyond(&mut reg, sq, 4_000.0);
        let total = ft::fighter_total(&reg);
        let _ = ft::pending(&mut reg);
        shoot_at(&mut reg, sq, rat, FIGHTER_HP * 2.0, t0 + ms(2_000));
        assert!(reg.roster().get(sq).is_none(), "the ball is gone");
        assert!(state(&reg).squadron(sq).is_none());
        assert!(!state(&reg).rt.contains_key(&sq));
        assert!(reg.store().get(sq).is_none(), "the item is gone");
        assert_eq!(ft::fighter_total(&reg), total - 1);
        assert!(
            state(&reg).task(ALICE_SHIP, FIRST_TUBE).is_none(),
            "the tube is empty, not busy"
        );
        let sent = ft::due(&mut reg);
        let removed = ft::named(&sent, "OnFighterRemovedFromController");
        assert_eq!(removed.len(), 1, "{sent:?}");
        assert_eq!(
            removed[0].2,
            vec![PyValue::Int(sq), PyValue::Int(FIRST_TUBE)]
        );
        assert!(
            sent.iter().any(|(_, n, a)| *n == "DoDestinyUpdate"
                && format!("{a:?}").contains(&format!("{:?}", bstr("RemoveBall")))),
            "the explosion: {:?}",
            sent.iter().map(|(_, n, _)| n).collect::<Vec<_>>()
        );
        // And the tube can be loaded again: the bay has more.
        let again = ft::call(&mut reg, "GetFightersForShip", &[]).expect("rows");
        let PyValue::Tuple(parts) = again else {
            panic!("3-tuple")
        };
        assert_eq!(parts[0], PyValue::List(vec![]));
        assert_eq!(parts[1], PyValue::List(vec![]));
    }

    #[test]
    fn a_shot_at_a_ship_or_a_rat_is_not_a_fighters_loss() {
        let (mut reg, ids, _) = fly(&[(ATTACKER, SQUADRON)]);
        destroyed(&mut reg, ALICE_SHIP, vec![ALICE]);
        destroyed(&mut reg, 990_000_999_999, vec![ALICE]);
        assert_eq!(state(&reg).squadron(ids[0]).map(|s| s.size), Some(SQUADRON));
    }

    #[test]
    fn the_fighters_hook_is_a_row_of_the_player_kill_table() {
        assert!(
            crate::kill_hooks::PLAYER
                .iter()
                .any(|r| r.name == "fighters_destroyed"),
            "kill_hooks::PLAYER carries the fighters row"
        );
    }
}
