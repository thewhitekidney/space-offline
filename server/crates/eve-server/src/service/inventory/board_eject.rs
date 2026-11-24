//! **`ship.Eject` and `ship.Board` in space** (lane `be-2`, issue #77; spec
//! `handback/r-board-eject/report.md` §2-§3, state in [`crate::pilotless`]).
//!
//! **Eject** (`menuFunctions.Eject` -> `ship.Eject()`, no arguments, reply
//! discarded [V]): the hull is left where it is, with its modules, cargo and
//! drones, and becomes an abandoned ship. The pilot is in the capsule, as a
//! death leaves them (`{'shipid': (old, pod)}`, the capsule's `AddBalls2`).
//!
//! **Board** (`ship.Board(shipID, oldShipID)`, reply unpacked as `shipState`
//! [V]): the abandoned hull becomes the pilot's. If it was not theirs, the
//! hull and everything in it change owner **[A: EvEJS only; falsifier: a
//! retail capture]**. The ship the pilot leaves is abandoned in turn; if it
//! was the capsule, its ball is removed one second after the session change
//! ([`POD_REMOVE_DELAY`]) so the client's ego has moved first.
//!
//! The store: a hull in space is at `(system, flag 0)`; a flown ship is at
//! its pilot's station hangar, the old convention (`dock_at` and
//! `restore_active_ships` rely on it). The capsule is parked at the pilot's
//! hangar on an eject, as a death does (`combat::death_plan`).
//!
//! Every refusal key is from the client's dialog table [V]; every raise site
//! is ours **[A]** (none is in the client). Nothing is changed or pushed
//! before the last refusal has passed.

use super::*;
use crate::destiny::{Action, BallsAdded, BallsRemoved, HandoffEntry, Park, ShipHandoff};
use crate::item::{Edit, Slot};
use crate::pilotless::{self, Abandoned, BOARD_RANGE, POD_REMOVE_DELAY};
use crate::space_state::{Motion, Presence};

/// `ShipEjectFailedSystemsOccupied` (a module holds the ship): no module
/// qualifies on this build (no siege, bastion or entosis rules), so it is a
/// documented constant and never raised.
#[allow(dead_code)]
const SYSTEMS_OCCUPIED: &str = "ShipEjectFailedSystemsOccupied";

fn is_capsule(item: &crate::item::Item) -> bool {
    item.group_id == crate::fitting::GROUP_CAPSULE
}

/// The refusals common to leaving a ship in space, in the spec's order: a
/// weapons timer, then a criminal flag (suspects included **[A]**).
fn refuse_leaving(reg: &Registry, actor: i64) -> Result<(), ServiceError> {
    let now = crate::pyutil::filetime_now();
    if crate::crimewatch::has_weapons_timer(reg, actor, now) {
        return Err(bare_user_error("CannotEjectActiveWeaponsTimer"));
    }
    if crate::crimewatch::flag_of(reg, actor, now).is_some() {
        return Err(bare_user_error("CannotEjectInvolvedInCrime"));
    }
    Ok(())
}

/// The flight numbers of `ship` as the client's ball must hold them after the
/// pilot changed: `SetBallAgility`, `SetBallMass`, `SetMaxSpeed`
/// (`modules::motion_changed`'s three). Empty without a sheet.
fn flight_entries(reg: &Registry, ship: i64) -> Vec<HandoffEntry> {
    let Some(h) = crate::dogma::hull(reg, ship) else {
        return Vec::new();
    };
    vec![
        HandoffEntry::Act(Action::SetBallAgility {
            ball: ship,
            agility: f64::from(h.agility),
        }),
        HandoffEntry::Act(Action::SetBallMass {
            ball: ship,
            mass: h.mass,
        }),
        HandoffEntry::Act(Action::SetMaxSpeed {
            ball: ship,
            max: f64::from(h.max_velocity),
        }),
    ]
}

/// `OnSlimItemChange` for `slim`, with the ship's fitted modules and slim
/// extras: the full attribute set, never the five-field refresh.
fn slim_entry(reg: &Registry, slim: crate::destiny::Slim) -> HandoffEntry {
    HandoffEntry::Slim {
        slim,
        modules: crate::modules::slim_modules(reg.store(), slim.item_id),
        extras: crate::slim_extra::for_slims(reg, &[slim.item_id]),
    }
}

/// What a ship's ball becomes when its pilot leaves: not interactive, **not
/// massive** (the server collides only piloted ships, so the hull the
/// observers hold massive must stop being), stopped, a slim with no `charID`,
/// and its unskilled flight numbers.
fn abandoned_entries(reg: &Registry, a: &Abandoned) -> Vec<HandoffEntry> {
    let mut out = vec![
        HandoffEntry::Act(Action::SetBallInteractive {
            ball: a.ship_id,
            interactive: false,
        }),
        HandoffEntry::Act(Action::SetBallMassive {
            ball: a.ship_id,
            massive: false,
        }),
        HandoffEntry::Act(Action::Stop { ball: a.ship_id }),
        slim_entry(reg, pilotless::slim(a)),
    ];
    out.extend(flight_entries(reg, a.ship_id));
    out
}

/// Send one batch to the park of `system`.
fn send_handoff(reg: &mut Registry, system: i64, entries: Vec<HandoffEntry>) {
    let to = crate::dungeon::audience(reg, system);
    let stamp = reg.next_destiny_stamp();
    if let Some(h) = ShipHandoff::new(to, stamp, entries) {
        reg.push(h);
    }
}

/// `ship.Eject()`.
pub(super) fn eject(reg: &mut Registry, req: &Request<'_>) -> Result<PyValue, ServiceError> {
    let Some(actor) = req.actor else {
        return Err(custom_notify("No character is selected."));
    };
    eject_actor(reg, actor)
}

