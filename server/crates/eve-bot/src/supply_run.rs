//! **Supply runs** (PROG-NULL): a bot that needs a thing sold only far away
//! goes and gets it, the way a player does, and weighs the danger first.
//!
//! **Why.** PROG-MINE found that a closed world never builds a mining barge
//! on its own: the barge originals are sold only by ORE, in null-sec, and
//! D-HUB found the Sov Hub original behind the same wall. [`crate::travel`]
//! fetches within [`crate::travel::max_jumps`] of the value, at the bot's
//! security floor (high-sec); a rung whose only seller is farther, or behind
//! a low-sec or null-sec gate, was a dead rung. This module makes it a
//! **run**: the same [`crate::travel::fetch`] (gates, buy, hold, home), but
//! over a longer, riskier route that is *chosen* by what the goods are worth.
//!
//! - **The danger** ([`jump_death`], [`route_risk`]): the chance of losing
//!   the ship on the way, per jump into a system by its security band, plus
//!   the [`Hazards`] a society layer reports for a system (C5 hunters, reds).
//!   A round trip is the route twice.
//! - **The route** ([`plan`]): the highest security floor that still reaches
//!   the seller ([`FLOORS`]: high-sec only, then low-sec allowed, then
//!   anything), so a run uses null-sec only when it must.
//! - **The weighing** ([`plan`]): `net = gain - price - fee - risk x
//!   (price + exposure)`. `gain` is what the rung is worth to the bot
//!   ([`rung_worth`]), `exposure` the hull and wallet a loss would cost.
//!   Positive and under [`MAX_RISK`], the best net wins.
//! - **Cover** ([`Cover`]): an escort (`escort.rs`' role) cuts the risk to
//!   [`ESCORT_FACTOR`] of itself for [`ESCORT_FEE_PER_JUMP`] a jump. `plan`
//!   is told how many escorts are on offer and takes one when it raises the
//!   net; with none, a risk the gain does not pay for is a run not made.
//! - **The run** is the builder's `fetch_rung` over [`trips_for`] the chosen
//!   floor; this module owns no I/O.
//!
//! Everything is **[A] policy**; the falsifier of each constant is beside it.

use crate::travel::{ANY_SECURITY, Map, SECURITY_FLOOR, Source, Trips};
use crate::verbs::Book;
use eve_server::celestial::pseudo_security;
use std::collections::BTreeMap;

/// The floors a run tries, safest first **[A]**: high-sec only, low-sec
/// allowed (security 0.05 and up), anything. Falsified by runs that cross
/// null-sec when a low-sec route existed (the order is wrong).
pub const FLOORS: [f64; 3] = [SECURITY_FLOOR, LOW_FLOOR, ANY_SECURITY];

/// The floor that lets low-sec through (`pseudo_security` lifts a
/// near-zero system to 0.05).
pub const LOW_FLOOR: f64 = 0.05;

/// Chance of losing the ship on entering a system **[A]**, by band: a
/// careful pilot in high-sec is almost never lost (CONCORD), low-sec has
/// gate camps, null-sec has hunters and no law. Falsified by the soak's
/// loss ledger: runs that lose more (raise) or never lose (lower).
pub const DEATH_HIGH: f64 = 0.0005;
pub const DEATH_LOW: f64 = 0.01;
pub const DEATH_NULL: f64 = 0.03;
/// A system whose security the map does not know counts as null-sec.
pub const DEATH_UNKNOWN: f64 = DEATH_NULL;

/// The most risk a run accepts **[A]**, whatever it pays: a coin flip with
/// the hull is not a plan. Falsified by runs refused that a player makes.
pub const MAX_RISK: f64 = 0.35;

/// An escort leaves this share of the risk **[A]** (the fight is theirs).
pub const ESCORT_FACTOR: f64 = 0.4;
/// What an escort costs per jump of the round trip, ISK **[A]**.
pub const ESCORT_FEE_PER_JUMP: f64 = 20_000.0;

/// What a rung is worth, as a multiple of its original's price **[A]**: an
/// original builds many runs; the barge pays back its blueprint several
/// times over (PROG-MINE's model: a barge earns 4-5x a Venture). Falsified
/// by builders that make the run and never recover its price.
pub const RUNG_WORTH_X: f64 = 6.0;

/// Longest route a run flies, jumps one way **[A]**: the far-null ORE
/// station is 21-32 jumps from the schools (E5b).
pub const RUN_MAX_JUMPS: u32 = 40;

