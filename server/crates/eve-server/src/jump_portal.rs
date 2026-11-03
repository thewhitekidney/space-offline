//! **Jump portals and the conduit** (CAP-G; `docs/lanes/cap-g.md`,
//! `out/research/R-CAP.md` §3.4.6-7): a Jump Portal Generator opens a portal
//! to a fleet member's lit cynosural field and fleet members step through;
//! a conduit anchor jumps with fleet members riding along.
//!
//! ## The calls **[V]** (`cynoTravelSvc.py`)
//!
//! - `beyonce.CmdBridgeToFleetModuleBeacon(charID, beaconID, solarsystemID)`:
//!   no session change; the answer is dropped. [`cmd_bridge_to_fleet_module_beacon`].
//! - `beyonce.CmdJumpThroughFleet(otherCharID, otherShipID, beaconID,
//!   solarsystemID)` inside `PerformSessionChange('jump', ...)`; the last two
//!   come from `fleet.GetActiveBridgeForShip(otherShipID)`.
//!   [`cmd_jump_through_fleet`].
//! - `beyonce.CmdGroupJumpToFleetModuleBeacon(charID, beaconID, solarsystemID)`
//!   inside `PerformSessionChange('jump', ...)` (the conduit; the arm is
//!   `jump_drive::cmd_group_jump_to_fleet_module_beacon`, the passengers are
//!   chosen and sent here).
//! - Pushes: `OnBridgeModeChange(shipID, solarsystemID, itemID, active)` to the
//!   fleet (`fleetSvc.py:196`, kept and replayed by `fleet.rs`, as the
//!   beacons are), `OnGroupJumpAnchorActivated(toSolarsystemID, numPassengers)`
//!   to the anchor and `OnGroupJumpedPassenger(anchorCharID, toSolarsystemID)`
//!   to each passenger (`cynoTravelSvc.py:19-24`, each shows a log line).
//!
//! ## Opening a portal
//!
//! The pilot's hull has a drive (`canJump`) **and** an online module whose
//! attribute 3318 is set (`sessionChecker.canOpenJumpPortal` [V]); else
//! `JumpNoDrive` / `JumpNoPortal` ("Your ship is not fitted with a jump portal
//! generator" [V text]). The beacon is one the pilot's fleet holds
//! (`NoActiveBeacon`), not in this system, not high-sec or a wormhole, the
//! hull's typelist admits the field, and the distance is within the hull's
//! `jumpDriveRange`: the client builds the "Bridge to" menu with the very
//! function that builds "Jump to" (`menusvc.GetHybridBridgeMenu` calls
//! `_GetCynoTargetSubMenus`, whose distance filter is the drive's range [V]).
//!
//! The module is then **cycled by the ordinary module engine**: the effect is
//! 2152 `jumpPortalGeneration` (covert: 3674), whose duration attribute is
//! 1002 (the module's 60 s [V-data]) and whose two modifiers on the ship are
//! `warpScrambleStatus` plus the module's `siegeModeWarpStatus` and
//! `maxVelocity` post-percent by `speedFactor` (-100) [V-data]: a bridging
//! ship is held still while the portal is open, exactly as a cyno's pilot is.
//! A run is one cycle (`repeat` 1), and it starts through
//! `modules::activate`, so the capacitor, the online state and the
//! module's own refusals are the engine's. A click on the module itself is
//! refused ([`check_activation`]): the client's only call to open a portal is
//! the menu's.
//!
//! The module's own fuel is Strontium Clathrates (`consumptionType` 713 /
//! `consumptionQuantity` 714 [V-data], the latter reduced by Jump Portal
//! Generation through effect 4814 [V-data], which `crate::dogma` applies),
//! paid at the opening from the fuel bay then the cargo.
//!
//! ## Jumping through
//!
//! In the client the entry is offered when the pilot is in the bridging
//! ship's system, not warping, **within `maxStargateJumpingDistance` (2,500 m)
//! of it**, in its fleet and the ship has an active bridge
//! (`celestialCheckers.OfferJumpFleetThroughToSystem` [V]); the server asks the
//! same again and answers `JumpNoBridge` ("No portal active on this ship"
//! [V text]) or `NotCloseEnoughToJump{maxdist}` (the client's own key, which
//! the autopilot answers by approaching [V]). The passenger's ship must carry
//! the attribute the module names in 3318 (3319 titan, 3320 black ops, 3325
//! industrial [V-data]): the attribute's `defaultValue` is 1 for the titan's
//! and 0 for the others and the hulls the portal cannot carry set it to 0
//! [V-data]; otherwise `JumpHarmonicsDoNotMatch` ("Your ship does not have the
//! technology to utilize that jump portal" [V text]).
//!
//! **Fuel is paid by the bridging ship**, not the passenger: the client's own
//! dialog says so, `JumpNotEnoughCharge2` "...requires N units of X, but the
//! servicing ship only has M in its cargo hold" [V text], with the argument
//! names of `JumpNotEnoughCharge`. The amount is
//! `max(long(ly x passengerMass x jumpPortalConsumptionMassFactor x
//! jumpDriveConsumptionAmount), 1)` of the bridging ship's `jumpDriveConsumptionType`,
//! the factor the module's (1001, modified), the per-light-year amount the
//! bridging hull's modified 868. The *product* is **[A]**, see below; the
//! rounding is the drive's [V].
//!
//! The passenger then leaves by the jump drive's legs
//! (`jump_drive::depart`) and lands within the field's
//! `cynosuralFieldSpawnRadius`. Their fatigue is set by CAP-F's `on_jump` at
//! the distance in light years with the multiplier **1971 of the passenger's
//! hull times 1972 `jumpThroughFatigueMultiplier` of the bridging hull**
//! (only the black ops hulls set the latter, 0.25 [V-data]); the HUD's own
//! text says "jump portal travel leaves capsuleers feeling fatigued" [V text].
//!
//! ## The conduit
//!
//! The anchor's hull names, in 3321, an attribute passengers must carry
//! (5682 `isCarrierJumpConduitPassenger` default 1, capitals 0; 3322 black
//! ops; 3324 industrial [V-data]) and its `conduitJumpPassengerCount` (3133)
//! is how many ride. A covert portal generator assigns the hull's 3321
//! when fitted (effect 8467 [V-data]), which `crate::dogma` applies. Fuel is the
//! anchor's own, at `conduitJumpDriveConsumptionAmount` (3131) per light
//! year where the hull has one [V-data names]. Passengers are the fleet
//! members that accept conduit jumps (`SetAcceptsConduitJumpsValue`,
//! `memberOptOuts.acceptsConduitJumps`, default true [V]), carry the
//! attribute, are in the anchor's system within [`CONDUIT_RANGE_M`] and free
//! to jump; the nearest `3133` go. Each passenger gets the fatigue of their
//! own hull's multiplier.
//!
//! ## The [A]s (the lane note has each falsifier)
//!
//! - the portal fuel product (no client code computes it: the client names the
//!   factor, the mass and the servicing ship's cargo only);
//! - conduit passengers' range [`CONDUIT_RANGE_M`] (nothing in the client);
//! - the activation timer refuses a passenger as it refuses a pilot, and does
//!   not refuse the bridging pilot (the client's tooltip says only that
//!   portal travel fatigues);
//! - 1971 x 1972 compose;
//! - the portal closes when its destination field goes out.
//!
//! ## State
//!
//! The open portals live in the world slot [`SLOT`] and in nothing persisted
//! (a restart empties space, as for the cynos). The fleet's own view is
//! `crate::fleet::Fleets::bridges`.

