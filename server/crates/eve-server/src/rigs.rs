//! **Rigs** (X-RIG, `docs/lanes/x-rig.md`): the rules for fitting one into a
//! hull's rig slots, and what unfitting one does.
//!
//! `fitting.rs` fits the three module racks. A rig is a module (category 7)
//! whose slot effect is `rigSlot` (2663), so `modules::find_in` has no row
//! for it (`typedata::rack_of` leaves it out) and `fitting::plan_fit_in`
//! answers `CantFitModuleToThatShip`. This file is the other half. The
//! service hook (`service/inventory.rs` `fit_one`, `unfit_one` and the
//! `DestroyFitting` arm) asks [`is_rig`] first and calls in here for a rig.
//!
//! ## What the client says [V]
//!
//! - The slot effect is `effectRigSlot` 2663; the flags are `flagRigSlot0..`
//!   92.. (`inventorycommon/const`), and `rigSlotFlags` holds three entries
//!   (`item::is_ship_fitting_flag`). The client matches a rig to a rig slot
//!   itself (`fittingSlotController.py:89-101`) and asks "Fit to Active Ship"
//!   confirmation with `RigFittingInfo` before it sends the fit
//!   (`fittingUtil.py:237`).
//! - The client's dogma adds a rig's `upgradeCost` (1153) to the hull's
//!   `upgradeLoad` (1152) when the rig is fitted
//!   (`dogma_fitting.UpgradeFittedEffect`); the limit is the hull's
//!   `upgradeCapacity` (1132), **calibration**. A hull's `rigSize` (1547) is
//!   1 small to 4 capital and every rig carries one.
//! - **Unfitting a rig destroys it**, unless the hull is a Strategic Cruiser
//!   (`inventorycommon.util.ShipCanUnfitRigs`: `groupID == groupStrategicCruiser`).
//!   The client asks `RigUnFittingInfo` (OK/Cancel) and then calls
//!   `DestroyFitting(moduleID)` on the ship's inventory
//!   (`fittingSlotController.py:245-257`); a Strategic Cruiser's rig goes the
//!   ordinary unfit way (`UnfitModule`, an `Add` to the hangar).
//! - The refusal keys exist in the client's table: `CannotFitRigWrongSize`
//!   (args `item`, `rigSize`, `shipRigSize`) and `NotEnoughUpgradeCapacity`
//!   (arg `moduleName`), `findings/user-errors.tsv`. **They have no raise
//!   site in the client**: the check is the server's, so both the raise and
//!   the argument types are [A] (below).
//!
//! ## What is ours [A]
//!
//! - **The order of the checks**: the size before the calibration before the
//!   slot. Falsified by a TQ log of a rig that is both too big and over
//!   calibration, with the other message.
//! - **`item` is a type reference** (as `CantFitModuleToThatShip`'s) and the
//!   two sizes are plain integers. Falsified by a message that shows a raw
//!   `1` where it should name a size ("small").
//! - **The calibration limit is `<=`**: a rig that brings the load exactly to
//!   the hull's capacity fits. Falsified by a TQ hull at capacity that refuses.
//! - **A rig already fitted is not moved**: the client has no slot-to-slot
//!   drag for a rig (unfit destroys it), so a fitted rig dropped on another
//!   slot is `CantFitModuleToThatShip`.
//! - **An `Add` to the hangar of a fitted rig also destroys it** on a hull
//!   that cannot unfit rigs: only a stale window sends it, and the rule is
//!   the same.
//!
//! Skills are not checked at fit time (`docs/lanes/fitting.md` §2.2: the
//! client does not validate them there either and the server's fitting does
//! not), so a bot or a pilot without the Rigging skills fits a rig; the
//! rig's own bonuses are the dogma engine's.

