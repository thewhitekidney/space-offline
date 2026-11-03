//! **Fleet warp: a commander's warp takes the fleet along** (SOC-C7).
//!
//! Lane note: `docs/lanes/soc-c7.md`. The client has one call for it, the
//! warp every pilot makes, with a keyword:
//!
//! - `menusvc.WarpFleet(targetID, warpRange)` (`menusvc.py:3250`) is
//!   `bp.CmdWarpToStuff('item', targetID, minRange=warpRange, fleet=True)`
//!   **[V]**; `WarpFleetToMember` is the same with `'char'` and a character
//!   id, and `movementFunctions.WarpFleetToBookmark` with `'bookmark'`
//!   **[V]**. `fleetbroadcastexports.GetWarpLocationMenu` offers the one
//!   `WarpFleet` under three labels -- fleet, wing, squad -- by
//!   `session.fleetrole` **[V]**, so the **server** decides who follows from
//!   the commander's seat **[I]**.
//!
//! So `CmdWarpToStuff` warps the caller as before, and then [`follow`] warps
//! each follower to the same target with the same `minRange`, as if each
//! had sent the call: the same resolvers, the same movement guards (a
//! scrambled member stays), the same pushes. Who follows is [`followers`],
//! pure over the fleet:
//!
//! - the boss or the fleet commander: every member; a wing commander: the
//!   wing; a squad commander: the squad; anybody else: nobody **[I]**;
//! - only members whose `memberOptOuts.acceptsFleetWarp` is on (the client's
//!   own opt-out, `evefleet/members.GetNewMemberOptOutsKeyval` **[V]**);
//! - only members in space in the commander's system ([`follow`]): a fleet
//!   warp does not cross systems or undock anybody **[A]** (falsified by a
//!   member in another system being moved);
//! - never the member being warped to (`'char'`).
//!
//! **A fleet warp to the commander himself is a regroup** [A]: the
//! commander does not move ([`is_to_self`]) and the fleet warps to him. The
//! client's own `beyonce.CmdFleetRegroup()` (`fleetSvc.py:916` [V]) is not
//! answered here; falsified by a client whose "Warp Fleet to Member" on
//! oneself is refused or moves the commander.
//!
//! Registration is one hunk in `service/space.rs`'s `CmdWarpToStuff` arm
//! (coordinator diff): the warp itself stays private to that module and is
//! handed in as a closure.

use crate::fleet::{Fleet, Member, consts};
use crate::service::{Registry, Request};
use eve_marshal::PyValue;

/// The keyword that makes a warp a fleet warp **[V]** (`menusvc.WarpFleet`).
pub const FLEET_KW: &str = "fleet";

/// `CmdWarpToStuff`'s subject for a character **[V]**.
const SUBJECT_CHAR: &[u8] = crate::space_state::WARP_SUBJECT_CHAR;

/// Whether the call asked for a fleet warp: a truthy `fleet=` keyword.
pub fn is_fleet_warp(req: &Request<'_>) -> bool {
    match crate::space_state::kwarg(req.kwargs, FLEET_KW) {
        Some(PyValue::Bool(b)) => *b,
        Some(PyValue::Int(n)) => *n != 0,
        _ => false,
    }
}

/// The warp's destination as a plain id, when it is one.
fn destination(req: &Request<'_>) -> Option<i64> {
    crate::pyutil::opt_id(req.args.get(1))
}

/// **A fleet warp to the commander's own character: the regroup.** The
/// commander stays where he is (module header, [A]).
pub fn is_to_self(req: &Request<'_>, subject: &[u8]) -> bool {
    is_fleet_warp(req)
        && subject == SUBJECT_CHAR
        && req.actor.is_some()
        && destination(req) == req.actor
}

/// **Who follows `commander`'s fleet warp**, by seat (module header). Pure;
/// the commander is never in the answer. Sorted by character id.
pub fn followers(fleet: &Fleet, commander: i64) -> Vec<i64> {
    let Some(me) = fleet.members.get(&commander) else {
        return Vec::new();
    };
    let in_scope = |m: &Member| -> bool {
        if me.is_boss() || me.role == consts::ROLE_LEADER {
            true
        } else if me.role == consts::ROLE_WING_CMDR {
            m.wing_id == me.wing_id
        } else if me.role == consts::ROLE_SQUAD_CMDR {
            m.squad_id == me.squad_id
        } else {
            false
        }
    };
    fleet
        .members
        .values()
        .filter(|m| m.char_id != commander)
        .filter(|m| m.opt_outs.accepts_fleet_warp)
        .filter(|m| in_scope(m))
        .map(|m| m.char_id)
        .collect()
}

