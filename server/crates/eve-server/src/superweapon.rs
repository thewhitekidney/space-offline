//! **Doomsdays, part a** (NW-9a, issue #195; spec `handback/r-null-war/
//! report.md` §6): the **legacy targeted** doomsday (effects 4489-4492) and
//! the **Lance** (a cylinder, `doomsdayAOEShape` 2429 = 1). The Reaper slice
//! is 9b (below), the Disruptive Lance and the Bosonic cone are 9c; the Oscillator is 9d.
//!
//! # Facts
//!
//! - **[V client]** a module with `isPointTargeted` (2269) does not call
//!   `dogmaIM.Activate`: `positionalControl._StartTargetedEffect` calls
//!   `superWeaponMgr.ActivateSinglePointTargetedModule(moduleID, effectID,
//!   destination)` and `_StartSlasherEffect` calls `ActivateSlashModule(
//!   moduleID, p0, p1)`. The legacy ones are ordinary targeted modules.
//! - **[V-data]** attributes (`dogma/const.py`): 2262 warning, 2263 damage
//!   radius, 2264 damage duration, 2265 damage cycle time (all ms), 2427
//!   no-jump-or-cloak duration, 2428 immobility duration, 2429 shape, 2430
//!   fixed range, 1654 `immuneToSuperWeapon`, 1839 `damageDelayDuration`,
//!   713/714 fuel type and quantity, 6 capacitor need, 73 the cycle (the
//!   reuse delay), 54 range. Effects 6472 and 4489-4492 carry no modifiers,
//!   so the holds are ours.
//! - Refusal keys [V-loc]: `CantActivateDoomsDayOnImmuneShips{moduleName,
//!   targetName}`, `CantActivateDoomsDayOnNonShip{module}`,
//!   `CannotActivateSuperWeaponOnNonCapitalShip{moduleTypeID}`.
//!
//! # What it does
//!
//! - **Lance** (`fire_lance`): fuel and capacitor are paid at the click, the
//!   cylinder is fixed there (origin = the titan, axis = toward `destination`,
//!   length = the fixed range 54 when 2430 is 1), the damage waits the
//!   warning (2262), then ticks every 2265 ms for 2264 ms. Each tick is a
//!   volley (114/116/117/118) at every ship whose centre is inside the
//!   cylinder, one `crimewatch::attack_refusal` per victim as the smartbomb
//!   does, immune ships (1654) and ships in warp spared.
//! - **Holds** (both kinds): the firer is immobile for 2428 (`WARP_GUARDS`
//!   row) and may not jump (`JUMP_GUARDS` row) or cloak ([`check_cloak`])
//!   for 2427.
//!
//! # Assumed, with a falsifier each
//!
//! - **[A]** damage ticks start when the warning ends (the spec's [E] hint
//!   agrees). Falsifier: TQ video timing.
//! - **[A]** `destination` marshals as a 3-tuple of floats. Falsifier: the
//!   first socket trace.
//! - **[I]** `destination` is an offset from the firer, not a world point: the
//!   client sends `_currentPath.GetEndPosition()` unconverted, an ego-centred
//!   render-space vector (`positionalControl.py:108-111`; the points that
//!   are server-bound go through `_ConvertPositionToGlobalSpace`, `:160-163`,
//!   `:192-206`). The aim is therefore used as is.
//! - **[I]** the cylinder starts at the hull surface (origin = centre plus
//!   axis times hull radius); its length is the fixed range when 2430 is 1
//!   and otherwise the smaller of the aim past the hull and the range
//!   (`controlPaths.py:244-250`).
//! - **[A]** the cylinder is tested on ship centres. Falsifier: damage
//!   outside the client's own indicator.
//! - **[A]** state is in memory only: a server restart releases every hold
//!   and drops a beam in flight. Falsifier: a TQ titan held across a
//!   downtime.
//! - **[A]** 2266 (`speedFactorFloor` -100) is not applied: "immobile" bans
//!   warp and docking here, not sub-warp speed. Falsifier: a TQ titan that
//!   is stopped dead for 30 s.
//! - **[V-data]** the Disruptive Lance (type effect 11691 `debuffLance`)
//!   carries attributes 5412 (debuff ms), 5425 (no cloak; passive 11714 sets
//!   2454) and 5426 (needs an active siege module); 5425/5426 are attributes,
//!   not effects. 9c: it fires as a Lance, needs a running siege-family
//!   module, and each victim hit gets a debuff for 5412 ms.
//! - **[A]** the debuff's effect: no warp, jump, dock or cloak for its
//!   duration (dbuff collections 4, 5, 6 are the warp, cloak and jump/dock
//!   penalties; the data names none for 5412). Not applied as a client
//!   `dbuffState`, so no buff-bar slot. Falsifier: the TQ buff bar.
//! - **[A]** the Bosonic Field Generator (6473, shape 3) is a cone from the
//!   firer's hull surface (as the client's `SinglePointPath` anchors it), fixed range, [`CONE_HALF_ANGLE_DEG`] half angle, ticking
//!   like a Lance; it applies no debuff here (the data names none).
//! - **[A]** the holds start at the click, not at the end of the warning.
//!   Falsifier: a TQ titan that may warp during the warning.
//! - **[A]** the Lance's reuse delay is its attribute 73 and the click is
//!   refused in the meantime; the legacy one rides the module cycle.
//! - **[A]** legacy damage lands after 1839 ms from the cycle start, at the
//!   target's then position (no re-check of range), and is refused on a
//!   non-capital target; a Lance hits anything by data.
//! - The beams' FX guids (`DoomsdayFx`): the effect rows carry none (6201,
//!   6472); `effects.fsdbinary` keys them by race, `effects.SWSlashBeam<Race>`
//!   (Reaper) and `effects.SWDamageBeam<Race>` (Lance) **[V]**, picked by the
//!   module's damage type ([`race_index`]). **[A]** the Reaper's rows are
//!   `SlashWeapon` (reads `graphicInfo.startTargetOffset`/`endTargetOffset`/
//!   `duration`) and the Lance's `DirectionalWeapon` (`targetOffset`/
//!   `duration`); the graphicInfo shape is [V] (`superWeapon.py:299-351`).
//!   Falsifier: the client log has an `AttributeError` on graphicInfo, or
//!   draws no beam.
//!   The FX starts when the warning ends and stops after the last tick. The
//!   legacy cycle's FX is the generic module one.
//! - **[I]** the Reaper's slice is the triangle from the firer's centre to
//!   its two arc ends (`ArcPath`, `SliceAreaIndication`), `damageRadius`
//!   (2263) either side of its plane (the indicator is scaled `2 * radius`
//!   thick); **[A]** that the slice is static (the client sweeps its beam
//!   from one end to the other over the burn, `sweepTime`) and that the
//!   thickness is centred. Falsifier: damage outside the indicator, or the
//!   sweep hitting a ship late.
//! - **[A]** an NPC ship is hit and its type read for 1654 from the roster.

