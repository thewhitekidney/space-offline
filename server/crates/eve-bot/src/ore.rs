//! **Ore anomalies and mining drones** (R-SOCIETY §1 row 1, stage B packet
//! SOC-B10): the miner's two additions to its belt loop.
//!
//! **Ore anomalies.** Ore sites are anomalies (archetype 27 [V]): the
//! Agency lists them with no probing (`dungeonInstanceCacheMgr.
//! GetOreAnomalyInstances` [V], the call `service/sites.rs` answers), and a
//! site's rocks are ordinary rocks of the field (`eve_server::site`), so
//! the miner's own rules (claim, approach, lock, mine) work there unchanged.
//! Once a trip, when the park has no rock, the miner asks for one
//! [`OreAct::Site`] before it looks for a belt ([`to_an_anomaly`]): read
//! this system's rows, pick one by seed ([`pick_site`]), warp to it. Then
//! it waits [`brain::BELT_WAITS`] turns for the site's rocks, as at a belt;
//! no site, or none of its rocks, and it goes to a belt as before.
//!
//! **Mining drones.** A player with mining drones in the bay launches them
//! at the rock the laser is on. Docked, a miner with mining drones
//! ([`MINING_DRONES`]) in its hangar moves them into the drone bay once a
//! visit ([`OreAct::Stock`]; a hull with no bay takes none). In space, once
//! the laser runs on the claim, one [`OreAct::Mine`] launches what the bay
//! holds (`ship.LaunchDrones` [V]) and orders them onto the rock
//! (`entity.CmdMineRepeatedly(droneIDs, targetID)`,
//! `droneFunctions.MineRepeatedly:175` [V]). A new claim re-orders the
//! drones already out. Going home recalls them first (`entity.CmdReturnBay`
//! [V], [`OreAct::Recall`]) and waits a little for them to come aboard.
//! The server (`eve_server::drones`, `eve_server::mining::drone_cycle`)
//! puts their ore in the ship's hold at the end of each cycle.
//!
//! **Where the hulls are.** The miner's Merlin has no drone bay
//! (`droneCapacity` 0 [V-data]); the rookie ships have 5 m3, one light
//! drone [V-data]. So today drones fly from a closed-economy miner's rookie
//! ship, and from any hull a later ladder gives the miner (Venture,
//! barges). See the lane note.
//!
//! Every rule here is **[A] policy** -- what a player would do -- except the
//! client's own calls and ids, cited [V].

use crate::brain::{self, Act, Memory, ParkView};
use crate::site_runner::read_rows;
use crate::{Conn, Error, ItemRow, Result};
use eve_marshal::PyValue;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// The light mining drones **[V-data]** (group 101, default effect 17,
/// 5 m3 each): Civilian Mining Drone, Mining Drone I, Mining Drone II and
/// 'Augmented' Mining Drone. The bot ships no type table, so a row is a
/// mining drone by its type id. The 10 m3 Harvester and the 1,100 m3
/// Excavators are left out (see [`DRONE_VOLUME`]).
pub const MINING_DRONES: &[i64] = &[1_202, 10_246, 10_250, 43_694];
/// Mining Drone I **[V-data]**: what the miner's kit buys
/// ([`crate::acquire::MINER_KIT`]).
pub const MINING_DRONE_I: i64 = 10_246;
/// The **Drones** skill **[V-data]**: one drone in space per level
/// (`eve_server::drones::SKILL_DRONES`); without it nothing launches.
pub const SKILL_DRONES: i64 = 3_436;
/// One light mining drone's volume, m3 **[V-data]** (every type in
/// [`MINING_DRONES`]).
pub const DRONE_VOLUME: f64 = 5.0;
/// `const.flagDroneBay` **[V]**.
pub const FLAG_DRONE_BAY: i64 = 87;
/// `droneCapacity` (283), the hull's drone bay in m3 **[V]**.
pub const ATTR_DRONE_CAPACITY: i64 = 283;

