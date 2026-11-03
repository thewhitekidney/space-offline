//! **Kill hooks** -- seam S13 (packet IP34, `docs/review/plan.md` §4.6,
//! `docs/review/V1-architecture.md` F1 and A1).
//!
//! What a death sets in motion beyond the death itself. `combat::kill`
//! builds one [`KillEvent`] for a player's ship, capsule or drone and fires
//! [`PLAYER`]; `combat::kill_npc` builds one for an NPC and fires [`NPC`].
//! The two paths are disjoint: one kill fires exactly one table, once.
//!
//! **Coordinator-only** from IP34 on, like `session_hooks::ON_LOGOFF`: a
//! packet exports its [`Hook`] and names the row in its report. A kill side
//! effect (insurance, killmails, kill rights, security status, faction
//! warfare points, a site squad cleared) is a row here, never an edit to
//! `combat.rs`.
//!
//! **Where each table fires**, and what a hook can still read there:
//!
//! - [`PLAYER`] fires **first** in `combat::kill`, before the death plan:
//!   the dead ship, its contents and its roster entry are all still there,
//!   which is what a killmail or an insurance payout wants to read. Its one
//!   row, `drones::destroyed`, is the whole death of a drone; for a ship or
//!   a capsule it finds no drone and does nothing, as `kill`'s drone check
//!   did before.
//! - [`NPC`] fires **last** in `combat::kill_npc`: the wreck is out, the
//!   locks are released and the ball has left the roster (so
//!   `dungeon::cleared` counts it dead), but the record is still in the NPC
//!   table, which `npc::pay_bounty` reads its bounty from. The record leaves
//!   the table after the hooks.
//!
//! The three side effects `combat.rs` called by hand before IP34 are the
//! first rows, in their order: `pay_bounty`, `drones::destroyed`,
//! `mission::npc_killed`. **One ordering moved** `[A]`: the bounty's
//! `OnAccountChange` is now scheduled after the kill batch and `TargetLost`
//! of the same instant, rather than before them. Both go out on the same
//! clock drain; falsifier: a client that reads the wallet change as part of
//! the explosion (none known).

use crate::damage::BlueTime;
use crate::service::Registry;

/// One death, built once by `combat::kill` or `combat::kill_npc`.
#[derive(Debug, Clone)]
pub struct KillEvent {
    /// The roster key that died: a character, an NPC's id, or a drone's
    /// item id.
    pub victim_character: i64,
    /// The ball that died: the ship, the capsule, the NPC or the drone.
    pub victim_ship: i64,
    /// Its type: the hull, the capsule, the NPC's or the drone's type.
    pub ship_type: i64,
    /// The roster key that fired the killing volley.
    pub killer: Option<i64>,
    pub system: i64,
    /// `Some` for an NPC, and only then; it selects [`NPC`].
    pub npc: Option<&'static crate::npc::NpcType>,
    pub when: BlueTime,
    /// The characters shown the death (the victim's park).
    pub park: Vec<i64>,
}

/// What a hook does with the world, for one death.
pub type Hook = fn(&mut Registry, &KillEvent);

/// One hook, named for diagnostics and for the disjointness test.
#[derive(Clone, Copy)]
pub struct Row {
    pub name: &'static str,
    pub run: Hook,
}

/// Every hook a player-side death runs (a ship, a capsule, a drone), in
/// order. Coordinator-only.
pub const PLAYER: &[Row] = &[
    Row {
        name: "drones_destroyed",
        run: drones_destroyed,
    },
    // A squadron whose health ran out loses a fighter, or all of them
    // (FT-E).
    Row {
        name: "fighters_destroyed",
        run: fighters_destroyed,
    },
    // A ship under contract pays its owner (W16 CP44, C5 §5.5).
    Row {
        name: "insurance_on_kill",
        run: crate::service::insurance::on_kill,
    },
    // A character's ship or capsule gets a kill report (SOC-C6).
    Row {
        name: "killmail_on_kill",
        run: crate::killmail::on_kill,
    },
    // A bounty on the victim is claimed by the attackers; it patches the report.
    Row {
        name: "player_bounty_on_kill",
        run: crate::bounty_pool::on_kill,
    },
    // The victim and the final blow are notified of the report (NOTIF-1).
    Row {
        name: "notifications_on_kill",
        run: crate::notifications::on_kill,
    },
    // A capsuleer's death ends their engagements; a criminal kill earns a
    // kill right, a kill under an activated one uses it (CW-1).
    Row {
        name: "crimewatch_on_kill",
        run: crate::crimewatch::on_kill,
    },
    // FW-5: a kill of the enemy militia counts for each attacker, their
    // corp, alliance and militia; it reads the report `killmail_on_kill` wrote.
    Row {
        name: "fw_stats_on_kill",
        run: crate::fw_stats::on_kill,
    },
    // #133: a kill of the enemy militia pays the attackers LP by the ISK
    // destroyed; it reads the report `killmail_on_kill` wrote.
    Row {
        name: "fw_lp_on_kill",
        run: crate::fw_lp::on_kill,
    },
    // A kill between two sides at war is filed under the war (C8-b); it
    // reads the report `killmail_on_kill` just wrote.
    Row {
        name: "war_on_kill",
        run: crate::war::on_kill,
    },
    // The offender's ship died: CONCORD lingers, then leaves (CONCORD-1a).
    Row {
        name: "concord_on_kill",
        run: crate::concord::on_kill,
    },
    // A dead ship loses its fleet target tag.
    Row {
        name: "fleet_untag",
        run: fleet_untag,
    },
    // GAP-KILLMARKS: a pilot's ship kill adds a mark to the killer's ship;
    // a dead ship's marks go.
    Row {
        name: "killmarks_on_kill",
        run: crate::killmarks::on_kill,
    },
    // A pilot's ship or pod death goes in the map's hourly statistics.
    Row {
        name: "map_stats_on_kill",
        run: crate::map_stats::on_kill,
    },
];

