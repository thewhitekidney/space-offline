//! Boarding a ship and fitting modules into it: slots, budgets, the validator.
//!
//! Spec: `docs/lanes/fitting.md`. Owned by the P2.0 lane.
//!
//! **Fitting is not a dogma call; it is an inventory move** (spec §0.2): the
//! client fits with `shipInv.Add(itemID, sourceID, qty=1, flag=slotFlag)` on
//! the ship's own `invbroker` inventory and unfits with the same call on the
//! hangar's [V]. So nothing here talks to the wire. This file answers the one
//! question both arms ask -- *may this module go in that slot, and which plan
//! puts it there?* -- and `service/inventory.rs` turns the answer into a call
//! result. Boarding's own state rule, "the active ship is the one holding the
//! capsule", lives here too, because a restore needs it and nothing else
//! does.
//!
//! **CPU and powergrid are not checked here.** CCP's own reference dogma fits
//! first and refuses only at *online* (`online.py:56,62`, `bdl:749-785`) [V];
//! the budget is [`crate::modules`]'s, at `SetModuleOnline` and at fit time
//! through [`crate::modules::on_fitted`].

use crate::content::Content;
use crate::item::{Edit, Item, ItemType, Slot, Store};
use crate::model::Character;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `flagCapsule`, where a boarded character's pod sits inside the ship
/// (`inventorycommon/const`) [V]. `godma.OnItemChange` has an explicit skip
/// for exactly "a capsule at `(session.shipid, flagCapsule)`", which is the
/// evidence the real server parks it here (spec §1.2) [I].
pub const FLAG_CAPSULE: i64 = 56;
/// `flagCargo` [V].
pub const FLAG_CARGO: i64 = 5;
/// `flagAutoFit`: "Fit to Active Ship" sends `flag=0` and lets the server
/// choose the slot (`fittingUtil.py:171`) [V].
pub const FLAG_AUTOFIT: i64 = 0;
/// `groupCapsule` [V].
pub const GROUP_CAPSULE: i64 = 29;
/// `categoryModule` [V].
pub const CATEGORY_MODULE: i64 = 7;

/// The attributes that name the hull groups a module fits: `canFitShipGroup01`
/// .. `20` (1298-1301, 1872, 1879-1881, 2065, 2396, 2476-2485), the type data's
/// own rows [V-data]. The client collects them by name from the group-typed
/// attributes (`baseDogmaStaticSvc.py:90`); this server has no attribute
/// categories, so the ids are listed, and a test pins the count.
pub const CAN_FIT_GROUP_ATTRS: &[i64] = &[
    1298, 1299, 1300, 1301, 1872, 1879, 1880, 1881, 2065, 2396, 2476, 2477, 2478, 2479, 2480, 2481,
    2482, 2483, 2484, 2485,
];
/// The attributes that name the hull types a module fits: `canFitShipType1`
/// .. `12` (1302-1305, 1944, 2103, 2463, 2486-2488, 2758, 5948) and
/// `fitsToShipType` (1380) [V-data]; the client takes the type-typed
/// attributes named `fitsToShipType` or `canFitShipType*`
/// (`baseDogmaStaticSvc.py:101`).
pub const CAN_FIT_TYPE_ATTRS: &[i64] = &[
    1302, 1303, 1304, 1305, 1944, 2103, 2463, 2486, 2487, 2488, 2758, 5948, 1380,
];

/// `BaseDogmaStaticSvc.CanFitModuleToShipTypeOrGroup` [V]
/// (`baseDogmaStaticSvc.py:278`, pin 3662168 carries the same names): a
/// module that names no hull group and no hull type fits any hull; else the
/// hull's group must be among the groups it names, or the hull's type among
/// the types it names. `own` is the module's own attribute (`None` when it
/// does not carry it, a default is not "named").
///
/// The client's first test, `HasIgnoreFittingRestrictionsComponent(ship)`
/// (a space component on the hull type), has no data here: no hull is
/// exempt **[A]**; falsified by a hull a player fits freely on TQ that this
/// refuses.
pub fn hull_allows(own: impl Fn(i64) -> Option<f64>, ship_group: i64, ship_type: i64) -> bool {
    let named = |ids: &[i64], v: i64| {
        let mut any = false;
        for &a in ids {
            if let Some(x) = own(a) {
                any = true;
                if x.round() as i64 == v {
                    return (true, true);
                }
            }
        }
        (any, false)
    };
    let (g_any, g_hit) = named(CAN_FIT_GROUP_ATTRS, ship_group);
    let (t_any, t_hit) = named(CAN_FIT_TYPE_ATTRS, ship_type);
    !(g_any || t_any) || g_hit || t_hit
}

/// `maxNonCapitalModuleSize` [V] (`inventorycommon/const.py:2766`): a module
/// whose volume is over this many m3 is too big for a ship that is neither a
/// capital hull nor a structure.
pub const MAX_NON_CAPITAL_MODULE_M3: f64 = 3500.0;

/// `shipfitting.IsModuleTooBigForShip` [V] (`fittingStuff.py:74`): strictly
/// over [`MAX_NON_CAPITAL_MODULE_M3`], on a ship whose group is not one of
/// [`crate::typedata::CAPITAL_GROUPS`] and whose category is not structure.
/// (Freighters are not capital by the client's table, so the gate holds on
/// them.)
pub fn module_too_big(module_volume: f64, ship_group_id: i64, ship_category_id: i64) -> bool {
    !crate::typedata::is_capital_group(ship_group_id)
        && ship_category_id != crate::typedata::CATEGORY_STRUCTURE
        && module_volume > MAX_NON_CAPITAL_MODULE_M3
}

/// A module rack, from the module's slot effect: `loPower` 11, `hiPower` 12,
/// `medPower` 13 (`dogma/const`) [V]. Rigs and subsystems are not fitted by
/// this server (no rig or subsystem is spawnable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rack {
    Low,
    Medium,
    High,
}

impl Rack {
    /// The rack's first flag: `flagLoSlot0` 11, `flagMedSlot0` 19,
    /// `flagHiSlot0` 27, eight flags each (`inventorycommon/const`) [V].
    pub fn first_flag(self) -> i64 {
        match self {
            Rack::Low => 11,
            Rack::Medium => 19,
            Rack::High => 27,
        }
    }

    /// Which rack a flag is in, and its index within it.
    pub fn of_flag(flag_id: i64) -> Option<(Rack, i64)> {
        [Rack::Low, Rack::Medium, Rack::High]
            .into_iter()
            .find(|r| (r.first_flag()..r.first_flag() + 8).contains(&flag_id))
            .map(|r| (r, flag_id - r.first_flag()))
    }
}

/// A hardpoint a module uses: the `turretFitted` (42) or `launcherFitted`
/// (40) effect (`dogma/const`) [V].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hardpoint {
    Turret,
    Launcher,
}

/// A hull's fitting numbers: its dogma attributes `hiSlots` 14, `medSlots`
/// 13, `lowSlots` 12, `rigSlots` 1137, `turretSlotsLeft` 102,
/// `launcherSlotsLeft` 101, `cpuOutput` 48, `powerOutput` 11 [V-data].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullFit {
    pub type_id: i64,
    pub hi: i64,
    pub med: i64,
    pub low: i64,
    pub rig: i64,
    pub turrets: i64,
    pub launchers: i64,
    pub cpu_output: f64,
    pub power_output: f64,
}

impl HullFit {
    /// How many slots this hull has in a rack.
    pub fn slots(&self, rack: Rack) -> i64 {
        match rack {
            Rack::Low => self.low,
            Rack::Medium => self.med,
            Rack::High => self.hi,
        }
    }

    /// How many of a hardpoint this hull has.
    pub fn hardpoints(&self, h: Hardpoint) -> i64 {
        match h {
            Hardpoint::Turret => self.turrets,
            Hardpoint::Launcher => self.launchers,
        }
    }
}

const fn hull_fit(
    type_id: i64,
    [hi, med, low, rig, turrets, launchers]: [i64; 6],
    cpu_output: f64,
    power_output: f64,
) -> HullFit {
    HullFit {
        type_id,
        hi,
        med,
        low,
        rig,
        turrets,
        launchers,
        cpu_output,
        power_output,
    }
}

