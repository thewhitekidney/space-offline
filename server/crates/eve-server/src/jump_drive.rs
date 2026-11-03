//! **The jump drive** (CAP-E; `docs/lanes/cap-e.md`, `out/research/R-CAP.md`
//! §3.4): a jump-capable hull jumps to a fleet member's lit cynosural field.
//!
//! ## The call **[V]**
//!
//! `cynoTravelSvc.JumpToFleetModuleBeacon(charID, solarsystemID, beaconID)`
//! sends one call, `beyonce.CmdJumpToFleetModuleBeacon(charID, beaconID,
//! solarsystemID)`, through `sessionMgr.PerformSessionChange('jump', ...)`,
//! which discards the answer (`cynoTravelSvc.py:34-41`): the same shape as the
//! stargate's `CmdStargateJump`, so everything the pilot sees is a push, in
//! `crate::jump`'s order -- the departure FX, the ball gone, the session
//! change, the bind. [`cmd_jump_to_fleet_module_beacon`] is the arm, and it
//! reuses `jump::arm_at` for the legs: the destination is a point, not a gate.
//!
//! ## The rules, in the order they refuse
//!
//! 1. in space; not already in a jump (`ShipJumping` [V key]: "Can't navigate
//!    while jumping"); CONCORD's and the weapons timer's refusals as the gate
//!    gives them, and `movement_guard::JUMP_GUARDS`;
//! 2. the hull has a drive: `canJump` (861) on the type
//!    (`sessionChecker.isShipJumpCapable` reads it [V]); else `JumpNoDrive`;
//! 3. the pilot's own cyno is not running: `CantJumpWhileModuleActive{
//!    moduleType}` ("Your jump drive is unusable while <module> is active."
//!    [V text]; CAP-D's `cyno::running_module_on`);
//! 4. the beacon is one the pilot's fleet holds (`fleet.GetActiveBeacons`'
//!    shape, CAP-D) and the system is the one it names, else `NoActiveBeacon`
//!    ("You do not have an active Cynosural Field Beacon." [V text]); not the
//!    pilot's own system (`JumpDestinationInvalid`; the client's menu leaves
//!    the current system out [V], `menusvc._GetCynoTargetSubMenus`);
//! 5. the destination is not high-sec: `CantEnterHighSec{typeID}` ("You cannot
//!    jump the <ship> into high security space (0.5 security and above)." [V
//!    text]; `IsValidJumpTarget` is the client's own test of the same rule);
//! 6. the harmonics: the hull's `jumpDriveTargetBeaconTypelistID` (3317 [V-data])
//!    names a typelist that must admit the field's type, else
//!    `JumpDestinationHarmonicInvalid` ("...mismatched jump harmonics." [V
//!    text]);
//! 7. the activation timer, CAP-F's `activation_remaining`:
//!    `JumpActivationTimer{remaining}` ([V text]: "...require <time> more to
//!    recover from the last jump.");
//! 8. the range: `ly(here, there)` (`celestial::Centers::ly`, the client's
//!    `GetLightYearDistance` [V]) at most the hull's `jumpDriveRange` (867),
//!    **the dogma sheet's modified value**, which is where Jump Drive
//!    Calibration acts (below); else `JumpTooFar{dist, range}` [V keys and
//!    names]. The client's tests are `distance > jumpDriveRange` to leave a
//!    system out (`menusvc.py:2168`) and `distance <= jumpDriveRange` to put
//!    it in (`capitalnavigation.py:81`), so the edge is inclusive;
//! 9. the fuel: `max(int(ly * jumpDriveConsumptionAmount), 1)` units of
//!    `jumpDriveConsumptionType`, the amount again the **modified** attribute
//!    (`markerTooltipUtil._CalculateJumpFuelCostAndType` [V]); the fuel bay
//!    first, then the cargo; `JumpNotEnoughCharge{typeName, available,
//!    required}` [V names].
//!
//! ## The skills are dogma, not code **[V-data]**
//!
//! Jump Fuel Conservation (21610) carries `consumptionQuantityBonusPercentage`
//! (1296) -10 and the effects 3527 (that bonus x its own level) and 3532 (ship
//! `jumpDriveConsumptionAmount` 868 post-percent by it); Jump Drive Calibration
//! (21611) carries `jumpDriveRangeBonus` (870) 20 with 1582 and 1581 (ship 867
//! post-percent). `crate::dogma` applies passive skill effects, so
//! `dogma::sheet(..).ship(867 / 868)` is the number the client's own fitting
//! engine shows -- the number the client's tooltip then multiplies. No skill
//! id appears below.
//!
//! ## What happens, in this order
//!
//! Fuel is taken (one item plan), CAP-F's `on_jump` sets both timers and tells
//! the pilot (`OnJumpTimersUpdated`), the departure FX goes to the jumper and
//! everyone whose park holds the ship, locks drop, the roster presence
//! leaves, the park is forgotten, the ball is removed on [`jump::REMOVE_AFTER`],
//! and the leg is armed at a point within the field's `cynosuralFieldSpawnRadius`
//! (5,000 m [V-data]; the client draws that radius as the landing zone,
//! `tactical.py:701-708`). [`jump::tick`] sends the session change and the
//! bind lands the ship there at rest.
//!
//! ## The departure FX **[V]**
//!
//! `effects.JumpDriveOut` is one of the eight travel effects
//! (`traveleffects.fsdbinary`, `docs/re/jump-gates.md` §2.5). `subwaySvc.
//! OnSpecialFX:388-411` starts the tunnel for the jumper and the departure for
//! everyone else, with `graphicInfo[0]` the destination system exactly as for a
//! gate. `targetID` is **`None`**: the effect's start (`Jump.py`
//! `JumpDriveOut.Start`) hands `ballIDs[1]` to `JumpTransitionCyno(.., gateID)`,
//! which takes `None` (a look animation without a focus) and, given a ball,
//! would take `GetBall(gateID).GetModel()` -- a ball of the *other* system's
//! park, which the destination's field is not in; and `fxSequencer.AddTrigger`
//! ignores a trigger whose target ball is missing. `None` is the one value that
//! works. The jumper is told nothing of the field's position: the tunnel
//! points at the destination system (`graphicInfo[0]`).
//!
//! ## What this does not do (`docs/lanes/cap-e.md`, "Not done")
//!
//! No capacitor (the unit of `jumpDriveCapacitorNeed` is not in the client),
//! no `effects.JumpDriveIn` on arrival, no jump cloak, no spool-up beyond the
//! gate's [`jump::JUMP_DELAY`].
//!
//! ## The conduit (CAP-G)
//!
//! `CmdGroupJumpToFleetModuleBeacon` is this same call with [`Drive::Conduit`]:
//! the hull must name a passenger attribute in 3321, the fuel is its
//! `conduitJumpDriveConsumptionAmount` (3131) where it has one, and the
//! passengers `crate::jump_portal::conduit_passengers` picks leave with the
//! anchor by [`depart`]. Portals (`CmdBridgeToFleetModuleBeacon`,
//! `CmdJumpThroughFleet`) are `crate::jump_portal`'s.

