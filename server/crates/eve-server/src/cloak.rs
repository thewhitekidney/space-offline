//! **Cloaking modules** (lane `cloak-1`, issue #21; research `r-pvp-spec` §1).
//!
//! A cloaking device is a module whose effect has **no cycle**: effects 607
//! `cloaking`, 980 `cloakingWarpSafe` and 5945 `cloakingPrototype` carry
//! `durationAttributeID` 0 **[V data]**. `modules::activate` gives such a
//! module a [`CLOAK_PERIOD`] of about three years and tells the client a
//! duration of `-1`, which `shipmodulebutton.DoActivateRampsThread` takes as
//! "draw no ramp" **[V]**; the module ends when it is deactivated, not at a
//! cycle boundary ([`uncloak`]).
//!
//! ## What the client does itself **[V]**
//!
//! - `michelle.RealFlushState`: a `CloakBall` for a ball other than the ego
//!   makes the client `RemoveBall` it locally. So an observer loses the ball
//!   and this server must model that ([`crate::grid::GridRole::Hides`]) and
//!   must not re-add it ([`hidden_from`]).
//! - The ego's own model is cloaked by the client when the ball record's
//!   cloak byte (offset 0x33) is non-zero (`spaceObject.Display` plays
//!   `effects.CloakNoAmim`); `destiny::Ball::cloak` carries it.
//! - A follow, orbit or missile aimed at a cloaked ball is cancelled by the
//!   client's native; the server agrees in [`on_activated`].
//! - `cloakingSvc.OnCloakModuleForceDeactivated(key, args)` plays the forced
//!   decloak sound and shows the dialog.
//!
//! ## What is assumed **[A]**
//!
//! Every number or rule below that is not marked **[V]**; the falsifier is
//! at each. The Destiny mode values are in one place, [`MODE_NORMAL`] and
//! friends; `fci_cloak` step 0 reads the client's `DST*CLOAK` constants and
//! fails if they differ.

use crate::damage::BlueTime;
use crate::modules::{Running, type_arg, user_error};
use crate::movement_guard::Refusal;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// `destiny.DSTNORMALCLOAK`. **[A]** 1: the constant is exported by
/// `_destiny.dll` and appears nowhere in the Python tree; the DLL ignores a
/// mode of 0 or less ("CloakBall called with mode <= 0. Ignored.").
/// Falsifier: `fci_cloak` step 0 reads `destiny.DSTNORMALCLOAK`.
pub const MODE_NORMAL: u8 = 1;
/// `destiny.DSTRESTORECLOAK`, the gate cloak's mode (CLOAK-2). **[A]** 2.
pub const MODE_RESTORE: u8 = 2;
/// `destiny.DSTGMCLOAK`. **[A]** 3. Nothing here uses it.
pub const MODE_GM: u8 = 3;

/// `appConst.minCloakingDistance`, metres **[V]**: no cloak within this of
/// another object, surface to surface **[A]** (the client's constant does not
/// say what the distance is measured between).
pub const MIN_CLOAKING_DISTANCE: f64 = 2000.0;

/// `effectCloaking` / `effectCloakingWarpSafe` / `effectCloakingPrototype`
/// **[V]** (`dogma/const.py`).
pub const EFFECT_CLOAKING: i64 = 607;
pub const EFFECT_CLOAKING_WARP_SAFE: i64 = 980;
pub const EFFECT_CLOAKING_PROTOTYPE: i64 = 5945;

/// `cloakingTargetingDelay`, ms **[V data]**: how long after a decloak the
/// ship may not lock.
const ATTR_TARGETING_DELAY: i64 = 560;
/// `disallowEarlyDeactivation` **[V]**: a module that keeps the cloak out.
const ATTR_DISALLOW_EARLY_DEACTIVATION: i64 = 906;
/// The hull's `canCloak` **[V]**.
const ATTR_CAN_CLOAK: i64 = 1163;
/// The ship's `disallowCloaking` **[V]**.
const ATTR_DISALLOW_CLOAKING: i64 = 2454;

/// How long a gate cloak lasts, blue ticks. **[A]** 60 s: no client constant
/// exists (a whole-tree grep for jumpCloak finds none); EVE knowledge and
/// EvEJS. Falsifier: a TQ `JumpCloak` timer of another length.
pub const JUMP_CLOAK_DURATION: i64 = 60 * 10_000_000;

