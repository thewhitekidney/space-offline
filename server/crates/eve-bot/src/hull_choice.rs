//! **The hull chooser** (ship-pool): what a combat bot flies in the closed
//! economy and in society play, from what *exists*.
//!
//! The owner's rule: escort bots fly many different ships, by what they have
//! available, and those ships are made by industrialists; nothing appears out
//! of nowhere. So there is no fixed combat hull. A ratter, an escort, a site
//! runner, a mission runner or a hunter docked without a combat ship looks at:
//!
//! 1. **its hangar** (free),
//! 2. **asks at its station** it can afford ([`SPEND_SHARE`] of the wallet),
//! 3. **asks within reach** ([`crate::travel::find`], only when no station
//!    sells a hull at all),
//!
//! scores each candidate hull ([`rank`], pure) by role fit (size class, the
//! weapon its skills use, a drone bay when it fields drones, tank, hardpoints
//! by risk), cost against the wallet and a per-bot taste, and buys the best it
//! can ([`obtain`]). With no hull to be had it rests **one standing bid** for
//! the best one it would buy ([`want_bid`]): that is the demand a builder
//! reads (`builder::promote_demanded`, `signal::product_need`). A pilot still
//! in a capsule then takes the free rookie ship, as before.
//!
//! The GM `/createitem` of a hull stays in the open/dev mode only
//! (`Policy::closed` off), which keeps the old fixed Rifter ([`crate::escort::RIFTER`]).
//!
//! Every number is **[A]** policy. The hull table's type ids are the client's
//! ship types [V-data]; its classes, hardpoints, bays and tank ratings are
//! rounded **[A]** figures for ranking only (falsified by a bot that flies a
//! hull it should not have preferred): the server's own dogma decides what a
//! fit really does. Hull skills are checked by the bot ([`acquire::flyable`]);
//! a hull it cannot board yet is set aside, not banned
//! ([`Need::untrained`]), while a boarding the server refused for another
//! reason is avoided for good ([`crate::drone_use::Mind::refused`]).

use crate::acquire::{self, Lacking, Order};
use crate::pricing::Prices;
use crate::travel::Trips;
use crate::{Conn, Error, ItemRow, Result};

/// A hull's size class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Frigate = 0,
    Destroyer = 1,
    Cruiser = 2,
    /// PROG-PVE: the career ladder's fourth and fifth rungs.
    Battlecruiser = 3,
    Battleship = 4,
}

/// How many size classes the table knows ([`Class`]).
pub const CLASSES: usize = 5;

impl Class {
    /// Every class, smallest first.
    pub const ALL: [Class; CLASSES] = [
        Class::Frigate,
        Class::Destroyer,
        Class::Cruiser,
        Class::Battlecruiser,
        Class::Battleship,
    ];
}

/// The weapon a hull's bonuses favour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weapon {
    Projectile,
    Hybrid,
    Laser,
    Missile,
    /// Bonused to drones.
    Drones,
}

/// The weapon of a bot the open mode runs, and of a closed one before its
/// race is known: small projectile turrets ([`crate::escort::SKILL_GUNNERY`]).
/// **[A]** PROG-PVE2: a closed-economy bot's weapon follows its race
/// ([`crate::progression::weapon_of`]).
pub const BOT_WEAPON: Weapon = Weapon::Projectile;

/// One hull the chooser knows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullInfo {
    pub type_id: i64,
    pub class: Class,
    pub bonus: Weapon,
    /// Turret hardpoints.
    pub turrets: u8,
    /// Drone bay, m3 (0: none).
    pub bay: f64,
    /// A tank rating, 0 to 1.
    pub tank: f64,
}

const fn h(
    type_id: i64,
    class: Class,
    bonus: Weapon,
    turrets: u8,
    bay: f64,
    tank: f64,
) -> HullInfo {
    HullInfo {
        type_id,
        class,
        bonus,
        turrets,
        bay,
        tank,
    }
}

