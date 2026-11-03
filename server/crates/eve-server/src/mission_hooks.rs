//! **Mission hooks** -- W10.5 seam S10 (`docs/content/plan.md` §6.1).
//!
//! What a mission's change sets in motion beyond the conversation itself --
//! a storyline counter, a standings ripple, a site that opens -- belongs to
//! the packet that owns that consequence, not to `mission.rs`. Such a packet
//! exports a [`Hook`] and the coordinator adds `(change, hook)` to
//! [`HOOKS`]. `mission::converse` fires the table after it has stored the
//! character's new progress, whenever the turn changed something.
//!
//! [`HOOKS`] is empty, so nothing fires today.

use crate::mission::Change;
use crate::service::Registry;

/// One mission change, as a hook sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionEvent {
    pub character_id: i64,
    pub agent_id: i64,
    /// The mission the action was about (`Spec::mission_id`).
    pub mission_id: i64,
    pub change: Change,
}

/// What a hook does with the world when its change happens.
pub type Hook = fn(&mut Registry, &MissionEvent);

/// Every hook, keyed by the change it answers. **Coordinator-only** after
/// S10: a packet exports its [`Hook`] and names the row in its report.
pub const HOOKS: &[(Change, Hook)] = &[
    (Change::Completed, crate::loyalty::on_complete),
    (Change::Completed, crate::storyline::on_complete),
    (Change::Completed, crate::epic_arc::on_complete),
];

/// Run every hook in `hooks` whose change is `ev.change`, in table order.
pub fn fire(hooks: &[(Change, Hook)], w: &mut Registry, ev: &MissionEvent) {
    for (change, hook) in hooks {
        if *change == ev.change {
            hook(w, ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each hook records which row it is and the mission it saw, on a
    // thread-local list: tests run in parallel, each on its own thread.
    fn first(w: &mut Registry, ev: &MissionEvent) {
        record(w, 1, ev);
    }

    fn second(w: &mut Registry, ev: &MissionEvent) {
        record(w, 2, ev);
    }

    fn other(w: &mut Registry, ev: &MissionEvent) {
        record(w, 3, ev);
    }

    std::thread_local! {
        static FIRED: std::cell::RefCell<Vec<(i64, i64)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn record(_: &mut Registry, which: i64, ev: &MissionEvent) {
        FIRED.with(|f| f.borrow_mut().push((which, ev.mission_id)));
    }

    /// Only the rows whose change matches fire, in table order, each once.
    #[test]
    fn only_matching_hooks_fire_in_table_order() {
        let table: &[(Change, Hook)] = &[
            (Change::Completed, second),
            (Change::Accepted, other),
            (Change::Completed, first),
        ];
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        let ev = MissionEvent {
            character_id: 90_000_001,
            agent_id: 3_000_001,
            mission_id: 4_000_001,
            change: Change::Completed,
        };
        FIRED.with(|f| f.borrow_mut().clear());
        fire(table, &mut w, &ev);
        let fired = FIRED.with(|f| f.borrow().clone());
        assert_eq!(fired, vec![(2, 4_000_001), (1, 4_000_001)]);
    }
}
