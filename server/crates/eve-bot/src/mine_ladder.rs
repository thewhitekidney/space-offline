//! **The mining career ladder** (PROG-MINE): the hulls a miner grows into
//! over time, the skills each asks, which one it buys, and what each is worth
//! an hour. Lane note `docs/lanes/prog-mine.md`.
//!
//! A player starts in a rookie ship, flies a Venture, then a T1 mining
//! barge (Procurer, Retriever, Covetor), then a T2 exhumer (Skiff, Mackinaw,
//! Hulk), and at some point mines in a fleet with an Orca or Rorqual pilot
//! who boosts it and compresses its ore. This file is the first half of
//! that: the pure table and the chooser. The second half is
//! [`crate::foreman`].
//!
//! - **The table** ([`HULLS`], best first): type ids, tier, the ore hold and
//!   the lasers a hull can carry are **[V-data]** (`types/typedata.tsv`:
//!   `generalMiningHoldCapacity` 1556, high slots 14), as are the skills
//!   each asks (`requiredSkill1..3` 182-184 at 277-279): a Venture
//!   Mining Frigate I; a barge Mining Barge I and Astrogeology III; an
//!   exhumer Exhumers I and Mining Barge V. A Strip Miner I asks Mining IV
//!   and Astrogeology I. A barge cannot use a Miner I well, so the ladder
//!   also names the laser ([`laser_of`]).
//! - **The chooser** ([`candidates`], pure): of the table, the hulls the
//!   pilot can fly (skills), is holding or can pay for ([`SPEND_SHARE`] of
//!   the wallet, the laser included), and that beat the hull it flies by
//!   [`UPGRADE_MARGIN`]. [`crate::acquire`] tries them best first and then
//!   the old Venture-then-Merlin tail, so nothing that worked stops working.
//!   **Nothing from nowhere**: the hulls are bought from asks, which are
//!   what builders list ([`crate::hulls::mining_rungs`] gives them a rung)
//!   or NPCs drop; nothing here calls a GM verb.
//! - **The model** ([`effective_m3_h`], [`isk_h`], pure): what a hull earns
//!   an hour, from the laser's own cycle and cut, the hull's hold, a trip's
//!   overhead, a burst and compression. It is the ranking and the career
//!   prior's scale ([`income_mult`]); the report numbers in the lane note
//!   come from it at the bundle's reference prices.
//!
//! **Every figure that is not a table [V] is [A]**: the hull bonus
//! ([`Hull::bonus`], a ranking weight where the real number is a skill-bonus
//! stack the server's dogma computes), [`TRIP_OVERHEAD_H`], [`SPEND_SHARE`],
//! [`UPGRADE_MARGIN`]. Falsified by a closed soak where the miner in a hull
//! this ranks higher earns less than the one it ranks lower (the `[econ]`
//! ledger per hull).

pub use crate::brain::MERLIN;
pub use crate::huffer::VENTURE;
use std::collections::BTreeMap;

/// Procurer, Retriever, Covetor (T1 barges) **[V-data]**, group 463.
pub const PROCURER: i64 = 17_480;
pub const RETRIEVER: i64 = 17_478;
pub const COVETOR: i64 = 17_476;
/// Skiff, Mackinaw, Hulk (exhumers) **[V-data]**, group 543.
pub const SKIFF: i64 = 22_546;
pub const MACKINAW: i64 = 22_548;
pub const HULK: i64 = 22_544;
/// The Orca (group 941) and the Rorqual (group 883) **[V-data]**.
pub const ORCA: i64 = 28_606;
pub const RORQUAL: i64 = 28_352;
/// The Porpoise, the Orca's small sister (group 941) **[V-data]**.
pub const PORPOISE: i64 = 42_244;
/// Strip Miner I **[V-data]**: group 464, fits barges and exhumers only
/// (`canFitShipGroup` 1298/1299 = 463/543).
pub const STRIP_MINER_I: i64 = 17_482;

/// Skills **[V-data]**.
pub const SKILL_MINING: i64 = 3_386;
pub const SKILL_ASTROGEOLOGY: i64 = 3_410;
pub const SKILL_MINING_FRIGATE: i64 = 32_918;
pub const SKILL_MINING_BARGE: i64 = 17_940;
pub const SKILL_EXHUMERS: i64 = 22_551;
pub const SKILL_INDUSTRIAL_COMMAND: i64 = 29_637;
pub const SKILL_MINING_FOREMAN: i64 = 22_536;
pub const SKILL_MINING_DIRECTOR: i64 = 22_552;
pub const SKILL_LEADERSHIP: i64 = 3_348;
pub const SKILL_COMPRESSION: i64 = 62_450;

