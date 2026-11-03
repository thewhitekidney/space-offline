//! **Abandoning and scooping squadrons** (FT-G, `docs/lanes/ft-gh.md`; spec
//! R-DEEP §2.4, §2.7).
//!
//! `fighterMgr.CmdAbandonFighter(fighterID)` and
//! `fighterMgr.CmdScoopAbandonedFighterFromSpace(fighterID, toFlagID)`; the
//! client discards both answers and shows a `UserError` raised by the second
//! (`fightersSvc.py:211-215`; the caller guards `try/except UserError`) [V].
//!
//! # Abandon
//!
//! The pilot confirms (`ConfirmAbandonFighter`) and the client calls the
//! server. **The squadron stays in space**: its ball, its item (still in the
//! solar system) and its `ownerID` stay; only the controller goes. The client
//! learns it as it learns every loss of control, `OnFighterRemovedFromController
//! (fighterID, tubeFlagID)`, whose handler empties the tube unless it is
//! landing or refuelling (`shipFighterState.py:118-126`) [V]. From then on
//! nobody commands it: [`super::Squadron::ship_id`] and `tube` are `0`, so
//! every "is this my ship's squadron" test already says no, and the pilot's
//! ability buttons and holds end with it.
//!
//! **A squadron nobody controls can be taken by anybody**: the client offers
//! "Scoop" for any fighter in space that is not in the pilot's own squadron
//! window, with no owner test (`celestialCheckers.IsScoopable`, `:1952`)
//! [V]; the three destinations are `flagCargo`, `flagFleetHangar` and
//! `flagFighterBay` (`menusvc.RealScoop`, `RealScoopToFleetHangar`,
//! `RealScoopToFighterBay`) [V].
//!
//! # Scoop
//!
//! Refused: a squadron that is controlled (`CannotScoopControlledFighter`
//! [V key]); one further than `fighters.MAX_SCOOP_DISTANCE` (5,000 m [V
//! `fighters/__init__.py:26`]) from the scooping ship; a destination without
//! room. The item moves into the scooping pilot's ship **whole**, at its
//! current size, and the pilot becomes its owner. The ball leaves every park.
//!
//! # `[A]`
//!
//! 1. **An abandoned squadron despawns after [`ABANDONED_LIFETIME_MS`]**
//!    (5 minutes), taken out of space and its item destroyed. R-DEEP §2.7
//!    states the figure as an assumption; drones are taken out after the same
//!    time (`drones::ABANDON_AFTER`, K-DRONE). The client has no clock for it.
//!    Falsifier: an abandoned squadron that stays for ever, or goes sooner.
//! 2. **An abandoned squadron stops** where it is and does nothing; the
//!    behaviour of a fighter nobody controls is server-side. Falsifier: a
//!    retail abandoned squadron that keeps orbiting or fights on.
//! 3. **No key says "too far"**: a text refusal is raised. The client moves
//!    the pilot into reach before it calls (`menusvc.GetCloseAndTryCommand`).
//! 4. **`flagFleetHangar` is refused**: this build does not model the fleet
//!    hangar as a place to scoop into.
//! 5. **Taking another pilot's squadron raises no theft or aggression.** The
//!    client has `crimewatchSvc.IsOkToScoopItem` for it; the server side is
//!    not read. Falsifier: scooping a stranger's squadron flags the scooper
//!    in the retail game.

use super::*;

/// How long an abandoned squadron lasts, ms **[A]** (module header).
pub const ABANDONED_LIFETIME_MS: i64 = 300_000;
/// `fighters.MAX_SCOOP_DISTANCE`, metres [V `fighters/__init__.py:26`].
pub const MAX_SCOOP_DISTANCE: f64 = 5_000.0;
/// `const.flagCargo` [V].
pub const FLAG_CARGO: i64 = crate::fitting::FLAG_CARGO;

