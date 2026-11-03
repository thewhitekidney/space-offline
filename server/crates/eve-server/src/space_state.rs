// SPDX-License-Identifier: LicenseRef-NMSA-1.0 AND MIT
// Portions transcribed from carbonengine/destiny (src/Ballpark.cpp),
// Copyright (c) 2026 CCP Games, MIT License; see NOTICE.

//! Where each ship is, and what it was last told to do.
//!
//! **The server is the only authority on a ship's motion, and it is not
//! because we chose to be.** Nothing on the client applies a movement command
//! locally: `michelle.GetRemotePark()` hands back the bound `beyonce` object
//! itself (`michelle.py:511-516`), `Michelle` has exactly two `Cmd*` methods of
//! its own and both are pure forwarders, and `CmdSetSpeedFraction` does not
//! occur in `michelle.pyc` at all **[V]**. The native `destiny.Ballpark`
//! integrates the motion, but only from a state and the changes we push to it.
//! `docs/lanes/movement.md` §1 has the proof and §5 the field-by-field reason
//! for what is stored here.
//!
//! So this module holds the small amount of state that makes that possible:
//! "where the ship was when it was last commanded" and "what it was commanded
//! to do".
//!
//! ## And it integrates, which an earlier version of this file said it would
//! ## deliberately not do
//!
//! That paragraph argued there should be **no integrator**, because the
//! client's ballpark advances position over time and that is the point of
//! Destiny. The first half is still true and the conclusion was wrong, for a
//! reason the argument never addressed: *the client integrates for itself,
//! and we are not the client.* A state blob we build has to say where a ship
//! **is**, and "where it was when it was last commanded" is only the same
//! thing while nothing is moving. Three consequences, all of them live:
//!
//! - a pilot who binds their ballpark mid-flight is handed every other ship's
//!   position from whenever that ship last got a command, which for a ship
//!   crossing a grid is kilometres of error that never corrects -- there is no
//!   correction available, because `SetBallPosition`/`SetBallVelocity` sit
//!   outside every client dispatch set and so reset nothing
//!   (`docs/lanes/movement.md` §8). The *state* has to be right when it is
//!   built, because it is the only chance;
//! - nothing could answer "how far is that?", which is the question warp,
//!   stargate range, targeting range and every weapon ask;
//! - `Motion::after` turns a heading into a point from `self.position`, so a
//!   stale position made each new command compound the last one's error.
//!
//! [`Motion::advanced`] is therefore the client's own physics, run on our
//! side: an exponential approach to the commanded velocity with the hull's
//! own time constant. It is not a second simulation competing with the
//! client's -- we never push a position correction -- it is us being able to
//! read the clock the client is already reading. The positive control that it
//! is *their* curve and not merely a plausible one is
//! `hull::align_seconds`: a ship that has been accelerating for its published
//! align time is at three quarters of its top speed, and a test asserts
//! exactly that.
//!
//! ## Why the mode matters more than it looks
//!
//! `mode` is not only the blob's byte `0x0d` and its tail. **It is an input to
//! the client's next command.** `movementFunctions._IsAlreadyFollowingBallAtRange`
//! (`:251-255`) reads the local ball's `mode`, `followId` and `followRange` and
//! *suppresses* a repeat `CmdOrbit`/`CmdFollowBall`; `activeShipController.SetSpeed`
//! (`:211`, `:222`) debounces on `int(ball.speedFraction * 1000)` and branches
//! on `mode == DSTBALL_STOP` to decide whether to send `CmdGotoDirection` at
//! all **[V]**. A server whose idea of the mode disagrees with the client's
//! makes the player's clicks vanish, with nothing wrong on either side.

use crate::destiny::{Action, Mode};
use eve_marshal::PyValue;

/// How far along a heading a `GotoDirection` points.
///
/// The client's GOTO tail is a **point**, and `CmdGotoDirection` gives a
/// **direction**, so a direction has to become a point somewhere. Far enough
/// that the ship never arrives: at `1e16` metres an `f64` still resolves about
/// two metres, which is irrelevant for a destination nothing will reach.
/// `docs/port-tracker.md` §10.3 has the magnitude as **[V2]**.
pub const GOTO_DISTANCE: f64 = 1e16;

/// One ship's motion, as the server last knew it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub mode: Mode,
    /// What the player has **asked** for, which is not the same as how fast
    /// the ship is going: a ship shoved out of a station mouth has a velocity
    /// and a speed fraction of zero.
    pub speed_fraction: f32,
}

/// Carbon's per-step y damping in `DSTBALL_STOP`, as a continuous rate.
///
/// `Ballpark::EvolveStop` is three lines, and they operate on the velocity
/// directly rather than returning an acceleration:
///
/// ```text
/// mNewVel[1] = (mNewVel[1] - 0.07 * mNewVel[1]) * 0.9345794392523364485981308411215
/// ```
///
/// which is `0.93 * (1/1.07)` = **0.869158879** per step, applied to **y
/// alone**. A damping that singles out one world axis is not physics; it is
/// legacy behaviour kept for compatibility, and it is the shape to look for
/// in a dump -- our stop diverges from the client vertically and not
/// otherwise.
///
/// **Two measurements had to land before this could be written** and both are
/// in `docs/port-tracker.md`:
///
/// - §55.1 — it is gated on `!g_useDynamicalOrientation`, and the real client
///   reports `destiny.settings.Get().useDynamicalOrientation = False`
///   (`tools/client-ci.sh run undock_distance`, 12/12). So the negation is
///   true and the damping runs. `[V]`
/// - §55.2 — the step is **one second**, not the 100 ms §51.5 assumed:
///   `Ballpark`'s constructor sets `mTickInterval(1000)`. The factor is *per
///   step*, so the step is part of the law, and getting it wrong put §51.5's
///   divergence table an order of magnitude out. `[V]`
///
/// As a rate: `-ln(0.869158879) / 1 s` = 0.1402 s⁻¹, i.e. a time constant of
/// 7.13 s on its own. Composed with the hull's own drag it gives
/// `1/tau_y = 1/tau + this`, which is what the integrator uses — the product
/// of two exponentials is one exponential, so the closed form stays closed
/// and agrees with the client exactly at every tick boundary.
///
/// What it is worth, recomputed in §55.2 against the client's own step order:
/// 54 m of y divergence at 2 s and 232 m at 10 s for a frigate, 72 m and
/// 888 m for a cruiser. Real, and not the emergency the old table implied.
const STOP_Y_DAMPING_RATE: f64 = 0.140_229_341_308_650_25;

impl Motion {
    /// **A warp has been commanded**: the ship is aligning for it or flying
    /// it. EVE semantics: until the warp activates, aligning is ordinary
    /// sub-warp flight (it can be locked, shot, scrammed, and it may use
    /// any module). `service::space::warp_to_point` stamps the activation
    /// tick in the *future* (command + align time), so this is true for the
    /// whole align; callers that mean "the ship is in the tunnel" want
    /// [`Motion::warp_active`].
    pub fn warp_commanded(&self) -> bool {
        matches!(self.mode, Mode::Warp { .. })
    }

    /// **The warp is active** at `now` (seconds): the activation stamp is
    /// set and has come due, the same test `advanced_at` uses to leave the
    /// align and fly the curve. Aligning (negative stamp, or a stamp still
    /// ahead) is not warping.
    pub fn warp_active_at(&self, now: f64) -> bool {
        matches!(self.mode, Mode::Warp { effect_stamp, .. }
            if effect_stamp >= 0 && now >= f64::from(effect_stamp))
    }

    /// [`Motion::warp_active_at`] on the wall clock.
    pub fn warp_active(&self) -> bool {
        self.warp_active_at(now_seconds())
    }

    /// A ship that has just arrived somewhere and been told nothing.
    ///
    /// [`Mode::Stop`] is "come to a halt", not "is halted" -- a ball with
    /// velocity in STOP decelerates under the client's own simulation, which
    /// is what leaving a station looks like.
    pub fn arrived(position: [f64; 3], velocity: [f64; 3]) -> Self {
        Motion {
            position,
            velocity,
            mode: Mode::Stop,
            speed_fraction: 0.0,
        }
    }

    /// Record what an action we are about to push will do.
    ///
    /// Only the fields the blob carries: the client integrates everything
    /// else. [`Action::SetMaxSpeed`] changes no field here because the hull's
    /// top speed is `destiny::Hull`'s business and the blob reads it from
    /// there.
    pub fn after(&mut self, action: Action) {
        match action {
            Action::SetSpeedFraction { fraction, .. } => {
                self.speed_fraction = fraction as f32;
            }
            Action::GotoDirection { direction, .. } => {
                self.mode = Mode::Goto(along(self.position, direction));
            }
            Action::GotoPoint { point, .. } => self.mode = Mode::Goto(point),
            Action::Stop { .. } => self.mode = Mode::Stop,
            // The blob carries the flag, not this record.
            Action::SetBallMassive { .. } => {}
            // Interactivity is a flag in the blob, not in this record.
            Action::SetBallInteractive { .. } => {}
            // A cloak is the cloak slot's (`crate::cloak`), not the motion's.
            Action::CloakBall { .. } | Action::UncloakBall { .. } => {}
            Action::FollowBall { target, range, .. } => {
                self.mode = Mode::Follow {
                    ball_id: target,
                    range: range as f32,
                };
            }
            Action::Orbit { target, range, .. } => {
                self.mode = Mode::Orbit {
                    ball_id: target,
                    range: range as f32,
                };
            }
            Action::SetBallPosition { point, .. } => self.position = point,
            Action::SetBallVelocity { velocity, .. } => self.velocity = velocity,
            Action::SetMaxSpeed { .. } => {}
            // Mass and agility are the hull's business too (dogma-modified).
            Action::SetBallMass { .. } | Action::SetBallAgility { .. } => {}
            // A missile is not a ship `Motion`: `missile.rs` flies its own
            // copy (R-MISSILE §2.1), so the launch changes nothing here.
            Action::LaunchMissile { .. } => {}
            // **The server believes the ship is already there**, and that is
            // a choice with a reason rather than an oversight.
            //
            // A warp is the one motion this type does not integrate: the
            // client runs align, accelerate, cruise, decelerate and park off
            // a single message with no further input, and when this was
            // written there was no way to reconcile the arrival either --
            // `Registry::pending` drains only inside a dispatcher turn.
            //
            // Given that, the choice was between believing the ship is at its
            // origin for the whole flight or at its destination. The
            // destination is right within seconds and wrong for seconds,
            // which is the better of the two for the thing that reads this --
            // the state blob handed to a pilot who binds their ballpark
            // mid-warp. Measured, so `[V]` rather than `[A]`: see
            // `docs/port-tracker.md` §30.3.
            //
            // **The premise has since changed twice and the way out is no
            // longer a timer.** `Registry::schedule` exists (§36) and §37
            // closed the last of the queue confusion, but neither matters
            // here: this server integrates motion *lazily*, in
            // [`Motion::advanced`] at observation time, so a warp wants a
            // `Mode` it can evaluate rather than an effect fired at the
            // arrival tick. §37.4 has the curve, which is exactly derivable
            // from the constants `destiny::warp_flight_seconds` already
            // uses and closes against it to the metre.
            Action::WarpTo {
                target,
                min_range,
                warp_factor,
                ..
            } => {
                // **This teleported until 2026-10-24**, and §30.3 measured it
                // doing so: the record jumped to the arrival point at command
                // time, so a pilot binding their ballpark mid-warp was handed
                // a ship already parked at its destination. It was the right
                // trade while `Motion` could not carry a warp -- "right
                // within seconds and wrong for seconds" -- and §§46-53 spent
                // six sections removing the reason.
                //
                // What it sets now is the ball **as the client writes it at
                // this instant**: aligning. The stamp is negative, which is
                // the client's own discriminator (`michelle.IsPreparingWarp`
                // tests `effect_stamp < 0` and nothing else `[V]`), `goto`
                // holds the commanded *centre* rather than the arrival point,
                // and there is no length yet. [`crate::destiny::Mode::warp_goto`]
                // and `warp_total_distance` are those two rules; they are
                // called rather than inlined so the two places that build a
                // `Mode::Warp` cannot drift.
                //
                // **The position is deliberately left alone.** The ship is
                // where it is and still moving; `advanced_at` carries it
                // forward under the ordinary goto integrator, which is what
                // Carbon's `AlignForWarp` does until its gate opens (§53.1).
                //
                // `service::space::warp` refines this a moment later with the
                // two facts only it has -- the hull's align time and the
                // flight's length -- because `after` is given an action and
                // not a ship.
                self.mode = Mode::Warp {
                    goto: crate::destiny::Mode::warp_goto(self.position, target, min_range, -1),
                    effect_stamp: -1,
                    total_distance: crate::destiny::Mode::warp_total_distance(-1, 0.0),
                    stop_distance: min_range,
                    warp_factor,
                };
                // **The throttle is deliberately left alone**, because the
                // client leaves it alone: native `WarpTo` writes
                // `effectStamp`, `followId`, `ownerId`, `followRange` and the
                // goto target, `StopBall` writes those five again, and
                // `SetMode` writes only `mode` -- none of the three touches
                // `speedFraction` at `ball+0xf4` **[V]**. This record is the
                // blob the next observer reads, so clearing a field the ball
                // still holds would hand them a ship with a shut throttle and
                // a ship that is accelerating. Warp *sets* it to full on the
                // way in; see `service::space::warp`.
            }
        }
    }