/// PROG-PVE corrected three rows against the type data: the Atron (608, not
/// 614, the Fury), the Moa (623, which rung 5 builds, not the Caracal), and
/// the Breacher and Condor, which have launchers and no turret hardpoint (a
/// projectile bot cannot arm them: [`score`] skips a turretless hull).
///
/// The T1 combat hulls the builder ladder makes (`builder::BLUEPRINT_RUNGS`
/// rungs 2, 3 and 5) **[V-data]** with rounded **[A]** figures, frigates, then
/// destroyers, then cruisers, then (PROG-PVE) the battlecruisers and
/// battleships, which the builders make from [`crate::hulls::warship_rungs`].
pub static HULLS: &[HullInfo] = &[
    h(587, Class::Frigate, Weapon::Projectile, 3, 0.0, 0.2),
    h(585, Class::Frigate, Weapon::Projectile, 3, 0.0, 0.15),
    h(598, Class::Frigate, Weapon::Missile, 0, 0.0, 0.2),
    h(597, Class::Frigate, Weapon::Laser, 3, 0.0, 0.25),
    h(583, Class::Frigate, Weapon::Missile, 0, 0.0, 0.15),
    h(608, Class::Frigate, Weapon::Hybrid, 3, 0.0, 0.2),
    h(594, Class::Frigate, Weapon::Drones, 3, 10.0, 0.25),
    // BP-T2-HULLS: the covert ops frigates (group 830) **[V-data]**, a
    // scout's. [`ROLE_HULLS`]: a doctrine's role hull only.
    h(11_172, Class::Frigate, Weapon::Hybrid, 2, 5.0, 0.15),
    h(11_182, Class::Frigate, Weapon::Projectile, 2, 0.0, 0.1),
    h(11_188, Class::Frigate, Weapon::Laser, 2, 0.0, 0.15),
    h(11_192, Class::Frigate, Weapon::Missile, 1, 0.0, 0.15),
    h(16_242, Class::Destroyer, Weapon::Projectile, 7, 0.0, 0.3),
    h(16_236, Class::Destroyer, Weapon::Laser, 8, 0.0, 0.3),
    h(16_240, Class::Destroyer, Weapon::Hybrid, 8, 0.0, 0.3),
    h(16_238, Class::Destroyer, Weapon::Missile, 7, 0.0, 0.3),
    // BP-T2-HULLS: the interdictors (group 541) **[V-data]**.
    h(22_452, Class::Destroyer, Weapon::Laser, 4, 0.0, 0.3),
    h(22_456, Class::Destroyer, Weapon::Projectile, 7, 0.0, 0.3),
    h(22_460, Class::Destroyer, Weapon::Hybrid, 7, 0.0, 0.3),
    h(22_464, Class::Destroyer, Weapon::Missile, 2, 0.0, 0.3),
    h(629, Class::Cruiser, Weapon::Projectile, 4, 40.0, 0.55),
    h(624, Class::Cruiser, Weapon::Laser, 4, 40.0, 0.6),
    h(627, Class::Cruiser, Weapon::Hybrid, 4, 40.0, 0.55),
    h(623, Class::Cruiser, Weapon::Hybrid, 5, 15.0, 0.55),
    // BP-T2-HULLS: the heavy interdiction cruisers (group 894) **[V-data]**.
    h(11_995, Class::Cruiser, Weapon::Missile, 0, 0.0, 0.6),
    h(12_013, Class::Cruiser, Weapon::Projectile, 5, 0.0, 0.6),
    h(12_017, Class::Cruiser, Weapon::Laser, 5, 50.0, 0.6),
    h(12_021, Class::Cruiser, Weapon::Hybrid, 5, 0.0, 0.6),
    // PROG-PVE: the T1 battlecruisers and battleships **[V-data]** (group
    // 419 and 27 in the client's type data; turrets are the hull's turret
    // hardpoints, attribute 102, bays attribute 283 rounded, tank a rounded
    // **[A]** 0.65 to 0.75 for a battlecruiser, 0.85 to 0.95 for a
    // battleship from shield + armour + structure).
    h(16_233, Class::Battlecruiser, Weapon::Laser, 4, 225.0, 0.7),
    h(16_227, Class::Battlecruiser, Weapon::Hybrid, 6, 25.0, 0.75),
    h(16_229, Class::Battlecruiser, Weapon::Hybrid, 6, 75.0, 0.7),
    h(16_231, Class::Battlecruiser, Weapon::Missile, 2, 50.0, 0.7),
    h(24_696, Class::Battlecruiser, Weapon::Laser, 6, 75.0, 0.7),
    h(24_698, Class::Battlecruiser, Weapon::Missile, 0, 25.0, 0.75),
    h(24_700, Class::Battlecruiser, Weapon::Drones, 5, 200.0, 0.7),
    h(
        24_702,
        Class::Battlecruiser,
        Weapon::Projectile,
        6,
        40.0,
        0.7,
    ),
    h(24_688, Class::Battleship, Weapon::Hybrid, 8, 125.0, 0.92),
    h(24_690, Class::Battleship, Weapon::Hybrid, 6, 175.0, 0.95),
    h(24_692, Class::Battleship, Weapon::Laser, 8, 75.0, 0.95),
    h(
        24_694,
        Class::Battleship,
        Weapon::Projectile,
        6,
        125.0,
        0.92,
    ),
    h(638, Class::Battleship, Weapon::Missile, 4, 75.0, 0.88),
    h(639, Class::Battleship, Weapon::Projectile, 6, 75.0, 0.9),
    h(641, Class::Battleship, Weapon::Hybrid, 7, 75.0, 0.9),
    h(642, Class::Battleship, Weapon::Laser, 8, 75.0, 0.88),
    h(643, Class::Battleship, Weapon::Laser, 5, 375.0, 0.95),
    h(644, Class::Battleship, Weapon::Missile, 6, 125.0, 0.85),
    h(645, Class::Battleship, Weapon::Drones, 6, 375.0, 0.95),
];

/// BP-T2-HULLS: the T2 role hulls of [`HULLS`] (interdictors, heavy
/// interdiction cruisers, covert ops): rows for their class, never a bot's
/// own pick. Only a doctrine's role fit boards one ([`score`]); a ratter or
/// a hunter without a doctrine does not take a bubble ship for a destroyer.
pub const ROLE_HULLS: [i64; 12] = [
    11_172, 11_182, 11_188, 11_192, 22_452, 22_456, 22_460, 22_464, 11_995, 12_013, 12_017, 12_021,
];

/// The hulls of [`HULLS`] a bot climbs through on its own: all but the
/// [`ROLE_HULLS`].
pub fn ladder_hulls() -> impl Iterator<Item = &'static HullInfo> {
    HULLS.iter().filter(|h| !ROLE_HULLS.contains(&h.type_id))
}

/// The base worth of a size class **[A]**: bigger is better, not by much, so
/// price and taste still spread the pool over classes.
pub const CLASS_VALUE: [f64; CLASSES] = [1.0, 1.5, 2.0, 2.5, 3.0];
/// A hull bonused to the bots' weapon **[A]**.
pub const WEAPON_MATCH: f64 = 0.5;
/// A drone bay, for a bot that fields drones **[A]**.
pub const DRONE_BONUS: f64 = 0.6;
/// The most a bot spends on a hull, as a share of its wallet **[A]**: the rest
/// buys guns, rounds and drones.
pub const SPEND_SHARE: f64 = 0.5;
/// The price penalty per unit of wallet spent **[A]**.
pub const COST_WEIGHT: f64 = 1.5;
/// How much a hull's tank counts for a cautious bot (x `1 - risk`) **[A]**.
pub const TANK_WEIGHT: f64 = 0.6;
/// How much a hull's hardpoints count for a bold bot (x `risk`) **[A]**.
pub const GUN_WEIGHT: f64 = 0.5;
/// A trip to fetch the hull **[A]**.
pub const TRIP_PENALTY: f64 = 0.2;
/// The spread of a bot's own taste, peak to peak **[A]**: the same wallet and
/// market give different bots different hulls, deterministically.
pub const TASTE: f64 = 0.8;
/// DOC-5: what the leader's doctrine hull adds to an offer's score **[A]**,
/// larger than [`TASTE`] and [`WEAPON_MATCH`] together so a pilot that must
/// buy a hull buys the corp's, unless the wallet will not cover it (an
/// unaffordable offer is no offer). Falsifier: members still on other hulls
/// a week after a switch with the doctrine hull on sale (too low).
pub const DOC_BONUS: f64 = 3.0;
/// A hangar hull replaces the flown one only when it scores this much more
/// **[A]**.
pub const UPGRADE_MARGIN: f64 = 0.5;
/// Hull types probed one by one for a trip when no station sells any **[A]**.
pub const TRIP_PROBES: usize = 6;
/// How many of the best candidates one visit tries to buy **[A]**.
pub const BUY_TRIES: usize = 3;
/// A standing hull bid rests this many days **[A]**.
pub const BID_DAYS: i64 = 7;

/// Where a candidate hull is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// In the hangar, as this item.
    Hangar(i64),
    /// An ask at this station, ISK.
    Local(f64),
    /// An ask within reach, ISK, a trip away.
    Trip(f64),
}

/// A hull on offer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub type_id: i64,
    pub source: Source,
}

