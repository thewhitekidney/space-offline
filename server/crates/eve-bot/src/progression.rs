//! **The combat career ladder** (PROG-PVE): how a bot that fights goes from a
//! small frigate to a battleship the way a player does, by skills first and
//! hulls as the wallet allows.
//!
//! Three pure pieces, each used where the bot already decides:
//!
//! 1. **Where the bot stands** ([`rung`]): the highest size class it flies or
//!    holds ([`crate::hull_choice::Class`]) and the race of that hull. A bot
//!    with no hull has no race yet, so it draws one from its name
//!    ([`Race::draw`]); the first hull it buys then sets it.
//! 2. **The skills for the next rung** ([`plan`]): the racial hull skill of
//!    each class up to the next one, that size's turret skill, support
//!    skills, the tank of the race's style and the T2 weapon specialisation.
//!    Appended after the career plan by [`crate::skill_plan::fill_for`]; the
//!    prerequisites come from the client's skill table as for every plan
//!    ([`crate::skill_plan::expand`]), so Battleship III brings Spaceship
//!    Command IV and the racial battlecruiser with it. The **names** are the
//!    client's [V-data]; every **level** is **[A]** (what a player trains
//!    before buying the hull).
//! 3. **The step up** ([`wanted`]): the classes above the flown one whose
//!    racial hull skill and turret skill are trained, which
//!    [`crate::drone_use`]'s `Upgrade` act buys from (`hull_choice::obtain_from`),
//!    at most once per [`UPGRADE_EVERY`].
//!
//! The server refuses a boarding for a missing hull skill once the skill
//! gate lands (SKILLGATE; before it, `hull_choice`'s header **[A]**); the bot
//! keeps the rule itself ([`acquire::flyable`]), which is why the step up
//! waits for the skills ([`FLY_LEVEL`]) and why even a frigate is bought only
//! when its racial skill is trained.

use crate::hull_choice::{Class, Weapon, ladder_class, ladder_hulls};
use crate::skill_plan::Want;
use crate::{ItemRow, acquire};
use std::collections::BTreeMap;
use std::time::Duration;

/// The four races whose hulls the builders make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Race {
    Amarr,
    Caldari,
    Gallente,
    Minmatar,
}

impl Race {
    pub const ALL: [Race; 4] = [Race::Amarr, Race::Caldari, Race::Gallente, Race::Minmatar];

    /// A race drawn from a seed, evenly -- except that a draw no frigate of
    /// the table can carry is drawn again (SKILLGATE): an odd seed makes a
    /// Caldari bot a hybrid one ([`weapon_of`]), and the table's only Caldari
    /// frigate has launchers and no turret, so that bot would have no hull to
    /// buy for its first rungs. The re-draw keeps the seed's other races
    /// evenly; a Caldari bot keeps missiles.
    pub fn draw(seed: u64) -> Race {
        for n in 0..8u64 {
            let r = Race::draw_once(seed.wrapping_add(n.wrapping_mul(0x2545_F491_4F6C_DD1D)));
            if r != Race::Caldari || seed.is_multiple_of(2) {
                return r;
            }
        }
        Race::Minmatar
    }

    fn draw_once(seed: u64) -> Race {
        let mut x = seed ^ 0x9E37_79B9_7F4A_7C15;
        x ^= x >> 31;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 29;
        Race::ALL[(x % 4) as usize]
    }

    /// The racial hull skill of `class` **[V-data]** (`invTypes` ids in the
    /// client's skill table: 3328-3331 frigate, 33091-33094 destroyer,
    /// 3332-3335 cruiser, 33095-33098 battlecruiser, 3336-3339 battleship,
    /// in Gallente, Caldari, Minmatar, Amarr order within each).
    pub fn hull_skill(self, class: Class) -> i64 {
        let slot = match self {
            Race::Gallente => 0,
            Race::Caldari => 1,
            Race::Minmatar => 2,
            Race::Amarr => 3,
        };
        match class {
            Class::Frigate => 3328 + [0, 2, 1, 3][slot],
            Class::Destroyer => 33_091 + [2, 1, 3, 0][slot],
            Class::Cruiser => 3332 + [0, 2, 1, 3][slot],
            Class::Battlecruiser => 33_095 + [2, 1, 3, 0][slot],
            Class::Battleship => 3336 + [0, 2, 1, 3][slot],
        }
    }

    /// The skill's name in the client's table.
    pub fn hull_skill_name(self, class: Class) -> &'static str {
        let names: [&'static str; 5] = match self {
            Race::Amarr => [
                "Amarr Frigate",
                "Amarr Destroyer",
                "Amarr Cruiser",
                "Amarr Battlecruiser",
                "Amarr Battleship",
            ],
            Race::Caldari => [
                "Caldari Frigate",
                "Caldari Destroyer",
                "Caldari Cruiser",
                "Caldari Battlecruiser",
                "Caldari Battleship",
            ],
            Race::Gallente => [
                "Gallente Frigate",
                "Gallente Destroyer",
                "Gallente Cruiser",
                "Gallente Battlecruiser",
                "Gallente Battleship",
            ],
            Race::Minmatar => [
                "Minmatar Frigate",
                "Minmatar Destroyer",
                "Minmatar Cruiser",
                "Minmatar Battlecruiser",
                "Minmatar Battleship",
            ],
        };
        names[class as usize]
    }

    /// Whether the race's ships are armour tanked **[A]**: Amarr and
    /// Gallente. Caldari are shield tanked; Minmatar are both, and take the
    /// shield here.
    pub fn armour(self) -> bool {
        matches!(self, Race::Amarr | Race::Gallente)
    }
}