use crate::celestial::{Band, Centers};
use crate::cyno::{self, Field};
use crate::damage::BlueTime;
use crate::fleet::Bridge;
use crate::jump_drive::{
    ATTR_CAN_JUMP, ATTR_FUEL_AMOUNT, ATTR_FUEL_TYPE, ATTR_RANGE, attribute, attribute_or_default,
    held_beacon, ship_type_of,
};
use crate::modules::{type_arg, type_arg_for, user_error};
use crate::notify::Push;
use crate::service::{Registry, Request, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// `effectJumpPortalGeneration` and `effectJumpPortalGenerationBO` **[V]**
/// (`dogma/const`): the standard (and industrial) and the covert generator's
/// effect.
pub const EFFECT_JUMP_PORTAL: i64 = 2_152;
pub const EFFECT_JUMP_PORTAL_BO: i64 = 3_674;

/// `jumpPortalConsumptionMassFactor` **[V]**.
pub const ATTR_MASS_FACTOR: i64 = 1_001;
/// `jumpPortalPassengerRequiredAttributeID`, on the module **[V]**: the
/// attribute a passenger's ship must carry.
pub const ATTR_PASSENGER_REQUIRED: i64 = 3_318;
/// `jumpConduitPassengerRequiredAttributeID`, on the hull **[V]**.
pub const ATTR_CONDUIT_REQUIRED: i64 = 3_321;
/// `conduitJumpPassengerCount` **[V]**.
pub const ATTR_CONDUIT_COUNT: i64 = 3_133;
/// `conduitJumpDriveConsumptionAmount` **[V]** (the name is the data's).
pub const ATTR_CONDUIT_FUEL_AMOUNT: i64 = 3_131;
/// `jumpThroughFatigueMultiplier` **[V]** (`dogma/const`).
pub const ATTR_THROUGH_FATIGUE: i64 = 1_972;
/// `consumptionType` and `consumptionQuantity` of the module **[V]**.
pub const ATTR_MODULE_FUEL_TYPE: i64 = 713;
pub const ATTR_MODULE_FUEL_QUANTITY: i64 = 714;
/// `capacitorNeed` **[V]**.
pub const ATTR_CAPACITOR_NEED: i64 = 6;

/// How far from the anchor a conduit passenger may be, surface to surface,
/// metres **[A]**: no client code names it (EvEJS uses the same ten
/// kilometres). Falsified by a retail conduit that takes, or leaves, a ship
/// at another distance.
pub const CONDUIT_RANGE_M: f64 = 10_000.0;

/// Whether `effect_id` is a jump portal generator's.
pub fn is_portal(effect_id: i64) -> bool {
    effect_id == EFFECT_JUMP_PORTAL || effect_id == EFFECT_JUMP_PORTAL_BO
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One open portal, by its generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Portal {
    pub character_id: i64,
    pub ship_id: i64,
    pub module_id: i64,
    pub module_type_id: i64,
    pub system_id: i64,
    pub dest_system: i64,
    /// The field's ball.
    pub beacon_id: i64,
    /// The attribute passengers must carry (the module's 3318).
    pub passenger_attribute: i64,
}

/// The open portals and the modules a portal is being started on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Portals {
    open: BTreeMap<i64, Portal>,
    armed: BTreeSet<i64>,
}

fn make_slot(_: &crate::content::Content) -> Boxed {
    Box::new(Portals::default())
}

/// The world slot (coordinator: `world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "jump_portal",
    make: make_slot,
};

