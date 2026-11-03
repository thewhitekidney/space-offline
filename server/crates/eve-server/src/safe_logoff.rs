//! **Safe logoff**: Escape > Log Off Character while in space (lane
//! `session-fix2`).
//!
//! In space the client does not call `userSvc.UserLogOffCharacter`. Both the
//! Escape menu and the Log Off command take `menuFunctions.SafeLogoff`:
//!
//! ```text
//! failed = gameui.GetShipAccess().SafeLogoff()      # ship.MachoBindObject(.., ("SafeLogoff",..))
//! if failed: dialog of the labels in `failed`       # an iterable; [] means go
//! ```
//!
//! and then waits for the server to say `OnSafeLogoffTimerStarted(deadline)`
//! (a countdown window, with an Abort button that calls `AbortSafeLogoff`),
//! then `OnSafeLogoffActivated`, on which `gameui` runs `DoCharacterLogoff`
//! itself (or quits the client, if the player ticked "quit"). We answered
//! `SafeLogoff` with `None` -- `UNANSWERED` in the log -- which the client
//! cannot iterate, so nothing happened. **[V]** from the client source and the
//! capture (`ship.MachoBindObject(.., "SafeLogoff")`, then silence).
//!
//! What this module does: `SafeLogoff` checks the conditions, answers `[]`,
//! records a deadline [`DELAY`] out and pushes the timer. [`tick`] ends it:
//! the ship leaves space (a safe logoff leaves no hull behind), the character
//! is logged off server-side, and `OnSafeLogoffActivated` goes out first so the
//! client returns to the character selection. No session change is sent: the
//! client's own `DoCharacterLogoff` resets its session, as it does for the
//! Escape menu's log-off. [A] falsified by the client sitting on the select
//! screen with stale keys, or by a second click on a character failing.
//!
//! The conditions are the two the server can judge today: a running weapons
//! timer and an activated warp (EvEJS refuses more: fleet, locks, modules; the
//! labels are its facts, the text is the client's). Anything that changes
//! them during the countdown ends it with `OnSafeLogoffFailed`.

use crate::damage::BlueTime;
use crate::notify::Push;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::time::Instant;

/// How long the countdown runs: 30 s, as TQ's.
pub const DELAY: BlueTime = 30 * 10_000_000;

/// The client's own labels for why a safe logoff will not start.
const PVP_TIMER: &str = "UI/Inflight/SafeLogoff/SafeLogoffCantHavePvpTimerBody";
const WARPING: &str = "UI/Inflight/SafeLogoff/SafeLogoffShipCantBeInWarpBody";
/// The client's own label for it (`UI/Inflight/SafeLogoff/ConditionCloaked`,
/// "Your ship is cloaked") **[V]** (CLOAK-1).
const CLOAKED: &str = "UI/Inflight/SafeLogoff/ConditionCloaked";

#[derive(Default)]
pub struct State {
    /// `(character, deadline)` of every running countdown.
    due: Vec<(i64, BlueTime)>,
    /// Pilots whose ship has left space and whose `OnSafeLogoffActivated`
    /// is still on the clock's queue: their connection is logged off once
    /// the push has gone (see [`tick`]).
    leaving: Vec<i64>,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "safe_logoff",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

/// `OnSafeLogoffTimerStarted(deadline)`: a FILETIME the countdown reads
/// against `blue.os.GetSimTime()`.
#[derive(Debug)]
struct TimerStarted {
    who: i64,
    deadline: BlueTime,
}

impl Push for TimerStarted {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnSafeLogoffTimerStarted",
            vec![PyValue::Int(self.deadline)],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.who])
    }
}

/// `OnSafeLogoffActivated()`: the client goes to the character selection.
#[derive(Debug)]
struct Activated {
    who: i64,
}

impl Push for Activated {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnSafeLogoffActivated", Vec::new())
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.who])
    }
}

/// `OnSafeLogoffFailed(labels)`: the countdown ends with a dialog.
#[derive(Debug)]
struct Failed {
    who: i64,
    labels: Vec<&'static str>,
}

impl Push for Failed {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let labels = self
            .labels
            .iter()
            .map(|l| PyValue::Unicode((*l).to_owned()));
        ("OnSafeLogoffFailed", vec![PyValue::List(labels.collect())])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.who])
    }
}