/// Every spawnable hull's fitting numbers.
///
/// **The client's own data, read with `tools/dogma.py` against build
/// 3619879** -- the dump is `out/fitting/fit_numbers.tsv` (gitignored
/// scratch, `docs/lanes/fitting.md` §3.3) -- the same curated-table trade as
/// `crate::modules::TYPES` and `crate::destiny::hull`. Kept out of
/// `destiny::hull` because that table is another lane's; a test below keeps
/// the two covering the same hulls. Columns: `[hi, med, low, rig, turret,
/// launcher]`, then `cpuOutput`, `powerOutput`. Raw hull values: no skill
/// bonus (CPU Management etc.) applies yet -- spec §12 risk 3.
pub const HULLS: &[HullFit] = &[
    hull_fit(587, [3, 3, 4, 3, 3, 2], 130.0, 41.0), // Rifter
    hull_fit(594, [3, 3, 4, 3, 3, 0], 135.0, 45.0), // Incursus
    hull_fit(597, [4, 2, 5, 3, 4, 0], 140.0, 67.0), // Punisher
    hull_fit(601, [2, 2, 2, 0, 2, 2], 135.0, 23.0), // Ibis
    hull_fit(603, [3, 4, 3, 3, 3, 0], 180.0, 40.0), // Merlin
    hull_fit(621, [5, 5, 4, 3, 0, 5], 430.0, 630.0), // Caracal
    hull_fit(629, [5, 4, 5, 3, 4, 1], 350.0, 860.0), // Rupture
    hull_fit(639, [8, 5, 6, 3, 6, 4], 550.0, 16000.0), // Tempest
    hull_fit(641, [7, 4, 8, 3, 7, 0], 600.0, 15500.0), // Megathron
    hull_fit(642, [8, 4, 7, 3, 8, 0], 540.0, 21000.0), // Apocalypse
    hull_fit(648, [2, 6, 4, 3, 1, 1], 380.0, 185.0), // Badger
    hull_fit(657, [2, 4, 5, 3, 1, 0], 270.0, 85.0), // Iteron Mark V
    hull_fit(670, [0, 0, 0, 0, 0, 0], 1.0, 1.0),    // Capsule
    hull_fit(1944, [2, 3, 6, 3, 1, 0], 230.0, 90.0), // Bestower
    hull_fit(2006, [5, 3, 6, 3, 5, 0], 330.0, 1000.0), // Omen
    hull_fit(16227, [7, 6, 4, 3, 6, 0], 515.0, 1150.0), // Ferox
    hull_fit(16240, [8, 2, 3, 3, 8, 0], 178.0, 70.0), // Catalyst
    hull_fit(16242, [8, 3, 2, 3, 7, 1], 170.0, 70.0), // Thrasher
    hull_fit(24702, [7, 4, 6, 3, 6, 3], 400.0, 1140.0), // Hurricane
];

/// One hull's fitting numbers: [`HULLS`] first, then every ship in the
/// world's typedata (`crate::typedata::hull_fit_in`, CP58c).
pub fn hull_in(content: &Content, type_id: i64) -> Option<&HullFit> {
    HULLS
        .iter()
        .find(|h| h.type_id == type_id)
        .or_else(|| crate::typedata::hull_fit_in(content, type_id))
}

/// A ship's fitting numbers as it stands: [`hull_in`] for an ordinary hull,
/// and for a strategic cruiser that row plus its fitted subsystems'
/// modifiers (`crate::subsystems::hull_fit`, T3-D).
pub fn hull_of_ship(content: &Content, store: &Store, ship_id: i64) -> Option<HullFit> {
    crate::subsystems::hull_fit(content, store, ship_id, None)
}

/// One item type, from the world's tables: `item::find_in` (the hand rows,
/// PI's types, the skillbooks, then the world's typedata).
fn item_type(content: &Content, type_id: i64) -> Option<&ItemType> {
    crate::item::find_in(content, type_id)
}

// ---------------------------------------------------------------------------
// The active ship
// ---------------------------------------------------------------------------

/// This character's capsule: the one they own that is either their active
/// ship or parked in it at [`FLAG_CAPSULE`].
pub fn capsule_of<'a>(store: &'a Store, c: &Character) -> Option<&'a Item> {
    let is_pod = |i: &&Item| i.owner_id == c.character_id && i.group_id == GROUP_CAPSULE;
    store.get(c.ship_id).filter(is_pod).or_else(|| {
        store
            .contents_of(c.ship_id)
            .filter(is_pod)
            .find(|i| i.flag_id == FLAG_CAPSULE)
    })
}

/// The ship this character is in, as `(ship_id, type_id)`, **derived from
/// where the capsule is** rather than stored (spec §7) [A]: the ship holding
/// the pod at [`FLAG_CAPSULE`], else the pod itself. `None` if the character
/// has no capsule at all.
///
/// `Character.ship_id` is not in the snapshot, so after a restart the seeded
/// value names the config's capsule -- which is exactly the item this starts
/// from. A pod's id never changes, so the answer survives any number of
/// boards.
pub fn active_ship_of(store: &Store, c: &Character) -> Option<(i64, i64)> {
    let pod = capsule_of(store, c)?;
    if pod.flag_id == FLAG_CAPSULE
        && let Some(ship) = store.get(pod.location_id)
        && ship.owner_id == c.character_id
        && ship.category_id == crate::model::CATEGORY_SHIP
    {
        return Some((ship.item_id, ship.type_id));
    }
    Some((pod.item_id, pod.type_id))
}

/// Re-derive every character's active ship after a snapshot loads.
///
/// **The one call the restore path needs** -- `Registry::apply_snapshot`
/// calls it after the items are in. A character with no capsule keeps the
/// seeded values.
pub fn restore_active_ships(w: &mut Registry) {
    let found: Vec<(i64, (i64, i64))> = w
        .config()
        .characters
        .iter()
        .filter_map(|c| active_ship_of(w.store(), c).map(|s| (c.character_id, s)))
        .collect();
    for (character_id, (ship_id, type_id)) in found {
        if let Some(c) = w.character_mut(character_id) {
            c.ship_id = ship_id;
            c.ship_type_id = type_id;
        }
    }
}

// ---------------------------------------------------------------------------
// The validator
// ---------------------------------------------------------------------------

/// Why a fit was refused. **Every key it becomes is [A]**: all exist in the
/// client's dialog table (`findings/user-errors.tsv`) and none has a raise
/// site in the client, because the check lives in the server's `invbroker`
/// (spec §2.3).
///
/// `Eq` is not derived: [`Refusal::NoRoom`] carries cubic metres.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    NoCharacter,
    NotDocked,
    /// The bound container is not the active ship.
    NotActiveShip,
    /// The active ship is a capsule (`CantFitToCapsule`).
    Capsule,
    /// Missing, not ours, or not somewhere a fit may take it from.
    NotAvailable,
    /// Not a module this server can fit, or the wrong rack for the flag
    /// (`CantFitModuleToThatShip`).
    WrongKind {
        type_id: i64,
        ship_type_id: i64,
    },
    /// A slot index the hull does not have, or no free slot for an autofit
    /// (`NoFreeShipSlots`).
    NoSlot,
    /// Another module is in that slot (`SlotAlreadyOccupied`).
    Occupied,
    /// The module's volume is over [`MAX_NON_CAPITAL_MODULE_M3`] and the
    /// hull is not a capital ship or a structure (`ModuleTooBigForThisShip`,
    /// FSD 2424, args `item` and `ship`) [V key and args,
    /// `findings/user-errors.tsv`; the client refuses first, so the raise
    /// site is ours, [A]].
    ModuleTooBig {
        type_id: i64,
        ship_type_id: i64,
    },
    /// A hold whose `inventoryFlagData` row does not take the type
    /// (`CannotPlaceTypeInFuelBay`, FSD 2345, arg `type`) [V key and arg; no
    /// client raise site -- `DoesAcceptItem` refuses the drop first, so the
    /// raise is ours, [A]].
    WrongHold {
        type_id: i64,
    },
    /// Every hardpoint of the kind is in use.
    NoHardpoint {
        hardpoint: Hardpoint,
        type_id: i64,
    },
    /// Unfit: the module is cycling (`CannotRemoveActivatedModule`).
    Active,
    /// Unfit or slot-to-slot: the module still has charges in it
    /// (`CannotRemoveModuleWithLoadedCharges`). **[A]** the key -- it exists
    /// with no argument and no client raise site -- and the refusal itself:
    /// the client unloads before it unfits (`fittingSlotController.py`
    /// `UnfitModule` calls `UnfitCharge` first) [V], so only a path that
    /// skips that meets this.
    Loaded,
    /// Something this server does not do yet. Our words, not CCP's.
    Unsupported(&'static str),
    /// Cargo: the hold cannot take that much (`NotEnoughCargoSpace`, FSD
    /// 1336, args `available` and `volume` [V] `findings/user-errors.tsv`).
    /// **The key is the protocol, not just the message**:
    /// `invControllers._AddItem` catches exactly this key (`:319`-`:323`),
    /// works out from its own capacity how many units *do* fit, and offers
    /// the move again at that quantity (`:344`, `:360`) [V]. Which number goes
    /// in which argument is [A]: `available` the free m3, `volume` the m3
    /// the move needed.
    NoRoom {
        available: f64,
        volume: f64,
    },
    /// Cargo: an assembled ship (`AssembledShipsDontFitInCargo`, FSD 105, no
    /// args) [V key; A raise site -- the client has none].
    AssembledShip,
    /// Cargo: the ship into its own hold (`CannotPlaceItemInsideItself`) --
    /// the client's own check, `invCache.py:1092` [V]; a second client or a
    /// stale window is the only way here.
    InsideItself,
    /// Cargo: the active ship into another hold
    /// (`CanNotPlaceActiveShipInAnotherHangar`, `invCache.py:1074`) [V].
    ActiveShip,
    /// Loot: the wreck is out of reach (`NotCloseEnoughToLoot`, FSD 1330,
    /// arg `maxdist` [V key and arg, `findings/user-errors.tsv`; A raise
    /// site -- the client checks range itself, `invControllers.py:1502`]).
    TooFar {
        maxdist: f64,
    },
    /// A subsystem for another hull, or onto a hull that takes none
    /// (`CannotFitSubSystemNotShip`, args `subSystemName`, `validShipName`,
    /// `shipName`, each `(UE_TYPEID, typeID)`) [V: `fittingSvc.py:674`; the
    /// raise site is ours, [A]].
    SubSystemNotShip {
        type_id: i64,
        valid_ship_type_id: i64,
        ship_type_id: i64,
    },
    /// A subsystem into a flag that is not its own, or one it has none
    /// (`NoSuitableSlotsForThisSubSystem`, arg `subSystemName`)
    /// [V: `fittingStuff.py:269`; the raise site is ours, [A]].
    NoSubSystemSlot {
        type_id: i64,
    },
    /// More of a module's group than its `maxGroupFitted` (1544) allows
    /// (`CantFitTooManyByGroup`; args `module`, `ship`, `groupName`,
    /// `noOfModules`, `noOfModulesFitted`) [V key and args,
    /// `findings/user-errors.tsv`; the client raises it itself before a
    /// drag (`clientDogmaLocation.py:1397`), the raise from `FitFitting` is
    /// ours, [A]]. `groupName` is the group id as text: no table here holds
    /// group names [A; falsifier: the dialog shows a number].
    TooManyByGroup {
        type_id: i64,
        ship_type_id: i64,
        group_id: i64,
        limit: i64,
        fitted: i64,
    },
    /// The same for `maxTypeFitted` (2431): `CantFitTooManyByType`, args
    /// `module`, `ship`, `noOfModules`, `noOfModulesFitted` [V].
    TooManyByType {
        type_id: i64,
        ship_type_id: i64,
        limit: i64,
        fitted: i64,
    },
}

