//! **What a hull carries** (PROG-PVE, PROG-PVE2): the guns and rounds a combat
//! bot fits for the size class it flies and the weapon system its race
//! favours, so a battleship does not go out with two 125mm autocannons and an
//! Amarr bot does not fly projectile guns.
//!
//! The ratter's whole rule set was written for one fit, the Rifter's: a
//! 125mm Gatling AutoCannon I ([`crate::escort::GUN`]) and EMP S
//! ([`crate::escort::AMMO`]). This module is the table that makes the fit a
//! function of the hull's size class ([`crate::hull_choice::Class`]) and the
//! bot's weapon ([`Weapon`]):
//!
//! | weapon | small (frigate, destroyer) | medium (cruiser, battlecruiser) | large (battleship) |
//! |---|---|---|---|
//! | projectile | 125mm Gatling AutoCannon I, EMP S | Dual 180mm AutoCannon I, EMP M | Dual 425mm AutoCannon I, EMP L |
//! | hybrid | 75mm Gatling Rail I, Antimatter S | Dual 150mm Railgun I, Antimatter M | 425mm Railgun I, Antimatter L |
//! | laser | Small Focused Pulse Laser I, Multifrequency S | Focused Medium Pulse Laser I, Multifrequency M | Dual Heavy Pulse Laser I, Multifrequency L |
//! | missile | Light Missile Launcher I, Scourge Light | Heavy Missile Launcher I, Scourge Heavy | Cruise Missile Launcher I, Scourge Cruise |
//!
//! Every id and every size is **[V-data]**: the module's required skill
//! (attribute 182) is the size's turret or launcher skill, and the module's
//! `chargeSize` (attribute 128) is 1, 2 and 3 (a missile launcher states none:
//! its charge group, 384, 385 and 386, is the size), which is the test the
//! server runs when a round is loaded. The **gun counts** are **[A]** (CPU and
//! power grid decide what a hull can really carry; falsified by a `FitAuto`
//! the server refuses for want of either, after which the bot keeps what it
//! fitted, as the makeshift rule already does). A drone race (Gallente) fires
//! the hybrid fit and keeps drones beside it ([`crate::drone_use`]).
//!
//! The weapon follows the **race** ([`crate::progression::weapon_of`]); a
//! turret takes any hull's turret hardpoint and a launcher any launcher
//! hardpoint, whatever the hull's race. Everything here is pure.

use crate::ItemRow;
use crate::hull_choice::{Class, HULLS, Weapon};

/// A gun's size, and so its rounds'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Size {
    Small,
    Medium,
    Large,
}

/// What one size class fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    /// The weapon system.
    pub weapon: Weapon,
    pub size: Size,
    /// The gun module's type.
    pub gun: i64,
    /// Guns to fit **[A]**.
    pub guns: usize,
    /// The round the bot buys for it.
    pub round: i64,
    /// The turret or launcher skill the gun asks for.
    pub skill: i64,
}

/// 125mm Gatling AutoCannon I, EMP S, Small Projectile Turret **[V-data]**.
pub const SMALL: Fit = Fit {
    weapon: Weapon::Projectile,
    size: Size::Small,
    gun: crate::escort::GUN,
    guns: crate::escort::WANT_GUNS,
    round: crate::escort::AMMO,
    skill: crate::escort::SKILL_GUNNERY,
};
/// Dual 180mm AutoCannon I, EMP M, Medium Projectile Turret **[V-data]**.
pub const MEDIUM_GUN: i64 = 489;
pub const MEDIUM_ROUND: i64 = 193;
pub const SKILL_MEDIUM: i64 = 3305;
/// Dual 425mm AutoCannon I, EMP L, Large Projectile Turret **[V-data]**.
pub const LARGE_GUN: i64 = 494;
pub const LARGE_ROUND: i64 = 201;
pub const SKILL_LARGE: i64 = 3308;

/// One weapon's three sizes: `(gun, default round, skill)` **[V-data]**.
type Sizes = [(i64, i64, i64); 3];

