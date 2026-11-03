//! **The ice miner** (R-SOCIETY §1 row 2, stage B packet SOC-B1): the
//! miner's ice mode. A bot flies to an ice field, harvests ice, brings it
//! home, refines it and sells the ice products, the way a player does.
//!
//! [`ROLE`] is the `ice` row of [`crate::roles::ROLES`] and the
//! `ice_miner` career's role. Docked, in its hull, it asks for one
//! [`Act::Ice`], whose driver arm is one [`run`]; in space outside a run it
//! goes home ([`brain::go_home`]). A [`run`] is a whole trip, dock to dock,
//! every step a player verb:
//!
//! 1. **Ready** (docked): board the best ice hull held ([`ice_hull_in`]), fit up to
//!    [`WANT_HARVESTERS`] ice harvesters from the hangar ([`HARVESTERS`])
//!    and online them. With none, the run ends lacking one
//!    ([`Lack::Harvester`]); the next docked turn asks GM setup or the
//!    closed economy's [`Act::Acquire`] for [`ICE_KIT`].
//! 2. **Find an ice field** ([`choose_field`]): the Agency's ice-belt
//!    counts over every system (`dungeonInstanceCacheMgr.GetIceBeltsCount`
//!    [V], the Agency's own call). This system first; else, with a map, the
//!    nearest system that has one, has a station, and is within
//!    [`MAX_JUMPS`] at the bot's security floor, flown to by
//!    [`crate::travel::go`]. Ice belts are anomalies (archetype 28 [V]), so
//!    no probing: undock, read this system's rows (`GetIceBeltInstances`
//!    [V]), pick one by seed and warp to its `siteID`.
//! 3. **Harvest** by the huffer's [`huffer::next_step`] on the grid's ice
//!    rocks ([`ICE_TYPES`], by slim type): claim, approach inside the
//!    harvester's range, lock, start every harvester (`miningLaser`, repeat:
//!    an ice harvester's default effect is the laser's, 67 [V-data]), and
//!    read the stop pushes. A full hold ends the trip; a rock mined out is
//!    the next claim.
//! 4. **Home**, unload the ice into the hangar, and **refine and sell**:
//!    [`crate::builder::sell_goods`], the miner's own sale, which refines
//!    every asteroid-category stack (ice is category 25 [V-data]) and sells
//!    the products (ice products, group 423, are `FLOOR_GROUPS` [V]) into
//!    bids at the miner's least price. What no bid takes stays in the
//!    hangar for the builders' and the next trip's sale.
//!
//! **The hull** (ICE-HULL, #130): a mining barge, then an exhumer, as a
//! player flies, by the mining ladder ([`crate::mine_ladder::ice_hulls`]),
//! because an ice harvester fits only those (`canFitShipGroup` 463/543
//! [V-data]). The ice goes into the hull's mining hold, which takes the
//! ice group (`ShouldAllowAdd` for flag 134 [V]; `mining_hold::accepts`),
//! then spills into cargo; the bot empties **both** holds
//! ([`Conn::holds`]). A pilot without the hull's skills flies no ice hull
//! and mines no ice; [`crate::skill_plan::ICE_PLAN`] trains them, and the
//! closed economy buys the hull by the ladder ([`Act::Acquire`]).
//!
//! **The server side** is `eve_server::mining` unchanged: an Ice Harvester
//! I cuts `miningAmount` 1,000 m3 per 240 s cycle [V-data], one 1,000 m3
//! unit of ice ([`eve_server::mining::units_in`]). See the lane note.
//!
//! Every rule here is **[A] policy** -- what a player would do -- except
//! the client's own calls and ids, cited [V].

use crate::brain::{self, Act, Memory, Observation, StopReason};
use crate::courier::{Here, Move, next_move};
use crate::huffer::{Harvest, Step, next_step, pick_cloud};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::site_runner::{read_counts, read_rows};
use crate::travel::{Map, Trips};
use crate::{Conn, DOCK_RANGE, Error, Event, GodmaEffect, ItemRow, LockState, Result};
use eve_marshal::PyValue;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Ice Harvester I **[V-data]** (group 464, default effect 67, 1,000 m3 a
/// 240 s cycle, `maxRange` 10 km).
pub const ICE_HARVESTER_I: i64 = 16_278;
/// Ice Harvesting **[V-data]**: the harvester's `requiredSkill1` (182).
pub const SKILL_ICE_HARVESTING: i64 = 16_281;
/// Every module that cuts ice **[V-data]**: the ice harvesters (group 464:
/// Ice Harvester I and II, ORE Ice Harvester) and the ice mining lasers
/// (group 54), all default effect 67.
pub const HARVESTERS: &[i64] = &[16_278, 22_229, 28_752, 37_450, 37_451, 37_452];
/// The ice harvester's effect, as `dogmaIM.Activate` names it: the mining
/// laser's ([`crate::verbs::MINING_LASER`], `dogmaeffects[67]` [V-data]).
pub const MINING_LASER: (&str, i64) = crate::verbs::MINING_LASER;
/// `groupIce` **[V-data]** (465): a hold row of this group is ice.
pub const GROUP_ICE: i64 = 465;
/// Every ice type found in space **[V-data]** (group 465, the compressed
/// forms left out): the eight ices, the four IV-grades, Azure Ice and
/// Crystalline Icicle. The bot ships no type table, so a ball is ice by its
/// slim's type id.
pub const ICE_TYPES: &[i64] = &[
    16_262, 16_263, 16_264, 16_265, 16_266, 16_267, 16_268, 16_269, 17_975, 17_976, 17_977, 17_978,
    28_627, 28_628,
];
/// The ice products **[V-data]** (group 423): heavy water, liquid ozone,
/// helium, strontium clathrates, oxygen, nitrogen and hydrogen isotopes:
/// what refined ice gives and what the career's basket prices
/// ([`crate::signal`]).
pub const ICE_PRODUCTS: &[i64] = &[16_272, 16_273, 16_274, 16_275, 17_887, 17_888, 17_889];

/// Harvesters fitted at once. **[A]** a barge's or an exhumer's two high
/// slots ([`crate::mine_ladder::Hull::lasers`], 2 for every rung); falsified
/// by an ice miner idle with a free slot, or refused a fit.
pub const WANT_HARVESTERS: i64 = 2;

/// What the ice miner flies (E8's closed economy buys it, GM setup gives
/// it): [`WANT_HARVESTERS`] Ice Harvester I and Ice Harvesting. The hull is
/// the ladder's pick ([`crate::acquire::hulls_of`]); `hull` here is its
/// lowest rung, only so the order names its skills (the foreman's `ORCA`
/// is the same placeholder). The skill's own prerequisite (Mining IV) is
/// trained by [`crate::skill_plan::ICE_PLAN`].
pub const ICE_KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(crate::mine_ladder::PROCURER),
    modules: Some((ICE_HARVESTER_I, WANT_HARVESTERS)),
    ammo: None,
    skills: &[(SKILL_ICE_HARVESTING, 1)],
    more: &[],
};

