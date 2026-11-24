//! Typed extension slots on the `Registry`.
//!
//! W10.5 seam S7(a), `docs/content/plan.md` §6.1. A module that needs world
//! state of its own (a liveness index, a site population) used to need a
//! field on `Registry`, which meant an edit to `service.rs`, the hottest file
//! in the crate. It registers a [`SlotSpec`] in [`SLOTS`] instead, and reads
//! its state back with `reg.ext::<T>()` and `reg.ext_mut::<T>()`.
//!
//! [`SLOTS`] is **coordinator-only**, like `DOMAINS`: a packet exports its
//! spec, and the coordinator adds the line at landing.

use crate::content::Content;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;

/// One slot's state, boxed.
pub type Boxed = Box<dyn Any + Send + Sync>;

/// One registered slot: its name, and how to build its first value from the
/// loaded content.
#[derive(Clone, Copy)]
pub struct SlotSpec {
    /// For the panic that names a clash, and for a reader.
    pub name: &'static str,
    /// Called once, when the `Registry` is built.
    pub make: fn(&Content) -> Boxed,
}

/// Every slot a `Registry` carries.
pub const SLOTS: &[SlotSpec] = &[
    // U4: which docked pilots hold a ballpark, and which undocks wait for a bind.
    crate::structure_dock::SLOT,
    crate::bot_control::SLOT,
    // admin-web: the listener the clock hands snapshots to.
    crate::admin_web::SLOT,
    // #205: the image server URL dispatch advertises at login.
    crate::images::SLOT,
    // #57: which container passwords each pilot has entered this run.
    crate::containers::SLOT,
    // JAM-1: what each system's cyno jammer was last told as.
    crate::sov_jammer::SLOT,
    // #82: when each MTU's next cycle is due.
    crate::mtu::SLOT,
    // MINING-CRITS: the dev knob that forces a crit or residue.
    crate::mining::SLOT,
    // MAP-STATS: the hourly jump and kill counter behind map.GetHistory.
    crate::map_stats::SLOT,
    // SESSION-FIX2: running safe-logoff countdowns.
    crate::safe_logoff::SLOT,
    // GAP-SELFDESTRUCT: running self-destruct sequences.
    crate::self_destruct::SLOT,
    // #133: the insurance paid for the ship just killed, which an FW kill subtracts.
    crate::fw_lp::SLOT,
    // UNDOCK-INVULN: who is protected after an undock or a gate arrival.
    crate::invuln::SLOT,
    // CLOAK-1: who is cloaked and who is recalibrating.
    crate::cloak::SLOT,
    // NW-9a: doomsdays in the warning or burning, and who may not move.
    crate::superweapon::SLOT,
    crate::liveness::SLOT,
    // BOT-PANEL: the dev Bot Control agent's groups and issued /bots lines.
    crate::bot_panel::SLOT,
    crate::live_status::SLOT,
    crate::signal_tracker::SLOT,
    // PERF-HOLDS: the kept config.GetAverageMarketPrices answer.
    crate::econ_prices::SLOT,
    // #143: the live self-build cost insurance caps a payout by.
    crate::econ_prices::BUILD_SLOT,
    crate::population::SLOT,
    crate::site::SLOT,
    crate::moon_belt::SLOT,
    // #53 MOON-SCHEDULE: the moon extraction tick's last stamp (the rows are the truth).
    crate::moon_extract::SLOT,
    // MN-D: every survey probe out (R-MOON MN-D).
    crate::moon_scan::SLOT,
    crate::dungeon_tracking::SLOT,
    crate::site_spawn::SLOT,
    crate::combat_site::SLOT,
    crate::loyalty::SLOT,
    crate::mission::SLOT,
    crate::rollover::SLOT,
    crate::econ_dbs::SLOT,
    crate::grid::SLOT,
    crate::storyline::SLOT,
    crate::wormhole::SLOT,
    crate::wormhole_jump::SLOT,
    crate::tackle::SLOT,
    // K-EWAR: what each webbed, jammed, damped or painted target was told.
    crate::ewar::SLOT,
    // K-LOGI: the cycles of each rep already applied.
    crate::remote_rep::SLOT,
    // K-BOOST: each boosted ship's warfare buffs and what its pilot was told.
    crate::dbuff::SLOT,
    // K-OVERHEAT: overloaded modules, rack heat, module damage and repairs.
    crate::overheat::SLOT,
    // K-OVERHEAT: each ship's weapon banks.
    crate::weapon_bank::SLOT,
    crate::service::facwar_mgr::SLOT,
    // M3: every missile in flight, the server's copy (R-MISSILE §2).
    crate::missile::SLOT,
    // W18 CP49: NPC scram and web holds, and what each target was told.
    crate::npc_ewar::SLOT,
    // W17 CP53b: the warzone systems and their gate graph.
    crate::facwar::SLOT,
    // #8 (FW-7): the battlefield scoreboards.
    crate::fw_scoreboard::SLOT,
    // W18 CP58c (CA7): the per-ship dogma sheet cache, one per world.
    crate::dogma::SHEETS,
    // MUTA-3 (#71): the override maps of mutated items, by item id.
    crate::dynamic_item::CACHE,
    // SYS-3: which pilots have been told their system's effect beacon.
    crate::system_effect::SLOT,
    // SOV-UPG-4: which pilots a sov generator holds, and the claimed systems with one.
    crate::sov_effect::SLOT,
    // BOOST: the soonest booster expiry, so the clock's stage is one comparison.
    crate::boosters::SLOT,
    // Lane shutdown: the /shutdown countdown and the server's stop handle.
    crate::shutdown::SLOT,
    // PV-7: a disintegrator's damage ramp, by module.
    crate::pochven::spool::SLOT,
    // HK2: the hacking games, the cans' states and the armed analyzers.
    crate::hacking::SLOT,
    // HK3: the hacked cans that hold loot.
    crate::hacking_loot::SLOT,
    // T3-ANCIENT-RATE: counters of the hacked cans and what they gave.
    crate::ancient_rate::SLOT,
    // GAPS-2: the fleet finder's adverts, join requests and watchlists.
    crate::fleet_finder::SLOT,
    // GAPS-2: ship-clone offers waiting for their target.
    crate::ship_clone::SLOT,
    // SOC-C6: each player ship's recent attackers, for its killmail.
    crate::killmail::SLOT,
    // Lane clock: the scale the slow game timers run at.
    crate::game_clock::SLOT,
    // #141: the world income multipliers.
    crate::econ_scale::SLOT,
    // SOC-D3a: the entosis links' fuel already paid and the last window
    // phase pushed per system.
    crate::entosis::SLOT,
    // CONCORD-1a: the open responses, the pursuers' cycles and the jams.
    crate::concord::SLOT,
    // FT-B: the fighter tubes' tasks and the squadrons in space.
    crate::fighters::SLOT,
    // E1: the offline agents' sweep clock and recent reports.
    crate::econ_agents::SLOT,
    // CAP-K: the /dock-capitals switch.
    crate::dock_policy::SLOT,
    // CAP-D: the lit cynosural fields.
    crate::cyno::SLOT,
    // BUBBLES-SB: the warp disrupt probes out.
    crate::bubble::SLOT,
    // CAP-G: the open jump portals.
    crate::jump_portal::SLOT,
    // #88: the Needlejack groups waiting out their spool.
    crate::random_jump::SLOT,
    // #91: the Abyss probe's pockets.
    crate::abyss::SLOT,
    // #90 ESS-2: the ESS links, their cooldowns and the table of ESS systems.
    crate::ess::SLOT,
    // T3-E: each ship's last stance switch and when it is free again.
    crate::stance::SLOT,
    // NPC-THREAT: who acted on each rat, and the healing they gave.
    crate::npc_threat::SLOT,
    // GW-OUT: notices the world queued for gateway clients, until the turn drains them.
    crate::gateway_out::SLOT,
    // #77 BE-2: ships left in space with no pilot.
    crate::pilotless::SLOT,
    // FIL-WIRE: the filament candidates, built once per world.
    crate::combat::FILAMENT_SLOT,
    // PV-9 (#228): the Pochven loot pools and Triglavian key candidates, built once per world.
    crate::pochven::loot::SLOT,
    // #44: what each Upwell structure took lately, for its repair pause.
    crate::structure_combat::SLOT,
    // HN-1 (#203): who listens to which raffle, and the soonest expiry.
    crate::hypernet::SLOT,
    // #64 PG-1c: who waits in the Proving Grounds queue.
    crate::proving::queue::SLOT,
    // #65 PG-2b: the Proving Grounds matches in play.
    crate::proving::arena::SLOT,
];