/// Every open portal.
pub fn portals(w: &Registry) -> impl Iterator<Item = &Portal> {
    w.ext::<Portals>().open.values()
}

/// The portal a ship holds open.
pub fn portal_of_ship(w: &Registry, ship_id: i64) -> Option<Portal> {
    portals(w).find(|p| p.ship_id == ship_id).copied()
}

// ---------------------------------------------------------------------------
// Reading the module
// ---------------------------------------------------------------------------

/// A fitted module's attribute: the dogma sheet's modified value, else the
/// type's own.
fn module_value(w: &Registry, ship_id: i64, module_id: i64, type_id: i64, a: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a)
        .filter(|v| *v != 0.0)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, a))
}

/// The online portal generator on `ship_id`, lowest item id first: `(module,
/// type, effect name)`.
fn portal_module(w: &Registry, ship_id: i64) -> Option<(i64, i64, &'static str)> {
    w.store()
        .contents_of(ship_id)
        .filter(|i| {
            crate::item::is_ship_fitting_flag(i.flag_id) && w.modules().is_online(i.item_id)
        })
        .filter_map(|i| {
            let effect = crate::modules::find_in(w.content(), i.type_id)?.effect?;
            is_portal(effect.effect_id).then_some((i.item_id, i.type_id, effect.name))
        })
        .min_by_key(|(id, _, _)| *id)
}

