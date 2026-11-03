//! **The gas huffer** (R-SOCIETY stage B, packet SOC-B2): a bot that
//! harvests gas clouds at gas sites and sells the gas, the way a player
//! does in a Venture with a Gas Cloud Scoop.
//!
//! [`ROLE`] is the `huff` row of [`crate::roles::ROLES`] and the
//! `gas_huffer` career's role. Docked, in a hull, it asks for one
//! [`Act::Huff`], whose driver arm is one [`run`]; in space outside a run
//! it goes home ([`brain::go_home`]). A [`run`] is a whole trip, dock to
//! dock, every step a player verb:
//!
//! 1. **Ready** (docked): board a hull, fit a gas harvester from the hangar
//!    ([`HARVESTERS`]) and online it. With none, the run ends lacking one
//!    ([`Lack::Harvester`]), and the next docked turn asks GM setup or the
//!    closed economy's [`Act::Acquire`] for it ([`HUFFER_KIT`]).
//! 2. **Sell** the gas in the hangar ([`sale_plan`], below).
//! 3. **Find a gas site** with the [`Finder`]: gas sites are signatures
//!    only (`content/population.tsv`'s `gas_sig_*` rows) and a signature is
//!    warped to only once scanned down to a perfect result
//!    (`signal_tracker::signature_target`). **Scanning is SOC-B3's**
//!    (`probe.rs`): the finder is a seam ([`FINDER`]), wired by merge-b to
//!    [`probe_finder`], which scans with the launcher and probes the trip
//!    fits from the hangar. Without them it finds nothing, and the run says
//!    so ([`MISSING_PROBING`]).
//! 4. **On the site's grid**: wait [`SITE_SETTLE`] for the room's balls; the
//!    gas clouds are the balls whose slim type is a gas ([`is_gas`]). No
//!    cloud: not a gas site (or a harvested one); the finder is asked again,
//!    told the sites already tried, at most [`MAX_SITES`] a trip.
//! 5. **Harvest** by [`next_step`]: claim a cloud ([`pick_cloud`]),
//!    approach it inside the harvester's range, lock it, start the
//!    harvester (`miningClouds`, repeat), and read the stop pushes: a full
//!    hold ends the trip, a cloud harvested out is the next claim.
//! 6. **Home**: fly back to the station it undocked from and dock, then move
//!    the gas from the hold to the hangar. The next trip sells it.
//!
//! **The server side** is `eve_server::mining` (effect 2726, the cloud as a
//! rock) and `eve_server::site` (`Law::cloud`). The gas lands in the cargo
//! hold: no mining hold is modelled (`mining.rs`' header).
//!
//! **What gas feeds** (R-SOCIETY §1 row 4, the honest note): nothing, yet.
//! Booster gas and fullerenes go to reactions and magmatic gas to sov fuel,
//! and neither exists until stage D. So [`sale_plan`] **sells into whoever
//! bids** -- a human, a bot, an NPC buy order if the exchange has one -- and
//! lists at the seller's ask ([`pricing::ask`]) only where the server
//! quotes a reference price. Most cloud types have a `basePrice` of 0
//! [V-data], so demand is thin, the career's income ledger shows it, and
//! few minds stay huffers until something consumes gas.
//!
//! Every rule here is **[A] policy** -- what a player would do -- except
//! the client's own calls and ids, cited [V].

