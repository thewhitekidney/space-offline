//! **`eve-mod-econ`: the owner's custom economy**, as one module over the
//! core (`docs/lanes/pr-seam-1.md`, issue #29). Everything here is a choice
//! of the owner's; what CCP's server does stays in `eve-server`.
//!
//! - [`drops`]: PLEX that an elite NPC leaves in its wreck. A kill-hook row.
//! - [`injectors`]: a skill injector that an elite or heavy NPC leaves in its
//!   wreck, ledgered in the core's `sptrade`. A kill-hook row.
//! - [`filaments`]: a Needlejack filament that an elite or heavy NPC leaves in
//!   its wreck, the only supply of them. A kill-hook row.
//! - [`boosters`]: a drug-booster blueprint copy that an elite or heavy NPC
//!   leaves in its wreck, the only supply of them besides the NPC-sold
//!   formulas. A kill-hook row.
//! - [`geode`]: a rare crate a mining cut may leave in the hold, and what is
//!   in it when it is opened at a station (`crateService`). A mining-hook
//!   row and a crate row.
//! - [`motherlode`]: motherlodes (a rich belt for 45 minutes, announced), belt
//!   ambushes with a prize wreck, and rich sites. A mining-hook row, a stage
//!   and a kill-hook row.
//! - [`opening`]: the PLEX seed, one finite ledgered stack on sale, once. A
//!   log-in row.
//! - [`tunables`]: the numbers, all **[A]**.
//!
//! Nothing here replaces a core service or changes a wire shape. It adds
//! two hook rows and one delta kind ([`KIND`]); the core gave it three small
//! pieces to do so, listed in the lane report (`Registry::seed_ask`,
//! `nes_vault::ledger_in`, the module version in the world format).
//!
//! **Nothing from nowhere.** Every PLEX unit this module makes is added to
//! the `nes` ledger under a name (`seed`, `drop:<source>`) in the same step
//! that creates the item, and the day's minted total is kept so the budget
//! bites.
//!
//! The module is installed by `eve-offline-server`
//! (`cli::run(&[&MODULE])`); the core `eve-server` binary has no modules.

pub mod boosters;
pub mod drops;
pub mod filaments;
pub mod geode;
pub mod injectors;
pub mod motherlode;
pub mod opening;
pub mod tunables;

use eve_server::module::{
    CrateRow, KillRow, MinedRow, Module, Placed, SessionRow, SettledRow, Stage,
};

/// The module's delta kind: `seed` (the opening stack) and `day` (the
/// budget's counter). Owned by this module.
pub const KIND: &str = "econmod";

/// The module's own world-format number (`Module::version`): bump it when the
/// rows of [`KIND`] change shape.
pub const VERSION: u32 = 1;

/// The module.
pub const MODULE: Module = Module {
    name: "econ",
    version: VERSION,
    kinds: &[(KIND, "econ")],
    kill_npc: &[
        KillRow {
            name: "econ_plex_drop",
            run: drops::on_npc_kill,
        },
        KillRow {
            name: "econ_injector_drop",
            run: injectors::on_npc_kill,
        },
        KillRow {
            name: "econ_filament_drop",
            run: filaments::on_npc_kill,
        },
        KillRow {
            name: "econ_booster_bpc_drop",
            run: boosters::on_npc_kill,
        },
        KillRow {
            name: "econ_ambush_prize",
            run: motherlode::on_npc_kill,
        },
    ],
    tables: &[geode::TABLE],
    crates: &[CrateRow {
        name: "econ_geode",
        is_crate: geode::is_crate,
        open: geode::open,
    }],
    // The motherlode row first: it notes where the miners are, and pays the
    // zone's ore; the geode row then reads the same zone.
    mined: &[
        MinedRow {
            name: "econ_motherlode_yield",
            run: motherlode::on_mined,
        },
        MinedRow {
            name: "econ_geode_drop",
            run: geode::on_mined,
        },
    ],
    // The same four tables for a kill an offline settlement books (no wreck:
    // the killer's hangar is paid).
    settled: &[
        SettledRow {
            name: "econ_plex_drop",
            run: drops::on_settled,
        },
        SettledRow {
            name: "econ_injector_drop",
            run: injectors::on_settled,
        },
        SettledRow {
            name: "econ_filament_drop",
            run: filaments::on_settled,
        },
        SettledRow {
            name: "econ_booster_bpc_drop",
            run: boosters::on_settled,
        },
    ],
    slots: &[motherlode::SLOT],
    stages: &[Placed {
        stage: Stage {
            name: "econ_motherlode",
            run: motherlode::tick,
            every: 1,
            phase: 0,
        },
        after: "asteroid",
    }],
    commands: &[
        eve_server::module::Command {
            name: "geode",
            usage: "/geode give [tier]",
            run: geode::slash,
        },
        eve_server::module::Command {
            name: "motherlode",
            usage: "/motherlode [begin|end|status]",
            run: motherlode::slash,
        },
    ],
    on_login: &[SessionRow {
        name: "econ_plex_seed",
        run: opening::on_login,
    }],
    ..Module::NONE
};
