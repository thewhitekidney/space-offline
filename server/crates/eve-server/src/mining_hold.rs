//! **The mining hold** (GAPS-B): a ship whose type carries
//! `generalMiningHoldCapacity` has a second hold for what it mines, and
//! every mining cut fills it before the cargo hold.
//!
//! Lane note `docs/lanes/gaps-b.md`. Read from the client:
//!
//! - **The flag** is `flagGeneralMiningHold` = 134 [V]
//!   (`inventorycommon/const.py`), and its capacity attribute is
//!   `attributeGeneralMiningHoldCapacity` (1556) [V]
//!   (`inventoryFlagsCommon.inventoryFlagData[flagGeneralMiningHold]`). A
//!   Venture's is 5,000 m3 [V-data].
//! - **What it takes** is `inventoryFlagData`'s row for the flag, read by
//!   `ShouldAllowAdd` [V]: category `categoryAsteroid` (25) unless the group
//!   is blocked (`groupScalableDecorativeAsteroid` 4094,
//!   `groupTerranArtifacts` 519, `groupNonInteractableAsteroid` 4714), or a
//!   group it allows outright (`groupHarvestableCloud` 711,
//!   `groupCompressedGas` 4168, `groupPrismaticite` 4915). So ore, ice,
//!   moon ore and gas, and nothing else ([`accepts`]).
//! - **The window**: `invCtrl.ShipGeneralMiningHold` is a `BaseShipBay`
//!   with `locationFlag = flagGeneralMiningHold` [V]
//!   (`invControllers.py`); the hold window opens for a ship whose dogma
//!   has `generalMiningHoldCapacity` (`DOGMA_SM_HOLD_MAP`, `const.py`) [V].
//!   Its items are `invCacheContainer.List(flag=134)` on the ship's
//!   inventory [V] (`BaseInvContainer.GetItems`), which the server answers
//!   as for any flag; its capacity is the active ship's
//!   `godmaItem.GetCapacity(flag)`, computed in the client (`invCache.py`
//!   `GetCapacity`) [V], so no server call is involved.
//!
//! **Where a cut goes** ([`room_for`], `crate::mining::cut`): into the
//! mining hold as many units as it has room for when it takes the ore,
//! then the rest into the cargo hold **[A]** (EVE as players know it: an
//! ore hold that fills spills into cargo). Falsified by a client or a TQ
//! capture where a full ore hold stops the laser with room left in cargo.
//! **[A]** the capacity is the type's own attribute: no skill or rig bonus
//! (a Venture's Mining Frigate bonus is not modelled).

use crate::content::Content;
use crate::service::Registry;

/// `flagGeneralMiningHold` **[V]** (`inventorycommon/const.py`).
pub const FLAG_GENERAL_MINING_HOLD: i64 = 134;
/// `attributeGeneralMiningHoldCapacity` **[V]** (`dogma/const.py`,
/// `inventoryFlagData`).
pub const ATTR_GENERAL_MINING_HOLD_CAPACITY: i64 = 1556;
/// `categoryAsteroid` **[V]**: the category the hold allows.
pub const CATEGORY_ASTEROID: i64 = 25;
/// The groups the hold allows outside the category **[V]**:
/// `groupHarvestableCloud`, `groupCompressedGas`, `groupPrismaticite`.
pub const ALLOW_GROUPS: [i64; 3] = [711, 4168, 4915];
/// The asteroid-category groups the hold refuses **[V]**:
/// `groupScalableDecorativeAsteroid`, `groupTerranArtifacts`,
/// `groupNonInteractableAsteroid`.
pub const BLOCK_GROUPS: [i64; 3] = [4094, 519, 4714];

/// **Whether the mining hold takes a type**: `ShouldAllowAdd` for
/// [`FLAG_GENERAL_MINING_HOLD`] returning no error [V]. Its rule for this
/// row: an allowed category passes unless the group is blocked; any other
/// category passes only by an allowed group.
pub fn accepts(category_id: i64, group_id: i64) -> bool {
    if category_id == CATEGORY_ASTEROID {
        !BLOCK_GROUPS.contains(&group_id)
    } else {
        ALLOW_GROUPS.contains(&group_id)
    }
}