/// What the bot needs of a hull.
#[derive(Debug, Clone, Copy)]
pub struct Need<'a> {
    pub weapon: Weapon,
    /// The bot fields combat drones.
    pub drones: bool,
    pub wallet: f64,
    /// The risk trait, 0 to 1.
    pub risk: f64,
    /// Per-class lean of the role, added to [`CLASS_VALUE`].
    pub bias: [f64; CLASSES],
    /// The bot's own seed ([`TASTE`]).
    pub taste: u64,
    /// Hull types refused at boarding.
    pub avoid: &'a [i64],
    /// SKILLGATE: hull types the pilot's skills do not board yet; skipped
    /// like [`Need::avoid`] but cleared when the skill plan runs again.
    pub untrained: &'a [i64],
    /// PROG-PVE2: an upgrade -- the hull must carry the class's guns of the
    /// weapon ([`score`]).
    pub strict: bool,
    /// DOC-5: the fit the leader's doctrine gives this bot; its hull gets
    /// [`DOC_BONUS`] and is not held to the hardpoint rules of the bot's own
    /// weapon.
    pub doctrine: Option<&'a crate::doctrine::RoleFit>,
}

impl HullInfo {
    /// DOC-5: a doctrine hull the table has no row for, listed by its
    /// catalog class when [`HULLS`] knows it (else a cruiser). Its other
    /// figures are placeholders: [`score`] reads only the class, price and
    /// bonus of a doctrine hull.
    pub fn doctrine(type_id: i64) -> HullInfo {
        HullInfo {
            type_id,
            class: class_of(HULLS, type_id).unwrap_or(Class::Cruiser),
            bonus: BOT_WEAPON,
            turrets: 0,
            bay: 0.0,
            tank: 0.0,
        }
    }
}

/// A scored candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    pub type_id: i64,
    pub source: Source,
    pub score: f64,
}

/// Whether `type_id` is a hull of `table`.
pub fn in_table(table: &[HullInfo], type_id: i64) -> bool {
    table.iter().any(|t| t.type_id == type_id)
}

/// Whether `type_id` is one of the combat hulls ([`HULLS`], or the open
/// mode's fixed ones).
pub fn is_combat_hull(type_id: i64) -> bool {
    in_table(HULLS, type_id)
        || type_id == crate::escort::RIFTER
        || type_id == crate::drone_use::DRONE_HULL
}

/// The size class of `type_id` in `table`.
pub fn class_of(table: &[HullInfo], type_id: i64) -> Option<Class> {
    table.iter().find(|t| t.type_id == type_id).map(|t| t.class)
}

/// The per-bot taste for a type, in `[-TASTE/2, TASTE/2]`.
fn taste_of(seed: u64, type_id: i64) -> f64 {
    let mut x = seed ^ (type_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    ((x % 1000) as f64 / 999.0 - 0.5) * TASTE
}

/// **The score** of one offer (pure), or `None` when the hull is unknown,
/// avoided or not affordable.
pub fn score(table: &[HullInfo], offer: &Offer, need: &Need<'_>) -> Option<f64> {
    let info = table.iter().find(|t| t.type_id == offer.type_id)?;
    if need.avoid.contains(&info.type_id) || need.untrained.contains(&info.type_id) {
        return None;
    }
    let doc = need.doctrine.is_some_and(|d| d.hull == offer.type_id);
    if !doc && ROLE_HULLS.contains(&info.type_id) {
        return None;
    }
    // PROG-PVE: a hull with no turret hardpoint cannot carry a turret
    // bot's guns (the Drake): [V-data] attribute 102 is 0. PROG-PVE2: an
    // upgrade (`Need::strict`) is held to what the class fits -- as many
    // turret hardpoints as guns, or for a missile bot as many launcher
    // hardpoints ([`crate::fit_plan::launchers_of`]; a missile hull with no
    // turret hardpoint at all is a launcher ship). Short of strict the
    // weapon is a preference: a bot with no hull yet, or one that lost its
    // ship, takes what exists, and its race and weapon follow the hull.
    let guns = crate::fit_plan::guns_of_class(info.class);
    match need.weapon {
        _ if doc => {}
        Weapon::Missile => {
            let launchers = crate::fit_plan::launchers_of(info.type_id) as usize;
            if need.strict
                && launchers < guns
                && !(info.bonus == Weapon::Missile && info.turrets == 0)
            {
                return None;
            }
        }
        Weapon::Drones => {}
        _ => {
            if info.turrets == 0 || (need.strict && (info.turrets as usize) < guns) {
                return None;
            }
        }
    }
    let risk = need.risk.clamp(0.0, 1.0);
    let mut s = CLASS_VALUE[info.class as usize] + need.bias[info.class as usize];
    if doc {
        s += DOC_BONUS;
    }
    if info.bonus == need.weapon {
        s += WEAPON_MATCH;
    }
    if need.drones && info.bay >= 10.0 {
        s += DRONE_BONUS;
    }
    s += info.tank * TANK_WEIGHT * (1.0 - risk);
    s += f64::from(info.turrets) / 8.0 * GUN_WEIGHT * risk;
    s += taste_of(need.taste, info.type_id);
    match offer.source {
        Source::Hangar(_) => {}
        Source::Local(price) | Source::Trip(price) => {
            if price <= 0.0 || price.is_nan() || price > need.wallet * SPEND_SHARE {
                return None;
            }
            s -= COST_WEIGHT * price / need.wallet.max(1.0);
            if matches!(offer.source, Source::Trip(_)) {
                s -= TRIP_PENALTY;
            }
        }
    }
    Some(s)
}

/// **The ranking** (pure): the best offer per hull type (a hangar hull over
/// an ask), best score first, ties by type id. Unknown, avoided and
/// unaffordable hulls are left out.
pub fn rank(table: &[HullInfo], offers: &[Offer], need: &Need<'_>) -> Vec<Pick> {
    let mut best: Vec<Pick> = Vec::new();
    for o in offers {
        let Some(score) = score(table, o, need) else {
            continue;
        };
        let pick = Pick {
            type_id: o.type_id,
            source: o.source,
            score,
        };
        match best.iter_mut().find(|p| p.type_id == o.type_id) {
            Some(p) => {
                let held = matches!(p.source, Source::Hangar(_));
                let now_held = matches!(o.source, Source::Hangar(_));
                if (now_held && !held) || (now_held == held && score > p.score) {
                    *p = pick;
                }
            }
            None => best.push(pick),
        }
    }
    best.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.type_id.cmp(&b.type_id)));
    best
}

/// The hulls of `hangar` as offers: assembled ships of the table, other
/// than `active`.
pub fn hangar_offers(table: &[HullInfo], hangar: &[ItemRow], active: Option<i64>) -> Vec<Offer> {
    hangar
        .iter()
        .filter(|r| {
            r.category_id == acquire::CATEGORY_SHIP
                && in_table(table, r.type_id)
                && Some(r.item_id) != active
        })
        .map(|r| Offer {
            type_id: r.type_id,
            source: Source::Hangar(r.item_id),
        })
        .collect()
}