use crate::damage::{BlueTime, Damage, Shot, attr};
use crate::modules::{MSEC, type_arg, type_arg_for, user_error};
use crate::movement_guard::Refusal;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Instant;

pub const ATTR_CAP_NEED: i64 = 6;
pub const ATTR_RANGE: i64 = 54;
pub const ATTR_CYCLE: i64 = 73;
pub const ATTR_IMMUNE: i64 = 1_654;
pub const ATTR_DAMAGE_DELAY: i64 = 1_839;
pub const ATTR_WARNING: i64 = 2_262;
pub const ATTR_RADIUS: i64 = 2_263;
pub const ATTR_DAMAGE_DURATION: i64 = 2_264;
pub const ATTR_DAMAGE_CYCLE: i64 = 2_265;
pub const ATTR_POINT_TARGETED: i64 = 2_269;
pub const ATTR_NO_JUMP: i64 = 2_427;
pub const ATTR_IMMOBILE: i64 = 2_428;
pub const ATTR_SHAPE: i64 = 2_429;
pub const ATTR_FIXED_RANGE: i64 = 2_430;

/// `doomsdayAppliedDBuffDuration` **[V-data]**: how long a Disruptive Lance's
/// debuff stays on a victim (ms). A type that carries it is a Disruptive one.
pub const ATTR_DEBUFF_DURATION: i64 = 5_412;

/// `activationRequiresActiveSiegeModule` **[V-data]**: the Disruptive Lance
/// fires only while a siege module of the firer runs. (5425 and 5426 are
/// attributes, not effects: the type's effect is 11691 `debuffLance`.)
pub const ATTR_NEEDS_SIEGE: i64 = 5_426;

/// `doomsdayAOEShape` 3: the cone (the Bosonic Field Generator, effect 6473).
pub const SHAPE_CONE: i64 = 3;

/// The cone's half angle in degrees **[A]**: the data carries no angle (the
/// client's `ConeAreaIndication` is a unit model scaled by the length alone,
/// `controlPaths.py:43-55`). Falsifier: damage outside the client's cone.
pub const CONE_HALF_ANGLE_DEG: f64 = 30.0;

/// `doomsdayAOEShape` 1: the cylinder.
pub const SHAPE_CYLINDER: i64 = 1;

/// `doomsdayAOEShape` 2: the slice (`positionalControl._SW_SLICE`).
pub const SHAPE_SLICE: i64 = 2;

/// How far past the range a slice's arc end may sit: sqrt(1 + tan(15 deg)^2),
/// rounded up (`controlPaths.ArcPath`) **[I]**.
pub const SLICE_REACH: f64 = 1.04;

/// `superWeaponAmarr` .. `superWeaponMinmatar` **[V-data]**.
pub const LEGACY_EFFECTS: [i64; 4] = [4_489, 4_490, 4_491, 4_492];

/// Whether `effect_id` is a legacy targeted doomsday's.
pub fn is_legacy(effect_id: i64) -> bool {
    LEGACY_EFFECTS.contains(&effect_id)
}

/// Where a beam hurts, fixed at the click.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Area {
    /// The Lance: from `origin` along unit `axis`; `aim` is what the client
    /// sent (an offset from the firer), kept for the FX.
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        length: f64,
        radius: f64,
        aim: [f64; 3],
    },
    /// The Bosonic cone: apex at the firer's hull surface, along unit `axis` for
    /// `length`, `cos_half` the cosine of its half angle.
    Cone {
        apex: [f64; 3],
        axis: [f64; 3],
        length: f64,
        cos_half: f64,
        aim: [f64; 3],
    },
    /// The Reaper: the triangle `apex`, `apex + p0`, `apex + p1`, `half`
    /// thick either side of its plane.
    Slice {
        apex: [f64; 3],
        p0: [f64; 3],
        p1: [f64; 3],
        half: f64,
    },
}

