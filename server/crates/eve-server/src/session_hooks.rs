//! **Session hooks** -- W13 seam S14 (packet IP28-a, `docs/review/plan.md`
//! §4.5).
//!
//! What a character's log-on or log-off sets in motion beyond the session
//! table itself. [`crate::service::Registry::session_closed`] stamps the
//! logoff date first (`stamp_logoff`, which stays in `service.rs`) and then
//! runs [`ON_LOGOFF`]; `Registry::session_selected_character` runs
//! [`ON_LOGIN`]. Both fire only on the edge: after the character's **last**
//! session closes, and when their **first** one selects them. A second
//! client window is neither a log-on nor a log-off.
//!
//! **Coordinator-only** from IP28-a on, like `rollover_steps::STEPS`: a
//! packet exports its [`Hook`] and names the row in its report; a logoff
//! step is a row here, never an edit to `service.rs`.
//!
//! The five [`ON_LOGOFF`] rows are the steps `session_closed` ran inline
//! before IP28-a, moved verbatim and in their order. Four of them put their
//! pushes **on the clock** (`schedule`), because a disconnect is not a
//! payload (`server.rs:402`) and nothing would drain `pending` for it.

use std::time::Duration;

use crate::service::Registry;

/// What a hook does with the world, for one character.
pub type Hook = fn(&mut Registry, i64);

/// One hook, named for diagnostics and for the uniqueness test.
#[derive(Clone, Copy)]
pub struct Row {
    pub name: &'static str,
    pub run: Hook,
}

/// Every log-off step after the logoff stamp, in the order they run.
pub const ON_LOGOFF: &[Row] = &[
    Row {
        name: "stop_modules",
        run: stop_modules,
    },
    Row {
        name: "contact_presence",
        run: contact_presence_off,
    },
    Row {
        name: "guest_frame",
        run: guest_frame_off,
    },
    Row {
        name: "logged_off",
        run: logged_off,
    },
    Row {
        name: "fleet_leave",
        run: fleet_leave,
    },
    // GAP-TRADE: a trade in progress is cancelled and its items go home.
    Row {
        name: "trade",
        run: crate::trade::on_logoff,
    },
    // ESS-2: a link dies with its pilot's session; a bot has none, so this
    // is the hook and not an `is_online` check on the tick.
    Row {
        name: "ess_unlink",
        run: crate::ess::on_logoff,
    },
    // U6C: a pilot at a structure's controls leaves the seat with his session,
    // so the relog is a pilot in his hull (no push: the session is closed).
    Row {
        name: "structure_control",
        run: structure_control_off,
    },
    // Last: chat's directory drops the pilot (PA-11).
    Row {
        name: "chat_dir",
        run: crate::chat_dir::on_logoff,
    },
];

/// Every log-on step, after the character's first session selects them.
/// Empty: nothing ran there before S14. (`tales::on_enter_system`, CP51, is
/// an arrival hook keyed on `solarsystemid2`, not a log-on one.)
pub const ON_LOGIN: &[Row] = &[
    // E1: a parked bot's offline career is settled before its first read.
    Row {
        name: "econ_agents",
        run: crate::econ_agents::on_login,
    },
    // Chat's directory learns the pilot is online before any chat token (PA-11).
    Row {
        name: "chat_dir",
        run: crate::chat_dir::on_login,
    },
    // SKILLGATE: a module the pilot cannot use goes offline, for worlds
    // saved before the gate.
    Row {
        name: "skillgate",
        run: skillgate_login,
    },
];

fn structure_control_off(w: &mut Registry, id: i64) {
    crate::structure_dock::control::drop_control(w, id, false);
}

fn skillgate_login(w: &mut Registry, id: i64) {
    if let Some(ship) = w.character(id).map(|c| c.ship_id) {
        crate::modules::offline_unskilled(w, id, ship);
    }
}

/// Run every row of `hooks` for `character_id`, in table order, each once.
pub fn fire(hooks: &[Row], w: &mut Registry, character_id: i64) {
    for row in hooks {
        (row.run)(w, character_id);
    }
}

/// **Stop the ship's running modules.** A laser left running on a disconnect
/// vanished a cycle later with no push, so a pilot who relogged inside that
/// cycle was refused `EffectAlreadyActive2` on a module their client showed
/// as idle (found by the P8b bot lane). Running only: an offline module stays
/// offline.
fn stop_modules(w: &mut Registry, id: i64) {
    if let Some(ship) = w.character(id).map(|c| c.ship_id) {
        let fitted: Vec<i64> = w.store().contents_of(ship).map(|i| i.item_id).collect();
        let now = crate::pyutil::filetime_now();
        for module in fitted {
            crate::modules::stop(w, module, now, None);
        }
    }
}

/// On the clock, like the guest frame below and for its reason.
fn contact_presence_off(w: &mut Registry, id: i64) {
    if let Some(n) = crate::service::contact_presence(w, id, false) {
        w.schedule(Duration::ZERO, n);
    }
}