/// `CmdAbandonFighter(fighterID)`: the pilot's ship lets a squadron go.
///
/// Refused as every command is: not a squadron of this ship
/// (`CannotControlFighterNotControlledByYourShip`), not `INSPACE`
/// (`FighterTubeIsBusy`: one launching, recalling or landing has a task to
/// finish). No range test: abandoning a squadron that has flown beyond reach
/// is what the command is for.
pub fn abandon(w: &mut Registry, actor: Option<i64>, id: i64) -> Result<(), Refusal> {
    let (_owner, ship, _) = pilot_ship(w, actor)?;
    let s = state(w)
        .squadron(id)
        .copied()
        .filter(|s| s.ship_id == ship && ship != 0)
        .ok_or_else(|| Refusal::key("CannotControlFighterNotControlledByYourShip"))?;
    if state(w).task(ship, s.tube).map(|t| t.status) != Some(Status::InSpace) {
        return Err(Refusal::key("FighterTubeIsBusy"));
    }
    let now = crate::pyutil::filetime_now();
    w.advance_system(s.solar_system_id);
    // Its abilities end (the FX off, the buttons told), its holds on other
    // ships go, a kamikaze run is off.
    for slot in 0..3 {
        if let Some(a) = ability::active_in(w, id, slot) {
            ability::end(w, &s, slot, &a, false);
        }
    }
    effects::source_gone(w, id);
    strike::forget(w, id);
    if let Some(r) = state_mut(w).rt.get_mut(&id) {
        r.order = ability::Order::Stop;
    }
    // It stops where it is.
    let stop = Action::Stop { ball: id };
    if let Some(p) = w.roster_mut().get_mut(id) {
        p.motion.after(stop);
    }
    send_actions(w, Via::Clock, s.solar_system_id, vec![stop]);
    // Out of the carrier's books.
    state_mut(w).tasks.remove(&(ship, s.tube));
    state_mut(w).live.insert(
        id,
        Squadron {
            ship_id: 0,
            tube: 0,
            ..s
        },
    );
    state_mut(w).abandoned.insert(id, now);
    tell(
        w,
        Via::Clock,
        s.owner_id,
        Kind::Removed {
            fighter: id,
            tube: s.tube,
        },
    );
    Ok(())
}

/// `CmdScoopAbandonedFighterFromSpace(fighterID, toFlagID)`: the pilot's ship
/// takes an abandoned squadron aboard, into its cargo hold or its fighter bay.
pub fn scoop(w: &mut Registry, actor: Option<i64>, id: i64, flag: i64) -> Result<(), Refusal> {
    let (scooper, ship, _) = pilot_ship(w, actor)?;
    w.advance_character(scooper);
    let me = w
        .roster()
        .get(scooper)
        .copied()
        .filter(|p| p.ship_id == ship)
        .ok_or_else(|| Refusal::text("Fighters can only be scooped in space."))?;
    let s = state(w)
        .squadron(id)
        .copied()
        .ok_or_else(|| Refusal::text("That fighter is not in space."))?;
    if !state(w).is_abandoned(id) {
        return Err(Refusal::key("CannotScoopControlledFighter"));
    }
    if s.solar_system_id != me.solar_system_id {
        return Err(Refusal::text("That fighter is not in this system."));
    }
    w.advance_system(s.solar_system_id);
    let at = position_in(w, id, s.solar_system_id)
        .ok_or_else(|| Refusal::text("That fighter is not in space."))?;
    let me = w.roster().get(scooper).copied().unwrap_or(me);
    if crate::space_state::distance(me.motion.position, at) > MAX_SCOOP_DISTANCE {
        return Err(Refusal::text("That fighter is too far away to scoop."));
    }
    let unit = volume_of(w, s.type_id);
    let volume = unit * s.size as f64;
    match flag {
        FLAG_FIGHTER_BAY => {
            if !skills::ship_slots(w, ship).has_bay() {
                return Err(Refusal::key("ShipHasNoFighterBay"));
            }
            let free = (bay_capacity(w, ship) - bay_used(w, ship)).max(0.0);
            if crate::fitting::units_that_fit(free, unit) < s.size {
                return Err(no_room(free, volume));
            }
        }
        FLAG_CARGO => {
            let free = crate::fitting::cargo_free_in(w, ship);
            if crate::fitting::units_that_fit(free, unit) < s.size {
                return Err(Refusal::with(
                    "NotEnoughCargoSpace",
                    vec![
                        ("available", PyValue::Real(free)),
                        ("volume", PyValue::Real(volume)),
                    ],
                ));
            }
        }
        _ => {
            return Err(Refusal::text(
                "That squadron can be scooped into the cargo hold or the fighter bay.",
            ));
        }
    }
    let to = Slot::new(scooper, ship, flag);
    let (_, held) = apply_held(w, &[Edit::Move { item_id: id, to }])
        .ok_or_else(|| Refusal::text("That squadron could not be scooped."))?;
    // Out of space, as a drone's scoop: the ball out of every park, the books.
    let park = audience(w, s.solar_system_id);
    ability::drop_runtime(w, id);
    w.roster_mut().leave(id);
    state_mut(w).live.remove(&id);
    state_mut(w).abandoned.remove(&id);
    let stamp = w.next_destiny_stamp();
    if let Some(removed) = destiny::BallsRemoved::new(park, stamp, vec![id]) {
        send(w, Via::Answer, removed);
    }
    release(w, Via::Answer, scooper, held);
    Ok(())
}

