// SPDX-License-Identifier: LicenseRef-NMSA-1.0 AND MIT
// Portions transcribed from carbonengine/destiny (src/Ballpark.cpp),
// Copyright (c) 2026 CCP Games, MIT License; see NOTICE.

//! The plane an orbiting ship circles in, and the radius it circles at.
//!
//! Both come from Carbon's own orbit integrator, not from a guess. The two
//! facts in here are the ones that decide where a joining observer draws the
//! bracket of a ship that has been orbiting something for a minute, which
//! [`crate::space_state::Motion::tethered_to`] had to approximate because
//! nothing told it any better. They are used by
//! [`crate::space_state::Motion::tethered_in_plane`], which puts an orbiting
//! ship on its circle -- the plane and radius `[V]`, the projection onto it
//! ours, and the point on the circle still `[A]`.
//!
//! **Which orbit is the live one.** `Ballpark::EvolveOrbit`
//! (`Ballpark.cpp:1116`) branches on `g_useNewOrbit`, and
//! `Settings.h:13` sets `g_useNewOrbitDefault = false` `[V]`. So the
//! exponential-spiral `EvolveNewStyleOrbit` right below it -- the longer,
//! newer, more interesting one -- is **not** what a default client runs;
//! `EvolveOldStyleOrbit` (`Ballpark.cpp:1261`) is. This is the second time a
//! `g_use*Default = false` has pointed at the less obvious branch, after the
//! y-damping gate of `docs/port-tracker.md` §51, and the first time it would
//! have cost a day: the spiral shares no term with the law below.

/// Carbon's `ORBITAL_PRECESSION` (`Ballpark.cpp:70`) `[V]`.
///
/// Radians per tick, so the orbital plane makes a full turn in `2*pi/0.001`
/// = 6,283 ticks, a little under two minutes at 100 ms per tick. An orbit is
/// not a fixed ring: it precesses the whole time.
pub const ORBITAL_PRECESSION: f64 = 0.001;

/// Truncate to seven decimal digits, the way Carbon does before it uses any
/// transcendental result.
///
/// `(double)((int64_t)(x*10000000))/10000000`, from the comment that admits
/// why: "`exp()` returns slightly different results depending on target
/// platform (32bit or 64bit) so we round to 7 significant decimal digits".
/// It is a **truncation toward zero**, not a rounding -- `sin(4096)`, which
/// this module's own first case runs through, is `-0.5946419876...` and
/// comes out `-0.5946419`, where a rounding would give `-0.5946420` -- so
/// `trunc` is the operation and not `round`.
///
/// Carbon does this so that two machines agree. We do it for the same reason
/// and one more: our answer has to agree with *theirs*, and seven digits of
/// a direction is 100 m of lateral error at 1,000 km, which is bracket-width.
fn truncated(x: f64) -> f64 {
    (x * 10_000_000.0).trunc() / 10_000_000.0
}

/// The normal of the plane a ball orbits in, at a given tick.
///
/// From `Ballpark::DefaultOrbitalNormal` (`Ballpark.cpp:735-750`), which
/// `EvolveOldStyleOrbit` inlines at `:1272-1281` `[V]`:
///
/// ```text
/// phi1 = tick * ORBITAL_PRECESSION
/// phi2 = (ball_id & 0xffff) + tick * ORBITAL_PRECESSION
/// n    = (cos(phi1)*cos(phi2), sin(phi2), sin(phi1)*cos(phi2))
/// ```
///
/// **This retires a claim we had been making.** `Motion::tethered_to` said
/// that where a ship sits around the thing it orbits "depends on the orbit
/// plane the client chose and the angle it has swept, neither of which the
/// server is told". The first half is simply wrong: the client chooses
/// nothing. The plane is a pure function of the ball id and the tick, both
/// of which the server owns -- it *assigns* the ball id. It is the same
/// shape of finding as the warp-in point being seeded by `ballID`: what
/// looks like client-side state is a shared formula over data we already
/// have.
///
/// **Why the low 16 bits.** Carbon's own comment: "this ensures that
/// different ships orbiting the same target will have different orbital
/// planes". Ball ids one apart give planes nowhere near each other, which is
/// the point -- a fleet ordered to orbit one station does not stack up.
///
/// **The result is deliberately not a unit vector.** The truncation leaves
/// its length at about `0.9999999`, and Carbon does not renormalise it; it
/// crosses the normal with the direction to the target and normalises *that*
/// instead. Normalising here would make us disagree with the client in the
/// seventh digit for no gain, so we return what it returns.
pub fn orbital_normal(ball_id: i64, tick: u32) -> [f64; 3] {
    let phi1 = f64::from(tick) * ORBITAL_PRECESSION;
    let phi2 = ((ball_id & 0xffff) as f64) + f64::from(tick) * ORBITAL_PRECESSION;
    [
        truncated(phi1.cos() * phi2.cos()),
        truncated(phi2.sin()),
        truncated(phi1.sin() * phi2.cos()),
    ]
}