/// The Agency **[V]** (`evedungeons/client/anomalyTracker`, C2 §3).
pub const AGENCY: &str = crate::site_runner::AGENCY;
/// `{solarSystemID: n}` over every system **[V]** (the Agency's ice
/// tracker; `service/sites.rs`).
pub const COUNT_METHOD: &str = "GetIceBeltsCount";
/// `{solarSystemID: [row, ...] | n}`: rows for the caller's own system
/// **[V]**.
pub const ROWS_METHOD: &str = "GetIceBeltInstances";

/// The farthest an ice miner flies for a field. **[A]** the errand's
/// reach ([`crate::travel::TRIP_MAX_JUMPS`]); falsified by ice miners that
/// idle at home with a field six jumps out, or spend a session in transit.
pub const MAX_JUMPS: u32 = crate::travel::TRIP_MAX_JUMPS;
/// How often the own system's rows are read before the field counts as
/// gone: a system's sites reach the tracker once a pilot is in it, so the
/// first read can be empty. **[A]** the plexer's [`crate::plexer::EMPTY_LOOKS`]
/// and a little more.
pub const ROW_LOOKS: u32 = 3;
/// The wait between those reads. **[A]**
pub const ROW_WAIT: Duration = Duration::from_secs(2);
/// How long a field's grid must be quiet before its rocks are counted.
/// **[A]**, the site runner's ([`crate::site_runner::SITE_SETTLE`]).
pub const SITE_SETTLE: Duration = crate::site_runner::SITE_SETTLE;
/// How far from the belt's anchor to land, metres. **[A]** the miner's
/// standoff ([`brain::MINING_STANDOFF`]).
pub const WARP_RANGE: f64 = brain::MINING_STANDOFF;
/// The longest one trip harvests. **[A]** a player's sitting: three units
/// at two harvesters is about twelve minutes; falsified by ice miners who
/// go home long before their hold is full, or never.
pub const HARVEST_CEILING: Duration = Duration::from_secs(60 * 60);
/// `maxRange` **[V]** (`dogmaattributes[54]`).
pub const ATTR_MAX_RANGE: i64 = 54;
/// The range kept from a rock while approaching it. **[A]**: the miner's
/// ([`brain::APPROACH_RANGE`]), well inside an Ice Harvester I's 10 km.
pub const APPROACH_RANGE: f64 = brain::APPROACH_RANGE;
/// Harvester stops that are neither "full" nor "mined out" before the trip
/// ends. **[A]** the huffer's.
pub const MAX_ODD_STOPS: u32 = crate::huffer::MAX_ODD_STOPS;
/// How long each read waits while something runs.
const POLL: Duration = Duration::from_millis(250);
/// How long a warp, an approach or a lock may take. **[A]** the courier's.
const CEILING: Duration = crate::courier::TRAVEL_CEILING;

/// The `MISSING` line of a trip that found no ice field.
pub const MISSING_FIELD: &str = "an ice field here or within reach (the Agency counts none)";

/// Mines ice, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "ice",
    greeting: "o7 -- out on the ice, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

// ---- what the bot remembers between trips -----------------------------------

/// What the last [`run`] of a bot left for its next docked turn, by bot
/// name (the huffer's [`crate::huffer::Note`] kind). Per process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Note {
    /// The last run had no harvester to fit.
    pub lacks_harvester: bool,
    /// The last run found no ice field: dwell the longer dwell.
    pub found_nothing: bool,
}

static NOTES: Mutex<BTreeMap<String, Note>> = Mutex::new(BTreeMap::new());

/// The note of bot `name`.
pub fn note_of(name: &str) -> Note {
    NOTES
        .lock()
        .map(|n| n.get(name).copied().unwrap_or_default())
        .unwrap_or_default()
}

/// Remember `note` for bot `name`.
pub fn set_note(name: &str, note: Note) {
    if let Ok(mut n) = NOTES.lock() {
        n.insert(name.to_owned(), note);
    }
}

// ---- the role's rules ---------------------------------------------------------

