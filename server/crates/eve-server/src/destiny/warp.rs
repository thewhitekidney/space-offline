// SPDX-License-Identifier: LicenseRef-NMSA-1.0 AND MIT
// Portions transcribed from carbonengine/destiny (src/Ballpark.cpp),
// Copyright (c) 2026 CCP Games, MIT License; see NOTICE.

//! Warp and the Destiny actions: [`Action`] and the warp flight model.
//!
//! Moved verbatim out of `destiny.rs` (IP36-c); `destiny` re-exports every
//! item here with `pub use warp::*`, so every caller's path is unchanged.
//! The module doc of [`super`] is the spec this code satisfies.

use super::hull;
use eve_marshal::PyValue;

#[cfg(doc)]
use super::ActionUpdate;

/// One Destiny action: a native ballpark method, its ball, and its arguments.
///
/// **This is the only way the server can make anything move.** Nothing on the
/// client applies a command locally: `michelle.GetRemotePark()` returns the
/// bound `beyonce` object itself (`michelle.py:511-516`), `Michelle` has only
/// two `Cmd*` methods of its own and both are pure forwarders, and
/// `CmdSetSpeedFraction` does not occur in `michelle.pyc` at all **[V]**. A
/// throttle drag that we answer and do not echo gives a working slider, a
/// confident "Approaching..." caption, a drawn trajectory line -- and a ship
/// that does not move. `docs/lanes/movement.md` §1 has the proof.
///
/// Each variant's argument tuple is `[V]`, from one of two sources: an
/// in-client probe that ran against a private `destiny.Ballpark` and whose
/// readback shows the effect (`findings/destiny-ref/00-REPORT.txt`), or the
/// client's own `flightControls.FlightSimulation`, which builds a park of its
/// own at `flightControls.py:166` and calls the same natives on it. See
/// `docs/lanes/movement.md` §3 for the table and which row rests on which.
///
/// **The first argument is always the ball id**, confirmed by three
/// independent `OnBallparkCall` consumers -- `tacticalNavigationService.py:608`
/// and `:613-614`, `statefulButtonController.py:56`, `tactical.py:199-203`
/// **[V]**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// The throttle. Not in `scatterEvents`, so no `OnBallparkCall`: the HUD's
    /// gauge reads the native simulation instead.
    SetSpeedFraction { ball: i64, fraction: f64 },
    /// Align to a direction. **Is** in `scatterEvents`, so the stateful
    /// buttons and the tactical overlay both light up from it.
    GotoDirection { ball: i64, direction: [f64; 3] },
    /// Fly to a point. In `scatterEvents`.
    GotoPoint { ball: i64, point: [f64; 3] },
    /// Come to a halt. In `scatterEvents`.
    Stop { ball: i64 },
    /// Approach and hold at range. In `scatterEvents`. The client reads
    /// `args[2]` as the range and scatters its own
    /// `OnClientEvent_ApproachExecuted` from it
    /// (`tacticalNavigationService.py:616-636`) **[V]**.
    FollowBall { ball: i64, target: i64, range: f64 },
    /// Orbit at range. In `scatterEvents`.
    Orbit { ball: i64, target: i64, range: f64 },
    /// Teleport a ball. **Outside every dispatch set**, so it resets nothing
    /// and scatters nothing -- which is what makes it the cheap correction for
    /// an observer whose predicted position has drifted.
    SetBallPosition { ball: i64, point: [f64; 3] },
    /// Set a ball's velocity outright. Outside every dispatch set, as above.
    SetBallVelocity { ball: i64, velocity: [f64; 3] },
    /// Change a ball's top speed. Outside every dispatch set.
    SetMaxSpeed { ball: i64, max: f64 },
    /// Change a ball's mass in kg (`park.SetBallMass`, an f64 at record offset
    /// 0x2b). The client's ego ball learns a mass change only from here
    /// (MJD-MASS). Outside every dispatch set.
    SetBallMass { ball: i64, mass: f64 },
    /// Change a ball's agility (attribute 70, modified; the blob's f32 at 0x60).
    /// Outside every dispatch set.
    SetBallAgility { ball: i64, agility: f64 },
    /// Set or clear a ball's `isMassive` flag (`["SetBallMassive", [id, 0|1]]`,
    /// ints, as EvEJS sends it). **Outside every dispatch set.** A ship's ball
    /// is non-massive on TQ; the client's native defaults it massive and
    /// collides massive balls (CAPTURE-UNDOCK, EVEJS-MOTION).
    SetBallMassive { ball: i64, massive: bool },
    /// Set or clear a ball's `isInteractive` flag (`["SetBallInteractive", [id, 0|1]]`,
    /// ints). In `Park.scatterEvents`, so the client dispatches it to the native.
    /// The Board menu is offered only on a ball whose flag is clear (a ship with
    /// no pilot, bit 0x08). **Outside every dispatch set.** The native argument
    /// shape is **[A]** (`tactical` reads `argTuple[1] == 1`, as for
    /// `SetBallMassive`; EvEJS sends the same); falsifier: the flag bit does not
    /// change on the client's ball after the call.
    SetBallInteractive { ball: i64, interactive: bool },
    /// Cloak a ball: `("CloakBall", (ball, mode))`. The native reaches it as
    /// `_parent_CloakBall(ball, mode)`, which "needs at least 2 arguments"
    /// (`findings/destiny-ref` probe 72), and `michelle.RealFlushState` then
    /// removes the ball locally for every client but the ego's. Outside
    /// every dispatch set. `mode` is a `crate::cloak::MODE_*` **[A]**.
    CloakBall { ball: i64, mode: u8 },
    /// Uncloak a ball. **[A]** one argument (the native's arity); falsifier:
    /// the client logs "UncloakBall failed." with a `TypeError`.
    UncloakBall { ball: i64 },
    /// Enter warp. In `scatterEvents`.
    ///
    /// **The one action whose argument list is settled three independent
    /// ways**, which is worth stating because it was `[V2]`-only for two
    /// rounds and that was the stated reason warp could not be built:
    ///
    /// 1. the client's own `PyArg_ParseTuple` format string **`"Lddd|di"`**
    ///    at `_destiny.dll 0x1800acf88` -- a `long long`, three doubles, then
    ///    an optional double and an optional int;
    /// 2. the call into `Ballpark::WarpTo` at `0x1800928d2`, which passes
    ///    them in that order;
    /// 3. `spaceMgr.OnBallparkCall:524`, which reads `args[1..3]` as `x, y,
    ///    z` into `warpDestinationCache` -- so the argument order also decides
    ///    whether the HUD's "Warp destination" text and distance readout are
    ///    right, not only the physics.
    ///
    /// `target` is the **true** destination; the client applies `min_range`
    /// itself, so sending an already-offset target lands the ship twice as
    /// short. `warp_factor` is milli-AU/s, `int(au_per_second * 1000)`
    /// (`eveCommonUtils.AUPerSecondToDestinyWarpSpeed`) **[V]**, and the
    /// native clamps `<= 0` to `1` -- which is 0.001 AU/s and looks exactly
    /// like a hang, so [`warp_factor_in`] clamps instead.
    WarpTo {
        ball: i64,
        target: [f64; 3],
        min_range: f64,
        warp_factor: i64,
    },
    /// Launch a missile ball at a target. **Outside `scatterEvents`**; the
    /// client dispatches it as `_parent_LaunchMissile`, the native, because
    /// `Park` has no Python method of that name (`michelle.RealFlushState`'s
    /// `else` branch) **[V]**.
    ///
    /// The format string is **`"LLLbb"`** `[V]` (`_destiny.dll` `.rdata`,
    /// immediately before `Can not launch missile on a negative ballID`):
    /// `(srcID, dstID, ownerID, aimedLaunch, massive)` from Carbon's
    /// `PyLaunchMissile` `[V]`. `ball` is the missile, `target` the ball it
    /// follows, `owner` the launcher ball; the server always sends
    /// `aimed = massive = true` (R-MISSILE §1.3). The missile ball must
    /// already be in the park, so it goes after the `AddBalls2` that adds it,
    /// in the same stamp.
    ///
    /// `[A]` a marshal `Bool` passes `PyArg_ParseTuple`'s `b` format (a
    /// Python 2 `bool` is an `int`). **Falsifier:** the client logs
    /// `LaunchMissile failed`, or the missile is added and never flies.
    LaunchMissile {
        ball: i64,
        target: i64,
        owner: i64,
        aimed: bool,
        massive: bool,
    },
}