/// `attributeCanActivateInGateCloak` **[V]** (`dogma/const.py`): a module
/// with this set to 1 (a Warp Core Stabilizer) runs without breaking the
/// gate cloak.
const ATTR_CAN_ACTIVATE_IN_GATE_CLOAK: i64 = 3123;

/// The cycle a cloak is given, blue ticks: 10^15, about three years. The
/// module is ended by [`uncloak`], never by reaching this.
pub const CLOAK_PERIOD: i64 = 1_000_000_000_000_000;

const MSEC: i64 = crate::modules::MSEC;

/// Whether `effect_id` is one of the three cloaking effects.
pub fn is_cloak_effect(effect_id: i64) -> bool {
    matches!(
        effect_id,
        EFFECT_CLOAKING | EFFECT_CLOAKING_WARP_SAFE | EFFECT_CLOAKING_PROTOTYPE
    )
}

/// How the ship came to be cloaked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A cloaking device. `warp_safe` is the effect's `isWarpSafe` (980).
    Device { module_id: i64, warp_safe: bool },
    /// The gate cloak of an arrival (CLOAK-2): ends at `until`, or when the
    /// pilot moves, locks or activates a module. No recalibration, no
    /// proximity break, and none of the device's refusals.
    Jump { until: BlueTime },
}

/// One cloaked ship.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cloak {
    pub character: i64,
    pub ship_id: i64,
    /// The Destiny mode the ball record carries.
    pub mode: u8,
    pub kind: Kind,
}

/// The world slot: who is cloaked, and who may not lock yet.
#[derive(Debug, Default)]
pub struct State {
    by_ship: BTreeMap<i64, Cloak>,
    /// Ship -> when it may lock again (`cloakingTargetingDelay` after a decloak).
    recal_until: BTreeMap<i64, BlueTime>,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "cloak",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

/// Is `ship` cloaked? (An empty-table fast path: the grid calls this per ball.)
pub fn is_cloaked(w: &Registry, ship: i64) -> bool {
    let s = w.ext::<State>();
    !s.by_ship.is_empty() && s.by_ship.contains_key(&ship)
}

/// The cloak byte of `ship`'s ball: `0` when it is not cloaked.
pub fn mode_of(w: &Registry, ship: i64) -> u8 {
    let s = w.ext::<State>();
    if s.by_ship.is_empty() {
        return 0;
    }
    s.by_ship.get(&ship).map_or(0, |c| c.mode)
}

/// Does `viewer_ship` not get to hold `ball`? A cloaked ball is hidden from
/// every client but its own pilot's.
pub fn hidden_from(w: &Registry, ball: i64, viewer_ship: i64) -> bool {
    ball != viewer_ship && is_cloaked(w, ball)
}

/// Every cloaked ship id (for the reconcile's per-viewer diff).
pub fn hidden_ids(w: &Registry) -> BTreeSet<i64> {
    w.ext::<State>().by_ship.keys().copied().collect()
}

/// Is `ship` still inside its post-cloak recalibration at `now`?
pub fn recalibrating(w: &Registry, ship: i64, now: BlueTime) -> bool {
    let s = w.ext::<State>();
    !s.recal_until.is_empty() && s.recal_until.get(&ship).is_some_and(|until| now < *until)
}

/// Why a lock from `locker_ship` to `target_ship` is refused, as the client's
/// `UserError` key; `None` when the cloak has nothing against it.
pub fn lock_refusal(
    w: &Registry,
    locker_ship: i64,
    target_ship: i64,
    now: BlueTime,
) -> Option<&'static str> {
    if is_cloaked(w, locker_ship) {
        return Some("DeniedTargetingCloaked");
    }
    if is_cloaked(w, target_ship) {
        return Some("DeniedTargetingTargetCloaked");
    }
    recalibrating(w, locker_ship, now).then_some("DeniedTargetAfterCloak")
}

/// A module other than the cloak, activated while cloaked, is refused
/// (`DeniedActivateCloaked`). **[A]** a refusal rather than a decloak, and
/// that a probe launcher is allowed; falsifier: a TQ covert-ops pilot who
/// cannot launch probes cloaked, or one whose weapon activation decloaks.
pub fn check_other_activation(
    w: &Registry,
    ship_id: i64,
    effect_id: i64,
    type_id: i64,
) -> Result<(), ServiceError> {
    if is_cloak_effect(effect_id)
        || !device_cloaked(w, ship_id)
        || crate::scan::is_probe_launcher_in(w.content(), type_id)
    {
        return Ok(());
    }
    Err(user_error("DeniedActivateCloaked", vec![]))
}