    /// How fast the ship is actually going, in metres per second.
    pub fn speed(&self) -> f64 {
        let v = self.velocity;
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    /// Where this ship is `dt` seconds later, under the client's own physics.
    ///
    /// **The model, and it is one equation.** A ship's speed approaches the
    /// speed it has been told to hold, exponentially, with a time constant
    /// `tau = agility * mass / 1e6` seconds. That is not a guess: it is
    /// `hull::align_seconds` read backwards. The client publishes align time
    /// as `-ln(0.25) * agility * mass / 1e6` **[V]**
    /// (`shipfitting/fittingDogmaLocationUtil`, from dogma attributes 70 and
    /// 4), and `-ln(0.25)` is exactly how many time constants an exponential
    /// takes to cover three quarters of its range. So the published number
    /// *is* `tau` with a constant in front of it, and
    /// `align_time_is_when_three_quarters_of_the_speed_is_there` is the test
    /// that keeps us honest about it.
    ///
    /// Per axis, with `u` the commanded velocity and `v0` the current one:
    ///
    /// ```text
    /// v(t) = u + (v0 - u) * e^(-t/tau)
    /// x(t) = x0 + u*t + (v0 - u) * tau * (1 - e^(-t/tau))
    /// ```
    ///
    /// Closed form rather than stepped, for two reasons that both bite: a
    /// stepped integrator's answer depends on how often it is called, so two
    /// observers would get two positions; and `dt` here can be any size at
    /// all -- a pilot who undocks into a system where somebody has been
    /// cruising for an hour needs one evaluation, not fourteen thousand.
    ///
    /// **What is exact and what is not.** For [`Mode::Stop`] it is exact: the
    /// commanded velocity is zero, and the ship coasts `v0 * tau` metres,
    /// which is the same number the client's own simulation converges on. For
    /// [`Mode::Goto`] it is exact once the ship is pointed at the
    /// destination and approximate while it is still turning, because this
    /// treats the turn as instantaneous -- the error is bounded by the turn
    /// and disappears as the ship settles. For [`Mode::Follow`] and
    /// [`Mode::Orbit`] it is **`[A]`**: those depend on where the *other*
    /// ball is, which this type does not know, so they are integrated as a
    /// straight-line cruise at the commanded speed. `Mode::Follow` is the
    /// milder of the two -- Carbon's `EvolveFollow` is an ordinary goto at a
    /// point `range` out along the line joining the two balls, so once the
    /// target stops moving a follow *is* the law below -- while an orbit
    /// never settles. An orbiting ship's true
    /// position stays within `range` of its target while ours sails off, so a
    /// long-orbiting ship is the one case a joining observer still sees
    /// wrongly. Falsified by: two clients, one orbiting a station for a
    /// minute, a third joining -- the bracket will be in the wrong place.
    /// The fix needs the target's position and so belongs to [`Roster`], not
    /// here.
    ///
    /// Non-finite and non-positive `dt` return the motion unchanged, as does
    /// a hull with no time constant. A NaN that reached a position would be
    /// written into a ball blob and discarded by the native **in silence**,
    /// which is the failure mode this project has learned to refuse at the
    /// boundary rather than debug later.
    pub fn advanced(self, h: &crate::destiny::hull::Hull, dt: f64) -> Motion {
        self.advanced_at(h, dt, now_seconds())
    }

    /// This motion, which is true at `since`, as it was at the whole-second
    /// `boundary` before it: the state a stamped `AddBalls2`/`SetState` must
    /// carry, because the client's tick for that stamp starts at the boundary
    /// and a ball encoded at "now" arrives up to a second of travel ahead
    /// (`handback/evejs-motion/report.md` §7.1).
    ///
    /// A straight step back along the velocity. **[A]**: a ship mid-turn or
    /// mid-accelerate curves in that second, a difference of centimetres to
    /// metres against the hundreds of metres it removes; falsified by a moving
    /// ship that the owner sees jump on being first drawn. A warp is left as
    /// it is (its position is a function of the clock, not of this record),
    /// and so is a Stop: the undock shove (up to 700 m/s) is a Stop, and
    /// stepping a fresh undocker back would put it inside the station's
    /// 419 m clearance. Also left alone is anything at or before the boundary or with a non-finite
    /// input. At most one second is taken back.
    pub fn at_boundary(mut self, since: f64, boundary: f64) -> Motion {
        let back = since - boundary;
        if !back.is_finite() || back <= 0.0 || matches!(self.mode, Mode::Warp { .. } | Mode::Stop) {
            return self;
        }
        let back = back.min(1.0);
        for (p, v) in self.position.iter_mut().zip(self.velocity) {
            *p -= v * back;
        }
        self
    }

    /// The same, with the wall clock supplied.
    ///
    /// **Only a warp needs this**, and it needs it because its position is a
    /// function of time since *activation* rather than of the step: see
    /// [`crate::destiny::warp_trajectory`]. Everything else integrates a
    /// step and does not care what o'clock it is.
    ///
    /// `effect_stamp` and `now` are **the same clock** -- `destiny::tick()`
    /// is `SystemTime` since the Unix epoch as whole seconds and
    /// [`now_seconds`] is the same epoch as an `f64` -- so the elapsed time
    /// is a subtraction and nothing has to be mapped. §52.2 spells that out,
    /// because this was twice reported as a design question it never was.
    ///
    /// Split out rather than added as a parameter so the existing callers --
    /// thirteen tests and one production site -- keep working unchanged, and
    /// so a warp test can pin the clock instead of reading the real one.
    pub fn advanced_at(mut self, h: &crate::destiny::hull::Hull, dt: f64, now: f64) -> Motion {
        if !dt.is_finite() || dt <= 0.0 {
            return self;
        }
        // **A warp is flown, not integrated.** The drag solver below answers
        // "where does a ship pulled toward a commanded velocity end up",
        // which is the wrong question for a ball on a prescribed curve --
        // and `commanded_velocity` returns zero for `Mode::Warp` precisely
        // so that it cannot be asked.
        //
        // A negative stamp means the ball has not activated and is still
        // aligning (§38.1). That ship *is* under the ordinary integrator,
        // pointing itself at the target with the throttle open, so it falls
        // through to the code below on purpose.
        // A `match` with a guard rather than a nested `if`: the stamp's
        // sign is part of *which case this is*, not a condition inside the
        // warp case, and clippy is right that the nesting said otherwise.
        match self.mode {
            Mode::Warp {
                goto,
                effect_stamp,
                total_distance,
                warp_factor,
                ..
            } if effect_stamp >= 0 && now >= f64::from(effect_stamp) => {
                let t = now - f64::from(effect_stamp);
                let (at, velocity, finished) = crate::destiny::warp_trajectory(
                    self.position,
                    goto,
                    total_distance,
                    warp_factor,
                    f64::from(h.max_velocity),
                    // The ball's own speed, for the upward clamp of §48.3:
                    // the curve starts at `k_a` m/s and a ship that just
                    // crossed the align gate is doing hundreds, so without
                    // this an observer sees it brake on entering warp.
                    self.speed(),
                    t,
                );
                self.position = at;
                self.velocity = velocity;
                if finished {
                    // Dropout parks the ball where it landed. Carbon also
                    // restores `isMassive` and the sensor here (§48.2);
                    // neither is a field of this record, so neither is lost
                    // by being absent from it.
                    self.mode = Mode::Stop;
                    self.velocity = [0.0; 3];
                }
                return self;
            }
            _ => {}
        }
        let tau = f64::from(h.agility) * h.mass / 1.0e6;
        if !tau.is_finite() || tau <= 0.0 {
            return self;
        }
        let commanded = self.commanded_velocity(h);
        let mut position = self.position;
        let mut velocity = self.velocity;
        for axis in 0..3 {
            // **The y axis decays faster while stopping, and only while
            // stopping.** See [`STOP_Y_DAMPING_RATE`].
            let tau_axis = if axis == 1 && matches!(self.mode, Mode::Stop) {
                1.0 / (1.0 / tau + STOP_Y_DAMPING_RATE)
            } else {
                tau
            };
            let decay = (-dt / tau_axis).exp();
            let gap = self.velocity[axis] - commanded[axis];
            position[axis] =
                self.position[axis] + commanded[axis] * dt + gap * tau_axis * (1.0 - decay);
            velocity[axis] = commanded[axis] + gap * decay;
        }
        if !position.iter().all(|c| c.is_finite()) || !velocity.iter().all(|c| c.is_finite()) {
            return self;
        }
        // **A ship does not sail past the point it was sent to.** Only
        // reachable for a real `CmdGotoPoint`: a heading becomes a point
        // [`GOTO_DISTANCE`] away, which nothing arrives at. Arrival parks the
        // ship rather than guessing at the deceleration the client is about
        // to apply -- it is already stopping, and a stopped ship at the right
        // place beats a moving one past it.
        //
        // **Progress is measured *along the line to the target*, and the
        // first version of this measured the unsigned distance travelled.**
        // That says nothing about direction, so a ship whose velocity pointed
        // *away* from its destination satisfied it while receding and was
        // teleported onto the point. Reachable with one click: a station
        // mouth shoves a ship out at up to 700 m/s, so any nearby point
        // behind it qualified in the first seconds of a flight. Projecting
        // the step onto the heading makes receding negative progress, which
        // cannot arrive.
        if let Mode::Goto(point) = self.mode {
            let to_target = [
                point[0] - self.position[0],
                point[1] - self.position[1],
                point[2] - self.position[2],
            ];
            let remaining = distance(self.position, point);
            if let Some(heading) = unit(to_target) {
                let step = [
                    position[0] - self.position[0],
                    position[1] - self.position[1],
                    position[2] - self.position[2],
                ];
                let along = step[0] * heading[0] + step[1] * heading[1] + step[2] * heading[2];
                if along >= remaining {
                    self.position = point;
                    self.velocity = [0.0; 3];
                    return self;
                }
            }
        }
        self.position = position;
        self.velocity = velocity;
        self
    }

    /// Stopped and not moving: [`Self::advanced_at`] returns it unchanged
    /// whatever the hull, so a caller that needs the hull only to advance
    /// may skip building it.
    pub fn is_at_rest(&self) -> bool {
        matches!(self.mode, Mode::Stop) && self.velocity == [0.0; 3]
    }

    /// The ball this ship is tethered to, and at what range -- `None` unless
    /// it is following or orbiting something.
    ///
    /// The pair [`Roster`] needs to finish what [`Motion::advanced`] cannot:
    /// an orbit's position depends on where the *other* ball is, which this
    /// type does not know.
    pub fn tether(&self) -> Option<(i64, f64)> {
        match self.mode {
            Mode::Follow { ball_id, range } | Mode::Orbit { ball_id, range } => {
                Some((ball_id, f64::from(range)))
            }
            _ => None,
        }
    }

    /// Put this ship back at `range` from `target`, on the side it is already
    /// on.
    ///
    /// **What a tether can honestly say, and no more.** A ship orbiting a
    /// station is, at every instant, about `range` from it; *where* around it
    /// depends on the orbit plane and the angle it has swept. So this keeps
    /// the angle it last had and fixes the distance -- which bounds the error
    /// at `range` instead of at speed times elapsed time, and that is the
    /// whole improvement. A ship orbiting at 1 km reads as somewhere on a
    /// 1 km sphere rather than nine kilometres downrange after a minute.
    ///
    /// **`range` here is a centre separation**, which is not what a player
    /// typed. "Orbit at 500" is 500 metres of space between the hulls, so
    /// the caller adds both radii first -- see
    /// [`crate::orbit::centre_distance`], which is where that law and the
    /// 13 km of station this used to bury a ship in are written down.
    ///
    /// **For a follow this is the whole law, and for an orbit it is not.**
    /// It is applied only *while within reach* ([`Motion::tether_within_reach`]):
    /// out of reach the ship is flown toward its target, not snapped.
    /// `EvolveFollow` aims at the point on the line joining the two balls, on
    /// the side the follower is already on (`docs/port-tracker.md` §54.1)
    /// `[V]`, which is what this does. An orbit also has a *plane*, and the
    /// client chooses nothing about it: [`crate::orbit::orbital_normal`] is a
    /// pure function of the ball id and the tick, and §56 settled that the
    /// tick is ours. [`Motion::tethered_in_plane`] uses it; this is what it
    /// falls back to for everything that is not an orbit.
    ///
    /// Still `[A]` for an orbit reconciled through here, and the morning
    /// round can see it: the bracket will be on the wrong side of the thing
    /// it is circling. It will be *at* it.
    ///
    /// A ship exactly on top of its target has no side to be on, and gets an
    /// arbitrary one rather than a NaN.
    pub fn tethered_to(&mut self, target: [f64; 3], range: f64) {
        let side = unit([
            self.position[0] - target[0],
            self.position[1] - target[1],
            self.position[2] - target[2],
        ])
        .unwrap_or([1.0, 0.0, 0.0]);
        if !range.is_finite() || range < 0.0 {
            return;
        }
        self.position = [
            target[0] + side[0] * range,
            target[1] + side[1] * range,
            target[2] + side[2] * range,
        ];
    }

    /// Whether this ship can be at `range` from `target` within `dt` s.
    pub fn tether_within_reach(
        &self,
        h: &crate::destiny::hull::Hull,
        target: [f64; 3],
        range: f64,
        dt: f64,
    ) -> bool {
        let cruise = f64::from(h.max_velocity) * f64::from(self.speed_fraction);
        let reach = if cruise.is_finite() && cruise > 0.0 && dt > 0.0 {
            cruise * dt
        } else {
            0.0
        };
        distance(self.position, target) <= range + reach + TETHER_SLACK
    }

    /// `self` carried `dt` s to `end` on a tether: the old snap when within
    /// reach (bit-identical to `advanced_at` + `tethered_in_plane`), else flown
    /// toward the target at cruise ([`Motion::tethered_step`]), as Carbon's
    /// follow/orbit would.
    pub fn tethered_advance(
        self,
        h: &crate::destiny::hull::Hull,
        ball_id: i64,
        target: [f64; 3],
        range: f64,
        dt: f64,
        end: f64,
    ) -> Motion {
        if self.tether_within_reach(h, target, range, dt) {
            let mut m = self.advanced_at(h, dt, end);
            m.tethered_in_plane(target, range, ball_id, end as u32);
            m
        } else {
            self.tethered_step(h, ball_id, target, range, end.floor() as i64, dt)
        }
    }

    /// Put an orbiting ship on its orbit's **circle**, not merely its sphere.
    ///
    /// `ball_id` is the orbiting ship's **own** ball id -- the one Carbon
    /// reads as `ball->mId` -- and not the id of the thing it circles, which
    /// is what [`Mode::Orbit`] carries. Mixing the two would give every ship
    /// orbiting one station the same plane, which is the exact outcome
    /// Carbon's comment says the low 16 bits are there to prevent. `tick` is
    /// our Unix-seconds clock, the one `destiny::tick()` puts in the state
    /// header that becomes the client's `mCurrentTime` (§56.1) `[V]`.
    ///
    /// What is `[V]` here: the plane's normal is
    /// [`crate::orbit::orbital_normal`] at this ball and tick, the circle is
    /// centred on the target, and `range` is the centre separation from
    /// [`crate::orbit::centre_distance`]. In the steady state of
    /// `EvolveOldStyleOrbit` (`dist == r`) the radial and transverse terms
    /// vanish and the thrust is `unit(n x toVector)` alone -- so the
    /// velocity a settled orbit holds points along that tangent, and this
    /// re-aims ours there, keeping its speed.
    ///
    /// **The projection is our construction, not Carbon's.** Carbon never
    /// places a ball on a plane; it steers toward one, and the instantaneous
    /// orbit normal is `n` with its component along the line to the target
    /// removed, which converges on `n` only as the orbit settles
    /// (`docs/port-tracker.md` §56.3). So the error of snapping onto `n`'s
    /// plane is **largest right after an orbit order**, while a ship is still
    /// spiralling in from wherever it was, and shrinks as it settles. It is
    /// also the reason the steady-state tangent is only the right heading
    /// for a settled orbit.
    ///
    /// **Where on the circle is `[A]`.** That needs the orbit's start time,
    /// which nothing records. So this keeps the nearest point of the circle
    /// to where the ship already was -- the 1-DOF version of
    /// [`Motion::tethered_to`]'s "the side it is already on". Because the
    /// cruise between reconciliations runs along the tangent, the ship does
    /// advance round the circle in the right sense, but by `atan(v*dt/r)`
    /// rather than `v*dt/r`: an under-estimate that grows with the gap
    /// between reads, and that the morning round should expect to see.
    ///
    /// Anything not orbiting goes through [`Motion::tethered_to`] unchanged,
    /// so the caller does not have to branch. A ship sitting on the plane's
    /// axis has no nearest point and gets an arbitrary one rather than a NaN
    /// `[A]`.
    pub fn tethered_in_plane(&mut self, target: [f64; 3], range: f64, ball_id: i64, tick: u32) {
        let Mode::Orbit { .. } = self.mode else {
            self.tethered_to(target, range);
            return;
        };
        if !range.is_finite() || range < 0.0 {
            return;
        }
        // Normalised here, where the orbit module deliberately does not:
        // Carbon never uses `n` as a plane normal on its own, only crossed
        // with `toVector` and normalised afterwards, so a unit `n` is part of
        // our construction and its seventh-digit length is irrelevant to it.
        let Some(n) = unit(crate::orbit::orbital_normal(ball_id, tick)) else {
            self.tethered_to(target, range);
            return;
        };
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let offset = [
            self.position[0] - target[0],
            self.position[1] - target[1],
            self.position[2] - target[2],
        ];
        let along_n = dot(offset, n);
        let side = unit([
            offset[0] - n[0] * along_n,
            offset[1] - n[1] * along_n,
            offset[2] - n[2] * along_n,
        ])
        // On the axis: any direction in the plane. Two candidates, because
        // one of them is parallel to `n` for some tick.
        .or_else(|| unit(cross(n, [1.0, 0.0, 0.0])))
        .or_else(|| unit(cross(n, [0.0, 1.0, 0.0])))
        .unwrap_or([1.0, 0.0, 0.0]);
        self.position = [
            target[0] + side[0] * range,
            target[1] + side[1] * range,
            target[2] + side[2] * range,
        ];
        // `toVector` points from the ship *to* the target, so it is `-side`,
        // and Carbon's settled thrust is `n x toVector`.
        let speed = self.speed();
        if let Some(tangent) = unit(cross(n, [-side[0], -side[1], -side[2]]))
            && speed.is_finite()
        {
            self.velocity = [tangent[0] * speed, tangent[1] * speed, tangent[2] * speed];
        }
    }

    /// The velocity the ship has been *told* to hold -- the thing the
    /// exponential converges on, as distinct from the one it currently has.
    ///
    /// `max_velocity * speed_fraction` along the mode's heading. A stopped
    /// ship is commanded to nothing, which is what makes [`Mode::Stop`]
    /// "come to a halt" rather than "is halted".
    fn commanded_velocity(&self, h: &crate::destiny::hull::Hull) -> [f64; 3] {
        let target_speed = f64::from(h.max_velocity) * f64::from(self.speed_fraction);
        if !target_speed.is_finite() || target_speed <= 0.0 {
            return [0.0; 3];
        }
        let heading = match self.mode {
            Mode::Goto(point) => unit([
                point[0] - self.position[0],
                point[1] - self.position[1],
                point[2] - self.position[2],
            ]),
            // `[A]`, and the method's own doc says why: without the target
            // ball we can only keep the ship going the way it is going.
            Mode::Follow { .. } | Mode::Orbit { .. } => unit(self.velocity),
            // STOP is a command to stop, and TROLL is a visual effect on a
            // ball that is not under thrust.
            Mode::Stop | Mode::Troll { .. } => return [0.0; 3],
            // **Aligning for warp is an ordinary goto**, and that is the
            // question §37.4 left open as "the align phase".
            //
            // It has an answer rather than an assumption. Carbon's
            // `AlignForWarp` activates the warp only once the ball is
            // pointed at the target *and* `speed^2 > (0.75 * maxVelocity)^2`
            // (§34), and until then the ball is steered by the ordinary goto
            // integrator at `speedFraction * maxVelocity` -- the same law
            // the `Mode::Goto` arm above applies. So an aligning ship
            // accelerates toward its destination, which is what makes the
            // gate reachable at all.
            //
            // The stamp's sign is the discriminator, as everywhere else in
            // the warp (§38.1, §46): negative means aligning, and
            // `Mode::warp_goto` puts the commanded **centre** in `goto`
            // while that holds, which is exactly the point to steer at.
            //
            // **Returning zero here was wrong the moment `advanced_at`
            // learned to carry a warp** (§53). Before that no `Motion` ever
            // held `Mode::Warp`, so zero could not be observed; after it, an
            // aligning ship would have been handed a commanded velocity of
            // zero and *decelerated* toward its own destination, never
            // reaching the 0.75 gate.
            //
            // **One arm for every warp, and that is deliberate.** This used
            // to be two: a negative stamp steered at the goto, and anything
            // else returned zero under a comment saying an activated warp
            // "never reaches here". That reasoning was true -- `advanced_at`
            // flies the curve and returns first -- and §53 is the record of
            // what true-but-fragile costs. A safety argument that rests on
            // another function's control flow expires silently when that
            // function changes, and §57's y-damping round changed it again by
            // adding a second way for a stamp to be unreached (a stamp in the
            // *future*, which §59 introduces for the align phase).
            //
            // Steering toward `goto` is right for every case this can
            // actually be asked about, and no worse than zero for the case it
            // cannot: a ball that somehow arrived here activated would be
            // pointed at its own destination instead of told to stop. So the
            // arm no longer needs a guard, and there is no comment left to
            // expire.
            // A missile's launch tick thrusts straight at `goto` (Carbon
            // `EvolveMissile` [V], R-MISSILE §1.4). No `Motion` holds this
            // mode today; the arm is here because the match is exhaustive.
            Mode::Warp { goto, .. } | Mode::Missile { goto, .. } => unit([
                goto[0] - self.position[0],
                goto[1] - self.position[1],
                goto[2] - self.position[2],
            ]),
        };
        match heading {
            Some(u) => [
                u[0] * target_speed,
                u[1] * target_speed,
                u[2] * target_speed,
            ],
            None => [0.0; 3],
        }
    }

    /// One step of a ship tethered to **another moving ball**, under the
    /// client's own law (FX9).
    ///
    /// `other` is the other ball's position at the start of the step,
    /// `range` the centre separation ([`crate::orbit::centre_distance`]),
    /// `ball_id` this ship's own ball id and `tick` the destiny tick the step
    /// starts on. The commanded velocity is Carbon's (`Ballpark.cpp`, MIT,
    /// read-only) **[V]**:
    ///
    /// - an orbit is `EvolveOldStyleOrbit` (`:1246-1327`, the default branch,
    ///   §54.3): a thrust of `radialFactor * unit(n x toVector) +
    ///   transverseFactor * toVector`, with the tangent-point correction
    ///   outside the sphere, the 40,000 m^2 Gaussian and both seven-digit
    ///   truncations, where `n` is [`crate::orbit::orbital_normal`] at this
    ///   ball and tick;
    /// - a follow is `EvolveFollow` (`:1073-1103`): `GotoThrust` (`:1398`) at
    ///   the point `range` from the other ball on the line between them,
    ///   with the homing taper inside one tick of travel (§54.2). Here for
    ///   completeness: [`Roster::advance`] keeps a follow's snap;
    /// - anything else is [`Motion::commanded_velocity`].
    ///
    /// The thrust is `friction * cruise / (mass * agility)`, so the velocity
    /// it drives toward is the thrust direction times `speedFraction *
    /// maxVelocity`, and `Ballpark::Integrate` (`:751`) is the closed form
    /// [`Motion::advanced_at`] already uses with that velocity held for the
    /// step **[V]** (§51.1).
    ///
    /// **Not the snap of [`Motion::tethered_in_plane`]**, which puts a ship
    /// exactly on its circle: Carbon's orbit settles *outside* the set range
    /// at speed (a 300 m/s, `tau` 3 s ship told 2,080 m settles near 2,350 m,
    /// `docs/lanes/fx9.md`), and around a ball that is itself moving the snap
    /// is what made a mutual pair sail off (FX6). A static target keeps the
    /// snap; see [`Roster::advance`].
    pub fn tethered_step(
        self,
        h: &crate::destiny::hull::Hull,
        ball_id: i64,
        other: [f64; 3],
        range: f64,
        tick: i64,
        dt: f64,
    ) -> Motion {
        if !dt.is_finite() || dt <= 0.0 || !range.is_finite() {
            return self;
        }
        let cruise = f64::from(h.max_velocity) * f64::from(self.speed_fraction);
        let commanded = if !cruise.is_finite() || cruise <= 0.0 {
            [0.0; 3]
        } else {
            match self.mode {
                Mode::Orbit { .. } => {
                    let tick = u32::try_from(tick).unwrap_or(0);
                    scale(
                        orbit_thrust(self.position, other, range, ball_id, tick),
                        cruise,
                    )
                }
                Mode::Follow { .. } => follow_velocity(self.position, other, range, cruise),
                _ => self.commanded_velocity(h),
            }
        };
        self.integrated(h, commanded, dt)
    }

    /// The drag law over `dt` with the commanded velocity held: Carbon's
    /// `Integrate`, which is [`Motion::advanced_at`]'s closed form without
    /// the stop damping (a tethered ship is never in STOP).
    fn integrated(self, h: &crate::destiny::hull::Hull, commanded: [f64; 3], dt: f64) -> Motion {
        let tau = f64::from(h.agility) * h.mass / 1.0e6;
        if !tau.is_finite() || tau <= 0.0 {
            return self;
        }
        let decay = (-dt / tau).exp();
        let mut out = self;
        for (axis, u) in commanded.into_iter().enumerate() {
            let gap = self.velocity[axis] - u;
            out.position[axis] = self.position[axis] + u * dt + gap * tau * (1.0 - decay);
            out.velocity[axis] = u + gap * decay;
        }
        if out
            .position
            .iter()
            .chain(out.velocity.iter())
            .all(|c| c.is_finite())
        {
            out
        } else {
            self
        }
    }
}

fn scale(v: [f64; 3], k: f64) -> [f64; 3] {
    [v[0] * k, v[1] * k, v[2] * k]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Carbon's `Vector3d::Normalize`: a zero vector stays zero (`Vector3d.h`).
fn normalized(v: [f64; 3]) -> [f64; 3] {
    unit(v).unwrap_or(v)
}

/// Carbon's seven-digit truncation, as [`crate::orbit`] applies it.
fn truncated7(x: f64) -> f64 {
    (x * 10_000_000.0).trunc() / 10_000_000.0
}

/// `EvolveOldStyleOrbit`'s thrust direction (`Ballpark.cpp:1246-1327`)
/// **[V]**, as a vector of length about one: [`Motion::tethered_step`]
/// scales it by the cruise speed.
fn orbit_thrust(me: [f64; 3], other: [f64; 3], r: f64, ball_id: i64, tick: u32) -> [f64; 3] {
    let raw = [other[0] - me[0], other[1] - me[1], other[2] - me[2]];
    let dist = dot3(raw, raw).sqrt();
    let mut to = normalized(raw);
    let n = crate::orbit::orbital_normal(ball_id, tick);
    let radial = normalized([
        n[1] * to[2] - n[2] * to[1],
        n[2] * to[0] - n[0] * to[2],
        n[0] * to[1] - n[1] * to[0],
    ]);
    // Outside the sphere: aim at the tangent point instead of the centre.
    let to_comp = dist * dist - r * r;
    if to_comp >= 0.0 && dist > 0.0 {
        let rad_comp = r * to_comp.sqrt() / dist;
        to = normalized([
            to_comp / dist * to[0] + rad_comp * radial[0],
            to_comp / dist * to[1] + rad_comp * radial[1],
            to_comp / dist * to[2] + rad_comp * radial[2],
        ]);
    }
    let radial_factor = truncated7((-(r - dist) * (r - dist) / 40_000.0).exp());
    let phi = -dot3(to, radial);
    let disc = 1.0 + radial_factor * radial_factor * (phi * phi - 1.0);
    let mut transverse = if disc > 0.0 {
        radial_factor * phi + disc.sqrt()
    } else {
        radial_factor * phi
    };
    transverse *= if dist - r >= 0.0 { 1.0 } else { -1.0 };
    [
        radial_factor * radial[0] + transverse * to[0],
        radial_factor * radial[1] + transverse * to[1],
        radial_factor * radial[2] + transverse * to[2],
    ]
}

/// `EvolveFollow` then `GotoThrust` (`Ballpark.cpp:1073-1103`, `:1398-1426`)
/// **[V]**, as the velocity the thrust drives toward. The homing distance is
/// one tick of travel at the cruise speed ([`TICK_SECONDS`]).
fn follow_velocity(me: [f64; 3], other: [f64; 3], r: f64, cruise: f64) -> [f64; 3] {
    let delta = [me[0] - other[0], me[1] - other[1], me[2] - other[2]];
    let dist = dot3(delta, delta).sqrt();
    let target = if dist == 0.0 {
        [other[0] + r, other[1], other[2]]
    } else {
        [
            other[0] + delta[0] * r / dist,
            other[1] + delta[1] * r / dist,
            other[2] + delta[2] * r / dist,
        ]
    };
    let a = [target[0] - me[0], target[1] - me[1], target[2] - me[2]];
    let length2 = dot3(a, a);
    let reach = cruise * TICK_SECONDS;
    let k = if length2 < reach * reach {
        let coff = length2 / (reach * reach);
        cruise * coff * coff
    } else {
        cruise
    };
    scale(normalized(a), k)
}

/// The client's destiny step: `Ballpark`'s `mTickInterval(1000)` and
/// `const.simulationTimeStep = 1000` ms (`docs/lanes/movement.md` §4.3,
/// port-tracker §55.2) **[V]**.
pub const TICK_SECONDS: f64 = 1.0;

/// `[A]` The most ticks one advance steps a tick-grid ship through; a
/// longer gap is crossed in one closed-form step first. A cost bound, not a
/// law: a system nobody has looked at for over an hour is caught up in one
/// lump. Falsified by a pair someone watches across such a gap (nobody can:
/// any observer advances the system).
pub const MAX_CATCHUP_TICKS: i64 = 3_600;

/// **A tether snaps only what the ship could have flown** (live-fleetgrid): a
/// follow or orbit is put at its range from its target only when the ship
/// starts within `range + cruise * dt + TETHER_SLACK` of it; otherwise it is
/// flown toward it ([`Motion::tethered_step`]). [A] 10 km: bounds the model's
/// error to 10 km (it was unbounded -- a follower rode its leader's warp 16 AU),
/// keeps every on-grid approach as before, and is ~400x under `grid::GRID_ENTER`.
/// Falsified by a follower the client shows 5-10 km from where the server's
/// range checks put it.
///
/// **Only a follow of a moving ship is bounded.** A tether to a fixed ball (a
/// rock, a wreck, a gate, a station) still snaps at any distance, as before:
/// 17 eve-bot socket tests (and the live miners) rely on an approach to a rock
/// 95-120 km off being instant, and a fixed target cannot drag a ship out of
/// a park. Falsified by a ship that appears at a rock the client shows far
/// away (then bound the fixed sites too, and fix the fixtures).
pub const TETHER_SLACK: f64 = 10_000.0;

/// One ship [`Roster::advance`] is to carry forward, with the facts only the
/// registry has.
#[derive(Debug, Clone, Copy)]
pub struct Mover {
    pub character_id: i64,
    /// The hull it flies with (fitted when dogma has a sheet).
    pub hull: crate::destiny::hull::Hull,
    pub tether: Option<Tether>,
}

/// What a mover is following or orbiting, resolved by the registry.
#[derive(Debug, Clone, Copy)]
pub struct Tether {
    /// The other ball's id. When it is the ship of another mover in the
    /// same call, the pair is stepped together on the tick grid.
    pub ball_id: i64,
    /// Centre separation: [`crate::orbit::centre_distance`].
    pub range: f64,
    /// Where the other ball is now, for a ball that is not a mover.
    pub at: [f64; 3],
}

/// Where a tick-grid mover's tether points.
#[derive(Clone, Copy)]
enum Pull {
    Free,
    Fixed { at: [f64; 3], range: f64 },
    Mover { index: usize, range: f64 },
}

/// One mover during a tick-grid advance.
struct Run {
    character_id: i64,
    ship_id: i64,
    hull: crate::destiny::hull::Hull,
    pull: Pull,
    state: Motion,
    tick: i64,
}

impl Run {
    /// Whether this mover follows (not orbits) another mover. Such a ship
    /// keeps the snap of [`Motion::tethered_to`] (while within reach, see
    /// [`Motion::tethered_advance`]) and so is placed after the
    /// mover it follows ([`follow_order`]).
    fn follows_a_mover(&self) -> bool {
        matches!(self.pull, Pull::Mover { .. }) && matches!(self.state.mode, Mode::Follow { .. })
    }

    /// This mover, `dt` (at most a tick) on from the start of `tick`.
    /// `start` is every mover at the tick's start, which an orbit reads
    /// (Carbon's first pass); `placed` is every mover as far as this tick
    /// has placed it, which a follow snaps to.
    fn stepped(&self, start: &[[f64; 3]], placed: &[[f64; 3]], tick: i64, dt: f64) -> Motion {
        let end = tick as f64 + dt;
        match self.pull {
            Pull::Free => self.state.advanced_at(&self.hull, dt, end),
            Pull::Fixed { at, range } => {
                let mut m = self.state.advanced_at(&self.hull, dt, end);
                m.tethered_in_plane(at, range, self.ship_id, end as u32);
                m
            }
            Pull::Mover { index, range } if self.follows_a_mover() => {
                self.state
                    .tethered_advance(&self.hull, self.ship_id, placed[index], range, dt, end)
            }
            Pull::Mover { index, range } => {
                self.state
                    .tethered_step(&self.hull, self.ship_id, start[index], range, tick, dt)
            }
        }
    }
}

/// The order a tick places its movers in: everything that does not follow
/// another mover first, then each follower after the mover it follows, and
/// a ring of followers (A follows B follows A) in roster order, the second
/// snapping to where the first was just put -- the order the two-pass
/// advance used before FX9.
fn follow_order(runs: &[Run]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..runs.len())
        .filter(|&i| !runs[i].follows_a_mover())
        .collect();
    let mut waiting: Vec<usize> = (0..runs.len())
        .filter(|&i| runs[i].follows_a_mover())
        .collect();
    while !waiting.is_empty() {
        let ready = waiting
            .iter()
            .position(|&i| match runs[i].pull {
                Pull::Mover { index, .. } => !waiting.contains(&index),
                _ => true,
            })
            .unwrap_or(0);
        order.push(waiting.remove(ready));
    }
    order
}

/// Metres between two points.
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// `v` scaled to length 1, or `None` if it has no usable length.
///
/// `None` rather than a zero vector so that a caller has to decide what no
/// direction means; normalising a zero vector in place is how a NaN gets into
/// a position, and a NaN in a ball blob is discarded by the native with no
/// exception and no log.
pub fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !len.is_finite() || len <= 0.0 {
        return None;
    }
    Some([v[0] / len, v[1] / len, v[2] / len])
}

