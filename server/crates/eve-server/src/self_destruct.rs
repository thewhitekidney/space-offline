//! **Self-destruct of a ship and of a capsule** (GAP-SELFDESTRUCT, from
//! EVEJS-GAP svcD §3.3).
//!
//! The client calls `ship.SelfDestruct(shipID)` and `ship.AbortSelfDestruct(
//! shipID)` through `PerformSessionChange` and drops the answer
//! (`menuFunctions.py:520-548`) [V]. What it reads back is the **slim**: the
//! menu entries test `slimItem.selfDestructTime is not None`
//! (`celestialCheckers.py:1957`, `OfferSelfDestructShip`,
//! `OfferAbortSelfDestructShip`) [V], and the abyss, filament and void
//! validators read the same field. The words come as
//! `OnRemoteMessage(key, dict)` (`gameui.py:334-344`) [V].
//!
//! The timer is 2 minutes, for a hull and for a capsule (the confirm text
//! says so: `ConfirmSelfDestruct`, `ConfirmSelfDestructCapsule`) [V]. The
//! state is not persisted: a restart takes ships out of space anyway.
//!
//! **Messages**, with the dict keys the client's text reads [V]:
//!
//! | to | key | dict |
//! |---|---|---|
//! | the pilot | `SelfDestructInitiated` | `what`, `when` (seconds) |
//! | the pilot | `SelfDestructAborted2` | `timeLeft` (**minutes**: `from=minute`) |
//! | the pilot | `SelfDestructImmediate` | `shipTypeID` |
//! | the pilot | `SelfDestructCancelledWarp`, `...External` | `what` |
//! | the others on the grid | `SelfDestructInitiatedOther` | `charID`, `shipTypeID`, `timeRemaining` (seconds) |
//! | the others on the grid | `SelfDestructAbortedOther2` | `charID`, `timeLeft` (minutes) |
//! | the others on the grid | `SelfDestructImmediateOther` | `charID`, `shipTypeID` |
//!
//! EvEJS sends the "...Other" ones to the pilot with an empty dict, which the
//! client's text cannot format; here they go to the others, filled.
//!
//! **[A]**, each with its falsifier:
//! - `selfDestructTime` is the FILETIME the ship blows up at (any non-`None`
//!   value satisfies the menus); falsified by a client that reads it as a
//!   duration or a countdown.
//! - `what` is the English word "ship" or "capsule", the answer of the
//!   `{what}` slot; falsified by the text showing a type name where a word
//!   was meant.
//! - `timeLeft` is minutes, as a float; falsified by the abort line reading
//!   "1.5 seconds".
//! - A second `SelfDestruct` while one runs does nothing (no refusal, no
//!   second message); falsified by the client showing a dialog for it.
//! - A start in warp is refused with `SelfDestructCancelledWarp`, not a
//!   raise; the ship leaving the grid (dock, jump, a boarded ship) cancels
//!   with `SelfDestructCancelledExternal`.
//! - A start does not abort a running safe logoff here (`safe_logoff` is not
//!   in this lane's files): follow-up, with `ConditionSelfDestruct` as the
//!   label.
//!
//! **Insurance** is an open owner question: our insurance pays nothing for a
//! self kill (`insurance::voided`), and `docs/archive/plan-living-world.md`
//! decision 7 assumes a self-destruct can be paid. [`SELF_DESTRUCT_PAYS_INSURANCE`]
//! is the one switch; it keeps today's behaviour until the owner decides.

use crate::damage::BlueTime;
use crate::kill_hooks::KillEvent;
use crate::notify::Push;
use crate::service::{Registry, ServiceError};
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::time::Instant;

/// The sequence runs 2 minutes, FILETIME ticks of 100 ns [V: the confirm text].
pub const DURATION: BlueTime = 120 * 10_000_000;

/// **Whether a self-destructed, insured ship pays its contract.** `false`
/// is today's behaviour (`insurance::voided`: the owner killing their own
/// ship pays nothing). **[A]** an open owner question -- the living-world
/// plan's decision 7 assumes a self-destruct can be paid; nobody has decided.
/// Flip this one const to pay; nothing else changes. Falsifier: the owner's
/// answer, or a retail payout on a self-destruct.
pub const SELF_DESTRUCT_PAYS_INSURANCE: bool = false;