/// A ship's mass, kilograms: the modified hull's when `crate::dogma` has a
/// sheet, else the type's.
fn ship_mass(w: &Registry, ship_id: i64, ship_type: i64) -> f64 {
    crate::dogma::hull(w, ship_id)
        .map(|h| h.mass)
        .unwrap_or_else(|| crate::destiny::hull::of_in(w.content(), ship_type).mass)
}

fn type_name(w: &Registry, type_id: i64) -> String {
    crate::item::find_in(w.content(), type_id)
        .map_or_else(|| format!("type {type_id}"), |t| t.name.to_owned())
}

/// What a bridging hull pays for one passenger **[A]** (module header):
/// `max(long(ly x mass x factor x perLY), 1)`.
pub fn portal_fuel_required(ly: f64, passenger_mass: f64, mass_factor: f64, per_ly: f64) -> i64 {
    crate::jump_drive::fuel_required(ly * passenger_mass * mass_factor, per_ly)
}

/// Surface-to-surface metres between two ships in space in one system.
fn surface_distance(w: &Registry, a: i64, b: i64) -> Option<f64> {
    let me = w.roster().get(a)?;
    let them = w.roster().get(b)?;
    if me.solar_system_id != them.solar_system_id {
        return None;
    }
    let r = |ship_id: i64, who: i64| {
        let ty = ship_type_of(w, ship_id, who);
        f64::from(crate::destiny::hull::of_in(w.content(), ty).radius)
    };
    Some(
        crate::space_state::distance(me.motion.position, them.motion.position)
            - r(me.ship_id, a)
            - r(them.ship_id, b),
    )
}

// ---------------------------------------------------------------------------
// Opening a portal
// ---------------------------------------------------------------------------

/// `beyonce.CmdBridgeToFleetModuleBeacon(charID, beaconID, solarsystemID)`.
pub fn cmd_bridge_to_fleet_module_beacon(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    open(reg, req, crate::pyutil::filetime_now())
}

