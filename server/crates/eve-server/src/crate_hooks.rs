//! **Crate hooks** (lane l6a-geodes, issue #32): which item types are crates
//! and what is inside one when it opens. `crateService` (`service/crate_svc.rs`)
//! is CCP's mechanics and lives in the core: where a crate may be opened, the
//! one crate it consumes, the answer the client unpacks. What a crate *holds*
//! is the owner's design, so the core has no rows and a module adds them
//! (`eve-mod-econ`'s geodes).

use crate::service::Registry;

/// One thing a crate yielded, as the client's reveal lists it
/// (`LootItemFromType(itemID, typeID, quantity, ...)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loot {
    /// The item now in the pilot's hangar, or `0` for a prize that is not an
    /// item (PLEX paid straight into the vault): the client's claim of it is
    /// then a no-op.
    pub item_id: i64,
    pub type_id: i64,
    pub quantity: i64,
}

/// One crate being opened.
#[derive(Debug, Clone, Copy)]
pub struct OpenEvent {
    pub character: i64,
    pub crate_type: i64,
    /// The stack the crate comes from. It is already one short when the row
    /// runs.
    pub crate_item: i64,
    /// The station the pilot is docked in: the prizes go to its hangar.
    pub station: i64,
    /// Which crate of this request this is (0 for a single open). A row mixes
    /// it into its draw, so a stack opened at once does not open alike.
    pub nth: u64,
    /// The instant of the open (filetime), for a daily budget.
    pub when: i64,
}

/// A module's rule for one family of crates.
#[derive(Clone, Copy)]
pub struct Row {
    pub name: &'static str,
    /// Whether `type_id` is a crate of this row.
    pub is_crate: fn(&Registry, i64) -> bool,
    /// Make the prizes: create the items in the pilot's hangar (or pay the
    /// vault), ledger every unit in the same step, and say what came out.
    pub open: fn(&mut Registry, &OpenEvent) -> Vec<Loot>,
}

/// The core's rows: none.
pub const CRATES: &[Row] = &[];