/// 75mm Gatling Rail I, Dual 150mm Railgun I, 425mm Railgun I; Antimatter
/// Charge S, M, L; Small, Medium and Large Hybrid Turret.
const HYBRID: Sizes = [(561, 222, 3301), (567, 230, 3304), (574, 238, 3307)];
/// Small Focused Pulse Laser I, Focused Medium Pulse Laser I, Dual Heavy
/// Pulse Laser I; Multifrequency S, M, L; Small, Medium and Large Energy
/// Turret.
const LASER: Sizes = [(453, 246, 3303), (456, 254, 3306), (460, 262, 3309)];
/// Light, Heavy and Cruise Missile Launcher I; Scourge Light, Heavy and
/// Cruise Missile; Light, Heavy and Cruise Missiles.
const MISSILE: Sizes = [(499, 210, 3321), (501, 209, 3324), (13_320, 203, 3326)];
const PROJECTILE: Sizes = [
    (
        crate::escort::GUN,
        crate::escort::AMMO,
        crate::escort::SKILL_GUNNERY,
    ),
    (MEDIUM_GUN, MEDIUM_ROUND, SKILL_MEDIUM),
    (LARGE_GUN, LARGE_ROUND, SKILL_LARGE),
];

/// Every turret and launcher skill a fit of any weapon and size asks for:
/// what a dev grant adds to a fighter's ids, since a hull's own required
/// skills do not name its guns' (`dev::expand`).
pub fn gun_skills() -> Vec<i64> {
    [&PROJECTILE, &HYBRID, &LASER, &MISSILE]
        .iter()
        .flat_map(|t| t.iter().map(|&(_, _, skill)| skill))
        .collect()
}

/// A weapon's sizes. The drone race fires hybrids.
fn sizes(weapon: Weapon) -> (Weapon, &'static Sizes) {
    match weapon {
        Weapon::Projectile => (Weapon::Projectile, &PROJECTILE),
        Weapon::Hybrid | Weapon::Drones => (Weapon::Hybrid, &HYBRID),
        Weapon::Laser => (Weapon::Laser, &LASER),
        Weapon::Missile => (Weapon::Missile, &MISSILE),
    }
}

/// The gun size a class fits.
pub fn size_of_class(class: Class) -> Size {
    match class {
        Class::Frigate | Class::Destroyer => Size::Small,
        Class::Cruiser | Class::Battlecruiser => Size::Medium,
        Class::Battleship => Size::Large,
    }
}

/// How many guns a class fits **[A]**.
pub fn guns_of_class(class: Class) -> usize {
    match class {
        Class::Frigate | Class::Destroyer => crate::escort::WANT_GUNS,
        Class::Cruiser => 3,
        Class::Battlecruiser | Class::Battleship => 4,
    }
}

/// **How many guns a hull fits, from its own numbers**: the turret hardpoints
/// (attribute 102; launcher hardpoints, 101, for a missile weapon) capped by
/// the high slots ([`guns_of_class`] is only the guess for a hull that states
/// no hardpoint of the weapon).
pub fn guns_of_hardpoints(class: Class, hardpoints: usize, highs: usize) -> usize {
    match hardpoints.min(highs) {
        0 => guns_of_class(class),
        n => n,
    }
}

/// **The fit of a size class for a weapon.**
pub fn fit_for(class: Class, weapon: Weapon) -> Fit {
    let (weapon, table) = sizes(weapon);
    let size = size_of_class(class);
    let (gun, round, skill) = table[size as usize];
    Fit {
        weapon,
        size,
        gun,
        guns: guns_of_class(class),
        round,
        skill,
    }
}

/// **The fit of a size class, projectile** (the original table).
pub fn fit_of(class: Class) -> Fit {
    fit_for(class, Weapon::Projectile)
}

/// The fit for the hull type being flown and the bot's weapon: the small
/// fit of the weapon for a hull the table does not know (a capsule, the
/// rookie ship, the open mode's Rifter).
pub fn fit_for_bot(flying: Option<i64>, weapon: Weapon) -> Fit {
    let info = flying.and_then(|t| HULLS.iter().find(|h| h.type_id == t));
    let class = info.map_or(Class::Frigate, |h| h.class);
    let mut fit = fit_for(class, weapon);
    // The gun count is the hull's own hardpoints, not the class's guess
    // (the table lists turret hardpoints and the launcher hardpoints; the
    // high slot count is not in it, so the hardpoints alone cap the count).
    // The open mode's Rifter keeps the kit's [`crate::escort::WANT_GUNS`]
    // (its fixed fit, held by the fighter kit).
    if let Some(h) = info.filter(|h| h.type_id != crate::escort::RIFTER) {
        let hardpoints = if fit.weapon == Weapon::Missile {
            usize::from(launchers_of(h.type_id))
        } else {
            usize::from(h.turrets)
        };
        fit.guns = guns_of_hardpoints(class, hardpoints, usize::MAX);
    }
    fit
}

