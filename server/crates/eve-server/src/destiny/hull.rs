use crate::content::Content;

/// One hull's ball-record numbers.
///
/// **The unfitted hull.** These are base values, before dogma modifiers:
/// a fitted ship's `maxVelocity` is its own business and the client
/// recomputes it, but the blob wants the hull's own number.
///
/// `radius`, `max_velocity` and `agility` are `f32` because that is their
/// width on the wire. Keeping them `f32` here rather than converting at
/// the call site is what stops a test comparing `134.43_f64` against a
/// field that can only hold `134.42999267578125`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hull {
    pub type_id: i64,
    pub mass: f64,
    pub radius: f32,
    pub max_velocity: f32,
    pub agility: f32,
    /// Dogma attribute 263. The HUD's shield ring is
    /// `shieldCharge / shieldCapacity`, so a zero here is a division by
    /// zero that `GetShieldHPPortion` catches and turns into an **empty
    /// ring** -- which is what a player saw.
    pub shield_capacity: f64,
    /// Dogma attribute 479, in milliseconds. Not a ring input, but
    /// `godma.UpdateAttribute` only builds the shield's charged
    /// attribute when **both** this and the capacity are truthy, so a
    /// zero here costs the shield its regeneration.
    pub shield_recharge_rate: f64,
    /// Dogma attribute 265, with 266 (`armorDamage`) as its complement.
    pub armor_hp: f64,
    /// Dogma attribute **76**. Zero is not a small range, it is **no
    /// lock at all**: `celestialCheckers.IsWithinTargettingRange` is
    /// `dist < maxTargetRange`, so a zero **hides the right-click "Lock
    /// target" entry** and there is no error anywhere to explain it.
    /// `docs/re/targeting.md` §6.1.
    pub max_target_range: f64,
    /// Dogma attribute **192**. A zero makes the ctrl-marquee mass-lock
    /// select nothing -- the client slices `balls[:int(maxLockedTargets)]`.
    pub max_locked_targets: f64,
    /// Dogma attribute **564**. The client's own lock animation is
    /// `min(180000, 40e6 / scanResolution / asinh(sigRadius)^2)` ms, so
    /// this is the only input to lock *speed* and the server never times
    /// anything. A zero is a division by zero the client catches into an
    /// instant or absent lock.
    pub scan_resolution: f64,
    /// Dogma attribute **552**, and it is read off the **type** rather
    /// than off our ship -- the client has its own FSD copy for every
    /// hull, so we never serve a target's attributes. Kept here because
    /// it is the second input to the lock-time formula and a reader
    /// checking that arithmetic should not have to leave the file.
    pub signature_radius: f64,
    /// `baseWarpSpeed * warpSpeedMultiplier` (dogma 1281 and 600), in
    /// AU/s: `crate::destiny::warp_factor_in`'s input.
    pub warp_speed_au: f64,
    /// Dogma attribute 9, with 3 (`damage`) as its complement. The
    /// *structure* ring, despite the bare name.
    pub hp: f64,
    /// Dogma attribute 482, and 18 (`charge`) is its state. The
    /// capacitor the three rings are drawn around, which is the same
    /// mechanism and was equally empty.
    pub capacitor_capacity: f64,
    /// Dogma attribute 55, in milliseconds.
    pub capacitor_recharge_rate: f64,
}

/// **Invented hull numbers, one class template per row** (O3, ruling 3).
///
/// The client's own hull numbers are class-B content (plan §2.1): they live
/// in the bundle's `types/typedata.tsv` and reach [`find_in`] through
/// `crate::typedata::hull_in`. What stays in code is the **type ids** (the
/// hulls `item::SPAWNABLE` grants, the capsule the first undocked ship
/// flies) with round, made-up numbers of the right order per class, so a
/// server with no `--content` and every test still fly something sane.
///
/// **None of these is a client number** and no test may read one as a
/// fact: a test asserts a rule over [`FIXTURE`] (align time, a non-zero
/// ring) or compares code against itself.
const fn class(type_id: i64, t: Hull) -> Hull {
    Hull { type_id, ..t }
}