/// A `WARP_GUARDS` row: a cloak that is not warp-safe (607, 5945) does not
/// warp (`CantWarpWhileCloaked`); the covert-ops cloak (980) does.
pub fn warp_guard(w: &Registry, character_id: i64) -> Option<Refusal> {
    let c = of_character(w, character_id)?;
    match c.kind {
        Kind::Device {
            warp_safe: false, ..
        } => Some(Refusal {
            user_error: "CantWarpWhileCloaked",
            args: Vec::new(),
        }),
        _ => None,
    }
}

/// A `JUMP_GUARDS` row: no jump while cloaked. A wormhole has its own text.
pub fn jump_guard(w: &Registry, character_id: i64) -> Option<Refusal> {
    of_character(w, character_id)?;
    Some(Refusal {
        user_error: if crate::wormhole_jump::entering(w, character_id) {
            "CantEnterWormholeWhileCloaked"
        } else {
            "CantJumpWhileCloaked"
        },
        args: Vec::new(),
    })
}

/// Dock and safe logoff: the pilot is cloaked.
pub fn refuse_dock(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    match actor.and_then(|a| of_character(w, a)) {
        Some(_) => Err(user_error("CantDockWhileCloaked", vec![])),
        None => Ok(()),
    }
}

/// Is this character's ship cloaked?
pub fn character_cloaked(w: &Registry, character_id: i64) -> bool {
    of_character(w, character_id).is_some()
}

fn of_character(w: &Registry, character_id: i64) -> Option<Cloak> {
    let s = w.ext::<State>();
    if s.by_ship.is_empty() {
        return None;
    }
    let ship = w.roster().get(character_id)?.ship_id;
    s.by_ship
        .get(&ship)
        .copied()
        .filter(|c| matches!(c.kind, Kind::Device { .. }))
}

/// Does `ship` run a cloaking *device* (not just a gate cloak)?
fn device_cloaked(w: &Registry, ship: i64) -> bool {
    let s = w.ext::<State>();
    s.by_ship
        .get(&ship)
        .is_some_and(|c| matches!(c.kind, Kind::Device { .. }))
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// The nearest object within [`MIN_CLOAKING_DISTANCE`] of `ship`, surface to
/// surface: `(distance, type id)`. **[A]** that asteroids, stations, gates,
/// planets and moons, other ships and a site's objects count, and that this
/// ship's own drones, other cloaked ships, wrecks, cynosural fields and the
/// belt's beacon do not. Falsifier: TQ cloaks at 1.5 km from a rock's
/// surface, or refuses next to a wreck.
fn nearest_within(w: &Registry, character: i64, ship: i64) -> Option<(f64, i64)> {
    let me = w.roster().get(character).copied()?;
    let content = w.content();
    let my_radius = w.pilot(character).map_or(0.0, |p| {
        f64::from(crate::destiny::hull::of_in(content, p.ship_type_id).radius)
    });
    let at = me.motion.position;
    let mut best: Option<(f64, i64)> = None;
    let mut consider = |type_id: i64, position: [f64; 3], radius: f64| {
        let d = crate::space_state::distance(at, position) - my_radius - radius;
        if d < MIN_CLOAKING_DISTANCE && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, type_id));
        }
    };
    for p in w.roster().in_system(me.solar_system_id) {
        if p.ship_id == ship || is_cloaked(w, p.ship_id) {
            continue;
        }
        if w.modules()
            .drones()
            .get(p.character_id)
            .is_some_and(|d| d.owner_id == character)
        {
            continue;
        }
        let Some(pilot) = w.pilot(p.character_id) else {
            continue;
        };
        let radius = f64::from(crate::destiny::hull::of_in(content, pilot.ship_type_id).radius);
        consider(pilot.ship_type_id, p.motion.position, radius);
    }
    for r in w.rocks().in_system(me.solar_system_id) {
        consider(r.type_id, r.position, r.radius);
    }
    for c in w.celestials().served_in(me.solar_system_id) {
        if c.kind != crate::celestial::Kind::Belt {
            consider(c.type_id, c.position, c.radius);
        }
    }
    for (ball, slim) in crate::site::balls_in(w, me.solar_system_id) {
        consider(slim.type_id, ball.position, f64::from(ball.radius));
    }
    best
}