/// Seconds since the Unix epoch, with the fraction kept.
///
/// **A second clock, and the reason it is not [`crate::destiny::tick`].**
/// `tick` is the *wire* clock: whole seconds, because that is what a Destiny
/// stamp is, and every stamp we send has to be non-decreasing for ever. This
/// is the *physics* clock, and it needs the fraction -- a ship under way
/// covers a few hundred metres in a second, which rounding would throw away,
/// and a ship in warp covers a sizeable fraction of an astronomical unit.
/// Both read the same `SystemTime`, so they agree to within the fraction by
/// construction, and neither is derived from the other because the two have
/// different jobs and only one of them may be rounded.
pub fn now_seconds() -> f64 {
    #[cfg(test)]
    let t = crate::test_clock::system_now();
    #[cfg(not(test))]
    let t = std::time::SystemTime::now();
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// `from` plus [`GOTO_DISTANCE`] along `direction`, or `from` if the direction
/// has no length.
///
/// The zero-length case is the one that matters: normalising a zero vector is
/// how a NaN gets as far as a native, where it is **discarded in silence**
/// (`Action::is_finite`). A heading of nowhere becomes "go to where you
/// already are", which the client can act on and which cannot poison a batch.
fn along(from: [f64; 3], direction: [f64; 3]) -> [f64; 3] {
    let len =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if !len.is_finite() || len == 0.0 {
        return from;
    }
    [
        from[0] + direction[0] / len * GOTO_DISTANCE,
        from[1] + direction[1] / len * GOTO_DISTANCE,
        from[2] + direction[2] / len * GOTO_DISTANCE,
    ]
}

/// A number the client sent, as an `f64`.
///
/// `Int` as well as `Real` because a slider at either end is the one place a
/// round number could arrive as an integer, and because refusing it would
/// silently drop a command rather than mis-execute one.
fn number(v: Option<&PyValue>) -> Option<f64> {
    match v? {
        // NaN and the infinities are not numbers a pilot can mean: one in a
        // heading or a range poisons the integrator for good.
        PyValue::Real(f) => f.is_finite().then_some(*f),
        PyValue::Int(i) => Some(*i as f64),
        PyValue::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn integer(v: Option<&PyValue>) -> Option<i64> {
    match v? {
        PyValue::Int(i) => Some(*i),
        PyValue::Real(f) if f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

fn triple(args: &[PyValue]) -> Option<[f64; 3]> {
    Some([
        number(args.first())?,
        number(args.get(1))?,
        number(args.get(2))?,
    ])
}

/// The inbound `beyonce` methods that move a ship, and the action each means.
///
/// `ball` is the caller's own ship: every `Cmd*` is called on a `beyonce`
/// bound to the solar system and names no ship, because the real server knows
/// whose session it is. The ball id goes in front of the arguments the client
/// sent, which is why the mapping is here and not in the arm.
///
/// Arities and callers are `docs/lanes/movement.md` §1.2 and the mapping is
/// its §3.2. `None` means "not a movement command" or "arguments we cannot
/// read", and the caller answers `None` without pushing -- never a guess,
/// because a wrong action is worse than a missing one.
pub const COMMANDS: &[&str] = &[
    "CmdSetSpeedFraction",
    "CmdGotoDirection",
    "CmdSteerDirection",
    "CmdGotoPoint",
    "CmdStop",
    "CmdFollowBall",
    "CmdOrbit",
];

/// Whether this inbound method is one [`command`] maps.
///
/// Separate from `command` returning `None` because the two `None`s mean
/// different things: "not a movement command", which belongs in
/// `no_method`'s hands so the miss is logged, and "arguments we could not
/// read", which must be answered rather than refused -- the client's call
/// blocks on the answer either way.
pub fn is_command(method: &str) -> bool {
    COMMANDS.contains(&method)
}

pub fn command(ball: i64, method: &str, args: &[PyValue]) -> Option<Action> {
    match method {
        "CmdSetSpeedFraction" => Some(Action::SetSpeedFraction {
            ball,
            // The client's own throttle is 0..1; a 5.0 would be a speed hack.
            fraction: number(args.first())?.clamp(0.0, 1.0),
        }),
        // `CmdSteerDirection` maps here too, and that is **[I]**: there is no
        // `SteerDirection` in any of the client's three dispatch sets and no
        // native method by that name in anything we can read, and its only
        // caller is the flight-control predictor, whose own private ballpark
        // it drives with `GotoDirection` (`flightControls.py:296`) **[V]**.
        "CmdGotoDirection" | "CmdSteerDirection" => {
            let direction = triple(args)?;
            // No heading to take: refuse rather than go to where we are.
            if direction == [0.0; 3] {
                return None;
            }
            Some(Action::GotoDirection { ball, direction })
        }
        "CmdGotoPoint" => Some(Action::GotoPoint {
            ball,
            point: triple(args)?,
        }),
        "CmdStop" => Some(Action::Stop { ball }),
        // A ship cannot follow or orbit itself, and a range is not negative.
        "CmdFollowBall" => {
            let target = integer(args.first())?;
            (target != ball).then_some(())?;
            Some(Action::FollowBall {
                ball,
                target,
                range: number(args.get(1))?.max(0.0),
            })
        }
        "CmdOrbit" => {
            let target = integer(args.first())?;
            (target != ball).then_some(())?;
            Some(Action::Orbit {
                ball,
                target,
                range: number(args.get(1))?.max(0.0),
            })
        }
        _ => None,
    }
}

/// Inbound `beyonce` methods we answer `None` to and push nothing for.
///
/// **Answered by name rather than left to `no_method`, which would also answer
/// `None`.** The difference is diagnostic: a miss prints `UNANSWERED` once per
/// throttle drag, and a log that cries wolf on every frame is a log nobody
/// reads -- this project has lost a round to exactly that three times.
///
/// Empty now: `CmdGotoBookmark` left it with GAP-QUICKFIX B6 and
/// `CmdStargateJump` with P3.3 (`crate::jump` answers it).
pub const UNIMPLEMENTED: &[&str] = &[];

/// `CmdWarpToStuff`'s `subject`: which kind of thing `subjectID` names.
///
/// A **closed vocabulary of thirteen**, defined exhaustively in shipped
/// *shared* code -- `ballparkCommon/parkhelpers/warpSubjects.py`, which is
/// CCP's client and server both **[V]**. Only two of the thirteen are
/// reachable by a human in this sandbox: `'item'`, which every right-click
/// "Warp To" sends, and `'bookmark'`, which we have no catalogue for.
///
/// Matched by name rather than accepted wholesale so that an unreachable
/// subject refuses *visibly*. A silent `None` for `'scan'` or `'district'`
/// would be indistinguishable from a server that had not noticed the call.
pub const WARP_SUBJECT_ITEM: &[u8] = b"item";

/// `subject` for warping to a **character** -- a fleet member.
///
/// `subjectID` is then a **character** id and not a ship id `[V]`, observed
/// on the wire as `CmdWarpToStuff("char", 90000001)` when one fleet member
/// warps to another. This was missing, so every fleet warp answered `None`
/// and pushed nothing: both players reported being unable to warp to each
/// other, with nothing in any log on either side.
pub const WARP_SUBJECT_CHAR: &[u8] = b"char";

/// How far clear of a thing's hull a warp comes out, in metres.
///
/// **A warp aimed at an object's centre flies the ship into the model**, and
/// that is not hypothetical: a player warped to the station and landed 16 km
/// inside it, because the station's radius is 15,733 m and we sent
/// `(0, 0, 0)` with `minimumRange = 0`. The client applies `minimumRange`
/// itself from the object's centre, so the floor on any warp has to be the
/// target's own radius plus a standoff.
///
/// 1 km, `[A]`, chosen so that a station warp lands **inside docking
/// range**: `autopilot.py:624` requires `surfaceDist < 2500` before it will
/// dock `[V]`, and this leaves 1 km of surface distance. For a ship it is
/// simply "beside them rather than inside them".
///
/// A player who asks for more -- "warp to within 100 km" -- gets what they
/// asked for; this is a floor, not a setting.
pub const WARP_STANDOFF: f64 = 1_000.0;

/// `appConst.minWarpDistance` **[V]** (`docs/re/warp.md` 2.5): 150 km. The bot crate's
/// `MIN_WARP_DISTANCE` is the same number.
pub const MIN_WARP_DISTANCE: f64 = 150_000.0;

/// How far off its destination the **autopilot** asks to come out of warp.
///
/// `appConst.minAutoPilotWarpInDistance`, 10 km **[V]**
/// (`docs/re/warp.md` §6.2). Unlike every other warp, the server picks this
/// one: `CmdWarpToStuffAutopilot` sends a destination and no keywords at all,
/// so there is nothing to read. `[A]` on it being the right choice, and the
/// constant exists for no other purpose -- an autopilot has to land near
/// enough to a gate to jump, not on top of it.
pub const AUTOPILOT_WARP_IN: f64 = 10_000.0;

/// `minRange`, in metres, from a `CmdWarpToStuff` keyword argument.
///
/// **Absent, `int` or `float`**, all three observed at real call sites
/// **[V]**: the People & Places window sends no kwargs at all, the submenu
/// sends `float(ranges[i])`, and the right-click default sends
/// `GetDefaultActionDistance('WarpTo')`. So this coerces, and a missing
/// keyword is **`0.0`** -- deliberately *not* the native's own `20000.0`
/// default, which would put every ship 20 km short of everything it warped
/// to without anyone asking.
pub fn warp_min_range(kwargs: &[(PyValue, PyValue)]) -> f64 {
    kwarg(kwargs, "minRange")
        .and_then(|v| number(Some(v)))
        .filter(|r| r.is_finite() && *r >= 0.0)
        .unwrap_or(0.0)
}

/// `CmdAlignTo`'s `dstID`, if the call named a destination object.
///
/// **Keyword-only, 0 positional + 2 keyword** -- `menusvc.py:3315` is
/// `bp.CmdAlignTo(dstID=targetID, bookmarkID=bookmarkID)`, a
/// `CALL_FUNCTION 512` **[V]** (`docs/lanes/movement.md` §1.3). Exactly one
/// of the two is non-`None`, and this reads the one we can act on: a
/// `bookmarkID` names a bookmark, which this server has no catalogue for.
///
/// Reading it by name rather than by position is not fussiness.
/// `findings/client-calls.tsv` records this call as "0 args", which is true
/// of the positional count and misleading as an arity -- the census is blind
/// to `CALL_FUNCTION_KW` and `CALL_FUNCTION_VAR` and reports zero for both
/// **[V]**. A positional read would find nothing and look like a client that
/// sent nothing.
pub fn align_destination(kwargs: &[(PyValue, PyValue)]) -> Option<i64> {
    kwarg(kwargs, "dstID").and_then(|v| integer(Some(v)))
}

/// `CmdAlignTo`'s `bookmarkID`, when the call named a saved point instead.
pub fn align_bookmark(kwargs: &[(PyValue, PyValue)]) -> Option<i64> {
    kwarg(kwargs, "bookmarkID").and_then(|v| integer(Some(v)))
}

/// One keyword argument by name, matching either string representation.
///
/// **Both, because nothing in this server has ever read a keyword argument
/// before** -- `CmdAlignTo` is the first -- and a marshal stream can carry a
/// short name as either a byte string or a unicode one. `[I]`: which the
/// client uses here is unobserved, and the cost of accepting both is one
/// comparison while the cost of guessing wrong is a command that silently
/// does nothing.
///
/// **The key is the SECOND element of each pair, and reading the first was a
/// real defect for the whole life of this function.** A marshal dict is
/// stored in wire order and each pair is `(value, key)` -- the dict handler
/// does `A = ReadObject(); B = ReadObject(); PyDict_SetItem(dict, B, A)`,
/// confirmed from `0x1802dd1d0` and written down on
/// [`eve_marshal::PyValue::Dict`] from the day the parser was built. This
/// matched `name` against the *value* slot, so a real client's
/// `CmdAlignTo(dstID=60012526)` arrived as `[(Int(60012526),
/// Bytes("dstID"))]`, found nothing, and `align` returned early: **"Align to"
/// did nothing at all, silently.** `CmdWarpToStuff`'s `minRange` was read as
/// absent the same way, so "warp to within 100 km" landed at the standoff.
///
/// Every unit test of a keyword command passed throughout, because each one
/// hand-built its kwargs as `(key, value)` -- the order the function wanted
/// rather than the order the wire uses. The only test that could not pass for
/// the wrong reason is one that goes over a socket through the real codec:
/// `eve-bot`'s `an_align_sent_the_way_the_client_sends_it_reaches_the_server`.
/// `docs/port-tracker.md` §39.
pub fn kwarg<'a>(kwargs: &'a [(PyValue, PyValue)], name: &str) -> Option<&'a PyValue> {
    // **Delegated, not re-spelled.** This function existed because nothing
    // had read a keyword argument before, and it hand-rolled a lookup that
    // `pyutil::dict_get` had already got right -- then got the slot backwards.
    // `pyutil::pairs_get` is now the single definition; `as_bytes` there
    // accepts a `str` or a `unicode` key, which is what this used to spell out.
    crate::pyutil::pairs_get(kwargs, name.as_bytes())
}

/// One character who is in space: where, in what, and doing what.
///
/// **One record, not two.** An earlier shape kept a `BTreeMap<ship_id,
/// Motion>` and nothing else, so "is in space" and "has a motion" were
/// separate facts that could disagree -- and the moment a second observer
/// exists, a ship in one and not the other is either an invisible player or a
/// ghost nobody can remove. Keeping them in one record makes that
/// unrepresentable.
///
/// **Keyed by character**, because every audience in this project is a list of
/// character ids (`crate::notify::Push::audience`), every lifecycle event is a
/// character event (undock, dock, disconnect), and `Registry::character` is
/// the only source of the corporation, alliance and ship type a ball needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Presence {
    pub character_id: i64,
    pub ship_id: i64,
    pub solar_system_id: i64,
    pub motion: Motion,
    /// When `motion` was last true, on [`now_seconds`]'s clock.
    ///
    /// **The field that makes the motion mean anything.** A position without
    /// a time is "somewhere the ship once was", which is what this record
    /// used to hold. Everything that reads a position has to bring it up to
    /// now first, and `Registry::advance_character` is the one place that
    /// does it -- integrating on read rather than on a timer, so there is no
    /// background task to keep alive and no question of what happens to a
    /// world nobody is looking at.
    pub since: f64,
    /// The motion at the last whole **tick**, for a ship advanced on the
    /// client's tick grid (FX9, [`Roster::advance`]): `None` for every ship
    /// the closed form carries in one step, which is every ship not tethered
    /// to another ship. Written only by [`Roster::advance`]; anything else
    /// that writes `motion` or `since` makes it stale, which
    /// [`Ticked::still_holds`] notices.
    pub ticked: Option<Ticked>,
}

/// A presence's state on the tick grid, beside the view handed to readers.
///
/// **Why two motions.** Readers want the ship *now*, which is between ticks;
/// the next advance has to start from a tick, because the tethered law is
/// stepped and a step's answer depends on where it starts. Continuing from
/// the between-ticks view is exactly the cadence dependence FX6 measured
/// (`docs/lanes/fx9.md`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ticked {
    /// The motion at the start of tick `tick`.
    pub motion: Motion,
    /// A destiny tick: whole seconds on [`now_seconds`]'s clock, the clock
    /// the state header gives the client as `mCurrentTime` (§56.1) `[V]`.
    pub tick: i64,
    /// What [`Roster::advance`] last wrote into `motion` and `since`.
    pub view: Motion,
    pub view_at: f64,
}

impl Ticked {
    /// Whether nobody has written the presence since [`Roster::advance`]
    /// did. A command (`Motion::after`), a teleport or a test winding
    /// `since` back each break it, and the next advance starts again from
    /// the written record.
    pub fn still_holds(&self, p: &Presence) -> bool {
        self.view == p.motion && self.view_at.to_bits() == p.since.to_bits()
    }
}

impl Presence {
    /// A ship that has just arrived, as of now.
    ///
    /// The constructor exists so that `since` cannot be forgotten: the one
    /// value that must never be defaulted is the time, because a zero there
    /// means 1970 and the first integration moves the ship by fifty years of
    /// cruising.
    pub fn arriving(character_id: i64, ship_id: i64, solar_system_id: i64, motion: Motion) -> Self {
        Presence {
            character_id,
            ship_id,
            solar_system_id,
            motion,
            since: now_seconds(),
            ticked: None,
        }
    }
}

/// Everyone in space, by character, with a per-system index beside it.
///
/// **The system is a field, not a key**: a nested map has two insert points
/// and makes "a character in two systems at once" representable. **The
/// index** (`system -> ids`, C7 §2.3, W11 CP6) is written only by
/// [`Self::arrive`] and [`Self::leave`], so it is derived, never a second
/// truth; it makes [`Self::in_system`] O(balls there) and gives
/// `crate::liveness` the occupied systems. **Invariant:** [`Self::get_mut`]
/// never changes `solar_system_id` or `ship_id` (use `set_ship_id`; a jump leaves and arrives; no caller
/// writes it). Debug builds check every `in_system` answer against the scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roster {
    by_character: std::collections::BTreeMap<i64, Presence>,
    by_system: std::collections::BTreeMap<i64, std::collections::BTreeSet<i64>>,
    /// ship (ball) id -> the characters whose presence has that `ship_id`,
    /// so [`Self::by_ship`] is O(log n) instead of a scan of every pilot.
    /// A set because nothing forbids two records sharing a ship id; the
    /// lowest character id answers, exactly as the old scan did. Kept in
    /// step by `arrive`, `leave` and `set_ship_id`.
    by_ship: std::collections::BTreeMap<i64, std::collections::BTreeSet<i64>>,
    /// A count of structural writes (PA-8): see [`Revision`].
    revision: Revision,
}