impl Refusal {
    /// The `UserError` this refusal is shown as.
    pub fn to_error(&self) -> ServiceError {
        let type_arg = |t: i64| {
            PyValue::Tuple(vec![
                PyValue::Int(crate::modules::UE_TYPEID),
                PyValue::Int(t),
            ])
        };
        let (key, args): (&str, Vec<(&str, PyValue)>) = match self {
            Refusal::NoCharacter => notify("No character is selected."),
            Refusal::NotDocked => notify("Fitting in space is not supported by this server yet."),
            Refusal::NotActiveShip => notify("Modules can only be fitted to your active ship."),
            Refusal::Capsule => ("CantFitToCapsule", vec![]),
            Refusal::NotAvailable => notify("That item is not in your hangar or your ship."),
            Refusal::WrongKind {
                type_id,
                ship_type_id,
            } => (
                "CantFitModuleToThatShip",
                vec![
                    ("item", type_arg(*type_id)),
                    ("ship", type_arg(*ship_type_id)),
                ],
            ),
            Refusal::NoSlot => ("NoFreeShipSlots", vec![]),
            Refusal::Occupied => ("SlotAlreadyOccupied", vec![]),
            Refusal::ModuleTooBig {
                type_id,
                ship_type_id,
            } => (
                "ModuleTooBigForThisShip",
                vec![
                    ("item", type_arg(*type_id)),
                    ("ship", type_arg(*ship_type_id)),
                ],
            ),
            Refusal::WrongHold { type_id } => (
                "CannotPlaceTypeInFuelBay",
                vec![("type", type_arg(*type_id))],
            ),
            Refusal::NoHardpoint {
                hardpoint: Hardpoint::Turret,
                type_id,
            } => (
                "NotEnoughTurretSlots",
                vec![("moduleName", type_arg(*type_id))],
            ),
            Refusal::NoHardpoint {
                hardpoint: Hardpoint::Launcher,
                type_id,
            } => (
                "NotEnoughLauncherSlots",
                vec![("moduleName", type_arg(*type_id))],
            ),
            Refusal::Active => ("CannotRemoveActivatedModule", vec![]),
            Refusal::Loaded => ("CannotRemoveModuleWithLoadedCharges", vec![]),
            Refusal::Unsupported(text) => notify(text),
            Refusal::NoRoom { available, volume } => (
                "NotEnoughCargoSpace",
                vec![
                    ("available", PyValue::Real(*available)),
                    ("volume", PyValue::Real(*volume)),
                ],
            ),
            Refusal::AssembledShip => ("AssembledShipsDontFitInCargo", vec![]),
            Refusal::InsideItself => ("CannotPlaceItemInsideItself", vec![]),
            Refusal::ActiveShip => ("CanNotPlaceActiveShipInAnotherHangar", vec![]),
            Refusal::TooFar { maxdist } => (
                "NotCloseEnoughToLoot",
                vec![("maxdist", PyValue::Real(*maxdist))],
            ),
            Refusal::SubSystemNotShip {
                type_id,
                valid_ship_type_id,
                ship_type_id,
            } => (
                "CannotFitSubSystemNotShip",
                vec![
                    ("subSystemName", type_arg(*type_id)),
                    ("validShipName", type_arg(*valid_ship_type_id)),
                    ("shipName", type_arg(*ship_type_id)),
                ],
            ),
            Refusal::NoSubSystemSlot { type_id } => (
                "NoSuitableSlotsForThisSubSystem",
                vec![("subSystemName", type_arg(*type_id))],
            ),
            Refusal::TooManyByGroup {
                type_id,
                ship_type_id,
                group_id,
                limit,
                fitted,
            } => (
                "CantFitTooManyByGroup",
                vec![
                    (
                        "module",
                        crate::modules::type_arg_for("CantFitTooManyByGroup", "module", *type_id),
                    ),
                    (
                        "ship",
                        crate::modules::type_arg_for(
                            "CantFitTooManyByGroup",
                            "ship",
                            *ship_type_id,
                        ),
                    ),
                    ("groupName", PyValue::Unicode(group_id.to_string())),
                    ("noOfModules", PyValue::Int(*limit)),
                    ("noOfModulesFitted", PyValue::Int(*fitted)),
                ],
            ),
            Refusal::TooManyByType {
                type_id,
                ship_type_id,
                limit,
                fitted,
            } => (
                "CantFitTooManyByType",
                vec![
                    ("module", type_arg(*type_id)),
                    ("ship", type_arg(*ship_type_id)),
                    ("noOfModules", PyValue::Int(*limit)),
                    ("noOfModulesFitted", PyValue::Int(*fitted)),
                ],
            ),
        };
        ServiceError::Raise(eve_proto::user_error(key, crate::pyutil::pypairs(args)))
    }
}

fn notify(text: &str) -> (&'static str, Vec<(&'static str, PyValue)>) {
    ("CustomNotify", vec![("notify", crate::pyutil::bstr(text))])
}

/// A fit the validator accepted: the plan, and the slot it chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fit {
    pub plan: Vec<Edit>,
    pub flag_id: i64,
}

/// **No fitting in space**: [`Refusal::NotDocked`] while the pilot is in the
/// roster.
///
/// `Character::station_id` is the pilot's *home* and is never cleared on
/// undock (`Registry::undock_change` reads it; nothing writes `None`), so the
/// `station_id.ok_or(NotDocked)` the two plans open with never fired: an
/// unfit in space moved the module to the station hangar from wherever the
/// ship was. Real EVE fits only docked (or beside a fitting service this
/// server does not have) `[A]`.
///
/// **And an observer could not see the change.** Another pilot's client
/// fits a ship's guns once, when its model assembles, from the slim's
/// `modules` (`crate::destiny::SlimModule`); `OnSlimItemChange` refits only
/// a modular hull's subsystems (`michelle.py:2021`) and `ship.OnSlimItemUpdated`
/// never touches the guns (`ship.py:304-323`) **[V]**. Refusing here keeps
/// the slim an observer was sent at undock the truth.
///
/// `plan_unfit` calls this; `plan_fit_in` takes a bare store, so its caller
/// (`service/inventory.rs` `fit_one`) does.
pub fn refuse_in_space(w: &Registry, c: &Character) -> Result<(), Refusal> {
    // `docked_station`, not the bare roster: a pilot mid-jump is in neither
    // the roster nor a station (`docs/lanes/docked.md`).
    if w.docked_station(c.character_id).is_none() {
        return Err(Refusal::NotDocked);
    }
    Ok(())
}

/// Fit `module_id` into the active ship at `flag_id` ([`FLAG_AUTOFIT`] to
/// let the server choose), or say why not.
///
/// The checks are spec §2.3's table, in its order. The client has already
/// checked rack, slot existence and `maxGroupFitted` itself (§2.2), so the
/// server mostly sees a refusal from a second client or a stale window.
/// `ship_id` is the bound container the call arrived on.
///
/// **A module already fitted to this ship may be moved** to another slot
/// (slot-to-slot drag): its own slot and its own hardpoint are not counted
/// against it.
pub fn plan_fit_in(
    content: &Content,
    store: &Store,
    c: Option<&Character>,
    ship_id: i64,
    module_id: i64,
    flag_id: i64,
) -> Result<Fit, Refusal> {
    // T3-D: a subsystem has its own rules (`crate::subsystems::plan_fit`).
    if store
        .get(module_id)
        .is_some_and(|m| m.category_id == crate::subsystems::CATEGORY_SUBSYSTEM)
    {
        return crate::subsystems::plan_fit(content, store, c, ship_id, module_id, flag_id);
    }
    // And a modular hull has the slots its fitted subsystems give it.
    let modular = store
        .get(ship_id)
        .filter(|s| crate::subsystems::is_modular_group(s.group_id))
        .and_then(|_| crate::subsystems::hull_fit(content, store, ship_id, None));
    let modular_type = store.get(ship_id).map(|s| s.type_id);
    plan_fit_with(
        |t| match &modular {
            Some(h) if Some(t) == modular_type => Some(h),
            _ => hull_in(content, t),
        },
        |t| crate::modules::find_in(content, t),
        |t| item_type(content, t).map_or(0.0, |i| i.volume),
        |t, a| crate::typedata::attribute_in(content, t, a),
        |t, a| crate::typedata::own_attribute_in(content, t, a),
        store,
        c,
        ship_id,
        module_id,
        flag_id,
    )
}

