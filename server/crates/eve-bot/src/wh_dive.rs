//! **The wormhole diver** (Y-WH, series T3-WH-SPACE): a bot that leaves
//! k-space through a wormhole, harvests the Fullerite reservoirs of
//! J-space, and comes back out the way it went in -- every step a player
//! verb (`probe.rs`, `huffer.rs`, `verbs.rs`), against the server's
//! wormhole space (`eve_server::wormhole_sites`, `wormhole_jump`).
//!
//! # The trip (a gas dive)
//!
//! [`ROLE`] (career `wormhole_diver`) is the gas huffer's role asking for
//! [`Act::Dive`]; the trip itself is `huffer::run_via` under [`VIA`]:
//!
//! 1. **Dock to dock as a huffer**: board the Venture, fit the scoop and the
//!    probes, sell the last trip's gas, undock.
//! 2. **In** ([`dive_in`], called by [`gas_finder`] before the first site):
//!    scan the home system's signatures for a wormhole (scan strength 1908
//!    [V]), warp to it, read where it leads from its slim --
//!    `otherSolarSystemClass` [V key] is what the client's info window shows
//!    -- take a hole to C1-C6 only, approach to [`JUMP_RANGE`]
//!    (`menusvc.EnterWormhole` [V]) and call `wormholeMgr.WormholeJump`
//!    (`movementFunctions.RealEnterWormhole:644` [V]). The session change
//!    that follows is the jump (`jump::JUMP_DELAY`). Where the bot lands is
//!    remembered ([`Entered`]): the K162 is next to it.
//! 3. **In J-space**: the huffer's own finder ([`huffer::probe_finder`])
//!    scans for a gas signature; the harvest is the huffer's, with two
//!    differences ([`VIA`]): **a site is worked for [`SITE_BUDGET`] only**,
//!    shorter than the Sleepers' arrival delay at a reservoir
//!    (`site_spawn::DELAY_SPAWN`, the client's own `delay_spawn` flag on
//!    those spawn rows [V-data]), so the bot is gone before they come; and
//!    **a Sleeper on the grid ends the trip** ([`sleepers_near`]).
//! 4. **Out** ([`come_out`], `before_home`): scan the J-system's wormhole
//!    signatures, nearest to where the bot arrived first, warp, take a hole
//!    that leads to k-space (class 7-9), bind. The trip then flies home in
//!    the home system, where the static is.
//!
//! # Who dives
//!
//! The diver career always does. The **gas huffer** does when the market
//! pays ([`prefer`]): Fullerite's `P_ref` at least [`PREMIUM`] times the
//! dearest k-space gas the server prices; with no price for Fullerite yet
//! (the T3 pricing lane's, `t3-supply.md` lane 4) one huffer in
//! [`UNPRICED_ONE_IN`] dives, so a closed world's Fullerite starts to flow
//! before it has a price. All of it **[A]** policy.
//!
//! # What is not here
//!
//! Hacking in J-space (the relic and data cans are guarded the moment a
//! pilot warps in: the spawn rows have `delay_spawn` 0 [V-data], and an
//! unescorted frigate does not survive Sleepless sentries), Sleeper combat
//! and salvage (the combat bots' and the salvagers'), and fleets. See the
//! lane note's *Not done*.

use crate::brain::{self, Act, Memory, Observation};
use crate::huffer::{self, Via};
use crate::probe::{self, Kind, Signature};
use crate::roles::{RoleSpec, Summary};
use crate::{Conn, Error, Result};
use eve_marshal::PyValue;
use eve_server::celestial::WORMHOLE_SYSTEMS;
use eve_server::wormhole_sites::{self, CLASSES};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

// ---- the numbers ------------------------------------------------------------

/// How near a hole the bot jumps from: `menusvc.EnterWormhole` approaches to
/// 5,000 m (R-WH §2.1) **[V]**.
pub const JUMP_RANGE: f64 = 5_000.0;
/// Wormhole signatures tried to find a hole into J-space in one dive, and to
/// find the way out. **[A]** Falsified by divers that give up in a system
/// with a hole, or that spend a trip's time on k-space holes.
pub const MAX_HOLES: usize = 4;
/// How long a jump may take to show its session change: the stargate's delay
/// (`eve_server::jump::JUMP_DELAY`) and two beats of evidence.
pub const JUMP_PATIENCE: Duration =
    Duration::from_millis(eve_server::jump::JUMP_DELAY.as_millis() as u64 + 5_000);