/// A count of the roster's structural writes (`arrive`, `leave`,
/// `set_ship_id`, and every non-empty `advance`): a plan made against one
/// revision is still true at the same one. **Not state**, so it never makes
/// two rosters unequal. `get_mut` does not bump it: its callers change a
/// ball's mode and speed fraction, never a position, system or ship id.
#[derive(Debug, Clone, Copy, Default)]
pub struct Revision(u64);

impl PartialEq for Revision {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Roster {
    /// The count of structural writes so far ([`Revision`]).
    pub fn revision(&self) -> u64 {
        self.revision.0
    }

    pub fn get(&self, character_id: i64) -> Option<&Presence> {
        self.by_character.get(&character_id)
    }

    /// The presence, to change its motion. **Not its system**: see the type.
    pub fn get_mut(&mut self, character_id: i64) -> Option<&mut Presence> {
        self.by_character.get_mut(&character_id)
    }

    /// Give a character in space another ship id, keeping the ship index
    /// true. **The only way to change `ship_id`**: [`Self::get_mut`] must not
    /// (debug builds check every [`Self::by_ship`] answer against the scan).
    pub fn set_ship_id(&mut self, character_id: i64, ship_id: i64) {
        let Some(p) = self.by_character.get_mut(&character_id) else {
            return;
        };
        let old = std::mem::replace(&mut p.ship_id, ship_id);
        self.revision.0 += 1;
        Self::unindex_ship(&mut self.by_ship, old, character_id);
        self.by_ship
            .entry(ship_id)
            .or_default()
            .insert(character_id);
    }

    fn unindex_ship(
        by_ship: &mut std::collections::BTreeMap<i64, std::collections::BTreeSet<i64>>,
        ship_id: i64,
        character_id: i64,
    ) {
        if let Some(set) = by_ship.get_mut(&ship_id) {
            set.remove(&character_id);
            if set.is_empty() {
                by_ship.remove(&ship_id);
            }
        }
    }

    /// Put a character in space, replacing anything already there.
    ///
    /// **Replacing, not merging.** This is called when a ship *arrives*, and
    /// the arrival's own position is the truth; whatever a previous flight
    /// left behind is stale by definition. Docking and undocking twice would
    /// otherwise put the ship back where it docked.
    pub fn arrive(&mut self, presence: Presence) {
        let id = presence.character_id;
        self.revision.0 += 1;
        if let Some(old) = self.by_character.insert(id, presence) {
            self.unindex(old.solar_system_id, id);
            Self::unindex_ship(&mut self.by_ship, old.ship_id, id);
        }
        self.by_ship.entry(presence.ship_id).or_default().insert(id);
        self.by_system
            .entry(presence.solar_system_id)
            .or_default()
            .insert(id);
    }

    /// Take a character out of space. Returns what was there, so the caller
    /// can tell the observers which ball to remove.
    pub fn leave(&mut self, character_id: i64) -> Option<Presence> {
        let gone = self.by_character.remove(&character_id)?;
        self.revision.0 += 1;
        self.unindex(gone.solar_system_id, character_id);
        Self::unindex_ship(&mut self.by_ship, gone.ship_id, character_id);
        Some(gone)
    }

    /// Drop one id from a system's set, and the set once it is empty.
    fn unindex(&mut self, system: i64, character_id: i64) {
        if let Some(set) = self.by_system.get_mut(&system) {
            set.remove(&character_id);
            if set.is_empty() {
                self.by_system.remove(&system);
            }
        }
    }

    /// Everyone in one solar system, in character order.
    pub fn in_system(&self, solar_system_id: i64) -> Vec<Presence> {
        let out: Vec<Presence> = self
            .ids_in(solar_system_id)
            .filter_map(|id| self.by_character.get(&id))
            .copied()
            .collect();
        #[cfg(debug_assertions)]
        {
            let scanned = self
                .by_character
                .values()
                .filter(|p| p.solar_system_id == solar_system_id)
                .count();
            debug_assert!(
                scanned == out.len() && out.iter().all(|p| p.solar_system_id == solar_system_id),
                "Roster index out of step in system {solar_system_id}: a get_mut \
                 caller changed solar_system_id instead of leave + arrive"
            );
        }
        out
    }

    /// Everyone in one solar system **except** this character -- the audience
    /// for "something happened to me" and the set of parks a new ball has to
    /// be added to.
    pub fn others_in_system(&self, solar_system_id: i64, except: i64) -> Vec<i64> {
        self.ids_in(solar_system_id)
            .filter(|id| *id != except)
            .collect()
    }

    /// The ids in one system, in character order, from the index.
    fn ids_in(&self, solar_system_id: i64) -> impl Iterator<Item = i64> + '_ {
        self.by_system
            .get(&solar_system_id)
            .into_iter()
            .flatten()
            .copied()
    }

    /// Every occupied system with its ids (characters, NPCs and drones).
    pub fn systems(&self) -> impl Iterator<Item = (i64, &std::collections::BTreeSet<i64>)> {
        self.by_system.iter().map(|(s, ids)| (*s, ids))
    }