/// [`eject`] for `actor`, from the server's side as well as the client's (the
/// AIR tutorial puts a pilot who logs back in to scene 1.1 in the capsule).
pub(super) fn eject_actor(reg: &mut Registry, actor: i64) -> Result<PyValue, ServiceError> {
    let Some(c) = reg.character(actor).cloned() else {
        return Err(custom_notify("No character is selected."));
    };
    let actor = c.character_id;
    // ---- refusals, nothing changed ----
    if reg.docked_station(actor).is_some() {
        return Err(bare_user_error("NoEjectingToSpaceInStation"));
    }
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(custom_notify("You are not in space."));
    };
    let Some(hull) = reg.item(c.ship_id).cloned() else {
        return Err(custom_notify("This server does not know your ship."));
    };
    if is_capsule(&hull) {
        return Err(bare_user_error("CannotEjectFromCapsule"));
    }
    if here.motion.warp_active() {
        return Err(bare_user_error("ShipInWarp"));
    }
    refuse_leaving(reg, actor)?;
    let Some(pod) = crate::fitting::capsule_of(reg.store(), &c)
        .filter(|p| p.location_id == hull.item_id)
        .cloned()
    else {
        return Err(custom_notify("Your capsule could not be found."));
    };

    // ---- the store, in one plan ----
    let system = here.solar_system_id;
    let pod_to = match c.station_id {
        Some(station) => Slot::new(actor, station, crate::model::FLAG_HANGAR),
        None => Slot::new(actor, system, crate::combat::FLAG_IN_SPACE),
    };
    reg.apply_items(&[
        Edit::Move {
            item_id: hull.item_id,
            to: Slot::new(hull.owner_id, system, crate::combat::FLAG_IN_SPACE),
        },
        Edit::Move {
            item_id: pod.item_id,
            to: pod_to,
        },
    ])
    .map_err(plan_refused)?;

    // ---- the pilot's side ----
    let now = crate::pyutil::filetime_now();
    crate::modules::stop_ship(reg, hull.item_id, now);
    // The locks this ship holds go; the locks ON it stay.
    crate::targeting::clear_all(reg, actor, crate::targeting::Reason::ShipChanged);
    crate::targeting::left_ship_for_another(
        reg,
        actor,
        hull.item_id,
        crate::targeting::Reason::ShipChanged,
    );
    // A self-destruct cancels itself on the next tick (`p.ship_id` differs).
    crate::safe_logoff::abort(reg, Some(actor));

    // ---- the world ----
    let mut motion = here.motion;
    motion.after(Action::Stop { ball: hull.item_id });
    let a = Abandoned {
        ship_id: hull.item_id,
        type_id: hull.type_id,
        owner_id: hull.owner_id,
        corp_id: c.corporation_id,
        alliance_id: c.alliance_id,
        system,
        abandoned_at: now,
    };
    reg.roster_mut().leave(actor);
    pilotless::abandon(reg, a, motion);
    let pod_motion = Motion::arrived(motion.position, [0.0; 3]);
    reg.roster_mut()
        .arrive(Presence::arriving(actor, pod.item_id, system, pod_motion));
    if let Some(ch) = reg.character_mut(actor) {
        ch.ship_id = pod.item_id;
        ch.ship_type_id = pod.type_id;
    }

    // ---- what the park is told ----
    send_handoff(reg, system, abandoned_entries(reg, &a));
    let corp = i32::try_from(c.corporation_id).unwrap_or(-1);
    let alliance = c
        .alliance_id
        .and_then(|x| i32::try_from(x).ok())
        .unwrap_or(-1);
    let ball =
        crate::destiny::pilot_ball(reg, pod.item_id, pod.type_id, corp, alliance, &pod_motion);
    let slim = crate::destiny::Slim {
        item_id: pod.item_id,
        type_id: pod.type_id,
        owner_id: actor,
        char_id: Some(actor),
        corp_id: Some(c.corporation_id),
        response_thresholds: None,
    };
    let damage = crate::combat::damage_state(reg, pod.item_id, now).to_pyvalue();
    let stamp = reg.next_destiny_stamp();
    let to = crate::dungeon::audience(reg, system);
    if let Some(added) = BallsAdded::new(
        to,
        stamp,
        Park {
            current_time: stamp,
            balls: vec![ball],
        },
        vec![(slim, Some(damage))],
    ) {
        let extras = crate::slim_extra::for_slims(reg, &[pod.item_id]);
        reg.push(added.with_extras(extras));
    }
    // The one message the client cannot live without: the same bare
    // `{'shipid': (old, new)}` the death path sends in space.
    reg.queue_session_change(actor, shipid_change(hull.item_id, pod.item_id));
    Ok(PyValue::None)
}

