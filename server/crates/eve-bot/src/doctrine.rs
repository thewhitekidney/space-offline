//! **Doctrines** (DOC-1, R-DOCTRINE section 2): a role-weighted set of paper
//! fits, derived by search from the type table and the dogma engine, then
//! scored by one pure exchange function ([`crate::doctrine_exchange`]).
//!
//! Pure: no socket, no clock, no random. A world without a typedata table
//! (or with a bundle whose parts are not [`Makeable`]) has an empty catalog.
//!
//! | file | what |
//! |---|---|
//! | `doctrine.rs` (this) | the model: axes, [`Profile`], labels, [`catalog`] |
//! | `doctrine_fit.rs` | classifying parts, [`crate::doctrine_fit::fill_slots`], the paper evaluation |
//! | `doctrine_exchange.rs` | the counter matrix as a Lanchester exchange, the `[doc] matrix` line |
//!
//! **Nothing is a hand table of "armour beats shield".** Every number a rule
//! reads is computed by [`eve_server::dogma::evaluate`] on a paper fit, and
//! the application terms are the server's own functions
//! ([`eve_server::combat::Application`], [`eve_server::missile`],
//! [`eve_server::ewar`]), so the paper and the engine cannot disagree.
//!
//! **Supersedes, does not replace.** [`crate::fit_plan`] and [`crate::tank`]
//! keep serving the bots that fly today; DOC-2 makes them follow a
//! [`RoleFit`]. [`crate::doctrine_fit::rounds_of`] is [`crate::ammo::rounds_for`]
//! over a bare table.

use crate::doctrine_fit::{Library, Limits};
use crate::hull_choice::Class;
use eve_server::damage::{self, Damage};
use eve_server::missile::Aoe;
use eve_server::typedata::Installed;
use std::collections::BTreeSet;

// ---- the named constants. Each [A] has its falsifier ----------------------

/// A primary gun's reach (optimal + half its falloff) under this is Short
/// **[A]**. Falsified by a catalog whose labels are all one value.
pub const RANGE_SHORT_M: f64 = 15_000.0;
/// ... and over this is Long **[A]** (same falsifier).
pub const RANGE_LONG_M: f64 = 45_000.0;
/// A fit is a Kite when its speed is this multiple of the catalog's median
/// fit speed and it shoots from Medium or Long **[A]**. Falsified by a
/// catalog with no kite or only kites.
pub const KITE_SPEED: f64 = 1.3;
/// Tackle share of the fleet (a Brawl needs it) **[A]**. Falsified by a
/// soak in which a doctrine labelled Brawl never holds a target.
pub const BRAWL_TACKLE: f64 = 0.15;
/// The highest meta level a part may have **[A]**: T1 and named-free. DOC-7
/// raises it when builders can invent. Falsified by builders making T2.
pub const META_MAX: f64 = 1.0;
/// Drop-and-retry rounds when a fit overflows CPU, power or capacitor
/// **[A]**. Raised from 6 when the disruptors joined the ewar rack: a
/// battleship's ewar fit needed seven drops to hold on capacitor. Falsified
/// by a catalog whose fits shrink to a few modules.
pub const FIT_RETRIES: usize = 8;
/// Ships on one grid (G-FIGHT).
pub const GRID_CAP: usize = 30;
/// The level the doctrine assumes for every skill it lists **[A]**.
/// Falsified by a leader whose members cannot reach it in a week.
pub const DOCTRINE_LEVEL: i64 = 4;
/// The tank layer must hold this share of the fitted EHP to be the label
/// (the argmax in practice) **[A]**.
pub const LAYER_SHARE: f64 = 0.6;
/// Role shares of a full set **[A]**; a role with no fit gives its share to
/// the damage dealers.
pub const SHARE_LOGI: f64 = 0.15;
pub const SHARE_TACKLE: f64 = 0.15;
pub const SHARE_EWAR: f64 = 0.10;
/// BP-T2-HULLS: the interdictors and the scouts are a few ships a fleet **[A]**:
/// one bubble and one eye serve a gang of a dozen. A role that has no fit (no
/// builder has supplied the hull) gives its share back to the damage dealers.
/// Falsified by a gang that holds a bubble with one dictor and wants a second,
/// or by a camp that never posts a scout.
pub const SHARE_INTERDICT: f64 = 0.05;
pub const SHARE_SCOUT: f64 = 0.05;
/// A resonance under this is read as this (an invulnerable layer would give
/// an infinite EHP).
pub const RES_FLOOR: f64 = 0.01;

// ---- the axes -------------------------------------------------------------

/// The weapon family a fit shoots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    Projectile,
    Hybrid,
    Laser,
    Missile,
    /// Named for the axis; the catalog does not build drone fits yet
    /// (needs a drone profile; DOC-1 Not done).
    Drone,
}

impl Family {
    pub const ALL: [Family; 5] = [
        Family::Projectile,
        Family::Hybrid,
        Family::Laser,
        Family::Missile,
        Family::Drone,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Family::Projectile => "projectile",
            Family::Hybrid => "hybrid",
            Family::Laser => "laser",
            Family::Missile => "missile",
            Family::Drone => "drone",
        }
    }

    /// The family of a bot weapon bonus ([`crate::hull_choice::Weapon`]).
    pub fn of_weapon(w: crate::hull_choice::Weapon) -> Family {
        use crate::hull_choice::Weapon;
        match w {
            Weapon::Projectile => Family::Projectile,
            Weapon::Hybrid => Family::Hybrid,
            Weapon::Laser => Family::Laser,
            Weapon::Missile => Family::Missile,
            Weapon::Drones => Family::Drone,
        }
    }
}

/// Where the fitted EHP is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Shield,
    Armour,
    Hull,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::Shield => "shield",
            Layer::Armour => "armour",
            Layer::Hull => "hull",
        }
    }
}

/// How far the primary gun with its loaded round reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Range {
    Short,
    Medium,
    Long,
}