/// Extra danger the society layer reports per system (C5 hunters, society
/// reds): a chance added to the band's, per entry into the system. Empty
/// until the society layer posts into it (the seam; `docs/lanes/prog-null.md`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hazards(pub BTreeMap<i64, f64>);

impl Hazards {
    /// Nothing reported.
    pub fn none() -> Hazards {
        Hazards::default()
    }

    /// The added chance in `system`.
    pub fn extra(&self, system: i64) -> f64 {
        self.0.get(&system).copied().unwrap_or(0.0).clamp(0.0, 1.0)
    }
}

/// The chance of losing the ship entering a system of `security` (`None`:
/// unknown).
pub fn jump_death(security: Option<f64>) -> f64 {
    match security.map(pseudo_security) {
        Some(s) if s >= SECURITY_FLOOR => DEATH_HIGH,
        Some(s) if s >= LOW_FLOOR => DEATH_LOW,
        Some(_) => DEATH_NULL,
        None => DEATH_UNKNOWN,
    }
}

/// The chance of losing the ship along `route` (systems, start included
/// and not entered): each system after the first is a roll.
pub fn route_risk(map: &Map, route: &[i64], hazards: &Hazards) -> f64 {
    let mut safe = 1.0;
    for &s in route.iter().skip(1) {
        let p = (jump_death(map.security(s)) + hazards.extra(s)).min(1.0);
        safe *= 1.0 - p;
    }
    1.0 - safe
}

/// Who watches the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    /// Alone.
    None,
    /// A hired escort flies with the run.
    Escort,
}

/// One run worth making.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// The seller; `jumps` is the one-way length at `floor`.
    pub source: Source,
    /// The floor to route at.
    pub floor: f64,
    /// The chance of losing the ship out and back, covered.
    pub risk: f64,
    pub cover: Cover,
    /// Gain less every cost, ISK; positive.
    pub net: f64,
}

/// The worth of a rung whose original costs `price`.
pub fn rung_worth(price: f64) -> f64 {
    price * RUNG_WORTH_X
}

/// What a lost hull and wallet put at stake on a builder's run **[A]**: the
/// builder flies a rookie ship or an industrial; the original rides in its
/// hold. Falsified by builders whose lost runs cost more than this.
pub const BUILDER_EXPOSURE: f64 = 1_000_000.0;

/// What the weighing needs besides the book.
#[derive(Debug, Clone, Copy)]
pub struct Want<'a> {
    pub map: &'a Map,
    /// Where the bot is, and its home station (excluded as a seller).
    pub from: i64,
    pub home: i64,
    /// The consumer's price cap (`None`: any).
    pub cap: Option<f64>,
    pub wallet: f64,
    /// Units wanted.
    pub units: i64,
    /// What the goods are worth to the bot, ISK: `gain` plus `worth_x`
    /// times the seller's price ([`rung_worth`]).
    pub gain: f64,
    pub worth_x: f64,
    /// What a loss would cost besides the goods: the hull flown.
    pub exposure: f64,
    pub hazards: &'a Hazards,
    /// Escorts on offer.
    pub escorts: u32,
}

/// **The best run** for the goods in `book`, from stations other than home
/// that the safest floor reaches within [`RUN_MAX_JUMPS`]; `None` when none
/// pays. Pure.
pub fn plan(book: &Book, w: &Want<'_>) -> Option<Candidate> {
    let mut by_station: BTreeMap<i64, (f64, i64)> = BTreeMap::new();
    for a in &book.asks {
        if a.bid || a.station_id == w.home || a.vol_remaining <= 0 || a.price <= 0.0 {
            continue;
        }
        if w.cap.is_some_and(|c| a.price > c) || a.price > w.wallet {
            continue;
        }
        let e = by_station.entry(a.station_id).or_insert((a.price, 0));
        e.0 = e.0.min(a.price);
        e.1 += a.vol_remaining;
    }
    let mut best: Option<Candidate> = None;
    for (station, (price, on_offer)) in by_station {
        let Some(system) = w.map.system_of(station) else {
            continue;
        };
        let Some((floor, route)) = FLOORS.iter().find_map(|&f| {
            w.map
                .route(w.from, system, f)
                .filter(|r| r.len() - 1 <= RUN_MAX_JUMPS as usize)
                .map(|r| (f, r))
        }) else {
            continue;
        };
        let jumps = u32::try_from(route.len() - 1).unwrap_or(u32::MAX);
        let one_way = route_risk(w.map, &route, w.hazards);
        let back: Vec<i64> = route.iter().rev().copied().collect();
        let risk = 1.0 - (1.0 - one_way) * (1.0 - route_risk(w.map, &back, w.hazards));
        let goods = price * w.units.min(on_offer).max(1) as f64;
        let mut options = vec![(Cover::None, risk, 0.0)];
        if w.escorts > 0 {
            options.push((
                Cover::Escort,
                risk * ESCORT_FACTOR,
                ESCORT_FEE_PER_JUMP * f64::from(jumps) * 2.0,
            ));
        }
        for (cover, risk, fee) in options {
            if risk > MAX_RISK {
                continue;
            }
            let net = w.gain + w.worth_x * price - goods - fee - risk * (goods + w.exposure);
            if net <= 0.0 {
                continue;
            }
            let c = Candidate {
                source: Source {
                    station,
                    system,
                    jumps,
                    price,
                    units: on_offer,
                },
                floor,
                risk,
                cover,
                net,
            };
            let better = best.is_none_or(|b| {
                c.net > b.net || (c.net == b.net && c.source.station < b.source.station)
            });
            if better {
                best = Some(c);
            }
        }
    }
    best
}

