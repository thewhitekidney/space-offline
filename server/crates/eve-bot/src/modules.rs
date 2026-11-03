//! **Where a bot's module numbers come from** (FX11).
//!
//! CP58d retired the baked module table: with no typedata installed,
//! `eve_server::modules::find` answers from the **invented**
//! `modules::FIXTURE`, and the bot process installs none. So the miner's
//! laser range, an escort's gun range, its reload lockout and which effect
//! a gun fires were read from made-up rows. This module is the bot's
//! answer, in the order [`crate::hulls`] uses for the hull (FX10):
//!
//! 1. **The server's word.** A fitted module's dogma row
//!    (`dogmaIM.ItemGetInfo(moduleID)`, `modules.rs::module_info`) carries
//!    the module type's own attributes as the server serves them to the
//!    client, modified by the ship's dogma sheet when it has one. The bot
//!    already reads that row every listing for the module's running
//!    effects ([`crate::Conn::module_row`]), so its attributes cost nothing
//!    more. The server's range check and reload lockout read the same type
//!    row (`modules::find_in`), so a bot that reads it agrees by
//!    construction.
//! 2. **The bundle** (`eve-bots --content DIR`, [`crate::hulls::open`]):
//!    through `eve_server::modules::find_in`, as the server reads it.
//! 3. **The invented fixture**, with no bundle: the binary says so once at
//!    startup ([`NO_CONTENT`]).
//!
//! A weapon's effect (its name and id) has no attribute to read: it comes
//! from the table only, the bundle's with `--content`.

use crate::hulls::Source;
use eve_server::content::Content;
use eve_server::modules::{self, ModuleType};
use std::time::Duration;

/// `maxRange` (54): a mining laser's reach, a turret's optimal **[V]**.
pub const ATTRIBUTE_MAX_RANGE: i64 = 54;
/// `falloff` (158) **[V]**.
pub const ATTRIBUTE_FALLOFF: i64 = 158;
/// `reloadTime` (1795), milliseconds **[V]**.
pub const ATTRIBUTE_RELOAD_TIME: i64 = 1795;

/// Printed once by `eve-bots` when it runs without `--content`.
pub const NO_CONTENT: &str = "eve-bots: no --content: module numbers are INVENTED \
    (modules::FIXTURE) wherever a module's own ItemGetInfo row does not \
    answer, and which modules are guns comes from the fixture";

/// This module type's row: the bundle's through `modules::find_in` when
/// `content` is given, else the invented `modules::FIXTURE` (what
/// `modules::find_in` answers on a content with no bundle and no typedata
/// rows; X2a). `None` for a type neither knows.
pub fn find(content: Option<&Content>, type_id: i64) -> Option<(ModuleType, Source)> {
    match content {
        Some(c) => modules::find_in(c, type_id).map(|m| {
            let from = if c.is_bundle() {
                Source::Bundle
            } else {
                Source::Invented
            };
            (*m, from)
        }),
        None => modules::FIXTURE
            .iter()
            .find(|m| m.type_id == type_id)
            .map(|m| (*m, Source::Invented)),
    }
}

/// **How many rounds of `charge_type_id` a `module_type_id` holds**:
/// `modules::rounds_capacity_in` on the bundle when `content` is given,
/// else over the hand item rows (`item::SPAWNABLE`, `WRECK_TYPES`,
/// `COURIER_TYPES`), which is what the server's lookup answered for these
/// types with no typedata (X2b).
pub fn rounds_capacity(content: Option<&Content>, module_type_id: i64, charge_type_id: i64) -> i64 {
    match content {
        Some(c) => modules::rounds_capacity_in(c, module_type_id, charge_type_id),
        None => {
            let hand = |id: i64| {
                eve_server::item::SPAWNABLE
                    .iter()
                    .chain(eve_server::item::WRECK_TYPES)
                    .chain(eve_server::item::COURIER_TYPES)
                    .find(|t| t.type_id == id)
            };
            modules::rounds_of(hand(module_type_id), hand(charge_type_id))
        }
    }
}

/// **A gun's effect**, `(name, id)`: the type's offensive target effect
/// (category 2), from [`find`]'s row; or, for **a missile launcher**, its
/// `useMissiles` (101). That one is an activation effect (category 1, not
/// offensive) in the client's table, so the category test alone never named
/// a launcher and a bot's missile hull never fired (ENGINE-FIX). The server
/// takes it with a target (`modules::needs_lock`). A probe or survey
/// launcher has the same effect and is not a weapon. `None` for a module
/// that is not a gun.
pub fn weapon_effect(content: Option<&Content>, type_id: i64) -> Option<(&'static str, i64)> {
    let (t, _) = find(content, type_id)?;
    let e = t.effect?;
    if e.is_offensive && e.category == 2 {
        return Some((e.name, e.effect_id));
    }
    (modules::is_launcher(&t) && !is_non_weapon_launcher(content, &t))
        .then_some((e.name, e.effect_id))
}