/// A hull's rung on the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The rookie ship's successor, the Merlin the loop started on.
    Starter,
    /// A mining frigate.
    Frigate,
    /// A T1 mining barge.
    Barge,
    /// A T2 exhumer.
    Exhumer,
}

/// One mining hull.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hull {
    pub type_id: i64,
    pub tier: Tier,
    /// The hold the ore goes into, m3 **[V-data]** (the Merlin's is its
    /// cargo hold, the others' `generalMiningHoldCapacity`).
    pub hold: f64,
    /// High slots a laser can take **[V-data]** (attribute 14; the Merlin's
    /// 3 hardpoints and the Venture's 3 high slots, 2 of them turrets).
    pub lasers: u8,
    /// The hull's mining multiplier over the bare laser **[A]**, a ranking
    /// weight: the real stack (hull bonus x skill levels) is the server's.
    pub bonus: f64,
    /// What it takes to fly it **[V-data]**: `(skill, level)`.
    pub skills: &'static [(i64, i64)],
}

const BARGE_SKILLS: &[(i64, i64)] = &[(SKILL_MINING_BARGE, 1), (SKILL_ASTROGEOLOGY, 3)];
const EXHUMER_SKILLS: &[(i64, i64)] = &[(SKILL_EXHUMERS, 1), (SKILL_MINING_BARGE, 5)];

const fn hull(
    type_id: i64,
    tier: Tier,
    hold: f64,
    lasers: u8,
    bonus: f64,
    skills: &'static [(i64, i64)],
) -> Hull {
    Hull {
        type_id,
        tier,
        hold,
        lasers,
        bonus,
        skills,
    }
}

/// **The ladder, best first** (by [`effective_m3_h`] at the defaults; a test
/// pins the order to the model, so this is not a second opinion).
pub static HULLS: &[Hull] = &[
    hull(HULK, Tier::Exhumer, 11_500.0, 2, 1.45, EXHUMER_SKILLS),
    hull(MACKINAW, Tier::Exhumer, 31_500.0, 2, 1.30, EXHUMER_SKILLS),
    hull(COVETOR, Tier::Barge, 9_000.0, 2, 1.35, BARGE_SKILLS),
    hull(RETRIEVER, Tier::Barge, 27_500.0, 2, 1.25, BARGE_SKILLS),
    hull(SKIFF, Tier::Exhumer, 18_500.0, 2, 1.20, EXHUMER_SKILLS),
    hull(PROCURER, Tier::Barge, 16_000.0, 2, 1.10, BARGE_SKILLS),
    hull(
        VENTURE,
        Tier::Frigate,
        5_000.0,
        2,
        1.25,
        &[(SKILL_MINING_FRIGATE, 1)],
    ),
    hull(MERLIN, Tier::Starter, 150.0, 3, 1.0, &[]),
];

/// The tail [`crate::acquire::hulls_of`] always tries last (GAPS-B): a
/// Venture, then the Merlin. What worked before the ladder keeps working.
pub const LEGACY: [i64; 2] = [VENTURE, MERLIN];

/// The hull rows' type ids in [`HULLS`] order: `brain::MINER_HULLS`.
pub const ORDER: [i64; 8] = [
    HULK, MACKINAW, COVETOR, RETRIEVER, SKIFF, PROCURER, VENTURE, MERLIN,
];

/// The row for `type_id`.
pub fn hull_of(type_id: i64) -> Option<&'static Hull> {
    HULLS.iter().find(|h| h.type_id == type_id)
}

/// The laser a hull is fitted with: a Strip Miner I on a barge or an
/// exhumer (the only hulls that take one), a Miner I on the rest.
pub fn laser_of(type_id: i64) -> i64 {
    match hull_of(type_id).map(|h| h.tier) {
        Some(Tier::Barge | Tier::Exhumer) => STRIP_MINER_I,
        _ => crate::brain::MINER_I,
    }
}

/// What flying a Strip Miner I asks **[V-data]**.
pub const STRIP_SKILLS: &[(i64, i64)] = &[(SKILL_MINING, 4), (SKILL_ASTROGEOLOGY, 1)];

/// A laser's cycle and cut **[V-data]** (`77` m3 per `73` ms): a Miner I
/// cuts 10 m3 in 15 s, a Strip Miner I 150 m3 in 45 s.
pub fn laser_m3_h(laser: i64) -> f64 {
    let (m3, secs) = if laser == STRIP_MINER_I {
        (150.0, 45.0)
    } else {
        (10.0, 15.0)
    };
    m3 * 3600.0 / secs
}