/// Every hook an NPC's death runs, in order. Coordinator-only.
pub const NPC: &[Row] = &[
    Row {
        name: "pay_bounty",
        run: pay_bounty,
    },
    // The killer's security status rises by the type's 252, capped by its
    // 563 (W17 CP50, C6 N8).
    Row {
        name: "security_status_on_kill",
        run: crate::npc::bounty::on_kill,
    },
    Row {
        name: "mission_npc_killed",
        run: mission_npc_killed,
    },
    // A site squad's death counts toward the site's completion (W15 CP28b).
    Row {
        name: "site_squad_killed",
        run: crate::site_spawn::npc_killed,
    },
    // An NPC killed in a claimed system raises its military index (SOC-D4).
    Row {
        name: "sov_military",
        run: crate::sov_index::on_npc_kill,
    },
    // A dead NPC loses its fleet target tag.
    Row {
        name: "fleet_untag_npc",
        run: fleet_untag,
    },
    // An NPC death goes in the map's hourly statistics (NPC kills column).
    Row {
        name: "map_stats_on_npc_kill",
        run: crate::map_stats::on_kill,
    },
];

/// The table `ev` belongs to: [`NPC`] for an NPC, [`PLAYER`] otherwise, each
/// with the modules' rows after it ([`crate::module::plan`]).
pub fn table_for(ev: &KillEvent) -> &'static [Row] {
    let plan = crate::module::plan();
    if ev.npc.is_some() {
        &plan.kill_npc
    } else {
        &plan.kill_player
    }
}

/// Run every row of `hooks` for `ev`, in table order, each once.
pub fn fire(hooks: &[Row], w: &mut Registry, ev: &KillEvent) {
    for row in hooks {
        #[cfg(test)]
        tests::TRACE.with(|t| t.borrow_mut().push(row.name));
        (row.run)(w, ev);
    }
}

/// **An NPC's bounty**, to whoever fired the killing volley
/// (`npc::pay_bounty`). It reads the bounty off the NPC table, which is why
/// [`NPC`] fires before the record leaves it.
fn pay_bounty(w: &mut Registry, ev: &KillEvent) {
    if let Some(killer) = ev.killer {
        crate::npc::pay_bounty(w, killer, ev.victim_character);
    }
}

/// **A tagged target died**: its tag leaves every fleet, and the members are
/// told (`Fleets::untag_item`). The ball is the ship or NPC; a drone's key is
/// its item id, so both ids are tried.
fn fleet_untag(w: &mut Registry, ev: &KillEvent) {
    for id in [ev.victim_ship, ev.victim_character] {
        let e = w.fleets_mut().untag_item(id);
        w.fleet_apply(e);
    }
}

/// **A drone shot to pieces** (`drones::destroyed`): no capsule, no wreck,
/// no revive. Nothing for a ship or a capsule: it finds no drone record.
fn drones_destroyed(w: &mut Registry, ev: &KillEvent) {
    crate::drones::destroyed(w, ev.victim_character, ev.park.clone());
}

/// **A fighter lost** (`fighters::destroyed`): the squadron shrinks, or the
/// last fighter takes it out of space. Nothing for a ship or a drone.
fn fighters_destroyed(w: &mut Registry, ev: &KillEvent) {
    crate::fighters::destroyed(w, ev.victim_character, ev.park.clone());
}