/// `{solarSystemID: [row, ...] | n}`: ore-anomaly rows for the caller's
/// own system **[V]** (`service/sites.rs`).
pub const ROWS_METHOD: &str = "GetOreAnomalyInstances";
/// The entity moniker's service: `Moniker('entity',
/// session.solarsystemid2)` (`eveMoniker.GetEntityAccess:92-95`) **[V]**.
pub const ENTITY: &str = "entity";

/// How often this system's rows are read before it counts as having none.
/// **[A]** the ice miner's ([`crate::ice::ROW_LOOKS`]).
pub const ROW_LOOKS: u32 = crate::ice::ROW_LOOKS;
/// The wait between those reads. **[A]**
pub const ROW_WAIT: Duration = crate::ice::ROW_WAIT;
/// How long a recall waits for the drones to come aboard before the miner
/// leaves without them. **[A]** a light drone at 400 m/s comes back from
/// its 5 km reach in well under this; falsified by miners that warp off
/// with their drones still flying home (too short), or sit for long.
pub const RECALL_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(250);
/// The longest a warp to a site is waited for. **[A]** the couriers'.
const CEILING: Duration = crate::courier::TRAVEL_CEILING;

/// Whether `type_id` is one of [`MINING_DRONES`].
pub fn is_mining_drone(type_id: i64) -> bool {
    MINING_DRONES.contains(&type_id)
}

/// One trip's ore-anomaly step, in [`Memory::ore_site`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SiteStep {
    /// Not looked for yet this trip.
    #[default]
    Untried,
    /// Asked for ([`OreAct::Site`]): waiting for its rocks.
    Asked,
    /// Done with anomalies this trip: belts from here on.
    Done,
}

/// One ore step, docked or in space: the driver's arm is one [`act`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OreAct {
    /// Read this system's ore anomalies, pick one, warp to it.
    Site,
    /// Docked: mining drones from the hangar into the drone bay.
    Stock,
    /// Launch the bay's mining drones (if none are out) and order every one
    /// out onto this rock.
    Mine(i64),
    /// Recall the drones out and wait for them to come aboard.
    Recall,
    /// Compress the ore in the holds at a fleet mate's (or the ship's own)
    /// compressor (PROG-MINE): `inSpaceCompressionMgr.CompressItemInSpace`.
    Compress,
    /// Compress again, then move the ore into this ship's fleet hangar
    /// (FLEET-HANGAR, [`crate::fleet_hangar::drop_into`]).
    Drop(i64),
}

impl OreAct {
    /// The act line.
    pub fn describe(self) -> (String, Vec<i64>) {
        match self {
            OreAct::Site => ("ore anomaly: Agency, pick, warp".to_owned(), vec![]),
            OreAct::Stock => ("mining drones: hangar -> drone bay".to_owned(), vec![]),
            OreAct::Mine(rock) => (
                format!("LaunchDrones + CmdMineRepeatedly -> {rock}"),
                vec![rock],
            ),
            OreAct::Recall => ("CmdReturnBay".to_owned(), vec![]),
            OreAct::Compress => (
                "inSpaceCompressionMgr.CompressItemInSpace".to_owned(),
                vec![],
            ),
            OreAct::Drop(ship) => (
                format!("Add(flag=155) -> fleet hangar of {ship}"),
                vec![ship],
            ),
        }
    }
}

// ---- the brain's rules (pure) ----------------------------------------------

/// **No rock in the park: try an ore anomaly first**, once a trip. `None`:
/// the policy has no anomalies, or this trip's anomaly is done -- the
/// caller goes on to a belt ([`brain::to_a_belt`]).
pub fn to_an_anomaly(m: &mut Memory, park: &ParkView) -> Option<Vec<Act>> {
    if !m.policy.ore_anomalies {
        return None;
    }
    if park.ego_warping {
        return Some(vec![Act::Wait(brain::SHORT_WAIT)]);
    }
    match m.ore_site {
        SiteStep::Untried => {
            m.ore_site = SiteStep::Asked;
            m.belt_waits = 0;
            Some(vec![Act::Ore(OreAct::Site)])
        }
        SiteStep::Asked if m.belt_waits < brain::BELT_WAITS => {
            m.belt_waits += 1;
            Some(vec![Act::Wait(brain::SHORT_WAIT)])
        }
        SiteStep::Asked | SiteStep::Done => {
            if m.ore_site == SiteStep::Asked {
                m.belt_waits = 0;
            }
            m.ore_site = SiteStep::Done;
            None
        }
    }
}