    /// The presence flying this ball, if the world knows of one.
    ///
    /// A Destiny action names a **ball**, and a ball is a ship id, while this
    /// map is keyed by character. Answered from the ship index; debug builds
    /// check it against the scan it replaced.
    pub fn by_ship(&self, ship_id: i64) -> Option<&Presence> {
        let found = self
            .by_ship
            .get(&ship_id)
            .and_then(|set| set.iter().next())
            .and_then(|id| self.by_character.get(id));
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            found.map(|p| p.character_id),
            self.by_character
                .values()
                .find(|p| p.ship_id == ship_id)
                .map(|p| p.character_id),
            "Roster ship index out of step for ship {ship_id}"
        );
        found
    }

    /// Whether this character is in space at all. The server's first
    /// representation of "undocked" -- `Character::station_id` is never
    /// cleared, so until this existed nothing could answer the question.
    pub fn is_in_space(&self, character_id: i64) -> bool {
        self.by_character.contains_key(&character_id)
    }

    /// Carry `movers` forward to `now` (FX9).
    ///
    /// **Two regimes, chosen by whether any mover is tethered to another
    /// mover's ship.**
    ///
    /// - None is: the closed form, one step of any length, exactly as
    ///   `Registry::advance_character` did before FX9 -- ships under their
    ///   own power first, then ships tethered to something fixed, snapped
    ///   onto their circle ([`Motion::tethered_in_plane`]). The closed form
    ///   composes, so the cadence of the calls does not matter.
    /// - One is: every mover is stepped together on the client's tick grid
    ///   ([`TICK_SECONDS`], whole seconds of the header clock), each tick
    ///   reading every other mover's position at the tick's start -- Carbon's
    ///   `Ballpark::Evolve` computes every ball's thrust in a first pass and
    ///   integrates in a second (`Ballpark.cpp:421-600`) **[V]** -- with a
    ///   ship *orbiting* a mover under [`Motion::tethered_step`], and a ship
    ///   *following* one snapped to its range from where that mover was just
    ///   put *while within reach* ([`Motion::tethered_advance`]; the [`Motion::tethered_to`] law, `[A]` as before; Carbon's
    ///   follow is a goto at that point, §54.1, which the snap is the settled
    ///   state of, and the bot harness pins the range). The state
    ///   at the last whole tick is kept ([`Ticked`]) and the next call starts
    ///   there, so the positions on the grid do not depend on how often this
    ///   is called; between ticks, readers get a partial step from it.
    ///
    /// Before FX9 a mutual pair (an escort orbiting its rat while the rat
    /// orbits it) was advanced one step per call, in character order, and
    /// translated by 79 km in 120 s at a 0.05 s cadence and 6 km at 3 s
    /// (`docs/lanes/fx6.md`): faster than either ship can fly.
    ///
    /// A mover whose record was written since the last call (a command, a
    /// teleport) starts from that record, first carried to the next whole
    /// tick by the closed form. `[A]`: the client applies a command on a
    /// tick boundary, so this is up to one tick early or late.
    pub fn advance(&mut self, movers: &[Mover], now: f64) {
        if !now.is_finite() {
            return;
        }
        if !movers.is_empty() {
            self.revision.0 += 1;
        }
        let ships: std::collections::BTreeMap<i64, usize> = movers
            .iter()
            .enumerate()
            .filter_map(|(i, m)| self.get(m.character_id).map(|p| (p.ship_id, i)))
            .collect();
        let paired = movers.iter().any(|m| {
            m.tether.is_some_and(|t| {
                ships
                    .get(&t.ball_id)
                    .is_some_and(|&j| movers[j].character_id != m.character_id)
            })
        });
        if paired {
            self.advance_on_ticks(movers, now, &ships);
        } else {
            self.advance_closed_form(movers, now);
        }
    }

    /// Whether anything in this system is tethered to another ship in it:
    /// when it is, advancing one ship means advancing the system, because a
    /// pair is stepped together ([`Self::advance`]).
    pub fn has_pair(&self, solar_system_id: i64) -> bool {
        let ships: std::collections::BTreeSet<i64> = self
            .ids_in(solar_system_id)
            .filter_map(|id| self.by_character.get(&id).map(|p| p.ship_id))
            .collect();
        self.ids_in(solar_system_id)
            .filter_map(|id| self.by_character.get(&id))
            .any(|p| {
                p.motion
                    .tether()
                    .is_some_and(|(ball, _)| ball != p.ship_id && ships.contains(&ball))
            })
    }

    fn advance_closed_form(&mut self, movers: &[Mover], now: f64) {
        for tethered in [false, true] {
            for m in movers.iter().filter(|m| m.tether.is_some() == tethered) {
                let Some(p) = self.by_character.get_mut(&m.character_id) else {
                    continue;
                };
                p.ticked = None;
                let dt = now - p.since;
                if dt > 0.0 {
                    p.motion = p.motion.advanced_at(&m.hull, dt, now);
                    if let Some(t) = m.tether {
                        p.motion
                            .tethered_in_plane(t.at, t.range, p.ship_id, now as u32);
                    }
                    p.since = now;
                }
            }
        }
    }

    fn advance_on_ticks(
        &mut self,
        movers: &[Mover],
        now: f64,
        ships: &std::collections::BTreeMap<i64, usize>,
    ) {
        let last = now.floor() as i64;
        let mut runs: Vec<Run> = Vec::with_capacity(movers.len());
        for m in movers {
            let Some(p) = self.by_character.get(&m.character_id) else {
                continue;
            };
            let pull = match m.tether {
                None => Pull::Free,
                Some(t) => match ships.get(&t.ball_id) {
                    Some(&j) if movers[j].character_id != m.character_id => Pull::Mover {
                        index: j,
                        range: t.range,
                    },
                    _ => Pull::Fixed {
                        at: t.at,
                        range: t.range,
                    },
                },
            };
            let (state, tick) = match p.ticked {
                Some(t) if t.still_holds(p) => (t.motion, t.tick),
                _ => {
                    // From the written record to the next whole tick.
                    let tick = p.since.ceil() as i64;
                    let gap = tick as f64 - p.since;
                    let mut state = p.motion.advanced_at(&m.hull, gap, tick as f64);
                    if let Pull::Fixed { at, range } = pull {
                        state.tethered_in_plane(at, range, p.ship_id, tick as u32);
                    }
                    (state, tick)
                }
            };
            runs.push(Run {
                character_id: m.character_id,
                ship_id: p.ship_id,
                hull: m.hull,
                pull,
                state,
                tick,
            });
        }
        // `ships` indexes `movers`; re-point it at `runs` (a mover not in the
        // roster has no run, and a tether to it is left where it was).
        let index_of: std::collections::BTreeMap<i64, usize> = runs
            .iter()
            .enumerate()
            .map(|(i, r)| (r.character_id, i))
            .collect();
        for r in &mut runs {
            if let Pull::Mover { index, range } = r.pull {
                r.pull = match index_of.get(&movers[index].character_id) {
                    Some(&i) => Pull::Mover { index: i, range },
                    None => Pull::Free,
                };
            }
        }
        // A gap longer than the cost bound is crossed in one lump first.
        let floor = last - MAX_CATCHUP_TICKS;
        let fixed_points: Vec<[f64; 3]> = runs.iter().map(|r| r.state.position).collect();
        for r in &mut runs {
            if r.tick < floor {
                let dt = (floor - r.tick) as f64;
                let mut m = r.state.advanced_at(&r.hull, dt, floor as f64);
                match r.pull {
                    Pull::Free => {}
                    Pull::Fixed { at, range } => {
                        m.tethered_in_plane(at, range, r.ship_id, floor as u32)
                    }
                    Pull::Mover { index, range } => {
                        m = r.state.tethered_advance(
                            &r.hull,
                            r.ship_id,
                            fixed_points[index],
                            range,
                            dt,
                            floor as f64,
                        )
                    }
                }
                r.state = m;
                r.tick = floor;
            }
        }
        let first = runs.iter().map(|r| r.tick).min().unwrap_or(last);
        let order = follow_order(&runs);
        let mut positions: Vec<[f64; 3]> = runs.iter().map(|r| r.state.position).collect();
        for tick in first..last {
            let start = positions.clone();
            for &i in &order {
                if runs[i].tick > tick {
                    continue;
                }
                let m = runs[i].stepped(&start, &positions, tick, TICK_SECONDS);
                runs[i].state = m;
                runs[i].tick = tick + 1;
                positions[i] = m.position;
            }
        }
        // The views: a partial step from the grid, in the same order.
        let start = positions.clone();
        let mut views: Vec<Option<Motion>> = vec![None; runs.len()];
        for &i in &order {
            let r = &runs[i];
            let Some(p) = self.by_character.get(&r.character_id) else {
                continue;
            };
            let part = now - r.tick as f64;
            let view = if part >= 0.0 {
                r.stepped(&start, &positions, r.tick, part)
            } else {
                // Commanded within the current tick: the grid state is the
                // next tick's, so the view comes from the record itself.
                let dt = now - p.since;
                match r.pull {
                    Pull::Fixed { at, range } => {
                        let mut m = p.motion.advanced_at(&r.hull, dt, now);
                        m.tethered_in_plane(at, range, r.ship_id, now as u32);
                        m
                    }
                    Pull::Mover { index, range } if r.follows_a_mover() => p
                        .motion
                        .tethered_advance(&r.hull, r.ship_id, positions[index], range, dt, now),
                    _ => p.motion.advanced_at(&r.hull, dt, now),
                }
            };
            positions[i] = view.position;
            views[i] = Some(view);
        }
        for (r, view) in runs.iter().zip(views) {
            let (Some(p), Some(view)) = (self.by_character.get_mut(&r.character_id), view) else {
                continue;
            };
            let at = now.max(p.since);
            p.motion = view;
            p.since = at;
            p.ticked = Some(Ticked {
                motion: r.state,
                tick: r.tick,
                view,
                view_at: at,
            });
        }
    }

    pub fn len(&self) -> usize {
        self.by_character.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_character.is_empty()
    }
}

#[cfg(test)]
mod tests {

    /// MOVE-VALIDATE: what a pilot's arguments may and may not become.
    #[test]
    fn movement_commands_refuse_nonsense_arguments() {
        let frac = |v: f64| command(7, "CmdSetSpeedFraction", &[PyValue::Real(v)]);
        assert_eq!(
            frac(5.0),
            Some(Action::SetSpeedFraction {
                ball: 7,
                fraction: 1.0
            })
        );
        assert_eq!(
            frac(-2.0),
            Some(Action::SetSpeedFraction {
                ball: 7,
                fraction: 0.0
            })
        );
        assert!(frac(f64::NAN).is_none());
        assert!(frac(f64::INFINITY).is_none());
        let zero = [PyValue::Real(0.0), PyValue::Real(0.0), PyValue::Real(0.0)];
        assert!(command(7, "CmdGotoDirection", &zero).is_none());
        let nan = [
            PyValue::Real(1.0),
            PyValue::Real(f64::NAN),
            PyValue::Real(0.0),
        ];
        assert!(command(7, "CmdGotoPoint", &nan).is_none());
        assert!(command(7, "CmdOrbit", &[PyValue::Int(7), PyValue::Real(500.0)]).is_none());
        assert!(command(7, "CmdFollowBall", &[PyValue::Int(7), PyValue::Real(5.0)]).is_none());
        assert_eq!(
            command(
                7,
                "CmdFollowBall",
                &[PyValue::Int(99), PyValue::Real(-50.0)]
            ),
            Some(Action::FollowBall {
                ball: 7,
                target: 99,
                range: 0.0
            })
        );
        assert_eq!(
            command(7, "CmdOrbit", &[PyValue::Int(99), PyValue::Real(-1.0)]),
            Some(Action::Orbit {
                ball: 7,
                target: 99,
                range: 0.0
            })
        );
    }
    use super::*;

    /// **A moving ball is encoded as of the second boundary, not of now**
    /// (EVEJS-MOTION 7.1): 0.4 s past the boundary at 300 m/s is 120 m back.
    #[test]
    fn a_moving_ball_is_stepped_back_to_the_boundary() {
        let m = Motion {
            position: [1000.0, 0.0, 5000.0],
            velocity: [300.0, 0.0, 0.0],
            mode: Mode::Goto([9.0e6, 0.0, 5000.0]),
            speed_fraction: 1.0,
        };
        let back = m.at_boundary(1_000.4, 1_000.0);
        assert!((back.position[0] - 880.0).abs() < 1e-6);
        assert_eq!(back.position[2], 5000.0);
        assert_eq!(back.velocity, m.velocity);
        // Never forward, never more than a second, never a stop or a warp.
        assert_eq!(m.at_boundary(999.0, 1_000.0), m);
        assert!((m.at_boundary(1_005.0, 1_000.0).position[0] - 700.0).abs() < 1e-6);
        let stopped = Motion {
            mode: Mode::Stop,
            ..m
        };
        assert_eq!(stopped.at_boundary(1_000.4, 1_000.0), stopped);
    }

    /// **A ship aligning for warp accelerates toward its destination.**
    ///
    /// The align phase §37.4 left open, and the defect that opened the
    /// moment `advanced_at` learned to carry a warp: `commanded_velocity`
    /// returned zero for every `Mode::Warp`, so a ball with a negative
    /// stamp -- the client's own "still aligning" state -- would have been
    /// told to stop, and could never cross the 0.75-of-top-speed gate that
    /// activates the warp.
    ///
    /// Asserted as the property rather than as a formula: from rest, a few
    /// seconds of aligning must leave the ship **closer** to the target and
    /// **moving toward** it at a decent fraction of its top speed.
    #[test]
    fn a_ship_aligning_for_warp_accelerates_toward_the_target() {
        let goto = [4.0 * 149_597_870_700.0, 0.0, 0.0];
        let hull = &crate::destiny::hull::FALLBACK;
        let m = Motion {
            position: [0.0; 3],
            velocity: [0.0; 3],
            // Negative: the client's "preparing warp" state (§38.1).
            mode: Mode::Warp {
                goto,
                effect_stamp: -2,
                total_distance: distance([0.0; 3], goto),
                stop_distance: 0.0,
                warp_factor: 4_000,
            },
            speed_fraction: 1.0,
        };
        let start = distance(m.position, goto);
        // A fixed clock, so the aligning branch cannot accidentally be taken
        // for an activated one.
        let later = m.advanced_at(hull, 5.0, 1_792_800_000.0);

        assert!(
            later.velocity[0] > 0.0,
            "an aligning ship must thrust toward its target, got {:?}",
            later.velocity
        );
        let speed = (later.velocity[0] * later.velocity[0]
            + later.velocity[1] * later.velocity[1]
            + later.velocity[2] * later.velocity[2])
            .sqrt();
        assert!(
            speed > 0.5 * f64::from(hull.max_velocity),
            "after 5 s of aligning the ship is at {speed} m/s of a possible \
             {}; it will never reach the 0.75 gate that activates the warp",
            hull.max_velocity
        );
        assert!(
            distance(later.position, goto) < start,
            "the ship got further from the point it was aligning to"
        );
        assert!(
            matches!(later.mode, Mode::Warp { .. }),
            "aligning must not end the warp"
        );
    }

    /// A standing start sends **two** commands, and both have to land.
    ///
    /// `activeShipController.SetSpeed`'s `mode == DSTBALL_STOP` branch at
    /// `:222` falls through with no `else`, so the server gets
    /// `CmdGotoDirection` and *then* `CmdSetSpeedFraction` **[V]**. Pins that
    /// each changes a different field: the tempting wrong version drives only
    /// the fraction, which is what round2-spec's "either/or" reading would
    /// suggest and which leaves the ship in STOP for ever.
    /// **A warping ship flies the curve and parks itself on arrival.**
    ///
    /// The wiring §52.5 promised. Two properties that the pure function
    /// could not cover on its own:
    ///
    /// 1. observing repeatedly -- which is the only way this server ever
    ///    advances motion -- walks the ship in, rather than re-deriving a
    ///    different flight each time;
    /// 2. dropout leaves `Mode::Stop` at the destination, so the *next*
    ///    observation integrates a parked ship and not a warping one.
    ///
    /// The clock is pinned through `advanced_at`, so this does not depend on
    /// when it runs.
    #[test]
    fn a_warping_ship_is_walked_in_by_repeated_observation_and_then_parks() {
        let goto = [4.0 * 149_597_870_700.0, 0.0, 0.0];
        let start = [0.0, 0.0, 0.0];
        let total = distance(start, goto);
        let activated = 1_792_800_000.0_f64;
        let hull = &crate::destiny::hull::FALLBACK;

        let mut m = Motion {
            position: start,
            velocity: [0.0; 3],
            mode: Mode::Warp {
                goto,
                effect_stamp: activated as i32,
                total_distance: total,
                stop_distance: 0.0,
                warp_factor: 4_000,
            },
            speed_fraction: 1.0,
        };

        let mut last = total;
        let mut parked_after = None;
        for step in 1..400 {
            let now = activated + f64::from(step);
            m = m.advanced_at(hull, 1.0, now);
            let remaining = distance(m.position, goto);
            if m.mode == Mode::Stop {
                parked_after = Some(step);
                break;
            }
            assert!(
                remaining <= last + 1.0,
                "step {step}: the ship went backwards, {remaining} > {last}"
            );
            last = remaining;
        }

        let step = parked_after.expect("a 4 AU warp parks within 400 observations");
        assert_eq!(m.position, goto, "it parked somewhere other than the point");
        assert_eq!(m.velocity, [0.0; 3], "a parked ship still has velocity");
        assert_eq!(m.mode, Mode::Stop, "the mode outlived the warp");
        assert!(
            step > 5,
            "a 4 AU warp finishing in {step} s is not a flight"
        );

        // And once parked it stays put: the next observation must integrate a
        // stopped ship, which is the half a `return` in the warp arm could
        // have broken without any of the above noticing.
        let after = m.advanced_at(hull, 10.0, activated + f64::from(step) + 10.0);
        assert_eq!(after.position, goto, "a parked ship drifted");
    }

