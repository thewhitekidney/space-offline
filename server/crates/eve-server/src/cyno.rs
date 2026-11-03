//! **The cynosural field** (CAP-D; lane note `docs/lanes/cap-d.md`, spec
//! R-CAP §1.4, §1.5, §3.3).
//!
//! A pilot with a Cynosural Field Generator online runs it
//! (`dogmaIM.Activate(moduleID, 'cynosuralGeneration', ...)`, effect 2857,
//! category 1, duration attribute 73 **[V-data]**). While it runs the pilot's
//! ship carries a **field**: a ball that never moves, in the pilot's grid,
//! which the pilot's fleet is told of with `OnFleetJumpBeaconModuleChange`
//! and which a jump drive (CAP-E) can aim at.
//!
//! # What is the client's, and what is ours
//!
//! The client checks nothing before the call: `smb` sends `Activate` for any
//! activation-category effect, and the dogma attributes the cyno modules
//! carry have **no client reader** but the tooltip's grouping
//! (`dogma/attributes/groups`) and, for `disallowEarlyDeactivation` (906), the
//! fitting screen's cloak check (`pythonEffects.FakeCloaking.PreStartChecks:
//! 157`) **[V]**. So every refusal is the server's, in the client's own keys
//! and argument names (the keys are in `findings/user-errors.tsv`, the words
//! and token names in the localisation corpus):
//!
//! | rule | key | args | source |
//! |---|---|---|---|
//! | no fleet | `CynoMustBeInFleet` | none | **[V]** text "You must be in a fleet to activate your Cynosural Field." |
//! | in warp | `DeniedActivateInWarp` | none | **[V]** key and text; the rule is `disallowActivateOnWarp` 1245 on all three modules **[V-data]** |
//! | high-sec | `CantInHighSecSpace` | none | **[V]** key (`moduleRestrictions.py:34`, which tests attribute 1970, absent on every cyno module **[V-data]**), so **[A]** that retail raises it for a cyno |
//! | wormhole | `JumpBeaconInWormholeSpace` | none | **[V]** key and text; **[A]** that retail raises it when lighting |
//! | too fast | `VelocityTooHighToActivate` | `type`, `shipVelocity`, `velocityLimit` | **[V]** text and token names; the limit is `maxVelocityActivationLimit` 1334 = 500 **[V-data]** |
//! | a second cyno | `EffectCrowdedOut` | `module` (type id), `count` | **[V]** `fittingDogmaLocation._CheckIfTooManyModulesInGroupAreActive:641`, `maxGroupActive` 763 = 1 **[V-data]** |
//! | no fuel | `ModuleRequiresFuel` | `fuel`, `fueled` (names) | **[V]** text: "... enough of the necessary fuel in a single stack" |
//! | dock | `CantDockWhileModuleActive` | `moduleType` | **[V]** key and arg; the rule is `disallowDocking` 2354 **[V-data]** |
//!
//! # Immobility is data, not code
//!
//! Effect 2857 carries three modifiers on the ship **[V-data]**: `maxVelocity`
//! post-percent by the module's `speedFactor` (-100), `warpScrambleStatus`
//! (104) plus the module's `siegeModeWarpStatus` (852 = 100), and `canCloak`
//! assigned 0. `crate::dogma` applies a running module's default effect, so
//! the ship's top speed falls to 0 (`modules::speed_changed` tells the park:
//! [`crate::modules`] treats the cyno as a speed-changing effect) and
//! `crate::tackle::scrammed` refuses every warp, with nothing added here.
//!
//! # The fuel
//!
//! Liquid Ozone (`consumptionType` 713 = 16273), `consumptionQuantity` 714 a
//! cycle (500 / 50 / 800 for the normal, covert and industrial module)
//! **[V-data]**, paid **at the start of every cycle**, the first at the click
//! (the module's `repeat` is the client's 1000, so a cycle is a field's
//! lifetime and a refill is the next ten minutes). The text of the refusal
//! says the fuel must be **in a single stack**, so a stack that holds a
//! cycle's worth is looked for: the ship's fuel bay (flag 133) first, then
//! the cargo hold **[A]** for that order. A cycle that cannot be paid stops
//! the module, with `ModuleRequiresFuel` as the stop's error, and the field
//! goes with it.
//!
//! # The field
//!
//! An item of the field type (21094 normal, 28650 covert, 52696 industrial
//! **[V]** `fleetSvc.py:98-101`), picked by the module's flags: `isCovert`
//! 1252 and `isIndustrialCyno` 2826 are on the module and on its field
//! **[V-data]**, so **[I]** the module makes the field with the same flag.
//! It is **not a store item** here: the field types are not in the bundle's
//! type table (typedata has no row for them), so an item could not be made,
//! and a restart empties space anyway (the spec's "restart: gone, fuel lost").
//! Its id is minted from [`FIELD_ID_BASE`]. The ball is a body at rest
//! (`Mode::Stop`, no mass, not free), as a hub's or a wreck's is, radius
//! [`FIELD_RADIUS`] 250 m **[V-data]**, carrying the pilot's corporation and
//! alliance. The client draws groups 2020 and 897 as a
//! `LargeCollidableObject` (`spaceObject/repository.py:138-140`) **[V]**. The
//! slim is the five ordinary attributes: the pilot as owner, their corp, **no
//! `charID`** **[A]**: `charID` is how a ship's pilot is primed, and the
//! field is no ship; falsified by an overview that shows the field without
//! its owner.
//!
//! It is a **local** ball: in [`balls_in`], which `grid::local_balls` and the
//! scene list, so a pilot on the grid is sent it and one who warps in gets it
//! with the grid.
//!
//! # State
//!
//! The fields live in the world slot [`SLOT`], and in nothing persisted. The
//! fleet's own list of lit pilots is `crate::fleet::Fleets::beacons`.
//! The field lasts as long as the module runs: [`tick`], run from
//! `modules::tick`, takes down the field of a module that stopped for any
//! reason (a cycle that ended, an offlined module, a dead or docked pilot).