use crate::celestial::{Band, Centers};
use crate::cyno::{self, Field};
use crate::damage::BlueTime;
use crate::fleet::Beacon;
use crate::item::{Edit, Item};
use crate::modules::{type_arg, type_arg_for, user_error};
use crate::service::{Registry, Request, ServiceError};
use eve_marshal::PyValue;

/// `canJump`: the hull has a jump drive **[V]**
/// (`sessionChecker.isShipJumpCapable`).
pub const ATTR_CAN_JUMP: i64 = 861;
/// `jumpDriveConsumptionType`: the isotope the drive burns **[V]**.
pub const ATTR_FUEL_TYPE: i64 = 866;
/// `jumpDriveRange`, light years **[V]**.
pub const ATTR_RANGE: i64 = 867;
/// `jumpDriveConsumptionAmount`, units per light year **[V]**.
pub const ATTR_FUEL_AMOUNT: i64 = 868;
/// `jumpDriveTargetBeaconTypelistID`: the typelist of the field types the
/// drive may jump to **[V-data]** (no client reader; see the lane note).
pub const ATTR_BEACON_TYPELIST: i64 = 3_317;

/// The departure effect's guid **[V]**.
pub const FX_JUMP_DRIVE_OUT: &str = "effects.JumpDriveOut";