/// Whether `skills` (`{skill: trained level}`) meet `needs`.
pub fn meets(skills: &BTreeMap<i64, i64>, needs: &[(i64, i64)]) -> bool {
    needs
        .iter()
        .all(|&(s, l)| skills.get(&s).is_some_and(|&t| t >= l))
}

/// Whether the pilot can fly `h` **and** use its laser.
pub fn can_fly(skills: &BTreeMap<i64, i64>, h: &Hull) -> bool {
    meets(skills, h.skills) && (laser_of(h.type_id) != STRIP_MINER_I || meets(skills, STRIP_SKILLS))
}

// ---- the model ---------------------------------------------------------------

/// A trip's overhead, hours (dock, unload, sell, undock, warp back) **[A]**,
/// 3 minutes: the closed soak's Merlin trip is about 7 minutes (docs/lanes/
/// gb-fix.md) of which a 150 m3 hold at a Miner I's 2,400 m3/h is 3.75.
/// Falsified by a trip log whose non-mining part is far from 3 minutes.
pub const TRIP_OVERHEAD_H: f64 = 0.05;

/// The lasers the bot's brain runs today: one (`Observation::miner`).
pub const LASERS_USED: u8 = 1;

/// A mining burst's cycle cut with a Burst I and the Optimization charge
/// **[V-data]** (charge `warfareBuff1Multiplier` -15 % at the module's
/// 1.0): a laser cycle 15 % shorter is `1 / (1 - 0.15)` the yield.
pub const BURST_CYCLE_CUT: f64 = 0.15;

/// Compressed ore is a hundredth of the raw volume per unit **[V-data]**.
pub const COMPRESS_VOLUME: f64 = 0.01;

/// The laser rate of `h` with `lasers` running, m3/h, `boost` being a burst's
/// cycle cut (0 for none).
pub fn rate_m3_h(h: &Hull, lasers: u8, boost: f64) -> f64 {
    let lasers = lasers.min(h.lasers) as f64;
    lasers * laser_m3_h(laser_of(h.type_id)) * h.bonus / (1.0 - boost.clamp(0.0, 0.9))
}

/// **What a hull mines an hour, trips included**: fill the hold at the laser
/// rate, then spend [`TRIP_OVERHEAD_H`] getting rid of it. With `compress`
/// the ore is made a hundredth of the volume ([`COMPRESS_VOLUME`]) in space,
/// so the hold lasts a hundred times longer and the overhead nearly goes.
pub fn effective_m3_h(h: &Hull, lasers: u8, boost: f64, compress: bool) -> f64 {
    let rate = rate_m3_h(h, lasers, boost);
    if rate <= 0.0 {
        return 0.0;
    }
    let hold = if compress {
        h.hold / COMPRESS_VOLUME
    } else {
        h.hold
    };
    let fill = hold / rate;
    rate * fill / (fill + TRIP_OVERHEAD_H)
}

/// The ISK an hour at `isk_per_m3` (the ore basket's price per m3).
pub fn isk_h(h: &Hull, lasers: u8, boost: f64, compress: bool, isk_per_m3: f64) -> f64 {
    effective_m3_h(h, lasers, boost, compress) * isk_per_m3
}

/// **The career prior's scale for a hull**: its effective m3/h over the
/// Merlin's, the hull the prior ([`crate::career::MINER_PRIOR_ISK_H`]) was
/// read from. 1.0 for the Merlin and for any hull not on the ladder.
pub fn income_mult(type_id: Option<i64>) -> f64 {
    let (Some(h), Some(base)) = (type_id.and_then(hull_of), hull_of(MERLIN)) else {
        return 1.0;
    };
    effective_m3_h(h, LASERS_USED, 0.0, false) / effective_m3_h(base, LASERS_USED, 0.0, false)
}

/// **The prior follows the hull** (pure): the miner career's price index
/// scaled by [`income_mult`] of the hull flown, so a bot in a barge expects
/// what a barge earns before its own ledger has the hours
/// ([`crate::mind::LEDGER_MIN_HOURS`]).
pub fn scale_prior(mut s: crate::career::Signals, flown: Option<i64>) -> crate::career::Signals {
    let mult = income_mult(flown);
    if (mult - 1.0).abs() > f64::EPSILON {
        let e = s.price_index.entry("miner".to_owned()).or_insert(1.0);
        *e *= mult;
    }
    s
}

/// **A hull change moves the ledger** (pure): when the hull flown goes from
/// a `from` multiplier to a `to` one, the career's learned ISK/h is scaled
/// by `to / from` -- the model's projection -- and the EWMA then corrects it
/// with the real wallet. Without it a pilot would keep expecting its Merlin's
/// income for a half-life after buying a barge.
pub fn rescale(mind: &mut crate::mind::Mind, career: &str, from: f64, to: f64) {
    if from <= 0.0 || to <= 0.0 || (from - to).abs() < 1e-9 {
        return;
    }
    if let Some(l) = mind.income.get_mut(career) {
        l.isk_h *= to / from;
    }
}

