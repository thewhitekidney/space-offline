//! **Mining hooks** (lane l6a-geodes, issue #32): what a finished mining cut
//! sets in motion beyond the ore itself. `mining::cut` builds one
//! [`MinedEvent`] per cut that took at least a unit (laser, harvester and
//! drone alike) and runs every row a module added. The core has no rows:
//! what a cut may also yield (a geode, in `eve-mod-econ`) is a choice of the
//! owner's, not CCP's.
//!
//! A row runs after the ore is in the hold and the rock is smaller, on the
//! clock (`mining::on_clock`), so the pushes of an item it creates reach the
//! miner like the ore's own. A row must not touch the rock.

use crate::service::Registry;

/// One cut that took ore.
#[derive(Debug, Clone, Copy)]
pub struct MinedEvent {
    pub miner: i64,
    pub ship_id: i64,
    pub ship_type_id: i64,
    pub rock_id: i64,
    pub rock_type: i64,
    pub system: i64,
    /// Units that left the rock into the hold.
    pub normal: i64,
    /// Units the cut added from nothing (a crit); the cycle was a crit when
    /// this is above 0.
    pub crit: i64,
    /// The cut's own instant (blue time).
    pub at: i64,
    /// The module or drone that cut.
    pub source_id: i64,
}

/// What a row does with the world, for one cut.
pub type Hook = fn(&mut Registry, &MinedEvent);

/// One row, named for diagnostics and the uniqueness check.
#[derive(Clone, Copy)]
pub struct Row {
    pub name: &'static str,
    pub run: Hook,
}

/// The core's rows: none.
pub const MINED: &[Row] = &[];
