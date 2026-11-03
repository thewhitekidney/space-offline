//! **The farmer's settlement** (R-ECON §1.5 / R-SOCIETY: "PI extraction").
//! The tending half of the PI bot's cycle (`eve-bot/src/pi.rs::run`), away
//! from the keyboard:
//!
//! 1. **The colonies run by themselves.** The simulation is lazy and exact
//!    (`planet::Colonies::advance`): extractors cut, factories cycle, goods
//!    pile up in the command centre while the pilot is away, at the game
//!    clock's pace. Nothing is minted here: it is the colony's own ledger.
//! 2. **Restart** every extraction unit whose program ran out
//!    (`Registry::settle_colonies`: the same program again, through the real
//!    `INSTALLPROGRAM` command and its quote). An extractor idle between its
//!    expiry and this settlement lost that gap: an agent restarts late, as a
//!    pilot who is away does.
//! 3. **Launch** the command centre's commodities to the hangar (its launch
//!    cooldown permitting).
//! 4. **Sell** the planetary commodities into the bids at the station at the
//!    floor or better, and list what is left as asks ([`shop::list_where`]),
//!    as the live farmer's `sale_plan` does.
//!
//! **Not settled**: founding a colony, upgrading a command centre, new routes
//! and schematics (the pilot's), the customs office import and export tax
//! path (the launch here is the hangar launch the bot uses).

use super::{Agent, shop};
use crate::damage::BlueTime;
use crate::item::Item;
use crate::service::Registry;

/// `categoryPlanetaryCommodities` **[V]**.
const CATEGORY_PLANETARY_COMMODITIES: i64 = 43;

/// Settle one farmer.
pub(super) fn settle(
    reg: &mut Registry,
    character: i64,
    a: &Agent,
    _now: BlueTime,
    notes: &mut Vec<String>,
) {
    let tend = reg.settle_colonies(character, crate::pyutil::filetime_now());
    if tend.colonies == 0 {
        notes.push("no colonies to tend".to_owned());
    } else {
        notes.push(format!(
            "{} colonies, {} programs restarted, {} commodity stacks launched",
            tend.colonies,
            tend.restarted,
            tend.launched.len()
        ));
    }
    let planetary = |i: &Item| i.category_id == CATEGORY_PLANETARY_COMMODITIES;
    shop::sell_where(reg, character, a.station, planetary, notes);
    shop::list_where(reg, character, a.station, planetary, |_| 0.0, notes);
}
