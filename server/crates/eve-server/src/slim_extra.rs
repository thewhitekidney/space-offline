//! **Extra slim-item attributes** -- W10.5 seam S9 (`docs/content/plan.md`
//! §6.1).
//!
//! A [`crate::destiny::Slim`] carries the five attributes every ball has
//! (plus a hostile NPC's thresholds), and stays `Copy` because it is built by
//! struct literal across the tree. A later packet that needs one more key on
//! some balls' slims -- a site's, a structure's -- does not widen `Slim`: it
//! exports a [`Source`] and the coordinator adds it to [`SOURCES`]. The
//! bootstrap (`destiny::bootstrap_for`) and the `AddBalls2` builder in
//! `destiny::on_ballpark_bind` ask every source about the slims they are
//! about to send, and each slim gets the entry whose item id is its own,
//! appended after its own attributes and its `modules`.
//!
//! [`SOURCES`] lists the packets that add keys; a ball no source names keeps its plain slim.

use crate::service::Registry;
use eve_marshal::PyValue;

/// One slim's extra attributes: `(itemID, [(key, value), …])`.
pub type SlimExtra = (i64, Vec<(&'static str, PyValue)>);

/// Extra attributes for a batch of slims, by item id.
pub type SlimExtras = Vec<SlimExtra>;

/// A source of extras: given the item ids of the slims about to go out,
/// the extras it has for any of them. An id it knows nothing about it
/// leaves out.
pub type Source = fn(&Registry, &[i64]) -> SlimExtras;

/// Every source. **Coordinator-only** after S9: a packet exports its
/// [`Source`] and names the row in its report.
pub const SOURCES: &[Source] = &[
    crate::site::slim_extras,
    crate::wormhole::slim_extras,
    // SOC-C1: a pilot's ship carries `allianceID`.
    crate::alliance::slim_extras,
    // SOC-D2: a sov hub carries `allianceID`, `entosis_score` and `nameID`.
    crate::sov_hub::slim_extras,
    // SOC-D3b: a command node carries `campaign_sourceInfo` and `entosis_score`.
    crate::command_nodes::slim_extras,
    // SEC-1a: a pilot's ship carries `securityStatus` (the overview's
    // Outlaw and Dangerous flags).
    crate::sec_status::slim_extras,
    // FT-C: a fighter squadron's slim carries `fighter.squadronSize`.
    crate::fighters::slim_extras,
    // PROG-MINE: a ship with an online compressor is a compression facility.
    crate::compression::slim_extras,
    // T3-E: a tactical destroyer's ship carries `shipStance`.
    crate::stance::slim_extras,
    // GAP-SELFDESTRUCT: a ship counting down carries `selfDestructTime`.
    crate::self_destruct::slim_extras,
    // GAP-KILLMARKS: a ship with kill marks carries `kills`.
    crate::killmarks::slim_extras,
    // FW-1: an enlisted pilot's ship carries `warFactionID`.
    crate::service::facwar_mgr::slim_extras,
    // U3: an Upwell structure carries `state`, `timer`, `upkeepState`, ...
    crate::structure_space::slim_extras,
    // BUBBLES-SB: a warp disrupt probe carries `warpDisruptionStartTime`.
    crate::bubble::slim_extras,
    // FIX-W7A: an ESS warp beacon carries a `name` (the warp raised without).
    crate::ess::slim_extras,
];

/// Every source's extras for `ids`, concatenated in table order.
pub fn collect(sources: &[Source], w: &Registry, ids: &[i64]) -> SlimExtras {
    sources.iter().flat_map(|source| source(w, ids)).collect()
}

/// [`collect`] over [`SOURCES`]: what the builders call.
pub fn for_slims(w: &Registry, ids: &[i64]) -> SlimExtras {
    collect(&crate::module::plan().slim, w, ids)
}
