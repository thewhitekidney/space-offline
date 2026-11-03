//! **The Micro Jump Drive** (MJD-MASS, lane note `docs/lanes/mjd-mass.md`;
//! spec `handback/r-pvp-spec/report.md` §3).
//!
//! A pilot activates the module (`dogmaIM.Activate(moduleID, 'microJumpDrive',
//! ...)`, effect 4921, category 1, duration attribute 73 -- the spool). The
//! cycle is one cycle: the client sends `repeat` 0 or 1 because of
//! `disallowRepeatingActivation` 1014, and `modules::activate` makes it one whatever
//! it sent. While it spools, the ship's signature blooms by attribute 973
//! (`BaseMicroJumpDrive`, a Python effect: `crate::dogma` applies it). At the
//! end of the cycle the ship is moved [`DISTANCE`] metres along its heading
//! and keeps its velocity and its command.
//!
//! # Facts
//!
//! - **[V]** the distance is `appConst.microJumpDriveDistance = 100000`, not
//!   a module attribute (type 4383 has no attribute 2066; that is the MJFG's).
//! - **[V]** "Upon arrival, the ship maintains its direction and velocity.
//!   Warp scramblers can be used to disrupt the module" (the module text).
//! - **[V]** `effects.MicroJumpDriveJump` reads `trigger.graphicInfo` as a
//!   position **relative to the viewer's ego ball**
//!   (`GenericEffect._SpawnClientBall`: `position + GetCurrentEgoPos()`), so
//!   the push is built **per viewer**.
//! - **[V]** the keys `MicroJumpDriveSpoolupPhase`,
//!   `CantActivateMJDBecauseOfZeroVelocity` and `ModuleIsBlocked`
//!   (`findings/user-errors.tsv`).
//!
//! # What is ours, and assumed
//!
//! - **[A]** the "facing" is the velocity direction when the ship moves faster
//!   than 1 m/s, else the direction of its goto point, else +z: this server
//!   tracks no orientation. Falsifier: an MJD from a near-stop lands somewhere
//!   other than the ship's nose.
//! - **[A]** a scram that lands during the spool fails the jump **at the end
//!   of the cycle** with `ModuleIsBlocked`; it does not abort the module the
//!   moment it lands. Falsifier: a TQ module stops when the scram lands.
//! - **[A]** a ship "immobile" means maximum velocity 0, not stationary.
//!   Falsifier: TQ refuses an MJD from a full stop.
//! - **[A]** the flash is drawn at the departure point, and the pilot's own
//!   client has already moved its ball (`SetBallPosition` precedes the effect
//!   in the same stamp), so the pilot's offset is taken from the destination.
//!   Falsifier: on TQ footage the flash draws at the landing spot, or the
//!   pilot sees it 100 km away.
//! - **[A]** warp, gate jumps and docking are refused during the spool with
//!   `MicroJumpDriveSpoolupPhase`; the key is [V], the list of actions is not.
//! - **[A]** a deactivation during the spool still jumps (it schedules a stop
//!   at the cycle's end, which is the moment of the jump).

use crate::modules::{Running, user_error};
use crate::movement_guard::Refusal;
use crate::service::{Registry, ServiceError};
use eve_marshal::PyValue;

/// `microJumpDrive` **[V-data]**: the module's effect.
pub const EFFECT: i64 = crate::dogma::effect::MICRO_JUMP_DRIVE;

/// `appConst.microJumpDriveDistance` **[V]**, metres.
pub const DISTANCE: f64 = 100_000.0;

/// `OnSpecialFX`'s guid for the jump **[V]**.
pub const FX_JUMP: &str = "effects.MicroJumpDriveJump";

/// A ship slower than this has no heading from its velocity.
const MOVING: f64 = 1.0;

/// `MicroJumpDriveSpoolupPhase`: "You can't do that while Micro Jump Drive is
/// activating." **[V]**
pub const SPOOLING: &str = "MicroJumpDriveSpoolupPhase";

/// Whether `effect_id` is the Micro Jump Drive's.
pub fn is_mjd(effect_id: i64) -> bool {
    effect_id == EFFECT
}