/// Group of a cynosural field: normal and industrial 2020, covert 897
/// **[V-data]** (`cyno.rs`'s header). The typelists name types, so the group
/// matters only to a list that names groups.
pub const GROUP_CYNOSURAL_BEACON: i64 = 2_020;
pub const GROUP_COVERT_BEACON: i64 = 897;

/// `beyonce.CmdJumpToFleetModuleBeacon(charID, beaconID, solarsystemID)`.
pub fn cmd_jump_to_fleet_module_beacon(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    jump_to(reg, req, crate::pyutil::filetime_now())
}

/// `beyonce.CmdGroupJumpToFleetModuleBeacon(charID, beaconID, solarsystemID)`
/// **[V]** (`cynoTravelSvc.GroupJumpToFleetModuleBeacon`): the conduit. The
/// same rules as the solo jump, then the passengers `crate::jump_portal`
/// picks go along.
pub fn cmd_group_jump_to_fleet_module_beacon(
    reg: &mut Registry,
    req: &Request<'_>,
) -> Result<PyValue, ServiceError> {
    jump_with(reg, req, crate::pyutil::filetime_now(), Drive::Conduit)
}

/// Which jump this is: the pilot's own, or the conduit that takes fleet
/// members along (CAP-G).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    Solo,
    Conduit,
}

fn jump_to(reg: &mut Registry, req: &Request<'_>, now: BlueTime) -> Result<PyValue, ServiceError> {
    jump_with(reg, req, now, Drive::Solo)
}