fn open(reg: &mut Registry, req: &Request<'_>, now: BlueTime) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let (Some(owner), Some(beacon_id), Some(system)) = (int(req, 0), int(req, 1), int(req, 2))
    else {
        return Err(notify(
            "CmdBridgeToFleetModuleBeacon needs (charID, beaconID, solarsystemID).",
        ));
    };
    if reg.jumps().contains(actor) {
        return Err(user_error("ShipJumping", vec![]));
    }
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    if here.motion.warp_active() {
        return Err(user_error("DeniedActivateInWarp", vec![]));
    }
    let ship_id = here.ship_id;
    let origin = here.solar_system_id;
    let ship_type = ship_type_of(reg, ship_id, actor);
    if attribute(reg, ship_id, ship_type, ATTR_CAN_JUMP).is_none_or(|v| v <= 0.0) {
        return Err(user_error("JumpNoDrive", vec![]));
    }
    let Some((module_id, module_type, effect_name)) = portal_module(reg, ship_id) else {
        return Err(user_error("JumpNoPortal", vec![]));
    };
    if reg.modules().running(module_id).is_some() || portal_of_ship(reg, ship_id).is_some() {
        return Err(user_error(
            "EffectAlreadyActive2",
            vec![("modulename", type_arg(module_type))],
        ));
    }
    let Some(beacon) = held_beacon(reg, actor, owner, beacon_id, system) else {
        return Err(user_error("NoActiveBeacon", vec![]));
    };
    if system == origin {
        return Err(user_error("JumpDestinationInvalid", vec![]));
    }
    match reg
        .content()
        .get::<crate::celestial::Universe>()
        .band(system)
    {
        Some(Band::High) => {
            return Err(user_error(
                "CantEnterHighSec",
                vec![("typeID", type_arg(ship_type))],
            ));
        }
        Some(Band::Wormhole | Band::Void) => {
            return Err(user_error("JumpDestinationInvalid", vec![]));
        }
        _ => {}
    }
    if !crate::jump_drive::harmonics_match(reg, ship_id, ship_type, beacon.type_id) {
        return Err(user_error("JumpDestinationHarmonicInvalid", vec![]));
    }
    let Some(ly) = reg.content().get::<Centers>().ly(origin, system) else {
        return Err(user_error("JumpDestinationInvalid", vec![]));
    };
    let range = attribute(reg, ship_id, ship_type, ATTR_RANGE).unwrap_or(0.0);
    if ly > range {
        return Err(user_error(
            "JumpTooFar",
            vec![("dist", PyValue::Real(ly)), ("range", PyValue::Real(range))],
        ));
    }
    if cyno::field_by_ball(reg, beacon.beacon_id).is_none() {
        return Err(user_error("NoActiveBeacon", vec![]));
    }
    // The module's own fuel, checked now and paid once the module runs.
    let fuel = module_value(reg, ship_id, module_id, module_type, ATTR_MODULE_FUEL_TYPE)
        .map_or(0, |v| v.round() as i64)
        .max(0);
    let quantity = module_value(
        reg,
        ship_id,
        module_id,
        module_type,
        ATTR_MODULE_FUEL_QUANTITY,
    )
    .map_or(0, |v| v.round() as i64)
    .max(0);
    let mut fuel_edits = Vec::new();
    if fuel > 0 && quantity > 0 {
        let stacks = crate::jump_drive::fuel_stacks(reg, actor, ship_id, fuel);
        let Some(edits) = crate::jump_drive::fuel_plan(&stacks, quantity) else {
            return Err(user_error(
                "ModuleRequiresFuel",
                vec![
                    ("fuel", crate::pyutil::bstr(&type_name(reg, fuel))),
                    ("fueled", crate::pyutil::bstr(&type_name(reg, module_type))),
                ],
            ));
        };
        fuel_edits = edits;
    }
    let passenger_attribute = module_value(
        reg,
        ship_id,
        module_id,
        module_type,
        ATTR_PASSENGER_REQUIRED,
    )
    .map_or(0, |v| v.round() as i64);

    // Start the module: the engine's refusals (capacitor, lock, online) and
    // its cycle. Armed, so that `check_activation` lets this one through.
    reg.ext_mut::<Portals>().armed.insert(module_id);
    let started = crate::modules::activate(reg, Some(actor), module_id, effect_name, None, 1, now);
    reg.ext_mut::<Portals>().armed.remove(&module_id);
    started?;
    if reg.modules().running(module_id).is_none() {
        // "Not enough capacitor": the engine let the module fall idle with no
        // dialog; the client has a key for this.
        let (got, _) = crate::modules::capacitor_at(reg, ship_id, ship_type, now);
        let need = crate::typedata::attribute_in(reg.content(), module_type, ATTR_CAPACITOR_NEED)
            .unwrap_or(0.0);
        return Err(user_error(
            "JumpNotEnoughCap",
            vec![("got", PyValue::Real(got)), ("need", PyValue::Real(need))],
        ));
    }
    if let Err(e) = reg.apply_items(&fuel_edits) {
        crate::errln!("[jump-portal] fuel for {actor} not taken: {e:?}");
    }
    let portal = Portal {
        character_id: actor,
        ship_id,
        module_id,
        module_type_id: module_type,
        system_id: origin,
        dest_system: system,
        beacon_id: beacon.beacon_id,
        passenger_attribute,
    };
    reg.ext_mut::<Portals>().open.insert(module_id, portal);
    let pushes = reg.fleets_mut().open_bridge(
        actor,
        Bridge {
            ship_id,
            solar_system_id: system,
            beacon_id: beacon.beacon_id,
        },
    );
    for p in pushes {
        reg.push(p);
    }
    crate::outln!(
        "[jump-portal] character {actor} ship {ship_id}: portal {origin} -> {system} to \
         {owner}'s field {}, {ly:.3} ly",
        beacon.beacon_id
    );
    Ok(PyValue::None)
}

/// What `modules::activate` asks before a portal generator starts (the `if
/// is_portal` arm, in `modules.rs`): only this module's own
/// [`cmd_bridge_to_fleet_module_beacon`] starts one, because a portal needs a
/// destination and the client's click on the module has none.
pub fn check_activation(w: &Registry, effect_id: i64, module_id: i64) -> Result<(), ServiceError> {
    if !is_portal(effect_id) || w.ext::<Portals>().armed.contains(&module_id) {
        return Ok(());
    }
    Err(notify(
        "Open a portal with Bridge To in the ship's menu: the module needs a destination.",
    ))
}