/// The class templates [`FIXTURE`] is built from. Invented.
const FRIGATE: Hull = Hull {
    type_id: 0,
    mass: 1_000_000.0,
    radius: 40.0,
    max_velocity: 350.0,
    agility: 3.0,
    shield_capacity: 400.0,
    shield_recharge_rate: 600_000.0,
    armor_hp: 400.0,
    max_target_range: 25_000.0,
    max_locked_targets: 4.0,
    scan_resolution: 600.0,
    signature_radius: 40.0,
    warp_speed_au: 5.0,
    hp: 400.0,
    capacitor_capacity: 300.0,
    capacitor_recharge_rate: 150_000.0,
};
const ROOKIE: Hull = Hull {
    radius: 45.0,
    max_velocity: 300.0,
    agility: 4.0,
    shield_capacity: 200.0,
    armor_hp: 200.0,
    hp: 200.0,
    max_target_range: 20_000.0,
    max_locked_targets: 3.0,
    scan_resolution: 500.0,
    signature_radius: 50.0,
    warp_speed_au: 3.0,
    capacitor_capacity: 150.0,
    capacitor_recharge_rate: 75_000.0,
    ..FRIGATE
};
const DESTROYER: Hull = Hull {
    mass: 1_500_000.0,
    radius: 120.0,
    max_velocity: 270.0,
    agility: 2.8,
    shield_capacity: 800.0,
    armor_hp: 800.0,
    hp: 800.0,
    max_target_range: 30_000.0,
    max_locked_targets: 6.0,
    scan_resolution: 520.0,
    signature_radius: 60.0,
    warp_speed_au: 4.5,
    capacitor_capacity: 600.0,
    capacitor_recharge_rate: 300_000.0,
    ..FRIGATE
};
const CRUISER: Hull = Hull {
    mass: 12_000_000.0,
    radius: 150.0,
    max_velocity: 230.0,
    agility: 0.5,
    shield_capacity: 1_500.0,
    shield_recharge_rate: 1_200_000.0,
    armor_hp: 1_500.0,
    hp: 1_500.0,
    max_target_range: 55_000.0,
    max_locked_targets: 6.0,
    scan_resolution: 280.0,
    signature_radius: 120.0,
    warp_speed_au: 4.0,
    capacitor_capacity: 1_400.0,
    capacitor_recharge_rate: 500_000.0,
    ..FRIGATE
};
const BATTLECRUISER: Hull = Hull {
    mass: 13_000_000.0,
    radius: 230.0,
    max_velocity: 160.0,
    agility: 0.65,
    shield_capacity: 4_500.0,
    shield_recharge_rate: 1_400_000.0,
    armor_hp: 4_000.0,
    hp: 4_000.0,
    max_target_range: 65_000.0,
    max_locked_targets: 7.0,
    scan_resolution: 210.0,
    signature_radius: 300.0,
    warp_speed_au: 3.5,
    capacitor_capacity: 2_500.0,
    capacitor_recharge_rate: 650_000.0,
    ..FRIGATE
};
const BATTLESHIP: Hull = Hull {
    mass: 100_000_000.0,
    radius: 300.0,
    max_velocity: 120.0,
    agility: 0.12,
    shield_capacity: 7_000.0,
    shield_recharge_rate: 2_500_000.0,
    armor_hp: 7_000.0,
    hp: 7_500.0,
    max_target_range: 85_000.0,
    max_locked_targets: 7.0,
    scan_resolution: 125.0,
    signature_radius: 380.0,
    warp_speed_au: 3.0,
    capacitor_capacity: 6_000.0,
    capacitor_recharge_rate: 1_100_000.0,
    ..FRIGATE
};
const INDUSTRIAL: Hull = Hull {
    mass: 12_000_000.0,
    radius: 300.0,
    max_velocity: 110.0,
    agility: 0.9,
    shield_capacity: 800.0,
    armor_hp: 800.0,
    hp: 1_500.0,
    max_target_range: 45_000.0,
    max_locked_targets: 2.0,
    scan_resolution: 70.0,
    signature_radius: 200.0,
    warp_speed_au: 3.5,
    capacitor_capacity: 600.0,
    capacitor_recharge_rate: 400_000.0,
    ..FRIGATE
};
/// The capsule's invented row. Its three targeting attributes are **zero**
/// on purpose: the client's own capsule carries zeros there [V]
/// (`docs/re/targeting.md` §6.1), and the service's floor for that case
/// (`service::tests::the_capsule_keeps_its_real_zeros_and_is_served_a_floor`)
/// needs a hull that has them.
pub const CAPSULE: Hull = Hull {
    type_id: 670,
    mass: 30_000.0,
    radius: 3.0,
    max_velocity: 160.0,
    agility: 2.5,
    shield_capacity: 60.0,
    shield_recharge_rate: 600_000.0,
    armor_hp: 150.0,
    max_target_range: 0.0,
    max_locked_targets: 0.0,
    scan_resolution: 0.0,
    signature_radius: 25.0,
    warp_speed_au: 4.0,
    hp: 150.0,
    capacitor_capacity: 120.0,
    capacitor_recharge_rate: 180_000.0,
};