use crate::destiny::{Ball, FLAGS_STATIC_BALL, Mode, NO_FORMATION, Slim};
use crate::fleet::Beacon;
use crate::item::{Edit, Item};
use crate::modules::{ModuleType, Running, type_arg, type_arg_for, user_error};
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `effectCynosuralGeneration` **[V]** (`dogma/const`, `dogmaeffects[2857]`):
/// category 1, duration attribute 73, no guid **[V-data]**.
pub const EFFECT_CYNOSURAL_GENERATION: i64 = 2_857;
/// The name the client sends to `Activate` **[V-data]**.
pub const EFFECT_NAME: &str = "cynosuralGeneration";

/// `consumptionType` and `consumptionQuantity` **[V]** (`dogma/const`).
pub const ATTR_FUEL_TYPE: i64 = 713;
pub const ATTR_FUEL_QUANTITY: i64 = 714;
/// `maxGroupActive` **[V]**.
pub use crate::modules::ATTR_MAX_GROUP_ACTIVE;
/// `isCovert`, `maxVelocityActivationLimit`, `cynosuralFieldSpawnRadius` and
/// `isIndustrialCyno` **[V]** (`dogma/const`).
pub const ATTR_IS_COVERT: i64 = 1_252;
pub const ATTR_MAX_VELOCITY_ACTIVATION_LIMIT: i64 = 1_334;
pub const ATTR_SPAWN_RADIUS: i64 = 2_455;
pub const ATTR_IS_INDUSTRIAL_CYNO: i64 = 2_826;

/// The three field types **[V]** (`inventorycommon/const`, named in
/// `fleetSvc.cynoTypeNames`).
pub const TYPE_CYNOSURAL_FIELD: i64 = 21_094;
pub const TYPE_COVERT_CYNOSURAL_FIELD: i64 = 28_650;
pub const TYPE_INDUSTRIAL_CYNOSURAL_FIELD: i64 = 52_696;

/// The fields' radius, metres: every field type's `radius` **[V-data]**.
pub const FIELD_RADIUS: f64 = 250.0;