/// Whether the ship has a Micro Jump Drive spooling now.
pub fn spooling_on(w: &Registry, ship_id: i64) -> bool {
    w.modules().effect_running_on(ship_id, EFFECT)
}

/// **The `WARP_GUARDS` / `JUMP_GUARDS` row**: nothing moves while the drive
/// spools.
pub fn spooling(w: &Registry, character_id: i64) -> Option<Refusal> {
    let ship = w.roster().get(character_id)?.ship_id;
    spooling_on(w, ship).then(|| Refusal {
        user_error: SPOOLING,
        args: Vec::new(),
    })
}

/// **The dock refusal**: `MicroJumpDriveSpoolupPhase` while the drive spools.
pub fn refuse_dock(w: &Registry, actor: Option<i64>) -> Result<(), ServiceError> {
    match actor.and_then(|a| spooling(w, a)) {
        Some(refusal) => Err(refusal.into_error()),
        None => Ok(()),
    }
}

/// **The activation check** `modules::activate` makes for the drive: an
/// immobile ship (maximum velocity 0) cannot jump
/// (`CantActivateMJDBecauseOfZeroVelocity`, no arguments).
pub fn check_activation(w: &Registry, ship_id: i64) -> Result<(), ServiceError> {
    match crate::dogma::hull(w, ship_id) {
        Some(h) if h.max_velocity <= 0.0 => Err(user_error(
            "CantActivateMJDBecauseOfZeroVelocity",
            Vec::new(),
        )),
        _ => Ok(()),
    }
}

/// The scram check at the end of the cycle: `Some` fails the jump
/// (`ModuleIsBlocked`, the same refusal the activation makes).
pub fn blocked(w: &Registry, r: &Running, at: i64) -> Option<Refusal> {
    crate::tackle::activation_blocked(w, r.ship_id, r.type_id, at)
}

/// A refusal as the `(key, dict)` of an `OnGodmaShipEffect` stop's error.
pub fn error_of(refusal: Refusal) -> PyValue {
    // The pairs are held `(value, key)` (`pyutil::pypairs`).
    PyValue::Tuple(vec![
        crate::pyutil::bstr(refusal.user_error),
        PyValue::Dict(refusal.args),
    ])
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (len.is_finite() && len > 0.0).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

/// The direction the ship faces, by the **[A]** model in the module header.
pub fn facing(position: [f64; 3], velocity: [f64; 3], goto: Option<[f64; 3]>) -> [f64; 3] {
    let speed =
        (velocity[0] * velocity[0] + velocity[1] * velocity[1] + velocity[2] * velocity[2]).sqrt();
    if speed > MOVING
        && let Some(d) = unit(velocity)
    {
        return d;
    }
    goto.and_then(|p| unit([p[0] - position[0], p[1] - position[1], p[2] - position[2]]))
        .unwrap_or([0.0, 0.0, 1.0])
}

/// `OnSpecialFX(ship, module, moduleType, None, None, 'effects.MicroJumpDriveJump',
/// 0, 1, 0, -1, None, None, 0, graphicInfo)` to **one** viewer, with the
/// departure point relative to **that viewer's** ego ball.
#[derive(Debug, Clone, PartialEq)]
pub struct JumpFx {
    pub to: i64,
    pub stamp: u32,
    pub ship: i64,
    pub module_id: i64,
    pub type_id: i64,
    /// `origin - ego`, metres.
    pub offset: [f64; 3],
}

impl crate::notify::Push for JumpFx {
    fn restamp(&mut self, stamp: u32) {
        self.stamp = stamp;
    }

    fn to_push(&self) -> (&'static str, Vec<PyValue>) {
        let args = vec![
            PyValue::Int(self.ship),
            PyValue::Int(self.module_id),
            PyValue::Int(self.type_id),
            // targetID, otherTypeID.
            PyValue::None,
            PyValue::None,
            crate::pyutil::bstr(FX_JUMP),
            // isOffensive, start, active.
            PyValue::Int(0),
            PyValue::Int(1),
            PyValue::Int(0),
            // duration, repeat, startTime, timeFromStart: the signature's own
            // defaults, as `jump::JumpFx` sends them **[V]**.
            PyValue::Int(-1),
            PyValue::None,
            PyValue::None,
            PyValue::Int(0),
            // graphicInfo: a position relative to the viewer's ego ball.
            PyValue::Tuple(self.offset.iter().map(|c| PyValue::Real(*c)).collect()),
        ];
        let entry = PyValue::Tuple(vec![
            PyValue::Int(i64::from(self.stamp)),
            PyValue::Tuple(vec![
                crate::pyutil::bstr("OnSpecialFX"),
                PyValue::Tuple(args),
            ]),
        ]);
        (
            "DoDestinyUpdate",
            vec![PyValue::List(vec![entry]), PyValue::Int(0)],
        )
    }

    fn audience(&self) -> Option<Vec<i64>> {
        Some(vec![self.to])
    }

    /// Goes to the viewer if he holds the ship.
    fn grid(&self) -> crate::grid::GridRole {
        crate::grid::GridRole::Names(vec![self.ship])
    }
}

/// What a jump did, for the caller and the tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jumped {
    pub from: [f64; 3],
    pub to: [f64; 3],
}