/// **Docked: stock the drone bay**, once a visit, when the hangar holds a
/// mining drone. `None` otherwise.
pub fn stock_first(m: &mut Memory, hangar: &[ItemRow]) -> Option<Vec<Act>> {
    if !m.policy.mining_drones || m.drones_stocked {
        return None;
    }
    if !hangar.iter().any(|r| is_mining_drone(r.type_id)) {
        return None;
    }
    m.drones_stocked = true;
    Some(vec![Act::Ore(OreAct::Stock)])
}

/// **The laser runs on `rock`: put the drones on it too.** Once a trip
/// when none are out (the first try launches what the bay holds, if
/// anything), and again whenever drones are out on another rock. `None`
/// when there is nothing to do.
pub fn drones_on(m: &mut Memory, rock: i64) -> Option<Vec<Act>> {
    if !m.policy.mining_drones || m.drone_rock == Some(rock) {
        return None;
    }
    if m.drones_tried && m.drones.is_empty() {
        return None;
    }
    m.drones_tried = true;
    m.drone_rock = Some(rock);
    Some(vec![Act::Ore(OreAct::Mine(rock))])
}

/// **Going home: drones first.** `Some` once while drones are out; the
/// arm forgets them whether or not they came aboard.
pub fn recall_first(m: &mut Memory) -> Option<Vec<Act>> {
    if m.drones.is_empty() {
        return None;
    }
    m.drone_rock = None;
    Some(vec![Act::Ore(OreAct::Recall)])
}

/// **The site to warp to**: among `sites` (this system's ore-anomaly rows),
/// in id order, the one at `seed % n`, so miners spread. A site is shared,
/// as a belt is: no claim board.
pub fn pick_site(sites: &[i64], seed: u64) -> Option<i64> {
    crate::site_runner::pick(sites, seed, None, &BTreeSet::new())
}

