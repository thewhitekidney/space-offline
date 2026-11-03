//! **Settle hooks** (lane l7b, issue #111): what an offline kill sets in
//! motion beyond the loot the settlement already books. A parked fighter
//! (`econ_agents`) kills NPCs in bulk with no ball in space; for each kill
//! `econ_agents::combat` builds one [`Settled`] and runs every row a module
//! added. The core has no rows: a drop table of the owner's (PLEX, injectors,
//! in `eve-mod-econ`) is a choice of theirs, not CCP's.
//!
//! The event is an ordinary [`KillEvent`] with `victim_ship == 0` and
//! `victim_character == 0`: there is no wreck, so a row pays the killer's
//! hangar at [`Settled::station`] instead, through the same ledger keys it
//! uses for a wreck. A row must be deterministic in the event (the stream
//! key is [`Settled::seed`], not an NPC id).

use crate::kill_hooks::KillEvent;
use crate::service::Registry;

/// One settled kill.
#[derive(Debug, Clone)]
pub struct Settled {
    /// `victim_ship == 0`, `npc` set, `killer` the parked character.
    pub ev: KillEvent,
    /// Where the killer's hangar is: the place to pay into.
    pub station: i64,
    /// A stable key for this kill's roll (`econ_agents::combat::npc_seed`),
    /// standing where an NPC's id stands for a live kill.
    pub seed: i64,
}

/// What a row does with the world, for one settled kill.
pub type Hook = fn(&mut Registry, &Settled);

/// One row, named for diagnostics and the uniqueness check.
#[derive(Clone, Copy)]
pub struct Row {
    pub name: &'static str,
    pub run: Hook,
}

/// The core's rows: none.
pub const SETTLED: &[Row] = &[];

/// Run every module row for `s`.
pub fn fire(reg: &mut Registry, s: &Settled) {
    for row in &crate::module::plan().settled {
        (row.run)(reg, s);
    }
}