/// `flagCargo` and `flagSpecializedFuelBay` **[V]** (`invConst`).
pub const FLAG_CARGO: i64 = 5;
pub const FLAG_FUEL_BAY: i64 = 133;
/// Where a cycle's fuel is looked for, the bay first **[A]**. Falsified by a
/// retail ship that spends cargo ozone while its bay holds some.
pub const FUEL_FLAGS: [i64; 2] = [FLAG_FUEL_BAY, FLAG_CARGO];

/// Field ids come from here: above the missile range
/// (`missile::ID_END`) and below `u64`'s fakes **[A]** (no other range here;
/// `field_ids_clear_of_every_other_range` lists the ones known). Falsified by
/// an id that two systems mint.
pub const FIELD_ID_BASE: i64 = 9_800_000_000_000;
pub const FIELD_ID_END: i64 = 9_900_000_000_000;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One lit field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    /// The field's ball and slim id, [`FIELD_ID_BASE`] up.
    pub ball_id: i64,
    pub character_id: i64,
    pub ship_id: i64,
    pub module_id: i64,
    pub module_type_id: i64,
    /// The field's type: 21094, 28650 or 52696.
    pub type_id: i64,
    pub system_id: i64,
    pub position: [f64; 3],
    /// `cynosuralFieldSpawnRadius` of the module: how far from the field a
    /// jump may land a ship (5000 m **[V-data]**). For CAP-E.
    pub spawn_radius: f64,
    pub corporation_id: i64,
    pub alliance_id: Option<i64>,
}

/// Every lit field, by the module that makes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cynos {
    fields: BTreeMap<i64, Field>,
    /// Ids already minted, so a wrap skips one still lit.
    next: i64,
}

impl Cynos {
    fn alloc(&mut self) -> i64 {
        let span = FIELD_ID_END - FIELD_ID_BASE;
        loop {
            let id = FIELD_ID_BASE + self.next % span;
            self.next += 1;
            if !self.fields.values().any(|f| f.ball_id == id) {
                return id;
            }
        }
    }
}

fn make_slot(_: &crate::content::Content) -> Boxed {
    Box::new(Cynos::default())
}

/// The world slot (coordinator: `world_ext::SLOTS`).
pub const SLOT: SlotSpec = SlotSpec {
    name: "cyno",
    make: make_slot,
};

/// Whether `effect_id` is the cynosural field effect: the one the module
/// hooks test.
pub fn is_cyno(effect_id: i64) -> bool {
    effect_id == EFFECT_CYNOSURAL_GENERATION
}

// ---------------------------------------------------------------------------
// The Mobile Cynosural Inhibitor (NW-4, issue #177)
// ---------------------------------------------------------------------------
//
// `groupCynoInhibitor` 1249, a `DeployableSpaceObject` [V]. It is launched
// like a mobile disruptor (`crate::anchor`) but has no anchor effect [V], so
// it activates by itself `activate` seconds later (its `activate` component,
// "one minute"), and it self-destructs one `decay` after that ("one hour of
// operation"; loc 292384) and "cannot be retrieved once deployed". Once
// active it refuses every non-covert cyno within its `cynoInhibitor.range`
// ("100 km"), with `CynosuralGenerationInhibited{range, type}` (loc 500808).
// A covert field is not affected; nothing is refused before it activates.
// The numbers are `deploy/units.tsv`'s `inhibit_range` row
// (`crate::mtu::inhibitor`, `tools/content_deploy.py`).
//
// - **[A]** the industrial cyno (`isIndustrialCyno`) is not covert and is
//   refused. Falsifier: TQ, or the rest of the loc text.
// - **[A]** the inhibitor is refused only at *activation*; a field already
//   lit stays until its own cycle ends. Falsifier: the client or TQ.
// - **[A]** the range is measured from the cyno ship to the inhibitor's
//   centre, and the inhibitor's own `decay` (else one hour) is its life
//   after activation. Falsifier: `fci_cyno_inhibit`, or a `decay_s` of 0.
// - Placement is data, not code: `deploy.minDistanceFromOwnGroup` (spacing),
//   `minDistanceFromStargatesAndStations` (gates, stations and Upwell
//   structures) and `maxSecurityLevel` (a launch above it is refused) are
//   columns of the inhibitor row. **[A]** the key `maxSecurityLevel` is
//   named in the NW-4 fix brief, unread in the client's code; falsifier: the
//   regenerated row has it empty. The 40 km Starbase rule is out (no POS).