    #[test]
    fn the_throttle_pair_changes_the_mode_and_then_the_fraction() {
        let mut m = Motion::arrived([1000.0, 0.0, 0.0], [0.0; 3]);
        let heading = command(
            7,
            "CmdGotoDirection",
            &[PyValue::Real(1.0), PyValue::Real(0.0), PyValue::Real(0.0)],
        )
        .expect("a heading is a command");
        m.after(heading);
        assert!(
            matches!(m.mode, Mode::Goto(_)),
            "the mode has to leave STOP or the client never accelerates: {m:?}"
        );
        assert_eq!(m.speed_fraction, 0.0, "a heading is not a throttle");

        let throttle = command(7, "CmdSetSpeedFraction", &[PyValue::Real(0.375)])
            .expect("a throttle is a command");
        m.after(throttle);
        assert!((m.speed_fraction - 0.375).abs() < 1e-6);
        assert!(
            matches!(m.mode, Mode::Goto(_)),
            "and the throttle must not undo the heading"
        );
    }

    /// A heading becomes a point the ship cannot reach, and a heading of
    /// nowhere cannot become a NaN.
    ///
    /// Pins the *finiteness*, not the magnitude: a NaN reaching a native is
    /// discarded with no exception and no log, so the only safe property is
    /// that one cannot be constructed here. The tempting wrong version asserts
    /// the point equals `position + 1e16 * dir`, which says nothing about the
    /// zero case that actually produces the NaN.
    #[test]
    fn a_heading_becomes_a_finite_point_even_when_it_has_no_length() {
        let mut m = Motion::arrived([17.0, 10_096.0, -12_607.0], [0.0; 3]);
        m.after(Action::GotoDirection {
            ball: 7,
            direction: [0.0, 0.0, 0.0],
        });
        let Mode::Goto(point) = m.mode else {
            panic!("a heading sets GOTO: {m:?}")
        };
        assert_eq!(point, [17.0, 10_096.0, -12_607.0], "nowhere is where it is");

        m.after(Action::GotoDirection {
            ball: 7,
            direction: [0.0, 1.0, 0.0],
        });
        let Mode::Goto(point) = m.mode else {
            panic!("a heading sets GOTO: {m:?}")
        };
        assert!(
            point.iter().all(|c| c.is_finite()),
            "a non-finite coordinate is discarded by the native in silence: {point:?}"
        );
        assert!(
            point[1] > 1e15,
            "and the destination is far enough that the ship never arrives: {point:?}"
        );
    }

    /// Follow and orbit carry the two fields the client compares before it
    /// bothers to send the command again.
    ///
    /// Pins that `followId` and `followRange` are **stored**, because
    /// `_IsAlreadyFollowingBallAtRange` suppresses a repeat command when the
    /// local ball already matches -- so a server that forgets them makes the
    /// player's second click do nothing. The tempting wrong version asserts
    /// only the mode code.
    #[test]
    fn follow_and_orbit_remember_their_target_and_range() {
        let mut m = Motion::arrived([0.0; 3], [0.0; 3]);
        m.after(
            command(
                7,
                "CmdFollowBall",
                &[PyValue::Int(99), PyValue::Real(500.0)],
            )
            .unwrap(),
        );
        assert_eq!(
            m.mode,
            Mode::Follow {
                ball_id: 99,
                range: 500.0
            }
        );
        m.after(command(7, "CmdOrbit", &[PyValue::Int(99), PyValue::Real(1000.0)]).unwrap());
        assert_eq!(
            m.mode,
            Mode::Orbit {
                ball_id: 99,
                range: 1000.0
            }
        );
        m.after(command(7, "CmdStop", &[]).unwrap());
        assert_eq!(m.mode, Mode::Stop);
    }

    /// Every command names the caller's own ship as the ball.
    ///
    /// Pins it across the **whole** mapping rather than one row: the client's
    /// three independent `OnBallparkCall` consumers all read `args[0]` as the
    /// ball id, and the bug this guards against is a row added later with the
    /// ship in the wrong slot.
    #[test]
    fn every_command_names_the_callers_ship() {
        let args = [PyValue::Real(1.0), PyValue::Real(2.0), PyValue::Real(3.0)];
        for method in COMMANDS {
            let a = command(4242, method, &args)
                .unwrap_or_else(|| panic!("{method} must map to an action"));
            assert_eq!(a.ball(), 4242, "{method}");
            assert_eq!(a.args()[0], PyValue::Int(4242), "{method}");
            assert!(is_command(method), "{method}");
        }
    }

    /// Arguments we cannot read produce no action at all.
    ///
    /// Pins "refuse rather than guess": a missing argument used to be the kind
    /// of thing that becomes a zero, and a `CmdFollowBall` with target zero is
    /// a command to follow a ball that does not exist -- which the native
    /// discards while the player watches nothing happen.
    #[test]
    fn an_unreadable_argument_is_not_a_zero() {
        assert!(command(7, "CmdSetSpeedFraction", &[]).is_none());
        assert!(command(7, "CmdGotoDirection", &[PyValue::Real(1.0)]).is_none());
        assert!(command(7, "CmdFollowBall", &[PyValue::None, PyValue::Real(1.0)]).is_none());
        assert!(command(7, "CmdWarpToStuff", &[PyValue::Int(1)]).is_none());
        assert!(
            UNIMPLEMENTED
                .iter()
                .all(|m| command(7, m, &args_of_three()).is_none()),
            "nothing in UNIMPLEMENTED may also be in the mapping"
        );
    }

    fn args_of_three() -> Vec<PyValue> {
        vec![PyValue::Real(1.0), PyValue::Real(2.0), PyValue::Real(3.0)]
    }

    /// The ship index equals a full scan after every kind of mutation.
    #[test]
    fn the_ship_index_matches_a_scan_after_every_mutation() {
        fn check(r: &Roster) {
            let ships: std::collections::BTreeSet<i64> =
                r.by_character.values().map(|p| p.ship_id).collect();
            for s in ships.iter().copied().chain([9, 123_456]) {
                let scan = r.by_character.values().find(|p| p.ship_id == s);
                assert_eq!(
                    r.by_ship(s).map(|p| p.character_id),
                    scan.map(|p| p.character_id)
                );
            }
            let indexed: usize = r.by_ship.values().map(|s| s.len()).sum();
            assert_eq!(indexed, r.by_character.len());
        }
        let mut r = Roster::default();
        for c in 1..=5 {
            r.arrive(presence(c, 1));
            check(&r);
        }
        // Replace: same character arrives again with another ship.
        let mut again = presence(3, 2);
        again.ship_id = 77;
        r.arrive(again);
        check(&r);
        assert_eq!(r.by_ship(77).map(|p| p.character_id), Some(3));
        assert!(r.by_ship(10_003).is_none());
        // Two records sharing a ship id: the lowest character answers.
        r.set_ship_id(5, 77);
        check(&r);
        assert_eq!(r.by_ship(77).map(|p| p.character_id), Some(3));
        r.leave(3);
        check(&r);
        assert_eq!(r.by_ship(77).map(|p| p.character_id), Some(5));
        // A motion change through get_mut leaves the index alone.
        r.get_mut(5).unwrap().motion.position = [9.0; 3];
        check(&r);
        r.leave(5);
        r.leave(99);
        check(&r);
        assert!(r.by_ship(77).is_none());
    }

    fn presence(character_id: i64, system: i64) -> Presence {
        Presence {
            character_id,
            ship_id: character_id + 10_000,
            solar_system_id: system,
            motion: Motion::arrived([1.0, 2.0, 3.0], [0.0; 3]),
            // A fixed stamp, not `now_seconds()`: a roster test that reads
            // the clock is a roster test that can fail at midnight.
            since: 0.0,
            ticked: None,
        }
    }

    /// A hull with a one-second time constant, so the arithmetic in these
    /// tests can be done by hand: `agility * mass / 1e6 == 1.0`.
    const UNIT_TAU: crate::destiny::hull::Hull = crate::destiny::hull::Hull {
        type_id: 0,
        mass: 1.0e6,
        radius: 1.0,
        max_velocity: 100.0,
        agility: 1.0,
        shield_capacity: 1.0,
        shield_recharge_rate: 1.0,
        armor_hp: 1.0,
        hp: 1.0,
        capacitor_capacity: 1.0,
        capacitor_recharge_rate: 1.0,
        // Irrelevant to an integrator, and present because the struct is
        // the ship's whole row rather than only its kinematics.
        max_target_range: 1.0,
        max_locked_targets: 1.0,
        scan_resolution: 1.0,
        signature_radius: 1.0,
        warp_speed_au: 1.0,
    };

    /// **A ship at rest advances the same with any hull** (PERF-STAGES):
    /// `Registry::mover` gives it the placeholder hull instead of building a
    /// dogma sheet. Pinned in both regimes -- the closed form, and a system
    /// where another ship orbits it, stepped on the tick grid.
    #[test]
    fn a_ship_at_rest_advances_the_same_with_any_hull() {
        let fallback = crate::destiny::hull::FALLBACK;
        let build = || {
            let mut r = Roster::default();
            for (c, x) in [(1, 0.0), (2, 5_000.0), (3, 9_000.0)] {
                let mut p = presence(c, 30_000_001);
                p.motion = Motion::arrived([x, 0.0, 0.0], [0.0; 3]);
                r.arrive(p);
            }
            r.get_mut(3).unwrap().motion = Motion {
                position: [9_000.0, 0.0, 0.0],
                velocity: [50.0, 0.0, 0.0],
                mode: crate::destiny::Mode::Goto([1.0e6, 0.0, 0.0]),
                speed_fraction: 1.0,
            };
            r
        };
        assert!(build().get(1).unwrap().motion.is_at_rest());
        assert!(!build().get(3).unwrap().motion.is_at_rest());
        let movers = |rest: crate::destiny::hull::Hull, tether: bool| {
            vec![
                Mover {
                    character_id: 1,
                    hull: rest,
                    tether: None,
                },
                Mover {
                    character_id: 2,
                    hull: UNIT_TAU,
                    tether: tether.then_some(Tether {
                        ball_id: 10_001,
                        range: 2_000.0,
                        at: [0.0; 3],
                    }),
                },
                Mover {
                    character_id: 3,
                    hull: UNIT_TAU,
                    tether: None,
                },
            ]
        };
        for tether in [false, true] {
            for now in [37.25, 4_000.5] {
                let (mut real, mut cheap) = (build(), build());
                real.advance(&movers(UNIT_TAU, tether), now);
                cheap.advance(&movers(fallback, tether), now);
                assert_eq!(real, cheap, "tether {tether}, now {now}");
            }
        }
    }

    /// PA-8: the revision counts the structural writes (`arrive`, `leave`,
    /// `set_ship_id`, a non-empty `advance`), not `get_mut`, and is not state.
    #[test]
    fn the_revision_counts_structural_writes_only() {
        let mut r = Roster::default();
        let r0 = r.revision();
        r.arrive(presence(7, 30_000_001));
        assert_eq!(r.revision(), r0 + 1);
        r.set_ship_id(7, 40_000);
        assert_eq!(r.revision(), r0 + 2);
        r.get_mut(7).expect("here").motion.speed_fraction = 0.5;
        assert_eq!(r.revision(), r0 + 2, "get_mut is not structural");
        r.advance(&[], 10.0);
        assert_eq!(r.revision(), r0 + 2, "no movers, no bump");
        let mover = Mover {
            character_id: 7,
            hull: crate::destiny::hull::FALLBACK,
            tether: None,
        };
        r.advance(&[mover], 10.0);
        assert_eq!(r.revision(), r0 + 3);
        assert!(r.leave(7).is_some());
        assert_eq!(r.revision(), r0 + 4);
        assert!(r.leave(7).is_none());
        assert_eq!(r.revision(), r0 + 4, "a leave of nobody is not a write");
        // Not state: equal rosters at different revisions compare equal.
        let mut a = Roster::default();
        a.arrive(presence(8, 30_000_001));
        let mut b = a.clone();
        b.arrive(presence(8, 30_000_001));
        assert_ne!(a.revision(), b.revision());
        assert_eq!(a, b);
    }

    /// A ship that arrives forgets where it was last time.
    ///
    /// Pins the dock-up loop's requirement: undocking twice must put the ship
    /// at the mouth twice, not at wherever the first flight ended. The
    /// tempting wrong version checks that `arrive` inserts -- true of a
    /// merge, which is the bug.
    #[test]
    fn arriving_in_space_forgets_the_previous_flight() {
        let mut roster = Roster::default();
        roster.arrive(presence(7, 30_000_001));
        roster
            .get_mut(7)
            .expect("just arrived")
            .motion
            .after(Action::SetBallPosition {
                ball: 10_007,
                point: [9.0, 9.0, 9.0],
            });
        assert_eq!(roster.get(7).map(|p| p.motion.position), Some([9.0; 3]));

        roster.leave(7);
        assert_eq!(roster.get(7), None);
        roster.arrive(presence(7, 30_000_001));
        assert_eq!(
            roster.get(7).map(|p| p.motion.position),
            Some([1.0, 2.0, 3.0]),
            "the second arrival starts at the mouth, not where the first \
             flight ended"
        );
    }

    /// **A stopping ship loses its vertical speed faster than its
    /// horizontal, because the client's does.**
    ///
    /// The law is asserted the way the client applies it -- `exp(-t/tau)` on
    /// every axis, times `0.869158879^t` on y alone, with `t` in seconds
    /// because the destiny step is one second (`docs/port-tracker.md` §55.2).
    /// The expected ratios are computed here from CCP's **two literals**
    /// rather than from our own constant, so this fails if
    /// [`STOP_Y_DAMPING_RATE`] is ever mistyped -- a test that reused the
    /// constant would agree with any value it was given.
    #[test]
    fn a_stopping_ship_sheds_vertical_speed_faster_than_horizontal() {
        let hull = &crate::destiny::hull::FALLBACK;
        let tau = f64::from(hull.agility) * hull.mass / 1.0e6;
        // **CCP's own literal, transcribed digit for digit.** It carries
        // more precision than an `f64` holds, which is exactly why clippy
        // objects and exactly why it stays: the value of this test is that it
        // quotes `Ballpark::EvolveStop` rather than agreeing with our reading
        // of it, and a literal trimmed to 17 digits no longer quotes
        // anything. The excess rounds to the same double, so nothing is lost
        // but the provenance -- and the provenance is the asset.
        #[allow(clippy::excessive_precision)]
        let k: f64 = (1.0 - 0.07) * 0.934_579_439_252_336_448_598_130_841_121_5;

        for t in [1.0_f64, 2.0, 5.0, 10.0] {
            let m = Motion {
                position: [0.0; 3],
                velocity: [300.0, 300.0, 300.0],
                mode: Mode::Stop,
                speed_fraction: 1.0,
            };
            let after = m.advanced_at(hull, t, 0.0);

            let want_xz = 300.0 * (-t / tau).exp();
            let want_y = want_xz * k.powf(t);
            for (axis, want) in [(0, want_xz), (1, want_y), (2, want_xz)] {
                assert!(
                    (after.velocity[axis] - want).abs() < 1.0e-6,
                    "t={t}s axis {axis}: {} != {want}",
                    after.velocity[axis],
                );
            }
            assert!(
                after.velocity[1] < after.velocity[0],
                "y must shed faster than x at t={t}s",
            );
        }
    }

    /// The damping is **`Mode::Stop` only**, because `EvolveStop` is the one
    /// evolve function that touches the velocity directly instead of
    /// returning an acceleration. A ship under way keeps all three axes on
    /// one time constant.
    #[test]
    fn the_y_damping_does_not_apply_to_a_ship_under_way() {
        let hull = &crate::destiny::hull::FALLBACK;
        let m = Motion {
            position: [0.0; 3],
            velocity: [0.0, 200.0, 200.0],
            mode: Mode::Goto([1.0e9, 0.0, 0.0]),
            speed_fraction: 1.0,
        };
        let after = m.advanced_at(hull, 3.0, 0.0);
        assert!(
            (after.velocity[1] - after.velocity[2]).abs() < 1.0e-9,
            "y and z must decay alike under GOTO: {:?}",
            after.velocity,
        );
    }

    /// The audience for "something happened to me" is the system, minus me.
    ///
    /// Pins both halves: everyone in the system is included, and the subject
    /// is not. The tempting wrong version asserts the length -- which passes
    /// while naming the wrong people -- or tests one system, which never
    /// exercises the filter that keeps two systems apart.
    #[test]
    fn the_audience_is_the_system_minus_the_subject() {
        let mut roster = Roster::default();
        roster.arrive(presence(1, 30_000_001));
        roster.arrive(presence(2, 30_000_001));
        roster.arrive(presence(3, 30_000_002));

        assert_eq!(roster.others_in_system(30_000_001, 1), vec![2]);
        assert_eq!(roster.others_in_system(30_000_001, 2), vec![1]);
        assert_eq!(
            roster.others_in_system(30_000_002, 3),
            Vec::<i64>::new(),
            "alone in a system hears from nobody"
        );
        assert_eq!(
            roster
                .in_system(30_000_001)
                .iter()
                .map(|p| p.character_id)
                .collect::<Vec<_>>(),
            vec![1, 2],
            "and the bootstrap's list does include the subject"
        );
    }

    /// **The positive control: our curve is the client's curve.**
    ///
    /// `hull::align_seconds` is the client's own published align time,
    /// `-ln(0.25) * tau`, and `-ln(0.25)` time constants is exactly how long
    /// an exponential takes to cover three quarters of its range. So a ship
    /// that has been accelerating for its align time must be at three
    /// quarters of its commanded speed -- a property of *their* formula,
    /// which this integrator either reproduces or does not.
    ///
    /// This is the test that would catch the integrator being a plausible
    /// invention rather than the client's physics, and it is the reason the
    /// time constant is read out of the align-time formula instead of being
    /// chosen.
    #[test]
    fn align_time_is_when_three_quarters_of_the_speed_is_there() {
        for h in crate::destiny::hull::FIXTURE {
            let mut m = Motion::arrived([0.0; 3], [0.0; 3]);
            m.mode = Mode::Goto([1.0e12, 0.0, 0.0]);
            m.speed_fraction = 1.0;
            let flown = m.advanced(h, crate::destiny::hull::align_seconds(h));
            let fraction = flown.speed() / f64::from(h.max_velocity);
            assert!(
                (fraction - 0.75).abs() < 1e-9,
                "type {} is at {fraction} of top speed after its own align \
                 time, which the client says is three quarters",
                h.type_id
            );
        }
    }