/// The rung of an ice hull in the ladder, best first.
fn rank_of(type_id: i64) -> usize {
    crate::mine_ladder::ice_hulls()
        .position(|h| h.type_id == type_id)
        .unwrap_or(usize::MAX)
}

/// **The best ice hull in a hangar** that `ok` allows: a mining barge or an
/// exhumer, the ladder's order.
pub fn ice_hull_in(hangar: &[ItemRow], ok: impl Fn(&ItemRow) -> bool) -> Option<&ItemRow> {
    hangar
        .iter()
        .filter(|r| crate::mine_ladder::is_ice_hull(r.type_id) && ok(r))
        .min_by_key(|r| rank_of(r.type_id))
}

/// Is `type_id` ice (a rock in space, and what it gives)?
pub fn is_ice(type_id: i64) -> bool {
    ICE_TYPES.contains(&type_id)
}

/// Is `type_id` a module that cuts ice?
pub fn is_harvester(type_id: i64) -> bool {
    HARVESTERS.contains(&type_id)
}

/// Ice in a hold or hangar: a stack of the ice group.
pub fn is_ice_row(row: &ItemRow) -> bool {
    !row.singleton && (row.group_id == GROUP_ICE || is_ice(row.type_id))
}

/// Docked: unload ice left in the hold, the hull, the harvester the last
/// run lacked, then one [`Act::Ice`] and a dwell.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.claim = None;
    m.homing = None;
    let ice: Vec<(i64, i64)> = o
        .cargo
        .iter()
        .chain(&o.ore_hold)
        .filter(|r| is_ice_row(r))
        .map(|r| (r.item_id, r.quantity))
        .collect();
    if !ice.is_empty() {
        return vec![Act::Unload(ice)];
    }
    let active = o.ship.map(|s| s.ship_id);
    let flying = o.ship.and_then(|s| s.type_id);
    let in_capsule = flying.is_none_or(|t| t == brain::CAPSULE);
    let hangar_hull = ice_hull_in(&o.hangar, |r| {
        Some(r.item_id) != active && !m.combat.untrained.contains(&r.type_id)
    });
    let flying_ice = flying.is_some_and(crate::mine_ladder::is_ice_hull);
    if !flying_ice && let Some(h) = hangar_hull {
        return vec![Act::Board(h.item_id)];
    }
    let note = note_of(&m.name);
    let hangar_harvester = o.hangar.iter().any(|r| is_harvester(r.type_id));
    if !flying_ice {
        // A rookie ship or a Merlin cannot fit a harvester: a barge first.
        return brain::gm_or_missing(
            m,
            true,
            !hangar_harvester && (in_capsule || note.lacks_harvester),
            false,
            "a mining barge or an exhumer for ice in the hangar",
        );
    }
    if note.lacks_harvester && !hangar_harvester {
        return brain::gm_or_missing(m, false, true, false, "an ice harvester in the hangar");
    }
    let rest = m.jitter(if note.found_nothing {
        m.policy.idle_dwell
    } else {
        m.policy.dwell
    });
    vec![Act::Ice, Act::Wait(rest)]
}

/// In space outside a run (a dropped connection mid-trip): home.
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

/// GM setup: the skill, and the hull and harvesters when lacking.
fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {SKILL_ICE_HARVESTING} 1")];
    if lacks.ship {
        // The ladder's lowest ice rung and the skills it asks (a GM's
        // setup only: the closed economy buys by the ladder).
        let rung = crate::mine_ladder::PROCURER;
        for (skill, level) in crate::mine_ladder::hull_of(rung).map_or(&[][..], |h| h.skills) {
            lines.push(format!("/giveskill {skill} {level}"));
        }
        lines.push(format!("/createitem {rung}"));
    }
    if lacks.module {
        lines.push(format!("/createitem {ICE_HARVESTER_I} {WANT_HARVESTERS}"));
    }
    lines
}

// ---- where the ice is, pure ---------------------------------------------------

/// Where [`choose_field`] sends the trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// This system has an ice belt.
    Here,
    /// The nearest system that has one: fly to `station` in `system`.
    There { system: i64, station: i64 },
    /// None here, and none within reach (or no map).
    Nowhere,
}

/// **The field to go to** (module header, step 2): this system when the
/// Agency counts an ice belt here; else, with `trips`, the nearest system
/// with a count, a station, and a route at the floor of at most
/// `max_jumps` (ties to the lower system id, then the lowest station id).
pub fn choose_field(
    here: i64,
    counts: &BTreeMap<i64, i64>,
    trips: Option<Trips<'_>>,
    max_jumps: u32,
) -> Field {
    if counts.get(&here).copied().unwrap_or(0) > 0 {
        return Field::Here;
    }
    let Some(Trips { map, floor }) = trips else {
        return Field::Nowhere;
    };
    nearest(map, here, counts, floor, max_jumps).map_or(Field::Nowhere, |(system, station)| {
        Field::There { system, station }
    })
}

fn nearest(
    map: &Map,
    here: i64,
    counts: &BTreeMap<i64, i64>,
    floor: f64,
    max_jumps: u32,
) -> Option<(i64, i64)> {
    counts
        .iter()
        .filter(|&(&s, &n)| n > 0 && s != here)
        .filter_map(|(&s, _)| {
            let station = map.stations_in(s).into_iter().min()?;
            let jumps = map.jumps(here, s, floor)?;
            (jumps <= max_jumps).then_some((jumps, s, station))
        })
        .min()
        .map(|(_, s, station)| (s, station))
}