/// How far apart the *centres* of two balls are when one is orbiting or
/// following the other at `range`.
///
/// `range + own_radius + other_radius`, which is how both
/// `Ballpark::EvolveFollow` (`Ballpark.cpp:1086`) and
/// `EvolveOldStyleOrbit` (`:1264`) compute their `r` `[V]`:
///
/// ```text
/// double r = (double)ball->mFollowRange + (double)ball->mRadius + (double)other->mRadius;
/// ```
///
/// **"Orbit at 500" means 500 metres of empty space**, hull surface to hull
/// surface -- not 500 metres between centres. We had been reconciling an
/// orbiting ship to bare `range` from the target's centre, so a ship told to
/// orbit the station at 500 m was placed 500 m from the station's origin:
/// 15,233 m *inside* a model whose radius is 15,733 m. That is the same
/// defect as warping to a station's centre, which `Service::ball_target`
/// already carries a paragraph about, arrived at from the other direction.
///
/// A negative or non-finite `range` is the caller's problem and comes back
/// unchanged, because clamping here would hide it.
pub fn centre_distance(range: f64, own_radius: f64, other_radius: f64) -> f64 {
    if !range.is_finite() {
        return range;
    }
    range + own_radius + other_radius
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The oracle is CPython's own `math`, run over Carbon's expression, the
    /// same way `warp_in` checks its draw against CPython's Mersenne
    /// Twister. Both sit on the platform libm and IEEE doubles, so an
    /// agreement to the last digit means we transcribed the formula and not
    /// merely its shape.
    #[test]
    fn the_orbital_plane_matches_carbons_own_expression() {
        let cases = [
            (1_000_000_000_000_i64, 0_u32, [0.8039906, -0.5946419, 0.0]),
            (1_000_000_000_001, 0, [0.9347719, 0.3552483, 0.0]),
            (
                1_000_000_000_000,
                12_345,
                [0.6378528, -0.7566609, -0.1435545],
            ),
            (70_123, 999_999, [0.1790541, 0.9481176, 0.2627022]),
        ];
        for (ball, tick, want) in cases {
            let got = orbital_normal(ball, tick);
            for axis in 0..3 {
                assert!(
                    (got[axis] - want[axis]).abs() < 1.0e-9,
                    "ball {ball} tick {tick} axis {axis}: {} != {}",
                    got[axis],
                    want[axis],
                );
            }
        }
    }

    /// The truncation is toward zero on both signs. A `round` would pass the
    /// positive half of this and fail the negative, which is exactly the
    /// mistake the name "shitround" in Carbon's comment invites.
    #[test]
    fn seven_digits_are_truncated_and_not_rounded() {
        assert_eq!(truncated(0.59464189), 0.5946418);
        assert_eq!(truncated(-0.59464189), -0.5946418);
        assert_eq!(truncated(0.99999999), 0.9999999);
    }

    /// Carbon crosses this with the direction to the target and normalises
    /// the product, so the normal itself is left just short of unit length.
    /// Asserting that it *is* short is what stops a future reader "fixing"
    /// it into a disagreement with the client.
    #[test]
    fn the_plane_normal_is_left_just_short_of_unit_length() {
        let n = orbital_normal(1_000_000_000_000, 0);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!(len < 1.0, "truncation can only shorten it: {len}");
        assert!(len > 0.999_999, "but only in the seventh digit: {len}");
    }

    /// Two ball ids one apart are two different planes. If this ever starts
    /// failing, a fleet told to orbit one station is stacking up inside one
    /// ring.
    #[test]
    fn neighbouring_ball_ids_orbit_in_different_planes() {
        let a = orbital_normal(1_000_000_000_000, 0);
        let b = orbital_normal(1_000_000_000_001, 0);
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        assert!(dot.abs() < 0.9, "planes nearly coincide: dot {dot}");
    }

    /// A range is a gap between hulls, so both radii are in the centre
    /// distance. The station number is the one that made this worth fixing.
    #[test]
    fn a_range_is_measured_between_hulls_and_not_between_centres() {
        let station = f64::from(crate::destiny::STATION_RADIUS);
        let ship = 27.0;
        assert_eq!(
            centre_distance(500.0, ship, station),
            500.0 + ship + station
        );
        assert!(
            centre_distance(500.0, ship, station) > station,
            "orbiting at 500 m must leave the ship outside the station",
        );
    }
}
