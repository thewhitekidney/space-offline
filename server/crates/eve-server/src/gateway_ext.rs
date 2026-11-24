//! The gateway's ordered answer chain (GW-OUT, `docs/lanes/gw-out.md`).
//!
//! `gateway::answer` used to name `gateway_plans::handle` in its own body, so
//! every lane that wanted the gateway to answer something edited gateway.rs.
//! It now consults [`CHAIN`] instead: each arm takes the request and says
//! `Some(reply)` for the types it owns and `None` for the rest, and the first
//! `Some` wins. A lane adds ONE line to [`CHAIN`] at landing, and its own
//! module holds the arm. What no arm claims falls through to `answer`'s own
//! match (the stubs).

use crate::gateway::{Ground, Reply};

/// One arm: `(type name, request body, pilot, ground)`.
pub type Arm = fn(&str, &[u8], i64, &Ground) -> Option<Reply>;

/// The arms, in the order they are asked. **Coordinator-only**, like
/// `world_ext::SLOTS`: a packet exports its arm, the coordinator adds the line.
pub const CHAIN: &[Arm] = &[
    // SKILL-PLANS: eve_public.character.skill.plan.* and the corp-plan 404/403.
    crate::gateway_plans::handle,
    // NES-BACKEND: the PLEX vault's balance and transaction log.
    crate::nes_vault::gateway,
    // SKINS: the applied SKIN of a ship (`cosmetic.ship.api`).
    crate::skins::gateway,
    // EM-3 (#207): ship emblem licences and logos (`entitlement.character`,
    // `cosmetic.ship.logo`).
    crate::emblem::gateway,
    // DG-2 (#202): the daily goals (`eve_public.dailygoal.api`).
    crate::daily_goals::gateway,
    // #205: the NPE career achievements (`eve_public.achievement`).
    crate::achievements::gateway,
    // SOV-FUEL (#218): the hub's reagent bay, `hub.fuel.api` Get and Add.
    crate::sov_hub_fuel::gateway,
    // SOV-RES (#78): the hub's power and workforce, planet and star definitions.
    crate::sov_resources::gateway,
    // SKY-1 (#81): skyhook state, Activate and Deactivate, above the stub.
    crate::skyhook::gateway,
    // SOV-UPG-1 (#79): hub upgrade definitions, install, online/offline, remove.
    crate::sov_upgrade::gateway,
    // MERC-1 (#83): a mercenary den's state, the pilot's dens and maximum.
    crate::merc_den::gateway,
    // SOV-WF (#219): workforce transfer between hubs, above the stub.
    crate::sov_workforce::gateway,
    // SOV-GW-0: honest sov refusals; keep LAST of the sov arms.
    crate::sov_gw_stub::gateway,
];

/// Ask `chain` in order; the first `Some` wins.
pub fn consult(chain: &[Arm], ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    chain.iter().find_map(|arm| arm(ty, body, character, g))
}

/// [`consult`] over [`CHAIN`]: what `gateway::answer` calls.
pub fn handle(ty: &str, body: &[u8], character: i64, g: &Ground) -> Option<Reply> {
    consult(CHAIN, ty, body, character, g)
}

/// The account (`session.userid`) that owns `character`, for the requests
/// that are per user, not per pilot (the PLEX vault). `None` for a pilot the
/// world does not know, one with no owner, and on a [`Ground::Local`] (no
/// characters there). Takes the world read lock briefly, like any gateway
/// read; never call it while holding the world.
pub fn user_of(g: &Ground, character: i64) -> Option<i64> {
    match g {
        Ground::Server(w) => crate::service::world_read_timed(w, "gateway_user_of")
            .character(character)
            .and_then(|c| c.owner_user_id),
        Ground::Local(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(status: u32) -> Reply {
        Reply {
            status,
            payload: None,
        }
    }

    fn none(_: &str, _: &[u8], _: i64, _: &Ground) -> Option<Reply> {
        None
    }
    fn only_a(ty: &str, _: &[u8], _: i64, _: &Ground) -> Option<Reply> {
        (ty == "a").then(|| r(201))
    }
    fn any_a(_: &str, _: &[u8], _: i64, _: &Ground) -> Option<Reply> {
        Some(r(202))
    }
    fn any_b(_: &str, _: &[u8], _: i64, _: &Ground) -> Option<Reply> {
        Some(r(203))
    }

    #[test]
    fn the_first_some_wins_and_nones_fall_through() {
        let g = Ground::local();
        let chain: &[Arm] = &[none, only_a, any_a, any_b];
        assert_eq!(consult(chain, "a", b"", 1, &g), Some(r(201)));
        // "x" is not only_a's: the next arm that answers is any_a, not any_b.
        assert_eq!(consult(chain, "x", b"", 1, &g), Some(r(202)));
        assert_eq!(consult(&[none, none], "a", b"", 1, &g), None);
        assert_eq!(consult(&[], "a", b"", 1, &g), None);
    }

    #[test]
    fn the_real_chain_still_serves_skill_plans_and_passes_the_rest() {
        let g = Ground::local();
        let hit = handle("eve_public.character.skill.plan.GetAllRequest", b"", 7, &g);
        assert!(hit.is_some_and(|r| r.status == 200));
        assert_eq!(
            handle("eve_public.invented.thing.GetRequest", b"", 7, &g),
            None
        );
        let corp = handle("eve_public.corporation.skill.plan.GetRequest", b"", 7, &g);
        assert_eq!(corp.map(|r| r.status), Some(404));
    }

    #[test]
    fn a_local_ground_has_no_owner() {
        assert_eq!(user_of(&Ground::local(), 5), None);
    }
}