/// The per-class lean of a role **[A]**: the hunter wants speed (frigates
/// and destroyers tackle), the mission runner wants a tank.
pub fn bias_of(role: crate::brain::RoleRef) -> [f64; CLASSES] {
    if std::ptr::eq(role, &crate::hunt::ROLE) {
        [0.5, 0.3, 0.0, -0.2, -0.4]
    } else if std::ptr::eq(role, &crate::missioner::ROLE) {
        [0.0, 0.1, 0.4, 0.5, 0.6]
    } else {
        [0.0; CLASSES]
    }
}

/// What the flown hull `flying` and the hangar's best say: stay in what is
/// flown (`None`) or board this item. A flown combat hull is replaced only
/// by a hangar hull [`UPGRADE_MARGIN`] better.
pub fn board_choice(
    table: &[HullInfo],
    hangar: &[ItemRow],
    active: Option<i64>,
    flying: Option<i64>,
    need: &Need<'_>,
) -> Option<i64> {
    // DOC-5: the doctrine's hull is a row of the table whether or not
    // [`HULLS`] lists it.
    let extended;
    let table = match need.doctrine {
        Some(d) if !in_table(table, d.hull) => {
            extended = [table, &[HullInfo::doctrine(d.hull)]].concat();
            &extended[..]
        }
        _ => table,
    };
    let offers = hangar_offers(table, hangar, active);
    let ranked = rank(table, &offers, need);
    let best = ranked.first()?;
    let Source::Hangar(item) = best.source else {
        return None;
    };
    match flying.filter(|f| in_table(table, *f) && !need.avoid.contains(f)) {
        None => Some(item),
        Some(f) => {
            // DOC-5: an old hull keeps flying until it is lost. The doctrine
            // hull's bonus decides what is bought and boarded when there is
            // no hull to keep; against a flown hull the two are compared
            // as the bot's own taste would, so only a real upgrade swaps.
            let plain = Need {
                doctrine: None,
                ..*need
            };
            let held = Offer {
                type_id: f,
                source: Source::Hangar(0),
            };
            // A role hull (dictor, scout) that is not the doctrine's scores
            // nothing plain: after a doctrine change its pilot is not
            // stranded in it; any other hull in the hangar takes over.
            if ROLE_HULLS.contains(&f) && need.doctrine.is_none_or(|d| d.hull != f) {
                return (best.type_id != f).then_some(item);
            }
            let current = score(table, &held, &plain)?;
            let better = score(
                table,
                &Offer {
                    type_id: best.type_id,
                    source: best.source,
                },
                &plain,
            )
            .unwrap_or(best.score);
            // PROG-PVE: a bigger class in the hangar is always the step up
            // (the class values differ by less than the margin); within the
            // class the margin rule stays.
            let bigger = class_of(table, best.type_id) > class_of(table, f);
            ((bigger || better > current + UPGRADE_MARGIN) && best.type_id != f).then_some(item)
        }
    }
}

// ---- over the wire ---------------------------------------------------------------

/// Whether the hangar holds an assembled or packaged combat hull of the
/// ladder ([`ladder_hulls`]; a spare dictor is no rookie ship) or the
/// doctrine's hull.
pub fn hangar_holds_hull(conn: &mut Conn, doctrine_hull: Option<i64>) -> Result<bool> {
    Ok(conn.hangar_items()?.iter().any(|r| {
        r.category_id == acquire::CATEGORY_SHIP
            && (Some(r.type_id) == doctrine_hull || ladder_class(r.type_id).is_some())
    }))
}

/// The size class of a hull of the ladder ([`ladder_hulls`]); `None` for a
/// role hull or a type the table does not know.
pub fn ladder_class(type_id: i64) -> Option<Class> {
    ladder_hulls()
        .find(|h| h.type_id == type_id)
        .map(|h| h.class)
}

fn soft<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => Err(e),
        Err(_) => Ok(None),
    }
}

/// The offers a docked bot can see: the hangar's, the station's asks it can
/// pay, and, when no station here sells any hull, one probe per hull type
/// within reach ([`TRIP_PROBES`]).
pub fn offers_in(
    conn: &mut Conn,
    table: &[HullInfo],
    prices: &mut Prices,
    trips: Option<Trips<'_>>,
    wallet: f64,
) -> Result<Vec<Offer>> {
    let station = conn
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("hulls: not docked".to_owned()))?;
    let mut offers = Vec::new();
    let mut local = false;
    for info in table {
        let cap = soft(prices.reservation(conn, info.type_id))?.flatten();
        let Some(book) = soft(conn.market_orders(info.type_id))? else {
            continue;
        };
        let asks = acquire::plan(&book, station, cap, wallet * SPEND_SHARE, 1);
        if let Some((ask, _)) = asks.first() {
            local = true;
            offers.push(Offer {
                type_id: info.type_id,
                source: Source::Local(ask.price),
            });
        }
    }
    if !local && let Some(trips) = trips {
        for info in table.iter().take(TRIP_PROBES) {
            let cap = soft(prices.reservation(conn, info.type_id))?.flatten();
            if let Some(Some(src)) = soft(crate::travel::find(conn, trips, info.type_id, 1, cap))?
                && src.price <= wallet * SPEND_SHARE
            {
                offers.push(Offer {
                    type_id: info.type_id,
                    source: Source::Trip(src.price),
                });
            }
        }
    }
    Ok(offers)
}

/// **A standing bid** for `units` of `type_id` at the reservation price,
/// through [`crate::demand::post`] (FE-DEMAND): the pilot's resting bids for
/// the type are topped up to `units`, repriced toward the cap, and the whole
/// is cut to [`SPEND_SHARE`] of the wallet. The demand a builder reads.
/// `false` when no new order was placed.
pub fn want_bid(
    conn: &mut Conn,
    prices: &mut Prices,
    type_id: i64,
    units: i64,
    wallet: f64,
) -> Result<bool> {
    Ok(crate::demand::post(conn, prices, type_id, units, wallet)?.placed > 0)
}

/// [`want_bid`] with the prices read here.
pub fn bid_for(conn: &mut Conn, type_id: i64, units: i64, wallet: f64) -> Result<bool> {
    let mut prices = match soft(Prices::read(conn))? {
        Some(p) => p,
        None => return Ok(false),
    };
    want_bid(conn, &mut prices, type_id, units, wallet)
}

