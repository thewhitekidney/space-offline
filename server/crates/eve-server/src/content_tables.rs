//! The registered content tables (`docs/content/plan.md` §1.5).
//!
//! W10.5 seam S7(b). **Coordinator-only**, like `service.rs`'s `DOMAINS`: a
//! data module exports its `TableSpec`, and the coordinator adds the one
//! line here at landing. Empty until W11.

use crate::content::TableSpec;

/// Every table [`crate::content::Content`] builds, from a bundle or from
/// fixtures.
pub const TABLES: &[TableSpec] = &[
    crate::celestial::UNIVERSE,
    crate::celestial::CENTERS_TABLE,
    crate::combat::WRECK_TABLE,
    crate::docking::LOCATORS,
    crate::dbuff_data::TABLE,
    crate::dungeon_data::TABLE,
    crate::econ_exchange::TABLE,
    crate::econ_prices::TABLE,
    crate::epic_arc::TABLE,
    crate::facwar::WARZONES,
    crate::fighters_data::TABLE,
    crate::hacking_loot::TABLE,
    crate::industry::TABLE,
    crate::loyalty::TABLE,
    crate::mission_pool::TABLE,
    crate::mission_rewards::TABLE,
    crate::mtu::TABLE,
    crate::nes::TABLE,
    crate::skins::TABLE,
    crate::npc_data::TABLE,
    crate::planet::STATICS,
    crate::population::AUTHORED,
    crate::reprocess::TABLE,
    crate::salvage::TABLE,
    crate::skill::TABLE,
    crate::skill_trading::TABLE,
    crate::start::TABLE,
    crate::starter_ship::TABLE,
    crate::stations::TABLE,
    crate::system_effect::TABLE,
    crate::typedata::TABLE,
    crate::wormhole::TABLE,
    crate::wormhole_sites::TABLE,
];