/// `chargeGroup1` of a probe (scan probes) in the invented fixture rows.
const GROUP_PROBES: f64 = 479.0;
/// `chargeGroup1` of a warp-disruption probe (the sphere launcher's) **[V]**.
const GROUP_BUBBLE_PROBES: f64 = 548.0;
/// `chargeGroup1` (604) **[V]**.
const ATTRIBUTE_CHARGE_GROUP1: i64 = 604;

/// A launcher of probes, not missiles: scan probes are charge group 479, the
/// sphere launcher's warp-disruption probes group 548, and a bundle also
/// names the survey launchers. Fitted as a gun it would be fired at the
/// bot's target (BP-HOLD).
fn is_non_weapon_launcher(content: Option<&Content>, t: &ModuleType) -> bool {
    matches!(
        t.attribute(ATTRIBUTE_CHARGE_GROUP1),
        Some(g) if g == GROUP_PROBES || g == GROUP_BUBBLE_PROBES
    ) || content.is_some_and(|c| eve_server::moon_scan::is_survey_launcher_in(c, t.type_id))
}

/// **How far a missile flies**, metres: its speed (37) times its fuse (281,
/// ms) -- the server's `missile::Charge::reach` for an unskilled pilot. A
/// launcher's own row carries no range; the round does. `None` for a type
/// that is not a missile.
pub fn missile_reach(content: &Content, charge_type_id: i64) -> Option<f64> {
    let a = |id| eve_server::typedata::attribute_in(content, charge_type_id, id);
    let reach = a(ATTRIBUTE_MAX_VELOCITY)? * a(ATTRIBUTE_EXPLOSION_DELAY)? / 1000.0;
    (reach > 0.0).then_some(reach)
}

/// `maxVelocity` (37): a missile's speed **[V]**.
pub const ATTRIBUTE_MAX_VELOCITY: i64 = 37;
/// `explosionDelay` (281), ms: a missile's fuse **[V]**.
pub const ATTRIBUTE_EXPLOSION_DELAY: i64 = 281;

/// **One attribute of a module**: `served` (the module's own dogma row)
/// when the row carries it, else [`find`]'s row, else `0` from
/// [`Source::Unknown`].
pub fn attribute(
    served: Option<f64>,
    content: Option<&Content>,
    type_id: i64,
    attribute: i64,
) -> (f64, Source) {
    if let Some(v) = served {
        return (v, Source::Server);
    }
    match find(content, type_id) {
        Some((t, from)) => (t.attribute(attribute).unwrap_or(0.0), from),
        None => (0.0, Source::Unknown),
    }
}

/// The less trusted of two sources: a number built from both is only as
/// good as the worse.
fn worse(a: Source, b: Source) -> Source {
    let rank = |s| match s {
        Source::Server => 0,
        Source::Bundle => 1,
        Source::Invented => 2,
        Source::Unknown => 3,
    };
    if rank(a) >= rank(b) { a } else { b }
}

/// A mining laser's reach, metres: `maxRange` (54).
pub fn laser_range(
    served: impl Fn(i64) -> Option<f64>,
    content: Option<&Content>,
    type_id: i64,
) -> (f64, Source) {
    attribute(
        served(ATTRIBUTE_MAX_RANGE),
        content,
        type_id,
        ATTRIBUTE_MAX_RANGE,
    )
}

/// A gun's fighting range, metres: optimal plus half its falloff. The half
/// is **[A]** as a policy; the numbers are the module's.
pub fn gun_range(
    served: impl Fn(i64) -> Option<f64>,
    content: Option<&Content>,
    type_id: i64,
) -> (f64, Source) {
    let (optimal, a) = attribute(
        served(ATTRIBUTE_MAX_RANGE),
        content,
        type_id,
        ATTRIBUTE_MAX_RANGE,
    );
    let (falloff, b) = attribute(
        served(ATTRIBUTE_FALLOFF),
        content,
        type_id,
        ATTRIBUTE_FALLOFF,
    );
    (optimal + falloff / 2.0, worse(a, b))
}

/// `falloffEffectiveness` (2044): how far past its optimal an ewar module
/// still works **[V]** (`ewar.rs`).
pub const ATTRIBUTE_EWAR_FALLOFF: i64 = 2044;

/// An ewar module's reach, metres: `maxRange` (54) plus `falloffEffectiveness`
/// (2044), none for a scrambler. Past it the effect is under half. **[A]**
/// as a policy; the numbers are the module's.
pub fn ewar_reach(
    served: impl Fn(i64) -> Option<f64>,
    content: Option<&Content>,
    type_id: i64,
) -> f64 {
    let at = |a| attribute(served(a), content, type_id, a).0;
    (at(ATTRIBUTE_MAX_RANGE) + at(ATTRIBUTE_EWAR_FALLOFF)).max(0.0)
}