/// The mining hold's capacity of a ship type, m3: its
/// [`ATTR_GENERAL_MINING_HOLD_CAPACITY`], 0 for a hull without one.
pub fn capacity(content: &Content, ship_type_id: i64) -> f64 {
    crate::typedata::attribute_in(content, ship_type_id, ATTR_GENERAL_MINING_HOLD_CAPACITY)
        .filter(|c| c.is_finite() && *c > 0.0)
        .unwrap_or(0.0)
}

/// The free m3 in `ship_id`'s mining hold (a `ship_type_id`).
pub fn free(w: &Registry, ship_id: i64, ship_type_id: i64) -> f64 {
    (capacity(w.content(), ship_type_id)
        - w.store()
            .used_volume_in(w.content(), ship_id, FLAG_GENERAL_MINING_HOLD))
    .max(0.0)
}

/// Where a cut of one type can go, in whole units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// Units the mining hold takes (0 without one, or when it refuses the
    /// type).
    pub hold: i64,
    /// Units the cargo hold takes.
    pub cargo: i64,
}

impl Room {
    pub fn total(self) -> i64 {
        self.hold.saturating_add(self.cargo)
    }

    /// `units` split: the mining hold first, the rest to cargo.
    pub fn split(self, units: i64) -> (i64, i64) {
        let hold = units.min(self.hold).max(0);
        (hold, (units - hold).min(self.cargo).max(0))
    }
}

/// **The room for a mined type** in `ship_id` (a `ship_type_id`): the
/// mining hold's whole units of `ore` when it [`accepts`] it, and the cargo
/// hold's (`crate::mining::cargo_free`).
pub fn room_for(
    w: &Registry,
    ship_id: i64,
    ship_type_id: i64,
    ore: &crate::item::ItemType,
) -> Room {
    let hold = if accepts(ore.category_id, ore.group_id) {
        crate::mining::units_in(free(w, ship_id, ship_type_id), ore.volume)
    } else {
        0
    };
    Room {
        hold,
        cargo: crate::mining::units_in(
            crate::mining::cargo_free(w, ship_id, ship_type_id),
            ore.volume,
        ),
    }
}

/// The free m3 a mined type can go into: the mining hold's when it takes
/// it, plus the cargo hold's (the refusal's `available`).
pub fn free_for(w: &Registry, ship_id: i64, ship_type_id: i64, ore: &crate::item::ItemType) -> f64 {
    let hold = if accepts(ore.category_id, ore.group_id) {
        free(w, ship_id, ship_type_id)
    } else {
        0.0
    };
    hold + crate::mining::cargo_free(w, ship_id, ship_type_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ShouldAllowAdd`'s row for the flag, case by case (invented group
    /// ids where the client names none).
    #[test]
    fn the_hold_takes_asteroids_and_the_allowed_groups_only() {
        assert!(accepts(CATEGORY_ASTEROID, 99_001), "any asteroid group");
        for g in BLOCK_GROUPS {
            assert!(!accepts(CATEGORY_ASTEROID, g), "{g} is blocked");
        }
        for g in ALLOW_GROUPS {
            assert!(accepts(2, g) && accepts(4, g), "{g} is allowed anywhere");
        }
        assert!(!accepts(4, 99_002), "a mineral is not");
        assert!(!accepts(7, 99_003), "a module is not");
    }

    #[test]
    fn a_cut_fills_the_hold_first_then_cargo() {
        let room = Room { hold: 3, cargo: 5 };
        assert_eq!(room.total(), 8);
        assert_eq!(room.split(2), (2, 0));
        assert_eq!(room.split(6), (3, 3));
        assert_eq!(room.split(20), (3, 5));
        assert_eq!(Room { hold: 0, cargo: 4 }.split(3), (0, 3));
    }
}