/// The map and floor a [`fetch`](crate::travel::fetch) of `c` flies.
pub fn trips_for<'a>(map: &'a Map, c: &Candidate) -> Trips<'a> {
    Trips {
        map,
        floor: c.floor,
    }
}

// ---- the closed-world clock ------------------------------------------------------

/// **How long until the first barge** in a closed world under these rules
/// (item 3 of the lane): a pure model over the ladder's numbers, in hours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    /// The Venture's income, ISK/h (PROG-MINE: the closed economy's
    /// scaled 43.7 k).
    pub venture_isk_h: f64,
    /// The barge original's price, ISK.
    pub original: f64,
    /// Hull plus Strip Miner I, ISK (PROG-MINE: 42.9 M for a Procurer).
    pub hull_and_laser: f64,
    /// Mineral units a barge's recipe takes (Megacyte, ...).
    pub rare_units: f64,
    /// A rare miner's yield of those units per hour.
    pub rare_per_h: f64,
    /// The rare mineral's price per unit.
    pub rare_price: f64,
    /// Jumps one way to the seller, and minutes a jump takes.
    pub jumps: f64,
    pub minutes_per_jump: f64,
    /// Hours the build job takes.
    pub build_h: f64,
}

/// The hours of each stage and the whole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeline {
    /// Saving for the original ([`crate::builder::may_climb`]'s cushion
    /// when a blueprint is already held) .
    pub save_original: f64,
    /// The run there and back.
    pub run: f64,
    /// The rare miners' hours to cut the recipe's minerals (in parallel
    /// with the saving, so it only adds what the saving does not cover).
    pub rare: f64,
    pub build: f64,
    /// Saving the hull and laser to fly the barge it builds is *not* in the
    /// total: the builder sells barges; the first barge exists when built.
    pub total: f64,
}