/// A beam (a Lance or a Reaper) in its warning or burning.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Beam {
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    system: i64,
    area: Area,
    damage: Damage,
    /// The next tick's time, and the ticks left.
    next: BlueTime,
    step: BlueTime,
    left: i64,
    /// The burn length in ms (the FX's `duration`) and whether it was sent.
    duration_ms: i64,
    announced: bool,
}

/// A legacy doomsday's strike, waiting out its delay.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Strike {
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    type_id: i64,
    target_ship_id: i64,
    at: BlueTime,
    damage: Damage,
}

/// What a firer may not do until when.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Hold {
    immobile_until: BlueTime,
    no_jump_until: BlueTime,
    /// A victim of a Disruptive Lance: no warp, jump, dock or cloak.
    disrupted_until: BlueTime,
}

#[derive(Default)]
pub struct State {
    beams: Vec<Beam>,
    strikes: Vec<Strike>,
    holds: BTreeMap<i64, Hold>,
    /// Module -> when a Lance may be fired again.
    ready: BTreeMap<i64, BlueTime>,
}

pub const SLOT: SlotSpec = SlotSpec {
    name: "superweapon",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

fn ms(v: f64) -> BlueTime {
    (v.max(0.0) as BlueTime).saturating_mul(MSEC)
}

fn own(w: &Registry, type_id: i64, a: i64) -> Option<f64> {
    crate::typedata::attribute_in(w.content(), type_id, a)
}

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

fn name_of(w: &Registry, type_id: i64) -> String {
    crate::item::find_in(w.content(), type_id)
        .map_or_else(|| format!("type {type_id}"), |t| t.name.to_owned())
}

// ---------------------------------------------------------------------------
// The holds
// ---------------------------------------------------------------------------

fn set_hold(w: &mut Registry, ship_id: i64, type_id: i64, now: BlueTime) {
    let immobile = own(w, type_id, ATTR_IMMOBILE).map_or(0, ms);
    let no_jump = own(w, type_id, ATTR_NO_JUMP).map_or(0, ms);
    let h = w.ext_mut::<State>().holds.entry(ship_id).or_default();
    h.immobile_until = h.immobile_until.max(now.saturating_add(immobile));
    h.no_jump_until = h.no_jump_until.max(now.saturating_add(no_jump));
}

/// Whether `ship` may not warp at `now` (it fired a doomsday, 2428).
pub fn immobile_at(w: &Registry, ship_id: i64, now: BlueTime) -> bool {
    w.ext::<State>()
        .holds
        .get(&ship_id)
        .is_some_and(|h| now < h.immobile_until)
}

/// Whether `ship` may not jump or cloak at `now` (2427).
pub fn no_jump_at(w: &Registry, ship_id: i64, now: BlueTime) -> bool {
    w.ext::<State>()
        .holds
        .get(&ship_id)
        .is_some_and(|h| now < h.no_jump_until)
}

/// Whether `ship` carries a Disruptive Lance's debuff at `now` **[A]**.
pub fn disrupted_at(w: &Registry, ship_id: i64, now: BlueTime) -> bool {
    w.ext::<State>()
        .holds
        .get(&ship_id)
        .is_some_and(|h| now < h.disrupted_until)
}

fn disrupted_refusal() -> Refusal {
    Refusal {
        user_error: "CustomNotify",
        args: vec![(
            crate::pyutil::bstr("notify"),
            crate::pyutil::bstr("A disruptive doomsday field holds your ship."),
        )],
    }
}

fn held_refusal() -> Refusal {
    Refusal {
        user_error: "CustomNotify",
        args: vec![(
            crate::pyutil::bstr("notify"),
            crate::pyutil::bstr("Your doomsday weapon is still discharging."),
        )],
    }
}

/// **The `WARP_GUARDS` row**: a ship that fired a doomsday does not warp
/// while it is immobile.
pub fn immobile_guard(w: &Registry, character_id: i64) -> Option<Refusal> {
    let ship = w.roster().get(character_id)?.ship_id;
    let now = crate::pyutil::filetime_now();
    if immobile_at(w, ship, now) {
        return Some(held_refusal());
    }
    disrupted_at(w, ship, now).then(disrupted_refusal)
}

/// **The `JUMP_GUARDS` row**: no jump (gate, bridge, cyno or wormhole) while
/// the no-jump hold runs.
pub fn jump_guard(w: &Registry, character_id: i64) -> Option<Refusal> {
    let ship = w.roster().get(character_id)?.ship_id;
    let now = crate::pyutil::filetime_now();
    if no_jump_at(w, ship, now) {
        return Some(held_refusal());
    }
    disrupted_at(w, ship, now).then(disrupted_refusal)
}

/// **The dock row** (`module_fuel::refuse_dock`, which both dock paths call):
/// a titan that fired a doomsday does not dock while it is immobile (the
/// types carry `disallowDocking` 2354 and `disallowTethering` 2343).
pub fn dock_refusal(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    let Some(ship) = actor.and_then(|a| w.roster().get(a)).map(|p| p.ship_id) else {
        return Ok(());
    };
    if immobile_at(w, ship, crate::pyutil::filetime_now()) {
        return Err(notify("You cannot dock while your doomsday discharges."));
    }
    if disrupted_at(w, ship, crate::pyutil::filetime_now()) {
        return Err(notify(
            "You cannot dock while a disruptive field holds you.",
        ));
    }
    Ok(())
}

/// **The cloak row** (`modules::activate`, beside the cloak's own checks).
pub fn check_cloak(w: &Registry, ship_id: i64, now: BlueTime) -> Result<(), ServiceError> {
    if no_jump_at(w, ship_id, now) {
        return Err(notify("You cannot cloak while your doomsday discharges."));
    }
    if disrupted_at(w, ship_id, now) {
        return Err(notify(
            "You cannot cloak while a disruptive field holds you.",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Who is hit
// ---------------------------------------------------------------------------

fn ship_type_of(w: &Registry, p: &crate::space_state::Presence) -> Option<i64> {
    w.item(p.ship_id)
        .map(|i| i.type_id)
        .or_else(|| w.npcs().get(p.character_id).map(|n| n.kind.type_id))
}

fn immune(w: &Registry, p: &crate::space_state::Presence) -> bool {
    ship_type_of(w, p).is_some_and(|t| own(w, t, ATTR_IMMUNE).is_some_and(|v| v > 0.0))
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Whether `p` is inside the cylinder from `origin` along unit `axis`,
/// `length` long, `radius` wide.
pub fn in_cylinder(
    origin: [f64; 3],
    axis: [f64; 3],
    length: f64,
    radius: f64,
    p: [f64; 3],
) -> bool {
    let d = sub(p, origin);
    let along = dot(d, axis);
    if !(0.0..=length).contains(&along) {
        return false;
    }
    let perp = sub(d, [axis[0] * along, axis[1] * along, axis[2] * along]);
    dot(perp, perp).sqrt() <= radius
}

/// A module attribute as the dogma sheet reads it (skills and rigs count),
/// else the type's own.
fn live(w: &Registry, ship_id: i64, module_id: i64, type_id: i64, a: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a).or_else(|| own(w, type_id, a))
}

fn volley(w: &Registry, ship_id: i64, module_id: i64, type_id: i64) -> Damage {
    let d = |a: i64| {
        live(w, ship_id, module_id, type_id, a)
            .unwrap_or(0.0)
            .max(0.0)
    };
    Damage {
        em: d(attr::EM_DAMAGE),
        explosive: d(attr::EXPLOSIVE_DAMAGE),
        kinetic: d(attr::KINETIC_DAMAGE),
        thermal: d(attr::THERMAL_DAMAGE),
    }
}

fn hit(
    w: &mut Registry,
    c: (i64, i64, i64, i64),
    victim: crate::space_state::Presence,
    damage: Damage,
    at: BlueTime,
) -> bool {
    let (character_id, ship_id, module_id, type_id) = c;
    // A protected ship takes no damage (combat drops it), so it takes no
    // debuff either: a Disruptive Lance down an undock must not pin anyone.
    if crate::invuln::ship_protected(w, victim.ship_id, at) {
        return false;
    }
    if crate::crimewatch::attack_refusal(w, character_id, Some(type_id), victim.ship_id, at)
        .is_some()
    {
        return false;
    }
    let shot = Shot {
        at,
        attacker_ship_id: ship_id,
        weapon_item_id: module_id,
        weapon_type_id: type_id,
        target_ship_id: victim.ship_id,
        damage,
        hit_quality: crate::combat::HIT_QUALITY,
    };
    crate::combat::fire(w, shot, character_id, victim, at);
    true
}

// ---------------------------------------------------------------------------
// The Lance
// ---------------------------------------------------------------------------

/// The cylinder's length: the fixed range when `fixed`, else the aim's
/// distance past the hull surface, capped by the range **[I]**.
pub fn cylinder_length(aim_len: f64, hull_radius: f64, range: Option<f64>, fixed: bool) -> f64 {
    let to_aim = (aim_len - hull_radius).max(0.0);
    match (fixed, range) {
        (true, Some(r)) => r,
        (false, Some(r)) => to_aim.min(r),
        (_, None) => to_aim,
    }
}

/// Whether a siege-family module of `ship_id` is running (not ending) **[A]**
/// (`CannotActivateModule_SiegeModuleNotActive`, the client's key). Falsifier:
/// a TQ Disruptive Lance that fires with its siege module cycling out.
fn siege_runs(w: &Registry, ship_id: i64, now: BlueTime) -> bool {
    w.modules().running_all().iter().any(|r| {
        r.ship_id == ship_id
            && crate::module_fuel::is_siege_like(r.effect.effect_id)
            && r.stop_at.is_none_or(|t| t > now)
    })
}

/// What the two fire calls share once the gates have passed.
struct Prep {
    character_id: i64,
    here: crate::space_state::Presence,
    type_id: i64,
    ship_type: i64,
    hull_radius: f64,
}

/// The gates of a click on a doomsday (`modules::activate`'s refusals, as
/// the Lance has always had them): a fitted, online module of `shape`, in
/// space and not in warp, not in the empire, not cooling down.
fn prepare(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    shape: i64,
    now: BlueTime,
) -> Result<Prep, ServiceError> {
    let Some(character_id) = actor else {
        return Err(notify("No character is selected."));
    };
    let Some(item) = w.item(module_id).cloned() else {
        return Err(notify("This server does not know that module."));
    };
    w.advance_character(character_id);
    let Some(here) = w.roster().get(character_id).copied() else {
        return Err(notify("Modules can only be activated in space."));
    };
    if item.location_id != here.ship_id
        || item.owner_id != character_id
        || !crate::item::is_ship_fitting_flag(item.flag_id)
    {
        return Err(notify("That module is not fitted to your ship."));
    }
    let type_id = item.type_id;
    if own(w, type_id, ATTR_POINT_TARGETED).is_none_or(|v| v < 1.0) {
        return Err(user_error(
            "CantActivateDoomsDayOnNonShip",
            vec![(
                "module",
                type_arg_for("CantActivateDoomsDayOnNonShip", "module", type_id),
            )],
        ));
    }
    if own(w, type_id, ATTR_SHAPE).map(|v| v.round() as i64) != Some(shape) {
        return Err(notify("This server cannot fire that doomsday yet."));
    }
    if crate::bubble::forbidden_here(w, type_id, here.solar_system_id) {
        return Err(notify("That doomsday cannot be fired in empire space."));
    }
    if !w.modules().is_online(module_id) {
        return Err(user_error(
            "ModuleNotOnline",
            vec![("module", type_arg_for("ModuleNotOnline", "module", type_id))],
        ));
    }
    if here.motion.warp_active() {
        return Err(notify("You cannot fire a doomsday weapon in warp."));
    }
    if w.ext::<State>()
        .ready
        .get(&module_id)
        .is_some_and(|t| now < *t)
    {
        return Err(user_error(
            "EffectAlreadyActive2",
            vec![("modulename", type_arg(type_id))],
        ));
    }
    if own(w, type_id, ATTR_NEEDS_SIEGE).is_some_and(|v| v >= 1.0)
        && !siege_runs(w, here.ship_id, now)
    {
        return Err(notify("This weapon needs your siege module to be active."));
    }
    crate::cloak::check_other_activation(w, here.ship_id, 0, type_id)?;
    let ship_type = w.character(character_id).map_or(0, |c| c.ship_type_id);
    let hull_radius = crate::typedata::hull_in(w.content(), ship_type)
        .or_else(|| crate::destiny::hull::find_in(w.content(), ship_type))
        .map_or(0.0, |h| f64::from(h.radius));
    Ok(Prep {
        character_id,
        here,
        type_id,
        ship_type,
        hull_radius,
    })
}

/// Pays the fuel (it names itself when short) and the capacitor, drops the
/// undock protection and the gate cloak. Nothing is taken on a refusal.
fn pay(w: &mut Registry, p: &Prep, module_id: i64, now: BlueTime) -> Result<(), ServiceError> {
    let (ship, ty) = (p.here.ship_id, p.type_id);
    crate::module_fuel::check(w, p.character_id, ship, module_id, 0, ty)?;
    let need = live(w, ship, module_id, ty, ATTR_CAP_NEED)
        .unwrap_or(0.0)
        .max(0.0);
    let got = crate::modules::capacitor_at(w, ship, p.ship_type, now).0;
    if got < need {
        return Err(crate::modules::energy_error(w, ty, need, got));
    }
    if crate::module_fuel::burn(w, p.character_id, ship, module_id, 0, ty).is_err() {
        return Err(notify("The doomsday's fuel could not be taken."));
    }
    if let Some(m) = crate::modules::drain_capacitor_by(w, ship, need, now)
        && let Some(push) = m.push
    {
        w.push(push);
    }
    // The offensive module gives up the undock protection and the gate cloak,
    // as `modules::activate` does after its refusals.
    crate::invuln::break_for(w, p.character_id);
    crate::cloak::break_for_module(w, p.character_id, ty);
    Ok(())
}

/// Books the beam and the holds after [`pay`].
fn book(
    w: &mut Registry,
    p: &Prep,
    module_id: i64,
    area: Area,
    now: BlueTime,
) -> Result<(), ServiceError> {
    let ty = p.type_id;
    let step = ms(own(w, ty, ATTR_DAMAGE_CYCLE).unwrap_or(1000.0)).max(MSEC);
    let duration = ms(own(w, ty, ATTR_DAMAGE_DURATION).unwrap_or(0.0));
    let warning = ms(own(w, ty, ATTR_WARNING).unwrap_or(0.0));
    let beam = Beam {
        character_id: p.character_id,
        ship_id: p.here.ship_id,
        module_id,
        type_id: ty,
        system: p.here.solar_system_id,
        area,
        damage: volley(w, p.here.ship_id, module_id, ty),
        next: now.saturating_add(warning),
        step,
        left: (duration / step).max(1),
        duration_ms: duration / MSEC,
        announced: false,
    };
    let cooldown = ms(own(w, ty, ATTR_CYCLE).unwrap_or(0.0));
    set_hold(w, p.here.ship_id, ty, now);
    let s = w.ext_mut::<State>();
    s.ready.insert(module_id, now.saturating_add(cooldown));
    s.beams.push(beam);
    Ok(())
}

/// **Fire a Lance** (`superWeaponMgr.ActivateSinglePointTargetedModule`).
/// Checks, then pays the fuel and the capacitor, fixes the cylinder and the
/// holds. The damage comes later, from [`tick`].
pub fn fire_lance(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    destination: [f64; 3],
    now: BlueTime,
) -> Result<(), ServiceError> {
    // The one point-targeted call serves the cylinder (Lance, Disruptive
    // Lance) and the cone (Bosonic); the type's own shape says which.
    let shape = actor
        .and_then(|_| w.item(module_id).map(|i| i.type_id))
        .and_then(|t| own(w, t, ATTR_SHAPE))
        .map_or(SHAPE_CYLINDER, |v| v.round() as i64);
    let shape = if shape == SHAPE_CONE {
        SHAPE_CONE
    } else {
        SHAPE_CYLINDER
    };
    let p = prepare(w, actor, module_id, shape, now)?;
    let type_id = p.type_id;
    // The client's destination is an offset from the firer (module header).
    let aim = destination;
    let len = dot(aim, aim).sqrt();
    if !len.is_finite() || len <= 0.0 {
        return Err(notify("The aim point is on your ship."));
    }
    let axis = [aim[0] / len, aim[1] / len, aim[2] / len];
    let max = live(w, p.here.ship_id, module_id, type_id, ATTR_RANGE).filter(|r| *r > 0.0);
    let fixed = own(w, type_id, ATTR_FIXED_RANGE).is_some_and(|v| v >= 1.0);
    let length = cylinder_length(len, p.hull_radius, max, fixed);
    let pos = p.here.motion.position;
    let origin = [
        pos[0] + axis[0] * p.hull_radius,
        pos[1] + axis[1] * p.hull_radius,
        pos[2] + axis[2] * p.hull_radius,
    ];
    pay(w, &p, module_id, now)?;
    if shape == SHAPE_CONE {
        let cos_half = CONE_HALF_ANGLE_DEG.to_radians().cos();
        return book(
            w,
            &p,
            module_id,
            Area::Cone {
                apex: origin,
                axis,
                length,
                cos_half,
                aim,
            },
            now,
        );
    }
    let radius = own(w, type_id, ATTR_RADIUS).unwrap_or(0.0).max(0.0);
    book(
        w,
        &p,
        module_id,
        Area::Cylinder {
            origin,
            axis,
            length,
            radius,
            aim,
        },
        now,
    )
}

/// Whether `q` is inside the cone from `apex` along unit `axis` for `length`
/// with half angle cosine `cos_half` **[A]**.
pub fn in_cone(apex: [f64; 3], axis: [f64; 3], length: f64, cos_half: f64, q: [f64; 3]) -> bool {
    let d = sub(q, apex);
    let dist = dot(d, d).sqrt();
    if !dist.is_finite() {
        return false;
    }
    if dist == 0.0 {
        return true;
    }
    let along = dot(d, axis);
    along <= length && along / dist >= cos_half
}

/// Whether `q` is inside the slice: the triangle from `apex` to `apex + p0`
/// and `apex + p1`, `half` thick either side of its plane **[A]**.
pub fn in_slice(apex: [f64; 3], p0: [f64; 3], p1: [f64; 3], half: f64, q: [f64; 3]) -> bool {
    let n = cross(p0, p1);
    let nl = dot(n, n).sqrt();
    if !nl.is_finite() || nl <= 0.0 {
        return false;
    }
    let n = [n[0] / nl, n[1] / nl, n[2] / nl];
    let d = sub(q, apex);
    let off = dot(d, n);
    if off.abs() > half {
        return false;
    }
    // In the plane: d = a * p0 + b * p1 with a, b >= 0 and a + b <= 1.
    let inplane = sub(d, [n[0] * off, n[1] * off, n[2] * off]);
    let (a00, a01, a11) = (dot(p0, p0), dot(p0, p1), dot(p1, p1));
    let (b0, b1) = (dot(inplane, p0), dot(inplane, p1));
    let det = a00 * a11 - a01 * a01;
    if det <= 0.0 {
        return false;
    }
    let a = (b0 * a11 - b1 * a01) / det;
    let b = (b1 * a00 - b0 * a01) / det;
    a >= 0.0 && b >= 0.0 && a + b <= 1.0
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// **Fire a Reaper** (`superWeaponMgr.ActivateSlashModule(moduleID, p0, p1)`):
/// the same gates, fuel, capacitor, warning, ticks and holds as the Lance,
/// over the slice the client's `ArcPath` drew. `p0` and `p1` are offsets from
/// the firer, like the Lance's aim (`controlPaths.ArcPath.GetPath` returns
/// the two destination functions' values, ego-centred like `GetEndPosition`).
pub fn fire_slash(
    w: &mut Registry,
    actor: Option<i64>,
    module_id: i64,
    p0: [f64; 3],
    p1: [f64; 3],
    now: BlueTime,
) -> Result<(), ServiceError> {
    let p = prepare(w, actor, module_id, SHAPE_SLICE, now)?;
    let type_id = p.type_id;
    let (l0, l1) = (dot(p0, p0).sqrt(), dot(p1, p1).sqrt());
    let n = cross(p0, p1);
    if ![l0, l1].iter().all(|l| l.is_finite() && *l > 0.0) || dot(n, n) <= 0.0 {
        return Err(notify("The slash points do not make a slice."));
    }
    // The arc's ends sit at tan(15 degrees) either side of a point no farther
    // than the range (`ArcPath.__init__`, `UpdatePosition`): at most 1.035 of
    // it. A wider slice than that is not one the client could draw **[I]**.
    if let Some(r) = live(w, p.here.ship_id, module_id, type_id, ATTR_RANGE).filter(|r| *r > 0.0)
        && l0.max(l1) > r * SLICE_REACH
    {
        return Err(notify("The slash is beyond the weapon's range."));
    }
    pay(w, &p, module_id, now)?;
    let half = own(w, type_id, ATTR_RADIUS).unwrap_or(0.0).max(0.0);
    book(
        w,
        &p,
        module_id,
        Area::Slice {
            apex: p.here.motion.position,
            p0,
            p1,
            half,
        },
        now,
    )
}

// ---------------------------------------------------------------------------
// What the client draws
// ---------------------------------------------------------------------------

/// **`OnSpecialFX` for a doomsday beam**, fourteen positionals with a
/// `graphicInfo` KeyVal: `SlashWeapon` reads `.startTargetOffset`,
/// `.endTargetOffset` and `.duration` (ms); `DirectionalWeapon` reads
/// `.targetOffset` and `.duration` (`superWeapon.py:299-351`, `trigger` is a
/// `KeyVal`, so the attributes must be a KeyVal's) **[V]**.
#[derive(Debug, Clone, PartialEq)]
pub struct DoomsdayFx {
    pub to: Vec<i64>,
    pub ship_id: i64,
    pub module_id: i64,
    pub type_id: i64,
    pub guid: &'static str,
    pub start: bool,
    pub duration_ms: i64,
    pub offsets: Vec<(&'static str, [f64; 3])>,
}

/// The guids the Reaper's FX is started under, by race: Amarr, Caldari,
/// Gallente, Minmatar. `res:/staticdata/effects.fsdbinary` (build 3662168,
/// the client's `fsdBuiltData.client.effects`, which `fxSequencer.py:228`
/// looks a guid up in) carries exactly these four `SWSlashBeam<Race>` and
/// four `SWDamageBeam<Race>` keys and **no** bare `SWSlashBeam` /
/// `SWDamageBeam` **[V]** (the bare one raised `Activation::Unable to load
/// effect for guid` in fci_doomsday, regress-w23a). **[A]** the Slash rows
/// are `SlashWeapon` and the DamageBeam rows `DirectionalWeapon`: the
/// blob's string table names `DirectionalWeapon` beside `SWDamageBeamCaldari`
/// and `SlashWeapon` beside `SlashStretch`, but the class strings are shared,
/// so the other rows were not decoded. Falsifier: an `AttributeError` on
/// `graphicInfo.targetOffset` or `startTargetOffset` in the client log.
pub const GUIDS_SLASH: [&str; 4] = [
    "effects.SWSlashBeamAmarr",
    "effects.SWSlashBeamCaldari",
    "effects.SWSlashBeamGallente",
    "effects.SWSlashBeamMinmatar",
];
/// The Lance's, same order and the same [V] / [A].
pub const GUIDS_BEAM: [&str; 4] = [
    "effects.SWDamageBeamAmarr",
    "effects.SWDamageBeamCaldari",
    "effects.SWDamageBeamGallente",
    "effects.SWDamageBeamMinmatar",
];

/// The race index into [`GUIDS_SLASH`] / [`GUIDS_BEAM`] of a doomsday that
/// deals `d`: its largest damage type. Each racial Lance and Reaper deals one
/// type, the race's own (EM Amarr, kinetic Caldari, thermal Gallente,
/// explosive Minmatar) [V-data: the eight modules' `race_id` in
/// `types/market.tsv` against their names]. A tie or no damage reads Amarr
/// **[A]** (the Bosonic Field Generator has no race); falsifier: a cone FX
/// drawn in the wrong colour.
pub fn race_index(d: &Damage) -> usize {
    let by = [d.em, d.kinetic, d.thermal, d.explosive];
    let mut best = 0;
    for (i, v) in by.iter().enumerate() {
        if *v > by[best] {
            best = i;
        }
    }
    best
}

impl crate::notify::Push for DoomsdayFx {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let mut info: Vec<(&str, PyValue)> = self
            .offsets
            .iter()
            .map(|(k, v)| {
                (
                    *k,
                    PyValue::Tuple(v.iter().map(|x| PyValue::Real(*x)).collect()),
                )
            })
            .collect();
        info.push(("duration", PyValue::Int(self.duration_ms)));
        let flag = PyValue::Int(i64::from(self.start));
        (
            "OnSpecialFX",
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Int(self.module_id),
                PyValue::Int(self.type_id),
                PyValue::None,
                PyValue::None,
                crate::pyutil::bstr(self.guid),
                PyValue::Int(1),
                flag.clone(),
                flag,
                PyValue::Int(self.duration_ms),
                PyValue::None,
                PyValue::None,
                PyValue::Int(0),
                crate::pyutil::keyval(info),
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

fn announce(w: &mut Registry, b: &Beam, start: bool) {
    let mut to = vec![b.character_id];
    to.extend(w.roster().others_in_system(b.system, b.character_id));
    let (guid, offsets) = match b.area {
        Area::Cylinder { aim, .. } | Area::Cone { aim, .. } => (
            GUIDS_BEAM[race_index(&b.damage)],
            vec![("targetOffset", aim)],
        ),
        Area::Slice { p0, p1, .. } => (
            GUIDS_SLASH[race_index(&b.damage)],
            vec![("startTargetOffset", p0), ("endTargetOffset", p1)],
        ),
    };
    w.push(DoomsdayFx {
        to,
        ship_id: b.ship_id,
        module_id: b.module_id,
        type_id: b.type_id,
        guid,
        start,
        duration_ms: b.duration_ms,
        offsets,
    });
}

fn beam_tick(w: &mut Registry, b: &Beam) {
    let c = (b.character_id, b.ship_id, b.module_id, b.type_id);
    let victims: Vec<_> = w
        .roster()
        .in_system(b.system)
        .into_iter()
        .filter(|p| p.ship_id != b.ship_id && !p.motion.warp_active())
        .filter(|p| !crate::cloak::is_cloaked(w, p.ship_id))
        .filter(|p| match b.area {
            Area::Cylinder {
                origin,
                axis,
                length,
                radius,
                ..
            } => in_cylinder(origin, axis, length, radius, p.motion.position),
            Area::Cone {
                apex,
                axis,
                length,
                cos_half,
                ..
            } => in_cone(apex, axis, length, cos_half, p.motion.position),
            Area::Slice { apex, p0, p1, half } => in_slice(apex, p0, p1, half, p.motion.position),
        })
        .filter(|p| !immune(w, p))
        .collect();
    // A Disruptive Lance's debuff (5412, ms) rides each hit that lands **[A]**.
    let debuff = own(w, b.type_id, ATTR_DEBUFF_DURATION).map_or(0, ms);
    for v in victims {
        if hit(w, c, v, b.damage, b.next) && debuff > 0 {
            let h = w.ext_mut::<State>().holds.entry(v.ship_id).or_default();
            h.disrupted_until = h.disrupted_until.max(b.next.saturating_add(debuff));
        }
    }
}

// ---------------------------------------------------------------------------
// The legacy targeted doomsday
// ---------------------------------------------------------------------------

/// **The activation refusal of a legacy doomsday** (`modules::activate`): a
/// target that is not a ship in space, not a capital, or immune.
pub fn legacy_refusal(
    w: &Registry,
    type_id: i64,
    effect_id: i64,
    target: Option<i64>,
) -> Option<Refusal> {
    if !is_legacy(effect_id) {
        return None;
    }
    let module = type_arg_for("CantActivateDoomsDayOnNonShip", "module", type_id);
    let p = target.and_then(|t| w.roster().by_ship(t)).copied();
    let Some(p) = p else {
        return Some(Refusal {
            user_error: "CantActivateDoomsDayOnNonShip",
            args: crate::pyutil::pypairs(vec![("module", module)]),
        });
    };
    let target_type = ship_type_of(w, &p).unwrap_or(0);
    if immune(w, &p) {
        return Some(Refusal {
            user_error: "CantActivateDoomsDayOnImmuneShips",
            args: crate::pyutil::pypairs(vec![
                ("moduleName", crate::pyutil::bstr(&name_of(w, type_id))),
                ("targetName", crate::pyutil::bstr(&name_of(w, target_type))),
            ]),
        });
    }
    if !crate::typedata::is_capital_hull_in(w.content(), target_type) {
        return Some(Refusal {
            user_error: "CannotActivateSuperWeaponOnNonCapitalShip",
            args: crate::pyutil::pypairs(vec![(
                "moduleTypeID",
                type_arg_for(
                    "CannotActivateSuperWeaponOnNonCapitalShip",
                    "moduleTypeID",
                    type_id,
                ),
            )]),
        });
    }
    None
}

/// **A legacy doomsday's cycle**: the firer is held at once, the strike
/// lands after the delay (1839).
pub fn on_legacy_cycle(w: &mut Registry, c: &crate::modules::Cycle, now: BlueTime) {
    let Some(target) = c.target_id.and_then(|t| w.roster().by_ship(t)).copied() else {
        return;
    };
    set_hold(w, c.ship_id, c.type_id, now);
    let strike = Strike {
        character_id: c.character_id,
        ship_id: c.ship_id,
        module_id: c.module_id,
        type_id: c.type_id,
        target_ship_id: target.ship_id,
        at: c
            .at
            .saturating_add(ms(own(w, c.type_id, ATTR_DAMAGE_DELAY).unwrap_or(0.0))),
        damage: volley(w, c.ship_id, c.module_id, c.type_id),
    };
    w.ext_mut::<State>().strikes.push(strike);
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **The tick stage**: Lance ticks and legacy strikes that are due, then the
/// holds that ran out are forgotten.
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    {
        let s = w.ext::<State>();
        if s.beams.is_empty() && s.strikes.is_empty() && s.holds.is_empty() && s.ready.is_empty() {
            return;
        }
    }
    let mut beams = std::mem::take(&mut w.ext_mut::<State>().beams);
    for b in &mut beams {
        // A firer who left space ends his beam.
        if w.roster().by_ship(b.ship_id).is_none() {
            b.left = 0;
            continue;
        }
        if !b.announced && b.next <= now {
            b.announced = true;
            announce(w, b, true);
        }
        while b.left > 0 && b.next <= now {
            beam_tick(w, b);
            b.left -= 1;
            b.next = b.next.saturating_add(b.step);
        }
        if b.left == 0 && b.announced {
            announce(w, b, false);
        }
    }
    beams.retain(|b| b.left > 0);
    let s = w.ext_mut::<State>();
    beams.append(&mut s.beams);
    s.beams = beams;
    let due: Vec<Strike> = {
        let s = w.ext_mut::<State>();
        let (due, rest): (Vec<_>, Vec<_>) = s.strikes.drain(..).partition(|k| k.at <= now);
        s.strikes = rest;
        due
    };
    for k in due {
        let Some(v) = w
            .roster()
            .by_ship(k.target_ship_id)
            .copied()
            .filter(|p| !p.motion.warp_active())
        else {
            continue;
        };
        if immune(w, &v) {
            continue;
        }
        hit(
            w,
            (k.character_id, k.ship_id, k.module_id, k.type_id),
            v,
            k.damage,
            k.at,
        );
    }
    let s = w.ext_mut::<State>();
    s.holds
        .retain(|_, h| now < h.immobile_until || now < h.no_jump_until || now < h.disrupted_until);
    s.ready.retain(|_, t| now < *t);
}

#[cfg(test)]
pub(crate) mod tests;