/// The drone ids a `LaunchDrones` answer gave for `stack`:
/// `{itemID: [droneID | (label, args), ...]}` (`eveMisc.LaunchFromShip:
/// 94-109`) **[V]**; refusals are dropped.
pub fn launched(answer: &PyValue, stack: i64) -> Vec<i64> {
    answer
        .dict_as_kv()
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| **k == PyValue::Int(stack))
        .flat_map(|(_, v)| match v {
            PyValue::List(v) | PyValue::Tuple(v) => v.clone(),
            _ => vec![],
        })
        .filter_map(|e| match e {
            PyValue::Int(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// How many light mining drones a bay of `capacity` m3 holding `held` of
/// them has room for.
pub fn room_for(capacity: f64, held: i64) -> i64 {
    let free = capacity - held as f64 * DRONE_VOLUME;
    if !free.is_finite() || free <= 0.0 {
        return 0;
    }
    (free / DRONE_VOLUME + 1e-9).floor() as i64
}

// ---- the arm (I/O) ----------------------------------------------------------

/// **One [`OreAct`]**, the driver's arm. Writes what it launched or
/// recalled into `m` ([`Memory::drones`]). A site not found, a bay with no
/// room or drones that did not launch are not errors: they are notes, and
/// the miner goes on with its laser and its belts.
pub fn act(c: &mut Conn, m: &mut Memory, op: OreAct) -> Result<()> {
    match op {
        OreAct::Site => {
            warp_to_site(c, m.seed)?;
            Ok(())
        }
        OreAct::Stock => stock(c).map(|_| ()),
        OreAct::Mine(rock) => {
            let ids = mine(c, &m.drones, rock)?;
            m.drones = ids;
            Ok(())
        }
        OreAct::Recall => {
            let out = std::mem::take(&mut m.drones);
            recall(c, &out)
        }
        OreAct::Compress => {
            let done = compress(c)?;
            m.compress.tried_at = (done == 0).then(Instant::now);
            m.compress.stacks += done;
            Ok(())
        }
        OreAct::Drop(ship) => crate::fleet_hangar::drop_into(c, m, ship).map(|_| ()),
    }
}

// ---- compression (PROG-MINE) ---------------------------------------------------

/// What the miner remembers of its compression tries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compress {
    /// When a try compressed nothing: no new try before
    /// [`COMPRESS_RETRY`] after it.
    pub tried_at: Option<Instant>,
    /// Stacks compressed, this run.
    pub stacks: u32,
}

/// How long after a try that compressed nothing the miner tries again
/// **[A]**: a fleet mate's compressor appears when he arrives, minutes.
/// Falsified by miners that go home full with a compressor on the grid
/// (too long) or that call the service every turn (too short).
pub const COMPRESS_RETRY: Duration = Duration::from_secs(5 * 60);
/// `inSpaceCompressionMgr` **[V]** (`itemcompression/client/itemCompression.py`).
pub const COMPRESSION_MGR: &str = "inSpaceCompressionMgr";
/// Facilities tried per stack, nearest first **[A]**.
pub const FACILITY_TRIES: usize = 4;
/// The holds the client offers to compress from **[V]**
/// (`HARVESTABLE_ORE_HOLDS`): the ones a miner's ore can sit in.
pub const ORE_FLAGS: [i64; 2] = [crate::FLAG_CARGO, brain::FLAG_MINING_HOLD];

/// **Hold full: try to compress before going home** (pure). `Some`: the act
/// to do instead of homing. Only a pilot in a fleet whose boss is on the
/// grid can have a facility ([`FACILITY_TRIES`] of them are tried, but the
/// server checks the fleet, so this is a cheap gate, not the rule), with ore
/// in its holds and no empty try inside [`COMPRESS_RETRY`].
pub fn on_hold_full(m: &Memory, o: &brain::Observation) -> Option<Vec<Act>> {
    let in_fleet_with_boss = o.fleet.is_some() && o.park.as_ref().is_some_and(|p| p.boss.is_some());
    if !in_fleet_with_boss {
        return None;
    }
    let has_ore = o
        .cargo
        .iter()
        .chain(o.ore_hold.iter())
        .any(|r| r.category_id == brain::CATEGORY_ASTEROID && !r.singleton);
    if !has_ore {
        return None;
    }
    if let Some(at) = m.compress.tried_at
        && o.now.saturating_duration_since(at) < COMPRESS_RETRY
    {
        return None;
    }
    Some(vec![Act::Ore(OreAct::Compress)])
}

/// **The facilities to try**, nearest first: the ship's own ball, then the
/// other pilots' ships on the grid (the server decides who is a fleet mate
/// with a compressor in range).
pub fn facilities(c: &mut Conn, own: i64) -> Vec<i64> {
    let world = c.world_now();
    let Some(ego) = world.get(own).copied() else {
        return vec![own];
    };
    let mut near: Vec<(f64, i64)> = world
        .balls()
        .filter(|b| b.ball_id != own && b.slim.is_some_and(|s| s.char_id.is_some()))
        .map(|b| {
            (
                eve_server::space_state::distance(ego.motion.position, b.motion.position),
                b.ball_id,
            )
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    std::iter::once(own)
        .chain(near.into_iter().map(|(_, id)| id))
        .take(FACILITY_TRIES)
        .collect()
}

/// **Compress what the holds carry**: each ore stack offered to each
/// facility in turn until one compresses it. The stacks compressed.
pub fn compress(c: &mut Conn) -> Result<u32> {
    let ship = c
        .session()
        .ship_id()
        .ok_or_else(|| Error::NotSent("ore: compress: no ship".to_owned()))?;
    let stacks: Vec<i64> = c
        .ship_items(ship, None)?
        .iter()
        .filter(|r| {
            r.category_id == brain::CATEGORY_ASTEROID
                && !r.singleton
                && ORE_FLAGS.contains(&r.flag_id)
        })
        .map(|r| r.item_id)
        .collect();
    let facilities = facilities(c, ship);
    let mut done = 0;
    for stack in stacks {
        for &ball in &facilities {
            let outcome = c.call(
                COMPRESSION_MGR,
                "CompressItemInSpace",
                vec![PyValue::Int(stack), PyValue::Int(ball)],
            )?;
            if let Some(key) = outcome.refusal() {
                return Err(Error::Refused(format!("ore: compress: {key}")));
            }
            if matches!(outcome.value(), Some(PyValue::Tuple(t)) if t.len() == 6) {
                done += 1;
                break;
            }
        }
    }
    Ok(done)
}

/// In space: read this system's ore-anomaly rows (a few times,
/// [`ROW_LOOKS`]), pick one ([`pick_site`]), warp to it and land. `None`
/// when the system lists none.
pub fn warp_to_site(c: &mut Conn, seed: u64) -> Result<Option<i64>> {
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("ore: in space with no system".to_owned()))?;
    let mut sites = Vec::new();
    for look in 0..ROW_LOOKS {
        let outcome = c.call(crate::site_runner::AGENCY, ROWS_METHOD, vec![])?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!("ore: {ROWS_METHOD}: {key}")));
        }
        sites = read_rows(&outcome.value().unwrap_or(PyValue::None), system)
            .iter()
            .map(|r| r.site_id)
            .collect();
        if !sites.is_empty() {
            break;
        }
        if look + 1 < ROW_LOOKS {
            settle_for(c, ROW_WAIT)?;
        }
    }
    let Some(site) = pick_site(&sites, seed) else {
        return Ok(None);
    };
    let outcome = c.warp_to_item(site, brain::MINING_STANDOFF)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("ore: warp to {site}: {key}")));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))?;
    Ok(Some(site))
}

