//! **Undock and arrival invulnerability** (lane `undock-invuln`).
//!
//! A ship that has just come out of a station, or through a gate, cannot be
//! hurt until it does something of its own or a timer runs out. Before this
//! the server had none: a pilot undocking into a belt with rats was shot
//! at once, and so was a bot's fresh arrival.
//!
//! ## What the client says **[V]**
//!
//! `crimewatchTimers.TimerContainer` (`eve/client/script/ui/crimewatch/`):
//!
//! - `OnInvulunOnUndockingUpdated(shipID, endTime, duration)` (sic, the
//!   client's spelling) starts the HUD's **Invulnerable** timer
//!   (`TimerType.Invulnerable`, tooltip `UI/Crimewatch/Timers/InvulTooltip`,
//!   read against `gametime.GetSimTime`, `DeleteWhenFinished`);
//!   `OnRestoringInvulnUpdated` is the same call for a restore after a
//!   session change, and does the same thing.
//! - `OnInvulnCancelled(shipID)` deletes that timer.
//! - The client never ends the timer itself on an action; the *server* says
//!   when protection is over, so [`break_for`] and [`tick`] both send the
//!   cancel.
//!
//! The gate cloak is a **different** mechanism: `OnJumpCloakUpdated` /
//! `OnJumpCloakCancelled`, a `JumpCloak` timer and a Destiny cloak mode. It
//! lives in `cloak.rs` (CLOAK-2) and is what a gate, wormhole or jump-drive
//! arrival gets *instead of* this; an undock and a restore keep this.
//!
//! ## What the client does not say, **[A]**
//!
//! Every number and trigger below is from EvEJS (`space/transitions.js`,
//! `space/runtime/timedInvulnerability.js`), which is a hint and not a fact:
//!
//! - the length is [`DURATION`], 30 s, which is **[V]** since the client's
//!   `appConst.invulnerabilityUndocking` is 30000 ms (the lane `login-space`
//!   read it); a restore after a login in space is [`RESTORING`], 60 s
//!   (`invulnerabilityRestoring` 60000) **[V]**;
//! - it breaks on **movement** (any `Cmd*` movement order and a warp), on
//!   **locking** a target, and on activating an **offensive** module
//!   ("EvEJS does the same three triggers"); falsified by a retail pilot who
//!   keeps the timer after one of them;
//! - a gate arrival used to get the same [`DURATION`]; since CLOAK-2 it gets
//!   the gate cloak (`cloak::grant_jump`) and this is the undock's alone.
//!
//! ## What invulnerable means here
//!
//! No volley lands (`combat::fire` returns first: no damage, no aggression,
//! no combat-log line); no electronic warfare or tackle counts (`ewar::held`,
//! `tackle::sources_of`, `npc_ewar::sources`); NPCs do not see the ship as a
//! candidate (`npc::view_of`), so they neither lock nor chase it.

use crate::damage::BlueTime;
use crate::notify::Push;
use crate::service::Registry;
use crate::world_ext::{Boxed, SlotSpec};
use eve_marshal::PyValue;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const SECOND: BlueTime = 10_000_000;

/// How long the protection lasts after an undock or a gate. **[V]** 30 s
/// (`appConst.invulnerabilityUndocking`, 30000 ms).
pub const DURATION: BlueTime = 30 * SECOND;

/// How long a ship restored at its log-off spot is protected. **[V]** 60 s
/// (`appConst.invulnerabilityRestoring`, 60000 ms). The same break triggers
/// as [`DURATION`]'s are **[A]**; falsified by a retail pilot who keeps the
/// timer through movement.
pub const RESTORING: BlueTime = 60 * SECOND;

/// One pilot's protection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grant {
    /// The ship it covers: a pilot who changes ship (a pod, a new hull) is
    /// not covered by the old one's grant.
    pub ship_id: i64,
    pub until: BlueTime,
}

#[derive(Default)]
pub struct State {
    /// Off until [`set_enabled`] (the binary turns it on; fixtures do not).
    enabled: bool,
    /// By character.
    held: BTreeMap<i64, Grant>,
}