/// **The belt to warp to**: among `sites` (this system's ice-belt rows),
/// in id order, the one at `seed % n`, so ice miners spread. An ice field
/// is shared, as players share one: there is no claim board.
pub fn pick_belt(sites: &[i64], seed: u64) -> Option<i64> {
    crate::site_runner::pick(sites, seed, None, &BTreeSet::new())
}

// ---- the run ------------------------------------------------------------------

/// Why a run could not go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lack {
    Hull,
    Harvester,
}

/// What one [`run`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trip {
    /// Set when the run never left: what it lacked.
    pub lacking: Option<Lack>,
    /// The system mined in, when it left home's for it.
    pub travelled_to: Option<i64>,
    /// The ice belt warped to.
    pub belt: Option<i64>,
    /// Harvest starts (one per claim).
    pub claims: u32,
    /// Ice units unloaded at home.
    pub units: i64,
    /// The trip ended on a full hold.
    pub full: bool,
    /// What the refinery made of the hangar's ice, if it refined.
    pub refined: Vec<(i64, i64)>,
    /// Why it did not refine, if it did not.
    pub not_refined: Option<String>,
    /// Stacks sold into bids.
    pub sold: usize,
    /// Wallet before and after the sale.
    pub before: f64,
    pub after: f64,
}

impl Trip {
    /// The act line's note.
    pub fn summary(&self) -> String {
        if let Some(l) = self.lacking {
            return format!("did not go out: no {l:?}");
        }
        let found = if self.belt.is_none() {
            format!("; MISSING {MISSING_FIELD}")
        } else {
            String::new()
        };
        let refine = match &self.not_refined {
            Some(why) => format!("not refined ({why})"),
            None => format!("refined {:?}", self.refined),
        };
        format!(
            "belt {:?}{}, claims {}, {} ice units home{}; {refine}, sold {} stacks; wallet {:.2} -> {:.2} ISK{found}",
            self.belt,
            self.travelled_to
                .map_or_else(String::new, |s| format!(" in {s}")),
            self.claims,
            self.units,
            if self.full { " (hold full)" } else { "" },
            self.sold,
            self.before,
            self.after
        )
    }
}

/// **One ice trip, dock to dock** (module header). `trips` lets it fly to a
/// field outside this system; `name` is the bot's ([`Note`]) and `seed` its
/// spread. Docked to start; docked at the same station at the end, unless
/// an error cut it short.
pub fn run(c: &mut Conn, trips: Option<Trips<'_>>, name: &str, seed: u64) -> Result<Trip> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("ice: not docked".to_owned()))?;
    let home_system = c
        .session()
        .solar_system_id2()
        .ok_or_else(|| Error::NotSent("ice: no system".to_owned()))?;
    let mut trip = Trip::default();
    let harvesters = match ready(c)? {
        Ok(h) => h,
        Err(lack) => {
            trip.lacking = Some(lack);
            set_note(
                name,
                Note {
                    lacks_harvester: lack == Lack::Harvester,
                    found_nothing: false,
                },
            );
            return Ok(trip);
        }
    };
    // Ice left in the hangar by a trip cut short: refine and sell it first.
    if c.hangar_items()?.iter().any(is_ice_row) {
        sell(c, &mut trip)?;
    }
    let range = c
        .module_row(harvesters[0])?
        .attribute(ATTR_MAX_RANGE)
        .unwrap_or(APPROACH_RANGE / huffer_margin());

    let counts = read_counts(&agency(c, COUNT_METHOD)?);
    let field = choose_field(home_system, &counts, trips, MAX_JUMPS);
    if let Field::There { system, station } = field
        && let Some(t) = trips
    {
        crate::travel::go(c, t, station)?;
        trip.travelled_to = Some(system);
    }
    if field != Field::Nowhere {
        leave(c)?;
        trip.belt = warp_to_belt(c, seed)?;
    }
    if trip.belt.is_some() {
        settle_for(c, SITE_SETTLE)?;
        let out = harvest(c, &harvesters, range, seed)?;
        trip.claims = out.claims;
        trip.full = out.full;
    }
    let found_nothing = trip.belt.is_none();
    if trip.travelled_to.is_some()
        && let Some(t) = trips
    {
        crate::travel::go(c, t, home)?;
    } else {
        fly_home(c, home)?;
    }
    // The ice sits in the mining hold first, then spills into cargo.
    if let Some(ship) = c.session().ship_id() {
        for row in c.holds(ship)? {
            if is_ice_row(&row) {
                trip.units += row.quantity;
                c.unload(row.item_id, None)?;
            }
        }
    }
    if trip.units > 0 {
        sell(c, &mut trip)?;
    }
    set_note(
        name,
        Note {
            lacks_harvester: false,
            found_nothing,
        },
    );
    Ok(trip)
}

fn huffer_margin() -> f64 {
    crate::huffer::RANGE_MARGIN
}