/// The hulls a server with no bundle can fly, ascending by type id: every
/// category-6 row of `item::SPAWNABLE`, with **invented** numbers (above).
/// [`find_in`] answers from here only when `Content` has no bundle.
pub const FIXTURE: &[Hull] = &[
    class(587, FRIGATE),
    class(594, FRIGATE),
    class(597, FRIGATE),
    class(601, ROOKIE),
    class(603, FRIGATE),
    class(621, CRUISER),
    class(629, CRUISER),
    class(639, BATTLESHIP),
    class(641, BATTLESHIP),
    class(642, BATTLESHIP),
    class(648, INDUSTRIAL),
    class(657, INDUSTRIAL),
    CAPSULE,
    class(1_944, INDUSTRIAL),
    class(2_006, CRUISER),
    class(16_227, BATTLECRUISER),
    class(16_240, DESTROYER),
    class(16_242, DESTROYER),
    class(24_702, BATTLECRUISER),
];

fn fixture_row(type_id: i64) -> Option<&'static Hull> {
    FIXTURE
        .binary_search_by_key(&type_id, |h| h.type_id)
        .ok()
        .map(|i| &FIXTURE[i])
}

/// **This hull's numbers, from `content`** (CP58b, ruling 3): the hand NPC
/// table (`crate::npc::hull`), the drone table (`crate::drones::hull`), then
/// the bundle's ships (`crate::typedata::hull_in`). With no bundle, last,
/// the invented [`FIXTURE`]. `None` for a type none of them has; callers
/// take [`FALLBACK`].
///
/// An NPC and a drone are balls whose hull is their own type, and
/// everything that sizes, integrates or damages a ball asks here -- so a
/// type missing from this lookup is a ball with the fallback's radius,
/// mass and HP (`docs/lanes/npc.md` §5 S3).
pub fn find_in(content: &Content, type_id: i64) -> Option<&Hull> {
    crate::npc::hull(type_id)
        .or_else(|| crate::drones::hull(type_id))
        .or_else(|| crate::typedata::hull_in(content, type_id))
        .or_else(|| {
            if content.is_bundle() {
                None
            } else {
                fixture_row(type_id)
            }
        })
}

/// [`find_in`], or [`FALLBACK`].
pub fn of_in(content: &Content, type_id: i64) -> &Hull {
    find_in(content, type_id).unwrap_or(&FALLBACK)
}