/// Every refusal a cloak activation can meet, in the spec's order (the
/// generic ones -- online, already running, reactivation -- ran before).
/// `now` is blue ticks. Takes the world mutably only to bring the system's
/// positions up to now for the proximity test.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module_id: i64,
    effect_id: i64,
) -> Result<(), ServiceError> {
    let fail = |key: &str| Err(user_error(key, vec![]));
    if device_cloaked(w, ship_id) {
        return fail("CantCloakTooManyDevices");
    }
    let hull = w.item(ship_id).map_or(0, |s| s.type_id);
    let on_hull = |a| crate::typedata::attribute_in(w.content(), hull, a);
    if on_hull(ATTR_CAN_CLOAK).is_none_or(|v| v == 0.0) {
        return fail("CantCloakTooShipUnsuitable");
    }
    if on_hull(ATTR_DISALLOW_CLOAKING).is_some_and(|v| v != 0.0) {
        return fail("CantCloak");
    }
    if let Some(r) = w.modules().running_all().into_iter().find(|r| {
        r.ship_id == ship_id
            && r.module_id != module_id
            && crate::typedata::attribute_in(
                w.content(),
                r.type_id,
                ATTR_DISALLOW_EARLY_DEACTIVATION,
            )
            .is_some_and(|v| v != 0.0)
    }) {
        return Err(user_error(
            "CantCloakModuleActive",
            vec![("module", type_arg(r.type_id))],
        ));
    }
    crate::concord::refuse_cloak(w, character_id)?;
    // **[A]** TQ refuses rather than decloaks. Falsifier: a TQ pilot cloaks
    // while a friend holds a lock on him. (A lock still being acquired
    // counts: the model holds it from the click.)
    if !w.locks().targeters_of(ship_id).is_empty() {
        return fail("CantCloakTargeted");
    }
    // **[A]** the key for a cloak in active warp; falsifier: another text.
    if effect_id != EFFECT_CLOAKING_WARP_SAFE
        && w.roster()
            .get(character_id)
            .is_some_and(|p| p.motion.warp_active())
    {
        return fail("CantCloak");
    }
    if let Some(system) = w.roster().get(character_id).map(|p| p.solar_system_id) {
        w.advance_system(system);
    }
    if let Some((_, type_id)) = nearest_within(w, character_id, ship_id) {
        return Err(user_error(
            "CantCloakProximity",
            vec![
                ("distance", PyValue::Real(MIN_CLOAKING_DISTANCE)),
                ("item", type_arg(type_id)),
            ],
        ));
    }
    Ok(())
}

fn emit(w: &mut Registry, clock: bool, p: impl Push + 'static) {
    if clock {
        w.schedule(Duration::ZERO, p);
    } else {
        w.push(p);
    }
}

/// Everyone in the ship's system with a session, the owner first.
fn audience(w: &Registry, character: i64, system: i64) -> Vec<i64> {
    let mut to = vec![character];
    to.extend(w.roster().others_in_system(system, character));
    to.retain(|id| w.character(*id).is_some());
    to
}