/// **[A]** How long a landing waits for the hole's ball to show on the
/// grid. Falsified by a dive that finds a signature and sees no hole.
pub const HOLE_WAIT: Duration = Duration::from_secs(30);
/// **[A]** The longest a dive sits in J-space, whatever the sites hold: a
/// 5,000 m3 hold of Fullerite is four hours of one scoop's cycles, and the
/// holes live for hours. Falsified by divers that come home with a third
/// of a hold (too short) or whose hole has collapsed behind them (too long).
pub const DIVE_CEILING: Duration = Duration::from_secs(2 * 3_600);
/// **[A]** How long one reservoir is worked: the Sleepers' arrival delay
/// less two minutes. Falsified by a huffer shot at a reservoir.
pub const SITE_BUDGET: Duration = Duration::from_secs(wormhole_sites::ARRIVAL_DELAY_S as u64 - 120);
/// **[A]** A Sleeper this near (metres) is a threat: the Sleepers'
/// aggression range is 250 km [V-data, `spawns.tsv`], and a ship that has
/// seen one at all is better off gone. Falsified by divers that leave a
/// site a Sleeper only passes.
pub const THREAT_RANGE: f64 = 250_000.0;
/// **[A]** Fullerite must fetch this many times the dearest k-space gas
/// for a huffer to take the risk. Falsified by huffers that dive for no
/// gain or never dive when it pays.
pub const PREMIUM: f64 = 1.5;
/// **[A]** With no price for Fullerite, one huffer in this many dives, so
/// supply starts before the price exists. Falsified by Fullerite stocks
/// that never leave zero (more) or a huffer fleet that is all divers (fewer).
pub const UNPRICED_ONE_IN: u64 = 4;

/// The Fullerite types (`huffer::GAS_TYPES`' tail) **[V]**: group 711 of
/// J-space reservoirs only.
pub const FULLERITE: &[i64] = &[
    30_370, 30_371, 30_372, 30_373, 30_374, 30_375, 30_376, 30_377, 30_378,
];

/// How a dive differs from a home trip.
pub const VIA: Via = Via {
    ceiling: DIVE_CEILING,
    site_budget: Some(SITE_BUDGET),
    threat,
    before_home: come_out_hook,
};

// ---- the role ---------------------------------------------------------------

/// Dives into wormhole space for Fullerite, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "dive",
    greeting: "o7 -- diving for gas, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup: huffer::setup,
};

fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    huffer::docked_with(m, o, Act::Dive)
}

fn in_space(m: &mut Memory, o: &Observation) -> Vec<Act> {
    match &o.park {
        Some(park) => brain::go_home(m, o, park),
        None => vec![Act::Wait(brain::SHORT_WAIT)],
    }
}

fn banner(s: &Summary) -> String {
    format!(
        "{} summary: {}, sold {:.2} ISK, reconnects {}",
        s.name, s.phase, s.isk_sold, s.reconnects
    )
}

// ---- the pure rules ---------------------------------------------------------

/// Where a hole leads, by the class its slim names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leads {
    /// C1 to C6.
    JSpace,
    /// C1 up to the given class (T3-SALVAGE: a hull is sent only to the
    /// classes whose sites it can run).
    JSpaceTo(i64),
    /// High, low or null security (classes 7, 8, 9 [V constants]).
    KSpace,
}

impl Leads {
    pub fn matches(self, class: i64) -> bool {
        match self {
            Leads::JSpace => CLASSES.contains(&class),
            Leads::JSpaceTo(max) => CLASSES.contains(&class) && class <= max,
            Leads::KSpace => (7..=9).contains(&class),
        }
    }
}

/// One wormhole on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoleBall {
    pub ball: i64,
    /// Surface distance from the ship.
    pub gap: f64,
    /// The slim's `otherSolarSystemClass`.
    pub class: i64,
}

/// **The nearest hole that leads where we want**; ties to the lower id.
pub fn pick_hole(holes: &[HoleBall], leads: Leads) -> Option<HoleBall> {
    holes
        .iter()
        .filter(|h| leads.matches(h.class))
        .min_by(|a, b| a.gap.total_cmp(&b.gap).then(a.ball.cmp(&b.ball)))
        .copied()
}