fn jump_with(
    reg: &mut Registry,
    req: &Request<'_>,
    now: BlueTime,
    drive: Drive,
) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(notify("No character on this session."));
    };
    let (Some(owner), Some(beacon_id), Some(system)) = (int(req, 0), int(req, 1), int(req, 2))
    else {
        return Err(notify(
            "CmdJumpToFleetModuleBeacon needs (charID, beaconID, solarsystemID).",
        ));
    };
    // In the tunnel the pilot is out of the roster, so this comes first.
    if reg.jumps().contains(actor) {
        return Err(user_error("ShipJumping", vec![]));
    }
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(notify("You are not in space."));
    };
    crate::concord::refuse_beacon_jump(reg, Some(actor), now)?;
    crate::crimewatch::refuse_jump(reg, Some(actor), now)?;
    if let Some(refusal) =
        crate::movement_guard::check(&crate::module::plan().jump_guards, reg, actor)
    {
        return Err(refusal.into_error());
    }
    let ship_id = here.ship_id;
    let ship_type = ship_type_of(reg, ship_id, actor);
    if attribute(reg, ship_id, ship_type, ATTR_CAN_JUMP).is_none_or(|v| v <= 0.0) {
        return Err(user_error("JumpNoDrive", vec![]));
    }
    // The conduit needs a hull that names who may ride
    // (`sessionChecker.canPerformGroupJump` reads 3321 off the ship **[V]**).
    let conduit_attribute = if drive == Drive::Conduit {
        let required = attribute_or_default(
            reg,
            ship_id,
            ship_type,
            crate::jump_portal::ATTR_CONDUIT_REQUIRED,
        )
        .map_or(0, |v| v as i64);
        if required <= 0 {
            return Err(notify("Your ship cannot open a conduit field."));
        }
        Some(required)
    } else {
        None
    };
    if let Some(module_type) = cyno::running_module_on(reg, ship_id) {
        return Err(user_error(
            "CantJumpWhileModuleActive",
            vec![(
                "moduleType",
                type_arg_for("CantJumpWhileModuleActive", "moduleType", module_type),
            )],
        ));
    }
    let origin = here.solar_system_id;
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
    if !harmonics_match(reg, ship_id, ship_type, beacon.type_id) {
        return Err(user_error("JumpDestinationHarmonicInvalid", vec![]));
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
    let range = attribute(reg, ship_id, ship_type, ATTR_RANGE).unwrap_or(0.0);
    if ly > range {
        return Err(user_error(
            "JumpTooFar",
            vec![("dist", PyValue::Real(ly)), ("range", PyValue::Real(range))],
        ));
    }
    let fuel_type = attribute(reg, ship_id, ship_type, ATTR_FUEL_TYPE).map_or(0, |v| v as i64);
    let per_ly = drive_per_ly(reg, ship_id, ship_type, drive);
    let required = fuel_required(ly, per_ly);
    let stacks = fuel_stacks(reg, actor, ship_id, fuel_type);
    let available: i64 = stacks.iter().map(|s| s.quantity).sum();
    let Some(edits) = fuel_plan(&stacks, required) else {
        return Err(user_error(
            "JumpNotEnoughCharge",
            vec![
                (
                    "typeName",
                    type_arg_for("JumpNotEnoughCharge", "typeName", fuel_type),
                ),
                ("available", PyValue::Int(available)),
                ("required", PyValue::Int(required)),
            ],
        ));
    };
    // The field must still be there to land at: it goes with its pilot's
    // module, and the beacon list and the field are cleared on one clock.
    let Some(field) = cyno::field_by_ball(reg, beacon.beacon_id) else {
        return Err(user_error("NoActiveBeacon", vec![]));
    };
    // Who rides: chosen before anything is paid, so the count is the one
    // the anchor is told.
    let passengers = match conduit_attribute {
        Some(required_attribute) => {
            let limit = attribute(
                reg,
                ship_id,
                ship_type,
                crate::jump_portal::ATTR_CONDUIT_COUNT,
            )
            .map_or(0, |v| v as usize);
            crate::jump_portal::conduit_passengers(
                reg,
                actor,
                origin,
                system,
                required_attribute,
                limit,
                now,
            )
        }
        None => Vec::new(),
    };

    // **Nothing refuses from here.** Fuel first, in one plan.
    if let Err(e) = reg.apply_items(&edits) {
        crate::errln!("[jump-drive] fuel for {actor} not taken: {e:?}");
        return Err(user_error("JumpDestinationBlocked", vec![]));
    }
    crate::jump_fatigue::on_jump(reg, actor, ly);
    let spot = landing_spot(
        &field,
        crate::seed::mix(0, &[actor as u64, field.ball_id as u64, now as u64]),
    );
    depart(reg, actor, ship_id, origin, system, spot);
    if drive == Drive::Conduit {
        crate::jump_portal::send_passengers(
            reg,
            actor,
            &passengers,
            &field,
            origin,
            system,
            ly,
            now,
        );
    }
    crate::outln!(
        "[jump-drive] character {actor} ship {ship_id}: {origin} -> {system} to {owner}'s field \
         {}, {ly:.3} ly, {required} of type {fuel_type}{}",
        field.ball_id,
        if drive == Drive::Conduit {
            format!(", {} passengers", passengers.len())
        } else {
            String::new()
        }
    );
    Ok(PyValue::None)
}