/// Docked: in the ice hull (or any hull, failing it), harvesters fitted
/// and online. `Ok(Err(lack))` for what is missing; `Ok(Ok(ids))` ready.
fn ready(c: &mut Conn) -> Result<std::result::Result<Vec<i64>, Lack>> {
    let hangar = c.hangar_items()?;
    let active = c.session().ship_id();
    let flying = hangar
        .iter()
        .find(|r| Some(r.item_id) == active)
        .map(|r| r.type_id);
    if !flying.is_some_and(crate::mine_ladder::is_ice_hull) {
        match ice_hull_in(&hangar, |_| true) {
            Some(h) => match c.board(h.item_id) {
                Ok(_) => {}
                // Too junior for it (the skill gate): no ice hull flown.
                Err(Error::Refused(_)) => return Ok(Err(Lack::Hull)),
                Err(e) => return Err(e),
            },
            None => return Ok(Err(Lack::Hull)),
        }
    }
    let mut fitted: Vec<i64> = c
        .fitted()?
        .into_iter()
        .filter(|r| is_harvester(r.type_id))
        .map(|r| r.item_id)
        .collect();
    let want = usize::try_from(WANT_HARVESTERS).unwrap_or(1);
    if fitted.len() < want {
        // FIT-REFUSE: only a harvester the hull takes (the ORE ice laser
        // names its own groups), or a refused one would block the rest.
        let rows = c.hangar_items()?;
        let flown = rows
            .iter()
            .find(|r| Some(r.item_id) == c.session().ship_id())
            .map(|r| r.type_id);
        let spare: Vec<i64> = rows
            .iter()
            .filter(|r| is_harvester(r.type_id))
            .filter(|r| flown.is_none_or(|h| crate::acquire::hull_takes(r.type_id, h)))
            .map(|r| r.item_id)
            .take(want - fitted.len())
            .collect();
        if !spare.is_empty() {
            match c.fit_auto(&spare) {
                Ok(_) | Err(Error::Refused(_)) => {}
                Err(e) => return Err(e),
            }
            fitted = c
                .fitted()?
                .into_iter()
                .filter(|r| is_harvester(r.type_id))
                .map(|r| r.item_id)
                .collect();
        }
    }
    if fitted.is_empty() {
        return Ok(Err(Lack::Harvester));
    }
    for &h in &fitted {
        if !c.is_online(h)? && crate::acquire::set_online_gated(c, h)?.is_err() {
            return Ok(Err(Lack::Harvester));
        }
    }
    Ok(Ok(fitted))
}

/// Refine the hangar's ice and sell the products: the miner's sale
/// ([`crate::builder::sell_goods`]). A refusal is noted, not an error.
fn sell(c: &mut Conn, trip: &mut Trip) -> Result<()> {
    let sold = match crate::builder::sell_goods(c) {
        Ok(s) => s,
        Err(Error::Refused(why) | Error::NotSent(why)) => {
            trip.not_refined = Some(why);
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if trip.before == 0.0 && trip.after == 0.0 {
        trip.before = sold.before;
    }
    trip.after = sold.after;
    trip.refined.extend(sold.refined);
    if sold.not_refined.is_some() {
        trip.not_refined = sold.not_refined;
    }
    trip.sold += sold.lines.len();
    Ok(())
}

/// One Agency call's value.
fn agency(c: &mut Conn, method: &str) -> Result<PyValue> {
    let outcome = c.call(AGENCY, method, vec![])?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("ice: {method}: {key}")));
    }
    Ok(outcome.value().unwrap_or(PyValue::None))
}

/// In space in the field's system: read its ice-belt rows (a few times,
/// [`ROW_LOOKS`]), pick one ([`pick_belt`]), warp to it and land. `None`
/// when the system lists none.
fn warp_to_belt(c: &mut Conn, seed: u64) -> Result<Option<i64>> {
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("ice: in space with no system".to_owned()))?;
    let mut sites = Vec::new();
    for look in 0..ROW_LOOKS {
        sites = read_rows(&agency(c, ROWS_METHOD)?, system)
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
    let Some(belt) = pick_belt(&sites, seed) else {
        return Ok(None);
    };
    let outcome = c.warp_to_item(belt, WARP_RANGE)?;
    if let Some(key) = outcome.refusal() {
        return Err(Error::Refused(format!("ice: warp to {belt}: {key}")));
    }
    settle(c, warping)?;
    settle(c, |c| !warping(c))?;
    Ok(Some(belt))
}

/// Undock and bind the system's park.
fn leave(c: &mut Conn) -> Result<()> {
    if c.session().station_id().is_some() {
        c.undock_repaired()?;
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("ice: in space with no system".to_owned()))?;
    if c.ballpark().is_none() {
        c.bind_ballpark(system)?;
    }
    settle(c, |c| c.world().has_state())
}

/// What one field's harvest did.
struct Harvested {
    claims: u32,
    full: bool,
}