/// The race of each hull in [`HULLS`] **[V-data]**: the race whose hull skill
/// the type asks for (attribute 182).
pub const HULL_RACES: &[(i64, Race)] = &[
    (587, Race::Minmatar),
    (585, Race::Minmatar),
    (598, Race::Minmatar),
    (597, Race::Amarr),
    (583, Race::Caldari),
    (608, Race::Gallente),
    (594, Race::Gallente),
    (16_242, Race::Minmatar),
    (16_236, Race::Amarr),
    (16_240, Race::Gallente),
    (16_238, Race::Caldari),
    (629, Race::Minmatar),
    (624, Race::Amarr),
    (627, Race::Gallente),
    (623, Race::Caldari),
    (16_233, Race::Amarr),
    (16_227, Race::Caldari),
    (16_229, Race::Gallente),
    (16_231, Race::Minmatar),
    (24_696, Race::Amarr),
    (24_698, Race::Caldari),
    (24_700, Race::Gallente),
    (24_702, Race::Minmatar),
    (24_688, Race::Caldari),
    (24_690, Race::Gallente),
    (24_692, Race::Amarr),
    (24_694, Race::Minmatar),
    (638, Race::Caldari),
    (639, Race::Minmatar),
    (641, Race::Gallente),
    (642, Race::Amarr),
    (643, Race::Amarr),
    (644, Race::Minmatar),
    (645, Race::Gallente),
];

/// **Whether the pilot may buy a table hull of its own race for its weapon**
/// (SKILLGATE, pure): a hull of `rung.race` whose skills are trained
/// ([`acquire::flyable`]) and which carries the weapon (a turret weapon needs
/// turret hardpoints, [`crate::hull_choice::HullInfo::turrets`]). A pilot
/// with none starts with that race's Frigate 1 and small turret 1
/// ([`crate::skill_plan::fill_with`]), not with whatever hull happens to be
/// flyable on its starter sheet.
pub fn has_usable_hull(rung: &Rung, levels: &BTreeMap<i64, i64>) -> bool {
    let turrets = !matches!(rung.weapon, Weapon::Missile | Weapon::Drones);
    ladder_hulls().any(|h| {
        race_of_hull(h.type_id) == Some(rung.race)
            && (!turrets || h.turrets > 0)
            && acquire::flyable(levels, h.type_id)
    })
}

/// The race of a hull type, if it is one of [`HULLS`].
pub fn race_of_hull(type_id: i64) -> Option<Race> {
    HULL_RACES
        .iter()
        .find(|(t, _)| *t == type_id)
        .map(|(_, r)| *r)
}

/// Where a combat bot stands on the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    pub race: Race,
    /// The highest class it flies or holds.
    pub class: Class,
    /// The weapon system its race favours ([`weapon_of`]).
    pub weapon: Weapon,
}

/// **The weapon system a race favours** (PROG-PVE2) **[A]**: Amarr lasers,
/// Gallente hybrids (and drones, [`uses_drones`]), Minmatar projectiles,
/// Caldari missiles or, for every odd `seed`, hybrids -- the race's two
/// systems. Falsified by a bot that cannot get its weapon's guns or rounds
/// at its station for long (then the choice is by what the markets stock),
/// or by a society whose bots all end on one system.
pub fn weapon_of(race: Race, seed: u64) -> Weapon {
    match race {
        Race::Amarr => Weapon::Laser,
        Race::Gallente => Weapon::Hybrid,
        Race::Minmatar => Weapon::Projectile,
        Race::Caldari => {
            if seed.is_multiple_of(2) {
                Weapon::Missile
            } else {
                Weapon::Hybrid
            }
        }
    }
}

/// Whether a race's ships field drones beside their guns **[A]**: Gallente.
pub fn uses_drones(race: Race) -> bool {
    race == Race::Gallente
}

/// **Where the bot stands** (pure): the highest class among the hull it
/// flies and the combat hulls in its hangar, and the race of that hull (the
/// flown one on a tie); with none, [`Class::Frigate`] and a race drawn from
/// `seed`.
pub fn rung(flying: Option<i64>, hangar: &[ItemRow], seed: u64) -> Rung {
    let held = hangar
        .iter()
        .filter(|r| r.category_id == acquire::CATEGORY_SHIP)
        .map(|r| r.type_id);
    let mut best: Option<(Class, i64)> = None;
    for t in flying.into_iter().chain(held) {
        let Some(c) = ladder_class(t) else {
            continue;
        };
        if best.is_none_or(|(b, _)| c > b) {
            best = Some((c, t));
        }
    }
    let (class, race) = match best {
        Some((class, t)) => (class, race_of_hull(t).unwrap_or_else(|| Race::draw(seed))),
        None => (Class::Frigate, Race::draw(seed)),
    };
    Rung {
        class,
        race,
        weapon: weapon_of(race, seed),
    }
}