/// Milli-AU/s for a hull, which is what the action carries.
///
/// `int(au_per_second * 1000)` from CCP's own
/// `AUPerSecondToDestinyWarpSpeed`, where AU/s is
/// `attributeBaseWarpSpeed` (1281) x `attributeWarpSpeedMultiplier` (600) --
/// the same product the client's fitting window prints **[V]**. Our table
/// carries it pre-multiplied because `baseWarpSpeed` is `1.0` for all
/// nineteen hulls.
///
/// **Clamped to at least 1000.** The native clamps anything `<= 0` to `1`,
/// which is 0.001 AU/s: a ship that takes four hours to cross a system and
/// looks precisely like a server that has hung. One AU/s is slow and
/// unmistakably moving.
///
/// The hull's numbers come from `content` ([`hull::of_in`]).
pub fn warp_factor_in(content: &crate::content::Content, ship_type_id: i64) -> i64 {
    factor_of(hull::of_in(content, ship_type_id))
}

fn factor_of(h: &hull::Hull) -> i64 {
    let factor = (h.warp_speed_au * 1000.0).round();
    if !factor.is_finite() || factor < 1000.0 {
        return 1_000;
    }
    factor as i64
}

/// Where a warping ball is, and how fast, `t` seconds after activation.
///
/// Returns `(position, velocity, finished)`. `finished` is the **dropout**,
/// and it is deliberately not a time test: see below.
///
/// # The curve, from CCP's own source `[V]`
///
/// `Ballpark::WarpDistance` computes position from the *remaining* distance
/// rather than by integrating, which is what makes it usable here -- this
/// server advances motion lazily at observation time (§37), so a closed form
/// in `t` is exactly the shape it needs and an incremental one is not.
///
/// Three phases, with `k_a = warp_factor/1000` and
/// `k_d = min(warp_factor/3000, 2)`:
///
/// | phase | speed | distance covered |
/// |---|---|---|
/// | accelerate | `k_a·e^(k_a·t)` | `e^(k_a·t)` |
/// | cruise | `v_c` | `v_c·(t − t_a) + d_a` |
/// | decelerate | `v_c·e^(−k_d·Δt)` | `v_c/k_d·(1 − e^(−k_d·Δt)) + d_a + d_c` |
///
/// and the position is `goto − (d_a + d_c + d_d − covered)·dir`.
///
/// **The `+1` in `v_cruise` is what makes the three distances sum to the
/// flight.** `d_a = v_c/k_a` and `d_d = v_c/k_d`, so with no cruise phase
/// `d_a + d_d = (d+1)` exactly -- and `covered` starts at `e^0 = 1`, so the
/// ball starts the remaining-distance `d` from `goto`. The apparent
/// off-by-one in [`warp_flight_seconds`] and in §48.3's comparison with our
/// own curve is this, and it cancels.
///
/// # Dropout is a speed test `[V]`
///
/// `speed < min(max_velocity/2, 100)` -- §48.4. **Not** elapsed time, which
/// is what this server used to assume and what [`warp_flight_seconds`] still
/// computes for the *scheduler*. The two agree on a nominal flight and
/// diverge whenever the clock and the curve do: a dilated tick, an
/// interrupted warp, or a hull fast enough that `max_velocity/2` binds
/// instead of the 100 m/s floor. On dropout CCP snaps the ball to `goto`
/// with zero velocity, and restores `isMassive` and the sensor (§48.2).
///
/// # What this does not do
///
/// **The align phase is not here.** `t` is measured from *activation*, and
/// everything before it -- pointing the ship, reaching 0.75 of top speed,
/// the client's 180-tick watchdog -- is the ordinary goto integrator's job
/// and §34's. A caller with a negative `effect_stamp` has a ball that has
/// not activated and must not call this.
pub fn warp_trajectory(
    position: [f64; 3],
    goto: [f64; 3],
    total_distance: f64,
    warp_factor: i64,
    max_velocity: f64,
    current_speed: f64,
    t: f64,
) -> ([f64; 3], [f64; 3], bool) {
    // **The length is given, not measured.** §49 shipped this computing
    // `d` as the distance from the passed position to `goto`, which is only
    // the warp's length while the ship is still at the point it activated
    // from. Called again mid-flight -- which is exactly how lazy
    // integration calls it -- that measures the *remaining* distance,
    // derives a smaller `v_c` from it, and reports a ship flying a shorter,
    // slower warp on every observation.
    //
    // CCP does not make that mistake and the reason is instructive: it
    // reads the total from `mLastCollision`, the field §46 identified, and
    // takes only the *direction* from the current position. The ball is
    // always on the line, so the direction is the same from anywhere on it
    // while the length is not -- which is why the total is a stored field
    // and not something the simulation recomputes.
    let d = total_distance;
    let Some(dir) = crate::space_state::unit([
        goto[0] - position[0],
        goto[1] - position[1],
        goto[2] - position[2],
    ]) else {
        // Already there: no line to fly along, so the warp is over.
        return (goto, [0.0; 3], true);
    };
    let k_a = warp_factor as f64 * 0.001;
    let k_d = (warp_factor_decel(warp_factor)).max(f64::MIN_POSITIVE);
    if !t.is_finite() || t < 0.0 || k_a <= 0.0 || !k_a.is_finite() || !d.is_finite() || d <= 0.0 {
        return (position, [0.0; 3], false);
    }

    const AU: f64 = 149_597_870_700.0;
    let v_c = ((d + 1.0) * k_a * k_d / (k_a + k_d)).min(k_a * AU);
    if v_c <= 0.0 || !v_c.is_finite() {
        return (goto, [0.0; 3], true);
    }
    let d_a = v_c / k_a;
    let d_d = v_c / k_d;
    let t_a = (v_c / k_a).ln() / k_a;
    let d_c = (d + 1.0 - d_a - d_d).max(0.0);
    let t_c = d_c / v_c;
    let total = d_a + d_c + d_d;

    // **The dropout is tested only in the deceleration phase**, and the
    // first version of this tested it in all three. That is wrong in a way
    // a test caught immediately: at `t = 0` the curve's speed is `k_a`
    // itself -- 4 m/s for a capsule -- which is far below a 100 m/s floor,
    // so a global test ends every warp on the tick it begins.
    //
    // CCP nests it inside the decel branch and says why: it is what
    // "ensures that very fast ships still spend a bit of time in the
    // end-warp phase". A ship accelerating *through* the floor is not
    // landing; a ship decelerating through it is. §48.4 stated this as a
    // flat condition and was imprecise.
    let t_decel_end = t_a + t_c;
    let (speed, covered, landing) = if t < t_a {
        let e = (k_a * t).exp();
        // **The upward clamp of §48.3, and it is narrower than that section
        // implied.** CCP's `WarpDistance` acceleration branch is:
        //
        // ```c
        // speed = accelRate * exp(accelRate * t);
        // double currSpeed = v.Length();
        // if (currSpeed > speed)
        //     // Faking the speed to be continuous even though it actually isn't
        //     speed = currSpeed;
        // v = speed * dir;
        // distance = exp(accelRate * t);
        // ```
        //
        // So the clamp reaches **`v` and not `distance`** -- the trajectory is
        // untouched and only the *reported* velocity is raised. CCP's own
        // comment says why it exists, and it is candid: the curve starts at
        // `accelRate` itself, 4 m/s for a capsule, while a ship that just
        // crossed the 0.75 align gate is doing hundreds. Without the clamp an
        // observer would see a ship entering warp appear to brake hard for a
        // second and then accelerate, which is a discontinuity in the one
        // quantity a player can see on the HUD.
        //
        // §48.3 said we "do not clamp upwards, so a ship entering warp faster
        // than the curve predicts does not decelerate to meet it". The first
        // half is what was missing; the second half overstated it -- nothing
        // ever decelerated, because position never depended on the clamp.
        // The defect was a velocity readout, not a trajectory.
        (
            (k_a * e).max(if current_speed.is_finite() {
                current_speed
            } else {
                0.0
            }),
            e,
            false,
        )
    } else if t - t_a < t_c {
        (v_c, v_c * (t - t_a) + d_a, false)
    } else {
        let dt = t - t_decel_end;
        let e = (-k_d * dt).exp();
        (v_c * e, d_d * (1.0 - e) + d_a + d_c, true)
    };

    if !speed.is_finite() || !covered.is_finite() {
        return (position, [0.0; 3], false);
    }
    let floor = (max_velocity / 2.0).min(100.0);
    if landing && speed < floor {
        return (goto, [0.0; 3], true);
    }

    let remaining = total - covered;
    let at = [
        goto[0] - remaining * dir[0],
        goto[1] - remaining * dir[1],
        goto[2] - remaining * dir[2],
    ];
    let velocity = [speed * dir[0], speed * dir[1], speed * dir[2]];
    if !at.iter().all(|c| c.is_finite()) {
        return (position, [0.0; 3], false);
    }
    (at, velocity, false)
}