/// The slots' state, keyed by type.
#[derive(Default)]
pub struct Slots {
    slots: BTreeMap<TypeId, (&'static str, Boxed)>,
}

impl Slots {
    /// One value per spec, each `make` called once.
    ///
    /// Two specs making the same type would leave one unreachable, so that is
    /// a panic naming both: a registration mistake, caught by the first test
    /// that builds a `Registry`.
    pub fn build(specs: &[SlotSpec], content: &Content) -> Self {
        Slots::try_build(specs, content).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`Self::build`], saying what clashed instead of panicking.
    pub fn try_build(specs: &[SlotSpec], content: &Content) -> Result<Self, String> {
        let mut out = Slots::default();
        for spec in specs {
            let value = (spec.make)(content);
            let key = Any::type_id(&*value);
            if let Some((other, _)) = out.slots.insert(key, (spec.name, value)) {
                return Err(format!(
                    "world slots {other} and {} have the same type; \
                     Registry::ext could only ever return one of them",
                    spec.name
                ));
            }
        }
        Ok(out)
    }

    /// The slot of type `T`, if one is registered.
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.slots
            .get(&TypeId::of::<T>())
            .and_then(|(_, v)| v.downcast_ref())
    }

    /// The same, mutably.
    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.slots
            .get_mut(&TypeId::of::<T>())
            .and_then(|(_, v)| v.downcast_mut())
    }
}