/// The clock's part: an abandoned squadron that has lain [`ABANDONED_LIFETIME_MS`]
/// goes ([`ability::explode`] without a controller event: its item destroyed,
/// the ball out with an explosion).
pub(super) fn tick(w: &mut Registry, now: BlueTime) {
    let due: Vec<i64> = state(w)
        .abandoned
        .iter()
        .filter(|(_, since)| now.saturating_sub(**since) >= ms(ABANDONED_LIFETIME_MS))
        .map(|(id, _)| *id)
        .collect();
    for id in due {
        let Some(s) = state(w).squadron(id).copied() else {
            state_mut(w).abandoned.remove(&id);
            continue;
        };
        let park = audience(w, s.solar_system_id);
        ability::explode(w, &s, park);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fighters::ability::tests as at;
    use crate::fighters::tests as ft;
    use crate::fighters::tests::{ALICE, ALICE_SHIP, BAY, BOB, BOB_SHIP, SYSTEM};

    const SQ: i64 = 6;

    fn call_as(
        reg: &mut Registry,
        who: i64,
        method: &str,
        args: &[PyValue],
    ) -> Result<PyValue, ServiceError> {
        reg.dispatch(&Request {
            service: "fighterMgr",
            method,
            args,
            kwargs: &[],
            bind_params: None,
            actor: Some(who),
        })
    }

    fn abandoned() -> (Registry, i64, BlueTime) {
        let (mut reg, ids, t0) = at::fly(&[(at::ATTACKER, SQ)]);
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        let sq = ids[0];
        call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)]).expect("abandoned");
        (reg, sq, t0)
    }

    fn position(reg: &Registry, id: i64) -> [f64; 3] {
        reg.roster().get(id).expect("in space").motion.position
    }

    #[test]
    fn abandon_keeps_ball_drops_controller() {
        let (mut reg, ids, t0) = at::fly(&[(at::ATTACKER, SQ)]);
        let sq = ids[0];
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        assert!(call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)]).is_ok());
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        // The controller event, with the tube the client empties.
        let removed = ft::named(&sent, "OnFighterRemovedFromController");
        assert_eq!(removed.len(), 1, "{sent:?}");
        assert_eq!(
            removed[0].2,
            vec![PyValue::Int(sq), PyValue::Int(FIRST_TUBE)]
        );
        assert_eq!(removed[0].0, Some(vec![ALICE]));
        // The ball and the item stay, the owner stays.
        assert!(reg.roster().get(sq).is_some(), "the ball stays");
        let item = reg.store().get(sq).expect("the item stays");
        assert_eq!((item.location_id, item.owner_id), (SYSTEM, ALICE));
        assert_eq!(item.quantity, SQ);
        let s = state(&reg).squadron(sq).copied().expect("still a squadron");
        assert_eq!((s.ship_id, s.tube, s.owner_id), (0, 0, ALICE));
        assert!(state(&reg).is_abandoned(sq));
        // The carrier's books: no task, nothing in space for it.
        assert!(state(&reg).task(ALICE_SHIP, FIRST_TUBE).is_none());
        let PyValue::Tuple(parts) = ft::call(&mut reg, "GetFightersForShip", &[]).expect("rows")
        else {
            panic!("3-tuple")
        };
        assert_eq!(parts[1], PyValue::List(vec![]), "no squadron in space");
        // Nobody commands it, and it does not come home by itself.
        let a = at::activate_call(&mut reg, &[sq], 0, &[]);
        assert_eq!(
            at::refusal_for(&a, sq),
            "CannotControlFighterNotControlledByYourShip"
        );
        let r = ft::call(&mut reg, "RecallFightersToTubes", &[ft::ints(&[sq])]).expect("a dict");
        assert_eq!(
            at::refusal_for(&r, sq),
            "CannotControlFighterNotControlledByYourShip"
        );
        let e = at::move_call(&mut reg, &[sq], "STOP", &[]).expect_err("not controlled");
        assert_eq!(
            ft::key_of(&e),
            "CannotControlFighterNotControlledByYourShip"
        );
        tick(&mut reg, t0 + ms(ABANDONED_LIFETIME_MS / 2));
        assert!(state(&reg).squadron(sq).is_some(), "it lies there");
        assert_eq!(reg.store().get(sq).expect("item").location_id, SYSTEM);
    }

    #[test]
    fn an_abandoned_squadron_stops_where_it_is() {
        let (mut reg, sq, _) = abandoned();
        let a = position(&reg, sq);
        std::thread::sleep(std::time::Duration::from_millis(30));
        reg.advance_system(SYSTEM);
        assert_eq!(position(&reg, sq), a, "it does not orbit on");
    }

    #[test]
    fn abandoning_ends_what_the_squadron_was_doing() {
        let (mut reg, ids, t0) = at::fly(&[(at::ATTACKER, SQ)]);
        let sq = ids[0];
        let rat = at::rat_beyond(&mut reg, sq, 4_000.0);
        at::velocity_zero(&mut reg, rat);
        assert_eq!(
            at::activate_call(&mut reg, &ids, 0, &[PyValue::Int(rat)]),
            at::ok_for(&ids)
        );
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)]).expect("abandoned");
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        let off = ft::named(&sent, "OnFighterAbilitySlotDeactivated");
        assert_eq!(off.len(), 1, "the pilot's button goes off: {sent:?}");
        let names: Vec<&str> = sent.iter().map(|(_, n, _)| *n).collect();
        let at = |n: &str| names.iter().position(|x| *x == n).expect(n);
        assert!(
            at("OnFighterAbilitySlotDeactivated") < at("OnFighterRemovedFromController"),
            "{names:?}"
        );
        assert!(
            super::super::ability::active_in(&reg, sq, 0).is_none(),
            "no ability left running"
        );
        let hurt = reg.combat().ship(rat).map(|r| r.shield_at(t0 + ms(60_000)));
        tick(&mut reg, t0 + ms(60_000));
        assert!(
            ft::named(&ft::due(&mut reg), "OnDamageMessage").is_empty(),
            "no more volleys"
        );
        let _ = hurt;
    }

    #[test]
    fn abandon_refuses_what_is_not_the_pilots_or_not_flying_yet() {
        let (mut reg, ids, _) = at::fly_with_bob(&[(at::ATTACKER, SQ)]);
        // A stranger's: Bob has no such squadron.
        let e = call_as(&mut reg, BOB, "CmdAbandonFighter", &[PyValue::Int(ids[0])])
            .expect_err("not his");
        assert_eq!(
            ft::key_of(&e),
            "CannotControlFighterNotControlledByYourShip"
        );
        let e = call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(1)])
            .expect_err("no such squadron");
        assert_eq!(
            ft::key_of(&e),
            "CannotControlFighterNotControlledByYourShip"
        );
        // One that is still launching has a task to finish.
        let mut reg = ft::world(ft::CARRIER, &[(at::ATTACKER, SQ)]);
        ft::undock(&mut reg);
        ft::load_to(&mut reg, BAY, FIRST_TUBE).expect("loaded");
        ft::settle(&mut reg);
        let sq = tube_item(&reg, ALICE_SHIP, FIRST_TUBE).expect("loaded").0;
        ft::call(
            &mut reg,
            "LaunchFightersFromTubes",
            &[ft::ints(&[FIRST_TUBE])],
        )
        .expect("a dict");
        let e = call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)])
            .expect_err("launching");
        assert_eq!(ft::key_of(&e), "FighterTubeIsBusy");
        assert!(!state(&reg).is_abandoned(sq));
    }

    #[test]
    fn scoop_refused_when_controlled_or_too_far() {
        let (mut reg, ids, _) = at::fly(&[(at::ATTACKER, SQ)]);
        let sq = ids[0];
        // Under control: refused with the client's key.
        let e = call_as(
            &mut reg,
            ALICE,
            "CmdScoopAbandonedFighterFromSpace",
            &[PyValue::Int(sq), PyValue::Int(FLAG_FIGHTER_BAY)],
        )
        .expect_err("controlled");
        assert_eq!(ft::key_of(&e), "CannotScoopControlledFighter");
        // Abandoned, but far: refused.
        call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)]).expect("abandoned");
        let at_sq = position(&reg, sq);
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.position = [at_sq[0] + MAX_SCOOP_DISTANCE + 1_000.0, at_sq[1], at_sq[2]];
            p.motion.velocity = [0.0; 3];
        }
        let e = call_as(
            &mut reg,
            ALICE,
            "CmdScoopAbandonedFighterFromSpace",
            &[PyValue::Int(sq), PyValue::Int(FLAG_FIGHTER_BAY)],
        )
        .expect_err("too far");
        assert_eq!(ft::key_of(&e), "CustomNotify");
        assert!(state(&reg).squadron(sq).is_some(), "nothing was taken");
        // A flag nobody scoops a squadron into.
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.position = [at_sq[0] + 500.0, at_sq[1], at_sq[2]];
        }
        let e = call_as(
            &mut reg,
            ALICE,
            "CmdScoopAbandonedFighterFromSpace",
            &[PyValue::Int(sq), PyValue::Int(4)],
        )
        .expect_err("a hangar flag");
        assert_eq!(ft::key_of(&e), "CustomNotify");
    }

    #[test]
    fn scoop_takes_an_abandoned_squadron_into_the_fighter_bay() {
        let (mut reg, sq, _) = abandoned();
        let at_sq = position(&reg, sq);
        if let Some(p) = reg.roster_mut().get_mut(ALICE) {
            p.motion.position = [at_sq[0] + MAX_SCOOP_DISTANCE - 100.0, at_sq[1], at_sq[2]];
            p.motion.velocity = [0.0; 3];
        }
        let _ = ft::pending(&mut reg);
        call_as(
            &mut reg,
            ALICE,
            "CmdScoopAbandonedFighterFromSpace",
            &[PyValue::Int(sq), PyValue::Int(FLAG_FIGHTER_BAY)],
        )
        .expect("scooped, just inside the range");
        let item = reg.store().get(sq).expect("the item is aboard");
        assert_eq!(
            (item.location_id, item.flag_id, item.owner_id, item.quantity),
            (ALICE_SHIP, FLAG_FIGHTER_BAY, ALICE, SQ)
        );
        assert!(reg.roster().get(sq).is_none(), "the ball is gone");
        assert!(state(&reg).squadron(sq).is_none());
        assert!(!state(&reg).is_abandoned(sq));
        let sent = ft::pending(&mut reg);
        assert!(
            !ft::named(&sent, "DoDestinyUpdate").is_empty(),
            "RemoveBalls"
        );
        assert!(!ft::named(&sent, "OnItemChange").is_empty());
        // It counts against the bay, and it can be loaded and launched again.
        let want = SQ as f64 * volume_of(&reg, at::ATTACKER);
        assert!((bay_used(&reg, ALICE_SHIP) - want).abs() < 1e-9);
        ft::load_to(&mut reg, sq, FIRST_TUBE).expect("loadable again");
    }

    /// **Anybody may take it**: Bob's ship takes Alice's abandoned squadron
    /// into his cargo hold and owns it.
    #[test]
    fn another_pilot_scoops_an_abandoned_squadron_into_his_cargo_hold() {
        let (mut reg, ids, _) = at::fly_with_bob(&[(at::ATTACKER, SQ)]);
        let sq = ids[0];
        call_as(&mut reg, ALICE, "CmdAbandonFighter", &[PyValue::Int(sq)]).expect("abandoned");
        // Bob lies 3 km along x: in reach.
        call_as(
            &mut reg,
            BOB,
            "CmdScoopAbandonedFighterFromSpace",
            &[PyValue::Int(sq), PyValue::Int(FLAG_CARGO)],
        )
        .expect("a stranger's squadron, taken");
        let item = reg.store().get(sq).expect("aboard");
        assert_eq!(
            (item.location_id, item.flag_id, item.owner_id),
            (BOB_SHIP, FLAG_CARGO, BOB)
        );
        assert!(reg.roster().get(sq).is_none());
        // Alice's books have nothing of it.
        assert!(state(&reg).squadron(sq).is_none());
    }

    #[test]
    fn an_abandoned_squadron_despawns_after_its_lifetime() {
        let (mut reg, sq, _) = abandoned();
        let since = *state(&reg).abandoned.get(&sq).expect("abandoned");
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        tick(&mut reg, since + ms(ABANDONED_LIFETIME_MS - 1));
        assert!(state(&reg).squadron(sq).is_some(), "not yet");
        tick(&mut reg, since + ms(ABANDONED_LIFETIME_MS));
        assert!(state(&reg).squadron(sq).is_none(), "gone");
        assert!(reg.roster().get(sq).is_none());
        assert!(reg.store().get(sq).is_none(), "its item is destroyed");
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        assert!(
            ft::named(&sent, "OnFighterRemovedFromController").is_empty(),
            "the client has forgotten it: its handler would raise"
        );
        // The explosion, and the item's destruction (its location's cache).
        assert!(!ft::named(&sent, "DoDestinyUpdate").is_empty());
        assert!(!ft::named(&sent, "OnInvalidateCacheForLocations").is_empty());
    }

    #[test]
    fn a_shot_abandoned_squadron_tells_the_pilot_nothing() {
        let (mut reg, sq, _) = abandoned();
        let _ = ft::pending(&mut reg);
        let _ = ft::due(&mut reg);
        let park = audience(&reg, SYSTEM);
        ability::destroyed(&mut reg, sq, park.clone());
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        assert!(
            ft::named(&sent, "OnInSpaceSquadronSizeChanged").is_empty(),
            "{sent:?}"
        );
        assert_eq!(state(&reg).squadron(sq).map(|s| s.size), Some(SQ - 1));
        // Down to the last one.
        for _ in 0..SQ {
            if state(&reg).squadron(sq).is_none() {
                break;
            }
            ability::destroyed(&mut reg, sq, park.clone());
        }
        assert!(state(&reg).squadron(sq).is_none());
        let mut sent = ft::pending(&mut reg);
        sent.extend(ft::due(&mut reg));
        assert!(ft::named(&sent, "OnFighterRemovedFromController").is_empty());
        assert!(reg.store().get(sq).is_none());
        assert!(!state(&reg).is_abandoned(sq));
    }
}