/// **The jump**: move the ship [`DISTANCE`] along its heading, keep its
/// velocity and command, tell everyone who holds it, and bring the grids up
/// to the new place. `None` when the pilot is not in space or is in a warp
/// tunnel (nothing moves).
///
/// The scram re-check is the caller's ([`blocked`]): it also needs the stop's
/// error.
pub fn jump(w: &mut Registry, clock: bool, r: &Running) -> Option<Jumped> {
    let here = w.roster().get(r.character_id).copied()?;
    if here.ship_id != r.ship_id {
        return None;
    }
    w.advance_character(r.character_id);
    let p = w.roster().get(r.character_id).copied()?;
    if p.motion.warp_active() {
        return None;
    }
    let goto = match p.motion.mode {
        crate::destiny::Mode::Goto(point) => Some(point),
        _ => None,
    };
    let dir = facing(p.motion.position, p.motion.velocity, goto);
    let from = p.motion.position;
    let to = [
        from[0] + dir[0] * DISTANCE,
        from[1] + dir[1] * DISTANCE,
        from[2] + dir[2] * DISTANCE,
    ];
    let action = crate::destiny::Action::SetBallPosition {
        ball: r.ship_id,
        point: to,
    };
    if let Some(pr) = w.roster_mut().get_mut(r.character_id) {
        pr.motion.after(action);
    }
    let system = p.solar_system_id;
    let mut audience = vec![r.character_id];
    audience.extend(w.roster().others_in_system(system, r.character_id));
    audience.retain(|id| w.character(*id).is_some());
    let stamp = w.next_destiny_stamp();
    // One stamp: the position first, then each viewer's flash. No
    // `SetBallVelocity`: the velocity is kept.
    if let Some(update) = crate::destiny::ActionUpdate::new(audience.clone(), stamp, vec![action]) {
        emit(w, clock, update);
    }
    for viewer in audience {
        // The viewer's ego position now. The pilot's own ball has moved.
        let ego = if viewer == r.character_id {
            to
        } else {
            w.advance_character(viewer);
            w.roster()
                .get(viewer)
                .map_or([0.0; 3], |v| v.motion.position)
        };
        emit(
            w,
            clock,
            JumpFx {
                to: viewer,
                stamp,
                ship: r.ship_id,
                module_id: r.module_id,
                type_id: r.type_id,
                offset: [from[0] - ego[0], from[1] - ego[1], from[2] - ego[2]],
            },
        );
    }
    crate::grid::reconcile_in(w, system);
    Some(Jumped { from, to })
}

/// `w.push` with the answer, `w.schedule` on the clock.
fn emit(w: &mut Registry, clock: bool, push: impl crate::notify::Push + 'static) {
    if clock {
        w.schedule(std::time::Duration::ZERO, push);
    } else {
        w.push(push);
    }
}

#[cfg(test)]
mod tests;