/// What a hull the table does not know gets.
///
/// **`_destiny.dll`'s own initialisers** (`radius 1.0`, `Agility 1.0`,
/// `mass 1e6`) **[V]**, so a fallback ball is one the native code would
/// have been happy to invent for itself -- which is the honest shape for
/// a default: visibly a default, and not a plausible wrong number for
/// some other ship. `1e6` kg is also EvEJS's ship default (**[V2]**).
///
/// Reachable only for a type nothing in this sandbox can be flying, since
/// the bundle has every ship and [`FIXTURE`] every category-6 member of
/// `item::SPAWNABLE` (`tests::every_spawnable_hull_has_its_numbers`).
/// The health numbers are the **capsule's** ([`CAPSULE`]), and that is a different
/// kind of default from the three above. A zero `hp` or `armorHP` is a
/// division by zero in `activeShipController`, which the client catches
/// and turns into an empty gauge -- so "visibly a default" is not
/// available here: the honest-looking choice draws three empty rings on
/// a healthy ship, which is the exact defect this table's health columns
/// were added to fix. A pod's numbers on an unknown hull are wrong and
/// harmless; zeros are wrong and look like damage.
pub const FALLBACK: Hull = Hull {
    type_id: 0,
    // **Workable, not zero**, by the same argument the health columns
    // above settle: a zero `max_target_range` hides the right-click
    // "Lock target" entry with no error anywhere, which is wrong *and*
    // looks like our bug, while a frigate's numbers on an unknown hull
    // are wrong and harmless. [A] the invented rookie frigate's
    // ([`FIXTURE`]); falsifier: none needed, any non-zero range works.
    max_target_range: ROOKIE.max_target_range,
    max_locked_targets: ROOKIE.max_locked_targets,
    scan_resolution: ROOKIE.scan_resolution,
    signature_radius: ROOKIE.signature_radius,
    warp_speed_au: CAPSULE.warp_speed_au,
    mass: 1.0e6,
    radius: 1.0,
    max_velocity: 1.0,
    agility: 1.0,
    shield_capacity: CAPSULE.shield_capacity,
    shield_recharge_rate: CAPSULE.shield_recharge_rate,
    armor_hp: CAPSULE.armor_hp,
    hp: CAPSULE.hp,
    capacitor_capacity: CAPSULE.capacitor_capacity,
    capacitor_recharge_rate: CAPSULE.capacitor_recharge_rate,
};