/// `maxGroupFitted` [V] (`dogma/const`).
const ATTR_MAX_GROUP_FITTED: i64 = 1544;
/// `maxTypeFitted` [V].
const ATTR_MAX_TYPE_FITTED: i64 = 2431;

/// The fit check, with the hull and module tables as lookups: [`plan_fit_in`]
/// passes the content's.
#[allow(clippy::too_many_arguments)]
fn plan_fit_with<'h, 'm>(
    hull_of: impl Fn(i64) -> Option<&'h HullFit>,
    module_of: impl Fn(i64) -> Option<&'m crate::modules::ModuleType>,
    volume_of: impl Fn(i64) -> f64,
    attribute_of: impl Fn(i64, i64) -> Option<f64>,
    own_attribute_of: impl Fn(i64, i64) -> Option<f64>,
    store: &Store,
    c: Option<&Character>,
    ship_id: i64,
    module_id: i64,
    flag_id: i64,
) -> Result<Fit, Refusal> {
    let c = c.ok_or(Refusal::NoCharacter)?;
    let station = c.station_id.ok_or(Refusal::NotDocked)?;
    if ship_id != c.ship_id {
        return Err(Refusal::NotActiveShip);
    }
    let ship = store.get(ship_id).ok_or(Refusal::NotActiveShip)?;
    if ship.group_id == GROUP_CAPSULE {
        return Err(Refusal::Capsule);
    }
    let module = store.get(module_id).ok_or(Refusal::NotAvailable)?;
    let in_hangar = module.location_id == station && module.flag_id == crate::model::FLAG_HANGAR;
    let in_ship =
        module.location_id == ship_id && crate::item::is_ship_fitting_flag(module.flag_id);
    if module.owner_id != c.character_id || !(in_hangar || in_ship) {
        return Err(Refusal::NotAvailable);
    }
    let wrong_kind = Refusal::WrongKind {
        type_id: module.type_id,
        ship_type_id: ship.type_id,
    };
    if module.category_id != CATEGORY_MODULE {
        return Err(wrong_kind);
    }
    // A module whose rack this server does not know cannot be placed.
    let mt = module_of(module.type_id).ok_or(wrong_kind.clone())?;
    // The client's own order (`GetModuleErrorInfo`): kind, then the hull
    // restriction (`CANT_FIT_MODULE_TO_SHIP`), then size.
    // A module already fitted (an illegal fit in a saved world) may still
    // move slot to slot: only a module coming in from the hangar is asked.
    if !in_ship
        && !hull_allows(
            |a| own_attribute_of(module.type_id, a),
            ship.group_id,
            ship.type_id,
        )
    {
        return Err(wrong_kind);
    }
    if module_too_big(volume_of(module.type_id), ship.group_id, ship.category_id) {
        return Err(Refusal::ModuleTooBig {
            type_id: module.type_id,
            ship_type_id: ship.type_id,
        });
    }
    let h = hull_of(ship.type_id).ok_or(Refusal::NoSlot)?;

    // A fitted module moving slots would leave its rounds behind in the old
    // one: they are addressed by the flag (`crate::item::is_loaded_charge`).
    if in_ship && crate::modules::loaded(store, ship_id, module.flag_id).is_some() {
        return Err(Refusal::Loaded);
    }

    // What else is fitted, not counting the module itself -- **nor a loaded
    // charge**, which shares its module's flag and is not a module.
    let others: Vec<&Item> = store
        .contents_of(ship_id)
        .filter(|i| {
            i.item_id != module_id
                && crate::item::is_ship_fitting_flag(i.flag_id)
                && !crate::item::is_loaded_charge(i)
        })
        .collect();
    let free = |flag: i64| !others.iter().any(|i| i.flag_id == flag);

    // `maxGroupFitted` / `maxTypeFitted`: the client checks them before a
    // drag, a saved fitting arrives without that (`FitFitting`). Counted over
    // the other fitted modules, never a loaded charge.
    let limit_of = |attr: i64| {
        attribute_of(module.type_id, attr)
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v as i64)
    };
    if let Some(limit) = limit_of(ATTR_MAX_GROUP_FITTED) {
        let fitted = others
            .iter()
            .filter(|i| i.group_id == module.group_id)
            .count() as i64;
        if fitted >= limit {
            return Err(Refusal::TooManyByGroup {
                type_id: module.type_id,
                ship_type_id: ship.type_id,
                group_id: module.group_id,
                limit,
                fitted,
            });
        }
    }
    if let Some(limit) = limit_of(ATTR_MAX_TYPE_FITTED) {
        let fitted = others
            .iter()
            .filter(|i| i.type_id == module.type_id)
            .count() as i64;
        if fitted >= limit {
            return Err(Refusal::TooManyByType {
                type_id: module.type_id,
                ship_type_id: ship.type_id,
                limit,
                fitted,
            });
        }
    }

    let flag_id = if flag_id == FLAG_AUTOFIT {
        // The lowest free slot of the module's rack [A].
        let first = mt.rack.first_flag();
        (first..first + h.slots(mt.rack))
            .find(|f| free(*f))
            .ok_or(Refusal::NoSlot)?
    } else {
        match Rack::of_flag(flag_id) {
            Some((rack, _)) if rack != mt.rack => return Err(wrong_kind),
            Some((_, index)) if index >= h.slots(mt.rack) => return Err(Refusal::NoSlot),
            Some(_) if !free(flag_id) => return Err(Refusal::Occupied),
            Some(_) => flag_id,
            None => return Err(wrong_kind),
        }
    };
    if let Some(hp) = mt.hardpoint {
        let used = others
            .iter()
            .filter(|i| module_of(i.type_id).and_then(|t| t.hardpoint) == Some(hp))
            .count() as i64;
        if used >= h.hardpoints(hp) {
            return Err(Refusal::NoHardpoint {
                hardpoint: hp,
                type_id: module.type_id,
            });
        }
    }

    let to = Slot::new(c.character_id, ship_id, flag_id);
    let plan = if module.singleton {
        vec![Edit::Move {
            item_id: module_id,
            to,
        }]
    } else {
        vec![Edit::Assemble {
            item_id: module_id,
            to,
        }]
    };
    Ok(Fit { plan, flag_id })
}