/// **Harvest the grid's ice** by [`next_step`] until the hold is full, the
/// rocks are gone, the stops pile up, or the trip's ceiling passes. Every
/// harvester runs on the one claim.
fn harvest(c: &mut Conn, harvesters: &[i64], range: f64, seed: u64) -> Result<Harvested> {
    let started = Instant::now();
    let mut out = Harvested {
        claims: 0,
        full: false,
    };
    let (mut claim, mut locked) = (None::<i64>, false);
    let mut running: BTreeSet<i64> = BTreeSet::new();
    let mut odd = 0;
    while started.elapsed() < HARVEST_CEILING && odd < MAX_ODD_STOPS {
        let rocks = rocks_in(c);
        if claim.is_some_and(|id| !rocks.iter().any(|(r, _)| *r == id)) {
            (claim, locked) = (None, false);
            running.clear();
        }
        let h = Harvest {
            warping: warping(c),
            hold_full: out.full,
            clouds: rocks.len(),
            gap: claim.and_then(|id| rocks.iter().find(|(r, _)| *r == id).map(|(_, g)| *g)),
            range,
            locked,
            active: running.len() == harvesters.len(),
        };
        match next_step(&h) {
            Step::Home | Step::NextSite => break,
            Step::Claim => {
                claim = pick_cloud(seed, &rocks, &BTreeSet::new());
            }
            Step::Approach => {
                let id = claim.unwrap_or_default();
                c.follow(id, APPROACH_RANGE)?;
                settle(c, |c| {
                    rocks_in(c)
                        .iter()
                        .find(|(rid, _)| *rid == id)
                        .is_none_or(|(_, g)| *g <= range * huffer_margin())
                })?;
            }
            Step::Lock => {
                let id = claim.unwrap_or_default();
                locked = match c.lock_target(id)? {
                    LockState::Locked => true,
                    LockState::Pending => await_lock(c, id)?,
                };
                if !locked {
                    odd += 1;
                }
            }
            Step::Start => {
                let id = claim.unwrap_or_default();
                let idle: Vec<i64> = harvesters
                    .iter()
                    .copied()
                    .filter(|h| !running.contains(h))
                    .collect();
                let first = running.is_empty();
                for h in idle {
                    match c.activate(h, MINING_LASER, Some(id), crate::REPEAT_UNTIL_STOPPED) {
                        Ok(_) => {
                            running.insert(h);
                        }
                        Err(Error::Refused(why)) => {
                            if why.contains("NotEnoughCargoSpace") {
                                out.full = true;
                            } else {
                                odd += 1;
                            }
                            // A harvester that will not start is left idle
                            // for this claim: the rest run.
                            running.insert(h);
                        }
                        Err(e) => return Err(e),
                    }
                }
                if first {
                    out.claims += 1;
                }
            }
            Step::Wait => {
                for e in c.pump(POLL * 4)? {
                    let Event::Push(name, args) = e else { continue };
                    let Some(g) = GodmaEffect::from_push(&name, &args) else {
                        continue;
                    };
                    if !harvesters.contains(&g.item_id) || g.start {
                        continue;
                    }
                    running.remove(&g.item_id);
                    match StopReason::from_key(g.error.as_deref()) {
                        StopReason::HoldFull => out.full = true,
                        StopReason::Depleted => {
                            (claim, locked) = (None, false);
                            running.clear();
                        }
                        StopReason::TooFar | StopReason::Stopped => {}
                        StopReason::Other(_) => odd += 1,
                    }
                }
            }
        }
    }
    for &h in harvesters {
        let _ = c.deactivate(h, MINING_LASER.0);
    }
    Ok(out)
}

/// The grid's ice rocks as `(ball id, surface gap from the ship)`.
pub fn rocks_in(c: &mut Conn) -> Vec<(i64, f64)> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    world
        .balls()
        .filter(|b| b.slim.and_then(|s| s.type_id).is_some_and(is_ice))
        .filter_map(|b| Some((b.ball_id, world.surface_distance(ego, b.ball_id)?)))
        .collect()
}

/// Read pushes until an `OnTarget('add', id)` (`true`) or a loss or the
/// ceiling (`false`).
fn await_lock(c: &mut Conn, id: i64) -> Result<bool> {
    let deadline = Instant::now() + CEILING;
    while Instant::now() < deadline {
        for e in c.pump(POLL)? {
            let Event::Push(name, args) = e else { continue };
            if name != "OnTarget" || args.get(1) != Some(&PyValue::Int(id)) {
                continue;
            }
            match args.first().and_then(crate::text).as_deref() {
                Some("add") => return Ok(true),
                Some("lost") => return Ok(false),
                _ => {}
            }
        }
    }
    Ok(false)
}

