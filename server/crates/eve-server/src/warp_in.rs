//! Where a ship warping to a celestial actually arrives: the client's own
//! **warp-in point**.
//!
//! Every claim here is **[V]** — read out of this build's own bytecode with
//! `tools/dis.py`, with the client's `co_lnotab` line numbers named. Nothing
//! in this module is inferred, and nothing in it was copied from another
//! implementation.
//!
//! # Why this module exists
//!
//! The server used to land a warp at the target's centre pulled back by
//! `radius + 1000` along the line the ship came in on: one kilometre off the
//! surface, in a direction that depended on where the ship started. That is wrong twice over, and the
//! owner found it by playing: a warp to a planet of radius 5,060 km put the
//! ship *on* it, and two pilots warping to one planet from different systems
//! would have landed in different places.
//!
//! The client has had the answer all along. Three functions in
//! `eve/common/script/sys/eveCfg.py`, dispatched on `groupID` by
//! `michelle.GetWarpinPoint` (`michelle.py:1904`-`1917`), compute **one
//! point per celestial** — a point that depends on the body, not on the
//! approach. That is what makes it the shared spot every pilot lands on, and
//! `minRange` ("warp to within 100 km") is then an offset measured *from that
//! point* back along the approach.
//!
//! # The gate: not every target has one
//!
//! `ballparkCommon/parkhelpers/__init__.py:14`-`17`, in full:
//!
//! ```text
//! def HasWarpInPoint(item, radius):
//!     if radius >= 90000 and item.categoryID == appConst.categoryCelestial \
//!                        and item.groupID != appConst.groupWreck:
//!         return True
//!     return False
//! ```
//!
//! Three conditions and no others. Note what is **absent**: there is no
//! asteroid-belt case. A belt is excluded by the radius test alone (ours
//! carry radius 0, and CCP's own static data defaults them to 15,000 — both
//! under 90,000), which is why a belt behaves as a bare position you land
//! on. Stations are excluded by the category test, being category 3 and not
//! 2. Stargates are category 2 group 10 and would qualify on category, but
//! their 15,000 m radius keeps them out too.
//!
//! # The constants, each from this build
//!
//! `inventorycommon/const.py`: `categoryCelestial = 2` (:336),
//! `groupSun = 6` (:1014), `groupPlanet = 7` (:882), `groupMoon = 8` (:866),
//! `groupAsteroidBelt = 9` (:398), `groupStargate = 10` (:964),
//! `groupStation = 15` (:971), `groupWreck = 186` (:1053).
//! `eve/common/lib/appConst.py`: `jumpRadiusFactor = 130` (:1719).

/// `HasWarpInPoint`'s radius floor, in metres **[V]**
/// (`parkhelpers/__init__.py:15`).
pub const MIN_WARP_IN_RADIUS: f64 = 90_000.0;

/// `const.jumpRadiusFactor` **[V]** (`appConst.py:1719`). Used by the
/// generic rule as `p = jumpRadiusFactor / 100.0`.
pub const JUMP_RADIUS_FACTOR: f64 = 130.0;

/// CPython's Mersenne Twister, enough of it to draw one `random()`.
///
/// **Why reimplement a PRNG at all.** `GetPlanetWarpInPoint` perturbs its
/// angle with `random.Random(ballID).random()` (`eveCfg.py:1863`-`1864`), so
/// a planet's warp-in point is only reproducible if we draw the *same*
/// number CPython would. The seeding path for an integer seed is
/// `init_by_array` over the seed's 32-bit little-endian words, and it is
/// unchanged between the client's 2.7 and any modern CPython — which is what
/// makes the host's `random` module a usable oracle for the tests below.
struct PyRandom {
    mt: [u32; 624],
    index: usize,
}