/// **The hull a docked fighter is about to fly** (PROG-PVE2): the flown one
/// when it is a table hull, else the biggest class of the table's hulls in
/// the hangar (the lowest item id among equals) -- the hull a bot has just
/// bought and not yet boarded. `None` for a pilot with neither.
pub fn hull_to_fly(flying: Option<i64>, hangar: &[ItemRow]) -> Option<i64> {
    if let Some(f) = flying.filter(|f| ladder_class(*f).is_some()) {
        return Some(f);
    }
    hangar
        .iter()
        .filter(|r| r.category_id == acquire::CATEGORY_SHIP)
        .filter_map(|r| ladder_class(r.type_id).map(|c| (c, r)))
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.item_id.cmp(&a.1.item_id)))
        .map(|(_, r)| r.type_id)
}

/// The weapon of a bot about to fly `hull`: its race's, a race drawn from
/// `seed` for a hull the table does not know.
pub fn weapon_for(hull: Option<i64>, seed: u64) -> Weapon {
    let race = hull
        .and_then(race_of_hull)
        .unwrap_or_else(|| Race::draw(seed));
    weapon_of(race, seed)
}

/// The class after `c`, if any.
pub fn next(c: Class) -> Option<Class> {
    Class::ALL.get(c as usize + 1).copied()
}

/// One weapon's skills by size: the turret (or launcher) skill, then its
/// T2 specialisation, all by the client's names **[V-data]**.
fn weapon_skills(
    weapon: Weapon,
    size: crate::fit_plan::Size,
) -> (&'static str, Option<&'static str>) {
    use crate::fit_plan::Size::{Large, Medium, Small};
    match (weapon, size) {
        (Weapon::Projectile, Small) => (
            "Small Projectile Turret",
            Some("Small Autocannon Specialization"),
        ),
        (Weapon::Projectile, Medium) => (
            "Medium Projectile Turret",
            Some("Medium Autocannon Specialization"),
        ),
        (Weapon::Projectile, Large) => (
            "Large Projectile Turret",
            Some("Large Autocannon Specialization"),
        ),
        (Weapon::Hybrid, Small) => ("Small Hybrid Turret", Some("Small Railgun Specialization")),
        (Weapon::Hybrid, Medium) => (
            "Medium Hybrid Turret",
            Some("Medium Railgun Specialization"),
        ),
        (Weapon::Hybrid, Large) => ("Large Hybrid Turret", Some("Large Railgun Specialization")),
        (Weapon::Laser, Small) => (
            "Small Energy Turret",
            Some("Small Pulse Laser Specialization"),
        ),
        (Weapon::Laser, Medium) => (
            "Medium Energy Turret",
            Some("Medium Pulse Laser Specialization"),
        ),
        (Weapon::Laser, Large) => (
            "Large Energy Turret",
            Some("Large Pulse Laser Specialization"),
        ),
        (Weapon::Missile, Small) => ("Light Missiles", Some("Light Missile Specialization")),
        (Weapon::Missile, Medium) => ("Heavy Missiles", Some("Heavy Missile Specialization")),
        (Weapon::Missile, Large) => ("Cruise Missiles", Some("Cruise Missile Specialization")),
        (Weapon::Drones, Small) => ("Light Drone Operation", None),
        (Weapon::Drones, Medium) => ("Medium Drone Operation", None),
        (Weapon::Drones, Large) => ("Heavy Drone Operation", None),
    }
}

/// The skill that sets the weapon's rate of fire **[V-data]**.
fn rate_skill(weapon: Weapon) -> &'static str {
    match weapon {
        Weapon::Missile => "Rapid Launch",
        _ => "Rapid Firing",
    }
}

/// The skill that raises the weapon's damage **[V-data]**.
fn damage_skill(weapon: Weapon) -> &'static str {
    match weapon {
        Weapon::Missile => "Warhead Upgrades",
        _ => "Surgical Strike",
    }
}

/// The tank skills of a style, easiest first **[A]**.
fn tank_skills(armour: bool) -> [&'static str; 4] {
    if armour {
        [
            "Mechanics",
            "Hull Upgrades",
            "Armor Layering",
            "Repair Systems",
        ]
    } else {
        [
            "Shield Management",
            "Shield Upgrades",
            "Tactical Shield Manipulation",
            "Shield Operation",
        ]
    }
}