/// `groupCynoInhibitor` [V] (`inventorycommon/const.py:1053`).
pub const GROUP_INHIBITOR: i64 = 1_249;
/// One hour, blue ticks: the life after activation when the row gives none.
const HOUR: crate::damage::BlueTime = 3_600 * 10_000_000;

/// The inhibitor row of `group_id`, its decay filled in.
pub fn inhibitor_spec(w: &Registry, group_id: i64) -> Option<crate::mtu::Inhibitor> {
    let mut i = crate::mtu::inhibitor(w.content(), group_id)?;
    if i.decay == 0 {
        i.decay = HOUR;
    }
    Some(i)
}

/// Why an inhibitor `spec` may not be put at `at` in `system`, or `Ok`. The
/// distances and the security ceiling are the row's (`deploy` component).
pub fn placement_refusal(
    w: &Registry,
    spec: &crate::mtu::Inhibitor,
    system: i64,
    at: [f64; 3],
) -> Result<(), String> {
    let near = |p: [f64; 3], m: f64| m > 0.0 && crate::space_state::distance(p, at) < m;
    if let Some(max) = spec.max_security
        && w.content()
            .get::<crate::celestial::Universe>()
            .system(system)
            .is_some_and(|s| s.security > max)
    {
        return Err("That cannot be launched in a system this secure.".to_owned());
    }
    if crate::anchor::all_in(w, system).iter().any(|u| {
        crate::scan::group_of(w.content(), u.type_id) == Some(GROUP_INHIBITOR)
            && near(u.centre, spec.spacing)
    }) {
        return Err("That cannot be launched that close to another one.".to_owned());
    }
    // Gates and stations, and (the client's type text names them) Upwell
    // structures.
    if w.celestials().served_in(system).any(|c| {
        matches!(
            c.kind,
            crate::celestial::Kind::Gate | crate::celestial::Kind::Station
        ) && near(c.position, spec.from_gate)
    }) || crate::structure::in_system(w.deltas(), system)
        .iter()
        .any(|s| near(s.pos, spec.from_gate))
    {
        return Err(
            "That cannot be launched that close to a gate, station or structure.".to_owned(),
        );
    }
    Ok(())
}

/// The active inhibitor that bars a cyno at `at` in `system`: its range and
/// type. An inhibitor is active once anchored (its activation time over).
pub fn inhibited_by(w: &Registry, system: i64, at: [f64; 3]) -> Option<(f64, i64)> {
    crate::anchor::anchored(w, system)
        .into_iter()
        .find_map(|u| {
            if crate::scan::group_of(w.content(), u.type_id)? != GROUP_INHIBITOR {
                return None;
            }
            let range = inhibitor_spec(w, GROUP_INHIBITOR)?.range;
            (crate::space_state::distance(u.centre, at) <= range).then_some((range, u.type_id))
        })
}

/// Every field, by module id.
pub fn fields(w: &Registry) -> impl Iterator<Item = &Field> {
    w.ext::<Cynos>().fields.values()
}

/// The field a ball id names.
pub fn field_by_ball(w: &Registry, ball_id: i64) -> Option<Field> {
    fields(w).find(|f| f.ball_id == ball_id).copied()
}

/// The field a pilot's ship carries.
pub fn field_of_ship(w: &Registry, ship_id: i64) -> Option<Field> {
    fields(w).find(|f| f.ship_id == ship_id).copied()
}