impl Range {
    pub const ALL: [Range; 3] = [Range::Short, Range::Medium, Range::Long];

    pub fn name(self) -> &'static str {
        match self {
            Range::Short => "short",
            Range::Medium => "medium",
            Range::Long => "long",
        }
    }
}

/// Derived, never chosen by hand ([`stance_of`]). As a template it is the
/// utility slots a fit is built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stance {
    Brawl,
    Skirmish,
    Kite,
}

impl Stance {
    pub const ALL: [Stance; 3] = [Stance::Brawl, Stance::Skirmish, Stance::Kite];

    pub fn name(self) -> &'static str {
        match self {
            Stance::Brawl => "brawl",
            Stance::Skirmish => "skirmish",
            Stance::Kite => "kite",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Dps,
    Logi,
    Tackle,
    Ewar,
    Command,
    /// BP-T2-HULLS: an interdictor or heavy interdictor, which holds a bubble.
    Interdict,
    /// BP-T2-HULLS: a cloaked eye.
    Scout,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Dps => "dps",
            Role::Logi => "logi",
            Role::Tackle => "tackle",
            Role::Ewar => "ewar",
            Role::Command => "command",
            Role::Interdict => "interdict",
            Role::Scout => "scout",
        }
    }
}

// ---- the numbers every rule reads -----------------------------------------

/// A turret's application numbers (`combat::Application`'s four).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Gun {
    pub optimal: f64,
    pub falloff: f64,
    pub tracking: f64,
    pub sig_res: f64,
}

impl Gun {
    pub fn application(&self) -> eve_server::combat::Application {
        eve_server::combat::Application {
            optimal: self.optimal,
            falloff: self.falloff,
            tracking: self.tracking,
            sig_resolution: self.sig_res,
        }
    }

    /// Optimal plus half the falloff.
    pub fn reach(&self) -> f64 {
        self.optimal + 0.5 * self.falloff
    }
}

/// A missile's flight: `velocity * fuse` metres, then it fizzles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub reach: f64,
    pub aoe: Aoe,
}

/// How the fit's damage reaches a target.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Delivery {
    #[default]
    None,
    Turret(Gun),
    Missile(Flight),
}

/// What a fit's ewar does, summed over its modules (best of each kind).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ewar {
    /// The fraction of speed a web takes off (`0.6` = target at 40 %).
    pub web_slow: f64,
    /// The shortest range among the fit's webs and scrams: where it can pin.
    pub pin_range: f64,
    pub scram: bool,
    /// The signature added by a painter (`0.5` = +50 %).
    pub paint: f64,
    pub paint_range: f64,
    /// The fraction of the victim's lock range a damp takes.
    pub damp: f64,
    pub damp_range: f64,
    /// Jam strength by sensor type: radar, ladar, magnetometric, gravimetric.
    pub ecm: [f64; 4],
    pub ecm_range: f64,
    pub ecm_falloff: f64,
    /// Capacitor drained per second from one target.
    pub neut: f64,
    pub neut_range: f64,
    /// A tracking disruptor: the fractions it takes off a turret's tracking
    /// speed and off its optimal and falloff, and where it works.
    pub track_cut: f64,
    pub opt_cut: f64,
    pub track_range: f64,
    pub track_falloff: f64,
    /// A guidance disruptor: the fraction it takes off a missile's reach and
    /// explosion velocity (and adds to its explosion radius).
    pub guide_cut: f64,
    pub guide_range: f64,
    pub guide_falloff: f64,
    /// Smartbomb volley per second by damage type (em, explosive, kinetic,
    /// thermal), the module's own numbers; it lands on every ship in range.
    pub bomb: [f64; 4],
    pub bomb_range: f64,
    /// A warp disruptor's reach (a scrambler's is [`Ewar::pin_range`] too).
    pub point_range: f64,
}

/// **The numbers every rule reads**, computed once per fit from the dogma
/// sheet at the doctrine's skills (R-DOCTRINE 2.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    /// Shield, armour and hull points with all twelve resonances: the
    /// engine's own [`damage::Profile`], so the paper EHP is the engine's.
    pub defence: damage::Profile,
    /// Damage per second by type (em, explosive, kinetic, thermal), pre-
    /// resistance and pre-application.
    pub dps: [f64; 4],
    pub delivery: Delivery,
    pub speed: f64,
    pub sig: f64,
    pub agility: f64,
    /// Local repair, hit points per second: `[shield, armour]`.
    pub rep_local: [f64; 2],
    /// Remote repair this fit gives, hit points per second, and its range.
    pub remote_rep: f64,
    pub remote_layer: Option<Layer>,
    pub remote_range: f64,
    /// Capacitor lasts the standard fight, CPU and power fit.
    pub cap_ok: bool,
    pub cpu_ok: bool,
    pub pg_ok: bool,
    /// Peak capacitor recharge, per second.
    pub cap_regen: f64,
    pub ewar: Ewar,
    /// Sensor strengths: radar, ladar, magnetometric, gravimetric.
    pub sensor: [f64; 4],
    pub lock_range: f64,
    /// The fit carries a micro jump drive: a scrambler alone holds it.
    pub mjd: bool,
}

impl Profile {
    /// A ship that does and takes nothing; tests fill what they read.
    pub fn blank(defence: damage::Profile) -> Profile {
        Profile {
            defence,
            dps: [0.0; 4],
            delivery: Delivery::None,
            speed: 0.0,
            sig: 100.0,
            agility: 0.0,
            rep_local: [0.0; 2],
            remote_rep: 0.0,
            remote_layer: None,
            remote_range: 0.0,
            cap_ok: true,
            cpu_ok: true,
            pg_ok: true,
            cap_regen: 0.0,
            ewar: Ewar::default(),
            sensor: [0.0; 4],
            lock_range: 0.0,
            mjd: false,
        }
    }