    /// A ship told to stop coasts `v0 * tau` metres and no further.
    ///
    /// The exact closed-form result for a zero commanded velocity, and the
    /// number the client's own simulation converges on. Pins the *total*,
    /// which is a property of the model, rather than any intermediate
    /// position, which would be a property of our arithmetic.
    #[test]
    fn a_ship_told_to_stop_coasts_its_speed_times_its_time_constant() {
        let m = Motion::arrived([0.0; 3], [250.0, 0.0, 0.0]);
        assert_eq!(m.mode, Mode::Stop, "arrived means told to stop");
        let tau = 1.0;
        let far = m.advanced(&UNIT_TAU, 1_000.0);
        assert!(
            (far.position[0] - 250.0 * tau).abs() < 1e-6,
            "coasted {} m, expected {} m",
            far.position[0],
            250.0 * tau
        );
        assert!(
            far.speed() < 1.0e-6,
            "and is at rest afterwards, not merely slower: {}",
            far.speed()
        );
    }

    /// Integrating twice over half the time is integrating once over all of
    /// it.
    ///
    /// The semigroup property, and the one test here that mentions no
    /// constant of ours at all: whatever the curve is, evolving it in two
    /// steps must agree with evolving it in one, or two observers who ask at
    /// different moments get two different worlds. A stepped integrator fails
    /// this and a closed form cannot, which is why the implementation is
    /// closed form.
    ///
    /// The destination is far enough away that the heading does not change
    /// over the interval, because the turn is the one part this treats as
    /// instantaneous.
    #[test]
    fn advancing_in_two_steps_lands_where_one_step_does() {
        let mut m = Motion::arrived([3.0, -4.0, 5.0], [10.0, 0.0, -2.0]);
        m.mode = Mode::Goto([1.0e14, 0.0, 0.0]);
        m.speed_fraction = 0.6;

        let one = m.advanced(&UNIT_TAU, 8.0);
        let two = m.advanced(&UNIT_TAU, 3.0).advanced(&UNIT_TAU, 5.0);
        assert!(
            distance(one.position, two.position) < 1e-6,
            "one step {:?} vs two {:?}",
            one.position,
            two.position
        );
        for axis in 0..3 {
            assert!(
                (one.velocity[axis] - two.velocity[axis]).abs() < 1e-9,
                "velocity disagrees on axis {axis}: {:?} vs {:?}",
                one.velocity,
                two.velocity
            );
        }
    }

    /// A ship at rest with nothing commanded stays exactly where it is, for
    /// any length of time.
    ///
    /// The case every observer hits most often -- a docked system's worth of
    /// ships sitting still -- and the one where a sloppy integrator leaks
    /// drift into a position that was correct.
    #[test]
    fn a_ship_at_rest_does_not_drift() {
        let m = Motion::arrived([17.0, 10_096.0, -12_607.0], [0.0; 3]);
        for dt in [0.001, 1.0, 3_600.0, 86_400.0 * 365.0] {
            let later = m.advanced(&UNIT_TAU, dt);
            assert_eq!(later.position, m.position, "drifted after {dt}s: {later:?}");
            assert_eq!(later.velocity, [0.0; 3]);
        }
    }

    /// A ship sent to a point arrives at it rather than sailing past.
    ///
    /// Only reachable for a real `CmdGotoPoint` -- a heading becomes a point
    /// [`GOTO_DISTANCE`] away. Pins "never beyond", which is the property;
    /// the tempting wrong version pins the arrival *time*, which depends on
    /// the curve.
    #[test]
    fn a_ship_sent_to_a_point_does_not_sail_past_it() {
        let mut m = Motion::arrived([0.0; 3], [0.0; 3]);
        m.mode = Mode::Goto([5_000.0, 0.0, 0.0]);
        m.speed_fraction = 1.0;
        let far = m.advanced(&UNIT_TAU, 10_000.0);
        assert_eq!(far.position, [5_000.0, 0.0, 0.0]);
        assert!(far.speed() < 1e-9, "and is parked there");

        let partway = m.advanced(&UNIT_TAU, 1.0);
        assert!(
            partway.position[0] > 0.0 && partway.position[0] < 5_000.0,
            "and gets there progressively: {:?}",
            partway.position
        );
    }

    /// **A ship moving away from its destination is not "arriving" at it.**
    ///
    /// Found in self-review. The arrival clamp compared the *unsigned*
    /// distance travelled against the distance remaining, which says nothing
    /// about direction -- so a ship with velocity pointing away from its goto
    /// point, which is exactly what undocking into a click behind you
    /// produces, satisfied `travelled >= remaining` while receding and was
    /// teleported onto the point.
    ///
    /// The shove out of a station mouth is up to 700 m/s, so this was
    /// reachable with one click on a nearby point in the first seconds of
    /// any flight.
    ///
    /// The property is directional: progress is the displacement *projected
    /// onto the line to the target*, so moving away is negative progress and
    /// cannot arrive.
    #[test]
    fn a_ship_receding_from_its_destination_cannot_arrive_at_it() {
        let mut m = Motion::arrived([0.0; 3], [-1_000.0, 0.0, 0.0]);
        m.mode = Mode::Goto([50.0, 0.0, 0.0]);
        m.speed_fraction = 1.0;

        let later = m.advanced(&UNIT_TAU, 1.0);
        assert!(
            later.position[0] < 0.0,
            "the ship is still travelling backwards at 1 km/s; it cannot \
             have reached a point 50 m ahead of where it started: {:?}",
            later.position
        );
        assert_ne!(
            later.position,
            [50.0, 0.0, 0.0],
            "teleported onto its destination while receding from it"
        );
        assert!(
            later.speed() > 0.0,
            "and it is certainly not parked: {later:?}"
        );
    }

    /// Flying *at* a point still arrives at it, which is the behaviour the
    /// fix above must not break.
    ///
    /// Kept beside the receding case deliberately: the two together pin the
    /// sign, which is the whole content of the correction.
    #[test]
    fn a_ship_flying_at_a_point_still_arrives_and_parks() {
        let mut m = Motion::arrived([0.0; 3], [500.0, 0.0, 0.0]);
        m.mode = Mode::Goto([50.0, 0.0, 0.0]);
        m.speed_fraction = 1.0;
        let later = m.advanced(&UNIT_TAU, 1.0);
        assert_eq!(later.position, [50.0, 0.0, 0.0], "{later:?}");
        assert!(later.speed() < 1e-9, "and parks there: {later:?}");
    }

    /// **A ship cannot orbit itself into the distance.**
    ///
    /// Also found in self-review. `Roster`-level tethering places a ship at
    /// its orbit range from the ball it is circling -- and nothing stopped
    /// that ball being its *own*. Each integration then moved it another
    /// `range` metres from wherever it already was, which is unbounded drift
    /// with no thrust behind it.
    ///
    /// `CmdOrbit` on your own ship is not something the UI offers, but it is
    /// a command the client could send and the client's own self-target guard
    /// applies only while piloting a structure `[V]`. A position that grows
    /// without a force is worth one comparison to prevent.
    ///
    /// Asserted at the `Registry` level, because the tether is resolved there
    /// and that is where the guard has to live.
    #[test]
    fn a_ship_told_to_orbit_itself_stays_where_it_is() {
        use crate::model::{Character, SandboxConfig};
        use crate::service::Registry;
        const ALICE: i64 = 1_000_001;
        let alice = Character {
            character_id: ALICE,
            name: "Alice".to_owned(),
            ship_id: 100_000_001,
            ..Character::default()
        };
        let mut reg = Registry::new(SandboxConfig {
            characters: vec![alice],
            ..SandboxConfig::default()
        });
        crate::destiny::on_ballpark_bind(&mut reg, Some(ALICE));
        let _ = reg.take_pending();
        let start = reg.roster().get(ALICE).expect("in space").motion.position;

        {
            let p = reg.roster_mut().get_mut(ALICE).expect("in space");
            p.motion.after(Action::Orbit {
                ball: p.ship_id,
                target: p.ship_id,
                range: 5_000.0,
            });
            p.since -= 10.0;
        }
        reg.advance_character(ALICE);
        let after_one = reg.roster().get(ALICE).expect("in space").motion.position;
        reg.advance_character(ALICE);
        let after_two = reg.roster().get(ALICE).expect("in space").motion.position;

        assert!(
            distance(start, after_one) < 5_000.0,
            "one integration moved it a whole orbit radius from nowhere: \
             {start:?} -> {after_one:?}"
        );
        assert_eq!(
            after_one, after_two,
            "and it keeps going, which is unbounded drift with no thrust"
        );
    }

    /// Nothing an integration is given can put a NaN in a position.    /// Nothing an integration is given can put a NaN in a position.
    ///
    /// A non-finite coordinate reaches a ball blob, goes into the native, and
    /// is **discarded in silence** -- no exception, no log, on either side.
    /// So the only safe property is that one cannot be constructed, and this
    /// asserts it over every degenerate input the callers could produce: no
    /// elapsed time, a negative one, an infinity, a hull with no time
    /// constant, a heading of nowhere.
    #[test]
    fn no_elapsed_time_and_no_bad_number_can_move_a_ship_to_nowhere() {
        let mut m = Motion::arrived([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]);
        m.speed_fraction = 1.0;
        for dt in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(m.advanced(&UNIT_TAU, dt), m, "dt {dt} changed the motion");
        }

        let no_tau = crate::destiny::hull::Hull {
            agility: 0.0,
            ..UNIT_TAU
        };
        assert_eq!(m.advanced(&no_tau, 10.0), m, "a hull with no time constant");

