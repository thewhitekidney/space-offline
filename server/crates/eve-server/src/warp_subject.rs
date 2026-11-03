//! What a warp's subject and id name: the table `service/space.rs::warp`
//! resolves its destination through.
//!
//! W10.5 seam S8 (`docs/content/plan.md` §6.1). Until this seam the rows were
//! the arms of one `match subject` inside `warp`, so every packet that taught
//! warp a new kind of destination edited the same function. A packet now
//! exports a [`Resolver`] and the coordinator adds its row to [`RESOLVERS`].
//!
//! **The first `Some` wins**, among the rows whose subject matches, and the
//! rows are tried in table order. That is what lets a later packet add a
//! second `item` row, for ids with no ball, without touching the first: a
//! ball id still resolves where it always did, and only an id the first row
//! answers `None` for falls through to the new one.

use crate::model::Target;
use crate::service::Registry;
use eve_marshal::PyValue;

/// One row's resolution: `(world, actor, id, raw)`.
///
/// `id` is the destination as an integer, and `0` for a subject whose id is
/// not one (`scan`, whose id is a string). `raw` is the destination argument
/// as the client sent it, for exactly those rows.
pub type Resolver = fn(&Registry, i64, i64, Option<&PyValue>) -> Option<Target>;

/// Every subject a warp resolves, in the order the `match` had them.
pub const RESOLVERS: &[(&[u8], Resolver)] = &[
    // **Two subjects, and the id means a different thing in each.** `'item'`
    // carries a ball id -- a station or another ship; `'char'` carries a
    // **character** id, which is what a fleet member's "Warp to" sends. The
    // second was missing, so every fleet warp answered `None` and pushed
    // nothing, and both players reported being unable to warp to each other
    // with no trace on either side.
    (crate::space_state::WARP_SUBJECT_ITEM, item),
    // An anomaly's site id, which has no ball (W12 CP17): only reached when
    // the ball row above answers `None`.
    (
        crate::space_state::WARP_SUBJECT_ITEM,
        crate::signal_tracker::anomaly_target,
    ),
    // A site object that is not a rock (W14 CP28a): no roster ball, so
    // only reached when the rows above answer `None`.
    (
        crate::space_state::WARP_SUBJECT_ITEM,
        crate::site::object_target,
    ),
    (crate::space_state::WARP_SUBJECT_CHAR, character),
    (crate::service::WARP_SUBJECT_BOOKMARK, bookmark),
    // A probe result's "Warp to" (`menusvc._WarpXToScanResult:1700`):
    // the ship the target id names, where it is now.
    (crate::scan::WARP_SUBJECT_SCAN, scan),
    // A scanned-down signature (W14 CP29): only reached when the ships' row
    // above answers `None`, and only after a perfect probe result.
    (
        crate::scan::WARP_SUBJECT_SCAN,
        crate::signal_tracker::signature_target,
    ),
    // An escalation's site, from the Expeditions transmission, the Agency
    // and the Job Board (`warpSubjects.EPIC_INSTANCE`, Y-ESCAL).
    (
        crate::escalation::WARP_SUBJECT,
        crate::escalation::warp_target,
    ),
    // An agent bookmark's pocket, from `agentMgr.WarpToLocation`: the
    // id is the agent's. Our own subject, not one of the client's.
    (crate::mission::WARP_SUBJECT_MISSION, mission),
    // A fleet-mate's pocket, for "Warp Fleet to" (`fleet_warp::follow_if`).
    (crate::mission::WARP_SUBJECT_MISSION_FLEET, mission_fleet),
    // Dungeons and the rest of the thirteen: no row, so `None`.
];

/// The first `Some` among `table`'s rows for `subject`, in table order.
///
/// `None` when no row matches the subject, or every matching row answers
/// `None`: the warp is refused.
pub fn resolve(
    table: &[(&[u8], Resolver)],
    w: &Registry,
    subject: &[u8],
    actor: i64,
    id: i64,
    raw: Option<&PyValue>,
) -> Option<Target> {
    table
        .iter()
        .filter(|(s, _)| *s == subject)
        .find_map(|(_, resolver)| resolver(w, actor, id, raw))
}

fn item(w: &Registry, _actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    w.ball_target(id)
}

fn character(w: &Registry, _actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    w.character_target(id)
}

fn bookmark(w: &Registry, actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    crate::service::bookmark_target(w, actor, id)
}

fn scan(w: &Registry, actor: i64, _id: i64, raw: Option<&PyValue>) -> Option<Target> {
    raw.and_then(|d| crate::scan::warp_target(w, actor, d))
}

fn mission(w: &Registry, actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    crate::mission::warp_target(w, actor, id)
}

fn mission_fleet(w: &Registry, actor: i64, id: i64, _raw: Option<&PyValue>) -> Option<Target> {
    crate::mission::fleet_warp_target(w, actor, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SandboxConfig;

    fn at(x: f64) -> Option<Target> {
        Some(Target {
            position: [x, 0.0, 0.0],
            radius: 0.0,
            warp_in: None,
        })
    }
    fn nothing(_: &Registry, _: i64, _: i64, _: Option<&PyValue>) -> Option<Target> {
        None
    }
    fn first(_: &Registry, _: i64, _: i64, _: Option<&PyValue>) -> Option<Target> {
        at(1.0)
    }
    fn second(_: &Registry, _: i64, _: i64, _: Option<&PyValue>) -> Option<Target> {
        at(2.0)
    }
    fn other(_: &Registry, _: i64, _: i64, _: Option<&PyValue>) -> Option<Target> {
        at(3.0)
    }

    /// The rule a later packet relies on to add a second row for a subject:
    /// a row answering `None` falls through, the first `Some` wins, a row for
    /// another subject is never consulted, and a subject with no row refuses.
    #[test]
    fn first_match_wins_and_unknown_subjects_refuse() {
        let table: &[(&[u8], Resolver)] = &[
            (b"x", other),
            (b"a", nothing),
            (b"a", first),
            (b"a", second),
        ];
        let w = Registry::new(SandboxConfig::default());
        assert_eq!(resolve(table, &w, b"a", 1, 7, None), at(1.0));
        assert_eq!(resolve(table, &w, b"x", 1, 7, None), at(3.0));
        assert_eq!(resolve(table, &w, b"zz", 1, 7, None), None);
        assert_eq!(resolve(&table[1..2], &w, b"a", 1, 7, None), None);
        assert_eq!(resolve(&[], &w, b"a", 1, 7, None), None);
    }
}