/// The client's own align time, in seconds: `-ln(0.25) * agility * mass / 1e6`.
///
/// `shipfitting/fittingDogmaLocationUtil` **[V]**, fed by dogma
/// attributes 70 and 4. Not a ball field -- it is here because it is the
/// one published number that depends on both columns at once, which makes
/// it the table's positive control.
pub fn align_seconds(h: &Hull) -> f64 {
    -(0.25f64.ln()) * f64::from(h.agility) * h.mass / 1.0e6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;

    /// A content whose typedata holds one ship at a [`FIXTURE`] id, with
    /// numbers the fixture does not have.
    fn bundle_shadowing(type_id: i64) -> Content {
        let row = crate::typedata::FIXTURE
            .lines()
            .find(|l| l.starts_with("type\t990000001\t"))
            .expect("the typedata fixture has its frigate");
        let text = crate::typedata::FIXTURE
            .replace(row, &row.replacen("990000001", &type_id.to_string(), 1));
        let table = crate::typedata::Table::parse(&text).expect("parses");
        let mut c = Content::fixture();
        c.insert("typedata", crate::typedata::Installed::build(table));
        c
    }

    /// The bundle's ship is the answer: [`find_in`] reads
    /// `typedata::hull_in`, and a [`FIXTURE`] row never shadows it.
    #[test]
    fn find_in_reads_the_content_typedata_before_the_fixture() {
        let c = Content::fixture();
        assert_eq!(
            find_in(&c, 990_000_001),
            crate::typedata::hull_in(&c, 990_000_001)
        );
        assert!(find_in(&c, 990_000_001).is_some());
        let shadowed = bundle_shadowing(FIXTURE[0].type_id);
        assert_eq!(
            find_in(&shadowed, FIXTURE[0].type_id),
            crate::typedata::hull_in(&shadowed, FIXTURE[0].type_id)
        );
        assert_ne!(find_in(&shadowed, FIXTURE[0].type_id), Some(&FIXTURE[0]));
    }

    /// Without a bundle every [`FIXTURE`] row answers; NPC and drone types
    /// fall through to their own tables first.
    #[test]
    fn find_in_falls_through_npcs_drones_and_the_fixture() {
        let c = Content::fixture();
        assert!(!c.is_bundle());
        for h in FIXTURE {
            assert_eq!(find_in(&c, h.type_id), Some(h));
        }
        let rat = crate::npc::FIXTURE_RAT.type_id;
        assert_eq!(find_in(&c, rat), crate::npc::hull(rat));
        assert!(find_in(&c, rat).is_some());
        for d in crate::drones::TYPES {
            assert_eq!(find_in(&c, d.type_id), crate::drones::hull(d.type_id));
            assert!(find_in(&c, d.type_id).is_some());
        }
        assert!(find_in(&c, 1).is_none());
        assert_eq!(of_in(&c, 1), &FALLBACK);
    }

    /// Every hull a character here can be flying has its numbers: growing
    /// `item::SPAWNABLE` with a new hull and no [`FIXTURE`] row is a test
    /// failure rather than a ship that flies like the native default.
    /// (Moved from `destiny::tests`, CP58b.)
    #[test]
    fn every_spawnable_hull_has_its_numbers() {
        let c = Content::fixture();
        for ty in crate::item::SPAWNABLE {
            if ty.category_id != crate::typedata::CATEGORY_SHIP {
                continue;
            }
            assert!(
                find_in(&c, ty.type_id).is_some(),
                "type {} ({}) is a spawnable hull with no destiny::hull::FIXTURE row",
                ty.type_id,
                ty.name
            );
        }
    }

    /// Ascending by type id: [`find_in`] binary-searches it.
    #[test]
    fn the_fixture_is_sorted() {
        assert!(
            FIXTURE.windows(2).all(|w| w[0].type_id < w[1].type_id),
            "FIXTURE must be ascending and have no duplicate type"
        );
    }

    /// No fixture hull has a zero a ring, an integrator or a warp divides
    /// by; only the capsule carries the targeting zeros (see [`CAPSULE`]).
    #[test]
    fn no_fixture_hull_has_a_zero_the_client_divides_by() {
        for h in FIXTURE {
            for v in [
                h.mass,
                f64::from(h.radius),
                f64::from(h.max_velocity),
                f64::from(h.agility),
                h.shield_capacity,
                h.shield_recharge_rate,
                h.armor_hp,
                h.hp,
                h.capacitor_capacity,
                h.capacitor_recharge_rate,
                h.signature_radius,
                h.warp_speed_au,
            ] {
                assert!(v > 0.0, "type {}", h.type_id);
            }
            if h.type_id != CAPSULE.type_id {
                assert!(h.max_target_range > 0.0 && h.scan_resolution > 0.0);
                assert!(h.max_locked_targets >= 1.0);
            }
        }
    }

    /// The align time is `-ln(0.25) * tau`, the time a ship from rest takes
    /// to reach three quarters of its top speed.
    #[test]
    fn align_seconds_is_the_three_quarter_speed_time() {
        for h in FIXTURE {
            let tau = f64::from(h.agility) * h.mass / 1e6;
            let v = 1.0 - (-align_seconds(h) / tau).exp();
            assert!((v - 0.75).abs() < 1e-9, "type {}", h.type_id);
        }
    }
}