/// The model. The builder saves for the original, runs for it, and the
/// rare miners work meanwhile; the job installs when both are done.
pub fn first_barge(c: &Clock, cushion: f64) -> Timeline {
    let save_original = c.original * cushion / c.venture_isk_h.max(1.0);
    let run = 2.0 * c.jumps * c.minutes_per_jump / 60.0;
    let rare = c.rare_units / c.rare_per_h.max(1e-9);
    let ready = (save_original + run).max(rare);
    Timeline {
        save_original,
        run,
        rare,
        build: c.build_h,
        total: ready + c.build_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verbs::BookOrder;

    // Invented ids only.
    const HOME_SYS: i64 = 31_000_001;
    const HIGH2: i64 = 31_000_002;
    const LOW1: i64 = 31_000_003;
    const NULL1: i64 = 31_000_004;
    const NULL2: i64 = 31_000_005;
    const ST_HOME: i64 = 61_000_001;
    const ST_NULL: i64 = 61_000_004;
    const ST_HIGH: i64 = 61_000_002;
    const TYPE: i64 = 91_000_001;

    fn line(map: Map, a: i64, b: i64) -> Map {
        map.with_gate(a, a * 10 + b, b, b * 10 + a)
            .with_gate(b, b * 10 + a, a, a * 10 + b)
    }

    /// home - high - low - null - null2.
    fn world() -> Map {
        let m = Map::new();
        let m = line(m, HOME_SYS, HIGH2);
        let m = line(m, HIGH2, LOW1);
        let m = line(m, LOW1, NULL1);
        let m = line(m, NULL1, NULL2);
        m.with_security(HOME_SYS, 0.9)
            .with_security(HIGH2, 0.8)
            .with_security(LOW1, 0.3)
            .with_security(NULL1, -0.4)
            .with_security(NULL2, -0.6)
            .with_station(ST_HOME, HOME_SYS)
            .with_station(ST_HIGH, HIGH2)
            .with_station(ST_NULL, NULL2)
    }

    fn ask(station: i64, price: f64) -> BookOrder {
        BookOrder {
            order_id: 1,
            type_id: TYPE,
            station_id: station,
            price,
            vol_remaining: 1,
            bid: false,
        }
    }

    fn want<'a>(map: &'a Map, h: &'a Hazards, gain: f64, escorts: u32) -> Want<'a> {
        Want {
            map,
            from: HOME_SYS,
            home: ST_HOME,
            cap: None,
            wallet: 1.0e9,
            units: 1,
            gain,
            worth_x: 0.0,
            exposure: 50_000.0,
            hazards: h,
            escorts,
        }
    }

    #[test]
    fn the_band_sets_the_chance_and_an_unknown_system_counts_as_null() {
        assert_eq!(jump_death(Some(0.9)), DEATH_HIGH);
        assert_eq!(jump_death(Some(0.3)), DEATH_LOW);
        assert_eq!(jump_death(Some(-0.5)), DEATH_NULL);
        assert_eq!(jump_death(None), DEATH_UNKNOWN);
    }

    #[test]
    fn a_route_is_as_risky_as_the_systems_it_enters_and_hazards_add() {
        let m = world();
        let r = [HOME_SYS, HIGH2, LOW1, NULL1];
        let none = route_risk(&m, &r, &Hazards::none());
        let sum = DEATH_HIGH + DEATH_LOW + DEATH_NULL;
        assert!(none > 0.0 && none < sum, "{none}");
        let h = Hazards([(NULL1, 0.2)].into());
        assert!(route_risk(&m, &r, &h) > none + 0.15);
        assert_eq!(route_risk(&m, &[HOME_SYS], &h), 0.0);
    }

    #[test]
    fn a_seller_behind_null_is_reached_by_the_lowest_floor_that_routes() {
        let m = world();
        let b = Book {
            asks: vec![ask(ST_HIGH, 1_000.0), ask(ST_NULL, 2_000.0)],
            bids: vec![],
        };
        let h = Hazards::none();
        let c = plan(&b, &want(&m, &h, 1.0e7, 0)).expect("a run");
        // The high-sec seller wins on net (cheaper, safer) and routes at
        // the high floor.
        assert_eq!(c.source.station, ST_HIGH);
        assert_eq!(c.floor, SECURITY_FLOOR);
        let only_null = Book {
            asks: vec![ask(ST_NULL, 2_000.0)],
            bids: vec![],
        };
        let c = plan(&only_null, &want(&m, &h, 1.0e7, 0)).expect("a run");
        assert_eq!((c.source.station, c.source.jumps), (ST_NULL, 4));
        assert_eq!(c.floor, ANY_SECURITY, "null is on the way");
        assert_eq!(c.cover, Cover::None);
    }

    #[test]
    fn a_run_that_does_not_pay_is_not_made() {
        let m = world();
        let b = Book {
            asks: vec![ask(ST_NULL, 5_000_000.0)],
            bids: vec![],
        };
        let h = Hazards::none();
        // Gain below the price: no run.
        assert!(plan(&b, &want(&m, &h, 1_000_000.0, 0)).is_none());
        // A big gain pays.
        assert!(plan(&b, &want(&m, &h, 1.0e9, 0)).is_some());
    }

    #[test]
    fn too_much_danger_refuses_and_an_escort_can_make_it_pass() {
        let m = world();
        let b = Book {
            asks: vec![ask(ST_NULL, 1_000.0)],
            bids: vec![],
        };
        let h = Hazards([(NULL1, 0.3), (NULL2, 0.3)].into());
        let g = 1.0e8;
        assert!(plan(&b, &want(&m, &h, g, 0)).is_none(), "over MAX_RISK");
        let c = plan(&b, &want(&m, &h, g, 1)).expect("an escort cuts it");
        assert_eq!(c.cover, Cover::Escort);
        assert!(c.risk <= MAX_RISK);
    }

    #[test]
    fn the_wallet_cap_and_home_are_respected() {
        let m = world();
        let b = Book {
            asks: vec![ask(ST_HOME, 10.0), ask(ST_NULL, 2_000.0)],
            bids: vec![],
        };
        let h = Hazards::none();
        let mut w = want(&m, &h, 1.0e8, 0);
        w.wallet = 1_000.0;
        assert!(plan(&b, &w).is_none(), "cannot pay, home excluded");
        w.wallet = 1.0e9;
        w.cap = Some(1_000.0);
        assert!(plan(&b, &w).is_none(), "over the consumer's cap");
    }

    #[test]
    fn the_clock_adds_the_run_to_the_saving_and_the_rare_work_overlaps_it() {
        let c = Clock {
            venture_isk_h: 1_000.0,
            original: 10_000.0,
            hull_and_laser: 0.0,
            rare_units: 100.0,
            rare_per_h: 1.0,
            rare_price: 1.0,
            jumps: 10.0,
            minutes_per_jump: 3.0,
            build_h: 5.0,
        };
        let t = first_barge(&c, 3.0);
        assert_eq!(t.save_original, 30.0);
        assert_eq!(t.run, 1.0);
        assert_eq!(t.rare, 100.0);
        assert_eq!(t.total, 105.0, "the rare miners are the long pole");
        let fast = first_barge(
            &Clock {
                rare_units: 1.0,
                ..c
            },
            3.0,
        );
        assert_eq!(fast.total, 36.0);
    }

    /// **The measure** (item 3 of the lane), an ignored report: how long
    /// until the first barge in a closed world, from the bundle's own
    /// numbers. `PROG_NULL_BUNDLE=out/content BARGE_BP=<original type id>
    /// RARE_MINERAL=<mineral type id> RARE_ORE=<ore type id> ore volume
    /// per unit in RARE_ORE_M3` -- ids are the bundle's, so they come from
    /// the environment and nothing is committed.
    #[test]
    #[ignore = "reads the bundle: PROG_NULL_BUNDLE, BARGE_BP, RARE_MINERAL, RARE_ORE, RARE_ORE_M3"]
    fn report_the_hours_to_the_first_barge() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
        let dir = std::path::PathBuf::from(env("PROG_NULL_BUNDLE"));
        let (bp, mineral, ore): (i64, i64, i64) = (
            env("BARGE_BP").parse().unwrap(),
            env("RARE_MINERAL").parse().unwrap(),
            env("RARE_ORE").parse().unwrap(),
        );
        let ore_m3: f64 = env("RARE_ORE_M3").parse().unwrap();
        let rows = |f: &str| -> Vec<Vec<String>> {
            std::fs::read_to_string(dir.join(f))
                .unwrap()
                .lines()
                .filter(|l| !l.starts_with('#') && !l.is_empty())
                .map(|l| l.split('\t').map(str::to_owned).collect())
                .collect()
        };
        let price = rows("econ/prices.tsv")
            .into_iter()
            .find(|r| r[0] == bp.to_string())
            .map(|r| r[1].parse::<f64>().unwrap())
            .expect("a price for the original");
        let need = rows("industry/blueprints.tsv")
            .into_iter()
            .find(|r| r[0] == bp.to_string() && r[2] == "material" && r[3] == mineral.to_string())
            .map(|r| r[4].parse::<f64>().unwrap())
            .expect("the barge takes the mineral");
        let per_portion = rows("types/materials.tsv")
            .into_iter()
            .find(|r| r[0] == ore.to_string() && r[1] == mineral.to_string())
            .map(|r| r[2].parse::<f64>().unwrap())
            .expect("the ore yields it");
        // A Venture's modelled m3 an hour (PROG-MINE's table: 937 k ISK/h
        // at 321.6 ISK/m3), and a portion is 100 units.
        let m3_h = 937_000.0 / 321.6;
        let rare_per_h = m3_h / ore_m3 / 100.0 * per_portion;
        for (who, income) in [("builder prior", 35_000.0), ("Venture, closed", 43_700.0)] {
            for cushion in [1.0, 3.0] {
                let t = first_barge(
                    &Clock {
                        venture_isk_h: income,
                        original: price,
                        hull_and_laser: 0.0,
                        rare_units: need,
                        rare_per_h,
                        rare_price: 0.0,
                        jumps: 27.0,
                        minutes_per_jump: 3.0,
                        build_h: 12_000.0 / 3_600.0,
                    },
                    cushion,
                );
                println!(
                    "{who:16} cushion {cushion}: save {:.0} h, run {:.1} h, rare {:.1} h, \
                     total {:.0} h = {:.1} years",
                    t.save_original,
                    t.run,
                    t.rare,
                    t.total,
                    t.total / 8_766.0
                );
            }
        }
    }
}