/// The panic for a slot nobody registered, shared by `Registry::ext` and
/// `Registry::ext_mut`.
pub(crate) fn unregistered<T>() -> ! {
    panic!(
        "no world slot of type {}: add its SlotSpec to world_ext::SLOTS",
        std::any::type_name::<T>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static MADE: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug, PartialEq)]
    struct Heat(Vec<i64>);

    fn heat(_: &Content) -> Boxed {
        MADE.fetch_add(1, Ordering::SeqCst);
        Box::new(Heat(Vec::new()))
    }

    /// `heat` without the count: tests run in parallel, so only the
    /// built-once test may move `MADE`.
    fn heat_uncounted(_: &Content) -> Boxed {
        Box::new(Heat(Vec::new()))
    }

    /// A slot is built once, when the slots are, and is then the same value
    /// on every read: a write through `get_mut` is what the next `get` sees.
    /// A type nobody registered is `None`, never another slot's value.
    #[test]
    fn a_registered_slot_is_built_once_and_typed() {
        let specs = [SlotSpec {
            name: "heat",
            make: heat,
        }];
        let before = MADE.load(Ordering::SeqCst);
        let mut slots = Slots::build(&specs, &Content::default());
        assert_eq!(MADE.load(Ordering::SeqCst) - before, 1);

        slots.get_mut::<Heat>().expect("registered").0.push(7);
        assert_eq!(slots.get::<Heat>(), Some(&Heat(vec![7])));
        slots.get_mut::<Heat>().expect("registered").0.push(8);
        assert_eq!(slots.get::<Heat>(), Some(&Heat(vec![7, 8])));
        assert_eq!(MADE.load(Ordering::SeqCst) - before, 1, "built once");

        assert!(slots.get::<String>().is_none());
        assert!(slots.get_mut::<i64>().is_none());
    }

    /// Two specs making one type are refused by name, not by panic, so a
    /// module list can be refused at startup.
    #[test]
    fn try_build_names_a_type_clash() {
        let specs = [
            SlotSpec {
                name: "heat",
                make: heat_uncounted,
            },
            SlotSpec {
                name: "heat2",
                make: heat_uncounted,
            },
        ];
        let e = Slots::try_build(&specs, &Content::default()).err().unwrap();
        assert!(e.contains("heat") && e.contains("heat2"), "{e}");
    }
}