// ---- the chooser -----------------------------------------------------------------

/// The share of the wallet one hull-and-laser purchase may take **[A]**:
/// a miner keeps working capital for the laser's crystals, skills and the
/// next fit. Falsified by a miner that buys a hull and cannot afford its
/// laser (too high), or never upgrades with ISK to spare (too low).
pub const SPEND_SHARE: f64 = 0.6;

/// A hull must out-mine the one flown by this much to be worth the change
/// **[A]**: a quarter more, so a close pair does not trade places.
pub const UPGRADE_MARGIN: f64 = 0.25;

/// What the chooser reads.
pub struct Means<'a> {
    /// `{skill: trained level}`.
    pub skills: &'a BTreeMap<i64, i64>,
    pub wallet: f64,
    /// Hull types in the hangar (assembled or packaged).
    pub held: &'a [i64],
    /// A hull's expected ask, ISK (`P_ref` x the reservation): `None` for
    /// no price known.
    pub price: &'a dyn Fn(i64) -> Option<f64>,
    /// Whether the pilot already owns a Strip Miner I.
    pub has_strip: bool,
    /// The hull flown now.
    pub flown: Option<i64>,
}

/// **The hulls to try, best first** (pure): flyable, held or affordable, and
/// [`UPGRADE_MARGIN`] better than the hull flown.
pub fn candidates(m: &Means<'_>) -> Vec<i64> {
    pick(m, Tier::Frigate, false)
}

/// **The ice miner's hulls** (ICE-HULL, #130): the barges and exhumers of
/// the ladder, since an ice harvester fits only those (`canFitShipGroup`
/// 463/543 [V-data]). Best first, as [`HULLS`] has them.
pub fn ice_hulls() -> impl Iterator<Item = &'static Hull> {
    HULLS.iter().filter(|h| h.tier >= Tier::Barge)
}

/// Whether `type_id` is a hull an ice harvester fits.
pub fn is_ice_hull(type_id: i64) -> bool {
    hull_of(type_id).is_some_and(|h| h.tier >= Tier::Barge)
}

/// **The ice hulls to try, best first** (pure): [`candidates`] over
/// [`ice_hulls`]. The pilot needs the hull's own skills ([`meets`]); the
/// laser is an ice harvester, so the Strip Miner's Mining IV does not
/// count, and no laser is priced here (the kit buys harvesters apart).
pub fn ice_candidates(m: &Means<'_>) -> Vec<i64> {
    pick(m, Tier::Barge, true)
}

fn pick(m: &Means<'_>, min_tier: Tier, ice: bool) -> Vec<i64> {
    let flown = m
        .flown
        .and_then(hull_of)
        .map_or(0.0, |h| effective_m3_h(h, LASERS_USED, 0.0, false));
    HULLS
        .iter()
        .filter(|h| h.tier >= min_tier)
        .filter(|h| {
            if ice {
                meets(m.skills, h.skills)
            } else {
                can_fly(m.skills, h)
            }
        })
        .filter(|h| effective_m3_h(h, LASERS_USED, 0.0, false) >= flown * (1.0 + UPGRADE_MARGIN))
        .filter(|h| {
            if m.held.contains(&h.type_id) {
                return true;
            }
            let hull_price = (m.price)(h.type_id);
            let laser = laser_of(h.type_id);
            let laser_price = if !ice && laser == STRIP_MINER_I && !m.has_strip {
                (m.price)(STRIP_MINER_I).unwrap_or(0.0)
            } else {
                0.0
            };
            hull_price.is_some_and(|p| p + laser_price <= m.wallet * SPEND_SHARE)
        })
        .map(|h| h.type_id)
        .collect()
}

/// The ISK a hull and its laser cost together at `price`, `held` things
/// being free: what [`candidates`] weighs against the wallet.
pub fn outlay(h: &Hull, price: &dyn Fn(i64) -> Option<f64>, has_strip: bool) -> Option<f64> {
    let laser = laser_of(h.type_id);
    let laser_price = if laser == STRIP_MINER_I && !has_strip {
        price(STRIP_MINER_I)?
    } else {
        0.0
    };
    Some(price(h.type_id)? + laser_price)
}