/// **A standing bid per missing part** (DOC-2): one bid for each type of
/// `order` the pilot is short of, its units the missing count
/// ([`Order::short`]), at the reservation price ([`want_bid`]: not twice for
/// a type, not above [`SPEND_SHARE`] of the wallet). The demand a builder
/// reads for plates, guns and rounds, as [`obtain_from`]'s bid reads for a
/// hull. The types bid for, in order.
pub fn want_parts(
    conn: &mut Conn,
    prices: &mut Prices,
    order: &Order,
    wallet: f64,
) -> Result<Vec<(i64, i64)>> {
    let mut held = std::collections::BTreeMap::new();
    for &(t, _) in order.modules.iter().chain(&order.charges) {
        held.insert(t, acquire::held(conn, t)?);
    }
    let mut placed = Vec::new();
    for (t, units) in order.short(&|t| held.get(&t).copied().unwrap_or(0)) {
        let posted = crate::demand::post(conn, prices, t, units, wallet)?;
        if posted.placed > 0 {
            placed.push((t, posted.placed));
        }
    }
    Ok(placed)
}

/// **Replace what a loss took** (FE-DEMAND), docked: a standing bid for the
/// hull (when none is held), each module of `order` the pilot lacks and
/// [`crate::demand::FE_AMMO_DAYS`] of rounds ([`crate::demand::replenish`]; a
/// day's fire is the order's own load **[A]**), plus a spare hull when the
/// wallet is [`crate::demand::FE_SPARE_RICH`] hull-values deep. All through
/// [`crate::demand::post`]. The `(type, units placed)` list.
pub fn replenish(
    conn: &mut Conn,
    prices: &mut Prices,
    order: &Order,
    wallet: f64,
) -> Result<Vec<(i64, i64)>> {
    let mut held = std::collections::BTreeMap::new();
    for &(t, _) in order.modules.iter().chain(&order.charges) {
        held.insert(t, acquire::held(conn, t)?);
    }
    let hull_held = match order.hull {
        Some(h) => {
            let n = acquire::held(conn, h)?;
            held.insert(h, n);
            n
        }
        None => 0,
    };
    let per_day = |t: i64| {
        order
            .charges
            .iter()
            .find(|c| c.0 == t)
            .map_or(0.0, |c| c.1 as f64)
    };
    let mut needs =
        crate::demand::replenish(order, &|t| held.get(&t).copied().unwrap_or(0), &per_day);
    if let Some(h) = order.hull
        && hull_held >= 1
        && let Some(v) = soft(prices.value(conn, h))?.flatten()
        && crate::demand::spare_hull_wanted(wallet, v, hull_held - 1)
    {
        needs.push(crate::demand::Need {
            type_id: h,
            units: 1,
            kind: crate::demand::Kind::Spare,
        });
    }
    crate::demand::post_all(conn, prices, &needs, wallet)
}

/// **Equip a hull for an order** (DOC-2), docked: skills, modules and rounds
/// bought from asks at this station ([`acquire::run_order`]; with `rounds` any
/// round of the guns' group and size), then a standing bid for what is still
/// short ([`want_parts`]). The hull is [`obtain`]'s.
pub fn equip(
    conn: &mut Conn,
    order: &Order,
    trips: Option<Trips<'_>>,
    rounds: &acquire::Rounds<'_>,
) -> Result<(acquire::Acquired, Vec<(i64, i64)>)> {
    let got = acquire::run_order(
        conn,
        order,
        Lacking {
            ship: false,
            module: true,
            ammo: true,
            in_capsule: false,
        },
        trips,
        rounds,
    )?;
    let mut prices = match soft(Prices::read(conn))? {
        Some(p) => p,
        None => Prices::new(Default::default(), eve_server::pyutil::filetime_now()),
    };
    let wallet = conn.cash_balance()?;
    let bids = want_parts(conn, &mut prices, order, wallet)?;
    Ok((got, bids))
}

/// What [`obtain`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Got {
    /// The hull now in the hangar, if one was bought.
    pub hull: Option<i64>,
    pub note: String,
}

/// **Get a combat hull, docked, closed economy**: rank what exists
/// ([`offers_in`], [`rank`]) and buy the best [`BUY_TRIES`] in turn
/// ([`acquire::run_with`], which assembles and, with trips, fetches); with
/// none bought, rest a bid for the best hull the wallet would cover
/// ([`want_bid`]) and, in a capsule, take the free rookie ship. No GM.
pub fn obtain(
    conn: &mut Conn,
    need: &Need<'_>,
    trips: Option<Trips<'_>>,
    in_capsule: bool,
) -> Result<Got> {
    // PROG-PVE: a hull above the frigate needs the pilot's racial hull skill
    // and the class's gun skill; a pilot whose skills cannot be read is
    // offered frigates only.
    let table = match acquire::skill_levels(conn) {
        Ok(levels) => crate::progression::allowed_with(&levels, need.weapon, need.doctrine),
        Err(e @ (Error::Io(_) | Error::Protocol(_))) => return Err(e),
        Err(_) => crate::progression::allowed_with(&Default::default(), need.weapon, need.doctrine),
    };
    obtain_from(conn, &table, need, trips, in_capsule, true)
}