/// The type id of the cyno module running on `ship_id`, for the jump and
/// dock guards (`CantJumpWhileModuleActive{moduleType}`, CAP-E/J).
pub fn running_module_on(w: &Registry, ship_id: i64) -> Option<i64> {
    field_of_ship(w, ship_id).map(|f| f.module_type_id)
}

// ---------------------------------------------------------------------------
// Reading the module
// ---------------------------------------------------------------------------

/// An attribute of a fitted cyno module: the dogma sheet's modified value,
/// else the type's own, else the module row's.
fn module_value(w: &Registry, ship_id: i64, module_id: i64, type_id: i64, a: i64) -> Option<f64> {
    crate::dogma::module_attribute(w, ship_id, module_id, a)
        .filter(|v| *v != 0.0)
        .or_else(|| crate::typedata::attribute_in(w.content(), type_id, a))
        .or_else(|| crate::modules::find_in(w.content(), type_id).and_then(|t| t.attribute(a)))
}

/// The field type a module makes: the covert module's 28650, the
/// industrial's 52696, the plain 21094 **[I]** (see the module header).
pub fn field_type_for(w: &Registry, module_type_id: i64) -> i64 {
    let flag = |a: i64| {
        crate::typedata::attribute_in(w.content(), module_type_id, a).is_some_and(|v| v > 0.0)
            || crate::modules::find_in(w.content(), module_type_id)
                .and_then(|t| t.attribute(a))
                .is_some_and(|v| v > 0.0)
    };
    if flag(ATTR_IS_INDUSTRIAL_CYNO) {
        TYPE_INDUSTRIAL_CYNOSURAL_FIELD
    } else if flag(ATTR_IS_COVERT) {
        TYPE_COVERT_CYNOSURAL_FIELD
    } else {
        TYPE_CYNOSURAL_FIELD
    }
}

/// A cycle's fuel: `(type, quantity)`, or `None` for a module that names
/// none (it then burns none).
fn fuel_of(w: &Registry, module_type_id: i64) -> Option<(i64, i64)> {
    let read = |a: i64| {
        crate::typedata::attribute_in(w.content(), module_type_id, a)
            .or_else(|| {
                crate::modules::find_in(w.content(), module_type_id).and_then(|t| t.attribute(a))
            })
            .map(|v| v.round() as i64)
    };
    let ty = read(ATTR_FUEL_TYPE).filter(|t| *t > 0)?;
    let qty = read(ATTR_FUEL_QUANTITY).filter(|q| *q > 0).unwrap_or(1);
    Some((ty, qty))
}

/// The one stack that pays a cycle: the pilot's own, in the ship's fuel bay
/// or cargo hold, holding at least `qty`; the bay before the cargo, the
/// smallest sufficient stack first.
fn fuel_stack(w: &Registry, character_id: i64, ship_id: i64, fuel: i64, qty: i64) -> Option<Item> {
    FUEL_FLAGS.iter().find_map(|flag| {
        w.store()
            .items()
            .iter()
            .filter(|i| {
                i.location_id == ship_id
                    && i.flag_id == *flag
                    && i.type_id == fuel
                    && i.owner_id == character_id
                    && i.quantity >= qty
            })
            .min_by_key(|i| (i.quantity, i.item_id))
            .cloned()
    })
}

fn type_name(w: &Registry, type_id: i64) -> String {
    crate::item::find_in(w.content(), type_id)
        .map_or_else(|| format!("type {type_id}"), |t| t.name.to_owned())
}