use crate::content::Content;
use crate::fitting::{CATEGORY_MODULE, Fit, GROUP_CAPSULE, Refusal};
use crate::item::{Edit, Slot, Store};
use crate::model::Character;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `effectRigSlot` [V] (`dogma/const`).
pub const EFFECT_RIG_SLOT: i64 = 2663;
/// `flagRigSlot0` [V] (`inventorycommon/const`): rig slot `n` is `92 + n`.
pub const FIRST_RIG_FLAG: i64 = 92;
/// The highest rig flag the client's `fittingFlags` knows
/// (`item::is_ship_fitting_flag`: `rigSlotFlags` is 92-94) [V].
pub const LAST_RIG_FLAG: i64 = 94;
/// `upgradeCapacity` (1132): a hull's calibration [V-data].
pub const ATTR_CALIBRATION: i64 = 1132;
/// `upgradeCost` (1153): what a rig takes of it [V-data].
pub const ATTR_UPGRADE_COST: i64 = 1153;
/// `rigSize` (1547): 1 small, 2 medium, 3 large, 4 capital [V-data].
pub const ATTR_RIG_SIZE: i64 = 1547;
/// `groupStrategicCruiser` [V] (`inventorycommon/const`): the only hull
/// that unfits rigs without destroying them.
pub const GROUP_STRATEGIC_CRUISER: i64 = 963;

/// A rig type's fitting numbers, from the world's typedata.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigType {
    pub type_id: i64,
    /// Calibration it takes (`upgradeCost`).
    pub cost: f64,
    /// `rigSize`.
    pub size: i64,
}

/// A hull's rig numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullRigs {
    /// `rigSlots` (1137).
    pub slots: i64,
    /// `upgradeCapacity`: the calibration.
    pub calibration: f64,
    /// `rigSize`.
    pub size: i64,
}

/// Is `flag_id` one of the rig flags?
pub fn is_rig_flag(flag_id: i64) -> bool {
    (FIRST_RIG_FLAG..=LAST_RIG_FLAG).contains(&flag_id)
}

/// The rig this type is, if it is one: a module (category 7) with the
/// `rigSlot` effect.
pub fn rig_in(content: &Content, type_id: i64) -> Option<RigType> {
    let row = crate::typedata::installed_in(content)
        .table()
        .get(type_id)?;
    if row.category_id != CATEGORY_MODULE || !row.has_effect(EFFECT_RIG_SLOT) {
        return None;
    }
    Some(RigType {
        type_id,
        cost: row.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0),
        size: row.attribute(ATTR_RIG_SIZE).unwrap_or(0.0).round() as i64,
    })
}

/// Every published rig type with its group, ascending by type id: what a
/// bot picking rigs for a career chooses among.
pub fn published_in(content: &Content) -> Vec<(RigType, i64)> {
    crate::typedata::installed_in(content)
        .table()
        .types()
        .filter(|r| {
            r.published && r.category_id == CATEGORY_MODULE && r.has_effect(EFFECT_RIG_SLOT)
        })
        .map(|r| {
            (
                RigType {
                    type_id: r.type_id,
                    cost: r.attribute(ATTR_UPGRADE_COST).unwrap_or(0.0),
                    size: r.attribute(ATTR_RIG_SIZE).unwrap_or(0.0).round() as i64,
                },
                r.group_id,
            )
        })
        .collect()
}

/// A type's group, from the typedata.
pub fn group_in(content: &Content, type_id: i64) -> Option<i64> {
    crate::typedata::installed_in(content)
        .table()
        .get(type_id)
        .map(|r| r.group_id)
}

/// Is this a rig type? ([`rig_in`])
pub fn is_rig(content: &Content, type_id: i64) -> bool {
    rig_in(content, type_id).is_some()
}

/// A hull's rig numbers: the slots from the hull's fitting row, the
/// calibration and size from its typedata row. `None` for a type with no
/// typedata row (a hand-table hull with no bundle).
pub fn hull_rigs_in(content: &Content, type_id: i64) -> Option<HullRigs> {
    let row = crate::typedata::installed_in(content)
        .table()
        .get(type_id)?;
    let slots = crate::fitting::hull_in(content, type_id).map_or(0, |h| h.rig);
    Some(HullRigs {
        slots,
        calibration: row.attribute(ATTR_CALIBRATION).unwrap_or(0.0),
        size: row.attribute(ATTR_RIG_SIZE).unwrap_or(0.0).round() as i64,
    })
}

/// The calibration the rigs fitted to `ship_id` use.
pub fn calibration_used(content: &Content, store: &Store, ship_id: i64) -> f64 {
    store
        .contents_of(ship_id)
        .filter(|i| is_rig_flag(i.flag_id))
        .filter_map(|i| rig_in(content, i.type_id))
        .map(|r| r.cost)
        .sum()
}