// ---------------------------------------------------------------------------
// Jumping through
// ---------------------------------------------------------------------------

/// `beyonce.CmdJumpThroughFleet(otherCharID, otherShipID, beaconID,
/// solarsystemID)`.
pub fn cmd_jump_through_fleet(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    jump_through(reg, req, crate::pyutil::filetime_now())
}

fn jump_through(
    reg: &mut Registry,
    req: &Request<'_>,
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let (Some(owner), Some(bridge_ship), Some(beacon_id), Some(system)) =
        (int(req, 0), int(req, 1), int(req, 2), int(req, 3))
    else {
        return Err(notify(
            "CmdJumpThroughFleet needs (charID, shipID, beaconID, solarsystemID).",
        ));
    };
    if reg.jumps().contains(actor) {
        return Err(user_error("ShipJumping", vec![]));
    }
    reg.advance_character(actor);
    reg.advance_character(owner);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    if here.ship_id == bridge_ship || owner == actor {
        return Err(notify("You cannot jump through your own portal."));
    }
    // The portal: the fleet's, and still open.
    let held = reg
        .fleets()
        .bridges_visible_to(actor)
        .into_iter()
        .find(|(who, b)| {
            *who == owner
                && b.ship_id == bridge_ship
                && b.beacon_id == beacon_id
                && b.solar_system_id == system
        });
    let Some(portal) = held.and_then(|_| portal_of_ship(reg, bridge_ship)) else {
        return Err(user_error("JumpNoBridge", vec![]));
    };
    let origin = here.solar_system_id;
    if portal.character_id != owner || portal.system_id != origin || portal.dest_system != system {
        return Err(user_error("JumpNoBridge", vec![]));
    }
    let Some(gap) = surface_distance(reg, actor, owner) else {
        return Err(user_error("JumpNoBridge", vec![]));
    };
    if gap >= crate::jump::MAX_JUMP_DISTANCE {
        return Err(user_error(
            "NotCloseEnoughToJump",
            vec![(
                "maxdist",
                PyValue::Int(crate::jump::MAX_JUMP_DISTANCE as i64),
            )],
        ));
    }
    crate::concord::refuse_beacon_jump(reg, Some(actor), now)?;
    crate::crimewatch::refuse_jump(reg, Some(actor), now)?;
    if let Some(refusal) =
        crate::movement_guard::check(&crate::module::plan().jump_guards, reg, actor)
    {
        return Err(refusal.into_error());
    }
    let ship_id = here.ship_id;
    let ship_type = ship_type_of(reg, ship_id, actor);
    if attribute_or_default(reg, ship_id, ship_type, portal.passenger_attribute)
        .is_none_or(|v| v <= 0.0)
    {
        return Err(user_error("JumpHarmonicsDoNotMatch", vec![]));
    }
    match reg
        .content()
        .get::<crate::celestial::Universe>()
        .band(system)
    {
        Some(Band::High) => {
            return Err(user_error(
                "CantEnterHighSec",
                vec![("typeID", type_arg(ship_type))],
            ));
        }
        Some(Band::Wormhole | Band::Void) => {
            return Err(user_error("JumpDestinationInvalid", vec![]));
        }
        _ => {}
    }
    if let Some(left) = crate::jump_fatigue::activation_remaining(reg, actor, now) {
        return Err(user_error(
            "JumpActivationTimer",
            vec![("remaining", PyValue::Int(left))],
        ));
    }
    let Some(ly) = reg.content().get::<Centers>().ly(origin, system) else {
        return Err(user_error("JumpDestinationInvalid", vec![]));
    };
    let Some(field) = cyno::field_by_ball(reg, portal.beacon_id) else {
        return Err(user_error("JumpDestinationInvalid", vec![]));
    };

    // The bridging ship pays, by the passenger's mass.
    let bridge_type = ship_type_of(reg, bridge_ship, owner);
    let fuel_type =
        attribute(reg, bridge_ship, bridge_type, ATTR_FUEL_TYPE).map_or(0, |v| v as i64);
    let per_ly = attribute(reg, bridge_ship, bridge_type, ATTR_FUEL_AMOUNT).unwrap_or(0.0);
    let factor = module_value(
        reg,
        bridge_ship,
        portal.module_id,
        portal.module_type_id,
        ATTR_MASS_FACTOR,
    )
    .unwrap_or(0.0);
    let required = portal_fuel_required(ly, ship_mass(reg, ship_id, ship_type), factor, per_ly);
    let stacks = crate::jump_drive::fuel_stacks(reg, owner, bridge_ship, fuel_type);
    let available: i64 = stacks.iter().map(|s| s.quantity).sum();
    let Some(edits) = crate::jump_drive::fuel_plan(&stacks, required) else {
        return Err(user_error(
            "JumpNotEnoughCharge2",
            vec![
                (
                    "typeName",
                    type_arg_for("JumpNotEnoughCharge2", "typeName", fuel_type),
                ),
                ("available", PyValue::Int(available)),
                ("required", PyValue::Int(required)),
            ],
        ));
    };

    // **Nothing refuses from here.**
    if let Err(e) = reg.apply_items(&edits) {
        crate::errln!("[jump-portal] fuel for {owner} not taken: {e:?}");
        return Err(user_error("JumpDestinationBlocked", vec![]));
    }
    let multiplier = crate::jump_fatigue::ship_multiplier(reg, actor)
        * crate::typedata::attribute_in(reg.content(), bridge_type, ATTR_THROUGH_FATIGUE)
            .unwrap_or(1.0);
    crate::jump_fatigue::on_jump_at(reg, actor, ly, multiplier, now);
    let spot = crate::jump_drive::landing_spot(
        &field,
        crate::seed::mix(
            1,
            &[actor as u64, field.ball_id as u64, now as u64, owner as u64],
        ),
    );
    crate::jump_drive::depart(reg, actor, ship_id, origin, system, spot);
    crate::outln!(
        "[jump-portal] character {actor} ship {ship_id}: {origin} -> {system} through {owner}'s \
         portal, {ly:.3} ly, {required} of type {fuel_type} from the bridging ship"
    );
    Ok(PyValue::None)
}

