//! The eve-offline server: accept a real EVE client, answer its handshake, and
//! record everything.
//!
//! ## What this crate can and cannot do, stated plainly
//!
//! **It can** run the complete GPS low-level version check and authentication
//! handshake — the ten payloads H1..H10 of `docs/reference/login-path.md` §4. Every one
//! of them is a plain marshal value: a tuple, a dict, an int, a real, a string.
//! Not one is a Python class instance. So the handshake needs nothing from the
//! unresolved object-wrapper type codes, and it is implemented here in full.
//!
//! **It cannot** yet answer what comes after, but no longer for the reason it
//! once could not. The object wrapper is recovered (`docs/reference/object-wrapper.md`):
//! a machoNet packet is an old-style instance under tag `0x17`, `eve-marshal`
//! reads and writes all three wrapper codes, and
//! `eve-proto::MachoPacket::to_payload` emits a complete packet. What is
//! missing now is the **service layer** — something to answer a `CallReq`
//! with — not the ability to form a reply.
//!
//! So after authentication this server still listens and logs. It decodes each
//! payload and reports any type code it cannot read, which is now a narrower
//! and more interesting question than it was: the wrapper codes are handled,
//! so what turns up will be `0x2A` (DBRow), `0x2B` (SubStream), `0x21`
//! (pickle) or something we have not predicted. Whichever it is, the first
//! client connection names it.
//!
//! **One handshake step is unverified.** H8 carries a `signedFunc`, a CPython
//! 2.7 marshalled code object the client `eval`s. We emit one (`pycode`) whose
//! encoding is confirmed by parsing it back with our own Python 2.7 parser, but
//! nothing here can confirm that the real `marshal.loads` accepts it, nor that
//! `Crypto.Verify` under Placebo is the identity we infer. Expect H8 to be the
//! first thing to break.
//!
//! ## No dependencies, and why that is not stubbornness
//!
//! `std::net` and threads, nothing else. The workspace has an empty
//! `deps.allow` and `tools/check_deps.py` enforces it. For one to ten players a
//! thread per connection is not a compromise; an async runtime would add a
//! large dependency tree to a project whose whole premise is that we ship only
//! our own code.

#![forbid(unsafe_code)]

pub mod abyss;
pub mod account_book;
pub mod accounts;
pub mod air_npe;
pub mod alliance;
pub mod asset_safety;
pub mod asteroid;
pub mod authored;
pub(crate) mod belts;
pub mod booster_sources;
pub mod boosters;
pub mod bot_names;
pub mod brain;
pub mod capture;
pub mod celestial;
pub mod cli;