/// **The skills of one class's step** (pure), in the order a player trains
/// them. Every level **[A]**.
fn step(race: Race, weapon: Weapon, class: Class) -> Vec<Want> {
    use crate::fit_plan::fit_of;
    let size = fit_of(class).size;
    let (turret, spec) = weapon_skills(weapon, size);
    let tank = tank_skills(race.armour());
    let mut out: Vec<Want> = Vec::new();
    match class {
        Class::Frigate => {
            out.push((race.hull_skill_name(Class::Frigate), 3));
        }
        Class::Destroyer => {
            out.push((race.hull_skill_name(Class::Frigate), 4));
            out.push((race.hull_skill_name(Class::Destroyer), 3));
            out.push((turret, 4));
            if uses_drones(race) {
                out.push(("Drones", 3));
            }
        }
        Class::Cruiser => {
            out.push((race.hull_skill_name(Class::Cruiser), 3));
            out.push((turret, 3));
            out.push((rate_skill(weapon), 4));
            if weapon == Weapon::Laser {
                out.push(("Controlled Bursts", 3));
            }
            if uses_drones(race) {
                out.push(("Light Drone Operation", 4));
            }
            out.push((tank[0], 4));
            out.push((tank[1], 4));
            out.push((race.hull_skill_name(Class::Cruiser), 4));
        }
        Class::Battlecruiser => {
            out.push((race.hull_skill_name(Class::Battlecruiser), 3));
            out.push((turret, 4));
            out.push(("Weapon Upgrades", 4));
            out.push((damage_skill(weapon), 4));
            out.push((tank[2], 3));
            out.push((tank[3], 4));
            if let Some(s) = spec {
                out.push((s, 1));
            }
        }
        Class::Battleship => {
            out.push((race.hull_skill_name(Class::Battleship), 3));
            // Large turrets ask Gunnery V: the table's prerequisites bring it.
            out.push((turret, 3));
            out.push(("Advanced Weapon Upgrades", 2));
            out.push((race.hull_skill_name(Class::Battleship), 4));
            out.push((turret, 4));
            if let Some(s) = spec {
                out.push((s, 1));
            }
        }
    }
    out
}

/// **The ladder plan** (pure): the steps of every class above frigate up to
/// the one after `at.class` (a player trains the next hull's skills before
/// buying it), in class order. A bot already at battleship trains the
/// battleship step's tail only.
pub fn plan(at: Rung, weapon: Weapon) -> Vec<Want> {
    let upto = next(at.class).unwrap_or(at.class);
    let mut out: Vec<Want> = step(at.race, weapon, Class::Frigate);
    for c in Class::ALL.iter().skip(1).filter(|c| **c <= upto) {
        out.extend(step(at.race, weapon, *c));
    }
    out
}

/// Whether a role climbs the ladder: the fighters that take the ratter's or
/// the hunter's plan.
pub fn climbs(role: crate::brain::RoleRef) -> bool {
    let is = |r: crate::brain::RoleRef| std::ptr::eq(role, r);
    is(&crate::roles::ESCORT)
        || is(&crate::wing::ROLE)
        || is(&crate::wing_fight::LOGI_ROLE)
        || is(&crate::site_runner::ROLE)
        || is(&crate::missioner::ROLE)
        || is(&crate::plexer::ROLE)
        || is(&crate::hunt::ROLE)
        || is(&crate::hostile::ROLE)
}

/// The trained level a hull needs before the bot flies it **[V-data]**: the
/// type's required level of its hull skill (attribute 277) is 1 for every
/// T1 hull.
pub const FLY_LEVEL: i64 = 1;

/// How often a docked bot looks for a bigger hull **[A]**: it reads a market
/// page per hull type, so once an hour of play is plenty. Falsified by a bot
/// that sits in a frigate with the ISK for a cruiser for long (too slow).
pub const UPGRADE_EVERY: Duration = Duration::from_secs(30 * 60);

/// The most of its wallet an upgrade may spend on the hull, as a share
/// **[A]**; [`crate::hull_choice::SPEND_SHARE`] is the rule, this is the
/// extra a bot keeps for the guns and rounds the bigger fit needs
/// ([`crate::fit_plan`]).
pub const FIT_RESERVE: f64 = 0.15;

/// **Which hulls the bot may step up to** (pure): the table's hulls of a
/// class above `held`, whose racial hull skill and class turret skill the
/// bot has trained to [`FLY_LEVEL`] (`levels` is `{skill: trained level}`).
pub fn wanted(
    held: Class,
    levels: &BTreeMap<i64, i64>,
    weapon: Weapon,
) -> Vec<crate::hull_choice::HullInfo> {
    let trained = |skill: i64| levels.get(&skill).is_some_and(|l| *l >= FLY_LEVEL);
    ladder_hulls()
        .filter(|h| h.class > held)
        .filter(|h| {
            race_of_hull(h.type_id).is_some_and(|r| trained(r.hull_skill(h.class)))
                && trained(crate::fit_plan::fit_for(h.class, weapon).skill)
        })
        .copied()
        .collect()
}

/// **The frigates the pilot's skills board** (SKILLGATE, pure): a frigate
/// hull skill is trained like any other, and a new pilot has its own race's
/// only. With no content to read ([`acquire::flyable`]) every frigate.
fn flyable_frigates(levels: &BTreeMap<i64, i64>) -> Vec<crate::hull_choice::HullInfo> {
    ladder_hulls()
        .filter(|h| h.class == Class::Frigate && acquire::flyable(levels, h.type_id))
        .copied()
        .collect()
}

/// **Every hull the pilot may buy** (pure): the frigates its skills board
/// ([`flyable_frigates`]; every one when the content is unread), and
/// [`wanted`] above them.
pub fn allowed(levels: &BTreeMap<i64, i64>, weapon: Weapon) -> Vec<crate::hull_choice::HullInfo> {
    let mut out = flyable_frigates(levels);
    out.extend(wanted(Class::Frigate, levels, weapon));
    out
}