/// `k_d`, kept in one place because two functions need it and §44's review
/// found EvEJS diverging from Carbon here by their own admission.
fn warp_factor_decel(warp_factor: i64) -> f64 {
    (warp_factor as f64 / 3000.0).min(2.0)
}

/// How long a warp takes, in seconds, from command to standstill.
///
/// **CCP's own warp model, coefficient for coefficient.** Read out of
/// `Ballpark::OnDeactivatingWarp` -- the coefficients at `_destiny.dll`
/// `0x180042b20`-`0x180042bcf`, the phase times at `0x180042c04`-`0x180042cc9`
/// **[V]**, with the constants from `.rdata`: `0.001` at `0x1800ad7f0`,
/// `1/3000` at `0x1800ad7e8`, the `2.0` cap at `0x1800ad8a0`, and one AU as
/// `149597870700.0` at `0x1800ad940`.
///
/// ```text
/// k_accel  = warp_factor * 0.001                       AU/s
/// k_decel  = min(2.0, warp_factor / 3000.0)            AU/s   (= min(k_accel/3, 2))
/// v_cruise = min((d + 1) * k_accel*k_decel/(k_accel+k_decel), k_accel * AU)   m/s
/// t_accel  = ln(v_cruise / k_accel) / k_accel
/// t_decel  = ln(v_cruise / k_decel) / k_decel
/// t_cruise = d/v_cruise - 1/k_accel - 1/k_decel + 1/v_cruise
/// ```
///
/// ## The align phase is the hull's published align time, exactly
///
/// `docs/re/warp.md` §1.3 budgets `[A]` **15 seconds** for align, on the
/// grounds that it "is not predictable server-side from first principles".
/// **It is, and the number was already in our own hull table.** The client
/// activates the warp when `speed² > (0.75 * maxVelocity)²` (§34), and a ship
/// accelerating from rest reaches three quarters of its commanded speed at
/// `-ln(0.25) * tau` -- which is the definition of
/// [`hull::Hull::align_seconds`]. The align gate and the published align time
/// are the same quantity, which is why warping is full throttle (§34.3): the
/// gate is calibrated to the figure in the fitting window.
///
/// So the budget here is `align_seconds * ALIGN_BUDGET` rather than a flat
/// fifteen. `[A]`, and the factor is doing one specific job: a ship that has
/// to *reverse* an existing velocity before it can point at the target spends
/// longer turning than one starting from rest, and the heading gate
/// (`|1 - dot| < 0.01`) has to close too. Three align times covers a full
/// reversal with room over. **Falsified by** a human test where the warp
/// tunnel visibly lingers more than a few seconds after the ship is clearly
/// parked, which means the budget is too generous and the turn should be
/// modelled from the velocity we already hold.
///
/// ## Always long rather than short
///
/// Every rounding here is upward and every margin is additive, because the two
/// failure directions are not comparable. The client arrives and stops by
/// itself (`docs/re/warp.md` §7), so a **late** reconciliation lands on a ball
/// already in mode `STOP` at the right position and `Stop`'s own
/// implementation is `if ball.mode != 2: StopBall(ball)` -- a no-op **[V]**.
/// An **early** one aborts a warp in flight wherever the ship happens to be,
/// which looks like a teleport and leaves the two positions disagreeing.
pub fn warp_flight_seconds(distance: f64, warp_factor: i64, align_seconds: f64) -> f64 {
    /// One AU in metres, as the client holds it (`0x1800ad940`) **[V]**.
    const AU: f64 = 149_597_870_700.0;
    /// Align budgets, in multiples of the hull's own align time. See above.
    const ALIGN_BUDGET: f64 = 3.0;
    /// Ticks of slack on top, in seconds. Destiny ticks at 1 Hz and the gate
    /// is a strict inequality, so the activation lands on the tick *after*
    /// the align time; one second covers that and one covers the ticker's own
    /// `push::TICK` granularity.
    const SLACK: f64 = 2.0;

    let align = if align_seconds.is_finite() && align_seconds > 0.0 {
        align_seconds * ALIGN_BUDGET
    } else {
        0.0
    };
    let d = if distance.is_finite() && distance > 0.0 {
        distance
    } else {
        return align + SLACK;
    };
    let k_accel = warp_factor as f64 * 0.001;
    if k_accel <= 0.0 || !k_accel.is_finite() {
        // `warp_factor` is clamped by `warp_factor_in()` and cannot be zero, but
        // a zero here would be a division that produces an infinity and a
        // schedule a century away, so it is refused rather than trusted.
        return align + SLACK;
    }
    let k_decel = (warp_factor as f64 / 3000.0).min(2.0);
    let v_cruise = ((d + 1.0) * k_accel * k_decel / (k_accel + k_decel)).min(k_accel * AU);
    if v_cruise <= 0.0 || !v_cruise.is_finite() {
        return align + SLACK;
    }
    let t_accel = (v_cruise / k_accel).ln() / k_accel;
    let t_decel = (v_cruise / k_decel).ln() / k_decel;
    // Negative for a short hop -- the ship never reaches cruise, and the
    // model's own expression says so. Clamping at zero is what makes the sum
    // the real flight time rather than an underestimate.
    let t_cruise = (d / v_cruise - 1.0 / k_accel - 1.0 / k_decel + 1.0 / v_cruise).max(0.0);
    let total = align + t_accel.max(0.0) + t_cruise + t_decel.max(0.0) + SLACK;
    if total.is_finite() {
        total
    } else {
        align + SLACK
    }
}