/// `signatures` (target id, point) ordered nearest to `at` first; ties by
/// target id.
pub fn nearest_first(signatures: &[(String, [f64; 3])], at: [f64; 3]) -> Vec<String> {
    let mut v: Vec<(f64, &String)> = signatures.iter().map(|(t, p)| (dist(*p, at), t)).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)));
    v.into_iter().map(|(_, t)| t.clone()).collect()
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Whether any ball owned by the Sleeper faction is within `range`:
/// `balls` is `(ownerID, surface gap)`.
pub fn sleepers_near(balls: &[(i64, f64)], range: f64) -> bool {
    balls
        .iter()
        .any(|&(owner, gap)| owner == wormhole_sites::FACTION && gap <= range)
}

/// Whether `system` is J-space: the `31xxxxxx` ids (`celestial::
/// WORMHOLE_SYSTEMS`).
pub fn in_j_space(system: i64) -> bool {
    WORMHOLE_SYSTEMS.contains(&system)
}

/// **Whether a huffer should dive** this trip, from the server's reference
/// prices `refs` and the bot's `seed` (module header, *Who dives*).
pub fn prefer_from(refs: &BTreeMap<i64, f64>, seed: u64) -> bool {
    let price = |t: i64| refs.get(&t).copied().filter(|p| *p > 0.0);
    let fullerite: Vec<f64> = FULLERITE.iter().filter_map(|t| price(*t)).collect();
    if fullerite.is_empty() {
        return seed.is_multiple_of(UNPRICED_ONE_IN);
    }
    let best_k = huffer::GAS_TYPES
        .iter()
        .filter(|t| !FULLERITE.contains(t))
        .filter_map(|t| price(*t))
        .fold(None::<f64>, |m, p| Some(m.map_or(p, |m| m.max(p))));
    let mean = fullerite.iter().sum::<f64>() / fullerite.len() as f64;
    best_k.is_none_or(|k| mean >= k * PREMIUM)
}

/// [`prefer_from`] over the server's prices; a failed read says no.
pub fn prefer(c: &mut Conn, seed: u64) -> bool {
    match crate::pricing::Prices::read(c) {
        Ok(p) => prefer_from(p.refs(), seed),
        Err(_) => false,
    }
}

// ---- where the bot came in --------------------------------------------------

/// What a dive remembers of its way in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entered {
    /// The k-space system left.
    pub from: i64,
    /// The class entered.
    pub class: i64,
    /// Where the bot landed in J-space: the K162 is next to it.
    pub arrival: [f64; 3],
}

static ENTERED: Mutex<BTreeMap<i64, Entered>> = Mutex::new(BTreeMap::new());

fn remember(who: i64, e: Entered) {
    if let Ok(mut m) = ENTERED.lock() {
        m.insert(who, e);
    }
}

fn recall(who: i64) -> Option<Entered> {
    ENTERED.lock().ok()?.get(&who).copied()
}

fn forget(who: i64) {
    if let Ok(mut m) = ENTERED.lock() {
        m.remove(&who);
    }
}

// ---- on the wire ------------------------------------------------------------

impl Conn {
    /// **Jump through a wormhole** (Y-WH): `wormholeMgr.WormholeJump(
    /// itemID)` (`movementFunctions.RealEnterWormhole:644` [V]) on the hole's
    /// ball, with the ship inside [`JUMP_RANGE`]. The answer is `None`
    /// (`PerformSessionChange` discards it [V]); the evidence is the
    /// `solarsystemid` session change, [`JUMP_PATIENCE`] later.
    pub fn wormhole_jump(&mut self, hole: i64) -> Result<()> {
        let from = self.session().solar_system_id();
        let mut outcome = self.call("wormholeMgr", "WormholeJump", vec![PyValue::Int(hole)])?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("WormholeJump({hole}): {key}")));
        }
        self.await_session_change(&mut outcome, JUMP_PATIENCE)?;
        let now = self.session().solar_system_id();
        if outcome.session_changed && now.is_some() && now != from {
            return Ok(());
        }
        Err(Error::Refused(format!(
            "WormholeJump({hole}): no solarsystemid session change"
        )))
    }
}