/// Take an item out of a ship into the station hangar: an unfit of the
/// active ship (`Add(moduleID, shipID, flag=4)` on the hangar, spec §2.5),
/// or a cargo item dragged out -- of the active ship, or of another of my
/// ships parked in this hangar, whose hold the client opens the same way
/// (`invCtrl.ShipCargo(itemID)`, `invControllers.py:742`) [V]. `quantity`
/// is the call's `qty`, for a cargo stack: the whole stack, or the part the
/// client's quantity prompt chose (`invControllers.py:304`, sent as `qty=`
/// at `:267`) [V].
///
/// **An unfitted module stays assembled** [A]: in TQ it is an assembled item
/// until repackaged, and `repackagingSvc` already handles that.
///
/// **A moved stack is not merged into one already in the hangar** [A]. A
/// merge is a `Destroy` of the moved stack, and a destroyed item has no
/// row for `OnItemChange` to carry, so the hold window would keep showing a
/// stack that is gone. "Stack All" does the merge, with the client's own
/// refresh after it.
pub fn plan_unfit(
    w: &Registry,
    c: Option<&Character>,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, Refusal> {
    let c = c.ok_or(Refusal::NoCharacter)?;
    refuse_in_space(w, c)?;
    let station = c.station_id.ok_or(Refusal::NotDocked)?;
    let item = w.store().get(item_id).ok_or(Refusal::NotAvailable)?;
    crate::subsystems::check_unfit(w.content(), w.store(), item)?;
    let from_ship = (item.location_id == c.ship_id
        && (crate::item::is_ship_fitting_flag(item.flag_id) || is_plain_hold(item.flag_id)))
        || (item.flag_id == FLAG_CARGO && parked_here(w.store(), c, station, item.location_id));
    // A loaded charge is not an item the client can name (it is a
    // `(ship, flag, type)` sublocation to it); `UnloadAmmo` is its way out.
    if item.owner_id != c.character_id || !from_ship || crate::item::is_loaded_charge(item) {
        return Err(Refusal::NotAvailable);
    }
    if w.modules().running(item_id).is_some() {
        return Err(Refusal::Active);
    }
    if crate::item::is_ship_fitting_flag(item.flag_id)
        && crate::modules::loaded(w.store(), item.location_id, item.flag_id).is_some()
    {
        return Err(Refusal::Loaded);
    }
    let to = Slot::new(c.character_id, station, crate::model::FLAG_HANGAR);
    Ok(match quantity {
        Some(q) if !item.singleton && q > 0 && q < item.quantity => vec![Edit::Split {
            item_id,
            quantity: q,
            to,
        }],
        _ => vec![Edit::Move { item_id, to }],
    })
}

// ---------------------------------------------------------------------------
// The cargo hold
// ---------------------------------------------------------------------------

/// A ship's cargo capacity in m3: **attribute 38, `capacity`**, which is
/// what the client itself measures a hold against --
/// `invCacheContainer.GetCapacity` asks `dogmaIM.GetCapacityForItem(itemID,
/// const.attributeCapacity)` for `flagCargo` and falls back to
/// `evetypes.GetCapacity(typeID)` (`invCache.py:1380`-`:1383`) [V]. Ours is
/// the type's own value ([`crate::item::ItemType::capacity`], from
/// `types.fsdbinary` [V-data]: a Merlin's is 150). **[A]** nothing modifies
/// it: no cargo expander or hull bonus is modelled. Zero for anything that
/// is not a known item, which refuses every move into it.
///
/// The type from the world's tables (CP58c).
pub fn base_cargo_capacity(content: &Content, store: &Store, ship_id: i64) -> f64 {
    store
        .get(ship_id)
        .and_then(|s| item_type(content, s.type_id))
        .map_or(0.0, |t| t.capacity)
}

/// [`base_cargo_capacity`] with the ship's dogma applied: the **modified**
/// `capacity` (38) -- expanded holds, hull cargo bonuses -- when
/// `crate::dogma` has a sheet for the ship, which is the number the client's
/// `dogmaIM.GetCapacityForItem` route reads. [`base_cargo_capacity`]
/// otherwise.
pub fn cargo_capacity_in(w: &Registry, ship_id: i64) -> f64 {
    crate::dogma::sheet(w, ship_id)
        .map(|s| s.ship(crate::dogma::attr::CAPACITY))
        .unwrap_or_else(|| base_cargo_capacity(w.content(), w.store(), ship_id))
}

/// The free m3 in a ship's cargo hold. Loaded charges are at fitting flags,
/// so they are never counted against it.
///
/// Against [`cargo_capacity_in`] (the fitted capacity) and every type the
/// world's `Content` knows.
pub fn cargo_free_in(w: &Registry, ship_id: i64) -> f64 {
    (cargo_capacity_in(w, ship_id) - w.store().used_volume_in(w.content(), ship_id, FLAG_CARGO))
        .max(0.0)
}

/// How many units of `unit_volume` fit in `free` m3, by the client's own
/// arithmetic: `int(round(free / (volume or 1), 7))`
/// (`invControllers._CalculateMaxQuantity`, `:373`) [V]. Matching it
/// matters: a server that refused a quantity the client's prompt had just
/// offered as the maximum would put the player in a loop.
pub fn units_that_fit(free: f64, unit_volume: f64) -> i64 {
    // `volume or 1`: a zero volume divides by one, as the client's does.
    let per = if unit_volume == 0.0 { 1.0 } else { unit_volume };
    let n = ((free / per) * 1e7).round() / 1e7;
    if n.is_nan() || n <= 0.0 {
        0
    } else {
        n.floor() as i64
    }
}

/// Refuse `n` units of `type_id` into `ship_id`'s hold if they do not fit:
/// the unit volume and the hold's (unmodified) capacity from the world's
/// tables (CP58c).
pub fn check_cargo_room_in(
    content: &Content,
    store: &Store,
    ship_id: i64,
    type_id: i64,
    n: i64,
) -> Result<(), Refusal> {
    let unit = item_type(content, type_id).map_or(0.0, |t| t.volume);
    let free = (base_cargo_capacity(content, store, ship_id)
        - store.used_volume_in(content, ship_id, FLAG_CARGO))
    .max(0.0);
    room_for(free, unit, n)
}

/// [`check_cargo_room_in`] for any hold: the hull's capacity in `hold_flag`
/// ([`crate::item::hold_capacity_in`]) less what is in it. The cargo hold is
/// [`check_cargo_room_in`] itself, so its numbers do not move.
pub fn check_room_in_hold(
    content: &Content,
    store: &Store,
    ship_id: i64,
    type_id: i64,
    n: i64,
    hold_flag: i64,
) -> Result<(), Refusal> {
    if hold_flag == FLAG_CARGO {
        return check_cargo_room_in(content, store, ship_id, type_id, n);
    }
    let unit = item_type(content, type_id).map_or(0.0, |t| t.volume);
    let ship_type = store.get(ship_id).map_or(0, |s| s.type_id);
    let free = (crate::item::hold_capacity_in(content, ship_type, hold_flag)
        - store.used_volume_in(content, ship_id, hold_flag))
    .max(0.0);
    room_for(free, unit, n)
}

fn room_for(free: f64, unit: f64, n: i64) -> Result<(), Refusal> {
    if units_that_fit(free, unit) < n {
        return Err(Refusal::NoRoom {
            available: free,
            volume: unit * n as f64,
        });
    }
    Ok(())
}

/// One of my ships, assembled, in this station's hangar: one whose hold I
/// can open while docked here.
fn parked_here(store: &Store, c: &Character, station_id: i64, ship_id: i64) -> bool {
    store.get(ship_id).is_some_and(|s| {
        s.owner_id == c.character_id
            && s.category_id == crate::model::CATEGORY_SHIP
            && s.singleton
            && s.location_id == station_id
            && s.flag_id == crate::model::FLAG_HANGAR
    })
}

/// Put an item into `ship_id`'s cargo hold, or say why not.
///
/// The call is `Add(itemID, sourceID, qty=n, flag=flagCargo)` on the
/// ship's inventory (`invControllers.py:267`, `ShipCargo.locationFlag =
/// const.flagCargo` at `:738`, bound by `GetInventoryFromId(shipID,
/// locationID=session.stationid)` at `:772`) [V]; `qty` is the whole stack
/// or what the quantity prompt chose. What may go in, all [A] beyond the
/// keys cited on [`Refusal`]:
///
/// - **the hold**: my active ship's anywhere, or -- docked -- another of my
///   ships parked in this hangar;
/// - **from the station hangar** (docked), or **from this ship's own
///   fitting slots** (docked: a module dragged from the fitting window to
///   the hold arrives here through `UnloadModuleToContainer`'s
///   `Add(itemID, shipID, ...)`, `clientDogmaLocation.py:1335`-`:1339`) [V];
/// - **a split within the hold** (shift-drag in the same window -- the only
///   cargo-to-cargo move the client sends, `__AddItem` returns early for a
///   whole stack dropped where it is, `invControllers.py:249`-`:251`) [V],
///   in space as well. A whole stack "moved" where it already is changes
///   nothing.
///
/// - **from a wreck in space** (P2.4): `combat::plan_loot`'s rules, not
///   these.
///
/// Ship-to-ship (another ship's hold, in space or docked) and a container
/// in the hangar are not here; see `docs/lanes/inventory.md` (P5.12).
/// (`inventory.rs` calls [`plan_to_hold`] with the flag it was sent; this is
/// the cargo case by name, which the tests and the notes cite.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn plan_to_cargo(
    w: &Registry,
    c: Option<&Character>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
) -> Result<Vec<Edit>, Refusal> {
    plan_to_hold(w, c, ship_id, item_id, quantity, FLAG_CARGO)
}

/// Whether a flag is one of the ship's plain holds an item moves between
/// without leaving the ship: cargo and every specialised hold of
/// [`crate::item::HOLDS`] (fuel, mining, gas, mineral, salvage, ship, ...).
/// The drone bay, the maintenance bay and the fleet hangar keep their own
/// routes (`crate::drones`, `crate::ship_config`).
pub fn is_plain_hold(flag_id: i64) -> bool {
    crate::item::hold_of(flag_id).is_some()
        && flag_id != crate::drones::FLAG_DRONE_BAY
        && flag_id != crate::ship_config::FLAG_SMB
        && flag_id != crate::ship_config::FLAG_FLEET_HANGAR
}