/// **The hulls to try over the wire** (docked): the pilot's skills, wallet,
/// hangar and flown hull, the ask each hull would cost
/// (`P_ref` x [`crate::pricing::RESERVATION`]), then [`candidates`].
pub fn candidates_in(
    conn: &mut crate::Conn,
    prices: &crate::pricing::Prices,
) -> crate::Result<(Vec<i64>, Option<i64>)> {
    choose_in(conn, prices, false)
}

/// [`candidates_in`] for the ice miner: [`ice_candidates`].
pub fn ice_candidates_in(
    conn: &mut crate::Conn,
    prices: &crate::pricing::Prices,
) -> crate::Result<(Vec<i64>, Option<i64>)> {
    choose_in(conn, prices, true)
}

fn choose_in(
    conn: &mut crate::Conn,
    prices: &crate::pricing::Prices,
    ice: bool,
) -> crate::Result<(Vec<i64>, Option<i64>)> {
    let skills = crate::acquire::skill_levels(conn)?;
    let wallet = conn.cash_balance()?;
    let hangar = conn.hangar_items()?;
    let held: Vec<i64> = hangar
        .iter()
        .filter(|r| r.category_id == crate::acquire::CATEGORY_SHIP)
        .map(|r| r.type_id)
        .collect();
    let has_strip = hangar.iter().any(|r| r.type_id == STRIP_MINER_I)
        || conn.fitted()?.iter().any(|r| r.type_id == STRIP_MINER_I);
    let active = conn.session().ship_id();
    let flown = hangar
        .iter()
        .find(|r| Some(r.item_id) == active)
        .map(|r| r.type_id);
    // FE-CAREER: the market's index where there is one, else `P_ref`.
    let price = |t: i64| {
        prices
            .average(t)
            .or_else(|| prices.p_ref(t))
            .map(|p| p * crate::pricing::RESERVATION)
    };
    let means = Means {
        skills: &skills,
        wallet,
        held: &held,
        price: &price,
        has_strip,
        flown,
    };
    let found = if ice {
        ice_candidates(&means)
    } else {
        candidates(&means)
    };
    Ok((found, flown))
}

/// Whether `type_id` is a Venture or better: past the legacy tail, so a
/// pilot in it has nothing to fall back to.
pub fn venture_or_better(type_id: i64) -> bool {
    ORDER
        .iter()
        .position(|&h| h == type_id)
        .zip(ORDER.iter().position(|&h| h == VENTURE))
        .is_some_and(|(at, venture)| at <= venture)
}