/// **Warp the followers**, after the commander's own warp was accepted.
/// `warp` is `service::space::warp` for one pilot's request. Returns how
/// many followers were sent; a follower whose warp a guard refused is not
/// counted and not an error (he stays, as a scrambled pilot does).
pub fn follow(
    reg: &mut Registry,
    req: &Request<'_>,
    subject: &[u8],
    warp: impl FnMut(&mut Registry, &Request<'_>) -> Option<crate::movement_guard::Refusal>,
) -> usize {
    follow_if(reg, req, subject, is_fleet_warp(req), warp)
}

/// [`follow`] with the fleet flag given: `agentMgr.WarpToLocation` carries it
/// as its positional 3, not as a keyword (FLEET-MISSION).
pub fn follow_if(
    reg: &mut Registry,
    req: &Request<'_>,
    subject: &[u8],
    fleet: bool,
    mut warp: impl FnMut(&mut Registry, &Request<'_>) -> Option<crate::movement_guard::Refusal>,
) -> usize {
    let Some(commander) = req.actor else {
        return 0;
    };
    if !fleet {
        return 0;
    }
    let Some(fleet) = reg.fleets().fleet_of(commander) else {
        return 0;
    };
    let mut who = followers(fleet, commander);
    let Some(system) = reg.roster().get(commander).map(|p| p.solar_system_id) else {
        return 0;
    };
    if subject == SUBJECT_CHAR
        && let Some(target) = destination(req)
    {
        who.retain(|&c| c != target);
    }
    who.retain(|&c| {
        reg.roster()
            .get(c)
            .is_some_and(|p| p.solar_system_id == system)
    });
    // `fleet=` is dropped: a follower's warp is his own, and a follower who
    // is a commander does not take *his* wing along a second time.
    let kwargs: Vec<(PyValue, PyValue)> = req
        .kwargs
        .iter()
        .filter(|(_, k)| crate::pyutil::as_bytes(k) != Some(FLEET_KW.as_bytes()))
        .cloned()
        .collect();
    let mut sent = 0;
    for member in who {
        let one = Request {
            service: req.service,
            method: req.method,
            args: req.args,
            kwargs: &kwargs,
            bind_params: None,
            actor: Some(member),
        };
        if warp(reg, &one).is_none() {
            sent += 1;
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::Fleets;

    const BOSS: i64 = 1_000_001;
    const ANN: i64 = 1_000_002;
    const BEN: i64 = 1_000_003;
    const CAT: i64 = 1_000_004;
    const SHIP: i64 = 670;

    /// A fleet of four: the boss and three members in the default squad.
    fn fleet() -> (Fleets, i64) {
        let mut f = Fleets::new(1);
        let id = f.create(BOSS).expect("created");
        f.init(BOSS, id, Some(SHIP), None, None).expect("init");
        for who in [ANN, BEN, CAT] {
            f.invite(BOSS, id, who, None, None, None).expect("invited");
            f.accept_invite(who, id, Some(SHIP), None, None)
                .expect("accepted");
        }
        (f, id)
    }

    #[test]
    fn the_boss_takes_every_member_and_a_member_takes_nobody() {
        let (f, id) = fleet();
        let fleet = f.fleet(id).expect("a fleet");
        assert_eq!(followers(fleet, BOSS), vec![ANN, BEN, CAT]);
        assert!(
            followers(fleet, ANN).is_empty(),
            "a plain member commands nobody"
        );
        assert!(followers(fleet, 9).is_empty(), "nor does a stranger");
    }

    #[test]
    fn a_member_who_opted_out_of_fleet_warp_stays() {
        let (mut f, id) = fleet();
        f.set_opt_out(BEN, id, "SetTakesFleetWarp", false)
            .expect("opted out");
        let fleet = f.fleet(id).expect("a fleet");
        assert_eq!(followers(fleet, BOSS), vec![ANN, CAT]);
    }

    #[test]
    fn only_a_truthy_fleet_keyword_is_a_fleet_warp() {
        let args = [PyValue::Bytes(b"char".to_vec()), PyValue::Int(BOSS)];
        let fleet_kw = [(PyValue::Bool(true), PyValue::Bytes(b"fleet".to_vec()))];
        let plain_kw = [(PyValue::Real(0.0), PyValue::Bytes(b"minRange".to_vec()))];
        let r = Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &args,
            kwargs: &fleet_kw,
            bind_params: None,
            actor: Some(BOSS),
        };
        assert!(is_fleet_warp(&r));
        assert!(
            is_to_self(&r, b"char"),
            "a fleet warp to oneself is the regroup"
        );
        assert!(!is_to_self(&r, b"item"));
        let plain = Request {
            kwargs: &plain_kw,
            ..r
        };
        assert!(!is_fleet_warp(&plain));
        assert!(!is_to_self(&plain, b"char"));
        let other = Request {
            actor: Some(ANN),
            ..r
        };
        assert!(
            !is_to_self(&other, b"char"),
            "warping the fleet to the boss is no regroup for Ann"
        );
    }
}