/// [`plan_to_cargo`] for a hold: [`FLAG_CARGO`] or the fuel bay
/// ([`crate::item::FLAG_FUEL_BAY`], CAP-B).
///
/// **The fuel bay** is `invCtrl.ShipFuelBay`, a `BaseShipBay` with
/// `locationFlag = flagSpecializedFuelBay` [V] (`invControllers.py:669`):
/// a drop on it is `Add(itemID, sourceID, qty=n, flag=133)` on the ship's
/// inventory like cargo's. It takes what `ShouldAllowAdd` lets in -- group
/// Ice Product only ([`crate::item::hold_accepts`]) -- and holds at most the
/// hull's `specialFuelBayCapacity`
/// ([`crate::item::hold_capacity_in`]); a hull without one has no bay and
/// refuses with no room. Beyond the sources of [`plan_to_cargo`], an item
/// may come from **another plain hold of the same ship** (cargo to bay and
/// back), in space as well [A: the client sends the same `Add` from the
/// hold window; nothing in the client limits it to a station].
pub fn plan_to_hold(
    w: &Registry,
    c: Option<&Character>,
    ship_id: i64,
    item_id: i64,
    quantity: Option<i64>,
    hold_flag: i64,
) -> Result<Vec<Edit>, Refusal> {
    let c = c.ok_or(Refusal::NoCharacter)?;
    // Another ship's hold only while docked beside it: `station_id` is "my
    // station" and stays set in space (`docs/lanes/docked.md`).
    let reachable = ship_id == c.ship_id
        || w.docked_station(c.character_id)
            .is_some_and(|st| parked_here(w.store(), c, st, ship_id));
    let ship = w.store().get(ship_id).filter(|s| {
        reachable && s.owner_id == c.character_id && s.category_id == crate::model::CATEGORY_SHIP
    });
    if ship.is_none() {
        return Err(Refusal::Unsupported(
            "Only your own ship's cargo hold can take items here.",
        ));
    }
    if item_id == ship_id {
        return Err(Refusal::InsideItself);
    }
    if item_id == c.ship_id {
        return Err(Refusal::ActiveShip);
    }
    let item = w.store().get(item_id).ok_or(Refusal::NotAvailable)?;
    // **Loot** (P2.4): an item lying in a wreck is nobody's to refuse on
    // ownership -- whether the taker has the right is the client's question
    // and the consequence is crimewatch's (`combat::plan_loot`).
    // A hacked can's loot (HK3) is taken the same way.
    if crate::hacking_loot::loot_source(w, item.location_id).is_some() {
        return crate::combat::plan_loot(w, c, ship_id, item, quantity);
    }
    if item.owner_id != c.character_id || crate::item::is_loaded_charge(item) {
        return Err(Refusal::NotAvailable);
    }
    let to = Slot::new(c.character_id, ship_id, hold_flag);
    let part = quantity.filter(|q| !item.singleton && *q > 0 && *q < item.quantity);
    if item.slot() == to {
        // Already in this hold: a split, or nothing.
        return Ok(part.map_or_else(Vec::new, |q| {
            vec![Edit::Split {
                item_id,
                quantity: q,
                to,
            }]
        }));
    }
    // The type must be one this hold takes (`DoesAcceptItem`, the client's
    // `ShouldAllowAdd`) [V]; checked before any move, so a refused drop
    // changes nothing.
    if hold_flag != FLAG_CARGO
        && crate::item::hold_accepts(hold_flag, item.category_id, item.group_id) != Some(true)
    {
        return Err(Refusal::WrongHold {
            type_id: item.type_id,
        });
    }
    // Between two holds of one ship: no station needed.
    let from_own_hold =
        item.location_id == ship_id && item.flag_id != hold_flag && is_plain_hold(item.flag_id);
    if from_own_hold {
        let n = part.unwrap_or(item.quantity);
        check_room_in_hold(w.content(), w.store(), ship_id, item.type_id, n, hold_flag)?;
        return Ok(match part {
            Some(q) => vec![Edit::Split {
                item_id,
                quantity: q,
                to,
            }],
            None => vec![Edit::Move { item_id, to }],
        });
    }
    // **Docked by `Registry::docked_station`, not by `station_id`**, which
    // is the pilot's home and is never cleared ([`refuse_in_space`]): without this a fitted
    // module went to the hold in space, and a hangar stack reached it from
    // across the system.
    let in_space =
        Refusal::Unsupported("In space, only a split inside your own cargo hold is supported.");
    let station = w.docked_station(c.character_id).ok_or(in_space)?;
    let from_hangar = item.location_id == station && item.flag_id == crate::model::FLAG_HANGAR;
    let from_slot = item.location_id == ship_id && crate::item::is_ship_fitting_flag(item.flag_id);
    if !from_hangar && !from_slot {
        return Err(Refusal::NotAvailable);
    }
    if from_slot {
        if w.modules().running(item_id).is_some() {
            return Err(Refusal::Active);
        }
        if crate::modules::loaded(w.store(), ship_id, item.flag_id).is_some() {
            return Err(Refusal::Loaded);
        }
    }
    if item.category_id == crate::model::CATEGORY_SHIP && item.singleton {
        return Err(Refusal::AssembledShip);
    }
    let n = part.unwrap_or(item.quantity);
    check_room_in_hold(w.content(), w.store(), ship_id, item.type_id, n, hold_flag)?;
    Ok(match part {
        Some(q) => vec![Edit::Split {
            item_id,
            quantity: q,
            to,
        }],
        None => vec![Edit::Move { item_id, to }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hull `/createitem` can grant has fitting numbers, and so does
    /// every hull Destiny can fly -- the two tables stay one list.
    #[test]
    fn every_spawnable_hull_has_fitting_numbers() {
        let content = Content::fixture();
        let hull = |t| hull_in(&content, t);
        for t in crate::item::SPAWNABLE
            .iter()
            .filter(|t| t.category_id == crate::model::CATEGORY_SHIP)
        {
            assert!(hull(t.type_id).is_some(), "{} has no row", t.name);
        }
        for h in crate::destiny::hull::FIXTURE {
            assert!(hull(h.type_id).is_some(), "hull {} has no row", h.type_id);
        }
    }

    /// CP58c: a hull the hand table lacks fits from the world's typedata,
    /// and a type neither holds has no fitting numbers.
    #[test]
    fn a_hull_fits_from_the_worlds_typedata() {
        let content = Content::fixture();
        let table = crate::typedata::installed_in(&content).table();
        let id = table
            .types()
            .map(|t| t.type_id)
            .find(|&t| crate::typedata::hull_fit_in(&content, t).is_some())
            .expect("the typedata fixture has a hull");
        assert!(HULLS.iter().all(|h| h.type_id != id), "not a hand row");
        assert_eq!(
            hull_in(&content, id),
            crate::typedata::hull_fit_in(&content, id)
        );
        assert!(hull_in(&content, -1).is_none());
    }

    /// CP58c: the hold and the unit volume come from the world's tables. A
    /// ship whose type only the world's typedata holds has that type's
    /// capacity, and exactly as many units fit as the client's arithmetic
    /// says -- one more is refused.
    #[test]
    fn the_cargo_check_reads_the_worlds_tables() {
        let content = Content::fixture();
        let installed = crate::typedata::installed_in(&content);
        let ship_type = installed
            .table()
            .types()
            .find(|t| installed.hull_fit(t.type_id).is_some())
            .expect("a fixture hull");
        let ship_t = item_type(&content, ship_type.type_id).expect("an item type");
        let unit_t = installed
            .table()
            .types()
            .filter_map(|t| item_type(&content, t.type_id))
            .find(|t| t.volume > 0.0 && t.volume < ship_t.capacity)
            .expect("a fixture type that fits");
        let mut store = Store::new();
        let ship = 100_000_950;
        store.insert(Item {
            item_id: ship,
            type_id: ship_t.type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: ship_t.group_id,
            category_id: ship_t.category_id,
            singleton: true,
        });
        assert_eq!(base_cargo_capacity(&content, &store, ship), ship_t.capacity);
        let n = units_that_fit(ship_t.capacity, unit_t.volume);
        assert!(n >= 1);
        check_cargo_room_in(&content, &store, ship, unit_t.type_id, n).expect("fits");
        assert!(matches!(
            check_cargo_room_in(&content, &store, ship, unit_t.type_id, n + 1),
            Err(Refusal::NoRoom { .. })
        ));
    }

    /// The three hulls the spec quotes [V-data] (§3.3).
    #[test]
    fn the_spec_hulls_read_as_dumped() {
        let content = Content::fixture();
        let hull = |t| hull_in(&content, t);
        let merlin = hull(603).expect("Merlin");
        assert_eq!(
            (merlin.hi, merlin.med, merlin.low, merlin.turrets),
            (3, 4, 3, 3)
        );
        assert_eq!((merlin.cpu_output, merlin.power_output), (180.0, 40.0));
        let rifter = hull(587).expect("Rifter");
        assert_eq!((rifter.low, rifter.launchers), (4, 2));
        let pod = hull(670).expect("Capsule");
        assert_eq!((pod.hi, pod.med, pod.low), (0, 0, 0));
    }

    /// The client's own "how many fit" (`invControllers.py:373`,
    /// `int(round(free / (volume or 1), 7))`) [V]: a quotient a hair under
    /// a whole number is that whole number, and a zero volume divides by one.
    #[test]
    fn units_that_fit_is_the_clients_arithmetic() {
        assert_eq!(units_that_fit(150.0, 0.1), 1500);
        assert_eq!(units_that_fit(0.3, 0.0025), 120);
        assert_eq!(units_that_fit(0.29, 0.1), 2);
        assert_eq!(units_that_fit(0.0, 0.1), 0);
        assert_eq!(units_that_fit(7.6, 0.0), 7, "int() truncates");
    }

    /// The flag ranges of `inventorycommon/const` [V].
    #[test]
    fn a_flag_names_its_rack_and_index() {
        assert_eq!(Rack::of_flag(11), Some((Rack::Low, 0)));
        assert_eq!(Rack::of_flag(26), Some((Rack::Medium, 7)));
        assert_eq!(Rack::of_flag(29), Some((Rack::High, 2)));
        assert_eq!(Rack::of_flag(4), None);
        assert_eq!(Rack::of_flag(35), None);
    }

    /// Test 20: after a restart the active ship is the one holding the
    /// capsule -- derived, because `Character.ship_id` is not stored (§7).
    #[test]
    fn a_restore_finds_the_ship_the_capsule_is_in() {
        let mut reg = Registry::new(crate::model::SandboxConfig::default());
        let c = reg.config().characters[0].clone();
        let station = c.station_id.expect("docked");
        let merlin = 100_000_900;
        let t = item_type(reg.content(), 603).expect("Merlin");
        let mut items = reg.store().items().to_vec();
        let pod = items
            .iter_mut()
            .find(|i| i.item_id == c.ship_id)
            .expect("the seeded capsule");
        pod.location_id = merlin;
        pod.flag_id = FLAG_CAPSULE;
        items.push(Item {
            item_id: merlin,
            type_id: 603,
            owner_id: c.character_id,
            location_id: station,
            flag_id: crate::model::FLAG_HANGAR,
            quantity: 1,
            group_id: t.group_id,
            category_id: t.category_id,
            singleton: true,
        });
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: merlin + 1,
            ..crate::store::Snapshot::default()
        });
        restore_active_ships(&mut reg);
        let now = reg.character(c.character_id).expect("still here");
        assert_eq!((now.ship_id, now.ship_type_id), (merlin, 603));
        // And it is idempotent: deriving again from the ship finds the pod.
        restore_active_ships(&mut reg);
        assert_eq!(
            reg.character(c.character_id).map(|c| c.ship_id),
            Some(merlin)
        );
    }

    // ---- CAP-B: capital hulls, the module-size gate, the fuel bay ----

    const CAP_HULL: i64 = 990_930_001;
    const SMALL_HULL: i64 = 990_930_002;
    const BIG_MODULE: i64 = 990_930_003;
    const SMALL_MODULE: i64 = 990_930_004;
    const FUEL: i64 = 990_930_005;
    const ROCKS: i64 = 990_930_006;
    const FREIGHTER_HULL: i64 = 990_930_007;
    /// Fits only hulls of the frigate group (`canFitShipGroup01`).
    const GROUP_MODULE: i64 = 990_930_008;
    /// Fits only the capital fixture hull (`canFitShipType1`).
    const TYPE_MODULE: i64 = 990_930_009;
    /// Names a group the fixture hulls are not in, and the small hull's type.
    const EITHER_MODULE: i64 = 990_930_010;
    /// Invented: the fuel bay of [`CAP_HULL`], and the unit volume of
    /// [`FUEL`], so that exactly [`BAY_UNITS`] units fit.
    const BAY_M3: f64 = 100.0;
    const FUEL_M3: f64 = 0.5;
    const BAY_UNITS: i64 = 200;
    const BIG_M3: f64 = 4000.0;
    const CAPITAL_GROUP: i64 = 485;
    const FRIGATE_GROUP: i64 = 25;
    const FREIGHTER_GROUP: i64 = 513;
    const ICE_PRODUCT_GROUP: i64 = crate::item::GROUP_ICE_PRODUCT;
    const ALICE: i64 = 990_930_100;
    const STATION: i64 = 990_930_101;

    fn cap_content() -> Content {
        let bay = crate::item::ATTR_FUEL_BAY_CAPACITY;
        let hull = |id: i64, group: i64, extra: &str| {
            format!(
                "type\t{id}\t{group}\t6\t1\t1\t1000000.0\t500.0\t1200000000.0\t300.0\t0\t\t\
                 9=1000000.0;11=45000.0;14=8;13=4;12=6;37=100.0;48=125000.0;552=3000.0;\
                 600=2.0;1281=3.0{extra}\tFixture Hull {id}\n"
            )
        };
        let module = |id: i64, m3: f64, extra: &str| {
            format!(
                "type\t{id}\t55\t7\t1\t1\t{m3}\t0.0\t0.0\t1.0\t0\t12\t50=1.0{extra}\tFixture Module {id}\n"
            )
        };
        let rows = format!(
            "{}{}{}{}{}{}{}{}\
             type\t{FUEL}\t{ICE_PRODUCT_GROUP}\t17\t1\t1\t{FUEL_M3}\t0.0\t0.0\t1.0\t0\t\t\tFixture Fuel\n\
             type\t{ROCKS}\t18\t4\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t\t\tFixture Rocks\n",
            hull(CAP_HULL, CAPITAL_GROUP, &format!(";{bay}={BAY_M3}")),
            hull(SMALL_HULL, FRIGATE_GROUP, ""),
            hull(FREIGHTER_HULL, FREIGHTER_GROUP, ""),
            module(BIG_MODULE, BIG_M3, ""),
            module(SMALL_MODULE, 5.0, ""),
            module(GROUP_MODULE, 5.0, &format!(";1298={FRIGATE_GROUP}.0")),
            module(TYPE_MODULE, 5.0, &format!(";1302={CAP_HULL}.0")),
            module(
                EITHER_MODULE,
                5.0,
                &format!(";1299=1.0;1303={SMALL_HULL}.0"),
            ),
        );
        let text = format!("{}{rows}", crate::typedata::FIXTURE);
        let table = crate::typedata::Table::parse(&text).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// Alice, docked, flying `hull`, with a big and a small module, and
    /// `fuel` units of fuel and some rocks in the hangar.
    fn alice_in(hull: i64, fuel: i64) -> (Registry, Vec<i64>) {
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            station_id: Some(STATION),
            ..Character::default()
        };
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![alice],
            ..crate::model::SandboxConfig::default()
        })
        .with_content(std::sync::Arc::new(cap_content()));
        let station = reg
            .character(ALICE)
            .and_then(|c| c.station_id)
            .expect("docked");
        let mut items = reg.store().items().to_vec();
        let mut ids = Vec::new();
        for (n, (type_id, flag, quantity, singleton)) in [
            (hull, crate::model::FLAG_HANGAR, 1, true),
            (BIG_MODULE, crate::model::FLAG_HANGAR, 1, true),
            (SMALL_MODULE, crate::model::FLAG_HANGAR, 1, true),
            (FUEL, crate::model::FLAG_HANGAR, fuel, false),
            (ROCKS, crate::model::FLAG_HANGAR, 10, false),
            (GROUP_MODULE, crate::model::FLAG_HANGAR, 1, true),
            (TYPE_MODULE, crate::model::FLAG_HANGAR, 1, true),
            (EITHER_MODULE, crate::model::FLAG_HANGAR, 1, true),
        ]
        .into_iter()
        .enumerate()
        {
            let id = 990_931_000 + n as i64;
            let t = item_type(reg.content(), type_id).expect("a fixture type");
            items.push(Item {
                item_id: id,
                type_id,
                owner_id: ALICE,
                location_id: station,
                flag_id: flag,
                quantity,
                group_id: t.group_id,
                category_id: t.category_id,
                singleton,
            });
            ids.push(id);
        }
        reg.apply_snapshot(crate::store::Snapshot {
            items,
            next_item_id: 990_932_000,
            ..crate::store::Snapshot::default()
        });
        let c = reg.character_mut(ALICE).expect("Alice");
        c.ship_id = ids[0];
        c.ship_type_id = hull;
        (reg, ids)
    }

    fn fit_plan(reg: &Registry, ids: &[i64], module: usize) -> Result<Fit, Refusal> {
        plan_fit_in(
            reg.content(),
            reg.store(),
            reg.character(ALICE),
            ids[0],
            ids[module],
            FLAG_AUTOFIT,
        )
    }

    /// `IsModuleTooBigForShip` [V]: strictly over 3500 m3, and only for a
    /// ship that is neither one of the eight capital groups nor a structure.
    /// Freighters are industrial class, so the gate holds on them.
    #[test]
    fn the_size_gate_is_the_clients_predicate() {
        let frigate = (FRIGATE_GROUP, crate::model::CATEGORY_SHIP);
        assert!(!module_too_big(
            MAX_NON_CAPITAL_MODULE_M3,
            frigate.0,
            frigate.1
        ));
        assert!(module_too_big(
            MAX_NON_CAPITAL_MODULE_M3 + 0.1,
            frigate.0,
            frigate.1
        ));
        for g in crate::typedata::CAPITAL_GROUPS {
            assert!(!module_too_big(1e9, g, crate::model::CATEGORY_SHIP), "{g}");
        }
        assert!(module_too_big(
            MAX_NON_CAPITAL_MODULE_M3 + 1.0,
            FREIGHTER_GROUP,
            crate::model::CATEGORY_SHIP
        ));
        assert!(!module_too_big(
            1e9,
            FRIGATE_GROUP,
            crate::typedata::CATEGORY_STRUCTURE
        ));
    }

    /// The test `a_big_module_needs_a_capital_hull` (spec §3.6): the same
    /// module is refused on a frigate and on a freighter with the key the
    /// client names, fits a capital hull, and a small one fits anywhere.
    #[test]
    fn a_big_module_needs_a_capital_hull() {
        for hull in [SMALL_HULL, FREIGHTER_HULL] {
            let (reg, ids) = alice_in(hull, 0);
            assert!(matches!(
                fit_plan(&reg, &ids, 1),
                Err(Refusal::ModuleTooBig {
                    type_id: BIG_MODULE,
                    ..
                })
            ));
            assert!(fit_plan(&reg, &ids, 2).is_ok(), "a small module fits");
        }
        let (reg, ids) = alice_in(CAP_HULL, 0);
        let fit = fit_plan(&reg, &ids, 1).expect("a capital hull takes it");
        assert_eq!(fit.flag_id, Rack::High.first_flag());
    }

    /// `CanFitModuleToShipTypeOrGroup` [V]: a module naming no hull fits any;
    /// one that names groups or types fits a hull in one of the groups, or
    /// of one of the types; a hull outside both is refused with the client's
    /// own key and arguments, and nothing moves.
    #[test]
    fn a_module_that_names_hulls_is_refused_on_any_other() {
        // (hull, module index in alice_in's ids, fits)
        let cases = [
            (SMALL_HULL, 5, true),
            (FREIGHTER_HULL, 5, false),
            (CAP_HULL, 5, false),
            (CAP_HULL, 6, true),
            (SMALL_HULL, 6, false),
            (FREIGHTER_HULL, 6, false),
            // Group 1 is no fixture hull's group; the type matches the small hull.
            (SMALL_HULL, 7, true),
            (CAP_HULL, 7, false),
            // Unrestricted modules fit everywhere.
            (FREIGHTER_HULL, 2, true),
        ];
        for (hull, module, fits) in cases {
            let (reg, ids) = alice_in(hull, 0);
            let got = fit_plan(&reg, &ids, module);
            if fits {
                assert!(got.is_ok(), "hull {hull} module {module}: {got:?}");
            } else {
                assert_eq!(
                    got.unwrap_err(),
                    Refusal::WrongKind {
                        type_id: [0, 0, 0, 0, 0, GROUP_MODULE, TYPE_MODULE, EITHER_MODULE][module],
                        ship_type_id: hull,
                    },
                    "hull {hull} module {module}"
                );
            }
        }
    }

    /// A module fitted illegally in a saved world may still move slot to
    /// slot; from the hangar it is refused.
    #[test]
    fn an_illegally_fitted_module_may_still_change_slot() {
        let (reg, ids) = alice_in(CAP_HULL, 0);
        assert!(fit_plan(&reg, &ids, 5).is_err(), "refused from the hangar");
        let mut moved = 0;
        for rack in [Rack::Low, Rack::Medium, Rack::High] {
            let (mut reg, ids) = alice_in(CAP_HULL, 0);
            let mut items = reg.store().items().to_vec();
            for i in &mut items {
                if i.item_id == ids[5] {
                    i.location_id = ids[0];
                    i.flag_id = rack.first_flag();
                }
            }
            reg.apply_snapshot(crate::store::Snapshot {
                items,
                next_item_id: 990_932_000,
                ..crate::store::Snapshot::default()
            });
            let c = reg.character_mut(ALICE).expect("Alice");
            c.ship_id = ids[0];
            c.ship_type_id = CAP_HULL;
            if fit_plan(&reg, &ids, 5).is_ok() {
                moved += 1;
            }
        }
        assert_eq!(moved, 3, "fitted anywhere, it may be re-seated");
    }

    /// The attribute lists hold the client's names: canFitShipGroup01..20,
    /// canFitShipType1..12 and fitsToShipType, each id once.
    #[test]
    fn the_can_fit_attribute_lists_are_complete() {
        assert_eq!(CAN_FIT_GROUP_ATTRS.len(), 20);
        assert_eq!(CAN_FIT_TYPE_ATTRS.len(), 13);
        let all: std::collections::BTreeSet<i64> = CAN_FIT_GROUP_ATTRS
            .iter()
            .chain(CAN_FIT_TYPE_ATTRS)
            .copied()
            .collect();
        assert_eq!(all.len(), 33, "no id twice, none in both");
    }

    /// Only an attribute the module carries is a name; absent means any hull.
    #[test]
    fn only_a_carried_attribute_names_a_hull() {
        let named = |a: i64| (a == 1298).then_some(1.0);
        assert!(!hull_allows(named, 25, 7));
        assert!(hull_allows(named, 1, 7));
        // A default of zero is not a name: only a carried attribute counts.
        assert!(hull_allows(|_| None, 25, 7));
    }

    /// The refusal is the client's own key with its two arguments.
    #[test]
    fn the_size_refusal_names_the_module_and_the_ship() {
        let e = Refusal::ModuleTooBig {
            type_id: BIG_MODULE,
            ship_type_id: SMALL_HULL,
        }
        .to_error();
        let ty = |t: i64| {
            PyValue::Tuple(vec![
                PyValue::Int(crate::modules::UE_TYPEID),
                PyValue::Int(t),
            ])
        };
        let want = ServiceError::Raise(eve_proto::user_error(
            "ModuleTooBigForThisShip",
            crate::pyutil::pypairs(vec![("item", ty(BIG_MODULE)), ("ship", ty(SMALL_HULL))]),
        ));
        assert_eq!(format!("{e:?}"), format!("{want:?}"));
    }

    /// A capital hull is an ordinary ship to the typedata: complete ball
    /// and fitting numbers, a capital group, and its fuel bay; a frigate is
    /// not capital; a module is not a hull.
    #[test]
    fn a_capital_hull_reads_as_a_complete_ship() {
        let content = cap_content();
        assert!(crate::typedata::is_capital_hull_in(&content, CAP_HULL));
        assert!(!crate::typedata::is_capital_hull_in(&content, SMALL_HULL));
        assert!(!crate::typedata::is_capital_hull_in(
            &content,
            FREIGHTER_HULL
        ));
        assert!(!crate::typedata::is_capital_hull_in(&content, BIG_MODULE));
        assert!(crate::typedata::hull_gaps_in(&content, CAP_HULL).is_empty());
        assert_eq!(
            crate::typedata::hull_gaps_in(&content, BIG_MODULE),
            vec!["not a ship type"]
        );
        let fit = hull_in(&content, CAP_HULL).expect("fitting numbers");
        assert_eq!((fit.hi, fit.med, fit.low), (8, 4, 6));
        assert_eq!(
            crate::item::hold_capacity_in(&content, CAP_HULL, crate::item::FLAG_FUEL_BAY),
            BAY_M3
        );
        assert_eq!(
            crate::item::hold_capacity_in(&content, SMALL_HULL, crate::item::FLAG_FUEL_BAY),
            0.0,
            "no attribute, no bay"
        );
        assert_eq!(
            crate::item::hold_capacity_in(&content, CAP_HULL, FLAG_CARGO),
            500.0,
            "cargo is the type's own capacity"
        );
    }

    fn to_bay(
        reg: &mut Registry,
        ids: &[i64],
        item: usize,
        qty: Option<i64>,
    ) -> Result<(), Refusal> {
        let plan = plan_to_hold(
            reg,
            reg.character(ALICE),
            ids[0],
            ids[item],
            qty,
            crate::item::FLAG_FUEL_BAY,
        )?;
        reg.apply_items(&plan).expect("a plan the store takes");
        Ok(())
    }

    fn units_at(reg: &Registry, ship: i64, flag: i64, type_id: i64) -> i64 {
        reg.store()
            .items()
            .iter()
            .filter(|i| i.location_id == ship && i.flag_id == flag && i.type_id == type_id)
            .map(|i| i.quantity)
            .sum()
    }

    /// The fuel bay takes the Ice Product group and nothing else [V], up to
    /// the hull's `specialFuelBayCapacity`, by the client's own arithmetic;
    /// a refused drop changes nothing.
    #[test]
    fn the_fuel_bay_takes_only_fuel_and_only_what_fits() {
        let bay = crate::item::FLAG_FUEL_BAY;
        let (mut reg, ids) = alice_in(CAP_HULL, BAY_UNITS + 50);
        let before = reg.store().items().to_vec();
        assert!(matches!(
            to_bay(&mut reg, &ids, 4, None),
            Err(Refusal::WrongHold { type_id: ROCKS })
        ));
        assert!(matches!(
            to_bay(&mut reg, &ids, 3, Some(BAY_UNITS + 1)),
            Err(Refusal::NoRoom { .. })
        ));
        assert_eq!(reg.store().items(), &before[..], "refusals move nothing");
        to_bay(&mut reg, &ids, 3, Some(BAY_UNITS)).expect("exactly full");
        assert_eq!(units_at(&reg, ids[0], bay, FUEL), BAY_UNITS);
        assert!(matches!(
            to_bay(&mut reg, &ids, 3, Some(1)),
            Err(Refusal::NoRoom { .. })
        ));
    }

    /// A hull with no `specialFuelBayCapacity` has no bay: the same drop is
    /// refused for want of room.
    #[test]
    fn a_hull_without_a_fuel_bay_takes_nothing_into_it() {
        let (mut reg, ids) = alice_in(SMALL_HULL, 10);
        assert!(matches!(
            to_bay(&mut reg, &ids, 3, None),
            Err(Refusal::NoRoom { .. })
        ));
    }

    /// Fuel moves between the bay and the cargo hold inside the ship, in
    /// space too, and out of the bay to the hangar when docked.
    #[test]
    fn fuel_moves_between_bay_cargo_and_hangar() {
        let bay = crate::item::FLAG_FUEL_BAY;
        let (mut reg, ids) = alice_in(CAP_HULL, 40);
        to_bay(&mut reg, &ids, 3, None).expect("hangar to bay");
        assert_eq!(units_at(&reg, ids[0], bay, FUEL), 40);
        let plan = plan_to_cargo(&reg, reg.character(ALICE), ids[0], ids[3], Some(15))
            .expect("bay to cargo");
        reg.apply_items(&plan).expect("applied");
        assert_eq!(units_at(&reg, ids[0], FLAG_CARGO, FUEL), 15);
        assert_eq!(units_at(&reg, ids[0], bay, FUEL), 25);
        let plan = plan_unfit(&reg, reg.character(ALICE), ids[3], None).expect("bay to hangar");
        reg.apply_items(&plan).expect("applied");
        assert_eq!(units_at(&reg, ids[0], bay, FUEL), 0);
        assert_eq!(units_at(&reg, STATION, crate::model::FLAG_HANGAR, FUEL), 25);
    }

    /// The refusal is the client's key with its one argument.
    #[test]
    fn the_hold_refusal_names_the_type() {
        let ty = PyValue::Tuple(vec![
            PyValue::Int(crate::modules::UE_TYPEID),
            PyValue::Int(ROCKS),
        ]);
        let want = ServiceError::Raise(eve_proto::user_error(
            "CannotPlaceTypeInFuelBay",
            crate::pyutil::pypairs(vec![("type", ty)]),
        ));
        let got = Refusal::WrongHold { type_id: ROCKS }.to_error();
        assert_eq!(format!("{got:?}"), format!("{want:?}"));
    }
}