/// Whether `laser` is the right laser for the hull `flown`: a Strip Miner I
/// on a barge or an exhumer, a Miner I on anything else (the rookie ship's
/// Civilian Miner is not in a hangar to be fitted).
pub fn laser_fits(laser: i64, flown: Option<i64>) -> bool {
    laser == flown.map_or(crate::brain::MINER_I, laser_of)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skills(rows: &[(i64, i64)]) -> BTreeMap<i64, i64> {
        rows.iter().copied().collect()
    }

    fn price_of(rows: &'static [(i64, f64)]) -> impl Fn(i64) -> Option<f64> {
        move |t| rows.iter().find(|r| r.0 == t).map(|r| r.1)
    }

    /// ICE-HULL: ice candidates are barges and exhumers only, need the
    /// hull's skills but not the Strip Miner's, and are never a Venture.
    #[test]
    fn ice_hulls_are_barges_and_exhumers_the_pilot_can_fly() {
        let price = price_of(PRICES);
        let flown = Some(MERLIN);
        let mine = |sk: &BTreeMap<i64, i64>, held: &[i64], wallet: f64| {
            ice_candidates(&Means {
                skills: sk,
                wallet,
                held,
                price: &price,
                has_strip: false,
                flown,
            })
        };
        assert!(ice_hulls().all(|h| h.tier >= Tier::Barge));
        assert!(is_ice_hull(PROCURER) && is_ice_hull(HULK));
        assert!(!is_ice_hull(VENTURE) && !is_ice_hull(MERLIN) && !is_ice_hull(ORCA));
        // Too junior for a barge: no ice hull, held or not.
        let junior = skills(&[(SKILL_MINING, 4)]);
        assert!(mine(&junior, &[PROCURER], 1e12).is_empty());
        // Mining Barge I and Astrogeology III fly a barge, with no Mining IV.
        let barge = skills(&[(SKILL_MINING_BARGE, 1), (SKILL_ASTROGEOLOGY, 3)]);
        let got = mine(&barge, &[PROCURER, RETRIEVER], 0.0);
        assert_eq!(got, vec![RETRIEVER, PROCURER], "held ones, best first");
        assert!(got.iter().all(|t| is_ice_hull(*t)));
        // An exhumer asks the exhumer skills too.
        assert!(!mine(&barge, &[HULK], 0.0).contains(&HULK));
        let exhumer = skills(&[
            (SKILL_MINING_BARGE, 5),
            (SKILL_EXHUMERS, 1),
            (SKILL_ASTROGEOLOGY, 3),
        ]);
        assert_eq!(mine(&exhumer, &[HULK], 0.0)[0], HULK);
    }

    /// Invented prices, in the ratio the ladder has: a barge 100 Mercoxit
    /// of ISK is no client number.
    const PRICES: &[(i64, f64)] = &[
        (VENTURE, 200.0),
        (RETRIEVER, 5_000.0),
        (COVETOR, 5_000.0),
        (PROCURER, 5_000.0),
        (HULK, 9_000.0),
        (MACKINAW, 9_000.0),
        (SKIFF, 9_000.0),
        (STRIP_MINER_I, 1_000.0),
    ];

    const ALL_SKILLS: &[(i64, i64)] = &[
        (SKILL_MINING, 5),
        (SKILL_ASTROGEOLOGY, 5),
        (SKILL_MINING_FRIGATE, 3),
        (SKILL_MINING_BARGE, 5),
        (SKILL_EXHUMERS, 5),
    ];

    #[test]
    fn the_table_is_in_the_models_order_and_unique() {
        let eff: Vec<f64> = HULLS
            .iter()
            .map(|h| effective_m3_h(h, LASERS_USED, 0.0, false))
            .collect();
        assert!(eff.windows(2).all(|w| w[0] > w[1]), "{eff:?}");
        assert_eq!(
            HULLS.iter().map(|h| h.type_id).collect::<Vec<_>>(),
            ORDER.to_vec()
        );
        let mut ids = ORDER.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ORDER.len());
    }

    #[test]
    fn the_ladder_climbs_by_tier_and_the_skills_match_the_tiers() {
        for h in HULLS {
            match h.tier {
                Tier::Exhumer => assert!(h.skills.contains(&(SKILL_EXHUMERS, 1))),
                Tier::Barge => assert!(h.skills.contains(&(SKILL_MINING_BARGE, 1))),
                Tier::Frigate => assert!(h.skills.contains(&(SKILL_MINING_FRIGATE, 1))),
                Tier::Starter => assert!(h.skills.is_empty()),
            }
        }
        assert_eq!(laser_of(VENTURE), crate::brain::MINER_I);
        assert_eq!(laser_of(MERLIN), crate::brain::MINER_I);
        assert_eq!(laser_of(HULK), STRIP_MINER_I);
        assert!(laser_fits(STRIP_MINER_I, Some(RETRIEVER)));
        assert!(!laser_fits(crate::brain::MINER_I, Some(RETRIEVER)));
        assert!(laser_fits(crate::brain::MINER_I, None));
        assert!(venture_or_better(VENTURE) && venture_or_better(HULK));
        assert!(!venture_or_better(MERLIN) && !venture_or_better(1));
        assert_eq!(laser_of(RETRIEVER), STRIP_MINER_I);
    }

    #[test]
    fn a_bare_miner_flies_nothing_above_the_starter_and_a_trained_one_the_best_it_can_pay() {
        let price = price_of(PRICES);
        let none = skills(&[]);
        let mut m = Means {
            skills: &none,
            wallet: 1_000_000.0,
            held: &[],
            price: &price,
            has_strip: false,
            flown: Some(MERLIN),
        };
        assert_eq!(candidates(&m), Vec::<i64>::new(), "no Mining Frigate yet");
        let frigate = skills(&[(SKILL_MINING_FRIGATE, 1)]);
        m.skills = &frigate;
        assert_eq!(candidates(&m), vec![VENTURE]);
        let all = skills(ALL_SKILLS);
        m.skills = &all;
        assert_eq!(
            candidates(&m),
            vec![HULK, MACKINAW, COVETOR, RETRIEVER, SKIFF, PROCURER, VENTURE],
            "best first"
        );
    }

    #[test]
    fn the_wallet_limits_the_climb_and_a_laser_counts() {
        let price = price_of(PRICES);
        let all = skills(ALL_SKILLS);
        let mut m = Means {
            skills: &all,
            wallet: 8_000.0,
            held: &[],
            price: &price,
            has_strip: false,
            flown: Some(VENTURE),
        };
        // 60 % of 8,000 is 4,800: a barge is 5,000 + a 1,000 laser.
        assert_eq!(candidates(&m), Vec::<i64>::new());
        m.wallet = 10_000.0;
        assert_eq!(candidates(&m), vec![COVETOR, RETRIEVER, PROCURER]);
        m.has_strip = true;
        m.wallet = 8_500.0;
        assert_eq!(
            candidates(&m),
            vec![COVETOR, RETRIEVER, PROCURER],
            "an owned laser is free; an exhumer is 9,000"
        );
        m.wallet = 15_000.0;
        assert_eq!(
            candidates(&m),
            vec![HULK, MACKINAW, COVETOR, RETRIEVER, SKIFF, PROCURER]
        );
    }

    #[test]
    fn a_held_hull_is_free_and_a_close_hull_is_not_worth_the_change() {
        let price = price_of(PRICES);
        let all = skills(ALL_SKILLS);
        let m = Means {
            skills: &all,
            wallet: 0.0,
            held: &[HULK],
            price: &price,
            has_strip: false,
            flown: Some(MERLIN),
        };
        assert_eq!(candidates(&m), vec![HULK], "held, so free");
        // Flying a Hulk, nothing is [`UPGRADE_MARGIN`] better.
        let rich = Means {
            wallet: 1.0e9,
            flown: Some(HULK),
            held: &[],
            ..m
        };
        assert!(candidates(&rich).is_empty());
        // From a Procurer only the Hulk is a quarter better.
        let sidestep = Means {
            flown: Some(PROCURER),
            ..rich
        };
        assert_eq!(candidates(&sidestep), vec![HULK]);
    }

    #[test]
    fn a_strip_miner_needs_its_own_skills() {
        let barge_only = skills(&[(SKILL_MINING_BARGE, 1), (SKILL_ASTROGEOLOGY, 3)]);
        let r = hull_of(RETRIEVER).expect("row");
        assert!(!can_fly(&barge_only, r), "Mining IV is the strip's");
        let with_mining = skills(&[
            (SKILL_MINING_BARGE, 1),
            (SKILL_ASTROGEOLOGY, 3),
            (SKILL_MINING, 4),
        ]);
        assert!(can_fly(&with_mining, r));
    }

    #[test]
    fn a_burst_and_compression_each_add_and_a_strip_dwarfs_a_miner_i() {
        let h = hull_of(HULK).expect("row");
        let plain = effective_m3_h(h, 1, 0.0, false);
        let boosted = effective_m3_h(h, 1, BURST_CYCLE_CUT, false);
        let both = effective_m3_h(h, 1, BURST_CYCLE_CUT, true);
        assert!(
            boosted > plain * 1.1 && both > boosted,
            "{plain} {boosted} {both}"
        );
        // The burst is 1 / (1 - cut) of the laser rate.
        assert!(
            (rate_m3_h(h, 1, BURST_CYCLE_CUT) / rate_m3_h(h, 1, 0.0) - 1.0 / 0.85).abs() < 1e-9
        );
        // More lasers, more ore, capped at the hull's.
        assert!(rate_m3_h(h, 2, 0.0) > 1.9 * rate_m3_h(h, 1, 0.0));
        assert_eq!(rate_m3_h(h, 9, 0.0), rate_m3_h(h, 2, 0.0));
        assert!(laser_m3_h(STRIP_MINER_I) > 4.0 * laser_m3_h(crate::brain::MINER_I));
        // A hold a hundred times as long takes the overhead out of a trip.
        let v = hull_of(VENTURE).expect("row");
        assert!(
            effective_m3_h(v, 1, 0.0, true) / rate_m3_h(v, 1, 0.0) > 0.99,
            "compressed, a Venture mines nearly all the time"
        );
    }

    #[test]
    fn the_prior_scales_with_the_hull_and_the_merlin_is_the_reference() {
        assert_eq!(income_mult(Some(MERLIN)), 1.0);
        assert_eq!(income_mult(None), 1.0);
        assert_eq!(income_mult(Some(99_999)), 1.0, "off the ladder");
        let v = income_mult(Some(VENTURE));
        let r = income_mult(Some(RETRIEVER));
        let h = income_mult(Some(HULK));
        assert!(1.0 < v && v < r && r < h, "{v} {r} {h}");
    }

    #[test]
    fn the_prior_index_follows_the_hull_and_a_hull_change_moves_the_ledger() {
        let s = scale_prior(crate::career::Signals::default(), Some(HULK));
        let idx = s.price_index.get("miner").copied().expect("scaled");
        assert!((idx - income_mult(Some(HULK))).abs() < 1e-9);
        // The Merlin and an unknown hull leave the index alone.
        for hull in [Some(MERLIN), None, Some(1)] {
            assert!(
                scale_prior(crate::career::Signals::default(), hull)
                    .price_index
                    .is_empty()
            );
        }
        // A market index already there is multiplied, not replaced.
        let mut base = crate::career::Signals::default();
        base.price_index.insert("miner".to_owned(), 2.0);
        let s = scale_prior(base, Some(VENTURE));
        assert!((s.price_index["miner"] - 2.0 * income_mult(Some(VENTURE))).abs() < 1e-9);

        let table = crate::career::CAREERS;
        let sig = crate::career::Signals::default();
        let mut mind = crate::mind::Mind::seeded("Invented Pilot", 1, table, &sig, 0);
        mind.record("miner", 10_000.0, 10.0);
        let before = mind.income["miner"].isk_h;
        let (from, to) = (income_mult(Some(MERLIN)), income_mult(Some(RETRIEVER)));
        rescale(&mut mind, "miner", from, to);
        assert!((mind.income["miner"].isk_h - before * to / from).abs() < 1e-6);
        // The wallet then corrects it: an hour at the old rate pulls it back.
        let scaled = mind.income["miner"].isk_h;
        mind.record("miner", 0.0, crate::mind::INCOME_HALF_LIFE_H);
        assert!((mind.income["miner"].isk_h - scaled / 2.0).abs() < 1e-6);
        // No ledger for the career, or a no-change, is a no-op.
        rescale(&mut mind, "ratter", 1.0, 2.0);
        assert!(!mind.income.contains_key("ratter"));
        rescale(&mut mind, "miner", 3.0, 3.0);
    }

    /// **The lane note's numbers**, read from the bundle at
    /// `$CARGO_MANIFEST_DIR/../../../out/content` (gitignored; copy it in and
    /// delete it after). `cargo test -p eve-bot --lib mine_ladder::tests::report
    /// -- --ignored --nocapture`. Prints, never asserts.
    #[test]
    #[ignore = "needs out/content, which is never committed"]
    fn report_what_each_hull_earns_at_the_bundles_reference_prices() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/content");
        let read = |f: &str| std::fs::read_to_string(root.join(f)).expect("the bundle is there");
        let prices_text = read("econ/prices.tsv");
        let price = |t: i64| -> Option<f64> {
            prices_text
                .lines()
                .filter(|l| !l.starts_with('#'))
                .find_map(|l| {
                    let c: Vec<&str> = l.split('\t').collect();
                    (c.first()?.parse::<i64>().ok()? == t).then(|| c[1].parse().ok())?
                })
        };
        let portion = |t: i64| -> f64 {
            prices_text
                .lines()
                .filter(|l| !l.starts_with('#'))
                .find_map(|l| {
                    let c: Vec<&str> = l.split('\t').collect();
                    (c.first()?.parse::<i64>().ok()? == t).then(|| c.get(7)?.parse().ok())?
                })
                .unwrap_or(1.0)
        };
        let td = read("types/typedata.tsv");
        let volume = |t: i64| -> f64 {
            td.lines()
                .filter(|l| l.starts_with("type\t"))
                .find_map(|l| {
                    let c: Vec<&str> = l.split('\t').collect();
                    (c.get(1)?.parse::<i64>().ok()? == t).then(|| c.get(6)?.parse().ok())?
                })
                .expect("the ore has a volume")
        };
        // Veldspar, Scordite, Pyroxeres, Plagioclase: the high-sec belts.
        let basket = [1230_i64, 1228, 1224, 18];
        let per_m3: Vec<f64> = basket
            .iter()
            .map(|&t| price(t).expect("a price") / portion(t) / volume(t))
            .collect();
        let isk_m3 = per_m3.iter().sum::<f64>() / per_m3.len() as f64;
        println!("ore basket ISK per m3: {per_m3:?} -> mean {isk_m3:.1}");
        let prior = crate::career::MINER_PRIOR_ISK_H;
        println!(
            "hull        cost(+laser)      plain   boosted  +compress  2 lasers+both   hours@prior"
        );
        for h in HULLS {
            let p = price(h.type_id).unwrap_or(0.0);
            let laser = if laser_of(h.type_id) == STRIP_MINER_I {
                price(STRIP_MINER_I).unwrap_or(0.0)
            } else {
                0.0
            };
            let plain = isk_h(h, 1, 0.0, false, isk_m3);
            let boosted = isk_h(h, 1, BURST_CYCLE_CUT, false, isk_m3);
            let both = isk_h(h, 1, BURST_CYCLE_CUT, true, isk_m3);
            let full = isk_h(h, h.lasers, BURST_CYCLE_CUT, true, isk_m3);
            println!(
                "{:>8} {:>16.0} {:>10.0} {:>9.0} {:>10.0} {:>13.0} {:>12.0}",
                h.type_id,
                p + laser,
                plain,
                boosted,
                both,
                full,
                (p + laser) / prior
            );
        }
    }
}