/// **The doctrine's hull, when the pilot can fly it** (DOC-5, pure): its
/// table row if the pilot has every skill the role fit names at its level
/// (the cheapest unlock first is the skill plan's business,
/// [`crate::skill_plan::merge_doctrine`]); `None` while one is missing.
pub fn doctrine_hull(
    levels: &BTreeMap<i64, i64>,
    fit: &crate::doctrine::RoleFit,
) -> Option<crate::hull_choice::HullInfo> {
    fit.skills
        .iter()
        .all(|&(s, l)| levels.get(&s).is_some_and(|have| *have >= l))
        .then(|| crate::hull_choice::HullInfo::doctrine(fit.hull))
}

/// [`wanted`] with a doctrine (DOC-5): **the doctrine's class replaces the
/// ladder.** With a fit, the only hull worth stepping up to is the
/// doctrine's, when it is above `held` and the pilot can fly it; a bot
/// already in the doctrine's class or above does not climb. Without one,
/// [`wanted`].
pub fn wanted_with(
    held: Class,
    levels: &BTreeMap<i64, i64>,
    weapon: Weapon,
    doctrine: Option<&crate::doctrine::RoleFit>,
) -> Vec<crate::hull_choice::HullInfo> {
    match doctrine {
        None => wanted(held, levels, weapon),
        Some(fit) => doctrine_hull(levels, fit)
            .filter(|h| h.class > held)
            .into_iter()
            .collect(),
    }
}

/// [`allowed`] with a doctrine (DOC-5): the frigates its skills board, then [`wanted_with`] above them -- and the doctrine's
/// hull at any class once the pilot can fly it (a pilot in a capsule buys
/// the corp's hull, not the ladder's first rung).
pub fn allowed_with(
    levels: &BTreeMap<i64, i64>,
    weapon: Weapon,
    doctrine: Option<&crate::doctrine::RoleFit>,
) -> Vec<crate::hull_choice::HullInfo> {
    let Some(fit) = doctrine else {
        return allowed(levels, weapon);
    };
    let mut out = flyable_frigates(levels);
    if let Some(h) = doctrine_hull(levels, fit)
        && !out.iter().any(|o| o.type_id == h.type_id)
    {
        out.push(h);
    }
    out
}