/// The drive's fuel per light year as the pilot's ship shows it: the
/// conduit's own `conduitJumpDriveConsumptionAmount` (3131) when the hull
/// carries one, else `jumpDriveConsumptionAmount` (868).
fn drive_per_ly(reg: &Registry, ship_id: i64, ship_type: i64, drive: Drive) -> f64 {
    let solo = attribute(reg, ship_id, ship_type, ATTR_FUEL_AMOUNT).unwrap_or(0.0);
    if drive == Drive::Solo {
        return solo;
    }
    attribute(
        reg,
        ship_id,
        ship_type,
        crate::jump_portal::ATTR_CONDUIT_FUEL_AMOUNT,
    )
    .filter(|v| *v > 0.0)
    .unwrap_or(solo)
}

/// **A ship leaves for a field in another system**: the departure FX to the
/// pilot and everyone whose park holds the ship, locks dropped, the roster
/// presence gone, the park forgotten, the ball removed on
/// [`jump::REMOVE_AFTER`], and the leg armed at `spot`. The same legs for a
/// pilot's own jump, a conduit's passengers and a portal's.
pub fn depart(
    reg: &mut Registry,
    actor: i64,
    ship_id: i64,
    origin: i64,
    system: i64,
    spot: [f64; 3],
) {
    let mut audience = vec![actor];
    audience.extend(
        reg.roster()
            .others_in_system(origin, actor)
            .into_iter()
            .filter(|id| reg.character(*id).is_some()),
    );
    let stamp = reg.next_destiny_stamp();
    reg.push(JumpDriveFx {
        to: audience.clone(),
        stamp,
        ship: ship_id,
        dest_system: system,
    });
    crate::targeting::left_space(
        reg,
        actor,
        crate::targeting::Reason::Unlocked,
        crate::targeting::Delivery::WithAnswer,
    );
    let held = park_held(reg, actor, ship_id);
    reg.roster_mut().leave(actor);
    crate::grid::forget(reg, actor);
    schedule_departure_removal(reg, stamp, ship_id, held, audience);
    crate::jump::arm_at(reg, actor, origin, system, spot);
}

/// **What the jumper's own park holds** besides its ship, read before the
/// park is forgotten (`crate::grid::forget`). Empty for a pilot whose park is
/// not modelled.
pub(crate) fn park_held(reg: &Registry, actor: i64, ship_id: i64) -> Vec<i64> {
    reg.ext::<crate::grid::Parks>()
        .of(actor)
        .map(|park| park.iter().copied().filter(|b| *b != ship_id).collect())
        .unwrap_or_default()
}

/// **A departure's `RemoveBalls`**, on [`jump::REMOVE_AFTER`]; `audience` has
/// the jumper first, as a stargate, a wormhole and a jump drive all build it.
///
/// **The jumper's window of 2 s with no ego (S8).** Its ball goes at
/// [`jump::REMOVE_AFTER`] and the session changes 2 s later, and the
/// overview polls twice a second in between: `overviewScrollEntry.
/// _GetSurfaceDistance` reads the ego ball, finds `None`, and raises on
/// every row. So in the stamp of the ego's removal the jumper is also told
/// to drop every other ball it holds (`held`): with no ball, `node.ball()` is
/// `None` and the entry answers its `rawDistance` instead. The jumper only;
/// the others lose the ship alone. **[A]** that the client takes
/// `RemoveBalls` of its whole park, ego last, without a broken tunnel;
/// falsifier: an empty or glitching subway in `fci_needlejack` /
/// `fci_wh_range` / a stargate scenario, or the overview exception still
/// logged. A pilot whose park is not modelled holds nothing here and is sent
/// the ship alone, as before.
pub(crate) fn schedule_departure_removal(
    reg: &mut Registry,
    stamp: u32,
    ship_id: i64,
    held: Vec<i64>,
    mut audience: Vec<i64>,
) {
    let to_others = audience.split_off(1.min(audience.len()));
    let mut dropped = held;
    dropped.push(ship_id);
    if let Some(removed) = crate::destiny::BallsRemoved::new(audience, stamp, dropped) {
        reg.schedule(crate::jump::REMOVE_AFTER, removed);
    }
    if let Some(removed) = crate::destiny::BallsRemoved::new(to_others, stamp, vec![ship_id]) {
        reg.schedule(crate::jump::REMOVE_AFTER, removed);
    }
}