/// The wormholes on the grid, from their slims.
pub fn holes_in(c: &mut Conn) -> Vec<HoleBall> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    world
        .balls()
        .filter_map(|b| {
            let class = b.slim.and_then(|s| s.wormhole_class)?;
            Some(HoleBall {
                ball: b.ball_id,
                gap: world.surface_distance(ego, b.ball_id)?,
                class,
            })
        })
        .collect()
}

/// `(ownerID, surface gap)` of every ball on the grid with an owner.
fn owned_balls(c: &mut Conn) -> Vec<(i64, f64)> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    world
        .balls()
        .filter_map(|b| {
            Some((
                b.slim.and_then(|s| s.owner_id)?,
                world.surface_distance(ego, b.ball_id)?,
            ))
        })
        .collect()
}

/// The threat check of [`VIA`]: a Sleeper within [`THREAT_RANGE`].
fn threat(c: &mut Conn) -> bool {
    in_j_space(c.session().solar_system_id().unwrap_or_default())
        && sleepers_near(&owned_balls(c), THREAT_RANGE)
}

fn here(c: &Conn) -> Result<i64> {
    if c.session().station_id().is_some() {
        return Err(Error::NotSent("dive: docked".to_owned()));
    }
    c.session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("dive: in space with no system".to_owned()))
}

fn position(c: &mut Conn) -> [f64; 3] {
    c.world_now()
        .ego_ball()
        .map(|b| b.motion.position)
        .unwrap_or([0.0; 3])
}

/// **Take a hole that leads `leads`** from the grid the bot is on: pick it
/// by its slim, approach to [`JUMP_RANGE`], jump, and bind the far system.
/// `Ok(false)`: no such hole here.
fn take(c: &mut Conn, leads: Leads) -> Result<Option<HoleBall>> {
    // The hole's room is laid out when the ship is near it (the next
    // `wormhole::arrivals` look), so its ball is not there at the instant of
    // landing: read pushes until it shows or the grid has had its say.
    let deadline = std::time::Instant::now() + HOLE_WAIT;
    let hole = loop {
        let holes = holes_in(c);
        if let Some(h) = pick_hole(&holes, leads) {
            break h;
        }
        // A hole that leads elsewhere is an answer; an empty grid may not be.
        if !holes.is_empty() || std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        c.pump(Duration::from_millis(250))?;
    };
    // A player picks the probes up before a jump: out in the old system
    // they are lost to the launcher in the new one. Best effort.
    let _ = probe::recover_all(c);
    probe::reach(c, hole.ball, JUMP_RANGE)?;
    c.wormhole_jump(hole.ball)?;
    probe::bound(c)?;
    Ok(Some(hole))
}

/// **In** (module header, step 2): `Ok(None)` when the system holds no
/// scannable hole into J-space, or the bot is there already.
pub fn dive_in(c: &mut Conn) -> Result<Option<Entered>> {
    dive_in_where(c, Leads::JSpace, &|_, _| true)
}