        // A heading of nowhere: GOTO its own position. The direction is a
        // zero vector, which is where a normalise produces a NaN.
        let mut here = Motion::arrived([9.0, 9.0, 9.0], [0.0; 3]);
        here.mode = Mode::Goto([9.0, 9.0, 9.0]);
        here.speed_fraction = 1.0;
        let later = here.advanced(&UNIT_TAU, 5.0);
        assert!(
            later.position.iter().all(|c| c.is_finite())
                && later.velocity.iter().all(|c| c.is_finite()),
            "{later:?}"
        );
    }

    /// Orbit and follow keep moving, and the record says so rather than
    /// pretending otherwise.
    ///
    /// `[A]`: without the target ball's position these can only be a
    /// straight-line cruise, so this asserts the one thing that is true of
    /// the approximation -- the ship does not stop and does not go to
    /// nowhere. It deliberately does **not** assert a position, because the
    /// position is known to be wrong for a long orbit, and a test that
    /// asserted it would be pinning a limitation in place.
    #[test]
    fn an_orbiting_ship_keeps_moving_even_though_we_cannot_say_where() {
        let mut m = Motion::arrived([0.0; 3], [50.0, 0.0, 0.0]);
        m.mode = Mode::Orbit {
            ball_id: 99,
            range: 1_000.0,
        };
        m.speed_fraction = 1.0;
        let later = m.advanced(&UNIT_TAU, 10.0);
        assert!(
            later.speed() > 50.0,
            "accelerating toward the commanded speed"
        );
        assert!(later.position.iter().all(|c| c.is_finite()));
        assert!(
            distance(m.position, later.position) > 0.0,
            "and it went somewhere"
        );
    }

    /// The unit normal [`Motion::tethered_in_plane`] projects onto, for the
    /// assertions below.
    fn plane_normal(ball_id: i64, tick: u32) -> [f64; 3] {
        unit(crate::orbit::orbital_normal(ball_id, tick)).expect("never short")
    }

    /// **An orbiting ship lands on its own orbit's circle.** Centred on the
    /// target, in the plane `orbital_normal` derives for *its* ball id at
    /// that tick, at the hull-gap centre distance -- every one of which is
    /// `[V]`. Where on the circle is `[A]` and is not asserted.
    ///
    /// The `Mode::Orbit` target id is deliberately different from every
    /// `ball_id` passed: the plane belongs to the orbiter, and a version that
    /// read the target's id from the mode would put the whole fleet on one
    /// ring.
    #[test]
    fn an_orbiting_ship_reconciles_onto_its_derived_plane_at_the_hull_gap() {
        let target = [1_000.0, -2_000.0, 30_000.0];
        let r =
            crate::orbit::centre_distance(500.0, 27.0, f64::from(crate::destiny::STATION_RADIUS));
        let starts = [
            [50_000.0, 0.0, 0.0],
            [0.0, 70_000.0, 12.0],
            [-3.0, -4.0, 5.0],
        ];
        let orbiters = [
            (1_000_000_000_000_i64, 0_u32),
            (1_000_000_000_001, 1_761_000_000),
            (70_123, 999_999),
        ];
        for (ball_id, tick) in orbiters {
            let n = plane_normal(ball_id, tick);
            for start in starts {
                let mut m = Motion::arrived(start, [0.0, 0.0, 300.0]);
                m.mode = Mode::Orbit {
                    ball_id: 60_000_004,
                    range: 500.0,
                };
                m.tethered_in_plane(target, r, ball_id, tick);
                let off = [
                    m.position[0] - target[0],
                    m.position[1] - target[1],
                    m.position[2] - target[2],
                ];
                let out_of_plane = off[0] * n[0] + off[1] * n[1] + off[2] * n[2];
                assert!(
                    out_of_plane.abs() < 1e-6,
                    "ball {ball_id} tick {tick} from {start:?}: {out_of_plane} m off the plane"
                );
                assert!(
                    (distance(m.position, target) - r).abs() < 1e-6,
                    "ball {ball_id} tick {tick} from {start:?}: not at the hull gap"
                );
            }
        }
    }

    /// **The velocity is re-aimed along Carbon's settled thrust**, `n x
    /// toVector` with `toVector` pointing at the target, and keeps its
    /// speed. That direction is what `EvolveOldStyleOrbit` thrusts along
    /// once `dist == r` `[V]`.
    #[test]
    fn a_reconciled_orbit_moves_along_the_settled_tangent_at_its_own_speed() {
        let (ball_id, tick) = (1_000_000_000_000_i64, 12_345_u32);
        let target = [0.0; 3];
        let mut m = Motion::arrived([8_000.0, 1_000.0, -500.0], [120.0, -40.0, 90.0]);
        m.mode = Mode::Orbit {
            ball_id: 60_000_004,
            range: 2_500.0,
        };
        let speed = m.speed();
        m.tethered_in_plane(target, 2_500.0, ball_id, tick);

        let n = plane_normal(ball_id, tick);
        let to = unit([-m.position[0], -m.position[1], -m.position[2]]).expect("off centre");
        let want = unit([
            n[1] * to[2] - n[2] * to[1],
            n[2] * to[0] - n[0] * to[2],
            n[0] * to[1] - n[1] * to[0],
        ])
        .expect("n is not along the radius");
        let got = unit(m.velocity).expect("still moving");
        for axis in 0..3 {
            assert!((got[axis] - want[axis]).abs() < 1e-9, "{got:?} != {want:?}");
        }
        assert!((m.speed() - speed).abs() < 1e-9, "speed changed");
    }

    /// A follow is not an orbit: `EvolveFollow` keeps the side the follower
    /// is on and has no plane (§54.1) `[V]`, so the plane-aware call must
    /// leave it exactly where `tethered_to` would.
    #[test]
    fn a_follow_is_untouched_by_the_orbital_plane() {
        let mut a = Motion::arrived([9_000.0, 4_000.0, -1_000.0], [10.0, 0.0, 0.0]);
        a.mode = Mode::Follow {
            ball_id: 60_000_004,
            range: 1_000.0,
        };
        let mut b = a;
        a.tethered_in_plane([0.0; 3], 1_000.0, 1_000_000_000_000, 0);
        b.tethered_to([0.0; 3], 1_000.0);
        assert_eq!(a, b);
    }

    /// A ship exactly on its plane's axis has no nearest point on the
    /// circle. It still lands on the circle, finitely.
    #[test]
    fn a_ship_on_the_planes_axis_still_lands_on_the_circle() {
        let (ball_id, tick) = (1_000_000_000_000_i64, 0_u32);
        let n = plane_normal(ball_id, tick);
        let mut m = Motion::arrived([n[0] * 5_000.0, n[1] * 5_000.0, n[2] * 5_000.0], [0.0; 3]);
        m.mode = Mode::Orbit {
            ball_id: 60_000_004,
            range: 1_000.0,
        };
        m.tethered_in_plane([0.0; 3], 1_000.0, ball_id, tick);
        assert!(m.position.iter().all(|c| c.is_finite()), "{m:?}");
        let along = m.position[0] * n[0] + m.position[1] * n[1] + m.position[2] * n[2];
        assert!(along.abs() < 1e-6, "{along}");
        assert!((distance(m.position, [0.0; 3]) - 1_000.0).abs() < 1e-6);
    }

    /// Being in space is a fact the world can answer, which it could not
    /// before.
    ///
    /// `Character::station_id` is never cleared, so it means "the station I
    /// undock from" and not "where I am". The station guest list filtered on
    /// it and so listed pilots who were out flying.
    #[test]
    fn the_roster_is_the_only_answer_to_whether_a_pilot_is_in_space() {
        let mut roster = Roster::default();
        assert!(!roster.is_in_space(7));
        roster.arrive(presence(7, 30_000_001));
        assert!(roster.is_in_space(7));
        assert_eq!(roster.leave(7).map(|p| p.ship_id), Some(10_007));
        assert!(!roster.is_in_space(7));
        assert_eq!(roster.leave(7), None, "leaving twice is not an event");
    }

    /// **The per-system index equals the scan** (W11 packet CP6, C7 §2.3),
    /// on random rosters: arrivals, re-arrivals in another system (a jump's
    /// leave + arrive collapsed into one replacing `arrive`) and departures,
    /// drawn from a fixed-seed generator so a failure replays. After every
    /// step, for every system, `in_system` and `others_in_system` answer what
    /// a linear filter over the presences answers, `systems` lists exactly
    /// the occupied ones, and an emptied system leaves no index entry.
    #[test]
    fn the_per_system_index_equals_the_scan_on_random_rosters() {
        // A 64-bit LCG (Knuth's MMIX constants): deterministic, std-only.
        let mut state: u64 = 0x5eed_cafe;
        let mut next = |n: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % n
        };
        const SYSTEMS: [i64; 4] = [30_000_001, 30_000_002, 30_000_003, 30_000_004];
        let mut roster = Roster::default();
        let mut truth: std::collections::BTreeMap<i64, Presence> = Default::default();
        for step in 0..2_000 {
            let id = 1 + next(40) as i64;
            if next(3) == 0 {
                assert_eq!(roster.leave(id), truth.remove(&id), "step {step}");
            } else {
                let p = presence(id, SYSTEMS[next(4) as usize]);
                roster.arrive(p);
                truth.insert(id, p);
            }
            for system in SYSTEMS {
                let scan: Vec<Presence> = truth
                    .values()
                    .filter(|p| p.solar_system_id == system)
                    .copied()
                    .collect();
                assert_eq!(
                    roster.in_system(system),
                    scan,
                    "step {step} system {system}"
                );
                let except = 1 + next(40) as i64;
                let others: Vec<i64> = scan
                    .iter()
                    .map(|p| p.character_id)
                    .filter(|c| *c != except)
                    .collect();
                assert_eq!(roster.others_in_system(system, except), others);
            }
            let occupied: Vec<i64> = roster.systems().map(|(s, _)| s).collect();
            let mut want: Vec<i64> = truth.values().map(|p| p.solar_system_id).collect();
            want.sort_unstable();
            want.dedup();
            assert_eq!(occupied, want, "step {step}: no empty system is indexed");
            assert_eq!(roster.len(), truth.len());
        }
    }

    // ---- FX9: a pair on the tick grid ------------------------------------

    /// A frigate-ish hull with round numbers: 300 m/s, `tau` 3 s. Invented.
    const PAIR_HULL: crate::destiny::hull::Hull = crate::destiny::hull::Hull {
        mass: 3.0e6,
        max_velocity: 300.0,
        ..UNIT_TAU
    };
    const PAIR_SYSTEM: i64 = 30_000_001;
    /// Invented ids; the low 16 bits differ, so the two planes do.
    const SHIP_A: i64 = 9_000_000_045;
    const SHIP_B: i64 = 1_000_000_123;
    const CENTRE: f64 = 2_080.0;

    /// Two ships told to orbit each other, as the record holds them at `t0`.
    fn mutual_pair(t0: f64) -> Roster {
        let mut roster = Roster::default();
        for (character_id, ship_id, other, x, vy) in [
            (1, SHIP_A, SHIP_B, 0.0, 300.0),
            (2, SHIP_B, SHIP_A, CENTRE, -300.0),
        ] {
            let mut motion = Motion::arrived([x, 0.0, 0.0], [0.0, vy, 0.0]);
            motion.after(Action::Orbit {
                ball: ship_id,
                target: other,
                range: CENTRE,
            });
            motion.speed_fraction = 1.0;
            roster.arrive(Presence {
                character_id,
                ship_id,
                solar_system_id: PAIR_SYSTEM,
                motion,
                since: t0,
                ticked: None,
            });
        }
        roster
    }

    /// What the registry hands [`Roster::advance`] for the pair.
    fn pair_movers(roster: &Roster) -> Vec<Mover> {
        [(1, SHIP_B), (2, SHIP_A)]
            .into_iter()
            .map(|(character_id, other)| Mover {
                character_id,
                hull: PAIR_HULL,
                tether: Some(Tether {
                    ball_id: other,
                    range: CENTRE,
                    at: roster.by_ship(other).expect("in space").motion.position,
                }),
            })
            .collect()
    }

    fn pair_positions(roster: &Roster) -> [[f64; 3]; 2] {
        [1, 2].map(|c| roster.get(c).expect("in space").motion.position)
    }

    /// Advance the pair from `t0` to `end`, one call every `every` seconds.
    fn advanced_every(t0: f64, end: f64, every: f64) -> Roster {
        let mut roster = mutual_pair(t0);
        let mut k = 1.0;
        while t0 + k * every < end {
            let movers = pair_movers(&roster);
            roster.advance(&movers, t0 + k * every);
            k += 1.0;
        }
        let movers = pair_movers(&roster);
        roster.advance(&movers, end);
        roster
    }

    /// **Where a mutual pair is does not depend on how often it is asked**
    /// (FX9; FX6 measured 79 km of difference between a 0.05 s and a 3 s
    /// cadence over the same 120 s).
    ///
    /// The tolerance is a micrometre and not a kilometre because the claim
    /// is exact: every cadence steps the same grid states through the same
    /// arithmetic from the same record, and only the last partial step
    /// differs in nothing at all. Anything above rounding is the defect.
    #[test]
    fn a_mutual_orbit_lands_in_the_same_place_whatever_the_cadence() {
        let t0 = 1_792_800_000.25;
        let end = t0 + 120.0;
        let reference = pair_positions(&advanced_every(t0, end, 1.0));
        for every in [0.05, 0.25, 0.7, 3.0, 7.3, 120.0] {
            let got = pair_positions(&advanced_every(t0, end, every));
            for (g, r) in got.iter().zip(reference.iter()) {
                assert!(
                    distance(*g, *r) < 1e-6,
                    "every {every} s: {g:?} against {r:?} at 1 s"
                );
            }
        }
    }

    /// **A pair's centre never outruns its ships.** Two balls that each fly
    /// at most at top speed have a midpoint that moves at most at top speed;
    /// before FX9 the pair's midpoint moved at about twice it (79 km in
    /// 120 s at 300 m/s), with no thrust to account for it. Checked every
    /// second for ten minutes, and the separation stays a separation (finite
    /// and nonzero), not a number running away.
    #[test]
    fn a_mutual_orbits_centre_never_outruns_its_ships() {
        let t0 = 1_792_800_000.0;
        let mut roster = mutual_pair(t0);
        let top = f64::from(PAIR_HULL.max_velocity);
        let centre = |r: &Roster| {
            let [a, b] = pair_positions(r);
            [
                (a[0] + b[0]) / 2.0,
                (a[1] + b[1]) / 2.0,
                (a[2] + b[2]) / 2.0,
            ]
        };
        let start = centre(&roster);
        let mut last = start;
        for second in 1..=600 {
            let movers = pair_movers(&roster);
            roster.advance(&movers, t0 + f64::from(second));
            let now = centre(&roster);
            assert!(
                distance(now, last) <= top * TICK_SECONDS + 1e-6,
                "second {second}: the centre moved {} m",
                distance(now, last)
            );
            let [a, b] = pair_positions(&roster);
            assert!(distance(a, b).is_finite() && distance(a, b) > 0.0);
            last = now;
        }
        assert!(distance(last, start) <= top * 600.0);
    }

    /// **The step is Carbon's**: the same pair, stepped by a transcription
    /// of `Ballpark::Evolve` written here from `Ballpark.cpp` and nothing
    /// else -- every ball's `EvolveOldStyleOrbit` thrust from the tick's
    /// start positions, then `Integrate` over one second -- lands where
    /// [`Roster::advance`] puts it, tick for tick.
    #[test]
    fn the_pair_is_stepped_as_carbons_evolve_steps_it() {
        fn trunc7(x: f64) -> f64 {
            (x * 1e7).trunc() / 1e7
        }
        fn norm(v: [f64; 3]) -> [f64; 3] {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l == 0.0 {
                v
            } else {
                [v[0] / l, v[1] / l, v[2] / l]
            }
        }
        // EvolveOldStyleOrbit, line by line.
        fn thrust(me: [f64; 3], other: [f64; 3], id: i64, t: f64, r: f64) -> [f64; 3] {
            let d = [other[0] - me[0], other[1] - me[1], other[2] - me[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let mut to = norm(d);
            let (p1, p2) = (t * 0.001, (id & 0xffff) as f64 + t * 0.001);
            let n = [
                trunc7(p1.cos() * p2.cos()),
                trunc7(p2.sin()),
                trunc7(p1.sin() * p2.cos()),
            ];
            let rad = norm([
                n[1] * to[2] - n[2] * to[1],
                n[2] * to[0] - n[0] * to[2],
                n[0] * to[1] - n[1] * to[0],
            ]);
            let to_comp = dist * dist - r * r;
            if to_comp >= 0.0 {
                let rad_comp = r * to_comp.sqrt() / dist;
                to = norm([0, 1, 2].map(|i| to_comp / dist * to[i] + rad_comp * rad[i]));
            }
            let rf = trunc7((-(r - dist) * (r - dist) / 40000.0).exp());
            let phi = -(to[0] * rad[0] + to[1] * rad[1] + to[2] * rad[2]);
            let disc = 1.0 + rf * rf * (phi * phi - 1.0);
            let mut tf = if disc > 0.0 {
                rf * phi + disc.sqrt()
            } else {
                rf * phi
            };
            if dist - r < 0.0 {
                tf = -tf;
            }
            [0, 1, 2].map(|i| rf * rad[i] + tf * to[i])
        }
        let t0 = 1_792_800_000.0;
        let mut roster = mutual_pair(t0);
        let mut balls: Vec<([f64; 3], [f64; 3], i64)> = [1, 2]
            .map(|c| {
                let p = roster.get(c).expect("in space");
                (p.motion.position, p.motion.velocity, p.ship_id)
            })
            .to_vec();
        let (m, k) = (PAIR_HULL.mass * f64::from(PAIR_HULL.agility), 1.0e6);
        let cruise = f64::from(PAIR_HULL.max_velocity);
        for tick in 0..90 {
            let t = t0 + f64::from(tick);
            // Pass one: every thrust from the start positions.
            let a: Vec<[f64; 3]> = (0..2)
                .map(|i| {
                    let a = thrust(balls[i].0, balls[1 - i].0, balls[i].2, t, CENTRE);
                    a.map(|c| c * k * cruise / m)
                })
                .collect();
            // Pass two: Ballpark::Integrate, dt = 1.
            for (b, a) in balls.iter_mut().zip(a) {
                let tf = (-k / m).exp();
                for (i, a) in a.into_iter().enumerate() {
                    let (p, v) = (b.0[i], b.1[i]);
                    b.0[i] =
                        (m * (m * a * (tf - 1.0) + k * (a + v - tf * v)) + p * k * k) / (k * k);
                    b.1[i] = (m * a - (m * a - v * k) * tf) / k;
                }
            }
            let movers = pair_movers(&roster);
            roster.advance(&movers, t + 1.0);
            for (c, b) in [1, 2].into_iter().zip(&balls) {
                let got = roster.get(c).expect("in space").motion.position;
                assert!(
                    distance(got, b.0) < 1e-3,
                    "tick {tick}, ball {c}: {got:?} against Carbon's {:?}",
                    b.0
                );
            }
        }
    }

    /// **A command between two advances is honoured**: the next advance
    /// starts from what was written, not from the kept grid state. A pair
    /// whose one half is told to stop slows that half down.
    #[test]
    fn a_command_between_advances_restarts_from_the_record() {
        let t0 = 1_792_800_000.5;
        let mut roster = mutual_pair(t0);
        let movers = pair_movers(&roster);
        roster.advance(&movers, t0 + 30.0);
        assert!(roster.get(1).expect("in space").ticked.is_some());
        let before = roster.get(1).expect("in space").motion.speed();
        roster
            .get_mut(1)
            .expect("in space")
            .motion
            .after(Action::Stop { ball: SHIP_A });
        let movers: Vec<Mover> = pair_movers(&roster)
            .into_iter()
            .map(|m| Mover {
                tether: if m.character_id == 1 { None } else { m.tether },
                ..m
            })
            .collect();
        roster.advance(&movers, t0 + 40.0);
        let after = roster.get(1).expect("in space").motion.speed();
        assert!(after < before / 2.0, "stopping: {before} -> {after} m/s");
    }

    /// **Without a pair, nothing changed**: a ship tethered to something
    /// that is not another mover is carried by the closed form and snapped
    /// to its circle exactly as `advance_character` did before FX9, and
    /// keeps no grid state.
    #[test]
    fn a_tether_to_a_fixed_ball_keeps_the_closed_form() {
        let t0 = 1_792_800_000.5;
        let mut roster = mutual_pair(t0);
        roster.leave(2);
        let station = [50_000.0, 0.0, 0.0];
        let mover = Mover {
            character_id: 1,
            hull: PAIR_HULL,
            tether: Some(Tether {
                ball_id: SHIP_B,
                range: CENTRE,
                at: station,
            }),
        };
        let record = roster.get(1).expect("in space").motion;
        let mut want = record.advanced_at(&PAIR_HULL, 25.0, t0 + 25.0);
        want.tethered_in_plane(station, CENTRE, SHIP_A, (t0 + 25.0) as u32);
        assert!(!roster.has_pair(PAIR_SYSTEM));
        roster.advance(&[mover], t0 + 25.0);
        let p = roster.get(1).expect("in space");
        assert_eq!((p.motion, p.ticked), (want, None));
    }

    /// **A follower is not dragged into its leader's warp** (live-fleetgrid):
    /// the follow snap used to put A at its range from B whatever the
    /// distance, so a wing rode its FC 16 AU and never left his park.
    #[test]
    fn a_follower_is_not_dragged_into_its_leaders_warp() {
        let t0 = 1_792_800_000.5;
        let mut roster = mutual_pair(t0);
        {
            let a = roster.get_mut(1).expect("in space");
            a.motion.after(Action::FollowBall {
                ball: SHIP_A,
                target: SHIP_B,
                range: CENTRE,
            });
        }
        {
            let b = roster.get_mut(2).expect("in space");
            b.motion.mode = Mode::Warp {
                goto: [1.0e12, 0.0, 0.0],
                effect_stamp: t0 as i32,
                total_distance: 1.0e12,
                stop_distance: 0.0,
                warp_factor: 3_000,
            };
            b.motion.speed_fraction = 1.0;
        }
        let start = roster.get(1).expect("in space").motion.position;
        for k in 1..=30 {
            let mut m = pair_movers(&roster);
            m[1].tether = None;
            roster.advance(&m, t0 + f64::from(k));
        }
        let [a, b] = pair_positions(&roster);
        // The first seconds are inside the tether's reach and may follow B a
        // little (the model's bounded error); the point is that A is not
        // carried along his warp.
        assert!(
            distance(start, a) < crate::grid::GRID_ENTER / 10.0,
            "A moved {} m",
            distance(start, a)
        );
        assert!(distance(a, b) > crate::grid::GRID_LEAVE);
    }

    /// **A follow of a moving ship keeps its snap (while within reach) and loses the cadence**:
    /// it is placed after the ship it follows each tick, so it reads at its
    /// range from where that ship *is* (the bot harness's
    /// `a_following_ship_reads_at_its_follow_range_from_the_ship_it_follows`
    /// pins the range), and where both end up does not depend on how often
    /// they were asked.
    #[test]
    fn a_follower_of_a_moving_ship_is_at_range_whatever_the_cadence() {
        let t0 = 1_792_800_000.75;
        let run = |every: f64| {
            let mut roster = mutual_pair(t0);
            {
                let b = roster.get_mut(2).expect("in space");
                b.motion.after(Action::GotoPoint {
                    ball: SHIP_B,
                    point: [1.0e9, 1.0e9, 0.0],
                });
            }
            {
                let a = roster.get_mut(1).expect("in space");
                a.motion.after(Action::FollowBall {
                    ball: SHIP_A,
                    target: SHIP_B,
                    range: CENTRE,
                });
            }
            let movers = |r: &Roster| {
                let mut m = pair_movers(r);
                m[1].tether = None;
                m
            };
            let end = t0 + 60.0;
            let mut k = 1.0;
            while t0 + k * every < end {
                let m = movers(&roster);
                roster.advance(&m, t0 + k * every);
                k += 1.0;
            }
            let m = movers(&roster);
            roster.advance(&m, end);
            pair_positions(&roster)
        };
        let reference = run(1.0);
        assert!((distance(reference[0], reference[1]) - CENTRE).abs() < 1e-6);
        for every in [0.1, 0.6, 4.0, 60.0] {
            let got = run(every);
            for (g, r) in got.iter().zip(reference.iter()) {
                assert!(
                    distance(*g, *r) < 1e-6,
                    "every {every} s: {g:?} against {r:?}"
                );
            }
        }
    }

    /// **Aligning is not warping**: a commanded warp whose activation stamp
    /// is ahead (or negative) is ordinary flight; the stamp coming due makes
    /// it active.
    #[test]
    fn a_commanded_warp_is_active_only_once_its_stamp_is_due() {
        let warp = |effect_stamp: i32| Motion {
            position: [0.0; 3],
            velocity: [0.0; 3],
            mode: Mode::Warp {
                goto: [1.0e9, 0.0, 0.0],
                effect_stamp,
                total_distance: 1.0e9,
                stop_distance: 0.0,
                warp_factor: 4_000,
            },
            speed_fraction: 1.0,
        };
        let aligning = warp(5_000);
        assert!(aligning.warp_commanded());
        assert!(!aligning.warp_active_at(4_999.5), "the stamp is ahead");
        assert!(aligning.warp_active_at(5_000.0), "due");
        let preparing = warp(-1);
        assert!(preparing.warp_commanded());
        assert!(
            !preparing.warp_active_at(1.0e12),
            "negative stamp: preparing"
        );
        let flying = Motion::arrived([0.0; 3], [0.0; 3]);
        assert!(!flying.warp_commanded() && !flying.warp_active_at(1.0e12));
    }
}
