//! Rules that can refuse a warp or a stargate jump before it starts.
//!
//! W10.5 seam S12 (`docs/content/plan.md` §6.1). A packet that stops a ship
//! moving (a warp disruptor, a scrambled pilot, a gate's own refusal) exports
//! a [`Guard`] and the coordinator adds it to [`WARP_GUARDS`] or
//! [`JUMP_GUARDS`]. W16 CP57 added the first rows: `crate::tackle`'s warp
//! scram and its gate-scramble seam. W18 CP49 added `crate::npc_ewar`'s NPC
//! scram to the warp table only: a scram does not stop a gate jump
//! (`docs/content/R-TACKLE.md` §5).
//!
//! **Where they run.** `service/space.rs`'s warp (which the autopilot's warp
//! also reaches) checks [`WARP_GUARDS`] after `advance_character` and before
//! any state changes; a refusal is raised as its `UserError` (W16 CP57,
//! `docs/content/R-TACKLE.md` §3), with nothing pushed. The emergency warp of
//! a dropped pilot checks the same table and stays put when refused. `jump::cmd_stargate_jump` checks [`JUMP_GUARDS`] beside
//! its crimewatch refusal and raises the refusal as a `UserError`, as its
//! other refusals do.

use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// Why a ship may not move: the `UserError` message and its dict.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub user_error: &'static str,
    pub args: Vec<(PyValue, PyValue)>,
}

impl Refusal {
    /// The refusal as the client's `UserError`.
    pub fn into_error(self) -> ServiceError {
        ServiceError::Raise(eve_proto::user_error(self.user_error, self.args))
    }
}

/// One rule: `(world, character id)`, and `Some` to refuse.
pub type Guard = fn(&Registry, i64) -> Option<Refusal>;

/// Checked by every warp, in order.
pub const WARP_GUARDS: &[Guard] = &[
    crate::tackle::scrammed,
    // NPC scrams (W18 CP49): the whole 104, player and NPC sources together.
    crate::npc_ewar::scrammed,
    // A squadron's warp disruption or tackle (FT-F): the whole 104 again.
    crate::fighters::scrammed,
    // SOC-D3a: a ship with an Entosis Link running does not warp.
    crate::entosis::linking,
    // CONCORD-1b: a criminal under a CONCORD response does not warp.
    crate::concord::warp_guard,
    // MJD-MASS: nothing warps while a Micro Jump Drive spools.
    crate::mjd::spooling,
    // CLOAK-1: a cloak that is not warp-safe does not warp.
    crate::cloak::warp_guard,
    // BUBBLES-SB: inside a warp bubble (a probe's, or a HIC's field).
    crate::bubble::inside,
    // NW-9a: a titan that has fired a doomsday sits still.
    crate::superweapon::immobile_guard,
];

/// Checked by every stargate jump, in order.
pub const JUMP_GUARDS: &[Guard] = &[
    crate::tackle::gate_scrambled,
    // A hull heavier than the wormhole's per-jump mass (W17 CP54b); `None`
    // for a stargate jump.
    crate::wormhole_jump::too_heavy,
    // SOC-D3a: nor jump.
    crate::entosis::linking,
    // MJD-MASS: nor does a ship whose Micro Jump Drive spools.
    crate::mjd::spooling,
    // CLOAK-1: nor does a cloaked ship (a wormhole has its own text).
    crate::cloak::jump_guard,
    // NW-9a: nor does a ship that has fired a doomsday jump.
    crate::superweapon::jump_guard,
];

/// The first refusal among `guards`, in table order; `None` lets the ship go.
pub fn check(guards: &[Guard], w: &Registry, character_id: i64) -> Option<Refusal> {
    guards.iter().find_map(|guard| guard(w, character_id))
}

/// **A warp that is still aligning is stopped by a scram.** EVE: the warp
/// activates only when the align gate opens, and a ship that is warp
/// disrupted by then does not go -- a scram landing during the align
/// cancels the warp. The record holds the commanded warp with its
/// activation stamp ahead ([`crate::space_state::Motion::warp_commanded`]),
/// so the guards are asked again for each such ship, and a refused one gets
/// a `Stop` (its pending arrival batch dropped, observers told). A warp
/// that has already activated is untouched: a scram does not pull a ship
/// out of the tunnel. **[A]** that the aborted ship brakes (`Stop`) and
/// does not keep its heading; falsified by a live client whose scrammed
/// aligning ship coasts on.
pub fn abort_scrammed_warps(w: &mut Registry) {
    abort_warps_refused_by(w, &crate::module::plan().warp_guards);
}