/// The slot, for `world_ext::SLOTS` (coordinator).
pub const SLOT: SlotSpec = SlotSpec {
    name: "invuln",
    make,
};

fn make(_: &crate::content::Content) -> Boxed {
    Box::new(State::default())
}

/// `--no-undock-invuln` / the binary's default (`server::Config::undock_invuln`).
pub fn set_enabled(w: &mut Registry, on: bool) {
    w.ext_mut::<State>().enabled = on;
}

/// `OnInvulunOnUndockingUpdated(shipID, endTime, duration)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub character: i64,
    pub ship_id: i64,
    pub until: BlueTime,
    pub duration: BlueTime,
    /// `OnRestoringInvulnUpdated` instead of the undock's name: the client
    /// starts the same timer for both.
    pub restoring: bool,
}

impl Push for Started {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        (
            if self.restoring {
                "OnRestoringInvulnUpdated"
            } else {
                "OnInvulunOnUndockingUpdated"
            },
            vec![
                PyValue::Int(self.ship_id),
                PyValue::Int(self.until),
                PyValue::Int(self.duration),
            ],
        )
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// `OnInvulnCancelled(shipID)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cancelled {
    pub character: i64,
    pub ship_id: i64,
}

impl Push for Cancelled {
    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        ("OnInvulnCancelled", vec![PyValue::Int(self.ship_id)])
    }
    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.character])
    }
}

/// Protect `character`'s `ship_id` for [`DURATION`] from `now`, and tell the
/// pilot.
pub fn grant(w: &mut Registry, character: i64, ship_id: i64, now: BlueTime) {
    grant_for(w, character, ship_id, now, DURATION, false);
}

/// [`grant`] for a ship restored at its log-off spot: [`RESTORING`], and the
/// client's `OnRestoringInvulnUpdated`.
pub fn grant_restoring(w: &mut Registry, character: i64, ship_id: i64, now: BlueTime) {
    grant_for(w, character, ship_id, now, RESTORING, true);
}

fn grant_for(
    w: &mut Registry,
    character: i64,
    ship_id: i64,
    now: BlueTime,
    duration: BlueTime,
    restoring: bool,
) {
    if !w.ext::<State>().enabled {
        return;
    }
    let until = now.saturating_add(duration);
    w.ext_mut::<State>()
        .held
        .insert(character, Grant { ship_id, until });
    w.schedule(
        Duration::ZERO,
        Started {
            character,
            ship_id,
            until,
            duration,
            restoring,
        },
    );
}

/// Is the undock/arrival protection switched on? (The gate cloak follows it.)
pub fn enabled(w: &Registry) -> bool {
    w.ext::<State>().enabled
}

/// Is `ship_id` (a roster ship, any kind) protected at `now`?
pub fn ship_protected(w: &Registry, ship_id: i64, now: BlueTime) -> bool {
    let state = w.ext::<State>();
    if state.held.is_empty() {
        return false;
    }
    w.roster()
        .by_ship(ship_id)
        .is_some_and(|p| covers(state, p.character_id, p.ship_id, now))
}

/// Every grant held, by character (for a reader that must notice a change:
/// the NPC plan's staleness check).
pub fn grants(w: &Registry) -> Vec<(i64, Grant)> {
    w.ext::<State>()
        .held
        .iter()
        .map(|(c, g)| (*c, *g))
        .collect()
}

/// Is `character`'s current ship protected at `now`?
pub fn character_protected(w: &Registry, character: i64, now: BlueTime) -> bool {
    let state = w.ext::<State>();
    if state.held.is_empty() {
        return false;
    }
    w.roster()
        .get(character)
        .is_some_and(|p| covers(state, character, p.ship_id, now))
}

fn covers(state: &State, character: i64, ship_id: i64, now: BlueTime) -> bool {
    state
        .held
        .get(&character)
        .is_some_and(|g| g.ship_id == ship_id && now < g.until)
}