/// Docked: move the hangar's mining drones into the active ship's drone
/// bay, as many as it has room for ([`room_for`]). The units moved; 0 for
/// a hull with no bay.
pub fn stock(c: &mut Conn) -> Result<i64> {
    let (Some(ship), Some(station)) = (c.session().ship_id(), c.session().station_id()) else {
        return Err(Error::NotSent(
            "ore: stock: not docked in a ship".to_owned(),
        ));
    };
    let capacity = c.ship_attribute(ship, ATTR_DRONE_CAPACITY)?.unwrap_or(0.0);
    let held: i64 = c
        .ship_items(ship, Some(FLAG_DRONE_BAY))?
        .iter()
        .map(|r| r.quantity.max(1))
        .sum();
    let mut room = room_for(capacity, held);
    let stacks: Vec<ItemRow> = c
        .hangar_items()?
        .into_iter()
        .filter(|r| is_mining_drone(r.type_id))
        .collect();
    let mut moved = 0;
    for s in stacks {
        if room <= 0 {
            break;
        }
        let n = room.min(s.quantity.max(1));
        let inventory = c.open_ship(ship)?;
        let outcome = c.call_on(
            &inventory,
            "Add",
            vec![PyValue::Int(s.item_id), PyValue::Int(station)],
            vec![
                (PyValue::Int(n), crate::bstr("qty")),
                (PyValue::Int(FLAG_DRONE_BAY), crate::bstr("flag")),
            ],
        )?;
        if let Some(key) = outcome.refusal() {
            return Err(Error::Refused(format!(
                "ore: Add({}, flag={FLAG_DRONE_BAY}): {key}",
                s.item_id
            )));
        }
        room -= n;
        moved += n;
    }
    Ok(moved)
}