/// The beacon `owner` has lit that `actor`'s fleet holds, if it is the one
/// `beacon_id` in `system` names.
pub(crate) fn held_beacon(
    reg: &Registry,
    actor: i64,
    owner: i64,
    beacon_id: i64,
    system: i64,
) -> Option<Beacon> {
    reg.fleets()
        .beacons_visible_to(actor)
        .into_iter()
        .find(|(who, b)| *who == owner && b.beacon_id == beacon_id && b.solar_system_id == system)
        .map(|(_, b)| b)
}

/// The type of the ship `actor` flies: the item's, else the character row's.
pub(crate) fn ship_type_of(reg: &Registry, ship_id: i64, actor: i64) -> i64 {
    reg.item(ship_id)
        .map(|i| i.type_id)
        .or_else(|| reg.character(actor).map(|c| c.ship_type_id))
        .unwrap_or(0)
}

/// A ship attribute as the pilot's client would show it: the dogma sheet's
/// modified value when the world has a sheet for the hull and the sheet
/// carries the attribute, else the type's own.
pub(crate) fn attribute(
    reg: &Registry,
    ship_id: i64,
    ship_type: i64,
    attribute_id: i64,
) -> Option<f64> {
    crate::typedata::attribute_in(reg.content(), ship_type, attribute_id)?;
    crate::dogma::sheet(reg, ship_id)
        .map(|s| s.ship(attribute_id))
        .or_else(|| crate::typedata::attribute_in(reg.content(), ship_type, attribute_id))
}

/// A ship attribute whose `defaultValue` matters: the sheet's modified value
/// when the hull's row names the attribute or a modifier reaches it, else the
/// attribute's default.
///
/// [`attribute`] is right for the attributes every drive hull sets. The
/// passenger attributes are the other kind: 3319 and 5682 default to 1 and
/// only the hulls that cannot ride set them to 0, so a hull with no row must
/// read as its default, which the sheet (it holds only what a row, a modifier
/// or its reads name) does not say. `None` for a type the table lacks.
pub(crate) fn attribute_or_default(
    reg: &Registry,
    ship_id: i64,
    ship_type: i64,
    attribute_id: i64,
) -> Option<f64> {
    let typed = crate::typedata::attribute_in(reg.content(), ship_type, attribute_id)?;
    let in_row = crate::typedata::installed_in(reg.content())
        .table()
        .get(ship_type)
        .and_then(|r| r.attribute(attribute_id))
        .is_some();
    match crate::dogma::sheet(reg, ship_id).map(|s| s.ship(attribute_id)) {
        Some(v) if in_row || v != 0.0 => Some(v),
        _ => Some(typed),
    }
}

/// Whether the hull's typelist admits the field's type. A hull with no list,
/// a list the table does not hold, or no typelist table at all admits any
/// field: nothing then says otherwise.
pub fn harmonics_match(reg: &Registry, ship_id: i64, ship_type: i64, field_type: i64) -> bool {
    let Some(list) = attribute(reg, ship_id, ship_type, ATTR_BEACON_TYPELIST)
        .map(|v| v as i64)
        .filter(|l| *l > 0)
    else {
        return true;
    };
    let Some(table) = reg.content().try_get::<crate::dungeon_data::Table>() else {
        return true;
    };
    let Some(list) = table.typelists.get(&list) else {
        return true;
    };
    let group = if field_type == cyno::TYPE_COVERT_CYNOSURAL_FIELD {
        GROUP_COVERT_BEACON
    } else {
        GROUP_CYNOSURAL_BEACON
    };
    list.contains(field_type, group, 0)
}

/// What a jump of `ly` costs: `max(long(ly * perLY), 1)` **[V]**
/// (`markerTooltipUtil._CalculateJumpFuelCostAndType`). `long()` truncates.
pub fn fuel_required(ly: f64, per_ly: f64) -> i64 {
    ((ly * per_ly) as i64).max(1)
}