/// The projectile fit for the hull type being flown: [`SMALL`] for a hull
/// the table does not know.
pub fn fit_for_hull(flying: Option<i64>) -> Fit {
    fit_for_bot(flying, Weapon::Projectile)
}

/// `(skill, 1)` as the static row a [`crate::acquire::Kit`] carries.
fn skill_rows(skill: i64) -> &'static [(i64, i64)] {
    match skill {
        3301 => &[(3301, 1)],
        3302 => &[(3302, 1)],
        3303 => &[(3303, 1)],
        3304 => &[(3304, 1)],
        3305 => &[(3305, 1)],
        3306 => &[(3306, 1)],
        3307 => &[(3307, 1)],
        3308 => &[(3308, 1)],
        3309 => &[(3309, 1)],
        3321 => &[(3321, 1)],
        3324 => &[(3324, 1)],
        3326 => &[(3326, 1)],
        _ => crate::acquire::FIGHTER_KIT.skills,
    }
}

/// **A fighter's kit for the hull it flies and its weapon** (pure): the kit
/// with its guns, rounds and gun skill swapped for the flown class's
/// ([`fit_for_bot`]) when it is the small-gun fighter kit, so a salvager
/// bought for a cruiser does not drag two 125mm guns along and a laser bot
/// buys lasers. Any other kit (a miner's, a hauler's) is returned unchanged,
/// and so is the fighter kit for a projectile frigate or an unknown hull.
pub fn kit_for_hull(
    mut kit: crate::acquire::Kit,
    flying: Option<i64>,
    weapon: Weapon,
) -> crate::acquire::Kit {
    if kit.modules == crate::acquire::FIGHTER_KIT.modules {
        let fit = fit_for_bot(flying, weapon);
        kit.modules = Some((fit.gun, fit.guns as i64));
        kit.ammo = Some((fit.round, crate::escort::SETUP_ROUNDS));
        kit.skills = skill_rows(fit.skill);
    }
    kit
}

// ---- the doctrine's fit (DOC-2) ----------------------------------------------

/// `categoryModule` **[V]**.
const CATEGORY_MODULE: i64 = 7;
/// `requiredSkill1` (182), a gun's turret or launcher skill **[V-data]**.
const ATTR_REQUIRED_SKILL: i64 = 182;

/// **The gun fit a doctrine's [`crate::doctrine::RoleFit`] states**, read from
/// the table: the gun is the first module that takes the fit's primary round
/// ([`crate::doctrine_fit::rounds_of`]), its count is the fit's, its weapon
/// system the round's charge group, its size the gun's `chargeSize` (a
/// launcher states none: the round's group says), its skill the gun's own
/// requirement. `None` for a fit with no gun and round (a logi, a tackler).
pub fn fit_from_role(
    table: &eve_server::typedata::Table,
    role: &crate::doctrine::RoleFit,
) -> Option<Fit> {
    let &(round, _) = role.charges.first()?;
    let &(gun, guns) = role
        .modules
        .iter()
        .find(|(m, _)| crate::doctrine_fit::rounds_of(table, *m).contains(&round))?;
    let group = table.get(round)?.group_id;
    let weapon = weapon_of_group(group)?;
    let size = match table.get(gun)?.attribute(crate::ammo::ATTR_CHARGE_SIZE) {
        Some(s) if (s - 1.0).abs() < 1e-9 => Size::Small,
        Some(s) if (s - 2.0).abs() < 1e-9 => Size::Medium,
        Some(s) if (s - 3.0).abs() < 1e-9 => Size::Large,
        _ => size_of_round(round, group),
    };
    let skill = table.get(gun)?.attribute(ATTR_REQUIRED_SKILL)? as i64;
    Some(Fit {
        weapon,
        size,
        gun,
        guns: usize::from(guns),
        round,
        skill,
    })
}