/// **The cloak engages.** `r` is the `Running` that `modules::activate` has
/// just inserted. Stops the ship's other modules at their cycle boundaries,
/// drops its locks, and tells the owner (`CloakBall` and `effects.Cloak`) and
/// every other holder of the ball (`CloakBall`, which removes it from their
/// park).
pub(crate) fn on_activated(w: &mut Registry, r: &Running, now: BlueTime) {
    let Some(here) = w.roster().get(r.character_id).copied() else {
        return;
    };
    // **[A]** the others stop at their own cycle boundary (the MWD-cloak
    // trick). Falsifier: TQ's MWD stops the instant the cloak engages.
    for other in w.modules().running_all() {
        if other.ship_id == r.ship_id && other.module_id != r.module_id && other.stop_at.is_none() {
            let _ = crate::modules::deactivate(
                w,
                Some(r.character_id),
                other.module_id,
                other.effect.name,
                now,
            );
        }
    }
    // **[A]** TQ clears the pilot's locks rather than refusing with
    // `CantCloakTargeting`; falsifier: that text appears on cloak.
    let held = w.locks().targets_of(r.ship_id);
    crate::targeting::clear_all(w, r.character_id, crate::targeting::Reason::Cloaked);
    for target in held {
        // The completion of a lock still acquiring is still queued.
        crate::targeting::cancel(w, r.character_id, target);
    }
    w.ext_mut::<State>().by_ship.insert(
        r.ship_id,
        Cloak {
            character: r.character_id,
            ship_id: r.ship_id,
            mode: MODE_NORMAL,
            kind: Kind::Device {
                module_id: r.module_id,
                warp_safe: r.effect.warp_safe == Some(true),
            },
        },
    );
    // The client stops a follow or orbit of a cloaked ball itself; so does
    // the server's model of it.
    let followers: Vec<(i64, i64)> = w
        .roster()
        .in_system(here.solar_system_id)
        .into_iter()
        .filter(|p| {
            matches!(
                p.motion.mode,
                crate::destiny::Mode::Follow { ball_id, .. }
                    | crate::destiny::Mode::Orbit { ball_id, .. } if ball_id == r.ship_id
            )
        })
        .map(|p| (p.character_id, p.ship_id))
        .collect();
    for (character, ship) in followers {
        if let Some(p) = w.roster_mut().get_mut(character) {
            p.motion.after(crate::destiny::Action::Stop { ball: ship });
        }
    }
    let stamp = w.next_destiny_stamp();
    let to = audience(w, r.character_id, here.solar_system_id);
    let action = crate::destiny::Action::CloakBall {
        ball: r.ship_id,
        mode: MODE_NORMAL,
    };
    if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, vec![action]) {
        w.push(update);
    }
    // The owner's shimmer: the abyss, filament and void controllers track the
    // cloak from this effect (`baseActivationController`).
    let stamp = w.next_destiny_stamp();
    if let Some(fx) = crate::destiny::SpecialFx::new(
        vec![r.character_id],
        stamp,
        r.ship_id,
        "effects.Cloak",
        true,
    ) {
        w.push(fx);
    }
}

// ---------------------------------------------------------------------------
// Decloak
// ---------------------------------------------------------------------------

/// What ended the cloak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cause {
    /// The pilot deactivated the module.
    Manual,
    /// Another object came within [`MIN_CLOAKING_DISTANCE`]; its type.
    Proximity(i64),
    /// The module stopped, went offline or was lost.
    Lost,
}

/// `OnCloakModuleForceDeactivated(key, args)` (`cloakingSvc.py:18`).
#[derive(Debug, Clone, PartialEq)]
pub struct ForceDeactivated {
    pub character: i64,
    pub proximity_type: i64,
}