/// A gun's `reloadTime` (1795): a reload in space locks the gun this long
/// (`modules::load_ammo`).
pub fn reload_time(
    served: impl Fn(i64) -> Option<f64>,
    content: Option<&Content>,
    type_id: i64,
) -> (Duration, Source) {
    let (ms, from) = attribute(
        served(ATTRIBUTE_RELOAD_TIME),
        content,
        type_id,
        ATTRIBUTE_RELOAD_TIME,
    );
    (Duration::from_millis(ms.max(0.0) as u64), from)
}

#[cfg(test)]
mod tests {
    //! Invented rows only: `typedata::FIXTURE`'s autocannon (an invented
    //! type id, `54=900`, `158=4000`) and `modules::FIXTURE`'s gun 484.
    use super::*;
    use eve_server::typedata::{FIXTURE, Installed, Table};

    /// `typedata::FIXTURE`'s "Fixture Autocannon": in no `modules::FIXTURE`.
    const TABLE_GUN: i64 = 990_000_002;
    /// `typedata::FIXTURE`'s shield extender: a module, not a gun.
    const TABLE_EXTENDER: i64 = 990_000_003;
    /// `modules::FIXTURE`'s gun, the escort's (`escort::GUN`).
    const FIXTURE_GUN: i64 = crate::escort::GUN;