/// A type's own attributes as the typedata holds them, for a fitted rig's
/// `ItemGetInfo` row (`modules::module_info`): `modules::find_in` has no row
/// for a rig, and godma reads an attribute a row leaves out as the
/// definition's default (`service.rs::capsule_dogma_row`), which would put a
/// rig's `upgradeCost` at 0 in the fitting window.
pub fn attributes_in(content: &Content, type_id: i64) -> Option<&[(i64, f64)]> {
    rig_in(content, type_id)?;
    crate::typedata::installed_in(content)
        .table()
        .get(type_id)
        .map(|r| r.attributes.as_slice())
}

/// Why a rig was refused: one of `fitting`'s refusals, or one of the two the
/// rigs add.
#[derive(Debug, Clone, PartialEq)]
pub enum RigRefusal {
    Generic(Refusal),
    /// `CannotFitRigWrongSize`.
    WrongSize {
        type_id: i64,
        rig_size: i64,
        ship_rig_size: i64,
    },
    /// `NotEnoughUpgradeCapacity`.
    Calibration {
        type_id: i64,
    },
}

impl From<Refusal> for RigRefusal {
    fn from(r: Refusal) -> Self {
        RigRefusal::Generic(r)
    }
}

impl RigRefusal {
    /// The `UserError` this is shown as.
    pub fn to_error(&self) -> ServiceError {
        let type_arg = |t: i64| {
            PyValue::Tuple(vec![
                PyValue::Int(crate::modules::UE_TYPEID),
                PyValue::Int(t),
            ])
        };
        match self {
            RigRefusal::Generic(r) => r.to_error(),
            RigRefusal::WrongSize {
                type_id,
                rig_size,
                ship_rig_size,
            } => ServiceError::Raise(eve_proto::user_error(
                "CannotFitRigWrongSize",
                crate::pyutil::pypairs(vec![
                    ("item", type_arg(*type_id)),
                    ("rigSize", PyValue::Int(*rig_size)),
                    ("shipRigSize", PyValue::Int(*ship_rig_size)),
                ]),
            )),
            RigRefusal::Calibration { type_id } => ServiceError::Raise(eve_proto::user_error(
                "NotEnoughUpgradeCapacity",
                crate::pyutil::pypairs(vec![("moduleName", type_arg(*type_id))]),
            )),
        }
    }
}