/// **`schedule`, not `push`, and this is the whole fix.** `pending` is
/// drained by `dispatch.rs` after a payload, and a disconnect is not a
/// payload (`server.rs:402`), so a departure used to wait for *any*
/// connection's next call. The comment below this one used to argue that was
/// fine because the audience's own next call would carry it. It is not fine,
/// and the client is why: `StationSvc.GetGuests` caches the list per station
/// behind `guestListReceived` (`station/base.py:135`), so reopening the lobby
/// makes no call at all. The frame was the only thing that could ever correct
/// the panel, and it was waiting on a turn that may never come -- a ghost in
/// the lobby for up to a tick-less eternity. `schedule` is drained by the
/// world ticker, by the clock, independent of anybody's turn.
fn guest_frame_off(w: &mut Registry, id: i64) {
    if let Some(n) = w.station_guest_frame(id, false) {
        w.schedule(Duration::ZERO, n);
    }
    // And the structure form (U4).
    if let Some(n) = crate::structure_dock::logoff_frame(w, id) {
        w.schedule(Duration::ZERO, n);
    }
}

/// **The ship stays in space.** A pilot who quits undocked leaves their ship
/// where it is -- still a ball in every park, still targetable and killable
/// -- for at least a minute, and for as long as a log-off timer runs ("Ship
/// will remain in space on log-off until timer expires", 283269/283270):
/// logging off is not an escape from a fight. `crate::crimewatch::logged_off`
/// holds it and drops the pilot's own locks; the world clock's
/// `crimewatch::release_holds` takes it out of space later, with the lock
/// teardown and the `RemoveBalls` `session_closed` used to send at once
/// (`crimewatch::leave_space`, which carries their history: both on the
/// clock, because a disconnect is not a payload).
///
/// **And it warps off**, unless the pilot is the aggressor (`logged_off`'s
/// `[A]`): the emergency warp, on the clock for the same reason.
///
/// A pilot not in space has no ship to hold and nothing to remove, which is
/// the docked case and the mid-jump one.
fn logged_off(w: &mut Registry, id: i64) {
    let now = crate::pyutil::filetime_now();
    if crate::crimewatch::logged_off(w, id, now) {
        crate::service::emergency_warp(w, id, now);
    }
}

/// **And out of the fleet.** `Fleets::character_disconnected` was written,
/// documented and unit-tested, and nothing called it, so a pilot who closed
/// the client stayed a member for the life of the process and every fleet
/// push was computed for a character with no socket (`docs/port-tracker.md`
/// §60.3). It is in `ON_LOGOFF`, behind `is_online`, for the reason the
/// logoff stamp is: a second client of the same character is still in the
/// fleet.
///
/// **Pushes on the clock, for the fourth time in this table**, and not
/// through `fleet_apply`, whose `push` waits on a payload turn a disconnect
/// does not have. The session changes (a new boss's `fleetrole`) still go
/// onto `Fleets`' own queue, which only a turn drains: **[A]** acceptable
/// because the members' own `OnFleetLeave` / `OnFleetMemberChanged` arrive
/// by the clock, and the session key catches up on any connection's next
/// call.
fn fleet_leave(w: &mut Registry, id: i64) {
    let effects = w
        .fleets_mut()
        .character_disconnected(id, crate::pyutil::filetime_now());
    for push in effects.pushes {
        w.schedule(Duration::ZERO, push);
    }
    w.fleets_mut().queue_session_changes(effects.sessions);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_table::SessionEntry;

    // Each hook records which row it is and the character it saw, on a
    // thread-local list: tests run in parallel, each on its own thread.
    std::thread_local! {
        static FIRED: std::cell::RefCell<Vec<(u8, i64)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn first(_: &mut Registry, id: i64) {
        FIRED.with(|f| f.borrow_mut().push((1, id)));
    }

    fn second(_: &mut Registry, id: i64) {
        FIRED.with(|f| f.borrow_mut().push((2, id)));
    }

    fn fired() -> Vec<(u8, i64)> {
        FIRED.with(|f| std::mem::take(&mut *f.borrow_mut()))
    }

    /// Two windows of one character: closing the first is not a log-off, so
    /// nothing fires; closing the last fires every row once, in table order;
    /// closing it again (an unknown client) fires nothing.
    #[test]
    fn logoff_runs_each_hook_once_after_the_last_session_only() {
        const WHO: i64 = 90_000_001;
        let table = &[
            Row {
                name: "first",
                run: first,
            },
            Row {
                name: "second",
                run: second,
            },
        ];
        let mut w = Registry::new(crate::model::SandboxConfig::default());
        for client_id in [10_000_000_001, 10_000_000_002] {
            w.session_opened(SessionEntry {
                client_id,
                session_id: client_id,
                user_id: 1,
                character_id: None,
            });
            w.session_selected_character(client_id, WHO);
        }
        fired();

        w.session_closed_with(10_000_000_001, table);
        assert_eq!(fired(), vec![], "a second window is still online");

        w.session_closed_with(10_000_000_002, table);
        assert_eq!(fired(), vec![(1, WHO), (2, WHO)]);

        w.session_closed_with(10_000_000_002, table);
        assert_eq!(fired(), vec![], "that session is already gone");
    }

    /// A row's name is how a report names it, so two rows with one name in a
    /// table would be ambiguous.
    #[test]
    fn row_names_are_unique() {
        for table in [ON_LOGOFF, ON_LOGIN] {
            let mut names: Vec<&str> = table.iter().map(|r| r.name).collect();
            names.sort_unstable();
            let n = names.len();
            names.dedup();
            assert_eq!(names.len(), n, "{names:?}");
        }
    }
}