/// **What a fighter buys** (DOC-2): without a doctrine, the race and weapon
/// defaults ([`kit_for_hull`], as PROG-PVE2 left them); with one, the role
/// fit's hull, every module at its count, its rounds and its skills
/// ([`crate::acquire::Order::from_fit`]) plus the tools the role's own kit
/// carries beside its guns (`more`: a ratter's salvager).
pub fn fight_order(
    kit: crate::acquire::Kit,
    flying: Option<i64>,
    weapon: Weapon,
    doctrine: Option<&crate::doctrine::RoleFit>,
) -> crate::acquire::Order {
    let plain = kit_for_hull(kit, flying, weapon);
    let Some(role) = doctrine else {
        return crate::acquire::Order::from_kit(&plain);
    };
    let mut order = crate::acquire::Order::from_fit(role);
    for &(t, n) in plain.more {
        order.want_module(t, n);
    }
    order
}

/// **The modules to fit** (DOC-2): the hangar's item ids, per module of the
/// role fit, up to its count less what is fitted already, in the fit's order
/// (the guns first). One `FitAuto` takes them all
/// ([`crate::Conn::fit_auto`]); the server picks the slots.
pub fn fit_ids(
    role: &crate::doctrine::RoleFit,
    hangar: &[ItemRow],
    fitted: &[ItemRow],
) -> Vec<i64> {
    let mut out = Vec::new();
    for &(t, n) in &role.modules {
        let have = fitted.iter().filter(|r| r.type_id == t).count();
        let need = usize::from(n).saturating_sub(have);
        out.extend(
            hangar
                .iter()
                .filter(|r| r.type_id == t && r.category_id == CATEGORY_MODULE && !r.is_fitted())
                .map(|r| r.item_id)
                .take(need),
        );
    }
    out
}

/// Every gun type of a weapon, with the most of it any class fits: what the
/// loot sale keeps ([`crate::escort::lots`]).
pub fn keep_guns(weapon: Weapon) -> [(i64, usize); 3] {
    let most = |size: Size| {
        Class::ALL
            .iter()
            .filter(|c| size_of_class(**c) == size)
            .map(|c| guns_of_class(*c))
            .max()
            .unwrap_or(0)
    };
    let (_, table) = sizes(weapon);
    [
        (table[0].0, most(Size::Small)),
        (table[1].0, most(Size::Medium)),
        (table[2].0, most(Size::Large)),
    ]
}

// ---- rounds ------------------------------------------------------------------

/// The charge groups of the rounds the bots fire **[V-data]**: projectile
/// ammo, hybrid charges, frequency crystals, light, heavy and cruise missiles.
pub const GROUP_PROJECTILE: i64 = 83;
pub const GROUP_HYBRID: i64 = 85;
pub const GROUP_LASER: i64 = 86;
pub const GROUP_LIGHT_MISSILE: i64 = 384;
pub const GROUP_HEAVY_MISSILE: i64 = 385;
pub const GROUP_CRUISE_MISSILE: i64 = 386;
/// Heavy assault missiles (a medium round, taken by the HAM launcher) and
/// torpedoes (a large round) **[V-data]**: the `chargeGroup1` of the
/// launchers that fire them.
pub const GROUP_HEAVY_ASSAULT_MISSILE: i64 = 772;
pub const GROUP_TORPEDO: i64 = 89;

/// The weapon whose rounds are of this charge group.
pub fn weapon_of_group(group: i64) -> Option<Weapon> {
    match group {
        GROUP_PROJECTILE => Some(Weapon::Projectile),
        GROUP_HYBRID => Some(Weapon::Hybrid),
        GROUP_LASER => Some(Weapon::Laser),
        GROUP_LIGHT_MISSILE
        | GROUP_HEAVY_MISSILE
        | GROUP_CRUISE_MISSILE
        | GROUP_HEAVY_ASSAULT_MISSILE
        | GROUP_TORPEDO => Some(Weapon::Missile),
        _ => None,
    }
}

/// The size of a round by its type and charge group: the T1 projectile,
/// hybrid and laser rounds come in runs of eight (small, medium, large:
/// projectile 178-201, hybrid 215-238, laser 239-262 **[V-data]**), a missile's
/// size is its group's; any other round (a faction or T2 variant, the
/// starter rung's) is small.
pub fn size_of_round(type_id: i64, group: i64) -> Size {
    let run = |first: i64| -> Option<Size> {
        match type_id - first {
            0..=7 => Some(Size::Small),
            8..=15 => Some(Size::Medium),
            16..=23 => Some(Size::Large),
            _ => None,
        }
    };
    match group {
        GROUP_HEAVY_MISSILE | GROUP_HEAVY_ASSAULT_MISSILE => Size::Medium,
        GROUP_CRUISE_MISSILE | GROUP_TORPEDO => Size::Large,
        GROUP_PROJECTILE => run(178).unwrap_or(Size::Small),
        GROUP_HYBRID => run(215).unwrap_or(Size::Small),
        GROUP_LASER => run(239).unwrap_or(Size::Small),
        _ => Size::Small,
    }
}