/// **In, by a hole that leads `leads`, to a system `accept` takes**
/// (T3-SALVAGE): the way [`dive_in`] goes, and a landing `accept` refuses
/// (`accept(landed system, entered)`) is left again by the hole the bot came
/// through and the next signature is tried. An exit that cannot be found is
/// an error: the bot is in J-space.
pub fn dive_in_where(
    c: &mut Conn,
    leads: Leads,
    accept: &dyn Fn(i64, &Entered) -> bool,
) -> Result<Option<Entered>> {
    let from = here(c)?;
    if in_j_space(from) {
        return Ok(None);
    }
    let mut scanner = match probe::Scanner::open(c) {
        Ok(s) => s,
        Err(Error::NotSent(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut tried: Vec<String> = Vec::new();
    while tried.len() < MAX_HOLES {
        let Some(sig) = probe::find_with(c, &mut scanner, |t, k| {
            !tried.iter().any(|x| x == t) && k.is_none_or(|k| k == Kind::Wormhole)
        })?
        else {
            return Ok(None);
        };
        tried.push(sig.target_id.clone());
        probe::warp_to(c, &sig)?;
        let Some(hole) = take(c, leads)? else {
            // A hole to k-space (or a signature that is no hole): the next.
            continue;
        };
        let landed = here(c)?;
        let e = Entered {
            from,
            class: hole.class,
            arrival: position(c),
        };
        println!(
            "[dive] {from} -> {landed} (class {}), arrived at {:?}",
            e.class, e.arrival
        );
        if accept(landed, &e) {
            return Ok(Some(e));
        }
        if !come_out(c, Some(e.arrival))? {
            return Err(Error::Refused(
                "dive: no hole out of an unwanted J-space system".to_owned(),
            ));
        }
    }
    Ok(None)
}

/// **Out** (module header, step 4): `Ok(true)` once the bot is in k-space.
/// Nothing to do in k-space (`Ok(true)`); `Ok(false)` when no hole out was
/// found.
pub fn come_out(c: &mut Conn, arrival: Option<[f64; 3]>) -> Result<bool> {
    let system = here(c)?;
    if !in_j_space(system) {
        return Ok(true);
    }
    let at = arrival.unwrap_or_else(|| position(c));
    let mut scanner = match probe::Scanner::open(c) {
        Ok(s) => s,
        Err(Error::NotSent(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    // Every wormhole signature of the system, scanned down.
    let mut found: Vec<Signature> = Vec::new();
    while found.len() < MAX_HOLES {
        let Some(sig) = probe::find_with(c, &mut scanner, |t, k| {
            !found.iter().any(|s| s.target_id == t) && k.is_none_or(|k| k == Kind::Wormhole)
        })?
        else {
            break;
        };
        found.push(sig);
    }
    let points: Vec<(String, [f64; 3])> = found
        .iter()
        .map(|s| (s.target_id.clone(), s.position))
        .collect();
    for target in nearest_first(&points, at) {
        let Some(sig) = found.iter().find(|s| s.target_id == target) else {
            continue;
        };
        probe::warp_to(c, sig)?;
        if take(c, Leads::KSpace)?.is_some() {
            println!("[dive] out: {system} -> {}", here(c)?);
            return Ok(true);
        }
    }
    Ok(false)
}

/// The `before_home` of [`VIA`]: out of J-space, by the remembered arrival.
/// An exit that fails is an error: the trip is not docked and the driver
/// reports it.
fn come_out_hook(c: &mut Conn) -> Result<()> {
    let who = c.session().char_id().unwrap_or_default();
    let ok = come_out(c, recall(who).map(|e| e.arrival))?;
    if !ok {
        return Err(Error::Refused(
            "dive: no hole out of J-space was found".to_owned(),
        ));
    }
    forget(who);
    Ok(())
}

// ---- the trip ---------------------------------------------------------------

/// **The finder of a gas dive**: before the first site, in k-space, dive in;
/// then the huffer's own finder. In J-space with no gas signature left it
/// finds nothing, and the trip goes out.
pub fn gas_finder(c: &mut Conn, tried: &[String]) -> Result<Option<String>> {
    let system = here(c)?;
    if !in_j_space(system) {
        let who = c.session().char_id().unwrap_or_default();
        match dive_in(c)? {
            Some(e) => remember(who, e),
            None => return Ok(None),
        }
    }
    huffer::probe_finder(c, tried)
}

/// **One gas dive, dock to dock** (module header).
pub fn run_gas(c: &mut Conn, name: &str, seed: u64) -> Result<huffer::Trip> {
    huffer::run_via(c, gas_finder, name, seed, &VIA)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented balls and ids; the classes are the rule's [V] constants.
    fn hole(ball: i64, gap: f64, class: i64) -> HoleBall {
        HoleBall { ball, gap, class }
    }

    #[test]
    fn the_nearest_hole_that_leads_where_we_want_is_picked() {
        let holes = [
            hole(11, 300.0, 8),
            hole(12, 900.0, 3),
            hole(13, 100.0, 5),
            hole(14, 100.0, 4),
        ];
        assert_eq!(pick_hole(&holes, Leads::JSpace).map(|h| h.ball), Some(13));
        assert_eq!(pick_hole(&holes, Leads::KSpace).map(|h| h.ball), Some(11));
        assert_eq!(
            pick_hole(&[hole(1, 1.0, 12)], Leads::JSpace),
            None,
            "Thera is not C1-C6"
        );
        assert_eq!(pick_hole(&[], Leads::KSpace), None);
        for class in 1..=6 {
            assert!(Leads::JSpace.matches(class) && !Leads::KSpace.matches(class));
        }
        for class in 7..=9 {
            assert!(Leads::KSpace.matches(class) && !Leads::JSpace.matches(class));
        }
    }

    #[test]
    fn signatures_come_nearest_the_arrival_first() {
        let sigs = vec![
            ("BBB-222".to_owned(), [9.0e6, 0.0, 0.0]),
            ("AAA-111".to_owned(), [1.0e6, 0.0, 0.0]),
            ("CCC-333".to_owned(), [0.0, 5.0e6, 0.0]),
        ];
        assert_eq!(
            nearest_first(&sigs, [0.0; 3]),
            ["AAA-111", "CCC-333", "BBB-222"]
        );
        assert_eq!(
            nearest_first(&sigs, [9.1e6, 0.0, 0.0]),
            ["BBB-222", "AAA-111", "CCC-333"]
        );
        assert!(nearest_first(&[], [0.0; 3]).is_empty());
    }

    #[test]
    fn only_a_near_sleeper_is_a_threat() {
        let s = wormhole_sites::FACTION;
        assert!(sleepers_near(&[(1, 10.0), (s, THREAT_RANGE)], THREAT_RANGE));
        assert!(!sleepers_near(&[(s, THREAT_RANGE + 1.0)], THREAT_RANGE));
        assert!(!sleepers_near(&[(1, 10.0), (2, 10.0)], THREAT_RANGE));
        assert!(!sleepers_near(&[], THREAT_RANGE));
    }

    #[test]
    fn j_space_is_the_31_million_ids() {
        assert!(in_j_space(31_000_001) && in_j_space(31_999_999));
        assert!(!in_j_space(30_000_001) && !in_j_space(32_000_001));
    }

    /// The site budget fits inside the Sleepers' arrival delay, and the
    /// dive is longer than one site.
    #[test]
    fn a_site_is_left_before_the_sleepers_come() {
        assert!(SITE_BUDGET.as_secs() < wormhole_sites::ARRIVAL_DELAY_S as u64);
        assert!(SITE_BUDGET < DIVE_CEILING);
    }

    #[test]
    fn the_market_decides_who_dives() {
        let k = huffer::GAS_TYPES[0];
        let f = FULLERITE[0];
        let refs = |pairs: &[(i64, f64)]| pairs.iter().copied().collect::<BTreeMap<_, _>>();
        // Fullerite dearer than the premium: dive, whatever the seed.
        let rich = refs(&[(k, 100.0), (f, 100.0 * PREMIUM)]);
        assert!(prefer_from(&rich, 1) && prefer_from(&rich, 2));
        // Cheaper: stay.
        let poor = refs(&[(k, 100.0), (f, 100.0 * PREMIUM - 1.0)]);
        assert!(!prefer_from(&poor, 1) && !prefer_from(&poor, 0));
        // Fullerite priced, nothing else: dive.
        assert!(prefer_from(&refs(&[(f, 5.0)]), 1));
        // A zero price is no price.
        assert!(!prefer_from(&refs(&[(f, 0.0)]), 1));
        // No Fullerite price: one in UNPRICED_ONE_IN, by seed.
        let none = refs(&[(k, 100.0)]);
        let divers = (0..UNPRICED_ONE_IN * 10)
            .filter(|s| prefer_from(&none, *s))
            .count();
        assert_eq!(divers as u64, 10);
    }

    #[test]
    fn fullerite_is_nine_gas_types_the_huffer_already_scoops() {
        assert_eq!(FULLERITE.len(), 9);
        for t in FULLERITE {
            assert!(huffer::is_gas(*t), "{t}");
        }
    }

    #[test]
    fn the_diver_asks_for_a_dive_not_a_huff() {
        use crate::brain::ActKind;
        let acts = [Act::Dive];
        assert_eq!(acts[0].kind(), ActKind::Dive);
        assert_eq!(ROLE.name, "dive");
    }

    #[test]
    fn what_a_dive_remembers_is_kept_per_character() {
        let e = Entered {
            from: 1,
            class: 3,
            arrival: [1.0, 2.0, 3.0],
        };
        remember(-4_242, e);
        assert_eq!(recall(-4_242), Some(e));
        assert_eq!(recall(-4_243), None);
        forget(-4_242);
        assert_eq!(recall(-4_242), None);
    }
}