    fn content() -> Content {
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(FIXTURE).expect("the fixture parses")),
        );
        c
    }

    fn none(_: i64) -> Option<f64> {
        None
    }

    #[test]
    fn with_content_a_module_reads_its_rows_numbers() {
        let c = content();
        assert_eq!(laser_range(none, Some(&c), TABLE_GUN).0, 900.0);
        assert_eq!(gun_range(none, Some(&c), TABLE_GUN).0, 900.0 + 2000.0);
        assert_eq!(
            weapon_effect(Some(&c), TABLE_GUN),
            Some(("projectileFired", 34))
        );
        assert_eq!(weapon_effect(Some(&c), TABLE_EXTENDER), None);
        // The invented fixture does not know the content's gun.
        assert_eq!(weapon_effect(None, TABLE_GUN), None);
        assert_eq!(gun_range(none, None, TABLE_GUN), (0.0, Source::Unknown));
    }

    #[test]
    fn without_content_the_fixture_answers_and_says_it_is_invented() {
        let (f, _) = find(None, FIXTURE_GUN).expect("the fixture has the gun");
        let a = |id| f.attribute(id).unwrap_or(0.0);
        assert_eq!(
            gun_range(none, None, FIXTURE_GUN),
            (
                a(ATTRIBUTE_MAX_RANGE) + a(ATTRIBUTE_FALLOFF) / 2.0,
                Source::Invented
            )
        );
        assert_eq!(
            reload_time(none, None, FIXTURE_GUN),
            (
                Duration::from_millis(a(ATTRIBUTE_RELOAD_TIME) as u64),
                Source::Invented
            )
        );
        assert!(weapon_effect(None, FIXTURE_GUN).is_some());
        assert_eq!(laser_range(none, None, -1), (0.0, Source::Unknown));
        assert!(NO_CONTENT.contains("INVENTED"));
    }

    #[test]
    fn the_modules_served_attributes_win_over_any_table() {
        let c = content();
        let served = |id| match id {
            ATTRIBUTE_MAX_RANGE => Some(1_234.0),
            ATTRIBUTE_FALLOFF => Some(600.0),
            ATTRIBUTE_RELOAD_TIME => Some(4_321.0),
            _ => None,
        };
        assert_eq!(
            gun_range(served, Some(&c), TABLE_GUN),
            (1_234.0 + 300.0, Source::Server)
        );
        assert_eq!(
            reload_time(served, Some(&c), TABLE_GUN),
            (Duration::from_millis(4_321), Source::Server)
        );
        // A row that serves the optimal but not the falloff: the falloff
        // is the table's, and the sum is only as good as the table.
        let half = |id| (id == ATTRIBUTE_MAX_RANGE).then_some(1_234.0);
        assert_eq!(
            gun_range(half, Some(&c), TABLE_GUN),
            (1_234.0 + 2000.0, Source::Invented)
        );
    }

    /// DOC-2: a gun fires by **the type's default effect**, whatever it is
    /// called -- a hybrid or laser turret and a missile launcher each carry
    /// their own offensive target effect (category 2) -- and a module whose
    /// default effect is no offensive target one (a plate's, a hardener's
    /// activation) is no gun. Invented rows and effect ids.
    #[test]
    fn a_hybrid_gun_activates() {
        const HYBRID: i64 = 990_940_001;
        const LAUNCHER: i64 = 990_940_002;
        const HARDENER: i64 = 990_940_003;
        const E_HYBRID: i64 = 990_941;
        const E_MISSILE: i64 = 990_942;
        const E_ACTIVE: i64 = 990_943;
        let rows = format!(
            "{}\
             type\t{HYBRID}\t74\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t{E_HYBRID}\t12,16,{E_HYBRID},42\t\
             6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=1.0;158=4000.0;160=400.0;604=85.0\tFixture Hybrid Gun\n\
             type\t{LAUNCHER}\t507\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t{E_MISSILE}\t12,16,{E_MISSILE},40\t\
             6=0.0;30=2.0;50=4.0;51=2500.0;604=384.0\tFixture Launcher\n\
             type\t{HARDENER}\t77\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t{E_ACTIVE}\t13,16,{E_ACTIVE}\t30=3.0;50=10.0\tFixture Hardener\n\
             effect\t{E_HYBRID}\t2\t1\t51\t6\t54\t158\t160\tfixtureHybridFired\teffects.FixtureHybrid\n\
             effect\t{E_MISSILE}\t2\t1\t51\t0\t0\t0\t0\tfixtureMissileLaunching\teffects.FixtureMissile\n\
             effect\t{E_ACTIVE}\t1\t0\t51\t0\t0\t0\t0\tfixtureArmorHardener\teffects.FixtureHardener\n",
            FIXTURE
        );
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(&rows).expect("the rows parse")),
        );
        assert_eq!(
            weapon_effect(Some(&c), HYBRID),
            Some(("fixtureHybridFired", E_HYBRID))
        );
        assert_eq!(
            weapon_effect(Some(&c), LAUNCHER),
            Some(("fixtureMissileLaunching", E_MISSILE))
        );
        assert_eq!(weapon_effect(Some(&c), HARDENER), None);
        // The fixture's autocannon still answers by its own default effect.
        assert_eq!(
            weapon_effect(Some(&c), TABLE_GUN),
            Some(("projectileFired", 34))
        );
    }

    /// ENGINE-FIX: a launcher's `useMissiles` (101) is an activation effect
    /// (category 1, not offensive) in the client's table, yet the launcher is
    /// a weapon and fires at a target; a probe launcher has the same effect
    /// and is not one. A missile reaches speed times fuse. Invented rows and
    /// ids; the effect id is the client's.
    #[test]
    fn a_launcher_is_a_weapon_and_a_probe_launcher_is_not() {
        const LAUNCHER: i64 = 990_950_001;
        const PROBE_LAUNCHER: i64 = 990_950_002;
        const MISSILE: i64 = 990_950_003;
        const GROUP_MISSILE: i64 = 990_951;
        let rows = format!(
            "{}\
             type\t{LAUNCHER}\t507\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,40,101\t\
             6=0.0;30=2.0;50=4.0;51=4000.0;604={GROUP_MISSILE}.0\tFixture Launcher\n\
             type\t{PROBE_LAUNCHER}\t481\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,101\t\
             6=0.0;30=2.0;50=4.0;51=4000.0;604=479.0\tFixture Probe Launcher\n\
             type\t{MISSILE}\t{GROUP_MISSILE}\t8\t1\t100\t0.015\t0.0\t700.0\t300.0\t0\t\t\
             37=3000.0;281=5000.0\tFixture Missile\n\
             effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t\n",
            FIXTURE
        );
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(&rows).expect("the rows parse")),
        );
        assert_eq!(
            weapon_effect(Some(&c), LAUNCHER),
            Some(("useMissiles", 101))
        );
        assert_eq!(weapon_effect(Some(&c), PROBE_LAUNCHER), None);
        assert_eq!(missile_reach(&c, MISSILE), Some(15_000.0));
        assert_eq!(missile_reach(&c, LAUNCHER), None);
    }

    /// BP-HOLD: the sphere launcher (charge group 548) has the same effect as
    /// a missile launcher and is not a gun either. Invented rows and ids.
    #[test]
    fn the_sphere_launcher_is_not_a_gun() {
        const SPHERE_LAUNCHER: i64 = 990_960_001;
        let rows = format!(
            "{}\
             type\t{SPHERE_LAUNCHER}\t481\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,101\t\
             6=0.0;30=2.0;50=4.0;51=4000.0;604=548.0\tFixture Sphere Launcher\n\
             effect\t101\t1\t0\t51\t6\t0\t0\t0\tuseMissiles\t\n",
            FIXTURE
        );
        let mut c = Content::fixture();
        c.insert(
            "typedata",
            Installed::build(Table::parse(&rows).expect("the rows parse")),
        );
        assert_eq!(weapon_effect(Some(&c), SPHERE_LAUNCHER), None);
    }
}