/// Where a ship sent to `target` with `min_range` actually ends up.
///
/// `target` offset back along the line the ship is approaching from, which is
/// what `Ballpark::ActivateWarp` computes (`0x18004287e`-`0x180042951`)
/// **[V]** -- except that the native uses the ship's position at the
/// *activation* tick and this uses it at the *command* tick. For the default
/// right-click warp `min_range` is `0` and the two agree exactly; for a "warp
/// to within 100 km" across several AU the align drift is a few kilometres on
/// a 100 km offset.
///
/// A ship already at its destination has no line to be offset along, and gets
/// the target itself rather than a NaN.
pub fn warp_arrival(from: [f64; 3], target: [f64; 3], min_range: f64) -> [f64; 3] {
    if !min_range.is_finite() || min_range <= 0.0 {
        return target;
    }
    let back = crate::space_state::unit([
        from[0] - target[0],
        from[1] - target[1],
        from[2] - target[2],
    ]);
    match back {
        Some(u) => [
            target[0] + u[0] * min_range,
            target[1] + u[1] * min_range,
            target[2] + u[2] * min_range,
        ],
        None => target,
    }
}

impl Action {
    /// The native method name, which the client reaches as
    /// `getattr(self, '_parent_' + funcName)(*args)` (`michelle.py:1581`)
    /// **[V]**.
    pub fn name(self) -> &'static str {
        match self {
            Action::SetSpeedFraction { .. } => "SetSpeedFraction",
            Action::GotoDirection { .. } => "GotoDirection",
            Action::GotoPoint { .. } => "GotoPoint",
            Action::Stop { .. } => "Stop",
            Action::FollowBall { .. } => "FollowBall",
            Action::Orbit { .. } => "Orbit",
            Action::SetBallPosition { .. } => "SetBallPosition",
            Action::SetBallVelocity { .. } => "SetBallVelocity",
            Action::SetMaxSpeed { .. } => "SetMaxSpeed",
            Action::SetBallMass { .. } => "SetBallMass",
            Action::SetBallAgility { .. } => "SetBallAgility",
            Action::SetBallMassive { .. } => "SetBallMassive",
            Action::SetBallInteractive { .. } => "SetBallInteractive",
            Action::CloakBall { .. } => "CloakBall",
            Action::UncloakBall { .. } => "UncloakBall",
            Action::WarpTo { .. } => "WarpTo",
            Action::LaunchMissile { .. } => "LaunchMissile",
        }
    }

    /// The ball this acts on, which is also `args()[0]`.
    pub fn ball(self) -> i64 {
        match self {
            Action::SetSpeedFraction { ball, .. }
            | Action::GotoDirection { ball, .. }
            | Action::GotoPoint { ball, .. }
            | Action::Stop { ball }
            | Action::FollowBall { ball, .. }
            | Action::Orbit { ball, .. }
            | Action::SetBallPosition { ball, .. }
            | Action::SetBallVelocity { ball, .. }
            | Action::SetMaxSpeed { ball, .. }
            | Action::SetBallMass { ball, .. }
            | Action::SetBallAgility { ball, .. }
            | Action::SetBallMassive { ball, .. }
            | Action::SetBallInteractive { ball, .. }
            | Action::CloakBall { ball, .. }
            | Action::UncloakBall { ball }
            | Action::WarpTo { ball, .. }
            | Action::LaunchMissile { ball, .. } => ball,
        }
    }

    /// The positional arguments, splatted by the client as `(*args)`.
    ///
    /// Floats are `Real` and ids are `Int`: every probe passed Python floats,
    /// and `flightControls` passes `trinity` vector components, which are
    /// floats **[V]**.
    pub fn args(self) -> Vec<PyValue> {
        let ball = || PyValue::Int(self.ball());
        let triple = |v: [f64; 3]| {
            vec![
                ball(),
                PyValue::Real(v[0]),
                PyValue::Real(v[1]),
                PyValue::Real(v[2]),
            ]
        };
        match self {
            Action::SetSpeedFraction { fraction, .. } => vec![ball(), PyValue::Real(fraction)],
            Action::GotoDirection { direction, .. } => triple(direction),
            Action::GotoPoint { point, .. } | Action::SetBallPosition { point, .. } => {
                triple(point)
            }
            Action::Stop { .. } => vec![ball()],
            Action::FollowBall { target, range, .. } | Action::Orbit { target, range, .. } => {
                vec![ball(), PyValue::Int(target), PyValue::Real(range)]
            }
            Action::SetBallVelocity { velocity, .. } => triple(velocity),
            Action::SetMaxSpeed { max, .. } => vec![ball(), PyValue::Real(max)],
            Action::SetBallMass { mass, .. } => vec![ball(), PyValue::Real(mass)],
            Action::SetBallAgility { agility, .. } => vec![ball(), PyValue::Real(agility)],
            Action::SetBallMassive { massive, .. } => {
                vec![ball(), PyValue::Int(i64::from(massive))]
            }
            Action::SetBallInteractive { interactive, .. } => {
                vec![ball(), PyValue::Int(i64::from(interactive))]
            }
            Action::CloakBall { mode, .. } => vec![ball(), PyValue::Int(i64::from(mode))],
            Action::UncloakBall { .. } => vec![ball()],
            // Six arguments, and the two optional ones are always sent: the
            // native pre-seeds them with `20000.0` and `20`, which would be a
            // 20 km offset nobody asked for at 0.02 AU/s.
            Action::WarpTo {
                target,
                min_range,
                warp_factor,
                ..
            } => vec![
                ball(),
                PyValue::Real(target[0]),
                PyValue::Real(target[1]),
                PyValue::Real(target[2]),
                PyValue::Real(min_range),
                PyValue::Int(warp_factor),
            ],
            // "LLLbb": three ids, then two flags.
            Action::LaunchMissile {
                target,
                owner,
                aimed,
                massive,
                ..
            } => vec![
                ball(),
                PyValue::Int(target),
                PyValue::Int(owner),
                PyValue::Bool(aimed),
                PyValue::Bool(massive),
            ],
        }
    }

    /// Whether every float in this action is finite.
    ///
    /// **`_destiny.dll` discards a NaN argument in silence.** It carries one
    /// warning string per entry point -- `NaN in GotoDirection. Ignored`,
    /// `NaN in SetSpeedFraction. Ignored`, `NaN in FollowBall. Ignored`,
    /// `NaN in Orbit. Ignored`, `NaN in GotoPoint. Ignored` **[V]**, raw
    /// offsets in `docs/lanes/movement.md` §3.1 -- and a discarded action
    /// raises nothing in `RealFlushState`'s `try/except`, logs nothing on a
    /// closed channel, and leaves no trace on our side at all. The way we
    /// would produce one is the obvious way: a direction normalised by
    /// dividing by a zero length.
    ///
    /// So the guard is here, where the bytes are, rather than at each call
    /// site. [`ActionUpdate::new`] refuses a batch containing one.
    pub fn is_finite(self) -> bool {
        self.args().iter().all(|a| match a {
            PyValue::Real(f) => f.is_finite(),
            _ => true,
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// **The flight model, against CCP's own published numbers.**
    ///
    /// Not an assertion on one arithmetic result -- that would only pin
    /// whatever this function happens to compute. Three properties that a
    /// wrong implementation of the warp model breaks, and a scale check:
    ///
    /// 1. **monotonic in distance.** Further is never quicker.
    /// 2. **monotonic in warp speed.** A faster hull is never slower over the
    ///    same distance.
    /// 3. **never shorter than the align budget**, because the ship has to
    ///    point itself before the engine will activate at all (§34), and an
    ///    arrival scheduled before the warp has even started would abort the
    ///    warp mid-flight -- the one failure direction `warp_flight_seconds`
    ///    exists to avoid.
    /// 4. **the right order of magnitude.** A 5 AU warp at 3 AU/s is single
    ///    digit seconds of cruise plus the accelerate and decelerate ramps;
    ///    anything under a second or over five minutes means a unit error, and
    ///    a unit error here is a ship teleporting or an arrival that never
    ///    comes.
    #[test]
    fn the_warp_flight_model_is_monotonic_and_the_right_size() {
        const AU: f64 = 149_597_870_700.0;
        // A cruiser-ish 3 AU/s, and a pod's near-zero align.
        let t5 = warp_flight_seconds(5.0 * AU, 3_000, 0.09);
        let t50 = warp_flight_seconds(50.0 * AU, 3_000, 0.09);
        assert!(t5 < t50, "further must not be quicker: {t5} vs {t50}");
        let slow = warp_flight_seconds(5.0 * AU, 1_000, 0.09);
        let fast = warp_flight_seconds(5.0 * AU, 6_000, 0.09);
        assert!(
            fast < slow,
            "a faster hull must not take longer: {fast} vs {slow}"
        );
        for align in [0.09, 1.0, 13.9] {
            let t = warp_flight_seconds(5.0 * AU, 3_000, align);
            assert!(
                t > align,
                "an arrival inside the align phase would abort the warp: \
                 align {align}, scheduled {t}"
            );
        }
        assert!(
            (2.0..300.0).contains(&t5),
            "5 AU at 3 AU/s should be seconds, not milliseconds or hours: {t5}"
        );
        // Degenerate inputs must not produce a schedule a century away: an
        // infinity here becomes a `Duration` and then a batch nobody ever
        // sends, which is indistinguishable from the bug this replaced.
        for d in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let t = warp_flight_seconds(d, 3_000, 1.0);
            assert!(
                t.is_finite() && t > 0.0 && t < 60.0,
                "distance {d} gave {t}"
            );
        }
    }

    /// Align time is exactly when the client's own warp gate opens.
    ///
    /// The positive control for using [`hull::align_seconds`] as the align
    /// budget instead of `docs/re/warp.md` §1.3's flat 15 seconds. The client
    /// activates a warp when `speed² > (0.75 * maxVelocity)²` (§34) and a ship
    /// accelerating from rest reaches `0.75 * u` at `-ln(0.25) * tau`, which is
    /// the definition of the published align time. So the two numbers are the
    /// same quantity, and this asserts it over every hull rather than
    /// believing the algebra.
    #[test]
    fn the_published_align_time_is_when_the_warp_gate_opens() {
        for h in hull::FIXTURE {
            let align = hull::align_seconds(h);
            let tau = f64::from(h.agility) * h.mass / 1e6;
            let u = f64::from(h.max_velocity);
            // Speed at the published align time, from rest.
            let v = u * (1.0 - (-align / tau).exp());
            assert!(
                (v - 0.75 * u).abs() < 1e-6 * u,
                "{}: at its published align time of {align} s a ship from rest \
                 is at {v} m/s, and the client's warp gate opens at {} m/s",
                h.type_id,
                0.75 * u
            );
        }
    }

    /// One of every [`Action`], for the tests that must cover all of them.
    ///
    /// The exhaustive `match` in
    /// [`every_action_names_its_ball_first`] is the forcing function: adding a
    /// variant without adding a sample here stops that test compiling.
    pub(in crate::destiny) const EVERY_ACTION: [Action; 17] = [
        Action::SetSpeedFraction {
            ball: 4242,
            fraction: 0.375,
        },
        Action::GotoDirection {
            ball: 4242,
            direction: [1.0, 0.0, 0.0],
        },
        Action::GotoPoint {
            ball: 4242,
            point: [4004.0, 5005.0, 6006.0],
        },
        Action::Stop { ball: 4242 },
        Action::FollowBall {
            ball: 4242,
            target: 99,
            range: 500.0,
        },
        Action::Orbit {
            ball: 4242,
            target: 99,
            range: 1000.0,
        },
        Action::SetBallMassive {
            ball: 4242,
            massive: false,
        },
        Action::SetBallInteractive {
            ball: 4242,
            interactive: false,
        },
        Action::CloakBall {
            ball: 4242,
            mode: 1,
        },
        Action::UncloakBall { ball: 4242 },
        Action::SetBallPosition {
            ball: 4242,
            point: [1001.0, 2002.0, 3003.0],
        },
        Action::SetBallVelocity {
            ball: 4242,
            velocity: [111.0, 222.0, 333.0],
        },
        Action::SetMaxSpeed {
            ball: 4242,
            max: 2500.5,
        },
        Action::SetBallMass {
            ball: 4242,
            mass: 1.5e9,
        },
        Action::SetBallAgility {
            ball: 4242,
            agility: 0.75,
        },
        Action::WarpTo {
            ball: 4242,
            target: [1.0e9, -2.0e9, 3.0e9],
            min_range: 100_000.0,
            warp_factor: 5_000,
        },
        Action::LaunchMissile {
            ball: 4242,
            target: 99,
            owner: 98,
            aimed: true,
            massive: true,
        },
    ];

    /// Every action names its ball first, and every float is a `Real`.
    ///
    /// Three independent `OnBallparkCall` consumers read `args[0]` as the ball
    /// id -- `tacticalNavigationService.py:608`,
    /// `statefulButtonController.py:56`, `tactical.py:199-203` **[V]** -- so
    /// this loops over **every** variant rather than one: the bug it guards
    /// against is a variant added later with the ball in the wrong slot.
    ///
    /// The `Real` half matters because the natives took Python floats in every
    /// probe, and a round number is exactly the kind of thing that arrives as
    /// an integer. Asserting the *value* would not catch it.
    #[test]
    fn every_action_names_its_ball_first() {
        for action in EVERY_ACTION {
            // Exhaustive on purpose: a new variant must be added to
            // `EVERY_ACTION` as well, and this is what makes forgetting it a
            // compile error rather than a silent gap in coverage.
            match action {
                Action::SetSpeedFraction { .. }
                | Action::GotoDirection { .. }
                | Action::GotoPoint { .. }
                | Action::Stop { .. }
                | Action::FollowBall { .. }
                | Action::Orbit { .. }
                | Action::SetBallPosition { .. }
                | Action::SetBallVelocity { .. }
                | Action::SetMaxSpeed { .. }
                | Action::SetBallMass { .. }
                | Action::SetBallAgility { .. }
                | Action::SetBallMassive { .. }
                | Action::SetBallInteractive { .. }
                | Action::CloakBall { .. }
                | Action::UncloakBall { .. }
                | Action::WarpTo { .. }
                | Action::LaunchMissile { .. } => {}
            }
            let args = action.args();
            let name = action.name();
            assert_eq!(args[0], PyValue::Int(4242), "{name} must name its ball");
            assert_eq!(args[0], PyValue::Int(action.ball()), "{name}");
            for (i, arg) in args.iter().enumerate().skip(1) {
                // `Bool` only for `LaunchMissile`'s `bb` flags.
                assert!(
                    matches!(arg, PyValue::Real(_) | PyValue::Int(_))
                        || matches!(
                            (action, arg),
                            (Action::LaunchMissile { .. }, PyValue::Bool(_))
                        ),
                    "{name} argument {i} is a number: {arg:?}"
                );
            }
        }
        // Three actions carry one `Int` after the ball, `LaunchMissile`
        // carries two (`"LLLbb"`), and the rest are all floats; sending `Int(0)` for a speed fraction is the mistake this
        // pins. The two id-carrying actions name a target ball, and
        // `WarpTo`'s is `warpFactor`, which the client's own format string
        // `"Lddd|di"` ends with as an **int32** -- a `Real` there is a
        // `TypeError` in `PyArg_ParseTuple` and the whole action is
        // discarded.
        for action in EVERY_ACTION {
            let ints = action
                .args()
                .iter()
                .skip(1)
                .filter(|a| matches!(a, PyValue::Int(_)))
                .count();
            let expected = match action {
                Action::FollowBall { .. }
                | Action::Orbit { .. }
                | Action::WarpTo { .. }
                | Action::SetBallMassive { .. }
                | Action::SetBallInteractive { .. }
                | Action::CloakBall { .. } => 1,
                // `LLL`: the target and the owner after the missile.
                Action::LaunchMissile { .. } => 2,
                _ => 0,
            };
            assert_eq!(ints, expected, "{}: {:?}", action.name(), action.args());
        }
    }

    /// `LaunchMissile`'s marshalled form is the `"LLLbb"` shape of
    /// R-MISSILE §1.3: the missile, the target, the launcher, then two flags.
    #[test]
    fn launch_missile_is_lllbb_naming_the_missile_first() {
        let action = Action::LaunchMissile {
            ball: 9_500_000_000_001,
            target: 100_045_002,
            owner: 100_045_001,
            aimed: true,
            massive: true,
        };
        assert_eq!(action.name(), "LaunchMissile");
        assert_eq!(action.ball(), 9_500_000_000_001);
        assert_eq!(
            action.args(),
            vec![
                PyValue::Int(9_500_000_000_001),
                PyValue::Int(100_045_002),
                PyValue::Int(100_045_001),
                PyValue::Bool(true),
                PyValue::Bool(true),
            ]
        );
        assert!(action.is_finite());
        let off = Action::LaunchMissile {
            ball: 9_500_000_000_001,
            target: 100_045_002,
            owner: 100_045_001,
            aimed: false,
            massive: false,
        };
        assert_eq!(
            off.args()[3..],
            [PyValue::Bool(false), PyValue::Bool(false)]
        );
    }

    /// **The curve is the same curve when asked from halfway along it.**
    ///
    /// The falsification for the defect §52 fixes. Lazy integration calls
    /// `warp_trajectory` again at every observation, from wherever the ship
    /// has got to -- so if the function measured its own length from the
    /// position it was handed, each call would describe a *shorter* warp
    /// than the last: a smaller `v_c`, a slower ship, and a trajectory that
    /// disagrees with the one before it.
    ///
    /// Asking from the midpoint with the same stored total must give the
    /// same answer as asking from the start. The old signature could not
    /// satisfy this; the test that shipped with §49 could not see it,
    /// because it only ever called from the activation point.
    #[test]
    fn asking_the_curve_from_mid_flight_describes_the_same_warp() {
        let from = [0.0, 0.0, 0.0];
        let goto = [4.0 * 149_597_870_700.0, 0.0, 0.0];
        let d = crate::space_state::distance(from, goto);
        let (wf, max_vel) = (4_000, 3_000.0);

        // Fly it from the start, and remember where it was at each second.
        let mut track = Vec::new();
        for step in 1..200 {
            let t = f64::from(step);
            let (p, v, done) = warp_trajectory(from, goto, d, wf, max_vel, 0.0, t);
            if done {
                break;
            }
            track.push((t, p, v));
        }
        assert!(
            track.len() > 20,
            "expected a long flight, got {}",
            track.len()
        );

        // Now re-ask from each of those positions, with the same stored
        // total, for the same absolute `t`. The answer must not move.
        for &(t, p, v) in &track {
            let (p2, v2, done2) = warp_trajectory(p, goto, d, wf, max_vel, 0.0, t);
            assert!(!done2, "re-asking at t={t} ended the warp");
            let drift = crate::space_state::distance(p, p2);
            assert!(
                drift < 1.0,
                "at t={t} the position moved {drift} m just by being asked \
                 from where the ship already was"
            );
            let (s1, s2) = (
                (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt(),
                (v2[0] * v2[0] + v2[1] * v2[1] + v2[2] * v2[2]).sqrt(),
            );
            assert!(
                (s1 - s2).abs() < 1.0,
                "at t={t} the speed changed from {s1} to {s2} on re-asking: \
                 the length is being measured from the position again"
            );
        }
    }

    /// **The warp trajectory starts where the ship is and ends on the
    /// point, and it drops out on speed.**
    ///
    /// Four properties, each of which has a way of being wrong:
    ///
    /// 1. `t = 0` puts the ball at `from`. The `+1` bookkeeping of §48.3
    ///    makes this the non-obvious one -- `covered` starts at `e^0 = 1`,
    ///    not 0, and the remaining distance has to come out as `d` anyway.
    /// 2. Progress is monotonic toward the point.
    /// 3. Dropout snaps to the point exactly, with zero velocity.
    /// 4. The dropout threshold is `min(max_velocity/2, 100)` and is
    ///    therefore **hull-dependent** -- a slow hull coasts to a lower
    ///    speed before it lands. A time-based model cannot show this
    ///    difference at all, which is why it is asserted.
    #[test]
    fn a_warp_flies_the_curve_from_where_it_started_to_the_point() {
        let from = [0.0, 0.0, 0.0];
        let goto = [4.0 * 149_597_870_700.0, 0.0, 0.0];
        let d = crate::space_state::distance(from, goto);
        let wf = 4_000; // the capsule's 4.0 AU/s
        let max_vel = 3_000.0;

        // 1. Starts where it started.
        let (p0, v0, done0) = warp_trajectory(from, goto, d, wf, max_vel, 0.0, 0.0);
        assert!(!done0, "a warp is not over before it starts");
        assert!(
            crate::space_state::distance(p0, from) < 1.0,
            "t=0 put the ball {} m from where it was",
            crate::space_state::distance(p0, from)
        );
        assert!(
            v0[0] > 0.0 && v0[1] == 0.0,
            "it should be heading at the point"
        );

        // 2. Monotonic, and never past the point.
        let mut last = d;
        let mut landed_at = None;
        for step in 0..=600 {
            let t = f64::from(step);
            let (p, _, done) = warp_trajectory(from, goto, d, wf, max_vel, 0.0, t);
            let remaining = crate::space_state::distance(p, goto);
            if done {
                landed_at = Some(t);
                break;
            }
            assert!(
                remaining <= last + 1.0,
                "at t={t} the ball moved backwards: {remaining} > {last}"
            );
            last = remaining;
        }

        // 3. It lands, and exactly on the point.
        let t_land = landed_at.expect("a 4 AU warp finishes inside 600 s");
        let (p, v, done) = warp_trajectory(from, goto, d, wf, max_vel, 0.0, t_land);
        assert!(done);
        assert_eq!(p, goto, "dropout must snap to the point, not near it");
        assert_eq!(v, [0.0; 3], "a landed ball is stopped");

        // 4. The threshold is the hull's, not the clock's. A hull with a low
        //    top speed has a lower floor, so it stays in warp longer.
        let slow = 60.0; // floor 30 m/s
        let fast = 3_000.0; // floor 100 m/s, the cap
        let land = |mv: f64| {
            (0..6_000)
                .map(|i| f64::from(i) * 0.1)
                .find(|&t| warp_trajectory(from, goto, d, wf, mv, 0.0, t).2)
                .expect("it lands")
        };
        let (t_slow, t_fast) = (land(slow), land(fast));
        assert!(
            t_slow > t_fast,
            "a hull whose floor is 30 m/s should stay in warp longer than one \
             at the 100 m/s cap, got slow={t_slow} fast={t_fast}"
        );
    }
}