fn fuel_error(
    w: &Registry,
    module_type_id: i64,
    fuel: i64,
) -> (String, Vec<(&'static str, PyValue)>) {
    (
        "ModuleRequiresFuel".to_owned(),
        vec![
            ("fuel", crate::pyutil::bstr(&type_name(w, fuel))),
            ("fueled", crate::pyutil::bstr(&type_name(w, module_type_id))),
        ],
    )
}

// ---------------------------------------------------------------------------
// Activation
// ---------------------------------------------------------------------------

/// What `modules::activate` asks before a cyno starts (the `if
/// is_cyno(effect.effect_id)` arm, in `modules.rs`), in this order:
/// fleet, warp, security, speed, group, fuel. See the module header for each
/// key. A pilot not in space has already been refused by the caller.
pub fn check_activation(
    w: &mut Registry,
    character_id: i64,
    ship_id: i64,
    module: &ModuleType,
    module_id: i64,
) -> Result<(), ServiceError> {
    if w.fleets().fleet_of(character_id).is_none() {
        return Err(user_error("CynoMustBeInFleet", vec![]));
    }
    w.advance_character(character_id);
    let Some(here) = w.roster().get(character_id).copied() else {
        return Ok(());
    };
    if here.motion.warp_active() {
        return Err(user_error("DeniedActivateInWarp", vec![]));
    }
    match w
        .content()
        .get::<crate::celestial::Universe>()
        .band(here.solar_system_id)
    {
        Some(crate::celestial::Band::High) => {
            return Err(user_error("CantInHighSecSpace", vec![]));
        }
        Some(crate::celestial::Band::Wormhole) => {
            return Err(user_error("JumpBeaconInWormholeSpace", vec![]));
        }
        _ => {}
    }
    if field_type_for(w, module.type_id) != TYPE_COVERT_CYNOSURAL_FIELD
        && let Some((range, inhibitor)) =
            inhibited_by(w, here.solar_system_id, here.motion.position)
    {
        return Err(user_error(
            "CynosuralGenerationInhibited",
            vec![
                ("range", PyValue::Real(range)),
                ("type", type_arg(inhibitor)),
            ],
        ));
    }
    if let Some(limit) = module_value(
        w,
        ship_id,
        module_id,
        module.type_id,
        ATTR_MAX_VELOCITY_ACTIVATION_LIMIT,
    )
    .filter(|l| *l > 0.0)
    {
        let speed = here.motion.speed();
        if speed >= limit {
            return Err(user_error(
                "VelocityTooHighToActivate",
                vec![
                    (
                        "type",
                        type_arg_for("VelocityTooHighToActivate", "type", module.type_id),
                    ),
                    ("shipVelocity", PyValue::Real(speed)),
                    ("velocityLimit", PyValue::Real(limit)),
                ],
            ));
        }
    }
    crate::modules::check_group_active(w, ship_id, module_id, module.type_id)?;
    if let Some((fuel, qty)) = fuel_of(w, module.type_id)
        && fuel_stack(w, character_id, ship_id, fuel, qty).is_none()
    {
        let (key, args) = fuel_error(w, module.type_id, fuel);
        return Err(user_error(&key, args));
    }
    Ok(())
}

/// Take one cycle's fuel out of the one stack that holds it. `false` when
/// no stack does (nothing is taken).
fn burn(w: &mut Registry, character_id: i64, ship_id: i64, module_type_id: i64) -> bool {
    let Some((fuel, qty)) = fuel_of(w, module_type_id) else {
        return true;
    };
    let Some(stack) = fuel_stack(w, character_id, ship_id, fuel, qty) else {
        return false;
    };
    let edit = if stack.quantity == qty {
        Edit::Destroy {
            item_id: stack.item_id,
        }
    } else {
        Edit::Consume {
            item_id: stack.item_id,
            quantity: qty,
        }
    };
    match w.apply_items(&[edit]) {
        Ok(_) => true,
        Err(e) => {
            crate::errln!("[cyno] fuel for {character_id} not burned: {e:?}");
            false
        }
    }
}

/// **The module started** (`modules::activate`, after the effect is
/// recorded): pay the first cycle, make the field, tell the grid and the
/// fleet. Pushes leave with the answer.
pub fn on_activated(w: &mut Registry, r: &Running) {
    if !burn(w, r.character_id, r.ship_id, r.type_id) {
        // `check_activation` found the stack a moment ago and nothing ran
        // since: only an item store that refused the edit lands here.
        crate::errln!("[cyno] module {} started without its fuel", r.module_id);
    }
    w.advance_character(r.character_id);
    let Some(here) = w.roster().get(r.character_id).copied() else {
        return;
    };
    let (corporation_id, alliance_id) = w
        .character(r.character_id)
        .map_or((0, None), |c| (c.corporation_id, c.alliance_id));
    let ball_id = w.ext_mut::<Cynos>().alloc();
    let spawn_radius =
        module_value(w, r.ship_id, r.module_id, r.type_id, ATTR_SPAWN_RADIUS).unwrap_or_default();
    let field = Field {
        ball_id,
        character_id: r.character_id,
        ship_id: r.ship_id,
        module_id: r.module_id,
        module_type_id: r.type_id,
        type_id: field_type_for(w, r.type_id),
        system_id: here.solar_system_id,
        position: here.motion.position,
        spawn_radius,
        corporation_id,
        alliance_id,
    };
    w.ext_mut::<Cynos>().fields.insert(r.module_id, field);
    announce_added(w, &field);
    let pushes = w.fleets_mut().light_beacon(
        r.character_id,
        Beacon {
            solar_system_id: field.system_id,
            beacon_id: field.ball_id,
            type_id: field.type_id,
        },
    );
    for p in pushes {
        w.push(p);
    }
}

/// **A later cycle starts** (`modules::advance`, before the cycle is
/// recorded): pay its fuel. `Err` is the stop's `error` --
/// `(ModuleRequiresFuel, {fuel, fueled})` -- and the module must stop.
pub fn pay_cycle(w: &mut Registry, r: &Running) -> Result<(), PyValue> {
    if burn(w, r.character_id, r.ship_id, r.type_id) {
        return Ok(());
    }
    let fuel = fuel_of(w, r.type_id).map_or(0, |(f, _)| f);
    let (key, args) = fuel_error(w, r.type_id, fuel);
    Err(PyValue::Tuple(vec![
        crate::pyutil::bstr(&key),
        PyValue::Dict(crate::pyutil::pypairs(args)),
    ]))
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// **Take down the field of every module that stopped** (`modules::tick`,
/// after the cycles are advanced). A module that is no longer running, or
/// whose pilot is no longer in that ship in that system, loses its field:
/// `RemoveBalls` to the system and the beacon off the fleet's lists, both on
/// the clock.
pub fn tick(w: &mut Registry) {
    let gone: Vec<i64> = fields(w)
        .filter(|f| {
            w.modules().running(f.module_id).is_none()
                || w.roster()
                    .get(f.character_id)
                    .is_none_or(|p| p.ship_id != f.ship_id || p.solar_system_id != f.system_id)
        })
        .map(|f| f.module_id)
        .collect();
    for module_id in gone {
        remove_field(w, module_id);
    }
}

/// Take one field down, now: forget it, tell the pilot's fleet, remove the
/// ball from every park in the system. Pushes leave on the clock.
pub fn remove_field(w: &mut Registry, module_id: i64) -> bool {
    let Some(f) = w.ext_mut::<Cynos>().fields.remove(&module_id) else {
        return false;
    };
    for p in w.fleets_mut().douse_beacon(f.character_id) {
        w.schedule(Duration::ZERO, p);
    }
    let to = audience(w, f.system_id);
    let stamp = w.next_destiny_stamp();
    if let Some(p) = crate::destiny::BallsRemoved::new(to, stamp, vec![f.ball_id]) {
        w.schedule(Duration::ZERO, p);
    }
    true
}

// ---------------------------------------------------------------------------
// The ball
// ---------------------------------------------------------------------------

/// Every character in space in `system`, the audience of anything that
/// happens to a ball there.
fn audience(w: &Registry, system: i64) -> Vec<i64> {
    w.roster()
        .in_system(system)
        .into_iter()
        .map(|p| p.character_id)
        .filter(|id| w.character(*id).is_some())
        .collect()
}

/// The field's ball: a body at rest, no mass, not free (a hub's, a wreck's),
/// carrying its pilot's corporation and alliance.
pub fn ball(f: &Field) -> Ball {
    Ball {
        ball_id: f.ball_id,
        mode: Mode::Stop,
        radius: FIELD_RADIUS as f32,
        position: f.position,
        flags: FLAGS_STATIC_BALL,
        mass: 0.0,
        cloak: 0,
        harmonic: -1,
        corporation_id: i32::try_from(f.corporation_id).unwrap_or(-1),
        alliance_id: f
            .alliance_id
            .and_then(|a| i32::try_from(a).ok())
            .unwrap_or(-1),
        physics: None,
        formation_id: NO_FORMATION,
    }
}

/// The field's slim: the field's type, the pilot as owner, their corp, no
/// `charID` **[A]** (module header).
pub fn slim(f: &Field) -> Slim {
    Slim {
        item_id: f.ball_id,
        type_id: f.type_id,
        owner_id: f.character_id,
        char_id: None,
        corp_id: Some(f.corporation_id),
        response_thresholds: None,
    }
}

/// `system`'s fields as balls and slims, for `destiny::scene` and
/// `grid::local_balls` (coordinator: one line in each).
pub fn balls_in(w: &Registry, system: i64) -> Vec<(Ball, Slim)> {
    fields(w)
        .filter(|f| f.system_id == system)
        .map(|f| (ball(f), slim(f)))
        .collect()
}

/// Where the field `ball_id` is, for `Registry::ball_target` (coordinator:
/// one line there): approach, orbit and look at it. **[A]** it has no warp-in
/// point: a field is a beacon, not a celestial, so the client's own gate
/// gives none (the same reading as the hub's).
pub fn target(w: &Registry, ball_id: i64) -> Option<crate::model::Target> {
    field_by_ball(w, ball_id).map(|f| crate::model::Target {
        position: f.position,
        radius: FIELD_RADIUS,
        warp_in: None,
    })
}

/// `AddBalls2` of a new field to the pilots in its system (the grid's
/// filter then keeps it to those on the grid).
fn announce_added(w: &mut Registry, f: &Field) {
    let to = audience(w, f.system_id);
    if to.is_empty() {
        return;
    }
    let stamp = w.next_destiny_stamp();
    let added = crate::destiny::BallsAdded::new(
        to,
        stamp,
        crate::destiny::Park {
            current_time: stamp,
            balls: vec![ball(f)],
        },
        vec![(slim(f), None)],
    );
    if let Some(a) = added {
        w.push(a);
    }
}

// ---------------------------------------------------------------------------
// Readers for the services
// ---------------------------------------------------------------------------

/// `map.GetBeaconCount()`: `{solarSystemID: count}`, a dict **[V]**
/// (`mapViewColorHandler.ColorStarsByCynosuralFields:787` iterates
/// `.iteritems()`, `starModeHandler:679` calls `.keys()`, and the filter
/// `.get(systemID, 0)`). The count is the module beacons the caller's fleet
/// can jump to **[A]**: the call has no arguments and the cache is named
/// `fleetBeaconCount`; falsified by a retail map that counts every fleet's.
pub fn beacon_count(w: &Registry, actor: Option<i64>) -> PyValue {
    let mut by_system: BTreeMap<i64, i64> = BTreeMap::new();
    if let Some(actor) = actor {
        for (_, b) in w.fleets().beacons_visible_to(actor) {
            *by_system.entry(b.solar_system_id).or_default() += 1;
        }
    }
    PyValue::Dict(
        by_system
            .into_iter()
            .map(|(s, n)| (PyValue::Int(n), PyValue::Int(s)))
            .collect(),
    )
}

/// **The dock guard** (coordinator: one line in `service/space.rs`'s
/// `CmdDock` arm, beside `entosis::refuse_dock`): a ship whose cyno runs
/// does not dock (`disallowDocking` 2354 on the module **[V-data]**), with
/// `CantDockWhileModuleActive{moduleType}` **[V]**.
pub fn refuse_dock(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    let module = actor
        .and_then(|a| w.character(a))
        .and_then(|c| running_module_on(w, c.ship_id));
    match module {
        Some(t) => Err(user_error(
            "CantDockWhileModuleActive",
            vec![(
                "moduleType",
                type_arg_for("CantDockWhileModuleActive", "moduleType", t),
            )],
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