// ---------------------------------------------------------------------------
// The conduit
// ---------------------------------------------------------------------------

/// Who rides with `anchor` (module header): fleet members that accept
/// conduit jumps, are in `origin` within [`CONDUIT_RANGE_M`], are free to
/// jump (the gate's own guards, no activation timer running) and whose hull
/// carries `required_attribute`; nearest first, at most `limit`.
pub fn conduit_passengers(
    reg: &mut Registry,
    anchor: i64,
    origin: i64,
    _dest_system: i64,
    required_attribute: i64,
    limit: usize,
    now: BlueTime,
) -> Vec<i64> {
    if limit == 0 {
        return Vec::new();
    }
    let members: Vec<i64> = reg
        .fleets()
        .fleet_of(anchor)
        .map(|f| f.members.keys().copied().filter(|m| *m != anchor).collect())
        .unwrap_or_default();
    let mut ranked: Vec<(f64, i64)> = Vec::new();
    for who in members {
        if !reg.fleets().accepts_conduit_jumps(who) || reg.jumps().contains(who) {
            continue;
        }
        reg.advance_character(who);
        let Some(there) = reg.roster().get(who).copied() else {
            continue;
        };
        if there.solar_system_id != origin {
            continue;
        }
        let Some(gap) = surface_distance(reg, anchor, who) else {
            continue;
        };
        if gap > CONDUIT_RANGE_M {
            continue;
        }
        if crate::concord::refuse_beacon_jump(reg, Some(who), now).is_err()
            || crate::crimewatch::refuse_jump(reg, Some(who), now).is_err()
            || crate::movement_guard::check(&crate::module::plan().jump_guards, reg, who).is_some()
            || crate::jump_fatigue::activation_remaining(reg, who, now).is_some()
        {
            continue;
        }
        let ty = ship_type_of(reg, there.ship_id, who);
        if attribute_or_default(reg, there.ship_id, ty, required_attribute).is_none_or(|v| v <= 0.0)
        {
            continue;
        }
        ranked.push((gap, who));
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    ranked.into_iter().take(limit).map(|(_, who)| who).collect()
}

/// **The passengers leave with the anchor** (`jump_drive`, after the
/// anchor's own legs): each gets their fatigue, the departure and a landing
/// of their own in the field's ball, and is told; the anchor is told how many
/// went.
#[allow(clippy::too_many_arguments)]
pub fn send_passengers(
    reg: &mut Registry,
    anchor: i64,
    passengers: &[i64],
    field: &Field,
    origin: i64,
    system: i64,
    ly: f64,
    now: BlueTime,
) {
    let mut sent = 0;
    for (i, who) in passengers.iter().enumerate() {
        let Some(there) = reg.roster().get(*who).copied() else {
            continue;
        };
        crate::jump_fatigue::on_jump_at(
            reg,
            *who,
            ly,
            crate::jump_fatigue::ship_multiplier(reg, *who),
            now,
        );
        let spot = crate::jump_drive::landing_spot(
            field,
            crate::seed::mix(
                2,
                &[*who as u64, field.ball_id as u64, now as u64, i as u64],
            ),
        );
        crate::jump_drive::depart(reg, *who, there.ship_id, origin, system, spot);
        reg.push(OnGroupJumpedPassenger {
            character: *who,
            anchor,
            to_system: system,
        });
        sent += 1;
    }
    reg.push(OnGroupJumpAnchorActivated {
        character: anchor,
        to_system: system,
        passengers: sent,
    });
}

/// `OnGroupJumpAnchorActivated(toSolarsystemID, numPassengers)` **[V]**
/// (`cynoTravelSvc.py:19`), to the anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnGroupJumpAnchorActivated {
    pub character: i64,
    pub to_system: i64,
    pub passengers: i64,
}