    /// Effective hit points against pure damage of each type
    /// (em, explosive, kinetic, thermal): the points of a pure volley that
    /// kill the ship, passing shield, armour and hull in turn, exactly as
    /// `ShipDamage::apply` takes them.
    pub fn ehp(&self) -> [f64; 4] {
        let d = &self.defence;
        let by = |cap: f64, r: &Damage| {
            [
                cap / r.em.max(RES_FLOOR),
                cap / r.explosive.max(RES_FLOOR),
                cap / r.kinetic.max(RES_FLOOR),
                cap / r.thermal.max(RES_FLOOR),
            ]
        };
        let s = by(d.shield_capacity, &d.shield_resonance);
        let a = by(d.armor_hp, &d.armor_resonance);
        let h = by(d.hp, &d.hull_resonance);
        [0, 1, 2, 3].map(|i| s[i] + a[i] + h[i])
    }

    /// Points a repair of `layer` is worth against each damage type.
    pub fn rep_ehp(&self, shield_hp: f64, armour_hp: f64) -> [f64; 4] {
        let d = &self.defence;
        let r = |hp: f64, res: &Damage| {
            [
                hp / res.em.max(RES_FLOOR),
                hp / res.explosive.max(RES_FLOOR),
                hp / res.kinetic.max(RES_FLOOR),
                hp / res.thermal.max(RES_FLOOR),
            ]
        };
        let s = r(shield_hp, &d.shield_resonance);
        let a = r(armour_hp, &d.armor_resonance);
        [0, 1, 2, 3].map(|i| s[i] + a[i])
    }

    pub fn ehp_mean(&self) -> f64 {
        self.ehp().iter().sum::<f64>() / 4.0
    }

    pub fn total_dps(&self) -> f64 {
        self.dps.iter().sum()
    }

    /// Local hit points per second repaired.
    pub fn rep_rate(&self) -> f64 {
        self.rep_local[0] + self.rep_local[1]
    }

    /// How far the damage reaches, metres. Zero with no weapon.
    pub fn reach(&self) -> f64 {
        match self.delivery {
            Delivery::None => 0.0,
            Delivery::Turret(g) => g.reach(),
            Delivery::Missile(f) => f.reach,
        }
    }

    /// A fit that passes the three checks.
    pub fn ok(&self) -> bool {
        self.cpu_ok && self.pg_ok && self.cap_ok
    }
}

/// One paper fit: a hull, its modules and rounds, the skills the pilot
/// needs, the numbers, and the price.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleFit {
    pub hull: i64,
    /// `(module type, count)`, highs/mids/lows implied by each module.
    pub modules: Vec<(i64, u8)>,
    /// `(round type, rounds)`: the primary and one alternate damage type.
    pub charges: Vec<(i64, u32)>,
    /// `(skill type, level)` ascending: the hull's and parts' requirements
    /// and the support skills that move their numbers.
    pub skills: Vec<(i64, i64)>,
    pub profile: Profile,
    /// P_ref of hull, modules and rounds.
    pub cost: f64,
}

/// `{min, target, max_on_grid}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetSize {
    pub min: usize,
    pub target: usize,
    pub max_on_grid: usize,
}

/// The fleet a class fields **[A]**: a frigate gang is big, a battleship
/// fleet small. Falsified by soak fleets that never reach `min`.
pub fn fleet_size(class: Class) -> FleetSize {
    let target = match class {
        Class::Frigate => 20,
        Class::Destroyer => 18,
        Class::Cruiser => 15,
        Class::Battlecruiser => 12,
        Class::Battleship => 10,
    };
    FleetSize {
        min: target / 3,
        target,
        max_on_grid: GRID_CAP,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Doctrine {
    /// `"armour/short/brawl/battlecruiser/projectile"`: derived, stable.
    pub key: String,
    pub class: Class,
    pub family: Family,
    pub layer: Layer,
    /// Reps fitted (true) or buffer only.
    pub active: bool,
    pub range: Range,
    pub stance: Stance,
    /// Share of the fleet, one paper fit per role; shares sum to 1.
    pub roles: Vec<(Role, f64, RoleFit)>,
    pub fleet: FleetSize,
}

impl Doctrine {
    /// The damage dealers' fit (the first Dps role).
    pub fn line(&self) -> Option<&RoleFit> {
        self.roles
            .iter()
            .find(|(r, _, _)| *r == Role::Dps)
            .map(|(_, _, f)| f)
    }

    /// Share-weighted price of one ship of the set.
    pub fn cost_per_ship(&self) -> f64 {
        self.roles.iter().map(|(_, s, f)| s * f.cost).sum()
    }

    /// Share of the fleet in a role.
    pub fn share(&self, role: Role) -> f64 {
        self.roles
            .iter()
            .filter(|(r, _, _)| *r == role)
            .map(|(_, s, _)| s)
            .sum()
    }

    /// Every type id a pilot of the doctrine must have or buy.
    pub fn parts(&self) -> BTreeSet<i64> {
        let mut out = BTreeSet::new();
        for (_, _, f) in &self.roles {
            out.insert(f.hull);
            out.extend(f.modules.iter().map(|m| m.0));
            out.extend(f.charges.iter().map(|c| c.0));
        }
        out
    }
}

/// The key of a doctrine.
pub fn key_of(layer: Layer, range: Range, stance: Stance, class: Class, family: Family) -> String {
    format!(
        "{}/{}/{}/{}/{}",
        layer.name(),
        range.name(),
        stance.name(),
        class_name(class),
        family.name()
    )
}

pub fn class_name(c: Class) -> &'static str {
    match c {
        Class::Frigate => "frigate",
        Class::Destroyer => "destroyer",
        Class::Cruiser => "cruiser",
        Class::Battlecruiser => "battlecruiser",
        Class::Battleship => "battleship",
    }
}

// ---- labels ---------------------------------------------------------------

/// The layer holding most of the fitted EHP (mean over the damage types).
pub fn layer_of(d: &damage::Profile) -> Layer {
    let mean = |cap: f64, r: &Damage| {
        cap * (1.0 / r.em.max(RES_FLOOR)
            + 1.0 / r.explosive.max(RES_FLOOR)
            + 1.0 / r.kinetic.max(RES_FLOOR)
            + 1.0 / r.thermal.max(RES_FLOOR))
            / 4.0
    };
    let layers = [
        (Layer::Shield, mean(d.shield_capacity, &d.shield_resonance)),
        (Layer::Armour, mean(d.armor_hp, &d.armor_resonance)),
        (Layer::Hull, mean(d.hp, &d.hull_resonance)),
    ];
    layers
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(Layer::Hull, |l| l.0)
}

pub fn range_of(reach_m: f64) -> Range {
    if reach_m < RANGE_SHORT_M {
        Range::Short
    } else if reach_m > RANGE_LONG_M {
        Range::Long
    } else {
        Range::Medium
    }
}

/// Kite when fast and shooting from afar; Brawl when close with tackle;
/// else Skirmish.
pub fn stance_of(speed: f64, median_speed: f64, range: Range, tackle_share: f64) -> Stance {
    if speed >= KITE_SPEED * median_speed && range != Range::Short {
        Stance::Kite
    } else if range == Range::Short && tackle_share >= BRAWL_TACKLE {
        Stance::Brawl
    } else {
        Stance::Skirmish
    }
}

// ---- what may be used -----------------------------------------------------

/// **Nothing from nowhere**: the part types that exist in the world, as
/// NPC-sold, builder-made or dropped. DOC-7 computes it from recipes and
/// sellers; DOC-1 takes it as given.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Makeable {
    all: bool,
    types: BTreeSet<i64>,
}