impl Push for ForceDeactivated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnCloakModuleForceDeactivated",
            vec![
                crate::pyutil::bstr("CloakDeactivatedByProximityToType"),
                PyValue::Dict(crate::pyutil::pypairs(vec![(
                    "proximityTypeID",
                    PyValue::Int(self.proximity_type),
                )])),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// **Decloak** `ship`. `false` when it was not cloaked. `clock` says which
/// queue the pushes use: the world clock's, or the answer to the call that
/// caused it.
///
/// The recalibration delay starts now (`cloakingTargetingDelay`, the
/// **modified** value), the module stops now (not at a cycle boundary), the
/// owner gets `UncloakBall` and `effects.Uncloak`, and the observers get the
/// ball back through the grid reconcile, then the same effect.
pub fn uncloak(w: &mut Registry, ship: i64, cause: Cause, now: BlueTime, clock: bool) -> bool {
    let Some(c) = w.ext_mut::<State>().by_ship.remove(&ship) else {
        return false;
    };
    let Kind::Device { module_id, .. } = c.kind else {
        // A gate cloak: no module, no recalibration.
        return announce(w, c, ship, None, now, clock, true);
    };
    let type_id = w.item(module_id).map_or(0, |i| i.type_id);
    let delay_ms = crate::modules::module_attr(w, ship, module_id, type_id, ATTR_TARGETING_DELAY)
        .unwrap_or(0.0);
    w.ext_mut::<State>()
        .recal_until
        .insert(ship, now.saturating_add(delay_ms.round() as i64 * MSEC));
    // `modules::stop` also puts the top speed back (the cloak's 306 no
    // longer applies). Not running: the module was already ended.
    crate::modules::stop(w, module_id, now, None);
    announce(w, c, ship, Some(cause), now, clock, false)
}

/// The pushes of a decloak, shared by a device and a gate cloak: the owner's
/// `UncloakBall` and `effects.Uncloak` (and the timer's cancel for a gate
/// cloak, the forced-decloak dialog for a proximity), the observers' ball
/// back through the reconcile, then the effect to them.
fn announce(
    w: &mut Registry,
    c: Cloak,
    ship: i64,
    cause: Option<Cause>,
    _now: BlueTime,
    clock: bool,
    jump: bool,
) -> bool {
    let Some(here) = w
        .roster()
        .get(c.character)
        .copied()
        .filter(|p| p.ship_id == ship)
    else {
        return true;
    };
    let stamp = w.next_destiny_stamp();
    if let Some(update) = crate::destiny::ActionUpdate::new(
        vec![c.character],
        stamp,
        vec![crate::destiny::Action::UncloakBall { ball: ship }],
    ) {
        emit(w, clock, update);
    }
    let stamp = w.next_destiny_stamp();
    if let Some(fx) =
        crate::destiny::SpecialFx::new(vec![c.character], stamp, ship, "effects.Uncloak", true)
    {
        emit(w, clock, fx);
    }
    if jump {
        emit(
            w,
            clock,
            JumpCloakCancelled {
                character: c.character,
                ship_id: ship,
            },
        );
    }
    if let Some(Cause::Proximity(type_id)) = cause {
        emit(
            w,
            clock,
            ForceDeactivated {
                character: c.character,
                proximity_type: type_id,
            },
        );
    }
    // The holders-to-be get `AddBalls2` now, not at the next second's
    // reconcile; the effect follows the add in the queue and the `Names`
    // filter delivers it only to those who then hold the ball.
    crate::grid::reconcile_in(w, here.solar_system_id);
    let others: Vec<i64> = audience(w, c.character, here.solar_system_id)
        .into_iter()
        .skip(1)
        .collect();
    let stamp = w.next_destiny_stamp();
    if let Some(fx) = crate::destiny::SpecialFx::new(others, stamp, ship, "effects.Uncloak", true) {
        w.schedule(Duration::ZERO, fx);
    }
    true
}

/// `OnJumpCloakUpdated(shipID, endTime, duration)`
/// (`crimewatchTimers.py:779` **[V]**), in blue ticks like `invuln::Started`.
#[derive(Debug, Clone, PartialEq)]
pub struct JumpCloakUpdated {
    pub character: i64,
    pub ship_id: i64,
    pub until: BlueTime,
    pub duration: BlueTime,
}

impl Push for JumpCloakUpdated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnJumpCloakUpdated",
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Int(self.until),
                PyValue::Int(self.duration),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// `OnJumpCloakCancelled(shipID)` (`crimewatchTimers.py:782` **[V]**).
#[derive(Debug, Clone, PartialEq)]
pub struct JumpCloakCancelled {
    pub character: i64,
    pub ship_id: i64,
}

impl Push for JumpCloakCancelled {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnJumpCloakCancelled", vec![PyValue::Int(self.ship_id)])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// **The gate cloak.** An arrival through a gate, a wormhole or a jump drive
/// is cloaked for [`JUMP_CLOAK_DURATION`] in place of the undock's
/// invulnerability. Follows `invuln`'s switch (`--no-undock-invuln`). The
/// ego's SetState, built after this, carries [`MODE_RESTORE`] in the ball's
/// cloak byte and the client plays `CloakNoAmim` itself **[V]**; observers
/// are never given the ball. **[A]** the mode is `DSTRESTORECLOAK` (falsifier:
/// `CloakBall called with mode` in the client log, or no cloak look on
/// arrival; the fallback is [`MODE_NORMAL`]), and that wormhole and
/// jump-drive arrivals get it too (falsifier: a TQ jump-drive arrival with
/// no timer).
pub fn grant_jump(w: &mut Registry, character: i64, ship_id: i64, now: BlueTime) {
    if !crate::invuln::enabled(w) {
        return;
    }
    let until = now.saturating_add(JUMP_CLOAK_DURATION);
    w.ext_mut::<State>().by_ship.insert(
        ship_id,
        Cloak {
            character,
            ship_id,
            mode: MODE_RESTORE,
            kind: Kind::Jump { until },
        },
    );
    w.schedule(
        Duration::ZERO,
        JumpCloakUpdated {
            character,
            ship_id,
            until,
            duration: JUMP_CLOAK_DURATION,
        },
    );
}

/// Drop a gate cloak left over from before a dock, silently.
pub fn forget(w: &mut Registry, ship_id: i64) {
    let s = w.ext_mut::<State>();
    if s.by_ship.is_empty() {
        return;
    }
    if s.by_ship
        .get(&ship_id)
        .is_some_and(|c| matches!(c.kind, Kind::Jump { .. }))
    {
        s.by_ship.remove(&ship_id);
    }
}

/// **The pilot did something**: a movement order, a warp, a lock, a module
/// activation without attribute 3123. Ends the character's gate cloak and
/// tells everyone; `false` when there was none. No recalibration delay
/// **[A]**; falsifier: TQ shows "recalibrating" after a gate cloak. The
/// triggers are **[A]** too: a TQ pilot who keeps the cloak after one.
pub fn break_jump(w: &mut Registry, character: i64) -> bool {
    {
        let s = w.ext::<State>();
        if s.by_ship.is_empty() {
            return false;
        }
    }
    let Some(ship) = w.roster().get(character).map(|p| p.ship_id) else {
        return false;
    };
    let is_jump = w
        .ext::<State>()
        .by_ship
        .get(&ship)
        .is_some_and(|c| matches!(c.kind, Kind::Jump { .. }));
    if !is_jump {
        return false;
    }
    uncloak(w, ship, Cause::Manual, 0, true)
}

/// A module activation by `character`: breaks the gate cloak unless the
/// module carries attribute 3123 (a Warp Core Stabilizer).
pub(crate) fn break_for_module(w: &mut Registry, character: i64, type_id: i64) -> bool {
    if crate::typedata::attribute_in(w.content(), type_id, ATTR_CAN_ACTIVATE_IN_GATE_CLOAK)
        .is_some_and(|v| v != 0.0)
    {
        return false;
    }
    break_jump(w, character)
}

/// `modules::stop` ended a cloak module by some other route (the warp
/// shutdown, a reason of its own): the ship decloaks.
pub(crate) fn on_module_stopped(w: &mut Registry, ship: i64, now: BlueTime) {
    uncloak(w, ship, Cause::Lost, now, true);
}

/// **The world clock's stage**: proximity decloak, a cloak whose module is
/// gone, and the forgetting of a cloak whose ship has left space (a dock, a
/// jump, a death), which pushes nothing: the client's park is torn down.
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    {
        let s = w.ext_mut::<State>();
        s.recal_until.retain(|_, until| now < *until);
        if s.by_ship.is_empty() {
            return;
        }
    }
    let cloaks: Vec<Cloak> = w.ext::<State>().by_ship.values().copied().collect();
    let mut advanced: BTreeSet<i64> = BTreeSet::new();
    for c in cloaks {
        let Some(here) = w
            .roster()
            .get(c.character)
            .copied()
            .filter(|p| p.ship_id == c.ship_id)
        else {
            w.ext_mut::<State>().by_ship.remove(&c.ship_id);
            continue;
        };
        let (module_id, warp_safe) = match c.kind {
            Kind::Device {
                module_id,
                warp_safe,
            } => (module_id, warp_safe),
            // The gate cloak runs out; it never breaks on proximity.
            Kind::Jump { until } => {
                if now >= until {
                    uncloak(w, c.ship_id, Cause::Lost, now, true);
                }
                continue;
            }
        };
        if w.modules().running(module_id).is_none() {
            uncloak(w, c.ship_id, Cause::Lost, now, true);
            continue;
        }
        // The covert-ops cloak passes things in active warp.
        if warp_safe && here.motion.warp_active() {
            continue;
        }
        if advanced.insert(here.solar_system_id) {
            w.advance_system(here.solar_system_id);
        }
        if let Some((_, type_id)) = nearest_within(w, c.character, c.ship_id) {
            uncloak(w, c.ship_id, Cause::Proximity(type_id), now, true);
        }
    }
}

/// A cloaking device running on `ship`, for another module's test.
#[cfg(test)]
pub(crate) fn set_device_for_test(w: &mut Registry, character: i64, ship_id: i64) {
    w.ext_mut::<State>().by_ship.insert(
        ship_id,
        Cloak {
            character,
            ship_id,
            mode: MODE_RESTORE,
            kind: Kind::Device {
                module_id: 0,
                warp_safe: false,
            },
        },
    );
}

#[cfg(test)]
mod tests;