/// The wallet a bot passes to the hull chooser for an upgrade: less the
/// reserve for the bigger fit.
pub fn upgrade_wallet(wallet: f64) -> f64 {
    wallet * (1.0 - FIT_RESERVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A doctrine fit on an invented hull needing invented skills.
    fn fit(hull: i64, skills: &[(i64, i64)]) -> crate::doctrine::RoleFit {
        use crate::doctrine::testkit::{brawler, flat_tank};
        crate::doctrine::RoleFit {
            hull,
            modules: vec![],
            charges: vec![],
            skills: skills.to_vec(),
            profile: brawler(flat_tank(1.0, 1.0, 1.0)),
            cost: 1.0,
        }
    }

    #[test]
    fn a_doctrine_replaces_the_ladder_and_waits_for_its_skills() {
        const HULL: i64 = 970_600_001;
        const SKILL: i64 = 970_600_101;
        let f = fit(HULL, &[(SKILL, 3)]);
        let none = BTreeMap::new();
        let trained = BTreeMap::from([(SKILL, 3)]);
        // Skills short: no doctrine hull is offered, only the frigates.
        assert!(doctrine_hull(&none, &f).is_none());
        let a = allowed_with(&none, Weapon::Projectile, Some(&f));
        assert!(a.iter().all(|h| h.class == Class::Frigate));
        assert!(wanted_with(Class::Frigate, &none, Weapon::Projectile, Some(&f)).is_empty());
        // Trained: the doctrine hull, at its own class, joins the offer, and
        // is the only step up.
        let a = allowed_with(&trained, Weapon::Projectile, Some(&f));
        assert!(a.iter().any(|h| h.type_id == HULL));
        let up = wanted_with(Class::Frigate, &trained, Weapon::Projectile, Some(&f));
        assert_eq!(up.iter().map(|h| h.type_id).collect::<Vec<_>>(), [HULL]);
        // Already in its class or above: the bot does not climb.
        let class = up[0].class;
        assert!(wanted_with(class, &trained, Weapon::Projectile, Some(&f)).is_empty());
        // No doctrine: exactly the old ladder.
        assert_eq!(
            wanted_with(Class::Frigate, &trained, Weapon::Projectile, None),
            wanted(Class::Frigate, &trained, Weapon::Projectile)
        );
        assert_eq!(
            allowed_with(&trained, Weapon::Projectile, None),
            allowed(&trained, Weapon::Projectile)
        );
    }

    fn ship(type_id: i64) -> ItemRow {
        ItemRow {
            item_id: type_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: acquire::CATEGORY_SHIP,
            singleton: true,
        }
    }

    #[test]
    fn every_table_hull_has_a_race_and_a_racial_skill_of_its_class() {
        for h in ladder_hulls() {
            let r = race_of_hull(h.type_id);
            assert!(r.is_some(), "{} has no race", h.type_id);
        }
        // The four races take four distinct skills per class.
        for c in Class::ALL {
            let mut ids: Vec<i64> = Race::ALL.iter().map(|r| r.hull_skill(c)).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), 4, "{c:?}");
        }
        // The class steps are consecutive ids within a family.
        assert_eq!(Race::Gallente.hull_skill(Class::Frigate), 3328);
        assert_eq!(Race::Minmatar.hull_skill(Class::Frigate), 3329);
        assert_eq!(Race::Amarr.hull_skill(Class::Destroyer), 33_091);
        assert_eq!(Race::Minmatar.hull_skill(Class::Battleship), 3337);
        assert_eq!(Race::Caldari.hull_skill(Class::Battlecruiser), 33_096);
    }

    #[test]
    fn a_bots_race_follows_its_biggest_hull_and_a_hullless_bot_draws_one() {
        let (rifter, rupture) = (587, 629);
        let r = rung(Some(rifter), &[], 7);
        assert_eq!((r.class, r.race), (Class::Frigate, Race::Minmatar));
        let held = [ship(rupture), ship(1)];
        let r = rung(Some(rifter), &held, 7);
        assert_eq!(r.class, Class::Cruiser);
        let none = rung(None, &[], 7);
        assert_eq!(none.class, Class::Frigate);
        assert_eq!(none.race, Race::draw(7));
        // The draw is even enough to give every race to some of 200 bots.
        let seen: std::collections::BTreeSet<Race> = (0..200).map(Race::draw).collect();
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn the_plan_grows_one_class_ahead_of_the_hull() {
        let at = |class| Rung {
            race: Race::Minmatar,
            class,
            weapon: Weapon::Projectile,
        };
        let names = |p: &[Want]| p.iter().map(|w| w.0).collect::<Vec<_>>();
        let frigate = plan(at(Class::Frigate), Weapon::Projectile);
        assert!(names(&frigate).contains(&"Minmatar Destroyer"));
        assert!(!names(&frigate).contains(&"Minmatar Cruiser"));
        let cruiser = plan(at(Class::Cruiser), Weapon::Projectile);
        assert!(names(&cruiser).contains(&"Minmatar Battlecruiser"));
        assert!(!names(&cruiser).contains(&"Minmatar Battleship"));
        let bc = plan(at(Class::Battlecruiser), Weapon::Projectile);
        assert!(names(&bc).contains(&"Minmatar Battleship"));
        assert!(names(&bc).contains(&"Medium Autocannon Specialization"));
        assert!(names(&bc).contains(&"Large Projectile Turret"));
        let bs = plan(at(Class::Battleship), Weapon::Projectile);
        assert!(names(&bs).contains(&"Large Autocannon Specialization"));
        assert!(bs.len() > cruiser.len() && cruiser.len() > frigate.len());
    }

    #[test]
    fn the_race_sets_the_hull_skills_and_the_tank_style() {
        let amarr = plan(
            Rung {
                race: Race::Amarr,
                class: Class::Cruiser,
                weapon: Weapon::Laser,
            },
            Weapon::Laser,
        );
        let names: Vec<&str> = amarr.iter().map(|w| w.0).collect();
        assert!(names.contains(&"Amarr Battlecruiser"));
        assert!(names.contains(&"Armor Layering"));
        assert!(names.contains(&"Medium Energy Turret"));
        assert!(!names.contains(&"Shield Upgrades"));
        let caldari = plan(
            Rung {
                race: Race::Caldari,
                class: Class::Cruiser,
                weapon: Weapon::Missile,
            },
            Weapon::Missile,
        );
        let names: Vec<&str> = caldari.iter().map(|w| w.0).collect();
        assert!(names.contains(&"Shield Upgrades"));
        assert!(names.contains(&"Heavy Missiles"));
    }

    #[test]
    fn every_level_is_one_to_five_and_a_skill_never_goes_down() {
        for race in Race::ALL {
            for w in [
                Weapon::Projectile,
                Weapon::Hybrid,
                Weapon::Laser,
                Weapon::Missile,
                Weapon::Drones,
            ] {
                let p = plan(
                    Rung {
                        race,
                        class: Class::Battleship,
                        weapon: w,
                    },
                    w,
                );
                assert!(p.iter().all(|(_, l)| (1..=5).contains(l)));
                // A later mention of a skill is never lower than an earlier
                // one (skill_plan::expand raises in place).
                for (i, (n, l)) in p.iter().enumerate() {
                    assert!(
                        p[..i].iter().filter(|(m, _)| m == n).all(|(_, k)| k <= l),
                        "{n}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_step_up_waits_for_the_hull_skill_and_the_gun_skill() {
        let mut levels: BTreeMap<i64, i64> = BTreeMap::new();
        // Nothing trained: nothing to step up to.
        assert!(wanted(Class::Frigate, &levels, Weapon::Projectile).is_empty());
        // The Minmatar cruiser skill alone is not enough: the medium gun
        // skill is needed too.
        levels.insert(Race::Minmatar.hull_skill(Class::Cruiser), 1);
        assert!(wanted(Class::Frigate, &levels, Weapon::Projectile).is_empty());
        levels.insert(crate::fit_plan::SKILL_MEDIUM, 1);
        let got = wanted(Class::Frigate, &levels, Weapon::Projectile);
        assert!(!got.is_empty());
        assert!(
            got.iter()
                .all(|h| h.class == Class::Cruiser
                    && race_of_hull(h.type_id) == Some(Race::Minmatar))
        );
        // Only classes above the held one.
        assert!(wanted(Class::Cruiser, &levels, Weapon::Projectile).is_empty());
        // The battlecruiser skill adds the Minmatar battlecruiser too.
        levels.insert(Race::Minmatar.hull_skill(Class::Battlecruiser), 1);
        let got = wanted(Class::Frigate, &levels, Weapon::Projectile);
        assert!(got.iter().any(|h| h.class == Class::Battlecruiser));
    }

    #[test]
    fn a_new_pilot_may_buy_frigates_only_and_the_skills_open_the_rest_in_class_order() {
        let mut levels: BTreeMap<i64, i64> = BTreeMap::new();
        let classes = |l: &BTreeMap<i64, i64>| {
            let mut c: Vec<Class> = allowed(l, Weapon::Projectile)
                .iter()
                .map(|h| h.class)
                .collect();
            c.dedup();
            c
        };
        assert_eq!(classes(&levels), vec![Class::Frigate]);
        levels.insert(Race::Gallente.hull_skill(Class::Destroyer), 1);
        levels.insert(crate::fit_plan::SMALL.skill, 1);
        assert_eq!(classes(&levels), vec![Class::Frigate, Class::Destroyer]);
        // A skill alone never opens a hull of another race's.
        assert!(
            allowed(&levels, Weapon::Projectile)
                .iter()
                .filter(|h| h.class == Class::Destroyer)
                .all(|h| race_of_hull(h.type_id) == Some(Race::Gallente))
        );
        // Every frigate stays on offer.
        let frigates = ladder_hulls().filter(|h| h.class == Class::Frigate).count();
        assert_eq!(
            allowed(&levels, Weapon::Projectile)
                .iter()
                .filter(|h| h.class == Class::Frigate)
                .count(),
            frigates
        );
    }

    #[test]
    fn the_climbing_roles_are_the_fighters() {
        assert!(climbs(&crate::roles::ESCORT));
        assert!(climbs(&crate::missioner::ROLE));
        assert!(!climbs(&crate::roles::MINE));
        assert!(!climbs(&crate::builder::ROLE));
    }

    #[test]
    fn the_upgrade_leaves_a_reserve_for_the_fit() {
        assert!(upgrade_wallet(1_000.0) < 1_000.0 && upgrade_wallet(1_000.0) > 500.0);
    }

    #[test]
    fn a_race_favours_one_weapon_system_and_caldari_two() {
        assert_eq!(weapon_of(Race::Amarr, 5), Weapon::Laser);
        assert_eq!(weapon_of(Race::Gallente, 5), Weapon::Hybrid);
        assert_eq!(weapon_of(Race::Minmatar, 5), Weapon::Projectile);
        let caldari: std::collections::BTreeSet<String> = (0..4)
            .map(|s| format!("{:?}", weapon_of(Race::Caldari, s)))
            .collect();
        assert_eq!(caldari.len(), 2);
        assert!(uses_drones(Race::Gallente) && !uses_drones(Race::Amarr));
        // Among a few hundred bots every system is flown.
        let systems: std::collections::BTreeSet<String> = (0..400)
            .map(|s| format!("{:?}", weapon_of(Race::draw(s), s)))
            .collect();
        assert_eq!(systems.len(), 4, "{systems:?}");
    }

    #[test]
    fn a_rung_carries_the_weapon_of_its_hulls_race() {
        let (punisher, atron, rifter, kestrel) = (597, 608, 587, 583);
        assert_eq!(rung(Some(punisher), &[], 1).weapon, Weapon::Laser);
        assert_eq!(rung(Some(atron), &[], 1).weapon, Weapon::Hybrid);
        assert_eq!(rung(Some(rifter), &[], 1).weapon, Weapon::Projectile);
        let r = rung(Some(kestrel), &[], 1);
        assert_eq!(r.race, Race::Caldari);
        assert_eq!(r.weapon, weapon_of(Race::Caldari, 1));
        assert_eq!(weapon_for(Some(punisher), 9), Weapon::Laser);
        assert_eq!(
            weapon_for(None, 9),
            weapon_of(Race::draw(9), 9),
            "no hull: the drawn race's"
        );
    }

    #[test]
    fn the_hull_a_bot_is_about_to_fly_is_the_flown_one_else_the_best_it_holds() {
        let (rifter, rupture, punisher) = (587, 629, 597);
        // Flying a table hull: that one, whatever the hangar holds.
        assert_eq!(hull_to_fly(Some(rifter), &[ship(rupture)]), Some(rifter));
        // A rookie ship: the biggest table hull in the hangar.
        assert_eq!(
            hull_to_fly(Some(1), &[ship(punisher), ship(rupture), ship(2)]),
            Some(rupture)
        );
        assert_eq!(hull_to_fly(None, &[ship(2)]), None);
        assert_eq!(hull_to_fly(None, &[]), None);
    }

    #[test]
    fn the_plan_names_the_weapons_own_skills() {
        let names = |race, class, weapon| {
            plan(
                Rung {
                    race,
                    class,
                    weapon,
                },
                weapon,
            )
            .iter()
            .map(|w| w.0)
            .collect::<Vec<_>>()
        };
        let laser = names(Race::Amarr, Class::Cruiser, Weapon::Laser);
        assert!(laser.contains(&"Controlled Bursts") && laser.contains(&"Rapid Firing"));
        assert!(!laser.contains(&"Rapid Launch") && !laser.contains(&"Drones"));
        let missile = names(Race::Caldari, Class::Battlecruiser, Weapon::Missile);
        assert!(missile.contains(&"Rapid Launch") && missile.contains(&"Warhead Upgrades"));
        assert!(!missile.contains(&"Rapid Firing") && !missile.contains(&"Surgical Strike"));
        let hybrid = names(Race::Gallente, Class::Cruiser, Weapon::Hybrid);
        assert!(hybrid.contains(&"Medium Hybrid Turret"));
        assert!(hybrid.contains(&"Drones") && hybrid.contains(&"Light Drone Operation"));
        let projectile = names(Race::Minmatar, Class::Cruiser, Weapon::Projectile);
        assert!(!projectile.contains(&"Drones"));
    }

    #[test]
    fn a_step_up_waits_for_the_skill_of_the_bots_own_weapon() {
        let mut levels: BTreeMap<i64, i64> = BTreeMap::new();
        levels.insert(Race::Amarr.hull_skill(Class::Cruiser), 1);
        // The projectile gun skill is not a laser bot's.
        levels.insert(crate::fit_plan::SKILL_MEDIUM, 1);
        assert!(wanted(Class::Frigate, &levels, Weapon::Laser).is_empty());
        let laser = crate::fit_plan::fit_for(Class::Cruiser, Weapon::Laser);
        levels.insert(laser.skill, 1);
        let got = wanted(Class::Frigate, &levels, Weapon::Laser);
        assert!(!got.is_empty());
        assert!(
            got.iter()
                .all(|h| race_of_hull(h.type_id) == Some(Race::Amarr))
        );
        // And the projectile bot is not helped by the laser skill alone.
        levels.remove(&crate::fit_plan::SKILL_MEDIUM);
        assert!(wanted(Class::Frigate, &levels, Weapon::Projectile).is_empty());
    }
    /// A content whose every table hull asks for its race's skill of its class
    /// at level 1 (the real hull ids, as the bots' tables have them; the
    /// skill ids are the table's own). Caldari-style sheet: only that race's.
    fn gated_hulls() {
        let mut rows = String::new();
        for h in ladder_hulls() {
            let skill = race_of_hull(h.type_id).expect("a race").hull_skill(h.class);
            rows.push_str(&format!(
                "type\t{}\t25\t6\t1\t1\t20000.0\t130.0\t1200000.0\t33.0\t0\t5000\t\
                 182={skill};277=1\tFrigate {}\n",
                h.type_id, h.type_id
            ));
        }
        let table =
            eve_server::typedata::Table::parse(&format!("{}{rows}", eve_server::typedata::FIXTURE))
                .expect("the rows parse");
        let mut c = eve_server::content::Content::fixture();
        c.insert("typedata", eve_server::typedata::Installed::build(table));
        acquire::use_content(&std::sync::Arc::new(c));
    }

    /// SKILLGATE: on a Caldari-style sheet the only flyable frigate is the
    /// Caldari one, which has launchers and no turret: a turret bot of any
    /// race has no usable hull until its own race's skill is trained, a
    /// missile bot of the Caldari race has one at once.
    #[test]
    fn a_caldari_sheet_has_a_usable_hull_only_for_a_caldari_missile_bot() {
        gated_hulls();
        let sheet = BTreeMap::from([(Race::Caldari.hull_skill(Class::Frigate), 1)]);
        let rung = |race, weapon| Rung {
            race,
            class: Class::Frigate,
            weapon,
        };
        assert!(has_usable_hull(
            &rung(Race::Caldari, Weapon::Missile),
            &sheet
        ));
        assert!(!has_usable_hull(
            &rung(Race::Caldari, Weapon::Hybrid),
            &sheet
        ));
        for (race, weapon) in [
            (Race::Minmatar, Weapon::Projectile),
            (Race::Amarr, Weapon::Laser),
            (Race::Gallente, Weapon::Hybrid),
        ] {
            assert!(!has_usable_hull(&rung(race, weapon), &sheet), "{race:?}");
            let mut trained = sheet.clone();
            trained.insert(race.hull_skill(Class::Frigate), 1);
            assert!(has_usable_hull(&rung(race, weapon), &trained), "{race:?}");
        }
        // Only the Caldari-hybrid draw has no exit; the draw never makes it,
        // and the other races stay evenly drawn.
        let mut seen = BTreeMap::new();
        for seed in 0..2_000u64 {
            let race = Race::draw(seed);
            assert!(
                race != Race::Caldari || weapon_of(race, seed) == Weapon::Missile,
                "seed {seed}"
            );
            *seen.entry(race).or_insert(0u32) += 1;
        }
        assert_eq!(seen.len(), 4, "{seen:?}");
        assert!(seen.values().all(|n| *n >= 150), "{seen:?}");
    }
}