/// In space: launch the bay's mining drones when none are `out`, then
/// order every drone out onto `rock`. Answers the drones now out.
pub fn mine(c: &mut Conn, out: &[i64], rock: i64) -> Result<Vec<i64>> {
    let mut ids = out.to_vec();
    if ids.is_empty() {
        ids = launch(c)?;
    }
    if ids.is_empty() {
        return Ok(ids);
    }
    let answer = entity(
        c,
        "CmdMineRepeatedly",
        vec![
            PyValue::List(ids.iter().map(|i| PyValue::Int(*i)).collect()),
            PyValue::Int(rock),
        ],
    )?;
    // `{droneID: (label, args)}` for the refused (`HandleMultipleCallError`
    // [V]): a drone refused is not ours to fly any more.
    let refused: Vec<i64> = answer
        .dict_as_kv()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, _)| match k {
            PyValue::Int(n) => Some(*n),
            _ => None,
        })
        .collect();
    ids.retain(|i| !refused.contains(i));
    Ok(ids)
}

/// `ship.LaunchDrones([(itemID, qty), ...], whoseBehalfID, ignoreWarning)`
/// on `Moniker('ship', (solarsystemid, groupSolarSystem))`
/// (`eveMisc.LaunchFromShip:79`) **[V]**, for every mining-drone stack in
/// the bay. The ids launched.
fn launch(c: &mut Conn) -> Result<Vec<i64>> {
    let Some(ship) = c.session().ship_id() else {
        return Err(Error::NotSent("ore: launch: no ship".to_owned()));
    };
    let stacks: Vec<ItemRow> = c
        .ship_items(ship, Some(FLAG_DRONE_BAY))?
        .into_iter()
        .filter(|r| is_mining_drone(r.type_id))
        .collect();
    if stacks.is_empty() {
        return Ok(Vec::new());
    }
    let params = c
        .session()
        .location_moniker()
        .ok_or_else(|| Error::NotSent("ore: launch: no location".to_owned()))?;
    let items = PyValue::List(
        stacks
            .iter()
            .map(|r| {
                PyValue::Tuple(vec![
                    PyValue::Int(r.item_id),
                    PyValue::Int(r.quantity.max(1)),
                ])
            })
            .collect(),
    );
    let answer = c.moniker_call(
        "ship",
        params,
        "LaunchDrones",
        vec![items, PyValue::None, PyValue::Bool(false)],
        vec![],
    )?;
    if let Some(key) = answer.refusal() {
        return Err(Error::Refused(format!("ore: LaunchDrones: {key}")));
    }
    let value = answer.value.unwrap_or(PyValue::None);
    Ok(stacks
        .iter()
        .flat_map(|r| launched(&value, r.item_id))
        .collect())
}

/// `entity.CmdReturnBay(droneIDs)` [V], then wait up to [`RECALL_WAIT`]
/// for every one to be back in the bay.
pub fn recall(c: &mut Conn, out: &[i64]) -> Result<()> {
    if out.is_empty() {
        return Ok(());
    }
    entity(
        c,
        "CmdReturnBay",
        vec![PyValue::List(
            out.iter().map(|i| PyValue::Int(*i)).collect(),
        )],
    )?;
    let Some(ship) = c.session().ship_id() else {
        return Ok(());
    };
    let until = Instant::now() + RECALL_WAIT;
    while Instant::now() < until {
        let aboard: Vec<i64> = c
            .ship_items(ship, Some(FLAG_DRONE_BAY))?
            .iter()
            .map(|r| r.item_id)
            .collect();
        if out.iter().all(|id| aboard.contains(id)) {
            return Ok(());
        }
        settle_for(c, Duration::from_secs(1))?;
    }
    Err(Error::Refused(format!(
        "ore: drones {out:?} not aboard after {RECALL_WAIT:?}; left in space"
    )))
}