/// [`obtain`] over `table` (PROG-PVE: an upgrade looks only at the classes
/// above the flown one), with the standing bid and the rookie ship only when
/// `fallback`: an upgrade that finds nothing to buy waits, it does not bid
/// or fall back to a free ship.
pub fn obtain_from(
    conn: &mut Conn,
    table: &[HullInfo],
    need: &Need<'_>,
    trips: Option<Trips<'_>>,
    in_capsule: bool,
    fallback: bool,
) -> Result<Got> {
    let mut prices = match soft(Prices::read(conn))? {
        Some(p) => p,
        None => Prices::new(Default::default(), eve_server::pyutil::filetime_now()),
    };
    let offers = offers_in(conn, table, &mut prices, trips, need.wallet)?;
    let ranked: Vec<Pick> = rank(table, &offers, need)
        .into_iter()
        .filter(|p| !matches!(p.source, Source::Hangar(_)))
        .collect();
    let mut got = Got::default();
    let mut notes = Vec::new();
    for pick in ranked.iter().take(BUY_TRIES) {
        let lacking = Lacking {
            ship: true,
            module: false,
            ammo: false,
            in_capsule: false,
        };
        let done = acquire::run_order(
            conn,
            &Order::hull(pick.type_id),
            lacking,
            trips,
            &acquire::Rounds::default(),
        )?;
        notes.push(format!("hull {}: {}", pick.type_id, done.summary()));
        let present = conn
            .hangar_items()?
            .iter()
            .any(|r| r.type_id == pick.type_id && r.category_id == acquire::CATEGORY_SHIP);
        if present {
            got.hull = Some(pick.type_id);
            break;
        }
    }
    // FE-DEMAND: nothing to buy: say what the bot would take as a standing
    // bid (an upgrade, `need.strict`, bids as a fallback does, but never
    // falls back to the free rookie ship).
    if got.hull.is_none() && (fallback || need.strict) {
        let wish = table
            .iter()
            .filter_map(|info| {
                let priced = prices.p_ref(info.type_id)? * acquire::RESERVATION;
                let offer = Offer {
                    type_id: info.type_id,
                    source: Source::Local(priced),
                };
                Some(Pick {
                    type_id: info.type_id,
                    source: offer.source,
                    score: score(table, &offer, need)?,
                })
            })
            .max_by(|a, b| a.score.total_cmp(&b.score).then(b.type_id.cmp(&a.type_id)));
        if let Some(w) = wish
            && want_bid(conn, &mut prices, w.type_id, 1, need.wallet)?
        {
            notes.push(format!("no hull on offer: bid for {}", w.type_id));
        }
    }
    if got.hull.is_none() && fallback && in_capsule {
        let done = acquire::run_order(
            conn,
            &Order::default(),
            Lacking {
                ship: false,
                module: false,
                ammo: false,
                in_capsule: true,
            },
            None,
            &acquire::Rounds::default(),
        )?;
        notes.push(done.summary());
    }
    got.note = notes.join("; ");
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented hulls: ids that are no client type.
    const F1: i64 = 9_001;
    const F2: i64 = 9_002;
    const D1: i64 = 9_101;
    const C1: i64 = 9_201;
    const C2: i64 = 9_202;
    const B1: i64 = 9_301;
    const S1: i64 = 9_401;
    /// A hull with launchers and no turret hardpoint.
    const M0: i64 = 9_501;

    const TABLE: &[HullInfo] = &[
        h(F1, Class::Frigate, Weapon::Projectile, 3, 0.0, 0.2),
        h(F2, Class::Frigate, Weapon::Laser, 3, 10.0, 0.2),
        h(D1, Class::Destroyer, Weapon::Projectile, 7, 0.0, 0.3),
        h(C1, Class::Cruiser, Weapon::Projectile, 4, 40.0, 0.6),
        h(C2, Class::Cruiser, Weapon::Laser, 4, 40.0, 0.6),
        h(B1, Class::Battlecruiser, Weapon::Projectile, 6, 40.0, 0.7),
        h(S1, Class::Battleship, Weapon::Projectile, 8, 125.0, 0.9),
        h(M0, Class::Battlecruiser, Weapon::Missile, 0, 25.0, 0.75),
    ];

    fn need(wallet: f64) -> Need<'static> {
        Need {
            weapon: Weapon::Projectile,
            drones: false,
            wallet,
            risk: 0.5,
            bias: [0.0; CLASSES],
            taste: 0,
            avoid: &[],
            untrained: &[],
            strict: false,
            doctrine: None,
        }
    }

    fn ask(type_id: i64, price: f64) -> Offer {
        Offer {
            type_id,
            source: Source::Local(price),
        }
    }

    /// A doctrine fit on hull `hull`.
    fn fit_on(hull: i64) -> crate::doctrine::RoleFit {
        use crate::doctrine::testkit::{brawler, flat_tank};
        crate::doctrine::RoleFit {
            hull,
            modules: vec![],
            charges: vec![],
            skills: vec![],
            profile: brawler(flat_tank(1.0, 1.0, 1.0)),
            cost: 1.0,
        }
    }

    #[test]
    fn the_doctrine_hull_outranks_taste() {
        // Two cruisers of one class; the bot's guns and its taste favour C1
        // and the doctrine names C2.
        let fit_c2 = fit_on(C2);
        let mut best_taste = None;
        for taste in 0..2_000u64 {
            let n = Need { taste, ..need(1e6) };
            let a = score(TABLE, &ask(C1, 100.0), &n).unwrap();
            let b = score(TABLE, &ask(C2, 100.0), &n).unwrap();
            // C1 is bonused to the bot's guns; find a seed whose taste adds
            // to that, the most the taste can.
            if a > b + WEAPON_MATCH + TASTE * 0.4 {
                best_taste = Some(taste);
                break;
            }
        }
        let taste = best_taste.expect("some seed likes C1 beyond the weapon match");
        let n = Need { taste, ..need(1e6) };
        let plain = rank(TABLE, &[ask(C1, 100.0), ask(C2, 100.0)], &n);
        assert_eq!(plain[0].type_id, C1, "taste decides without a doctrine");
        let with = Need {
            doctrine: Some(&fit_c2),
            ..n
        };
        let r = rank(TABLE, &[ask(C1, 100.0), ask(C2, 100.0)], &with);
        assert_eq!(r[0].type_id, C2, "the doctrine hull wins: {r:?}");
        const { assert!(DOC_BONUS > TASTE + WEAPON_MATCH) };
        // A doctrine hull the wallet cannot cover is no offer: the bot
        // takes what it can pay for.
        let poor = rank(TABLE, &[ask(C1, 100.0), ask(C2, 900_000.0)], &with);
        assert_eq!(poor.len(), 1);
        assert_eq!(poor[0].type_id, C1);
        // A hull the table does not list is scored, with no hardpoint rule.
        let unlisted = fit_on(77_777);
        let table = [TABLE, &[HullInfo::doctrine(unlisted.hull)]].concat();
        let n = Need {
            doctrine: Some(&unlisted),
            ..need(1e6)
        };
        let r = rank(&table, &[ask(C1, 100.0), ask(unlisted.hull, 100.0)], &n);
        assert_eq!(r[0].type_id, unlisted.hull);
    }

    #[test]
    fn an_owned_old_hull_keeps_flying_until_lost() {
        let fit = fit_on(C2);
        let hangar = [
            row(10, F1, acquire::CATEGORY_SHIP),
            row(11, C1, acquire::CATEGORY_SHIP),
            row(12, C2, acquire::CATEGORY_SHIP),
        ];
        let n = Need {
            doctrine: Some(&fit),
            ..need(1e6)
        };
        // Flying the old cruiser with the doctrine's cruiser in the
        // hangar: the old hull stays (same class, no real upgrade).
        assert_eq!(board_choice(TABLE, &hangar, Some(11), Some(C1), &n), None);
        // Lost it (a capsule): the doctrine hull is boarded first.
        assert_eq!(board_choice(TABLE, &hangar, Some(99), None, &n), Some(12));
        // And one not in the table at all is boarded too.
        let odd = fit_on(88_888);
        let hangar = [
            row(10, F1, acquire::CATEGORY_SHIP),
            row(13, 88_888, acquire::CATEGORY_SHIP),
        ];
        let n = Need {
            doctrine: Some(&odd),
            ..need(1e6)
        };
        assert_eq!(board_choice(TABLE, &hangar, Some(99), None, &n), Some(13));
    }

    #[test]
    fn a_pilot_in_a_role_hull_the_doctrine_dropped_is_not_stranded() {
        let dictor = ROLE_HULLS[0];
        let table = [TABLE, &[HullInfo::doctrine(dictor)]].concat();
        let hangar = [
            row(10, dictor, acquire::CATEGORY_SHIP),
            row(12, C2, acquire::CATEGORY_SHIP),
        ];
        // The doctrine moved to C2: the dictor scores nothing plain, yet its
        // pilot boards the doctrine hull.
        let fit = fit_on(C2);
        let n = Need {
            doctrine: Some(&fit),
            ..need(1e6)
        };
        assert_eq!(
            board_choice(&table, &hangar, Some(10), Some(dictor), &n),
            Some(12)
        );
        // No doctrine at all: same.
        assert_eq!(
            board_choice(&table, &hangar, Some(10), Some(dictor), &need(1e6)),
            Some(12)
        );
        // Still the doctrine's own hull: it keeps flying.
        let mine = fit_on(dictor);
        let n = Need {
            doctrine: Some(&mine),
            ..need(1e6)
        };
        assert_eq!(
            board_choice(&table, &hangar, Some(10), Some(dictor), &n),
            None
        );
    }

    #[test]
    fn a_hangar_hull_beats_the_same_hull_on_the_market() {
        let offers = [
            ask(C1, 100.0),
            Offer {
                type_id: C1,
                source: Source::Hangar(7),
            },
        ];
        let r = rank(TABLE, &offers, &need(1_000.0));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].source, Source::Hangar(7));
    }

    #[test]
    fn what_the_wallet_cannot_cover_is_left_out() {
        let offers = [ask(C1, 900.0), ask(F1, 100.0)];
        let r = rank(TABLE, &offers, &need(1_000.0));
        assert_eq!(r.iter().map(|p| p.type_id).collect::<Vec<_>>(), vec![F1]);
        assert!(rank(TABLE, &offers, &need(10.0)).is_empty());
    }

    #[test]
    fn a_rich_bot_prefers_the_bigger_class_and_a_poor_one_the_cheap_hull() {
        let offers = [ask(F1, 10.0), ask(C1, 400.0)];
        let rich = rank(TABLE, &offers, &need(1_000_000.0));
        assert_eq!(rich[0].type_id, C1);
        let poor = rank(TABLE, &offers, &need(600.0));
        assert_eq!(poor[0].type_id, F1);
    }

    #[test]
    fn the_weapon_and_the_drone_bay_count() {
        let offers = [ask(C1, 100.0), ask(C2, 100.0)];
        assert_eq!(rank(TABLE, &offers, &need(1e6))[0].type_id, C1);
        let offers = [ask(F1, 10.0), ask(F2, 10.0)];
        assert_eq!(rank(TABLE, &offers, &need(1e6))[0].type_id, F1);
        let mut n = need(1e6);
        n.drones = true;
        n.weapon = Weapon::Laser;
        assert_eq!(rank(TABLE, &offers, &n)[0].type_id, F2);
    }

    #[test]
    fn risk_trades_tank_for_guns() {
        let offers = [ask(D1, 10.0), ask(F1, 10.0)];
        let mut cautious = need(1e6);
        cautious.risk = 0.0;
        cautious.bias = [1.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(rank(TABLE, &offers, &cautious)[0].type_id, F1);
        let mut bold = need(1e6);
        bold.risk = 1.0;
        assert_eq!(rank(TABLE, &offers, &bold)[0].type_id, D1);
    }

    #[test]
    fn avoided_hulls_and_unknown_ones_are_skipped() {
        let offers = [ask(C1, 10.0), ask(1, 10.0), ask(F1, 10.0)];
        let mut n = need(1e6);
        let avoid = [C1];
        n.avoid = &avoid;
        let r = rank(TABLE, &offers, &n);
        assert_eq!(r.iter().map(|p| p.type_id).collect::<Vec<_>>(), vec![F1]);
    }

    /// SKILLGATE: a hull the skills do not board yet is set aside, not
    /// banned: it ranks again once `untrained` is cleared.
    #[test]
    fn an_untrained_hull_is_skipped_and_not_banned() {
        let offers = [ask(C1, 10.0), ask(F1, 10.0)];
        let mut n = need(1e6);
        let set_aside = [C1];
        n.untrained = &set_aside;
        let r = rank(TABLE, &offers, &n);
        assert_eq!(r.iter().map(|p| p.type_id).collect::<Vec<_>>(), vec![F1]);
        assert!(n.avoid.is_empty(), "nothing was refused");
        n.untrained = &[];
        assert_eq!(rank(TABLE, &offers, &n).len(), 2);
    }

    #[test]
    fn taste_spreads_equal_bots_over_hulls_deterministically() {
        let offers = [ask(C1, 10.0), ask(C2, 10.0), ask(D1, 10.0), ask(F1, 10.0)];
        let mut firsts = std::collections::BTreeSet::new();
        for seed in 0..200u64 {
            let mut n = need(1e6);
            n.taste = seed;
            let a = rank(TABLE, &offers, &n)[0].type_id;
            assert_eq!(a, rank(TABLE, &offers, &n)[0].type_id);
            firsts.insert(a);
        }
        assert!(firsts.len() >= 2, "{firsts:?}");
    }

    fn row(item: i64, type_id: i64, cat: i64) -> ItemRow {
        ItemRow {
            item_id: item,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: cat,
            singleton: true,
        }
    }

    #[test]
    fn a_capsule_boards_the_best_hangar_hull_and_a_flown_hull_stays_unless_much_better() {
        let hangar = [
            row(10, F1, acquire::CATEGORY_SHIP),
            row(11, C1, acquire::CATEGORY_SHIP),
            row(12, 5, acquire::CATEGORY_SHIP),
            row(13, C2, 7),
        ];
        let n = need(1e6);
        assert_eq!(board_choice(TABLE, &hangar, Some(99), None, &n), Some(11));
        // Flying the cruiser: the frigate is no upgrade.
        assert_eq!(board_choice(TABLE, &hangar, Some(11), Some(C1), &n), None);
        // Flying a frigate with a cruiser waiting: board it.
        assert_eq!(
            board_choice(TABLE, &hangar, Some(10), Some(F1), &n),
            Some(11)
        );
        assert_eq!(board_choice(TABLE, &[], None, None, &n), None);
    }

    /// PROG-PVE: the ladder's top two classes score above the cruiser, a
    /// rich bot takes the bigger hull, a poor one still cannot.
    #[test]
    fn a_rich_bot_climbs_to_the_battleship_and_a_poorer_one_stops_where_its_wallet_does() {
        let offers = [ask(C1, 100.0), ask(B1, 400.0), ask(S1, 3_000.0)];
        assert_eq!(rank(TABLE, &offers, &need(1e9))[0].type_id, S1);
        assert_eq!(rank(TABLE, &offers, &need(2_000.0))[0].type_id, B1);
        assert_eq!(rank(TABLE, &offers, &need(300.0))[0].type_id, C1);
        for c in 1..CLASSES {
            assert!(CLASS_VALUE[c] > CLASS_VALUE[c - 1]);
        }
    }

    /// A hull with no turret hardpoint cannot carry a turret bot's guns: it
    /// is not offered to one, and a missile bot may still take it.
    #[test]
    fn a_turretless_hull_is_not_for_a_turret_bot() {
        let offers = [ask(M0, 10.0), ask(B1, 10.0)];
        let turret = rank(TABLE, &offers, &need(1e6));
        assert_eq!(
            turret.iter().map(|p| p.type_id).collect::<Vec<_>>(),
            vec![B1]
        );
        let mut missile = need(1e6);
        missile.weapon = Weapon::Missile;
        assert!(
            rank(TABLE, &offers, &missile)
                .iter()
                .any(|p| p.type_id == M0)
        );
    }

    /// PROG-PVE: a hangar hull of a bigger class is always the step up, even
    /// where the class values differ by less than the margin; within a
    /// class the margin rule is as before.
    #[test]
    fn a_bigger_class_in_the_hangar_boards_whatever_the_margin() {
        const { assert!(CLASS_VALUE[1] - CLASS_VALUE[0] <= UPGRADE_MARGIN) };
        let hangar = [
            row(10, F1, acquire::CATEGORY_SHIP),
            row(11, D1, acquire::CATEGORY_SHIP),
        ];
        let mut n = need(1e6);
        n.risk = 0.0;
        n.bias = [1.0, 0.0, 0.0, 0.0, 0.0];
        // The bias makes the frigate score above the destroyer, but a
        // flown frigate still boards the destroyer waiting in the hangar
        // (the margin rule alone would have kept it).
        let f = score(
            TABLE,
            &Offer {
                type_id: F1,
                source: Source::Hangar(10),
            },
            &n,
        )
        .unwrap();
        let d = score(
            TABLE,
            &Offer {
                type_id: D1,
                source: Source::Hangar(11),
            },
            &n,
        )
        .unwrap();
        assert!(f > d, "{f} {d}");
        assert_eq!(
            board_choice(TABLE, &hangar, Some(10), Some(F1), &n),
            Some(11)
        );
        n.bias = [0.0; CLASSES];
        let up = [
            row(10, F1, acquire::CATEGORY_SHIP),
            row(12, B1, acquire::CATEGORY_SHIP),
        ];
        assert_eq!(board_choice(TABLE, &up, Some(10), Some(F1), &n), Some(12));
        assert_eq!(class_of(TABLE, B1), Some(Class::Battlecruiser));
        assert_eq!(class_of(TABLE, 1), None);
    }

    #[test]
    fn the_real_table_is_unique_and_ordered_by_class() {
        let mut ids: Vec<i64> = HULLS.iter().map(|t| t.type_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), HULLS.len());
        assert!(HULLS.windows(2).all(|w| w[0].class <= w[1].class));
        assert!(is_combat_hull(crate::escort::RIFTER));
    }

    #[test]
    fn unmet_bids_move_a_blueprint_to_the_front_of_the_builders_queue() {
        let bp = |type_id| crate::verbs::BlueprintRow {
            item_id: type_id,
            type_id,
            location_id: 1,
            flag_id: 4,
            runs: -1,
            material_efficiency: 0,
            job_id: None,
        };
        let (a, b, c) = (bp(1), bp(2), bp(3));
        let demand = std::collections::BTreeMap::from([(2, 5), (3, -4)]);
        let order = crate::builder::promote_demanded(vec![&a, &b, &c], &demand);
        assert_eq!(
            order.iter().map(|r| r.type_id).collect::<Vec<_>>(),
            vec![2, 1, 3]
        );
        let none = std::collections::BTreeMap::new();
        let order = crate::builder::promote_demanded(vec![&a, &b, &c], &none);
        assert_eq!(
            order.iter().map(|r| r.type_id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_bid_with_no_ask_raises_a_products_need() {
        let q = |ask, bid| crate::signal::Quote {
            type_id: 1,
            p_ref: None,
            p_mkt: None,
            ask_units: ask,
            bid_units: bid,
            ..Default::default()
        };
        let stocked = crate::signal::product_need(&q(100, 0));
        let wanted = crate::signal::product_need(&q(100, 80));
        assert!(wanted > stocked, "{wanted} {stocked}");
        assert!(crate::signal::product_need(&q(0, 0)) >= crate::signal::stock_need(0));
    }

    #[test]
    fn the_drone_blueprints_are_a_ladder_rung() {
        assert_eq!(
            crate::builder::BLUEPRINT_LADDER.len(),
            crate::builder::BLUEPRINT_RUNGS
                .iter()
                .map(|r| r.len())
                .sum::<usize>()
        );
        assert!(crate::builder::rung_of(2_455).is_some());
    }

    /// PROG-PVE2: a first hull takes the weapon as a preference; an upgrade
    /// is held to the class's hardpoints.
    #[test]
    fn an_upgrade_is_held_to_the_hardpoints_and_a_first_hull_is_not() {
        // Real types, ids only: the Hurricane has three launcher hardpoints
        // (a battlecruiser fits four), the Drake six, the Raven six.
        let (hurricane, drake, raven) = (24_702, 24_698, 638);
        // Sorted by type id: Raven 638, Drake 24_698, Hurricane 24_702.
        let offers = [ask(hurricane, 10.0), ask(drake, 10.0), ask(raven, 10.0)];
        let mut n = need(1e6);
        n.weapon = Weapon::Missile;
        let ids = |n: &Need<'_>| {
            let mut v: Vec<i64> = rank(HULLS, &offers, n).iter().map(|p| p.type_id).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(ids(&n), vec![raven, drake, hurricane]);
        n.strict = true;
        assert_eq!(ids(&n), vec![raven, drake]);
        // A turret bot: the Cyclone has two turret hardpoints, a battlecruiser
        // fits four.
        let cyclone = 16_231;
        let offers = [ask(cyclone, 10.0), ask(hurricane, 10.0)];
        let mut t = need(1e6);
        t.strict = false;
        assert_eq!(rank(HULLS, &offers, &t).len(), 2);
        t.strict = true;
        let got: Vec<i64> = rank(HULLS, &offers, &t).iter().map(|p| p.type_id).collect();
        assert_eq!(got, vec![hurricane]);
    }
}
