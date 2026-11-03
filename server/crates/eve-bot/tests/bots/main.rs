//! Every bot-over-a-socket test, as **one** test binary.
//!
//! `cargo test` runs test binaries one after another and parallelises only
//! inside a binary (`docs/review/V4-testing.md` S1). These tests are mostly
//! waits on game clocks -- mining cycles, the jump delay, warps -- and so
//! CPU-idle: six binaries in series cost the sum of their waits, one binary
//! costs about the longest of them. Each module boots its own in-process
//! server through [`common::serve`], so no two tests share a world and the
//! order they run in does not matter.
//!
//! A new bot test file is `tests/bots/<name>.rs` plus a `mod` line here.
//! The same limitation holds for all of them: both ends marshal with
//! `eve-marshal`, so this proves behaviour, not bytes.

mod common;

mod alliance;
mod arena_fight;
mod board_eject;
mod bot_churn;
mod bot_corp;
mod bot_ctl;
mod capital;
mod co_found;
mod concord;
mod contracts;
mod corp_buy;
mod corp_ceo;
mod corp_office;
mod corp_stock;
mod corp_wallet;
mod crimewatch;
mod dbuff;
mod dev_mode;
mod doctrine_assign;
mod doctrine_sense;
mod drift;
mod drones;
mod engine_fix;
mod entosis;
mod escort;
mod ewar;
mod fe_build;
mod fe_demand;
mod fe_trade;
mod fit_doctrine;
mod fleet;
mod fwlp;
mod gb_fix;
mod group_contacts;
mod hacker;
mod handshake;
mod hangar;
mod hostile;
mod hunt;
mod ice_tier;
mod industry;
mod insure;
mod invention;
mod jump;
mod killmail;
mod lp_spend;
mod mail;
mod market_value;
mod miner_defense;
mod mining;
mod mining_hold;
mod missions;
mod moon_belt;
mod moon_scan;
mod nes_use;
mod node_fight;
mod notifications;
mod npc;
mod offline;
mod overheat;
mod pi;
mod pi_p2;
mod pi_p3;
mod probe;
mod prog_mine;
mod prog_null;
mod r_rel_2;
mod r_rel_3;
mod r_rel_4;
mod r_rel_5;
mod rat_switch;
mod reactions;
mod reactor;
mod remote_rep;
mod repair;
mod roam_gang;
mod scram;
mod sec_status;
mod session_gm;
mod site_runner;
mod society;
mod sov;
mod sov_bot;
mod sov_hub;
mod sov_index;
mod t3;
mod theatre_reinforce;
mod threat;
mod trader;
mod travel;
mod verbs;
mod war;
mod wh_dive;
mod wh_raid;
mod wing;
mod wing_fight;
mod wing_words;
mod y_combat;
mod y_escal;