/// One call on the entity moniker; its value (`{}` when all went).
fn entity(c: &mut Conn, method: &str, args: Vec<PyValue>) -> Result<PyValue> {
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent(format!("ore: {method}: not in space")))?;
    let answer = c.moniker_call(ENTITY, PyValue::Int(system), method, args, vec![])?;
    if let Some(key) = answer.refusal() {
        return Err(Error::Refused(format!("ore: {method}: {key}")));
    }
    Ok(answer.value.unwrap_or(PyValue::Dict(vec![])))
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

/// Read pushes for `d`.
fn settle_for(c: &mut Conn, d: Duration) -> Result<()> {
    let until = Instant::now() + d;
    while Instant::now() < until {
        c.pump(POLL)?;
    }
    Ok(())
}

/// Read pushes until `done` holds or [`CEILING`] passes.
fn settle(c: &mut Conn, mut done: impl FnMut(&mut Conn) -> bool) -> Result<()> {
    let deadline = Instant::now() + CEILING;
    while !done(c) {
        if Instant::now() >= deadline {
            return Err(Error::Refused(
                "ore: still warping after the ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Ball, Policy};
    use std::collections::BTreeMap;

    /// Invented ids.
    const ROCK_A: i64 = 8_200_000_001;
    const ROCK_B: i64 = 8_200_000_002;
    const STACK: i64 = 100_000_401;

    fn memory() -> Memory {
        let mut p = Policy::fast();
        p.ore_anomalies = true;
        p.mining_drones = true;
        Memory::new("ore-test", p)
    }

    fn park() -> ParkView {
        let ego = Ball {
            id: 100_000_400,
            position: [0.0; 3],
            radius: 50.0,
        };
        ParkView {
            ego,
            ego_warping: false,
            rocks: vec![],
            belts: vec![],
            station: None,
            boss: None,
            taken: BTreeMap::new(),
            hostiles: vec![],
            wrecks: vec![],
            ego_tether: None,
        }
    }

    fn row(item_id: i64, type_id: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 1,
            location_id: 2,
            flag_id: 4,
            quantity: 1,
            group_id: 0,
            category_id: 0,
            singleton: false,
        }
    }

    /// Once a trip: ask, wait for the rocks, then give up for belts; and a
    /// policy without anomalies never asks.
    #[test]
    fn an_anomaly_is_asked_for_once_then_waited_for_then_dropped() {
        let mut m = memory();
        let p = park();
        assert_eq!(
            to_an_anomaly(&mut m, &p),
            Some(vec![Act::Ore(OreAct::Site)])
        );
        for _ in 0..brain::BELT_WAITS {
            assert_eq!(
                to_an_anomaly(&mut m, &p),
                Some(vec![Act::Wait(brain::SHORT_WAIT)])
            );
        }
        assert_eq!(to_an_anomaly(&mut m, &p), None);
        assert_eq!(m.ore_site, SiteStep::Done);
        assert_eq!(m.belt_waits, 0, "the belt starts its own count");
        assert_eq!(to_an_anomaly(&mut m, &p), None, "once a trip");

        let mut off = Memory::new("ore-off", Policy::fast());
        assert_eq!(to_an_anomaly(&mut off, &p), None);
    }

    /// The drones go onto the laser's rock once; a trip with none out after
    /// that try asks no more; drones out follow a new claim.
    #[test]
    fn drones_follow_the_laser_and_none_means_none_again() {
        let mut m = memory();
        assert_eq!(
            drones_on(&mut m, ROCK_A),
            Some(vec![Act::Ore(OreAct::Mine(ROCK_A))])
        );
        assert_eq!(drones_on(&mut m, ROCK_A), None, "already on it");
        // The arm launched nothing: no drones this trip.
        assert_eq!(drones_on(&mut m, ROCK_B), None);

        let mut m = memory();
        drones_on(&mut m, ROCK_A);
        m.drones = vec![STACK];
        assert_eq!(
            drones_on(&mut m, ROCK_B),
            Some(vec![Act::Ore(OreAct::Mine(ROCK_B))])
        );
        assert_eq!(recall_first(&mut m), Some(vec![Act::Ore(OreAct::Recall)]));
        m.drones.clear();
        assert_eq!(recall_first(&mut m), None);
    }

    /// The bay is stocked once a visit, and only with mining drones about.
    #[test]
    fn the_bay_is_stocked_once_a_visit_when_the_hangar_has_drones() {
        let mut m = memory();
        assert_eq!(stock_first(&mut m, &[row(STACK, 34)]), None);
        let hangar = [row(STACK, MINING_DRONE_I)];
        assert_eq!(
            stock_first(&mut m, &hangar),
            Some(vec![Act::Ore(OreAct::Stock)])
        );
        assert_eq!(stock_first(&mut m, &hangar), None);
        let mut off = Memory::new("ore-off", Policy::fast());
        assert_eq!(stock_first(&mut off, &hangar), None);
    }

    #[test]
    fn a_site_is_picked_by_seed_in_id_order_and_room_is_whole_drones() {
        assert_eq!(pick_site(&[], 3), None);
        assert_eq!(pick_site(&[30, 10, 20], 4), Some(20));
        assert_eq!(room_for(5.0, 0), 1);
        assert_eq!(room_for(10.0, 1), 1);
        assert_eq!(room_for(12.0, 0), 2);
        assert_eq!(room_for(0.0, 0), 0);
        assert_eq!(room_for(5.0, 1), 0);
    }

    /// The launch answer's ids for one stack, refusals dropped.
    #[test]
    fn the_launch_answer_gives_the_stacks_drone_ids() {
        let answer = PyValue::Dict(vec![(
            PyValue::List(vec![
                PyValue::Int(STACK),
                PyValue::Tuple(vec![
                    crate::bstr("NoDroneManagementAbilitiesLeft"),
                    PyValue::Dict(vec![]),
                ]),
            ]),
            PyValue::Int(STACK),
        )]);
        assert_eq!(launched(&answer, STACK), vec![STACK]);
        assert!(launched(&answer, STACK + 1).is_empty());
    }

    // ---- compression (PROG-MINE) -------------------------------------------------

    fn full_hold_scene() -> brain::Observation {
        let mut o = brain::Observation::empty(Instant::now());
        o.park = Some(park());
        let mut ore = row(STACK, 1230);
        ore.category_id = brain::CATEGORY_ASTEROID;
        ore.singleton = false;
        ore.quantity = 500;
        o.ore_hold = vec![ore];
        o
    }

    /// A full hold in a fleet whose boss is here asks for a compression
    /// before going home; alone, with no boss on the grid, with no ore, or
    /// just after an empty try, it goes home as before.
    #[test]
    fn a_full_hold_in_a_fleet_tries_to_compress_before_going_home() {
        let m = memory();
        let mut o = full_hold_scene();
        assert_eq!(on_hold_full(&m, &o), None, "no fleet");
        o.fleet = Some(brain::FleetView {
            fleet_id: 7,
            boss: Some(90_000_001),
        });
        assert_eq!(on_hold_full(&m, &o), None, "the boss is not on the grid");
        o.park.as_mut().expect("park").boss = Some(Ball {
            id: 5,
            position: [0.0; 3],
            radius: 100.0,
        });
        assert_eq!(on_hold_full(&m, &o), Some(vec![Act::Ore(OreAct::Compress)]));
        let mut empty = o.clone();
        empty.ore_hold.clear();
        assert_eq!(on_hold_full(&m, &empty), None, "nothing to compress");
        let mut tried = memory();
        tried.compress.tried_at = Some(o.now);
        assert_eq!(on_hold_full(&tried, &o), None, "an empty try just now");
        tried.compress.tried_at = Some(o.now - COMPRESS_RETRY);
        assert!(on_hold_full(&tried, &o).is_some(), "and again later");
    }

    #[test]
    fn the_compress_act_describes_itself() {
        let (what, ids) = OreAct::Compress.describe();
        assert!(what.contains("CompressItemInSpace") && ids.is_empty());
        assert!(ORE_FLAGS.contains(&crate::FLAG_CARGO));
    }
}