/// Whether a stack is a round of `weapon`, of any size.
pub fn is_round_of(weapon: Weapon, r: &ItemRow) -> bool {
    r.category_id == crate::CATEGORY_CHARGE
        && match weapon_of_group(r.group_id) {
            Some(w) => w == sizes(weapon).0,
            None => r.type_id == crate::escort::AMMO && sizes(weapon).0 == Weapon::Projectile,
        }
}

/// Whether a stack is a round for **this fit**: the same weapon system and
/// size.
pub fn suits(fit: &Fit, r: &ItemRow) -> bool {
    is_round_of(fit.weapon, r) && size_of_round(r.type_id, round_group(fit.weapon, r)) == fit.size
}

/// The size of a round stack for `weapon`'s system.
pub fn round_size(weapon: Weapon, r: &ItemRow) -> Size {
    size_of_round(r.type_id, round_group(weapon, r))
}

/// The group a round counts under: its own, else the projectile group for
/// the one round the table names without a group on the row.
fn round_group(weapon: Weapon, r: &ItemRow) -> i64 {
    if weapon_of_group(r.group_id).is_some() {
        r.group_id
    } else if sizes(weapon).0 == Weapon::Projectile {
        GROUP_PROJECTILE
    } else {
        r.group_id
    }
}

/// The round a gun type is measured with (its magazine, `modules::rounds_capacity`):
/// its own default round, [`crate::escort::AMMO`] for any other gun.
pub fn round_of_gun(gun: i64) -> i64 {
    for table in [&PROJECTILE, &HYBRID, &LASER, &MISSILE] {
        if let Some(row) = table.iter().find(|r| r.0 == gun) {
            return row.1;
        }
    }
    crate::escort::AMMO
}

// ---- hardpoints --------------------------------------------------------------

/// The launcher hardpoints (attribute 101) of the table hulls that have any
/// **[V-data]**: a missile bot flies a hull only if it has as many as the
/// class fits launchers ([`guns_of_class`]).
pub const LAUNCHERS: &[(i64, u8)] = &[
    (587, 2),
    (598, 3),
    (583, 3),
    (16_242, 1),
    (16_238, 1),
    (629, 1),
    (16_233, 4),
    (16_231, 5),
    (24_698, 6),
    (24_702, 3),
    (24_688, 4),
    (24_690, 1),
    (24_692, 1),
    (638, 6),
    (639, 4),
    (643, 5),
    (644, 6),
];

/// A table hull's launcher hardpoints (0 when it has none or is unknown).
pub fn launchers_of(type_id: i64) -> u8 {
    LAUNCHERS
        .iter()
        .find(|(t, _)| *t == type_id)
        .map_or(0, |(_, n)| *n)
}

// ---- what a builder can make for the fits ----------------------------------------

