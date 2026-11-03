//! What a probe scan can find: the sources `scan::request_scans` asks.
//!
//! W10.5 seam S8 (`docs/content/plan.md` §6.1). Until this seam the probe
//! scan called [`crate::scan::scan_ships`] by name, so the first packet with
//! something else to find (a cosmic signature, an NPC) would have edited the
//! scan itself. A packet now exports a [`Source`] and the coordinator adds it
//! to [`SOURCES`]; the results concatenate in table order.

use crate::scan::{Probe, ProbeResult};
use crate::service::Registry;

/// One source: `(world, scanning character, the probes that scanned)`.
pub type Source = fn(&mut Registry, i64, &[Probe]) -> Vec<ProbeResult>;

/// Every source a probe scan reports from, in the order its results go out.
pub const SOURCES: &[Source] = &[
    crate::scan::scan_ships,
    // Cosmic signatures (W14 CP29).
    crate::signal_tracker::signatures,
];

/// Every source's results for one scan, concatenated in `table` order.
pub fn run(
    table: &[Source],
    w: &mut Registry,
    character_id: i64,
    probes: &[Probe],
) -> Vec<ProbeResult> {
    let mut out = Vec::new();
    for source in table {
        out.extend(source(w, character_id, probes));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    fn result(target_id: &str) -> ProbeResult {
        ProbeResult {
            target_id: target_id.to_owned(),
            scan_group_id: 0,
            group_id: 0,
            type_id: 0,
            item_id: 0,
            certainty: 1.0,
            pos: [0.0; 3],
            deviation: 0.0,
            strength_attribute: None,
        }
    }
    fn two(_: &mut Registry, _: i64, _: &[Probe]) -> Vec<ProbeResult> {
        vec![result("A-1"), result("A-2")]
    }
    fn none(_: &mut Registry, _: i64, _: &[Probe]) -> Vec<ProbeResult> {
        vec![]
    }
    fn one(_: &mut Registry, _: i64, _: &[Probe]) -> Vec<ProbeResult> {
        vec![result("B-1")]
    }

    /// Each source's results, in table order and each source's own order;
    /// a source that hears nothing adds nothing, and an empty table finds
    /// nothing.
    #[test]
    fn results_concatenate_in_table_order() {
        let mut w = Registry::new(SandboxConfig::default());
        let ids = |rs: Vec<ProbeResult>| rs.into_iter().map(|r| r.target_id).collect::<Vec<_>>();
        assert_eq!(
            ids(run(&[two, none, one], &mut w, 1, &[])),
            ["A-1", "A-2", "B-1"]
        );
        assert_eq!(ids(run(&[one, two], &mut w, 1, &[])), ["B-1", "A-1", "A-2"]);
        assert!(run(&[], &mut w, 1, &[]).is_empty());
    }
}
