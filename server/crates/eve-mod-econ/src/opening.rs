//! **The PLEX seed**: one finite stack of PLEX on sale at a reference price,
//! listed once at the NPC corporation that owns the station of the first
//! pilot to log in, and never restocked. It is how PLEX gets a first price
//! before any drop has been looted. The ISK a buyer pays goes to a
//! corporation with no wallet, so it leaves the economy (a sink).
//!
//! **Ledgered**: the stack is made by `Registry::seed_ask`, and the same step
//! adds [`tunables::SEED_UNITS`] to the `seed` row of the `nes` ledger.
//! **Once**: the `seed` row of [`KIND`] says it was done; a world that has it
//! never seeds again, whatever the tunables say now.

use crate::KIND;
use crate::tunables;
use eve_server::nes_vault;
use eve_server::service::Registry;

fn key() -> Vec<String> {
    vec!["seed".to_owned()]
}

/// Whether the stack was already listed in this world.
pub fn seeded(reg: &Registry) -> bool {
    reg.deltas().of(KIND).contains_key(&key())
}

/// The log-in row: list the seed at the pilot's home station if it was never
/// listed. A station with no NPC owner, or none the market knows, listed
/// nothing and is tried again at the next log-in.
pub fn on_login(reg: &mut Registry, character: i64) {
    if seeded(reg) {
        return;
    }
    let Some(station) = reg.character(character).and_then(|c| c.station_id) else {
        return;
    };
    let Ok((order, owner)) = reg.seed_ask(
        character,
        station,
        nes_vault::TYPE_PLEX,
        tunables::SEED_UNITS,
        tunables::SEED_PRICE,
        tunables::SEED_DAYS,
    ) else {
        return;
    };
    nes_vault::ledger_in(reg.deltas_mut(), "seed", tunables::SEED_UNITS);
    reg.deltas_mut().put(
        KIND,
        key(),
        vec![
            order.to_string(),
            station.to_string(),
            owner.to_string(),
            tunables::SEED_UNITS.to_string(),
        ],
    );
    eve_server::outln!(
        "[econ-mod] plex seed: {} PLEX at {} ISK, corp {owner}, station {station}",
        tunables::SEED_UNITS,
        tunables::SEED_PRICE
    );
}