/// Every gun and round type of one weapon system (all three sizes, all T1
/// rounds of its groups): what `hulls::arms_rungs` offers a builder as one
/// rung, so a bot's weapon has a maker **[A]**.
pub fn products_of(weapon: Weapon) -> Vec<i64> {
    let (w, table) = sizes(weapon);
    let mut out: Vec<i64> = table.iter().flat_map(|r| [r.0, r.1]).collect();
    let run = match w {
        Weapon::Projectile => 178..=201,
        Weapon::Hybrid => 215..=238,
        Weapon::Laser => 239..=262,
        _ => 202..=213,
    };
    out.extend(run);
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hull_fits_as_many_guns_as_its_hardpoints_and_highs_allow() {
        // An invented battleship with eight turret hardpoints and eight highs.
        assert_eq!(guns_of_hardpoints(Class::Battleship, 8, 8), 8);
        // A frigate with three hardpoints and four highs fits three, not two.
        assert_eq!(guns_of_hardpoints(Class::Frigate, 3, 4), 3);
        // Highs cap the hardpoints.
        assert_eq!(guns_of_hardpoints(Class::Cruiser, 5, 4), 4);
        // A hull with no hardpoint of the weapon keeps the class's guess.
        assert_eq!(
            guns_of_hardpoints(Class::Battlecruiser, 0, 7),
            guns_of_class(Class::Battlecruiser)
        );
        // A launcher cruiser with four launchers fits four.
        assert_eq!(guns_of_hardpoints(Class::Cruiser, 4, 6), 4);
    }

    fn charge(type_id: i64, group: i64) -> ItemRow {
        ItemRow {
            item_id: type_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 100,
            group_id: group,
            category_id: crate::CATEGORY_CHARGE,
            singleton: false,
        }
    }

    const WEAPONS: [Weapon; 4] = [
        Weapon::Projectile,
        Weapon::Hybrid,
        Weapon::Laser,
        Weapon::Missile,
    ];

    /// The charge group a fit's own round has.
    fn group_of(f: &Fit) -> i64 {
        match (f.weapon, f.size) {
            (Weapon::Projectile, _) => GROUP_PROJECTILE,
            (Weapon::Hybrid, _) => GROUP_HYBRID,
            (Weapon::Laser, _) => GROUP_LASER,
            (_, Size::Small) => GROUP_LIGHT_MISSILE,
            (_, Size::Medium) => GROUP_HEAVY_MISSILE,
            (_, Size::Large) => GROUP_CRUISE_MISSILE,
        }
    }

    #[test]
    fn a_bigger_class_never_fits_a_smaller_gun() {
        for w in WEAPONS {
            let sizes: Vec<Size> = Class::ALL.iter().map(|c| fit_for(*c, w).size).collect();
            assert!(sizes.windows(2).all(|p| p[0] <= p[1]), "{w:?} {sizes:?}");
            assert_eq!(fit_for(Class::Battleship, w).size, Size::Large);
        }
        assert_eq!(fit_of(Class::Frigate), SMALL);
    }

    #[test]
    fn the_rifter_keeps_its_old_fit() {
        let f = fit_for_hull(Some(crate::escort::RIFTER));
        assert_eq!(f.gun, crate::escort::GUN);
        assert_eq!(f.guns, crate::escort::WANT_GUNS);
        assert_eq!(f.round, crate::escort::AMMO);
        assert_eq!(fit_for_hull(None), SMALL);
        assert_eq!(fit_for_hull(Some(1)), SMALL, "a type the table lacks");
    }

    #[test]
    fn every_weapon_has_its_own_gun_round_and_skill_in_every_size() {
        let mut guns = Vec::new();
        for w in WEAPONS {
            for c in Class::ALL {
                let f = fit_for(c, w);
                assert_eq!(f.weapon, w);
                guns.push((f.gun, f.size));
            }
        }
        guns.sort_unstable();
        guns.dedup();
        // Four weapons, three sizes: twelve distinct guns.
        assert_eq!(guns.len(), 12);
        // The drone race fires the hybrid fit.
        assert_eq!(
            fit_for(Class::Cruiser, Weapon::Drones),
            fit_for(Class::Cruiser, Weapon::Hybrid)
        );
        assert_ne!(
            fit_for(Class::Frigate, Weapon::Laser).gun,
            fit_for(Class::Frigate, Weapon::Hybrid).gun
        );
    }

    #[test]
    fn rounds_have_the_size_and_weapon_of_the_gun_that_fires_them() {
        for w in WEAPONS {
            for c in Class::ALL {
                let f = fit_for(c, w);
                assert!(suits(&f, &charge(f.round, group_of(&f))), "{w:?} {c:?}");
                for other in Class::ALL {
                    let o = fit_for(other, w);
                    assert_eq!(
                        suits(&f, &charge(o.round, group_of(&o))),
                        f.size == o.size,
                        "{w:?} {c:?} {other:?}"
                    );
                }
            }
        }
        // A laser bot's gun does not take a hybrid charge, nor the reverse.
        let laser = fit_for(Class::Frigate, Weapon::Laser);
        let hybrid = fit_for(Class::Frigate, Weapon::Hybrid);
        assert!(!suits(&laser, &charge(hybrid.round, GROUP_HYBRID)));
        assert!(!suits(&hybrid, &charge(laser.round, GROUP_LASER)));
        assert!(!suits(&SMALL, &charge(hybrid.round, GROUP_HYBRID)));
        // The projectile table's own default round is small by its type
        // alone, as the original rule was.
        assert_eq!(
            size_of_round(crate::escort::AMMO, GROUP_PROJECTILE),
            Size::Small
        );
        assert_eq!(round_of_gun(MEDIUM_GUN), MEDIUM_ROUND);
        assert_eq!(round_of_gun(crate::escort::GUN), crate::escort::AMMO);
        assert_eq!(round_of_gun(laser.gun), laser.round);
        assert_eq!(round_of_gun(1), crate::escort::AMMO);
    }

    #[test]
    fn a_projectile_bot_does_not_count_a_foreign_round_as_its_own() {
        let own = charge(crate::escort::AMMO, GROUP_PROJECTILE);
        let foreign = charge(222, GROUP_HYBRID);
        assert!(is_round_of(Weapon::Projectile, &own));
        assert!(!is_round_of(Weapon::Projectile, &foreign));
        assert!(is_round_of(Weapon::Hybrid, &foreign));
        // A module is never a round.
        let mut gun = own;
        gun.category_id = crate::acquire::CATEGORY_SHIP;
        assert!(!is_round_of(Weapon::Projectile, &gun));
    }

    #[test]
    fn a_fighter_kit_follows_the_hull_and_the_weapon_and_other_kits_do_not() {
        let fighter = crate::acquire::FIGHTER_KIT;
        assert_eq!(
            kit_for_hull(fighter, Some(crate::escort::RIFTER), Weapon::Projectile),
            fighter
        );
        assert_eq!(kit_for_hull(fighter, None, Weapon::Projectile), fighter);
        let big = kit_for_hull(fighter, Some(24_692), Weapon::Projectile);
        assert_eq!(
            big.modules,
            Some((
                LARGE_GUN,
                fit_for_bot(Some(24_692), Weapon::Projectile).guns as i64
            ))
        );
        assert_eq!(big.ammo.map(|a| a.0), Some(LARGE_ROUND));
        assert_eq!(big.hull, fighter.hull);
        // A laser frigate buys lasers, the laser skill and crystals.
        let laser = fit_for(Class::Frigate, Weapon::Laser);
        let kit = kit_for_hull(fighter, None, Weapon::Laser);
        assert_eq!(kit.modules, Some((laser.gun, laser.guns as i64)));
        assert_eq!(kit.ammo.map(|a| a.0), Some(laser.round));
        assert_eq!(kit.skills, &[(laser.skill, 1)]);
        let miner = crate::acquire::MINER_KIT;
        assert_eq!(kit_for_hull(miner, Some(24_692), Weapon::Laser), miner);
    }

    #[test]
    fn every_fit_skill_has_a_static_kit_row() {
        for w in WEAPONS {
            for c in Class::ALL {
                let f = fit_for(c, w);
                assert_eq!(skill_rows(f.skill), &[(f.skill, 1)], "{w:?} {c:?}");
            }
        }
    }

    #[test]
    fn the_loot_sale_keeps_every_class_its_guns() {
        for w in WEAPONS {
            let keep = keep_guns(w);
            for c in Class::ALL {
                let f = fit_for(c, w);
                let kept = keep.iter().find(|(g, _)| *g == f.gun).map(|(_, n)| *n);
                assert!(kept.is_some_and(|n| n >= f.guns), "{w:?} {c:?}");
            }
        }
        assert_eq!(
            keep_guns(Weapon::Projectile)[0],
            (crate::escort::GUN, crate::escort::WANT_GUNS)
        );
    }

    #[test]
    fn a_missile_bot_needs_the_launchers_and_a_turret_bot_does_not() {
        // The Rifter has two launcher hardpoints: enough for a frigate's fit.
        assert!(launchers_of(crate::escort::RIFTER) as usize >= guns_of_class(Class::Frigate));
        assert_eq!(launchers_of(1), 0);
        // The Rupture has one: not enough for a cruiser's three.
        assert!((launchers_of(629) as usize) < guns_of_class(Class::Cruiser));
    }

    #[test]
    fn a_builder_can_be_offered_each_weapon_system_whole() {
        for w in WEAPONS {
            let products = products_of(w);
            for c in Class::ALL {
                let f = fit_for(c, w);
                assert!(products.contains(&f.gun) && products.contains(&f.round));
            }
        }
        // The systems do not share a product.
        let p = products_of(Weapon::Projectile);
        assert!(!products_of(Weapon::Laser).iter().any(|t| p.contains(t)));
    }

    // ---- DOC-2 ---------------------------------------------------------------

    const D_HULL: i64 = 990_960_001;
    const D_GUN: i64 = 990_960_002;
    const D_PLATE: i64 = 990_960_003;
    const D_ROUND: i64 = 990_960_004;
    const D_ALT: i64 = 990_960_005;
    const D_GROUP_ROUND: i64 = 85;

    fn doctrine_table() -> eve_server::typedata::Table {
        // A medium hybrid gun (charge group 85, size 2) with its two rounds
        // and a plate.
        let rows = format!(
            "{}\
             type\t{D_GUN}\t74\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
             6=0.0;30=2.0;50=4.0;51=2500.0;54=900.0;64=2.5;128=2.0;158=4000.0;160=400.0;182=3304.0;604=85.0\tFixture Rail\n\
             type\t{D_ROUND}\t{D_GROUP_ROUND}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t114=7.0;117=3.0;128=2.0\tFixture Charge A\n\
             type\t{D_ALT}\t{D_GROUP_ROUND}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t116=7.0;118=3.0;128=2.0\tFixture Charge B\n\
             type\t{D_PLATE}\t329\t7\t1\t1\t5.0\t0.0\t0.0\t1.0\t0\t11,16\t30=3.0;50=10.0\tFixture Plate\n",
            eve_server::typedata::FIXTURE
        );
        eve_server::typedata::Table::parse(&rows).expect("the rows parse")
    }

    fn doctrine_fit() -> crate::doctrine::RoleFit {
        crate::doctrine::RoleFit {
            hull: D_HULL,
            modules: vec![(D_GUN, 3), (D_PLATE, 2)],
            charges: vec![(D_ROUND, 60), (D_ALT, 20)],
            skills: vec![(3304, 3), (3392, 1)],
            profile: crate::doctrine::testkit::brawler(crate::doctrine::testkit::flat_tank(
                1.0, 1.0, 1.0,
            )),
            cost: 1.0,
        }
    }

    fn module_row(item: i64, type_id: i64, flag: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: flag,
            quantity: 1,
            group_id: 74,
            category_id: CATEGORY_MODULE,
            singleton: true,
        }
    }

    #[test]
    fn the_fight_kit_follows_the_role_fit() {
        use crate::acquire::{FIGHTER_KIT, Order};
        let table = doctrine_table();
        let role = doctrine_fit();
        // The fit states its gun: the module that takes the primary round.
        let fit = fit_from_role(&table, &role).expect("a gun and a round");
        assert_eq!(
            (
                fit.weapon, fit.size, fit.gun, fit.guns, fit.round, fit.skill
            ),
            (Weapon::Hybrid, Size::Medium, D_GUN, 3, D_ROUND, 3304)
        );
        // A fit with no gun states none.
        let mut logi = role.clone();
        logi.modules = vec![(D_PLATE, 2)];
        assert_eq!(fit_from_role(&table, &logi), None);
        // With a doctrine the order is the fit's: hull, parts, rounds, skills.
        let o = fight_order(FIGHTER_KIT, Some(588), Weapon::Projectile, Some(&role));
        assert_eq!(o, Order::from_fit(&role));
        // Without one, the race and weapon defaults of PROG-PVE2: a laser
        // bot flying a frigate buys the small laser, its crystal, its skill.
        let none = fight_order(FIGHTER_KIT, None, Weapon::Laser, None);
        let small = fit_for(Class::Frigate, Weapon::Laser);
        assert_eq!(none.modules, vec![(small.gun, small.guns as i64)]);
        assert_eq!(
            none.charges,
            vec![(small.round, crate::escort::SETUP_ROUNDS)]
        );
        assert_eq!(none.skills, vec![(small.skill, 1)]);
        assert_eq!(none.hull, FIGHTER_KIT.hull);
        // The modules to fit: the guns first, as many as the fit says less
        // what is fitted; a fitted plate is not taken twice.
        let hangar = [
            module_row(1, D_PLATE, 4),
            module_row(2, D_GUN, 4),
            module_row(3, D_GUN, 4),
            module_row(4, D_GUN, 4),
            module_row(5, D_GUN, 4),
            module_row(6, D_PLATE, 4),
            module_row(7, D_PLATE, 4),
        ];
        let fitted = [module_row(9, D_PLATE, 12)];
        assert_eq!(fit_ids(&role, &hangar, &fitted), vec![2, 3, 4, 1]);
        assert!(fit_ids(&role, &hangar, &[]).len() == 5);
    }
}