impl Makeable {
    /// Every type in the table (tests, the open mode).
    pub fn all() -> Makeable {
        Makeable {
            all: true,
            types: BTreeSet::new(),
        }
    }

    pub fn of(types: impl IntoIterator<Item = i64>) -> Makeable {
        Makeable {
            all: false,
            types: types.into_iter().collect(),
        }
    }

    pub fn has(&self, type_id: i64) -> bool {
        self.all || self.types.contains(&type_id)
    }

    /// DOC-7: the parts the world can supply -- an NPC station sells them,
    /// a builder makes them from a recipe whose original is on sale or is
    /// invented and whose inputs have a source, or they drop
    /// ([`crate::doctrine_parts::makeable`]).
    pub fn from_sources(src: &crate::doctrine_parts::Sources<'_>) -> Makeable {
        crate::doctrine_parts::makeable(src)
    }
}

/// Everything [`catalog`] reads.
pub struct Source<'a> {
    pub installed: &'a Installed,
    pub makeable: &'a Makeable,
    /// P_ref of a type (`econ_prices::p_ref`); `0` when unknown.
    pub price: &'a dyn Fn(i64) -> f64,
    pub limits: Limits,
    /// What players fly (`out/bots/fits.tsv`); `None` seeds nothing.
    pub real_fits: Option<&'a crate::real_fits::RealFits>,
}

/// **The catalog** (R-DOCTRINE 2.4): for every combat hull, every family it
/// can mount, both tank layers and the three utility templates, a greedy
/// fit; the fits that pass CPU, power and capacitor are labelled and grouped
/// by `(class, family, layer, range, stance)` into role sets. One doctrine
/// per key; stable order (by key).
pub fn catalog(src: &Source<'_>) -> Vec<Doctrine> {
    catalog_seeded(src).0
}