/// **May this rig go into the active ship, and where?** The checks, in
/// order: the pilot docked at a station with the ship, the ship not a pod,
/// the rig theirs and in the hangar, the hull has rig slots, the size, the
/// calibration, the slot (`flag_id` or, for [`crate::fitting::FLAG_AUTOFIT`],
/// the lowest free one).
pub fn plan_fit(
    content: &Content,
    store: &Store,
    c: Option<&Character>,
    ship_id: i64,
    module_id: i64,
    flag_id: i64,
) -> Result<Fit, RigRefusal> {
    let c = c.ok_or(Refusal::NoCharacter)?;
    let station = c.station_id.ok_or(Refusal::NotDocked)?;
    if ship_id != c.ship_id {
        return Err(Refusal::NotActiveShip.into());
    }
    let ship = store.get(ship_id).ok_or(Refusal::NotActiveShip)?;
    if ship.group_id == GROUP_CAPSULE {
        return Err(Refusal::Capsule.into());
    }
    let module = store.get(module_id).ok_or(Refusal::NotAvailable)?;
    let in_hangar = module.location_id == station && module.flag_id == crate::model::FLAG_HANGAR;
    if module.owner_id != c.character_id || !in_hangar {
        // A rig already fitted is not moved (module header).
        let fitted = module.location_id == ship_id && is_rig_flag(module.flag_id);
        return Err(if fitted && module.owner_id == c.character_id {
            Refusal::WrongKind {
                type_id: module.type_id,
                ship_type_id: ship.type_id,
            }
        } else {
            Refusal::NotAvailable
        }
        .into());
    }
    let wrong_kind = Refusal::WrongKind {
        type_id: module.type_id,
        ship_type_id: ship.type_id,
    };
    let rig = rig_in(content, module.type_id).ok_or(wrong_kind.clone())?;
    // `GetModuleErrorInfo` asks a rig the same hull question as a module.
    if !crate::fitting::hull_allows(
        |a| crate::typedata::own_attribute_in(content, module.type_id, a),
        ship.group_id,
        ship.type_id,
    ) {
        return Err(wrong_kind.into());
    }
    let hull = hull_rigs_in(content, ship.type_id).ok_or(Refusal::NoSlot)?;
    if hull.slots <= 0 {
        return Err(Refusal::NoSlot.into());
    }
    if rig.size != hull.size {
        return Err(RigRefusal::WrongSize {
            type_id: rig.type_id,
            rig_size: rig.size,
            ship_rig_size: hull.size,
        });
    }
    if calibration_used(content, store, ship_id) + rig.cost > hull.calibration {
        return Err(RigRefusal::Calibration {
            type_id: rig.type_id,
        });
    }
    let last = (FIRST_RIG_FLAG + hull.slots - 1).min(LAST_RIG_FLAG);
    let free = |f: i64| {
        !store
            .contents_of(ship_id)
            .any(|i| i.flag_id == f && i.item_id != module_id)
    };
    let flag_id = if flag_id == crate::fitting::FLAG_AUTOFIT {
        (FIRST_RIG_FLAG..=last)
            .find(|f| free(*f))
            .ok_or(Refusal::NoSlot)?
    } else if is_rig_flag(flag_id) {
        if flag_id > last {
            return Err(Refusal::NoSlot.into());
        }
        if !free(flag_id) {
            return Err(Refusal::Occupied.into());
        }
        flag_id
    } else {
        // A rack flag: the rig is the wrong kind for that slot.
        return Err(wrong_kind.into());
    };
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

/// What unfitting `item_id` does when it is a rig fitted to the pilot's
/// active ship, docked: destroy it, or on a Strategic Cruiser move it to the
/// hangar. `None` when it is not such a rig (the ordinary unfit applies).
pub fn plan_unfit(w: &Registry, c: &Character, item_id: i64) -> Option<Result<Vec<Edit>, Refusal>> {
    let item = w.store().get(item_id)?;
    if item.owner_id != c.character_id
        || item.location_id != c.ship_id
        || !is_rig_flag(item.flag_id)
        || !is_rig(w.content(), item.type_id)
    {
        return None;
    }
    if let Err(r) = crate::fitting::refuse_in_space(w, c) {
        return Some(Err(r));
    }
    let modular = w
        .store()
        .get(c.ship_id)
        .is_some_and(|s| s.group_id == GROUP_STRATEGIC_CRUISER);
    Some(Ok(if modular {
        match c.station_id {
            Some(station) => vec![Edit::Move {
                item_id,
                to: Slot::new(c.character_id, station, crate::model::FLAG_HANGAR),
            }],
            None => return Some(Err(Refusal::NotDocked)),
        }
    } else {
        vec![Edit::Destroy { item_id }]
    }))
}

/// The unfit arm's hook: a fitted rig is destroyed (or, on a Strategic
/// Cruiser, moved to the hangar). `None` for anything else.
pub fn unfit(
    reg: &mut Registry,
    actor: Option<i64>,
    item_id: i64,
) -> Option<Result<(), ServiceError>> {
    let c = actor.and_then(|id| reg.character(id)).cloned()?;
    let plan = match plan_unfit(reg, &c, item_id)? {
        Ok(plan) => plan,
        Err(r) => return Some(Err(r.to_error())),
    };
    // `apply_items` tells the client a destroyed rig left the ship (an
    // `OnItemChange` to the junkyard); its own dogma unfits on that, and
    // without one `upgradeLoad` kept the destroyed rig's cost.
    Some(match reg.apply_items(&plan) {
        Ok(_) => Ok(()),
        Err(e) => {
            crate::outln!("[rigs] unfit plan refused: {e}");
            Err(Refusal::Unsupported("The server could not unfit that rig.").to_error())
        }
    })
}

/// `invbroker` bound `DestroyFitting(itemID)` on the ship's inventory: the
/// call the fitting window sends when the pilot confirms `RigUnFittingInfo`
/// [V]. The reply is discarded; what the client acts on is the item change.
pub fn destroy_fitting(
    reg: &mut Registry,
    actor: Option<i64>,
    item_id: Option<i64>,
) -> Result<PyValue, ServiceError> {
    let Some(item_id) = item_id else {
        return Err(Refusal::NotAvailable.to_error());
    };
    unfit(reg, actor, item_id).unwrap_or(Err(Refusal::NotAvailable.to_error()))?;
    Ok(PyValue::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::model::SandboxConfig;

    // Invented ids throughout.
    const HULL_SMALL: i64 = 990_800_001;
    const HULL_MEDIUM: i64 = 990_800_002;
    const HULL_NO_RIGS: i64 = 990_800_003;
    const HULL_MODULAR: i64 = 990_800_004;
    const RIG_A: i64 = 990_800_011;
    const RIG_B: i64 = 990_800_012;
    const RIG_BIG: i64 = 990_800_013;
    const RIG_MEDIUM: i64 = 990_800_014;
    const GUN: i64 = 990_800_021;
    const CALIBRATION: f64 = 100.0;
    const COST_A: f64 = 60.0;
    const COST_B: f64 = 40.0;
    const COST_BIG: f64 = 50.0;

    /// The fixture's tables and a few invented hulls and rigs.
    pub(super) fn content() -> Content {
        let rows = format!(
            "{}type\t{HULL_SMALL}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
             1137=3.0;1132={CALIBRATION};1547=1.0\tInvented Small\n\
             type\t{HULL_MEDIUM}\t26\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
             1137=3.0;1132={CALIBRATION};1547=2.0\tInvented Medium\n\
             type\t{HULL_NO_RIGS}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
             1137=0.0;1132=0.0;1547=1.0\tInvented No Rigs\n\
             type\t{HULL_MODULAR}\t{GROUP_STRATEGIC_CRUISER}\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t\t\
             1137=3.0;1132={CALIBRATION};1547=2.0\tInvented Modular\n\
             type\t{RIG_A}\t773\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
             1153={COST_A};1547=1.0\tInvented Rig A\n\
             type\t{RIG_B}\t773\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
             1153={COST_B};1547=1.0\tInvented Rig B\n\
             type\t{RIG_BIG}\t773\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
             1153={COST_BIG};1547=1.0\tInvented Rig Big\n\
             type\t{RIG_MEDIUM}\t773\t7\t1\t1\t5.0\t0.0\t200.0\t100.0\t2663\t2663\t\
             1153=10.0;1547=2.0\tInvented Rig Medium\n",
            crate::typedata::FIXTURE
        );
        let table = crate::typedata::Table::parse(&rows).expect("the test rows parse");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    const STATION: i64 = 60_012_526;
    const ME: i64 = 90_000_001;

    /// A registry on `content()` with one pilot docked at `STATION`; the
    /// pilot's active ship is a new item of `hull`. Returns `(reg, ship)`.
    pub(super) fn docked_in(hull: i64) -> (Registry, i64) {
        let mut reg =
            Registry::new(SandboxConfig::default()).with_content(std::sync::Arc::new(content()));
        let me = reg.config().characters[0].character_id;
        let _ = ME;
        let (ship, _) = make(&mut reg, hull, me, STATION, crate::model::FLAG_HANGAR);
        reg.character_mut(me).expect("pilot").ship_id = ship;
        reg.character_mut(me).expect("pilot").ship_type_id = hull;
        (reg, ship)
    }

    /// A new singleton of `type_id` at an address; its id and row.
    pub(super) fn make(
        reg: &mut Registry,
        type_id: i64,
        owner: i64,
        location: i64,
        flag: i64,
    ) -> (i64, Item) {
        reg.apply_items(&[Edit::Create {
            type_id,
            quantity: 1,
            to: Slot::new(owner, location, flag),
        }])
        .expect("created");
        let it = reg
            .store()
            .items()
            .iter()
            .filter(|i| i.type_id == type_id && i.location_id == location && i.flag_id == flag)
            .max_by_key(|i| i.item_id)
            .expect("the new item")
            .clone();
        (it.item_id, it)
    }

    fn fit_plan(reg: &Registry, ship: i64, rig: i64, flag: i64) -> Result<Fit, RigRefusal> {
        let me = reg.config().characters[0].character_id;
        plan_fit(
            reg.content(),
            reg.store(),
            reg.character(me),
            ship,
            rig,
            flag,
        )
    }

    fn put(reg: &mut Registry, ship: i64, rig: i64, flag: i64) {
        let me = reg.config().characters[0].character_id;
        let (id, _) = make(reg, rig, me, ship, flag);
        let _ = id;
    }

    fn rig_in_hangar(reg: &mut Registry, rig: i64) -> i64 {
        let me = reg.config().characters[0].character_id;
        make(reg, rig, me, STATION, crate::model::FLAG_HANGAR).0
    }

    #[test]
    fn a_rig_is_a_module_with_the_slot_effect_and_a_hull_is_not() {
        let c = content();
        let a = rig_in(&c, RIG_A).expect("a rig");
        assert_eq!((a.cost, a.size), (COST_A, 1));
        assert!(rig_in(&c, GUN).is_none());
        assert!(rig_in(&c, HULL_SMALL).is_none());
        // The fixture's own rig has no cost or size: zeros.
        let f = rig_in(&c, 990_000_004).expect("the fixture rig");
        assert_eq!((f.cost, f.size), (0.0, 0));
        let h = hull_rigs_in(&c, HULL_SMALL).expect("a hull");
        assert_eq!((h.slots, h.calibration, h.size), (3, CALIBRATION, 1));
    }

    #[test]
    fn rig_flags_are_the_clients_three() {
        assert!(is_rig_flag(92) && is_rig_flag(94));
        assert!(!is_rig_flag(91) && !is_rig_flag(95) && !is_rig_flag(27));
    }

    #[test]
    fn a_rig_within_calibration_fits_the_lowest_free_slot() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let a = rig_in_hangar(&mut reg, RIG_A);
        let fit = fit_plan(&reg, ship, a, crate::fitting::FLAG_AUTOFIT).expect("fits");
        assert_eq!(fit.flag_id, FIRST_RIG_FLAG);
        reg.apply_items(&fit.plan).expect("applied");
        let b = rig_in_hangar(&mut reg, RIG_B);
        let fit = fit_plan(&reg, ship, b, crate::fitting::FLAG_AUTOFIT).expect("fits");
        assert_eq!(fit.flag_id, FIRST_RIG_FLAG + 1);
        // Exactly at capacity is within it.
        assert_eq!(COST_A + COST_B, CALIBRATION, "the invented numbers add up");
    }

    #[test]
    fn a_rig_over_calibration_is_refused_with_the_clients_key() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        put(&mut reg, ship, RIG_A, FIRST_RIG_FLAG);
        let big = rig_in_hangar(&mut reg, RIG_BIG);
        assert_eq!(
            fit_plan(&reg, ship, big, 93),
            Err(RigRefusal::Calibration { type_id: RIG_BIG })
        );
        let ServiceError::Raise(_) = RigRefusal::Calibration { type_id: RIG_BIG }.to_error() else {
            panic!("a raise");
        };
    }

    #[test]
    fn a_rig_of_the_wrong_size_is_refused_before_calibration() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let m = rig_in_hangar(&mut reg, RIG_MEDIUM);
        assert_eq!(
            fit_plan(&reg, ship, m, 92),
            Err(RigRefusal::WrongSize {
                type_id: RIG_MEDIUM,
                rig_size: 2,
                ship_rig_size: 1
            })
        );
        let (mut reg, ship) = docked_in(HULL_MEDIUM);
        let a = rig_in_hangar(&mut reg, RIG_A);
        assert!(matches!(
            fit_plan(&reg, ship, a, 92),
            Err(RigRefusal::WrongSize { .. })
        ));
    }

    #[test]
    fn slots_flags_and_places() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let b = rig_in_hangar(&mut reg, RIG_B);
        put(&mut reg, ship, RIG_B, 92);
        assert_eq!(
            fit_plan(&reg, ship, b, 92),
            Err(RigRefusal::Generic(Refusal::Occupied))
        );
        // A rack flag, or a flag past the client's three rig flags, is the
        // wrong kind of slot for a rig.
        for flag in [27, LAST_RIG_FLAG + 1] {
            assert!(matches!(
                fit_plan(&reg, ship, b, flag),
                Err(RigRefusal::Generic(Refusal::WrongKind { .. }))
            ));
        }
        // A hull without rig slots.
        let (mut reg, ship) = docked_in(HULL_NO_RIGS);
        let a = rig_in_hangar(&mut reg, RIG_A);
        assert_eq!(
            fit_plan(&reg, ship, a, 0),
            Err(RigRefusal::Generic(Refusal::NoSlot))
        );
        // Not the pilot's, or not in the hangar.
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let (other, _) = make(&mut reg, RIG_A, 1, STATION, crate::model::FLAG_HANGAR);
        assert_eq!(
            fit_plan(&reg, ship, other, 0),
            Err(RigRefusal::Generic(Refusal::NotAvailable))
        );
        // A fitted rig is not moved to another slot.
        put(&mut reg, ship, RIG_A, 92);
        let fitted = reg
            .store()
            .contents_of(ship)
            .find(|i| i.flag_id == 92)
            .map(|i| i.item_id)
            .expect("fitted");
        assert!(matches!(
            fit_plan(&reg, ship, fitted, 93),
            Err(RigRefusal::Generic(Refusal::WrongKind { .. }))
        ));
    }

    #[test]
    fn a_pod_and_a_pilot_in_space_are_refused() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let a = rig_in_hangar(&mut reg, RIG_A);
        let me = reg.config().characters[0].character_id;
        assert_eq!(
            plan_fit(reg.content(), reg.store(), None, ship, a, 0),
            Err(RigRefusal::Generic(Refusal::NoCharacter))
        );
        assert_eq!(
            fit_plan(&reg, ship + 1, a, 0),
            Err(RigRefusal::Generic(Refusal::NotActiveShip))
        );
        reg.character_mut(me).expect("pilot").station_id = None;
        assert_eq!(
            fit_plan(&reg, ship, a, 0),
            Err(RigRefusal::Generic(Refusal::NotDocked))
        );
    }

    #[test]
    fn unfitting_a_rig_destroys_it_and_a_strategic_cruiser_keeps_it() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        put(&mut reg, ship, RIG_A, 92);
        let id = reg
            .store()
            .contents_of(ship)
            .find(|i| i.flag_id == 92)
            .map(|i| i.item_id)
            .expect("fitted");
        let actor = reg.config().characters[0].character_id;
        assert_eq!(unfit(&mut reg, Some(actor), id), Some(Ok(())));
        assert!(reg.store().get(id).is_none(), "destroyed");

        let (mut reg, ship) = docked_in(HULL_MODULAR);
        put(&mut reg, ship, RIG_A, 92);
        let id = reg
            .store()
            .contents_of(ship)
            .find(|i| i.flag_id == 92)
            .map(|i| i.item_id)
            .expect("fitted");
        assert_eq!(unfit(&mut reg, Some(actor), id), Some(Ok(())));
        let kept = reg.store().get(id).expect("kept");
        assert_eq!(
            (kept.location_id, kept.flag_id),
            (STATION, crate::model::FLAG_HANGAR)
        );
    }

    #[test]
    fn only_a_fitted_rig_is_the_rigs_to_unfit() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let actor = reg.config().characters[0].character_id;
        let a = rig_in_hangar(&mut reg, RIG_A);
        assert!(unfit(&mut reg, Some(actor), a).is_none(), "in the hangar");
        assert!(unfit(&mut reg, Some(actor), ship).is_none(), "a hull");
        assert!(destroy_fitting(&mut reg, Some(actor), Some(a)).is_err());
        assert!(destroy_fitting(&mut reg, Some(actor), None).is_err());
    }

    #[test]
    fn calibration_used_counts_only_fitted_rigs() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let _held = rig_in_hangar(&mut reg, RIG_B);
        put(&mut reg, ship, RIG_A, 92);
        assert_eq!(calibration_used(reg.content(), reg.store(), ship), COST_A);
    }

    #[test]
    fn a_rigs_attributes_are_served_from_the_typedata() {
        let c = content();
        let attrs = attributes_in(&c, RIG_A).expect("a rig's");
        assert!(attrs.contains(&(ATTR_UPGRADE_COST, COST_A)));
        assert!(attributes_in(&c, HULL_SMALL).is_none());
    }

    // RIG-CAL: the fitting window reads the ship's attributes from godma.
    fn ship_attr(reg: &Registry, _ship: i64, id: i64) -> Option<f64> {
        let me = reg.config().characters[0].character_id;
        let c = reg.character(me).expect("pilot").clone();
        let PyValue::Instance { state, .. } = reg.capsule_dogma_row(&c) else {
            panic!("a KeyVal");
        };
        let PyValue::Dict(fields) = *state else {
            panic!("a dict state");
        };
        let attrs = fields.into_iter().find_map(|(v, k)| match k {
            PyValue::Bytes(b) if b == b"attributes" => Some(v),
            _ => None,
        });
        let Some(PyValue::Dict(e)) = attrs else {
            panic!("an attributes dict");
        };
        e.iter().find_map(|(v, k)| match (k, v) {
            (PyValue::Int(k), PyValue::Real(v)) if *k == id => Some(*v),
            _ => None,
        })
    }

    fn changes(reg: &mut Registry) -> Vec<(i64, f64)> {
        reg.take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| *n == "OnModuleAttributeChange")
            .map(|(_, a)| match (&a[2], &a[4]) {
                (PyValue::Int(id), PyValue::Real(v)) => (*id, *v),
                other => panic!("a change of an id and a real, not {other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_ships_loaded_attributes_carry_calibration_used_and_total() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        assert_eq!(ship_attr(&reg, ship, 1132), Some(CALIBRATION));
        assert_eq!(ship_attr(&reg, ship, 1152), Some(0.0));
        put(&mut reg, ship, RIG_A, 92);
        put(&mut reg, ship, RIG_B, 93);
        assert_eq!(ship_attr(&reg, ship, 1152), Some(COST_A + COST_B));
        assert_eq!(ship_attr(&reg, ship, 1154), Some(1.0));
    }

    /// The client's dogma drops a rig's calibration only when an
    /// `OnItemChange` takes the rig off the ship: a destroyed rig is told as
    /// moved to the junkyard, from the ship, at the rig flag.
    #[test]
    fn a_destroyed_rig_is_told_as_leaving_the_ship() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let me = reg.config().characters[0].character_id;
        put(&mut reg, ship, RIG_A, 92);
        let a = reg
            .store()
            .contents_of(ship)
            .find(|i| i.type_id == RIG_A)
            .expect("fitted")
            .item_id;
        let _ = reg.take_pending();
        destroy_fitting(&mut reg, Some(me), Some(a)).expect("destroyed");
        let sent: Vec<_> = reg
            .take_pending()
            .into_iter()
            .map(|(_, p)| p.to_push())
            .filter(|(n, _)| *n == "OnItemChange")
            .collect();
        assert_eq!(sent.len(), 1, "one item change: {sent:?}");
        let PyValue::Dict(change) = &sent[0].1[1] else {
            panic!("the change is a dict: {:?}", sent[0].1);
        };
        // A marshal dict holds (value, key).
        assert!(change.contains(&(
            PyValue::Int(ship),
            PyValue::Int(crate::item::IX_LOCATION_ID)
        )));
        assert!(change.contains(&(PyValue::Int(92), PyValue::Int(crate::item::IX_FLAG))));
        assert!(reg.store().get(a).is_none());
    }

    #[test]
    fn fitting_and_unfitting_rigs_push_the_used_calibration() {
        let (mut reg, ship) = docked_in(HULL_SMALL);
        let me = reg.config().characters[0].character_id;
        let _ = reg.take_pending();
        crate::fitstate::around(&mut reg, Some(me), |reg| put(reg, ship, RIG_A, 92));
        assert!(changes(&mut reg).contains(&(1152, COST_A)));
        crate::fitstate::around(&mut reg, Some(me), |reg| put(reg, ship, RIG_B, 93));
        assert!(changes(&mut reg).contains(&(1152, COST_A + COST_B)));
        let a = reg
            .store()
            .contents_of(ship)
            .find(|i| i.type_id == RIG_A)
            .expect("fitted")
            .item_id;
        crate::fitstate::around(&mut reg, Some(me), |reg| {
            destroy_fitting(reg, Some(me), Some(a)).expect("destroyed")
        });
        assert!(changes(&mut reg).contains(&(1152, COST_B)));
        // Nothing moved, nothing said.
        crate::fitstate::around(&mut reg, Some(me), |_| ());
        assert!(changes(&mut reg).is_empty());
    }
}