/// The pilot did something that gives the protection up (a movement order,
/// a lock, an offensive module): end it and tell the client. `false` when
/// there was nothing to end.
pub fn break_for(w: &mut Registry, character: i64) -> bool {
    if w.ext::<State>().held.is_empty() {
        return false;
    }
    let Some(g) = w.ext_mut::<State>().held.remove(&character) else {
        return false;
    };
    w.schedule(
        Duration::ZERO,
        Cancelled {
            character,
            ship_id: g.ship_id,
        },
    );
    true
}

/// The world clock: end every grant whose time is up, or whose ship has left
/// space (a dock, a death, a jump), with the cancel the client waits for.
pub fn tick(w: &mut Registry, _: Instant, now: BlueTime) {
    if w.ext::<State>().held.is_empty() {
        return;
    }
    let ended: Vec<(i64, Grant)> = w
        .ext::<State>()
        .held
        .iter()
        .filter(|(c, g)| {
            now >= g.until || w.roster().get(**c).is_none_or(|p| p.ship_id != g.ship_id)
        })
        .map(|(c, g)| (*c, *g))
        .collect();
    for (character, g) in ended {
        w.ext_mut::<State>().held.remove(&character);
        w.schedule(
            Duration::ZERO,
            Cancelled {
                character,
                ship_id: g.ship_id,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: i64 = 91_001;
    const ALICE_SHIP: i64 = 92_001;
    const T0: BlueTime = 133_000_000_000_000_000;

    fn world() -> Registry {
        let mut reg = world_off();
        set_enabled(&mut reg, true);
        reg
    }

    fn world_off() -> Registry {
        let mut reg = unbound();
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let _ = reg.take_due(Instant::now() + Duration::from_secs(5));
        reg
    }

    /// Alice online, docked: her bind has not happened.
    fn unbound() -> Registry {
        let mut reg = crate::testkit::registry_with(|c| {
            c.characters = vec![crate::model::Character {
                character_id: ALICE,
                name: "Alice".to_owned(),
                ship_id: ALICE_SHIP,
                ..crate::model::Character::default()
            }];
        });
        reg.session_opened(crate::session_table::SessionEntry {
            client_id: 1,
            session_id: 1,
            user_id: 1,
            character_id: Some(ALICE),
        });
        reg
    }

    fn names(reg: &mut Registry) -> Vec<&'static str> {
        reg.take_due(Instant::now() + Duration::from_secs(5))
            .iter()
            .map(|p| p.to_push().0)
            .collect()
    }

    #[test]
    fn a_restoring_grant_runs_sixty_seconds_under_its_own_name() {
        let mut reg = world();
        grant_restoring(&mut reg, ALICE, ALICE_SHIP, T0);
        assert!(ship_protected(&reg, ALICE_SHIP, T0 + 59 * SECOND));
        assert!(!ship_protected(&reg, ALICE_SHIP, T0 + RESTORING));
        let due = reg.take_due(Instant::now() + Duration::from_secs(5));
        let (event, args) = due.last().expect("a push").to_push();
        assert_eq!(event, "OnRestoringInvulnUpdated");
        assert_eq!(
            args,
            vec![
                PyValue::Int(ALICE_SHIP),
                PyValue::Int(T0 + RESTORING),
                PyValue::Int(RESTORING)
            ]
        );
    }

    #[test]
    fn a_grant_protects_the_ship_and_tells_the_pilot() {
        let mut reg = world();
        grant(&mut reg, ALICE, ALICE_SHIP, T0);
        assert!(ship_protected(&reg, ALICE_SHIP, T0 + SECOND));
        assert!(character_protected(&reg, ALICE, T0 + SECOND));
        let due = reg.take_due(Instant::now() + Duration::from_secs(5));
        let (event, args) = due.last().expect("a push").to_push();
        assert_eq!(event, "OnInvulunOnUndockingUpdated");
        assert_eq!(
            args,
            vec![
                PyValue::Int(ALICE_SHIP),
                PyValue::Int(T0 + DURATION),
                PyValue::Int(DURATION)
            ]
        );
    }

    #[test]
    fn it_ends_with_the_timer() {
        let mut reg = world();
        grant(&mut reg, ALICE, ALICE_SHIP, T0);
        let _ = names(&mut reg);
        assert!(!ship_protected(&reg, ALICE_SHIP, T0 + DURATION));
        tick(&mut reg, Instant::now(), T0 + DURATION - 1);
        assert!(names(&mut reg).is_empty(), "not yet");
        tick(&mut reg, Instant::now(), T0 + DURATION);
        assert_eq!(names(&mut reg), vec!["OnInvulnCancelled"]);
        tick(&mut reg, Instant::now(), T0 + DURATION + SECOND);
        assert!(names(&mut reg).is_empty(), "once");
    }

    #[test]
    fn an_action_ends_it_and_says_so_once() {
        let mut reg = world();
        grant(&mut reg, ALICE, ALICE_SHIP, T0);
        let _ = names(&mut reg);
        assert!(break_for(&mut reg, ALICE));
        assert_eq!(names(&mut reg), vec!["OnInvulnCancelled"]);
        assert!(!break_for(&mut reg, ALICE));
        assert!(!ship_protected(&reg, ALICE_SHIP, T0 + SECOND));
    }

    #[test]
    fn another_ship_is_not_covered() {
        let mut reg = world();
        grant(&mut reg, ALICE, ALICE_SHIP + 1, T0);
        assert!(!ship_protected(&reg, ALICE_SHIP, T0 + SECOND));
    }

    #[test]
    fn leaving_space_ends_it_at_the_next_tick() {
        let mut reg = world();
        grant(&mut reg, ALICE, ALICE_SHIP, T0);
        let _ = names(&mut reg);
        reg.roster_mut().leave(ALICE);
        tick(&mut reg, Instant::now(), T0 + SECOND);
        assert_eq!(names(&mut reg), vec!["OnInvulnCancelled"]);
    }

    #[test]
    fn a_volley_does_nothing_to_a_protected_ship() {
        let mut reg = world();
        let victim = *reg.roster().get(ALICE).expect("in space");
        grant(&mut reg, ALICE, ALICE_SHIP, crate::pyutil::filetime_now());
        let shot = crate::damage::Shot {
            at: crate::pyutil::filetime_now(),
            attacker_ship_id: ALICE_SHIP + 50,
            weapon_item_id: 1,
            weapon_type_id: 2,
            target_ship_id: ALICE_SHIP,
            damage: crate::damage::Damage::default(),
            hit_quality: 3,
        };
        crate::combat::fire(
            &mut reg,
            shot,
            ALICE + 50,
            victim,
            crate::pyutil::filetime_now(),
        );
        assert!(reg.combat().ship(ALICE_SHIP).is_none(), "no record opened");
    }

    #[test]
    fn the_bind_into_space_grants_it_when_enabled_and_only_then() {
        let mut off = unbound();
        crate::destiny::on_ballpark_bind(&mut off, Some(ALICE));
        assert!(!character_protected(
            &off,
            ALICE,
            crate::pyutil::filetime_now()
        ));

        let mut on = unbound();
        set_enabled(&mut on, true);
        crate::destiny::on_ballpark_bind(&mut on, Some(ALICE));
        assert!(character_protected(
            &on,
            ALICE,
            crate::pyutil::filetime_now()
        ));
        let sent = names(&mut on);
        assert!(sent.contains(&"OnInvulunOnUndockingUpdated"), "{sent:?}");
    }

    #[test]
    fn flying_the_ship_ends_it() {
        let mut reg = unbound();
        set_enabled(&mut reg, true);
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = names(&mut reg);
        let req = crate::service::Request {
            service: "beyonce",
            method: "CmdSetSpeedFraction",
            args: &[PyValue::Real(1.0)],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        let _ = crate::service::space::dispatch(&mut reg, &req);
        assert!(!character_protected(
            &reg,
            ALICE,
            crate::pyutil::filetime_now()
        ));
        assert!(names(&mut reg).contains(&"OnInvulnCancelled"));
    }
}