/// [`catalog`] and the real fits it was seeded with (for a bot's own pick,
/// [`crate::doctrine_assign`]).
pub fn catalog_seeded(src: &Source<'_>) -> (Vec<Doctrine>, crate::doctrine_real::RealSeeds) {
    let mut lib = Library::build(src.installed, src.makeable, src.price, src.limits);
    lib.real_fits = src.real_fits;
    let seeds = crate::doctrine_real::seeds(&lib);
    (crate::doctrine_fit::catalog_with(&lib, &seeds), seeds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(shield: f64, armour: f64, hull: f64, rs: f64, ra: f64, rh: f64) -> damage::Profile {
        let l = |r: f64| Damage {
            em: r,
            explosive: r,
            kinetic: r,
            thermal: r,
        };
        damage::Profile {
            shield_capacity: shield,
            shield_recharge_rate: 0.0,
            armor_hp: armour,
            hp: hull,
            shield_resonance: l(rs),
            armor_resonance: l(ra),
            hull_resonance: l(rh),
        }
    }

    #[test]
    fn labels_follow_the_numbers() {
        assert_eq!(
            layer_of(&flat(900.0, 400.0, 300.0, 1.0, 1.0, 1.0)),
            Layer::Shield
        );
        assert_eq!(
            layer_of(&flat(300.0, 400.0, 300.0, 1.0, 0.5, 1.0)),
            Layer::Armour
        );
        assert_eq!(range_of(RANGE_SHORT_M - 1.0), Range::Short);
        assert_eq!(range_of(RANGE_LONG_M + 1.0), Range::Long);
        assert_eq!(
            range_of((RANGE_SHORT_M + RANGE_LONG_M) / 2.0),
            Range::Medium
        );
        // Fast and far is a kite; close with tackle is a brawl; else skirmish.
        assert_eq!(stance_of(400.0, 300.0, Range::Long, 0.0), Stance::Kite);
        assert_eq!(
            stance_of(300.0, 300.0, Range::Short, BRAWL_TACKLE),
            Stance::Brawl
        );
        assert_eq!(stance_of(300.0, 300.0, Range::Short, 0.0), Stance::Skirmish);
        // Fast but short is no kite.
        assert_eq!(stance_of(900.0, 300.0, Range::Short, 0.0), Stance::Skirmish);
    }

    #[test]
    fn ehp_adds_the_layers_through_their_resonances() {
        let p = Profile::blank(flat(100.0, 200.0, 300.0, 0.5, 0.5, 1.0));
        let e = p.ehp();
        assert!((e[0] - (200.0 + 400.0 + 300.0)).abs() < 1e-9);
        assert_eq!(e[0], e[3]);
    }

    #[test]
    fn makeable_gates_the_parts() {
        assert!(Makeable::all().has(5));
        let m = Makeable::of([1, 2]);
        assert!(m.has(2) && !m.has(3));
    }

    #[test]
    fn fleets_never_exceed_the_grid() {
        for c in Class::ALL {
            let f = fleet_size(c);
            assert!(f.min <= f.target && f.target <= f.max_on_grid);
        }
    }
}

/// Hand-made profiles and doctrines for the exchange's tests: invented
/// numbers, never the client's.
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;

    pub fn flat_tank(shield: f64, armour: f64, hull: f64) -> damage::Profile {
        let l = Damage {
            em: 1.0,
            explosive: 1.0,
            kinetic: 1.0,
            thermal: 1.0,
        };
        damage::Profile {
            shield_capacity: shield,
            shield_recharge_rate: 0.0,
            armor_hp: armour,
            hp: hull,
            shield_resonance: l,
            armor_resonance: l,
            hull_resonance: l,
        }
    }

    fn gun(optimal: f64, falloff: f64, tracking: f64) -> Delivery {
        Delivery::Turret(Gun {
            optimal,
            falloff,
            tracking,
            sig_res: 400.0,
        })
    }

    /// A cruiser that stands and fires kinetic at short range.
    pub fn brawler(tank: damage::Profile) -> Profile {
        let mut p = Profile::blank(tank);
        p.dps = [0.0, 0.0, 300.0, 0.0];
        p.delivery = gun(8_000.0, 8_000.0, 0.5);
        p.speed = 200.0;
        p.sig = 400.0;
        p.lock_range = 50_000.0;
        p.sensor = [12.0, 0.0, 0.0, 0.0];
        p.cap_regen = 50.0;
        p
    }

    /// Slow, big, short guns, a lot of armour.
    pub fn brick() -> Profile {
        let mut p = brawler(flat_tank(6_000.0, 6_000.0, 3_000.0));
        p.dps = [0.0, 0.0, 500.0, 0.0];
        p
    }

    /// Fast, long guns, a thin tank.
    pub fn kiter() -> Profile {
        let mut p = Profile::blank(flat_tank(1_500.0, 500.0, 500.0));
        p.dps = [0.0, 0.0, 250.0, 0.0];
        p.delivery = gun(30_000.0, 10_000.0, 0.05);
        p.speed = 400.0;
        p.sig = 150.0;
        p.lock_range = 80_000.0;
        p.sensor = [8.0, 0.0, 0.0, 0.0];
        p
    }

    /// A frigate that outruns the kiter and holds it.
    pub fn pinner() -> Profile {
        let mut p = Profile::blank(flat_tank(400.0, 300.0, 300.0));
        p.dps = [0.0, 0.0, 40.0, 0.0];
        p.delivery = gun(2_000.0, 2_000.0, 1.0);
        p.speed = 600.0;
        p.sig = 40.0;
        p.lock_range = 40_000.0;
        p.ewar.web_slow = 0.6;
        p.ewar.scram = true;
        p.ewar.pin_range = 9_000.0;
        p
    }

    pub fn battleship() -> Profile {
        let mut p = Profile::blank(flat_tank(20_000.0, 20_000.0, 10_000.0));
        p.dps = [0.0, 0.0, 900.0, 0.0];
        p.delivery = gun(20_000.0, 20_000.0, 0.01);
        p.speed = 100.0;
        p.sig = 500.0;
        p.lock_range = 90_000.0;
        p.sensor = [20.0, 0.0, 0.0, 0.0];
        p
    }

    /// A cheap, quick, small-signature frigate.
    pub fn swarm() -> Profile {
        let mut p = Profile::blank(flat_tank(300.0, 300.0, 200.0));
        p.dps = [0.0, 0.0, 60.0, 0.0];
        p.delivery = gun(3_000.0, 2_000.0, 1.0);
        p.speed = 500.0;
        p.sig = 30.0;
        p.lock_range = 40_000.0;
        p
    }

    pub fn logi_ship() -> Profile {
        let mut p = Profile::blank(flat_tank(2_000.0, 2_000.0, 1_000.0));
        p.remote_rep = 200.0;
        p.remote_layer = Some(Layer::Armour);
        p.remote_range = 40_000.0;
        p.speed = 250.0;
        p.sig = 300.0;
        p.lock_range = 60_000.0;
        p.cap_regen = 100.0;
        p
    }

    fn fit(p: Profile, cost: f64) -> RoleFit {
        RoleFit {
            hull: 1,
            modules: Vec::new(),
            charges: Vec::new(),
            skills: Vec::new(),
            profile: p,
            cost,
        }
    }

    /// A one-role doctrine around a profile.
    pub fn doc(p: Profile, cost: f64) -> Doctrine {
        Doctrine {
            key: "test".to_owned(),
            class: Class::Cruiser,
            family: Family::Projectile,
            layer: Layer::Armour,
            active: false,
            range: Range::Medium,
            stance: Stance::Skirmish,
            roles: vec![(Role::Dps, 1.0, fit(p, cost))],
            fleet: fleet_size(Class::Cruiser),
        }
    }

    pub fn brick_doc() -> Doctrine {
        doc(brick(), 100.0)
    }

    /// `d` with a support role taking `share` of the fleet.
    pub fn with_role(mut d: Doctrine, role: Role, share: f64, p: Profile, cost: f64) -> Doctrine {
        for r in d.roles.iter_mut() {
            r.1 *= 1.0 - share;
        }
        d.roles.push((role, share, fit(p, cost)));
        d.key = format!("{}+{}", d.key, role.name());
        d
    }

    /// Six doctrines with the mechanical contrasts; each is named.
    pub fn invented_catalog() -> Vec<Doctrine> {
        let named = |mut d: Doctrine, k: &str| {
            d.key = k.to_owned();
            d
        };
        vec![
            named(doc(kiter(), 100.0), "kite"),
            named(doc(brick(), 100.0), "brick"),
            named(
                with_role(doc(brick(), 100.0), Role::Tackle, 0.2, pinner(), 100.0),
                "brick+tackle",
            ),
            named(doc(battleship(), 800.0), "blob"),
            named(doc(swarm(), 50.0), "swarm"),
            named(
                with_role(doc(brick(), 100.0), Role::Logi, 0.2, logi_ship(), 100.0),
                "brick+logi",
            ),
        ]
    }

    // ---- an invented world in typedata's own format ------------------------

    /// `(name, id)` of the invented skill that guns require.
    pub const GUNNERY: i64 = 970_000_001;
    /// Its damage specialisation (moves every gun that requires `GUNNERY`).
    pub const SPECIALISATION: i64 = 970_000_002;
    pub const HULL_SKILL: i64 = 970_000_003;
    /// A hull with too little CPU for anything.
    pub const CRAMPED: i64 = 970_000_100;
    pub const FRIGATE: i64 = 970_000_101;
    pub const FRIGATE_MISSILE: i64 = 970_000_102;
    pub const CRUISER: i64 = 970_000_103;
    pub const CRUISER_MISSILE: i64 = 970_000_104;
    pub const BATTLESHIP: i64 = 970_000_105;
    /// A plate far above `META_MAX`.
    pub const NAMED_PLATE: i64 = 970_000_900;
    /// The small projectile gun, and its two rounds.
    pub const SMALL_PROJECTILE: i64 = 970_001_001;

    /// The world as `typedata.tsv` text. **Invented rows**: every number
    /// round, every id above 970,000,000; the shapes of the effects and
    /// modifiers are the engine's own (`dogma.rs`' fixture is the model).
    pub fn world_text() -> String {
        let mut t = String::new();
        let mut line = |s: String| {
            t.push_str(&s);
            t.push('\n');
        };
        for (a, d, n) in [
            (4, 0.0, "mass"),
            (20, 0.0, "speedFactor"),
            (64, 1.0, "damageMultiplier"),
            (109, 1.0, "kineticDamageResonance"),
            (110, 1.0, "thermalDamageResonance"),
            (111, 1.0, "explosiveDamageResonance"),
            (113, 1.0, "emDamageResonance"),
            (212, 1.0, "missileDamageMultiplier"),
            (267, 1.0, "armorEmDamageResonance"),
            (268, 1.0, "armorExplosiveDamageResonance"),
            (269, 1.0, "armorKineticDamageResonance"),
            (270, 1.0, "armorThermalDamageResonance"),
            (271, 1.0, "shieldEmDamageResonance"),
            (272, 1.0, "shieldExplosiveDamageResonance"),
            (273, 1.0, "shieldKineticDamageResonance"),
            (274, 1.0, "shieldThermalDamageResonance"),
            (280, 0.0, "skillLevel"),
            (292, 0.0, "damageMultiplierBonus"),
            (552, 100.0, "signatureRadius"),
            (567, 0.0, "speedBoostFactor"),
            (796, 0.0, "massAddition"),
            (2033, 1_000_000.0, "speedLimit"),
        ] {
            line(format!("attribute\t{a}\t{d}\t{n}"));
        }
        line("attrmeta\t37\t0\t1\t2033\t0\t4".to_owned());
        for (id, name) in [
            (GUNNERY, "Gunnery"),
            (SPECIALISATION, "Specialisation"),
            (HULL_SKILL, "Hulls"),
        ] {
            let attrs = if id == SPECIALISATION { "292=5.0" } else { "" };
            let fx = if id == SPECIALISATION {
                "9100,9101"
            } else {
                "9100"
            };
            line(format!(
                "type\t{id}\t255\t16\t1\t1\t0.01\t0.0\t0.0\t1.0\t0\t{fx}\t{attrs}\t{name}"
            ));
        }
        // effects the engine and the classifier read
        for e in [
            "effect\t11\t0\t0\t0\t0\t0\t0\t0\tloPower\t",
            "effect\t12\t0\t0\t0\t0\t0\t0\t0\thiPower\t",
            "effect\t13\t0\t0\t0\t0\t0\t0\t0\tmedPower\t",
            "effect\t16\t1\t0\t0\t0\t0\t0\t0\tonline\t",
            "effect\t40\t0\t0\t0\t0\t0\t0\t0\tlauncherFitted\t",
            "effect\t42\t0\t0\t0\t0\t0\t0\t0\tturretFitted\t",
            "effect\t34\t2\t1\t51\t6\t54\t158\t160\tprojectileFired\teffects.ProjectileFired",
            "effect\t101\t2\t1\t51\t6\t0\t0\t0\tuseMissiles\t",
            "effect\t6731\t1\t0\t73\t6\t0\t0\t0\tmoduleBonusAfterburner\teffects.Afterburner",
            "effect\t9100\t0\t0\t0\t0\t0\t0\t0\tskillEffect\t",
            "effect\t9101\t0\t0\t0\t0\t0\t0\t0\tspecialisation\t",
            "effect\t9300\t4\t0\t0\t0\t0\t0\t0\tshieldExtender\t",
            "effect\t9301\t4\t0\t0\t0\t0\t0\t0\tplate\t",
            "effect\t9302\t4\t0\t0\t0\t0\t0\t0\tdamageMod\t",
            "effect\t9310\t1\t0\t73\t6\t0\t0\t0\tlocalRepair\t",
            "effect\t9311\t1\t0\t73\t6\t0\t0\t0\tshieldHardener\t",
            "effect\t9312\t1\t0\t73\t6\t0\t0\t0\tarmourHardener\t",
            "effect\t9320\t2\t1\t73\t6\t54\t0\t0\tremoteEwar\t",
            "effect\t9321\t2\t0\t73\t6\t54\t0\t0\tremoteRepair\t",
            "modifier\t9100\titemID\tItemModifier\t280\t276\t2\t\t",
            "modifier\t9101\titemID\tItemModifier\t292\t280\t0\t\t",
            "modifier\t9101\tshipID\tLocationRequiredSkillModifier\t64\t292\t6\t970000001\t",
            "modifier\t9300\tshipID\tItemModifier\t263\t72\t2\t\t",
            "modifier\t9301\tshipID\tItemModifier\t265\t1159\t2\t\t",
            "modifier\t9302\tshipID\tLocationRequiredSkillModifier\t64\t64\t4\t970000001\t",
            "modifier\t9311\tshipID\tItemModifier\t271\t984\t6\t\t",
            "modifier\t9311\tshipID\tItemModifier\t272\t985\t6\t\t",
            "modifier\t9311\tshipID\tItemModifier\t273\t986\t6\t\t",
            "modifier\t9311\tshipID\tItemModifier\t274\t987\t6\t\t",
            "modifier\t9312\tshipID\tItemModifier\t267\t984\t6\t\t",
            "modifier\t9312\tshipID\tItemModifier\t268\t985\t6\t\t",
            "modifier\t9312\tshipID\tItemModifier\t269\t986\t6\t\t",
            "modifier\t9312\tshipID\tItemModifier\t270\t987\t6\t\t",
        ] {
            line(e.to_owned());
        }
        // hulls: (id, group, name, hi, med, low, turrets, launchers, cpu, pg,
        //         shield, armour, hull, speed, sig, lock, mass)
        type Hull = (
            i64,
            i64,
            &'static str,
            [f64; 5],
            [f64; 2],
            [f64; 3],
            [f64; 4],
        );
        let hulls: [Hull; 6] = [
            (
                CRAMPED,
                25,
                "Cramped",
                [3.0, 3.0, 3.0, 3.0, 0.0],
                [5.0, 1.0],
                [300.0, 300.0, 300.0],
                [300.0, 40.0, 0.0, 1.0e6],
            ),
            (
                FRIGATE,
                25,
                "Alpha",
                [3.0, 3.0, 3.0, 3.0, 0.0],
                [150.0, 40.0],
                [400.0, 300.0, 250.0],
                [400.0, 35.0, 40000.0, 1.2e6],
            ),
            (
                FRIGATE_MISSILE,
                25,
                "Bravo",
                [3.0, 3.0, 3.0, 0.0, 3.0],
                [150.0, 40.0],
                [300.0, 400.0, 250.0],
                [380.0, 36.0, 40000.0, 1.2e6],
            ),
            (
                CRUISER,
                26,
                "Charlie",
                [5.0, 4.0, 5.0, 4.0, 0.0],
                [400.0, 600.0],
                [1800.0, 1500.0, 1200.0],
                [240.0, 120.0, 60000.0, 1.2e7],
            ),
            (
                CRUISER_MISSILE,
                26,
                "Delta",
                [5.0, 4.0, 5.0, 0.0, 5.0],
                [400.0, 600.0],
                [1500.0, 1800.0, 1200.0],
                [230.0, 125.0, 60000.0, 1.2e7],
            ),
            (
                BATTLESHIP,
                27,
                "Echo",
                [8.0, 6.0, 7.0, 6.0, 0.0],
                [700.0, 12000.0],
                [7000.0, 7000.0, 6000.0],
                [110.0, 400.0, 90000.0, 1.0e8],
            ),
        ];
        for (id, group, name, slots, grid, hp, motion) in hulls {
            let [hi, med, low, turrets, launchers] = slots;
            let [cpu, pg] = grid;
            let [shield, armour, hull] = hp;
            let [speed, sig, lock, mass] = motion;
            line(format!(
                "type\t{id}\t{group}\t6\t1\t1\t1000.0\t100.0\t{mass}\t30.0\t0\t\
                 \t9={hull};11={pg};12={low};13={med};14={hi};37={speed};48={cpu};55=150000.0;70=3.0;\
                 76={lock};101={launchers};102={turrets};182={HULL_SKILL};192=5.0;208=14.0;209=0.0;\
                 210=0.0;211=0.0;263={shield};265={armour};277=1.0;479=600000.0;482=500.0;\
                 552={sig};564=500.0;1137=3.0\t{name}"
            ));
        }
        // guns: family -> (charge group, optimal, falloff, tracking, name)
        let fams: [(&str, i64, f64, f64, f64); 3] = [
            ("Autocannon", 83, 4000.0, 4000.0, 0.10),
            ("Railgun", 85, 18000.0, 6000.0, 0.06),
            ("Beam", 86, 30000.0, 12000.0, 0.04),
        ];
        for (fi, (name, group, optimal, falloff, tracking)) in fams.iter().enumerate() {
            for size in 1..=3_i64 {
                let k = size as f64;
                let gid = 970_001_000 + fi as i64 * 10 + size;
                line(format!(
                    "type\t{gid}\t55\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t34\t12,16,34,42\t\
                     6=0.0;30={};50={};51=3000.0;54={};64={};128={size};158={};160={};182={GUNNERY};\
                     277=1.0;604={group};620=400.0;633=0.0\t{name} {size}",
                    4.0 * k * k,
                    5.0 * k,
                    optimal * (1.0 + 0.3 * (k - 1.0)),
                    2.0 * k,
                    falloff,
                    tracking / k
                ));
                // two rounds of different damage types
                for (ri, tag) in ["A", "B"].into_iter().enumerate() {
                    let rid = 970_002_000 + fi as i64 * 100 + size * 10 + ri as i64;
                    let dmg = if ri == 0 {
                        format!("116={};117={}", 3.0 * k, 5.0 * k)
                    } else {
                        format!("114={}", 7.0 * k)
                    };
                    line(format!(
                        "type\t{rid}\t{group}\t8\t1\t100\t0.0025\t0.0\t0.01\t1.0\t0\t\t{dmg};128={size};633=0.0\t{name} Round {tag} {size}"
                    ));
                }
            }
        }
        // launchers (101 and 40) take a missile group per size
        for size in 1..=3_i64 {
            let k = size as f64;
            let group = 383 + size;
            line(format!(
                "type\t{}\t507\t7\t1\t1\t5.0\t0.4\t500.0\t25.0\t101\t12,16,40,101\t\
                 6=0.0;30={};50={};51=6000.0;182={GUNNERY};277=1.0;604={group};633=0.0\tLauncher {size}",
                970_001_100 + size,
                6.0 * k * k,
                6.0 * k
            ));
            for (ri, dmg) in [("A", "116=40.0;117=60.0"), ("B", "114=90.0")]
                .into_iter()
                .enumerate()
            {
                line(format!(
                    "type\t{}\t{group}\t8\t1\t100\t0.01\t0.0\t0.01\t1.0\t0\t\t{};37=3000.0;281=6000.0;653=100.0;654=200.0;1353=0.5;633=0.0\tMissile {} {size}",
                    970_002_500 + size * 10 + ri as i64,
                    dmg.1,
                    dmg.0
                ));
            }
        }
        // tank and utility modules: (id, rack effect, name, attrs, extra effects, default)
        let mut next = 970_003_000_i64;
        let mut module = |rack: i64, name: &str, attrs: String, extra: &str, default: i64| {
            next += 1;
            line(format!(
                "type\t{next}\t60\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t{default}\t{rack},16{extra}\t{attrs};633=0.0\t{name}"
            ));
            next
        };
        for (n, (cap, cpu, pg)) in [
            (150.0, 6.0, 1.0),
            (500.0, 20.0, 25.0),
            (1500.0, 40.0, 400.0),
        ]
        .into_iter()
        .enumerate()
        {
            module(
                13,
                &format!("Extender {n}"),
                format!("30={pg};50={cpu};72={cap}"),
                ",9300",
                0,
            );
        }
        for (n, (hp, cpu, pg)) in [
            (150.0, 8.0, 3.0),
            (500.0, 20.0, 60.0),
            (1600.0, 40.0, 500.0),
        ]
        .into_iter()
        .enumerate()
        {
            module(
                11,
                &format!("Plate {n}"),
                format!("30={pg};50={cpu};1159={hp}"),
                ",9301",
                0,
            );
        }
        for (n, (amt, cpu, pg)) in [(60.0, 10.0, 2.0), (200.0, 25.0, 70.0), (600.0, 40.0, 450.0)]
            .into_iter()
            .enumerate()
        {
            module(
                11,
                &format!("Repairer {n}"),
                format!("6=20.0;30={pg};50={cpu};73=5000.0;84={amt}"),
                ",9310",
                9310,
            );
            module(
                13,
                &format!("Booster {n}"),
                format!("6=20.0;30={pg};50={cpu};73=5000.0;68={amt}"),
                ",9310",
                9310,
            );
            module(
                12,
                &format!("Remote armour {n}"),
                format!("6=20.0;30={pg};50={cpu};54=30000.0;73=5000.0;84={amt}"),
                ",9321",
                9321,
            );
            module(
                12,
                &format!("Remote shield {n}"),
                format!("6=20.0;30={pg};50={cpu};54=30000.0;73=5000.0;68={amt}"),
                ",9321",
                9321,
            );
        }
        module(
            13,
            "Shield hardener",
            "6=10.0;30=5.0;50=20.0;73=10000.0;984=-30.0;985=-30.0;986=-30.0;987=-30.0".to_owned(),
            ",9311",
            9311,
        );
        module(
            11,
            "Armour hardener",
            "6=10.0;30=5.0;50=20.0;73=10000.0;984=-30.0;985=-30.0;986=-30.0;987=-30.0".to_owned(),
            ",9312",
            9312,
        );
        module(11, "Gyro", "30=1.0;50=10.0;64=1.1".to_owned(), ",9302", 0);
        module(
            13,
            "Afterburner",
            "6=5.0;30=2.0;50=15.0;73=5000.0;20=100.0;567=1000000.0;796=500000.0".to_owned(),
            ",6731",
            6731,
        );
        module(
            13,
            "Web",
            "6=5.0;30=2.0;50=15.0;73=5000.0;20=-60.0;54=10000.0".to_owned(),
            ",9320",
            9320,
        );
        module(
            13,
            "Scrambler",
            "6=5.0;30=2.0;50=15.0;73=5000.0;105=2.0;54=9000.0".to_owned(),
            ",9320",
            9320,
        );
        module(
            13,
            "Painter",
            "6=5.0;30=2.0;50=15.0;73=5000.0;554=30.0;54=30000.0".to_owned(),
            ",9320",
            9320,
        );
        module(
            13,
            "Dampener",
            "6=5.0;30=2.0;50=15.0;73=5000.0;309=-30.0;566=-30.0;54=40000.0".to_owned(),
            ",9320",
            9320,
        );
        module(
            13,
            "Jammer",
            "6=10.0;30=2.0;50=20.0;73=5000.0;241=3.0;239=3.0;240=3.0;238=3.0;54=30000.0".to_owned(),
            ",9320",
            9320,
        );
        module(
            12,
            "Neutraliser",
            "6=10.0;30=5.0;50=20.0;73=5000.0;97=50.0;54=10000.0".to_owned(),
            ",9320",
            9320,
        );
        // a plate far above the meta cap
        line(format!(
            "type\t{NAMED_PLATE}\t329\t7\t1\t1\t5.0\t0.0\t500.0\t25.0\t0\t11,16,9301\t30=1.0;50=1.0;1159=99999.0;633=5.0\tNamed plate"
        ));
        t
    }

    pub fn world() -> eve_server::typedata::Installed {
        let table =
            eve_server::typedata::Table::parse(&world_text()).expect("the invented world parses");
        eve_server::typedata::Installed::build(table)
    }
}