/// Fly back to `home` in this system and dock, by the courier's
/// [`next_move`].
fn fly_home(c: &mut Conn, home: i64) -> Result<()> {
    for _ in 0..crate::courier::MAX_MOVES {
        let here = Here {
            docked_at: c.session().station_id(),
            bound: c.ballpark().is_some() && c.world().has_state(),
            gap: gap(c, home),
            warping: warping(c),
        };
        match next_move(&here, home) {
            Move::Arrived => return Ok(()),
            Move::Undock => {
                c.undock_repaired()?;
            }
            Move::Bind => leave(c)?,
            Move::Wait => settle(c, |c| !warping(c))?,
            Move::Warp => {
                c.warp_to_item(home, 0.0)?;
                settle(c, warping)?;
                settle(c, |c| !warping(c))?;
            }
            Move::Approach => {
                c.follow(home, 0.0)?;
                settle(c, |c| gap(c, home).is_some_and(|g| g < DOCK_RANGE))?;
            }
            Move::Dock => {
                c.dock(home)?;
            }
            Move::NotHere => {
                return Err(Error::NotSent(format!(
                    "ice: home station {home} is not in this system's park"
                )));
            }
        }
    }
    Err(Error::Refused(format!("ice: not docked at {home}")))
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
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
                "ice: still moving after the travel ceiling".into(),
            ));
        }
        c.pump(POLL)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Place, Policy, ShipInfo};

    /// Invented ids.
    const STATION: i64 = 61_000_001;
    const STATION_B: i64 = 61_000_002;
    const STATION_C: i64 = 61_000_003;
    const SYS_A: i64 = 39_000_001;
    const SYS_B: i64 = 39_000_002;
    const SYS_C: i64 = 39_000_003;
    const SYS_D: i64 = 39_000_004;
    const SHIP: i64 = 100_000_301;
    const BARGE: i64 = crate::mine_ladder::RETRIEVER;
    const SPARE: i64 = 100_000_302;
    const ICE_STACK: i64 = 100_000_303;
    const OTHER_HULL: i64 = 100_000_304;
    const BELT_1: i64 = 8_100_000_101;
    const BELT_2: i64 = 8_100_000_102;

    fn row(item_id: i64, type_id: i64, group_id: i64, category_id: i64, quantity: i64) -> ItemRow {
        ItemRow {
            item_id,
            type_id,
            owner_id: 90_000_001,
            location_id: STATION,
            flag_id: crate::FLAG_CARGO,
            quantity,
            group_id,
            category_id,
            singleton: category_id == crate::courier::CATEGORY_SHIP,
        }
    }

    fn docked_obs(ship_type: Option<i64>) -> Observation {
        let mut o = Observation::empty(Instant::now());
        o.place = Place::Docked { station: STATION };
        o.ship = Some(ShipInfo {
            ship_id: SHIP,
            type_id: ship_type,
        });
        o
    }

    fn memory(name: &str) -> Memory {
        Memory::new(
            name,
            Policy {
                role: &ROLE,
                ..Policy::fast()
            },
        )
    }

    /// A line of systems A - B - C - D, a station in B and in C (and one in
    /// A, home), every system at security 1.
    fn line() -> Map {
        Map::new()
            .with_gate(SYS_A, 50_000_001, SYS_B, 50_000_002)
            .with_gate(SYS_B, 50_000_003, SYS_C, 50_000_004)
            .with_gate(SYS_C, 50_000_005, SYS_D, 50_000_006)
            .with_station(STATION, SYS_A)
            .with_station(STATION_B, SYS_B)
            .with_station(STATION_C, SYS_C)
            .with_security(SYS_A, 1.0)
            .with_security(SYS_B, 1.0)
            .with_security(SYS_C, 1.0)
            .with_security(SYS_D, 1.0)
    }

    #[test]
    fn the_ice_row_is_civilian_and_named() {
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "ice");
        assert!(HARVESTERS.iter().all(|t| is_harvester(*t)));
        assert!(!is_harvester(brain::MINER_I));
        assert!(ICE_TYPES.iter().all(|t| is_ice(*t)));
        assert!(is_ice_row(&row(ICE_STACK, 1, GROUP_ICE, 25, 3)));
        assert!(!is_ice_row(&row(ICE_STACK, 1, 18, 4, 3)), "a mineral");
    }

    /// The field: here first; else the nearest system with a count and a
    /// station within reach; never without a map, past the reach, or in a
    /// system with no station.
    #[test]
    fn the_field_is_here_else_the_nearest_with_a_station() {
        let map = line();
        let trips = Some(Trips {
            map: &map,
            floor: 0.5,
        });
        let counts = |v: &[(i64, i64)]| v.iter().copied().collect::<BTreeMap<_, _>>();
        let both = counts(&[(SYS_A, 1), (SYS_C, 2), (SYS_B, 1)]);
        assert_eq!(choose_field(SYS_A, &both, trips, 5), Field::Here);
        assert_eq!(choose_field(SYS_A, &both, None, 5), Field::Here);
        let away = counts(&[(SYS_A, 0), (SYS_C, 2), (SYS_B, 1)]);
        assert_eq!(
            choose_field(SYS_A, &away, trips, 5),
            Field::There {
                system: SYS_B,
                station: STATION_B
            },
            "the nearest"
        );
        assert_eq!(choose_field(SYS_A, &away, None, 5), Field::Nowhere);
        let far = counts(&[(SYS_C, 2)]);
        assert_eq!(choose_field(SYS_A, &far, trips, 1), Field::Nowhere);
        assert_eq!(
            choose_field(SYS_A, &far, trips, 2),
            Field::There {
                system: SYS_C,
                station: STATION_C
            }
        );
        let stationless = counts(&[(SYS_D, 4)]);
        assert_eq!(choose_field(SYS_A, &stationless, trips, 5), Field::Nowhere);
        let low = Trips {
            map: &map,
            floor: 1.5,
        };
        assert_eq!(
            choose_field(SYS_A, &away, Some(low), 5),
            Field::Nowhere,
            "under the floor"
        );
    }

    /// Belts spread by seed in id order; none listed is `None`.
    #[test]
    fn a_belt_is_picked_by_seed_in_id_order() {
        assert_eq!(pick_belt(&[BELT_2, BELT_1], 0), Some(BELT_1));
        assert_eq!(pick_belt(&[BELT_2, BELT_1], 1), Some(BELT_2));
        assert_eq!(pick_belt(&[], 7), None);
    }

    /// Docked: ice in the hold is unloaded first, then the hull boarded,
    /// then a trip and a dwell.
    #[test]
    fn docked_unloads_boards_then_goes_for_ice() {
        let mut m = memory("Invented Ice Miner A");
        let mut o = docked_obs(Some(brain::MERLIN));
        o.cargo = vec![row(ICE_STACK, ICE_TYPES[0], GROUP_ICE, 25, 3)];
        o.hangar = vec![row(SPARE, BARGE, 463, crate::courier::CATEGORY_SHIP, 1)];
        assert_eq!(docked(&mut m, &o), vec![Act::Unload(vec![(ICE_STACK, 3)])]);
        // The mining hold is emptied with the cargo.
        let mut held = row(ICE_STACK + 1, ICE_TYPES[1], GROUP_ICE, 25, 2);
        held.flag_id = brain::FLAG_MINING_HOLD;
        o.ore_hold = vec![held];
        assert_eq!(
            docked(&mut m, &o),
            vec![Act::Unload(vec![(ICE_STACK, 3), (ICE_STACK + 1, 2)])]
        );
        o.cargo.clear();
        o.ore_hold.clear();
        assert_eq!(docked(&mut m, &o), vec![Act::Board(SPARE)]);
        let o = docked_obs(Some(BARGE));
        let acts = docked(&mut m, &o);
        assert_eq!(acts[0], Act::Ice);
        assert!(matches!(acts[1], Act::Wait(_)));
    }

    /// Out of the ice hull with none in the hangar: GM setup for the hull
    /// (and the harvesters in a capsule), else a `MISSING`; a run that
    /// lacked a harvester asks for one once.
    #[test]
    fn a_lacking_hull_or_harvester_is_asked_for() {
        let mut m = memory("Invented Ice Miner B");
        m.policy.gm_setup = true;
        let mut o = docked_obs(Some(brain::CAPSULE));
        o.hangar = vec![row(OTHER_HULL, brain::MERLIN, 25, 6, 1)];
        assert_eq!(
            docked(&mut m, &o),
            vec![Act::GmSetup {
                ship: true,
                module: true,
                ammo: false
            }]
        );
        let acts = docked(&mut m, &o);
        assert!(matches!(acts[0], Act::Missing(_)), "{acts:?}");

        let name = "Invented Ice Miner C";
        let mut m = memory(name);
        m.policy.gm_setup = true;
        set_note(
            name,
            Note {
                lacks_harvester: true,
                found_nothing: false,
            },
        );
        let o = docked_obs(Some(BARGE));
        assert_eq!(
            docked(&mut m, &o),
            vec![Act::GmSetup {
                ship: false,
                module: true,
                ammo: false
            }]
        );
        let mut o = docked_obs(Some(BARGE));
        o.hangar = vec![row(SPARE, ICE_HARVESTER_I, 464, 7, 1)];
        assert_eq!(docked(&mut m, &o)[0], Act::Ice, "one in the hangar now");
    }

    #[test]
    fn setup_gives_the_skill_and_what_is_lacking() {
        let lacks = |ship, module| Lacks {
            ship,
            module,
            ammo: false,
            isk: false,
        };
        assert_eq!(
            setup(lacks(false, false)),
            vec![format!("/giveskill {SKILL_ICE_HARVESTING} 1")]
        );
        let all = setup(lacks(true, true));
        let rung = crate::mine_ladder::PROCURER;
        assert!(all.contains(&format!("/createitem {rung}")));
        // The rung's own skills, from the ladder.
        assert!(all.contains(&format!(
            "/giveskill {} 1",
            crate::mine_ladder::SKILL_MINING_BARGE
        )));
        assert!(all.contains(&format!("/createitem {ICE_HARVESTER_I} {WANT_HARVESTERS}")));
    }

    /// ICE-HULL: the ice miner flies a barge or an exhumer, never an
    /// industrial, and the best held rung first.
    #[test]
    fn an_ice_hull_is_a_barge_or_an_exhumer_best_held_first() {
        use crate::mine_ladder::{HULK, PROCURER, RETRIEVER, VENTURE};
        assert!(!crate::mine_ladder::is_ice_hull(crate::hulls::BADGER));
        assert!(!crate::mine_ladder::is_ice_hull(VENTURE));
        assert!(!crate::mine_ladder::is_ice_hull(brain::MERLIN));
        let ship = |id, t| row(id, t, 463, crate::courier::CATEGORY_SHIP, 1);
        let hangar = [
            ship(1, crate::hulls::BADGER),
            ship(2, PROCURER),
            ship(3, HULK),
            ship(4, RETRIEVER),
        ];
        assert_eq!(ice_hull_in(&hangar, |_| true).map(|r| r.item_id), Some(3));
        assert_eq!(
            ice_hull_in(&hangar, |r| r.item_id != 3).map(|r| r.item_id),
            Some(4)
        );
        assert_eq!(
            ice_hull_in(&hangar[..1], |_| true),
            None,
            "a Badger is none"
        );
        // Flying a Badger, an ice miner boards the barge.
        let mut m = memory("Invented Ice Miner D");
        let mut o = docked_obs(Some(crate::hulls::BADGER));
        o.hangar = vec![ship(2, PROCURER)];
        assert_eq!(docked(&mut m, &o), vec![Act::Board(2)]);
        // A junior pilot's hull is set aside, so it is asked for, not boarded.
        m.combat.untrained.push(PROCURER);
        m.policy.gm_setup = false;
        assert!(matches!(docked(&mut m, &o)[0], Act::Missing(_)));
    }

    #[test]
    fn the_kit_is_the_hull_the_harvesters_and_the_skill() {
        assert_eq!(
            ICE_KIT.hull,
            crate::mine_ladder::ice_hulls().last().map(|h| h.type_id),
            "the placeholder is the lowest ice rung"
        );
        assert_eq!(ICE_KIT.modules, Some((ICE_HARVESTER_I, WANT_HARVESTERS)));
        assert_eq!(ICE_KIT.skills, &[(SKILL_ICE_HARVESTING, 1)]);
        assert_eq!(crate::acquire::kit_of(&ROLE), ICE_KIT);
    }

    #[test]
    fn the_summary_names_the_field_or_says_missing() {
        let none = Trip::default();
        assert!(none.summary().contains("MISSING"), "{}", none.summary());
        let t = Trip {
            belt: Some(BELT_1),
            travelled_to: Some(SYS_B),
            units: 3,
            full: true,
            ..Trip::default()
        };
        let s = t.summary();
        assert!(!s.contains("MISSING") && s.contains("hold full"), "{s}");
        let lacking = Trip {
            lacking: Some(Lack::Harvester),
            ..Trip::default()
        };
        assert_eq!(lacking.summary(), "did not go out: no Harvester");
    }
}