/// `ship.Board(shipID, oldShipID)` for a pilot in space. Docked boarding is
/// [`super::board`]'s.
pub(super) fn board_in_space(
    reg: &mut Registry,
    c: &crate::model::Character,
    ship_id: i64,
) -> Result<PyValue, ServiceError> {
    let actor = c.character_id;
    // ---- refusals, nothing changed ----
    reg.advance_character(actor);
    let Some(here) = reg.roster().get(actor).copied() else {
        return Err(custom_notify("You are not in space."));
    };
    let Some(ship) = reg.item(ship_id).cloned() else {
        return Err(bare_user_error("ShipNotHere"));
    };
    if ship.category_id != crate::model::CATEGORY_SHIP || !ship.singleton {
        return Err(bare_user_error("ShipNotBoardable"));
    }
    if ship_id == c.ship_id {
        // Already aboard: the state, and no change [A].
        return Ok(ship_state(reg, ship_id));
    }
    // A ship in the roster under a character's key has a pilot.
    match reg.roster().by_ship(ship_id).copied() {
        None => return Err(bare_user_error("ShipNotHere")),
        Some(p) if p.character_id != ship_id => {
            return Err(bare_user_error("ShipAlreadyHasPilot"));
        }
        Some(_) => {}
    }
    let Some(a) = pilotless::get(reg, ship_id) else {
        return Err(bare_user_error("ShipAlreadyHasPilot"));
    };
    if a.system != here.solar_system_id {
        return Err(bare_user_error("ShipNotHere"));
    }
    reg.advance_character(ship_id);
    let Some(there) = reg.roster().get(ship_id).copied() else {
        return Err(bare_user_error("ShipNotHere"));
    };
    if here.motion.warp_active() || there.motion.warp_active() {
        return Err(bare_user_error("ShipInWarp"));
    }
    let radius =
        |type_id: i64| f64::from(crate::destiny::hull::of_in(reg.content(), type_id).radius);
    let surface = crate::space_state::distance(here.motion.position, there.motion.position)
        - radius(c.ship_type_id)
        - radius(ship.type_id);
    if surface > BOARD_RANGE + crate::combat::LOOT_RANGE_SLACK {
        return Err(ServiceError::Raise(eve_proto::user_error(
            "NotCloseEnoughToBoard",
            crate::pyutil::pypairs(vec![("maxdist", PyValue::Real(BOARD_RANGE))]),
        )));
    }
    let lockers = reg.locks().targeters_of(ship_id);
    if ship.owner_id != actor {
        let player_lock = lockers.iter().any(|s| {
            *s != c.ship_id
                && reg
                    .roster()
                    .by_ship(*s)
                    .and_then(|p| reg.pilot(p.character_id))
                    .is_some_and(|pl| pl.char_id.is_some())
        });
        if player_lock {
            return Err(bare_user_error("CantBoardTargeted"));
        }
    }
    let old = reg
        .item(c.ship_id)
        .cloned()
        .ok_or_else(|| custom_notify("This server does not know your ship."))?;
    let leaving_pod = is_capsule(&old);
    if !leaving_pod {
        refuse_leaving(reg, actor)?;
    }
    let engaged = lockers.iter().any(|s| {
        reg.roster().by_ship(*s).is_some_and(|p| {
            reg.npcs()
                .get(p.character_id)
                .is_some_and(|n| n.law.is_some())
        })
    });
    crate::concord::refuse_board(reg, Some(actor), engaged, crate::pyutil::filetime_now())?;
    crate::skillgate::check(reg, actor, ship.type_id, "ShipHasSkillPrerequisites")?;
    if !crate::subsystems::may_undock(reg.store(), ship_id) {
        return Err(bare_user_error("TooFewSubSystemsToUndock"));
    }
    let Some(pod) = crate::fitting::capsule_of(reg.store(), c).cloned() else {
        return Err(custom_notify("Your capsule could not be found."));
    };

    // ---- the store, in one plan ----
    let system = here.solar_system_id;
    let home = match c.station_id {
        Some(station) => Slot::new(actor, station, crate::model::FLAG_HANGAR),
        None => Slot::new(actor, system, crate::combat::FLAG_IN_SPACE),
    };
    let mut edits = Vec::new();
    if ship.owner_id != actor {
        // The hull's contents, at any depth, change owner in place.
        let mut open = vec![ship_id];
        while let Some(container) = open.pop() {
            for i in reg.store().contents_of(container) {
                edits.push(Edit::Move {
                    item_id: i.item_id,
                    to: Slot::new(actor, i.location_id, i.flag_id),
                });
                open.push(i.item_id);
            }
        }
    }
    edits.push(Edit::Move {
        item_id: ship_id,
        to: home,
    });
    edits.push(Edit::Move {
        item_id: pod.item_id,
        to: Slot::new(actor, ship_id, crate::fitting::FLAG_CAPSULE),
    });
    if !leaving_pod {
        edits.push(Edit::Move {
            item_id: old.item_id,
            to: Slot::new(old.owner_id, system, crate::combat::FLAG_IN_SPACE),
        });
    }
    reg.apply_items(&edits).map_err(plan_refused)?;

    // ---- the world ----
    let now = crate::pyutil::filetime_now();
    if !leaving_pod {
        crate::modules::stop_ship(reg, old.item_id, now);
    }
    crate::targeting::clear_all(reg, actor, crate::targeting::Reason::ShipChanged);
    crate::targeting::left_ship_for_another(
        reg,
        actor,
        old.item_id,
        crate::targeting::Reason::ShipChanged,
    );
    crate::safe_logoff::abort(reg, Some(actor));
    let mut old_motion = here.motion;
    old_motion.after(Action::Stop { ball: old.item_id });
    reg.roster_mut().leave(actor);
    pilotless::release(reg, ship_id);
    let abandoned_old = (!leaving_pod).then_some(Abandoned {
        ship_id: old.item_id,
        type_id: old.type_id,
        owner_id: old.owner_id,
        corp_id: c.corporation_id,
        alliance_id: c.alliance_id,
        system,
        abandoned_at: now,
    });
    if let Some(a) = abandoned_old {
        pilotless::abandon(reg, a, old_motion);
    }
    reg.roster_mut()
        .arrive(Presence::arriving(actor, ship_id, system, there.motion));
    switch_ship(reg, actor, ship_id, ship.type_id);
    crate::modules::offline_unskilled(reg, actor, ship_id);

    // ---- what the park is told ----
    let mut entries = abandoned_old.map_or_else(Vec::new, |a| abandoned_entries(reg, &a));
    entries.push(HandoffEntry::Act(Action::SetBallInteractive {
        ball: ship_id,
        interactive: true,
    }));
    // A piloted ship is massive (the server collides it from now on).
    entries.push(HandoffEntry::Act(Action::SetBallMassive {
        ball: ship_id,
        massive: true,
    }));
    if leaving_pod {
        // The capsule left behind has no pilot: the server no longer collides
        // it, so the clients must not either, for its POD_REMOVE_DELAY.
        entries.push(HandoffEntry::Act(Action::SetBallMassive {
            ball: pod.item_id,
            massive: false,
        }));
    }
    entries.push(slim_entry(
        reg,
        crate::destiny::Slim {
            item_id: ship_id,
            type_id: ship.type_id,
            owner_id: actor,
            char_id: Some(actor),
            corp_id: Some(c.corporation_id),
            response_thresholds: None,
        },
    ));
    entries.extend(flight_entries(reg, ship_id));
    send_handoff(reg, system, entries);
    if leaving_pod {
        // After the session change, or the client's ego names no ball.
        let to = crate::dungeon::audience(reg, system);
        let stamp = reg.next_destiny_stamp();
        if let Some(gone) = BallsRemoved::new(to, stamp, vec![pod.item_id]) {
            reg.schedule(POD_REMOVE_DELAY, gone);
        }
    }
    // The AIR tutorial's `Ship Boarded` event (scene 1.1's Astero).
    crate::air_npe::on_ship_boarded(reg, actor, ship.type_id, ship.group_id, system);
    Ok(ship_state(reg, ship_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::tests::{
        ALICE, ALICE_POD, ALICE_SHIP, BOB, BOB_POD, BOB_SHIP, SYSTEM, alice_has_bob_locked, due,
        locked_on,
    };
    use std::time::{Duration, Instant};

    const STATION: i64 = 60_700_007;
    const CAROL: i64 = 1_980_003;
    const CAROL_SHIP: i64 = 100_000_003;

    fn call(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        let bind = PyValue::Tuple(vec![PyValue::Int(SYSTEM), PyValue::Int(5)]);
        dispatch(
            reg,
            &Request {
                service: "ship",
                method,
                args,
                kwargs: &[],
                bind_params: Some(&bind),
                actor: Some(who),
            },
        )
    }

    /// The key of a raised `UserError`.
    fn key(r: Result<PyValue, ServiceError>) -> String {
        let Err(ServiceError::Raise(PyValue::ObjectEx { header, .. })) = r else {
            panic!("not a raised exception: {r:?}")
        };
        let PyValue::Tuple(parts) = header.as_ref() else {
            panic!("a 0x22 header is a tuple")
        };
        let PyValue::Tuple(args) = &parts[1] else {
            panic!("UserError's header args are a tuple")
        };
        String::from_utf8_lossy(crate::pyutil::as_bytes(&args[0]).expect("key")).into_owned()
    }

    fn eject_it(reg: &mut Registry, who: i64) -> Result<PyValue, ServiceError> {
        call(reg, who, "Eject", &[])
    }

    fn board_it(reg: &mut Registry, who: i64, ship: i64) -> Result<PyValue, ServiceError> {
        let old = reg.character(who).expect("character").ship_id;
        call(reg, who, "Board", &[PyValue::Int(ship), PyValue::Int(old)])
    }

    fn world() -> Registry {
        let mut reg = alice_has_bob_locked();
        for who in [ALICE, BOB] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        let _ = reg.take_pending();
        reg
    }

    type Entries = Vec<(Vec<u8>, Vec<PyValue>)>;

    /// The `(name, args)` of every entry of the destiny pushes among `pushes`.
    fn entries_of(pushes: Vec<Box<dyn crate::notify::Push>>) -> Entries {
        let mut out = Vec::new();
        for p in pushes {
            let (name, args) = p.to_push();
            if name != "DoDestinyUpdate" {
                continue;
            }
            let PyValue::List(list) = &args[0] else {
                panic!("a batch")
            };
            for e in list {
                let PyValue::Tuple(e) = e else {
                    panic!("entry")
                };
                let PyValue::Tuple(call) = &e[1] else {
                    panic!("call")
                };
                let PyValue::Tuple(a) = &call[1] else {
                    panic!("args")
                };
                out.push((
                    crate::pyutil::as_bytes(&call[0]).expect("name").to_vec(),
                    a.clone(),
                ));
            }
        }
        out
    }

    /// What the answer's flush would send.
    fn pending_entries(reg: &mut Registry) -> Entries {
        entries_of(reg.take_pending().into_iter().map(|(_, p)| p).collect())
    }

    /// What the clock sends within five seconds.
    fn due_entries(reg: &mut Registry) -> Entries {
        entries_of(reg.take_due(Instant::now() + Duration::from_secs(5)))
    }

    /// Both, the answer's first.
    fn entries(reg: &mut Registry) -> Entries {
        let mut out = pending_entries(reg);
        out.extend(due_entries(reg));
        out
    }

    fn names(e: &[(Vec<u8>, Vec<PyValue>)]) -> Vec<String> {
        e.iter()
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
            .collect()
    }

    fn shipid_changes(reg: &mut Registry, who: i64) -> Vec<(i64, i64)> {
        reg.take_session_changes()
            .into_iter()
            .filter(|(c, _)| *c == who)
            .filter_map(|(_, ch)| match crate::pyutil::pairs_get(&ch, b"shipid") {
                Some(PyValue::Tuple(t)) => match (&t[0], &t[1]) {
                    (PyValue::Int(a), PyValue::Int(b)) => Some((*a, *b)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// **Eject** leaves the hull at `(system, 0)` and the capsule at the
    /// pilot's hangar; the roster holds the pilot in the capsule and the hull
    /// under its own id; the pilot's session changes `shipid` once.
    #[test]
    fn eject_leaves_the_hull_in_space_and_the_pilot_in_the_capsule() {
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let hull = reg.item(ALICE_SHIP).expect("the hull is kept");
        assert_eq!(
            (hull.location_id, hull.flag_id, hull.owner_id),
            (SYSTEM, crate::combat::FLAG_IN_SPACE, ALICE)
        );
        let pod = reg.item(ALICE_POD).expect("the capsule");
        assert_eq!(
            (pod.location_id, pod.flag_id),
            (STATION, crate::model::FLAG_HANGAR)
        );
        let me = reg.character(ALICE).expect("Alice");
        assert_eq!(me.ship_id, ALICE_POD);
        assert_eq!(me.ship_type_id, crate::damage::CAPSULE_TYPE_ID);
        assert_eq!(reg.roster().get(ALICE).map(|p| p.ship_id), Some(ALICE_POD));
        let abandoned = reg.roster().get(ALICE_SHIP).expect("the hull's presence");
        assert_eq!(abandoned.ship_id, ALICE_SHIP);
        assert!(pilotless::is_pilotless(&reg, ALICE_SHIP));
        assert_eq!(
            reg.pilot(ALICE_SHIP).map(|p| (p.owner_id, p.char_id)),
            Some((ALICE, None))
        );
        assert!(
            reg.deltas()
                .of(pilotless::KIND)
                .contains_key(&vec![ALICE_SHIP.to_string()]),
            "the row that survives a restart"
        );
        assert_eq!(
            shipid_changes(&mut reg, ALICE),
            vec![(ALICE_SHIP, ALICE_POD)]
        );
    }

    /// The batch, in order: the hull's interactive bit off, its massive bit
    /// off (the server collides only piloted ships), stopped, its slim with no
    /// `charID`; then the capsule's `AddBalls2`.
    #[test]
    fn eject_batch_order() {
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let e = entries(&mut reg);
        let n = names(&e);
        assert_eq!(
            &n[..4],
            [
                "SetBallInteractive",
                "SetBallMassive",
                "Stop",
                "OnSlimItemChange"
            ],
            "{n:?}"
        );
        assert_eq!(
            e[0].1,
            vec![PyValue::Int(ALICE_SHIP), PyValue::Int(0)],
            "interactive off"
        );
        assert_eq!(
            e[1].1,
            vec![PyValue::Int(ALICE_SHIP), PyValue::Int(0)],
            "massive off"
        );
        let PyValue::Instance { state, .. } = &e[3].1[1] else {
            panic!("the slim is an instance: {:?}", e[3].1)
        };
        assert_eq!(
            crate::pyutil::dict_get(state, b"charID"),
            Some(&PyValue::None)
        );
        assert_eq!(
            crate::pyutil::dict_get(state, b"ownerID"),
            Some(&PyValue::Int(ALICE))
        );
        assert_eq!(n.last().map(String::as_str), Some("AddBalls2"), "{n:?}");
    }

    /// A pilot who arrives later is sent the hull as a ball with no
    /// interactive bit and a slim with no `charID`.
    #[test]
    fn a_fresh_scene_holds_the_hull_pilotless() {
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let scene = crate::destiny::scene(&reg, SYSTEM, BOB_SHIP, None, 1);
        let ball = scene
            .park
            .balls
            .iter()
            .find(|b| b.ball_id == ALICE_SHIP)
            .expect("the hull is in the scene");
        assert_eq!(ball.flags, pilotless::FLAGS_PILOTLESS);
        assert_eq!(ball.flags, 0x13);
        let pod = scene.park.balls.iter().find(|b| b.ball_id == ALICE_POD);
        assert_eq!(pod.map(|b| b.flags), Some(crate::destiny::FLAGS_PILOT_BALL));
        let slim = scene
            .slims
            .iter()
            .find(|s| s.item_id == ALICE_SHIP)
            .expect("its slim");
        assert_eq!((slim.char_id, slim.owner_id), (None, ALICE));
    }

    /// Every refusal changes nothing and pushes nothing.
    #[test]
    fn refusals_change_nothing() {
        // In the capsule.
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let _ = (entries(&mut reg), reg.take_session_changes());
        let items = reg.store().items().to_vec();
        let ok = |reg: &mut Registry, r: Result<PyValue, ServiceError>, why: &str, want: &str| {
            assert_eq!(key(r), want, "{why}: the key");
            assert_eq!(reg.store().items(), &items[..], "{why}: store");
            assert!(reg.take_pending().is_empty(), "{why}: pending");
            assert!(reg.take_session_changes().is_empty(), "{why}: session");
        };
        let r = eject_it(&mut reg, ALICE);
        ok(
            &mut reg,
            r,
            "ejecting from a capsule",
            "CannotEjectFromCapsule",
        );
        // Out of range.
        let far = [9.0e9, 0.0, 0.0];
        reg.roster_mut()
            .get_mut(ALICE_SHIP)
            .expect("hull")
            .motion
            .position = far;
        let r = board_it(&mut reg, ALICE, ALICE_SHIP);
        ok(&mut reg, r, "too far", "NotCloseEnoughToBoard");
        // A ship with a pilot.
        let r = board_it(&mut reg, ALICE, BOB_SHIP);
        ok(&mut reg, r, "a piloted ship", "ShipAlreadyHasPilot");
        // Not in space.
        let r = board_it(&mut reg, ALICE, 424_242);
        ok(&mut reg, r, "no such ship", "ShipNotHere");
        // Warping.
        let mut reg = world();
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp: crate::destiny::tick() as i32 - 1,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
        let before = reg.store().items().to_vec();
        assert_eq!(key(eject_it(&mut reg, ALICE)), "ShipInWarp");
        assert_eq!(reg.store().items(), &before[..]);
        assert!(reg.take_pending().is_empty());
        assert!(reg.take_session_changes().is_empty());
        assert!(!pilotless::is_pilotless(&reg, ALICE_SHIP));
    }

    /// The locks the hull holds are dropped; the locks on it stay (which is
    /// what makes `CantBoardTargeted` work).
    #[test]
    fn locks_on_the_hull_survive_and_its_own_go() {
        let mut reg = world();
        reg.locks_mut().add(BOB_SHIP, ALICE_SHIP);
        eject_it(&mut reg, ALICE).expect("ejected");
        assert!(reg.locks().targeters_of(ALICE_SHIP).contains(&BOB_SHIP));
        assert!(
            reg.locks().targeters_of(BOB_SHIP).is_empty(),
            "Alice's lock on Bob went with the ship change"
        );
    }

    /// Boarding someone else's abandoned ship: the hull and what is in it
    /// change owner, the boarder's old ship is left in space, the ball is
    /// told interactive again.
    #[test]
    fn boarding_steals_the_hull_and_its_contents() {
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let _ = (entries(&mut reg), reg.take_session_changes());
        let gun = reg
            .store()
            .contents_of(ALICE_SHIP)
            .find(|i| i.flag_id == 27)
            .map(|i| i.item_id)
            .expect("Alice's gun");
        board_it(&mut reg, BOB, ALICE_SHIP).expect("boarded");
        let hull = reg.item(ALICE_SHIP).expect("hull");
        assert_eq!(
            (hull.owner_id, hull.location_id, hull.flag_id),
            (BOB, STATION, crate::model::FLAG_HANGAR)
        );
        assert_eq!(
            reg.item(gun).map(|i| (i.owner_id, i.location_id)),
            Some((BOB, ALICE_SHIP))
        );
        let bobs_pod = reg.item(BOB_POD).expect("the capsule");
        assert_eq!(
            (bobs_pod.location_id, bobs_pod.flag_id),
            (ALICE_SHIP, crate::fitting::FLAG_CAPSULE)
        );
        assert!(!pilotless::is_pilotless(&reg, ALICE_SHIP));
        assert!(
            pilotless::is_pilotless(&reg, BOB_SHIP),
            "Bob's own ship stays"
        );
        assert_eq!(reg.roster().get(BOB).map(|p| p.ship_id), Some(ALICE_SHIP));
        assert!(reg.roster().get(ALICE_SHIP).is_none());
        assert_eq!(shipid_changes(&mut reg, BOB), vec![(BOB_SHIP, ALICE_SHIP)]);
        let e = entries(&mut reg);
        let interactive_on = e.iter().any(|(n, a)| {
            n == b"SetBallInteractive" && a == &vec![PyValue::Int(ALICE_SHIP), PyValue::Int(1)]
        });
        assert!(interactive_on, "{:?}", names(&e));
        let massive_on = e.iter().any(|(n, a)| {
            n == b"SetBallMassive" && a == &vec![PyValue::Int(ALICE_SHIP), PyValue::Int(1)]
        });
        assert!(massive_on, "a boarded hull is a piloted ship again");
    }

    /// Every `'otherlost'` the actor is sent (answer and clock), by locker ship.
    fn told_let_go(reg: &mut Registry, who: i64) -> Vec<i64> {
        let mut pushes: Vec<_> = reg.take_pending().into_iter().map(|(_, p)| p).collect();
        pushes.extend(reg.take_due(Instant::now() + Duration::from_secs(5)));
        let mut told: Vec<i64> = pushes
            .iter()
            .filter(|p| p.audience().unwrap_or_default() == vec![who])
            .filter_map(|p| {
                let (name, args) = p.to_push();
                match (name, &args[..]) {
                    ("OnTarget", [PyValue::Bytes(w), PyValue::Int(ship), ..])
                        if w == b"otherlost" =>
                    {
                        Some(*ship)
                    }
                    _ => None,
                }
            })
            .collect();
        told.sort_unstable();
        told
    }

    /// A pilot who ejects or boards in space is told `'otherlost'` for every
    /// locker of the ship he left, so his client's `targetedBy` does not keep
    /// them; the locks stay on the hull. The rat id is invented.
    #[test]
    fn leaving_a_locked_hull_tells_the_pilot_every_locker_let_go() {
        const RAT: i64 = 7_000_000_902;
        let mut reg = world();
        reg.locks_mut().add(RAT, ALICE_SHIP);
        reg.locks_mut().add(BOB_SHIP, ALICE_SHIP);
        eject_it(&mut reg, ALICE).expect("ejected");
        assert_eq!(told_let_go(&mut reg, ALICE), vec![BOB_SHIP, RAT]);
        assert!(reg.locks().targeters_of(ALICE_SHIP).contains(&RAT));

        // Bob steps from his locked ship into Alice's abandoned hull.
        reg.locks_mut().add(RAT, BOB_SHIP);
        let _ = told_let_go(&mut reg, BOB);
        board_it(&mut reg, BOB, ALICE_SHIP).expect("boarded");
        assert_eq!(told_let_go(&mut reg, BOB), vec![RAT]);
    }

    /// A ship another player has locked cannot be boarded by a stranger, and
    /// can by its owner.
    #[test]
    fn a_locked_ship_is_boarded_only_by_its_owner() {
        let mut reg = locked_on(&[(CAROL, CAROL_SHIP)], None);
        for who in [ALICE, BOB, CAROL] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        eject_it(&mut reg, ALICE).expect("ejected");
        reg.locks_mut().add(CAROL_SHIP, ALICE_SHIP);
        assert_eq!(
            key(board_it(&mut reg, BOB, ALICE_SHIP)),
            "CantBoardTargeted"
        );
        assert!(pilotless::is_pilotless(&reg, ALICE_SHIP));
        board_it(&mut reg, ALICE, ALICE_SHIP).expect("the owner may");
        assert_eq!(reg.character(ALICE).map(|c| c.ship_id), Some(ALICE_SHIP));
    }

    /// Leaving the capsule: its ball goes one second AFTER the session
    /// change, scheduled and not pushed now.
    #[test]
    fn the_pod_ball_is_removed_later_not_now() {
        let mut reg = world();
        eject_it(&mut reg, ALICE).expect("ejected");
        let _ = (entries(&mut reg), reg.take_session_changes());
        board_it(&mut reg, ALICE, ALICE_SHIP).expect("back aboard");
        let now = pending_entries(&mut reg);
        assert!(
            now.iter().any(|(n, a)| n == b"SetBallMassive"
                && a == &vec![PyValue::Int(ALICE_POD), PyValue::Int(0)]),
            "the pilotless capsule is not massive while it waits: {:?}",
            names(&now)
        );
        assert!(
            !names(&now).iter().any(|n| n == "RemoveBalls"),
            "not in the answer's flush: {:?}",
            names(&now)
        );
        assert_eq!(reg.scheduled_count(), 1, "one scheduled removal");
        let later = due_entries(&mut reg);
        let removed = later.iter().any(|(n, a)| {
            n == b"RemoveBalls" && a == &vec![PyValue::List(vec![PyValue::Int(ALICE_POD)])]
        });
        assert!(
            removed,
            "the capsule's removal is on the clock: {:?}",
            names(&later)
        );
    }

    /// An abandoned ship that is shot to pieces dies with a wreck in its own
    /// system, owned by its owner, and leaves the roster, the table and the
    /// delta rows; shooting it is no crime for the shooter.
    #[test]
    fn a_shot_hull_dies_in_its_system() {
        let mut reg = world();
        eject_it(&mut reg, BOB).expect("ejected");
        let _ = (entries(&mut reg), due(&mut reg));
        crate::combat::tests::kill_bob(&mut reg);
        assert!(!pilotless::is_pilotless(&reg, BOB_SHIP));
        assert!(reg.roster().get(BOB_SHIP).is_none());
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        assert!(
            !reg.deltas()
                .of(pilotless::KIND)
                .contains_key(&vec![BOB_SHIP.to_string()])
        );
        let wreck = crate::combat::wrecks_in(&reg, SYSTEM);
        assert_eq!(wreck.len(), 1, "one wreck in the system");
        assert!(
            !crate::crimewatch::is_criminal(&reg, ALICE, crate::pyutil::filetime_now())
                && !crate::crimewatch::is_suspect(&reg, ALICE, crate::pyutil::filetime_now()),
            "no crime"
        );
        assert!(
            reg.roster().get(BOB).is_some(),
            "Bob, in his capsule, is untouched"
        );
        let _ = BOB_POD;
    }

    /// An invented price for the Rifter-typed test hull.
    const INVENTED_PRICE: f64 = 100_000.0;
    const RIFTER: i64 = 587;

    /// **An abandoned ship shot dead with its owner elsewhere** (BE-3): the
    /// wreck lands in the ship's own system though the owner is docked far
    /// away; the insurance the owner bought pays once, on journal ref 19
    /// with the hull's type as reference; the contract ends; the shooter is
    /// no criminal.
    #[test]
    fn a_shot_hull_with_its_owner_away_pays_the_insurance_and_leaves_a_wreck() {
        let mut content = crate::content::Content::fixture();
        content.insert(
            "econ_prices",
            crate::econ_prices::Table::parse(&[format!(
                "{RIFTER}\t{INVENTED_PRICE}\trecipe\t{}\t6\t25\t1\t1",
                INVENTED_PRICE / 2.0
            )])
            .expect("parses"),
        );
        let mut reg = locked_on(&[], Some(std::sync::Arc::new(content)));
        for who in [ALICE, BOB] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        let now = crate::pyutil::filetime_now();
        let fraction = 0.9;
        reg.deltas_mut().put(
            crate::service::insurance::KIND,
            vec![BOB_SHIP.to_string()],
            vec![
                BOB.to_string(),
                RIFTER.to_string(),
                fraction.to_string(),
                now.to_string(),
                (now + 100 * 10_000_000 * 3_600).to_string(),
            ],
        );
        let payout = fraction * crate::service::insurance::full_price(&reg, RIFTER);
        assert!(payout > 0.0, "setup: the hull is priced");
        let _ = reg.take_pending();
        eject_it(&mut reg, BOB).expect("ejected");
        // Bob's capsule docks far away: the owner is not in space at all.
        crate::crimewatch::leave_space(&mut reg, BOB);
        assert!(reg.roster().get(BOB).is_none(), "setup: the owner is away");
        let before = reg.character(BOB).expect("Bob").balance;
        let _ = reg.take_pending();
        crate::combat::tests::kill_bob(&mut reg);
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        let wrecks = crate::combat::wrecks_in(&reg, SYSTEM);
        assert_eq!(wrecks.len(), 1, "the wreck is in the hull's system");
        assert_eq!(reg.character(BOB).expect("Bob").balance, before + payout);
        let paid: Vec<_> = reg
            .journal()
            .entries()
            .iter()
            .filter(|e| e.entry_type_id == crate::service::insurance::REF_INSURANCE)
            .filter(|e| e.amount == payout)
            .collect();
        assert_eq!(paid.len(), 1, "paid once");
        assert_eq!((paid[0].wallet(), paid[0].reference_id), (BOB, RIFTER));
        assert!(
            reg.deltas().of(crate::service::insurance::KIND).is_empty(),
            "the contract ended with the ship"
        );
        let ft = crate::pyutil::filetime_now();
        assert!(
            !crate::crimewatch::is_criminal(&reg, ALICE, ft)
                && !crate::crimewatch::is_suspect(&reg, ALICE, ft),
            "no crime for the shooter"
        );
    }

    /// One volley by `who` at the abandoned `BOB_SHIP`, booked as `combat::fire`
    /// books it.
    fn shoot_hull(reg: &mut Registry, who: i64, ship: i64, weapon: i64, dealt: f64) {
        let shot = crate::damage::Shot {
            at: crate::pyutil::filetime_now(),
            attacker_ship_id: ship,
            weapon_item_id: weapon,
            weapon_type_id: weapon,
            target_ship_id: BOB_SHIP,
            damage: crate::damage::Damage::default(),
            hit_quality: 3,
        };
        crate::killmail::on_volley(reg, &shot, who, BOB_SHIP, dealt);
    }

    /// Bob ejects, Carol boards his hull and ejects in turn, Bob is docked
    /// far away, then Alice shoots the hull dead. Bob shot it before Carol
    /// boarded, Carol before she ejected.
    fn alice_shoots_carols_hull(reg: &mut Registry) {
        for who in [ALICE, BOB, CAROL] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        eject_it(reg, BOB).expect("Bob ejected");
        shoot_hull(reg, BOB, BOB_SHIP, 3_001, 7.0);
        // Alice's lock on the hull would forbid a stranger boarding it.
        reg.locks_mut().remove(ALICE_SHIP, BOB_SHIP);
        // Carol shoots the empty hull, then boards it: on her own mail she
        // must not be an attacker.
        shoot_hull(reg, CAROL, CAROL_SHIP, 3_002, 9.0);
        board_it(reg, CAROL, BOB_SHIP).expect("Carol boarded");
        assert_eq!(reg.item(BOB_SHIP).map(|i| i.owner_id), Some(CAROL));
        eject_it(reg, CAROL).expect("Carol ejected");
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        assert!(pilotless::is_pilotless(reg, BOB_SHIP));
        for who in [BOB, CAROL] {
            crate::crimewatch::leave_space(reg, who);
        }
        let _ = (entries(reg), due(reg));
        for who in [BOB, CAROL] {
            reg.character_mut(who).expect("character").solar_system_id = None;
        }
        crate::combat::tests::kill_bob(reg);
    }

    /// [`alice_shoots_carols_hull`] up to the killing volley: Carol has
    /// boarded Bob's hull and ejected, both are away.
    fn alice_shoots_carols_hull_setup(reg: &mut Registry) {
        for who in [ALICE, BOB, CAROL] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        eject_it(reg, BOB).expect("Bob ejected");
        reg.locks_mut().remove(ALICE_SHIP, BOB_SHIP);
        board_it(reg, CAROL, BOB_SHIP).expect("Carol boarded");
        eject_it(reg, CAROL).expect("Carol ejected");
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        assert!(pilotless::is_pilotless(reg, BOB_SHIP));
        for who in [BOB, CAROL] {
            crate::crimewatch::leave_space(reg, who);
            reg.character_mut(who).expect("character").solar_system_id = None;
        }
        let _ = (entries(reg), due(reg));
    }

    /// **An abandoned ship shot dead writes a killmail** (#132, `be-3`
    /// option b): the shooter has the kill, the victim is whoever owns the
    /// hull when it dies, wherever that character is. The system is the
    /// hull's own, Bob's combat log stays empty, and the victim, who shot the
    /// hull before boarding it, is no attacker on her own mail.
    #[test]
    fn a_shot_hull_writes_a_killmail_with_its_current_owner_as_victim() {
        let mut reg = locked_on(&[(CAROL, CAROL_SHIP)], None);
        alice_shoots_carols_hull(&mut reg);
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        let mails = crate::killmail::involving(&reg, ALICE, None, 10);
        assert_eq!(mails.len(), 1, "one report for the shooter");
        let r = &mails[0];
        assert_eq!(r.victim, CAROL, "the hull's owner, not its first one");
        assert_eq!(r.final_blow.character, Some(ALICE));
        assert!(r.final_blow.damage > 0, "the killing volley's damage");
        assert!(r.final_blow.weapon_type.is_some(), "and its weapon");
        assert_eq!(r.system, SYSTEM, "the hull's system, not the owner's");
        assert_eq!(r.ship_type, RIFTER);
        let others: Vec<_> = r.attackers.iter().filter_map(|a| a.character).collect();
        assert_eq!(others, vec![BOB], "Bob is the second shooter");
        assert!(
            !others.contains(&CAROL) && r.final_blow.character != Some(CAROL),
            "the victim is on her own mail as nothing"
        );
        assert_eq!(crate::killmail::involving(&reg, CAROL, None, 10).len(), 1);
        assert_eq!(
            crate::killmail::involving(&reg, BOB, None, 10).len(),
            1,
            "Bob shot it, so the mail is in his log, not as its victim"
        );
    }

    /// A world with prices where Bob holds a policy at `FRACTION` on his hull
    /// and Carol is a second pilot; the test base share is `BASE`.
    const FRACTION: f64 = 0.9;
    const BASE: f64 = 0.3;

    fn insured_world() -> Registry {
        let mut content = crate::content::Content::fixture();
        content.insert(
            "econ_prices",
            crate::econ_prices::Table::parse(&[format!(
                "{RIFTER}\t{INVENTED_PRICE}\trecipe\t{}\t6\t25\t1\t1",
                INVENTED_PRICE / 2.0
            )])
            .expect("parses"),
        );
        let mut reg = locked_on(&[(CAROL, CAROL_SHIP)], Some(std::sync::Arc::new(content)));
        let now = crate::pyutil::filetime_now();
        reg.deltas_mut().put(
            crate::service::insurance::KIND,
            vec![BOB_SHIP.to_string()],
            vec![
                BOB.to_string(),
                RIFTER.to_string(),
                FRACTION.to_string(),
                now.to_string(),
                (now + 100 * 10_000_000 * 3_600).to_string(),
            ],
        );
        crate::service::insurance::tests_base::set(Some(BASE));
        reg
    }

    fn wallet(reg: &Registry, who: i64) -> f64 {
        reg.character(who).expect("character").balance
    }

    /// The split both parties must see: the policy holder (Bob) gets the
    /// policy less the base, the owner at death (Carol) the base; together
    /// the policy; the contract is gone; each is told once, Carol with the
    /// "uninsured" flag.
    fn assert_split(reg: &Registry, bob0: f64, carol0: f64) {
        let full = crate::service::insurance::full_price(reg, RIFTER);
        let (policy, base) = (FRACTION * full, BASE * full);
        assert!(base > 0.0 && policy > base, "setup: priced");
        assert_eq!(wallet(reg, BOB) - bob0, policy - base, "the holder");
        assert_eq!(wallet(reg, CAROL) - carol0, base, "the owner at death");
        let sum = (wallet(reg, BOB) - bob0) + (wallet(reg, CAROL) - carol0);
        assert!((sum - policy).abs() < 1e-6, "the parts sum to the policy");
        assert!(reg.deltas().of(crate::service::insurance::KIND).is_empty());
        let payouts = |who| {
            crate::notifications::unread(reg.deltas(), who)
                .iter()
                .filter(|n| n.type_id == crate::notifications::kind::INSURANCE_PAYOUT)
                .count()
        };
        assert_eq!((payouts(BOB), payouts(CAROL)), (1, 1), "each told once");
    }

    /// **Boarded, then lost flown** (#134): Carol boards Bob's insured hull
    /// and is shot dead in it. The policy stays Bob's across the boarding.
    #[test]
    fn a_boarded_hull_lost_flown_splits_its_policy() {
        let mut reg = insured_world();
        for who in [ALICE, BOB, CAROL] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        eject_it(&mut reg, BOB).expect("Bob ejected");
        reg.locks_mut().remove(ALICE_SHIP, BOB_SHIP);
        board_it(&mut reg, CAROL, BOB_SHIP).expect("Carol boarded");
        let held =
            crate::service::insurance::contract(&reg, BOB_SHIP, crate::pyutil::filetime_now());
        assert_eq!(
            held.map(|c| c.owner_id),
            Some(BOB),
            "the policy is still Bob's"
        );
        reg.locks_mut().add(ALICE_SHIP, BOB_SHIP);
        crate::crimewatch::leave_space(&mut reg, BOB);
        let (bob0, carol0) = (wallet(&reg, BOB), wallet(&reg, CAROL));
        crate::combat::tests::kill_bob(&mut reg);
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        assert_split(&reg, bob0, carol0);
    }

    /// **Boarded, abandoned again, then shot** (#134), both pilots away.
    #[test]
    fn a_boarded_then_abandoned_hull_shot_splits_its_policy() {
        let mut reg = insured_world();
        alice_shoots_carols_hull_setup(&mut reg);
        let (bob0, carol0) = (wallet(&reg, BOB), wallet(&reg, CAROL));
        crate::combat::tests::kill_bob(&mut reg);
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        assert_split(&reg, bob0, carol0);
    }

    /// **A restart between the boarding and the loss** keeps the policy and
    /// splits it the same: the world is rebuilt from its snapshot.
    #[test]
    fn a_restart_between_boarding_and_loss_keeps_the_split() {
        let mut reg = insured_world();
        alice_shoots_carols_hull_setup(&mut reg);
        let snap = reg.to_snapshot();
        let mut again = insured_world();
        again.apply_snapshot(snap);
        // A fresh process has nobody in space.
        for who in [BOB, CAROL] {
            crate::crimewatch::leave_space(&mut again, who);
            again.character_mut(who).expect("character").solar_system_id = None;
        }
        let held =
            crate::service::insurance::contract(&again, BOB_SHIP, crate::pyutil::filetime_now());
        assert_eq!(held.map(|c| c.owner_id), Some(BOB), "the policy survived");
        crate::pilotless::restore(&mut again);
        assert!(crate::pilotless::is_pilotless(&again, BOB_SHIP));
        let (bob0, carol0) = (wallet(&again, BOB), wallet(&again, CAROL));
        crate::combat::tests::kill_bob(&mut again);
        assert!(again.item(BOB_SHIP).is_none(), "the hull is destroyed");
        assert_split(&again, bob0, carol0);
    }

    /// **A stolen uninsured hull pays the base** (#134): Bob has no policy,
    /// Carol boards his hull and ejects, Alice shoots it dead. Carol, the
    /// owner at death, is paid the base on ref 19 and told once with the
    /// "uninsured" note; Bob, who owned it no longer, nothing.
    #[test]
    fn a_stolen_uninsured_hull_pays_its_owner_at_death_the_base() {
        let mut reg = insured_world();
        reg.deltas_mut()
            .del(crate::service::insurance::KIND, &[BOB_SHIP.to_string()]);
        alice_shoots_carols_hull_setup(&mut reg);
        let (bob0, carol0) = (wallet(&reg, BOB), wallet(&reg, CAROL));
        crate::combat::tests::kill_bob(&mut reg);
        assert!(reg.item(BOB_SHIP).is_none(), "the hull is destroyed");
        let full = crate::service::insurance::full_price(&reg, RIFTER);
        assert_eq!(wallet(&reg, CAROL) - carol0, BASE * full);
        assert_eq!(wallet(&reg, BOB), bob0);
        let told = |who| {
            crate::notifications::unread(reg.deltas(), who)
                .iter()
                .filter(|n| n.type_id == crate::notifications::kind::INSURANCE_PAYOUT)
                .count()
        };
        assert_eq!((told(CAROL), told(BOB)), (1, 0));
    }

    /// **An unstolen hull pays its holder all of it** (#134): Bob ejects and
    /// the hull is shot with nobody else having boarded it.
    #[test]
    fn an_unstolen_abandoned_hull_pays_its_holder_in_full() {
        let mut reg = insured_world();
        for who in [ALICE, BOB] {
            reg.character_mut(who).expect("character").station_id = Some(STATION);
        }
        eject_it(&mut reg, BOB).expect("Bob ejected");
        crate::crimewatch::leave_space(&mut reg, BOB);
        reg.character_mut(BOB).expect("Bob").solar_system_id = None;
        let (bob0, carol0) = (wallet(&reg, BOB), wallet(&reg, CAROL));
        crate::combat::tests::kill_bob(&mut reg);
        let full = crate::service::insurance::full_price(&reg, RIFTER);
        assert_eq!(wallet(&reg, BOB) - bob0, FRACTION * full);
        assert_eq!(wallet(&reg, CAROL), carol0);
    }

    /// **The shooter and the owner are told** (#132): the final blow gets
    /// the kill report (141), the hull's owner, offline, the victim's (140).
    #[test]
    fn a_shot_hull_tells_the_shooter_and_the_offline_owner() {
        let mut reg = locked_on(&[(CAROL, CAROL_SHIP)], None);
        alice_shoots_carols_hull(&mut reg);
        let kinds = |reg: &Registry, who| -> Vec<i64> {
            crate::notifications::unread(reg.deltas(), who)
                .iter()
                .map(|n| n.type_id)
                .collect()
        };
        assert!(reg.roster().get(CAROL).is_none(), "setup: Carol is away");
        assert_eq!(
            kinds(&reg, CAROL),
            vec![crate::notifications::kind::KILL_REPORT_VICTIM]
        );
        assert_eq!(
            kinds(&reg, ALICE),
            vec![crate::notifications::kind::KILL_REPORT_FINAL_BLOW]
        );
        assert!(kinds(&reg, BOB).is_empty(), "Bob owned it no longer");
    }

    /// **Eject cancels a running self-destruct** (BE-3): the countdown was
    /// the hull's, the pilot is in the capsule now, so the run ends with
    /// `SelfDestructCancelledExternal` and the hull is not blown up.
    #[test]
    fn eject_cancels_a_running_self_destruct() {
        let mut reg = world();
        let now = crate::pyutil::filetime_now();
        crate::self_destruct::start(
            &mut reg,
            Some(ALICE),
            Args::of(&[PyValue::Int(ALICE_SHIP)]),
            now,
        )
        .expect("starts");
        assert!(crate::self_destruct::is_running(&reg, ALICE_SHIP));
        let _ = reg.take_pending();
        eject_it(&mut reg, ALICE).expect("ejected");
        crate::self_destruct::tick(&mut reg, Instant::now(), now + 10_000_000);
        assert!(!crate::self_destruct::is_running(&reg, ALICE_SHIP));
        // The tick's message is on the clock's queue.
        let told = reg
            .take_due(Instant::now() + std::time::Duration::from_secs(3_600))
            .into_iter()
            .any(|p| {
                let (name, args) = p.to_push();
                name == "OnRemoteMessage"
                    && args.first().and_then(crate::pyutil::as_bytes)
                        == Some(b"SelfDestructCancelledExternal".as_slice())
            });
        assert!(told, "the pilot is told it was cancelled");
        assert!(reg.item(ALICE_SHIP).is_some(), "the hull is not destroyed");
    }
}