/// Read a celestial extract, with an error a player can act on.
///
/// Thin, and in the library rather than the binary so that the message is the
/// same wherever it is loaded from.
pub fn load_celestials(path: &std::path::Path) -> Result<celestial::Catalogue, String> {
    let c = celestial::Catalogue::load(path)
        .map_err(|e| format!("--celestials {}: {e}", path.display()))?;
    if c.is_empty() {
        return Err(format!(
            "--celestials {}: no rows. Regenerate it with \
             tools/celestials.py",
            path.display()
        ));
    }
    Ok(c)
}
pub mod ancient_rate;
pub mod bulletins;
pub mod char_settings;
pub mod chat;
pub mod chat_dir;
pub(crate) mod clones;
pub mod collision;
pub mod combat;
pub mod concord;
pub(crate) mod conn;
pub mod contact_labels;
pub(crate) mod contacts;
pub mod content;
pub(crate) mod content_tables;
// #57: cargo containers, their passwords and a courier package's seal.
pub mod achievements;
pub mod anchor;
pub mod bubble;
pub mod containers;
pub(crate) mod contracts;
pub mod corp_medals;
pub mod corp_rec;
pub mod corp_roles;
pub mod corp_shares;
pub mod corp_titles;
pub mod corp_votes;
pub mod corp_wallet;
pub mod crimewatch;
pub mod cyno;
pub mod daily_goals;
pub mod damage;
pub mod dbuff;
pub mod dbuff_data;
pub(crate) mod delta;
pub(crate) mod delta_kinds;
pub mod destiny;
pub mod dispatch;
pub mod dock_policy;
pub mod docking;
pub mod dogma;
pub mod drone_skills;
pub mod drones;
pub(crate) mod dungeon;
pub mod dungeon_data;
pub(crate) mod dungeon_tracking;
pub mod dynamic_item;
pub mod econ_agents;
pub(crate) mod econ_dbs;
pub mod econ_exchange;
pub(crate) mod econ_ledger;
pub mod econ_prices;
pub(crate) mod econ_report;
pub(crate) mod epic_arc;
pub(crate) mod ess;
pub mod facwar;
pub mod fighters;
pub mod fighters_data;
pub mod fitstate;
pub mod fitting;
pub mod fitting_store;
pub(crate) mod fleet;
pub(crate) mod fleet_broadcast;
pub mod fleet_finder;
pub(crate) mod fleet_warp;
pub mod free_sp;
pub mod fw_capture;
pub mod fw_lp;
// #8 (FW-7): battlefield scoreboards.
pub mod fw_scoreboard;
pub mod fw_stats;
pub mod gateway;
pub mod gateway_auth;
pub mod gateway_ext;
pub mod gateway_out;
pub mod gateway_plans;
pub mod gm_tools;
pub mod mtu;
pub mod smartbomb;
pub mod superweapon;
pub mod world_events;
// Lane clock: `--time-scale N`, the slow game timers N times faster.
pub mod bot_control;
// Lane admin-web: `--admin ADDR`, the owner's read-only web page and API.
pub mod admin_actions;
pub mod admin_web;
pub mod bot_words;
pub mod cloak;
pub mod clock_probe;
pub mod econ_scale;
pub mod game_clock;
pub mod invuln;
pub mod safe_logoff;
pub mod sched_stat;
pub mod self_destruct;
pub mod wing_words;
// The closed `InitClientGlobalConfig` route, kept as its own documented dead
// end (the module docs); only its tests build it.
#[cfg(test)]
pub(crate) mod global_config;
// The source scan that keeps `Character::solar_system_id` reads honest.
pub mod crate_hooks;
pub mod filament_drops;
pub mod grid;
pub mod group_contacts;
pub mod hacking;
pub mod hacking_loot;
pub mod handshake;
pub mod hypernet;
pub mod implants;
pub mod industry;
pub mod item;
pub mod item_names;
// CQ-2: the FW Conquest item traders and the hook that hears their trades.
pub mod item_trader;
pub mod item_trader_hooks;
pub mod jettison;
pub mod jump;
pub mod jump_bridge;
pub mod jump_fatigue;
pub(crate) mod kill_hooks;
#[cfg(test)]
mod location_lint;
pub mod mining_hooks;
pub mod settle_hooks;
// SOC-C6: kill reports, the tally and the `killmail` delta kind.
pub(crate) mod killmail;
// GAP-CALENDAR: events, invites and RSVPs, the `calendar` delta kind.
pub mod calendar;
// GAP-KILLMARKS: kill marks on a ship, the `killmark` delta kind.
pub mod killmarks;
// MAP-PERSONAL: visited systems, the `visit` delta kind.
pub(crate) mod ledger;
pub(crate) mod live_status;
pub(crate) mod liveness;
pub mod visits;
// MAP-STATS: the hourly kill/jump counter behind map.GetHistory.
pub mod logline;
pub mod logoff_spot;
pub mod loyalty;
pub mod lp_transfer;
pub(crate) mod map_stats;
// X-TRADE: the market's fee, tax and limit formulas, shared with the bots.
pub mod ewar;
pub mod market_fees;
pub mod mining;
pub mod mining_hold;
pub mod mining_ledger;
pub mod missile;
pub(crate) mod missile_skills;
pub mod mission;
pub(crate) mod mission_hooks;
pub mod mission_pool;
pub(crate) mod mission_rewards;
pub mod mjd;
pub mod model;
pub mod module;
pub mod module_fuel;
pub mod module_range;
pub mod modules;
pub mod moon;
pub mod moon_beacon;
pub mod moon_belt;
pub mod moon_extract;
pub mod moon_scan;
pub(crate) mod movement_guard;
pub mod mutator_data;
pub mod nes;
pub mod nes_vault;
pub mod node_graph;
pub mod notes;
pub mod notify;
pub mod owner_groups;
pub mod par;
pub mod rules;
#[cfg(test)]
pub(crate) mod test_clock;
// NOTIF-1: the notification store and the `notify` API.
pub mod bounty_pool;
pub mod combat_site;
pub mod escalation;
pub mod notifications;
pub mod npc;
pub mod npc_data;
pub(crate) mod npc_ewar;
pub mod npc_gen;
pub(crate) mod npc_missile;
pub mod npc_threat;
pub mod orbit;
pub mod overheat;
pub mod planet;
pub mod pochven;
pub mod population;
pub mod proving;
pub(crate) mod push;
pub mod pycode;
pub mod pyutil;
pub mod reaction_market;
pub mod reactions;
pub mod remote_rep;
pub mod repair;
pub mod reprocess;
pub mod rigs;
pub mod roles;
pub(crate) mod rollover;
pub(crate) mod rollover_steps;
pub mod salvage;
pub(crate) mod scan;
pub(crate) mod scan_sources;
pub mod sec_status;
pub mod security_office;
pub mod seed;
pub mod sentry;
pub mod server;
pub mod service;
pub mod session;
pub(crate) mod session_hooks;
pub(crate) mod session_table;
pub mod share_links;
pub mod ship_clone;
pub mod ship_config;
pub mod ship_scan;
pub mod shutdown;
pub mod signal_tracker;
pub(crate) mod site;
pub(crate) mod site_spawn;
pub mod skill;
pub(crate) mod skill_queue;
pub mod skill_trading;
pub(crate) mod skillgate;
pub mod skins;
// EM-3 (#207): ship emblem licences and logos on hulls.
pub mod emblem;
pub(crate) mod slash;
pub(crate) mod slim_extra;
pub mod trade;
// SOC-D1: the sov ledger (`sov`, `sovprime`, `sovact`), its rows and pushes.
pub mod sov;
pub mod sov_hub_fuel;
pub mod sov_index;
pub mod sov_jammer;
pub mod sov_resources;
pub mod sov_upgrade;
pub mod sov_workforce;
// SOC-D2: the Sovereignty Hub in space -- launch, ball, GM capture, stand-in fuel.
pub mod command_nodes;
pub mod corp_office;
pub(crate) mod corp_standings;
pub mod entosis;
pub mod jump_drive;
pub mod jump_portal;
pub mod mail;
pub mod merc_den;
pub mod random_jump;
pub mod skyhook;
pub mod skyhook_theft;
pub mod sov_campaign;
pub mod sov_gw_stub;
pub mod sov_hub;
pub mod sov_proto;
// U1-UPWELL: the Upwell structure model (states, timers, upkeep, deployment check).
pub mod structure;
pub mod structure_bays;
pub mod structure_combat;
pub mod structure_dock;
// #42 part b (STR-FUEL): the hourly fuel burn, the online cost, the fuel expiry.
pub mod structure_fuel;
pub mod structure_notify;
pub mod structure_profile;
// #41 UPWELL-FIT: service modules at a structure's controls.
pub mod structure_industry; // #57
pub mod structure_services;
pub mod structure_space;
// PROG-MINE: in-space compression.
pub mod bot_panel;
pub mod compression;
pub mod dev_mode;
pub mod pilotless;
pub mod sov_effect;
pub mod space_component;
pub mod space_state;
pub mod stance;
pub(crate) mod standings;
pub(crate) mod start;
pub mod starter_ship;
pub(crate) mod stations;
pub(crate) mod store;
pub(crate) mod storyline;
pub mod subsystems;
pub mod system_effect;
pub(crate) mod tackle;
pub(crate) mod tales;
pub(crate) mod targeting;
#[cfg(test)]
pub(crate) mod testkit;
pub(crate) mod tick;
pub(crate) mod tick_budget;
pub mod typedata;
// C8: player wars -- the registry and its clock, the deals between sides,
// the notices, and the report's numbers. `pub` so crimewatch, CONCORD and
// the bots call `war::at_war` and `war::legal`.
pub mod war;
pub mod war_deal;
pub mod war_notice;
pub mod war_stats;
pub mod warp_in;
pub(crate) mod warp_subject;
pub mod weapon_bank;
pub mod wire;
pub(crate) mod world_ext;
pub mod wormhole;
// W17 CP54b: wormhole jumps, mass and decay.
pub(crate) mod wormhole_jump;
pub mod wormhole_sites;

/// The hull-to-wreck table, so a socket test can give an invented hull a
/// wreck (CW-1's theft test).
pub use combat::Wrecks;
pub use conn::{ConnEnd, PREAUTH_MAX_FRAME};
pub use dispatch::{CaptureOnly, Dispatch, GpsSession, Reply};
pub use handshake::{Handshake, HandshakeError, Phase};
pub use model::{Character, SandboxConfig};
pub use server::{Config, Mode, Server};
pub use service::{Registry, Request, ServiceError};
pub use session::Session;
pub use store::{FileStore, Store};
pub use wire::{crypto_hash, dumps, loads};