/// [`abort_scrammed_warps`] over `guards`.
pub fn abort_warps_refused_by(w: &mut Registry, guards: &[Guard]) {
    let now = crate::space_state::now_seconds();
    let aligning: Vec<(i64, i64, i64)> = w
        .roster()
        .systems()
        .flat_map(|(system, ids)| ids.iter().map(move |id| (system, *id)))
        .filter_map(|(system, id)| w.roster().get(id).map(|p| (system, p)))
        .filter(|(_, p)| p.motion.warp_commanded() && !p.motion.warp_active_at(now))
        .map(|(system, p)| (system, p.character_id, p.ship_id))
        .collect();
    for (system, character, ship) in aligning {
        if check(guards, w, character).is_none() {
            continue;
        }
        w.cancel_scheduled(|p| crate::service::space::is_warp_settle(p, ship));
        let stop = crate::destiny::Action::Stop { ball: ship };
        if let Some(p) = w.roster_mut().get_mut(character) {
            p.motion.after(stop);
        }
        let stamp = w.next_destiny_stamp();
        let mut to = vec![character];
        to.extend(w.roster().others_in_system(system, character));
        if let Some(update) = crate::destiny::ActionUpdate::new(to, stamp, vec![stop]) {
            w.schedule(std::time::Duration::ZERO, update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Character, SandboxConfig};
    use crate::service::Request;

    const ALICE: i64 = 1_000_001;
    const BOB: i64 = 1_000_002;

    fn allow(_: &Registry, _: i64) -> Option<Refusal> {
        None
    }
    fn refuse(_: &Registry, _: i64) -> Option<Refusal> {
        Some(Refusal {
            user_error: "CustomNotify",
            args: vec![],
        })
    }

    fn two_pilots_in_space() -> Registry {
        let ch = |id: i64, ship: i64, name: &str| Character {
            character_id: id,
            name: name.to_owned(),
            ship_id: ship,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![ch(ALICE, 100_000_001, "Alice"), ch(BOB, 100_000_002, "Bob")],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        crate::destiny::on_ballpark_bind(&mut reg, Some(BOB));
        // Bob far off, so a warp to him is a warp and not a refusal for range.
        reg.roster_mut()
            .get_mut(BOB)
            .expect("in space")
            .motion
            .position = [1.0e10, 0.0, 0.0];
        // Alice at rest, so `advance_character`, which runs before the
        // guards, has nothing to advance and the roster can be compared.
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .velocity = [0.0; 3];
        let _ = reg.take_pending();
        reg
    }

    /// The roster's record of Alice, less `since`: the clock
    /// `advance_character` stamps on every call, refused or not.
    fn alice(reg: &Registry) -> crate::space_state::Presence {
        let mut p = *reg.roster().get(ALICE).expect("in space");
        p.since = 0.0;
        p
    }

    fn warp_to_bob(reg: &mut Registry, guards: &[Guard]) {
        let req = Request {
            service: "beyonce",
            method: "CmdWarpToStuff",
            args: &[],
            kwargs: &[],
            bind_params: None,
            actor: Some(ALICE),
        };
        crate::service::warp_guarded(
            reg,
            &req,
            crate::space_state::WARP_SUBJECT_CHAR.to_vec(),
            Some(&PyValue::Int(BOB)),
            0.0,
            guards,
        );
    }

    /// One refusing guard anywhere in the table stops the warp, and stops it
    /// before anything changes: the roster is as it was and nothing is
    /// pushed. The same warp with only an allowing guard goes, so the
    /// fixture is one that would have warped.
    #[test]
    fn a_refusing_guard_stops_the_warp_before_any_change() {
        assert_eq!(check(&[allow], &two_pilots_in_space(), ALICE), None);
        assert!(check(&[allow, refuse], &two_pilots_in_space(), ALICE).is_some());

        let mut reg = two_pilots_in_space();
        let before = alice(&reg);
        warp_to_bob(&mut reg, &[allow, refuse]);
        assert_eq!(alice(&reg), before, "the roster moved");
        assert!(reg.take_pending().is_empty(), "a refused warp pushed");

        let mut reg = two_pilots_in_space();
        let before = alice(&reg);
        warp_to_bob(&mut reg, &[allow]);
        assert_ne!(alice(&reg), before, "an allowed warp");
        assert!(!reg.take_pending().is_empty(), "an allowed warp pushes");
    }

    fn aligning(reg: &mut Registry, effect_stamp: i32) {
        reg.roster_mut()
            .get_mut(ALICE)
            .expect("in space")
            .motion
            .mode = crate::destiny::Mode::Warp {
            goto: [1.0e9, 0.0, 0.0],
            effect_stamp,
            total_distance: 1.0e9,
            stop_distance: 0.0,
            warp_factor: 4_000,
        };
    }

    fn refuse_alice(_: &Registry, c: i64) -> Option<Refusal> {
        (c == ALICE).then(|| Refusal {
            user_error: "WarpScrambled",
            args: vec![],
        })
    }

    /// **A scram during the align stops the warp**: the commanded warp is
    /// dropped to a Stop, and the pilot and the system are told.
    #[test]
    fn a_scram_during_the_align_cancels_the_warp() {
        let mut reg = two_pilots_in_space();
        let ahead = crate::destiny::tick() as i32 + 3_600;
        aligning(&mut reg, ahead);
        abort_warps_refused_by(&mut reg, &[refuse_alice]);
        let p = reg.roster().get(ALICE).expect("in space");
        assert_eq!(p.motion.mode, crate::destiny::Mode::Stop);
        let due = reg.take_due(std::time::Instant::now());
        let stop = due.iter().find(|n| n.to_push().0 == "DoDestinyUpdate");
        let stop = stop.expect("a Stop is pushed");
        let mut to = stop.audience().unwrap_or_default();
        to.sort_unstable();
        assert_eq!(to, vec![ALICE, BOB], "the pilot and the observer");
    }

    /// The tunnel is not undone by a scram, and an unscrammed align goes on.
    #[test]
    fn a_scram_does_not_pull_an_active_warp_and_no_scram_does_nothing() {
        let mut reg = two_pilots_in_space();
        aligning(&mut reg, crate::destiny::tick() as i32 - 5);
        let before = reg.roster().get(ALICE).expect("in space").motion.mode;
        abort_warps_refused_by(&mut reg, &[refuse_alice]);
        assert_eq!(
            reg.roster().get(ALICE).expect("in space").motion.mode,
            before
        );
        aligning(&mut reg, crate::destiny::tick() as i32 + 3_600);
        let before = reg.roster().get(ALICE).expect("in space").motion.mode;
        abort_warps_refused_by(&mut reg, &[allow]);
        assert_eq!(
            reg.roster().get(ALICE).expect("in space").motion.mode,
            before
        );
        assert!(reg.take_due(std::time::Instant::now()).is_empty());
    }
}