const ONE_SECOND: BlueTime = 10_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    ship_id: i64,
    character: i64,
    system: i64,
    fires_at: BlueTime,
}

#[derive(Default)]
pub struct State {
    runs: Vec<Run>,
    /// The ship whose self-destruct kill is running right now, so the kill
    /// hooks (insurance) can tell it from a shot.
    killing: Option<i64>,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "self_destruct",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

/// `OnRemoteMessage(key, dict)`.
#[derive(Debug)]
struct Remote {
    to: Vec<i64>,
    key: &'static str,
    dict: Vec<(&'static str, PyValue)>,
}

impl Push for Remote {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            "OnRemoteMessage",
            vec![
                crate::pyutil::bstr(self.key),
                crate::pyutil::pydict(self.dict.clone()),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(self.to.clone())
    }
}

fn tell(reg: &mut Registry, to: Vec<i64>, key: &'static str, dict: Vec<(&'static str, PyValue)>) {
    if !to.is_empty() {
        reg.push(Remote { to, key, dict });
    }
}

fn refusal() -> ServiceError {
    ServiceError::Raise(eve_proto::user_error("CannotSelfDestruct", Vec::new()))
}

fn what(type_id: i64) -> PyValue {
    let word = if type_id == crate::damage::CAPSULE_TYPE_ID {
        "capsule"
    } else {
        "ship"
    };
    crate::pyutil::bstr(word)
}

/// The other pilots whose park holds the ship (unmodelled parks are
/// unfiltered, as in `grid`).
fn others_on_grid(reg: &Registry, run: &Run) -> Vec<i64> {
    let parks = reg.ext::<crate::grid::Parks>();
    reg.roster()
        .others_in_system(run.system, run.character)
        .into_iter()
        .filter(|id| reg.character(*id).is_some())
        .filter(|id| parks.of(*id).is_none_or(|p| p.contains(&run.ship_id)))
        .collect()
}

fn ship_type(reg: &Registry, character: i64) -> i64 {
    reg.character(character).map_or(0, |c| c.ship_type_id)
}

/// Whether `ship_id` is counting down: what the slim's `selfDestructTime`
/// says.
pub fn is_running(reg: &Registry, ship_id: i64) -> bool {
    reg.ext::<State>().runs.iter().any(|r| r.ship_id == ship_id)
}

/// `ship.SelfDestruct(shipID)`. Raises `CannotSelfDestruct` for a ship that
/// is not the pilot's own active one in space; answers `None`.
pub fn start(
    reg: &mut Registry,
    actor: Option<i64>,
    args: &[PyValue],
    now: BlueTime,
) -> Result<PyValue, ServiceError> {
    let ship_id = match args.first() {
        Some(PyValue::Int(n)) => Some(*n),
        _ => None,
    };
    let pilot = actor
        .and_then(|a| reg.roster().get(a).copied())
        .filter(|p| Some(p.ship_id) == ship_id)
        .ok_or_else(refusal)?;
    let who = pilot.character_id;
    let type_id = ship_type(reg, who);
    if reg
        .ext::<State>()
        .runs
        .iter()
        .any(|r| r.ship_id == pilot.ship_id)
    {
        return Ok(PyValue::None);
    }
    if pilot.motion.warp_active() {
        tell(
            reg,
            vec![who],
            "SelfDestructCancelledWarp",
            vec![("what", what(type_id))],
        );
        return Ok(PyValue::None);
    }
    let run = Run {
        ship_id: pilot.ship_id,
        character: who,
        system: pilot.solar_system_id,
        fires_at: now + DURATION,
    };
    reg.ext_mut::<State>().runs.push(run);
    crate::outln!(
        "[self-destruct] character {who}: ship {} counting down",
        run.ship_id
    );
    tell(
        reg,
        vec![who],
        "SelfDestructInitiated",
        vec![
            ("what", what(type_id)),
            ("when", PyValue::Int(DURATION / ONE_SECOND)),
        ],
    );
    let others = others_on_grid(reg, &run);
    tell(
        reg,
        others,
        "SelfDestructInitiatedOther",
        vec![
            ("charID", PyValue::Int(who)),
            ("shipTypeID", PyValue::Int(type_id)),
            ("timeRemaining", PyValue::Int(DURATION / ONE_SECOND)),
        ],
    );
    refresh_slim(reg, who);
    Ok(PyValue::None)
}

/// `ship.AbortSelfDestruct(shipID)`: drop the run, clear the slim field and
/// say how long was left. Nothing running: nothing happens.
pub fn abort(reg: &mut Registry, actor: Option<i64>, now: BlueTime) -> PyValue {
    let Some(who) = actor else {
        return PyValue::None;
    };
    let Some(i) = reg
        .ext::<State>()
        .runs
        .iter()
        .position(|r| r.character == who)
    else {
        return PyValue::None;
    };
    let run = reg.ext_mut::<State>().runs.remove(i);
    let minutes = (run.fires_at - now).max(0) as f64 / (60.0 * ONE_SECOND as f64);
    tell(
        reg,
        vec![who],
        "SelfDestructAborted2",
        vec![("timeLeft", PyValue::Real(minutes))],
    );
    let others = others_on_grid(reg, &run);
    tell(
        reg,
        others,
        "SelfDestructAbortedOther2",
        vec![
            ("charID", PyValue::Int(who)),
            ("timeLeft", PyValue::Real(minutes)),
        ],
    );
    refresh_slim(reg, who);
    PyValue::None
}

/// The world clock's stage: cancel the runs that no longer hold (warp, dock,
/// jump, a different ship), blow up the ones that are due.
pub fn tick(reg: &mut Registry, _now: Instant, ft: BlueTime) {
    if reg.ext::<State>().runs.is_empty() {
        return;
    }
    let runs = std::mem::take(&mut reg.ext_mut::<State>().runs);
    let mut keep = Vec::new();
    let mut due = Vec::new();
    for run in runs {
        let here = reg
            .roster()
            .get(run.character)
            .copied()
            .filter(|p| p.ship_id == run.ship_id && p.solar_system_id == run.system);
        let Some(p) = here else {
            // The ship died of something else: nothing to cancel. Otherwise
            // it left the grid (docked, jumped, switched ship).
            if reg.store().get(run.ship_id).is_some() {
                cancel(reg, &run, "SelfDestructCancelledExternal");
            }
            continue;
        };
        if p.motion.warp_active() {
            cancel(reg, &run, "SelfDestructCancelledWarp");
        } else if ft >= run.fires_at {
            due.push(run);
        } else {
            keep.push(run);
        }
    }
    reg.ext_mut::<State>().runs = keep;
    for run in due {
        detonate(reg, &run, ft);
    }
}

fn cancel(reg: &mut Registry, run: &Run, key: &'static str) {
    let type_id = ship_type(reg, run.character);
    crate::outln!(
        "[self-destruct] character {}: cancelled ({key})",
        run.character
    );
    tell(reg, vec![run.character], key, vec![("what", what(type_id))]);
    // The slim field is gone with the run; a ship still in space says so.
    refresh_slim(reg, run.character);
}

fn detonate(reg: &mut Registry, run: &Run, ft: BlueTime) {
    let type_id = ship_type(reg, run.character);
    crate::outln!(
        "[self-destruct] character {}: ship {} blows up",
        run.character,
        run.ship_id
    );
    tell(
        reg,
        vec![run.character],
        "SelfDestructImmediate",
        vec![("shipTypeID", PyValue::Int(type_id))],
    );
    let others = others_on_grid(reg, run);
    tell(
        reg,
        others,
        "SelfDestructImmediateOther",
        vec![
            ("charID", PyValue::Int(run.character)),
            ("shipTypeID", PyValue::Int(type_id)),
        ],
    );
    reg.ext_mut::<State>().killing = Some(run.ship_id);
    crate::combat::self_destruct_kill(reg, run.character, ft);
    reg.ext_mut::<State>().killing = None;
}

/// **The `slim_extra::SOURCES` row**: `selfDestructTime` on a ship that is
/// counting down.
pub fn slim_extras(w: &Registry, ids: &[i64]) -> crate::slim_extra::SlimExtras {
    let runs = &w.ext::<State>().runs;
    ids.iter()
        .filter_map(|id| {
            let r = runs.iter().find(|r| r.ship_id == *id)?;
            Some((*id, vec![("selfDestructTime", PyValue::Int(r.fires_at))]))
        })
        .collect()
}

/// Whether the insurance of a ship that `ev` killed is paid although the
/// owner killed it: only a self-destruct, and only with
/// [`SELF_DESTRUCT_PAYS_INSURANCE`]. Called by `insurance::on_kill`.
pub fn pays_insurance(w: &Registry, ev: &KillEvent) -> bool {
    pays_with(switch(), w, ev)
}

fn pays_with(on: bool, w: &Registry, ev: &KillEvent) -> bool {
    on && ev.killer == Some(ev.victim_character) && w.ext::<State>().killing == Some(ev.victim_ship)
}

#[cfg(not(test))]
fn switch() -> bool {
    SELF_DESTRUCT_PAYS_INSURANCE
}

#[cfg(test)]
thread_local! {
    static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// The tests exercise both values of the switch without editing the const.
#[cfg(test)]
fn switch() -> bool {
    FORCE
        .with(|f| f.get())
        .unwrap_or(SELF_DESTRUCT_PAYS_INSURANCE)
}

/// A fresh `OnSlimItemChange` for the pilot's ship to everyone in its system
/// (`UpdateSlimItemField`), as `stance::refresh_slim` sends one. A pilot
/// docked has nothing to refresh: the next bootstrap carries it.
pub(crate) fn refresh_slim(w: &mut Registry, character: i64) {
    let Some(p) = w.roster().get(character).copied() else {
        return;
    };
    let Some(c) = w.character(character) else {
        return;
    };
    let (type_id, corp) = (c.ship_type_id, c.corporation_id);
    let to = crate::dungeon::audience(w, p.solar_system_id);
    if to.is_empty() {
        return;
    }
    let mut pairs: Vec<(&'static str, PyValue)> = vec![
        ("itemID", PyValue::Int(p.ship_id)),
        ("typeID", PyValue::Int(type_id)),
        ("ownerID", PyValue::Int(character)),
        ("charID", PyValue::Int(character)),
        ("corpID", PyValue::Int(corp)),
    ];
    let fitted = crate::modules::slim_modules(w.store(), p.ship_id);
    if !fitted.is_empty() {
        pairs.push((
            "modules",
            PyValue::List(
                fitted
                    .iter()
                    .map(|m| {
                        PyValue::Tuple(vec![
                            PyValue::Int(m.item_id),
                            PyValue::Int(m.type_id),
                            PyValue::Int(m.flag_id),
                        ])
                    })
                    .collect(),
            ),
        ));
    }
    for (id, extra) in crate::slim_extra::for_slims(w, &[p.ship_id]) {
        if id == p.ship_id {
            pairs.extend(extra);
        }
    }
    let stamp = w.next_destiny_stamp();
    w.schedule(
        std::time::Duration::ZERO,
        crate::sec_status::ShipSlimChange {
            to,
            stamp,
            item_id: p.ship_id,
            pairs,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::tests::{
        ALICE, ALICE_POD, ALICE_SHIP, BOB, BOB_SHIP, alice_has_bob_locked, due,
    };

    const T0: BlueTime = 1_000_000 * DURATION;
    const RIFTER: i64 = 587;

    fn world() -> Registry {
        let mut reg = alice_has_bob_locked();
        let _ = reg.take_pending();
        reg
    }

    /// Every `OnRemoteMessage` waiting, as `(audience, key, dict)`.
    fn remote(reg: &mut Registry) -> Vec<(Vec<i64>, String, PyValue)> {
        let mut out = Vec::new();
        for (_, p) in reg.take_pending() {
            let (name, args) = p.to_push();
            if name == "OnRemoteMessage" {
                let PyValue::Bytes(k) = &args[0] else {
                    panic!("a key")
                };
                out.push((
                    p.audience().unwrap_or_default(),
                    String::from_utf8_lossy(k).into_owned(),
                    args[1].clone(),
                ));
            }
        }
        out
    }

    fn field(d: &PyValue, k: &str) -> Option<PyValue> {
        crate::pyutil::dict_get(d, k.as_bytes()).cloned()
    }

    fn start_alice(reg: &mut Registry, now: BlueTime) {
        start(reg, Some(ALICE), &[PyValue::Int(ALICE_SHIP)], now).expect("starts");
    }

    fn slim_field(reg: &Registry, ship: i64) -> Option<PyValue> {
        crate::slim_extra::for_slims(reg, &[ship])
            .into_iter()
            .flat_map(|(_, e)| e)
            .find(|(k, _)| *k == "selfDestructTime")
            .map(|(_, v)| v)
    }

    #[test]
    fn start_sets_the_slim_field_the_record_and_both_messages() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        assert!(is_running(&reg, ALICE_SHIP));
        assert_eq!(
            slim_field(&reg, ALICE_SHIP),
            Some(PyValue::Int(T0 + DURATION))
        );
        assert_eq!(slim_field(&reg, BOB_SHIP), None, "only the one ship");
        let msgs = remote(&mut reg);
        let mine = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructInitiated")
            .expect("the pilot's");
        assert_eq!(mine.0, vec![ALICE]);
        assert_eq!(field(&mine.2, "when"), Some(PyValue::Int(120)));
        assert_eq!(field(&mine.2, "what"), Some(crate::pyutil::bstr("ship")));
        let other = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructInitiatedOther")
            .expect("the other pilot's");
        assert_eq!(other.0, vec![BOB], "not the pilot themself");
        assert_eq!(field(&other.2, "charID"), Some(PyValue::Int(ALICE)));
        assert_eq!(field(&other.2, "shipTypeID"), Some(PyValue::Int(RIFTER)));
        assert_eq!(field(&other.2, "timeRemaining"), Some(PyValue::Int(120)));
        // The slim change reached the system.
        let out = due(&mut reg);
        assert!(
            out.iter()
                .any(|(to, n, _)| *n == "DoDestinyUpdate" && to.contains(&BOB)),
            "OnSlimItemChange to the system"
        );
    }

    #[test]
    fn a_second_start_does_nothing() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        start_alice(&mut reg, T0 + 5 * ONE_SECOND);
        assert_eq!(reg.ext::<State>().runs.len(), 1);
        assert_eq!(
            reg.ext::<State>().runs[0].fires_at,
            T0 + DURATION,
            "not reset"
        );
        assert!(remote(&mut reg).is_empty());
    }

    #[test]
    fn a_ship_that_is_not_yours_gets_cannot_self_destruct() {
        let mut reg = world();
        let raised = |r: Result<PyValue, ServiceError>| matches!(r, Err(ServiceError::Raise(_)));
        assert!(raised(start(
            &mut reg,
            Some(ALICE),
            &[PyValue::Int(BOB_SHIP)],
            T0
        )));
        assert!(raised(start(&mut reg, Some(ALICE), &[], T0)));
        assert!(raised(start(
            &mut reg,
            None,
            &[PyValue::Int(ALICE_SHIP)],
            T0
        )));
        assert!(reg.ext::<State>().runs.is_empty());
    }

    #[test]
    fn abort_clears_both_and_tells_the_pilot_and_the_grid() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        // 30 s in: 90 s, 1.5 minutes, are left.
        assert_eq!(
            abort(&mut reg, Some(ALICE), T0 + 30 * ONE_SECOND),
            PyValue::None
        );
        assert!(!is_running(&reg, ALICE_SHIP));
        assert_eq!(slim_field(&reg, ALICE_SHIP), None);
        let msgs = remote(&mut reg);
        let mine = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructAborted2")
            .expect("pilot");
        assert_eq!(mine.0, vec![ALICE]);
        assert_eq!(field(&mine.2, "timeLeft"), Some(PyValue::Real(1.5)));
        let other = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructAbortedOther2")
            .expect("other");
        assert_eq!(other.0, vec![BOB]);
        assert_eq!(field(&other.2, "charID"), Some(PyValue::Int(ALICE)));
        assert_eq!(field(&other.2, "timeLeft"), Some(PyValue::Real(1.5)));
        // And it stays cancelled.
        tick(&mut reg, Instant::now(), T0 + 2 * DURATION);
        assert!(reg.roster().by_ship(ALICE_SHIP).is_some());
    }

    #[test]
    fn at_the_deadline_the_ship_blows_up_through_the_kill_path() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), T0 + DURATION - 1);
        assert!(reg.roster().by_ship(ALICE_SHIP).is_some(), "not yet");
        let ft = crate::pyutil::filetime_now().max(T0 + DURATION);
        reg.ext_mut::<State>().runs[0].fires_at = ft;
        tick(&mut reg, Instant::now(), ft);
        let msgs = remote(&mut reg);
        let imm = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructImmediate")
            .expect("pilot");
        assert_eq!(imm.0, vec![ALICE]);
        assert_eq!(field(&imm.2, "shipTypeID"), Some(PyValue::Int(RIFTER)));
        let other = msgs
            .iter()
            .find(|m| m.1 == "SelfDestructImmediateOther")
            .expect("other");
        assert_eq!(other.0, vec![BOB]);
        // Dead hull, a wreck, the pilot in the capsule.
        assert!(reg.roster().by_ship(ALICE_SHIP).is_none());
        assert!(reg.combat().wrecks().count() >= 1, "a wreck");
        let c = reg.character(ALICE).expect("Alice");
        assert_eq!(
            (c.ship_id, c.ship_type_id),
            (ALICE_POD, crate::damage::CAPSULE_TYPE_ID)
        );
        // The killmail names the victim as the final blow.
        let report = crate::killmail::involving(&reg, ALICE, None, 1)
            .into_iter()
            .next()
            .expect("a killmail");
        assert_eq!(report.victim, ALICE);
        assert_eq!(report.final_blow.character, Some(ALICE));
        assert!(reg.ext::<State>().runs.is_empty());
        assert_eq!(reg.ext::<State>().killing, None);
    }

    #[test]
    fn a_capsule_self_destruct_revives_in_the_clone() {
        let mut reg = world();
        // Put Alice in her capsule first.
        crate::combat::self_destruct_kill(&mut reg, ALICE, crate::pyutil::filetime_now());
        let _ = reg.take_pending();
        let _ = reg.take_session_changes();
        let pod = reg.character(ALICE).expect("Alice").ship_id;
        assert_eq!(pod, ALICE_POD);
        let wrecks = reg.combat().wrecks().count();
        let now = crate::pyutil::filetime_now();
        start(&mut reg, Some(ALICE), &[PyValue::Int(pod)], now).expect("starts");
        let what_said = remote(&mut reg);
        let said = what_said
            .iter()
            .find(|m| m.1 == "SelfDestructInitiated")
            .expect("said");
        assert_eq!(field(&said.2, "what"), Some(crate::pyutil::bstr("capsule")));
        reg.ext_mut::<State>().runs[0].fires_at = now;
        tick(&mut reg, Instant::now(), now);
        assert!(reg.roster().get(ALICE).is_none(), "out of space");
        assert_eq!(
            reg.combat().wrecks().count(),
            wrecks,
            "a capsule leaves no wreck"
        );
        assert!(
            reg.character(ALICE).expect("Alice").station_id.is_some(),
            "revived docked"
        );
    }

    #[test]
    fn a_warp_cancels_it() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 - 5,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        tick(&mut reg, Instant::now(), T0 + ONE_SECOND);
        assert!(!is_running(&reg, ALICE_SHIP));
        assert_eq!(slim_field(&reg, ALICE_SHIP), None);
        let msgs = remote(&mut reg);
        assert!(
            msgs.iter()
                .any(|m| m.1 == "SelfDestructCancelledWarp" && m.0 == vec![ALICE])
        );
    }

    #[test]
    fn a_warping_ship_cannot_start() {
        let mut reg = world();
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 - 5,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        start_alice(&mut reg, T0);
        assert!(!is_running(&reg, ALICE_SHIP));
        let msgs = remote(&mut reg);
        assert!(msgs.iter().any(|m| m.1 == "SelfDestructCancelledWarp"));
    }

    #[test]
    fn leaving_space_cancels_it_with_the_external_message() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        reg.leave_space(ALICE);
        tick(&mut reg, Instant::now(), T0 + ONE_SECOND);
        assert!(reg.ext::<State>().runs.is_empty());
        let msgs = remote(&mut reg);
        assert!(
            msgs.iter()
                .any(|m| m.1 == "SelfDestructCancelledExternal" && m.0 == vec![ALICE])
        );
    }

    #[test]
    fn dying_to_a_shot_cancels_it_quietly() {
        let mut reg = world();
        start_alice(&mut reg, T0);
        let _ = reg.take_pending();
        crate::combat::self_destruct_kill(&mut reg, ALICE, T0 + ONE_SECOND);
        let _ = reg.take_pending();
        tick(&mut reg, Instant::now(), T0 + 2 * ONE_SECOND);
        assert!(reg.ext::<State>().runs.is_empty());
        assert!(
            remote(&mut reg).is_empty(),
            "no cancel notice for a dead ship"
        );
    }

    // ---- the insurance switch ---------------------------------------------

    /// Alice's ship insured at 100 %, in a world that prices the Rifter.
    fn insured_world() -> (Registry, f64) {
        let mut content = crate::content::Content::fixture();
        let prices = crate::econ_prices::Table::parse(&[format!(
            "{RIFTER}\t100000\trecipe\t50000\t6\t25\t1\t1"
        )])
        .expect("parses");
        content.insert("econ_prices", prices);
        let mut reg = crate::combat::tests::locked_on(&[], Some(std::sync::Arc::new(content)));
        let full = crate::service::insurance::full_price(&reg, RIFTER);
        assert!(full > 0.0, "setup: the hull has a price");
        reg.deltas_mut().put(
            crate::service::insurance::KIND,
            vec![ALICE_SHIP.to_string()],
            vec![
                ALICE.to_string(),
                RIFTER.to_string(),
                "1".to_owned(),
                "0".to_owned(),
                i64::MAX.to_string(),
            ],
        );
        let _ = reg.take_pending();
        (reg, full)
    }

    fn blow_up_alice(reg: &mut Registry) {
        start_alice(reg, T0);
        let ft = crate::pyutil::filetime_now();
        reg.ext_mut::<State>().runs[0].fires_at = ft;
        tick(reg, Instant::now(), ft);
        assert!(
            reg.roster().by_ship(ALICE_SHIP).is_none(),
            "setup: it blew up"
        );
    }

    #[test]
    fn the_switch_off_pays_nothing_for_a_self_destruct() {
        FORCE.with(|f| f.set(Some(false)));
        let (mut reg, _) = insured_world();
        let before = reg.character(ALICE).expect("Alice").balance;
        blow_up_alice(&mut reg);
        FORCE.with(|f| f.set(None));
        assert_eq!(reg.character(ALICE).expect("Alice").balance, before);
        assert!(
            reg.deltas().of(crate::service::insurance::KIND).is_empty(),
            "the contract ended"
        );
    }

    #[test]
    fn the_switch_on_pays_the_contract_for_a_self_destruct() {
        FORCE.with(|f| f.set(Some(true)));
        let (mut reg, full) = insured_world();
        let before = reg.character(ALICE).expect("Alice").balance;
        blow_up_alice(&mut reg);
        FORCE.with(|f| f.set(None));
        assert_eq!(reg.character(ALICE).expect("Alice").balance, before + full);
        assert!(reg.deltas().of(crate::service::insurance::KIND).is_empty());
    }

    #[test]
    fn the_const_keeps_todays_behaviour() {
        const { assert!(!SELF_DESTRUCT_PAYS_INSURANCE) };
    }
}