impl Push for OnGroupJumpAnchorActivated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnGroupJumpAnchorActivated",
            vec![PyValue::Int(self.to_system), PyValue::Int(self.passengers)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// `OnGroupJumpedPassenger(anchorCharID, toSolarsystemID)` **[V]**
/// (`cynoTravelSvc.py:22`), to one passenger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnGroupJumpedPassenger {
    pub character: i64,
    pub anchor: i64,
    pub to_system: i64,
}

impl Push for OnGroupJumpedPassenger {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnGroupJumpedPassenger",
            vec![PyValue::Int(self.anchor), PyValue::Int(self.to_system)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **Close every portal that ended** (`modules::tick`, after the cycles are
/// advanced): the module is no longer running (the cycle ended, the module
/// was offlined, the pilot died or docked), the pilot left the ship, the
/// system or the fleet, or the destination field went out **[A]**.
pub fn tick(w: &mut Registry) {
    if w.ext::<Portals>().open.is_empty() {
        return;
    }
    let gone: Vec<i64> = portals(w)
        .filter(|p| {
            w.modules().running(p.module_id).is_none()
                || w.roster()
                    .get(p.character_id)
                    .is_none_or(|r| r.ship_id != p.ship_id || r.solar_system_id != p.system_id)
                || w.fleets().fleet_of(p.character_id).is_none()
                || cyno::field_by_ball(w, p.beacon_id).is_none()
        })
        .map(|p| p.module_id)
        .collect();
    for module_id in gone {
        close(w, module_id);
    }
}

/// Close one portal now: forget it, stop the generator if it still runs, tell
/// the pilot's fleet. Pushes leave on the clock.
pub fn close(w: &mut Registry, module_id: i64) -> bool {
    let Some(p) = w.ext_mut::<Portals>().open.remove(&module_id) else {
        return false;
    };
    if w.modules().running(module_id).is_some() {
        crate::modules::stop(w, module_id, crate::pyutil::filetime_now(), None);
    }
    for push in w.fleets_mut().close_bridge(p.character_id) {
        w.schedule(Duration::ZERO, push);
    }
    true
}

fn int(req: &Request<'_>, i: usize) -> Option<i64> {
    match req.args.get(i) {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    }
}

fn notify(text: &str) -> ServiceError {
    user_error("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

#[cfg(test)]
mod tests;