/// The pilot's stacks of the fuel in the ship's fuel bay and cargo, bay first
/// (the order `cyno::FUEL_FLAGS` gives the ozone), each flag by item id.
pub(crate) fn fuel_stacks(reg: &Registry, actor: i64, ship_id: i64, fuel: i64) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::new();
    for flag in cyno::FUEL_FLAGS {
        let mut here: Vec<Item> = reg
            .store()
            .items()
            .iter()
            .filter(|i| {
                i.location_id == ship_id
                    && i.flag_id == flag
                    && i.type_id == fuel
                    && i.owner_id == actor
                    && i.quantity > 0
            })
            .cloned()
            .collect();
        here.sort_by_key(|i| i.item_id);
        out.extend(here);
    }
    out
}

/// The edits that take `required` units out of `stacks` in order, whole
/// stacks destroyed and the last one consumed in part; `None` when the stacks
/// hold less.
pub fn fuel_plan(stacks: &[Item], required: i64) -> Option<Vec<Edit>> {
    let mut left = required;
    let mut edits = Vec::new();
    for s in stacks {
        if left <= 0 {
            break;
        }
        if s.quantity <= left {
            edits.push(Edit::Destroy { item_id: s.item_id });
            left -= s.quantity;
        } else {
            edits.push(Edit::Consume {
                item_id: s.item_id,
                quantity: left,
            });
            left = 0;
        }
    }
    (left <= 0).then_some(edits)
}

/// Where a ship lands: uniform in the ball of `cynosuralFieldSpawnRadius`
/// round the field **[V-data]**, drawn from `seed`. A radius of zero lands on
/// the field's position.
pub fn landing_spot(field: &Field, seed: u64) -> [f64; 3] {
    let mut s = crate::seed::Stream::new(seed);
    // Rejection in the unit cube; thirty-two tries leave nothing (one in
    // 10^15), and the last resort is a point on the axis.
    let mut unit = [1.0, 0.0, 0.0];
    for _ in 0..32 {
        let p = [
            s.unit() * 2.0 - 1.0,
            s.unit() * 2.0 - 1.0,
            s.unit() * 2.0 - 1.0,
        ];
        if p[0] * p[0] + p[1] * p[1] + p[2] * p[2] <= 1.0 {
            unit = p;
            break;
        }
    }
    [
        field.position[0] + unit[0] * field.spawn_radius,
        field.position[1] + unit[1] * field.spawn_radius,
        field.position[2] + unit[2] * field.spawn_radius,
    ]
}

/// The departure: `jump::JumpFx`'s fourteen positionals with the jump
/// drive's guid and **no target** (see the module header).
#[derive(Debug, Clone, PartialEq)]
pub struct JumpDriveFx {
    pub to: Vec<i64>,
    pub stamp: u32,
    pub ship: i64,
    pub dest_system: i64,
}

impl crate::notify::Push for JumpDriveFx {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let args = vec![
            PyValue::Int(self.ship),
            // moduleID, moduleTypeID: `None` scatters once (`Park.OnSpecialFX`).
            PyValue::None,
            PyValue::None,
            // targetID: none, see the header.
            PyValue::None,
            PyValue::None,
            crate::pyutil::bstr(FX_JUMP_DRIVE_OUT),
            // isOffensive, start, active.
            PyValue::Int(0),
            PyValue::Int(1),
            PyValue::Int(0),
            // duration, repeat, startTime, timeFromStart: the signature's
            // own defaults.
            PyValue::Int(-1),
            PyValue::None,
            PyValue::None,
            PyValue::Int(0),
            // graphicInfo: `[0]` is where the tunnel goes.
            PyValue::Tuple(vec![PyValue::Int(self.dest_system)]),
        ];
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSpecialFX"),
                PyValue::Tuple(args),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }

    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship])
    }
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