impl PyRandom {
    fn seeded(seed: i64) -> PyRandom {
        let mut r = PyRandom {
            mt: [0; 624],
            index: 624,
        };
        r.init_genrand(19_650_218);
        // CPython takes the seed's absolute value and splits it into 32-bit
        // words, least significant first. A zero seed is the one-word key
        // `[0]`, not the empty key.
        let mut v = seed.unsigned_abs();
        let mut key: Vec<u32> = Vec::new();
        while v > 0 {
            key.push((v & 0xffff_ffff) as u32);
            v >>= 32;
        }
        if key.is_empty() {
            key.push(0);
        }
        r.init_by_array(&key);
        r
    }

    fn init_genrand(&mut self, seed: u32) {
        self.mt[0] = seed;
        for i in 1..624 {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = 624;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ 1_664_525u32.wrapping_mul(prev ^ (prev >> 30)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ 1_566_083_941u32.wrapping_mul(prev ^ (prev >> 30)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
        self.index = 624;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
                let mut next = self.mt[(i + 397) % 624] ^ (y >> 1);
                if y & 1 == 1 {
                    next ^= 0x9908_b0df;
                }
                self.mt[i] = next;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `genrand_res53`: 53 bits of mantissa from two draws, which is what
    /// `random.random()` returns.
    fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// One `random.Random(seed).random()`, for callers that want the draw itself.
pub fn seeded_random(seed: i64) -> f64 {
    PyRandom::seeded(seed).random()
}

/// `eveCfg.GetSunWarpInPoint` **[V]** (`eveCfg.py:1845`).
///
/// `cos`/`sin` are taken **of the radius**, which reads like a bug and is
/// reproduced exactly because the client is the thing we have to agree with.
/// It makes the direction a deterministic function of the body's size.
pub fn sun_warp_in(position: [f64; 3], radius: f64) -> [f64; 3] {
    let offset = 100_000.0;
    [
        position[0] + (radius + offset) * radius.cos(),
        position[1] + radius / 5.0,
        position[2] - (radius + offset) * radius.sin(),
    ]
}

/// `eveCfg.GetWarpInPoint` **[V]** (`eveCfg.py:1892`-`1898`), the rule for
/// every celestial that is neither a planet nor the star.
pub fn generic_warp_in(position: [f64; 3], radius: f64) -> [f64; 3] {
    let offset = 5_000_000.0;
    let p = JUMP_RADIUS_FACTOR / 100.0;
    [
        position[0] + (radius + offset) * radius.cos(),
        position[1] + p * radius - 7_500.0,
        position[2] - (radius + offset) * radius.sin(),
    ]
}

/// `eveCfg.GetPlanetWarpInPoint` **[V]** (`eveCfg.py:1853`-`1889`).
///
/// The angle comes from the planet's own position in the system — `asin` of
/// `dz / hypot(dx, dz)`, with a sign flip for two of the four quadrants —
/// and is then nudged by `rr = (random() - 1) / 3`, drawn from a generator
/// seeded with the **ball id**. `ballID` is the client's own parameter name
/// (`michelle.py:1913` passes `ball.id`); for a celestial that is its item
/// id.
///
/// The distance is `radius + 10^6 + radius * dd`, where `dd` falls off with
/// the log of the radius in millions of metres. The effect is a near-constant
/// *clearance*: across this sandbox's three planets the arrival sits 4,667,
/// 4,859 and 4,845 km above the surface for radii of 5,060, 6,060 and 3,490
/// km. That flatness is the strongest sign the formula has been read right.
pub fn planet_warp_in(ball_id: i64, position: [f64; 3], radius: f64) -> [f64; 3] {
    let dx = position[0];
    let dz = -position[2];
    let mut f = dz / (dx * dx + dz * dz).sqrt();
    // The client writes this as two clauses, `(dz > 0 and dx > 0) or
    // (dz < 0 and dx > 0)` (`eveCfg.py:1859`). They are one condition: the
    // sign flips in both quadrants with `dx > 0`, and only `dz == 0` is
    // excluded. Written as the client has it, clippy objects -- correctly --
    // so it is collapsed here and the original is recorded above.
    if dx > 0.0 && dz != 0.0 {
        f *= -1.0;
    }
    let rr = (seeded_random(ball_id) - 1.0) / 3.0;
    let theta = f.asin() + rr;

    const FACTOR: f64 = 20.0;
    let mut dd =
        ((FACTOR - 5.0 * (radius / 1_000_000.0).log10() - 0.5) / FACTOR).powf(FACTOR) * FACTOR;
    dd = dd.clamp(0.0, 10.0) + 0.5;
    let d = radius + 1_000_000.0 + radius * dd;

    [
        position[0] + theta.sin() * d,
        position[1] + radius * rr.sin() * 0.5,
        position[2] - theta.cos() * d,
    ]
}

/// The warp-in point for one celestial, or `None` when it has none.
///
/// `None` is not a failure: it is `HasWarpInPoint` returning false, and the
/// caller should then fall back to the target's own position. Belts,
/// stations and stargates all land here.
pub fn warp_in_point(c: &crate::celestial::Celestial) -> Option<[f64; 3]> {
    use crate::celestial::Kind;
    if c.radius < MIN_WARP_IN_RADIUS {
        return None;
    }
    match c.kind {
        // categoryCelestial, and not groupWreck.
        Kind::Planet => Some(planet_warp_in(c.id, c.position, c.radius)),
        Kind::Star => Some(sun_warp_in(c.position, c.radius)),
        Kind::Moon | Kind::Gate => Some(generic_warp_in(c.position, c.radius)),
        // Belt: category 2, but no belt reaches the radius floor.
        // Station: category 3, so the category test excludes it.
        // Other: we do not know its category, so we do not claim a point.
        Kind::Belt | Kind::Station | Kind::Other => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::{Celestial, Kind};

    /// The seeded draw, against CPython's own numbers.
    ///
    /// These are not hand-computed: each is `random.Random(seed).random()`
    /// printed by the host's stdlib, which runs the same Mersenne Twister the
    /// client's 2.7 does for an integer seed. If this test fails, the port of
    /// the generator is wrong and every planet arrival is in the wrong place.
    #[test]
    fn the_seeded_draw_matches_cpythons_own_mersenne_twister() {
        for (seed, want) in [
            (1i64, 0.13436424411240122),
            (2, 0.9560342718892494),
            (1234, 0.9664535356921388),
            (40000002, 0.9377985219206512),
            (40000005, 0.9601132215568895),
            (40000007, 0.580709310652556),
            (40000001, 0.8223523261012717),
            (60012526, 0.5617694972401518),
            (0, 0.8444218515250481),
        ] {
            let got = seeded_random(seed);
            assert!(
                (got - want).abs() < 1e-15,
                "seed {seed}: got {got:e}, CPython says {want:e}"
            );
        }
    }

    /// The planet rule, on the planet the owner actually warped to.
    ///
    /// The old code put the ship 1,000 m above a 5,060 km surface. The
    /// client's own rule puts it 9,727,002 m from the centre. The assertion
    /// is on the distance rather than the three coordinates because the
    /// distance is what the defect was about.
    #[test]
    fn a_planet_warp_in_sits_far_enough_out_to_be_outside_the_planet() {
        let centre = [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0];
        let radius = 5_060_000.0;
        let p = planet_warp_in(40_000_002, centre, radius);
        let d =
            ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2) + (p[2] - centre[2]).powi(2))
                .sqrt();
        assert!(
            (d - 9_727_001.5).abs() < 1.0,
            "distance from centre was {d}, expected 9,727,001.5"
        );
        assert!(d > radius, "the arrival point is inside the planet");
        // The old behaviour, named so a regression is obvious.
        assert!(
            d > radius + 1_000.0 * 100.0,
            "this is no better than the 1 km standoff it replaced"
        );
    }

    /// Clearance is near-constant across planet sizes, which is the property
    /// that makes the formula recognisable as the real one.
    #[test]
    fn clearance_above_the_surface_barely_depends_on_the_planets_size() {
        let mut clearances = Vec::new();
        for (id, centre, radius) in [
            (
                40_000_002i64,
                [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0],
                5_060_000.0f64,
            ),
            (
                40_000_005,
                [-201_870_212_031.0, -26_546_269_629.0, -291_156_066_156.0],
                6_060_000.0,
            ),
            (
                40_000_007,
                [32_357_672_472.0, 4_255_087_907.0, 488_987_096_848.0],
                3_490_000.0,
            ),
        ] {
            let p = planet_warp_in(id, centre, radius);
            let d = ((p[0] - centre[0]).powi(2)
                + (p[1] - centre[1]).powi(2)
                + (p[2] - centre[2]).powi(2))
            .sqrt();
            clearances.push(d - radius);
        }
        let lo = clearances.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = clearances.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(lo > 4_000_000.0, "clearance {lo} is too small");
        assert!(
            hi / lo < 1.2,
            "clearances {clearances:?} vary by more than 20%, so the \
             radius-dependent term is wrong"
        );
    }

    /// Same celestial, same point, every time and for everybody.
    ///
    /// This is the structural half of the defect: the old arrival was
    /// computed from the ship's own position, so two pilots arriving from
    /// different directions landed in different places. Nothing in any of
    /// these functions takes the ship's position as an argument, and this
    /// test is what keeps it that way.
    #[test]
    fn a_celestials_warp_in_point_is_one_shared_point_not_one_per_approach() {
        let c = Celestial {
            id: 40_000_002,
            type_id: 11,
            radius: 5_060_000.0,
            position: [161_891_117_336.0, 21_288_951_986.0, -73_529_712_226.0],
            kind: Kind::Planet,
        };
        let first = warp_in_point(&c).expect("a planet has a warp-in point");
        for _ in 0..8 {
            assert_eq!(warp_in_point(&c), Some(first));
        }
    }

    /// `HasWarpInPoint`, including the cases that have none.
    #[test]
    fn only_a_big_enough_celestial_has_a_warp_in_point() {
        let at = |kind, radius| {
            warp_in_point(&Celestial {
                id: 40_000_002,
                type_id: 11,
                radius,
                position: [1.0e11, 2.0e10, -7.0e10],
                kind,
            })
        };
        // Under the 90 km floor: no point, whatever it is.
        assert_eq!(at(Kind::Planet, 89_999.0), None);
        assert!(
            at(Kind::Planet, 90_000.0).is_some(),
            "the floor is >=, not >"
        );
        // A belt carries radius 0 here and 15,000 in CCP's static data;
        // both are under the floor, which is the whole reason a belt is a
        // bare position you land on.
        assert_eq!(at(Kind::Belt, 0.0), None);
        assert_eq!(at(Kind::Belt, 15_000.0), None);
        // A station is category 3, so the category test excludes it however
        // large it is.
        assert_eq!(at(Kind::Station, 50_000_000.0), None);
        // A stargate is category 2 and would qualify, but its radius is
        // 15,000.
        assert_eq!(at(Kind::Gate, 15_000.0), None);
        assert!(at(Kind::Gate, 1_000_000.0).is_some());
        // We do not know what `Other` is, so we claim nothing.
        assert_eq!(at(Kind::Other, 10_000_000.0), None);
    }

    /// The star and the generic rule, which take no random draw at all.
    #[test]
    fn the_star_and_the_generic_rule_are_pure_functions_of_body_and_radius() {
        let pos = [0.0, 0.0, 0.0];
        let r = 63_350_000.0;
        let sun = sun_warp_in(pos, r);
        assert_eq!(sun[1], r / 5.0, "the star's y offset is radius/5");
        // cos/sin are taken of the radius itself; reproduce, do not fix.
        assert!((sun[0] - (r + 100_000.0) * r.cos()).abs() < 1e-6);
        assert!((sun[2] + (r + 100_000.0) * r.sin()).abs() < 1e-6);

        let moon_r = 270_000.0;
        let g = generic_warp_in([1.0, 2.0, 3.0], moon_r);
        assert!(
            (g[1] - (2.0 + 1.3 * moon_r - 7_500.0)).abs() < 1e-9,
            "the generic y offset is jumpRadiusFactor/100 * radius - 7500"
        );
    }
}