use crate::brain::{self, Act, Memory, Observation, StopReason};
use crate::courier::{Here, Move, next_move};
use crate::pricing::{self, Quote};
use crate::roles::{Lacks, RoleSpec, Summary};
use crate::verbs::{Book, SellLine};
use crate::{Conn, DOCK_RANGE, Error, Event, GodmaEffect, ItemRow, LockState, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The Venture's type id **[V-data]**: ORE's mining frigate, the gas hull
/// players start in. Its 5,000 m3 mining hold is not modelled, so it
/// carries gas in its 50 m3 cargo hold for now.
pub const VENTURE: i64 = 32_880;
/// Gas Cloud Scoop I **[V-data]** (group 737): the harvester the huffer
/// buys or is given.
pub const GAS_CLOUD_SCOOP_I: i64 = 25_266;
/// Gas Cloud Harvesting **[V-data]**: the scoop's `requiredSkill1` (182).
pub const SKILL_GAS_CLOUD_HARVESTING: i64 = 25_544;
/// Every gas harvester whose default effect is `miningClouds` (2726)
/// **[V-data]**: groups 737 (scoops) and 4138 (harvesters), the abyssal
/// rolls left out.
pub const HARVESTERS: &[i64] = &[
    25_266, 25_540, 25_542, 25_812, 28_788, 60_313, 60_314, 60_315,
];
/// The harvester's effect, as `dogmaIM.Activate` names it **[V-data]**
/// (`dogmaeffects[2726].effectName`).
pub const MINING_CLOUDS: (&str, i64) = ("miningClouds", eve_server::mining::EFFECT_MINING_CLOUDS);
/// `groupHarvestableCloud` **[V]**: a hold row of this group is gas.
pub const GROUP_GAS: i64 = eve_server::mining::GROUP_HARVESTABLE_CLOUD;
/// Every harvestable cloud type **[V]** (`inventorycommon/const`
/// `typeAmberCytoserocin` ... `typeFulleriteC540`, group 711): the
/// cytoserocins, the mykoserocins and the fullerites. The bot ships no
/// type table, so a ball is a cloud by its slim's type id.
pub const GAS_TYPES: &[i64] = &[
    25_268, 25_273, 25_274, 25_275, 25_276, 25_277, 25_278, 25_279, 28_629, 28_630, 28_694, 28_695,
    28_696, 28_697, 28_698, 28_699, 28_700, 28_701, 30_370, 30_371, 30_372, 30_373, 30_374, 30_375,
    30_376, 30_377, 30_378,
];

/// What the huffer flies (E8's closed economy buys it, GM setup gives it):
/// a Venture, one Gas Cloud Scoop I and Gas Cloud Harvesting, and (SOC-B1)
/// a core probe launcher and a setup load of core probes, so a huffer
/// without GM setup can scan its gas sites down. **[A]** one scoop: a
/// Venture fits three high slots, one for the launcher; falsified by
/// huffers that idle for want of income with a free high slot.
pub const HUFFER_KIT: crate::acquire::Kit = crate::acquire::Kit {
    hull: Some(VENTURE),
    modules: Some((GAS_CLOUD_SCOOP_I, 1)),
    ammo: None,
    skills: &[(SKILL_GAS_CLOUD_HARVESTING, 1)],
    more: &[
        (crate::probe::CORE_LAUNCHER, 1),
        (crate::probe::CORE_PROBE, crate::probe::SETUP_PROBES),
    ],
};

/// How long a site's grid must be quiet before its clouds are counted.
/// **[A]**, the site runner's ([`crate::site_runner::SITE_SETTLE`]).
pub const SITE_SETTLE: Duration = crate::site_runner::SITE_SETTLE;
/// Sites the finder may land the huffer on in one trip before it goes
/// home. **[A]**: falsified by huffers that go home empty from a system
/// with a live gas site.
pub const MAX_SITES: usize = 3;
/// The longest one trip harvests. **[A]** a player's sitting; falsified by
/// huffers that go home long before their hold is full, or never.
pub const HARVEST_CEILING: Duration = Duration::from_secs(45 * 60);
/// `maxRange` **[V]** (`dogmaattributes[54]`).
pub const ATTR_MAX_RANGE: i64 = 54;
/// Harvest only inside this fraction of the harvester's range. **[A]**,
/// the miner's ([`brain::RANGE_MARGIN`]).
pub const RANGE_MARGIN: f64 = brain::RANGE_MARGIN;
/// The range kept from a cloud while approaching it. **[A]**: inside a
/// scoop's 1,500 m `maxRange` [V-data] with room to drift.
pub const APPROACH_RANGE: f64 = 500.0;
/// Clouds chosen among, by the bot's seed, so huffers spread. **[A]**
pub const SPREAD: usize = brain::SPREAD;
/// Harvester stops that are neither "full" nor "harvested out" before the
/// trip ends. **[A]**
pub const MAX_ODD_STOPS: u32 = 3;
/// How long each read waits while something runs.
const POLL: Duration = Duration::from_millis(250);
/// How long a warp, an approach or a lock may take. **[A]** the courier's.
const CEILING: Duration = crate::courier::TRAVEL_CEILING;

/// The `MISSING` line of a trip that found no gas site: none in the
/// system, or no probe launcher and probes to scan one down with.
pub const MISSING_PROBING: &str =
    "a scanned-down gas site (none here, or no probe launcher and probes)";

/// Harvests gas, and stays out of fights.
pub static ROLE: RoleSpec = RoleSpec {
    name: "huff",
    greeting: "o7 -- huffing gas, not fighting",
    civilian: true,
    docked,
    in_space,
    banner,
    setup,
};

// ---- the finder seam ------------------------------------------------------

/// **Finds a gas site and lands on it**: in space and bound, scan the
/// system's signatures down and warp to one not in `tried` (target ids),
/// returning its target id once the ship has landed; `Ok(None)` when there
/// is nothing left to try. It need not know which signature is gas: the
/// huffer reads the grid ([`clouds_in`]) and asks again.
pub type Finder = fn(&mut Conn, &[String]) -> Result<Option<String>>;

/// No scanning: nothing is found. The finder until SOC-B3's probing is
/// wired in.
pub fn no_probing(_: &mut Conn, _: &[String]) -> Result<Option<String>> {
    Ok(None)
}

/// **SOC-B3's scan-down-and-warp as a [`Finder`]** (merge-b): open the
/// probes ([`crate::probe::Scanner::open`]), scan the overlay down to the
/// nearest gas signature ([`crate::probe::Kind::Gas`]) not in `tried`,
/// warp to it and land ([`crate::probe::warp_to`]). No launcher or no
/// probes (`NotSent`) finds nothing, and the trip says so.
pub fn probe_finder(c: &mut Conn, tried: &[String]) -> Result<Option<String>> {
    use crate::probe::{self, Kind};
    let mut scanner = match probe::Scanner::open(c) {
        Ok(s) => s,
        Err(Error::NotSent(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    let found = probe::find_with(c, &mut scanner, |t, k| {
        !tried.iter().any(|x| x == t) && k.is_none_or(|k| k == Kind::Gas)
    })?;
    let Some(sig) = found else {
        return Ok(None);
    };
    probe::warp_to(c, &sig)?;
    Ok(Some(sig.target_id))
}

/// **The finder the driver's [`Act::Huff`] arm uses.** Coordinator-owned:
/// SOC-B3's scan-down-and-warp ([`probe_finder`]).
pub const FINDER: Finder = probe_finder;

// ---- what the bot remembers between trips -----------------------------------

/// What the last [`run`] of a bot left for its next docked turn, by bot
/// name: the bot-side note the courier's and site runner's boards keep
/// too. Per process; a restart forgets it, which costs one trip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Note {
    /// The last run had no harvester to fit.
    pub lacks_harvester: bool,
    /// The last run found no gas site: dwell the longer dwell.
    pub found_nothing: bool,
    /// The last run had no probe launcher or no probes to fit (SOC-B1):
    /// ask for them ([`HUFFER_KIT`]'s `more`).
    pub lacks_probing: bool,
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

/// Is `type_id` a gas cloud (and the gas it gives)?
pub fn is_gas(type_id: i64) -> bool {
    GAS_TYPES.contains(&type_id)
}

/// Is `type_id` a gas harvester this bot can run?
pub fn is_harvester(type_id: i64) -> bool {
    HARVESTERS.contains(&type_id)
}

/// Gas in a hold or hangar: a stack of a cloud type.
pub fn is_gas_row(row: &ItemRow) -> bool {
    !row.singleton && (row.group_id == GROUP_GAS || is_gas(row.type_id))
}

/// Docked: unload gas left in the hold, a hull, the harvester the last run
/// lacked, then one [`Act::Huff`] and a dwell.
fn docked(m: &mut Memory, o: &Observation) -> Vec<Act> {
    docked_with(m, o, Act::Huff)
}

/// [`docked`], asking for `trip` instead of [`Act::Huff`]: the wormhole
/// diver (`wh_dive`) flies the same hull with the same kit.
pub(crate) fn docked_with(m: &mut Memory, o: &Observation, trip: Act) -> Vec<Act> {
    if o.failures.values().any(|&n| n >= brain::GIVE_UP_AFTER) {
        return vec![Act::Reconnect];
    }
    m.claim = None;
    m.homing = None;
    let gas: Vec<(i64, i64)> = o
        .cargo
        .iter()
        .filter(|r| is_gas_row(r))
        .map(|r| (r.item_id, r.quantity))
        .collect();
    if !gas.is_empty() {
        return vec![Act::Unload(gas)];
    }
    let active = o.ship.map(|s| s.ship_id);
    let flying = o.ship.and_then(|s| s.type_id);
    let in_capsule = flying.is_none_or(|t| t == brain::CAPSULE);
    let hangar_venture = o.hangar.iter().find(|r| {
        r.type_id == VENTURE
            && Some(r.item_id) != active
            && !m.combat.untrained.contains(&r.type_id)
    });
    if flying != Some(VENTURE)
        && let Some(v) = hangar_venture
    {
        return vec![Act::Board(v.item_id)];
    }
    let note = note_of(&m.name);
    let hangar_harvester = o.hangar.iter().any(|r| is_harvester(r.type_id));
    if in_capsule {
        return brain::gm_or_missing(m, true, !hangar_harvester, false, "a Venture in the hangar");
    }
    if note.lacks_harvester && !hangar_harvester {
        return brain::gm_or_missing(m, false, true, false, "a gas harvester in the hangar");
    }
    if note.lacks_probing {
        // Asked for once: the next run looks again (a launcher bought may
        // be fitted, so the hangar alone cannot say). The rounds' slot of
        // the setup carries the launcher and probes: `setup` and
        // `HUFFER_KIT.more` both read it so.
        set_note(
            &m.name,
            Note {
                lacks_probing: false,
                ..note
            },
        );
        return brain::gm_or_missing(
            m,
            false,
            false,
            true,
            "a probe launcher and probes in the hangar",
        );
    }
    let rest = m.jitter(if note.found_nothing {
        m.policy.idle_dwell
    } else {
        m.policy.dwell
    });
    vec![trip, Act::Wait(rest)]
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

/// GM setup: the skill, and a Venture and a scoop when lacking.
pub(crate) fn setup(lacks: Lacks) -> Vec<String> {
    let mut lines = vec![format!("/giveskill {SKILL_GAS_CLOUD_HARVESTING} 1")];
    if lacks.ship {
        lines.push(format!("/createitem {VENTURE}"));
    }
    if lacks.module {
        lines.push(format!("/createitem {GAS_CLOUD_SCOOP_I}"));
    }
    if lacks.module || lacks.ammo {
        // Gas sites are signatures: the probes that find them (merge-b;
        // SOC-B1: also alone, when a run found no launcher or probes).
        lines.push(format!("/createitem {}", crate::probe::CORE_LAUNCHER));
        lines.push(format!(
            "/createitem {} {}",
            crate::probe::CORE_PROBE,
            crate::probe::SETUP_PROBES
        ));
    }
    lines
}

// ---- the harvest, pure --------------------------------------------------------

/// Where the harvest stands, as far as the next step needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Harvest {
    pub warping: bool,
    /// A stop said the hold is full.
    pub hold_full: bool,
    /// Clouds on the grid.
    pub clouds: usize,
    /// Surface distance to the claimed cloud; `None`: no claim.
    pub gap: Option<f64>,
    /// The harvester's `maxRange` (54), metres.
    pub range: f64,
    pub locked: bool,
    /// The harvester is cycling on the claim.
    pub active: bool,
}

/// One step of the harvest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The hold is full: home.
    Home,
    /// No cloud here: another site.
    NextSite,
    /// No claim: pick a cloud.
    Claim,
    Approach,
    Lock,
    Start,
    /// In warp, or the harvester runs: read the next push.
    Wait,
}

/// **The next harvest step**, in priority order: let a warp land; home
/// with a full hold; another site with no cloud; claim; approach until
/// inside [`RANGE_MARGIN`] of the range; lock; start; wait.
pub fn next_step(h: &Harvest) -> Step {
    if h.warping {
        return Step::Wait;
    }
    if h.hold_full {
        return Step::Home;
    }
    if h.clouds == 0 {
        return Step::NextSite;
    }
    match h.gap {
        None => Step::Claim,
        Some(g) if g > h.range * RANGE_MARGIN => Step::Approach,
        Some(_) if !h.locked => Step::Lock,
        Some(_) if !h.active => Step::Start,
        Some(_) => Step::Wait,
    }
}

/// **The cloud to claim**: among the [`SPREAD`] clouds nearest the ship
/// that no other ship harvests (`taken`), the one at `seed % n`, so huffers
/// with different names start apart. `clouds` is `(ball id, surface gap)`.
pub fn pick_cloud(seed: u64, clouds: &[(i64, f64)], taken: &BTreeSet<i64>) -> Option<i64> {
    let mut free: Vec<(f64, i64)> = clouds
        .iter()
        .filter(|(id, _)| !taken.contains(id))
        .map(|&(id, gap)| (gap, id))
        .collect();
    free.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    free.truncate(SPREAD);
    if free.is_empty() {
        return None;
    }
    Some(free[(seed % free.len() as u64) as usize].1)
}

// ---- the sale, pure -----------------------------------------------------------

/// What to do with one gas stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sale {
    /// Into the best bid, whoever's, at its price.
    Bid(SellLine),
    /// Listed for [`crate::escort::LIST_DAYS`] at the seller's ask.
    List(SellLine),
    /// Nobody bids and the server quotes no price: kept.
    Keep { type_id: i64, quantity: i64 },
}

/// **The huffer's sale** of `stacks` at `station` (module header): a bid of
/// at least one [`pricing::TICK`] takes the stack at the bid's price
/// (gas sells to whoever bids, [A]); else a `P_ref` lists it at
/// [`pricing::ask`]; else it is kept. `books` and `p_refs` by type id.
pub fn sale_plan(
    stacks: &[ItemRow],
    books: &BTreeMap<i64, Book>,
    p_refs: &BTreeMap<i64, f64>,
    station: i64,
) -> Vec<Sale> {
    stacks
        .iter()
        .filter(|r| is_gas_row(r) && r.quantity > 0)
        .map(|r| {
            let line = |price: f64| SellLine {
                station_id: station,
                type_id: r.type_id,
                item_id: r.item_id,
                price,
                quantity: r.quantity,
            };
            let bid = books
                .get(&r.type_id)
                .and_then(|b| b.best_bid())
                .map(|b| b.price)
                .filter(|&p| p >= pricing::TICK);
            if let Some(p) = bid {
                return Sale::Bid(line(p));
            }
            match p_refs
                .get(&r.type_id)
                .and_then(|&p| pricing::ask(&Quote::at_ref(p)))
            {
                Some(ask) => Sale::List(line(ask)),
                None => Sale::Keep {
                    type_id: r.type_id,
                    quantity: r.quantity,
                },
            }
        })
        .collect()
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
    /// The sale before leaving.
    pub sales: Vec<Sale>,
    /// Target ids of the sites landed on.
    pub sites: Vec<String>,
    /// Harvester cycles started (one per claim).
    pub claims: u32,
    /// Gas units in the hold when it docked.
    pub units: i64,
    /// The trip ended on a full hold.
    pub full: bool,
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
        let (bids, lists, kept) = self.sales.iter().fold((0, 0, 0), |(b, l, k), s| match s {
            Sale::Bid(_) => (b + 1, l, k),
            Sale::List(_) => (b, l + 1, k),
            Sale::Keep { .. } => (b, l, k + 1),
        });
        let found = if self.sites.is_empty() {
            format!("; MISSING {MISSING_PROBING}")
        } else {
            String::new()
        };
        format!(
            "sold {bids} into bids, listed {lists}, kept {kept}; sites {:?}, claims {}, {} gas units home{}; wallet {:.2} -> {:.2} ISK{found}",
            self.sites,
            self.claims,
            self.units,
            if self.full { " (hold full)" } else { "" },
            self.before,
            self.after
        )
    }
}

/// **How a trip differs from the home-system one** (Y-WH, `wh_dive`): a
/// dive leaves k-space through a wormhole, so it needs a longer sitting, a
/// per-site budget (the Sleepers of a reservoir arrive after
/// `site_spawn::DELAY_SPAWN`), a way out of J-space before the way home, and
/// a threat check that cuts a harvest short.
#[derive(Clone, Copy)]
pub struct Via {
    /// The trip's ceiling, in place of [`HARVEST_CEILING`].
    pub ceiling: Duration,
    /// The longest one site is harvested; `None`: the trip's ceiling.
    pub site_budget: Option<Duration>,
    /// Called on the site's grid before each harvest step: `true` ends the
    /// harvest and the trip (the Sleepers are here).
    pub threat: fn(&mut Conn) -> bool,
    /// Called once the harvest is over, before the flight home: the dive's
    /// way back out of J-space.
    pub before_home: fn(&mut Conn) -> Result<()>,
}

fn never(_: &mut Conn) -> bool {
    false
}

fn nothing(_: &mut Conn) -> Result<()> {
    Ok(())
}

/// The home-system trip: no budget, no threat, nothing before home.
pub const HOME_ONLY: Via = Via {
    ceiling: HARVEST_CEILING,
    site_budget: None,
    threat: never,
    before_home: nothing,
};

/// **One gas trip, dock to dock** (module header), finding sites with
/// `finder`; `name` is the bot's (its [`Note`]) and `seed` its spread.
/// Docked to start; docked at the same station at the end, unless an error
/// cut it short.
pub fn run(c: &mut Conn, finder: Finder, name: &str, seed: u64) -> Result<Trip> {
    run_via(c, finder, name, seed, &HOME_ONLY)
}

/// [`run`] under a [`Via`].
pub fn run_via(c: &mut Conn, finder: Finder, name: &str, seed: u64, via: &Via) -> Result<Trip> {
    let home = c
        .session()
        .station_id()
        .ok_or_else(|| Error::NotSent("huff: not docked".to_owned()))?;
    let mut trip = Trip::default();
    let harvester = match ready(c)? {
        Ok(h) => h,
        Err(lack) => {
            trip.lacking = Some(lack);
            set_note(
                name,
                Note {
                    lacks_harvester: lack == Lack::Harvester,
                    ..Note::default()
                },
            );
            return Ok(trip);
        }
    };
    // A probe launcher and probes from the hangar, if there are any: what
    // [`FINDER`] scans with (merge-b). None is no error: the finder then
    // finds nothing, the trip says so, and the next docked turn asks for
    // them (SOC-B1).
    let lacks_probing = match crate::probe::outfit(c) {
        Ok(_) => false,
        Err(Error::NotSent(_) | Error::Refused(_)) => true,
        Err(e) => return Err(e),
    };
    trip.before = c.cash_balance()?;
    trip.sales = sell(c, home)?;
    trip.after = c.cash_balance()?;

    leave(c)?;
    // Off the harvester's own row; a row without it harvests from the
    // approach range.
    let range = c
        .module_row(harvester)?
        .attribute(ATTR_MAX_RANGE)
        .unwrap_or(APPROACH_RANGE / RANGE_MARGIN);
    let started = Instant::now();
    while trip.sites.len() < MAX_SITES && !trip.full && started.elapsed() < via.ceiling {
        let Some(site) = finder(c, &trip.sites)? else {
            break;
        };
        trip.sites.push(site);
        settle_for(c, SITE_SETTLE)?;
        let out = harvest(c, harvester, range, seed, started, via)?;
        trip.claims += out.claims;
        trip.full = out.full;
        if out.fled {
            break;
        }
    }
    let found_nothing = trip.sites.is_empty();
    (via.before_home)(c)?;
    fly_home(c, home)?;
    for row in c.cargo()? {
        if is_gas_row(&row) {
            trip.units += row.quantity;
            c.unload(row.item_id, None)?;
        }
    }
    set_note(
        name,
        Note {
            lacks_harvester: false,
            found_nothing,
            lacks_probing,
        },
    );
    Ok(trip)
}

/// Docked: in a hull, a harvester fitted and online. `Ok(Err(lack))` for
/// what is missing; `Ok(Ok(module id))` ready.
pub(crate) fn ready(c: &mut Conn) -> Result<std::result::Result<i64, Lack>> {
    let hangar = c.hangar_items()?;
    let active = c.session().ship_id();
    let flying = hangar
        .iter()
        .find(|r| Some(r.item_id) == active)
        .map(|r| r.type_id);
    if flying.is_none_or(|t| t == brain::CAPSULE) {
        // SKILLGATE: only a hull the pilot's skills board.
        let levels = crate::acquire::gate_levels(c)?;
        let fly = |r: &&ItemRow| crate::acquire::flyable_in(&levels, r.type_id);
        let hull = hangar
            .iter()
            .filter(fly)
            .find(|r| r.type_id == VENTURE)
            .or_else(|| {
                hangar.iter().filter(fly).find(|r| {
                    r.category_id == crate::courier::CATEGORY_SHIP && r.type_id != brain::CAPSULE
                })
            });
        match hull {
            Some(h) => {
                c.board(h.item_id)?;
            }
            None => {
                if let (Some(r), Some(l)) = (
                    hangar.iter().find(|r| r.type_id == VENTURE),
                    levels.as_ref(),
                ) {
                    let _ = crate::acquire::learn_urgent_soft(
                        c,
                        &crate::acquire::lacking_of(l, r.type_id),
                    )?;
                }
                return Ok(Err(Lack::Hull));
            }
        }
    }
    let mut fitted = c.fitted()?.into_iter().find(|r| is_harvester(r.type_id));
    if fitted.is_none() {
        // FIT-REFUSE: only a harvester the hull takes (some name hulls this
        // Venture is not), or one refused would block the rest every pass.
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
            .take(1)
            .collect();
        if spare.is_empty() {
            return Ok(Err(Lack::Harvester));
        }
        // SKILLGATE: a harvester the skills do not fit yet is a lack, not a fault.
        let Ok(rows) = crate::acquire::fit_auto_gated(c, &spare)? else {
            return Ok(Err(Lack::Harvester));
        };
        fitted = rows.into_iter().find(|r| is_harvester(r.type_id));
    }
    let Some(h) = fitted else {
        return Ok(Err(Lack::Harvester));
    };
    if !c.is_online(h.item_id)? && crate::acquire::set_online_gated(c, h.item_id)?.is_err() {
        return Ok(Err(Lack::Harvester));
    }
    Ok(Ok(h.item_id))
}

/// Sell the hangar's gas by [`sale_plan`]: the bids in one call, each
/// listing in its own.
pub(crate) fn sell(c: &mut Conn, station: i64) -> Result<Vec<Sale>> {
    let stacks: Vec<ItemRow> = c.hangar_items()?.into_iter().filter(is_gas_row).collect();
    if stacks.is_empty() {
        return Ok(Vec::new());
    }
    let mut prices = pricing::Prices::read(c)?;
    let mut books = BTreeMap::new();
    for r in &stacks {
        if let std::collections::btree_map::Entry::Vacant(e) = books.entry(r.type_id) {
            e.insert(c.market_orders(r.type_id)?);
        }
    }
    let listing = {
        let types: Vec<i64> = stacks.iter().map(|r| r.type_id).collect();
        prices.values_of(c, &types)?
    };
    let plan = sale_plan(&stacks, &books, &listing, station);
    let bids: Vec<SellLine> = plan
        .iter()
        .filter_map(|s| match s {
            Sale::Bid(l) => Some(*l),
            _ => None,
        })
        .collect();
    if !bids.is_empty() {
        c.sell(&bids, 0, crate::escort::LIST_BROKER_FEE)?;
    }
    for s in &plan {
        if let Sale::List(l) = s {
            c.sell(
                &[*l],
                crate::escort::LIST_DAYS,
                crate::escort::LIST_BROKER_FEE,
            )?;
        }
    }
    Ok(plan)
}

/// Undock and bind the system's park.
pub(crate) fn leave(c: &mut Conn) -> Result<()> {
    if c.session().station_id().is_some() {
        c.undock_repaired()?;
    }
    let system = c
        .session()
        .solar_system_id()
        .ok_or_else(|| Error::NotSent("huff: in space with no system".to_owned()))?;
    if c.ballpark().is_none() {
        c.bind_ballpark(system)?;
    }
    settle(c, |c| c.world().has_state())
}

/// What one site's harvest did.
struct Harvested {
    claims: u32,
    full: bool,
    /// The threat check fired: the trip ends.
    fled: bool,
}

/// **Harvest the grid's clouds** by [`next_step`] until the hold is full,
/// the clouds are gone, the stops pile up, or the trip's ceiling passes.
fn harvest(
    c: &mut Conn,
    harvester: i64,
    range: f64,
    seed: u64,
    started: Instant,
    via: &Via,
) -> Result<Harvested> {
    let mut out = Harvested {
        claims: 0,
        full: false,
        fled: false,
    };
    let (mut claim, mut locked, mut active) = (None::<i64>, false, false);
    let mut odd = 0;
    let here = Instant::now();
    while started.elapsed() < via.ceiling
        && via.site_budget.is_none_or(|b| here.elapsed() < b)
        && odd < MAX_ODD_STOPS
    {
        if (via.threat)(c) {
            out.fled = true;
            break;
        }
        let clouds = clouds_in(c);
        if claim.is_some_and(|id| !clouds.iter().any(|(c, _)| *c == id)) {
            (claim, locked, active) = (None, false, false);
        }
        let h = Harvest {
            warping: warping(c),
            hold_full: out.full,
            clouds: clouds.len(),
            gap: claim.and_then(|id| clouds.iter().find(|(c, _)| *c == id).map(|(_, g)| *g)),
            range,
            locked,
            active,
        };
        match next_step(&h) {
            Step::Home | Step::NextSite => break,
            Step::Claim => {
                claim = pick_cloud(seed, &clouds, &BTreeSet::new());
            }
            Step::Approach => {
                let id = claim.unwrap_or_default();
                c.follow(id, APPROACH_RANGE)?;
                settle(c, |c| {
                    clouds_in(c)
                        .iter()
                        .find(|(cid, _)| *cid == id)
                        .is_none_or(|(_, g)| *g <= range * RANGE_MARGIN)
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
                match c.activate(
                    harvester,
                    MINING_CLOUDS,
                    Some(id),
                    crate::REPEAT_UNTIL_STOPPED,
                ) {
                    Ok(_) => {
                        active = true;
                        out.claims += 1;
                    }
                    Err(Error::Refused(why)) => {
                        // `NotEnoughCargoSpace` is the full hold's refusal
                        // (`StopReason::from_key`).
                        if why.contains("NotEnoughCargoSpace") {
                            out.full = true;
                        } else {
                            odd += 1;
                            claim = None;
                            locked = false;
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            Step::Wait => {
                for e in c.pump(POLL * 4)? {
                    let Event::Push(name, args) = e else { continue };
                    let Some(g) = GodmaEffect::from_push(&name, &args) else {
                        continue;
                    };
                    if g.item_id != harvester || g.start {
                        continue;
                    }
                    active = false;
                    match StopReason::from_key(g.error.as_deref()) {
                        StopReason::HoldFull => out.full = true,
                        StopReason::Depleted => (claim, locked) = (None, false),
                        StopReason::TooFar | StopReason::Stopped => {}
                        StopReason::Other(_) => odd += 1,
                    }
                }
            }
        }
    }
    if active {
        let _ = c.deactivate(harvester, MINING_CLOUDS.0);
    }
    Ok(out)
}

/// The grid's gas clouds as `(ball id, surface gap from the ship)`.
pub fn clouds_in(c: &mut Conn) -> Vec<(i64, f64)> {
    let world = c.world_now();
    let Some(ego) = world.ego else {
        return Vec::new();
    };
    world
        .balls()
        .filter(|b| b.slim.and_then(|s| s.type_id).is_some_and(is_gas))
        .filter_map(|b| Some((b.ball_id, world.surface_distance(ego, b.ball_id)?)))
        .collect()
}

/// Read pushes until an `OnTarget('add', id)` (`True`) or a loss or the
/// ceiling (`false`).
fn await_lock(c: &mut Conn, id: i64) -> Result<bool> {
    let deadline = Instant::now() + CEILING;
    while Instant::now() < deadline {
        for e in c.pump(POLL)? {
            let Event::Push(name, args) = e else { continue };
            if name != "OnTarget" || args.get(1) != Some(&eve_marshal::PyValue::Int(id)) {
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
pub(crate) fn fly_home(c: &mut Conn, home: i64) -> Result<()> {
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
                    "huff: home station {home} is not in this system's park"
                )));
            }
        }
    }
    Err(Error::Refused(format!("huff: not docked at {home}")))
}

fn gap(c: &mut Conn, target: i64) -> Option<f64> {
    let world = c.world_now();
    world.surface_distance(world.ego?, target)
}

fn warping(c: &mut Conn) -> bool {
    c.world_now().ego_ball().is_some_and(|b| b.warp_commanded())
}

/// Read pushes for `d`.
pub(crate) fn settle_for(c: &mut Conn, d: Duration) -> Result<()> {
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
                "huff: still moving after the travel ceiling".into(),
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
    const SHIP: i64 = 100_000_201;
    const SPARE: i64 = 100_000_202;
    const GAS_STACK: i64 = 100_000_203;
    const A: i64 = 8_700_000_001;
    const B: i64 = 8_700_000_002;
    const C: i64 = 8_700_000_003;

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

    fn harvest(gap: Option<f64>) -> Harvest {
        Harvest {
            warping: false,
            hold_full: false,
            clouds: 2,
            gap,
            range: 1_000.0,
            locked: false,
            active: false,
        }
    }

    #[test]
    fn the_huff_row_is_civilian_and_named() {
        assert!(ROLE.civilian);
        assert_eq!(ROLE.name(), "huff");
        assert!(HARVESTERS.iter().all(|t| is_harvester(*t)));
        assert!(!is_harvester(brain::MINER_I));
    }

    /// The harvest rules as a table, in priority order.
    #[test]
    fn the_harvest_steps_follow_their_priority() {
        let mut h = harvest(None);
        h.warping = true;
        h.hold_full = true;
        assert_eq!(next_step(&h), Step::Wait, "a warp lands first");
        h.warping = false;
        assert_eq!(next_step(&h), Step::Home);
        let mut h = harvest(None);
        h.clouds = 0;
        assert_eq!(next_step(&h), Step::NextSite);
        assert_eq!(next_step(&harvest(None)), Step::Claim);
        let far = harvest(Some(h.range * RANGE_MARGIN + 1.0));
        assert_eq!(next_step(&far), Step::Approach);
        let near = harvest(Some(h.range * RANGE_MARGIN));
        assert_eq!(next_step(&near), Step::Lock);
        let locked = Harvest {
            locked: true,
            ..near
        };
        assert_eq!(next_step(&locked), Step::Start);
        let running = Harvest {
            active: true,
            ..locked
        };
        assert_eq!(next_step(&running), Step::Wait);
    }

    /// Taken clouds are never picked; the seed spreads over the nearest
    /// rest; nothing free is `None`.
    #[test]
    fn a_cloud_is_picked_by_seed_among_the_free_nearest() {
        let clouds = [(A, 30.0), (B, 10.0), (C, 20.0)];
        let none = BTreeSet::new();
        assert_eq!(pick_cloud(0, &clouds, &none), Some(B), "nearest first");
        assert_eq!(pick_cloud(1, &clouds, &none), Some(C));
        assert_eq!(pick_cloud(2, &clouds, &none), Some(A));
        let taken = BTreeSet::from([B]);
        assert_eq!(pick_cloud(0, &clouds, &taken), Some(C));
        assert_eq!(pick_cloud(0, &clouds, &BTreeSet::from([A, B, C])), None);
        assert_eq!(pick_cloud(0, &[], &none), None);
    }

    fn book(bid: Option<f64>) -> Book {
        Book {
            asks: Vec::new(),
            bids: bid
                .map(|price| crate::verbs::BookOrder {
                    order_id: 1,
                    type_id: 0,
                    station_id: STATION,
                    price,
                    vol_remaining: 1_000,
                    bid: true,
                })
                .into_iter()
                .collect(),
        }
    }

    /// Gas goes into whoever's bid at its price; with no bid it is listed
    /// at the seller's ask on a `P_ref`; with neither it is kept. A
    /// non-gas stack is never offered.
    #[test]
    fn gas_sells_into_bids_lists_on_a_price_or_is_kept() {
        let gas = |id, t| row(id, t, GROUP_GAS, 2, 40);
        let stacks = [
            gas(GAS_STACK, GAS_TYPES[0]),
            gas(GAS_STACK + 1, GAS_TYPES[1]),
            gas(GAS_STACK + 2, GAS_TYPES[2]),
            row(SPARE, brain::MINER_I, 54, 7, 1),
        ];
        let books = BTreeMap::from([
            (GAS_TYPES[0], book(Some(12.5))),
            (GAS_TYPES[1], book(None)),
            (GAS_TYPES[2], book(None)),
        ]);
        let refs = BTreeMap::from([(GAS_TYPES[1], 100.0)]);
        let plan = sale_plan(&stacks, &books, &refs, STATION);
        assert_eq!(plan.len(), 3, "the laser is not gas");
        match plan[0] {
            Sale::Bid(l) => assert_eq!((l.price, l.quantity, l.station_id), (12.5, 40, STATION)),
            other => panic!("{other:?}"),
        }
        match plan[1] {
            Sale::List(l) => {
                assert_eq!(Some(l.price), pricing::ask(&Quote::at_ref(100.0)));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            plan[2],
            Sale::Keep {
                type_id: GAS_TYPES[2],
                quantity: 40
            }
        );
    }

    /// Docked: gas in the hold is unloaded first; a Venture in the hangar
    /// is boarded; in a hull, one huff and a rest.
    #[test]
    fn docked_unloads_boards_then_huffs() {
        let mut m = memory("huffer-a");
        let mut o = docked_obs(Some(VENTURE));
        o.cargo = vec![row(GAS_STACK, GAS_TYPES[0], GROUP_GAS, 2, 7)];
        assert_eq!(docked(&mut m, &o), vec![Act::Unload(vec![(GAS_STACK, 7)])]);

        let mut o = docked_obs(Some(brain::MERLIN));
        o.hangar = vec![row(SPARE, VENTURE, 237, crate::courier::CATEGORY_SHIP, 1)];
        assert_eq!(docked(&mut m, &o), vec![Act::Board(SPARE)]);

        let acts = docked(&mut m, &docked_obs(Some(VENTURE)));
        assert_eq!(acts.first(), Some(&Act::Huff));
    }

    /// A run that lacked a harvester makes the next docked turn ask for one
    /// (GM setup here), and a harvester in the hangar ends the ask.
    #[test]
    fn a_lacking_harvester_is_asked_for_once_noted() {
        let name = "huffer-b";
        let mut m = memory(name);
        set_note(
            name,
            Note {
                lacks_harvester: true,
                ..Note::default()
            },
        );
        let acts = docked(&mut m, &docked_obs(Some(VENTURE)));
        assert_eq!(
            acts,
            vec![Act::GmSetup {
                ship: false,
                module: true,
                ammo: false
            }]
        );
        let mut o = docked_obs(Some(VENTURE));
        o.hangar = vec![row(SPARE, GAS_CLOUD_SCOOP_I, 737, 7, 1)];
        assert_eq!(docked(&mut m, &o).first(), Some(&Act::Huff));
    }

    #[test]
    fn setup_gives_the_skill_and_what_is_lacking() {
        let lacks = |ship, module| Lacks {
            ship,
            module,
            ammo: false,
            isk: false,
        };
        let all = setup(lacks(true, true));
        assert_eq!(all.len(), 5);
        assert!(
            all.iter()
                .any(|l| l.contains(&crate::probe::CORE_LAUNCHER.to_string()))
        );
        assert!(all[0].contains(&SKILL_GAS_CLOUD_HARVESTING.to_string()));
        assert_eq!(setup(lacks(false, false)).len(), 1);
        let probing = setup(Lacks {
            ammo: true,
            ..lacks(false, false)
        });
        assert_eq!(
            probing.len(),
            3,
            "the skill, a launcher, probes: {probing:?}"
        );
    }

    /// A run that found no launcher or probes makes the next docked turn
    /// ask for them (the rounds' slot) once; the turn after huffs, and the
    /// run looks again (SOC-B1).
    #[test]
    fn lacking_probes_are_asked_for_once_noted() {
        let name = "huffer-c";
        let mut m = memory(name);
        set_note(
            name,
            Note {
                lacks_probing: true,
                ..Note::default()
            },
        );
        let mut o = docked_obs(Some(VENTURE));
        o.hangar = vec![row(SPARE, GAS_CLOUD_SCOOP_I, 737, 7, 1)];
        assert_eq!(
            docked(&mut m, &o),
            vec![Act::GmSetup {
                ship: false,
                module: false,
                ammo: true
            }]
        );
        assert_eq!(
            docked(&mut m, &o).first(),
            Some(&Act::Huff),
            "asked once; the next run looks again"
        );
    }

    #[test]
    fn the_kit_is_a_venture_a_scoop_and_the_skill() {
        assert_eq!(HUFFER_KIT.hull, Some(VENTURE));
        assert_eq!(HUFFER_KIT.modules, Some((GAS_CLOUD_SCOOP_I, 1)));
        assert!(HUFFER_KIT.skills.contains(&(SKILL_GAS_CLOUD_HARVESTING, 1)));
        let more: Vec<i64> = HUFFER_KIT.more.iter().map(|&(t, _)| t).collect();
        assert_eq!(
            more,
            [crate::probe::CORE_LAUNCHER, crate::probe::CORE_PROBE]
        );
    }
}