/// **The mission that NPC belonged to** (`mission::npc_killed`), so a
/// pocket's last kill ticks the objective live.
fn mission_npc_killed(w: &mut Registry, ev: &KillEvent) {
    crate::mission::npc_killed(w, ev.victim_character);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::damage::{Damage, Shot};
    use std::cell::RefCell;

    thread_local! {
        /// Every row name [`fire`] ran on this test thread, in order.
        pub(crate) static TRACE: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    const ALICE: i64 = 90_000_101;
    const BOB: i64 = 90_000_102;
    const ALICE_SHIP: i64 = 100_000_101;
    const BOB_SHIP: i64 = 100_000_102;
    const RIFTER: i64 = 587;

    fn take_trace() -> Vec<&'static str> {
        TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
    }

    fn names(table: &[Row]) -> Vec<&'static str> {
        table.iter().map(|r| r.name).collect()
    }

    /// Alice and Bob in Rifters, both undocked into one system.
    fn world() -> Registry {
        let pilot = |id: i64, name: &str, ship: i64| crate::model::Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ship_type_id: RIFTER,
            ..crate::model::Character::default()
        };
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![
                pilot(ALICE, "Alice", ALICE_SHIP),
                pilot(BOB, "Bob", BOB_SHIP),
            ];
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        let _ = reg.take_pending();
        reg
    }

    /// One overwhelming volley from Alice at `victim`.
    fn kill_by_alice(reg: &mut Registry, victim: i64) {
        let t0 = crate::pyutil::filetime_now();
        let v = reg.roster().get(victim).copied().expect("in space");
        let alice_ship = reg.roster().get(ALICE).expect("Alice in space").ship_id;
        let shot = Shot {
            at: t0,
            attacker_ship_id: alice_ship,
            weapon_item_id: 1,
            weapon_type_id: 3640,
            target_ship_id: v.ship_id,
            damage: Damage {
                em: 1.0e9,
                explosive: 1.0e9,
                kinetic: 1.0e9,
                thermal: 1.0e9,
            },
            hit_quality: crate::combat::HIT_QUALITY,
        };
        crate::combat::fire(reg, shot, ALICE, v, t0);
    }

    thread_local! {
        static SEEN: RefCell<Vec<(&'static str, i64)>> = const { RefCell::new(Vec::new()) };
    }
    fn first(_: &mut Registry, ev: &KillEvent) {
        SEEN.with(|s| s.borrow_mut().push(("first", ev.victim_ship)));
    }
    fn second(_: &mut Registry, ev: &KillEvent) {
        SEEN.with(|s| s.borrow_mut().push(("second", ev.victim_ship)));
    }

    /// A local table runs in its order, each row once per fire, each with
    /// the event it was given; and a real NPC kill fires [`NPC`] exactly
    /// once, row by row in table order.
    #[test]
    fn hooks_fire_in_table_order_once_per_kill() {
        let mut reg = world();
        let local = [
            Row {
                name: "first",
                run: first,
            },
            Row {
                name: "second",
                run: second,
            },
        ];
        let ev = KillEvent {
            victim_character: 7,
            victim_ship: 8,
            ship_type: 9,
            killer: None,
            system: 10,
            npc: None,
            when: 0,
            park: Vec::new(),
        };
        let _ = take_trace();
        fire(&local, &mut reg, &ev);
        assert_eq!(
            SEEN.with(|s| s.borrow().clone()),
            vec![("first", 8), ("second", 8)]
        );
        assert_eq!(take_trace(), vec!["first", "second"]);

        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let npc = crate::npc::spawn(
            &mut reg,
            &crate::npc::ANGEL_HIJACKER,
            system,
            [at[0] + 3_000.0, at[1], at[2]],
        )
        .expect("an id");
        kill_by_alice(&mut reg, npc);
        assert!(reg.roster().get(npc).is_none(), "one volley kills it");
        assert_eq!(take_trace(), names(NPC), "the NPC table, once, in order");
        assert!(
            reg.npcs().get(npc).is_none(),
            "the record leaves after the hooks"
        );
    }

    /// An NPC's death fires only [`NPC`] and a player's only [`PLAYER`];
    /// no row is in both, and [`table_for`] picks by `npc` alone.
    #[test]
    fn npc_and_player_tables_are_disjoint_paths() {
        for p in PLAYER {
            assert!(
                !names(NPC).contains(&p.name),
                "{} is in both tables",
                p.name
            );
        }
        let mut ev = KillEvent {
            victim_character: 1,
            victim_ship: 2,
            ship_type: 3,
            killer: Some(4),
            system: 5,
            npc: None,
            when: 0,
            park: Vec::new(),
        };
        assert_eq!(names(table_for(&ev)), names(PLAYER));
        ev.npc = Some(&crate::npc::ANGEL_HIJACKER);
        assert_eq!(names(table_for(&ev)), names(NPC));

        let mut reg = world();
        let system = reg.roster().get(ALICE).expect("in space").solar_system_id;
        let at = reg.roster().get(ALICE).expect("in space").motion.position;
        let npc = crate::npc::spawn(
            &mut reg,
            &crate::npc::ANGEL_HIJACKER,
            system,
            [at[0] + 3_000.0, at[1], at[2]],
        )
        .expect("an id");
        let _ = take_trace();
        kill_by_alice(&mut reg, npc);
        assert_eq!(take_trace(), names(NPC), "an NPC death: the NPC table only");

        kill_by_alice(&mut reg, BOB);
        assert_ne!(
            reg.roster().get(BOB).map(|p| p.ship_id),
            Some(BOB_SHIP),
            "Bob's ship is dead"
        );
        assert_eq!(
            take_trace(),
            names(PLAYER),
            "a player death: the player table only"
        );
    }
}