/// Why `who` cannot safely log off now; empty when they can.
fn failed_conditions(reg: &Registry, who: i64, now: BlueTime) -> Vec<&'static str> {
    let mut out = Vec::new();
    if crate::crimewatch::has_weapons_timer(reg, who, now) {
        out.push(PVP_TIMER);
    }
    if reg
        .roster()
        .get(who)
        .is_some_and(|p| p.motion.warp_active())
    {
        out.push(WARPING);
    }
    if crate::cloak::character_cloaked(reg, who) {
        out.push(CLOAKED);
    }
    // ESS-2: a pilot linked to an ESS main bank cannot slip away.
    if crate::ess::linked(reg, who) {
        out.push(crate::ess::SAFE_LOGOFF_LINKED);
    }
    out
}

/// `ship.SafeLogoff()`: `[]` when the countdown has begun, else the labels of
/// what stops it. Raises `CustomNotify` outside space.
pub fn start(
    reg: &mut Registry,
    actor: Option<i64>,
    now: BlueTime,
) -> Result<PyValue, crate::service::ServiceError> {
    let in_space = actor.filter(|a| reg.roster().get(*a).is_some());
    let Some(who) = in_space else {
        return Err(crate::service::ServiceError::Raise(eve_proto::user_error(
            "CustomNotify",
            vec![(
                crate::pyutil::bstr("You must be in space to log off safely."),
                crate::pyutil::bstr("notify"),
            )],
        )));
    };
    let failed = failed_conditions(reg, who, now);
    if !failed.is_empty() {
        let labels = failed
            .into_iter()
            .map(|l| PyValue::Unicode(l.to_owned()))
            .collect();
        return Ok(PyValue::List(labels));
    }
    let deadline = now + DELAY;
    let st = reg.ext_mut::<State>();
    st.due.retain(|(c, _)| *c != who);
    st.due.push((who, deadline));
    crate::outln!("[safe-logoff] character {who}: countdown started");
    reg.push(TimerStarted { who, deadline });
    Ok(PyValue::List(Vec::new()))
}

/// `ship.AbortSafeLogoff()`: the player pressed Abort. The client closes its
/// own window.
pub fn abort(reg: &mut Registry, actor: Option<i64>) -> PyValue {
    if let Some(who) = actor {
        reg.ext_mut::<State>().due.retain(|(c, _)| *c != who);
    }
    PyValue::None
}

/// The world clock's stage: fail the countdowns that no longer hold, finish
/// the ones that are up.
/// Is `who`'s `OnSafeLogoffActivated` still waiting on the clock's queue?
fn activation_waiting(reg: &Registry, who: i64) -> bool {
    reg.has_scheduled(|p| {
        p.to_push().0 == "OnSafeLogoffActivated" && p.audience() == Some(vec![who])
    })
}

/// Log off the connections of every pilot whose activation push has left.
///
/// **The push goes by the clock, and it is addressed to the character**: the
/// clock resolves a character to its connections when it drains
/// (`push::clock_slice`). Logging the connection off in the same tick as the
/// push removed the character from the session table before that, and the
/// push was dropped -- the ship gone, the client still in space. The push
/// is a `schedule`d one, so it is out of the queue exactly when it has been
/// resolved, and that is when the log-off runs.
fn finish_leaving(reg: &mut Registry) {
    let leaving = reg.ext::<State>().leaving.clone();
    if leaving.is_empty() {
        return;
    }
    let mut waiting = Vec::new();
    for who in leaving {
        if activation_waiting(reg, who) {
            waiting.push(who);
            continue;
        }
        let clients: Vec<i64> = reg
            .sessions()
            .of_character(who)
            .iter()
            .map(|e| e.client_id)
            .collect();
        for client in clients {
            reg.character_log_off(client);
        }
    }
    reg.ext_mut::<State>().leaving = waiting;
}

pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    finish_leaving(reg);
    let running: Vec<(i64, BlueTime)> = reg.ext::<State>().due.clone();
    if running.is_empty() {
        return;
    }
    let mut keep = Vec::new();
    for (who, deadline) in running {
        // Gone already (docked, destroyed, disconnected): nothing to log off.
        if reg.roster().get(who).is_none() || reg.sessions().of_character(who).is_empty() {
            continue;
        }
        let failed = failed_conditions(reg, who, ft);
        if !failed.is_empty() {
            crate::outln!("[safe-logoff] character {who}: countdown failed");
            reg.push(Failed {
                who,
                labels: failed,
            });
            continue;
        }
        if ft < deadline {
            keep.push((who, deadline));
            continue;
        }
        crate::outln!("[safe-logoff] character {who}: logged off safely");
        // **A pocket is no exit** (ABYSS-LOGOFF): the client has no label for
        // refusing here, so the log-off goes through and the ship stays, held
        // as for a dropped connection; the run's clock still ends it.
        if crate::abyss::in_run(reg, who) {
            reg.schedule(std::time::Duration::ZERO, Activated { who });
            reg.ext_mut::<State>().leaving.push(who);
            continue;
        }
        crate::logoff_spot::record(reg, who);
        // A ship that passed the check while its warp was still aligning has
        // an arrival batch queued: it must not snap the restored ball later.
        if let Some(ship) = reg.character(who).map(|c| c.ship_id) {
            reg.cancel_scheduled(|p| crate::service::space::is_warp_settle(p, ship));
        }
        reg.leave_space(who);
        reg.schedule(std::time::Duration::ZERO, Activated { who });
        reg.ext_mut::<State>().leaving.push(who);
    }
    reg.ext_mut::<State>().due = keep;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_state::{Motion, Presence};

    // Invented ids.
    const ALICE: i64 = 1_000_001;
    const ALICE_SHIP: i64 = 100_000_001;
    const SYSTEM: i64 = 30_000_001;
    const T0: BlueTime = 1_000_000 * DELAY;

    fn world(in_space: bool) -> Registry {
        let mut reg = Registry::new(crate::model::SandboxConfig {
            characters: vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }],
            ..crate::model::SandboxConfig::default()
        });
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: 1,
            character_id: Some(ALICE),
        });
        if in_space {
            reg.roster_mut().arrive(Presence::arriving(
                ALICE,
                ALICE_SHIP,
                SYSTEM,
                Motion::arrived([0.0; 3], [0.0; 3]),
            ));
        }
        reg
    }

    fn pushed(reg: &mut Registry) -> Vec<&'static str> {
        reg.take_pending().iter().map(|p| p.1.to_push().0).collect()
    }

    #[test]
    fn in_space_the_call_answers_an_empty_list_and_starts_the_timer() {
        let mut reg = world(true);
        let _ = reg.take_pending();
        let out = start(&mut reg, Some(ALICE), T0).expect("answers");
        assert_eq!(out, PyValue::List(Vec::new()), "[] means all checks passed");
        let sent = reg.take_pending();
        assert_eq!(sent.len(), 1);
        let (event, args) = sent[0].1.to_push();
        assert_eq!(event, "OnSafeLogoffTimerStarted");
        assert_eq!(args, vec![PyValue::Int(T0 + DELAY)]);
        assert_eq!(sent[0].1.audience(), Some(vec![ALICE]));
    }

    #[test]
    fn docked_it_raises_instead() {
        let mut reg = world(false);
        assert!(matches!(
            start(&mut reg, Some(ALICE), T0),
            Err(crate::service::ServiceError::Raise(_))
        ));
        assert!(reg.ext::<State>().due.is_empty());
    }

    #[test]
    fn a_weapons_timer_answers_the_failed_labels_and_starts_nothing() {
        let mut reg = world(true);
        reg.combat_mut().crimewatch_mut().start(
            ALICE,
            crate::crimewatch::Timer::Weapons,
            T0,
            false,
        );
        let out = start(&mut reg, Some(ALICE), T0).expect("answers");
        assert_eq!(
            out,
            PyValue::List(vec![PyValue::Unicode(PVP_TIMER.to_owned())])
        );
        assert!(reg.ext::<State>().due.is_empty());
    }

    #[test]
    fn the_ship_stays_until_the_deadline_then_the_pilot_is_logged_off() {
        let mut reg = world(true);
        start(&mut reg, Some(ALICE), T0).unwrap();
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), T0 + DELAY - 1);
        assert!(reg.roster().get(ALICE).is_some(), "still in space");
        assert!(pushed(&mut reg).is_empty());
        tick(&mut reg, Instant::now(), T0 + DELAY);
        assert!(reg.roster().get(ALICE).is_none(), "the hull left space");
        assert!(reg.sessions().is_online(ALICE), "the push goes out first");
        assert!(activation_waiting(&reg, ALICE));
        // The next tick finds the push still queued; once the clock has
        // drained it, the one after logs the connection off.
        let _ = reg.take_due(Instant::now() + std::time::Duration::from_secs(1));
        tick(&mut reg, Instant::now(), T0 + DELAY);
        assert!(!reg.sessions().is_online(ALICE), "logged off");
        assert!(reg.ext::<State>().due.is_empty());
        assert!(reg.ext::<State>().leaving.is_empty());
    }

    /// **Through the clock's own delivery**, with another connection online:
    /// the push reaches the pilot's connection, and only then is it logged
    /// off. (`take_pending` cannot show this: the clock resolves the
    /// audience against the session table when it drains.)
    #[test]
    fn the_activation_reaches_the_clients_connection_through_the_clock() {
        let mut reg = world(true);
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 2,
            session_id: 2,
            user_id: 2,
            character_id: None,
        });
        start(&mut reg, Some(ALICE), T0).unwrap();
        let _ = reg.take_pending();
        let late = Instant::now() + std::time::Duration::from_secs(5);
        let mut told = Vec::new();
        for _ in 0..3 {
            let turn = crate::push::clock_turn(&mut reg, late, T0 + DELAY);
            told.extend(
                turn.frames
                    .into_iter()
                    .filter(|f| f.push.to_push().0 == "OnSafeLogoffActivated")
                    .map(|f| f.clients),
            );
        }
        assert_eq!(told, vec![vec![1]], "delivered once, to Alice's connection");
        assert!(!reg.sessions().is_online(ALICE), "then logged off");
    }

    #[test]
    fn safe_logoff_records_a_spot() {
        let mut reg = world(true);
        reg.roster_mut().get_mut(ALICE).unwrap().motion.position = [11.0, 22.0, 33.0];
        start(&mut reg, Some(ALICE), T0).unwrap();
        tick(&mut reg, Instant::now(), T0 + DELAY);
        let spot = crate::logoff_spot::get(&reg, ALICE).expect("a spot is written");
        assert_eq!((spot.system, spot.position), (SYSTEM, [11.0, 22.0, 33.0]));
    }

    #[test]
    fn abort_cancels_the_countdown() {
        let mut reg = world(true);
        start(&mut reg, Some(ALICE), T0).unwrap();
        assert_eq!(abort(&mut reg, Some(ALICE)), PyValue::None);
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), T0 + 2 * DELAY);
        assert!(reg.roster().get(ALICE).is_some());
        assert!(pushed(&mut reg).is_empty());
    }

    #[test]
    fn a_timer_that_starts_mid_countdown_fails_it() {
        let mut reg = world(true);
        start(&mut reg, Some(ALICE), T0).unwrap();
        let _ = reg.take_pending();
        reg.combat_mut().crimewatch_mut().start(
            ALICE,
            crate::crimewatch::Timer::Weapons,
            T0 + 1,
            false,
        );
        tick(&mut reg, Instant::now(), T0 + 2);
        assert_eq!(pushed(&mut reg), vec!["OnSafeLogoffFailed"]);
        assert!(reg.roster().get(ALICE).is_some(), "the ship stays");
        assert!(reg.ext::<State>().due.is_empty());
    }

    /// Safe logoff refuses an active warp, not an align.
    #[test]
    fn only_an_active_warp_blocks_a_safe_logoff() {
        let warp = |effect_stamp: i32| crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        let mut reg = world(true);
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = warp(crate::destiny::tick() as i32 + 3_600);
        assert!(failed_conditions(&reg, ALICE, T0).is_empty());
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = warp(crate::destiny::tick() as i32 - 5);
        assert_eq!(failed_conditions(&reg, ALICE, T0), vec![WARPING]);
    }
}
